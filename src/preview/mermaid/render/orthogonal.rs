//! `[ui] mermaid_routing = "konoma-orthogonal"` — konoma's own right-angle wiring mode for a
//! flowchart's edges.
//!
//! `docs/FEATURE-MERMAID-RENDERER.md` §10-1 is the confirmed spec (a Claude Design handoff). This
//! module implements the part of it that is **route, not layout**: dagre's ranking, ordering and
//! coordinates are untouched, and everything here only decides, for a node-to-node edge, which
//! face of each box a line leaves and enters through, at which exact point on that face, and how
//! many right-angle bends connect the two. §10-1's "レーン揃え" (straight-through lanes shared
//! across ranks), "ラベル区間長" (minimum span for a labelled segment) and "外周レーン" (a
//! perimeter lane for back-edges) are later work; the back-edge route here is still the "暫定"
//! (stopgap) §10-2 describes: dagre's own waypoints, bent onto right angles, rather than a real
//! perimeter route.
//!
//! # Two passes: classify, then place
//!
//! [`classify`] decides, from the two nodes' geometry and ranks alone (no port position yet),
//! which face of each node an edge uses and how many bends its shape wants — the same four shapes
//! stage 1 had (reverse / aligned / branch / merge). That has to happen for *every* edge before
//! *any* edge can be given an exact port, because [`evict`] — §10-1's "退避則" — needs to see every
//! edge that shares a face before it can space them 16px apart, decide which one (if any) keeps
//! the centre, and tell whether the face is even wide enough. [`route_flowchart`] is the one
//! function that runs both passes over a whole diagram; [`route_edge`] is the single-edge shortcut
//! the unit tests below use — the eviction pass's own `n = 1` case, with no sibling to share a
//! port with, so nothing in it disagrees with what `route_flowchart` does for an edge with no
//! competition on either face.
//!
//! # Growing a node changes the layout, so eviction can take more than one pass
//!
//! A face too narrow for its ports has to grow the node — and a node's size is exactly what dagre
//! lays out from, so growing one can move everything. [`super::lay_out_spec`] is what actually
//! retries dagre with a grown size; this module only ever answers, for one already-completed
//! layout, "how many edges are on this face, and how big does the node need to be to fit them" —
//! see [`Eviction::required_size`].
//!
//! # Two axes, four directions, one set of formulas
//!
//! A flowchart's `direction` picks which physical axis (x or y) is "along the flow" and which is
//! "across it", and — for the along-the-flow axis — which physical direction counts as
//! downstream. [`flow`]/[`cross`]/[`make`] are the one place that knowledge lives; everything else
//! in this module reasons in "flow" and "cross" and calls those three functions to convert, which
//! is what lets a single set of formulas below draw a correct picture in all four directions
//! (`TD`/`BT`/`LR`/`RL`) rather than needing one branch per direction at every call site.

use std::collections::HashMap;

use super::{shapes, Glyph, PlacedNode, Size};
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

/// How far apart two adjacent ports on the same face sit — §10-1 item 1: "16px間隔のポートに等分配".
pub const PORT_SPACING: f64 = 16.0;

/// How close the outermost port on a face may sit to that face's own corner (the chamfered corner
/// included — see [`Eviction::required_size`]) — §10-1 item 1: "ポートは角から8px以上".
pub const PORT_CLEARANCE: f64 = 8.0;

/// Which flat face of a node's bounding box a line leaves or enters through.
///
/// Always a *physical* direction (`Top` is always the lesser-y side), independent of
/// `direction` — the flow/cross split lives in [`flow_face`]/[`cross_face`], not here, which is
/// what lets [`face_port`]/[`port_at`] stay direction-agnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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
/// answer [`classify`]'s rank comparison already reasons in.
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
/// [`classify`]'s way of picking a perpendicular exit/entry face for a back edge, when there is no
/// `out`/`in` degree to decide branch vs merge from, only "which way does the route actually need
/// to go".
fn dominant_face(direction: Direction, center: &Point, reference: &Point) -> Side {
    let dflow = flow(direction, reference) - flow(direction, center);
    let dcross = cross(direction, reference) - cross(direction, center);
    if dflow.abs() >= dcross.abs() {
        flow_face(direction, dflow)
    } else {
        cross_face(direction, dcross)
    }
}

