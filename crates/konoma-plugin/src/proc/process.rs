//! Internal child supervision for the SDK, konoma and konoma-plugin-test.
//! This module is not part of the SDK's stable API.

use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::process::{Child, ExitStatus, Output};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{Duration, Instant};

pub(crate) fn signal_group(pid: u32, signal: libc::c_int) {
    // SAFETY: callers supply the group of a child they started, never a name match.
    unsafe {
        libc::kill(-(pid as libc::pid_t), signal);
    }
}

/// Observe an exit without reaping: the leader must reserve its pid until group cleanup.
pub fn poll_exit(child: &Child) -> io::Result<Option<ExitStatus>> {
    use std::os::unix::process::ExitStatusExt;
    loop {
        // SAFETY: waitid writes one initialized siginfo; WNOWAIT never reaps this owned child.
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        let result = unsafe {
            libc::waitid(
                libc::P_PID,
                child.id() as libc::id_t,
                &mut info,
                libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
            )
        };
        if result == -1 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        // SAFETY: these fields are initialized by waitid, or zero when no child has exited.
        if unsafe { info.si_pid() } == 0 {
            return Ok(None);
        }
        let status = unsafe { info.si_status() };
        let raw = match info.si_code {
            libc::CLD_EXITED => status << 8,
            libc::CLD_DUMPED => status | 0x80,
            _ => status,
        };
        return Ok(Some(ExitStatus::from_raw(raw)));
    }
}

/// Stop the owned group and reap its direct child, including after a natural leader exit.
pub fn stop_group(child: &mut Child, grace: Duration) {
    stop_group_with(child, grace, signal_group);
}

fn stop_group_with(child: &mut Child, grace: Duration, mut signal: impl FnMut(u32, libc::c_int)) {
    // WNOWAIT keeps the leader alive or a zombie until the final group signal. An earlier
    // Child::wait/try_wait forfeits that reservation, so never signal such a Child.
    let exited = match poll_exit(child) {
        Ok(status) => status.is_some(),
        Err(_) => {
            let _ = child.wait();
            return;
        }
    };
    let pid = child.id();
    // A zombie cannot use a TERM grace. A zombie-only group's kill(-pid, 0) gives EPERM on
    // macOS and succeeds on Linux, so neither result tells us whether live members remain.
    if !exited && !grace.is_zero() {
        signal(pid, libc::SIGTERM);
        let until = Instant::now() + grace;
        while Instant::now() < until {
            // SAFETY: signal zero only queries the owned group, including its remaining members.
            if unsafe { libc::kill(-(pid as libc::pid_t), 0) } == -1
                && io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    signal(pid, libc::SIGKILL);
    let _ = child.wait();
}

/// Wait without collecting pipes. The caller must consume or redirect any piped streams.
pub fn wait(child: &mut Child, timeout: Duration) -> io::Result<ExitStatus> {
    let start = Instant::now();
    let result = loop {
        match poll_exit(child) {
            Ok(Some(status)) => break Ok(status),
            Err(error) => break Err(error),
            Ok(None) if start.elapsed() >= timeout => {
                break Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "child did not exit",
                ))
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
        }
    };
    stop_group(child, Duration::ZERO);
    result
}

fn nonblocking(pipe: &impl AsRawFd) -> io::Result<()> {
    let fd = pipe.as_raw_fd();
    // SAFETY: preserve the flags on this live, owned descriptor.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags == -1 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[derive(Clone)]
pub struct Cancellation {
    local: Arc<AtomicBool>,
    external: fn() -> bool,
}

impl Default for Cancellation {
    fn default() -> Self {
        Self::with_external(|| false)
    }
}

impl Cancellation {
    pub fn with_external(external: fn() -> bool) -> Self {
        Self {
            local: Arc::new(AtomicBool::new(false)),
            external,
        }
    }
    pub fn cancel(&self) {
        self.local.store(true, Ordering::SeqCst);
    }
    pub fn is_cancelled(&self) -> bool {
        self.local.load(Ordering::SeqCst) || (self.external)()
    }
    fn check(&self) -> io::Result<()> {
        if self.is_cancelled() {
            Err(io::Error::new(
                io::ErrorKind::ConnectionAborted,
                "process I/O cancelled",
            ))
        } else {
            Ok(())
        }
    }
}

/// A bounded-wait adapter: read_exact/write_all can be cancelled even when an escaped process
/// retains the other end of the pipe. ConnectionAborted is not retried by those standard loops.
pub struct CancellablePipe<T> {
    pipe: T,
    cancel: Cancellation,
    final_reads: usize,
}

impl<T: AsRawFd> CancellablePipe<T> {
    pub fn new(pipe: T, cancel: Cancellation) -> io::Result<Self> {
        nonblocking(&pipe)?;
        Ok(Self {
            pipe,
            cancel,
            final_reads: 16,
        })
    }
}

impl<T: Read> Read for CancellablePipe<T> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            if self.cancel.is_cancelled() {
                // Preserve already buffered diagnostics, without waiting for another byte or
                // allowing a continuously writing peer to postpone cancellation indefinitely.
                if self.final_reads > 0 {
                    self.final_reads -= 1;
                    if let Ok(n) = self.pipe.read(buf) {
                        if n > 0 {
                            return Ok(n);
                        }
                    }
                }
                return Err(io::Error::new(
                    io::ErrorKind::ConnectionAborted,
                    "process I/O cancelled",
                ));
            }
            match self.pipe.read(buf) {
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                    ) =>
                {
                    std::thread::sleep(Duration::from_millis(10))
                }
                result => return result,
            }
        }
    }
}

