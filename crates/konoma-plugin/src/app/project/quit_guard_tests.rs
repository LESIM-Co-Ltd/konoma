//! Quitting the standalone project app while an act that declares `writes` runs: every way out
//! asks first (the running write would be orphaned), a running read or a view refresh never asks,
//! and the act keeps running to completion when the question is declined. Order comes from a
//! latch held inside the plugin, never from the clock.
use super::global_help_tests::lines;
use super::*;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex};

/// A gate a test opens by hand; `wait` is bounded so a forgotten open fails instead of hanging.
#[derive(Default)]
struct Latch {
    opened: Mutex<bool>,
    cv: Condvar,
}
impl Latch {
    fn open(&self) {
        *self.opened.lock().unwrap_or_else(|e| e.into_inner()) = true;
        self.cv.notify_all();
    }
    fn wait(&self) {
        let guard = self.opened.lock().unwrap_or_else(|e| e.into_inner());
        let _ = self
            .cv
            .wait_timeout_while(guard, Duration::from_secs(120), |open| !*open);
    }
}

#[derive(Default)]
struct Shared {
    hold: Latch,
    /// Acts that ran to their end.
    finished: AtomicUsize,
    /// While set, a view read waits on `hold` (a screen refresh in flight).
    block_views: AtomicBool,
}

struct QuitPlugin(Arc<Shared>);
impl Plugin for QuitPlugin {
    fn manifest(&self) -> Manifest {
        let act = |id: &str, key: &str| {
            Act::new(id)
                .key(key)
                .label(crate::t!(en: "act", ja: "操作"))
                .help(crate::t!(en: "act", ja: "操作"))
                .footer(1)
                .project_context(ActContext::screen("changes", Vec::<String>::new()))
        };
        Manifest::new("quit-project", "0")
            .project(
                ProjectDecl::new()
                    .marker(DetectionMarker::new("project.marker", MarkerKind::File))
                    .screen(ProjectScreen::new("changes", "Changes"))
                    .screen(ProjectScreen::new("log", "Log")),
            )
            .action(act("write", "w").writes())
            .action(act("read", "x"))
            .action(
                Act::new("ask")
                    .key("a")
                    .label(crate::t!(en: "ask", ja: "確認"))
                    .help(crate::t!(en: "ask", ja: "確認"))
                    .project_context(ActContext::screen("changes", Vec::<String>::new()))
                    .confirmation(Confirmation::new(
                        "Sure?",
                        vec![ConfirmChoice::new('y', "yes", "Yes")],
                    )),
            )
    }
    fn open(&self, _: Input, _: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        Ok(Box::new(QuitSession(self.0.clone())))
    }
}
struct QuitSession(Arc<Shared>);
impl Session for QuitSession {
    fn view(&mut self, req: &ViewReq, cx: &ViewCx) -> Result<View, Failure> {
        if self.0.block_views.load(Ordering::SeqCst) {
            self.0.hold.wait();
        }
        ProjectSession.view(req, cx)
    }
    fn act(&mut self, _: &str, _: &ActCx) -> Result<Outcome, Failure> {
        self.0.hold.wait();
        self.0.finished.fetch_add(1, Ordering::SeqCst);
        Ok(Outcome::Unhandled)
    }
}

/// A project on its `changes` screen; dropping it always opens the latch so no thread is left
/// waiting when a test fails.
struct Held {
    _dir: konoma_test_support::TmpDir,
    p: PreparedProjects,
    t: Terminal<TestBackend>,
    shared: Arc<Shared>,
}
impl Drop for Held {
    fn drop(&mut self) {
        self.shared.hold.open();
    }
}

fn config(lang: &str) -> String {
    format!("[ui]\nlang='{lang}'\ndiff_view='source'\nwrap=false\n")
}

fn open_project(lang: &str) -> Held {
    let shared = Arc::new(Shared::default());
    let (dir, mut p, _) = setup_plugin(
        Arc::new(QuitPlugin(shared.clone())),
        false,
        true,
        &config(lang),
    );
    let mut t = Terminal::new(TestBackend::new(110, 40)).unwrap();
    opening(&mut p, &mut t, "changes");
    for runtime in p.runtimes.values_mut() {
        runtime.enabled.insert("write".into(), true);
        runtime.enabled.insert("read".into(), true);
        runtime.enabled.insert("ask".into(), true);
    }
    Held {
        _dir: dir,
        p,
        t,
        shared,
    }
}

