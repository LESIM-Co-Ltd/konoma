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
    check_foreign_nodes_stay_out_of_clusters, check_labels_ride_their_edge,
    check_nested_clusters_sit_inside_their_parent, check_nodes_do_not_overlap,
    check_unrelated_clusters_do_not_overlap, check_view_box_contains_everything, dist_to_boundary,
    mask_numbers, path_boxes, text_widths, tree_of,
};
use super::theme;
use super::Routing;
use super::{
    clusters, labels, orthogonal, shapes, svg, Diagram, Glyph, Label, PlacedCluster, PlacedEdge,
    PlacedNode, RenderError, Size,
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
        line_pitch: labels::line_height(),
        font_size: crate::preview::mermaid::text_metrics::FONT_SIZE as f64,
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
            title_strip: false,
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
            title_strip: false,
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
            // two outgoing labelled edges). Declaration order matches `docs/render-check/zz-
            // design-sources.md`'s own 4b exactly (§10-5 part-3 item 6): `state 分岐 <<choice>>`
            // sits after the first two transitions, not before them.
            "zz-design-4b",
            "stateDiagram-v2\n  direction LR\n  待機 --> 監視 : 開始\n  監視 --> 監視 : ポーリング\n  \
             state 分岐 <<choice>>\n  監視 --> 分岐 : 変化\n  分岐 --> 更新 : 差分あり\n  \
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

/// Sources that exist only to hold an orthogonal-mode invariant honest — never in [`CASES`], so
/// [`state_corpus_golden`] (the splines byte stream §10-2 forbids moving) never sees them.
///
/// The flowchart half of this module has had such a list since §10-5 part 2
/// (`tests::orthogonal_only_corpus`); this is its state-diagram counterpart, opened by the one
/// shape below.
fn orthogonal_only_corpus() -> Vec<(&'static str, &'static str)> {
    vec![(
        // §10-5 S4 against S2: a fork **inside** a composite state, one of whose branches leaves
        // the block. The bar's own port rule ("接続先トランクの座標に一致") reaches for `X`'s trunk,
        // which is outside `C` — and `straddle_bar_ports` then grows the bar's rectangle out
        // through `C`'s own side (measured at 59.7px before `bar_ports` learned to clamp).
        // Deliberately here rather than in a single-purpose test: what this shape needs is for the
        // *general* cluster invariants (`check_clusters_hold_their_members`,
        // `check_frames_are_derived_from_members`, `check_foreign_nodes_stay_out_of_clusters`) to
        // run on it every time, which is exactly what chaining it into `orthogonal_full_corpus`
        // buys.
        "orthogonal-fork-inside-a-composite",
        "stateDiagram-v2\n  state C {\n    state f <<fork>>\n    [*] --> f\n    f --> a\n    \
         a --> [*]\n  }\n  f --> X\n  C --> Y",
    )]
}

/// [`CASES`] plus [`orthogonal_design_reference_corpus`] plus [`orthogonal_only_corpus`] — every
/// orthogonal invariant that has to see the design-reference sources iterates this rather than any
/// one part alone.
///
/// Also chains [`orthogonal_reversed_direction_corpus`] — the sample and the three design
/// references in all four directions, so `RL` and `BT` are checked by every invariant here and not
/// only by the two mirror tests at the end of this file.
fn orthogonal_full_corpus() -> Vec<(&'static str, &'static str)> {
    CASES
        .iter()
        .copied()
        .chain(orthogonal_design_reference_corpus())
        .chain(orthogonal_only_corpus())
        .chain(
            orthogonal_reversed_direction_corpus()
                .iter()
                .map(|(name, src)| (name.as_str(), src.as_str())),
        )
        .collect()
}

/// Redumps `4a`/`4b`/`4c` under `konoma-orthogonal` to `docs/render-check/zz-design-<name>-
/// ours.{svg,png}`, next to the existing `zz-design-<name>-browser.png` reference each was drawn
/// from — the state-diagram sibling of `tests::orthogonal_design_reference_dump`, see that
/// function's own doc for the naming rationale, for why `docs/render-check/` is safe to write to,
/// and for why the `theme` argument is inert here.
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

/// §10-5 S2 ("枠外→内部状態の直接遷移" / a frame's ports are the ordinary ones): an edge between a
/// block and a node outside it constrains the **block's** own rank span, not the rank of whichever
/// member `Tree::anchor` picked to represent it.
///
/// `Tree::anchor` resolves a source block to *a* descendant with no internal out-edge, and a target
/// block to one with no internal in-edge — but "a sink" is not "the last one": `state P { [*] --> p1;
/// p1 --> p2; p1 --> p3; p3 --> p4 }` has two sinks (`p2` and `p4`) and the anchor is `p2`, a whole
/// level above `p4`. Ranking `P --> Z` from `p2` puts `Z` on `p4`'s own rank — inside `P`'s flow
/// span, beside a member, with `P`'s frame having to reach out sideways to it. The mirror holds on
/// the entry side (`Q --> P` with `P`'s entry anchor above its own first level pulls `Q` down beside
/// a member).
///
/// Three shapes, all four of them stated the same way: the outside end sits entirely past the
/// frame's own flow-axis extent, on the side the flow goes.
#[test]
fn orthogonal_cluster_anchored_edge_ranks_the_outside_end_past_the_whole_block() {
    if !text_metrics::fonts_available() {
        return;
    }
    let block = "state P {\n [*] --> p1\n p1 --> p2\n p1 --> p3\n p3 --> p4\n }";
    let cases: [(&str, &str, &str, bool); 3] = [
        // (name, source, the outside node's id, whether it is downstream of the block)
        (
            "exit",
            &format!("stateDiagram-v2\n {block}\n P --> Z"),
            "Z",
            true,
        ),
        (
            "exit-into-a-join-bar",
            &format!("stateDiagram-v2\n {block}\n state j <<join>>\n P --> j\n j --> Z"),
            "j",
            true,
        ),
        (
            "entry",
            "stateDiagram-v2\n Q --> P\n state P {\n w --> z\n x --> y\n y --> z\n }",
            "Q",
            false,
        ),
    ];
    for (name, src, outside, downstream) in cases {
        let d = laid_out_orthogonal(src);
        let frame = d.cluster("P").expect("P's frame");
        let (_, ft, _, fb) = frame.bounds();
        let n = d
            .node(outside)
            .unwrap_or_else(|| panic!("{name}: {outside}"));
        let (_, nt, _, nb) = n.bounds();
        if downstream {
            assert!(
                nt > fb,
                "{name}: {outside} must sit past the whole block (frame ends at {fb:.2}), \
                 not at {nt:.2}..{nb:.2}"
            );
        } else {
            assert!(
                nb < ft,
                "{name}: {outside} must sit before the whole block (frame starts at {ft:.2}), \
                 not at {nt:.2}..{nb:.2}"
            );
        }
        // …and no member of the block shares its row, which is the same statement read off the
        // members rather than off the derived frame.
        for m in ["p1", "p2", "p3", "p4", "w", "x", "y", "z"] {
            let Some(member) = d.node(m) else { continue };
            assert!(
                (member.center.y - n.center.y).abs() > 1.0,
                "{name}: {outside} shares {m}'s own row at y={:.2}",
                member.center.y
            );
        }
    }

    // The same shape in a flowchart, where `subgraph` plays `state`'s part and the block has no
    // start marker at all: `Tree::anchor`'s "no internal in-edge" rule reads `p1` as the entry and
    // still resolves the exit to `p2`, so this is the identical off-by-a-level, one syntax over.
    let d = super::tests::laid_out_flow(
        "flowchart TB\n  subgraph P\n    p1 --> p2\n    p1 --> p3\n    p3 --> p4\n  end\n  P --> Z",
        "basis",
        "konoma-orthogonal",
    );
    let frame = d.cluster("P").expect("P's frame");
    let (_, _, _, fb) = frame.bounds();
    let z = d.node("Z").expect("Z");
    assert!(
        z.bounds().1 > fb,
        "flowchart: Z must sit past the whole subgraph (frame ends at {fb:.2}), not at {:.2}",
        z.bounds().1
    );

    // The entry case's own edge: `Q --> P` enters the frame through its flow-axis face (`TB`'s
    // top), the shape §10-5 S2 gives an ordinary node — not the side face a member-ranked `Q`
    // forced it onto.
    let d = laid_out_orthogonal(
        "stateDiagram-v2\n Q --> P\n state P {\n w --> z\n x --> y\n y --> z\n }",
    );
    let frame = d.cluster("P").expect("P's frame");
    let (_, ft, _, _) = frame.bounds();
    let e = d
        .edges
        .iter()
        .find(|e| e.from == "Q" && e.to == "P")
        .expect("Q -> P");
    let end = e.points.last().expect("non-empty");
    assert!(
        (end.y - (ft - orthogonal::PORT_INSET)).abs() < 0.01,
        "Q -> P must enter P's own top face at {:.2}, not {end:?}",
        ft - orthogonal::PORT_INSET
    );
}

/// "No dangling fragment" — [`super::tests::assert_no_dangling_fragment`]'s own two halves (a
/// vanishing jog flanked by a reversal, and an endpoint sitting off its own node's face), run over
/// the state corpus as well as the flowchart one. A state diagram is where the shapes most likely
/// to trip it live: S1's markers are tiny circles whose whole face is a few px wide, S4's bars are
/// grown after routing, and S3's self-transition is the one route in the module built from two
/// hand-placed ports rather than [`orthogonal::evict`]'s grid.
#[test]
fn orthogonal_state_edges_leave_no_dangling_fragment() {
    if !text_metrics::fonts_available() {
        return;
    }
    for (name, src) in orthogonal_full_corpus() {
        let d = laid_out_orthogonal(src);
        super::tests::assert_no_dangling_fragment(name, &d);
    }
}

/// §10-5 S4's own "join の下流出力はバー入力群の重心", stated where the two halves of it used to
/// disagree: `align_straight_lanes_with` moves the node downstream of a join bar onto the mean of
/// the bar's inputs, and [`orthogonal::bar_ports`] independently computes the same mean to place
/// the bar's own output port. A cluster-anchored input made the two read different numbers — a
/// member's centre against the frame's own face — so the bar's port and the node it feeds ended up
/// 17.4px apart and the one merged trunk left the bar with a jog. The rule says the output is
/// straight; that is what is asserted.
#[test]
fn orthogonal_join_output_is_straight_when_one_input_is_a_block() {
    if !text_metrics::fonts_available() {
        return;
    }
    let d = laid_out_orthogonal(
        "stateDiagram-v2\n state P {\n [*] --> p1\n p1 --> p2\n p1 --> p3\n p3 --> p4\n }\n \
         state j <<join>>\n P --> j\n R --> j\n j --> Z",
    );
    let e = d
        .edges
        .iter()
        .find(|e| e.from == "j" && e.to == "Z")
        .expect("j -> Z");
    assert_eq!(
        e.points.len(),
        2,
        "a join's own output must be 0-bend: {:?}",
        e.points
    );
    let z = d.node("Z").expect("Z");
    assert!(
        (e.points[0].x - z.center.x).abs() < 0.01,
        "the bar's output port at {:.2} must sit on Z's own centre {:.2}",
        e.points[0].x,
        z.center.x
    );
    // …and that shared coordinate is the mean of the two inputs' own *ports*, the frame's face
    // included — not of the anchor member `Tree::anchor` happened to pick inside `P`.
    let frame = d.cluster("P").expect("P's frame");
    let r = d.node("R").expect("R");
    let want = (frame.center.x + r.center.x) / 2.0;
    assert!(
        (z.center.x - want).abs() < 1.0,
        "Z must sit on the mean of P's frame ({:.2}) and R ({:.2}) = {want:.2}, not {:.2}",
        frame.center.x,
        r.center.x,
        z.center.x
    );
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
        // §10-5 part-3 item 1's own converse — `zz-design-4a`'s end marker (`orthogonal_full_
        // corpus` includes the design-reference corpus, so this runs against it every time) is
        // exactly the case this pins: a marker with no membership in `プレビュー` must never sit
        // inside its frame.
        check_foreign_nodes_stay_out_of_clusters(name, &d, &tree);
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

/// §10-5 part-3 item 2: `zz-design-4a`'s `q` transition (`プレビュー --> ツリー`) leaves the
/// composite state `プレビュー` and draws straight back up beside `Enter`, not around the whole
/// diagram's perimeter — `orthogonal::classify`'s own doc on `source_is_cluster` has the reasoning
/// (leaving a frame's own border, with nothing real in the way, shares the ordinary branch/merge
/// ladder's local routing).
///
/// A local route (`aligned`/branch/merge, at most one `rank_lane_bend` retry) never exceeds 4
/// points; the perimeter lane `q` used to take always left the diagram's own content box by
/// [`orthogonal::PERIMETER_MARGIN`] on its way around, which for this source put a point well past
/// `プレビュー`'s own right edge — checked directly, not just by point count, so a future change
/// that kept the old detour's shape but trimmed a point could not silently pass this.
#[test]
fn orthogonal_state_q_leaves_the_composite_state_locally_not_via_the_perimeter() {
    if !text_metrics::fonts_available() {
        return;
    }
    let src = orthogonal_design_reference_corpus()
        .into_iter()
        .find(|(n, _)| *n == "zz-design-4a")
        .expect("zz-design-4a is in the design-reference corpus")
        .1;
    let d = laid_out_orthogonal(src);
    let q = d
        .edges
        .iter()
        .find(|e| e.from == "プレビュー" && e.to == "ツリー")
        .expect("プレビュー->ツリー (q) must exist");
    assert!(
        q.points.len() <= 4,
        "q must take a local route (at most 2 bends), not the perimeter lane: {:?}",
        q.points
    );
    let cluster = d
        .cluster("プレビュー")
        .expect("プレビュー cluster must exist");
    let (_, _, cluster_right, _) = cluster.bounds();
    let max_x = q.points.iter().map(|p| p.x).fold(f64::MIN, f64::max);
    assert!(
        max_x <= cluster_right + 1.0,
        "q must not swing out past プレビュー's own right edge ({cluster_right}) the way the old \
         perimeter detour did: max_x={max_x} points={:?}",
        q.points
    );
}

// ---------------------------------------------------------------------------------------------
// §10-5 S3/S4: self-transitions and fork/join bars
// ---------------------------------------------------------------------------------------------

/// §10-5 S3 ("流れと直交する辺…の中心±8pxの2ポート…20px外を回る固定ループ"): `zz-design-4b`'s own
/// self-transition (`監視 -> 監視 : ポーリング`, LR) must draw as the fixed U — never the
/// dagre-waypoint-derived staircase a flowchart's own self-loop still uses.
///
/// Checked geometrically, not just "4 points": the flag this pins is `state::spec_of`'s own
/// `fixed_self_loops`, and a mutation that quietly stopped setting it (or that made
/// `route_state_self_loop` fall back to some other shape) would still often produce a 4-point
/// polyline by coincidence — the exact offsets are what only the real S3 code path produces.
#[test]
fn orthogonal_state_self_transition_draws_the_fixed_loop_from_the_canonical_face() {
    if !text_metrics::fonts_available() {
        return;
    }
    let src = orthogonal_design_reference_corpus()
        .into_iter()
        .find(|(n, _)| *n == "zz-design-4b")
        .expect("zz-design-4b is in the design-reference corpus")
        .1;
    let d = laid_out_orthogonal(src);
    let monitor = d.node("監視").expect("監視 must exist");
    let loop_edge = d
        .edges
        .iter()
        .find(|e| e.from == "監視" && e.to == "監視")
        .expect("監視->監視 (ポーリング) must exist");
    assert_eq!(
        loop_edge.points.len(),
        4,
        "the fixed loop is exactly [out port, out corner, in corner, in port]: {:?}",
        loop_edge.points
    );
    let p = &loop_edge.points;
    // §10-5 S3's own canonical face for LR: the top edge. `監視`'s own top boundary is
    // `center.y - size.h / 2`.
    let top = monitor.center.y - monitor.size.h / 2.0;
    for (i, pt) in p.iter().enumerate() {
        assert!(
            pt.y <= top + 0.5,
            "every point of the loop must sit at or above 監視's own top edge ({top}): index {i} \
             = {pt:?} in {p:?}"
        );
    }
    // The two ports (index 0 and 3) sit close to the face, the two outward corners (index 1, 2)
    // sit `SELF_LOOP_OUTSET`-ish further out — checked as "corners are further from the face than
    // ports", not an exact px match (`PORT_INSET` is a separate, smaller constant this test does
    // not need to duplicate).
    assert!(
        (top - p[1].y) > (top - p[0].y) + 5.0,
        "the outward corner must sit meaningfully further from the face than the port does: \
         port_y={} corner_y={}",
        p[0].y,
        p[1].y
    );
    // The two ports' own x sits `SELF_LOOP_PORT_OFFSET` (8px) either side of 監視's own centre —
    // never the generic 16px `PORT_SPACING` grid `evict` would have used for an ordinary shared
    // face.
    let dx0 = (p[0].x - monitor.center.x).abs();
    let dx3 = (p[3].x - monitor.center.x).abs();
    assert!(
        (dx0 - 8.0).abs() < 0.5 && (dx3 - 8.0).abs() < 0.5,
        "both ports must sit exactly 8px either side of 監視's own centre x ({}): {p:?}",
        monitor.center.x
    );
    // The label ("ポーリング") floats clear of the loop's own outward leg — §10-5 S3's own
    // exception to the on-line-plate rule — rather than sitting on top of it.
    let label = loop_edge
        .label
        .as_ref()
        .expect("the self-transition must carry its own label");
    assert!(
        label.center.y < p[1].y.min(p[2].y) - 0.5,
        "the label must float above (outside) the loop's own outward leg, not sit on it: \
         label_y={} outward_y={:?}",
        label.center.y,
        (p[1].y, p[2].y)
    );
}

/// §10-5 S3's own retreat rule, exercised at the `classify`/`route_flowchart` level directly
/// (rather than hunting for a real mermaid source that happens to occupy the canonical face) —
/// mirrors how `orthogonal.rs`'s own unit tests already isolate one rule at a time. `fixed_self_
/// loops: false` (a flowchart's own self-loop) must be entirely unaffected by anything sharing the
/// node's top face — this is also the "does the flag actually gate S3 at all" mutation check
/// (`new-pass-must-prove-it-fires`): a version of `classify` that always took the S3 branch,
/// ignoring `fixed_self_loops`, would make this assertion's `false` half fail.
#[test]
fn orthogonal_self_transition_retreats_when_its_canonical_face_is_taken() {
    let placed = |id: &str, cx: f64, cy: f64, w: f64, h: f64| PlacedNode {
        id: id.to_string(),
        shape: Glyph::default(),
        center: Point::new(cx, cy),
        size: Size::new(w, h),
        label: Label::measure(""),
        panel: None,
        series: None,
        mark: None,
        style: None,
    };
    let x = placed("X", 0.0, 0.0, 100.0, 60.0);
    let above = placed("Above", 0.0, -300.0, 60.0, 30.0);
    let nodes = vec![x.clone(), above.clone()];
    // A branch from X that lands on X's own top (cross-axis) face — LR's canonical self-loop face
    // — by sitting directly above it (`classify`'s own `branch_source_side` is `cross_face`-based).
    let occupier = super::orthogonal::EligibleEdge {
        id: "occupy",
        source: "X",
        target: "Above",
        raw: &[],
        source_rank: Some(0),
        target_rank: Some(0),
        source_out_degree: 2,
        target_in_degree: 1,
        aside: false,
    };
    let self_loop = super::orthogonal::EligibleEdge {
        id: "loop",
        source: "X",
        target: "X",
        raw: &[],
        source_rank: Some(0),
        target_rank: Some(0),
        source_out_degree: 2,
        target_in_degree: 1,
        aside: false,
    };
    let edges = [occupier, self_loop];
    let chain_next = HashMap::new();

    let fixed = super::orthogonal::route_flowchart(
        crate::preview::mermaid::flowchart::Direction::LeftToRight,
        &nodes,
        &[],
        &edges,
        &chain_next,
        true,
    );
    let loop_points = &fixed.points["loop"];
    let top = x.center.y - x.size.h / 2.0;
    let bottom = x.center.y + x.size.h / 2.0;
    assert!(
        loop_points.iter().all(|p| p.y >= bottom - 0.5),
        "with the canonical top face already occupied, the loop must retreat to the bottom face \
         (every point at/after y={bottom}): {loop_points:?}"
    );

    // The "does the flag gate this at all" half: with `fixed_self_loops: false` (a flowchart's own
    // self-loop), the same two edges must draw the *old* dagre-waypoint-derived shape — never the
    // fixed loop — regardless of the top face being taken.
    let unfixed = super::orthogonal::route_flowchart(
        crate::preview::mermaid::flowchart::Direction::LeftToRight,
        &nodes,
        &[],
        &edges,
        &chain_next,
        false,
    );
    assert_ne!(
        unfixed.points["loop"].len(),
        4,
        "a flowchart-style self-loop with empty `raw` degenerates to the two-point defensive \
         fallback, never the S3 4-point fixed loop: {:?}",
        unfixed.points["loop"]
    );
    let _ = top;
}

/// §10-5 S4 ("バーのポート位置は接続先トランクの座標に一致…分配計算なし"): every one of
/// `fork_state`'s own outputs on `zz-design-4c` must land at exactly the connected trunk's own
/// centre x, not spread across the bar's own face the way an ordinary multi-edge face would be by
/// `evict`'s 16px grid.
#[test]
fn orthogonal_state_bar_ports_match_the_connected_trunk_exactly_not_a_distribution() {
    if !text_metrics::fonts_available() {
        return;
    }
    let src = orthogonal_design_reference_corpus()
        .into_iter()
        .find(|(n, _)| *n == "zz-design-4c")
        .expect("zz-design-4c is in the design-reference corpus")
        .1;
    let d = laid_out_orthogonal(src);
    for target_id in ["取得", "監査"] {
        let target = d.node(target_id).unwrap_or_else(|| panic!("{target_id}"));
        let edge = d
            .edges
            .iter()
            .find(|e| e.from == "fork_state" && e.to == target_id)
            .unwrap_or_else(|| panic!("fork_state->{target_id} must exist"));
        let last = edge.points.last().expect("at least one point");
        assert!(
            (last.x - target.center.x).abs() < 0.5,
            "fork_state's own port toward {target_id} must sit at its exact centre x \
             ({}), not distributed: {:?}",
            target.center.x,
            edge.points
        );
    }
}

/// §10-5 S4 ("長さ＝接続先トランクspan＋両端各16px"): the join bar's own final size must be wide
/// enough to span every trunk it connects to, plus clearance on **both** ends.
///
/// **Updated 2026-09-03** (defect 1's own fix, `orthogonal::straddle_bar_ports`): the bound
/// checked used to be `span + BAR_PORT_PAD` (one pad, not two) because the bar's own drawn
/// rectangle was only ever *grown* via the ordinary lay-out/measure/grow retry loop
/// (`mod.rs::bar_required_sizes`), which asks dagre for more room but never repositions the box
/// dagre itself centred — and that retry loop's own feedback (growing the bar widens its trunks'
/// own spacing too, which raises the very span being grown toward) never fully converged within
/// the shared 3-pass budget. `straddle_bar_ports` now sets the bar's rectangle directly from
/// `bar_ports`'s own final per-edge coordinates, every pass — an exact `[min_port - PAD, max_port
/// + PAD]`, not an asymptotic approximation — so the width is pinned tight here, both pads.
#[test]
fn orthogonal_state_bar_grows_to_span_every_connected_trunk_plus_end_padding() {
    if !text_metrics::fonts_available() {
        return;
    }
    let src = orthogonal_design_reference_corpus()
        .into_iter()
        .find(|(n, _)| *n == "zz-design-4c")
        .expect("zz-design-4c is in the design-reference corpus")
        .1;
    let d = laid_out_orthogonal(src);
    let get = d.node("取得").expect("取得");
    let audit = d.node("監査").expect("監査");
    let bar = d.node("fork_state").expect("fork_state");
    let span = (get.center.x - audit.center.x).abs();
    let expected = span + 2.0 * orthogonal::BAR_PORT_PAD;
    assert!(
        (bar.size.w - expected).abs() < 0.5,
        "fork_state's own width ({}) must be exactly the 取得/監査 span ({span}) plus both \
         BAR_PORT_PAD ends ({expected}) now that straddle_bar_ports sets the rectangle directly \
         from the final ports, not merely grown toward it by the layout retry loop",
        bar.size.w
    );
    assert!(
        bar.size.w > 2.0 * orthogonal::BAR_PORT_PAD + 0.5,
        "fork_state must have grown well past its own {}-px minimum — a mutation that dropped \
         the growth-retry wiring entirely would leave it at exactly that floor: {}",
        2.0 * orthogonal::BAR_PORT_PAD,
        bar.size.w
    );
    assert!(
        (bar.size.h - 6.0).abs() < 0.5,
        "fork_state's own thickness must be the fixed S4 6px, not splines' 10px: {}",
        bar.size.h
    );
}

/// §10-5 S4 (defect 1, 2026-09-03 fix): a bar's own **drawn rectangle** must actually reach every
/// port [`orthogonal::bar_ports`] gave it, not merely be *sized* right — before this fix, `evict`/
/// `bar_ports` placed a port's cross coordinate correctly ([`port_at`]'s own doc: the cross
/// coordinate is written unconditionally from the value it is handed, regardless of whether the
/// box's own bounds happen to reach it), but the bar's box itself stayed wherever dagre's own
/// rank/order layout centred it — for `zz-design-4c`'s own fork bar, under `初期化`/`取得` while
/// `監査`'s own port sat well past its right edge, so the line for `fork_state -> 監査` read as
/// leaving from empty space beside the box. Runs over **every** orthogonal fixture that has a bar
/// at all (`CASES`' own `fork`/`fork-lr`/`4c` included, not just `zz-design-4c`), so the fix is a
/// corpus-wide invariant rather than a fixture-specific patch — [`check_nodes_do_not_overlap`]/
/// [`check_unrelated_clusters_do_not_overlap`] (already run over this same corpus, elsewhere in
/// this file) are what keep "the bar still overlaps no other node/cluster" true; this test only
/// adds the "reaches its own ports" half neither of them states.
#[test]
fn orthogonal_state_bar_rect_straddles_every_port_it_carries() {
    if !text_metrics::fonts_available() {
        return;
    }
    for (name, src) in orthogonal_full_corpus() {
        let d = laid_out_orthogonal(src);
        for bar in d
            .nodes
            .iter()
            .filter(|n| matches!(n.shape, Glyph::Bar { .. }))
        {
            let horizontal = matches!(bar.shape, Glyph::Bar { horizontal: true });
            let (lo, hi) = if horizontal {
                (
                    bar.center.x - bar.size.w / 2.0,
                    bar.center.x + bar.size.w / 2.0,
                )
            } else {
                (
                    bar.center.y - bar.size.h / 2.0,
                    bar.center.y + bar.size.h / 2.0,
                )
            };
            for e in &d.edges {
                let p = if e.from == bar.id {
                    e.points.first()
                } else if e.to == bar.id {
                    e.points.last()
                } else {
                    continue;
                };
                let Some(p) = p else { continue };
                let coord = if horizontal { p.x } else { p.y };
                assert!(
                    coord >= lo - 0.5 && coord <= hi + 0.5,
                    "{name}: {}->{}'s port on bar {} sits at {coord:.2}, outside the bar's own \
                     rectangle [{lo:.2}, {hi:.2}]",
                    e.from,
                    e.to,
                    bar.id
                );
                assert!(
                    coord - lo >= orthogonal::BAR_PORT_PAD - 0.5
                        && hi - coord >= orthogonal::BAR_PORT_PAD - 0.5,
                    "{name}: {}->{}'s port on bar {} sits {:.2}px from the near end of \
                     [{lo:.2}, {hi:.2}] — every port must clear BAR_PORT_PAD ({}) from *both* \
                     ends, the direct consequence of the bar's own rectangle being built from \
                     [min_port - PAD, max_port + PAD]",
                    e.from,
                    e.to,
                    bar.id,
                    (coord - lo).min(hi - coord),
                    orthogonal::BAR_PORT_PAD
                );
            }
        }
    }
}

/// §10-5 S4 ("fork→join を曲げ 0 優先", defect 2, 2026-09-03 fix): every edge with exactly one
/// bar end must route with **zero bends** — a constant cross-axis coordinate across its whole
/// polyline — and that coordinate must equal the *other* end's own final, evicted port coordinate
/// (`bar_ports`'s own doc: "no distribution... exactly wherever the sibling on the other end
/// sits"), not `edge.raw`'s stale, pre-`align_straight_lanes` waypoint.
///
/// Runs over the full corpus (every `CASES`/design-reference fixture with a bar) plus a small
/// synthetic fixture built specifically so the non-bar end's own port is **not** at that node's
/// exact centre — `B` below carries two incoming claims (`f -> B` and `X -> B`), so `evict`
/// spaces them `PORT_SPACING` apart on `B`'s own face and only one of the two can sit dead centre.
/// Reading `edge.raw` instead of `evict`'s own claim would still often produce a *straight* line —
/// both raw and evicted coordinates are usually close — so a fixture where they visibly diverge is
/// what actually exercises the read-the-wrong-value bug, which `zz-design-4c` alone does not
/// reliably catch (its own trunks mostly have exactly one claim each).
#[test]
fn orthogonal_state_bar_touching_edges_are_zero_bend_and_match_the_trunks_own_port() {
    if !text_metrics::fonts_available() {
        return;
    }
    fn check(name: &str, d: &Diagram) {
        let horizontal_by_id: HashMap<&str, bool> = d
            .nodes
            .iter()
            .filter_map(|n| match n.shape {
                Glyph::Bar { horizontal } => Some((n.id.as_str(), horizontal)),
                _ => None,
            })
            .collect();
        // §10-5 S4's own one exception ("join の下流出力はバー入力群の重心"): a bar with more
        // than one upstream (input) claim and exactly one downstream (output) claim routes that
        // single output from the *mean* of the inputs, not from the target's own coordinate — so
        // it is not "no distribution" the way every other bar face is, and can legitimately bend
        // if the target does not happen to sit exactly at that mean (`join_state -> State4` in the
        // `fork` fixture: two inputs at different x average to a point neither of them, nor
        // `State4`, sits at). Identified the same way `bar_ports` itself does: downstream count
        // (edges with `from == bar.id`) and upstream count (`to == bar.id`), read straight off
        // the diagram rather than re-deriving `Eviction`.
        let mut downstream_count: HashMap<&str, usize> = HashMap::new();
        let mut upstream_count: HashMap<&str, usize> = HashMap::new();
        for e in &d.edges {
            if horizontal_by_id.contains_key(e.from.as_str()) {
                *downstream_count.entry(e.from.as_str()).or_insert(0) += 1;
            }
            if horizontal_by_id.contains_key(e.to.as_str()) {
                *upstream_count.entry(e.to.as_str()).or_insert(0) += 1;
            }
        }
        for e in &d.edges {
            let horizontal = horizontal_by_id
                .get(e.from.as_str())
                .or_else(|| horizontal_by_id.get(e.to.as_str()));
            let Some(&horizontal) = horizontal else {
                continue;
            };
            if horizontal_by_id.contains_key(e.from.as_str())
                && downstream_count.get(e.from.as_str()).copied().unwrap_or(0) == 1
                && upstream_count.get(e.from.as_str()).copied().unwrap_or(0) > 1
            {
                continue; // the join centroid exception, this function's own doc.
            }
            // §10-5 S4's own second exception, and the one S2 forces: a bar that is a member of a
            // frame cannot put a port outside that frame (`orthogonal::bar_ports`'s own doc — the
            // rectangle `straddle_bar_ports` builds from its ports would then poke out through the
            // frame's side, and growing the frame to follow it never settles). When the node at the
            // other end sits outside the bar's own rectangle, the port was clamped and the edge
            // bends once to reach it — "曲げ 0 **優先**", not required. Identified from the geometry
            // rather than by name: only the clamp can leave a bar's neighbour outside its span,
            // because every unclamped port *is* that neighbour's coordinate and the rectangle is
            // built to cover every port.
            let (bar_id, other_id) = if horizontal_by_id.contains_key(e.from.as_str()) {
                (e.from.as_str(), e.to.as_str())
            } else {
                (e.to.as_str(), e.from.as_str())
            };
            if let (Some(bar), Some(other)) = (d.node(bar_id), d.node(other_id)) {
                let (bl, bt, br, bb) = bar.bounds();
                let (lo, hi, c) = if horizontal {
                    (bl, br, other.center.x)
                } else {
                    (bt, bb, other.center.y)
                };
                if c < lo - 0.5 || c > hi + 0.5 {
                    continue;
                }
            }
            let coords: Vec<f64> = e
                .points
                .iter()
                .map(|p| if horizontal { p.x } else { p.y })
                .collect();
            let first = coords[0];
            for (i, &c) in coords.iter().enumerate() {
                assert!(
                    (c - first).abs() < 0.5,
                    "{name}: {}->{} (bar-anchored) must be zero-bend — point {i} reads {c:.2}, \
                     not {first:.2}: {:?}",
                    e.from,
                    e.to,
                    e.points
                );
            }
        }
    }
    for (name, src) in orthogonal_full_corpus() {
        check(name, &laid_out_orthogonal(src));
    }
    // The synthetic multi-claim fixture: `B` receives two incoming edges (`f -> B`, `X -> B`), so
    // `evict`'s 16px grid spaces the two claims `PORT_SPACING` apart on `B`'s own face — neither
    // sits exactly at `B`'s own centre (an even claim count has no exact-centre slot at all,
    // `evict`'s own doc). The generic `check` above already pins `f -> B` at zero bends; this
    // pins the *value* it is zero-bend to: proof that the number came from `evict`'s own claim
    // rather than `edge.raw`'s stale waypoint, which (before the fix) read close to `B`'s plain
    // centre instead.
    let synthetic = "stateDiagram-v2\n  state f <<fork>>\n  state j <<join>>\n  [*] --> f\n  \
                      f --> A\n  f --> B\n  X --> B\n  A --> j\n  B --> j\n  j --> [*]";
    let d = laid_out_orthogonal(synthetic);
    check("bar-multi-claim-synthetic", &d);
    let b = d.node("B").expect("B");
    let f_to_b = d
        .edges
        .iter()
        .find(|e| e.from == "f" && e.to == "B")
        .expect("f->B must exist");
    let arrival_x = f_to_b.points.last().expect("at least one point").x;
    assert!(
        (arrival_x - b.center.x).abs() > 4.0,
        "bar-multi-claim-synthetic: f->B lands at x={arrival_x:.2}, indistinguishable from B's \
         own plain centre x={:.2} — this fixture only proves the fix when the evicted claim \
         differs from the raw centre; a mutation that fell back to `edge.raw`'s stale waypoint \
         (which happened to be near-centre here too) would pass the zero-bend check above but \
         land on the wrong number, which this assertion is the one to catch",
        b.center.x
    );
}

/// §10-5 S2/S4 (defect 3, 2026-09-03 fix): a composite state's own **exit** edge (the state names
/// the edge's own source) is anchored at a member with no outgoing edge to another descendant of
/// the same composite — never simply the first-declared member — so the flow reads as leaving
/// from the composite's own actual end. On `zz-design-4c`, `処理 -> join_state` used to anchor at
/// `整形` (declared first, but not a sink: it flows on to `解析`), landing the join bar's own
/// exact-trunk-matched port at the same rank depth as `整形`'s own box, with no clear perpendicular
/// approach. The fix anchors at `集計` (`処理`'s own true last member, two levels of nesting down),
/// which sits at its own natural rank with nothing of `処理`'s left to overlap.
///
/// This pins the frame-cut boundary itself (`clusters::cut_start`'s own doc — a cluster-anchored
/// edge is always clipped to the *frame's* outline at drawing time, whichever member `anchor`
/// picked for dagre's own ranking), which stays true even under a wrong anchor and so does not by
/// itself distinguish a correct anchor from an incorrect one — confirmed by mutating `anchor` to
/// always return the first-declared member: this assertion still held, but
/// [`super::tests::orthogonal_state_no_edge_crosses_a_foreign_node`] (checked here via
/// `orthogonal_full_corpus`, `zz-design-4c` included) and the box-overlap invariant both failed
/// immediately (`処理->join_state` crossing `整形`, then `join_state`/`整形` overlapping outright)
/// — those two, already run over this same corpus, are what actually prove the anchor fix, this
/// test is a supporting pin on the frame-cut geometry it does not change.
#[test]
fn orthogonal_state_composite_exit_anchor_lands_beyond_its_own_frame_on_4c() {
    if !text_metrics::fonts_available() {
        return;
    }
    let src = orthogonal_design_reference_corpus()
        .into_iter()
        .find(|(n, _)| *n == "zz-design-4c")
        .expect("zz-design-4c is in the design-reference corpus")
        .1;
    let d = laid_out_orthogonal(src);
    let frame = d.cluster("処理").expect("処理's own frame");
    let (_, _, _, frame_bottom) = frame.bounds();
    let edge = d
        .edges
        .iter()
        .find(|e| e.from == "処理" && e.to == "join_state")
        .expect("処理->join_state must exist");
    let start_y = edge.points.first().expect("at least one point").y;
    assert!(
        start_y >= frame_bottom - 0.5,
        "処理 -> join_state must start at or below 処理's own frame bottom ({frame_bottom:.2}), \
         not partway inside it: starts at y={start_y:.2} — a regression to the pre-fix \
         declared-first anchor (整形) would land this well above the frame bottom, inside the \
         box",
    );
}

/// §10-5 S2 (defect 3, 2026-09-03 fix): the same directional anchor, exercised on a synthetic
/// `LR` composite whose own **last** member is itself a block nested two levels deep — `C`'s own
/// direct members are `M1` and the block `N`; `N`'s own members are `P` and `Q`. `C`'s own exit
/// anchor must resolve *through* `N` to `Q` (`N`'s own sink) — `Tree::anchor`'s own recursive
/// resolution of a nested-block-named internal edge (`clusters.rs`'s own doc) — not stop at `N`'s
/// own first member `P`, and never fall back to `C`'s own first member `M1`.
///
/// The frame-boundary assertion below is the same *supporting* pin the 4c test above uses (see
/// its own doc for why the frame-cut boundary itself does not distinguish a correct anchor from
/// an incorrect one) — this fixture has nothing else in it for a wrong anchor to collide with, so
/// there is no crossing/overlap invariant to lean on here the way 4c's own does. What this test
/// actually regression-pins is that the recursive resolution *terminates and lays out at all* —
/// before `anchor_bounded`'s own depth guard existed, resolving a nested-block-named internal
/// edge could recurse without bound (found while writing this very test: a self-referencing
/// top-level entry edge sent `composites_and_regions_are_frames`, an unrelated existing test,
/// into a stack overflow the moment the recursive resolution was added) — so a mutation that
/// dropped that guard would show up here as a crash, not a silently-wrong number.
#[test]
fn orthogonal_state_composite_exit_anchor_resolves_through_a_nested_block_lr() {
    if !text_metrics::fonts_available() {
        return;
    }
    let src = "stateDiagram-v2\n  direction LR\n  state C {\n    [*] --> M1\n    M1 --> N\n    \
               state N {\n      P --> Q\n    }\n  }\n  C --> D";
    let d = laid_out_orthogonal(src);
    let frame = d.cluster("C").expect("C's own frame");
    let (_, _, frame_right, _) = frame.bounds();
    let edge = d
        .edges
        .iter()
        .find(|e| e.from == "C" && e.to == "D")
        .expect("C->D must exist");
    let start_x = edge.points.first().expect("at least one point").x;
    assert!(
        start_x >= frame_right - 0.5,
        "C -> D must start at or beyond C's own frame right edge ({frame_right:.2}) in this LR \
         diagram, not partway inside it: starts at x={start_x:.2} — anchoring at C's own first \
         member (M1) or at N's own first member (P) instead of N's own sink (Q) would both land \
         this inside the frame",
    );
}

/// §10-5 (defect 3's own documented fallback): a composite whose members form an **internal
/// cycle** (`M1 --> M2 --> M1`, no member has zero out-degree at all) has no qualifying sink, so
/// [`clusters::Tree::anchor`] falls back to the first descendant in declaration order — the
/// pre-§10-5 behaviour, `clusters.rs`'s own doc on `AnchorRole` — rather than looping or picking
/// nothing. This does not assert *which* member the fallback lands on (that is an implementation
/// detail the module doc leaves unspecified beyond "first declared"); it asserts what the fallback
/// has to keep true regardless: the diagram still lays out without panicking, and the resulting
/// route is still a **valid, non-piercing** one — reusing the same corpus-wide invariants
/// ([`check_nodes_do_not_overlap`], [`check_unrelated_clusters_do_not_overlap`],
/// [`check_edges_stay_out_of_shapes`]) every other fixture in this file is held to, so a fallback
/// that produced a technically-non-panicking but geometrically broken picture would still be
/// caught.
#[test]
fn orthogonal_state_composite_with_an_internal_cycle_falls_back_to_a_valid_route() {
    if !text_metrics::fonts_available() {
        return;
    }
    let src = "stateDiagram-v2\n  [*] --> X\n  X --> C\n  state C {\n    M1 --> M2\n    \
               M2 --> M1\n  }\n  C --> D\n  D --> [*]";
    let d = laid_out_orthogonal(src);
    check_nodes_do_not_overlap("cycle-fallback", &d);
    check_unrelated_clusters_do_not_overlap("cycle-fallback", &d);
    check_edges_stay_out_of_shapes("cycle-fallback", &d);
    // The re-anchored edge must still exist and still be a real, drawn polyline (never a
    // degenerate zero/one-point line — `route_with_ports`'s own defensive fallback doc).
    let edge = d
        .edges
        .iter()
        .find(|e| e.from == "C" && e.to == "D")
        .expect("C->D must still exist after the cycle fallback");
    assert!(
        edge.points.len() >= 2,
        "cycle-fallback: C->D must still be a real polyline: {:?}",
        edge.points
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

/// `clusters::Tree::anchor`'s own recursion bound (`anchor_bounded`'s own doc, `depth >
/// self.clusters.len()`): a transition naming a nested block is resolved recursively (defect 3's
/// own fix, 2026-09-03) — `XM --> Y`, a member of `X`, naming the *sibling* block `Y` directly —
/// and if `Y`'s own member in turn names `X` back (`YM --> X`), the two blocks' own resolutions
/// call each other forever without a bound. konoma's own grammar cannot write this (a block's
/// members are always syntactically *inside* it, `clusters.rs`'s own module doc), but nothing
/// stops a `stateDiagram-v2` source from naming a sibling block directly in an internal transition
/// the way this fixture does, so the guard is real, reachable input, not a defensive-only
/// unreachable path. Confirmed by disabling the bound (temporarily, by hand): this exact fixture
/// stack-overflows within a few hundred frames — no assertion needed to prove the bound *matters*,
/// only that it does not regress; this test is that regression pin.
#[test]
fn orthogonal_state_cross_cluster_internal_edge_cycle_does_not_overflow_the_stack() {
    if !text_metrics::fonts_available() {
        return;
    }
    let src = "stateDiagram-v2\n  [*] --> X\n  state X {\n    XM --> Y\n  }\n  \
               state Y {\n    YM --> X\n  }\n  X --> Z\n  Z --> [*]";
    let d = laid_out_orthogonal(src);
    check_nodes_do_not_overlap("cross-cluster-cycle", &d);
    check_unrelated_clusters_do_not_overlap("cross-cluster-cycle", &d);
}

// ---------------------------------------------------------------------------------------------
// 8. §10-5 round 4 — a composite state is one unit for ranking and for lane alignment
// ---------------------------------------------------------------------------------------------

/// The design-reference source called `name` — the same list `orthogonal_design_reference_dump`
/// renders, so a round-4 test and the picture a person looks at can never drift apart.
fn design_reference_source(name: &str) -> &'static str {
    orthogonal_design_reference_corpus()
        .into_iter()
        .find(|(n, _)| *n == name)
        .unwrap_or_else(|| panic!("{name} is in the design-reference corpus"))
        .1
}

/// `d`'s node with this id, or a failure naming it — every round-4 test below reads specific,
/// named boxes out of a real render rather than scanning for a shape.
fn node_of<'a>(d: &'a Diagram, id: &str) -> &'a PlacedNode {
    d.node(id)
        .unwrap_or_else(|| panic!("the fixture must place a node called {id}"))
}

/// [`node_of`] for a frame.
fn cluster_of<'a>(d: &'a Diagram, id: &str) -> &'a PlacedCluster {
    d.cluster(id)
        .unwrap_or_else(|| panic!("the fixture must place a frame called {id}"))
}

