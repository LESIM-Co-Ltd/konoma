//! Stage 1 (skeleton) of `[ui] mermaid_routing = "konoma-orthogonal"` — konoma's own right-angle wiring
//! mode for a flowchart's edges.
//!
//! `docs/FEATURE-MERMAID-RENDERER.md` §10-1 is the confirmed spec (a Claude Design handoff); this
//! module implements the part of it stage 1 owns: **the route, not the layout**. dagre's ranking,
//! ordering and coordinates are untouched — this module only decides, for a node-to-node edge,
//! which face of each box a line leaves and enters through and how many right-angle bends connect
//! them. Everything §10-1 calls the "退避則" (port-eviction / node-growth when several edges share
//! a face), "レーン揃え" (straight-through lanes shared across ranks), "ラベル区間長" (minimum span
//! for a labelled segment) and "外周レーン" (a perimeter lane for back-edges) is later work; this
//! stage's back-edge route is explicitly the "暫定" (stopgap) §10-2 describes: dagre's own
//! waypoints, bent onto right angles, rather than a real perimeter route.
//!
//! # The four shapes of route
//!
//! Every node-to-node edge that reaches [`route_edge`] falls into exactly one of these, decided in
//! this order:
//!
//! 1. **reverse** — the target's rank is at or before the source's. dagre still produced a
//!    waypoint chain for it (walking through whatever dummy nodes the intervening ranks needed),
//!    so [`route_reverse`] keeps that chain and only straightens each leg onto two right-angle
//!    segments — see its own doc for why this, and not the single-bend shapes below, is the safe
//!    choice for an edge that may have to clear nodes sitting between the two ranks.
//! 2. **aligned** — same coordinate on the axis across the flow (within half a pixel). A dead
//!    straight line, flow-centre to flow-centre, no bend at all ([`route_aligned`]).
//! 3. **branch** — the source has more than one outgoing edge (or neither 3 nor this applies, the
//!    default): leaves the source through the face across the flow (the face nearest the target's
//!    side) and turns once into the target's upstream face.
//! 4. **merge** — not a branch, and the target has more than one incoming edge: leaves the source
//!    through its downstream face and turns once into the target's face across the flow.
//!
//! Branch and merge share one shape (a single right-angle turn between two node faces) and one
//! builder, [`route_single_bend`]; they differ only in *which* face each end uses.
//!
//! # Two axes, four directions, one set of formulas
//!
//! A flowchart's `direction` picks which physical axis (x or y) is "along the flow" and which is
//! "across it", and — for the along-the-flow axis — which physical direction counts as
//! downstream. [`flow`]/[`cross`]/[`make`] are the one place that knowledge lives; everything else
//! in this module reasons in "flow" and "cross" and calls those three functions to convert, which
//! is what lets a single set of formulas below draw a correct picture in all four directions
//! (`TD`/`BT`/`LR`/`RL`) rather than needing one branch per direction at every call site.

use super::PlacedNode;
use crate::preview::mermaid::flowchart::Direction;
use crate::preview::mermaid::layout::Point;

/// `[ui] mermaid_routing`'s raw string, resolved.
///
/// The mode itself is named after Graphviz's `splines=ortho` / ELK's `edgeRouting: ORTHOGONAL` —
/// the value `"konoma-orthogonal"` carries a `konoma-` prefix precisely because it is *not*
/// upstream mermaid vocabulary the way `[ui] mermaid_curve`'s values are: it is konoma's own
/// wiring mode, and the prefix keeps it from colliding if upstream mermaid ever defines its own
/// meaning for the bare word `"orthogonal"` (`docs/FEATURE-MERMAID-RENDERER.md` §10-2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Routing {
    /// The curve every diagram has always drawn — `Curve::path` smooths dagre's own waypoints.
    /// `[ui] mermaid_curve` only ever means anything under this mode.
    #[default]
    Splines,
    /// Konoma's own right-angle wiring: see the module docs.
    Orthogonal,
}

impl Routing {
    /// Parses `[ui] mermaid_routing`. Permissive like every config value in this crate: only the
    /// exact spelling `"konoma-orthogonal"` turns it on, and anything else — including a typo — is
    /// `Splines`, the byte-stable default. A render never fails over an unrecognised routing.
    pub fn parse(s: &str) -> Routing {
        match s {
            "konoma-orthogonal" => Routing::Orthogonal,
            _ => Routing::Splines,
        }
    }
}

