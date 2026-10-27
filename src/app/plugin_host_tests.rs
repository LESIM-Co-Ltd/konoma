//! The host's plugin Session mechanism (`app/plugin_host.rs`): the first-frame budget, stale
//! results, a panicking plugin, the settings a plugin receives, and the actions and later views of
//! an open session. Plugins here are stand-ins written for the test, so each property is observed
//! on its own and does not depend on how fast a real parse is.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use konoma_plugin::view::{Grid, Layout, TableView, View, ViewReq};
use konoma_plugin::{
    t, ActCx, Failure, Input, Manifest, OpenCx, Outcome, Plugin, Session, Settings, Value, ViewCx,
};

use super::plugin_host::{Handoff, LongFirstFrame, PluginResult, FIRST_FRAME_BUDGET};
use super::*;
use crate::plugins::test_plugins::Latch;
use crate::test_support::unique_tmp;

// ---- stand-in plugins -----------------------------------------------------------------------

/// A grid of `n` rows and one column (`r0`, `r1`...).
struct Rows(usize);

impl Grid for Rows {
    fn nrows(&self) -> usize {
        self.0
    }
    fn ncols(&self) -> usize {
        1
    }
    fn header(&self, _col: usize) -> Option<String> {
        Some("h".into())
    }
    fn cell(&self, row: usize, _col: usize) -> String {
        format!("r{row}")
    }
}

/// What the stand-in does at each step.
#[derive(Clone, Default)]
struct Script {
    /// Sleep this long in `open` (checking the cancel flag every millisecond).
    open_delay: Duration,
    /// Hold `open` until the test opens this gate (still checking the cancel flag), so "the answer
    /// has not come yet" is a state the test makes, not a race against `open_delay`.
    gate: Option<Arc<Latch>>,
    panic_in_open: bool,
    panic_in_view: bool,
    /// Rows of the table it returns.
    rows: usize,
}

/// Everything the stand-in saw, shared with the test.
#[derive(Default)]
struct Seen {
    cancelled: AtomicBool,
    opens: AtomicUsize,
    views: AtomicUsize,
    acts: Mutex<Vec<String>>,
    settings: Mutex<Option<Settings>>,
    lang: Mutex<String>,
    open_thread: Mutex<Option<std::thread::ThreadId>>,
}

struct Stand {
    script: Script,
    seen: Arc<Seen>,
}

struct StandSession {
    script: Script,
    seen: Arc<Seen>,
    page: usize,
}

impl Plugin for Stand {
    fn manifest(&self) -> Manifest {
        Manifest::new("stand", "0")
    }