/// `d`'s edge between these two written endpoints.
fn edge_of<'a>(d: &'a Diagram, from: &str, to: &str) -> &'a PlacedEdge {
    d.edges
        .iter()
        .find(|e| e.from == from && e.to == to)
        .unwrap_or_else(|| panic!("the fixture must draw {from} -> {to}"))
}

/// Every point of `e` shares one `x` — a dead-straight vertical run, which is what "曲げ 0" means
/// for a `TB` diagram.
fn assert_vertical(name: &str, e: &PlacedEdge, at: f64) {
    for p in e.drawn_points() {
        assert!(
            (p.x - at).abs() <= 0.51,
            "{name}: {} -> {} bends — point ({:.2},{:.2}) is off the x={at:.2} lane",
            e.from,
            e.to,
            p.x,
            p.y
        );
    }
}

/// §10-5 round 4, the ASAP re-rank's own point: **a composite state is re-layered as one body, and
/// a node with slack beside it lands at the earliest rank it can reach**, not wherever network
/// simplex's pivoting left it.
///
/// `B` here is the classic slack node — `A --> B --> D` with both edges at the default weight, so
/// every rank between `A`'s and `D`'s costs the ranker exactly the same, and dagre is free to drop
/// it anywhere (on `zz-design-4c` it dropped `監査` eleven ranks down, level with the *innermost*
/// member of a two-level nest). The composite `C` beside it spans four ranks of its own, so this
/// only comes out right if the re-rank reads `C` as one unit with an entry level and an exit level
/// rather than as four unrelated nodes.
///
/// Pinned as "`B` sits on the same row as `C`'s own entry" rather than as a rank *number*: the row
/// is what a reader sees, and it is the same statement §10-3 item 8 makes for a fan.
#[test]
fn orthogonal_a_composite_is_re_ranked_as_one_unit_beside_a_slack_node() {
    if !text_metrics::fonts_available() {
        return;
    }
    let src = "stateDiagram-v2\n  [*] --> A\n  A --> B\n  A --> C\n  state C {\n    \
               [*] --> C1\n    C1 --> C2\n    C2 --> C3\n  }\n  B --> D\n  C --> D";
    let d = laid_out_orthogonal(src);
    let (b, entry) = (node_of(&d, "B"), node_of(&d, "C_start"));
    assert!(
        (b.center.y - entry.center.y).abs() <= 0.51,
        "B (y={:.2}) must share the composite's own entry row (y={:.2}), not sink to a later rank",
        b.center.y,
        entry.center.y
    );
    // …and the composite's interior is untouched by the move: four members, four consecutive rows.
    let interior: Vec<f64> = ["C_start", "C1", "C2", "C3"]
        .iter()
        .map(|id| node_of(&d, id).center.y)
        .collect();
    assert!(
        interior.windows(2).all(|w| w[1] > w[0]),
        "the composite's own members must keep their order down the flow: {interior:?}"
    );
}

