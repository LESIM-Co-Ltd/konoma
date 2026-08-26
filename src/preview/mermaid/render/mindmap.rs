//! Drawing a `mindmap`.
//!
//! # It is a tree, so it goes through `lay_out_spec`
//!
//! Every node has exactly one parent and there are no cycles, which is the easiest thing a
//! layered layout ever gets asked. So this module is a translation into [`GraphSpec`] and nothing
//! else — the same shape stage 2 gave the state diagram.
//!
//! # Left to right, not radially
//!
//! mermaid draws a mindmap around its root, with branches going both ways. konoma draws it left
//! to right, and that is a decision rather than a shortcut (§0-1 — konoma decides its own
//! legibility):
//!
//! * a radial layout puts half the labels to the *left* of their parent, so the text runs away
//!   from the line that connects it. At a terminal's scale that is the difference between a
//!   readable tree and a wheel of words;
//! * a mindmap's depth is what a reader follows, and left-to-right is the direction depth already
//!   runs in every other diagram konoma draws.
//!
//! # The line has no arrowhead
//!
//! A mindmap edge is containment, not flow. Drawing an arrow on it would say the parent *goes to*
//! the child, which is a different diagram.

use crate::preview::mermaid::flowchart::{Direction, Stroke};
use crate::preview::mermaid::mindmap::{Mindmap, NodeShape};
use crate::preview::mermaid::text_metrics;

use super::{
    lay_out_spec, shapes, svg, Diagram, Glyph, GraphSpec, Label, RenderError, SpecEdge, SpecNode,
    Theme, Tip,
};

/// Reads a mermaid mindmap source and draws it.
///
/// **This is the entry point the golden tests go through** (§6).
pub fn render(code: &str, theme: &str) -> Result<String, RenderError> {
    let map = crate::preview::mermaid::mindmap::parse(code)?;
    let laid = lay_out(&map)?;
    Ok(svg::emit(&laid, &Theme::named(theme)))
}

/// Measures, sizes, lays out and routes a parsed mindmap.
pub fn lay_out(map: &Mindmap) -> Result<Diagram, RenderError> {
    if !text_metrics::fonts_available() {
        return Err(RenderError::NoFonts);
    }
    if map.nodes.is_empty() {
        return Err(RenderError::NothingToDraw);
    }
    lay_out_spec(&spec_of(map))
}

/// The whole of what is specific to the mindmap language.
pub fn spec_of(map: &Mindmap) -> GraphSpec {
    // A node's id is whatever the author wrote, and two nodes may legitimately share it — the
    // same word twice in different branches. The layout keys on the id, so the position in the
    // node list is what identifies a node here.
    let ids: Vec<String> = (0..map.nodes.len()).map(|i| format!("n{i}")).collect();

    let nodes: Vec<SpecNode> = map
        .nodes
        .iter()
        .enumerate()
        .map(|(i, node)| {
            let label = Label::measure(&node.label);
            let glyph = glyph_of(node.shape);
            let size = shapes::size(glyph, super::Size::new(label.width, label.height));
            SpecNode {
                id: ids[i].clone(),
                glyph,
                label,
                size,
                panel: None,
            }
        })
        .collect();

    let edges: Vec<SpecEdge> = map
        .nodes
        .iter()
        .enumerate()
        .filter_map(|(i, node)| {
            let parent = node.parent?;
            Some(SpecEdge {
                id: format!("e{i}"),
                from: ids[parent].clone(),
                to: ids[i].clone(),
                label: None,
                tip_start: Tip::None,
                tip_end: Tip::None,
                stroke: Stroke::Normal,
                minlen: 1,
                start_label: None,
                end_label: None,
            })
        })
        .collect();

    GraphSpec {
        direction: Direction::LeftToRight,
        nodes,
        edges,
        blocks: Vec::new(),
    }
}

/// `mindmapDb`'s seven node types, as the seven things konoma draws.
fn glyph_of(shape: NodeShape) -> Glyph {
    use crate::preview::mermaid::flowchart::Shape;
    match shape {
        NodeShape::NoBorder => Glyph::Underline,
        NodeShape::RoundedRect => Glyph::Flow(Shape::RoundedRect),
        NodeShape::Rect => Glyph::Flow(Shape::Rect),
        NodeShape::Circle => Glyph::Flow(Shape::Circle),
        NodeShape::Cloud => Glyph::Cloud,
        NodeShape::Bang => Glyph::Bang,
        NodeShape::Hexagon => Glyph::Flow(Shape::Hexagon),
    }
}
