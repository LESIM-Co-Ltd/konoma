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
    drive(&mut h.p, &mut h.t, vec![Key::char(key)]);
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
            let events = drive(&mut h.p, &mut h.t, with(&keys, &[Key::char('j')]));
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
            let events = drive(
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
            let events = drive(&mut h.p, &mut h.t, with(&again, &[Key::char('j')]));
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
        let events = drive(
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
    let events = drive(
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
        let events = drive(&mut h.p, &mut h.t, with(&keys, &[Key::char('j')]));
        assert_eq!(events.read, keys.len(), "{name}: a read does not ask");
        assert!(!events.ended, "{name}");
    }
    // `r` re-reads the screen: the owner is busy, but no act runs.
    for (name, _, keys) in paths().into_iter().filter(|(_, d, _)| !*d) {
        let mut h = open_project("en");
        h.shared.block_views.store(true, Ordering::SeqCst);
        drive(&mut h.p, &mut h.t, vec![Key::char('r')]);
        // A view read is held in the plugin: the owner is busy, but no act runs.
        settle(&mut h, "a refresh is running", |h| {
            h.p.runtimes.values().any(|r| r.busy && !r.acting)
        });
        let events = drive(&mut h.p, &mut h.t, with(&keys, &[Key::char('j')]));
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
        let events = drive(&mut h.p, &mut h.t, with(&keys, &[Key::char('j')]));
        assert_eq!(events.read, keys.len(), "{name}");
        assert!(!events.ended, "{name}");
    }
}