/// The act on `key` started and held inside the plugin; `diff_open` then opens the diff screen
/// (the running act stays with the project runtime).
fn held(lang: &str, key: char, diff_open: bool) -> Held {
    let mut h = open_project(lang);
    drive_held(&mut h.p, &mut h.t, vec![Key::char(key)]);
    // The key may be queued behind the project's own first view reply; it starts the act once
    // that reply is taken.
    settle(&mut h, "the act is running", |h| {
        h.p.runtimes.values().any(|r| r.acting)
    });
    if diff_open {
        diff(&mut h.p);
    }
    h
}

fn write_running(h: &Held) -> bool {
    h.p.runtimes.values().any(|r| r.write_act_running())
}

/// `n` timeouts, then the end of the input.
struct Poll(usize);
impl EventSource for Poll {
    fn next(&mut self, _: Option<Duration>) -> Next {
        if self.0 == 0 {
            return Next::End;
        }
        self.0 -= 1;
        std::thread::sleep(Duration::from_millis(2));
        Next::Timeout
    }
}

/// Lets the held act finish and waits until the act's reply was taken.
fn finish(h: &mut Held) {
    h.shared.hold.open();
    settle(h, "the act completes", |h| {
        !h.p.runtimes.values().any(|r| r.acting)
    });
}

/// Runs the loop on timeouts only, until `done` holds (state-checked, bounded; the bound only
/// turns a hang into a failure and never orders anything).
fn settle(h: &mut Held, what: &str, done: impl Fn(&Held) -> bool) {
    for _ in 0..2000 {
        if done(h) {
            return;
        }
        run_project_with(
            &mut h.t,
            &mut h.p,
            &mut Poll(10),
            &ProjectLoopCx::default(),
            None,
        )
        .unwrap();
    }
    assert!(done(h), "never settled: {what}");
}

fn text(lang: &str) -> (&'static str, &'static str) {
    if lang == "ja" {
        (
            "終了しますか？",
            "git 操作が実行中です — 終了すると中断されます",
        )
    } else {
        (
            "Quit?",
            "a git operation is still running — quitting will interrupt it",
        )
    }
}

fn shows_question(h: &Held, lang: &str) -> bool {
    let (title, warning) = text(lang);
    let screen = lines(&h.t);
    let has = |s: &str| {
        let want = s.replace(' ', "");
        screen.iter().any(|l| l.contains(&want))
    };
    has(title) && has(warning)
}

/// Every way out of the program: (name, diff screen open first, keys).
fn paths() -> Vec<(&'static str, bool, Vec<Key>)> {
    let q = Key::char('Q');
    let c = Key::ctrl('c');
    let help = Key::char('?');
    let r = Key::char('R');
    vec![
        ("canvas Q", false, vec![q.clone()]),
        ("canvas ctrl-c", false, vec![c.clone()]),
        ("canvas help Q", false, vec![help.clone(), q.clone()]),
        ("canvas help ctrl-c", false, vec![help.clone(), c.clone()]),
        ("root screen q", false, vec![Key::char('q'), Key::char('q')]),
        ("diff Q", true, vec![q.clone()]),
        ("diff ctrl-c", true, vec![c.clone()]),
        ("diff help Q", true, vec![help.clone(), q.clone()]),
        ("preview Q", true, vec![r.clone(), q.clone()]),
        ("preview ctrl-c", true, vec![r.clone(), c.clone()]),
        ("preview help Q", true, vec![r, help, q]),
    ]
}

fn with(keys: &[Key], more: &[Key]) -> Vec<Key> {
    keys.iter().chain(more).cloned().collect()
}

#[test]
fn every_quit_path_asks_while_a_write_runs_in_en_and_ja() {
    for lang in ["en", "ja"] {
        for (name, diff_open, keys) in paths() {
            let mut h = held(lang, 'w', diff_open);
            assert!(write_running(&h), "{name}");
            let events = drive_held(&mut h.p, &mut h.t, with(&keys, &[Key::char('j')]));
            assert_eq!(
                events.read,
                keys.len() + 1,
                "{lang} {name}: the quit did not end the loop, and `j` was read"
            );
            assert!(events.ended, "{lang} {name}: the loop is still running");
            assert!(
                shows_question(&h, lang),
                "{lang} {name}: the question is on screen: {:#?}",
                lines(&h.t)
            );
            assert!(write_running(&h), "{lang} {name}: the write runs on");
        }
    }
}

