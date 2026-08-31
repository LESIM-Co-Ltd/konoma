//! What stage 4 is checked with.
//!
//! `docs/FEATURE-MERMAID-RENDERER.md` §6 sets the terms, and the corpus is built from the *cases
//! in the grammar and in mermaid's own documentation page* rather than from bugs already found
//! (memory `corpus-from-spec-not-from-bugs`).
//!
//! # Which of the twelve shared invariants apply here, and which do not
//!
//! Rule: a shared check is **called**, never copied ([`super::tests`] owns all twelve). But several
//! of them are statements about a *layered graph* and would be meaningless — or quietly wrong —
//! about a sequence diagram, so each one is accounted for:
//!
//! | # | invariant | here |
//! |---|---|---|
//! | 1 | `check_nodes_do_not_overlap` | **called**. Participant boxes, notes and the title are nodes; activation bars deliberately are not (see below). |
//! | 2 | `check_edges_stay_out_of_shapes` | **called**. A message must not run through a participant's box or through a note. |
//! | 3 | `check_labels_ride_their_edge` | **does not apply.** It asserts the label sits *on* its own line, which is right for a flowchart (the label is placed at the arc midpoint, on an opaque patch). A sequence diagram's message label goes in a band *above* its arrow, so this check would fail on a correct diagram. Replaced by [`a_message_label_sits_above_its_own_arrow`]. |
//! | 4 | `check_endpoints_land_on_the_outline` | **does not apply.** A message does not end on a node's outline; it ends on a *lifeline*, which is not a node. Worse, the check would `panic!` here, because `Diagram::node(from)` does find the head box and would then measure against the wrong thing. Replaced by [`every_message_ends_on_its_participants_axis`]. |
//! | 5 | `check_view_box_contains_everything` | **called**, and extended by [`nothing_is_drawn_outside_the_view_box`] for the geometry it does not know about (lifelines, bars, frames). |
//! | 6 | `check_clusters_hold_their_members` | **called**, over a [`clusters::Tree`] built from the `box` groups — a `box` is a frame that really does hold participants. A `loop` / `alt` / … holds *messages*, which the tree cannot express, so those frames are skipped by `touches` and are covered by [`a_group_frame_encloses_every_message_inside_it`] instead. |
//! | 7 | `check_nested_clusters_sit_inside_their_parent` | **called**. `par` inside `par` really does nest. |
//! | 8 | `check_unrelated_clusters_do_not_overlap` | **does not apply.** A sequence diagram has two orthogonal kinds of frame: a `box` is a *vertical* band over some participants and a `loop` is a *horizontal* band over some messages. They cross by construction, and every diagram with both would fail. |
//! | 9 | `check_edges_keep_out_of_foreign_frames` | **does not apply.** It decides "does this edge belong in this frame?" from `Tree::touches`, i.e. from the endpoints' *identity*. Membership of a sequence group is by **position in the event stream**: the same two participants can exchange one message inside a `loop` and the next one outside it. Replaced by [`a_message_outside_a_frame_stays_out_of_it`], which asks the question by position. |
//! | 10 | `check_edge_that_names_a_block_stops_on_its_frame` | **does not apply**, and is vacuous besides: no sequence statement can name a block as a message endpoint. |
//! | 11 | `check_cluster_titles` | **does not apply** as written — it needs a `Tree` and asks about *member nodes*, and a group frame has none. The property is still worth having, so [`a_frame_title_stays_clear_of_everything_inside_it`] states it about messages. |
//! | 12 | `check_panels_stay_inside_their_box` | **called**. An `actor`'s name lives in a [`Panel`](super::panel::Panel), because the stick figure needs a band of its own above the text. |
//!
//! **Activation bars are not nodes**, and that is why (1) can be called at all: a bar spans the y
//! of everything drawn on its lifeline, so a `note over A` while `A` is active overlaps it by
//! construction. They live on [`super::PlacedLifeline`] instead, which makes "the bar is on its
//! own lifeline" true by construction and leaves "it spans from its activating message to its
//! deactivating one" for a test to state — which is the half that can actually be wrong.

use std::collections::{HashMap, HashSet};
use std::path::Path as FsPath;

use super::clusters;
use super::sequence::{lay_out, render};
use super::tests::{
    assert_snapshot, check_clusters_hold_their_members, check_edges_stay_out_of_shapes,
    check_nested_clusters_sit_inside_their_parent, check_nodes_do_not_overlap,
    check_panels_stay_inside_their_box, check_view_box_contains_everything, mask_numbers,
    measured_texts, tree_of,
};
use super::{
    labels, svg, theme, ClusterSection, Curve, Diagram, Glyph, Label, PlacedActivation,
    PlacedCluster, PlacedEdge, PlacedEdgeLabel, PlacedLifeline, PlacedNode, RenderError, Size,
    SpecBlock, Theme, Tip, ACTIVATION_WIDTH,
};
use crate::preview::mermaid::flowchart::{Shape, Stroke};
use crate::preview::mermaid::layout::Point;
use crate::preview::mermaid::sequence::{Event, SequenceDiagram};
use crate::preview::mermaid::text_metrics;

// ---------------------------------------------------------------------------------------------
// Corpus
// ---------------------------------------------------------------------------------------------

/// The diagrams every structural test runs over, and the ones [`gallery`] writes out.
///
/// Every one of mermaid's own documented examples is here in the form the documentation gives it,
/// plus one case per distinction the grammar makes that the documentation does not illustrate.
pub const CASES: &[(&str, &str)] = &[
    (
        "basic",
        "sequenceDiagram\n    Alice->>John: Hello John, how are you?\n    John-->>Alice: Great!\n    \
         Alice-)John: See you later!",
    ),
    (
        "participants-ordered",
        "sequenceDiagram\n    participant Alice\n    participant Bob\n    Bob->>Alice: Hi Alice\n    \
         Alice->>Bob: Hi Bob",
    ),
    (
        "actors",
        "sequenceDiagram\n    actor Alice\n    actor Bob\n    Alice->>Bob: Hi Bob\n    \
         Bob->>Alice: Hi Alice",
    ),
    (
        "aliases",
        "sequenceDiagram\n    participant A as Alice\n    participant J as John\n    \
         A->>J: Hello John, how are you?\n    J->>A: Great!",
    ),
    (
        "arrows",
        "sequenceDiagram\n    A->B: open solid\n    A-->B: open dotted\n    A->>B: solid head\n    \
         A-->>B: dotted head\n    A<<->>B: bidirectional\n    A<<-->>B: bidirectional dotted\n    \
         A-xB: solid cross\n    A--xB: dotted cross\n    A-)B: solid async\n    A--)B: dotted async",
    ),
    (
        "half-arrows",
        "sequenceDiagram\n    A-|\\B: top half\n    A--|/B: bottom half dotted\n    \
         A/|-B: reverse top\n    A\\\\--B: reverse bottom stick dotted",
    ),
    (
        "activation",
        "sequenceDiagram\n    Alice->>John: Hello John, how are you?\n    activate John\n    \
         John-->>Alice: Great!\n    deactivate John",
    ),
    (
        "activation-suffix",
        "sequenceDiagram\n    Alice->>+John: Hello John, how are you?\n    John-->>-Alice: Great!",
    ),
    (
        "activation-stacked",
        "sequenceDiagram\n    Alice->>+John: Hello John, how are you?\n    \
         Alice->>+John: John, can you hear me?\n    John-->>-Alice: Hi Alice, I can hear you!\n    \
         John-->>-Alice: I feel great!",
    ),
    (
        "notes",
        "sequenceDiagram\n    Alice->John: Hello John, how are you?\n    \
         Note over Alice,John: A typical interaction\n    Note right of John: Text in note\n    \
         Note left of Alice: On the left",
    ),
    (
        "line-breaks",
        "sequenceDiagram\n    participant Alice as Alice<br/>Johnson\n    \
         Alice->John: Hello John,<br/>how are you?\n    \
         Note over Alice,John: A typical interaction<br/>But now in two lines",
    ),
    (
        "loop",
        "sequenceDiagram\n    Alice->John: Hello John, how are you?\n    loop Every minute\n        \
         John-->Alice: Great!\n    end",
    ),
    (
        "alt-opt",
        "sequenceDiagram\n    Alice->>Bob: Hello Bob, how are you?\n    alt is sick\n        \
         Bob->>Alice: Not so good :(\n    else is well\n        \
         Bob->>Alice: Feeling fresh like a daisy\n    end\n    opt Extra response\n        \
         Bob->>Alice: Thanks for asking\n    end",
    ),
    (
        "par",
        "sequenceDiagram\n    par Alice to Bob\n        Alice->>Bob: Hello guys!\n    \
         and Alice to John\n        Alice->>John: Hello guys!\n    end\n    \
         Bob-->>Alice: Hi Alice!\n    John-->>Alice: Hi Alice!",
    ),
    (
        "par-nested",
        "sequenceDiagram\n    par Alice to Bob\n        Alice->>Bob: Go help John\n    \
         and Alice to John\n        Alice->>John: I want this done today\n        \
         par John to Charlie\n            John->>Charlie: Can we do this today?\n        \
         and John to Diana\n            John->>Diana: Can you help us today?\n        end\n    end",
    ),
    (
        "critical",
        "sequenceDiagram\n    critical Establish a connection to the DB\n        \
         Service-->DB: connect\n    option Network timeout\n        Service-->Service: Log error\n    \
         option Credentials rejected\n        Service-->Service: Log different error\n    end",
    ),
    (
        "break",
        "sequenceDiagram\n    Consumer-->API: Book something\n    \
         API-->BookingService: Start booking process\n    break when the booking process fails\n        \
         API-->Consumer: show failure\n    end\n    API-->BillingService: Start billing process",
    ),
    (
        "rect",
        "sequenceDiagram\n    participant Alice\n    participant John\n\n    \
         rect rgb(191, 223, 255)\n    note right of Alice: Alice calls John.\n    \
         Alice->>+John: Hello John, how are you?\n    rect rgb(200, 150, 255)\n    \
         Alice->>+John: John, can you hear me?\n    John-->>-Alice: Hi Alice, I can hear you!\n    \
         end\n    John-->>-Alice: I feel great!\n    end\n    \
         Alice ->>+ John: Did you want to go to the game tonight?\n    \
         John -->>- Alice: Yeah! See you there.",
    ),
    (
        "autonumber",
        "sequenceDiagram\n    autonumber\n    Alice->>John: Hello John, how are you?\n    \
         loop HealthCheck\n        John->>John: Fight against hypochondria\n    end\n    \
         Note right of John: Rational thoughts!\n    John-->>Alice: Great!\n    \
         John->>Bob: How about you?\n    Bob-->>John: Jolly good!",
    ),
    (
        "autonumber-start-step",
        "sequenceDiagram\n    autonumber 10 5\n    A->>B: one\n    B->>A: two\n    A->>B: three",
    ),
    (
        "box",
        "sequenceDiagram\n    box Purple Alice & John\n    participant A\n    participant J\n    \
         end\n    box Another Group\n    participant B\n    participant C\n    end\n    \
         A->>J: Hello John, how are you?\n    J->>A: Great!\n    \
         A->>B: Hello Bob, how is Charley?\n    B->>C: Hello Charley, how are you?",
    ),
    (
        "create-destroy",
        "sequenceDiagram\n    Alice->>Bob: Hello Bob, how are you ?\n    \
         Bob->>Alice: Fine, thank you. And you?\n    create participant Carl\n    \
         Alice->>Carl: Hi Carl!\n    create actor D as Donald\n    Carl->>D: Hi!\n    \
         destroy Carl\n    Alice-xCarl: We are too many\n    destroy Bob\n    Bob->>Alice: I agree",
    ),
    (
        "title",
        "sequenceDiagram\n    title Checkout flow\n    Customer->>Cart: add item\n    \
         Cart-->>Customer: total",
    ),
    (
        "cjk",
        "sequenceDiagram\n    participant U as 利用者\n    participant K as konoma\n    \
         participant FS as ファイル\n    U->>K: ツリーで選ぶ\n    K->>FS: 種別を解決\n    \
         FS-->>K: バイト列\n    K-->>U: 全画面プレビュー\n    \
         Note over K,FS: 端末が画像を話せば実ピクセル",
    ),
    (
        "self-message",
        "sequenceDiagram\n    A->>A: retry\n    A->>B: give up\n    B->>B: log it",
    ),
    (
        "actor-menus",
        "sequenceDiagram\n    participant Alice\n    participant John\n    \
         link Alice: Dashboard @ https://dashboard.contoso.com/alice\n    \
         links John: {\"Dashboard\": \"https://dashboard.contoso.com/john\"}\n    \
         Alice->>John: Hello John, how are you?\n    John-->>Alice: Great!\n    \
         Alice-)John: See you later!",
    ),
];

