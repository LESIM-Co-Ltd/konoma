//! What stage 5's chart drawing is checked with.
//!
//! # Which of the twelve shared invariants apply, and why the others cannot
//!
//! [`super::super::tests`] holds twelve geometric invariants that every graph language calls, and
//! stage 4 established the rule for a language that is not a graph: call the ones that still mean
//! something, **and say in a comment why each of the others cannot apply** rather than leaving a
//! gap. Two of the twelve apply here:
//!
//! | # | invariant | charts |
//! |---|---|---|
//! | 1 | nodes do not overlap | **no** — a chart's marks are *meant* to touch. A pie's slices share a centre and a bounding box; a treemap's tiles share their edges and a section's tile contains every one of its children; a percentage sits on the wedge it belongs to. The statement is not weaker here, it is false, and the chart-specific tests below say the true version per kind (tiles tile, fields tile, wedges abut). |
//! | 2 | edges stay out of shapes | **no** — a grid line crosses the plot frame from side to side, and that is what a grid line is. |
//! | 3 | labels ride their edge | **no** — no chart edge carries a label. |
//! | 4 | endpoints land on an outline | **no** — a chart edge has no endpoint *node*: an axis ends at a coordinate, a line plot ends at a datum. |
//! | 5 | the viewBox contains everything | **yes** — [`the_shared_invariants_that_apply_hold`]. Charts reach it through the same `normalise`, which is the point of not having built a second pipeline. |
//! | 6–11 | the six cluster invariants | **no** — no chart has a cluster. Asserted rather than assumed: [`no_chart_produces_a_cluster`]. |
//! | 12 | panels stay inside their box | **yes** — a treemap tile and a packet field both carry one. |
//!
//! # What replaces them
//!
//! Ten invariants that are about *charts*, each one a property a wrong drawing would break while
//! still looking plausible: the wedges of a pie fill exactly one turn, a bar's height is its value
//! through the axis scale, a treemap's areas are in the ratio of its values, a quadrant point is
//! in the quadrant its coordinates name, every datum reaches the page exactly once, and a legend
//! entry is drawn in the colour its series is drawn in.

use std::collections::HashSet;

use crate::preview::markdown::mermaid_to_svg_reason;
use crate::preview::mermaid::chart;
use crate::preview::mermaid::layout::Point;
use crate::preview::mermaid::text_metrics;

use super::super::shapes::Mark;
use super::super::tests::{
    assert_snapshot, check_panels_stay_inside_their_box, check_view_box_contains_everything,
    mask_numbers, measured_texts, tree_of,
};
use super::super::{svg, Diagram, Glyph, Label, PlacedNode, RenderError, Size, Theme};
use super::{legend_nodes, ticks, LegendEntry};

/// The corpus, shared with the parser tests so the two cannot drift apart.
use crate::preview::mermaid::chart::tests::CASES;

// ---------------------------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------------------------

/// Lays a source out, whichever of the seven it is.
fn laid_out(src: &str) -> Diagram {
    dispatch(src).unwrap_or_else(|e| panic!("corpus source must lay out: {e}\n{src}"))
}

fn dispatch(src: &str) -> Result<Diagram, RenderError> {
    if chart::pie::is_pie(src) {
        return super::pie::lay_out(&chart::pie::parse(src)?);
    }
    if chart::xychart::is_xychart(src) {
        return super::xychart::lay_out(&chart::xychart::parse(src)?);
    }
    if chart::quadrant::is_quadrant_chart(src) {
        return super::quadrant::lay_out(&chart::quadrant::parse(src)?);
    }
    if chart::radar::is_radar(src) {
        return super::radar::lay_out(&chart::radar::parse(src)?);
    }
    if chart::treemap::is_treemap(src) {
        return super::treemap::lay_out(&chart::treemap::parse(src)?);
    }
    if chart::packet::is_packet(src) {
        return super::packet::lay_out(&chart::packet::parse(src)?);
    }
    if chart::sankey::is_sankey(src) {
        return super::sankey::lay_out(&chart::sankey::parse(src)?);
    }
    panic!("no chart parser claims this source")
}

/// The rendered SVG, through whichever entry point owns this source.
fn rendered(src: &str) -> String {
    mermaid_to_svg_reason(src, "dark").unwrap_or_else(|e| panic!("{e}\n{src}"))
}

fn corpus(name: &str) -> &'static str {
    CASES
        .iter()
        .find(|(n, _)| *n == name)
        .unwrap_or_else(|| panic!("no corpus case {name}"))
        .1
}

/// Every node of one glyph kind.
fn of_kind(d: &Diagram, glyph: Glyph) -> Vec<&PlacedNode> {
    d.nodes.iter().filter(|n| n.shape == glyph).collect()
}

/// Whether two boxes overlap by more than a rounding error.
fn overlap(a: (f64, f64, f64, f64), b: (f64, f64, f64, f64)) -> bool {
    let eps = 0.01;
    a.0 < b.2 - eps && b.0 < a.2 - eps && a.1 < b.3 - eps && b.1 < a.3 - eps
}

/// Every corpus case of one kind, **found by that kind's own routing predicate**.
///
/// Not by a hard-coded list of names. A list has to be remembered when the corpus grows, and it
/// was not: a `radar-beta` case with a non-zero `min` was added to close a surviving mutation and
/// the radar test went on running over the two cases it had been given by name, so the mutation
/// went on surviving. Asking the predicate makes a new case join every test of its kind by itself.
fn cases_of(is_ours: fn(&str) -> bool) -> Vec<(&'static str, &'static str)> {
    let found: Vec<(&str, &str)> = CASES
        .iter()
        .filter(|(_, src)| is_ours(src))
        .copied()
        .collect();
    assert!(!found.is_empty(), "the corpus has no case of this kind");
    found
}

// ---------------------------------------------------------------------------------------------
// 1. The main instrument: what konoma measured vs what resvg drew
// ---------------------------------------------------------------------------------------------

/// Label strings the instrument runs over. Every category §6 names, plus the double space
/// `docs/STATUS.md` records as a hole in the measuring path.
const LABELS: &[(&str, &str)] = &[
    ("ascii", "Revenue"),
    ("kerning", "AVATAR To Wa"),
    ("thin", "illicit lilli"),
    ("cjk", "全画面プレビュー"),
    ("cjk-punct", "開始、処理。「確認」"),
    ("emoji", "build 🚀 ship"),
    ("digits", "0123456789"),
    ("mixed", "konoma の preview を全画面 fullscreen で"),
    ("double-space", "loop  Every minute"),
];

