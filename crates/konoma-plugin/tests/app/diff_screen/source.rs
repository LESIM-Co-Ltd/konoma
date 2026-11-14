use std::collections::VecDeque;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};
use std::time::Duration;

use konoma_host_core::shared::SharedConfig;
use konoma_plugin::app::diff_screen::{FileDiffReply, FileDiffRequest};
use konoma_plugin::app::project::{
    prepare_projects, run_project_with, ProjectBackend, ProjectLoopCx, ProjectServiceInfo,
    ProjectServices, ProjectSettings,
};
use konoma_plugin::app::{AppEvent, EventSource, Next};
use konoma_plugin::host::project::MatchedProject;
use konoma_plugin::prelude::*;
use konoma_plugin::TermCaps;
use konoma_vcs_core::git::{ChangeEntry, DiffLine};
use ratatui::{backend::TestBackend, Terminal};

#[test]
fn fixed_source_keys_use_the_shared_preview_plan() {
    use konoma_host_core::{keymap::KeyPress, project::DiffLayout};
    use konoma_plugin::app::diff_screen::{DiffScreenAction, DiffScreenState};
    let dir = konoma_test_support::unique_tmp("source_fixed_keys");
    std::fs::create_dir_all(&dir).unwrap();
    let (ready, replies) = mpsc::channel();
    let mut state = DiffScreenState::open(
        FileDiffRequest {
            root: dir.to_path_buf(),
            path: dir.join("file.txt"),
            info: ProjectServiceInfo {
                workdir: dir.to_path_buf(),
                backend: ProjectBackend::Git,
            },
        },
        Arc::new(Services(ready, AtomicUsize::new(0))),
        &SharedConfig::parse("[keys.diff]\nn='noop'").unwrap(),
        DiffLayout::Unified,
        None,
    )
    .unwrap();
    replies.recv_timeout(Duration::from_secs(10)).unwrap();
    state.poll().unwrap();
    state.handle_key(KeyPress::parse("Down").unwrap()).unwrap();
    assert_eq!(state.scroll, 1, "fixed Down scrolls Source");
    state.handle_key(KeyPress::ch('n')).unwrap();
    assert_eq!(state.scroll, 1, "a noop must not revive the block key");
    state.handle_key(KeyPress::parse("Home").unwrap()).unwrap();
    assert_eq!(state.scroll, 0);
    assert_eq!(
        state.handle_key(KeyPress::parse("Esc").unwrap()).unwrap().0,
        DiffScreenAction::Back
    );
}

#[test]
fn failed_open_keeps_project_and_reports_the_service_message() {
    struct Failed;
    impl ProjectServices for Failed {
        fn inspect(
            &self,
            _: &MatchedProject,
            _: &ProjectSettings,
        ) -> Result<ProjectServiceInfo, Failure> {
            unreachable!()
        }
        fn file_diff(&self, _: &ProjectServiceInfo, _: &Path) -> Vec<DiffLine> {
            unreachable!()
        }
        fn changed_files(&self, _: &ProjectServiceInfo) -> Vec<ChangeEntry> {
            unreachable!()
        }
        fn base_contents(&self, _: &ProjectServiceInfo, _: &Path) -> Option<Vec<u8>> {
            unreachable!()
        }
        fn spawn_file_diff(self: Arc<Self>, _: FileDiffRequest) -> Result<FileDiffReply, Failure> {
            Err(Failure::external("not a file"))
        }
    }
    let (_dir, mut prepared, mut events, mut terminal) = setup();
    let before = prepared.screen.as_ref().unwrap().frame().unwrap().clone();
    prepared.services = Arc::new(Failed);
    events.keys = [raw_key("Enter")].into();
    run_project_with(
        &mut terminal,
        &mut prepared,
        &mut events,
        &ProjectLoopCx::default(),
        None,
    )
    .unwrap();
    assert!(
        prepared.diff_return.is_none(),
        "save the return screen only after success"
    );
    let after = prepared.screen.as_ref().unwrap().frame().unwrap();
    assert_eq!(
        (&after.key, after.cursor, after.scroll),
        (&before.key, before.cursor, before.scroll)
    );
    assert_eq!(prepared.notice.as_deref(), Some("not a file"));
}

