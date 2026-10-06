//! Runs the real binary on a pseudo-terminal and ends it the ways a terminal can: the terminal
//! going away (SIGHUP, the master closed) and SIGTERM. The exit must be the signal's conventional
//! `128 + n` code, never an abort (SIGABRT) from a panic while restoring a dead tty, and the
//! terminal must be told to leave bracketed paste and the alternate screen.

#![cfg(unix)]

use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

struct Pty {
    master: Option<OwnedFd>,
    child: Child,
    home: std::path::PathBuf,
}

impl Drop for Pty {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.home);
    }
}

fn spawn_konoma(tag: &str) -> Pty {
    let mut master = 0;
    let mut slave = 0;
    let ws = libc::winsize {
        ws_row: 30,
        ws_col: 100,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    // SAFETY: out-pointers are valid; the termios pointer is null and the winsize is a valid
    // initialized struct (`openpty` takes it by `*const` on both platforms' libc bindings).
    let rc = unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &ws as *const libc::winsize as *mut libc::winsize,
        )
    };
    assert_eq!(rc, 0, "openpty failed");
    // SAFETY: both fds were just returned by openpty and are owned by us.
    let (master, slave) = unsafe { (OwnedFd::from_raw_fd(master), OwnedFd::from_raw_fd(slave)) };

    let home = std::env::temp_dir().join(format!("konoma-test-{tag}-{}", std::process::id()));
    let work = home.join("work");
    std::fs::create_dir_all(&work).unwrap();
    std::fs::write(work.join("a.txt"), "hello\n").unwrap();

    // Close-on-exec on both ends: a sibling test's child must not inherit this master (it would
    // keep the terminal open and the hangup would never be delivered). The child gets its own
    // copies as stdio below.
    for fd in [master.as_raw_fd(), slave.as_raw_fd()] {
        // SAFETY: plain fcntl on an fd we own.
        unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) };
    }
    let slave_fd = slave.as_raw_fd();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_konoma"));
    cmd.arg(&work)
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", home.join("cfg"))
        .env("XDG_CACHE_HOME", home.join("cache"))
        .env("TMPDIR", &home)
        .env("TERM", "xterm-256color")
        .stdin(Stdio::from(slave.try_clone().unwrap()))
        .stdout(Stdio::from(slave.try_clone().unwrap()))
        .stderr(Stdio::from(slave.try_clone().unwrap()));
    // SAFETY: only async-signal-safe calls (`setsid`, `ioctl`) between fork and exec.
    unsafe {
        cmd.pre_exec(move || {
            libc::setsid();
            libc::ioctl(slave_fd, libc::TIOCSCTTY as _, 0);
            Ok(())
        });
    }
    let child = cmd.spawn().expect("spawn konoma");
    drop(slave);
    Pty {
        master: Some(master),
        child,
        home,
    }
}

/// Reads until the child has switched to the alternate screen (the signal handlers are installed
/// before that), up to 30 s, so the test never depends on how fast startup is.
fn wait_for_alt_screen(fd: &OwnedFd) -> Vec<u8> {
    let mut out = Vec::new();
    let until = Instant::now() + Duration::from_secs(30);
    while Instant::now() < until {
        out.extend(read_for(fd, Duration::from_millis(100)));
        if out.windows(8).any(|w| w == b"\x1b[?1049h") {
            // A short grace so the app is past init and drawing, not for correctness.
            out.extend(read_for(fd, Duration::from_millis(300)));
            return out;
        }
    }
    panic!("konoma never entered the alternate screen");
}

/// Reads whatever the child has written so far, for up to `wait`, stopping at EOF/error.
fn read_for(fd: &OwnedFd, wait: Duration) -> Vec<u8> {
    let mut out = Vec::new();
    let until = Instant::now() + wait;
    let mut buf = [0u8; 65536];
    while Instant::now() < until {
        let mut p = libc::pollfd {
            fd: fd.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: `p` is a valid pollfd and the count is 1.
        let n = unsafe { libc::poll(&mut p, 1, 50) };
        if n > 0 {
            // SAFETY: `buf` is valid for `buf.len()` bytes.
            let r = unsafe { libc::read(fd.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) };
            if r <= 0 {
                break;
            }
            out.extend_from_slice(&buf[..r as usize]);
        }
    }
    out
}

fn wait_exit(child: &mut Child) -> std::process::ExitStatus {
    let until = Instant::now() + Duration::from_secs(15);
    loop {
        if let Some(st) = child.try_wait().unwrap() {
            return st;
        }
        if Instant::now() > until {
            let _ = child.kill();
            panic!("konoma did not exit");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn hangup_exits_129_not_abort() {
    let mut p = spawn_konoma("hup");
    // Let it start and draw its first frames, then hang the terminal up.
    let _ = wait_for_alt_screen(p.master.as_ref().unwrap());
    drop(p.master.take());
    let st = wait_exit(&mut p.child);
    assert_eq!(
        st.signal(),
        None,
        "konoma was killed by signal {:?} (SIGABRT=6) instead of exiting",
        st.signal()
    );
    assert_eq!(st.code(), Some(128 + libc::SIGHUP));
}

#[test]
fn sigterm_exits_143_and_restores_the_terminal() {
    let mut p = spawn_konoma("term");
    let _ = wait_for_alt_screen(p.master.as_ref().unwrap());
    // SAFETY: plain kill(2) on our own child.
    unsafe { libc::kill(p.child.id() as i32, libc::SIGTERM) };
    let out = read_for(p.master.as_ref().unwrap(), Duration::from_millis(1500));
    let st = wait_exit(&mut p.child);
    assert_eq!(st.signal(), None);
    assert_eq!(st.code(), Some(128 + libc::SIGTERM));
    let text = String::from_utf8_lossy(&out);
    assert!(
        text.contains("\x1b[?2004l"),
        "bracketed paste was not turned off: {text:?}"
    );
    assert!(
        text.contains("\x1b[?1049l"),
        "the alternate screen was not left: {text:?}"
    );
    assert!(
        text.contains("\x1b[?25h"),
        "the cursor was not shown again: {text:?}"
    );
}