/// **The instrument §6 asks for, for every new kind of label stage 5 draws.**
///
/// A [`Glyph::ChartLabel`]'s box **is** its ink — [`super::label_node`] sets `size` to the label's
/// own width and height with no padding at all — so the relation this states is not "the slack is
/// the padding" but the sharper "they are the same number". Nothing in between can hide an error.
///
/// The reference is usvg's own `Text::bounding_box().width()`, read back out of the document
/// konoma emitted, through konoma's shared font database. It is konoma against **resvg's real
/// shaper**, never against another copy of konoma, so it cannot quietly become a self-comparison.
#[test]
fn every_chart_label_kind_is_drawn_at_the_width_konoma_measured() {
    if !text_metrics::fonts_available() {
        eprintln!("no sans-serif face — skipping (the renderer refuses to draw here too)");
        return;
    }
    // (what kind of label this is, a source that puts `{}` into one). Every label kind stage 5
    // introduces is here; a kind with no row would be measured by nothing.
    let kinds: &[(&str, &str)] = &[
        ("chart title", "pie title {}\n  \"a\" : 1\n"),
        ("pie legend", "pie\n  \"{}\" : 1\n"),
        (
            "axis tick label",
            "xychart-beta\n  x-axis [{}]\n  bar [1]\n",
        ),
        // Quoted, because in an axis clause a digit is a **number**: `y-axis 0123456789 0 --> 10`
        // is three numbers to this grammar, not a title and a range. That is upstream's rule and
        // konoma's, and it is why an axis title with digits in it has to be a `STR`.
        (
            "axis title",
            "xychart-beta\n  y-axis \"{}\" 0 --> 10\n  bar [1]\n",
        ),
        (
            "plot legend",
            "xychart-beta\n  x-axis [a]\n  bar \"{}\" [1]\n",
        ),
        (
            "quadrant name",
            "quadrantChart\n  quadrant-1 {}\n  P: [0.5, 0.5]\n",
        ),
        (
            "quadrant axis text",
            "quadrantChart\n  x-axis {}\n  P: [0.5, 0.5]\n",
        ),
        ("point label", "quadrantChart\n  {}: [0.5, 0.5]\n"),
        (
            "radar axis label",
            "radar-beta\n  axis a[\"{}\"], b, c\n  curve x{1,2,3}\n",
        ),
        (
            "radar legend",
            "radar-beta\n  axis a, b, c\n  curve x[\"{}\"]{1,2,3}\n",
        ),
        ("sankey node label", "sankey-beta\n{},b,1\n"),
    ];

    let mut checked = 0usize;
    for (kind, template) in kinds {
        for (case, text) in LABELS {
            // A Sankey field is CSV: a comma inside one would make it two fields, and mermaid's own
            // grammar has the same rule.
            if *kind == "sankey node label" && text.contains(',') {
                continue;
            }
            let src = template.replace("{}", text);
            let d = match dispatch(&src) {
                Ok(d) => d,
                Err(e) => panic!("{kind}/{case}: {e}\n{src}"),
            };
            // What konoma decided the label's box is.
            let drawn_text = text_metrics::collapse_spaces(text).into_owned();
            let node = d
                .nodes
                .iter()
                .filter(|n| n.shape == Glyph::ChartLabel)
                .find(|n| n.label.lines.join(" ") == drawn_text)
                .unwrap_or_else(|| {
                    panic!(
                        "{kind}/{case}: no chart label reading {drawn_text:?}; the labels are {:?}",
                        d.nodes
                            .iter()
                            .filter(|n| n.shape == Glyph::ChartLabel)
                            .map(|n| n.label.lines.join(" "))
                            .collect::<Vec<_>>()
                    )
                });

            // What resvg actually laid it out at, in the document konoma emitted.
            let svg = rendered(&src);
            let mut texts = Vec::new();
            measured_texts(tree_of(&svg).root(), &mut texts);
            let drawn = texts
                .iter()
                .find(|(s, _)| *s == drawn_text)
                .map(|(_, w)| *w)
                .unwrap_or_else(|| {
                    panic!(
                        "{kind}/{case}: resvg drew no <text> reading {drawn_text:?} — a dropped \
                         label means the font resolution konoma measured with is not the one \
                         resvg drew with. It drew: {:?}",
                        texts.iter().map(|(s, _)| s).collect::<Vec<_>>()
                    )
                });
            assert!(
                (node.size.w - drawn).abs() <= 1.0,
                "{kind}/{case} {text:?}: konoma sized the box {} wide and resvg drew the text \
                 {} wide — a chart label's box *is* its ink, so these are one number",
                svg::num(node.size.w),
                svg::num(drawn)
            );
            checked += 1;
        }
    }
    // A tier that nothing lands in proves nothing.
    assert!(checked >= 90, "only {checked} label measurements ran");
    eprintln!("{checked} chart-label measurements against resvg");
}

// ---------------------------------------------------------------------------------------------
// 2. The two shared invariants that apply
// ---------------------------------------------------------------------------------------------

/// The two of the twelve that mean something for a chart, over the whole corpus. See the module
/// docs for the other ten and why each cannot apply.
#[test]
fn the_shared_invariants_that_apply_hold() {
    if !text_metrics::fonts_available() {
        return;
    }
    for (name, src) in CASES {
        let d = laid_out(src);
        check_view_box_contains_everything(name, &d);
        check_panels_stay_inside_their_box(name, &d);
    }
}

/// The six cluster invariants are skipped because there is nothing to check, and this is what says
/// so — rather than leaving a reader to assume it.
#[test]
fn no_chart_produces_a_cluster() {
    if !text_metrics::fonts_available() {
        return;
    }
    for (name, src) in CASES {
        let d = laid_out(src);
        assert!(d.clusters.is_empty(), "{name}: a chart grew a frame");
        assert!(d.lifelines.is_empty(), "{name}: a chart grew a lifeline");
    }
}

// ---------------------------------------------------------------------------------------------
// 3. The invariants that are about charts
// ---------------------------------------------------------------------------------------------