    fn open(&self, _input: Input, cx: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        self.seen.opens.fetch_add(1, Ordering::SeqCst);
        *self.seen.settings.lock().unwrap() = Some(cx.settings.clone());
        *self.seen.lang.lock().unwrap() = cx.lang.to_string();
        *self.seen.open_thread.lock().unwrap() = Some(std::thread::current().id());
        if let Some(gate) = &self.script.gate {
            while !gate.is_open() {
                if cx.cancel.is_cancelled() {
                    self.seen.cancelled.store(true, Ordering::SeqCst);
                    return Err(Failure::cancelled(t!(en: "cancelled")));
                }
                std::thread::sleep(Duration::from_millis(1));
            }
        }
        let until = Instant::now() + self.script.open_delay;
        while Instant::now() < until {
            if cx.cancel.is_cancelled() {
                self.seen.cancelled.store(true, Ordering::SeqCst);
                return Err(Failure::cancelled(t!(en: "cancelled")));
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        if self.script.panic_in_open {
            panic!("the stand-in plugin panics in open");
        }
        Ok(Box::new(StandSession {
            script: self.script.clone(),
            seen: Arc::clone(&self.seen),
            page: 0,
        }))
    }
}

impl Session for StandSession {
    fn layout(&self) -> Layout {
        Layout {
            default_repr: Some("stand-layout".into()),
            ..Layout::single()
        }
    }

    fn view(&mut self, req: &ViewReq, _cx: &ViewCx) -> Result<View, Failure> {
        self.seen.views.fetch_add(1, Ordering::SeqCst);
        if self.script.panic_in_view {
            panic!("the stand-in plugin panics in view");
        }
        self.page = req.page.unwrap_or(0);
        Ok(View::Table(TableView {
            grid: Arc::new(Rows(self.script.rows + self.page)),
            title: None,
            rainbow: true,
            addressed: false,
        }))
    }

    fn act(&mut self, action: &str, _cx: &ActCx) -> Result<Outcome, Failure> {
        self.seen.acts.lock().unwrap().push(action.to_string());
        match action {
            "flash" => Ok(Outcome::Flash(t!(en: "flashed", ja: "表示した"))),
            "reload" => Ok(Outcome::Reload),
            "redraw" => Ok(Outcome::Redraw),
            "fail" => Err(Failure::external(t!(en: "the action failed"))),
            _ => Ok(Outcome::Unhandled),
        }
    }
}

fn stand(script: Script) -> (Arc<dyn Plugin>, Arc<Seen>) {
    let seen = Arc::new(Seen::default());
    (
        Arc::new(Stand {
            script,
            seen: Arc::clone(&seen),
        }),
        seen,
    )
}

/// An app previewing `path` as the stand-in plugin `stand`, not opened yet.
fn app_on(dir: &Path, path: &Path) -> App {
    let mut app = App::new(dir.to_path_buf(), Config::default()).unwrap();
    app.tab.preview_kind = Some(PreviewKind::Plugin {
        path: path.to_path_buf(),
        id: "stand".into(),
        hint: None,
    });
    app.tab.preview_path = Some(path.to_path_buf());
    app.tab.mode = Mode::Preview;
    app
}

fn open_stand(app: &mut App, plugin: Arc<dyn Plugin>, path: &Path) {
    let input = Input::File {
        path: path.to_path_buf(),
        hint: None,
    };
    app.start_plugin_open("stand".into(), plugin, input, false);
}

/// Receive one result and apply it, the way the run loop does.
#[track_caller]
fn land(app: &mut App, rx: &Receiver<MediaResult>) -> bool {
    let res = rx
        .recv_timeout(Duration::from_secs(10))
        .expect("the worker always answers");
    app.apply_media(res)
}

fn rows_of(app: &App) -> Option<usize> {
    app.grid().map(|g| g.nrows())
}

fn sandbox(name: &str) -> (crate::test_support::TmpDir, PathBuf) {
    let dir = unique_tmp(name);
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("f.stand");
    std::fs::write(&file, "x").unwrap();
    (dir, file)
}

// ---- the first frame --------------------------------------------------------------------------

/// A plugin that answers within the budget shows on the very first frame, exactly as a synchronous
/// load did: nothing is left to land afterwards.
#[test]
fn a_quick_open_is_on_screen_in_the_first_frame() {
    let (dir, file) = sandbox("konoma_plugin_host_quick");
    // The answer is immediate; the budget is long so that a loaded machine cannot make it late.
    let _budget = LongFirstFrame::of(Duration::from_secs(5));
    let mut app = app_on(&dir, &file);
    let (tx, rx) = channel();
    app.attach_media_loader(tx);
    let (plugin, _) = stand(Script {
        rows: 3,
        ..Script::default()
    });
    open_stand(&mut app, plugin, &file);
    assert!(!app.is_plugin_loading());
    assert_eq!(rows_of(&app), Some(3));
    assert!(!app.is_media_loading(), "the loading mark is clear");
    assert!(
        rx.recv_timeout(Duration::from_millis(100)).is_err(),
        "the result was taken by the first frame, nothing is sent again"
    );
}

/// A plugin slower than the budget leaves a "Loading..." screen and the UI is free again at once;
/// the result is applied when it lands.
#[test]
fn a_slow_open_does_not_block_the_ui_and_lands_later() {
    let (dir, file) = sandbox("konoma_plugin_host_slow");
    let mut app = app_on(&dir, &file);
    let gate = Arc::new(Latch::default());
    let (tx, rx) = channel();
    app.attach_media_loader(tx);
    let (plugin, seen) = stand(Script {
        rows: 7,
        gate: Some(Arc::clone(&gate)),
        ..Script::default()
    });
    let t0 = Instant::now();
    open_stand(&mut app, plugin, &file);
    let spent = t0.elapsed();
    assert!(
        spent < FIRST_FRAME_BUDGET + Duration::from_millis(250),
        "the UI thread waited {spent:?} for a plugin that needs 400ms"
    );
    assert!(spent < Duration::from_millis(350), "{spent:?}");
    assert!(app.is_plugin_loading(), "Loading... until the result lands");
    assert!(app.is_media_loading(), "the run loop keeps ticking");
    assert!(!app.is_table_preview());
    assert_eq!(app.surface(), crate::keymap::Surface::PreviewText);
    assert_ne!(
        *seen.open_thread.lock().unwrap(),
        Some(std::thread::current().id()),
        "open ran on a worker"
    );
    gate.open();
    assert!(land(&mut app, &rx), "the result is applied");
    assert!(!app.is_plugin_loading());
    assert!(!app.is_media_loading());
    assert!(app.is_table_preview());
    assert_eq!(rows_of(&app), Some(7));
    assert_eq!(
        app.plugin_layout_for_test()
            .and_then(|l| l.default_repr.as_deref()),
        Some("stand-layout"),
        "the layout the session reported is kept"
    );
}

/// The real CSV plugin on a big file: opening it never blocks the UI thread for the parse.
#[test]
fn opening_a_big_csv_does_not_block_enter_preview() {
    let dir = unique_tmp("konoma_plugin_host_bigcsv");
    std::fs::create_dir_all(&dir).unwrap();
    let csv = dir.join("big.csv");
    // 200,000 rows of 30 cells: past the 100,000-row cap, several million cells to read.
    {
        use std::io::Write as _;
        let mut f = std::io::BufWriter::new(std::fs::File::create(&csv).unwrap());
        let header: Vec<String> = (0..30).map(|c| format!("col{c}")).collect();
        writeln!(f, "{}", header.join(",")).unwrap();
        for r in 0..200_000 {
            let row: Vec<String> = (0..30).map(|c| format!("v{r}x{c}")).collect();
            writeln!(f, "{}", row.join(",")).unwrap();
        }
    }
    // What the parse costs on this machine, taken on its own.
    let t0 = Instant::now();
    let parsed = crate::preview::table::parse(&csv, b',').unwrap();
    let parse_time = t0.elapsed();
    assert_eq!(parsed.nrows(), crate::preview::table::MAX_ROWS);
    drop(parsed);

    let mut app = App::new(dir.to_path_buf(), Config::default()).unwrap();
    let (tx, rx) = channel();
    app.attach_media_loader(tx);
    let t0 = Instant::now();
    app.enter_preview(&csv);
    let spent = t0.elapsed();
    assert!(
        spent < FIRST_FRAME_BUDGET + Duration::from_millis(250),
        "enter_preview took {spent:?} (the parse alone takes {parse_time:?})"
    );
    if parse_time > Duration::from_millis(400) {
        assert!(
            spent < parse_time / 2,
            "enter_preview {spent:?} vs parse {parse_time:?}: it waited for the parse"
        );
    }
    // Whether it fit the first frame or not, the table ends up on screen with the cap noted.
    if app.is_plugin_loading() {
        assert!(!app.is_table_preview());
        assert!(land(&mut app, &rx));
    }
    assert!(app.is_table_preview());
    let g = app.grid().unwrap();
    assert_eq!(
        (g.nrows(), g.ncols()),
        (crate::preview::table::MAX_ROWS, 30)
    );
    assert!(g.truncated());
}

// ---- cancelling and stale results -------------------------------------------------------------

/// A result that comes after the preview moved on (the generation advanced) is dropped, and the
/// plugin that was still running is told to stop.
#[test]
fn a_result_from_an_older_generation_is_dropped() {
    let (dir, file) = sandbox("konoma_plugin_host_stale");
    let mut app = app_on(&dir, &file);
    let gate = Arc::new(Latch::default());
    let (tx, rx) = channel();
    app.attach_media_loader(tx);
    let (plugin, seen) = stand(Script {
        rows: 5,
        gate: Some(Arc::clone(&gate)),
        ..Script::default()
    });
    open_stand(&mut app, plugin, &file);
    assert!(app.is_plugin_loading());
    // The preview moves on: the same thing `enter_preview` of another file does first.
    app.clear_image();
    assert!(!land(&mut app, &rx), "the stale result is not applied");
    assert!(
        seen.cancelled.load(Ordering::SeqCst),
        "the running plugin saw its Cancel turn true"
    );
    assert_eq!(rows_of(&app), None, "nothing was installed");
    assert!(!app.is_media_loading());
}

/// A result of an earlier open of the same preview is stale once a newer open began (a reload).
#[test]
fn only_the_newest_open_is_applied() {
    let (dir, file) = sandbox("konoma_plugin_host_newest");
    let mut app = app_on(&dir, &file);
    let gate = Arc::new(Latch::default());
    let (tx, rx) = channel();
    app.attach_media_loader(tx);
    let (slow, _) = stand(Script {
        rows: 1,
        gate: Some(Arc::clone(&gate)),
        ..Script::default()
    });
    let (fast, _) = stand(Script {
        rows: 9,
        ..Script::default()
    });
    open_stand(&mut app, slow, &file);
    open_stand(&mut app, fast, &file);
    // The fast one was taken by its first frame; the slow one's result arrives and is dropped.
    assert_eq!(rows_of(&app), Some(9));
    gate.open();
    assert!(!land(&mut app, &rx));
    assert_eq!(rows_of(&app), Some(9), "the newest stays");
}

/// A result for a preview that is no longer this plugin's (another kind took over the tab) is
/// dropped even though no new media job advanced the generation.
#[test]
fn a_result_after_the_kind_changed_is_dropped() {
    let (dir, file) = sandbox("konoma_plugin_host_kind");
    let mut app = app_on(&dir, &file);
    let gate = Arc::new(Latch::default());
    let (tx, rx) = channel();
    app.attach_media_loader(tx);
    let (plugin, _) = stand(Script {
        rows: 2,
        gate: Some(Arc::clone(&gate)),
        ..Script::default()
    });
    open_stand(&mut app, plugin, &file);
    app.set_preview_kind(Some(PreviewKind::Text(file.clone())));
    assert!(app.plugin.is_none(), "the show is released with the kind");
    gate.open();
    assert!(!land(&mut app, &rx));
    assert!(app.plugin.is_none());
    assert!(
        !app.is_media_loading(),
        "the loading mark ends with the result"
    );
}

// ---- a panicking plugin -----------------------------------------------------------------------

/// A panic in `open` on the worker is caught and reported: the loading mark cannot stick.
#[test]
fn a_panic_in_open_becomes_a_failure_not_a_stuck_loading() {
    let (dir, file) = sandbox("konoma_plugin_host_panic_open");
    let mut app = app_on(&dir, &file);
    let gate = Arc::new(Latch::default());
    let (tx, rx) = channel();
    app.attach_media_loader(tx);
    let (plugin, _) = stand(Script {
        panic_in_open: true,
        gate: Some(Arc::clone(&gate)),
        ..Script::default()
    });
    open_stand(&mut app, plugin, &file);
    assert!(app.is_plugin_loading());
    gate.open();
    assert!(land(&mut app, &rx), "the failure is applied");
    assert!(!app.is_plugin_loading(), "Loading... is gone");
    assert!(!app.is_media_loading());
    assert!(app.plugin_failure().is_some());
    assert!(!app.is_table_preview());
}

/// The same inside the first-frame budget, and a panic in `view` too.
#[test]
fn a_panic_is_a_failure_in_the_first_frame_and_without_a_worker_channel() {
    let _budget = LongFirstFrame::of(Duration::from_secs(5));
    for (panic_in_open, panic_in_view) in [(true, false), (false, true)] {
        // With a channel (first frame).
        let (dir, file) = sandbox("konoma_plugin_host_panic_quick");
        let mut app = app_on(&dir, &file);
        let (tx, _rx) = channel();
        app.attach_media_loader(tx);
        let (plugin, _) = stand(Script {
            panic_in_open,
            panic_in_view,
            ..Script::default()
        });
        open_stand(&mut app, plugin, &file);
        assert!(!app.is_plugin_loading(), "open={panic_in_open}");
        assert!(!app.is_media_loading());
        assert!(app.plugin_failure().is_some());
        // Without one (the tests' own path).
        let mut app = app_on(&dir, &file);
        let (plugin, _) = stand(Script {
            panic_in_open,
            panic_in_view,
            ..Script::default()
        });
        open_stand(&mut app, plugin, &file);
        assert!(!app.is_plugin_loading());
        assert!(!app.is_media_loading());
        assert!(app.plugin_failure().is_some());
    }
}

/// A plugin's own `Failure` is shown too (the open fails, nothing panics).
#[test]
fn a_failed_open_leaves_the_failure_to_show() {
    let (dir, file) = sandbox("konoma_plugin_host_failed");
    let mut app = app_on(&dir, &file);
    app.start_plugin_open(
        "stand".into(),
        Arc::new(Refuses),
        Input::File {
            path: file.clone(),
            hint: None,
        },
        false,
    );
    let f = app.plugin_failure().expect("the failure is kept");
    assert_eq!(f.kind, konoma_plugin::FailureKind::Corrupt);
    assert!(!app.is_table_preview());
    assert!(!app.is_media_loading());
}

struct Refuses;

impl Plugin for Refuses {
    fn manifest(&self) -> Manifest {
        Manifest::new("stand", "0")
    }
    fn open(&self, _input: Input, _cx: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        Err(Failure::corrupt(t!(en: "broken")))
    }
}

// ---- what the plugin receives -----------------------------------------------------------------

fn cfg_from(text: &str) -> Config {
    toml::from_str::<Config>(text).unwrap()
}

fn csv_app(cfg: Config) -> (App, crate::test_support::TmpDir) {
    let dir = unique_tmp("konoma_plugin_host_csv");
    std::fs::create_dir_all(&dir).unwrap();
    let csv = dir.join("t.csv");
    std::fs::write(&csv, "a,b\n1,2\n").unwrap();
    let mut app = App::new(dir.to_path_buf(), cfg).unwrap();
    app.enter_preview(&csv);
    (app, dir)
}

/// The rainbow switch reaches the view through `OpenCx::settings` from either name.
#[test]
fn rainbow_follows_the_new_and_the_old_setting() {
    let cases = [
        ("", true),
        ("[plugins.csv]\nrainbow = false\n", false),
        ("[ui]\ncsv_rainbow = false\n", false),
        ("[plugins.csv]\nrainbow = true\n", true),
        // Both set: the new name wins.
        (
            "[ui]\ncsv_rainbow = false\n[plugins.csv]\nrainbow = true\n",
            true,
        ),
        (
            "[ui]\ncsv_rainbow = true\n[plugins.csv]\nrainbow = false\n",
            false,
        ),
    ];
    for (text, want) in cases {
        let (app, _dir) = csv_app(cfg_from(text));
        assert!(app.is_table_preview(), "{text:?}");
        assert_eq!(app.table_rainbow(), want, "{text:?}");
        let view_says = app.plugin_table().map(|t| t.rainbow);
        assert_eq!(view_says, Some(want), "the view carries it: {text:?}");
    }
}

/// `[plugins.csv] enabled = false` hands the file to plain text; the archive plugin the same.
#[test]
fn a_disabled_table_plugin_shows_the_file_as_text() {
    let dir = unique_tmp("konoma_plugin_host_disabled");
    std::fs::create_dir_all(&dir).unwrap();
    let csv = dir.join("t.csv");
    std::fs::write(&csv, "a,b\n1,2\n").unwrap();
    let mut app = App::new(
        dir.to_path_buf(),
        cfg_from("[plugins.csv]\nenabled = false\n"),
    )
    .unwrap();
    app.enter_preview(&csv);
    assert!(
        matches!(app.tab.preview_kind, Some(PreviewKind::Text(_))),
        "{:?}",
        app.tab.preview_kind
    );
    assert!(!app.is_table_preview());
    assert!(app.plugin.is_none());
    // With it on (the default) the same file is a table.
    let mut on = App::new(dir.to_path_buf(), Config::default()).unwrap();
    on.enter_preview(&csv);
    assert!(on.is_table_preview());
    // Switching the archive plugin off: a zip is no longer listed.
    let zip = dir.join("a.zip");
    std::fs::write(&zip, b"PK\x03\x04").unwrap();
    let mut app = App::new(
        dir.to_path_buf(),
        cfg_from("[plugins.archive]\nenabled = false\n"),
    )
    .unwrap();
    app.enter_preview(&zip);
    assert!(
        !app.tab
            .preview_kind
            .as_ref()
            .is_some_and(|k| k.is_plugin("archive")),
        "{:?}",
        app.tab.preview_kind
    );
}

/// What a plugin is told: its `[plugins.<id>]` table, the user's language, and whether the terminal
/// draws pixels.
#[test]
fn the_plugin_receives_its_table_and_the_language() {
    let (dir, file) = sandbox("konoma_plugin_host_cx");
    for (lang_cfg, want_lang) in [("en", "en"), ("jp", "ja")] {
        let mut cfg = cfg_from("[plugins.stand]\nlevel = 3\nname = \"x\"\n");
        cfg.ui.lang = lang_cfg.into();
        let mut app = App::new(dir.to_path_buf(), cfg).unwrap();
        app.tab.preview_kind = Some(PreviewKind::Plugin {
            path: file.clone(),
            id: "stand".into(),
            hint: None,
        });
        app.tab.preview_path = Some(file.clone());
        let (plugin, seen) = stand(Script::default());
        open_stand(&mut app, plugin, &file);
        assert_eq!(*seen.lang.lock().unwrap(), want_lang);
        let settings = seen.settings.lock().unwrap().clone().unwrap();
        assert_eq!(settings.int("level", 0), 3);
        assert_eq!(settings.str("name", ""), "x");
    }
}

/// The old `[ui]` names are filled in under the new names only where the new one is absent.
#[test]
fn effective_settings_overlay_the_old_names() {
    let cfg = cfg_from("[ui]\ncsv_rainbow = false\n");
    let s = crate::plugins::effective_settings(&cfg, "csv");
    assert_eq!(s.get("rainbow"), Some(&Value::Bool(false)));
    let cfg = cfg_from("[ui]\ncsv_rainbow = false\n[plugins.csv]\nrainbow = true\n");
    let s = crate::plugins::effective_settings(&cfg, "csv");
    assert_eq!(s.get("rainbow"), Some(&Value::Bool(true)));
    // A plugin with no old name gets exactly its table.
    let cfg = cfg_from("[ui]\ncsv_rainbow = false\n");
    assert!(crate::plugins::effective_settings(&cfg, "archive").is_empty());
}

// ---- later views and actions ------------------------------------------------------------------

/// A later `view` (another page) goes to the worker with the session and comes back with it.
#[test]
fn a_later_view_reuses_the_open_session() {
    let (dir, file) = sandbox("konoma_plugin_host_view");
    for with_channel in [false, true] {
        let mut app = app_on(&dir, &file);
        let rx = if with_channel {
            let (tx, rx) = channel();
            app.attach_media_loader(tx);
            Some(rx)
        } else {
            None
        };
        let (plugin, seen) = stand(Script {
            rows: 2,
            ..Script::default()
        });
        open_stand(&mut app, plugin, &file);
        assert_eq!(rows_of(&app), Some(2));
        let mut req = ViewReq::new(80, 24);
        req.page = Some(3);
        assert!(app.plugin_request_view(req));
        if let Some(rx) = &rx {
            // The session is on the worker: a second ask finds none, and the old view stays on
            // screen until the new one lands (or has landed within the grace).
            if app.plugin.as_ref().is_some_and(|p| p.session.is_none()) {
                assert!(!app.plugin_request_view(ViewReq::new(80, 24)));
                assert_eq!(rows_of(&app), Some(2), "the old view stays");
                assert!(land(&mut app, rx));
            }
        }
        assert_eq!(rows_of(&app), Some(5), "page 3 of a 2-row base");
        assert_eq!(seen.opens.load(Ordering::SeqCst), 1, "opened once");
        assert_eq!(seen.views.load(Ordering::SeqCst), 2, "two views");
        assert!(app.plugin.as_ref().unwrap().session.is_some());
    }
}

/// Actions: a flash is shown, a reload opens the file again, a failure is told and the view stays.
#[test]
fn actions_report_through_the_host() {
    let (dir, file) = sandbox("konoma_plugin_host_act");
    let mut app = app_on(&dir, &file);
    let (plugin, seen) = stand(Script {
        rows: 4,
        ..Script::default()
    });
    open_stand(&mut app, plugin, &file);
    app.lang = crate::i18n::Lang::Jp;
    assert!(app.plugin_act("flash"));
    assert_eq!(app.flash.as_deref(), Some("表示した"));
    app.flash = None;
    assert!(app.plugin_act("fail"));
    assert_eq!(app.flash.as_deref(), Some("the action failed"));
    assert_eq!(rows_of(&app), Some(4), "a failed action keeps the view");
    assert!(app.plugin_act("redraw"));
    assert!(app.plugin_act("nobody-knows-this"));
    assert_eq!(
        *seen.acts.lock().unwrap(),
        ["flash", "fail", "redraw", "nobody-knows-this"]
    );
    // Without a session there is nothing to act on.
    app.drop_plugin();
    assert!(!app.plugin_act("flash"));
    assert!(!app.plugin_request_view(ViewReq::new(80, 24)));
}

/// `Outcome::Reload` runs `open` again: the file is read afresh (here: the registry has no plugin
/// `stand`, so the reload ends as the "not available" failure, which is the path it takes).
#[test]
fn a_reload_outcome_opens_the_file_again() {
    let (dir, file) = sandbox("konoma_plugin_host_reload");
    let mut app = app_on(&dir, &file);
    let (plugin, seen) = stand(Script {
        rows: 4,
        ..Script::default()
    });
    open_stand(&mut app, plugin, &file);
    assert!(app.plugin_act("reload"));
    assert!(
        app.plugin_failure().is_some(),
        "reload asks the registry for `stand`, which it does not have"
    );
    assert_eq!(seen.opens.load(Ordering::SeqCst), 1);
}

// ---- the show follows the preview -------------------------------------------------------------

/// A plugin show exists only while the preview is a plugin's, and a table is released with it.
#[test]
fn the_show_is_dropped_when_the_preview_is_not_a_plugins() {
    let (mut app, dir) = csv_app(Config::default());
    assert!(app.plugin.is_some());
    app.back_to_tree();
    assert!(app.plugin.is_none());
    assert!(!app.is_table_preview());
    // Opening another file of the same plugin replaces it.
    let other = dir.join("o.csv");
    std::fs::write(&other, "x\n1\n2\n3\n").unwrap();
    app.enter_preview(&other);
    assert_eq!(rows_of(&app), Some(3));
    let third = dir.join("p.csv");
    std::fs::write(&third, "x\n1\n").unwrap();
    app.enter_preview(&third);
    assert_eq!(rows_of(&app), Some(1));
}

/// Both table plugins go through the same mechanism and fall back as they always did.
#[test]
fn a_csv_that_cannot_be_read_and_a_bad_archive_fall_back() {
    let dir = unique_tmp("konoma_plugin_host_fallback");
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = App::new(dir.to_path_buf(), Config::default()).unwrap();
    // A directory named like a csv cannot be read as one: it is not previewable at all.
    let bad = dir.join("bad.zip");
    std::fs::write(&bad, b"not a zip file at all").unwrap();
    app.enter_preview(&bad);
    assert!(app.tab.preview_kind.as_ref().unwrap().is_plugin("archive"));
    assert!(!app.is_table_preview());
    assert!(app.plugin_failure().is_some());
}

// ---- the hand-off -----------------------------------------------------------------------------

#[test]
fn the_handoff_gives_the_result_to_exactly_one_side() {
    fn result() -> PluginResult {
        PluginResult::failed(
            1,
            "x".into(),
            ViewReq::default(),
            Failure::external(t!(en: "stand-in")),
        )
    }
    let (tx, rx) = channel();
    // The worker is quicker than the wait: the waiter gets it, the channel gets nothing.
    let h = Handoff::new();
    h.deliver(result(), &tx);
    assert!(h.wait(Duration::from_millis(50)).is_some());
    assert!(rx.try_recv().is_err());
    // The waiter gives up first: the worker sends it through the channel.
    let h = Handoff::new();
    assert!(h.wait(Duration::ZERO).is_none());
    h.deliver(result(), &tx);
    assert!(rx.try_recv().is_ok());
    // The waiter that is woken in time takes it from another thread.
    let h = Arc::new(Handoff::new());
    let worker = Arc::clone(&h);
    let tx2 = tx.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(20));
        worker.deliver(result(), &tx2);
    });
    assert!(h.wait(Duration::from_secs(5)).is_some());
    assert!(rx.try_recv().is_err());
}