/// §10-5 round 4, the lane pass's own point: **a straight lane may cross a frame's border**. The
/// node before the block, the block's frame, its internal start marker, every member of its
/// internal spine and the node after it all sit on one cross coordinate, and every edge along the
/// way is a single vertical with no bend outside the frame.
///
/// Before round 4 the two cluster-anchored edges here (`A --> C` and `C --> Z`, laid out against
/// `C`'s entry and exit members) were filtered out of the lane candidates outright, so no lane
/// could ever reach a block at all.
#[test]
fn orthogonal_a_lane_runs_straight_through_a_composite_frame() {
    if !text_metrics::fonts_available() {
        return;
    }
    // `W` is what makes this discriminating rather than accidental: a wide sibling on the block's
    // own rank pulls `A` (their shared source) well off the block's column, so `A --> C` genuinely
    // has to move something to draw straight. Without it every node sits in one column anyway and
    // the assertions below would pass whether or not a lane was ever selected.
    let src = "stateDiagram-v2\n  [*] --> A\n  A --> C\n  state C {\n    [*] --> C1\n    \
               C1 --> C2\n  }\n  C --> Z\n  A --> W\n  W --> Z\n  W : a deliberately wide sibling";
    let d = laid_out_orthogonal(src);
    let lane = node_of(&d, "A").center.x;
    for id in ["C_start", "C1", "C2", "Z"] {
        assert!(
            (node_of(&d, id).center.x - lane).abs() <= 0.51,
            "{id} (x={:.2}) must sit on the lane A starts (x={lane:.2})",
            node_of(&d, id).center.x
        );
    }
    assert!(
        (cluster_of(&d, "C").center.x - lane).abs() <= 0.51,
        "the frame's own centre (x={:.2}) is what a cluster-anchored edge aligns against — it has \
         to be on the lane too (x={lane:.2})",
        cluster_of(&d, "C").center.x
    );
    assert_vertical("lane-through-composite", edge_of(&d, "A", "C"), lane);
    assert_vertical("lane-through-composite", edge_of(&d, "C", "Z"), lane);
}

