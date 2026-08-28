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

/// Length of the concave chevron a sequence diagram's `-)` ends in.
pub const ASYNC_LENGTH: f64 = 10.0;

/// Half the width of that chevron, across the line.
pub const ASYNC_HALF_WIDTH: f64 = 5.0;

/// How far back from the tip the chevron's notch sits — what makes it concave, and so tells it
/// apart from the filled triangle of `->>` at the size a terminal shows a diagram.
pub const ASYNC_NOTCH: f64 = 4.5;

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
    /// `-)`: the concave chevron mermaid draws for an asynchronous message. Its own marker is
    /// `M 18,7 L9,13 L14,7 L9,1 Z` — four points, not three, and the notch is the whole point.
    Async,
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
            // Only as far as the notch: the shaft is *meant* to reach into the chevron's hollow,
            // which is what makes it read as fletching rather than as a small solid arrow.
            Tip::Async => ASYNC_NOTCH,
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

/// The four points of the concave chevron a `-)` ends in: tip, one shoulder, the notch, the other
/// shoulder. Built from the line's own last segment, so it turns with the edge.
pub fn async_head(from: &Point, tip: &Point) -> [Point; 4] {
    let (ux, uy) = unit(from, tip);
    let back = Point::new(tip.x - ux * ASYNC_LENGTH, tip.y - uy * ASYNC_LENGTH);
    let (nx, ny) = (-uy * ASYNC_HALF_WIDTH, ux * ASYNC_HALF_WIDTH);
    [
        tip.clone(),
        Point::new(back.x + nx, back.y + ny),
        Point::new(tip.x - ux * ASYNC_NOTCH, tip.y - uy * ASYNC_NOTCH),
        Point::new(back.x - nx, back.y - ny),
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

/// The `d` of a straight polyline through `points`.
///
/// **A data path has to touch its data.** [`curve_basis_path`] passes through the first and last
/// point and through *none* of the others, which is exactly what a flowchart edge wants — the
/// waypoints between the ends are a route, not a claim — and exactly wrong for a line plot or a
/// radar curve, where every vertex *is* a value. Smoothing one of those moves the reader's eye off
/// the number it is meant to land on.
pub fn polyline_path(points: &[Point]) -> String {
    let mut d = String::new();
    for (i, p) in points.iter().enumerate() {
        d.push_str(&format!(
            "{}{},{}",
            if i == 0 { "M" } else { "L" },
            num(p.x),
            num(p.y)
        ));
        if i + 1 < points.len() {
            d.push(' ');
        }
    }
    d
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

/// `M{x},{y}` for one point, empty for none — the two cases a real edge (always 2+ waypoints from
/// [`clip`]) never reaches, but every curve function in this module still has to answer *something*
/// for, so they all share one guard instead of each reproducing d3's own answer for them.
///
/// d3's own answer, run through a plain line-drawing `context`, is actually `moveTo` **then
/// `closePath`**: every curve's `lineEnd` checks `this._line !== 0 && this._point === 1` before
/// closing, and `this._line` is only ever set to `0`/`1` by `areaStart`/`areaEnd` — which a plain
/// `d3.line()` (what mermaid, and every path in this module, actually draws with) never calls, so
/// `this._line` stays `undefined`, and `undefined !== 0` is `true`. That is a real, checkable
/// quirk of d3-shape used through `d3.line()` with exactly one point — not a mistake in this
/// doc — but [`curve_basis_path`] already declined to reproduce it (its own one-point branch is a
/// bare `M`), and every curve below stays consistent with that established answer rather than
/// each re-deciding it. It is moot in practice: `clip` never hands a curve fewer than two points.
fn degenerate_path(points: &[Point]) -> Option<String> {
    match points.len() {
        0 => Some(String::new()),
        1 => {
            let mut d = String::new();
            push_move(&mut d, points[0].x, points[0].y);
            Some(d)
        }
        _ => None,
    }
}

fn push_move(d: &mut String, x: f64, y: f64) {
    d.push_str(&format!("M{},{}", num(x), num(y)));
}

fn push_line(d: &mut String, x: f64, y: f64) {
    d.push_str(&format!("L{},{}", num(x), num(y)));
}

fn push_bezier(d: &mut String, x1: f64, y1: f64, x2: f64, y2: f64, x: f64, y: f64) {
    d.push_str(&format!(
        "C{},{} {},{} {},{}",
        num(x1),
        num(y1),
        num(x2),
        num(y2),
        num(x),
        num(y)
    ));
}

fn push_quad(d: &mut String, x1: f64, y1: f64, x: f64, y: f64) {
    d.push_str(&format!("Q{},{} {},{}", num(x1), num(y1), num(x), num(y)));
}

/// The `d` of d3-shape's `curveNatural` through `points`: a natural cubic spline that touches
/// **every** point (unlike [`curve_basis_path`]'s uniform B-spline), each segment's tangent
/// solved so the whole curve's second derivative is continuous.
///
/// Ported from `d3-shape`'s `curve/natural.js`, whose own comment cites
/// <https://www.particleincell.com/2012/bezier-splines/> for the tridiagonal system
/// [`natural_control_points`] solves.
pub fn curve_natural_path(points: &[Point]) -> String {
    if let Some(d) = degenerate_path(points) {
        return d;
    }
    let mut d = String::new();
    push_move(&mut d, points[0].x, points[0].y);
    if points.len() == 2 {
        push_line(&mut d, points[1].x, points[1].y);
        return d;
    }
    let xs: Vec<f64> = points.iter().map(|p| p.x).collect();
    let ys: Vec<f64> = points.iter().map(|p| p.y).collect();
    let (cx0, cx1) = natural_control_points(&xs);
    let (cy0, cy1) = natural_control_points(&ys);
    for i in 1..points.len() {
        push_bezier(
            &mut d,
            cx0[i - 1],
            cy0[i - 1],
            cx1[i - 1],
            cy1[i - 1],
            points[i].x,
            points[i].y,
        );
    }
    d
}

/// d3-shape's `controlPoints`: the two Bezier control-point arrays for a natural cubic spline
/// through the values in `x` (one coordinate at a time — [`curve_natural_path`] calls this once
/// for the x's and once for the y's), each returned array holding one entry per segment.
///
/// Requires `x.len() >= 3` (the caller special-cases 0, 1 and 2 points before ever reaching
/// this), which is what keeps every `n - 1` index below in bounds.
fn natural_control_points(x: &[f64]) -> (Vec<f64>, Vec<f64>) {
    let n = x.len() - 1;
    let mut a = vec![0.0; n];
    let mut b = vec![0.0; n];
    let mut r = vec![0.0; n];
    a[0] = 0.0;
    b[0] = 2.0;
    r[0] = x[0] + 2.0 * x[1];
    for i in 1..n - 1 {
        a[i] = 1.0;
        b[i] = 4.0;
        r[i] = 4.0 * x[i] + 2.0 * x[i + 1];
    }
    a[n - 1] = 2.0;
    b[n - 1] = 7.0;
    r[n - 1] = 8.0 * x[n - 1] + x[n];
    for i in 1..n {
        let m = a[i] / b[i - 1];
        b[i] -= m;
        r[i] -= m * r[i - 1];
    }
    a[n - 1] = r[n - 1] / b[n - 1];
    for i in (0..n - 1).rev() {
        a[i] = (r[i] - a[i + 1]) / b[i];
    }
    b[n - 1] = (x[n] + a[n - 1]) / 2.0;
    for i in 0..n - 1 {
        b[i] = 2.0 * x[i + 1] - a[i + 1];
    }
    (a, b)
}

/// The online state d3-shape's `Cardinal` and `CatmullRom` curves both keep — three most-recently
/// seen points, shifted in on every call — ported field-for-field from `cardinal.js` /
/// `catmullRom.js` so the control flow (including a `switch` case that falls through into the
/// next, and a `lineEnd` that calls back into the *middle* of that same state machine) stays the
/// shape it has upstream rather than being re-derived and risking a subtly different curve.
struct SplineState {
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
    x2: f64,
    y2: f64,
    point: u8,
}

impl SplineState {
    fn new() -> Self {
        Self {
            x0: f64::NAN,
            y0: f64::NAN,
            x1: f64::NAN,
            y1: f64::NAN,
            x2: f64::NAN,
            y2: f64::NAN,
            point: 0,
        }
    }

    fn shift(&mut self, x: f64, y: f64) {
        self.x0 = self.x1;
        self.x1 = self.x2;
        self.x2 = x;
        self.y0 = self.y1;
        self.y1 = self.y2;
        self.y2 = y;
    }
}

/// The `d` of d3-shape's `curveCardinal` (tension `0`, d3's own default) through `points`.
///
/// Ported from `d3-shape`'s `curve/cardinal.js`.
pub fn curve_cardinal_path(points: &[Point]) -> String {
    // `k = (1 - tension) / 6` at tension 0.
    curve_cardinal_like(points, 1.0 / 6.0)
}

fn curve_cardinal_like(points: &[Point], k: f64) -> String {
    if let Some(d) = degenerate_path(points) {
        return d;
    }
    let mut d = String::new();
    let mut st = SplineState::new();
    let emit = |st: &SplineState, d: &mut String, x: f64, y: f64| {
        push_bezier(
            d,
            st.x1 + k * (st.x2 - st.x0),
            st.y1 + k * (st.y2 - st.y0),
            st.x2 + k * (st.x1 - x),
            st.y2 + k * (st.y1 - y),
            st.x2,
            st.y2,
        );
    };
    for p in points {
        match st.point {
            0 => {
                st.point = 1;
                push_move(&mut d, p.x, p.y);
            }
            1 => {
                st.point = 2;
                st.x1 = p.x;
                st.y1 = p.y;
            }
            2 => {
                st.point = 3;
                emit(&st, &mut d, p.x, p.y);
            }
            _ => emit(&st, &mut d, p.x, p.y),
        }
        st.shift(p.x, p.y);
    }
    match st.point {
        2 => push_line(&mut d, st.x2, st.y2),
        3 => emit(&st, &mut d, st.x1, st.y1),
        _ => {}
    }
    d
}

/// d3-shape's epsilon (`src/math.js`), used by `curveCatmullRom` to decide a neighbour segment is
/// long enough to weight the tangent by.
const D3_EPSILON: f64 = 1e-12;

/// The `d` of d3-shape's `curveCatmullRom` — d3's default `alpha = 0.5` (centripetal), which is
/// what mermaid draws for `catmullRom` (it imports the curve unparametrised) — through `points`.
///
/// Ported from `d3-shape`'s `curve/catmullRom.js`. Centripetal Catmull-Rom reduces to the same
/// tangent as [`curve_cardinal_path`] when consecutive segments happen to be the same length
/// (uniform parametrisation and centripetal parametrisation agree there); the two curves differ
/// only once the waypoints are unevenly spaced, which is why the fixed test points below include
/// an uneven set.
pub fn curve_catmull_rom_path(points: &[Point]) -> String {
    if let Some(d) = degenerate_path(points) {
        return d;
    }
    let mut d = String::new();
    let mut st = SplineState::new();
    let mut l01_a = 0.0_f64;
    let mut l12_a = 0.0_f64;
    let mut l23_a = 0.0_f64;
    let mut l01_2a = 0.0_f64;
    let mut l12_2a = 0.0_f64;
    let mut l23_2a = 0.0_f64;
    let alpha = 0.5;

    // The free `point(that, x, y)` helper both `.point()` below and `line_end` (which calls back
    // into it with the last-seen point) use.
    let emit = |st: &SplineState,
                l01_a: f64,
                l12_a: f64,
                l23_a: f64,
                l01_2a: f64,
                l12_2a: f64,
                l23_2a: f64,
                d: &mut String,
                x: f64,
                y: f64| {
        let (mut x1, mut y1) = (st.x1, st.y1);
        let (mut x2, mut y2) = (st.x2, st.y2);
        if l01_a > D3_EPSILON {
            let a = 2.0 * l01_2a + 3.0 * l01_a * l12_a + l12_2a;
            let n = 3.0 * l01_a * (l01_a + l12_a);
            x1 = (x1 * a - st.x0 * l12_2a + st.x2 * l01_2a) / n;
            y1 = (y1 * a - st.y0 * l12_2a + st.y2 * l01_2a) / n;
        }
        if l23_a > D3_EPSILON {
            let b = 2.0 * l23_2a + 3.0 * l23_a * l12_a + l12_2a;
            let m = 3.0 * l23_a * (l23_a + l12_a);
            x2 = (x2 * b + st.x1 * l23_2a - x * l12_2a) / m;
            y2 = (y2 * b + st.y1 * l23_2a - y * l12_2a) / m;
        }
        push_bezier(d, x1, y1, x2, y2, st.x2, st.y2);
    };

    for p in points {
        if st.point != 0 {
            let x23 = st.x2 - p.x;
            let y23 = st.y2 - p.y;
            l23_2a = (x23 * x23 + y23 * y23).powf(alpha);
            l23_a = l23_2a.sqrt();
        }
        match st.point {
            0 => {
                st.point = 1;
                push_move(&mut d, p.x, p.y);
            }
            1 => st.point = 2,
            2 => {
                st.point = 3;
                emit(
                    &st, l01_a, l12_a, l23_a, l01_2a, l12_2a, l23_2a, &mut d, p.x, p.y,
                );
            }
            _ => emit(
                &st, l01_a, l12_a, l23_a, l01_2a, l12_2a, l23_2a, &mut d, p.x, p.y,
            ),
        }
        l01_a = l12_a;
        l12_a = l23_a;
        l01_2a = l12_2a;
        l12_2a = l23_2a;
        st.shift(p.x, p.y);
    }
    match st.point {
        2 => push_line(&mut d, st.x2, st.y2),
        // `lineEnd`'s `case 3` calls `this.point(this._x2, this._y2)` — the full method, with the
        // last point fed back in as a "new" one, not the free `point()` helper directly. That
        // recomputes `l23` as the distance from the last point to itself (zero), so the `l23_a >
        // epsilon` branch never fires here and the emitted control point is `st.x2, st.y2`
        // unweighted — reproduced directly rather than re-running the whole state machine for a
        // point whose only effect is this one, now-known, degenerate case.
        3 => emit(
            &st, l01_a, l12_a, 0.0, l01_2a, l12_2a, l23_2a, &mut d, st.x2, st.y2,
        ),
        _ => {}
    }
    d
}

fn js_sign(x: f64) -> f64 {
    if x < 0.0 {
        -1.0
    } else {
        1.0
    }
}

/// d3-shape's `h0 || h1 < 0 && -0` — used by `slope3` to substitute a signed zero for a
/// coincident x (or y, for the reflected [`curve_monotone_y_path`]) so the division that follows
/// produces a correctly-signed infinity rather than a NaN. Real, not a corner case this crate
/// invented to worry about: two waypoints sharing a coordinate is what an axis-aligned dagre
/// route (an ordinary flowchart corner) *is*.
fn signed_zero_or(h0: f64, h1: f64) -> f64 {
    if h0 != 0.0 {
        h0
    } else if h1 < 0.0 {
        -0.0
    } else {
        0.0
    }
}

/// d3-shape's `slope3` (Steffen 1990) — the two-sided tangent at the middle of three points.
fn slope3(x0: f64, y0: f64, x1: f64, y1: f64, x2: f64, y2: f64) -> f64 {
    let h0 = x1 - x0;
    let h1 = x2 - x1;
    let s0 = (y1 - y0) / signed_zero_or(h0, h1);
    let s1 = (y2 - y1) / signed_zero_or(h1, h0);
    let p = (s0 * h1 + s1 * h0) / (h0 + h1);
    let m = (js_sign(s0) + js_sign(s1)) * s0.abs().min(s1.abs()).min(0.5 * p.abs());
    if m.is_nan() || m == 0.0 {
        0.0
    } else {
        m
    }
}

/// d3-shape's `slope2` — the one-sided tangent at a line's first or last point.
fn slope2(x0: f64, y0: f64, x1: f64, y1: f64, t: f64) -> f64 {
    let h = x1 - x0;
    if h != 0.0 {
        (3.0 * (y1 - y0) / h - t) / 2.0
    } else {
        t
    }
}

/// The online state d3-shape's `MonotoneX` keeps, generalised to draw `curveMonotoneY` too: every
/// coordinate below is named `a`/`b` rather than `x`/`y` because `curveMonotoneY` is, upstream,
/// the *exact same* `MonotoneX` state machine run through a `ReflectContext` that swaps every
/// `x`/`y` on the way in and back out again (`monotone.js`'s own `MonotoneY.prototype.point =
/// function(x,y) { MonotoneX.prototype.point.call(this, y, x); }`). `reflect` here is that swap,
/// applied at the two edges of this struct — [`Monotone::point`]'s caller and every `push_*` call
/// — instead of wrapping a context object, which is what makes one state machine answer both
/// curves without the two ever drifting apart.
struct Monotone {
    reflect: bool,
    a0: f64,
    b0: f64,
    a1: f64,
    b1: f64,
    t0: f64,
    point: u8,
}

impl Monotone {
    fn new(reflect: bool) -> Self {
        Self {
            reflect,
            a0: f64::NAN,
            b0: f64::NAN,
            a1: f64::NAN,
            b1: f64::NAN,
            t0: f64::NAN,
            point: 0,
        }
    }

    fn push_move(&self, d: &mut String, a: f64, b: f64) {
        if self.reflect {
            push_move(d, b, a);
        } else {
            push_move(d, a, b);
        }
    }

    fn push_line(&self, d: &mut String, a: f64, b: f64) {
        if self.reflect {
            push_line(d, b, a);
        } else {
            push_line(d, a, b);
        }
    }

    /// d3-shape's `point(that, t0, t1)`: the Hermite-to-Bezier conversion.
    fn emit_hermite(&self, d: &mut String, t0: f64, t1: f64) {
        let dx = (self.a1 - self.a0) / 3.0;
        let (c1a, c1b) = (self.a0 + dx, self.b0 + dx * t0);
        let (c2a, c2b) = (self.a1 - dx, self.b1 - dx * t1);
        let (ea, eb) = (self.a1, self.b1);
        if self.reflect {
            push_bezier(d, c1b, c1a, c2b, c2a, eb, ea);
        } else {
            push_bezier(d, c1a, c1b, c2a, c2b, ea, eb);
        }
    }

    /// `x`, `y` here are the *real* (unreflected) coordinates a caller hands every curve
    /// function in this module — the swap into `a`/`b` space happens once, right here, exactly
    /// where upstream's `ReflectContext` would have.
    fn point(&mut self, d: &mut String, x: f64, y: f64) {
        let (a, b) = if self.reflect { (y, x) } else { (x, y) };
        if a == self.a1 && b == self.b1 {
            return; // Ignore coincident points.
        }
        let mut t1 = f64::NAN;
        match self.point {
            0 => {
                self.point = 1;
                self.push_move(d, a, b);
            }
            1 => self.point = 2,
            2 => {
                self.point = 3;
                t1 = slope3(self.a0, self.b0, self.a1, self.b1, a, b);
                let t0 = slope2(self.a0, self.b0, self.a1, self.b1, t1);
                self.emit_hermite(d, t0, t1);
            }
            _ => {
                t1 = slope3(self.a0, self.b0, self.a1, self.b1, a, b);
                self.emit_hermite(d, self.t0, t1);
            }
        }
        self.a0 = self.a1;
        self.a1 = a;
        self.b0 = self.b1;
        self.b1 = b;
        self.t0 = t1;
    }

    fn line_end(&mut self, d: &mut String) {
        match self.point {
            2 => self.push_line(d, self.a1, self.b1),
            3 => {
                let t1 = slope2(self.a0, self.b0, self.a1, self.b1, self.t0);
                self.emit_hermite(d, self.t0, t1);
            }
            _ => {}
        }
    }
}

fn curve_monotone_path(points: &[Point], reflect: bool) -> String {
    if let Some(d) = degenerate_path(points) {
        return d;
    }
    let mut d = String::new();
    let mut st = Monotone::new(reflect);
    for p in points {
        st.point(&mut d, p.x, p.y);
    }
    st.line_end(&mut d);
    d
}

/// The `d` of d3-shape's `curveMonotoneX` through `points`: a Hermite spline whose tangents are
/// clamped (Steffen 1990) so the curve never overshoots a waypoint — the property "monotone" is
/// named for, along the x axis.
///
/// Ported from `d3-shape`'s `curve/monotone.js`.
pub fn curve_monotone_x_path(points: &[Point]) -> String {
    curve_monotone_path(points, false)
}

/// [`curve_monotone_x_path`], monotone along the y axis instead of the x axis — d3-shape's
/// `curveMonotoneY`, which upstream is the identical state machine with x and y swapped
/// end to end (see [`Monotone`]'s own doc).
pub fn curve_monotone_y_path(points: &[Point]) -> String {
    curve_monotone_path(points, true)
}

fn curve_bump_path(points: &[Point], is_x: bool) -> String {
    if let Some(d) = degenerate_path(points) {
        return d;
    }
    let mut d = String::new();
    push_move(&mut d, points[0].x, points[0].y);
    let (mut x0, mut y0) = (points[0].x, points[0].y);
    for p in &points[1..] {
        if is_x {
            let mx = (x0 + p.x) / 2.0;
            push_bezier(&mut d, mx, y0, mx, p.y, p.x, p.y);
        } else {
            let my = (y0 + p.y) / 2.0;
            push_bezier(&mut d, x0, my, p.x, my, p.x, p.y);
        }
        x0 = p.x;
        y0 = p.y;
    }
    d
}

/// The `d` of d3-shape's `curveBumpX` through `points`: every segment, including the first, is
/// its own S-shaped cubic Bezier whose two control points sit at the segment's horizontal
/// midpoint — unlike every other curve in this module, a two-point `curveBumpX` is **already a
/// curve**, not a straight line (there is no `lineTo` fallback in d3-shape's own `Bump.point`).
///
/// Ported from `d3-shape`'s `curve/bump.js`.
pub fn curve_bump_x_path(points: &[Point]) -> String {
    curve_bump_path(points, true)
}

/// [`curve_bump_x_path`], with the S-curve's midpoint taken on the vertical axis instead — d3-
/// shape's `curveBumpY`.
pub fn curve_bump_y_path(points: &[Point]) -> String {
    curve_bump_path(points, false)
}

/// The `d` of mermaid's own `rounded` curve through `points`: every corner cut to a quadratic
/// Bezier of radius `radius`, every straight run between corners left exactly straight — mermaid's
/// answer to the same "give me a right-angle line" request `linear`/`step*` answer with a sharp
/// corner instead (module docs, and mermaid-js#2817/#2549).
///
/// Ported from `edges.js`'s `generateRoundedPath` (mermaid's own function, not d3-shape's — no
/// curve of that name exists upstream). Two differences from upstream, both because konoma's
/// pipeline already handles what the two are for:
///
/// * upstream's caller skips `fixCorners` for `rounded` and instead calls
///   `applyMarkerOffsetsToPoints` before this — a *different* corner-relevant adjustment that
///   shortens a line's two end segments so mermaid's SVG `<marker>` arrow heads do not draw over
///   it. konoma draws no SVG markers: an arrow head is explicit triangle geometry built from the
///   clipped polyline's own last segment (module docs, point 4), already landing exactly on the
///   boundary [`shapes::intersect`] computed — so there is nothing here for a marker offset to
///   correct for, and reproducing it would visibly shorten a line konoma's own arrow-head math
///   depends on ending where it does.
/// * this crate's own `radius` parameter, rather than upstream's hardcoded `5` — [`CORNER_RADIUS`]
///   is the same constant [`fix_corners`] already uses for every other curve, kept as one named
///   value rather than two copies of the literal `5.0`.
pub fn rounded_path(points: &[Point], radius: f64) -> String {
    if points.len() < 2 {
        return degenerate_path(points).unwrap_or_default();
    }
    // mermaid's own epsilon for this function (`generateRoundedPath`'s `const epsilon = 1e-5`) —
    // deliberately not [`EPS`] or [`D3_EPSILON`], which belong to different algorithms upstream
    // chose different tolerances for.
    const EPSILON: f64 = 1e-5;
    let mut d = String::new();
    let last = points.len() - 1;
    for i in 0..points.len() {
        let curr = &points[i];
        if i == 0 {
            push_move(&mut d, curr.x, curr.y);
            continue;
        }
        if i == last {
            push_line(&mut d, curr.x, curr.y);
            continue;
        }
        let prev = &points[i - 1];
        let next = &points[i + 1];
        let (dx1, dy1) = (curr.x - prev.x, curr.y - prev.y);
        let (dx2, dy2) = (next.x - curr.x, next.y - curr.y);
        let len1 = dx1.hypot(dy1);
        let len2 = dx2.hypot(dy2);
        if len1 < EPSILON || len2 < EPSILON {
            push_line(&mut d, curr.x, curr.y);
            continue;
        }
        let (nx1, ny1) = (dx1 / len1, dy1 / len1);
        let (nx2, ny2) = (dx2 / len2, dy2 / len2);
        let dot = (nx1 * nx2 + ny1 * ny2).clamp(-1.0, 1.0);
        let angle = dot.acos();
        if angle < EPSILON || (std::f64::consts::PI - angle).abs() < EPSILON {
            push_line(&mut d, curr.x, curr.y);
            continue;
        }
        let cut_len = (radius / (angle / 2.0).sin())
            .min(len1 / 2.0)
            .min(len2 / 2.0);
        let (start_x, start_y) = (curr.x - nx1 * cut_len, curr.y - ny1 * cut_len);
        let (end_x, end_y) = (curr.x + nx2 * cut_len, curr.y + ny2 * cut_len);
        push_line(&mut d, start_x, start_y);
        push_quad(&mut d, curr.x, curr.y, end_x, end_y);
    }
    d
}

/// Which curve draws an edge's line — mermaid's `flowchart.curve` / `linkStyle ... interpolate` /
/// `%%{init: {"flowchart": {"curve": ...}}}%%`.
///
/// mermaid's most-requested flowchart feature is a straight/right-angle line rather than the
/// default spline (mermaid-js#2817, 129 reactions; mermaid-js#2549, 75 reactions; both open 4+
/// years), which `Linear`/`Step`/`StepBefore`/`StepAfter` answer directly. The rest of d3-shape's
/// curve family — `Natural`, `Cardinal`, `CatmullRom`, `MonotoneX`, `MonotoneY`, `BumpX`, `BumpY`
/// — and mermaid's own `Rounded` are implemented for the same reason `Basis` is: whatever a
/// document actually says, `%%{init}%%` and `linkStyle interpolate` included, is what has to
/// reach the page (module docs' "what a line has to touch its data" — that principle, and
/// GitHub's own picture, apply just as much to a route as to a data path).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Curve {
    /// d3's `curveBasis` — mermaid's own default. A uniform cubic B-spline: touches the first and
    /// last point and none of the others. See [`curve_basis_path`].
    #[default]
    Basis,
    /// d3's `curveLinear` — straight segments through every waypoint, corners un-rounded.
    Linear,
    /// d3's `curveStep` — a right angle at the midpoint of each segment.
    Step,
    /// d3's `curveStepBefore` — vertical first, then horizontal.
    StepBefore,
    /// d3's `curveStepAfter` — horizontal first, then vertical.
    StepAfter,
    /// d3's `curveNatural`. See [`curve_natural_path`].
    Natural,
    /// d3's `curveCardinal` (tension `0`). See [`curve_cardinal_path`].
    Cardinal,
    /// d3's `curveCatmullRom` (`alpha = 0.5`). See [`curve_catmull_rom_path`].
    CatmullRom,
    /// d3's `curveMonotoneX`. See [`curve_monotone_x_path`].
    MonotoneX,
    /// d3's `curveMonotoneY`. See [`curve_monotone_y_path`].
    MonotoneY,
    /// d3's `curveBumpX`. See [`curve_bump_x_path`].
    BumpX,
    /// d3's `curveBumpY`. See [`curve_bump_y_path`].
    BumpY,
    /// mermaid's own `rounded` — not a d3-shape curve. See [`rounded_path`].
    Rounded,
}

impl Curve {
    /// Parses `[ui] mermaid_curve` / a `linkStyle ... interpolate <curve>` /
    /// `%%{init}%%`'s `flowchart.curve` argument. Case-sensitive, matching mermaid's own
    /// spellings (`stepBefore`, not `stepbefore`; `catmullRom`, not `catmullrom`); anything not in
    /// the table becomes [`Curve::Basis`] — this crate's convention for a config value nothing
    /// understands (an unknown value never blocks a render).
    pub fn parse(s: &str) -> Curve {
        match s {
            "linear" => Curve::Linear,
            "step" => Curve::Step,
            "stepBefore" => Curve::StepBefore,
            "stepAfter" => Curve::StepAfter,
            "natural" => Curve::Natural,
            "cardinal" => Curve::Cardinal,
            "catmullRom" => Curve::CatmullRom,
            "monotoneX" => Curve::MonotoneX,
            "monotoneY" => Curve::MonotoneY,
            "bumpX" => Curve::BumpX,
            "bumpY" => Curve::BumpY,
            "rounded" => Curve::Rounded,
            _ => Curve::Basis,
        }
    }

    /// Whether right-angle corners should be rounded off ([`fix_corners`]) before this curve
    /// draws them.
    ///
    /// **Every curve but `Rounded` wants that — including `Linear` and the three `Step*`
    /// curves**, matching upstream exactly: `mermaid@11.4.1` and `mermaid@11.12.0` (both released
    /// versions; checked directly in `edges.js`, `insertEdge`) call `fixCorners` unconditionally,
    /// with no curve-type check at all. The condition only exists on the `develop` branch, added
    /// the same commit that introduced `rounded` — `if (edgeCurveType !== 'rounded') { lineData =
    /// fixCorners(lineData); }` — purely so `rounded`'s own corner-rounding
    /// ([`rounded_path`]) does not get run twice. So `Rounded` is the *only* exception here, for
    /// exactly that reason: it rounds every corner itself, at its own radius, and running
    /// `fix_corners` first would round the same corner twice.
    ///
    /// This crate got this wrong once: an earlier version of this method exempted
    /// `Linear`/`Step`/`StepBefore`/`StepAfter` too, on the reasoning that they are chosen
    /// *because* they keep an angle sharp and rounding one off would undo the shape asked for —
    /// a plausible-sounding rule invented from the *look*, never checked against upstream. It was
    /// wrong: a real `linear`/`step` flowchart on GitHub has its right angles softened by
    /// `fixCorners` too (a 5px radius — subtle, but present, on every release checked). Kept here
    /// as a durable note, not as a "some day" TODO: this crate's whole acceptance test is drawing
    /// what GitHub actually draws, so a plausible-looking rule that was never checked against
    /// upstream is exactly the failure mode to distrust on sight next time one is proposed.
    pub fn rounds_corners(self) -> bool {
        !matches!(self, Curve::Rounded)
    }

    /// The `d` path through `points`, in this curve.
    pub fn path(self, points: &[Point]) -> String {
        match self {
            Curve::Basis => curve_basis_path(points),
            Curve::Linear => polyline_path(points),
            Curve::Step => step_path(points, 0.5),
            Curve::StepBefore => step_path(points, 0.0),
            Curve::StepAfter => step_path(points, 1.0),
            Curve::Natural => curve_natural_path(points),
            Curve::Cardinal => curve_cardinal_path(points),
            Curve::CatmullRom => curve_catmull_rom_path(points),
            Curve::MonotoneX => curve_monotone_x_path(points),
            Curve::MonotoneY => curve_monotone_y_path(points),
            Curve::BumpX => curve_bump_x_path(points),
            Curve::BumpY => curve_bump_y_path(points),
            Curve::Rounded => rounded_path(points, CORNER_RADIUS),
        }
    }
}

/// The `d` of d3-shape's `Step` curve family: a strictly axis-aligned path whose right-angle
/// transition, within each segment `p0 -> p1`, sits `t` of the way from `p0` to `p1` —
/// `t = 0` draws the vertical leg first (`curveStepBefore`), `t = 1` draws the horizontal leg
/// first (`curveStepAfter`), and `t = 0.5` splits the two at the midpoint (`curveStep`).
///
/// **Every segment lands exactly on `p1`.** d3-shape's own `Step.point` does not, for `0 < t <
/// 1`: it draws to `(mid, p1.y)` and leaves the last leg — `(mid, p1.y) -> (p1.x, p1.y)` — for
/// its `lineEnd` to patch on, *once, over the whole line* rather than once per segment, so an
/// interior waypoint of a 3+-point `curveStep` route is not actually touched. An edge's waypoints
/// are dagre's own routing, not decoration, so leaving one of them un-met is the wrong trade here
/// (the tail end of the arrow head is built from this polyline's own last segment — see the
/// module docs' step 4 — and it has to land on the boundary the clipping step put there). This
/// closes that gap by drawing the leftover leg inline, immediately, for every segment: for `t = 0`
/// or `t = 1` the leftover leg has zero length and is skipped, so those two curves are unaffected
/// and match d3 exactly regardless of point count; only `curveStep` (`t = 0.5`) draws a shape
/// distinct from upstream's, and only when a route has three or more points — on the two-point
/// case (an ordinary flowchart edge) it is identical, `lineEnd`'s patch included.
pub fn step_path(points: &[Point], t: f64) -> String {
    let mut d = String::new();
    let Some(first) = points.first() else {
        return d;
    };
    d.push_str(&format!("M{},{}", num(first.x), num(first.y)));
    for w in points.windows(2) {
        let (p0, p1) = (&w[0], &w[1]);
        let mid_x = p0.x * (1.0 - t) + p1.x * t;
        if (mid_x - p0.x).abs() > EPS {
            d.push_str(&format!("L{},{}", num(mid_x), num(p0.y)));
        }
        d.push_str(&format!("L{},{}", num(mid_x), num(p1.y)));
        if (mid_x - p1.x).abs() > EPS {
            d.push_str(&format!("L{},{}", num(p1.x), num(p1.y)));
        }
    }
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
