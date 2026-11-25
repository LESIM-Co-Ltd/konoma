// Native process fixtures shared by the host and protocol checker tests.
use std::io::Read;
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::process::Command;
use std::time::Duration;

#[derive(Debug)]
pub struct PendingHelper {
    ready: UnixStream,
    sender: UnixStream,
}

pub struct Helper {
    pub pid: libc::pid_t,
    armed: std::cell::Cell<bool>,
    lifetime: UnixStream,
}

impl PendingHelper {
    pub fn attach(command: &mut Command, detached: bool, redirect_output: bool) -> Self {
        let (ready, sender) = UnixStream::pair().unwrap();
        ready
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let fd = sender.as_raw_fd();
        // SAFETY: only async-signal-safe calls run between fork and exec. Exec closes Rust's
        // private CLOEXEC spawn descriptors, so the fixture cannot block spawn's handshake.
        unsafe {
            command.pre_exec(move || {
                let helper = libc::fork();
                if helper == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                if helper == 0 {
                    if detached && libc::setsid() == -1 {
                        libc::_exit(1);
                    }
                    if redirect_output {
                        let null = libc::open(c"/dev/null".as_ptr(), libc::O_WRONLY);
                        if null == -1 || libc::dup2(null, 1) == -1 || libc::dup2(null, 2) == -1 {
                            libc::_exit(1);
                        }
                        libc::close(null);
                    }
                    let pid = libc::getpid();
                    libc::write(
                        fd,
                        (&pid as *const libc::pid_t).cast(),
                        std::mem::size_of_val(&pid),
                    );
                    // Keep only this extra lifetime descriptor across exec, alongside stdio.
                    libc::fcntl(fd, libc::F_SETFD, 0);
                    libc::execl(
                        c"/bin/sleep".as_ptr(),
                        c"sleep".as_ptr(),
                        c"300".as_ptr(),
                        std::ptr::null::<libc::c_char>(),
                    );
                    libc::_exit(1);
                }
                if libc::write(
                    fd,
                    (&helper as *const libc::pid_t).cast(),
                    std::mem::size_of_val(&helper),
                ) == -1
                {
                    libc::kill(helper, libc::SIGKILL);
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        Self { ready, sender }
    }

    pub fn ready(self) -> Helper {
        let Self { mut ready, sender } = self;
        let mut bytes = [0; std::mem::size_of::<libc::pid_t>()];
        ready.read_exact(&mut bytes).unwrap();
        let mut helper = Helper {
            pid: libc::pid_t::from_ne_bytes(bytes),
            armed: std::cell::Cell::new(true),
            lifetime: ready,
        };
        helper.lifetime.read_exact(&mut bytes).unwrap();
        assert_eq!(libc::pid_t::from_ne_bytes(bytes), helper.pid);
        // The launcher may still be between configure and spawn. Keep the source descriptor
        // alive until the child has published both records, rather than closing it prematurely.
        drop(sender);
        helper
    }
}

impl Helper {
    pub fn gone(&mut self) -> bool {
        // The readiness socket already has a watchdog; keep it when the peer has closed.
        let gone = matches!(self.lifetime.read(&mut [0]), Ok(0));
        if gone {
            self.armed.set(false);
        }
        gone
    }
    pub fn stop(&self) {
        self.stop_with(|pid| unsafe {
            libc::kill(pid, libc::SIGKILL);
        });
    }
    fn stop_with(&self, mut signal: impl FnMut(libc::pid_t)) {
        let mut event = libc::pollfd {
            fd: self.lifetime.as_raw_fd(),
            events: libc::POLLIN | libc::POLLHUP,
            revents: 0,
        };
        // Do not kill an old recorded pid even if the caller has not yet called gone().
        if unsafe { libc::poll(&mut event, 1, 0) } > 0 {
            let mut pipe = &self.lifetime;
            if matches!(pipe.read(&mut [0]), Ok(0)) {
                self.armed.set(false);
            }
        }
        // SAFETY: this exact pid was recorded by our fixture, never found by name.
        if !self.armed.get() {
            return;
        }
        signal(self.pid);
    }
}

#[test]
fn a_gone_helper_never_signals_its_recorded_pid_again() {
    use std::process::Stdio;
    let mut command = Command::new("/bin/cat");
    command
        .process_group(0)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let pending = PendingHelper::attach(&mut command, false, true);
    struct Leader(std::process::Child);
    impl Drop for Leader {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let _leader = Leader(command.spawn().unwrap());
    let mut helper = pending.ready();
    helper.stop();
    assert!(helper.gone());
    let mut signals = Vec::new();
    helper.stop_with(|pid| signals.push(pid));
    assert!(
        signals.is_empty(),
        "EOF must disarm pid cleanup: {signals:?}"
    );
}

impl Drop for Helper {
    fn drop(&mut self) {
        self.stop();
    }
}
