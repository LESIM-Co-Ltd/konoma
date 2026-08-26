//! Everything that happens to an edge between "dagre gave me waypoints" and "there is a line on
//! the page".
//!
//! Four steps, in this order, each of which mermaid also does:
//!
//! 1. [`clip`] — throw away dagre's first and last waypoint and replace them with the real
//!    crossings of the two shapes. dagre only ever knew about bounding boxes.
//! 2. [`arc_midpoint`] — the edge's label goes at the half-way point *of the clipped polyline*,
//!    not where dagre put it. Together with reserving a rank for the label before layout (which
//!    dagre's `makeSpaceForEdgeLabels` does when the label has a size), this is what keeps a label
//!    on its line. **Doing only one of the two breaks it**: reserve without re-placing and the
//!    label sits beside the line, re-place without reserving and it lands on top of a node.
//! 3. [`fix_corners`] — round right angles off with a radius of 5 before the curve sees them.
//! 4. [`curve_basis_path`] — a uniform cubic B-spline, mermaid's default `curve: 'basis'`. It
//!    passes through the first and last point and through **none** of the others, which is why
//!    step 1 has to put the first and last point exactly on the shapes' boundaries.
//!
//! Arrow heads are drawn as explicit geometry rather than as SVG `<marker>`s. A marker's
//! `orient="auto"` is resolved from the path's own tangent, so what ends up on the page would
//! depend on the renderer agreeing with us about where the curve points; drawing the head from
//! the clipped polyline's last segment keeps the invariant "the tip is on the target's boundary"
//! something the tests can check on konoma's own model.

use crate::preview::mermaid::flowchart::Arrow;
use crate::preview::mermaid::layout::Point;

use super::clusters;
use super::shapes::{self, Glyph, Size};
use super::svg::num;

/// Radius mermaid rounds a right-angle corner to (`edges.js`'s `fixCorners`).
pub const CORNER_RADIUS: f64 = 5.0;

/// Length of an arrow head along the line.
pub const ARROW_LENGTH: f64 = 9.0;

/// Half the width of an arrow head across the line.
pub const ARROW_HALF_WIDTH: f64 = 4.0;

/// Radius of the `--o` terminator.
pub const CIRCLE_RADIUS: f64 = 4.0;

/// Half the diagonal of the `--x` terminator.
pub const CROSS_HALF: f64 = 4.5;

/// Two points closer than this are the same point. dagre's own pipeline can emit coincident
/// waypoints (its `dedupe_adjacent_edge_points` says so); a zero-length segment would make the
/// arrow head's direction undefined.
const EPS: f64 = 1e-6;

/// What a line has to meet at one of its ends.
///
/// Two different questions with the same answer shape. A node end asks "where does this line
/// leave the outline?", which is [`shapes::intersect`]. A cluster end asks "where does this line
/// leave the frame?", which is a rectangle crossing — and, unlike the node case, it also throws
/// away everything the line was doing *inside* the frame, because that part of the route only
/// exists so dagre had a leaf to aim at.
#[derive(Debug, Clone, PartialEq)]
pub enum End {
    /// An ordinary node: what it is drawn as, its centre, its bounding box.
    Node(Glyph, Point, Size),
    /// A subgraph frame the author named as this end of the edge.
    Cluster(clusters::Rect),
}