#[test]
fn declining_keeps_the_write_running_and_it_completes() {
    for decline in [
        Key::char('n'),
        Key::char('N'),
        Key::new(crate::KeyCode::Esc),
    ] {
        for (name, diff_open, keys) in paths() {
            let mut h = held("en", 'w', diff_open);
            let events = drive_held(
                &mut h.p,
                &mut h.t,
                with(&keys, &[decline.clone(), Key::char('j')]),
            );
            assert_eq!(events.read, keys.len() + 2, "{name}: declining stays open");
            assert!(events.ended, "{name}");
            assert!(!shows_question(&h, "en"), "{name}: the question closed");
            assert!(write_running(&h), "{name}: the write runs on");
            assert_eq!(h.shared.finished.load(Ordering::SeqCst), 0, "{name}");
            finish(&mut h);
            assert_eq!(h.shared.finished.load(Ordering::SeqCst), 1, "{name}");
            assert!(!write_running(&h), "{name}: completed");
            // The write is over, so quitting is immediate again.
            // The first run already popped the pushed screen, so one `q` leaves the root.
            let again = if name == "root screen q" {
                vec![Key::char('q')]
            } else {
                keys.clone()
            };
            let events = drive_held(&mut h.p, &mut h.t, with(&again, &[Key::char('j')]));
            assert_eq!(
                events.read,
                again.len(),
                "{name}: quits at once after the write"
            );
            assert!(!events.ended, "{name}");
        }
    }
}

#[test]
fn yes_quits_while_the_write_runs() {
    for (name, diff_open, keys) in paths() {
        let mut h = held("en", 'w', diff_open);
        let events = drive_held(
            &mut h.p,
            &mut h.t,
            with(&keys, &[Key::char('y'), Key::char('j')]),
        );
        assert_eq!(events.read, keys.len() + 1, "{name}: `y` ends the loop");
        assert!(!events.ended, "{name}");
    }
}

#[test]
fn the_question_takes_every_other_key_and_stays_open() {
    let mut h = held("en", 'w', false);
    let events = drive_held(
        &mut h.p,
        &mut h.t,
        vec![
            Key::char('Q'),
            Key::char('j'),
            Key::char('Q'),
            Key::char('?'),
        ],
    );
    assert_eq!(events.read, 4);
    assert!(events.ended);
    assert!(shows_question(&h, "en"));
}

#[test]
fn a_running_read_or_a_view_refresh_quits_at_once() {
    for (name, diff_open, keys) in paths() {
        let h = held("en", 'x', diff_open);
        assert!(
            h.p.runtimes.values().any(|r| r.acting && !r.acting_writes),
            "{name}: a read act is running"
        );
        let mut h = h;
        let events = drive_held(&mut h.p, &mut h.t, with(&keys, &[Key::char('j')]));
        assert_eq!(events.read, keys.len(), "{name}: a read does not ask");
        assert!(!events.ended, "{name}");
    }
    // `r` re-reads the screen: the owner is busy, but no act runs.
    for (name, _, keys) in paths().into_iter().filter(|(_, d, _)| !*d) {
        let mut h = open_project("en");
        h.shared.block_views.store(true, Ordering::SeqCst);
        drive_held(&mut h.p, &mut h.t, vec![Key::char('r')]);
        // A view read is held in the plugin: the owner is busy, but no act runs.
        settle(&mut h, "a refresh is running", |h| {
            h.p.runtimes.values().any(|r| r.busy && !r.acting)
        });
        let events = drive_held(&mut h.p, &mut h.t, with(&keys, &[Key::char('j')]));
        assert_eq!(events.read, keys.len(), "{name}: a refresh does not ask");
        assert!(!events.ended, "{name}");
    }
}

#[test]
fn an_idle_project_quits_at_once() {
    for (name, diff_open, keys) in paths() {
        let mut h = open_project("en");
        if diff_open {
            diff(&mut h.p);
        }
        let events = drive_held(&mut h.p, &mut h.t, with(&keys, &[Key::char('j')]));
        assert_eq!(events.read, keys.len(), "{name}");
        assert!(!events.ended, "{name}");
    }
}

