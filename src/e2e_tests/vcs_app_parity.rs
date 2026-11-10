//! The real project host and the binary's uninterrupted project loop receive identical input.
use super::project_frames::compare_frames;
use super::vcs_changes::{drain, git_repo};
use super::vcs_log::{
    commit, detail_repo, empty_subject_commit, empty_subject_repo, jj, jj_repo,
    remove_commit_object, remove_head_object, repo,
};
use super::*;
use konoma_host_core::clipboard::{clear_test_clipboard, get_test_clipboard};
use konoma_host_core::shared::SharedConfig;
use konoma_plugin::app::project::{
    observe_project_state_for_test, prepare_projects, probe_graph_plugin_for_test,
    read_snapshot_for_test, run_project_fenced_state_for_test, ProjectLoopCx, ProjectStateSnapshot,
};
use konoma_plugin::app::{key_from_crossterm, AppEvent, Next};
use konoma_plugin::host::project::ProjectContent;
use konoma_plugin::{Plugin, TermCaps};
use ratatui::{
    backend::{Backend, ClearType, WindowSize},
    buffer::{Buffer, Cell},
    layout::{Position, Rect, Size},
    style::{Color, Modifier, Style},
    text::Line,
    widgets::{Paragraph, Widget},
};
use std::{
    path::Path,
    sync::{Arc, Mutex},
};

include!("vcs_app_write_cases.rs");

thread_local! {
    static TALLY: std::cell::Cell<(usize, usize)> = const { std::cell::Cell::new((0, 0)) };
}

fn report_cases() {
    TALLY.with(|t| {
        let (runs, fences) = t.get();
        eprintln!("U8b: {runs} uninterrupted sessions, {fences} frame/state/clipboard fences");
    });
}

/// Sharing the backend only lets the observer resize it while Terminal owns its mutable borrow.
struct SharedBackend(Arc<Mutex<TestBackend>>);
impl Backend for SharedBackend {
    type Error = std::convert::Infallible;
    fn draw<'a, I>(&mut self, cells: I) -> Result<(), Self::Error>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        self.0.lock().unwrap().draw(cells)
    }
    fn hide_cursor(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
    fn show_cursor(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
    fn get_cursor_position(&mut self) -> Result<Position, Self::Error> {
        Ok((0, 0).into())
    }
    fn set_cursor_position<P: Into<Position>>(&mut self, _: P) -> Result<(), Self::Error> {
        Ok(())
    }
    fn clear(&mut self) -> Result<(), Self::Error> {
        self.0.lock().unwrap().clear()
    }
    fn clear_region(&mut self, region: ClearType) -> Result<(), Self::Error> {
        self.0.lock().unwrap().clear_region(region)
    }
    fn size(&self) -> Result<Size, Self::Error> {
        self.0.lock().unwrap().size()
    }
    fn window_size(&mut self) -> Result<WindowSize, Self::Error> {
        self.0.lock().unwrap().window_size()
    }
    fn flush(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}

type BeforeKey = Box<dyn Fn(&Path)>;
enum Step {
    Key(KeyEvent),
    Resize(u16, u16),
    BeforeKey(BeforeKey, KeyEvent),
    Write(KeyEvent, &'static str),
    Policy(String),
}
fn key(code: KeyCode) -> Step {
    Step::Key(KeyEvent::new(code, KeyModifiers::NONE))
}
fn ch(c: char) -> Step {
    key(KeyCode::Char(c))
}
fn typed(text: &str) -> Vec<Step> {
    text.chars().map(ch).collect()
}

/// Each scenario process has a temporary HOME/config, including the real host's context home.
fn isolated(name: &str) -> bool {
    if std::env::var("KONOMA_VCS_APP_PARITY_CHILD").as_deref() == Ok(name) {
        return false;
    }
    let home = konoma_test_support::unique_tmp("vcs_app_home");
    std::fs::create_dir_all(&home).unwrap();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            &format!("e2e_tests::vcs_app_parity::{name}"),
            "--nocapture",
        ])
        .env("KONOMA_VCS_APP_PARITY_CHILD", name)
        .env("HOME", home.canonicalize().unwrap())
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_DATA_HOME", home.join("data"))
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("JJ_CONFIG", "")
        .env("GIT_AUTHOR_DATE", "2026-01-01T00:00:00+0000")
        .env("GIT_COMMITTER_DATE", "2026-01-01T00:00:00+0000")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.lines().any(|line| line == "running 1 test")
            && stdout
                .lines()
                .any(|line| line.starts_with("test result: ok. 1 passed;")),
        "isolated child must run and pass exactly one test ({name}):\n{stdout}"
    );
    eprint!("{}", String::from_utf8_lossy(&output.stderr));
    true
}

/// Reuse the existing recipe, then place its result under the child process's temporary HOME.
fn at_home(dir: crate::test_support::TmpDir) -> std::path::PathBuf {
    let target =
        std::path::PathBuf::from(std::env::var_os("HOME").unwrap()).join(dir.file_name().unwrap());
    std::fs::rename(&dir, &target).unwrap();
    target.canonicalize().unwrap()
}

fn config(id: &str, lang: &str, scheme: &str, path: &str, align: &str, extra: &str) -> String {
    let plugin: Arc<dyn Plugin> = match id {
        "git" => Arc::new(konoma_vcs::Git),
        "jj" => Arc::new(konoma_vcs::Jj),
        _ => unreachable!(),
    };
    let marker = crate::plugins::test_plugins::register(&format!("vcs_app_{id}"), plugin);
    let other = if id == "git" { "jj" } else { "git" };
    format!("[ui]\nlang='{lang}'\nkeys='{scheme}'\ntabbar='hidden'\nstatusbar='split'\ndiff_view='source'\npath_style='{path}'\ncommit_meta_align='{align}'\n[git]\ndiff='unified'\n[project]\nvcs='{id}'\n[plugins.{marker:?}]\nenabled=false\n[plugins.{other}]\nenabled=false\n{extra}")
}

fn host_state(host: &Sim) -> ProjectStateSnapshot {
    let content = host.app.project_content();
    let menu = host.app.pending_leader.map(|menu| match menu {
        crate::keymap::LeaderId::Copy => "copy",
        crate::keymap::LeaderId::Actions => "actions",
        _ => panic!("unexpected menu {menu:?}"),
    });
    let mut state = observe_project_state_for_test(
        host.app.tab.project_navigation.as_deref(),
        content.as_deref(),
        host.app.project_dialog.as_ref(),
        menu,
        host.app.flash.clone(),
    );
    state.chip = host
        .app
        .tab
        .project_navigation
        .as_ref()
        .and_then(|nav| host.app.project_runtimes.get(&nav.project))
        .and_then(|runtime| runtime.chip.clone());
    state
}