#[test]
fn project_config_keeps_the_unrounded_view_and_shared_settings() {
    use konoma_host_core::project::DiffView;
    for (value, want) in [
        ("source", DiffView::Source),
        ("rendered", DiffView::Rendered),
        ("preview", DiffView::Preview),
        ("unknown", DiffView::Rendered),
    ] {
        let config = SharedConfig::parse(&format!(
            "[ui]\nlang='en'\ndiff_view='{value}'\n[git]\ndiff='split'\n"
        ))
        .unwrap();
        assert_eq!(DiffView::parse(&config.ui.diff_view), want);
        assert_eq!(config.git.diff, "split");
    }
    assert!(SharedConfig::parse("[ui]\ndiff_view=17").is_err());
}

#[test]
fn source_diff_worker_lands_without_input_and_refreshes_with_r() {
    use konoma_host_core::{
        keymap::KeyPress,
        project::{DiffLayout, DiffView},
    };
    use konoma_plugin::app::diff_screen::DiffScreenState;
    let dir = konoma_test_support::unique_tmp("source_raw_worker");
    std::fs::create_dir_all(&dir).unwrap();
    let (ready, replies) = mpsc::channel();
    let services = Arc::new(Services(ready, AtomicUsize::new(0)));
    let request = FileDiffRequest {
        root: dir.to_path_buf(),
        path: dir.join("deleted.txt"),
        info: ProjectServiceInfo {
            workdir: dir.to_path_buf(),
            backend: ProjectBackend::Git,
        },
    };
    let mut state = DiffScreenState::open(
        request,
        services.clone(),
        &SharedConfig::default(),
        DiffLayout::Unified,
        Some(DiffView::Preview),
    )
    .unwrap();
    assert_eq!(
        state.wake_in(),
        Some(Duration::from_millis(16)),
        "raw worker needs its own deadline"
    );
    replies.recv_timeout(Duration::from_secs(10)).unwrap();
    assert!(state.poll().unwrap(), "raw reply lands without a key");
    assert_eq!(state.wake_in(), None);
    assert_eq!(state.choice, DiffView::Preview);
    assert_eq!(state.effective, DiffView::Source);
    state.handle_key(KeyPress::ch('r')).unwrap();
    assert!(
        state.data.is_none(),
        "r drops the raw cache before fetching"
    );
    assert_eq!(state.wake_in(), Some(Duration::from_millis(16)));
    replies.recv_timeout(Duration::from_secs(10)).unwrap();
    assert!(state.poll().unwrap());
    assert_eq!(
        services.1.load(Ordering::SeqCst),
        2,
        "r refetches raw data on the worker"
    );
    let (_dir, mut prepared, mut events, mut terminal) = setup();
    let expected = reference();
    for (index, key) in fixture::REFRESH_KEYS.iter().enumerate() {
        events.keys = [raw_key(key)].into();
        run_project_with(
            &mut terminal,
            &mut prepared,
            &mut events,
            &ProjectLoopCx::default(),
            None,
        )
        .unwrap();
        assert!(
            prepared.diff.as_ref().unwrap().data.is_some(),
            "raw worker must land with no further input"
        );
        assert_eq!(prepared.diff.as_ref().unwrap().wake_in(), None);
        assert_cells(&terminal, &expected["sequences"][2]["stages"][index + 1]);
    }
}

#[path = "source_fixture.rs"]
mod fixture;
use fixture::{SourceProject, FILE_KEYS, SOURCE_KEYS};

struct Services(mpsc::Sender<()>, AtomicUsize);
impl ProjectServices for Services {
    fn inspect(
        &self,
        p: &MatchedProject,
        _: &ProjectSettings,
    ) -> Result<ProjectServiceInfo, Failure> {
        Ok(ProjectServiceInfo {
            workdir: p.root.clone(),
            backend: ProjectBackend::Git,
        })
    }
    fn file_diff(&self, _: &ProjectServiceInfo, _: &Path) -> Vec<DiffLine> {
        assert_eq!(
            std::thread::current().name(),
            Some("file-diff"),
            "raw I/O belongs on the worker"
        );
        self.1.fetch_add(1, Ordering::SeqCst);
        fixture::raw_diff()
    }
    fn changed_files(&self, info: &ProjectServiceInfo) -> Vec<ChangeEntry> {
        ["c.txt", "deleted.txt", "b.txt", "a.txt"]
            .map(|name| ChangeEntry {
                path: info.workdir.join(name),
                status: konoma_vcs_core::git::FileStatus::Modified,
                staged: false,
            })
            .into()
    }
    fn base_contents(&self, _: &ProjectServiceInfo, _: &Path) -> Option<Vec<u8>> {
        None
    }
    fn spawn_file_diff(
        self: Arc<Self>,
        request: FileDiffRequest,
    ) -> Result<FileDiffReply, Failure> {
        let worker = request.spawn(self.clone())?;
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            tx.send(worker.recv().unwrap()).unwrap();
            self.0.send(()).unwrap();
        });
        Ok(rx)
    }
}

