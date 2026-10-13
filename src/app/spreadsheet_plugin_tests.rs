//! The `spreadsheet` plugin on the host's plugin road (and the work group the Word and
//! presentation plugins share with it): the exclusive work group (one read at a
//! time, only the newest waiting request kept), the sheets as pages, the settings it receives and
//! the switch that turns it off. The reading and the words are tested in `crates/konoma-office`;
//! the keys, the cursor and the search over real workbooks in `e2e_tests`.
//!
//! The work-group tests use a stand-in plugin whose view takes a while, so that "how many ran at
//! the same time" is observed on its own and does not depend on how fast a real parse is.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
use std::time::Duration;

use konoma_plugin::view::{Grid, Layout, TableView, View, ViewReq};
use konoma_plugin::{t, Failure, Input, Manifest, OpenCx, Plugin, Session, ViewCx};

use super::plugin_host::{PluginJob, PluginPhase, PluginShow, Wait};
use super::*;
use crate::test_support::unique_tmp;

const GROUP: &str = "test-group";

/// A grid of `n` rows and one column.
struct Rows(usize);

impl Grid for Rows {
    fn nrows(&self) -> usize {
        self.0
    }
    fn ncols(&self) -> usize {
        1
    }
    fn header(&self, _col: usize) -> Option<String> {
        None
    }
    fn cell(&self, row: usize, _col: usize) -> String {
        format!("r{row}")
    }
}

/// What the heavy stand-in saw, shared with the test.
#[derive(Default)]
struct Seen {
    running: AtomicUsize,
    most_at_once: AtomicUsize,
    views: AtomicUsize,
    cancelled: AtomicBool,
}

/// A plugin whose `view` takes `VIEW_MS` (long, so that a loaded machine cannot let it end before
/// the requests have been made) and stops early when it is cancelled, like the reader of a big
/// workbook does between rows.
struct Heavy {
    seen: Arc<Seen>,
    panic_in_view: bool,
}

const VIEW_MS: u64 = 400;

struct HeavySession {
    seen: Arc<Seen>,
    panic_in_view: bool,
}

impl Plugin for Heavy {
    fn manifest(&self) -> Manifest {
        Manifest::new("stand", "0").work_group(GROUP)
    }

    fn open(&self, _input: Input, _cx: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        Ok(Box::new(HeavySession {
            seen: Arc::clone(&self.seen),
            panic_in_view: self.panic_in_view,
        }))
    }
}

