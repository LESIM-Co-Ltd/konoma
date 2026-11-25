//! Tests that kill the mutants of `slide_order` (reading order of a slide's shapes) that the
//! first round of tests left alive: the depth limit of the cut, the sweep that finds the empty
//! bands, the widest band, the holding tree (parent choice, the 512-shape limit, equal rectangles)
//! and the shapes outside the slide (every edge, both sides of it).

use super::slide_order::{reading_order, Kind, Rect, Shape, MIN_COLUMN_GAP, ROW_TOLERANCE};

/// The two thresholds are 0.1 inch (a column) and 0.05 inch (a row): the tests below use the
/// literal values, so a changed constant is noticed.
const COLUMN_GAP: i64 = 91_440;
const ROW_TOL: i64 = 45_720;

const IN: i64 = 914_400;
/// Half an inch: the unit of the staircase below.
const U: i64 = IN / 2;

fn s(x: i64, y: i64, w: i64, h: i64) -> Shape {
    Shape {
        rect: Some(Rect::new(x, y, w, h)),
        kind: Kind::Other,
    }
}

fn kind(kind: Kind, r: Option<(i64, i64, i64, i64)>) -> Shape {
    Shape {
        rect: r.map(|(x, y, w, h)| Rect::new(x, y, w, h)),
        kind,
    }
}

fn is_permutation(order: &[usize], n: usize) -> bool {
    let mut seen = vec![false; n];
    order
        .iter()
        .all(|&i| i < n && !std::mem::replace(&mut seen[i], true))
        && order.len() == n
}

// ---------------------------------------------------------------------------------------------
// The depth limit of the cut
// ---------------------------------------------------------------------------------------------

/// A pinwheel in which every cut peels exactly one shape, and in which the order the cuts give
/// (peel order = file order) differs from the order of the rows fallback at every step.
///
/// Shape `k` is a column (left edge of the region, starting 2 inches below its top, to the bottom)
/// or a stripe (top of the region, 2.5 inches thick, the full width), alternating; `first_is_column`
/// says which comes first. The next region lies 2 inches to the right of a column (the only empty
/// band is that one) or 1 inch below a stripe, and the last shape `n` is a small box in the region
/// that is left. A stripe starts higher than the column before it, so reading by rows (top first)
/// would put the stripe before the column.
fn pinwheel(n: usize, first_is_column: bool) -> Vec<Shape> {
    let x_end = 1_000_000 * U;
    let y_end = 1_000_000 * U;
    let (mut x0, mut y0) = (0i64, 0i64);
    let mut v = Vec::with_capacity(n + 1);
    for k in 0..n {
        if (k % 2 == 0) == first_is_column {
            v.push(s(x0, y0 + 4 * U, 2 * U, y_end - (y0 + 4 * U)));
            x0 += 6 * U;
        } else {
            v.push(s(x0, y0, x_end - x0, 5 * U));
            y0 += 7 * U;
        }
    }
    // The box left at the end sits where the next column would start.
    v.push(s(x0, y0 + 4 * U, 2 * U, 2 * U));
    v
}

fn top_left(sh: &Shape) -> (i64, i64) {
    let r = sh.rect.unwrap();
    (r.y, r.x)
}

/// What the rows fallback makes of the shapes `from..` of `v`.
fn by_rows(v: &[Shape], from: usize) -> Vec<usize> {
    let mut rest: Vec<usize> = (from..v.len()).collect();
    rest.sort_by_key(|&i| (top_left(&v[i]), i));
    rest
}