/// The layout gate gives what `open` learned to exactly one side: the waiting UI, or the channel
/// when the UI has stopped waiting; and a job that ends without a layout releases a waiter.
#[test]
fn the_layout_gate_gives_the_layout_to_exactly_one_side() {
    use super::plugin_host::LayoutGate;
    let layout = || Layout {
        default_repr: Some("g".into()),
        ..Layout::single()
    };
    // Published in time: the waiter gets it and the worker has nothing to send.
    let g = LayoutGate::new();
    assert!(g.publish(layout()).is_none());
    assert!(g.wait(Duration::from_millis(50)).is_some());
    // The waiter gave up first: the worker is handed the layout back to send itself.
    let g = LayoutGate::new();
    assert!(g.wait(Duration::ZERO).is_none());
    assert!(g.publish(layout()).is_some());
    // A waiter woken from another thread takes it.
    let g = Arc::new(LayoutGate::new());
    let worker = Arc::clone(&g);
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(20));
        assert!(worker.publish(layout()).is_none());
    });
    assert!(g.wait(Duration::from_secs(5)).is_some());
    // A job that never got as far as a layout (a failed open) releases the waiter at once, with
    // nothing.
    let g = Arc::new(LayoutGate::new());
    let worker = Arc::clone(&g);
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(20));
        worker.release();
    });
    let t0 = Instant::now();
    assert!(g.wait(Duration::from_secs(5)).is_none());
    assert!(
        t0.elapsed() < Duration::from_secs(2),
        "released, not timed out"
    );
}

