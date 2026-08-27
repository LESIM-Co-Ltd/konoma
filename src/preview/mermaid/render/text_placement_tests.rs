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
use super::{class, er, sequence, state, Diagram, PlacedCluster, PlacedNode, RenderError};
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

/// Stage 5b's own corpus, and its dispatcher. The eleven languages this module's second
/// half is about are laid out by [`super::kinds_tests::laid_out`] — the same entry every
/// other stage-5b test goes through, so there is one place that knows which parser claims a
/// source and a case added to the corpus reaches these tests without being mentioned again.
use super::kinds_tests::{cases_of as kind_cases_of, laid_out as kind_diagram};

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

/// `f64` order, with this file's usual answer for a comparison that cannot be made.
fn cmp(a: f64, b: f64) -> std::cmp::Ordering {
    a.partial_cmp(&b).unwrap_or(std::cmp::Ordering::Equal)
}

/// The one box that holds `p`, or `None` when no box does or more than one does.
///
/// The workhorse for the second half of this module. An edge's endpoint has been clipped to the
/// outline it meets, so it lies inside *that* node's box — and the kinds this is used on are the
/// ones whose boxes do not overlap (`kinds_tests`' first invariant), so it lies inside no other.
/// That is what makes "the box at this end of this line" a statement about the drawing rather
/// than about the ids the renderer wrote on the edge, which is the whole difference between a
/// placement test and a spelling one.
fn node_at<'a>(d: &'a Diagram, p: &Point) -> Option<&'a PlacedNode> {
    let mut hit = d.nodes.iter().filter(|n| {
        let (l, t, r, b) = n.bounds();
        p.x >= l - 0.75 && p.x <= r + 0.75 && p.y >= t - 0.75 && p.y <= b + 0.75
    });
    let first = hit.next()?;
    hit.next().is_none().then_some(first)
}

/// Every word a node draws, whether they live in its own label or in its panel's rows.
fn node_text(n: &PlacedNode) -> String {
    match n.panel {
        Some(_) => panel_rows(n).join(" "),
        None => words(&n.label),
    }
}

/// The lines a node draws, top to bottom — a C4 caption is three of them.
fn node_lines(n: &PlacedNode) -> Vec<String> {
    match n.panel {
        Some(_) => panel_rows(n),
        None => n
            .label
            .lines
            .iter()
            .filter(|l| !l.trim().is_empty())
            .cloned()
            .collect(),
    }
}

/// How many frames strictly hold this one — its nesting depth, read off the drawing.
///
/// Needed wherever two frames hold the same boxes: a `Deployment_Node` inside a `Deployment_Node`
/// contains exactly the same elements as its parent, so *what a frame contains* cannot tell the
/// two apart and *how deep it is* has to.
fn frame_depth(d: &Diagram, i: usize) -> usize {
    let me = d.clusters[i].bounds();
    d.clusters
        .iter()
        .enumerate()
        .filter(|(j, o)| *j != i && contains(o.bounds(), me))
        .count()
}

/// One grid's cells, read the way a grid is read: rows top to bottom, cells left to right.
///
/// A row is found by which boxes **overlap in y**, not by a row height, so a row holding a tall
/// block beside a short one is still one row — and a block that has landed on the wrong row stops
/// being read where its source says it is.
fn grid_order<'a>(cells: &[&'a PlacedNode]) -> Vec<&'a PlacedNode> {
    let mut sorted: Vec<&PlacedNode> = cells.to_vec();
    sorted.sort_by(|a, b| cmp(a.bounds().1, b.bounds().1));
    let mut out: Vec<&PlacedNode> = Vec::new();
    let mut row: Vec<&PlacedNode> = Vec::new();
    // The shallowest box in the row so far: a box whose top is below it shares no y with the row.
    let mut floor = f64::INFINITY;
    for n in sorted {
        let (_, t, _, b) = n.bounds();
        if !row.is_empty() && t > floor - 0.5 {
            row.sort_by(|a, b| cmp(a.center.x, b.center.x));
            out.append(&mut row);
            floor = f64::INFINITY;
        }
        floor = floor.min(b);
        row.push(n);
    }
    row.sort_by(|a, b| cmp(a.center.x, b.center.x));
    out.append(&mut row);
    out
}

/// The runs of consecutive items that share a band's name, as `(name, member indices)`.
///
/// A journey's sections, a gantt's and a timeline's are all this: `band` opens a frame when the
/// name changes and closes it when it changes again, so a name written twice with something else
/// between draws two frames — which is what the source says happened. A run with no name draws no
/// frame at all (`sections.retain`), so those are dropped here too.
fn bands(names: &[String]) -> Vec<(String, Vec<usize>)> {
    let mut runs: Vec<(String, Vec<usize>)> = Vec::new();
    for (i, name) in names.iter().enumerate() {
        match runs.last_mut() {
            Some((open, members)) if open == name => members.push(i),
            _ => runs.push((name.clone(), vec![i])),
        }
    }
    runs.retain(|(name, _)| !Label::measure(name).is_blank());
    runs
}

