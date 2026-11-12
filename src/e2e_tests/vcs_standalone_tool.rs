//! The standalone `konoma-vcs` program's `!` reaches the launch seam with the configured tool.
//! The seam records the attempt and runs no program.
use super::vcs_changes::git_repo;
use konoma_host_core::shared::SharedConfig;
use konoma_plugin::app::project::{prepare_projects, run_project_with, ProjectLoopCx};
use konoma_plugin::app::{key_from_crossterm, AppEvent, EventSource, LaunchCx, Next};
use konoma_plugin::{Plugin, TermCaps};
use ratatui::{backend::TestBackend, Terminal};
use std::path::Path;
use std::sync::{Arc, Mutex};

/// One recorded launch: program, arguments and working directory.
type Launch = (String, Vec<String>, String);

/// Feeds `keys` one at a time, giving the loop idle turns (a bounded number of short waits) before
/// each key and after the last, so asynchronous screen loading and queued launches settle.
struct Keys {
    keys: std::collections::VecDeque<char>,
    gap: usize,
    log: Arc<Mutex<Vec<Launch>>>,
    after: usize,
}

impl EventSource for Keys {
    fn next(&mut self, _wait: Option<std::time::Duration>) -> Next {
        const IDLE_TURNS: usize = 40;
        if self.gap < IDLE_TURNS {
            self.gap += 1;
            std::thread::sleep(std::time::Duration::from_millis(5));
            return Next::Timeout;
        }
        if let Some(c) = self.keys.pop_front() {
            self.gap = 0;
            return Next::Event(AppEvent::Key(
                key_from_crossterm(&crossterm::event::KeyEvent::new(
                    // `\t` stands for the Tab key, which cycles the project's screens.
                    if c == '\t' {
                        crossterm::event::KeyCode::Tab
                    } else {
                        crossterm::event::KeyCode::Char(c)
                    },
                    crossterm::event::KeyModifiers::NONE,
                ))
                .expect("a key"),
            ));
        }
        // Keys are used up: settle until a launch is recorded or the idle budget is spent.
        if self.log.lock().unwrap().is_empty() && self.after < 400 {
            self.after += 1;
            std::thread::sleep(std::time::Duration::from_millis(5));
            return Next::Timeout;
        }
        Next::End
    }
}

/// Runs the standalone project loop on `root` with `keys` and returns what the seam recorded as
/// `(program, args, cwd)`.
fn run_keys(root: &Path, config: &str, keys: &str) -> Vec<Launch> {
    let mut plugins: Vec<Arc<dyn Plugin>> =
        konoma_vcs::program().into_iter().map(Arc::from).collect();
    plugins.extend(konoma_file_plugins::plugins());
    let mut prepared = prepare_projects(
        plugins,
        root,
        TermCaps::new(),
        (SharedConfig::parse(config).unwrap(), None),
        Arc::new(konoma_vcs::standalone::VcsServices),
    )
    .unwrap_or_else(|e| panic!("{}", e.error));
    let log = Arc::new(Mutex::new(Vec::new()));
    let sink = log.clone();
    let launch = LaunchCx::new(
        prepared.config.editor.clone(),
        &prepared.config.external,
        vec![],
    )
    .with_hand_off(Arc::new(move |attempt, cwd, redraw| {
        sink.lock().unwrap().push((
            attempt.prog.clone(),
            attempt.args.clone(),
            cwd.display().to_string(),
        ));
        redraw()?;
        Ok(Some(0))
    }));
    let cx = ProjectLoopCx {
        launch: Some(launch),
        ..Default::default()
    };
    let mut events = Keys {
        keys: keys.chars().collect(),
        gap: 0,
        log: log.clone(),
        after: 0,
    };
    let mut terminal = Terminal::new(TestBackend::new(120, 20)).unwrap();
    run_project_with(&mut terminal, &mut prepared, &mut events, &cx, None).unwrap();
    let records = log.lock().unwrap().clone();
    records
}

#[test]
fn vcs_standalone_bang_hands_the_configured_tool_to_the_launch_seam() {
    let repo = git_repo("vcs_standalone_bang", true);
    let root = repo.canonicalize().unwrap();
    // The trailing keys keep the loop turning so the queued launch is polled before the end.
    let records = run_keys(
        &root,
        "[ui]\nlang='en'\n[git]\ntool='fixture-git-tool --flag'\n",
        "!jjj",
    );
    assert_eq!(records.len(), 1, "{records:?}");
    let (prog, args, cwd) = &records[0];
    assert_eq!(prog, "fixture-git-tool");
    assert_eq!(args, &["--flag".to_string()]);
    assert_eq!(Path::new(cwd).canonicalize().unwrap(), root);
}

#[test]
fn vcs_standalone_bang_without_the_key_launches_nothing_and_a_disabled_tool_stays_off() {
    let repo = git_repo("vcs_standalone_no_bang", true);
    let root = repo.canonicalize().unwrap();
    let config = "[ui]\nlang='en'\n[git]\ntool='fixture-git-tool'\n";
    assert!(run_keys(&root, config, "jjj").is_empty());
    let off = "[ui]\nlang='en'\n[git]\ntool='fixture-git-tool'\n[external]\ngit_tool=false\n";
    assert!(run_keys(&root, off, "!jjj").is_empty());
}

/// Old hub: `!` is bound only in the changes screen's table, so the standalone program's `!`
/// launches nothing on the log, graph, branches and worktrees screens either (Tab cycles to them).
#[test]
fn vcs_standalone_bang_acts_only_on_the_changes_screen() {
    let repo = git_repo("vcs_standalone_bang_screens", true);
    let root = repo.canonicalize().unwrap();
    let config = "[ui]\nlang='en'\n[git]\ntool='fixture-git-tool'\n";
    for tabs in 1..=4 {
        let keys = format!("{}!jjj", "\t".repeat(tabs));
        assert!(
            run_keys(&root, config, &keys).is_empty(),
            "`!` {tabs} screen(s) after changes launched the tool"
        );
    }
    // The same keys without the Tabs do launch (changes), so the empty results above are not a
    // harness that never launches.
    assert_eq!(run_keys(&root, config, "!jjj").len(), 1);
}