impl Session for HeavySession {
    fn view(&mut self, req: &ViewReq, cx: &ViewCx) -> Result<View, Failure> {
        self.seen.views.fetch_add(1, Ordering::SeqCst);
        let now = self.seen.running.fetch_add(1, Ordering::SeqCst) + 1;
        self.seen.most_at_once.fetch_max(now, Ordering::SeqCst);
        let mut cancelled = false;
        for _ in 0..VIEW_MS {
            if cx.cancel.is_cancelled() {
                cancelled = true;
                self.seen.cancelled.store(true, Ordering::SeqCst);
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        self.seen.running.fetch_sub(1, Ordering::SeqCst);
        if self.panic_in_view {
            panic!("the stand-in plugin panics in view");
        }
        if cancelled {
            return Err(Failure::cancelled(t!(en: "cancelled")));
        }
        Ok(View::Table(TableView {
            grid: Arc::new(Rows(1 + req.page.unwrap_or(0))),
            title: None,
            rainbow: true,
            addressed: false,
        }))
    }

    fn layout(&self) -> Layout {
        Layout::single()
    }
}

fn heavy(panic_in_view: bool) -> (Arc<dyn Plugin>, Arc<Seen>) {
    let seen = Arc::new(Seen::default());
    (
        Arc::new(Heavy {
            seen: Arc::clone(&seen),
            panic_in_view,
        }),
        seen,
    )
}

/// An app previewing `f.stand` as the stand-in plugin, with the worker channel attached and a show
/// that waits for its first answer.
fn app_with_worker() -> (
    App,
    crate::test_support::TmpDir,
    PathBuf,
    Receiver<MediaResult>,
) {
    let dir = unique_tmp("konoma_spreadsheet_plugin");
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("f.stand");
    std::fs::write(&file, "x").unwrap();
    let mut app = App::new(dir.to_path_buf(), Config::default()).unwrap();
    app.tab.preview_kind = Some(PreviewKind::Plugin {
        path: file.clone(),
        id: "stand".into(),
        hint: None,
    });
    app.tab.preview_path = Some(file.clone());
    app.tab.mode = Mode::Preview;
    let (tx, rx) = channel();
    app.attach_media_loader(tx);
    app.plugin = Some(PluginShow {
        id: "stand".into(),
        path: file.clone(),
        phase: PluginPhase::Loading,
        session: None,
        layout: Layout::single(),
        view: None,
        req: ViewReq::default(),
    });
    (app, dir, file, rx)
}

/// Asks the stand-in for page `page` as a new job of its work group (what a page key does when a
/// job holds the session: the plugin is opened again).
fn ask(app: &mut App, plugin: &Arc<dyn Plugin>, file: &Path, page: usize) {
    let req = ViewReq {
        page: Some(page),
        ..ViewReq::default()
    };
    let job = PluginJob::Open {
        plugin: Arc::clone(plugin),
        input: Input::File {
            path: file.to_path_buf(),
            hint: None,
        },
    };
    let mut work = app.plugin_work("stand".into(), file.to_path_buf(), job, req);
    // The stand-in is not a registered plugin, so it names its group here, as the manifest of a
    // standard plugin does for the registry.
    work.group = Some(GROUP.into());
    app.dispatch_plugin_work(work, Wait::Nothing);
}

/// Applies worker results until one is current; returns how many arrived.
#[track_caller]
fn land_until_current(app: &mut App, rx: &Receiver<MediaResult>) -> usize {
    let mut arrived = 0;
    loop {
        let res = rx
            .recv_timeout(Duration::from_secs(10))
            .expect("the worker always answers");
        arrived += 1;
        if app.apply_media(res) {
            return arrived;
        }
    }
}

/// Holding a page key asks for a page per key repeat. At most one job of the group runs at any
/// moment, the requests made meanwhile collapse into the newest one, the running job is told to
/// stop (its `Cancel` turns true), and what ends up on screen is the last page asked for.
#[test]
fn twenty_requests_run_one_at_a_time_and_end_on_the_last_one() {
    let (mut app, _dir, file, rx) = app_with_worker();
    let (plugin, seen) = heavy(false);
    ask(&mut app, &plugin, &file, 0);
    for page in 1..=20 {
        ask(&mut app, &plugin, &file, page);
    }
    // The first job runs; the twenty others wait as one request.
    assert_eq!(app.plugin_group_dispatches, 1, "ONE job was started");
    assert!(app.is_media_loading());

    // The running job's answer is for an old request (dropped), then the newest one runs alone.
    let arrived = land_until_current(&mut app, &rx);
    assert_eq!(arrived, 2, "the stale answer, then the last request's");
    assert_eq!(app.plugin_group_dispatches, 2, "the first and the last");
    assert_eq!(seen.views.load(Ordering::SeqCst), 2);
    assert_eq!(
        seen.most_at_once.load(Ordering::SeqCst),
        1,
        "never two reads at the same time"
    );
    assert!(
        seen.cancelled.load(Ordering::SeqCst),
        "the running read was told to stop"
    );
    assert_eq!(app.grid().map(|g| g.nrows()), Some(21), "page 20 is shown");
    assert!(!app.is_media_loading());

    // Nothing is left behind: another request starts a fresh job at once.
    ask(&mut app, &plugin, &file, 3);
    assert_eq!(app.plugin_group_dispatches, 3);
    land_until_current(&mut app, &rx);
    assert_eq!(app.grid().map(|g| g.nrows()), Some(4));
}

/// A job that panics still frees its group: the answer comes back as a failure, and the next
/// request runs (the loading mark and the group can never stick).
#[test]
fn a_panicking_job_frees_its_group() {
    let (mut app, _dir, file, rx) = app_with_worker();
    let (bad, _) = heavy(true);
    ask(&mut app, &bad, &file, 0);
    assert_eq!(land_until_current(&mut app, &rx), 1);
    assert!(app.plugin_failure().is_some());
    assert!(!app.is_media_loading());

    let (good, _) = heavy(false);
    ask(&mut app, &good, &file, 1);
    assert_eq!(app.plugin_group_dispatches, 2, "the group was free again");
    land_until_current(&mut app, &rx);
    assert_eq!(app.grid().map(|g| g.nrows()), Some(2));
}

/// A request waiting behind the running job is dropped, not started, when the preview moved on
/// before the running one ended (the generation changed).
#[test]
fn a_waiting_request_of_a_preview_that_is_gone_is_not_started() {
    let (mut app, _dir, file, rx) = app_with_worker();
    let (plugin, seen) = heavy(false);
    ask(&mut app, &plugin, &file, 0);
    ask(&mut app, &plugin, &file, 1); // waits
    app.clear_image(); // the preview is left: the waiting request is forgotten
    let res = rx.recv_timeout(Duration::from_secs(10)).unwrap();
    assert!(!app.apply_media(res), "the running job's answer is stale");
    assert_eq!(app.plugin_group_dispatches, 1, "the waiting one never ran");
    assert_eq!(seen.views.load(Ordering::SeqCst), 1);
    // The group is free: the next request runs.
    app.plugin = Some(PluginShow {
        id: "stand".into(),
        path: file.clone(),
        phase: PluginPhase::Loading,
        session: None,
        layout: Layout::single(),
        view: None,
        req: ViewReq::default(),
    });
    ask(&mut app, &plugin, &file, 2);
    assert_eq!(app.plugin_group_dispatches, 2);
    land_until_current(&mut app, &rx);
    assert_eq!(app.grid().map(|g| g.nrows()), Some(3));
}

/// A plugin that names no group is not serialised: its jobs start at once, as before.
#[test]
fn a_plugin_without_a_group_starts_every_job() {
    let (mut app, _dir, file, rx) = app_with_worker();
    let (plugin, _) = heavy(false);
    let req = |page| ViewReq {
        page: Some(page),
        ..ViewReq::default()
    };
    for page in 0..2 {
        let job = PluginJob::Open {
            plugin: Arc::clone(&plugin),
            input: Input::File {
                path: file.clone(),
                hint: None,
            },
        };
        let work = app.plugin_work("stand".into(), file.clone(), job, req(page));
        assert_eq!(work.group, None, "the stand-in is not a registered plugin");
        app.dispatch_plugin_work(work, Wait::Nothing);
    }
    // Both ran (the first was cancelled by the second asking, and answers stale).
    assert_eq!(land_until_current(&mut app, &rx), 2);
    assert_eq!(app.plugin_group_dispatches, 0);
}

// ---- the registered plugin ----------------------------------------------------------------

/// The standard `spreadsheet` plugin is in the Office work group, and gives the page the tab is on.
#[test]
fn the_registered_spreadsheet_plugin_names_the_office_group() {
    let dir = unique_tmp("konoma_spreadsheet_group");
    std::fs::create_dir_all(&dir).unwrap();
    let app = App::new(dir.to_path_buf(), Config::default()).unwrap();
    let work = app.plugin_work(
        konoma_office::SPREADSHEET_ID.into(),
        dir.join("b.xlsx"),
        PluginJob::View {
            session: Box::new(NoSession),
        },
        ViewReq::default(),
    );
    assert_eq!(
        work.group.as_deref(),
        Some(konoma_office::OFFICE_WORK_GROUP)
    );
}

struct NoSession;

impl Session for NoSession {
    fn view(&mut self, _req: &ViewReq, _cx: &ViewCx) -> Result<View, Failure> {
        Err(Failure::unsupported(t!(en: "none")))
    }
}

/// `[plugins.spreadsheet] enabled = false`: a workbook is not text, so it is the plain "can not
/// preview", not a sheet (and not an error screen of the plugin).
#[test]
fn a_switched_off_spreadsheet_plugin_leaves_the_file_unclaimed() {
    let dir = unique_tmp("konoma_spreadsheet_off");
    std::fs::create_dir_all(&dir).unwrap();
    konoma_test_support::build_xlsx(
        &dir.join("b.xlsx"),
        &[(
            "S",
            "visible",
            r#"<row r="1"><c r="A1"><v>1</v></c></row>"#,
            "",
        )],
    );
    let book = dir.canonicalize().unwrap().join("b.xlsx");
    let mut on = Config::default();
    on.ui.lang = "en".into();
    assert!(on.resolve_preview(&book).is_spreadsheet());

    let mut cfg = on.clone();
    cfg.plugins.insert(
        "spreadsheet".into(),
        toml::Table::from_iter([("enabled".to_string(), toml::Value::Boolean(false))]),
    );
    match cfg.resolve_preview(&book) {
        PreviewKind::CanNotPreview { ext } => assert_eq!(ext, "xlsx"),
        other => panic!("[can not preview] を期待: {other:?}"),
    }
}

/// A sheet's columns follow `[plugins.spreadsheet] rainbow`, then `[plugins.csv] rainbow`, then the
/// old `[ui] csv_rainbow`.
#[test]
fn the_rainbow_of_a_sheet_follows_its_own_setting_then_csv_then_the_old_ui_one() {
    let rainbow = |cfg: &Config| {
        crate::plugins::effective_settings(cfg, konoma_office::SPREADSHEET_ID).bool("rainbow", true)
    };
    let table =
        |v: bool| toml::Table::from_iter([("rainbow".to_string(), toml::Value::Boolean(v))]);
    // Nothing set: the old `[ui] csv_rainbow` (on by default).
    let mut cfg = Config::default();
    assert!(rainbow(&cfg));
    cfg.ui.csv_rainbow = false;
    assert!(!rainbow(&cfg), "the old [ui] csv_rainbow");
    // `[plugins.csv]` wins over the old name.
    cfg.plugins.insert("csv".into(), table(true));
    assert!(rainbow(&cfg), "[plugins.csv] rainbow");
    // `[plugins.spreadsheet]` wins over both.
    cfg.plugins.insert("spreadsheet".into(), table(false));
    assert!(!rainbow(&cfg), "[plugins.spreadsheet] rainbow");
    cfg.plugins.insert("csv".into(), table(false));
    cfg.plugins.insert("spreadsheet".into(), table(true));
    assert!(rainbow(&cfg));
}

/// The Word, presentation and spreadsheet plugins are one work group: whatever mix of them is
/// asked for, one read runs at a time and only the newest request waits behind it.
#[test]
fn a_word_a_deck_and_a_workbook_share_one_read_at_a_time() {
    let (mut app, _dir, file, rx) = app_with_worker();
    let (plugin, seen) = heavy(false);
    let office = [
        konoma_office::DOCUMENT_ID,
        konoma_office::PRESENTATION_ID,
        konoma_office::SPREADSHEET_ID,
        konoma_office::DOCUMENT_ID,
        konoma_office::PRESENTATION_ID,
    ];
    for id in office {
        let job = PluginJob::Open {
            plugin: Arc::clone(&plugin),
            input: Input::File {
                path: file.clone(),
                hint: None,
            },
        };
        let work = app.plugin_work(id.into(), file.clone(), job, ViewReq::default());
        assert_eq!(
            work.group.as_deref(),
            Some(konoma_office::OFFICE_WORK_GROUP),
            "{id}"
        );
        app.dispatch_plugin_work(work, Wait::Nothing);
    }
    // The first one runs; the four others wait as one request.
    assert_eq!(app.plugin_group_dispatches, 1, "ONE read was started");
    // Answers arrive until the group has nothing running and nothing waiting.
    while app.plugin_groups[konoma_office::OFFICE_WORK_GROUP].busy {
        let res = rx
            .recv_timeout(Duration::from_secs(10))
            .expect("the worker always answers");
        app.apply_media(res);
    }
    assert_eq!(app.plugin_group_dispatches, 2, "the first and the last");
    assert_eq!(seen.views.load(Ordering::SeqCst), 2);
    assert_eq!(
        seen.most_at_once.load(Ordering::SeqCst),
        1,
        "never two reads at the same time"
    );
    assert!(
        seen.cancelled.load(Ordering::SeqCst),
        "the running read was told to stop"
    );
}

/// The old per-kind Office slot is gone: a Word document goes through the plugin road, so it
/// counts in the same dispatch tally as a sheet.
#[test]
fn the_word_plugin_takes_its_reads_from_the_plugin_road() {
    let dir = unique_tmp("konoma_word_group");
    std::fs::create_dir_all(&dir).unwrap();
    let docx = dir.join("w.docx");
    std::fs::write(&docx, b"broken").unwrap();
    let mut app = App::new(dir.to_path_buf(), Config::default()).unwrap();
    let (tx, rx) = channel();
    app.attach_media_loader(tx);
    app.enter_preview(&docx);
    assert!(app.is_document());
    assert_eq!(app.workbook_loads_started(), 1, "one read for the document");
    while app.is_document_loading() {
        let res = rx
            .recv_timeout(Duration::from_secs(10))
            .expect("the worker answers");
        app.apply_media(res);
    }
    // A broken file says why, in the plugin's words.
    assert!(app
        .document_error()
        .is_some_and(|f| f.message.en().starts_with("[document] ")));
}