/// States, for one banded diagram, that each frame is titled with its own band's name and holds
/// exactly its own band's marks — with the frames taken **in the order they are drawn along the
/// axis**, which is the order the bands were written in.
fn check_bands(
    name: &str,
    d: &Diagram,
    marks: &[&PlacedNode],
    runs: &[(String, Vec<usize>)],
    along: impl Fn(&Point) -> f64,
) {
    let mut frames: Vec<&PlacedCluster> = d.clusters.iter().collect();
    frames.sort_by(|a, b| cmp(along(&a.center), along(&b.center)));
    assert_eq!(
        frames.len(),
        runs.len(),
        "{name}: {} frames were drawn for {} bands",
        frames.len(),
        runs.len()
    );
    for (frame, (band, members)) in frames.iter().zip(runs.iter()) {
        assert_eq!(
            words(&frame.title),
            as_drawn(band),
            "{name}: a frame is titled with another band's words"
        );
        let inside: Vec<usize> = marks
            .iter()
            .enumerate()
            .filter(|(_, m)| contains(frame.bounds(), m.bounds()))
            .map(|(k, _)| k)
            .collect();
        assert_eq!(
            &inside, members,
            "{name}: the frame reading {band:?} holds the marks at {inside:?}, and that band's \
             own are at {members:?}"
        );
    }
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
        //
        // **By the label's id**, not by finding whichever node happens to draw these words. The
        // id is the corner's own place in reading order — `quadrant#0` is the top left,
        // `quadrant#1` the top right, then the two below (`render::chart::quadrant`) — so it says
        // which corner is being asked about without saying what is written there, and the words
        // can then be asserted as well. The older form could only ask where a string had landed,
        // which is one half of the statement and the wrong half to keep on its own: it says
        // nothing at all about a corner, and two quadrants written with the same words would have
        // made it fail for a reason that is not a defect.
        for (text, id, want_right, want_top) in [
            (&model.quadrant1, "quadrant#1", true, true),
            (&model.quadrant2, "quadrant#0", false, true),
            (&model.quadrant3, "quadrant#2", false, false),
            (&model.quadrant4, "quadrant#3", true, false),
        ] {
            let drawn = as_drawn(text);
            if drawn.trim().is_empty() {
                continue;
            }
            let node = d
                .nodes
                .iter()
                .find(|n| n.shape == Glyph::ChartLabel && n.id == id)
                .unwrap_or_else(|| panic!("{name}: {id} was not drawn at all"));
            assert_eq!(
                words(&node.label),
                drawn,
                "{name}: {id} is where {drawn:?} belongs and it names another quadrant"
            );
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
/// # The tile is found by its id, never by the words it draws
///
/// The first version of this test built a map from *drawn text* to tile and looked each datum up
/// by its own name. That is §6-A item 15 exactly, and it was demonstrated rather than argued:
/// making every leaf draw its **next sibling's** name left this test green, because a rotation
/// does not change the multiset of strings — every name still found a tile, and for siblings the
/// tile it found was still inside the same section, so the nesting half passed too. The only test
/// that went red was [`super::chart::tests::a_treemap_tile_carries_its_own_name_and_its_sections_colour`],
/// which anchors on the id.
///
/// So this anchors on the id as well. `tile#k` is the node's own index in a depth-first walk of
/// the source (`render::chart::treemap::place`), which the walk below recomputes from
/// [`super::chart::treemap::subtree_len`] — the same arithmetic the renderer does, but driven off
/// the *model's* shape rather than off anything the renderer drew. What is then stated about that
/// tile is what it says and where it sits, and both halves fail under a rotation.
///
/// Containment is what keeps this a *placement* test rather than a spelling one: a leaf whose
/// tile has moved out of its section fails even when every name is right.
#[test]
fn a_treemaps_tiles_name_their_own_data_and_nest_the_way_the_source_nests() {
    if !text_metrics::fonts_available() {
        return;
    }
    for (name, src) in cases_of(chart::treemap::is_treemap) {
        let model = chart::treemap::parse(src).expect("parses");
        let d = chart_diagram(src);

        /// Walks the model's tree, stating for each node what its own tile says and where it sits.
        ///
        /// `base` is the depth-first index of `items[0]`; a node owns the span of indices its
        /// whole subtree covers, so the next sibling starts `subtree_len` further along.
        fn walk(
            name: &str,
            items: &[chart::treemap::Node],
            base: usize,
            outer: Option<(f64, f64, f64, f64)>,
            d: &Diagram,
            hit: &mut usize,
        ) {
            let mut index = base;
            for item in items {
                let me = index;
                index += draw_chart::treemap::subtree_len(item);
                // A tile with no area is not drawn at all, and neither is anything under it.
                let Some(tile) = d.nodes.iter().find(|n| n.id == format!("tile#{me}")) else {
                    walk(name, &item.children, me + 1, None, d, hit);
                    continue;
                };
                let mine = tile.bounds();
                if let Some(o) = outer {
                    assert!(
                        contains(o, mine),
                        "{name}: tile#{me} is the tile for {:?}, and it is drawn outside the \
                         section that owns it",
                        item.name
                    );
                }
                // A tile too small for its words draws none, which is a deliberate drop
                // (`tile_panel`), so an empty panel is not a failure — the wrong words are.
                let rows = panel_rows(tile);
                if let Some(first) = rows.first() {
                    assert_eq!(
                        *first,
                        as_drawn(&item.name),
                        "{name}: tile#{me} is where {:?} belongs and it reads {first:?}",
                        item.name
                    );
                    *hit += 1;
                }
                // A leaf also carries its own number, and a rotation moves that too.
                if let (Some(v), Some(second)) = (item.value, rows.get(1)) {
                    assert_eq!(
                        *second,
                        as_drawn(&draw_chart::tick_text(v)),
                        "{name}: tile#{me} is where {:?} belongs and the number under its name \
                         reads {second:?}",
                        item.name
                    );
                }
                walk(name, &item.children, me + 1, Some(mine), d, hit);
            }
        }
        let mut hit = 0usize;
        walk(name, &model.roots, 0, None, &d, &mut hit);
        assert!(
            hit > 0,
            "{name}: not one tile carried words, so nothing was read back"
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

        check_sequence_text_placement(name, &model, &d);
    }
}

/// The three statements above, as one function, because **two languages reach this geometry**.
///
/// A `zenuml` source is read into the sequence *model* and drawn by the sequence renderer, so
/// these are exactly the right statements to make about one — see
/// [`a_zenuml_diagrams_participants_messages_and_notes_carry_their_own_translated_text`]. Calling
/// this rather than copying it is the rule stages 3, 4 and 5a settled: a copy that stops matching
/// its original is the failure konoma keeps hitting.
fn check_sequence_text_placement(
    name: &str,
    model: &crate::preview::mermaid::sequence::SequenceDiagram,
    d: &Diagram,
) {
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

// ---------------------------------------------------------------------------------------------
// The eleven of stage 5b
// ---------------------------------------------------------------------------------------------
//
// The twelve tests above were written for the kinds that existed when this module was; the eleven
// languages `cba308a` added had none, and the gap was not a guess. Labelling every gantt row with
// the **next** task's name — a rotation, which is what a real off-by-one produces — turned exactly
// one test red, the masked corpus golden, and no named test at all. A snapshot is the thing this
// project has twice lost behaviour to by regenerating it (§6-A), so a chart whose every row names
// the wrong task shipped green.
//
// A rotation is also the right acceptance test for what is written below, and for the same reason
// it defeats the older instruments: it leaves the **multiset of strings unchanged**. The size
// check in `kinds_tests` looks each drawn string up by *whichever* node owns it, so under a
// permutation every string still finds an owner and every box is still wide enough; it is
// structurally blind to mis-pairing. "Is this text somewhere on the page" is worth nothing here.
// Only "is this text at **this datum's** place" is.

/// **gantt** — the name beside a bar is that bar's own task, the ticks read in date order, and a
/// task sits in its own section's frame.
///
/// The place is the **row**, read off the drawing: a gantt's rows are its tasks in source order,
/// so the `k`th mark down the page is the `k`th task and the label sharing its row is that task's
/// name. Nothing here recomputes a y.
#[test]
fn a_gantt_charts_rows_ticks_and_sections_each_carry_their_own_tasks_words() {
    if !text_metrics::fonts_available() {
        return;
    }
    use crate::preview::mermaid::gantt::{self, time};
    for (name, src) in kind_cases_of(gantt::is_gantt) {
        let model = gantt::parse(src).expect("parses");
        let d = kind_diagram(src);

        // The marks — a bar, or the diamond a milestone is drawn as — read down the page.
        let mut marks: Vec<&PlacedNode> = d
            .nodes
            .iter()
            .filter(|n| n.id.starts_with("task#") && !n.id.ends_with("#name"))
            .collect();
        marks.sort_by(|a, b| cmp(a.center.y, b.center.y));
        assert_eq!(
            marks.len(),
            model.tasks.len(),
            "{name}: {} marks were drawn for {} tasks",
            marks.len(),
            model.tasks.len()
        );

        // (1) the row names, top to bottom, spell the source's tasks. A label shares a row with
        // exactly one mark and stands to its left, which is where a gantt puts a row's name.
        let mut rows: Vec<String> = Vec::new();
        for mark in &marks {
            let beside: Vec<&PlacedNode> = d
                .nodes
                .iter()
                .filter(|n| n.shape == Glyph::ChartLabel)
                .filter(|n| {
                    (n.center.y - mark.center.y).abs() < 0.5 && n.center.x < mark.bounds().0
                })
                .collect();
            assert!(
                beside.len() <= 1,
                "{name}: {} labels share one row, so no one of them is that row's name",
                beside.len()
            );
            rows.push(beside.first().map(|n| words(&n.label)).unwrap_or_default());
        }
        let wanted: Vec<String> = model.tasks.iter().map(|t| as_drawn(&t.name)).collect();
        assert_eq!(
            rows, wanted,
            "{name}: the rows read {rows:?} top to bottom, but the source's tasks are {wanted:?} \
             — the name beside a bar has to be that bar's own task"
        );

        // (2) the ticks. Each carries the date its own mark stands on, written the way the source
        // asked for it, and they run left to right in date order.
        let mut ticks: Vec<(f64, time::Instant, String)> = d
            .nodes
            .iter()
            .filter(|n| n.shape == Glyph::ChartLabel && n.id.starts_with("tick#"))
            .map(|n| {
                let iso = n.id.trim_start_matches("tick#");
                let at = time::parse(iso, "YYYY-MM-DDTHH:mm")
                    .unwrap_or_else(|| panic!("{name}: a tick standing on no date: {}", n.id));
                (n.center.x, at, words(&n.label))
            })
            .collect();
        ticks.sort_by(|a, b| cmp(a.0, b.0));
        assert!(!ticks.is_empty(), "{name}: the time axis carries no ticks");
        let drawn: Vec<String> = ticks.iter().map(|(_, _, t)| t.clone()).collect();
        let expected: Vec<String> = ticks
            .iter()
            .map(|(_, at, _)| as_drawn(&time::format(*at, &model.axis_format)))
            .collect();
        assert_eq!(
            drawn, expected,
            "{name}: the axis reads {drawn:?} left to right, and the dates its ticks stand on are \
             {expected:?}"
        );
        for w in ticks.windows(2) {
            assert!(
                w[1].1 > w[0].1,
                "{name}: the axis reads {drawn:?} left to right, which is not date order"
            );
        }

        // (3) a task is in its own section's frame, and the frame carries that section's name.
        let sections: Vec<String> = model.tasks.iter().map(|t| t.section.clone()).collect();
        check_bands(name, &d, &marks, &bands(&sections), |p| p.y);
    }
}

/// **gitGraph** — a caption belongs to its own commit's dot, and a branch's name to its own lane.
///
/// The place is stated twice over, once for each axis of the picture. A caption is drawn half a
/// dot plus a gap **below** its own commit in all three directions, so the commit directly above
/// it on its own column is the one it captions. A lane's position is read off the commits made on
/// it, so nothing here recomputes the lane spacing.
#[test]
fn a_git_graphs_captions_and_lane_names_belong_to_their_own_commits_and_branches() {
    if !text_metrics::fonts_available() {
        return;
    }
    use crate::preview::mermaid::gitgraph::{self, Commit, Direction};
    for (name, src) in kind_cases_of(gitgraph::is_git_graph) {
        let model = gitgraph::parse(src).expect("parses");
        let d = kind_diagram(src);
        let across = |p: &Point| match model.direction {
            Direction::LeftToRight => p.y,
            _ => p.x,
        };

        // What a commit has to say for itself: its tags, or the id **the author wrote**. A
        // generated id names nothing, so a commit that was given none is captioned with nothing.
        let says = |c: &Commit| -> String {
            if !c.tags.is_empty() {
                as_drawn(&c.tags.join(", "))
            } else if c.explicit_id {
                as_drawn(&c.id)
            } else {
                String::new()
            }
        };

        // (a) the captions.
        let dots: Vec<&PlacedNode> = model
            .commits
            .iter()
            .map(|c| {
                d.node(&c.id)
                    .unwrap_or_else(|| panic!("{name}: commit {:?} was not drawn", c.id))
            })
            .collect();
        let captions: Vec<&PlacedNode> = d
            .nodes
            .iter()
            .filter(|n| n.shape == Glyph::ChartLabel && n.id.ends_with("#caption"))
            .collect();
        for caption in &captions {
            let owner = dots
                .iter()
                .enumerate()
                .filter(|(_, dot)| {
                    (dot.center.x - caption.center.x).abs() < 0.5 && dot.center.y < caption.center.y
                })
                .max_by(|a, b| cmp(a.1.center.y, b.1.center.y))
                .map(|(i, _)| i)
                .unwrap_or_else(|| {
                    panic!(
                        "{name}: the caption {:?} stands under no commit at all",
                        words(&caption.label)
                    )
                });
            assert_eq!(
                words(&caption.label),
                says(&model.commits[owner]),
                "{name}: the caption under commit {:?} says something another commit said",
                model.commits[owner].id
            );
        }
        let carried = model.commits.iter().filter(|c| !says(c).is_empty()).count();
        assert_eq!(
            captions.len(),
            carried,
            "{name}: {} captions were drawn, and {carried} commits carry one",
            captions.len()
        );

        // (b) the lane names, read across the page.
        let lanes: Vec<&str> = model.lanes().iter().map(|b| b.name.as_str()).collect();
        let mut named: Vec<(f64, String)> = d
            .nodes
            .iter()
            .filter(|n| n.shape == Glyph::ChartLabel && n.id.starts_with("branch#"))
            .map(|n| (across(&n.center), words(&n.label)))
            .collect();
        named.sort_by(|a, b| cmp(a.0, b.0));
        let drawn: Vec<String> = named.iter().map(|(_, t)| t.clone()).collect();
        let expected: Vec<String> = lanes.iter().map(|b| as_drawn(b)).collect();
        assert_eq!(
            drawn, expected,
            "{name}: the lanes are named {drawn:?} across the page, and the source's branches are \
             {expected:?}"
        );
        // …and each name stands on the lane its own branch's commits are on.
        for (k, lane) in lanes.iter().enumerate() {
            let Some(sample) = model
                .commits
                .iter()
                .find(|c| c.branch == **lane)
                .and_then(|c| d.node(&c.id))
            else {
                continue;
            };
            assert!(
                (named[k].0 - across(&sample.center)).abs() < 0.5,
                "{name}: {:?} names branch {lane:?} and does not stand on the lane its commits \
                 are drawn on",
                named[k].1
            );
        }
    }
}

/// **mindmap** — a box holds its own node's words, and a line joins a parent's box to its own
/// child's.
///
/// The place is the **line**: both of its ends have been clipped to the outline they meet, so the
/// pair of boxes a line joins is read off the drawing. Shuffle the labels and the branches the
/// picture shows stop being the branches the source wrote, whichever way the tree was laid out.
///
/// # …except when the shuffle is between two leaves of one parent
///
/// That last sentence is not true of every shuffle, and the exception was demonstrated rather
/// than argued: making every **childless** node draw its next childless sibling's words left both
/// halves of this test green. The bag of drawn words is unchanged by any permutation at all, and
/// the branches are unchanged by this one, because a leaf's only appearance in a parent–child
/// pair is as the child — so rotating a parent's leaves rotates the second element of a set of
/// pairs that share a first, and the set comes back the same. **In the whole suite only the
/// corpus golden went red**, which is §6-A item 15 with the golden as the sole guard.
///
/// So the words are anchored on the id as well. A box's id is `n{i}`, its node's own index in the
/// source (`render::mindmap::spec_of` assigns it, because a mindmap's written ids are not unique
/// and cannot identify anything); the label beside it comes from `map.nodes[i].label`, which is a
/// different read of the same node, so a rotation moves one and not the other.
#[test]
fn a_mindmaps_boxes_hold_their_own_words_and_hang_off_their_own_parents() {
    if !text_metrics::fonts_available() {
        return;
    }
    use crate::preview::mermaid::mindmap;
    for (name, src) in kind_cases_of(mindmap::is_mindmap) {
        let model = mindmap::parse(src).expect("parses");
        let d = kind_diagram(src);

        for (i, node) in model.nodes.iter().enumerate() {
            let placed = d
                .node(&format!("n{i}"))
                .unwrap_or_else(|| panic!("{name}: node {i} was not drawn"));
            assert_eq!(
                words(&placed.label),
                as_drawn(&node.label),
                "{name}: n{i} is the box for {:?} and it holds another node's words",
                node.label
            );
        }

        let mut drawn: Vec<String> = d.nodes.iter().map(|n| words(&n.label)).collect();
        let mut wanted: Vec<String> = model.nodes.iter().map(|n| as_drawn(&n.label)).collect();
        drawn.sort();
        wanted.sort();
        assert_eq!(
            drawn, wanted,
            "{name}: the boxes read {drawn:?}, and the source's nodes are {wanted:?}"
        );

        let mut joined: Vec<(String, String)> = Vec::new();
        for e in &d.edges {
            let ends = (e.points.first(), e.points.last());
            let (Some(from), Some(to)) = ends else {
                panic!("{name}: a line with no ends")
            };
            let a =
                node_at(&d, from).unwrap_or_else(|| panic!("{name}: a line starts on no one box"));
            let b = node_at(&d, to).unwrap_or_else(|| panic!("{name}: a line ends on no one box"));
            joined.push((words(&a.label), words(&b.label)));
        }
        let mut branches: Vec<(String, String)> = model
            .nodes
            .iter()
            .filter_map(|n| {
                let parent = n.parent?;
                Some((as_drawn(&model.nodes[parent].label), as_drawn(&n.label)))
            })
            .collect();
        joined.sort();
        branches.sort();
        assert_eq!(
            joined, branches,
            "{name}: the lines join {joined:?}, and the source's branches are {branches:?} — a \
             child has to hang off its own parent's box"
        );
    }
}

/// **kanban** — a card's words are in its own column, in the order the column lists them.
///
/// The place is **containment plus reading order**: the columns run left to right in source
/// order, so the `k`th frame across is the `k`th column, and the cards inside it read down.
#[test]
fn a_kanban_boards_cards_are_in_their_own_columns_in_source_order() {
    if !text_metrics::fonts_available() {
        return;
    }
    use crate::preview::mermaid::kanban;
    for (name, src) in kind_cases_of(kanban::is_kanban) {
        let model = kanban::parse(src).expect("parses");
        let d = kind_diagram(src);

        let mut frames: Vec<&PlacedCluster> = d.clusters.iter().collect();
        frames.sort_by(|a, b| cmp(a.center.x, b.center.x));
        assert_eq!(
            frames.len(),
            model.columns.len(),
            "{name}: {} frames for {} columns",
            frames.len(),
            model.columns.len()
        );

        for (frame, column) in frames.iter().zip(model.columns.iter()) {
            let title = match column.ticket.as_deref().filter(|t| !t.trim().is_empty()) {
                Some(t) => format!("{} ({t})", column.label),
                None => column.label.clone(),
            };
            assert_eq!(
                words(&frame.title),
                as_drawn(&title),
                "{name}: a column's frame is titled with another column's words"
            );

            let mut inside: Vec<&PlacedNode> = d
                .nodes
                .iter()
                .filter(|n| contains(frame.bounds(), n.bounds()))
                .collect();
            inside.sort_by(|a, b| cmp(a.center.y, b.center.y));
            let drawn: Vec<Vec<String>> = inside.iter().map(|n| panel_rows(n)).collect();
            let wanted: Vec<Vec<String>> = column
                .cards
                .iter()
                .map(|c| {
                    // A card says its own words, and then whatever the source pinned to it.
                    // A panel row is **one drawn line**, so a card written with a two-line
                    // label is two rows — the same rule `panel::class_panel` follows, stated
                    // here so the model side and the drawing are counting the same things.
                    let mut rows: Vec<String> = Label::measure(&c.label).lines.clone();
                    for (key, value) in [
                        ("assigned", &c.assigned),
                        ("ticket", &c.ticket),
                        ("priority", &c.priority),
                    ] {
                        if let Some(v) = value.as_deref().filter(|v| !v.trim().is_empty()) {
                            rows.push(as_drawn(&format!("{key}: {v}")));
                        }
                    }
                    // A card written `[]` has no words, and a row with no words draws nothing —
                    // `panel_rows` reads what was drawn, so the model side has to drop the blank
                    // row too or the two would disagree about a card that is simply empty.
                    rows.retain(|r| !r.trim().is_empty());
                    rows
                })
                .collect();
            assert_eq!(
                drawn, wanted,
                "{name}: the column titled {title:?} holds {drawn:?} top to bottom, and its own \
                 cards are {wanted:?}"
            );
        }
    }
}

/// **journey** — a step's name, its score and the people on it all belong to that step, and the
/// step is in its own section's frame.
///
/// The place is the **column**: a journey is a row of steps in source order, and a step's face
/// stands above its own box and the people under it, on that box's own centre line. A score is
/// not text, so it is stated through the mark that carries it — the mouth curve **is** the number
/// (`journey`'s module docs), which is what makes a face on the wrong step a wrong reading rather
/// than a wrong colour.
#[test]
fn a_journeys_steps_carry_their_own_names_scores_and_people_in_their_own_sections() {
    if !text_metrics::fonts_available() {
        return;
    }
    use crate::preview::mermaid::flowchart::Shape;
    use crate::preview::mermaid::journey;
    for (name, src) in kind_cases_of(journey::is_journey) {
        let model = journey::parse(src).expect("parses");
        let d = kind_diagram(src);

        let mut boxes: Vec<&PlacedNode> = d
            .nodes
            .iter()
            .filter(|n| n.shape == Glyph::Flow(Shape::RoundedRect))
            .collect();
        boxes.sort_by(|a, b| cmp(a.center.x, b.center.x));
        assert_eq!(
            boxes.len(),
            model.tasks.len(),
            "{name}: {} boxes were drawn for {} steps",
            boxes.len(),
            model.tasks.len()
        );
        let drawn: Vec<String> = boxes.iter().map(|n| words(&n.label)).collect();
        let wanted: Vec<String> = model.tasks.iter().map(|t| as_drawn(&t.name)).collect();
        assert_eq!(
            drawn, wanted,
            "{name}: the steps read {drawn:?} left to right, and the source's are {wanted:?}"
        );

        for (k, step) in boxes.iter().enumerate() {
            let task = &model.tasks[k];
            let column = |n: &PlacedNode| (n.center.x - step.center.x).abs() < 0.5;

            // The face above this step's own box.
            let face = d
                .nodes
                .iter()
                .find(|n| n.shape == Glyph::Face && column(n) && n.center.y < step.center.y);
            match task.score {
                Some(score) => {
                    let face = face.unwrap_or_else(|| {
                        panic!(
                            "{name}: the step {:?} has no face over it",
                            words(&step.label)
                        )
                    });
                    assert_eq!(
                        face.mark,
                        Some(Mark::Face { score }),
                        "{name}: the face over {:?} is drawn at another step's score",
                        words(&step.label)
                    );
                }
                // An unreadable score draws no face, because a neutral one is a claim.
                None => assert!(
                    face.is_none(),
                    "{name}: the step {:?} has no readable score and grew a face anyway",
                    words(&step.label)
                ),
            }

            // The people under it, on the same column.
            let below: Vec<&PlacedNode> = d
                .nodes
                .iter()
                .filter(|n| n.shape == Glyph::ChartLabel && column(n) && n.center.y > step.center.y)
                .collect();
            let people = as_drawn(&task.people.join(", "));
            let said = below.first().map(|n| words(&n.label)).unwrap_or_default();
            assert!(
                below.len() <= 1,
                "{name}: {} names stand under the step {:?}",
                below.len(),
                words(&step.label)
            );
            assert_eq!(
                said,
                people,
                "{name}: {said:?} is drawn under the step {:?}, whose own people are {people:?}",
                words(&step.label)
            );
        }

        let sections: Vec<String> = model.tasks.iter().map(|t| t.section.clone()).collect();
        check_bands(name, &d, &boxes, &bands(&sections), |p| p.x);
    }
}

/// **timeline** — an event sits under its own period, and a period is in its own section's frame.
///
/// The place is a different relation in each direction, and that is the point: a left-to-right
/// timeline hangs a period's events in **its own column**, and a top-to-bottom one lays them
/// along **its own row** — where every event of every period shares one column, so the column
/// says nothing. Both are read off the drawing; neither is the id the renderer wrote.
#[test]
fn a_timelines_events_sit_under_their_own_periods_in_their_own_sections() {
    if !text_metrics::fonts_available() {
        return;
    }
    use crate::preview::mermaid::flowchart::Shape;
    use crate::preview::mermaid::timeline::{self, Direction};
    for (name, src) in kind_cases_of(timeline::is_timeline) {
        let model = timeline::parse(src).expect("parses");
        let d = kind_diagram(src);
        let along = |p: &Point| match model.direction {
            Direction::LeftToRight => p.x,
            Direction::TopToBottom => p.y,
        };

        let mut periods: Vec<&PlacedNode> = d
            .nodes
            .iter()
            .filter(|n| n.shape == Glyph::Flow(Shape::RoundedRect))
            .collect();
        periods.sort_by(|a, b| cmp(along(&a.center), along(&b.center)));
        assert_eq!(
            periods.len(),
            model.periods.len(),
            "{name}: {} periods were drawn for {}",
            periods.len(),
            model.periods.len()
        );
        let drawn: Vec<String> = periods.iter().map(|n| words(&n.label)).collect();
        let wanted: Vec<String> = model.periods.iter().map(|p| as_drawn(&p.name)).collect();
        assert_eq!(
            drawn, wanted,
            "{name}: the spine reads {drawn:?} along the axis, and the source's periods are \
             {wanted:?}"
        );

        let events: Vec<&PlacedNode> = d
            .nodes
            .iter()
            .filter(|n| n.shape == Glyph::Flow(Shape::Rect))
            .collect();
        // Which period each event belongs to, by the relation this direction makes available.
        let owner = |e: &PlacedNode| -> usize {
            match model.direction {
                Direction::LeftToRight => periods
                    .iter()
                    .position(|p| (p.center.x - e.center.x).abs() < 0.5)
                    .unwrap_or_else(|| {
                        panic!(
                            "{name}: the event {:?} hangs in no period's column",
                            words(&e.label)
                        )
                    }),
                Direction::TopToBottom => periods
                    .iter()
                    .enumerate()
                    .min_by(|a, b| {
                        cmp(
                            (a.1.center.y - e.center.y).abs(),
                            (b.1.center.y - e.center.y).abs(),
                        )
                    })
                    .map(|(i, _)| i)
                    .expect("a timeline has periods"),
            }
        };
        for (k, period) in model.periods.iter().enumerate() {
            let mut mine: Vec<&PlacedNode> =
                events.iter().copied().filter(|e| owner(e) == k).collect();
            mine.sort_by(|a, b| cmp(a.center.y, b.center.y));
            let drawn: Vec<String> = mine.iter().map(|e| words(&e.label)).collect();
            let wanted: Vec<String> = period.events.iter().map(|e| as_drawn(e)).collect();
            assert_eq!(
                drawn, wanted,
                "{name}: the period {:?} carries {drawn:?}, and its own events are {wanted:?}",
                period.name
            );
        }

        let sections: Vec<String> = model.periods.iter().map(|p| p.section.clone()).collect();
        check_bands(name, &d, &periods, &bands(&sections), along);
    }
}

/// **requirement** — a box holds its own body rows in order, and a verb belongs to its own line.
///
/// The place for a verb is the **line's two ends**, clipped to the outlines they meet, so a verb
/// on the wrong pair of boxes is caught wherever the layout happened to put them. That is also
/// what used to pin which box was which, and it only reaches a box some link names: making the
/// boxes **no link names** swap rows with each other left this test green, and in the whole suite
/// only the corpus golden went red (§6-A item 15). The corpus has such a box — `tested`, in the
/// case that writes four requirements and one link.
///
/// So the rows are anchored on the id, which for both a requirement and an element is its own
/// name (`render::requirement::box_node`). The bag comparison is kept underneath it because it is
/// the half that catches a row the drawing *added*.
#[test]
fn a_requirement_boxs_rows_and_a_verbs_line_belong_to_their_own_datum() {
    if !text_metrics::fonts_available() {
        return;
    }
    use crate::preview::mermaid::requirement;
    for (name, src) in kind_cases_of(requirement::is_requirement_diagram) {
        let model = requirement::parse(src).expect("parses");
        let d = kind_diagram(src);

        // What one box says: its stereotype, its name, then the fields the source declared. A
        // field left out is a row that is not there, which is what "declares nothing" looks like.
        let rows_of = |stereotype: &str, subject: &str, fields: &[(&str, &str)]| -> Vec<String> {
            let mut rows = vec![as_drawn(&format!("«{stereotype}»")), as_drawn(subject)];
            rows.extend(
                fields
                    .iter()
                    .filter(|(_, v)| !v.trim().is_empty())
                    .map(|(k, v)| as_drawn(&format!("{k}: {v}"))),
            );
            rows
        };
        // Every box the source declares, with the id it is drawn under.
        let mut declared: Vec<(&str, Vec<String>)> = model
            .requirements
            .iter()
            .map(|r| {
                (
                    r.name.as_str(),
                    rows_of(
                        r.kind.title(),
                        &r.name,
                        &[
                            ("Id", &r.id),
                            ("Text", &r.text),
                            ("Risk", &r.risk),
                            ("Verification", &r.verify_method),
                        ],
                    ),
                )
            })
            .collect();
        declared.extend(model.elements.iter().map(|e| {
            (
                e.name.as_str(),
                rows_of(
                    "Element",
                    &e.name,
                    &[("Type", &e.kind), ("Doc Ref", &e.doc_ref)],
                ),
            )
        }));

        // **By the id**, which is the box's own name — never by looking the rows up among the
        // rows that were drawn, and never only through a link, which an isolated box has none of.
        for (id, want) in &declared {
            let placed = d
                .node(id)
                .unwrap_or_else(|| panic!("{name}: the box {id:?} was not drawn"));
            assert_eq!(
                &panel_rows(placed),
                want,
                "{name}: the box {id:?} does not hold that datum's own rows in order"
            );
        }

        let mut wanted: Vec<Vec<String>> = declared.into_iter().map(|(_, rows)| rows).collect();
        let mut drawn: Vec<Vec<String>> = d.nodes.iter().map(panel_rows).collect();
        drawn.sort();
        wanted.sort();
        assert_eq!(
            drawn, wanted,
            "{name}: the boxes hold {drawn:?}, and the source declares {wanted:?}"
        );

        // A box's second row is its name, which is what a line's ends have to be read back as.
        let subject = |n: &PlacedNode| panel_rows(n).get(1).cloned().unwrap_or_default();
        let mut joined: Vec<(String, String, String)> = Vec::new();
        for e in &d.edges {
            let (Some(from), Some(to)) = (e.points.first(), e.points.last()) else {
                panic!("{name}: a line with no ends")
            };
            let a =
                node_at(&d, from).unwrap_or_else(|| panic!("{name}: a line starts on no one box"));
            let b = node_at(&d, to).unwrap_or_else(|| panic!("{name}: a line ends on no one box"));
            joined.push((
                subject(a),
                subject(b),
                e.label
                    .as_ref()
                    .map(|l| words(&l.label))
                    .unwrap_or_default(),
            ));
        }
        let known = |id: &str| {
            model.requirements.iter().any(|r| r.name == id)
                || model.elements.iter().any(|e| e.name == id)
        };
        let mut links: Vec<(String, String, String)> = model
            .links
            .iter()
            .filter(|l| known(&l.src) && known(&l.dst))
            .map(|l| {
                (
                    as_drawn(&l.src),
                    as_drawn(&l.dst),
                    as_drawn(l.relation.word()),
                )
            })
            .collect();
        joined.sort();
        links.sort();
        assert_eq!(
            joined, links,
            "{name}: the lines say {joined:?}, and the source's relationships are {links:?}"
        );
    }
}

/// **C4** — a box holds its own element's name, type and description, and a frame carries its own
/// boundary's title.
///
/// A boundary's place is **what it holds and how deeply it is nested**. Containment alone is not
/// enough and the corpus says why: a `Deployment_Node` inside a `Deployment_Node` holds exactly
/// the same element as its parent, so the two frames are told apart by depth — which is read off
/// the drawing, not off the `depth` the renderer wrote on them.
///
/// # What the bag and the lines together still cannot see
///
/// Both statements below are about *bags*, and what pins an entry of one to a datum is the line
/// that names it. So an element **no relationship names** is pinned by nothing: two of them, of
/// the same shape, could swap captions unseen. It is not the case today — the mutation that says
/// so is vacuous on this corpus, because every untied element is the only one of its shape — and
/// that is precisely the kind of guarantee §6-A item 10 says stops holding the moment a case is
/// added. The caption is therefore anchored on the id (`e.alias`) first, which needs nothing from
/// the corpus, and the bags are kept for what they alone catch: a caption or a frame the drawing
/// *added*.
#[test]
fn a_c4_boxs_caption_and_a_boundarys_title_belong_to_their_own_datum() {
    if !text_metrics::fonts_available() {
        return;
    }
    use crate::preview::mermaid::c4;
    for (name, src) in kind_cases_of(c4::is_c4) {
        let model = c4::parse(src).expect("parses");
        let d = kind_diagram(src);

        // The three lines inside a box: who it is, what it is, and what it does.
        let caption = |e: &c4::Element| -> Vec<String> {
            let mut lines = vec![e.label.clone()];
            if !e.kind_line.trim().is_empty() {
                let external = if e.external { ", external" } else { "" };
                lines.push(format!("[{}{external}]", e.kind_line));
            }
            if !e.descr.trim().is_empty() {
                lines.push(e.descr.clone());
            }
            Label::measure(&lines.join("\n")).lines
        };
        // **By the alias**, which is the id the element's own box is drawn under.
        for e in &model.elements {
            let placed = d
                .node(&e.alias)
                .unwrap_or_else(|| panic!("{name}: the element {:?} was not drawn", e.alias));
            assert_eq!(
                node_lines(placed),
                caption(e),
                "{name}: the box {:?} reads another element's caption",
                e.alias
            );
        }

        let mut wanted: Vec<Vec<String>> = model.elements.iter().map(caption).collect();
        let mut drawn: Vec<Vec<String>> = d.nodes.iter().map(node_lines).collect();
        drawn.sort();
        wanted.sort();
        assert_eq!(
            drawn, wanted,
            "{name}: the boxes read {drawn:?}, and the source's elements are {wanted:?}"
        );

        // …and each caption is on the box the source's *lines* reach, which is what turns the
        // multiset above into a placement. A rotation leaves that multiset untouched — it is the
        // whole reason a permutation defeats a size check — so the relationships are what say
        // which box is which: both ends of a line have been clipped to the outline they meet.
        let head = |n: &PlacedNode| node_lines(n).first().cloned().unwrap_or_default();
        let mut joined: Vec<(String, String, String)> = Vec::new();
        for e in &d.edges {
            let (Some(from), Some(to)) = (e.points.first(), e.points.last()) else {
                panic!("{name}: a line with no ends")
            };
            let a =
                node_at(&d, from).unwrap_or_else(|| panic!("{name}: a line starts on no one box"));
            let b = node_at(&d, to).unwrap_or_else(|| panic!("{name}: a line ends on no one box"));
            joined.push((
                head(a),
                head(b),
                e.label
                    .as_ref()
                    .map(|l| words(&l.label))
                    .unwrap_or_default(),
            ));
        }
        let named = |alias: &str| {
            model
                .elements
                .iter()
                .find(|e| e.alias == alias)
                .map(|e| as_drawn(&e.label))
                .unwrap_or_default()
        };
        let drawable = |id: &str| model.elements.iter().any(|e| e.alias == id);
        let mut rels: Vec<(String, String, String)> = model
            .rels
            .iter()
            .filter(|r| drawable(&r.from) && drawable(&r.to))
            .map(|r| {
                let text = if r.techn.trim().is_empty() {
                    r.label.clone()
                } else {
                    format!("{}\n[{}]", r.label, r.techn)
                };
                (named(&r.from), named(&r.to), as_drawn(&text))
            })
            .collect();
        joined.sort();
        rels.sort();
        assert_eq!(
            joined, rels,
            "{name}: the lines say {joined:?}, and the source's relationships are {rels:?}"
        );

        // What one boundary's frame says. Two boundaries holding the same elements at the same
        // depth are indistinguishable in the bag below, so the title is anchored on the alias too.
        let frame_title = |b: &c4::Boundary| -> Vec<String> {
            let title = if b.kind_line.trim().is_empty() {
                b.label.clone()
            } else {
                format!("{}\n[{}]", b.label, b.kind_line)
            };
            Label::measure(&title).lines
        };
        for b in &model.boundaries {
            // A boundary with nothing under it draws no frame at all.
            let Some(frame) = d.cluster(&b.alias) else {
                continue;
            };
            assert_eq!(
                frame.title.lines,
                frame_title(b),
                "{name}: the frame {:?} is titled with another boundary's words",
                b.alias
            );
        }

        // A frame, as the drawing gives it: what it holds, how deep it is, and what it says.
        let head = |n: &PlacedNode| node_lines(n).first().cloned().unwrap_or_default();
        let mut frames: Vec<(Vec<String>, usize, Vec<String>)> = d
            .clusters
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let mut held: Vec<String> = d
                    .nodes
                    .iter()
                    .filter(|n| contains(c.bounds(), n.bounds()))
                    .map(head)
                    .collect();
                held.sort();
                (held, frame_depth(&d, i), c.title.lines.clone())
            })
            .collect();

        // …and as the source gives it. A boundary's members are elements *and* boundaries, so the
        // elements under one are found by walking down through the boundaries it names.
        fn under(model: &c4::C4, alias: &str, out: &mut Vec<String>, guard: usize) {
            if guard == 0 {
                return;
            }
            for m in model
                .boundaries
                .iter()
                .filter(|b| b.alias == alias)
                .flat_map(|b| b.members.iter())
            {
                if let Some(e) = model.elements.iter().find(|e| e.alias == *m) {
                    out.push(as_drawn(&e.label));
                } else {
                    under(model, m, out, guard - 1);
                }
            }
        }
        let depth_of = |b: &c4::Boundary| -> usize {
            let mut at = b.parent.clone();
            let mut n = 0usize;
            for _ in 0..model.boundaries.len() + 1 {
                let Some(id) = at else { break };
                n += 1;
                at = model
                    .boundaries
                    .iter()
                    .find(|o| o.alias == id)
                    .and_then(|o| o.parent.clone());
            }
            n
        };
        let mut wanted: Vec<(Vec<String>, usize, Vec<String>)> = model
            .boundaries
            .iter()
            .map(|b| {
                let mut held = Vec::new();
                under(&model, &b.alias, &mut held, model.boundaries.len() + 1);
                held.sort();
                (held, depth_of(b), frame_title(b))
            })
            .collect();
        frames.sort();
        wanted.sort();
        assert_eq!(
            frames, wanted,
            "{name}: the frames hold and say {frames:?}, and the source's boundaries are {wanted:?}"
        );
    }
}

/// **block** — a cell holds its own words, in grid order, spans and nested blocks included.
///
/// The place is the **reading order of the grid**, which is what `columns` and `:n` exist to
/// decide: rows top to bottom, cells left to right, with a row found by which boxes overlap in y.
/// A nested `block:` is its own grid inside its own frame, so it is asked the same question about
/// its own cells and the outer grid is asked about the ones outside every frame.
#[test]
fn a_block_grids_cells_hold_their_own_words_in_grid_order() {
    if !text_metrics::fonts_available() {
        return;
    }
    use crate::preview::mermaid::block::{self, Item};
    for (name, src) in kind_cases_of(block::is_block_diagram) {
        let model = block::parse(src).expect("parses");
        let d = kind_diagram(src);

        // Which frame each box and each frame sits *directly* inside — the deepest one holding it.
        let deepest = |bounds: (f64, f64, f64, f64), not: Option<usize>| -> Option<usize> {
            d.clusters
                .iter()
                .enumerate()
                .filter(|(i, c)| Some(*i) != not && contains(c.bounds(), bounds))
                .max_by_key(|(i, _)| frame_depth(&d, *i))
                .map(|(i, _)| i)
        };
        let owner: Vec<Option<usize>> = d.nodes.iter().map(|n| deepest(n.bounds(), None)).collect();
        let holder: Vec<Option<usize>> = (0..d.clusters.len())
            .map(|i| deepest(d.clusters[i].bounds(), Some(i)))
            .collect();

        // One grid, stated: the cells directly in it, read in grid order, spell its own items.
        fn check(
            name: &str,
            where_: &str,
            items: &[Item],
            region: Option<usize>,
            d: &Diagram,
            owner: &[Option<usize>],
            holder: &[Option<usize>],
        ) {
            let cells: Vec<&PlacedNode> = d
                .nodes
                .iter()
                .enumerate()
                .filter(|(i, _)| owner[*i] == region)
                .map(|(_, n)| n)
                .collect();
            let drawn: Vec<String> = grid_order(&cells).iter().map(|n| words(&n.label)).collect();
            let wanted: Vec<String> = items
                .iter()
                .filter_map(|i| match i {
                    Item::Node(n) => Some(as_drawn(&n.label)),
                    _ => None,
                })
                .collect();
            assert_eq!(
                drawn, wanted,
                "{name}: {where_} reads {drawn:?} in grid order, and its own blocks are {wanted:?}"
            );

            // …and every block nested in it is its own grid, in the same order.
            let frames: Vec<usize> = {
                let mut v: Vec<usize> = (0..d.clusters.len())
                    .filter(|i| holder[*i] == region)
                    .collect();
                v.sort_by(|a, b| {
                    cmp(d.clusters[*a].bounds().1, d.clusters[*b].bounds().1)
                        .then(cmp(d.clusters[*a].center.x, d.clusters[*b].center.x))
                });
                v
            };
            let nested: Vec<&block::Composite> = items
                .iter()
                .filter_map(|i| match i {
                    Item::Composite(c) => Some(c),
                    _ => None,
                })
                .collect();
            assert_eq!(
                frames.len(),
                nested.len(),
                "{name}: {where_} drew {} frames for {} nested blocks",
                frames.len(),
                nested.len()
            );
            for (frame, c) in frames.iter().zip(nested.iter()) {
                assert_eq!(
                    words(&d.clusters[*frame].title),
                    as_drawn(if c.named { c.id.as_str() } else { "" }),
                    "{name}: a nested block's frame is titled with other words"
                );
                check(
                    name,
                    &format!("the block {:?}", c.id),
                    &c.children,
                    Some(*frame),
                    d,
                    owner,
                    holder,
                );
            }
        }
        check(name, "the grid", &model.items, None, &d, &owner, &holder);
    }
}

/// **architecture** — a box holds its own service's name, and a frame carries its own group's
/// title.
///
/// The place for a service is the **line**: an edge names a side at each end, both ends are on
/// the outline they meet, so the pair of boxes a line joins is read off the drawing and a name
/// that moved to its neighbour makes the picture disagree with the source. A group's place is
/// what its frame holds and how deeply it is nested, as it is for a C4 boundary.
///
/// Both of those reach a service only through something else that names it: a line, or a frame it
/// changes the contents of. A service **no edge names**, in the same group as another such, is
/// reached by neither — the bag of drawn words is unchanged by any permutation, and a swap inside
/// one group changes no frame's contents. As with C4 the mutation proving it is vacuous on this
/// corpus (no two unwired services share a group) and that is exactly the guarantee §6-A item 10
/// says a corpus can withdraw without anyone noticing, so the words are anchored on the service's
/// own id first.
#[test]
fn an_architectures_boxes_and_group_titles_belong_to_their_own_services_and_groups() {
    if !text_metrics::fonts_available() {
        return;
    }
    use crate::preview::mermaid::architecture;
    for (name, src) in kind_cases_of(architecture::is_architecture) {
        let model = architecture::parse(src).expect("parses");
        let d = kind_diagram(src);

        // A junction is a dot and says nothing; a service with no title is named by its own id.
        let says = |s: &architecture::Service| -> String {
            if s.junction {
                String::new()
            } else if s.title.trim().is_empty() {
                as_drawn(&s.id)
            } else {
                as_drawn(&s.title)
            }
        };
        // **By the service's own id**, which is the id its box is drawn under.
        for s in &model.services {
            let placed = d
                .node(&s.id)
                .unwrap_or_else(|| panic!("{name}: the service {:?} was not drawn", s.id));
            assert_eq!(
                words(&placed.label),
                says(s),
                "{name}: the box for {:?} reads another service's words",
                s.id
            );
        }

        let mut drawn: Vec<String> = d.nodes.iter().map(|n| words(&n.label)).collect();
        let mut wanted: Vec<String> = model.services.iter().map(says).collect();
        drawn.sort();
        wanted.sort();
        assert_eq!(
            drawn, wanted,
            "{name}: the boxes read {drawn:?}, and the source's services are {wanted:?}"
        );

        let mut joined: Vec<(String, String)> = Vec::new();
        for e in &d.edges {
            let (Some(from), Some(to)) = (e.points.first(), e.points.last()) else {
                panic!("{name}: a line with no ends")
            };
            let a =
                node_at(&d, from).unwrap_or_else(|| panic!("{name}: a line starts on no one box"));
            let b = node_at(&d, to).unwrap_or_else(|| panic!("{name}: a line ends on no one box"));
            joined.push((words(&a.label), words(&b.label)));
        }
        let known = |id: &str| model.services.iter().any(|s| s.id == id);
        let service = |id: &str| {
            model
                .services
                .iter()
                .find(|s| s.id == id)
                .map(says)
                .unwrap_or_default()
        };
        let mut wired: Vec<(String, String)> = model
            .edges
            .iter()
            .filter(|e| known(&e.from) && known(&e.to))
            .map(|e| (service(&e.from), service(&e.to)))
            .collect();
        joined.sort();
        wired.sort();
        assert_eq!(
            joined, wired,
            "{name}: the lines join {joined:?}, and the source wires {wired:?}"
        );

        // The groups. Two groups holding the same services at the same depth cannot be told apart
        // in the bag below, so the title is anchored on the group's own id first.
        for g in &model.groups {
            // A group nothing is in draws no frame at all.
            let Some(frame) = d.cluster(&g.id) else {
                continue;
            };
            assert_eq!(
                words(&frame.title),
                as_drawn(&g.title),
                "{name}: the frame for {:?} is titled with another group's words",
                g.id
            );
        }

        // The groups: what each frame holds, how deep it is, and what it says.
        let mut frames: Vec<(Vec<String>, usize, String)> = d
            .clusters
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let mut held: Vec<String> = d
                    .nodes
                    .iter()
                    .filter(|n| contains(c.bounds(), n.bounds()))
                    .map(|n| words(&n.label))
                    .collect();
                held.sort();
                (held, frame_depth(&d, i), words(&c.title))
            })
            .collect();
        let inside = |group: Option<&str>, want: &str| -> bool {
            let mut at = group;
            for _ in 0..model.groups.len() + 1 {
                let Some(id) = at else { return false };
                if id == want {
                    return true;
                }
                at = model
                    .groups
                    .iter()
                    .find(|g| g.id == id)
                    .and_then(|g| g.parent.as_deref());
            }
            false
        };
        let mut wanted: Vec<(Vec<String>, usize, String)> = model
            .groups
            .iter()
            .filter_map(|g| {
                let mut held: Vec<String> = model
                    .services
                    .iter()
                    .filter(|s| inside(s.group.as_deref(), &g.id))
                    .map(says)
                    .collect();
                // A group nothing is in draws no frame at all.
                if held.is_empty() {
                    return None;
                }
                held.sort();
                Some((held, inside_depth(&model, &g.id), as_drawn(&g.title)))
            })
            .collect();
        frames.sort();
        wanted.sort();
        assert_eq!(
            frames, wanted,
            "{name}: the frames hold and say {frames:?}, and the source's groups are {wanted:?}"
        );
    }
}