struct Events {
    keys: VecDeque<Key>,
    replies: mpsc::Receiver<()>,
    read: usize,
}
impl EventSource for Events {
    fn next(&mut self, wait: Option<Duration>) -> Next {
        if wait == Some(Duration::from_millis(16)) {
            self.replies
                .recv_timeout(Duration::from_secs(10))
                .expect("published worker reply");
            Next::Timeout
        } else {
            self.keys
                .pop_front()
                .map(|k| {
                    self.read += 1;
                    Next::Event(AppEvent::Key(k))
                })
                .unwrap_or(Next::End)
        }
    }
}

fn setup() -> (
    konoma_test_support::TmpDir,
    konoma_plugin::app::project::PreparedProjects,
    Events,
    Terminal<TestBackend>,
) {
    let dir = konoma_test_support::unique_tmp("source_screen");
    let root = dir.join("reference");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("project.marker"), "").unwrap();
    for name in ["a.txt", "b.txt", "c.txt"] {
        std::fs::write(root.join(name), "body").unwrap();
    }
    let (ready, replies) = mpsc::channel();
    let mut prepared = prepare_projects(
        vec![Arc::new(SourceProject)],
        &root,
        TermCaps::new(),
        (
            SharedConfig::parse("[ui]\nlang='en'\ndiff_view='source'\n[git]\ndiff='unified'\n")
                .unwrap(),
            None,
        ),
        Arc::new(Services(ready.clone(), AtomicUsize::new(0))),
    )
    .unwrap_or_else(|e| panic!("{}", e.error));
    for runtime in prepared.runtimes.values_mut() {
        let (tx, rx) = mpsc::sync_channel(1);
        let owner = std::mem::replace(&mut runtime.rx, rx);
        let ready = ready.clone();
        std::thread::spawn(move || {
            while let Ok(reply) = owner.recv() {
                if tx.send(reply).is_err() || ready.send(()).is_err() {
                    break;
                }
            }
        });
    }
    let mut terminal = Terminal::new(TestBackend::new(54, 14)).unwrap();
    let mut events = Events {
        keys: ['j', 'j', 'j', 'j', 'j', 'j', 'j', 'j', 'j', 'j', 'j', 'j']
            .into_iter()
            .map(|c| Key::new(KeyCode::Char(c)))
            .collect(),
        replies,
        read: 0,
    };
    run_project_with(
        &mut terminal,
        &mut prepared,
        &mut events,
        &ProjectLoopCx::default(),
        None,
    )
    .unwrap();
    (dir, prepared, events, terminal)
}

fn raw_key(name: &str) -> Key {
    let (name, mods) = name
        .strip_prefix("ctrl-")
        .map(|s| (s, KeyMods::CTRL))
        .unwrap_or((name, KeyMods::NONE));
    Key::new(match name {
        "Enter" => KeyCode::Enter,
        "Down" => KeyCode::Down,
        "PageDown" => KeyCode::PageDown,
        "PageUp" => KeyCode::PageUp,
        "End" => KeyCode::End,
        "Home" => KeyCode::Home,
        "Right" => KeyCode::Right,
        "Left" => KeyCode::Left,
        "Esc" => KeyCode::Esc,
        _ => KeyCode::Char(name.chars().next().unwrap()),
    })
    .with_mods(mods)
}

fn reference() -> serde_json::Value {
    konoma_plugin::app::project::read_snapshot_for_test("diff_source_screen")
}

fn assert_cells(terminal: &Terminal<TestBackend>, expected: &serde_json::Value) {
    let rect = ratatui::layout::Rect::new(0, 0, 54, 14);
    let buffer = terminal.backend().buffer();
    assert_eq!(buffer.area, rect);
    assert_eq!(expected["count"], 756);
    // The frozen cells are the old hub's drawing; the diff is now inside a frame.
    crate::framed::assert_frozen_stage("diff_source_screen", buffer, rect, expected);
}

