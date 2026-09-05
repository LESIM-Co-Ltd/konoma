//! Drawing a parsed diagram — stages 1c and 1d of konoma's own mermaid renderer, and the shared
//! half of every stage after them.
//!
//! Stage 1b produced a [`Flowchart`]: what the author wrote. This module turns that into a
//! picture, and it is the first stage whose output a person can look at. Three more languages
//! have since joined it — [`state`], [`class`] and [`er`] — and each of them contributed a
//! translation into [`GraphSpec`] and nothing else: [`lay_out_spec`] and everything below it is
//! written once.
//!
//! ```text
//! Flowchart ──▶ measure labels ──▶ size the shapes ──▶ dagre ──▶ clip / curve / place ──▶ SVG
//! ```
//!
//! # The two halves of the pipeline, and why the seam is where it is
//!
//! [`lay_out`] stops at a [`Diagram`]: boxes with coordinates, edges with waypoints, labels with
//! their own rectangles — geometry, no colour, no markup. [`svg::emit`] turns that into text.
//! The seam is deliberate: every invariant worth checking about a diagram ("no two boxes overlap",
//! "no edge crosses the inside of a box", "the arrow tip is on the target's boundary") is a
//! statement about geometry, and checking it on a [`Diagram`] is a real test where checking it on
//! a string would be a parser for our own output. `docs/FEATURE-MERMAID-RENDERER.md` §6 asks for
//! exactly these checks, and for them to be seen failing before they are trusted.
//!
//! # Frames (stage 1d)
//!
//! A `subgraph` becomes a *cluster*: a parent in dagre's compound graph, drawn as a titled frame
//! around its members. [`clusters`] holds the parts dagre has no opinion about — resolving the
//! parser's flat list into a tree, growing a frame until its title fits, and cutting a line back
//! to the frame when the author wrote the block's name as an endpoint. Nothing here places a
//! frame: `set_parent` hands the nesting to dagre and the rectangle is read back out of it.
//!
//! **`direction` inside a block is read and not applied** — see [`lay_out`] for why.
//!
//! # Boxes that hold a table (stage 3)
//!
//! A class and an entity say several things at once, in compartments a reader scans down. That is
//! the one thing the seam could not express, so a node may now carry a [`Panel`]: rows, cells and
//! rules, in coordinates relative to the box, worked out by [`panel`] and only *translated* by
//! [`svg`]. It is geometry, so the invariant tests can state things about it.
//!
//! # On the drawing path (stage 1e)
//!
//! [`render`] is what `preview::markdown::mermaid_to_svg` calls for every `flowchart` / `graph` /
//! `flowchart-elk` konoma shows, and its siblings own the other twenty-two languages. There is no
//! second renderer left to ask: a diagram one of them *refuses* degrades to the Unicode text
//! diagram, so a failure here is visible rather than papered over.

// konoma is a binary crate, so `pub` marks nothing as used: the parts of this module's surface that
// exist for the tests and for the stages still to come look unreachable to rustc. Same reasoning,
// and the same lint, as the sibling `flowchart` and `layout` modules.
#![allow(dead_code)]

pub mod architecture;
pub mod band;
pub mod block;
pub mod c4;
pub mod chart;
pub mod class;
pub mod clusters;
pub mod edges;
pub mod er;
pub mod gantt;
pub mod gitgraph;
pub mod journey;
pub mod kanban;
pub mod labels;
pub mod mindmap;
pub mod orthogonal;
pub mod panel;
pub mod requirement;
pub mod sequence;
pub mod shapes;
pub mod state;
pub mod style;
pub mod svg;
pub mod theme;
pub mod timeline;
pub mod zenuml;

#[cfg(test)]
mod class_tests;
#[cfg(test)]
mod decoration_tests;
#[cfg(test)]
mod er_tests;
#[cfg(test)]
mod kinds_tests;
#[cfg(test)]
mod sequence_tests;
#[cfg(test)]
mod state_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod text_placement_tests;

use std::collections::{HashMap, HashSet};
use std::fmt;

use crate::preview::mermaid::flowchart::{
    self, Direction, Flowchart, LinkStyleTarget, ParseError, Shape, Stroke,
};
use crate::preview::mermaid::layout::{
    graph::{Graph, GraphOptions},
    layout, EdgeLabel, LayoutOptions, NodeLabel, Point, RankDir,
};
use crate::preview::mermaid::text_metrics;

pub use edges::{Curve, Tip};
pub use labels::Label;
pub use orthogonal::Routing;
pub use panel::Panel;
#[allow(unused_imports)]
pub use shapes::{Glyph, Mark, Size};
pub use style::ShapeStyle;
pub use theme::Theme;

/// Blank space kept around the drawing, in px. mermaid's `marginx`/`marginy` for dagre.
pub const MARGIN: f64 = 8.0;

/// Gap between two nodes on the same rank. mermaid's `flowchart.nodeSpacing`, default 50.
pub const NODE_SEP: f64 = 50.0;

/// Gap between ranks. mermaid's `flowchart.rankSpacing`, default 50. dagre halves it itself when
/// it makes room for edge labels.
pub const RANK_SEP: f64 = 50.0;

/// Gap between two edges sharing a rank. dagre's own default; mermaid does not override it.
pub const EDGE_SEP: f64 = 20.0;

/// `nodesep` (the gap between two nodes on the same rank) for `Routing::Orthogonal` only —
/// `NODE_SEP`'s own 50px is mermaid's `flowchart.nodeSpacing` default, tuned for splines curving
/// past each other; §10-1's token set is built entirely out of 8px multiples instead (port
/// pitch 16px, lane offset 8px, frame margin 16px), and the round-3 handoff's own reference
/// (`docs/mermaid-theme/handoff/round3-Konoma-Flowchart-Routing.dc.html`'s `3a`) draws its
/// densest column — `設定のルール`'s 10-way fan-out — at a measured ~56px row pitch against a
/// ~45px node height, i.e. roughly a 8-16px gap, not 50px (`docs/FEATURE-MERMAID-RENDERER.md`
/// §10-3 item 9, "行ピッチの均等・密"). Keeping the default `NODE_SEP` for this mode reproduces
/// dagre's ordinary rank-gap-sized breathing room between orthogonal boxes that are supposed to
/// sit right-angle-close to each other, which is exactly the "間延び" (elongation) the item's own
/// wording flags.
///
/// `24` rather than 3a's own ~16 is not the design token itself — it is the tightest value that
/// still clears the whole corpus's own invariants (bisected empirically: 16 and 20 both reproduce
/// a real self-crossing edge on the `branch` fixture, `orthogonal_no_edge_crosses_its_own_
/// endpoint_across_the_whole_corpus`'s own failure — two boxes packed that close leave no room
/// for a lane-offset detour between them once `evict`'s port spacing and `align_straight_lanes`'s
/// own overlap sweep both want their share of the same gap; 24 is the first value bisection found
/// where every corpus case stays clear). `docs/STATUS.md`'s "行ピッチの均等・密" note has the
/// before/after row-pitch measurement this constant produces on the round-3 reference diagram.
const ORTHO_NODE_SEP: f64 = 24.0;

/// Why a diagram could not be drawn.
///
/// The contract (`docs/FEATURE-MERMAID-RENDERER.md` §1) is that failing means *do not draw*: the
/// caller falls back to the text diagram. Every variant here is a case where drawing something
/// would be worse than drawing nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenderError {
    /// The source is not a flowchart konoma can read. Carries the parser's own reason, which is
    /// worth keeping: §8 notes that the current crate throws its error messages away.
    Parse(ParseError),
    /// The source is not a state diagram konoma can read.
    StateParse(crate::preview::mermaid::state::ParseError),
    /// The source is not a class diagram konoma can read.
    ClassParse(crate::preview::mermaid::class::ParseError),
    /// The source is not an ER diagram konoma can read.
    ErParse(crate::preview::mermaid::er::ParseError),
    /// The source is not a sequence diagram konoma can read.
    SequenceParse(crate::preview::mermaid::sequence::ParseError),
    /// The source is not the data chart konoma took it for.
    ChartParse(crate::preview::mermaid::chart::ParseError),
    /// The chart parsed, and then turned out to have no extent to draw on: every value zero,
    /// every point on one spot, an axis whose two ends are the same number.
    ///
    /// A separate variant from a parse error because it is a different kind of fact — the source
    /// is well formed and the *picture* is the thing that cannot exist — and because §1 says the
    /// message is the point.
    ChartHasNoExtent {
        /// What has no extent, in words.
        what: &'static str,
    },
    /// No sans-serif font could be resolved, so usvg would draw the boxes and silently drop every
    /// glyph inside them. Checked *before* an SVG is built, because there is no error later.
    ///
    /// A machine with no fonts of its own no longer lands here:
    /// [`crate::preview::svg::shared_fontdb`] registers an embedded sans-serif face as a
    /// last resort, so this is reachable only if even that face fails to load. The message stays
    /// as it is because it still says the true thing when it happens — there is no sans-serif to
    /// draw the labels with — and it is what sends the reader to the text diagram.
    NoFonts,
    /// Every node was dropped (only reachable if a chart's whole node list names subgraphs).
    NothingToDraw,
}

impl fmt::Display for RenderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RenderError::Parse(e) => write!(f, "{e}"),
            RenderError::StateParse(e) => write!(f, "{e}"),
            RenderError::ClassParse(e) => write!(f, "{e}"),
            RenderError::ErParse(e) => write!(f, "{e}"),
            RenderError::SequenceParse(e) => write!(f, "{e}"),
            RenderError::ChartParse(e) => write!(f, "{e}"),
            RenderError::ChartHasNoExtent { what } => write!(f, "{what}"),
            RenderError::NoFonts => write!(f, "no sans-serif font available for diagram labels"),
            RenderError::NothingToDraw => write!(f, "flowchart has nothing to draw"),
        }
    }
}

impl std::error::Error for RenderError {}

impl From<ParseError> for RenderError {
    fn from(e: ParseError) -> RenderError {
        RenderError::Parse(e)
    }
}

impl From<crate::preview::mermaid::state::ParseError> for RenderError {
    fn from(e: crate::preview::mermaid::state::ParseError) -> RenderError {
        RenderError::StateParse(e)
    }
}

impl From<crate::preview::mermaid::class::ParseError> for RenderError {
    fn from(e: crate::preview::mermaid::class::ParseError) -> RenderError {
        RenderError::ClassParse(e)
    }
}

impl From<crate::preview::mermaid::er::ParseError> for RenderError {
    fn from(e: crate::preview::mermaid::er::ParseError) -> RenderError {
        RenderError::ErParse(e)
    }
}

impl From<crate::preview::mermaid::sequence::ParseError> for RenderError {
    fn from(e: crate::preview::mermaid::sequence::ParseError) -> RenderError {
        RenderError::SequenceParse(e)
    }
}

impl From<crate::preview::mermaid::chart::ParseError> for RenderError {
    fn from(e: crate::preview::mermaid::chart::ParseError) -> RenderError {
        RenderError::ChartParse(e)
    }
}

/// One node, placed.
#[derive(Debug, Clone, PartialEq)]
pub struct PlacedNode {
    /// The id from the source.
    pub id: String,
    /// The outline to draw.
    pub shape: Glyph,
    /// Centre of the bounding box.
    pub center: Point,
    /// Bounding box, as [`shapes::size`] computed it and as dagre laid it out.
    pub size: Size,
    /// The measured label drawn inside. Blank for a node whose text lives in its [`panel`].
    ///
    /// [`panel`]: PlacedNode::panel
    pub label: Label,
    /// The compartments drawn inside a `classBox` / `erBox`, in coordinates relative to the box's
    /// top-left corner. `None` for every glyph whose whole content is one centred label.
    pub panel: Option<Panel>,
    /// Which entry of the palette's categorical [`series`](Theme::series) paints this node.
    ///
    /// `None` — every node in every diagram kind before stage 5 — means the ordinary node colours,
    /// so the four graph languages emit exactly what they emitted before this field existed. A
    /// chart's *data* carries one and a chart's *furniture* (frame, axis, tick label) does not,
    /// which is what makes "a legend entry is the colour of its series" a thing a test can state.
    pub series: Option<usize>,
    /// Geometry the bounding box cannot express — a wedge's angles, a ribbon's four corners.
    /// `None` for every glyph that does not need one; see [`shapes::Mark`].
    pub mark: Option<shapes::Mark>,
    /// The resolved `classDef` / `class` / `:::` / `style` paint, if the source declared any.
    /// `None` — every node before `style::cascade` had anywhere to write to — draws exactly what
    /// it always drew; see [`style`] for why this is not folded into [`PlacedNode::series`].
    pub style: Option<ShapeStyle>,
}

impl PlacedNode {
    /// Left, top, right, bottom of the bounding box.
    pub fn bounds(&self) -> (f64, f64, f64, f64) {
        (
            self.center.x - self.size.w / 2.0,
            self.center.y - self.size.h / 2.0,
            self.center.x + self.size.w / 2.0,
            self.center.y + self.size.h / 2.0,
        )
    }
}

/// An edge's label, placed on the line.
#[derive(Debug, Clone, PartialEq)]
pub struct PlacedEdgeLabel {
    /// Centre of the label's box.
    pub center: Point,
    /// The box, which is the measured text plus a little breathing room.
    pub size: Size,
    /// The measured text.
    pub label: Label,
}

/// One edge, routed.
#[derive(Debug, Clone, PartialEq)]
pub struct PlacedEdge {
    /// Source node id.
    pub from: String,
    /// Target node id.
    pub to: String,
    /// The polyline **after clipping to the two shapes** and before the corners are rounded.
    /// This is what the geometric invariants are stated about, and what the label is centred on.
    pub points: Vec<Point>,
    /// §10-1 item 4's 12px crossing gaps — each `(a, b)` is one closed sub-interval of `points` to
    /// leave undrawn, so the line this edge draws reads as passing *under* whatever it crosses
    /// rather than as a plain "+" intersection with no depth of its own. `a` and `b` are always
    /// exactly [`orthogonal::CROSSING_GAP`] px apart **by arc length along `points`**, centred on
    /// the crossing point that asked for them — usually both on the same original segment, but not
    /// always: a crossing point close enough to a corner (`orthogonal::gap_around`'s own doc)
    /// makes the gap continue past that corner onto the next segment, so `a` and `b` can land on
    /// two different segments, in which case the straight-line distance between them is *shorter*
    /// than `CROSSING_GAP` even though the arc length removed is not.
    ///
    /// Empty for every edge before `[ui] mermaid_routing` existed, and empty still for `"splines"`
    /// and for every diagram kind but a flowchart's own orthogonal-routed edges — `svg::emit_edge`
    /// draws the single `<path>` it always drew whenever this is empty, so nothing about the
    /// existing rendering byte stream can move just because this field now exists. Only
    /// `orthogonal::insert_crossing_gaps` (§10-1 item 4, "跨ぐ側の線に12pxの隙間を開ける") ever puts
    /// anything in it, and only for the **spanning** side of a crossing (a perimeter-routed edge —
    /// a back edge or a collision-fallback forward one, never a self-loop): the edge it crosses is
    /// left completely untouched. §10-1's own "下をくぐる線は連続のまま" reads the gap as depth —
    /// the interrupted line is the one that visually dips *under*, the untouched one stays on top.
    pub gaps: Vec<(Point, Point)>,
    /// What is drawn where the line meets `from`.
    pub tip_start: Tip,
    /// What is drawn where the line meets `to`.
    pub tip_end: Tip,
    /// Line style.
    pub stroke: Stroke,
    /// The label, if the edge carries one.
    pub label: Option<PlacedEdgeLabel>,
    /// Text drawn beside the line at the `from` end — a class diagram's cardinality.
    pub start_label: Option<PlacedEdgeLabel>,
    /// Text drawn beside the line at the `to` end.
    pub end_label: Option<PlacedEdgeLabel>,
    /// A number drawn **on** the line at the sending end, on a disc of its own — a sequence
    /// diagram's `autonumber`.
    ///
    /// Not a `start_label`: that one is set *beside* the line and left bare, which is right for a
    /// cardinality and wrong for a sequence number. mermaid puts the number on the shaft, and the
    /// disc is what stops the shaft running through the digits.
    pub badge: Option<PlacedEdgeLabel>,
    /// Which entry of the palette's categorical [`series`](Theme::series) this line is drawn in.
    ///
    /// `None` is [`Theme::line`], which is every edge in every diagram kind before stage 5. A
    /// chart's line plot and a radar chart's curve are the only things that set it, and they must:
    /// a legend that names three series is meaningless if all three are the same colour.
    pub series: Option<usize>,
    /// Whether the route is drawn as a **polyline** rather than as a smooth curve.
    ///
    /// A route through a layered layout is smoothed, because dagre's waypoints are a suggestion
    /// and the curve is what makes a long edge read as one line. A route on a **grid** is not: an
    /// architecture edge's elbow and a block link's detour are right angles the author asked for,
    /// and a spline through them draws a wide arc that wanders across the cells in between.
    ///
    /// Before this field existed, `series` doubled as the flag — a data path is straight for the
    /// same reason — so a straight route had to pretend to be a series and take a series colour
    /// with it. Saying it directly is what lets a grid route be [`Theme::line`] and straight at
    /// once.
    pub straight: bool,
    /// Whether this line is drawn **over** the nodes rather than under them.
    ///
    /// `false` is the ordinary depth and the one every route wants: a flowchart's arrow, a
    /// sequence message, a chart's axes and grid, a git graph's lane. Structure goes under the
    /// marks that stand on it, so a box covers the line that arrives at it and a label covers the
    /// line that runs beneath it — which is what makes a label readable without an opaque patch.
    ///
    /// `true` is for a **data path** — an xy chart's line plot and a radar chart's curve. A line
    /// plot drawn under the bars disappears behind the tall ones, which is the one place it most
    /// needs to be visible; and unlike a route it is not structure the marks are allowed to cover,
    /// it is a second reading of the same numbers laid on the first so the two can be compared.
    ///
    /// **This is the same mistake [`straight`] was made to fix**, one field along: `series` used to
    /// double as the flag, so anything that wanted to be drawn on top had to claim to be a series
    /// — and a git graph's lane line, which carries a series because a lane is a colour, was drawn
    /// over its own captions and tags and struck the words through. Depth is a property of *what a
    /// line is*, not of which colour it takes.
    ///
    /// [`straight`]: PlacedEdge::straight
    pub overlay: bool,
    /// The resolved `class` / `:::` / `linkStyle` paint, if the source declared any for this
    /// edge. `None` draws exactly what every edge drew before `style::cascade_edge` existed; see
    /// [`style`] for why this is not folded into [`PlacedEdge::series`].
    pub style: Option<ShapeStyle>,
    /// Which curve draws the line, when neither [`PlacedEdge::series`] nor
    /// [`PlacedEdge::straight`] already decided it (`svg::emit_edge`'s branch).
    ///
    /// Only a flowchart's own edges ever resolve to anything but [`Curve::Basis`] — `curve` is
    /// mermaid's `flowchart.curve` / `linkStyle ... interpolate`, and every other diagram kind
    /// that reaches this struct (state, class, ER, C4, requirement, mindmap, sequence, git graph,
    /// architecture, block) sets this to `Basis` unconditionally, which is exactly what it drew
    /// before this field existed.
    pub curve: Curve,
    /// Whether the terminal mark (`svg::emit_tip`) is painted the line's own resolved stroke
    /// colour rather than [`Theme::arrowhead`].
    ///
    /// `false` — every edge before `[ui] mermaid_routing` existed, and every edge of every kind
    /// but a flowchart's own orthogonal-routed one today — draws exactly what it always drew:
    /// mermaid's own convention of one fixed arrowhead colour regardless of the line's `class` /
    /// `:::` / `linkStyle`. `true` is set only by `lay_out_spec`'s orthogonal branch, for
    /// `docs/FEATURE-MERMAID-RENDERER.md` §10-1 item 5 — "辺・矢尻・ラベル文字・ノード枠を同色で
    /// 揃える" — which `[ui] mermaid_routing = "splines"` (mermaid's own look) does not ask for.
    pub tip_matches_line: bool,
}

impl PlacedEdge {
    /// The polyline the curve is actually built from: corners rounded off (`edges::fix_corners`)
    /// **only when [`PlacedEdge::curve`] wants that** ([`Curve::rounds_corners`]).
    ///
    /// **A series edge keeps its own vertices.** Rounding a right angle replaces the corner with
    /// two points either side of it, which for a route is an improvement and for a *data path* is
    /// a value moved: an xy chart's line and a radar chart's curve both have vertices that are
    /// the data, and two consecutive equal values make exactly the right angle `fix_corners`
    /// would take out.
    ///
    /// **Every route curve but `Rounded` gets its corners rounded first, `linear`/`step*`
    /// included** — see [`Curve::rounds_corners`]'s own doc for the versioned evidence
    /// (`mermaid@11.4.1`/`mermaid@11.12.0` run `fixCorners` unconditionally). `Rounded` is the
    /// one exception, and only because it rounds every corner itself at its own radius, so
    /// running `fix_corners` first would round the same corner twice.
    pub fn drawn_points(&self) -> Vec<Point> {
        if self.series.is_some() || self.straight || !self.curve.rounds_corners() {
            return self.points.clone();
        }
        edges::fix_corners(&self.points)
    }
}

/// One subgraph frame, placed.
///
/// A cluster is not a node: it has no shape of its own, it is never an endpoint dagre knows
/// about, and its rectangle is *derived* from where its members ended up. What it does have is a
/// title, and the room that title needs is the only thing about a frame that is not decided by
/// the layout engine.
#[derive(Debug, Clone, PartialEq)]
pub struct PlacedCluster {
    /// The subgraph id, written or generated.
    pub id: String,
    /// The measured title. Blank for `subgraph` with no title at all.
    pub title: Label,
    /// Centre of the frame.
    pub center: Point,
    /// The frame, after it has been grown to hold its title.
    pub size: Size,
    /// Id of the enclosing cluster, if this one is nested.
    pub parent: Option<String>,
    /// 0 for a top-level block, 1 for a block inside it, and so on.
    pub depth: usize,
    /// Whether the frame is drawn dashed — one concurrent region of a state diagram's `--`, and
    /// every `loop` / `alt` / `opt` / `par` / `critical` / `break` of a sequence diagram.
    pub dashed: bool,
    /// Whether the frame's interior is painted.
    ///
    /// A `subgraph` is painted: it is ground, and the nodes sit on it. A sequence diagram's group
    /// frame is not, for two reasons — it is crossed by every lifeline in it, and frames nest, so
    /// two paints stack into a darker slab that means nothing.
    pub filled: bool,
    /// Horizontal rules that divide the frame into sections, with the text that names each one.
    ///
    /// Only a sequence diagram's `else` / `and` / `option` fills this. Empty everywhere else.
    pub sections: Vec<ClusterSection>,
    /// §10-5 S2: draw the title as a filled strip along the frame's own top edge, rather than the
    /// ordinary centred word — the composite-state look `konoma-orthogonal` gives a state
    /// diagram's `state X { ... }` frame. Set only by `state::lay_out` when its own `routing` is
    /// `Routing::Orthogonal`; `false` everywhere else (every flowchart subgraph, and a state
    /// diagram's own splines rendering, unconditionally), so `svg::emit_cluster_title`'s existing
    /// centred style is the only one any other caller can ever reach.
    pub title_strip: bool,
}

