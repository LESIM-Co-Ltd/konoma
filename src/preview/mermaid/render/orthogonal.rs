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
//! # A back edge draws from a perimeter lane, not dagre's own waypoints
//!
//! §10-1 item 4's "外周レーン" — stage 5 — is [`route_perimeter`]: a reverse edge
//! ([`EdgeShape::reverse`]) or a collision-fallback forward edge ([`EdgeShape::staircase`]) leaves
//! and enters through the same ports every other shape uses, then travels straight out to a
//! rectangle [`PERIMETER_MARGIN`] px outside every node and frame in the diagram and around
//! whichever way is shorter to the same straight-out point at the other end. This *replaces*
//! stages 1-4's "暫定" (stopgap) — dagre's own waypoint chain, bent onto right angles — which the
//! module doc used to describe as this module's honest limitation; [`route_staircase_with_ports`]
//! is that stopgap, kept only for a **self-loop** (`route_with_ports`'s own doc explains why: a
//! loop that starts and ends at the same box has no business circling the whole diagram).
//!
//! # Growing a node changes the layout, so eviction can take more than one pass
//!
//! A face too narrow for its ports has to grow the node — and a node's size is exactly what dagre
//! lays out from, so growing one can move everything. [`super::lay_out_spec`] is what actually
//! retries dagre with a grown size; this module only ever answers, for one already-completed
//! layout, "how many edges are on this face, and how big does the node need to be to fit them" —
//! see [`Eviction::required_size`].
//!
//! # Lane alignment moves nodes; collision avoidance changes a shape's faces
//!
//! §10-1 item 2 ("レーン揃え") is [`align_straight_lanes`] — a separate pass `lay_out_spec` runs
//! *before* `route_flowchart`, because it moves [`PlacedNode::center`] itself (greedily selects a
//! maximal set of node-disjoint straight-lane edges between adjacent ranks and slides every member
//! of each resulting chain onto one shared cross coordinate), and every downstream node position —
//! frames, other edges' ports — has to see the moved position, not the one dagre laid out. Once
//! nodes are moved, [`classify`]'s existing `aligned` check (unchanged) recognises a lane-aligned
//! chain's edges on its own: they are now geometrically aligned, the same way a chain that already
//! happened to line up under stage 1/2 was.
//!
//! §10-1 item 1's collision fix ("分岐形の走行がノード箱と交差するなら合流形に切替えて再試行…両形
//! とも交差するなら…階段経路へフォールバック") lives inside [`classify`] itself: a branch or merge
//! shape's cross-axis sweep can run through a *sibling* node's box (the same rank as whichever end
//! is doing the sweeping), so `classify` builds the shape's route at zero eviction offset, tests it
//! against every other node's box, and — if it crosses one — tries the other shape, and if that
//! also crosses, marks the edge `staircase` so [`route_with_ports`] draws it the same way a back
//! edge is drawn (dagre's own waypoints, straightened) instead.
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

use super::{shapes, Glyph, PlacedCluster, PlacedEdgeLabel, PlacedNode, Size};
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
///   edge's staircase ([`route_staircase_with_ports`]).
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
    /// Whether the shape's own route (branch, merge, or even aligned) crossed another node's box
    /// at both attempts and had to fall back to dagre's own waypoint chain, the same way a back
    /// edge is drawn — §10-1 item 1's collision fix, item (b). Drawn by [`route_with_ports`]
    /// exactly like [`EdgeShape::reverse`] (`route_staircase_with_ports`), but keeps its own flag:
    /// unlike a true back edge, a staircase-fallback edge is still forward (its ports still came
    /// from the ordinary branch/merge/aligned face choice `evict` grouped it by), so conflating
    /// the two would make [`route_flowchart`]'s bend-count reasoning about which edges are
    /// rank-reversed (exempt from the ≤2 cap) wrong for this one.
    staircase: bool,
    /// Which face of the source the line leaves through, and which axis leaving perpendicular to
    /// it means moving along.
    source_side: Side,
    source_axis: Axis,
    /// Which face of the target the line enters through, and which axis entering perpendicular to
    /// it means moving along.
    target_side: Side,
    target_axis: Axis,
}

/// How much a routed segment's box test is padded past the node's real boundary — §10-1 item 1's
/// "少しのマージン付き": a segment that only grazes a corner should still count as blocked, not
/// pass the test by a fraction of a pixel.
pub(crate) const COLLISION_MARGIN: f64 = 4.0;

/// Whether the axis-parallel segment `a`–`b` crosses `node`'s box, padded by [`COLLISION_MARGIN`].
/// `a`–`b` is assumed axis-parallel (every segment this module ever builds is); a genuinely
/// diagonal pair is treated as a non-crossing no-op rather than panicking, since a defensive
/// "never block on a shape that cannot happen" is safer than a crash over this check alone.
///
/// `pub(crate)` (not private) so the integration tests in `render::tests` can state the same
/// "does a routed segment cross a foreign node" question `classify` asks itself, against the
/// *final*, evicted route a real diagram actually draws — reusing this rather than a second,
/// hand-rolled copy of the same box-intersection arithmetic in a different module.
pub(crate) fn segment_crosses_node(a: &Point, b: &Point, node: &PlacedNode) -> bool {
    let (l, t, r, bo) = node.bounds();
    let (l, t, r, bo) = (
        l - COLLISION_MARGIN,
        t - COLLISION_MARGIN,
        r + COLLISION_MARGIN,
        bo + COLLISION_MARGIN,
    );
    if (a.y - b.y).abs() < EPS {
        let y = a.y;
        let (x0, x1) = (a.x.min(b.x), a.x.max(b.x));
        y >= t && y <= bo && x1 >= l && x0 <= r
    } else if (a.x - b.x).abs() < EPS {
        let x = a.x;
        let (y0, y1) = (a.y.min(b.y), a.y.max(b.y));
        x >= l && x <= r && y1 >= t && y0 <= bo
    } else {
        false
    }
}

/// Whether `shape`'s route between `source` and `target` — built at zero eviction offset, since a
/// port's few-pixel eviction offset never changes whether a sweep spanning the whole diagram
/// clears a sibling's box — crosses any node in `nodes` other than `source`/`target` themselves.
/// §10-1 item 1's collision test: "辺セグメント vs 全ノード境界箱…の交差テストで機械的に".
fn shape_crosses_a_node(
    direction: Direction,
    source: &PlacedNode,
    target: &PlacedNode,
    shape: &EdgeShape,
    nodes: &[PlacedNode],
) -> bool {
    let source_port = face_port(source, shape.source_side, PORT_INSET);
    let target_port = face_port(target, shape.target_side, PORT_INSET);
    let mut pts = vec![source_port.clone()];
    pts.extend(bridge(
        direction,
        &source_port,
        &target_port,
        shape.source_axis,
        shape.target_axis,
    ));
    for w in pts.windows(2) {
        for node in nodes {
            if node.id == source.id || node.id == target.id {
                continue;
            }
            if segment_crosses_node(&w[0], &w[1], node) {
                return true;
            }
        }
    }
    false
}

/// Decides `source`→`target`'s route shape — the same four-way decision stage 1's `route_edge`
/// made inline, factored out so [`evict`] can see it for every edge before any of them gets a
/// port. Four shapes, decided in this order:
///
/// 1. **reverse** — the target's rank is at or before the source's. dagre still produced a
///    waypoint chain for it (walking through whatever dummy nodes the intervening ranks needed),
///    so the exit/entry face is picked by [`dominant_face`] against the nearest waypoint dagre
///    actually routed through, and the interior chain is what [`route_staircase_with_ports`]
///    straightens onto right angles.
/// 2. **aligned** — same coordinate on the axis across the flow (within half a pixel). Both faces
///    are on the flow axis, facing each other.
/// 3. **branch** — the source has more than one outgoing edge (or neither this nor 4 applies, the
///    default): leaves the source through the face across the flow and turns once into the
///    target's upstream face.
/// 4. **merge** — not a branch, and the target has more than one incoming edge: leaves the source
///    through its downstream face and turns once into the target's face across the flow.
///
/// For the non-reverse shapes (2–4), §10-1 item 1's collision fix runs before returning: if the
/// shape's own route crosses a node other than its two ends, the *other* of branch/merge is tried
/// (aligned has no alternate shape to swap to — its faces are fixed by which axis the two nodes
/// are aligned on); if that also crosses, or there was never an alternate to try, the edge is
/// marked `staircase` and keeps the last shape's faces (so `evict` still groups it by a real face)
/// — [`route_with_ports`] then draws it from dagre's own waypoints instead of a bend/bridge.
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
    nodes: &[PlacedNode],
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
            staircase: false,
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
        let mut shape = EdgeShape {
            reverse: false,
            aligned: true,
            staircase: false,
            source_side,
            source_axis: Axis::Flow,
            target_side,
            target_axis: Axis::Flow,
        };
        // An aligned edge has no alternate shape to swap to (its two faces are fixed by which
        // axis the nodes are aligned on), so the collision fix's only move for it is the
        // fallback: a multi-rank aligned pair (e.g. a long edge that happens to line up) can
        // still run straight through a node sitting in one of the ranks it skips over.
        if shape_crosses_a_node(direction, source, target, &shape, nodes) {
            shape.staircase = true;
        }
        return shape;
    }

    // "分岐形（source の out-degree > 1、または下記どちらでもない既定）" / "合流形（分岐形でなく
    // target の in-degree > 1）" — branch is the default; merge is the one exception, taken only
    // when the source is not itself branching and the target actually merges.
    let branching = source_out_degree > 1 || target_in_degree <= 1;
    let (branch_source_side, branch_target_side) = (
        cross_face(
            direction,
            cross(direction, &target.center) - cross(direction, &source.center),
        ),
        flow_face(
            direction,
            flow(direction, &source.center) - flow(direction, &target.center),
        ),
    );
    let (merge_source_side, merge_target_side) = (
        flow_face(
            direction,
            flow(direction, &target.center) - flow(direction, &source.center),
        ),
        cross_face(
            direction,
            cross(direction, &source.center) - cross(direction, &target.center),
        ),
    );
    let (source_side, target_side) = if branching {
        (branch_source_side, branch_target_side)
    } else {
        (merge_source_side, merge_target_side)
    };
    let mut shape = EdgeShape {
        reverse: false,
        aligned: false,
        staircase: false,
        source_side,
        source_axis: axis_of(direction, source_side),
        target_side,
        target_axis: axis_of(direction, target_side),
    };
    if shape_crosses_a_node(direction, source, target, &shape, nodes) {
        // "分岐形の走行がノード箱と交差するなら合流形に切替えて再試行" — generalised (§10-1's
        // own "衝突しない限り" is the instruction to generalise) to run symmetrically from
        // whichever shape was natural: try the other one next, whichever that is.
        let (alt_source_side, alt_target_side) = if branching {
            (merge_source_side, merge_target_side)
        } else {
            (branch_source_side, branch_target_side)
        };
        let alt = EdgeShape {
            reverse: false,
            aligned: false,
            staircase: false,
            source_side: alt_source_side,
            source_axis: axis_of(direction, alt_source_side),
            target_side: alt_target_side,
            target_axis: axis_of(direction, alt_target_side),
        };
        if shape_crosses_a_node(direction, source, target, &alt, nodes) {
            // "両形とも交差するなら既存の階段経路（dagre経由点）へフォールバック" — keep the
            // alternate's faces (the last one actually tried) so `evict` still has a real face
            // to group this edge's ports by; only how the two ports are *joined* changes.
            shape = alt;
            shape.staircase = true;
        } else {
            shape = alt;
        }
    }
    shape
}

