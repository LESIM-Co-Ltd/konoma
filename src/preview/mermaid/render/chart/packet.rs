//! Drawing a `packet-beta`.
//!
//! # A field's width is its bit count, and nothing else
//!
//! That is the whole diagram. A 32-bit field is twice as wide as a 16-bit one, and a row is
//! exactly [`BITS_PER_ROW`](crate::preview::mermaid::chart::packet::BITS_PER_ROW) bits across, so
//! two fields on different rows can be compared by eye. The crate konoma is replacing lays every
//! field out at the same width in one row joined by little connector lines, which is a flowchart
//! of a packet rather than a picture of one.
//!
//! # A field is labelled with its range and its name, and drops the name before the range
//!
//! When a field is too narrow for both, the bit range survives: a reader can find the name in the
//! source, but not the offset.

use crate::preview::mermaid::chart::packet::{Packet, BITS_PER_ROW};
use crate::preview::mermaid::layout::Point;

use super::super::labels::line_height;
use super::super::panel::{Panel, PanelCell, PanelRow};
use super::super::{normalise, Diagram, Label, PlacedNode, RenderError, Size, Theme};
use super::{add_title, bar_node};

/// Width of one bit.
pub const BIT_WIDTH: f64 = 15.0;

/// Height of one row.
pub const ROW_HEIGHT: f64 = 42.0;

/// Blank space between two rows.
pub const ROW_GAP: f64 = 6.0;

/// Blank space between a field's edge and the text in it.
pub const TEXT_PAD: f64 = 4.0;

/// Reads a mermaid packet source and draws it.
///
/// **This is the entry point the golden tests go through** (§6).
pub fn render(code: &str, theme: &str) -> Result<String, RenderError> {
    let packet = crate::preview::mermaid::chart::packet::parse(code)?;
    let diagram = lay_out(&packet)?;
    Ok(super::super::svg::emit(&diagram, &Theme::named(theme)))
}

/// Works out where every field goes.
pub fn lay_out(packet: &Packet) -> Result<Diagram, RenderError> {
    if !crate::preview::mermaid::text_metrics::fonts_available() {
        return Err(RenderError::NoFonts);
    }
    let mut nodes: Vec<PlacedNode> = Vec::new();
    // The series index runs over the *fields as written*, so the two halves of a field split
    // across a row boundary are the same colour — which is what says they are one field.
    let mut colour_of: Vec<(String, usize)> = Vec::new();

    for (r, row) in packet.rows.iter().enumerate() {
        let top = r as f64 * (ROW_HEIGHT + ROW_GAP);
        for field in row {
            let from = (field.start % BITS_PER_ROW) as f64;
            let w = field.bits() as f64 * BIT_WIDTH;
            let x = from * BIT_WIDTH;
            let series = match colour_of.iter().position(|(l, _)| *l == field.label) {
                Some(i) => colour_of[i].1,
                None => {
                    let i = colour_of.len();
                    colour_of.push((field.label.clone(), i));
                    i
                }
            };
            let mut node = bar_node(
                format!("field#{}-{}", field.start, field.end),
                Point::new(x + w / 2.0, top + ROW_HEIGHT / 2.0),
                Size::new(w, ROW_HEIGHT),
                Some(series),
            );
            node.panel = field_panel(field.start, field.end, &field.label, w);
            nodes.push(node);
        }
    }
    if nodes.is_empty() {
        return Err(RenderError::ChartHasNoExtent {
            what: "the packet has no fields",
        });
    }

    let mut diagram = Diagram {
        nodes,
        ..Diagram::default()
    };
    add_title(&mut diagram, &packet.preamble);
    normalise(&mut diagram);
    Ok(diagram)
}

/// The two centred lines inside a field: its bit range, then its name.
///
/// Both are dropped rather than overhanging: a name wider than its field lands on the neighbouring
/// field and reads as belonging to it.
fn field_panel(start: u32, end: u32, label: &str, w: f64) -> Option<Panel> {
    let range = Label::measure(&if start == end {
        format!("{start}")
    } else {
        format!("{start}-{end}")
    });
    let name = Label::measure(label);
    let usable = w - TEXT_PAD * 2.0;
    let mut rows: Vec<PanelRow> = Vec::new();
    let fits = |l: &Label| l.width <= usable;
    let show_range = fits(&range);
    let show_name = fits(&name) && !name.is_blank();
    let count = usize::from(show_range) + usize::from(show_name);
    if count == 0 {
        return None;
    }
    let block = count as f64 * line_height();
    let mut y = ROW_HEIGHT / 2.0 - block / 2.0 + line_height() / 2.0;
    if show_range {
        rows.push(PanelRow {
            cells: vec![PanelCell {
                label: range,
                x: w / 2.0,
                centered: true,
            }],
            y,
        });
        y += line_height();
    }
    if show_name {
        rows.push(PanelRow {
            cells: vec![PanelCell {
                label: name,
                x: w / 2.0,
                centered: true,
            }],
            y,
        });
    }
    Some(Panel {
        rows,
        rules: Vec::new(),
        columns: Vec::new(),
        size: Size::new(w, ROW_HEIGHT),
    })
}