/// Label strings the measuring instrument runs over — the classes §6 names explicitly.
const LABELS: &[(&str, &str)] = &[
    ("ascii", "Start"),
    ("kerning", "AVATAR To Wa"),
    ("thin", "illicit lilli"),
    ("cjk", "全画面プレビュー"),
    ("cjk-punct", "開始、処理。「確認」"),
    ("emoji", "build 🚀 ship"),
    (
        "long",
        "resolve the preview kind and hand it to the right renderer",
    ),
    (
        "mixed",
        "konoma のプレビュー preview を全画面 fullscreen で",
    ),
    ("digits", "0123456789"),
    // The case stage 4 met for real. usvg folds runs of whitespace before shaping, so this is
    // drawn one space narrower than its bytes; `docs/STATUS.md` recorded the hole as belonging to
    // every diagram kind, and each kind's instrument now carries a case of it.
    ("double-space", "loop  Every minute"),
];

// ---------------------------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------------------------

fn parsed(src: &str) -> SequenceDiagram {
    crate::preview::mermaid::sequence::parse(src)
        .unwrap_or_else(|e| panic!("corpus source must parse: {e}\n{src}"))
}

fn laid_out(src: &str) -> Diagram {
    lay_out(&parsed(src)).unwrap_or_else(|e| panic!("corpus source must lay out: {e}\n{src}"))
}

fn lifeline<'a>(d: &'a Diagram, id: &str) -> &'a PlacedLifeline {
    d.lifelines
        .iter()
        .find(|l| l.id == id)
        .unwrap_or_else(|| panic!("no lifeline for {id}"))
}

/// What resvg actually drew `text` at, in the SVG a source produced.
///
/// The lookup key is `text` **after [`text_metrics::collapse_spaces`]**, because that is the
/// string konoma writes into the document: usvg folds runs of whitespace before it shapes, so
/// `Label::measure` folds them once, up front, and measures what will be drawn. Searching for the
/// author's bytes would make a double-space case unfindable rather than measurable.
fn drawn_width(src: &str, text: &str) -> f64 {
    let text = text_metrics::collapse_spaces(text).into_owned();
    let svg = render(src, "dark").expect("renders");
    let mut all = Vec::new();
    measured_texts(tree_of(&svg).root(), &mut all);
    let hits: Vec<f64> = all
        .iter()
        .filter(|(s, _)| *s == text)
        .map(|(_, w)| *w)
        .collect();
    assert!(
        !hits.is_empty(),
        "resvg drew no <text> reading {text:?} — a dropped label means the font resolution konoma \
         measured with is not the one resvg drew with. It drew: {:?}",
        all.iter().map(|(s, _)| s).collect::<Vec<_>>()
    );
    // Every copy is the same string in the same face, so they agree; the head and foot boxes of a
    // participant are two of them.
    hits.iter().copied().fold(0.0_f64, f64::max)
}

/// The rectangle of a placed label, as `(l, t, r, b)`.
fn label_bounds(l: &PlacedEdgeLabel) -> (f64, f64, f64, f64) {
    (
        l.center.x - l.size.w / 2.0,
        l.center.y - l.size.h / 2.0,
        l.center.x + l.size.w / 2.0,
        l.center.y + l.size.h / 2.0,
    )
}

fn overlap(a: (f64, f64, f64, f64), b: (f64, f64, f64, f64)) -> (f64, f64) {
    (a.2.min(b.2) - a.0.max(b.0), a.3.min(b.3) - a.1.max(b.1))
}

/// A `box` group as the shared cluster tree understands it: a frame that holds participant nodes.
///
/// Both the head box and the mirrored foot box are members, because both are drawn and the frame
/// has to hold both. Re-derived from [`sequence::parse`] rather than from the diagram, so the
/// question "is this node a member of that box?" is answered by the parser.
fn box_tree(src: &str) -> clusters::Tree {
    let parsed = parsed(src);
    let ids: HashSet<String> = parsed
        .participants
        .iter()
        .flat_map(|p| [p.id.clone(), format!("{}#foot", p.id)])
        .collect();
    let blocks: Vec<SpecBlock> = parsed
        .boxes
        .iter()
        .map(|b| SpecBlock {
            id: b.id.clone(),
            title: b.title.clone(),
            members: b
                .members
                .iter()
                .flat_map(|m| [m.clone(), format!("{m}#foot")])
                .collect(),
            dashed: false,
        })
        .collect();
    clusters::Tree::from_blocks(&blocks, |id| ids.contains(id))
}

/// Which message indices fall inside which frame, replayed from the **parser's** event stream.
///
/// A sequence group's membership is positional, so it cannot be derived from the geometry without
/// asking the geometry the very question under test. Replaying the stream is cheap and is not a
/// second copy of the layout: it counts `BlockStart`/`BlockEnd` and nothing else. The frame ids
/// match [`super::sequence`]'s, which numbers them in the order they open.
fn messages_in_frames(src: &str) -> HashMap<String, Vec<usize>> {
    let parsed = parsed(src);
    let mut open: Vec<String> = Vec::new();
    let mut out: HashMap<String, Vec<usize>> = HashMap::new();
    let mut next_frame = 0usize;
    let mut message = 0usize;
    for e in &parsed.events {
        match e {
            Event::BlockStart { .. } => {
                open.push(format!("g#{next_frame}"));
                next_frame += 1;
            }
            Event::BlockEnd => {
                open.pop();
            }
            Event::Message(_) => {
                for id in &open {
                    out.entry(id.clone()).or_default().push(message);
                }
                message += 1;
            }
            _ => {}
        }
    }
    out
}

// ---------------------------------------------------------------------------------------------
// 1. The main instrument: what konoma measured vs what resvg drew
// ---------------------------------------------------------------------------------------------

