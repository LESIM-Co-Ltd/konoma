//! What stage 2's drawing is checked with.
//!
//! `docs/FEATURE-MERMAID-RENDERER.md` §6 sets the terms, and this file follows the shape stage 1
//! established, with one addition that matters:
//!
//! 1. **The main instrument still measures konoma against resvg**, not against konoma. A state
//!    box and a note box are sized by different rules from a flowchart's shapes, so each gets its
//!    own "box − drawn text == the padding the shape declares" check. This cannot decay into a
//!    self-comparison, because the other side of it is resvg's real shaper.
//! 2. **The geometric invariants are the same functions**, not copies of them:
//!    [`super::tests::check_nodes_do_not_overlap`] and its ten siblings are called from here over
//!    a state corpus. If a property drifts, it drifts for both diagram kinds at once — which is
//!    the opposite of what happened to konoma's markdown diff harness, where two things that were
//!    supposed to stay in step quietly stopped being compared at all.
//! 3. **The corpus is built from the specification**, not from bugs already found (memory
//!    `corpus-from-spec-not-from-bugs`): every construct `stateDiagram.md` documents, in the
//!    forms it documents them, plus CJK and the awkward shapes §6 asks for.
//!
//! # The one property that is about the *side*
//!
//! [`a_note_is_drawn_on_the_side_it_was_written_on`] asserts a note's box is entirely past its
//! state's edge on the requested side. This is deliberately not "a note node exists": the first
//! version of stage 2 followed mermaid and let dagre place notes, which drew a note reading "this
//! is the note to the left" on the right — and a presence test would have passed on that picture.

use std::collections::{HashMap, HashSet};

use super::state::{lay_out, render, render_flow, spec_of, NOTE_GAP};
use super::tests::{
    assert_snapshot, boundary, check_cluster_titles, check_clusters_hold_their_members,
    check_edge_that_names_a_block_stops_on_its_frame, check_edges_keep_out_of_foreign_frames,
    check_edges_stay_out_of_shapes, check_endpoints_land_on_the_outline,
    check_labels_ride_their_edge, check_nested_clusters_sit_inside_their_parent,
    check_nodes_do_not_overlap, check_unrelated_clusters_do_not_overlap,
    check_view_box_contains_everything, dist_to_boundary, mask_numbers, path_boxes, text_widths,
    tree_of,
};
use super::Routing;
use super::{
    clusters, labels, orthogonal, shapes, svg, Diagram, Glyph, Label, PlacedNode, RenderError, Size,
};
use crate::preview::mermaid::flowchart::{Shape, Stroke};
use crate::preview::mermaid::layout::Point;
use crate::preview::mermaid::state::{self, Kind};
use crate::preview::mermaid::text_metrics;

// ---------------------------------------------------------------------------------------------
// Corpus
// ---------------------------------------------------------------------------------------------

/// The diagrams every structural test runs over, and the ones [`gallery`] writes out to be looked
/// at. Every one of mermaid's own documented examples is here in the form the documentation gives
/// it, so that what is checked is what a reader is most likely to have written.
pub const CASES: &[(&str, &str)] = &[
    (
        "basic",
        "stateDiagram-v2\n    [*] --> Still\n    Still --> [*]\n\n    Still --> Moving\n    \
         Moving --> Still\n    Moving --> Crash\n    Crash --> [*]",
    ),
    (
        "v1-header",
        "stateDiagram\n    [*] --> Still\n    Still --> [*]\n    Still --> Moving",
    ),
    (
        "described",
        "stateDiagram-v2\n    state \"This is a state description\" as s2\n    s3 : Another\n    \
         [*] --> s2\n    s2 --> s3 : A transition\n    s3 --> [*]",
    ),
    (
        "titled",
        "stateDiagram-v2\n    s2 : Waiting for input\n    s2 : press any key\n    [*] --> s2\n    \
         s2 --> [*]",
    ),
    (
        "composite",
        "stateDiagram-v2\n    [*] --> First\n    state First {\n        [*] --> second\n        \
         second --> [*]\n    }\n    First --> [*]",
    ),
    (
        "composite-named",
        "stateDiagram-v2\n    [*] --> NamedComposite\n    NamedComposite: Another Composite\n    \
         state NamedComposite {\n        [*] --> namedSimple\n        namedSimple --> [*]\n        \
         namedSimple: Another simple\n    }",
    ),
    (
        "nested",
        "stateDiagram-v2\n    [*] --> First\n    state First {\n        [*] --> Second\n        \
         state Second {\n            [*] --> second\n            second --> Third\n            \
         state Third {\n                [*] --> third\n                third --> [*]\n            \
         }\n        }\n    }",
    ),
    (
        "composite-siblings",
        "stateDiagram-v2\n    [*] --> First\n    First --> Second\n    First --> Third\n\n    \
         state First {\n        [*] --> fir\n        fir --> [*]\n    }\n    state Second {\n        \
         [*] --> sec\n        sec --> [*]\n    }\n    state Third {\n        [*] --> thi\n        \
         thi --> [*]\n    }",
    ),
    (
        "choice",
        "stateDiagram-v2\n    state if_state <<choice>>\n    [*] --> IsPositive\n    \
         IsPositive --> if_state\n    if_state --> False: if n < 0\n    if_state --> True : if n >= 0",
    ),
    (
        "fork",
        "stateDiagram-v2\n    state fork_state <<fork>>\n      [*] --> fork_state\n      \
         fork_state --> State2\n      fork_state --> State3\n\n      state join_state <<join>>\n      \
         State2 --> join_state\n      State3 --> join_state\n      join_state --> State4\n      \
         State4 --> [*]",
    ),
    (
        "fork-lr",
        "stateDiagram-v2\n    direction LR\n    state f <<fork>>\n    [*] --> f\n    f --> A\n    \
         f --> B\n    state j <<join>>\n    A --> j\n    B --> j\n    j --> [*]",
    ),
    (
        "notes",
        "stateDiagram-v2\n        State1: The state with a note\n        note right of State1\n            \
         Important information! You can write\n            notes.\n        end note\n        \
         State1 --> State2\n        note left of State2 : This is the note to the left.",
    ),
    (
        "notes-crowded",
        "stateDiagram-v2\n    A --> B\n    A --> C\n    B --> D\n    C --> D\n    \
         note right of A : first\n    note right of B : second\n    note left of C : third",
    ),
    (
        // A note on a left-to-right diagram, where "to the right of" is the next rank's column.
        // Kept in the corpus because it is the case that shows the limitation
        // [`a_notes_connector_can_cross_a_state_in_a_crowded_diagram`] records: the note is on
        // the side it was asked for and its box is clear, and its connector passes a state.
        "note-lr",
        "stateDiagram-v2\n    direction LR\n    A --> B\n    A --> C\n    B --> D\n    C --> D\n    \
         note right of B : second",
    ),
    (
        "concurrent",
        "stateDiagram-v2\n    [*] --> Active\n\n    state Active {\n        [*] --> NumLockOff\n        \
         NumLockOff --> NumLockOn : EvNumLockPressed\n        NumLockOn --> NumLockOff : EvNumLockPressed\n        \
         --\n        [*] --> CapsLockOff\n        CapsLockOff --> CapsLockOn : EvCapsLockPressed\n        \
         CapsLockOn --> CapsLockOff : EvCapsLockPressed\n    }",
    ),
    (
        "direction-lr",
        "stateDiagram\n    direction LR\n    [*] --> A\n    A --> B\n    B --> C\n    \
         state B {\n      direction LR\n      a --> b\n    }\n    B --> D",
    ),
    (
        "styled",
        "stateDiagram\n   direction TB\n\n   accTitle: This is the accessible title\n   \
         accDescr: This is an accessible description\n\n   classDef notMoving fill:white\n   \
         classDef movement font-style:italic\n\n   [*]--> Still\n   Still --> [*]\n   \
         Still --> Moving\n   Moving --> Still\n   Moving --> Crash\n   Crash --> [*]\n\n   \
         class Still notMoving\n   class Moving movement",
    ),
    (
        "style-separator",
        "stateDiagram\n   classDef notMoving fill:white\n   [*] --> Still:::notMoving\n   \
         Still --> [*]\n   Still --> Moving:::movement\n   Moving --> Still",
    ),
    (
        "spaces-in-names",
        "stateDiagram\n    yswsii: Your state with spaces in it\n    [*] --> yswsii\n    \
         [*] --> SomeOtherState\n    SomeOtherState --> YetAnotherState\n    \
         yswsii --> YetAnotherState\n    YetAnotherState --> [*]",
    ),
    (
        "cjk",
        "stateDiagram-v2\n    [*] --> ツリー\n    ツリー --> プレビュー : Enter\n    \
         プレビュー --> ツリー : q\n    プレビュー --> [*]\n    state プレビュー {\n      \
         [*] --> 種別を解決\n      種別を解決 --> 全画面描画\n    }",
    ),
    (
        "cjk-note",
        "stateDiagram-v2\n    direction LR\n    [*] --> ツリー\n    ツリー --> 全画面\n    \
         note right of ツリー : 全画面のファイル一覧",
    ),
    (
        "multiline-label",
        "stateDiagram-v2\n    [*] --> A\n    A --> B : first<br>second\n    \
         B : one<br/>two<br />three",
    ),
    (
        "self-loop",
        "stateDiagram-v2\n    [*] --> Retry\n    Retry --> Retry : again\n    Retry --> [*]",
    ),
    (
        "block-endpoint",
        "stateDiagram-v2\n    state Inner {\n        a --> b\n    }\n    Outside --> Inner\n    \
         Inner --> Done",
    ),
];

