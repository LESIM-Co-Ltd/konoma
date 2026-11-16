use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use konoma_host_core::shared::SharedConfig;
use konoma_plugin::app::diff_screen::{FileDiffReply, FileDiffRequest};
use konoma_plugin::app::project::*;
use konoma_plugin::app::{AppEvent, EventSource, LaunchCx, Next};
use konoma_plugin::host::project::MatchedProject;
use konoma_plugin::prelude::*;
use konoma_plugin::registry::Registry;
use konoma_plugin::TermCaps;
use konoma_plugin::{LaunchMode, Launcher, Verb, VerbDecl};
use konoma_vcs_core::git::{ChangeEntry, DiffLine, DiffLineKind};
use ratatui::{backend::TestBackend, Terminal};
use serde_json::{json, Value};

struct Files;
impl Plugin for Files {
    fn manifest(&self) -> Manifest {
        Manifest::new("launch-files", "1")
            .bind(Bind::files(["*.launch"]))
            .verb(
                VerbDecl::new(Verb::Edit)
                    .launcher(Launcher::Command(vec![
                        "row-editor".into(),
                        "--line={line}".into(),
                    ]))
                    .mode(LaunchMode::Terminal),
            )
            .verb(
                VerbDecl::new(Verb::Open)
                    .launcher(Launcher::Command(vec!["row-opener".into()]))
                    .mode(LaunchMode::Gui),
            )
    }
    fn open(&self, _: Input, _: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        unreachable!()
    }
}

struct Project;
impl Plugin for Project {
    fn manifest(&self) -> Manifest {
        Manifest::new("launch-project", "1")
            .project(
                ProjectDecl::new()
                    .marker(DetectionMarker::new("project.marker", MarkerKind::File))
                    // `!` acts on a version-control project's changes screen only.
                    .version_control()
                    .screen(ProjectScreen::new("changes", "Changes").row_kind("file")),
            )
            .action(
                Act::new("diff")
                    .key("Enter")
                    .project_context(ActContext::screen("changes", ["file"])),
            )
    }
    fn open(&self, input: Input, _: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        let Input::Project { root, .. } = input else {
            unreachable!()
        };
        Ok(Box::new(Rows(root)))
    }
}
struct Rows(PathBuf);
impl Session for Rows {
    fn view(&mut self, _: &ViewReq, _: &ViewCx) -> Result<View, Failure> {
        Ok(View::List(konoma_plugin::view::ListView {
            rows: ["first.launch", "nested/second.launch", "deleted.launch"]
                .into_iter()
                .map(|name| {
                    let row = konoma_plugin::view::ListRow::new("file", vec![Line::raw(name)])
                        .value(name);
                    if name == "deleted.launch" {
                        row
                    } else {
                        row.target(self.0.join(name))
                    }
                })
                .collect(),
            ..Default::default()
        }))
    }
    fn act(&mut self, _: &str, cx: &ActCx) -> Result<Outcome, Failure> {
        Ok(Outcome::OpenDiff {
            path: self.0.join(
                &cx.project
                    .as_ref()
                    .unwrap()
                    .selected
                    .as_ref()
                    .unwrap()
                    .value,
            ),
        })
    }
}

