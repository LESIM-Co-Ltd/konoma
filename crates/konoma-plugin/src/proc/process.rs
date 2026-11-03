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
    poll_exit_pid(child.id())
}

fn poll_exit_pid(pid: u32) -> io::Result<Option<ExitStatus>> {
    use std::os::unix::process::ExitStatusExt;
    loop {
        // SAFETY: waitid writes one initialized siginfo; WNOWAIT never reaps this owned child.
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        let result = unsafe {
            libc::waitid(
                libc::P_PID,
                pid as libc::id_t,
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

fn stop_group_with(child: &mut Child, grace: Duration, signal: impl FnMut(u32, libc::c_int)) {
    stop_group_observing(child, grace, signal, group_has_live_members, || {});
}

#[cfg(target_os = "macos")]
fn group_has_live_members(group: u32) -> io::Result<bool> {
    // libc's proc_listpgrppids wraps PROC_PGRP_ONLY and returns a count, not bytes.
    // XNU enumerates both allproc and zombproc; TBSDINFO's arg=1 includes zombies too.
    // https://github.com/apple-oss-distributions/xnu/blob/main/bsd/kern/proc_info.c
    // https://github.com/apple-oss-distributions/xnu/blob/main/libsyscall/wrappers/libproc/libproc.c
    for attempt in 0..8 {
        let mut pids: Vec<libc::pid_t> = vec![0; 64 << attempt];
        let bytes = (pids.len() * std::mem::size_of::<libc::pid_t>()) as i32;
        // SAFETY: the buffer has space for `bytes` bytes; our unreaped child reserves the group.
        let count = unsafe {
            libc::proc_listpgrppids(group as libc::pid_t, pids.as_mut_ptr().cast(), bytes)
        };
        if count <= 0 {
            return Err(io::Error::last_os_error());
        }
        if count as usize >= pids.len() {
            continue;
        }
        pids.truncate(count as usize);
        if !pids.contains(&(group as libc::pid_t)) {
            return Err(io::Error::other(
                "group enumeration omitted the reserved leader",
            ));
        }
        for pid in pids {
            // SAFETY: proc_pidinfo initializes this buffer on a full-sized successful result.
            let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
            let size = std::mem::size_of_val(&info) as i32;
            let read = unsafe {
                libc::proc_pidinfo(
                    pid,
                    libc::PROC_PIDTBSDINFO,
                    1,
                    (&mut info as *mut libc::proc_bsdinfo).cast(),
                    size,
                )
            };
            if read <= 0 {
                let error = io::Error::last_os_error();
                if error.raw_os_error() == Some(libc::ESRCH) {
                    continue;
                }
                return Err(error);
            }
            if read != size {
                return Err(io::Error::other("short process-status result"));
            }
            if info.pbi_pgid == group && info.pbi_status != libc::SZOMB {
                return Ok(true);
            }
        }
        return Ok(false);
    }
    // A full buffer may have omitted a live member. Never infer quiescence from a partial list.
    Err(io::Error::other(
        "process group kept outgrowing its enumeration buffer",
    ))
}

#[cfg(target_os = "linux")]
fn group_has_live_members(group: u32) -> io::Result<bool> {
    linux_group_has_live_members(std::path::Path::new("/proc"), group)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn group_has_live_members(_: u32) -> io::Result<bool> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "process-group enumeration unavailable",
    ))
}

#[cfg(any(target_os = "linux", test))]
fn linux_group_has_live_members(root: &std::path::Path, group: u32) -> io::Result<bool> {
    // /proc/<pid>/stat includes zombies as state Z (and dead tasks as X/x). Its pgrp is field 5.
    // https://man7.org/linux/man-pages/man5/proc_pid_stat.5.html
    let mut saw_leader = false;
    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() else {
            continue;
        };
        let stat = match std::fs::read(entry.path().join("stat")) {
            Ok(stat) => stat,
            Err(e)
                if e.kind() == io::ErrorKind::NotFound || e.raw_os_error() == Some(libc::ESRCH) =>
            {
                continue
            }
            Err(e) => return Err(e),
        };
        let (state, pgrp) = linux_stat_group(&stat)?;
        saw_leader |= pid == group && pgrp == group;
        if pgrp == group && !matches!(state, b'Z' | b'X' | b'x') {
            return Ok(true);
        }
    }
    if !saw_leader {
        return Err(io::Error::other("procfs omitted the reserved leader"));
    }
    Ok(false)
}

#[cfg(any(target_os = "linux", test))]
fn linux_stat_group(stat: &[u8]) -> io::Result<(u8, u32)> {
    let invalid = || io::Error::new(io::ErrorKind::InvalidData, "invalid process stat");
    // comm can contain spaces, parentheses and non-UTF-8 bytes. Only the suffix is numeric.
    let end = stat.iter().rposition(|b| *b == b')').ok_or_else(invalid)?;
    let mut fields = stat[end + 1..]
        .split(u8::is_ascii_whitespace)
        .filter(|f| !f.is_empty());
    let state = fields.next().filter(|s| s.len() == 1).ok_or_else(invalid)?[0];
    fields.next().ok_or_else(invalid)?; // ppid, field 4
    let group = std::str::from_utf8(fields.next().ok_or_else(invalid)?)
        .map_err(|_| invalid())?
        .parse()
        .map_err(|_| invalid())?;
    Ok((state, group))
}

fn stop_group_observing(
    child: &mut Child,
    grace: Duration,
    mut signal: impl FnMut(u32, libc::c_int),
    mut live_members: impl FnMut(u32) -> io::Result<bool>,
    mut waiting: impl FnMut(),
) {
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
            // Failed or incomplete enumeration is not evidence that the group has finished.
            if !live_members(pid).unwrap_or(true) {
                break;
            }
            waiting();
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

    struct TestChild(Child);

    impl Drop for TestChild {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    fn live_leader() -> TestChild {
        use std::os::unix::process::CommandExt;
        use std::process::Stdio;
        TestChild(
            Command::new("/bin/cat")
                .process_group(0)
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        )
    }

    #[test]
    fn a_live_leader_that_exits_on_term_finishes_without_the_full_grace() {
        let mut child = live_leader();
        assert!(poll_exit(&child.0).unwrap().is_none());
        let (tx, rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            stop_group(&mut child.0, Duration::from_secs(30));
            assert_eq!(
                poll_exit(&child.0).unwrap_err().raw_os_error(),
                Some(libc::ECHILD)
            );
            tx.send(()).unwrap();
        });
        let completed = rx.recv_timeout(Duration::from_secs(10));
        worker.join().unwrap();
        completed.expect("TERM ended the leader; its zombie must not consume the grace");
    }

    #[test]
    fn a_live_member_holds_the_grace_until_released_then_finishes_promptly() {
        use std::io::{BufRead, BufReader};
        use std::os::fd::OwnedFd;
        use std::os::unix::process::CommandExt;
        use std::process::Stdio;
        let mut leader = live_leader();
        let pid = leader.0.id();
        let (output, child_output) = UnixStream::pair().unwrap();
        output
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let script =
            "trap 'echo term; IFS= read -r release; exit 0' TERM; echo ready; IFS= read -r initial";
        let mut member = TestChild(
            Command::new("/bin/sh")
                .args(["-c", script])
                .process_group(pid as i32)
                .stdin(Stdio::piped())
                .stdout(Stdio::from(OwnedFd::from(child_output)))
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        );
        let mut output = BufReader::new(output);
        let mut line = String::new();
        output.read_line(&mut line).unwrap();
        assert_eq!(line, "ready\n");
        let (waiting_tx, waiting_rx) = std::sync::mpsc::channel();
        let (permit_tx, permit_rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let mut waits = 0;
            stop_group_observing(
                &mut leader.0,
                Duration::from_secs(30),
                signal_group,
                group_has_live_members,
                || {
                    if waits < 2 {
                        waits += 1;
                        waiting_tx.send(()).unwrap();
                        permit_rx.recv_timeout(Duration::from_secs(20)).unwrap();
                    }
                },
            );
            assert_eq!(
                poll_exit(&leader.0).unwrap_err().raw_os_error(),
                Some(libc::ECHILD)
            );
            done_tx.send(()).unwrap();
        });
        line.clear();
        let term_read = output.read_line(&mut line);
        let waiting = waiting_rx.recv_timeout(Duration::from_secs(10));
        // The observer is reached only after the loop has decided that it must keep waiting.
        // Keep it latched while the member is alive and the leader is observed as a zombie.
        if waiting.is_ok() {
            let until = Instant::now() + Duration::from_secs(10);
            while matches!(poll_exit_pid(pid), Ok(None)) && Instant::now() < until {
                std::thread::yield_now();
            }
        }
        let leader_exited = matches!(poll_exit_pid(pid), Ok(Some(_)));
        // Inspect again after observing the leader's exit, while the member is still latched.
        // Otherwise the first inspection could have seen only the still-live leader.
        let _ = permit_tx.send(());
        let waiting_with_zombie_leader = waiting_rx.recv_timeout(Duration::from_secs(10));
        let member_alive = member.0.try_wait().unwrap().is_none();
        let before_release = done_rx.try_recv();
        let released = member.0.stdin.as_mut().unwrap().write_all(b"release\n");
        let until = Instant::now() + Duration::from_secs(10);
        while matches!(poll_exit(&member.0), Ok(None)) && Instant::now() < until {
            std::thread::yield_now();
        }
        // Retain this member as a zombie too: it must not consume the grace after release.
        let member_exited = matches!(poll_exit(&member.0), Ok(Some(_)));
        let _ = permit_tx.send(());
        let completed = done_rx.recv_timeout(Duration::from_secs(10));
        worker.join().unwrap();
        let member_status = member.0.wait().unwrap();
        term_read.unwrap();
        assert_eq!(line, "term\n");
        waiting.expect("the live member must keep the grace loop waiting");
        waiting_with_zombie_leader
            .expect("the member must also keep waiting after its leader exits");
        assert!(leader_exited && member_alive);
        assert!(matches!(
            before_release,
            Err(std::sync::mpsc::TryRecvError::Empty)
        ));
        released.unwrap();
        assert!(member_exited);
        assert!(
            member_status.success(),
            "the member must finish after its release, not be killed"
        );
        completed.expect("the grace must end promptly after the last live member exits");
    }

    #[test]
    fn an_enumeration_failure_keeps_the_grace_waiting() {
        let mut child = live_leader();
        let (waiting_tx, waiting_rx) = std::sync::mpsc::channel();
        let (permit_tx, permit_rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let mut fail_next = true;
            let mut first_wait = true;
            stop_group_observing(
                &mut child.0,
                Duration::from_secs(30),
                signal_group,
                |pid| {
                    if std::mem::take(&mut fail_next) {
                        Err(io::Error::from(io::ErrorKind::PermissionDenied))
                    } else {
                        group_has_live_members(pid)
                    }
                },
                || {
                    if first_wait {
                        first_wait = false;
                        waiting_tx.send(()).unwrap();
                        permit_rx.recv_timeout(Duration::from_secs(20)).unwrap();
                    }
                },
            );
            done_tx.send(()).unwrap();
        });
        let waiting = waiting_rx.recv_timeout(Duration::from_secs(10));
        let before_retry = done_rx.try_recv();
        let _ = permit_tx.send(());
        let completed = done_rx.recv_timeout(Duration::from_secs(10));
        worker.join().unwrap();
        waiting.expect("an enumeration error must reach the waiting branch");
        assert!(matches!(
            before_retry,
            Err(std::sync::mpsc::TryRecvError::Empty)
        ));
        completed.expect("a successful retry may observe that the group has finished");
    }

    fn proc_stat(root: &std::path::Path, pid: u32, stat: &[u8]) {
        let dir = root.join(pid.to_string());
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("stat"), stat).unwrap();
    }

    #[test]
    fn linux_group_members_ignore_zombies_and_other_groups() {
        let root = konoma_test_support::unique_tmp("proc-group-status");
        proc_stat(&root, 700, b"700 (leader) Z 1 700 0 0");
        proc_stat(&root, 800, b"800 (other) R 1 800 0 0");
        assert!(!linux_group_has_live_members(&root, 700).unwrap());
        proc_stat(&root, 701, b"701 (name ) with \xff) S 1 700 0 0");
        assert!(linux_group_has_live_members(&root, 700).unwrap());
        proc_stat(&root, 701, b"701 (name ) with \xff) Z 1 700 0 0");
        assert!(!linux_group_has_live_members(&root, 700).unwrap());
        proc_stat(&root, 701, b"701 (dead) X 1 700 0 0");
        assert!(!linux_group_has_live_members(&root, 700).unwrap());
    }

    #[test]
    fn linux_group_enumeration_errors_are_not_empty_groups() {
        let root = konoma_test_support::unique_tmp("proc-group-errors");
        assert!(linux_group_has_live_members(&root, 700).is_err());
        proc_stat(&root, 700, b"700 (leader) Z 1 700 0 0");
        proc_stat(&root, 701, b"not a valid stat record");
        assert_eq!(
            linux_group_has_live_members(&root, 700).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        std::fs::remove_file(root.join("701/stat")).unwrap();
        std::fs::create_dir(root.join("701/stat")).unwrap();
        assert!(linux_group_has_live_members(&root, 700).is_err());
    }

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
