//! Drawing an `xychart-beta`.
//!
//! # The one number everything else hangs off
//!
//! [`value_to_y`] maps a datum onto the plot, and every bar's height, every line vertex and every
//! grid line comes out of it. It is deliberately the only place the y scale is applied, so that
//! "a bar's height is proportional to its value" is a property of *one* function that a test can
//! state exactly, rather than a coincidence between three places that each do the arithmetic.
//!
//! # A bar stands on the axis, not on the bottom of the frame
//!
//! When the range does not include zero — `y-axis 4000 --> 11000`, mermaid's own example — a bar's
//! foot is the bottom of the plot, and the reader is being shown a *difference*, which is what
//! upstream draws too. When the range does straddle zero, the baseline is where zero is, so a
//! negative value hangs below it. Both are [`baseline_y`], and the test that pins it is the one a
//! mutation moving the baseline off zero has to get past.
//!
//! # `horizontal` is read and not drawn
//!
//! See [`crate::preview::mermaid::chart::xychart::Orientation`]. The chart still draws; it draws
//! the other way round.

use crate::preview::mermaid::chart::xychart::{Plot, PlotKind, XAxis, XyChart};
use crate::preview::mermaid::flowchart::Stroke;
use crate::preview::mermaid::layout::Point;

use super::super::{normalise, Diagram, Glyph, Label, PlacedNode, RenderError, Size, Theme};
use super::{
    add_title, bar_node, label_node, legend_nodes, legend_size, rule, text_node, tick_text, ticks,
    LegendEntry, AXIS_TITLE_GAP, LEGEND_GAP, POINT_RADIUS, TICK_GAP, TICK_LEN,
};

/// How wide the plotting area is before the categories ask for more.
pub const PLOT_WIDTH: f64 = 440.0;

/// How tall it is.
pub const PLOT_HEIGHT: f64 = 260.0;

/// Blank space between a category slot's edge and the bars in it.
pub const SLOT_PAD: f64 = 6.0;

/// Blank space between two bars sharing a slot.
pub const BAR_GAP: f64 = 2.0;

/// The narrowest a category slot may be, before the labels widen it.
pub const MIN_SLOT: f64 = 24.0;

/// How many y ticks to aim for.
pub const Y_TICKS: usize = 5;

/// Reads a mermaid xy chart source and draws it.
///
/// **This is the entry point the golden tests go through** (§6).
pub fn render(code: &str, theme: &str) -> Result<String, RenderError> {
    let chart = crate::preview::mermaid::chart::xychart::parse(code)?;
    let diagram = lay_out(&chart)?;
    Ok(super::super::svg::emit(&diagram, &Theme::named(theme)))
}

/// The plot's geometry, shared by everything that has to place something on it.
pub struct Frame {
    /// Left edge of the plotting area.
    pub left: f64,
    /// Right edge.
    pub right: f64,
    /// Top edge.
    pub top: f64,
    /// Bottom edge.
    pub bottom: f64,
    /// Low end of the value scale.
    pub min: f64,
    /// High end.
    pub max: f64,
}

impl Frame {
    /// Where a value sits, in px. **The only place the y scale is applied.**
    pub fn value_to_y(&self, v: f64) -> f64 {
        let span = self.max - self.min;
        if span <= 0.0 {
            return self.bottom;
        }
        self.bottom - (v - self.min) / span * (self.bottom - self.top)
    }

    /// Where a bar's foot goes: zero when the range straddles it, the bottom of the plot when it
    /// does not.
    pub fn baseline_y(&self) -> f64 {
        if self.min <= 0.0 && self.max >= 0.0 {
            self.value_to_y(0.0)
        } else {
            self.bottom
        }
    }

    /// The centre of category slot `i` of `n`.
    pub fn slot_center(&self, i: usize, n: usize) -> f64 {
        if n == 0 {
            return (self.left + self.right) / 2.0;
        }
        self.left + (self.right - self.left) * (i as f64 + 0.5) / n as f64
    }

    /// How wide one slot is.
    pub fn slot_width(&self, n: usize) -> f64 {
        if n == 0 {
            0.0
        } else {
            (self.right - self.left) / n as f64
        }
    }
}