/// `node`'s own coordinate along `side`'s tangent axis — the position [`evict`] offsets from, and
/// what [`face_port`] calls when nothing shares the face and the offset is zero.
fn face_center_coord(node: &PlacedNode, side: Side) -> f64 {
    match side {
        Side::Top | Side::Bottom => node.center.x,
        Side::Left | Side::Right => node.center.y,
    }
}

/// The centre of `node`'s `side` face, pulled [`PORT_INSET`] px **outside** the node — clear of
/// the boundary, not into it; see that constant's own doc for the bug this direction fixes.
fn face_port(node: &PlacedNode, side: Side, inset: f64) -> Point {
    port_at(node, side, face_center_coord(node, side), inset)
}

/// [`face_port`], at an explicit position along the face rather than its centre — what an evicted
/// port (offset from the face's own centre) is built from.
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
///   right-angle segment each. This is branch/merge's shape and also covers one leg of a back
///   edge's staircase ([`route_reverse_with_ports`]).
/// * **same axis, other coordinate already equal**: no corner needed at all — `a` to `b` is
///   already a straight run on that axis. An aligned edge takes this path whenever eviction gave
///   both its ends the same coordinate (the common case: nothing else shares either face).
/// * **same axis, other coordinate differs**: one corner is not enough (it would leave *that*
///   axis's coordinate wrong at one end), so this takes two — out along the axis to a point
///   half-way between `a` and `b`, across on the other axis, then the rest of the way along the
///   first axis into `b`. An aligned edge takes this path when eviction gave its two ends
///   different coordinates (§10-1 item 1's "ポート分配で端点がずれた辺は曲げ2回まで許す"); it is
///   also reachable from a back edge's interior legs, where consecutive dummy waypoints are not
///   axis-aligned with each other.
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

/// One edge's route shape, decided from the two nodes' geometry and ranks alone — before any
/// port position exists. [`evict`] needs every edge's shape gathered first (it has to see a whole
/// face at once); [`route_with_ports`] needs one edge's shape plus the two ports [`evict`] (or,
/// for a single edge in isolation, [`route_edge`]) decided for it.
#[derive(Debug, Clone, Copy)]
struct EdgeShape {
    /// The back-edge shape: keep dagre's own waypoint chain rather than the single/double-bend
    /// shapes below — see [`classify`]'s own doc.
    reverse: bool,
    /// Whether this is the dead-straight, zero-bend shape (§10-1 item 1: "直進辺") — the one rule
    /// 2 gives the centre port to over any sibling on the same face.
    aligned: bool,
    /// Which face of the source the line leaves through, and which axis leaving perpendicular to
    /// it means moving along.
    source_side: Side,
    source_axis: Axis,
    /// Which face of the target the line enters through, and which axis entering perpendicular to
    /// it means moving along.
    target_side: Side,
    target_axis: Axis,
}