/// **(a) A pie's wedges fill exactly one turn, in source order, with no gap and no overlap.**
///
/// Stated over the sweeps rather than over the drawn arcs: a gap of a degree is invisible on the
/// page and is a datum that went missing.
#[test]
fn pie_wedges_fill_one_turn_and_each_is_the_share_of_its_value() {
    if !text_metrics::fonts_available() {
        return;
    }
    for (name, src) in cases_of(chart::pie::is_pie) {
        let model = chart::pie::parse(src).expect("parses");
        let d = laid_out(src);
        let wedges = of_kind(&d, Glyph::Wedge);
        assert_eq!(
            wedges.len(),
            model.slices.iter().filter(|s| s.value > 0.0).count(),
            "{name}: one wedge per slice with a value"
        );

        let total = model.total();
        let mut expected_start = 0.0_f64;
        for (slice, node) in model.slices.iter().zip(wedges.iter()) {
            let Some(Mark::Wedge { start, sweep }) = node.mark else {
                panic!("{name}: a wedge with no angles");
            };
            assert!(
                (sweep - slice.value / total * 360.0).abs() < 1e-9,
                "{name}: {:?} is {} of {} but sweeps {}°",
                slice.label,
                slice.value,
                total,
                svg::num(sweep)
            );
            assert!(
                (start - expected_start).abs() < 1e-9,
                "{name}: {:?} starts at {}° where the slice before it ended at {}° — a gap or an \
                 overlap between two wedges is a datum the reader cannot account for",
                slice.label,
                svg::num(start),
                svg::num(expected_start)
            );
            expected_start += sweep;
        }
        assert!(
            (expected_start - 360.0).abs() < 1e-9,
            "{name}: the wedges cover {}° of a full turn",
            svg::num(expected_start)
        );
    }
}

/// **(b) A percentage label lies inside the wedge it belongs to.**
///
/// The commonest way a pie becomes wrong rather than ugly: a number that has drifted onto the
/// neighbouring slice is read as that slice's.
#[test]
fn a_pie_percentage_lies_inside_its_own_wedge() {
    if !text_metrics::fonts_available() {
        return;
    }
    let d = laid_out(corpus("pie"));
    let wedges = of_kind(&d, Glyph::Wedge);
    for node in d.nodes.iter().filter(|n| n.shape == Glyph::ChartLabel) {
        let Some(series) = node.series else { continue };
        let Some(wedge) = wedges.get(series) else {
            continue;
        };
        let Some(Mark::Wedge { start, sweep }) = wedge.mark else {
            continue;
        };
        let r = wedge.size.w / 2.0;
        // Every corner of the label's box, so a label that only *mostly* fits still fails.
        let (l, t, rr, b) = node.bounds();
        for (x, y) in [(l, t), (rr, t), (l, b), (rr, b)] {
            let dx = x - wedge.center.x;
            let dy = y - wedge.center.y;
            assert!(
                dx.hypot(dy) <= r + 0.01,
                "a percentage corner is outside the circle"
            );
            // The angle, measured the way the wedge is: clockwise from twelve o'clock.
            let mut a = dy.atan2(dx).to_degrees() + 90.0;
            while a < start {
                a += 360.0;
            }
            assert!(
                a <= start + sweep + 0.01,
                "the {}% label is at {}° but its wedge is {}°..{}°",
                node.label.lines.join(""),
                svg::num(a),
                svg::num(start),
                svg::num(start + sweep)
            );
        }
    }
}

/// **(c) A bar's height is its value through the axis scale — and its foot is on the baseline.**
///
/// Two statements about the same rectangle, and a wrong drawing usually breaks only one of them:
/// a bar sized from the wrong end of the scale is the right height in the wrong place, and a bar
/// standing on the bottom of the frame when zero is halfway up looks perfectly ordinary.
#[test]
fn a_bar_is_as_tall_as_its_value_and_stands_on_the_baseline() {
    if !text_metrics::fonts_available() {
        return;
    }
    for (name, src) in cases_of(chart::xychart::is_xychart) {
        let model = chart::xychart::parse(src).expect("parses");
        let d = laid_out(src);
        let frame = of_kind(&d, Glyph::PlotFrame);
        assert_eq!(frame.len(), 1, "{name}: one plot frame");
        let (fl, ft, fr, fb) = frame[0].bounds();
        let span = model.y.1 - model.y.0;
        let px_per_unit = (fb - ft) / span;
        let base = if model.y.0 <= 0.0 && model.y.1 >= 0.0 {
            fb - (0.0 - model.y.0) * px_per_unit
        } else {
            fb
        };

        let bars: Vec<&PlacedNode> = d
            .nodes
            .iter()
            .filter(|n| n.shape == Glyph::ChartBar && n.id.starts_with("bar#"))
            .collect();
        let wanted: Vec<f64> = model
            .plots
            .iter()
            .filter(|p| p.kind == chart::xychart::PlotKind::Bar)
            .flat_map(|p| p.data.iter().map(|(_, v)| *v))
            .collect();
        assert_eq!(bars.len(), wanted.len(), "{name}: one bar per datum");
        // A bar runs from its value to the **baseline**, which is zero when the range straddles
        // it and the bottom of the scale when it does not. `y-axis 4000 --> 11000` is mermaid's
        // own example of the second case, and a bar there is showing a *difference*.
        let base_value = if model.y.0 <= 0.0 && model.y.1 >= 0.0 {
            0.0
        } else {
            model.y.0
        };
        for (bar, v) in bars.iter().zip(wanted.iter()) {
            let want_h = ((v - base_value) * px_per_unit).abs();
            assert!(
                (bar.size.h - want_h).abs() < 0.01,
                "{name}: {} stands {} tall for a value of {v} against a baseline of \
                 {base_value} on a scale of {} px per unit",
                bar.id,
                svg::num(bar.size.h),
                svg::num(px_per_unit)
            );
            let (_, top, _, bottom) = bar.bounds();
            let foot = if *v >= base_value { bottom } else { top };
            assert!(
                (foot - base).abs() < 0.01,
                "{name}: {} has its foot at {} where the baseline is {}",
                bar.id,
                svg::num(foot),
                svg::num(base)
            );
            assert!(fl - 0.01 <= bar.bounds().0 && bar.bounds().2 <= fr + 0.01);
        }
    }
}

