use std::collections::VecDeque;
use std::path::Path;
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use konoma_host_core::shared::SharedConfig;
use konoma_plugin::app::project::{
    prepare_projects, run_project_with, PreparedProjects, ProjectBackend, ProjectLoopCx,
    ProjectServiceInfo, ProjectServices, ProjectSettings,
};
use konoma_plugin::app::{AppEvent, EventSource, Next};
use konoma_plugin::host::project::MatchedProject;
use konoma_plugin::prelude::*;
use konoma_plugin::view::{
    DiffHunk, DiffKind, DiffLine as ViewDiffLine, DiffView, DocOptions, DocumentView, ListRow,
    ListStrip, ListView,
};
use konoma_plugin::{Origin, Resources, TermCaps};
use konoma_test_support::TmpDir;
use konoma_vcs_core::git::{ChangeEntry, DiffLine};
use ratatui::{backend::TestBackend, layout::Rect, Terminal};
use serde_json::Value;

#[derive(Default)]
struct Log {
    views: Vec<ViewReq>,
    acts: Vec<ProjectAct>,
    outcome: Option<Outcome>,
    chips: bool,
    refresh: bool,
    prompt: bool,
    confirm: bool,
    choice: bool,
    disabled: bool,
}

struct Screens(Arc<Mutex<Log>>);
impl Plugin for Screens {
    fn manifest(&self) -> Manifest {
        let log = self.0.lock().unwrap();
        let mut action = Act::new("push")
            .key("Enter")
            .writes()
            .project_context(ActContext::screen("items", ["item"]));
        if log.prompt {
            action = action.text_input(TextInput::new("unwired prompt", ""));
        }
        if log.confirm {
            action = action.confirmation(Confirmation::new(
                "Proceed?",
                vec![ConfirmChoice::new('y', "accepted", "Yes")],
            ));
        }
        if log.choice {
            action = action.choice_input(ChoiceInput::new(
                "Target",
                vec![
                    Choice::new("first", "First"),
                    Choice::new("second", "Second"),
                ],
            ));
        }
        if log.refresh {
            action = action.refresh(RefreshTarget::CurrentScreen);
        }
        Manifest::new("project-screens", "1")
            .project(
                ProjectDecl::new()
                    .marker(DetectionMarker::new("project.marker", MarkerKind::File))
                    .screen(
                        ProjectScreen::new("items", "Items")
                            .key('i')
                            .marks(true)
                            .checks(true)
                            .row_kind("item")
                            .row_kind("other"),
                    )
                    .screen(ProjectScreen::new("hidden", "Hidden").pushed())
                    .screen(ProjectScreen::new("text", "Text").key('z'))
                    .screen(ProjectScreen::new("diff", "Diff").key('d'))
                    .screen(ProjectScreen::new("doc", "Document").key('b')),
            )
            .action(action)
            .action(
                Act::new("pop")
                    .key("Enter")
                    .project_context(ActContext::screen("text", Vec::<String>::new())),
            )
    }
    fn open(&self, _: Input, _: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        Ok(Box::new(Screens(self.0.clone())))
    }
}