/// One `else` / `and` / `option` inside a frame.
#[derive(Debug, Clone, PartialEq)]
pub struct ClusterSection {
    /// Absolute y of the rule that divides the section above from the section below.
    pub y: f64,
    /// The text drawn just under the rule.
    pub title: Label,
}

impl PlacedCluster {
    /// Left, top, right, bottom of the frame.
    pub fn bounds(&self) -> (f64, f64, f64, f64) {
        (
            self.center.x - self.size.w / 2.0,
            self.center.y - self.size.h / 2.0,
            self.center.x + self.size.w / 2.0,
            self.center.y + self.size.h / 2.0,
        )
    }

    /// Centre of the title's line block: horizontally centred, sitting just inside the top edge.
    pub fn title_center(&self) -> Point {
        let (_, top, _, _) = self.bounds();
        Point::new(
            self.center.x,
            top + clusters::TITLE_PAD_Y + self.title.height / 2.0,
        )
    }
}

/// A whole diagram, in px, with the origin at the top-left of the drawing.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Diagram {
    /// Overall width, margins included.
    pub width: f64,
    /// Overall height, margins included.
    pub height: f64,
    /// Nodes, in source order.
    pub nodes: Vec<PlacedNode>,
    /// Edges, in source order. Edges whose endpoints are not both nodes are not here.
    pub edges: Vec<PlacedEdge>,
    /// Subgraph frames, outermost first, so drawing them in order nests them correctly.
    pub clusters: Vec<PlacedCluster>,
    /// A sequence diagram's lifelines, in participant order. Empty for every other kind.
    ///
    /// This is the one piece of geometry stage 4 had to add rather than reuse. A lifeline is not
    /// a node (it has no label and nothing ends on its outline) and not an edge (it joins nothing
    /// to anything); it is the *axis* a participant occupies, and the activation bars belong to it
    /// rather than sitting beside it — which is what makes "a bar is on its own lifeline"
    /// true by construction and "a bar spans from its activating message to its deactivating one"
    /// the thing left for a test to state.
    pub lifelines: Vec<PlacedLifeline>,
}

/// One participant's vertical axis, and the activation bars on it.
#[derive(Debug, Clone, PartialEq)]
pub struct PlacedLifeline {
    /// The participant's id.
    pub id: String,
    /// The x every message to or from this participant is anchored on.
    pub x: f64,
    /// Where the line starts — the bottom of the participant's box.
    pub top: f64,
    /// Where it ends: the top of the mirrored box at the foot of the diagram, or the message that
    /// destroyed the participant.
    pub bottom: f64,
    /// Whether the line ends in a cross because the participant was `destroy`ed.
    pub destroyed: bool,
    /// The bars, in the order they were opened.
    pub activations: Vec<PlacedActivation>,
}

impl PlacedLifeline {
    /// The rectangle of the `i`th activation bar, as `(l, t, r, b)`.
    pub fn activation_bounds(&self, i: usize) -> Option<(f64, f64, f64, f64)> {
        let a = self.activations.get(i)?;
        let left = self.x - ACTIVATION_WIDTH / 2.0 + a.depth as f64 * ACTIVATION_WIDTH;
        Some((left, a.top, left + ACTIVATION_WIDTH, a.bottom))
    }

    /// How far the bars reach to each side of the axis, as `(left, right)` absolute x.
    pub fn extent(&self) -> (f64, f64) {
        let mut left = self.x;
        let mut right = self.x;
        for i in 0..self.activations.len() {
            if let Some((l, _, r, _)) = self.activation_bounds(i) {
                left = left.min(l);
                right = right.max(r);
            }
        }
        (left, right)
    }
}

/// One activation bar.
#[derive(Debug, Clone, PartialEq)]
pub struct PlacedActivation {
    /// How many bars were already open on this lifeline when this one opened. 0 sits centred on
    /// the axis and each one after it steps a full width to the right, so the bars form a
    /// staircase and never overlap.
    pub depth: usize,
    /// Y of the message that opened it.
    pub top: f64,
    /// Y of the message that closed it, or the foot of the lifeline if nothing did.
    pub bottom: f64,
}

/// Width of an activation bar. mermaid's `sequence.activationWidth`, schema default 10.
pub const ACTIVATION_WIDTH: f64 = 10.0;

impl Diagram {
    /// Looks a placed node up by id.
    pub fn node(&self, id: &str) -> Option<&PlacedNode> {
        self.nodes.iter().find(|n| n.id == id)
    }

    /// Looks a placed cluster up by id.
    pub fn cluster(&self, id: &str) -> Option<&PlacedCluster> {
        self.clusters.iter().find(|c| c.id == id)
    }
}

/// Reads a mermaid flowchart source and draws it.
///
/// **This is the entry point the golden tests go through**, so that what they pin is what a caller
/// gets — `docs/FEATURE-MERMAID-RENDERER.md` §6 ("ゴールデンは本番の入口関数を通すこと"), after
/// konoma was once caught pinning a function that was not the one in production.
///
/// `theme` is `ui.mermaid_theme`'s raw string; an unknown value silently means `dark`. Draws every
/// edge in [`Curve::Basis`] — the same as [`render_curve`] called with `"basis"` — which is what
/// keeps this the byte-stable entry point every golden test and every non-flowchart caller goes
/// through.
pub fn render(code: &str, theme: &str) -> Result<String, RenderError> {
    render_curve(code, theme, "basis")
}

/// [`render`], with the curve every edge draws in resolved from `curve` — `ui.mermaid_curve`'s raw
/// string, `Curve::Basis`("basis") reproducing [`render`] exactly. An individual edge's own
/// `linkStyle ... interpolate <curve>` wins over this default; see [`spec_of`].
///
/// [`render_flow`] with `routing` fixed at `"splines"` — every existing caller (the golden tests
/// included) keeps this exact three-argument signature; `[ui] mermaid_routing` reaches konoma's
/// own renderer only through [`render_flow`].
pub fn render_curve(code: &str, theme: &str, curve: &str) -> Result<String, RenderError> {
    render_flow(code, theme, curve, "splines")
}

/// [`render_curve`], with a flowchart's edges routed by `routing` — `[ui] mermaid_routing`'s raw
/// string. `"splines"` reproduces [`render_curve`] exactly, byte for byte; `"konoma-orthogonal"` is
/// `docs/FEATURE-MERMAID-RENDERER.md` §10's right-angle wiring mode. `curve` stays flowchart-only
/// (every other diagram kind ignores it, always `Curve::Basis`); `routing` does not — a state
/// diagram reads it too, through its own entry point ([`state::render_flow`], §10-5), while the
/// rest still ignore it and stay `Routing::Splines` (`GraphSpec::routing`'s own doc).
///
/// `theme` is `[ui] mermaid_theme`'s raw string and reaches the drawing only under `"splines"`:
/// `"konoma-orthogonal"` carries its own palette, for the reason [`Theme::for_routing`] gives.
pub fn render_flow(
    code: &str,
    theme: &str,
    curve: &str,
    routing: &str,
) -> Result<String, RenderError> {
    let chart = flowchart::parse(code)?;
    let diagram = lay_out_flow(&chart, curve, routing)?;
    Ok(svg::emit(
        &diagram,
        &Theme::for_routing(theme, Routing::parse(routing)),
    ))
}

/// Measures, sizes, lays out and routes — everything except turning geometry into markup.
///
/// # `direction` inside a `subgraph` is read and deliberately not applied
///
/// The parser keeps it ([`flowchart::Subgraph::direction`]); this function ignores it, and the
/// whole chart is laid out with one `rankdir`. That is a choice, so here is the reasoning.
///
/// dagre has exactly one `rankdir` per layout. Honouring a per-block direction therefore means
/// laying that block out in a **graph of its own** and inserting the result into the parent as an
/// opaque box — which is what mermaid does, and it is why mermaid can only do it for a block that
/// has *no edge crossing its border*: a member laid out in a separate coordinate space cannot be
/// ranked against a node outside it. Three things follow, and together they decide it:
///
/// 1. **It would work only sometimes, silently.** Upstream's own documentation says "if any of a
///    subgraph's nodes are linked to the outside, subgraph direction will be ignored" — and
///    linking to the outside is the ordinary case. A statement that does nothing, with no way to
///    tell that it did nothing, reads as a bug rather than as a limitation.
/// 2. **Upstream tried the other way and took it back.** 11.16.0 made dagre respect it; 11.17.0
///    reverted because the arrows *between* subgraphs broke. Those arrows are the part of a
///    diagram that carries the meaning, so trading them for an internal axis is the wrong trade
///    under §0-1 ("the same source read correctly").
/// 3. **One layout is one coordinate space, and that is what the tests can check.** Every cluster
///    invariant this stage adds — a frame holds its members, a nested frame is inside its parent,
///    a line does not run through a frame it has nothing to do with — is a statement about one
///    geometry. A second, independent layout inside a box would be geometry nothing checks;
///    upstream's own code says as much, in the note on `applyDagreLayoutResult` that consumers
///    "must not assume it is a complete description of the rendered output for cluster diagrams".
///
/// What the reader loses is the internal axis of a block. What they keep is every node, every
/// edge, and every frame the source declared, which is the degradation §0-1 asks for. If a later
/// stage wants the axis back, the place to start is a nested layout — and
/// [`super::tests::a_block_direction_does_not_move_anything`] is the test it has to change.
pub fn lay_out(chart: &Flowchart) -> Result<Diagram, RenderError> {
    lay_out_curve(chart, "basis")
}

/// [`lay_out`], with every edge's curve resolved from `curve` the way [`render_curve`] resolves
/// it — see [`spec_of`] for exactly how a per-edge `linkStyle interpolate` overrides it.
///
/// [`lay_out_flow`] with `routing` fixed at `"splines"` — kept at this exact two-argument
/// signature for the same reason [`render_curve`] is.
pub fn lay_out_curve(chart: &Flowchart, curve: &str) -> Result<Diagram, RenderError> {
    lay_out_flow(chart, curve, "splines")
}

/// [`lay_out_curve`], with a flowchart's edges routed by `routing` — see [`render_flow`].
pub fn lay_out_flow(chart: &Flowchart, curve: &str, routing: &str) -> Result<Diagram, RenderError> {
    // The gate. usvg does not fail on a missing font, it just drops the glyphs, so the only place
    // this can be caught is before anything is built (PRD design principle #3).
    if !text_metrics::fonts_available() {
        return Err(RenderError::NoFonts);
    }
    if chart.nodes.is_empty() {
        return Err(RenderError::NothingToDraw);
    }
    lay_out_spec(&spec_of(chart, curve, Routing::parse(routing)))
}

/// A flowchart, measured and sized, as the language-neutral [`GraphSpec`] the layout reads.
///
/// This is the whole of what is specific to the flowchart language: a label is measured, a shape
/// is sized around it, and a `subgraph` becomes a block. Everything after this point —
/// [`lay_out_spec`] — is shared with the state diagram, and that sharing is the point
/// (`docs/FEATURE-MERMAID-RENDERER.md` §7: stage 2 reuses stage 1's layout).
///
/// `curve` is the caller's own default (`ui.mermaid_curve`'s raw string — permissive, so an
/// unknown spelling reaching here still ends up `Curve::Basis` once [`Curve::parse`] sees it). The
/// full priority order, weakest first, mirrors mermaid's own config cascade exactly:
///
/// 1. `curve` — this function's own parameter, `[ui] mermaid_curve`;
/// 2. `chart.curve` — `%%{init: {"flowchart": {"curve": ...}}}%%`, read once per chart
///    (`preprocess::init_flowchart_curve`) and overriding 1 whenever the source declared one;
/// 3. `linkStyle default interpolate` — overrides 1 and 2 for every edge that names no index of
///    its own, the same `style::cascade_edge` already runs for paint;
/// 4. `linkStyle <n> interpolate` — this edge's own index, overriding all three.
///
/// Only the flowchart language reads any of `%%{init}%%`'s `flowchart.curve` or `interpolate` at
/// all — every other diagram kind's own `spec_of` sets [`SpecEdge::curve`] to `Curve::Basis`
/// unconditionally.
fn spec_of(chart: &Flowchart, curve: &str, routing: Routing) -> GraphSpec {
    // Step 2 of the priority order above: `%%{init}%%`'s own `flowchart.curve`, read once, wins
    // over the caller's `ui.mermaid_curve` default for the rest of this function — every
    // `linkStyle interpolate` cascade below now overrides *this*, not the raw parameter.
    let curve = chart.curve.as_deref().unwrap_or(curve);
    // `classDef default` → the classes a node/edge carries, in applied order → its own `style` —
    // see `style::cascade`'s docs for the pipeline this closure feeds.
    let class_of = |name: &str| {
        chart
            .class_defs
            .iter()
            .find(|d| d.name == name)
            .map(|d| d.styles.as_slice())
    };
    let nodes = chart
        .nodes
        .iter()
        .map(|node| {
            // A decision node under orthogonal routing draws as a chamfered rectangle rather than
            // a diamond — see `Glyph::ChamferedRect`'s own docs for why. Nothing else changes: the
            // parsed `Shape` konoma keeps for every other reader of `chart` (`docs`, `%%{init}%%`
            // round-tripping, …) is untouched, only the glyph this one node is drawn as.
            let glyph = if routing == Routing::Orthogonal && node.shape == Shape::Diamond {
                Glyph::ChamferedRect
            } else {
                Glyph::Flow(node.shape)
            };
            // §10-8 N1–N3, and only under orthogonal routing: a declared 36px-tall box on an 8px
            // width grid, with the label re-wrapped if it would push past the 240px cap.
            // `orthogonal_node` answers `None` for anything N3 leaves alone, and `Routing::Splines`
            // never asks — so the `unwrap_or_else` arm below is byte-for-byte the path this
            // function has always taken.
            let (label, size) = (routing == Routing::Orthogonal)
                .then(|| shapes::orthogonal_node(glyph, &node.label))
                .flatten()
                .unwrap_or_else(|| {
                    let label = Label::measure(&node.label);
                    let size = shapes::size(glyph, Size::new(label.width, label.height));
                    (label, size)
                });
            SpecNode {
                id: node.id.clone(),
                glyph,
                label,
                size,
                panel: None,
                style: style::cascade(class_of, &node.classes, &node.styles),
                has_class: !node.classes.is_empty(),
            }
        })
        .collect();
    let blocks = chart
        .subgraphs
        .iter()
        .map(|s| SpecBlock {
            id: s.id.clone(),
            title: s.title.clone(),
            members: s.members.clone(),
            dashed: false,
        })
        .collect();
    let edges = chart
        .edges
        .iter()
        .enumerate()
        .map(|(i, edge)| {
            // `linkStyle` indices are positions in `Flowchart::edges` (the model's own doc
            // comment), which is exactly this iterator's index.
            let link_default = chart.link_styles.iter().filter_map(|ls| {
                matches!(ls.target, LinkStyleTarget::Default).then_some(ls.styles.as_slice())
            });
            let link_indexed = chart.link_styles.iter().filter_map(|ls| match &ls.target {
                LinkStyleTarget::Indices(idx) if idx.contains(&i) => Some(ls.styles.as_slice()),
                _ => None,
            });
            // Same cascade as the paint above, over `interpolate` instead of `styles`: chart-wide
            // default, then `linkStyle default interpolate`, then this edge's own
            // `linkStyle <n> interpolate` — each iterator's *last* match wins, exactly as
            // `style::cascade_edge` applies its declarations in order and lets a later one
            // overwrite an earlier one.
            let default_interpolate = chart
                .link_styles
                .iter()
                .filter(|ls| matches!(ls.target, LinkStyleTarget::Default))
                .filter_map(|ls| ls.interpolate.as_deref())
                .next_back();
            let indexed_interpolate = chart
                .link_styles
                .iter()
                .filter(
                    |ls| matches!(&ls.target, LinkStyleTarget::Indices(idx) if idx.contains(&i)),
                )
                .filter_map(|ls| ls.interpolate.as_deref())
                .next_back();
            let resolved_curve = indexed_interpolate.or(default_interpolate).unwrap_or(curve);
            SpecEdge {
                id: edge.id.clone(),
                from: edge.from.clone(),
                to: edge.to.clone(),
                label: edge
                    .label
                    .as_deref()
                    .map(Label::measure)
                    .filter(|l| !l.is_blank()),
                tip_start: edges::Tip::of_arrow(edge.arrow).0,
                tip_end: edges::Tip::of_arrow(edge.arrow).1,
                stroke: edge.stroke,
                minlen: edge.length,
                start_label: None,
                end_label: None,
                style: style::cascade_edge(class_of, &edge.classes, link_default, link_indexed),
                curve: Curve::parse(resolved_curve),
            }
        })
        .collect();
    GraphSpec {
        direction: chart.direction,
        nodes,
        edges,
        blocks,
        routing,
        // A flowchart's own self-loop keeps its pre-existing, dagre-derived shape — §10-5 S3 is a
        // state-diagram-only extension (this struct's own field doc).
        fixed_self_loops: false,
    }
}

/// One node to lay out: already measured, already sized, and no longer tied to a language.
#[derive(Debug, Clone, PartialEq)]
pub struct SpecNode {
    /// The id edges refer to it by.
    pub id: String,
    /// What it is drawn as.
    pub glyph: Glyph,
    /// The measured label drawn inside it.
    pub label: Label,
    /// The bounding box dagre lays out.
    pub size: Size,
    /// The compartments drawn inside it, for a glyph whose content is a table rather than one
    /// centred label. Sized already: `size` is the panel's own size for such a node.
    pub panel: Option<Panel>,
    /// The resolved `classDef`/`style` paint, carried through to [`PlacedNode::style`] unchanged.
    pub style: Option<ShapeStyle>,
    /// Whether the author attached at least one `classDef`-backed class (`class`/`:::`) to this
    /// node — *not* whether it carries any resolved paint at all, which an inline `style`
    /// statement alone (with no `class`) also produces. Only a flowchart's own `spec_of` ever
    /// sets this meaningfully (`chart.nodes[i].classes.is_empty()`); every other diagram kind
    /// leaves it `true`, a harmless default — `regroup_fan_lanes` is the only reader, and while
    /// `Routing::Orthogonal` is no longer flowchart-exclusive (a state diagram may request it too,
    /// §10-5), a state node left at `true` is simply never treated as the classless/dead-end group
    /// rule that field distinguishes; nothing in §10-5's own rule set (S1–S5) depends on it. It
    /// exists
    /// because [`GraphSpec`] is deliberately language-neutral (this struct's own doc) so it
    /// cannot carry a raw class *name*, but the fan-lane regroup still needs to tell "this node
    /// belongs to one of the diagram's semantic colour groups" apart from "this node was only
    /// ever given an ad-hoc `style` override" (`docs/FEATURE-MERMAID-RENDERER.md` §10-3's own
    /// "無クラス・行き止まりを外側" rule) — a distinction [`crate::preview::mermaid::render::style::
    /// ShapeStyle`]'s resolved colours cannot make on their own, since an inline `style` resolves
    /// to paint just as real as a `classDef`'s.
    pub has_class: bool,
}

/// One edge to lay out. `from`/`to` are as the author wrote them, so either may name a block.
#[derive(Debug, Clone, PartialEq)]
pub struct SpecEdge {
    /// Unique within the spec; used as dagre's edge name so parallel edges stay apart.
    pub id: String,
    /// Source, as written.
    pub from: String,
    /// Target, as written.
    pub to: String,
    /// The measured label, if the edge carries one.
    pub label: Option<Label>,
    /// What is drawn where the line meets `from`.
    pub tip_start: Tip,
    /// What is drawn where the line meets `to`.
    pub tip_end: Tip,
    /// Line style.
    pub stroke: Stroke,
    /// How many ranks the edge should try to span (mermaid's link length; dagre's `minlen`).
    pub minlen: usize,
    /// Text drawn beside the line at the `from` end — a class diagram's cardinality.
    pub start_label: Option<Label>,
    /// Text drawn beside the line at the `to` end.
    pub end_label: Option<Label>,
    /// The resolved `class`/`linkStyle` paint, carried through to [`PlacedEdge::style`] unchanged.
    pub style: Option<ShapeStyle>,
    /// Carried through to [`PlacedEdge::curve`] unchanged — see that field's docs for why every
    /// spec builder but the flowchart's own sets this to [`Curve::Basis`].
    pub curve: Curve,
}

/// One frame to lay out: a flowchart's `subgraph`, or a state diagram's composite state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpecBlock {
    /// The block's id, which an edge may name as an endpoint.
    pub id: String,
    /// The text on the frame. Empty for a block with no title.
    pub title: String,
    /// Direct members by id, in declaration order — nodes and nested blocks alike.
    pub members: Vec<String>,
    /// Whether the frame is drawn dashed.
    pub dashed: bool,
}

/// Everything the layout needs, and nothing about which diagram language it came from.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct GraphSpec {
    /// The axis the ranks run along.
    pub direction: Direction,
    /// Nodes, in the order they should be drawn.
    pub nodes: Vec<SpecNode>,
    /// Edges, in the order they should be drawn.
    pub edges: Vec<SpecEdge>,
    /// Frames, innermost first (a nested block before the one that contains it).
    pub blocks: Vec<SpecBlock>,
    /// `[ui] mermaid_routing`, resolved. `Routing::Splines` — most diagram kinds' `spec_of` leaves
    /// this at its `#[default]` unconditionally — reproduces every edge exactly as
    /// [`lay_out_spec`] always routed it; only the flowchart's and (since §10-5) the state
    /// diagram's own `spec_of` ever set this to `Routing::Orthogonal`, and only when
    /// `[ui] mermaid_routing = "konoma-orthogonal"`.
    pub routing: Routing,
    /// §10-5 S3: whether a self-transition (`source.id == target.id`) draws as the state
    /// diagram's own fixed 20px loop rather than a flowchart's dagre-derived staircase. `false`
    /// for every diagram kind but a state diagram (`state::spec_of`'s own caller), and `false`
    /// there too unless `routing == Routing::Orthogonal` — a flowchart's self-loop (`A --> A` is
    /// valid mermaid flowchart syntax) keeps its pre-existing, separately-tested shape either way.
    pub fixed_self_loops: bool,
}

