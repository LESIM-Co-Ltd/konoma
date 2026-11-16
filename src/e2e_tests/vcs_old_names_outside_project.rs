//! An old hub action name bound on a surface that is not a project canvas (`[keys.tree]`,
//! `[keys.preview_text]`): the name is read as the plugin operation that `plugins/old_screens.rs`
//! maps it to, through the project the way `p` opens it, and no state of the old hub is raised.
//! Every expected value is written out; a name that needed a hub row still does nothing.
use super::vcs_changes::drain;
use super::*;
use crate::plugins::old_screens::ROWS;

const KEY: char = 'Z';
const RO: &str = "konoma only reads a jj repository — use jj itself to change it";
const NO_COMMIT: &str = "no commit to copy";

fn isolated(name: &str) -> bool {
    super::vcs_host_only::isolate_test("vcs_old_names_outside_project", name)
}

fn cfg_for(id: &str, table: &str, action: &str) -> Config {
    let mut cfg = Config::default();
    cfg.ui.lang = "en".into();
    cfg.ui.tabbar = "hidden".into();
    cfg.ui.statusbar = "split".into();
    cfg.plugins
        .entry(if id == "git" { "jj" } else { "git" }.into())
        .or_default()
        .insert("enabled".into(), toml::Value::Boolean(false));
    cfg.keys
        .surfaces
        .insert(table.into(), [(KEY.to_string(), action.to_string())].into());
    cfg
}

/// A fresh repository of the backend (`None`: jj is not installed).
fn repo(id: &str, tag: &str) -> Option<std::path::PathBuf> {
    let name = format!("vcs_old_names_{id}_{tag}");
    let dir = if id == "git" {
        super::vcs_changes::git_repo(&name, true)
    } else {
        crate::test_support::jj_scratch_seeded(&name)?
    };
    let path = dir.to_path_buf();
    // Kept until the test process exits (the guard would remove the repository under the app).
    std::mem::forget(dir);
    Some(path)
}

/// What one press visibly did, as text: the project and its screen, the flash, the surface, the
/// dialog of the host and the prompt of the project, the launch of the tool, and the seven
/// predicates of the old hub.
fn effect(sim: &mut Sim) -> String {
    let launched = sim.app.take_launch_git_tool();
    let a = &sim.app;
    format!(
        "project={} screen={} flash={:?} surface={:?} dialog={} prompt={} launched={launched} \
         hub[view={} log={} branches={} worktrees={} graph={} picker={} detail={}]",
        a.project_active(),
        a.project_frame()
            .map(|f| f.key.screen.clone())
            .unwrap_or_default(),
        a.flash,
        a.surface(),
        a.is_dialog(),
        a.project_dialog.is_some(),
        a.is_git_view(),
        a.is_git_log(),
        a.is_git_branches(),
        a.is_git_worktrees(),
        a.is_git_graph(),
        a.is_git_graph_picker(),
        a.is_git_detail(),
    )
}

const HUB_OFF: &str =
    "hub[view=false log=false branches=false worktrees=false graph=false picker=false detail=false]";

/// The surface is left as it was and nothing happens but the flash (if any).
fn stays(surface: &str, flash: Option<&str>) -> String {
    format!(
        "project=false screen= flash={flash:?} surface={surface} dialog=false prompt=false \
         launched=false {HUB_OFF}"
    )
}

/// The project is open on `screen` the way `p` opens it; `prompt`: the operation asked for input.
fn canvas(screen: &str, flash: Option<&str>, prompt: bool) -> String {
    format!(
        "project=true screen={screen} flash={flash:?} surface=PreviewCanvas dialog=false \
         prompt={prompt} launched=false {HUB_OFF}"
    )
}