fn expected_footer(
    fixture: &serde_json::Value,
    lang: &str,
    kind: &str,
    side: &str,
    width: u16,
) -> Buffer {
    let text = fixture[lang][kind][side][width.to_string()]
        .as_str()
        .unwrap_or_else(|| panic!("missing explicit footer: {lang}/{kind}/{side}/{width}"));
    let style = |name: &str| {
        Style::default()
            .fg(fixture[name]["fg"].as_str().unwrap().parse().unwrap())
            .bg(fixture[name]["bg"].as_str().unwrap().parse().unwrap())
            .add_modifier(Modifier::from_bits_truncate(
                fixture[name]["modifiers"].as_u64().unwrap() as u16,
            ))
    };
    let mut row = Buffer::empty(Rect::new(0, 0, width, 1));
    for cell in &mut row.content {
        cell.set_symbol(fixture["blank_style"]["symbol"].as_str().unwrap())
            .set_style(style("blank_style"));
    }
    Paragraph::new(Line::raw(text).style(style("hint_style"))).render(row.area, &mut row);
    row
}

fn cells(frame: &Buffer, y: u16) -> Buffer {
    let mut row = Buffer::empty(Rect::new(0, 0, frame.area.width, 1));
    for x in 0..frame.area.width {
        row[(x, 0)] = frame[(x, y)].clone();
    }
    row
}

fn frames(
    host: &Buffer,
    app: &Buffer,
    state: &ProjectStateSnapshot,
    lang: &str,
    fixture: &serde_json::Value,
    graph_fixture: &serde_json::Value,
    help_open: bool,
) {
    assert_eq!(host.area, app.area);
    let normal_root = state.stack.len() == 1
        && !state.filtering
        && state.menu.is_none()
        && state.prompt.is_none()
        && state.notice.is_none();
    let normal_root = normal_root && !help_open;
    let mut expected = host.clone();
    if help_open && state.stack.len() == 1 {
        // §4-1:220 and §7-5:967 apply to the root return key's help row as well as its footer.
        // Check each description's complete cells before substituting only this documented row.
        let screen = state.stack.last().unwrap();
        let row = &graph_fixture["root_help"][lang][screen];
        if let Some(keys) = row["key"].as_str() {
            let panel_width = konoma_widgets::help_panel::PANEL_WIDTH
                .min(host.area.width.saturating_sub(2))
                .max(20);
            let panel_x = (host.area.width.saturating_sub(panel_width)) / 2;
            let key_x = panel_x + 3;
            let desc_x = panel_x + 19;
            let width = panel_width - 20;
            let ys: Vec<_> = (0..host.area.height)
                .filter(|y| {
                    keys.chars()
                        .enumerate()
                        .all(|(i, c)| host[(key_x + i as u16, *y)].symbol() == c.to_string())
                })
                .collect();
            assert_eq!(ys.len(), 1, "exactly one explicit root return help row");
            let y = ys[0];
            for (side, frame) in [("host", host), ("app", app)] {
                let mut wanted = Buffer::empty(Rect::new(0, 0, width, 1));
                Paragraph::new(
                    Line::raw(row[side].as_str().unwrap())
                        .style(Style::new().add_modifier(Modifier::DIM)),
                )
                .render(wanted.area, &mut wanted);
                let mut actual = wanted.clone();
                for x in 0..width {
                    actual[(x, 0)] = frame[(desc_x + x, y)].clone();
                }
                compare_frames(&wanted, &actual)
                    .unwrap_or_else(|e| panic!("explicit {side} root help {screen}: {e}"));
            }
            for x in 0..width {
                expected[(desc_x + x, y)] = app[(desc_x + x, y)].clone();
            }
        }
    }
    if normal_root {
        // FEATURE-PLUGIN-ARCHITECTURE.md §4-1:220: "ツリー・タブ・ブックマーク・フォローは無い。`q` で終了。"
        // §10-3:1266: "`Ctrl-t`（新しいタブ）" is absent; §7-5:967:
        // "root の画面の `q` は終了、重ねた画面の `q` は戻る。"
        // Only the footer differs: body/title y coordinates are still compared below.
        let screen = state.stack.last().unwrap().as_str();
        let screen = if screen == "changes" && state.backend.as_deref() == Some("jj") {
            "jj_changes"
        } else {
            screen
        };
        let kind = if screen == "graph" {
            format!(
                "{}graph_{}",
                if state.backend.as_deref() == Some("jj") {
                    "jj_"
                } else {
                    ""
                },
                state
                    .selected
                    .as_ref()
                    .map_or("empty", |row| row.kind.as_str())
            )
        } else if screen == "branches" && state.backend.as_deref() == Some("jj") {
            "jj_branches".into()
        } else if state.selected.is_none() {
            format!("empty_{screen}")
        } else {
            screen.into()
        };
        let y = host.area.bottom() - 1;
        let mut graph_chrome = fixture.clone();
        if let Some(row) = graph_fixture[lang][&kind].as_object() {
            for (side, tokens) in row {
                graph_chrome[lang][&kind][side][host.area.width.to_string()] =
                    serde_json::Value::String(konoma_widgets::help_panel::fit_tokens(
                        &tokens
                            .as_str()
                            .unwrap()
                            .split("  ")
                            .map(str::to_owned)
                            .collect::<Vec<_>>(),
                        host.area.width,
                    ));
            }
        }
        let fixture = &graph_chrome;
        if fixture[lang][&kind]["host"][host.area.width.to_string()].is_null() {
            let plain = |frame: &Buffer| -> String {
                (0..frame.area.width)
                    .map(|x| frame[(x, y)].symbol())
                    .collect()
            };
            panic!(
                "missing explicit footer {lang}/{kind}/{}\nhost: {:?}\napp: {:?}",
                host.area.width,
                plain(host),
                plain(app)
            );
        }
        for (side, frame) in [("host", host), ("app", app)] {
            compare_frames(
                &expected_footer(fixture, lang, &kind, side, host.area.width),
                &cells(frame, y),
            )
            .unwrap_or_else(|e| panic!("explicit {side} footer {kind}: {e}"));
        }
        for x in 0..host.area.width {
            expected[(x, y)] = app[(x, y)].clone();
        }
    }
    compare_frames(&expected, app).unwrap_or_else(|e| panic!("whole frame: {e}"));
}