/// Lays out and routes a spec — everything except turning geometry into markup.
///
/// # `direction` inside a block is read and deliberately not applied
///
/// Both parsers keep it and neither this function nor its callers act on it; the whole diagram is
/// laid out with one `rankdir`. That is a choice, so here is the reasoning.
///
/// dagre has exactly one `rankdir` per layout. Honouring a per-block direction therefore means
/// laying that block out in a **graph of its own** and inserting the result into the parent as an
/// opaque box — which is what mermaid does, and it is why mermaid can only do it for a block that
/// has *no edge crossing its border*: a member laid out in a separate coordinate space cannot be
/// ranked against a node outside it. Three things follow, and together they decide it:
///
/// 1. **It would work only sometimes, silently.** Upstream's own documentation says "if any of a
///    subgraph's nodes are linked to the outside, subgraph direction will be ignored" — and
///    linking to the outside is the ordinary case. A statement that does nothing, with no way to
///    tell that it did nothing, reads as a bug rather than as a limitation.
/// 2. **Upstream tried the other way and took it back.** 11.16.0 made dagre respect it; 11.17.0
///    reverted because the arrows *between* subgraphs broke. Those arrows are the part of a
///    diagram that carries the meaning, so trading them for an internal axis is the wrong trade
///    under §0-1 ("the same source read correctly").
/// 3. **One layout is one coordinate space, and that is what the tests can check.** Every cluster
///    invariant this stage adds — a frame holds its members, a nested frame is inside its parent,
///    a line does not run through a frame it has nothing to do with — is a statement about one
///    geometry. A second, independent layout inside a box would be geometry nothing checks;
///    upstream's own code says as much, in the note on `applyDagreLayoutResult` that consumers
///    "must not assume it is a complete description of the rendered output for cluster diagrams".
///
/// What the reader loses is the internal axis of a block. What they keep is every node, every
/// edge, and every frame the source declared, which is the degradation §0-1 asks for. If a later
/// stage wants the axis back, the place to start is a nested layout — and
/// [`super::tests::a_block_direction_does_not_move_anything`] is the test it has to change.
pub fn lay_out_spec(spec: &GraphSpec) -> Result<Diagram, RenderError> {
    // The gate. usvg does not fail on a missing font, it just drops the glyphs, so the only place
    // this can be caught is before anything is built (PRD design principle #3).
    if !text_metrics::fonts_available() {
        return Err(RenderError::NoFonts);
    }
    if spec.nodes.is_empty() {
        return Err(RenderError::NothingToDraw);
    }

    // Stage 2's port eviction (`docs/FEATURE-MERMAID-RENDERER.md` §10-1 item 1, "退避則") can only
    // see how many edges land on each node's face once dagre has actually run a layout, and the
    // fix for a face too narrow to fit them — growing the node — changes what dagre lays out from.
    // So a flowchart under `[ui] mermaid_routing = "konoma-orthogonal"` may run the whole "lay out,
    // then route" pass more than once. Every other spec (`Routing::Splines`, which is every kind
    // but a flowchart's own) always takes exactly one pass: `sizes` starts — and stays — empty, so
    // `lay_out_spec_pass`'s `sizes.get(id).unwrap_or(node.size)` reads the exact value it always
    // read, and `required` comes back empty too, so `apply_growth` never has anything to add.
    //
    // Growth only ever grows (`apply_growth` takes the max of the current and required size), so
    // this is monotonic and would converge on its own; `MAX_GROWTH_PASSES` is a defensive cap, not
    // the expected path for *ordinary* port-eviction growth, which never changes what it is
    // measuring as a side effect of the growth itself.
    //
    // §10-5 S4's own bar-length growth (`bar_required_sizes`) is not quite that shape: widening a
    // fork/join bar to fit its connected trunks' span can itself push those very trunks further
    // apart (dagre's own `nodesep` needs more room for a wider same-rank sibling), which *raises*
    // the next pass's own required span — a real, geometrically-narrowing feedback loop (found on
    // `zz-design-4c`: `fork_state` needed 32 → 113 → 145 → 161 → 169…px, each shortfall roughly
    // half the last), not a disagreement between passes, but one ordinary 16px-grid port growth
    // never exhibits (growing one node's face to fit its own ports does not move a *different*
    // node's centre). Raising `MAX_GROWTH_PASSES` high enough to fully settle this (measured:
    // ~14 passes) was tried and reverted — it changed *unrelated* flowcharts' own convergence path
    // enough to reopen an old bug this same loop already fixed once
    // (`orthogonal_decision_retry_loop_does_not_span_the_whole_ring`'s own back-edge ring-spanning
    // regression, at pass counts this constant had never run before), which is worse than leaving
    // a bar's own length one `BAR_PORT_PAD` (16px) short of its true asymptotic span. So `zz-
    // design-4c`'s own join bar stays visibly slightly under its final trunk span — a known,
    // accepted gap (`docs/STATUS.md`'s own ★未修正), not a silently wrong number: every port on
    // it is still evicted correctly (`bar_ports`), and the shortfall shrinks fast enough (halving
    // each pass) that it is a few px, not the kind of miss that puts a port outside the bar
    // altogether.
    //
    // The same loop also carries §10-1 item 3's "ラベル付き区間の最低長" fix: `label_boosts` is the
    // extra px `lay_out_spec_pass` asks be added to a labelled edge's flow-axis `EdgeLabel`
    // dimension (§10-1's own recommendation — "dagre に渡す辺ラベル寸法…の引き上げ" — chosen because
    // dagre already reserves rank space sized to exactly that field, verified against a real
    // layout dump before this was written rather than assumed: a `TD` edge's rank gap grows by
    // precisely its label's raw height, an `LR` edge's by precisely its raw width, in every case
    // checked). It is a second, independent growth signal over the same retry shape as node sizes
    // — a pass can ask for both at once — so it rides the identical loop rather than a second one.
    const MAX_GROWTH_PASSES: usize = 3;
    let mut sizes: HashMap<String, Size> = HashMap::new();
    let mut label_boosts: HashMap<String, f64> = HashMap::new();
    let mut diagram: Option<Diagram> = None;
    for _ in 0..MAX_GROWTH_PASSES {
        let (this_diagram, required, label_shortfall) =
            lay_out_spec_pass(spec, &sizes, &label_boosts)?;
        let grew_nodes = apply_growth(spec, &mut sizes, &required);
        let grew_labels = apply_label_growth(&mut label_boosts, &label_shortfall);
        diagram = Some(this_diagram);
        if !grew_nodes && !grew_labels {
            break;
        }
    }
    Ok(diagram.expect("the loop body runs at least once: MAX_GROWTH_PASSES > 0"))
}

/// Grows `sizes[id]` to `max(current, required)` for every node `required` names — `current`
/// falling back to the node's original `spec` size when this is the first pass to touch it.
/// Returns whether anything actually grew, which is `lay_out_spec`'s retry-loop stopping
/// condition. Never shrinks: `required` states a minimum, and a size already at or past it from
/// an earlier pass is left alone (the epsilon guards against a pass looping forever over
/// floating-point noise that never actually changes the number dagre lays out from).
fn apply_growth(
    spec: &GraphSpec,
    sizes: &mut HashMap<String, Size>,
    required: &HashMap<String, Size>,
) -> bool {
    let mut grew = false;
    for node in &spec.nodes {
        let Some(need) = required.get(&node.id) else {
            continue;
        };
        let cur = sizes.get(&node.id).copied().unwrap_or(node.size);
        let grown = Size::new(cur.w.max(need.w), cur.h.max(need.h));
        if grown.w > cur.w + 1e-6 || grown.h > cur.h + 1e-6 {
            sizes.insert(node.id.clone(), grown);
            grew = true;
        }
    }
    grew
}

/// §10-5 S4 ("長さ＝接続先トランクspan＋両端各16px"): every fork/join bar's own minimum size, read
/// straight from `nodes`' current positions — a bar's length is the cross-axis span of every trunk
/// node an edge connects it to (its own union of upstream *and* downstream neighbours: a fork's
/// single input plus its many outputs, or a join's many inputs plus its single output, both read
/// the same way, since which one is "the many side" never matters to a span), plus
/// [`orthogonal::BAR_PORT_PAD`] on each end; thickness is always whatever `state::spec_of` already
/// set it to (this function does not read that constant directly — the node's own already-placed
/// `size` carries it, and this only ever asks for more length along the bar's own long axis, never
/// a different thickness).
///
/// A node with no edge touching it at all (unreachable from any real diagram — a fork/join always
/// has at least one transition, or the parser would not have made it a `Kind::Fork`/`Kind::Join`
/// in the first place) is simply absent from the result, the same "no entry means no requirement"
/// convention [`orthogonal::RoutedFlowchart::required_size`] already uses.
fn bar_required_sizes(
    direction: Direction,
    nodes: &[PlacedNode],
    by_id: &HashMap<String, usize>,
    drawable: &[Drawable],
) -> HashMap<String, Size> {
    let cross_coord = |p: &Point| match direction {
        Direction::TopToBottom | Direction::BottomToTop => p.x,
        Direction::LeftToRight | Direction::RightToLeft => p.y,
    };
    let is_bar = |id: &str| {
        by_id
            .get(id)
            .is_some_and(|&i| matches!(nodes[i].shape, Glyph::Bar { .. }))
    };
    let mut spans: HashMap<String, (f64, f64)> = HashMap::new();
    for d in drawable {
        if d.tail == d.head {
            continue;
        }
        if is_bar(&d.tail) {
            if let Some(&hi) = by_id.get(&d.head) {
                let c = cross_coord(&nodes[hi].center);
                let e = spans
                    .entry(d.tail.clone())
                    .or_insert((f64::INFINITY, f64::NEG_INFINITY));
                e.0 = e.0.min(c);
                e.1 = e.1.max(c);
            }
        }
        if is_bar(&d.head) {
            if let Some(&ti) = by_id.get(&d.tail) {
                let c = cross_coord(&nodes[ti].center);
                let e = spans
                    .entry(d.head.clone())
                    .or_insert((f64::INFINITY, f64::NEG_INFINITY));
                e.0 = e.0.min(c);
                e.1 = e.1.max(c);
            }
        }
    }
    let mut out = HashMap::new();
    for (id, (min_c, max_c)) in spans {
        let Some(&i) = by_id.get(&id) else { continue };
        let length = (max_c - min_c).max(0.0) + 2.0 * orthogonal::BAR_PORT_PAD;
        let horizontal = matches!(nodes[i].shape, Glyph::Bar { horizontal: true });
        let thickness = if horizontal {
            nodes[i].size.h
        } else {
            nodes[i].size.w
        };
        let size = if horizontal {
            Size::new(length, thickness)
        } else {
            Size::new(thickness, length)
        };
        out.insert(id, size);
    }
    out
}

/// Adds each edge's `shortfall` — this pass's own measured gap between what its labelled segment
/// needed and what it actually got, `orthogonal::label_min_length(...) - slot.length` — onto its
/// running `boosts` total. Unlike [`apply_growth`]'s `max` (a size is a standing requirement that
/// does not change shape from one pass to the next), this **adds**: `shortfall` is a *delta* for
/// this pass alone, because the previous pass's boost is already baked into the segment length
/// `lay_out_spec_pass` just measured, so what remains unmet has to be added on top of what was
/// already asked for, not compared against it. `shortfall` only ever holds positive entries
/// (`lay_out_spec_pass` never records a segment that already met its minimum), so this is
/// monotonic the same way `apply_growth` is, and converges for the same reason: growing a labelled
/// edge's flow-axis `EdgeLabel` dimension can only ever grow the rank gap it sits in, never shrink
/// it, so each pass's shortfall can only shrink toward zero.
fn apply_label_growth(boosts: &mut HashMap<String, f64>, shortfall: &HashMap<String, f64>) -> bool {
    let mut grew = false;
    for (id, extra) in shortfall {
        if *extra > 1e-6 {
            *boosts.entry(id.clone()).or_insert(0.0) += *extra;
            grew = true;
        }
    }
    grew
}

/// [`lay_out_spec_pass`]'s own result — named only to keep clippy's `type_complexity` lint quiet;
/// see that function's own doc for what each element means.
type LayoutPassResult = (Diagram, HashMap<String, Size>, HashMap<String, f64>);

/// Whether an edge is an **aside** — §10-1 item 4's "補助辺", a link the reader should be able to
/// find but which is not part of the flow the picture is about.
///
/// §10-1 item 4 ("戻り辺・補助辺は破線で外周レーンを回す") says a dashed line is how the design marks
/// one, and the design answers by taking it round the outside rather than through the middle.
/// konoma never dashes an edge itself (§10-1's own note: the line style is the author's), so the
/// author's own `-.->` is the whole signal.
///
/// Three separate decisions read this one predicate, so that "what counts as an aside" is stated
/// once: [`aside_weight`] (it gets no vote on where the flow's nodes go), the vendored engine's
/// [`EdgeLabel::rank_only`] (it constrains ranks and nothing else — [`lay_out_spec_pass`]'s own
/// call site), and [`orthogonal::classify`] (it is drawn on the outer perimeter lane, forward or
/// reverse).
///
/// [`Routing::Splines`] is deliberately excluded from all three: it routes a dotted edge exactly
/// like any other and its output is pinned byte for byte (§10-2), so the default rendering of
/// every diagram kind is untouched.
fn is_aside(routing: Routing, stroke: Stroke) -> bool {
    routing == Routing::Orthogonal && stroke == Stroke::Dotted
}

/// The dagre `weight` an edge is handed with — how hard its two ends pull towards each other while
/// dagre ranks, orders and positions the graph. Every edge has carried the default `1` since the
/// vendored engine landed; an [`is_aside`] edge is the one exception.
///
/// This function is about the *layout* half of §10-1 item 4, not the routing half:
/// [`orthogonal::classify`] takes every aside — forward or reverse — round the perimeter ring, so
/// where an aside is drawn is decided after the layout, and the layout has no reason to let it
/// decide where the nodes go.
///
/// dagre, though, saw an ordinary edge and pulled just as hard on it. Measured on `zz-design-2b`,
/// whose single `CLI -.->|リンク| PAY` is the only dotted edge in the diagram — raw dagre
/// cross-axis positions, the same graph with and without that one line:
///
/// | node  | with the aside | without it |
/// |-------|----------------|------------|
/// | `EX`  | 102.7          | 241.5      |
/// | `UI`  | 172.1          | 172.1      |
/// | `CLI` | **413.0**      | **102.7**  |
///
/// Without it the three clients sit one node pitch (69.4px) apart in declaration order. With it
/// `CLI` is dragged 240.9px — three and a half pitches — down to `決済ページ`'s own row at the far
/// side of the diagram, out of the stack it belongs to, and its real edge into `API ゲート` then has
/// to climb all the way back up and cross `ブラウザ UI`'s edge on the way. The tangle is the pull,
/// not the routing.
///
/// Weight `0` states in dagre's own terms what the design already states about the edge: it is
/// still drawn, and it still spans its ranks (`minlen` is untouched, so nothing about how far
/// apart the two ends are allowed to be changes), but it gets no vote on *where* its endpoints go.
/// The vendored engine takes a zero cleanly at each of the three phases that read a weight —
/// `rank::network_simplex` sums it into a cut value, where it contributes nothing;
/// `order::barycenter`, `order::sort` and `order::resolve_conflicts::merge_entries` all divide by
/// a weight sum and all three already guard that divisor with `weight > 0`, so a node whose only
/// in-edge is an aside comes back with no barycenter at all and `sort` leaves it at its original
/// index rather than producing a NaN; `position::bk` never reads an edge label's weight (its own
/// `weight` is a separation distance). `nesting_graph` sums every edge weight to size the frame-
/// compaction edges it injects, which is one smaller per aside and still, as upstream intends,
/// larger than any real edge's.
///
/// Weight `0` and [`EdgeLabel::rank_only`] are two halves of one rule, not alternatives: the
/// weight is what the *rank* phase reads (the edge holds its `minlen` and asks for nothing else),
/// and `rank_only` is what takes the edge out of `order`/`position` entirely, so it cannot spread
/// a rank by way of the dummy chain `normalize` would otherwise build for it.
fn aside_weight(routing: Routing, stroke: Stroke) -> i32 {
    if is_aside(routing, stroke) {
        0
    } else {
        EdgeLabel::default().weight
    }
}

