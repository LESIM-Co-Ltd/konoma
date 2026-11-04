//! Re-exec supervision for plugin CLI jobs, including when the application cannot run cleanup.
//!
//! The private socket is a lifetime pipe and carries the final raw wait status (or a watcher
//! error). Child stdout/stderr go directly to the caller's terminal or capture pipes.

use std::ffi::OsStr;
use std::io::{self, Read, Write};
use std::net::Shutdown;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::{
    net::UnixStream,
    process::{CommandExt, ExitStatusExt},
};
use std::process::{Command, ExitStatus, Output};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use konoma_plugin::proc::process::{self, ChildGuard};

use super::build::STOP_GRACE;

pub(crate) const CHILD_FLAG: &str = "--internal-plugin-watch";
// Allow the watcher its entire TERM grace and bounded reap before a last-resort stop of it.
const WATCHER_STOP: Duration = Duration::from_secs(5);
const MAX_REPORT: u64 = 64 * 1024;

pub(super) struct Launch {
    command: Command,
    control: UnixStream,
    inherited: UnixStream,
}

impl Launch {
    pub fn new(program: impl AsRef<OsStr>) -> Result<Self> {
        let executable = std::env::current_exe()?;
        // Unit tests are linked into a harness; integration tests and production re-exec the
        // running application itself. Cargo builds this application beside the harness's deps.
        #[cfg(test)]
        let executable = executable
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("konoma");
        Self::with_executable(&executable, program.as_ref())
    }

