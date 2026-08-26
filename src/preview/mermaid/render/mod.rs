//! Drawing a parsed flowchart — stages 1c and 1d of konoma's own mermaid renderer.
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
//! # What this stage still does not do
//!
//! * **Feeding `preview::markdown::mermaid_to_svg`.** Stage 1e changes that one function. Until
//!   then every diagram konoma actually shows still comes from `mermaid-rs-renderer`, and the
//!   golden files that pin the markdown renderer's output cannot move.

// This module is not on the drawing path yet — `mermaid_to_svg` is swapped in stage 1e — and konoma
// is a binary crate, so `pub` marks nothing as used. Same reasoning, and the same lint, as the
// sibling `flowchart` and `layout` modules.
#![allow(dead_code)]

pub mod clusters;
pub mod edges;
pub mod labels;
pub mod shapes;
pub mod svg;
pub mod theme;

#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet};
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
}

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
/// `theme` is `ui.mermaid_theme`'s raw string; an unknown value silently means `dark`.
pub fn render(code: &str, theme: &str) -> Result<String, RenderError> {
    let chart = flowchart::parse(code)?;
    let diagram = lay_out(&chart)?;
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
    // The gate. usvg does not fail on a missing font, it just drops the glyphs, so the only place
    // this can be caught is before anything is built (PRD design principle #3).
    if !text_metrics::fonts_available() {
        return Err(RenderError::NoFonts);
    }
    if chart.nodes.is_empty() {
        return Err(RenderError::NothingToDraw);
    }

    // The subgraph tree, resolved before anything is measured: it decides which nodes get a
    // parent, and a block that holds no node at all never reaches dagre (`clusters::Tree::build`).
    let node_ids: HashSet<&str> = chart.nodes.iter().map(|n| n.id.as_str()).collect();
    let tree = clusters::Tree::build(chart, |id| node_ids.contains(id));

    let mut g: Graph<NodeLabel, EdgeLabel> = Graph::with_options(GraphOptions {
        directed: true,
        multigraph: true,
        // Only when there is something to nest. dagre runs its nesting graph either way, but a
        // compound graph reaches `parent_dummy_chains` and `add_border_segments`, and a chart with
        // no blocks has no reason to pay for a code path it cannot use.
        compound: !tree.is_empty(),
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
    for edge in &chart.edges {
        let is_node = |id: &str| measured.contains_key(id);
        let (Some(tail), Some(head)) = (
            tree.anchor(&edge.from, &is_node),
            tree.anchor(&edge.to, &is_node),
        ) else {
            // Neither a node nor a block that holds one: there is nothing to draw a line between.
            continue;
        };
        let (tail, head) = (tail.to_string(), head.to_string());
        let label = edge
            .label
            .as_deref()
            .map(Label::measure)
            .filter(|l| !l.is_blank());
        let (w, h) = label.as_ref().map_or((0.0, 0.0), |l| (l.width, l.height));
        g.set_edge(
            tail.clone(),
            head.clone(),
            Some(EdgeLabel {
                width: w,
                height: h,
                minlen: edge.length.max(1) as i32,
                ..EdgeLabel::default()
            }),
            Some(edge.id.as_str()),
        );
        drawable.push(Drawable {
            edge,
            label,
            tail,
            head,
        });
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

    // --- read the frames back, then grow them until their titles fit ----------------------------
    let placed_clusters = read_clusters(&g, &tree, &nodes);

    // --- clip each edge to the real outlines, then put its label on the clipped line -------------
    let mut placed_edges: Vec<PlacedEdge> = Vec::with_capacity(drawable.len());
    for Drawable {
        edge,
        label,
        tail: tail_id,
        head: head_id,
    } in drawable
    {
        let raw = g
            .edge(&tail_id, &head_id, Some(edge.id.as_str()))
            .map(|l| l.points.clone())
            .unwrap_or_default();
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
        let tail = end_of(&edge.from, &nodes[ti]);
        let head = end_of(&edge.to, &nodes[hi]);
        let points = edges::route(&raw, &tail, &head);

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
        clusters: placed_clusters,
    };
    normalise(&mut diagram);
    Ok(diagram)
}

/// One edge on its way to being drawn.
///
/// `tail`/`head` are the nodes dagre was actually asked to route between, which are the endpoints
/// the author wrote unless one of them named a block — see [`clusters::Tree::anchor`].
struct Drawable<'a> {
    edge: &'a flowchart::Edge,
    label: Option<Label>,
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
    for c in &mut diagram.clusters {
        c.center = Point::new(c.center.x + dx, c.center.y + dy);
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