/// How far a routed line's end sits **outside** the node's geometric boundary —
/// `docs/FEATURE-MERMAID-RENDERER.md` §10-1 item 1's last bullet: "矢尻…の先端はノード枠の外縁から
/// 1px離す＝端点は枠座標の2px手前".
///
/// A node's outline is a 1.5px (`svg::NODE_STROKE_WIDTH`) stroke centred on the geometric
/// boundary [`PlacedNode::bounds`] returns, so the *visible* outer edge of the ink sits
/// `NODE_STROKE_WIDTH / 2.0` beyond that boundary; the tip is asked to clear that by another 1px.
/// `[face_port]`/[`port_at`] move the endpoint *away* from the node by this much — **not** into
/// it. Getting the sign of that backwards is exactly the bug this constant's own doc once
/// described and the code once did the opposite of: an endpoint pulled *inward* draws a tip that
/// pokes past the boundary into the node's own interior, where svg::emit's node pass — nodes are
/// drawn **after** edges (`emit`'s own doc: "edges" group before "nodes") — paints over it, so the
/// visible tip looks flush with the boundary (zero gap) instead of clear of it. Measured against a
/// real SVG dump, not reasoned about: `A --> B` in a `TD` diagram had the routed endpoint at
/// `B`'s top `y + PORT_INSET` (inside `B`) before this was fixed.
pub const PORT_INSET: f64 = super::svg::NODE_STROKE_WIDTH / 2.0 + 1.0;

/// Which flat face of a node's bounding box a line leaves or enters through.
///
/// Always a *physical* direction (`Top` is always the lesser-y side), independent of
/// `direction` — the flow/cross split lives in [`flow_face`]/[`cross_face`], not here, which is
/// what lets [`face_port`]/[`port_at`] stay direction-agnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Side {
    Top,
    Bottom,
    Left,
    Right,
}

impl Side {
    fn opposite(self) -> Side {
        match self {
            Side::Top => Side::Bottom,
            Side::Bottom => Side::Top,
            Side::Left => Side::Right,
            Side::Right => Side::Left,
        }
    }
}

/// Which of the two abstract axes a segment moves along — see the module docs' "two axes" section.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Axis {
    Flow,
    Cross,
}

/// `p`'s coordinate along the flow axis: y for `TD`/`BT`, x for `LR`/`RL`.
fn flow(direction: Direction, p: &Point) -> f64 {
    match direction {
        Direction::TopToBottom | Direction::BottomToTop => p.y,
        Direction::LeftToRight | Direction::RightToLeft => p.x,
    }
}

/// `p`'s coordinate along the axis across the flow: x for `TD`/`BT`, y for `LR`/`RL`.
fn cross(direction: Direction, p: &Point) -> f64 {
    match direction {
        Direction::TopToBottom | Direction::BottomToTop => p.x,
        Direction::LeftToRight | Direction::RightToLeft => p.y,
    }
}

/// The point at flow coordinate `flow_v`, cross coordinate `cross_v` — [`flow`]/[`cross`]'s
/// inverse.
fn make(direction: Direction, flow_v: f64, cross_v: f64) -> Point {
    match direction {
        Direction::TopToBottom | Direction::BottomToTop => Point::new(cross_v, flow_v),
        Direction::LeftToRight | Direction::RightToLeft => Point::new(flow_v, cross_v),
    }
}

/// The face whose outward normal runs along the flow axis, on the side `delta` (a flow-coordinate
/// difference, target minus source or similar) points to.
///
/// `TD`/`BT` both flow along y, so this is always `Top`/`Bottom` for them regardless of which end
/// is visually "up" — `BT`'s upstream/downstream sense is undone by
/// [`super::super::layout`]'s own coordinate transform before this module ever sees a coordinate,
/// so `delta >= 0.0` always means "further along the axis dagre laid the rank out on", the same
/// answer [`route_edge`]'s rank comparison already reasons in.
fn flow_face(direction: Direction, delta: f64) -> Side {
    match direction {
        Direction::TopToBottom | Direction::BottomToTop => {
            if delta >= 0.0 {
                Side::Bottom
            } else {
                Side::Top
            }
        }
        Direction::LeftToRight | Direction::RightToLeft => {
            if delta >= 0.0 {
                Side::Right
            } else {
                Side::Left
            }
        }
    }
}

