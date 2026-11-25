use std::path::Path;
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use konoma_host_core::project::{DiffLayout, DiffView};
use konoma_host_core::shared::SharedConfig;
use konoma_plugin::app::diff_screen::{
    DiffScreenState, FileDiffData, FileDiffReply, FileDiffRequest,
};
use konoma_plugin::app::project::{
    ProjectBackend, ProjectServiceInfo, ProjectServices, ProjectSettings,
};
use konoma_plugin::host::project::md_diff::{MdDiffOutcome, MdDiffWorker};
use konoma_plugin::host::project::MatchedProject;
use konoma_plugin::Failure;
use konoma_vcs_core::git::{ChangeEntry, DiffLine};
use ratatui::layout::Rect;

const DEADLINE: Duration = Duration::from_secs(10);

struct Services;
impl ProjectServices for Services {
    fn inspect(
        &self,
        project: &MatchedProject,
        _: &ProjectSettings,
    ) -> Result<ProjectServiceInfo, Failure> {
        Ok(ProjectServiceInfo {
            workdir: project.root.clone(),
            backend: ProjectBackend::Git,
        })
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
    fn spawn_file_diff(
        self: Arc<Self>,
        request: FileDiffRequest,
    ) -> Result<FileDiffReply, Failure> {
        let (tx, rx) = mpsc::channel();
        tx.send(Ok(FileDiffData {
            lines: vec![],
            paths: vec![request.path],
        }))
        .unwrap();
        Ok(rx)
    }
}

fn request(root: &Path, name: &str) -> FileDiffRequest {
    FileDiffRequest {
        root: root.into(),
        path: root.join(name),
        info: ProjectServiceInfo {
            workdir: root.into(),
            backend: ProjectBackend::Git,
        },
    }
}

#[test]
fn rendered_diff_waits_then_scrolls_to_first_change_without_a_key() {
    let dir = konoma_test_support::unique_tmp("rendered_wait");
    let old = (0..25)
        .map(|i| format!("paragraph {i}\n\n"))
        .collect::<String>();
    let new = format!("{old}changed\n\n{}", "tail\n\n".repeat(20));
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let release_rx = Mutex::new(release_rx);
    let mut worker = MdDiffWorker::with_compute(Arc::new(move |_| {
        entered_tx.send(()).unwrap();
        release_rx.lock().unwrap().recv().unwrap();
        let (_, _, ops) = konoma_widgets::markdown::diff_align(&old, &new);
        MdDiffOutcome::Ready {
            old_pre: old.clone(),
            new_pre: new.clone(),
            ops,
            any_change: true,
            front_matter_only: false,
        }
    }));
    let (tx, rx) = mpsc::channel();
    let (published_tx, published_rx) = mpsc::channel();
    let (forward_tx, forward_rx) = mpsc::channel();
    worker.attach(tx);
    std::thread::spawn(move || {
        forward_tx.send(rx.recv().unwrap()).unwrap();
        published_tx.send(()).unwrap();
    });
    let mut state = DiffScreenState::open_with_markdown_worker(
        request(&dir, "doc.md"),
        Arc::new(Services),
        &SharedConfig::parse("[ui]\nlang='en'").unwrap(),
        DiffLayout::Unified,
        Some(DiffView::Rendered),
        worker,
        forward_rx,
    )
    .unwrap();
    entered_rx.recv_timeout(DEADLINE).unwrap();
    state.poll().unwrap();
    let area = Rect::new(0, 1, 54, 12);
    state.prepare_frame(area, None);
    assert_eq!(
        state.scroll, 0,
        "computing frame cannot consume first-change reservation"
    );
    assert_eq!(
        state.wake_in(),
        Some(Duration::from_millis(16)),
        "Markdown needs a deadline without pictures"
    );
    let mut prepared = prepared(&dir, state, SharedConfig::parse("[ui]\nlang='en'").unwrap());
    struct Finish {
        release: Option<mpsc::Sender<()>>,
        published: mpsc::Receiver<()>,
    }
    impl konoma_plugin::app::EventSource for Finish {
        fn next(&mut self, wait: Option<Duration>) -> konoma_plugin::app::Next {
            if let Some(release) = self.release.take() {
                assert_eq!(
                    wait,
                    Some(Duration::from_millis(16)),
                    "idle Markdown worker must wake without a key"
                );
                release.send(()).unwrap();
                self.published.recv_timeout(DEADLINE).unwrap();
                konoma_plugin::app::Next::Timeout
            } else {
                konoma_plugin::app::Next::End
            }
        }
    }
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(54, 14)).unwrap();
    konoma_plugin::app::project::run_project_with(
        &mut terminal,
        &mut prepared,
        &mut Finish {
            release: Some(release_tx),
            published: published_rx,
        },
        &Default::default(),
        None,
    )
    .unwrap();
    let state = prepared.diff.as_mut().unwrap();
    assert!(
        state.scroll > 20,
        "real result consumes the reserved first-change scroll"
    );
    assert_eq!(state.wake_in(), None, "landed worker is idle");
    state.scroll = 1;
    state.prepare_frame(area, None);
    assert_eq!(state.scroll, 1, "reservation is consumed exactly once");
}