fn compare_states(host: &ProjectStateSnapshot, app: &ProjectStateSnapshot) -> Result<(), String> {
    if host == app {
        Ok(())
    } else {
        Err(format!("host: {host:?}\napp: {app:?}"))
    }
}

fn run(
    root: &Path,
    id: &str,
    text: &str,
    steps: Vec<Step>,
    failure: bool,
    check: impl FnMut(usize, &ProjectStateSnapshot, &Sim, &[String]),
) {
    run_roots((root, root), id, text, steps, failure, check);
}

fn run_roots(
    (root, app_root): (&Path, &Path),
    id: &str,
    text: &str,
    steps: Vec<Step>,
    failure: bool,
    mut check: impl FnMut(usize, &ProjectStateSnapshot, &Sim, &[String]),
) {
    use crate::plugins::test_projects::{ObservedVcs, VcsCalls};
    let host_calls = Arc::new(Mutex::new(VcsCalls::default()));
    let app_calls = Arc::new(Mutex::new(VcsCalls::default()));
    let mut cfg: Config = toml::from_str(text).unwrap();
    let plugin = || -> Arc<dyn Plugin> {
        match id {
            "git" => Arc::new(konoma_vcs::Git),
            "jj" => Arc::new(konoma_vcs::Jj),
            _ => unreachable!(),
        }
    };
    let write_actions: Vec<_> = plugin()
        .manifest()
        .actions
        .into_iter()
        .filter(|a| a.writes)
        .map(|a| a.id)
        .collect();
    let (host_plugin, host_graph) = probe_graph_plugin_for_test(Arc::new(ObservedVcs {
        plugin: plugin(),
        calls: host_calls.clone(),
    }));
    static PROBE_ID: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let marker = crate::plugins::test_plugins::register(
        &format!(
            "vcs_app_probe_{}",
            PROBE_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ),
        host_plugin,
    );
    let previous = cfg
        .plugins
        .keys()
        .find(|key| key.starts_with("+test:"))
        .unwrap()
        .clone();
    let settings = cfg.plugins.remove(&previous).unwrap();
    cfg.plugins.insert(marker, settings);
    let lang = cfg.ui.lang.clone();
    let read_only = cfg
        .plugins
        .get(id)
        .and_then(|p| p.get("read_only"))
        .and_then(toml::Value::as_bool)
        == Some(true);
    let mut host = Sim::with_config_sized(root, cfg, 90, 14);
    host.app.tab.projects.wait_for_result();
    host.app.poll_projects();
    host.key('p');
    drain(&mut host);
    assert!(host.app.project_active(), "host must open the real project");
    let mut plugins: Vec<Arc<dyn Plugin>> =
        konoma_vcs::program().into_iter().map(Arc::from).collect();
    let (app_plugin, app_graph) = probe_graph_plugin_for_test(Arc::new(ObservedVcs {
        plugin: plugin(),
        calls: app_calls.clone(),
    }));
    *plugins
        .iter_mut()
        .find(|plugin| plugin.manifest().id == id)
        .unwrap() = app_plugin;
    plugins.extend(konoma_file_plugins::plugins());
    let mut app = prepare_projects(
        plugins,
        app_root,
        TermCaps::new(),
        (SharedConfig::parse(text).unwrap(), None),
        Arc::new(konoma_vcs::standalone::VcsServices),
    )
    .unwrap_or_else(|e| panic!("{}", e.error));
    assert_eq!(
        app.screen.as_ref().unwrap().project.plugin,
        id,
        "standalone backend"
    );
    assert_eq!(
        host.app
            .tab
            .project_navigation
            .as_ref()
            .unwrap()
            .project
            .plugin,
        id,
        "host backend"
    );
    assert!(host.app.tab.projects.uses_injected_events());
    assert!(app.detection.uses_injected_events());
    let clipboard = app.clipboard_for_test();
    clipboard.fail(failure);
    let backend = Arc::new(Mutex::new(TestBackend::new(90, 14)));
    let mut terminal = Terminal::new(SharedBackend(backend.clone())).unwrap();
    let fixture = read_snapshot_for_test("vcs_app_chrome");
    let mut graph_fixture = read_snapshot_for_test("vcs_app_graph");
    if root != app_root {
        // These are the same §4-1:220 / §7-5:967 root-footer exceptions as U8b/U8c.
        let write_fixture = read_snapshot_for_test("vcs_app_writes");
        for lang in ["en", "ja"] {
            for screen in ["branches", "changes", "empty_changes", "jj_changes"] {
                let kind = if read_only {
                    format!("readonly_{screen}")
                } else {
                    screen.into()
                };
                if !write_fixture[lang][&kind].is_null() {
                    graph_fixture[lang][screen] = write_fixture[lang][&kind].clone();
                }
            }
        }
    }
    let mut host_writes = Vec::new();
    let count = steps.len();
    let mut steps = steps.into_iter();
    let mut step = 0;
    let mut expected_acts = Vec::<String>::new();
    let mut refresh_before = None;
    let cx = ProjectLoopCx {
        home: std::env::var_os("HOME").map(Into::into),
        launch: None,
    };
    // Public prepared state can be reconfigured only between runs. Ordinary scenarios still
    // use one uninterrupted run; the submit-boundary case retains its real dialog and owners.
    loop {
        let mut policy_change = None;
        run_project_fenced_state_for_test(
        &mut terminal,
        &mut app,
        &cx,
        None,
        |idle, frame, mut state| {
            assert!(idle.is_idle());
            let mut host_frame = host.project_frame();
            let mut frame = frame;
            let mut host_snapshot = host_state(&host);
            if root != app_root {
                normalize_write_state(&mut host_snapshot, root);
                normalize_write_state(&mut state, app_root);
                normalize_write_frame(&mut host_frame, root);
                normalize_write_frame(&mut frame, app_root);
                assert_write_repositories(root, app_root, id, step);
                let writes = |calls: &Arc<Mutex<VcsCalls>>| calls.lock().unwrap().acts.iter().filter(|a| write_actions.contains(a)).cloned().collect::<Vec<_>>();
                compare_dispatched_writes(&writes(&host_calls), &writes(&app_calls), &expected_acts)
                    .unwrap_or_else(|e| panic!("step {step}: {e}"));
                if let Some(((host_views, host_chips), (app_views, app_chips))) = refresh_before.take() {
                    let host = host_calls.lock().unwrap();
                    let app = app_calls.lock().unwrap();
                    assert_eq!(host.views - host_views, app.views - app_views, "step {step}: screen refresh count after write");
                    assert_eq!(host.chips - host_chips, app.chips - app_chips, "step {step}: chip refresh count after write");
                }
            }
            let picker_cursor = |snapshot: &ProjectStateSnapshot| {
                (snapshot.stack.last().map(String::as_str) == Some("graph_picker"))
                    .then_some(snapshot.cursor)
            };
            host_snapshot.graph = host_graph.snapshot(picker_cursor(&host_snapshot));
            state.graph = app_graph.snapshot(picker_cursor(&state));
            compare_states(&host_snapshot, &state).unwrap_or_else(|e| panic!("step {step}: screen/identity/filter/cursor/marks/prompt/menu/notice/graph: {e}"));
            assert_eq!(
                host_writes,
                clipboard.writes(),
                "step {step}: every clipboard write"
            );
            if let Some(row) = &state.selected {
                assert!(
                    state
                        .rows
                        .iter()
                        .any(|(r, selectable)| r == row && *selectable),
                    "a non-selectable row was selected"
                );
            }
            if root != app_root && cells(&host_frame, 0) != cells(&frame, 0) {
                for (side, frame) in [("host", &host_frame), ("app", &frame)] {
                    eprintln!("{side} complete frame ({}x{}):", frame.area.width, frame.area.height);
                    for y in 0..frame.area.height {
                        let row: String = (0..frame.area.width).map(|x| frame[(x, y)].symbol()).collect();
                        eprintln!("{y:02}: {row}");
                    }
                }
            }
            frames(&host_frame, &frame, &state, &lang, &fixture, &graph_fixture, host.app.show_help);
            check(step, &state, &host, &host_writes);
            let Some(next) = steps.next() else {
                return Next::End;
            };
            step += 1;
            let next = match next {
                Step::BeforeKey(before, key) => {
                    before(root);
                    if root != app_root { before(app_root); }
                    Step::Key(key)
                }
                Step::Write(key, action) => {
                    let counts = |calls: &Arc<Mutex<VcsCalls>>| { let calls = calls.lock().unwrap(); (calls.views, calls.chips) };
                    refresh_before = Some((counts(&host_calls), counts(&app_calls)));
                    expected_acts.push(action.into());
                    Step::Key(key)
                }
                Step::Policy(text) => {
                    policy_change = Some(text);
                    return Next::End;
                }
                other => other,
            };
            let event = match next {
                Step::Resize(width, height) => {
                    host.resize(width, height);
                    backend.lock().unwrap().resize(width, height);
                    AppEvent::Resize
                }
                Step::Key(key) => {
                    clear_test_clipboard();
                    let mut press = || {
                        host.press(key.code, key.modifiers);
                        drain(&mut host);
                    };
                    if failure {
                        App::with_clipboard_failure_for_test(press);
                    } else {
                        press();
                    }
                    if let Some(text) = get_test_clipboard() {
                        host_writes.push(text);
                    }
                    clear_test_clipboard();
                    AppEvent::Key(key_from_crossterm(&key).unwrap())
                }
                Step::BeforeKey(..) | Step::Write(..) | Step::Policy(..) => unreachable!(),
            };
            Next::Event(event)
        },
    )
    .unwrap();
        let Some(text) = policy_change else { break };
        let shared = SharedConfig::parse(&text).unwrap();
        let config: Config = toml::from_str(&text).unwrap();
        for (id, settings) in config.plugins {
            host.app.cfg.plugins.insert(id, settings);
        }
        for (id, settings) in shared.plugins {
            app.config.plugins.insert(id, settings);
        }
    }
    assert_eq!(step, count, "every input must reach its fence");
    TALLY.with(|t| {
        let (runs, fences) = t.get();
        t.set((runs + 1, fences + count + 1));
    });
}

