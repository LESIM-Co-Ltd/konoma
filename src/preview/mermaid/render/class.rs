//! Drawing a parsed class diagram — half of stage 3 of konoma's own mermaid renderer.
//!
//! ```text
//! ClassDiagram ──▶ measure ──▶ build the panels ──▶ GraphSpec ──▶ (stage 1's layout) ──▶ SVG
//! ```
//!
//! # What this module is, and what it is not
//!
//! It is **not** a layout. A class diagram is the same kind of picture a flowchart is — boxes,
//! lines and frames — so the whole of [`super::lay_out_spec`] is reused unchanged: ranking,
//! ordering, coordinates, the compound frames, the clipping, the label placement, the viewBox.
//! Everything this file adds is the translation from one language's model to the
//! language-neutral [`GraphSpec`], and that is checkable: [`super::class_tests`] runs the same
//! geometric invariants over class sources that [`super::tests`] runs over flowchart sources,
//! because they are properties of the same [`Diagram`].
//!
//! # The three decisions this file does make
//!
//! 1. **A class is a box with compartments, and the compartments are geometry.** The annotations
//!    and the class name go in the top one, the attributes in the next, the methods in the last;
//!    [`super::panel`] works out where every row sits and how wide the box has to be, and hands
//!    back a [`Panel`] that [`super::svg`] only translates. Sizing a class box from one label
//!    would make every box the width of its longest line with the lines centred, which is not a
//!    UML class box — it is a flowchart node with newlines in it.
//! 2. **A `namespace` is a frame.** It is the only thing here that contains other things, and
//!    stage 1d already solved that: `set_parent` hands the nesting to dagre and the rectangle is
//!    read back out of it. A dotted `namespace A.B` is a frame inside a frame, because the parser
//!    resolved the chain.
//! 3. **A lollipop is a mark on the line, not a node.** mermaid makes `bar ()-- foo` into an
//!    invisible `opacity: 0` box plus a real class, so the layout reserves room for something the
//!    reader cannot see. Drawing the ring on the line says the same thing and costs nothing.

use crate::preview::mermaid::class::{self, ClassDiagram, Line, Marker};
use crate::preview::mermaid::flowchart::Stroke;
use crate::preview::mermaid::text_metrics;

use super::edges::Tip;
use super::panel::{self, Compartment};
use super::shapes::{self, Glyph};
use super::svg;
use super::{
    lay_out_spec, Curve, Diagram, GraphSpec, Label, RenderError, SpecBlock, SpecEdge, SpecNode,
    Theme,
};

/// Reads a mermaid class-diagram source and draws it.
///
/// **This is the entry point the golden tests go through**, so that what they pin is what a
/// caller gets — `docs/FEATURE-MERMAID-RENDERER.md` §6, after konoma was once caught pinning a
/// function that was not the one in production.
///
/// `theme` is `ui.mermaid_theme`'s raw string; an unknown value silently means `dark`.
pub fn render(code: &str, theme: &str) -> Result<String, RenderError> {
    let diagram = class::parse(code)?;
    let laid = lay_out(&diagram)?;
    Ok(svg::emit(&laid, &Theme::named(theme)))
}

/// Measures, sizes, lays out and routes a parsed class diagram.
pub fn lay_out(diagram: &ClassDiagram) -> Result<Diagram, RenderError> {
    // The gate. usvg does not fail on a missing font, it just drops the glyphs, so the only place
    // this can be caught is before anything is built (PRD design principle #3).
    if !text_metrics::fonts_available() {
        return Err(RenderError::NoFonts);
    }
    if diagram.classes.is_empty() && diagram.notes.is_empty() {
        return Err(RenderError::NothingToDraw);
    }
    lay_out_spec(&spec_of(diagram))
}