impl Session for Screens {
    fn enabled(&self, _: &str) -> bool {
        !self.0.lock().unwrap().disabled
    }
    fn view(&mut self, req: &ViewReq, _: &ViewCx) -> Result<View, Failure> {
        let mut log = self.0.lock().unwrap();
        log.views.push(req.clone());
        let changed = log.refresh && !log.acts.is_empty();
        let chips = log.chips;
        let p = req.project.as_ref().unwrap();
        Ok(match p.screen.as_str() {
            "items" => View::List(ListView {
                rows: vec![
                    ListRow::new(
                        "item",
                        vec![Line::from(vec![
                            Span::styled(
                                if changed { "changed" } else { "alpha" },
                                Style::new().fg(Color::Red).bold(),
                            ),
                            Span::styled(" styled", Style::new().fg(Color::Green)),
                        ])],
                    )
                    .value("opaque-alpha")
                    .checked(true)
                    .marked(true),
                    ListRow::new("other", vec![Line::raw("beta")])
                        .value("opaque-beta")
                        .checked(false),
                    ListRow::new("item", vec![Line::raw("gamma")]).value("opaque-gamma"),
                ],
                bottom: chips.then(|| ListStrip::new(vec![Line::raw("legend")], 6)),
                ..Default::default()
            }),
            "text" => View::text([format!(
                "text selection={}",
                p.selection.as_deref().unwrap_or("none")
            )]),
            "doc" => View::Document(DocumentView {
                markdown: "# Project document\n\n**strong body**\n\n- list entry\n".into(),
                origin: Origin::Converted {
                    from: p.tree_root.clone().unwrap(),
                },
                resources: Resources::default(),
                options: DocOptions::default(),
                title: None,
                truncated: false,
            }),
            "diff" => View::Diff(DiffView::new(vec![DiffHunk::new(vec![
                ViewDiffLine {
                    kind: DiffKind::Added,
                    old_no: None,
                    new_no: Some(1),
                    text: "project addition".into(),
                },
                ViewDiffLine {
                    kind: DiffKind::Removed,
                    old_no: Some(1),
                    new_no: None,
                    text: "project removal".into(),
                },
            ])])),
            _ => panic!("unexpected screen {}", p.screen),
        })
    }
    fn act(&mut self, action: &str, cx: &ActCx) -> Result<Outcome, Failure> {
        let p = cx.project.as_ref().unwrap();
        let mut log = self.0.lock().unwrap();
        log.acts.push(p.clone());
        if action == "pop" {
            return Ok(Outcome::PopScreen);
        }
        if let Some(outcome) = &log.outcome {
            return Ok(outcome.clone());
        }
        Ok(Outcome::PushScreen {
            screen: "text".into(),
            selection: p.selected.as_ref().map(|r| r.value.clone()),
        })
    }
}

#[test]
fn list_right_alias_and_real_pop_outcome_use_the_shared_executor() {
    for alias in [KeyCode::Right, KeyCode::Char('l')] {
        let (_dir, mut prepared, mut events, log) = fixture();
        let mut terminal = Terminal::new(TestBackend::new(54, 14)).unwrap();
        events.keys = [AppEvent::Key(Key::new(alias)), key("Enter")].into();
        run_project_with(
            &mut terminal,
            &mut prepared,
            &mut events,
            &ProjectLoopCx::default(),
            None,
        )
        .unwrap();
        assert_eq!(log.lock().unwrap().acts.len(), 2);
        assert_eq!(prepared.screen.as_ref().unwrap().stack.len(), 1);
        assert_cells(&terminal, &reference()["stages"][0]);
    }
}

pub(super) struct Services;
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
        unreachable!()
    }
    fn changed_files(&self, _: &ProjectServiceInfo) -> Vec<ChangeEntry> {
        unreachable!()
    }
    fn base_contents(&self, _: &ProjectServiceInfo, _: &Path) -> Option<Vec<u8>> {
        unreachable!()
    }
}

struct Events {
    keys: VecDeque<AppEvent>,
    replies: mpsc::Receiver<()>,
}
impl EventSource for Events {
    fn next(&mut self, wait: Option<Duration>) -> Next {
        if wait.is_some() {
            self.replies
                .recv_timeout(Duration::from_secs(10))
                .expect("an actual owner reply, not a sleep");
            Next::Timeout
        } else {
            self.keys.pop_front().map(Next::Event).unwrap_or(Next::End)
        }
    }
}

fn fixture_with(log: Log) -> (TmpDir, PreparedProjects, Events, Arc<Mutex<Log>>) {
    let dir = konoma_test_support::unique_tmp("project_navigation");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("project.marker"), "").unwrap();
    let dir = dir.into_canonical().unwrap();
    let log = Arc::new(Mutex::new(log));
    let mut prepared = prepare_projects(
        vec![Arc::new(Screens(log.clone()))],
        &dir,
        TermCaps::new(),
        (SharedConfig::parse("[ui]\nlang='en'\n").unwrap(), None),
        Arc::new(Services),
    )
    .unwrap_or_else(|e| panic!("{}", e.error));
    let (ready, replies) = mpsc::channel();
    for runtime in prepared.runtimes.values_mut() {
        let (tx, rx) = mpsc::sync_channel(1);
        let owner = std::mem::replace(&mut runtime.rx, rx);
        let ready = ready.clone();
        // Publish the real reply before releasing the event-source fence. This cannot race
        // the UI's try_recv, and never injects navigation state or an invented outcome.
        std::thread::spawn(move || {
            while let Ok(reply) = owner.recv() {
                if tx.send(reply).is_err() || ready.send(()).is_err() {
                    break;
                }
            }
        });
    }
    (
        dir,
        prepared,
        Events {
            keys: VecDeque::new(),
            replies,
        },
        log,
    )
}