/// The face whose outward normal runs along the axis across the flow, on the side `delta` points
/// to. The pair [`flow_face`] does not use: `Left`/`Right` for `TD`/`BT`, `Top`/`Bottom` for
/// `LR`/`RL`.
fn cross_face(direction: Direction, delta: f64) -> Side {
    match direction {
        Direction::TopToBottom | Direction::BottomToTop => {
            if delta >= 0.0 {
                Side::Right
            } else {
                Side::Left
            }
        }
        Direction::LeftToRight | Direction::RightToLeft => {
            if delta >= 0.0 {
                Side::Bottom
            } else {
                Side::Top
            }
        }
    }
}

/// Which axis a segment leaving/entering `side` has to move along to meet it perpendicularly.
fn axis_of(direction: Direction, side: Side) -> Axis {
    let top_bottom_is_flow = matches!(direction, Direction::TopToBottom | Direction::BottomToTop);
    match (top_bottom_is_flow, side) {
        (true, Side::Top | Side::Bottom) => Axis::Flow,
        (true, Side::Left | Side::Right) => Axis::Cross,
        (false, Side::Left | Side::Right) => Axis::Flow,
        (false, Side::Top | Side::Bottom) => Axis::Cross,
    }
}

/// The face of `reference` (relative to `center`) whose axis has the larger absolute difference —
/// [`route_reverse`]'s way of picking a perpendicular exit/entry face when there is no `out`/`in`
/// degree to decide branch vs merge from, only "which way does the route actually need to go".
fn dominant_face(direction: Direction, center: &Point, reference: &Point) -> Side {
    let dflow = flow(direction, reference) - flow(direction, center);
    let dcross = cross(direction, reference) - cross(direction, center);
    if dflow.abs() >= dcross.abs() {
        flow_face(direction, dflow)
    } else {
        cross_face(direction, dcross)
    }
}

/// The centre of `node`'s `side` face, pulled [`PORT_INSET`] px **outside** the node — clear of
/// the boundary, not into it; see that constant's own doc for the bug this direction fixes.
fn face_port(node: &PlacedNode, side: Side, inset: f64) -> Point {
    let coord = match side {
        Side::Top | Side::Bottom => node.center.x,
        Side::Left | Side::Right => node.center.y,
    };
    port_at(node, side, coord, inset)
}

/// [`face_port`], at an explicit position along the face rather than its centre — what
/// [`route_aligned`] uses to put both ends of a straight edge on the same line.
///
/// `inset` moves *away* from the node: `Top` gets a **smaller** y (further up, outside), `Bottom`
/// a **larger** y, `Left` a smaller x, `Right` a larger x. [`PlacedNode::bounds`]'s `(l, t, r, b)`
/// is the geometric boundary the node's stroke is centred on, not its visible outer edge — see
/// [`PORT_INSET`]'s own doc for why the offset has to clear the stroke's own half-width too.
fn port_at(node: &PlacedNode, side: Side, coord: f64, inset: f64) -> Point {
    let (l, t, r, b) = node.bounds();
    match side {
        Side::Top => Point::new(coord, t - inset),
        Side::Bottom => Point::new(coord, b + inset),
        Side::Left => Point::new(l - inset, coord),
        Side::Right => Point::new(r + inset, coord),
    }
}

/// Coordinates within this much of each other on an axis are the same coordinate — dagre's own
/// waypoints can differ by less than a pixel from floating-point layout arithmetic that a reader
/// would never notice, and treating that as "still needs a bend" would draw a visible dogleg
/// nobody asked for.
const EPS: f64 = 1e-6;