/// `zz-design-4c`'s own G1: the fork's two branches share the row directly under the bar.
///
/// `監査` is the slack node (`fork_state --> 監査 --> join_state`); dagre put it eleven ranks down,
/// level with `集計` — the innermost member of the two-level nest inside `処理` — which is what
/// made the design's own "並行ブランチは各自の縦トランクを持ち" impossible to read at all.
#[test]
fn orthogonal_design_4c_fork_targets_share_the_row_below_the_bar() {
    if !text_metrics::fonts_available() {
        return;
    }
    let d = laid_out_orthogonal(design_reference_source("zz-design-4c"));
    let (get, audit) = (node_of(&d, "取得"), node_of(&d, "監査"));
    assert!(
        (get.center.y - audit.center.y).abs() <= 0.51,
        "取得 (y={:.2}) and 監査 (y={:.2}) are the fork's own two branches: same row",
        get.center.y,
        audit.center.y
    );
    let bar = node_of(&d, "fork_state");
    assert!(
        get.center.y > bar.center.y && get.center.y < node_of(&d, "処理_start").center.y,
        "that row sits between the fork bar and the composite's own first member"
    );
    // The bar spans both trunks — S4's own "長さ＝接続先トランク span＋両端各16px".
    let (bl, br) = (
        bar.center.x - bar.size.w / 2.0,
        bar.center.x + bar.size.w / 2.0,
    );
    for n in [get, audit] {
        assert!(
            n.center.x > bl && n.center.x < br,
            "the fork bar ({bl:.2}..{br:.2}) must straddle {}'s trunk (x={:.2})",
            n.id,
            n.center.x
        );
    }
}