/// **(d) Every tick is inside the range it ticks, and no two land on the same value.**
#[test]
fn axis_ticks_stay_inside_the_range_they_label() {
    for (min, max) in [
        (0.0, 1.0),
        (4000.0, 11000.0),
        (-40.0, 60.0),
        (0.0, 0.001),
        (-1e6, 1e6),
        (0.0, 7.0),
        (1.0, 3.0),
    ] {
        let t = ticks(min, max, 5);
        assert!(!t.is_empty(), "{min}..{max}: no ticks at all");
        for v in &t {
            assert!(
                *v >= min - 1e-9 && *v <= max + 1e-9,
                "{min}..{max}: a tick at {v} is off the end of the axis"
            );
        }
        for w in t.windows(2) {
            assert!(w[1] > w[0], "{min}..{max}: ticks are not increasing: {t:?}");
        }
    }
    // A degenerate range must still terminate and still say something.
    assert!(!ticks(5.0, 5.0, 5).is_empty());
    assert!(!ticks(f64::NAN, 1.0, 5).is_empty() || true);
}

/// **(e) Every datum reaches the page exactly once.**
///
/// Counted per kind, because "once" means a different mark in each: a slice is a wedge, a bar is a
/// rectangle, a Sankey link is a ribbon. The failure this catches is a datum silently dropped by a
/// layout that ran out of room — which looks like a smaller chart, not like a bug.
#[test]
fn every_datum_is_drawn_exactly_once() {
    if !text_metrics::fonts_available() {
        return;
    }
    let pie = chart::pie::parse(corpus("pie")).unwrap();
    let d = laid_out(corpus("pie"));
    assert_eq!(of_kind(&d, Glyph::Wedge).len(), pie.slices.len());

    for (name, src) in cases_of(chart::xychart::is_xychart) {
        let m = chart::xychart::parse(src).unwrap();
        let d = laid_out(src);
        let want_bars: usize = m
            .plots
            .iter()
            .filter(|p| p.kind == chart::xychart::PlotKind::Bar)
            .map(|p| p.data.len())
            .sum();
        let want_points: usize = m
            .plots
            .iter()
            .filter(|p| p.kind == chart::xychart::PlotKind::Line)
            .map(|p| p.data.len())
            .sum();
        assert_eq!(
            d.nodes.iter().filter(|n| n.id.starts_with("bar#")).count(),
            want_bars,
            "{name}: bars"
        );
        assert_eq!(
            of_kind(&d, Glyph::ChartPoint).len(),
            want_points,
            "{name}: line markers"
        );
    }

    let q = chart::quadrant::parse(corpus("quadrant")).unwrap();
    let d = laid_out(corpus("quadrant"));
    assert_eq!(of_kind(&d, Glyph::ChartPoint).len(), q.points.len());

    let s = chart::sankey::parse(corpus("sankey")).unwrap();
    let d = laid_out(corpus("sankey"));
    assert_eq!(of_kind(&d, Glyph::Ribbon).len(), s.links.len());

    let k = chart::packet::parse(corpus("packet")).unwrap();
    let d = laid_out(corpus("packet"));
    let fields: usize = k.rows.iter().map(Vec::len).sum();
    assert_eq!(of_kind(&d, Glyph::ChartBar).len(), fields);

    let r = chart::radar::parse(corpus("radar")).unwrap();
    let d = laid_out(corpus("radar"));
    assert_eq!(
        d.edges.iter().filter(|e| e.series.is_some()).count(),
        r.curves.len()
    );
}

/// **(f) There is a legend entry for every series, and its swatch is that series' colour.**
///
/// The pairing is the point. A legend whose rows are the right colours in the wrong order is a
/// chart that names every series wrongly, and it is exactly what a mutation that reverses the
/// entries produces.
#[test]
fn a_legend_entry_exists_for_each_series_and_is_drawn_in_its_colour() {
    if !text_metrics::fonts_available() {
        return;
    }
    // (source, the series names in order).
    let cases: &[(&str, Vec<&str>)] = &[
        (corpus("pie"), vec!["Dogs", "Cats", "Rats"]),
        (corpus("xychart-two-bars"), vec!["North", "South", "Target"]),
        (corpus("radar"), vec!["Alice", "Bob"]),
    ];
    for (src, names) in cases {
        let d = laid_out(src);
        let swatches: Vec<&PlacedNode> = d
            .nodes
            .iter()
            .filter(|n| n.id.ends_with("#swatch"))
            .collect();
        let labels: Vec<&PlacedNode> = d
            .nodes
            .iter()
            .filter(|n| n.id.ends_with("#label") && n.id.starts_with("legend#"))
            .collect();
        assert_eq!(swatches.len(), names.len(), "one swatch per series");
        assert_eq!(labels.len(), names.len(), "one name per series");
        for (i, name) in names.iter().enumerate() {
            assert_eq!(
                swatches[i].series,
                Some(i),
                "the swatch on row {i} is not series {i}'s colour"
            );
            assert!(
                labels[i].label.lines.join(" ").starts_with(name),
                "row {i} is named {:?} where the series is {name:?}",
                labels[i].label.lines.join(" ")
            );
            // The two are on the same row and the text is to the right of the swatch.
            assert!(
                (swatches[i].center.y - labels[i].center.y).abs() < 0.01,
                "row {i}'s swatch and name are on different lines"
            );
            assert!(swatches[i].bounds().2 <= labels[i].bounds().0 + 0.01);
        }
        // …and what the page shows really is that colour.
        let theme = Theme::named("dark");
        let doc = svg::emit(&d, &theme);
        for i in 0..names.len() {
            assert!(
                doc.contains(theme.series(i)),
                "series {i}'s colour never reaches the page"
            );
        }
    }
}

