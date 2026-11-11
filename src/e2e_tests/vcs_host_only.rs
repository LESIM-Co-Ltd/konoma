//! Host contracts that have no standalone counterpart. Always enter via the real p key.
use super::vcs_changes::drain;
use super::*;
use konoma_plugin::host::project::ProjectContent;
use konoma_plugin::view::ListView;

fn isolated(name: &str) -> bool {
    isolate_test("vcs_host_only", name)
}

pub(super) fn isolate_test(module: &str, name: &str) -> bool {
    if std::env::var("KONOMA_VCS_HOST_CHILD").as_deref() == Ok(name) {
        return false;
    }
    let home = unique_tmp("vcs_host_home");
    std::fs::create_dir_all(&home).unwrap();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            &format!("e2e_tests::{module}::{name}"),
            "--nocapture",
        ])
        .env("KONOMA_VCS_HOST_CHILD", name)
        .env("HOME", home.canonicalize().unwrap())
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_DATA_HOME", home.join("data"))
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("JJ_CONFIG", "")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "{stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(stdout.lines().any(|s| s == "running 1 test"));
    assert!(stdout
        .lines()
        .any(|s| s.starts_with("test result: ok. 1 passed;")));
    true
}

fn open(root: &std::path::Path, id: &str, cap: usize, priority: bool) -> Sim {
    let mut cfg = Config::default();
    cfg.ui.lang = "en".into();
    cfg.ui.tabbar = "hidden".into();
    cfg.ui.statusbar = "split".into();
    cfg.plugins
        .entry(if id == "git" { "jj" } else { "git" }.into())
        .or_default()
        .insert("enabled".into(), toml::Value::Boolean(false));
    let settings = cfg.plugins.entry(id.into()).or_default();
    settings.insert(
        "graph_max_branches".into(),
        toml::Value::Integer(cap as i64),
    );
    if priority {
        settings.insert(
            "graph_base_branches".into(),
            toml::Value::Array(vec![
                toml::Value::String("missing-branch".into()),
                toml::Value::String("priority-branch-with-a-long-name".into()),
            ]),
        );
    }
    let mut sim = Sim::with_config_sized(root, cfg, 160, 16);
    enter(&mut sim);
    sim
}

fn enter(sim: &mut Sim) {
    if sim.app.tab.projects.pending() {
        sim.app.tab.projects.wait_for_result();
    }
    sim.app.poll_projects();
    let (_, legacy) = App::count_legacy_hub_opens(|| sim.key('p'));
    drain(sim);
    assert_eq!(legacy, 0);
    assert!(sim.app.project_active());
}

fn graph(sim: &mut Sim) {
    for _ in 0..2 {
        sim.press(KeyCode::Tab, KeyModifiers::NONE);
        drain(sim);
    }
    assert_eq!(sim.app.project_frame().unwrap().key.screen, "graph");
}

fn list(sim: &Sim) -> ListView {
    let content = sim.app.project_content().unwrap();
    let ProjectContent::List(list) = content.as_ref() else {
        panic!("list required")
    };
    list.clone()
}

fn keys(sim: &mut Sim, sequence: &str) {
    for key in sequence.chars() {
        sim.key(key);
        drain(sim);
    }
}

#[test]
fn vcs_host_picker_visible_set_is_per_tab() {
    if isolated("vcs_host_picker_visible_set_is_per_tab") {
        return;
    }
    let root = super::vcs_log::graph_picker_fixture();
    let mut sim = open(&root, "git", 12, false);
    graph(&mut sim);
    let original = list(&sim);
    keys(&mut sim, "bjxl");
    let filtered = list(&sim);
    assert_ne!(filtered, original, "tab 0 applies its own visible set");
    sim.app.tab_new().unwrap();
    enter(&mut sim);
    graph(&mut sim);
    assert_eq!(
        list(&sim),
        original,
        "tab 1 starts with its own visible set"
    );
    sim.app.tab_goto(0);
    drain(&mut sim);
    assert_eq!(
        list(&sim),
        filtered,
        "tab 0 retains its applied visible set"
    );
}

#[test]
fn vcs_host_tab_change_discards_tentative_picker_but_keeps_order() {
    if isolated("vcs_host_tab_change_discards_tentative_picker_but_keeps_order") {
        return;
    }
    let root = super::vcs_log::graph_picker_fixture();
    let mut reference = open(&root, "git", 12, true);
    graph(&mut reference);
    keys(&mut reference, "bJq");
    let expected_graph = reference.project_frame();
    keys(&mut reference, "b");
    let expected_picker = reference.project_frame();
    let mut sim = open(&root, "git", 12, true);
    graph(&mut sim);
    let original = list(&sim);
    keys(&mut sim, "bxJ");
    let tentative = list(&sim);
    assert!(sim.app.project_pushed());
    sim.app.tab_new().unwrap();
    enter(&mut sim);
    graph(&mut sim);
    assert!(!sim.app.project_pushed(), "new tab has no picker");
    assert_eq!(list(&sim), original);
    sim.app.tab_goto(0);
    drain(&mut sim);
    assert!(
        !sim.app.project_pushed(),
        "tab change closes the original picker"
    );
    assert_eq!(list(&sim), original, "tentative visibility was not applied");
    super::project_frames::compare_frames(&expected_graph, &sim.project_frame()).unwrap();
    keys(&mut sim, "b");
    super::project_frames::compare_frames(&expected_picker, &sim.project_frame()).unwrap();
    let reopened = list(&sim);
    assert_ne!(reopened, tentative, "tentative visibility is discarded");
    keys(&mut sim, "q");
    keys(&mut sim, "b");
    assert_eq!(
        list(&sim),
        reopened,
        "reordered priority is retained on reopen"
    );
}