/// Decides `source`→`target`'s route shape — the same four-way decision stage 1's `route_edge`
/// made inline, factored out so [`evict`] can see it for every edge before any of them gets a
/// port. Four shapes, decided in this order:
///
/// 1. **reverse** — the target's rank is at or before the source's. dagre still produced a
///    waypoint chain for it (walking through whatever dummy nodes the intervening ranks needed),
///    so the exit/entry face is picked by [`dominant_face`] against the nearest waypoint dagre
///    actually routed through, and the interior chain is what [`route_reverse_with_ports`]
///    straightens onto right angles.
/// 2. **aligned** — same coordinate on the axis across the flow (within half a pixel). Both faces
///    are on the flow axis, facing each other.
/// 3. **branch** — the source has more than one outgoing edge (or neither this nor 4 applies, the
///    default): leaves the source through the face across the flow and turns once into the
///    target's upstream face.
/// 4. **merge** — not a branch, and the target has more than one incoming edge: leaves the source
///    through its downstream face and turns once into the target's face across the flow.
#[allow(clippy::too_many_arguments)]
fn classify(
    direction: Direction,
    source: &PlacedNode,
    target: &PlacedNode,
    raw: &[Point],
    source_rank: Option<i32>,
    target_rank: Option<i32>,
    source_out_degree: usize,
    target_in_degree: usize,
) -> EdgeShape {
    let is_reverse = matches!((source_rank, target_rank), (Some(sr), Some(tr)) if tr <= sr);
    if is_reverse {
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
        let source_side = dominant_face(direction, &source.center, &ref_start);
        let target_side = dominant_face(direction, &target.center, &ref_end);
        return EdgeShape {
            reverse: true,
            aligned: false,
            source_side,
            source_axis: axis_of(direction, source_side),
            target_side,
            target_axis: axis_of(direction, target_side),
        };
    }

    let dcross = cross(direction, &target.center) - cross(direction, &source.center);
    if dcross.abs() < 0.5 {
        let delta = flow(direction, &target.center) - flow(direction, &source.center);
        let source_side = flow_face(direction, delta);
        let target_side = source_side.opposite();
        return EdgeShape {
            reverse: false,
            aligned: true,
            source_side,
            source_axis: Axis::Flow,
            target_side,
            target_axis: Axis::Flow,
        };
    }

    // "分岐形（source の out-degree > 1、または下記どちらでもない既定）" / "合流形（分岐形でなく
    // target の in-degree > 1）" — branch is the default; merge is the one exception, taken only
    // when the source is not itself branching and the target actually merges.
    let branching = source_out_degree > 1 || target_in_degree <= 1;
    let (source_side, target_side) = if branching {
        (
            cross_face(
                direction,
                cross(direction, &target.center) - cross(direction, &source.center),
            ),
            flow_face(
                direction,
                flow(direction, &source.center) - flow(direction, &target.center),
            ),
        )
    } else {
        (
            flow_face(
                direction,
                flow(direction, &target.center) - flow(direction, &source.center),
            ),
            cross_face(
                direction,
                cross(direction, &source.center) - cross(direction, &target.center),
            ),
        )
    };
    EdgeShape {
        reverse: false,
        aligned: false,
        source_side,
        source_axis: axis_of(direction, source_side),
        target_side,
        target_axis: axis_of(direction, target_side),
    }
}

/// Builds the final polyline for one edge, given the exact port coordinate [`evict`] (or, for a
/// single edge in isolation, [`route_edge`]) decided for each end — `source_coord`/`target_coord`
/// are positions along each face's own tangent axis, the same thing [`face_center_coord`] returns
/// for the unevicted (`n = 1`) case.
fn route_with_ports(
    direction: Direction,
    shape: &EdgeShape,
    source: &PlacedNode,
    target: &PlacedNode,
    source_coord: f64,
    target_coord: f64,
    raw: &[Point],
) -> Vec<Point> {
    let source_port = port_at(source, shape.source_side, source_coord, PORT_INSET);
    let target_port = port_at(target, shape.target_side, target_coord, PORT_INSET);

    let mut points = if shape.reverse {
        route_reverse_with_ports(direction, shape, source_port, target_port, raw)
    } else {
        // The one-bend shape branch and merge share, and aligned falls into too: `bridge` between
        // faces of unlike axes is always exactly one corner regardless of eviction's offsets
        // (branch/merge always face unlike axes, so this never grows past its stage-1 bend
        // count); between two faces on the *same* axis (aligned, when both ends land on the flow
        // axis) it is zero bends when eviction gave both ends the same coordinate and two when it
        // did not — see `bridge`'s own doc.
        let mut out = vec![source_port.clone()];
        out.extend(bridge(
            direction,
            &source_port,
            &target_port,
            shape.source_axis,
            shape.target_axis,
        ));
        out
    };

    super::edges::dedupe(&mut points);
    if points.len() < 2 {
        // Defensive only — two distinct nodes with any real size cannot collapse their two ports
        // onto the same pixel. Never draw nothing rather than a degenerate one-point "line".
        points = vec![source.center.clone(), target.center.clone()];
    }
    points
}

/// The back-edge shape: keep dagre's own waypoint chain (it already threads any nodes sitting
/// between the two ranks — §10-2's "暫定" note is exactly that this reuse, not a real perimeter
/// route, is what makes that safe before stage 5) and straighten every leg onto right angles,
/// starting and ending at the ports [`route_with_ports`] already placed.
fn route_reverse_with_ports(
    direction: Direction,
    shape: &EdgeShape,
    source_port: Point,
    target_port: Point,
    raw: &[Point],
) -> Vec<Point> {
    let mut deduped = raw.to_vec();
    super::edges::dedupe(&mut deduped);
    let interior: Vec<Point> = if deduped.len() > 2 {
        deduped[1..deduped.len() - 1].to_vec()
    } else {
        Vec::new()
    };

    let mut out = vec![source_port.clone()];
    let mut prev = source_port;
    let mut prev_axis = shape.source_axis;
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
        shape.target_axis,
    ));
    out
}

