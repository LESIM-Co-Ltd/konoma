//! **The right words in the right places** — the third hole in one family, closed as a family.
//!
//! # What was missing
//!
//! Stage 4's and stage 5a's suites answer two questions about a drawing: *is a mark there*
//! ([`super::decoration_tests`]) and *is it the right size, in the right spot*
//! (the geometric invariants in [`super::tests`] and the per-chart ones in
//! [`super::chart::tests`]). Neither of them ever asks the third question:
//!
//! > does the text drawn at that spot say what **that** datum says?
//!
//! A mutation making every x-axis category label draw the *first* category's text was caught by
//! exactly one test — the masked corpus golden — and by no named test at all. For a chart that is
//! the worst failure available: the picture still reads as data, so nobody looks twice, and the
//! data is wrong.
//!
//! It is the same family as two earlier holes, which is why this module states it once for every
//! kind konoma draws rather than patching the one case:
//!
//! | stage | what survived | what was checked instead |
//! |---|---|---|
//! | 4 | an arrow tip drawn on the wrong end of a flowchart edge | that *an* arrow was emitted |
//! | 5a | a section rule drawn at the wrong y | that the wrapper `<g>` was emitted |
//! | 5b | every category label drawing the first category's words | that a label of the right *width* was drawn |
//!
//! # The shape of every test here
//!
//! Each one is a statement of the form **"the text belonging to datum *k* is drawn at the place
//! belonging to datum *k*"**, and it is stated so that *both halves have to be right*:
//!
//! * the **text** comes from the parsed model, never from another part of the renderer, so
//!   swapping two labels is caught;
//! * the **place** is stated as a relation the geometry has to satisfy — a reading order, a
//!   containment, an angular sector, a nearest-neighbour — never by recomputing the coordinate the
//!   renderer computed, which would be konoma checking konoma.
//!
//! Reading order is the workhorse and it is worth saying why it is strong: sorting the drawn
//! labels by position and comparing the resulting *sequence of words* to the source order fails if
//! any label carries the wrong words, if any two are swapped, if one is dropped, or if the axis
//! is drawn backwards. One assertion, four mutations.
//!
//! # Cases come from the corpus, never from a list of names
//!
//! §6-A item 10: a radar mutation survived twice, the second time because the test that should
//! have caught it listed its cases by name and a new case never joined. Every test here takes its
//! cases from its kind's corpus through its kind's own routing predicate, so a case added
//! tomorrow is checked by all of them without anybody remembering to add it.
//!
//! # This module never parses konoma's SVG
//!
//! The rule from stage 1 (§6): geometry is checked on the [`Diagram`], not on the markup. What a
//! `<text>` element *contains* is `svg::emit` writing out `Label::lines`, and
//! [`super::tests::corpus_golden`] pins that byte for byte; what this module checks is that the
//! right `Label` reached the right place, which is a statement about the diagram.

use std::collections::{HashMap, HashSet};

use super::labels::Label;
use super::shapes::{Glyph, Mark};
use super::{class, er, sequence, state, Diagram, PlacedNode, RenderError};
use crate::preview::mermaid::chart;
use crate::preview::mermaid::layout::Point;
use crate::preview::mermaid::text_metrics;

// ---------------------------------------------------------------------------------------------
// Corpora and dispatch
// ---------------------------------------------------------------------------------------------

use super::chart as draw_chart;
use super::class_tests::CASES as CLASS_CASES;
use super::er_tests::CASES as ER_CASES;
use super::sequence_tests::CASES as SEQUENCE_CASES;
use super::state_tests::CASES as STATE_CASES;
use super::tests::CORPUS as FLOW_CASES;
use crate::preview::mermaid::chart::tests::CASES as CHART_CASES;

/// Every chart-corpus case of one kind, **found by that kind's own routing predicate**.
///
/// The `cases_of` pattern stage 5a introduced, reused verbatim: a hard-coded list of case names
/// stops covering the corpus the moment somebody adds a case, and that is not hypothetical — it
/// is how a radar mutation survived a second time (§6-A item 10).
fn cases_of(is_ours: fn(&str) -> bool) -> Vec<(&'static str, &'static str)> {
    let found: Vec<(&str, &str)> = CHART_CASES
        .iter()
        .filter(|(_, src)| is_ours(src))
        .copied()
        .collect();
    assert!(!found.is_empty(), "the corpus has no case of this kind");
    found
}

/// Lays a chart source out, whichever of the seven it is.
fn chart_diagram(src: &str) -> Diagram {
    fn go(src: &str) -> Result<Diagram, RenderError> {
        if chart::pie::is_pie(src) {
            return draw_chart::pie::lay_out(&chart::pie::parse(src)?);
        }
        if chart::xychart::is_xychart(src) {
            return draw_chart::xychart::lay_out(&chart::xychart::parse(src)?);
        }
        if chart::quadrant::is_quadrant_chart(src) {
            return draw_chart::quadrant::lay_out(&chart::quadrant::parse(src)?);
        }
        if chart::radar::is_radar(src) {
            return draw_chart::radar::lay_out(&chart::radar::parse(src)?);
        }
        if chart::treemap::is_treemap(src) {
            return draw_chart::treemap::lay_out(&chart::treemap::parse(src)?);
        }
        if chart::packet::is_packet(src) {
            return draw_chart::packet::lay_out(&chart::packet::parse(src)?);
        }
        if chart::sankey::is_sankey(src) {
            return draw_chart::sankey::lay_out(&chart::sankey::parse(src)?);
        }
        panic!("no chart parser claims this source")
    }
    go(src).unwrap_or_else(|e| panic!("corpus source must lay out: {e}\n{src}"))
}