#[test]
fn the_pinwheel_is_peeled_one_shape_per_cut_until_the_depth_limit() {
    // The first 12 cuts (depth 0..=11) are made, the rest is read by rows. 12 is the depth limit:
    // not 11, not 13, not unlimited - whichever kind of shape the 12th is.
    for first_is_column in [true, false] {
        let mut told_apart = false;
        for n in 12..=17 {
            let v = pinwheel(n, first_is_column);
            let got = reading_order(&v, None);
            assert!(is_permutation(&got, v.len()));
            let tag = format!("n = {n}, first_is_column = {first_is_column}: {got:?}");
            assert_eq!(got[..12], (0..12).collect::<Vec<_>>()[..], "{tag}");
            assert_eq!(got[12..], by_rows(&v, 12)[..], "{tag}");
            told_apart |= by_rows(&v, 12) != (12..v.len()).collect::<Vec<_>>();
        }
        assert!(told_apart, "the layout must tell the two orders apart");
    }
}

#[test]
fn the_pinwheel_shorter_than_the_limit_is_cut_all_the_way() {
    // 12 peeled shapes and the box: every cut fits in depth 0..=11 (the box is alone at depth 12
    // and a lone shape needs no cut), so the cut order is the peel order.
    for first_is_column in [true, false] {
        for n in [1usize, 2, 3, 4, 5, 10, 11, 12] {
            let v = pinwheel(n, first_is_column);
            assert_eq!(
                reading_order(&v, None),
                (0..=n).collect::<Vec<_>>(),
                "n = {n}, first_is_column = {first_is_column}"
            );
        }
    }
}

#[test]
fn the_shape_after_the_limit_is_read_by_rows_even_when_it_would_have_come_first_by_the_cut() {
    // With the column first, shape 12 is a column, read after the stripe that follows it by rows.
    // That tells a limit of 12 from one of 13 (which would still cut it); with the stripe first,
    // shape 11 is the column, which tells 12 from 11.
    let v = pinwheel(14, true);
    let got = reading_order(&v, None);
    assert_eq!(got[..12], (0..12).collect::<Vec<_>>()[..], "{got:?}");
    assert_eq!(by_rows(&v, 12)[..2], [13, 12]);
    assert_eq!(got[12..], by_rows(&v, 12)[..], "{got:?}");
}

#[test]
fn a_huge_staircase_neither_overflows_the_stack_nor_takes_forever() {
    // Without the depth limit this recursion is one frame per shape (and a full sort of what is
    // left at each): it overflows the stack or runs for minutes.
    for first_is_column in [true, false] {
        let v = pinwheel(20_000, first_is_column);
        let t = std::time::Instant::now();
        let got = reading_order(&v, None);
        assert!(is_permutation(&got, v.len()));
        assert_eq!(got[..12], (0..12).collect::<Vec<_>>()[..]);
        assert!(t.elapsed().as_secs() < 20, "too slow: {:?}", t.elapsed());
    }
}

// ---------------------------------------------------------------------------------------------
// The sweep that finds the empty bands
// ---------------------------------------------------------------------------------------------

#[test]
fn the_sweep_runs_in_order_of_the_start_not_of_the_end() {
    // A long box over the whole width, and two boxes that stick out of its rows (so no shape
    // holds another) and sit inside its columns: there is no empty vertical band. Sorted by
    // the end, the two small ones would look separated by a band.
    let a = s(0, 0, 10 * IN, 3 * IN);
    let b = s(2 * IN, IN, IN, 5 * IN);
    let c = s(5 * IN, 2 * IN, IN, 5 * IN);
    assert_eq!(reading_order(&[a, b, c], None), vec![0, 1, 2]);
}

#[test]
fn a_shape_inside_the_extent_does_not_pull_the_end_of_the_band_back() {
    // The same, with the small boxes' tops the other way round: rows say a, c, b. If the end of
    // the band followed the last shape (b, ending early) instead of the furthest one, c would
    // be cut off from a and b and read last.
    let a = s(0, 0, 10 * IN, 3 * IN);
    let b = s(2 * IN, 2 * IN, IN, 5 * IN);
    let c = s(5 * IN, IN, IN, 5 * IN);
    assert_eq!(reading_order(&[a, b, c], None), vec![0, 2, 1]);
}

