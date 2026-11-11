//! Recipes of the old worktree tests in `vcs_worktrees.rs` (`same` and `same_diff` sites).

use super::super::super::vcs_changes::{drain, plugin_config};
use super::super::super::vcs_log::{commit_worktree, seed_worktrees};
use super::super::super::*;
use super::super::Ctx;
use super::stable_root;
use std::path::Path;

// The worktree rows print each path with the home prefix shortened to `~`, so these recipes point
// `HOME` at the fixture (`set_home`, which needs an isolated child process) to keep the frames
// free of the machine's temp path and its length. Both sides read `HOME` when they draw.

/// `pair_settings` of `vcs_worktrees.rs` with its defaults: the worktrees screen of the legacy hub
/// (`w`) or of the project host (`w` after `p`).
fn open_worktrees(c: &mut Ctx, root: &Path, lang: &str, width: u16) {
    let mut cfg = plugin_config("git", "vcs_worktrees", lang);
    cfg.git.worktree_dir = "../".into();
    cfg.ui.graph_base_branches = Vec::new();
    let settings = cfg.plugins.entry("git".into()).or_default();
    settings.insert("worktree_dir".into(), toml::Value::String("../".into()));
    settings.insert("read_only".into(), toml::Value::Boolean(false));
    settings.insert("graph_base_branches".into(), toml::Value::Array(Vec::new()));
    c.open(root, "git", cfg, width, 20);
    c.key('w');
    c.on_old(|old| assert!(old.app.is_git_worktrees()));
    c.on_new(|new| {
        assert_eq!(new.app.project_frame().unwrap().key.screen, "worktrees");
    });
}

pub(super) fn cells(c: &mut Ctx) {
    let (_guard, parent) = stable_root("vcs_worktrees");
    let dir = parent.parent().unwrap().to_path_buf();
    let (main, linked, _detached) = seed_worktrees(&dir);
    super::super::set_home(&dir);
    for (name, root) in [("main", &main), ("linked", &linked)] {
        for lang in ["en", "ja"] {
            for width in [80, 160] {
                open_worktrees(c, root, lang, width);
                let label = format!("root={name} lang={lang} w={width}");
                c.body(&format!("{label} open"), width, 20);
                c.key('j');
                c.body(&format!("{label} move"), width, 20);
                for query in ["日本語", "LINKED", "no-match"] {
                    c.press(KeyCode::Esc, KeyModifiers::NONE);
                    // Reopen after Esc closed an unfiltered list.
                    c.on_old(|old| {
                        if !old.app.is_git_worktrees() {
                            old.key('w');
                        }
                    });
                    c.on_new(|new| {
                        if !new.app.project_active() {
                            new.key('p');
                            drain(new);
                            new.key('w');
                        }
                    });
                    c.raw_key('/');
                    for key in query.chars() {
                        c.raw_key(key);
                    }
                    c.settle();
                    c.body(&format!("{label} query={query:?}"), width, 20);
                }
            }
        }
    }
}

/// Move the cursor to the worktree at `path` (the old test moves both sides in lockstep).
fn select(c: &mut Ctx, path: &Path) {
    let old = c.is_old();
    for _ in 0..50 {
        let (index, current) = {
            let sim = c.sim();
            if old {
                (
                    sim.app
                        .git_worktree_view()
                        .iter()
                        .position(|w| w.path == path)
                        .unwrap(),
                    sim.app.git_worktree_sel(),
                )
            } else {
                // A project row carries the worktree path as its value.
                let content = sim.app.project_content().unwrap();
                let crate::app::project_screens::ProjectContent::List(list) = content.as_ref()
                else {
                    panic!("list required")
                };
                let index = list
                    .rows
                    .iter()
                    .position(|row| row.value == path.to_string_lossy())
                    .unwrap();
                (index, sim.app.project_frame().unwrap().cursor)
            }
        };
        if current == index {
            return;
        }
        c.key(if current < index { 'j' } else { 'k' });
    }
    panic!("worktree {} not reached", path.display());
}

/// Title width the old test compares: the detail title, clipped to the frame.
fn title_width(c: &mut Ctx, width: u16) -> u16 {
    let old = c.is_old();
    let sim = c.sim();
    let title = if old {
        sim.app.git_detail_title().unwrap().to_string()
    } else {
        let content = sim.app.project_content().unwrap();
        let crate::app::project_screens::ProjectContent::Diff(diff) = content.as_ref() else {
            panic!("diff required")
        };
        diff.header[0].to_string()
    };
    ratatui::text::Line::raw(title)
        .width()
        .min(usize::from(width - 4)) as u16
}

pub(super) fn diff(c: &mut Ctx) {
    let (_guard, parent) = stable_root("vcs_worktrees_diff");
    let dir = parent.parent().unwrap().to_path_buf();
    let (main, linked, _detached) = seed_worktrees(&dir);
    super::super::set_home(&dir);
    let body = (0..70)
        .map(|i| format!("line {i} 日本語 {}\n", "long ".repeat(30)))
        .collect::<String>();
    std::fs::write(linked.join("file.txt"), body).unwrap();
    commit_worktree(&linked);
    let uncommitted = (0..70)
        .map(|i| format!("uncommitted {i} {}\n", "long ".repeat(30)))
        .collect::<String>();
    std::fs::write(main.join("file.txt"), uncommitted).unwrap();
    for lang in ["en", "ja"] {
        for width in [80, 160] {
            for (name, path) in [("linked", &linked), ("main", &main)] {
                open_worktrees(c, &main, lang, width);
                select(c, path);
                c.key('d');
                let label = format!("lang={lang} w={width} worktree={name}");
                let tw = title_width(c, width);
                // Old title at (x + 2, 1); new title at (x, 2).
                c.region(&format!("{label} title"), 0..tw, 0..1, (2, 1), (0, 2));
                // The old frame has a border and one title row more than the new diff header.
                let diff_region = |c: &mut Ctx, what: &str| {
                    c.region(
                        &format!("{label} {what}"),
                        0..width - 2,
                        2..16,
                        (1, 0),
                        (0, 2),
                    );
                };
                diff_region(c, "diff");
                for key in ['j', 'l', 's'] {
                    c.key(key);
                    diff_region(c, &format!("diff key={key}"));
                }
                c.key('q');
                c.body(&format!("{label} back"), width, 20);
            }
        }
    }
}