#[test]
fn vcs_host_base_order_and_cached_view_are_per_tab() {
    if isolated("vcs_host_base_order_and_cached_view_are_per_tab") {
        return;
    }
    let root = super::vcs_log::graph_picker_fixture();
    let mut sim = open(&root, "git", 1, false);
    graph(&mut sim);
    let a = sim.app.tab.id;
    keys(&mut sim, "js");
    let cached = sim.app.project_content().unwrap();
    let pinned = list(&sim);
    assert!(pinned.title.as_ref().unwrap().plain().contains("base:"));
    assert!(pinned
        .bottom
        .as_ref()
        .unwrap()
        .chips
        .last()
        .unwrap()
        .plain()
        .contains("hidden"));
    keys(&mut sim, "bJq");
    let reordered = list(&sim);
    sim.app.tab_new().unwrap();
    assert_ne!(a, sim.app.tab.id);
    enter(&mut sim);
    graph(&mut sim);
    assert!(!list(&sim).title.as_ref().unwrap().plain().contains("base:"));
    keys(&mut sim, "r");
    sim.app.tab_goto(0);
    drain(&mut sim);
    assert_eq!(
        list(&sim),
        reordered,
        "tab 1 reload preserves tab 0 base/order/view"
    );
    assert_eq!(list(&sim), pinned);
    assert!(std::sync::Arc::ptr_eq(
        &cached,
        &sim.app.project_content().unwrap()
    ));
}

#[test]
fn vcs_host_close_notifies_and_discards_only_the_closed_tab() {
    if isolated("vcs_host_close_notifies_and_discards_only_the_closed_tab") {
        return;
    }
    use crate::plugins::test_projects::{ObservedVcs, VcsCalls};
    use std::sync::{Arc, Mutex};
    let root = super::vcs_log::graph_picker_fixture();
    for inactive in [false, true] {
        let calls = Arc::new(Mutex::new(VcsCalls::default()));
        let token = crate::plugins::test_plugins::register_replacement(
            &format!("vcs_host_close_{inactive}"),
            Arc::new(ObservedVcs {
                plugin: Arc::new(konoma_vcs::Git),
                calls: calls.clone(),
            }),
        );
        let mut cfg = crate::plugins::test_plugins::marker_config(&token);
        cfg.ui.lang = "en".into();
        let mut sim = Sim::with_config_sized(&root, cfg, 160, 16);
        enter(&mut sim);
        graph(&mut sim);
        let a = sim.app.tab.id;
        let original = list(&sim);
        sim.app.tab_new().unwrap();
        enter(&mut sim);
        graph(&mut sim);
        let b = sim.app.tab.id;
        assert!(sim.app.project_cached_tabs_for_test().contains(&a));
        assert!(sim.app.project_cached_tabs_for_test().contains(&b));
        if inactive {
            sim.app.tab_goto(0);
            sim.app.toggle_tab_list();
            sim.app.tab_list_move(1);
            sim.app.tab_list_close_selected();
            sim.app.toggle_tab_list();
        } else {
            sim.app.tab_close();
        }
        drain(&mut sim);
        assert_eq!(calls.lock().unwrap().closed_tabs, [b]);
        let tabs = sim.app.project_cached_tabs_for_test();
        assert!(tabs.contains(&a));
        assert!(!tabs.contains(&b));
        assert_eq!(list(&sim), original);
        keys(&mut sim, "r");
        assert!(!sim.app.project_cached_tabs_for_test().contains(&b));
    }
}

fn return_to_origin(root: &std::path::Path, id: &str) {
    for preview in [false, true] {
        for code in [KeyCode::Char('h'), KeyCode::Left] {
            let mut sim = open(root, id, 12, false);
            keys(&mut sim, "q");
            if preview {
                sim.select("origin.txt");
                sim.enter();
            }
            let origin = sim.app.tab.mode;
            let path = sim.app.tab.preview_path.clone();
            enter(&mut sim);
            graph(&mut sim);
            sim.press(code, KeyModifiers::NONE);
            drain(&mut sim);
            assert!(!sim.app.project_active(), "{id} {code:?}");
            assert_eq!(sim.app.tab.mode, origin);
            assert_eq!(sim.app.tab.preview_path, path);
        }
    }
}

