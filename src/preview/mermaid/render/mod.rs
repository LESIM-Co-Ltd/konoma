//! Drawing a parsed flowchart — stage 1c of konoma's own mermaid renderer.
//!
//! Stage 1b produced a [`Flowchart`]: what the author wrote. This module turns that into a
//! picture, and it is the first stage whose output a person can look at.
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
//! # Two things this stage does not do
//!
//! * **Subgraphs.** Stage 1d draws the frames. Here a `subgraph` block is invisible and its
//!   members are ordinary nodes, which is the honest degradation: every node and every edge the
//!   author wrote is still on the page, just without the box around the group. The one thing that
//!   cannot survive is an edge that *ends on a subgraph* — the parser removes such an id from the
//!   node list (mermaid does too), so there is no shape to attach the line to and the edge is
//!   dropped rather than pointed at a phantom.
//! * **Feeding `preview::markdown::mermaid_to_svg`.** Stage 1e changes that one function. Until
//!   then every diagram konoma actually shows still comes from `mermaid-rs-renderer`, and the
//!   golden files that pin the markdown renderer's output cannot move.

// Stage 1c is not on the drawing path yet — `mermaid_to_svg` is swapped in stage 1e — and konoma
// is a binary crate, so `pub` marks nothing as used. Same reasoning, and the same lint, as the
// sibling `flowchart` and `layout` modules.
#![allow(dead_code)]

pub mod edges;
pub mod labels;
pub mod shapes;
pub mod svg;
pub mod theme;

#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::fmt;

use crate::preview::mermaid::flowchart::{
    self, Arrow, Direction, Flowchart, ParseError, Shape, Stroke,
};
use crate::preview::mermaid::layout::{
    graph::{Graph, GraphOptions},
    layout, EdgeLabel, LayoutOptions, NodeLabel, Point, RankDir,
};
use crate::preview::mermaid::text_metrics;

pub use labels::Label;
pub use shapes::Size;
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
    /// No sans-serif font could be resolved, so usvg would draw the boxes and silently drop every
    /// glyph inside them. Checked *before* an SVG is built, because there is no error later.
    NoFonts,
    /// Every node was dropped (only reachable if a chart's whole node list names subgraphs).
    NothingToDraw,
}

impl fmt::Display for RenderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RenderError::Parse(e) => write!(f, "{e}"),
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

/// One node, placed.
#[derive(Debug, Clone, PartialEq)]
pub struct PlacedNode {
    /// The id from the source.
    pub id: String,
    /// The outline to draw.
    pub shape: Shape,
    /// Centre of the bounding box.
    pub center: Point,
    /// Bounding box, as [`shapes::size`] computed it and as dagre laid it out.
    pub size: Size,
    /// The measured label drawn inside.
    pub label: Label,
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
    /// Arrow heads.
    pub arrow: Arrow,
    /// Line style.
    pub stroke: Stroke,
    /// The label, if the edge carries one.
    pub label: Option<PlacedEdgeLabel>,
}