/// Turns dagre's waypoints into the polyline that gets drawn, whatever the two ends are.
///
/// For two node ends this is exactly [`clip`], unchanged — a chart with no subgraphs comes out of
/// stage 1d byte for byte as it came out of stage 1c, which is what makes the golden able to say
/// that frames were the only thing that moved.
///
/// With a cluster end the order matters and is not obvious:
///
/// 1. **cut at the frames first**, on the route exactly as dagre returned it. The cut is "keep
///    the part that is outside", so it has to be able to see that the route does eventually get
///    outside — and the point that proves it is the far end's waypoint. Dropping that first (the
///    tempting order, since it is about to be replaced anyway) leaves a route that looks entirely
///    contained and cuts in the wrong place. That was a real bug, caught by the invariant that
///    says an edge naming a block is not drawn inside it;
/// 2. drop dagre's terminal waypoint at the node ends, which sit on that node's bounding box;
/// 3. put the node ends back, aiming at whatever is now next to them — which may be the cut point
///    itself, so this cannot happen before step 1 either.
pub fn route(points: &[Point], tail: &End, head: &End) -> Vec<Point> {
    if let (End::Node(ts, tc, tz), End::Node(hs, hc, hz)) = (tail, head) {
        return clip(points, (*ts, tc.clone(), *tz), (*hs, hc.clone(), *hz));
    }

    let mut pts: Vec<Point> = points.to_vec();
    dedupe(&mut pts);
    if pts.is_empty() {
        return Vec::new();
    }

    if let End::Cluster(rect) = tail {
        pts = clusters::cut_start(&pts, rect);
    }
    if let End::Cluster(rect) = head {
        pts = clusters::cut_end(&pts, rect);
    }

    // Only ever strips a *node's* own waypoint: after a cut the first (or last) point is the
    // crossing of the frame, which is the answer, not a placeholder for one.
    if matches!(head, End::Node(..)) && pts.len() >= 2 {
        pts.pop();
    }
    if matches!(tail, End::Node(..)) && pts.len() >= 2 {
        pts.remove(0);
    }

    if let End::Node(shape, center, size) = tail {
        let target = pts.first().cloned().unwrap_or_else(|| center.clone());
        pts.insert(0, shapes::intersect(*shape, center.clone(), *size, &target));
    }
    if let End::Node(shape, center, size) = head {
        let target = pts.last().cloned().unwrap_or_else(|| center.clone());
        pts.push(shapes::intersect(*shape, center.clone(), *size, &target));
    }

    dedupe(&mut pts);
    pts
}

/// Replaces dagre's bounding-box endpoints with the real crossings of the two shapes.
///
/// Mirrors the three lines `edges.js` runs for every non-cluster edge:
///
/// ```text
/// points = points.slice(1, edge.points.length - 1);
/// points.unshift(tail.intersect(points[0]));
/// points.push(head.intersect(points[points.length - 1]));
/// ```
///
/// `tail` and `head` are `(shape, centre, size)`. When dagre returned fewer than three waypoints
/// there is nothing in the middle to aim at, so each end aims at the other node's centre —
/// upstream would dereference `undefined` here, which is not a behaviour worth reproducing.
pub fn clip(
    points: &[Point],
    tail: (Glyph, Point, Size),
    head: (Glyph, Point, Size),
) -> Vec<Point> {
    let (tail_shape, tail_center, tail_size) = tail;
    let (head_shape, head_center, head_size) = head;

    let mut inner: Vec<Point> = if points.len() >= 3 {
        points[1..points.len() - 1].to_vec()
    } else {
        Vec::new()
    };
    dedupe(&mut inner);

    let first_target = inner
        .first()
        .cloned()
        .unwrap_or_else(|| head_center.clone());
    let last_target = inner.last().cloned().unwrap_or_else(|| tail_center.clone());

    let start = shapes::intersect(tail_shape, tail_center, tail_size, &first_target);
    let end = shapes::intersect(head_shape, head_center, head_size, &last_target);

    let mut out = Vec::with_capacity(inner.len() + 2);
    out.push(start);
    out.append(&mut inner);
    out.push(end);
    dedupe(&mut out);
    out
}

/// Drops points that coincide with their predecessor.
pub fn dedupe(points: &mut Vec<Point>) {
    points.dedup_by(|a, b| (a.x - b.x).abs() < EPS && (a.y - b.y).abs() < EPS);
}

