//! Reading order of the shapes of one slide: a pure function from rectangles and kinds to an order
//! (design: `docs/FEATURE-OFFICE-PREVIEW.md` section 11-2).
//!
//! The order of the shapes in the file is the z-order (PowerPoint's own reading order), which can
//! differ a lot from what a reader sees. The rule here follows what the people who convert slides
//! to text converge on:
//!
//! 1. title shapes, then subtitle shapes (whatever their position);
//! 2. the rest by position, with a recursive XY-cut: shapes are split into stripes at empty
//!    horizontal bands, or into columns at empty vertical bands (a two-column slide, left/right
//!    comparisons). When both cuts exist, the wider band wins, and columns win a tie (bands within
//!    [`ROW_TOLERANCE`] of each other are a tie: a grid drawn with one gap between rows and columns
//!    is stored with rounding differences of a few EMU, which must not turn it from columns into
//!    rows). A wide empty band separates groups; a narrow one only links the things inside a group.
//!    Cards and left/right comparisons are groups of columns (the gap between the cards is wider
//!    than the gap between a card's title and its text); "label | value" pairs are groups of rows
//!    (the gap between the lines is wider than the gap between a label and its value). What cannot
//!    be split either way is read as rows: top to bottom, and within a row (tops within
//!    [`ROW_TOLERANCE`] of the previous shape's top) left to right;
//!
//!    A shape that holds other shapes entirely (a background picture, a panel, a strip with text
//!    on it) is an **underlay**, and it is read just before the shapes it holds, as the reader
//!    sees it: the underlay and its contents are one unit with the underlay's rectangle, and that
//!    unit takes part in the cut and the rows of the shapes around it. Each shape's parent is the
//!    smallest shape that holds it (of equal rectangles, the earlier in the file is the parent);
//!    each level is ordered by the rule above, and a unit is written as the underlay, then its
//!    contents ordered the same way. A full-slide background is the root of everything, so it comes
//!    first and its contents are cut; side panels are units that make columns; a text box on a
//!    picture is the picture, then the text. Past [`UNDERLAY_MAX_SHAPES`] shapes (the search
//!    compares every pair) no underlays are looked for;
//! 3. shapes with no position, in the order of the file.
//!
//! Shapes that lie wholly outside the slide (when its rectangle is known) are kept, but read after
//! the ones on the slide.
//!
//! No XML, no I/O: the caller resolves the positions (placeholder inheritance, group transforms).

use std::collections::HashMap;

/// Tops closer than this (EMU, 0.05 inch) are one row.
pub(crate) const ROW_TOLERANCE: i64 = 45_720;
/// An empty vertical band narrower than this (EMU, 0.1 inch) does not make two columns.
pub(crate) const MIN_COLUMN_GAP: i64 = 91_440;
/// Underlays are looked for only among this many shapes (the search compares every pair).
const UNDERLAY_MAX_SHAPES: usize = 512;
/// Deepest cut; a slide that needs more is read as rows from there on.
const MAX_DEPTH: usize = 12;

/// An absolute rectangle in EMU. `w` and `h` are never negative.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Rect {
    pub x: i64,
    pub y: i64,
    pub w: i64,
    pub h: i64,
}

impl Rect {
    /// A rectangle from raw values: a negative size is flipped, and values are clamped so the
    /// arithmetic below can never overflow.
    pub fn new(x: i64, y: i64, w: i64, h: i64) -> Rect {
        const LIM: i64 = 1 << 40;
        let c = |v: i64| v.clamp(-LIM, LIM);
        let (mut x, mut y, mut w, mut h) = (c(x), c(y), c(w), c(h));
        if w < 0 {
            x += w;
            w = -w;
        }
        if h < 0 {
            y += h;
            h = -h;
        }
        Rect { x, y, w, h }
    }

    /// Whether `o` lies entirely inside `self` (edges may touch).
    fn contains(&self, o: &Rect) -> bool {
        self.x <= o.x && self.y <= o.y && o.right() <= self.right() && o.bottom() <= self.bottom()
    }

    /// Whether the two share at least a point.
    fn touches(&self, o: &Rect) -> bool {
        self.x <= o.right() && o.x <= self.right() && self.y <= o.bottom() && o.y <= self.bottom()
    }

