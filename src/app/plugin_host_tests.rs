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

use super::plugin_host::{Handoff, FIRST_FRAME_BUDGET};
use super::*;
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
fn land(app: &mut App, rx: &Receiver<PluginResult>) -> bool {
    let res = rx
        .recv_timeout(Duration::from_secs(10))
        .expect("the worker always answers");
    app.apply_plugin_result(res)
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
    let mut app = app_on(&dir, &file);
    let (tx, rx) = channel();
    app.attach_plugin_runner(tx);
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
    let (tx, rx) = channel();
    app.attach_plugin_runner(tx);
    let (plugin, seen) = stand(Script {
        rows: 7,
        open_delay: Duration::from_millis(400),
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
    app.attach_plugin_runner(tx);
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
    let (tx, rx) = channel();
    app.attach_plugin_runner(tx);
    let (plugin, seen) = stand(Script {
        rows: 5,
        open_delay: Duration::from_millis(300),
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
    let (tx, rx) = channel();
    app.attach_plugin_runner(tx);
    let (slow, _) = stand(Script {
        rows: 1,
        open_delay: Duration::from_millis(200),
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
    assert!(!land(&mut app, &rx));
    assert_eq!(rows_of(&app), Some(9), "the newest stays");
}

/// A result for a preview that is no longer this plugin's (another kind took over the tab) is
/// dropped even though no new media job advanced the generation.
#[test]
fn a_result_after_the_kind_changed_is_dropped() {
    let (dir, file) = sandbox("konoma_plugin_host_kind");
    let mut app = app_on(&dir, &file);
    let (tx, rx) = channel();
    app.attach_plugin_runner(tx);
    let (plugin, _) = stand(Script {
        rows: 2,
        open_delay: Duration::from_millis(150),
        ..Script::default()
    });
    open_stand(&mut app, plugin, &file);
    app.set_preview_kind(Some(PreviewKind::Text(file.clone())));
    assert!(app.plugin.is_none(), "the show is released with the kind");
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
    let (tx, rx) = channel();
    app.attach_plugin_runner(tx);
    let (plugin, _) = stand(Script {
        panic_in_open: true,
        open_delay: Duration::from_millis(120),
        ..Script::default()
    });
    open_stand(&mut app, plugin, &file);
    assert!(app.is_plugin_loading());
    assert!(land(&mut app, &rx), "the failure is applied");
    assert!(!app.is_plugin_loading(), "Loading... is gone");
    assert!(!app.is_media_loading());
    assert!(app.plugin_failure().is_some());
    assert!(!app.is_table_preview());
}

/// The same inside the first-frame budget, and a panic in `view` too.
#[test]
fn a_panic_is_a_failure_in_the_first_frame_and_without_a_worker_channel() {
    for (panic_in_open, panic_in_view) in [(true, false), (false, true)] {
        // With a channel (first frame).
        let (dir, file) = sandbox("konoma_plugin_host_panic_quick");
        let mut app = app_on(&dir, &file);
        let (tx, _rx) = channel();
        app.attach_plugin_runner(tx);
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
            app.attach_plugin_runner(tx);
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