/// Works out where every bar, line, tick and legend row goes.
pub fn lay_out(chart: &XyChart) -> Result<Diagram, RenderError> {
    if !crate::preview::mermaid::text_metrics::fonts_available() {
        return Err(RenderError::NoFonts);
    }
    let (mut min, mut max) = chart.y;
    if !min.is_finite() || !max.is_finite() {
        return Err(RenderError::ChartHasNoExtent {
            what: "the value axis has no range",
        });
    }
    if (max - min).abs() < f64::EPSILON {
        // Every value the same. A zero-height scale divides by zero everywhere; widening it by one
        // unit is what a reader expects to see — a flat line halfway up — and is what upstream's
        // d3 scale does with a degenerate domain.
        min -= 0.5;
        max += 0.5;
    }
    if max < min {
        std::mem::swap(&mut min, &mut max);
    }

    // The x labels decide how wide the plot has to be: two categories whose names touch is the
    // commonest way one of these charts becomes unreadable.
    let categories = category_labels(chart);
    let widest = categories.iter().map(|l| l.width).fold(0.0_f64, f64::max);
    let want_slot = (widest + TICK_GAP * 2.0).max(MIN_SLOT);
    let plot_w = PLOT_WIDTH.max(want_slot * categories.len().max(1) as f64);

    // The y tick labels decide how far the plot's left edge is from the origin.
    let tick_values = ticks(min, max, Y_TICKS);
    let tick_labels: Vec<Label> = tick_values
        .iter()
        .map(|v| Label::measure(&tick_text(*v)))
        .collect();
    let widest_tick = tick_labels.iter().map(|l| l.width).fold(0.0_f64, f64::max);

    let y_title = Label::measure(&chart.y_title);
    let x_title = Label::measure(&chart.x_title);
    // The y title is drawn upright rather than turned on its side: a terminal renders the diagram
    // small, and rotated text at that size is the first thing to become unreadable. It therefore
    // costs width, not height, which is why it is added here.
    let y_title_w = if y_title.is_blank() {
        0.0
    } else {
        y_title.width + AXIS_TITLE_GAP
    };
    let left = y_title_w + widest_tick + TICK_GAP + TICK_LEN;
    let frame = Frame {
        left,
        right: left + plot_w,
        top: 0.0,
        bottom: PLOT_HEIGHT,
        min,
        max,
    };

    let mut nodes: Vec<PlacedNode> = Vec::new();
    let mut edges = Vec::new();

    // --- the frame and its grid ---------------------------------------------------------------
    nodes.push(PlacedNode {
        id: "plot".to_string(),
        shape: Glyph::PlotFrame,
        center: Point::new(
            (frame.left + frame.right) / 2.0,
            (frame.top + frame.bottom) / 2.0,
        ),
        size: Size::new(frame.right - frame.left, frame.bottom - frame.top),
        label: Label::measure(""),
        panel: None,
        series: None,
        mark: None,
    });
    for (v, label) in tick_values.iter().zip(tick_labels.iter()) {
        let y = frame.value_to_y(*v);
        edges.push(rule(
            Point::new(frame.left, y),
            Point::new(frame.right, y),
            Stroke::Dotted,
            None,
        ));
        edges.push(rule(
            Point::new(frame.left - TICK_LEN, y),
            Point::new(frame.left, y),
            Stroke::Normal,
            None,
        ));
        if let Some(n) = label_node(
            format!("ytick#{}", tick_text(*v)),
            label.clone(),
            Point::new(frame.left - TICK_LEN - TICK_GAP - label.width / 2.0, y),
            None,
        ) {
            nodes.push(n);
        }
    }

    // --- the x labels -------------------------------------------------------------------------
    let n = categories.len();
    for (i, label) in categories.iter().enumerate() {
        let x = frame.slot_center(i, n);
        edges.push(rule(
            Point::new(x, frame.bottom),
            Point::new(x, frame.bottom + TICK_LEN),
            Stroke::Normal,
            None,
        ));
        if let Some(node) = label_node(
            format!("xtick#{i}"),
            label.clone(),
            Point::new(x, frame.bottom + TICK_LEN + TICK_GAP + label.height / 2.0),
            None,
        ) {
            nodes.push(node);
        }
    }

    // --- the plots ----------------------------------------------------------------------------
    let bars: Vec<usize> = chart
        .plots
        .iter()
        .enumerate()
        .filter(|(_, p)| p.kind == PlotKind::Bar)
        .map(|(i, _)| i)
        .collect();
    for (i, plot) in chart.plots.iter().enumerate() {
        match plot.kind {
            PlotKind::Bar => {
                let which = bars.iter().position(|b| *b == i).unwrap_or(0);
                draw_bars(&mut nodes, &frame, plot, i, which, bars.len(), n);
            }
            PlotKind::Line => draw_line(&mut nodes, &mut edges, &frame, plot, i, n),
        }
    }

    // --- the axis titles ----------------------------------------------------------------------
    let x_label_h = categories.iter().map(|l| l.height).fold(0.0_f64, f64::max);
    if let Some(node) = label_node(
        "xtitle",
        x_title.clone(),
        Point::new(
            (frame.left + frame.right) / 2.0,
            frame.bottom + TICK_LEN + TICK_GAP + x_label_h + AXIS_TITLE_GAP + x_title.height / 2.0,
        ),
        None,
    ) {
        nodes.push(node);
    }
    if let Some(node) = label_node(
        "ytitle",
        y_title.clone(),
        Point::new(y_title.width / 2.0, (frame.top + frame.bottom) / 2.0),
        None,
    ) {
        nodes.push(node);
    }

    // --- the legend ---------------------------------------------------------------------------
    //
    // Only when a series was actually named. A legend of blank rows is furniture that says
    // nothing, and it is what mermaid draws.
    let entries: Vec<LegendEntry> = chart
        .plots
        .iter()
        .enumerate()
        .filter(|(_, p)| !p.title.trim().is_empty())
        .map(|(i, p)| LegendEntry {
            label: Label::measure(&p.title),
            series: i,
        })
        .collect();
    if !entries.is_empty() {
        let size = legend_size(&entries);
        nodes.extend(legend_nodes(
            &entries,
            frame.right + LEGEND_GAP,
            (frame.top + frame.bottom) / 2.0 - size.h / 2.0,
        ));
    }

    let mut diagram = Diagram {
        nodes,
        edges,
        ..Diagram::default()
    };
    add_title(&mut diagram, &chart.preamble);
    normalise(&mut diagram);
    Ok(diagram)
}