/// Builds the final polyline for one edge, given the exact port coordinate [`evict`] (or, for a
/// single edge in isolation, [`route_edge`]) decided for each end — `source_coord`/`target_coord`
/// are positions along each face's own tangent axis, the same thing [`face_center_coord`] returns
/// for the unevicted (`n = 1`) case. `ring` and `nodes` are [`route_perimeter`]'s own inputs (a
/// lane rectangle and the whole diagram's nodes, for its collision check) — unused by every shape
/// but a non-self-loop `reverse`/`staircase` edge, but threaded through uniformly rather than
/// rebuilt per call (`route_flowchart` already has both on hand).
#[allow(clippy::too_many_arguments)]
fn route_with_ports(
    direction: Direction,
    shape: &EdgeShape,
    source: &PlacedNode,
    target: &PlacedNode,
    source_coord: f64,
    target_coord: f64,
    raw: &[Point],
    ring: (f64, f64, f64, f64),
    nodes: &[PlacedNode],
) -> Vec<Point> {
    let source_port = port_at(source, shape.source_side, source_coord, PORT_INSET);
    let target_port = port_at(target, shape.target_side, target_coord, PORT_INSET);

    let mut points = if (shape.reverse || shape.staircase) && source.id == target.id {
        // A self-loop starts and ends at the same box — §10-1 item 4's perimeter lane is for a
        // real return path across the diagram, and would be absurd here (a small loop ballooning
        // out to circle the whole drawing). Dagre's own tiny waypoint chain, already sitting right
        // beside the node, stays exactly as stages 1-4 drew it.
        route_staircase_with_ports(direction, shape, source_port, target_port, raw)
    } else if shape.reverse || shape.staircase {
        let ids = (source.id.as_str(), target.id.as_str());
        let blocked = |a: &Point, b: &Point| segment_crosses_any_node(a, b, nodes, ids);
        route_perimeter(shape, source_port, target_port, ring, &blocked)
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

/// The self-loop shape: dagre's own waypoint chain, straightened onto right angles. Stages 1-4's
/// only staircase shape, now used for nothing else — see [`route_with_ports`]'s own doc for why a
/// self-loop stays off the perimeter lane [`route_perimeter`] draws every other reverse/staircase
/// edge from.
fn route_staircase_with_ports(
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

// -------------------------------------------------------------------------------------------
// §10-1 item 4: the perimeter lane
// -------------------------------------------------------------------------------------------

/// How far outside every node and frame in the diagram the *nearest* perimeter lane sits — §10-1
/// item 4: "外周レーンは最も外側の枠から16px以上外に置く".
pub const PERIMETER_MARGIN: f64 = 16.0;

/// How far apart two perimeter edges' own lanes sit when more than one needs one — §10-1 item 4:
/// "複数の外周辺は8px ずつずらして並走させる". A separate constant from [`PORT_SPACING`]/
/// [`LABEL_CLEARANCE`]'s own 16px-ish numbers for the same reason those two are separate from each
/// other: same value, different rule, and a future change to one must not silently move the rest.
pub const PERIMETER_LANE_SPACING: f64 = 8.0;

/// The smallest axis-aligned box holding every node and every subgraph frame in the diagram — what
/// [`route_perimeter`]'s lane rectangle is built by expanding outward from. `clusters` alone is not
/// enough on its own (a top-level node outside every frame still has to stay clear of a lane) and
/// neither is `nodes` alone (a frame can extend past its own members' padding); the box has to hold
/// both.
///
/// `(0.0, 0.0, 0.0, 0.0)` for an empty `nodes` — defensive only, `lay_out_spec` already rejects an
/// empty diagram before any routing code runs (`RenderError::NothingToDraw`).
fn content_bounds(nodes: &[PlacedNode], clusters: &[PlacedCluster]) -> (f64, f64, f64, f64) {
    let mut l = f64::INFINITY;
    let mut t = f64::INFINITY;
    let mut r = f64::NEG_INFINITY;
    let mut b = f64::NEG_INFINITY;
    for n in nodes {
        let (nl, nt, nr, nb) = n.bounds();
        l = l.min(nl);
        t = t.min(nt);
        r = r.max(nr);
        b = b.max(nb);
    }
    for c in clusters {
        let (cl, ct, cr, cb) = c.bounds();
        l = l.min(cl);
        t = t.min(ct);
        r = r.max(cr);
        b = b.max(cb);
    }
    if !l.is_finite() {
        return (0.0, 0.0, 0.0, 0.0);
    }
    (l, t, r, b)
}

/// `bounds`, pushed `by` px further out on every side — [`content_bounds`] turned into one edge's
/// own perimeter lane rectangle.
fn expand_bounds(bounds: (f64, f64, f64, f64), by: f64) -> (f64, f64, f64, f64) {
    (bounds.0 - by, bounds.1 - by, bounds.2 + by, bounds.3 + by)
}

/// Where a straight stub leaving `port` perpendicular to `side` first reaches `ring` — a
/// perimeter edge's port-to-lane leg when nothing sits in the way. Always a single axis-parallel
/// segment from `port` (§10-1 item 1's "垂直入射" — the port itself is unchanged by this stage)
/// because `ring` is built by expanding [`content_bounds`], which always contains the node `port`
/// sits on, so the interior point (`port`) and `ring`'s corresponding side are already aligned on
/// `port`'s own tangent coordinate — no bend is needed to reach it *geometrically*. Whether that
/// straight run is actually clear of every other node is [`safe_ring_exit`]'s question, not this
/// function's — this is the one candidate every route always considers first.
fn ring_touch(side: Side, port: &Point, ring: (f64, f64, f64, f64)) -> Point {
    let (l, t, r, b) = ring;
    match side {
        Side::Top => Point::new(port.x, t),
        Side::Bottom => Point::new(port.x, b),
        Side::Left => Point::new(l, port.y),
        Side::Right => Point::new(r, port.y),
    }
}

/// Whether the axis-parallel segment `a`-`b` crosses any node in `nodes` other than the two whose
/// ids are `exclude` (an edge's own two ends) — [`safe_ring_exit`]'s collision test, reusing
/// [`segment_crosses_node`] (the same check `classify`'s branch/merge collision fix already runs)
/// rather than a second copy of the same box arithmetic.
fn segment_crosses_any_node(
    a: &Point,
    b: &Point,
    nodes: &[PlacedNode],
    exclude: (&str, &str),
) -> bool {
    nodes
        .iter()
        .any(|n| n.id != exclude.0 && n.id != exclude.1 && segment_crosses_node(a, b, n))
}

/// A perpendicular exit from `port` through `side`, out to `ring` — §10-1 item 1's own collision
/// fix ("辺セグメント...の交差テストで機械的に"), applied here because a straight run from an
/// interior port out to the ring can pass through whatever sits between the two: a back edge's own
/// node is not always already at the diagram's own edge on the axis its exit face happens to point
/// along. Found by dumping a real routed diagram, not assumed — `cjk`'s `D->A` exited straight up
/// through `B`'s own row (and `B`'s edge label) on the way to the ring, since `D->A`'s exit face
/// was chosen (`classify`'s `dominant_face`, unchanged) from the interior dagre waypoint it used to
/// thread through, not from where `D` itself sits in the finished layout.
///
/// Three candidates, in order: the direct run ([`ring_touch`]); an L that turns onto whichever of
/// the ring's *other* two sides (left/right for a `Top`/`Bottom` exit, top/bottom for `Left`/
/// `Right`) is nearer, **stopping the instant it actually reaches that side**; the same L via the
/// *farther* side, for the case something still blocks the nearer one. The first candidate whose
/// every leg clears every other node wins; if all three still cross something, the direct run is
/// kept anyway — the same "nothing left to try" honesty `classify`'s own branch/merge fallback
/// keeps rather than pretending a clean answer exists.
///
/// The L's second leg is safe regardless of what the diagram holds, because nothing sits beyond
/// `ring`'s own boundary by construction ([`content_bounds`]) — **and stopping there is load-
/// bearing, not a style choice**: an earlier version continued a third leg back along the original
/// axis to `direct`'s own coordinate (matching the straight run's own touch point), reasoning that
/// [`ring_path`] needed a "properly aligned" point to hand off to. It does not — any point on the
/// boundary is equally valid, `ring_perimeter_dist` measures every one of them the same way — and
/// the extra leg was actively wrong: when both ends of an edge fall back to the L on the *same*
/// ring side, their two touch points both land exactly on that side's far corners (top-left and
/// bottom-left, say), so `ring_path` — correctly — draws the short way between them, which is now
/// the *whole length of that side*, not the short local jog either end actually needed. Caught by
/// dumping a real `TD` diagram (`flowchart TB` with a decision node looping back into itself,
/// `C -->|再試行| B`): the old third leg drew the return edge's own vertical run the full 394px
/// height of the ring, when the two ports it actually connects sit only 45.9px apart. Stopping at
/// `(corner, hop.y)` keeps the touch point near where the edge actually is.
///
/// Returns the points from (not including) `port` to (including) the point that actually lands on
/// `ring`'s own boundary.
///
/// `blocked` is the obstacle test, taken as a closure rather than a fixed `nodes` slice so this one
/// function serves two different moments: [`route_perimeter`] calls it against every other node,
/// before any label exists to test against; `avoid_label_plates` calls it a second time, later,
/// against every *other edge's label plate* too — the same "found by dumping a real diagram, not
/// assumed" case (`cjk`'s `D->A` also swept straight through `B->D`'s own `テキスト` plate) that
/// motivated this function at all, but one no `nodes`-only test can see, since a plate does not
/// exist until stage 4's own label placement has already run once.
fn safe_ring_exit(
    side: Side,
    port: &Point,
    ring: (f64, f64, f64, f64),
    blocked: &dyn Fn(&Point, &Point) -> bool,
) -> Vec<Point> {
    let (l, t, r, b) = ring;
    let direct = ring_touch(side, port, ring);
    // §10-1 item 1's "垂直入射" binds the segment touching the port itself, not the whole route —
    // `orthogonal_endpoints_sit_outside_the_node_and_arrive_perpendicular`'s own invariant checks
    // only `points[0]`/`points[1]` (and the symmetric pair at the other end). So an L-shaped
    // candidate still leaves perpendicular for a short `PORT_CLEARANCE`-px hop — long enough to
    // clear the node itself, the same clearance stage 2's own eviction already guarantees a port
    // keeps from its face's corner — before the first turn onto the cross axis.
    let hop = match side {
        Side::Top => Point::new(port.x, port.y - PORT_CLEARANCE),
        Side::Bottom => Point::new(port.x, port.y + PORT_CLEARANCE),
        Side::Left => Point::new(port.x - PORT_CLEARANCE, port.y),
        Side::Right => Point::new(port.x + PORT_CLEARANCE, port.y),
    };
    let l_shape = |corner: f64| -> Vec<Point> {
        match side {
            // Stop at `(corner, hop.y)` — that point already sits exactly on `ring`'s own `corner`
            // side, so nothing is gained (and, per this function's own doc, real harm is done) by
            // continuing on to `direct`'s coordinate on the far side.
            Side::Top | Side::Bottom => vec![hop.clone(), Point::new(corner, hop.y)],
            Side::Left | Side::Right => vec![hop.clone(), Point::new(hop.x, corner)],
        }
    };
    let (near, far) = match side {
        Side::Top | Side::Bottom => {
            if (port.x - l) <= (r - port.x) {
                (l, r)
            } else {
                (r, l)
            }
        }
        Side::Left | Side::Right => {
            if (port.y - t) <= (b - port.y) {
                (t, b)
            } else {
                (b, t)
            }
        }
    };
    let candidates: [Vec<Point>; 3] = [vec![direct.clone()], l_shape(near), l_shape(far)];
    for cand in &candidates {
        let mut prev = port.clone();
        let mut clear = true;
        for p in cand {
            if blocked(&prev, p) {
                clear = false;
                break;
            }
            prev = p.clone();
        }
        if clear {
            return cand.clone();
        }
    }
    vec![direct]
}

/// `p`'s clockwise distance around `ring`'s own perimeter from its top-left corner — top edge
/// left-to-right, then right edge top-to-bottom, then bottom edge right-to-left, then left edge
/// bottom-to-top. [`ring_path`]'s own unit of measure for "how far around", so two points on the
/// same ring can be compared regardless of which of the four sides each sits on.
///
/// Every caller builds `p` sitting exactly on one side ([`ring_touch`]'s own contract), so which
/// side is unambiguous in practice; a point that is not (never produced by this module, but this
/// function makes no assumption it cannot happen) is assigned to whichever side it is nearest,
/// rather than panicking — the corner tie (equidistant from two sides) resolves to top/bottom over
/// left/right, an arbitrary but deterministic choice for a case no real caller reaches.
fn ring_perimeter_dist(ring: (f64, f64, f64, f64), p: &Point) -> f64 {
    let (l, t, r, b) = ring;
    let w = r - l;
    let h = b - t;
    let d_top = (p.y - t).abs();
    let d_bottom = (p.y - b).abs();
    let d_left = (p.x - l).abs();
    let d_right = (p.x - r).abs();
    let m = d_top.min(d_bottom).min(d_left).min(d_right);
    if m == d_top {
        (p.x - l).clamp(0.0, w)
    } else if m == d_right {
        w + (p.y - t).clamp(0.0, h)
    } else if m == d_bottom {
        w + h + (r - p.x).clamp(0.0, w)
    } else {
        w + h + w + (b - p.y).clamp(0.0, h)
    }
}

/// The ring's own four corners, each paired with its clockwise distance from the top-left one —
/// [`ring_perimeter_dist`]'s own scale, so [`corners_between_clockwise`] can place a corner
/// relative to any two points on the ring the same way it places those points themselves.
fn ring_corners(ring: (f64, f64, f64, f64)) -> [(f64, Point); 4] {
    let (l, t, r, b) = ring;
    let w = r - l;
    let h = b - t;
    [
        (0.0, Point::new(l, t)),
        (w, Point::new(r, t)),
        (w + h, Point::new(r, b)),
        (2.0 * w + h, Point::new(l, b)),
    ]
}

/// Every corner of `ring` that lies strictly between the points at perimeter-distance `from` and
/// `to`, travelling **clockwise** from `from` — in the order the path passes them (nearest first).
/// [`ring_path`]'s only real work: it calls this once each way and keeps whichever direction visits
/// fewer corners' worth of distance, so a perimeter edge takes the shorter arc round the rectangle
/// rather than always going one fixed way.
fn corners_between_clockwise(ring: (f64, f64, f64, f64), from: f64, to: f64) -> Vec<Point> {
    let w = ring.2 - ring.0;
    let h = ring.3 - ring.1;
    let perim = 2.0 * (w + h);
    if perim <= 0.0 {
        return Vec::new();
    }
    let span = (to - from).rem_euclid(perim);
    let mut out: Vec<(f64, Point)> = ring_corners(ring)
        .into_iter()
        .filter_map(|(dist, p)| {
            let rel = (dist - from).rem_euclid(perim);
            (rel > EPS && rel < span - EPS).then_some((rel, p))
        })
        .collect();
    out.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    out.into_iter().map(|(_, p)| p).collect()
}

/// The corners a route has to pass through to get from `a` to `b` along `ring`'s own boundary,
/// taking whichever of the two directions round the rectangle is shorter (ties broken clockwise) —
/// **not including `a` or `b` themselves**. Empty when `a` and `b` sit on the same side (the direct
/// segment between them is already along the boundary, `bridge`-free).
fn ring_path(ring: (f64, f64, f64, f64), a: &Point, b: &Point) -> Vec<Point> {
    let w = ring.2 - ring.0;
    let h = ring.3 - ring.1;
    let perim = 2.0 * (w + h);
    if perim <= 0.0 {
        return Vec::new();
    }
    let da = ring_perimeter_dist(ring, a);
    let db = ring_perimeter_dist(ring, b);
    let cw_span = (db - da).rem_euclid(perim);
    let ccw_span = perim - cw_span;
    if cw_span <= ccw_span {
        corners_between_clockwise(ring, da, db)
    } else {
        // The counter-clockwise path from `a` to `b` visits the same corners, in the same
        // physical order, as the clockwise path from `b` to `a` — just walked backwards, so the
        // list this builds has to be reversed to read in the `a`-to-`b` direction the caller
        // wants.
        let mut v = corners_between_clockwise(ring, db, da);
        v.reverse();
        v
    }
}

/// §10-1 item 4's perimeter lane: the route a back edge ([`EdgeShape::reverse`]) or a
/// collision-fallback forward edge ([`EdgeShape::staircase`]) draws (a self-loop excepted — see
/// [`route_with_ports`]'s own doc). Leaves/enters through the exact same ports every other shape
/// uses — `port_at` was already called by [`route_with_ports`], `source_port`/`target_port` are
/// its result — then walks out to `ring` ([`safe_ring_exit`], not a bare [`ring_touch`]: see that
/// function's own doc for why a straight run alone is not always safe) and around whichever way
/// ([`ring_path`]) is shorter to the same kind of exit at the other end. `blocked` is
/// `safe_ring_exit`'s own obstacle test, passed through unchanged.
///
/// No `direction` parameter, unlike every other `route_*` helper in this module: `ring_touch`/
/// `ring_path` reason in absolute `x`/`y`, not `flow`/`cross` — a rectangle's own boundary has no
/// "along the flow" to abstract away.
fn route_perimeter(
    shape: &EdgeShape,
    source_port: Point,
    target_port: Point,
    ring: (f64, f64, f64, f64),
    blocked: &dyn Fn(&Point, &Point) -> bool,
) -> Vec<Point> {
    let source_exit = safe_ring_exit(shape.source_side, &source_port, ring, blocked);
    let target_exit = safe_ring_exit(shape.target_side, &target_port, ring, blocked);
    let source_ring = source_exit
        .last()
        .cloned()
        .unwrap_or_else(|| source_port.clone());
    let target_ring = target_exit
        .last()
        .cloned()
        .unwrap_or_else(|| target_port.clone());

    let mut out = vec![source_port];
    out.extend(source_exit);
    out.extend(ring_path(ring, &source_ring, &target_ring));
    out.push(target_ring);
    out.extend(target_exit.into_iter().rev().skip(1));
    out.push(target_port);
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
    // No other nodes exist in this isolated helper, so there is nothing to collide with —
    // `shape_crosses_a_node` degrades harmlessly to "never crosses" over an empty slice.
    let shape = classify(
        direction,
        source,
        target,
        raw,
        source_rank,
        target_rank,
        source_out_degree,
        target_in_degree,
        &[],
    );
    let source_coord = face_center_coord(source, shape.source_side);
    let target_coord = face_center_coord(target, shape.target_side);
    // The only two nodes that exist in this isolated helper are the ring's own content — the same
    // "no siblings" simplification `classify`'s call above already leans on.
    let both = [source.clone(), target.clone()];
    let ring = expand_bounds(content_bounds(&both, &[]), PERIMETER_MARGIN);
    route_with_ports(
        direction,
        &shape,
        source,
        target,
        source_coord,
        target_coord,
        raw,
        ring,
        &both,
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

/// §10-1 item 4's 8px lane stagger: every perimeter-routed edge (`reverse`/`staircase`, minus a
/// self-loop — [`route_with_ports`]'s own doc) gets its own lane index, assigned in a stable order
/// (edge id, not declaration or `HashMap` iteration order) so the same source always draws the same
/// picture. [`route_flowchart`] turns a lane index into an actual ring (`PERIMETER_MARGIN +
/// lane * PERIMETER_LANE_SPACING` px out from [`content_bounds`]); `avoid_label_plates` calls this
/// a second time, after labels exist, so it can retry a leg that turned out to cross a plate
/// against the *exact* ring `route_flowchart` gave that edge — the two must never disagree about
/// which lane an edge sits on, so both read it from here rather than each keeping their own count.
fn perimeter_lanes<'a>(
    by_id: &HashMap<&str, &PlacedNode>,
    edges: &[EligibleEdge<'a>],
    shapes: &[Option<EdgeShape>],
) -> HashMap<&'a str, usize> {
    let mut ids: Vec<&str> = edges
        .iter()
        .zip(shapes)
        .filter_map(|(e, s)| {
            let s = s.as_ref()?;
            let (Some(&source), Some(&target)) = (by_id.get(e.source), by_id.get(e.target)) else {
                return None;
            };
            ((s.reverse || s.staircase) && source.id != target.id).then_some(e.id)
        })
        .collect();
    ids.sort_unstable();
    ids.into_iter().enumerate().map(|(i, id)| (id, i)).collect()
}

/// Routes every node-to-node edge in one flowchart under `[ui] mermaid_routing =
/// "konoma-orthogonal"`, running [`classify`] and [`evict`] once each over the whole diagram
/// before building any polyline — the two-pass shape the module doc describes.
///
/// `nodes` is the diagram's placed nodes from *this* layout pass; `clusters` is every subgraph
/// frame from the same pass, needed only for [`content_bounds`] (§10-1 item 4's perimeter lane has
/// to clear a frame the way it clears a node, even though a frame is never itself a routable
/// endpoint here — see `edges` below); `edges` is every drawable edge whose two ends are both real
/// nodes (a cluster boundary never reaches this function — it is out of scope until §10-2's later
/// stages, and keeps drawing through the ordinary spline path, `lay_out_spec`'s own filter on
/// `edges::End::Node`).
pub fn route_flowchart(
    direction: Direction,
    nodes: &[PlacedNode],
    clusters: &[PlacedCluster],
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
                nodes,
            ))
        })
        .collect();

    let eviction = evict(direction, &by_id, edges, &shapes);

    let base_bounds = content_bounds(nodes, clusters);
    let lane_of = perimeter_lanes(&by_id, edges, &shapes);

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
        let ring = match lane_of.get(edge.id) {
            Some(&lane) => expand_bounds(
                base_bounds,
                PERIMETER_MARGIN + lane as f64 * PERIMETER_LANE_SPACING,
            ),
            // Not a perimeter edge (or a self-loop) — `route_with_ports` never reads `ring` for
            // any other shape, so this value is never actually used; built anyway so the call
            // below has one signature for every edge.
            None => base_bounds,
        };
        let routed = route_with_ports(
            direction,
            shape,
            source,
            target,
            source_coord,
            target_coord,
            edge.raw,
            ring,
            nodes,
        );
        points.insert(edge.id.to_string(), routed);
    }

    RoutedFlowchart {
        points,
        required_size: eviction.required_size,
    }
}