/// Press `KEY` bound to `name` in `table` (a text preview of a file when `preview`).
fn press_bound(id: &str, table: &str, name: &str, preview: bool) -> Option<String> {
    let root = repo(id, "x")?;
    let mut sim = Sim::with_config_sized(&root, cfg_for(id, table, name), 120, 30);
    sim.app.tab.projects.wait_for_result();
    sim.app.poll_projects();
    if preview {
        sim.select(".txt");
        sim.press(KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(sim.app.surface(), crate::keymap::Surface::PreviewText);
    }
    sim.key(KEY);
    drain(&mut sim);
    Some(effect(&mut sim))
}

/// `(old name, git on the tree, jj on the tree)`. A name that opens or runs something is a canvas;
/// a name that needed a hub row, and a name the backend has no counterpart for, leaves the tree as
/// it was (the flash is the write guard's or the handler's own, as before).
fn tree_cases() -> Vec<(&'static str, String, String)> {
    let t = |flash| stays("Tree", flash);
    let same = |flash| (t(flash), t(flash));
    let ro = |git: String| (git, t(Some(RO)));
    let mut v: Vec<(&str, (String, String))> = vec![
        ("git_graph_picker_apply", same(None)),
        ("git_diff_discard", ro(t(None))),
        ("cycle_diff_view", same(None)),
        ("toggle_follow_diff_scope", same(None)),
        ("diff_next_change", same(None)),
        ("diff_prev_change", same(None)),
        ("git_stage", ro(t(None))),
        ("git_unstage", ro(t(None))),
        (
            "git_stage_all",
            ro(canvas("changes", Some("Staged all (7)"), false)),
        ),
        (
            "git_unstage_all",
            ro(canvas("changes", Some("Unstaged all"), false)),
        ),
        ("git_discard", ro(t(None))),
        ("git_commit", ro(canvas("changes", None, true))),
        (
            "git_worktree_diff",
            (
                canvas("detail", None, false),
                canvas("changes", Some("no changes"), false),
            ),
        ),
        (
            "git_open_log",
            (canvas("log", None, false), canvas("log", None, false)),
        ),
        (
            "git_open_graph",
            (canvas("graph", None, false), canvas("graph", None, false)),
        ),
        (
            "git_open_branches",
            (
                canvas("branches", None, false),
                canvas("branches", None, false),
            ),
        ),
        (
            "git_launch_tool",
            (
                stays("Tree", None).replace("launched=false", "launched=true"),
                stays("Tree", None).replace("launched=false", "launched=true"),
            ),
        ),
        ("git_open_selected_diff", same(None)),
        ("git_open_detail", same(None)),
        ("git_graph_set_base", same(None)),
        ("git_graph_clear_base", same(None)),
        (
            "git_graph_toggle_all",
            (
                t(None),
                canvas("graph", Some("showing every revision"), false),
            ),
        ),
        ("jj_sync", (t(None), canvas("changes", None, true))),
        ("git_graph_open_picker", same(None)),
        ("git_graph_picker_toggle", same(None)),
        ("git_graph_picker_all", same(None)),
        ("git_graph_picker_current_only", same(None)),
        ("git_graph_picker_move_up", same(None)),
        ("git_graph_picker_move_down", same(None)),
        ("git_screen_next", same(None)),
        ("git_screen_prev", same(None)),
        ("branch_filter_start", same(None)),
        ("branch_checkout", ro(t(None))),
        ("branch_create", ro(canvas("branches", None, true))),
        ("branch_delete", ro(t(None))),
        (
            "git_open_worktrees",
            (canvas("worktrees", None, false), t(None)),
        ),
        ("worktree_filter_start", same(None)),
        ("worktree_goto", same(None)),
        ("worktree_goto_new_tab", same(None)),
        ("worktree_show_changes", same(None)),
        ("worktree_create", ro(canvas("worktrees", None, true))),
    ];
    for name in [
        "git_copy_short_hash",
        "git_copy_full_hash",
        "git_copy_subject",
        "git_copy_message",
        "git_copy_author",
        "git_copy_date",
    ] {
        v.push((name, same(Some(NO_COMMIT))));
    }
    v.push(("copy_branch_name", same(Some("no copy target"))));
    v.push(("git_close", same(None)));
    v.into_iter().map(|(n, (g, j))| (n, g, j)).collect()
}

/// Names that are not in the table above on purpose. `worktree_close` is not one of them: it has
/// its own cases below (it does what `p` does off a canvas).
const EXCLUDED: &[&str] = &[];

#[test]
fn vcs_old_names_table_covers_every_row_of_the_old_screens_table() {
    let cases = tree_cases();
    let mut seen = 0;
    for row in ROWS {
        if EXCLUDED.contains(&row.old) {
            continue;
        }
        if row.old == "worktree_close" {
            continue;
        }
        assert!(
            cases.iter().any(|(n, _, _)| *n == row.old),
            "{} has no case",
            row.old
        );
        seen += 1;
    }
    assert_eq!(cases.len(), seen, "a case without a row");
    assert_eq!(seen, 49, "the number of names under test");
    assert!(ROWS.iter().any(|r| r.old == "worktree_close"));
}

fn run_tree(id: &str) {
    let mut checked = 0;
    for (name, git, jj) in tree_cases() {
        let want = if id == "git" { git } else { jj };
        let Some(got) = press_bound(id, "tree", name, false) else {
            return;
        };
        assert_eq!(got, want, "{id} `{name}` bound in [keys.tree]");
        checked += 1;
    }
    assert_eq!(checked, 49);
}

#[test]
fn vcs_old_names_every_name_on_the_tree_in_a_git_repository() {
    if isolated("vcs_old_names_every_name_on_the_tree_in_a_git_repository") {
        return;
    }
    run_tree("git");
}

#[test]
fn vcs_old_names_every_name_on_the_tree_in_a_jj_repository() {
    if isolated("vcs_old_names_every_name_on_the_tree_in_a_jj_repository") {
        return;
    }
    if repo("jj", "probe").is_none() {
        eprintln!("SKIPPED: jj is not installed");
        return;
    }
    run_tree("jj");
}

#[test]
fn vcs_old_names_on_a_text_preview_open_the_project_like_the_tree() {
    if isolated("vcs_old_names_on_a_text_preview_open_the_project_like_the_tree") {
        return;
    }
    for (name, want) in [
        ("git_open_log", canvas("log", None, false)),
        ("git_open_graph", canvas("graph", None, false)),
        ("git_open_branches", canvas("branches", None, false)),
        ("git_open_worktrees", canvas("worktrees", None, false)),
        ("git_worktree_diff", canvas("detail", None, false)),
        ("git_commit", canvas("changes", None, true)),
        (
            "git_stage_all",
            canvas("changes", Some("Staged all (7)"), false),
        ),
        ("git_stage", stays("PreviewText", None)),
        ("git_open_detail", stays("PreviewText", None)),
    ] {
        let got = press_bound("git", "preview_text", name, true).unwrap();
        assert_eq!(got, want, "git `{name}` bound in [keys.preview_text]");
    }
}

#[test]
fn vcs_old_names_outside_a_repository_answer_like_p() {
    if isolated("vcs_old_names_outside_a_repository_answer_like_p") {
        return;
    }
    for name in ["git_open_log", "git_commit", "git_worktree_diff"] {
        let dir = crate::test_support::unique_tmp("vcs_old_names_no_repo");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), "x\n").unwrap();
        let cfg = cfg_for("git", "tree", name);
        let mut sim = Sim::with_config_sized(&dir, cfg, 120, 30);
        sim.app.tab.projects.wait_for_result();
        sim.app.poll_projects();
        let mut p_sim = Sim::with_config_sized(&dir, cfg_for("git", "tree", "noop"), 120, 30);
        p_sim.app.tab.projects.wait_for_result();
        p_sim.app.poll_projects();
        p_sim.key('p');
        drain(&mut p_sim);
        sim.key(KEY);
        drain(&mut sim);
        assert_eq!(effect(&mut sim), effect(&mut p_sim), "`{name}` vs p");
        assert_eq!(sim.app.flash.as_deref(), Some("not a git repo"), "`{name}`");
    }
}