/// The axis-parallel points that connect `a` to `b` — **not including `a`** — leaving `a` along
/// `leave` and arriving at `b` along `enter`.
///
/// Four shapes, one for each pair of axes:
///
/// * **unlike axes** (`Flow, Cross` or `Cross, Flow`): exactly one corner does it — hold the axis
///   `a` leaves on fixed at `a`'s own coordinate and the axis `b` arrives on fixed at `b`'s, and
///   the corner where those two fixed values meet is reachable from both ends in a single
///   right-angle segment each. This is [`route_single_bend`]'s shape (branch/merge) and also
///   covers one leg of [`route_reverse`]'s staircase.
/// * **same axis, other coordinate already equal**: no corner needed at all — `a` to `b` is
///   already a straight run on that axis.
/// * **same axis, other coordinate differs**: one corner is not enough (it would leave *that*
///   axis's coordinate wrong at one end), so this takes two — out along the axis to a point
///   half-way between `a` and `b`, across on the other axis, then the rest of the way along the
///   first axis into `b`. Reachable in practice today only from [`route_reverse`]'s interior legs,
///   where consecutive dummy waypoints are not axis-aligned with each other.
fn bridge(direction: Direction, a: &Point, b: &Point, leave: Axis, enter: Axis) -> Vec<Point> {
    match (leave, enter) {
        (Axis::Flow, Axis::Cross) => {
            let corner = make(direction, flow(direction, b), cross(direction, a));
            vec![corner, b.clone()]
        }
        (Axis::Cross, Axis::Flow) => {
            let corner = make(direction, flow(direction, a), cross(direction, b));
            vec![corner, b.clone()]
        }
        (Axis::Flow, Axis::Flow) => {
            if (cross(direction, a) - cross(direction, b)).abs() < EPS {
                vec![b.clone()]
            } else {
                let mid = (flow(direction, a) + flow(direction, b)) / 2.0;
                vec![
                    make(direction, mid, cross(direction, a)),
                    make(direction, mid, cross(direction, b)),
                    b.clone(),
                ]
            }
        }
        (Axis::Cross, Axis::Cross) => {
            if (flow(direction, a) - flow(direction, b)).abs() < EPS {
                vec![b.clone()]
            } else {
                let mid = (cross(direction, a) + cross(direction, b)) / 2.0;
                vec![
                    make(direction, flow(direction, a), mid),
                    make(direction, flow(direction, b), mid),
                    b.clone(),
                ]
            }
        }
    }
}

/// Routes one node-to-node flowchart edge under `[ui] mermaid_routing = "konoma-orthogonal"`.
///
/// `raw` is dagre's own waypoint chain for this edge (before konoma's ordinary spline clipping
/// touches it) — only [`route_reverse`] reads it; every other shape ignores dagre's route entirely
/// and works out its own from the two nodes' geometry, because a same-direction edge with no
/// intervening dummy chain of its own (or one dagre placed purely to reserve room for a label) has
/// nothing in `raw` worth keeping.
///
/// `source_rank`/`target_rank` are read straight from dagre's `NodeLabel::rank`; either being
/// absent (should not happen for a real node once layout has run) is treated as "not a back-edge"
/// rather than guessed at, since [`route_aligned`]/[`route_single_bend`] degrade harmlessly for an
/// edge that happens to run backward while [`route_reverse`]'s waypoint-borrowing does not have
/// the rank comparison it is built around.
///
/// The caller is `lay_out_spec`, and only when both ends are [`super::edges::End::Node`] — a
/// cluster boundary is out of scope for stage 1 (§10-2's later stages own it) and keeps drawing
/// through the ordinary spline path.
#[allow(clippy::too_many_arguments)]
pub fn route_edge(
    direction: Direction,
    source: &PlacedNode,
    target: &PlacedNode,
    raw: &[Point],
    source_rank: Option<i32>,
    target_rank: Option<i32>,
    source_out_degree: usize,
    target_in_degree: usize,
) -> Vec<Point> {
    let is_reverse = matches!((source_rank, target_rank), (Some(sr), Some(tr)) if tr <= sr);

    let mut points = if is_reverse {
        route_reverse(direction, source, target, raw)
    } else {
        let dcross = cross(direction, &target.center) - cross(direction, &source.center);
        if dcross.abs() < 0.5 {
            route_aligned(direction, source, target)
        } else {
            // "分岐形（source の out-degree > 1、または下記どちらでもない既定）" / "合流形（分岐形で
            // なく target の in-degree > 1）" — branch is the default; merge is the one exception,
            // taken only when the source is not itself branching and the target actually merges.
            let branching = source_out_degree > 1 || target_in_degree <= 1;
            if branching {
                let source_side = cross_face(
                    direction,
                    cross(direction, &target.center) - cross(direction, &source.center),
                );
                let target_side = flow_face(
                    direction,
                    flow(direction, &source.center) - flow(direction, &target.center),
                );
                route_single_bend(direction, source, target, source_side, target_side)
            } else {
                let source_side = flow_face(
                    direction,
                    flow(direction, &target.center) - flow(direction, &source.center),
                );
                let target_side = cross_face(
                    direction,
                    cross(direction, &source.center) - cross(direction, &target.center),
                );
                route_single_bend(direction, source, target, source_side, target_side)
            }
        }
    };

    super::edges::dedupe(&mut points);
    if points.len() < 2 {
        // Defensive only — two distinct nodes with any real size cannot collapse their two ports
        // onto the same pixel. Never draw nothing rather than a degenerate one-point "line".
        points = vec![source.center.clone(), target.center.clone()];
    }
    points
}

