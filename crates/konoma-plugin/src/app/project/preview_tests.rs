//! Exercise the project executor with real keys and publication fences.
use super::*;
use crate::app::diff_screen::{FileDiffData, FileDiffReply, FileDiffRequest};
use crate::app::run_with;
use crate::prelude::*;
use crate::view::{Grid, ListRow, ListView, TableView, TextSource, TextView};
use crate::TermCaps;
use konoma_host_core::project::DiffView;
use konoma_host_core::shared::SharedConfig;
use konoma_vcs_core::git::{ChangeEntry, DiffLine};
use ratatui::{backend::TestBackend, buffer::Buffer};
use std::path::Path;
use std::sync::{mpsc, Arc};

struct Project;
impl Plugin for Project {
    fn manifest(&self) -> Manifest {
        Manifest::new("preview-project", "0").project(
            ProjectDecl::new()
                .marker(DetectionMarker::new("project.marker", MarkerKind::File))
                .screen(ProjectScreen::new("changes", "Changes"))
                .screen(ProjectScreen::new("log", "Log")),
        )
    }
    fn open(&self, _: Input, _: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        Ok(Box::new(ProjectSession))
    }
}
struct ProjectSession;
impl Session for ProjectSession {
    fn view(&mut self, _: &ViewReq, _: &ViewCx) -> Result<View, Failure> {
        Ok(View::List(ListView {
            rows: (0..40)
                .map(|i| {
                    ListRow::new(
                        "file",
                        vec![Line::raw(format!("row {i}: {}", "wide ".repeat(20)))],
                    )
                    .value(i.to_string())
                })
                .collect(),
            ..Default::default()
        }))
    }
}
struct FilePlugin(bool);
impl Plugin for FilePlugin {
    fn manifest(&self) -> Manifest {
        Manifest::new("preview-file", "0")
            .bind(Bind::files(["*.txt"]))
            .shows(ViewKind::Text)
            .file_text()
    }
    fn open(&self, input: Input, _: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        assert_eq!(std::thread::current().name(), Some("project-file-preview"));
        let Input::File { path, .. } = input else {
            unreachable!()
        };
        Ok(Box::new(FileSession(
            self.0,
            std::fs::read_to_string(path)?,
        )))
    }
}
struct FileSession(bool, String);
struct Cells;
impl Grid for Cells {
    fn header(&self, _: usize) -> Option<String> {
        None
    }
    fn nrows(&self) -> usize {
        3
    }
    fn ncols(&self) -> usize {
        2
    }
    fn cell(&self, row: usize, col: usize) -> String {
        format!("cell {row},{col}")
    }
}
impl Session for FileSession {
    fn view(&mut self, _: &ViewReq, _: &ViewCx) -> Result<View, Failure> {
        Ok(if self.0 {
            View::Table(TableView {
                grid: Arc::new(Cells),
                title: None,
                rainbow: false,
                addressed: false,
            })
        } else {
            View::Text(TextView {
                source: TextSource::Owned(self.1.clone()),
                styler: None,
                file_lines: true,
                syntax_name: None,
            })
        })
    }
}
struct Services;
impl super::super::ProjectServices for Services {
    fn inspect(
        &self,
        p: &MatchedProject,
        _: &super::super::ProjectSettings,
    ) -> Result<super::super::ProjectServiceInfo, Failure> {
        Ok(super::super::ProjectServiceInfo {
            workdir: p.root.clone(),
            backend: super::super::ProjectBackend::Git,
        })
    }
    fn file_diff(&self, _: &super::super::ProjectServiceInfo, _: &Path) -> Vec<DiffLine> {
        Vec::new()
    }
    fn changed_files(&self, _: &super::super::ProjectServiceInfo) -> Vec<ChangeEntry> {
        Vec::new()
    }
    fn base_contents(&self, _: &super::super::ProjectServiceInfo, _: &Path) -> Option<Vec<u8>> {
        None
    }
    fn spawn_file_diff(self: Arc<Self>, _: FileDiffRequest) -> Result<FileDiffReply, Failure> {
        let (tx, rx) = mpsc::channel();
        tx.send(Ok(FileDiffData {
            lines: Vec::new(),
            paths: Vec::new(),
        }))
        .unwrap();
        Ok(rx)
    }
}

