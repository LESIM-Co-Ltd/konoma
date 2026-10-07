//! The SVG drawing process (`konoma --internal-svg-render`), driven the way the application
//! drives it: a request on stdin, a response on stdout. These run the real binary, so they cover
//! what unit tests cannot: that the mode exists and is not advertised, that it answers on the
//! wire, that it holds nothing it inherited, and that it ends by itself if nobody supervises it.

#![cfg(unix)]

use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const FLAG: &str = "--internal-svg-render";

fn konoma() -> Command {
    Command::new(env!("CARGO_BIN_EXE_konoma"))
}

fn request(data: &[u8], base: &[u8], max_px: u32) -> Vec<u8> {
    let mut r = b"KSV1".to_vec();
    r.extend_from_slice(&max_px.to_le_bytes());
    r.extend_from_slice(&(base.len() as u32).to_le_bytes());
    r.extend_from_slice(base);
    r.extend_from_slice(&(data.len() as u64).to_le_bytes());
    r.extend_from_slice(data);
    r
}

/// Runs the child on `input` and returns (exit success, stdout).
fn run(input: &[u8]) -> (bool, Vec<u8>) {
    let mut child = konoma()
        .arg(FLAG)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let data = input.to_vec();
    let w = std::thread::spawn(move || {
        let _ = stdin.write_all(&data);
    });
    let mut out = Vec::new();
    child.stdout.take().unwrap().read_to_end(&mut out).unwrap();
    let status = child.wait().unwrap();
    let _ = w.join();
    (status.success(), out)
}