/// Positions of the right-angle corners in a polyline (`edges.js`'s `extractCornerPoints`).
///
/// The exact `==` on coordinates is upstream's and is the right test here: dagre emits runs that
/// share a coordinate bit-for-bit, and a tolerance would start rounding gentle bends into corners.
fn corner_positions(points: &[Point]) -> Vec<usize> {
    let mut out = Vec::new();
    if points.len() < 3 {
        return out;
    }
    for i in 1..points.len() - 1 {
        let (prev, curr, next) = (&points[i - 1], &points[i], &points[i + 1]);
        let vertical_then_horizontal = prev.x == curr.x
            && curr.y == next.y
            && (curr.x - next.x).abs() > 5.0
            && (curr.y - prev.y).abs() > 5.0;
        let horizontal_then_vertical = prev.y == curr.y
            && curr.x == next.x
            && (curr.x - prev.x).abs() > 5.0
            && (curr.y - next.y).abs() > 5.0;
        if vertical_then_horizontal || horizontal_then_vertical {
            out.push(i);
        }
    }
    out
}

/// The point `distance` back along `a -> b` from `b` (`edges.js`'s `findAdjacentPoint`).
fn adjacent_point(a: &Point, b: &Point, distance: f64) -> Point {
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    let len = (dx * dx + dy * dy).sqrt();
    if len == 0.0 {
        return b.clone();
    }
    let ratio = distance / len;
    Point::new(b.x - ratio * dx, b.y - ratio * dy)
}

/// Rounds every right-angle corner of a polyline off with a radius of [`CORNER_RADIUS`].
///
/// A direct port of `edges.js`'s `fixCorners`, including the `sqrt(2) * 2` offset it uses to push
/// the replacement corner into the bend, and including the guard that leaves very short corners
/// alone. mermaid runs this on every curve except its own `rounded`, which uses a different path
/// builder entirely.
pub fn fix_corners(line: &[Point]) -> Vec<Point> {
    let corners = corner_positions(line);
    if corners.is_empty() {
        return line.to_vec();
    }
    let mut out = Vec::with_capacity(line.len() + corners.len() * 2);
    for (i, point) in line.iter().enumerate() {
        if !corners.contains(&i) {
            out.push(point.clone());
            continue;
        }
        let prev = &line[i - 1];
        let next = &line[i + 1];
        let corner = point;

        let new_prev = adjacent_point(prev, corner, CORNER_RADIUS);
        let new_next = adjacent_point(next, corner, CORNER_RADIUS);
        let x_diff = new_next.x - new_prev.x;
        let y_diff = new_next.y - new_prev.y;
        out.push(new_prev.clone());

        let a = std::f64::consts::SQRT_2 * 2.0;
        let mut new_corner = corner.clone();
        if (next.x - prev.x).abs() > 10.0 && (next.y - prev.y).abs() >= 10.0 {
            let r = CORNER_RADIUS;
            if corner.x == new_prev.x {
                new_corner = Point::new(
                    if x_diff < 0.0 {
                        new_prev.x - r + a
                    } else {
                        new_prev.x + r - a
                    },
                    if y_diff < 0.0 {
                        new_prev.y - a
                    } else {
                        new_prev.y + a
                    },
                );
            } else {
                new_corner = Point::new(
                    if x_diff < 0.0 {
                        new_prev.x - a
                    } else {
                        new_prev.x + a
                    },
                    if y_diff < 0.0 {
                        new_prev.y - r + a
                    } else {
                        new_prev.y + r - a
                    },
                );
            }
        }
        out.push(new_corner);
        out.push(new_next);
    }
    out
}

/// Total length of a polyline.
pub fn length(points: &[Point]) -> f64 {
    points
        .windows(2)
        .map(|w| (w[1].x - w[0].x).hypot(w[1].y - w[0].y))
        .sum()
}

