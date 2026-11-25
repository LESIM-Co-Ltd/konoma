//! Compare the old hub with the real project host; frame chrome is outside the body.

use super::*;
use crate::plugins::test_plugins;
use konoma_plugin::Plugin;
use std::sync::Arc;

#[path = "vcs_writes.rs"]
mod writes;

#[path = "vcs_branches.rs"]
mod branches;

pub(super) fn seed_write_repository(root: &std::path::Path, changed: bool) {
    writes::seed(root, changed);
}

pub(super) fn branch_write_fixture() -> crate::test_support::TmpDir {
    branches::fixture("vcs_app_branch_writes")
}

pub(super) fn branches_fixture(name: &str) -> crate::test_support::TmpDir {
    branches::fixture(name)
}

pub(super) fn write_git_output(root: &std::path::Path, args: &[&str]) -> String {
    writes::git_output(root, args)
}

pub(super) fn write_jj_state(root: &std::path::Path) -> Vec<String> {
    writes::jj_state(root)
}

pub(super) fn seed_jj_write_repository(root: &std::path::Path, colocated: bool) {
    writes::seed_jj(root, colocated);
}

pub(super) fn add_jj_bookmarks(dir: &std::path::Path) {
    branches::add_jj_bookmarks(dir);
}

pub(super) fn drain(sim: &mut Sim) {
    for _ in 0..32 {
        sim.app.poll_projects();
        sim.draw();
        if !sim.app.project_jobs_pending() {
            return;
        }
        sim.app.wait_project_job();
    }
    panic!("VCS project did not become idle");
}

/// Like `drain`, but every worker result that is already on its way lands BEFORE the first frame is
/// drawn: the stale screen is never drawn while a job is pending. A plain `drain` draws or skips
/// that stale frame depending on whether the worker beat the first `poll_projects` (both orders
/// are legitimate), which makes an allocation count differ by one whole frame between runs. Use
/// this wherever a byte count is asserted. The wait is on the job's reply channel, never a sleep.
pub(super) fn drain_settled(sim: &mut Sim) {
    for _ in 0..32 {
        sim.app.poll_projects();
        if !sim.app.project_jobs_pending() {
            break;
        }
        sim.app.wait_project_job();
    }
    drain(sim);
}

pub(super) fn plugin_config(id: &str, name: &str, lang: &str) -> Config {
    let plugin: Arc<dyn Plugin> = match id {
        "git" => Arc::new(konoma_vcs::Git),
        "jj" => Arc::new(konoma_vcs::Jj),
        _ => panic!("unknown VCS"),
    };
    let token = test_plugins::register_replacement(name, plugin);
    let mut cfg = test_plugins::marker_config(&token);
    let other = if id == "git" { "jj" } else { "git" };
    cfg.plugins
        .entry(other.into())
        .or_default()
        .insert("enabled".into(), toml::Value::Boolean(false));
    cfg.ui.lang = lang.into();
    cfg.ui.statusbar = "split".into();
    cfg.ui.tabbar = "hidden".into();
    cfg.ui.diff_view = "source".into();
    cfg
}

/// The project (`p`) side alone: a sim with `cfg`, the VCS plugin registered, and the Changes
/// screen of the project open. No old hub is opened.
pub(super) fn project_sim(
    root: &std::path::Path,
    id: &str,
    cfg: Config,
    width: u16,
    height: u16,
) -> Sim {
    let mut new = Sim::with_config_sized(root, cfg, width, height);
    assert!(
        crate::plugins::build_registry(&new.app.cfg.plugin_disabled())
            .manifest(id)
            .is_some(),
        "VCS plugin refused by registry"
    );
    new.app.tab.projects.wait_for_result();
    new.app.poll_projects();
    new.key('p');
    drain(&mut new);
    assert!(new.app.project_active(), "{:?}", new.app.flash);
    new
}

pub(super) fn git_repo(name: &str, changed: bool) -> crate::test_support::TmpDir {
    let dir = sandbox(name);
    git_repo_at(&dir, changed);
    dir.into_canonical().unwrap()
}

