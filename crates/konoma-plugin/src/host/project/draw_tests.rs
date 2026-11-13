//! The screen tabs on the first row show the key that jumps now.

use std::collections::BTreeSet;

use konoma_host_core::project::DiffLayout;
use ratatui::{buffer::Buffer, layout::Rect};

use super::draw::{draw_project, ProjectDrawValues};
use crate::host::bind::{bind_jumps, JumpBinding};
use crate::project::{ProjectDecl, ProjectScreen};
use crate::Manifest;

fn declaration() -> ProjectDecl {
    ProjectDecl::new()
        .screen(ProjectScreen::new("changes", "Changes"))
        .screen(ProjectScreen::new("branches", "Branches").key('b'))
        .screen(ProjectScreen::new("worktrees", "Worktrees").key('w'))
        // A pushed screen is not a tab.
        .screen(ProjectScreen::new("detail", "Detail").key('z').pushed())
}

fn jumps(user: &[(&str, &str)]) -> Vec<JumpBinding> {
    let manifest = Manifest::new("vcs", "1").project(declaration());
    let table = user
        .iter()
        .map(|(k, a)| (k.to_string(), a.to_string()))
        .collect();
    bind_jumps("vcs", &manifest, (!user.is_empty()).then_some(&table)).0
}

/// The tab row (row 0) as text.
fn tab_row(jumps: &[JumpBinding], lang: &str) -> String {
    let decl = declaration();
    let area = Rect::new(0, 0, 60, 6);
    let mut buf = Buffer::empty(area);
    let marked = BTreeSet::new();
    draw_project(
        &mut buf,
        area,
        ProjectDrawValues {
            declaration: &decl,
            active_screen: "changes",
            cursor: 0,
            scroll: 0,
            hscroll: 0,
            marked: &marked,
            content: None,
            visible_rows: &[],
            layout: DiffLayout::Unified,
            lang,
            jumps,
            unavailable: "",
        },
    );
    (0..area.width)
        .map(|x| buf[(x, 0)].symbol().to_string())
        .collect::<String>()
        .trim_end()
        .to_string()
}

#[test]
fn the_default_tab_bar_shows_the_declared_keys() {
    assert_eq!(
        tab_row(&jumps(&[]), "en"),
        " Changes  b:Branches  w:Worktrees"
    );
}

#[test]
fn a_rebound_key_is_the_one_the_tab_shows() {
    let row = tab_row(&jumps(&[("B", "goto_branches")]), "en");
    assert_eq!(row, " Changes  B:Branches  w:Worktrees");
    assert!(!row.contains("b:Branches"), "{row}");
}

#[test]
fn a_removed_jump_shows_its_tab_without_a_key() {
    let row = tab_row(&jumps(&[("b", "noop")]), "en");
    assert_eq!(row, " Changes  Branches  w:Worktrees");
    let row = tab_row(&jumps(&[("b", "noop"), ("w", "noop")]), "en");
    assert_eq!(row, " Changes  Branches  Worktrees");
}

#[test]
fn a_swap_shows_each_screen_with_the_other_key() {
    let row = tab_row(
        &jumps(&[("w", "goto_branches"), ("b", "goto_worktrees")]),
        "en",
    );
    assert_eq!(row, " Changes  w:Branches  b:Worktrees");
}

#[test]
fn no_jump_bindings_at_all_show_no_keys() {
    // The tab bar reads the effective keys only: the declared ones are not looked at.
    assert_eq!(tab_row(&[], "en"), " Changes  Branches  Worktrees");
}
