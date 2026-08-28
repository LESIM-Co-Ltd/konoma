//! Drawing the seven data charts — stage 5.
//!
//! ```text
//! Pie / XyChart / … ──▶ measure ──▶ work the geometry out directly ──▶ Diagram ──▶ SVG
//! ```
//!
//! # None of these goes through `lay_out_spec`, and that is the design
//!
//! `docs/FEATURE-MERMAID-RENDERER.md` §7 settles it, and stage 4 already took the same decision
//! for sequence diagrams. [`super::lay_out_spec`] is the pipeline for a **layered graph**:
//! measure, rank, order to reduce crossings, assign coordinates, route. Not one of these seven has
//! any of those questions in it. A pie has no edges. A bar's position is its index. A packet
//! field's position is the bit it starts at, and moving it would be a lie. Feeding them to dagre
//! would mean inventing nodes and edges that mean something other than what they are.
//!
//! So each of these modules computes its geometry and returns a [`Diagram`] in one pass.
//! **Everything below layout is shared**: the same `Diagram`, the same [`Glyph`] and [`Tip`]
//! enums, the same [`svg::emit`], the same five palettes, the same
//! [`normalise`](super::normalise) — so "nothing is drawn outside the viewBox" is true here for
//! the same reason it is true of a flowchart, rather than for a new reason nothing checks.
//!
//! [`Tip`]: super::Tip
//!
//! # What stage 5 added to the seam, and what it did not
//!
//! Seven [`Glyph`] variants and three [`Mark`](super::shapes::Mark)s — no second emission path,
//! and **no new field on [`Diagram`]**. That is worth stating because stage 4 did need one
//! (`lifelines`): a chart's marks really are nodes and its lines really are edges, so nothing here
//! is a new kind of geometry, only a new kind of *shape*.
//!
//! Two fields did grow on [`PlacedNode`] and [`PlacedEdge`]:
//! [`series`](PlacedNode::series), which is what makes a legend entry provably the colour of its
//! series, and [`mark`](PlacedNode::mark), which carries the angles and corners a bounding box
//! cannot say.
//!
//! # The charts do not reuse most of the graph invariants, and each one says why
//!
//! [`super::tests`] holds twelve geometric invariants that every graph language calls. A chart
//! calls the ones that still mean something and states, in its own tests, why the others cannot
//! apply — the same treatment stage 4 gave sequence diagrams. The commonest reason is that a
//! chart's marks **are meant to touch**: a pie's slices share a centre, a treemap's tiles share
//! their edges, a label sits on top of the mark it names. "No two nodes overlap" is not a weaker
//! statement about those, it is a false one.

pub mod packet;
pub mod pie;
pub mod quadrant;
pub mod radar;
pub mod sankey;
pub mod treemap;
pub mod xychart;

#[cfg(test)]
mod tests;

use crate::preview::mermaid::chart::Preamble;
use crate::preview::mermaid::layout::Point;

use super::labels::line_height;
use super::shapes::Glyph;
use super::{Diagram, Label, PlacedEdge, PlacedNode, Size, Tip};
use crate::preview::mermaid::flowchart::Stroke;

/// Blank space between the title and the chart under it.
pub const TITLE_GAP: f64 = 12.0;

/// Blank space between an axis and the labels along it.
pub const TICK_GAP: f64 = 5.0;

/// How far a tick mark reaches out of the axis.
pub const TICK_LEN: f64 = 4.0;

/// Blank space between the tick labels and the axis title beyond them.
pub const AXIS_TITLE_GAP: f64 = 6.0;

/// Side of a legend swatch.
pub const SWATCH: f64 = 11.0;

/// Blank space between a swatch and its text.
pub const SWATCH_GAP: f64 = 6.0;

/// Blank space between two legend rows.
pub const LEGEND_ROW_GAP: f64 = 4.0;

/// Blank space between the chart and the legend beside it.
pub const LEGEND_GAP: f64 = 18.0;

/// Radius of the disc marking one datum on a line plot.
pub const POINT_RADIUS: f64 = 3.0;

/// The chart's own title, measured, or `None` when it has none.
pub fn title_label(preamble: &Preamble) -> Option<Label> {
    let text = preamble.title.as_deref()?.trim();
    if text.is_empty() {
        return None;
    }
    let label = Label::measure(text);
    (!label.is_blank()).then_some(label)
}

/// A [`Glyph::ChartLabel`] node — words with no outline, in a box that is exactly the words.
///
/// `None` for text that would draw nothing, so a blank axis title does not leave an invisible node
/// widening the viewBox.
pub fn label_node(
    id: impl Into<String>,
    label: Label,
    center: Point,
    series: Option<usize>,
) -> Option<PlacedNode> {
    if label.is_blank() {
        return None;
    }
    Some(PlacedNode {
        id: id.into(),
        shape: Glyph::ChartLabel,
        center,
        size: Size::new(label.width, label.height),
        label,
        panel: None,
        series,
        mark: None,
        style: None,
    })
}