    pub fn right(&self) -> i64 {
        self.x.saturating_add(self.w)
    }

    pub fn bottom(&self) -> i64 {
        self.y.saturating_add(self.h)
    }

    /// The smallest rectangle holding both.
    pub fn union(&self, o: &Rect) -> Rect {
        let x = self.x.min(o.x);
        let y = self.y.min(o.y);
        Rect {
            x,
            y,
            w: self.right().max(o.right()) - x,
            h: self.bottom().max(o.bottom()) - y,
        }
    }
}

/// What a shape is for the order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Title,
    Subtitle,
    Other,
}

/// One shape (or one group, at its bounding rectangle) to put in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Shape {
    pub rect: Option<Rect>,
    pub kind: Kind,
}

/// The indices of `shapes` in reading order; every index appears exactly once. `slide` is the
/// rectangle of the slide when known: shapes wholly outside it are read after those on it.
pub(crate) fn reading_order(shapes: &[Shape], slide: Option<Rect>) -> Vec<usize> {
    let mut titles = Vec::new();
    let mut subs = Vec::new();
    let mut placed = Vec::new();
    let mut off_slide = Vec::new();
    let mut loose = Vec::new();
    for (i, s) in shapes.iter().enumerate() {
        match (s.kind, s.rect) {
            (Kind::Title, _) => titles.push(i),
            (Kind::Subtitle, _) => subs.push(i),
            (Kind::Other, Some(r)) => match slide {
                Some(sl) if !sl.touches(&r) => off_slide.push(i),
                _ => placed.push(i),
            },
            (Kind::Other, None) => loose.push(i),
        }
    }
    let mut out = titles;
    out.extend(subs);
    // The units are the roots of the holding tree over every shape that has a position; those on
    // the slide come first, then those off it.
    let all: Vec<usize> = placed.iter().chain(&off_slide).copied().collect();
    let children = holding_tree(shapes, &all);
    let is_child: std::collections::HashSet<usize> = children.values().flatten().copied().collect();
    for group in [&placed, &off_slide] {
        let roots: Vec<usize> = group
            .iter()
            .copied()
            .filter(|i| !is_child.contains(i))
            .collect();
        level(shapes, &roots, &children, &mut out);
    }
    out.extend(loose);
    out
}

/// The union of some rectangles (`None` for none).
pub(crate) fn bounding(rects: impl IntoIterator<Item = Rect>) -> Option<Rect> {
    rects.into_iter().reduce(|a, b| a.union(&b))
}

#[derive(Clone, Copy)]
enum Axis {
    X,
    Y,
}

fn rect_of(shapes: &[Shape], i: usize) -> Rect {
    // `idx` only holds shapes that have a rectangle.
    shapes[i].rect.unwrap_or(Rect {
        x: 0,
        y: 0,
        w: 0,
        h: 0,
    })
}

fn span(r: &Rect, a: Axis) -> (i64, i64) {
    match a {
        Axis::X => (r.x, r.right()),
        Axis::Y => (r.y, r.bottom()),
    }
}

/// Splits `idx` into groups at the empty bands of the projection on `axis` wider than `min_gap`;
/// `None` when there is only one group. Also returns the widest band.
fn split(
    shapes: &[Shape],
    idx: &[usize],
    axis: Axis,
    min_gap: i64,
) -> Option<(Vec<Vec<usize>>, i64)> {
    let mut v: Vec<usize> = idx.to_vec();
    v.sort_by_key(|&i| (span(&rect_of(shapes, i), axis).0, i));
    let mut groups: Vec<Vec<usize>> = Vec::new();
    let mut cur: Vec<usize> = Vec::new();
    let mut end = i64::MIN;
    let mut widest = 0i64;
    for &i in &v {
        let (s, e) = span(&rect_of(shapes, i), axis);
        let gap = s.saturating_sub(end);
        if !cur.is_empty() && gap > min_gap {
            widest = widest.max(gap);
            groups.push(std::mem::take(&mut cur));
            end = e;
        } else {
            end = end.max(e);
        }
        cur.push(i);
    }
    groups.push(cur);
    (groups.len() > 1).then_some((groups, widest))
}