/// **§6's instrument, for every new kind of label stage 4 introduced.**
///
/// Each case emits an SVG through the production entry point, puts it through usvg with konoma's
/// own font database, and compares the box konoma sized against the width resvg laid the text out
/// at. The relation is exact and declared per kind, so a padding constant that drifts fails here
/// rather than being absorbed.
///
/// This is konoma against **resvg's real shaper**, so it cannot decay into konoma versus konoma —
/// the failure mode §6 exists to prevent.
#[test]
fn participant_box_width_matches_resvg() {
    if !text_metrics::fonts_available() {
        return;
    }
    let pad = super::shapes::PARTICIPANT_PAD_X * 2.0;
    for (name, text) in LABELS {
        let src = format!("sequenceDiagram\n  participant A as {text}");
        let drawn = drawn_width(&src, text);
        let d = laid_out(&src);
        let node = d.node("A").expect("the head box is placed");
        let expected = (drawn + pad).max(super::shapes::PARTICIPANT_MIN_WIDTH);
        assert!(
            (node.size.w - expected).abs() <= 1.0,
            "{name} {text:?}: box is {} wide, resvg drew the text {} wide, so the box should be {}",
            svg::num(node.size.w),
            svg::num(drawn),
            svg::num(expected)
        );
    }
}

/// The same comparison for an `actor`'s name, which is sized through its [`Panel`] instead.
///
/// [`Panel`]: super::panel::Panel
#[test]
fn actor_name_width_matches_resvg() {
    if !text_metrics::fonts_available() {
        return;
    }
    let pad = super::shapes::PARTICIPANT_PAD_X * 2.0;
    for (name, text) in LABELS {
        let src = format!("sequenceDiagram\n  actor A as {text}");
        let drawn = drawn_width(&src, text);
        let d = laid_out(&src);
        let node = d.node("A").expect("the head figure is placed");
        let expected = (drawn + pad).max(super::shapes::PARTICIPANT_MIN_WIDTH);
        assert!(
            (node.size.w - expected).abs() <= 1.0,
            "{name} {text:?}: actor is {} wide, resvg drew the name {} wide, expected {}",
            svg::num(node.size.w),
            svg::num(drawn),
            svg::num(expected)
        );
        // …and the panel really is the node's own size, which is what (12) then checks the rows
        // against.
        let panel = node.panel.as_ref().expect("an actor carries a panel");
        assert!((panel.size.w - node.size.w).abs() < 0.01);
    }
}

/// A message label's box is the measured text plus the shared edge-label padding.
#[test]
fn message_label_box_matches_resvg() {
    if !text_metrics::fonts_available() {
        return;
    }
    for (name, text) in LABELS {
        let src = format!("sequenceDiagram\n  a->>b: {text}");
        let drawn = drawn_width(&src, text);
        let d = laid_out(&src);
        let label = d.edges[0].label.as_ref().expect("the message carries one");
        let slack = label.size.w - drawn;
        assert!(
            (slack - super::LABEL_PAD_X * 2.0).abs() <= 1.0,
            "{name} {text:?}: label box {} vs resvg's {} → {} of padding, declared {}",
            svg::num(label.size.w),
            svg::num(drawn),
            svg::num(slack),
            svg::num(super::LABEL_PAD_X * 2.0)
        );
    }
}

/// A note's box is the measured text plus the shared note padding.
#[test]
fn note_box_width_matches_resvg() {
    if !text_metrics::fonts_available() {
        return;
    }
    let pad = super::shapes::PADDING * 2.0;
    for (name, text) in LABELS {
        let src = format!("sequenceDiagram\n  participant a\n  Note over a: {text}");
        let drawn = drawn_width(&src, text);
        let d = laid_out(&src);
        let note = d.node("note#0").expect("the note is placed");
        let expected = (drawn + pad).max(super::shapes::NOTE_FOLD * 3.0);
        assert!(
            (note.size.w - expected).abs() <= 1.0,
            "{name} {text:?}: note is {} wide, resvg drew the text {} wide, expected {}",
            svg::num(note.size.w),
            svg::num(drawn),
            svg::num(expected)
        );
    }
}

/// A group frame is at least as wide as the title drawn on it, plus the shared title padding.
#[test]
fn group_frame_title_fits_the_frame_resvg_draws() {
    if !text_metrics::fonts_available() {
        return;
    }
    for (name, text) in LABELS {
        let src = format!("sequenceDiagram\n  loop {text}\n    a->>b: x\n  end");
        let drawn = drawn_width(&src, &format!("loop {text}"));
        let d = laid_out(&src);
        let frame = d.cluster("g#0").expect("the frame is placed");
        assert!(
            frame.size.w + 0.01 >= drawn + clusters::TITLE_PAD_X,
            "{name} {text:?}: frame is {} wide but resvg drew its title {} wide",
            svg::num(frame.size.w),
            svg::num(drawn)
        );
        // …and the title's own measured block is inside the frame.
        let t = frame.title_center();
        let title = (
            t.x - frame.title.width / 2.0,
            t.y - frame.title.height / 2.0,
            t.x + frame.title.width / 2.0,
            t.y + frame.title.height / 2.0,
        );
        let (l, top, r, b) = frame.bounds();
        assert!(
            title.0 >= l - 0.01 && title.2 <= r + 0.01 && title.1 >= top - 0.01 && title.3 <= b,
            "{name}: the frame title pokes out of its own frame"
        );
    }
}

/// An `autonumber` badge's disc is big enough for the digits resvg draws inside it.
#[test]
fn autonumber_badge_holds_the_number_resvg_draws() {
    if !text_metrics::fonts_available() {
        return;
    }
    for start in ["1", "10", "100", "1000", "99999"] {
        let src = format!("sequenceDiagram\n  autonumber {start} 1\n  a->>b: x");
        let drawn = drawn_width(&src, start);
        let d = laid_out(&src);
        let badge = d.edges[0].badge.as_ref().expect("a badge was placed");
        assert!(
            badge.size.w + 0.01 >= drawn + super::sequence::BADGE_PAD * 2.0,
            "{start}: badge is {} across but resvg drew the digits {} wide",
            svg::num(badge.size.w),
            svg::num(drawn)
        );
        assert_eq!(badge.size.w, badge.size.h, "{start}: the badge is a disc");
    }
}

// ---------------------------------------------------------------------------------------------
// 2. The shared invariants that apply (see the module docs for the seven that do not)
// ---------------------------------------------------------------------------------------------

#[test]
fn shared_invariants_hold_over_the_corpus() {
    if !text_metrics::fonts_available() {
        return;
    }
    for (name, src) in CASES {
        let d = laid_out(src);
        check_nodes_do_not_overlap(name, &d);
        check_edges_stay_out_of_shapes(name, &d);
        check_view_box_contains_everything(name, &d);
        check_nested_clusters_sit_inside_their_parent(name, &d);
        check_panels_stay_inside_their_box(name, &d);
        check_clusters_hold_their_members(name, &d, &box_tree(src));
    }
}

// ---------------------------------------------------------------------------------------------
// 3. The invariants that are meaningful for a sequence diagram
// ---------------------------------------------------------------------------------------------

/// **Every message's y is strictly greater than the previous message's.**
///
/// The one property a sequence diagram exists to express: this happened, then that happened. A
/// self-message occupies a range rather than a line, so the comparison is against the *bottom* of
/// the message before.
#[test]
fn messages_run_strictly_down_the_page() {
    if !text_metrics::fonts_available() {
        return;
    }
    for (name, src) in CASES {
        let d = laid_out(src);
        let mut floor = f64::NEG_INFINITY;
        for e in &d.edges {
            let top = e.points.iter().map(|p| p.y).fold(f64::INFINITY, f64::min);
            let bottom = e
                .points
                .iter()
                .map(|p| p.y)
                .fold(f64::NEG_INFINITY, f64::max);
            assert!(
                top > floor,
                "{name}: {} -> {} starts at y={} but the message before it reached y={}",
                e.from,
                e.to,
                svg::num(top),
                svg::num(floor)
            );
            floor = bottom;
        }
        // …and they are in the order the author wrote them.
        let source = parsed(src);
        let written: Vec<(&str, &str)> = source
            .messages()
            .map(|m| (m.from.as_str(), m.to.as_str()))
            .collect();
        let drawn: Vec<(&str, &str)> = d
            .edges
            .iter()
            .map(|e| (e.from.as_str(), e.to.as_str()))
            .collect();
        assert_eq!(written, drawn, "{name}: messages are not in source order");
    }
}

/// **A message endpoint sits on its participant's lifeline.**
///
/// Precisely: its x is the axis itself, or the near edge of an activation bar open on that axis at
/// that y, or the edge of the participant's own box when this is the message that *creates* it.
/// Anything else means the message was drawn against the wrong participant.
#[test]
fn every_message_ends_on_its_participants_axis() {
    if !text_metrics::fonts_available() {
        return;
    }
    for (name, src) in CASES {
        let d = laid_out(src);
        for e in &d.edges {
            for (which, id, p) in [
                ("start", &e.from, e.points.first().expect("has points")),
                ("end", &e.to, e.points.last().expect("has points")),
            ] {
                let l = lifeline(&d, id);
                let mut allowed = vec![l.x];
                for i in 0..l.activations.len() {
                    let Some((left, top, right, bottom)) = l.activation_bounds(i) else {
                        continue;
                    };
                    if p.y >= top - 0.01 && p.y <= bottom + 0.01 {
                        allowed.push(left);
                        allowed.push(right);
                    }
                }
                // The head box, when this y is the one it was created at.
                if let Some(n) = d.node(id) {
                    let (bl, bt, br, bb) = n.bounds();
                    if p.y >= bt - 0.01 && p.y <= bb + 0.01 {
                        allowed.push(bl);
                        allowed.push(br);
                    }
                }
                assert!(
                    allowed.iter().any(|a| (a - p.x).abs() <= 0.01),
                    "{name}: the {which} of {} -> {} is at x={} but {id}'s axis is at {} \
                     (allowed: {:?})",
                    e.from,
                    e.to,
                    svg::num(p.x),
                    svg::num(l.x),
                    allowed.iter().map(|a| svg::num(*a)).collect::<Vec<_>>()
                );
            }
        }
    }
}

