//! Drawing the five C4 diagrams.
//!
//! # A C4 element is a box with three lines in it
//!
//! The name, then `[Person]` / `[Container: Go]`, then the description. That is three lines of one
//! label rather than three compartments of a [`Panel`](super::Panel): they are read together as
//! one caption, and a rule between them would say they are separate things.
//!
//! # A boundary is a frame, so it is a `SpecBlock`
//!
//! Which means the six cluster invariants apply here for free — a boundary holds its members, a
//! nested boundary sits inside its parent, an unrelated boundary does not overlap. Those are
//! exactly the statements a C4 diagram exists to make, and none of them is written here.
//!
//! # `Rel_Up` and its siblings are read and not applied
//!
//! Upstream lets a relationship ask for a direction (`Rel_U`, `Rel_D`, `Rel_L`, `Rel_R`). dagre
//! has one `rankdir` for the whole graph, and the reasoning is [`super::lay_out_spec`]'s, in full:
//! honouring a per-edge direction means a second coordinate space that nothing checks. What the
//! reader keeps is every element, every boundary and every line the source declared.

use crate::preview::mermaid::c4::{Boundary, Element, ShapeKind, C4};
use crate::preview::mermaid::flowchart::{Shape, Stroke};
use crate::preview::mermaid::text_metrics;

use super::panel::{class_panel, Compartment};
use super::{
    lay_out_spec, shapes, svg, Curve, Diagram, Glyph, GraphSpec, Label, RenderError, Size,
    SpecBlock, SpecEdge, SpecNode, Theme, Tip,
};

/// Reads a mermaid C4 source and draws it.
///
/// **This is the entry point the golden tests go through** (§6).
pub fn render(code: &str, theme: &str) -> Result<String, RenderError> {
    let diagram = crate::preview::mermaid::c4::parse(code)?;
    let laid = lay_out(&diagram)?;
    Ok(svg::emit(&laid, &Theme::named(theme)))
}

/// Measures, sizes, lays out and routes a parsed C4 diagram.
pub fn lay_out(diagram: &C4) -> Result<Diagram, RenderError> {
    if !text_metrics::fonts_available() {
        return Err(RenderError::NoFonts);
    }
    if diagram.elements.is_empty() {
        // A diagram of nothing but empty boundaries has no content: `lay_out_spec` would drop
        // every frame (a block with no node in it never reaches dagre) and return a blank page.
        return Err(RenderError::NothingToDraw);
    }
    lay_out_spec(&spec_of(diagram))
}

/// The whole of what is specific to the C4 languages.
pub fn spec_of(diagram: &C4) -> GraphSpec {
    let nodes: Vec<SpecNode> = diagram.elements.iter().map(element_node).collect();

    let blocks: Vec<SpecBlock> = diagram
        .boundaries
        .iter()
        .map(|b| SpecBlock {
            id: b.alias.clone(),
            title: frame_title(b),
            members: b.members.clone(),
            // A C4 boundary is drawn dashed everywhere it is drawn: it is a line on a map, not a
            // thing that exists.
            dashed: true,
        })
        .collect();

    let edges: Vec<SpecEdge> = diagram
        .rels
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let text = if r.techn.trim().is_empty() {
                r.label.clone()
            } else {
                format!("{}\n[{}]", r.label, r.techn)
            };
            SpecEdge {
                id: format!("rel{i}"),
                from: r.from.clone(),
                to: r.to.clone(),
                label: Some(Label::measure(&text)).filter(|l| !l.is_blank()),
                tip_start: if r.bidirectional {
                    Tip::Arrow
                } else {
                    Tip::None
                },
                tip_end: Tip::Arrow,
                stroke: Stroke::Normal,
                minlen: 1,
                start_label: None,
                end_label: None,
                style: None,
                curve: Curve::Basis,
            }
        })
        .collect();

    GraphSpec {
        direction: diagram.direction,
        nodes,
        edges,
        blocks,
    }
}

/// One element's box.
fn element_node(e: &Element) -> SpecNode {
    let glyph = match e.shape {
        // A person is drawn as a stick figure over a name, exactly as a sequence `actor` is —
        // and it needs the same panel, because the figure's band and the words are two rows.
        ShapeKind::Person => Glyph::Actor,
        ShapeKind::Box => Glyph::Flow(Shape::Rect),
        ShapeKind::Database => Glyph::Flow(Shape::Cylinder),
        ShapeKind::Queue => Glyph::Flow(Shape::Stadium),
    };
    let lines = caption(e);
    if glyph == Glyph::Actor {
        let mut panel = class_panel(&[Compartment {
            lines: lines.iter().map(|l| Label::measure(l)).collect(),
            centered: true,
        }]);
        // The stick figure needs a band of its own above the words, and the panel has to *own*
        // it: `check_panels_stay_inside_their_box` states that a panel and its node are the same
        // box, so adding the band to the node and not to the panel would be two opinions about
        // one number. Same shape as `sequence::actor_panel`.
        for row in &mut panel.rows {
            row.y += shapes::ACTOR_FIGURE_HEIGHT;
        }
        panel
            .rules
            .iter_mut()
            .for_each(|r| *r += shapes::ACTOR_FIGURE_HEIGHT);
        panel.size = Size::new(panel.size.w, panel.size.h + shapes::ACTOR_FIGURE_HEIGHT);
        return SpecNode {
            id: e.alias.clone(),
            glyph,
            label: Label::measure(""),
            size: shapes::size(glyph, panel.size),
            panel: Some(panel),
            style: None,
        };
    }
    let label = Label::measure(&lines.join("\n"));
    SpecNode {
        id: e.alias.clone(),
        glyph,
        size: shapes::size(glyph, Size::new(label.width, label.height)),
        label,
        panel: None,
        style: None,
    }
}

/// The three lines inside an element's box, blank ones left out.
fn caption(e: &Element) -> Vec<String> {
    let mut out = vec![e.label.clone()];
    if !e.kind_line.trim().is_empty() {
        // The `_Ext` forms say so on the page, because greying the box out — mermaid's answer —
        // is a fill, and §1 forbids painting over the terminal.
        let external = if e.external { ", external" } else { "" };
        out.push(format!("[{}{external}]", e.kind_line));
    }
    if !e.descr.trim().is_empty() {
        out.push(e.descr.clone());
    }
    out
}

/// The words on a boundary's frame.
fn frame_title(b: &Boundary) -> String {
    if b.kind_line.trim().is_empty() {
        return b.label.clone();
    }
    format!("{}\n[{}]", b.label, b.kind_line)
}