#[test]
fn the_widest_band_counts_not_the_last_one() {
    // Two rows, three columns. The columns are 3 inches and 0.15 inch apart (in either order), the
    // rows 0.5 inch: the widest vertical band (3 inches) beats the rows, and every band wider
    // than the minimum is a cut, so the three columns are read one after the other, each down.
    let cols = |x1: i64, x2: i64, x3: i64| {
        let mut v = Vec::new();
        for x in [x1, x2, x3] {
            v.push(s(x, 0, IN, IN));
        }
        for x in [x1, x2, x3] {
            v.push(s(x, 3 * IN / 2, IN, IN));
        }
        v
    };
    // 0 1 2 = top row (columns 1 2 3); 3 4 5 = bottom row.
    let wide_last = cols(0, 4 * IN, 5 * IN + 3 * IN / 20);
    assert_eq!(reading_order(&wide_last, None), vec![0, 3, 1, 4, 2, 5]);
    let wide_first = cols(0, IN + 3 * IN / 20, 2 * IN + 3 * IN / 20 + 3 * IN);
    assert_eq!(reading_order(&wide_first, None), vec![0, 3, 1, 4, 2, 5]);
    // Both column gaps 0.15 inch: narrower than the rows' 0.5, so the rows are cut first.
    let narrow = cols(0, IN + 3 * IN / 20, 2 * IN + 6 * IN / 20);
    assert_eq!(reading_order(&narrow, None), vec![0, 1, 2, 3, 4, 5]);
}

#[test]
fn a_horizontal_band_of_any_width_above_zero_makes_stripes() {
    // Two shapes side by side and one wide shape under them with a band of 0.08 inch (less than a
    // column needs): the stripes are cut (band > 0), and the pair is then read as columns.
    let p = s(3 * IN, 0, IN, IN);
    let q = s(0, IN / 2, IN, IN);
    let r = s(0, 3 * IN / 2 + IN * 2 / 25, 4 * IN, IN);
    const { assert!(IN * 2 / 25 < COLUMN_GAP) };
    assert_eq!(reading_order(&[p, q, r], None), vec![1, 0, 2]);
}

#[test]
fn a_band_of_one_emu_is_a_stripe_and_touching_shapes_are_not() {
    let p = s(3 * IN, 0, IN, IN);
    let q = s(0, IN / 2, IN, IN);
    let at = |gap: i64| s(0, 3 * IN / 2 + gap, 4 * IN, IN);
    // 1 EMU: stripes (q before p inside the first, then r).
    assert_eq!(reading_order(&[p, q, at(1)], None), vec![1, 0, 2]);
    // 0: one block; rows by the top: p, q, r.
    assert_eq!(reading_order(&[p, q, at(0)], None), vec![0, 1, 2]);
}

#[test]
fn a_vertical_band_must_be_wider_than_the_minimum() {
    // `a` is left and lower, `b` right and higher, and they overlap in height: columns say a, b;
    // rows say b, a.
    let pair = |gap: i64| [s(0, IN / 2, IN, IN), s(IN + gap, 0, IN, IN)];
    assert_eq!(MIN_COLUMN_GAP, COLUMN_GAP);
    assert_eq!(COLUMN_GAP, IN / 10);
    assert_eq!(reading_order(&pair(COLUMN_GAP + 1), None), vec![0, 1]);
    assert_eq!(reading_order(&pair(COLUMN_GAP), None), vec![1, 0]);
    assert_eq!(reading_order(&pair(COLUMN_GAP - 1), None), vec![1, 0]);
}

#[test]
fn the_row_tolerance_is_inclusive_and_reads_left_to_right() {
    // Left shape lower by exactly the tolerance: still one row (left first); one EMU more: the
    // right one is above, so first.
    let pair = |dy: i64| [s(0, dy, 3 * IN, 3 * IN), s(2 * IN, 0, 3 * IN, 3 * IN)];
    assert_eq!(ROW_TOLERANCE, ROW_TOL);
    assert_eq!(ROW_TOL, IN / 20);
    assert_eq!(reading_order(&pair(ROW_TOL), None), vec![0, 1]);
    assert_eq!(reading_order(&pair(ROW_TOL + 1), None), vec![1, 0]);
}

