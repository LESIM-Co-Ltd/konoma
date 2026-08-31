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
    /// rather than as a plain "+" intersection with no depth of its own. `a` and `b` always lie on
    /// the same original segment of `points` (never spanning a corner) and are always exactly
    /// [`orthogonal::CROSSING_GAP`] px apart, centred on the crossing point that asked for them.
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
/// `docs/FEATURE-MERMAID-RENDERER.md` §10's right-angle wiring mode. Every other diagram kind
/// ignores `routing` entirely, the same as every kind but the flowchart ignores `curve`.
pub fn render_flow(
    code: &str,
    theme: &str,
    curve: &str,
    routing: &str,
) -> Result<String, RenderError> {
    let chart = flowchart::parse(code)?;
    let diagram = lay_out_flow(&chart, curve, routing)?;
    Ok(svg::emit(&diagram, &Theme::named(theme)))
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
            let label = Label::measure(&node.label);
            // A decision node under orthogonal routing draws as a chamfered rectangle rather than
            // a diamond — see `Glyph::ChamferedRect`'s own docs for why. Nothing else changes: the
            // parsed `Shape` konoma keeps for every other reader of `chart` (`docs`, `%%{init}%%`
            // round-tripping, …) is untouched, only the glyph this one node is drawn as.
            let glyph = if routing == Routing::Orthogonal && node.shape == Shape::Diamond {
                Glyph::ChamferedRect
            } else {
                Glyph::Flow(node.shape)
            };
            let size = shapes::size(glyph, Size::new(label.width, label.height));
            SpecNode {
                id: node.id.clone(),
                glyph,
                label,
                size,
                panel: None,
                style: style::cascade(class_of, &node.classes, &node.styles),
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
    /// `[ui] mermaid_routing`, resolved. `Routing::Splines` — every diagram kind's `spec_of` but
    /// the flowchart's own leaves this at its `#[default]` — reproduces every edge exactly as
    /// [`lay_out_spec`] always routed it; only a flowchart's own `spec_of` ever sets this to
    /// `Routing::Orthogonal`, and only when `[ui] mermaid_routing = "konoma-orthogonal"`.
    pub routing: Routing,
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
    // the expected path — reached only if some future change made growth requirements disagree
    // between passes, and even then the last pass's diagram (not a panic) is what a reader gets.
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
    let mut drawable: Vec<Drawable> = Vec::new();
    for edge in &spec.edges {
        let is_node = |id: &str| measured.contains_key(id);
        let (Some(tail), Some(head)) = (
            tree.anchor(&edge.from, &is_node),
            tree.anchor(&edge.to, &is_node),
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
                ..EdgeLabel::default()
            }),
            Some(edge.id.as_str()),
        );
        drawable.push(Drawable { edge, tail, head });
    }

    layout(
        &mut g,
        Some(LayoutOptions {
            rankdir: rank_dir(spec.direction),
            nodesep: NODE_SEP,
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
    // `alignment_deltas` is this pass's return: every node it actually moved, as its own
    // cross-axis delta. A self-loop's raw dagre waypoints (read from `g` below, `raw`'s own
    // comment) are the one thing alignment cannot correct itself — they live in `g`, which this
    // pass never touches — so whichever self-loop's owner appears in this map gets the same shift
    // applied to `raw` before `route_staircase_with_ports` ever sees it.
    let mut alignment_deltas: HashMap<String, f64> = HashMap::new();
    if spec.routing == Routing::Orthogonal {
        let node_rank: HashMap<String, i32> = nodes
            .iter()
            .filter_map(|n| {
                g.node(&n.id)
                    .and_then(|nl| nl.rank)
                    .map(|r| (n.id.clone(), r))
            })
            .collect();
        let candidates: Vec<(String, String)> = drawable
            .iter()
            .filter(|d| d.edge.from == d.tail && d.edge.to == d.head)
            .map(|d| (d.tail.clone(), d.head.clone()))
            .collect();
        alignment_deltas =
            orthogonal::align_straight_lanes(spec.direction, &mut nodes, &node_rank, &candidates);
    }

    let by_id: HashMap<&str, usize> = nodes
        .iter()
        .enumerate()
        .map(|(i, n)| (n.id.as_str(), i))
        .collect();

    // --- read the frames back, then grow them until their titles fit ----------------------------
    let placed_clusters = read_clusters(&g, &tree, &nodes);

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
    for Drawable {
        edge,
        tail: tail_id,
        head: head_id,
    } in drawable
    {
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
        let orthogonal::RoutedFlowchart {
            mut points,
            required_size,
        } = orthogonal::route_flowchart(spec.direction, &nodes, &placed_clusters, &eligible);
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
        tail_id: _,
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
                let slot = orthogonal::label_slot(spec.direction, &points)?;
                let size = Size::new(l.width + LABEL_PAD_X * 2.0, l.height + LABEL_PAD_Y * 2.0);
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
            style: edge.style.clone(),
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

/// Reads the frames dagre computed and grows each one until its title fits inside it.
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
/// Growing runs deepest-first so that a block which grew is final by the time its parent is
/// measured, and every parent starts by absorbing its children — which is what keeps a grown
/// block inside the frame that contains it instead of poking out of the top of it.
fn read_clusters(
    g: &Graph<NodeLabel, EdgeLabel>,
    tree: &clusters::Tree,
    nodes: &[PlacedNode],
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
    fit_titles(&mut out, tree, nodes);
    out
}

/// The growing half of [`read_clusters`].
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