#[test]
fn changes_log_detail_cells_and_state_match_host() {
    if isolated("changes_log_detail_cells_and_state_match_host") {
        return;
    }
    comparator_styles();
    let root = at_home(git_repo("vcs_app_changes", true));
    assert_eq!(crate::git::changed_files(&root).len(), 7);
    for lang in ["en", "ja"] {
        for scheme in ["vim", "less"] {
            for path in ["relative", "home", "full"] {
                for align in ["right", "inline"] {
                    let text = config("git", lang, scheme, path, align, "");
                    changes_case(&root, "git", &text);
                }
            }
        }
    }
    let detail = at_home(detail_repo());
    for lang in ["en", "ja"] {
        for scheme in ["vim", "less"] {
            let text = config(
                "git",
                lang,
                scheme,
                "relative",
                "right",
                "[keys.text]\nJ='navigate:down'\n",
            );
            detail_case(&detail, "git", &text, scheme);
        }
    }
    let filtered = at_home(repo(13));
    for lang in ["en", "ja"] {
        for align in ["right", "inline"] {
            let text = config("git", lang, "vim", "relative", align, "");
            let mut steps = vec![key(KeyCode::Tab), ch('G'), ch('/')];
            steps.extend(typed("作者"));
            steps.extend([
                key(KeyCode::Backspace),
                ch('者'),
                key(KeyCode::Left),
                key(KeyCode::Enter),
                key(KeyCode::Esc),
                ch('/'),
            ]);
            steps.extend(typed("__no_matching_row__"));
            steps.extend([key(KeyCode::Enter), key(KeyCode::Esc), ch('j'), ch('k')]);
            run(&filtered, "git", &text, steps, false, |_, state, _, _| {
                if state.query == "__no_matching_row__" {
                    assert_eq!(state.selected, None);
                }
            });
        }
    }
    // Neither an empty changes screen nor an unborn log has a selectable placeholder.
    for root in [at_home(repo(0)), at_home(git_repo("vcs_app_clean", false))] {
        let text = config("git", "en", "vim", "relative", "right", "");
        let steps = vec![
            ch('j'),
            ch('V'),
            key(KeyCode::Tab),
            ch('j'),
            ch('G'),
            ch('g'),
        ];
        run(&root, "git", &text, steps, false, |_, state, _, _| {
            if state.rows.iter().all(|(_, selectable)| !selectable) {
                assert_eq!(state.selected, None, "empty rows must not be selected");
            }
        });
    }
    let refresh = at_home(repo(3));
    let mut selected = None;
    let text = config("git", "en", "vim", "relative", "right", "");
    let mut steps = vec![key(KeyCode::Tab), ch('j'), ch('/')];
    steps.extend(typed("Summary"));
    steps.push(key(KeyCode::Enter));
    let before = steps.len();
    steps.push(Step::BeforeKey(
        Box::new(|p| commit(p, 3)),
        KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE),
    ));
    run(&refresh, "git", &text, steps, false, |step, state, _, _| {
        if step == before {
            selected = state.selected.clone();
        }
        if step == before + 1 {
            assert_eq!(
                state.selected, selected,
                "refresh must restore row identity, not the old index"
            );
            assert_eq!(state.cursor, 2);
        }
    });
    let big = at_home(repo(201));
    limit_case(&big, "git");
    for (root, id) in jj_cases("changes/log/detail") {
        for lang in ["en", "ja"] {
            let text = config(id, lang, "vim", "home", "inline", "");
            changes_case(&root, id, &text);
            detail_case(&root, id, &text, "vim");
        }
        let initial = backend(id).log(&root, 201).len();
        for i in initial..201 {
            if id == "git" {
                commit(&root, i);
            } else {
                super::vcs_log::jj(&root, &["new", "-m", &format!("limit {i}")]);
            }
        }
        limit_case(&root, id);
    }
    report_cases();
}