/// The fixture of [`git_repo`] built in an existing empty directory (the frozen frames need a
/// repository whose directory name does not carry the pid).
pub(super) fn git_repo_at(dir: &std::path::Path, changed: bool) {
    use crate::test_support::{init_git_repo, run_git};
    let dir = dir.to_path_buf();
    init_git_repo(&dir);
    for file in [
        "a-staged.txt",
        "b-unstaged.txt",
        "c-deleted.txt",
        "d-old.txt",
        "日本語.txt",
    ] {
        std::fs::write(dir.join(file), "base\n").unwrap();
    }
    run_git(&dir, &["add", "."]);
    run_git(&dir, &["commit", "-qm", "seed"]);
    if changed {
        std::fs::write(dir.join("a-staged.txt"), "index\n").unwrap();
        run_git(&dir, &["add", "a-staged.txt"]);
        std::fs::write(dir.join("a-staged.txt"), "worktree\n").unwrap();
        std::fs::write(dir.join("b-unstaged.txt"), "changed\n").unwrap();
        std::fs::remove_file(dir.join("c-deleted.txt")).unwrap();
        run_git(&dir, &["mv", "d-old.txt", "d-renamed.txt"]);
        std::fs::write(dir.join("e-untracked.txt"), "new\n").unwrap();
        std::fs::write(dir.join("f-added.txt"), "new\n").unwrap();
        run_git(&dir, &["add", "f-added.txt"]);
        std::fs::write(dir.join("日本語.txt"), "変更\n").unwrap();
    }
}

/// The list's title as text: the heading's parts around the count (no query, no count), or the
/// plain title when the list has one. The drawn top edge is compared separately, cell for cell.
pub(super) fn heading_text(list: &konoma_plugin::view::ListView) -> String {
    match (&list.heading, &list.title) {
        (Some(heading), _) => format!("{}{}", heading.before.plain(), heading.after.plain()),
        (None, Some(title)) => title.plain(),
        (None, None) => String::new(),
    }
}

/// The first screen's top-edge text of a sim, drawn as the real frame.
pub(super) fn drawn_top_edge(sim: &mut Sim, width: u16) -> String {
    let buf = sim
        .term
        .draw(|frame| ui::render(frame, &mut sim.app))
        .unwrap()
        .buffer
        .clone();
    crate::test_support::top_edge::row_text(&buf, 1, 0..width)
}

/// A sim on the Changes screen of the project, optionally with a filter applied.
fn filtered_new(
    dir: &std::path::Path,
    id: &str,
    lang: &str,
    icons: bool,
    query: &str,
    width: u16,
) -> Sim {
    let mut cfg = plugin_config(id, "vcs_changes_titles", lang);
    cfg.ui.icons = icons;
    let mut new = project_sim(dir, id, cfg, width, 12);
    if !query.is_empty() {
        new.key('/');
        for c in query.chars() {
            new.key(c);
        }
        new.enter();
        drain(&mut new);
    }
    new
}

/// Each matrix cell: the title words, the marker and the filter are drawn on the top edge. The
/// edge and the body of every cell, cell for cell, are the frozen frames of the recipe
/// `e2e_vcs_changes_title_matches_hub_for_icons_languages_filters_and_branch_states`.
#[test]
fn e2e_vcs_changes_title_matches_hub_for_icons_languages_filters_and_branch_states() {
    use crate::test_support::run_git;
    let mut checked = 0;
    for state in ["branch", "detached"] {
        let dir = git_repo("vcs_changes_titles", true);
        if state == "detached" {
            run_git(&dir, &["checkout", "-q", "--detach"]);
        }
        for icons in [false, true] {
            for lang in ["en", "ja"] {
                for query in ["", "txt", "a-", "zzz-no-hit"] {
                    let mut new = filtered_new(&dir, "git", lang, icons, query, 90);
                    let got = drawn_top_edge(&mut new, 90);
                    let marker = if icons { "\u{2387}" } else { "br:" };
                    assert!(
                        got.starts_with("┌ Git "),
                        "state={state} icons={icons} lang={lang} q={query}: {got:?}"
                    );
                    if state == "branch" {
                        assert!(got.starts_with(&format!("┌ Git {marker} ")), "{got:?}");
                    } else {
                        assert!(got.starts_with(&format!("┌ Git {marker} HEAD")), "{got:?}");
                    }
                    if !query.is_empty() {
                        assert!(got.contains(query), "new edge shows the filter: {got:?}");
                    }
                    checked += 1;
                }
            }
        }
    }
    assert_eq!(checked, 32, "every matrix cell ran");
}