// ---- worktree_close: off a canvas it does what `p` does ----

const P_NAME: &str = "open_git_view";

#[test]
fn vcs_old_names_worktree_close_off_a_canvas_does_what_p_does() {
    if isolated("vcs_old_names_worktree_close_off_a_canvas_does_what_p_does") {
        return;
    }
    for id in ["git", "jj"] {
        if repo(id, "probe").is_none() {
            eprintln!("SKIPPED: jj is not installed");
            continue;
        }
        for (table, preview) in [("tree", false), ("preview_text", true)] {
            let got = press_bound(id, table, "worktree_close", preview).unwrap();
            let p = press_bound(id, table, P_NAME, preview).unwrap();
            assert_eq!(got, p, "{id} `worktree_close` in [keys.{table}] vs p");
            assert_eq!(
                got,
                canvas("changes", None, false),
                "{id} `worktree_close` in [keys.{table}]"
            );
        }
    }
}

#[test]
fn vcs_old_names_worktree_close_outside_a_repository_answers_like_p() {
    if isolated("vcs_old_names_worktree_close_outside_a_repository_answers_like_p") {
        return;
    }
    let dir = crate::test_support::unique_tmp("vcs_old_names_close_no_repo");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.txt"), "x\n").unwrap();
    let mut got = Vec::new();
    for name in ["worktree_close", P_NAME] {
        let mut sim = Sim::with_config_sized(&dir, cfg_for("git", "tree", name), 120, 30);
        sim.app.tab.projects.wait_for_result();
        sim.app.poll_projects();
        sim.key(KEY);
        drain(&mut sim);
        got.push(effect(&mut sim));
    }
    assert_eq!(got[0], got[1], "worktree_close vs p");
    assert_eq!(got[0], stays("Tree", Some("not a git repo")));
}