/// The straight-line shape: both ports on the same cross-axis coordinate (their two centres'
/// average, so a barely-misaligned pair still draws one exact right angle rather than a hairline
/// diagonal), zero bends.
fn route_aligned(direction: Direction, source: &PlacedNode, target: &PlacedNode) -> Vec<Point> {
    let delta = flow(direction, &target.center) - flow(direction, &source.center);
    let source_side = flow_face(direction, delta);
    let target_side = source_side.opposite();
    let shared_cross = (cross(direction, &source.center) + cross(direction, &target.center)) / 2.0;
    vec![
        port_at(source, source_side, shared_cross, PORT_INSET),
        port_at(target, target_side, shared_cross, PORT_INSET),
    ]
}

/// The one-bend shape branch and merge share: a port on each of the two given faces, joined by
/// whatever [`bridge`] draws between faces of unlike axes — always a single corner for this pair,
/// because `source_side`/`target_side` are always chosen (by [`route_edge`]) from different axes.
fn route_single_bend(
    direction: Direction,
    source: &PlacedNode,
    target: &PlacedNode,
    source_side: Side,
    target_side: Side,
) -> Vec<Point> {
    let source_port = face_port(source, source_side, PORT_INSET);
    let target_port = face_port(target, target_side, PORT_INSET);
    let mut out = vec![source_port.clone()];
    out.extend(bridge(
        direction,
        &source_port,
        &target_port,
        axis_of(direction, source_side),
        axis_of(direction, target_side),
    ));
    out
}

