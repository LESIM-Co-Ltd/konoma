use super::navigation::Services;
use konoma_host_core::shared::SharedConfig;
use konoma_plugin::app::project::{
    prepare_projects, run_project_with, PreparedProjects, ProjectLoopCx,
};
use konoma_plugin::app::{AppEvent, EventSource, Next};
use konoma_plugin::TermCaps;
use konoma_test_support::TmpDir;
use ratatui::{backend::TestBackend, Terminal};
use std::collections::VecDeque;
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

#[path = "presentation_fixture.rs"]
mod fixture;
use fixture::{Log, PresentationPlugin};

#[path = "presentation_wake.rs"]
mod wake;

struct Events {
    keys: VecDeque<AppEvent>,
    replies: mpsc::Receiver<Box<dyn FnOnce() + Send>>,
    #[cfg(feature = "test-hooks")]
    last_build: usize,
}
impl EventSource for Events {
    fn next(&mut self, wait: Option<Duration>) -> Next {
        #[cfg(feature = "test-hooks")]
        {
            let count =
                konoma_plugin::host::project::presentation::project_presentation_build_count();
            assert_eq!(
                count - self.last_build,
                1,
                "one presentation per drawn frame"
            );
            self.last_build = count;
        }
        if wait == Some(Duration::from_millis(16)) {
            let publish = self
                .replies
                .recv_timeout(Duration::from_secs(10))
                .expect("owner reply fence");
            publish();
            Next::Timeout
        } else {
            self.keys.pop_front().map(Next::Event).unwrap_or(Next::End)
        }
    }
}

