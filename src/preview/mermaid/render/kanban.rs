//! Drawing a `kanban`.
//!
//! # A column is a frame and a card is a box with a table in it
//!
//! Both pieces already exist: a column is a [`PlacedCluster`] (so the six cluster invariants hold
//! for it), and a card's name over its `assigned` / `ticket` / `priority` lines is a
//! [`Panel`](super::Panel) — the same one a class box and an ER entity use.
//!
//! # The columns are all the same width
//!
//! A board is read across, and a row of columns of different widths reads as a hierarchy. So the
//! width is the widest card on the board, and every column takes it.

use crate::preview::mermaid::flowchart::Shape;
use crate::preview::mermaid::kanban::{Card, Kanban};
use crate::preview::mermaid::layout::Point;
use crate::preview::mermaid::text_metrics;

use super::band;
use super::panel::{class_panel, Compartment};
use super::{
    normalise, shapes, svg, Diagram, Glyph, Label, PlacedCluster, PlacedNode, RenderError, Size,
    Theme,
};

/// Blank space between two columns.
///
/// Wider than twice [`COLUMN_PAD`], because the frames reach that far past their cards: a gap
/// narrower than the padding would make two neighbouring frames overlap, which
/// `check_unrelated_clusters_do_not_overlap` catches and which reads as one wide column.
pub const COLUMN_GAP: f64 = 26.0;

/// Blank space between two cards.
pub const CARD_GAP: f64 = 8.0;

/// Blank space between a column frame and the cards in it.
pub const COLUMN_PAD: f64 = 8.0;

/// The narrowest a column may be.
pub const MIN_COLUMN: f64 = 120.0;

/// Reads a mermaid kanban board and draws it.
///
/// **This is the entry point the golden tests go through** (§6).
pub fn render(code: &str, theme: &str) -> Result<String, RenderError> {
    let board = crate::preview::mermaid::kanban::parse(code)?;
    let diagram = lay_out(&board)?;
    Ok(svg::emit(&diagram, &Theme::named(theme)))
}

/// Works out where every column and card goes.
pub fn lay_out(board: &Kanban) -> Result<Diagram, RenderError> {
    if !text_metrics::fonts_available() {
        return Err(RenderError::NoFonts);
    }
    if board.columns.is_empty() {
        return Err(RenderError::NothingToDraw);
    }

    // Every card is measured first, because the board's column width is the widest of all of them.
    let panels: Vec<Vec<super::Panel>> = board
        .columns
        .iter()
        .map(|c| c.cards.iter().map(card_panel).collect())
        .collect();
    let widest = panels
        .iter()
        .flatten()
        .map(|p| p.size.w)
        .fold(MIN_COLUMN, f64::max);

    let mut nodes: Vec<PlacedNode> = Vec::new();
    let mut clusters: Vec<PlacedCluster> = Vec::new();
    let mut x = 0.0_f64;
    let mut n = 0usize;
    for (ci, column) in board.columns.iter().enumerate() {
        let mut y = 0.0_f64;
        for (k, card) in column.cards.iter().enumerate() {
            let panel = &panels[ci][k];
            let size = Size::new(widest, panel.size.h);
            // The panel decides the box, so a card whose panel is narrower than the column is
            // *widened* rather than left ragged — and the panel's own size has to say so, or
            // `check_panels_stay_inside_their_box` would be comparing two different boxes.
            let mut panel = panel.clone();
            panel.size = size;
            nodes.push(PlacedNode {
                id: card.id.clone(),
                shape: Glyph::Flow(Shape::RoundedRect),
                center: Point::new(x + widest / 2.0, y + size.h / 2.0),
                size,
                label: Label::measure(""),
                panel: Some(panel),
                series: Some(ci),
                mark: None,
            });
            y += size.h + CARD_GAP;
            n += 1;
        }
        let bottom = if column.cards.is_empty() {
            shapes::PADDING * 4.0
        } else {
            y - CARD_GAP
        };
        clusters.push(band::frame(
            column.id.clone(),
            &column_title(column.label.as_str(), column.ticket.as_deref()),
            x - COLUMN_PAD,
            -COLUMN_PAD,
            x + widest + COLUMN_PAD,
            bottom + COLUMN_PAD,
        ));
        x += widest + COLUMN_GAP;
    }
    if n == 0 && clusters.is_empty() {
        return Err(RenderError::NothingToDraw);
    }

    // The frames are the tallest thing on the board and they were built to different heights, so
    // they are levelled: a board whose columns end at different depths reads as unfinished.
    let deepest = clusters
        .iter()
        .map(|c| c.bounds().3)
        .fold(f64::NEG_INFINITY, f64::max);
    for c in &mut clusters {
        let (l, t, r, b) = c.bounds();
        if deepest > b {
            c.size = Size::new(r - l, deepest - t);
            c.center = Point::new((l + r) / 2.0, (t + deepest) / 2.0);
        }
    }

    let mut diagram = Diagram {
        nodes,
        clusters,
        ..Diagram::default()
    };
    normalise(&mut diagram);
    Ok(diagram)
}

/// A card's contents: its words, then whatever metadata it carries.
fn card_panel(card: &Card) -> super::Panel {
    let mut meta: Vec<Label> = Vec::new();
    for (key, value) in [
        ("assigned", &card.assigned),
        ("ticket", &card.ticket),
        ("priority", &card.priority),
    ] {
        if let Some(v) = value.as_deref().filter(|v| !v.trim().is_empty()) {
            meta.push(Label::measure(&format!("{key}: {v}")));
        }
    }
    let mut compartments = vec![Compartment {
        lines: vec![Label::measure(&card.label)],
        centered: false,
    }];
    // Only when there is something to say. An empty compartment on every card would double the
    // height of a board whose cards carry no metadata, which is most of them.
    if !meta.is_empty() {
        compartments.push(Compartment {
            lines: meta,
            centered: false,
        });
    }
    class_panel(&compartments)
}

/// The words on a column's frame, with its ticket count where the source gave one.
fn column_title(label: &str, ticket: Option<&str>) -> String {
    match ticket.filter(|t| !t.trim().is_empty()) {
        Some(t) => format!("{label} ({t})"),
        None => label.to_string(),
    }
}
