//! Tests of the rules behind `n` / `N` over changes that came out of a mutation audit: how change
//! marks merge into blocks, which block the view is on, and where a step lands (`Blocks` is built
//! by hand, so every position and both directions can be reached).

use super::*;
use crate::config::Config;
use crate::test_support::unique_tmp;

fn app_at(scroll: u16) -> (App, crate::test_support::TmpDir) {
    let dir = unique_tmp("konoma_change_jump");
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = App::new(dir.to_path_buf(), Config::default()).unwrap();
    app.tab.preview_path = Some(dir.join("f.md"));
    app.tab.preview_scroll = scroll;
    (app, dir)
}

fn blocks(rows: &[usize], max_v: usize) -> Blocks {
    Blocks {
        rows: rows.to_vec(),
        max_v,
    }
}

// ---- merging the marks into blocks -----------------------------------------------------------

#[test]
fn touching_and_overlapping_marks_are_one_block_and_a_gap_starts_another() {
    // Touching: the second starts where the first ends.
    assert_eq!(block_starts(vec![0..3, 3..5]), [0]);
    assert_eq!(block_starts(vec![2..4, 4..6, 6..7]), [2]);
    // Overlapping, and one inside another (the end is the larger).
    assert_eq!(block_starts(vec![0..5, 2..4, 4..6]), [0]);
    assert_eq!(block_starts(vec![0..9, 2..4, 8..10]), [0]);
    // A gap of one line starts a new block.
    assert_eq!(block_starts(vec![0..2, 3..5]), [0, 3]);
    assert_eq!(block_starts(vec![5..6, 8..9, 20..22]), [5, 8, 20]);
    // Out of order input is sorted first.
    assert_eq!(block_starts(vec![20..22, 5..6, 6..8]), [5, 20]);
    // Nothing, and a single mark.
    assert_eq!(block_starts(Vec::new()), Vec::<usize>::new());
    let one = std::iter::once(7..8).collect::<Vec<_>>();
    assert_eq!(block_starts(one), [7]);
    // A mark at the very top.
    assert_eq!(block_starts(vec![0..1, 2..3]), [0, 2]);
}

// ---- which block the view is on --------------------------------------------------------------

#[test]
fn the_current_block_is_the_last_one_at_or_above_the_top_of_the_view() {
    let b = blocks(&[0, 5, 9], 100);
    for (scroll, want) in [
        (0u16, Some(0)),
        (4, Some(0)),
        (5, Some(1)),
        (8, Some(1)),
        (9, Some(2)),
        (50, Some(2)),
    ] {
        let (app, _d) = app_at(scroll);
        assert_eq!(app.current_block(&b), want, "scroll {scroll}");
    }
    // Above every block there is none.
    let b = blocks(&[3, 8], 100);
    let (app, _d) = app_at(1);
    assert_eq!(app.current_block(&b), None);
}

#[test]
fn the_block_the_last_step_landed_on_stays_current_while_the_view_has_not_moved() {
    // The last two blocks cannot both scroll to the top (`max_v` 25): both sit at row 25.
    let b = blocks(&[0, 10, 30, 40], 25);
    let (mut app, d) = app_at(25);
    app.change_at = Some((d.join("f.md"), 3));
    assert_eq!(app.current_block(&b), Some(3), "remembered");
    // Another file's memory is not this file's.
    app.change_at = Some((d.join("other.md"), 3));
    assert_eq!(app.current_block(&b), Some(1), "falls back to the position");
    // A memory that no longer matches the position is not believed.
    app.change_at = Some((d.join("f.md"), 1));
    assert_eq!(app.current_block(&b), Some(1));
    app.change_at = Some((d.join("f.md"), 9));
    assert_eq!(app.current_block(&b), Some(1));
}

// ---- where a step lands ----------------------------------------------------------------------

fn step(scroll: u16, rows: &[usize], max_v: usize, dir: i32) -> u16 {
    let (mut app, _d) = app_at(scroll);
    app.scroll_to_block(&blocks(rows, max_v), dir);
    app.tab.preview_scroll
}

