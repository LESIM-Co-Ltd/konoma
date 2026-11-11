//! Recipes of the old graph tests in `vcs_graph.rs`. Each `same` / `compare_body` site is a frozen
//! frame; assertions on flashes, titles, help and footers stay in the old tests.

use super::super::super::vcs_changes::{drain, plugin_config};
use super::super::super::vcs_log::{
    graph_fixture, graph_jj_fixture, graph_picker_fixture, seed_jj_ids,
};
use super::super::super::*;
use super::super::Ctx;
use super::log::Pin;
use konoma_vcs_core::git2;

/// `graph_pair` of `vcs_graph.rs`: the config, then the graph screen (hub API on the old side,
/// two Tab presses through the project host on the new side).
#[allow(clippy::too_many_arguments)]
fn open_graph(
    c: &mut Ctx,
    root: &std::path::Path,
    lang: &str,
    align: &str,
    size: (u16, u16),
    cap: usize,
    priority: bool,
    icons: bool,
) {
    let (width, height) = size;
    let mut cfg = plugin_config("git", "vcs_graph_parity", lang);
    cfg.ui.commit_meta_align = align.into();
    cfg.ui.graph_max_branches = cap;
    cfg.ui.icons = icons;
    if priority {
        cfg.ui.graph_base_branches = vec![
            "missing-branch".into(),
            "priority-branch-with-a-long-name".into(),
        ];
    }
    let settings = cfg.plugins.entry("git".into()).or_default();
    settings.insert(
        "commit_meta_align".into(),
        toml::Value::String(align.into()),
    );
    settings.insert(
        "graph_max_branches".into(),
        toml::Value::Integer(cap as i64),
    );
    settings.insert(
        "graph_base_branches".into(),
        toml::Value::Array(
            cfg.ui
                .graph_base_branches
                .iter()
                .cloned()
                .map(toml::Value::String)
                .collect(),
        ),
    );
    c.open(root, "git", cfg, width, height);
    enter_graph(c);
}

/// Old hub: `open_git_graph`; project host: Tab twice (changes -> log -> graph).
fn enter_graph(c: &mut Ctx) {
    c.on_old(|old| {
        old.app.open_git_graph();
        assert!(old.app.is_git_graph());
    });
    for _ in 0..2 {
        c.on_new(|new| new.press(KeyCode::Tab, KeyModifiers::NONE));
    }
    c.on_new(|new| assert_eq!(new.app.project_frame().unwrap().key.screen, "graph"));
}

/// `new_graph_tab` of `vcs_graph.rs`: a second tab on the graph screen (project side only).
fn new_graph_tab(c: &mut Ctx) {
    c.on_new(|new| {
        new.app.tab_new().unwrap();
        new.app.tab.projects.wait_for_result();
        new.app.poll_projects();
        new.key('p');
        drain(new);
        for _ in 0..2 {
            new.press(KeyCode::Tab, KeyModifiers::NONE);
            drain(new);
        }
    });
}

/// The tab-change test also opens a tab on the old hub; the jj graph test does not.
fn other_tab(c: &mut Ctx) {
    c.on_old(|old| old.app.tab_new().unwrap());
    new_graph_tab(c);
}

fn back_to_first_tab(c: &mut Ctx) {
    c.on_old(|old| old.app.tab_goto(0));
    c.on_new(|new| new.app.tab_goto(0));
}

pub(super) fn picker_opens(c: &mut Ctx) {
    let dir = graph_picker_fixture();
    for icons in [false, true] {
        open_graph(c, &dir, "en", "right", (160, 16), 12, false, icons);
        c.key('b');
        c.body(&format!("icons={icons} open"), 160, 16);
        c.key('q');
        // Opening the panel refreshes the priority order even before a watch event arrives.
        let repository = git2::Repository::open(&dir).unwrap();
        let head = repository.head().unwrap().peel_to_commit().unwrap();
        repository
            .branch(&format!("added-{icons}"), &head, false)
            .unwrap();
        c.key('b');
        c.body(&format!("icons={icons} reopened-after-new-branch"), 160, 16);
    }
}