fn limit_case(root: &Path, id: &str) {
    let commits = backend(id).log(root, 201);
    assert_eq!(
        commits.len(),
        201,
        "the real backend must have the 201st commit"
    );
    let oldest = commits[200].id.clone();
    let text = config(id, "en", "vim", "relative", "right", "");
    run(
        root,
        id,
        &text,
        vec![
            key(KeyCode::Tab),
            ch('j'),
            ch('k'),
            key(KeyCode::PageDown),
            key(KeyCode::PageUp),
            ch('G'),
            ch('k'),
            ch('g'),
        ],
        false,
        |_, state, _, _| {
            if state.stack.last().map(String::as_str) == Some("log") {
                assert_eq!(state.rows.len(), 200, "log shows exactly 200 rows");
                assert!(
                    state.rows.iter().all(|(row, _)| row.value != oldest),
                    "the 201st commit is absent"
                );
            }
        },
    );
}

#[test]
fn copy_values_failures_and_cached_detail_match_host() {
    if isolated("copy_values_failures_and_cached_detail_match_host") {
        return;
    }
    comparator_styles();
    let root = at_home(detail_repo());
    for lang in ["en", "ja"] {
        copy_case(&root, "git", lang);
    }
    let empty = at_home(empty_subject_repo());
    let fixture = read_snapshot_for_test("vcs_app_chrome");
    let text = config("git", "en", "vim", "relative", "right", "");
    run(
        &empty,
        "git",
        &text,
        vec![
            key(KeyCode::Tab),
            ch('y'),
            ch('t'),
            key(KeyCode::Enter),
            ch('y'),
            ch('t'),
        ],
        false,
        |step, _, _, writes| {
            assert!(
                writes
                    .iter()
                    .all(|s| s == fixture["empty_subject"].as_str().unwrap()),
                "an empty subject is a valid copy"
            );
            if step == 6 {
                assert_eq!(
                    writes,
                    &[String::new(), String::new()],
                    "both empty-string writes must occur"
                );
            }
        },
    );
    for detail in [false, true] {
        let root = at_home(repo(1));
        let expected = metadata(&root, "git", 0).message;
        let mut steps = vec![key(KeyCode::Tab)];
        if detail {
            steps.push(key(KeyCode::Enter));
        }
        steps.extend([
            Step::BeforeKey(
                Box::new(remove_head_object),
                KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE),
            ),
            ch('m'),
        ]);
        let end = steps.len();
        run(
            &root,
            "git",
            &text,
            steps,
            false,
            |step, state, _, writes| {
                if step == end {
                    if detail {
                        assert_eq!(
                            writes,
                            std::slice::from_ref(&expected),
                            "removed commit detail still copies shown metadata"
                        );
                    } else {
                        assert!(writes.is_empty());
                        assert!(state.notice.is_some(), "missing commit must report failure");
                    }
                }
            },
        );
    }
    cached_case(&at_home(repo(2)), "git");
    for (root, id) in jj_cases("copy/cache/failure") {
        for lang in ["en", "ja"] {
            copy_case(&root, id, lang);
        }
        cached_case(&root, id);
        if id == "git" {
            empty_subject_commit(&root);
        } else {
            super::vcs_log::jj(&root, &["describe", "-m", ""]);
        }
        empty_copy_case(&root, id);
        // Removing real objects makes the log's metadata read fail; cached details remain usable.
        for detail in [false, true] {
            let broken = at_home(jj_repo(id == "git" || root.join(".git").exists()));
            missing_copy_case(&broken, id, detail);
        }
    }
    report_cases();
}

fn comparator_styles() {
    let expected = Buffer::empty(Rect::new(0, 0, 2, 2));
    for field in 0..3 {
        let mut actual = expected.clone();
        match field {
            0 => actual[(1, 1)].fg = Color::Red,
            1 => actual[(1, 1)].bg = Color::Blue,
            _ => actual[(1, 1)].modifier = Modifier::BOLD,
        }
        assert!(
            compare_frames(&expected, &actual).is_err(),
            "frame comparison must detect style field {field} on the last row"
        );
    }
}