#[test]
fn a_step_on_a_block_goes_to_the_next_or_the_previous_and_wraps() {
    let rows = [0, 10, 20, 30];
    // Forward from the first, a middle and the last (which wraps to the first).
    assert_eq!(step(0, &rows, 100, 1), 10);
    assert_eq!(step(10, &rows, 100, 1), 20);
    assert_eq!(step(30, &rows, 100, 1), 0);
    // Backward from the last, a middle and the first (which wraps to the last).
    assert_eq!(step(30, &rows, 100, -1), 20);
    assert_eq!(step(20, &rows, 100, -1), 10);
    assert_eq!(step(0, &rows, 100, -1), 30);
    // Any positive or negative number steps by one.
    assert_eq!(step(10, &rows, 100, 7), 20);
    assert_eq!(step(10, &rows, 100, -7), 0);
}

#[test]
fn a_step_that_is_between_blocks_goes_to_the_first_block_past_or_before_the_view() {
    let rows = [0, 10, 20, 30];
    // 15 is on no block: forward is the first block below, backward the last above.
    assert_eq!(step(15, &rows, 100, 1), 20);
    assert_eq!(step(15, &rows, 100, -1), 10);
    // Direction 0 counts as forward.
    assert_eq!(step(15, &rows, 100, 0), 20);
    // Past the last block: forward wraps to the first; backward goes to the last.
    assert_eq!(step(35, &rows, 100, 1), 0);
    assert_eq!(step(35, &rows, 100, -1), 30);
    // Above the first block (the first is at 4): backward wraps to the last.
    assert_eq!(step(2, &[4, 10], 100, -1), 10);
    assert_eq!(step(2, &[4, 10], 100, 1), 4);
}

/// The view cannot scroll past `max_v`: a late block lands there, and the next step does not get
/// stuck on it (it is told apart from the one before by what the last step remembered).
#[test]
fn a_step_never_scrolls_past_the_last_scroll_position_and_walks_the_late_blocks() {
    let rows = [0, 10, 30, 40];
    let (mut app, _d) = app_at(10);
    let b = blocks(&rows, 25);
    app.scroll_to_block(&b, 1);
    assert_eq!(
        app.tab.preview_scroll, 25,
        "block 2 is at 30 but the view stops at 25"
    );
    app.scroll_to_block(&b, 1);
    assert_eq!(app.tab.preview_scroll, 25, "block 3 is also at the end");
    assert_eq!(app.change_at.as_ref().map(|c| c.1), Some(3));
    app.scroll_to_block(&b, 1);
    assert_eq!(app.tab.preview_scroll, 0, "and the next wraps");
    app.scroll_to_block(&b, -1);
    assert_eq!(app.tab.preview_scroll, 25);
    assert_eq!(app.change_at.as_ref().map(|c| c.1), Some(3));
    app.scroll_to_block(&b, -1);
    assert_eq!(app.change_at.as_ref().map(|c| c.1), Some(2));
    // A scroll beyond what a `u16` holds is cut, not wrapped.
    let (mut app, _d) = app_at(0);
    app.scroll_to_block(&blocks(&[0, 1_000_000], usize::MAX), 1);
    assert_eq!(app.tab.preview_scroll, u16::MAX);
}

#[test]
fn a_step_remembers_the_block_it_landed_on_for_this_file() {
    let (mut app, d) = app_at(0);
    app.scroll_to_block(&blocks(&[0, 10, 20], 100), 1);
    assert_eq!(app.change_at, Some((d.join("f.md"), 1)));
}

#[test]
fn a_step_with_no_block_only_says_so() {
    let (mut app, d) = app_at(7);
    app.change_at = Some((d.join("f.md"), 2));
    app.scroll_to_block(&blocks(&[], 100), 1);
    assert_eq!(app.tab.preview_scroll, 7, "the view stays");
    assert_eq!(
        app.change_at,
        Some((d.join("f.md"), 2)),
        "and so does the memory"
    );
    assert!(app.flash.is_some());
}