/// The text under each slot.
///
/// A band axis names them; a linear one is labelled by the steps `transformDataWithoutCategory`
/// computed, which are already in the plot's data.
fn category_labels(chart: &XyChart) -> Vec<Label> {
    match &chart.x {
        XAxis::Band(categories) if !categories.is_empty() => {
            categories.iter().map(|c| Label::measure(c)).collect()
        }
        _ => chart
            .plots
            .first()
            .map(|p| p.data.iter().map(|(k, _)| Label::measure(k)).collect())
            .unwrap_or_default(),
    }
}

/// One bar series. `which` of `total` decides which share of the slot it gets, so two bar plots
/// stand side by side instead of one hiding the other.
fn draw_bars(
    nodes: &mut Vec<PlacedNode>,
    frame: &Frame,
    plot: &Plot,
    series: usize,
    which: usize,
    total: usize,
    slots: usize,
) {
    let slot = frame.slot_width(slots);
    let usable = (slot - SLOT_PAD * 2.0).max(2.0);
    let each =
        ((usable - BAR_GAP * (total.saturating_sub(1)) as f64) / total.max(1) as f64).max(1.0);
    let base = frame.baseline_y();
    for (i, (_, v)) in plot.data.iter().enumerate() {
        let cx = frame.slot_center(i, slots) - usable / 2.0
            + which as f64 * (each + BAR_GAP)
            + each / 2.0;
        let y = frame.value_to_y(*v);
        let (top, bottom) = if y <= base { (y, base) } else { (base, y) };
        let h = (bottom - top).max(0.0);
        nodes.push(bar_node(
            format!("bar#{series}#{i}"),
            Point::new(cx, (top + bottom) / 2.0),
            Size::new(each, h),
            Some(series),
        ));
    }
}

/// One line series: the polyline, plus a disc on each datum and the datum's own label when the
/// source gave it one.
fn draw_line(
    nodes: &mut Vec<PlacedNode>,
    edges: &mut Vec<super::super::PlacedEdge>,
    frame: &Frame,
    plot: &Plot,
    series: usize,
    slots: usize,
) {
    let points: Vec<Point> = plot
        .data
        .iter()
        .enumerate()
        .map(|(i, (_, v))| Point::new(frame.slot_center(i, slots), frame.value_to_y(*v)))
        .collect();
    if points.len() >= 2 {
        let mut e = rule(
            points[0].clone(),
            points[points.len() - 1].clone(),
            Stroke::Normal,
            Some(series),
        );
        e.points = points.clone();
        edges.push(e);
    }
    for (i, p) in points.iter().enumerate() {
        nodes.push(PlacedNode {
            id: format!("point#{series}#{i}"),
            shape: Glyph::ChartPoint,
            center: p.clone(),
            size: Size::new(POINT_RADIUS * 2.0, POINT_RADIUS * 2.0),
            label: Label::measure(""),
            panel: None,
            series: Some(series),
            mark: None,
        });
        if let Some(text) = plot.point_labels.get(i).filter(|t| !t.trim().is_empty()) {
            if let Some(n) = text_node(
                format!("point#{series}#{i}#label"),
                text,
                Point::new(
                    p.x,
                    p.y - POINT_RADIUS - TICK_GAP - super::super::labels::line_height() / 2.0,
                ),
                None,
            ) {
                nodes.push(n);
            }
        }
    }
}