fn changes_case(root: &Path, id: &str, text: &str) {
    let mut steps = vec![
        ch('j'),
        ch('k'),
        ch('V'),
        key(KeyCode::PageDown),
        key(KeyCode::PageUp),
        ch('G'),
        ch('g'),
        ch('/'),
    ];
    steps.extend(typed("日本語"));
    steps.extend([
        key(KeyCode::Left),
        key(KeyCode::Backspace),
        key(KeyCode::Enter),
        key(KeyCode::Esc),
        ch('/'),
    ]);
    steps.extend(typed("__no_matching_row__"));
    steps.extend([
        key(KeyCode::Enter),
        key(KeyCode::Esc),
        Step::Resize(48, 9),
        Step::Resize(180, 20),
    ]);
    // Exercise the real prompt, but cancel before any repository write.
    steps.push(ch(if id == "git" { 'c' } else { 's' }));
    if id == "git" {
        steps.extend(typed("cancel me"));
        steps.push(key(KeyCode::Backspace));
    }
    steps.extend([
        key(KeyCode::Esc),
        key(KeyCode::Tab),
        ch('G'),
        ch('g'),
        key(KeyCode::Enter),
        key(KeyCode::Tab),
        ch('q'),
        key(KeyCode::BackTab),
    ]);
    let end = steps.len();
    run(root, id, text, steps, false, |step, state, _, _| {
        assert!(
            state.marked.is_empty(),
            "changes/log do not declare marks: V must preserve the host's no-op"
        );
        if state.query == "__no_matching_row__" {
            assert_eq!(state.selected, None);
        }
        if state.stack.len() > 1 {
            assert_eq!(
                state.stack.last().unwrap(),
                "detail",
                "detail is pushed, never in the tab cycle"
            );
        }
        if [end - 3, end - 2].contains(&step) {
            assert_eq!(
                state.stack.last().unwrap(),
                "detail",
                "detail must open as a pushed screen and stay there on Tab"
            );
        }
    });
}

fn empty_copy_case(root: &Path, id: &str) {
    let text = config(id, "en", "vim", "relative", "right", "");
    run(
        root,
        id,
        &text,
        vec![
            key(KeyCode::Tab),
            ch('y'),
            ch('t'),
            key(KeyCode::Enter),
            ch('y'),
            ch('t'),
        ],
        false,
        |step, _, _, writes| {
            assert!(writes.iter().all(String::is_empty));
            if step == 6 {
                assert_eq!(
                    writes,
                    &[String::new(), String::new()],
                    "both empty-string copies must be written"
                );
            }
        },
    );
}

fn missing_copy_case(root: &Path, id: &str, detail: bool) {
    let expected = metadata(root, id, 0);
    let commit = expected.id.clone();
    let mut steps = vec![key(KeyCode::Tab)];
    if detail {
        steps.push(key(KeyCode::Enter));
    }
    steps.extend([
        Step::BeforeKey(
            Box::new(move |p| remove_commit_object(p, &commit)),
            KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE),
        ),
        ch('m'),
    ]);
    let end = steps.len();
    let text = config(id, "en", "vim", "relative", "right", "");
    run(root, id, &text, steps, false, |step, state, _, writes| {
        if step == end {
            if detail {
                assert_eq!(
                    writes,
                    std::slice::from_ref(&expected.message),
                    "missing commit detail copies shown metadata"
                );
            } else {
                assert!(writes.is_empty());
                assert!(state.notice.is_some());
            }
        }
    });
}

fn detail_case(root: &Path, id: &str, text: &str, scheme: &str) {
    let mut steps = vec![
        key(KeyCode::Tab),
        ch('j'),
        key(KeyCode::Enter),
        key(KeyCode::Tab),
        ch('j'),
        ch('J'),
        ch('k'),
        ch('G'),
        ch('g'),
        ch('l'),
        ch('h'),
        ch('0'),
        ch('$'),
        key(KeyCode::PageDown),
        key(KeyCode::PageUp),
    ];
    for c in ['d', 'u', 'f', 'b'] {
        steps.push(Step::Key(KeyEvent::new(
            KeyCode::Char(c),
            if scheme == "less" {
                KeyModifiers::NONE
            } else {
                KeyModifiers::CONTROL
            },
        )));
    }
    steps.extend([
        Step::Resize(48, 9),
        Step::Resize(180, 20),
        ch('s'),
        ch('s'),
        ch('q'),
        key(KeyCode::BackTab),
    ]);
    let mut before_remap = 0;
    run(root, id, text, steps, false, |step, state, host, _| {
        if step == 4 {
            assert_eq!(
                state.stack.last().unwrap(),
                "detail",
                "Tab on pushed detail must do nothing"
            );
        }
        if step == 5 {
            before_remap = state.scroll;
        }
        if step == 6 && text.contains("J='navigate:down'") {
            assert_eq!(
                state.scroll,
                before_remap + 1,
                "the shared remapped key must scroll the real detail"
            );
        }
        if state.stack.last().map(String::as_str) == Some("detail") {
            assert!(matches!(
                host.app.project_content().as_deref(),
                Some(ProjectContent::Diff(_))
            ));
        }
    });
}

fn backend(id: &str) -> &'static dyn konoma_vcs_core::vcs::Vcs {
    if id == "git" {
        &konoma_vcs_core::vcs::Git
    } else {
        &konoma_vcs_core::vcs::Jj
    }
}
fn metadata(root: &Path, id: &str, index: usize) -> konoma_vcs_core::git::CommitMeta {
    let row = &backend(id).log(root, 200)[index];
    backend(id).commit_meta(root, &row.id).unwrap()
}
fn copies(meta: &konoma_vcs_core::git::CommitMeta) -> Vec<String> {
    vec![
        meta.short.clone(),
        meta.id.clone(),
        meta.message.lines().next().unwrap_or("").into(),
        meta.message.clone(),
        meta.author.clone(),
        meta.date.clone(),
    ]
}
fn copy_case(root: &Path, id: &str, lang: &str) {
    let expected = copies(&metadata(root, id, 1));
    let expected: Vec<_> = expected.iter().chain(&expected).cloned().collect();
    let fixture = read_snapshot_for_test("vcs_app_chrome");
    let mut steps = vec![key(KeyCode::Tab), ch('j')];
    for detail in [false, true] {
        if detail {
            steps.push(key(KeyCode::Enter));
        }
        for key in fixture["copy_keys"].as_array().unwrap() {
            steps.extend([ch('y'), ch(key.as_str().unwrap().chars().next().unwrap())]);
        }
    }
    let end = steps.len();
    let text = config(id, lang, "vim", "relative", "right", "");
    run(root, id, &text, steps, false, |step, _, _, writes| {
        assert!(writes.len() <= expected.len());
        assert_eq!(
            writes,
            &expected[..writes.len()],
            "all six copy values must describe the selected/displayed commit"
        );
        if step == end {
            assert_eq!(writes.len(), 12, "all six menu items on log and detail");
        }
    });
    let steps = vec![
        key(KeyCode::Tab),
        ch('y'),
        ch('m'),
        key(KeyCode::Enter),
        ch('y'),
        ch('m'),
    ];
    run(root, id, &text, steps, true, |step, state, _, writes| {
        assert!(
            writes.is_empty(),
            "failed clipboard writes must not reach either sink"
        );
        if [3, 6].contains(&step) {
            assert!(state
                .notice
                .as_deref()
                .unwrap()
                .contains("test clipboard failure"));
        }
    });
}

