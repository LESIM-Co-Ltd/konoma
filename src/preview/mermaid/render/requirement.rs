//! Drawing a `requirementDiagram`.
//!
//! # A requirement is a compartmented box, so it reuses stage 3's [`Panel`]
//!
//! `<<Requirement>>` over the name over the four fields is a small table, which is the one thing
//! stage 3 added to the seam. Nothing new is needed here at all: [`panel::class_panel`] takes the
//! compartments and decides the box, [`lay_out_spec`] places it, and [`svg`] translates it.
//!
//! [`Panel`]: super::Panel
//! [`panel::class_panel`]: super::panel::class_panel
//!
//! # The defect this is measured against
//!
//! `docs/FEATURE-MERMAID-RENDERER.md` §6-A records the crate clipping the **top** of a
//! requirement box. That cannot happen here: the box's height is the panel's height, the panel's
//! height is the sum of its rows, and `normalise` sizes the viewBox from the boxes — so
//! [`super::tests::check_view_box_contains_everything`] and
//! [`super::tests::check_panels_stay_inside_their_box`] are the two statements that make it
//! structurally impossible rather than merely fixed.

use crate::preview::mermaid::flowchart::Stroke;
use crate::preview::mermaid::requirement::{Element, Relation, Requirement, RequirementDiagram};
use crate::preview::mermaid::text_metrics;

use super::panel::{class_panel, Compartment};
use super::{
    lay_out_spec, shapes, svg, Diagram, Glyph, GraphSpec, Label, RenderError, SpecEdge, SpecNode,
    Theme, Tip,
};

/// Reads a mermaid requirement diagram and draws it.
///
/// **This is the entry point the golden tests go through** (§6).
pub fn render(code: &str, theme: &str) -> Result<String, RenderError> {
    let diagram = crate::preview::mermaid::requirement::parse(code)?;
    let laid = lay_out(&diagram)?;
    Ok(svg::emit(&laid, &Theme::named(theme)))
}

/// Measures, sizes, lays out and routes a parsed requirement diagram.
pub fn lay_out(diagram: &RequirementDiagram) -> Result<Diagram, RenderError> {
    if !text_metrics::fonts_available() {
        return Err(RenderError::NoFonts);
    }
    if diagram.requirements.is_empty() && diagram.elements.is_empty() {
        return Err(RenderError::NothingToDraw);
    }
    lay_out_spec(&spec_of(diagram))
}

/// The whole of what is specific to the requirement language.
pub fn spec_of(diagram: &RequirementDiagram) -> GraphSpec {
    let mut nodes: Vec<SpecNode> = Vec::new();
    for r in &diagram.requirements {
        nodes.push(box_node(&r.name, &requirement_rows(r)));
    }
    for e in &diagram.elements {
        nodes.push(box_node(&e.name, &element_rows(e)));
    }

    let edges: Vec<SpecEdge> = diagram
        .links
        .iter()
        .enumerate()
        .map(|(i, link)| SpecEdge {
            id: format!("r{i}"),
            from: link.src.clone(),
            to: link.dst.clone(),
            label: Some(Label::measure(link.relation.word())),
            tip_start: Tip::None,
            tip_end: Tip::Arrow,
            // `contains` is the one structural relationship — it says the destination is *part
            // of* the source — and upstream draws it solid while every other verb is dashed.
            stroke: if link.relation == Relation::Contains {
                Stroke::Normal
            } else {
                Stroke::Dotted
            },
            minlen: 1,
            start_label: None,
            end_label: None,
        })
        .collect();

    GraphSpec {
        direction: diagram.direction,
        nodes,
        edges,
        blocks: Vec::new(),
    }
}

/// A box: the two centred compartments over the field rows.
fn box_node(name: &str, rows: &[(&'static str, String)]) -> SpecNode {
    let mut compartments = vec![Compartment {
        lines: vec![Label::measure(&format!("«{}»", rows[0].1))],
        centered: true,
    }];
    compartments.push(Compartment {
        lines: vec![Label::measure(name)],
        centered: true,
    });
    let fields: Vec<Label> = rows[1..]
        .iter()
        .filter(|(_, v)| !v.trim().is_empty())
        .map(|(k, v)| Label::measure(&format!("{k}: {v}")))
        .collect();
    // An empty compartment still gets its rule, the way a class with no members does: it says
    // "this requirement declares nothing", which is different from "it has no compartment".
    compartments.push(Compartment {
        lines: fields,
        centered: false,
    });
    let panel = class_panel(&compartments);
    SpecNode {
        id: name.to_string(),
        glyph: Glyph::ClassBox,
        label: Label::measure(""),
        size: shapes::size(Glyph::ClassBox, panel.size),
        panel: Some(panel),
    }
}

/// The rows of a requirement box. The first is the stereotype; the rest are its fields.
fn requirement_rows(r: &Requirement) -> Vec<(&'static str, String)> {
    vec![
        ("", r.kind.title().to_string()),
        ("Id", r.id.clone()),
        ("Text", r.text.clone()),
        ("Risk", r.risk.clone()),
        ("Verification", r.verify_method.clone()),
    ]
}

/// The rows of an element box.
fn element_rows(e: &Element) -> Vec<(&'static str, String)> {
    vec![
        ("", "Element".to_string()),
        ("Type", e.kind.clone()),
        ("Doc Ref", e.doc_ref.clone()),
    ]
}