/// The point half-way along a polyline **by arc length**.
///
/// mermaid's `traverseEdge`/`calcLabelPosition`. Measuring by arc length rather than by waypoint
/// count is what keeps a label centred on an edge whose waypoints are unevenly spaced — which
/// they always are, because dagre's dummy nodes sit one per rank.
pub fn arc_midpoint(points: &[Point]) -> Option<Point> {
    match points.len() {
        0 => None,
        1 => Some(points[0].clone()),
        _ => {
            let mut remaining = length(points) / 2.0;
            for w in points.windows(2) {
                let d = (w[1].x - w[0].x).hypot(w[1].y - w[0].y);
                if d == 0.0 {
                    continue;
                }
                if d < remaining {
                    remaining -= d;
                    continue;
                }
                let t = remaining / d;
                return Some(Point::new(
                    w[0].x + t * (w[1].x - w[0].x),
                    w[0].y + t * (w[1].y - w[0].y),
                ));
            }
            points.last().cloned()
        }
    }
}

/// Shortens a polyline by `by` px at its end, leaving the tip for the arrow head to occupy.
///
/// Returns the shortened polyline. A line with no room to give (shorter than `by + 1`) is
/// returned unchanged: an arrow head overlapping its own line looks worse than nothing, but a
/// line collapsed to a point has no direction at all.
pub fn trim_end(points: &[Point], by: f64) -> Vec<Point> {
    if by <= 0.0 || points.len() < 2 || length(points) <= by + 1.0 {
        return points.to_vec();
    }
    let mut out = points.to_vec();
    let mut budget = by;
    while out.len() >= 2 {
        let n = out.len();
        let (a, b) = (out[n - 2].clone(), out[n - 1].clone());
        let d = (b.x - a.x).hypot(b.y - a.y);
        if d <= budget {
            budget -= d;
            out.pop();
            continue;
        }
        let t = (d - budget) / d;
        out[n - 1] = Point::new(a.x + t * (b.x - a.x), a.y + t * (b.y - a.y));
        break;
    }
    out
}

/// [`trim_end`] from the other end, for the head an edge like `<-->` draws at its start.
pub fn trim_start(points: &[Point], by: f64) -> Vec<Point> {
    let mut reversed: Vec<Point> = points.iter().rev().cloned().collect();
    reversed = trim_end(&reversed, by);
    reversed.reverse();
    reversed
}

/// Length of the hollow triangle a class diagram's `<|--` ends in.
pub const TRIANGLE_LENGTH: f64 = 12.0;

/// Half the width of that triangle, across the line.
pub const TRIANGLE_HALF_WIDTH: f64 = 7.0;

/// Length of the diamond `*--` and `o--` end in.
pub const DIAMOND_LENGTH: f64 = 16.0;

/// Half the width of that diamond, across the line.
pub const DIAMOND_HALF_WIDTH: f64 = 5.5;

/// Radius of the ring a class diagram's `()--` ends in.
pub const LOLLIPOP_RADIUS: f64 = 5.0;

/// How far from the boundary the mark nearest an entity sits, on an ER relationship.
pub const ER_NEAR: f64 = 8.0;

/// How far back from the boundary a crow's foot's apex sits.
pub const ER_FOOT_LENGTH: f64 = 10.0;

/// How far from the boundary the second mark sits, on an ER relationship.
pub const ER_FAR: f64 = 18.0;

/// Half the height of the bar that means "one" on an ER relationship.
pub const ER_BAR_HALF: f64 = 6.0;

/// Half the spread of the crow's foot that means "many".
///
/// Deliberately wider than [`ER_BAR_HALF`], and it has to be: `}o` and `|o` differ only in
/// whether the mark at the entity is a foot or a bar, so a foot the width of a bar makes the two
/// cardinalities look alike at the size a terminal shows a diagram. Chosen by rasterising the
/// corpus and looking at it.
pub const ER_FOOT_HALF: f64 = 9.0;

/// Radius of the ring that means "zero" on an ER relationship.
pub const ER_RING_RADIUS: f64 = 4.0;

