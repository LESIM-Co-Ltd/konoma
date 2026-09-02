//! `[ui] mermaid_routing = "konoma-orthogonal"` — konoma's own right-angle wiring mode for a
//! flowchart's edges.
//!
//! `docs/FEATURE-MERMAID-RENDERER.md` §10-1 is the confirmed spec (a Claude Design handoff). This
//! module implements the part of it that is **route, not layout**: dagre's ranking, ordering and
//! coordinates are untouched, and everything here only decides, for an edge whose ends are ordinary
//! nodes, subgraph frames, or one of each, which face of each box a line leaves and enters through,
//! at which exact point on that face, and how many right-angle bends connect the two.
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
//! # A back edge draws from a perimeter lane; a collision-fallback forward edge stays local
//!
//! §10-1 item 4's "外周レーン" — stage 5 — is [`route_perimeter`]: a genuine reverse edge
//! ([`EdgeShape::reverse`], never a self-loop) leaves and enters through the same ports every
//! other shape uses, then travels straight out to a rectangle [`PERIMETER_MARGIN`] px outside
//! every node and frame in the diagram and around whichever way is shorter to the same
//! straight-out point at the other end. Item 4's own spec text is "戻り辺・補助辺は…外周レーン" —
//! about *back* edges, and stage 3's own note on the collision fix ("『どの辺も他ノード箱と交差
//! しない』を全コーパス不変条件に") already relies on dagre's own waypoint chain routing clear of
//! every node by construction, no perimeter lane required. So a **collision-fallback forward
//! edge** ([`EdgeShape::staircase`] — a branch or merge whose direct shape crossed a node on both
//! attempts) and a **self-loop** (`EdgeShape::reverse` with the same source and target — never
//! `staircase`, `classify`'s own `is_reverse` check catches it first) both stay on
//! [`route_staircase_with_ports`], stages 1-4's own "暫定" (stopgap): dagre's own waypoint chain,
//! bent onto right angles, exactly as it always was. Stage 5 originally routed `staircase` through
//! the perimeter lane alongside a real back edge too — reverted 2026-09-01 once lane alignment
//! (§10-1 item 2) started actually moving nodes, which made a branch/merge collide against its
//! neighbours far more often and sent every one of those ordinary forward edges looping around the
//! whole diagram's outer edge, caught in a real diagram's own rendered pixels
//! (`samples/mermaid.ja.md`'s large flowchart).
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
//!
//! # A cluster-anchored edge is routed against a box, not a node
//!
//! §10-1 item 1's ports and item 4's perimeter lane are both written in terms of "a face of a
//! box"; a subgraph frame is exactly that (a centre and a size, [`PlacedCluster::bounds`] shaped
//! identically to [`PlacedNode::bounds`]), so an edge naming a subgraph as one of its own ends
//! (`one --> two`, where `one`/`two` are block ids, not node ids) is routed by the *same*
//! `classify`/`evict`/`route_with_ports` pipeline a node-to-node edge already goes through —
//! [`cluster_as_node`] is the one place that turns a [`PlacedCluster`] into the [`PlacedNode`]
//! shape everything else here already knows how to route against, and [`build_by_id`] is the one
//! lookup [`route_flowchart`]/[`avoid_label_plates`]/[`insert_crossing_gaps`] each resolve an
//! [`EligibleEdge`]'s `source`/`target` id through, node or cluster alike.
//!
//! This is deliberately **not** the same as adding a cluster to the *obstacle* list a node-to-node
//! edge's branch/merge sweep is tested against ([`shape_crosses_a_node`]'s `nodes` argument stays
//! real nodes only): §10-1 item 4's own "辺と枠の交差は隙間なし（直交して跨ぐだけ）" means a frame
//! that is not itself an edge's endpoint is never something a route has to avoid, only something it
//! may cross — exactly the behaviour a node-to-node edge already had before this module knew
//! clusters existed, and [`build_by_id`]'s own doc says why the two lookups are kept apart.

use std::collections::HashMap;

use super::{shapes, Glyph, Label, PlacedCluster, PlacedEdgeLabel, PlacedNode, Size};
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

/// [`classify`]'s own "多本数ファンアウト" threshold: the most branches 1b's basic shape (one
/// straight trunk plus one on each of the two cross-axis faces) can seat one-per-face before the
/// retreat rule has to take over — `classify`'s own `fan_eligible` doc has the full derivation.
const FAN_ELIGIBLE_MIN_BRANCHES: usize = 3;

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
    /// §10-3 item 1/2 ("多本数ファンアウトの流れ方向ポート" / "分岐レーンは8px刻み"): a branch
    /// edge whose source shares its face with a flow-axis-aligned sibling (`aligned`, on the same
    /// physical face) rides that same face — [`route_with_ports`]'s own doc on this field explains
    /// why that changes how the bend point is placed. Never set on a `merge`, `aligned`, or
    /// `reverse` shape — those already had (or, for `merge` since the round-3 correction, now
    /// always get) a flow-axis face on both ends without needing this flag at all.
    fan_lane: bool,
    /// §10-3 item 4's own "列間の空きレーン" — set only when `classify`'s ordinary two attempts
    /// (the natural branch/merge shape and its alt-swap) both crossed a node, one more attempt was
    /// tried before giving up to `staircase`, and that attempt cleared: a rank-skipping edge (its
    /// source and target are not on adjacent ranks — `align_straight_lanes`'s own doc on why a
    /// literal `r + 1` check cannot tell that) whose plain flow-axis-to-flow-axis bridge (a straight
    /// mid-point bend, `bridge`'s own `(Axis::Flow, Axis::Flow)` case) runs through an intervening
    /// rank's own node — `docs/mermaid-theme/handoff/round3-Konoma-Flowchart-Routing.dc.html`'s `3a`
    /// draws exactly this shape for `デコード → セルに合わせる` etc, bent through the empty column
    /// gap immediately before the target's own rank rather than the raw midpoint. Holds the exact
    /// flow-axis coordinate [`rank_lane_gap_bend`] found and [`shape_crosses_a_node`] already
    /// verified clear — [`route_with_ports`] reads it back rather than re-deriving it, so the route
    /// drawn is provably the one collision-tested, never a second, potentially different computation
    /// over the same (unchanged, same layout pass) node positions.
    rank_lane_bend: Option<f64>,
    /// Which face of the source the line leaves through, and which axis leaving perpendicular to
    /// it means moving along.
    source_side: Side,
    source_axis: Axis,
    /// Which face of the target the line enters through, and which axis entering perpendicular to
    /// it means moving along.
    target_side: Side,
    target_axis: Axis,
}

/// Whether `shape` bridges two flow-axis faces with a genuine bend (not the dead-straight
/// `aligned` shape) — every `fan_lane` edge is one, and so is an ordinary `branch`/`merge`/
/// collision-fallback `alt` shape once §10-3 item 3's correction gave `merge_target_side` the same
/// formula `branch_target_side` already used (`classify`'s own doc on that constant): the two
/// shapes now only ever differ in which face the *source* uses, never the target, so any of them
/// can land here.
///
/// [`separate_coincident_detours`]/[`insert_crossing_gaps`] both widen their "is this a detour
/// edge" test with this predicate, alongside the pre-existing `reverse`/`staircase` check —
/// `bridge`'s own `(Axis::Flow, Axis::Flow)` case computes its bend purely from the two ports' own
/// flow coordinates, with no awareness of any *other* edge sharing the same corridor, so two
/// unrelated Flow/Flow edges whose ports happen to share both flow coordinates (a genuine "X"
/// crossing — `amp-chain`'s own `A->D` and `B->C`, found once §10-3's correction put both on this
/// shape) draw the identical bend segment. `reverse`/`staircase` edges already had this exact
/// problem and this exact fix (`separate_coincident_detours`'s own doc); this is that same fix,
/// widened to the new shape class capable of it.
fn is_flow_flow_bend(shape: &EdgeShape) -> bool {
    !shape.aligned && shape.source_axis == Axis::Flow && shape.target_axis == Axis::Flow
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
    segment_crosses_node_padded(a, b, node, COLLISION_MARGIN)
}