/// **An activation bar lies on its lifeline, and spans from a message that touched that
/// participant to another one.**
///
/// The first half is structural — a bar is stored *on* its lifeline — so what is stated here is
/// where it sits across the axis (which is what the depth staircase decides) and that both its
/// ends line up with something that actually happened to this participant. A bar copied onto the
/// wrong lifeline fails the second half: the y it opens at belongs to a message that does not name
/// the participant it is drawn under.
#[test]
fn every_activation_bar_belongs_to_its_own_lifeline() {
    if !text_metrics::fonts_available() {
        return;
    }
    for (name, src) in CASES {
        let d = laid_out(src);
        // How many bars each participant should have, replayed from the parser's own event stream
        // — the same trick as `messages_in_frames`, and the thing that catches a bar drawn under
        // the wrong column even when the two columns happen to share a message's y.
        let mut expected: HashMap<String, usize> = HashMap::new();
        for e in &parsed(src).events {
            if let Event::Activate(id) = e {
                *expected.entry(id.clone()).or_default() += 1;
            }
        }
        for l in &d.lifelines {
            assert_eq!(
                l.activations.len(),
                expected.get(&l.id).copied().unwrap_or(0),
                "{name}: {} carries {} bars but the source activates it {} time(s)",
                l.id,
                l.activations.len(),
                expected.get(&l.id).copied().unwrap_or(0)
            );
            // Every y at which something happened to this participant.
            let mut moments: Vec<f64> = vec![l.top, l.bottom];
            for e in &d.edges {
                if e.from != l.id && e.to != l.id {
                    continue;
                }
                for p in &e.points {
                    moments.push(p.y);
                }
            }
            for (i, a) in l.activations.iter().enumerate() {
                let (left, top, right, bottom) =
                    l.activation_bounds(i).expect("the bar has bounds");
                assert!(
                    (left - (l.x - ACTIVATION_WIDTH / 2.0 + a.depth as f64 * ACTIVATION_WIDTH))
                        .abs()
                        < 0.01,
                    "{name}: a depth-{} bar on {} is at x={}, not on its own staircase",
                    a.depth,
                    l.id,
                    svg::num(left)
                );
                assert!(
                    (right - left - ACTIVATION_WIDTH).abs() < 0.01,
                    "{name}: a bar on {} is {} wide",
                    l.id,
                    svg::num(right - left)
                );
                assert!(
                    bottom >= top,
                    "{name}: a bar on {} ends above where it started",
                    l.id
                );
                assert!(
                    top >= l.top - 0.01 && bottom <= l.bottom + 0.01,
                    "{name}: a bar on {} runs from {} to {} outside its lifeline {}..{}",
                    l.id,
                    svg::num(top),
                    svg::num(bottom),
                    svg::num(l.top),
                    svg::num(l.bottom)
                );
                for (which, y) in [("top", top), ("bottom", bottom)] {
                    assert!(
                        moments.iter().any(|m| (m - y).abs() < 0.01),
                        "{name}: a bar on {} has its {which} at y={}, which is not a moment \
                         anything happened to {} (it happened at {:?})",
                        l.id,
                        svg::num(y),
                        l.id,
                        moments.iter().map(|m| svg::num(*m)).collect::<Vec<_>>()
                    );
                }
            }
            // Two bars on one lifeline never overlap: the staircase is what makes nesting legible.
            for i in 0..l.activations.len() {
                for j in (i + 1)..l.activations.len() {
                    let (al, at, ar, ab) = l.activation_bounds(i).expect("bounds");
                    let (bl, bt, br, bb) = l.activation_bounds(j).expect("bounds");
                    let (dx, dy) = overlap((al, at, ar, ab), (bl, bt, br, bb));
                    assert!(
                        dx <= 0.01 || dy <= 0.01,
                        "{name}: two bars on {} overlap by {}x{}",
                        l.id,
                        svg::num(dx),
                        svg::num(dy)
                    );
                }
            }
        }
    }
}

/// The stacked-activation case from mermaid's documentation, stated as exact spans.
///
/// The corpus-wide invariant above says a bar's ends land on *some* moment; this says **which**.
/// It is the witness for the mutation "an activation bar on the wrong lifeline", which the loose
/// version can miss when two participants exchange messages at the same y.
#[test]
fn a_stacked_activation_spans_exactly_the_messages_that_opened_and_closed_it() {
    if !text_metrics::fonts_available() {
        return;
    }
    let src = "sequenceDiagram\n    Alice->>+John: one\n    Alice->>+John: two\n    \
               John-->>-Alice: three\n    John-->>-Alice: four";
    let d = laid_out(src);
    let ys: Vec<f64> = d.edges.iter().map(|e| e.points[0].y).collect();
    assert_eq!(ys.len(), 4);

    let john = lifeline(&d, "John");
    assert_eq!(john.activations.len(), 2, "John is activated twice");
    // The outer bar: opened by message 1, closed by message 4.
    assert!((john.activations[0].top - ys[0]).abs() < 0.01);
    assert!((john.activations[0].bottom - ys[3]).abs() < 0.01);
    assert_eq!(john.activations[0].depth, 0);
    // The inner bar: opened by message 2, closed by message 3, and one width to the right.
    assert!((john.activations[1].top - ys[1]).abs() < 0.01);
    assert!((john.activations[1].bottom - ys[2]).abs() < 0.01);
    assert_eq!(john.activations[1].depth, 1);

    // Alice is never activated — `-` deactivates the sender, and she never sends a `-`.
    assert!(
        lifeline(&d, "Alice").activations.is_empty(),
        "the bars are on John, not on Alice"
    );
}

/// **A group frame encloses every message inside it**, and its title band sits above all of them.
#[test]
fn a_group_frame_encloses_every_message_inside_it() {
    if !text_metrics::fonts_available() {
        return;
    }
    for (name, src) in CASES {
        let d = laid_out(src);
        let inside = messages_in_frames(src);
        for (frame_id, messages) in &inside {
            let Some(frame) = d.cluster(frame_id) else {
                panic!("{name}: {frame_id} holds messages but no frame was placed");
            };
            let (l, t, r, b) = frame.bounds();
            for i in messages {
                let e = &d.edges[*i];
                for p in &e.points {
                    assert!(
                        p.x >= l - 0.01 && p.x <= r + 0.01 && p.y >= t - 0.01 && p.y <= b + 0.01,
                        "{name}: {frame_id} does not enclose {} -> {} (waypoint at {}, {} vs \
                         frame {}..{} x {}..{})",
                        e.from,
                        e.to,
                        svg::num(p.x),
                        svg::num(p.y),
                        svg::num(l),
                        svg::num(r),
                        svg::num(t),
                        svg::num(b)
                    );
                }
                if let Some(label) = &e.label {
                    let (ll, lt, lr, lb) = label_bounds(label);
                    assert!(
                        ll >= l - 0.01 && lr <= r + 0.01 && lt >= t - 0.01 && lb <= b + 0.01,
                        "{name}: {frame_id} does not enclose the label of {} -> {}",
                        e.from,
                        e.to
                    );
                }
            }
        }
    }
}

/// A message that is *not* inside a frame does not stray into it.
///
/// This is the positional counterpart of (9), which cannot be used here — see the module docs.
#[test]
fn a_message_outside_a_frame_stays_out_of_it() {
    if !text_metrics::fonts_available() {
        return;
    }
    for (name, src) in CASES {
        let d = laid_out(src);
        let inside = messages_in_frames(src);
        for (i, e) in d.edges.iter().enumerate() {
            for c in &d.clusters {
                // A `box` frame is a vertical band over the participants and every message
                // between two of them is inside it by construction; only group frames are
                // positional.
                if !c.dashed {
                    continue;
                }
                if inside.get(&c.id).is_some_and(|ms| ms.contains(&i)) {
                    continue;
                }
                let (l, t, r, b) = c.bounds();
                for p in &e.points {
                    let within = p.x >= l && p.x <= r && p.y >= t && p.y <= b;
                    assert!(
                        !within,
                        "{name}: {} -> {} is not inside {} but a waypoint of it is ({}, {})",
                        e.from,
                        e.to,
                        c.id,
                        svg::num(p.x),
                        svg::num(p.y)
                    );
                }
            }
        }
    }
}

