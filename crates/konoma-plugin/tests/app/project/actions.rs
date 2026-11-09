use std::collections::VecDeque;
use std::path::Path;
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use konoma_host_core::shared::SharedConfig;
use konoma_plugin::app::project::{
    prepare_projects, run_project_with, PreparedProjects, ProjectLoopCx,
};
use konoma_plugin::app::{AppEvent, EventSource, Next};
use konoma_plugin::host::project::{ProjectDialog, ProjectPrompt};
use konoma_plugin::prelude::*;
use konoma_plugin::TermCaps;
use konoma_test_support::TmpDir;
use ratatui::{backend::TestBackend, layout::Rect, Terminal};
use serde_json::{json, Value};

#[derive(Default)]
struct Log {
    acts: Vec<(String, ProjectAct)>,
    hold: Option<Hold>,
    act_hold: Option<Hold>,
    disabled: bool,
    omit_alpha: bool,
    failures: usize,
    view_failure: bool,
    watch_failure: bool,
    outcome: Option<Outcome>,
    refresh_content: bool,
}

struct Hold {
    entered: mpsc::Sender<()>,
    release: mpsc::Receiver<()>,
}
fn held() -> (Hold, mpsc::Receiver<()>, mpsc::Sender<()>) {
    let (entered, received) = mpsc::channel();
    let (release, released) = mpsc::channel();
    (
        Hold {
            entered,
            release: released,
        },
        received,
        release,
    )
}

struct Actions(Arc<Mutex<Log>>);
impl Plugin for Actions {
    fn manifest(&self) -> Manifest {
        Manifest::new("project-screens", "1")
            .project(
                ProjectDecl::new()
                    .marker(DetectionMarker::new("project.marker", MarkerKind::File))
                    .screen(
                        ProjectScreen::new("items", "Items")
                            .marks(true)
                            .checks(true)
                            .key('i')
                            .row_kind("item")
                            .row_kind("other"),
                    )
                    .screen(ProjectScreen::new("text", "Text").key('z'))
                    .screen(ProjectScreen::new("diff", "Diff").key('d'))
                    .screen(ProjectScreen::new("doc", "Document").key('b')),
            )
            .action(
                Act::new("retry-confirmed")
                    .key("ctrl-F6")
                    .project_context(ActContext::screen("items", ["item"]))
                    .confirmation(Confirmation::new(
                        "Confirm retry?",
                        vec![ConfirmChoice::new('y', "yes", "Yes")],
                    ))
                    .text_input(TextInput::new("Retry message", "seed").keep_on_failure()),
            )
            .action(
                Act::new("retry-refresh")
                    .key("ctrl-F7")
                    .project_context(ActContext::screen("items", ["item"]))
                    .text_input(TextInput::new("Retry message", "seed").keep_on_failure())
                    .refresh(RefreshTarget::CurrentScreen),
            )
            .action(
                Act::new("answers")
                    .key("F5")
                    .project_context(ActContext::screen("items", ["item"]))
                    .confirmation(Confirmation::new(
                        "Proceed?",
                        vec![ConfirmChoice::new('y', "accepted", "Yes")],
                    ))
                    .text_input(TextInput::new("Name", "seed"))
                    .choice_input(ChoiceInput::new(
                        "Target",
                        vec![
                            Choice::new("first", "First"),
                            Choice::new("second", "Second"),
                        ],
                    )),
            )
    }
    fn open(&self, _: Input, _: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        Ok(Box::new(Self(self.0.clone())))
    }
}