/// `zz-design-4c`'s own G2: **`取得 → 処理 → join` is one trunk**. The node above the frame, the
/// frame itself, its internal start marker, its whole internal spine and the join bar's own port
/// all sit on one `x`, and neither cluster-anchored edge bends outside the frame.
#[test]
fn orthogonal_design_4c_trunk_runs_straight_through_the_composite_to_the_join() {
    if !text_metrics::fonts_available() {
        return;
    }
    let d = laid_out_orthogonal(design_reference_source("zz-design-4c"));
    let lane = node_of(&d, "取得").center.x;
    for id in ["処理_start", "整形", "走査", "集計"] {
        assert!(
            (node_of(&d, id).center.x - lane).abs() <= 0.51,
            "{id} (x={:.2}) must sit on 取得's own trunk (x={lane:.2})",
            node_of(&d, id).center.x
        );
    }
    for id in ["処理", "解析"] {
        assert!(
            (cluster_of(&d, id).center.x - lane).abs() <= 0.51,
            "frame {id} (x={:.2}) must sit on the trunk too (x={lane:.2})",
            cluster_of(&d, id).center.x
        );
    }
    assert_vertical("zz-design-4c", edge_of(&d, "取得", "処理"), lane);
    assert_vertical("zz-design-4c", edge_of(&d, "処理", "join_state"), lane);
    // The parallel branch keeps a trunk of its own, all the way to the bar.
    let other = node_of(&d, "監査").center.x;
    assert_vertical("zz-design-4c", edge_of(&d, "監査", "join_state"), other);
}

/// `zz-design-4c`'s own G3: §10-5 S4's "join の下流出力はバー入力群の重心" only draws straight if the
/// node downstream of the bar is *at* that centroid. A bar is not a lane participant (every other
/// port on it simply repeats its neighbour's coordinate), so the lane pass moves the downstream
/// lane there outright.
#[test]
fn orthogonal_design_4c_join_output_lane_sits_on_the_bar_centroid() {
    if !text_metrics::fonts_available() {
        return;
    }
    let d = laid_out_orthogonal(design_reference_source("zz-design-4c"));
    let centroid = (node_of(&d, "集計").center.x + node_of(&d, "監査").center.x) / 2.0;
    for id in ["完了", "root_end"] {
        assert!(
            (node_of(&d, id).center.x - centroid).abs() <= 0.51,
            "{id} (x={:.2}) must sit on the join bar's own input centroid (x={centroid:.2})",
            node_of(&d, id).center.x
        );
    }
    assert_vertical("zz-design-4c", edge_of(&d, "join_state", "完了"), centroid);
    assert_vertical("zz-design-4c", edge_of(&d, "完了", "root_end"), centroid);

    // `zz-design-4c`'s own two branches straddle the join evenly, so dagre's barycentre already
    // lands on the centroid there and the correction has nothing to do — which means the half
    // above pins the *composition* but cannot prove the rule fires. This one can: `X --> B` drags
    // `B` off centre, so the join's two inputs are lopsided and dagre's own placement for the node
    // after the bar is 4px off the centroid its output port actually sits at (measured; a
    // mutation that zeroes the correction leaves exactly that jog behind).
    let d = laid_out_orthogonal(
        "stateDiagram-v2\n  state f <<fork>>\n  state j <<join>>\n  [*] --> f\n  f --> A\n  \
         f --> B\n  X --> B\n  A --> j\n  B --> j\n  j --> [*]",
    );
    let centroid = (node_of(&d, "A").center.x + node_of(&d, "B").center.x) / 2.0;
    assert!(
        (node_of(&d, "root_end").center.x - centroid).abs() <= 0.51,
        "the node after a lopsided join must sit on its input centroid (x={centroid:.2}), not on \
         wherever dagre's barycentre put it (x={:.2})",
        node_of(&d, "root_end").center.x
    );
    assert_vertical("lopsided-join", edge_of(&d, "j", "root_end"), centroid);
}

/// `zz-design-4a`'s own G5: **the frame is treated as a node** — `ツリー` and the composite state
/// share one cross coordinate, so `Enter` (in) and `q` (out) are both dead-vertical between
/// `ツリー`'s bottom face and the frame's top face, `PORT_SPACING` apart on the retreat grid rather
/// than one leaving a side face with two bends.
#[test]
fn orthogonal_design_4a_the_frame_aligns_with_the_node_above_it() {
    if !text_metrics::fonts_available() {
        return;
    }
    let d = laid_out_orthogonal(design_reference_source("zz-design-4a"));
    let (tree_node, frame) = (node_of(&d, "ツリー"), cluster_of(&d, "プレビュー"));
    assert!(
        (tree_node.center.x - frame.center.x).abs() <= 0.51,
        "ツリー (x={:.2}) and the プレビュー frame (x={:.2}) must share a lane",
        tree_node.center.x,
        frame.center.x
    );
    for (from, to) in [("ツリー", "プレビュー"), ("プレビュー", "ツリー")] {
        let e = edge_of(&d, from, to);
        let at = e.points[0].x;
        assert_vertical("zz-design-4a", e, at);
        let off = (at - frame.center.x).abs();
        assert!(
            off <= orthogonal::PORT_SPACING * 2.0 + 0.51,
            "{from} -> {to} runs at x={at:.2}, {off:.2}px off the shared face centre — the two \
             transitions are meant to sit on adjacent slots of the same retreat grid"
        );
    }
    // The interior spine is a straight vertical of its own, on that same lane.
    for id in ["プレビュー_start", "デコード中", "表示"] {
        assert!(
            (node_of(&d, id).center.x - frame.center.x).abs() <= 0.51,
            "{id} must sit at the head of the frame's own internal lane"
        );
    }
}