    fn with_executable(executable: &std::path::Path, program: &OsStr) -> Result<Self> {
        let (control, inherited) = UnixStream::pair()?;
        // Configure this while the peer is connected; macOS rejects SO_RCVTIMEO after its exit.
        control.set_read_timeout(Some(Duration::from_millis(500)))?;
        let fd = inherited.as_raw_fd();
        let mut command = Command::new(executable);
        command
            .arg(CHILD_FLAG)
            .arg(fd.to_string())
            .arg(program)
            .process_group(0);
        // SAFETY: fcntl is async-signal-safe. The stream remains owned until spawn completes.
        unsafe {
            command.pre_exec(move || {
                if libc::fcntl(fd, libc::F_SETFD, 0) == -1 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        Ok(Self {
            command,
            control,
            inherited,
        })
    }

    pub fn command(&mut self) -> &mut Command {
        &mut self.command
    }

    pub fn spawn(mut self) -> Result<Watched> {
        let child = self.command.spawn().with_context(|| {
            format!(
                "cannot start the plugin job watcher ({})",
                self.command.get_program().to_string_lossy()
            )
        })?;
        drop(self.inherited);
        Ok(Watched {
            child: ChildGuard::new(child),
            control: self.control,
            finished: false,
        })
    }
}

pub(super) struct Watched {
    child: ChildGuard,
    control: UnixStream,
    finished: bool,
}

impl Watched {
    fn request_stop(&self) {
        let _ = self.control.shutdown(Shutdown::Write);
    }

    fn report(&mut self) -> Result<ExitStatus> {
        let mut report = Vec::new();
        (&self.control)
            .take(MAX_REPORT + 1)
            .read_to_end(&mut report)
            .context("cannot read the plugin job watcher's result")?;
        match report.first() {
            Some(0) if report.len() == 5 => Ok(ExitStatus::from_raw(i32::from_le_bytes(
                report[1..].try_into().unwrap(),
            ))),
            Some(1) if report.len() <= MAX_REPORT as usize => bail!(
                "plugin job watcher: {}",
                String::from_utf8_lossy(&report[1..])
            ),
            _ => bail!("plugin job watcher ended without a valid child result"),
        }
    }

    pub fn wait(&mut self, interrupted: &AtomicBool) -> Result<ExitStatus> {
        loop {
            if interrupted.load(Ordering::SeqCst) {
                return Err(super::build::Interrupted.into());
            }
            if let Some(status) = process::poll_exit(&self.child)? {
                process::stop_group(&mut self.child, Duration::ZERO)?;
                self.finished = true;
                if !status.success() {
                    bail!("plugin job watcher failed ({status})");
                }
                return self.report();
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    pub fn output(
        &mut self,
        interrupted: &AtomicBool,
        timeout: Option<Duration>,
    ) -> Result<Output> {
        let start = Instant::now();
        let result = process::collect_output(
            &mut self.child,
            || {
                let interrupted = interrupted.load(Ordering::SeqCst);
                if interrupted || timeout.is_some_and(|limit| start.elapsed() >= limit) {
                    let _ = self.control.shutdown(Shutdown::Write);
                }
                interrupted
            },
            timeout,
            usize::MAX,
            WATCHER_STOP,
        );
        match result {
            Ok(capture) => {
                self.finished = true;
                let mut output = capture.output;
                if !output.status.success() {
                    bail!("plugin job watcher failed ({})", output.status);
                }
                output.status = self.report()?;
                Ok(output)
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {
                Err(super::build::Interrupted.into())
            }
            Err(error) => Err(error).context("cannot collect the plugin job watcher's output"),
        }
    }
}

impl Drop for Watched {
    fn drop(&mut self) {
        if !self.finished {
            self.request_stop();
            // Bounded even if the watcher itself fails; ChildGuard owns the final fallback.
            let _ = process::wait(&mut self.child, WATCHER_STOP);
        }
    }
}

/// Dispatch before plugins/config/terminal setup. No diagnostics use the child's output pipes.
pub(crate) fn child_main() -> i32 {
    let mut args = std::env::args_os().skip(2);
    let Some(fd) = args
        .next()
        .and_then(|a| a.to_str()?.parse::<i32>().ok())
        .filter(|fd| *fd > 2)
    else {
        return 1;
    };
    // SAFETY: the launcher passed this owned UnixStream descriptor across exec.
    let mut control = unsafe { UnixStream::from_raw_fd(fd) };
    let result = run(&mut control, args);
    let mut report = Vec::new();
    match result {
        Ok(status) => {
            report.push(0);
            report.extend_from_slice(&status.into_raw().to_le_bytes());
        }
        Err(error) => {
            report.push(1);
            report.extend_from_slice(format!("{error:#}").as_bytes());
            report.truncate(MAX_REPORT as usize);
        }
    }
    // The parent may have died. A finite socket timeout also bounds a live but unresponsive peer.
    let _ = control.set_write_timeout(Some(Duration::from_millis(500)));
    let _ = control.write_all(&report);
    0
}

fn run(
    control: &mut UnixStream,
    mut args: impl Iterator<Item = std::ffi::OsString>,
) -> Result<ExitStatus> {
    let program = args.next().context("missing watched program")?;
    // Do not let cargo inherit the lifetime socket: only konoma's end may keep the watcher alive.
    if unsafe { libc::fcntl(control.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) } == -1 {
        return Err(io::Error::last_os_error().into());
    }
    control.set_nonblocking(true)?;
    let stopped = Arc::new(AtomicBool::new(false));
    signal_hook::flag::register(libc::SIGTERM, stopped.clone())?;
    // Terminal INT/HUP belong to konoma. TERM is a fallback stop request if its pipe handling fails.
    unsafe {
        libc::signal(libc::SIGINT, libc::SIG_IGN);
        libc::signal(libc::SIGHUP, libc::SIG_IGN);
    }
    let mut command = Command::new(&program);
    command.args(args).process_group(0);
    // SAFETY: only async-signal-safe disposition/mask operations run after fork.
    unsafe {
        command.pre_exec(reset_child_signals);
    }
    let child = command
        .spawn()
        .with_context(|| format!("cannot start watched program {}", program.to_string_lossy()))?;
    let mut child = ChildGuard::new(child);
    let observed = (|| -> Result<()> {
        loop {
            if process::poll_exit(&child)?.is_some() {
                return Ok(());
            }
            if stopped.load(Ordering::SeqCst) {
                return Ok(());
            }
            match control.read(&mut [0]) {
                Ok(0) => return Ok(()),
                Ok(_) => bail!("unexpected watcher control data"),
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                    ) => {}
                Err(error) => return Err(error.into()),
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    })();
    let status = process::stop_group(&mut child, STOP_GRACE)?;
    observed?;
    Ok(status)
}

fn reset_child_signals() -> io::Result<()> {
    // Exec resets caught handlers, but retains ignored dispositions and the signal mask.
    unsafe {
        let mut action: libc::sigaction = std::mem::zeroed();
        action.sa_sigaction = libc::SIG_DFL;
        libc::sigemptyset(&mut action.sa_mask);
        let mut unblocked = std::mem::zeroed();
        libc::sigemptyset(&mut unblocked);
        for signal in [libc::SIGINT, libc::SIGHUP, libc::SIGTERM] {
            if libc::sigaction(signal, &action, std::ptr::null_mut()) == -1 {
                return Err(io::Error::last_os_error());
            }
            libc::sigaddset(&mut unblocked, signal);
        }
        if libc::sigprocmask(libc::SIG_UNBLOCK, &unblocked, std::ptr::null_mut()) == -1 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{unique_tmp, ProgramFixture};
    use std::process::Stdio;

    fn child_receives_inherited_blocked_signal(name: &str, signal: libc::c_int) {
        let mut launch = Launch::new("/bin/sh").unwrap();
        launch
            .command()
            .args(["-c", &format!("kill -{name} $$; printf survived")])
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // SAFETY: configure only the forked watcher's dispositions and mask with
        // async-signal-safe calls; the test process and its other threads stay unchanged.
        unsafe {
            launch.command().pre_exec(|| {
                let mut ignored: libc::sigaction = std::mem::zeroed();
                ignored.sa_sigaction = libc::SIG_IGN;
                libc::sigemptyset(&mut ignored.sa_mask);
                if libc::sigaction(libc::SIGINT, &ignored, std::ptr::null_mut()) == -1 {
                    return Err(io::Error::last_os_error());
                }
                let mut blocked = std::mem::zeroed();
                libc::sigemptyset(&mut blocked);
                for signal in [libc::SIGINT, libc::SIGHUP, libc::SIGTERM] {
                    libc::sigaddset(&mut blocked, signal);
                }
                if libc::sigprocmask(libc::SIG_BLOCK, &blocked, std::ptr::null_mut()) == -1 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut job = launch.spawn().unwrap();
        let output = job
            .output(&AtomicBool::new(false), Some(Duration::from_secs(10)))
            .unwrap();
        assert_eq!(output.status.signal(), Some(signal), "{output:?}");
        assert!(output.stdout.is_empty(), "{output:?}");
        assert!(
            job.child.try_wait().unwrap().is_some(),
            "watcher must be reaped"
        );
    }

    #[test]
    fn watcher_unblocks_inherited_int_mask() {
        child_receives_inherited_blocked_signal("INT", libc::SIGINT);
    }

    #[test]
    fn watcher_unblocks_inherited_term_mask() {
        child_receives_inherited_blocked_signal("TERM", libc::SIGTERM);
    }

    #[test]
    fn watcher_unblocks_inherited_hangup_mask() {
        child_receives_inherited_blocked_signal("HUP", libc::SIGHUP);
    }

    #[test]
    fn watcher_forwards_exit_status_and_output() {
        for (script, code, signal) in [
            ("printf out; printf err >&2; exit 0", Some(0), None),
            ("printf out; printf err >&2; exit 37", Some(37), None),
            (
                "printf out; printf err >&2; kill -TERM $$",
                None,
                Some(libc::SIGTERM),
            ),
            (
                "printf out; printf err >&2; kill -INT $$",
                None,
                Some(libc::SIGINT),
            ),
            (
                "printf out; printf err >&2; kill -HUP $$",
                None,
                Some(libc::SIGHUP),
            ),
        ] {
            let mut launch = Launch::new("/bin/sh").unwrap();
            launch
                .command()
                .args(["-c", script])
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            let mut job = launch.spawn().unwrap();
            let output = job
                .output(&AtomicBool::new(false), Some(Duration::from_secs(10)))
                .unwrap();
            assert_eq!(output.stdout, b"out");
            assert_eq!(output.stderr, b"err");
            assert_eq!(output.status.code(), code);
            assert_eq!(output.status.signal(), signal);
            assert!(
                job.child.try_wait().unwrap().is_some(),
                "watcher must be reaped"
            );
        }
    }

    #[test]
    fn watcher_start_failures_name_the_cause() {
        let dir = unique_tmp("watcher-start-failure");
        std::fs::create_dir_all(&dir).unwrap();
        let missing = dir.join("missing");
        let launch = Launch::with_executable(&missing, OsStr::new("/bin/true")).unwrap();
        let error = launch.spawn().err().expect("missing watcher must fail");
        assert!(format!("{error:#}").contains("cannot start the plugin job watcher"));
        assert!(error.chain().any(|cause| cause
            .downcast_ref::<io::Error>()
            .is_some_and(|cause| cause.kind() == io::ErrorKind::NotFound)));
        let mut launch = Launch::new(&missing).unwrap();
        launch
            .command()
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut job = launch.spawn().unwrap();
        let error = job
            .output(&AtomicBool::new(false), Some(Duration::from_secs(10)))
            .unwrap_err();
        let text = format!("{error:#}");
        assert!(text.contains("cannot start watched program"), "{text}");
        assert!(text.contains(missing.to_str().unwrap()), "{text}");
        assert!(text.contains("os error"), "{text}");
        assert!(job.child.try_wait().unwrap().is_some());
    }

    #[test]
    fn watcher_failures_are_not_child_statuses() {
        let dir = unique_tmp("watcher-failed-supervisor");
        std::fs::create_dir_all(&dir).unwrap();
        let executable = dir.join("watcher");
        for (status, reason) in [(42, "watcher failed"), (0, "without a valid child result")] {
            crate::test_support::write_executable(
                &executable,
                format!("#!/bin/sh\nexit {status}\n"),
            );
            let mut launch = Launch::with_executable(&executable, OsStr::new("/bin/true")).unwrap();
            launch
                .command()
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            let mut job = launch.spawn().unwrap();
            let error = job.wait(&AtomicBool::new(false)).unwrap_err();
            assert!(format!("{error:#}").contains(reason), "{error:#}");
            assert!(job.child.try_wait().unwrap().is_some());
        }
    }

    fn stopped_capture(interrupt: bool) {
        let dir = unique_tmp("watcher-stopped-capture");
        std::fs::create_dir_all(&dir).unwrap();
        let helper_dir = dir.join("helper");
        std::fs::create_dir_all(&helper_dir).unwrap();
        let mut helper = ProgramFixture::new(
            &helper_dir,
            "trap '' TERM\nprintf '%s\\n' \"$$\" >&3\nexec sleep 300",
        );
        let mut program = ProgramFixture::new(
            &dir,
            &format!(
                "'{}' &\nprintf '%s\\n' \"$$\" >&3\nwait",
                helper.program.display()
            ),
        );
        let mut launch = Launch::new(&program.program).unwrap();
        launch
            .command()
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut job = launch.spawn().unwrap();
        program.ready();
        helper.ready();
        // The deadline is created only after both processes have published readiness.
        let error = job
            .output(
                &AtomicBool::new(interrupt),
                Some(if interrupt {
                    Duration::from_secs(60)
                } else {
                    Duration::from_millis(100)
                }),
            )
            .unwrap_err();
        assert!(
            job.child.try_wait().unwrap().is_some(),
            "watcher must be reaped after cancellation"
        );
        drop(job);
        if interrupt {
            assert!(error.is::<super::super::build::Interrupted>(), "{error:#}");
        } else {
            assert!(format!("{error:#}").contains("timed out"), "{error:#}");
        }
        assert!(program.gone(), "direct child survived stop");
        assert!(helper.gone(), "TERM-ignoring helper survived stop");
    }

    #[test]
    fn watcher_natural_exits_stop_remaining_group() {
        for status in [0, 37] {
            let dir = unique_tmp("watcher-natural-exit");
            let helper_dir = dir.join("helper");
            std::fs::create_dir_all(&helper_dir).unwrap();
            let mut helper =
                ProgramFixture::new(&helper_dir, "printf '%s\\n' \"$$\" >&3\nexec sleep 300");
            let mut program = ProgramFixture::new(&dir, &format!(
                "'{}' </dev/null >/dev/null 2>&1 &\nprintf '%s\\n' \"$$\" >&3\nread line\nexit {status}", helper.program.display()));
            let mut launch = Launch::new(&program.program).unwrap();
            launch
                .command()
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            let mut job = launch.spawn().unwrap();
            program.ready();
            helper.ready();
            // The leader cannot exit until its background member is known to be alive.
            job.child
                .stdin
                .take()
                .unwrap()
                .write_all(b"release\n")
                .unwrap();
            let output = job
                .output(&AtomicBool::new(false), Some(Duration::from_secs(10)))
                .unwrap();
            assert_eq!(output.status.code(), Some(status));
            assert!(
                helper.gone(),
                "natural exit {status} left its group member alive"
            );
            assert!(program.gone());
            assert!(job.child.try_wait().unwrap().is_some());
        }
    }

    #[test]
    fn watcher_capture_timeout_stops_the_whole_job() {
        stopped_capture(false);
    }
    #[test]
    fn watcher_capture_interruption_stops_the_whole_job() {
        stopped_capture(true);
    }

    #[test]
    fn watcher_ignores_terminal_interrupt_and_hangup() {
        let dir = unique_tmp("watcher-terminal-signals");
        std::fs::create_dir_all(&dir).unwrap();
        let mut program = ProgramFixture::new(
            &dir,
            "printf '%s\\n' \"$$\" >&3\nread line\nprintf finished",
        );
        let mut launch = Launch::new(&program.program).unwrap();
        launch
            .command()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut job = launch.spawn().unwrap();
        program.ready();
        for signal in [libc::SIGINT, libc::SIGHUP] {
            // SAFETY: the watcher is our unreaped direct child.
            assert_eq!(unsafe { libc::kill(job.child.id() as i32, signal) }, 0);
        }
        job.child
            .stdin
            .take()
            .unwrap()
            .write_all(b"release\n")
            .unwrap();
        let output = job
            .output(&AtomicBool::new(false), Some(Duration::from_secs(10)))
            .unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout, b"finished");
        assert!(program.gone());
        assert!(job.child.try_wait().unwrap().is_some());
    }
}