/// **Nothing is drawn outside the diagram's bounds** — including the geometry the shared check (5)
/// does not know about: lifelines, activation bars, frames and sequence numbers.
#[test]
fn nothing_is_drawn_outside_the_view_box() {
    if !text_metrics::fonts_available() {
        return;
    }
    for (name, src) in CASES {
        let d = laid_out(src);
        let check = |x: f64, y: f64, what: &str| {
            assert!(
                x >= -0.01 && y >= -0.01 && x <= d.width + 0.01 && y <= d.height + 0.01,
                "{name}: {what} at ({}, {}) is outside the {}x{} viewBox",
                svg::num(x),
                svg::num(y),
                svg::num(d.width),
                svg::num(d.height)
            );
        };
        for l in &d.lifelines {
            check(l.x, l.top, "lifeline");
            check(l.x, l.bottom, "lifeline");
            let (left, right) = l.extent();
            check(left, l.top, "activation");
            check(right, l.bottom, "activation");
        }
        for c in &d.clusters {
            let (l, t, r, b) = c.bounds();
            check(l, t, &format!("frame {}", c.id));
            check(r, b, &format!("frame {}", c.id));
            for s in &c.sections {
                check(l, s.y, "section rule");
            }
        }
        for e in &d.edges {
            for b in [&e.badge, &e.label].into_iter().flatten() {
                let (l, t, r, bb) = label_bounds(b);
                check(l, t, "edge label");
                check(r, bb, "edge label");
            }
        }
        // …and the margin really is there.
        let left = d
            .lifelines
            .iter()
            .map(|l| l.x)
            .fold(f64::INFINITY, f64::min);
        assert!(
            left >= super::MARGIN,
            "{name}: an axis is left of the margin"
        );
    }
}

/// A lifeline hangs from its own participant's box and stops at the foot — or at the cross that
/// says the participant was destroyed.
#[test]
fn a_lifeline_hangs_from_its_own_box() {
    if !text_metrics::fonts_available() {
        return;
    }
    for (name, src) in CASES {
        let d = laid_out(src);
        for l in &d.lifelines {
            let head = d
                .node(&l.id)
                .unwrap_or_else(|| panic!("{name}: no head box for {}", l.id));
            let (_, _, _, head_bottom) = head.bounds();
            assert!(
                (l.top - head_bottom).abs() < 0.01,
                "{name}: {}'s axis starts at {} but its box ends at {}",
                l.id,
                svg::num(l.top),
                svg::num(head_bottom)
            );
            assert!(
                (head.center.x - l.x).abs() < 0.01,
                "{name}: {}'s box is not centred on its own axis",
                l.id
            );
            match d.node(&format!("{}#foot", l.id)) {
                Some(foot) => {
                    let (_, foot_top, _, _) = foot.bounds();
                    assert!(
                        (l.bottom - foot_top).abs() < 0.01,
                        "{name}: {}'s axis ends at {} but its mirrored box starts at {}",
                        l.id,
                        svg::num(l.bottom),
                        svg::num(foot_top)
                    );
                    assert!(!l.destroyed, "{name}: {} is mirrored and destroyed", l.id);
                }
                None => assert!(
                    l.destroyed,
                    "{name}: {} has no mirrored box and was not destroyed",
                    l.id
                ),
            }
            assert!(l.bottom > l.top, "{name}: {}'s axis has no length", l.id);
        }
    }
}

/// The columns are in the order the parser produced, left to right, and none of the boxes touch.
#[test]
fn the_columns_keep_the_order_the_author_wrote() {
    if !text_metrics::fonts_available() {
        return;
    }
    for (name, src) in CASES {
        let d = laid_out(src);
        let source = parsed(src);
        let written: Vec<&str> = source.participants.iter().map(|p| p.id.as_str()).collect();
        let drawn: Vec<&str> = d.lifelines.iter().map(|l| l.id.as_str()).collect();
        assert_eq!(written, drawn, "{name}: the columns were reordered");
        for w in d.lifelines.windows(2) {
            assert!(
                w[1].x > w[0].x,
                "{name}: {} is not to the left of {}",
                w[0].id,
                w[1].id
            );
        }
    }
}

/// A message label sits **above** its own arrow, centred on it — which is why shared check (3)
/// does not apply.
#[test]
fn a_message_label_sits_above_its_own_arrow() {
    if !text_metrics::fonts_available() {
        return;
    }
    for (name, src) in CASES {
        let d = laid_out(src);
        for e in &d.edges {
            let Some(label) = &e.label else { continue };
            if e.from == e.to {
                // A self-message's loop has nothing above it, so its label goes beside it.
                let (l, _, _, _) = label_bounds(label);
                let right = e
                    .points
                    .iter()
                    .map(|p| p.x)
                    .fold(f64::NEG_INFINITY, f64::max);
                assert!(
                    l >= right - 0.01,
                    "{name}: the label of the self-message on {} is not clear of its loop",
                    e.from
                );
                continue;
            }
            let line_y = e.points[0].y;
            let (l, _, r, b) = label_bounds(label);
            assert!(
                b <= line_y + 0.01,
                "{name}: the label of {} -> {} reaches y={} but the arrow is at {}",
                e.from,
                e.to,
                svg::num(b),
                svg::num(line_y)
            );
            let mid = (e.points[0].x + e.points[1].x) / 2.0;
            assert!(
                ((l + r) / 2.0 - mid).abs() < 0.01,
                "{name}: the label of {} -> {} is not centred on its arrow",
                e.from,
                e.to
            );
        }
    }
}

/// A note is drawn on the side the author named, and a `note over A,B` really does span the two.
#[test]
fn a_note_is_drawn_where_the_author_put_it() {
    if !text_metrics::fonts_available() {
        return;
    }
    let src = "sequenceDiagram\n  participant Alice\n  participant John\n  \
               Note left of Alice: L\n  Note right of Alice: R\n  Note over John: O\n  \
               Note over Alice,John: Both";
    let d = laid_out(src);
    let (alice, john) = (lifeline(&d, "Alice").x, lifeline(&d, "John").x);
    let note = |i: usize| d.node(&format!("note#{i}")).expect("placed");

    let (_, _, r, _) = note(0).bounds();
    assert!(r < alice, "`left of` is drawn left of the axis");
    let (l, _, _, _) = note(1).bounds();
    assert!(l > alice, "`right of` is drawn right of the axis");
    assert!(
        (note(2).center.x - john).abs() < 0.01,
        "`over A` is centred on A's axis"
    );
    let (l, _, r, _) = note(3).bounds();
    assert!(
        l < alice && r > john,
        "`over A,B` spans both axes ({}..{} vs {}..{})",
        svg::num(l),
        svg::num(r),
        svg::num(alice),
        svg::num(john)
    );
    assert!(
        (note(3).center.x - (alice + john) / 2.0).abs() < 0.01,
        "`over A,B` is centred between them"
    );
}

