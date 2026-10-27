//! End to end through `ProcPlugin`: the child is a thread over socket pairs.

use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use konoma_plugin::proc::client::{Config, Limits, ProcPlugin, Spawned};
use konoma_plugin::proc::frame;
use konoma_plugin::proc::proto::*;
use konoma_plugin::proc::wire::*;
use konoma_plugin::view::*;
use konoma_plugin::*;
use konoma_test_support::unique_tmp;

use crate::common::*;

fn text_of(v: &View) -> String {
    match v {
        View::Text(t) => match &t.source {
            TextSource::Owned(s) => s.clone(),
            TextSource::Lines(l) => l.iter().map(Line::plain).collect::<Vec<_>>().join("\n"),
            other => format!("{other:?}"),
        },
        other => panic!("not a text view: {other:?}"),
    }
}

fn open(p: &ProcPlugin) -> Result<Box<dyn Session>, Failure> {
    p.open(file_input("/work/a.t"), &OpenCx::bare())
}

fn view(s: &mut Box<dyn Session>, repr: &str) -> Result<View, Failure> {
    s.view(&ViewReq::new(80, 24).repr(repr), &ViewCx::bare())
}

fn act(s: &mut Box<dyn Session>, a: &str) -> Result<Outcome, Failure> {
    s.act(a, &ActCx::bare())
}

fn scratch_children(scratch: &konoma_test_support::TmpDir) -> Vec<PathBuf> {
    std::fs::read_dir(scratch.as_path())
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect()
}

#[test]
fn open_view_act_key_close() {
    let scratch = unique_tmp("kp_flow");
    let rig = Rig::new();
    let p = rig.plugin(patient_limits(), &scratch);
    assert_eq!(p.manifest().id, "test");
    assert!(p.learned_manifest().is_none());

    let mut s = open(&p).unwrap();
    assert_eq!(s.layout(), Layout::single());
    let v = s.view(&ViewReq::new(80, 24), &ViewCx::bare()).unwrap();
    assert_eq!(text_of(&v), "hello 80x24 page None");
    let v = s
        .view(&ViewReq::new(10, 3).page(2), &ViewCx::bare())
        .unwrap();
    assert_eq!(text_of(&v), "hello 10x3 page Some(2)");
    assert_eq!(
        act(&mut s, "flash").unwrap(),
        Outcome::Flash(Tr::new("flashed").ja("フラッシュ"))
    );
    assert_eq!(act(&mut s, "unknown-action").unwrap(), Outcome::Unhandled);
    let o = s
        .key(
            &Key::new(KeyCode::Char('x')).with_mods(KeyMods::CTRL),
            &ActCx::bare(),
        )
        .unwrap();
    assert_eq!(
        o,
        Outcome::Flash(Tr::new("key Char('x') KeyMods(1)")),
        "{o:?}"
    );
    assert_eq!(rig.launches.load(Ordering::SeqCst), 1);
    assert_eq!(rig.ctl.opened.load(Ordering::SeqCst), 1);

    drop(s);
    eventually("the child closes the session", || {
        rig.ctl.closed.load(Ordering::SeqCst) == 1
    });
    assert_eq!(rig.kills.load(Ordering::SeqCst), 0, "a close does not kill");
    assert!(p.is_running());
}

#[test]
fn layout_and_enabled_come_with_every_answer() {
    let scratch = unique_tmp("kp_state");
    let rig = Rig::new();
    let p = rig.plugin(patient_limits(), &scratch);
    let mut s = open(&p).unwrap();
    assert!(!s.enabled("gated"));
    assert!(s.enabled("flash"));
    assert!(
        s.enabled("not-declared"),
        "an undeclared action counts as enabled"
    );
    assert_eq!(s.layout().reprs.len(), 0);

    // An action changes both.
    assert_eq!(act(&mut s, "toggle").unwrap(), Outcome::Redraw);
    assert!(s.enabled("gated"));
    assert_eq!(s.layout().reprs.len(), 2);
    assert_eq!(s.layout().default_repr.as_deref(), Some("rendered"));

    // A view carries them too: toggle through act, then read via view after a second toggle.
    assert_eq!(act(&mut s, "toggle").unwrap(), Outcome::Redraw);
    assert!(!s.enabled("gated"));
    view(&mut s, "plain").unwrap();
    assert!(!s.enabled("gated"));
    assert_eq!(s.layout().reprs.len(), 0);
}

#[test]
fn several_sessions_share_one_child_and_keep_their_own_state() {
    let scratch = unique_tmp("kp_multi");
    let rig = Rig::new();
    let p = rig.plugin(patient_limits(), &scratch);
    let mut a = open(&p).unwrap();
    let mut b = open(&p).unwrap();
    act(&mut a, "toggle").unwrap();
    assert!(a.enabled("gated"));
    assert!(!b.enabled("gated"), "state does not leak across sessions");
    assert_eq!(
        text_of(&view(&mut b, "plain").unwrap()),
        "hello 80x24 page None"
    );
    assert_eq!(
        rig.launches.load(Ordering::SeqCst),
        1,
        "one child serves both"
    );
    drop(a);
    eventually("a closes", || rig.ctl.closed.load(Ordering::SeqCst) == 1);
    assert!(view(&mut b, "plain").is_ok(), "the other session lives on");
}