fn fixture() -> (TmpDir, PreparedProjects, Events, Arc<Mutex<Log>>) {
    fixture_with(Log::default())
}

fn reference() -> Value {
    serde_json::from_str(
        &std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../snapshots/project_navigation.json"),
        )
        .unwrap(),
    )
    .unwrap()
}

fn key(name: &str) -> AppEvent {
    AppEvent::Key(Key::new(match name {
        "Tab" => KeyCode::Tab,
        "BackTab" => KeyCode::BackTab,
        "Enter" => KeyCode::Enter,
        _ => KeyCode::Char(name.chars().next().unwrap()),
    }))
}

fn assert_cells(terminal: &Terminal<TestBackend>, expected: &Value) {
    let rect = Rect::new(0, 1, 54, 12);
    assert_eq!(
        expected["rect"],
        serde_json::json!([rect.x, rect.y, rect.width, rect.height])
    );
    let cells: Vec<_> = expected["runs"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|run| std::iter::repeat_n(&run[1], run[0].as_u64().unwrap() as usize))
        .collect();
    assert_eq!(cells.len(), 648);
    assert_eq!(expected["count"], cells.len());
    let buffer = terminal.backend().buffer();
    let mut compared = 0;
    let mut nonempty = 0;
    for y in rect.y..rect.bottom() {
        for x in rect.x..rect.right() {
            let actual = &buffer[(x, y)];
            assert_eq!(
                serde_json::json!([
                    actual.symbol(),
                    format!("{:?}", actual.fg),
                    format!("{:?}", actual.bg),
                    actual.modifier.bits()
                ]),
                *cells[compared],
                "cell ({x}, {y})"
            );
            nonempty += usize::from(!actual.symbol().trim().is_empty());
            compared += 1;
        }
    }
    assert_eq!(compared, 648);
    assert!(nonempty > 0);
}

#[test]
fn project_loop_runs_host_screen_navigation_sequence() {
    let reference = reference();
    for end in 0..=6 {
        let (_dir, mut prepared, mut events, log) = fixture();
        events.keys = reference["keys"]
            .as_array()
            .unwrap()
            .iter()
            .take(end)
            .map(|k| key(k.as_str().unwrap()))
            .collect();
        let mut terminal = Terminal::new(TestBackend::new(54, 14)).unwrap();
        run_project_with(
            &mut terminal,
            &mut prepared,
            &mut events,
            &ProjectLoopCx::default(),
            None,
        )
        .unwrap();
        assert!(
            events.keys.is_empty(),
            "every key through stage {end} must be read"
        );
        let expected = &reference["stages"][end];
        let frame = prepared.screen.as_ref().unwrap().frame().unwrap();
        assert_eq!(frame.key.screen, expected["screen"]);
        assert_eq!(
            serde_json::json!(frame.key.selection),
            expected["selection"]
        );
        assert_eq!(frame.cursor, expected["cursor"]);
        assert_cells(&terminal, expected);
        if end >= 5 {
            assert_eq!(
                log.lock().unwrap().acts[0].selected.as_ref().unwrap().value,
                "opaque-alpha"
            );
        }
    }
}

fn run_keys(
    terminal: &mut Terminal<TestBackend>,
    prepared: &mut PreparedProjects,
    events: &mut Events,
    names: &[&str],
) {
    events.keys = names.iter().map(|k| key(k)).collect();
    run_project_with(terminal, prepared, events, &ProjectLoopCx::default(), None).unwrap();
}

#[test]
fn project_root_q_exits_and_pushed_q_restores_the_same_frame() {
    let (_dir, mut prepared, mut events, log) = fixture();
    let mut terminal = Terminal::new(TestBackend::new(54, 14)).unwrap();
    run_keys(
        &mut terminal,
        &mut prepared,
        &mut events,
        &["/", "a", "Enter", "j", "j", "V"],
    );
    let before = terminal.backend().buffer().clone();
    let frame = prepared.screen.as_ref().unwrap().frame().unwrap();
    assert_eq!(
        (
            frame.cursor,
            frame.query.as_str(),
            frame.marked.iter().copied().collect::<Vec<_>>()
        ),
        (2, "a", vec![0, 2])
    );
    run_keys(&mut terminal, &mut prepared, &mut events, &["Enter", "q"]);
    let frame = prepared.screen.as_ref().unwrap().frame().unwrap();
    assert_eq!(frame.key.screen, "items");
    assert_eq!(
        (
            frame.cursor,
            frame.query.as_str(),
            frame.marked.iter().copied().collect::<Vec<_>>()
        ),
        (2, "a", vec![0, 2])
    );
    assert_eq!(
        terminal.backend().buffer(),
        &before,
        "pop must restore every cell of the same frame"
    );
    assert_eq!(
        log.lock()
            .unwrap()
            .acts
            .last()
            .unwrap()
            .selected
            .as_ref()
            .unwrap()
            .value,
        "opaque-gamma"
    );
    run_keys(&mut terminal, &mut prepared, &mut events, &["q", "j"]);
    assert_eq!(
        events.keys.len(),
        1,
        "root q exits before reading the later j"
    );
}