/// Routes one node-to-node flowchart edge **in isolation** — the `n = 1` case of [`evict`], with
/// no sibling on either face to share a port with. [`route_flowchart`] is what a real diagram
/// actually goes through (every edge's shape is decided, *then* every face's ports are assigned
/// together); this stays as the direct entry point the single-edge unit tests below exercise each
/// route shape through, and it is exactly what `route_flowchart` also does for any face nothing
/// else lands on — so it disagrees with nothing a real diagram draws.
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
    let shape = classify(
        direction,
        source,
        target,
        raw,
        source_rank,
        target_rank,
        source_out_degree,
        target_in_degree,
    );
    let source_coord = face_center_coord(source, shape.source_side);
    let target_coord = face_center_coord(target, shape.target_side);
    route_with_ports(
        direction,
        &shape,
        source,
        target,
        source_coord,
        target_coord,
        raw,
    )
}

/// Which end of an edge a [`FaceClaim`] is — which of [`Eviction::source_coord`] /
/// [`Eviction::target_coord`] the port [`evict`] decides for it belongs in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FaceEnd {
    Source,
    Target,
}

/// One edge's claim on one face — everything [`evict`] needs to place it: which edge (so the
/// decided coordinate can be written back), which end of it, where its *other* end sits (the sort
/// key, so port order matches "もう一方の端点のcross座標順" and the lines to those other ends
/// never cross just before the face), and whether it is the one aligned edge rule 2 reserves the
/// centre for.
struct FaceClaim {
    edge_id: String,
    end: FaceEnd,
    other_cross: f64,
    aligned: bool,
}

/// The caller's own view of one node-to-node edge — everything [`route_flowchart`] needs about it
/// that a completed dagre layout does not already carry on the two [`PlacedNode`]s themselves.
pub struct EligibleEdge<'a> {
    pub id: &'a str,
    pub source: &'a str,
    pub target: &'a str,
    pub raw: &'a [Point],
    pub source_rank: Option<i32>,
    pub target_rank: Option<i32>,
    pub source_out_degree: usize,
    pub target_in_degree: usize,
}

/// [`route_flowchart`]'s result: every edge's finished polyline, and the minimum size every node
/// needs so its busiest face still keeps [`PORT_CLEARANCE`] px between the outermost port and the
/// corner.
pub struct RoutedFlowchart {
    /// Edge id → the polyline [`route_with_ports`] built for it.
    pub points: HashMap<String, Vec<Point>>,
    /// Node id → the smallest size that face demand alone asks for. `lay_out_spec`'s growth loop
    /// takes the max of this and whatever size the node already has — this map only ever states a
    /// *minimum*, never shrinks anything, and a node no face names at all is simply absent from
    /// it.
    pub required_size: HashMap<String, Size>,
}

/// [`evict`]'s result — see its own doc for how each field is built.
struct Eviction {
    source_coord: HashMap<String, f64>,
    target_coord: HashMap<String, f64>,
    required_size: HashMap<String, Size>,
}