// ---- the project is not known yet when the key is pressed ----

fn both_enabled(keys: &[(&str, &str)]) -> Config {
    let mut cfg = Config::default();
    cfg.ui.lang = "en".into();
    cfg.ui.tabbar = "hidden".into();
    cfg.ui.statusbar = "split".into();
    for (key, action) in keys {
        cfg.keys
            .surfaces
            .entry("tree".into())
            .or_default()
            .insert(key.to_string(), action.to_string());
    }
    cfg
}

/// The action bound to `KEY` dispatched without `handle_key` (which takes a finished detection
/// answer first) and without the redraw that `Sim::press` ends with: detection stays pending.
fn press_undrawn(sim: &mut Sim, _key: char) {
    let name = sim
        .app
        .cfg
        .keys
        .surfaces
        .get("tree")
        .and_then(|t| t.get(&KEY.to_string()))
        .cloned()
        .unwrap();
    let action = crate::keymap::action_from_str(&name).unwrap();
    crate::dispatch_action(&mut sim.app, action, crate::keymap::Surface::Tree).unwrap();
}

fn plugin_of(sim: &Sim) -> String {
    sim.app
        .tab
        .project_navigation
        .as_ref()
        .map(|n| n.project.plugin.clone())
        .unwrap_or_default()
}

#[test]
fn vcs_old_names_pressed_while_detection_is_pending_run_when_the_project_opens() {
    if isolated("vcs_old_names_pressed_while_detection_is_pending_run_when_the_project_opens") {
        return;
    }
    for (name, screen, flash) in [
        ("git_stage_all", "changes", Some("Staged all (7)")),
        ("git_open_log", "log", None),
        ("git_open_worktrees", "worktrees", None),
    ] {
        let root = repo("git", "pending").unwrap();
        let mut sim = Sim::with_config_sized(&root, cfg_for("git", "tree", name), 120, 30);
        // The worker has answered but the host has not taken the answer: detection is pending.
        sim.app.tab.projects.wait_for_result();
        assert!(sim.app.tab.projects.pending(), "{name}: the seam holds");
        press_undrawn(&mut sim, KEY);
        assert!(!sim.app.project_active(), "{name}: not open yet");
        assert!(sim.app.tab.project_enter_pending, "{name}: queued");
        drain(&mut sim);
        assert!(!sim.app.tab.project_enter_pending);
        assert_eq!(
            effect(&mut sim),
            canvas(screen, flash, false),
            "git `{name}` pressed while detection was pending"
        );
    }
}