// ---- the install step itself: only the newest generation is applied ----------------------------

fn table_view(rows: usize) -> View {
    View::Table(TableView {
        grid: Arc::new(Rows(rows)),
        title: None,
        rainbow: true,
        addressed: false,
    })
}

/// An answer for the stand-in plugin of generation `gen` carrying a table of `rows` rows.
fn answer_of(gen: u64, rows: usize) -> PluginResult {
    PluginResult::answer(
        gen,
        "stand",
        None,
        Some(table_view(rows)),
        None,
        ViewReq::new(80, 24),
    )
}

/// An answer of an older generation for the same plugin and the same file (two opens of one file,
/// two page turns) is not put into the show, however it got here, and it does not end the loading
/// mark of the newer job; the newest generation is applied and ends it.
#[test]
fn a_result_of_an_older_generation_is_not_installed_for_the_same_file() {
    let (dir, file) = sandbox("konoma_plugin_host_install_gen");
    let mut app = app_on(&dir, &file);
    let (plugin, _) = stand(Script {
        rows: 5,
        ..Script::default()
    });
    open_stand(&mut app, plugin, &file);
    assert_eq!(rows_of(&app), Some(5));
    // A newer job is out (a page turn): the loading mark is up.
    app.bump_media_gen();
    app.media_loading = true;
    let newest = app.media_gen;
    // The result of the older job arrives late.
    assert!(!app.install_plugin_result(answer_of(newest - 1, 9)));
    assert_eq!(rows_of(&app), Some(5), "the late page is not shown");
    assert!(app.media_loading, "the newer job is still out");
    // The result of the newest job is applied and ends the mark.
    assert!(app.install_plugin_result(answer_of(newest, 7)));
    assert_eq!(rows_of(&app), Some(7));
    assert!(!app.media_loading);
    // And a result from the future of nothing (a generation not yet reached) is not the newest.
    app.media_loading = true;
    assert!(!app.install_plugin_result(answer_of(newest + 1, 3)));
    assert_eq!(rows_of(&app), Some(7));
    assert!(app.media_loading);
}

