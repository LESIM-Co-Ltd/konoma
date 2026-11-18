//! Tests of the `/` filter component that came out of a mutation audit: the rules that decide
//! which rows a query leaves, where the cursor goes, and what starting and clearing a query
//! touch. They set the lists' state directly, so every list (the version-control ones too) is
//! reached without a repository.

use super::*;
use crate::config::Config;
use crate::test_support::unique_tmp;

fn app() -> (App, crate::test_support::TmpDir) {
    let dir = unique_tmp("konoma_list_filter");
    std::fs::create_dir_all(&dir).unwrap();
    (App::new(dir.to_path_buf(), Config::default()).unwrap(), dir)
}

/// An app with three tabs rooted in directories called `zq-red`, `zq-blue` and `zq-rose`.
fn three_tabs() -> (App, crate::test_support::TmpDir) {
    let (mut app, dir) = app();
    for n in ["zq-red", "zq-blue", "zq-rose"] {
        std::fs::create_dir_all(dir.join(n)).unwrap();
    }
    let root = dir.to_path_buf();
    app.tab.root = root.join("zq-red");
    app.tab_new().unwrap();
    app.tab.root = root.join("zq-blue");
    app.tab_new().unwrap();
    app.tab.root = root.join("zq-rose");
    (app, dir)
}

fn all_lists_are_listed_once() {
    use std::collections::HashSet;
    let set: HashSet<String> = ListId::ALL.iter().map(|l| format!("{l:?}")).collect();
    assert_eq!(set.len(), ListId::ALL.len());
}

#[test]
fn the_list_of_lists_names_every_list() {
    all_lists_are_listed_once();
    for l in [ListId::Bookmarks, ListId::Tabs, ListId::Outline] {
        assert!(ListId::ALL.contains(&l), "{l:?}");
    }
    assert_eq!(ListId::ALL.len(), 3);
}

// ---- moving along the shown rows --------------------------------------------------------------

#[test]
fn step_visible_moves_clamps_and_counts_from_the_nearest_shown_row() {
    let v = [2, 5, 9];
    assert_eq!(step_visible(&v, 5, 1), 9);
    assert_eq!(step_visible(&v, 5, -1), 2);
    assert_eq!(step_visible(&v, 9, 1), 9, "clamped at the end");
    assert_eq!(step_visible(&v, 2, -1), 2, "clamped at the start");
    assert_eq!(step_visible(&v, 2, i32::MAX), 9);
    assert_eq!(step_visible(&v, 9, i32::MIN), 2);
    // A cursor on a hidden row counts from the nearest shown row below it.
    assert_eq!(step_visible(&v, 3, 0), 5);
    assert_eq!(step_visible(&v, 3, 1), 9);
    assert_eq!(step_visible(&v, 0, 0), 2);
    // Below every shown row it counts from the last one.
    assert_eq!(step_visible(&v, 12, 0), 9);
    assert_eq!(step_visible(&v, 12, -1), 5);
    // Nothing shown: the cursor stays.
    assert_eq!(step_visible(&[], 4, 1), 4);
}

#[test]
fn an_overlay_step_wraps_along_the_shown_rows_and_along_all_rows_without_a_query() {
    let (mut app, _d) = three_tabs();
    assert_eq!(app.tab_count(), 3);
    // No query: all three rows, wrapping both ways.
    assert_eq!(app.overlay_step(ListId::Tabs, 2, 3, 1), 0);
    assert_eq!(app.overlay_step(ListId::Tabs, 0, 3, -1), 2);
    assert_eq!(app.overlay_step(ListId::Tabs, 0, 3, 1), 1);
    // The query `zq-r` shows the tabs 0 (red) and 2 (rose): the step wraps among those.
    app.tab_list_filter = "zq-r".into();
    assert_eq!(app.list_visible(ListId::Tabs), Some(vec![0, 2]));
    assert_eq!(app.overlay_step(ListId::Tabs, 2, 3, 1), 0, "wraps forward");
    assert_eq!(
        app.overlay_step(ListId::Tabs, 0, 3, -1),
        2,
        "wraps backward"
    );
    assert_eq!(app.overlay_step(ListId::Tabs, 0, 3, 1), 2);
    // A cursor on the hidden row 1 steps from the next shown row.
    assert_eq!(app.overlay_step(ListId::Tabs, 1, 3, 1), 0);
    // A query that hides everything leaves the cursor where it is; no rows at all too.
    app.tab_list_filter = "zzz".into();
    assert_eq!(app.overlay_step(ListId::Tabs, 1, 3, 1), 1);
    assert_eq!(app.overlay_step(ListId::Tabs, 1, 0, 1), 1);
    // The edges.
    app.tab_list_filter = "zq-r".into();
    assert_eq!(app.overlay_edge(ListId::Tabs, 3, false), 0);
    assert_eq!(app.overlay_edge(ListId::Tabs, 3, true), 2);
    app.tab_list_filter = "zzz".into();
    assert_eq!(app.overlay_edge(ListId::Tabs, 3, true), 0);
    app.tab_list_filter.clear();
    assert_eq!(app.overlay_edge(ListId::Tabs, 3, true), 2);
}

// ---- the text a query is matched against ------------------------------------------------------

#[test]
fn a_tab_row_is_numbered_from_one_and_names_the_tab_and_its_root() {
    let (app, _d) = three_tabs();
    for i in 0..3 {
        let t = app.tab_row_text(i);
        assert!(
            t.starts_with(&format!("{}:{}", i + 1, app.tab_label(i))),
            "{t}"
        );
        assert!(t.contains("  —  "), "{t}");
    }
    // So a query by the number finds the tab.
    let mut app = app;
    app.tab_list_filter = "3:".into();
    assert_eq!(app.list_visible(ListId::Tabs), Some(vec![2]));
    app.tab_list_filter = "0:".into();
    assert_eq!(app.list_visible(ListId::Tabs), Some(vec![]));
}