impl Session for Actions {
    fn enabled(&self, _: &str) -> bool {
        !self.0.lock().unwrap().disabled
    }
    fn watch_paths(&mut self, _: &ViewCx) -> Result<Option<Vec<std::path::PathBuf>>, Failure> {
        let log = self.0.lock().unwrap();
        if log.watch_failure && !log.acts.is_empty() {
            return Err(Failure::external("project watch failed"));
        }
        Ok(None)
    }
    fn view(&mut self, req: &ViewReq, _: &ViewCx) -> Result<View, Failure> {
        use konoma_plugin::view::{ListRow, ListView};
        let hold = self.0.lock().unwrap().hold.take();
        if let Some(hold) = hold {
            hold.entered.send(()).unwrap();
            hold.release
                .recv_timeout(Duration::from_secs(10))
                .expect("release held view");
        }
        let log = self.0.lock().unwrap();
        if log.view_failure && !log.acts.is_empty() {
            return Err(Failure::external("project refresh failed"));
        }
        if req.project.as_ref().unwrap().screen == "text" {
            return Ok(View::text(["text selection=none"]));
        }
        let mut list = ListView {
            rows: vec![
                ListRow::new(
                    "item",
                    vec![Line::from(vec![
                        Span::styled(
                            if log.refresh_content && !log.acts.is_empty() {
                                "changed"
                            } else {
                                "alpha"
                            },
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
            ..Default::default()
        };
        if log.omit_alpha {
            list.rows.remove(0);
        }
        Ok(View::List(list))
    }
    fn act(&mut self, id: &str, cx: &ActCx) -> Result<Outcome, Failure> {
        let mut log = self.0.lock().unwrap();
        log.acts.push((id.into(), cx.project.clone().unwrap()));
        let hold = log.act_hold.take();
        drop(log);
        if let Some(hold) = hold {
            hold.entered.send(()).unwrap();
            hold.release
                .recv_timeout(Duration::from_secs(10))
                .expect("release held act");
        }
        let mut log = self.0.lock().unwrap();
        if log.failures > 0 {
            log.failures -= 1;
            return Err(Failure::external("project action failed"));
        }
        Ok(log
            .outcome
            .clone()
            .unwrap_or_else(|| Outcome::Flash(format!("did {id}").into())))
    }
}

struct Events {
    keys: VecDeque<AppEvent>,
    replies: mpsc::Receiver<()>,
    steps: VecDeque<Step>,
}
enum Step {
    Key(AppEvent),
    BusyKey(AppEvent),
    IdleHook(Box<dyn FnOnce()>),
    BusyHook(Box<dyn FnOnce()>),
}
impl EventSource for Events {
    fn next(&mut self, wait: Option<Duration>) -> Next {
        while self.steps.front().is_some_and(|s| {
            wait != Some(Duration::from_millis(16))
                || matches!(s, Step::BusyKey(_) | Step::BusyHook(_))
        }) {
            match self.steps.pop_front().unwrap() {
                Step::Key(k) | Step::BusyKey(k) => return Next::Event(k),
                Step::IdleHook(f) | Step::BusyHook(f) => f(),
            }
        }
        if wait == Some(Duration::from_millis(16)) {
            self.replies
                .recv_timeout(Duration::from_secs(10))
                .expect("owner reply fence");
            Next::Timeout
        } else {
            self.keys.pop_front().map(Next::Event).unwrap_or(Next::End)
        }
    }
}

fn fixture() -> (TmpDir, PreparedProjects, Events, Arc<Mutex<Log>>) {
    let dir = konoma_test_support::unique_tmp("project_actions");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("project.marker"), "").unwrap();
    let dir = dir.into_canonical().unwrap();
    let log = Arc::new(Mutex::new(Log::default()));
    let mut prepared = prepare_projects(
        vec![Arc::new(Actions(log.clone()))],
        &dir,
        TermCaps::new(),
        (SharedConfig::parse("[ui]\nlang='en'\n").unwrap(), None),
        Arc::new(super::navigation::Services),
    )
    .unwrap_or_else(|e| panic!("{}", e.error));
    let (ready, replies) = mpsc::channel();
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
    (
        dir,
        prepared,
        Events {
            keys: VecDeque::new(),
            steps: VecDeque::new(),
            replies,
        },
        log,
    )
}

fn key(name: &str) -> AppEvent {
    if name == "ctrl-F6" || name == "ctrl-F7" {
        return AppEvent::Key(
            Key::new(if name == "ctrl-F6" {
                KeyCode::F(6)
            } else {
                KeyCode::F(7)
            })
            .with_mods(konoma_plugin::KeyMods::CTRL),
        );
    }
    AppEvent::Key(Key::new(match name {
        "F5" => KeyCode::F(5),
        "Enter" => KeyCode::Enter,
        "Left" => KeyCode::Left,
        "Tab" => KeyCode::Tab,
        "Esc" => KeyCode::Esc,
        _ => KeyCode::Char(name.chars().next().unwrap()),
    }))
}

fn reference() -> Value {
    serde_json::from_str(
        &std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../snapshots/project_actions.json"),
        )
        .unwrap(),
    )
    .unwrap()
}

fn assert_cells(terminal: &Terminal<TestBackend>, expected: &Value) {
    let rect = Rect::new(0, 1, 54, 12);
    assert_eq!(expected["rect"], json!([0, 1, 54, 12]));
    let cells: Vec<_> = expected["runs"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|r| std::iter::repeat_n(&r[1], r[0].as_u64().unwrap() as usize))
        .collect();
    assert_eq!(cells.len(), 648);
    assert_eq!(expected["count"], cells.len());
    let mut compared = 0;
    let mut nonempty = 0;
    for y in rect.y..rect.bottom() {
        for x in rect.x..rect.right() {
            let c = &terminal.backend().buffer()[(x, y)];
            assert_eq!(
                json!([
                    c.symbol(),
                    format!("{:?}", c.fg),
                    format!("{:?}", c.bg),
                    c.modifier.bits()
                ]),
                *cells[compared],
                "cell ({x}, {y})"
            );
            compared += 1;
            nonempty += usize::from(!c.symbol().trim().is_empty());
        }
    }
    assert_eq!(compared, 648);
    assert!(nonempty > 0);
}

#[test]
fn project_prompt_sequence_matches_host_answers_and_cells() {
    let expected = reference();
    let names = key_names(&expected["keys"]);
    for end in 1..=names.len() {
        let (_dir, mut prepared, mut events, log) = fixture();
        events.keys = names[..end].iter().map(|k| key(k)).collect();
        let mut terminal = Terminal::new(TestBackend::new(54, 14)).unwrap();
        run_project_with(
            &mut terminal,
            &mut prepared,
            &mut events,
            &ProjectLoopCx::default(),
            None,
        )
        .unwrap();
        if end < names.len() {
            let Some(ProjectDialog::Act {
                prompts,
                stage,
                buffer,
                cursor,
                selected,
                ..
            }) = &prepared.dialog
            else {
                panic!("prompt must open at stage {end}");
            };
            match end {
                1 => assert!(
                    matches!(&prompts[*stage], ProjectPrompt::Confirmation(p) if p.text.get("en") == "Proceed?")
                ),
                2..=4 => {
                    assert!(
                        matches!(&prompts[*stage], ProjectPrompt::Text(p) if p.label.get("en") == "Name")
                    );
                    assert_eq!(buffer, ["seed", "seed追", "seed追加"][end - 2]);
                    assert_eq!(
                        *cursor,
                        buffer.chars().count(),
                        "text cursor uses characters"
                    );
                }
                5..=6 => {
                    assert!(
                        matches!(&prompts[*stage], ProjectPrompt::Choice(p) if p.label.get("en") == "Target")
                    );
                    assert_eq!(*selected, end - 5);
                }
                _ => unreachable!(),
            }
            assert!(
                log.lock().unwrap().acts.is_empty(),
                "act must wait for all answers"
            );
        } else {
            let acts = &log.lock().unwrap().acts;
            assert_eq!(acts.len(), 1);
            assert_eq!(acts[0].0, "answers");
            assert_eq!(acts[0].1.confirmation.as_deref(), Some("accepted"));
            assert_eq!(acts[0].1.text.as_deref(), Some("seed追加"));
            assert_eq!(acts[0].1.choice.as_deref(), Some("second"));
            assert_eq!(
                acts[0].1.selected,
                Some(ProjectRow::new("item", "opaque-alpha"))
            );
            assert!(prepared.dialog.is_none());
        }
        assert_cells(&terminal, &reference()["stages"][end - 1]);
    }
}

#[test]
fn project_held_view_replays_keys_and_rechecks_submit() {
    let reference = extra_reference();
    let before_submit = key_names(&reference["keys"]["held_prompt"]);
    let raw = key_names(&reference["keys"]["held_raw"]);
    for change in 0..4 {
        let (_dir, mut prepared, mut events, log) = fixture();
        let (hold, entered, release) = held();
        let mut terminal = Terminal::new(TestBackend::new(54, 14)).unwrap();
        if matches!(change, 1 | 2) {
            events.keys = before_submit.iter().map(|s| key(s)).collect();
            run_project_with(
                &mut terminal,
                &mut prepared,
                &mut events,
                &ProjectLoopCx::default(),
                None,
            )
            .unwrap();
            log.lock().unwrap().hold = Some(hold);
        } else {
            let setter = log.clone();
            events.steps.push_back(Step::IdleHook(Box::new(move || {
                setter.lock().unwrap().hold = Some(hold)
            })));
            events.steps.push_back(Step::Key(key("r")));
        }
        events.steps.push_back(Step::BusyHook(Box::new(move || {
            entered
                .recv_timeout(Duration::from_secs(10))
                .expect("view entered")
        })));
        let names: Vec<&str> = if change == 3 {
            [vec!["F5", "y"], vec!["a"; 90], vec!["Enter", "j", "Enter"]].concat()
        } else if change == 0 {
            raw[..raw.len() - 1].to_vec()
        } else {
            vec!["Enter"]
        };
        events
            .steps
            .extend(names.iter().map(|s| Step::BusyKey(key(s))));
        if matches!(change, 0 | 3) {
            events.steps.push_back(Step::BusyKey(key("j")));
        }
        let check = log.clone();
        events.steps.push_back(Step::BusyHook(Box::new(move || {
            let mut log = check.lock().unwrap();
            assert!(
                log.acts.is_empty(),
                "act must not run before held view reply"
            );
            log.disabled = change == 1;
            log.omit_alpha = change == 2;
            release.send(()).unwrap();
        })));
        run_project_with(
            &mut terminal,
            &mut prepared,
            &mut events,
            &ProjectLoopCx::default(),
            None,
        )
        .unwrap();
        let acts = &log.lock().unwrap().acts;
        if matches!(change, 0 | 3) {
            assert_eq!(
                acts.len(),
                1,
                "held raw keys must replay through each prompt"
            );
            let text = if change == 3 {
                format!("seed{}", "a".repeat(90))
            } else {
                "seed追加".into()
            };
            assert_eq!(acts[0].1.text.as_deref(), Some(text.as_str()));
            assert_eq!(acts[0].1.choice.as_deref(), Some("second"));
            assert_eq!(
                prepared.screen.as_ref().unwrap().frame().unwrap().cursor,
                0,
                "executing the act cancels later queued keys"
            );
        } else {
            assert!(
                acts.is_empty(),
                "submit must recheck the latest guard and selected row: {change}"
            );
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
            &extra_reference()["held"][if change == 3 { 0 } else { change }],
        );
    }
}

#[test]
fn project_retry_reopens_only_failed_act_text() {
    let reference = extra_reference();
    for mode in 0..4 {
        let (_dir, mut prepared, mut events, log) = fixture();
        {
            let mut state = log.lock().unwrap();
            state.failures = usize::from(mode == 0 || mode == 3);
            state.view_failure = mode == 1;
            state.watch_failure = mode == 2;
        }
        let names = if mode == 0 || mode == 3 {
            key_names(&reference["keys"]["retry_confirmed"])
        } else {
            key_names(&reference["keys"]["retry_refresh"])
        };
        events.steps.extend(names.iter().map(|s| Step::Key(key(s))));
        if mode == 3 {
            let (hold, entered, release) = held();
            log.lock().unwrap().act_hold = Some(hold);
            events.steps.push_back(Step::BusyHook(Box::new(move || {
                entered
                    .recv_timeout(Duration::from_secs(10))
                    .expect("act entered")
            })));
            events.steps.push_back(Step::BusyKey(key("Tab")));
            events
                .steps
                .push_back(Step::BusyHook(Box::new(move || release.send(()).unwrap())));
        }
        let mut terminal = Terminal::new(TestBackend::new(54, 14)).unwrap();
        run_project_with(
            &mut terminal,
            &mut prepared,
            &mut events,
            &ProjectLoopCx::default(),
            None,
        )
        .unwrap();
        assert_eq!(
            log.lock().unwrap().acts.len(),
            1,
            "retry action must be dispatched"
        );
        if mode == 0 {
            let Some(ProjectDialog::Act {
                prompts,
                stage,
                buffer,
                cursor,
                ..
            }) = &prepared.dialog
            else {
                panic!("failed act must reopen text");
            };
            assert_eq!(
                prompts.len(),
                1,
                "retry never repeats confirmation or choice"
            );
            assert_eq!(*stage, 0);
            assert!(
                matches!(&prompts[0], ProjectPrompt::Text(p) if p.label.get("en") == "Retry message")
            );
            assert_eq!(buffer, "seed追加");
            assert_eq!(*cursor, 6, "retry cursor uses characters");
        } else {
            assert!(
                prepared.dialog.is_none(),
                "only an act failure in the same origin reopens text: {mode}"
            );
        }
        assert_cells(&terminal, &extra_reference()["retry"][mode]);
        if mode == 0 {
            events.keys = [key("Enter")].into();
            run_project_with(
                &mut terminal,
                &mut prepared,
                &mut events,
                &ProjectLoopCx::default(),
                None,
            )
            .unwrap();
            let acts = &log.lock().unwrap().acts;
            assert_eq!(acts.len(), 2);
            assert_eq!(acts[1].1.confirmation.as_deref(), Some("yes"));
            assert_eq!(acts[1].1.text.as_deref(), Some("seed追加"));
            assert!(prepared.dialog.is_none());
        }
    }
}

#[test]
fn project_outcomes_copy_flash_push_pop_and_notice_are_delivered() {
    use konoma_host_core::clipboard::{clear_test_clipboard, get_test_clipboard};
    let expected: Value = serde_json::from_str(
        &std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../snapshots/project_outcome_refresh.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let names = key_names(&expected["keys"]);
    assert_eq!(
        expected["host_only"],
        json!(["OpenFile", "OpenTab", "OpenDiff", "SwitchRoot"])
    );
    let outcomes = [
        Outcome::Copy {
            text: "copied 追加".into(),
        },
        Outcome::Flash("visible flash".into()),
        Outcome::PushScreen {
            screen: "text".into(),
            selection: None,
        },
        Outcome::PopScreen,
        Outcome::OpenFile {
            path: "other.txt".into(),
            line: Some(3),
        },
        Outcome::OpenTab {
            path: "other.txt".into(),
        },
        Outcome::OpenDiff {
            path: "other.txt".into(),
        },
        Outcome::SwitchRoot {
            root: "elsewhere".into(),
            message: Some("switch message".into()),
        },
    ];
    for (index, outcome) in outcomes.into_iter().enumerate() {
        clear_test_clipboard();
        let (_dir, mut prepared, mut events, log) = fixture();
        let mut terminal = Terminal::new(TestBackend::new(54, 14)).unwrap();
        if index == 3 {
            log.lock().unwrap().outcome = Some(Outcome::PushScreen {
                screen: "items".into(),
                selection: None,
            });
            events.keys = names.iter().map(|name| key(name)).collect();
            run_project_with(
                &mut terminal,
                &mut prepared,
                &mut events,
                &ProjectLoopCx::default(),
                None,
            )
            .unwrap();
            assert_eq!(prepared.screen.as_ref().unwrap().stack.len(), 2);
        }
        log.lock().unwrap().outcome = Some(outcome);
        log.lock().unwrap().refresh_content = true;
        events.keys = names.iter().map(|name| key(name)).collect();
        run_project_with(
            &mut terminal,
            &mut prepared,
            &mut events,
            &ProjectLoopCx::default(),
            None,
        )
        .unwrap();
        let nav = prepared
            .screen
            .as_ref()
            .expect("OpenFile must keep the project screen");
        assert_eq!(
            nav.frame().unwrap().key.screen,
            if index == 2 { "text" } else { "items" }
        );
        assert_eq!(nav.stack.len(), if index == 2 { 2 } else { 1 });
        assert_eq!(
            log.lock().unwrap().acts.len(),
            if index == 3 { 2 } else { 1 }
        );
        if index == 0 {
            assert_eq!(get_test_clipboard().as_deref(), Some("copied 追加"));
            assert_eq!(prepared.notice.as_deref(), Some("copied: copied 追加"));
        } else {
            assert!(get_test_clipboard().is_none());
        }
        if index == 1 {
            assert_eq!(prepared.notice.as_deref(), Some("visible flash"));
        }
        if index >= 4 {
            assert_eq!(
                prepared.notice.as_deref(),
                Some("this action is not supported in the standalone app")
            );
        }
        if index == 2 {
            events.keys = [key("q")].into();
            run_project_with(
                &mut terminal,
                &mut prepared,
                &mut events,
                &ProjectLoopCx::default(),
                None,
            )
            .unwrap();
        }
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(
            text.contains("changed"),
            "every outcome must adopt its reply's refresh: {index}"
        );
        assert_cells(&terminal, &expected);
    }
}

fn extra_reference() -> Value {
    serde_json::from_str(
        &std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../snapshots/project_actions_extra.json"),
        )
        .unwrap(),
    )
    .unwrap()
}

fn key_names(value: &Value) -> Vec<&str> {
    value
        .as_array()
        .unwrap()
        .iter()
        .map(|key| key.as_str().unwrap())
        .collect()
}