/// What is drawn at one end of an edge.
///
/// One enum over every diagram family, for the same reason [`Glyph`] is one enum over every
/// shape: clipping, trimming and emission are written once and know nothing about which language
/// asked for a mark. A flowchart's [`Arrow`] is a *pair* of ends rather than one, so it is
/// mapped onto two of these by [`Tip::of_arrow`] instead of being wrapped the way [`Glyph::Flow`]
/// wraps a shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Tip {
    /// Nothing is drawn: `A --- B`, or a note's connector.
    #[default]
    None,
    /// A filled triangle. `-->` in every language that has it.
    Arrow,
    /// `--x`: two strokes crossing.
    Cross,
    /// `--o`: a filled disc.
    Circle,
    /// `<|--` / `..|>`: a hollow triangle — inheritance, and (dotted) realization.
    HollowTriangle,
    /// `*--`: a filled diamond — composition.
    FilledDiamond,
    /// `o--`: a hollow diamond — aggregation.
    HollowDiamond,
    /// `()--`: a ring on the boundary — a required/provided interface.
    Lollipop,
    /// `||`: exactly one.
    ErOnlyOne,
    /// `|o`: zero or one.
    ErZeroOrOne,
    /// `}|`: one or more.
    ErOneOrMore,
    /// `}o`: zero or more.
    ErZeroOrMore,
}

impl Tip {
    /// The two ends a flowchart's [`Arrow`] means.
    ///
    /// `Arrow::Invalid` — the two halves of a `x-- text -->` disagreeing — draws nothing, which is
    /// what it did before this enum existed.
    pub fn of_arrow(arrow: Arrow) -> (Tip, Tip) {
        match arrow {
            Arrow::None | Arrow::Invalid => (Tip::None, Tip::None),
            Arrow::Point => (Tip::None, Tip::Arrow),
            Arrow::Cross => (Tip::None, Tip::Cross),
            Arrow::Circle => (Tip::None, Tip::Circle),
            Arrow::DoublePoint => (Tip::Arrow, Tip::Arrow),
            Arrow::DoubleCross => (Tip::Cross, Tip::Cross),
            Arrow::DoubleCircle => (Tip::Circle, Tip::Circle),
        }
    }

    /// How much room this mark needs at its end of the line, in px.
    ///
    /// The line is shortened by this much so the shaft does not run out through the front of the
    /// mark. A mark the shaft is *meant* to pass through — a bar, a ring — asks for nothing.
    pub fn room(self) -> f64 {
        match self {
            Tip::None => 0.0,
            Tip::Arrow => ARROW_LENGTH,
            Tip::Cross => CROSS_HALF * 2.0,
            Tip::Circle => CIRCLE_RADIUS * 2.0,
            Tip::HollowTriangle => TRIANGLE_LENGTH,
            Tip::FilledDiamond | Tip::HollowDiamond => DIAMOND_LENGTH,
            Tip::Lollipop => LOLLIPOP_RADIUS * 2.0,
            // The two bars and the ring are drawn across the shaft, so it runs all the way to the
            // boundary. The crow's foot has its apex on the shaft, so the shaft stops there.
            Tip::ErOnlyOne | Tip::ErZeroOrOne => 0.0,
            Tip::ErOneOrMore | Tip::ErZeroOrMore => ER_FOOT_LENGTH,
        }
    }
}

/// How much room the terminators need at each end, as `(start, end)` px.
pub fn terminator_lengths(start: Tip, end: Tip) -> (f64, f64) {
    (start.room(), end.room())
}

/// The four points of the diamond `*--` and `o--` end in: tip, the two shoulders, and the far
/// corner. Built from the line's own last segment, so it turns with the edge.
pub fn diamond(from: &Point, tip: &Point) -> [Point; 4] {
    let (ux, uy) = unit(from, tip);
    let (nx, ny) = (-uy * DIAMOND_HALF_WIDTH, ux * DIAMOND_HALF_WIDTH);
    let mid = Point::new(
        tip.x - ux * DIAMOND_LENGTH / 2.0,
        tip.y - uy * DIAMOND_LENGTH / 2.0,
    );
    let back = Point::new(tip.x - ux * DIAMOND_LENGTH, tip.y - uy * DIAMOND_LENGTH);
    [
        tip.clone(),
        Point::new(mid.x + nx, mid.y + ny),
        back,
        Point::new(mid.x - nx, mid.y - ny),
    ]
}

