//! Drawing a `pie`.
//!
//! The geometry is one line long — a slice's sweep is its share of a full turn — so what this
//! module is really about is the two things around it.
//!
//! **The percentage goes on the slice, and only when it fits.** mermaid draws every one, which on
//! a slice of half a percent puts the digits outside the circle and over the next slice's. konoma
//! drops a label whose own box does not fit inside its wedge, and lets the legend carry the number
//! instead — which it does anyway when the source says `showData`.
//!
//! **A zero-value slice is kept in the legend and drawn nowhere.** A wedge of no angle is not a
//! thin wedge, it is a stroke on the circle's radius that reads as a boundary between the two
//! slices either side of it. The datum still exists, so the legend still names it.

use crate::preview::mermaid::chart::pie::Pie;
use crate::preview::mermaid::layout::Point;

use super::super::shapes::Mark;
use super::super::{normalise, Diagram, Glyph, Label, PlacedNode, RenderError, Size, Theme};
use super::{
    add_title, label_node, legend_nodes, legend_size, LegendEntry, LEGEND_GAP, POINT_RADIUS,
};

/// Radius of the circle.
pub const RADIUS: f64 = 118.0;

/// A full turn.
const TURN: f64 = 360.0;

/// Reads a mermaid pie source and draws it.
///
/// **This is the entry point the golden tests go through** (§6).
pub fn render(code: &str, theme: &str) -> Result<String, RenderError> {
    let pie = crate::preview::mermaid::chart::pie::parse(code)?;
    let diagram = lay_out(&pie)?;
    Ok(super::super::svg::emit(&diagram, &Theme::named(theme)))
}

/// Works out where every slice, percentage and legend row goes.
pub fn lay_out(pie: &Pie) -> Result<Diagram, RenderError> {
    // The gate: usvg drops glyphs rather than failing, so a fontless machine has to be caught
    // before an SVG exists at all (PRD design principle #3).
    if !crate::preview::mermaid::text_metrics::fonts_available() {
        return Err(RenderError::NoFonts);
    }
    let total = pie.total();
    if total <= 0.0 {
        // Every share would be undefined. Upstream draws a circle divided into NaN sectors, which
        // resvg renders as nothing inside a stroked ring — a chart that looks deliberate and says
        // nothing at all.
        return Err(RenderError::ChartHasNoExtent {
            what: "every slice is zero, so no slice has a share",
        });
    }

    let mut nodes: Vec<PlacedNode> = Vec::with_capacity(pie.slices.len() * 2 + 1);
    let center = Point::new(RADIUS, RADIUS);

    // --- the slices ------------------------------------------------------------------------
    //
    // Source order, clockwise from twelve o'clock, which is what `pieRenderer` gets from d3's
    // `pie().sort(null)`. The running start is carried rather than recomputed per slice so the
    // sweeps are guaranteed to abut exactly and the last one to end where the first began.
    let mut start = 0.0_f64;
    for (i, slice) in pie.slices.iter().enumerate() {
        let sweep = slice.value / total * TURN;
        if sweep > 0.0 {
            nodes.push(PlacedNode {
                id: format!("slice#{i}"),
                shape: Glyph::Wedge,
                center: center.clone(),
                size: Size::new(RADIUS * 2.0, RADIUS * 2.0),
                label: Label::measure(""),
                panel: None,
                series: Some(i),
                mark: Some(Mark::Wedge { start, sweep }),
            });
        }
        start += sweep;
    }

    // --- the percentages -------------------------------------------------------------------
    let mut start = 0.0_f64;
    for (i, slice) in pie.slices.iter().enumerate() {
        let sweep = slice.value / total * TURN;
        if sweep > 0.0 {
            let text = percent_text(slice.value / total);
            let label = Label::measure(&text);
            // Two thirds out along the bisector: far enough from the centre that the labels of
            // several thin slices do not pile up there, near enough that a wide one stays inside.
            let mid = (start + sweep / 2.0 - 90.0).to_radians();
            let at = RADIUS * 0.62;
            let c = Point::new(center.x + at * mid.cos(), center.y + at * mid.sin());
            if fits_inside_wedge(&label, at, sweep) {
                if let Some(n) = label_node(format!("slice#{i}#pct"), label, c, Some(i)) {
                    nodes.push(n);
                }
            }
        }
        start += sweep;
    }

    // --- the legend ------------------------------------------------------------------------
    //
    // Every slice gets a row, zero-valued ones included: the datum is in the source, and a legend
    // that quietly omits it is a chart that answers a different question.
    let entries: Vec<LegendEntry> = pie
        .slices
        .iter()
        .enumerate()
        .map(|(i, s)| LegendEntry {
            label: Label::measure(&if pie.show_data {
                format!("{} [{}]", s.label, tidy(s.value))
            } else {
                s.label.clone()
            }),
            series: i,
        })
        .collect();
    let size = legend_size(&entries);
    let left = center.x + RADIUS + LEGEND_GAP;
    let top = center.y - size.h / 2.0;
    nodes.extend(legend_nodes(&entries, left, top));

    let mut diagram = Diagram {
        nodes,
        ..Diagram::default()
    };
    add_title(&mut diagram, &pie.preamble);
    normalise(&mut diagram);
    Ok(diagram)
}

/// Whether a label of this size sits inside a wedge of this sweep, at this distance from the
/// centre.
///
/// The chord across the wedge at radius `at` is `2 * at * sin(sweep/2)`, and the label has to fit
/// across it with room at each end; its height has to fit in the radial direction too, which for
/// a very thin slice is what rules it out.
fn fits_inside_wedge(label: &Label, at: f64, sweep_deg: f64) -> bool {
    // **Half the sweep, but never more than a right angle.** `2 * at * sin(sweep/2)` is the chord
    // across the wedge only while the wedge is at most a half turn; past that the sine comes back
    // down and reaches zero again at a full turn, so a slice that fills the whole circle measured
    // as having no room in it at all. A one-slice pie therefore drew no `100%`, and
    // `"Bulk": 9990` beside `"Trace": 8` and `"Residue": 2` drew no share on the bulk either — the
    // two cases where the number is least in doubt were the two konoma left off. Beyond a half
    // turn the wedge contains the whole disc of radius `at` about its bisector, so the width
    // available is `2 * at`, which is what clamping the half-angle to 90° gives.
    let half = (sweep_deg / 2.0).min(90.0).to_radians();
    let chord = 2.0 * at * half.sin();
    label.width + POINT_RADIUS * 2.0 <= chord && label.height <= RADIUS - at + RADIUS * 0.3
}

/// The percentage mermaid prints: a whole number with a `%`, matching `pieRenderer`'s
/// `format('.0%')` unless that would round a real slice to nothing.
fn percent_text(share: f64) -> String {
    let pct = share * 100.0;
    if pct < 0.5 && pct > 0.0 {
        // `.0%` would say "0%" for a slice that is really there, which is the one number a reader
        // must not be given.
        return "<1%".to_string();
    }
    format!("{}%", pct.round() as i64)
}

/// A raw value beside its label when `showData` was written.
///
/// **Not [`super::tick_text`].** That one rounds for legibility, which is right for an axis — a
/// tick is a position — and wrong here: `showData` exists so the reader can see the number the
/// author wrote, and `42.96` shown as `43` is the one thing this label must not do.
fn tidy(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 1e15 {
        return format!("{}", v as i64);
    }
    let s = format!("{v:.6}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}