struct Services {
    ready: mpsc::Sender<()>,
    backend: ProjectBackend,
}
impl ProjectServices for Services {
    fn inspect(
        &self,
        p: &MatchedProject,
        _: &ProjectSettings,
    ) -> Result<ProjectServiceInfo, Failure> {
        Ok(ProjectServiceInfo {
            workdir: p.root.clone(),
            backend: self.backend,
        })
    }
    fn file_diff(&self, _: &ProjectServiceInfo, _: &Path) -> Vec<DiffLine> {
        (1..=36)
            .flat_map(|n| {
                let changed = [1, 8, 19, 32].contains(&n);
                let line = |kind, old_no, new_no| DiffLine {
                    kind,
                    old_no,
                    new_no,
                    text: format!("line {n}"),
                };
                changed
                    .then(|| line(DiffLineKind::Removed, Some(n), None))
                    .into_iter()
                    .chain([line(
                        if changed {
                            DiffLineKind::Added
                        } else {
                            DiffLineKind::Context
                        },
                        (!changed).then_some(n),
                        Some(n),
                    )])
            })
            .collect()
    }
    fn changed_files(&self, _: &ProjectServiceInfo) -> Vec<ChangeEntry> {
        vec![]
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
            self.ready.send(()).unwrap();
        });
        Ok(rx)
    }
    fn spawn_launch_target(
        self: Arc<Self>,
        path: PathBuf,
        line: Option<usize>,
        registry: Arc<Registry>,
    ) -> mpsc::Receiver<Option<ProjectLaunchTarget>> {
        let worker = ProjectLaunchTarget::spawn(path, line, self.clone(), registry);
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            tx.send(worker.recv().unwrap()).unwrap();
            self.ready.send(()).unwrap();
        });
        rx
    }
}