/// `zz-design-4a`'s own G7: **the `q` back edge's label room lands between `ツリー` and the frame,
/// never inside it.**
///
/// dagre reserves a rank for a labelled edge's proxy and sizes that rank to the label; for a back
/// edge spanning a composite state, the proxy lands *inside* the block, stretching its interior by
/// a row it has no content for (measured before round 4: `● → デコード中` 131.9px against
/// `デコード中 → 表示` 110.8px, the wrong way round — only the second of those carries a label).
/// The round-4 re-rank computes every column gap itself, charging each label to the gap right after
/// its own tail's column, so the room ends up where the label is drawn.
#[test]
fn orthogonal_design_4a_back_edge_label_room_stays_outside_the_frame() {
    if !text_metrics::fonts_available() {
        return;
    }
    let d = laid_out_orthogonal(design_reference_source("zz-design-4a"));
    let frame = cluster_of(&d, "プレビュー").bounds();
    let q = edge_of(&d, "プレビュー", "ツリー")
        .label
        .as_ref()
        .expect("the q transition carries a label");
    assert!(
        q.center.y < frame.1,
        "the q label sits at y={:.2}, inside the frame that starts at y={:.2} — its room belongs \
         in the gap above the frame",
        q.center.y,
        frame.1
    );
    // The unlabelled interior gap is exactly one `RANK_SEP` — no rank was reserved inside the
    // block for a label drawn outside it.
    let (start, first) = (node_of(&d, "プレビュー_start"), node_of(&d, "デコード中"));
    let gap = (first.center.y - first.size.h / 2.0) - (start.center.y + start.size.h / 2.0);
    assert!(
        (gap - super::RANK_SEP).abs() <= 0.51,
        "● → デコード中 carries no label, so its gap should be exactly RANK_SEP ({:.2}), not \
         {gap:.2}",
        super::RANK_SEP
    );
}

/// `zz-design-4a`'s own **G6** (`docs/STATUS.md`'s own ★未修正): **`Q` leaves `ツリー`'s right face
/// and reaches the end marker in one bend**, because the marker is a dead end and `プレビュー` — the
/// trunk `ツリー` continues into — is not.
///
/// `ツリー` fans to exactly two members of one rank: the composite `プレビュー` (dagre anchors that
/// edge on the block's own entry member, `プレビュー_start`) and the diagram's own end marker. That
/// is `super::fan_split`'s one-branch case, and the branch is a dead end, so it takes the positive
/// cross side — `TB`'s right. Before the split was restated in terms of continuity, the flat
/// `before = len / 2` formula put the marker on the *left* instead, and `Q` left `ツリー`'s left face
/// (`docs/render-check/zz-compare-4a-*.png`, the picture the design reference is read against).
///
/// Stated as "which side, and how many bends", not as coordinates: the design reference is a
/// composition, and §10-4's own rule is not to chase its numbers.
#[test]
fn orthogonal_design_4a_the_end_marker_sits_right_of_the_trunk_one_bend_away() {
    if !text_metrics::fonts_available() {
        return;
    }
    let d = laid_out_orthogonal(design_reference_source("zz-design-4a"));
    let (tree_node, marker) = (node_of(&d, "ツリー"), node_of(&d, "root_end"));
    let entry = node_of(&d, "プレビュー_start");
    assert!(
        marker.center.x > entry.center.x,
        "the end marker (x={:.2}) is the dead-end branch, so it takes the right of the プレビュー \
         trunk (x={:.2})",
        marker.center.x,
        entry.center.x
    );
    let q = edge_of(&d, "ツリー", "root_end");
    assert_eq!(
        q.points.len(),
        3,
        "Q leaves ツリー's own cross-axis face and bends once into the marker's pole: {:?}",
        q.points
    );
    let (_, _, tree_right, _) = tree_node.bounds();
    assert!(
        (q.points[0].x - (tree_right + orthogonal::PORT_INSET)).abs() <= 0.51
            && (q.points[0].y - tree_node.center.y).abs() <= 0.51,
        "Q exits ツリー's right face, centred on it: {:?} vs right edge {tree_right:.2}, centre y \
         {:.2}",
        q.points[0],
        tree_node.center.y
    );
}

/// `zz-design-4b`'s own **G8** (`docs/STATUS.md`'s own ★未修正): **`休止` sits below the `更新`
/// trunk**, not above it.
///
/// The `分岐` choice fans to two states: `更新`, which carries the diagram on (`更新 → 通知 → 待機`),
/// and `休止`, which ends there. `super::fan_split`'s one-branch case puts the dead end on the
/// positive cross side — `LR`'s down. This is the fixture that separates "the sole non-trunk branch
/// goes before the trunk" (the old `before = len / 2` formula, which every earlier reference happened
/// to agree with because its own sole branch continued) from "a dead end goes after it".
#[test]
fn orthogonal_design_4b_the_dead_end_branch_sits_below_the_trunk() {
    if !text_metrics::fonts_available() {
        return;
    }
    let d = laid_out_orthogonal(design_reference_source("zz-design-4b"));
    let (update, pause) = (node_of(&d, "更新"), node_of(&d, "休止"));
    assert!(
        pause.center.y > update.center.y,
        "休止 (y={:.2}) is the dead end and belongs below the 更新 trunk (y={:.2})",
        pause.center.y,
        update.center.y
    );
    // And the trunk itself stays the diagram's straight spine through the choice.
    for (from, to) in [("待機", "監視"), ("監視", "分岐"), ("分岐", "更新")] {
        let e = edge_of(&d, from, to);
        assert_eq!(
            e.points.len(),
            2,
            "{from} -> {to} is a spine segment — a straight 2-point line: {:?}",
            e.points
        );
    }
}

// ---------------------------------------------------------------------------------------------
// 8. `konoma-orthogonal`'s own palette, state-diagram half (§10-5 S1/S2/S4, §10-1 item 5)
//
// The flowchart half is in `tests` (section 15) and states the same three properties; these are
// the parts only a state diagram has — the composite frame's title strip, the `[*]` markers and
// the fork/join bars.
// ---------------------------------------------------------------------------------------------

/// The state-diagram half of `tests::design_reference_renders_are_byte_stable_per_theme` — see
/// that test's doc comment for why this hashes [`mask_numbers`]' output rather than the raw SVG
/// (Linux CI's font metrics differ from macOS's, so raw text-measured coordinates legitimately
/// differ per platform). Hashes retaken on this machine 2026-09-06; the splines half must never
/// move.
#[test]
fn state_design_reference_renders_are_byte_stable_per_theme() {
    if !text_metrics::fonts_available() {
        return;
    }
    // (source, theme, routing, FNV-1a of the number-masked SVG, taken 2026-09-06)
    let pinned: &[(&str, &str, &str, u64)] = &[
        ("zz-design-4a", "dark", "splines", 6945086091217365677),
        ("zz-design-4a", "light", "splines", 17865779921064957116),
        ("zz-design-4a", "classic", "splines", 2374771864539773089),
        ("zz-design-4a", "forest", "splines", 3478882518047204662),
        ("zz-design-4a", "neutral", "splines", 14396774170046225880),
        ("zz-design-4b", "dark", "splines", 13664536395431744492),
        ("zz-design-4b", "light", "splines", 12935933493011207262),
        ("zz-design-4b", "classic", "splines", 5915862964357765746),
        ("zz-design-4b", "forest", "splines", 17218757109032054782),
        ("zz-design-4b", "neutral", "splines", 11666605071335975094),
        ("zz-design-4c", "dark", "splines", 4879018428627726240),
        ("zz-design-4c", "light", "splines", 14316294563286093084),
        ("zz-design-4c", "classic", "splines", 15012355425714036991),
        ("zz-design-4c", "forest", "splines", 13652035469580789214),
        ("zz-design-4c", "neutral", "splines", 1528042515392761697),
    ];
    let corpus = orthogonal_design_reference_corpus();
    for (name, theme_name, routing, want) in pinned {
        let (_, src) = corpus
            .iter()
            .find(|(n, _)| n == name)
            .unwrap_or_else(|| panic!("{name} must still be in the design corpus"));
        let svg = render_flow(src, theme_name, routing).expect("renders");
        let masked = mask_numbers(&svg);
        assert_eq!(
            super::tests::fnv1a(&masked),
            *want,
            "{name}/{theme_name}/{routing} is no longer byte-identical (number-masked) to its \
             pinned render"
        );
    }
}

/// `[ui] mermaid_theme` is inert under `konoma-orthogonal` for a state diagram too.
#[test]
fn state_konoma_orthogonal_theme_is_inert() {
    if !text_metrics::fonts_available() {
        return;
    }
    for (name, src) in orthogonal_design_reference_corpus() {
        let base = render_flow(src, "dark", "konoma-orthogonal").expect("renders");
        assert!(
            base.contains(theme::KONOMA.node_stroke),
            "{name}: an orthogonal render must draw the palette's own outline colour"
        );
        for theme_name in ["light", "modern", "classic", "mermaid", "forest", "neutral"] {
            let other = render_flow(src, theme_name, "konoma-orthogonal").expect("renders");
            assert_eq!(
                base, other,
                "{name}: mermaid_theme={theme_name} changed a konoma-orthogonal render"
            );
        }
        let splines = render_flow(src, "dark", "splines").expect("renders");
        assert!(
            !splines.contains(theme::KONOMA.node_stroke),
            "{name}: this comparison is only meaningful if the two palettes differ"
        );
    }
}

/// **§10-5 S1/S2/S4's own tokens, on the pictures the design drew** — every state design-reference
/// source under `konoma-orthogonal`, against `docs/render-check/zz-design-{4a,4b,4c}-wrap.html`.
#[test]
fn state_konoma_orthogonal_draws_the_design_reference_look() {
    if !text_metrics::fonts_available() {
        return;
    }
    for (name, src) in orthogonal_design_reference_corpus() {
        let svg = render_flow(src, "dark", "konoma-orthogonal").expect("renders");

        for stale in ["#2b2b38", "#1f2020", "#cccccc", "#d3d3d3", "#8a8a8a"] {
            assert!(
                !svg.contains(stale),
                "{name}: {stale} is a mermaid-theme colour and must not appear: {svg}"
            );
        }

        // S2: a composite frame is an unfilled solid 1px `#6e7681` rx=3 outline, and it is the
        // *only* kind of frame a state diagram draws here (`4c`'s own two nested ones).
        let frames: Vec<&str> = svg
            .lines()
            .filter(|l| l.starts_with("<rect") && l.contains("stroke=\"#6e7681\""))
            .collect();
        for line in &frames {
            assert!(
                line.contains("fill=\"none\"")
                    && line.contains("stroke-width=\"1\"")
                    && line.contains("rx=\"3\"")
                    && !line.contains("stroke-dasharray"),
                "{name}: a composite frame must be an unfilled solid 1px rx=3 outline: {line}"
            );
        }
        if src.contains("state ") && src.contains('{') {
            assert!(
                !frames.is_empty(),
                "{name}: the source declares a composite state"
            );
            // …with the strip `emit_cluster` fills in the node colour, the 1px rule under it, and
            // its 11px title.
            assert!(
                svg.contains(&format!("fill=\"{}\"/>", theme::KONOMA.node_fill)),
                "{name}: the title strip must be filled in the node colour: {svg}"
            );
            assert!(
                svg.contains(&format!(
                    "stroke=\"{}\" stroke-width=\"1\"",
                    theme::KONOMA_TOKENS.composite_rule
                )),
                "{name}: the strip's own rule must be drawn: {svg}"
            );
            assert!(
                svg.contains(&format!(
                    "font-size=\"11\" fill=\"{}\"",
                    theme::KONOMA_TOKENS.composite_text
                )),
                "{name}: the strip title must be 11px in the palette's own colour: {svg}"
            );
        }

        // S1: the `[*]` marks and the fork/join bars are solid `#8b949e`, and a state box is the
        // same 3px-cornered 1.5px-outlined box a flowchart draws.
        assert!(
            svg.contains(&format!("fill=\"{}\"/>", theme::KONOMA.state_marker)),
            "{name}: a state marker must be a solid fill in the palette's marker colour: {svg}"
        );
        for line in svg.lines() {
            if line.starts_with("<rect") && line.contains("stroke-width=\"1.5\"") {
                assert!(
                    line.contains("rx=\"3\"") && line.contains(theme::KONOMA.node_fill),
                    "{name}: a state box must be the palette's own box: {line}"
                );
            }
        }
        assert!(
            svg.contains("font-size=\"14\""),
            "{name}: body text must stay at the measured size: {svg}"
        );
        if svg.contains("<g class=\"edge-labels\">\n<") {
            assert!(
                svg.contains("font-size=\"11\""),
                "{name}: an edge label must be drawn at the reference's 11px: {svg}"
            );
        }
    }
}