/// The three points of the hollow triangle `<|--` ends in.
pub fn triangle(from: &Point, tip: &Point) -> [Point; 3] {
    let (ux, uy) = unit(from, tip);
    let base = Point::new(tip.x - ux * TRIANGLE_LENGTH, tip.y - uy * TRIANGLE_LENGTH);
    let (nx, ny) = (-uy * TRIANGLE_HALF_WIDTH, ux * TRIANGLE_HALF_WIDTH);
    [
        tip.clone(),
        Point::new(base.x + nx, base.y + ny),
        Point::new(base.x - nx, base.y - ny),
    ]
}

/// The two ends of a bar drawn across the line, `distance` back from `tip`.
pub fn cross_bar(from: &Point, tip: &Point, distance: f64, half: f64) -> (Point, Point) {
    let (ux, uy) = unit(from, tip);
    let c = Point::new(tip.x - ux * distance, tip.y - uy * distance);
    let (nx, ny) = (-uy * half, ux * half);
    (
        Point::new(c.x + nx, c.y + ny),
        Point::new(c.x - nx, c.y - ny),
    )
}

/// A point `distance` back along the line from `tip`.
pub fn back_along(from: &Point, tip: &Point, distance: f64) -> Point {
    let (ux, uy) = unit(from, tip);
    Point::new(tip.x - ux * distance, tip.y - uy * distance)
}

/// The crow's foot that means "many": three prongs from an apex [`ER_FOOT_LENGTH`] back along the
/// line out to the boundary, spreading [`ER_FOOT_HALF`] either side of it.
pub fn crows_foot(from: &Point, tip: &Point) -> [(Point, Point); 3] {
    let (ux, uy) = unit(from, tip);
    let apex = Point::new(tip.x - ux * ER_FOOT_LENGTH, tip.y - uy * ER_FOOT_LENGTH);
    let (nx, ny) = (-uy * ER_FOOT_HALF, ux * ER_FOOT_HALF);
    [
        (apex.clone(), tip.clone()),
        (apex.clone(), Point::new(tip.x + nx, tip.y + ny)),
        (apex, Point::new(tip.x - nx, tip.y - ny)),
    ]
}

/// How far back from a line's end a cardinality label is anchored, before its own size is
/// allowed for. Past the longest terminator ([`DIAMOND_LENGTH`]) so the two never sit on top of
/// each other.
pub const CARDINALITY_SET_BACK: f64 = 18.0;

/// Blank space between a cardinality label and the line it belongs to.
pub const CARDINALITY_GAP: f64 = 3.0;

/// Where a cardinality label goes: beside the line, near the end it describes.
///
/// A class diagram's `Customer "1" --> "*" Ticket` puts one string at each end of the line rather
/// than one in the middle, and neither belongs on the line — `"1"` sitting on the shaft reads as
/// a label for the whole relationship. So it is set back from the end past the terminator and
/// pushed clear of the line along the normal, by exactly as much as its own box needs on that
/// axis (the support function of an axis-aligned rectangle), so a vertical line pushes it
/// sideways by half its width and a horizontal one by half its height.
///
/// Both ends are pushed to the **same** side of the line, which takes one sign flip: the vector
/// "from the tip, back along the line" runs *with* the edge at the tail and *against* it at the
/// head, so the two normals derived from it point opposite ways. mermaid puts the two on opposite
/// sides deliberately (`startLabelRight`, `endLabelLeft`); on the short lines a terminal diagram
/// is made of, that reads as two unrelated annotations rather than as the two ends of one
/// relationship.
///
/// The set-back is **capped at a third of the line**, so a cardinality on a short edge stays near
/// its own end instead of drifting into the middle, where the relationship's own label already is.
pub fn end_label_anchor(points: &[Point], at_start: bool, w: f64, h: f64) -> Option<Point> {
    if points.len() < 2 {
        return None;
    }
    let n = points.len();
    let (tip, inward) = if at_start {
        (&points[0], &points[1])
    } else {
        (&points[n - 1], &points[n - 2])
    };
    // `unit` points *at* the tip, so this is the direction from the tip back along the line.
    let (ux, uy) = unit(tip, inward);
    let side = if at_start { 1.0 } else { -1.0 };
    let (nx, ny) = (side * -uy, side * ux);
    let along = CARDINALITY_SET_BACK.min(length(points) / 3.0);
    let across = nx.abs() * w / 2.0 + ny.abs() * h / 2.0 + CARDINALITY_GAP;
    Some(Point::new(
        tip.x + ux * along + nx * across,
        tip.y + uy * along + ny * across,
    ))
}