#[test]
fn open_diff_source_keys_and_q_restore_project() {
    let (_dir, mut prepared, mut events, mut terminal) = setup();
    let expected = reference();
    assert_eq!(
        expected["sequences"][0]["keys"],
        serde_json::json!(SOURCE_KEYS)
    );
    assert_cells(&terminal, &expected["sequences"][0]["stages"][0]);
    let before = prepared.screen.as_ref().unwrap().frame().unwrap().clone();
    for (index, key) in SOURCE_KEYS.iter().enumerate() {
        events.keys = [raw_key(key)].into();
        run_project_with(
            &mut terminal,
            &mut prepared,
            &mut events,
            &ProjectLoopCx::default(),
            None,
        )
        .unwrap();
        if *key == "Enter" {
            assert_eq!(
                prepared.diff.as_ref().unwrap().choice,
                konoma_host_core::project::DiffView::Source,
                "read the configured view before rounding"
            );
        }
        assert_cells(&terminal, &expected["sequences"][0]["stages"][index + 1]);
    }
    events.keys = [raw_key("k")].into();
    run_project_with(
        &mut terminal,
        &mut prepared,
        &mut events,
        &ProjectLoopCx::default(),
        None,
    )
    .unwrap();
    assert_eq!(
        events.read, 21,
        "q must return and read the next project key"
    );
    let after = prepared
        .screen
        .as_ref()
        .expect("return project screen")
        .frame()
        .unwrap();
    assert_eq!(after.key, before.key, "same return screen");
    assert_eq!(
        after.cursor,
        before.cursor - 1,
        "preserve return row before k"
    );
    assert_eq!(after.scroll, before.scroll, "preserve return scroll");
}

#[test]
fn source_scroll_motions_match_the_host_at_every_frame() {
    let (_dir, mut prepared, mut events, mut terminal) = setup();
    let expected = reference();
    assert_eq!(
        expected["sequences"][3]["keys"],
        serde_json::json!(fixture::MOTION_KEYS)
    );
    for (index, key) in fixture::MOTION_KEYS.iter().enumerate() {
        events.keys = [raw_key(key)].into();
        run_project_with(
            &mut terminal,
            &mut prepared,
            &mut events,
            &ProjectLoopCx::default(),
            None,
        )
        .unwrap();
        assert_cells(&terminal, &expected["sequences"][3]["stages"][index + 1]);
    }
}

#[test]
fn file_navigation_preserves_choice_and_return_screen() {
    use konoma_host_core::{
        keymap::KeyPress,
        project::{DiffLayout, DiffView},
    };
    use konoma_plugin::app::diff_screen::DiffScreenState;
    let dir = konoma_test_support::unique_tmp("source_file_navigation");
    std::fs::create_dir_all(&dir).unwrap();
    for name in ["a.txt", "b.txt", "c.txt"] {
        std::fs::write(dir.join(name), "body").unwrap();
    }
    let (ready, replies) = mpsc::channel();
    let mut state = DiffScreenState::open(
        FileDiffRequest {
            root: dir.to_path_buf(),
            path: dir.join("b.txt"),
            info: ProjectServiceInfo {
                workdir: dir.to_path_buf(),
                backend: ProjectBackend::Git,
            },
        },
        Arc::new(Services(ready, AtomicUsize::new(0))),
        &SharedConfig::default(),
        DiffLayout::Split,
        Some(DiffView::Preview),
    )
    .unwrap();
    replies.recv_timeout(Duration::from_secs(10)).unwrap();
    state.poll().unwrap();
    for (key, name) in [
        ("ctrl-n", "c.txt"),
        ("ctrl-n", "a.txt"),
        ("ctrl-p", "c.txt"),
    ] {
        state.handle_key(KeyPress::parse(key).unwrap()).unwrap();
        assert_eq!(
            state.request.path,
            dir.join(name),
            "sorted existing-file stepping order"
        );
        assert_eq!(
            state.choice,
            DiffView::Preview,
            "carry the chosen view, not the effective Source"
        );
        replies.recv_timeout(Duration::from_secs(10)).unwrap();
        state.poll().unwrap();
    }
    let (_dir, mut prepared, mut events, mut terminal) = setup();
    let expected = reference();
    assert_eq!(
        expected["sequences"][1]["keys"],
        serde_json::json!(FILE_KEYS)
    );
    let before = prepared.screen.as_ref().unwrap().frame().unwrap().clone();
    for (index, key) in FILE_KEYS.iter().enumerate() {
        events.keys = [raw_key(key)].into();
        run_project_with(
            &mut terminal,
            &mut prepared,
            &mut events,
            &ProjectLoopCx::default(),
            None,
        )
        .unwrap();
        assert_cells(&terminal, &expected["sequences"][1]["stages"][index + 1]);
    }
    let after = prepared
        .screen
        .as_ref()
        .expect("file stepping keeps the return screen")
        .frame()
        .unwrap();
    assert_eq!(
        (&after.key, after.cursor, after.scroll),
        (&before.key, before.cursor, before.scroll)
    );
}
