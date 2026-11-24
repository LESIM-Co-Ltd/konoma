//! Compare the old hub with the real project host; frame chrome is outside the body.

use super::*;
use crate::plugins::test_plugins;
use konoma_plugin::Plugin;
use std::sync::Arc;

#[path = "vcs_writes.rs"]
mod writes;

#[path = "vcs_branches.rs"]
mod branches;

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

pub(super) fn plugin_config(id: &str, name: &str, lang: &str) -> Config {
    let plugin: Arc<dyn Plugin> = match id {
        "git" => Arc::new(konoma_vcs::Git),
        "jj" => Arc::new(konoma_vcs::Jj),
        _ => panic!("unknown VCS"),
    };
    let token = test_plugins::register(name, plugin);
    let mut cfg = test_plugins::marker_config(&token);
    cfg.ui.lang = lang.into();
    cfg.ui.statusbar = "split".into();
    cfg.ui.tabbar = "hidden".into();
    cfg.ui.diff_view = "source".into();
    cfg
}

fn pair(
    root: &std::path::Path,
    id: &str,
    name: &str,
    lang: &str,
    width: u16,
    height: u16,
) -> (Sim, Sim) {
    let cfg = plugin_config(id, name, lang);
    pair_config(root, id, cfg, width, height)
}

pub(super) fn pair_config(
    root: &std::path::Path,
    id: &str,
    cfg: Config,
    width: u16,
    height: u16,
) -> (Sim, Sim) {
    let mut old_cfg = cfg.clone();
    // The legacy key path stays the user entry until PR5; open it explicitly in this comparison.
    old_cfg.plugins.clear();
    let mut old = Sim::with_config_sized(root, old_cfg, width, height);
    old.app.open_git_view();
    assert!(old.app.is_git_view());
    old.draw();
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
    (old, new)
}

#[test]
fn e2e_vcs_changes_nested_tree_and_launch_directory_path_styles_match_the_hub() {
    let dir = git_repo("vcs_paths_nested", true);
    let nested = dir.join("sub/deep");
    std::fs::create_dir_all(&nested).unwrap();
    for root in [dir.as_path(), nested.as_path()] {
        for style in ["relative", "home", "full"] {
            let mut cfg = plugin_config("git", "vcs_paths_nested", "en");
            cfg.ui.path_style = style.into();
            cfg.plugins
                .entry("git".into())
                .or_default()
                .insert("path_style".into(), toml::Value::String(style.into()));
            let (mut old, mut new) = pair_config(root, "git", cfg, 200, 20);
            assert_eq!(new.app.tab.projects.projects[0].root, dir.as_path());
            assert_eq!(old.app.git_view_entries().len(), 7);
            assert!(
                compare_body(&mut old, &mut new, 200, 20).is_empty(),
                "tree={} style={style}",
                if root == dir.as_path() {
                    "repo"
                } else {
                    "nested"
                }
            );
        }
    }
}

#[test]
fn e2e_vcs_changes_linked_worktree_external_index_event_refreshes_the_session() {
    use crate::test_support::run_git;
    let main = git_repo("vcs_injected_main", false);
    let linked = crate::test_support::unique_tmp("vcs_injected_linked");
    run_git(
        &main,
        &[
            "worktree",
            "add",
            "-q",
            "--detach",
            linked.to_str().unwrap(),
        ],
    );
    let root = linked.canonicalize().unwrap();
    let (_, mut new) = pair(&root, "git", "vcs_injected_linked", "en", 180, 20);
    assert!(new.app.tab.projects.uses_injected_events());
    std::fs::write(root.join("a-staged.txt"), "changed\n").unwrap();
    run_git(&root, &["add", "a-staged.txt"]);
    let gitdir = crate::git::git_dir(&root).unwrap();
    new.app
        .tab
        .projects
        .inject_event(Ok(notify::Event::new(notify::EventKind::Modify(
            notify::event::ModifyKind::Any,
        ))
        .add_path(gitdir.join("index"))));
    drain(&mut new);
    new.see("● M");
    new.enter();
    drain(&mut new);
    assert_eq!(new.app.tab.preview_path, Some(root.join("a-staged.txt")));
    assert!(new.app.is_git_diff_preview());
}

fn git_repo(name: &str, changed: bool) -> crate::test_support::TmpDir {
    use crate::test_support::{init_git_repo, run_git};
    let dir = sandbox(name);
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
    dir.into_canonical().unwrap()
}

pub(super) fn compare_body(old: &mut Sim, new: &mut Sim, width: u16, height: u16) -> Vec<String> {
    // Compare logical frames. TestBackend retains old symbols in cells hidden under a wide
    // glyph; those terminal continuation cells are not the frame the renderer produced.
    let old = old
        .term
        .draw(|frame| ui::render(frame, &mut old.app))
        .unwrap()
        .buffer
        .clone();
    let new = new
        .term
        .draw(|frame| ui::render(frame, &mut new.app))
        .unwrap()
        .buffer
        .clone();
    // Split header: y=0. Hub title/project tabs: y=1. Both bodies start at y=2.
    // The old hub's two border columns and last border row belong to its chrome.
    let mut differences = Vec::new();
    for y in 2..height - 2 {
        for x in 0..width - 2 {
            let before = &old[(x + 1, y)];
            let after = &new[(x, y)];
            let before = (before.symbol(), before.fg, before.bg, before.modifier);
            let after = (after.symbol(), after.fg, after.bg, after.modifier);
            if before != after {
                differences.push(format!("({x}, {y}): {before:?} != {after:?}"));
            }
        }
    }
    differences
}