/// The unit vector along `from -> tip`, or `(1, 0)` when the two coincide.
fn unit(from: &Point, tip: &Point) -> (f64, f64) {
    let (dx, dy) = (tip.x - from.x, tip.y - from.y);
    let len = dx.hypot(dy);
    if len < EPS {
        (1.0, 0.0)
    } else {
        (dx / len, dy / len)
    }
}

/// The `d` of a uniform cubic B-spline through `points` — d3's `curveBasis`, which is mermaid's
/// default `flowchart.curve`.
///
/// The spline touches only the first and last point; every other point is a control point that
/// bends the curve without being on it. d3 compensates at the ends by drawing a straight sixth of
/// the first and last segment, which is reproduced here so the tangent at each end — and so the
/// arrow head that follows it — matches the segment the clipping step produced.
pub fn curve_basis_path(points: &[Point]) -> String {
    let mut d = String::new();
    match points.len() {
        0 => return d,
        1 => {
            d.push_str(&format!("M{},{}", num(points[0].x), num(points[0].y)));
            return d;
        }
        2 => {
            d.push_str(&format!(
                "M{},{}L{},{}",
                num(points[0].x),
                num(points[0].y),
                num(points[1].x),
                num(points[1].y)
            ));
            return d;
        }
        _ => {}
    }

    let bezier = |p0: &Point, p1: &Point, p: &Point| {
        format!(
            "C{},{} {},{} {},{}",
            num((2.0 * p0.x + p1.x) / 3.0),
            num((2.0 * p0.y + p1.y) / 3.0),
            num((p0.x + 2.0 * p1.x) / 3.0),
            num((p0.y + 2.0 * p1.y) / 3.0),
            num((p0.x + 4.0 * p1.x + p.x) / 6.0),
            num((p0.y + 4.0 * p1.y + p.y) / 6.0),
        )
    };

    d.push_str(&format!("M{},{}", num(points[0].x), num(points[0].y)));
    d.push_str(&format!(
        "L{},{}",
        num((5.0 * points[0].x + points[1].x) / 6.0),
        num((5.0 * points[0].y + points[1].y) / 6.0)
    ));
    for i in 2..points.len() {
        d.push_str(&bezier(&points[i - 2], &points[i - 1], &points[i]));
    }
    let last = points.len() - 1;
    d.push_str(&bezier(&points[last - 1], &points[last], &points[last]));
    d.push_str(&format!("L{},{}", num(points[last].x), num(points[last].y)));
    d
}

/// The three points of a filled arrow head whose tip is at `tip` and which points along
/// `from -> tip`.
pub fn arrow_head(from: &Point, tip: &Point) -> [Point; 3] {
    let dx = tip.x - from.x;
    let dy = tip.y - from.y;
    let len = dx.hypot(dy);
    let (ux, uy) = if len < EPS {
        (1.0, 0.0)
    } else {
        (dx / len, dy / len)
    };
    let base = Point::new(tip.x - ux * ARROW_LENGTH, tip.y - uy * ARROW_LENGTH);
    // Normal of the direction, scaled to half the head's width.
    let (nx, ny) = (-uy * ARROW_HALF_WIDTH, ux * ARROW_HALF_WIDTH);
    [
        tip.clone(),
        Point::new(base.x + nx, base.y + ny),
        Point::new(base.x - nx, base.y - ny),
    ]
}
