//! The drawing process of `svg_proc`, tested through the real konoma binary.
//!
//! The unit-test executable is not konoma, so these tests start the binary Cargo built next to it
//! (`target/<profile>/konoma`; the integration tests in `tests/` make `cargo test` build it) and
//! drive it through `svg_proc::run_with`, exactly as the application does. Hostile files here are
//! the shapes that used to abort, hang or exhaust memory the whole process; each is generated in
//! code (small enough to read, big enough to hurt) and must now cost the *child* only.
//!
//! Every test that starts a child holds `SERIAL`: the registry of live children is process-wide,
//! and a test that counts or kills them must not see another test's.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use image::DynamicImage;

use super::svg_guard::SvgFail;
use super::svg_proc::{
    busy_children, idle_children, kill_live, live_children, live_pids, run_with, Limits, Request,
    RunError,
};
use crate::test_support::TmpDir;

/// A fresh directory that disappears with the guard.
fn unique_tmp(prefix: &str) -> TmpDir {
    let dir = crate::test_support::unique_tmp(prefix);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

static SERIAL: Mutex<()> = Mutex::new(());

/// Take the lock that keeps tests that start children one at a time, and start from nothing: the
/// idle children an earlier test left behind are stopped and reaped.
fn serial() -> std::sync::MutexGuard<'static, ()> {
    let guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    kill_live();
    guard
}

/// The konoma binary: `KONOMA_TEST_BIN` when set (used to measure a release build), else the one
/// Cargo placed next to the test executable.
fn bin() -> PathBuf {
    if let Ok(p) = std::env::var("KONOMA_TEST_BIN") {
        return PathBuf::from(p);
    }
    let exe = std::env::current_exe().unwrap();
    let p = exe.parent().unwrap().parent().unwrap().join("konoma");
    assert!(
        p.is_file(),
        "{} is missing: run `cargo build` (or `cargo test`, which builds it for tests/)",
        p.display()
    );
    p
}

/// Nothing is running and, once the idle children are stopped too, nothing is left alive (none
/// unreaped either: the registry is only emptied when a child has been waited for).
fn no_children_left() {
    assert_eq!(busy_children(), 0, "a drawing is still running");
    kill_live();
    assert_eq!(live_children(), 0, "a child process is left");
}

fn limits() -> Limits {
    Limits {
        wall: Duration::from_secs(4),
        rss: 700 << 20,
    }
}

fn own_rss() -> u64 {
    super::svg_proc::rss_bytes(std::process::id()).unwrap_or(0)
}

/// What the application does for an SVG from a file: the flat pre-checks, then the child.
fn draw(
    svg: &[u8],
    base: Option<&Path>,
    lim: Limits,
    cancelled: &dyn Fn() -> bool,
) -> Result<DynamicImage, RunError> {
    super::svg_guard::precheck(svg)?;
    run_with(
        &bin(),
        lim,
        &Request {
            data: svg,
            base,
            max_px: 800,
        },
        cancelled,
    )
}

fn never() -> bool {
    false
}

fn svg(body: &str) -> String {
    format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="800" height="600">{body}</svg>"#
    )
}

/// What a hostile file may cost: the answer comes within the limit (plus start-up and teardown),
/// the supervisor's own memory does not grow, and no child is left running.
struct Outcome {
    result: Result<DynamicImage, RunError>,
    took: Duration,
}

fn contained(name: &str, doc: &[u8], base: Option<&Path>, lim: Limits) -> Outcome {
    let _g = serial();
    let before = own_rss();
    let t = Instant::now();
    let result = draw(doc, base, lim, &never);
    let took = t.elapsed();
    let grew = own_rss().saturating_sub(before);
    assert!(
        took < lim.wall + Duration::from_secs(3),
        "{name}: answered after {took:?} (limit {:?})",
        lim.wall
    );
    assert!(
        grew < 400 << 20,
        "{name}: the supervising process grew by {} MiB",
        grew >> 20
    );
    assert_eq!(busy_children(), 0, "{name}: a drawing is still running");
    Outcome { result, took }
}

fn must_not_draw(name: &str, doc: &[u8]) -> SvgFail {
    let o = contained(name, doc, None, limits());
    match o.result {
        Err(RunError::Failed(f)) => f,
        other => panic!(
            "{name}: expected a refusal, got {:?}",
            other.map(|i| i.width())
        ),
    }
}

// ---- the child draws what the process would ------------------------------------------------