/// A frame's title band is above everything the frame contains, so the words are never crossed by
/// a message. This is the property shared check (11) states for a subgraph.
#[test]
fn a_frame_title_stays_clear_of_everything_inside_it() {
    if !text_metrics::fonts_available() {
        return;
    }
    for (name, src) in CASES {
        let d = laid_out(src);
        let inside = messages_in_frames(src);
        for c in &d.clusters {
            if !c.dashed || c.title.is_blank() {
                continue;
            }
            let t = c.title_center();
            let title = (
                t.x - c.title.width / 2.0,
                t.y - c.title.height / 2.0,
                t.x + c.title.width / 2.0,
                t.y + c.title.height / 2.0,
            );
            for i in inside.get(&c.id).into_iter().flatten() {
                let e = &d.edges[*i];
                for p in &e.points {
                    assert!(
                        p.y > title.3 + 0.01,
                        "{name}: the title of {} reaches y={} and {} -> {} is at y={}",
                        c.id,
                        svg::num(title.3),
                        e.from,
                        e.to,
                        svg::num(p.y)
                    );
                }
                if let Some(l) = &e.label {
                    let (dx, dy) = overlap(title, label_bounds(l));
                    assert!(
                        dx <= 0.01 || dy <= 0.01,
                        "{name}: the title of {} sits on the label of {} -> {}",
                        c.id,
                        e.from,
                        e.to
                    );
                }
            }
            // Each section's own rule and title are inside the frame, below its title, in order.
            let mut previous = title.3;
            for s in &c.sections {
                assert!(
                    s.y > previous,
                    "{name}: a section rule of {} is at y={}, above what came before it",
                    c.id,
                    svg::num(s.y)
                );
                previous = s.y + s.title.height;
                let (_, top, _, bottom) = c.bounds();
                assert!(
                    s.y > top && s.y < bottom,
                    "{name}: a section rule of {} is outside its own frame",
                    c.id
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// 4. What each spelling draws
// ---------------------------------------------------------------------------------------------

/// Every arrow spelling reaches the drawing with the line style and the end marks it asked for.
///
/// The mutation this exists for is "a dashed reply drawn solid": `-->>` is the reply convention in
/// every sequence diagram anybody writes, and drawing it solid makes a round trip look like two
/// requests.
#[test]
fn every_arrow_spelling_reaches_the_drawing_intact() {
    if !text_metrics::fonts_available() {
        return;
    }
    let cases: &[(&str, Stroke, Tip, Tip)] = &[
        ("->", Stroke::Normal, Tip::None, Tip::None),
        ("-->", Stroke::Dotted, Tip::None, Tip::None),
        ("->>", Stroke::Normal, Tip::None, Tip::Arrow),
        ("-->>", Stroke::Dotted, Tip::None, Tip::Arrow),
        ("<<->>", Stroke::Normal, Tip::Arrow, Tip::Arrow),
        ("<<-->>", Stroke::Dotted, Tip::Arrow, Tip::Arrow),
        ("-x", Stroke::Normal, Tip::None, Tip::Cross),
        ("--x", Stroke::Dotted, Tip::None, Tip::Cross),
        ("-)", Stroke::Normal, Tip::None, Tip::Async),
        ("--)", Stroke::Dotted, Tip::None, Tip::Async),
        // The half-arrows, folded (divergence 4). `_REVERSE` moves the head to the sending end.
        ("-|\\", Stroke::Normal, Tip::None, Tip::Arrow),
        ("--|/", Stroke::Dotted, Tip::None, Tip::Arrow),
        ("/|-", Stroke::Normal, Tip::Arrow, Tip::None),
        ("\\\\--", Stroke::Dotted, Tip::Arrow, Tip::None),
    ];
    for (spelling, stroke, start, end) in cases {
        let src = format!("sequenceDiagram\n  A{spelling}B: hi");
        let d = laid_out(&src);
        let e = &d.edges[0];
        assert_eq!(e.stroke, *stroke, "{spelling}: line style");
        assert_eq!(
            (e.tip_start, e.tip_end),
            (*start, *end),
            "{spelling}: marks"
        );

        // …and the SVG says the same thing. Both participants are boxes, so a `<polygon>` can only
        // be a head and a dash pattern on a `<path>` can only be the shaft.
        let svg_text = render(&src, "dark").expect("renders");
        let heads = svg_text.matches("<polygon").count();
        let expected_heads = [start, end]
            .iter()
            .filter(|t| ***t == Tip::Arrow || ***t == Tip::Async)
            .count();
        assert_eq!(heads, expected_heads, "{spelling}: heads drawn");
        let dashed =
            svg_text.contains(&format!("stroke-dasharray=\"{}\"", super::svg::DOTTED_DASH));
        assert_eq!(
            dashed,
            *stroke == Stroke::Dotted,
            "{spelling}: a reply drawn solid reads as a second request"
        );
    }
}

/// `autonumber` counts the way the source asked it to, and `off` stops it.
#[test]
fn autonumber_counts_the_way_the_source_asked() {
    if !text_metrics::fonts_available() {
        return;
    }
    let numbers = |src: &str| -> Vec<Option<String>> {
        laid_out(src)
            .edges
            .iter()
            .map(|e| e.badge.as_ref().map(|b| b.label.lines.join("")))
            .collect()
    };
    assert_eq!(
        numbers("sequenceDiagram\n  autonumber\n  A->>B: a\n  B->>A: b\n  A->>B: c"),
        vec![
            Some("1".to_string()),
            Some("2".to_string()),
            Some("3".to_string())
        ]
    );
    assert_eq!(
        numbers("sequenceDiagram\n  autonumber 10 5\n  A->>B: a\n  B->>A: b\n  A->>B: c"),
        vec![
            Some("10".to_string()),
            Some("15".to_string()),
            Some("20".to_string())
        ]
    );
    // Two decimals are legal, and a whole number never grows a `.00`.
    assert_eq!(
        numbers("sequenceDiagram\n  autonumber 1.5 0.25\n  A->>B: a\n  B->>A: b"),
        vec![Some("1.5".to_string()), Some("1.75".to_string())]
    );
    // Off by default, and `off` turns it back off part-way through.
    assert_eq!(numbers("sequenceDiagram\n  A->>B: a"), vec![None]);
    assert_eq!(
        numbers("sequenceDiagram\n  autonumber\n  A->>B: a\n  autonumber off\n  B->>A: b"),
        vec![Some("1".to_string()), None]
    );
}

/// A created participant's box appears at the message that creates it, and a destroyed one's axis
/// stops at the message that destroys it. Neither is drawn by the crate this replaces.
#[test]
fn create_and_destroy_move_the_box_and_cut_the_axis() {
    if !text_metrics::fonts_available() {
        return;
    }
    let src = "sequenceDiagram\n    Alice->>Bob: one\n    create participant Carl\n    \
               Alice->>Carl: two\n    destroy Carl\n    Alice-xCarl: three";
    let d = laid_out(src);
    let ys: Vec<f64> = d.edges.iter().map(|e| e.points[0].y).collect();

    // Alice and Bob are at the head; Carl is not.
    let alice = d.node("Alice").expect("placed");
    let carl = d.node("Carl").expect("placed");
    assert!(
        carl.center.y > alice.center.y,
        "a created participant is not drawn at the head"
    );
    assert!(
        (carl.center.y - ys[1]).abs() < 0.01,
        "Carl's box is centred on the message that created him ({} vs {})",
        svg::num(carl.center.y),
        svg::num(ys[1])
    );
    // …and his axis stops at the message that destroyed him, with a cross.
    let l = lifeline(&d, "Carl");
    assert!(l.destroyed);
    assert!((l.bottom - ys[2]).abs() < 0.01);
    assert!(
        d.node("Carl#foot").is_none(),
        "a destroyed participant is not mirrored"
    );
    assert!(d.node("Bob#foot").is_some(), "a living one is");
}

/// `actor` is a stick figure and `participant` is a box. The crate draws a box for both, so this
/// is a distinction a reader gets from konoma and did not get before.
#[test]
fn an_actor_is_drawn_as_a_figure_and_a_participant_as_a_box() {
    if !text_metrics::fonts_available() {
        return;
    }
    let d = laid_out("sequenceDiagram\n  actor A\n  participant B\n  A->>B: x");
    assert_eq!(d.node("A").expect("placed").shape, Glyph::Actor);
    assert_eq!(d.node("B").expect("placed").shape, Glyph::Participant);
    // The figure is taller than the box, and both axes still start together.
    assert!(d.node("A").unwrap().size.h > d.node("B").unwrap().size.h);
    assert!((lifeline(&d, "A").top - lifeline(&d, "B").top).abs() < 0.01);

    let svg_text = render(
        "sequenceDiagram\n  actor A\n  participant B\n  A->>B: x",
        "dark",
    )
    .expect("renders");
    assert!(
        svg_text.contains("<circle"),
        "the figure's head is a circle and nothing else in this diagram is"
    );

    // The name sits **below** the figure's band, which is the whole reason an actor carries a
    // panel rather than a centred label: a name drawn over the figure is unreadable, and
    // `check_panels_stay_inside_their_box` cannot see it because it is still inside the box.
    let panel = d
        .node("A")
        .unwrap()
        .panel
        .as_ref()
        .expect("an actor has one");
    let row = panel.rows.first().expect("one row: the name");
    assert!(
        row.y - labels::line_height() / 2.0 >= super::shapes::ACTOR_FIGURE_HEIGHT - 0.01,
        "the name is drawn at y={} inside the figure's own {}px band",
        svg::num(row.y),
        svg::num(super::shapes::ACTOR_FIGURE_HEIGHT)
    );
}

/// A sequence message label is drawn with **nothing behind it**, and a flowchart's is drawn on a
/// patch — because a flowchart's label sits on its own line and this one does not.
///
/// Stated as a named test rather than left to the golden: `docs/STATUS.md` records what happens to
/// behaviour only a golden holds, and this rule lives in shared code that every diagram kind goes
/// through. It also closes the recorded "the edge-label plate reads as grey on a transparent
/// ground" gap for the one placement where the patch was never needed.
#[test]
fn a_message_label_gets_no_patch_and_a_flowchart_label_still_does() {
    if !text_metrics::fonts_available() {
        return;
    }
    let patch = |out: &str| {
        let fill = format!("fill=\"{}\"", theme::DARK.background_ref);
        out.lines()
            .filter(|l| l.starts_with("<rect ") && l.contains(&fill))
            .count()
    };
    let sequence = render("sequenceDiagram\n  A->>B: hello", "dark").expect("renders");
    assert_eq!(
        patch(&sequence),
        0,
        "a sequence label sits above its arrow, so a patch behind it is a grey slab and nothing \
         else:\n{sequence}"
    );
    let flow = super::render("flowchart TD\n  A --> |hello| B", "dark").expect("renders");
    assert_eq!(
        patch(&flow),
        1,
        "a flowchart label sits *on* its line and would be illegible without one:\n{flow}"
    );
    // …and a self-message's label is beside its loop, so it needs none either.
    let loop_back = render("sequenceDiagram\n  A->>A: retry", "dark").expect("renders");
    assert_eq!(patch(&loop_back), 0);
}

/// A `box` groups participants into a vertical band with its title above them.
#[test]
fn a_box_frames_its_participants_and_keeps_its_title() {
    if !text_metrics::fonts_available() {
        return;
    }
    let src = "sequenceDiagram\n  box Purple Alice & John\n  participant A\n  participant J\n  \
               end\n  participant Z\n  A->>J: x\n  J->>Z: y";
    let d = laid_out(src);
    let frame = d.cluster("box0").expect("the box frame is placed");
    assert_eq!(frame.title.lines.join(""), "Alice & John");
    assert!(frame.filled, "a `box` is ground, so it is painted");
    assert!(!frame.dashed);
    let (l, t, r, b) = frame.bounds();
    for id in ["A", "J", "A#foot", "J#foot"] {
        let n = d.node(id).expect("placed");
        let (nl, nt, nr, nb) = n.bounds();
        assert!(
            nl >= l && nr <= r && nt >= t && nb <= b,
            "{id} is not inside its box"
        );
    }
    // Z is outside it.
    let z = d.node("Z").expect("placed");
    assert!(
        z.bounds().0 > r,
        "a participant outside the box stays outside"
    );
    // The title is above the participants, not on them.
    let title_bottom = frame.title_center().y + frame.title.height / 2.0;
    assert!(title_bottom <= d.node("A").unwrap().bounds().1 + 0.01);
}

/// Things the parser reads and the renderer deliberately does not draw. A statement that is
/// dropped must leave **nothing** behind — no phantom column, no stray box.
#[test]
fn parsed_and_not_drawn_leaves_nothing_on_the_page() {
    if !text_metrics::fonts_available() {
        return;
    }
    let plain =
        laid_out("sequenceDiagram\n  participant Alice\n  participant John\n  Alice->>John: Hello");
    let shape_of = |d: &Diagram| -> (usize, usize, usize, usize) {
        (
            d.nodes.len(),
            d.edges.len(),
            d.clusters.len(),
            d.lifelines.len(),
        )
    };
    for (name, src) in [
        (
            "actor menus",
            "sequenceDiagram\n  participant Alice\n  participant John\n  \
             link Alice: Dashboard @ https://x.test/a\n  \
             links John: {\"Wiki\": \"https://x.test/j\"}\n  \
             properties Alice: {\"class\": \"internal\"}\n  \
             details John: {\"a\": 1}\n  Alice->>John: Hello",
        ),
        (
            "accessibility",
            "sequenceDiagram\n  participant Alice\n  participant John\n  accTitle: t\n  \
             accDescr: d\n  Alice->>John: Hello",
        ),
        (
            "comments and a directive",
            "%%{init: {'theme':'dark'}}%%\nsequenceDiagram\n  participant Alice\n  \
             participant John\n  %% a comment\n  # another\n  Alice->>John: Hello",
        ),
        (
            "central connections",
            "sequenceDiagram\n  participant Alice\n  participant John\n  Alice->>()John: Hello",
        ),
    ] {
        let d = laid_out(src);
        assert_eq!(
            shape_of(&d),
            shape_of(&plain),
            "{name}: a dropped statement changed what is drawn"
        );
    }
    // A `rect`'s colour is parsed and not painted: the frame's caption is the keyword, never
    // `rgb(191, 223, 255)`.
    let d = laid_out("sequenceDiagram\n  rect rgb(191, 223, 255)\n    A->>B: x\n  end");
    let frame = d.cluster("g#0").expect("placed");
    assert_eq!(frame.title.lines.join(""), "rect");
    // …and a `box`'s colour is not painted either: every palette paints it the same.
    let a = render(
        "sequenceDiagram\n  box Purple P\n  participant A\n  end\n  A->>B: x",
        "dark",
    )
    .expect("renders");
    let b = render(
        "sequenceDiagram\n  box Aqua P\n  participant A\n  end\n  A->>B: x",
        "dark",
    )
    .expect("renders");
    assert_eq!(a, b, "a box's colour reached the drawing");
}

// ---------------------------------------------------------------------------------------------
// 5. The SVG contract (§1) and the round trip through resvg
// ---------------------------------------------------------------------------------------------

#[test]
fn emitted_svg_obeys_the_renderer_contract() {
    if !text_metrics::fonts_available() {
        return;
    }
    for (name, src) in CASES {
        for theme_name in ["dark", "light", "classic", "forest", "neutral"] {
            let out = render(src, theme_name).expect("renders");
            assert!(
                out.contains("viewBox=\""),
                "{name}/{theme_name}: no viewBox"
            );
            assert!(!out.contains("foreignObject"), "{name}/{theme_name}");
            assert!(!out.contains("var(--"), "{name}/{theme_name}");
            assert!(!out.contains("<style"), "{name}/{theme_name}");
            assert!(
                out.contains("font-family=\"sans-serif\""),
                "{name}/{theme_name}: the drawing font must be the one that was measured"
            );
            let opaque_background = out
                .lines()
                .any(|l| l.starts_with("<rect width=") && !l.contains("fill=\"none\""));
            assert!(!opaque_background, "{name}/{theme_name}: opaque background");
        }
    }
}

#[test]
fn every_corpus_diagram_rasterises_with_ink_in_it() {
    if !text_metrics::fonts_available() {
        return;
    }
    for (name, src) in CASES {
        let out = render(src, "dark").expect("renders");
        let img = crate::preview::svg::rasterize_bytes(out.as_bytes(), FsPath::new("m.svg"), 600)
            .unwrap_or_else(|| panic!("{name}: konoma's resvg could not rasterise the output"));
        let opaque = img.to_rgba8().pixels().filter(|p| p.0[3] > 32).count();
        assert!(opaque > 500, "{name}: only {opaque} pixels were drawn");
    }
}

/// The three colours stage 4 added read on the ground they are drawn on.
#[test]
fn the_new_palette_entries_are_legible() {
    for t in theme::ALL {
        assert!(
            theme::parse_hex(t.lifeline).is_some()
                && theme::parse_hex(t.activation_fill).is_some()
                && theme::parse_hex(t.activation_stroke).is_some(),
            "{}: the sequence colours are not normalised hex",
            t.name
        );
        // A lifeline is composited straight onto the terminal, like `line` is, so it has to
        // survive a dark ground — §4-1's whole point.
        assert!(
            Theme::contrast(t.lifeline, "#000000") >= 2.0,
            "{}: lifeline {} vanishes on a dark terminal ({:.2})",
            t.name,
            t.lifeline,
            Theme::contrast(t.lifeline, "#000000")
        );
        // A bar is a filled shape with an outline, so the two have to be told apart.
        assert!(
            Theme::contrast(t.activation_stroke, t.activation_fill) >= 1.5,
            "{}: an activation bar's outline is invisible against its own fill",
            t.name
        );
        // …and a different colour from the messages, which is the decision the field exists to
        // record: an axis drawn exactly like an arrow turns the diagram into a grid.
        assert_ne!(
            t.lifeline, t.line,
            "{}: the axes and the messages are the same colour",
            t.name
        );
        const {
            assert!(
                super::svg::LIFELINE_STROKE_WIDTH < super::svg::EDGE_STROKE_WIDTH,
                "an axis is drawn thinner than a message"
            )
        };
    }
}

// ---------------------------------------------------------------------------------------------
// 6. Goldens (§6: SVG text, never a PNG)
// ---------------------------------------------------------------------------------------------

/// The regression net over the whole corpus, **through [`render`]** — the function production
/// calls. Numeric attribute values are masked, for the reason [`super::tests`]'s module docs give:
/// every coordinate descends from the system's sans-serif face.
#[test]
fn sequence_corpus_golden() {
    if !text_metrics::fonts_available() {
        return;
    }
    let mut out = String::new();
    for (name, src) in CASES {
        out.push_str(&format!("=== {name} ===\n"));
        out.push_str(&mask_numbers(&render(src, "dark").expect("renders")));
        out.push('\n');
    }
    assert_snapshot("mermaid_sequence_render", &out);
}

/// A diagram assembled by hand, so **no font is involved and every number is pinned exactly**, in
/// all five palettes. Covers every glyph and every terminator stage 4 added.
#[test]
fn sequence_emit_golden() {
    let d = synthetic_sequence();
    let mut out = String::new();
    for t in theme::ALL {
        out.push_str(&format!("=== {} ===\n", t.name));
        out.push_str(&svg::emit(&d, t));
        out.push('\n');
    }
    assert_snapshot("mermaid_sequence_emit", &out);
}

/// One of everything, at coordinates chosen by hand.
fn synthetic_sequence() -> Diagram {
    let label = |text: &str, w: f64| Label {
        lines: text.split('\n').map(str::to_string).collect(),
        width: w,
        height: text.split('\n').count() as f64 * labels::line_height(),
    };
    let participant = |id: &str, x: f64, y: f64| PlacedNode {
        id: id.to_string(),
        shape: Glyph::Participant,
        center: Point::new(x, y),
        size: Size::new(80.0, 36.0),
        label: label(id, 40.0),
        panel: None,
        series: None,
        mark: None,
        style: None,
    };
    let actor = |id: &str, x: f64, y: f64| PlacedNode {
        id: id.to_string(),
        shape: Glyph::Actor,
        center: Point::new(x, y),
        size: Size::new(80.0, 56.0),
        label: label("", 0.0),
        panel: Some(super::panel::Panel {
            rows: vec![super::panel::PanelRow {
                cells: vec![super::panel::PanelCell {
                    label: label(id, 40.0),
                    x: 40.0,
                    centered: true,
                }],
                y: 44.0,
            }],
            rules: Vec::new(),
            columns: Vec::new(),
            size: Size::new(80.0, 56.0),
        }),
        series: None,
        mark: None,
        style: None,
    };

    let mut nodes = vec![
        participant("A", 80.0, 40.0),
        actor("B", 240.0, 30.0),
        participant("A#foot", 80.0, 460.0),
        actor("B#foot", 240.0, 450.0),
        PlacedNode {
            id: "note#0".to_string(),
            shape: Glyph::Note,
            center: Point::new(160.0, 300.0),
            size: Size::new(180.0, 44.0),
            label: label("a note\nover both", 90.0),
            panel: None,
            series: None,
            mark: None,
            style: None,
        },
    ];
    nodes.push(PlacedNode {
        id: "title#".to_string(),
        shape: Glyph::Flow(Shape::Text),
        center: Point::new(160.0, 12.0),
        size: Size::new(120.0, 24.0),
        label: label("A title", 80.0),
        panel: None,
        series: None,
        mark: None,
        style: None,
    });

    // One message per terminator, solid and dotted, plus a self-loop and a badge.
    let tips = [
        (Tip::None, Tip::None, Stroke::Normal),
        (Tip::None, Tip::Arrow, Stroke::Dotted),
        (Tip::Arrow, Tip::Arrow, Stroke::Normal),
        (Tip::None, Tip::Cross, Stroke::Dotted),
        (Tip::None, Tip::Async, Stroke::Normal),
    ];
    let mut edges: Vec<PlacedEdge> = Vec::new();
    for (i, (start, end, stroke)) in tips.into_iter().enumerate() {
        let y = 100.0 + i as f64 * 30.0;
        let l = label("msg", 26.0);
        edges.push(PlacedEdge {
            from: "A".to_string(),
            to: "B".to_string(),
            points: vec![Point::new(80.0, y), Point::new(240.0, y)],
            tip_start: start,
            tip_end: end,
            stroke,
            label: Some(PlacedEdgeLabel {
                center: Point::new(160.0, y - 14.0),
                size: Size::new(26.0 + super::LABEL_PAD_X * 2.0, 16.0),
                label: l,
            }),
            start_label: None,
            end_label: None,
            badge: (i == 0).then(|| PlacedEdgeLabel {
                center: Point::new(92.0, y),
                size: Size::new(22.0, 22.0),
                label: label("1", 8.0),
            }),
            series: None,
            straight: false,
            overlay: false,
            style: None,
            curve: Curve::Basis,
            tip_matches_line: false,
        });
    }
    // A self-message: a loop out to the right and back.
    edges.push(PlacedEdge {
        from: "B".to_string(),
        to: "B".to_string(),
        points: vec![
            Point::new(240.0, 260.0),
            Point::new(282.0, 260.0),
            Point::new(282.0, 286.0),
            Point::new(240.0, 286.0),
        ],
        tip_start: Tip::None,
        tip_end: Tip::Arrow,
        stroke: Stroke::Normal,
        label: Some(PlacedEdgeLabel {
            center: Point::new(330.0, 273.0),
            size: Size::new(50.0, 16.0),
            label: label("retry", 42.0),
        }),
        start_label: None,
        end_label: None,
        badge: None,
        series: None,
        straight: false,
        overlay: false,
        style: None,
        curve: Curve::Basis,
        tip_matches_line: false,
    });

    let lifelines = vec![
        PlacedLifeline {
            id: "A".to_string(),
            x: 80.0,
            top: 58.0,
            bottom: 442.0,
            destroyed: false,
            activations: vec![
                PlacedActivation {
                    depth: 0,
                    top: 100.0,
                    bottom: 220.0,
                },
                PlacedActivation {
                    depth: 1,
                    top: 130.0,
                    bottom: 190.0,
                },
            ],
        },
        PlacedLifeline {
            id: "B".to_string(),
            x: 240.0,
            top: 58.0,
            bottom: 400.0,
            destroyed: true,
            activations: Vec::new(),
        },
    ];

    let clusters = vec![
        PlacedCluster {
            id: "box0".to_string(),
            title: label("a box", 44.0),
            center: Point::new(160.0, 250.0),
            size: Size::new(240.0, 460.0),
            parent: None,
            depth: 0,
            dashed: false,
            filled: true,
            sections: Vec::new(),
        },
        PlacedCluster {
            id: "g#0".to_string(),
            title: label("alt is sick", 90.0),
            center: Point::new(160.0, 200.0),
            size: Size::new(220.0, 160.0),
            parent: None,
            depth: 0,
            dashed: true,
            filled: false,
            sections: vec![ClusterSection {
                y: 210.0,
                title: label("else is well", 92.0),
            }],
        },
        PlacedCluster {
            id: "g#1".to_string(),
            title: label("loop twice", 80.0),
            center: Point::new(160.0, 210.0),
            size: Size::new(180.0, 100.0),
            parent: Some("g#0".to_string()),
            depth: 1,
            dashed: true,
            filled: false,
            sections: Vec::new(),
        },
    ];

    Diagram {
        width: 420.0,
        height: 500.0,
        nodes,
        edges,
        clusters,
        lifelines,
    }
}

// ---------------------------------------------------------------------------------------------
// 7. Adversarial
// ---------------------------------------------------------------------------------------------

/// Sources chosen to be awkward rather than realistic. konoma's rule is that the renderer runs on
/// a worker thread wrapped in `catch_unwind` (§1) — a safety net, not a licence — so each of these
/// has to come back with a diagram or an error, never a panic and never a NaN.
#[test]
fn awkward_sources_produce_a_diagram_or_an_error_and_never_a_panic() {
    if !text_metrics::fonts_available() {
        return;
    }
    let mut wide = String::from("sequenceDiagram\n");
    for i in 0..100 {
        wide.push_str(&format!("  p{i}->>p{}: m{i}\n", i + 1));
    }
    let mut deep = String::from("sequenceDiagram\n");
    for i in 0..30 {
        deep.push_str(&format!("  par level {i}\n"));
    }
    deep.push_str("  A->>B: x\n");
    for _ in 0..30 {
        deep.push_str("  end\n");
    }
    let mut many_bars = String::from("sequenceDiagram\n");
    for _ in 0..20 {
        many_bars.push_str("  A->>+B: open\n");
    }
    for _ in 0..20 {
        many_bars.push_str("  B-->>-A: close\n");
    }
    let mut long_note = String::from("sequenceDiagram\n  A->>B: x\n  Note over A,B: ");
    long_note.push_str(&"very long ".repeat(60));

    let cases: &[&str] = &[
        "sequenceDiagram",
        "sequenceDiagram\n  A->>A: only a self message",
        "sequenceDiagram\n  A->>B: \n  B->>A: ",
        "sequenceDiagram\n  loop\n  end",
        "sequenceDiagram\n  loop empty\n  end\n  A->>B: after",
        "sequenceDiagram\n  alt a\n  else b\n  else c\n  end",
        "sequenceDiagram\n  activate A\n  A->>B: x",
        "sequenceDiagram\n  participant A\n  activate A\n  A->>B: x",
        "sequenceDiagram\n  A->>B: x\n  activate A",
        "sequenceDiagram\n  create participant A\n  B->>C: never creates A",
        "sequenceDiagram\n  destroy A\n  B->>C: never destroys A",
        "sequenceDiagram\n  box\n  end\n  A->>B: x",
        "sequenceDiagram\n  box only a title\n  end\n  A->>B: x",
        "sequenceDiagram\n  Note over A: alone",
        "sequenceDiagram\n  Note left of A: alone",
        "sequenceDiagram\n  A->>B: 🎉🚀\n  Note over A,B: 全画面プレビュー",
        "sequenceDiagram\n  参加者->>ファイル: 読む",
        "sequenceDiagram\n  A->>B: a < b & c > \"d\"",
        "sequenceDiagram\n  A->>B: <script>alert(1)</script>",
        "sequenceDiagram\n  title \n  A->>B: x",
        "sequenceDiagram\n  autonumber 0 0\n  A->>B: x\n  B->>A: y",
        "sequenceDiagram\n  autonumber -5 -1\n  A->>B: x",
        &many_bars,
        &long_note,
        &deep,
        &wide,
    ];
    for src in cases {
        match render(src, "dark") {
            Err(_) => {}
            Ok(out) => {
                assert!(!out.contains("NaN"), "NaN reached the document for {src:?}");
                assert!(!out.contains("inf"), "an infinity reached it for {src:?}");
                let d = laid_out(src);
                assert!(
                    d.width.is_finite() && d.height.is_finite() && d.width > 0.0 && d.height > 0.0,
                    "{src:?}: {}x{} is not a drawable size",
                    svg::num(d.width),
                    svg::num(d.height)
                );
                assert!(
                    crate::preview::svg::rasterize_bytes(out.as_bytes(), FsPath::new("m.svg"), 300)
                        .is_some(),
                    "{src:?}: the output did not rasterise"
                );
                check_nodes_do_not_overlap("adversarial", &d);
                check_view_box_contains_everything("adversarial", &d);
            }
        }
    }
}

/// Refusing means refusing: the reasons are kept and the diagram is not drawn.
#[test]
fn errors_say_what_was_wrong() {
    assert_eq!(
        render("flowchart TD\n  A --> B", "dark").unwrap_err(),
        RenderError::SequenceParse(sequence_error("flowchart TD\n  A --> B"))
    );
    assert_eq!(
        render("sequenceDiagram\n", "dark").unwrap_err().to_string(),
        "sequence diagram declares no participants"
    );
    assert_eq!(
        render("sequenceDiagram\n  A->>B: x\n  end", "dark")
            .unwrap_err()
            .to_string(),
        "`end` with no open block at line 3"
    );
}

fn sequence_error(src: &str) -> crate::preview::mermaid::sequence::ParseError {
    crate::preview::mermaid::sequence::parse(src).expect_err("refuses")
}

// ---------------------------------------------------------------------------------------------
// 8. The gallery, and the crate this replaces
// ---------------------------------------------------------------------------------------------

/// Writes every case out for a person to look at.
///
/// §6 is explicit that a comparison with mermaid is **not** an automatic judgement: pixel equality
/// is structurally impossible, so what a drawing looks like is a question only a pair of eyes can
/// answer. This used to write the crate's rendering of the same source beside konoma's; stage 5b
/// removed the crate, so there is one file per case now and the decision is recorded in the commit
/// message and in `docs/STATUS.md`.
///
/// `KONOMA_GALLERY=/some/dir cargo test -- --ignored sequence_tests::gallery`
#[test]
#[ignore = "writes SVG files for a person to look at: KONOMA_GALLERY=<dir> cargo test -- --ignored gallery"]
fn gallery() {
    let dir = std::env::var("KONOMA_GALLERY").expect("set KONOMA_GALLERY to a directory");
    std::fs::create_dir_all(&dir).expect("create the gallery directory");
    for (name, src) in CASES {
        let out = render(src, "dark").unwrap_or_else(|e| panic!("{name}: {e}"));
        std::fs::write(format!("{dir}/{name}.svg"), out).expect("write the SVG");
    }
}