/// One layout-and-route pass: builds the dagre graph at `sizes` (falling back to each
/// [`SpecNode`]'s own `size` for any node `sizes` does not name — every node, on the first pass),
/// lays it out, and routes every edge. Returns the diagram, the minimum size stage 2's port
/// eviction says each node needs, and — §10-1 item 3 — how much further each labelled edge's
/// segment still falls short of its own minimum length; all three maps are empty for
/// `Routing::Splines`, which is what keeps that path byte-identical to before this function existed
/// (see [`lay_out_spec`]'s own doc).
///
/// `label_boosts` is what the *previous* pass asked for (via the second map this function
/// returns) — added to a labelled edge's flow-axis `EdgeLabel` dimension below, before dagre ever
/// sees the graph, so a retried pass starts from a wider rank gap rather than measuring the same
/// shortfall twice.
fn lay_out_spec_pass(
    spec: &GraphSpec,
    sizes: &HashMap<String, Size>,
    label_boosts: &HashMap<String, f64>,
) -> Result<LayoutPassResult, RenderError> {
    // The block tree, resolved before anything reaches dagre: it decides which nodes get a
    // parent, and a block that holds no node at all never reaches it (`clusters::Tree::build`).
    let node_ids: HashSet<&str> = spec.nodes.iter().map(|n| n.id.as_str()).collect();
    let tree = clusters::Tree::from_blocks(&spec.blocks, |id| node_ids.contains(id));

    let mut g: Graph<NodeLabel, EdgeLabel> = Graph::with_options(GraphOptions {
        directed: true,
        multigraph: true,
        // Only when there is something to nest. dagre runs its nesting graph either way, but a
        // compound graph reaches `parent_dummy_chains` and `add_border_segments`, and a chart with
        // no blocks has no reason to pay for a code path it cannot use.
        compound: !tree.is_empty(),
    });

    // --- nodes: the caller already measured the label and sized the shape around it -------------
    // (or, on a growth retry, `sizes` overrides it for a node stage 2's eviction found too small).
    let mut measured: HashMap<&str, &SpecNode> = HashMap::new();
    for node in &spec.nodes {
        let size = sizes.get(&node.id).copied().unwrap_or(node.size);
        g.set_node(
            node.id.clone(),
            Some(NodeLabel {
                width: size.w,
                height: size.h,
                ..NodeLabel::default()
            }),
        );
        measured.insert(node.id.as_str(), node);
    }

    // --- blocks: a node in dagre with no size of its own ----------------------------------------
    //
    // dagre's compound support does the whole job: `set_parent` puts a node under a block, the
    // nesting graph gives each block a rank of its own above and below its members, border
    // segments pin its sides, and `removeBorderNodes` hands back a rectangle in `x`/`y`/`width`/
    // `height`. Nothing here computes a frame — a frame is *read*, after the layout, from the
    // parent node dagre filled in. The `width`/`height` set here are ignored for exactly that
    // reason: dagre overwrites them.
    //
    // Every block is registered before any parent is set, because `set_parent` would otherwise
    // create the missing one implicitly and leave it without a label.
    for c in tree.iter() {
        g.set_node(c.id.clone(), Some(NodeLabel::default()));
    }
    for c in tree.iter() {
        if let Some(parent) = &c.parent {
            g.set_parent(&c.id, Some(parent.as_str()));
        }
        for m in &c.member_nodes {
            if g.has_node(m) {
                g.set_parent(m, Some(c.id.as_str()));
            }
        }
    }

    // --- edges: hand dagre the label's size so it reserves a rank for it ------------------------
    //
    // This is the half of "the label stays on the line" that happens *before* layout: dagre's
    // `makeSpaceForEdgeLabels` doubles every `minlen` and injects a proxy node for any edge whose
    // label has a non-zero width and height, so the label gets a rank of its own to live in.
    //
    // An endpoint that names a *block* is re-anchored onto a member first. dagre cannot route to a
    // compound parent — the ranking phase only ever sees leaves — so mermaid lays such an edge out
    // against a representative descendant and cuts the line back to the frame when it draws it.
    // The anchor is remembered here because it is also what the layout has to be read back from.
    //
    // `internal_edges` is every transition's own written `(from, to)` pair, over the whole
    // diagram — `clusters::Tree::anchor`'s own doc: it restricts this to whichever pairs turn out
    // to be *both* descendants of the one block being anchored, so handing it the full,
    // unfiltered list here (built once, not per edge) is exactly what "an edge to/from outside
    // the block says nothing about its own internal flow" needs — an edge whose ends are not both
    // inside a given block is simply never counted for that block's own sink/source search.
    let internal_edges: Vec<(String, String)> = spec
        .edges
        .iter()
        .map(|e| (e.from.clone(), e.to.clone()))
        .collect();
    // §10-5's own acceptance criterion ("既定 splines の状態図は1バイト不変") and the same
    // requirement for every other diagram kind's own default rendering: the directional sink/
    // source anchor only ever applies under `konoma-orthogonal`. `Routing::Splines` keeps
    // `AnchorRole::Declared` — the pre-§10-5 "first descendant, direction-blind" rule — so no
    // splines-routed cluster-anchored edge changes which member it lays out against.
    let (exit_role, entry_role) = if spec.routing == Routing::Orthogonal {
        (clusters::AnchorRole::Exit, clusters::AnchorRole::Entry)
    } else {
        (
            clusters::AnchorRole::Declared,
            clusters::AnchorRole::Declared,
        )
    };
    let mut drawable: Vec<Drawable> = Vec::new();
    let mut edge_label_dims: HashMap<String, (f64, f64)> = HashMap::new();
    for edge in &spec.edges {
        let is_node = |id: &str| measured.contains_key(id);
        let (Some(tail), Some(head)) = (
            tree.anchor(&edge.from, &is_node, exit_role, &internal_edges),
            tree.anchor(&edge.to, &is_node, entry_role, &internal_edges),
        ) else {
            // Neither a node nor a block that holds one: there is nothing to draw a line between.
            continue;
        };
        let (tail, head) = (tail.to_string(), head.to_string());
        let (mut w, mut h) = edge
            .label
            .as_ref()
            .map_or((0.0, 0.0), |l| (l.width, l.height));
        // §10-1 item 3's minimum-length fix: `label_boosts` is the previous pass's own measured
        // shortfall (see this function's own doc), added to whichever of `w`/`h` is this diagram's
        // flow-axis dimension — the one dagre's rank assignment actually grows the gap from (a
        // `TD`/`BT` edge's rank gap tracks its label's raw *height*; `LR`/`RL` tracks *width* —
        // verified against a real layout dump, not assumed, before this was written). Adding to
        // the other dimension would ask dagre for room in an axis it does not use to size a rank
        // gap at all, so it would inflate nothing this edge's segment actually needs.
        if let Some(&boost) = label_boosts.get(&edge.id) {
            if matches!(
                spec.direction,
                Direction::TopToBottom | Direction::BottomToTop
            ) {
                h += boost;
            } else {
                w += boost;
            }
        }
        g.set_edge(
            tail.clone(),
            head.clone(),
            Some(EdgeLabel {
                width: w,
                height: h,
                minlen: edge.minlen.max(1) as i32,
                weight: aside_weight(spec.routing, edge.stroke),
                // §10-1 item 4's layout half, second and larger part (`EdgeLabel::rank_only`'s own
                // doc in the vendored engine): an aside constrains ranks and nothing else. Without
                // this it still spanned its ranks as a dummy chain, which `parent_dummy_chains`
                // parents into the source's own subgraph and which then has to sit outside every
                // wider sibling frame — measured on `zz-design-2b`, that one chain stretched
                // `クライアント` far enough for `position::bk` to spread its three members to
                // 114.5/190.2px apart where one node pitch is 69.4px.
                rank_only: is_aside(spec.routing, edge.stroke),
                ..EdgeLabel::default()
            }),
            Some(edge.id.as_str()),
        );
        // §10-3 item 8's own `pull_back_fan_ranks` reads this back to size a rank gap for a
        // labelled edge — captured *here*, the exact `(w, h)` just handed to dagre (raw label
        // size plus any `label_boosts`), rather than read back off `g` after `layout()` runs:
        // `normalize::run`'s own denormalize step overwrites `EdgeLabel.width`/`.height` with the
        // dummy label-proxy *node*'s own box size once the dummy chain collapses back into one
        // edge, which is not the same number (confirmed by dumping a plain, label-less edge and
        // finding a non-zero width there — dagre's own internal bookkeeping, not a label at all).
        edge_label_dims.insert(edge.id.clone(), (w, h));
        drawable.push(Drawable { edge, tail, head });
    }

    layout(
        &mut g,
        Some(LayoutOptions {
            rankdir: rank_dir(spec.direction),
            nodesep: if spec.routing == Routing::Orthogonal {
                ORTHO_NODE_SEP
            } else {
                NODE_SEP
            },
            edgesep: EDGE_SEP,
            ranksep: RANK_SEP,
            marginx: MARGIN,
            marginy: MARGIN,
            // mermaid draws with `dagre-d3-es`, a port of dagre.js 0.8.5, which keeps the *first*
            // of two layerings that tie on crossings. The vendored engine follows 3.0.1-pre by
            // default, which keeps the last.
            tie_keep_first: true,
            ..LayoutOptions::default()
        }),
    );

    // §10-3 item 8 ("ファン先は同一ランクに整列する", `docs/FEATURE-MERMAID-RENDERER.md`) —
    // `Routing::Orthogonal` only. See that function's doc for the full reasoning; in short, it throws
    // dagre's own rank numbers away and recomputes every node's rank and flow-axis position from
    // scratch by the classic "as-soon-as-possible" layering, which is immune to the "slack node lands
    // wherever network simplex's pivoting happened to leave it" problem network simplex has no way
    // around. §10-5 round 4 removed the `tree.is_empty()` gate this used to carry: `tree` is now
    // passed in, every block is re-ranked as one atomic unit, and the frames themselves are re-derived
    // from wherever the members end up (`rebuild_frames`) rather than read from dagre's own border
    // nodes — which is exactly what the gate existed to avoid going stale.
    if spec.routing == Routing::Orthogonal {
        pull_back_fan_ranks(
            &mut g,
            spec.direction,
            &drawable,
            sizes,
            &measured,
            &edge_label_dims,
            &tree,
        );
    }

    // --- read the layout back -------------------------------------------------------------------
    let mut nodes: Vec<PlacedNode> = Vec::with_capacity(spec.nodes.len());
    for node in &spec.nodes {
        let placed = g.node(&node.id);
        let center = Point::new(
            placed.and_then(|n| n.x).unwrap_or(0.0),
            placed.and_then(|n| n.y).unwrap_or(0.0),
        );
        let size = sizes.get(&node.id).copied().unwrap_or(node.size);
        nodes.push(PlacedNode {
            id: node.id.clone(),
            shape: node.glyph,
            center,
            size,
            label: node.label.clone(),
            panel: node.panel.clone(),
            series: None,
            mark: None,
            style: node.style.clone(),
        });
    }
    if nodes.is_empty() {
        return Err(RenderError::NothingToDraw);
    }

    // Stage 3's lane alignment (§10-1 item 2, "レーン揃え") runs here — after dagre has placed
    // every node and before anything downstream reads a position from one, most importantly
    // `read_clusters` just below: a frame has to enclose wherever its members actually end up, so
    // moving a member after its frame was already computed would leave the frame stale. Only a
    // genuine node-to-node edge is a lane candidate — one whose written endpoint dagre did not
    // have to re-anchor onto a block member (`d.edge.from == d.tail`) — the same "cluster boundary
    // is out of scope" rule stage 1 already applies to routing applies here to alignment too.
    //
    // `alignment_deltas` is every node's own cross-axis delta from where dagre put it. A self-loop's
    // raw dagre waypoints (read from `g` below, `raw`'s own comment) are the one thing these passes
    // cannot correct themselves — they live in `g`, which none of them touches — so whichever
    // self-loop's owner appears in this map gets the same shift applied to `raw` before
    // `route_staircase_with_ports` ever sees it.
    //
    // Measured **here**, against `pre_pass_cross` below, over every cross-axis pass at once, rather
    // than taken from `align_straight_lanes`'s own return: there are three of them (the probe
    // alignment, `regroup_fan_lanes`, and the final alignment), each measures only its own step, and
    // the last one's own baseline is whatever the two before it left behind. Taking the last return
    // therefore silently drops the first two whenever `regroup_fan_lanes` moves anything — a
    // self-loop whose owner a fan regroup moved kept drawing its bump at the owner's pre-regroup
    // position, which `self_loop_routes_correctly_across_lane_alignment_stress_cases` catches on
    // `dense-diamond-lattice` the moment a regroup reaches that rank.
    let mut alignment_deltas: HashMap<String, f64> = HashMap::new();
    let pre_pass_cross: Vec<f64> = nodes.iter().map(|n| cross_of(spec.direction, n)).collect();
    // §10-3 item 1's own robustness note (`orthogonal::align_straight_lanes`'s own doc on its
    // `next` return): every selected trunk/chain edge (source id → target id), even one the later
    // overlap-resolution sweep pushed off that chain's own average — `orthogonal::classify`'s
    // fan-lane trigger and `orthogonal::evict`'s centre-port rule both need this alongside the
    // purely geometric check, or a trunk edge crowded out of exact alignment reads as having no
    // trunk at all.
    let mut chain_next: HashMap<String, String> = HashMap::new();
    // Hoisted out of the `if` below (unlike every other local that block computes) because the
    // pass-through-reservation retry loop, much further down at `route_flowchart`'s own call site,
    // needs it again — `reserve_pass_through_rows`'s own doc on why this has to be the *real*,
    // per-edge-`minlen` numbering (`pull_back_fan_ranks`'s own recomputation), never dagre's raw
    // one, which is exactly what this map already is whenever `tree.is_empty()`.
    let mut node_rank: HashMap<String, i32> = HashMap::new();
    // §10-5 round 4's own block-as-a-body model, built once for both cross-axis passes below
    // (`align_straight_lanes` and `regroup_fan_lanes`) — empty, and therefore a no-op, for a diagram
    // with no frames at all.
    let lane_units = orthogonal::LaneUnits::build(&tree, &nodes);
    if spec.routing == Routing::Orthogonal {
        node_rank = nodes
            .iter()
            .filter_map(|n| {
                g.node(&n.id)
                    .and_then(|nl| nl.rank)
                    .map(|r| (n.id.clone(), r))
            })
            .collect();
        // §10-5 round 4: a cluster-anchored edge (one whose written endpoint names a block, so
        // `d.tail`/`d.head` is the block's own anchor member rather than the endpoint the author
        // wrote) **is** a lane candidate now. It used to be filtered out — `d.edge.from == d.tail &&
        // d.edge.to == d.head` — which is why `zz-design-4c`'s own `取得 --> 処理` could never be a
        // straight trunk: alignment simply never saw it. What makes it sound to let it through is
        // `lane_units`: the anchor stands for its whole block, and a lane that picks it moves every
        // descendant by the same delta (`orthogonal::LaneUnits`'s own doc). Self-loops stay out —
        // a `(id, id)` pair would be selected as its own lane and then never appear as a chain head.
        let candidates: Vec<(String, String)> = drawable
            .iter()
            .filter(|d| d.tail != d.head)
            .map(|d| (d.tail.clone(), d.head.clone()))
            .collect();
        // §10-3's "ファン列内の並び順" (`docs/FEATURE-MERMAID-RENDERER.md`, `regroup_fan_lanes`'s own
        // doc): a first `align_straight_lanes` call, on whatever cross-axis order dagre/`pull_back_
        // fan_ranks` left behind, exists here only to *learn* `chain_next` — which target each
        // branching source's trunk edge selected. That selection depends on rank membership and
        // each candidate's own "does it keep going" / sibling-median tie-break, not on which
        // physical slot a sibling currently sits in, so computing it before `regroup_fan_lanes`
        // touches the rank order is safe. This first call's own alignment/overlap-resolution work
        // on `nodes` is intentionally thrown away — the second call below redoes it — because that
        // very mechanism is what `regroup_fan_lanes` has to run *ahead of*: its "ランク内の並び順は
        // 変えない" overlap sweep can only ever push a crowded chain member later, never reorder
        // around it, so the trunk keeps landing off its own shared centreline for as long as nine
        // other same-rank siblings keep sorting ahead of it. Moving the trunk to its new, colour-
        // grouped slot is what finally gives that sweep room to grant it the centreline. Its own
        // return is *not* always thrown away — see `touched` below: when `regroup_fan_lanes` finds
        // nothing to reorder, this first call's own results are the final ones.
        let (_probe_deltas, chain_next_probe) = orthogonal::align_straight_lanes(
            spec.direction,
            &mut nodes,
            &node_rank,
            &candidates,
            &lane_units,
        );
        let has_class: HashMap<String, bool> = spec
            .nodes
            .iter()
            .map(|n| (n.id.clone(), n.has_class))
            .collect();
        let touched = regroup_fan_lanes(
            spec.direction,
            &mut nodes,
            &node_rank,
            &candidates,
            &chain_next_probe,
            &has_class,
            &lane_units,
        );
        if touched {
            // Only pay for a second `align_straight_lanes` pass when `regroup_fan_lanes` actually
            // moved something: running the alignment/overlap-resolution machinery twice in a row is
            // *not* a no-op even on an unchanged rank order (found on the `branch` corpus fixture —
            // `orthogonal_no_edge_crosses_its_own_endpoint_across_the_whole_corpus` caught a second
            // pass re-opening the exact `D -> B` perimeter-ring collision `docs/STATUS.md`'s own
            // ★未修正 history already spent a careful, narrowly-scoped fix closing). Every diagram
            // whose fan-outs are all too small to need colour-grouping (the overwhelming majority of
            // this module's own corpus) takes the `else` branch and reproduces the pre-existing
            // single-call behaviour byte for byte — `chain_next` is exactly the first call's own
            // return, untouched by a second pass that never had anything to redo.
            //
            // The second call reuses the *first* call's own selection (`Some(&chain_next_probe)`)
            // rather than re-deriving it: `align_straight_lanes_with`'s own doc explains why a fresh
            // selection over the just-regrouped order is not safe to trust (a trunk placed exactly
            // at its fan's centre index is, by construction, tied with its nearest same-coloured
            // neighbour on every tie-break the selection loop has, so it can reselect a different
            // trunk than `regroup_fan_lanes` just built room for). Only the alignment/overlap-
            // resolution geometry needs to rerun on the new order; the selection itself does not
            // change under a pure cross-axis permutation.
            (_, chain_next) = orthogonal::align_straight_lanes_with(
                spec.direction,
                &mut nodes,
                &node_rank,
                &candidates,
                Some(&chain_next_probe),
                &lane_units,
            );
        } else {
            chain_next = chain_next_probe;
        }
        // §10-5 round 5's own tier rule (`orthogonal::place_dead_end_tiers`'s own doc). Runs here,
        // last of the passes that move a node and after every one that moves it *across* the flow:
        // a shelf can only be hung off a lane once that lane is final, and its own outward push
        // ("nothing on this side may overlap it") is only exact once every other unit has stopped
        // moving. `read_clusters`, immediately below, then derives the frame around wherever the
        // members landed, the same way it does for every other pass here.
        //
        // Its own edge list, not `candidates`: this pass needs a self-loop (which disqualifies its
        // owner from being a dead end) and needs to know which edges are asides, neither of which
        // the lane candidates carry.
        let tier_edges: Vec<(String, String, bool)> = drawable
            .iter()
            .map(|d| {
                (
                    d.tail.clone(),
                    d.head.clone(),
                    is_aside(spec.routing, d.edge.stroke),
                )
            })
            .collect();
        let tiers = orthogonal::place_dead_end_tiers(
            spec.direction,
            &mut nodes,
            &tree,
            &lane_units,
            &chain_next,
            &tier_edges,
        );
        for member in tiers.iter().flat_map(|t| t.members.iter()) {
            // A tier member is off the rank axis (`place_dead_end_tiers`'s own doc): it sits in its
            // source's own column, not in a column of its own, so a rank number would now say the
            // wrong thing about which side of it is downstream. Dropped from both readings of it —
            // `g` (what `EligibleEdge::source_rank`/`target_rank` are built from, so `classify`
            // falls back to geometry through `flow_rank_delta`) and `node_rank` (what
            // `reserve_pass_through_rows` counts rank gaps with).
            if let Some(n) = g.node_mut(member) {
                n.rank = None;
            }
            node_rank.remove(member);
            // ...and off the lane selection too: a lane that ended on this member would otherwise
            // still claim it as a trunk target (`evict`'s own centre-port rule) after this pass has
            // deliberately taken it off that lane.
            chain_next.retain(|_, t| t != member);
        }
        // Every cross-axis pass above is now behind us, so this is the total move from dagre's own
        // placement — `alignment_deltas`'s own doc above explains why it is taken here rather than
        // from whichever pass ran last.
        alignment_deltas = nodes
            .iter()
            .enumerate()
            .filter_map(|(i, n)| {
                let delta = cross_of(spec.direction, n) - pre_pass_cross[i];
                (delta.abs() > f64::EPSILON).then(|| (n.id.clone(), delta))
            })
            .collect();
        // §10-3 item 13's own "pass-through 行の予約" no longer runs here: deciding it from rank
        // topology alone, before any route exists, is exactly the `long-edge` regression
        // (`docs/STATUS.md`'s own ★未修正 entry — `A -> E`, a branching source whose own shape never
        // draws a flow-axis row at all, still got one reserved purely because its rank skipped two).
        // Moved to `route_flowchart`'s own call site below, where a trial route's own `EdgeShape`
        // decides which rank-skipping edges actually have a row worth reserving.
    }

    // Owned `String` keys, not borrowed `&str`: the pass-through-reservation retry loop below
    // (`route_flowchart`'s own call site) needs `&mut nodes` again, after this map's own last use —
    // a borrow of `nodes[i].id.as_str()` would keep that borrow alive across the whole function and
    // conflict with it.
    let by_id: HashMap<String, usize> = nodes
        .iter()
        .enumerate()
        .map(|(i, n)| (n.id.clone(), i))
        .collect();

    // §10-5 S4 ("長さ＝接続先トランクspan＋両端各16px"): every fork/join bar's minimum size, from
    // the *current* pass's own node positions — `nodes` here already carries whatever
    // `pull_back_fan_ranks`/`align_straight_lanes`/`regroup_fan_lanes` above settled on, the same
    // "current geometry" every other §10-1 item 1 growth signal (`route_flowchart`'s own
    // `RoutedFlowchart::required_size`) reads from. Folded into `required_size` alongside that one,
    // just below, so `apply_growth`'s existing monotonic "lay out, measure, grow, lay out again"
    // loop (`lay_out_spec`'s own doc) is the *only* growth mechanism this module has — a bar's
    // length is not a special case that needs its own retry loop, just another entry in the same
    // map.
    let bar_min_sizes: HashMap<String, Size> = if spec.routing == Routing::Orthogonal {
        bar_required_sizes(spec.direction, &nodes, &by_id, &drawable)
    } else {
        HashMap::new()
    };

    // --- read the frames back, then grow them until their titles fit ----------------------------
    //
    // Runs **after** every pass above that can still move a node (`pull_back_fan_ranks`,
    // `align_straight_lanes`, `regroup_fan_lanes`) and **before** every pass below that reads a
    // frame (`clear_foreign_cluster_overlaps`, `route_flowchart`'s own cluster ports and
    // `content_bounds`): that ordering is what makes a frame derived from its members correct rather
    // than stale, and it is the single place either fact is stated (`read_clusters`'s own doc).
    let mut placed_clusters = read_clusters(&g, &tree, &nodes, spec.routing);

    // §10-5 part-3 item 1: a node that is not a member of a cluster must never end up sitting
    // inside that cluster's frame — `orthogonal::clear_foreign_cluster_overlaps`'s own doc has the
    // real-diagram case (`zz-design-4a`'s end marker) and the dagre/nodesep reasoning. Orthogonal
    // only: splines never reaches this call, so its byte-stable output is untouched. Runs here,
    // after every pass above that can still move a node's cross coordinate and after `placed_
    // clusters` has read every frame's final rectangle back, and before `route_flowchart` (below)
    // reads either `nodes` or `placed_clusters` to place a single port.
    //
    // Whatever it pushes clear of one frame is usually a member of **another** (`zz-design-2b`'s
    // `解析サンドボックス` belongs to `クラウド` and overlapped its sibling `保存層`), so the frames are
    // derived again from the moved members — round 4's own rule that a frame follows its contents
    // rather than the other way round. Bounded rather than iterated to a fixpoint, the same
    // finite-retry shape `lay_out_spec`'s own growth loop uses: a re-derived frame is larger, which
    // can in principle swallow a node that was clear a moment ago, and two rounds settle every
    // corpus source there is (measured: the second round already reports nothing moved).
    if spec.routing == Routing::Orthogonal {
        for _ in 0..2 {
            if !orthogonal::clear_foreign_cluster_overlaps(
                spec.direction,
                &mut nodes,
                &placed_clusters,
                &tree,
            ) {
                break;
            }
            rebuild_frames(&mut placed_clusters, &tree, &nodes);
        }
    }
    let placed_clusters = placed_clusters;

    // `orthogonal::route_edge`'s branch/merge shapes read a node's out-degree and its edges'
    // target's in-degree (`docs/FEATURE-MERMAID-RENDERER.md` §10-1 item 1) — counted over exactly
    // the edges that are actually going to be drawn (`drawable`, already anchored past any block
    // endpoint), not `spec.edges`, so a `~~~` layout-only link or an edge naming a block with no
    // member node does not inflate a degree nothing will draw a port for.
    //
    // Keyed by `d.edge.from`/`d.edge.to` — the *written* endpoint — rather than `d.tail`/`d.head`
    // (the anchor `tree.anchor` resolved a block endpoint onto): for an ordinary node-to-node edge
    // the two are identical (a node's own anchor is itself), so this is byte-for-byte the same
    // count it always was; only a cluster-anchored edge differs, and there it is the *cluster's*
    // own degree — how many drawn edges name that subgraph — that `classify`'s branch/merge choice
    // needs, not the representative member's, which several unrelated cluster-endpoint edges could
    // all have been anchored onto and so would wrongly inflate. Owned `String` keys: `drawable` is
    // moved into the loop below, so a borrowed key could not outlive it.
    let mut out_degree: HashMap<String, usize> = HashMap::new();
    let mut in_degree: HashMap<String, usize> = HashMap::new();
    for d in &drawable {
        *out_degree.entry(d.edge.from.clone()).or_insert(0) += 1;
        *in_degree.entry(d.edge.to.clone()).or_insert(0) += 1;
    }

    // --- work out what each edge has to meet at each end, and fetch dagre's own waypoints -------
    //
    // Split from "build the final `PlacedEdge`" (below) because stage 2's port eviction
    // (`orthogonal::route_flowchart`) needs to see *every* node-to-node edge's shape before it can
    // give *any* of them an exact port — a face's occupancy is not known until every claim on it
    // has been gathered.
    struct PreparedEdge<'a> {
        edge: &'a SpecEdge,
        tail_id: String,
        head_id: String,
        raw: Vec<Point>,
        tail_end: edges::End,
        head_end: edges::End,
    }
    let mut prepared: Vec<PreparedEdge> = Vec::with_capacity(drawable.len());
    // By reference, not by value: the pass-through-reservation retry loop below (`route_flowchart`'s
    // own call site) needs `drawable` again, to decide which rank-skipping edge actually earned a
    // reserved row from the *first* trial route (`RoutedFlowchart::pass_through_eligible`'s own
    // doc) — this loop used to consume `drawable` outright, back when `reserve_pass_through_rows`
    // ran once, blindly, before any route existed at all.
    for Drawable {
        edge,
        tail: tail_id,
        head: head_id,
    } in &drawable
    {
        let edge = *edge;
        let tail_id = tail_id.clone();
        let head_id = head_id.clone();
        let mut raw = g
            .edge(&tail_id, &head_id, Some(edge.id.as_str()))
            .map(|l| l.points.clone())
            .unwrap_or_default();
        // A self-loop is the one shape `route_with_ports` still draws from `raw`
        // (`route_staircase_with_ports`'s own doc) — `raw` was captured from `g`, which
        // `align_straight_lanes` above never touches, so if the loop's own owner just moved,
        // `raw` is still sitting at its pre-alignment position. `alignment_deltas` names exactly
        // the nodes that actually moved; shifting every point here keeps the loop attached to
        // where its owner ended up, the same way `route_with_ports` already reads the owner's
        // *current* `PlacedNode` for the two ports.
        if tail_id == head_id {
            if let Some(&delta) = alignment_deltas.get(&tail_id) {
                for p in &mut raw {
                    *p = orthogonal::shift_cross(spec.direction, p, delta);
                }
            }
        }
        let (Some(&ti), Some(&hi)) = (by_id.get(tail_id.as_str()), by_id.get(head_id.as_str()))
        else {
            continue;
        };
        // What the line has to meet at each end: the node's own outline, or — when the author
        // wrote the block's name — the frame around it.
        let end_of = |written: &str, anchor: &PlacedNode| match placed_clusters
            .iter()
            .find(|c| c.id == written)
        {
            Some(c) => edges::End::Cluster(clusters::Rect::new(&c.center, c.size)),
            None => edges::End::Node(anchor.shape, anchor.center.clone(), anchor.size),
        };
        let tail_end = end_of(&edge.from, &nodes[ti]);
        let head_end = end_of(&edge.to, &nodes[hi]);
        prepared.push(PreparedEdge {
            edge,
            tail_id,
            head_id,
            raw,
            tail_end,
            head_end,
        });
    }

    // --- stage 2: one global port-eviction pass over every node-to-node AND cluster-anchored edge -
    //
    // Orthogonal routing is `docs/FEATURE-MERMAID-RENDERER.md` §10-1's stage 1 + 2, plus §10-2's
    // cluster-edge stage: every drawable edge is eligible now, whichever kind of box each of its
    // two ends resolved to (`End::Node` or `End::Cluster`) — `EligibleEdge::source`/`target` name
    // whichever id that end's own box lives under (the anchor node's id for a node end, the
    // subgraph's own id for a cluster end), which is exactly what `orthogonal::build_by_id` (fed by
    // this same `placed_clusters`) resolves a box out of. There is no longer a case where an edge
    // falls back to the spline path just because one of its ends names a block.
    //
    // `eligible` is kept (not scoped to this block) because stage 4's `orthogonal::avoid_label_plates`
    // needs the same edge list again, once labels exist to check ports against.
    let eligible: Vec<orthogonal::EligibleEdge> = if spec.routing == Routing::Orthogonal {
        prepared
            .iter()
            .map(|p| {
                let source = match p.tail_end {
                    edges::End::Cluster(_) => p.edge.from.as_str(),
                    edges::End::Node(..) => p.tail_id.as_str(),
                };
                let target = match p.head_end {
                    edges::End::Cluster(_) => p.edge.to.as_str(),
                    edges::End::Node(..) => p.head_id.as_str(),
                };
                orthogonal::EligibleEdge {
                    id: p.edge.id.as_str(),
                    source,
                    target,
                    raw: &p.raw,
                    // Rank is always read off the *anchor* node, whichever end is a cluster: a
                    // subgraph itself carries no rank of its own in dagre's compound layout (only
                    // its member leaves are ranked), and the anchor's rank is the same proxy
                    // `align_straight_lanes` above and `classify`'s reverse-edge detection below
                    // already treat "how far into the flow this end's block sits" as.
                    source_rank: g.node(p.tail_id.as_str()).and_then(|n| n.rank),
                    target_rank: g.node(p.head_id.as_str()).and_then(|n| n.rank),
                    source_out_degree: out_degree.get(&p.edge.from).copied().unwrap_or(0),
                    target_in_degree: in_degree.get(&p.edge.to).copied().unwrap_or(0),
                    // §10-1 item 4's routing half, the counterpart of the `rank_only`/
                    // `aside_weight` pair this same edge was handed to dagre with: an aside is
                    // drawn on the outer perimeter lane whichever way it points.
                    aside: is_aside(spec.routing, p.edge.stroke),
                }
            })
            .collect()
    } else {
        Vec::new()
    };
    let (mut orthogonal_points, required_size): (
        HashMap<String, Vec<Point>>,
        HashMap<String, Size>,
    ) = if spec.routing == Routing::Orthogonal {
        // §10-3 item 13's own "pass-through 行の予約", now two `route_flowchart` calls at most —
        // `reserve_pass_through_rows`'s own doc explains why deciding it before any route exists
        // (rank topology alone) is the `long-edge` regression. The first call here is the trial: its
        // own `pass_through_eligible` (which rank-skipping edges' `classify`-decided shape actually
        // draws a flow-axis row at all) is what `reserve_pass_through_rows` needs, so nothing before
        // this point can compute it. Capped at two total calls — the same finite-retry shape `lay_
        // out_spec`'s own growth loop uses (`MAX_GROWTH_PASSES`'s doc) — because a second reservation
        // pass would need a *third* route to judge itself against, which this cap declines to chase.
        let mut routed = orthogonal::route_flowchart(
            spec.direction,
            &nodes,
            &placed_clusters,
            &eligible,
            &chain_next,
            spec.fixed_self_loops,
        );
        if tree.is_empty() {
            let moved = reserve_pass_through_rows(
                spec.direction,
                &mut nodes,
                &node_rank,
                &drawable,
                &routed.pass_through_eligible,
            );
            if moved {
                routed = orthogonal::route_flowchart(
                    spec.direction,
                    &nodes,
                    &placed_clusters,
                    &eligible,
                    &chain_next,
                    spec.fixed_self_loops,
                );
            }
        }
        let orthogonal::RoutedFlowchart {
            mut points,
            mut required_size,
            pass_through_eligible: _,
            bar_geometry,
        } = routed;
        apply_bar_geometry(&mut nodes, &bar_geometry);
        // §10-5 S4: `route_flowchart` itself never sizes a bar (`evict`'s own doc — a bar's face
        // is never claimed the way an ordinary node's is), so this pass's own `bar_min_sizes`
        // (computed above, from the *same* node positions this route was just drawn against) is
        // the only source of a bar's growth requirement — folded in here rather than kept apart,
        // so a bar competing for `apply_growth`'s `max(current, required)` behaves exactly like
        // every other node's growth signal already does.
        for (id, need) in bar_min_sizes {
            let entry = required_size.entry(id).or_insert(Size::new(0.0, 0.0));
            entry.w = entry.w.max(need.w);
            entry.h = entry.h.max(need.h);
        }
        // A `staircase` edge's own local route can coincide with another unrelated detour edge's
        // (`orthogonal::separate_coincident_detours`'s own doc — lost the perimeter lane's shared
        // stagger bookkeeping once it stopped using it, 2026-09-01). Runs before label plates and
        // crossing gaps so both later passes see the final, separated geometry.
        orthogonal::separate_coincident_detours(
            spec.direction,
            &nodes,
            &placed_clusters,
            &eligible,
            &mut points,
        );
        (points, required_size)
    } else {
        (HashMap::new(), HashMap::new())
    };

    // --- clip each edge to the real outlines, then put its label on the clipped line -------------
    //
    // `orthogonal_index` remembers, for every orthogonal-routed edge, where it landed in
    // `placed_edges` — stage 4's `avoid_label_plates` pass below needs to write a pushed port's new
    // polyline (and, if the edge carries one, its label's new position) back into the exact
    // `PlacedEdge` it came from, and a `PlacedEdge` does not otherwise carry the `SpecEdge::id` a
    // `HashMap<String, _>` result is keyed by.
    let mut orthogonal_index: HashMap<String, usize> = HashMap::new();
    // §10-1 item 3's minimum-length fix: how far short of its own minimum each labelled edge's
    // chosen segment falls, this pass — `lay_out_spec`'s growth loop adds this onto `label_boosts`
    // for the next retry. Only ever holds an entry for an edge whose segment actually came up
    // short; `apply_label_growth`'s own doc explains why an unmet edge is the only kind recorded.
    let mut label_shortfall: HashMap<String, f64> = HashMap::new();
    let mut placed_edges: Vec<PlacedEdge> = Vec::with_capacity(prepared.len());
    // Borrowed, not moved: `eligible`'s `EligibleEdge::raw` fields borrow directly into
    // `prepared`'s own `raw` vectors, and that borrow has to stay alive past this loop for the
    // `avoid_label_plates` call below to reuse `eligible` — so `prepared` cannot be consumed here
    // the way it could before this pass existed.
    for PreparedEdge {
        edge,
        tail_id,
        head_id: _,
        raw,
        tail_end,
        head_end,
    } in &prepared
    {
        // Every drawable edge is orthogonal-eligible under `Routing::Orthogonal` now, whichever
        // kind of box each end resolved to — `eligible`'s own doc above.
        let is_orthogonal = spec.routing == Routing::Orthogonal;
        let (points, straight) = if is_orthogonal {
            // Present for every eligible edge — `route_flowchart` was called with exactly this
            // `eligible` set above. The spline fallback is defensive only, never expected to run.
            let pts = orthogonal_points
                .get(edge.id.as_str())
                .cloned()
                .unwrap_or_else(|| edges::route(raw, tail_end, head_end));
            (pts, true)
        } else {
            (edges::route(raw, tail_end, head_end), false)
        };

        // The second half of "the label stays on the line": put it at the half-way point of the
        // line as clipped, not where dagre parked it — except under orthogonal routing, where
        // §10-1 item 3 asks for more than "somewhere on the line": the *segment* the label sits on,
        // snapped by `orthogonal::label_slot` rather than measured by arc length (that function's
        // own doc walks through a real case — a `TD` edge's label landing 1.5px from a bend, most
        // of a wide plate hanging off the actual line — that plain arc-length midpoint drew before
        // this existed, found by dumping a real routed diagram rather than guessed at).
        let placed_label = edge.label.clone().and_then(|l| {
            if is_orthogonal {
                let size = Size::new(l.width + LABEL_PAD_X * 2.0, l.height + LABEL_PAD_Y * 2.0);
                // §10-5 round 5: which segment the plate goes on is decided against every *other*
                // routed line and every node/frame border, not on the polyline alone
                // (`orthogonal::label_slot_clear`'s own doc). Every orthogonal route in the
                // diagram is already final here — `route_flowchart` returned the whole map before
                // this loop started — so this is asking the question against the finished picture,
                // not against a half-built one.
                let slot = orthogonal::label_slot_clear(spec.direction, &points, &|center| {
                    orthogonal::plate_coverage(
                        center,
                        size,
                        edge.id.as_str(),
                        &orthogonal_points,
                        &nodes,
                        &placed_clusters,
                    )
                })?;
                // §10-5 S3 ("ラベルはループ外側4pxに浮かせて中央揃え…線上プレート則の唯一の例外"):
                // a self-transition's label never sits *on* its own line the way every other
                // orthogonal edge's does — `label_slot` already finds the loop's one flow-axis
                // segment (the outward leg `route_state_self_loop` built), but centring the plate
                // on it would paint the plate over the line itself, which the S3 exception exists
                // specifically to avoid (the loop's own legs are always too short to meet the
                // ordinary minimum-length rule, §10-1 item 3's own reasoning for why this needs a
                // dedicated exception rather than a bigger plate). Pushed further along the
                // *cross* axis, past the segment, by `SELF_LOOP_LABEL_GAP` plus half the plate's
                // own cross-axis extent — away from the node, the same direction the loop itself
                // already bulges.
                if spec.fixed_self_loops && edge.from == edge.to {
                    if let Some(&ni) = by_id.get(tail_id.as_str()) {
                        let node = &nodes[ni];
                        let cross_gap = orthogonal::SELF_LOOP_LABEL_GAP
                            + match spec.direction {
                                Direction::TopToBottom | Direction::BottomToTop => size.w / 2.0,
                                Direction::LeftToRight | Direction::RightToLeft => size.h / 2.0,
                            };
                        let center = match spec.direction {
                            Direction::TopToBottom | Direction::BottomToTop => {
                                let sign = if slot.center.x >= node.center.x {
                                    1.0
                                } else {
                                    -1.0
                                };
                                Point::new(slot.center.x + sign * cross_gap, slot.center.y)
                            }
                            Direction::LeftToRight | Direction::RightToLeft => {
                                let sign = if slot.center.y >= node.center.y {
                                    1.0
                                } else {
                                    -1.0
                                };
                                Point::new(slot.center.x, slot.center.y + sign * cross_gap)
                            }
                        };
                        return Some(PlacedEdgeLabel {
                            center,
                            size,
                            label: l,
                        });
                    }
                }
                // Only a flow-axis segment's length is ever grown by `label_boosts` (see
                // `lay_out_spec_pass`'s own doc on why boosting the other dimension would ask
                // dagre for room in an axis it never uses to size this gap) — recording a
                // shortfall for a cross-axis segment would ask `lay_out_spec`'s growth loop for
                // something it has no way to deliver, and it would never stop asking (the
                // shortfall can never shrink), which is exactly `MAX_GROWTH_PASSES`'s "defensive
                // cap, not the expected path" scenario, not a real fix.
                if slot.is_flow_axis {
                    let need = orthogonal::label_min_length(size, slot.horizontal);
                    if slot.length + 1e-6 < need {
                        label_shortfall.insert(edge.id.clone(), need - slot.length);
                    }
                }
                Some(PlacedEdgeLabel {
                    center: slot.center,
                    size,
                    label: l,
                })
            } else {
                edges::arc_midpoint(&points).map(|center| PlacedEdgeLabel {
                    center,
                    size: Size::new(l.width + LABEL_PAD_X * 2.0, l.height + LABEL_PAD_Y * 2.0),
                    label: l,
                })
            }
        });

        let side_label = |label: &Option<Label>, at_start: bool| -> Option<PlacedEdgeLabel> {
            let l = label.clone()?;
            let center = edges::end_label_anchor(&points, at_start, l.width, l.height)?;
            Some(PlacedEdgeLabel {
                center,
                size: Size::new(l.width + LABEL_PAD_X * 2.0, l.height + LABEL_PAD_Y * 2.0),
                label: l,
            })
        };
        let start_label = side_label(&edge.start_label, true);
        let end_label = side_label(&edge.end_label, false);

        if is_orthogonal {
            orthogonal_index.insert(edge.id.clone(), placed_edges.len());
        }
        // §10-3 item 6 ("辺色＝下流クラス色を1段明るく", konoma-orthogonal only): fills the gap the
        // ordinary cascade above leaves when the source named neither `linkStyle`/`class`/`:::` for
        // *this edge itself* — priority is "linkStyle 明示 > 自動導出 > テーマ既定", so this only
        // ever runs when `edge.style` (the edge's own explicit resolution) has no `stroke` of its
        // own to begin with, and only ever *adds* a stroke/text, never touches `fill`/`stroke_width`
        // /`dash` (an edge does not have a fill, and a lightened line keeps the same width and dash
        // pattern its own theme/explicit style already chose). "下流ノード" is `edge.to`'s own
        // resolved `class`/`style` stroke (`PlacedNode::style`, `SpecNode`'s own cascade in
        // `spec_of` above) — `by_id`/`nodes` are the very same map and slice `route_flowchart`
        // routed this edge against, so "downstream" always means the edge's own real target, cluster
        // -anchored edges included via `by_id` simply having no entry for a cluster id (the `?`
        // chain answers `None`, leaving the edge exactly as it drew before this rule existed).
        let style = if is_orthogonal
            && edge
                .style
                .as_ref()
                .and_then(|s| s.stroke.as_deref())
                .is_none()
        {
            by_id
                .get(edge.to.as_str())
                .and_then(|&i| nodes[i].style.as_ref())
                .and_then(|s| s.stroke.as_deref())
                .and_then(style::lighten)
                .map(|lightened| {
                    let mut s = edge.style.clone().unwrap_or_default();
                    s.stroke = Some(lightened.clone());
                    // The label's own colour follows the line's, same as the arrowhead already does
                    // via `tip_matches_line` — but only when the edge did not already ask for its
                    // own label colour (`color:` in a `linkStyle`/`class`/`style` declaration).
                    if s.text.is_none() {
                        s.text = Some(lightened);
                    }
                    s
                })
                .or_else(|| edge.style.clone())
        } else {
            edge.style.clone()
        };
        placed_edges.push(PlacedEdge {
            from: edge.from.clone(),
            to: edge.to.clone(),
            points,
            gaps: Vec::new(),
            tip_start: edge.tip_start,
            tip_end: edge.tip_end,
            stroke: edge.stroke,
            label: placed_label,
            start_label,
            end_label,
            badge: None,
            series: None,
            straight,
            overlay: false,
            style,
            curve: edge.curve,
            // Set together with `straight`, by the same `if`: both are true exactly when
            // `orthogonal::route_flowchart` routed this edge, and false for every other edge this
            // function ever builds (a flowchart's own splines edge included).
            tip_matches_line: straight,
        });
    }

    // --- stage 4's other half: push a port clear of a foreign label plate -------------------------
    //
    // §10-1 item 3's other rule ("ポートの垂直レーンがラベルプレートと重なる場合はポートをさらに
    // 16px外へ") runs once, now that every edge's label is placed: `orthogonal::avoid_label_plates`
    // reads every plate this pass built, pushes any port whose stub runs through a plate that is
    // not its own, and hands back the (possibly changed) polyline and, for the pushed edge's own
    // label if it carries one, the label's new position — both written back into `placed_edges`
    // through `orthogonal_index`.
    if spec.routing == Routing::Orthogonal && !eligible.is_empty() {
        let mut plates: HashMap<String, PlacedEdgeLabel> = HashMap::new();
        for (id, &idx) in &orthogonal_index {
            if let Some(l) = &placed_edges[idx].label {
                plates.insert(id.clone(), l.clone());
            }
        }
        orthogonal::avoid_label_plates(
            spec.direction,
            &nodes,
            &placed_clusters,
            &eligible,
            &mut orthogonal_points,
            &mut plates,
            &chain_next,
        );
        for (id, &idx) in &orthogonal_index {
            if let Some(new_points) = orthogonal_points.get(id) {
                if *new_points != placed_edges[idx].points {
                    placed_edges[idx].points = new_points.clone();
                    if let Some(new_plate) = plates.get(id) {
                        placed_edges[idx].label = Some(new_plate.clone());
                    }
                }
            }
        }

        // --- stage 5's last piece: the 12px crossing gap on the spanning side ------------------
        //
        // §10-1 item 4's other rule ("辺どうしの交差では跨ぐ側…に12pxの隙間を開ける"), run last —
        // after every earlier stage 1-5 pass, `avoid_label_plates` included, has already settled
        // every edge's final route, so a gap is never computed against a polyline about to change
        // out from under it. `PlacedEdge::gaps` itself has the rest of the story (empty for every
        // edge this loop does not touch, which keeps `svg::emit_edge` drawing the single `<path>`
        // it always drew for anything that never crosses another edge).
        let gap_map = orthogonal::insert_crossing_gaps(
            spec.direction,
            &nodes,
            &placed_clusters,
            &eligible,
            &orthogonal_points,
        );
        for (id, &idx) in &orthogonal_index {
            if let Some(g) = gap_map.get(id) {
                placed_edges[idx].gaps = g.clone();
            }
        }
    }

    let mut diagram = Diagram {
        width: 0.0,
        height: 0.0,
        nodes,
        edges: placed_edges,
        clusters: placed_clusters,
        lifelines: Vec::new(),
    };
    normalise(&mut diagram);
    Ok((diagram, required_size, label_shortfall))
}

