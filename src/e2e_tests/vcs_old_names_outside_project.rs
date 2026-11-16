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

/// Names that are not in the table above on purpose (see the report): `worktree_close` opens the
/// old hub's changes screen today and has no counterpart row that says what to do instead.
const EXCLUDED: &[&str] = &["worktree_close"];

#[test]
fn vcs_old_names_table_covers_every_row_of_the_old_screens_table() {
    let cases = tree_cases();
    let mut seen = 0;
    for row in ROWS {
        if EXCLUDED.contains(&row.old) {
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