/// The shown count is drawn as `  (n)` right after the filter, and `n` is the number of rows the
/// filter leaves: the fixture has seven changed files (a-staged, b-unstaged, c-deleted,
/// d-renamed, e-untracked, f-added, 日本語), so no filter draws `(7)`, `e-un` matches exactly
/// e-untracked and draws `(1)`, and a miss draws `(0)`.
#[test]
fn e2e_vcs_changes_title_filtered_count_is_exactly_the_matching_rows() {
    let dir = git_repo("vcs_changes_title_count", true);
    for lang in ["en", "ja"] {
        for icons in [false, true] {
            for (query, rows) in [("", 7usize), ("e-un", 1), ("zzz-no-hit", 0)] {
                let mut new = filtered_new(&dir, "git", lang, icons, query, 90);
                let want = format!("{}  ({rows})", if query.is_empty() { "" } else { query });
                let edge = drawn_top_edge(&mut new, 90);
                let shown = if query.is_empty() {
                    want.clone()
                } else {
                    format!("/{want}")
                };
                assert!(
                    edge.contains(&shown),
                    "lang={lang} icons={icons} q={query:?}: want {shown:?} in {edge:?}"
                );
                // Exactly one count is drawn, and it is not a longer number that merely
                // starts with the expected digits.
                assert_eq!(edge.matches("  (").count(), 1, "{edge:?}");
                assert!(
                    edge.contains(&format!("({rows}) ")) || edge.contains(&format!("({rows})─")),
                    "{edge:?}"
                );
            }
        }
    }
}

/// A repository with no commit yet names its unborn branch; the title edge and the body are the
/// frozen frames of the recipe `e2e_vcs_changes_title_without_a_branch_matches_hub`.
#[test]
fn e2e_vcs_changes_title_without_a_branch_matches_hub() {
    let dir = sandbox("vcs_changes_title_unborn");
    crate::test_support::init_git_repo(&dir);
    std::fs::write(dir.join("new.txt"), "x\n").unwrap();
    let dir = dir.into_canonical().unwrap();
    for (icons, marker) in [(false, "br:"), (true, "\u{2387}")] {
        let mut new = filtered_new(&dir, "git", "en", icons, "", 60);
        let edge = drawn_top_edge(&mut new, 60);
        assert!(edge.starts_with(&format!("┌ Git {marker} ")), "{edge:?}");
        assert!(edge.contains("  (1) "), "{edge:?}");
    }
}

/// The Changes and Log titles carry the marker; their cells are the frozen frames of the recipe
/// `e2e_git_titles_changes_log_branches_and_jj_labels_come_from_the_plugin`.
#[test]
fn e2e_git_titles_changes_log_branches_and_jj_labels_come_from_the_plugin() {
    let dir = git_repo("vcs_changes_title_words", true);
    for (icons, marker) in [(false, "br:"), (true, "\u{2387}")] {
        for lang in ["en", "ja"] {
            let mut new = filtered_new(&dir, "git", lang, icons, "", 80);
            let edge = drawn_top_edge(&mut new, 80);
            assert!(edge.starts_with(&format!("┌ Git {marker} ")), "{edge:?}");
            new.press(KeyCode::Tab, KeyModifiers::NONE);
            drain(&mut new);
            let edge = drawn_top_edge(&mut new, 80);
            assert!(
                edge.starts_with(&format!("┌ Git log {marker} ")),
                "{edge:?}"
            );
        }
    }
}