/// A failure of an older generation does not replace the view of the newer one either.
#[test]
fn a_failure_of_an_older_generation_does_not_replace_the_view() {
    let (dir, file) = sandbox("konoma_plugin_host_install_gen_fail");
    let mut app = app_on(&dir, &file);
    let (plugin, _) = stand(Script {
        rows: 4,
        ..Script::default()
    });
    open_stand(&mut app, plugin, &file);
    app.bump_media_gen();
    let old = app.media_gen - 1;
    let failed = PluginResult::failed(
        old,
        "stand".into(),
        ViewReq::new(80, 24),
        Failure::external(t!(en: "late failure")),
    );
    assert!(!app.install_plugin_result(failed));
    assert!(app.plugin_failure().is_none());
    assert_eq!(rows_of(&app), Some(4));
}

/// `[external] pdf` and `[external] video` are what the worker is told to allow; no other plugin is
/// ever allowed to start an external program.
#[test]
fn the_external_switches_reach_the_work_of_the_plugin_they_belong_to() {
    let (dir, file) = sandbox("konoma_plugin_host_external");
    for (pdf, video) in [(true, false), (false, true), (true, true), (false, false)] {
        let mut cfg = Config::default();
        cfg.external.pdf = pdf;
        cfg.external.video = video;
        let app = App::new(dir.to_path_buf(), cfg).unwrap();
        let work = |id: &str| {
            let (plugin, _) = stand(Script::default());
            let job = plugin_host::PluginJob::Open {
                plugin,
                input: Input::File {
                    path: file.clone(),
                    hint: None,
                },
            };
            app.plugin_work(id.to_string(), file.clone(), job, ViewReq::default())
                .external
        };
        assert_eq!(work(konoma_media::PDF_ID), pdf, "pdf={pdf}");
        assert_eq!(work(konoma_media::VIDEO_ID), video, "video={video}");
        assert!(!work(konoma_media::IMAGE_ID));
        assert!(!work(konoma_table::CSV_ID));
        assert!(!work("stand"));
    }
}

// ---- a stale answer hands the session back (`reclaim_plugin_session`) ---------------------------

/// An app with the stand-in open on `file` and ready, the session of its show taken out the way a
/// job takes it, and a stale answer carrying that session and `outcome`.
fn stale_answer_with_session(
    app: &mut App,
    file: &Path,
    outcome: Option<Outcome>,
) -> (Arc<Seen>, MediaResult) {
    let (plugin, seen) = stand(Script {
        rows: 2,
        ..Script::default()
    });
    open_stand(app, plugin, file);
    let session = app.plugin.as_mut().unwrap().session.take();
    assert!(session.is_some(), "the show was ready with its session");
    app.bump_media_gen();
    let mut res = answer_of(app.media_gen - 1, 9);
    res.session = session;
    res.path = file.to_path_buf();
    res.outcome = outcome;
    let result = MediaResult {
        gen: res.gen,
        payload: Some(MediaPayload::Plugin(Box::new(res))),
    };
    (seen, result)
}

/// The stale answer is not applied; what it carries is the session and what it said.
#[test]
fn a_stale_answer_returns_its_session_and_flash_to_the_open_show() {
    let (dir, file) = sandbox("konoma_plugin_host_reclaim_flash");
    let mut app = app_on(&dir, &file);
    let (seen, stale) = stale_answer_with_session(
        &mut app,
        &file,
        Some(Outcome::Flash(t!(en: "flashed", ja: "表示した"))),
    );
    assert!(!app.apply_media(stale));
    assert!(app.plugin.as_ref().unwrap().session.is_some(), "given back");
    assert_eq!(
        app.flash.as_deref(),
        Some("flashed"),
        "what it said is said"
    );
    assert_eq!(rows_of(&app), Some(2), "the stale view is not applied");
    // And the session is the one that came back: the next action runs on it.
    assert!(app.plugin_act("again"));
    assert_eq!(*seen.acts.lock().unwrap(), ["again"]);
}

/// A `Reload` the stale action asked for is still done: the file is opened again.
#[test]
fn a_stale_answer_that_asked_for_a_reload_reloads() {
    let (dir, file) = sandbox("konoma_plugin_host_reclaim_reload");
    let mut app = app_on(&dir, &file);
    let (_, stale) = stale_answer_with_session(&mut app, &file, Some(Outcome::Reload));
    assert!(app.plugin_failure().is_none());
    assert!(!app.apply_media(stale));
    assert!(
        app.plugin_failure().is_some(),
        "the reload asked the registry for `stand`, which it does not have"
    );
}

/// A show that already holds a session (something else gave one back first) keeps it: the stale
/// one is dropped, not put over it.
#[test]
fn a_stale_answer_does_not_replace_a_session_the_show_holds() {
    let (dir, file) = sandbox("konoma_plugin_host_reclaim_has");
    let mut app = app_on(&dir, &file);
    let (stale_seen, stale) =
        stale_answer_with_session(&mut app, &file, Some(Outcome::Flash(t!(en: "flashed"))));
    // The show's own session: a second, distinguishable stand-in.
    let (own, own_seen) = stand(Script {
        rows: 2,
        ..Script::default()
    });
    open_stand(&mut app, own, &file);
    assert!(app.plugin.as_ref().unwrap().session.is_some());
    // The first open moved the generation on: the stale answer is older than that too.
    assert!(!app.apply_media(stale));
    assert!(app.flash.is_none(), "nothing of the stale answer is said");
    assert!(app.plugin_act("mine"));
    assert_eq!(*own_seen.acts.lock().unwrap(), ["mine"]);
    assert!(stale_seen.acts.lock().unwrap().is_empty());
}