#[test]
fn the_child_draws_exactly_what_the_in_process_renderer_draws() {
    let _g = serial();
    let samples = Path::new(env!("CARGO_MANIFEST_DIR")).join("samples");
    let mut docs: Vec<(String, Vec<u8>, Option<PathBuf>)> = vec![
        (
            "rect".into(),
            svg(r##"<rect width="800" height="600" fill="#08f"/><circle cx="400" cy="300" r="100" fill="red" opacity=".5"/>"##).into_bytes(),
            None,
        ),
        (
            "badge-like text".into(),
            svg(r##"<rect width="300" height="40" rx="4" fill="#555"/><text x="10" y="25" font-size="14" fill="#fff">build passing</text>"##).into_bytes(),
            None,
        ),
        (
            "gradient + filter".into(),
            svg(r##"<defs><linearGradient id="g"><stop offset="0" stop-color="red"/><stop offset="1" stop-color="blue"/></linearGradient><filter id="f"><feGaussianBlur stdDeviation="3"/></filter></defs><rect width="800" height="600" fill="url(#g)" filter="url(#f)"/>"##).into_bytes(),
            None,
        ),
    ];
    for e in std::fs::read_dir(&samples).unwrap().flatten() {
        if e.path().extension().and_then(|x| x.to_str()) == Some("svg") {
            docs.push((
                e.file_name().to_string_lossy().into_owned(),
                std::fs::read(e.path()).unwrap(),
                Some(samples.clone()),
            ));
        }
    }
    for (name, doc, base) in docs {
        let in_process = super::svg::rasterize_guarded(&doc, base.as_deref(), 800)
            .unwrap_or_else(|f| panic!("{name}: in-process drawing failed: {f:?}"));
        let child = draw(&doc, base.as_deref(), limits(), &never)
            .unwrap_or_else(|f| panic!("{name}: the child failed: {f:?}"));
        assert_eq!(
            (child.width(), child.height()),
            (in_process.width(), in_process.height()),
            "{name}"
        );
        assert!(
            child.to_rgba8().as_raw() == in_process.to_rgba8().as_raw(),
            "{name}: the child's pixels differ from the in-process pixels"
        );
    }
}

#[test]
fn a_refusal_made_inside_the_child_keeps_its_reason() {
    // A `<use>` bomb passes the flat checks (it is shallow) and is refused by the XML check inside
    // the child.
    let mut defs = String::from(r#"<defs><g id="u0"><rect width="1" height="1"/></g>"#);
    for i in 1..22 {
        defs.push_str(&format!(
            r##"<g id="u{i}"><use href="#u{}"/><use href="#u{}"/></g>"##,
            i - 1,
            i - 1
        ));
    }
    defs.push_str(r##"</defs><use href="#u21"/>"##);
    assert_eq!(
        must_not_draw("use bomb", svg(&defs).as_bytes()),
        SvgFail::TooComplex
    );
    // Not an SVG at all.
    assert_eq!(
        must_not_draw("text file", b"hello, world"),
        SvgFail::Invalid
    );
}

// ---- the flat pre-check refuses without starting a process -----------------------------------

#[test]
fn deep_nesting_is_refused_before_any_process_starts() {
    let _g = serial();
    let doc = svg(&format!("{}{}", "<g>".repeat(1000), "</g>".repeat(1000)));
    let t = Instant::now();
    let r = draw(doc.as_bytes(), None, limits(), &never);
    assert_eq!(r.err(), Some(RunError::Failed(SvgFail::TooDeep)));
    assert!(
        t.elapsed() < Duration::from_millis(200),
        "{:?}",
        t.elapsed()
    );
    no_children_left();
}

// ---- stopping, crashing, cancelling ------------------------------------------------------------

/// An executable shell script standing in for the child, so the supervisor's behaviour can be
/// tested without a file that makes the real renderer misbehave. Writes its pid to `pid_file`.
fn script(dir: &Path, name: &str, body: &str, pid_file: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let p = dir.join(name);
    std::fs::write(
        &p,
        format!("#!/bin/sh\necho $$ > '{}'\n{body}\n", pid_file.display()),
    )
    .unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    p
}

fn gone(pid: i32) -> bool {
    // SAFETY: signal 0 only checks that the process exists. A zombie still exists, so this also
    // proves the child was reaped.
    unsafe { libc::kill(pid, 0) != 0 }
}

/// The pid the script wrote (the file exists a moment before it has content).
fn pid_of(f: &Path) -> i32 {
    let t = Instant::now();
    loop {
        if let Some(pid) = std::fs::read_to_string(f)
            .ok()
            .and_then(|s| s.trim().parse().ok())
        {
            return pid;
        }
        assert!(
            t.elapsed() < Duration::from_secs(20),
            "the script never wrote its pid"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn tiny_request() -> Request<'static> {
    Request {
        data: b"<svg/>",
        base: None,
        max_px: 100,
    }
}

#[test]
fn a_child_that_runs_too_long_is_killed_and_reaped() {
    let _g = serial();
    let dir = unique_tmp("svg-proc-timeout");
    let pid_file = dir.join("pid");
    let exe = script(&dir, "sleeper", "exec sleep 60", &pid_file);
    // Generous: the shell has to start before it writes its pid, on a machine that is also running
    // thousands of other tests.
    let lim = Limits {
        wall: Duration::from_millis(2500),
        rss: 1 << 30,
    };
    let t = Instant::now();
    let r = run_with(&exe, lim, &tiny_request(), &never);
    let took = t.elapsed();
    assert_eq!(r.err(), Some(RunError::Failed(SvgFail::Timeout)));
    assert!(
        took >= Duration::from_millis(2500) && took < Duration::from_secs(5),
        "{took:?}"
    );
    assert!(
        gone(pid_of(&pid_file)),
        "the child is still there (or a zombie)"
    );
    no_children_left();
}

#[test]
fn a_child_that_dies_by_a_signal_is_a_crash_not_a_hang() {
    let _g = serial();
    let dir = unique_tmp("svg-proc-crash");
    let pid_file = dir.join("pid");
    // What a stack overflow looks like from outside: the process is gone by a signal. (SIGKILL, not
    // SIGSEGV or SIGABRT: those would make the operating system write a crash report for the shell
    // into the user's diagnostics folder on every test run.)
    let exe = script(&dir, "crasher", "kill -KILL $$", &pid_file);
    let t = Instant::now();
    let r = run_with(&exe, Limits::default(), &tiny_request(), &never);
    assert_eq!(r.err(), Some(RunError::Failed(SvgFail::Crashed)));
    assert!(t.elapsed() < Duration::from_secs(2), "{:?}", t.elapsed());
    assert!(gone(pid_of(&pid_file)));
    no_children_left();
}

#[test]
fn garbage_and_truncated_answers_are_a_crash() {
    let _g = serial();
    let dir = unique_tmp("svg-proc-garbage");
    let pid_file = dir.join("pid");
    for (name, body) in [
        ("garbage", "echo 'not a response'"),
        ("empty", "true"),
        (
            "truncated",
            "printf 'KSR1\\000\\002\\000\\000\\000\\002\\000\\000\\000abc'",
        ),
        ("fail", "printf 'KSR1\\004'"),
    ] {
        let exe = script(&dir, name, body, &pid_file);
        let r = run_with(&exe, Limits::default(), &tiny_request(), &never);
        let want = if name == "fail" {
            SvgFail::TooHeavy
        } else {
            SvgFail::Crashed
        };
        assert_eq!(r.err(), Some(RunError::Failed(want)), "{name}");
    }
    no_children_left();
}

#[test]
fn a_stale_request_stops_its_child_at_once() {
    let _g = serial();
    let dir = unique_tmp("svg-proc-cancel");
    let pid_file = dir.join("pid");
    let exe = script(&dir, "sleeper", "exec sleep 60", &pid_file);
    // The preview "moves on" 150 ms after the child was seen running (it writes its pid first).
    let seen = std::cell::Cell::new(None::<Instant>);
    let cancelled = || {
        if seen.get().is_none()
            && std::fs::read_to_string(&pid_file).is_ok_and(|s| !s.trim().is_empty())
        {
            seen.set(Some(Instant::now()));
        }
        seen.get()
            .is_some_and(|t| t.elapsed() > Duration::from_millis(150))
    };
    let started = Instant::now();
    let r = run_with(&exe, Limits::default(), &tiny_request(), &cancelled);
    assert_eq!(r.err(), Some(RunError::Cancelled));
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "{:?}",
        started.elapsed()
    );
    assert!(gone(pid_of(&pid_file)));
    no_children_left();
}

#[test]
fn a_request_cancelled_while_waiting_for_a_slot_never_starts_a_child() {
    let _g = serial();
    let dir = unique_tmp("svg-proc-slot");
    let pid_file = dir.join("pid");
    let exe = script(&dir, "sleeper", "exec sleep 5", &pid_file);
    let cap = super::svg_proc::max_children();
    let started = Instant::now();
    let stop = std::sync::atomic::AtomicBool::new(false);
    std::thread::scope(|s| {
        // Fill every slot with a child that stays.
        for _ in 0..cap {
            let (exe, stop) = (&exe, &stop);
            s.spawn(move || {
                let _ = run_with(
                    exe,
                    Limits {
                        wall: Duration::from_secs(10),
                        rss: 1 << 30,
                    },
                    &tiny_request(),
                    &|| stop.load(std::sync::atomic::Ordering::SeqCst),
                );
            });
        }
        while live_children() < cap {
            std::thread::sleep(Duration::from_millis(10));
            assert!(started.elapsed() < Duration::from_secs(10));
        }
        // One more cannot get a slot; cancelling it ends the wait without a process.
        let t = Instant::now();
        let r = run_with(&exe, Limits::default(), &tiny_request(), &|| {
            t.elapsed() > Duration::from_millis(200)
        });
        assert_eq!(r.err(), Some(RunError::Cancelled));
        assert_eq!(live_children(), cap, "the waiting request started a child");
        stop.store(true, std::sync::atomic::Ordering::SeqCst);
    });
    no_children_left();
}

#[test]
fn at_most_max_children_run_at_once() {
    let _g = serial();
    let dir = unique_tmp("svg-proc-cap");
    let pid_file = dir.join("pid");
    let exe = script(&dir, "short", "sleep 0.3", &pid_file);
    let cap = super::svg_proc::max_children();
    let peak = std::sync::atomic::AtomicUsize::new(0);
    let done = std::sync::atomic::AtomicBool::new(false);
    std::thread::scope(|s| {
        s.spawn(|| {
            while !done.load(std::sync::atomic::Ordering::SeqCst) {
                peak.fetch_max(live_children(), std::sync::atomic::Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(5));
            }
        });
        std::thread::scope(|inner| {
            for _ in 0..cap * 3 {
                let exe = &exe;
                inner.spawn(move || {
                    let _ = run_with(exe, Limits::default(), &tiny_request(), &never);
                });
            }
        });
        done.store(true, std::sync::atomic::Ordering::SeqCst);
    });
    let peak = peak.load(std::sync::atomic::Ordering::SeqCst);
    assert!(
        peak >= 1 && peak <= cap,
        "peak {peak} children with a cap of {cap}"
    );
    no_children_left();
}

#[test]
fn killing_the_live_children_stops_them() {
    let _g = serial();
    let dir = unique_tmp("svg-proc-killall");
    let pid_file = dir.join("pid");
    let exe = script(&dir, "sleeper", "exec sleep 60", &pid_file);
    std::thread::scope(|s| {
        let h = s.spawn(|| {
            run_with(
                &exe,
                Limits {
                    wall: Duration::from_secs(30),
                    rss: 1 << 30,
                },
                &tiny_request(),
                &never,
            )
        });
        pid_of(&pid_file);
        super::svg_proc::kill_live();
        let r = h.join().unwrap();
        // Killed from outside: no valid answer, so a crash — and the thread came back.
        assert_eq!(r.err(), Some(RunError::Failed(SvgFail::Crashed)));
    });
    assert!(gone(pid_of(&pid_file)));
    no_children_left();
}

// ---- one child, many drawings ---------------------------------------------------------------

fn badge(i: usize) -> String {
    svg(&format!(
        r##"<rect width="300" height="40" rx="4" fill="#555"/><text x="10" y="25" font-size="14" fill="#fff">badge {i}</text>"##
    ))
}

#[test]
fn many_drawings_are_served_by_one_child_and_text_still_gets_its_fonts() {
    let _g = serial();
    no_children_left();
    // A document with no text first: that child does not enumerate the fonts. Then text, in the
    // very same child: it must get them (a regression here draws text as nothing).
    let plain = svg(r##"<rect width="300" height="40" fill="#08f"/>"##);
    let first = draw(plain.as_bytes(), None, limits(), &never).expect("plain draws");
    let with_text = badge(0);
    let drawn = draw(with_text.as_bytes(), None, limits(), &never).expect("text draws");
    let expected = super::svg::rasterize_guarded(with_text.as_bytes(), None, 800).unwrap();
    assert!(
        drawn.to_rgba8().as_raw() == expected.to_rgba8().as_raw(),
        "text drawn by a child that had drawn a text-free SVG first differs from the in-process pixels"
    );
    assert!(first.width() > 0);
    for i in 1..30 {
        draw(badge(i).as_bytes(), None, limits(), &never).expect("badge draws");
    }
    assert_eq!(live_children(), 1, "thirty-two drawings, one process");
    assert_eq!(idle_children(), 1, "and it waits for the next");
    no_children_left();
}

#[test]
fn the_number_of_children_stays_within_the_cap_for_a_document_full_of_badges() {
    let _g = serial();
    no_children_left();
    let cap = super::svg_proc::max_children();
    let peak = std::sync::atomic::AtomicUsize::new(0);
    let done = std::sync::atomic::AtomicBool::new(false);
    let ok = std::sync::atomic::AtomicUsize::new(0);
    std::thread::scope(|s| {
        s.spawn(|| {
            while !done.load(std::sync::atomic::Ordering::SeqCst) {
                peak.fetch_max(live_children(), std::sync::atomic::Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(2));
            }
        });
        std::thread::scope(|inner| {
            for i in 0..50 {
                let ok = &ok;
                inner.spawn(move || {
                    if draw(badge(i).as_bytes(), None, limits(), &never).is_ok() {
                        ok.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    }
                });
            }
        });
        done.store(true, std::sync::atomic::Ordering::SeqCst);
    });
    assert_eq!(ok.load(std::sync::atomic::Ordering::SeqCst), 50);
    let peak = peak.load(std::sync::atomic::Ordering::SeqCst);
    assert!(peak >= 1 && peak <= cap, "peak {peak} processes, cap {cap}");
    assert!(idle_children() <= cap);
    no_children_left();
}

#[test]
fn a_refusal_made_by_the_child_leaves_the_child_fit_for_the_next_drawing() {
    let _g = serial();
    no_children_left();
    // Past the flat checks on purpose (sent straight to the child): it refuses and goes on.
    let deep = svg(&format!("{}{}", "<g>".repeat(1000), "</g>".repeat(1000)));
    let r = run_with(
        &bin(),
        limits(),
        &Request {
            data: deep.as_bytes(),
            base: None,
            max_px: 800,
        },
        &never,
    );
    assert_eq!(r.err(), Some(RunError::Failed(SvgFail::TooDeep)));
    assert!(draw(badge(1).as_bytes(), None, limits(), &never).is_ok());
    assert_eq!(live_children(), 1, "the same process answered both");
    no_children_left();
}

#[test]
fn a_child_that_had_to_be_stopped_is_never_asked_again() {
    let _g = serial();
    no_children_left();
    assert!(draw(badge(1).as_bytes(), None, limits(), &never).is_ok());
    let first_pid = live_pids();
    assert_eq!(first_pid.len(), 1);
    // Taken over by a drawing that outlasts its limit: stopped.
    let slow = svg(&r#"<rect width="800" height="600"/>"#.repeat(100_000));
    let lim = Limits {
        wall: Duration::from_millis(800),
        rss: 1 << 30,
    };
    let r = draw(slow.as_bytes(), None, lim, &never);
    assert_eq!(r.err(), Some(RunError::Failed(SvgFail::Timeout)));
    assert_eq!(live_children(), 0, "the stopped child was reaped");
    assert_eq!(idle_children(), 0);
    // The next drawing gets a new process.
    assert!(draw(badge(2).as_bytes(), None, limits(), &never).is_ok());
    let second_pid = live_pids();
    assert_eq!(second_pid.len(), 1);
    assert_ne!(first_pid, second_pid);
    no_children_left();
}

#[test]
fn an_idle_child_that_died_is_replaced_without_the_caller_noticing() {
    let _g = serial();
    no_children_left();
    assert!(draw(badge(1).as_bytes(), None, limits(), &never).is_ok());
    let pids = live_pids();
    assert_eq!(pids.len(), 1);
    // Killed from outside while it waits (what its own idle timer does after IDLE_EXIT).
    super::svg_proc::kill_pid(pids[0]);
    let r = draw(badge(2).as_bytes(), None, limits(), &never);
    assert!(r.is_ok(), "{:?}", r.err());
    let after = live_pids();
    assert_eq!(after.len(), 1);
    assert_ne!(after, pids);
    no_children_left();
}

// ---- the production entry points -------------------------------------------------------------

/// `svg::rasterize_untrusted` and `svg::rasterize`, as the application calls them, with the real
/// binary as the drawing process: the flat checks come first and cost no process, the rest goes to
/// a child.
#[test]
fn the_production_path_refuses_what_it_can_before_starting_a_process_and_draws_the_rest_in_one() {
    let _g = serial();
    no_children_left();
    let dir = unique_tmp("svg-proc-production");
    let path = dir.join("x.svg");
    super::svg_proc::with_real_child(bin(), || {
        let never = &|| false;
        // Too deep: refused by the flat scan, no process.
        let deep = svg(&format!("{}{}", "<g>".repeat(1000), "</g>".repeat(1000)));
        assert_eq!(
            super::svg::rasterize_untrusted(deep.as_bytes(), &path, 100, never).err(),
            Some(SvgFail::TooDeep)
        );
        assert_eq!(
            live_children(),
            0,
            "a process was started for a file the scan refuses"
        );
        // Too large: refused by size, no process.
        let big = vec![b' '; super::svg_guard::MAX_SVG_BYTES + 1];
        assert_eq!(
            super::svg::rasterize_untrusted(&big, &path, 100, never).err(),
            Some(SvgFail::TooLarge)
        );
        assert_eq!(live_children(), 0);
        // An ordinary one: drawn by a child.
        let ok = badge(1);
        let img = super::svg::rasterize_untrusted(ok.as_bytes(), &path, 100, never).unwrap();
        assert_eq!(
            (img.width(), img.height()),
            (800, 600),
            "its own size: drawing never shrinks"
        );
        assert_eq!(live_children(), 1);
        // A file: read, then the same.
        std::fs::write(&path, &ok).unwrap();
        assert!(super::svg::rasterize(&path, 100, never).is_ok());
        assert_eq!(live_children(), 1, "the same child served both");
        // A compressed one is not scanned here (the child decompresses it, bounded): small ones
        // draw, deep ones are refused by the child with the same reason.
        use std::io::Write;
        let gz = |d: &[u8]| {
            let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
            e.write_all(d).unwrap();
            e.finish().unwrap()
        };
        assert!(super::svg::rasterize_untrusted(&gz(ok.as_bytes()), &path, 100, never).is_ok());
        assert_eq!(
            super::svg::rasterize_untrusted(&gz(deep.as_bytes()), &path, 100, never).err(),
            Some(SvgFail::TooDeep)
        );
        // Refusals that need the parser come back from the child with their reason.
        let bomb = svg(&format!(
            r##"<defs><g id="u0"><rect width="1" height="1"/></g>{}</defs><use href="#u22"/>"##,
            (1..=22)
                .map(|i| format!(
                    r##"<g id="u{i}"><use href="#u{}"/><use href="#u{}"/></g>"##,
                    i - 1,
                    i - 1
                ))
                .collect::<String>()
        ));
        assert_eq!(
            super::svg::rasterize_untrusted(bomb.as_bytes(), &path, 100, never).err(),
            Some(SvgFail::TooComplex)
        );
        assert_eq!(live_children(), 1, "through all of it, one child");
    });
    no_children_left();
}

/// Waits (up to `secs`) for `done` to hold.
fn eventually(secs: u64, done: impl Fn() -> bool) -> bool {
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(secs) {
        if done() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    done()
}

#[test]
fn an_idle_child_is_stopped_by_konoma_after_a_while_and_not_left_for_its_own_timer() {
    let _g = serial();
    struct ShortRetire;
    impl Drop for ShortRetire {
        fn drop(&mut self) {
            super::svg_proc::RETIRE_AFTER_MS_FOR_TESTS
                .store(0, std::sync::atomic::Ordering::SeqCst);
        }
    }
    let _short = ShortRetire;
    super::svg_proc::RETIRE_AFTER_MS_FOR_TESTS.store(600, std::sync::atomic::Ordering::SeqCst);
    assert!(draw(badge(1).as_bytes(), None, limits(), &never).is_ok());
    let pid = live_pids()[0] as i32;
    assert_eq!(idle_children(), 1);
    assert!(
        eventually(10, || idle_children() == 0 && live_children() == 0),
        "the idle child was never retired"
    );
    assert!(
        gone(pid),
        "retired, and reaped (a zombie would still answer a signal)"
    );
    // The next drawing simply starts a new one.
    assert!(draw(badge(2).as_bytes(), None, limits(), &never).is_ok());
    no_children_left();
}

#[test]
fn an_idle_child_that_dies_is_reaped_at_once_not_when_the_next_drawing_comes() {
    let _g = serial();
    assert!(draw(badge(1).as_bytes(), None, limits(), &never).is_ok());
    let pid = live_pids()[0];
    super::svg_proc::kill_pid(pid);
    // Well inside the 8 s a live idle child is kept: it is the death that is noticed.
    assert!(
        eventually(4, || live_children() == 0),
        "the dead child was left in the pool"
    );
    assert!(gone(pid as i32), "reaped: not a zombie");
    assert_eq!(idle_children(), 0);
    no_children_left();
}

// ---- memory: the real renderer, a real allocation -------------------------------------------

#[test]
fn a_huge_pattern_is_stopped_at_the_memory_limit_not_after_gigabytes() {
    // 30,000 x 30,000 px pattern tile = 3.6 GB of pixels if it is drawn.
    let doc = svg(
        r#"<defs><pattern id="p" width="30000" height="30000" patternUnits="userSpaceOnUse"><rect width="30000" height="30000" fill="red"/></pattern></defs><rect width="800" height="600" fill="url(#p)"/>"#,
    );
    let lim = Limits {
        wall: Duration::from_secs(8),
        rss: 400 << 20,
    };
    let o = contained("pattern_huge", doc.as_bytes(), None, lim);
    match o.result {
        Err(RunError::Failed(SvgFail::Memory)) => {}
        other => panic!(
            "expected a stop for memory, got {:?}",
            other.map(|i| i.width())
        ),
    }
    // How far past the limit the child got before it was stopped (the polling interval's worth of
    // allocation). Printed for the record; bounded so a regression in the watch shows up.
    let peak = super::svg_proc::LAST_PEAK_RSS.load(std::sync::atomic::Ordering::SeqCst);
    eprintln!(
        "memory watch: limit {} MiB, last seen {} MiB",
        lim.rss >> 20,
        peak >> 20
    );
    assert!(peak > lim.rss, "the stop was not because of the limit");
    assert!(
        peak < lim.rss + (400 << 20),
        "overshoot {} MiB",
        (peak - lim.rss) >> 20
    );
}

// ---- the families of hostile files that got past the in-process checks ----------------------

fn clip_chain(n: usize) -> String {
    let mut s = String::from(r#"<clipPath id="c0"><rect width="800" height="600"/></clipPath>"#);
    for i in 1..n {
        s.push_str(&format!(
            r##"<clipPath id="c{i}" clip-path="url(#c{})"><rect width="800" height="600"/></clipPath>"##,
            i - 1
        ));
    }
    svg(&format!(
        r##"<defs>{s}</defs><rect width="800" height="600" clip-path="url(#c{})"/>"##,
        n - 1
    ))
}

fn mask_chain(n: usize) -> String {
    let mut s =
        String::from(r#"<mask id="m0"><rect width="800" height="600" fill="white"/></mask>"#);
    for i in 1..n {
        s.push_str(&format!(
            r##"<mask id="m{i}" mask="url(#m{})"><rect width="800" height="600" fill="white"/></mask>"##,
            i - 1
        ));
    }
    svg(&format!(
        r##"<defs>{s}</defs><rect width="800" height="600" mask="url(#m{})"/>"##,
        n - 1
    ))
}

fn nested_images(count: usize) -> String {
    let inner = svg(r#"<rect width="800" height="600" fill="red" opacity=".5"/>"#);
    let b64 = base64_of(inner.as_bytes());
    let mut s = String::new();
    for _ in 0..count {
        s.push_str(&format!(
            r#"<image width="800" height="600" href="data:image/svg+xml;base64,{b64}"/>"#
        ));
    }
    svg(&s)
}

fn base64_of(b: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(b)
}

#[test]
fn every_family_of_hostile_svg_costs_only_the_child() {
    let docs: Vec<(&str, String)> = vec![
        ("clip_chain_1000", clip_chain(1000)),
        ("mask_chain_1000", mask_chain(1000)),
        ("images_2000", nested_images(2000)),
        (
            "overdraw_rect_100k",
            svg(&r#"<rect width="800" height="600"/>"#.repeat(100_000)),
        ),
        (
            "overdraw_opacity_groups_5k",
            svg(&r#"<g opacity=".5"><rect width="800" height="600"/></g>"#.repeat(5_000)),
        ),
        (
            "gradient_20k",
            svg(&format!(
                r#"<defs><linearGradient id="g"><stop offset="0" stop-color="red"/><stop offset="1" stop-color="blue"/></linearGradient></defs>{}"#,
                r##"<rect width="800" height="600" fill="url(#g)"/>"##.repeat(20_000)
            )),
        ),
        (
            "filter_300k",
            svg(&format!(
                r##"<defs><filter id="f"><feGaussianBlur stdDeviation="1"/></filter></defs>{}"##,
                r##"<rect width="1" height="1" filter="url(#f)"/>"##.repeat(300_000)
            )),
        ),
        (
            "dash_many",
            svg(&r#"<path d="M0 0 L800 600" stroke="black" stroke-width="3" stroke-dasharray="0.001 0.001"/>"#.repeat(3000)),
        ),
        (
            "text_200k",
            svg(&format!(
                r#"<text x="10" y="50" font-size="12">{}</text>"#,
                "W".repeat(200_000)
            )),
        ),
    ];
    let lim = Limits {
        wall: Duration::from_secs(2),
        rss: 700 << 20,
    };
    for (name, doc) in &docs {
        let o = contained(name, doc.as_bytes(), None, lim);
        eprintln!(
            "{name}: {} in {:?}",
            match &o.result {
                Ok(i) => format!("drawn {}x{}", i.width(), i.height()),
                Err(e) => format!("{e:?}"),
            },
            o.took
        );
        // Each of these took tens of seconds, gigabytes or the whole process in the old
        // in-process renderer; here it is stopped (or refused) inside its limits.
        assert!(
            matches!(o.result, Err(RunError::Failed(_))),
            "{name}: expected the drawing to be stopped or refused"
        );
    }
}

#[test]
fn referenced_svg_files_cannot_take_the_child_down() {
    let _g = serial();
    let dir = unique_tmp("svg-proc-ext");
    // 1000 nested groups in a file next to the SVG: 1000 stack frames of usvg and resvg each.
    std::fs::write(
        dir.join("deep.svg"),
        svg(&format!(
            "{}{}",
            r#"<g opacity="0.9">"#.repeat(1000),
            "</g>".repeat(1000)
        )),
    )
    .unwrap();
    // A gzip bomb: 300 MB of spaces in a few hundred KB.
    {
        use std::io::Write;
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        gz.write_all(b"<svg xmlns=\"http://www.w3.org/2000/svg\">")
            .unwrap();
        let chunk = vec![b' '; 1 << 20];
        for _ in 0..300 {
            gz.write_all(&chunk).unwrap();
        }
        gz.write_all(b"</svg>").unwrap();
        std::fs::write(dir.join("bomb.svgz"), gz.finish().unwrap()).unwrap();
    }
    for (name, target) in [("deep", "deep.svg"), ("bomb", "bomb.svgz")] {
        let outer = svg(&format!(
            r##"<rect width="800" height="600" fill="#0a0"/><image width="800" height="600" href="{target}"/>"##
        ));
        let t = Instant::now();
        let r = draw(outer.as_bytes(), Some(&dir), limits(), &never);
        // The refused file is simply not drawn: the outer picture still comes out whole, and
        // quickly. (Before, the deep file overflowed the stack and the bomb ate gigabytes.)
        assert!(
            t.elapsed() < Duration::from_secs(4),
            "{name}: {:?}",
            t.elapsed()
        );
        let img = r.unwrap_or_else(|e| panic!("{name}: the outer picture failed: {e:?}"));
        let px = img.to_rgba8();
        assert_eq!(
            px.get_pixel(400, 300).0,
            [0, 170, 0, 255],
            "{name}: the outer rect is drawn"
        );
    }
    no_children_left();
}

#[test]
fn a_stack_overflowing_drawing_kills_only_its_child() {
    // Not reachable through the checks any more, so the child's death is staged with the real
    // binary's own protocol: a request whose declared size is a lie makes it exit at once without
    // an answer. The supervisor must see "no answer" and carry on.
    let _g = serial();
    let dir = unique_tmp("svg-proc-lie");
    let pid_file = dir.join("pid");
    let exe = script(&dir, "overflow", "kill -KILL $$", &pid_file);
    let r = run_with(&exe, Limits::default(), &tiny_request(), &never);
    assert_eq!(r.err(), Some(RunError::Failed(SvgFail::Crashed)));
    // The application is alive, and the next drawing works.
    let ok = draw(
        svg(r#"<rect width="10" height="10"/>"#).as_bytes(),
        None,
        limits(),
        &never,
    );
    assert!(ok.is_ok());
}

// ---- measurements (run by hand: cargo test --release -- --ignored --nocapture) ---------------

/// Draws every `.svg`/`.svgz` in `$SVG_BENCH_DIR` through the child and prints one line each:
/// outcome, wall time, the supervisor's own memory growth.
#[test]
#[ignore]
fn bench_files() {
    let _g = serial();
    let dir = PathBuf::from(std::env::var("SVG_BENCH_DIR").expect("SVG_BENCH_DIR"));
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| matches!(p.extension().and_then(|e| e.to_str()), Some("svg" | "svgz")))
        .collect();
    files.sort();
    if let Ok(only) = std::env::var("SVG_BENCH_ONLY") {
        files.retain(|p| p.file_name().unwrap().to_string_lossy().contains(&only));
    }
    for p in files {
        let Ok(data) = std::fs::read(&p) else {
            continue;
        };
        let before = own_rss();
        let t = Instant::now();
        let r = draw(&data, p.parent(), Limits::default(), &never);
        let took = t.elapsed();
        let grew = own_rss() as i64 - before as i64;
        println!(
            "BENCH {:<34} {:<22} {:>7.0} ms  parent {:+} MiB",
            p.file_name().unwrap().to_string_lossy(),
            match &r {
                Ok(i) => format!("drawn {}x{}", i.width(), i.height()),
                Err(RunError::Failed(f)) => format!("{f:?}"),
                Err(RunError::Cancelled) => "cancelled".into(),
            },
            took.as_secs_f64() * 1000.0,
            grew >> 20
        );
    }
}

fn fnv(b: &[u8]) -> u64 {
    let mut h = 0xcbf29ce484222325u64;
    for &x in b {
        h ^= x as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// Prints a hash of the raster each file in `$HASH_LIST` (one path per line) gets from the child,
/// in the format of the baseline probe, so the two can be compared line by line.
#[test]
#[ignore]
fn hash_files() {
    let _g = serial();
    let list = std::fs::read_to_string(std::env::var("HASH_LIST").unwrap()).unwrap();
    for line in list.lines().filter(|l| !l.trim().is_empty()) {
        let p = Path::new(line.trim());
        let Ok(data) = std::fs::read(p) else { continue };
        let t = Instant::now();
        // `HASH_TRUSTED=1`: the in-process door (what konoma's own mermaid and math SVGs use).
        let r = if std::env::var("HASH_TRUSTED").is_ok() {
            super::svg::rasterize_trusted(&data, p, 800).ok_or(RunError::Failed(SvgFail::Invalid))
        } else {
            draw(&data, p.parent(), Limits::default(), &never)
        };
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        match r {
            Ok(img) => println!(
                "HASH {} {}x{} {:016x} {:.0}ms",
                p.display(),
                img.width(),
                img.height(),
                fnv(img.to_rgba8().as_raw()),
                ms
            ),
            Err(e) => println!("HASH {} none {:?} {:.0}ms", p.display(), e, ms),
        }
    }
}

/// Writes konoma's own mermaid and math SVGs to `$HASH_DUMP_DIR` (for the hash comparison).
#[test]
#[ignore]
fn dump_trusted_svgs() {
    let dir = PathBuf::from(std::env::var("HASH_DUMP_DIR").unwrap());
    std::fs::create_dir_all(&dir).unwrap();
    let flows = [
        "flowchart LR\n  A[Start] --> B{Choice}\n  B -->|yes| C[Done]\n  B -->|no| D[Retry]",
        "sequenceDiagram\n  Alice->>Bob: Hello\n  Bob-->>Alice: Hi",
        "pie title Pets\n  \"Dogs\" : 386\n  \"Cats\" : 85",
        "stateDiagram-v2\n  [*] --> A\n  A --> B\n  B --> [*]",
        "gantt\n  title T\n  dateFormat YYYY-MM-DD\n  section S\n  A :a1, 2024-01-01, 5d",
    ];
    for (i, code) in flows.iter().enumerate() {
        let svg =
            crate::preview::markdown::mermaid_to_svg_flow(code, "default", "basis", "splines")
                .unwrap();
        std::fs::write(dir.join(format!("mermaid{i}.svg")), svg).unwrap();
    }
    for (i, (latex, display)) in [
        ("E = mc^2", false),
        ("\\frac{a}{b} + \\sum_{i=0}^n i^2", true),
        ("x", false),
    ]
    .iter()
    .enumerate()
    {
        let svg = crate::preview::math::latex_to_svg(latex, *display, "#d0d0d0").unwrap();
        std::fs::write(dir.join(format!("math{i}.svg")), svg).unwrap();
    }
}