/// One edge on its way to being drawn.
///
/// `tail`/`head` are the nodes dagre was actually asked to route between, which are the endpoints
/// the author wrote unless one of them named a block — see [`clusters::Tree::anchor`].
struct Drawable<'a> {
    edge: &'a SpecEdge,
    tail: String,
    head: String,
}

/// §10-3 item 8's own fix, in full: overwrites every real node's `rank`, and its flow-axis `x`/`y`,
/// directly on `g` — every later read of either (this function's own caller, `align_straight_lanes`'s
/// `node_rank`, and the `source_rank`/`target_rank` `EligibleEdge` is built from) sees the corrected
/// value with no further plumbing, because all three read straight off `g.node(id)`.
///
/// # Why network simplex cannot be trusted for this
///
/// dagre's `network_simplex` ranker (`layout::rank::network_simplex`) finds a rank assignment that
/// minimises `Σ weight(e) * length(e)` subject to every edge's `minlen`. That objective is silent
/// about *where* a node with slack sits: a node whose one in-edge and one out-edge both carry the
/// default weight of 1 can be placed anywhere in its feasible range without changing the total cost
/// at all (moving it one rank further from its source costs exactly what moving it one rank closer
/// to its own successor saves) — which rank the pivoting settles on is an implementation artefact,
/// not a preference. A branching source's fan-out edge is exactly this shape whenever the target
/// has its own downstream chain (`docs/FEATURE-MERMAID-RENDERER.md` §10-3 item 8's own example:
/// `設定のルール -->|画像| デコード --> セルに合わせる`, `デコード` free to land anywhere between the
/// two), which is why some of a ten-way fan-out hugged its source and others drifted downstream
/// toward whatever their own chain eventually needed, in no way tied to which branch a reader would
/// expect to look "closer". An earlier version of this fix tried breaking the tie by giving a
/// branching source's fan-out edges a heavier network-simplex `weight` instead of replacing the
/// ranker outright — abandoned once it was shown, on the `length` corpus case
/// (`flowchart TD  A ---> B  A --> C  B --> D  C --> D`), to change *which* rank network simplex
/// puts `B` on at all (not just position within a rank), producing a strictly higher-cost tree by
/// this same objective — evidence the ported network simplex's pivoting does not always reach the
/// optimum once edge weights are far from uniform, not a lever this fix can lean on safely.
///
/// # As-soon-as-possible layering
///
/// Every node's rank is instead recomputed from scratch as `max` over its forward in-edges of
/// `(that predecessor's own new rank + minlen)`, `0` for a node with none — the classic Sugiyama
/// "as-soon-as-possible" layering (in contrast to the *vendored* `layout::rank::longest_path`
/// ranker, which despite the similar name computes the mirror-image "as-late-as-possible" scheduling
/// pull-to-the-sinks dagre.js itself calls `longestPath`; using it here would pull every leaf flush
/// with the diagram's deepest sink instead, the opposite of what §10-3 item 8 asks for). This is
/// *the* minimum feasible rank — no rank assignment satisfying every `minlen` can place any node
/// earlier — so it can never violate a constraint, and it always gives a fan target `source_rank +
/// minlen`, regardless of how many further ranks its own downstream chain goes on to need.
///
/// Sorting real nodes by dagre's own (about-to-be-discarded) rank is a valid topological order over
/// the forward-only edge subgraph: a forward edge, by `is_reverse`'s own convention (reproduced
/// here), always goes from a strictly lower old rank to a strictly higher one, because dagre's own
/// ranking is already consistent with the DAG `acyclic::run` produced (a back edge, and a self-loop,
/// are both excluded from the constraint graph below the same way dagre's own `remove_self_edges`
/// and cycle-reversal keep them out of its ranking).
///
/// # Blocks are re-ranked as one unit
///
/// §10-5 round 4. Until then this pass was skipped outright for any diagram with a frame in it
/// (`tree.is_empty()`), because a frame's rectangle was *read back* from dagre's own compound-layout
/// border nodes and this pass never touches a border node — so a moved member left the frame around
/// it stale. `rebuild_frames` removes that reason: under `Routing::Orthogonal` a frame is derived
/// from its members afterwards, so moving a member is simply moving a member.
///
/// What replaces the gate is a **unit**: every top-level block is one, and so is every node no block
/// holds ([`clusters::Tree::outermost`]). A unit's members keep their relative *level* — their index
/// within the sorted set of dagre ranks the unit's own members occupy — so the block's interior
/// ordering survives untouched and only the block as a whole is re-layered. An edge crossing a
/// border therefore reads as a constraint between two units, offset by the two levels it actually
/// touches:
///
/// ```text
/// start[unit(head)]  >=  start[unit(tail)] + level(tail) + minlen - level(head)
/// ```
///
/// which is exactly the plain node ASAP rule when both units are single nodes (both levels are 0),
/// and is what `AnchorRole::Entry`/`Exit` already mean geometrically: an edge into a block arrives
/// at whichever level its entry member sits on, and one out of a block leaves from its exit
/// member's. Edges wholly inside one unit are not constraints at all — the levels already encode
/// them.
///
/// Solved by bounded relaxation rather than in one topological sweep: units are *not* guaranteed to
/// be topologically ordered by their own lowest dagre rank (a wide block can receive an edge into a
/// late member from a node that sits above the block's own first member), so a single ordered pass
/// could read a predecessor's start before it was final. If the relaxation has not settled within
/// one round per unit — only reachable from a cycle in the unit graph, which nesting a *block*'s
/// members inside it cannot produce but a hand-built [`GraphSpec`] is not stopped from writing —
/// this pass gives up and leaves dagre's own ranking exactly as it found it, rather than emitting a
/// half-relaxed layering.
///
/// # Room for the frames themselves
///
/// The column loop below also has to leave the space a frame needs *around* its members, since
/// nothing else will: [`clusters::PAD`] beyond the block's own first and last member column, plus
/// the title band at the top, once per level of nesting that starts (or ends) on that column. That
/// is the same arithmetic [`rebuild_frames`] does on the cross axis, applied to the flow axis, so a
/// frame's edge never lands on the node in the column before it.
///
/// # What this does not do
///
/// Cross-axis (row) position is untouched — only `align_straight_lanes`, which runs after this and
/// reads the corrected rank back off `g`, ever moves a node across the flow. A self-loop's raw
/// dagre waypoints (`mod.rs`'s own `raw`, read from `g`'s edge label after this point) are not
/// shifted for a flow-axis move this function made, the same way they were not shifted for a
/// cross-axis move before `align_straight_lanes`'s own `alignment_deltas` return existed to fix
/// it — a self-loop on a node this pass actually relocates is a known gap, not silently assumed
/// impossible; see `docs/STATUS.md`'s own ★未修正 entry.
fn pull_back_fan_ranks(
    g: &mut Graph<NodeLabel, EdgeLabel>,
    direction: Direction,
    drawable: &[Drawable],
    sizes: &HashMap<String, Size>,
    measured: &HashMap<&str, &SpecNode>,
    edge_label_dims: &HashMap<String, (f64, f64)>,
    tree: &clusters::Tree,
) {
    let old_rank = |g: &Graph<NodeLabel, EdgeLabel>, id: &str| -> i32 {
        g.node(id).and_then(|n| n.rank).unwrap_or(0)
    };

    let mut ids: Vec<String> = measured.keys().map(|id| id.to_string()).collect();
    ids.sort_by_key(|id| old_rank(g, id));

    // --- units, and each node's own level inside its unit ---------------------------------------
    let unit_of: HashMap<&str, &str> = ids
        .iter()
        .map(|id| {
            let unit = tree.outermost(id).unwrap_or(id.as_str());
            (id.as_str(), unit)
        })
        .collect();
    let mut unit_ranks: HashMap<&str, Vec<i32>> = HashMap::new();
    for id in &ids {
        unit_ranks
            .entry(unit_of[id.as_str()])
            .or_default()
            .push(old_rank(g, id));
    }
    // A node that **leaves a block and comes straight back into it** occupies one of that block's
    // own flow levels, even though it is not a member: `subgraph C { c1 --> c2 }` with
    // `c1 --> X --> c2` needs three levels across `C`'s span, and dagre already laid it out that
    // way. Counting only the levels the *members* occupy collapses `c1` and `c2` to adjacent ones,
    // which then makes the unit graph state two contradictory constraints at once (`X` after `C`,
    // `C` after `X`) — the relaxation below could never satisfy them and gave up, dropping ASAP for
    // the whole diagram. Giving the level back removes the contradiction at its source rather than
    // papering over it: the constraints become `X >= C + 1` and `C >= X - 1`, which agree.
    //
    // Deliberately narrow — a rank strictly inside the block's own span, held by a node that both
    // receives an edge from a member and feeds one back into it. Every *other* node that happens to
    // share a rank with the block stays irrelevant, which is what keeps a wide block in a busy
    // diagram (`zz-design-2b`'s own `クラウド`) from inventing levels it does not have.
    {
        let mut leaves: std::collections::HashSet<(&str, &str)> = std::collections::HashSet::new();
        let mut returns: std::collections::HashSet<(&str, &str)> = std::collections::HashSet::new();
        for d in drawable {
            if d.tail == d.head {
                continue;
            }
            let (Some(&tail_unit), Some(&head_unit)) =
                (unit_of.get(d.tail.as_str()), unit_of.get(d.head.as_str()))
            else {
                continue;
            };
            if tail_unit == head_unit {
                continue;
            }
            leaves.insert((tail_unit, d.head.as_str()));
            returns.insert((head_unit, d.tail.as_str()));
        }
        for (unit, outside) in leaves.intersection(&returns) {
            let r = old_rank(g, outside);
            let Some(rs) = unit_ranks.get_mut(*unit) else {
                continue;
            };
            let (Some(&lo), Some(&hi)) = (rs.iter().min(), rs.iter().max()) else {
                continue;
            };
            if r > lo && r < hi && !rs.contains(&r) {
                rs.push(r);
            }
        }
    }
    for rs in unit_ranks.values_mut() {
        rs.sort_unstable();
        rs.dedup();
    }
    let level_of = |id: &str| -> i32 {
        let r = old_rank(g, id);
        unit_ranks
            .get(unit_of[id])
            .and_then(|rs| rs.iter().position(|&x| x == r))
            .unwrap_or(0) as i32
    };

    // Forward-only adjacency, `(predecessor unit, offset)` per unit — a back edge or a self-loop is
    // never a rank constraint, matching `classify`'s own `is_reverse` and dagre's own
    // `remove_self_edges`/cycle-reversal respectively, and an edge whose two ends share a unit is
    // already expressed by the two levels themselves (this function's own doc).
    let mut incoming: HashMap<&str, Vec<(&str, i32)>> = HashMap::new();
    for d in drawable {
        if d.tail == d.head {
            continue;
        }
        let (sr, tr) = (old_rank(g, &d.tail), old_rank(g, &d.head));
        if tr <= sr {
            continue;
        }
        let (Some(&tail_unit), Some(&head_unit)) =
            (unit_of.get(d.tail.as_str()), unit_of.get(d.head.as_str()))
        else {
            continue;
        };
        if tail_unit == head_unit {
            continue;
        }
        // A cluster-anchored end constrains the rank of the **block**, not of whichever member
        // `Drawable` resolved it to. `Tree::anchor` picks one member — the first descendant with no
        // internal out-edge (`Exit`) or no internal in-edge (`Entry`) — and that member is very
        // often *not* the block's own last (or first) level: in `state P { [*] --> p1; p1 --> p2;
        // p1 --> p3; p3 --> p4 }`, `P`'s exit anchor is `p2`, one level above `p4`. Constraining
        // `P --> Z` with `p2`'s level lands `Z` on `p4`'s own rank — beside `p4`, *inside* `P`'s
        // flow span, with the edge forced out through the frame's side face — which is the whole
        // shape §10-5's own S2 says a frame must not have (its ports are the ordinary ones, and a
        // forward edge out of a block leaves through the flow face past the block's own end).
        //
        // So a block end is read as the block: the largest level any of its descendants occupies
        // when it is the source, the smallest when it is the target — the block's own flow span,
        // which is what an edge into or out of the frame actually has to clear. `level_of` already
        // measures inside the **outermost** unit (`unit_of`), so this stays correct when the
        // written endpoint is a *nested* block: its descendants' levels are still counted in the
        // outer block's own rank list, which is the unit `start` is solved for.
        let block_level = |written: &str, exit: bool| -> Option<i32> {
            if !tree.contains(written) {
                return None;
            }
            let levels = tree
                .descendants(written)
                .into_iter()
                .filter(|m| unit_of.contains_key(m))
                .map(level_of);
            if exit {
                levels.max()
            } else {
                levels.min()
            }
        };
        let minlen = d.edge.minlen.max(1) as i32;
        let tail_level = block_level(&d.edge.from, true).unwrap_or_else(|| level_of(&d.tail));
        let head_level = block_level(&d.edge.to, false).unwrap_or_else(|| level_of(&d.head));
        let offset = tail_level + minlen - head_level;
        incoming
            .entry(head_unit)
            .or_default()
            .push((tail_unit, offset));
    }

    // Bounded relaxation to the least fixpoint — see this function's own doc on why one ordered
    // sweep is not enough, and on why an unsettled result is thrown away rather than used.
    let mut units: Vec<&str> = unit_ranks.keys().copied().collect();
    units.sort_unstable();
    let mut start: HashMap<&str, i32> = units.iter().map(|u| (*u, 0)).collect();
    let mut settled = false;
    for _ in 0..=units.len() {
        let mut changed = false;
        for u in &units {
            let Some(preds) = incoming.get(*u) else {
                continue;
            };
            let want = preds
                .iter()
                .map(|(p, offset)| start.get(*p).copied().unwrap_or(0) + offset)
                .max()
                .unwrap_or(0)
                .max(0);
            if want > start[*u] {
                start.insert(u, want);
                changed = true;
            }
        }
        if !changed {
            settled = true;
            break;
        }
    }
    // Not settled: the unit graph still holds a genuine cycle — an edge that leaves a block and
    // comes back into it through *more than one* outside node, so the level the returning path
    // needs cannot be recovered the way the single-node case's is (just above). dagre's own
    // ranking already resolved it (the cycle only exists once a block is collapsed to one unit),
    // and measured on `subgraph C { c1 --> c2 }` + `c1 --> X --> Y --> c2` its answer is the better
    // picture: five ranks with `c1 --> X --> Y --> c2` running forward the whole way, against the
    // three ASAP would compress it to with `Y --> c2` forced onto the perimeter ring. So this keeps
    // dagre's ranking rather than breaking the cycle by dropping a constraint — tried, and it made
    // that diagram worse while changing nothing else in the corpus.
    if !settled {
        return;
    }

    let new_rank: HashMap<String, i32> = ids
        .iter()
        .map(|id| {
            let r = start[unit_of[id.as_str()]] + level_of(id);
            (id.clone(), r)
        })
        .collect();

    // --- recompute the flow-axis coordinate per rank column, tightly packed ---------------------
    //
    // One column per distinct new rank, in ascending order, each positioned immediately after the
    // previous one's far edge plus `RANK_SEP` — the same gap dagre's own rank columns use, just
    // computed fresh because the set of nodes sharing a rank has changed. `flow`/`cross`'s own doc
    // in `orthogonal.rs` is the authority this reuses without importing it: "delta >= 0.0 always
    // means further along the axis dagre laid the rank out on" already holds for whatever
    // coordinate `layout`'s own `coordinate_system::undo` leaves behind for `BT`/`RL`, so assigning
    // a strictly increasing coordinate to a strictly increasing rank, unconditionally on
    // `direction`, reproduces that same convention rather than fighting it.
    let flow_extent = |id: &str| -> f64 {
        let size = sizes
            .get(id)
            .copied()
            .or_else(|| measured.get(id).map(|n| n.size))
            .unwrap_or(Size::new(0.0, 0.0));
        match direction {
            Direction::TopToBottom | Direction::BottomToTop => size.h / 2.0,
            Direction::LeftToRight | Direction::RightToLeft => size.w / 2.0,
        }
    };

    let mut distinct_ranks: Vec<i32> = new_rank.values().copied().collect();
    distinct_ranks.sort_unstable();
    distinct_ranks.dedup();
    let rank_index: HashMap<i32, usize> = distinct_ranks
        .iter()
        .enumerate()
        .map(|(i, &r)| (r, i))
        .collect();

    // §10-1 item 3's own "ラベル付き区間の最低長" still has to hold once this function is the one
    // deciding column gaps, not dagre: `edge_label_dims` is the exact `(w, h)` `lay_out_spec_pass`'s
    // edge-building loop already handed dagre for this edge (raw label size plus any prior pass's
    // `label_boosts`) — captured there rather than read back off `g` after `layout()` runs, because
    // `normalize::run`'s own denormalize step overwrites `EdgeLabel.width`/`.height` with the dummy
    // label-proxy *node*'s own box size once its chain collapses back into one edge, which is a
    // different number entirely (confirmed by dumping a plain, label-less edge and finding a
    // non-zero width there — dagre's own internal bookkeeping, not a label). Reusing the captured
    // value gives every labelled edge the same "gap = `RANK_SEP` + the flow-axis label dimension"
    // dagre's own halved-`ranksep`-either-side-of-the-label-proxy convention already gives every
    // other routing mode. Charged to the gap immediately after the edge's *tail* column
    // (`rank_index[tail]`) — where dagre's own doubled `minlen` always inserts the first proxy rank
    // — even for an edge spanning more than one column in the new ranking; an edge that skips
    // columns entirely (no node landed on an intermediate rank) still charges its one immediate
    // gap, same as a direct edge would.
    let mut extra_gap: HashMap<usize, f64> = HashMap::new();
    for d in drawable {
        if d.tail == d.head {
            continue;
        }
        let (Some(&tr), Some(&hr)) = (new_rank.get(d.tail.as_str()), new_rank.get(d.head.as_str()))
        else {
            continue;
        };
        if hr <= tr {
            continue;
        }
        let Some(&i) = rank_index.get(&tr) else {
            continue;
        };
        let label_dim = edge_label_dims
            .get(&d.edge.id)
            .map(|&(w, h)| match direction {
                Direction::TopToBottom | Direction::BottomToTop => h,
                Direction::LeftToRight | Direction::RightToLeft => w,
            })
            .unwrap_or(0.0);
        if label_dim > 0.0 {
            let entry = extra_gap.entry(i).or_insert(0.0);
            if label_dim > *entry {
                *entry = label_dim;
            }
        }
    }

    // Flow-axis room a frame's own edge needs beyond its first/last member column — one
    // `clusters::PAD` (plus the title band at the top) for a block that starts, or ends, on that
    // column.
    //
    // **Nesting sums; siblings take the maximum.** Two frames one inside the other really do each
    // need their own padding past the same member column, one beyond the other. Two *sibling*
    // frames starting on the same column do not: they sit side by side across the flow, so the room
    // one needs is room the other is using at the same time, and adding them reserved twice the gap
    // any picture ever shows (measured on `s --> a1` / `s --> b1` with `a1` and `b1` in two sibling
    // subgraphs — `s`'s own gap to the frames' top came out as `RANK_SEP` plus *two* heads).
    let head_of = |block: &clusters::Cluster| -> f64 {
        let title = Label::measure(&block.title);
        clusters::PAD
            + if title.is_blank() {
                0.0
            } else {
                title.height + clusters::TITLE_PAD_Y * 2.0
            }
    };
    // Each block's own first/last member column, once — read by the ancestor walk below as well as
    // by the accumulation itself.
    let span_of: HashMap<&str, (i32, i32)> = tree
        .iter()
        .filter_map(|block| {
            let member_ranks: Vec<i32> = tree
                .descendants(&block.id)
                .iter()
                .filter_map(|m| new_rank.get(*m).copied())
                .collect();
            let (&first, &last) = (member_ranks.iter().min()?, member_ranks.iter().max()?);
            Some((block.id.as_str(), (first, last)))
        })
        .collect();
    let (mut frame_head, mut frame_tail): (HashMap<i32, f64>, HashMap<i32, f64>) =
        (HashMap::new(), HashMap::new());
    for block in tree.iter() {
        let Some(&(first, last)) = span_of.get(block.id.as_str()) else {
            continue;
        };
        // This block's own nesting chain at `first`/`last`: itself plus every ancestor whose own
        // span starts (ends) on the same column, which are exactly the frames whose edges stack up
        // one beyond another there. Bounded by the tree's depth, which `Tree::build` keeps acyclic.
        let mut chain_head = 0.0;
        let mut chain_tail = 0.0;
        let mut cur = Some(block);
        for _ in 0..=tree.iter().len() {
            let Some(b) = cur else { break };
            match span_of.get(b.id.as_str()) {
                Some(&(f, _)) if f == first => chain_head += head_of(b),
                _ => {}
            }
            match span_of.get(b.id.as_str()) {
                Some(&(_, l)) if l == last => chain_tail += clusters::PAD,
                _ => {}
            }
            cur = b.parent.as_deref().and_then(|p| tree.get(p));
        }
        let head_entry = frame_head.entry(first).or_insert(0.0);
        *head_entry = head_entry.max(chain_head);
        let tail_entry = frame_tail.entry(last).or_insert(0.0);
        *tail_entry = tail_entry.max(chain_tail);
    }

    let mut column_flow: HashMap<i32, f64> = HashMap::new();
    let mut cursor = MARGIN;
    let mut prev_half = 0.0_f64;
    let mut prev_tail = 0.0_f64;
    for (i, &r) in distinct_ranks.iter().enumerate() {
        let half = ids
            .iter()
            .filter(|id| new_rank.get(id.as_str()) == Some(&r))
            .map(|id| flow_extent(id))
            .fold(0.0_f64, f64::max);
        let head = frame_head.get(&r).copied().unwrap_or(0.0);
        let pos = if i == 0 {
            MARGIN + head + half
        } else {
            let extra = i
                .checked_sub(1)
                .and_then(|p| extra_gap.get(&p))
                .copied()
                .unwrap_or(0.0);
            cursor + prev_half + prev_tail + RANK_SEP + extra + head + half
        };
        column_flow.insert(r, pos);
        cursor = pos;
        prev_half = half;
        prev_tail = frame_tail.get(&r).copied().unwrap_or(0.0);
    }

    for id in &ids {
        let Some(&r) = new_rank.get(id.as_str()) else {
            continue;
        };
        let Some(&flow_pos) = column_flow.get(&r) else {
            continue;
        };
        if let Some(node) = g.node_mut(id) {
            node.rank = Some(r);
            match direction {
                Direction::TopToBottom | Direction::BottomToTop => node.y = Some(flow_pos),
                Direction::LeftToRight | Direction::RightToLeft => node.x = Some(flow_pos),
            }
        }
    }
}