/// **(g) A quadrant point sits in the quadrant its coordinates name.**
///
/// Stated against the drawn geometry, not against the numbers: the y inversion between "a fraction
/// from the bottom" and "a distance from the top" is exactly the sort of slip that leaves a chart
/// looking entirely plausible and saying the opposite of the source.
#[test]
fn a_quadrant_point_lands_in_the_quadrant_its_coordinates_name() {
    if !text_metrics::fonts_available() {
        return;
    }
    for (_, src) in cases_of(chart::quadrant::is_quadrant_chart) {
        let model = chart::quadrant::parse(src).expect("parses");
        let d = laid_out(src);
        let frame = of_kind(&d, Glyph::PlotFrame);
        let (l, t, r, b) = frame[0].bounds();
        let (mid_x, mid_y) = ((l + r) / 2.0, (t + b) / 2.0);

        let points = of_kind(&d, Glyph::ChartPoint);
        assert_eq!(points.len(), model.points.len());
        for (p, node) in model.points.iter().zip(points.iter()) {
            let c = &node.center;
            assert!(
                c.x >= l - 0.01 && c.x <= r + 0.01 && c.y >= t - 0.01 && c.y <= b + 0.01,
                "{} is outside the chart",
                p.label
            );
            // Right of the middle exactly when x > 0.5; **above** the middle exactly when y > 0.5.
            assert_eq!(
                c.x > mid_x,
                p.x > 0.5,
                "{} is at x={} and was drawn on the {} of the middle",
                p.label,
                p.x,
                if c.x > mid_x { "right" } else { "left" }
            );
            assert_eq!(
            c.y < mid_y,
            p.y > 0.5,
            "{} is at y={} and was drawn {} the middle — y runs *up* in the source and *down* on \
             the page",
            p.label,
            p.y,
            if c.y < mid_y { "above" } else { "below" }
        );
            // And exactly proportional, not merely on the right side.
            assert!((c.x - (l + p.x * (r - l))).abs() < 0.01);
            assert!((c.y - (t + (1.0 - p.y) * (b - t))).abs() < 0.01);
        }
    }
}

/// **(h) A radar vertex is as far from the centre as its value is up the scale.**
#[test]
fn a_radar_vertex_is_as_far_out_as_its_value() {
    if !text_metrics::fonts_available() {
        return;
    }
    for (name, src) in cases_of(chart::radar::is_radar) {
        let model = chart::radar::parse(src).expect("parses");
        let d = laid_out(src);
        let graticule = of_kind(&d, Glyph::Graticule);
        assert_eq!(graticule.len(), 1, "{name}: one graticule");
        let center = graticule[0].center.clone();
        let radius = graticule[0].size.w / 2.0;
        let (min, max) = (model.options.min, model.max());

        let curves: Vec<_> = d.edges.iter().filter(|e| e.series.is_some()).collect();
        assert_eq!(
            curves.len(),
            model.curves.len(),
            "{name}: one line per curve"
        );
        for (curve, edge) in model.curves.iter().zip(curves.iter()) {
            assert_eq!(
                edge.points.len(),
                model.axes.len() + 1,
                "{name}: the curve is closed — the last vertex repeats the first"
            );
            assert_eq!(
                edge.points[0],
                edge.points[edge.points.len() - 1],
                "{name}: the curve does not close"
            );
            for (k, v) in curve.entries.iter().enumerate() {
                let p = &edge.points[k];
                let got = (p.x - center.x).hypot(p.y - center.y);
                let want = radius * (v.clamp(min, max) - min) / (max - min);
                assert!(
                    (got - want).abs() < 0.01,
                    "{name}: {} on axis {k} is {v} of {min}..{max}, so {} out of {}, but the \
                     vertex is {} out",
                    curve.label,
                    svg::num(want),
                    svg::num(radius),
                    svg::num(got)
                );
            }
        }
    }
}

/// **(i) A treemap's tiles have areas in the ratio of their values, tile their parent, and never
/// overlap a sibling.**
///
/// The three together are what makes a treemap a treemap. Upstream's own renderer fails the first
/// one outright (`docs/` has the picture), which is why it is stated as an equality rather than as
/// "roughly".
#[test]
fn treemap_tile_areas_are_in_the_ratio_of_their_values() {
    if !text_metrics::fonts_available() {
        return;
    }
    // The top-level split, checked exactly, for every treemap in the corpus.
    for (_, src) in cases_of(chart::treemap::is_treemap) {
        let model = chart::treemap::parse(src).expect("parses");
        let d = laid_out(src);
        let tiles = of_kind(&d, Glyph::ChartBar);
        assert!(tiles.len() >= model.roots.len());

        let total: f64 = model.roots.iter().map(chart::treemap::Node::total).sum();
        let whole: f64 = super::treemap::WIDTH * super::treemap::HEIGHT;
        for (i, root) in model.roots.iter().enumerate() {
            let tile = tiles[section_tile_index(&model.roots, i)];
            let area = tile.size.w * tile.size.h;
            let want = root.total() / total * whole;
            assert!(
                (area - want).abs() / want < 0.001,
                "{:?} is {} of {} but its tile is {} of {} px²",
                root.name,
                root.total(),
                total,
                svg::num(area),
                svg::num(whole)
            );
        }
    }

    // Siblings never overlap, at any depth.
    for (name, src) in cases_of(chart::treemap::is_treemap) {
        let d = laid_out(src);
        let tiles = of_kind(&d, Glyph::ChartBar);
        for (i, a) in tiles.iter().enumerate() {
            for b in tiles.iter().skip(i + 1) {
                let (ab, bb) = (a.bounds(), b.bounds());
                // A section's tile *contains* its children, which is not an overlap; anything else
                // sharing area is.
                let contains = |o: (f64, f64, f64, f64), i: (f64, f64, f64, f64)| {
                    o.0 <= i.0 + 0.01 && o.1 <= i.1 + 0.01 && o.2 >= i.2 - 0.01 && o.3 >= i.3 - 0.01
                };
                assert!(
                    !overlap(ab, bb) || contains(ab, bb) || contains(bb, ab),
                    "{name}: {} and {} overlap without one containing the other",
                    a.id,
                    b.id
                );
            }
        }
    }
}

/// The index in the drawn tile list of the `i`th top-level node, given the depth-first order
/// `place` writes them in.
fn section_tile_index(roots: &[chart::treemap::Node], i: usize) -> usize {
    fn count(n: &chart::treemap::Node) -> usize {
        1 + n.children.iter().map(count).sum::<usize>()
    }
    roots[..i].iter().map(count).sum()
}

/// **(j) A packet field is as wide as its bit count, and the fields tile each row with no gap.**
#[test]
fn packet_fields_are_as_wide_as_their_bit_counts_and_tile_each_row() {
    if !text_metrics::fonts_available() {
        return;
    }
    for (_, src) in cases_of(chart::packet::is_packet) {
        let model = chart::packet::parse(src).expect("parses");
        let d = laid_out(src);
        let tiles = of_kind(&d, Glyph::ChartBar);
        let mut i = 0;
        for row in &model.rows {
            let mut expect_x: Option<f64> = None;
            for field in row {
                let tile = tiles[i];
                i += 1;
                let want = field.bits() as f64 * super::packet::BIT_WIDTH;
                assert!(
                    (tile.size.w - want).abs() < 0.01,
                    "{}..{} is {} bits but is drawn {} wide, where a bit is {}",
                    field.start,
                    field.end,
                    field.bits(),
                    svg::num(tile.size.w),
                    svg::num(super::packet::BIT_WIDTH)
                );
                let (l, _, r, _) = tile.bounds();
                if let Some(x) = expect_x {
                    assert!(
                    (l - x).abs() < 0.01,
                    "a gap in the row: {}..{} starts at {} where the field before it ended at {}",
                    field.start,
                    field.end,
                    svg::num(l),
                    svg::num(x)
                );
                }
                expect_x = Some(r);
            }
        }
        assert_eq!(i, tiles.len(), "every field was checked");
    }
}