impl<T: Write> Write for CancellablePipe<T> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        loop {
            self.cancel.check()?;
            match self.pipe.write(buf) {
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                    ) =>
                {
                    std::thread::sleep(Duration::from_millis(10))
                }
                result => return result,
            }
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        self.cancel.check()?;
        self.pipe.flush()
    }
}

pub struct Capture {
    pub output: Output,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
}

fn read_batch(pipe: &mut impl Read, bytes: &mut Vec<u8>, cap: usize) -> io::Result<(bool, bool)> {
    for _ in 0..16 {
        let mut buf = [0; 8192];
        match pipe.read(&mut buf) {
            Ok(0) => return Ok((true, false)),
            Ok(n) => {
                let keep = n.min(cap.saturating_sub(bytes.len()));
                bytes.extend_from_slice(&buf[..keep]);
                if keep < n {
                    return Ok((true, true));
                }
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok((false, false)),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok((false, false))
}

/// Collect both streams without blocking on their holders. Every return path stops the group,
/// including successful and failed natural exits, and errors while setting up the pipes.
pub fn collect_output(
    child: &mut Child,
    cancelled: impl Fn() -> bool,
    timeout: Option<Duration>,
    cap: usize,
    grace: Duration,
) -> io::Result<Capture> {
    let start = Instant::now();
    let mut stopped = false;
    let result = (|| {
        let mut out = child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("missing stdout pipe"))?;
        let mut err = child
            .stderr
            .take()
            .ok_or_else(|| io::Error::other("missing stderr pipe"))?;
        nonblocking(&out)?;
        nonblocking(&err)?;
        let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
        let (mut out_done, mut err_done) = (false, false);
        let mut status = None;
        loop {
            if cancelled() {
                return Err(io::Error::new(io::ErrorKind::Interrupted, "interrupted"));
            }
            if timeout.is_some_and(|limit| start.elapsed() >= limit) {
                return Err(io::Error::new(io::ErrorKind::TimedOut, "timed out"));
            }
            if status.is_none() {
                status = poll_exit(child)?;
            }
            let mut stdout_truncated = false;
            let mut stderr_truncated = false;
            if !out_done {
                (out_done, stdout_truncated) = read_batch(&mut out, &mut stdout, cap)?;
            }
            if !err_done {
                (err_done, stderr_truncated) = read_batch(&mut err, &mut stderr, cap)?;
            }
            if stdout_truncated || stderr_truncated {
                stop_group(child, Duration::ZERO);
                stopped = true;
                return Ok(Capture {
                    output: Output {
                        status: child.wait()?,
                        stdout,
                        stderr,
                    },
                    stdout_truncated,
                    stderr_truncated,
                });
            }
            if let Some(status) = status {
                if out_done && err_done {
                    return Ok(Capture {
                        output: Output {
                            status,
                            stdout,
                            stderr,
                        },
                        stdout_truncated,
                        stderr_truncated,
                    });
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    })();
    if !stopped {
        stop_group(child, grace);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixStream;
    use std::process::Command;

    fn stop_an_observed_exit(with_member: bool) {
        use std::os::unix::process::{CommandExt, ExitStatusExt};
        use std::process::Stdio;
        let mut leader = Command::new("/bin/cat")
            .process_group(0)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let member = with_member.then(|| {
            let mut member = Command::new("/bin/cat")
                .process_group(leader.id() as i32)
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap();
            let pid = member.id();
            // Keep stdin open until after cleanup, so EOF cannot end the member by itself.
            let input = member.stdin.take().unwrap();
            let (tx, rx) = std::sync::mpsc::channel();
            let waiter = std::thread::spawn(move || {
                tx.send(member.wait()).unwrap();
            });
            (pid, input, rx, waiter)
        });
        drop(leader.stdin.take());
        let until = Instant::now() + Duration::from_secs(10);
        while poll_exit(&leader).unwrap().is_none() {
            assert!(Instant::now() < until, "leader did not exit");
            std::thread::yield_now();
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            stop_group(&mut leader, Duration::from_secs(30));
            assert_eq!(
                poll_exit(&leader).unwrap_err().raw_os_error(),
                Some(libc::ECHILD)
            );
            tx.send(()).unwrap();
        });
        let completed = rx.recv_timeout(Duration::from_secs(10));
        // Also join on the old implementation: its finite grace ends and reaps the leader.
        worker.join().unwrap();
        if let Some((pid, input, exited, waiter)) = member {
            let status = exited.recv_timeout(Duration::from_secs(5));
            if status.is_err() {
                // SAFETY: this is the exact member pid recorded at spawn, never a name match.
                unsafe {
                    libc::kill(pid as i32, libc::SIGKILL);
                }
            }
            waiter.join().unwrap();
            drop(input);
            assert_eq!(
                status
                    .expect("cleanup left the member alive")
                    .unwrap()
                    .signal(),
                Some(libc::SIGKILL)
            );
        }
        completed.expect("an observed exit must not spend the termination grace on a zombie");
    }

    #[test]
    fn an_observed_exit_skips_the_termination_grace() {
        stop_an_observed_exit(false);
    }

    #[test]
    fn an_observed_exit_still_kills_its_live_group_member() {
        stop_an_observed_exit(true);
    }

    #[test]
    fn a_reaped_leader_never_signals_its_former_group() {
        use std::os::unix::process::CommandExt;
        let mut child = Command::new("/bin/sh")
            .args(["-c", "exit 0"])
            .process_group(0)
            .spawn()
            .unwrap();
        child.wait().unwrap();
        let mut signals = Vec::new();
        stop_group_with(&mut child, Duration::ZERO, |pid, signal| {
            signals.push((pid, signal))
        });
        assert!(
            signals.is_empty(),
            "a reaped pid no longer reserves its group id: {signals:?}"
        );
    }

    #[test]
    fn cancellation_drains_available_tail_without_waiting_for_eof() {
        let (mut sender, receiver) = UnixStream::pair().unwrap();
        sender.write_all(b"diagnostic tail").unwrap();
        let cancel = Cancellation::default();
        let mut reader = CancellablePipe::new(receiver, cancel.clone()).unwrap();
        cancel.cancel();
        let mut tail = Vec::new();
        let error = reader.read_to_end(&mut tail).unwrap_err();
        assert_eq!(tail, b"diagnostic tail");
        assert_eq!(error.kind(), io::ErrorKind::ConnectionAborted);
    }
}