// -------------------------------------------------------------------------------------------
// §10-1 item 3: labels always sit on a straight run
// -------------------------------------------------------------------------------------------

/// The clearance §10-1 item 3 asks for on each side of a labelled segment: "両側各16px（線9px＋
/// 矢尻7px）". Reused, not a coincidence with [`PORT_SPACING`]'s own 16px — the two rules just
/// happen to pick the same number — kept as its own constant because the two mean different
/// things ("how far apart two ports sit" vs "how much clearance a label plate needs") and a future
/// change to one must not silently move the other.
pub const LABEL_CLEARANCE: f64 = 16.0;

/// One edge's label, resting on its own routed line — [`label_slot`]'s result.
pub struct LabelSlot {
    /// Where the label's plate should be centred — the midpoint of the segment it was snapped to.
    pub center: Point,
    /// That segment's own length, in px — what a minimum-length check compares against.
    pub length: f64,
    /// Whether the chosen segment runs along the flow axis (the one [`lay_out_spec_pass`]'s
    /// `EdgeLabel` width/height can actually grow — see [`super::lay_out_spec_pass`]'s own doc on
    /// how a label's flow-axis dimension already asks dagre for extra rank space). `false` means
    /// the label had to fall back to a cross-axis segment (only possible for a `reverse`/
    /// `staircase` edge with no flow-axis leg of its own), where no minimum-length mechanism
    /// exists yet — a documented gap, not silently "handled".
    pub is_flow_axis: bool,
    /// Whether the chosen segment runs left-right (`x` varies) rather than top-bottom. This is
    /// what decides which of the plate's own two dimensions has to fit inside `length`: width for
    /// a horizontal segment (text reads along it), height for a vertical one (text runs crosswise,
    /// so only the plate's short dimension needs headroom along the line).
    pub horizontal: bool,
}