// ---------------------------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------------------------

fn laid_out(src: &str) -> Diagram {
    let model = state::parse(src).unwrap_or_else(|e| panic!("corpus source must parse: {e}"));
    lay_out(&model, Routing::Splines).unwrap_or_else(|e| panic!("corpus source must lay out: {e}"))
}

/// The block tree of a source, rebuilt from the parser's own output — the same trick stage 1's
/// tests use, so the question "is this state inside that block?" is answered by the parser rather
/// than by the code under test.
fn tree_of_src(src: &str) -> clusters::Tree {
    let model = state::parse(src).expect("parses");
    let spec = spec_of(&model, Routing::Splines);
    let ids: HashSet<String> = spec.nodes.iter().map(|n| n.id.clone()).collect();
    clusters::Tree::from_blocks(&spec.blocks, |id| ids.contains(id))
}

/// The glyph a state ended up drawn as.
fn glyph_of(d: &Diagram, id: &str) -> Glyph {
    d.node(id).unwrap_or_else(|| panic!("no node {id}")).shape
}

// ---------------------------------------------------------------------------------------------
// 1. The main instrument: what konoma measured vs what resvg drew
// ---------------------------------------------------------------------------------------------

/// A state's box is its label plus `padding * 2`, and that has to be true of the text resvg
/// actually laid out, not of konoma's own idea of it. Same instrument as stage 1's
/// `box_width_matches_resvg`, over the glyph a state diagram is mostly made of.
#[test]
fn state_box_width_matches_resvg() {
    if !text_metrics::fonts_available() {
        eprintln!("no sans-serif face — skipping (the renderer refuses to draw here too)");
        return;
    }
    for label in [
        "Still",
        "AVATAR To Wa",
        "illicit lilli",
        "全画面プレビュー",
        "開始、処理。「確認」",
        "build 🚀 ship",
        "0123456789",
        // usvg folds runs of whitespace before shaping, so this is drawn one space narrower than
        // its bytes. `docs/STATUS.md` recorded that hole as belonging to every diagram kind.
        "loop  Every minute",
    ] {
        let src = format!("stateDiagram-v2\n  s : {label}\n  [*] --> s");
        let d = laid_out(&src);
        let svg = render(&src, "dark").expect("renders");
        let node = d.node("s").expect("the state");

        let mut widths = Vec::new();
        text_widths(tree_of(&svg).root(), &mut widths);
        assert_eq!(
            widths.len(),
            1,
            "{label:?}: exactly one <text> should survive usvg"
        );
        let slack = node.size.w - widths[0] as f64;
        assert!(
            (slack - shapes::PADDING * 2.0).abs() <= 1.0,
            "{label:?}: box {} wide, resvg drew {} → {} of padding, shape declares {}",
            svg::num(node.size.w),
            svg::num(widths[0] as f64),
            svg::num(slack),
            svg::num(shapes::PADDING * 2.0)
        );

        // …and the box konoma sized is the box it emitted.
        let mut boxes = Vec::new();
        path_boxes(tree_of(&svg).root(), &mut boxes);
        assert!(
            boxes
                .iter()
                .any(|b| (b.width() as f64 - node.size.w).abs() < 0.01),
            "{label:?}: no drawn path has the width the model declares ({})",
            svg::num(node.size.w)
        );
    }
}

/// The same comparison for a note, which is sized by its own rule and drawn with its own outline.
#[test]
fn note_box_width_matches_resvg() {
    if !text_metrics::fonts_available() {
        return;
    }
    let src = "stateDiagram-v2\n  A --> B\n  note right of A : 全画面の  ファイル一覧";
    let d = laid_out(src);
    let svg = render(src, "dark").expect("renders");
    let note = d
        .nodes
        .iter()
        .find(|n| n.shape == Glyph::Note)
        .expect("a note was drawn");

    let mut widths = Vec::new();
    text_widths(tree_of(&svg).root(), &mut widths);
    let drawn = widths.iter().copied().fold(0.0_f32, f32::max) as f64;
    let slack = note.size.w - drawn;
    assert!(
        (slack - shapes::PADDING * 2.0).abs() <= 1.0,
        "note box {} vs resvg's {} → {} of padding, declared {}",
        svg::num(note.size.w),
        svg::num(drawn),
        svg::num(slack),
        svg::num(shapes::PADDING * 2.0)
    );
}

// ---------------------------------------------------------------------------------------------
// 2. The geometric invariants — the same functions stage 1 runs
// ---------------------------------------------------------------------------------------------

