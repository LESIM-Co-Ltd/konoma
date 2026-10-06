//! Tests added after a mutation audit of the spreadsheet preview: rules of `App` that no key
//! sequence can reach (they are about state the other paths keep from ever existing), so they set
//! that state directly.

use super::*;
use crate::config::Config;
use crate::test_support::unique_tmp;

fn app_with_media_channel() -> (
    App,
    crate::test_support::TmpDir,
    std::sync::mpsc::Receiver<MediaResult>,
) {
    let dir = unique_tmp("konoma_surv_wbq");
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = App::new(dir.to_path_buf(), Config::default()).unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    app.attach_media_loader(tx);
    (app, dir, rx)
}

/// A workbook request waiting behind the running load is started when that load ends — unless the
/// preview moved on since it was asked (the media generation changed): then it is dropped, and the
/// one worker is simply free again. (Leaving a file clears the waiting request itself; this is the
/// second line of defence for any other path that bumps the generation.)
#[test]
fn a_waiting_workbook_request_of_an_old_generation_is_dropped_when_the_load_ends() {
    let (mut app, dir, _rx) = app_with_media_channel();
    let ask = |app: &App| WbRequest {
        path: dir.join("never-there.xlsx"),
        locale: crate::preview::office::Locale::En,
        sheet: 0,
        gen: app.media_gen,
    };

    // Stale: asked under a generation that has since moved on.
    app.wb_worker_busy = true;
    app.wb_queued = Some(ask(&app));
    let finished_gen = app.media_gen;
    app.bump_media_gen();
    let started = app.wb_dispatches;
    let changed = app.apply_media(MediaResult {
        gen: finished_gen,
        wb_worker: true,
        payload: None,
    });
    assert!(!changed, "the finished load is stale too");
    assert_eq!(
        app.wb_dispatches, started,
        "the stale request was not started"
    );
    assert!(!app.wb_worker_busy, "the worker is free");
    assert!(app.wb_queued.is_none(), "and nothing waits any more");

    // Current: asked under the generation in force: started.
    app.wb_worker_busy = true;
    app.wb_queued = Some(ask(&app));
    let started = app.wb_dispatches;
    let finished_gen = app.media_gen.wrapping_sub(1);
    app.apply_media(MediaResult {
        gen: finished_gen,
        wb_worker: true,
        payload: None,
    });
    assert_eq!(
        app.wb_dispatches,
        started + 1,
        "the current request was started"
    );
    assert!(app.wb_worker_busy);
}