fn cut(shapes: &[Shape], idx: &[usize], depth: usize, out: &mut Vec<usize>) {
    if idx.len() <= 1 || depth >= MAX_DEPTH {
        rows(shapes, idx, out);
        return;
    }
    let stripes = split(shapes, idx, Axis::Y, 0);
    let columns = split(shapes, idx, Axis::X, MIN_COLUMN_GAP);
    // The wider empty band separates groups; columns win a tie, and a tie is bands within the
    // tolerance of rounding (the same 0.05 inch that makes tops one row).
    let groups = match (stripes, columns) {
        (Some(s), Some(c)) => Some(if c.1.saturating_add(ROW_TOLERANCE) >= s.1 {
            c.0
        } else {
            s.0
        }),
        (Some(s), None) => Some(s.0),
        (None, Some(c)) => Some(c.0),
        (None, None) => None,
    };
    match groups {
        Some(gs) => {
            for g in gs {
                cut(shapes, &g, depth + 1, out);
            }
        }
        None => rows(shapes, idx, out),
    }
}

/// For each shape that holds others, the shapes whose parent it is, in file order. The parent of
/// a shape is the smallest shape that holds it; of equal rectangles the earlier one holds the
/// later (a chain). Empty past [`UNDERLAY_MAX_SHAPES`] shapes.
fn holding_tree(shapes: &[Shape], idx: &[usize]) -> HashMap<usize, Vec<usize>> {
    let mut kids: HashMap<usize, Vec<usize>> = HashMap::new();
    if idx.len() < 2 || idx.len() > UNDERLAY_MAX_SHAPES {
        return kids;
    }
    let area = |i: usize| {
        let r = rect_of(shapes, i);
        i128::from(r.w) * i128::from(r.h)
    };
    for &i in idx {
        let ri = rect_of(shapes, i);
        let parent = idx
            .iter()
            .copied()
            .filter(|&j| {
                j != i && {
                    let rj = rect_of(shapes, j);
                    rj.contains(&ri) && (rj != ri || j < i)
                }
            })
            .min_by_key(|&j| (area(j), std::cmp::Reverse(j)));
        if let Some(j) = parent {
            kids.entry(j).or_default().push(i);
        }
    }
    kids
}

/// Writes the units `roots` in reading order, each followed by what it holds.
fn level(
    shapes: &[Shape],
    roots: &[usize],
    children: &HashMap<usize, Vec<usize>>,
    out: &mut Vec<usize>,
) {
    let mut ordered = Vec::with_capacity(roots.len());
    cut(shapes, roots, 0, &mut ordered);
    for r in ordered {
        out.push(r);
        if let Some(k) = children.get(&r) {
            level(shapes, k, children, out);
        }
    }
}