/// The project vanishes while a write runs: the loop does not end (nor ask) until the write has
/// finished, then ends with the no-project error. The write is released by the event source after
/// the loop has already seen the answer, so the order comes from the latch, not the clock.
#[test]
fn a_vanished_project_ends_the_loop_only_after_the_running_write_finished() {
    let mut h = held("en", 'w', false);
    assert!(write_running(&h));
    let shared = h.shared.clone();
    let detection = h.p.detection.clone();
    struct Owned {
        shared: Arc<Shared>,
        detection: crate::host::project::detection::ProjectDetection,
        root: PathBuf,
        step: usize,
        steps_before_open: Arc<AtomicUsize>,
    }
    impl EventSource for Owned {
        fn next(&mut self, _: Option<Duration>) -> Next {
            let step = self.step;
            self.step += 1;
            match step {
                0 => {
                    std::fs::remove_dir_all(&self.root).unwrap();
                    self.detection
                        .inject_event(Ok(notify::Event::new(notify::EventKind::Remove(
                            notify::event::RemoveKind::Any,
                        ))
                        .add_path(self.root.join("project.marker"))));
                    Next::Timeout
                }
                1..=200 => {
                    std::thread::yield_now();
                    Next::Timeout
                }
                201 => {
                    self.steps_before_open.store(self.step, Ordering::SeqCst);
                    self.shared.hold.open();
                    Next::Timeout
                }
                // The write finishes on its own thread once the latch is open. The bound only
                // turns a hang into a failure (2 ms per step, 2000 steps); it orders nothing.
                202..=2201 => {
                    std::thread::sleep(Duration::from_millis(2));
                    Next::Timeout
                }
                _ => Next::End,
            }
        }
    }
    let opened_at = Arc::new(AtomicUsize::new(0));
    let mut events = Owned {
        shared,
        detection,
        // The canonical path, as the detection worker watches it.
        root: h._dir.canonicalize().unwrap(),
        step: 0,
        steps_before_open: opened_at.clone(),
    };
    let result = run_project_with(
        &mut h.t,
        &mut h.p,
        &mut events,
        &ProjectLoopCx::default(),
        None,
    );
    let error = result.expect_err("the project is gone, so the loop ends");
    assert!(matches!(error, AppError::NoProject(_)), "{error:?}");
    assert_eq!(error.exit_code(), 2);
    assert_eq!(
        opened_at.load(Ordering::SeqCst),
        202,
        "the loop kept running while the write was held"
    );
    assert_eq!(
        h.shared.finished.load(Ordering::SeqCst),
        1,
        "the loop ended only after the write finished"
    );
    std::fs::create_dir_all(&h._dir).unwrap();
}

/// A confirmation of the shown project is open while its write runs. The UI cannot open an act
/// while one runs, so the dialog is opened first (an idle run of the loop keeps it in `prepared`),
/// then the write is started in a second run, with the dialog set aside, and the dialog is put
/// back: the state the loop must handle. Returns the held project after a third run in which the
/// event source presses `j`, optionally makes the project vanish, and releases the write.
fn dialog_scenario(vanish: bool) -> (Result<(), AppError>, Held) {
    let mut h = open_project("en");
    drive_held(&mut h.p, &mut h.t, vec![Key::char('a')]);
    // Every run of the loop starts with a fresh viewport, so it re-reads the screen first; a key
    // read before that reply is taken waits in the screen's queue and acts once it is.
    settle(&mut h, "the confirmation opened", |h| h.p.dialog.is_some());
    let dialog = h.p.dialog.take();
    assert!(dialog.is_some(), "the confirmation opened");
    drive_held(&mut h.p, &mut h.t, vec![Key::char('w')]);
    settle(&mut h, "the write is running", write_running);
    h.p.dialog = dialog;
    struct Owned {
        shared: Arc<Shared>,
        detection: crate::host::project::detection::ProjectDetection,
        root: PathBuf,
        vanish: bool,
        step: usize,
    }
    impl EventSource for Owned {
        fn next(&mut self, _: Option<Duration>) -> Next {
            let step = self.step;
            self.step += 1;
            match step {
                0 => {
                    if self.vanish {
                        std::fs::remove_dir_all(&self.root).unwrap();
                        self.detection.inject_event(Ok(notify::Event::new(
                            notify::EventKind::Remove(notify::event::RemoveKind::Any),
                        )
                        .add_path(self.root.join("project.marker"))));
                    }
                    Next::Timeout
                }
                1..=200 => {
                    std::thread::yield_now();
                    Next::Timeout
                }
                201 => Next::Event(AppEvent::Key(Key::char('j'))),
                202..=250 => {
                    std::thread::yield_now();
                    Next::Timeout
                }
                251 => {
                    self.shared.hold.open();
                    Next::Timeout
                }
                // The bound only turns a hang into a failure; it orders nothing.
                252..=2251 => {
                    if !self.vanish && self.shared.finished.load(Ordering::SeqCst) == 1 {
                        return Next::End;
                    }
                    std::thread::sleep(Duration::from_millis(2));
                    Next::Timeout
                }
                _ => Next::End,
            }
        }
    }
    let mut events = Owned {
        shared: h.shared.clone(),
        detection: h.p.detection.clone(),
        root: h._dir.canonicalize().unwrap(),
        vanish,
        step: 0,
    };
    let result = run_project_with(
        &mut h.t,
        &mut h.p,
        &mut events,
        &ProjectLoopCx::default(),
        None,
    );
    (result, h)
}