#[test]
fn vcs_old_names_pending_name_that_has_no_operation_in_the_detected_backend_is_dropped() {
    if isolated(
        "vcs_old_names_pending_name_that_has_no_operation_in_the_detected_backend_is_dropped",
    ) {
        return;
    }
    let Some(root) = repo("jj", "pending") else {
        eprintln!("SKIPPED: jj is not installed");
        return;
    };
    let mut sim =
        Sim::with_config_sized(&root, cfg_for("jj", "tree", "git_open_worktrees"), 120, 30);
    sim.app.tab.projects.wait_for_result();
    assert!(sim.app.tab.projects.pending());
    press_undrawn(&mut sim, KEY);
    assert!(sim.app.tab.project_enter_pending);
    drain(&mut sim);
    // The project opens like `p` opens it; the operation jj has no row for is dropped.
    assert_eq!(effect(&mut sim), canvas("changes", None, false));
    assert_eq!(plugin_of(&sim), "jj");
}

fn colocated(tag: &str) -> Option<std::path::PathBuf> {
    if !crate::vcs::jj::available() {
        return None;
    }
    let dir = super::vcs_changes::git_repo(&format!("vcs_old_names_colo_{tag}"), true);
    let path = dir.to_path_buf();
    std::mem::forget(dir);
    assert!(crate::test_support::run_jj(
        &path,
        &["git", "init", "--colocate", "."]
    ));
    Some(path)
}

fn chooser_sim(root: &std::path::Path, keys: &[(&str, &str)]) -> Sim {
    let mut sim = Sim::with_config_sized(root, both_enabled(keys), 120, 30);
    sim.app.tab.projects.wait_for_result();
    sim.app.poll_projects();
    sim
}

fn chooser_open(sim: &Sim) -> bool {
    matches!(
        sim.app.project_dialog,
        Some(crate::app::project_screens::ProjectDialog::Projects { .. })
    )
}

#[test]
fn vcs_old_names_chooser_runs_the_operation_of_the_chosen_project() {
    if isolated("vcs_old_names_chooser_runs_the_operation_of_the_chosen_project") {
        return;
    }
    // (name, picks jj (`j` first), plugin, screen)
    for (name, jj, plugin, screen) in [
        ("git_open_log", true, "jj", "log"),
        ("git_open_log", false, "git", "log"),
        ("git_open_worktrees", false, "git", "worktrees"),
        // jj has no worktrees screen: the project opens, the operation is dropped.
        ("git_open_worktrees", true, "jj", "changes"),
    ] {
        let Some(root) = colocated("choose") else {
            eprintln!("SKIPPED: jj is not installed");
            return;
        };
        let mut sim = chooser_sim(&root, &[(&KEY.to_string(), name)]);
        sim.key(KEY);
        assert!(chooser_open(&sim), "{name}: the chooser is up");
        assert!(!sim.app.project_active());
        if jj {
            sim.key('j');
        }
        sim.enter();
        drain(&mut sim);
        assert_eq!(plugin_of(&sim), plugin, "{name} jj={jj}");
        assert_eq!(
            effect(&mut sim),
            canvas(screen, None, false),
            "{name} jj={jj}"
        );
    }
}

#[test]
fn vcs_old_names_cancelled_chooser_drops_the_operation() {
    if isolated("vcs_old_names_cancelled_chooser_drops_the_operation") {
        return;
    }
    let Some(root) = colocated("cancel") else {
        eprintln!("SKIPPED: jj is not installed");
        return;
    };
    let keys = [
        (KEY.to_string(), "git_open_worktrees"),
        ("Y".into(), P_NAME),
    ];
    let keys: Vec<(&str, &str)> = keys.iter().map(|(k, a)| (k.as_str(), *a)).collect();
    let mut sim = chooser_sim(&root, &keys);
    sim.key(KEY);
    assert!(chooser_open(&sim));
    sim.esc();
    drain(&mut sim);
    assert_eq!(
        effect(&mut sim),
        stays("Tree", Some("canceled")).replace("Some(\"canceled\")", "None"),
    );
    // Opening the project afterwards (a menu entry) runs nothing.
    assert!(sim.app.enter_project());
    assert!(chooser_open(&sim));
    sim.enter();
    drain(&mut sim);
    assert_eq!(plugin_of(&sim), "git");
    assert_eq!(effect(&mut sim), canvas("changes", None, false));
}