/// Every invariant §6 lists, over the state corpus, through the code stage 1's tests call.
///
/// One test rather than eleven on purpose: they share the layout, which is the expensive part,
/// and the failure message already says which property and which case gave way.
#[test]
fn the_geometry_invariants_hold_for_state_diagrams() {
    if !text_metrics::fonts_available() {
        return;
    }
    for (name, src) in CASES {
        let d = laid_out(src);
        let tree = tree_of_src(src);
        check_nodes_do_not_overlap(name, &d);
        check_edges_stay_out_of_shapes(name, &d);
        check_labels_ride_their_edge(name, &d);
        check_endpoints_land_on_the_outline(name, &d);
        check_view_box_contains_everything(name, &d);
        check_clusters_hold_their_members(name, &d, &tree);
        check_nested_clusters_sit_inside_their_parent(name, &d);
        check_unrelated_clusters_do_not_overlap(name, &d);
        check_edges_keep_out_of_foreign_frames(name, &d, &tree);
        check_edge_that_names_a_block_stops_on_its_frame(name, &d, &tree);
        check_cluster_titles(name, &d, &tree);
    }
}

/// Everything the author wrote is on the page: one shape per drawable state, one frame per block,
/// one line per transition. A dropped state is how a renderer quietly lies about a diagram.
#[test]
fn every_state_and_transition_survives() {
    if !text_metrics::fonts_available() {
        return;
    }
    for (name, src) in CASES {
        let model = state::parse(src).expect("parses");
        let d = laid_out(src);
        let boxes = model.boxes().count();
        assert_eq!(d.nodes.len(), boxes, "{name}: box count");
        let tree = tree_of_src(src);
        let blocks: HashSet<&str> = tree.iter().map(|c| c.id.as_str()).collect();
        let drawn: HashSet<&str> = d.clusters.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(drawn, blocks, "{name}: frames and blocks disagree");
        assert_eq!(
            d.edges.len(),
            model.transitions.len(),
            "{name}: transition count"
        );
    }
}

// ---------------------------------------------------------------------------------------------
// 3. Which glyph each kind is drawn as
// ---------------------------------------------------------------------------------------------

/// **Start and end are drawn differently.** `[*]` means two opposite things depending on which
/// way the arrow points, and a renderer that draws one mark for both says the diagram has no
/// direction. Asserted on the outline, not on the glyph name, so folding the two together in
/// `shapes::outline` has to fail here too.
#[test]
fn a_start_and_an_end_are_told_apart() {
    if !text_metrics::fonts_available() {
        return;
    }
    let d = laid_out("stateDiagram-v2\n  [*] --> A\n  A --> [*]");
    assert_eq!(glyph_of(&d, "root_start"), Glyph::StateStart);
    assert_eq!(glyph_of(&d, "root_end"), Glyph::StateEnd);

    let start = shapes::outline(
        Glyph::StateStart,
        d.node("root_start").expect("start").size,
        None,
    );
    let end = shapes::outline(Glyph::StateEnd, d.node("root_end").expect("end").size, None);
    assert_ne!(start, end, "the two markers must not be the same outline");
    assert!(
        matches!(start, shapes::Outline::Disc { .. }),
        "a start is a solid dot, was {start:?}"
    );
    assert!(
        matches!(end, shapes::Outline::Target { .. }),
        "an end is a ring round a dot, was {end:?}"
    );

    // …and the drawing says so too: the end marker emits two circles, the start one.
    let svg = render("stateDiagram-v2\n  [*] --> A\n  A --> [*]", "dark").expect("renders");
    assert_eq!(
        svg.matches("<circle").count(),
        3,
        "one circle for the start, two for the end:\n{svg}"
    );
}

/// A choice is a diamond, and it carries no text — mermaid's `choice.ts` draws a polygon and no
/// label, so a description written on one was never drawn by anybody.
#[test]
fn a_choice_is_a_textless_diamond() {
    if !text_metrics::fonts_available() {
        return;
    }
    let d = laid_out(
        "stateDiagram-v2\n  state c <<choice>>\n  c : ignored\n  A --> c\n  c --> B\n  c --> C",
    );
    let c = d.node("c").expect("the choice");
    assert_eq!(c.shape, Glyph::Choice);
    assert!(c.label.is_blank(), "a choice draws no text");
    assert_eq!(c.size, Size::new(shapes::CHOICE_SIZE, shapes::CHOICE_SIZE));
    assert_eq!(shapes::polygon(Glyph::Choice, c.size).len(), 4);
}

/// A fork bar lies **across** the flow, so it turns with the diagram: a bar drawn along the flow
/// is a line, not a bar.
#[test]
fn a_fork_bar_turns_with_the_diagram() {
    if !text_metrics::fonts_available() {
        return;
    }
    for (dir, horizontal) in [("TB", true), ("BT", true), ("LR", false), ("RL", false)] {
        let src = format!(
            "stateDiagram-v2\n  direction {dir}\n  state f <<fork>>\n  state j <<join>>\n  \
             A --> f\n  f --> B\n  f --> C\n  B --> j\n  C --> j\n  j --> D"
        );
        let d = laid_out(&src);
        for id in ["f", "j"] {
            let bar = d.node(id).expect("the bar");
            assert_eq!(bar.shape, Glyph::Bar { horizontal }, "{dir} {id}");
            let (long, short) = if horizontal {
                (bar.size.w, bar.size.h)
            } else {
                (bar.size.h, bar.size.w)
            };
            assert_eq!(long, shapes::BAR_LENGTH, "{dir} {id}: long axis");
            assert_eq!(short, shapes::BAR_THICKNESS, "{dir} {id}: short axis");
        }
    }
}

/// One description is a plain box; two make a box with a rule under the first, and the rule is
/// actually drawn.
#[test]
fn two_descriptions_get_a_rule_under_the_first() {
    if !text_metrics::fonts_available() {
        return;
    }
    let one = laid_out("stateDiagram-v2\n  s : only\n  [*] --> s");
    assert_eq!(glyph_of(&one, "s"), Glyph::Flow(Shape::RoundedRect));

    let src = "stateDiagram-v2\n  s : first\n  s : second\n  [*] --> s";
    let two = laid_out(src);
    let node = two.node("s").expect("the state");
    assert_eq!(node.shape, Glyph::TitledBox);
    assert_eq!(node.label.lines, vec!["first", "second"]);

    let svg = render(src, "dark").expect("renders");
    let rule_y = node.center.y - node.label.height / 2.0 + labels::line_height();
    assert!(
        svg.contains(&format!("y1=\"{}\"", svg::num(rule_y))),
        "the rule should sit where the first line ends ({}):\n{svg}",
        svg::num(rule_y)
    );
}