fn setup(
    table: bool,
    previews: bool,
) -> (
    konoma_test_support::TmpDir,
    PreparedProjects,
    Terminal<TestBackend>,
) {
    let dir = konoma_test_support::unique_tmp("project_preview");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("project.marker"), "").unwrap();
    std::fs::write(dir.join("file.txt"), "ordinary file 日本語\n".repeat(40)).unwrap();
    let prepared = super::super::prepare::prepare_projects_with_previews(
        vec![Arc::new(Project)],
        if previews {
            vec![Arc::new(FilePlugin(table))]
        } else {
            Vec::new()
        },
        &dir,
        TermCaps::new(),
        (
            SharedConfig::parse("[ui]\nlang='en'\ndiff_view='source'\nwrap=false\n").unwrap(),
            None,
        ),
        Arc::new(Services),
    )
    .unwrap_or_else(|e| panic!("{}", e.error));
    (
        dir,
        prepared,
        Terminal::new(TestBackend::new(54, 14)).unwrap(),
    )
}

fn opening(prepared: &mut PreparedProjects, terminal: &mut Terminal<TestBackend>, screen: &str) {
    let mut state = ProjectLoopState::new(prepared, false);
    state.viewport = ProjectViewport { cols: 52, rows: 10 };
    let info = state.info(prepared, &ProjectLoopCx::default());
    let nav = state.screen.as_mut().unwrap();
    nav.stack.push(ProjectFrame::new(ScreenKey::new(screen, 0)));
    nav.frame_mut().unwrap().cursor = 17;
    nav.frame_mut().unwrap().scroll = 12;
    nav.frame_mut().unwrap().hscroll = 7;
    nav.frame_mut().unwrap().query = "row".into();
    nav.frame_mut().unwrap().marked.extend([3, 17, 25]);
    let runtime = state.runtimes.values_mut().next().unwrap();
    runtime.pending = None;
    runtime.busy = false;
    runtime.ready = true;
    let key = state.screen.as_ref().unwrap().frame().unwrap().key.clone();
    runtime
        .contexts
        .insert(key.clone(), request(&info, &key).unwrap().project.unwrap());
    runtime.views.insert(
        key,
        Arc::new(ProjectContent::List(
            match ProjectSession
                .view(&ViewReq::new(52, 10), &ViewCx::bare())
                .unwrap()
            {
                View::List(v) => v,
                _ => unreachable!(),
            },
        )),
    );
    prepared.runtimes = state.runtimes;
    prepared.screen = state.screen;
    run_project_with(
        terminal,
        prepared,
        &mut Vec::<AppEvent>::new().into_iter(),
        &ProjectLoopCx::default(),
        None,
    )
    .unwrap();
}

fn diff(prepared: &mut PreparedProjects) {
    let mut state = ProjectLoopState::new(prepared, false);
    let info = state.info(prepared, &ProjectLoopCx::default());
    let mut update = ProjectUpdate::handled();
    update.commands.push(ProjectCommand::Navigation {
        outcome: Outcome::OpenDiff {
            path: prepared.open_dir.join("file.txt"),
        },
        save_return: true,
    });
    state.apply(prepared, &info, update);
    let diff = state.diff.as_mut().unwrap();
    diff.settle_preview_for_test();
    diff.scroll = 6;
    diff.hscroll = 9;
    prepared.diff = state.diff;
    prepared.screen = state.screen;
    prepared.runtimes = state.runtimes;
}