#[cfg(feature = "git")]
// ---- starting, typing and clearing a query ----------------------------------------------------
#[test]
fn starting_a_query_empties_it_and_turns_typing_on() {
    let (mut app, _d) = app();
    for l in ListId::ALL {
        app.list_filter_parts(l).0.push_str("old");
        app.list_filter_start(l);
        assert_eq!(app.list_filter_query(l), "", "{l:?}");
        // The overlay lists only count as typing while they are open; the rest at once.
        let open_needed = matches!(l, ListId::Bookmarks | ListId::Tabs | ListId::Outline);
        assert!(*app.list_filter_parts(l).1, "{l:?}");
        if !open_needed {
            assert!(app.list_filtering(l), "{l:?}");
        }
    }
}

fn type_str(app: &mut App, l: ListId, text: &str) {
    for c in text.chars() {
        app.list_filter_push(l, c);
    }
}

#[test]
fn typing_moves_the_cursor_onto_the_nearest_shown_row_below() {
    let (mut app, _d) = three_tabs();
    app.tab_list = true;
    app.tab_list_sel = 1;
    app.list_filter_start(ListId::Tabs);
    // `zq-r` hides row 1 (blue): the cursor goes to the nearest shown row below it.
    type_str(&mut app, ListId::Tabs, "zq-r");
    assert_eq!(app.list_visible(ListId::Tabs), Some(vec![0, 2]));
    assert_eq!(app.tab_list_sel, 2);
    // A cursor that is on a shown row stays.
    app.tab_list_sel = 0;
    app.list_filter_push(ListId::Tabs, 'e');
    assert_eq!(app.tab_list_sel, 0, "`zq-re` still shows row 0");
}

/// Backspace and Esc both re-place the cursor and leave typing in the right state.
#[test]
fn backspace_and_clear_put_the_cursor_back_on_a_shown_row() {
    let (mut app, _d) = three_tabs();
    app.tab_list = true;
    // Backspace from `zq-blue` (row 1) to `zq-blu`: nothing changes for the shown row. Put the
    // cursor on a row the *longer* query hid, then widen it: a hidden cursor row is only possible
    // while the query is long, so test the other direction too.
    app.tab_list_filter = "zq-rose".into();
    app.tab_list_sel = 1; // hidden by `zq-rose` (row 2 only)
    app.list_filter_backspace(ListId::Tabs); // `zq-ros`: still row 2 only
    assert_eq!(app.list_filter_query(ListId::Tabs), "zq-ros");
    assert_eq!(
        app.tab_list_sel, 2,
        "Backspace puts the cursor on a shown row"
    );
    app.tab_list_filter = "zq-blue".into();
    app.tab_list_sel = 2; // hidden by `zq-blue` (row 1 only)
    app.list_filter_backspace(ListId::Tabs); // `zq-blu`
    assert_eq!(app.tab_list_sel, 1);
    // Esc clears the query, turns typing off and leaves the cursor on a (now shown) row.
    app.tab_list_filter = "zq-r".into();
    app.tab_list_filtering = true;
    app.tab_list_sel = 2;
    app.list_filter_clear(ListId::Tabs);
    assert_eq!(app.list_filter_query(ListId::Tabs), "");
    assert!(!app.tab_list_filtering);
    assert_eq!(app.tab_list_sel, 2);
}

/// `Esc` re-places the cursor too: the cursor row of the *list* is moved by `list_filter_changed`,
/// which the clear runs after the query is gone (every row is then shown, so it stays), and
/// `Esc` on a bookmark or outline list does the same.
#[test]
fn clearing_a_query_runs_the_cursor_fix_for_the_overlay_lists_too() {
    let (mut app, _d) = app();
    app.bookmark_list_sel = 4;
    app.outline_sel = 6;
    app.list_filter_clear(ListId::Bookmarks);
    app.list_filter_clear(ListId::Outline);
    // Every row is shown with no query: the cursors are untouched.
    assert_eq!(app.bookmark_list_sel, 4);
    assert_eq!(app.outline_sel, 6);
    assert!(!app.list_filtering(ListId::Bookmarks) && !app.list_filtering(ListId::Outline));
}

#[test]
fn an_overlay_that_closed_under_the_typing_takes_the_typing_with_it() {
    let (mut app, _d) = app();
    app.bookmark_list_filtering = true;
    assert!(!app.list_filtering(ListId::Bookmarks));
    assert_eq!(app.filtering_list(), None);
    app.bookmark_list = true;
    assert!(app.list_filtering(ListId::Bookmarks));
    assert_eq!(app.filtering_list(), Some(ListId::Bookmarks));
    app.bookmark_list = false;
    app.tab_list_filtering = true;
    assert!(!app.list_filtering(ListId::Tabs));
    app.tab_list = true;
    assert!(app.list_filtering(ListId::Tabs));
    app.tab_list = false;
    app.outline_filtering = true;
    assert!(!app.list_filtering(ListId::Outline));
    app.outline_open = true;
    assert!(app.list_filtering(ListId::Outline));
}

#[test]
fn only_a_shown_row_is_acted_on() {
    let (mut app, _d) = three_tabs();
    assert!(app.list_row_shown(ListId::Tabs, 1));
    app.tab_list_filter = "zq-r".into();
    assert!(app.list_row_shown(ListId::Tabs, 0));
    assert!(!app.list_row_shown(ListId::Tabs, 1));
    app.tab_list_filter = "zzz".into();
    assert!(!app.list_row_shown(ListId::Tabs, 0));
}
