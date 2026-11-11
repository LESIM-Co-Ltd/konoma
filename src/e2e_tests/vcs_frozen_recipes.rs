//! One recipe per old test. A recipe is the shared sequence of the old test's `compare_body` / `same`
//! sites: the generator and the staleness check run it on the old hub, the replay runs it through `p`.
//! Steps that differ between the two (hub API versus keys) are written with `on_old` / `on_new`.

use super::super::vcs_changes::{branches_fixture, git_repo_at, plugin_config};
use super::super::*;
use super::{stable_root, Ctx, Recipe};

#[path = "vcs_frozen_recipes_log.rs"]
mod log;

pub(super) const RECIPES: &[Recipe] = &[
    Recipe {
        name: "e2e_vcs_detail_cells_motions_layout_empty_and_less_match_legacy",
        jj: false,
        // widths x layouts x empty x key schemes x (open + 19 keys)
        min_cases: 2 * 2 * 2 * 2 * (1 + 19),
        run: log::detail_cells,
    },
    Recipe {
        name: "e2e_vcs_detail_missing_commit_empty_without_header_matches_legacy_and_r_reloads",
        jj: false,
        // widths x layouts x (reload + 10 keys)
        min_cases: 2 * 2 * (1 + 10),
        run: log::detail_missing,
    },
    Recipe {
        name: "e2e_vcs_log_git_cells_alignments_boundaries_cjk_ellipsis_and_scrolling",
        jj: false,
        // languages x alignments x widths x (open + 10 keys)
        min_cases: 2 * 3 * 5 * (1 + 10),
        run: log::git_cells,
    },
    Recipe {
        name: "e2e_vcs_log_filter_uses_legacy_text_and_cursor_for_every_query",
        jj: false,
        // alignments x queries x (filtered + moved)
        min_cases: 2 * 9 * 2,
        run: log::filter,
    },
    Recipe {
        name: "e2e_vcs_log_unborn_empty_cells_and_fixed_tab_order",
        jj: false,
        min_cases: 2,
        run: log::unborn,
    },
    Recipe {
        name: "e2e_vcs_log_limit_matches_hub_at_200_and_omits_201st",
        jj: false,
        min_cases: 1,
        run: log::limit,
    },
    Recipe {
        name: "e2e_vcs_log_jj_and_both_colocated_backends_match_the_selected_legacy_backend",
        jj: true,
        // plain jj: detail + 3 alignments x 2 languages x entries; colocated: git and jj rows, plus the jj detail
        min_cases: (1 + 3 * 2 * log::jj_entries(false, false))
            + (3 * 2 * log::jj_entries(true, true))
            + (1 + 3 * 2 * log::jj_entries(true, false)),
        run: log::jj_log,
    },
    Recipe {
        name: "e2e_vcs_changes_nested_tree_and_launch_directory_path_styles_match_the_hub",
        jj: false,
        // trees x path styles (only `relative`; `home` / `full` embed the machine's absolute path)
        min_cases: 2,
        run: changes_paths,
    },
    Recipe {
        name:
            "e2e_vcs_changes_git_body_cell_parity_with_staged_unstaged_rename_delete_cjk_and_empty",
        jj: false,
        // changed: 7 entries -> 7 down frames + 6 up frames; unchanged: 1 frame; x 2 languages x 2 sizes.
        min_cases: (7 + 6 + 1) * 2 * 2,
        run: changes_git,
    },
    Recipe {
        name: "e2e_vcs_changes_jj_and_colocated_body_cell_parity_and_enter_parent_baseline",
        jj: true,
        min_cases: 2 * 2,
        run: changes_jj,
    },
    Recipe {
        name: "e2e_vcs_branches_cells_current_selection_move_and_filter_at_two_widths",
        jj: false,
        // widths x languages x (open + move + 5 filter queries)
        min_cases: 2 * 2 * (1 + 1 + 5),
        run: branches_cells,
    },
    Recipe {
        name: "e2e_vcs_branches_jj_bookmark_cells_copy_and_no_write_hints",
        jj: true,
        min_cases: 2,
        run: branches_jj,
    },
];

fn changes_paths(c: &mut Ctx) {
    let (_guard, dir) = stable_root("vcs_paths_nested");
    git_repo_at(&dir, true);
    let nested = dir.join("sub/deep");
    std::fs::create_dir_all(&nested).unwrap();
    for (tree, root) in [("repo", dir.as_path()), ("nested", nested.as_path())] {
        // `home` and `full` draw the absolute scratch path, which differs per machine and user, so
        // they cannot be frozen cell for cell; only the old test still compares them.
        for style in ["relative"] {
            let mut cfg = plugin_config("git", "vcs_paths_nested", "en");
            cfg.ui.path_style = style.into();
            cfg.plugins
                .entry("git".into())
                .or_default()
                .insert("path_style".into(), toml::Value::String(style.into()));
            c.open(root, "git", cfg, 200, 20);
            c.on_old(|old| assert_eq!(old.app.git_view_entries().len(), 7));
            c.body(&format!("tree={tree} style={style}"), 200, 20);
        }
    }
}