/// A composite is a frame and never a box; a `--` region is a frame with no title, dashed so that
/// a reader can tell "at the same time" from "inside".
#[test]
fn composites_and_regions_are_frames() {
    if !text_metrics::fonts_available() {
        return;
    }
    let d = laid_out(
        "stateDiagram-v2\n  [*] --> Active\n  state Active {\n    [*] --> A\n    --\n    \
         [*] --> B\n  }",
    );
    assert!(d.node("Active").is_none(), "a composite is not a box");
    let outer = d.cluster("Active").expect("the composite's frame");
    assert_eq!(outer.title.lines, vec!["Active".to_string()]);
    assert!(!outer.dashed, "a composite has a title and needs no hint");

    let regions: Vec<&super::PlacedCluster> = d
        .clusters
        .iter()
        .filter(|c| c.id.starts_with("divider"))
        .collect();
    assert_eq!(regions.len(), 2, "one frame per concurrent region");
    for r in regions {
        assert!(r.dashed, "a region is dashed");
        assert!(r.title.is_blank(), "a region has no title");
        assert_eq!(r.parent.as_deref(), Some("Active"));
    }

    let svg = render(
        "stateDiagram-v2\n  [*] --> Active\n  state Active {\n    [*] --> A\n    --\n    \
         [*] --> B\n  }",
        "dark",
    )
    .expect("renders");
    assert_eq!(
        svg.matches(&format!("stroke-dasharray=\"{}\"", svg::CLUSTER_DASH))
            .count(),
        2,
        "both regions are drawn dashed:\n{svg}"
    );
}

// ---------------------------------------------------------------------------------------------
// 4. Notes
// ---------------------------------------------------------------------------------------------

/// **The side is the side that was written.** Not "a note exists": the first version of stage 2
/// followed mermaid and let dagre place notes, which drew a note whose own text said "to the
/// left" on the right — and a presence test passes on that picture.
#[test]
fn a_note_is_drawn_on_the_side_it_was_written_on() {
    if !text_metrics::fonts_available() {
        return;
    }
    for (name, src) in CASES {
        let model = state::parse(src).expect("parses");
        let d = laid_out(src);
        for note in model.states.iter().filter(|s| s.kind == Kind::Note) {
            let position = note.note_position.expect("a note knows its side");
            let link = model
                .transitions
                .iter()
                .find(|t| t.is_note_link && (t.from == note.id || t.to == note.id))
                .expect("a note has a tie-line");
            let anchor_id = if link.from == note.id {
                &link.to
            } else {
                &link.from
            };
            let anchor = d
                .node(anchor_id)
                .map(PlacedNode::bounds)
                .or_else(|| d.cluster(anchor_id).map(|c| c.bounds()))
                .expect("the state the note belongs to");
            let placed = d.node(&note.id).expect("the note is drawn");
            let (nl, _, nr, _) = placed.bounds();
            match position {
                state::NotePosition::Right => assert!(
                    nl >= anchor.2 + NOTE_GAP - 0.01,
                    "{name}: `note right of {anchor_id}` starts at x={} but the state ends at {}",
                    svg::num(nl),
                    svg::num(anchor.2)
                ),
                state::NotePosition::Left => assert!(
                    nr <= anchor.0 - NOTE_GAP + 0.01,
                    "{name}: `note left of {anchor_id}` ends at x={} but the state starts at {}",
                    svg::num(nr),
                    svg::num(anchor.0)
                ),
            }
        }
    }
}

/// A note's connector touches the note at one end and its state at the other, is drawn without an
/// arrow head, and is dotted so it cannot be read as a transition.
#[test]
fn a_notes_connector_is_dotted_headless_and_lands_on_both_boxes() {
    if !text_metrics::fonts_available() {
        return;
    }
    let src = "stateDiagram-v2\n  A --> B\n  note right of A : hi\n  note left of B : there";
    let d = laid_out(src);
    let ties: Vec<&super::PlacedEdge> = d
        .edges
        .iter()
        .filter(|e| e.from.contains("note") || e.to.contains("note"))
        .collect();
    assert_eq!(ties.len(), 2);
    for e in ties {
        assert_eq!(e.tip_end, super::Tip::None, "a note is not a transition");
        assert_eq!(e.tip_start, super::Tip::None);
        assert_eq!(e.stroke, Stroke::Dotted);
        assert_eq!(e.points.len(), 2, "a straight connector");
        for (end, p) in [("start", &e.points[0]), ("end", &e.points[1])] {
            let id = if end == "start" { &e.from } else { &e.to };
            let node = d.node(id).expect("both ends are boxes");
            let off = dist_to_boundary(p, &boundary(node));
            assert!(
                off <= 0.05,
                "the {end} of the connector is {}px off {}'s outline",
                svg::num(off),
                id
            );
        }
    }
}

/// Two notes that want the same place are stacked, never overlapped — and both stay on the side
/// they asked for, which [`a_note_is_drawn_on_the_side_it_was_written_on`] also covers for this
/// source through the corpus.
#[test]
fn notes_that_collide_are_stacked() {
    if !text_metrics::fonts_available() {
        return;
    }
    let d = laid_out(
        "stateDiagram-v2\n  A --> B\n  note right of A : first\n  note right of A : second",
    );
    let notes: Vec<&PlacedNode> = d.nodes.iter().filter(|n| n.shape == Glyph::Note).collect();
    assert_eq!(notes.len(), 2);
    let (a, b) = (notes[0].bounds(), notes[1].bounds());
    let (dx, dy) = (a.2.min(b.2) - a.0.max(b.0), a.3.min(b.3) - a.1.max(b.1));
    assert!(dx <= 0.01 || dy <= 0.01, "the two notes overlap");
    assert!(
        (a.0 - b.0).abs() < 0.01,
        "stacking moves a note down, never sideways"
    );
}

/// **A known limitation, recorded rather than papered over.** A note's connector can cross a
/// state's box when there is no clear line to the side the author asked for.
///
/// The note itself is always where the words say — that is [`a_note_is_drawn_on_the_side_it_was_written_on`],
/// and it is the property that matters, because a note on the wrong side contradicts its own
/// text. The connector is the part that has to give: on a left-to-right diagram "to the right of"
/// is the *next rank's column*, so a line reaching a note there passes whatever the layout put in
/// between, and no row on that side avoids it.
///
/// The alternatives were both worse and both were tried. Placing the note where a clear line
/// exists means putting it on the wrong side, which is where stage 2 started and what the whole
/// placement pass exists to undo. Sliding the note further out until the line is clear sends it
/// the width of the picture away and drags the line across *everything*. So the box is
/// guaranteed and the connector is not, and `check_edges_stay_out_of_shapes` exempts a note's
/// connector for exactly this reason.
///
/// `#[ignore]`d with the reason rather than deleted: the test says what would be better, and it
/// fails for as long as it is not true.
#[test]
#[ignore = "known limitation: a note is placed on the side that was written, and on a crowded diagram no row on that side has a clear line to it. Fixing it means moving the note off the side it was asked for, or routing the connector rather than drawing it straight."]
fn a_notes_connector_can_cross_a_state_in_a_crowded_diagram() {
    if !text_metrics::fonts_available() {
        return;
    }
    let src = CASES
        .iter()
        .find(|(n, _)| *n == "note-lr")
        .expect("the case is in the corpus")
        .1;
    let d = laid_out(src);
    let outlines: Vec<Vec<Point>> = d.nodes.iter().map(boundary).collect();
    for e in &d.edges {
        for w in e.drawn_points().windows(2) {
            let len = (w[1].x - w[0].x).hypot(w[1].y - w[0].y);
            let steps = ((len * 2.0).ceil() as usize).clamp(1, 4000);
            for i in 0..=steps {
                let t = i as f64 / steps as f64;
                let p = Point::new(
                    w[0].x + t * (w[1].x - w[0].x),
                    w[0].y + t * (w[1].y - w[0].y),
                );
                for (node, poly) in d.nodes.iter().zip(&outlines) {
                    let inside_by = super::tests::depth(&p, poly);
                    assert!(
                        inside_by <= 1.0,
                        "edge {} -> {} runs {}px inside {}",
                        e.from,
                        e.to,
                        svg::num(inside_by),
                        node.id
                    );
                }
            }
        }
    }
}