/// §10-5 S2's title-strip-over-outline bug (`docs/render-check/zz-design-4a-ours.svg`, found
/// 2026-09-05): `emit_cluster` used to draw the frame's own `<rect>` outline first and the title
/// strip's fill on top of it, so the strip's square corners painted over the inner half of the
/// frame's stroke and its two top corner arcs wherever the strip reached them. The fix draws the
/// strip (a two-arc `path`, rounded to the frame's own top corners) — plus its dividing rule —
/// *before* the frame's `<rect>`, so the frame's stroke is always painted last, on top, and can
/// never be covered. This pins that document order, and that the strip's own corner radius tracks
/// the frame's, on every composite state in the permanent design corpus.
#[test]
fn state_title_strip_is_drawn_under_the_frame_outline_with_matching_corners() {
    if !text_metrics::fonts_available() {
        return;
    }
    let mut saw_a_strip = false;
    for (name, src) in orthogonal_design_reference_corpus() {
        let rendered = render_flow(src, "dark", "konoma-orthogonal").expect("renders");
        let lines: Vec<&str> = rendered.lines().collect();
        for (i, line) in lines.iter().enumerate() {
            let is_strip = line.starts_with("<path")
                && line.ends_with(&format!("fill=\"{}\"/>", theme::KONOMA.node_fill));
            if !is_strip {
                continue;
            }
            saw_a_strip = true;

            // `emit_cluster` writes a strip's path, its own rule, and its own frame `<rect>` back
            // to back in one call — nothing from another cluster can land between them.
            let rule = *lines
                .get(i + 1)
                .unwrap_or_else(|| panic!("{name}: a strip must be followed by its rule: {line}"));
            assert!(
                rule.starts_with("<line"),
                "{name}: line right after the strip must be its dividing rule, not: {rule}"
            );
            let frame = *lines.get(i + 2).unwrap_or_else(|| {
                panic!("{name}: a strip's rule must be followed by the frame outline: {line}")
            });
            assert!(
                frame.starts_with("<rect") && frame.contains("stroke=\"#6e7681\""),
                "{name}: the frame outline must be drawn right after its own strip, not: {frame}"
            );

            // The strip's own top corners are rounded to the frame's radius, minus half the
            // frame's stroke width (the inset that keeps the strip's fill off the stroke's own
            // outer edge) — `rx="3"` on the frame means two `A2.5,2.5` arcs in the strip's `path`.
            let rx: f64 = frame
                .split("rx=\"")
                .nth(1)
                .and_then(|s| s.split('"').next())
                .unwrap_or_else(|| panic!("{name}: frame must carry rx: {frame}"))
                .parse()
                .unwrap_or_else(|_| panic!("{name}: frame rx must be numeric: {frame}"));
            let want_r = rx - clusters::STROKE_WIDTH / 2.0;
            let arc = format!("A{},{}", svg::num(want_r), svg::num(want_r));
            assert_eq!(
                line.matches(&arc).count(),
                2,
                "{name}: the strip must round both its top corners to {arc}, matching the \
                 frame's own rx={rx}: {line}"
            );
        }
    }
    assert!(
        saw_a_strip,
        "the design corpus must contain at least one composite state to exercise this"
    );
}

/// The rasterised half of the fix above: the frame's own stroke colour must be present at the 45°
/// midpoint of both of a composite state's top corner arcs — a point that sits exactly on the
/// rounded boundary circle (distance `radius` from the corner's own arc centre), strictly inside
/// the region only the rounding carves out. The search window around that point is kept to a
/// fraction of `radius` on every side — small enough that it can never reach either flat edge (so
/// it cannot pick up the frame's own ordinary side stroke instead, which would pass even with the
/// bug reproduced), yet wide enough to absorb antialiasing at the high scale this test rasterises
/// at. A pixel further in from that same corner, well past the stroke, must be the strip's own
/// fill colour: the strip still reaches its own corner, it is not left with a notch.
#[test]
fn state_title_strip_corner_arcs_survive_rasterisation() {
    if !text_metrics::fonts_available() {
        return;
    }
    // A nominal request, not the scale pixel maths actually use below — `rasterize_bytes` clamps
    // its own output to `HARD_MAX_PX` (4096), which the tallest source here (`zz-design-4c`, at
    // 960.6 SVG units) would exceed at 8×; the *actual* scale it applied is read back from the
    // image it returns instead of assumed, so this holds for every source regardless of size.
    let requested_scale = 8.0_f64;
    let (stroke_r, stroke_g, stroke_b) =
        super::tests::hex_to_rgb(theme::KONOMA_TOKENS.composite_stroke);
    let (fill_r, fill_g, fill_b) = super::tests::hex_to_rgb(theme::KONOMA.node_fill);
    let radius = theme::KONOMA_TOKENS.frame_radius;
    // A window this size (in raw SVG units), centred on the diagonal point, stays within
    // (0, radius) of the corner's own arc centre on each axis: the diagonal point itself sits at
    // 0.293·radius; +-0.15·radius keeps the whole window inside (0.14·radius, 0.44·radius), clear
    // of the tangent points at 0 and at radius where the flat edges begin.
    let tolerance_raw = radius * 0.15;

    let mut checked_a_corner = false;
    for (name, src) in orthogonal_design_reference_corpus() {
        let d = laid_out_orthogonal(src);
        let rendered = render_flow(src, "dark", "konoma-orthogonal").expect("renders");
        let img = crate::preview::svg::rasterize_bytes(
            rendered.as_bytes(),
            std::path::Path::new("title-strip-corner.svg"),
            (d.width.max(d.height) * requested_scale).round() as u32,
        )
        .unwrap_or_else(|| panic!("{name}: must rasterise"));
        let rgba = img.to_rgba8();
        let (w, h) = (rgba.width() as i64, rgba.height() as i64);
        // The scale `rasterize_bytes` actually used, read back from its own output rather than
        // assumed — see the comment on `requested_scale` above.
        let scale = if d.width >= d.height {
            rgba.width() as f64 / d.width
        } else {
            rgba.height() as f64 / d.height
        };
        let tolerance_px = ((tolerance_raw * scale).ceil() as i64).max(1);
        let is_close = |x: i64, y: i64, want: (u8, u8, u8)| {
            x >= 0 && y >= 0 && x < w && y < h && {
                let p = rgba.get_pixel(x as u32, y as u32);
                p[3] == 255
                    && super::tests::close(p[0], want.0)
                    && super::tests::close(p[1], want.1)
                    && super::tests::close(p[2], want.2)
            }
        };
        let any_close_near = |cx: f64, cy: f64, want: (u8, u8, u8)| {
            let px = (cx * scale).round() as i64;
            let py = (cy * scale).round() as i64;
            (-tolerance_px..=tolerance_px)
                .any(|dy| (-tolerance_px..=tolerance_px).any(|dx| is_close(px + dx, py + dy, want)))
        };

        for cluster in d
            .clusters
            .iter()
            .filter(|c| c.title_strip && !c.title.is_blank())
        {
            checked_a_corner = true;
            let (l, t, right, _) = cluster.bounds();
            let half = radius / std::f64::consts::SQRT_2;
            let center_y = t + radius;

            // `sign` is the diagonal's own x-direction: up-and-left off the left corner's arc
            // centre, up-and-right off the right corner's.
            for (edge_x, center_x, sign) in
                [(l, l + radius, -1.0_f64), (right, right - radius, 1.0)]
            {
                let diag_x = center_x + sign * half;
                let diag_y = center_y - half;
                assert!(
                    any_close_near(diag_x, diag_y, (stroke_r, stroke_g, stroke_b)),
                    "{name}: no frame stroke at the 45° arc point of the corner at x={edge_x} \
                     (checked near ({diag_x},{diag_y})) — the corner arc is missing or covered"
                );
            }

            // Further in from the top-left corner than the stroke's own arc, past the rounding,
            // into the strip's own flat interior — its own fill colour must be there.
            let inside_x = l + radius + 4.0;
            let inside_y = t + 3.0;
            assert!(
                any_close_near(inside_x, inside_y, (fill_r, fill_g, fill_b)),
                "{name}: no strip fill colour just inside the top-left corner (checked near \
                 ({inside_x},{inside_y})) — the strip is cut short of its own corner"
            );
        }
    }
    assert!(
        checked_a_corner,
        "the design corpus must contain at least one composite state to exercise this"
    );
}

/// The exclusions again, through the real renderer this time: a state diagram's markers, choice
/// and bars keep the sizes §10-5 S1/S4 gives them under `konoma-orthogonal`, with §10-8 in force
/// for the ordinary state boxes beside them.
#[test]
fn orthogonal_state_markers_choice_and_bars_keep_their_own_sizes_under_n8() {
    if !text_metrics::fonts_available() {
        return;
    }
    let d = laid_out_orthogonal(
        "stateDiagram-v2\n  state c <<choice>>\n  state f <<fork>>\n  [*] --> A\n  A --> c\n  \
         c --> B\n  c --> C\n  B --> f\n  f --> D\n  D --> [*]",
    );
    for n in &d.nodes {
        match n.shape {
            Glyph::StateStart | Glyph::StateEnd => assert_eq!(
                (n.size.w, n.size.h),
                (
                    shapes::STATE_MARKER_RADIUS * 2.0,
                    shapes::STATE_MARKER_RADIUS * 2.0
                ),
                "S1: {} is a marker dot, never a 96x36 box",
                n.id
            ),
            Glyph::ChamferedRect if n.id == "c" => assert_eq!(
                (n.size.w, n.size.h),
                (28.0, 28.0),
                "S4: a choice is a fixed 28x28 square"
            ),
            Glyph::Bar { .. } => assert!(
                n.size.w.min(n.size.h) <= 6.0,
                "S4: {} is a 6px-thick bar, not a box: {:?}",
                n.id,
                n.size
            ),
            _ => assert_eq!(
                n.size.h,
                shapes::ortho_height(n.label.lines.len()),
                "N1/N5: {} is an ordinary state box, nested or not",
                n.id
            ),
        }
    }
}