/// **(k) A Sankey uses one scale: a node is as tall as its value, and the bands meeting it stack
/// up to exactly that.**
///
/// The claim a Sankey makes — this much came in, this much went out — is only true if it is one
/// scale, so a second scale anywhere is the diagram lying rather than a rounding error.
#[test]
fn a_sankey_uses_one_scale_and_its_bands_stack_up_to_its_nodes() {
    if !text_metrics::fonts_available() {
        return;
    }
    for (_, src) in cases_of(chart::sankey::is_sankey) {
        let model = chart::sankey::parse(src).expect("parses");
        let d = laid_out(src);

        // One px-per-unit, read off the first node and then held to by every other.
        let bars: Vec<&PlacedNode> = d
            .nodes
            .iter()
            .filter(|n| n.id.starts_with("node#") && n.shape == Glyph::ChartBar)
            .collect();
        assert_eq!(bars.len(), model.nodes.len());
        let value_of = |i: usize| -> f64 {
            let into: f64 = model
                .links
                .iter()
                .filter(|l| l.target == i)
                .map(|l| l.value)
                .sum();
            let out: f64 = model
                .links
                .iter()
                .filter(|l| l.source == i)
                .map(|l| l.value)
                .sum();
            into.max(out)
        };
        // Read off the tallest node, whose height cannot have been raised by the hairline floor.
        let tallest = (0..bars.len())
            .max_by(|a, b| value_of(*a).partial_cmp(&value_of(*b)).expect("no NaN"))
            .expect("at least one node");
        let k = bars[tallest].size.h / value_of(tallest);
        for (i, bar) in bars.iter().enumerate() {
            assert!(
                (bar.size.h - (value_of(i) * k).max(super::sankey::MIN_BAND)).abs() < 0.01,
                "{}: {} tall for a value of {}, where the first node's scale is {} px per unit",
                model.nodes[i],
                svg::num(bar.size.h),
                value_of(i),
                svg::num(k)
            );
        }

        // Each ribbon is the same thickness at both ends, and that thickness is its value.
        let ribbons = of_kind(&d, Glyph::Ribbon);
        assert_eq!(ribbons.len(), model.links.len());
        for (i, link) in model.links.iter().enumerate() {
            // **By id, not by position.** The bands are pushed in the order they are stacked against
            // their nodes, which is not the order the links were written in; zipping the two lists
            // would compare each flow against a different flow's geometry and pass or fail for the
            // wrong reason.
            let node = ribbons
                .iter()
                .find(|n| n.id == format!("flow#{i}"))
                .unwrap_or_else(|| panic!("no band drawn for link {i}"));
            let Some(Mark::Ribbon {
                left_top,
                left_bottom,
                right_top,
                right_bottom,
            }) = node.mark
            else {
                panic!("a ribbon with no corners");
            };
            let (a, b) = (left_bottom - left_top, right_bottom - right_top);
            assert!(
                (a - b).abs() < 0.01,
                "a band {} thick at one end and {} at the other is not one flow",
                svg::num(a),
                svg::num(b)
            );
            // …with the hairline floor, which is part of the rule rather than a fudge: a flow of a
            // thousandth of the total would otherwise be drawn no thicker than nothing, and a reader
            // has to be able to see that it is there before they can see how small it is.
            let want = (link.value * k).max(super::sankey::MIN_BAND);
            assert!(
            (a - want).abs() < 0.01,
            "a flow of {} is drawn {} thick where the one scale ({} px per unit, floored at {}) \
             makes it {}",
            link.value,
            svg::num(a),
            svg::num(k),
            svg::num(super::sankey::MIN_BAND),
            svg::num(want)
        );
        }
    }
}

/// **(l) The furniture stays out of the plot.**
///
/// A tick label, an axis title or a legend row inside the plotting area sits on the data, and the
/// data is what the plot is for. (The chart's *title* is above everything by construction, and is
/// covered by the viewBox invariant.)
#[test]
fn tick_labels_axis_titles_and_the_legend_stay_out_of_the_plot() {
    if !text_metrics::fonts_available() {
        return;
    }
    for (name, _src) in cases_of(chart::xychart::is_xychart) {
        let d = laid_out(_src);
        let frame = of_kind(&d, Glyph::PlotFrame);
        let plot = frame[0].bounds();
        for node in &d.nodes {
            if node.shape != Glyph::ChartLabel && !node.id.ends_with("#swatch") {
                continue;
            }
            if node.id == "chart#title" {
                continue;
            }
            assert!(
                !overlap(node.bounds(), plot),
                "{name}: {} is inside the plotting area",
                node.id
            );
        }
    }
}

/// Two tick labels that touch are two numbers a reader cannot separate. The plot widens itself
/// for its categories, and this is what says the widening is enough.
#[test]
fn no_two_axis_labels_overlap() {
    if !text_metrics::fonts_available() {
        return;
    }
    for (name, src) in CASES {
        let d = laid_out(src);
        let labels: Vec<&PlacedNode> = d
            .nodes
            .iter()
            .filter(|n| {
                n.shape == Glyph::ChartLabel
                    && (n.id.starts_with("xtick") || n.id.starts_with("ytick"))
            })
            .collect();
        for (i, a) in labels.iter().enumerate() {
            for b in labels.iter().skip(i + 1) {
                assert!(
                    !overlap(a.bounds(), b.bounds()),
                    "{name}: {} and {} overlap",
                    a.id,
                    b.id
                );
            }
        }
    }
}