// ---------------------------------------------------------------------------------------------
// The holding tree
// ---------------------------------------------------------------------------------------------

#[test]
fn the_parent_is_the_smallest_shape_that_holds_it_not_the_biggest() {
    // small (1) is held by mid (2), which is held by big (0). Small and mid share their top-left
    // corner, and small comes first in the file: if small were a child of big (the biggest), it
    // would be ordered with mid by (top, left, file index) and read first.
    let big = s(0, 0, 20 * IN, 10 * IN);
    let small = s(IN, IN, IN, IN);
    let mid = s(IN, IN, 8 * IN, 8 * IN);
    assert_eq!(reading_order(&[big, small, mid], None), vec![0, 2, 1]);
}

#[test]
fn of_equal_rectangles_each_is_held_by_the_one_just_before_it() {
    // a, b, c have one rectangle (a chain a > b > c), d sits inside all of them. d is the child of
    // c, the last link; a child of a would be read with b and c by (top, left, index) and come
    // first, because d is first in the file.
    let d = s(0, 0, IN, IN);
    let r = s(0, 0, 5 * IN, 5 * IN);
    assert_eq!(reading_order(&[d, r, r, r], None), vec![1, 2, 3, 0]);
}

#[test]
fn a_rectangle_one_emu_too_big_is_not_held_on_any_side() {
    // P (index 0) is the panel; W (index 1) pokes out of it on its left, between P and the probe
    // O (index 2) in reading by rows. O held by P is read right after P: [P, O, W]; O not held
    // takes its place in the rows: [P, W, O].
    let p = s(10 * IN, 10 * IN, 10 * IN, 10 * IN);
    let w = s(5 * IN, 11 * IN, 6 * IN, IN);
    let run = |o: Shape| reading_order(&[p, w, o], None);
    let held = vec![0, 2, 1];
    let not_held = vec![0, 1, 2];
    // Flush on one side only: held.
    assert_eq!(run(s(10 * IN, 12 * IN, IN, IN)), held, "left flush");
    assert_eq!(run(s(19 * IN, 12 * IN, IN, IN)), held, "right flush");
    assert_eq!(run(s(12 * IN, 19 * IN, IN, IN)), held, "bottom flush");
    // 1 EMU out of the panel on that side only: not held.
    assert_eq!(run(s(10 * IN - 1, 12 * IN, IN + 1, IN)), not_held, "left");
    assert_eq!(run(s(19 * IN, 12 * IN, IN + 1, IN)), not_held, "right");
    assert_eq!(run(s(12 * IN, 19 * IN, IN, IN + 1)), not_held, "bottom");
    // The top edge: the probe shares the panel's top with W between them in the row (W starts
    // at x = 11 in, the probe at x = 12 in), so the rows read P, W, O.
    // (O is taller than W, so W does not hold it.)
    let w_top = s(11 * IN, 10 * IN, 12 * IN, IN);
    let run_top = |o: Shape| reading_order(&[p, w_top, o], None);
    assert_eq!(run_top(s(12 * IN, 10 * IN, IN, 2 * IN)), held, "top flush");
    assert_eq!(
        run_top(s(12 * IN, 10 * IN - 1, IN, 2 * IN + 1)),
        not_held,
        "top"
    );
}