pub(super) fn picker_toggles(c: &mut Ctx) {
    let dir = graph_picker_fixture();
    for lang in ["en", "ja"] {
        for icons in [false, true] {
            open_graph(c, &dir, lang, "right", (160, 16), 12, true, icons);
            for (i, key) in ['b', 'x', 'j', 'x', 'a', 'c', 'J', 'K', 'G', 'k', 'g']
                .into_iter()
                .enumerate()
            {
                c.key(key);
                c.body(&format!("lang={lang} icons={icons} key{i}={key}"), 160, 16);
            }
            for (i, (code, mods)) in [
                (KeyCode::Char('d'), KeyModifiers::CONTROL),
                (KeyCode::Char('u'), KeyModifiers::CONTROL),
                (KeyCode::PageDown, KeyModifiers::NONE),
                (KeyCode::Home, KeyModifiers::NONE),
                (KeyCode::Left, KeyModifiers::NONE),
                (KeyCode::Right, KeyModifiers::NONE),
            ]
            .into_iter()
            .enumerate()
            {
                c.press(code, mods);
                c.body(&format!("lang={lang} icons={icons} motion{i}"), 160, 16);
            }
        }
    }
}

pub(super) fn picker_no_filter(c: &mut Ctx) {
    let dir = graph_picker_fixture();
    open_graph(c, &dir, "en", "right", (160, 16), 12, false, true);
    c.key('b');
    c.key('/');
    c.body("after slash", 160, 16);
    c.key('b');
    c.body("after b again", 160, 16);
}

pub(super) fn picker_apply(c: &mut Ctx) {
    let dir = graph_picker_fixture();
    for apply in [KeyCode::Char('l'), KeyCode::Enter] {
        for reorder in [false, true] {
            open_graph(c, &dir, "en", "right", (160, 16), 12, true, true);
            c.key('j');
            for key in ['b', 'x'] {
                c.key(key);
            }
            if reorder {
                c.key('J');
            }
            c.press(apply, KeyModifiers::NONE);
            let label = format!("apply={apply:?} reorder={reorder}");
            c.body(&format!("{label} applied"), 160, 16);
            c.key('b');
            c.body(&format!("{label} reopened"), 160, 16);
        }
    }
}

pub(super) fn picker_cancel(c: &mut Ctx) {
    let dir = graph_picker_fixture();
    for cancel in [KeyCode::Char('h'), KeyCode::Char('q'), KeyCode::Esc] {
        open_graph(c, &dir, "en", "right", (160, 16), 12, true, true);
        for key in ['b', 'x'] {
            c.key(key);
        }
        c.press(cancel, KeyModifiers::NONE);
        c.body(&format!("cancel={cancel:?} discarded"), 160, 16);
        for key in ['b', 'J'] {
            c.key(key);
        }
        c.press(cancel, KeyModifiers::NONE);
        c.key('b');
        // Legacy cancel retains priority order but discards the tentative set.
        c.body(&format!("cancel={cancel:?} order-retained"), 160, 16);
    }
}

pub(super) fn picker_tab_change(c: &mut Ctx) {
    let dir = graph_picker_fixture();
    open_graph(c, &dir, "en", "right", (160, 16), 12, true, true);
    for key in ['b', 'x', 'J'] {
        c.key(key);
    }
    other_tab(c);
    back_to_first_tab(c);
    c.settle();
    c.body("back on the first tab", 160, 16);
    c.key('b');
    c.body("reopened", 160, 16);
}

pub(super) fn picker_jj_source(c: &mut Ctx) {
    for colocated in [false, true] {
        seed_jj_ids(Some(1));
        let dir = {
            let dir = crate::test_support::unique_tmp("vcs_picker_jj");
            std::fs::create_dir_all(&dir).unwrap();
            let dir = dir.into_canonical().unwrap();
            let jj = super::super::super::vcs_log::jj;
            jj(
                &dir,
                &[
                    "git",
                    "init",
                    if colocated {
                        "--colocate"
                    } else {
                        "--no-colocate"
                    },
                    ".",
                ],
            );
            jj(&dir, &["commit", "-m", "Graph 日本語"]);
            jj(&dir, &["bookmark", "create", "-r", "@-", "日本語"]);
            dir
        };
        seed_jj_ids(None);
        let _pin = Pin::new("jj");
        for lang in ["en", "ja"] {
            let cfg = plugin_config("jj", "vcs_picker_jj", lang);
            c.open(&dir, "jj", cfg, 160, 16);
            enter_graph(c);
            let label = format!("colocated={colocated} lang={lang}");
            c.key('b');
            c.body(&format!("{label} picker"), 160, 16);
            c.key('/');
            c.body(&format!("{label} slash"), 160, 16);
            c.enter();
            c.body(&format!("{label} applied"), 160, 16);
        }
    }
}