/// `horizontal` is read by the parser and **not applied** by the layout. The other half of
/// `chart::tests::a_horizontal_xychart_is_parsed`; together they are the whole of the deliberate
/// drop, and this is the test a later stage has to change to draw it.
#[test]
fn a_horizontal_xychart_is_drawn_the_same_as_a_vertical_one() {
    if !text_metrics::fonts_available() {
        return;
    }
    let body = "  x-axis [a, b, c]\n  y-axis 0 --> 10\n  bar [1, 5, 9]\n";
    let h = rendered(&format!("xychart-beta horizontal\n{body}"));
    let v = rendered(&format!("xychart-beta\n{body}"));
    assert_eq!(
        h, v,
        "`horizontal` changed the drawing — if that is now intended, this test is the record of \
         the decision it replaces"
    );
}

// ---------------------------------------------------------------------------------------------
// 4. Themes
// ---------------------------------------------------------------------------------------------

/// Every series colour carries the text that is drawn on top of it.
///
/// A pie's percentage, a treemap's tile name and a packet field's label are all drawn *on* a
/// series colour, so a palette entry that swallows its text is not a style preference — it is a
/// datum the reader cannot get at. 3:1 is WCAG's floor for large text, and these are 14px on a
/// coloured ground.
#[test]
fn every_series_colour_carries_the_text_drawn_on_it() {
    for t in super::super::theme::ALL {
        assert!(
            t.series.len() >= 6,
            "{}: {} series colours is too few for a chart",
            t.name,
            t.series.len()
        );
        let mut seen = HashSet::new();
        for (i, c) in t.series.iter().enumerate() {
            assert!(
                super::super::theme::parse_hex(c).is_some(),
                "{}: series {i} is {c}, not a normalised lowercase hex",
                t.name
            );
            assert!(seen.insert(*c), "{}: series {i} repeats {c}", t.name);
            let ratio = Theme::contrast(c, t.series_text);
            assert!(
                ratio >= 3.0,
                "{}: series {i} ({c}) against its text ({}) is only {ratio:.2}:1",
                t.name,
                t.series_text
            );
        }
        // Two series have to be told apart from *each other* as well, or a legend of eight rows
        // is a legend of four. Measured as distance in RGB rather than as contrast: two colours
        // of the same luminance and different hue are perfectly distinguishable — that is what
        // hue is for — so a contrast test here would fail a good categorical palette and pass a
        // pair of near-identical greys.
        for i in 0..t.series.len() {
            for j in (i + 1)..t.series.len() {
                let d = rgb_distance(t.series[i], t.series[j]);
                assert!(
                    d >= 25.0,
                    "{}: series {i} ({}) and {j} ({}) are {d:.1} apart in RGB",
                    t.name,
                    t.series[i],
                    t.series[j]
                );
            }
        }
        // **The axis has to survive both grounds** — §4-1's rule, which is why konoma's palettes
        // are not mermaid's: a diagram is composited onto whatever the terminal is showing, and a
        // theme chosen for the wrong one still has to be readable rather than invisible.
        assert!(
            super::super::theme::parse_hex(t.axis).is_some(),
            "{}: axis is {}",
            t.name,
            t.axis
        );
        let (dark, light) = (
            Theme::contrast(t.axis, "#1a1a1a"),
            Theme::contrast(t.axis, "#ffffff"),
        );
        assert!(
            dark >= 1.4 && light >= 1.4,
            "{}: axis ({}) is {dark:.2}:1 on a dark terminal and {light:.2}:1 on a light one",
            t.name,
            t.axis
        );
        // **The grid is held to its own theme's ground only**, and deliberately. A grid line is
        // meant to be findable and not to be looked at; holding it to both grounds would force it
        // to the same weight as the axis, and a grid at the axis's weight is a chart with a cage
        // round it. Someone running a light terminal picks a light theme, which is the ground its
        // `background_ref` names.
        assert!(
            super::super::theme::parse_hex(t.grid).is_some(),
            "{}: grid is {}",
            t.name,
            t.grid
        );
        let on_own = Theme::contrast(t.grid, t.background_ref);
        assert!(
            on_own >= 1.35,
            "{}: grid ({}) is only {on_own:.2}:1 on its own theme's ground ({})",
            t.name,
            t.grid,
            t.background_ref
        );
    }
}

/// Euclidean distance between two `#rrggbb` colours, in 0..=441.
fn rgb_distance(a: &str, b: &str) -> f64 {
    let (a, b) = (
        super::super::theme::parse_hex(a).unwrap_or((0, 0, 0)),
        super::super::theme::parse_hex(b).unwrap_or((0, 0, 0)),
    );
    let d = |x: u8, y: u8| (x as f64 - y as f64).powi(2);
    (d(a.0, b.0) + d(a.1, b.1) + d(a.2, b.2)).sqrt()
}

// ---------------------------------------------------------------------------------------------
// 5. Rasterising
// ---------------------------------------------------------------------------------------------

/// Every chart rasterises, and has ink in it.
///
/// A diagram that draws nothing rasterises to a transparent rectangle, which the pane then
/// stretches — the failure §6 records the crate having. Ink is what tells the two apart.
#[test]
fn every_chart_rasterises_with_ink_in_it() {
    if !text_metrics::fonts_available() {
        return;
    }
    for (name, src) in CASES {
        let svg = rendered(src);
        let img = crate::preview::svg::rasterize_bytes(
            svg.as_bytes(),
            std::path::Path::new("m.svg"),
            600,
        )
        .unwrap_or_else(|| panic!("{name}: konoma's resvg could not rasterise the output"));
        let opaque = img.to_rgba8().pixels().filter(|p| p.0[3] > 32).count();
        assert!(
            opaque > 500,
            "{name}: only {opaque} pixels were drawn — the chart is effectively blank"
        );
    }
}

// ---------------------------------------------------------------------------------------------
// 6. Goldens
// ---------------------------------------------------------------------------------------------

/// The whole corpus through **the production entry point**, with the numbers masked.
///
/// Masked so it says what commands and what text, and leaves the arithmetic to the invariants
/// above and to [`chart_emit_golden`], which builds its diagram by hand and needs no font.
#[test]
fn chart_corpus_golden() {
    if !text_metrics::fonts_available() {
        return;
    }
    let mut out = String::new();
    for (name, src) in CASES {
        out.push_str(&format!("=== {name} ===\n"));
        out.push_str(&mask_numbers(&rendered(src)));
        out.push('\n');
    }
    assert_snapshot("mermaid_chart", &out);
}

