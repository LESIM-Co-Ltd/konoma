//! Drawing a `radar-beta`.
//!
//! # The scale, and the one function that applies it
//!
//! [`radius_of`] is `r * (clamp(v, min, max) - min) / (max - min)` — upstream's `relativeRadius`,
//! transcribed. Everything on the chart that stands for a value goes through it, so "a vertex is
//! as far out as its value is up the scale" is a property of one function.
//!
//! # A curve is drawn only if it has an entry for every axis
//!
//! [`is_drawable`], which is upstream's rule and konoma's for the same reason: a partial polygon
//! is not a partial reading, it is a whole shape of the wrong shape.
//!
//! # konoma draws the curves as outlines, not as translucent fills
//!
//! mermaid fills each curve at 20% opacity. Two filled curves on a terminal's own ground stack
//! into a third colour that means nothing, and the chart's own graticule disappears under them.
//! An outline per series, with the legend to name them, is the same information and survives
//! being composited onto whatever the terminal is showing (§0-1 is what allows the difference).

use crate::preview::mermaid::chart::radar::{Graticule, Radar};
use crate::preview::mermaid::flowchart::Stroke;
use crate::preview::mermaid::layout::Point;

use super::super::shapes::Mark;
use super::super::{normalise, Diagram, Glyph, Label, PlacedNode, RenderError, Size, Theme};
use super::{
    add_title, label_node, legend_nodes, legend_size, rule, LegendEntry, LEGEND_GAP, TICK_GAP,
};

/// Radius of the outermost ring.
pub const RADIUS: f64 = 120.0;

/// Blank space between the outer ring and an axis label.
pub const LABEL_GAP: f64 = 8.0;

/// Reads a mermaid radar source and draws it.
///
/// **This is the entry point the golden tests go through** (§6).
pub fn render(code: &str, theme: &str) -> Result<String, RenderError> {
    let radar = crate::preview::mermaid::chart::radar::parse(code)?;
    let diagram = lay_out(&radar)?;
    Ok(super::super::svg::emit(&diagram, &Theme::named(theme)))
}

/// How far out a value sits. Upstream's `relativeRadius`, clamping included.
pub fn radius_of(v: f64, min: f64, max: f64) -> f64 {
    if max <= min {
        return 0.0;
    }
    RADIUS * (v.clamp(min, max) - min) / (max - min)
}

/// Whether a curve can be drawn at all: it needs **an entry for every axis**.
///
/// Upstream's rule, transcribed — `radarRenderer.ts` opens its loop with
/// `if (curve.entries.length !== numAxes) { // Skip curves that do not have an entry for each
/// axis. return; }` — and it is a rule konoma wants for its own reason. A curve with three
/// entries on a five-axis chart is not a curve with two gaps in it: drawn on the first three
/// spokes it is a closed triangle, a perfectly ordinary-looking shape that says the series is
/// absent from two dimensions it was never measured on. Truncating a *longer* curve is the same
/// lie from the other end.
///
/// konoma used to draw both, taking `min(entries, axes)` vertices and skipping only when fewer
/// than three were left.
fn is_drawable(curve: &crate::preview::mermaid::chart::radar::Curve, axes: usize) -> bool {
    curve.entries.len() == axes
}

/// The direction of spoke `k` of `n`, in radians, starting at twelve o'clock.
pub fn angle_of(k: usize, n: usize) -> f64 {
    if n == 0 {
        return -std::f64::consts::FRAC_PI_2;
    }
    -std::f64::consts::FRAC_PI_2 + std::f64::consts::TAU * k as f64 / n as f64
}