#[test]
fn e2e_vcs_changes_git_body_cell_parity_with_staged_unstaged_rename_delete_cjk_and_empty() {
    let mut failures = Vec::new();
    for changed in [true, false] {
        let dir = git_repo("vcs_changes_git_parity", changed);
        for lang in ["en", "ja"] {
            for (width, height) in [(140, 20), (34, 8)] {
                let (mut old, mut new) =
                    pair(&dir, "git", "vcs_changes_git_parity", lang, width, height);
                let count = old.app.git_view_entries().len().max(1);
                for selected in 0..count {
                    if selected > 0 {
                        old.app.git_view_move(1);
                        old.draw();
                        new.key('j');
                        drain(&mut new);
                    }
                    let differences = compare_body(&mut old, &mut new, width, height);
                    if !differences.is_empty() {
                        failures.push(format!("changed={changed} lang={lang} size={width}x{height} selected={selected}: {} mismatched cells; {}; old rows={:?}; new active={} pending={}", differences.len(), differences.iter().take(3).cloned().collect::<Vec<_>>().join(", "), old.app.git_view_entries(), new.app.project_active(), new.app.project_jobs_pending()));
                    }
                }
                for selected in (0..count.saturating_sub(1)).rev() {
                    old.app.git_view_move(-1);
                    new.key('k');
                    drain(&mut new);
                    let differences = compare_body(&mut old, &mut new, width, height);
                    if !differences.is_empty() {
                        failures.push(format!(
                            "moving up: size={width}x{height} selected={selected}: {}",
                            differences[0]
                        ));
                    }
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

fn diff_lines(sim: &mut Sim) -> Vec<(crate::git::DiffLineKind, String)> {
    sim.app
        .git_diff_lines()
        .into_iter()
        .map(|line| (line.kind, line.text))
        .collect()
}

#[test]
fn e2e_vcs_changes_git_enter_opens_same_file_and_head_baseline_even_when_staged() {
    let dir = git_repo("vcs_changes_git_enter", true);
    let (mut old, mut new) = pair(&dir, "git", "vcs_changes_git_enter", "en", 140, 20);
    assert!(old.app.git_view_entries()[0].staged);
    old.enter();
    new.enter();
    drain(&mut new);
    assert!(old.app.is_git_diff_preview());
    assert!(new.app.is_git_diff_preview());
    assert_eq!(new.app.tab.preview_path, Some(dir.join("a-staged.txt")));
    let expected = diff_lines(&mut old);
    assert!(
        expected.iter().any(|(_, text)| text.contains("base")),
        "must compare against HEAD, not the index"
    );
    assert!(expected.iter().any(|(_, text)| text.contains("worktree")));
    assert!(!expected.iter().any(|(_, text)| text.contains("index")));
    assert_eq!(diff_lines(&mut new), expected);
}

#[test]
fn e2e_vcs_changes_jj_and_colocated_body_cell_parity_and_enter_parent_baseline() {
    use crate::test_support::{jj_scratch_bare, run_jj};
    if !crate::vcs::jj::available() {
        return;
    }
    let mut failures = Vec::new();
    for colocated in [false, true] {
        for changed in [false, true] {
            let dir = if colocated {
                let dir = git_repo("vcs_changes_colocated", false);
                assert!(run_jj(&dir, &["git", "init", "--colocate", "."]));
                dir
            } else {
                let dir = jj_scratch_bare("vcs_changes_jj")
                    .expect("jj is available: fixture must initialize");
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
                dir.into_canonical().unwrap()
            };
            if changed {
                std::fs::write(dir.join("a-staged.txt"), "worktree\n").unwrap();
                std::fs::remove_file(dir.join("c-deleted.txt")).unwrap();
                std::fs::rename(dir.join("d-old.txt"), dir.join("d-renamed.txt")).unwrap();
                std::fs::write(dir.join("e-untracked.txt"), "new\n").unwrap();
                std::fs::write(dir.join("日本語.txt"), "変更\n").unwrap();
            }
            crate::vcs::set_preference_for_test(Some(crate::vcs::Preference::Jj));
            let (mut old, mut new) = pair(&dir, "jj", "vcs_changes_jj_parity", "en", 140, 20);
            // Reset before assertions so even a failing cell comparison cannot leak the pin.
            crate::vcs::set_preference_for_test(None);
            let differences = compare_body(&mut old, &mut new, 140, 20);
            if !differences.is_empty() {
                failures.push(format!(
                    "colocated={colocated} changed={changed}: {} mismatched cells; {}",
                    differences.len(),
                    differences
                        .iter()
                        .take(3)
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
            if changed {
                crate::vcs::set_preference_for_test(Some(crate::vcs::Preference::Jj));
                old.enter();
                new.enter();
                drain(&mut new);
                let expected = diff_lines(&mut old);
                let actual = diff_lines(&mut new);
                crate::vcs::set_preference_for_test(None);
                assert_eq!(new.app.tab.preview_path, Some(dir.join("a-staged.txt")));
                assert!(new.app.is_git_diff_preview());
                assert!(expected.iter().any(|(_, text)| text.contains("base")));
                assert_eq!(actual, expected);
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