/// [`segment_crosses_node`], generalised over the padding amount — `segment_crosses_node` itself is
/// just this called with [`COLLISION_MARGIN`], kept as a separate name (rather than a default
/// argument, which Rust has none of) because every call site but one wants exactly that padding.
/// The one exception is [`staircase_punctures_its_own_endpoint`], which calls this directly with
/// `0.0`: `COLLISION_MARGIN` exists to give a route some *aesthetic* clearance from a foreign node
/// (§10-1 item 1's "少しのマージン付き"), and a port sitting [`PORT_INSET`] (~1.75px) outside its
/// own node's face is always closer than that margin — so the padded test can never tell a route
/// that merely leaves/enters its own node correctly apart from one that actually re-enters it, and
/// zero padding (the node's real, unpadded boundary) is the only test that can.
fn segment_crosses_node_padded(a: &Point, b: &Point, node: &PlacedNode, margin: f64) -> bool {
    let (l, t, r, bo) = node.bounds();
    let (l, t, r, bo) = (l - margin, t - margin, r + margin, bo + margin);
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

/// Whether `points` — a finished route, one edge's own two ends included — genuinely enters
/// `source`'s or `target`'s real, unpadded interior anywhere along its length: [`route_with_ports`]'s
/// own trigger for discarding a stale raw-derived staircase route (see its doc for the full story),
/// and reused as-is by `render::tests`' corpus-wide invariant for the same question, so the router's
/// own decision and the regression test that guards it never drift apart into two hand-rolled copies
/// of the same test.
///
/// Every segment is checked — not only the one immediately touching a port — because the staleness
/// this guards against is dagre's own interior waypoints, which can drift arbitrarily far from a
/// node's *current* position once `align_straight_lanes` has moved it; nothing pins the puncture to
/// either end of the polyline.
///
/// `source`/`target` are each `Option` so a caller whose end is a subgraph frame rather than a real
/// node ([`shape_crosses_a_node`]'s own doc explains why a frame is exempt from this strict test —
/// §10-2's "no gap" leniency) can pass `None` for that side without duplicating the windows-and-`||`
/// plumbing at every call site; `render::tests`' corpus invariant always has two real nodes
/// (`Diagram::node` never returns a cluster), so it always passes `Some`/`Some`.
pub(crate) fn staircase_punctures_its_own_endpoint(
    points: &[Point],
    source: Option<&PlacedNode>,
    target: Option<&PlacedNode>,
) -> bool {
    points.windows(2).any(|w| {
        source.is_some_and(|n| segment_crosses_node_padded(&w[0], &w[1], n, 0.0))
            || target.is_some_and(|n| segment_crosses_node_padded(&w[0], &w[1], n, 0.0))
    })
}

/// Whether `shape`'s route between `source` and `target` — built at zero eviction offset, since a
/// port's few-pixel eviction offset never changes whether a sweep spanning the whole diagram
/// clears a sibling's box — crosses any node in `nodes` other than `source`/`target` themselves, OR
/// genuinely re-enters `source`'s or `target`'s own real interior.
///
/// The second half is not the same test as the first: a *foreign* node uses the padded
/// [`segment_crosses_node`] (§10-1 item 1's own "少しのマージン付き"), but a shape's own two ends
/// use the strict, unpadded [`segment_crosses_node_padded`] with `0.0` — the same distinction
/// [`staircase_punctures_its_own_endpoint`]'s own doc explains, for the same reason: a port sits
/// only [`PORT_INSET`] outside its own face, always inside the padded margin, so the padded test
/// could never tell a route that correctly leaves/enters its own node apart from one that actually
/// crosses back through it.
///
/// This corrects an assumption [`bridge`]'s own doc states as fact ("branch/merge...always exactly
/// one corner...this never grows past its stage-1 bend count", silently relying on that one corner
/// never landing inside either box) that is not always true: the "unlike axes" bridge holds the
/// *leaving* port's own tangent coordinate fixed for its first leg, and when the two nodes sit close
/// together on that axis — the target's own span can straddle the source's port height — that held
/// coordinate can sit *inside* the target's box before the final bend ever turns to leave it. Found
/// on `samples/mermaid.ja.md`'s "大きさ" flowchart: `MA[数式] --> RS[ラスタライズ]`'s plain merge
/// shape (2 bends, never routed through `route_staircase_with_ports` at all) held `MA`'s own port
/// height across its horizontal leg, which sat inside `RS`'s taller box, so the line entered `RS`
/// from the side before turning down to its (correctly placed) bottom port — the same "arrow looks
/// disconnected" symptom the staircase-specific fix elsewhere in this module addresses for a
/// *raw*-derived route, here from `bridge`'s own ordinary construction instead.
///
/// Folding this into the existing collision test rather than adding a separate check is
/// deliberate: `classify`'s callers already know exactly what to do with "this shape collides" —
/// swap to the alternate shape, and if that also collides, fall back to `staircase` — the identical
/// response a foreign-node collision gets, and the right one here too (§10-1 item 1's collision
/// fix's own reasoning "衝突しない限り" applies just as much to a shape colliding with its own
/// endpoint as with a stranger's).
///
/// The own-endpoint half only ever runs against a **real node**, never a subgraph frame standing in
/// for one (`cluster_as_node`'s doc: a cluster-anchored edge's `source`/`target` can be a frame-
/// shaped `PlacedNode` with no entry of its own in `nodes`, which this module's own doc already
/// establishes stays real-nodes-only). §10-2's own item 4 rule — "辺と枠の交差は隙間なし（直交して
/// 跨ぐだけ）" — is written about a frame a route merely passes through, but the same "no gap" leniency
/// has to extend to a frame that *is* the edge's own endpoint too: unlike a node's tight bounding box,
/// a frame encloses every member plus padding, so the same corner that would be a genuine puncture of
/// a small node's box is routinely just "still over some unrelated member of my own subgraph" for a
/// frame — `subgraph-direction`'s own `one->D` (`one`'s frame is the source) was flagged as
/// colliding with itself here before this exemption, wrongly forcing it onto the very same
/// `staircase` fallback this whole fix exists to keep from firing spuriously.
///
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
    // §10-3 item 4's own "列間の空きレーン": a `rank_lane_bend` candidate's whole point is a bend
    // the plain midpoint `bridge` would not have picked, so this collision test has to build the
    // *same* route [`route_with_ports`] will actually draw — reusing `bend_at` is what keeps the
    // two from ever silently disagreeing.
    if let Some(bend) = shape.rank_lane_bend {
        pts.extend(bend_at(direction, bend, &source_port, &target_port));
    } else {
        pts.extend(bridge(
            direction,
            &source_port,
            &target_port,
            shape.source_axis,
            shape.target_axis,
        ));
    }
    let source_is_real = nodes.iter().any(|n| n.id == source.id);
    let target_is_real = nodes.iter().any(|n| n.id == target.id);
    for w in pts.windows(2) {
        for node in nodes {
            if node.id == source.id || node.id == target.id {
                continue;
            }
            if segment_crosses_node(&w[0], &w[1], node) {
                return true;
            }
        }
        if (source_is_real && segment_crosses_node_padded(&w[0], &w[1], source, 0.0))
            || (target_is_real && segment_crosses_node_padded(&w[0], &w[1], target, 0.0))
        {
            return true;
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
            fan_lane: false,
            rank_lane_bend: None,
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
            fan_lane: false,
            rank_lane_bend: None,
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
    // §10-5 S1 ("ポートは極のみ…流れ軸と円周の交点"): an edge leaving a start marker is never a
    // decision point — `state::spec_of`'s own per-transition marker duplication
    // (`docs/FEATURE-MERMAID-RENDERER.md` §10-5) guarantees `source_out_degree == 1` here, so the
    // plain formula above would otherwise read the common "one child, no other in-edges to that
    // child" shape as `branching` and send it out the *cross*-axis face — the face a real
    // decision node's flat side sits on, not a marker's pole. Forcing `branching` off routes it
    // through `merge_source_side` below instead, which is already the flow-axis pole formula
    // (identical to the `target_side` every branch/merge shape already uses, §10-3 item 3's own
    // unification) — so a start-anchored edge's source is the pole in both the 0-bend `aligned`
    // case above and this 1-bend case. There is no such correction needed on the *target* side for
    // an end marker: `branch_target_side`/`merge_target_side` are already the same flow-axis
    // formula regardless of `branching` (§10-3 item 3), so an end marker's incoming face is always
    // the pole already.
    let marker_anchored =
        matches!(source.shape, Glyph::StateStart) || matches!(target.shape, Glyph::StateEnd);
    let branching = branching && !matches!(source.shape, Glyph::StateStart);
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
    // §10-3 item 3 ("合流の直交方向拡大…目標の流れ方向辺"): the round-3 reference
    // (`docs/mermaid-theme/handoff/round3-Konoma-Flowchart-Routing.dc.html`'s `3a`) draws every
    // multi-way merge entering its target's flow-axis face (Left for `LR`), not the round-2 prose
    // ("目標の直交辺") this shape used until now — confirmed independently by `2b`'s and `2c`'s
    // own "API ゲート" merges (3-in, entering the flow-axis face too), so this is `3a` correcting
    // an imprecise gloss in `2d`'s prose rather than a genuinely new rule. §10-3's own instruction
    // ("2d と食い違う箇所は 3a を採る") is why the round-2-pinned unit test below now asserts the
    // corrected shape instead. `merge_target_side` is now identical to `branch_target_side`'s own
    // formula — a real merge and a real branch always agree on which face the target uses; only
    // the *source* side ever differed between the two shapes, so eviction's "grow this face"
    // reasoning (`Eviction::required_size`) never has to reconcile two different target faces on
    // the very same node.
    let (merge_source_side, merge_target_side) = (
        flow_face(
            direction,
            flow(direction, &target.center) - flow(direction, &source.center),
        ),
        flow_face(
            direction,
            flow(direction, &source.center) - flow(direction, &target.center),
        ),
    );
    // §10-3 item 1 ("多本数ファンアウトの流れ方向ポート"), reimplemented on a principled numeric
    // threshold rather than the earlier "does the source already have a flow-axis-aligned sibling"
    // heuristic (`docs/STATUS.md`'s own ★未修正 entry has the full post-mortem — that heuristic was
    // reverse-engineered from too small a reference set and gets `docs/mermaid-theme/handoff/
    // zz-design-sources.md`'s own `2a`, a plain 3-way branch with no aligned member at all, wrong).
    // 1b's own basic shape — one straight trunk plus one branch on *each* of the two cross-axis
    // faces — seats at most [`FAN_ELIGIBLE_MIN_BRANCHES`] branches before the retreat rule (§10-1
    // item 1's own "退避則") has to pack every one of them onto the flow-axis face instead,
    // `PORT_SPACING` apart.
    let fan_eligible = branching && source_out_degree > FAN_ELIGIBLE_MIN_BRANCHES;
    let fan_shape = fan_eligible.then_some(EdgeShape {
        reverse: false,
        aligned: false,
        staircase: false,
        fan_lane: true,
        rank_lane_bend: None,
        source_side: merge_source_side,
        source_axis: Axis::Flow,
        target_side: branch_target_side,
        target_axis: Axis::Flow,
    });

    let (source_side, target_side) = if branching {
        (branch_source_side, branch_target_side)
    } else {
        (merge_source_side, merge_target_side)
    };
    let mut shape = EdgeShape {
        reverse: false,
        aligned: false,
        staircase: false,
        fan_lane: false,
        rank_lane_bend: None,
        source_side,
        source_axis: axis_of(direction, source_side),
        target_side,
        target_axis: axis_of(direction, target_side),
    };

    if let Some(fan_shape) = fan_shape {
        if !shape_crosses_a_node(direction, source, target, &fan_shape, nodes) {
            return fan_shape;
        }
        // §10-3 item 4's own scope boundary (`rank_lane_gap_bends`'s own doc, and `docs/STATUS.md`'s
        // ★未修正 entry): deliberately *not* retried with a rank-lane bend here, unlike the ordinary
        // branch/merge ladder below. A `fan_lane` sibling sits on the *same* face as every other
        // member of its source's own fanout (`3a`'s own "分岐レーンは中心から外向きに8px刻み" — the
        // whole point of the shape), so any gap search anchored on this edge's own source/target
        // pair alone has no way to know it must also dodge every *sibling* fanout edge's own bend
        // corridor and label plate — `shape_crosses_a_node` only ever tests real node boxes,
        // confirmed by dumping `samples/mermaid.ja.md`'s own `設定のルール -> デコード`/`usvg`/
        // `ページ描画`/`キーフレーム`: every gap a bounded, source-half-restricted search found was
        // "clear" of node boxes yet visually landed inside the fanout's own dense bend region,
        // overlapping labels and other members' lines. The ordinary branch/merge ladder below does
        // not have this problem (its two shapes' faces are not shared with any sibling by
        // construction), so it keeps the retry. Falls through to it now exactly as if this source
        // had no flow-aligned sibling at all — its `branch_source_side` (cross-face) shape almost
        // always clears cleanly on the first try, which is what these four edges actually draw.
    }

    if marker_anchored && shape_crosses_a_node(direction, source, target, &shape, nodes) {
        // §10-5 S1: a marker-anchored shape has no cross-axis alternate to swap to — unlike an
        // ordinary node, whose flat sides are all legitimate ports, a start/end marker's only
        // valid port is its pole (this function's own doc, just above). So a collision here skips
        // straight to `staircase` (dagre's own waypoint chain, straightened) rather than trying
        // the `alt` shape below, the same "no alternate" treatment the `aligned` case already gets
        // for the identical reason (its own comment, above).
        shape.staircase = true;
        return shape;
    }

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
            fan_lane: false,
            rank_lane_bend: None,
            source_side: alt_source_side,
            source_axis: axis_of(direction, alt_source_side),
            target_side: alt_target_side,
            target_axis: axis_of(direction, alt_target_side),
        };
        if shape_crosses_a_node(direction, source, target, &alt, nodes) {
            // §10-3 item 4 ("列間の空きレーン"): one more attempt before giving up to
            // `staircase` — the same face pair as whichever of `shape`/`alt` already has a
            // flow-axis source (target is flow-axis on both, §10-3 item 3's correction, so
            // exactly one of the two has a flow-axis *source* too — `branch`'s own `alt` when
            // `branching`, or `shape` itself for an ordinary `merge`), with the bend moved from
            // the raw midpoint into the column gap immediately upstream of the target
            // (`rank_lane_gap_bend`). A rank-skipping edge whose *midpoint* bend runs straight
            // through an intervening rank's own node — exactly what just made both attempts
            // above collide — often still clears once routed through the gap instead.
            //
            // §10-3 item 10: for a genuine merge (`!branching` — a plain, single-out-edge source
            // feeding a multi-way target, `classify`'s own `branching` formula) the search is
            // `extended` (`rank_lane_gap_bends`'s own doc): a merge source is never a busy fanout,
            // so nothing stops the search from reaching the *whole* span, not just its nearer
            // half. A branching source keeps the old, narrower search unchanged.
            let flow_flow_base = if shape.source_axis == Axis::Flow {
                shape
            } else {
                alt
            };
            let candidates = rank_lane_gap_bends(
                direction,
                source,
                target,
                flow_flow_base.target_side,
                nodes,
                !branching,
            );
            for &bend in &candidates {
                let mut candidate = flow_flow_base;
                candidate.rank_lane_bend = Some(bend);
                if !shape_crosses_a_node(direction, source, target, &candidate, nodes) {
                    return candidate;
                }
            }
            // §10-3 item 10's own "面が曖昧" fix: a genuine merge never falls back to the
            // cross-axis `alt` shape at all — rule 10 requires every edge into a multi-way merge
            // to leave its source's own flow-axis face (`docs/FEATURE-MERMAID-RENDERER.md`'s own
            // "ソースの右辺中央から水平に出て"), never its top/bottom. When every candidate above
            // still collided, the nearest one (tried first, so also the shallowest into the
            // target) is used anyway — `route_with_ports`'s own `rank_lane_bend` branch now runs
            // the result through `clear_local_route`, the same local-nudge remediation a
            // `staircase` edge already gets, so a route that still needs a small foreign-node
            // detour after this still keeps its correct, unambiguous faces rather than the
            // top/bottom exit `alt` would draw. Only when the search finds no candidate at all
            // (no column gap exists anywhere in range — the merge's own target sits in the very
            // first column, `rank_lane_gap_bends_is_empty_when_target_is_the_first_column`'s own
            // shape) does this fall through to the old cross-axis `alt` + `staircase` safety net.
            if !branching {
                if let Some(&bend) = candidates.first() {
                    let mut candidate = flow_flow_base;
                    candidate.rank_lane_bend = Some(bend);
                    return candidate;
                }
            }
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
/// but a genuine (non-self-loop) `reverse` back edge, but threaded through uniformly rather than
/// rebuilt per call (`route_flowchart` already has both on hand). `fan_step` is only ever read for
/// `shape.fan_lane` — [`route_fan_lane`]'s own doc on why it is a hint from a whole-face pass
/// rather than something this single-edge function could work out alone.
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
    fan_step: Option<f64>,
) -> Vec<Point> {
    let source_port = port_at(source, shape.source_side, source_coord, PORT_INSET);
    let target_port = port_at(target, shape.target_side, target_coord, PORT_INSET);

    let mut points = if shape.staircase || (shape.reverse && source.id == target.id) {
        // §10-1 item 4's perimeter lane is spec'd for "戻り辺・補助辺" (back edges) — 10-2's own
        // stage 3 note is "分岐⇄合流の衝突時切替＋階段フォールバック（『どの辺も他ノード箱と
        // 交差しない』を全コーパス不変条件に）", stated with no perimeter lane in sight, because
        // dagre's own waypoint chain already routes clear of every node by construction (dummy
        // nodes reserve the space a real edge threads through). `shape.staircase` — a *forward*
        // edge whose branch/merge attempts both crossed a node — is exactly that stage 3
        // mechanism, not a back edge, and belongs on this same local path regardless of whether
        // it also happens to start and end at the same box (a self-loop, `route_with_ports`'s own
        // doc explains, is never `staircase` — `classify`'s `is_reverse` check catches it first —
        // but is included here defensively rather than assumed).
        //
        // Reverted 2026-09-01: stage 5 had widened this branch's condition to `shape.reverse ||
        // shape.staircase`, routing a collision-fallback *forward* edge onto the perimeter lane
        // right alongside a genuine back edge — plausible-looking (both use dagre's raw waypoint
        // chain as their starting point) but wrong: item 4's own "外周レーンは…戻り辺" is written
        // about back edges specifically. Once lane alignment (§10-1 item 2) started actually
        // pulling nodes to the diagram's own edges (its `align_straight_lanes` bug fix, same day),
        // branch/merge collisions against those relocated nodes became far more common, and every
        // one of them rode this over-broad perimeter path — the coordinator's own real-pixel check
        // of `samples/mermaid.ja.md`'s large flowchart caught it (ordinary forward edges detouring
        // around the whole diagram's outer edge). Dagre's own waypoint chain, straightened, stays
        // exactly as stage 3 drew it before stage 5 existed.
        let staircase = route_staircase_with_ports(
            direction,
            shape,
            source_port.clone(),
            target_port.clone(),
            raw,
        );
        // `raw`'s dummy-node waypoints predate `align_straight_lanes` (`mod.rs`'s own comment on
        // why `EligibleEdge::raw` is read before alignment runs) exactly the way a self-loop's did
        // before that coupling was fixed — but here the drift is against *any* node the route
        // threads near, not one this function already knows the delta for, so the fix is local
        // geometry rather than a lookup: `clear_local_route`.
        //
        // `clear_local_route` only ever nudges a route *away* from a *foreign* node — it
        // deliberately excludes the edge's own two ends (its own doc: the port itself always sits
        // within `COLLISION_MARGIN` of its own node by construction, `PORT_INSET` is only ~1.75px,
        // so the padded test would flag that harmless graze on every single edge). But
        // `align_straight_lanes` only ever moves a node's *cross*-axis coordinate (its own doc:
        // "this function never touches the flow axis") — `raw`'s dummy waypoints are never
        // reprojected onto that new cross position at all (unlike a self-loop's, `mod.rs`'s
        // `shift_cross` above), so a *forward* staircase edge's raw-derived interior chain can
        // still be sitting at stale cross coordinates relative to its own two nodes' *current*
        // positions — not just grazing near a port, but running straight back through the node's
        // own real interior on its way to (or away from) that port. `clear_local_route`'s own
        // exclusion can never catch that (it is never even asked the question for these two nodes),
        // and patching it to ask would be the wrong fix anyway: its remedy is to slide the whole
        // flagged run sideways past the *foreign* node it hit, which for a run that also carries a
        // port coordinate would drag the port itself off to the side of its own node — trading the
        // interior-puncture bug for a "the arrow lands beside the node instead of on it" one.
        //
        // So the real fix is upstream: once the route is caught actually entering (not just
        // grazing) either of its own two nodes, `raw`'s shape for this edge is no longer trustworthy
        // at all, and the same current-geometry synthesis every non-staircase shape already uses —
        // `bridge` directly between the two (unmovable, `port_at`-computed, definitionally correct)
        // ports — replaces it outright. `bridge` can still cross a *foreign* node (that is exactly
        // why this edge fell back to a staircase in the first place — its direct branch/merge shape
        // already failed `classify`'s own collision test), which is exactly what `clear_local_route`
        // below is for; unlike the raw-derived path, `bridge`'s own two legs are built from the same
        // ports it starts and ends at, so it structurally cannot re-enter either one (§10-1 item 1's
        // one/two-bend shapes never have to avoid their own endpoints — only `classify`'s foreign-
        // node test ever runs against them). Found on `samples/mermaid.ja.md`'s "大きさ" flowchart's
        // `MD->MM`/`MM->RS` (`ブロックモデル`→`mermaid`→`ラスタライズ`): once `align_straight_lanes`
        // moved `MM` off dagre's original position, `MD->MM`'s raw-derived route ran ~38px down
        // into `MM`'s own interior before reaching its (correctly placed) bottom port, and
        // `MM->RS`'s left MM's bottom port only to double straight back up through MM's own box —
        // both real, not merely close reads: the arrow tip disappeared under `MM`'s own fill and the
        // line into/out of it looked disconnected, exactly the two symptoms reported.
        // Self-loops are excluded: a self-loop's raw waypoints are already kept in sync with its
        // one owner's move by `shift_cross` (`mod.rs`), and a loop is *expected* to run close
        // beside its own node by design — this resynthesis (a straight `bridge` between the two
        // ports) is specifically the forward-edge fallback shape, meaningless for `source.id ==
        // target.id` besides.
        //
        // A cluster-anchored end passes `None` here, the same real-node-only exemption
        // `shape_crosses_a_node`'s own doc explains (`nodes` is the diagram's real nodes only —
        // `route_flowchart`'s own doc — so a frame standing in for a subgraph never appears in it).
        let source_is_real = nodes.iter().any(|n| n.id == source.id);
        let target_is_real = nodes.iter().any(|n| n.id == target.id);
        let staircase = if shape.staircase
            && staircase_punctures_its_own_endpoint(
                &staircase,
                source_is_real.then_some(source),
                target_is_real.then_some(target),
            ) {
            let mut resynthesised = vec![source_port.clone()];
            resynthesised.extend(bridge(
                direction,
                &source_port,
                &target_port,
                shape.source_axis,
                shape.target_axis,
            ));
            resynthesised
        } else {
            staircase
        };
        clear_local_route(staircase, nodes, (source.id.as_str(), target.id.as_str()))
    } else if shape.reverse {
        let ids = (source.id.as_str(), target.id.as_str());
        let blocked = |a: &Point, b: &Point| segment_crosses_any_node(a, b, nodes, ids);
        let routed = route_perimeter(shape, source_port, target_port, ring, &blocked);
        // The same "own-endpoint pierce" class the staircase fix above already closed
        // (`clear_local_route`'s own doc, and the module's own `fd616c5` history) can reach a
        // genuine back edge too, for a structurally different reason `route_perimeter`'s own
        // collision search cannot see: its `blocked` closure always excludes both `source` and
        // `target` (a legitimate exit/entry leg touches its own node by construction, `route_
        // with_ports`'s own doc on `ids`), so nothing in that search ever notices a *return* leg
        // of the ring swinging back through the source's own box on its way to the target — found
        // on the `branch` corpus fixture's own `D -> B` cycle once §10-3 item 11 widened `regroup_
        // fan_lanes` to a plain two-way fan: regrouping moved `D` close enough under `B` that the
        // ring's own safe-exit geometry, correct in isolation, re-crosses `D`'s own new box before
        // reaching `B`. `clear_self_puncture` is `clear_local_route`'s own mechanism run against
        // the opposite pair — the edge's *own* two ends, never a foreign node — a no-op for every
        // ordinary back edge (the overwhelming majority, whose ring never revisits either node).
        clear_self_puncture(routed, source, target)
    } else if shape.fan_lane {
        route_fan_lane(
            direction,
            shape,
            source,
            &source_port,
            &target_port,
            fan_step,
        )
    } else if let Some(bend) = shape.rank_lane_bend {
        // §10-3 item 4: `classify` already found and collision-tested this exact bend
        // (`rank_lane_gap_bend`) — `bend_at` reproduces the identical route here, never a second,
        // independently-derived one. `clear_local_route` is a no-op for every edge `classify`
        // already confirmed clear (the overwhelming majority — its own collision test already
        // passed before returning this shape); its only real work is §10-3 item 10's own
        // best-effort merge fallback (`classify`'s own doc on why a merge can reach here with a
        // bend that *still* grazes a foreign node rather than falling back to the ambiguous
        // cross-axis `alt` shape) — the same local nudge a `staircase` edge already gets, kept
        // this route on its correct flow-axis faces instead of resynthesising from raw waypoints.
        let mut out = vec![source_port.clone()];
        out.extend(bend_at(direction, bend, &source_port, &target_port));
        clear_local_route(out, nodes, (source.id.as_str(), target.id.as_str()))
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

/// `side`'s own outward direction along the flow axis, as a sign — `Right`/`Bottom` (the faces
/// [`flow_face`] returns for a non-negative delta) point in the increasing direction, `Left`/`Top`
/// the decreasing one. [`route_fan_lane`]'s only use of `Side` at all: everywhere else it works
/// purely in flow/cross coordinates, but a bend has to move *away* from the node, and "away" is a
/// fact about which physical face this is, not about flow/cross alone.
fn outward_sign(side: Side) -> f64 {
    match side {
        Side::Right | Side::Bottom => 1.0,
        Side::Left | Side::Top => -1.0,
    }
}

/// `node`'s own half-extent along the *flow* axis — `h/2` for `TD`/`BT`, `w/2` for `LR`/`RL`. The
/// flow-axis counterpart [`cross_extent`] does not provide: [`rank_lane_gap_bend`]'s own column-gap
/// search needs a node's downstream/upstream *boundary*, which is its centre offset by this, not by
/// `cross_extent`'s cross-axis half-extent.
fn flow_extent(direction: Direction, node: &PlacedNode) -> f64 {
    match direction {
        Direction::TopToBottom | Direction::BottomToTop => node.size.h / 2.0,
        Direction::LeftToRight | Direction::RightToLeft => node.size.w / 2.0,
    }
}

/// [`bridge`]'s own `(Axis::Flow, Axis::Flow)` shape, but through a caller-chosen bend coordinate
/// instead of the plain midpoint — the two interior points plus `b`, matching `bridge`'s own return
/// shape exactly so a caller that already has `a` in its own point list (every caller here does)
/// can `.extend()` this the same way. [`route_fan_lane`]'s own hand-built four-point vector and
/// [`shape_crosses_a_node`]'s `rank_lane_bend` branch both use this, so the route a collision test
/// checks and the route actually drawn can never silently differ.
fn bend_at(direction: Direction, bend_flow: f64, a: &Point, b: &Point) -> Vec<Point> {
    vec![
        make(direction, bend_flow, cross(direction, a)),
        make(direction, bend_flow, cross(direction, b)),
        b.clone(),
    ]
}

/// §10-3 item 4's own "目標側の列間の空きレーン" — every flow-axis coordinate a rank-skipping
/// edge's bend could sit at, one per column gap upstream of `target`'s own entry face, ordered
/// nearest-to-`target` first, rather than [`bridge`]'s own plain midpoint (which runs straight
/// through whichever rank the edge skips over — exactly the collision that lands an edge here at
/// all: `classify`'s own caller only tries this once its ordinary two attempts both crossed a
/// node). `docs/mermaid-theme/handoff/round3-Konoma-Flowchart-Routing.dc.html`'s `3a` draws every
/// rank-skipping merge landing in the gap immediately before the target — `デコード → セルに合わ
/// せる`'s own bend sits inside the gap between `ラスタライズ`'s column and `セルに合わせる`'s own
/// (`M620,434 H1000 V338 H1038`, bend at `x=1000`) — but a real `dagre` layout's own cross-axis
/// spread (`classify`'s own `fan_shape` doc: a target pushed to a much later rank by its own
/// further connectivity, like `デコード`, can sit a long way from its source on *both* axes) means
/// the nearest gap's own vertical run can still cross something the nearest-gap-only version of
/// this search never considered — so this returns every gap in order, nearest first, and
/// `classify`'s own caller tries each in turn until one actually clears
/// ([`shape_crosses_a_node`]), the same "keep trying until one works, not just the first" shape its
/// existing branch/merge/alt ladder already has.
///
/// `source` is excluded from the search — a rank-skipping edge's own source sits even further
/// upstream than any gap being searched for (`3a`'s own `デコード` sits two whole ranks before
/// `セルに合わせる`), so it can never legitimately supply a gap's near wall, and *would* if left in
/// for a diagram where source and target happen to sit close together. Capped at
/// [`RANK_LANE_MAX_CANDIDATES`] gaps so a diagram with many columns cannot make this unbounded.
const RANK_LANE_MAX_CANDIDATES: usize = 6;

fn rank_lane_gap_bends(
    direction: Direction,
    source: &PlacedNode,
    target: &PlacedNode,
    target_side: Side,
    nodes: &[PlacedNode],
    extended: bool,
) -> Vec<f64> {
    let sign = outward_sign(target_side);
    let entry_boundary = flow(direction, &target.center) + sign * flow_extent(direction, target);
    // How far outward (upstream, away from `target` through `target_side`) a flow coordinate `p`
    // sits from `entry_boundary` — positive when `p` is genuinely upstream, growing the further out
    // it is. Every wall/gap/bend computation below is stated purely in this metric so the sign
    // arithmetic only has to be gotten right once, in one place.
    let dist = |p: f64| sign * (p - entry_boundary);
    // The inverse: a point sitting `d` outward of `entry_boundary`.
    let at_dist = |d: f64| entry_boundary + sign * d;

    // §10-3 item 4's own scope guard, found by dumping `samples/mermaid.ja.md`'s "大きさ" flowchart
    // (`docs/STATUS.md`'s own ★未修正 entry has the fuller story): `shape_crosses_a_node` only ever
    // asks "does this segment cross a NODE's own box" — it has no idea a face-full of *sibling*
    // fanout edges (`fan_shape`'s own bend corridor, right next to `source`) or a label plate sits
    // in the way too, so an unconstrained search can walk all the way back past a busy fanout's own
    // bend region and report a route "clear" that visually collides with everything living there.
    // Capping the search to the *nearer half* of the source-target span keeps every candidate closer
    // to `target` than to `source` — never wandering back into `source`'s own crowded neighbourhood
    // — at the cost of occasionally finding no candidate at all for an edge whose only clear gap
    // really does sit that close to `source` (this function then returns fewer candidates, or none,
    // and `classify`'s own caller falls back to the ordinary cross-face shape or `staircase`, exactly
    // the "避けられない場合のみ" this rule was always allowed to do).
    //
    // §10-3 item 10 (`docs/FEATURE-MERMAID-RENDERER.md`): that "busy fanout" concern is a fact
    // about a *branching* source with several siblings crowding its own exit face — it does not
    // apply to a genuine merge's own source (`classify`'s `!branching` ladder, `source_out_degree
    // <= 1` by construction), which has no sibling fanout to wander back into. `extended` is
    // `classify`'s own signal for that case: the full span is searched (`source_facing` itself,
    // never past it), not just its nearer half — every caller that can be a busy fanout source
    // (the `branching` ladder) always passes `false`, unchanged from before this parameter existed.
    let source_facing = flow(direction, &source.center) - sign * flow_extent(direction, source);
    let max_dist = if extended {
        dist(source_facing)
    } else {
        dist(source_facing) / 2.0
    };

    // Every other node's own *pair* of boundaries along this axis: `near` faces `target` (where a
    // gap ending at this node has to stop), `far` faces away from it (where the *next* gap, on the
    // other side of this node's own body, has to start) — a node has real width, so a single
    // boundary cannot stand in for it the way an early version of this function assumed (found by
    // a failing test: two obstacles' own `near` walls alone described a "gap" that actually ran
    // straight through the nearer obstacle's own box). Kept only if `near` sits upstream of
    // `target` within `max_dist` — a node entirely past the search radius cannot narrow any gap
    // this function will actually offer a candidate in.
    let mut walls: Vec<(f64, f64)> = nodes
        .iter()
        .filter(|n| n.id != target.id && n.id != source.id)
        .filter_map(|n| {
            let near = flow(direction, &n.center) - sign * flow_extent(direction, n);
            let far = flow(direction, &n.center) + sign * flow_extent(direction, n);
            let d = dist(near);
            (d > EPS && d <= max_dist).then_some((near, far))
        })
        .collect();
    walls.sort_by(|a, b| {
        dist(a.0)
            .partial_cmp(&dist(b.0))
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut cursor_dist = 0.0; // `entry_boundary` itself, the first gap's near wall.
    let mut out = Vec::new();
    for &(near, far) in walls.iter().take(RANK_LANE_MAX_CANDIDATES) {
        let near_dist = dist(near);
        let gap_width = near_dist - cursor_dist;
        if gap_width > EPS {
            let offset = if gap_width <= 2.0 * PORT_CLEARANCE {
                // Too narrow a gap to bend inside cleanly — half-way is the best this can do, and
                // the caller's own collision test (§10-1 item 1's own "少しのマージン付き") is what
                // actually decides whether that is usable at all.
                gap_width / 2.0
            } else {
                // A third to a half into the gap from its near wall — inside the gap regardless of
                // its width, and checked against `3a`'s own numbers (its rank-skipping bends sit
                // roughly a third to a half into a several-tens-of-px gap) rather than picked blind.
                (gap_width * 0.4).clamp(PORT_CLEARANCE, gap_width - PORT_CLEARANCE)
            };
            out.push(at_dist(cursor_dist + offset));
        }
        // The next gap (if any) starts on the far side of *this* node's own body — never inside it,
        // regardless of how close its own `near` wall was to the previous node's.
        cursor_dist = cursor_dist.max(dist(far));
    }
    // §10-3 item 10's own trailing gap: once every in-range wall's own body has been stepped past,
    // whatever room is left between there and `max_dist` is itself a candidate — the region nearest
    // `source`'s own facing wall, offered *last* (nearest-target candidates, above, are still tried
    // first by `classify`'s own caller). The un-`extended` (branching) search never reaches this: its
    // own `max_dist` is already only half the span, so a trailing gap out here would sit deep in a
    // busy fanout source's own crowded region — precisely what capping the search was for.
    if extended && out.len() < RANK_LANE_MAX_CANDIDATES {
        let gap_width = max_dist - cursor_dist;
        if gap_width > EPS {
            let offset = if gap_width <= 2.0 * PORT_CLEARANCE {
                gap_width / 2.0
            } else {
                (gap_width * 0.4).clamp(PORT_CLEARANCE, gap_width - PORT_CLEARANCE)
            };
            out.push(at_dist(cursor_dist + offset));
        }
    }
    out
}

/// §10-3 item 10's own trailing sentence ("ホップ x の入れ子", `docs/FEATURE-MERMAID-RENDERER.md`) —
/// every sibling edge that merges into the same target through a [`rank_lane_gap_bends`] hop
/// (`EdgeShape::rank_lane_bend`), or an ordinary adjacent-rank merge whose bend is [`bridge`]'s own
/// plain midpoint, is classified independently, against the diagram's real node boxes alone
/// ([`classify`]'s own doc). Nothing in that per-edge search knows a *sibling* is converging on the
/// same target at all, so two siblings' independently-computed hops can land close enough — or
/// identical — to draw one sibling's vertical leg through another's horizontal one, or two verticals
/// directly on top of each other (`docs/STATUS.md`'s own ★未修正 entry has the two real diagrams this
/// was found on).
///
/// This pass states the fix as three requirements, held once for the whole group, never tuned
/// against one diagram's specific numbers:
///
/// 1. no two siblings' routes may cross or coincide;
/// 2. where a nested `x` is needed at all, siblings sit at least [`PORT_CLEARANCE`] (8px) apart;
/// 3. a sibling is never pushed further from the target than avoiding a crossing requires.
///
/// The assignment: order the group **least-slack-first** — the sibling with the smallest
/// [`Candidate::max_reach`] (the least room it has to be pushed at all, since pushing it past its
/// own source's facing wall would draw its bend growing out of the wrong side of the node) is placed
/// first, keeping its own independently-computed hop unchanged; each next-least-slack sibling then
/// takes its own hop unless that would cross an already-placed sibling, in which case it steps
/// outward by [`PORT_CLEARANCE`] (repeated until clear, capped at both [`RANK_LANE_MAX_CANDIDATES`]
/// steps and its own `max_reach` — a defensive bound, not a promise every pathological diagram
/// resolves cleanly, the same "bounded, not a fixpoint search" trade-off [`clear_local_route`]'s doc
/// already accepts). Least-slack-first is a plain scheduling heuristic (the tightest-constrained
/// candidate gets first claim on the scarce nearby positions) — not tuned to, and not validated
/// against, any one reference diagram's specific pixel values; whether it happens to reproduce a
/// given hand-drawn reference is reported separately, never encoded here as a target.
///
/// Grouped purely by target id, restricted to genuine merge siblings (`is_merge_hop_candidate`'s own
/// `!branching`-mirroring guard) — a branching source's own edge into a shared target is a different
/// shape family this pass does not touch (`orthogonal_merge_sibling_hops_never_cross_or_coincide_
/// across_corpus`'s own doc explains why `subgraph-bypass`'s own `X -> Y` is out of scope here).
fn nest_merge_target_hops(
    direction: Direction,
    edges: &[EligibleEdge],
    by_id: &HashMap<&str, &PlacedNode>,
    shapes: &mut [Option<EdgeShape>],
    eviction: &Eviction,
) {
    // A genuine merge's own two faces are always Flow-axis on both ends (§10-3 item 3's own
    // correction, `classify`'s doc) — the one shape family this pass's "hop" concept applies to at
    // all. Two kinds reach here: [`EdgeShape::rank_lane_bend`] (a rank-skipping merge, already
    // routed through a column-gap hop `classify` found) *and* an ordinary adjacent-rank merge with
    // no `rank_lane_bend` at all, whose bend is instead [`bridge`]'s own plain midpoint, computed
    // fresh at route time from nothing but the two ports — the `MA -> RS` shape this pass's own doc
    // explains (`samples/mermaid.ja.md`'s "大きさ" flowchart): its bend can still land inside a
    // rank-skipping sibling's own H1 corridor, a crossing `classify`'s per-edge collision test can
    // never see (it only ever checks a route against *node* boxes, never a sibling edge's own
    // route). Both kinds are included here — `!branching` mirrors `classify`'s own guard for which
    // shapes this whole mechanism is for — and both end up holding their nested `x` in the same
    // `rank_lane_bend` field: [`route_with_ports`]'s own `Some(bend)` branch already draws exactly
    // this shape regardless of which path put a value there, so giving an ordinary merge a `Some`
    // for the first time (only ever done here, after eviction, `aligned` shapes excluded on
    // purpose) does not need a second drawing path.
    let is_merge_hop_candidate = |i: usize| -> bool {
        edges[i].source_out_degree <= 1
            && edges[i].target_in_degree > 1
            && shapes[i].as_ref().is_some_and(|s| {
                !s.aligned
                    && !s.reverse
                    && !s.staircase
                    && !s.fan_lane
                    && s.source_axis == Axis::Flow
                    && s.target_axis == Axis::Flow
            })
    };
    let mut groups: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, edge) in edges.iter().enumerate() {
        if is_merge_hop_candidate(i) {
            groups.entry(edge.target).or_default().push(i);
        }
    }

    struct Candidate {
        idx: usize,
        orig_dist: f64,
        /// How far outward this candidate's own source is allowed to reach at all — the distance
        /// from the target's entry boundary to the source's own facing wall (`rank_lane_gap_bends`'s
        /// own "extended" `max_dist`, reused unmodified: every candidate here is already `!branching`
        /// by [`is_merge_hop_candidate`]'s own guard, so the "busy fanout" concern that halves it for
        /// a branching source never applies). Doubles as both the placement order (nearer-reach
        /// siblings claim their own small gap first, the same "nearest first" `rank_lane_gap_bends`
        /// itself already tries candidates in) and a hard clamp on how far the crossing-avoidance
        /// loop below may push this candidate's own hop — pushing an adjacent-rank sibling's hop
        /// past its own source's facing wall reads as the bend growing out of the *wrong* side of
        /// the node (found on `MA -> RS`, this function's own doc: an earlier, unclamped version of
        /// this loop pushed `数式`'s own hop back into the gap `ページ描画`'s own H1 leg already
        /// filled, past `数式`'s own source wall, because sorting purely by `orig_dist` processed the
        /// much-longer-reaching `ページ描画`/`usvg` candidates first and left `数式` — whose own
        /// reach is short — to find there was nowhere left inside its own small gap).
        max_reach: f64,
        source_port: Point,
        target_port: Point,
        src_y: f64,
        tgt_y: f64,
    }

    for idxs in groups.into_values() {
        if idxs.len() < 2 {
            continue;
        }
        let Some(target_side) = shapes[idxs[0]].as_ref().map(|s| s.target_side) else {
            continue;
        };
        let Some(&target) = by_id.get(edges[idxs[0]].target) else {
            continue;
        };
        let sign = outward_sign(target_side);
        let entry_boundary =
            flow(direction, &target.center) + sign * flow_extent(direction, target);
        let dist = |p: f64| sign * (p - entry_boundary);

        let mut candidates: Vec<Candidate> = idxs
            .iter()
            .filter_map(|&i| {
                let shape = shapes[i].as_ref()?;
                let &source = by_id.get(edges[i].source)?;
                let src_y = eviction
                    .source_coord
                    .get(edges[i].id)
                    .copied()
                    .unwrap_or_else(|| face_center_coord(source, shape.source_side));
                let tgt_y = eviction
                    .target_coord
                    .get(edges[i].id)
                    .copied()
                    .unwrap_or_else(|| face_center_coord(target, target_side));
                let source_port = port_at(source, shape.source_side, src_y, PORT_INSET);
                let target_port = port_at(target, target_side, tgt_y, PORT_INSET);
                // The bend this edge would draw *without* this pass — its own `rank_lane_bend` when
                // `classify` already found one, otherwise exactly [`bridge`]'s own plain midpoint
                // (built from the very same ports [`route_with_ports`] itself constructs, so a group
                // with no crossing at all reproduces the pre-existing route byte for byte).
                let bend = shape.rank_lane_bend.unwrap_or_else(|| {
                    (flow(direction, &source_port) + flow(direction, &target_port)) / 2.0
                });
                let source_facing =
                    flow(direction, &source.center) - sign * flow_extent(direction, source);
                Some(Candidate {
                    idx: i,
                    orig_dist: dist(bend),
                    max_reach: dist(source_facing),
                    source_port,
                    target_port,
                    src_y,
                    tgt_y,
                })
            })
            .collect();
        if candidates.len() < 2 {
            continue;
        }
        candidates.sort_by(|a, b| {
            a.max_reach
                .partial_cmp(&b.max_reach)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // Every already-placed sibling's own *full* polyline (source port → hop → hop → target
        // port, [`bend_at`]'s own four points) — a candidate's own horizontal source-leg can cross
        // a sibling's vertical hop leg just as easily as the reverse (`MA -> RS`, this function's
        // own doc: `MA`'s vertical crosses `PD`'s horizontal source-leg, not the other way round),
        // so every one of a sibling's three segments has to be checked, not only the one nearest the
        // target this module's own earlier, reverted attempt at this checked alone.
        let mut placed: Vec<[Point; 4]> = Vec::with_capacity(candidates.len());
        let mut new_hops: Vec<(usize, f64)> = Vec::with_capacity(candidates.len());

        for c in &candidates {
            let mut d = c.orig_dist.max(0.0);
            let build = |d: f64| -> [Point; 4] {
                let hop = entry_boundary + sign * d;
                [
                    c.source_port.clone(),
                    make(direction, hop, c.src_y),
                    make(direction, hop, c.tgt_y),
                    c.target_port.clone(),
                ]
            };
            let mut route = build(d);
            let mut guard = 0;
            // Bounded on two independent fronts: `RANK_LANE_MAX_CANDIDATES` steps (this module's
            // own "bounded, not a fixpoint search" shape, `clear_local_route`'s doc), *and* never
            // past this candidate's own `max_reach` (`Candidate::max_reach`'s own doc on why —
            // pushing past it would grow the bend out of the source's own far side). Hitting the
            // reach ceiling with a crossing unresolved leaves the last, closest-to-clear position in
            // place rather than force one past the source's own wall; genuinely reachable in a
            // pathological diagram (more siblings than a small gap has room for), never seen on this
            // module's own corpus.
            while guard < RANK_LANE_MAX_CANDIDATES
                && d + PORT_CLEARANCE <= c.max_reach
                && placed.iter().any(|p| polylines_cross(&route, p))
            {
                d += PORT_CLEARANCE;
                route = build(d);
                guard += 1;
            }
            new_hops.push((c.idx, entry_boundary + sign * d));
            placed.push(route);
        }

        for (idx, hop) in new_hops {
            if let Some(shape) = shapes[idx].as_mut() {
                shape.rank_lane_bend = Some(hop);
            }
        }
    }
}

/// Whether any segment of polyline `a` crosses, or coincides (overlapping and collinear) with, any
/// segment of polyline `b`. The perpendicular case is exactly what [`segment_crossing`] already
/// decides — reused rather than re-derived, for the same reason that function is `pub(crate)` in the
/// first place (its own doc: a second, hand-rolled copy of the same axis-parallel intersection
/// arithmetic could silently drift from what [`insert_crossing_gaps`] itself checks). The one case
/// `segment_crossing` does not cover — two *parallel* segments overlapping collinearly, both
/// verticals landing at the identical hop `x` (`docs/STATUS.md`'s own ★未修正 entry: `デコード`/
/// `キーフレーム` both landing on the identical independent gap-search answer is exactly this, not a
/// perpendicular cross `segment_crossing` was ever built to see) — is checked separately here.
/// `pub(crate)` for the same reason [`segment_crossing`] is: `render::tests`' own merge-sibling
/// invariants state the question against a real diagram's *finished* polylines using this exact
/// predicate, not a third, hand-rolled copy.
pub(crate) fn polylines_cross(a: &[Point], b: &[Point]) -> bool {
    a.windows(2).any(|wa| {
        b.windows(2).any(|wb| {
            segment_crossing(wa, wb).is_some()
                || segments_overlap_collinearly(&wa[0], &wa[1], &wb[0], &wb[1])
        })
    })
}

/// Whether axis-parallel segments `a1`–`a2` and `b1`–`b2` run parallel, share the same fixed
/// coordinate, and overlap along the other axis — the "two verticals at the same hop `x`" case
/// [`polylines_cross`]'s own doc explains `segment_crossing` cannot see (it only ever answers a
/// perpendicular vertical-vs-horizontal question). A touch at a shared endpoint alone (the ordinary
/// case of two edges leaving the same port) is not flagged — every comparison is strict
/// (`EPS`-padded), matching the "own endpoint" exclusion every other collision test in this module
/// already applies ([`route_perimeter`]'s own `blocked` closure, [`clear_local_route`]'s doc).
fn segments_overlap_collinearly(a1: &Point, a2: &Point, b1: &Point, b2: &Point) -> bool {
    let a_vertical = (a1.x - a2.x).abs() < EPS;
    let b_vertical = (b1.x - b2.x).abs() < EPS;
    if a_vertical != b_vertical {
        return false; // perpendicular — segment_crossing's own territory, not this function's.
    }
    if a_vertical {
        if (a1.x - b1.x).abs() >= EPS {
            return false;
        }
        let (a_lo, a_hi) = (a1.y.min(a2.y), a1.y.max(a2.y));
        let (b_lo, b_hi) = (b1.y.min(b2.y), b1.y.max(b2.y));
        a_lo < b_hi - EPS && b_lo < a_hi - EPS
    } else {
        if (a1.y - b1.y).abs() >= EPS {
            return false;
        }
        let (a_lo, a_hi) = (a1.x.min(a2.x), a1.x.max(a2.x));
        let (b_lo, b_hi) = (b1.x.min(b2.x), b1.x.max(b2.x));
        a_lo < b_hi - EPS && b_lo < a_hi - EPS
    }
}

/// §10-3 item 2's own "8px 刻み" bend lane for an [`EdgeShape::fan_lane`] edge — [`bridge`]'s plain
/// midpoint-of-the-two-flow-coordinates bend replaced with a fixed step out from the *source*'s own
/// face, sized by this port's own rank among its siblings on that face.
///
/// `source_coord`/`target_coord` already placed both ports on an exact [`PORT_SPACING`] (16px)
/// grid centred on the aligned sibling's own port ([`evict`]'s "an aligned edge takes the slot
/// closest to the face's own centre" — the fan-lane group's shared face always has exactly one
/// aligned claim, its own trunk sibling), so `offset / PORT_SPACING` is always
/// (within [`EPS`] of) a whole number — `k`, this port's 1-based rank by distance from the centre.
/// Two ports at the same `k` on opposite sides of the centre — §10-3 item 2's own "上下対称な組"
/// (a symmetric pair) — get the identical bend distance by construction, since the formula below
/// depends on `k` alone, never on which side of the centre the port sits.
///
/// `step_hint`, when given, is [`Eviction::fan_step`]'s own precomputed value for this edge —
/// `evict` sees every sibling on the shared face at once and nests them **outside-in**: the port
/// *farthest* from the face's centre gets the *shallowest* bend, and each port nearer the centre
/// bends `PORT_CLEARANCE` further out (`docs/mermaid-theme/handoff/round3-Konoma-Flowchart-
/// Routing.dc.html`'s `3a` section — `設定のルール`'s ten-way fanout — is the reference this was
/// reverse-engineered from). A single edge routed in isolation cannot tell, from its own port
/// alone, how many siblings sit further out on *either* half of the face — the whole reason this is
/// a hint from a whole-face pass rather than computed here — so a caller with no such pass on hand
/// (a fan-lane edge routed through [`route_edge`], never reachable from a real diagram since
/// [`classify`]'s own `fan_eligible` doc explains a fan face always needs siblings; or
/// [`avoid_label_plates`]'s port-push retry, which *does* still run its own `evict` pass for
/// exactly this) passes `None`, and this falls back to the plain "step scales with `k` alone"
/// formula — nested the *opposite* way from `3a` (`docs/STATUS.md`'s ★未修正 entry on this exact
/// bug: the un-hinted formula bends the face's *busiest* siblings' stubs across each other's own
/// lanes), kept only so every caller still gets *some* two-bend route rather than a panic or a
/// silently wrong axis.
///
/// The bend sits at least `PORT_CLEARANCE * 2` px out from the source's own face along the flow
/// axis, growing by `PORT_CLEARANCE` for every rank further toward the centre, so the sibling
/// nearest the face's own centre — the one whose horizontal stub every other sibling's bend lane
/// must clear — reaches the deepest bend, past every other sibling's own shorter stub.
///
/// If the computed bend would not sit strictly between the source and target's own flow
/// coordinates — never seen on a real diagram (fan-lane siblings sit on a small face, ranks apart
/// from their target by whole rank gaps far wider than a handful of `PORT_CLEARANCE` steps), but a
/// defensive guard belongs here regardless — this falls back to [`bridge`]'s own plain midpoint
/// rather than draw a bend that overshoots past the target and reads as pointing the wrong way.
fn route_fan_lane(
    direction: Direction,
    shape: &EdgeShape,
    source: &PlacedNode,
    source_port: &Point,
    target_port: &Point,
    step_hint: Option<f64>,
) -> Vec<Point> {
    let step = step_hint.unwrap_or_else(|| {
        let center = face_center_coord(source, shape.source_side);
        let offset = cross(direction, source_port) - center;
        let k = (offset.abs() / PORT_SPACING).round().max(1.0);
        PORT_CLEARANCE * k
    });
    let bend_flow = flow(direction, source_port) + outward_sign(shape.source_side) * step;

    let (lo, hi) = (
        flow(direction, source_port).min(flow(direction, target_port)),
        flow(direction, source_port).max(flow(direction, target_port)),
    );
    if bend_flow <= lo || bend_flow >= hi {
        let mut out = vec![source_port.clone()];
        out.extend(bridge(
            direction,
            source_port,
            target_port,
            shape.source_axis,
            shape.target_axis,
        ));
        return out;
    }

    vec![
        source_port.clone(),
        make(direction, bend_flow, cross(direction, source_port)),
        make(direction, bend_flow, cross(direction, target_port)),
        target_port.clone(),
    ]
}

/// Dagre's own waypoint chain, straightened onto right angles — stages 1-4's original mechanism
/// for both a self-loop and a collision-fallback forward edge (`EdgeShape::staircase`), and, since
/// 2026-09-01, both again: see [`route_with_ports`]'s own doc for why only a genuine back edge
/// (`reverse`, never a self-loop) draws from [`route_perimeter`] instead.
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

/// [`route_staircase_with_ports`]'s own output, checked against every node the edge does not
/// itself touch, and nudged clear of any it grazes — the local remediation §10-1 item 2's stage 3
/// note ("『どの辺も他ノード箱と交差しない』を全コーパス不変条件に") promises, kept even though
/// `raw`'s dagre-computed waypoints can predate a node `align_straight_lanes` later moved (found on
/// `strokes`' `C~~~E`, which grazed `D`'s padded box by ~1.3px once `D` had shifted).
///
/// For each axis-parallel segment that crosses a foreign node ([`segment_crosses_node`], the same
/// [`COLLISION_MARGIN`]-padded test `classify`'s own collision fix uses), every point on that
/// segment's own straight run — not just the two points of the one flagged segment — moves to
/// clear the node, on whichever side the run already sat nearer to (so a route already passing
/// below a node stays below it, just with more clearance, rather than jumping to the other side
/// and reading as a different shape). Moving the whole run keeps the polyline axis-parallel; moving
/// only the flagged segment's two points would not.
///
/// Bounded to a handful of passes — the same "monotonic retry, defensive cap" shape
/// `lay_out_spec`'s own growth loop uses — rather than an unbounded fixpoint search: clearing one
/// node can in principle open a fresh crossing against a different one, but needs a second node
/// sitting within one more push of the first, the same documented approximation
/// `avoid_label_plates`'s own single-pass doc already accepts for its own local fixes.
fn clear_local_route(
    mut points: Vec<Point>,
    nodes: &[PlacedNode],
    ids: (&str, &str),
) -> Vec<Point> {
    const MAX_PASSES: usize = 4;
    for _ in 0..MAX_PASSES {
        let mut fix: Option<LocalFix> = None;
        'search: for w in points.windows(2) {
            let (a, b) = (&w[0], &w[1]);
            let horizontal = (a.y - b.y).abs() < EPS;
            let vertical = (a.x - b.x).abs() < EPS;
            if !horizontal && !vertical {
                continue; // never happens for this module's own output, but not this fn's to assume
            }
            for n in nodes {
                if n.id == ids.0 || n.id == ids.1 {
                    continue;
                }
                if segment_crosses_node(a, b, n) {
                    fix = Some(local_fix(&points, a, horizontal, n));
                    break 'search;
                }
            }
        }
        let Some(f) = fix else {
            break;
        };
        apply_local_nudge(&mut points, &f);
    }
    points
}

/// One already-found local nudge: `old_c`/`new_c`/`horizontal` are [`apply_local_nudge`]'s plain
/// "slide every point on `old_c` to `new_c`" instruction, same as before §10-3 item 13's own fix;
/// `port_stubs`, when non-empty, is that fix's own §10-3 item 13 correction — see [`local_fix`]'s
/// doc. Zero, one, or two entries: a route whose *both* ports happen to sit on the very coordinate
/// being slid (a straight, unbent run end to end) needs a stub at each end, not just one.
struct LocalFix {
    old_c: f64,
    new_c: f64,
    horizontal: bool,
    port_stubs: Vec<PortStub>,
}

/// §10-3 item 13's own "曲げを足す" correction for a port that sits *on* the coordinate a local fix
/// is about to slide out from under it: which port (`points[0]` when `at_start`, `points[last]`
/// otherwise) and the coordinate, along the leg's own *original* axis, of the safe jog point — where
/// the leg can still leave the port in its own correctly-assigned perpendicular direction for a
/// short distance before turning onto the avoided line, rather than either moving the port
/// ([`apply_local_nudge`]'s own exclusion of `points[0]`/`points[last]`) or turning immediately at it
/// (breaking §10-1 item 1's "出入りは常に辺へ垂直" — the very defect an earlier draft of this fix
/// had, caught by `assert_endpoints_sit_outside_and_perpendicular` failing on real corpus fixtures
/// once this module's own tests ran).
struct PortStub {
    at_start: bool,
    stub_other: f64,
}

/// Builds the [`LocalFix`] for one already-found crossing (`clear_local_route`'s own search loop, a
/// window `a`–`b` already known to cross `n` — only `a` is needed here, `old_c`'s own value): the
/// same "which side of `n` to slide the shared coordinate to" §10-1 item 1's local remediation
/// always used, plus — new for §10-3 item 13 — a [`PortStub`] for *every* port that actually sits on
/// the coordinate being slid.
///
/// Judged by coordinate, not by whether `i` happens to be the literal first/last window
/// (`docs/STATUS.md`'s own ★未修正 entry — the real bug this reimplementation closes): a `staircase`
/// edge's raw dagre waypoints routinely carry several collinear points in a row before reaching a
/// port whose own coordinate already matches them (`2b`'s own `API -> ID`, a plain, unbent
/// left-to-right run the *entire* way to `ID`'s own port — the crossing `clear_local_route`'s search
/// found sat on an *early* window of that run, not literally `i + 2 == points.len()`, so the old
/// window-identity test never built a stub for the port at all: `apply_local_nudge`'s "ports never
/// move" then left `ID`'s own port stranded on the old coordinate while every other point on the
/// same straight run slid to the new one — a diagonal final segment, this module's own axis-parallel
/// invariant broken in exactly the corpus shape (a long cross-subgraph edge) no existing fixture
/// happened to exercise). Testing coordinate equality against `points[0]`/`points[last]` directly
/// catches this regardless of which window the search actually flagged, and is exactly equivalent to
/// the old `i == 0` / `i + 2 == points.len()` test whenever those *were* the flagged window (the
/// terminal window's own endpoint is that port by construction), so no existing behaviour changes.
fn local_fix(points: &[Point], a: &Point, horizontal: bool, n: &PlacedNode) -> LocalFix {
    let (l, t, r, bo) = n.bounds();
    let (old_c, new_c) = if horizontal {
        let y = a.y;
        let new_y = if y <= (t + bo) / 2.0 {
            t - COLLISION_MARGIN - 1.0
        } else {
            bo + COLLISION_MARGIN + 1.0
        };
        (y, new_y)
    } else {
        let x = a.x;
        let new_x = if x <= (l + r) / 2.0 {
            l - COLLISION_MARGIN - 1.0
        } else {
            r + COLLISION_MARGIN + 1.0
        };
        (x, new_x)
    };
    let coord_of = |p: &Point| if horizontal { p.y } else { p.x };
    let last = points.len() - 1;
    // Which side of `n` a stub belongs on, from the *direction of travel* along the port's own
    // adjacent segment (never the flagged window's `a`/`b`, which need not be that segment at all
    // once the test above is coordinate-based rather than window-identity-based) — `at_start`'s own
    // stub is the port's outward leg, so it has to stop on the *near* side of `n` (the side the port
    // faces) before ever reaching it; an end stub's own leg arrives *at* the port, so it has to
    // already be past `n`, on the *far* side — mirror images of the same "which edge of `n` do I
    // reach first from here" question.
    let stub_other_for = |dir: f64, at_start: bool| {
        let near_side = if at_start { dir >= 0.0 } else { dir < 0.0 };
        if horizontal {
            if near_side {
                l - COLLISION_MARGIN - 1.0
            } else {
                r + COLLISION_MARGIN + 1.0
            }
        } else if near_side {
            t - COLLISION_MARGIN - 1.0
        } else {
            bo + COLLISION_MARGIN + 1.0
        }
    };
    let mut port_stubs = Vec::new();
    if points.len() > 1 && (coord_of(&points[0]) - old_c).abs() < EPS {
        let dir = if horizontal {
            points[1].x - points[0].x
        } else {
            points[1].y - points[0].y
        };
        port_stubs.push(PortStub {
            at_start: true,
            stub_other: stub_other_for(dir, true),
        });
    }
    if points.len() > 1 && (coord_of(&points[last]) - old_c).abs() < EPS {
        let dir = if horizontal {
            points[last].x - points[last - 1].x
        } else {
            points[last].y - points[last - 1].y
        };
        port_stubs.push(PortStub {
            at_start: false,
            stub_other: stub_other_for(dir, false),
        });
    }
    LocalFix {
        old_c,
        new_c,
        horizontal,
        port_stubs,
    }
}

/// [`clear_local_route`]'s own "apply the fix" step: every point sharing the flagged run's own
/// constant coordinate (`f.old_c`) slides to `f.new_c` together, same as before §10-3 item 13's own
/// fix, except `points[0]`/`points[last]` — the route's own two ports, pinned to the exact slot
/// [`evict`] assigned them — which never move.
///
/// When the coordinate being slid touches a port (`f.port_stubs`, [`local_fix`]'s own doc on why
/// this is judged by coordinate, not by which window was flagged), a plain slide of everything else
/// would leave that port's own immediate neighbour on the *new* coordinate while the port itself
/// stayed on the *old* one — a diagonal leg, or (an earlier draft's own bug, `PortStub`'s own doc) a
/// leg bent at the port instead of past it. The constructive fix splices two extra points in beside
/// each such port instead: it keeps leaving/entering in its own correctly-assigned perpendicular
/// direction for a short stub (`PortStub::stub_other`, chosen clear of the very node this fix is
/// avoiding), *then* turns onto the shifted line — "曲げを足す", never a slide through the port
/// itself (`docs/FEATURE-MERMAID-RENDERER.md` §10-3 item 13's own motivating report: `ページ描画→
/// ラスタライズ`'s exit stub, displaced by `数式`'s own box sitting in the same pass-through row,
/// used to read as growing from `ページ描画`'s own corner instead of its assigned port).
fn apply_local_nudge(points: &mut Vec<Point>, f: &LocalFix) {
    let last = points.len() - 1;
    for (idx, p) in points.iter_mut().enumerate() {
        if idx == 0 || idx == last {
            continue; // ports never move.
        }
        if f.horizontal && (p.y - f.old_c).abs() < EPS {
            p.y = f.new_c;
        } else if !f.horizontal && (p.x - f.old_c).abs() < EPS {
            p.x = f.new_c;
        }
    }

    // End before start: splicing near `points.len() - 1` never disturbs index `1`, but splicing at
    // index `1` first would shift every later index the end stub still needs to compute — a route
    // whose *both* ports sit on the coordinate being slid (`LocalFix::port_stubs`'s own doc) needs
    // both, in this order, to land correctly.
    for stub in f.port_stubs.iter().filter(|s| !s.at_start) {
        let last = points.len() - 1;
        let (on_new, on_old) = if f.horizontal {
            (
                Point::new(stub.stub_other, f.new_c),
                Point::new(stub.stub_other, f.old_c),
            )
        } else {
            (
                Point::new(f.new_c, stub.stub_other),
                Point::new(f.old_c, stub.stub_other),
            )
        };
        points.splice(last..last, [on_new, on_old]);
    }
    for stub in f.port_stubs.iter().filter(|s| s.at_start) {
        let (on_old, on_new) = if f.horizontal {
            (
                Point::new(stub.stub_other, f.old_c),
                Point::new(stub.stub_other, f.new_c),
            )
        } else {
            (
                Point::new(f.old_c, stub.stub_other),
                Point::new(f.new_c, stub.stub_other),
            )
        };
        points.splice(1..1, [on_old, on_new]);
    }
}

/// [`clear_local_route`]'s own mechanism, run against the opposite pair: `source`'s and `target`'s
/// own boxes, the two nodes every route's own collision search (`route_with_ports`'s `blocked`
/// closures, built from [`segment_crosses_any_node`]) always excludes, on the reasoning that a
/// route's own two ends touch its own two nodes by construction. That reasoning covers an exit or
/// entry leg — a segment ending *at* the port — but not a `route_perimeter` ring's own *return*
/// leg re-crossing the source's box on its way to a target sitting close by (`route_with_ports`'s
/// own doc on why only `shape.reverse` needs this: every other shape's own two-or-fewer-bend
/// geometry is built directly from its own two ports, so it structurally cannot re-enter either —
/// [`staircase_punctures_its_own_endpoint`]'s own doc on `bridge`'s "unlike axes" shape is the one
/// documented exception, already handled where it is built). Uses the same strict, unpadded
/// [`segment_crosses_node_padded`] (margin `0.0`) that predicate does, for the same reason: a port
/// sits only [`PORT_INSET`] outside its own face, and a padded test could not tell a route that
/// correctly leaves/enters its own node apart from one that actually crosses back through it.
fn clear_self_puncture(
    mut points: Vec<Point>,
    source: &PlacedNode,
    target: &PlacedNode,
) -> Vec<Point> {
    // §10-3 item 13's own "ポートは動かさない" is scoped to `clear_local_route`'s own forward-edge
    // callers (`route_with_ports`'s `staircase`/`rank_lane_bend` branches — the reported bug's own
    // route shapes, `local_fix`'s own doc). A back edge's own return leg genuinely can need to
    // slide *through* a coordinate one of its own two ports also sits at (found on the `branch`
    // corpus fixture's `D -> B`: `regroup_fan_lanes` moved `D` close enough under `B` that both
    // ends' independently-evicted ports land on the exact same x, and the return ring's own local
    // fix has no way to clear `D`'s box without touching that shared coordinate) — a case §10-3
    // item 13 was not written against and this task does not extend to (`docs/STATUS.md`'s own
    // ★未修正 carries this residual: the fixed geometry still clears every node, `assert_no_edge_
    // crosses_its_own_endpoint` stays green, but neither port's exact eviction slot is pinned here
    // the way `clear_local_route`'s now is). Left as the same plain "every point sharing this
    // coordinate slides together" sweep it always was.
    const MAX_PASSES: usize = 4;
    for _ in 0..MAX_PASSES {
        let mut fix: Option<(f64, f64, bool)> = None; // (old constant coord, new one, horizontal?)
        'search: for w in points.windows(2) {
            let (a, b) = (&w[0], &w[1]);
            let horizontal = (a.y - b.y).abs() < EPS;
            let vertical = (a.x - b.x).abs() < EPS;
            if !horizontal && !vertical {
                continue;
            }
            for n in [source, target] {
                if segment_crosses_node_padded(a, b, n, 0.0) {
                    let (l, t, r, bo) = n.bounds();
                    fix = Some(if horizontal {
                        let y = a.y;
                        let new_y = if y <= (t + bo) / 2.0 {
                            t - COLLISION_MARGIN - 1.0
                        } else {
                            bo + COLLISION_MARGIN + 1.0
                        };
                        (y, new_y, true)
                    } else {
                        let x = a.x;
                        let new_x = if x <= (l + r) / 2.0 {
                            l - COLLISION_MARGIN - 1.0
                        } else {
                            r + COLLISION_MARGIN + 1.0
                        };
                        (x, new_x, false)
                    });
                    break 'search;
                }
            }
        }
        let Some((old_c, new_c, horizontal)) = fix else {
            break;
        };
        for p in &mut points {
            if horizontal && (p.y - old_c).abs() < EPS {
                p.y = new_c;
            } else if !horizontal && (p.x - old_c).abs() < EPS {
                p.x = new_c;
            }
        }
    }
    points
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
        // No sibling face to nest against in this isolated single-edge helper — never actually
        // reachable for a `fan_lane` shape anyway (`route_fan_lane`'s own doc: `classify`'s
        // `fan_eligible` trigger needs a real sibling), so the fallback formula is never exercised.
        None,
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
    /// §10-3 item 1 ("ファン面の中央ポート＝幹の直進辺"): whether this claim is the trunk/chain
    /// edge [`align_straight_lanes`] selected for this claim's own *source* — set only on the
    /// `FaceEnd::Source` claim, never the target (`evict`'s own doc explains why the target side
    /// stays `false`). Gives the fan face's centre port to the trunk even when `align_straight_
    /// lanes`'s geometric collapse never fully landed the two nodes on the same cross coordinate,
    /// so `shape.aligned` alone (the 0-bend case) would miss it.
    trunk: bool,
    /// Whether this claim's own edge draws as [`EdgeShape::fan_lane`] — set only on the
    /// `FaceEnd::Source` claim, the same `Source`-only rule `trunk` uses just above (the bend
    /// depth §10-3 item 2 assigns is a *source*-face concept, so a fan-lane edge's target-side
    /// claim never needs it). [`evict`]'s own per-face `fan_step` pass is the one reader: it needs
    /// to tell a face's fan-lane siblings apart from the one aligned/trunk claim sharing the same
    /// face, which never bends at all.
    fan_lane: bool,
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
    /// Edge ids whose own [`classify`]-decided shape is the "flow-axis exit, hop toward target"
    /// family §10-1 item 1 / §10-3 item 13 describe (`is_pass_through_shape`'s own doc) — the only
    /// shape family a rank-skipping pass-through corridor (`mod.rs`'s own `reserve_pass_through_
    /// rows`) can exist for at all. `mod.rs`'s own caller reads this to decide *which* rank-
    /// skipping edges actually have a row worth reserving, rather than every one topology alone
    /// would suggest (`docs/STATUS.md`'s own ★未修正 entry: `long-edge`'s own `A -> E`, a branching
    /// source whose natural shape never touches a flow-axis row at all, used to get one reserved
    /// anyway purely because its rank happened to skip two).
    pub pass_through_eligible: std::collections::HashSet<String>,
}

/// Whether `shape`'s own two faces are the "flow-axis exit, hop toward target" family rule 10 (the
/// merge side) and rule 13 (pass-through rows) both describe: a straight leg leaves the source
/// along the **flow** axis (`source_axis == Axis::Flow`) and is never one of the shapes that
/// abandons that leg entirely — [`EdgeShape::fan_lane`] (a busy branching face's own outside-in
/// nesting, a different corridor concept, §10-3 item 2), a genuine back edge
/// ([`EdgeShape::reverse`]), or a `staircase` fallback (dagre's own raw waypoint chain, straightened
/// — never a clean flow-axis leg by construction). [`route_flowchart`]'s own `pass_through_
/// eligible` field is exactly this predicate, applied once per edge, over every edge's own real
/// `classify` result — never re-derived from the *actual*, possibly collision-avoided polyline
/// (`RoutedFlowchart::pass_through_eligible`'s own doc: a `!branching` merge already forced into a
/// local nudge by an obstacle — `samples/mermaid.ja.md`'s own `ページ描画 -> ラスタライズ` — still
/// qualifies; only the shape family matters, not whether this particular pass happened to draw it
/// cleanly).
fn is_pass_through_shape(shape: &EdgeShape) -> bool {
    shape.source_axis == Axis::Flow && !shape.fan_lane && !shape.reverse && !shape.staircase
}

/// [`evict`]'s result — see its own doc for how each field is built.
struct Eviction {
    source_coord: HashMap<String, f64>,
    target_coord: HashMap<String, f64>,
    required_size: HashMap<String, Size>,
    /// Edge id → [`route_fan_lane`]'s own bend-depth step (already `PORT_CLEARANCE`-scaled px,
    /// ready to use as-is), for every `fan_lane` edge on a face `evict` just placed ports on —
    /// `route_fan_lane` cannot work this out alone from a single edge's own port (§10-3's own
    /// "outside-in nesting" needs to know how many siblings sit on *each* half of the shared face,
    /// which only this whole-face pass sees). An edge absent from this map (every non-`fan_lane`
    /// edge) draws unaffected — `route_fan_lane` is the only reader, and only when `shape.fan_lane`.
    fan_step: HashMap<String, f64>,
}

/// One global eviction pass: given every edge's already-decided [`EdgeShape`], works out the exact
/// port coordinate for both ends of every edge, and the minimum size every node needs to fit them.
///
/// Grouped by `(node id, face)` rather than by node: two different faces of the same node are
/// independent corridors with their own port count, so growing one (§10-1 item 1: "ノードをその
/// 軸方向に拡大") only ever widens (for `Top`/`Bottom`) or heightens (for `Left`/`Right`) the node
/// — the two axes never fight over the same requirement, and [`Eviction::required_size`] reports
/// each independently.
///
/// `chain_next` is [`align_straight_lanes`]'s own selection (source id → target id, `mod.rs`'s
/// threaded-through result) — §10-3 item 1's own centre-port rule for the trunk edge of a fan face
/// needs this alongside `shape.aligned`: a chain edge that lost its geometric alignment to a
/// crowded rank (`align_straight_lanes`'s own doc on why `chain_next` can disagree with the final
/// coordinates) still draws as the fan's `fan_lane` shape, not the flat `aligned` one, so
/// `shape.aligned` alone cannot find it.
fn evict(
    direction: Direction,
    by_id: &HashMap<&str, &PlacedNode>,
    edges: &[EligibleEdge],
    shapes: &[Option<EdgeShape>],
    chain_next: &HashMap<String, String>,
) -> Eviction {
    let mut groups: HashMap<(String, Side), Vec<FaceClaim>> = HashMap::new();
    for (edge, shape) in edges.iter().zip(shapes) {
        let Some(shape) = shape else { continue };
        let (Some(&source), Some(&target)) = (by_id.get(edge.source), by_id.get(edge.target))
        else {
            continue;
        };
        let is_trunk = chain_next.get(edge.source).map(String::as_str) == Some(edge.target);
        groups
            .entry((edge.source.to_string(), shape.source_side))
            .or_default()
            .push(FaceClaim {
                edge_id: edge.id.to_string(),
                end: FaceEnd::Source,
                other_cross: cross(direction, &target.center),
                aligned: shape.aligned,
                trunk: is_trunk,
                fan_lane: shape.fan_lane,
            });
        groups
            .entry((edge.target.to_string(), shape.target_side))
            .or_default()
            .push(FaceClaim {
                edge_id: edge.id.to_string(),
                end: FaceEnd::Target,
                other_cross: cross(direction, &source.center),
                aligned: shape.aligned,
                // Deliberately NOT `is_trunk`: §10-3 item 1 is about the *fan* face — a source's
                // own outgoing face, crowded with several siblings — not the target's incoming
                // face. Marking both ends trunk (tried first) silently swaps which of two unrelated
                // claims on a plain 1-or-2-claim target face sits on which side of the centre —
                // found on the `branch` corpus fixture (`D`'s own Top face carries `B->D`'s target
                // claim and `D->B`'s source claim; centring the former moved the latter by 16px),
                // which then re-pointed `D->B`'s own perimeter ring low enough to cut straight
                // through `D`'s own box on the way back up to `B`
                // (`orthogonal_no_edge_crosses_its_own_endpoint_across_the_whole_corpus`).
                trunk: false,
                // A fan-lane edge's *target* claim is an ordinary single/plain claim on the
                // target's own incoming face — the bend §10-3 item 2 describes belongs to the
                // source face's crowded fan, never the target side (`trunk`'s own doc, just
                // above, for the identical reasoning).
                fan_lane: false,
            });
    }

    let mut source_coord: HashMap<String, f64> = HashMap::new();
    let mut target_coord: HashMap<String, f64> = HashMap::new();
    let mut required_size: HashMap<String, Size> = HashMap::new();
    let mut fan_step: HashMap<String, f64> = HashMap::new();

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
        // Rule 2 / §10-3 item 1: the aligned (zero-bend) edge, OR — when geometry never fully
        // collapsed onto it — the trunk/chain edge `align_straight_lanes` selected for this source,
        // takes the slot closest to the face's own centre; exactly the centre when `n` is odd. When
        // `n` is even the 16px grid has no exact centre slot at all (its two middle slots sit at
        // ±(PORT_SPACING/2)); `f64::round`'s "half away from zero" rule picks the *higher* index of
        // the two (`(n-1)/2` is exactly `x.5`, which rounds up), deterministically — a corner case
        // the spec does not disambiguate further.
        // If more than one claim on the same face somehow qualifies (two dead-straight edges
        // sharing a face — not reachable from any real flowchart, since it needs two different
        // nodes at the same cross coordinate as this one; a face can never carry two distinct chain
        // edges either, since `align_straight_lanes` selects at most one outgoing/incoming chain
        // edge per node), only the first (in sort order) is treated as the anchor; the rest keep
        // their sorted position like any other claim.
        // `anchor`, when set, is the slot index that must sit at exactly `offset == 0.0` — the
        // face's own centre coordinate, un-nudged — because an aligned/trunk claim lives there.
        //
        // §10-3 item 12's own fix (`docs/FEATURE-MERMAID-RENDERER.md`, "面のポートは相手の側で配る"):
        // this used to *move* the aligned/trunk claim from wherever it naturally sorted to an
        // array-symmetric `center_idx` (`claims.remove(aligned_pos); claims.insert(center_idx, ..)`)
        // — but `claims` is already sorted by `other_cross` (rule 1's own "もう一方の端点のcross座標
        // 順"), so every claim *before* the anchor in that order is genuinely on one side of the
        // face's own centre and every claim *after* it is genuinely on the other (the sort key and
        // the centre-side test are the same axis). Splicing the anchor into a *different* index
        // physically swaps other claims across that boundary — `セルに合わせる`'s real 3-way merge
        // (`docs/STATUS.md`'s own ★未修正 entry) is exactly this: naturally sorted
        // `[幹, デコード, キーフレーム]` (幹's own other-end sits almost exactly on the face centre,
        // both siblings genuinely below it), `center_idx = round((3-1)/2) = 1` forces 幹 from index 0
        // to index 1, which drags デコード down into index 0 along the way — its offset flips from
        // the `+16` its true side calls for to `-16`, folding it back above centre right next to the
        // face's own left edge (the reported "line grows from the wrong corner" bug's own sibling
        // symptom: a below-centre port drawn above centre). Anchoring on the claim's own *natural*
        // sorted position instead — no splice — keeps every other claim exactly where the sort
        // already put it relative to the anchor, so a same-side sibling can never cross to the other
        // side: デコード and キーフレーム both land after 幹, at `+16`/`+32`, matching `3a`'s own
        // `338`/`354` (centre `322`) exactly.
        //
        // This does not regress the even-`n` fan-centring case the removed splice was originally
        // written for (`3a`'s own ten-way fanout, referenced below): that geometry is engineered
        // upstream, at layout time, by `mod.rs::regroup_fan_lanes`, which places the trunk's own
        // *node* at the fan's array-centre index before dagre ever runs — so by the time this
        // function's own sort runs, the trunk's `other_cross` is already the fan's own natural
        // sorted middle, and anchoring on its natural position lands it at offset 0 regardless (the
        // two ends coincide for every corpus/regression fixture this module pins — `an_aligned_
        // edge_keeps_the_centre_port_and_siblings_move_outward`,
        // `regroup_fan_lanes_groups_by_colour_centres_the_trunk_and_pushes_classless_outermost`,
        // `orthogonal_settings_rules_sample_fan_column_matches_3a_and_spine_is_all_zero_bend` all
        // still pass unchanged). A `trunk` claim whose own geometry never collapsed onto the centre
        // at all (`a_chain_selected_trunk_keeps_the_centre_port_even_when_geometry_never_aligned_it`)
        // still gets forced to offset 0 — anchoring is still unconditional — it just no longer drags
        // its natural neighbours across the centre line to make room.
        let anchor = claims.iter().position(|c| c.aligned || c.trunk);

        // §10-3 item 10's own retreat-rule fix: the widest offset actually handed out below, on
        // *either* side of the face's own centre — plain `|offset|`, tracked as the loop goes so
        // the retreat computation just past it never has to re-derive the anchor-relative grid a
        // second time. An anchored (odd `trunk`/`aligned` position) face is not symmetric about
        // its own centre index the way the un-anchored `(n-1)/2` grid always is (`anchor`'s own
        // doc: RS's own four-way merge in `samples/mermaid.ja.md`'s "大きさ" flowchart anchors on
        // its trunk claim at index 2 of 4, giving offsets `-32, -16, 0, +16` — `32`, not `16`, is
        // the true widest reach), so `required_flat` below reads this rather than assuming the
        // un-anchored, always-symmetric `(n-1)*PORT_SPACING` span.
        let mut max_abs_offset = 0.0_f64;
        for (i, claim) in claims.iter().enumerate() {
            let offset = match anchor {
                Some(anchor) => (i as f64 - anchor as f64) * PORT_SPACING,
                None => (i as f64 - (n as f64 - 1.0) / 2.0) * PORT_SPACING,
            };
            max_abs_offset = max_abs_offset.max(offset.abs());
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

        // §10-3 item 2's own "分岐レーンは中心から外向きに8px刻み", `3a`'s outside-in nesting
        // corrected ([`route_fan_lane`]'s own doc has the geometry — a plain "step scales with
        // distance from centre" formula draws the *opposite* nesting and the sibling bend lanes
        // cross each other's stubs). Anchoring on `anchor` alone (rather than requiring it to
        // exist) mirrors the coordinate loop just above: a face with a `fan_lane` claim always has
        // an aligned/trunk companion too (`classify`'s own `fan_eligible` doc — the trigger is
        // "does this source already have a flow-aligned sibling"), so `anchor` is always `Some`
        // whenever this loop finds anything to do, but the `let-else` here stays defensive rather
        // than assuming that invariant.
        if let Some(anchor) = anchor {
            // (edge id, this port's own rank from the centre — `route_fan_lane`'s old, still-used-
            // as-a-fallback `k`) for every fan-lane sibling on this face, split by which side of
            // the centre they sit on — `3a`'s own reference (`設定のルール`'s ten-way fanout) nests
            // both halves against the *larger* half's own outer rank, not each half's own count
            // (its `y=306`, one step in on the five-member half, and `y=338`, one step in on the
            // four-member half, bend to the identical depth), so the two halves cannot be nested
            // independently.
            let mut ranks: Vec<(String, f64)> = Vec::new();
            let mut outer_neg = 0.0_f64;
            let mut outer_pos = 0.0_f64;
            for (i, claim) in claims.iter().enumerate() {
                if !claim.fan_lane {
                    continue;
                }
                let offset = (i as f64 - anchor as f64) * PORT_SPACING;
                let k = (offset.abs() / PORT_SPACING).round().max(1.0);
                if offset < 0.0 {
                    outer_neg = outer_neg.max(k);
                } else {
                    outer_pos = outer_pos.max(k);
                }
                ranks.push((claim.edge_id.clone(), k));
            }
            let outer_rank = outer_neg.max(outer_pos);
            if outer_rank > 0.0 {
                for (edge_id, k) in ranks {
                    // Reflects `k` (1 = closest to the centre) around the face's own outer rank —
                    // the port that used to get the *smallest* step (closest to centre, `k = 1`)
                    // now gets the deepest bend (`PORT_CLEARANCE * (outer_rank + 1)`), and the
                    // outermost port (`k = outer_rank`) gets the shallowest one `PORT_CLEARANCE`
                    // can offer above the plain port-clearance minimum (`PORT_CLEARANCE * 2`).
                    let nested_k = outer_rank + 2.0 - k;
                    fan_step.insert(edge_id, PORT_CLEARANCE * nested_k);
                }
            }
        }

        // The flat run this face needs: twice the widest offset any port actually sits at
        // (`max_abs_offset`, above), plus `PORT_CLEARANCE` clear beyond that port's own corner —
        // §10-1 item 1: "ポートは角から8px以上". For the un-anchored, always-symmetric grid this
        // is exactly the old `(n-1)*PORT_SPACING + 2*PORT_CLEARANCE` (the widest offset is always
        // `(n-1)/2 * PORT_SPACING` from centre on both sides there), so nothing changes for it;
        // an anchored, asymmetric grid (`max_abs_offset`'s own doc — a trunk claim off the array's
        // geometric centre) needs however much *that* side alone reaches, not the old formula's
        // count-based guess, which undercounted it and left the outermost port sitting flush on
        // the node's own corner instead of `PORT_CLEARANCE` px inside it (`docs/STATUS.md`'s own
        // ★未修正 entry: RS's own four-way merge in `samples/mermaid.ja.md`'s "大きさ" flowchart,
        // `ページ描画→ラスタライズ`'s target port landing exactly on the box's own top edge).
        // §10-5 S1: a start/end marker never grows to clear a port. `state::spec_of`'s own
        // per-transition duplication (this module's own doc on `marker_anchored`, `classify`)
        // guarantees `n == 1` and `max_abs_offset == 0.0` for every marker face that reaches
        // here, so `required_flat` below would still only ever ask for `2 * PORT_CLEARANCE`
        // (16px) — but a circle has no corner to clear in the first place, and growing one to
        // satisfy a rectangular face's clearance rule turns the dot into an oval, which is a
        // real regression `non_flowchart_diagrams_ignore_mermaid_routing`'s own sibling test
        // caught (`orthogonal_state_markers_do_not_grow`): the marker's box grew from its
        // fixed 14px/16px diameter (`shapes::size`'s own `Glyph::StateStart`/`StateEnd` rule)
        // to 16px/18.5px. So a marker face is skipped here entirely — its coordinate is
        // already written above (`source_coord`/`target_coord`, always the face centre for
        // `n == 1`), and `required_size` simply never gets an entry for it, the same as any
        // node whose face carries no claim at all.
        if matches!(node.shape, Glyph::StateStart | Glyph::StateEnd) {
            continue;
        }
        let required_flat = if n == 0 {
            0.0
        } else {
            2.0 * (max_abs_offset + PORT_CLEARANCE)
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
        fan_step,
    }
}

/// §10-1 item 4's 8px lane stagger: every perimeter-routed edge — a genuine back edge (`reverse`,
/// minus a self-loop; a collision-fallback forward edge, `staircase`, stays local —
/// [`route_with_ports`]'s own doc) — gets its own lane index, assigned in a stable order (edge id,
/// not declaration or `HashMap` iteration order) so the same source always draws the same picture.
/// [`route_flowchart`] turns a lane index into an actual ring (`PERIMETER_MARGIN +
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
            (s.reverse && source.id != target.id).then_some(e.id)
        })
        .collect();
    ids.sort_unstable();
    ids.into_iter().enumerate().map(|(i, id)| (id, i)).collect()
}

/// A subgraph frame, reduced to the four facts [`classify`]/[`evict`]/[`route_with_ports`] ever
/// actually read off a [`PlacedNode`] — `id`, `center`, `size`, and a `shape` `evict`'s chamfer
/// check can compare against [`Glyph::ChamferedRect`] — so a **cluster-anchored edge** (`one -->
/// two`, where `one`/`two` name subgraphs rather than nodes) can be routed by exactly the same
/// `classify`/`evict`/`route_with_ports` pipeline a node-to-node edge already goes through, instead
/// of this module growing a second, parallel box type. `shape` is always [`Glyph::default`] (a
/// plain rectangle, no chamfer): a frame is drawn as a plain rect under every routing mode, unlike
/// a decision node, which only chamfers under orthogonal routing — so a cluster face never gets the
/// chamfer allowance [`evict`]'s `required_size` computation adds for a real chamfered node.
///
/// `label`/`panel`/`series`/`mark`/`style` are never read by anything this box reaches (every
/// caller only ever asks a `PlacedNode` for its outline), so they are filled with the same
/// "nothing here" values [`PlacedNode`]'s own node-glyph construction uses when a field does not
/// apply.
fn cluster_as_node(c: &PlacedCluster) -> PlacedNode {
    PlacedNode {
        id: c.id.clone(),
        shape: Glyph::default(),
        center: c.center.clone(),
        size: c.size,
        label: Label::measure(""),
        panel: None,
        series: None,
        mark: None,
        style: None,
    }
}

/// Every cluster in `clusters`, boxed by [`cluster_as_node`] — [`build_by_id`]'s other half of the
/// id → box lookup a cluster-anchored edge's ends resolve through.
fn cluster_node_boxes(clusters: &[PlacedCluster]) -> Vec<PlacedNode> {
    clusters.iter().map(cluster_as_node).collect()
}

/// The id → box lookup every routing entry point ([`route_flowchart`], [`avoid_label_plates`],
/// [`insert_crossing_gaps`]) resolves an [`EligibleEdge::source`]/[`EligibleEdge::target`] through:
/// every real node from `nodes`, plus every cluster box from `cluster_boxes` under its own id.
/// `mod.rs`'s own `end_of` is what decides whether an edge's written endpoint names a node or a
/// subgraph — this is the only place downstream that has to know both kinds of box exist at all;
/// everything past this lookup (`classify`, `evict`, `route_with_ports`, ...) just sees a
/// `&PlacedNode` and does not care which kind it came from.
///
/// A real node wins any id collision (`.entry().or_insert`, not overwrite) — defensive only, since
/// a real flowchart cannot declare a node and a subgraph under the same id, but a lookup that must
/// never panic should not depend on that being true.
///
/// **Deliberately not** what a route's own *collision* test (`shape_crosses_a_node`'s `nodes`
/// argument, and [`content_bounds`]'s `clusters` argument) is built from: a cluster frame is not an
/// obstacle a node-to-node edge has to avoid (§10-1 item 4's own "辺と枠の交差は隙間なし（直交して跨
/// ぐだけ）" — a frame a route merely crosses, not naming as an endpoint, stays out of every
/// obstacle test unchanged from before this function existed), so this lookup — used only to answer
/// "what box does this edge's *own* end meet" — is kept a separate map from the ones that answer
/// "what else is in the way".
fn build_by_id<'a>(
    nodes: &'a [PlacedNode],
    cluster_boxes: &'a [PlacedNode],
) -> HashMap<&'a str, &'a PlacedNode> {
    let mut by_id: HashMap<&str, &PlacedNode> = nodes.iter().map(|n| (n.id.as_str(), n)).collect();
    for cb in cluster_boxes {
        by_id.entry(cb.id.as_str()).or_insert(cb);
    }
    by_id
}

/// Routes every node-to-node **and cluster-anchored** edge in one flowchart under `[ui]
/// mermaid_routing = "konoma-orthogonal"`, running [`classify`] and [`evict`] once each over the
/// whole diagram before building any polyline — the two-pass shape the module doc describes.
///
/// `nodes` is the diagram's placed nodes from *this* layout pass; `clusters` is every subgraph
/// frame from the same pass — read twice over: [`content_bounds`] still needs it whole (§10-1 item
/// 4's perimeter lane has to clear a frame the way it clears a node), and [`cluster_node_boxes`]
/// turns it into the routable boxes a cluster-anchored [`EligibleEdge`] resolves its `source`/
/// `target` against (`mod.rs`'s own `end_of` decides, per edge end, whether that end's `source`/
/// `target` string names a node id or a cluster id).
///
/// A cluster face is never grown to fit its ports the way [`RoutedFlowchart::required_size`] grows
/// a real node's: `lay_out_spec`'s growth retry (`apply_growth`) only ever looks a size up by a real
/// `spec.nodes` id, so an entry this pass's `evict` writes under a cluster's id is silently never
/// read — a deliberate simplification (`docs/FEATURE-MERMAID-RENDERER.md` §10-1 item 1's own
/// "枠は元々広いので分配のみで足りるはず"), not an oversight: a subgraph frame's own size is *derived*
/// from dagre's layout of its members, not a quantity `lay_out_spec_pass` hands dagre directly the
/// way a node's `width`/`height` is, so "grow this box" has no single obvious lever to pull the way
/// it does for a node. Should a real corpus source ever need more than a frame's own width already
/// offers, `evict`'s 16px-spacing ports simply run past [`PORT_CLEARANCE`] of the frame's corner —
/// no different from what a face too narrow for its claims already risked before this function knew
/// about clusters at all.
pub fn route_flowchart(
    direction: Direction,
    nodes: &[PlacedNode],
    clusters: &[PlacedCluster],
    edges: &[EligibleEdge],
    chain_next: &HashMap<String, String>,
) -> RoutedFlowchart {
    let cluster_boxes = cluster_node_boxes(clusters);
    let by_id = build_by_id(nodes, &cluster_boxes);

    let mut shapes: Vec<Option<EdgeShape>> = edges
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

    let eviction = evict(direction, &by_id, edges, &shapes, chain_next);
    // §10-3 item 10's own "ホップ x の入れ子": run only once every sibling's exact port coordinate
    // is known (`eviction`, just above) — `nest_merge_target_hops`'s own doc explains why an earlier
    // attempt at this exact spacing, judged from node boxes alone, cannot see a sibling at all.
    // Never re-runs `evict` itself: only `EdgeShape::rank_lane_bend` changes here, a field `evict`
    // never reads (only `source_side`/`target_side`/`aligned`/`fan_lane` decide port placement).
    nest_merge_target_hops(direction, edges, &by_id, &mut shapes, &eviction);

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
            eviction.fan_step.get(edge.id).copied(),
        );
        points.insert(edge.id.to_string(), routed);
    }

    let pass_through_eligible: std::collections::HashSet<String> = edges
        .iter()
        .zip(&shapes)
        .filter_map(|(e, s)| {
            s.as_ref()
                .filter(|s| is_pass_through_shape(s))
                .map(|_| e.id.to_string())
        })
        .collect();

    RoutedFlowchart {
        points,
        required_size: eviction.required_size,
        pass_through_eligible,
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
/// "Adjacent" means the next rank some *real* node actually occupies, not `r + 1` — dagre's own
/// `makeSpaceForEdgeLabels` (`crate::preview::mermaid::layout`'s `mod.rs`, the `minlen *= 2`
/// comment there) unconditionally doubles every edge's `minlen` to reserve a rank for its label
/// proxy, so a real node-to-node edge's two ends are *always* two ranks apart in `node_rank` — a
/// literal `r + 1` check can never match one. `node_rank` only ever holds a rank for a real
/// [`PlacedNode`] (never a label-proxy dummy, `mod.rs`'s own `node_rank` construction filters to
/// `nodes`), so the sorted, deduplicated set of its values is already exactly "every rank a real
/// node sits on" — the next entry in that sorted set, whatever the gap to it, is the adjacency
/// this function means. Found and fixed 2026-09-01: confirmed by checking a real diagram's own
/// `node_rank` (`branch`, no labels or subgraphs at all: `A:0 B:2`), not assumed from the
/// `minlen *= 2` comment alone.
///
/// Mutates `nodes` in place and is meant to run once, right after a layout pass places them and
/// before anything downstream (frames, `route_flowchart`) reads a position from them — see the
/// module doc's "Lane alignment moves nodes" section for why the order matters.
///
/// `node_rank` is dagre's own rank per node id; `candidates` is every edge eligible to be
/// considered for a lane — the caller's job to have already excluded a cluster-anchored edge (one
/// whose written endpoint is a block, not the real node dagre routed against), since this module
/// has no opinion of its own about clusters.
///
/// Returns every node's own cross-axis delta (`final - dagre's original`, cross axis only — this
/// function never touches the flow axis), for every node this pass actually moved (by more than
/// [`EPS`]; a node it left alone is simply absent, not present at `0.0`). `mod.rs` needs this for
/// exactly one thing once the fix above made this function fire for real: a self-loop's raw dagre
/// waypoints (`EligibleEdge::raw`) are read from `g`, which this function never touches, straight
/// from `layout()`'s own `position_self_edges` — so once a self-loop's owner actually moves here,
/// `raw` is stale relative to it unless the caller applies the same delta back
/// ([`shift_cross`] is the one place that knows how, direction-aware). Reproduced directly before
/// this was added: a self-loop whose owner moved ~137px under a one-sided wide-sibling push drew
/// with its ports correctly on the node's current boundary but its interior bump landing ~176px
/// away in open space — see
/// `render::tests::self_loop_routes_correctly_across_lane_alignment_stress_cases`'s own doc.
#[must_use = "a moved node's self-loop raw waypoints go stale unless this delta is applied back — see this function's own doc"]
pub fn align_straight_lanes(
    direction: Direction,
    nodes: &mut [PlacedNode],
    node_rank: &HashMap<String, i32>,
    candidates: &[(String, String)],
) -> (HashMap<String, f64>, HashMap<String, String>) {
    align_straight_lanes_with(direction, nodes, node_rank, candidates, None)
}

/// [`align_straight_lanes`], with its own "greedy straight-lane selection" pass (this function's
/// own doc on the block below) optionally skipped in favour of a selection the caller already
/// trusts. `mod.rs`'s own `regroup_fan_lanes` (`docs/FEATURE-MERMAID-RENDERER.md` §10-3, "ファン列
/// 内の並び順") is the one caller that ever passes `Some`: it runs this function once to *learn*
/// `chain_next` on the layout dagre/`pull_back_fan_ranks` handed it, regroups the rank order by
/// colour, and needs the alignment/overlap-resolution machinery below to run *again* on the new
/// order without re-deriving the selection — because it cannot. Once a fan's trunk sits exactly at
/// its own group's centre index (`regroup_fan_lanes`'s own rule), it and its immediate,
/// same-coloured neighbour are, by construction, equidistant from the fan's numeric cross-axis
/// median (the midpoint of two points is always equidistant from both, regardless of how far apart
/// they are) — the selection loop's own "distance from each source's own sibling median" tie-break
/// (this function's own comment on it, a few lines down) can never discriminate between them, and
/// the tie-breaks *after* it (ascending target cross, then id) both happen to favour the neighbour
/// over almost any trunk id in this corpus, re-selecting a *different* trunk than the one `mod.rs`
/// just spent a whole pass giving room to. Passing the first call's own `next` back in sidesteps
/// the ambiguity at its root instead of trying to out-guess it with more tie-break keys: the
/// selection was already correct (rank membership and "does it keep going" do not change under a
/// pure cross-axis permutation), only its *geometry* needed a second pass.
pub(super) fn align_straight_lanes_with(
    direction: Direction,
    nodes: &mut [PlacedNode],
    node_rank: &HashMap<String, i32>,
    candidates: &[(String, String)],
    preselected: Option<&HashMap<String, String>>,
) -> (HashMap<String, f64>, HashMap<String, String>) {
    if nodes.len() < 2 {
        return (HashMap::new(), HashMap::new());
    }
    let id_index: HashMap<String, usize> = nodes
        .iter()
        .enumerate()
        .map(|(i, n)| (n.id.clone(), i))
        .collect();
    let initial_cross: Vec<f64> = nodes.iter().map(|n| cross(direction, &n.center)).collect();

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
    let next: HashMap<String, String> = if let Some(pre) = preselected {
        // The caller already trusts this selection (`align_straight_lanes_with`'s own doc on
        // why re-deriving it here can pick a different trunk) — skip straight to "build chains".
        pre.clone()
    } else {
        let mut used_out: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut used_in: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut next: HashMap<String, String> = HashMap::new();

        // §10-3 item 7 ("直進レーンは図を貫く幹"): every id that is *some* candidate's own source —
        // i.e. has at least one further out-edge of its own, at any rank. Read purely off `candidates`
        // (never off a chain already built — nothing here has been selected yet), this is the "does
        // picking this target let the trunk keep going, or does it dead-end the lane right here"
        // question a plain per-window greedy pass has no way to ask; `3a`'s own reference geometry
        // (`docs/mermaid-theme/handoff/round3-Konoma-Flowchart-Routing.dc.html`) is exactly what this
        // fixes: `設定のルール`'s ten same-rank targets are otherwise tied on every existing key (one
        // shared source, so the first key never discriminates at all, and the *original* cross
        // coordinates a hand-authored mockup and dagre's own barycenter layout assign the ten hardly
        // ever agree on which one sorts smallest), and only `ブロックモデル` — the one target that
        // itself goes on to `mermaid`/`数式` — keeps the seven-segment spine (`ファイル → 設定のルール
        // → ブロックモデル → mermaid → ラスタライズ → セルに合わせる → 端末 → 画像プロトコル`) whole
        // rather than terminating it at whichever leaf a coordinate happened to sort first.
        let continues: std::collections::HashSet<&str> =
            candidates.iter().map(|(s, _)| s.as_str()).collect();

        let mut ranks: Vec<i32> = node_rank.values().copied().collect();
        ranks.sort_unstable();
        ranks.dedup();
        for window in ranks.windows(2) {
            let (r, next_r) = (window[0], window[1]);
            let mut pair_candidates: Vec<&(String, String)> = candidates
                .iter()
                .filter(|(s, t)| node_rank.get(s) == Some(&r) && node_rank.get(t) == Some(&next_r))
                .collect();
            // §10-3 item 2 ("幹末端のタイブレーク"): for a source whose whole fan-out is leaves (every
            // candidate ties on "continues" below — none of them goes on to extend the chain further),
            // `3a`'s own reference geometry picks neither extreme but the *middle* of the fan: `端末`'s
            // three same-rank targets (`圧縮転送`/`画像プロトコル`/`ハーフブロック`, `docs/mermaid-theme/
            // handoff/round3-Konoma-Flowchart-Routing.dc.html`'s `3a` section) keep the spine running
            // through `画像プロトコル` — the cross-order *middle* one — not `圧縮転送`, the smallest-
            // cross target the plain ascending tie-break below would otherwise pick outright. Distance
            // from each source's own sibling median, smaller wins; a fan of exactly two candidates is
            // always tied here by construction (both sit equally far from their shared midpoint), so
            // this key is a genuine no-op for every existing two-candidate fixture and only ever
            // discriminates a fan of three or more.
            let mut targets_by_source: HashMap<&str, Vec<f64>> = HashMap::new();
            for (s, t) in &pair_candidates {
                targets_by_source
                    .entry(s.as_str())
                    .or_default()
                    .push(cross(direction, &nodes[id_index[t]].center));
            }
            for v in targets_by_source.values_mut() {
                v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            }
            let median_of = |s: &str| -> f64 {
                let v = &targets_by_source[s];
                let n = v.len();
                if n % 2 == 1 {
                    v[n / 2]
                } else {
                    (v[n / 2 - 1] + v[n / 2]) / 2.0
                }
            };
            // §10-3 item 7's own trunk-preserving order, ahead of every pre-existing key: a candidate
            // whose *source* is already mid-chain (`used_in` — some earlier window already selected an
            // edge landing on it) wins first, so the windowed pass keeps extending the chain it is
            // already building instead of a fresh window's plain coordinate tie-break cutting it off —
            // found on `3a`'s own reference: without this, `MM → RS` (already selected) loses `RS`'s
            // own onward pick to `IM → FIT` purely because `IM`'s cross coordinate sorts first, even
            // though `RS` (this window's true continuation of the chain already built) targets the
            // very same `FIT`. Only *then* does "タイは上・左優先＝cross座標の小さい方" (the source's
            // own cross coordinate) apply — so the topmost/leftmost source is offered its pick first
            // among candidates that are equally fresh (neither already mid-chain); then, before falling
            // to the target's own cross coordinate, whether *this* target continues on again (a target
            // that itself has a further out-edge wins over one that does not, so a source with several
            // equally-tied candidates always extends the longest chain it can); then id, for a fully
            // deterministic order a `HashMap`-built candidate list would not otherwise have.
            pair_candidates.sort_by(|(s1, t1), (s2, t2)| {
                used_in
                    .contains(s2.as_str())
                    .cmp(&used_in.contains(s1.as_str()))
                    .then_with(|| {
                        let sc1 = cross(direction, &nodes[id_index[s1]].center);
                        let sc2 = cross(direction, &nodes[id_index[s2]].center);
                        sc1.partial_cmp(&sc2).unwrap_or(std::cmp::Ordering::Equal)
                    })
                    .then_with(|| {
                        continues
                            .contains(t2.as_str())
                            .cmp(&continues.contains(t1.as_str()))
                    })
                    .then_with(|| {
                        let tc1 = cross(direction, &nodes[id_index[t1]].center);
                        let tc2 = cross(direction, &nodes[id_index[t2]].center);
                        let d1 = (tc1 - median_of(s1)).abs();
                        let d2 = (tc2 - median_of(s2)).abs();
                        d1.partial_cmp(&d2).unwrap_or(std::cmp::Ordering::Equal)
                    })
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
        next
    };

    // --- build chains: a head is a selected source that is nobody's selected target ------------
    //
    // Derived straight from `next` rather than kept as the selection loop's own `used_out`/
    // `used_in` sets, which only exist inside the `preselected.is_none()` branch above: a
    // preselected `next` is exactly as valid a selection as one this function built itself (this
    // function's own doc on `align_straight_lanes_with`), so "head" and "selected target" mean the
    // same thing regardless of which branch produced `next`.
    let used_out: std::collections::HashSet<String> = next.keys().cloned().collect();
    let used_in: std::collections::HashSet<String> = next.values().cloned().collect();
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
    // Every chain member's own index and its chain's average — captured here, right after each
    // member's `center` was set to it, so the overlap-resolution pass below has each chain
    // member's *intended* position on hand even after that pass has moved it away from it.
    // `docs/STATUS.md`'s own ★未修正 entry (2026-09-01, before this fix) is the bug this exists to
    // close: the forward sweep below can push a chain member — `3a`'s own `mermaid`, crowded by
    // `設定のルール`'s nine other same-rank fanout targets — off this exact position, and nothing
    // downstream ever tried to reclaim it.
    let mut chain_desired: HashMap<usize, f64> = HashMap::new();
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
            chain_desired.insert(i, avg);
        }
    }

    // --- resolve overlaps: one forward sweep per rank, in the fixed original order --------------
    //
    // §10-1 item 1's "重なった側を押し出して従来の最小間隔を維持する（ランク内の並び順は変えない）"
    // — `ORTHO_NODE_SEP` is `align_straight_lanes`'s own caller-side `nodesep` (`mod.rs`'s
    // `lay_out_spec_pass` only ever calls this function under `Routing::Orthogonal`, which is the
    // one mode `ORTHO_NODE_SEP` is dagre's actual `nodesep` for — §10-3 item 9's own density fix),
    // reused so a push respects the same gap the rank was laid out with in the first place,
    // whether or not either node in a pair moved at all.
    for ids in by_rank.values() {
        let mut prev_far_edge: Option<f64> = None;
        for &i in ids {
            let half = cross_extent(direction, &nodes[i]);
            let mut c = cross(direction, &nodes[i].center);
            if let Some(prev_edge) = prev_far_edge {
                let min_c = prev_edge + super::ORTHO_NODE_SEP + half;
                if c < min_c {
                    c = min_c;
                    let flow_v = flow(direction, &nodes[i].center);
                    nodes[i].center = make(direction, flow_v, c);
                }
            }
            prev_far_edge = Some(c + half);
        }
    }

    // --- reclaim a chain member's own position when its immediate predecessor already allows it -
    //
    // The forward sweep above only ever pushes a node *later* (§10-1 item 1's own "ランク内の並び
    // 順は変えない" — it cannot reorder, so a node with something in the way ahead of it can only
    // move further along, never behind). That is correct for an ordinary node (it has no position
    // of its own to defend), but a chain member's whole point is a *specific* shared coordinate —
    // §10-3 item 7's own spine, `3a`'s seven-segment centreline — so this pass gives it one more
    // chance: for each rank, in the same fixed original order, if a chain member was pushed past
    // its own `chain_desired` coordinate but its immediate predecessor's own (already final) far
    // edge leaves enough room for it to sit exactly on `chain_desired` without moving that
    // predecessor at all, it is moved back there.
    //
    // Deliberately does **not** cascade the pull back through a predecessor that does not already
    // have the room (an earlier version of this pass did, moving as many ordinary predecessors as
    // it took) — found, by the corpus's own `orthogonal_no_edge_crosses_its_own_endpoint_across_
    // the_whole_corpus` test, to reopen exactly the raw-waypoint staleness problem
    // `align_straight_lanes`'s own doc already describes for a self-loop: `branch`'s own `D -> B`
    // (a genuine, non-self-loop `reverse` edge — `route_perimeter`'s own ring is built from
    // *current* node positions, so it is not `raw`-waypoint staleness in the sense that doc means,
    // but the ring itself shifts when a node this pass moves sits inside it) re-entered `D`'s own
    // box once `D` — a chain member — was pushed back by cascading through its own rank-mate `C`.
    // Reclaiming only when the immediate predecessor already has slack (`ORTHO_NODE_SEP` was not
    // tight to begin with) keeps every node this pass touches to *one* — the member being reclaimed
    // itself, never a neighbour — which is enough to fix `3a`'s own `mermaid` (dumped and confirmed
    // visually: `設定のルール`'s ten-way fanout leaves just enough slack next to it) without ever
    // moving a second node whose own routing might depend on where it already was.
    for ids in by_rank.values() {
        for (pos, &i) in ids.iter().enumerate() {
            let Some(&desired) = chain_desired.get(&i) else {
                continue;
            };
            let current = cross(direction, &nodes[i].center);
            if current <= desired + EPS {
                continue; // already at (or before) its own desired spot — nothing to reclaim.
            }
            let max_far = desired - cross_extent(direction, &nodes[i]) - super::ORTHO_NODE_SEP;
            let predecessor_allows = match pos.checked_sub(1).map(|p| ids[p]) {
                None => true, // first in the rank — nothing behind it to leave room against.
                Some(j) => {
                    let far_j =
                        cross(direction, &nodes[j].center) + cross_extent(direction, &nodes[j]);
                    far_j <= max_far + EPS
                }
            };
            if predecessor_allows {
                let flow_v = flow(direction, &nodes[i].center);
                nodes[i].center = make(direction, flow_v, desired);
            }
        }
    }

    // Every node this pass actually moved, as its own cross-axis delta — see this function's own
    // doc for why `mod.rs` needs it (a self-loop's stale raw waypoints).
    let deltas = nodes
        .iter()
        .enumerate()
        .filter_map(|(i, n)| {
            let delta = cross(direction, &n.center) - initial_cross[i];
            (delta.abs() > EPS).then(|| (n.id.clone(), delta))
        })
        .collect();

    // §10-3 item 1's own robustness note: `next` — every selected chain edge, source id to target
    // id, *before* the overlap-resolution sweep above ran — is handed back separately from a purely
    // geometric alignment check (`cross` coordinates equal within `EPS`, downstream), because the
    // two can disagree. The overlap sweep's own "ランク内の並び順は変えない" constraint
    // (this function's own doc) means it can only ever push a later member of a crowded rank
    // *forward*, never make room by moving an earlier one back — so a chain member deep in a
    // crowded rank (found on `3a`'s own `ブロックモデル`, sitting among `設定のルール`'s other nine
    // same-rank targets) can be selected here and then pushed clear off the chain's own computed
    // average by that later pass, without this function ever un-selecting it. A purely geometric
    // re-check downstream (`cross` coordinates equal within `EPS`) would then wrongly conclude the
    // edge was never a trunk edge at all. Returning the *selection* alongside the *geometry* lets
    // `route_flowchart` trust "this pass picked it" even when the final coordinates no longer agree
    // — `route_flowchart`'s own doc on `chain_next` explains the consequence (the trunk edge draws
    // as a 2-bend `fan_lane` shape instead of a 0-bend `aligned` one) rather than falling all the
    // way back to the old, pre-round-3 cross-face branch shape.
    //
    // The map (source → target), not just the source-id set `used_out` used to be, is what
    // §10-3 item 1's port-centring fix (`evict`'s own doc on its `chain_next` parameter) needs: a
    // face can carry several of the chain source's siblings, and only the *one* claim whose own
    // (source, target) pair matches an entry here is the trunk edge rule 1 reserves the centre port
    // for — a source-id-only set could not tell that claim apart from any other sibling leaving the
    // same node.
    (deltas, next)
}

/// `p`, shifted by `delta` along `direction`'s cross axis — [`align_straight_lanes`]'s own return
/// value, applied to a point that function does not own directly. `mod.rs` is the one caller: a
/// self-loop's raw dagre waypoints (`EligibleEdge::raw`) live outside `PlacedNode`, so alignment
/// cannot correct them itself, but they need exactly this same shift once their owner moves.
pub fn shift_cross(direction: Direction, p: &Point, delta: f64) -> Point {
    make(direction, flow(direction, p), cross(direction, p) + delta)
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

/// Half of `node`'s flat run along `side`'s tangent axis — the same bound `evict`'s own sizing
/// (`required_flat + chamfer_allowance`, above) reserved when it grew the node's box to fit its
/// ports. [`push_outward`]'s give-up threshold: a coordinate past this sits in the node's curved
/// (or, for [`Glyph::ChamferedRect`], chamfered) corner, not on the flat run a pushed port needs.
fn face_flat_half_extent(node: &PlacedNode, side: Side) -> f64 {
    let full = match side {
        Side::Top | Side::Bottom => node.size.w,
        Side::Left | Side::Right => node.size.h,
    };
    let chamfer_allowance = if node.shape == Glyph::ChamferedRect {
        2.0 * shapes::CHAMFER
    } else {
        0.0
    };
    (full - chamfer_allowance).max(0.0) / 2.0
}

/// Every *other* edge's port on `node_id`'s `side` face, as tangent coordinates —
/// [`push_outward`]'s collision list. Read from `points`'s current state, which may already carry
/// earlier edges' pushes from this same forward pass through `avoid_label_plates` — the same "not
/// the value from before the fix" rule that function's own doc states for its plate bookkeeping.
fn ports_on_face(
    node_id: &str,
    side: Side,
    exclude_edge_id: &str,
    edges: &[EligibleEdge],
    shapes: &[Option<EdgeShape>],
    points: &HashMap<String, Vec<Point>>,
) -> Vec<f64> {
    let mut out = Vec::new();
    for (edge, shape) in edges.iter().zip(shapes) {
        if edge.id == exclude_edge_id {
            continue;
        }
        let Some(shape) = shape else { continue };
        let Some(pts) = points.get(edge.id) else {
            continue;
        };
        if pts.is_empty() {
            continue;
        }
        if edge.source == node_id && shape.source_side == side {
            out.push(tangent_coord(side, &pts[0]));
        }
        if edge.target == node_id && shape.target_side == side {
            out.push(tangent_coord(side, &pts[pts.len() - 1]));
        }
    }
    out
}

/// `cur`, pushed [`PORT_SPACING`] further from `node`'s own `side` face centre — §10-1 item 3:
/// "ポートをさらに16px外へ". "Further" means away from the centre in whichever direction the port
/// already sat on (or, for a port that started exactly centred, outward in the positive tangent
/// direction — the spec does not disambiguate a port with no existing side to keep, and this stays
/// deterministic rather than arbitrary-per-call).
///
/// A single `PORT_SPACING` step can land exactly on a port another edge already occupies on the
/// same face: three ports evicted to `-PORT_SPACING`/`0`/`+PORT_SPACING` push the centre one onto
/// the outer one's own coordinate. So this keeps stepping `PORT_SPACING` further, past every
/// coordinate in `occupied` (compared within [`EPS`]), until it lands somewhere free. If that walks
/// past the face's own flat run ([`face_flat_half_extent`] — the same bound `evict` sized the
/// node's box to hold), it gives up and returns `cur` unchanged rather than push the port into the
/// node's curved or chamfered corner: leaving the plate crossing this pass was trying to fix is
/// better than creating a new one ([`avoid_label_plates`]'s own doc already accepts a documented
/// approximation over a proven fixpoint, for the same reason).
fn push_outward(node: &PlacedNode, side: Side, cur: f64, occupied: &[f64]) -> f64 {
    let center = face_center_coord(node, side);
    let dir = if cur >= center { 1.0 } else { -1.0 };
    let half_extent = face_flat_half_extent(node, side);
    let mut candidate = cur;
    loop {
        candidate += dir * PORT_SPACING;
        if (candidate - center).abs() > half_extent {
            return cur;
        }
        if occupied.iter().all(|&o| (o - candidate).abs() > EPS) {
            return candidate;
        }
    }
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

/// §10-1 item 4's own "並走…は8pxずつずらしてレーン分離" — [`PERIMETER_LANE_SPACING`], the same
/// distance the perimeter lane already staggers by — applied to two *unrelated* (sharing no node —
/// a shared node is stage 1-4's own territory, eviction already spaces those 16px apart on the
/// shared face) detour-shaped edges ([`EdgeShape::reverse`]/[`EdgeShape::staircase`], minus a
/// self-loop) whose local routes happen to coincide.
///
/// Before 2026-09-01 this case could not arise: every detour shared the same perimeter-lane
/// mechanism, so [`perimeter_lanes`]'s own stagger already kept every one of them apart (recorded,
/// with an audit across the whole corpus finding zero instances, as
/// `render::tests::no_two_unrelated_edges_coincidentally_overlap_on_the_same_axis`'s own history).
/// Once a `staircase` edge went back to a local, `raw`-based route
/// ([`route_with_ports`]'s own doc) it lost that shared bookkeeping, and two unrelated ones can
/// genuinely land on the same segment — reproduced directly: `amp-chain`'s `A->D` and `B->C`, both
/// collision-fallback, both local, both routed onto the identical vertical run.
///
/// A single forward pass over every unordered pair of detour edges, in a stable (sorted edge id)
/// order: for the first coinciding segment found, the higher-id edge's whole overlapping straight
/// run (not just the flagged segment's own two points, the same "move the run, not the segment"
/// reasoning [`clear_local_route`] uses) is nudged [`PERIMETER_LANE_SPACING`] px along the
/// perpendicular axis. Bounded to a handful of passes rather than a fixpoint search, the same
/// "monotonic retry, defensive cap" shape this module's other local-collision fixes already use.
pub fn separate_coincident_detours(
    direction: Direction,
    nodes: &[PlacedNode],
    clusters: &[PlacedCluster],
    edges: &[EligibleEdge],
    points: &mut HashMap<String, Vec<Point>>,
) {
    let cluster_boxes = cluster_node_boxes(clusters);
    let by_id = build_by_id(nodes, &cluster_boxes);
    let mut detour_ids: Vec<&str> = edges
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
            ((shape.reverse || shape.staircase || is_flow_flow_bend(&shape))
                && source.id != target.id)
                .then_some(e.id)
        })
        .collect();
    detour_ids.sort_unstable();

    const MAX_PASSES: usize = 4;
    for _ in 0..MAX_PASSES {
        let mut fix: Option<(&str, f64, f64, bool)> = None; // (edge to nudge, old c, new c, horizontal?)
        'search: for (i, &id_a) in detour_ids.iter().enumerate() {
            for &id_b in &detour_ids[i + 1..] {
                let (Some(ea), Some(eb)) = (
                    edges.iter().find(|e| e.id == id_a),
                    edges.iter().find(|e| e.id == id_b),
                ) else {
                    continue;
                };
                if ea.source == eb.source
                    || ea.source == eb.target
                    || ea.target == eb.source
                    || ea.target == eb.target
                {
                    continue; // shares a node — stage 1-4's own territory, not "unrelated"
                }
                let (Some(pa), Some(pb)) = (points.get(id_a), points.get(id_b)) else {
                    continue;
                };
                for wa in pa.windows(2) {
                    for wb in pb.windows(2) {
                        let vert_a = (wa[0].x - wa[1].x).abs() < EPS;
                        let vert_b = (wb[0].x - wb[1].x).abs() < EPS;
                        if vert_a && vert_b && (wa[0].x - wb[0].x).abs() < EPS {
                            let (y0a, y1a) = (wa[0].y.min(wa[1].y), wa[0].y.max(wa[1].y));
                            let (y0b, y1b) = (wb[0].y.min(wb[1].y), wb[0].y.max(wb[1].y));
                            if y0a < y1b - EPS && y0b < y1a - EPS {
                                fix =
                                    Some((id_b, wb[0].x, wb[0].x + PERIMETER_LANE_SPACING, false));
                                break 'search;
                            }
                        }
                        let horiz_a = (wa[0].y - wa[1].y).abs() < EPS;
                        let horiz_b = (wb[0].y - wb[1].y).abs() < EPS;
                        if horiz_a && horiz_b && (wa[0].y - wb[0].y).abs() < EPS {
                            let (x0a, x1a) = (wa[0].x.min(wa[1].x), wa[0].x.max(wa[1].x));
                            let (x0b, x1b) = (wb[0].x.min(wb[1].x), wb[0].x.max(wb[1].x));
                            if x0a < x1b - EPS && x0b < x1a - EPS {
                                fix = Some((id_b, wb[0].y, wb[0].y + PERIMETER_LANE_SPACING, true));
                                break 'search;
                            }
                        }
                    }
                }
            }
        }
        let Some((id, old_c, new_c, horizontal)) = fix else {
            break;
        };
        if let Some(pts) = points.get_mut(id) {
            for p in pts.iter_mut() {
                if horizontal && (p.y - old_c).abs() < EPS {
                    p.y = new_c;
                } else if !horizontal && (p.x - old_c).abs() < EPS {
                    p.x = new_c;
                }
            }
        }
    }
}

/// §10-1 item 3: "ポートの垂直レーンがラベルプレートと重なる場合はポートをさらに16px外へ". Two
/// different fixes, by shape:
///
/// * A branch/merge/aligned edge, **or** a collision-fallback forward edge
///   ([`EdgeShape::staircase`] — [`route_with_ports`]'s own doc explains why this one stays local,
///   not on the perimeter lane, since 2026-09-01): checks its two port-adjacent stub segments
///   (`points[0]-points[1]` at the source end, the last pair at the target end — exactly where
///   stage 2's evicted, 16px apart lanes run close together right next to a busy face) against
///   every *other* edge's label plate; a crossing pushes that one port [`PORT_SPACING`] further
///   out and rebuilds just that edge's polyline from it ([`route_with_ports`], which reads
///   `staircase` back out of `raw` exactly the way it always did — pushing a port only moves where
///   the interior chain is bridged *from*, never `raw` itself).
/// * A genuine back edge ([`EdgeShape::reverse`], minus a self-loop): has no single port to
///   push — its whole route can run near a plate anywhere along its length, not only right next to
///   a node (`cjk`'s `D->A` swept straight through `B->D`'s own `テキスト` plate, found by dumping
///   a real routed diagram). So instead: check every segment of its already-built route; if any
///   crosses a foreign plate, rebuild it with [`route_perimeter`] again, this time against a
///   `blocked` test that includes every foreign plate as well as every node — the same
///   `safe_ring_exit` candidate search [`route_flowchart`] already ran, just given one more kind of
///   obstacle to avoid, using the identical ring [`perimeter_lanes`] already assigned it (so this
///   second pass can never disagree with the first about which lane the edge sits on). A self-loop
///   is left untouched, the same as [`route_with_ports`]'s own doc explains for why it never
///   reaches [`route_perimeter`] in the first place.
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
    chain_next: &HashMap<String, String>,
) {
    let cluster_boxes = cluster_node_boxes(clusters);
    let by_id = build_by_id(nodes, &cluster_boxes);
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
    // `route_fan_lane`'s own doc on why a fan-lane edge's bend depth needs a whole-face pass: this
    // retry rebuilds a fan-lane edge's route (below, when a label plate pushes its port) from a
    // freshly recomputed `source_coord`, not `evict`'s own port map, so it has to run its own
    // `evict` pass here too rather than threading one through from `route_flowchart` — the ports
    // it pushes have already drifted from whatever `evict` originally decided.
    let fan_step = evict(direction, &by_id, edges, &shapes, chain_next).fan_step;

    for (edge, shape) in edges.iter().zip(&shapes) {
        let Some(shape) = shape else { continue };
        let (Some(&source), Some(&target)) = (by_id.get(edge.source), by_id.get(edge.target))
        else {
            continue;
        };

        if shape.reverse && source.id != target.id {
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
        if shape.reverse {
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
            let occupied = ports_on_face(
                source.id.as_str(),
                shape.source_side,
                edge.id,
                edges,
                &shapes,
                points,
            );
            new_source_coord = Some(push_outward(source, shape.source_side, cur, &occupied));
        }
        let mut new_target_coord = None;
        if crosses_foreign(&pts[n - 2], &pts[n - 1]) {
            let cur = tangent_coord(shape.target_side, &pts[n - 1]);
            let occupied = ports_on_face(
                target.id.as_str(),
                shape.target_side,
                edge.id,
                edges,
                &shapes,
                points,
            );
            new_target_coord = Some(push_outward(target, shape.target_side, cur, &occupied));
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
            // out the only shape that touches them (`route_with_ports`'s own doc — a genuine,
            // non-self-loop `reverse` back edge). `edge.raw` still matters, though: a `staircase`
            // edge reaches this call too now, and `route_with_ports` bridges its interior chain
            // from `raw` exactly as before — only the port ends move.
            (0.0, 0.0, 0.0, 0.0),
            nodes,
            fan_step.get(edge.id).copied(),
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
///
/// `pub(crate)` (not private) for the same reason [`segment_crosses_node`] is: `render::tests`'
/// fan-lane invariant ("同一ファンの兄弟レーン・スタブ同士は交差しない", §10-3 item 2) states the
/// question against a real diagram's *finished* polylines using this exact predicate, rather than a
/// second, hand-rolled copy of the same axis-parallel intersection arithmetic that could silently
/// drift from what [`insert_crossing_gaps`] itself actually checks.
pub(crate) fn segment_crossing(a: &[Point], b: &[Point]) -> Option<Point> {
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

/// The `CROSSING_GAP`-px interval centred on `cross` — [`segment_crossing`]'s own result, which
/// was found on `points`' segment `seg_idx` (`points[seg_idx]`-`points[seg_idx + 1]`) — turned
/// into the two points [`edges::split_at_gaps`] cuts out.
///
/// Measured by arc length along the *whole* polyline `points`, not clamped to the single segment
/// `cross` sits on: a crossing within `CROSSING_GAP / 2` of a corner needs the omitted stretch to
/// keep going past that corner onto the next segment, or the gap comes out narrower than
/// `CROSSING_GAP` — exactly what CI caught on Linux (`orthogonal_crossing_gaps_cut_the_spanning_
/// edge_and_leave_the_crossed_one_whole`, 2026-09-01): Linux's font metrics size a node a few px
/// differently than macOS's, which is enough to shift `amp-chain`'s `B->C`/`A->D` crossing right up
/// against a bend that macOS's own metrics leave mid-segment, and the old segment-only clamp cut
/// the gap down to whatever room was left on that one segment (6px instead of 12).
///
/// The result is clamped to `[0, length(points)]` — the polyline's own two ends, i.e. its ports —
/// so a crossing too close to either port gets only as much gap as the line has room for rather
/// than spilling past the end `edges::trim_end`/`edges::trim_start` later carve out for an
/// arrowhead; a crossing that close to a port is not produced by a real diagram (`safe_ring_exit`'s
/// own collision check would already have tripped), but the clamp keeps this function total rather
/// than relying on that being true.
fn gap_around(points: &[Point], seg_idx: usize, cross: &Point) -> (Point, Point) {
    let half = CROSSING_GAP / 2.0;
    let seg_start = &points[seg_idx];
    let to_cross = (cross.x - seg_start.x).hypot(cross.y - seg_start.y);
    let s = super::edges::length(&points[..=seg_idx]) + to_cross;
    let total = super::edges::length(points);
    let lo = (s - half).max(0.0);
    let hi = (s + half).min(total);
    let g0 = super::edges::point_at_arc_distance(points, lo).unwrap_or_else(|| cross.clone());
    let g1 = super::edges::point_at_arc_distance(points, hi).unwrap_or_else(|| cross.clone());
    (g0, g1)
}

/// §10-1 item 4 / §10-3 item 5: two crossing-gap rules, told apart by whether either edge is a
/// genuine **perimeter edge** (`reverse`/`staircase`, minus a self-loop — [`route_with_ports`]'s
/// own doc explains why a self-loop is exempt from this whole mechanism):
///
/// * §10-1 item 4 ("外周辺 vs 主辺は従来規則（外周側が跨ぐ）"): when at least one side is a
///   perimeter edge, that side always gets the [`CROSSING_GAP`]-px gap — "下をくぐる線は連続のまま"
///   read as depth, not as a rule about orientation: a back/detour edge visually dips under
///   whatever it crosses, an ordinary edge always reads as passing over it, regardless of which
///   way either one runs. When *both* sides are perimeter edges (reachable — `amp-chain`'s own
///   collision-fallback `A->D` and `B->C` cross each other, neither a back edge nor sharing a node
///   with the other), the spec does not say which one gets the gap; the tie is broken by edge id,
///   lower id kept whole, deterministically rather than arbitrarily.
/// * §10-3 item 5 ("主辺どうしの交差は水平側が譲る"): when *neither* side is a perimeter edge —
///   two ordinary forward edges (`aligned`/`branch`/`merge`/`fan_lane`, any shape
///   [`is_flow_flow_bend`] does or does not cover included) simply happen to cross — the gap goes
///   on whichever of the two segments is **horizontal** at that crossing, never on the vertical
///   one. [`segment_crossing`]'s own contract (exactly one of the pair is vertical, the other
///   horizontal, or it would not have found a crossing at all) means this is never ambiguous the
///   way the perimeter-vs-perimeter tie above can be.
///
/// Node crossings and frame crossings are both out of scope here by construction: this only ever
/// compares two edges' own segments against each other, never against a node's or a cluster's
/// box — §10-1's own "辺と枠の交差は隙間なし（直交して跨ぐだけ）" is exactly what leaving a frame
/// out of this function achieves.
///
/// `points` is the *final* routed polyline per edge id — `route_flowchart`'s own result after
/// `avoid_label_plates` has already run, so a gap is never computed against a route stage 5 was
/// about to move out from under it.
///
/// `clusters` exists only so [`build_by_id`] can resolve a cluster-anchored edge's own `source`/
/// `target` back to a box (needed to `classify` it, to know whether it is a perimeter edge at all)
/// — the same reason [`route_flowchart`]/[`avoid_label_plates`] both take it.
pub fn insert_crossing_gaps(
    direction: Direction,
    nodes: &[PlacedNode],
    clusters: &[PlacedCluster],
    edges: &[EligibleEdge],
    points: &HashMap<String, Vec<Point>>,
) -> HashMap<String, Vec<(Point, Point)>> {
    let cluster_boxes = cluster_node_boxes(clusters);
    let by_id = build_by_id(nodes, &cluster_boxes);
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
            let (pi_perimeter, pj_perimeter) = (
                is_perimeter.get(ei).copied().unwrap_or(false),
                is_perimeter.get(ej).copied().unwrap_or(false),
            );
            if !pi_perimeter && !pj_perimeter {
                // §10-3 item 5: both sides are ordinary main edges — cut whichever segment is
                // horizontal at each crossing point, found independently per crossing (never
                // "pick a side for this whole pair" the way the perimeter rule below does).
                for (seg_idx, wi) in pi.windows(2).enumerate() {
                    for (seg_jdx, wj) in pj.windows(2).enumerate() {
                        let Some(cross) = segment_crossing(wi, wj) else {
                            continue;
                        };
                        if segment_is_vertical(&wi[0], &wi[1]) {
                            let (g0, g1) = gap_around(pj, seg_jdx, &cross);
                            gaps.entry(ej.to_string()).or_default().push((g0, g1));
                        } else {
                            let (g0, g1) = gap_around(pi, seg_idx, &cross);
                            gaps.entry(ei.to_string()).or_default().push((g0, g1));
                        }
                    }
                }
                continue;
            }
            // §10-1 item 4: at least one side is a perimeter edge — that side always spans,
            // regardless of orientation. Both spanning: the higher edge id is the one that gets
            // cut, so the lower one stays whole — this function's own doc explains why either
            // choice is equally arbitrary.
            let cut_on_i = if pi_perimeter && pj_perimeter {
                ei > ej
            } else {
                pi_perimeter
            };
            let (cut_id, cut_pts, other_pts) = if cut_on_i { (ei, pi, pj) } else { (ej, pj, pi) };
            for (seg_idx, wc) in cut_pts.windows(2).enumerate() {
                for wo in other_pts.windows(2) {
                    if let Some(cross) = segment_crossing(wc, wo) {
                        let (g0, g1) = gap_around(cut_pts, seg_idx, &cross);
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
            let routed = route_flowchart(
                direction,
                &nodes,
                &[],
                &edges,
                &std::collections::HashMap::new(),
            );
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
        let routed = route_flowchart(
            Direction::TopToBottom,
            &nodes,
            &[],
            &edges,
            &std::collections::HashMap::new(),
        );
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
            &std::collections::HashMap::new(),
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
        let routed = route_flowchart(
            Direction::TopToBottom,
            &nodes,
            &[],
            &edges,
            &std::collections::HashMap::new(),
        );
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
            &std::collections::HashMap::new(),
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
    fn avoid_label_plates_pushes_a_staircase_forward_edges_port_off_a_plate() {
        // The generic `push_outward` branch (`avoid_label_plates`'s final `else`) is reached by
        // *every* non-reverse shape, but `docs/FEATURE-MERMAID-RENDERER.md` §10's own test audit
        // (finding 7) found the whole corpus never once exercised it for a `staircase` edge (a
        // collision-fallback branch/merge, §10-1 item 1's "両形とも交差するなら…階段経路へ
        // フォールバック") specifically — every corpus source whose plate-crossing fix fires does
        // so for an ordinary branch/merge/aligned edge instead. This reproduces the same
        // `amp-chain` geometry that already proves `A->D` (`docs/FEATURE-MERMAID-RENDERER.md`
        // §10-2's own `orthogonal_dag_corpus` doc) collides on both the branch and the merge
        // attempt and falls back to `staircase`, built directly with `route_flowchart` rather than
        // through the full mermaid pipeline so a plate can be dropped exactly on its source stub —
        // `A & B --> C & D`'s real dagre-driven layout never happens to put a label there.
        //
        // `C`'s own width was widened past the dumped `amp-chain` value (2026-09-01, §10-3 item 3's
        // own correction): a merge target now rides the same flow-axis face a branch target always
        // did (`classify`'s `merge_target_side` doc), so the collision-retry's alternate shape for
        // a *branching* edge like `A->D` (source_out_degree 2) no longer swaps target faces at all
        // — only the source face differs between the two attempts now — and the original, narrower
        // `C` no longer blocked the alternate (flow-face-source) attempt, so `A->D` stopped needing
        // `staircase` at all. Widened until `C` blocks both attempts again — confirmed by the
        // `assert!(shape.staircase, …)` immediately below, still checked from `classify`'s own
        // output rather than assumed.
        let a = node("A", 42.6689453125, 30.7, 69.337890625, 45.4);
        let b = node("B", 42.6689453125, 137.76666666666668, 69.337890625, 45.4);
        let c = node("C", 102.39306640625, 30.7, 190.1103515625, 45.4);
        let d = node(
            "D",
            162.39306640625,
            137.76666666666668,
            70.1103515625,
            45.4,
        );
        let nodes = vec![a, b, c, d];
        let edges = vec![EligibleEdge {
            id: "ad",
            source: "A",
            target: "D",
            raw: &[],
            source_rank: Some(0),
            target_rank: Some(1),
            source_out_degree: 2,
            target_in_degree: 2,
        }];
        let shape = classify(
            Direction::LeftToRight,
            &nodes[0],
            &nodes[3],
            &[],
            Some(0),
            Some(1),
            2,
            2,
            &nodes,
        );
        assert!(
            shape.staircase,
            "the fixture must actually reach `staircase` (both the branch and the merge attempt \
             must cross `C`'s box): {shape:?}"
        );
        // `classify` (called fresh, from `nodes` alone, both by `route_flowchart` above and by
        // `avoid_label_plates` below) marks this edge `staircase` regardless of what `points["ad"]`
        // holds — `route_flowchart`'s own real output already runs the resulting bridge through
        // `clear_local_route`, which detours it hard around `C` (found by dumping: the plain
        // one-corner bridge between the two evicted ports runs straight through `C`'s own box,
        // since `C` sits on the same rank, between `A` and `D`) — far enough that the port lands
        // outside its own node's flat run and `push_outward` has nothing left to give (this file's
        // own doc on that function: "if that walks past the face's own flat run…it gives up").
        // So this fixture is built directly from the *pre-detour* one-corner bridge instead — the
        // shape `route_staircase_with_ports` builds before `clear_local_route` ever runs — which
        // is a real, reachable value of `points["ad"]` (`avoid_label_plates` is handed whatever
        // `points` map the caller has, and nothing about its own logic assumes `clear_local_route`
        // already ran against the *particular* plate being tested against): the port itself still
        // sits at its ordinary, undetoured coordinate, so `push_outward` has room to move it.
        let source_port = port_at(
            &nodes[0],
            shape.source_side,
            face_center_coord(&nodes[0], shape.source_side),
            PORT_INSET,
        );
        let target_port = port_at(
            &nodes[3],
            shape.target_side,
            face_center_coord(&nodes[3], shape.target_side),
            PORT_INSET,
        );
        let before = bridge(
            Direction::LeftToRight,
            &source_port,
            &target_port,
            shape.source_axis,
            shape.target_axis,
        );
        let before: Vec<Point> = std::iter::once(source_port.clone()).chain(before).collect();
        let mut points: HashMap<String, Vec<Point>> = HashMap::new();
        points.insert("ad".to_string(), before.clone());

        // Dropped exactly on `ad`'s own source stub (`before[0]`-`before[1]`) — a completely
        // unrelated edge's label, the same "found by dumping, not assumed" shape every other case
        // in this file uses.
        let stub_mid = Point::new(
            (before[0].x + before[1].x) / 2.0,
            (before[0].y + before[1].y) / 2.0,
        );
        let mut plates: HashMap<String, PlacedEdgeLabel> = HashMap::new();
        plates.insert(
            "someone-elses-label".to_string(),
            PlacedEdgeLabel {
                center: stub_mid,
                size: Size::new(40.0, 20.0),
                label: Label::measure("x"),
            },
        );
        let crosses_plate = |pts: &[Point], plate: &PlacedEdgeLabel| {
            pts.windows(2)
                .any(|w| segment_crosses_plate(&w[0], &w[1], plate))
        };
        assert!(
            crosses_plate(&before, &plates["someone-elses-label"]),
            "fixture must reproduce the crossing before the fix runs: {before:?}"
        );

        avoid_label_plates(
            Direction::LeftToRight,
            &nodes,
            &[],
            &edges,
            &mut points,
            &mut plates,
            &std::collections::HashMap::new(),
        );

        assert_ne!(
            points["ad"], before,
            "the staircase edge's port must actually move: {:?}",
            points["ad"]
        );
        assert!(
            !crosses_plate(&points["ad"], &plates["someone-elses-label"]),
            "the rerouted staircase line must clear the plate: {:?}",
            points["ad"]
        );
        for w in points["ad"].windows(2) {
            let dx = (w[1].x - w[0].x).abs();
            let dy = (w[1].y - w[0].y).abs();
            assert!(
                dx < 1e-9 || dy < 1e-9,
                "rerouted line must stay axis-parallel: {w:?}"
            );
        }
    }

    // --- test-sufficiency audit finding 9: tie-break pins (medium) --------------------------------
    //
    // Four deterministic tie-breaks the mutation-testing pass found no test pinned directly: each
    // one below states the tie case by construction (an exact 45°, an exactly-centred port, two
    // exactly-equal-length segments) rather than hoping a corpus source happens to land on one.

    #[test]
    fn dominant_face_at_exactly_45_degrees_prefers_the_flow_face() {
        // `dominant_face`'s own tie-break is `dflow.abs() >= dcross.abs()`, so an exact tie (a
        // reference point sitting on the true diagonal from `center`) must resolve to the flow
        // face, never the cross one. TD: flow is y, cross is x.
        let center = Point::new(0.0, 0.0);
        let reference = Point::new(50.0, 50.0); // dflow == dcross == 50.0, an exact tie.
        assert_eq!(
            dominant_face(Direction::TopToBottom, &center, &reference),
            Side::Bottom,
            "an exact 45° tie must resolve to the flow face (Bottom, since dflow >= 0), not the \
             cross face (Right)"
        );
        // And the mirror case for a direction whose flow axis is x (LR): the same tie must resolve
        // to Right (flow), not Top/Bottom (cross).
        assert_eq!(
            dominant_face(Direction::LeftToRight, &center, &reference),
            Side::Right,
            "the same 45° tie under LR must resolve to the flow face (Right), not the cross face \
             (Bottom)"
        );
    }

    #[test]
    fn safe_ring_exit_l_shape_tie_prefers_the_first_side_named_in_the_near_far_pair() {
        // A port sitting exactly on the ring's own horizontal centre, `(port.x - l) == (r -
        // port.x)`: `safe_ring_exit`'s own `near`/`far` choice (`if (port.x - l) <= (r - port.x)`)
        // is a `<=`, so an exact tie must resolve to `l` (the left side), never `r`. Forced onto
        // the L-shaped fallback by blocking the direct candidate outright, with *both* possible L
        // shapes left clear so the only thing deciding which one is returned is try-order — and the
        // try-order is exactly what the near/far tie-break picks.
        let ring = (0.0, 0.0, 600.0, 400.0);
        let port = Point::new(300.0, 50.0); // horizontally centred: 300-0 == 600-300.
        let direct = ring_touch(Side::Top, &port, ring);
        let blocked = |a: &Point, b: &Point| *a == port && *b == direct;
        let exit = safe_ring_exit(Side::Top, &port, ring, &blocked);
        assert_eq!(exit.len(), 2, "must take the L-shaped fallback: {exit:?}");
        assert!(
            (exit[1].x - ring.0).abs() < 1e-9,
            "an exact near/far tie must resolve to the ring's LEFT side (tried first), not the \
             right: {exit:?}"
        );
    }

    #[test]
    fn label_slot_keeps_the_first_segment_on_an_exact_length_tie() {
        // Two flow-axis (TD: vertical) segments of exactly the same length — `label_slot`'s own
        // `better` predicate is a strict `len > best_len`, so the *first* one encountered must win,
        // not the second.
        let pts = vec![
            Point::new(0.0, 0.0),
            Point::new(0.0, 50.0),   // first vertical leg: 50px
            Point::new(20.0, 50.0),  // a short cross-axis hop, irrelevant to the tie
            Point::new(20.0, 100.0), // second vertical leg: also exactly 50px
        ];
        let slot = label_slot(Direction::TopToBottom, &pts).expect("must return a slot");
        assert!(slot.is_flow_axis);
        assert!((slot.length - 50.0).abs() < 1e-9, "{}", slot.length);
        assert!(
            (slot.center.x - 0.0).abs() < 1e-9 && (slot.center.y - 25.0).abs() < 1e-9,
            "an exact-length tie must keep the FIRST segment (centre (0,25)), not the second \
             (centre (20,75)): {:?}",
            slot.center
        );
    }

    #[test]
    fn perimeter_lanes_are_assigned_by_edge_id_not_declaration_order() {
        // Two genuine back edges (source and target are different node ids, both `reverse`),
        // declared with `b_edge` first and `a_edge` second — `perimeter_lanes`'s own doc says the
        // order is "edge id, not declaration or `HashMap` iteration order", so `a_edge` (the
        // alphabetically smaller id, declared SECOND) must still land on lane 0.
        let a = node("A", 0.0, 0.0, 40.0, 30.0);
        let b = node("B", 0.0, 200.0, 40.0, 30.0);
        let by_id: HashMap<&str, &PlacedNode> = [("A", &a), ("B", &b)].into_iter().collect();
        let edges = vec![
            EligibleEdge {
                id: "b_edge",
                source: "B",
                target: "A",
                raw: &[],
                source_rank: Some(2),
                target_rank: Some(0),
                source_out_degree: 1,
                target_in_degree: 1,
            },
            EligibleEdge {
                id: "a_edge",
                source: "B",
                target: "A",
                raw: &[],
                source_rank: Some(2),
                target_rank: Some(0),
                source_out_degree: 1,
                target_in_degree: 1,
            },
        ];
        let back_shape = EdgeShape {
            reverse: true,
            aligned: false,
            staircase: false,
            fan_lane: false,
            rank_lane_bend: None,
            source_side: Side::Top,
            source_axis: Axis::Cross,
            target_side: Side::Bottom,
            target_axis: Axis::Cross,
        };
        let shapes = vec![Some(back_shape), Some(back_shape)];
        let lanes = perimeter_lanes(&by_id, &edges, &shapes);
        assert_eq!(
            lanes.get("a_edge").copied(),
            Some(0),
            "the alphabetically-smaller edge id must get lane 0 regardless of declaration order: \
             {lanes:?}"
        );
        assert_eq!(lanes.get("b_edge").copied(), Some(1), "{lanes:?}");
    }

    #[test]
    fn push_outward_skips_a_coordinate_another_port_already_occupies() {
        // Three ports evicted onto the same face at `-PORT_SPACING`/`0`/`+PORT_SPACING` — `evict`'s
        // own rule for `n == 3` (the `for (i, claim) in claims.iter().enumerate()` loop above,
        // §10-1 item 3) — is exactly the shape that makes a single `PORT_SPACING` step collide: the
        // review's own report. Pushing the centre port "further out" by one step would put it right
        // on top of the sibling already sitting at `+PORT_SPACING`.
        let a = node("A", 0.0, 0.0, 200.0, 40.0);
        let occupied = [-PORT_SPACING, PORT_SPACING];
        let pushed = push_outward(&a, Side::Bottom, 0.0, &occupied);
        assert!(
            occupied.iter().all(|&o| (o - pushed).abs() > EPS),
            "pushed port {pushed} must not land on a sibling port {occupied:?}"
        );
        // Still moved outward, past the sibling it had to step over rather than landing short.
        assert!(pushed > PORT_SPACING, "{pushed}");
    }

    #[test]
    fn push_outward_never_collides_across_a_dense_face() {
        // The same shape as above, generalised: every offset `evict` would ever hand out for
        // `n` up to 9 ports on one face, with every slot but the one under test already occupied —
        // the tightest case `push_outward` can be asked to solve on a face this wide.
        let a = node("A", 0.0, 0.0, 400.0, 40.0);
        for n in 1..=9usize {
            let offsets: Vec<f64> = (0..n)
                .map(|i| (i as f64 - (n as f64 - 1.0) / 2.0) * PORT_SPACING)
                .collect();
            for &cur in &offsets {
                let occupied: Vec<f64> = offsets.iter().copied().filter(|&o| o != cur).collect();
                let pushed = push_outward(&a, Side::Bottom, cur, &occupied);
                assert!(
                    occupied.iter().all(|&o| (o - pushed).abs() > EPS),
                    "n={n} cur={cur}: pushed {pushed} collided with {occupied:?}"
                );
            }
        }
    }

    #[test]
    fn push_outward_gives_up_rather_than_leave_the_nodes_flat_run() {
        // A node too narrow to hold even one `PORT_SPACING` step: pushing further would land the
        // port in the corner, past `face_flat_half_extent`, not on the flat run —
        // `push_outward`'s own doc says give up and return `cur` unchanged rather than worsen the
        // crossing this pass was trying to fix.
        let a = node("A", 0.0, 0.0, 20.0, 40.0); // half_extent = 10px < one PORT_SPACING step
        let pushed = push_outward(&a, Side::Bottom, 0.0, &[]);
        assert_eq!(pushed, 0.0, "must give up and keep the original coordinate");
    }

    #[test]
    fn ports_on_face_reads_only_the_named_face_excluding_the_edge_itself() {
        // Three edges sharing `A`: two land on its Bottom face (source ends), one on its Right
        // face — `ports_on_face` must return only the two Bottom ones, as tangent (x) coordinates,
        // and never the edge passed as `exclude_edge_id` even though it also touches Bottom.
        let a = node("A", 100.0, 100.0, 200.0, 40.0);
        let b = node("B", 100.0, 200.0, 60.0, 40.0);
        let c = node("C", 100.0, 200.0, 60.0, 40.0);
        let d = node("D", 250.0, 100.0, 60.0, 40.0);
        let nodes = [a, b, c, d];
        let make_edge = |id, target| EligibleEdge {
            id,
            source: "A",
            target,
            raw: &[] as &[Point],
            source_rank: Some(0),
            target_rank: Some(1),
            source_out_degree: 3,
            target_in_degree: 1,
        };
        let edges = vec![
            make_edge("ab", "B"),
            make_edge("ac", "C"),
            make_edge("ad", "D"),
        ];
        let shapes: Vec<Option<EdgeShape>> = vec![
            Some(EdgeShape {
                reverse: false,
                aligned: false,
                staircase: false,
                fan_lane: false,
                rank_lane_bend: None,
                source_side: Side::Bottom,
                source_axis: Axis::Cross,
                target_side: Side::Top,
                target_axis: Axis::Cross,
            }),
            Some(EdgeShape {
                reverse: false,
                aligned: false,
                staircase: false,
                fan_lane: false,
                rank_lane_bend: None,
                source_side: Side::Bottom,
                source_axis: Axis::Cross,
                target_side: Side::Top,
                target_axis: Axis::Cross,
            }),
            Some(EdgeShape {
                reverse: false,
                aligned: false,
                staircase: false,
                fan_lane: false,
                rank_lane_bend: None,
                source_side: Side::Right,
                source_axis: Axis::Flow,
                target_side: Side::Left,
                target_axis: Axis::Flow,
            }),
        ];
        let mut points: HashMap<String, Vec<Point>> = HashMap::new();
        points.insert(
            "ab".to_string(),
            vec![Point::new(84.0, 120.0), Point::new(84.0, 180.0)],
        );
        points.insert(
            "ac".to_string(),
            vec![Point::new(116.0, 120.0), Point::new(116.0, 180.0)],
        );
        points.insert(
            "ad".to_string(),
            vec![Point::new(200.0, 100.0), Point::new(220.0, 100.0)],
        );

        let occupied = ports_on_face(&nodes[0].id, Side::Bottom, "ac", &edges, &shapes, &points);
        assert_eq!(
            occupied,
            vec![84.0],
            "must see `ab`'s Bottom port, not `ac` (excluded) or `ad` (a different face)"
        );
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
    fn merge_lr_edge_now_bends_twice_entering_the_flow_axis_face() {
        // §10-3 item 3's own correction (`classify`'s `merge_target_side` doc): the round-3
        // reference (`docs/mermaid-theme/handoff/round3-Konoma-Flowchart-Routing.dc.html`'s `3a` —
        // every multi-way merge into `ラスタライズ`/`セルに合わせる` enters the flow-axis Left
        // face, confirmed independently by `2b`'s and `2c`'s own "API ゲート" merges) corrected
        // this shape: a merge target now rides the same flow-axis face a branch target always did,
        // not the cross-axis face this test originally pinned (round-2's own prose, "目標の直交辺
        // の中央へ", turned out to be an imprecise gloss `2d` never actually drew this way in any
        // of its own illustrated examples). Both ends now sit on unlike-rank flow-axis faces with
        // different cross coordinates, so `bridge`'s own `(Axis::Flow, Axis::Flow)` case applies:
        // two bends, not one.
        let a = node("A", 0.0, 0.0, 80.0, 40.0);
        let b = node("B", 200.0, 100.0, 80.0, 40.0);
        // out-degree 1, in-degree 2: not a branch, target merges.
        let pts = route_edge(Direction::LeftToRight, &a, &b, &[], Some(0), Some(1), 1, 2);
        assert_eq!(pts.len(), 4, "{pts:?}");
        // Leaves A horizontally (flow axis): y constant between pts[0] and pts[1].
        assert!((pts[0].y - pts[1].y).abs() < 1e-9, "{pts:?}");
        // The bend itself is a vertical run, at the midpoint between the two flow coordinates.
        assert!((pts[1].x - pts[2].x).abs() < 1e-9, "{pts:?}");
        assert!((pts[1].x - 100.0).abs() < 1e-9, "{pts:?}");
        // Enters B horizontally too (flow axis, not the old cross-axis vertical entry): y constant
        // between pts[2] and pts[3].
        assert!((pts[2].y - pts[3].y).abs() < 1e-9, "{pts:?}");
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
        route_flowchart(
            direction,
            nodes,
            &[],
            edges,
            &std::collections::HashMap::new(),
        )
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
    fn a_chain_selected_trunk_keeps_the_centre_port_even_when_geometry_never_aligned_it() {
        // §10-3 item 1 ("ファン面の中央ポート＝幹の直進辺", `docs/render-check`'s own `mermaid.ja-03`
        // fence-3 investigation): a fan face's centre port belongs to the source's own trunk/chain
        // edge (`align_straight_lanes`'s own `next` selection, threaded through as `chain_next`)
        // even when the two nodes never landed on the same cross coordinate — exactly `3a`'s own
        // `設定のルール -> ブロックモデル`, whose target sits among nine other same-rank siblings and
        // never gets pulled onto the source's own row. None of S's four targets (T/U/V/W) is
        // `aligned` (`dcross` is never `< 0.5` for any of them) — only `chain_next` can tell the
        // router which one is the trunk.
        //
        // Four targets, not three: `classify`'s own `fan_eligible` doc (§10-3 item 1, reimplemented
        // 2026-09-02 on `FAN_ELIGIBLE_MIN_BRANCHES` rather than "has an aligned sibling") — a
        // branching source needs *more* branches than 1b's own basic shape seats one-per-face before
        // this fan-lane mechanism (the one this test is about) applies at all; a plain 3-way branch
        // (`docs/mermaid-theme/handoff/zz-design-sources.md`'s own `2a`) uses 1b's ordinary
        // cross-axis faces instead, where there is no shared face — and so no centre-port contest —
        // for `chain_next` to arbitrate.
        //
        // Rule 1's own "もう一方の端点のcross座標順" sort would otherwise put V (other_cross 180,
        // the middle of {120, 180, 240, 600}) in the centre slot on its own — T is deliberately the
        // *extreme* one (600) so this test can tell "rule 1's plain sort happened to centre the
        // trunk" apart from "the trunk-centring fix actually moved it there".
        let s = node("S", 100.0, 200.0, 60.0, 40.0);
        let t = node("T", 400.0, 600.0, 60.0, 40.0); // the designated trunk — sorts last on its own.
        let u = node("U", 400.0, 120.0, 60.0, 40.0);
        let v = node("V", 400.0, 180.0, 60.0, 40.0);
        let w = node("W", 400.0, 240.0, 60.0, 40.0);
        let nodes = vec![s.clone(), t.clone(), u.clone(), v.clone(), w.clone()];
        let edge = |id: &'static str, target: &'static str| EligibleEdge {
            id,
            source: "S",
            target,
            raw: &[],
            source_rank: Some(0),
            target_rank: Some(1),
            source_out_degree: 4,
            target_in_degree: 1,
        };
        let edges = vec![
            edge("st", "T"),
            edge("su", "U"),
            edge("sv", "V"),
            edge("sw", "W"),
        ];
        let mut chain_next = HashMap::new();
        chain_next.insert("S".to_string(), "T".to_string());
        let routed = route_flowchart(Direction::LeftToRight, &nodes, &[], &edges, &chain_next);

        let st_port = routed.points["st"].first().unwrap();
        assert!(
            (st_port.y - s.center.y).abs() < 1e-9,
            "the chain-selected trunk (S->T) must keep S's own face centre even though T is not \
             geometrically aligned: {st_port:?}"
        );
        for (name, id) in [("su", "su"), ("sv", "sv"), ("sw", "sw")] {
            let port = routed.points[id].first().unwrap();
            assert!(
                (port.y - s.center.y).abs() > 1e-9,
                "{name}: a non-trunk sibling must not also claim the centre port: {port:?}"
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
        let _ = align_straight_lanes(Direction::TopToBottom, &mut nodes, &node_rank, &candidates);

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
    fn tie_break_prefers_a_target_that_continues_the_chain_over_a_leaf() {
        // §10-3 item 7 ("直進レーンは図を貫く幹"): C has three same-rank targets — LEAF1, LEAF2
        // (dead ends) and TRUNK, which itself continues on to NEXT. LEAF1 sorts first by the
        // pre-existing tie-break (smallest cross coordinate, y=0), but TRUNK must win instead,
        // because only TRUNK keeps the chain going past this window — exactly `3a`'s own
        // `設定のルール` choosing `ブロックモデル` (which continues to `mermaid`/`数式`) over its
        // nine other, dead-end same-rank targets.
        let mut nodes = vec![
            node("C", 0.0, 50.0, 40.0, 30.0),
            node("LEAF1", 100.0, 0.0, 40.0, 30.0),
            node("LEAF2", 100.0, 100.0, 40.0, 30.0),
            node("TRUNK", 100.0, 50.0, 40.0, 30.0),
            node("NEXT", 200.0, 999.0, 40.0, 30.0),
        ];
        let node_rank = ranks(&[
            ("C", 0),
            ("LEAF1", 1),
            ("LEAF2", 1),
            ("TRUNK", 1),
            ("NEXT", 2),
        ]);
        let candidates = [
            edge("C", "LEAF1"),
            edge("C", "LEAF2"),
            edge("C", "TRUNK"),
            edge("TRUNK", "NEXT"),
        ];
        let (_, chain_sources) =
            align_straight_lanes(Direction::LeftToRight, &mut nodes, &node_rank, &candidates);
        let by_id: HashMap<&str, &PlacedNode> = nodes.iter().map(|n| (n.id.as_str(), n)).collect();
        assert_eq!(
            by_id["C"].center.y,
            by_id["TRUNK"].center.y,
            "C must align with TRUNK (the target that itself continues), not LEAF1: {:?}",
            nodes
                .iter()
                .map(|n| (n.id.as_str(), n.center.y))
                .collect::<Vec<_>>()
        );
        assert!(
            chain_sources.contains_key("C") && chain_sources.contains_key("TRUNK"),
            "both chain links' own sources must be reported back: {chain_sources:?}"
        );
        assert_eq!(
            chain_sources.get("C").map(String::as_str),
            Some("TRUNK"),
            "the returned map must name TRUNK as C's own selected target, not merely list C: \
             {chain_sources:?}"
        );
    }

    #[test]
    fn tie_break_prefers_extending_an_already_selected_chain_over_a_fresh_pick() {
        // §10-3 item 7, the other half: once a chain is already flowing through a node (an earlier
        // *window* selected an edge landing on it), the next window must keep following that same
        // node rather than let a fresh, unrelated pair's smaller cross coordinate cut the chain off
        // — found on `3a`'s own reference geometry: `MM -> RS` is selected first (`MM`'s cross
        // coordinate sorts before `IM`'s among *that* window's candidates), so by the time the next
        // window offers `RS -> FIT` and `IM -> FIT` for the same target `FIT`, `RS` must win even
        // though `IM`'s own cross coordinate (0.0) sorts before `RS`'s (100.0) — `RS` is already
        // mid-chain (the previous window's own selection), `IM` is not.
        let mut nodes = vec![
            node("MM", 0.0, 100.0, 40.0, 30.0),
            node("RS", 100.0, 100.0, 40.0, 30.0),
            node("IM", 100.0, 0.0, 40.0, 30.0),
            node("FIT", 200.0, 50.0, 40.0, 30.0),
        ];
        let node_rank = ranks(&[("MM", 0), ("IM", 0), ("RS", 1), ("FIT", 2)]);
        let candidates = [edge("MM", "RS"), edge("RS", "FIT"), edge("IM", "FIT")];
        let (_, chain_sources) =
            align_straight_lanes(Direction::LeftToRight, &mut nodes, &node_rank, &candidates);
        assert!(
            chain_sources.contains_key("RS"),
            "RS must have been selected to extend the MM -> RS -> FIT chain: {chain_sources:?}"
        );
        assert!(
            !chain_sources.contains_key("IM"),
            "IM (a fresh, unrelated pick) must lose the tie to RS (already mid-chain): \
             {chain_sources:?}"
        );
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
        let _ = align_straight_lanes(Direction::TopToBottom, &mut nodes, &node_rank, &candidates);

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

    /// Test-sufficiency audit finding 9 (medium): `align_straight_lanes`'s own candidate sort has
    /// two keys ("タイは上・左優先" — source cross coordinate, then target cross coordinate,
    /// `pair_candidates.sort_by`'s own `.then_with`), but every existing test (this file's own
    /// `tie_break_prefers_the_smaller_cross_coordinate` included) only ever varies the *first* key
    /// — two different sources competing for one target. Here one single source `S` offers a lane
    /// to two different targets, so the first key ties by construction (both candidates share the
    /// same source, hence the same source cross coordinate) and only the second key can decide —
    /// §10-1 item 1's own "上・左優先" must still mean "the smaller of the two", read off the
    /// *target* this time: `S` must pair with `T1` (the smaller-cross target), not `T2`.
    #[test]
    fn tie_break_second_key_prefers_the_smaller_target_cross_coordinate_when_sources_tie() {
        // Ids deliberately spelled so alphabetical order is the *opposite* of cross-coordinate
        // order (`Alef` < `Zed`, but `Alef`'s own cross coordinate, 500, is the *larger* one) — a
        // sort that silently fell back past the (missing) numeric second key straight to the
        // third (`s1.cmp(s2)`, a no-op here since both share source `S`) or fourth (`t1.cmp(t2)`,
        // alphabetical by id) key would then pick the wrong target for a reason that has nothing
        // to do with cross coordinates, and this is built to make that visible rather than
        // coincide with the right answer the way two arbitrarily-named ids might.
        let mut nodes = vec![
            node("S", 100.0, 0.0, 40.0, 30.0),
            node("Zed", 20.0, 100.0, 40.0, 30.0),
            node("Alef", 500.0, 100.0, 40.0, 30.0),
        ];
        let node_rank = ranks(&[("S", 0), ("Zed", 1), ("Alef", 1)]);
        // Declared in an order that would win the *wrong* candidate if the sort fell back to
        // declaration order instead of the second key: `S -> Alef` listed first.
        let candidates = [edge("S", "Alef"), edge("S", "Zed")];
        let _ = align_straight_lanes(Direction::TopToBottom, &mut nodes, &node_rank, &candidates);

        let by_id: HashMap<&str, &PlacedNode> = nodes.iter().map(|n| (n.id.as_str(), n)).collect();
        // S can pair with at most one target ("各ノード高々1出"). If it paired with Zed (smaller
        // target cross, 20), the chain average is (100+20)/2 = 60 and Alef is left untouched.
        assert!(
            (by_id["S"].center.x - 60.0).abs() < 1e-9,
            "S must align with Zed (the smaller-cross target), landing at (100+20)/2=60, not \
             (100+500)/2=300: {:?}",
            by_id["S"].center
        );
        assert_eq!(
            by_id["Alef"].center.x, 500.0,
            "Alef lost the second-key tie-break and must be left exactly where it started"
        );
    }

    #[test]
    fn a_three_way_leaf_fan_picks_the_cross_order_middle_target_not_the_smallest() {
        // §10-3 item 2 ("幹末端のタイブレーク"): `3a`'s own `端末` (`docs/mermaid-theme/handoff/
        // round3-Konoma-Flowchart-Routing.dc.html`) has three same-rank leaf targets — none
        // continues the chain further, so the pre-existing "target cross ascending" key would pick
        // TOP (the smallest cross coordinate) outright. `3a` instead keeps the spine through MID,
        // the cross-order middle of the three — this is the shape that fixture stands in for.
        let mut nodes = vec![
            node("S", 0.0, 0.0, 40.0, 30.0),
            node("TOP", 100.0, 0.0, 40.0, 30.0),
            node("MID", 100.0, 50.0, 40.0, 30.0),
            node("BOTTOM", 100.0, 150.0, 40.0, 30.0),
        ];
        let node_rank = ranks(&[("S", 0), ("TOP", 1), ("MID", 1), ("BOTTOM", 1)]);
        // Declared with TOP first, so a sort that fell back past the median key straight to plain
        // ascending target-cross order would still (wrongly) land on TOP — this is not a
        // declaration-order artefact.
        let candidates = [edge("S", "TOP"), edge("S", "MID"), edge("S", "BOTTOM")];
        let (_, chain_next) =
            align_straight_lanes(Direction::LeftToRight, &mut nodes, &node_rank, &candidates);
        assert_eq!(
            chain_next.get("S").map(String::as_str),
            Some("MID"),
            "S must pick MID (the cross-order middle target), not TOP (the smallest): \
             {chain_next:?}"
        );
        // The chain *selection* is the thing this test pins — whether the geometry fully collapses
        // onto it afterwards is `align_straight_lanes`'s separate overlap-resolution sweep (its own
        // "ランク内の並び順は変えない" constraint can leave a selected chain member short of its own
        // desired coordinate when a neighbour is in the way, exactly as `docs/STATUS.md`'s own
        // ★未修正 entry on the cascading-push gap describes) and not this rule's own concern.
    }

    #[test]
    fn overlap_resolution_pushes_the_later_node_without_reordering() {
        // P (chain average 100) and Q (chain average 155) are only 55px apart after their own
        // independent alignment — short of the `half(20) + ORTHO_NODE_SEP(24) + half(20) = 64px`
        // minimum (this pass only ever runs under `Routing::Orthogonal`, so `ORTHO_NODE_SEP` —
        // §10-3 item 9's own density fix — is the gap it actually reuses, not the shared
        // `NODE_SEP` splines still lays out with). §10-1 item 1's "重なった側を押し出して従来の
        // 最小間隔を維持する（ランク内の並び順は変えない）": Q, which was already the later of the
        // two in original rank order (P at x=200, Q at x=210), is the one pushed — to exactly
        // 164, not moved to swap places with P.
        let mut nodes = vec![
            node("A", 0.0, 0.0, 40.0, 30.0),
            node("C", 100.0, 0.0, 40.0, 30.0),
            node("P", 200.0, 100.0, 40.0, 30.0),
            node("Q", 210.0, 100.0, 40.0, 30.0),
        ];
        let node_rank = ranks(&[("A", 0), ("C", 0), ("P", 1), ("Q", 1)]);
        let candidates = [edge("A", "P"), edge("C", "Q")];
        let _ = align_straight_lanes(Direction::TopToBottom, &mut nodes, &node_rank, &candidates);

        let by_id: HashMap<&str, &PlacedNode> = nodes.iter().map(|n| (n.id.as_str(), n)).collect();
        assert!(
            (by_id["P"].center.x - 100.0).abs() < 1e-9,
            "P is first in rank order and never needs pushing: {:?}",
            by_id["P"].center
        );
        assert!(
            (by_id["Q"].center.x - 164.0).abs() < 1e-9,
            "Q must be pushed to exactly P's far edge (120) + ORTHO_NODE_SEP(24) + Q's \
             half-width(20) = 164, not left at its own aligned 155: {:?}",
            by_id["Q"].center
        );
    }

    // --- stage 5: crossing gap, measured by arc length (Linux CI regression, 2026-09-01) ---------
    //
    // `orthogonal_crossing_gaps_cut_the_horizontal_side_of_each_crossing` (this module's own
    // consumer, `render::tests`, under its pre-round-3-item-5 name at the time) failed on Linux
    // only: its font metrics size a node a
    // few px differently than macOS's, which was enough to move `amp-chain`'s real `B->C`/`A->D`
    // crossing from mid-segment (macOS) to within `CROSSING_GAP / 2` of a corner (Linux) — and the
    // old `gap_around`, which clamped to the single segment the crossing point was found on, cut a
    // gap only 6px wide there instead of 12. These three tests pin `gap_around`'s arc-length fix
    // directly, with hand-built node centres and polylines rather than anything `route_with_ports`
    // measured a label through — the same font-independent reproduction CI's own failure needed.

    /// Builds the two [`EligibleEdge`]s [`insert_crossing_gaps`] needs to find a crossing between
    /// `cut_points` (forced to be the "spanning"/detour side by its rank pair, so it is always the
    /// one a gap gets cut into) and `other_points` (an ordinary edge, never cut) — both hand
    /// coordinates, standing in for whatever `route_with_ports` would have measured. Returns
    /// exactly the gaps `insert_crossing_gaps` computed for the "cut" edge.
    fn crossing_gaps(cut_points: Vec<Point>, other_points: Vec<Point>) -> Vec<(Point, Point)> {
        let nodes = vec![
            node("P", 0.0, -500.0, 4.0, 4.0),
            node("Q", 0.0, 500.0, 4.0, 4.0),
            // `M`/`N` used to share the same flow coordinate (`y = -1000.0` for both, under this
            // helper's `Direction::TopToBottom`), a coincidence nothing about "two far-away nodes"
            // required. `shape_crosses_a_node`'s own-endpoint check (added when `route_with_ports`'s
            // staircase fix was generalised, this file's own doc) is strict about a shape's own
            // corner never landing inside its own box, and `M`'s `Bottom`-face port `x` always
            // equals `M`'s own centre `x`, while a `Left`-face target's held tangent equals
            // `N`'s own centre `y` — so with `N.y == M.y`, the merge shape's one bend landed
            // exactly on `M`'s own centre, turning "other" into a self-colliding `staircase` edge
            // and defeating this function's own doc ("`other_points`: an ordinary edge, never cut").
            // Different `y` values break the coincidence without changing anything this helper's
            // actual callers read (`M`/`N`'s only job is to make `classify` see a real, non-
            // colliding forward edge for `is_detour`'s sake).
            node("M", -1000.0, -1000.0, 4.0, 4.0),
            node("N", 1000.0, -700.0, 4.0, 4.0),
        ];
        let edges = [
            EligibleEdge {
                id: "cut",
                source: "P",
                target: "Q",
                raw: &[],
                // `tr <= sr` trips `classify`'s `is_reverse` branch, which is what makes this edge
                // the spanning (detour) side of any crossing it takes part in.
                source_rank: Some(1),
                target_rank: Some(0),
                source_out_degree: 1,
                target_in_degree: 1,
            },
            EligibleEdge {
                id: "other",
                source: "M",
                target: "N",
                raw: &[],
                // `tr > sr` keeps this edge ordinary — it is never the spanning side and never cut.
                source_rank: Some(0),
                target_rank: Some(1),
                source_out_degree: 1,
                target_in_degree: 1,
            },
        ];
        let mut points = HashMap::new();
        points.insert("cut".to_string(), cut_points);
        points.insert("other".to_string(), other_points);
        let gaps = insert_crossing_gaps(Direction::TopToBottom, &nodes, &[], &edges, &points);
        gaps.get("cut").cloned().unwrap_or_default()
    }

    #[test]
    fn crossing_gap_mid_segment_is_the_full_12px() {
        // The cut edge's own polyline: a vertical leg (0,0)-(0,20), then a horizontal one
        // (0,20)-(30,20) — an ordinary orthogonal L, total length 50. The other edge crosses the
        // vertical leg at (0, 10), 10px from either end of that leg: comfortably mid-segment, no
        // corner within reach of `CROSSING_GAP / 2` (6px) either way.
        let cut = vec![
            Point::new(0.0, 0.0),
            Point::new(0.0, 20.0),
            Point::new(30.0, 20.0),
        ];
        let other = vec![Point::new(-10.0, 10.0), Point::new(10.0, 10.0)];
        let gaps = crossing_gaps(cut, other);
        assert_eq!(gaps.len(), 1, "{gaps:?}");
        let (g0, g1) = &gaps[0];
        assert!(
            (g0.x - 0.0).abs() < 1e-9 && (g0.y - 4.0).abs() < 1e-9,
            "{g0:?}"
        );
        assert!(
            (g1.x - 0.0).abs() < 1e-9 && (g1.y - 16.0).abs() < 1e-9,
            "{g1:?}"
        );
        let width = (g1.x - g0.x).hypot(g1.y - g0.y);
        assert!((width - CROSSING_GAP).abs() < 1e-9, "{width}");
    }

    #[test]
    fn crossing_gap_straddling_a_corner_still_totals_12px_by_arc_length() {
        // Same cut polyline as above, but the crossing now lands at (0, 17) — only 3px from the
        // corner at (0, 20), well inside the 6px half-gap. The straight-line (Euclidean) distance
        // between the two gap endpoints this produces is *less* than 12px (the corner bends the
        // path), which is exactly why the old segment-clamped `gap_around` under-cut it: the fix
        // must keep going past the corner onto the next segment so the *arc length* removed is
        // still the full 12px.
        let cut = vec![
            Point::new(0.0, 0.0),
            Point::new(0.0, 20.0),
            Point::new(30.0, 20.0),
        ];
        let other = vec![Point::new(-10.0, 17.0), Point::new(10.0, 17.0)];
        let gaps = crossing_gaps(cut, other);
        assert_eq!(gaps.len(), 1, "{gaps:?}");
        let (g0, g1) = &gaps[0];
        // Arc length 17 - 6 = 11, still on the vertical leg: (0, 11).
        assert!(
            (g0.x - 0.0).abs() < 1e-9 && (g0.y - 11.0).abs() < 1e-9,
            "g0 must stay on the vertical leg at arc length 11: {g0:?}"
        );
        // Arc length 17 + 6 = 23 overruns the vertical leg's own 20px by 3px, continuing 3px onto
        // the horizontal leg from the corner: (3, 20).
        assert!(
            (g1.x - 3.0).abs() < 1e-9 && (g1.y - 20.0).abs() < 1e-9,
            "g1 must continue 3px past the corner onto the horizontal leg: {g1:?}"
        );
        // The Euclidean chord is shorter than 12px — the corner bends it — but the *arc length*
        // removed (g0 -> corner -> g1) is the full 12px `CROSSING_GAP`.
        let chord = (g1.x - g0.x).hypot(g1.y - g0.y);
        assert!(
            chord < CROSSING_GAP - 1.0,
            "the chord must actually be shorter than CROSSING_GAP for this test to mean anything: \
             {chord}"
        );
        let corner = Point::new(0.0, 20.0);
        let arc = crate::preview::mermaid::render::edges::length(&[g0.clone(), corner, g1.clone()]);
        assert!((arc - CROSSING_GAP).abs() < 1e-9, "{arc}");
    }

    #[test]
    fn crossing_gap_near_a_port_is_clamped_to_the_room_available() {
        // A cut edge only 8px long in total — shorter than `CROSSING_GAP` itself — crossed at
        // (0, 3). Neither side of the gap has 6px of room: the low side runs out at 3px (the
        // polyline's own start, i.e. its port) and the high side at 5px (the polyline's own end).
        // The gap must be clamped to exactly what the line has, `(0,0)`-`(0,8)`, 8px wide — not
        // spill past either port.
        let cut = vec![Point::new(0.0, 0.0), Point::new(0.0, 8.0)];
        let other = vec![Point::new(-10.0, 3.0), Point::new(10.0, 3.0)];
        let gaps = crossing_gaps(cut, other);
        assert_eq!(gaps.len(), 1, "{gaps:?}");
        let (g0, g1) = &gaps[0];
        assert!(
            (g0.x - 0.0).abs() < 1e-9 && (g0.y - 0.0).abs() < 1e-9,
            "g0 must clamp to the polyline's own start: {g0:?}"
        );
        assert!(
            (g1.x - 0.0).abs() < 1e-9 && (g1.y - 8.0).abs() < 1e-9,
            "g1 must clamp to the polyline's own end: {g1:?}"
        );
        let width = (g1.x - g0.x).hypot(g1.y - g0.y);
        assert!(
            width < CROSSING_GAP,
            "a polyline shorter than CROSSING_GAP can only ever give back less than it: {width}"
        );
    }

    // -------------------------------------------------------------------------------------------
    // §10-3 item 5: two ordinary (non-perimeter) edges crossing — the horizontal side yields
    // -------------------------------------------------------------------------------------------

    /// Two genuinely forward edges (`tr > sr` on both, so neither is `reverse`, and both stay
    /// short enough that neither collides into a `staircase` fallback either — `crossing_gaps`'s
    // own `M`/`N` comment on why the non-cut helper edge already has to dodge that): a horizontal
    /// line crossed by a vertical one. §10-3 item 5 ("主辺どうしの交差は水平側が譲る") says the
    /// **horizontal** side gets the gap, regardless of which one this helper happens to call "cut"
    /// (its own `edge_id`) — the rule 4 tie-break `crossing_gaps` exercises elsewhere (higher edge
    /// id when *both* sides are perimeter edges) has no bearing here, since neither side is.
    #[allow(clippy::type_complexity)]
    fn main_vs_main_crossing_gaps(
        horiz_points: Vec<Point>,
        vert_points: Vec<Point>,
    ) -> (Vec<(Point, Point)>, Vec<(Point, Point)>) {
        let nodes = vec![
            node("H0", -1000.0, 0.0, 4.0, 4.0),
            node("H1", 1000.0, 0.0, 4.0, 4.0),
            node("V0", 0.0, -1000.0, 4.0, 4.0),
            node("V1", 0.0, 1000.0, 4.0, 4.0),
        ];
        let edges = [
            EligibleEdge {
                id: "horiz",
                source: "H0",
                target: "H1",
                raw: &[],
                source_rank: Some(0),
                target_rank: Some(1),
                source_out_degree: 1,
                target_in_degree: 1,
            },
            EligibleEdge {
                id: "vert",
                source: "V0",
                target: "V1",
                raw: &[],
                source_rank: Some(0),
                target_rank: Some(1),
                source_out_degree: 1,
                target_in_degree: 1,
            },
        ];
        let mut points = HashMap::new();
        points.insert("horiz".to_string(), horiz_points);
        points.insert("vert".to_string(), vert_points);
        let gaps = insert_crossing_gaps(Direction::TopToBottom, &nodes, &[], &edges, &points);
        (
            gaps.get("horiz").cloned().unwrap_or_default(),
            gaps.get("vert").cloned().unwrap_or_default(),
        )
    }

    #[test]
    fn two_main_edges_crossing_cut_the_horizontal_one_not_the_vertical_one() {
        let horiz = vec![Point::new(-50.0, 0.0), Point::new(50.0, 0.0)];
        let vert = vec![Point::new(0.0, -50.0), Point::new(0.0, 50.0)];
        let (horiz_gaps, vert_gaps) = main_vs_main_crossing_gaps(horiz, vert);
        assert_eq!(
            horiz_gaps.len(),
            1,
            "the horizontal edge must carry the gap: {horiz_gaps:?}"
        );
        assert!(
            vert_gaps.is_empty(),
            "the vertical edge must stay whole: {vert_gaps:?}"
        );
        let (g0, g1) = &horiz_gaps[0];
        assert!(
            (g0.y - 0.0).abs() < 1e-9 && (g1.y - 0.0).abs() < 1e-9,
            "the gap must sit on the horizontal edge's own (y=0) line: {horiz_gaps:?}"
        );
    }

    #[test]
    fn two_main_edges_crossing_ignore_which_one_is_named_first() {
        // The same crossing, but with the *vertical* line handed to the `"horiz"`-named edge (this
        // helper's own `edges[0]`, `insert_crossing_gaps`'s own `for i in 0..edges.len()` sweep's
        // `i`) and the horizontal line to `"vert"` (`edges[1]`, `j`) — the exact reverse pairing of
        // `two_main_edges_crossing_cut_the_horizontal_one_not_the_vertical_one`. If the outcome
        // there depended on `i`/`j` order rather than each segment's own orientation, swapping which
        // *name* gets which *shape* here would flip which edge carries the gap; it must not.
        let vert = vec![Point::new(0.0, -50.0), Point::new(0.0, 50.0)];
        let horiz = vec![Point::new(-50.0, 0.0), Point::new(50.0, 0.0)];
        let (horiz_named_gaps, vert_named_gaps) = main_vs_main_crossing_gaps(vert, horiz);
        assert!(
            horiz_named_gaps.is_empty(),
            "the edge named \"horiz\" is drawing the *vertical* line here and must stay whole: \
             {horiz_named_gaps:?}"
        );
        assert_eq!(
            vert_named_gaps.len(),
            1,
            "the edge named \"vert\" is drawing the *horizontal* line here and must carry the \
             gap: {vert_named_gaps:?}"
        );
    }

    // -------------------------------------------------------------------------------------------
    // §10-3 item 4: rank-skipping edges use the column gap upstream of the target
    // -------------------------------------------------------------------------------------------

    #[test]
    fn rank_lane_gap_bends_finds_the_single_gap_upstream_of_the_target() {
        // A (source, far upstream), C (target), and one obstacle B sitting between them — mirrors
        // `docs/mermaid-theme/handoff/round3-Konoma-Flowchart-Routing.dc.html`'s `3a`: `デコード →
        // セルに合わせる`'s own bend sits in the gap between `ラスタライズ`'s column (an
        // intervening rank's own node) and `セルに合わせる`'s own left edge, not at the raw
        // midpoint between the two nodes' own flow coordinates.
        let a = node("A", 0.0, 0.0, 80.0, 40.0); // right edge at 40
        let b = node("B", 300.0, 150.0, 200.0, 40.0); // spans x 200..400
        let c = node("C", 600.0, 300.0, 80.0, 40.0); // left edge at 560
        let nodes = vec![a.clone(), b, c.clone()];
        let bends = rank_lane_gap_bends(Direction::LeftToRight, &a, &c, Side::Left, &nodes, false);
        assert_eq!(
            bends.len(),
            1,
            "only B's own right edge (400) sits upstream of C's own left edge (560): {bends:?}"
        );
        assert!(
            bends[0] > 400.0 && bends[0] < 560.0,
            "the bend must sit inside the gap (400, 560): {}",
            bends[0]
        );
    }

    #[test]
    fn rank_lane_gap_bends_orders_multiple_gaps_nearest_target_first() {
        // Two obstacles between A and C: B (closer to C) and D (further upstream, closer to A —
        // but still within the nearer-half scope the sibling test below states explicitly).
        // The nearest-target gap (between B and C) must come first.
        let a = node("A", 0.0, 0.0, 80.0, 40.0);
        let d = node("D", 380.0, 150.0, 80.0, 40.0); // spans x 340..420
        let b = node("B", 500.0, 150.0, 100.0, 40.0); // spans x 450..550
        let c = node("C", 800.0, 300.0, 80.0, 40.0); // left edge at 760
        let nodes = vec![a.clone(), d, b, c.clone()];
        let bends = rank_lane_gap_bends(Direction::LeftToRight, &a, &c, Side::Left, &nodes, false);
        assert_eq!(bends.len(), 2, "{bends:?}");
        // First candidate: inside the gap between B's right edge (550) and C's left edge (760).
        assert!(
            bends[0] > 550.0 && bends[0] < 760.0,
            "nearest gap first: {bends:?}"
        );
        // Second candidate: inside the (narrow) gap between D's right edge (420) and B's left
        // edge (450) — the *next* gap starts on the far side of B's own body, never inside it.
        assert!(
            bends[1] > 420.0 && bends[1] < 450.0,
            "second candidate is the next gap out, past B's own body: {bends:?}"
        );
    }

    #[test]
    fn rank_lane_gap_bends_is_empty_when_target_is_the_first_column() {
        // Nothing sits upstream of C at all (A itself is excluded from the search — its own
        // rightward boundary can never legitimately supply a gap wall, `rank_lane_gap_bends`'s own
        // doc) — no gap exists, so no candidate is offered.
        let a = node("A", 0.0, 0.0, 80.0, 40.0);
        let c = node("C", 600.0, 0.0, 80.0, 40.0);
        let nodes = vec![a.clone(), c.clone()];
        let bends = rank_lane_gap_bends(Direction::LeftToRight, &a, &c, Side::Left, &nodes, false);
        assert!(bends.is_empty(), "{bends:?}");
    }

    #[test]
    fn rank_lane_gap_bends_stays_within_the_nearer_half_of_the_source_target_span() {
        // §10-3 item 4's own scope guard (`rank_lane_gap_bends`'s own doc): a wall sitting closer
        // to A than to C is never offered, even though it does sit "upstream" of C — searching that
        // far back risks landing inside a busy fanout's own bend corridor right next to the source
        // (`classify`'s own doc on why the `fan_lane` retry point does not use this search at all).
        // Here E sits at x=100..140, well within A's own half of the 0..600 span (A's own boundary
        // is 40, C's is 560 — the span midpoint in this search's own outward-distance metric lands
        // around x=300) — must not appear as a wall.
        let a = node("A", 0.0, 0.0, 80.0, 40.0);
        let e = node("E", 120.0, 150.0, 40.0, 40.0); // spans x 100..140, near A
        let c = node("C", 600.0, 300.0, 80.0, 40.0);
        let nodes = vec![a.clone(), e, c.clone()];
        let bends = rank_lane_gap_bends(Direction::LeftToRight, &a, &c, Side::Left, &nodes, false);
        assert!(
            bends.is_empty(),
            "a wall this close to A must be out of the nearer-half scope: {bends:?}"
        );
    }

    #[test]
    fn classify_uses_a_rank_lane_bend_when_both_ordinary_attempts_collide() {
        // Two obstacles, each blocking exactly one of `classify`'s first two attempts, neither
        // touching `A`'s own row (`y=0`) so the rank-lane candidate's own first leg — a long sweep
        // at `A`'s row, `3a`'s own `デコード → セルに合わせる` (`M620,434 H1000…`) does exactly
        // this, clearing `ラスタライズ`'s column because `ラスタライズ`'s own row does not reach
        // `y=434` — stays clear regardless of how far it has to run:
        // - `B1` sits across the merge shape's own straight vertical run (the flow/flow bridge's
        //   plain midpoint, roughly `x=500`), forcing that attempt to collide.
        // - `B2` sits across the branch-style alt's horizontal run (its own bend lands at `C`'s row,
        //   `y=300`, `bridge`'s `(Axis::Cross, Axis::Flow)` shape), forcing that attempt to collide
        //   too — but not across `B1`'s own column, so it does not also block the rank-lane
        //   candidate `classify` should fall back to.
        let a = node("A", 0.0, 0.0, 80.0, 40.0);
        let b1 = node("B1", 500.0, 150.0, 100.0, 280.0); // spans x 450..550, y 10..290
        let b2 = node("B2", 300.0, 300.0, 200.0, 40.0); // spans x 200..400, y 280..320
        let c = node("C", 1000.0, 300.0, 80.0, 40.0); // left edge at 960
        let nodes = vec![a.clone(), b1.clone(), b2.clone(), c.clone()];
        let shape = classify(
            Direction::LeftToRight,
            &a,
            &c,
            &[],
            Some(0),
            Some(1),
            1, // source_out_degree — not branching
            2, // target_in_degree — a genuine merge
            &nodes,
        );
        assert!(
            shape.rank_lane_bend.is_some(),
            "both ordinary attempts must have collided, landing on a rank-lane bend: {shape:?}"
        );
        assert!(
            !shape.staircase,
            "a working rank-lane bend must pre-empt staircase: {shape:?}"
        );
        assert_eq!(shape.source_axis, Axis::Flow, "{shape:?}");
        assert_eq!(shape.target_axis, Axis::Flow, "{shape:?}");
        // The route this shape actually draws must genuinely clear both obstacles — not merely
        // "classify says so", built and checked here the same way `route_flowchart` builds it for
        // real.
        let source_port = port_at(
            &a,
            shape.source_side,
            face_center_coord(&a, shape.source_side),
            PORT_INSET,
        );
        let target_port = port_at(
            &c,
            shape.target_side,
            face_center_coord(&c, shape.target_side),
            PORT_INSET,
        );
        let bend = shape.rank_lane_bend.unwrap();
        let mut pts = vec![source_port.clone()];
        pts.extend(bend_at(
            Direction::LeftToRight,
            bend,
            &source_port,
            &target_port,
        ));
        for w in pts.windows(2) {
            assert!(
                !segment_crosses_node(&w[0], &w[1], &b1),
                "segment {w:?} must not cross B1: {pts:?}"
            );
            assert!(
                !segment_crosses_node(&w[0], &w[1], &b2),
                "segment {w:?} must not cross B2: {pts:?}"
            );
        }
    }

    // --- §10-3 item 13: ports never move under local obstacle avoidance ------------------------

    /// §10-3 item 13's own motivating report (`docs/FEATURE-MERMAID-RENDERER.md`,
    /// `ページ描画→ラスタライズ`): a hand-built four-point route (the same shape
    /// `route_with_ports`'s `rank_lane_bend` branch always builds — a port, `bend_at`'s own two
    /// interior points, and the other port) whose *exit* stub (the very first segment) runs
    /// straight through `OBSTACLE`, sitting directly in `A`'s own row. Before this fix,
    /// `clear_local_route`'s blind "every point sharing this coordinate slides together" dragged
    /// `A`'s own port along with the rest of the run, off the exact face slot `evict` assigned it.
    #[test]
    fn clear_local_route_keeps_the_exit_port_pinned_and_still_clears_the_obstacle() {
        let a = node("A", 0.0, 100.0, 80.0, 40.0);
        let b = node("B", 400.0, 300.0, 80.0, 40.0);
        let obstacle = node("OBSTACLE", 120.0, 100.0, 80.0, 40.0);
        let nodes = vec![a.clone(), b.clone(), obstacle.clone()];

        let port_a = port_at(
            &a,
            Side::Right,
            face_center_coord(&a, Side::Right),
            PORT_INSET,
        );
        let port_b = port_at(
            &b,
            Side::Left,
            face_center_coord(&b, Side::Left),
            PORT_INSET,
        );
        let bend_flow = 200.0;
        let mut points = vec![port_a.clone()];
        points.extend(bend_at(Direction::LeftToRight, bend_flow, &port_a, &port_b));
        // The exit leg (port_a -> the first bend point) genuinely crosses OBSTACLE — confirms the
        // fixture actually exercises the fix rather than passing vacuously.
        assert!(
            segment_crosses_node(&points[0], &points[1], &obstacle),
            "fixture must genuinely cross OBSTACLE before the fix runs: {points:?}"
        );

        let fixed = clear_local_route(points, &nodes, (a.id.as_str(), b.id.as_str()));

        assert_eq!(
            fixed[0], port_a,
            "A's own port must stay exactly on its assigned face slot: {fixed:?}"
        );
        assert_eq!(
            *fixed.last().unwrap(),
            port_b,
            "B's own port must stay exactly on its assigned face slot: {fixed:?}"
        );
        assert!(
            (fixed[0].y - fixed[1].y).abs() < EPS && (fixed[0].x - fixed[1].x).abs() > EPS,
            "A must still leave its own Right face horizontally (perpendicular exit, §10-1 item \
             1): {fixed:?}"
        );
        for w in fixed.windows(2) {
            let (dx, dy) = ((w[1].x - w[0].x).abs(), (w[1].y - w[0].y).abs());
            assert!(
                dx < EPS || dy < EPS,
                "every segment must stay axis-parallel: {w:?} in {fixed:?}"
            );
            assert!(
                !segment_crosses_node(&w[0], &w[1], &obstacle),
                "the fixed route must still clear OBSTACLE: {w:?} in {fixed:?}"
            );
        }
    }

    /// The mirror image of the test above: the *entry* leg (the last segment, arriving at `B`'s
    /// own port) is the one that crosses `OBSTACLE` this time, not the exit leg. Pins that
    /// [`local_fix`]'s own near/far side selection for [`PortStub`] is not just a copy of the exit
    /// case's formula (an earlier draft used the same near-side choice for both ends, which built
    /// an entry stub that still ran straight through the obstacle on its way into the port — see
    /// `PortStub`'s own doc for why the two ends are mirror images, not the same answer).
    #[test]
    fn clear_local_route_keeps_the_entry_port_pinned_and_still_clears_the_obstacle() {
        let a = node("A", 0.0, 100.0, 80.0, 40.0);
        let b = node("B", 400.0, 300.0, 80.0, 40.0);
        let obstacle = node("OBSTACLE", 280.0, 300.0, 80.0, 40.0);
        let nodes = vec![a.clone(), b.clone(), obstacle.clone()];

        let port_a = port_at(
            &a,
            Side::Right,
            face_center_coord(&a, Side::Right),
            PORT_INSET,
        );
        let port_b = port_at(
            &b,
            Side::Left,
            face_center_coord(&b, Side::Left),
            PORT_INSET,
        );
        let bend_flow = 200.0;
        let mut points = vec![port_a.clone()];
        points.extend(bend_at(Direction::LeftToRight, bend_flow, &port_a, &port_b));
        let last = points.len() - 1;
        // The entry leg (the last bend point -> port_b) genuinely crosses OBSTACLE; the exit leg
        // does not (OBSTACLE sits on B's own row, not A's).
        assert!(
            segment_crosses_node(&points[last - 1], &points[last], &obstacle),
            "fixture must genuinely cross OBSTACLE before the fix runs: {points:?}"
        );
        assert!(
            !segment_crosses_node(&points[0], &points[1], &obstacle),
            "fixture's own exit leg must NOT cross OBSTACLE (isolates the entry-leg case): \
             {points:?}"
        );

        let fixed = clear_local_route(points, &nodes, (a.id.as_str(), b.id.as_str()));

        assert_eq!(
            fixed[0], port_a,
            "A's own port must stay exactly on its assigned face slot: {fixed:?}"
        );
        assert_eq!(
            *fixed.last().unwrap(),
            port_b,
            "B's own port must stay exactly on its assigned face slot: {fixed:?}"
        );
        let n = fixed.len();
        assert!(
            (fixed[n - 1].y - fixed[n - 2].y).abs() < EPS
                && (fixed[n - 1].x - fixed[n - 2].x).abs() > EPS,
            "B must still be entered horizontally on its own Left face (perpendicular entry, \
             §10-1 item 1): {fixed:?}"
        );
        for w in fixed.windows(2) {
            let (dx, dy) = ((w[1].x - w[0].x).abs(), (w[1].y - w[0].y).abs());
            assert!(
                dx < EPS || dy < EPS,
                "every segment must stay axis-parallel: {w:?} in {fixed:?}"
            );
            assert!(
                !segment_crosses_node(&w[0], &w[1], &obstacle),
                "the fixed route must still clear OBSTACLE: {w:?} in {fixed:?}"
            );
        }
    }
}