impl PlacedEdge {
    /// The polyline the curve is actually built from: corners rounded off (`edges::fix_corners`).
    pub fn drawn_points(&self) -> Vec<Point> {
        edges::fix_corners(&self.points)
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
}

impl Diagram {
    /// Looks a placed node up by id.
    pub fn node(&self, id: &str) -> Option<&PlacedNode> {
        self.nodes.iter().find(|n| n.id == id)
    }
}

/// Reads a mermaid flowchart source and draws it.
///
/// **This is the entry point the golden tests go through**, so that what they pin is what a caller
/// gets — `docs/FEATURE-MERMAID-RENDERER.md` §6 ("ゴールデンは本番の入口関数を通すこと"), after
/// konoma was once caught pinning a function that was not the one in production.
///
/// `theme` is `ui.mermaid_theme`'s raw string; an unknown value silently means `dark`.
pub fn render(code: &str, theme: &str) -> Result<String, RenderError> {
    let chart = flowchart::parse(code)?;
    let diagram = lay_out(&chart)?;
    Ok(svg::emit(&diagram, &Theme::named(theme)))
}

/// Measures, sizes, lays out and routes — everything except turning geometry into markup.
pub fn lay_out(chart: &Flowchart) -> Result<Diagram, RenderError> {
    // The gate. usvg does not fail on a missing font, it just drops the glyphs, so the only place
    // this can be caught is before anything is built (PRD design principle #3).
    if !text_metrics::fonts_available() {
        return Err(RenderError::NoFonts);
    }
    if chart.nodes.is_empty() {
        return Err(RenderError::NothingToDraw);
    }

    let mut g: Graph<NodeLabel, EdgeLabel> = Graph::with_options(GraphOptions {
        directed: true,
        multigraph: true,
        compound: false,
    });

    // --- nodes: measure the label, then size the shape around it -------------------------------
    let mut measured: HashMap<&str, (Shape, Size, Label)> = HashMap::new();
    for node in &chart.nodes {
        let label = Label::measure(&node.label);
        let size = shapes::size(node.shape, Size::new(label.width, label.height));
        g.set_node(
            node.id.clone(),
            Some(NodeLabel {
                width: size.w,
                height: size.h,
                ..NodeLabel::default()
            }),
        );
        measured.insert(node.id.as_str(), (node.shape, size, label));
    }

    // --- edges: hand dagre the label's size so it reserves a rank for it ------------------------
    //
    // This is the half of "the label stays on the line" that happens *before* layout: dagre's
    // `makeSpaceForEdgeLabels` doubles every `minlen` and injects a proxy node for any edge whose
    // label has a non-zero width and height, so the label gets a rank of its own to live in.
    let mut drawable: Vec<(&flowchart::Edge, Option<Label>)> = Vec::new();
    for edge in &chart.edges {
        if !measured.contains_key(edge.from.as_str()) || !measured.contains_key(edge.to.as_str()) {
            // An endpoint that names a subgraph rather than a node. Stage 1d.
            continue;
        }
        let label = edge
            .label
            .as_deref()
            .map(Label::measure)
            .filter(|l| !l.is_blank());
        let (w, h) = label.as_ref().map_or((0.0, 0.0), |l| (l.width, l.height));
        g.set_edge(
            edge.from.clone(),
            edge.to.clone(),
            Some(EdgeLabel {
                width: w,
                height: h,
                minlen: edge.length.max(1) as i32,
                ..EdgeLabel::default()
            }),
            Some(edge.id.as_str()),
        );
        drawable.push((edge, label));
    }

    layout(
        &mut g,
        Some(LayoutOptions {
            rankdir: rank_dir(chart.direction),
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
    let mut nodes: Vec<PlacedNode> = Vec::with_capacity(chart.nodes.len());
    for node in &chart.nodes {
        let (shape, size, label) = match measured.remove(node.id.as_str()) {
            Some(v) => v,
            None => continue,
        };
        let placed = g.node(&node.id);
        let center = Point::new(
            placed.and_then(|n| n.x).unwrap_or(0.0),
            placed.and_then(|n| n.y).unwrap_or(0.0),
        );
        nodes.push(PlacedNode {
            id: node.id.clone(),
            shape,
            center,
            size,
            label,
        });
    }
    if nodes.is_empty() {
        return Err(RenderError::NothingToDraw);
    }
    let by_id: HashMap<&str, usize> = nodes
        .iter()
        .enumerate()
        .map(|(i, n)| (n.id.as_str(), i))
        .collect();

    // --- clip each edge to the real outlines, then put its label on the clipped line -------------
    let mut placed_edges: Vec<PlacedEdge> = Vec::with_capacity(drawable.len());
    for (edge, label) in drawable {
        let raw = g
            .edge(&edge.from, &edge.to, Some(edge.id.as_str()))
            .map(|l| l.points.clone())
            .unwrap_or_default();
        let (Some(&ti), Some(&hi)) = (by_id.get(edge.from.as_str()), by_id.get(edge.to.as_str()))
        else {
            continue;
        };
        let tail = &nodes[ti];
        let head = &nodes[hi];
        let points = edges::clip(
            &raw,
            (tail.shape, tail.center.clone(), tail.size),
            (head.shape, head.center.clone(), head.size),
        );

        // The second half of "the label stays on the line": put it at the half-way point of the
        // line as clipped, not where dagre parked it.
        let placed_label = label.and_then(|l| {
            edges::arc_midpoint(&points).map(|center| PlacedEdgeLabel {
                center,
                size: Size::new(l.width + LABEL_PAD_X * 2.0, l.height + LABEL_PAD_Y * 2.0),
                label: l,
            })
        });

        placed_edges.push(PlacedEdge {
            from: edge.from.clone(),
            to: edge.to.clone(),
            points,
            arrow: edge.arrow,
            stroke: edge.stroke,
            label: placed_label,
        });
    }

    let mut diagram = Diagram {
        width: 0.0,
        height: 0.0,
        nodes,
        edges: placed_edges,
    };
    normalise(&mut diagram);
    Ok(diagram)
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
    for e in &diagram.edges {
        for p in e.drawn_points() {
            grow(p.x, p.y, p.x, p.y);
        }
        if let Some(l) = &e.label {
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
    for e in &mut diagram.edges {
        for p in &mut e.points {
            *p = Point::new(p.x + dx, p.y + dy);
        }
        if let Some(l) = &mut e.label {
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