/// One global eviction pass: given every edge's already-decided [`EdgeShape`], works out the exact
/// port coordinate for both ends of every edge, and the minimum size every node needs to fit them.
///
/// Grouped by `(node id, face)` rather than by node: two different faces of the same node are
/// independent corridors with their own port count, so growing one (§10-1 item 1: "ノードをその
/// 軸方向に拡大") only ever widens (for `Top`/`Bottom`) or heightens (for `Left`/`Right`) the node
/// — the two axes never fight over the same requirement, and [`Eviction::required_size`] reports
/// each independently.
fn evict(
    direction: Direction,
    by_id: &HashMap<&str, &PlacedNode>,
    edges: &[EligibleEdge],
    shapes: &[Option<EdgeShape>],
) -> Eviction {
    let mut groups: HashMap<(String, Side), Vec<FaceClaim>> = HashMap::new();
    for (edge, shape) in edges.iter().zip(shapes) {
        let Some(shape) = shape else { continue };
        let (Some(&source), Some(&target)) = (by_id.get(edge.source), by_id.get(edge.target))
        else {
            continue;
        };
        groups
            .entry((edge.source.to_string(), shape.source_side))
            .or_default()
            .push(FaceClaim {
                edge_id: edge.id.to_string(),
                end: FaceEnd::Source,
                other_cross: cross(direction, &target.center),
                aligned: shape.aligned,
            });
        groups
            .entry((edge.target.to_string(), shape.target_side))
            .or_default()
            .push(FaceClaim {
                edge_id: edge.id.to_string(),
                end: FaceEnd::Target,
                other_cross: cross(direction, &source.center),
                aligned: shape.aligned,
            });
    }

    let mut source_coord: HashMap<String, f64> = HashMap::new();
    let mut target_coord: HashMap<String, f64> = HashMap::new();
    let mut required_size: HashMap<String, Size> = HashMap::new();

    for ((node_id, side), mut claims) in groups {
        let Some(&node) = by_id.get(node_id.as_str()) else {
            continue;
        };

        // Deterministic order: "もう一方の端点のcross座標順" (rule 1), ties broken by edge id —
        // a real, stable key, unlike the arbitrary order a `HashMap`-built group starts in.
        claims.sort_by(|a, b| {
            a.other_cross
                .partial_cmp(&b.other_cross)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.edge_id.cmp(&b.edge_id))
        });

        let n = claims.len();
        // Rule 2: an aligned (zero-bend) edge takes the slot closest to the face's own centre —
        // exactly the centre when `n` is odd. When `n` is even the 16px grid has no exact centre
        // slot at all (its two middle slots sit at ±(PORT_SPACING/2)); `f64::round`'s "half away
        // from zero" rule picks the *higher* index of the two (`(n-1)/2` is exactly `x.5`, which
        // rounds up), deterministically — a corner case the spec does not disambiguate further.
        // If more than one claim on the same face is somehow aligned (two dead-straight edges
        // sharing a face — not reachable from any real flowchart, since it needs two different
        // nodes at the same cross coordinate as this one), only the first is moved; the rest keep
        // their sorted position like any other claim.
        if let Some(aligned_pos) = claims.iter().position(|c| c.aligned) {
            let center_idx = (((n - 1) as f64) / 2.0).round() as usize;
            if aligned_pos != center_idx {
                let claim = claims.remove(aligned_pos);
                claims.insert(center_idx, claim);
            }
        }

        for (i, claim) in claims.iter().enumerate() {
            let offset = (i as f64 - (n as f64 - 1.0) / 2.0) * PORT_SPACING;
            let coord = face_center_coord(node, side) + offset;
            match claim.end {
                FaceEnd::Source => {
                    source_coord.insert(claim.edge_id.clone(), coord);
                }
                FaceEnd::Target => {
                    target_coord.insert(claim.edge_id.clone(), coord);
                }
            }
        }

        // The flat run this face needs: `(n-1)` gaps of `PORT_SPACING` between `n` ports, plus
        // `PORT_CLEARANCE` clear on each side of the outermost one.
        let required_flat = if n == 0 {
            0.0
        } else {
            (n as f64 - 1.0) * PORT_SPACING + 2.0 * PORT_CLEARANCE
        };
        // A chamfered rectangle's flat run is shorter than its box by the chamfer on each end —
        // §10-1 item 1: "面取り矩形は面取り6px分も平坦部から除くこと" — so the box itself has to
        // be that much bigger again to leave the same flat run a plain rectangle would.
        let chamfer_allowance = if node.shape == Glyph::ChamferedRect {
            2.0 * shapes::CHAMFER
        } else {
            0.0
        };
        let needed = required_flat + chamfer_allowance;
        let entry = required_size.entry(node_id).or_insert(Size::new(0.0, 0.0));
        match side {
            // Top and Bottom are the box's horizontal edges: their flat run is the box's WIDTH,
            // whichever axis is "flow" in this diagram's `direction`. Left and Right are its
            // vertical edges, run along HEIGHT. This is a fact about a rectangle, not about
            // `direction` — unlike `flow`/`cross`, `Side` is already a physical direction.
            Side::Top | Side::Bottom => entry.w = entry.w.max(needed),
            Side::Left | Side::Right => entry.h = entry.h.max(needed),
        }
    }

    Eviction {
        source_coord,
        target_coord,
        required_size,
    }
}