/// Whether `a`–`b` runs top-to-bottom rather than left-to-right. Every segment this module builds
/// is axis-parallel (the invariant `orthogonal_routing_draws_only_axis_parallel_segments` checks),
/// so exactly one of the two coordinates changes; a genuinely diagonal pair (never produced) reads
/// as vertical, matching [`segment_crosses_node`]'s own "diagonal is a non-crossing no-op" spirit.
fn segment_is_vertical(a: &Point, b: &Point) -> bool {
    (a.x - b.x).abs() < EPS
}

/// §10-1 item 3's "ラベルは常に線上プレート": picks which segment of a routed edge's polyline a
/// label's plate should sit on, and where on it.
///
/// The rule (`docs/FEATURE-MERMAID-RENDERER.md` §10-1 item 3, konoma's own restatement in the
/// module handoff): "LR は水平区間があればそこ、なければ垂直区間。TB は逆" — generalised past the
/// two directions actually named the way [`flow`]/[`cross`] generalise every other formula here:
/// prefer whichever segment runs along the **flow** axis (horizontal for `LR`/`RL`, vertical for
/// `TD`/`BT`), and within a class of segments prefer the longest one, so a label gets the most
/// headroom the route actually offers. A flow-axis segment exists for every `aligned`/`branch`/
/// `merge` shape (see this function's own tests) — [`classify`]'s own doc walks through why each
/// one always has exactly one — so the fallback to a cross-axis segment is reached only by a
/// `reverse`/`staircase` edge whose dagre-waypoint chain happens to have no flow-axis leg at all.
///
/// `None` only for a degenerate zero/one-point polyline, which [`route_with_ports`] never actually
/// returns (its own doc: two distinct nodes cannot collapse onto the same point).
pub fn label_slot(direction: Direction, points: &[Point]) -> Option<LabelSlot> {
    if points.len() < 2 {
        return points.first().map(|p| LabelSlot {
            center: p.clone(),
            length: 0.0,
            is_flow_axis: false,
            horizontal: true,
        });
    }
    let flow_is_vertical = matches!(direction, Direction::TopToBottom | Direction::BottomToTop);
    // (window index, length, is_flow_axis) of the best candidate seen so far.
    let mut best: Option<(usize, f64, bool)> = None;
    for (i, w) in points.windows(2).enumerate() {
        let (a, b) = (&w[0], &w[1]);
        let vertical = segment_is_vertical(a, b);
        let len = (b.x - a.x).hypot(b.y - a.y);
        let is_flow = vertical == flow_is_vertical;
        let better = match best {
            None => true,
            // A flow-axis segment always outranks a cross-axis one, regardless of length — that
            // is the whole point of preferring it (only a flow-axis segment's length is ever
            // grown by a label's own size, so a longer cross-axis segment is not actually a
            // safer bet). Within the same class, the longer one wins.
            Some((_, best_len, best_is_flow)) => {
                if is_flow != best_is_flow {
                    is_flow
                } else {
                    len > best_len
                }
            }
        };
        if better {
            best = Some((i, len, is_flow));
        }
    }
    let (i, len, is_flow) = best?;
    let (a, b) = (&points[i], &points[i + 1]);
    Some(LabelSlot {
        center: Point::new((a.x + b.x) / 2.0, (a.y + b.y) / 2.0),
        length: len,
        is_flow_axis: is_flow,
        horizontal: !segment_is_vertical(a, b),
    })
}

/// The minimum length a labelled segment needs — §10-1 item 3: "ラベル付き区間の最低長 = ラベル幅
/// （縦区間はプレート高14px）＋両側各16px". `plate_size` is the drawn plate's own box (text plus
/// padding, [`super::LABEL_PAD_X`]/[`super::LABEL_PAD_Y`] already folded in by the caller); which
/// of its two dimensions has to fit is [`LabelSlot::horizontal`]'s call, not `direction`'s — a
/// label can, in principle, still land on a cross-axis segment (see [`label_slot`]'s own doc).
pub fn label_min_length(plate_size: Size, horizontal: bool) -> f64 {
    (if horizontal {
        plate_size.w
    } else {
        plate_size.h
    }) + 2.0 * LABEL_CLEARANCE
}

/// `node`'s own half-extent along the cross axis — `w/2` for `TD`/`BT` (whose cross axis is x),
/// `h/2` for `LR`/`RL` (whose cross axis is y). What [`align_straight_lanes`]'s overlap-resolution
/// sweep pads a gap by, on each side, and the same quantity dagre's own `nodesep` already spaces
/// same-rank nodes apart by.
fn cross_extent(direction: Direction, node: &PlacedNode) -> f64 {
    match direction {
        Direction::TopToBottom | Direction::BottomToTop => node.size.w / 2.0,
        Direction::LeftToRight | Direction::RightToLeft => node.size.h / 2.0,
    }
}