/// The same, measuring the text here.
pub fn text_node(
    id: impl Into<String>,
    text: &str,
    center: Point,
    series: Option<usize>,
) -> Option<PlacedNode> {
    label_node(id, Label::measure(text), center, series)
}

/// A straight line drawn as an edge with nothing on either end — an axis, a tick, a grid line.
///
/// An edge rather than a node because that is what it is: two points and a stroke. Reusing
/// [`PlacedEdge`] also means [`super::normalise`] already knows to include it in the viewBox.
pub fn rule(from: Point, to: Point, stroke: Stroke, series: Option<usize>) -> PlacedEdge {
    PlacedEdge {
        from: String::new(),
        to: String::new(),
        points: vec![from, to],
        tip_start: Tip::None,
        tip_end: Tip::None,
        stroke,
        label: None,
        start_label: None,
        end_label: None,
        badge: None,
        series,
        // **Deliberately `false`.** A rule is two points, so a spline through it *is* the straight
        // line — the geometry is identical either way and only the path's spelling differs
        // (`M…C…` against `M…L…`). Setting it would move six checked-in goldens for no change a
        // reader could see, and §6 says a golden that moves has to be explained rather than
        // regenerated. The flag exists for routes with a corner in them.
        straight: false,
        overlay: false,
        style: None,
    }
}

/// The line one series traces through the marks: an xy chart's plot, a radar chart's curve.
///
/// The one kind of edge that is drawn **over** the nodes — see [`PlacedEdge::overlay`] for why,
/// and for why saying it here rather than inferring it from `series` is what stopped a git graph's
/// lane line being drawn through its own captions.
pub fn data_path(points: Vec<Point>, series: usize) -> PlacedEdge {
    let mut e = rule(
        points[0].clone(),
        points[points.len() - 1].clone(),
        Stroke::Normal,
        Some(series),
    );
    e.points = points;
    e.overlay = true;
    e
}

/// A rectangle painted in a series colour.
pub fn bar_node(
    id: impl Into<String>,
    center: Point,
    size: Size,
    series: Option<usize>,
) -> PlacedNode {
    PlacedNode {
        id: id.into(),
        shape: Glyph::ChartBar,
        center,
        size,
        label: Label::measure(""),
        panel: None,
        series,
        mark: None,
        style: None,
    }
}

/// One legend row: what it is called, and which series colour it stands for.
pub struct LegendEntry {
    /// The text beside the swatch.
    pub label: Label,
    /// Which palette entry the swatch is painted in.
    pub series: usize,
}

/// How wide and tall a legend of these entries will be.
pub fn legend_size(entries: &[LegendEntry]) -> Size {
    if entries.is_empty() {
        return Size::new(0.0, 0.0);
    }
    let w = entries
        .iter()
        .map(|e| SWATCH + SWATCH_GAP + e.label.width)
        .fold(0.0_f64, f64::max);
    let row = line_height().max(SWATCH);
    let h = entries.len() as f64 * row + (entries.len() - 1) as f64 * LEGEND_ROW_GAP;
    Size::new(w, h)
}

/// Lays a legend out with its top-left corner at `(left, top)`.
///
/// The swatch and its text are two nodes, not one: the swatch carries the series and the text does
/// not, which is what lets a test say "the swatch beside *this* name is the colour *that* series
/// is drawn in" — the pairing a mutation swapping two legend entries destroys.
pub fn legend_nodes(entries: &[LegendEntry], left: f64, top: f64) -> Vec<PlacedNode> {
    let row = line_height().max(SWATCH);
    let mut out = Vec::with_capacity(entries.len() * 2);
    for (i, e) in entries.iter().enumerate() {
        let cy = top + i as f64 * (row + LEGEND_ROW_GAP) + row / 2.0;
        out.push(bar_node(
            format!("legend#{i}#swatch"),
            Point::new(left + SWATCH / 2.0, cy),
            Size::new(SWATCH, SWATCH),
            Some(e.series),
        ));
        if let Some(n) = label_node(
            format!("legend#{i}#label"),
            e.label.clone(),
            Point::new(left + SWATCH + SWATCH_GAP + e.label.width / 2.0, cy),
            None,
        ) {
            out.push(n);
        }
    }
    out
}