struct Events {
    keys: VecDeque<Key>,
    fence: mpsc::Receiver<()>,
    waiting: bool,
    read: usize,
    ended: bool,
}
impl EventSource for Events {
    fn next(&mut self, _: Option<Duration>) -> Next {
        if self.waiting {
            self.fence
                .recv_timeout(Duration::from_secs(10))
                .expect("preview publication");
            self.waiting = false;
            return Next::Timeout;
        }
        match self.keys.pop_front() {
            Some(key) => {
                self.waiting = key == Key::char('R');
                self.read += 1;
                Next::Event(AppEvent::Key(key))
            }
            None => {
                self.ended = true;
                Next::End
            }
        }
    }
}
fn drive(
    prepared: &mut PreparedProjects,
    terminal: &mut Terminal<TestBackend>,
    keys: Vec<Key>,
) -> Events {
    let (tx, fence) = mpsc::channel();
    prepared.preview_open_fence = Some(tx);
    let mut events = Events {
        keys: keys.into(),
        fence,
        waiting: false,
        read: 0,
        ended: false,
    };
    run_project_with(
        terminal,
        prepared,
        &mut events,
        &ProjectLoopCx::default(),
        None,
    )
    .unwrap();
    events
}
fn assert_cells(got: &Buffer, want: &Buffer) {
    assert_eq!(got.area, want.area);
    let count = usize::from(got.area.width) * usize::from(got.area.height);
    assert_eq!(got.content.len(), count);
    assert_eq!(want.content.len(), count);
    assert!(got.content.iter().any(|c| c.symbol() != " "));
    for (index, (a, b)) in got.content.iter().zip(&want.content).enumerate() {
        assert_eq!(
            (a.symbol(), a.fg, a.bg, a.modifier),
            (b.symbol(), b.fg, b.bg, b.modifier),
            "cell {index}"
        );
    }
}
fn assert_screen(
    got: &ProjectScreenState<StandaloneInvocation>,
    want: &ProjectScreenState<StandaloneInvocation>,
) {
    assert_eq!(got.project, want.project);
    assert_eq!(got.stack.len(), want.stack.len(), "restore pushed stack");
    for (a, b) in got.stack.iter().zip(&want.stack) {
        assert_eq!(a.key, b.key, "exact opening screen");
        assert_eq!(a.cursor, b.cursor, "restore selection");
        assert_eq!(a.scroll, b.scroll, "restore scroll");
        assert_eq!(a.hscroll, b.hscroll, "restore hscroll");
        assert_eq!(a.query, b.query, "restore query");
        assert_eq!(a.filtering, b.filtering);
        assert_eq!(a.marked, b.marked, "restore marks");
        assert_eq!(a.pending.len(), b.pending.len());
    }
}

#[test]
fn preview_q_restores_exact_opening_project_frame() {
    for screen in ["changes", "log"] {
        let (_dir, mut p, mut term) = setup(false, true);
        opening(&mut p, &mut term, screen);
        let before = crate::app::draw::test_frame();
        let saved = p.screen.clone().unwrap();
        diff(&mut p);
        let events = drive(&mut p, &mut term, vec![Key::char('R'), Key::char('q')]);
        assert_eq!(events.read, 2);
        // Read End after Back instead of queueing input behind the restored view's reply.
        assert!(events.ended, "Back continues the project loop");
        assert!(p.diff_return.is_none(), "one-step return to project");
        assert_screen(p.screen.as_ref().expect("project restored"), &saved);
        assert_cells(&crate::app::draw::test_frame(), &before);
    }
}

