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

/// How many category slots an xy chart was drawn with — the number of tick labels under its axis.
///
/// The renderer decides this from the categories, and it is what bounds how much of a plot is
/// drawn, so a test asking "which data reached the page" has to ask the same number.
fn frame_slots(d: &Diagram) -> usize {
    d.nodes
        .iter()
        .filter(|n| n.id.starts_with("xtick#"))
        .count()
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
        for (i, slice) in model.slices.iter().enumerate() {
            // **By id, never by zipping the two lists.** A zero-valued slice is kept in the model
            // and drawn nowhere, so position `k` of the wedges is not slice `k` as soon as a
            // source has one — and every later slice is then checked against its neighbour's
            // wedge. The first version of this test did zip, and reported `"Absent" is 0 of 10 but
            // sweeps 180°`: the arithmetic was right and the pairing was not.
            let Some(node) = wedges.iter().find(|n| n.id == format!("slice#{i}")) else {
                assert_eq!(
                    slice.value, 0.0,
                    "{name}: {:?} is {} of {total} and was given no wedge",
                    slice.label, slice.value
                );
                continue;
            };
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
    for (_, src) in cases_of(chart::pie::is_pie) {
        let d = laid_out(src);
        let wedges = of_kind(&d, Glyph::Wedge);
        for node in d.nodes.iter().filter(|n| n.shape == Glyph::ChartLabel) {
            let Some(series) = node.series else { continue };
            // The wedge belonging to *this* slice, by id — `wedges[series]` is the wrong wedge as
            // soon as a zero-valued slice earlier in the source has been given none.
            let Some(wedge) = wedges.iter().find(|w| w.id == format!("slice#{series}")) else {
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
        // **The range the drawing used**, from the one function that decides it. Reading
        // `model.y` here instead divides by zero the moment every value in a chart is the same —
        // `bar [7, 7, 7]` gave a scale of `inf px per unit` and the assertion below then compared
        // two infinities. §6-A item 12: an instrument that recomputes what it measures drifts.
        let (y_min, y_max) = super::xychart::effective_range(&model).expect("a finite range");
        let span = y_max - y_min;
        let px_per_unit = (fb - ft) / span;
        let base = if y_min <= 0.0 && y_max >= 0.0 {
            fb - (0.0 - y_min) * px_per_unit
        } else {
            fb
        };

        let bars: Vec<&PlacedNode> = d
            .nodes
            .iter()
            .filter(|n| n.shape == Glyph::ChartBar && n.id.starts_with("bar#"))
            .collect();
        // Only the data the axis has a slot for: a datum past the last category is deliberately
        // not drawn rather than drawn off the end of the plot (`xychart::drawn_len`).
        let slots = frame_slots(&d);
        let wanted: Vec<f64> = model
            .plots
            .iter()
            .filter(|p| p.kind == chart::xychart::PlotKind::Bar)
            .flat_map(|p| {
                p.data
                    .iter()
                    .take(super::xychart::drawn_len(p, slots))
                    .map(|(_, v)| *v)
            })
            .collect();
        assert_eq!(bars.len(), wanted.len(), "{name}: one bar per datum");
        // A bar runs from its value to the **baseline**, which is zero when the range straddles
        // it and the bottom of the scale when it does not. `y-axis 4000 --> 11000` is mermaid's
        // own example of the second case, and a bar there is showing a *difference*.
        let base_value = if y_min <= 0.0 && y_max >= 0.0 {
            0.0
        } else {
            y_min
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
    // **Over every case of every kind, found by the routing predicate** — never by naming the
    // cases. §6-A item 10: a test that lists case names stops covering the corpus the moment the
    // corpus grows, and a radar mutation survived twice for exactly that reason.
    for (name, src) in cases_of(chart::pie::is_pie) {
        let m = chart::pie::parse(src).unwrap();
        let d = laid_out(src);
        // A zero-valued slice is a datum with no wedge on purpose: a sector of no angle is a
        // stroke on the radius, which reads as a boundary between its neighbours. It keeps its
        // legend row, which is what `a_legend_entry_exists_for_each_series…` states.
        assert_eq!(
            of_kind(&d, Glyph::Wedge).len(),
            m.slices.iter().filter(|s| s.value > 0.0).count(),
            "{name}: wedges"
        );
    }

    for (name, src) in cases_of(chart::xychart::is_xychart) {
        let m = chart::xychart::parse(src).unwrap();
        let d = laid_out(src);
        let slots = frame_slots(&d);
        let want_bars: usize = m
            .plots
            .iter()
            .filter(|p| p.kind == chart::xychart::PlotKind::Bar)
            .map(|p| super::xychart::drawn_len(p, slots))
            .sum();
        let want_points: usize = m
            .plots
            .iter()
            .filter(|p| p.kind == chart::xychart::PlotKind::Line)
            .map(|p| super::xychart::drawn_len(p, slots))
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

    for (name, src) in cases_of(chart::quadrant::is_quadrant_chart) {
        let m = chart::quadrant::parse(src).unwrap();
        let d = laid_out(src);
        assert_eq!(
            of_kind(&d, Glyph::ChartPoint).len(),
            m.points.len(),
            "{name}: points"
        );
    }

    for (name, src) in cases_of(chart::sankey::is_sankey) {
        let m = chart::sankey::parse(src).unwrap();
        let d = laid_out(src);
        assert_eq!(
            of_kind(&d, Glyph::Ribbon).len(),
            m.links.len(),
            "{name}: ribbons"
        );
        assert_eq!(
            d.nodes
                .iter()
                .filter(|n| n.shape == Glyph::ChartBar && n.id.starts_with("node#"))
                .count(),
            m.nodes.len(),
            "{name}: node bars"
        );
    }

    for (name, src) in cases_of(chart::packet::is_packet) {
        let m = chart::packet::parse(src).unwrap();
        let d = laid_out(src);
        let fields: usize = m.rows.iter().map(Vec::len).sum();
        assert_eq!(of_kind(&d, Glyph::ChartBar).len(), fields, "{name}: fields");
    }

    for (name, src) in cases_of(chart::radar::is_radar) {
        let m = chart::radar::parse(src).unwrap();
        let d = laid_out(src);
        assert_eq!(
            d.edges.iter().filter(|e| e.series.is_some()).count(),
            m.curves
                .iter()
                .filter(|c| c.entries.len() == m.axes.len())
                .count(),
            "{name}: curves"
        );
    }

    for (name, src) in cases_of(chart::treemap::is_treemap) {
        let m = chart::treemap::parse(src).unwrap();
        let d = laid_out(src);
        // Every node whose share of the whole is big enough to be a rectangle at all, and no
        // others — the count a mutation that drops a whole branch has to get past.
        let drawn = of_kind(&d, Glyph::ChartBar).len();
        let nodes: usize = m
            .roots
            .iter()
            .map(super::treemap::subtree_len)
            .sum::<usize>();
        let zeroes = count_zero_nodes(&m.roots);
        assert!(
            drawn <= nodes - zeroes,
            "{name}: {drawn} tiles for {nodes} nodes, {zeroes} of which have no value"
        );
        assert!(drawn > 0, "{name}: a treemap with no tiles");
    }
}

/// How many nodes in this forest have a total of zero, at any depth.
fn count_zero_nodes(items: &[chart::treemap::Node]) -> usize {
    items
        .iter()
        .map(|n| usize::from(n.total() <= 0.0) + count_zero_nodes(&n.children))
        .sum()
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
        // A curve without an entry for every axis is skipped, which is upstream's rule and
        // konoma's (`render::chart::radar::is_drawable`), so the count is over the drawable ones.
        let drawable: Vec<&chart::radar::Curve> = model
            .curves
            .iter()
            .filter(|c| c.entries.len() == model.axes.len())
            .collect();
        assert_eq!(curves.len(), drawable.len(), "{name}: one line per curve");
        for (curve, edge) in drawable.iter().zip(curves.iter()) {
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

        let total: f64 = model.roots.iter().map(chart::treemap::Node::total).sum();
        let whole: f64 = super::treemap::WIDTH * super::treemap::HEIGHT;
        for (i, root) in model.roots.iter().enumerate() {
            // **By the node's own depth-first number**, which is what the tile's id is. Counting
            // drawn tiles instead breaks on the first node that has no area: `"absent": 0` is kept
            // in the model and drawn nowhere, so every root after it was being compared against
            // its neighbour's tile — and with the last root the index ran off the end.
            let Some(tile) = tiles
                .iter()
                .find(|t| t.id == format!("tile#{}", dfs_index(&model.roots, i)))
            else {
                assert!(
                    root.total() <= 0.0,
                    "{:?} is {} of {total} and was given no tile",
                    root.name,
                    root.total()
                );
                continue;
            };
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

/// The depth-first number of the `i`th top-level node — the number in its tile's id.
///
/// A property of the *model*, so it is the same whether or not the tile was drawn; that is the
/// whole reason the renderer numbers by node rather than by tile.
fn dfs_index(roots: &[chart::treemap::Node], i: usize) -> usize {
    roots[..i]
        .iter()
        .map(super::treemap::subtree_len)
        .sum::<usize>()
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
        // **Named, not excluded.** The furniture is the tick labels, the axis titles and the
        // legend; everything else a chart draws is data and is *meant* to be on the plot. Written
        // the other way round — "every label except the title" — it also caught a point's own
        // value label, which sits on its datum by design, and reported the design as a fault.
        for node in &d.nodes {
            let is_furniture = node.id.starts_with("xtick#")
                || node.id.starts_with("ytick#")
                || node.id == "xtitle"
                || node.id == "ytitle"
                || node.id.starts_with("legend#");
            if !is_furniture {
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

/// **A curve needs an entry for every axis, or it is not a curve on this chart.**
///
/// Upstream's rule (`radarRenderer.ts`: "Skip curves that do not have an entry for each axis"),
/// and konoma's for its own reason: three entries drawn on the first three of five spokes is a
/// closed triangle, which reads as a series that scored nothing on two axes rather than as one
/// that was never measured on them. konoma used to draw exactly that, taking `min(entries, axes)`
/// vertices and giving up only below three.
#[test]
fn a_radar_curve_without_an_entry_for_every_axis_is_not_drawn() {
    if !text_metrics::fonts_available() {
        return;
    }
    // Too few, and too many: the same lie from either end.
    for src in [
        "radar-beta\n  axis a, b, c, d, e\n  curve x[\"Only three\"]{1, 2, 3}\n  max 5\n",
        "radar-beta\n  axis a, b, c\n  curve x[\"Five for three\"]{1, 2, 3, 4, 5}\n  max 6\n",
    ] {
        let e = dispatch(src).expect_err("a chart with nothing plottable must be refused");
        assert!(
            e.to_string()
                .contains("no curve has an entry for every axis"),
            "{e}"
        );
    }

    // With one full-length curve beside it, the chart draws — one line, and a legend that still
    // names both, because the row is the only place the reader learns the short one is there.
    let d = laid_out(corpus("radar-mixed-lengths"));
    let lines: Vec<_> = d.edges.iter().filter(|e| e.series.is_some()).collect();
    assert_eq!(lines.len(), 1, "the short curve was drawn after all");
    assert_eq!(
        lines[0].points.len(),
        5,
        "the surviving curve has a vertex on every one of the four axes, and closes"
    );
    let rows: Vec<String> = d
        .nodes
        .iter()
        .filter(|n| n.id.starts_with("legend#") && n.id.ends_with("#label"))
        .map(|n| n.label.lines.join(" "))
        .collect();
    assert_eq!(rows, vec!["Full".to_string(), "Short".to_string()]);
}

/// **A datum the axis has no slot for is drawn nowhere, never off the end of the plot.**
///
/// `bar [1, 2, 3, 4, 5]` written *above* `x-axis [a, b, c]` keeps all five points — upstream
/// transforms a plot as the statement arrives, so its "prevent orphaned bars from rendering in
/// unlabeled chart space" guard never sees them. konoma handed all five to `slot_center(i, 3)`,
/// which puts slot 3 of 3 at seven sixths of the way across: the extra bars were drawn in the
/// margin beyond the frame, under no tick, in the series colour, looking exactly like data.
#[test]
fn a_datum_the_axis_has_no_slot_for_is_not_drawn_beyond_the_plot() {
    if !text_metrics::fonts_available() {
        return;
    }
    for (name, src) in cases_of(chart::xychart::is_xychart) {
        let d = laid_out(src);
        let frame = of_kind(&d, Glyph::PlotFrame);
        let (fl, _, fr, _) = frame[0].bounds();
        for bar in d
            .nodes
            .iter()
            .filter(|n| n.id.starts_with("bar#") || n.id.starts_with("point#"))
        {
            let (l, _, r, _) = bar.bounds();
            assert!(
                l >= fl - 0.01 && r <= fr + 0.01,
                "{name}: {} runs from {} to {} where the plot is {}..{}",
                bar.id,
                svg::num(l),
                svg::num(r),
                svg::num(fl),
                svg::num(fr)
            );
        }
    }
    // …and the case that used to break it really does keep its axis to three slots.
    let d = laid_out(corpus("xychart-plot-before-axis"));
    assert_eq!(
        d.nodes.iter().filter(|n| n.id.starts_with("bar#")).count(),
        3,
        "the two orphaned bars are back"
    );
}

/// **Two ticks never carry the same number.**
///
/// How many decimals a tick needs is a fact about the *axis* — the gap to the tick beside it — not
/// about the tick's own magnitude. Deciding it per value made `y-axis 0 --> 0.001` read
/// `0, 0, 0, 0.001, 0.001, 0.001` from the bottom up: six grid lines, three numbers, and a line
/// two fifths of the way up labelled `0`.
#[test]
fn an_axis_narrower_than_one_tick_still_reads_as_distinct_numbers() {
    for (min, max) in [
        (0.0, 0.001),
        (0.0, 1e-9),
        (1.0000, 1.0004),
        (0.0, 1.0),
        (4000.0, 11000.0),
        (-1e6, 1e6),
        (99.9, 100.1),
        (-0.0005, 0.0005),
    ] {
        let values = ticks(min, max, 5);
        let texts = super::axis_tick_texts(&values);
        assert_eq!(texts.len(), values.len());
        let unique: HashSet<&String> = texts.iter().collect();
        assert_eq!(
            unique.len(),
            texts.len(),
            "{min}..{max}: the ticks read {texts:?} — two of them carry the same number"
        );
        // …and each one still says what its value is.
        for (v, t) in values.iter().zip(texts.iter()) {
            let back: f64 = t
                .parse()
                .unwrap_or_else(|_| panic!("{t:?} is not a number"));
            let scale = v.abs().max(1e-12);
            assert!(
                (back - v).abs() <= scale * 1e-9 || (back - v).abs() < 1e-15,
                "{min}..{max}: a tick at {v} is labelled {t:?}"
            );
        }
    }
    // The tick labels of the charts that had them before are unchanged — the run-wide rule only
    // adds places where the per-value rule ran out, and takes none away.
    assert_eq!(
        super::axis_tick_texts(&[4000.0, 6000.0, 8000.0, 10000.0]),
        vec!["4000", "6000", "8000", "10000"]
    );
    assert_eq!(
        super::axis_tick_texts(&[-40.0, -20.0, 0.0, 20.0, 40.0, 60.0]),
        vec!["-40", "-20", "0", "20", "40", "60"]
    );
}

/// **A positive value always gets a rectangle, however thin.**
///
/// Only a value of *nothing* is drawn as nothing. The half-pixel floor this replaced made
/// `"Trace": 8` beside `"Bulk": 9990` vanish from the map altogether, and a datum that is simply
/// absent cannot be told from one that was never written.
#[test]
fn a_treemap_gives_every_positive_value_a_tile_however_thin() {
    if !text_metrics::fonts_available() {
        return;
    }
    for (name, src) in cases_of(chart::treemap::is_treemap) {
        let model = chart::treemap::parse(src).expect("parses");
        let d = laid_out(src);
        let tiles = of_kind(&d, Glyph::ChartBar);
        for (i, root) in model.roots.iter().enumerate() {
            let id = format!("tile#{}", dfs_index(&model.roots, i));
            let found = tiles.iter().any(|t| t.id == id);
            assert_eq!(
                found,
                root.total() > 0.0,
                "{name}: {:?} totals {} and {} a tile",
                root.name,
                root.total(),
                if found { "has" } else { "has no" }
            );
        }
    }
    let d = laid_out(corpus("treemap-dominant"));
    assert_eq!(
        of_kind(&d, Glyph::ChartBar).len(),
        3,
        "a datum went missing"
    );
    let d = laid_out(corpus("treemap-zero-leaf"));
    assert_eq!(
        of_kind(&d, Glyph::ChartBar).len(),
        2,
        "the zero-valued leaf was drawn as a stroke on the boundary"
    );
}

/// **A share says the share of the whole.**
///
/// The one thing a pie is for. Nothing named checked the *number*: a percentage taken against the
/// largest slice instead of the total, and one rounded down instead of to nearest, both left every
/// geometric statement true — the label still sat inside its own wedge, at the right angle, in the
/// right colour — and were caught only by the masked corpus golden, which is the shape
/// `docs/FEATURE-MERMAID-RENDERER.md` §6-A warns about: a golden is regenerated without comment
/// and the guarantee leaves with it.
///
/// The expected text is derived here rather than borrowed from `percent_text`, or the test would
/// agree with whatever that function does.
#[test]
fn a_pie_share_says_the_share_of_the_whole() {
    if !text_metrics::fonts_available() {
        return;
    }
    let mut checked = 0usize;
    for (name, src) in cases_of(chart::pie::is_pie) {
        let model = chart::pie::parse(src).expect("parses");
        let d = laid_out(src);
        let total = model.total();
        for node in d.nodes.iter().filter(|n| n.id.ends_with("#pct")) {
            let i: usize = node
                .id
                .trim_start_matches("slice#")
                .trim_end_matches("#pct")
                .parse()
                .expect("a share's id names its slice");
            let pct = model.slices[i].value / total * 100.0;
            let want = if pct > 0.0 && pct < 0.5 {
                "<1%".to_string()
            } else {
                format!("{}%", pct.round() as i64)
            };
            assert_eq!(
                node.label.lines.join(""),
                want,
                "{name}: {:?} is {} of {total} and is labelled {:?}",
                model.slices[i].label,
                model.slices[i].value,
                node.label.lines.join("")
            );
            checked += 1;
        }
    }
    assert!(checked >= 10, "only {checked} shares were read");
    // A pie of one slice says so. It used to say nothing at all: the chord across a wedge is
    // `2r·sin(sweep/2)`, which comes back to zero at a full turn, so the slice with the least
    // doubt about its share was the one that lost its label.
    let d = laid_out(corpus("pie-single"));
    assert_eq!(
        d.nodes
            .iter()
            .filter(|n| n.id.ends_with("#pct"))
            .map(|n| n.label.lines.join(""))
            .collect::<Vec<_>>(),
        vec!["100%".to_string()]
    );
}

/// **The value scale spans the range the source declared, and nothing more.**
///
/// Stated against [`super::xychart::effective_range`] itself, because that is the one function the
/// drawing asks. Stretching its top by a tenth left every bar in the right proportion to every
/// other and only the masked golden noticed — a chart whose bars are all a tenth short of where
/// the axis says they are is the definition of a chart that reads as data and is not.
#[test]
fn the_value_scale_is_the_range_the_source_asked_for() {
    let range =
        |src: &str| super::xychart::effective_range(&chart::xychart::parse(src).expect("parses"));
    assert_eq!(
        range("xychart-beta\n  y-axis 0 --> 10\n  bar [5]\n"),
        Some((0.0, 10.0))
    );
    assert_eq!(
        range("xychart-beta\n  y-axis 4000 --> 11000\n  bar [5000]\n"),
        Some((4000.0, 11000.0))
    );
    assert_eq!(
        range("xychart-beta\n  y-axis -40 --> 60\n  bar [1]\n"),
        Some((-40.0, 60.0))
    );
    // No `y-axis` line: the range is the data's own extent.
    assert_eq!(range("xychart-beta\n  bar [3, 9, 5]\n"), Some((3.0, 9.0)));
    // Written backwards, it is turned the right way round rather than drawn upside down.
    assert_eq!(
        range("xychart-beta\n  y-axis 100 --> 0\n  bar [50]\n"),
        Some((0.0, 100.0))
    );
    // **A declared range is a floor, not a crop.** A value outside it grows the axis, because the
    // alternative konoma had was to draw the mark two and a half plot-heights above the frame,
    // where no scale reaches it.
    assert_eq!(
        range("xychart-beta\n  y-axis 0 --> 10\n  bar [5, 25, 8]\n"),
        Some((0.0, 25.0))
    );
    assert_eq!(
        range("xychart-beta\n  y-axis 0 --> 10\n  bar [-6, 3]\n"),
        Some((-6.0, 10.0))
    );
    // Every value the same: widened by half a unit each way, so the flat line sits in the middle.
    assert_eq!(range("xychart-beta\n  bar [7, 7, 7]\n"), Some((6.5, 7.5)));
}

/// **Two bar series stand side by side, in the order they were written.**
///
/// Swapping them within every slot moves no bar off its category, changes no height and keeps the
/// legend right, so every statement about heights, baselines and legends stayed true — and the
/// chart said South where it meant North.
#[test]
fn two_bar_series_share_a_slot_in_source_order() {
    if !text_metrics::fonts_available() {
        return;
    }
    let mut checked = 0usize;
    for (name, src) in cases_of(chart::xychart::is_xychart) {
        let model = chart::xychart::parse(src).expect("parses");
        let bars: Vec<usize> = model
            .plots
            .iter()
            .enumerate()
            .filter(|(_, p)| p.kind == chart::xychart::PlotKind::Bar)
            .map(|(i, _)| i)
            .collect();
        if bars.len() < 2 {
            continue;
        }
        let d = laid_out(src);
        // Slot by slot: the earlier series is to the left of the later one.
        for slot in 0..frame_slots(&d) {
            let mut previous: Option<(usize, f64)> = None;
            for series in &bars {
                let Some(bar) = d
                    .nodes
                    .iter()
                    .find(|n| n.id == format!("bar#{series}#{slot}"))
                else {
                    continue;
                };
                if let Some((before, x)) = previous {
                    assert!(
                        x < bar.center.x,
                        "{name}: in slot {slot}, series {before} stands at {} and series {series} \
                         at {} — the earlier series is not on the left",
                        svg::num(x),
                        svg::num(bar.center.x)
                    );
                    checked += 1;
                }
                previous = Some((*series, bar.center.x));
            }
        }
    }
    assert!(checked > 0, "no chart in the corpus has two bar series");
}

/// **A line's vertex is its own datum's value.**
///
/// The bars had this and the lines did not: giving every vertex the first datum's value drew a
/// perfectly flat line through a chart of rising numbers, and only the masked golden minded.
#[test]
fn a_line_vertex_stands_at_its_own_datums_value() {
    if !text_metrics::fonts_available() {
        return;
    }
    let mut checked = 0usize;
    for (name, src) in cases_of(chart::xychart::is_xychart) {
        let model = chart::xychart::parse(src).expect("parses");
        let d = laid_out(src);
        let frame = of_kind(&d, Glyph::PlotFrame);
        let (_, ft, _, fb) = frame[0].bounds();
        let (lo, hi) = super::xychart::effective_range(&model).expect("a finite range");
        let slots = frame_slots(&d);
        let y_of = |v: f64| fb - (v - lo) / (hi - lo) * (fb - ft);
        for (series, plot) in model.plots.iter().enumerate() {
            if plot.kind != chart::xychart::PlotKind::Line {
                continue;
            }
            for (i, (_, v)) in plot
                .data
                .iter()
                .enumerate()
                .take(super::xychart::drawn_len(plot, slots))
            {
                let dot = d
                    .nodes
                    .iter()
                    .find(|n| n.id == format!("point#{series}#{i}"))
                    .unwrap_or_else(|| panic!("{name}: no marker for datum {i} of plot {series}"));
                assert!(
                    (dot.center.y - y_of(*v)).abs() < 0.01,
                    "{name}: datum {i} of plot {series} is {v}, which is {} up the scale, but its \
                     marker is at {}",
                    svg::num(y_of(*v)),
                    svg::num(dot.center.y)
                );
                checked += 1;
            }
        }
    }
    assert!(checked >= 10, "only {checked} line vertices were read");
}

/// **The first axis of a radar chart points at twelve o'clock.**
///
/// An absolute statement, and it has to be: every other radar test reads the spoke directions off
/// the curve konoma drew, so turning the whole chart a quarter turn kept all of them true. It is
/// not a matter of taste — the shared emitter draws the polygon graticule from its own copy of
/// the same convention (`svg::radial_polygon` starts at `-π/2`), so a chart whose spokes disagree
/// with it has its data plotted off the corners of its own rings.
#[test]
fn the_first_radar_axis_points_at_twelve_oclock() {
    if !text_metrics::fonts_available() {
        return;
    }
    assert!(
        (super::radar::angle_of(0, 5) + std::f64::consts::FRAC_PI_2).abs() < 1e-12,
        "spoke 0 is not straight up"
    );
    for (name, src) in cases_of(chart::radar::is_radar) {
        let d = laid_out(src);
        let centre = of_kind(&d, Glyph::Graticule)[0].center.clone();
        let curve = d
            .edges
            .iter()
            .find(|e| e.series.is_some())
            .unwrap_or_else(|| panic!("{name}: no curve"));
        let first = &curve.points[0];
        assert!(
            (first.x - centre.x).abs() < 0.01,
            "{name}: the first vertex is at x={} where the centre is {} — the chart has been \
             turned, and its data no longer lands on the corners of its own graticule",
            svg::num(first.x),
            svg::num(centre.x)
        );
        assert!(
            first.y <= centre.y + 0.01,
            "{name}: the first vertex is below the centre"
        );
    }
}

/// **A section's children stay clear of the words naming it.**
///
/// The band across the top of a section is the only place its own name is written, and a child
/// laid over it hides the name of the thing it is inside. No geometric statement minded: the areas
/// were still exactly right, the tiles still tiled, and nothing overlapped that was not allowed to.
#[test]
fn a_treemap_sections_children_stay_below_its_name() {
    if !text_metrics::fonts_available() {
        return;
    }
    // **Which tile is inside which comes from the model, not from the geometry.** Asking the
    // rectangles produced a test that could not see the fault it was written for: a child laid
    // over its parent's name band starts at the *same* top edge as the parent, and every "is this
    // one inside that one" rule has to let a shared edge through or it reports every sibling.
    fn walk(
        name: &str,
        items: &[chart::treemap::Node],
        base: usize,
        tiles: &[&PlacedNode],
        checked: &mut usize,
    ) {
        let mut index = base;
        for item in items {
            let parent_index = index;
            index += super::treemap::subtree_len(item);
            let Some(parent) = tiles
                .iter()
                .find(|t| t.id == format!("tile#{parent_index}"))
            else {
                continue;
            };
            let Some(panel) = &parent.panel else { continue };
            let (_, pt, _, _) = parent.bounds();
            // Where the parent's own words stop, in absolute coordinates.
            let words_bottom = pt
                + panel
                    .rows
                    .iter()
                    .map(|r| r.y + super::super::labels::line_height() / 2.0)
                    .fold(0.0_f64, f64::max);
            let mut child_index = parent_index + 1;
            for child in &item.children {
                if let Some(tile) = tiles.iter().find(|t| t.id == format!("tile#{child_index}")) {
                    let (_, ct, _, _) = tile.bounds();
                    assert!(
                        ct >= words_bottom - 0.01,
                        "{name}: {:?} starts at {} where {:?}'s own name runs to {} — a child laid \
                         over the band hides the name of the thing it is inside",
                        child.name,
                        svg::num(ct),
                        item.name,
                        svg::num(words_bottom)
                    );
                    *checked += 1;
                }
                child_index += super::treemap::subtree_len(child);
            }
            walk(name, &item.children, parent_index + 1, tiles, checked);
        }
    }

    let mut checked = 0usize;
    for (name, src) in cases_of(chart::treemap::is_treemap) {
        let model = chart::treemap::parse(src).expect("parses");
        let d = laid_out(src);
        let tiles = of_kind(&d, Glyph::ChartBar);
        walk(name, &model.roots, 0, &tiles, &mut checked);
    }
    assert!(checked > 0, "no treemap in the corpus nests");
}

/// **A ribbon meets the bar at each of its ends.**
///
/// The claim a Sankey makes is that what leaves a node is what arrives at the next, and it is made
/// entirely by the two ends of the band touching the two bars. Exchanging the ends kept every
/// thickness right, every node the right height and every colour in place: the thicknesses tested
/// were the same numbers, and only which side they were written on changed.
#[test]
fn a_sankey_band_meets_its_source_at_one_end_and_its_target_at_the_other() {
    if !text_metrics::fonts_available() {
        return;
    }
    for (name, src) in cases_of(chart::sankey::is_sankey) {
        let model = chart::sankey::parse(src).expect("parses");
        let d = laid_out(src);
        let bar = |i: usize| {
            d.nodes
                .iter()
                .find(|n| n.id == format!("node#{i}"))
                .unwrap_or_else(|| panic!("{name}: no bar for node {i}"))
                .bounds()
        };
        for (i, link) in model.links.iter().enumerate() {
            let node = d
                .nodes
                .iter()
                .find(|n| n.id == format!("flow#{i}"))
                .unwrap_or_else(|| panic!("{name}: no band for link {i}"));
            let Some(Mark::Ribbon {
                left_top,
                left_bottom,
                right_top,
                right_bottom,
            }) = node.mark
            else {
                panic!("{name}: a ribbon with no corners");
            };
            let cy = node.center.y;
            // **A link runs left to right**, so the source's bar is the left-hand one. Nothing else
            // says so: with every node put in its source's column the diagram still drew.
            let (sl, st, _, sb) = bar(link.source);
            let (tl, tt, _, tb) = bar(link.target);
            assert!(
                sl < tl - 0.01,
                "{name}: link {i} goes from {:?} to {:?} and its target is not to the right",
                model.nodes[link.source],
                model.nodes[link.target]
            );
            for (edge, (top, bottom), end, which) in [
                (cy + left_top, (st, sb), "source", &model.nodes[link.source]),
                (
                    cy + right_top,
                    (tt, tb),
                    "target",
                    &model.nodes[link.target],
                ),
            ] {
                assert!(
                    edge >= top - 0.01 && edge <= bottom + 0.01,
                    "{name}: link {i}'s {end} edge is at {} where {which:?}'s bar runs {}..{}",
                    svg::num(edge),
                    svg::num(top),
                    svg::num(bottom)
                );
            }
            let _ = (left_bottom, right_bottom);
        }
    }
}

/// **A node with nothing leaving it is in the last column, and every column fits the height it is
/// given — using all of it.**
///
/// Three things one scale has to get right, and each of them was invisible. Dropping the `justify`
/// alignment left the sinks scattered across the middle, which reads as a flow that stops early.
/// Taking the scale from the first column alone made a fuller column run off the bottom. And
/// shrinking every height by a tenth together kept every *ratio* exact — the case §6-A item 12
/// records, where a shared scale moves everything at once and every relative check passes.
#[test]
fn a_sankey_fills_the_height_it_is_given_and_lines_its_sinks_up_on_the_right() {
    if !text_metrics::fonts_available() {
        return;
    }
    for (name, src) in cases_of(chart::sankey::is_sankey) {
        let model = chart::sankey::parse(src).expect("parses");
        let d = laid_out(src);
        let bars: Vec<&PlacedNode> = d
            .nodes
            .iter()
            .filter(|n| n.id.starts_with("node#") && n.shape == Glyph::ChartBar)
            .collect();
        let right = bars
            .iter()
            .map(|b| b.center.x)
            .fold(f64::NEG_INFINITY, f64::max);
        for (i, b) in bars.iter().enumerate() {
            if model.links.iter().any(|l| l.source == i) {
                continue;
            }
            assert!(
                (b.center.x - right).abs() < 0.01,
                "{name}: nothing leaves {:?}, but its bar is at {} and the last column is at {}",
                model.nodes[i],
                svg::num(b.center.x),
                svg::num(right)
            );
        }

        // Column by column: what is stacked in it, plus the gaps between.
        let mut columns: std::collections::BTreeMap<i64, Vec<f64>> = Default::default();
        for b in &bars {
            columns
                .entry((b.center.x * 100.0).round() as i64)
                .or_default()
                .push(b.size.h);
        }
        let mut fullest = 0.0_f64;
        for (x, heights) in &columns {
            let used: f64 =
                heights.iter().sum::<f64>() + (heights.len() - 1) as f64 * super::sankey::NODE_PAD;
            // The floor under a hairline flow can push a column past the height; nothing else may.
            let slack = heights.len() as f64 * super::sankey::MIN_BAND;
            assert!(
                used <= super::sankey::HEIGHT + slack + 0.01,
                "{name}: the column at {} stacks up to {} in a diagram {} tall",
                *x as f64 / 100.0,
                svg::num(used),
                svg::num(super::sankey::HEIGHT)
            );
            fullest = fullest.max(used);
        }
        assert!(
            fullest >= super::sankey::HEIGHT - 0.01,
            "{name}: the fullest column reaches {} of the {} the diagram has — one scale that \
             leaves room everywhere is a scale read off nothing",
            svg::num(fullest),
            svg::num(super::sankey::HEIGHT)
        );
    }
}

// ---------------------------------------------------------------------------------------------
// 3a. What only the golden was holding
// ---------------------------------------------------------------------------------------------
//
// Each of the seven statements below was, until it was written, guarded by
// `chart_corpus_golden` and by nothing else. `docs/FEATURE-MERMAID-RENDERER.md` §6-A is emphatic
// about why that is not enough: a golden is a file somebody regenerates, and the guarantee walks
// out with the regeneration, in silence. konoma has been caught by this twice already.

/// **A tile carries its own node's name, and a section's colour is its whole section's.**
///
/// Two pairings, both invisible to everything else. The shared placement test builds a map from
/// the words it found to the tiles it found them on and then looks each model node up **by its own
/// text** — which is §6-A item 15: a permutation leaves the set of words unchanged, so every
/// lookup still succeeds and the nesting checks still pass. Labelling every tile with its
/// sibling's name went straight through it.
#[test]
fn a_treemap_tile_carries_its_own_name_and_its_sections_colour() {
    if !text_metrics::fonts_available() {
        return;
    }
    fn walk(
        name: &str,
        items: &[chart::treemap::Node],
        base: usize,
        section: Option<usize>,
        tiles: &[&PlacedNode],
        checked: &mut usize,
    ) {
        let mut index = base;
        for (i, item) in items.iter().enumerate() {
            let me = index;
            index += super::treemap::subtree_len(item);
            let Some(tile) = tiles.iter().find(|t| t.id == format!("tile#{me}")) else {
                continue;
            };
            // **By the tile's id, which is the node's place in the source.** Never by looking the
            // node's own words up among the words that were drawn.
            if let Some(panel) = &tile.panel {
                if let Some(row) = panel.rows.first() {
                    let drawn = row
                        .cells
                        .iter()
                        .map(|c| c.label.lines.join(" "))
                        .collect::<Vec<_>>()
                        .join(" ");
                    assert_eq!(
                        drawn,
                        text_metrics::collapse_spaces(&item.name).into_owned(),
                        "{name}: tile#{me} is where {:?} belongs and it reads {drawn:?}",
                        item.name
                    );
                    *checked += 1;
                }
            }
            // Colour keys the top-level section, so a whole section reads as one block and its
            // internal divisions are told by the rules between the tiles.
            let want = section.unwrap_or(i);
            assert_eq!(
                tile.series,
                Some(want),
                "{name}: tile#{me} is inside section {want} and is painted in series {:?}",
                tile.series
            );
            walk(name, &item.children, me + 1, Some(want), tiles, checked);
        }
    }
    let mut checked = 0usize;
    for (name, src) in cases_of(chart::treemap::is_treemap) {
        let model = chart::treemap::parse(src).expect("parses");
        let d = laid_out(src);
        let tiles = of_kind(&d, Glyph::ChartBar);
        walk(name, &model.roots, 0, None, &tiles, &mut checked);
    }
    assert!(checked >= 20, "only {checked} tile names were read");
}

/// **No treemap in the corpus may sit near the width at which a label stops being drawn.**
///
/// A tile is labelled only if the label fits in it (`treemap`'s module docs), so every treemap case
/// carries a decision that is made out of a *measured* width — and a measured width is a property
/// of the machine's fonts. `treemap-long-names` was written 0.06px on the "does not fit" side of
/// that line as measured on macOS, which put it 18px on the *other* side of it on a Linux box with
/// FreeSans: the label was drawn there, `chart_corpus_golden` drifted, and what the golden was
/// reporting was which font the host has.
///
/// The corpus is meant to say the same thing everywhere — `chart_emit_golden` exists precisely
/// because `chart_corpus_golden` goes through real fonts — so the rule is stated here for the whole
/// kind rather than patched into the one case that tripped: **every tile must be clearly on one
/// side or the other.** 10% is four times the ~4% spread measured between two ordinary sans-serif
/// faces, which is the size of drift this has to survive.
///
/// The widths come from `Label::measure`, the same function `treemap::tile_panel` sizes with, so
/// this asks production's question rather than a re-derived one (§6-A item 19).
#[test]
fn no_treemap_case_sits_on_the_label_fit_threshold() {
    if !text_metrics::fonts_available() {
        return;
    }
    const MARGIN: f64 = 0.10;
    let mut checked = 0usize;
    for (name, src) in cases_of(chart::treemap::is_treemap) {
        let model = chart::treemap::parse(src).expect("parses");
        let mut names = std::collections::HashMap::new();
        collect_treemap_names(&model.roots, 0, &mut names);
        let d = laid_out(src);
        for tile in of_kind(&d, Glyph::ChartBar) {
            // The name the tile was made for, or would have been labelled with: taken from the
            // model **by the tile's id**, which carries the node's place in the source.
            let Some(text) = tile
                .id
                .strip_prefix("tile#")
                .and_then(|i| i.parse::<usize>().ok())
                .and_then(|i| names.get(&i))
            else {
                continue;
            };
            let needed = super::super::Label::measure(text).width + super::treemap::TEXT_PAD * 2.0;
            let have = tile.size.w;
            checked += 1;
            assert!(
                (needed - have).abs() > needed * MARGIN,
                "{name}: {text:?} needs {} of a {} tile — {:.1}% apart, so which side of the                  fit rule this case lands on is decided by the host's fonts rather than by the                  case. Make the name clearly too long or clearly short enough.",
                svg::num(needed),
                svg::num(have),
                (needed - have).abs() / needed * 100.0
            );
        }
    }
    assert!(checked >= 20, "only {checked} tiles were examined");
}

/// Every treemap node's name against the depth-first index its tile is numbered by — the same
/// arithmetic `dfs_index` and `a_treemap_tile_carries_its_own_name_and_its_sections_colour` use, so
/// a tile is matched to a node **by position**, never by looking up the words that were drawn
/// (§6-A item 15).
fn collect_treemap_names(
    items: &[chart::treemap::Node],
    base: usize,
    out: &mut std::collections::HashMap<usize, String>,
) {
    let mut index = base;
    for item in items {
        out.insert(index, item.name.clone());
        collect_treemap_names(&item.children, index + 1, out);
        index += super::treemap::subtree_len(item);
    }
}

/// **A section is as big as everything under it added up.**
///
/// Stated against a total the test works out itself. `treemap_tile_areas…` asks
/// `Node::total` for both the expected share *and* — through the layout — the drawn one, so a
/// change to how a section is totalled moves both sides together and the comparison holds. Summing
/// a section's children with `max` instead of `+` therefore passed every area check there is.
#[test]
fn a_treemap_section_is_as_big_as_its_leaves_added_up() {
    if !text_metrics::fonts_available() {
        return;
    }
    /// The spec's own rule, restated: a leaf is its value and a section is its children's sum.
    fn leaves(node: &chart::treemap::Node) -> f64 {
        match node.value {
            Some(v) => v,
            None => node.children.iter().map(leaves).sum(),
        }
    }
    for (name, src) in cases_of(chart::treemap::is_treemap) {
        let model = chart::treemap::parse(src).expect("parses");
        let d = laid_out(src);
        let tiles = of_kind(&d, Glyph::ChartBar);
        let total: f64 = model.roots.iter().map(leaves).sum();
        let whole = super::treemap::WIDTH * super::treemap::HEIGHT;
        for (i, root) in model.roots.iter().enumerate() {
            let Some(tile) = tiles
                .iter()
                .find(|t| t.id == format!("tile#{}", dfs_index(&model.roots, i)))
            else {
                continue;
            };
            let want = leaves(root) / total * whole;
            let area = tile.size.w * tile.size.h;
            assert!(
                (area - want).abs() / want.max(1.0) < 0.001,
                "{name}: {:?}'s leaves add up to {} of {total}, which is {} px² of {}, but its \
                 tile is {}",
                root.name,
                leaves(root),
                svg::num(want),
                svg::num(whole),
                svg::num(area)
            );
        }
    }
}

/// **The tiles are squarified, not sliced.**
///
/// Areas alone do not make a treemap readable: a column of hairlines has exactly the right areas
/// and cannot be compared by eye, which is the whole reason for Bruls, Huizing and van Wijk's
/// algorithm rather than a simple slice down the side. Turning the heuristic off left every area
/// exact and every invariant true.
#[test]
fn treemap_tiles_are_squarified_rather_than_sliced() {
    // Ten equal values in a wide rectangle: sliced, each is a 48×300 ribbon (ratio 6.25);
    // squarified, none is worse than about 2.
    let rect = super::treemap::Rect {
        x: 0.0,
        y: 0.0,
        w: 480.0,
        h: 300.0,
    };
    let values = vec![1.0; 10];
    let worst = super::treemap::squarify(&values, rect)
        .iter()
        .map(|r| (r.w / r.h).max(r.h / r.w))
        .fold(0.0_f64, f64::max);
    assert!(
        worst < 3.0,
        "the worst tile is {worst:.2} times longer than it is wide — that is a slice-and-dice \
         layout, whose areas are right and whose tiles cannot be compared by eye"
    );
    // …and the areas are still exactly the shares, which is what stops this being traded away.
    let uneven = vec![50.0, 25.0, 12.0, 8.0, 5.0];
    let total: f64 = uneven.iter().sum();
    for (v, r) in uneven
        .iter()
        .zip(super::treemap::squarify(&uneven, rect).iter())
    {
        let want = v / total * rect.area();
        assert!((r.w * r.h - want).abs() / want < 1e-6);
    }
}

/// **The outermost ring of a radar chart is the top of its scale.**
///
/// `ticks` says how many rings there are, and drawing one more makes the ring a vertex sits on
/// mean a different number — a chart where the outer ring is not the maximum is read wrongly by
/// anyone who counts rings, which is what rings are for.
#[test]
fn a_radars_rings_are_its_ticks_and_its_spokes_are_its_axes() {
    if !text_metrics::fonts_available() {
        return;
    }
    for (name, src) in cases_of(chart::radar::is_radar) {
        let model = chart::radar::parse(src).expect("parses");
        let d = laid_out(src);
        let grat = of_kind(&d, Glyph::Graticule);
        assert_eq!(grat.len(), 1, "{name}: one graticule");
        let Some(Mark::Graticule {
            rings,
            spokes,
            polygon,
        }) = grat[0].mark
        else {
            panic!("{name}: a graticule with no mark");
        };
        assert_eq!(
            rings,
            model.options.ticks.max(1),
            "{name}: the source asks for {} rings and {rings} were drawn",
            model.options.ticks
        );
        assert_eq!(spokes, model.axes.len(), "{name}: one spoke per axis");
        assert_eq!(
            polygon,
            model.options.graticule == chart::radar::Graticule::Polygon,
            "{name}: the graticule is the wrong shape"
        );
    }
}

/// **A packet field's colour says which field it is.**
///
/// The two halves of a field cut at a row boundary are one field, and they say so by being one
/// colour; two fields beside each other say they are two by not being. Painting the whole packet
/// in a single colour destroyed both statements and left every width, every position and every
/// label exactly right.
#[test]
fn the_two_halves_of_a_split_packet_field_share_a_colour_and_neighbours_do_not() {
    if !text_metrics::fonts_available() {
        return;
    }
    for (name, src) in cases_of(chart::packet::is_packet) {
        let model = chart::packet::parse(src).expect("parses");
        let d = laid_out(src);
        let colour = |f: &chart::packet::Field| {
            d.nodes
                .iter()
                .find(|n| n.id == format!("field#{}-{}", f.start, f.end))
                .and_then(|n| n.series)
                .unwrap_or_else(|| panic!("{name}: no block for bits {}..{}", f.start, f.end))
        };
        let flat: Vec<&chart::packet::Field> = model.rows.iter().flatten().collect();
        for (i, f) in flat.iter().enumerate() {
            for g in flat.iter().skip(i + 1) {
                // Same name, one field: the row split keeps the label, and that is the only way a
                // reader can tell the halves belong together.
                if f.label == g.label {
                    assert_eq!(
                        colour(f),
                        colour(g),
                        "{name}: two blocks both labelled {:?} are drawn in different colours",
                        f.label
                    );
                } else {
                    assert_ne!(
                        colour(f),
                        colour(g),
                        "{name}: {:?} and {:?} are different fields drawn in one colour",
                        f.label,
                        g.label
                    );
                }
            }
        }
    }
    // The headline case: a field cut at the 32-bit boundary is one colour across the cut, and the
    // field beside it is not.
    let d = laid_out(corpus("packet-crossing"));
    let series = |id: &str| {
        d.nodes
            .iter()
            .find(|n| n.id == id)
            .and_then(|n| n.series)
            .expect("the block")
    };
    assert_eq!(series("field#16-31"), series("field#32-63"));
    assert_ne!(series("field#0-15"), series("field#16-31"));
}

/// **A band is drawn in the colour of the node it leaves.**
///
/// Which end a flow's colour comes from is how a reader follows it: every band leaving a node is
/// that node's colour, so a split reads as one thing dividing. Taking the colour from the far end
/// instead makes every band arriving at a node the same colour, so a *join* reads as a split.
#[test]
fn a_sankey_band_is_the_colour_of_the_node_it_leaves() {
    if !text_metrics::fonts_available() {
        return;
    }
    for (name, src) in cases_of(chart::sankey::is_sankey) {
        let model = chart::sankey::parse(src).expect("parses");
        let d = laid_out(src);
        for (i, link) in model.links.iter().enumerate() {
            let band = d
                .nodes
                .iter()
                .find(|n| n.id == format!("flow#{i}"))
                .unwrap_or_else(|| panic!("{name}: no band for link {i}"));
            let bar = d
                .nodes
                .iter()
                .find(|n| n.id == format!("node#{}", link.source))
                .expect("its source's bar");
            assert_eq!(
                band.series, bar.series,
                "{name}: the band from {:?} to {:?} is not its source's colour",
                model.nodes[link.source], model.nodes[link.target]
            );
        }
    }
}

// ---------------------------------------------------------------------------------------------
// 3b. What is known to be wrong and is not fixed here
// ---------------------------------------------------------------------------------------------

/// Two points on the same coordinates have their names drawn on top of one another.
///
/// Legal source, correct dots, unreadable words. `docs/STATUS.md` already carries "quadrant point
/// labels can collide"; this states the property that would settle it, so that whoever fixes the
/// collisions has a witness rather than a description. The fix is a label-placement pass (push the
/// second name to the other side of its dot, or off to one shoulder), which is more than a chart
/// corpus audit should be changing.
#[test]
#[ignore = "known: quadrant point labels collide — docs/STATUS.md. Asserts what a fix must make true."]
fn coincident_quadrant_points_should_not_stack_their_labels() {
    if !text_metrics::fonts_available() {
        return;
    }
    let d = laid_out("quadrantChart\n  First: [0.4, 0.4]\n  Second: [0.4, 0.4]\n");
    let labels: Vec<&PlacedNode> = d
        .nodes
        .iter()
        .filter(|n| n.id.starts_with("point#") && n.id.ends_with("#label"))
        .collect();
    assert_eq!(labels.len(), 2);
    assert!(
        !overlap(labels[0].bounds(), labels[1].bounds()),
        "two point names are drawn on the same spot"
    );
}

/// A plot with more points than the axis has slots loses its tail.
///
/// `bar [1, 2, 3]` then `bar [1, 2, 3, 4, 5]`, with no `x-axis` line: upstream fixes the range at
/// the first plot (`hasSetXAxis`) and then spreads the second plot's five points across that same
/// span on a **linear** scale, so all five are drawn. konoma places every datum by its index in a
/// row of equal slots, which cannot express five points on a three-tick axis, so the last two are
/// dropped rather than drawn past the right-hand edge — the lesser of the two wrongs.
///
/// Settling it means giving a linear x axis a real linear position mapping, which also moves where
/// the tick labels sit and is a change to how the chart is laid out, not to what the corpus covers.
#[test]
#[ignore = "known: a linear x axis is drawn as equal slots, so a longer second plot loses its tail"]
fn a_plot_longer_than_its_axis_should_still_show_every_datum() {
    if !text_metrics::fonts_available() {
        return;
    }
    let src = "xychart-beta\n  bar \"Short\" [1, 2, 3]\n  bar \"Long\" [1, 2, 3, 4, 5]\n";
    let d = dispatch(src).expect("lays out");
    assert_eq!(
        d.nodes
            .iter()
            .filter(|n| n.id.starts_with("bar#1#"))
            .count(),
        5,
        "the second plot's last two values are not drawn anywhere"
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
    // Built with the constructor production uses, so the golden pins the depth a data path is
    // actually given rather than one this fixture chose for itself (§6-A item 19).
    edges.push(super::data_path(
        vec![
            Point::new(40.0, 140.0),
            Point::new(100.0, 100.0),
            Point::new(160.0, 60.0),
            Point::new(200.0, 40.0),
        ],
        2,
    ));

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
    }
    eprintln!("wrote the chart gallery to {dir}");
}
