//! Branch cells and real writes through the project host.

use super::*;
use crate::test_support::{run_git, TmpDir};

pub(super) fn fixture(name: &str) -> TmpDir {
    let dir = git_repo(name, false);
    run_git(&dir, &["branch", "-m", "main"]);
    run_git(&dir, &["branch", "alpha"]);
    run_git(&dir, &["branch", "日本語"]);
    dir
}

/// The branches screen's top edge, written out: the title with the branch count, the filler, and
/// the project tabs in the language of the session, at both widths. The body cells are held by
/// the frozen replay of the cells test above.
#[test]
fn e2e_vcs_branches_top_edge_literal_title_count_and_tabs_at_two_widths() {
    let dir = fixture("vcs_branch_edge");
    for width in [80u16, 160] {
        for lang in ["en", "ja"] {
            let cfg = plugin_config("git", "vcs_branch_edge", lang);
            let mut sim = Sim::with_config_sized(&dir, cfg, width, 20);
            sim.app.tab.projects.wait_for_result();
            sim.app.poll_projects();
            sim.key('p');
            drain(&mut sim);
            sim.key('b');
            drain(&mut sim);
            assert_eq!(sim.app.project_frame().unwrap().key.screen, "branches");
            let edge = super::drawn_top_edge(&mut sim, width);
            // main, alpha and the Japanese-named branch.
            assert!(edge.starts_with("┌ Git branches  (3) ─"), "{edge:?}");
            let tabs = if lang == "en" {
                "─ p:Changes  Log  Graph  b:Branches  w:Worktrees ┐"
            } else {
                "─ p:変更  ログ  グラフ  b:ブランチ  w:ワークツリー ┐"
            };
            assert!(edge.ends_with(tabs), "{edge:?}");
        }
    }
}

pub(super) fn add_jj_bookmarks(dir: &std::path::Path) {
    for name in ["alpha", "日本語"] {
        assert!(crate::test_support::run_jj(
            dir,
            &["bookmark", "create", name]
        ));
    }
}