/// Works out where the graticule, the axis labels, each curve and the legend go.
pub fn lay_out(radar: &Radar) -> Result<Diagram, RenderError> {
    if !crate::preview::mermaid::text_metrics::fonts_available() {
        return Err(RenderError::NoFonts);
    }
    let n = radar.axes.len();
    if n < 3 {
        // Two spokes are a line and one is a stick. A "radar chart" of either is not a degenerate
        // radar chart, it is a picture of something else.
        return Err(RenderError::ChartHasNoExtent {
            what: "a radar chart needs at least three axes",
        });
    }
    let (min, max) = (radar.options.min, radar.max());
    if max <= min || !max.is_finite() {
        return Err(RenderError::ChartHasNoExtent {
            what: "the radar scale has no width",
        });
    }

    let center = Point::new(RADIUS, RADIUS);
    let mut nodes: Vec<PlacedNode> = Vec::new();
    let mut edges = Vec::new();

    nodes.push(PlacedNode {
        id: "graticule".to_string(),
        shape: Glyph::Graticule,
        center: center.clone(),
        size: Size::new(RADIUS * 2.0, RADIUS * 2.0),
        label: Label::measure(""),
        panel: None,
        series: None,
        mark: Some(Mark::Graticule {
            rings: radar.options.ticks.max(1),
            spokes: n,
            polygon: radar.options.graticule == Graticule::Polygon,
        }),
    });

    // --- the axis labels, each just outside its own spoke --------------------------------------
    //
    // Pushed out by half the label's own extent along the spoke's direction, so a label at three
    // o'clock clears the ring by its half-width and one at twelve clears it by its half-height.
    // A fixed gap would make the side labels sit on the ring and the top one float.
    for (k, axis) in radar.axes.iter().enumerate() {
        let a = angle_of(k, n);
        let label = Label::measure(&axis.label);
        let out = RADIUS + LABEL_GAP;
        let cx = center.x + out * a.cos() + a.cos() * label.width / 2.0;
        let cy = center.y + out * a.sin() + a.sin() * label.height / 2.0;
        if let Some(node) = label_node(
            format!("axis#{}", axis.name),
            label,
            Point::new(cx, cy),
            None,
        ) {
            nodes.push(node);
        }
    }

    // --- the curves ----------------------------------------------------------------------------
    for (i, curve) in radar.curves.iter().enumerate() {
        if !is_drawable(curve, n) {
            continue;
        }
        let mut points: Vec<Point> = (0..n)
            .map(|k| {
                let a = angle_of(k, n);
                let r = radius_of(curve.entries[k], min, max);
                Point::new(center.x + r * a.cos(), center.y + r * a.sin())
            })
            .collect();
        // Closed: the last vertex joins the first, which is what makes it an area rather than a
        // path with two loose ends.
        points.push(points[0].clone());
        let mut e = rule(
            points[0].clone(),
            points[points.len() - 1].clone(),
            Stroke::Normal,
            Some(i),
        );
        e.points = points;
        edges.push(e);
    }

    if !edges.iter().any(|e| e.series.is_some()) {
        // Every curve was skipped. What is left is a graticule with axis names round it and no
        // data at all — the transparent-rectangle failure §6 records, dressed up as a chart. Stage
        // 1's rule is to refuse rather than to draw it.
        return Err(RenderError::ChartHasNoExtent {
            what: "no curve has an entry for every axis, so there is nothing to plot",
        });
    }

    // --- the legend ------------------------------------------------------------------------
    if radar.options.show_legend {
        let entries: Vec<LegendEntry> = radar
            .curves
            .iter()
            .enumerate()
            // **Every curve, drawable or not.** A row naming a curve that was skipped is not a
            // row pointing at nothing: it is the only place the reader is told the series is in
            // the source and could not be plotted. Upstream builds its legend from `curves` the
            // same way, outside the loop that skips.
            .map(|(i, c)| LegendEntry {
                label: Label::measure(&c.label),
                series: i,
            })
            .collect();
        if !entries.is_empty() {
            let size = legend_size(&entries);
            // Placed against the *labelled* right edge, not the ring, or it would sit on top of
            // the three-o'clock axis label.
            let right = nodes
                .iter()
                .map(|node| node.bounds().2)
                .fold(f64::NEG_INFINITY, f64::max);
            nodes.extend(legend_nodes(
                &entries,
                right + LEGEND_GAP,
                center.y - size.h / 2.0,
            ));
        }
    }
    let _ = TICK_GAP;

    let mut diagram = Diagram {
        nodes,
        edges,
        ..Diagram::default()
    };
    add_title(&mut diagram, &radar.preamble);
    normalise(&mut diagram);
    Ok(diagram)
}