struct RenderedProject;
impl konoma_plugin::Plugin for RenderedProject {
    fn manifest(&self) -> konoma_plugin::Manifest {
        use konoma_plugin::prelude::*;
        Manifest::new("rendered-project", "1").project(
            ProjectDecl::new()
                .marker(DetectionMarker::new("project.marker", MarkerKind::File))
                .screen(ProjectScreen::new("items", "Items")),
        )
    }
    fn open(
        &self,
        _: konoma_plugin::Input,
        _: &konoma_plugin::OpenCx,
    ) -> Result<Box<dyn konoma_plugin::Session>, Failure> {
        struct Empty;
        impl konoma_plugin::Session for Empty {
            fn view(
                &mut self,
                _: &konoma_plugin::view::ViewReq,
                _: &konoma_plugin::ViewCx,
            ) -> Result<konoma_plugin::view::View, Failure> {
                Ok(konoma_plugin::view::View::List(Default::default()))
            }
        }
        Ok(Box::new(Empty))
    }
}

fn prepared(
    root: &Path,
    state: DiffScreenState,
    config: SharedConfig,
) -> konoma_plugin::app::project::PreparedProjects {
    use konoma_plugin::app::project::prepare_projects;
    use konoma_plugin::host::project::{ProjectScreenState, ScreenKey};
    std::fs::create_dir_all(root).unwrap();
    std::fs::write(root.join("project.marker"), "").unwrap();
    let mut p = prepare_projects(
        vec![Arc::new(RenderedProject)],
        root,
        konoma_plugin::TermCaps::new(),
        (config, None),
        Arc::new(Services),
    )
    .unwrap_or_else(|e| panic!("{}", e.error));
    let project = p.runtimes.keys().next().unwrap().clone();
    p.diff_return = Some(ProjectScreenState::new(
        project,
        ScreenKey {
            tab: 0,
            screen: "items".into(),
            selection: None,
        },
    ));
    p.diff = Some(state);
    p
}

fn fenced_worker(
    compute: impl Fn(&konoma_plugin::host::project::md_diff::MdDiffRequest) -> MdDiffOutcome
        + Send
        + Sync
        + 'static,
) -> (
    MdDiffWorker,
    mpsc::Receiver<konoma_plugin::host::project::md_diff::MdDiffResult>,
    mpsc::Receiver<()>,
) {
    let mut worker = MdDiffWorker::with_compute(Arc::new(compute));
    let (tx, rx) = mpsc::channel();
    worker.attach(tx);
    let (forward, replies) = mpsc::channel();
    let (published, ready) = mpsc::channel();
    std::thread::spawn(move || {
        while let Ok(result) = rx.recv() {
            if forward.send(result).is_err() || published.send(()).is_err() {
                break;
            }
        }
    });
    (worker, replies, ready)
}

