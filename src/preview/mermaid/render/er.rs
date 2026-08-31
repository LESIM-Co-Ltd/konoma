//! Drawing a parsed ER diagram — the other half of stage 3.
//!
//! ```text
//! ErDiagram ──▶ measure ──▶ build the panels ──▶ GraphSpec ──▶ (stage 1's layout) ──▶ SVG
//! ```
//!
//! Like [`super::class`], this is a translation and not a layout: an entity is a box, a
//! relationship is a line, a `subgraph` is a frame, and [`super::lay_out_spec`] already knows how
//! to place all three. What is specific to ER is the *ends of the lines*, and that is the one
//! thing in this file worth reading twice.
//!
//! # Which end a crow's foot goes on
//!
//! `CUSTOMER ||--o{ ORDER` means "one customer places zero or more orders", and the spelling says
//! where each mark is drawn: the character nearest an entity is the mark nearest that entity. So
//! `||` is at CUSTOMER, and `{` — the foot, opening onto ORDER — is at ORDER. `}o--` reads the
//! same way mirrored: the foot opens leftward onto the entity on the left, with the ring behind
//! it.
//!
//! Getting this backwards inverts the meaning of every relationship in the diagram while leaving
//! it looking perfectly plausible, which is exactly the kind of defect §6 asks to be pinned by a
//! test rather than by care. [`super::er_tests::a_crows_foot_is_drawn_at_the_end_that_asked_for_it`]
//! is that test, and the mutation that swaps the two ends is what proved it can fail.
//!
//! mermaid's own marker for "many" is a pointed lens rather than a crow's foot
//! (`markers.js`: `M0,18 Q18,0 36,18 Q18,36 0,18`). konoma draws the three-pronged foot the
//! notation is named after, which §0-1 allows and which is what a reader of an ER diagram is
//! looking for.

use crate::preview::mermaid::er::{self, Cardinality, ErDiagram, Identification};
use crate::preview::mermaid::flowchart::Stroke;
use crate::preview::mermaid::text_metrics;

use super::edges::Tip;
use super::panel::{self, AttributeRow};
use super::shapes::{self, Glyph};
use super::svg;
use super::{
    lay_out_spec, Curve, Diagram, GraphSpec, Label, RenderError, SpecBlock, SpecEdge, SpecNode,
    Theme,
};

/// Reads a mermaid ER source and draws it.
///
/// **This is the entry point the golden tests go through** (§6).
pub fn render(code: &str, theme: &str) -> Result<String, RenderError> {
    let diagram = er::parse(code)?;
    let laid = lay_out(&diagram)?;
    Ok(svg::emit(&laid, &Theme::named(theme)))
}

/// Measures, sizes, lays out and routes a parsed ER diagram.
pub fn lay_out(diagram: &ErDiagram) -> Result<Diagram, RenderError> {
    if !text_metrics::fonts_available() {
        return Err(RenderError::NoFonts);
    }
    if diagram.entities.is_empty() {
        return Err(RenderError::NothingToDraw);
    }
    lay_out_spec(&spec_of(diagram))
}

/// The whole of what is specific to the ER language.
pub fn spec_of(diagram: &ErDiagram) -> GraphSpec {
    // `classDef default` → the classes an entity carries, in applied order → its own `style` —
    // the same cascade the flowchart's `spec_of` builds; see `render::style::cascade`'s docs.
    let class_of = |name: &str| {
        diagram
            .class_defs
            .iter()
            .find(|d| d.name == name)
            .map(|d| d.styles.as_slice())
    };
    let nodes = diagram
        .entities
        .iter()
        .map(|e| {
            let attributes: Vec<AttributeRow> = e
                .attributes
                .iter()
                .map(|a| AttributeRow {
                    kind: Label::measure(&a.kind),
                    name: Label::measure(&a.name),
                    keys: Label::measure(&a.keys.join(", ")),
                    comment: Label::measure(&a.comment),
                })
                .collect();
            let panel = panel::er_panel(&Label::measure(&e.label), &attributes);
            SpecNode {
                id: e.id.clone(),
                glyph: Glyph::ErBox,
                label: Label::measure(""),
                size: shapes::size(Glyph::ErBox, panel.size),
                panel: Some(panel),
                style: super::style::cascade(class_of, &e.css_classes, &e.own_styles),
            }
        })
        .collect();

    let blocks = diagram
        .subgraphs
        .iter()
        .map(|s| SpecBlock {
            id: s.id.clone(),
            title: s.title.clone(),
            members: s.members.clone(),
            dashed: false,
        })
        .collect();

    let edges = diagram
        .relationships
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
            tip_start: tip_of(r.from_cardinality),
            tip_end: tip_of(r.to_cardinality),
            stroke: match r.identification {
                Identification::Identifying => Stroke::Normal,
                Identification::NonIdentifying => Stroke::Dotted,
            },
            minlen: 1,
            start_label: None,
            end_label: None,
            style: None,
            curve: Curve::Basis,
        })
        .collect();

    GraphSpec {
        direction: diagram.direction,
        nodes,
        edges,
        blocks,
        // Never `Routing::Orthogonal`: only the flowchart's own `spec_of` ever sets that.
        ..GraphSpec::default()
    }
}

/// What each cardinality is drawn as.
///
/// `MD_PARENT` has no documented meaning and no marker of its own upstream, so it is parsed —
/// which is what keeps its `u` out of the entity's name — and drawn as a plain end.
fn tip_of(cardinality: Cardinality) -> Tip {
    match cardinality {
        Cardinality::OnlyOne => Tip::ErOnlyOne,
        Cardinality::ZeroOrOne => Tip::ErZeroOrOne,
        Cardinality::ZeroOrMore => Tip::ErZeroOrMore,
        Cardinality::OneOrMore => Tip::ErOneOrMore,
        Cardinality::MdParent => Tip::None,
    }
}
