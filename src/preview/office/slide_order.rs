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
//!    comparisons). When both cuts exist, stripes win (a grid is read by rows) unless the columns'
//!    band is at least [`COLUMN_PREFERENCE`] times as wide (a picture with its caption beside a
//!    text column). What cannot be split either way is read as rows: top to bottom, and within a
//!    row (tops within [`ROW_TOLERANCE`] of the previous shape's top) left to right;
//! 3. shapes with no position, in the order of the file.
//!
//! No XML, no I/O: the caller resolves the positions (placeholder inheritance, group transforms).

/// Tops closer than this (EMU, 0.05 inch) are one row.
pub(crate) const ROW_TOLERANCE: i64 = 45_720;
/// An empty vertical band narrower than this (EMU, 0.1 inch) does not make two columns.
pub(crate) const MIN_COLUMN_GAP: i64 = 91_440;
/// Columns are preferred to stripes when their empty band is this many times as wide.
pub(crate) const COLUMN_PREFERENCE: i64 = 2;
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

/// The indices of `shapes` in reading order; every index appears exactly once.
pub(crate) fn reading_order(shapes: &[Shape]) -> Vec<usize> {
    let mut titles = Vec::new();
    let mut subs = Vec::new();
    let mut placed = Vec::new();
    let mut loose = Vec::new();
    for (i, s) in shapes.iter().enumerate() {
        match (s.kind, s.rect) {
            (Kind::Title, _) => titles.push(i),
            (Kind::Subtitle, _) => subs.push(i),
            (Kind::Other, Some(_)) => placed.push(i),
            (Kind::Other, None) => loose.push(i),
        }
    }
    let mut out = titles;
    out.extend(subs);
    cut(shapes, &placed, 0, &mut out);
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
    let groups = match (stripes, columns) {
        (Some(s), Some(c)) => {
            if c.1 >= s.1.saturating_mul(COLUMN_PREFERENCE) {
                Some(c.0)
            } else {
                Some(s.0)
            }
        }
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
        assert!(reading_order(&[]).is_empty());
        assert_eq!(reading_order(&[s(5, 5, 5, 5)]), vec![0]);
    }

    #[test]
    fn file_order_is_not_reading_order() {
        // The file lists the bottom shape first (z-order).
        let v = [s(0, 5 * IN, IN, IN), s(0, 0, IN, IN), s(0, 2 * IN, IN, IN)];
        assert_eq!(reading_order(&v), vec![1, 2, 0]);
    }

    #[test]
    fn title_then_subtitle_then_rest_whatever_the_position() {
        let v = [
            s(0, 0, IN, IN),
            k(Kind::Subtitle, Some((0, 9 * IN, IN, IN))),
            k(Kind::Title, Some((0, 8 * IN, IN, IN))),
        ];
        assert_eq!(reading_order(&v), vec![2, 1, 0]);
    }

    #[test]
    fn title_and_subtitle_without_a_position_still_come_first() {
        let v = [
            s(0, 0, IN, IN),
            k(Kind::Subtitle, None),
            k(Kind::Title, None),
        ];
        assert_eq!(reading_order(&v), vec![2, 1, 0]);
    }

    #[test]
    fn several_titles_keep_file_order() {
        let v = [
            k(Kind::Title, Some((0, 5 * IN, IN, IN))),
            k(Kind::Title, Some((0, 0, IN, IN))),
        ];
        assert_eq!(reading_order(&v), vec![0, 1]);
    }

    #[test]
    fn same_row_is_left_to_right_within_the_tolerance() {
        // Overlapping boxes (no band separates them); tops differ by exactly the tolerance: one
        // row, so left before right although the left one is lower.
        let right = s(2 * IN, 0, 3 * IN, 3 * IN);
        let left = s(0, ROW_TOLERANCE, 3 * IN, 3 * IN);
        assert_eq!(reading_order(&[right, left]), vec![1, 0]);
    }

    #[test]
    fn one_emu_past_the_tolerance_is_the_next_row() {
        let right = s(2 * IN, 0, 3 * IN, 3 * IN);
        let left = s(0, ROW_TOLERANCE + 1, 3 * IN, 3 * IN);
        // Two rows: the upper (right) one first.
        assert_eq!(reading_order(&[left, right]), vec![1, 0]);
    }

    #[test]
    fn the_row_tolerance_is_measured_from_the_previous_top() {
        // Three overlapping boxes whose tops step by 0.04 inch: each is within the tolerance of
        // the one before, so all are one row, read left to right.
        let step = 36_576;
        let a = s(4 * IN, 0, 6 * IN, 6 * IN);
        let b = s(2 * IN, step, 6 * IN, 6 * IN);
        let c = s(0, 2 * step, 6 * IN, 6 * IN);
        assert_eq!(reading_order(&[a, b, c]), vec![2, 1, 0]);
    }

    #[test]
    fn two_columns_are_read_column_by_column() {
        // Left column: two boxes; right column: one box between them in height.
        let l1 = s(0, 0, 4 * IN, 2 * IN);
        let l2 = s(0, 3 * IN, 4 * IN, 2 * IN);
        let r1 = s(5 * IN, IN, 4 * IN, 3 * IN);
        // A pure row sort would give l1, r1, l2.
        assert_eq!(reading_order(&[r1, l2, l1]), vec![2, 1, 0]);
    }

    #[test]
    fn header_above_two_columns() {
        let header = s(0, 0, 10 * IN, IN);
        let l1 = s(0, 2 * IN, 4 * IN, IN);
        let l2 = s(0, 4 * IN, 4 * IN, IN);
        let r1 = s(5 * IN, 3 * IN, 4 * IN, IN);
        // header, then the left column (l1, l2), then the right column.
        assert_eq!(reading_order(&[r1, l2, header, l1]), vec![2, 3, 1, 0]);
    }

    #[test]
    fn grid_is_read_by_rows() {
        let a = s(0, 0, 4 * IN, IN);
        let b = s(5 * IN, 0, 4 * IN, IN);
        let c = s(0, 2 * IN, 4 * IN, IN);
        let d = s(5 * IN, 2 * IN, 4 * IN, IN);
        assert_eq!(reading_order(&[d, c, b, a]), vec![3, 2, 1, 0]);
    }

    #[test]
    fn a_picture_with_its_caption_beside_a_text_column_keeps_the_caption_with_the_picture() {
        // The caption is under the picture (band 0.5 inch), the column is far to the right
        // (band 2 inches): the columns are cut first.
        let pic = s(0, 0, 4 * IN, 3 * IN);
        let cap = s(0, 3 * IN + IN / 2, 4 * IN, IN);
        let text = s(6 * IN, 0, 4 * IN, 3 * IN);
        assert_eq!(reading_order(&[text, cap, pic]), vec![2, 1, 0]);
    }

    #[test]
    fn columns_need_to_be_twice_as_far_apart_as_the_rows() {
        // Row band 1 inch; column band just under 2 inches: rows win (a, b, c, d by rows).
        let a = s(0, 0, 4 * IN, IN);
        let b = s(6 * IN - 1000, 0, 4 * IN, IN);
        let c = s(0, 2 * IN, 4 * IN, IN);
        let d = s(6 * IN - 1000, 2 * IN, 4 * IN, IN);
        assert_eq!(reading_order(&[d, c, b, a]), vec![3, 2, 1, 0]);
        // Exactly twice: columns win (a, c, b, d).
        let b = s(6 * IN, 0, 4 * IN, IN);
        let d = s(6 * IN, 2 * IN, 4 * IN, IN);
        assert_eq!(reading_order(&[a, b, c, d]), vec![0, 2, 1, 3]);
    }

    #[test]
    fn narrow_gap_is_not_a_column() {
        // 0.05 inch between the boxes: under MIN_COLUMN_GAP, so rows: a, b by left.
        let a = s(0, 0, 4 * IN, 3 * IN);
        let b = s(4 * IN + 45_720, IN, 4 * IN, 3 * IN);
        let c = s(0, 3 * IN + 10, 4 * IN, IN);
        // Without a column split a / b / c are ordered by top: a, b, c.
        assert_eq!(reading_order(&[c, b, a]), vec![2, 1, 0]);
    }

    #[test]
    fn caption_under_a_picture_in_a_column() {
        let pic = s(0, 0, 4 * IN, 3 * IN);
        let cap = s(0, 3 * IN + 100, 4 * IN, IN);
        let text = s(5 * IN, 0, 4 * IN, 4 * IN);
        // picture, caption, then the text column.
        assert_eq!(reading_order(&[text, cap, pic]), vec![2, 1, 0]);
    }

    #[test]
    fn shapes_without_a_position_come_last_in_file_order() {
        let v = [
            k(Kind::Other, None),
            s(0, 5 * IN, IN, IN),
            k(Kind::Other, None),
            s(0, 0, IN, IN),
        ];
        assert_eq!(reading_order(&v), vec![3, 1, 0, 2]);
    }

    #[test]
    fn overlapping_shapes_fall_back_to_rows() {
        // A text box on top of a picture: no band separates them.
        let pic = s(0, 0, 6 * IN, 4 * IN);
        let txt = s(IN, IN, 2 * IN, IN);
        assert_eq!(reading_order(&[txt, pic]), vec![1, 0]);
    }

    #[test]
    fn identical_rects_keep_file_order() {
        let v = [s(0, 0, IN, IN), s(0, 0, IN, IN), s(0, 0, IN, IN)];
        assert_eq!(reading_order(&v), vec![0, 1, 2]);
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
        let o = reading_order(&v);
        let mut sorted = o.clone();
        sorted.sort_unstable();
        assert_eq!(sorted, vec![0, 1, 2]);
    }

    #[test]
    fn zero_size_shapes_are_ordered() {
        let v = [s(0, 3 * IN, 0, 0), s(0, 0, 0, 0), s(0, 3 * IN, 0, 0)];
        assert_eq!(reading_order(&v), vec![1, 0, 2]);
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
            let mut o = reading_order(&v);
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
        assert_eq!(reading_order(&v).len(), 5000);
    }
}