fn cached_case(root: &Path, id: &str) {
    let ids: Vec<_> = backend(id)
        .log(root, 200)
        .iter()
        .take(2)
        .map(|r| r.id.clone())
        .collect();
    let expected = [ids[0].clone(), ids[1].clone(), ids[0].clone()];
    let text = config(id, "en", "vim", "relative", "right", "");
    let mut first = None;
    let mut steps = vec![
        key(KeyCode::Tab),
        key(KeyCode::Enter),
        ch('y'),
        ch('h'),
        ch('q'),
        ch('j'),
        key(KeyCode::Enter),
        ch('y'),
        ch('h'),
        ch('q'),
    ];
    let backend_id = id.to_owned();
    steps.push(Step::BeforeKey(
        Box::new(move |p| {
            if backend_id == "git" {
                commit(p, 20);
            } else {
                super::vcs_log::jj(p, &["new", "-m", "moved head"]);
            }
        }),
        KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE),
    ));
    steps.extend([key(KeyCode::Enter), ch('y'), ch('h')]);
    run(root, id, &text, steps, false, |step, _, host, writes| {
        if step == 2 {
            first = host.app.project_content();
        }
        if step == 12 {
            assert!(
                Arc::ptr_eq(
                    first.as_ref().unwrap(),
                    &host.app.project_content().unwrap()
                ),
                "reopened A must use the cached detail"
            );
        }
        assert_eq!(
            writes,
            &expected[..writes.len()],
            "cached detail copy must use shown metadata after HEAD moved"
        );
        if step == 14 {
            assert_eq!(writes.len(), 3);
        }
    });
}

fn jj_cases(scenario: &str) -> Vec<(std::path::PathBuf, &'static str)> {
    if !crate::vcs::jj::available() {
        eprintln!("NOT RUN: {scenario}: jj is unavailable; jj-only, colocated Git and colocated jj cases were not run");
        return Vec::new();
    }
    let jj = at_home(jj_repo(false));
    let git = at_home(jj_repo(true));
    let colocated_jj = at_home(jj_repo(true));
    eprintln!("RUN: {scenario}: jj-only, colocated Git, colocated jj");
    vec![(jj, "jj"), (git, "git"), (colocated_jj, "jj")]
}

#[test]
fn graph_base_visible_order_picker_and_cells_match_host() {
    if isolated("graph_base_visible_order_picker_and_cells_match_host") {
        return;
    }
    let root = at_home(super::vcs_log::graph_picker_fixture());
    let branch_tips: std::collections::HashMap<_, _> =
        konoma_vcs_core::git::branches_by_recency(&root)
            .into_iter()
            .map(|(name, _, _)| {
                let tip = konoma_vcs_core::git::branch_tip(&root, &name).unwrap();
                (name, tip)
            })
            .collect();
    for lang in ["en", "ja"] {
        for align in ["right", "inline"] {
            let text = config(
                "git",
                lang,
                "vim",
                "relative",
                align,
                "[keys.text]\nD='navigate:down'\n",
            );
            let mut steps = vec![
                key(KeyCode::Tab),
                key(KeyCode::Tab),
                ch('D'),
                ch('j'),
                ch('k'),
            ];
            steps.extend([
                ch('/'),
                ch('日'),
                ch('本'),
                key(KeyCode::Enter),
                key(KeyCode::Esc),
            ]);
            steps.extend([
                Step::Resize(48, 8),
                ch('G'),
                ch('g'),
                Step::Resize(180, 18),
                ch('s'),
                ch('j'),
                ch('s'),
                ch('r'),
                ch('y'),
                ch('h'),
                key(KeyCode::Enter),
                ch('y'),
                ch('m'),
                ch('q'),
                ch('x'),
                ch('x'),
                ch('b'),
                ch('/'),
                ch('b'),
                ch('x'),
                ch('a'),
                ch('c'),
                ch('j'),
                ch('x'),
                ch('J'),
                ch('K'),
                ch('J'),
                key(KeyCode::Enter),
                ch('b'),
                ch('a'),
                ch('q'),
                ch('b'),
                ch('J'),
                key(KeyCode::Esc),
                ch('b'),
                ch('?'),
                ch('q'),
                ch('q'),
            ]);
            let mut applied = None;
            let mut cancelled = None;
            let mut retained_order = None;
            let mut head_selection = None;
            run(&root, "git", &text, steps, false, |step, state, _, _| {
                if state.stack.last().map(String::as_str) == Some("graph_picker") {
                    assert!(!state.filtering, "the picker has no filter");
                    assert!(state.query.is_empty());
                    let graph = state.graph.as_ref().expect("observe the real graph state");
                    assert_eq!(graph.order.len(), 4);
                    assert_eq!(graph.picker_cursor, Some(state.cursor));
                }
                assert!(
                    state
                        .rows
                        .iter()
                        .all(|(row, selectable)| row.kind != "connector" || !selectable),
                    "connector rows must never be selectable"
                );
                if step == 2 {
                    assert!(state.rows.iter().any(|(row, _)| row.kind == "connector"));
                    let mut different = state.clone();
                    different
                        .graph
                        .as_mut()
                        .unwrap()
                        .visible
                        .push("__comparison_probe__".into());
                    assert!(
                        compare_states(state, &different).is_err(),
                        "the state comparison must include the graph visible set"
                    );
                    assert_eq!(state.graph.as_ref().unwrap().visible.len(), 4);
                }
                if step == 29 {
                    head_selection = Some(state.graph.as_ref().unwrap().tentative.clone());
                }
                if step == 30 {
                    assert_eq!(
                        Some(state.graph.as_ref().unwrap().tentative.clone()),
                        head_selection,
                        "HEAD is always visible"
                    );
                }
                if step == 20 {
                    applied = state.selected.clone();
                }
                if step == 24 {
                    assert_eq!(
                        state.selected, applied,
                        "detail returns to its graph cursor"
                    );
                }
                if step == 38 {
                    let graph = state.graph.as_ref().unwrap();
                    cancelled = Some(graph.visible.clone());
                    let first = graph
                        .order
                        .iter()
                        .find(|name| graph.visible.contains(name))
                        .unwrap();
                    assert_eq!(
                        graph.label.as_ref(),
                        Some(first),
                        "reorder derives the base from the new order"
                    );
                    assert_eq!(
                        graph.base.as_ref(),
                        branch_tips.get(first),
                        "reorder updates the base commit"
                    );
                }
                if step == 41 || step == 42 {
                    assert_eq!(
                        state.graph.as_ref().unwrap().visible,
                        cancelled.clone().unwrap(),
                        "cancel discards the tentative selection"
                    );
                }
                if step == 43 {
                    retained_order = Some(state.graph.as_ref().unwrap().order.clone());
                }
                if step == 45 {
                    assert_eq!(
                        Some(state.graph.as_ref().unwrap().order.clone()),
                        retained_order,
                        "cancel retains the reordered priority"
                    );
                    assert_eq!(
                        state.graph.as_ref().unwrap().tentative,
                        cancelled.clone().unwrap(),
                        "reopening discards cancelled tentative selection"
                    );
                }
            });
        }
    }
    for lang in ["en", "ja"] {
        for (cap, priority) in [(1, false), (0, true)] {
            let extra = format!("[keys.text]\nD='navigate:down'\n[plugins.git]\ngraph_max_branches={cap}\ngraph_base_branches={}\n", if priority { "['missing-branch','priority-branch-with-a-long-name']" } else { "[]" });
            let text = config("git", lang, "vim", "relative", "right", &extra);
            run(
                &root,
                "git",
                &text,
                vec![
                    key(KeyCode::Tab),
                    key(KeyCode::Tab),
                    ch('r'),
                    ch('b'),
                    ch('q'),
                ],
                false,
                |step, state, _, _| {
                    if step == 2 || step == 3 {
                        let graph = state.graph.as_ref().unwrap();
                        assert_eq!(
                            graph.visible.len(),
                            if cap == 0 { 4 } else { cap },
                            "graph branch cap"
                        );
                        if priority {
                            assert_eq!(
                                graph.order.first().map(String::as_str),
                                Some("priority-branch-with-a-long-name")
                            );
                            assert_eq!(
                                graph.label.as_deref(),
                                Some("priority-branch-with-a-long-name")
                            );
                            assert_eq!(
                                graph.base.as_ref(),
                                branch_tips.get("priority-branch-with-a-long-name")
                            );
                        }
                    }
                },
            );
        }
    }
    let big = at_home(repo(401));
    let text = config(
        "git",
        "en",
        "vim",
        "relative",
        "right",
        "[keys.text]\nD='navigate:down'\n",
    );
    run(
        &big,
        "git",
        &text,
        vec![key(KeyCode::Tab), key(KeyCode::Tab), ch('G'), ch('g')],
        false,
        |_, state, _, _| {
            if state.stack.last().map(String::as_str) == Some("graph") {
                assert_eq!(
                    state
                        .rows
                        .iter()
                        .filter(|(row, _)| row.kind == "commit")
                        .count(),
                    400,
                    "graph commit row cap"
                );
            }
        },
    );
    report_cases();
}