/// A show that is not ready (still loading) does not take the session back.
#[test]
fn a_stale_answer_is_not_given_to_a_show_that_is_not_ready() {
    let (dir, file) = sandbox("konoma_plugin_host_reclaim_phase");
    let mut app = app_on(&dir, &file);
    let (_, stale) =
        stale_answer_with_session(&mut app, &file, Some(Outcome::Flash(t!(en: "flashed"))));
    app.plugin.as_mut().unwrap().phase = plugin_host::PluginPhase::Loading;
    assert!(!app.apply_media(stale));
    assert!(app.plugin.as_ref().unwrap().session.is_none());
    assert!(app.flash.is_none());
}

/// A show of another file (the preview moved to a file of the same plugin) does not take it.
#[test]
fn a_stale_answer_is_not_given_to_a_show_of_another_file() {
    let (dir, file) = sandbox("konoma_plugin_host_reclaim_file");
    let mut app = app_on(&dir, &file);
    let (_, stale) =
        stale_answer_with_session(&mut app, &file, Some(Outcome::Flash(t!(en: "flashed"))));
    let other = dir.join("other.stand");
    std::fs::write(&other, "y").unwrap();
    app.tab.preview_kind = Some(PreviewKind::Plugin {
        path: other,
        id: "stand".into(),
        hint: None,
    });
    assert!(!app.apply_media(stale));
    assert!(app.plugin.as_ref().unwrap().session.is_none());
    assert!(app.flash.is_none());
}

/// An answer of another plugin never goes to this plugin's show.
#[test]
fn a_stale_answer_is_not_given_to_a_show_of_another_plugin() {
    let (dir, file) = sandbox("konoma_plugin_host_reclaim_id");
    let mut app = app_on(&dir, &file);
    let (_, mut stale) =
        stale_answer_with_session(&mut app, &file, Some(Outcome::Flash(t!(en: "flashed"))));
    if let Some(MediaPayload::Plugin(res)) = stale.payload.as_mut() {
        res.id = "someone-else".into();
    }
    assert!(!app.apply_media(stale));
    assert!(app.plugin.as_ref().unwrap().session.is_none());
    assert!(app.flash.is_none());
}

// ---- the page state follows what the session reports -------------------------------------------

/// A session that reports separate pages gives the tab a page count; one that reports none (a
/// workbook that failed to open has no sheets) takes it away, so the page keys have nothing to
/// turn; one that reports pages of another kind leaves the count as it was.
#[test]
fn the_page_count_follows_the_layout_the_session_reports() {
    use konoma_plugin::view::PageLayout;
    let (dir, file) = sandbox("konoma_plugin_host_page_count");
    let mut app = app_on(&dir, &file);
    let (plugin, _) = stand(Script {
        rows: 2,
        ..Script::default()
    });
    open_stand(&mut app, plugin, &file);
    let answer = |app: &App, layout: Layout, failed: bool| {
        let (view, failure) = if failed {
            (None, Some(Failure::external(t!(en: "no sheets"))))
        } else {
            (Some(table_view(2)), None)
        };
        PluginResult::answer(
            app.media_gen,
            "stand",
            Some(layout),
            view,
            failure,
            ViewReq::new(80, 24),
        )
    };
    let separate = |n: usize| Layout {
        pages: Some(PageLayout::Separate {
            count: Some(n),
            titles: Vec::new(),
        }),
        ..Layout::single()
    };
    assert!(app.install_plugin_result(answer(&app, separate(5), false)));
    assert_eq!(app.tab.page_count, Some(5));
    // Pages that the host's page keys do not turn: the count is not touched.
    let anchored = Layout {
        pages: Some(PageLayout::Anchored {
            heading_level: 1,
            count: 3,
        }),
        ..Layout::single()
    };
    assert!(app.install_plugin_result(answer(&app, anchored, false)));
    assert_eq!(
        app.tab.page_count,
        Some(5),
        "anchored pages are not its pages"
    );
    // No pages at all (the file could not be opened as a book): the page keys have nothing to turn.
    assert!(app.install_plugin_result(answer(&app, Layout::single(), true)));
    assert_eq!(app.tab.page_count, None);
    // And the other way round: a count comes back with the next good answer.
    assert!(app.install_plugin_result(answer(&app, separate(2), false)));
    assert_eq!(app.tab.page_count, Some(2));
    assert!(app.install_plugin_result(answer(&app, Layout::single(), false)));
    assert_eq!(app.tab.page_count, None);
}

// ---- a key's Redraw is asked again only when the very result did not carry a view ----------------
//
// A keys job takes the new view in the same job when an answer asked for a redraw. The host asks
// the plugin again only when that result carries no answer of a `view` (`viewed`). When the view
// failed, or came back as a document or a picture, the result is the answer: asking again would
// make the same failing call again, for ever.

/// The stand-in open and ready, its session taken the way a job takes it, and a result of the
/// current generation that gives the session back with `Outcome::Redraw` and no view.
fn redraw_answer(app: &mut App, file: &Path) -> (Arc<Seen>, PluginResult) {
    let (plugin, seen) = stand(Script {
        rows: 2,
        ..Script::default()
    });
    open_stand(app, plugin, file);
    assert_eq!(seen.views.load(Ordering::SeqCst), 1, "the open drew once");
    let session = app.plugin.as_mut().unwrap().session.take();
    assert!(session.is_some(), "the show was ready with its session");
    let mut res = answer_of(app.media_gen, 2);
    res.view = None;
    res.session = session;
    res.outcome = Some(Outcome::Redraw);
    (seen, res)
}

/// Install `res` as the answer of the current job; how many times the plugin was asked for a view
/// because of it, and whether the session is back in the show.
fn views_asked_by(app: &mut App, seen: &Seen, res: PluginResult) -> usize {
    let before = seen.views.load(Ordering::SeqCst);
    assert!(
        app.install_plugin_result(res),
        "the current result is applied"
    );
    seen.views.load(Ordering::SeqCst) - before
}

/// The control: a Redraw that carries no view at all (an action's own result) is asked for.
#[test]
fn a_redraw_answer_without_a_view_asks_the_plugin_for_one() {
    let (dir, file) = sandbox("konoma_plugin_host_viewed_none");
    let mut app = app_on(&dir, &file);
    let (seen, res) = redraw_answer(&mut app, &file);
    assert_eq!(views_asked_by(&mut app, &seen, res), 1);
    assert!(app.plugin.as_ref().unwrap().session.is_some());
}

/// The view the keys job asked for failed: the failure is the answer. Asking again would fail
/// again, and again (an endless loop of requests); the show is failed and stays so.
#[test]
fn a_redraw_answer_whose_view_failed_does_not_ask_again() {
    let (dir, file) = sandbox("konoma_plugin_host_viewed_failure");
    let mut app = app_on(&dir, &file);
    let (seen, mut res) = redraw_answer(&mut app, &file);
    res.failure = Some(Failure::external(t!(en: "the view failed")));
    assert_eq!(views_asked_by(&mut app, &seen, res), 0, "no second request");
    assert!(app.plugin_failure().is_some(), "the failure is shown");
    assert!(
        app.plugin.as_ref().unwrap().session.is_some(),
        "and the session was given back, not taken by a new request"
    );
    assert!(!app.is_media_loading(), "nothing is out");
}

/// The view came back as a picture (decoded or failed to decode): the result is the answer.
#[test]
fn a_redraw_answer_that_carries_a_picture_does_not_ask_again() {
    for picture in [
        plugin_host::Picture::Failed,
        plugin_host::Picture::Canvas(None),
    ] {
        let (dir, file) = sandbox("konoma_plugin_host_viewed_picture");
        let mut app = app_on(&dir, &file);
        let (seen, mut res) = redraw_answer(&mut app, &file);
        res.picture = picture;
        assert_eq!(views_asked_by(&mut app, &seen, res), 0, "no second request");
        assert!(app.plugin.as_ref().unwrap().session.is_some());
        assert!(!app.is_media_loading());
    }
}