/// §10-1 item 2's "レーン揃え": greedily selects a maximal set of node-disjoint "straight lane"
/// edges between *adjacent* ranks, then slides every node in each resulting chain onto one shared
/// cross coordinate — after which [`classify`]'s existing `aligned` check (unchanged) recognises
/// the chain's edges on its own, the same way an edge that already happened to line up under
/// stage 1/2 was recognised, and draws them dead straight.
///
/// Mutates `nodes` in place and is meant to run once, right after a layout pass places them and
/// before anything downstream (frames, `route_flowchart`) reads a position from them — see the
/// module doc's "Lane alignment moves nodes" section for why the order matters.
///
/// `node_rank` is dagre's own rank per node id; `candidates` is every edge eligible to be
/// considered for a lane — the caller's job to have already excluded a cluster-anchored edge (one
/// whose written endpoint is a block, not the real node dagre routed against), since this module
/// has no opinion of its own about clusters.
pub fn align_straight_lanes(
    direction: Direction,
    nodes: &mut [PlacedNode],
    node_rank: &HashMap<String, i32>,
    candidates: &[(String, String)],
) {
    if nodes.len() < 2 {
        return;
    }
    let id_index: HashMap<String, usize> = nodes
        .iter()
        .enumerate()
        .map(|(i, n)| (n.id.clone(), i))
        .collect();

    // Every rank's member ids, in their ORIGINAL cross-axis order — captured before this function
    // moves anything, and never resorted afterwards: §10-1 item 1's "ランク内の並び順は変えない"
    // applies just as much to a lane-aligned rank as it did to a grown one.
    let mut by_rank: HashMap<i32, Vec<usize>> = HashMap::new();
    for (id, &rank) in node_rank {
        if let Some(&i) = id_index.get(id) {
            by_rank.entry(rank).or_default().push(i);
        }
    }
    for ids in by_rank.values_mut() {
        ids.sort_by(|&a, &b| {
            cross(direction, &nodes[a].center)
                .partial_cmp(&cross(direction, &nodes[b].center))
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| nodes[a].id.cmp(&nodes[b].id))
        });
    }

    // --- greedy straight-lane selection, rank pair by rank pair, in rank order -------------------
    //
    // "各ノード高々1入1出" (at most one selected outgoing / incoming edge per node) makes the
    // selected edges a set of node-disjoint simple paths by construction — no node ever has two
    // selected successors or two selected predecessors, so following `next` from any node that
    // is not itself somebody's selected successor walks exactly one chain to its end.
    let mut used_out: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut used_in: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut next: HashMap<String, String> = HashMap::new();

    let mut ranks: Vec<i32> = node_rank.values().copied().collect();
    ranks.sort_unstable();
    ranks.dedup();
    for &r in &ranks {
        let mut pair_candidates: Vec<&(String, String)> = candidates
            .iter()
            .filter(|(s, t)| node_rank.get(s) == Some(&r) && node_rank.get(t) == Some(&(r + 1)))
            .collect();
        // "タイは上・左優先＝cross座標の小さい方" — sort by the source's own cross coordinate
        // first (so the topmost/leftmost source is offered its pick first), then the target's,
        // then by id for a fully deterministic order a `HashMap`-built candidate list would not
        // otherwise have.
        pair_candidates.sort_by(|(s1, t1), (s2, t2)| {
            let sc1 = cross(direction, &nodes[id_index[s1]].center);
            let sc2 = cross(direction, &nodes[id_index[s2]].center);
            sc1.partial_cmp(&sc2)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| {
                    let tc1 = cross(direction, &nodes[id_index[t1]].center);
                    let tc2 = cross(direction, &nodes[id_index[t2]].center);
                    tc1.partial_cmp(&tc2).unwrap_or(std::cmp::Ordering::Equal)
                })
                .then_with(|| s1.cmp(s2))
                .then_with(|| t1.cmp(t2))
        });
        for (s, t) in pair_candidates {
            if used_out.contains(s) || used_in.contains(t) {
                continue;
            }
            used_out.insert(s.clone());
            used_in.insert(t.clone());
            next.insert(s.clone(), t.clone());
        }
    }

    // --- build chains: a head is a selected source that is nobody's selected target ------------
    let mut chains: Vec<Vec<String>> = Vec::new();
    for s in &used_out {
        if used_in.contains(s) {
            continue; // not a head — some other selected edge already leads into it
        }
        let mut chain = vec![s.clone()];
        let mut cur = s.clone();
        while let Some(nxt) = next.get(&cur) {
            chain.push(nxt.clone());
            cur = nxt.clone();
        }
        chains.push(chain);
    }

    // --- align: every chain member's cross coordinate becomes the chain's own average -----------
    //
    // Chains are node-disjoint, so this can never read one chain's *already-moved* position while
    // computing another's average — every member's `center` here is still exactly where dagre (or
    // this pass's own growth retry) put it.
    for chain in &chains {
        if chain.len() < 2 {
            continue;
        }
        let avg: f64 = chain
            .iter()
            .map(|id| cross(direction, &nodes[id_index[id]].center))
            .sum::<f64>()
            / chain.len() as f64;
        for id in chain {
            let i = id_index[id];
            let flow_v = flow(direction, &nodes[i].center);
            nodes[i].center = make(direction, flow_v, avg);
        }
    }

    // --- resolve overlaps: one forward sweep per rank, in the fixed original order --------------
    //
    // §10-1 item 1's "重なった側を押し出して従来の最小間隔を維持する（ランク内の並び順は変えない）"
    // — `NODE_SEP` is dagre's own `nodesep`, reused so a push respects the same gap the rank was
    // laid out with in the first place, whether or not either node in a pair moved at all.
    for ids in by_rank.values() {
        let mut prev_far_edge: Option<f64> = None;
        for &i in ids {
            let half = cross_extent(direction, &nodes[i]);
            let mut c = cross(direction, &nodes[i].center);
            if let Some(prev_edge) = prev_far_edge {
                let min_c = prev_edge + super::NODE_SEP + half;
                if c < min_c {
                    c = min_c;
                    let flow_v = flow(direction, &nodes[i].center);
                    nodes[i].center = make(direction, flow_v, c);
                }
            }
            prev_far_edge = Some(c + half);
        }
    }
}

/// `side`'s tangent coordinate of a point already known to be a port on that face — `p.x` for
/// `Top`/`Bottom`, `p.y` for `Left`/`Right`. [`face_center_coord`]'s counterpart for a point
/// instead of a node, which is what [`avoid_label_plates`] has (a routed polyline's endpoint) where
/// it does not have eviction's own offset bookkeeping any more.
fn tangent_coord(side: Side, p: &Point) -> f64 {
    match side {
        Side::Top | Side::Bottom => p.x,
        Side::Left | Side::Right => p.y,
    }
}

/// `cur`, pushed [`PORT_SPACING`] further from `node`'s own `side` face centre — §10-1 item 3:
/// "ポートをさらに16px外へ". "Further" means away from the centre in whichever direction the port
/// already sat on (or, for a port that started exactly centred, outward in the positive tangent
/// direction — the spec does not disambiguate a port with no existing side to keep, and this stays
/// deterministic rather than arbitrary-per-call).
fn push_outward(node: &PlacedNode, side: Side, cur: f64) -> f64 {
    let center = face_center_coord(node, side);
    let dir = if cur >= center { 1.0 } else { -1.0 };
    cur + dir * PORT_SPACING
}

/// Whether the axis-parallel segment `a`–`b` crosses `plate`'s box — [`segment_crosses_node`]'s own
/// arithmetic, restated against a label's plate rectangle instead of a node's. No collision margin
/// here: item 3 is about a lane running *through* the plate, not grazing its edge, and a label
/// plate (unlike a node) has no stroke of its own for a margin to account for.
fn segment_crosses_plate(a: &Point, b: &Point, plate: &PlacedEdgeLabel) -> bool {
    let (l, t, r, bo) = (
        plate.center.x - plate.size.w / 2.0,
        plate.center.y - plate.size.h / 2.0,
        plate.center.x + plate.size.w / 2.0,
        plate.center.y + plate.size.h / 2.0,
    );
    if (a.y - b.y).abs() < EPS {
        let y = a.y;
        let (x0, x1) = (a.x.min(b.x), a.x.max(b.x));
        y >= t && y <= bo && x1 >= l && x0 <= r
    } else if (a.x - b.x).abs() < EPS {
        let x = a.x;
        let (y0, y1) = (a.y.min(b.y), a.y.max(b.y));
        x >= l && x <= r && y1 >= t && y0 <= bo
    } else {
        false
    }
}

/// §10-1 item 3: "ポートの垂直レーンがラベルプレートと重なる場合はポートをさらに16px外へ". Two
/// different fixes, by shape:
///
/// * A branch/merge/aligned edge: checks its two port-adjacent stub segments (`points[0]-points[1]`
///   at the source end, the last pair at the target end — exactly where stage 2's evicted, 16px
///   apart lanes run close together right next to a busy face) against every *other* edge's label
///   plate; a crossing pushes that one port [`PORT_SPACING`] further out and rebuilds just that
///   edge's polyline from it ([`route_with_ports`]).
/// * A perimeter-routed edge ([`EdgeShape::reverse`]/[`EdgeShape::staircase`], minus a self-loop):
///   has no single port to push — its whole route can run near a plate anywhere along its length,
///   not only right next to a node (`cjk`'s `D->A` swept straight through `B->D`'s own `テキスト`
///   plate, found by dumping a real routed diagram). So instead: check every segment of its already
///   -built route; if any crosses a foreign plate, rebuild it with [`route_perimeter`] again, this
///   time against a `blocked` test that includes every foreign plate as well as every node — the
///   same `safe_ring_exit` candidate search [`route_flowchart`] already ran, just given one more
///   kind of obstacle to avoid, using the identical ring [`perimeter_lanes`] already assigned it (so
///   this second pass can never disagree with the first about which lane the edge sits on). A
///   self-loop is left untouched, the same as [`route_with_ports`]'s own doc explains for why it
///   never reaches [`route_perimeter`] in the first place.
///
/// A single forward pass over `edges`, in caller order — not a fixpoint search. Fixing one edge can
/// in principle open a fresh crossing against a plate it did not use to reach, but that needs a
/// second plate to already sit within one more lane-width (or ring-candidate) of the first — a
/// documented approximation rather than a proven fixpoint, the same honesty
/// [`route_staircase_with_ports`]'s own doc gives the back-edge route it stands in for.
///
/// Mutates `points` for any edge whose route changed, and `plates` for that same edge's own label
/// (if it carries one) so the plate the *next* edge in this same pass checks against is the one
/// that will actually be drawn, not the one from before the fix.
pub fn avoid_label_plates(
    direction: Direction,
    nodes: &[PlacedNode],
    clusters: &[PlacedCluster],
    edges: &[EligibleEdge],
    points: &mut HashMap<String, Vec<Point>>,
    plates: &mut HashMap<String, PlacedEdgeLabel>,
) {
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
                nodes,
            ))
        })
        .collect();
    let base_bounds = content_bounds(nodes, clusters);
    let lane_of = perimeter_lanes(&by_id, edges, &shapes);

    for (edge, shape) in edges.iter().zip(&shapes) {
        let Some(shape) = shape else { continue };
        let (Some(&source), Some(&target)) = (by_id.get(edge.source), by_id.get(edge.target))
        else {
            continue;
        };

        if (shape.reverse || shape.staircase) && source.id != target.id {
            let Some(pts) = points.get(edge.id) else {
                continue;
            };
            if pts.len() < 2 {
                continue;
            }
            let crosses_a_plate = pts.windows(2).any(|w| {
                plates
                    .iter()
                    .any(|(id, plate)| id != edge.id && segment_crosses_plate(&w[0], &w[1], plate))
            });
            if !crosses_a_plate {
                continue;
            }
            let Some(&lane) = lane_of.get(edge.id) else {
                continue; // defensive: every edge that reached this branch is in `lane_of`
            };
            let ring = expand_bounds(
                base_bounds,
                PERIMETER_MARGIN + lane as f64 * PERIMETER_LANE_SPACING,
            );
            let source_port = pts[0].clone();
            let target_port = pts[pts.len() - 1].clone();
            let ids = (edge.source, edge.target);
            let blocked = |a: &Point, b: &Point| {
                segment_crosses_any_node(a, b, nodes, ids)
                    || plates
                        .iter()
                        .any(|(id, plate)| id != edge.id && segment_crosses_plate(a, b, plate))
            };
            let rebuilt = route_perimeter(shape, source_port, target_port, ring, &blocked);
            if let Some(plate) = plates.get_mut(edge.id) {
                if let Some(slot) = label_slot(direction, &rebuilt) {
                    plate.center = slot.center;
                }
            }
            points.insert(edge.id.to_string(), rebuilt);
            continue;
        }
        if shape.reverse || shape.staircase {
            continue; // a self-loop — `route_with_ports`'s own doc.
        }

        let Some(pts) = points.get(edge.id).cloned() else {
            continue;
        };
        if pts.len() < 2 {
            continue;
        }
        let n = pts.len();

        let crosses_foreign = |a: &Point, b: &Point| {
            plates
                .iter()
                .any(|(id, plate)| id != edge.id && segment_crosses_plate(a, b, plate))
        };

        let mut new_source_coord = None;
        if crosses_foreign(&pts[0], &pts[1]) {
            let cur = tangent_coord(shape.source_side, &pts[0]);
            new_source_coord = Some(push_outward(source, shape.source_side, cur));
        }
        let mut new_target_coord = None;
        if crosses_foreign(&pts[n - 2], &pts[n - 1]) {
            let cur = tangent_coord(shape.target_side, &pts[n - 1]);
            new_target_coord = Some(push_outward(target, shape.target_side, cur));
        }
        if new_source_coord.is_none() && new_target_coord.is_none() {
            continue;
        }

        let source_coord =
            new_source_coord.unwrap_or_else(|| tangent_coord(shape.source_side, &pts[0]));
        let target_coord =
            new_target_coord.unwrap_or_else(|| tangent_coord(shape.target_side, &pts[n - 1]));
        let rebuilt = route_with_ports(
            direction,
            shape,
            source,
            target,
            source_coord,
            target_coord,
            edge.raw,
            // Neither `ring` nor `nodes` is ever read here: the shape checks above already ruled
            // out every shape that touches them (`route_with_ports`'s own doc — only a
            // non-self-loop `reverse`/`staircase` edge ever does).
            (0.0, 0.0, 0.0, 0.0),
            nodes,
        );
        if let Some(plate) = plates.get_mut(edge.id) {
            if let Some(slot) = label_slot(direction, &rebuilt) {
                plate.center = slot.center;
            }
        }
        points.insert(edge.id.to_string(), rebuilt);
    }
}