#[test]
fn the_tree_is_built_for_two_shapes_and_not_for_one_more_than_the_limit() {
    // The probe (0, flush with the panel's top-left corner) is first in the file; the panel (1) is
    // bigger. Held: the panel, then what it holds. Not held: both start at the same point and the
    // one first in the file is read first.
    let probe = s(0, 0, IN, IN);
    let panel = s(0, 0, 10 * IN, 10 * IN);
    assert_eq!(reading_order(&[probe, panel], None), vec![1, 0]);
    // With fillers in a column far to the right (each its own stripe), up to 512 shapes that have a
    // position are searched for holders; one more and no holder is looked for.
    let with = |total: usize| {
        let mut v = vec![probe, panel];
        v.extend((0..total - 2).map(|i| s(100 * IN, i as i64 * 2 * IN, IN, IN)));
        v
    };
    let tree = |total: usize| {
        let mut want = vec![1, 0];
        want.extend(2..total);
        want
    };
    let flat = |total: usize| (0..total).collect::<Vec<_>>();
    assert_eq!(reading_order(&with(511), None), tree(511));
    assert_eq!(reading_order(&with(512), None), tree(512));
    assert_eq!(reading_order(&with(513), None), flat(513));
    assert_eq!(reading_order(&with(600), None), flat(600));
    // Shapes without a position and titles do not count towards the limit.
    let mut v = with(512);
    v.extend([
        kind(Kind::Title, None),
        kind(Kind::Subtitle, None),
        kind(Kind::Other, None),
        kind(Kind::Title, Some((0, 0, IN, IN))),
    ]);
    let got = reading_order(&v, None);
    assert_eq!(
        got[..3],
        [512, 515, 513][..],
        "titles in file order (whether placed or not), then the subtitle"
    );
    assert_eq!(
        got[3..5],
        [1, 0],
        "the panel, then the probe: the tree is on"
    );
    assert_eq!(
        *got.last().unwrap(),
        514,
        "the shape without a position is last"
    );
    // Those off the slide do count.
    let slide = Rect::new(0, 0, 200 * IN, 2000 * IN);
    let mut w = with(512);
    w.push(s(-500 * IN, 0, IN, IN));
    let got = reading_order(&w, Some(slide));
    assert_eq!(got.len(), 513);
    assert_eq!(got[..2], [0, 1], "513 shapes with a position: no holders");
}

#[test]
fn shapes_touching_the_slide_edge_are_on_it_and_one_emu_beyond_is_off_it() {
    let slide = Rect::new(0, 0, 10 * IN, 7 * IN);
    // `r` stays on the slide; the probe is read before it when it is on the slide too (it lies
    // above or to the left of it, or is to its left in the same band), after it when it is off.
    let check = |probe: Shape, r: Shape, on: bool, what: &str| {
        let o = reading_order(&[probe, r], Some(slide));
        assert_eq!(o, if on { vec![0, 1] } else { vec![1, 0] }, "{what}");
    };
    // Right edge: the probe starts at x = 10 in (touching) or 1 EMU further.
    let r = s(9 * IN, 6 * IN, IN, IN);
    check(s(10 * IN, 0, IN, IN), r, true, "right, touching");
    check(s(10 * IN + 1, 0, IN, IN), r, false, "right, 1 EMU off");
    // Left edge: the probe ends at x = 0 (touching) or 1 EMU before.
    check(s(-IN, 0, IN, IN), r, true, "left, touching");
    check(s(-IN - 1, 0, IN, IN), r, false, "left, 1 EMU off");
    // Top edge.
    check(s(5 * IN, -IN, IN, IN), r, true, "top, touching");
    check(s(5 * IN, -IN - 1, IN, IN), r, false, "top, 1 EMU off");
    // Bottom edge: `r2` is to the right of the probe in the same band.
    let r2 = s(5 * IN, 6 * IN + IN / 2, IN, IN);
    check(s(0, 7 * IN, IN, IN), r2, true, "bottom, touching");
    check(s(0, 7 * IN + 1, IN, IN), r2, false, "bottom, 1 EMU off");
}