pub(super) fn cells(c: &mut Ctx) {
    let dir = graph_fixture();
    for lang in ["en", "ja"] {
        for width in [80, 160] {
            for (align, cap, priority, icons) in [
                ("right", 12, false, true),
                ("inline", 12, false, true),
                ("right", 1, false, false),
                ("right", 0, true, false),
            ] {
                open_graph(c, &dir, lang, align, (width, 16), cap, priority, icons);
                let label = format!(
                    "lang={lang} w={width} align={align} cap={cap} prio={priority} icons={icons}"
                );
                c.on_old(|old| {
                    assert!(old
                        .app
                        .git_graph_rows()
                        .iter()
                        .any(|r| r.commit.is_none() && !r.worktree));
                });
                c.body(&format!("{label} open"), width, 16);
                for i in 0..2 {
                    c.key('j');
                    c.body(&format!("{label} move{i}"), width, 16);
                }
                for query in ["日本語", "Main", "nothing-matches", "│"] {
                    c.raw_key('/');
                    for key in query.chars() {
                        c.raw_key(key);
                    }
                    c.enter();
                    c.body(&format!("{label} query={query:?}"), width, 16);
                }
            }
        }
    }
    for width in [80, 160] {
        for height in [5, 6, 7, 8] {
            open_graph(c, &dir, "en", "right", (width, height), 12, false, true);
            let label = format!("w={width} h={height}");
            c.body(&format!("{label} open"), width, height);
            for key in ['G', 'g'] {
                c.key(key);
                c.body(&format!("{label} key={key}"), width, height);
            }
        }
    }
}

/// The commit id of the side branch of the fixture (read from the repository, so both sides
/// look for the same row).
fn side_commit(dir: &std::path::Path) -> String {
    let repository = git2::Repository::open(dir).unwrap();
    let id = repository
        .find_reference("refs/heads/priority-branch-with-a-long-name")
        .unwrap()
        .peel_to_commit()
        .unwrap()
        .id()
        .to_string();
    id
}

/// Move down until the side commit is selected (the old test moves both sides in lockstep).
fn select_side(c: &mut Ctx, side: &str) {
    for _ in 0..50 {
        let old = c.is_old();
        let current = {
            let sim = c.sim();
            if old {
                sim.app
                    .git_graph_selected_row()
                    .and_then(|r| r.commit.clone())
            } else {
                sim.app.project_act_context().selected.map(|s| s.value)
            }
        };
        if current.as_deref() == Some(side) {
            return;
        }
        c.key('j');
    }
    panic!("side commit {side} not reached");
}

pub(super) fn base_reload_detail(c: &mut Ctx) {
    let dir = graph_fixture();
    let side = side_commit(&dir);
    for lang in ["en", "ja"] {
        open_graph(c, &dir, lang, "right", (160, 16), 12, false, true);
        c.key('s');
        c.body(&format!("lang={lang} worktree row s"), 160, 16);
        for worktree in [true, false] {
            if !worktree {
                c.key('j');
                // The old test copies six items here; copying leaves the body untouched.
                select_side(c, &side);
                c.key('s');
                c.body(&format!("lang={lang} side base set"), 160, 16);
                c.key('x');
                c.body(&format!("lang={lang} side base cleared"), 160, 16);
                c.key('x');
                c.key('r');
                c.body(&format!("lang={lang} side reloaded"), 160, 16);
            }
            for key in [KeyCode::Enter, KeyCode::Char('l')] {
                c.press(key, KeyModifiers::NONE);
                let label = format!("lang={lang} worktree={worktree} open={key:?}");
                c.body(&format!("{label} detail"), 160, 16);
                c.key('q');
                c.body(&format!("{label} back"), 160, 16);
            }
        }
    }
}

pub(super) fn jj_cells(c: &mut Ctx) {
    seed_jj_ids(Some(1));
    let dir = graph_jj_fixture();
    seed_jj_ids(None);
    let _pin = Pin::new("jj");
    for lang in ["en", "ja"] {
        let cfg = plugin_config("jj", "vcs_graph_jj", lang);
        c.open(&dir, "jj", cfg, 160, 16);
        enter_graph(c);
        c.body(&format!("lang={lang} open"), 160, 16);
        for all in [true, false] {
            c.key('a');
            c.body(&format!("lang={lang} all={all}"), 160, 16);
            if all {
                // Only the project side changes tabs here (the old test does not touch the hub).
                new_graph_tab(c);
                c.on_new(|new| new.app.tab_goto(0));
            }
        }
    }
}