#[test]
fn preview_exits_and_failures_in_the_project_app() {
    for keys in [
        vec![Key::char('Q')],
        vec![Key::ctrl('c')],
        vec![Key::char('?'), Key::char('Q')],
        vec![Key::char('?'), Key::ctrl('c')],
    ] {
        let (_dir, mut p, mut t) = setup(false, true);
        opening(&mut p, &mut t, "log");
        diff(&mut p);
        let mut all = vec![Key::char('R')];
        all.extend(keys);
        let expected = all.len();
        all.push(Key::char('q'));
        assert_eq!(
            drive(&mut p, &mut t, all).read,
            expected,
            "Quit stops reading project keys"
        );
        assert!(p.screen.is_none());
    }
    for (table, overlay) in [
        (false, Key::char('?')),
        (true, Key::new(crate::KeyCode::Enter)),
    ] {
        let (_dir, mut p, mut t) = setup(table, true);
        opening(&mut p, &mut t, "log");
        diff(&mut p);
        drive(
            &mut p,
            &mut t,
            vec![Key::char('R'), overlay, Key::char('q')],
        );
        assert!(p.screen.is_none(), "q closes the overlay first");
        assert!(p.file_preview.is_some(), "Preview survives end of events");
        drive(&mut p, &mut t, vec![Key::char('q')]);
        assert!(p.screen.is_some());
    }
    let (_dir, mut p, mut t) = setup(false, false);
    opening(&mut p, &mut t, "log");
    diff(&mut p);
    run_project_with(
        &mut t,
        &mut p,
        &mut [AppEvent::Key(Key::char('R'))].into_iter(),
        &ProjectLoopCx::default(),
        None,
    )
    .unwrap();
    assert_eq!(p.diff.as_ref().unwrap().choice, DiffView::Source);
    assert!(p.file_preview.is_none(), "project-only keeps today's keys");

    let (_dir, mut p, mut t) = setup(false, true);
    opening(&mut p, &mut t, "log");
    let saved = p.screen.clone().unwrap();
    diff(&mut p);
    std::fs::remove_file(p.open_dir.join("file.txt")).unwrap();
    drive(&mut p, &mut t, vec![Key::char('R')]);
    assert!(p.notice.is_some(), "open failure is a diff notice");
    assert!(
        p.diff_return.is_some(),
        "open failure retains return ownership"
    );
    assert!(
        !p.diff.as_ref().unwrap().closed,
        "open failure retains diff"
    );
    drive(&mut p, &mut t, vec![Key::char('q')]);
    assert_screen(p.screen.as_ref().unwrap(), &saved);
}

#[test]
fn project_preview_uses_stage_three_file_runner_cells() {
    let (_dir, mut p, mut term) = setup(false, true);
    opening(&mut p, &mut term, "changes");
    diff(&mut p);
    drive(&mut p, &mut term, vec![Key::char('R')]);
    let preview = p.file_preview.as_ref().expect("Preview was executed");
    let cx = preview.context(&p, &ProjectLoopCx::default());
    let actual = crate::app::draw::test_frame();
    let mut plain = Terminal::new(TestBackend::new(54, 14)).unwrap();
    let text = std::fs::read_to_string(p.open_dir.join("file.txt")).unwrap();
    run_with(
        &mut plain,
        &mut FileSession(false, text),
        &mut Vec::<AppEvent>::new().into_iter(),
        &cx,
        &p.keys,
    )
    .unwrap();
    assert_cells(&actual, &crate::app::draw::test_frame());
}

#[test]
fn preview_escape_and_preview_back_binding_restore_the_project() {
    for (config, back) in [
        ("", Key::new(crate::KeyCode::Esc)),
        ("[keys.text]\nx='preview_back'\n", Key::char('x')),
    ] {
        let (_dir, mut p, mut term) = setup(false, true);
        if !config.is_empty() {
            let keys = SharedConfig::parse(config).unwrap().keys;
            p.config.keys = keys;
            p.keys = crate::app::keys_from_load((p.config.clone(), None)).0;
        }
        opening(&mut p, &mut term, "log");
        let saved = p.screen.clone().unwrap();
        diff(&mut p);
        drive(&mut p, &mut term, vec![Key::char('R'), back]);
        assert_screen(
            p.screen.as_ref().expect("PreviewBack restores project"),
            &saved,
        );
    }
}

