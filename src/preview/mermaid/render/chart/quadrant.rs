//! Drawing a `quadrantChart`.
//!
//! # The one thing this chart must get right
//!
//! A point at `[0.3, 0.6]` belongs in the top-left box, and **y runs up**: the source's y is a
//! fraction from the bottom, and SVG's y is a distance from the top, so [`place`] inverts it.
//! Getting that wrong produces a chart that is entirely plausible and says the opposite of what
//! the author wrote — which is why `tests` states it as "the point is inside the named quadrant"
//! rather than as a coordinate.
//!
//! # konoma does not paint the four boxes
//!
//! mermaid tints them four shades of near-white, which on a terminal is an opaque card over
//! whatever the user's background is (§1). Here the quadrants are made by the axes that divide
//! them, and each is named by the label in its corner — which is the information the tint was
//! standing in for.

use crate::preview::mermaid::chart::quadrant::QuadrantChart;
use crate::preview::mermaid::flowchart::Stroke;
use crate::preview::mermaid::layout::Point;

use super::super::{normalise, Diagram, Glyph, Label, PlacedNode, RenderError, Size, Theme};
use super::{add_title, label_node, rule, text_node, POINT_RADIUS, TICK_GAP};

/// Side of the square the four quadrants fill.
pub const SIDE: f64 = 340.0;

/// Blank space between a quadrant's corner and the label in it.
pub const CORNER_PAD: f64 = 8.0;

/// Radius of a plotted point.
pub const DOT_RADIUS: f64 = 4.5;

/// Reads a mermaid quadrant chart source and draws it.
///
/// **This is the entry point the golden tests go through** (§6).
pub fn render(code: &str, theme: &str) -> Result<String, RenderError> {
    let chart = crate::preview::mermaid::chart::quadrant::parse(code)?;
    let diagram = lay_out(&chart)?;
    Ok(super::super::svg::emit(&diagram, &Theme::named(theme)))
}

/// Where a `[x, y]` pair lands, in px.
///
/// `left`/`top` are the square's corner. **y is inverted**: `y = 1` is the top of the square.
pub fn place(left: f64, top: f64, x: f64, y: f64) -> Point {
    Point::new(left + x * SIDE, top + (1.0 - y) * SIDE)
}

/// Works out where the axes, the quadrant names and every point go.
pub fn lay_out(chart: &QuadrantChart) -> Result<Diagram, RenderError> {
    if !crate::preview::mermaid::text_metrics::fonts_available() {
        return Err(RenderError::NoFonts);
    }
    let (left, top) = (0.0, 0.0);
    let (right, bottom) = (SIDE, SIDE);
    let mid_x = left + SIDE / 2.0;
    let mid_y = top + SIDE / 2.0;

    let mut nodes: Vec<PlacedNode> = Vec::new();
    let mut edges = Vec::new();

    nodes.push(PlacedNode {
        id: "frame".to_string(),
        shape: Glyph::PlotFrame,
        center: Point::new(mid_x, mid_y),
        size: Size::new(SIDE, SIDE),
        label: Label::measure(""),
        panel: None,
        series: None,
        mark: None,
    });
    // The two dividers. These are what make the four quadrants four quadrants, so they are drawn
    // solid rather than as a grid: a reader has to be able to tell which side of the middle a
    // point is on without measuring.
    edges.push(rule(
        Point::new(mid_x, top),
        Point::new(mid_x, bottom),
        Stroke::Normal,
        None,
    ));
    edges.push(rule(
        Point::new(left, mid_y),
        Point::new(right, mid_y),
        Stroke::Normal,
        None,
    ));

    // --- the four names, each in its own corner -----------------------------------------------
    //
    // `quadrant-1` is the top **right** — mermaid's numbering is the mathematical one, going
    // anticlockwise from the top right, and it is the one thing about this chart everybody gets
    // backwards.
    for (text, qx, qy) in [
        (&chart.quadrant1, 1.0, 1.0),
        (&chart.quadrant2, 0.0, 1.0),
        (&chart.quadrant3, 0.0, 0.0),
        (&chart.quadrant4, 1.0, 0.0),
    ] {
        let label = Label::measure(text);
        if label.is_blank() {
            continue;
        }
        // Centred across its own half, tucked under the top edge of it.
        let cx = left + SIDE * 0.25 + qx * SIDE * 0.5;
        let band_top = top + (1.0 - qy) * SIDE * 0.5;
        let cy = band_top + CORNER_PAD + label.height / 2.0;
        if let Some(n) = label_node(
            format!(
                "quadrant#{}",
                if qy > 0.5 { qx as u8 } else { 2 + qx as u8 }
            ),
            label,
            Point::new(cx, cy),
            None,
        ) {
            nodes.push(n);
        }
    }

    // --- the points ---------------------------------------------------------------------------
    for (i, p) in chart.points.iter().enumerate() {
        let c = place(left, top, p.x, p.y);
        nodes.push(PlacedNode {
            id: format!("point#{i}"),
            shape: Glyph::ChartPoint,
            center: c.clone(),
            size: Size::new(DOT_RADIUS * 2.0, DOT_RADIUS * 2.0),
            label: Label::measure(""),
            panel: None,
            series: Some(i),
            mark: None,
        });
        let label = Label::measure(&p.label);
        if let Some(n) = label_node(
            format!("point#{i}#label"),
            label.clone(),
            Point::new(c.x, c.y + DOT_RADIUS + TICK_GAP + label.height / 2.0),
            None,
        ) {
            nodes.push(n);
        }
    }

    // --- the axis texts, one per side ---------------------------------------------------------
    let x_label_gap = POINT_RADIUS + TICK_GAP * 2.0;
    for (text, cx, below) in [
        (&chart.x_left, left + SIDE * 0.25, true),
        (&chart.x_right, left + SIDE * 0.75, true),
    ] {
        let label = Label::measure(text);
        if let Some(n) = label_node(
            format!(
                "xaxis#{}",
                if below && cx < mid_x { "left" } else { "right" }
            ),
            label.clone(),
            Point::new(cx, bottom + x_label_gap + label.height / 2.0),
            None,
        ) {
            nodes.push(n);
        }
    }
    // The y texts sit outside the left edge, upright — see `xychart`'s note on rotated text.
    for (text, cy, name) in [
        (&chart.y_bottom, top + SIDE * 0.75, "bottom"),
        (&chart.y_top, top + SIDE * 0.25, "top"),
    ] {
        let label = Label::measure(text);
        if let Some(n) = label_node(
            format!("yaxis#{name}"),
            label.clone(),
            Point::new(left - x_label_gap - label.width / 2.0, cy),
            None,
        ) {
            nodes.push(n);
        }
    }
    let _ = text_node("", "", Point::new(0.0, 0.0), None);

    let mut diagram = Diagram {
        nodes,
        edges,
        ..Diagram::default()
    };
    add_title(&mut diagram, &chart.preamble);
    normalise(&mut diagram);
    Ok(diagram)
}
