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
    spawn_konoma_with(tag, |work| {
        std::fs::write(work.join("a.txt"), "hello\n").unwrap();
    })
}

/// `spawn_konoma` with the files of the directory it opens chosen by `setup`.
fn spawn_konoma_with(tag: &str, setup: impl FnOnce(&std::path::Path)) -> Pty {
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
    setup(&work);

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

// ---- the SVG drawing processes do not outlive konoma ----------------------------------------

/// `(pid, args)` of every process whose parent is `ppid` (`ps` is the portable way to ask on both
/// macOS and Linux).
fn children_of(ppid: u32) -> Vec<(u32, String)> {
    let out = Command::new("ps")
        .args(["-A", "-o", "pid=,ppid=,args="])
        .output()
        .expect("ps");
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            let pid: u32 = it.next()?.parse().ok()?;
            let parent: u32 = it.next()?.parse().ok()?;
            let args = it.collect::<Vec<_>>().join(" ");
            (parent == ppid).then_some((pid, args))
        })
        .collect()
}

fn alive(pid: u32) -> bool {
    // SAFETY: signal 0 only checks that the process exists.
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

/// Opens an SVG in a konoma on a pseudo-terminal that answers the capability query (so pictures
/// are on), waits for the drawing process to exist and returns it. The drawing process is kept
/// for the next picture (it stops after 8 s idle), so it is still there when the signal comes.
fn konoma_with_a_drawing_process(tag: &str) -> (Pty, u32) {
    let p = spawn_konoma_with(tag, |work| {
        std::fs::write(
            work.join("a.svg"),
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="20"><rect width="40" height="20" fill="#c00"/></svg>"##,
        )
        .unwrap();
    });
    let _ = wait_for_alt_screen(p.master.as_ref().unwrap());
    let master = p.master.as_ref().unwrap();
    // The terminal's answer to the capability query: sixel, 7x14 px cells.
    let reply = b"\x1b[?64;4c\x1b[6;14;7t\x1b[6;6R\x1b[7;7R\x1b[6;6R\x1b[0n";
    // SAFETY: `reply` is valid for its length; `master` is an open descriptor.
    unsafe { libc::write(master.as_raw_fd(), reply.as_ptr().cast(), reply.len()) };
    let _ = read_for(master, Duration::from_millis(1500));
    // Enter on the first (only) entry: the SVG's preview.
    // SAFETY: as above.
    unsafe { libc::write(master.as_raw_fd(), b"\r".as_ptr().cast(), 1) };
    let konoma = p.child.id();
    let until = Instant::now() + Duration::from_secs(30);
    loop {
        let _ = read_for(p.master.as_ref().unwrap(), Duration::from_millis(100));
        if let Some((pid, _)) = children_of(konoma)
            .into_iter()
            .find(|(_, a)| a.contains("--internal-svg-render"))
        {
            return (p, pid);
        }
        assert!(
            Instant::now() < until,
            "konoma never started a drawing process"
        );
    }
}

fn assert_gone(pid: u32) {
    let until = Instant::now() + Duration::from_secs(5);
    while alive(pid) && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(!alive(pid), "the drawing process {pid} outlived konoma");
}

#[test]
fn sigterm_stops_the_svg_drawing_process() {
    let (mut p, child) = konoma_with_a_drawing_process("svgterm");
    // SAFETY: plain kill(2) on our own child.
    unsafe { libc::kill(p.child.id() as i32, libc::SIGTERM) };
    // Keep reading: a konoma that has drawn a picture has output queued for the terminal, and a
    // process cannot finish exiting while its tty still holds unread output.
    let _ = read_for(p.master.as_ref().unwrap(), Duration::from_millis(1500));
    let st = wait_exit(&mut p.child);
    assert_eq!(st.code(), Some(128 + libc::SIGTERM));
    assert_gone(child);
}

#[test]
fn hangup_stops_the_svg_drawing_process() {
    let (mut p, child) = konoma_with_a_drawing_process("svghup");
    drop(p.master.take());
    let st = wait_exit(&mut p.child);
    assert_eq!(st.code(), Some(128 + libc::SIGHUP));
    assert_gone(child);
}