/// How many groups a group is written inside.
fn inside_depth(model: &crate::preview::mermaid::architecture::Architecture, id: &str) -> usize {
    let mut at = model
        .groups
        .iter()
        .find(|g| g.id == id)
        .and_then(|g| g.parent.as_deref());
    let mut n = 0usize;
    for _ in 0..model.groups.len() + 1 {
        let Some(parent) = at else { break };
        n += 1;
        at = model
            .groups
            .iter()
            .find(|g| g.id == parent)
            .and_then(|g| g.parent.as_deref());
    }
    n
}

/// **zenuml** — each name, message and note is drawn on its own datum, and the words on the
/// arrows came out of the source in the order it wrote them.
///
/// ZenUML is read into the **sequence model** and drawn by stage 4's renderer; `render::zenuml`
/// is those two calls and nothing else. So the first half of this is deliberately not a new set
/// of statements — it is [`check_sequence_text_placement`], the same function
/// [`a_sequence_diagrams_messages_participants_and_notes_each_carry_their_own_text`] calls, over
/// sources `SEQUENCE_CASES` does not contain: a nested sync call, an `@Actor`, an `if`/`else` and
/// a `while`. Writing those statements again would be the copy this project keeps being bitten
/// by.
///
/// **Be exact about what that half can see.** `zenuml::parse` produces *both* the model the
/// checker compares against and the diagram it compares, so a translation that shuffled the
/// messages between arrows would move both sides together and the checker would stay green —
/// the self-comparison §6-A records konoma being caught by twice. That is why the second half is
/// here, and why it reads the **source bytes**, which the translation cannot move: a ZenUML
/// message's words are written literally on its own line, so "each arrow's words occur in the
/// source after the arrow before it" holds for every diagram this language can express and fails
/// the moment two messages change places.
#[test]
fn a_zenuml_diagrams_participants_messages_and_notes_carry_their_own_translated_text() {
    if !text_metrics::fonts_available() {
        return;
    }
    use crate::preview::mermaid::zenuml;
    for (name, src) in kind_cases_of(zenuml::is_zenuml) {
        let model = zenuml::parse(src).expect("parses");
        let d = super::zenuml::lay_out(src).expect("lays out");
        check_sequence_text_placement(name, &model, &d);

        // **Lines, not bytes.** One ZenUML statement can put two arrows' words on one line and
        // put them in the other order while it is at it: `total = Order.total()` is the call
        // `total()` *and* the reply labelled `total`, and the reply's word is written to the
        // left of the call's. So what holds for every diagram this language can express is that
        // the arrows read down the source — never that they read along it.
        let lines: Vec<&str> = src.lines().collect();
        let mut cursor = 0usize;
        let mut read = 0usize;
        for e in &d.edges {
            let Some(label) = e.label.as_ref() else {
                continue;
            };
            let text = words(&label.label);
            // Words the reader had to change on the way in — a `<br>`, a doubled space — cannot
            // be looked back up in the source, so they are left to the checker above rather than
            // reported as being out of order.
            if text.trim().is_empty() || !src.contains(text.as_str()) {
                continue;
            }
            let at = lines
                .iter()
                .enumerate()
                .skip(cursor)
                .find(|(_, line)| line.contains(text.as_str()))
                .map(|(i, _)| i)
                .unwrap_or_else(|| {
                    panic!(
                        "{name}: the arrow reading {text:?} is drawn after one whose words the \
                         source writes on a later line — the messages have changed places"
                    )
                });
            cursor = at;
            read += 1;
        }
        assert!(
            read > 0,
            "{name}: not one arrow's words could be found in the source, so nothing was read \
             back and this half of the test asserted nothing"
        );
    }
}