#[test]
fn fresh_preview_uses_the_resolved_plain_text_plan_and_resets_file_scroll() {
    let (_dir, mut p, mut term) = setup(false, true);
    // A text rule exercises the internal Text opener; ordinary file apps keep NotHandled.
    let rules = SharedConfig::parse("[[preview.rules]]\nglob='*.txt'\nbuiltin='text'\n")
        .unwrap()
        .preview
        .rules;
    p.file_previews = Some(crate::host::file_preview::FilePreviewSource::new(
        p.registry.clone(),
        rules,
        false,
    ));
    opening(&mut p, &mut term, "changes");
    diff(&mut p);
    drive(
        &mut p,
        &mut term,
        vec![Key::char('R'), Key::char('j'), Key::char('q')],
    );
    diff(&mut p);
    drive(&mut p, &mut term, vec![Key::char('R')]);
    let preview = p.file_preview.as_ref().unwrap();
    assert_eq!(preview.opened.plugin_id, "text");
    let cx = preview.context(&p, &ProjectLoopCx::default());
    let actual = crate::app::draw::test_frame();
    let mut plain = Terminal::new(TestBackend::new(54, 14)).unwrap();
    let text = std::fs::read_to_string(&preview.opened.path).unwrap();
    run_with(
        &mut plain,
        &mut FileSession(false, text),
        &mut Vec::<AppEvent>::new().into_iter(),
        &cx,
        &p.keys,
    )
    .unwrap();
    assert_cells(&actual, &crate::app::draw::test_frame());
}

#[test]
fn configured_preview_opens_without_a_representation_key() {
    let (_dir, mut p, mut term) = setup(false, true);
    opening(&mut p, &mut term, "log");
    let before = crate::app::draw::test_frame();
    let saved = p.screen.clone().unwrap();
    p.config.ui.diff_view = "preview".into();
    diff(&mut p);
    let (tx, fence) = mpsc::channel();
    p.preview_open_fence = Some(tx);
    let mut events = Events {
        keys: [Key::char('q')].into(),
        fence,
        waiting: true,
        read: 0,
        ended: false,
    };
    run_project_with(
        &mut term,
        &mut p,
        &mut events,
        &ProjectLoopCx::default(),
        None,
    )
    .unwrap();
    assert_screen(
        p.screen
            .as_ref()
            .expect("configured Preview returns directly"),
        &saved,
    );
    assert_cells(&crate::app::draw::test_frame(), &before);
}

#[test]
fn preview_resumed_after_end_keeps_its_return_when_a_raw_reply_fails() {
    struct HeldRaw(std::sync::Mutex<Option<FileDiffReply>>);
    impl super::super::ProjectServices for HeldRaw {
        fn inspect(
            &self,
            p: &MatchedProject,
            settings: &super::super::ProjectSettings,
        ) -> Result<super::super::ProjectServiceInfo, Failure> {
            Services.inspect(p, settings)
        }
        fn file_diff(&self, _: &super::super::ProjectServiceInfo, _: &Path) -> Vec<DiffLine> {
            unreachable!()
        }
        fn changed_files(&self, _: &super::super::ProjectServiceInfo) -> Vec<ChangeEntry> {
            unreachable!()
        }
        fn base_contents(&self, _: &super::super::ProjectServiceInfo, _: &Path) -> Option<Vec<u8>> {
            None
        }
        fn spawn_file_diff(self: Arc<Self>, _: FileDiffRequest) -> Result<FileDiffReply, Failure> {
            Ok(self.0.lock().unwrap().take().unwrap())
        }
    }
    let (_dir, mut p, mut term) = setup(false, true);
    opening(&mut p, &mut term, "log");
    let saved = p.screen.clone().unwrap();
    let (tx, rx) = mpsc::channel();
    p.services = Arc::new(HeldRaw(std::sync::Mutex::new(Some(rx))));
    diff(&mut p);
    // Classification can publish before raw content, saving this same return owner.
    p.diff_return = p.screen.take();
    drive(&mut p, &mut term, vec![Key::char('R')]);
    assert!(p.file_preview.is_some());
    tx.send(Err(Failure::external("raw diff failed"))).unwrap();
    drive(&mut p, &mut term, vec![Key::char('q')]);
    assert_screen(
        p.screen.as_ref().expect("Preview keeps its opening screen"),
        &saved,
    );
}