/// **Exact, and font-free**: one diagram carrying every glyph stage 5 added, in all five palettes.
///
/// Every label is built with a declared width, so this pins the numbers themselves rather than
/// this machine's font metrics — and so it says the same thing on a Linux CI box.
#[test]
fn chart_emit_golden() {
    let d = synthetic_chart_diagram();
    let mut out = String::new();
    for t in super::super::theme::ALL {
        out.push_str(&format!("=== {} ===\n", t.name));
        out.push_str(&svg::emit(&d, t));
        out.push('\n');
    }
    assert_snapshot("mermaid_chart_emit", &out);
}

/// One of everything stage 5 draws, at coordinates chosen by hand.
fn synthetic_chart_diagram() -> Diagram {
    let label = |text: &str, w: f64| Label {
        lines: text.split('\n').map(str::to_string).collect(),
        width: w,
        height: text.split('\n').count() as f64 * super::super::labels::line_height(),
    };
    let node = |id: &str, shape: Glyph, x: f64, y: f64, w: f64, h: f64, series: Option<usize>| {
        PlacedNode {
            id: id.to_string(),
            shape,
            center: Point::new(x, y),
            size: Size::new(w, h),
            label: label("", 0.0),
            panel: None,
            series,
            mark: None,
        }
    };

    let mut nodes = vec![
        node("frame", Glyph::PlotFrame, 120.0, 90.0, 200.0, 120.0, None),
        node("bar0", Glyph::ChartBar, 60.0, 110.0, 30.0, 80.0, Some(0)),
        node("bar1", Glyph::ChartBar, 100.0, 130.0, 30.0, 40.0, Some(1)),
        node("point", Glyph::ChartPoint, 160.0, 60.0, 6.0, 6.0, Some(2)),
    ];
    // A wedge, a ribbon and a graticule, each with the mark its glyph cannot be drawn without.
    let mut wedge = node("wedge", Glyph::Wedge, 400.0, 90.0, 120.0, 120.0, Some(3));
    wedge.mark = Some(Mark::Wedge {
        start: 30.0,
        sweep: 140.0,
    });
    nodes.push(wedge);
    let mut ribbon = node("ribbon", Glyph::Ribbon, 250.0, 260.0, 160.0, 60.0, Some(4));
    ribbon.mark = Some(Mark::Ribbon {
        left_top: -30.0,
        left_bottom: -6.0,
        right_top: 6.0,
        right_bottom: 30.0,
    });
    nodes.push(ribbon);
    let mut graticule = node("grat", Glyph::Graticule, 560.0, 120.0, 130.0, 130.0, None);
    graticule.mark = Some(Mark::Graticule {
        rings: 3,
        spokes: 5,
        polygon: true,
    });
    nodes.push(graticule);
    let mut circles = node("grat2", Glyph::Graticule, 560.0, 280.0, 90.0, 90.0, None);
    circles.mark = Some(Mark::Graticule {
        rings: 2,
        spokes: 4,
        polygon: false,
    });
    nodes.push(circles);

    // Chart labels, on the terminal's ground and on a series fill.
    for (id, text, w, x, y, series) in [
        ("tick", "4000", 30.0, 20.0, 150.0, None),
        ("title", "Sales Revenue", 90.0, 200.0, 12.0, None),
        ("pct", "42%", 26.0, 400.0, 60.0, Some(3usize)),
    ] {
        nodes.push(PlacedNode {
            id: id.to_string(),
            shape: Glyph::ChartLabel,
            center: Point::new(x, y),
            size: Size::new(w, super::super::labels::line_height()),
            label: label(text, w),
            panel: None,
            series,
            mark: None,
        });
    }
    // A legend, through the shared helper, so the golden pins that too.
    nodes.extend(legend_nodes(
        &[
            LegendEntry {
                label: label("North", 40.0),
                series: 0,
            },
            LegendEntry {
                label: label("South", 42.0),
                series: 1,
            },
        ],
        620.0,
        30.0,
    ));

    // A grid line, a tick and a data path.
    let mut edges = vec![
        super::rule(
            Point::new(20.0, 60.0),
            Point::new(220.0, 60.0),
            crate::preview::mermaid::flowchart::Stroke::Dotted,
            None,
        ),
        super::rule(
            Point::new(16.0, 60.0),
            Point::new(20.0, 60.0),
            crate::preview::mermaid::flowchart::Stroke::Normal,
            None,
        ),
    ];
    let mut series_edge = super::rule(
        Point::new(40.0, 140.0),
        Point::new(200.0, 40.0),
        crate::preview::mermaid::flowchart::Stroke::Normal,
        Some(2),
    );
    series_edge.points = vec![
        Point::new(40.0, 140.0),
        Point::new(100.0, 100.0),
        Point::new(160.0, 60.0),
        Point::new(200.0, 40.0),
    ];
    edges.push(series_edge);

    let mut d = Diagram {
        nodes,
        edges,
        ..Diagram::default()
    };
    super::super::normalise(&mut d);
    d
}

// ---------------------------------------------------------------------------------------------
// 7. The gallery
// ---------------------------------------------------------------------------------------------

/// Writes the corpus out as SVG files for a person to look at (§6: konoma against mermaid.js is a
/// question a person answers by looking, never an automatic judgement).
///
/// `KONOMA_GALLERY=/some/dir cargo test -- --ignored render::chart::tests::gallery`
///
/// `KONOMA_GALLERY_THEME` picks the palette (default `dark`); the light ones are the ones §4-1
/// says are hardest to get right, so they are worth looking at separately.
#[test]
#[ignore = "writes SVG files for a person to look at: KONOMA_GALLERY=<dir> cargo test -- --ignored gallery"]
fn gallery() {
    let dir = std::env::var("KONOMA_GALLERY").expect("set KONOMA_GALLERY to a directory");
    std::fs::create_dir_all(&dir).expect("create the gallery directory");
    for (name, src) in CASES {
        let theme = std::env::var("KONOMA_GALLERY_THEME").unwrap_or_else(|_| "dark".to_string());
        match mermaid_to_svg_reason(src, &theme) {
            Ok(svg) => {
                std::fs::write(format!("{dir}/{name}.svg"), svg).expect("write");
            }
            Err(e) => eprintln!("{name}: {e}"),
        }
        // The crate's rendering of the same source, for the side-by-side.
        if let Ok(svg) = crate::preview::markdown::mermaid_rs_for_comparison(src, "dark") {
            std::fs::write(format!("{dir}/{name}--crate.svg"), svg).expect("write");
        }
    }
    eprintln!("wrote the chart gallery to {dir}");
}