/// `note over A` is not part of this grammar — `stateDiagram.jison` has `left of`, `right of` and
/// the floating form and nothing else — so it is dropped rather than guessed at, and above all it
/// does not become a state called `note`.
#[test]
fn note_over_is_dropped_rather_than_guessed_at() {
    let model =
        state::parse("stateDiagram-v2\n  A --> B\n  note over A : hi\n  note over A, B : x")
            .expect("parses");
    assert_eq!(model.state_ids(), vec!["A", "B"]);
    assert!(model.transitions.iter().all(|t| !t.is_note_link));
}

// ---------------------------------------------------------------------------------------------
// 5. Degrading rather than lying (§6)
// ---------------------------------------------------------------------------------------------

/// A diagram of another kind is not drawn by this renderer.
#[test]
fn another_diagram_kind_is_refused() {
    assert!(matches!(
        render("flowchart TD\n  A --> B", "dark"),
        Err(RenderError::StateParse(_))
    ));
    assert!(matches!(
        render("sequenceDiagram\n  A->>B: hi", "dark"),
        Err(RenderError::StateParse(_))
    ));
}

/// A header with no body is not a diagram either, and the reason survives.
#[test]
fn a_state_diagram_with_no_states_is_refused() {
    let e = render("stateDiagram-v2", "dark").unwrap_err();
    assert!(matches!(e, RenderError::StateParse(_)));
    assert_eq!(e.to_string(), "state diagram declares no states");
}