/// The view came back as a document: the result is the answer.
#[test]
fn a_redraw_answer_that_carries_a_document_does_not_ask_again() {
    let (dir, file) = sandbox("konoma_plugin_host_viewed_document");
    let mut app = app_on(&dir, &file);
    let (seen, mut res) = redraw_answer(&mut app, &file);
    res.document = Some(Box::new(super::office_doc::LoadedDocument::from_view(
        "# doc".into(),
        false,
        Default::default(),
        Default::default(),
        konoma_plugin::Origin::File(file.clone()),
    )));
    assert_eq!(views_asked_by(&mut app, &seen, res), 0, "no second request");
    assert!(app.plugin.as_ref().unwrap().session.is_some());
    assert!(!app.is_media_loading());
}

#[test]
fn a_tables_frame_title_is_cut_and_cleaned_like_every_other_text_a_plugin_says() {
    // The title is drawn in the host's own frame, every frame: 10 MB of it must not reach the
    // view, and a private-use placeholder character must not (it would paint as a picture).
    let long = format!("{}\u{10EEEE}\u{1b}[31m", "x".repeat(10_000_000));
    let mut res = PluginResult::answer(
        1,
        "stand",
        None,
        Some(View::Table(TableView {
            grid: Arc::new(Rows(1)),
            title: Some(long),
            rainbow: true,
            addressed: false,
        })),
        None,
        ViewReq::new(80, 24),
    );
    res.clip_texts();
    let Some(View::Table(t)) = &res.view else {
        panic!("still a table");
    };
    let title = t.title.as_deref().unwrap();
    assert!(title.chars().count() <= 512, "{}", title.chars().count());
    assert!(!title.contains('\u{10EEEE}') && !title.contains('\u{1b}'));
    // A short, honest title is unchanged, and no title stays none.
    let mut ok = PluginResult::answer(
        1,
        "stand",
        None,
        Some(View::Table(TableView {
            grid: Arc::new(Rows(1)),
            title: Some("Sheet2 (2/3)".into()),
            rainbow: true,
            addressed: false,
        })),
        None,
        ViewReq::new(80, 24),
    );
    ok.clip_texts();
    assert_eq!(
        ok.view
            .as_ref()
            .map(|v| matches!(v, View::Table(t) if t.title.as_deref() == Some("Sheet2 (2/3)"))),
        Some(true)
    );
}

// ---- the read scope on a text file a plugin names (`TextSource::File`) --------------------------------

mod text_scope {
    use super::super::plugin_host::gate_text_file;
    use crate::test_support::unique_tmp;
    use konoma_plugin::view::{TextSource, TextView, View};
    use konoma_plugin::{FailureKind, ReadScope};
    use std::path::{Path, PathBuf};

    fn text_of(p: &Path) -> View {
        View::Text(TextView {
            source: TextSource::File(p.to_path_buf()),
            styler: None,
            file_lines: false,
            syntax_name: None,
        })
    }

    fn named(v: &View) -> PathBuf {
        match v {
            View::Text(TextView {
                source: TextSource::File(p),
                ..
            }) => p.clone(),
            _ => panic!("not a file text"),
        }
    }

    #[test]
    fn a_file_outside_the_scope_is_refused_and_the_message_names_the_key() {
        let dir = unique_tmp("ts_out");
        std::fs::create_dir_all(dir.join("root")).unwrap();
        std::fs::create_dir_all(dir.join("out")).unwrap();
        std::fs::write(dir.join("out/t.txt"), "x").unwrap();
        let scope = ReadScope::within([dir.join("root")]);
        let mut v = text_of(&dir.join("out/t.txt"));
        let f = gate_text_file(&mut v, "demo", &dir.join("root/a.x"), &scope).unwrap_err();
        assert_eq!(f.kind, FailureKind::Io);
        assert!(f.message.en().contains("read_dirs"));
        assert!(f.message.en().contains("[plugins.demo]"));
        assert!(f.message.get("ja").contains("read_dirs"));
    }

    #[test]
    fn a_file_inside_is_read_through_its_real_path() {
        let dir = unique_tmp("ts_in");
        std::fs::create_dir_all(dir.join("root")).unwrap();
        std::fs::write(dir.join("root/t.txt"), "x").unwrap();
        let scope = ReadScope::within([dir.join("root")]);
        let mut v = text_of(&dir.join("root/../root/t.txt"));
        gate_text_file(&mut v, "demo", &dir.join("root/a.x"), &scope).unwrap();
        // The path windowed is the one that was judged.
        assert_eq!(
            named(&v),
            std::fs::canonicalize(dir.join("root/t.txt")).unwrap()
        );
    }

    #[test]
    fn a_link_out_is_refused_and_the_previewed_file_is_allowed() {
        let dir = unique_tmp("ts_link");
        std::fs::create_dir_all(dir.join("root")).unwrap();
        std::fs::create_dir_all(dir.join("out")).unwrap();
        std::fs::write(dir.join("out/t.txt"), "x").unwrap();
        std::os::unix::fs::symlink(dir.join("out/t.txt"), dir.join("root/l.txt")).unwrap();
        let scope = ReadScope::within([dir.join("root")]);
        let mut v = text_of(&dir.join("root/l.txt"));
        assert!(gate_text_file(&mut v, "demo", &dir.join("root/a.x"), &scope).is_err());
        // The previewed file itself, wherever it is.
        let shown = dir.join("out/t.txt");
        let mut v = text_of(&shown);
        gate_text_file(&mut v, "demo", &shown, &scope).unwrap();
    }

    #[test]
    fn the_prefix_trap_and_dot_dot() {
        let dir = unique_tmp("ts_trap");
        std::fs::create_dir_all(dir.join("b")).unwrap();
        std::fs::create_dir_all(dir.join("bc")).unwrap();
        std::fs::write(dir.join("bc/t.txt"), "x").unwrap();
        let scope = ReadScope::within([dir.join("b")]);
        let shown = dir.join("b/a.x");
        let mut v = text_of(&dir.join("bc/t.txt"));
        assert!(gate_text_file(&mut v, "demo", &shown, &scope).is_err());
        let mut v = text_of(&dir.join("b/../bc/t.txt"));
        assert!(gate_text_file(&mut v, "demo", &shown, &scope).is_err());
    }

    #[test]
    fn an_unrestricted_scope_and_other_sources_are_left_alone() {
        let free = ReadScope::unrestricted();
        let mut v = text_of(Path::new("/etc/hosts"));
        gate_text_file(&mut v, "demo", Path::new("/x/a"), &free).unwrap();
        assert_eq!(named(&v), Path::new("/etc/hosts"));
        let scope = ReadScope::within([Path::new("/nonexistent-scope")]);
        let mut lines = View::text(["a"]);
        gate_text_file(&mut lines, "demo", Path::new("/x/a"), &scope).unwrap();
    }

    #[test]
    fn a_vanished_file_is_not_blamed_on_the_scope() {
        let dir = unique_tmp("ts_gone");
        std::fs::create_dir_all(dir.join("root")).unwrap();
        let scope = ReadScope::within([dir.join("root")]);
        let mut v = text_of(&dir.join("root/none.txt"));
        let f = gate_text_file(&mut v, "demo", &dir.join("root/a.x"), &scope).unwrap_err();
        assert!(!f.message.en().contains("read_dirs"));
    }
}

// ---- the scope the host settles for a job -----------------------------------------------------------