#[test]
fn project_resize_draws_shared_content_and_adopts_clamps() {
    let (_dir, mut prepared, mut events, log) = fixture();
    let mut terminal = Terminal::new(TestBackend::new(32, 8)).unwrap();
    run_keys(&mut terminal, &mut prepared, &mut events, &["z"]);
    let frame = prepared.screen.as_mut().unwrap().frame_mut().unwrap();
    frame.cursor = 999;
    frame.scroll = 999;
    frame.hscroll = 999;
    terminal.backend_mut().resize(54, 14);
    events.keys = [AppEvent::Resize].into();
    run_project_with(
        &mut terminal,
        &mut prepared,
        &mut events,
        &ProjectLoopCx::default(),
        None,
    )
    .unwrap();
    let frame = prepared.screen.as_ref().unwrap().frame().unwrap();
    assert_eq!(
        (frame.cursor, frame.scroll, frame.hscroll),
        (0, 0, 0),
        "draw corrections must be adopted"
    );
    assert_cells(&terminal, &reference()["stages"][1]);
    let req = log.lock().unwrap().views.last().unwrap().clone();
    assert_eq!((req.cols, req.rows), (52, 10));

    run_keys(&mut terminal, &mut prepared, &mut events, &["d"]);
    assert_cells(&terminal, &reference()["diff"]);
    log.lock().unwrap().chips = true;
    run_keys(&mut terminal, &mut prepared, &mut events, &["i", "r"]);
    assert_cells(&terminal, &reference()["chips"]);
    assert!(
        log.lock()
            .unwrap()
            .views
            .iter()
            .rev()
            .take(2)
            .all(|req| (req.cols, req.rows) == (52, 10)),
        "worker viewport precedes the chip reservation"
    );
}

#[test]
fn project_outcomes_and_their_refresh_are_adopted() {
    use konoma_host_core::clipboard::{clear_test_clipboard, get_test_clipboard};
    for outcome in [
        Outcome::Copy {
            text: "copy must not run".into(),
        },
        Outcome::Flash("flash must not appear".into()),
        Outcome::OpenFile {
            path: "other.txt".into(),
            line: None,
        },
        Outcome::OpenTab {
            path: "other.txt".into(),
        },
        Outcome::OpenDiff {
            path: "other.txt".into(),
        },
        Outcome::SwitchRoot {
            root: "elsewhere".into(),
            message: None,
        },
    ] {
        clear_test_clipboard();
        let expected_copy = if let Outcome::Copy { text } = &outcome {
            Some(text.clone())
        } else {
            None
        };
        let expected_flash = if let Outcome::Flash(message) = &outcome {
            Some(message.get("en").to_string())
        } else {
            None
        };
        let (_dir, mut prepared, mut events, log) = fixture_with(Log {
            outcome: Some(outcome),
            refresh: true,
            ..Default::default()
        });
        let mut terminal = Terminal::new(TestBackend::new(54, 14)).unwrap();
        run_keys(&mut terminal, &mut prepared, &mut events, &["Enter"]);
        assert_eq!(log.lock().unwrap().acts.len(), 1);
        assert_eq!(get_test_clipboard(), expected_copy);
        if let Some(flash) = expected_flash {
            assert_eq!(prepared.notice.as_deref(), Some(flash.as_str()));
        }
        assert!(prepared.notice.is_some());
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(text.contains("changed"), "outcome refresh must be visible");
        assert_eq!(prepared.screen.as_ref().unwrap().stack.len(), 1);
        assert!(prepared.dialog.is_none());
    }
}