fn svg(body: &str) -> String {
    format!(r#"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10">{body}</svg>"#)
}

#[test]
fn the_drawing_mode_answers_with_a_picture() {
    let (ok, out) = run(&request(
        svg(r##"<rect width="20" height="10" fill="#f00"/>"##).as_bytes(),
        b"",
        200,
    ));
    assert!(ok);
    assert_eq!(&out[..5], b"KSR1\0");
    let w = u32::from_le_bytes(out[5..9].try_into().unwrap());
    let h = u32::from_le_bytes(out[9..13].try_into().unwrap());
    assert_eq!(
        (w, h),
        (200, 100),
        "a 20x10 SVG scaled to a 200 px long side"
    );
    assert_eq!(out.len(), 13 + 200 * 100 * 4);
    // The middle pixel is the opaque red of the fill.
    let mid = 13 + (50 * 200 + 100) * 4;
    assert_eq!(&out[mid..mid + 4], &[255, 0, 0, 255]);
}

#[test]
fn the_drawing_mode_refuses_with_a_reason_instead_of_dying() {
    let deep = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10">{}{}</svg>"#,
        "<g>".repeat(1000),
        "</g>".repeat(1000)
    );
    let (ok, out) = run(&request(deep.as_bytes(), b"", 100));
    assert!(ok, "a refusal is an answer, not a failure");
    assert_eq!(out, [b'K', b'S', b'R', b'1', 1], "1 = too deeply nested");

    let (ok, out) = run(&request(b"this is not an svg", b"", 100));
    assert!(ok);
    assert_eq!(out, [b'K', b'S', b'R', b'1', 8], "8 = not a valid svg");
}

#[test]
fn the_child_serves_request_after_request_and_ends_with_its_input() {
    let ok = svg(r##"<rect width="20" height="10" fill="#00f"/>"##);
    let deep = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10">{}{}</svg>"#,
        "<g>".repeat(1000),
        "</g>".repeat(1000)
    );
    let mut input = request(ok.as_bytes(), b"", 40);
    input.extend(request(deep.as_bytes(), b"", 40));
    input.extend(request(ok.as_bytes(), b"", 80));
    let (success, out) = run(&input);
    assert!(success, "a clean end of input is a clean exit");
    // 1: a 40x20 picture, 2: a refusal, 3: an 80x40 picture.
    let first = 13 + 40 * 20 * 4;
    assert_eq!(&out[..5], b"KSR1\0");
    assert_eq!(&out[first..first + 5], b"KSR1\x01");
    let third = first + 5;
    assert_eq!(&out[third..third + 5], b"KSR1\0");
    assert_eq!(
        u32::from_le_bytes(out[third + 5..third + 9].try_into().unwrap()),
        80
    );
    assert_eq!(out.len(), third + 13 + 80 * 40 * 4);
}

#[test]
fn an_empty_input_is_a_clean_end() {
    let (success, out) = run(b"");
    assert!(success);
    assert!(out.is_empty());
}

#[test]
fn a_malformed_request_is_refused_quickly_without_a_picture() {
    for input in [
        &b"KSV1"[..],
        b"garbage garbage garbage",
        b"KSV1\0\0\0\0\xff\xff\xff\xff",
    ] {
        let t = Instant::now();
        let (ok, out) = run(input);
        assert!(!ok);
        assert!(out.is_empty());
        assert!(t.elapsed() < Duration::from_secs(5));
    }
}

#[test]
fn the_drawing_mode_is_not_advertised() {
    for flag in ["--help", "-h"] {
        let out = konoma().arg(flag).output().unwrap();
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(!text.is_empty(), "{flag} printed nothing");
        assert!(
            !text.contains("internal-svg-render"),
            "{flag} advertises the mode"
        );
        assert!(!text.contains("svg-render"), "{flag}");
    }
}

/// A descriptor the parent leaves open (here the write end of a pipe, as fd 9) must not survive
/// into the child: the pipe's read end sees end-of-file as soon as the parent's own copy is
/// closed, though the child is still alive waiting for its request.
#[test]
fn the_child_holds_no_descriptor_it_inherited() {
    let mut fds = [0i32; 2];
    // SAFETY: `fds` is a valid out-array of two ints.
    assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
    // SAFETY: both fds were just created and are owned here.
    let (read_end, write_end) =
        unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) };
    let w = write_end.as_raw_fd();
    let mut cmd = konoma();
    cmd.arg(FLAG)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // SAFETY: only async-signal-safe calls (dup2) between fork and exec.
    unsafe {
        cmd.pre_exec(move || {
            if libc::dup2(w, 9) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = cmd.spawn().unwrap();
    drop(write_end);
    // The child is blocked reading its request (stdin is still open). Wait for end-of-file.
    let mut pfd = libc::pollfd {
        fd: read_end.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: one valid pollfd.
    let n = unsafe { libc::poll(&mut pfd, 1, 5000) };
    let alive = child.try_wait().unwrap().is_none();
    let _ = child.kill();
    let _ = child.wait();
    assert!(alive, "the child exited before it could be checked");
    assert_eq!(n, 1, "fd 9 is still open in the child");
    assert!(pfd.revents & (libc::POLLHUP | libc::POLLIN) != 0);
}

/// If the supervisor disappears without killing it, the child still ends by itself (an alarm set
/// at start-up), instead of drawing — or waiting — forever.
#[test]
fn an_unsupervised_child_ends_itself() {
    let mut child = konoma()
        .arg(FLAG)
        .stdin(Stdio::piped()) // held open, nothing is ever sent
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let t = Instant::now();
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(!status.success(), "killed by the alarm");
            break;
        }
        if t.elapsed() > Duration::from_secs(40) {
            let _ = child.kill();
            let _ = child.wait();
            panic!("the child is still running after 40 s");
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let took = t.elapsed();
    assert!(
        took > Duration::from_secs(8) && took < Duration::from_secs(25),
        "{took:?}"
    );
}

/// A crash of the drawing process must not leave a core file where the application runs.
#[cfg(target_os = "linux")]
#[test]
fn the_child_does_not_dump_core() {
    let mut child = konoma()
        .arg(FLAG)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_millis(300));
    let mut rl = libc::rlimit {
        rlim_cur: 1,
        rlim_max: 1,
    };
    // SAFETY: prlimit on a child we own, reading into a valid struct.
    let rc = unsafe {
        libc::prlimit(
            child.id() as i32,
            libc::RLIMIT_CORE,
            std::ptr::null(),
            &mut rl,
        )
    };
    let _ = child.kill();
    let _ = child.wait();
    assert_eq!(rc, 0);
    assert_eq!(rl.rlim_cur, 0);
}