// -------------------------------------------------------------------------------------------
// §10-1 item 4's 12px crossing gap
// -------------------------------------------------------------------------------------------

/// How wide the omitted stretch is on the spanning side of an edge-to-edge crossing — §10-1 item
/// 4: "辺どうしの交差では跨ぐ側…の線に12pxの隙間を開ける". [`PlacedEdge::gaps`]'s own doc has the
/// full picture of what a gap is and who reads it.
///
/// [`PlacedEdge::gaps`]: super::PlacedEdge::gaps
pub const CROSSING_GAP: f64 = 12.0;

/// The point where axis-parallel segments `a` and `b` genuinely cross — one vertical, the other
/// horizontal, meeting strictly inside both (not merely touching at a shared endpoint, and not
/// running parallel or collinear with each other). `None` for every other case, defensively
/// including a segment that is not axis-parallel (never built by this module).
fn segment_crossing(a: &[Point], b: &[Point]) -> Option<Point> {
    let (a0, a1) = (&a[0], &a[1]);
    let (b0, b1) = (&b[0], &b[1]);
    let a_vert = (a0.x - a1.x).abs() < EPS;
    let b_horiz = (b0.y - b1.y).abs() < EPS;
    if a_vert && b_horiz {
        let x = a0.x;
        let (ay0, ay1) = (a0.y.min(a1.y), a0.y.max(a1.y));
        let y = b0.y;
        let (bx0, bx1) = (b0.x.min(b1.x), b0.x.max(b1.x));
        if x > bx0 + EPS && x < bx1 - EPS && y > ay0 + EPS && y < ay1 - EPS {
            return Some(Point::new(x, y));
        }
        return None;
    }
    let a_horiz = (a0.y - a1.y).abs() < EPS;
    let b_vert = (b0.x - b1.x).abs() < EPS;
    if a_horiz && b_vert {
        return segment_crossing(b, a);
    }
    None
}

/// The `CROSSING_GAP`-px interval on `seg` centred on `cross` — [`segment_crossing`]'s own result,
/// turned into the two points [`edges::split_at_gaps`] cuts out. Clamped to `seg`'s own two
/// endpoints, so a segment shorter than the gap itself (never produced by a real diagram — a
/// crossing this close to a port would already have tripped `safe_ring_exit`'s own collision
/// check, since a segment that short has almost no room to begin with) still returns *something*
/// rather than a point outside the line it is meant to interrupt.
fn gap_around(seg: &[Point], cross: Point) -> (Point, Point) {
    let (a, b) = (&seg[0], &seg[1]);
    let half = CROSSING_GAP / 2.0;
    if (a.x - b.x).abs() < EPS {
        let (lo, hi) = (a.y.min(b.y), a.y.max(b.y));
        let y0 = (cross.y - half).clamp(lo, hi);
        let y1 = (cross.y + half).clamp(lo, hi);
        (Point::new(cross.x, y0), Point::new(cross.x, y1))
    } else {
        let (lo, hi) = (a.x.min(b.x), a.x.max(b.x));
        let x0 = (cross.x - half).clamp(lo, hi);
        let x1 = (cross.x + half).clamp(lo, hi);
        (Point::new(x0, cross.y), Point::new(x1, cross.y))
    }
}