#[test]
fn calls_from_several_threads_are_multiplexed_by_request_id() {
    let scratch = unique_tmp("kp_threads");
    let rig = Rig::new();
    let p = Arc::new(rig.plugin(patient_limits(), &scratch));
    let handles: Vec<_> = (0..4u16)
        .map(|i| {
            let p = Arc::clone(&p);
            std::thread::spawn(move || {
                let mut s = open(&p).unwrap();
                for n in 0..25u16 {
                    let v = s
                        .view(&ViewReq::new(i + 1, n + 1), &ViewCx::bare())
                        .unwrap();
                    assert_eq!(text_of(&v), format!("hello {}x{} page None", i + 1, n + 1));
                }
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }
    assert_eq!(rig.launches.load(Ordering::SeqCst), 1);
}

#[test]
fn the_context_crosses() {
    let scratch = unique_tmp("kp_cx");
    let dir = unique_tmp("kp_cx_scope");
    let rig = Rig::new();
    let p = rig.plugin(patient_limits(), &scratch);
    let settings = Settings::new().with("k", Value::Str("v".into()));
    std::fs::create_dir_all(dir.as_path()).unwrap();
    let scope = ReadScope::within([dir.as_path()]);
    let ocx = OpenCx::new(&settings, "ja")
        .with_term(TermCaps::new().pixels(true).colors(ColorDepth::TrueColor));
    let mut s = p.open(file_input("/work/a.t"), &ocx).unwrap();
    let vcx = ViewCx::new(&settings, "ja")
        .with_term(TermCaps::new().pixels(true).colors(ColorDepth::Ansi256))
        .with_external(true)
        .with_read_scope(scope.clone());
    let v = s.view(&ViewReq::new(5, 5).repr("settings"), &vcx).unwrap();
    let t = text_of(&v);
    assert!(t.contains(r#"k=Some(Str("v"))"#), "{t}");
    assert!(t.contains("lang=ja"), "{t}");
    assert!(t.contains("pixels=true"), "{t}");
    assert!(t.contains("colors=Ansi256"), "{t}");
    assert!(t.contains("external=true"), "{t}");
    assert!(
        t.contains(dir.file_name().unwrap().to_str().unwrap()),
        "{t}"
    );

    // The default context: nothing set, unrestricted scope.
    let v = view(&mut s, "settings").unwrap();
    let t = text_of(&v);
    assert!(
        t.contains("k=None") && t.contains("lang=en") && t.contains("external=false"),
        "{t}"
    );
    assert!(t.contains("scope=None"), "{t}");
}

#[test]
fn inputs_reach_the_child_in_the_form_the_design_says() {
    let scratch = unique_tmp("kp_inputs");
    let rig = Rig::new();
    let p = rig.plugin(patient_limits(), &scratch);

    // A relative file becomes absolute (the child does not share the host's cwd).
    let _s = p.open(file_input("rel/b.t"), &OpenCx::bare()).unwrap();
    let seen = rig.ctl.last_input.lock().unwrap().clone().unwrap();
    let cwd = std::env::current_dir().unwrap();
    assert!(
        seen.contains(cwd.to_str().unwrap()) && seen.contains("rel/b.t"),
        "{seen}"
    );

    // Content: Bytes resources are sent (one of them through a side file), Lazy ones are not.
    #[derive(Debug)]
    struct L;
    impl LazyResource for L {
        fn natural_size(&self) -> Option<(u32, u32)> {
            None
        }
        fn render(&self, _: &ResourceReq, _: &Cancel) -> Result<ImageData, Failure> {
            unreachable!()
        }
    }
    let mut res = Resources::new();
    res.insert("small", Resource::Bytes(Arc::from(vec![9u8; 10])));
    res.insert(
        "large",
        Resource::Bytes(Arc::from(vec![5u8; (1 << 20) + 1])),
    );
    res.insert("lazy", Resource::Lazy(Arc::new(L)));
    let input = Input::Content {
        content_type: "text/markdown".into(),
        text: "# hi".into(),
        resources: res,
        origin: Origin::File(PathBuf::from("/o.md")),
    };
    let _s2 = p.open(input, &OpenCx::bare()).unwrap();
    let seen = rig.ctl.last_input.lock().unwrap().clone().unwrap();
    assert!(seen.contains("small:10:Some(9)"), "{seen}");
    assert!(
        seen.contains(&format!("large:{}:Some(5)", (1 << 20) + 1)),
        "{seen}"
    );
    assert!(
        !seen.contains("lazy"),
        "a Lazy resource is not sent: {seen}"
    );
    assert!(seen.contains("origin=File(\"/o.md\")"), "{seen}");
    // The side file was read and deleted by the child.
    for d in scratch_children(&scratch) {
        assert_eq!(count_files(&d), 0, "{d:?} still holds side files");
    }

    let _s3 = p
        .open(
            Input::Embed {
                kind: "math".into(),
                source: "x^2".into(),
                display: true,
            },
            &OpenCx::bare(),
        )
        .unwrap();
    let seen = rig.ctl.last_input.lock().unwrap().clone().unwrap();
    assert!(seen.contains("Embed") && seen.contains("x^2"), "{seen}");
}

#[test]
fn a_big_answer_comes_through_a_side_file_and_the_file_is_gone() {
    let scratch = unique_tmp("kp_bigdoc");
    let rig = Rig::new();
    let p = rig.plugin(patient_limits(), &scratch);
    let mut s = open(&p).unwrap();
    let v = view(&mut s, "bigdoc").unwrap();
    let View::Document(d) = v else { panic!() };
    assert!(
        matches!(d.resources.get("big.bin"), Some(Resource::Bytes(b)) if b.len() == 2 << 20 && b.iter().all(|&x| x == 7))
    );
    for dir in scratch_children(&scratch) {
        assert_eq!(count_files(&dir), 0);
    }
}

#[test]
fn a_failure_passes_through_unchanged() {
    let scratch = unique_tmp("kp_fail");
    let rig = Rig::new();
    let p = rig.plugin(patient_limits(), &scratch);
    let mut s = open(&p).unwrap();
    let f = view(&mut s, "fail").unwrap_err();
    assert_eq!(f.kind, FailureKind::Io);
    assert_eq!(f.message, Tr::new("nope").ja("だめ"));
    assert_eq!(f.detail.as_deref(), Some("why not"));
    assert!(p.is_running());
    assert!(
        view(&mut s, "plain").is_ok(),
        "the session goes on after a failure"
    );

    let f = p
        .open(file_input("/x/fail-open"), &OpenCx::bare())
        .err()
        .unwrap();
    assert_eq!(f.kind, FailureKind::Unsupported);
    assert_eq!(rig.launches.load(Ordering::SeqCst), 1);
    assert_eq!(rig.kills.load(Ordering::SeqCst), 0);
}

#[test]
fn a_panic_in_the_plugin_is_a_failure_and_the_child_goes_on() {
    let scratch = unique_tmp("kp_panic");
    let rig = Rig::new();
    let p = rig.plugin(patient_limits(), &scratch);
    let mut s = open(&p).unwrap();

    let f = view(&mut s, "panic").unwrap_err();
    assert_eq!(f.kind, FailureKind::External);
    assert!(
        f.detail
            .as_deref()
            .unwrap()
            .contains("panic in view: boom in view"),
        "{f:?}"
    );
    let f = act(&mut s, "panic").unwrap_err();
    assert!(
        f.detail
            .as_deref()
            .unwrap()
            .contains("panic in act: boom in act"),
        "{f:?}"
    );
    let f = p
        .open(file_input("/x/panic-open"), &OpenCx::bare())
        .err()
        .unwrap();
    assert!(
        f.detail.as_deref().unwrap().contains("panic in open"),
        "{f:?}"
    );

    assert!(view(&mut s, "plain").is_ok());
    assert!(p.is_running());
    assert_eq!(rig.launches.load(Ordering::SeqCst), 1);
    assert_eq!(rig.kills.load(Ordering::SeqCst), 0);
}

#[test]
fn a_canvas_comes_through_and_a_bad_canvas_fails_only_its_call() {
    let scratch = unique_tmp("kp_canvas");
    let rig = Rig::new();
    let p = rig.plugin(patient_limits(), &scratch);
    let mut s = open(&p).unwrap();
    let View::Canvas(c) = view(&mut s, "canvas").unwrap() else {
        panic!()
    };
    assert!(c.text_input);
    assert_eq!(c.cells[0].symbol, "h");
    assert_eq!(c.cells[3].symbol, "日");
    c.check().unwrap();

    let f = view(&mut s, "badcanvas").unwrap_err();
    assert!(
        matches!(f.kind, FailureKind::Corrupt | FailureKind::External),
        "{f:?}"
    );
    assert!(p.is_running(), "a bad canvas does not end the connection");
    assert!(view(&mut s, "plain").is_ok());
    assert_eq!(rig.kills.load(Ordering::SeqCst), 0);
}

#[test]
fn an_answer_over_the_frame_limit_is_a_failure_not_a_dead_child() {
    let scratch = unique_tmp("kp_huge");
    let rig = Rig::new();
    let p = rig.plugin(patient_limits(), &scratch);
    let mut s = open(&p).unwrap();
    let f = view(&mut s, "huge").unwrap_err();
    assert_eq!(f.kind, FailureKind::TooLarge);
    assert!(view(&mut s, "plain").is_ok());
    assert_eq!(rig.kills.load(Ordering::SeqCst), 0);
}

// ---------------------------------------------------------------------------------------------
// Cancel
// ---------------------------------------------------------------------------------------------

fn cancel_flag() -> (Arc<AtomicBool>, Cancel) {
    let flag = Arc::new(AtomicBool::new(false));
    let f = Arc::clone(&flag);
    (flag, Cancel::from_fn(move || f.load(Ordering::SeqCst)))
}

#[test]
fn a_cancel_the_plugin_honours_ends_the_call_and_not_the_child() {
    let scratch = unique_tmp("kp_cancel_ok");
    let rig = Rig::new();
    let p = rig.plugin(patient_limits(), &scratch);
    let mut s = open(&p).unwrap();
    let (flag, cancel) = cancel_flag();
    let h = std::thread::spawn(move || {
        let r = s.view(
            &ViewReq::new(5, 5).repr("wait-cancel"),
            &ViewCx::bare().with_cancel(cancel),
        );
        (s, r)
    });
    rig.ctl.started.wait();
    flag.store(true, Ordering::SeqCst);
    let (mut s, r) = h.join().unwrap();
    let f = r.unwrap_err();
    assert_eq!(f.kind, FailureKind::Cancelled);
    assert_eq!(
        f.message,
        Tr::new("stopped when asked"),
        "the plugin's own failure"
    );
    assert_eq!(rig.kills.load(Ordering::SeqCst), 0);
    assert!(p.is_running());
    assert!(view(&mut s, "plain").is_ok(), "the child still works");
}

#[test]
fn a_cancel_the_plugin_ignores_kills_the_child_after_the_grace() {
    let scratch = unique_tmp("kp_cancel_ignored");
    let rig = Rig::new();
    let mut limits = patient_limits();
    limits.cancel_grace = Duration::from_millis(200);
    let p = rig.plugin(limits, &scratch);
    let mut s = open(&p).unwrap();
    let (flag, cancel) = cancel_flag();
    let h = std::thread::spawn(move || {
        let r = s.view(
            &ViewReq::new(5, 5).repr("ignore-cancel"),
            &ViewCx::bare().with_cancel(cancel),
        );
        (s, r)
    });
    rig.ctl.started.wait();
    flag.store(true, Ordering::SeqCst);
    let (_s, r) = h.join().unwrap();
    let f = r.unwrap_err();
    assert_eq!(f.kind, FailureKind::Cancelled);
    assert_eq!(rig.kills.load(Ordering::SeqCst), 1);
    assert!(!p.is_running());
    rig.ctl.release.set(); // let the hung child thread end

    // The next open starts a new child.
    let rig2_ctl_opened = rig.ctl.opened.load(Ordering::SeqCst);
    let mut s2 = open(&p).unwrap();
    assert_eq!(rig.launches.load(Ordering::SeqCst), 2);
    assert!(view(&mut s2, "plain").is_ok());
    assert_eq!(rig.ctl.opened.load(Ordering::SeqCst), rig2_ctl_opened + 1);
}

#[test]
fn a_cancel_sent_is_one_message_per_call_and_a_cancelled_open_works_too() {
    let scratch = unique_tmp("kp_cancel_open");
    let rig = Rig::new();
    let mut limits = patient_limits();
    limits.cancel_grace = Duration::from_millis(200);
    let p = Arc::new(rig.plugin(limits, &scratch));
    let (flag, cancel) = cancel_flag();
    let p2 = Arc::clone(&p);
    let h = std::thread::spawn(move || {
        p2.open(
            file_input("/x/hang-open"),
            &OpenCx::bare().with_cancel(cancel),
        )
        .err()
    });
    rig.ctl.started.wait();
    flag.store(true, Ordering::SeqCst);
    let f = h.join().unwrap().expect("the open fails");
    assert_eq!(f.kind, FailureKind::Cancelled);
    rig.ctl.release.set();
}

// ---------------------------------------------------------------------------------------------
// Deadlines, death, restart
// ---------------------------------------------------------------------------------------------

#[test]
fn a_view_past_its_deadline_kills_the_child_fails_the_others_and_the_next_open_restarts() {
    let scratch = unique_tmp("kp_deadline_view");
    let rig = Rig::new();
    let p = rig.plugin(short_limits(), &scratch);
    let mut a = open(&p).unwrap();
    let mut b = open(&p).unwrap();
    let f = view(&mut a, "hang").unwrap_err();
    assert_eq!(f.kind, FailureKind::Timeout);
    assert_eq!(rig.kills.load(Ordering::SeqCst), 1);
    assert!(!p.is_running());

    // The other session fails with the connection's reason, without waiting.
    let started = Instant::now();
    let g = view(&mut b, "plain").unwrap_err();
    assert_eq!(g.kind, FailureKind::Timeout);
    assert!(started.elapsed() < Duration::from_millis(200));
    assert!(act(&mut b, "flash").is_err());
    assert!(b.key(&Key::new(KeyCode::Enter), &ActCx::bare()).is_err());

    rig.ctl.release.set();
    let mut c = open(&p).unwrap();
    assert_eq!(rig.launches.load(Ordering::SeqCst), 2);
    assert!(view(&mut c, "plain").is_ok());
    // Dropping the dead sessions is harmless.
    drop(a);
    drop(b);
    assert!(view(&mut c, "plain").is_ok());
}

#[test]
fn act_key_and_open_have_deadlines_too() {
    for what in ["act", "key", "open"] {
        let scratch = unique_tmp("kp_deadline_other");
        let rig = Rig::new();
        let p = rig.plugin(short_limits(), &scratch);
        let f = match what {
            "act" => act(&mut open(&p).unwrap(), "hang").unwrap_err(),
            "key" => open(&p)
                .unwrap()
                .key(&Key::new(KeyCode::Char('!')), &ActCx::bare())
                .unwrap_err(),
            _ => p
                .open(file_input("/x/hang-open"), &OpenCx::bare())
                .err()
                .unwrap(),
        };
        assert_eq!(f.kind, FailureKind::Timeout, "{what}");
        assert_eq!(rig.kills.load(Ordering::SeqCst), 1, "{what}");
        assert!(!p.is_running(), "{what}");
        rig.ctl.release.set();
    }
}

#[test]
fn the_deadline_of_open_and_view_is_not_the_one_of_act_and_key() {
    let scratch = unique_tmp("kp_deadline_split");
    let rig = Rig::new();
    let mut limits = patient_limits();
    limits.act_key = Duration::from_millis(150);
    let p = rig.plugin(limits, &scratch);
    let mut s = open(&p).unwrap();
    // A view may take longer than act_key (here: held until released after 400 ms of waiting).
    let ctl = Arc::clone(&rig.ctl);
    let releaser = std::thread::spawn(move || {
        ctl.started.wait();
        std::thread::sleep(Duration::from_millis(400));
        ctl.release.set();
    });
    assert!(
        view(&mut s, "hang").is_ok(),
        "view uses the open/view deadline"
    );
    releaser.join().unwrap();
    assert_eq!(rig.kills.load(Ordering::SeqCst), 0);
}

#[test]
fn a_timeout_carries_the_end_of_the_childs_stderr() {
    let scratch = unique_tmp("kp_stderr_timeout");
    let rig = Rig::new();
    let p = rig.plugin(short_limits(), &scratch);
    let mut s = open(&p).unwrap();
    let f = act(&mut s, "noisy-hang").unwrap_err();
    assert_eq!(f.kind, FailureKind::Timeout);
    let d = f.detail.expect("a detail");
    assert!(d.contains("MARKER-BEFORE-HANG"), "{d}");
    rig.ctl.release.set();
}

#[test]
fn a_dead_child_is_a_failure_with_at_most_4k_of_its_stderr_and_the_next_open_restarts() {
    let scratch = unique_tmp("kp_death");
    let rig = Rig::new();
    let p = rig.plugin(patient_limits(), &scratch);
    let mut s = open(&p).unwrap();
    let f = view(&mut s, "die").unwrap_err();
    assert_eq!(f.kind, FailureKind::External);
    let d = f.detail.expect("a detail");
    assert!(d.len() <= 4096, "{} bytes", d.len());
    assert!(
        d.ends_with("LAST-WORDS"),
        "the end of stderr is kept: {:?}",
        &d[d.len().saturating_sub(40)..]
    );
    assert!(!p.is_running());

    let mut s2 = open(&p).unwrap();
    assert_eq!(rig.launches.load(Ordering::SeqCst), 2);
    assert!(view(&mut s2, "plain").is_ok());
    assert!(
        s.enabled("flash"),
        "a dead session still answers the synchronous reads"
    );
}

#[test]
fn memory_over_the_limit_ends_the_child_with_too_heavy() {
    let scratch = unique_tmp("kp_rss");
    let mut rig = Rig::new();
    rig.pid = Some(std::process::id());
    let mut limits = patient_limits();
    limits.rss_limit = 1 << 20; // this test process uses far more
    limits.rss_interval = Duration::from_millis(10);
    let p = rig.plugin(limits, &scratch);
    // The watch may end the child before or after the open: either way a call that has to wait
    // for the (hung) child ends with TooHeavy.
    match open(&p) {
        Ok(mut s) => {
            let f = view(&mut s, "hang").unwrap_err();
            assert_eq!(f.kind, FailureKind::TooHeavy, "{f:?}");
        }
        Err(f) => assert_eq!(f.kind, FailureKind::TooHeavy, "{f:?}"),
    }
    eventually("the child was killed", || {
        rig.kills.load(Ordering::SeqCst) >= 1
    });
    rig.ctl.release.set();
}

#[test]
fn memory_under_the_limit_is_left_alone() {
    let scratch = unique_tmp("kp_rss_ok");
    let mut rig = Rig::new();
    rig.pid = Some(std::process::id());
    let mut limits = patient_limits();
    limits.rss_limit = 1 << 40;
    limits.rss_interval = Duration::from_millis(2);
    let p = rig.plugin(limits, &scratch);
    let mut s = open(&p).unwrap();
    for _ in 0..30 {
        view(&mut s, "plain").unwrap();
    }
    assert_eq!(rig.kills.load(Ordering::SeqCst), 0);
}

// ---------------------------------------------------------------------------------------------
// Idle, drop, cached manifest
// ---------------------------------------------------------------------------------------------

#[test]
fn an_idle_child_is_stopped_by_reap_idle_and_only_then() {
    let scratch = unique_tmp("kp_idle");
    let rig = Rig::new();
    let mut limits = patient_limits();
    limits.idle_after = Duration::from_secs(300);
    let p = rig.plugin(limits, &scratch);
    let long_after = Instant::now() + Duration::from_secs(301);

    let s = open(&p).unwrap();
    assert!(
        !p.reap_idle_at(long_after),
        "a live session keeps the child"
    );
    assert!(p.is_running());
    drop(s);
    assert!(!p.reap_idle(), "not idle long enough yet");
    assert!(p.is_running());
    assert!(!p.reap_idle_at(Instant::now() + Duration::from_secs(299)));
    assert!(p.reap_idle_at(long_after));
    assert!(!p.is_running());
    assert_eq!(rig.kills.load(Ordering::SeqCst), 1);
    assert!(!p.reap_idle_at(long_after), "nothing left to stop");

    let mut s = open(&p).unwrap();
    assert_eq!(rig.launches.load(Ordering::SeqCst), 2);
    assert!(view(&mut s, "plain").is_ok());
}

#[test]
fn dropping_the_plugin_kills_the_child_and_removes_the_handoff_directory() {
    let scratch = unique_tmp("kp_drop");
    let rig = Rig::new();
    let p = rig.plugin(patient_limits(), &scratch);
    let _s = open(&p).unwrap();
    assert_eq!(
        scratch_children(&scratch).len(),
        1,
        "one handoff directory while it runs"
    );
    drop(p);
    assert_eq!(rig.kills.load(Ordering::SeqCst), 1);
    assert!(
        scratch_children(&scratch).is_empty(),
        "the directory is removed"
    );
}

#[test]
fn stop_ends_the_child_and_a_later_open_starts_another() {
    let scratch = unique_tmp("kp_stop");
    let rig = Rig::new();
    let p = rig.plugin(patient_limits(), &scratch);
    let mut s = open(&p).unwrap();
    p.stop();
    assert!(!p.is_running());
    assert!(view(&mut s, "plain").is_err());
    let mut s2 = open(&p).unwrap();
    assert!(view(&mut s2, "plain").is_ok());
    assert_eq!(rig.launches.load(Ordering::SeqCst), 2);
}

#[test]
fn from_manifest_launches_nothing_until_the_first_open() {
    let scratch = unique_tmp("kp_cached");
    let rig = Rig::new();
    let p = ProcPlugin::from_manifest(
        test_manifest("test"),
        rig.launch(),
        config(patient_limits(), &scratch),
    );
    assert_eq!(rig.launches.load(Ordering::SeqCst), 0);
    assert!(!p.is_running());
    assert_eq!(p.manifest().id, "test");
    assert!(!p.reap_idle());
    let mut s = open(&p).unwrap();
    assert_eq!(rig.launches.load(Ordering::SeqCst), 1);
    assert!(view(&mut s, "plain").is_ok());
    assert!(
        p.learned_manifest().is_none(),
        "the program declared what was cached"
    );
}

#[test]
fn a_changed_manifest_is_learned_and_another_id_is_refused() {
    let scratch = unique_tmp("kp_learn");
    let rig = Rig::new();
    let cached = Manifest::new("test", "0.0.1").shows(ViewKind::Text);
    let p = ProcPlugin::from_manifest(cached, rig.launch(), config(patient_limits(), &scratch));
    let _s = open(&p).unwrap();
    let learned = p
        .learned_manifest()
        .expect("the declared manifest differs from the cached one");
    assert_eq!(learned.version, "1.0.0");
    assert_eq!(learned.actions.len(), test_manifest("test").actions.len());

    // Another id: the open fails and the child is ended.
    let rig2 = Rig::new();
    *rig2.id.lock().unwrap() = "impostor".into();
    let p = ProcPlugin::from_manifest(
        test_manifest("test"),
        rig2.launch(),
        config(patient_limits(), &scratch),
    );
    let f = open(&p).err().unwrap();
    assert_eq!(f.kind, FailureKind::External);
    assert!(f.detail.unwrap().contains("impostor"));
    assert_eq!(rig2.kills.load(Ordering::SeqCst), 1);
    assert!(!p.is_running());
}

#[test]
fn start_fails_cleanly_when_the_program_cannot_be_started() {
    let scratch = unique_tmp("kp_nostart");
    let launch = || -> std::io::Result<Spawned> { Err(std::io::Error::other("no such program")) };
    let f = ProcPlugin::start(launch, config(patient_limits(), &scratch))
        .err()
        .unwrap();
    assert_eq!(f.kind, FailureKind::External);
    assert!(f.detail.unwrap().contains("no such program"));

    let p = ProcPlugin::from_manifest(
        test_manifest("test"),
        launch,
        config(patient_limits(), &scratch),
    );
    assert_eq!(open(&p).err().unwrap().kind, FailureKind::External);
}

// ---------------------------------------------------------------------------------------------
// A scripted child: protocol violations
// ---------------------------------------------------------------------------------------------

type Script = Arc<dyn Fn(&mut UnixStream, &mut UnixStream, usize) + Send + Sync>;

/// A child that is a closure over the two protocol streams, given the number of this launch.
fn scripted(
    script: Script,
    launches: Arc<AtomicUsize>,
    kills: Arc<AtomicUsize>,
) -> impl Fn() -> std::io::Result<Spawned> + Send + Sync {
    move || {
        let n = launches.fetch_add(1, Ordering::SeqCst);
        let (host_w, mut child_r) = UnixStream::pair()?;
        let (mut child_w, host_r) = UnixStream::pair()?;
        let keep = vec![
            host_w.try_clone()?,
            host_r.try_clone()?,
            child_r.try_clone()?,
            child_w.try_clone()?,
        ];
        let script = Arc::clone(&script);
        std::thread::spawn(move || {
            script(&mut child_r, &mut child_w, n);
            let _ = child_r.shutdown(std::net::Shutdown::Both);
            let _ = child_w.shutdown(std::net::Shutdown::Both);
        });
        let kills = Arc::clone(&kills);
        Ok(Spawned {
            reader: Box::new(host_r),
            writer: Box::new(host_w),
            stderr: None,
            kill: Box::new(move || {
                kills.fetch_add(1, Ordering::SeqCst);
                for s in &keep {
                    let _ = s.shutdown(std::net::Shutdown::Both);
                }
            }),
            pid: None,
        })
    }
}

fn read_hello(r: &mut UnixStream) -> Hello {
    frame::recv::<_, Hello>(r).unwrap().expect("a Hello")
}

fn ack(w: &mut UnixStream) {
    let m = manifest_to_wire(&test_manifest("test")).unwrap();
    frame::send(
        w,
        &HelloReply::Ack {
            version: PROTOCOL_VERSION,
            manifest: m,
        },
    )
    .unwrap();
}

fn empty_state() -> WState {
    WState {
        layout: layout_to_wire(&Layout::single()),
        enabled: vec![],
    }
}

fn rid_of(m: &ToChild) -> u64 {
    match m {
        ToChild::Open { rid, .. }
        | ToChild::View { rid, .. }
        | ToChild::Act { rid, .. }
        | ToChild::Key { rid, .. } => *rid,
        other => panic!("not a call: {other:?}"),
    }
}

fn scripted_plugin(
    script: impl Fn(&mut UnixStream, &mut UnixStream, usize) + Send + Sync + 'static,
    scratch: &konoma_test_support::TmpDir,
    limits: Limits,
) -> (ProcPlugin, Arc<AtomicUsize>, Arc<AtomicUsize>) {
    let launches = Arc::new(AtomicUsize::new(0));
    let kills = Arc::new(AtomicUsize::new(0));
    let l = scripted(Arc::new(script), Arc::clone(&launches), Arc::clone(&kills));
    let p = ProcPlugin::from_manifest(test_manifest("test"), l, config(limits, scratch));
    (p, launches, kills)
}

#[test]
fn the_handshake_sends_the_versions_the_side_directory_and_the_language() {
    let scratch = unique_tmp("kp_hello");
    let seen: Arc<Mutex<Option<Hello>>> = Arc::default();
    let s2 = Arc::clone(&seen);
    let (p, ..) = scripted_plugin(
        move |r, w, _| {
            let h = read_hello(r);
            *s2.lock().unwrap() = Some(h);
            ack(w);
            while let Ok(Some(m)) = frame::recv::<_, ToChild>(r) {
                if let ToChild::Open { rid, .. } = m {
                    frame::send(
                        w,
                        &FromChild {
                            rid,
                            answer: Answer::Ok(Reply::Opened {
                                state: empty_state(),
                            }),
                        },
                    )
                    .unwrap();
                }
            }
        },
        &scratch,
        patient_limits(),
    );
    let mut cfg_lang = config(patient_limits(), &scratch);
    cfg_lang.lang = "ja".into();
    drop(p);
    let launches = Arc::new(AtomicUsize::new(0));
    let kills = Arc::new(AtomicUsize::new(0));
    let s3 = Arc::clone(&seen);
    let l = scripted(
        Arc::new(move |r, w, _| {
            *s3.lock().unwrap() = Some(read_hello(r));
            ack(w);
            let _ = frame::recv::<_, ToChild>(r);
        }),
        launches,
        kills,
    );
    let p = ProcPlugin::start(l, cfg_lang).unwrap();
    let h = seen.lock().unwrap().clone().unwrap();
    assert_eq!(h.versions, vec![1]);
    assert_eq!(h.lang, "ja");
    assert_eq!(h.host_version, "0.0.0-test");
    let dir = path_from_wire(h.side_dir).unwrap();
    assert!(dir.is_absolute());
    assert!(dir.starts_with(scratch.as_path()));
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
        std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
        0o700
    );
    drop(p);
}

#[test]
fn a_child_that_refuses_the_version_or_picks_another_fails_the_start() {
    let scratch = unique_tmp("kp_versions");
    // Refused.
    let (p, _, kills) = scripted_plugin(
        |r, w, _| {
            read_hello(r);
            frame::send(
                w,
                &HelloReply::Refused {
                    supported: vec![7],
                    reason: "too old".into(),
                },
            )
            .unwrap();
        },
        &scratch,
        patient_limits(),
    );
    let f = open(&p).err().unwrap();
    assert_eq!(f.kind, FailureKind::External);
    assert!(f.detail.as_deref().unwrap().contains("too old"), "{f:?}");
    assert_eq!(kills.load(Ordering::SeqCst), 1);

    // A version the host does not speak.
    let (p, ..) = scripted_plugin(
        |r, w, _| {
            read_hello(r);
            let m = manifest_to_wire(&test_manifest("test")).unwrap();
            frame::send(
                w,
                &HelloReply::Ack {
                    version: 99,
                    manifest: m,
                },
            )
            .unwrap();
        },
        &scratch,
        patient_limits(),
    );
    let f = open(&p).err().unwrap();
    assert_eq!(f.kind, FailureKind::External);
    assert!(f.detail.as_deref().unwrap().contains("99"), "{f:?}");
    let version_zero = scripted_plugin(
        |r, w, _| {
            read_hello(r);
            let m = manifest_to_wire(&test_manifest("test")).unwrap();
            frame::send(
                w,
                &HelloReply::Ack {
                    version: 0,
                    manifest: m,
                },
            )
            .unwrap();
        },
        &scratch,
        patient_limits(),
    );
    assert!(open(&version_zero.0).is_err());
}

#[test]
fn a_child_that_never_answers_the_handshake_times_out() {
    let scratch = unique_tmp("kp_hs_hang");
    let latch = Latch::new();
    let l2 = latch.clone();
    let mut limits = patient_limits();
    limits.start_timeout = Duration::from_millis(200);
    let (p, _, kills) = scripted_plugin(
        move |r, _w, _| {
            read_hello(r);
            l2.wait_for(PATIENCE);
        },
        &scratch,
        limits,
    );
    let f = open(&p).err().unwrap();
    assert_eq!(f.kind, FailureKind::Timeout);
    assert_eq!(kills.load(Ordering::SeqCst), 1);
    latch.set();
}

#[test]
fn broken_handshakes_fail_the_start() {
    let scratch = unique_tmp("kp_hs_broken");
    type Case = (
        &'static str,
        Box<dyn Fn(&mut UnixStream, &mut UnixStream, usize) + Send + Sync>,
    );
    let cases: Vec<Case> = vec![
        ("eof", Box::new(|_, _, _| {})),
        (
            "garbage",
            Box::new(|r, w, _| {
                use std::io::Write;
                read_hello(r);
                w.write_all(&[0, 0, 0, 3, 0xc1, 0xc1, 0xc1]).unwrap();
            }),
        ),
        (
            "huge prefix",
            Box::new(|r, w, _| {
                use std::io::Write;
                read_hello(r);
                w.write_all(&[0xff, 0xff, 0xff, 0xff]).unwrap();
            }),
        ),
        (
            "truncated",
            Box::new(|r, w, _| {
                use std::io::Write;
                read_hello(r);
                w.write_all(&[0, 0, 1, 0, 1, 2, 3]).unwrap();
            }),
        ),
        (
            "wrong message",
            Box::new(|r, w, _| {
                read_hello(r);
                frame::send(
                    w,
                    &FromChild {
                        rid: 1,
                        answer: Answer::Ok(Reply::Opened {
                            state: empty_state(),
                        }),
                    },
                )
                .unwrap();
            }),
        ),
    ];
    for (name, script) in cases {
        let (p, _, kills) =
            scripted_plugin(move |r, w, n| script(r, w, n), &scratch, patient_limits());
        let f = open(&p)
            .err()
            .unwrap_or_else(|| panic!("{name}: the open must fail"));
        assert_eq!(f.kind, FailureKind::External, "{name}");
        assert_eq!(kills.load(Ordering::SeqCst), 1, "{name}");
    }
}

/// After a good handshake, the child answers the first call the way the case says.
fn after_handshake(name: &str, reply: impl Fn(&mut UnixStream, ToChild) + Send + Sync + 'static) {
    let scratch = unique_tmp("kp_violation");
    let (p, launches, kills) = scripted_plugin(
        move |r, w, n| {
            read_hello(r);
            ack(w);
            while let Ok(Some(m)) = frame::recv::<_, ToChild>(r) {
                match &m {
                    // The first launch misbehaves on the first call after the open; the second is
                    // a good child.
                    ToChild::Open { rid, .. } => {
                        let rid = *rid;
                        frame::send(
                            w,
                            &FromChild {
                                rid,
                                answer: Answer::Ok(Reply::Opened {
                                    state: empty_state(),
                                }),
                            },
                        )
                        .unwrap();
                    }
                    ToChild::View { .. } if n == 0 => {
                        reply(w, m);
                        return;
                    }
                    ToChild::View { rid, .. } => {
                        let rid = *rid;
                        let view = WView::Message(vec![]);
                        frame::send(
                            w,
                            &FromChild {
                                rid,
                                answer: Answer::Ok(Reply::Viewed {
                                    view,
                                    state: empty_state(),
                                }),
                            },
                        )
                        .unwrap();
                    }
                    _ => {}
                }
            }
        },
        &scratch,
        patient_limits(),
    );
    let mut s = open(&p).unwrap_or_else(|f| panic!("{name}: {f:?}"));
    let f = s
        .view(&ViewReq::new(1, 1), &ViewCx::bare())
        .err()
        .unwrap_or_else(|| panic!("{name}: must fail"));
    assert_eq!(f.kind, FailureKind::External, "{name}: {f:?}");
    assert!(!p.is_running(), "{name}");
    assert_eq!(kills.load(Ordering::SeqCst), 1, "{name}");
    // The next open starts a good child.
    let mut s2 = open(&p).unwrap_or_else(|f| panic!("{name}: restart {f:?}"));
    assert_eq!(launches.load(Ordering::SeqCst), 2, "{name}");
    assert!(
        s2.view(&ViewReq::new(1, 1), &ViewCx::bare()).is_ok(),
        "{name}"
    );
}

#[test]
fn an_answer_nobody_asked_for_ends_the_connection() {
    after_handshake("unknown rid", |w, m| {
        let rid = rid_of(&m) + 1000;
        frame::send(
            w,
            &FromChild {
                rid,
                answer: Answer::Ok(Reply::Opened {
                    state: empty_state(),
                }),
            },
        )
        .unwrap();
    });
}

#[test]
fn an_answer_of_the_wrong_kind_ends_the_connection() {
    after_handshake("wrong kind", |w, m| {
        let rid = rid_of(&m);
        frame::send(
            w,
            &FromChild {
                rid,
                answer: Answer::Ok(Reply::Opened {
                    state: empty_state(),
                }),
            },
        )
        .unwrap();
    });
}

#[test]
fn garbage_after_the_handshake_ends_the_connection() {
    use std::io::Write;
    after_handshake("garbage", |w, _| {
        w.write_all(&[0, 0, 0, 2, 0xc1, 0xc1]).unwrap()
    });
    after_handshake("msgpack of another shape", |w, _| {
        let body = frame::encode(&("not", "a", "message")).unwrap();
        frame::write_frame(w, &body).unwrap();
    });
    after_handshake("trailing bytes", |w, m| {
        let rid = rid_of(&m);
        let mut body = frame::encode(&FromChild {
            rid,
            answer: Answer::Err(failure_to_wire(&Failure::io(Tr::new("x")))),
        })
        .unwrap();
        body.push(0xc0);
        frame::write_frame(w, &body).unwrap();
    });
}

#[test]
fn an_over_limit_or_cut_frame_ends_the_connection() {
    use std::io::Write;
    after_handshake("huge prefix", |w, _| {
        w.write_all(&[0xff, 0xff, 0xff, 0xff]).unwrap()
    });
    after_handshake("one over the limit", |w, _| {
        w.write_all(&((frame::MAX_FRAME as u32) + 1).to_be_bytes())
            .unwrap();
    });
    after_handshake("cut inside the body", |w, _| {
        w.write_all(&[0, 0, 0, 100, 1, 2, 3]).unwrap()
    });
    after_handshake("cut inside the prefix", |w, _| {
        w.write_all(&[0, 0]).unwrap()
    });
    after_handshake("eof", |_, _| {});
}

#[test]
fn a_bad_side_file_in_an_answer_ends_only_that_call() {
    // The answer names a side file that does not exist: the call fails, the child is not blamed.
    let scratch = unique_tmp("kp_badside");
    let (p, _, kills) = scripted_plugin(
        |r, w, _| {
            read_hello(r);
            ack(w);
            while let Ok(Some(m)) = frame::recv::<_, ToChild>(r) {
                match m {
                    ToChild::Open { rid, .. } => {
                        frame::send(
                            w,
                            &FromChild {
                                rid,
                                answer: Answer::Ok(Reply::Opened {
                                    state: empty_state(),
                                }),
                            },
                        )
                        .unwrap();
                    }
                    ToChild::View { rid, .. } => {
                        let view = WView::Image(WImageView {
                            data: WImage::One(WLeaf::Encoded(WBin::File {
                                name: "c-404".into(),
                                len: 3,
                            })),
                            sharp_zoom: false,
                            raster_max_px: None,
                        });
                        frame::send(
                            w,
                            &FromChild {
                                rid,
                                answer: Answer::Ok(Reply::Viewed {
                                    view,
                                    state: empty_state(),
                                }),
                            },
                        )
                        .unwrap();
                    }
                    _ => {}
                }
            }
        },
        &scratch,
        patient_limits(),
    );
    let mut s = open(&p).unwrap();
    let f = s.view(&ViewReq::new(1, 1), &ViewCx::bare()).err().unwrap();
    assert!(
        matches!(
            f.kind,
            FailureKind::Corrupt | FailureKind::External | FailureKind::Io
        ),
        "{f:?}"
    );
    assert_eq!(kills.load(Ordering::SeqCst), 0);
    assert!(p.is_running());
}

#[test]
fn a_hostile_view_that_decodes_but_breaks_the_rules_fails_the_call_not_the_host() {
    // Each case is a well-formed frame whose content breaks a rule of the wire.
    let scratch = unique_tmp("kp_hostile");
    let views: Vec<(&str, WView)> = vec![
        (
            "relative image path",
            WView::Image(WImageView {
                data: WImage::One(WLeaf::File(path_to_wire(std::path::Path::new("a.png")))),
                sharp_zoom: false,
                raster_max_px: None,
            }),
        ),
        (
            "rgba size",
            WView::Image(WImageView {
                data: WImage::One(WLeaf::Rgba {
                    width: 1 << 30,
                    height: 1 << 30,
                    pixels: WBin::Inline(vec![0; 4]),
                }),
                sharp_zoom: false,
                raster_max_px: None,
            }),
        ),
        (
            "canvas rows",
            WView::Canvas(WCanvas {
                cols: 3,
                rows: 2,
                runs: vec![],
                images: vec![],
                redraw_after_ms: None,
                text_input: false,
            }),
        ),
        (
            "table width",
            WView::Table(WTable {
                title: None,
                rainbow: false,
                addressed: false,
                ncols: 1,
                header: vec![],
                truncated: false,
                rows: vec![vec!["a".into(), "b".into()]],
            }),
        ),
    ];
    for (name, wv) in views {
        let wv2 = wv.clone();
        let (p, _, kills) = scripted_plugin(
            move |r, w, _| {
                read_hello(r);
                ack(w);
                while let Ok(Some(m)) = frame::recv::<_, ToChild>(r) {
                    match m {
                        ToChild::Open { rid, .. } => {
                            frame::send(
                                w,
                                &FromChild {
                                    rid,
                                    answer: Answer::Ok(Reply::Opened {
                                        state: empty_state(),
                                    }),
                                },
                            )
                            .unwrap();
                        }
                        ToChild::View { rid, .. } => {
                            frame::send(
                                w,
                                &FromChild {
                                    rid,
                                    answer: Answer::Ok(Reply::Viewed {
                                        view: wv2.clone(),
                                        state: empty_state(),
                                    }),
                                },
                            )
                            .unwrap();
                        }
                        _ => {}
                    }
                }
            },
            &scratch,
            patient_limits(),
        );
        let mut s = open(&p).unwrap();
        let f = s
            .view(&ViewReq::new(1, 1), &ViewCx::bare())
            .err()
            .unwrap_or_else(|| panic!("{name}: must be refused"));
        let _ = f;
        assert!(p.is_running(), "{name}: only the call fails");
        assert_eq!(kills.load(Ordering::SeqCst), 0, "{name}");
        // And the connection still answers.
        assert!(
            s.view(&ViewReq::new(1, 1), &ViewCx::bare()).is_err(),
            "{name}: the same hostile answer again"
        );
    }
}

#[test]
fn an_open_can_be_cancelled_before_it_is_even_sent_to_a_slow_reader() {
    // A child that never reads: the writer thread blocks, the call still ends at its deadline.
    let scratch = unique_tmp("kp_noread");
    let latch = Latch::new();
    let l2 = latch.clone();
    let (p, _, kills) = scripted_plugin(
        move |r, w, _| {
            read_hello(r);
            ack(w);
            l2.wait_for(PATIENCE); // never reads again
        },
        &scratch,
        short_limits(),
    );
    let f = open(&p).err().unwrap();
    assert_eq!(f.kind, FailureKind::Timeout);
    assert_eq!(kills.load(Ordering::SeqCst), 1);
    latch.set();
}

fn _unused(_: Config) {}