#[test]
fn jj_graph_and_bookmarks_offer_only_real_actions() {
    if !crate::vcs::jj::available() {
        eprintln!(
            "NOT RUN: graph/bookmarks: jj unavailable; 8 sessions (jj-only and colocated Git/jj, en/ja) not run; 0 sessions run"
        );
    }
    if !crate::vcs::jj::available() {
        return;
    }
    if isolated("jj_graph_and_bookmarks_offer_only_real_actions") {
        return;
    }
    let root = at_home(super::vcs_log::graph_jj_fixture());
    jj(&root, &["bookmark", "create", "-r", "@-", "prior"]);
    for (case, (root, id)) in std::iter::once((root, "jj"))
        .chain(jj_cases("graph/bookmarks"))
        .enumerate()
    {
        if id == "jj" {
            super::vcs_changes::add_jj_bookmarks(&root);
        }
        for lang in ["en", "ja"] {
            let text = config(
                id,
                lang,
                "vim",
                "relative",
                "right",
                "[keys.text]\nD='navigate:down'\n",
            );
            let mut steps = vec![
                key(KeyCode::Tab),
                key(KeyCode::Tab),
                Step::Resize(180, 18),
                ch('a'),
                ch('a'),
                ch('s'),
                ch('x'),
                ch('?'),
                ch('q'),
                ch('b'),
                ch('/'),
                ch('q'),
                key(KeyCode::Tab),
                ch('j'),
                ch('g'),
                ch('y'),
                ch('n'),
                key(KeyCode::Enter),
                ch('c'),
                ch('d'),
                ch('?'),
                ch('q'),
            ];
            if id == "git" {
                steps.truncate(12);
            }
            let mut range = None;
            run(
                &root,
                id,
                &text,
                steps,
                false,
                |step, state, host, writes| {
                    if id == "jj" && state.stack.last().map(String::as_str) == Some("graph") {
                        assert!(
                            state.graph.as_ref().unwrap().base.is_none(),
                            "jj has no graph base"
                        );
                        if step == 3 {
                            range = Some(state.rows.clone());
                        }
                        if step == 4 {
                            assert!(state.graph.as_ref().unwrap().all);
                            assert!(state.notice.is_some(), "all/range has a flash");
                            if case == 0 {
                                assert!(
                                    state.rows.len() > range.as_ref().unwrap().len(),
                                    "all expands the default jj range"
                                );
                            }
                        }
                        if step == 5 {
                            assert_eq!(Some(state.rows.clone()), range);
                        }
                    }
                    if id == "jj" && step == 17 && !state.rows.is_empty() {
                        assert_eq!(writes.last(), state.selected.as_ref().map(|row| &row.value));
                    }
                    if id == "jj" {
                        assert!(state.prompt.is_none(), "jj offers no write prompts");
                        let hints =
                            crate::ops::footer::tokens(&host.app, crate::keymap::Context::Canvas);
                        let forbidden: &[&str] = match state.stack.last().map(String::as_str) {
                            Some("graph") => &["s", "x"],
                            Some("branches") => &["c", "d", "Enter", "↵"],
                            _ => &[],
                        };
                        assert!(
                            !hints.iter().any(|hint| hint.split_once(':').is_some_and(
                                |(keys, _)| keys.split('/').any(|key| forbidden.contains(&key))
                            )),
                            "jj has no base/write hints: {hints:?}"
                        );
                    }
                },
            );
        }
    }
    report_cases();
}