/// §10-1 item 4: "辺どうしの交差では跨ぐ側（外周レーンの辺＝戻り辺・補助辺）の線に12pxの隙間を開け
/// る（下をくぐる線は連続のまま）". Finds every point where two *different* edges' segments
/// genuinely cross ([`segment_crossing`]), and — when exactly one of the two is a perimeter-routed
/// edge (`reverse`/`staircase`, minus a self-loop — [`route_with_ports`]'s own doc explains why a
/// self-loop is exempt from the whole perimeter mechanism) — cuts a [`CROSSING_GAP`]-px gap into
/// that edge's own line, leaving the edge it crosses completely untouched: §10-1's "下をくぐる線は
/// 連続のまま" read as depth, not as a rule about which edge is which — the interrupted line is
/// the one that visually dips under, the untouched one reads as passing over it.
///
/// When *both* sides of a crossing are perimeter edges (reachable — `amp-chain`'s own
/// collision-fallback `A->D` and `B->C` cross each other, neither a back edge nor sharing a node
/// with the other, dumped and confirmed before this was written), the spec does not say which one
/// gets the gap; the tie is broken by edge id, lower id kept whole, deterministically rather than
/// arbitrarily.
///
/// Node crossings and frame crossings are both out of scope here by construction: this only ever
/// compares two edges' own segments against each other, never against a node's or a cluster's
/// box — §10-1's own "辺と枠の交差は隙間なし（直交して跨ぐだけ）" is exactly what leaving a frame
/// out of this function achieves.
///
/// `points` is the *final* routed polyline per edge id — `route_flowchart`'s own result after
/// `avoid_label_plates` has already run, so a gap is never computed against a route stage 5 was
/// about to move out from under it.
pub fn insert_crossing_gaps(
    direction: Direction,
    nodes: &[PlacedNode],
    edges: &[EligibleEdge],
    points: &HashMap<String, Vec<Point>>,
) -> HashMap<String, Vec<(Point, Point)>> {
    let by_id: HashMap<&str, &PlacedNode> = nodes.iter().map(|n| (n.id.as_str(), n)).collect();
    let is_perimeter: HashMap<&str, bool> = edges
        .iter()
        .filter_map(|e| {
            let (Some(&source), Some(&target)) = (by_id.get(e.source), by_id.get(e.target)) else {
                return None;
            };
            let shape = classify(
                direction,
                source,
                target,
                e.raw,
                e.source_rank,
                e.target_rank,
                e.source_out_degree,
                e.target_in_degree,
                nodes,
            );
            Some((
                e.id,
                (shape.reverse || shape.staircase) && source.id != target.id,
            ))
        })
        .collect();

    let mut gaps: HashMap<String, Vec<(Point, Point)>> = HashMap::new();
    for i in 0..edges.len() {
        for j in (i + 1)..edges.len() {
            let (ei, ej) = (edges[i].id, edges[j].id);
            let (Some(pi), Some(pj)) = (points.get(ei), points.get(ej)) else {
                continue;
            };
            let (pi_perim, pj_perim) = (
                is_perimeter.get(ei).copied().unwrap_or(false),
                is_perimeter.get(ej).copied().unwrap_or(false),
            );
            if !pi_perim && !pj_perim {
                continue; // neither side spans — out of this rule's scope.
            }
            // Both spanning: the higher edge id is the one that gets cut, so the lower one stays
            // whole — this function's own doc explains why either choice is equally arbitrary.
            let cut_on_i = if pi_perim && pj_perim {
                ei > ej
            } else {
                pi_perim
            };
            let (cut_id, cut_pts, other_pts) = if cut_on_i { (ei, pi, pj) } else { (ej, pj, pi) };
            for wc in cut_pts.windows(2) {
                for wo in other_pts.windows(2) {
                    if let Some(cross) = segment_crossing(wc, wo) {
                        let (g0, g1) = gap_around(wc, cross);
                        gaps.entry(cut_id.to_string()).or_default().push((g0, g1));
                    }
                }
            }
        }
    }
    gaps
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

    // --- stage 4: label placement --------------------------------------------------------------

    #[test]
    fn label_slot_prefers_the_flow_axis_segment_even_when_a_cross_axis_one_is_longer() {
        // A hand-built polyline rather than a routed one: a short flow-axis (vertical, TD) leg
        // of only 10px, followed by a much longer cross-axis (horizontal) leg of 200px. §10-1
        // item 3's own rule ("LR は水平区間があればそこ、なければ垂直区間。TB は逆") asks for the
        // flow-axis segment whenever one exists, regardless of which one is longer — the whole
        // reason being that only a flow-axis segment's length is ever grown by
        // `lay_out_spec_pass`'s `label_boosts` (a longer cross-axis segment is not a safer bet,
        // because nothing can make it any longer if a label does not fit).
        let pts = vec![
            Point::new(0.0, 0.0),
            Point::new(0.0, 10.0),
            Point::new(200.0, 10.0),
        ];
        let slot = label_slot(Direction::TopToBottom, &pts).expect("2+ points must return a slot");
        assert!(
            slot.is_flow_axis,
            "{slot:?}",
            slot = (slot.center, slot.length)
        );
        assert!(
            (slot.length - 10.0).abs() < 1e-9,
            "must pick the 10px flow-axis leg, not the 200px cross-axis one: {}",
            slot.length
        );
        assert!((slot.center.x - 0.0).abs() < 1e-9 && (slot.center.y - 5.0).abs() < 1e-9);
    }

    #[test]
    fn label_slot_picks_the_longer_of_two_flow_axis_segments() {
        // Both legs run along the flow axis (vertical, TD) — the shape an aligned edge whose two
        // ends landed on different eviction coordinates draws (`bridge`'s own doc, "same axis,
        // other coordinate differs"). The second, longer leg must win.
        let pts = vec![
            Point::new(0.0, 0.0),
            Point::new(0.0, 30.0),
            Point::new(20.0, 30.0),
            Point::new(20.0, 130.0),
        ];
        let slot = label_slot(Direction::TopToBottom, &pts).expect("must return a slot");
        assert!(slot.is_flow_axis);
        assert!((slot.length - 100.0).abs() < 1e-9, "{}", slot.length);
        assert!((slot.center.x - 20.0).abs() < 1e-9 && (slot.center.y - 80.0).abs() < 1e-9);
    }

    #[test]
    fn label_slot_falls_back_to_a_cross_axis_segment_when_no_flow_axis_leg_exists() {
        // A pathological (never actually drawn by `route_with_ports`) all-cross-axis polyline —
        // stated anyway so the fallback branch is exercised directly rather than left unreached.
        let pts = vec![Point::new(0.0, 0.0), Point::new(50.0, 0.0)];
        let slot = label_slot(Direction::TopToBottom, &pts).expect("must return a slot");
        assert!(!slot.is_flow_axis);
        assert!(
            slot.horizontal,
            "a horizontal-only polyline must report horizontal=true"
        );
    }

    #[test]
    fn label_slot_handles_a_single_point_defensively() {
        let pts = vec![Point::new(5.0, 5.0)];
        let slot = label_slot(Direction::TopToBottom, &pts).expect("a single point is Some");
        assert_eq!(slot.length, 0.0);
        assert_eq!(slot.center, Point::new(5.0, 5.0));
    }

    #[test]
    fn label_min_length_uses_width_for_a_horizontal_segment_and_height_for_a_vertical_one() {
        let plate = Size::new(100.0, 20.0);
        assert!((label_min_length(plate, true) - (100.0 + 2.0 * LABEL_CLEARANCE)).abs() < 1e-9);
        assert!((label_min_length(plate, false) - (20.0 + 2.0 * LABEL_CLEARANCE)).abs() < 1e-9);
    }

    /// The real bug the coordinator's own verification of stage 5 item 1 found: `safe_ring_exit`'s
    /// L-shaped fallback used to add a third leg back out to `direct`'s own coordinate on the far
    /// side of the ring — landing exactly on a ring *corner* — instead of stopping the moment it
    /// touched the ring at all. Two edges (or, as here, one edge's two ends) that both fall back to
    /// the L on the *same* ring side then both land on that side's two far corners, and the "short
    /// way round" between two corners of one side is the *whole length of that side* — a `TD`
    /// diagram's real return edge drew the ring's full height (`docs/FEATURE-MERMAID-RENDERER.md`
    /// §10-1 item 4's own repro: `flowchart TB` with `C -->|再試行| B` looping a decision node back
    /// into itself) for a stub that only needed to span the ~46px between the two ports it actually
    /// connects.
    ///
    /// Pinned directly against [`safe_ring_exit`] rather than a full diagram, once for each `Side`
    /// (`Top`/`Bottom` covers `TD`/`BT`'s own exit faces, `Left`/`Right` covers `LR`/`RL`'s) — a
    /// diagram-level repro only happens to exist for `TD`; this is what the coordinator's own
    /// request ("既存テストが素通りした理由…も確認して同型の穴を塞ぐ") asks for: the *stage 1-4
    /// invariant suite never once called `safe_ring_exit` with a `blocked` closure that actually
    /// blocks anything* (every corpus source's `orthogonal_endpoints_sit_outside_the_node_...`
    /// check runs on the *finished* route, which only ever exercises whichever candidate won — the
    /// direct one, for nearly every corpus source), so a bug in the L-shaped candidate specifically
    /// had no route to being observed by anything already written.
    #[test]
    fn safe_ring_exit_l_shape_stops_at_the_ring_not_the_far_corner() {
        let ring = (0.0, 0.0, 600.0, 400.0);
        for side in [Side::Top, Side::Bottom, Side::Left, Side::Right] {
            let port = match side {
                Side::Top => Point::new(300.0, 50.0),
                Side::Bottom => Point::new(300.0, 350.0),
                Side::Left => Point::new(50.0, 200.0),
                Side::Right => Point::new(550.0, 200.0),
            };
            let direct = ring_touch(side, &port, ring);
            // Blocks exactly the direct candidate's single segment (`port` -> `direct`) and
            // nothing else, forcing the near-corner L to win.
            let blocked = |a: &Point, b: &Point| *a == port && *b == direct;
            let exit = safe_ring_exit(side, &port, ring, &blocked);
            assert_eq!(
                exit.len(),
                2,
                "{side:?}: the L-shaped fallback must be exactly [hop, ring-touch], not a third \
                 leg out to the far corner: {exit:?}"
            );
            let touch = &exit[1];
            match side {
                Side::Top | Side::Bottom => {
                    assert!(
                        (touch.y - exit[0].y).abs() < 1e-9,
                        "{side:?}: the ring touch point must stay at the hop's own y, not travel \
                         on to direct's ({}): {touch:?}",
                        direct.y
                    );
                }
                Side::Left | Side::Right => {
                    assert!(
                        (touch.x - exit[0].x).abs() < 1e-9,
                        "{side:?}: the ring touch point must stay at the hop's own x, not travel \
                         on to direct's ({}): {touch:?}",
                        direct.x
                    );
                }
            }
        }
    }

    /// The end-to-end version of the same fix, over all four `Direction`s: a decision node whose
    /// loop-back edge needs the L-shaped fallback at both ends (`raw` seeded with an interior
    /// waypoint that sits *inside* the sibling node on the direct path, so `classify`'s own
    /// collision test — unrelated to this bug, exercised here only to force the same fallback a
    /// real diagram's `shape_crosses_a_node` would) — the long run along the ring must be within a
    /// few px of the two ports' own separation, never the ring's full extent on that axis.
    #[test]
    fn perimeter_l_shape_fallback_spans_only_the_ports_own_separation_in_every_direction() {
        for direction in [
            Direction::TopToBottom,
            Direction::BottomToTop,
            Direction::LeftToRight,
            Direction::RightToLeft,
        ] {
            // A 3-node vertical (TD/BT) or horizontal (LR/RL) chain: `hi` -> `lo` normal, `lo` ->
            // `hi` reversed (the loop-back). `mid` sits directly between them on the same axis, so
            // the direct ring exit for both ends of `lo -> hi` would cut straight through it.
            let (hi, lo, mid) = match direction {
                Direction::TopToBottom | Direction::BottomToTop => (
                    node("hi", 100.0, 0.0, 60.0, 40.0),
                    node("lo", 100.0, 300.0, 60.0, 40.0),
                    node("mid", 100.0, 150.0, 60.0, 40.0),
                ),
                Direction::LeftToRight | Direction::RightToLeft => (
                    node("hi", 0.0, 100.0, 40.0, 60.0),
                    node("lo", 300.0, 100.0, 40.0, 60.0),
                    node("mid", 150.0, 100.0, 40.0, 60.0),
                ),
            };
            let nodes = vec![hi.clone(), lo.clone(), mid.clone()];
            let raw = vec![lo.center.clone(), mid.center.clone(), hi.center.clone()];
            let edges = vec![EligibleEdge {
                id: "back",
                source: "lo",
                target: "hi",
                raw: &raw,
                source_rank: Some(1),
                target_rank: Some(0),
                source_out_degree: 1,
                target_in_degree: 1,
            }];
            let routed = route_flowchart(direction, &nodes, &[], &edges);
            let pts = &routed.points["back"];
            for w in pts.windows(2) {
                let dx = (w[1].x - w[0].x).abs();
                let dy = (w[1].y - w[0].y).abs();
                assert!(
                    dx < 1e-9 || dy < 1e-9,
                    "{direction:?}: perimeter route must stay axis-parallel: {w:?}"
                );
            }
            // The long run along the ring is whichever segment is not adjacent to either port —
            // the two "hop" legs are short (`PORT_CLEARANCE`), so the longest segment is it.
            let longest = pts
                .windows(2)
                .map(|w| (w[1].x - w[0].x).hypot(w[1].y - w[0].y))
                .fold(0.0_f64, f64::max);
            // The two nodes' own separation on the flow axis, minus their half-extents — an upper
            // bound on how far apart the two ports genuinely are, generous enough to not depend on
            // exactly which face eviction picked.
            let needed = match direction {
                Direction::TopToBottom | Direction::BottomToTop => {
                    (lo.center.y - hi.center.y).abs()
                }
                Direction::LeftToRight | Direction::RightToLeft => {
                    (lo.center.x - hi.center.x).abs()
                }
            };
            assert!(
                longest <= needed + 1.0,
                "{direction:?}: the ring run is {longest}px, more than the ~{needed}px the two \
                 ports actually need — the far-corner bug is back: {pts:?}"
            );
        }
    }

    #[test]
    fn avoid_label_plates_ignores_a_self_loop() {
        // A self-loop starts and ends at the same box — `route_with_ports`'s own doc explains why
        // it never reaches `route_perimeter` at all, so `avoid_label_plates`'s perimeter branch
        // must leave it exactly as `route_flowchart` drew it, even when a plate genuinely covers
        // its whole loop.
        let a = node("A", 100.0, 100.0, 60.0, 40.0);
        let nodes = vec![a];
        let raw = vec![
            Point::new(130.0, 90.0),
            Point::new(160.0, 90.0),
            Point::new(160.0, 110.0),
            Point::new(130.0, 110.0),
        ];
        let edges = vec![EligibleEdge {
            id: "loop",
            source: "A",
            target: "A",
            raw: &raw,
            source_rank: Some(0),
            target_rank: Some(0),
            source_out_degree: 2,
            target_in_degree: 2,
        }];
        let routed = route_flowchart(Direction::TopToBottom, &nodes, &[], &edges);
        let mut points = routed.points;
        let before = points["loop"].clone();
        let mut plates: HashMap<String, PlacedEdgeLabel> = HashMap::new();
        plates.insert(
            "someone-elses-label".to_string(),
            PlacedEdgeLabel {
                center: Point::new(100.0, 100.0),
                size: Size::new(400.0, 400.0),
                label: Label::measure("x"),
            },
        );
        avoid_label_plates(
            Direction::TopToBottom,
            &nodes,
            &[],
            &edges,
            &mut points,
            &mut plates,
        );
        assert_eq!(points["loop"], before, "a self-loop must never be rebuilt");
    }

    #[test]
    fn avoid_label_plates_reroutes_a_perimeter_edge_off_a_plate_it_can_actually_clear() {
        // `cjk`'s real `D->A` bug, reproduced directly rather than through the whole mermaid
        // pipeline: `D`'s straight-up exit runs through open space between the ranks where some
        // *other* edge's label plate sits (`cjk`'s own case: `B->D`'s own "テキスト" plate) — no
        // node of its own to trip `route_flowchart`'s own node-avoidance, so `route_flowchart`
        // picks the direct run, and only `avoid_label_plates` — running after every label exists —
        // can see the plate at all. The plate does not cover the *ring* (unlike
        // `avoid_label_plates_ignores_a_self_loop`'s deliberately inescapable one), so the
        // L-shaped fallback candidate `safe_ring_exit` tries next actually clears it.
        let a = node("A", 100.0, 0.0, 60.0, 40.0);
        let d = node("D", 100.0, 200.0, 60.0, 40.0);
        let nodes = vec![a, d];
        let raw = vec![
            Point::new(100.0, 180.0),
            Point::new(100.0, 100.0),
            Point::new(100.0, 20.0),
        ];
        let edges = vec![EligibleEdge {
            id: "da",
            source: "D",
            target: "A",
            raw: &raw,
            // target rank <= source rank: a back edge.
            source_rank: Some(2),
            target_rank: Some(0),
            source_out_degree: 1,
            target_in_degree: 1,
        }];
        let routed = route_flowchart(Direction::TopToBottom, &nodes, &[], &edges);
        let mut points = routed.points;
        let mut plates: HashMap<String, PlacedEdgeLabel> = HashMap::new();
        // Sits in the open space between A and D, spanning D->A's straight-up column — narrow
        // enough that the ring itself (far outside both nodes) is still clear.
        plates.insert(
            "b-labels-edge".to_string(),
            PlacedEdgeLabel {
                center: Point::new(100.0, 100.0),
                size: Size::new(80.0, 20.0),
                label: Label::measure("x"),
            },
        );
        let crosses_plate = |pts: &[Point], plate: &PlacedEdgeLabel| {
            pts.windows(2)
                .any(|w| segment_crosses_plate(&w[0], &w[1], plate))
        };
        assert!(
            crosses_plate(&points["da"], &plates["b-labels-edge"]),
            "fixture must reproduce the bug before the fix runs: {:?}",
            points["da"]
        );

        avoid_label_plates(
            Direction::TopToBottom,
            &nodes,
            &[],
            &edges,
            &mut points,
            &mut plates,
        );

        assert!(
            !crosses_plate(&points["da"], &plates["b-labels-edge"]),
            "the rerouted line must actually clear the plate: {:?}",
            points["da"]
        );
        for w in points["da"].windows(2) {
            let dx = (w[1].x - w[0].x).abs();
            let dy = (w[1].y - w[0].y).abs();
            assert!(
                dx < 1e-9 || dy < 1e-9,
                "rerouted line must stay axis-parallel: {w:?}"
            );
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
        route_flowchart(direction, nodes, &[], edges)
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
        // X, Y and Z each branch into T (x=500) from above, none exactly matching T's own x (so
        // none is `aligned` by accident) — see `orthogonal_growth_widens_a_node_...`'s sibling
        // doc for a fuller explanation, but two things about these positions matter for stage 3's
        // collision fix (§10-1 item 1), not just stage 2's eviction math this test is actually
        // about: each source sits on its own row (y=50/150/250, comfortably more than a
        // box-height apart), so a branch's cross-axis sweep — which runs at the *source's own*
        // row — never grazes a sibling; and every source's x (50/950/1400) sits far from T's own
        // centre (500), because `classify`'s collision test runs at zero eviction offset, so
        // *every* candidate edge's vertical leg is tested landing at that same shared x during
        // classification, even though eviction later spreads their real endpoints 16px apart — a
        // sibling parked near that shared column (as an earlier draft's Y, at x=480, was) reads
        // as blocking every edge's vertical leg, not just its own.
        let target = node("T", 500.0, 300.0, 300.0, 60.0);
        let x = node("X", 50.0, 50.0, 60.0, 40.0);
        let y = node("Y", 950.0, 150.0, 60.0, 40.0);
        let z = node("Z", 1400.0, 250.0, 60.0, 40.0);
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

        // Order matches the sort key: X (cross=50) leftmost, Z (cross=1400) rightmost.
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
        // A, C and D each sit on their own row (see `three_incoming_edges_...`'s doc for why: at
        // one shared row, A itself — directly between C and D on the x axis — would sit in the
        // path of both C's and D's own cross-axis sweep and trip stage 3's collision fix).
        let a = node("A", 300.0, 0.0, 60.0, 40.0);
        let b = node("B", 300.0, 200.0, 60.0, 40.0);
        let c = node("C", 100.0, 70.0, 60.0, 40.0);
        let d = node("D", 500.0, 140.0, 60.0, 40.0);
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
        // Each source on its own row — see `three_incoming_edges_...`'s doc: at a shared row, Z's
        // sweep toward NARROW (x=0) would run right through Y's box sitting at x=225 in between.
        let narrow = node("NARROW", 0.0, 300.0, 20.0, 40.0);
        let roomy = node("ROOMY", 500.0, 300.0, 300.0, 40.0);
        let x = node("X", -50.0, 50.0, 40.0, 30.0);
        let y = node("Y", 225.0, 150.0, 40.0, 30.0);
        let z = node("Z", 490.0, 250.0, 40.0, 30.0);
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
        // Same spread-rows-and-columns fixture as `three_incoming_edges_...` — see its doc.
        let target = node("T", 500.0, 300.0, 300.0, 60.0);
        let x = node("X", 50.0, 50.0, 60.0, 40.0);
        let y = node("Y", 950.0, 150.0, 60.0, 40.0);
        let z = node("Z", 1400.0, 250.0, 60.0, 40.0);
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

    // --- stage 3: lane alignment --------------------------------------------------------------

    fn ranks(pairs: &[(&str, i32)]) -> HashMap<String, i32> {
        pairs.iter().map(|(id, r)| (id.to_string(), *r)).collect()
    }

    fn edge(source: &str, target: &str) -> (String, String) {
        (source.to_string(), target.to_string())
    }

    #[test]
    fn a_straight_chain_across_three_ranks_aligns_onto_the_average_cross_coordinate() {
        // A, B, C start at x = 0, 50, 100 (misaligned) across three adjacent ranks, one edge
        // apiece — a single node-disjoint chain. The average, 50, is where all three must end up;
        // B (already there) does not move, A and C do.
        let mut nodes = vec![
            node("A", 0.0, 0.0, 40.0, 30.0),
            node("B", 50.0, 100.0, 40.0, 30.0),
            node("C", 100.0, 200.0, 40.0, 30.0),
        ];
        let node_rank = ranks(&[("A", 0), ("B", 1), ("C", 2)]);
        let candidates = [edge("A", "B"), edge("B", "C")];
        align_straight_lanes(Direction::TopToBottom, &mut nodes, &node_rank, &candidates);

        for n in &nodes {
            assert!(
                (n.center.x - 50.0).abs() < 1e-9,
                "{}: expected x=50 (the chain's average), got {:?}",
                n.id,
                n.center
            );
            // The flow coordinate (rank position) must be untouched — only cross moves.
        }
        assert_eq!(nodes[0].center.y, 0.0);
        assert_eq!(nodes[1].center.y, 100.0);
        assert_eq!(nodes[2].center.y, 200.0);
    }

    #[test]
    fn tie_break_prefers_the_smaller_cross_coordinate() {
        // S1 (x=10) and S2 (x=500 — far enough that S1's own move below cannot possibly bring the
        // two within the minimum rank gap, which would otherwise entangle this test with the
        // separate overlap-resolution behaviour `overlap_resolution_pushes_...` already covers)
        // both offer T (x=50) a straight lane; only one can have it ("各ノード高々1入1出"). §10-1
        // item 1's "タイは上・左優先＝cross座標の小さい方" means S1 (the smaller cross coordinate)
        // wins — T ends up at (10+50)/2 = 30, not (500+50)/2 = 275 — and S2, having lost the tie,
        // is left exactly where it started.
        let mut nodes = vec![
            node("S1", 10.0, 0.0, 40.0, 30.0),
            node("S2", 500.0, 0.0, 40.0, 30.0),
            node("T", 50.0, 100.0, 40.0, 30.0),
        ];
        let node_rank = ranks(&[("S1", 0), ("S2", 0), ("T", 1)]);
        let candidates = [edge("S1", "T"), edge("S2", "T")];
        align_straight_lanes(Direction::TopToBottom, &mut nodes, &node_rank, &candidates);

        let by_id: HashMap<&str, &PlacedNode> = nodes.iter().map(|n| (n.id.as_str(), n)).collect();
        assert!(
            (by_id["T"].center.x - 30.0).abs() < 1e-9,
            "T must align with S1 (30), not S2 (275): {:?}",
            by_id["T"].center
        );
        assert_eq!(
            by_id["S2"].center.x, 500.0,
            "S2 lost the tie and must be left exactly where it started"
        );
    }

    #[test]
    fn overlap_resolution_pushes_the_later_node_without_reordering() {
        // P (chain average 100) and Q (chain average 155) are only 55px apart after their own
        // independent alignment — short of the `half(20) + NODE_SEP(50) + half(20) = 90px`
        // minimum. §10-1 item 1's "重なった側を押し出して従来の最小間隔を維持する（ランク内の並び
        // 順は変えない）": Q, which was already the later of the two in original rank order
        // (P at x=200, Q at x=210), is the one pushed — to exactly 190, not moved to swap places
        // with P.
        let mut nodes = vec![
            node("A", 0.0, 0.0, 40.0, 30.0),
            node("C", 100.0, 0.0, 40.0, 30.0),
            node("P", 200.0, 100.0, 40.0, 30.0),
            node("Q", 210.0, 100.0, 40.0, 30.0),
        ];
        let node_rank = ranks(&[("A", 0), ("C", 0), ("P", 1), ("Q", 1)]);
        let candidates = [edge("A", "P"), edge("C", "Q")];
        align_straight_lanes(Direction::TopToBottom, &mut nodes, &node_rank, &candidates);

        let by_id: HashMap<&str, &PlacedNode> = nodes.iter().map(|n| (n.id.as_str(), n)).collect();
        assert!(
            (by_id["P"].center.x - 100.0).abs() < 1e-9,
            "P is first in rank order and never needs pushing: {:?}",
            by_id["P"].center
        );
        assert!(
            (by_id["Q"].center.x - 190.0).abs() < 1e-9,
            "Q must be pushed to exactly P's far edge (120) + NODE_SEP(50) + Q's half-width(20) \
             = 190, not left at its own aligned 155: {:?}",
            by_id["Q"].center
        );
    }
}