fn changes_git(c: &mut Ctx) {
    for changed in [true, false] {
        let (_guard, dir) = stable_root("vcs_changes_git_parity");
        git_repo_at(&dir, changed);
        for lang in ["en", "ja"] {
            for (width, height) in [(140, 20), (34, 8)] {
                let cfg = plugin_config("git", "vcs_changes_git_parity", lang);
                c.open(&dir, "git", cfg, width, height);
                // The fixture has 7 changed entries; an unchanged repository still shows one row.
                let count = if changed { 7 } else { 1 };
                c.on_old(|old| assert_eq!(old.app.git_view_entries().len().max(1), count));
                for selected in 0..count {
                    if selected > 0 {
                        c.on_old(|old| {
                            old.app.git_view_move(1);
                            old.draw();
                        });
                        c.on_new(|new| new.key('j'));
                    }
                    c.body(
                        &format!(
                            "changed={changed} lang={lang} size={width}x{height} down-{selected}"
                        ),
                        width,
                        height,
                    );
                }
                for selected in (0..count - 1).rev() {
                    c.on_old(|old| old.app.git_view_move(-1));
                    c.on_new(|new| new.key('k'));
                    c.body(
                        &format!(
                            "changed={changed} lang={lang} size={width}x{height} up-{selected}"
                        ),
                        width,
                        height,
                    );
                }
            }
        }
    }
}

fn changes_jj(c: &mut Ctx) {
    use crate::test_support::run_jj;
    for colocated in [false, true] {
        for changed in [false, true] {
            let (_guard, dir) = stable_root("vcs_changes_jj");
            if colocated {
                git_repo_at(&dir, false);
                assert!(run_jj(&dir, &["git", "init", "--colocate", "."]));
            } else {
                assert!(run_jj(&dir, &["git", "init", "--no-colocate", "."]));
                for file in [
                    "a-staged.txt",
                    "b-unstaged.txt",
                    "c-deleted.txt",
                    "d-old.txt",
                    "日本語.txt",
                ] {
                    std::fs::write(dir.join(file), "base\n").unwrap();
                }
                assert!(run_jj(&dir, &["commit", "-m", "seed"]));
            }
            if changed {
                std::fs::write(dir.join("a-staged.txt"), "worktree\n").unwrap();
                std::fs::remove_file(dir.join("c-deleted.txt")).unwrap();
                std::fs::rename(dir.join("d-old.txt"), dir.join("d-renamed.txt")).unwrap();
                std::fs::write(dir.join("e-untracked.txt"), "new\n").unwrap();
                std::fs::write(dir.join("日本語.txt"), "変更\n").unwrap();
            }
            crate::vcs::set_preference_for_test(Some(crate::vcs::Preference::Jj));
            let cfg = plugin_config("jj", "vcs_changes_jj_parity", "en");
            c.open(&dir, "jj", cfg, 140, 20);
            // Reset before capturing so a failing frame cannot leak the pin.
            crate::vcs::set_preference_for_test(None);
            c.body(&format!("colocated={colocated} changed={changed}"), 140, 20);
        }
    }
}

fn branches_cells(c: &mut Ctx) {
    let dir = branches_fixture("vcs_branch_cells");
    for width in [80, 160] {
        for lang in ["en", "ja"] {
            let cfg = plugin_config("git", "vcs_branch_parity", lang);
            c.open(&dir, "git", cfg, width, 20);
            c.key('b');
            c.body(&format!("width={width} lang={lang} open"), width, 20);
            c.key('j');
            c.body(&format!("width={width} lang={lang} move"), width, 20);
            for query in ["ALP", "日本", "missing", "*", "  "] {
                c.key('/');
                for key in query.chars() {
                    c.raw_key(key);
                }
                c.settle();
                c.body(
                    &format!("width={width} lang={lang} query={query:?}"),
                    width,
                    20,
                );
                c.esc();
            }
        }
    }
}

fn branches_jj(c: &mut Ctx) {
    let dir = sandbox("vcs_bookmarks").into_canonical().unwrap();
    assert!(crate::test_support::run_jj(
        &dir,
        &["git", "init", "--no-colocate", "."]
    ));
    assert!(crate::test_support::run_jj(
        &dir,
        &["describe", "-m", "seed"]
    ));
    super::super::vcs_changes::add_jj_bookmarks(&dir);
    for lang in ["en", "ja"] {
        let cfg = plugin_config("jj", "vcs_bookmark_parity", lang);
        c.open(&dir, "jj", cfg, 160, 20);
        c.key('b');
        c.body(&format!("lang={lang}"), 160, 20);
    }
}