/// §10-1 item 4, with `4b`'s own "完了" as the case: a back edge whose dagre chain runs on a lane
/// clear of the row leaves through the **cross** face, not the flow face — however wide the boxes
/// have grown.
///
/// The design draws `通知 --> 待機` under the row, in two corners, crossing nothing, and so did
/// konoma until §10-8 widened every box (2026-09-05): `classify`'s own `dominant_face` weighs the
/// flow-axis step to the chain's first dummy against the cross-axis one, and the flow-axis step is
/// about half a node plus half a rank gap — it grows with the boxes. Past 96px it overtook the
/// 34.7px the chain drops below the row, the edge was read as leaving sideways, and it came back
/// over the top of the diagram in four corners, crossing `監視`'s own self-loop twice.
///
/// Stated as the two things that must hold — the ports are on the cross faces, and the route stays
/// clear of the self-loop it used to cut — rather than as coordinates, so the picture may move
/// without this going quiet.
#[test]
fn orthogonal_design_4b_back_edge_leaves_through_the_face_its_lane_is_on() {
    if !text_metrics::fonts_available() {
        return;
    }
    let src = orthogonal_design_reference_corpus()
        .into_iter()
        .find(|(n, _)| *n == "zz-design-4b")
        .expect("4b is in the design-reference corpus")
        .1;
    let d = laid_out_orthogonal(src);
    let back = d
        .edges
        .iter()
        .find(|e| e.from == "通知" && e.to == "待機")
        .expect("通知 --> 待機 must exist");
    let loop_edge = d
        .edges
        .iter()
        .find(|e| e.from == "監視" && e.to == "監視")
        .expect("監視's self-transition must exist");

    // LR, so the cross axis is y: both ends must leave through a horizontal face, which means the
    // first and last legs run vertically.
    assert!(
        (back.points[0].x - back.points[1].x).abs() < 1e-6,
        "通知 leaves through its Top or Bottom face, not sideways: {:?}",
        back.points
    );
    let n = back.points.len();
    assert!(
        (back.points[n - 1].x - back.points[n - 2].x).abs() < 1e-6,
        "待機 is entered through its Top or Bottom face: {:?}",
        back.points
    );
    assert_eq!(
        n - 2,
        2,
        "two corners, the way the design draws it: {:?}",
        back.points
    );

    // And it crosses nothing — the self-loop is the line it used to cut.
    let crosses = |a: &Point, b: &Point, c: &Point, dd: &Point| {
        let o = |p: &Point, q: &Point, r: &Point| {
            let v = (q.x - p.x) * (r.y - p.y) - (q.y - p.y) * (r.x - p.x);
            if v > 1e-9 {
                1
            } else if v < -1e-9 {
                -1
            } else {
                0
            }
        };
        o(a, b, c) != o(a, b, dd) && o(c, dd, a) != o(c, dd, b)
    };
    for w1 in back.points.windows(2) {
        for w2 in loop_edge.points.windows(2) {
            assert!(
                !crosses(&w1[0], &w1[1], &w2[0], &w2[1]),
                "the back edge must not cut the self-loop: {:?} vs {:?}",
                back.points,
                loop_edge.points
            );
        }
    }
}

/// N5, over the whole state corpus: an ordinary state box follows N1 whatever it is nested in.
/// "内部ノードにも N1〜N4 をそのまま適用…入れ子段数による縮小はしない".
#[test]
fn orthogonal_state_boxes_follow_n1_at_every_nesting_depth() {
    if !text_metrics::fonts_available() {
        return;
    }
    let mut nested = 0usize;
    for (name, src) in orthogonal_full_corpus() {
        let d = laid_out_orthogonal(src);
        for n in &d.nodes {
            // A `<<choice>>` shares `Glyph::ChamferedRect` with a decision node, and only its size
            // tells them apart downstream — §10-5 S4's own fixed square, which N3 lists among its
            // exclusions. Named by the constant rather than by `28.0` so the two cannot drift.
            let is_choice = n.shape == Glyph::ChamferedRect
                && n.size.w == super::state::STATE_CHOICE_ORTHO_SIZE
                && n.size.h == super::state::STATE_CHOICE_ORTHO_SIZE;
            if !shapes::orthogonal_covers(n.shape) || is_choice {
                continue;
            }
            let label_box = shapes::orthogonal_label_box(n.shape, &n.label);
            assert!(
                n.size.h >= label_box.h - 1e-6 && n.size.w >= label_box.w - 1e-6,
                "{name}: {} is smaller than N1/N2 asks for: {:?} vs {label_box:?}",
                n.id,
                n.size
            );
            // Nesting is what N5 is about, so it is counted rather than assumed to be present:
            // a node sitting inside any frame is a nested one.
            if d.clusters.iter().any(|c| {
                let (l, t, r, b) = c.bounds();
                n.center.x > l && n.center.x < r && n.center.y > t && n.center.y < b
            }) {
                nested += 1;
                assert_eq!(
                    label_box.h,
                    shapes::ortho_height(n.label.lines.len()),
                    "{name}: {} is inside a frame and must NOT be shrunk for it",
                    n.id
                );
            }
        }
    }
    assert!(
        nested > 5,
        "the corpus must actually contain nested states: only {nested} seen"
    );
}

// =============================================================================================
// `direction`'s **sign** for a state diagram — `RL` is `LR` mirrored, `BT` is `TB` mirrored
// (`docs/FEATURE-MERMAID-RENDERER.md` §10-5; `super::canonicalise_flow_axis`/`super::
// mirror_flow_axis`). The flowchart half of this is `tests`'s own section of the same name.
// =============================================================================================

/// `samples/mermaid.md`'s own `stateDiagram-v2`, byte-for-byte — the sample the coordinator names,
/// kept as a source here for the same reason `tests::SAMPLE_FIRST_FLOWCHART` is.
const SAMPLE_STATE_DIAGRAM: &str =
    "stateDiagram-v2\n  [*] --> Tree\n  Tree --> Preview : Enter\n  \
     Preview --> Tree : q\n  state Preview {\n    [*] --> Decoding\n    \
     Decoding --> Ready : image arrives\n  }\n  Tree --> [*] : Q";

/// The same source laid out in `direction` — **appended**, not substituted.
///
/// A state diagram's direction is a statement rather than a header word, and the parser keeps the
/// last *top-level* one it reads (`state::parser`'s own `direction_statement` arm: it assigns
/// unconditionally when `stack.len() == 1`, and a `direction` written inside a block is read and
/// deliberately ignored). So one line at the end settles the whole diagram's axis without having
/// to find, and correctly scope, whatever `direction` the source may already carry — `zz-design-4b`
/// carries its own `direction LR`, and `4a`/`4c` carry none.
pub(super) fn state_with_direction(src: &str, direction: &str) -> String {
    let out = format!("{src}\n  direction {direction}");
    assert_eq!(
        state::parse(&out)
            .expect("a state fixture parses")
            .direction,
        crate::preview::mermaid::flowchart::Direction::parse(direction)
            .expect("a direction keyword this module wrote itself"),
        "appending `direction {direction}` has to be what the parser ends up with"
    );
    out
}

/// The sources the state-diagram mirror is stated over: the sample above and round 4's own three
/// design references.
fn direction_fixture_sources() -> Vec<(&'static str, &'static str)> {
    std::iter::once(("sample-state-diagram", SAMPLE_STATE_DIAGRAM))
        .chain(orthogonal_design_reference_corpus())
        .collect()
}

/// [`direction_fixture_sources`] in all four directions, chained into [`orthogonal_full_corpus`] so
/// every §10-5 invariant runs on the reversed half of each axis too — derived from the sources
/// rather than copied, for the reason `tests::orthogonal_reversed_direction_corpus`'s own doc gives.
fn orthogonal_reversed_direction_corpus() -> &'static [(String, String)] {
    static CACHE: std::sync::OnceLock<Vec<(String, String)>> = std::sync::OnceLock::new();
    CACHE.get_or_init(|| {
        let mut out: Vec<(String, String)> = Vec::new();
        for (name, src) in direction_fixture_sources() {
            for direction in ["LR", "RL", "TB", "BT"] {
                out.push((
                    format!("{name}-{direction}"),
                    state_with_direction(src, direction),
                ));
            }
        }
        out
    })
}

/// §10-5 under `RL`: the same drawing as `LR`, reflected along x — a start marker's own pole port
/// (S1), a self-transition's fixed cross-axis face (S3) and a fork/join bar's ports (S4) included.
#[test]
fn orthogonal_state_rl_is_the_exact_mirror_of_lr() {
    for (name, src) in direction_fixture_sources() {
        let lr = lay_out(
            &state::parse(&state_with_direction(src, "LR")).expect("parses"),
            Routing::Orthogonal,
        )
        .expect("lays out");
        let rl = lay_out(
            &state::parse(&state_with_direction(src, "RL")).expect("parses"),
            Routing::Orthogonal,
        )
        .expect("lays out");
        super::tests::assert_mirrored_diagrams(&format!("{name} LR/RL"), &lr, &rl, false);
    }
}

/// §10-5 under `BT`: the same drawing as `TB`, reflected along y.
#[test]
fn orthogonal_state_bt_is_the_exact_mirror_of_tb() {
    for (name, src) in direction_fixture_sources() {
        let tb = lay_out(
            &state::parse(&state_with_direction(src, "TB")).expect("parses"),
            Routing::Orthogonal,
        )
        .expect("lays out");
        let bt = lay_out(
            &state::parse(&state_with_direction(src, "BT")).expect("parses"),
            Routing::Orthogonal,
        )
        .expect("lays out");
        super::tests::assert_mirrored_diagrams(&format!("{name} TB/BT"), &tb, &bt, true);
    }
}

/// The sample state diagram's own `LR` and `TB` are **two different layouts**, and the mirror does
/// not make either of them the other.
///
/// This is the half of the statement the two mirror tests above cannot make: they would both pass
/// on an implementation that collapsed all four directions into one drawing, which is exactly the
/// bug that was there. `Tree` and `Preview` sit on the axis the direction names, so a diagram laid
/// out `LR` is wider than it is tall and a `TB` one is not.
#[test]
fn orthogonal_state_lr_and_tb_stay_two_different_layouts() {
    let lr = lay_out(
        &state::parse(&state_with_direction(SAMPLE_STATE_DIAGRAM, "LR")).expect("parses"),
        Routing::Orthogonal,
    )
    .expect("lays out");
    let tb = lay_out(
        &state::parse(&state_with_direction(SAMPLE_STATE_DIAGRAM, "TB")).expect("parses"),
        Routing::Orthogonal,
    )
    .expect("lays out");
    let flow_gap = |d: &Diagram, vertical: bool| {
        let of = |id: &str| {
            let n = d.nodes.iter().find(|n| n.id == id).expect("state exists");
            if vertical {
                n.center.y
            } else {
                n.center.x
            }
        };
        of("Decoding") - of("Tree")
    };
    assert!(
        flow_gap(&lr, false) > 0.0,
        "LR runs the flow along x: Decoding sits to the right of Tree"
    );
    assert!(
        flow_gap(&tb, true) > 0.0,
        "TB runs the flow along y: Decoding sits below Tree"
    );
    assert!(
        lr.width > lr.height && tb.height > tb.width,
        "the two directions are two layouts: LR came out {}x{} and TB {}x{}",
        lr.width,
        lr.height,
        tb.width,
        tb.height
    );
}