#[test]
fn rendered_fallback_keeps_user_choice_across_file_detours() {
    use konoma_host_core::keymap::KeyPress;
    struct Detours;
    impl ProjectServices for Detours {
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
        fn spawn_file_diff(
            self: Arc<Self>,
            request: FileDiffRequest,
        ) -> Result<FileDiffReply, Failure> {
            let (tx, rx) = mpsc::channel();
            tx.send(Ok(FileDiffData {
                lines: vec![],
                paths: ["a.md", "b.txt", "c.md", "d.txt"]
                    .map(|n| request.root.join(n))
                    .into(),
            }))
            .unwrap();
            Ok(rx)
        }
    }
    let dir = konoma_test_support::unique_tmp("rendered_detours");
    let (release, held) = mpsc::channel();
    let held = Mutex::new(held);
    let (worker, rx, published) = fenced_worker(move |req| {
        if req.path.ends_with("a.md") {
            return MdDiffOutcome::Unavailable;
        }
        held.lock().unwrap().recv().unwrap();
        let old_pre = "unchanged\n\n".repeat(20);
        let new_pre = format!("{old_pre}new\n\n{}", "tail\n\n".repeat(20));
        let (_, _, ops) = konoma_widgets::markdown::diff_align(&old_pre, &new_pre);
        MdDiffOutcome::Ready {
            old_pre,
            new_pre,
            ops,
            any_change: true,
            front_matter_only: false,
        }
    });
    let mut state = DiffScreenState::open_with_markdown_worker(
        request(&dir, "a.md"),
        Arc::new(Detours),
        &SharedConfig::parse("[ui]\nlang='en'").unwrap(),
        DiffLayout::Unified,
        Some(DiffView::Rendered),
        worker,
        rx,
    )
    .unwrap();
    let area = Rect::new(0, 1, 54, 12);
    published.recv_timeout(DEADLINE).unwrap();
    state.poll().unwrap();
    assert_eq!(
        state.effective,
        DiffView::Rendered,
        "fallback waits for adoption"
    );
    assert!(state.take_notice().is_none());
    state.prepare_frame(area, None);
    assert_eq!(state.effective, DiffView::Source);
    assert_eq!(
        state.return_info().effective,
        DiffView::Source,
        "Preview return boundary preserves the effective view"
    );
    assert_eq!(
        state.choice,
        DiffView::Rendered,
        "fallback must not overwrite user choice"
    );
    assert_eq!(
        state.take_notice().as_deref(),
        Some("rendered diff unavailable (falling back to source)")
    );
    assert!(state.pending_first_change.is_none());
    state.prepare_frame(area, None);
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(54, 14)).unwrap();
    struct End;
    impl konoma_plugin::app::EventSource for End {
        fn next(&mut self, _: Option<Duration>) -> konoma_plugin::app::Next {
            konoma_plugin::app::Next::End
        }
    }
    let mut p = prepared(&dir, state, SharedConfig::parse("[ui]\nlang='en'").unwrap());
    konoma_plugin::app::project::run_project_with(
        &mut terminal,
        &mut p,
        &mut End,
        &Default::default(),
        None,
    )
    .unwrap();
    let mut state = p.diff.take().unwrap();
    assert!(
        state.take_notice().is_none(),
        "painting cannot repeat the flash"
    );
    for (name, effective) in [
        ("b.txt", DiffView::Source),
        ("c.md", DiffView::Rendered),
        ("d.txt", DiffView::Source),
    ] {
        state.handle_key(KeyPress::ctrl_ch('n')).unwrap();
        state.poll().unwrap();
        assert_eq!(state.request.path, dir.join(name));
        assert_eq!(state.choice, DiffView::Rendered);
        assert_eq!(state.effective, effective);
        assert_eq!(
            state.pending_first_change.as_deref(),
            (effective == DiffView::Rendered).then_some(state.request.path.as_path()),
            "old path reservation must be dropped"
        );
    }
    release.send(()).unwrap();
    published.recv_timeout(DEADLINE).unwrap();
    state.prepare_frame(area, None);
    assert_eq!(state.scroll, 0, "old result cannot scroll the detour");
    state.handle_key(KeyPress::ctrl_ch('p')).unwrap();
    state.poll().unwrap();
    release.send(()).unwrap();
    published.recv_timeout(DEADLINE).unwrap();
    state.prepare_frame(area, None);
    assert_eq!(
        state.effective,
        DiffView::Rendered,
        "Rendered returns after a Source detour"
    );
    assert!(state.scroll > 10);
}

#[path = "rendered_fixture.rs"]
mod fixture;

#[test]
fn rendered_diff_keys_cells_and_picture_crop_match_host() {
    fixture::check_screens();
}