#[test]
fn prompts_run_after_answers_while_guarded_acts_and_unwired_keys_stay_inert() {
    for mode in 0..6 {
        let (_dir, mut prepared, mut events, log) = fixture_with(Log {
            prompt: mode == 0,
            confirm: mode == 4,
            choice: mode == 5,
            disabled: mode == 1,
            ..Default::default()
        });
        if mode == 2 {
            prepared
                .config
                .plugins
                .entry("project-screens".into())
                .or_default()
                .insert("read_only".into(), true.into());
        }
        if mode == 3 {
            prepared.config.keys.surfaces.insert(
                "project-screens".into(),
                [("Enter".into(), "noop".into())].into(),
            );
        }
        let mut terminal = Terminal::new(TestBackend::new(54, 14)).unwrap();
        events.keys = [
            key("?"),
            key("?"),
            key("y"),
            key(" "),
            key("e"),
            key("o"),
            key("!"),
            AppEvent::Key(Key::ctrl('t')),
            AppEvent::Key(Key::new(KeyCode::F(2))),
            key("Enter"),
            AppEvent::Key(Key::new(KeyCode::Right)),
        ]
        .into();
        run_project_with(
            &mut terminal,
            &mut prepared,
            &mut events,
            &ProjectLoopCx::default(),
            None,
        )
        .unwrap();
        assert!(
            log.lock().unwrap().acts.is_empty(),
            "guard/prompt mode {mode}"
        );
        if matches!(mode, 0 | 4 | 5) {
            use konoma_plugin::host::project::{ProjectDialog, ProjectPrompt};
            let Some(ProjectDialog::Act {
                prompts,
                stage,
                buffer,
                ..
            }) = &prepared.dialog
            else {
                panic!("prompt must open: {mode}");
            };
            match mode {
                0 => {
                    assert!(
                        matches!(&prompts[*stage], ProjectPrompt::Text(p) if p.label.get("en") == "unwired prompt")
                    );
                    assert!(buffer.is_empty());
                }
                4 => assert!(
                    matches!(&prompts[*stage], ProjectPrompt::Confirmation(p) if p.text.get("en") == "Proceed?")
                ),
                5 => assert!(
                    matches!(&prompts[*stage], ProjectPrompt::Choice(p) if p.label.get("en") == "Target")
                ),
                _ => unreachable!(),
            }
            let answers: &[&str] = match mode {
                0 => &["追", "Enter"],
                4 => &["y"],
                _ => &["j", "Enter"],
            };
            run_keys(&mut terminal, &mut prepared, &mut events, answers);
            let acts = &log.lock().unwrap().acts;
            assert_eq!(acts.len(), 1, "act runs only after its prompt is answered");
            match mode {
                0 => assert_eq!(acts[0].text.as_deref(), Some("追")),
                4 => assert_eq!(acts[0].confirmation.as_deref(), Some("accepted")),
                _ => assert_eq!(acts[0].choice.as_deref(), Some("second")),
            }
        }
        assert!(prepared.dialog.is_none());
        assert!(prepared
            .screen
            .as_ref()
            .unwrap()
            .frame()
            .unwrap()
            .pending
            .is_empty());
        assert_cells(
            &terminal,
            &reference()["stages"][if matches!(mode, 0 | 4 | 5) { 5 } else { 0 }],
        );
        let header = "                                    Project   path:rel";
        let footer = if mode == 2 {
            "this plugin is set to read-only ([plugins.<id>] read_only = true)"
        } else {
            ""
        };
        for (y, text) in [(0, header), (13, footer)] {
            for (x, symbol) in text
                .chars()
                .chain(std::iter::repeat(' '))
                .take(54)
                .enumerate()
            {
                use ratatui::style::{Color, Modifier};
                let cell = &terminal.backend().buffer()[(x as u16, y)];
                assert_eq!(
                    cell.symbol(),
                    symbol.to_string(),
                    "chrome cell ({x}, {y}), mode {mode}"
                );
                let chip = y == 0 && (35..44).contains(&x);
                assert_eq!(cell.fg, if chip { Color::Black } else { Color::Reset });
                assert_eq!(cell.bg, if chip { Color::Cyan } else { Color::Reset });
                assert_eq!(
                    cell.modifier,
                    if chip || y == 13 && !footer.is_empty() {
                        Modifier::BOLD
                    } else if y == 0 && x >= 44 {
                        Modifier::DIM
                    } else {
                        Modifier::empty()
                    }
                );
            }
        }
    }
}