#[test]
fn vcs_old_names_pending_operation_does_not_leak_to_another_tab() {
    if isolated("vcs_old_names_pending_operation_does_not_leak_to_another_tab") {
        return;
    }
    let root = repo("git", "leak").unwrap();
    let mut cfg = cfg_for("git", "tree", "git_stage_all");
    cfg.keys
        .surfaces
        .get_mut("tree")
        .unwrap()
        .insert("Y".into(), P_NAME.into());
    let mut sim = Sim::with_config_sized(&root, cfg, 120, 30);
    sim.app.tab.projects.wait_for_result();
    assert!(sim.app.tab.projects.pending());
    press_undrawn(&mut sim, KEY);
    assert!(sim.app.tab.project_enter_pending);
    // A new tab is a fresh tab: nothing queued in it.
    sim.app.tab_new().unwrap();
    assert!(!sim.app.tab.project_enter_pending);
    sim.app.tab.projects.wait_for_result();
    sim.app.poll_projects();
    // The entry that is not a key (a menu item) opens the project the same way.
    assert!(sim.app.enter_project());
    drain(&mut sim);
    assert_eq!(effect(&mut sim), canvas("changes", None, false));
    assert_eq!(sim.app.tab.project_enter_op, None);
}

// ---- the file diff surface ----

fn diff_sim(root: &std::path::Path, name: &str) -> Sim {
    let mut sim = Sim::with_config_sized(root, cfg_for("git", "diff", name), 120, 30);
    sim.app.tab.projects.wait_for_result();
    sim.app.poll_projects();
    sim.select("b-unstaged.txt");
    sim.key('d');
    assert_eq!(sim.app.surface(), crate::keymap::Surface::PreviewGitDiff);
    sim
}

#[test]
fn vcs_old_names_on_the_file_diff_open_the_project_over_the_diff() {
    if isolated("vcs_old_names_on_the_file_diff_open_the_project_over_the_diff") {
        return;
    }
    for (name, screen, prompt) in [
        ("git_open_log", "log", false),
        ("git_commit", "changes", true),
    ] {
        let root = repo("git", "diff").unwrap();
        let mut sim = diff_sim(&root, name);
        sim.key(KEY);
        drain(&mut sim);
        assert_eq!(
            effect(&mut sim),
            canvas(screen, None, prompt),
            "`{name}` bound in [keys.diff]"
        );
        // The diff is kept underneath: it is still the tab's preview.
        assert!(
            sim.app.is_git_diff_preview(),
            "`{name}`: the diff is still the preview"
        );
    }
}

// ---- the same names bound on a project canvas run the operation in the open project ----

/// After `p`, press `KEY` bound to `name` in `table`: the effect and how many times the old hub
/// was opened.
fn press_on_canvas(id: &str, table: &str, name: &str) -> Option<(String, usize)> {
    let root = repo(id, "c")?;
    let mut sim = Sim::with_config_sized(&root, cfg_for(id, table, name), 120, 30);
    sim.app.tab.projects.wait_for_result();
    sim.app.poll_projects();
    sim.key('p');
    drain(&mut sim);
    assert_eq!(
        effect(&mut sim),
        canvas("changes", None, false),
        "{id} after p"
    );
    let ((), opens) = App::count_legacy_hub_opens(|| {
        sim.key(KEY);
        drain(&mut sim);
    });
    Some((effect(&mut sim), opens))
}

/// The expected effect of each name on a canvas that was opened by `p` (the changes screen). The
/// operations are the ones the tree cases expect after opening the project; a name with no
/// operation for the backend changes nothing.
fn canvas_cases() -> Vec<(&'static str, String, String)> {
    // Where the tree stays a tree (no operation for the backend, or a name that needs a hub row),
    // the canvas stays as it was, with the flash the write guard gives (jj reads only).
    let on_canvas = |s: String| {
        s.replace("project=false screen=", "project=true screen=changes")
            .replace("surface=Tree", "surface=PreviewCanvas")
    };
    tree_cases()
        .into_iter()
        .map(|(name, git, jj)| {
            let fix = |s: String| {
                if s.contains("surface=Tree") {
                    on_canvas(s)
                } else {
                    s
                }
            };
            (name, fix(git), fix(jj))
        })
        .collect()
}

