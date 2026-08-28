//! **Every drawn distinction, held by an ordinary test instead of by a golden.**
//!
//! `docs/STATUS.md` records the same failure twice, and this file exists because it happened a
//! third time. Stage 4 mutated [`svg`] so that a sequence frame's `else` / `and` divider rules were
//! never drawn; the only tests that reddened were `sequence_emit_golden` and
//! `sequence_corpus_golden` — **not one named test**. Stage 4's own Part A had already found the
//! identical shape in the flowchart arrows. The general statement is:
//!
//! > A visible distinction guarded only by a golden is lost silently the next time the golden is
//! > regenerated, because regenerating is how a golden is *meant* to be updated.
//!
//! So this file does not chase the one case. It walks the three enumerations of drawn things —
//! [`Glyph`], [`Tip`], and [`Decoration`], the last being konoma's own list of the marks that are
//! not a node's outline or a line's end — and states, for each, **what the emitted document
//! contains that no sibling's does**.
//!
//! # How a distinction is stated, and why not by naming the markup
//!
//! Naming the markup ("an arrow head is a `<polygon>`") would be a second copy of [`svg`]'s
//! decisions, and would have to be edited whenever the drawing changed — which is the same trap
//! as a golden, only more verbose. Instead each case is emitted **in isolation, at identical
//! coordinates**, and the resulting markup is used as a *fingerprint*:
//!
//! * every variant must draw **something** (a non-empty fingerprint), except the ones this file
//!   declares as drawing nothing on purpose;
//! * two variants must have the same fingerprint **exactly when this file says they are drawn the
//!   same**, in an equivalence table that gives a reason for each pairing.
//!
//! A mutation that stops drawing a mark empties its fingerprint. A mutation that draws two marks
//! the same merges two classes. Both fail here, by name, without any golden being consulted.
//!
//! # Adding a variant cannot be forgotten
//!
//! Each table is paired with a `covered()` whose `match` is exhaustive and has no `_` arm, the
//! trick from [`super::tests::a_flowchart_arrow_marks_only_the_ends_that_spelled_a_mark`]: a new
//! variant fails to **compile**, and a variant left out of the table fails the assertion.
//!
//! # No font is involved
//!
//! Every [`Label`] here is built with a declared width rather than measured, so this file says the
//! same thing on a machine with no fonts as on one with all of them — and so a change in font
//! metrics can never be mistaken for a change in what is drawn.

use std::collections::{BTreeMap, HashSet};

use super::edges::Tip;
use super::shapes::Glyph;
use super::{
    labels, panel, shapes, svg, theme, ClusterSection, Curve, Diagram, Label, PlacedActivation,
    PlacedCluster, PlacedEdge, PlacedEdgeLabel, PlacedLifeline, PlacedNode, Size,
};
use crate::preview::mermaid::flowchart::{Shape, Stroke};
use crate::preview::mermaid::layout::Point;

/// A label with a declared width, so nothing here depends on a font being installed.
fn label(text: &str, w: f64) -> Label {
    Label {
        lines: text.split('\n').map(str::to_string).collect(),
        width: w,
        height: text.split('\n').count() as f64 * labels::line_height(),
    }
}

fn blank() -> Label {
    label("", 0.0)
}