/// The back-edge shape: keep dagre's own waypoint chain (it already threads any nodes sitting
/// between the two ranks — §10-2's "暫定" note is exactly that this reuse, not a real perimeter
/// route, is what makes that safe for stage 1) and straighten every leg onto right angles, with a
/// perpendicular port at each end in place of dagre's bounding-box crossing.
///
/// The exit/entry face at each end is picked by [`dominant_face`] against the nearest waypoint
/// dagre actually routed through — there is no out/in-degree question to answer here, only "which
/// way does this particular route leave", so branch/merge's face rules do not apply.
fn route_reverse(
    direction: Direction,
    source: &PlacedNode,
    target: &PlacedNode,
    raw: &[Point],
) -> Vec<Point> {
    let mut deduped = raw.to_vec();
    super::edges::dedupe(&mut deduped);
    let interior: Vec<Point> = if deduped.len() > 2 {
        deduped[1..deduped.len() - 1].to_vec()
    } else {
        Vec::new()
    };

    let ref_start = interior
        .first()
        .cloned()
        .unwrap_or_else(|| target.center.clone());
    let ref_end = interior
        .last()
        .cloned()
        .unwrap_or_else(|| source.center.clone());
    let exit_side = dominant_face(direction, &source.center, &ref_start);
    let entry_side = dominant_face(direction, &target.center, &ref_end);

    let source_port = face_port(source, exit_side, PORT_INSET);
    let target_port = face_port(target, entry_side, PORT_INSET);

    let mut out = vec![source_port.clone()];
    let mut prev = source_port;
    let mut prev_axis = axis_of(direction, exit_side);
    for w in &interior {
        out.extend(bridge(direction, &prev, w, prev_axis, Axis::Flow));
        prev = w.clone();
        prev_axis = Axis::Flow;
    }
    out.extend(bridge(
        direction,
        &prev,
        &target_port,
        prev_axis,
        axis_of(direction, entry_side),
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::preview::mermaid::render::shapes::{Glyph, Size};
    use crate::preview::mermaid::render::Label;

    fn node(id: &str, cx: f64, cy: f64, w: f64, h: f64) -> PlacedNode {
        PlacedNode {
            id: id.to_string(),
            shape: Glyph::default(),
            center: Point::new(cx, cy),
            size: Size::new(w, h),
            label: Label::measure(""),
            panel: None,
            series: None,
            mark: None,
            style: None,
        }
    }

    #[test]
    fn routing_parse_is_permissive() {
        assert_eq!(Routing::parse("konoma-orthogonal"), Routing::Orthogonal);
        assert_eq!(Routing::parse("splines"), Routing::Splines);
        assert_eq!(Routing::parse(""), Routing::Splines);
        assert_eq!(Routing::parse("Orthogonal"), Routing::Splines);
        // The bare word, with no `konoma-` prefix, is deliberately NOT the trigger — it is
        // reserved in case upstream mermaid ever gives it a meaning of its own.
        assert_eq!(Routing::parse("orthogonal"), Routing::Splines);
        assert_eq!(Routing::parse("xyz"), Routing::Splines);
    }

    #[test]
    fn aligned_lr_edge_is_a_straight_two_point_line() {
        let a = node("A", 0.0, 100.0, 80.0, 40.0);
        let b = node("B", 200.0, 100.0, 80.0, 40.0);
        let pts = route_edge(Direction::LeftToRight, &a, &b, &[], Some(0), Some(1), 1, 1);
        assert_eq!(pts.len(), 2);
        assert!((pts[0].y - pts[1].y).abs() < 1e-9, "flat: {pts:?}");
        assert!(pts[0].x > 0.0 && pts[0].x < pts[1].x, "{pts:?}");
    }

    #[test]
    fn branch_lr_edge_bends_once_and_lands_perpendicular() {
        let a = node("A", 0.0, 0.0, 80.0, 40.0);
        let b = node("B", 200.0, 100.0, 80.0, 40.0);
        // out-degree 2 forces the branch shape even though in-degree is 1 too.
        let pts = route_edge(Direction::LeftToRight, &a, &b, &[], Some(0), Some(1), 2, 1);
        assert_eq!(pts.len(), 3, "{pts:?}");
        // Leaves A vertically (cross axis for LR): x constant between pts[0] and pts[1].
        assert!((pts[0].x - pts[1].x).abs() < 1e-9, "{pts:?}");
        // Enters B horizontally (flow axis for LR): y constant between pts[1] and pts[2].
        assert!((pts[1].y - pts[2].y).abs() < 1e-9, "{pts:?}");
    }

    #[test]
    fn merge_lr_edge_bends_once_the_other_way() {
        let a = node("A", 0.0, 0.0, 80.0, 40.0);
        let b = node("B", 200.0, 100.0, 80.0, 40.0);
        // out-degree 1, in-degree 2: not a branch, target merges.
        let pts = route_edge(Direction::LeftToRight, &a, &b, &[], Some(0), Some(1), 1, 2);
        assert_eq!(pts.len(), 3, "{pts:?}");
        // Leaves A horizontally (flow axis): y constant between pts[0] and pts[1].
        assert!((pts[0].y - pts[1].y).abs() < 1e-9, "{pts:?}");
        // Enters B vertically (cross axis): x constant between pts[1] and pts[2].
        assert!((pts[1].x - pts[2].x).abs() < 1e-9, "{pts:?}");
    }

    #[test]
    fn reverse_edge_is_detected_by_rank_and_stays_axis_parallel() {
        let a = node("A", 200.0, 0.0, 80.0, 40.0);
        let b = node("B", 0.0, 0.0, 80.0, 40.0);
        // target rank <= source rank: a back-edge, with one interior dagre waypoint.
        let raw = vec![
            Point::new(160.0, 0.0),
            Point::new(100.0, -60.0),
            Point::new(40.0, 0.0),
        ];
        let pts = route_edge(Direction::LeftToRight, &a, &b, &raw, Some(2), Some(0), 1, 1);
        assert!(pts.len() >= 2);
        for w in pts.windows(2) {
            let dx = (w[1].x - w[0].x).abs();
            let dy = (w[1].y - w[0].y).abs();
            assert!(dx < 1e-9 || dy < 1e-9, "diagonal segment: {w:?}");
        }
    }

    #[test]
    fn all_four_directions_produce_only_axis_parallel_segments() {
        for direction in [
            Direction::TopToBottom,
            Direction::BottomToTop,
            Direction::LeftToRight,
            Direction::RightToLeft,
        ] {
            let a = node("A", 0.0, 0.0, 80.0, 40.0);
            let b = node("B", 150.0, 90.0, 80.0, 40.0);
            for (sr, tr, out_d, in_d) in [(0, 1, 1, 1), (0, 1, 2, 1), (0, 1, 1, 2), (1, 0, 1, 1)] {
                let pts = route_edge(direction, &a, &b, &[], Some(sr), Some(tr), out_d, in_d);
                for w in pts.windows(2) {
                    let dx = (w[1].x - w[0].x).abs();
                    let dy = (w[1].y - w[0].y).abs();
                    assert!(
                        dx < 1e-9 || dy < 1e-9,
                        "{direction:?} sr={sr} tr={tr}: diagonal segment {w:?}"
                    );
                }
            }
        }
    }
}