struct Events {
    #[cfg(feature = "test-hooks")]
    ready: mpsc::Sender<()>,
    keys: VecDeque<Key>,
    replies: mpsc::Receiver<()>,
    read: usize,
}
impl EventSource for Events {
    fn next(&mut self, wait: Option<Duration>) -> Next {
        if wait == Some(Duration::from_millis(16)) {
            self.replies
                .recv_timeout(Duration::from_secs(10))
                .expect("published reply; launch count must settle");
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
fn key(s: &str) -> Key {
    Key::new(if s == "Enter" {
        KeyCode::Enter
    } else {
        KeyCode::Char(s.chars().next().unwrap())
    })
}
fn reference() -> Value {
    serde_json::from_str(
        &std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../snapshots/project_launch.json"),
        )
        .unwrap(),
    )
    .unwrap()
}
type Log = Arc<Mutex<Vec<Value>>>;
fn setup(
    backend: ProjectBackend,
) -> (
    konoma_test_support::TmpDir,
    PreparedProjects,
    Events,
    Terminal<TestBackend>,
    Log,
    ProjectLoopCx,
) {
    let dir = konoma_test_support::unique_tmp("project_launch");
    std::fs::create_dir_all(dir.join("nested")).unwrap();
    std::fs::write(dir.join("project.marker"), "").unwrap();
    std::fs::write(dir.join("first.launch"), "body").unwrap();
    std::fs::write(dir.join("nested/second.launch"), "body").unwrap();
    let dir = dir.into_canonical().unwrap();
    let (ready, replies) = mpsc::channel();
    let prepared = prepare_projects(vec![Arc::new(Project), Arc::new(Files)], &dir, TermCaps::new(),
        (SharedConfig::parse("[ui]\nlang='en'\ndiff_view='source'\n[git]\ntool='git-tool --git'\ndiff='unified'\n[jj]\ntool='jj-tool --jj'\n[editor]\ncommand='unused-editor'").unwrap(), None),
        Arc::new(Services { ready: ready.clone(), backend })).unwrap_or_else(|e| panic!("{}", e.error));
    let mut prepared = prepared;
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
    let log: Log = Default::default();
    let hand = log.clone();
    let gui = log.clone();
    let root = dir.to_path_buf();
    let gui_root = root.clone();
    let record = move |a: &konoma_plugin::launch::Attempt, cwd: Option<&Path>, root: &Path| {
        let args: Vec<_> = a
            .args
            .iter()
            .map(|s| s.replace(&root.to_string_lossy().to_string(), "$ROOT"))
            .collect();
        json!({"program":a.prog,"args":args,"cwd":cwd.map(|p| p.to_string_lossy().replace(&root.to_string_lossy().to_string(), "$ROOT"))})
    };
    let launch = LaunchCx::new(
        prepared.config.editor.clone(),
        &prepared.config.external,
        vec![],
    )
    .with_hand_off(Arc::new(move |a, cwd, redraw| {
        hand.lock().unwrap().push(record(a, Some(cwd), &root));
        redraw()?;
        Ok(Some(0))
    }))
    .with_runner(Arc::new(move |a| {
        gui.lock().unwrap().push(record(a, None, &gui_root));
        Ok(Some(0))
    }));
    (
        dir,
        prepared,
        Events {
            #[cfg(feature = "test-hooks")]
            ready,
            keys: VecDeque::new(),
            replies,
            read: 0,
        },
        Terminal::new(TestBackend::new(100, 14)).unwrap(),
        log,
        ProjectLoopCx {
            launch: Some(launch),
            ..Default::default()
        },
    )
}

#[test]
fn project_row_and_diff_block_launch_record_target_line_and_cwd() {
    let (_dir, mut prepared, mut events, mut terminal, log, cx) = setup(ProjectBackend::Git);
    let expected = reference();
    events.keys = expected["keys"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| key(s.as_str().unwrap()))
        .collect();
    run_project_with(&mut terminal, &mut prepared, &mut events, &cx, None).unwrap();
    assert_eq!(
        *log.lock().unwrap(),
        expected["launches"].as_array().unwrap().clone(),
        "selected row, block line and launch cwd"
    );
    assert_eq!(events.read, 7);
    events.keys = [key("q"), key("j"), key("e"), key("o")].into();
    run_project_with(&mut terminal, &mut prepared, &mut events, &cx, None).unwrap();
    assert_eq!(
        log.lock().unwrap().len(),
        4,
        "deleted row has neither e nor o"
    );
}

#[test]
fn terminal_handoff_retains_media_before_the_next_input() {
    use konoma_plugin::app::Pictures;
    use konoma_widgets::inline_images::MdImgEntry;
    let (dir, mut prepared, events, mut terminal, log, cx) = setup(ProjectBackend::Git);
    let mut pictures = Pictures::new(
        ratatui_image::picker::Picker::halfblocks(),
        "fixture",
        &dir.join("first.launch"),
        128,
    )
    .with_out(Box::new(std::io::sink()));
    let entry = MdImgEntry {
        failed: true,
        ..Default::default()
    };
    let ticket = entry.wish.ticket();
    pictures
        .doc_pics()
        .cache_for_test()
        .cache
        .insert("media-diff://handoff/old/1".into(), entry);
    struct Check {
        events: Events,
        ticket: Arc<konoma_svg::image::DecodeTicket>,
    }
    impl EventSource for Check {
        fn next(&mut self, wait: Option<Duration>) -> Next {
            let next = self.events.next(wait);
            if matches!(next, Next::End) {
                assert!(
                    !self.ticket.is_stale(),
                    "terminal handoff retains the live media owner, as konoma does"
                );
            }
            next
        }
    }
    let mut events = Check { events, ticket };
    events.events.keys = [Key::char('e')].into();
    run_project_with(
        &mut terminal,
        &mut prepared,
        &mut events,
        &cx,
        Some(pictures),
    )
    .unwrap();
    assert_eq!(log.lock().unwrap().len(), 1, "the terminal launcher ran");
}

#[test]
fn tool_selection_uses_current_backend_workdir_and_gate() {
    // Backend values are injected; no jj installation or tool process is needed.
    for backend in [ProjectBackend::Git, ProjectBackend::Jj] {
        for (tool, enabled) in [
            ("custom-tool --flag  tail", true),
            (" \t  ", true),
            ("ignored", false),
        ] {
            let (dir, mut prepared, mut events, mut terminal, log, cx) = setup(backend);
            // Keep the backend's workdir distinct from the directory the app opened.
            for info in prepared.service_info.values_mut() {
                info.workdir = dir.join("repository");
            }
            let section = if backend == ProjectBackend::Jj {
                "jj"
            } else {
                "git"
            };
            prepared.settings = ProjectSettings::from_config(
                &SharedConfig::parse(&format!(
                    "[external]\ngit_tool={enabled}\n[{section}]\ntool={}\n",
                    serde_json::to_string(tool).unwrap()
                ))
                .unwrap(),
            );
            events.keys = [key("!"), key("Enter"), key("!")].into();
            run_project_with(&mut terminal, &mut prepared, &mut events, &cx, None).unwrap();
            let ran = log.lock().unwrap();
            if enabled {
                let expected = if tool.trim().is_empty() {
                    if backend == ProjectBackend::Jj {
                        "lazyjj"
                    } else {
                        "lazygit"
                    }
                } else {
                    "custom-tool"
                };
                assert_eq!(ran.len(), 2, "tool acts in the project and file diff");
                for launch in ran.iter() {
                    assert_eq!(
                        launch["program"], expected,
                        "current backend and whitespace fallback"
                    );
                    assert_eq!(
                        launch["args"],
                        if tool.trim().is_empty() {
                            json!([])
                        } else {
                            json!(["--flag", "tail"])
                        }
                    );
                    assert_eq!(
                        launch["cwd"], "$ROOT/repository",
                        "tool uses repository workdir"
                    );
                }
            } else {
                assert!(ran.is_empty(), "disabled tool never hands off");
                assert_eq!(
                    prepared.notice.as_deref(),
                    Some("external git tool is disabled (config: [external] git_tool = false)")
                );
            }
        }
    }
}

struct Probe {
    backend: Arc<Mutex<TestBackend>>,
    writes: Arc<Mutex<Vec<usize>>>,
}
impl ratatui::backend::Backend for Probe {
    type Error = std::convert::Infallible;
    fn draw<'a, I>(&mut self, cells: I) -> Result<(), Self::Error>
    where
        I: Iterator<Item = (u16, u16, &'a ratatui::buffer::Cell)>,
    {
        let cells: Vec<_> = cells.collect();
        self.writes.lock().unwrap().push(cells.len());
        self.backend.lock().unwrap().draw(cells.into_iter())
    }
    fn hide_cursor(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
    fn show_cursor(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
    fn get_cursor_position(&mut self) -> Result<ratatui::layout::Position, Self::Error> {
        Ok((0, 0).into())
    }
    fn set_cursor_position<P: Into<ratatui::layout::Position>>(
        &mut self,
        _: P,
    ) -> Result<(), Self::Error> {
        Ok(())
    }
    fn clear(&mut self) -> Result<(), Self::Error> {
        self.backend.lock().unwrap().clear()
    }
    fn clear_region(&mut self, region: ratatui::backend::ClearType) -> Result<(), Self::Error> {
        self.backend.lock().unwrap().clear_region(region)
    }
    fn scroll_region_up(
        &mut self,
        region: std::ops::Range<u16>,
        line_count: u16,
    ) -> Result<(), Self::Error> {
        self.backend
            .lock()
            .unwrap()
            .scroll_region_up(region, line_count)
    }
    fn scroll_region_down(
        &mut self,
        region: std::ops::Range<u16>,
        line_count: u16,
    ) -> Result<(), Self::Error> {
        self.backend
            .lock()
            .unwrap()
            .scroll_region_down(region, line_count)
    }
    fn size(&self) -> Result<ratatui::layout::Size, Self::Error> {
        self.backend.lock().unwrap().size()
    }
    fn window_size(&mut self) -> Result<ratatui::backend::WindowSize, Self::Error> {
        self.backend.lock().unwrap().window_size()
    }
    fn flush(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}
struct Observe<'a> {
    events: Events,
    frame: Arc<Mutex<TestBackend>>,
    writes: Arc<Mutex<Vec<usize>>>,
    stages: Vec<ratatui::buffer::Buffer>,
    expected_flash: &'a str,
}
impl EventSource for Observe<'_> {
    fn next(&mut self, wait: Option<Duration>) -> Next {
        if wait != Some(Duration::from_millis(16)) {
            let buffer = self.frame.lock().unwrap().buffer().clone();
            if self.events.read == 1 {
                use ratatui::widgets::{Paragraph, Widget};
                let mut expected = self.stages[0].clone();
                for x in 0..100 {
                    expected[(x, 13)].reset();
                }
                Paragraph::new(ratatui::text::Line::from(
                    konoma_widgets::footer_line::flash_spans(self.expected_flash),
                ))
                .render(ratatui::layout::Rect::new(0, 13, 100, 1), &mut expected);
                assert_eq!(buffer.area, ratatui::layout::Rect::new(0, 0, 100, 14));
                assert!(buffer
                    .content
                    .iter()
                    .any(|cell| !cell.symbol().trim().is_empty()));
                assert!(*self.writes.lock().unwrap().last().unwrap() > 0);
                let mut compared = 0;
                for (cell, want) in buffer.content.iter().zip(&expected.content) {
                    assert_eq!(
                        (cell.symbol(), cell.fg, cell.bg, cell.modifier),
                        (want.symbol(), want.fg, want.bg, want.modifier),
                        "failure restores every cell and konoma's flash at cell {compared}"
                    );
                    compared += 1;
                }
                assert_eq!(compared, 1400, "full frame comparison");
            }
            self.stages.push(buffer);
        }
        self.events.next(wait)
    }
}

#[test]
fn launch_failure_restores_frame_and_later_keys_still_work() {
    for nonzero in [false, true] {
        let (_dir, mut prepared, mut events, _, _, mut cx) = setup(ProjectBackend::Git);
        let frame = Arc::new(Mutex::new(TestBackend::new(100, 14)));
        let dirty = frame.clone();
        cx.launch.as_mut().unwrap().hand_off = Arc::new(move |_, _, _| {
            use ratatui::backend::Backend;
            let mut backend = dirty.lock().unwrap();
            let cell = ratatui::buffer::Cell::new("X");
            let cell = &cell;
            backend
                .draw((0..14).flat_map(|y| (0..100).map(move |x| (x, y, cell))))
                .unwrap();
            if nonzero {
                Ok(Some(9))
            } else {
                Err(std::io::Error::other("recorded failure"))
            }
        });
        let writes = Arc::new(Mutex::new(vec![]));
        let mut terminal = Terminal::new(Probe {
            backend: frame.clone(),
            writes: writes.clone(),
        })
        .unwrap();
        events.keys = [key("e"), key("j"), key("?"), key("q"), key("q"), key("j")].into();
        let mut observe = Observe {
            events,
            frame,
            writes,
            stages: vec![],
            expected_flash: if nonzero {
                "editor failed: row-editor exited with status 9"
            } else {
                "editor failed: row-editor: recorded failure"
            },
        };
        run_project_with(&mut terminal, &mut prepared, &mut observe, &cx, None).unwrap();
        assert_eq!(
            observe.events.read, 5,
            "q closes help, then exits the project after failed launch"
        );
        assert_eq!(
            prepared.screen.as_ref().unwrap().frame().unwrap().cursor,
            1,
            "j still moves the selected row"
        );
        assert_ne!(observe.stages[1], observe.stages[2], "j changes the frame");
        assert_ne!(observe.stages[2], observe.stages[3], "? opens help");
        assert_eq!(
            observe.stages[2], observe.stages[4],
            "q closes help and restores the selected row"
        );
    }
    #[cfg(feature = "test-hooks")]
    {
        let (_dir, mut prepared, mut events, mut terminal, _, mut cx) = setup(ProjectBackend::Git);
        let published = events.ready.clone();
        let launch = cx.launch.as_mut().unwrap();
        launch.worker = true;
        launch.runner = Arc::new(|_| Err(std::io::Error::other("gui failure")));
        launch.gui_published = Some(Arc::new(move || {
            published.send(()).unwrap();
        }));
        events.keys = [key("o"), key("j"), key("?"), key("q"), key("q"), key("j")].into();
        run_project_with(&mut terminal, &mut prepared, &mut events, &cx, None).unwrap();
        assert_eq!(
            events.read, 5,
            "failed GUI launch count settles before later keys"
        );
        assert_eq!(prepared.screen.as_ref().unwrap().frame().unwrap().cursor, 1);
    }
}