#[test]
fn vcs_host_git_graph_h_and_left_return_to_tree_and_preview() {
    if isolated("vcs_host_git_graph_h_and_left_return_to_tree_and_preview") {
        return;
    }
    let root = super::vcs_log::graph_picker_fixture();
    std::fs::write(root.join("origin.txt"), "origin\n").unwrap();
    return_to_origin(&root, "git");
}

#[test]
fn vcs_host_jj_graph_h_and_left_return_to_tree_and_preview() {
    if !crate::vcs::jj::available() {
        return;
    }
    if isolated("vcs_host_jj_graph_h_and_left_return_to_tree_and_preview") {
        return;
    }
    let root = super::vcs_log::jj_repo(false);
    std::fs::write(root.join("origin.txt"), "origin\n").unwrap();
    return_to_origin(&root, "jj");
}

#[test]
fn vcs_host_unborn_log_fixed_tabs_and_q_origin() {
    if isolated("vcs_host_unborn_log_fixed_tabs_and_q_origin") {
        return;
    }
    let root = super::vcs_log::repo(0);
    std::fs::write(root.join("origin.txt"), "origin\n").unwrap();
    for preview in [false, true] {
        let mut sim = open(&root, "git", 12, false);
        keys(&mut sim, "q");
        if preview {
            sim.select("origin.txt");
            sim.enter();
        }
        let origin = sim.app.tab.mode;
        enter(&mut sim);
        for (code, screen) in [
            (KeyCode::Tab, "log"),
            (KeyCode::Tab, "graph"),
            (KeyCode::Tab, "branches"),
            (KeyCode::Tab, "worktrees"),
            (KeyCode::Tab, "changes"),
            (KeyCode::BackTab, "worktrees"),
            (KeyCode::BackTab, "branches"),
            (KeyCode::BackTab, "graph"),
            (KeyCode::BackTab, "log"),
        ] {
            sim.press(code, KeyModifiers::NONE);
            drain(&mut sim);
            assert_eq!(sim.app.project_frame().unwrap().key.screen, screen);
            assert!(!sim.screen().contains("Detail"));
        }
        assert!(sim.app.project_act_context().selected.is_none());
        assert!(!sim.app.project_controls().unwrap().contains("/l/"));
        for code in [KeyCode::Enter, KeyCode::Char('l'), KeyCode::Right] {
            sim.press(code, KeyModifiers::NONE);
            drain(&mut sim);
            assert_eq!(sim.app.project_frame().unwrap().key.screen, "log");
        }
        for code in [KeyCode::Char('h'), KeyCode::Left] {
            sim.press(code, KeyModifiers::NONE);
            drain(&mut sim);
            assert_eq!(sim.app.project_frame().unwrap().key.screen, "log");
            assert!(sim.app.pending_leader.is_none());
        }
        keys(&mut sim, "q");
        assert!(!sim.app.project_active());
        assert_eq!(sim.app.tab.mode, origin);
    }
}

#[test]
fn vcs_host_jj_all_range_is_per_tab() {
    if !crate::vcs::jj::available() {
        return;
    }
    if isolated("vcs_host_jj_all_range_is_per_tab") {
        return;
    }
    let root = super::vcs_log::graph_jj_fixture();
    let mut sim = open(&root, "jj", 12, false);
    graph(&mut sim);
    let range = list(&sim);
    keys(&mut sim, "a");
    let all = list(&sim);
    assert!(all.rows.len() > range.rows.len());
    sim.app.tab_new().unwrap();
    enter(&mut sim);
    graph(&mut sim);
    assert_eq!(list(&sim), range);
    sim.app.tab_goto(0);
    drain(&mut sim);
    assert_eq!(list(&sim), all);
    keys(&mut sim, "a");
    assert_eq!(list(&sim), range);
    for code in ['s', 'x'] {
        sim.key(code);
        drain(&mut sim);
        assert_eq!(list(&sim), range);
    }
}

#[test]
fn vcs_host_log_q_and_escape_return_to_the_origin() {
    if isolated("vcs_host_log_q_and_escape_return_to_the_origin") {
        return;
    }
    let root = super::vcs_log::repo(3);
    std::fs::write(root.join("origin.txt"), "origin\n").unwrap();
    for preview in [false, true] {
        for code in [KeyCode::Char('q'), KeyCode::Esc] {
            let mut sim = open(&root, "git", 12, false);
            keys(&mut sim, "q");
            if preview {
                sim.select("origin.txt");
                sim.enter();
            }
            let origin = sim.app.tab.mode;
            let path = sim.app.tab.preview_path.clone();
            let selected = sim.app.tab.selected;
            enter(&mut sim);
            sim.press(KeyCode::Tab, KeyModifiers::NONE);
            drain(&mut sim);
            keys(&mut sim, "h");
            assert_eq!(sim.app.project_frame().unwrap().key.screen, "log");
            assert!(sim.app.pending_leader.is_none());
            sim.press(code, KeyModifiers::NONE);
            drain(&mut sim);
            assert!(!sim.app.project_active());
            assert_eq!(sim.app.tab.mode, origin);
            assert_eq!(sim.app.tab.preview_path, path);
            assert_eq!(sim.app.tab.selected, selected);
        }
    }
}