/// Cross-axis position of a node's own centre — `x` for `TB`/`BT` (the flow axis runs vertically,
/// so the cross axis is horizontal), `y` for `LR`/`RL`. A free-standing helper rather than
/// `orthogonal::cross` (the same idea, already used throughout that module): that function is
/// private, and `regroup_fan_lanes` below is the one caller of this idea outside `orthogonal.rs`.
fn cross_of(direction: Direction, n: &PlacedNode) -> f64 {
    match direction {
        Direction::TopToBottom | Direction::BottomToTop => n.center.x,
        Direction::LeftToRight | Direction::RightToLeft => n.center.y,
    }
}

/// Flow-axis position of a node's own centre — the complement of [`cross_of`].
fn flow_of(direction: Direction, n: &PlacedNode) -> f64 {
    match direction {
        Direction::TopToBottom | Direction::BottomToTop => n.center.y,
        Direction::LeftToRight | Direction::RightToLeft => n.center.x,
    }
}

/// Half the node's own size along the cross axis — the complement of [`cross_of`], read from
/// [`PlacedNode::size`] rather than from dagre's own `g` the way [`pull_back_fan_ranks`]'s `flow_
/// extent` does, because every caller here already has the node's placed size on hand.
fn cross_extent_of(direction: Direction, n: &PlacedNode) -> f64 {
    match direction {
        Direction::TopToBottom | Direction::BottomToTop => n.size.w / 2.0,
        Direction::LeftToRight | Direction::RightToLeft => n.size.h / 2.0,
    }
}

/// One cross-axis slot of a fan's own rank, as [`regroup_fan_lanes`] lays it out: a whole frame
/// moved as a body, or one member moved on its own. That function's own "slots" comment has the
/// rule for which a given fan member becomes.
enum Slot {
    /// A frame the fan reaches exactly once — moved whole, spaced by its own band.
    Frame(String),
    /// One member, spaced by its own box plus whatever its own frame needs beyond it on each side
    /// (`0` for a member with no frame, or on a side another member of the same frame is on).
    Member { id: String, lead: f64, trail: f64 },
}

/// Where the trunk sits among its own fan's members — the index [`regroup_fan_lanes`] splits `rest`
/// (every non-trunk member, already in §10-3 item 1's own colour-group order) at, and the one place
/// the two fan regimes §10-4's own [`orthogonal::FAN_ELIGIBLE_MIN_BRANCHES`] separates disagree
/// about branch order.
///
/// **1b's regime** (at most that many branches — a straight trunk plus at most one branch on *each*
/// cross-axis face, which is exactly what `orthogonal::classify`'s own `fan_eligible` says the basic
/// shape can seat): each branch takes one face, and **which** face is its own target's continuity.
/// A branch whose target keeps going takes the negative side (`TB` left / `LR` up), a branch into a
/// dead end the positive side (`TB` right / `LR` down) — so a diagram reads with its live path on
/// one consistent side and its terminations on the other. With one branch that decides the split
/// outright (before the trunk, or after it); with two, both faces are spoken for regardless, so the
/// split is fixed at the centre and continuity only decides which branch takes which face — a
/// stable partition, so two branches of the same continuity keep the colour/declaration order they
/// came in with (`3a`'s own `端末`: `圧縮転送` and `ハーフブロック` are both dead ends, and stay one
/// above and one below `画像プロトコル`).
///
/// **The retreat regime** (more branches than one per face): §10-1 item 1's own 退避則 packs every
/// branch onto the flow-axis face, 16px apart, and §10-3 item 1's own colour grouping owns the order
/// there — so the trunk keeps the group's own centre index (`len / 2`, integer division; an odd
/// leftover lands above it, which is `3a`'s own ten-way fan splitting 5 above / 4 below). Continuity
/// is deliberately *not* consulted: `3a`'s own reference geometry interleaves continuing and
/// dead-end branches through that column (`窓読み`/`構文強調`/`表`/`一覧` dead, `ページ描画` live,
/// then the trunk, then `usvg`/`デコード`/`キーフレーム` live and `プレビュー不可` dead), so there is
/// no continuity boundary there to put a trunk at in the first place.
///
/// Derived from every non-trunk branch of the design references that has one (`1b`, `2a`, `3a`'s own
/// `ブロックモデル` and `端末`, `4a`, `4b` — `docs/STATUS.md`'s own ★未修正 G6/G8 entry lists all
/// nine), and it explains all nine; the ten-way fan is the counterexample that fixes the regime
/// boundary above rather than one this rule bends to fit.
fn fan_split(
    member_ids: &[String],
    rest: &mut [String],
    continues: &impl Fn(&str) -> bool,
) -> usize {
    if member_ids.len() > orthogonal::FAN_ELIGIBLE_MIN_BRANCHES {
        return member_ids.len() / 2;
    }
    // Stable, so same-continuity branches keep the order the colour grouping handed over.
    rest.sort_by_key(|m| !continues(m));
    match rest {
        [only] => usize::from(continues(only)),
        _ => member_ids.len() / 2,
    }
}