mod job_scope {
    use super::*;

    fn scope_dirs(app: &App, id: &str, file: &Path) -> Option<Vec<PathBuf>> {
        let (plugin, _) = stand(Script::default());
        let job = plugin_host::PluginJob::Open {
            plugin,
            input: Input::File {
                path: file.to_path_buf(),
                hint: None,
            },
        };
        let work = app.plugin_work(id.to_string(), file.to_path_buf(), job, ViewReq::default());
        work.read_scope.dirs().map(<[PathBuf]>::to_vec)
    }

    #[test]
    fn a_user_plugin_gets_the_root_and_the_previewed_folder() {
        let (dir, file) = sandbox("konoma_job_scope");
        let app = App::new(dir.to_path_buf(), Config::default()).unwrap();
        let got = scope_dirs(&app, "stand", &file).expect("a user plugin is limited");
        let real = std::fs::canonicalize(&*dir).unwrap();
        assert!(got.contains(&real), "{got:?}");
    }

    #[test]
    fn the_standard_plugins_are_not_limited() {
        let (dir, file) = sandbox("konoma_job_scope_std");
        let app = App::new(dir.to_path_buf(), Config::default()).unwrap();
        for id in [
            konoma_markdown::MARKDOWN_ID,
            konoma_media::IMAGE_ID,
            konoma_media::PDF_ID,
            konoma_table::CSV_ID,
            // The host's internal pseudo plugins: the command plugin names the private file its
            // output went to.
            crate::plugins::COMMAND_ID,
            crate::plugins::TEXT_ID,
            crate::plugins::UNSUPPORTED_ID,
        ] {
            assert!(scope_dirs(&app, id, &file).is_none(), "{id} is limited");
        }
    }

    #[test]
    fn read_dirs_in_the_config_widens_the_job_scope() {
        let (dir, file) = sandbox("konoma_job_scope_cfg");
        let extra = dir.join("extra");
        std::fs::create_dir_all(&extra).unwrap();
        let cfg = cfg_from(&format!(
            "[plugins.stand]\nread_dirs = [{:?}]\nlevel = 2\n",
            extra.to_str().unwrap()
        ));
        let app = App::new(dir.to_path_buf(), cfg).unwrap();
        let got = scope_dirs(&app, "stand", &file).unwrap();
        assert!(
            got.contains(&std::fs::canonicalize(&extra).unwrap()),
            "{got:?}"
        );
        // The plugin's own settings are untouched and do not carry the host key.
        let s = crate::plugins::effective_settings(&app.cfg, "stand");
        assert_eq!(s.int("level", 0), 2);
        assert!(s.get("read_dirs").is_none());
    }
}

// ---- the read scope of a job ------------------------------------------------------------------

/// A job for plugin `id` on `path`, as every key press and timed redraw builds one on the UI thread.
fn scoped_job(app: &App, id: &str, path: &Path) -> super::plugin_host::PluginWork {
    let (plugin, _) = stand(Script::default());
    let input = Input::File {
        path: path.to_path_buf(),
        hint: None,
    };
    app.plugin_work(
        id.to_string(),
        path.to_path_buf(),
        super::plugin_host::PluginJob::Open { plugin, input },
        ViewReq::new(80, 24),
    )
}

#[test]
fn a_user_plugins_scope_is_built_once_for_many_jobs() {
    use crate::plugins::read_scope::SCOPE_BUILDS;
    let (dir, file) = sandbox("konoma_plugin_scope_memo");
    let other = dir.join("g.stand");
    std::fs::write(&other, "y").unwrap();
    let app = app_on(&dir, &file);
    let first = scoped_job(&app, "demo", &file);
    assert!(first.read_scope.is_restricted());
    let before = SCOPE_BUILDS.with(|c| c.get());
    for _ in 0..30 {
        let w = scoped_job(&app, "demo", &file);
        assert_eq!(w.read_scope.fingerprint(), first.read_scope.fingerprint());
    }
    assert_eq!(
        SCOPE_BUILDS.with(|c| c.get()),
        before,
        "jobs reuse the scope"
    );
    // Another shown file, another plugin and another root are each judged anew (once).
    let _ = scoped_job(&app, "demo", &other);
    assert_eq!(SCOPE_BUILDS.with(|c| c.get()), before + 1);
    let _ = scoped_job(&app, "demo", &other);
    assert_eq!(SCOPE_BUILDS.with(|c| c.get()), before + 1);
    let _ = scoped_job(&app, "demo2", &other);
    assert_eq!(SCOPE_BUILDS.with(|c| c.get()), before + 2);
    let mut app = app;
    app.tab.root = dir.join("sub");
    let _ = scoped_job(&app, "demo2", &other);
    assert_eq!(SCOPE_BUILDS.with(|c| c.get()), before + 3);
}

#[test]
fn a_standard_plugins_job_does_no_scope_work() {
    use crate::plugins::read_scope::SCOPE_BUILDS;
    let (dir, file) = sandbox("konoma_plugin_scope_standard");
    let app = app_on(&dir, &file);
    let before = SCOPE_BUILDS.with(|c| c.get());
    for _ in 0..10 {
        assert!(!scoped_job(&app, "markdown", &file)
            .read_scope
            .is_restricted());
    }
    assert_eq!(SCOPE_BUILDS.with(|c| c.get()), before);
    assert!(app.read_scope_memo.borrow().is_none());
}

// ---- the `O` list's popup width and a layout reported again ------------------------------------

fn paged(count: usize, tag: &str) -> Layout {
    Layout {
        pages: Some(konoma_plugin::view::PageLayout::Separate {
            count: Some(count),
            titles: (0..count).map(|i| format!("{tag}{i}")).collect(),
        }),
        ..Layout::default()
    }
}

/// A session that reports the same layout again while `O` is open leaves the width floor alone
/// (the popup never shrinks while open); a layout whose list differs starts the width afresh.
#[test]
fn a_layout_reported_again_keeps_the_page_list_width_floor_and_a_changed_list_resets_it() {
    let (dir, file) = sandbox("konoma_plugin_host_outline_width");
    let _budget = LongFirstFrame::of(Duration::from_secs(5));
    let mut app = app_on(&dir, &file);
    let (tx, _rx) = channel();
    app.attach_media_loader(tx);
    let (plugin, _) = stand(Script {
        rows: 3,
        ..Script::default()
    });
    open_stand(&mut app, plugin, &file);
    let a = paged(3, "t");
    app.plugin.as_mut().unwrap().layout = a.clone();
    app.tab.page_count = Some(3);
    assert!(app.is_page_list_preview());
    app.outline_open = true;
    app.reset_outline_width_for_open();
    app.outline_width_seen("", 60);
    // The same layout, as a new allocation with the same content: nothing changed.
    let again = paged(3, "t");
    app.plugin.as_mut().unwrap().layout = again.clone();
    app.adopt_layout(&again);
    assert_eq!(app.outline_width_floor(""), 60, "the floor survives");
    // The titles differ: a different list, sized afresh.
    let b = paged(3, "u");
    app.plugin.as_mut().unwrap().layout = b.clone();
    app.adopt_layout(&b);
    assert_eq!(app.outline_width_floor(""), 0);
    app.outline_width_seen("", 40);
    // The count differs: the same.
    let c = paged(4, "u");
    app.plugin.as_mut().unwrap().layout = c.clone();
    app.adopt_layout(&c);
    assert_eq!(app.outline_width_floor(""), 0);
    // Closed: a layout does not touch anything.
    app.outline_width_seen("", 30);
    app.outline_open = false;
    let d = paged(4, "w");
    app.plugin.as_mut().unwrap().layout = d.clone();
    app.adopt_layout(&d);
    assert_eq!(app.outline_width_floor(""), 30);
}