/// "Nice" tick values spanning `min..=max`, using the 1 / 2 / 5 progression every axis library
/// settles on.
///
/// Two properties are what the callers rely on and what `tests` states: **every returned value is
/// inside `min..=max`**, so no tick is drawn off the end of the axis, and the values are strictly
/// increasing, so two ticks never land on the same pixel.
pub fn ticks(min: f64, max: f64, target: usize) -> Vec<f64> {
    if !min.is_finite() || !max.is_finite() || max <= min || target == 0 {
        return vec![min, max]
            .into_iter()
            .filter(|v| v.is_finite())
            .collect();
    }
    let raw = (max - min) / target as f64;
    let magnitude = 10f64.powf(raw.log10().floor());
    let normalised = raw / magnitude;
    let step = magnitude
        * if normalised <= 1.0 {
            1.0
        } else if normalised <= 2.0 {
            2.0
        } else if normalised <= 5.0 {
            5.0
        } else {
            10.0
        };
    if step <= 0.0 || !step.is_finite() {
        return vec![min, max];
    }
    let mut out = Vec::new();
    let first = (min / step).ceil() * step;
    let mut v = first;
    // The guard is on the *count*, not on the value: a step that rounds badly could otherwise
    // fail to advance and spin here.
    while v <= max + step * 1e-9 && out.len() <= target * 4 {
        // `-0` and a value one float step below `min` both come out of the arithmetic above.
        out.push(if v.abs() < step * 1e-9 { 0.0 } else { v });
        v += step;
    }
    out.retain(|t| *t >= min - step * 1e-9 && *t <= max + step * 1e-9);
    if out.is_empty() {
        out.push(min);
        out.push(max);
    }
    out
}

/// The whole run of tick labels for one axis, spelled so that **no two of them read the same**.
///
/// [`tick_text`] decides how many decimals a number gets from that number's own magnitude, which
/// is the right question for a value standing on its own — a treemap tile's `12.5` — and the wrong
/// one for a tick. How much precision a tick needs is a property of the *axis*: it is however much
/// separates it from the tick beside it. `y-axis 0 --> 0.001` steps by 0.0002 and every value in
/// it is under 1, so the per-value rule gave three decimals and the axis read
/// `0, 0, 0, 0.001, 0.001, 0.001` from the bottom up — six grid lines carrying three numbers, half
/// of them another tick's. That is a chart that lies rather than a chart that looks untidy: a
/// reader takes the line labelled `0` two fifths of the way up as the zero line.
///
/// So the decimals are chosen once, for the run: the fewest that tell every tick apart.
pub fn axis_tick_texts(values: &[f64]) -> Vec<String> {
    // 15 is where an `f64` runs out of significant decimal digits; past it more places cannot
    // separate anything and the loop would spin producing noise.
    for decimals in 0..=15usize {
        let out: Vec<String> = values.iter().map(|v| fixed(*v, decimals)).collect();
        let mut seen = std::collections::HashSet::with_capacity(out.len());
        if out.iter().all(|t| seen.insert(t.clone())) {
            return out;
        }
    }
    values.iter().map(|v| fixed(*v, 15)).collect()
}

/// One value at a fixed number of decimals, with the trailing zeros taken off.
///
/// Trimming cannot re-collide two labels: two different numbers written to the same number of
/// decimals differ in some place, and dropping trailing zeros keeps that place.
fn fixed(v: f64, decimals: usize) -> String {
    let s = format!("{v:.decimals$}");
    let s = if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        s
    };
    if s.is_empty() || s == "-0" {
        "0".to_string()
    } else {
        s
    }
}

/// A tick value as a reader wants to see it: no trailing zeros, and no exponent for the
/// magnitudes a chart in a terminal actually shows.
pub fn tick_text(v: f64) -> String {
    if v == 0.0 {
        return "0".to_string();
    }
    let a = v.abs();
    let decimals = if a >= 100.0 {
        0
    } else if a >= 10.0 {
        1
    } else if a >= 1.0 {
        2
    } else {
        3
    };
    let s = format!("{v:.decimals$}");
    let s = if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        s
    };
    if s.is_empty() || s == "-0" {
        "0".to_string()
    } else {
        s
    }
}

/// Puts the chart's title above `diagram`, if it has one, and returns the diagram.
///
/// Called last by every chart, after everything else has been placed, because the title is centred
/// on the *finished* drawing rather than on the part of it the chart happened to compute first.
pub fn add_title(diagram: &mut Diagram, preamble: &Preamble) {
    let Some(label) = title_label(preamble) else {
        return;
    };
    let (mut min_x, mut min_y) = (f64::INFINITY, f64::INFINITY);
    let mut max_x = f64::NEG_INFINITY;
    for n in &diagram.nodes {
        let (l, t, r, _) = n.bounds();
        min_x = min_x.min(l);
        min_y = min_y.min(t);
        max_x = max_x.max(r);
    }
    for e in &diagram.edges {
        for p in &e.points {
            min_x = min_x.min(p.x);
            min_y = min_y.min(p.y);
            max_x = max_x.max(p.x);
        }
    }
    if !min_x.is_finite() {
        return;
    }
    let center = Point::new(
        (min_x + max_x) / 2.0,
        min_y - TITLE_GAP - label.height / 2.0,
    );
    if let Some(n) = label_node("chart#title", label, center, None) {
        diagram.nodes.push(n);
    }
}