/// Top to bottom; shapes whose top is within [`ROW_TOLERANCE`] of the previous one's are a row,
/// read left to right.
fn rows(shapes: &[Shape], idx: &[usize], out: &mut Vec<usize>) {
    let mut v: Vec<usize> = idx.to_vec();
    v.sort_by_key(|&i| {
        let r = rect_of(shapes, i);
        (r.y, r.x, i)
    });
    let mut row: Vec<usize> = Vec::new();
    let mut prev_top = i64::MIN;
    let flush = |row: &mut Vec<usize>, out: &mut Vec<usize>| {
        row.sort_by_key(|&i| (rect_of(shapes, i).x, i));
        out.append(row);
    };
    for &i in &v {
        let top = rect_of(shapes, i).y;
        if !row.is_empty() && top.saturating_sub(prev_top) > ROW_TOLERANCE {
            flush(&mut row, out);
        }
        row.push(i);
        prev_top = top;
    }
    flush(&mut row, out);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(x: i64, y: i64, w: i64, h: i64) -> Shape {
        Shape {
            rect: Some(Rect::new(x, y, w, h)),
            kind: Kind::Other,
        }
    }

    fn k(kind: Kind, r: Option<(i64, i64, i64, i64)>) -> Shape {
        Shape {
            rect: r.map(|(x, y, w, h)| Rect::new(x, y, w, h)),
            kind,
        }
    }

    const IN: i64 = 914_400;

    #[test]
    fn empty_and_single() {
        assert!(reading_order(&[], None).is_empty());
        assert_eq!(reading_order(&[s(5, 5, 5, 5)], None), vec![0]);
    }

    #[test]
    fn file_order_is_not_reading_order() {
        // The file lists the bottom shape first (z-order).
        let v = [s(0, 5 * IN, IN, IN), s(0, 0, IN, IN), s(0, 2 * IN, IN, IN)];
        assert_eq!(reading_order(&v, None), vec![1, 2, 0]);
    }

    #[test]
    fn title_then_subtitle_then_rest_whatever_the_position() {
        let v = [
            s(0, 0, IN, IN),
            k(Kind::Subtitle, Some((0, 9 * IN, IN, IN))),
            k(Kind::Title, Some((0, 8 * IN, IN, IN))),
        ];
        assert_eq!(reading_order(&v, None), vec![2, 1, 0]);
    }

    #[test]
    fn title_and_subtitle_without_a_position_still_come_first() {
        let v = [
            s(0, 0, IN, IN),
            k(Kind::Subtitle, None),
            k(Kind::Title, None),
        ];
        assert_eq!(reading_order(&v, None), vec![2, 1, 0]);
    }

    #[test]
    fn several_titles_keep_file_order() {
        let v = [
            k(Kind::Title, Some((0, 5 * IN, IN, IN))),
            k(Kind::Title, Some((0, 0, IN, IN))),
        ];
        assert_eq!(reading_order(&v, None), vec![0, 1]);
    }

    #[test]
    fn same_row_is_left_to_right_within_the_tolerance() {
        // Overlapping boxes (no band separates them); tops differ by exactly the tolerance: one
        // row, so left before right although the left one is lower.
        let right = s(2 * IN, 0, 3 * IN, 3 * IN);
        let left = s(0, ROW_TOLERANCE, 3 * IN, 3 * IN);
        assert_eq!(reading_order(&[right, left], None), vec![1, 0]);
    }

    #[test]
    fn one_emu_past_the_tolerance_is_the_next_row() {
        let right = s(2 * IN, 0, 3 * IN, 3 * IN);
        let left = s(0, ROW_TOLERANCE + 1, 3 * IN, 3 * IN);
        // Two rows: the upper (right) one first.
        assert_eq!(reading_order(&[left, right], None), vec![1, 0]);
    }

    #[test]
    fn the_row_tolerance_is_measured_from_the_previous_top() {
        // Three overlapping boxes whose tops step by 0.04 inch: each is within the tolerance of
        // the one before, so all are one row, read left to right.
        let step = 36_576;
        let a = s(4 * IN, 0, 6 * IN, 6 * IN);
        let b = s(2 * IN, step, 6 * IN, 6 * IN);
        let c = s(0, 2 * step, 6 * IN, 6 * IN);
        assert_eq!(reading_order(&[a, b, c], None), vec![2, 1, 0]);
    }

    #[test]
    fn two_columns_are_read_column_by_column() {
        // Left column: two boxes; right column: one box between them in height.
        let l1 = s(0, 0, 4 * IN, 2 * IN);
        let l2 = s(0, 3 * IN, 4 * IN, 2 * IN);
        let r1 = s(5 * IN, IN, 4 * IN, 3 * IN);
        // A pure row sort would give l1, r1, l2.
        assert_eq!(reading_order(&[r1, l2, l1], None), vec![2, 1, 0]);
    }

    #[test]
    fn header_above_two_columns() {
        let header = s(0, 0, 10 * IN, IN);
        let l1 = s(0, 2 * IN, 4 * IN, IN);
        let l2 = s(0, 4 * IN, 4 * IN, IN);
        let r1 = s(5 * IN, 3 * IN, 4 * IN, IN);
        // header, then the left column (l1, l2), then the right column.
        assert_eq!(reading_order(&[r1, l2, header, l1], None), vec![2, 3, 1, 0]);
    }

    #[test]
    fn a_grid_with_wider_row_gaps_is_read_by_rows() {
        // Row band 2 inches, column band 1 inch: the wider band (rows) separates.
        let a = s(0, 0, 4 * IN, IN);
        let b = s(5 * IN, 0, 4 * IN, IN);
        let c = s(0, 3 * IN, 4 * IN, IN);
        let d = s(5 * IN, 3 * IN, 4 * IN, IN);
        assert_eq!(reading_order(&[d, c, b, a], None), vec![3, 2, 1, 0]);
    }

    #[test]
    fn a_picture_with_its_caption_beside_a_text_column_keeps_the_caption_with_the_picture() {
        // The caption is under the picture (band 0.5 inch), the column is far to the right
        // (band 2 inches): the columns are cut first.
        let pic = s(0, 0, 4 * IN, 3 * IN);
        let cap = s(0, 3 * IN + IN / 2, 4 * IN, IN);
        let text = s(6 * IN, 0, 4 * IN, 3 * IN);
        assert_eq!(reading_order(&[text, cap, pic], None), vec![2, 1, 0]);
    }

    #[test]
    fn the_wider_band_wins_and_a_tie_goes_to_columns() {
        // Row band 1 inch; column band 1 inch + 1 EMU: columns (a, c, b, d).
        let grid = |col_gap: i64, row_gap: i64| {
            let x = 4 * IN + col_gap;
            let y = IN + row_gap;
            [
                s(0, 0, 4 * IN, IN),
                s(x, 0, 4 * IN, IN),
                s(0, y, 4 * IN, IN),
                s(x, y, 4 * IN, IN),
            ]
        };
        assert_eq!(reading_order(&grid(IN + 1, IN), None), vec![0, 2, 1, 3]);
        // Equal: columns.
        assert_eq!(reading_order(&grid(IN, IN), None), vec![0, 2, 1, 3]);
        // Row band 1 EMU wider is still a tie (rounding): columns. Wider than the tolerance of
        // rounding: rows (a, b, c, d).
        assert_eq!(reading_order(&grid(IN, IN + 1), None), vec![0, 2, 1, 3]);
        assert_eq!(
            reading_order(&grid(IN, IN + ROW_TOLERANCE), None),
            vec![0, 2, 1, 3]
        );
        assert_eq!(
            reading_order(&grid(IN, IN + ROW_TOLERANCE + 1), None),
            vec![0, 1, 2, 3]
        );
    }

    #[test]
    fn heading_and_body_columns_are_read_column_by_column() {
        // A comparison: heading above body in each column; the gap between the columns (0.2
        // inch) equals the gap between a heading and its body (0.2 inch): columns.
        let gap = IN / 5;
        let lh = s(0, 0, 4 * IN, IN);
        let lb = s(0, IN + gap, 4 * IN, 3 * IN);
        let rh = s(4 * IN + gap, 0, 4 * IN, IN);
        let rb = s(4 * IN + gap, IN + gap, 4 * IN, 3 * IN);
        assert_eq!(reading_order(&[lh, rh, lb, rb], None), vec![0, 2, 1, 3]);
    }

    #[test]
    fn three_cards_are_read_card_by_card() {
        let g = IN / 4;
        let w = 3 * IN;
        let mut v = Vec::new();
        for c in 0..3 {
            v.push(s(c * (w + g), 0, w, IN)); // title
        }
        for c in 0..3 {
            v.push(s(c * (w + g), IN + g, w, 2 * IN)); // body
        }
        assert_eq!(reading_order(&v, None), vec![0, 3, 1, 4, 2, 5]);
    }

    #[test]
    fn cards_with_a_narrow_row_gap_and_a_wide_column_gap() {
        let w = 3 * IN;
        let mut v = Vec::new();
        for c in 0..3 {
            v.push(s(c * (w + 2 * IN / 5), 0, w, IN));
        }
        for c in 0..3 {
            v.push(s(c * (w + 2 * IN / 5), IN + IN / 10, w, 2 * IN));
        }
        assert_eq!(reading_order(&v, None), vec![0, 3, 1, 4, 2, 5]);
    }

    #[test]
    fn label_and_value_pairs_are_read_row_by_row() {
        // Row gap 0.4 inch, label-value gap 0.2 inch: rows.
        let mut v = Vec::new();
        for r in 0..3 {
            let y = r * (IN + 2 * IN / 5);
            v.push(s(0, y, 2 * IN, IN));
            v.push(s(2 * IN + IN / 5, y, 4 * IN, IN));
        }
        assert_eq!(reading_order(&v, None), vec![0, 1, 2, 3, 4, 5]);
    }

    #[test]
    fn narrow_gap_is_not_a_column() {
        // 0.05 inch between the boxes: under MIN_COLUMN_GAP, so rows: a, b by left.
        let a = s(0, 0, 4 * IN, 3 * IN);
        let b = s(4 * IN + 45_720, IN, 4 * IN, 3 * IN);
        let c = s(0, 3 * IN + 10, 4 * IN, IN);
        // Without a column split a / b / c are ordered by top: a, b, c.
        assert_eq!(reading_order(&[c, b, a], None), vec![2, 1, 0]);
    }

    #[test]
    fn caption_under_a_picture_in_a_column() {
        let pic = s(0, 0, 4 * IN, 3 * IN);
        let cap = s(0, 3 * IN + 100, 4 * IN, IN);
        let text = s(5 * IN, 0, 4 * IN, 4 * IN);
        // picture, caption, then the text column.
        assert_eq!(reading_order(&[text, cap, pic], None), vec![2, 1, 0]);
    }

    #[test]
    fn shapes_without_a_position_come_last_in_file_order() {
        let v = [
            k(Kind::Other, None),
            s(0, 5 * IN, IN, IN),
            k(Kind::Other, None),
            s(0, 0, IN, IN),
        ];
        assert_eq!(reading_order(&v, None), vec![3, 1, 0, 2]);
    }

    #[test]
    fn overlapping_shapes_fall_back_to_rows() {
        // A text box on top of a picture: no band separates them.
        let pic = s(0, 0, 6 * IN, 4 * IN);
        let txt = s(IN, IN, 2 * IN, IN);
        assert_eq!(reading_order(&[txt, pic], None), vec![1, 0]);
    }

    #[test]
    fn identical_rects_keep_file_order() {
        let v = [s(0, 0, IN, IN), s(0, 0, IN, IN), s(0, 0, IN, IN)];
        assert_eq!(reading_order(&v, None), vec![0, 1, 2]);
    }

    #[test]
    fn negative_size_is_normalised() {
        let r = Rect::new(10, 10, -4, -6);
        assert_eq!(
            r,
            Rect {
                x: 6,
                y: 4,
                w: 4,
                h: 6
            }
        );
    }

    #[test]
    fn extreme_values_do_not_overflow() {
        let v = [
            s(i64::MAX, i64::MAX, i64::MAX, i64::MAX),
            s(i64::MIN, i64::MIN, i64::MIN, i64::MIN),
            s(0, 0, 0, 0),
        ];
        let o = reading_order(&v, None);
        let mut sorted = o.clone();
        sorted.sort_unstable();
        assert_eq!(sorted, vec![0, 1, 2]);
    }

    #[test]
    fn zero_size_shapes_are_ordered() {
        let v = [s(0, 3 * IN, 0, 0), s(0, 0, 0, 0), s(0, 3 * IN, 0, 0)];
        assert_eq!(reading_order(&v, None), vec![1, 0, 2]);
    }

    #[test]
    fn bounding_is_the_union() {
        let b = bounding([Rect::new(0, 0, 10, 10), Rect::new(20, 5, 10, 10)]);
        assert_eq!(
            b,
            Some(Rect {
                x: 0,
                y: 0,
                w: 30,
                h: 15
            })
        );
        assert_eq!(bounding(std::iter::empty()), None);
    }

    #[test]
    fn result_is_always_a_permutation() {
        // A deterministic pseudo-random sweep: 200 slides of up to 40 shapes.
        let mut seed = 12345u64;
        let mut rnd = |m: i64| {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((seed >> 33) as i64) % m
        };
        for _ in 0..200 {
            let n = (rnd(40) + 1) as usize;
            let v: Vec<Shape> = (0..n)
                .map(|_| match rnd(5) {
                    0 => k(Kind::Title, None),
                    1 => k(Kind::Other, None),
                    2 => k(
                        Kind::Subtitle,
                        Some((rnd(9 * IN), rnd(6 * IN), rnd(3 * IN), rnd(2 * IN))),
                    ),
                    _ => s(rnd(9 * IN), rnd(6 * IN), rnd(3 * IN), rnd(2 * IN)),
                })
                .collect();
            let mut o = reading_order(&v, None);
            assert_eq!(o.len(), n);
            o.sort_unstable();
            assert_eq!(o, (0..n).collect::<Vec<_>>());
        }
    }

    #[test]
    fn many_shapes_stay_fast() {
        // 5,000 shapes in a staircase (no band separates the columns until the cut depth is
        // used up): must not blow up.
        let v: Vec<Shape> = (0..5000)
            .map(|i| s(i * 10, i * 10, 100_000, 100_000))
            .collect();
        assert_eq!(reading_order(&v, None).len(), 5000);
    }

    #[test]
    fn a_full_slide_background_is_read_first_and_the_columns_still_work() {
        // Background picture holding two columns of three text boxes each.
        let mut v = vec![s(0, 0, 10 * IN, 7 * IN)];
        for r in 0..3 {
            v.push(s(IN, IN + r * 2 * IN, 3 * IN, IN)); // left rows 1..3
            v.push(s(6 * IN, IN + r * 2 * IN, 3 * IN, IN)); // right rows 1..3
        }
        assert_eq!(reading_order(&v, None), vec![0, 1, 3, 5, 2, 4, 6]);
    }

    #[test]
    fn the_background_in_the_middle_of_the_file_is_still_first() {
        let l = s(IN, IN, 3 * IN, IN);
        let l2 = s(IN, 3 * IN, 3 * IN, IN);
        let bg = s(0, 0, 10 * IN, 7 * IN);
        let r = s(6 * IN, IN, 3 * IN, IN);
        assert_eq!(reading_order(&[r, l, bg, l2], None), vec![2, 1, 3, 0]);
    }

    #[test]
    fn side_panels_make_columns_and_each_is_read_with_its_content() {
        // Two panels side by side, each holding a heading and a body.
        let pl = s(0, 0, 4 * IN, 6 * IN);
        let pr = s(5 * IN, 0, 4 * IN, 6 * IN);
        let lh = s(IN / 4, IN / 4, 3 * IN, IN);
        let lb = s(IN / 4, 2 * IN, 3 * IN, 3 * IN);
        let rh = s(5 * IN + IN / 4, IN / 4, 3 * IN, IN);
        let rb = s(5 * IN + IN / 4, 2 * IN, 3 * IN, 3 * IN);
        // File order: right things first, then left, to prove the position decides.
        let v = [rb, rh, lb, lh, pr, pl];
        assert_eq!(reading_order(&v, None), vec![5, 3, 2, 4, 1, 0]);
    }

    #[test]
    fn nested_panels_are_read_outside_in() {
        let bg = s(0, 0, 10 * IN, 8 * IN);
        let panel = s(IN, IN, 8 * IN, 6 * IN);
        let a = s(2 * IN, 2 * IN, 3 * IN, IN);
        let b = s(2 * IN, 4 * IN, 3 * IN, IN);
        assert_eq!(reading_order(&[b, a, panel, bg], None), vec![3, 2, 1, 0]);
    }

    #[test]
    fn a_background_that_does_not_hold_everything_still_goes_first() {
        // The second box sticks out of the background: it is not an underlay of everything, but
        // the background holds the first box.
        let bg = s(0, 0, 4 * IN, 4 * IN);
        let inside = s(IN, IN, IN, IN);
        let out = s(3 * IN, 3 * IN, 3 * IN, IN);
        assert_eq!(reading_order(&[out, inside, bg], None), vec![2, 1, 0]);
    }

    #[test]
    fn an_underlay_is_read_just_before_what_it_holds() {
        // Sample_12 slide 3: a strip at the foot holds the second text and overlaps the first by
        // 60,000 EMU; a picture sits at the top right above the title text.
        let pic = s(6_538_048, -3343, 2_692_524, 659_026);
        let title = s(95_250, 476_250, 8_858_250, 830_997);
        let body1 = s(95_250, 1_238_250, 8_858_250, 2_308_324);
        let strip = s(-36_947, 3_492_494, 9_180_947, 3_411_221);
        let body2 = s(45_554, 4_445_311, 8_858_250, 2_308_324);
        // File order: strip first (z-order).
        let v = [strip, title, body1, body2, pic];
        assert_eq!(reading_order(&v, None), vec![4, 1, 2, 0, 3]);
    }

    #[test]
    fn equal_rectangles_form_a_chain_in_file_order() {
        let v = [s(0, 0, IN, IN), s(0, 0, IN, IN), s(0, 0, IN, IN)];
        assert_eq!(reading_order(&v, None), vec![0, 1, 2]);
        // An equal rectangle after the container is its content, wherever the file lists it.
        let w = [
            s(IN / 2, IN / 2, IN / 4, IN / 4),
            s(0, 0, IN, IN),
            s(0, 0, IN, IN),
        ];
        assert_eq!(reading_order(&w, None), vec![1, 2, 0]);
    }

    #[test]
    fn a_holder_off_the_slide_takes_its_contents_with_it() {
        let slide = Rect::new(0, 0, 10 * IN, 7 * IN);
        let panel = s(-50 * IN, 0, 4 * IN, 4 * IN);
        let inside = s(-49 * IN, IN, IN, IN);
        let on = s(0, 0, IN, IN);
        assert_eq!(
            reading_order(&[inside, panel, on], Some(slide)),
            vec![2, 1, 0]
        );
    }

    #[test]
    fn a_panel_beside_a_free_text_is_a_unit_in_the_row() {
        // Left panel (holding a heading) and a text to its right at the same height: the panel
        // and its contents, then the text.
        let panel = s(0, 0, 4 * IN, 4 * IN);
        let head = s(IN / 4, IN / 4, 3 * IN, IN);
        let side = s(5 * IN, 0, 4 * IN, IN);
        assert_eq!(reading_order(&[side, head, panel], None), vec![2, 1, 0]);
    }

    #[test]
    fn many_nested_and_overlapping_shapes_stay_fast() {
        let t = std::time::Instant::now();
        let nested: Vec<Shape> = (0..5000)
            .map(|i| s(i * 10, i * 10, 100_000_000 - i * 40, 100_000_000 - i * 40))
            .collect();
        assert_eq!(reading_order(&nested, None).len(), 5000);
        let mut with_bg = vec![s(0, 0, 50_000 * 5000, 50_000 * 5000)];
        with_bg.extend((0..5000).map(|i| s(i * 10, i * 10, 100_000, 100_000)));
        assert_eq!(reading_order(&with_bg, None).len(), 5001);
        assert!(t.elapsed().as_secs() < 5, "too slow: {:?}", t.elapsed());
    }

    #[test]
    fn shapes_outside_the_slide_are_read_after_the_ones_on_it() {
        let slide = Rect::new(0, 0, 10 * IN, 7 * IN);
        let far = s(-50 * IN, 0, IN, IN);
        let huge = s(0, 100 * IN, IN, IN);
        let on = s(0, 5 * IN, IN, IN);
        let on2 = s(0, 0, IN, IN);
        let loose = k(Kind::Other, None);
        let title = k(Kind::Title, Some((-50 * IN, 0, IN, IN)));
        let v = [far, loose, huge, on, on2, title];
        assert_eq!(reading_order(&v, Some(slide)), vec![5, 4, 3, 0, 2, 1]);
        // Unknown slide size: the old behaviour (plain position).
        assert_eq!(reading_order(&v, None), vec![5, 0, 4, 3, 2, 1]);
    }

    #[test]
    fn a_shape_touching_or_covering_the_slide_is_on_it() {
        let slide = Rect::new(0, 0, 10 * IN, 7 * IN);
        let touching = s(10 * IN, 0, IN, IN);
        let covering = s(-IN, -IN, 30 * IN, 30 * IN);
        let off = s(10 * IN + 1, 0, IN, IN);
        let v = [off, touching, covering];
        let o = reading_order(&v, Some(slide));
        assert_eq!(*o.last().unwrap(), 0);
    }
}