/// §10-3's "ファン列内の並び順" (`docs/FEATURE-MERMAID-RENDERER.md`) — reorders the members of a
/// *pure* single-source fan-out (every node at one rank tracing back to exactly one common
/// predecessor, at least two of them — §10-3 item 11 widened this from "at least three" once `1b`
/// and `3a` both turned out to need the same rule for a plain two-way fan too, `regroup_fan_lanes`'s
/// own size-cut doc below has the detail) into the mechanical rule `3a`'s reference geometry
/// (`docs/mermaid-theme/handoff/round3-Konoma-Flowchart-Routing.dc.html`) reverse-engineers to:
///
/// * the trunk/chain edge's own target (`chain_next`) sits wherever [`fan_split`] puts it — at the
///   group's own centre index (`len / 2`, integer division — an odd leftover member always lands
///   *above* it: `3a`'s own ten-member fan splits 5 above / 4 below, not 4/5, because `設定のルール`'s
///   trunk pick `ブロックモデル` is this group's 6th of 10 members, at index 5) once the retreat rule
///   has packed every branch onto one face, and at the continuity boundary while 1b's own
///   one-branch-per-face shape still holds; that function's own doc has both halves;
/// * every other member is grouped by its own resolved colour (`PlacedNode::style`'s `stroke`,
///   which is exactly "下流クラス（＝辺色）" once §10-3 item 6's `style::lighten` runs an edge's own
///   colour off its downstream node's — this function's own doc quotes `docs/FEATURE-MERMAID-
///   RENDERER.md`'s exact words for the rule), the groups themselves ordered by whichever one's
///   first member this fan's own `candidates` list (declaration order — the order the source
///   itself wrote the branches in, not dagre's) reaches soonest, and members *within* one colour
///   group kept in that same declaration order ("同色内は安定" — stable, not re-sorted by anything
///   this function invents);
/// * a member with no `classDef`-backed class at all (`has_class`) is *always* the last group,
///   regardless of where its own edge was declared — "無クラス・行き止まりを外側".
///
/// Reverse-engineered, not derived from a written mermaid rule (§10-3's own implementation note
/// says as much): `3a` is one hand-placed reference drawing, so this is the simplest rule that
/// explains it, not a guarantee every future corpus fan reproduces `3a`-style hand layout
/// coordinate-for-coordinate. What it does guarantee, by construction (this function's own "no-op
/// guard" and "re-stack" steps below), is every pre-existing invariant this module's own corpus
/// tests already check — no new collision, no lane invented that was not there before.
///
/// Deliberately conservative in scope, matching [`pull_back_fan_ranks`]'s own reasoning for why a
/// narrow trigger beats a numeric threshold applied everywhere: only a rank whose *every* member
/// traces back to the same one predecessor is touched at all (a merge rank, or a rank mixing two
/// unrelated branches' targets, is left exactly as `align_straight_lanes` laid it out — reordering
/// *part* of a mixed rank while leaving the rest could open a gap or a collision this function has
/// no way to check for); a single-member "fan" (`idxs.len() < 2`, below) is skipped since there is
/// nothing to reorder; and even a pure fan is left alone unless the rule above actually asks for a
/// different member order than the one already there (`current == new_order` is a silent no-op),
/// so an already-correct branch this module's own pinned exact-geometry tests cover never has its
/// coordinates so much as touched by a call this function did not need to make. §10-3 item 11
/// (`docs/FEATURE-MERMAID-RENDERER.md`) is what widened the trigger to a plain two-way fan: with
/// exactly one non-trunk member, `rest` (below) has one entry, and [`fan_split`] reads that one
/// branch's own continuity to put it on the cross-axis side it belongs on — which reproduces `1b`'s
/// "画像=上・なし=下" (`画像` continues into `デコード`), `3a`'s "ブロックモデル 直下の 数式=上"
/// (`数式` continues into `ラスタライズ`) and `4b`'s own `休止=下` (a dead end) with one rule rather
/// than a size-specific formula per fan.
///
/// Unlike [`pull_back_fan_ranks`], this pass is *not* cluster-aware, and deliberately so even since
/// §10-5 round 4 made a block one body everywhere else. It only ever permutes a rank's own
/// cross-axis order, and its own caller re-runs [`orthogonal::align_straight_lanes`] whenever it
/// reports having touched anything — so a block's internal spine, which is itself a lane, is
/// re-straightened by that call before any frame is read (`read_clusters`/`rebuild_frames` run
/// after it), and a frame left overlapping a neighbour is separated by
/// [`orthogonal::clear_foreign_cluster_overlaps`] after that. A unit-aware version was written and
/// then removed: three mutations (space a block by its member's box rather than its frame; move a
/// block member on its own rather than the block) changed neither a test nor a single byte of any
/// of the six design-reference renders, because those two later passes re-establish exactly what
/// it was protecting.
///
/// Callers must re-run [`orthogonal::align_straight_lanes`] afterwards (`lay_out_spec_pass` does):
/// this function only ever permutes *which* node sits in which cross-axis slot and re-stacks the
/// group around its own original midpoint — it does not attempt the trunk's exact centreline
/// alignment itself, because that alignment is a whole-chain computation (`3a`'s own seven-segment
/// spine runs through five separate ranks) `align_straight_lanes` already owns end to end. What
/// this pass changes is *whether that pass can succeed*: its own "ランク内の並び順は変えない"
/// overlap sweep can only ever push a crowded member later, never reorder around it, so a trunk
/// member buried among nine differently-coloured same-rank siblings keeps landing off the shared
/// centreline no matter how many times alignment reruns on the same order — moving it to its new,
/// colour-grouped slot (with a full [`ORTHO_NODE_SEP`] of slack on both sides, by construction) is
/// what finally gives the sweep room to grant it that centreline.
///
/// Returns whether it actually moved anything — `mod.rs`'s own caller needs this to decide
/// whether the (not free, and not fully idempotent — that caller's own doc explains why a second
/// `align_straight_lanes` pass is not safe to run unconditionally) second alignment pass is worth
/// running at all.
fn regroup_fan_lanes(
    direction: Direction,
    nodes: &mut [PlacedNode],
    node_rank: &HashMap<String, i32>,
    candidates: &[(String, String)],
    chain_next: &HashMap<String, String>,
    has_class: &HashMap<String, bool>,
    lane_units: &orthogonal::LaneUnits,
) -> bool {
    let mut touched = false;
    let id_index: HashMap<String, usize> = nodes
        .iter()
        .enumerate()
        .map(|(i, n)| (n.id.clone(), i))
        .collect();

    let mut by_rank: HashMap<i32, Vec<usize>> = HashMap::new();
    for (id, &r) in node_rank {
        if let Some(&i) = id_index.get(id) {
            by_rank.entry(r).or_default().push(i);
        }
    }

    // Declaration-order position of each candidate edge, keyed by its *target* — what "安定"
    // (stable) and "group order" are both measured against (this function's own doc).
    let decl_index: HashMap<String, usize> = candidates
        .iter()
        .enumerate()
        .map(|(i, (_, t))| (t.clone(), i))
        .collect();
    let decl_of = |id: &str| decl_index.get(id).copied().unwrap_or(usize::MAX);

    let mut ranks: Vec<i32> = by_rank.keys().copied().collect();
    ranks.sort_unstable();

    for r in ranks {
        let idxs = &by_rank[&r];
        if idxs.len() < 2 {
            continue; // a single member has nothing to reorder against.
        }

        // This rank's own fan source: the one node with a candidate edge into *every* member of it
        // — this function's own doc, "conservative in scope". A member with a second incoming edge
        // from somewhere else does not disqualify the rank (`zz-design-2c`'s own `メタデータ DB` is
        // written twice, `API --> DB` as well as `W --> DB`, and the rank is still `W`'s own fan by
        // any reading of the picture); a rank whose members do not *all* trace back to one common
        // source is still left exactly as `align_straight_lanes` laid it out, because reordering
        // part of a mixed rank while leaving the rest could open a gap or a collision this function
        // has no way to check for. More than one such source is ambiguous — which fan's order would
        // it be? — and is skipped for the same reason.
        let member_ids: Vec<String> = idxs.iter().map(|&i| nodes[i].id.clone()).collect();
        let mut feeders: Vec<&str> = candidates
            .iter()
            .filter(|(_, t)| member_ids.contains(t))
            .map(|(s, _)| s.as_str())
            .filter(|s| {
                member_ids
                    .iter()
                    .all(|m| candidates.iter().any(|(cs, ct)| cs == s && ct == m))
            })
            .collect();
        feeders.sort_unstable();
        feeders.dedup();
        let [source] = feeders.as_slice() else {
            continue;
        };
        let source = (*source).to_string();

        let trunk_id: Option<String> = chain_next
            .get(&source)
            .filter(|t| member_ids.contains(t))
            .cloned();

        // --- bucket every non-trunk member by its resolved colour, "無クラス" last -------------
        let mut buckets: Vec<(String, Vec<String>)> = Vec::new();
        let mut none_bucket: Vec<String> = Vec::new();
        for id in &member_ids {
            if trunk_id.as_ref() == Some(id) {
                continue;
            }
            let classed = has_class.get(id).copied().unwrap_or(true);
            if !classed {
                none_bucket.push(id.clone());
                continue;
            }
            let key = nodes[id_index[id]]
                .style
                .as_ref()
                .and_then(|s| s.stroke.clone())
                // No resolved colour at all: its own singleton group rather than lumping it in
                // with every other uncoloured node, which would wrongly treat two unrelated plain
                // nodes as "the same colour".
                .unwrap_or_else(|| format!("\0{id}"));
            match buckets.iter_mut().find(|(k, _)| *k == key) {
                Some((_, members)) => members.push(id.clone()),
                None => buckets.push((key, vec![id.clone()])),
            }
        }
        buckets.sort_by_key(|(_, members)| {
            members
                .iter()
                .map(|m| decl_of(m))
                .min()
                .unwrap_or(usize::MAX)
        });
        // Within one colour, "同色内は安定" (this function's own doc) is not simply the fan edge's
        // own declaration order: `3a`'s own reference (`docs/mermaid-theme/handoff/round3-Konoma-
        // Flowchart-Routing.dc.html`) puts `ページ描画`/`usvg` (both converge one rank later, into
        // `ラスタライズ`) closer to the trunk than `デコード`/`キーフレーム` (converge two ranks
        // later, into `セルに合わせる`), even though `デコード` is declared *before* `ページ描画`.
        // The member whose own forward edge reaches a nearer rank sorts first — the same "shorter
        // reach sits closer to the spine" reasoning §10-3 item 2 already established for the
        // trunk's own end-of-chain tie-break (`align_straight_lanes`'s own doc on that fix) —
        // falling back to plain declaration order for a leaf (no forward edge of its own at all,
        // so nothing to measure) or a genuine tie.
        let nearest_forward_rank = |m: &str| -> i32 {
            candidates
                .iter()
                .filter(|(s, _)| s == m)
                .filter_map(|(_, t)| node_rank.get(t))
                .min()
                .copied()
                .unwrap_or(i32::MAX)
        };
        for (_, members) in &mut buckets {
            members.sort_by_key(|m| (nearest_forward_rank(m), decl_of(m)));
        }
        none_bucket.sort_by_key(|m| decl_of(m));

        let mut rest: Vec<String> = buckets.into_iter().flat_map(|(_, m)| m).collect();
        rest.extend(none_bucket);

        // Whether a member's own target keeps going — it is some other candidate edge's source, so
        // the branch that reaches it carries the diagram on rather than ending there. `candidates`
        // already excludes a self-loop (`lay_out_spec_pass`'s own `d.tail != d.head` filter), so
        // "a state that only loops back to itself" reads as the dead end it is, and it already
        // carries a cluster-anchored edge as its own anchor member's, so a branch into (or out of)
        // a block counts exactly like any other. An end marker has no out-edge at all, so it needs
        // no case of its own.
        let continues = |id: &str| candidates.iter().any(|(s, _)| s == id);

        let new_order: Vec<String> = match &trunk_id {
            Some(t) => {
                let before = fan_split(&member_ids, &mut rest, &continues);
                let split = before.min(rest.len());
                let mut order: Vec<String> = rest[..split].to_vec();
                order.push(t.clone());
                order.extend_from_slice(&rest[split..]);
                order
            }
            None => rest,
        };

        // Centred on the *source* node's own cross coordinate (`C`'s own centre, in `3a`'s own
        // terms — regel1's "箱を中心線対称に保ったまま拡大") rather than on the group's own current
        // top/bottom: by the time this runs, `nodes` already reflects the *first* `align_straight_
        // lanes` probe call's own overlap-resolution sweep, which can (and, on `3a`'s own fence-3
        // source, does) push a crowded chain member well past the fan's natural block — reading
        // the block's extent back off that already-distorted state would inflate the very gap this
        // pass exists to close. The source's own centreline is stable regardless: no rank-order
        // pass this module runs above ever moves a node relative to its *own* rank's siblings by
        // growing or shrinking `C` itself, and `C`'s ports are already centred on it (regel1).
        let mid = id_index
            .get(&source)
            .map(|&i| cross_of(direction, &nodes[i]))
            .unwrap_or_else(|| {
                let (mut top, mut bottom) = (f64::INFINITY, f64::NEG_INFINITY);
                for &i in idxs {
                    let half = cross_extent_of(direction, &nodes[i]);
                    let c = cross_of(direction, &nodes[i]);
                    top = top.min(c - half);
                    bottom = bottom.max(c + half);
                }
                (top + bottom) / 2.0
            });

        // No-op guard: only touch geometry when the rule above actually asks for a different
        // member order than the one already there, *or* the trunk (if any) is not already sitting
        // exactly on `mid` — a rank whose members happen to already be in the right order can
        // still have its trunk crowded off the source's own centreline (found on `3a`'s own `端末`
        // fan: `K -> RI` is already the chosen, already-correctly-ordered trunk edge, and yet the
        // overlap sweep that ran before this function saw it still leaves `RI` off `K`'s own
        // centre — this function's own doc, "gives that sweep room to grant it the centreline",
        // does not help a member the sweep never had to move in the first place). A tolerance
        // rather than exact equality: this reads coordinates dagre's own floating-point layout
        // already carried through several passes of arithmetic.
        // A fan member inside a frame the fan's own source is outside of cannot be reordered freely
        // across the rank: a *foreign* member landing between two members of that frame ends up
        // inside its rectangle (the frame is derived from wherever its members sit —
        // `rebuild_frames`), and `clear_foreign_cluster_overlaps` then has to shove it back out —
        // off whatever lane it was put on. That is `zz-design-2c` exactly: `解析サンドボックス`
        // belongs to `クラウド`, `メタデータ DB` and `成果物保管` to `保存層` inside it, and the
        // straight lane's own member was the one being shoved.
        //
        // So the colour/continuity order above is kept, and then **each frame's own members are
        // pulled together into one run**, at the position of whichever of them the order reached
        // first. Bodies, not nodes, is what a frame's members form here; which body a member belongs
        // to is [`orthogonal::LaneUnits::separating_unit`]'s question, asked against the fan's own
        // source — the outermost frame holding the member that does not also hold the source, so a
        // fan *inside* one frame (`zz-design-2c`'s three, all in `クラウド`) still has separable
        // parts, and no body named here can contain `source`, whose own coordinate `mid` is read
        // from below. Deliberately not a *collapse* to one slot per body: `zz-design-2a`'s own
        // `ワーカー` holds two of `ルールに一致?`'s three branches, and the fan has to keep ordering
        // them *inside* the frame — a collapse leaves that to dagre and loses `デコード` /
        // `ブロックモデル`'s own places around the trunk.
        let body_of = |id: &str| lane_units.separating_unit(id, &source).to_string();
        let mut bodies: Vec<String> = Vec::new();
        for id in &new_order {
            let body = body_of(id);
            if !bodies.contains(&body) {
                bodies.push(body);
            }
        }
        let new_order: Vec<String> = bodies
            .iter()
            .flat_map(|body| {
                new_order
                    .iter()
                    .filter(|id| body_of(id) == *body)
                    .cloned()
                    .collect::<Vec<String>>()
            })
            .collect();

        // No-op guard: only touch geometry when the rule above actually asks for a different
        // member order than the one already there, *or* the trunk (if any) is not already sitting
        // exactly on `mid` — a rank whose members happen to already be in the right order can
        // still have its trunk crowded off the source's own centreline (found on `3a`'s own `端末`
        // fan: `K -> RI` is already the chosen, already-correctly-ordered trunk edge, and yet the
        // overlap sweep that ran before this function saw it still leaves `RI` off `K`'s own
        // centre — this function's own doc, "gives that sweep room to grant it the centreline",
        // does not help a member the sweep never had to move in the first place). A tolerance
        // rather than exact equality: this reads coordinates dagre's own floating-point layout
        // already carried through several passes of arithmetic.
        let mut current: Vec<String> = member_ids.clone();
        current.sort_by(|a, b| {
            cross_of(direction, &nodes[id_index[a]])
                .partial_cmp(&cross_of(direction, &nodes[id_index[b]]))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let trunk_aligned = trunk_id
            .as_ref()
            .map(|t| (cross_of(direction, &nodes[id_index[t]]) - mid).abs() < 0.01)
            .unwrap_or(true); // no trunk in this fan: order is the only thing that matters.
        if current == new_order && trunk_aligned {
            continue;
        }

        // The gap between two consecutive members, as [`orthogonal::align_straight_lanes`]'s own
        // overlap sweep will measure it afterwards: [`ORTHO_NODE_SEP`] between the two *bodies*, and
        // a body that is a frame reaches its own pad beyond the member's box on that side. Placing
        // members box-to-box instead leaves every frame boundary short by exactly that pad, and the
        // sweep then pushes the later member of the pair — which on `zz-design-2c` is the straight
        // lane's own `解析サンドボックス`, so the lane it was just given is lost again 16px later.
        // Each slot the rank is laid out in, in `new_order`'s own order: a whole frame, or one
        // member.
        //
        // A frame the fan reaches exactly once is one slot and moves as a body — `zz-design-4a`'s
        // own `プレビュー`, `composite-siblings`' own `Second`/`Third`. Moving only the member dagre
        // anchored the edge on would tear the frame open (its rectangle is derived from wherever its
        // members sit — `rebuild_frames`), and then nothing downstream can tell the frame's real
        // extent from the anchor's 14px marker box. A frame the fan reaches *more than once* is not
        // one slot: the fan has to order those members against each other, and `zz-design-2a`'s own
        // `ワーカー` (holding two of `ルールに一致?`'s three branches) is exactly that case — its
        // members take one slot each, the run stays contiguous so no foreign member lands inside the
        // frame, and the two ends of the run carry the frame's own pad so the neighbouring slots
        // leave the rectangle the room [`orthogonal::align_straight_lanes`]'s own overlap sweep will
        // demand of it afterwards.
        let runs: Vec<(String, Vec<String>)> = bodies
            .iter()
            .map(|body| {
                let members: Vec<String> = new_order
                    .iter()
                    .filter(|id| body_of(id) == *body)
                    .cloned()
                    .collect();
                (body.clone(), members)
            })
            .collect();
        let mut slots: Vec<Slot> = Vec::new();
        for (body, members) in &runs {
            match members.as_slice() {
                [only] if lane_units.is_block(body) => slots.push(Slot::Frame(body.clone())),
                _ => {
                    let pad = lane_units.frame_pad(body);
                    let last = members.len() - 1;
                    for (i, id) in members.iter().enumerate() {
                        slots.push(Slot::Member {
                            id: id.clone(),
                            lead: if i == 0 { pad } else { 0.0 },
                            trail: if i == last { pad } else { 0.0 },
                        });
                    }
                }
            }
        }

        // No-op guard: only touch geometry when the rule above actually asks for a different
        // member order than the one already there, *or* the trunk (if any) is not already sitting
        // exactly on `mid` — a rank whose members happen to already be in the right order can
        // still have its trunk crowded off the source's own centreline (found on `3a`'s own `端末`
        // fan: `K -> RI` is already the chosen, already-correctly-ordered trunk edge, and yet the
        // overlap sweep that ran before this function saw it still leaves `RI` off `K`'s own
        // centre — this function's own doc, "gives that sweep room to grant it the centreline",
        // does not help a member the sweep never had to move in the first place). A tolerance
        // rather than exact equality: this reads coordinates dagre's own floating-point layout
        // already carried through several passes of arithmetic.
        let mut current: Vec<String> = member_ids.clone();
        current.sort_by(|a, b| {
            cross_of(direction, &nodes[id_index[a]])
                .partial_cmp(&cross_of(direction, &nodes[id_index[b]]))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let trunk_aligned = trunk_id
            .as_ref()
            .map(|t| (cross_of(direction, &nodes[id_index[t]]) - mid).abs() < 0.01)
            .unwrap_or(true); // no trunk in this fan: order is the only thing that matters.
        if current == new_order && trunk_aligned {
            continue;
        }

        // What a slot takes up along the cross axis, and how it moves — a frame by its own band
        // (which is what the overlap sweep measures it by), a member by its box plus whatever pad
        // its own frame needs beyond it on the outward side.
        let extent = |nodes: &[PlacedNode], slot: &Slot| match slot {
            Slot::Frame(body) => lane_units.band(direction, nodes, &id_index, body),
            Slot::Member { id, lead, trail } => {
                let n = &nodes[id_index[id]];
                let (c, half) = (cross_of(direction, n), cross_extent_of(direction, n));
                (c - half - lead, c + half + trail)
            }
        };
        let slide = |nodes: &mut [PlacedNode], slot: &Slot, delta: f64| match slot {
            Slot::Frame(body) => lane_units.shift(direction, nodes, &id_index, body, delta),
            Slot::Member { id, .. } => {
                let idx = id_index[id];
                let flow_v = flow_of(direction, &nodes[idx]);
                let new_c = cross_of(direction, &nodes[idx]) + delta;
                nodes[idx].center = match direction {
                    Direction::TopToBottom | Direction::BottomToTop => Point::new(new_c, flow_v),
                    Direction::LeftToRight | Direction::RightToLeft => Point::new(flow_v, new_c),
                };
            }
        };

        touched = true;
        match trunk_id.as_ref().and_then(|t| {
            slots.iter().position(|slot| match slot {
                Slot::Frame(body) => *body == body_of(t),
                Slot::Member { id, .. } => id == t,
            })
        }) {
            // The trunk's own slot is moved until the trunk *node* sits on `mid` (the source's own
            // centreline) exactly, then every other slot stacked *outward* from it — never from a
            // computed block top — so the trunk's own cross coordinate lands precisely where
            // `align_straight_lanes`'s next call needs it to draw the source's own 0-bend edge, no
            // matter how asymmetric the before/after split is (`3a`'s own 5-before/4-after: this
            // function's own doc explains why it is uneven).
            Some(tp) => {
                let trunk = trunk_id
                    .as_ref()
                    .expect("the position came from `trunk_id`");
                let delta = mid - cross_of(direction, &nodes[id_index[trunk]]);
                slide(nodes, &slots[tp], delta);
                let (lo, hi) = extent(nodes, &slots[tp]);
                let mut edge = lo;
                for slot in slots[..tp].iter().rev() {
                    let (slot_lo, slot_hi) = extent(nodes, slot);
                    let delta = (edge - ORTHO_NODE_SEP) - slot_hi;
                    slide(nodes, slot, delta);
                    edge = slot_lo + delta;
                }
                let mut edge = hi;
                for slot in &slots[tp + 1..] {
                    let (slot_lo, slot_hi) = extent(nodes, slot);
                    let delta = (edge + ORTHO_NODE_SEP) - slot_lo;
                    slide(nodes, slot, delta);
                    edge = slot_hi + delta;
                }
            }
            // No trunk in this fan (e.g. every branch is a leaf): stack the whole group from its
            // own top, centred on `mid` as a block — there is no single slot's position to pin.
            None => {
                let total: f64 = slots
                    .iter()
                    .map(|slot| {
                        let (lo, hi) = extent(nodes, slot);
                        hi - lo
                    })
                    .sum::<f64>()
                    + ORTHO_NODE_SEP * (slots.len().saturating_sub(1)) as f64;
                let mut cursor = mid - total / 2.0;
                for slot in &slots {
                    let (slot_lo, slot_hi) = extent(nodes, slot);
                    let delta = cursor - slot_lo;
                    slide(nodes, slot, delta);
                    cursor = slot_hi + delta + ORTHO_NODE_SEP;
                }
            }
        }
    }
    touched
}

/// Extra clearance either side of a reserved pass-through row, beyond the occupying node's own
/// cross-extent — reuses §10-1 item 4's own "12px の隙間" figure for the same reason it exists
/// there: enough room that the straightened edge reads as visibly separate from the node it used
/// to detour around, not merely clear of its box by a hair.
const PASS_THROUGH_CLEARANCE: f64 = 12.0;

/// §10-3 item 13's own "pass-through 行の予約" (`docs/FEATURE-MERMAID-RENDERER.md`) — a rank-
/// skipping edge's own horizontal leg exits its source at the source's own cross coordinate (§10-1
/// item 1's "ソースの右辺中央から水平に出て" — `orthogonal::rank_lane_gap_bends`'s own doc quotes
/// the identical rule for the merge side) and runs straight through every intermediate rank column
/// at that exact row before bending toward its target. A node that happens to occupy that row in an
/// intermediate column sits directly in the edge's own straight run, so `orthogonal::classify`'s
/// collision ladder has no choice but to detour the edge around it — `docs/STATUS.md`'s own ★未修正
/// entry: `samples/mermaid.ja.md`'s "大きさ" flowchart draws `ページ描画 -> ラスタライズ` with four
/// bends, detouring around `数式`, purely because `数式` happens to land almost exactly on
/// `ページ描画`'s own row one column upstream of `ラスタライズ`. This pass moves the *occupying
/// node* instead — `3a`'s own reference geometry puts `数式` one row above `ページ描画`'s pass-
/// through row, never asking the edge to bend around it at all.
///
/// Reimplemented after a first attempt (2026-09-02, never committed — `docs/FEATURE-MERMAID-
/// RENDERER.md` §10-3's own implementation note has the post-mortem) judged the reserved row from
/// dagre's own internal, `minlen`-doubled rank numbering (`makeSpaceForEdgeLabels` — [`align_
/// straight_lanes`]'s own doc explains the doubling) instead of the real, per-edge-`minlen` rank
/// every node actually ends up on once [`pull_back_fan_ranks`] has run. The doubled numbering
/// invents a fictitious "every edge spans exactly two internal ranks" grid that has nothing to do
/// with which *real* nodes a rank-skipping edge's own row actually threads past, and so flagged the
/// `A ---> B` (`minlen` 2) corpus fixture's own `C` (`A --> C`, minlen 1) as sitting in `A ---> B`'s
/// pass-through row purely because the doubled grid placed both at the same internal rank — evicting
/// a node the edge's own source is directly, legitimately connected to, not a coincidental occupant.
///
/// This version works from `node_rank` (real ranks: [`pull_back_fan_ranks`]'s own recomputation from
/// each edge's own semantic `minlen`, never dagre's internal doubling — this is why the caller only
/// ever runs this pass in the same `tree.is_empty()` scope [`pull_back_fan_ranks`] itself is limited
/// to, so `node_rank` is guaranteed to be that real numbering here, not dagre's raw one) and adds the
/// one guard the failed attempt was missing: a candidate occupant connected to the pass-through
/// edge's own **source** by any real drawn edge (`drawable`'s own `(tail, head)` pairs, checked both
/// directions — `A --> C`, `A ---> B`'s own fixture) is never a coincidental blocker — it is exactly
/// the shape a genuine fan/chain member sitting on its own source's row is supposed to have, and is
/// left exactly where alignment/[`regroup_fan_lanes`] already put it. Deliberately *not* the same
/// guard against the edge's own **target**: `samples/mermaid.ja.md`'s own `数式` is connected to
/// `ラスタライズ` too (`数式 -> ラスタライズ` is itself a separate merge edge into the very node
/// `ページ描画 -> ラスタライズ` is heading for) — that is precisely the coincidental-occupant case
/// this pass exists to fix, not a reason to exempt it; only a real edge to the pass-through edge's
/// own *source* explains why a node legitimately shares its row.
///
/// A single pass per pass-through edge, not a fixpoint search — the same bounded-effort shape this
/// module's other local nudges use ([`orthogonal::clear_local_route`]'s own doc): moving one node
/// clear of a reserved row can in principle crowd a different sibling on the same rank, which this
/// pass's own inner sibling-clearance loop absorbs for the common case (one blocker, one or two
/// siblings) but does not chase through a second cascade.
///
/// `eligible` restricts which rank-skipping edges this runs for at all — [`orthogonal::
/// RoutedFlowchart::pass_through_eligible`]'s own doc explains why rank topology alone (`tr - sr >=
/// 2`, still checked below) is not enough: `long-edge`'s own `A -> E`, a branching source whose
/// `classify`-decided shape never draws a flow-axis row in the first place, used to get one
/// reserved anyway, evicting `D` off `C -> D`'s own legitimate straight lane for zero benefit (the
/// edge's real route was a perimeter detour that never threaded through that row at all). `eligible`
/// comes from a *trial* route (`mod.rs`'s own call site) — this is why the pass moved here from
/// running blind, before any route existed.
///
/// Returns whether anything actually moved, so the caller knows whether the trial route is now
/// stale and a second `route_flowchart` call is worth the cost.
#[must_use]
fn reserve_pass_through_rows(
    direction: Direction,
    nodes: &mut [PlacedNode],
    node_rank: &HashMap<String, i32>,
    drawable: &[Drawable],
    eligible: &std::collections::HashSet<String>,
) -> bool {
    let mut moved = false;
    let id_index: HashMap<String, usize> = nodes
        .iter()
        .enumerate()
        .map(|(i, n)| (n.id.clone(), i))
        .collect();

    // Every real, non-self-loop edge as an unordered id pair — the one thing that tells a
    // coincidental row occupant (evict it) apart from a real neighbour of either endpoint (leave it
    // exactly where alignment put it), per this function's own doc on the failed first attempt.
    let mut connected: std::collections::HashSet<(String, String)> =
        std::collections::HashSet::new();
    for d in drawable {
        if d.tail == d.head {
            continue;
        }
        connected.insert((d.tail.clone(), d.head.clone()));
        connected.insert((d.head.clone(), d.tail.clone()));
    }
    let is_connected = |a: &str, b: &str| connected.contains(&(a.to_string(), b.to_string()));

    let mut by_rank: HashMap<i32, Vec<usize>> = HashMap::new();
    for (id, &r) in node_rank {
        if let Some(&i) = id_index.get(id) {
            by_rank.entry(r).or_default().push(i);
        }
    }

    for d in drawable {
        if d.tail == d.head {
            continue;
        }
        if !eligible.contains(&d.edge.id) {
            continue; // this edge's own shape never draws a flow-axis row at all — nothing to protect.
        }
        let (Some(&sr), Some(&tr)) = (node_rank.get(&d.tail), node_rank.get(&d.head)) else {
            continue;
        };
        // "列 T（T > A+1）": at least one whole real rank column sits between source and target —
        // an ordinary adjacent-rank edge (the overwhelming majority) has nothing to reserve at all.
        if tr - sr < 2 {
            continue;
        }
        let Some(&source_idx) = id_index.get(&d.tail) else {
            continue;
        };
        let row = cross_of(direction, &nodes[source_idx]);

        for ri in (sr + 1)..tr {
            let Some(members) = by_rank.get(&ri) else {
                continue;
            };
            for &idx in members {
                let id = nodes[idx].id.clone();
                if id == d.tail || id == d.head {
                    continue;
                }
                if is_connected(&d.tail, &id) {
                    continue;
                }
                let half = cross_extent_of(direction, &nodes[idx]);
                let delta = cross_of(direction, &nodes[idx]) - row;
                if delta.abs() >= half + PASS_THROUGH_CLEARANCE {
                    continue; // already clear of the reserved row.
                }
                // Toward −∞ on the cross axis ("上へ", `3a`'s own 数式-above-ページ描画 placement)
                // when the occupant sits close enough to the row that which side it "already leans
                // toward" is not a meaningful signal (within half its own cross-extent — `3a`'s own
                // `数式` sits only ~1px south of `ページ描画`'s row, nowhere near a full node-width
                // away); otherwise moved further away from the row on whichever side it already
                // sat, so a node already clearly above/below the row is never flipped past it.
                let sign = if delta.abs() < half {
                    -1.0
                } else {
                    delta.signum()
                };
                let mut new_c = row + sign * (half + PASS_THROUGH_CLEARANCE);
                // Clear of every other member still on this rank, at its current position — see
                // this function's own doc on why this is one pass, not a fixpoint search.
                for &other in members {
                    if other == idx {
                        continue;
                    }
                    let other_half = cross_extent_of(direction, &nodes[other]);
                    let other_c = cross_of(direction, &nodes[other]);
                    let min_gap = half + other_half + ORTHO_NODE_SEP;
                    if (new_c - other_c).abs() < min_gap {
                        new_c = other_c + sign * min_gap;
                    }
                }
                let flow_v = flow_of(direction, &nodes[idx]);
                nodes[idx].center = match direction {
                    Direction::TopToBottom | Direction::BottomToTop => Point::new(new_c, flow_v),
                    Direction::LeftToRight | Direction::RightToLeft => Point::new(flow_v, new_c),
                };
                moved = true;
            }
        }
    }
    moved
}

/// Reads the frames dagre computed and settles each one's final rectangle.
///
/// A frame arrives from dagre hugging its members: `removeBorderNodes` sets it from the border
/// nodes' coordinates with no padding of its own, which leaves half a `nodesep` at the sides and
/// half a `ranksep` above and below. Two things can still be wrong with it, and both are about the
/// title, which the layout engine never saw:
///
/// * it can be **narrower than the title**. mermaid fixes the same thing the same way
///   (`clusters.js`: `width = max(node.width, bbox.width + padding)`).
/// * the band above the topmost member can be **shorter than the title**. That band is a fixed
///   consequence of `ranksep`, so a one-line title fits and a two-line one does not. mermaid does
///   not check, and draws the title over the node.
///
/// [`fit_titles`] fixes exactly those two, starting from dagre's own rectangle — which is right for
/// `Routing::Splines`, whose nodes sit precisely where dagre put them, and every golden file pins
/// it. It is **not** right for `Routing::Orthogonal`, where three passes have already moved nodes
/// across the flow since dagre's border nodes were computed (`lay_out_spec_pass`'s own call site
/// names them), so the rectangle they describe is stale by construction: it only ever grows to
/// swallow a moved member, never follows one, which leaves a frame lopsided around its own
/// contents and — because [`orthogonal::classify`] reads a cluster end's box *centre* — makes a
/// dead-straight lane through a block impossible to draw. So orthogonal takes [`rebuild_frames`]
/// instead, which derives the rectangle from the members outright.
///
/// **Ordering.** Whichever half runs, it runs once, after every pass that can move a node and
/// before every pass that reads a frame — `lay_out_spec_pass`'s own call site is where that is
/// stated, and it is the only place it is stated.
///
/// Either way the work runs deepest-first so that a block which grew is final by the time its
/// parent is measured, and every parent starts by absorbing its children — which is what keeps a
/// nested block inside the frame that contains it instead of poking out of the top of it.
fn read_clusters(
    g: &Graph<NodeLabel, EdgeLabel>,
    tree: &clusters::Tree,
    nodes: &[PlacedNode],
    routing: Routing,
) -> Vec<PlacedCluster> {
    let mut out: Vec<PlacedCluster> = Vec::new();
    for c in tree.iter() {
        let Some(n) = g.node(&c.id) else { continue };
        // A block with no rectangle never had border nodes, which means dagre never saw a child
        // under it. `Tree::build` already drops those; this is the guard that keeps a frame with
        // no coordinates from being drawn at the origin if one ever gets through.
        let (Some(x), Some(y)) = (n.x, n.y) else {
            continue;
        };
        out.push(PlacedCluster {
            id: c.id.clone(),
            title: Label::measure(&c.title),
            center: Point::new(x, y),
            size: Size::new(n.width, n.height),
            parent: c.parent.clone(),
            depth: c.depth,
            dashed: c.dashed,
            filled: true,
            sections: Vec::new(),
            // `state::lay_out` flips this on afterward, over every cluster at once, when its own
            // `routing` is `Routing::Orthogonal` (§10-5 S2) — `read_clusters` itself serves both a
            // flowchart's subgraphs and a state diagram's composite states, and has no way to tell
            // which language it is drawing for.
            title_strip: false,
        });
    }
    let placed: HashSet<String> = out.iter().map(|c| c.id.clone()).collect();
    for c in &mut out {
        if c.parent.as_ref().is_some_and(|p| !placed.contains(p)) {
            c.parent = None;
        }
    }
    // Outermost first, so drawing them in order puts a nested frame on top of the one that holds
    // it. `sort_by_key` is stable, so blocks at the same depth keep the parser's order.
    out.sort_by_key(|c| c.depth);
    match routing {
        Routing::Splines => fit_titles(&mut out, tree, nodes),
        Routing::Orthogonal => rebuild_frames(&mut out, tree, nodes),
    }
    out
}

/// §10-5 round 4: **a frame is its members' bounding box, padded** — never dagre's own border-node
/// rectangle, which stops describing anything real the moment a post-dagre pass moves a member.
///
/// * every side gets [`clusters::PAD`] (§10-1 item 4's "枠とノード…の余白は最低 16px");
/// * a titled block gets the title's own band on top of that at the top (§10-5 S2's title strip:
///   `svg::emit_cluster` draws the strip down to exactly `title.height + 2 * TITLE_PAD_Y` below the
///   frame's top edge, and the interior starts under it), which is the same band [`fit_titles`]
///   grows for on the splines path;
/// * and the box is widened *symmetrically* if the title is wider than it, so the frame's own
///   centre — the point [`orthogonal::classify`] measures a cluster-anchored edge's alignment
///   against, and [`orthogonal::evict`] hands out ports around — stays exactly where the members
///   put it. That is what makes "a lane runs dead straight from a node, through a frame's port,
///   into the block's own internal spine" expressible at all: with the members centred on one
///   cross coordinate, so is the frame.
///
/// Deepest-first, so a nested block is already final when the block holding it measures itself, and
/// a parent's box is derived from that final child rectangle rather than from the child's members
/// again — which is what makes the nesting gap exactly one [`clusters::PAD`] per level.
///
/// A block whose members this pass cannot find at all (none placed) keeps whatever rectangle
/// `read_clusters` read: there is nothing to derive from, and dropping the frame outright would
/// silently lose a box the source asked for.
/// §10-5 S4's own bar geometry, written onto the node vector [`Diagram::nodes`] is built from:
/// [`orthogonal::route_flowchart`] computes every fork/join bar's corrected, port-straddling
/// rectangle against its own *local* copy of `nodes` (its own doc on `bar_geometry` — the routing
/// maths inside it never touches the caller's), so without this the drawn bar would stay at dagre's
/// own, possibly port-mismatched, rectangle even though every polyline already meets the corrected
/// one.
///
/// A free function rather than an inline loop so the "what a bar's drawn box actually is" question
/// has one named answer next to [`rebuild_frames`], which is what derives a frame from that very
/// box when the bar is a block member.
fn apply_bar_geometry(nodes: &mut [PlacedNode], geometry: &HashMap<String, (Point, Size)>) {
    for node in nodes {
        if let Some((center, size)) = geometry.get(&node.id) {
            node.center = center.clone();
            node.size = *size;
        }
    }
}

fn rebuild_frames(clusters: &mut [PlacedCluster], tree: &clusters::Tree, nodes: &[PlacedNode]) {
    let mut order: Vec<usize> = (0..clusters.len()).collect();
    order.sort_by(|a, b| clusters[*b].depth.cmp(&clusters[*a].depth));

    for i in order {
        let id = clusters[i].id.clone();
        let mut rect: Option<clusters::Rect> = None;
        let hold = |r: clusters::Rect, rect: &mut Option<clusters::Rect>| match rect {
            Some(cur) => cur.absorb(&r),
            None => *rect = Some(r),
        };
        for c in clusters.iter() {
            if c.parent.as_deref() == Some(id.as_str()) {
                hold(clusters::Rect::new(&c.center, c.size), &mut rect);
            }
        }
        if let Some(block) = tree.get(&id) {
            for m in &block.member_nodes {
                if let Some(n) = nodes.iter().find(|n| &n.id == m) {
                    let (l, t, r, b) = n.bounds();
                    hold(
                        clusters::Rect {
                            left: l,
                            top: t,
                            right: r,
                            bottom: b,
                        },
                        &mut rect,
                    );
                }
            }
        }
        let Some(mut rect) = rect else { continue };
        rect.left -= clusters::PAD;
        rect.right += clusters::PAD;
        rect.top -= clusters::PAD;
        rect.bottom += clusters::PAD;

        let title = clusters[i].title.clone();
        if !title.is_blank() {
            rect.top -= title.height + clusters::TITLE_PAD_Y * 2.0;
            let short = (title.width + clusters::TITLE_PAD_X) - rect.size().w;
            if short > 0.0 {
                rect.left -= short / 2.0;
                rect.right += short / 2.0;
            }
        }
        clusters[i].center = rect.center();
        clusters[i].size = rect.size();
    }
}

/// The growing half of [`read_clusters`] under `Routing::Splines`.
fn fit_titles(clusters: &mut [PlacedCluster], tree: &clusters::Tree, nodes: &[PlacedNode]) {
    let mut order: Vec<usize> = (0..clusters.len()).collect();
    order.sort_by(|a, b| clusters[*b].depth.cmp(&clusters[*a].depth));

    for i in order {
        let id = clusters[i].id.clone();
        let mut rect = clusters::Rect::new(&clusters[i].center, clusters[i].size);

        // 1. Hold every direct child, at whatever size it ended up.
        let mut top_child = f64::INFINITY;
        for c in clusters.iter() {
            if c.parent.as_deref() == Some(id.as_str()) {
                let child = clusters::Rect::new(&c.center, c.size);
                rect.absorb(&child);
                top_child = top_child.min(child.top);
            }
        }
        if let Some(block) = tree.get(&id) {
            for m in &block.member_nodes {
                if let Some(n) = nodes.iter().find(|n| &n.id == m) {
                    let (l, t, r, b) = n.bounds();
                    rect.absorb(&clusters::Rect {
                        left: l,
                        top: t,
                        right: r,
                        bottom: b,
                    });
                    top_child = top_child.min(t);
                }
            }
        }

        let title = clusters[i].title.clone();
        if !title.is_blank() {
            // 2. Wide enough for the title.
            let need = title.width + clusters::TITLE_PAD_X;
            let short = need - rect.size().w;
            if short > 0.0 {
                rect.left -= short / 2.0;
                rect.right += short / 2.0;
            }
            // 3. Tall enough above the topmost member for the title to sit clear of it.
            let need = title.height + clusters::TITLE_PAD_Y * 2.0;
            if top_child.is_finite() {
                let short = need - (top_child - rect.top);
                if short > 0.0 {
                    rect.top -= short;
                }
            }
        }

        clusters[i].center = rect.center();
        clusters[i].size = rect.size();
    }
}

/// Padding between an edge label's text and the edge of the patch drawn behind it.
pub const LABEL_PAD_X: f64 = 4.0;

/// Vertical counterpart of [`LABEL_PAD_X`].
pub const LABEL_PAD_Y: f64 = 1.0;

/// Shifts everything so the drawing starts at [`MARGIN`], and records the overall size.
///
/// dagre already did this once, but only for the geometry it knew about: node boxes and the label
/// positions *it* chose. Clipping moved the line ends and the labels moved to the middle of their
/// lines, so the extent has to be recomputed over what is actually going to be drawn — otherwise
/// a label near the edge of the diagram gets cropped by the viewBox.
fn normalise(diagram: &mut Diagram) {
    let (mut min_x, mut min_y) = (f64::INFINITY, f64::INFINITY);
    let (mut max_x, mut max_y) = (f64::NEG_INFINITY, f64::NEG_INFINITY);
    let mut grow = |x0: f64, y0: f64, x1: f64, y1: f64| {
        min_x = min_x.min(x0);
        min_y = min_y.min(y0);
        max_x = max_x.max(x1);
        max_y = max_y.max(y1);
    };

    for n in &diagram.nodes {
        let (l, t, r, b) = n.bounds();
        grow(l, t, r, b);
    }
    // A frame is always at least as big as its members, but it can be *wider* than all of them
    // once it has grown to hold its title, so it has to be measured rather than assumed covered.
    for c in &diagram.clusters {
        let (l, t, r, b) = c.bounds();
        grow(l, t, r, b);
    }
    // A lifeline is the only geometry that is neither a node nor an edge, so it has to be
    // measured explicitly or a diagram whose bars stick out to the right gets cropped.
    for l in &diagram.lifelines {
        let (left, right) = l.extent();
        grow(left, l.top, right, l.bottom);
    }
    for e in &diagram.edges {
        for p in e.drawn_points() {
            grow(p.x, p.y, p.x, p.y);
        }
        for l in [&e.label, &e.start_label, &e.end_label, &e.badge]
            .into_iter()
            .flatten()
        {
            grow(
                l.center.x - l.size.w / 2.0,
                l.center.y - l.size.h / 2.0,
                l.center.x + l.size.w / 2.0,
                l.center.y + l.size.h / 2.0,
            );
        }
    }
    if !min_x.is_finite() {
        return;
    }

    let (dx, dy) = (MARGIN - min_x, MARGIN - min_y);
    for n in &mut diagram.nodes {
        n.center = Point::new(n.center.x + dx, n.center.y + dy);
    }
    for c in &mut diagram.clusters {
        c.center = Point::new(c.center.x + dx, c.center.y + dy);
        for section in &mut c.sections {
            section.y += dy;
        }
    }
    for l in &mut diagram.lifelines {
        l.x += dx;
        l.top += dy;
        l.bottom += dy;
        for a in &mut l.activations {
            a.top += dy;
            a.bottom += dy;
        }
    }
    for e in &mut diagram.edges {
        for p in &mut e.points {
            *p = Point::new(p.x + dx, p.y + dy);
        }
        // `gaps` is computed in the same pre-normalise coordinate space `points` starts this loop
        // in (`orthogonal::insert_crossing_gaps` reads straight from `route_flowchart`'s own
        // output) — missing this shift left every gap pointing at the old, un-translated location
        // once `points` moved out from under it, found by dumping a real crossing and comparing
        // the gap's own coordinates against the segment it was supposed to sit on.
        for (a, b) in &mut e.gaps {
            *a = Point::new(a.x + dx, a.y + dy);
            *b = Point::new(b.x + dx, b.y + dy);
        }
        for l in [
            &mut e.label,
            &mut e.start_label,
            &mut e.end_label,
            &mut e.badge,
        ]
        .into_iter()
        .flatten()
        {
            l.center = Point::new(l.center.x + dx, l.center.y + dy);
        }
    }
    diagram.width = (max_x - min_x) + MARGIN * 2.0;
    diagram.height = (max_y - min_y) + MARGIN * 2.0;
}

/// mermaid's direction keyword as dagre's `rankdir`.
fn rank_dir(direction: Direction) -> RankDir {
    match direction {
        Direction::TopToBottom => RankDir::TB,
        Direction::BottomToTop => RankDir::BT,
        Direction::LeftToRight => RankDir::LR,
        Direction::RightToLeft => RankDir::RL,
    }
}