#[test]
fn a_confirmation_of_a_vanished_project_is_closed_while_its_write_runs() {
    let (result, h) = dialog_scenario(false);
    result.expect("nothing vanished");
    assert!(
        h.p.dialog.is_some(),
        "control: the confirmation stays open and keeps the keys"
    );
    let (result, h) = dialog_scenario(true);
    let error = result.expect_err("the project is gone, so the loop ends");
    assert!(matches!(error, AppError::NoProject(_)), "{error:?}");
    assert!(
        h.p.dialog.is_none(),
        "the confirmation of the gone project was closed"
    );
    assert_eq!(
        h.shared.finished.load(Ordering::SeqCst),
        1,
        "the loop ended only after the write finished"
    );
    std::fs::create_dir_all(&h._dir).unwrap();
}

/// A key the project layer ignores (an Alt chord) does nothing at all while the screen reads: a
/// `x` waiting behind the read is neither displaced nor cancelled, and runs once the rows land.
/// Plain `q` during the read is unchanged: it goes back a screen.
#[test]
fn an_ignored_key_during_a_load_leaves_the_waiting_key_alone() {
    let alt = |c: char| Key::char(c).with_mods(crate::KeyMods::ALT);
    for ignored in ['j', 'h', 'q'] {
        let mut h = open_project("en");
        h.shared.block_views.store(true, Ordering::SeqCst);
        drive_held(&mut h.p, &mut h.t, vec![Key::char('r')]);
        settle(&mut h, "a refresh is running", |h| {
            h.p.runtimes.values().any(|r| r.busy && !r.acting)
        });
        // The refresh `r` waits in line too (the first read is still out); `x` joins it.
        drive_held(&mut h.p, &mut h.t, vec![Key::char('x')]);
        let waiting = h.p.screen.as_ref().unwrap().frame().unwrap().pending.len();
        assert!(waiting >= 1, "x did not wait in line");
        let events = drive_held(&mut h.p, &mut h.t, vec![alt(ignored)]);
        assert!(events.ended, "Alt-{ignored} ended the app");
        let frame = h.p.screen.as_ref().expect("the project is still open");
        assert_eq!(
            frame.frame().unwrap().pending.len(),
            waiting,
            "Alt-{ignored} changed the waiting keys"
        );
        h.shared.block_views.store(false, Ordering::SeqCst);
        h.shared.hold.open();
        settle(&mut h, "the waiting x ran", |h| {
            h.shared.finished.load(Ordering::SeqCst) == 1
        });
    }
    let mut h = open_project("en");
    h.shared.block_views.store(true, Ordering::SeqCst);
    drive_held(&mut h.p, &mut h.t, vec![Key::char('r')]);
    settle(&mut h, "a refresh is running", |h| {
        h.p.runtimes.values().any(|r| r.busy && !r.acting)
    });
    let depth = h.p.screen.as_ref().unwrap().stack.len();
    drive_held(&mut h.p, &mut h.t, vec![Key::char('q')]);
    assert_eq!(
        h.p.screen.as_ref().unwrap().stack.len(),
        depth - 1,
        "plain q during the read did not go back"
    );
}