/// The whole of what is specific to the class-diagram language.
pub fn spec_of(diagram: &ClassDiagram) -> GraphSpec {
    let mut nodes: Vec<SpecNode> = Vec::new();
    for c in &diagram.classes {
        let panel = panel::class_panel(&compartments_of(c));
        nodes.push(SpecNode {
            id: c.id.clone(),
            glyph: Glyph::ClassBox,
            // The text is all in the panel; a class box has no single centred label. Keeping the
            // name here as well would draw it twice.
            label: Label::measure(""),
            size: shapes::size(Glyph::ClassBox, panel.size),
            panel: Some(panel),
            style: None,
        });
    }
    // A note is an ordinary node here, unlike in a state diagram. There it had to be placed by
    // hand because `left of`/`right of` name a side the layout does not honour; a class note
    // names no side, so leaving it to dagre is both simpler and what upstream does.
    for n in &diagram.notes {
        let label = Label::measure(&n.text);
        let size = shapes::size(Glyph::Note, super::Size::new(label.width, label.height));
        nodes.push(SpecNode {
            id: n.id.clone(),
            glyph: Glyph::Note,
            label,
            size,
            panel: None,
            style: None,
        });
    }

    let blocks = diagram
        .namespaces
        .iter()
        .map(|n| SpecBlock {
            id: n.id.clone(),
            title: n.label.clone(),
            members: n.members.clone(),
            dashed: false,
        })
        .collect();

    let mut edges: Vec<SpecEdge> = diagram
        .relations
        .iter()
        .map(|r| SpecEdge {
            id: r.id.clone(),
            from: r.from.clone(),
            to: r.to.clone(),
            label: r
                .label
                .as_deref()
                .map(Label::measure)
                .filter(|l| !l.is_blank()),
            tip_start: tip_of(r.start),
            tip_end: tip_of(r.end),
            stroke: match r.line {
                Line::Solid => Stroke::Normal,
                Line::Dotted => Stroke::Dotted,
            },
            minlen: 1,
            start_label: r
                .start_cardinality
                .as_deref()
                .map(Label::measure)
                .filter(|l| !l.is_blank()),
            end_label: r
                .end_cardinality
                .as_deref()
                .map(Label::measure)
                .filter(|l| !l.is_blank()),
            style: None,
            curve: Curve::Basis,
        })
        .collect();

    // A `note for X` is tied to its class with a headless dotted line, which is what `getData`
    // emits (`arrowTypeStart: 'none'`, `pattern: 'dotted'`).
    for n in &diagram.notes {
        let Some(target) = &n.target else { continue };
        edges.push(SpecEdge {
            id: format!("edge{}", n.id),
            from: n.id.clone(),
            to: target.clone(),
            label: None,
            tip_start: Tip::None,
            tip_end: Tip::None,
            stroke: Stroke::Dotted,
            minlen: 1,
            start_label: None,
            end_label: None,
            style: None,
            curve: Curve::Basis,
        });
    }

    GraphSpec {
        direction: diagram.direction,
        nodes,
        edges,
        blocks,
        // Never `Routing::Orthogonal`: only the flowchart's own `spec_of` ever sets that.
        ..GraphSpec::default()
    }
}

/// The compartments of one class box.
///
/// Three of them when the class has anything to say, two when it has nothing: mermaid draws two
/// empty ones for a bare `class Duck` and an empty methods box for a class with only attributes,
/// which is space a terminal does not have to spare. The one rule under the name is what still
/// makes an empty class read as a class — see [`super::panel`].
fn compartments_of(c: &class::Class) -> Vec<Compartment> {
    let mut top: Vec<Label> = c
        .annotations
        .iter()
        .map(|a| Label::measure(&format!("«{a}»")))
        .collect();
    top.push(Label::measure(&c.label));
    let mut out = vec![Compartment {
        lines: top,
        centered: true,
    }];
    let attributes: Vec<Label> = c
        .attributes
        .iter()
        .map(|m| Label::measure(&m.display()))
        .collect();
    let methods: Vec<Label> = c
        .methods
        .iter()
        .map(|m| Label::measure(&m.display()))
        .collect();
    if attributes.is_empty() && methods.is_empty() {
        out.push(Compartment {
            lines: Vec::new(),
            centered: false,
        });
        return out;
    }
    if !attributes.is_empty() {
        out.push(Compartment {
            lines: attributes,
            centered: false,
        });
    }
    if !methods.is_empty() {
        out.push(Compartment {
            lines: methods,
            centered: false,
        });
    }
    out
}

/// What each relationship mark is drawn as.
fn tip_of(marker: Marker) -> Tip {
    match marker {
        Marker::None => Tip::None,
        Marker::Extension => Tip::HollowTriangle,
        Marker::Composition => Tip::FilledDiamond,
        Marker::Aggregation => Tip::HollowDiamond,
        // Association and dependency share a mark upstream too; the line style is what tells a
        // `-->` from a `..>`.
        Marker::Dependency => Tip::Arrow,
        Marker::Lollipop => Tip::Lollipop,
    }
}