/// Routes every node-to-node edge in one flowchart under `[ui] mermaid_routing =
/// "konoma-orthogonal"`, running [`classify`] and [`evict`] once each over the whole diagram
/// before building any polyline — the two-pass shape the module doc describes.
///
/// `nodes` is the diagram's placed nodes from *this* layout pass; `edges` is every drawable edge
/// whose two ends are both real nodes (a cluster boundary never reaches this function — it is out
/// of scope until §10-2's later stages, and keeps drawing through the ordinary spline path,
/// `lay_out_spec`'s own filter on `edges::End::Node`).
pub fn route_flowchart(
    direction: Direction,
    nodes: &[PlacedNode],
    edges: &[EligibleEdge],
) -> RoutedFlowchart {
    let by_id: HashMap<&str, &PlacedNode> = nodes.iter().map(|n| (n.id.as_str(), n)).collect();

    let shapes: Vec<Option<EdgeShape>> = edges
        .iter()
        .map(|e| {
            let (Some(&source), Some(&target)) = (by_id.get(e.source), by_id.get(e.target)) else {
                return None;
            };
            Some(classify(
                direction,
                source,
                target,
                e.raw,
                e.source_rank,
                e.target_rank,
                e.source_out_degree,
                e.target_in_degree,
            ))
        })
        .collect();

    let eviction = evict(direction, &by_id, edges, &shapes);

    let mut points = HashMap::with_capacity(edges.len());
    for (edge, shape) in edges.iter().zip(&shapes) {
        let Some(shape) = shape else { continue };
        let (Some(&source), Some(&target)) = (by_id.get(edge.source), by_id.get(edge.target))
        else {
            continue;
        };
        let source_coord = eviction
            .source_coord
            .get(edge.id)
            .copied()
            .unwrap_or_else(|| face_center_coord(source, shape.source_side));
        let target_coord = eviction
            .target_coord
            .get(edge.id)
            .copied()
            .unwrap_or_else(|| face_center_coord(target, shape.target_side));
        let routed = route_with_ports(
            direction,
            shape,
            source,
            target,
            source_coord,
            target_coord,
            edge.raw,
        );
        points.insert(edge.id.to_string(), routed);
    }

    RoutedFlowchart {
        points,
        required_size: eviction.required_size,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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

    // --- stage 2: port eviction --------------------------------------------------------------

    /// A helper that runs `route_flowchart` over a small set of nodes/edges and returns the
    /// routed points keyed by a simpler `(from, to)` pair, for tests that do not need to juggle
    /// edge ids.
    fn route_all<'a>(
        direction: Direction,
        nodes: &[PlacedNode],
        edges: &[EligibleEdge<'a>],
    ) -> RoutedFlowchart {
        route_flowchart(direction, nodes, edges)
    }

    /// Three edges into `target`, none of them exactly above it (so none is `aligned` by
    /// accident) — `source_out_degree: 2` forces every one of them to classify as **branch**
    /// (`docs/FEATURE-MERMAID-RENDERER.md` §10-1 item 1: a branch's target port is on the
    /// *flow* face — `Top`/`Bottom` for `TD` — which is the face this whole test file's
    /// eviction fixtures want, since [`face_center_coord`] reads its tangent from `.x` there. A
    /// **merge**'s target port would instead land on the *cross* face (`Left`/`Right`), which is
    /// why the first draft of these tests, written with `source_out_degree: 1` (merge), measured
    /// the wrong axis and failed for a reason that had nothing to do with eviction itself — see
    /// this function's own callers for the numbers that caught it.
    fn three_branches_into<'a>(target: &'a str) -> Vec<EligibleEdge<'a>> {
        vec![
            EligibleEdge {
                id: "e1",
                source: "X",
                target,
                raw: &[],
                source_rank: Some(0),
                target_rank: Some(1),
                source_out_degree: 2,
                target_in_degree: 3,
            },
            EligibleEdge {
                id: "e2",
                source: "Y",
                target,
                raw: &[],
                source_rank: Some(0),
                target_rank: Some(1),
                source_out_degree: 2,
                target_in_degree: 3,
            },
            EligibleEdge {
                id: "e3",
                source: "Z",
                target,
                raw: &[],
                source_rank: Some(0),
                target_rank: Some(1),
                source_out_degree: 2,
                target_in_degree: 3,
            },
        ]
    }

    #[test]
    fn three_incoming_edges_get_16px_ports_symmetric_about_the_face_and_clear_of_corners() {
        // X (x=200), Y (x=480) and Z (x=800) all branch into T (x=500) from above — none exactly
        // matches T's own x, so none is `aligned` by accident.
        let target = node("T", 500.0, 300.0, 300.0, 60.0);
        let x = node("X", 200.0, 100.0, 60.0, 40.0);
        let y = node("Y", 480.0, 100.0, 60.0, 40.0);
        let z = node("Z", 800.0, 100.0, 60.0, 40.0);
        let nodes = vec![target.clone(), x.clone(), y.clone(), z.clone()];
        let edges = three_branches_into("T");
        let routed = route_all(Direction::TopToBottom, &nodes, &edges);

        let tip_x = routed.points["e1"].last().unwrap().x;
        let tip_y = routed.points["e2"].last().unwrap().x;
        let tip_z = routed.points["e3"].last().unwrap().x;
        let mut xs = [tip_x, tip_y, tip_z];
        xs.sort_by(|a, b| a.partial_cmp(b).unwrap());

        // 16px apart, symmetric about T's own centre (500.0).
        assert!((xs[1] - target.center.x).abs() < 1e-9, "{xs:?}");
        assert!((xs[1] - xs[0] - PORT_SPACING).abs() < 1e-9, "{xs:?}");
        assert!((xs[2] - xs[1] - PORT_SPACING).abs() < 1e-9, "{xs:?}");

        // Order matches the sort key: X (cross=200) leftmost, Z (cross=800) rightmost.
        assert!(tip_x < tip_y && tip_y < tip_z, "{tip_x} {tip_y} {tip_z}");

        // Every port at least PORT_CLEARANCE from the corner (T is 300px wide, so its half-width,
        // 150px, is nowhere close to binding here — this states the margin explicitly).
        let half_width = target.size.w / 2.0;
        for x in xs {
            let from_center = (x - target.center.x).abs();
            assert!(
                half_width - from_center >= PORT_CLEARANCE - 1e-9,
                "port at {from_center}px from centre must clear the corner by {PORT_CLEARANCE}px \
                 (half-width {half_width}): {xs:?}"
            );
        }
    }

    #[test]
    fn an_aligned_edge_keeps_the_centre_port_and_siblings_move_outward() {
        // B sits directly below A (aligned, x=300 both) and ALSO receives a branch from each of C
        // and D (`source_out_degree: 2`, so each lands on B's Top face too, not B's Left/Right —
        // see `three_branches_into`'s own doc). Three claims on B's Top face, an odd count, is
        // what gives the 16px grid an exact centre slot at offset 0 — rule 2 puts the aligned
        // A->B there and pushes both branching siblings to the outer two slots, ±16px, never 0.
        let a = node("A", 300.0, 0.0, 60.0, 40.0);
        let b = node("B", 300.0, 200.0, 60.0, 40.0);
        let c = node("C", 100.0, 0.0, 60.0, 40.0);
        let d = node("D", 500.0, 0.0, 60.0, 40.0);
        let nodes = vec![a.clone(), b.clone(), c.clone(), d.clone()];
        let edges = vec![
            EligibleEdge {
                id: "ab",
                source: "A",
                target: "B",
                raw: &[],
                source_rank: Some(0),
                target_rank: Some(1),
                source_out_degree: 1,
                target_in_degree: 3,
            },
            EligibleEdge {
                id: "cb",
                source: "C",
                target: "B",
                raw: &[],
                source_rank: Some(0),
                target_rank: Some(1),
                source_out_degree: 2,
                target_in_degree: 3,
            },
            EligibleEdge {
                id: "db",
                source: "D",
                target: "B",
                raw: &[],
                source_rank: Some(0),
                target_rank: Some(1),
                source_out_degree: 2,
                target_in_degree: 3,
            },
        ];
        let routed = route_all(Direction::TopToBottom, &nodes, &edges);

        let ab_tip = routed.points["ab"].last().unwrap();
        let cb_tip = routed.points["cb"].last().unwrap();
        let db_tip = routed.points["db"].last().unwrap();
        assert!(
            (ab_tip.x - b.center.x).abs() < 1e-9,
            "the aligned edge must keep the centre port: {ab_tip:?}"
        );
        for (name, tip) in [("cb", cb_tip), ("db", db_tip)] {
            assert!(
                (tip.x - b.center.x).abs() > 1e-9,
                "{name}: the branching sibling must not share the centre port: {tip:?}"
            );
            assert!(
                ((tip.x - b.center.x).abs() - PORT_SPACING).abs() < 1e-9,
                "{name}: the sibling must sit exactly one port-spacing away: {tip:?}"
            );
        }
    }

    #[test]
    fn a_narrow_node_grows_to_fit_its_ports_and_a_roomy_one_does_not() {
        // NARROW is only 20px wide — nowhere near the 3-port minimum flat run of
        // `(3-1)*16 + 2*8 = 48px` — and receives 3 branching edges on its Top face. ROOMY is
        // 300px wide and receives the same 3 edges: its face was never the bottleneck.
        let narrow = node("NARROW", 0.0, 300.0, 20.0, 40.0);
        let roomy = node("ROOMY", 500.0, 300.0, 300.0, 40.0);
        let x = node("X", -50.0, 100.0, 40.0, 30.0);
        let y = node("Y", 225.0, 100.0, 40.0, 30.0);
        let z = node("Z", 490.0, 100.0, 40.0, 30.0);
        let nodes = vec![
            narrow.clone(),
            roomy.clone(),
            x.clone(),
            y.clone(),
            z.clone(),
        ];

        let routed_narrow = route_all(
            Direction::TopToBottom,
            &nodes,
            &three_branches_into("NARROW"),
        );
        let required_w = routed_narrow.required_size["NARROW"].w;
        assert!(
            (required_w - 48.0).abs() < 1e-9,
            "3 ports need (3-1)*16 + 2*8 = 48px of flat run: got {required_w}"
        );

        let routed_roomy = route_all(
            Direction::TopToBottom,
            &nodes,
            &three_branches_into("ROOMY"),
        );
        assert!(
            !routed_roomy.required_size.contains_key("ROOMY")
                || routed_roomy.required_size["ROOMY"].w <= roomy.size.w,
            "a face that already fits its ports must not ask to grow: {:?}",
            routed_roomy.required_size.get("ROOMY")
        );
    }

    #[test]
    fn chamfered_rect_required_size_adds_the_chamfer_allowance_on_top_of_the_flat_run() {
        let mut target = node("T", 300.0, 300.0, 20.0, 40.0);
        target.shape = Glyph::ChamferedRect;
        let x = node("X", 200.0, 100.0, 40.0, 30.0);
        let y = node("Y", 400.0, 100.0, 40.0, 30.0);
        let nodes = vec![target.clone(), x.clone(), y.clone()];
        let edges = vec![
            EligibleEdge {
                id: "e1",
                source: "X",
                target: "T",
                raw: &[],
                source_rank: Some(0),
                target_rank: Some(1),
                source_out_degree: 2,
                target_in_degree: 2,
            },
            EligibleEdge {
                id: "e2",
                source: "Y",
                target: "T",
                raw: &[],
                source_rank: Some(0),
                target_rank: Some(1),
                source_out_degree: 2,
                target_in_degree: 2,
            },
        ];
        let routed = route_all(Direction::TopToBottom, &nodes, &edges);
        // 2 ports on T's Top face: (2-1)*16 + 2*8 = 32px flat run, plus 2*6 = 12px chamfer
        // allowance = 44px.
        let required_w = routed.required_size["T"].w;
        assert!(
            (required_w - 44.0).abs() < 1e-9,
            "expected 32px flat run + 12px chamfer allowance = 44px: got {required_w}"
        );
    }

    #[test]
    fn eviction_keeps_every_port_exactly_port_inset_outside_the_node() {
        let target = node("T", 500.0, 300.0, 300.0, 60.0);
        let x = node("X", 200.0, 100.0, 60.0, 40.0);
        let y = node("Y", 480.0, 100.0, 60.0, 40.0);
        let z = node("Z", 800.0, 100.0, 60.0, 40.0);
        let nodes = vec![target.clone(), x.clone(), y.clone(), z.clone()];
        let edges = three_branches_into("T");
        let routed = route_all(Direction::TopToBottom, &nodes, &edges);
        let (_, top, _, _) = target.bounds();
        for id in ["e1", "e2", "e3"] {
            let tip = routed.points[id].last().unwrap();
            assert!(
                (tip.y - (top - PORT_INSET)).abs() < 1e-9,
                "{id}: evicted port must still sit exactly PORT_INSET outside the face: {tip:?}"
            );
        }
    }
}