/// **The marks a diagram puts on the page** — every emitted element that actually paints
/// something, and nothing else.
///
/// Four kinds of line are dropped, and the third one is the reason this function has a comment:
///
/// * the `<svg>` open and close, which every case shares;
/// * the transparent backing rect, likewise;
/// * **the `<g>` group wrappers.** A group paints nothing, but [`svg::emit`] opens
///   `<g class="cluster-labels">` whenever a frame has *either* a title *or* any sections — so a
///   diagram with a section but no rule drawn still differs from one with no section at all, by
///   two lines of markup that draw nothing. Keeping them made
///   [`every_decoration_leaves_a_mark_of_its_own_on_the_page`] pass under the very mutation it was
///   written to catch: the "mark" it found was the empty wrapper. Measured, not reasoned about —
///   the mutation was re-injected and the test stayed green until this filter went in.
/// * blank lines.
fn drawn(d: &Diagram) -> String {
    svg::emit(d, &theme::DARK)
        .lines()
        .filter(|l| {
            !l.is_empty()
                && !l.starts_with("<svg")
                && !l.starts_with("</svg")
                && !l.starts_with("<rect w")
                && !l.starts_with("<g ")
                && !l.starts_with("</g")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// An empty canvas of a fixed size, so every case is drawn in the same coordinate space and any
/// difference between two fingerprints is a difference in the *mark*, never in where it landed.
fn canvas() -> Diagram {
    Diagram {
        width: 400.0,
        height: 300.0,
        ..Diagram::default()
    }
}

// ---------------------------------------------------------------------------------------------
// 1. Glyphs
// ---------------------------------------------------------------------------------------------

/// A three-compartment class panel, at declared widths.
fn a_class_panel() -> panel::Panel {
    panel::class_panel(&[
        panel::Compartment {
            lines: vec![label("Shape", 40.0)],
            centered: true,
        },
        panel::Compartment {
            lines: vec![label("+int sides", 66.0)],
            centered: false,
        },
    ])
}

/// A one-attribute entity panel, at declared widths.
fn an_er_panel() -> panel::Panel {
    panel::er_panel(
        &label("PERSON", 55.0),
        &[panel::AttributeRow {
            kind: label("string", 40.0),
            name: label("name", 34.0),
            keys: blank(),
            comment: blank(),
        }],
    )
}

/// The band-over-name panel a stick figure needs — the same shape
/// `super::sequence::actor_panel` builds, written out here so this file needs no layout run.
fn an_actor_panel() -> panel::Panel {
    let name = label("Alice", 36.0);
    let w = (name.width + shapes::PARTICIPANT_PAD_X * 2.0).max(shapes::PARTICIPANT_MIN_WIDTH);
    let text_band = name.height + shapes::PARTICIPANT_PAD_Y;
    panel::Panel {
        rows: vec![panel::PanelRow {
            cells: vec![panel::PanelCell {
                label: name,
                x: w / 2.0,
                centered: true,
            }],
            y: shapes::ACTOR_FIGURE_HEIGHT + text_band / 2.0,
        }],
        rules: Vec::new(),
        columns: Vec::new(),
        size: Size::new(w, shapes::ACTOR_FIGURE_HEIGHT + text_band),
    }
}

/// One node drawn with `glyph`, centred on the canvas.
///
/// The *content* is chosen per glyph rather than shared, because three of them are boxes whose
/// outline is deliberately the same rounded rectangle and whose difference is entirely what goes
/// inside: a `TitledBox` rules off its first line, and the two panel-bearing glyphs draw a table.
/// Handing them all one centred word would make this file assert that konoma draws them alike,
/// which is the opposite of what a reader sees.
fn glyph_document(glyph: Glyph) -> String {
    let (size, panel, lab) = match glyph {
        // Sized by the chart rather than by a label (`shapes::size` passes the number through),
        // so the size is chosen here — one box for all seven, so that any difference between two
        // fingerprints is a difference in the *mark*.
        Glyph::Wedge | Glyph::Graticule => (Size::new(120.0, 120.0), None, blank()),
        Glyph::Ribbon | Glyph::ChartBar | Glyph::PlotFrame => {
            (Size::new(120.0, 60.0), None, blank())
        }
        Glyph::ChartPoint => (Size::new(7.0, 7.0), None, blank()),
        Glyph::ChartLabel => {
            let l = label("Node", 40.0);
            (Size::new(l.width, l.height), None, l)
        }
        // **The same panel for both**, deliberately: what tells a class box from an entity box is
        // the table `panel` builds, and this test is about the *glyph*. Handing them different
        // tables would let a difference in `panel` stand in for a difference in `shapes`, and the
        // test would go on passing if the two glyphs were merged. `Decoration::PanelColumn` is
        // where an entity's own grid is checked.
        Glyph::ClassBox | Glyph::ErBox => {
            let p = a_class_panel();
            (p.size, Some(p), blank())
        }
        Glyph::Actor => {
            let p = an_actor_panel();
            (shapes::size(Glyph::Actor, p.size), Some(p), blank())
        }
        // Two lines, because the rule under the first line is the whole point of this glyph.
        Glyph::TitledBox => {
            let l = label("Name\ndescription", 90.0);
            (shapes::size(glyph, Size::new(l.width, l.height)), None, l)
        }
        _ => {
            let l = label("Node", 40.0);
            (shapes::size(glyph, Size::new(l.width, l.height)), None, l)
        }
    };
    let mut d = canvas();
    d.nodes.push(PlacedNode {
        id: "n".to_string(),
        shape: glyph,
        center: Point::new(200.0, 150.0),
        size,
        label: lab,
        panel,
        // A chart mark carries a series so that its fill is the palette's rather than a node's;
        // without one, a bar and a plain rectangle would be the same markup and the table below
        // would be asserting something false.
        series: chart_series(glyph),
        mark: chart_mark(glyph),
        style: None,
    });
    drawn(&d)
}

/// The series a chart glyph is painted in, for [`glyph_document`]. `None` for everything else, so
/// the four graph languages are emitted exactly as they always were.
fn chart_series(glyph: Glyph) -> Option<usize> {
    match glyph {
        Glyph::Wedge | Glyph::ChartBar | Glyph::ChartPoint | Glyph::Ribbon | Glyph::ChartLabel => {
            Some(0)
        }
        _ => None,
    }
}

/// The continuous geometry three of the chart glyphs cannot be drawn without.
///
/// Supplying it here is the point: `shapes::outline` returns `Outline::None` for a wedge with no
/// angles, so a table row that forgot the mark would report "draws nothing" rather than a wrong
/// drawing — and this file's first assertion would catch it either way.
fn chart_mark(glyph: Glyph) -> Option<shapes::Mark> {
    match glyph {
        Glyph::Wedge => Some(shapes::Mark::Wedge {
            start: 30.0,
            sweep: 100.0,
        }),
        Glyph::Ribbon => Some(shapes::Mark::Ribbon {
            left_top: -20.0,
            left_bottom: 4.0,
            right_top: -6.0,
            right_bottom: 20.0,
        }),
        Glyph::Graticule => Some(shapes::Mark::Graticule {
            rings: 3,
            spokes: 5,
            polygon: false,
        }),
        Glyph::Face => Some(shapes::Mark::Face { score: 4.0 }),
        Glyph::BlockArrow => Some(shapes::Mark::BlockArrow {
            left: false,
            right: true,
            up: false,
            down: false,
        }),
        _ => None,
    }
}

/// Every [`Glyph`] the renderer can be asked for, with the two things this test states about each.
///
/// The tuple is `(name for a failure message, the glyph, what the finished node looks like, what
/// its bare outline is)`. Two rows sharing a name in either column must come out identical in that
/// column, and two rows with different names must not — so a mutation that merges two glyphs is
/// caught whether it happens in [`shapes::outline`] (the fourth column) or in [`svg`]'s drawing of
/// it (the third).
///
/// The two columns say different things and neither implies the other. A class box and a plain
/// square rectangle **have the same outline** — that is what `shapes::outline` returns for both —
/// and are told apart on the page only by the table inside one of them. A fork bar's two
/// orientations are the reverse case: one outline, because the orientation lives in the node's
/// *size* rather than in its shape, but two different nodes on the page.
#[allow(clippy::type_complexity)]
fn glyph_table() -> Vec<(&'static str, Glyph, &'static str, &'static str)> {
    vec![
        (
            "rect",
            Glyph::Flow(Shape::Rect),
            "square rectangle",
            "square rectangle",
        ),
        (
            "rounded",
            Glyph::Flow(Shape::RoundedRect),
            "rounded rectangle",
            "rounded rectangle",
        ),
        ("stadium", Glyph::Flow(Shape::Stadium), "stadium", "stadium"),
        ("circle", Glyph::Flow(Shape::Circle), "circle", "circle"),
        (
            "double-circle",
            Glyph::Flow(Shape::DoubleCircle),
            "two rings",
            "two rings",
        ),
        ("diamond", Glyph::Flow(Shape::Diamond), "diamond", "diamond"),
        ("hexagon", Glyph::Flow(Shape::Hexagon), "hexagon", "hexagon"),
        (
            "subroutine",
            Glyph::Flow(Shape::Subroutine),
            "framed rectangle",
            "framed rectangle",
        ),
        (
            "cylinder",
            Glyph::Flow(Shape::Cylinder),
            "cylinder",
            "cylinder",
        ),
        (
            "trapezoid",
            Glyph::Flow(Shape::Trapezoid),
            "trapezoid",
            "trapezoid",
        ),
        (
            "inv-trapezoid",
            Glyph::Flow(Shape::InvTrapezoid),
            "inverted trapezoid",
            "inverted trapezoid",
        ),
        (
            "lean-right",
            Glyph::Flow(Shape::LeanRight),
            "lean right",
            "lean right",
        ),
        (
            "lean-left",
            Glyph::Flow(Shape::LeanLeft),
            "lean left",
            "lean left",
        ),
        (
            "odd",
            Glyph::Flow(Shape::Odd),
            "notched rectangle",
            "notched rectangle",
        ),
        // `A>text]` in mermaid draws no outline at all; the words *are* the node. That is a
        // deliberate appearance, not a missing one, so it is named rather than exempted.
        ("text", Glyph::Flow(Shape::Text), "bare words", "nothing"),
        (
            "state-start",
            Glyph::StateStart,
            "filled dot",
            "disc of half the width",
        ),
        (
            "state-end",
            Glyph::StateEnd,
            "ring around a dot",
            "ring around a dot",
        ),
        // A `<<choice>>` is cut from the same polygon as a decision diamond and holds no text,
        // which is exactly how a reader tells them apart on the page.
        ("choice", Glyph::Choice, "empty diamond", "diamond"),
        (
            "fork-h",
            Glyph::Bar { horizontal: true },
            "wide bar",
            "solid bar",
        ),
        // One outline for both orientations: which way a fork bar lies is decided by the *size*
        // handed to it, not by its shape. Two different nodes on the page, one `Outline`.
        (
            "fork-v",
            Glyph::Bar { horizontal: false },
            "tall bar",
            "solid bar",
        ),
        ("note", Glyph::Note, "folded corner", "folded corner"),
        (
            "titled-box",
            Glyph::TitledBox,
            "box with a rule",
            "rounded rectangle",
        ),
        (
            "class-box",
            Glyph::ClassBox,
            "box with a table",
            "square rectangle",
        ),
        // Same outline as a class box and as a plain square rectangle — that is literally what
        // `shapes::outline` returns for all three. What a reader tells them apart by is the table
        // inside, which `panel` builds and `shapes` knows nothing about.
        (
            "er-box",
            Glyph::ErBox,
            "box with a table",
            "square rectangle",
        ),
        // A participant is a rounded rectangle like any other; what makes it a participant is the
        // minimum width and the lifeline hanging off it, neither of which is an outline.
        (
            "participant",
            Glyph::Participant,
            "wide rounded box",
            "rounded rectangle",
        ),
        ("actor", Glyph::Actor, "stick figure", "stick figure"),
        // --- stage 5 ---------------------------------------------------------------------------
        ("wedge", Glyph::Wedge, "pie slice", "pie slice"),
        (
            "chart-bar",
            Glyph::ChartBar,
            "filled rectangle",
            "square rectangle",
        ),
        // The same *kind* of outline as a state marker and not the same outline: a marker's
        // radius is half its box's width, a datum's is half its box's shorter side, so a
        // non-square box tells them apart. Which is the point of probing at one.
        (
            "chart-point",
            Glyph::ChartPoint,
            "small disc",
            "disc inscribed in the box",
        ),
        ("ribbon", Glyph::Ribbon, "flow band", "flow band"),
        // No outline of its own, exactly like `A>text]` — the words are the mark. The *drawing*
        // is still distinct, because the colour a chart label takes is the one that reads on a
        // series fill rather than the one that reads on the terminal.
        (
            "chart-label",
            Glyph::ChartLabel,
            "words on a fill",
            "nothing",
        ),
        // A plot's frame is the same rectangle a class box is, and is told apart by having
        // nothing inside it and by being drawn in the axis colour rather than the node colour —
        // neither of which is an `Outline`.
        (
            "plot-frame",
            Glyph::PlotFrame,
            "empty frame",
            "square rectangle",
        ),
        (
            "graticule",
            Glyph::Graticule,
            "rings and spokes",
            "rings and spokes",
        ),
        // --- stage 5b --------------------------------------------------------------------------
        ("cloud", Glyph::Cloud, "bumpy blob", "bumpy blob"),
        ("bang", Glyph::Bang, "spiky burst", "spiky burst"),
        // A rule and nothing else. Distinct from `text`, which draws no mark at all: the rule is
        // what makes an unadorned mindmap node read as a node rather than as a stray word.
        ("underline", Glyph::Underline, "words on a rule", "a rule"),
        ("face", Glyph::Face, "mood face", "mood face"),
        (
            "block-arrow",
            Glyph::BlockArrow,
            "arrow block",
            "arrow block",
        ),
        ("reverted", Glyph::Reverted, "crossed dot", "crossed dot"),
        // A tag has to be told from a *caption* — the bare words under a commit — because a git
        // graph draws both, and telling them apart is the whole reason this glyph exists. The
        // ticket's outline is what does it: `chart-label` above draws no outline at all.
        ("tag", Glyph::Tag, "ticket with a hole", "pointed ticket"),
    ]
}

/// **Every glyph draws, and two glyphs look alike exactly when this file says they do.**
#[test]
fn every_glyph_draws_a_mark_that_tells_it_from_its_siblings() {
    // Exhaustive: a new `Glyph` variant makes this fail to compile.
    fn covered(g: Glyph) -> bool {
        match g {
            Glyph::Flow(shape) => match shape {
                Shape::Rect
                | Shape::RoundedRect
                | Shape::Stadium
                | Shape::Circle
                | Shape::DoubleCircle
                | Shape::Diamond
                | Shape::Hexagon
                | Shape::Subroutine
                | Shape::Cylinder
                | Shape::Trapezoid
                | Shape::InvTrapezoid
                | Shape::LeanRight
                | Shape::LeanLeft
                | Shape::Odd
                | Shape::Text => true,
            },
            Glyph::StateStart
            | Glyph::StateEnd
            | Glyph::Choice
            | Glyph::Bar { .. }
            | Glyph::Note
            | Glyph::TitledBox
            | Glyph::ClassBox
            | Glyph::ErBox
            | Glyph::Participant
            | Glyph::Actor
            | Glyph::Wedge
            | Glyph::ChartBar
            | Glyph::ChartPoint
            | Glyph::Ribbon
            | Glyph::ChartLabel
            | Glyph::PlotFrame
            | Glyph::Graticule
            | Glyph::Cloud
            | Glyph::Bang
            | Glyph::Underline
            | Glyph::Face
            | Glyph::BlockArrow
            | Glyph::Reverted
            | Glyph::Tag => true,
        }
    }

    let table = glyph_table();
    let listed: HashSet<Glyph> = table.iter().map(|(_, g, _, _)| *g).collect();
    assert_eq!(listed.len(), table.len(), "a glyph is listed twice");
    for g in &listed {
        assert!(covered(*g));
    }
    // `Bar` carries a field, so the exhaustive match above cannot prove *both* orientations are
    // present. They are drawn differently and are stated separately.
    assert!(listed.contains(&Glyph::Bar { horizontal: true }));
    assert!(listed.contains(&Glyph::Bar { horizontal: false }));

    // --- the outline, at one size for every glyph ------------------------------------------------
    //
    // The size is shared on purpose: `shapes::size` already varies per glyph, and letting it vary
    // here would let a difference in *size* stand in for a difference in *shape*, so two glyphs
    // merged in `shapes::outline` would still look distinct.
    let probe = Size::new(120.0, 48.0);
    partition(
        "outline",
        table.iter().map(|(name, g, _, outline)| {
            (
                *name,
                *outline,
                format!("{:?}", shapes::outline(*g, probe, chart_mark(*g))),
            )
        }),
    );

    // --- the finished node on the page ----------------------------------------------------------
    partition(
        "drawing",
        table.iter().map(|(name, g, appearance, _)| {
            let doc = glyph_document(*g);
            assert!(
                !doc.trim().is_empty(),
                "{name}: drew nothing at all — a glyph that emits no markup is invisible"
            );
            (*name, *appearance, doc)
        }),
    );
}

/// Asserts that `cases` fall into exactly the classes they declare.
///
/// Each case is `(name, declared class, fingerprint)`. Two cases with the same declared class must
/// have the same fingerprint, and two with different declared classes must not — so the assertion
/// holds in both directions and neither "everything merged" nor "everything split" can pass.
fn partition<'a>(what: &str, cases: impl Iterator<Item = (&'a str, &'a str, String)>) {
    let mut by_print: BTreeMap<String, Vec<(&str, &str)>> = BTreeMap::new();
    let mut declared: HashSet<&str> = HashSet::new();
    let mut n = 0usize;
    for (name, class, print) in cases {
        declared.insert(class);
        n += 1;
        by_print.entry(print).or_default().push((name, class));
    }
    for (print, members) in &by_print {
        let classes: HashSet<&str> = members.iter().map(|(_, c)| *c).collect();
        assert_eq!(
            classes.len(),
            1,
            "{what}: these are identical but are declared different: {members:?}\n\
             --- what they share ---\n{print}"
        );
    }
    assert_eq!(
        by_print.len(),
        declared.len(),
        "{what}: {} distinct results across {n} glyphs, but {} classes are declared — one class \
         is coming out two ways, or two classes have merged",
        by_print.len(),
        declared.len()
    );
}

/// **A glyph whose geometry lives in a [`shapes::Mark`] draws nothing without one.**
///
/// The alternative would be a default, and every default here is a lie a reader cannot see through:
/// a wedge with no angles is not a full circle, a ribbon with no corners is not a straight band, a
/// graticule with no ring count is not one ring. Refusing to draw turns a bug in the layout into a
/// missing mark — which is visible — instead of a plausible wrong one.
#[test]
fn a_chart_glyph_with_no_mark_draws_nothing_rather_than_a_default() {
    for glyph in [Glyph::Wedge, Glyph::Ribbon, Glyph::Graticule] {
        assert!(
            matches!(
                shapes::outline(glyph, Size::new(100.0, 100.0), None),
                super::shapes::Outline::None
            ),
            "{glyph:?} invented geometry it was not given"
        );
        // …and the mark it *is* given produces something.
        assert!(
            !matches!(
                shapes::outline(glyph, Size::new(100.0, 100.0), chart_mark(glyph)),
                super::shapes::Outline::None
            ),
            "{glyph:?} draws nothing even with its mark"
        );
    }
}

// ---------------------------------------------------------------------------------------------
// 2. Line ends
// ---------------------------------------------------------------------------------------------

/// One horizontal line carrying `tip` at its head end and nothing at its tail.
fn tip_document(tip: Tip) -> String {
    let mut d = canvas();
    d.edges.push(PlacedEdge {
        from: "a".to_string(),
        to: "b".to_string(),
        points: vec![Point::new(40.0, 150.0), Point::new(360.0, 150.0)],
        tip_start: Tip::None,
        tip_end: tip,
        stroke: Stroke::Normal,
        label: None,
        start_label: None,
        end_label: None,
        badge: None,
        series: None,
        straight: false,
        overlay: false,
        style: None,
        curve: Curve::Basis,
    });
    drawn(&d)
}

/// **Every terminator draws a mark of its own.**
///
/// Unlike the glyphs, no two of these may look alike: a terminator *is* the meaning of the end it
/// sits on (`||` says "exactly one", `}o` says "zero or more"), so two that draw the same are a
/// diagram that says something the source did not.
#[test]
fn every_line_end_draws_a_mark_that_tells_it_from_its_siblings() {
    fn covered(t: Tip) -> bool {
        match t {
            Tip::None
            | Tip::Arrow
            | Tip::Cross
            | Tip::Circle
            | Tip::HollowTriangle
            | Tip::FilledDiamond
            | Tip::HollowDiamond
            | Tip::Lollipop
            | Tip::ErOnlyOne
            | Tip::ErZeroOrOne
            | Tip::ErOneOrMore
            | Tip::ErZeroOrMore
            | Tip::Async => true,
        }
    }
    let table = [
        Tip::None,
        Tip::Arrow,
        Tip::Cross,
        Tip::Circle,
        Tip::HollowTriangle,
        Tip::FilledDiamond,
        Tip::HollowDiamond,
        Tip::Lollipop,
        Tip::ErOnlyOne,
        Tip::ErZeroOrOne,
        Tip::ErOneOrMore,
        Tip::ErZeroOrMore,
        Tip::Async,
    ];
    let listed: HashSet<Tip> = table.iter().copied().collect();
    assert_eq!(listed.len(), table.len(), "a terminator is listed twice");
    for t in &listed {
        assert!(covered(*t));
    }

    // The bare line, which every case also contains. What is left after removing it is the mark.
    let bare = tip_document(Tip::None);
    let mut by_mark: BTreeMap<String, Vec<Tip>> = BTreeMap::new();
    for tip in table {
        let doc = tip_document(tip);
        let mark: String = doc
            .lines()
            .filter(|l| !bare.lines().any(|b| b == *l))
            .collect::<Vec<_>>()
            .join("\n");
        if tip == Tip::None {
            assert!(
                mark.is_empty(),
                "`---` is spelled with no mark on the end, so it must draw none: {mark}"
            );
        } else {
            assert!(
                !mark.is_empty(),
                "{tip:?}: drew no mark of its own — the end is indistinguishable from `---`"
            );
        }
        by_mark.entry(mark).or_default().push(tip);
    }
    for (mark, tips) in &by_mark {
        assert_eq!(
            tips.len(),
            1,
            "these terminators draw the same mark, so a reader cannot tell them apart: \
             {tips:?}\n--- the mark ---\n{mark}"
        );
    }
}

// ---------------------------------------------------------------------------------------------
// 3. Decorations — the drawn things that are neither an outline nor a line end
// ---------------------------------------------------------------------------------------------

/// A drawn thing that lives on a [`Diagram`] field of its own rather than on a `Glyph` or a `Tip`.
///
/// This is konoma's list, not mermaid's: it is exactly the set of fields [`svg::emit`] reads that
/// nothing else in this file covers. Each one is checked the same way — the diagram is built
/// twice, once with the feature and once with it switched off at the *model* level — so the test
/// says "turning this on changes the page", which is the property a mutation that stops drawing it
/// destroys.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Decoration {
    /// `PlacedCluster::sections` — the `else` / `and` / `option` rule across a frame. **This is
    /// the one the stage-4 mutation removed while every named test stayed green.**
    SectionRule,
    /// `ClusterSection::title` — the words under that rule.
    SectionTitle,
    /// `PlacedCluster::title` — a subgraph's name.
    ClusterTitle,
    /// `PlacedCluster::dashed` — a concurrent region, and every sequence group frame.
    ClusterDash,
    /// `PlacedCluster::filled` — whether the frame is ground or an outline.
    ClusterFill,
    /// `PlacedEdge::badge` — an `autonumber` on its disc.
    Badge,
    /// `Diagram::lifelines` — a participant's axis.
    Lifeline,
    /// `PlacedLifeline::activations` — a bar on that axis.
    ActivationBar,
    /// `PlacedLifeline::destroyed` — the cross where a `destroy`ed participant stops.
    DestroyCross,
    /// `Panel::rules` — the horizontal rule between two compartments.
    PanelRule,
    /// `Panel::columns` — the vertical rule between two cells.
    PanelColumn,
    /// `PlacedEdge::label` plus the patch drawn under it where the line runs through the words.
    EdgeLabelPatch,
    /// `PlacedEdge::start_label` / `end_label` — a cardinality, set beside the line and bare.
    SideLabel,
    /// `PlacedEdge::stroke` — `-.-` dashes and `===` weight.
    StrokeStyle,
}

/// A diagram carrying `decoration`, or — when `on` is false — the same diagram with just that one
/// thing taken out of the model.
fn decoration_document(decoration: Decoration, on: bool) -> String {
    let mut d = canvas();
    match decoration {
        Decoration::SectionRule | Decoration::SectionTitle => {
            let title = if decoration == Decoration::SectionTitle && on {
                label("is well", 44.0)
            } else {
                blank()
            };
            d.clusters.push(PlacedCluster {
                id: "alt".to_string(),
                title: blank(),
                center: Point::new(200.0, 150.0),
                size: Size::new(240.0, 160.0),
                parent: None,
                depth: 0,
                dashed: true,
                filled: false,
                sections: if decoration == Decoration::SectionTitle || on {
                    vec![ClusterSection { y: 150.0, title }]
                } else {
                    Vec::new()
                },
            });
        }
        Decoration::ClusterTitle | Decoration::ClusterDash | Decoration::ClusterFill => {
            d.clusters.push(PlacedCluster {
                id: "g".to_string(),
                title: if decoration == Decoration::ClusterTitle && on {
                    label("Block", 44.0)
                } else {
                    blank()
                },
                center: Point::new(200.0, 150.0),
                size: Size::new(240.0, 160.0),
                parent: None,
                depth: 0,
                dashed: decoration == Decoration::ClusterDash && on,
                filled: decoration == Decoration::ClusterFill && on,
                sections: Vec::new(),
            });
        }
        Decoration::Badge
        | Decoration::EdgeLabelPatch
        | Decoration::SideLabel
        | Decoration::StrokeStyle => {
            let text = label("2", 12.0);
            d.edges.push(PlacedEdge {
                from: "a".to_string(),
                to: "b".to_string(),
                points: vec![Point::new(40.0, 150.0), Point::new(360.0, 150.0)],
                tip_start: Tip::None,
                tip_end: Tip::Arrow,
                stroke: if decoration == Decoration::StrokeStyle && on {
                    Stroke::Dotted
                } else {
                    Stroke::Normal
                },
                // Placed *on* the line, so the patch under it is drawn too — the pair is the
                // decoration, since a bare label with no patch is the `SideLabel` case.
                label: (decoration == Decoration::EdgeLabelPatch && on).then(|| PlacedEdgeLabel {
                    center: Point::new(200.0, 150.0),
                    size: Size::new(40.0, 18.0),
                    label: label("yes", 30.0),
                }),
                start_label: (decoration == Decoration::SideLabel && on).then(|| PlacedEdgeLabel {
                    center: Point::new(80.0, 130.0),
                    size: Size::new(24.0, 18.0),
                    label: label("1", 10.0),
                }),
                end_label: None,
                badge: (decoration == Decoration::Badge && on).then(|| PlacedEdgeLabel {
                    center: Point::new(60.0, 150.0),
                    size: Size::new(22.0, 22.0),
                    label: text,
                }),
                series: None,
                straight: false,
                overlay: false,
                style: None,
                curve: Curve::Basis,
            });
        }
        Decoration::Lifeline | Decoration::ActivationBar | Decoration::DestroyCross => {
            if decoration == Decoration::Lifeline && !on {
                // Nothing at all: the axis is what is being switched off.
            } else {
                d.lifelines.push(PlacedLifeline {
                    id: "A".to_string(),
                    x: 200.0,
                    top: 40.0,
                    bottom: 260.0,
                    destroyed: decoration == Decoration::DestroyCross && on,
                    activations: if decoration == Decoration::ActivationBar && on {
                        vec![PlacedActivation {
                            depth: 0,
                            top: 90.0,
                            bottom: 200.0,
                        }]
                    } else {
                        Vec::new()
                    },
                });
            }
        }
        Decoration::PanelRule | Decoration::PanelColumn => {
            let mut p = if decoration == Decoration::PanelColumn {
                an_er_panel()
            } else {
                a_class_panel()
            };
            if !on {
                if decoration == Decoration::PanelRule {
                    p.rules.clear();
                } else {
                    p.columns.clear();
                }
            }
            d.nodes.push(PlacedNode {
                id: "n".to_string(),
                shape: if decoration == Decoration::PanelColumn {
                    Glyph::ErBox
                } else {
                    Glyph::ClassBox
                },
                center: Point::new(200.0, 150.0),
                size: p.size,
                label: blank(),
                panel: Some(p),
                series: None,
                mark: None,
                style: None,
            });
        }
    }
    drawn(&d)
}

/// **Every decoration changes the page, and no two change it the same way.**
///
/// The one this test was written for is [`Decoration::SectionRule`]: stage 4's mutation removed it
/// and only the goldens noticed.
#[test]
fn every_decoration_leaves_a_mark_of_its_own_on_the_page() {
    fn covered(d: Decoration) -> bool {
        match d {
            Decoration::SectionRule
            | Decoration::SectionTitle
            | Decoration::ClusterTitle
            | Decoration::ClusterDash
            | Decoration::ClusterFill
            | Decoration::Badge
            | Decoration::Lifeline
            | Decoration::ActivationBar
            | Decoration::DestroyCross
            | Decoration::PanelRule
            | Decoration::PanelColumn
            | Decoration::EdgeLabelPatch
            | Decoration::SideLabel
            | Decoration::StrokeStyle => true,
        }
    }
    let table = [
        Decoration::SectionRule,
        Decoration::SectionTitle,
        Decoration::ClusterTitle,
        Decoration::ClusterDash,
        Decoration::ClusterFill,
        Decoration::Badge,
        Decoration::Lifeline,
        Decoration::ActivationBar,
        Decoration::DestroyCross,
        Decoration::PanelRule,
        Decoration::PanelColumn,
        Decoration::EdgeLabelPatch,
        Decoration::SideLabel,
        Decoration::StrokeStyle,
    ];
    let listed: HashSet<Decoration> = table.iter().copied().collect();
    assert_eq!(listed.len(), table.len(), "a decoration is listed twice");
    for d in &listed {
        assert!(covered(*d));
    }

    let mut by_change: BTreeMap<String, Vec<Decoration>> = BTreeMap::new();
    for decoration in table {
        let with = decoration_document(decoration, true);
        let without = decoration_document(decoration, false);
        assert_ne!(
            with, without,
            "{decoration:?}: switching it on changed nothing on the page — whatever it means, the \
             reader cannot see it"
        );
        // What it added, minus everything that was there without it. A decoration that only ever
        // *removes* markup (there is none today) would land here as an empty string and be caught
        // by the uniqueness check below.
        let added: String = with
            .lines()
            .filter(|l| !without.lines().any(|b| b == *l))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            !added.is_empty(),
            "{decoration:?}: switching it on removed markup instead of adding any"
        );
        by_change.entry(added).or_default().push(decoration);
    }
    for (added, decorations) in &by_change {
        assert_eq!(
            decorations.len(),
            1,
            "these decorations put the same marks on the page, so they cannot be told apart: \
             {decorations:?}\n--- the marks ---\n{added}"
        );
    }
}