#[test]
fn a_shape_that_only_partly_leaves_the_slide_is_on_it() {
    let slide = Rect::new(0, 0, 10 * IN, 7 * IN);
    // A wide shape under the straddling ones (and above the bottom one): the cut between them is
    // by rows, so on the slide a straddling shape is read in its place and off the slide it would
    // go to the end.
    let r = s(0, 4 * IN, 10 * IN, IN);
    for (what, straddling) in [
        ("right", s(8 * IN, 0, 4 * IN, IN)),
        ("left", s(-3 * IN, 0, 4 * IN, IN)),
        ("top", s(5 * IN, -IN, IN, 2 * IN)),
        ("bottom", s(5 * IN, 6 * IN, IN, 3 * IN)),
    ] {
        // Read by position (before `r`, which is lower) because it is on the slide. The bottom
        // one is lower than `r`, so the band decides: it comes after.
        let o = reading_order(&[r, straddling], Some(slide));
        if what == "bottom" {
            assert_eq!(o, vec![0, 1], "{what}");
        } else {
            assert_eq!(o, vec![1, 0], "{what}");
        }
    }
}

#[test]
fn what_an_off_slide_holder_holds_follows_it_even_when_that_is_on_the_slide() {
    // A strip across the slide's edge holds a box that lies entirely beyond it: the box is read
    // with the strip (the strip is on the slide), not after everything.
    let slide = Rect::new(0, 0, 10 * IN, 7 * IN);
    let boxed = s(12 * IN, IN / 2, IN, IN);
    let strip = s(8 * IN, 0, 6 * IN, 2 * IN);
    let below = s(0, 5 * IN, 10 * IN, IN);
    assert_eq!(
        reading_order(&[boxed, strip, below], Some(slide)),
        vec![1, 0, 2]
    );
}

#[test]
fn off_slide_shapes_are_read_in_their_own_reading_order_after_the_rest() {
    let slide = Rect::new(0, 0, 10 * IN, 7 * IN);
    let on = s(0, 6 * IN, IN, IN);
    // Two off the slide, the lower one first in the file.
    let off_low = s(0, 20 * IN, IN, IN);
    let off_high = s(0, 10 * IN, IN, IN);
    assert_eq!(
        reading_order(&[off_low, off_high, on], Some(slide)),
        vec![2, 1, 0]
    );
}

#[test]
fn coordinates_left_of_the_origin_are_swept_like_any_other() {
    // A band is empty when nothing covers it: a shape wholly at negative x does not join the
    // next one as long as the band between them is wider than the minimum.
    let a = s(-10 * IN, 2 * IN, IN, IN);
    let b = s(-5 * IN, 0, IN, IN);
    assert_eq!(reading_order(&[b, a], None), vec![1, 0]);
    let c = s(-10 * IN, 0, IN, 3 * IN);
    let d = s(-9 * IN + IN / 2, IN, IN, IN);
    let e = s(-5 * IN, 5 * IN, IN, IN);
    // c and d overlap in x (one column), e is further right and lower.
    assert_eq!(reading_order(&[e, d, c], None), vec![2, 1, 0]);
}

#[test]
fn titles_come_first_then_subtitles_whatever_the_positions_and_the_other_kinds() {
    let o = s(0, 0, IN, IN);
    let sub = kind(Kind::Subtitle, Some((0, 9 * IN, IN, IN)));
    let title = kind(Kind::Title, Some((0, 8 * IN, IN, IN)));
    let loose = kind(Kind::Other, None);
    let title_loose = kind(Kind::Title, None);
    let sub_loose = kind(Kind::Subtitle, None);
    assert_eq!(
        reading_order(&[loose, o, sub_loose, sub, title_loose, title], None),
        vec![4, 5, 2, 3, 1, 0]
    );
}

#[test]
fn a_title_with_no_position_is_not_read_among_the_loose_shapes() {
    let a = kind(Kind::Other, None);
    let t = kind(Kind::Title, None);
    let b = kind(Kind::Other, None);
    assert_eq!(reading_order(&[a, t, b], None), vec![1, 0, 2]);
}