// ---------------------------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------------------------

/// The words a label will draw, as one string. Exactly what a golden would show.
fn words(label: &Label) -> String {
    label.lines.join(" ")
}

/// The words `text` will be drawn as once measured — the model side of every comparison here.
fn as_drawn(text: &str) -> String {
    words(&Label::measure(text))
}

/// The chart labels of a diagram whose id begins with `prefix`, as `(centre, words)`.
fn labels_with_prefix(d: &Diagram, prefix: &str) -> Vec<(Point, String)> {
    d.nodes
        .iter()
        .filter(|n| n.shape == Glyph::ChartLabel && n.id.starts_with(prefix))
        .map(|n| (n.center.clone(), words(&n.label)))
        .collect()
}

/// The same, read left to right.
fn read_across(d: &Diagram, prefix: &str) -> Vec<String> {
    let mut v = labels_with_prefix(d, prefix);
    v.sort_by(|a, b| {
        a.0.x
            .partial_cmp(&b.0.x)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    v.into_iter().map(|(_, t)| t).collect()
}

/// The same, read top to bottom.
fn read_down(d: &Diagram, prefix: &str) -> Vec<String> {
    let mut v = labels_with_prefix(d, prefix);
    v.sort_by(|a, b| {
        a.0.y
            .partial_cmp(&b.0.y)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    v.into_iter().map(|(_, t)| t).collect()
}

/// The legend's rows, read top to bottom. Every chart lays its legend out through
/// `chart::legend_nodes`, so one reader serves all of them.
fn legend_down(d: &Diagram) -> Vec<String> {
    read_down(d, "legend#")
}

/// Every piece of text a panel draws, row by row, each row's cells joined left to right.
fn panel_rows(node: &PlacedNode) -> Vec<String> {
    let Some(p) = &node.panel else {
        return Vec::new();
    };
    p.rows
        .iter()
        .map(|r| {
            let mut cells: Vec<(f64, String)> =
                r.cells.iter().map(|c| (c.x, words(&c.label))).collect();
            cells.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
            cells
                .into_iter()
                .map(|(_, t)| t)
                .filter(|t| !t.trim().is_empty())
                .collect::<Vec<_>>()
                .join(" ")
        })
        .filter(|t| !t.trim().is_empty())
        .collect()
}

/// Which of `anchors` is nearest to `p`, by centre distance.
///
/// The workhorse for "this text belongs to *that* mark": a label that has drifted onto its
/// neighbour, or been swapped with it, stops being nearest to its own.
fn nearest(p: &Point, anchors: &[(usize, Point)]) -> usize {
    anchors
        .iter()
        .min_by(|a, b| {
            let da = (a.1.x - p.x).hypot(a.1.y - p.y);
            let db = (b.1.x - p.x).hypot(b.1.y - p.y);
            da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|(i, _)| *i)
        .expect("at least one anchor")
}

/// Whether `inner` sits inside `outer`, allowing half a pixel of rounding.
fn contains(outer: (f64, f64, f64, f64), inner: (f64, f64, f64, f64)) -> bool {
    inner.0 >= outer.0 - 0.5
        && inner.1 >= outer.1 - 0.5
        && inner.2 <= outer.2 + 0.5
        && inner.3 <= outer.3 + 0.5
}

// ---------------------------------------------------------------------------------------------
// The seven charts
// ---------------------------------------------------------------------------------------------

/// **pie** — a slice's share is drawn *on that slice*, and the legend names the slices in order.
///
/// The share is checked by the angle it sits at: the vector from the centre of the pie to the
/// percentage's own centre has to fall inside the wedge's own sector. That is the one statement a
/// percentage moved onto the neighbouring slice cannot satisfy, and it needs nothing from the
/// renderer but the geometry it produced.
#[test]
fn a_pies_share_is_drawn_on_its_own_slice_and_its_legend_names_the_slices_in_order() {
    if !text_metrics::fonts_available() {
        return;
    }
    for (name, src) in cases_of(chart::pie::is_pie) {
        let model = chart::pie::parse(src).expect("parses");
        let d = chart_diagram(src);

        // (a) the legend, top to bottom, is the slices in source order.
        let expected: Vec<String> = model
            .slices
            .iter()
            .map(|s| {
                if model.show_data {
                    // The legend adds the raw value; the *name* is what this test is about, so it
                    // is compared as a prefix and the value is left to the golden.
                    as_drawn(&s.label)
                } else {
                    as_drawn(&s.label)
                }
            })
            .collect();
        let drawn = legend_down(&d);
        assert_eq!(
            drawn.len(),
            expected.len(),
            "{name}: the legend has {} rows for {} slices",
            drawn.len(),
            expected.len()
        );
        for (i, (got, want)) in drawn.iter().zip(expected.iter()).enumerate() {
            assert!(
                got.starts_with(want.as_str()),
                "{name}: legend row {i} reads {got:?} where slice {i} is {want:?} — the rows are \
                 {drawn:?}"
            );
        }

        // (b) every percentage that was drawn sits inside the sector of its own slice.
        let mut checked = 0usize;
        for (i, node) in d.nodes.iter().enumerate() {
            let _ = i;
            if node.shape != Glyph::ChartLabel || !node.id.ends_with("#pct") {
                continue;
            }
            let owner = node
                .id
                .trim_start_matches("slice#")
                .trim_end_matches("#pct")
                .to_string();
            let wedge = d
                .nodes
                .iter()
                .find(|n| n.shape == Glyph::Wedge && n.id == format!("slice#{owner}"))
                .unwrap_or_else(|| panic!("{name}: a share labelled for a slice with no wedge"));
            let Some(Mark::Wedge { start, sweep }) = wedge.mark else {
                panic!("{name}: a wedge with no angles");
            };
            // Screen coordinates: y grows downward, and a wedge starts at twelve o'clock.
            let dx = node.center.x - wedge.center.x;
            let dy = node.center.y - wedge.center.y;
            let mut deg = dy.atan2(dx).to_degrees() + 90.0;
            while deg < start {
                deg += 360.0;
            }
            assert!(
                deg <= start + sweep + 1e-6,
                "{name}: the share {:?} is drawn at {deg:.2}°, outside its own slice's \
                 {start:.2}°..{:.2}°",
                words(&node.label),
                start + sweep
            );
            checked += 1;
        }
        assert!(
            checked > 0 || model.slices.len() > 8,
            "{name}: no share label was checked — a case where every share is dropped proves \
             nothing about placement"
        );
    }
}

/// **xychart** — this is the mutation's own test.
///
/// Three statements, each about a different axis of the same failure:
///
/// 1. the category labels, read left to right, spell the categories in source order;
/// 2. the y tick labels, read bottom to top, are increasing numbers — so the value axis is not
///    drawn upside down and no two ticks carry each other's number;
/// 3. every bar is nearer to its own category's label than to any other, which is what ties the
///    *data* to the words under it.
#[test]
fn an_xy_charts_categories_ticks_and_bars_each_carry_their_own_datums_text() {
    if !text_metrics::fonts_available() {
        return;
    }
    for (name, src) in cases_of(chart::xychart::is_xychart) {
        let model = chart::xychart::parse(src).expect("parses");
        let d = chart_diagram(src);

        // The categories, as the model has them: the `x-axis [..]` list, or the keys of the first
        // plot when the source gave none. Derived from the *model*, not from the renderer.
        let categories: Vec<String> = match &model.x {
            chart::xychart::XAxis::Band(v) if !v.is_empty() => {
                v.iter().map(|c| as_drawn(c)).collect()
            }
            _ => model
                .plots
                .first()
                .map(|p| p.data.iter().map(|(k, _)| as_drawn(k)).collect())
                .unwrap_or_default(),
        };

        // (1) left to right, they spell the source.
        let drawn = read_across(&d, "xtick#");
        let expected: Vec<String> = categories
            .iter()
            .filter(|c| !c.trim().is_empty())
            .cloned()
            .collect();
        assert_eq!(
            drawn, expected,
            "{name}: the x axis reads {drawn:?} left to right, but the source's categories are \
             {expected:?} — every category label must draw its own words"
        );

        // (2) bottom to top, the value axis increases.
        let mut ticks: Vec<(Point, String)> = labels_with_prefix(&d, "ytick#");
        ticks.sort_by(|a, b| {
            b.0.y
                .partial_cmp(&a.0.y)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let values: Vec<f64> = ticks
            .iter()
            .map(|(_, t)| {
                t.parse::<f64>()
                    .unwrap_or_else(|_| panic!("{name}: a y tick reading {t:?} is not a number"))
            })
            .collect();
        for w in values.windows(2) {
            assert!(
                w[1] > w[0],
                "{name}: the y ticks read {values:?} from the bottom up — a tick carrying its \
                 neighbour's number reads as a different chart"
            );
        }

        // (3) each bar stands over its own category.
        let anchors: Vec<(usize, Point)> = {
            let mut v = labels_with_prefix(&d, "xtick#");
            v.sort_by(|a, b| {
                a.0.x
                    .partial_cmp(&b.0.x)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            v.into_iter()
                .enumerate()
                .map(|(i, (p, _))| (i, Point::new(p.x, 0.0)))
                .collect()
        };
        if anchors.is_empty() {
            continue;
        }
        for node in d.nodes.iter().filter(|n| n.shape == Glyph::ChartBar) {
            // The legend's swatches are bars too, and they stand beside the plot rather than in
            // it, so only the ones the plot drew are asked about.
            if node.id.starts_with("legend#") {
                continue;
            }
            let slot: usize = node
                .id
                .rsplit('#')
                .next()
                .and_then(|s| s.parse().ok())
                .unwrap_or_else(|| panic!("{name}: a bar with no slot in its id: {}", node.id));
            let got = nearest(&Point::new(node.center.x, 0.0), &anchors);
            assert_eq!(
                got, slot,
                "{name}: the bar for category {slot} stands nearest the label for category {got}"
            );
        }
    }
}

/// **quadrantChart** — the name of a quadrant is drawn *in* that quadrant, a point's name is
/// nearest its own dot, and the four axis words are on the four right sides.
#[test]
fn a_quadrant_charts_names_are_drawn_in_the_regions_and_beside_the_points_they_name() {
    if !text_metrics::fonts_available() {
        return;
    }
    for (name, src) in cases_of(chart::quadrant::is_quadrant_chart) {
        let model = chart::quadrant::parse(src).expect("parses");
        let d = chart_diagram(src);

        // The plot square, taken from the drawing rather than recomputed: it is the frame node.
        let frame = d
            .nodes
            .iter()
            .find(|n| n.shape == Glyph::PlotFrame)
            .unwrap_or_else(|| panic!("{name}: no quadrant frame"));
        let (fl, ft, fr, fb) = frame.bounds();
        let (mx, my) = ((fl + fr) / 2.0, (ft + fb) / 2.0);

        // (a) quadrant-1 is the top right, and mermaid numbers anticlockwise from there.
        for (text, want_right, want_top) in [
            (&model.quadrant1, true, true),
            (&model.quadrant2, false, true),
            (&model.quadrant3, false, false),
            (&model.quadrant4, true, false),
        ] {
            let drawn = as_drawn(text);
            if drawn.trim().is_empty() {
                continue;
            }
            let node = d
                .nodes
                .iter()
                .find(|n| n.shape == Glyph::ChartLabel && words(&n.label) == drawn)
                .unwrap_or_else(|| panic!("{name}: {drawn:?} was not drawn at all"));
            assert_eq!(
                node.center.x > mx,
                want_right,
                "{name}: {drawn:?} is on the wrong side of the vertical divide"
            );
            assert_eq!(
                node.center.y < my,
                want_top,
                "{name}: {drawn:?} is in the wrong half of the square"
            );
        }

        // (b) a point's name is nearer to its own dot than to any other.
        let dots: Vec<(usize, Point)> = d
            .nodes
            .iter()
            .filter(|n| n.shape == Glyph::ChartPoint)
            .map(|n| {
                let i: usize = n.id.trim_start_matches("point#").parse().expect("point id");
                (i, n.center.clone())
            })
            .collect();
        for node in d.nodes.iter() {
            if node.shape != Glyph::ChartLabel || !node.id.ends_with("#label") {
                continue;
            }
            let i: usize = node
                .id
                .trim_start_matches("point#")
                .trim_end_matches("#label")
                .parse()
                .expect("point label id");
            assert_eq!(
                words(&node.label),
                as_drawn(&model.points[i].label),
                "{name}: the label beside point {i} does not read that point's own name"
            );
            assert_eq!(
                nearest(&node.center, &dots),
                i,
                "{name}: the label {:?} is nearer to another point's dot than to its own",
                words(&node.label)
            );
        }

        // (c) the axis words: left before right, bottom below top.
        let at = |id: &str| {
            d.nodes
                .iter()
                .find(|n| n.shape == Glyph::ChartLabel && n.id == id)
                .map(|n| (n.center.clone(), words(&n.label)))
        };
        if let (Some((l, lt)), Some((r, rt))) = (at("xaxis#left"), at("xaxis#right")) {
            assert_eq!(lt, as_drawn(&model.x_left), "{name}: the left x word");
            assert_eq!(rt, as_drawn(&model.x_right), "{name}: the right x word");
            assert!(
                l.x < r.x,
                "{name}: the x axis words are the wrong way round"
            );
        }
        if let (Some((b, bt)), Some((t, tt))) = (at("yaxis#bottom"), at("yaxis#top")) {
            assert_eq!(bt, as_drawn(&model.y_bottom), "{name}: the bottom y word");
            assert_eq!(tt, as_drawn(&model.y_top), "{name}: the top y word");
            assert!(b.y > t.y, "{name}: the y axis words are upside down");
        }
    }
}

/// **radar** — an axis label is drawn beyond *its own* spoke, and the legend names the curves in
/// source order.
///
/// The spoke's direction is not recomputed: it is read off the curve the renderer drew, whose
/// `k`th vertex is by construction on the `k`th axis. So the statement is "the words naming axis
/// `k` are the ones nearest, in angle, to the data plotted on axis `k`" — which a label moved to
/// its neighbour's spoke cannot satisfy.
#[test]
fn a_radar_charts_axis_labels_stand_beyond_their_own_spokes() {
    if !text_metrics::fonts_available() {
        return;
    }
    for (name, src) in cases_of(chart::radar::is_radar) {
        let model = chart::radar::parse(src).expect("parses");
        let d = chart_diagram(src);
        let centre = d
            .nodes
            .iter()
            .find(|n| n.shape == Glyph::Graticule)
            .map(|n| n.center.clone())
            .unwrap_or_else(|| panic!("{name}: no graticule"));

        // The angle of each axis, read off the drawn curve. Any curve will do — they all share
        // the spokes — so the first one with a vertex clear of the centre is used.
        let curve = d
            .edges
            .iter()
            .find(|e| e.series.is_some() && e.points.len() > model.axes.len())
            .unwrap_or_else(|| panic!("{name}: no curve was drawn"));
        let angle = |p: &Point| (p.y - centre.y).atan2(p.x - centre.x);
        let spokes: Vec<(usize, f64)> = curve
            .points
            .iter()
            .take(model.axes.len())
            .enumerate()
            .filter(|(_, p)| (p.x - centre.x).hypot(p.y - centre.y) > 1.0)
            .map(|(k, p)| (k, angle(p)))
            .collect();
        assert!(
            !spokes.is_empty(),
            "{name}: every curve vertex is at the centre, so no spoke direction can be read"
        );

        for (k, axis) in model.axes.iter().enumerate() {
            let Some((_, want)) = spokes.iter().find(|(i, _)| *i == k) else {
                continue;
            };
            let drawn = as_drawn(&axis.label);
            if drawn.trim().is_empty() {
                continue;
            }
            let node = d
                .nodes
                .iter()
                .find(|n| n.shape == Glyph::ChartLabel && n.id == format!("axis#{}", axis.name))
                .unwrap_or_else(|| panic!("{name}: axis {:?} has no label", axis.name));
            assert_eq!(
                words(&node.label),
                drawn,
                "{name}: the label on axis {:?} does not read that axis's own words",
                axis.name
            );
            // The nearest spoke, in angle, has to be its own.
            let got = spokes
                .iter()
                .min_by(|a, b| {
                    let da = angular_gap(angle(&node.center), a.1);
                    let db = angular_gap(angle(&node.center), b.1);
                    da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
                })
                .map(|(i, _)| *i)
                .expect("a spoke");
            assert_eq!(
                got,
                k,
                "{name}: the words {drawn:?} name axis {k} but stand beyond spoke {got} \
                 (at {:.1}°, the spoke is at {:.1}°)",
                angle(&node.center).to_degrees(),
                want.to_degrees()
            );
        }

        // The legend, top to bottom, is the curves in source order.
        if model.options.show_legend && !model.curves.is_empty() {
            let expected: Vec<String> = model.curves.iter().map(|c| as_drawn(&c.label)).collect();
            assert_eq!(
                legend_down(&d),
                expected,
                "{name}: the legend does not name the curves in the order they were written"
            );
        }
    }
}

/// How far right of the further participant a self-call's loop is allowed to reach.
///
/// A message from a participant to itself is not a straight line between two columns: it steps
/// out, down and back. The number is generous on purpose — this assertion is about a message drawn
/// between the *wrong* participants, which is a whole column away, not about the loop's width.
const SELF_CALL_REACH: f64 = 120.0;

/// The smaller of the two ways round between two angles, in radians.
fn angular_gap(a: f64, b: f64) -> f64 {
    let mut d = (a - b).abs() % (std::f64::consts::PI * 2.0);
    if d > std::f64::consts::PI {
        d = std::f64::consts::PI * 2.0 - d;
    }
    d
}

/// **treemap** — a tile's words name the datum whose area it is, and a child's tile is inside the
/// tile of the section that owns it.
///
/// Containment is what makes this a *placement* test rather than a spelling one: shuffle the
/// names between tiles and a leaf ends up outside the section it is named under.
#[test]
fn a_treemaps_tiles_name_their_own_data_and_nest_the_way_the_source_nests() {
    if !text_metrics::fonts_available() {
        return;
    }
    for (name, src) in cases_of(chart::treemap::is_treemap) {
        let model = chart::treemap::parse(src).expect("parses");
        let d = chart_diagram(src);

        // Where each name was drawn. A name drawn twice is not usable as an anchor, so those are
        // recorded and skipped rather than silently taking the first.
        let mut seen: HashMap<String, Vec<(f64, f64, f64, f64)>> = HashMap::new();
        for node in d.nodes.iter().filter(|n| n.shape == Glyph::ChartBar) {
            let Some(first) = panel_rows(node).into_iter().next() else {
                continue;
            };
            seen.entry(first).or_default().push(node.bounds());
        }

        // Walk the model's tree and state the two things about each node.
        fn walk(
            name: &str,
            items: &[chart::treemap::Node],
            outer: Option<(f64, f64, f64, f64)>,
            seen: &HashMap<String, Vec<(f64, f64, f64, f64)>>,
            hit: &mut usize,
        ) {
            for item in items {
                let text = as_drawn(&item.name);
                // A tile too small for its words draws none, which is a deliberate drop
                // (`tile_panel`), so a missing entry is not a failure — a *misplaced* one is.
                let boxes = match seen.get(&text) {
                    Some(b) if b.len() == 1 => b,
                    _ => {
                        walk(name, &item.children, None, seen, hit);
                        continue;
                    }
                };
                let mine = boxes[0];
                if let Some(o) = outer {
                    assert!(
                        contains(o, mine),
                        "{name}: the tile reading {text:?} is drawn outside the section that \
                         owns it"
                    );
                }
                *hit += 1;
                walk(name, &item.children, Some(mine), seen, hit);
            }
        }
        let mut hit = 0usize;
        walk(name, &model.roots, None, &seen, &mut hit);
        assert!(
            hit > 0,
            "{name}: no tile's words could be matched to a datum"
        );
    }
}

/// **packet** — a field's bit range and its name are drawn in *that field's* block, and the blocks
/// run in bit order.
#[test]
fn a_packet_diagrams_fields_are_labelled_with_their_own_bits_and_names_in_bit_order() {
    if !text_metrics::fonts_available() {
        return;
    }
    for (name, src) in cases_of(chart::packet::is_packet) {
        let model = chart::packet::parse(src).expect("parses");
        let d = chart_diagram(src);

        // Reading order: rows top to bottom, fields left to right within a row.
        let mut blocks: Vec<(f64, f64, Vec<String>)> = d
            .nodes
            .iter()
            .filter(|n| n.shape == Glyph::ChartBar && n.id.starts_with("field#"))
            .map(|n| (n.center.y, n.center.x, panel_rows(n)))
            .collect();
        blocks.sort_by(|a, b| {
            a.0.partial_cmp(&b.0)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
        });

        let fields: Vec<&chart::packet::Field> = model.rows.iter().flatten().collect();
        assert_eq!(
            blocks.len(),
            fields.len(),
            "{name}: {} blocks drawn for {} fields",
            blocks.len(),
            fields.len()
        );
        for (i, (field, (_, _, rows))) in fields.iter().zip(blocks.iter()).enumerate() {
            let range = if field.start == field.end {
                format!("{}", field.start)
            } else {
                format!("{}-{}", field.start, field.end)
            };
            let label = as_drawn(&field.label);
            // Both are dropped rather than overhanging when the block is too narrow, so what is
            // stated is that whatever *is* drawn belongs to this field.
            for row in rows {
                assert!(
                    *row == range || *row == label,
                    "{name}: the block in position {i} — which is bits {range} — is labelled \
                     {row:?}, which belongs to another field"
                );
            }
        }
    }
}

/// **sankey** — a node's name is drawn beside *that node's* bar.
#[test]
fn a_sankey_diagrams_names_stand_beside_their_own_bars() {
    if !text_metrics::fonts_available() {
        return;
    }
    for (name, src) in cases_of(chart::sankey::is_sankey) {
        let model = chart::sankey::parse(src).expect("parses");
        let d = chart_diagram(src);
        let bars: Vec<(usize, Point)> = d
            .nodes
            .iter()
            .filter(|n| n.shape == Glyph::ChartBar && n.id.starts_with("node#"))
            .map(|n| {
                let i: usize = n.id.trim_start_matches("node#").parse().expect("node id");
                (i, n.center.clone())
            })
            .collect();
        assert_eq!(bars.len(), model.nodes.len(), "{name}: one bar per node");

        let mut checked = 0usize;
        for node in d.nodes.iter() {
            if node.shape != Glyph::ChartLabel || !node.id.ends_with("#label") {
                continue;
            }
            let i: usize = node
                .id
                .trim_start_matches("node#")
                .trim_end_matches("#label")
                .parse()
                .expect("node label id");
            assert_eq!(
                words(&node.label),
                as_drawn(&model.nodes[i]),
                "{name}: the name drawn for node {i} is not that node's own"
            );
            let bar = bars
                .iter()
                .find(|(j, _)| *j == i)
                .map(|(_, p)| p.clone())
                .expect("its bar");
            assert!(
                (node.center.y - bar.y).abs() <= 0.5,
                "{name}: {:?} is not on the centre line of its own bar",
                words(&node.label)
            );
            assert_eq!(
                nearest(&node.center, &bars),
                i,
                "{name}: {:?} stands nearer to another node's bar than to its own",
                words(&node.label)
            );
            checked += 1;
        }
        assert_eq!(checked, model.nodes.len(), "{name}: every node is named");
    }
}

// ---------------------------------------------------------------------------------------------
// The four graph languages, and the sequence diagram
// ---------------------------------------------------------------------------------------------

/// **flowchart** — every box holds its own node's words, every line carries its own edge's, and
/// every frame its own block's title.
#[test]
fn a_flowcharts_boxes_lines_and_frames_each_carry_their_own_source_text() {
    if !text_metrics::fonts_available() {
        return;
    }
    for (name, src) in FLOW_CASES {
        let model = crate::preview::mermaid::flowchart::parse(src).expect("parses");
        let d = super::lay_out(&model).expect("lays out");

        for node in &model.nodes {
            let placed = d
                .node(&node.id)
                .unwrap_or_else(|| panic!("{name}: node {:?} was not drawn", node.id));
            assert_eq!(
                words(&placed.label),
                as_drawn(&node.label),
                "{name}: the box for {:?} holds another node's words",
                node.id
            );
        }

        // The drawable edges, in source order — which is the order `lay_out_spec` places them, so
        // the `k`th drawn edge is the `k`th drawable one and its words must be that edge's.
        let drawable: Vec<&crate::preview::mermaid::flowchart::Edge> = model
            .edges
            .iter()
            .filter(|e| {
                let known = |id: &str| {
                    model.node(id).is_some()
                        || model.subgraphs.iter().any(|s| s.id == id)
                        || d.cluster(id).is_some()
                };
                known(&e.from) && known(&e.to)
            })
            .collect();
        assert_eq!(drawable.len(), d.edges.len(), "{name}: edge count");
        for (k, (want, got)) in drawable.iter().zip(d.edges.iter()).enumerate() {
            assert_eq!(
                (got.from.as_str(), got.to.as_str()),
                (want.from.as_str(), want.to.as_str()),
                "{name}: drawn edge {k} joins the wrong pair"
            );
            let expected = want
                .label
                .as_deref()
                .map(as_drawn)
                .filter(|t| !t.trim().is_empty());
            let drawn = got.label.as_ref().map(|l| words(&l.label));
            assert_eq!(
                drawn, expected,
                "{name}: the words on {}->{} are not that link's own",
                want.from, want.to
            );
        }

        for block in &model.subgraphs {
            let Some(frame) = d.cluster(&block.id) else {
                continue;
            };
            assert_eq!(
                words(&frame.title),
                as_drawn(&block.title),
                "{name}: the frame {:?} is titled with another block's words",
                block.id
            );
        }
    }
}

/// **stateDiagram** — a state's words are in its own box, a transition's on its own arrow, and a
/// note's text is in the note that was written beside *that* state, on the side asked for.
#[test]
fn a_state_diagrams_boxes_arrows_and_notes_each_carry_their_own_source_text() {
    if !text_metrics::fonts_available() {
        return;
    }
    for (name, src) in STATE_CASES {
        let model = crate::preview::mermaid::state::parse(src).expect("parses");
        let d = state::lay_out(&model).expect("lays out");

        for s in &model.states {
            if s.kind.is_block() {
                if let Some(frame) = d.cluster(&s.id) {
                    assert_eq!(
                        words(&frame.title),
                        as_drawn(&s.label),
                        "{name}: the frame {:?} is titled with another state's words",
                        s.id
                    );
                }
                continue;
            }
            let Some(placed) = d.node(&s.id) else {
                continue;
            };
            assert_eq!(
                words(&placed.label),
                as_drawn(&s.label),
                "{name}: the box for {:?} holds another state's words",
                s.id
            );
        }

        // Transitions, in source order, skipping the tie-lines that hold notes on.
        let drawn: Vec<&super::PlacedEdge> = d
            .edges
            .iter()
            .filter(|e| {
                let is_note = |id: &str| d.node(id).is_some_and(|n| n.shape == Glyph::Note);
                !is_note(&e.from) && !is_note(&e.to)
            })
            .collect();
        let wanted: Vec<&crate::preview::mermaid::state::Transition> = model
            .transitions
            .iter()
            .filter(|t| !t.is_note_link)
            .filter(|t| {
                let known = |id: &str| d.node(id).is_some() || d.cluster(id).is_some();
                known(&t.from) && known(&t.to)
            })
            .collect();
        assert_eq!(drawn.len(), wanted.len(), "{name}: transition count");
        for (t, e) in wanted.iter().zip(drawn.iter()) {
            let expected = t
                .label
                .as_deref()
                .map(as_drawn)
                .filter(|s| !s.trim().is_empty());
            assert_eq!(
                e.label.as_ref().map(|l| words(&l.label)),
                expected,
                "{name}: the words on {}->{} are not that transition's own",
                t.from,
                t.to
            );
        }

        // Notes: the right words, beside the right state, on the right side.
        for note in model
            .states
            .iter()
            .filter(|s| s.kind == crate::preview::mermaid::state::Kind::Note)
        {
            let Some(placed) = d.node(&note.id) else {
                continue;
            };
            assert_eq!(
                words(&placed.label),
                as_drawn(&note.label),
                "{name}: the note {:?} holds another note's words",
                note.id
            );
            let Some(link) = model
                .transitions
                .iter()
                .find(|t| t.is_note_link && (t.from == note.id || t.to == note.id))
            else {
                continue;
            };
            let anchor_id = if link.from == note.id {
                &link.to
            } else {
                &link.from
            };
            let anchor = d
                .node(anchor_id)
                .map(|n| n.center.clone())
                .or_else(|| d.cluster(anchor_id).map(|c| c.center.clone()));
            let (Some(anchor), Some(position)) = (anchor, note.note_position) else {
                continue;
            };
            let right = placed.center.x > anchor.x;
            assert_eq!(
                right,
                position == crate::preview::mermaid::state::NotePosition::Right,
                "{name}: the note {:?} was written {position:?} of {anchor_id} and is drawn on \
                 the other side",
                words(&placed.label)
            );
        }
    }
}

/// **classDiagram** — a class box's compartments hold *that class's* name, annotations, attributes
/// and methods, in the order they were written.
///
/// Stated over the whole panel at once rather than row by row: a mutation that copies one class's
/// members into every box, drops the methods, or reverses the compartments all fail the same
/// assertion.
#[test]
fn a_class_boxs_compartments_hold_that_classs_own_members_in_order() {
    if !text_metrics::fonts_available() {
        return;
    }
    for (name, src) in CLASS_CASES {
        let model = crate::preview::mermaid::class::parse(src).expect("parses");
        let d = class::lay_out(&model).expect("lays out");

        for c in &model.classes {
            let placed = d
                .node(&c.id)
                .unwrap_or_else(|| panic!("{name}: class {:?} was not drawn", c.id));
            let mut want: Vec<String> = c
                .annotations
                .iter()
                .map(|a| as_drawn(&format!("«{a}»")))
                .collect();
            want.push(as_drawn(&c.label));
            want.extend(c.attributes.iter().map(|m| as_drawn(&m.display())));
            want.extend(c.methods.iter().map(|m| as_drawn(&m.display())));
            assert_eq!(
                panel_rows(placed),
                want,
                "{name}: the box for {:?} does not hold that class's own rows in order",
                c.id
            );
        }

        for note in &model.notes {
            let Some(placed) = d.node(&note.id) else {
                continue;
            };
            assert_eq!(
                words(&placed.label),
                as_drawn(&note.text),
                "{name}: the note {:?} holds another note's words",
                note.id
            );
        }

        for ns in &model.namespaces {
            let Some(frame) = d.cluster(&ns.id) else {
                continue;
            };
            assert_eq!(
                words(&frame.title),
                as_drawn(&ns.label),
                "{name}: the namespace frame {:?} is titled with other words",
                ns.id
            );
        }
    }
}

/// **erDiagram** — an entity's grid holds *that entity's* name and attribute rows, in order.
#[test]
fn an_entitys_grid_holds_that_entitys_own_attribute_rows_in_order() {
    if !text_metrics::fonts_available() {
        return;
    }
    for (name, src) in ER_CASES {
        let model = crate::preview::mermaid::er::parse(src).expect("parses");
        let d = er::lay_out(&model).expect("lays out");

        for e in &model.entities {
            let placed = d
                .node(&e.id)
                .unwrap_or_else(|| panic!("{name}: entity {:?} was not drawn", e.id));
            let mut want = vec![as_drawn(&e.label)];
            for a in &e.attributes {
                let cells = [
                    as_drawn(&a.kind),
                    as_drawn(&a.name),
                    as_drawn(&a.keys.join(", ")),
                    as_drawn(&a.comment),
                ];
                want.push(
                    cells
                        .iter()
                        .filter(|c| !c.trim().is_empty())
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(" "),
                );
            }
            assert_eq!(
                panel_rows(placed),
                want,
                "{name}: the grid for {:?} does not hold that entity's own rows in order",
                e.id
            );
        }

        for r in &model.relationships {
            let Some(edge) = d
                .edges
                .iter()
                .find(|edge| edge.from == r.from && edge.to == r.to)
            else {
                continue;
            };
            let expected = r
                .label
                .as_deref()
                .map(as_drawn)
                .filter(|s| !s.trim().is_empty());
            assert_eq!(
                edge.label.as_ref().map(|l| words(&l.label)),
                expected,
                "{name}: the verb on {} -- {} is not that relationship's own",
                r.from,
                r.to
            );
        }
    }
}

/// **sequenceDiagram** — a message's words are drawn above *that message's* arrow, a participant's
/// name is in its own box and on its own lifeline, and a note's text is over the participants the
/// source named.
#[test]
fn a_sequence_diagrams_messages_participants_and_notes_each_carry_their_own_text() {
    if !text_metrics::fonts_available() {
        return;
    }
    for (name, src) in SEQUENCE_CASES {
        let model = crate::preview::mermaid::sequence::parse(src).expect("parses");
        let d = sequence::lay_out(&model).expect("lays out");

        // (a) participants: name in the box, box over the lifeline.
        for p in &model.participants {
            let Some(node) = d.nodes.iter().find(|n| n.id == p.id) else {
                continue;
            };
            let drawn = match &node.panel {
                Some(_) => panel_rows(node).join(" "),
                None => words(&node.label),
            };
            assert_eq!(
                drawn,
                as_drawn(&p.label),
                "{name}: the head box for {:?} holds another participant's name",
                p.id
            );
            let line = d
                .lifelines
                .iter()
                .find(|l| l.id == p.id)
                .unwrap_or_else(|| panic!("{name}: {:?} has no lifeline", p.id));
            assert!(
                (node.center.x - line.x).abs() <= 0.5,
                "{name}: the box naming {:?} does not stand on that participant's own lifeline",
                p.id
            );
        }

        // (b) messages, in the order the events list them.
        let messages: Vec<&crate::preview::mermaid::sequence::Message> = model
            .events
            .iter()
            .filter_map(|e| match e {
                crate::preview::mermaid::sequence::Event::Message(m) => Some(m),
                _ => None,
            })
            .collect();
        assert_eq!(messages.len(), d.edges.len(), "{name}: message count");
        let x_of: HashMap<&str, f64> = d.lifelines.iter().map(|l| (l.id.as_str(), l.x)).collect();
        for (k, (m, e)) in messages.iter().zip(d.edges.iter()).enumerate() {
            let expected = as_drawn(&m.text);
            let drawn = e.label.as_ref().map(|l| words(&l.label));
            if expected.trim().is_empty() {
                assert!(drawn.is_none(), "{name}: message {k} grew words of its own");
                continue;
            }
            assert_eq!(
                drawn.as_deref(),
                Some(expected.as_str()),
                "{name}: the words on message {k} ({} -> {}) are not that message's own",
                m.from,
                m.to
            );
            // …and they are drawn over that arrow: within its own horizontal span (a self-call
            // loops out to the right, so the span is the shaft's, not the two lifelines'), and
            // above its own shaft, which is where mermaid puts a message's text.
            let label = e.label.as_ref().expect("a label");
            let lo = e.points.iter().map(|p| p.x).fold(f64::INFINITY, f64::min);
            let hi = e
                .points
                .iter()
                .map(|p| p.x)
                .fold(f64::NEG_INFINITY, f64::max);
            assert!(
                label.center.x >= lo - label.size.w && label.center.x <= hi + label.size.w,
                "{name}: the words on message {k} are drawn away from the arrow they name"
            );
            let line_y = e.points.first().map(|p| p.y).unwrap_or(0.0);
            if m.from == m.to {
                // A self-call is a loop, so there is nothing above it but the message before; its
                // words go *beside* the loop, inside its own vertical extent.
                let foot = e
                    .points
                    .iter()
                    .map(|p| p.y)
                    .fold(f64::NEG_INFINITY, f64::max);
                assert!(
                    label.center.y >= line_y - 0.5 && label.center.y <= foot + 0.5,
                    "{name}: the words on the self-call {k} are drawn off its own loop"
                );
                assert!(
                    label.center.x > hi,
                    "{name}: the words on the self-call {k} are drawn over its own loop"
                );
            } else {
                assert!(
                    label.center.y < line_y + 0.5,
                    "{name}: the words on message {k} are drawn below their own arrow"
                );
            }
            // The two lifelines it joins are still what the message is *about*, so a message
            // whose text drifted onto a different pair of columns is caught too.
            if let (Some(&a), Some(&b)) = (x_of.get(m.from.as_str()), x_of.get(m.to.as_str())) {
                assert!(
                    lo >= a.min(b) - 0.5 && hi <= a.max(b) + SELF_CALL_REACH,
                    "{name}: message {k} ({} -> {}) is not drawn between its own participants",
                    m.from,
                    m.to
                );
            }
        }

        // (c) notes: the right text, over the right participants.
        let notes: Vec<&crate::preview::mermaid::sequence::Note> = model
            .events
            .iter()
            .filter_map(|e| match e {
                crate::preview::mermaid::sequence::Event::Note(n) => Some(n),
                _ => None,
            })
            .collect();
        let mut drawn_notes: Vec<&PlacedNode> =
            d.nodes.iter().filter(|n| n.shape == Glyph::Note).collect();
        drawn_notes.sort_by_key(|n| {
            n.id.trim_start_matches("note#")
                .parse::<usize>()
                .unwrap_or(usize::MAX)
        });
        assert_eq!(drawn_notes.len(), notes.len(), "{name}: note count");
        for (k, (want, got)) in notes.iter().zip(drawn_notes.iter()).enumerate() {
            assert_eq!(
                words(&got.label),
                as_drawn(&want.text),
                "{name}: note {k} holds another note's words"
            );
            let named: HashSet<&str> = want.actors.iter().map(|s| s.as_str()).collect();
            let mut mine: Vec<f64> = x_of
                .iter()
                .filter(|(id, _)| named.contains(*id))
                .map(|(_, x)| *x)
                .collect();
            if mine.is_empty() {
                continue;
            }
            mine.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            let (lo, hi) = (mine[0], mine[mine.len() - 1]);
            let reach = got.size.w.max(hi - lo) + 40.0;
            assert!(
                got.center.x >= lo - reach && got.center.x <= hi + reach,
                "{name}: note {k} is drawn away from the participants it names ({:?})",
                want.actors
            );
        }
    }
}