fn run_canvas(id: &str, table: &str) {
    let mut checked = 0;
    for (name, git, jj) in canvas_cases() {
        // `worktree_close` on a canvas keeps its own behaviour; the tool launch and the filters
        // have their own handlers.
        if matches!(
            name,
            "worktree_close" | "git_launch_tool" | "branch_filter_start" | "worktree_filter_start"
        ) || name == "git_close"
            || name.starts_with("git_copy_")
            || name == "copy_branch_name"
        {
            continue;
        }
        let want = if id == "git" { git } else { jj };
        let Some((got, opens)) = press_on_canvas(id, table, name) else {
            return;
        };
        assert_eq!(
            got, want,
            "{id} `{name}` bound in [keys.{table}] on a canvas"
        );
        assert_eq!(
            opens, 0,
            "{id} `{name}` in [keys.{table}] opened the old hub"
        );
        checked += 1;
    }
    assert_eq!(checked, 38, "the number of names under test");
}

#[test]
fn vcs_old_names_on_a_canvas_bound_in_keys_canvas_git() {
    if isolated("vcs_old_names_on_a_canvas_bound_in_keys_canvas_git") {
        return;
    }
    run_canvas("git", "canvas");
}

#[test]
fn vcs_old_names_on_a_canvas_bound_in_keys_global_git() {
    if isolated("vcs_old_names_on_a_canvas_bound_in_keys_global_git") {
        return;
    }
    run_canvas("git", "global");
}

#[test]
fn vcs_old_names_on_a_canvas_bound_in_keys_canvas_jj() {
    if isolated("vcs_old_names_on_a_canvas_bound_in_keys_canvas_jj") {
        return;
    }
    if repo("jj", "probe").is_none() {
        eprintln!("SKIPPED: jj is not installed");
        return;
    }
    run_canvas("jj", "canvas");
}

#[test]
fn vcs_old_names_on_a_canvas_bound_in_keys_global_jj() {
    if isolated("vcs_old_names_on_a_canvas_bound_in_keys_global_jj") {
        return;
    }
    if repo("jj", "probe").is_none() {
        eprintln!("SKIPPED: jj is not installed");
        return;
    }
    run_canvas("jj", "global");
}

/// The result of a name is the result of the plugin's own key for that operation, in a fresh
/// project opened by `p` (the operations of the changes screen).
#[test]
fn vcs_old_names_on_a_canvas_equal_the_plugins_own_key() {
    if isolated("vcs_old_names_on_a_canvas_equal_the_plugins_own_key") {
        return;
    }
    for (id, name, op) in [
        ("git", "git_stage_all", "stage_all"),
        ("git", "git_unstage_all", "unstage_all"),
        ("git", "git_commit", "commit"),
        ("jj", "jj_sync", "sync"),
    ] {
        let Some(root) = repo(id, "own") else {
            eprintln!("SKIPPED: jj is not installed");
            continue;
        };
        let mut sim = Sim::with_config_sized(&root, cfg_for(id, "canvas", "noop"), 120, 30);
        sim.app.tab.projects.wait_for_result();
        sim.app.poll_projects();
        sim.key('p');
        drain(&mut sim);
        let act = crate::plugins::PluginAct::new(id, op);
        let key = sim
            .app
            .plugin_key_of(crate::keymap::Context::Canvas, act)
            .unwrap_or_else(|| panic!("{id} `{op}` has no key on the canvas"));
        sim.press(key.code, KeyModifiers::NONE);
        drain(&mut sim);
        let own = effect(&mut sim);
        let (got, _) = press_on_canvas(id, "canvas", name).unwrap();
        assert_eq!(got, own, "{id} `{name}` vs the key of `{op}`");
        assert!(
            !sim.app.is_dialog(),
            "{id} `{name}`: the host dialog is open"
        );
    }
}