/// Sources chosen to be awkward rather than realistic. The renderer runs on a worker thread
/// wrapped in `catch_unwind` (§1) — a safety net, not a licence — so each of these has to come
/// back with a drawable diagram or an error, never a panic and never a NaN.
#[test]
fn awkward_sources_produce_a_diagram_or_an_error_and_never_a_panic() {
    if !text_metrics::fonts_available() {
        return;
    }
    let mut wide = String::from("stateDiagram-v2\n");
    for i in 0..120 {
        wide.push_str(&format!("  n{i} --> n{}\n", i + 1));
    }
    wide.push_str("  n0 --> n120\n");

    let mut deep = String::from("stateDiagram-v2\n");
    for i in 0..12 {
        deep.push_str(&format!("  state s{i} {{\n"));
    }
    deep.push_str("  [*] --> A\n  A --> [*]\n");
    for _ in 0..12 {
        deep.push_str("  }\n");
    }
    deep.push_str("  s0 --> C\n");

    let cases: &[&str] = &[
        "",
        "   \n\n  ",
        "stateDiagram-v2",
        "stateDiagram-v2\n  [*]",
        "stateDiagram-v2\n  [*] --> [*]",
        "stateDiagram-v2\n  A --> A",
        "stateDiagram-v2\n  s : \"\"\n  [*] --> s",
        "stateDiagram-v2\n  s : <br><br><br>\n  [*] --> s",
        "stateDiagram-v2\n  A[\"<script>&amp;\"] --> B",
        "stateDiagram-v2\n  一 --> 二 : 🎉",
        "stateDiagram-v2\n  state S {}\n  A --> B",
        "stateDiagram-v2\n  state S {}\n  S --> A",
        "stateDiagram-v2\n  state S { A --> B }\n  S --> S",
        "stateDiagram-v2\n  state S {\n    A --> B\n  }\n  S --> A",
        "stateDiagram-v2\n  state S {\n    --\n  }\n  A --> B",
        "stateDiagram-v2\n  note right of Nobody : orphan\n  A --> B",
        "stateDiagram-v2\n  state S { a --> b }\n  note right of S : on a frame",
        "stateDiagram-v2\n  A --> B\n  note right of A : x\n  note right of A : y\n  \
         note right of A : z",
        "stateDiagram-v2\n  direction LR\n  A --> B\n  note left of A : far left",
        "stateDiagram-v2\n  state c <<choice>>\n  c --> c",
        "stateDiagram-v2\n  state f <<fork>>\n  f --> f",
        &deep,
        &wide,
    ];
    for src in cases {
        match render(src, "dark") {
            Err(_) => {}
            Ok(svg) => {
                assert!(!svg.contains("NaN"), "NaN reached the document for {src:?}");
                assert!(!svg.contains("inf"), "an infinity reached it for {src:?}");
                let d = laid_out(src);
                assert!(
                    d.width.is_finite() && d.height.is_finite() && d.width > 0.0 && d.height > 0.0,
                    "{src:?}: {}x{} is not a drawable size",
                    svg::num(d.width),
                    svg::num(d.height)
                );
                assert!(
                    crate::preview::svg::rasterize_bytes(
                        svg.as_bytes(),
                        std::path::Path::new("m.svg"),
                        300
                    )
                    .is_some(),
                    "{src:?}: the output did not rasterise"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// 6. Goldens
// ---------------------------------------------------------------------------------------------

/// The regression net over the whole corpus, **through [`render`]** — the function
/// `preview::markdown::mermaid_to_svg` calls, not a stand-in for it (§6).
///
/// Numeric attribute values are masked: every coordinate descends from the system's sans-serif
/// face, so a byte-exact end-to-end golden would be a golden of this machine's fonts. What is
/// pinned is everything else — which elements, in what order, with which colours, which path
/// commands and which text. The numbers are pinned separately by [`state_emit_golden`], which
/// builds its diagram by hand.
#[test]
fn state_corpus_golden() {
    if !text_metrics::fonts_available() {
        return;
    }
    let mut out = String::new();
    for (name, src) in CASES {
        out.push_str(&format!("=== {name} ===\n"));
        out.push_str(&mask_numbers(&render(src, "dark").expect("renders")));
        out.push('\n');
    }
    assert_snapshot("mermaid_state", &out);
}

/// A diagram assembled by hand, so **no font is involved and every number is pinned exactly**:
/// the six glyphs stage 2 adds, in every palette, plus a dashed frame and a note's connector.
#[test]
fn state_emit_golden() {
    let d = synthetic_state_diagram();
    let mut out = String::new();
    for t in super::theme::ALL {
        out.push_str(&format!("=== {} ===\n", t.name));
        out.push_str(&svg::emit(&d, t));
        out.push('\n');
    }
    assert_snapshot("mermaid_state_emit", &out);
}

/// One node of every glyph stage 2 adds, at coordinates chosen by hand.
fn synthetic_state_diagram() -> Diagram {
    let label = |text: &str, w: f64| Label {
        lines: text.split('\n').map(str::to_string).collect(),
        width: w,
        height: text.split('\n').count() as f64 * labels::line_height(),
    };
    let glyphs = [
        Glyph::StateStart,
        Glyph::StateEnd,
        Glyph::Choice,
        Glyph::Bar { horizontal: true },
        Glyph::Bar { horizontal: false },
        Glyph::Note,
        Glyph::TitledBox,
        Glyph::Flow(Shape::RoundedRect),
    ];
    let mut nodes = Vec::new();
    for (i, glyph) in glyphs.into_iter().enumerate() {
        let text = match glyph {
            Glyph::TitledBox => "title\nbody",
            Glyph::Note => "a note",
            _ => "state",
        };
        let l = label(text, 50.0);
        let size = shapes::size(glyph, Size::new(l.width, l.height));
        nodes.push(PlacedNode {
            id: format!("n{i}"),
            shape: glyph,
            center: Point::new(90.0 + (i % 4) as f64 * 180.0, 90.0 + (i / 4) as f64 * 180.0),
            size,
            label: l,
            panel: None,
            series: None,
            mark: None,
            style: None,
        });
    }

    let edges = vec![
        super::PlacedEdge {
            from: "n0".to_string(),
            to: "n7".to_string(),
            points: vec![Point::new(60.0, 420.0), Point::new(260.0, 420.0)],
            gaps: Vec::new(),
            tip_start: super::Tip::None,
            tip_end: super::Tip::Arrow,
            stroke: Stroke::Normal,
            label: Some(super::PlacedEdgeLabel {
                center: Point::new(160.0, 420.0),
                size: Size::new(50.0 + super::LABEL_PAD_X * 2.0, 20.0),
                label: label("on", 50.0),
            }),
            start_label: None,
            end_label: None,
            badge: None,
            series: None,
            straight: false,
            overlay: false,
            style: None,
            curve: super::Curve::Basis,
            tip_matches_line: false,
        },
        super::PlacedEdge {
            from: "n7".to_string(),
            to: "n5".to_string(),
            points: vec![Point::new(60.0, 460.0), Point::new(260.0, 460.0)],
            gaps: Vec::new(),
            tip_start: super::Tip::None,
            tip_end: super::Tip::None,
            stroke: Stroke::Dotted,
            label: None,
            start_label: None,
            end_label: None,
            badge: None,
            series: None,
            straight: false,
            overlay: false,
            style: None,
            curve: super::Curve::Basis,
            tip_matches_line: false,
        },
    ];

    let clusters = vec![
        super::PlacedCluster {
            id: "block".to_string(),
            title: label("A composite", 110.0),
            center: Point::new(300.0, 560.0),
            size: Size::new(400.0, 140.0),
            parent: None,
            depth: 0,
            dashed: false,
            filled: true,
            sections: Vec::new(),
        },
        super::PlacedCluster {
            id: "region".to_string(),
            title: label("", 0.0),
            center: Point::new(300.0, 580.0),
            size: Size::new(300.0, 80.0),
            parent: Some("block".to_string()),
            depth: 1,
            dashed: true,
            filled: true,
            sections: Vec::new(),
        },
    ];

    Diagram {
        width: 800.0,
        height: 660.0,
        nodes,
        edges,
        clusters,
        lifelines: Vec::new(),
    }
}

// ---------------------------------------------------------------------------------------------
// 7. `konoma-orthogonal` routing (§10-5) — flowchart's own invariants, extended to state diagrams
// ---------------------------------------------------------------------------------------------

/// The three "design reference" sources round 4 confirms against
/// (`docs/render-check/zz-design-sources.md`'s own "第 4 回" section, pictures
/// `docs/render-check/zz-design-4a/4b/4c-browser.png`), made a **permanent** fixture list
/// (coordinator instruction, 2026-09-02): every orthogonal invariant below runs against all
/// three on every test run, not only the one-off audit that originally produced them. Kept apart
/// from [`CASES`] — `CASES` also drives [`state_corpus_golden`], and this task's own governing
/// rule (`docs/FEATURE-MERMAID-RENDERER.md` §10-2) is that the splines golden must never move.
fn orthogonal_design_reference_corpus() -> Vec<(&'static str, &'static str)> {
    vec![
        (
            // `zz-design-4a-browser.png`'s own source (TB, a real sample-shaped diagram: a
            // composite state with an internal start marker and transition, plus a labelled
            // self-closing `Q`-triggered exit).
            "zz-design-4a",
            "stateDiagram-v2\n  [*] --> ツリー\n  ツリー --> プレビュー : Enter\n  \
             プレビュー --> ツリー : q\n  state プレビュー {\n    [*] --> デコード中\n    \
             デコード中 --> 表示 : 画像が届く\n  }\n  ツリー --> [*] : Q",
        ),
        (
            // `zz-design-4b-browser.png`'s own source (LR, a self-transition plus a choice with
            // two outgoing labelled edges).
            "zz-design-4b",
            "stateDiagram-v2\n  direction LR\n  state 分岐 <<choice>>\n  待機 --> 監視 : 開始\n  \
             監視 --> 監視 : ポーリング\n  監視 --> 分岐 : 変化\n  分岐 --> 更新 : 差分あり\n  \
             分岐 --> 休止 : 差分なし\n  更新 --> 通知 : 適用\n  通知 --> 待機 : 完了",
        ),
        (
            // `zz-design-4c-browser.png`'s own source (TB, fork/join plus a two-level nested
            // composite state).
            "zz-design-4c",
            "stateDiagram-v2\n  state fork_state <<fork>>\n  state join_state <<join>>\n  \
             [*] --> 初期化\n  初期化 --> fork_state\n  fork_state --> 取得\n  \
             fork_state --> 監査\n  取得 --> 処理\n  state 処理 {\n    [*] --> 整形\n    \
             整形 --> 解析\n    state 解析 {\n      走査 --> 集計\n    }\n  }\n  \
             処理 --> join_state\n  監査 --> join_state : 監査済\n  join_state --> 完了\n  \
             完了 --> [*]",
        ),
    ]
}

/// [`CASES`] plus [`orthogonal_design_reference_corpus`] — every orthogonal invariant that has to
/// see the design-reference sources iterates this rather than either half alone.
fn orthogonal_full_corpus() -> Vec<(&'static str, &'static str)> {
    CASES
        .iter()
        .copied()
        .chain(orthogonal_design_reference_corpus())
        .collect()
}

/// Redumps `4a`/`4b`/`4c` under `konoma-orthogonal` to `docs/render-check/zz-design-<name>-
/// ours.{svg,png}`, next to the existing `zz-design-<name>-browser.png` reference each was drawn
/// from — the state-diagram sibling of `tests::orthogonal_design_reference_dump`, see that
/// function's own doc for the naming rationale and why `docs/render-check/` is safe to write to.
///
/// `#[ignore]`d like [`gallery`] — run explicitly:
/// `cargo test --features git -- --ignored orthogonal_design_reference_dump`.
#[test]
#[ignore = "writes PNG/SVG files for a person to look at: cargo test -- --ignored orthogonal_design_reference_dump"]
fn orthogonal_design_reference_dump() {
    let dir = std::path::Path::new("docs/render-check");
    std::fs::create_dir_all(dir).expect("create docs/render-check");
    for (name, src) in orthogonal_design_reference_corpus() {
        let svg = render_flow(src, "dark", "konoma-orthogonal")
            .unwrap_or_else(|e| panic!("{name}: must render under konoma-orthogonal: {e}"));
        let svg_path = dir.join(format!("{name}-ours.svg"));
        std::fs::write(&svg_path, &svg).unwrap_or_else(|e| panic!("{name}: write svg: {e}"));
        let img = crate::preview::svg::rasterize_bytes(svg.as_bytes(), &svg_path, 1600)
            .unwrap_or_else(|| panic!("{name}: must rasterise"));
        img.save(dir.join(format!("{name}-ours.png")))
            .unwrap_or_else(|e| panic!("{name}: write png: {e}"));
    }
}

fn laid_out_orthogonal(src: &str) -> Diagram {
    let model = state::parse(src).unwrap_or_else(|e| panic!("corpus source must parse: {e}"));
    lay_out(&model, Routing::Orthogonal)
        .unwrap_or_else(|e| panic!("corpus source must lay out under orthogonal: {e}"))
}

/// Every segment of every routed edge is axis-parallel — no diagonal line — the same property
/// `orthogonal_routing_draws_only_axis_parallel_segments` (`tests.rs`) states for a flowchart,
/// run here over the state corpus (§10-5's own routing-mode extension).
#[test]
fn orthogonal_state_edges_draw_only_axis_parallel_segments() {
    if !text_metrics::fonts_available() {
        return;
    }
    for (name, src) in orthogonal_full_corpus() {
        let d = laid_out_orthogonal(src);
        for e in &d.edges {
            // A note's connector (`place_notes`'s own module doc) is placed after `lay_out_spec`
            // runs and is deliberately a straight line, never routed by `orthogonal` at all — the
            // same reason `check_edges_stay_out_of_shapes` exempts it in splines mode. Identified
            // the same way the rest of this file already does (`a_notes_connector_is_dotted_
            // headless_and_lands_on_both_boxes`'s own filter).
            if e.from.contains("note") || e.to.contains("note") {
                continue;
            }
            for w in e.points.windows(2) {
                let dx = (w[1].x - w[0].x).abs();
                let dy = (w[1].y - w[0].y).abs();
                assert!(
                    dx < 1e-6 || dy < 1e-6,
                    "{name}: edge {}->{} draws a diagonal segment {:?} -> {:?}",
                    e.from,
                    e.to,
                    w[0],
                    w[1]
                );
            }
        }
    }
}

/// Every routed edge's two endpoints sit exactly [`orthogonal::PORT_INSET`] outside the
/// node/cluster/marker they meet, and arrive perpendicular to whichever face — the same property
/// `orthogonal_endpoints_sit_outside_the_node_and_arrive_perpendicular` (`tests.rs`) states for a
/// flowchart. A start/end marker's own pole is just another face under this same test — S1's
/// "極に垂直入射" is nothing but "perpendicular to the flow-axis face" restated, which this
/// already checks without a marker-specific branch.
#[test]
fn orthogonal_state_endpoints_sit_outside_and_perpendicular() {
    if !text_metrics::fonts_available() {
        return;
    }
    for (name, src) in orthogonal_full_corpus() {
        let d = laid_out_orthogonal(src);
        for e in &d.edges {
            // A note's connector is placed after `lay_out_spec` and is never routed by
            // `orthogonal` — see the axis-parallel test above for the full reasoning.
            if e.from.contains("note") || e.to.contains("note") {
                continue;
            }
            if e.points.len() < 2 {
                continue;
            }
            let n = e.points.len();
            let ends = [
                (&e.from, &e.points[0], &e.points[1]),
                (&e.to, &e.points[n - 1], &e.points[n - 2]),
            ];
            for (node_id, endpoint, neighbour) in ends {
                let bounds = d
                    .node(node_id)
                    .map(|nd| nd.bounds())
                    .or_else(|| d.cluster(node_id).map(|c| c.bounds()));
                let Some((l, t, r, b)) = bounds else {
                    continue;
                };
                let inset = orthogonal::PORT_INSET;
                let on_top = (endpoint.y - (t - inset)).abs() < 1e-6;
                let on_bottom = (endpoint.y - (b + inset)).abs() < 1e-6;
                let on_left = (endpoint.x - (l - inset)).abs() < 1e-6;
                let on_right = (endpoint.x - (r + inset)).abs() < 1e-6;
                assert!(
                    on_top || on_bottom || on_left || on_right,
                    "{name}: edge {}->{} endpoint at {node_id} {endpoint:?} is not {inset}px \
                     outside its bounds {:?}",
                    e.from,
                    e.to,
                    (l, t, r, b)
                );
                let dx = (endpoint.x - neighbour.x).abs();
                let dy = (endpoint.y - neighbour.y).abs();
                assert!(
                    dx < 1e-6 || dy < 1e-6,
                    "{name}: edge {}->{} segment into {node_id} is not axis-parallel",
                    e.from,
                    e.to
                );
            }
        }
    }
}

/// No routed edge crosses a foreign node's box (its own two ends excepted) — the same property
/// `orthogonal_no_segment_crosses_a_foreign_node_across_the_whole_corpus` (`tests.rs`) states for
/// a flowchart.
#[test]
fn orthogonal_state_no_edge_crosses_a_foreign_node() {
    if !text_metrics::fonts_available() {
        return;
    }
    for (name, src) in orthogonal_full_corpus() {
        let d = laid_out_orthogonal(src);
        for e in &d.edges {
            for w in e.points.windows(2) {
                for n in &d.nodes {
                    if n.id == e.from || n.id == e.to {
                        continue;
                    }
                    assert!(
                        !orthogonal::segment_crosses_node(&w[0], &w[1], n),
                        "{name}: edge {}->{} segment {:?}->{:?} crosses {} {:?}",
                        e.from,
                        e.to,
                        w[0],
                        w[1],
                        n.id,
                        n.bounds()
                    );
                }
            }
        }
    }
}

/// No routed edge re-enters its own endpoint node's interior — the same unpadded property
/// `orthogonal_no_edge_crosses_its_own_endpoint_across_the_whole_corpus` (`tests.rs`) states for
/// a flowchart.
#[test]
fn orthogonal_state_no_edge_crosses_its_own_endpoint() {
    if !text_metrics::fonts_available() {
        return;
    }
    for (name, src) in orthogonal_full_corpus() {
        let d = laid_out_orthogonal(src);
        for e in &d.edges {
            // A note's connector is placed after `lay_out_spec` and is never routed by
            // `orthogonal` — see the axis-parallel test above for the full reasoning. Unlike a
            // cluster-anchored end, a note *is* a real `PlacedNode` (`place_notes` pushes it to
            // `out.nodes`), so the `let-else` just below would not skip it on its own.
            if e.from.contains("note") || e.to.contains("note") {
                continue;
            }
            let (Some(source), Some(target)) = (d.node(&e.from), d.node(&e.to)) else {
                continue; // a cluster-anchored end has no real node box to puncture
            };
            assert!(
                !orthogonal::staircase_punctures_its_own_endpoint(
                    &e.points,
                    Some(source),
                    Some(target)
                ),
                "{name}: edge {}->{} re-enters its own endpoint node's interior: {:?}",
                e.from,
                e.to,
                e.points
            );
        }
    }
}

/// The shared cluster/node invariants stage 1 states (§2 above), run again under orthogonal
/// routing — the same reason `invariant_orthogonal_clusters_hold_their_members` and its three
/// siblings (`tests.rs`) exist for flowcharts: eviction growth moves boxes, and nothing before
/// this had checked a composite state's frame still holds its members once that growth happens.
#[test]
fn orthogonal_state_nodes_and_clusters_stay_correct_after_growth() {
    if !text_metrics::fonts_available() {
        return;
    }
    for (name, src) in orthogonal_full_corpus() {
        let d = laid_out_orthogonal(src);
        let tree = tree_of_src(src);
        check_nodes_do_not_overlap(name, &d);
        check_view_box_contains_everything(name, &d);
        check_clusters_hold_their_members(name, &d, &tree);
        check_nested_clusters_sit_inside_their_parent(name, &d);
        check_unrelated_clusters_do_not_overlap(name, &d);
    }
}

/// S1: a start/end marker's only port is its pole, and its box never grows to make room for a
/// port — across the whole corpus, not just a hand-built case. A marker's pole is the face-centre
/// point on whichever axis its one edge actually uses (§10-1's own "曲げ0優先"/"1"), so this
/// checks the endpoint lands exactly on the node's own centre line along one axis, and that the
/// box never exceeds the fixed size splines draws (`shapes::size`'s own `Glyph::StateStart`/
/// `StateEnd` rule, unconditional on routing).
#[test]
fn orthogonal_state_markers_are_pole_ports_and_never_grow() {
    if !text_metrics::fonts_available() {
        return;
    }
    for (name, src) in orthogonal_full_corpus() {
        let d = laid_out_orthogonal(src);
        for n in &d.nodes {
            if !matches!(n.shape, Glyph::StateStart | Glyph::StateEnd) {
                continue;
            }
            let fixed = shapes::size(n.shape, Size::new(0.0, 0.0));
            assert_eq!(
                n.size, fixed,
                "{name}: marker {} grew from its fixed size under orthogonal routing",
                n.id
            );
        }
        for e in &d.edges {
            for (node_id, endpoint) in [(&e.from, e.points.first()), (&e.to, e.points.last())] {
                let Some(endpoint) = endpoint else { continue };
                let Some(node) = d.node(node_id) else {
                    continue;
                };
                if !matches!(node.shape, Glyph::StateStart | Glyph::StateEnd) {
                    continue;
                }
                let on_pole = (endpoint.x - node.center.x).abs() < 1e-6
                    || (endpoint.y - node.center.y).abs() < 1e-6;
                assert!(
                    on_pole,
                    "{name}: marker {node_id}'s port at {endpoint:?} is not on its pole \
                     (centre {:?})",
                    node.center
                );
            }
        }
    }
}

/// S1: a marker more than one transition shares is drawn as one marker **per** transition under
/// orthogonal routing — never a single dot/ring carrying more than one port. `basic`'s own two
/// transitions into `[*]` (`Still --> [*]`, `Crash --> [*]`) share `root_end` under splines; under
/// orthogonal there must be two separate end markers, each with exactly one edge.
#[test]
fn orthogonal_state_shared_end_marker_is_duplicated_per_transition() {
    if !text_metrics::fonts_available() {
        return;
    }
    let src = CASES
        .iter()
        .find(|(n, _)| *n == "basic")
        .expect("`basic` is in the corpus")
        .1;
    let splines = laid_out(src);
    let ortho = laid_out_orthogonal(src);
    let end_count = |d: &Diagram| {
        d.nodes
            .iter()
            .filter(|n| n.shape == Glyph::StateEnd)
            .count()
    };
    assert_eq!(
        end_count(&splines),
        1,
        "splines must keep mermaid's own single shared end dot"
    );
    assert_eq!(
        end_count(&ortho),
        2,
        "orthogonal must draw one end marker per transition (`basic` has 2 into `[*]`)"
    );
    let mut in_degree: HashMap<String, usize> = HashMap::new();
    for e in &ortho.edges {
        *in_degree.entry(e.to.clone()).or_insert(0) += 1;
    }
    for n in ortho.nodes.iter().filter(|n| n.shape == Glyph::StateEnd) {
        assert_eq!(
            in_degree.get(&n.id).copied().unwrap_or(0),
            1,
            "{}: a duplicated end marker must carry exactly one edge",
            n.id
        );
    }
}

/// S4: a choice draws as a 28x28 chamfered square under orthogonal routing, never the 40px
/// diamond splines draws — the same swap the flowchart's own `spec_of` makes for a decision
/// `Shape::Diamond` (`ChamferedRect`'s own doc: a diamond has no flat run for `evict` to spread
/// more than one port along).
#[test]
fn orthogonal_state_choice_is_a_28px_chamfered_square() {
    if !text_metrics::fonts_available() {
        return;
    }
    let src = "stateDiagram-v2\n  state c <<choice>>\n  A --> c\n  c --> B\n  c --> C";
    let splines = laid_out(src);
    let ortho = laid_out_orthogonal(src);
    assert_eq!(
        splines.node("c").expect("the choice").shape,
        Glyph::Choice,
        "splines must keep drawing a choice as the plain diamond glyph"
    );
    let c = ortho.node("c").expect("the choice");
    assert_eq!(
        c.shape,
        Glyph::ChamferedRect,
        "orthogonal must draw a choice as a chamfered rectangle, not a diamond"
    );
    assert_eq!(
        c.size,
        Size::new(28.0, 28.0),
        "orthogonal's choice must be the fixed 28x28 S4 size"
    );
}

// ---------------------------------------------------------------------------------------------
// 8. Looking at it
// ---------------------------------------------------------------------------------------------

/// Writes the corpus out as SVG files so a person can look at them. Not a check — §6 is explicit
/// that comparing pictures is for eyes, not for assertions.
///
/// `KONOMA_GALLERY=/some/dir cargo test -- --ignored state_tests::gallery`
#[test]
#[ignore = "writes SVG files for a person to look at: KONOMA_GALLERY=<dir> cargo test -- --ignored gallery"]
fn gallery() {
    let dir = std::env::var("KONOMA_GALLERY").expect("set KONOMA_GALLERY to a directory");
    std::fs::create_dir_all(&dir).expect("create the gallery directory");
    for (name, src) in CASES {
        let svg = render(src, "dark").unwrap_or_else(|e| panic!("{name}: {e}"));
        std::fs::write(format!("{dir}/{name}.svg"), svg).expect("write the SVG");
    }
}