fn setup(config: &str) -> (TmpDir, PreparedProjects, Events, Arc<Mutex<Log>>) {
    setup_with(config, Log::default())
}
fn setup_with(config: &str, initial: Log) -> (TmpDir, PreparedProjects, Events, Arc<Mutex<Log>>) {
    let dir = konoma_test_support::unique_tmp("project_presentation");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("project.marker"), "").unwrap();
    let log = Arc::new(Mutex::new(initial));
    let mut prepared = prepare_projects(
        vec![Arc::new(PresentationPlugin(log.clone()))],
        &dir,
        TermCaps::new(),
        (SharedConfig::parse(config).unwrap(), None),
        Arc::new(Services),
    )
    .unwrap_or_else(|e| panic!("{}", e.error));
    let (ready, replies) = mpsc::channel::<Box<dyn FnOnce() + Send>>();
    for runtime in prepared.runtimes.values_mut() {
        let (tx, rx) = mpsc::sync_channel(1);
        let owner = std::mem::replace(&mut runtime.rx, rx);
        let ready = ready.clone();
        std::thread::spawn(move || {
            while let Ok(reply) = owner.recv() {
                let tx = tx.clone();
                // Keep each reply private until the frame fence publishes it. Sending a
                // notification after publication races with the loop consuming that reply.
                let publish = Box::new(move || {
                    assert!(tx.try_send(reply).is_ok(), "owner reply mailbox is empty");
                });
                if ready.send(publish).is_err() {
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
            #[cfg(feature = "test-hooks")]
            last_build: 0,
        },
        log,
    )
}

#[test]
fn standalone_project_host_only_keys_are_inert_and_unadvertised() {
    let config = "[ui]\nlang='en'\n[keys.global]\nx='tab_next'\nW='tab_new_after'\nv='toggle_follow'\n[keys.text]\nh='tab_prev'\n[keys.canvas]\nz='mark_set'\nb='tree_descend'\n";
    let (_dir, mut prepared, mut events, log) = setup(config);
    let before = run(&mut prepared, &mut events, &[])
        .backend()
        .buffer()
        .clone();
    let terminal = run(
        &mut prepared,
        &mut events,
        &[
            "ctrl-t", "T", "F", "H", "L", "1", "x", "W", "h", "z", "e", "o", "!", "v", "b",
        ],
    );
    assert_eq!(
        terminal.backend().buffer(),
        &before,
        "host-only keys must not reach a generic executor"
    );
    assert!(log.lock().unwrap().acts.is_empty());
    let terminal = run(&mut prepared, &mut events, &["?"]);
    // The Global section (`?`, `Q`) is the standalone's own; its `Q` row reads "back to tree in
    // preview" (the shared core text), so it is taken out before the host-only words are looked for.
    let help = text(&terminal);
    assert!(
        help.contains("Global") && help.contains("quit (back to tree in preview)"),
        "the standalone keeps its two Global rows: {help}"
    );
    let help = help.replace("quit (back to tree in preview)", "");
    for forbidden in [
        "new tab", "Tabs", "follow", "tree", "e:edit", "o:open", "!:tool",
    ] {
        assert!(!help.contains(forbidden), "host-only hint: {forbidden}");
    }
    let terminal = run(&mut prepared, &mut events, &["Space"]);
    assert!(!text(&terminal).contains("new tab"));
    assert!(log.lock().unwrap().acts.is_empty());
}

#[test]
fn project_open_aliases_and_root_pushed_wording_match_actions() {
    let (_dir, mut prepared, mut events, log) = setup("[ui]\nlang='en'\n");
    let terminal = run(&mut prepared, &mut events, &["?"]);
    // The Global section's `Q` row also says "quit"; it is taken out so that the words checked
    // below are the ones of the screen's own return row.
    let own = |terminal: &Terminal<TestBackend>| {
        text(terminal).replace("quit (back to tree in preview)", "")
    };
    let help = own(&terminal);
    assert!(
        help.contains("l/→"),
        "Enter coverage must leave both Right aliases"
    );
    assert!(help.contains("quit"));
    assert!(!help.contains("back to changes"));
    let terminal = run(&mut prepared, &mut events, &["Right", "?"]);
    assert_eq!(log.lock().unwrap().acts, ["open"]);
    assert!(own(&terminal).contains("back"));
    assert!(!own(&terminal).contains("quit"));
    assert_eq!(prepared.screen.as_ref().unwrap().stack.len(), 2);
    let (_dir, mut prepared, mut events, _) = setup_with(
        "[ui]\nlang='en'\n",
        Log {
            graph: true,
            ..Default::default()
        },
    );
    let terminal = run(&mut prepared, &mut events, &["?"]);
    assert!(own(&terminal).contains("quit"), "graph root must say quit");
    assert!(!text(&terminal).contains("back to changes"));
}

#[test]
fn project_notice_and_chip_error_follow_host_rules() {
    use konoma_host_core::clipboard::{clear_test_clipboard, get_test_clipboard};
    use konoma_plugin::Outcome;
    let outcomes = [
        Outcome::Copy {
            text: "alpha".into(),
        },
        Outcome::Flash("visible flash".into()),
        Outcome::OpenFile {
            path: "alpha.txt".into(),
            line: Some(2),
        },
        Outcome::OpenTab {
            path: "alpha.txt".into(),
        },
        Outcome::OpenDiff {
            path: "alpha.txt".into(),
        },
        Outcome::SwitchRoot {
            root: "elsewhere".into(),
            message: None,
        },
    ];
    for (index, outcome) in outcomes.into_iter().enumerate() {
        clear_test_clipboard();
        let (_dir, mut prepared, mut events, _) = setup_with(
            "[ui]\nlang='en'\n",
            Log {
                outcome: Some(outcome),
                ..Default::default()
            },
        );
        let terminal = run(&mut prepared, &mut events, &["Enter"]);
        let expected = match index {
            0 => "copied: alpha",
            1 => "visible flash",
            _ => "this action is not supported in the standalone app",
        };
        assert_eq!(prepared.notice.as_deref(), Some(expected));
        assert_eq!(
            get_test_clipboard(),
            (index == 0).then(|| "alpha".to_string())
        );
        assert_footer(&terminal, expected);
        assert_cells(&terminal, &reference()["notices"][index.min(2)]);
        let terminal = run(&mut prepared, &mut events, &["~"]);
        assert_eq!(prepared.notice, None, "the next key must clear the notice");
        assert_cells(&terminal, &reference()["stages"][6]);
    }
    for listed in [true, false] {
        let (_dir, mut prepared, mut events, _) = setup_with(
            "[ui]\nlang='en'\n",
            Log {
                chip_failure: true,
                ..Default::default()
            },
        );
        if !listed {
            prepared.detection.projects.clear();
        }
        let terminal = run(&mut prepared, &mut events, &[]);
        assert_eq!(
            prepared.notice.as_deref(),
            listed.then_some("chip failed"),
            "only a listed project's chip error flashes"
        );
        if listed {
            assert_footer(&terminal, "chip failed");
        }
        assert_cells(
            &terminal,
            &reference()["notices"][if listed { 3 } else { 4 }],
        );
        let terminal = run(&mut prepared, &mut events, &["~"]);
        assert_eq!(prepared.notice, None);
        assert_cells(&terminal, &reference()["stages"][6]);
    }
}

fn assert_footer(terminal: &Terminal<TestBackend>, notice: &str) {
    use ratatui::style::{Color, Modifier};
    for (x, expected) in notice
        .chars()
        .chain(std::iter::repeat(' '))
        .take(100)
        .enumerate()
    {
        let c = &terminal.backend().buffer()[(x as u16, 31)];
        assert_eq!(c.symbol(), expected.to_string());
        assert_eq!((c.fg, c.bg), (Color::Reset, Color::Reset));
        assert_eq!(
            c.modifier,
            if x < notice.len() {
                Modifier::BOLD
            } else {
                Modifier::empty()
            }
        );
    }
}

fn run(
    prepared: &mut PreparedProjects,
    events: &mut Events,
    keys: &[&str],
) -> Terminal<TestBackend> {
    #[cfg(feature = "test-hooks")]
    {
        events.last_build =
            konoma_plugin::host::project::presentation::project_presentation_build_count();
    }
    events.keys = keys.iter().map(|k| AppEvent::Key(raw_key(k))).collect();
    let mut terminal = Terminal::new(TestBackend::new(100, 32)).unwrap();
    run_project_with(
        &mut terminal,
        prepared,
        events,
        &ProjectLoopCx::default(),
        None,
    )
    .unwrap();
    terminal
}

fn raw_key(name: &str) -> konoma_plugin::Key {
    use konoma_plugin::{Key, KeyCode};
    if let Some(c) = name.strip_prefix("ctrl-") {
        return Key::ctrl(c.chars().next().unwrap());
    }
    Key::new(match name {
        "Enter" => KeyCode::Enter,
        "Right" => KeyCode::Right,
        "Esc" => KeyCode::Esc,
        "Tab" => KeyCode::Tab,
        "BackTab" => KeyCode::BackTab,
        "Space" => KeyCode::Char(' '),
        _ => KeyCode::Char(name.chars().next().unwrap()),
    })
}

fn text(terminal: &Terminal<TestBackend>) -> String {
    terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|c| c.symbol())
        .collect()
}

#[test]
fn project_help_footer_and_menus_follow_host_key_sequences() {
    let expected = reference();
    for (index, stage) in expected["stages"].as_array().unwrap().iter().enumerate() {
        let (_dir, mut prepared, mut events, log) = setup("[ui]\nlang='en'\n");
        let names: Vec<_> = stage["keys"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        let terminal = run(&mut prepared, &mut events, &names);
        assert_cells(&terminal, stage);
        let acts = &log.lock().unwrap().acts;
        assert_eq!(serde_json::json!(acts), stage["acts"], "stage {index}");
        if index == 0 {
            assert!(
                text(&terminal).contains("inspect row"),
                "project help must be drawn"
            );
        }
    }
    let (_dir, mut prepared, mut events, log) = setup("[ui]\nlang='en'\n");
    run(&mut prepared, &mut events, &["/", "y", "Space", "Enter"]);
    assert_eq!(
        prepared.screen.as_ref().unwrap().frame().unwrap().query,
        "y ",
        "menu triggers remain literal filter input"
    );
    assert!(log.lock().unwrap().acts.is_empty());
}

fn reference() -> serde_json::Value {
    serde_json::from_str(
        &std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../snapshots/project_presentation.json"),
        )
        .unwrap(),
    )
    .unwrap()
}

fn assert_cells(terminal: &Terminal<TestBackend>, expected: &serde_json::Value) {
    use serde_json::json;
    let buffer = terminal.backend().buffer();
    assert_eq!(buffer.area, ratatui::layout::Rect::new(0, 0, 100, 32));
    assert_eq!(expected["rect"], json!([0, 0, 100, 32]));
    let cells: Vec<_> = expected["runs"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|r| std::iter::repeat_n(&r[1], r[0].as_u64().unwrap() as usize))
        .collect();
    assert_eq!(cells.len(), 3200);
    assert_eq!(expected["count"], cells.len());
    let mut compared = 0;
    let mut nonempty = 0;
    for y in 0..32 {
        for x in 0..100 {
            let c = &buffer[(x, y)];
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
    assert_eq!(compared, 3200);
    assert!(nonempty > 0);
}
