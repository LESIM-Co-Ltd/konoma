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

#[path = "chooser_tests.rs"]
mod chooser_tests;
#[path = "global_help_tests.rs"]
mod global_help_tests;
#[path = "idle_resize_tests.rs"]
mod idle_resize_tests;
#[path = "quit_guard_tests.rs"]
mod quit_guard_tests;

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
    setup_with(
        table,
        previews,
        "[ui]\nlang='en'\ndiff_view='source'\nwrap=false\n",
    )
}

fn setup_with(
    table: bool,
    previews: bool,
    config: &str,
) -> (
    konoma_test_support::TmpDir,
    PreparedProjects,
    Terminal<TestBackend>,
) {
    setup_plugin(Arc::new(Project), table, previews, config)
}

fn setup_plugin(
    plugin: Arc<dyn Plugin>,
    table: bool,
    previews: bool,
    config: &str,
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
        vec![plugin],
        if previews {
            vec![Arc::new(FilePlugin(table))]
        } else {
            Vec::new()
        },
        &dir,
        TermCaps::new(),
        (SharedConfig::parse(config).unwrap(), None),
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

/// Keys fed at the code-side idle fence (`ProjectIdleSnapshot::is_idle`: owner replies landed,
/// queued keys replayed, contexts and previews published). A key accepted while a read is in
/// flight is therefore replayed before the run ends, so a test never asserts on a state the
/// loop had not finished reaching. `next` is never asked while fenced.
struct FencedKeys {
    keys: VecDeque<Key>,
    read: usize,
    ended: bool,
}
impl EventSource for FencedKeys {
    fn next(&mut self, _: Option<Duration>) -> Next {
        unreachable!("a fenced source is only asked at the idle fence")
    }
    fn project_fenced(&self) -> bool {
        true
    }
    fn project_idle(&mut self, _: crate::app::project::ProjectIdleSnapshot, _: Buffer) -> Next {
        match self.keys.pop_front() {
            Some(key) => {
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
) -> FencedKeys {
    let (tx, _published) = mpsc::channel();
    prepared.preview_open_fence = Some(tx);
    let mut events = FencedKeys {
        keys: keys.into(),
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
/// The unfenced driver: it ends the moment its keys are fed, WITHOUT waiting for the loop to go
/// idle. Only for a test that deliberately holds a worker (a write, a raw reply, a read) and asserts
/// on the loop while that work is still owed; every other test uses [`drive`], which waits for
/// the idle fence so a queued key is replayed first.
fn drive_held(
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

/// Walks every result kind exactly once. Each arm names the next kind and there is no wildcard
/// arm, so a new `Outcome` variant does not compile until it is placed in the chain (and then in
/// `standalone_notice`). `None` is the write that returned nothing to show.
fn next_outcome(
    current: Option<&Outcome>,
    open_file: &Path,
    root: &Path,
) -> Option<Option<Outcome>> {
    Some(match current {
        None => Some(Outcome::Reload),
        Some(Outcome::Reload) => Some(Outcome::Copy { text: "t".into() }),
        Some(Outcome::Copy { .. }) => Some(Outcome::Flash(crate::Tr::new("m"))),
        Some(Outcome::Flash(_)) => Some(Outcome::Redraw),
        Some(Outcome::Redraw) => Some(Outcome::Unhandled),
        Some(Outcome::Unhandled) => Some(Outcome::PushScreen {
            screen: "log".into(),
            selection: None,
        }),
        Some(Outcome::PushScreen { .. }) => Some(Outcome::PopScreen),
        Some(Outcome::PopScreen) => Some(Outcome::OpenFile {
            path: open_file.into(),
            line: None,
        }),
        Some(Outcome::OpenFile { .. }) => Some(Outcome::OpenDiff {
            path: open_file.into(),
        }),
        Some(Outcome::OpenDiff { .. }) => Some(Outcome::SwitchRoot {
            root: root.into(),
            message: None,
        }),
        Some(Outcome::SwitchRoot { .. }) => Some(Outcome::OpenTab {
            path: open_file.into(),
        }),
        Some(Outcome::OpenTab { .. }) => return None,
    })
}

/// The footer notice the standalone app shows today for each result kind (English).
fn standalone_notice(outcome: Option<&Outcome>) -> Option<String> {
    match outcome {
        // A write: the operation name.
        None | Some(Outcome::Reload) => Some("Diff".into()),
        Some(Outcome::Copy { text }) => Some(konoma_host_core::copy::copied_preview_notice(
            konoma_host_core::lang::Lang::En,
            text,
        )),
        Some(Outcome::Flash(_)) => Some("m".into()),
        // Nothing to name, nothing to say.
        Some(Outcome::Redraw | Outcome::Unhandled) => None,
        // Navigations: the destination answers, so no operation name.
        Some(Outcome::PushScreen { .. } | Outcome::PopScreen) => None,
        Some(Outcome::OpenDiff { .. }) => None,
        // Refused on this surface with its own notice, never the operation name (this fixture has
        // no root switching).
        Some(Outcome::OpenFile { .. } | Outcome::OpenTab { .. } | Outcome::SwitchRoot { .. }) => {
            Some("this action is not supported in the standalone app".into())
        }
    }
}

/// The standalone app takes the host's effect rule for every result kind: a write names the
/// operation in the footer, while a redraw, an unhandled result and every navigation do not.
#[test]
fn standalone_effects_name_the_operation_only_for_writes() {
    let mut outcome = Some(None);
    let mut seen = 0;
    while let Some(current) = outcome {
        seen += 1;
        let (dir, mut prepared, _term) = setup(false, true);
        let _sink = prepared.clipboard_for_test();
        let mut state = ProjectLoopState::new(&mut prepared, false);
        let info = state.info(&prepared, &ProjectLoopCx::default());
        let nav = state.screen.as_ref().unwrap();
        let mut effect = crate::host::project::ProjectEffect::new(
            ProjectOrigin {
                context: info.id,
                project: nav.project.clone(),
                root: info.root.clone(),
                screen: nav.frame().map(|f| f.key.clone()),
                surface: info.surface.clone(),
            },
            "Diff",
        );
        effect.outcome = current.clone();
        let mut update = ProjectUpdate::handled();
        update.events.push(ProjectEvent::Effect(effect));
        prepared.notice = None;
        state.apply(&mut prepared, &info, update);
        assert_eq!(
            prepared.notice,
            standalone_notice(current.as_ref()),
            "{current:?}"
        );
        outcome = next_outcome(current.as_ref(), &dir.join("file.txt"), &dir);
    }
    assert_eq!(seen, 12, "None plus every Outcome variant");
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
fn preview_r_reselects_source_instead_of_the_configured_default() {
    let (_dir, mut prepared, mut terminal) = setup(false, true);
    opening(&mut prepared, &mut terminal, "log");
    diff(&mut prepared);
    drive(&mut prepared, &mut terminal, vec![Key::char('R')]);
    assert!(prepared.file_preview.is_some());
    prepared.config.ui.diff_view = "preview".into();
    run_project_with(
        &mut terminal,
        &mut prepared,
        &mut vec![AppEvent::Key(Key::char('R'))].into_iter(),
        &ProjectLoopCx::default(),
        None,
    )
    .unwrap();
    assert!(prepared.file_preview.is_none(), "R must leave the Preview");
    let diff = prepared.diff.as_ref().expect("return to the opening diff");
    assert_eq!(diff.choice, DiffView::Source, "R explicitly selects Source");
    assert_eq!(diff.effective, DiffView::Source);
    assert!(prepared.screen.is_none());
    assert!(prepared.diff_return.is_some());
}

#[test]
fn preview_r_reopens_raw_diff_even_when_preview_owned_the_unlanded_reply() {
    struct Replies(std::sync::Mutex<VecDeque<FileDiffReply>>);
    impl super::super::ProjectServices for Replies {
        fn inspect(
            &self,
            p: &MatchedProject,
            s: &super::super::ProjectSettings,
        ) -> Result<super::super::ProjectServiceInfo, Failure> {
            Services.inspect(p, s)
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
            Ok(self
                .0
                .lock()
                .unwrap()
                .pop_front()
                .expect("one request per diff entry"))
        }
    }
    let (_dir, mut p, mut terminal) = setup(false, true);
    opening(&mut p, &mut terminal, "log");
    let (stale, first) = mpsc::channel();
    let (fresh, second) = mpsc::channel();
    let services = Arc::new(Replies(std::sync::Mutex::new([first, second].into())));
    p.services = services.clone();
    diff(&mut p);
    // Classification can publish before raw content, retaining the project return already.
    p.diff_return = p.screen.take();
    drive_held(&mut p, &mut terminal, vec![Key::char('R')]);
    assert!(p.file_preview.is_some());
    assert!(
        p.diff.as_ref().unwrap().data.is_none(),
        "the raw reply has not landed before Preview opens"
    );
    fresh
        .send(Ok(FileDiffData {
            lines: vec![DiffLine {
                kind: konoma_vcs_core::git::DiffLineKind::Added,
                old_no: None,
                new_no: Some(1),
                text: "fresh diff".into(),
            }],
            paths: vec![],
        }))
        .unwrap();
    run_project_with(
        &mut terminal,
        &mut p,
        &mut [AppEvent::Key(Key::char('R'))].into_iter(),
        &ProjectLoopCx::default(),
        None,
    )
    .unwrap();
    assert!(
        services.0.lock().unwrap().is_empty(),
        "return to the diff reopens raw ownership"
    );
    assert!(stale
        .send(Err(Failure::external("late old raw reply")))
        .is_err());
    run_project_with(
        &mut terminal,
        &mut p,
        &mut Vec::<AppEvent>::new().into_iter(),
        &ProjectLoopCx::default(),
        None,
    )
    .unwrap();
    assert_eq!(
        p.diff.as_ref().unwrap().data.as_ref().unwrap().lines[0].text,
        "fresh diff",
        "the returned Source displays a real diff"
    );
}

#[test]
fn preview_return_hints_follow_real_keys_and_overlays() {
    for remap in [false, true] {
        let (_dir, mut p, mut terminal) = setup(false, true);
        terminal.backend_mut().resize(160, 14);
        if remap {
            p.config = SharedConfig::parse("[ui]\nlang='en'\ndiff_view='source'\n[keys.text]\nx='cycle_representation'\nR='noop'\n").unwrap();
            p.keys = crate::app::AppKeys::new(&p.config.keys, &p.config.ui.keys);
        }
        opening(&mut p, &mut terminal, "log");
        diff(&mut p);
        drive(&mut p, &mut terminal, vec![Key::char('R')]);
        let key = if remap { 'x' } else { 'R' };
        let text = || {
            crate::app::draw::test_frame()
                .content
                .iter()
                .map(|c| c.symbol())
                .collect::<String>()
        };
        assert!(
            text().contains(&format!("{key}:back to diff")),
            "Preview footer names the executable key"
        );
        let mut run = |keys: Vec<Key>| {
            run_project_with(
                &mut terminal,
                &mut p,
                &mut keys.into_iter().map(AppEvent::Key),
                &ProjectLoopCx::default(),
                None,
            )
            .unwrap();
        };
        if remap {
            run(vec![Key::char('R')]);
            assert!(
                text().contains("x:back to diff"),
                "the old R must leave the Preview unchanged after remap"
            );
        }
        run(vec![Key::char('?'), Key::char(key)]);
        assert!(
            text().contains("back to diff"),
            "help lists the return key of the screen underneath"
        );
        run(vec![Key::char('q'), Key::char(key)]);
        assert!(
            p.file_preview.is_none(),
            "closing help restores the listed return key"
        );
        assert_eq!(p.diff.as_ref().unwrap().choice, DiffView::Source);
        drive(&mut p, &mut terminal, vec![Key::char('R')]);
        let mut run = |keys: Vec<Key>| {
            run_project_with(
                &mut terminal,
                &mut p,
                &mut keys.into_iter().map(AppEvent::Key),
                &ProjectLoopCx::default(),
                None,
            )
            .unwrap();
        };
        run(vec![Key::char(',')]);
        assert!(
            text().contains(&format!("{key}:back to diff")),
            "options list the same return declaration"
        );
        run(vec![Key::char(key)]);
        assert!(
            p.file_preview.is_none(),
            "the advertised menu item returns to the diff"
        );
        assert_eq!(p.diff.as_ref().unwrap().choice, DiffView::Source);
    }
}

#[test]
fn preview_return_key_waits_for_the_table_popup_to_close() {
    for remap in [false, true] {
        let (_dir, mut p, mut terminal) = setup(true, true);
        terminal.backend_mut().resize(160, 14);
        if remap {
            p.config = SharedConfig::parse("[ui]\nlang='en'\ndiff_view='source'\n[keys.table]\nx='cycle_representation'\nR='noop'\n").unwrap();
            p.keys = crate::app::AppKeys::new(&p.config.keys, &p.config.ui.keys);
        }
        opening(&mut p, &mut terminal, "log");
        diff(&mut p);
        drive(&mut p, &mut terminal, vec![Key::char('R')]);
        let key = if remap { 'x' } else { 'R' };
        let token = format!("{key}:back to diff");
        let footer = || {
            let frame = crate::app::draw::test_frame();
            (0..frame.area.width)
                .map(|x| frame[(x, frame.area.bottom() - 1)].symbol())
                .collect::<String>()
        };
        assert!(footer().contains(&token));
        let mut run = |keys: Vec<Key>| {
            run_project_with(
                &mut terminal,
                &mut p,
                &mut keys.into_iter().map(AppEvent::Key),
                &ProjectLoopCx::default(),
                None,
            )
            .unwrap();
        };
        run(vec![Key::new(crate::KeyCode::Enter)]);
        assert!(
            footer().contains("close"),
            "real Enter opens the cell popup"
        );
        assert!(
            !footer().contains(&token),
            "the popup owns the key, so its footer cannot offer return to diff"
        );
        run(vec![Key::char(key)]);
        assert!(
            footer().contains("close"),
            "the return key must leave the popup open"
        );
        run(vec![Key::char('?')]);
        assert!(
            !crate::app::draw::test_frame()
                .content
                .iter()
                .map(|c| c.symbol())
                .collect::<String>()
                .contains("back to diff"),
            "help over a popup cannot list the Preview return key"
        );
        run(vec![Key::char('q')]);
        run(vec![Key::new(crate::KeyCode::Esc)]);
        assert!(
            footer().contains(&token),
            "closing the popup restores the return hint"
        );
        run(vec![Key::char(key)]);
        assert!(
            p.file_preview.is_none(),
            "the restored key returns to the diff"
        );
        assert_eq!(p.diff.as_ref().unwrap().choice, DiffView::Source);
    }
}

#[test]
fn preview_footer_tokens_act_in_popups_options_and_input_capture() {
    use crate::app::{AppKeys, TableMode};
    use konoma_host_core::keymap::{CoreContext, KeyPress};

    fn footer_keys() -> Vec<KeyPress> {
        let frame = crate::app::draw::test_frame();
        let footer = (0..frame.area.width)
            .map(|x| frame[(x, frame.area.bottom() - 1)].symbol())
            .collect::<String>();
        let keys: Vec<_> = footer
            .split_whitespace()
            .filter_map(|s| s.split_once(':'))
            .filter(|(s, _)| !s.is_empty())
            .flat_map(|(s, _)| s.split('/'))
            .map(|s| KeyPress::parse(if s == "↵" { "Enter" } else { s }).unwrap())
            .collect();
        assert!(
            !keys.is_empty(),
            "footer must expose the state's keys: {footer}"
        );
        keys
    }
    fn run(p: &mut PreparedProjects, terminal: &mut Terminal<TestBackend>, keys: Vec<Key>) {
        run_project_with(
            terminal,
            p,
            &mut keys.into_iter().map(AppEvent::Key),
            &ProjectLoopCx::default(),
            None,
        )
        .unwrap();
    }
    let (_dir, mut p, mut terminal) = setup(true, true);
    terminal.backend_mut().resize(240, 14);
    opening(&mut p, &mut terminal, "log");
    diff(&mut p);
    drive(&mut p, &mut terminal, vec![Key::char('R')]);
    run(&mut p, &mut terminal, vec![Key::new(crate::KeyCode::Enter)]);
    let keys = AppKeys::defaults();
    for key in footer_keys() {
        let event = crossterm::event::KeyEvent::new(
            key.code,
            if key.ctrl {
                crossterm::event::KeyModifiers::CONTROL
            } else {
                crossterm::event::KeyModifiers::NONE
            },
        );
        let key = crate::host::typing::key_of(&event).unwrap();
        assert!(
            keys.table_acts_on(
                &key,
                false,
                TableMode {
                    popup: true,
                    cells: true,
                    ..Default::default()
                }
            ),
            "every popup footer key must resolve through the popup handler: {key:?}"
        );
    }
    run(
        &mut p,
        &mut terminal,
        vec![Key::new(crate::KeyCode::Esc), Key::char(',')],
    );
    for key in footer_keys() {
        assert!(
            key == KeyPress::ch('R') || keys.options_op_for_test(CoreContext::Table, key).is_some(),
            "every options footer key must resolve through the options handler: {key:?}"
        );
    }
    run(&mut p, &mut terminal, vec![Key::char('R')]);
    assert!(
        p.file_preview.is_none(),
        "the extra Preview menu item executes"
    );

    struct CapturePlugin;
    impl Plugin for CapturePlugin {
        fn manifest(&self) -> Manifest {
            FilePlugin(false).manifest()
        }
        fn open(&self, _: Input, _: &OpenCx) -> Result<Box<dyn Session>, Failure> {
            Ok(Box::new(Capture(true)))
        }
    }
    struct Capture(bool);
    impl Session for Capture {
        fn view(&mut self, req: &ViewReq, _: &ViewCx) -> Result<View, Failure> {
            Ok(View::Canvas(
                crate::view::CanvasView::new(req.cols, req.rows).text_input(self.0),
            ))
        }
        fn key(&mut self, key: &Key, _: &ActCx) -> Result<Outcome, Failure> {
            if key.code == crate::KeyCode::Esc {
                self.0 = false;
            }
            Ok(Outcome::Redraw)
        }
    }
    let mut registry = crate::registry::Registry::new();
    registry
        .register(
            Arc::new(CapturePlugin),
            crate::registry::PluginOrigin::Standard,
        )
        .unwrap();
    p.file_previews = Some(crate::host::file_preview::FilePreviewSource::new(
        Arc::new(registry),
        Vec::new(),
        false,
    ));
    drive(&mut p, &mut terminal, vec![Key::char('R')]);
    assert_eq!(
        footer_keys(),
        vec![KeyPress::key(crossterm::event::KeyCode::Esc)],
        "capture offers only the host's leave key"
    );
    run(&mut p, &mut terminal, vec![Key::char('R')]);
    assert!(p.file_preview.is_some(), "capture owns the return key");
    run(&mut p, &mut terminal, vec![Key::new(crate::KeyCode::Esc)]);
    assert!(
        !footer_keys()
            .iter()
            .any(|k| k.code == crossterm::event::KeyCode::Esc),
        "the advertised Esc actually leaves capture"
    );
}

#[test]
fn preview_q_releases_each_picture_owner_once_before_late_encode() {
    use konoma_widgets::inline_images::{
        InlineImageCache, MdEncodeKey, MdEncodeResult, MdImgEntry,
    };
    struct Output(Arc<std::sync::Mutex<Vec<u8>>>);
    impl std::io::Write for Output {
        fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(data);
            Ok(data.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let (_dir, mut p, mut terminal) = setup(false, true);
    opening(&mut p, &mut terminal, "log");
    diff(&mut p);
    let output = Arc::new(std::sync::Mutex::new(Vec::new()));
    let picker = ratatui_image::picker::Picker::halfblocks();
    let path = p.open_dir.join("file.txt");
    let mut pictures = Pictures::new(picker.clone(), "preview-file", &path, 128)
        .with_out(Box::new(Output(output.clone())));
    let mut cache = InlineImageCache {
        use_kitty: true,
        ..Default::default()
    };
    let mut tickets = Vec::new();
    let media = PathBuf::from("media-diff://old/hash/1");
    for (i, key) in [
        media.clone(),
        p.open_dir.join("doc.png"),
        PathBuf::from("math://one"),
        PathBuf::from("mermaid-fence://one"),
    ]
    .into_iter()
    .enumerate()
    {
        let entry = MdImgEntry {
            failed: true,
            ..Default::default()
        };
        tickets.push(entry.wish.ticket());
        cache.cache.insert(key.clone(), entry);
        cache.next_request(&key);
        cache
            .kitty_ids
            .insert(key, [vec![71 + i as u32], vec![], vec![]]);
    }
    let born = cache.cache[&media].born;
    let (late_tx, late_rx) = mpsc::channel();
    *pictures.doc_pics() = crate::app::DocPics::new(
        &picker,
        "preview-file",
        &path,
        128,
        Arc::new(crate::app::InProcessSvg),
    )
    .with_cache(cache, late_rx);
    let (published, fence) = mpsc::channel();
    p.preview_open_fence = Some(published);
    struct Departure {
        events: Events,
        tickets: Vec<Arc<konoma_svg::image::DecodeTicket>>,
        late: Option<(mpsc::Sender<MdEncodeResult>, MdEncodeResult)>,
    }
    impl EventSource for Departure {
        fn next(&mut self, wait: Option<Duration>) -> Next {
            let next = self.events.next(wait);
            if matches!(next, Next::End) {
                assert!(
                    self.tickets.iter().all(|t| t.is_stale()),
                    "q cancels every owner before the next project input"
                );
                if let Some((sender, reply)) = self.late.take() {
                    sender.send(reply).unwrap();
                    return Next::Timeout;
                }
            }
            next
        }
    }
    let mut events = Departure {
        events: Events {
            keys: [Key::char('R'), Key::char('q')].into(),
            fence,
            waiting: false,
            read: 0,
            ended: false,
        },
        tickets,
        late: Some((
            late_tx,
            MdEncodeResult {
                path: media,
                key: MdEncodeKey::Full { cols: 4, rows: 2 },
                image: None,
                born,
            },
        )),
    };
    run_project_with(
        &mut terminal,
        &mut p,
        &mut events,
        &ProjectLoopCx::default(),
        Some(pictures),
    )
    .unwrap();
    let output = String::from_utf8(output.lock().unwrap().clone()).unwrap();
    for id in 71..75 {
        assert_eq!(
            output
                .matches(&konoma_widgets::kitty::build_delete(id, false))
                .count(),
            1,
            "each kitty picture is deleted once, including on final drop"
        );
    }
    assert!(p.screen.is_some());
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
    assert_eq!(
        p.diff.as_ref().unwrap().choice,
        DiffView::Preview,
        "failed Preview open preserves the user's choice"
    );
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

/// A chord with Alt is no key of the project layer: the standalone app reads it as nothing, so
/// `Alt-h` does not go back, `Alt-j` does not move, `Alt-l` does not open and `Alt-q` does not
/// quit, while the plain letters do.
#[test]
fn alt_chords_are_keys_of_no_table_on_a_project_screen() {
    let (_dir, mut p, mut term) = setup(false, false);
    opening(&mut p, &mut term, "log");
    let depth = p.screen.as_ref().unwrap().stack.len();
    let cursor = p.screen.as_ref().unwrap().frame().unwrap().cursor;
    let alt = |c: char| Key::char(c).with_mods(crate::KeyMods::ALT);
    let events = drive(
        &mut p,
        &mut term,
        vec![alt('h'), alt('j'), alt('k'), alt('l'), alt('q'), alt('Q')],
    );
    assert!(events.ended, "an Alt chord quit the app");
    let nav = p.screen.as_ref().expect("an Alt chord left the project");
    assert_eq!(
        nav.stack.len(),
        depth,
        "an Alt chord changed the screen stack"
    );
    assert_eq!(
        nav.frame().unwrap().cursor,
        cursor,
        "an Alt chord moved the cursor"
    );
    let events = drive(&mut p, &mut term, vec![Key::char('j')]);
    assert!(events.ended);
    assert_eq!(
        p.screen.as_ref().unwrap().frame().unwrap().cursor,
        cursor + 1,
        "plain j moves"
    );
    drive(&mut p, &mut term, vec![Key::char('h')]);
    assert_eq!(
        p.screen.as_ref().unwrap().stack.len(),
        depth - 1,
        "plain h goes back"
    );
}

/// An Alt chord while the list's `/` filter takes the typing does not reach the filter field:
/// Alt-Backspace keeps the query, Alt-Enter does not confirm it, Alt-Esc does not clear it, and the
/// cursor stays (e4 item 6, the standalone app). The plain Backspace after them still edits it.
#[test]
fn alt_chords_in_the_list_filter_leave_the_filter_alone() {
    let (_dir, mut p, mut term) = setup(false, false);
    opening(&mut p, &mut term, "log");
    p.screen.as_mut().unwrap().frame_mut().unwrap().filtering = true;
    let before = {
        let frame = p.screen.as_ref().unwrap().frame().unwrap();
        (frame.query.clone(), frame.cursor)
    };
    let alt = |code: crate::KeyCode| Key::new(code).with_mods(crate::KeyMods::ALT);
    for code in [
        crate::KeyCode::Backspace,
        crate::KeyCode::Enter,
        crate::KeyCode::Esc,
    ] {
        let events = drive(&mut p, &mut term, vec![alt(code.clone())]);
        assert!(events.ended, "Alt-{code:?} quit the app");
        let frame = p.screen.as_ref().unwrap().frame().unwrap();
        assert!(frame.filtering, "Alt-{code:?} ended the filter");
        assert_eq!(
            (frame.query.clone(), frame.cursor),
            before,
            "Alt-{code:?} changed the filter or the cursor"
        );
    }
    drive(&mut p, &mut term, vec![Key::new(crate::KeyCode::Backspace)]);
    let frame = p.screen.as_ref().unwrap().frame().unwrap();
    assert_eq!(frame.query, "ro", "plain Backspace edits the filter");
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
    drive_held(&mut p, &mut term, vec![Key::char('R')]);
    assert!(p.file_preview.is_some());
    tx.send(Err(Failure::external("raw diff failed"))).unwrap();
    drive_held(&mut p, &mut term, vec![Key::char('q')]);
    assert_screen(
        p.screen.as_ref().expect("Preview keeps its opening screen"),
        &saved,
    );
}

/// A project whose view read waits for the test to release it.
struct HeldProject(Arc<std::sync::atomic::AtomicBool>);
impl Plugin for HeldProject {
    fn manifest(&self) -> Manifest {
        Project.manifest()
    }
    fn open(&self, _: Input, _: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        Ok(Box::new(HeldSession(self.0.clone())))
    }
}
struct HeldSession(Arc<std::sync::atomic::AtomicBool>);
impl Session for HeldSession {
    fn view(&mut self, req: &ViewReq, cx: &ViewCx) -> Result<View, Failure> {
        let start = std::time::Instant::now();
        while self.0.load(std::sync::atomic::Ordering::SeqCst)
            && start.elapsed() < Duration::from_secs(20)
        {
            std::thread::yield_now();
        }
        ProjectSession.view(req, cx)
    }
}

/// Run the loop until the code-side idle fence (owners, replay, contexts all settled), with no new key.
fn settle(p: &mut PreparedProjects, term: &mut Terminal<TestBackend>) {
    super::super::run_project_fenced_for_test(
        term,
        p,
        &ProjectLoopCx::default(),
        None,
        |snapshot, _| {
            assert!(snapshot.is_idle(), "{snapshot:?}");
            Next::End
        },
    )
    .unwrap();
}

fn held_read_setup() -> (
    Arc<std::sync::atomic::AtomicBool>,
    konoma_test_support::TmpDir,
    PreparedProjects,
    Terminal<TestBackend>,
    usize,
) {
    let hold = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (dir, mut p, mut term) = setup_plugin(
        Arc::new(HeldProject(hold.clone())),
        false,
        false,
        "[ui]\nlang='en'\ndiff_view='source'\nwrap=false\n",
    );
    opening(&mut p, &mut term, "log");
    let depth = p.screen.as_ref().unwrap().stack.len();
    (hold, dir, p, term, depth)
}

#[test]
fn a_key_sent_while_the_view_read_is_held_is_kept_and_replayed_after_release() {
    let (hold, _dir, mut p, mut term, depth) = held_read_setup();
    hold.store(true, std::sync::atomic::Ordering::SeqCst);
    let events = drive_held(&mut p, &mut term, vec![Key::char('h')]);
    assert!(events.ended);
    let nav = p.screen.as_ref().unwrap();
    assert_eq!(
        nav.stack.len(),
        depth,
        "h is queued, not applied, while the read is held"
    );
    assert_eq!(nav.frame().unwrap().pending.len(), 1, "the held h is kept");
    hold.store(false, std::sync::atomic::Ordering::SeqCst);
    settle(&mut p, &mut term);
    let nav = p.screen.as_ref().unwrap();
    assert_eq!(
        nav.stack.len(),
        depth - 1,
        "the kept h goes back after release"
    );
}

#[test]
fn keys_sent_while_the_view_read_is_held_wait_in_order_and_replay_after_release() {
    let (hold, _dir, mut p, mut term, depth) = held_read_setup();
    hold.store(true, std::sync::atomic::Ordering::SeqCst);
    drive_held(
        &mut p,
        &mut term,
        vec![Key::char('x'), Key::char('y'), Key::char('h')],
    );
    let nav = p.screen.as_ref().unwrap();
    let kept: Vec<char> = nav
        .frame()
        .unwrap()
        .pending
        .iter()
        .map(|input| match input {
            ProjectInput::Key(key) => match key.code {
                crossterm::event::KeyCode::Char(c) => c,
                other => panic!("unexpected key {other:?}"),
            },
            _ => panic!("only keys were sent"),
        })
        .collect();
    assert_eq!(
        kept,
        ['x', 'y', 'h'],
        "the held keys wait in the order they were sent"
    );
    assert_eq!(
        nav.stack.len(),
        depth,
        "nothing is applied while the read is held"
    );
    hold.store(false, std::sync::atomic::Ordering::SeqCst);
    settle(&mut p, &mut term);
    let nav = p.screen.as_ref().unwrap();
    assert_eq!(
        nav.stack.len(),
        depth - 1,
        "the kept h went back after release"
    );
}

struct JaProject;
impl Plugin for JaProject {
    fn manifest(&self) -> Manifest {
        Manifest::new("preview-project", "0").project(
            ProjectDecl::new()
                .marker(DetectionMarker::new("project.marker", MarkerKind::File))
                .screen(ProjectScreen::new(
                    "changes",
                    crate::t!(en: "Changes", ja: "変更"),
                ))
                .screen(ProjectScreen::new("log", crate::t!(en: "Log", ja: "ログ"))),
        )
    }
    fn open(&self, _: Input, _: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        Ok(Box::new(ProjectSession))
    }
}

/// Every cell a wide symbol covers is blank in the frame the standalone project loop draws: the
/// screen tabs are drawn over the frame's top edge with `Paragraph`, which leaves the `─` behind a
/// wide character, and the buffer diff trusts the buffer to have a blank there.
#[test]
fn the_project_loop_frame_has_a_blank_behind_every_wide_character() {
    use unicode_width::UnicodeWidthStr;
    let (_dir, mut p, mut term) = setup_plugin(
        Arc::new(JaProject),
        false,
        false,
        "[ui]\nlang='ja'\nwrap=false\n",
    );
    opening(&mut p, &mut term, "changes");
    let frame = crate::app::draw::test_frame();
    let (width, height) = (frame.area.width, frame.area.height);
    let mut wide_on_the_edge = 0;
    for y in 0..height {
        for x in 0..width {
            let w = frame[(x, y)].symbol().width() as u16;
            if w < 2 {
                continue;
            }
            if y == 0 || y == 1 {
                wide_on_the_edge += 1;
            }
            for hidden in x + 1..(x + w).min(width) {
                assert_eq!(
                    frame[(hidden, y)].symbol(),
                    " ",
                    "{x},{y}: {:?} covers {hidden},{y}, which holds {:?}",
                    frame[(x, y)].symbol(),
                    frame[(hidden, y)].symbol()
                );
            }
        }
    }
    assert!(
        wide_on_the_edge >= 2,
        "the Japanese screen tabs are drawn on the frame's top edge ({wide_on_the_edge})"
    );
}
