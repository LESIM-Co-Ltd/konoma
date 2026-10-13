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

/// An Office request waiting behind the running job of the work group is started when that job
/// ends — unless the preview moved on since it was asked (the media generation changed): then it
/// is dropped, and the group is simply free again. (Leaving a file clears the waiting request
/// itself; this is the second line of defence for any other path that bumps the generation.)
#[test]
fn a_waiting_workbook_request_of_an_old_generation_is_dropped_when_the_load_ends() {
    use konoma_plugin::view::ViewReq;
    use konoma_plugin::Input;
    use plugin_host::PluginJob;
    let (mut app, dir, _rx) = app_with_media_channel();
    let path = dir.join("never-there.docx");
    let ask = |app: &App| {
        let job = PluginJob::Open {
            plugin: Arc::new(konoma_office::DocumentPlugin),
            input: Input::File {
                path: path.clone(),
                hint: None,
            },
        };
        let mut work = app.plugin_work(
            konoma_office::DOCUMENT_ID.to_string(),
            path.clone(),
            job,
            ViewReq::default(),
        );
        work.gen = app.media_gen;
        work
    };
    let group = konoma_office::OFFICE_WORK_GROUP;

    // Stale: asked under a generation that has since moved on.
    let work = ask(&app);
    let slot = app.plugin_groups.entry(group.to_string()).or_default();
    slot.busy = true;
    slot.queued = Some(work);
    app.bump_media_gen();
    let started = app.plugin_group_dispatches;
    app.release_plugin_group(group);
    assert_eq!(
        app.plugin_group_dispatches, started,
        "the stale request was not started"
    );
    let slot = &app.plugin_groups[group];
    assert!(!slot.busy, "the group is free");
    assert!(slot.queued.is_none(), "and nothing waits any more");

    // Current: asked under the generation in force: started.
    let work = ask(&app);
    let slot = app.plugin_groups.entry(group.to_string()).or_default();
    slot.busy = true;
    slot.queued = Some(work);
    let started = app.plugin_group_dispatches;
    app.release_plugin_group(group);
    assert_eq!(
        app.plugin_group_dispatches,
        started + 1,
        "the current request was started"
    );
    assert!(app.plugin_groups[group].busy);
}
