//! What stage 2's parser is checked with.
//!
//! The corpus is built from the *cases in mermaid's own documentation and grammar* rather than
//! from bugs already found (memory `corpus-from-spec-not-from-bugs`): every construct
//! `stateDiagram.md` shows, plus the ones only `stateDiagram.jison` mentions (`[[fork]]`, the
//! floating note, `hide empty description`, `scale … width`).
//!
//! Two kinds of assertion carry most of the weight, and they are the ones that would be easy to
//! write vacuously:
//!
//! * **which states exist** — a parser that invents one turns a stylesheet line into a box, and a
//!   parser that loses one draws a diagram the author did not write. Every test that names a
//!   construct asserts the *whole* id list, not that a particular id is present.
//! * **which `[*]` is which** — the single hardest rule in this grammar, because it is resolved
//!   after parsing and by a rule (`docTranslator`) that no amount of reading the diagram would
//!   suggest.

use super::model::{Kind, NotePosition};
use super::{is_state_diagram, parse, Direction, ParseError};

/// The ids of every state, in order, as a `Vec<String>` that is easy to compare against a
/// literal.
fn ids(src: &str) -> Vec<String> {
    parse(src)
        .unwrap_or_else(|e| panic!("should parse: {e}"))
        .states
        .iter()
        .map(|s| s.id.clone())
        .collect()
}

/// `from -> to` for every transition, in order, with the label appended when there is one.
fn links(src: &str) -> Vec<String> {
    parse(src)
        .unwrap_or_else(|e| panic!("should parse: {e}"))
        .transitions
        .iter()
        .map(|t| match &t.label {
            Some(l) => format!("{} -> {} : {l}", t.from, t.to),
            None => format!("{} -> {}", t.from, t.to),
        })
        .collect()
}

// ---------------------------------------------------------------------------------------------
// Header and routing
// ---------------------------------------------------------------------------------------------

/// Both spellings are state diagrams, and nothing that merely starts with the letters is.
#[test]
fn the_header_decides_and_only_the_header() {
    assert!(is_state_diagram("stateDiagram-v2\n  A --> B"));
    assert!(is_state_diagram("stateDiagram\n  A --> B"));
    assert!(is_state_diagram(
        "\n\n%% comment\n  stateDiagram-v2\n  A --> B"
    ));
    assert!(is_state_diagram(
        "---\ntitle: t\n---\nstateDiagram-v2\n  A --> B"
    ));
    // A bare header is a state diagram — it just has nothing in it, which is a different refusal.
    assert!(is_state_diagram("stateDiagram-v2"));

    assert!(!is_state_diagram("stateDiagrams --> B"));
    assert!(!is_state_diagram("flowchart TD\n  A --> B"));
    assert!(!is_state_diagram("sequenceDiagram\n  A->>B: hi"));
    assert!(!is_state_diagram(""));
    assert!(!is_state_diagram("   \n\n  "));
}

/// `stateDiagram-v2` must win over `stateDiagram`, or the `-v2` becomes a statement.
#[test]
fn the_longer_keyword_wins() {
    let d = parse("stateDiagram-v2\n  A --> B").expect("parses");
    assert_eq!(d.state_ids(), vec!["A", "B"]);
}

/// Refusals are the ones §1 allows, and each says what was wrong.
#[test]
fn errors_say_what_was_wrong() {
    assert_eq!(parse("").unwrap_err(), ParseError::Empty);
    assert_eq!(
        parse("venn-beta\n  sets: [a]").unwrap_err(),
        ParseError::NotAStateDiagram {
            header: "venn-beta".to_string()
        }
    );
    assert_eq!(parse("stateDiagram-v2").unwrap_err(), ParseError::NoStates);
    assert_eq!(
        parse("stateDiagram-v2\n  state S {\n    A --> B\n").unwrap_err(),
        ParseError::UnclosedComposite {
            id: "S".to_string(),
            line: 2
        }
    );
    assert_eq!(
        parse("stateDiagram-v2\n  state \"unclosed as X\n  A --> B").unwrap_err(),
        ParseError::UnclosedString { line: 2 }
    );
    assert_eq!(
        parse("stateDiagram-v2\n  A --> B\n  note right of A\n    text").unwrap_err(),
        ParseError::UnclosedNote { line: 3 }
    );
    assert!(parse("stateDiagram-v2\n  state S {\n    A --> B\n")
        .unwrap_err()
        .to_string()
        .contains("unclosed"));
}

// ---------------------------------------------------------------------------------------------
// `[*]` — the rule that is resolved after parsing
// ---------------------------------------------------------------------------------------------

/// **The rule.** Every `[*]` an arrow leaves is one node per document; every `[*]` an arrow
/// enters is another. Four `[*]` here, two dots drawn.
#[test]
fn every_start_in_one_document_is_the_same_dot() {
    let src = "stateDiagram-v2\n  [*] --> A\n  [*] --> B\n  A --> [*]\n  B --> [*]";
    assert_eq!(ids(src), vec!["root_start", "A", "B", "root_end"]);
    assert_eq!(
        links(src),
        vec![
            "root_start -> A",
            "root_start -> B",
            "A -> root_end",
            "B -> root_end"
        ]
    );
    let d = parse(src).expect("parses");
    assert_eq!(d.state("root_start").expect("start").kind, Kind::Start);
    assert_eq!(d.state("root_end").expect("end").kind, Kind::End);
}

/// …and a block has its own pair, named after the block.
#[test]
fn a_block_has_its_own_start_and_end() {
    let src = "stateDiagram-v2\n  [*] --> S\n  state S {\n    [*] --> a\n    a --> [*]\n  }\n  \
               S --> [*]";
    assert_eq!(
        ids(src),
        vec!["root_start", "S", "S_start", "a", "S_end", "root_end"]
    );
    let d = parse(src).expect("parses");
    assert_eq!(
        d.state("S_start").expect("start").parent.as_deref(),
        Some("S")
    );
    assert_eq!(d.state("S_end").expect("end").parent.as_deref(), Some("S"));
    assert_eq!(d.state("root_end").expect("end").parent, None);
}

/// A bare `[*]` statement is a *start*, because `docTranslator` visits a lone statement with
/// `first = true`.
#[test]
fn a_lone_edge_state_is_a_start() {
    let d = parse("stateDiagram-v2\n  [*]\n  A --> B").expect("parses");
    assert_eq!(d.state_ids(), vec!["root_start", "A", "B"]);
    assert_eq!(d.state("root_start").expect("start").kind, Kind::Start);
}

// ---------------------------------------------------------------------------------------------
// States and their text
// ---------------------------------------------------------------------------------------------

/// The three ways to describe a state, and what each draws.
#[test]
fn a_state_can_be_described_three_ways() {
    let d = parse(
        "stateDiagram-v2\n  plain\n  state \"A described state\" as s2\n  s3 : Third\n  \
         plain --> s2\n  s2 --> s3",
    )
    .expect("parses");
    assert_eq!(d.state_ids(), vec!["plain", "s2", "s3"]);
    // With no description of its own, a state is drawn with its id.
    assert_eq!(d.state("plain").expect("plain").label, "plain");
    assert_eq!(d.state("s2").expect("s2").label, "A described state");
    assert_eq!(d.state("s3").expect("s3").label, "Third");
    assert!(d.states.iter().all(|s| !s.titled));
}

/// Two descriptions make a box with a rule under the first: mermaid's `SHAPE_STATE_WITH_DESC`,
/// which its own fix-up turns *back* into a plain box when only one description survives.
#[test]
fn two_descriptions_make_a_titled_box() {
    let d = parse("stateDiagram-v2\n  s : first\n  s : second\n  s --> t").expect("parses");
    let s = d.state("s").expect("s");
    assert_eq!(s.label, "first\nsecond");
    assert!(s.titled);
    assert!(!d.state("t").expect("t").titled, "one description is plain");
}

/// `state "text" as id : more` — the grammar's own `if ($3.match(':'))` branch, which splits the
/// id from a second description.
#[test]
fn an_as_form_can_carry_a_second_description() {
    let d = parse("stateDiagram-v2\n  state \"first\" as s : second\n  s --> t").expect("parses");
    let s = d.state("s").expect("s");
    assert_eq!(s.id, "s");
    assert_eq!(s.label, "first\nsecond");
    assert!(s.titled);
}

/// `<<fork>>`, `<<join>>` and `<<choice>>`, in both the `<<…>>` and the `[[…]]` spellings the
/// lexer accepts, and in any case.
#[test]
fn the_marked_kinds_are_recognised_in_both_spellings() {
    for (marker, kind) in [
        ("<<fork>>", Kind::Fork),
        ("<<join>>", Kind::Join),
        ("<<choice>>", Kind::Choice),
        ("[[fork]]", Kind::Fork),
        ("[[join]]", Kind::Join),
        ("[[choice]]", Kind::Choice),
        ("<<FORK>>", Kind::Fork),
    ] {
        let src = format!("stateDiagram-v2\n  state x {marker}\n  A --> x\n  x --> B");
        let d = parse(&src).unwrap_or_else(|e| panic!("{marker}: {e}"));
        assert_eq!(d.state_ids(), vec!["x", "A", "B"], "{marker}");
        assert_eq!(d.state("x").expect("x").kind, kind, "{marker}");
        // These draw no text at all, so no label is carried for them.
        assert_eq!(d.state("x").expect("x").label, "", "{marker}");
    }
}

/// A `-` inside an id is konoma's own permissiveness — mermaid rejects the whole diagram — and it
/// must not let an id swallow the arrow that follows it.
#[test]
fn a_hyphen_may_sit_inside_an_id_but_never_starts_an_arrow() {
    assert_eq!(
        ids("stateDiagram-v2\n  my-state --> other-state"),
        vec!["my-state", "other-state"]
    );
    assert_eq!(ids("stateDiagram-v2\n  A-->B"), vec!["A", "B"]);
    assert_eq!(ids("stateDiagram-v2\n  A --> B"), vec!["A", "B"]);
}

// ---------------------------------------------------------------------------------------------
// Composite states and concurrency
// ---------------------------------------------------------------------------------------------

/// A composite is a state *and* a container: it keeps its id as an endpoint and gains members.
#[test]
fn a_composite_is_a_state_and_a_container() {
    let d = parse(
        "stateDiagram-v2\n  [*] --> First\n  state First {\n    [*] --> second\n    \
         second --> [*]\n  }\n  First --> Last",
    )
    .expect("parses");
    let first = d.state("First").expect("First");
    assert_eq!(first.kind, Kind::Composite);
    assert_eq!(first.members, vec!["First_start", "second", "First_end"]);
    assert_eq!(first.label, "First", "no description means the id is drawn");
    assert!(links(
        "stateDiagram-v2\n  [*] --> First\n  state First {\n    [*] --> second\n    \
         second --> [*]\n  }\n  First --> Last"
    )
    .contains(&"First -> Last".to_string()));
}

/// `NamedComposite: Another Composite` names the frame, from the docs' own example.
#[test]
fn a_composite_can_be_described_before_it_is_opened() {
    let d = parse(
        "stateDiagram-v2\n  NamedComposite: Another Composite\n  state NamedComposite {\n    \
         [*] --> a\n  }",
    )
    .expect("parses");
    let c = d.state("NamedComposite").expect("composite");
    assert_eq!(c.kind, Kind::Composite);
    assert_eq!(c.label, "Another Composite");
}

/// Blocks nest, and every level keeps its own members.
#[test]
fn blocks_nest() {
    let d = parse(
        "stateDiagram-v2\n  state A {\n    state B {\n      state C {\n        x\n      }\n    \
         }\n  }",
    )
    .expect("parses");
    assert_eq!(d.state_ids(), vec!["A", "B", "C", "x"]);
    assert_eq!(d.state("A").expect("A").members, vec!["B"]);
    assert_eq!(d.state("B").expect("B").members, vec!["C"]);
    assert_eq!(d.state("C").expect("C").members, vec!["x"]);
    assert_eq!(d.state("x").expect("x").parent.as_deref(), Some("C"));
}

/// **`--` restructures.** Two regions, each an anonymous block, and the `[*]` inside each belongs
/// to *its own region* rather than to the block that holds both.
#[test]
fn a_divider_splits_a_block_into_regions() {
    let d = parse(
        "stateDiagram-v2\n  state Active {\n    [*] --> NumOff\n    --\n    [*] --> CapsOff\n  }",
    )
    .expect("parses");
    assert_eq!(
        d.state_ids(),
        vec![
            "Active",
            "divider-id-1",
            "divider-id-1_start",
            "NumOff",
            "divider-id-2",
            "divider-id-2_start",
            "CapsOff",
        ]
    );
    assert_eq!(
        d.state("Active").expect("Active").members,
        vec!["divider-id-1", "divider-id-2"]
    );
    for region in ["divider-id-1", "divider-id-2"] {
        let r = d.state(region).expect("region");
        assert_eq!(r.kind, Kind::Concurrent);
        assert_eq!(r.parent.as_deref(), Some("Active"));
        assert_eq!(r.label, "", "a region has no title of its own");
    }
    assert_eq!(
        d.state("divider-id-1_start")
            .expect("start")
            .parent
            .as_deref(),
        Some("divider-id-1")
    );
}

/// mermaid only restructures when there is something on both sides of the last divider
/// (`if (doc.length > 0 && currentDoc.length > 0)`), so a block that merely *ends* with one is
/// left alone — and konoma drops the divider rather than leaving an empty box behind.
#[test]
fn a_trailing_divider_changes_nothing() {
    let d = parse("stateDiagram-v2\n  state S {\n    A --> B\n    --\n  }").expect("parses");
    assert_eq!(d.state_ids(), vec!["S", "A", "B"]);
    assert_eq!(d.state("S").expect("S").members, vec!["A", "B"]);
    // …and a `--` outside any block is not a divider at all.
    assert_eq!(
        ids("stateDiagram-v2\n  A --> B\n  --\n  B --> C"),
        vec!["A", "B", "C"]
    );
}

// ---------------------------------------------------------------------------------------------
// Notes
// ---------------------------------------------------------------------------------------------

/// A note becomes a node plus a headless link, and the side it was asked for decides the link's
/// direction — which is what `dataFetcher` does and all it does.
#[test]
fn a_note_becomes_a_node_and_a_link() {
    let d = parse(
        "stateDiagram-v2\n  State1: The state with a note\n  note right of State1\n    \
         Important information! You can write\n    notes.\n  end note\n  State1 --> State2\n  \
         note left of State2 : This is the note to the left.",
    )
    .expect("parses");
    assert_eq!(
        d.state_ids(),
        vec!["State1", "State1----note-1", "State2", "State2----note-2"]
    );
    let right = d.state("State1----note-1").expect("note");
    assert_eq!(right.kind, Kind::Note);
    assert_eq!(
        right.label, "Important information! You can write\nnotes.",
        "a multi-line note keeps its lines"
    );
    assert_eq!(right.note_position, Some(NotePosition::Right));

    let left = d.state("State2----note-2").expect("note");
    assert_eq!(left.label, "This is the note to the left.");
    assert_eq!(left.note_position, Some(NotePosition::Left));

    // `right of` points away from the state; `left of` points at it.
    let ties: Vec<&super::Transition> = d.transitions.iter().filter(|t| t.is_note_link).collect();
    assert_eq!(ties.len(), 2);
    assert_eq!(
        (ties[0].from.as_str(), ties[0].to.as_str()),
        ("State1", "State1----note-1")
    );
    assert_eq!(
        (ties[1].from.as_str(), ties[1].to.as_str()),
        ("State2----note-2", "State2")
    );
}

/// A floating `note "text" as id` is recognised and drawn by nobody — mermaid's own grammar rule
/// for it has an empty action.
#[test]
fn a_floating_note_is_dropped_without_becoming_a_state() {
    assert_eq!(
        ids("stateDiagram-v2\n  note \"just floating\" as N\n  A --> B"),
        vec!["A", "B"]
    );
}

/// A note inside a block belongs to that block.
#[test]
fn a_note_stays_in_the_block_it_was_written_in() {
    let d = parse("stateDiagram-v2\n  state S {\n    A --> B\n    note right of A : inside\n  }")
        .expect("parses");
    let note = d
        .states
        .iter()
        .find(|s| s.kind == Kind::Note)
        .expect("a note");
    assert_eq!(note.parent.as_deref(), Some("S"));
    assert!(d.state("S").expect("S").members.contains(&note.id));
}

// ---------------------------------------------------------------------------------------------
// Direction
// ---------------------------------------------------------------------------------------------

/// A top-level `direction` is the diagram's axis — unlike a flowchart, where mermaid ignores it.
#[test]
fn a_top_level_direction_sets_the_axis() {
    for (word, dir) in [
        ("TB", Direction::TopToBottom),
        ("TD", Direction::TopToBottom),
        ("BT", Direction::BottomToTop),
        ("LR", Direction::LeftToRight),
        ("RL", Direction::RightToLeft),
    ] {
        let src = format!("stateDiagram-v2\n  direction {word}\n  A --> B");
        assert_eq!(parse(&src).expect("parses").direction, dir, "{word}");
    }
    assert_eq!(
        parse("stateDiagram-v2\n  A --> B")
            .expect("parses")
            .direction,
        Direction::TopToBottom,
        "the default"
    );
}

/// A `direction` inside a block is read and **not** applied. mermaid's lexer action unshifts it
/// into the *root* document, so a nested statement silently turns the whole picture sideways;
/// pinning konoma's choice as a test rather than a comment means changing it has to say so here.
#[test]
fn a_direction_inside_a_block_does_not_turn_the_diagram() {
    let d = parse("stateDiagram-v2\n  state S {\n    direction LR\n    a --> b\n  }\n  A --> S")
        .expect("parses");
    assert_eq!(d.direction, Direction::TopToBottom);
    assert_eq!(d.state_ids(), vec!["S", "a", "b", "A"]);
}

/// mermaid's lexer matches `.*direction\s+TB[^\n]*`, so a description that happens to contain the
/// word eats the whole statement. konoma requires the statement to *start* with it.
#[test]
fn direction_is_a_statement_only_at_the_start_of_one() {
    let d = parse("stateDiagram-v2\n  s1 : go direction TB now\n  s1 --> s2").expect("parses");
    assert_eq!(d.direction, Direction::TopToBottom, "unchanged");
    assert_eq!(d.state("s1").expect("s1").label, "go direction TB now");
}

// ---------------------------------------------------------------------------------------------
// Statements that must be recognised so they cannot become states (§2-3)
// ---------------------------------------------------------------------------------------------

/// The whole §2-3 list, in one diagram: none of it may add a box.
#[test]
fn nothing_that_is_not_a_state_becomes_one() {
    let src = "stateDiagram-v2\n\
               \x20  direction TB\n\
               \x20  accTitle: An accessible title\n\
               \x20  accDescr: An accessible description\n\
               \x20  classDef notMoving fill:white\n\
               \x20  classDef movement font-style:italic\n\
               \x20  style Still fill:#f9f,stroke:#333\n\
               \x20  click Still \"https://example.com\" \"tip\"\n\
               \x20  scale 350 width\n\
               \x20  hide empty description\n\
               \x20  [*] --> Still\n\
               \x20  Still --> Moving\n\
               \x20  class Still notMoving\n\
               \x20  class Moving movement";
    let d = parse(src).expect("parses");
    assert_eq!(d.state_ids(), vec!["root_start", "Still", "Moving"]);
    assert_eq!(d.acc_title.as_deref(), Some("An accessible title"));
    assert_eq!(d.acc_descr.as_deref(), Some("An accessible description"));
    // The class names are carried even though nothing colours with them yet.
    assert_eq!(d.state("Still").expect("Still").classes, vec!["notMoving"]);
}

/// `accDescr { … }`, the block form.
#[test]
fn acc_descr_block_form_is_read_and_not_drawn() {
    let d =
        parse("stateDiagram-v2\n  accDescr {\n    two\n    lines\n  }\n  A --> B").expect("parses");
    assert_eq!(d.state_ids(), vec!["A", "B"]);
    assert_eq!(d.acc_descr.as_deref(), Some("two\nlines"));
}

/// `:::` attaches a class to an endpoint without becoming part of the id.
#[test]
fn the_style_separator_never_lands_in_an_id() {
    let d = parse(
        "stateDiagram-v2\n  classDef hot fill:#f00\n  [*] --> Still:::hot\n  \
         Still --> Moving:::hot",
    )
    .expect("parses");
    assert_eq!(d.state_ids(), vec!["root_start", "Still", "Moving"]);
    assert_eq!(d.state("Still").expect("Still").classes, vec!["hot"]);
}

/// A `class`/`style` line naming a state nobody declared does **not** bring it into existence.
/// mermaid's `handleStyleDef` calls `addState`, so a typo there silently adds a box.
#[test]
fn a_style_line_does_not_invent_a_state() {
    assert_eq!(
        ids("stateDiagram-v2\n  A --> B\n  class Typo movement\n  style AlsoTypo fill:#f00"),
        vec!["A", "B"]
    );
}

// ---------------------------------------------------------------------------------------------
// Comments
// ---------------------------------------------------------------------------------------------

/// A state diagram has two comment markers and, unlike a flowchart, they work at the end of a
/// statement as well as on a line of their own.
#[test]
fn both_comment_markers_work_on_a_line_and_at_the_end_of_one() {
    let src = "stateDiagram-v2\n\
               %% a whole line\n\
               \x20 # another whole line\n\
               \x20 [*] --> Still\n\
               \x20 Still --> Moving %% at the end\n\
               \x20 Moving --> Crash # also at the end";
    assert_eq!(
        ids(src),
        vec!["root_start", "Still", "Moving", "Crash"],
        "a comment must not become a state"
    );
    assert_eq!(
        links(src),
        vec!["root_start -> Still", "Still -> Moving", "Moving -> Crash"]
    );
}

/// A marker inside a description is text, because mermaid's `DESCR` token is the longest match
/// from the colon and runs to the end of the line.
#[test]
fn a_marker_inside_a_description_is_text() {
    let d = parse("stateDiagram-v2\n  s1 : 50% done #1 of 2\n  s1 --> s2").expect("parses");
    assert_eq!(d.state("s1").expect("s1").label, "50% done #1 of 2");
}

/// A `#` that is not at a token boundary is part of the id, exactly as the lexer reads it.
#[test]
fn a_hash_inside_an_id_stays_in_it() {
    assert_eq!(ids("stateDiagram-v2\n  A#1 --> B#2"), vec!["A#1", "B#2"]);
}

// ---------------------------------------------------------------------------------------------
// Text decoding, front matter
// ---------------------------------------------------------------------------------------------

/// Labels go through the same decoding a flowchart's do: `<br>` in all four spellings, `\n`, and
/// `#nn;` entities.
#[test]
fn labels_are_decoded_the_way_a_flowcharts_are() {
    let d = parse(
        "stateDiagram-v2\n  s1 : first<br>second<BR/>third\n  s2 : a #35; b\n  s1 --> s2 : x<br />y",
    )
    .expect("parses");
    assert_eq!(d.state("s1").expect("s1").label, "first\nsecond\nthird");
    assert_eq!(d.state("s2").expect("s2").label, "a # b");
    assert_eq!(d.transitions[0].label.as_deref(), Some("x\ny"));
}

/// A front matter `title:` is lifted out and the diagram still parses from the line after it.
#[test]
fn front_matter_is_lifted_out() {
    let d =
        parse("---\ntitle: Simple sample\n---\nstateDiagram-v2\n  [*] --> Still").expect("parses");
    assert_eq!(d.title.as_deref(), Some("Simple sample"));
    assert_eq!(d.state_ids(), vec!["root_start", "Still"]);
}

/// CJK, which needs no quoting because a description runs to the end of its line.
#[test]
fn cjk_needs_no_quoting() {
    let d = parse("stateDiagram-v2\n  s1 : 開始、処理。「確認」\n  [*] --> s1").expect("parses");
    assert_eq!(d.state("s1").expect("s1").label, "開始、処理。「確認」");
}

// ---------------------------------------------------------------------------------------------
// Adversarial input
// ---------------------------------------------------------------------------------------------

/// Sources chosen to be awkward rather than realistic. The renderer runs on a worker thread
/// wrapped in `catch_unwind` (§1) — which is a safety net, not a licence — so each of these has
/// to come back with a diagram or an error, and never a panic.
#[test]
fn awkward_sources_produce_a_diagram_or_an_error_and_never_a_panic() {
    let mut deep = String::from("stateDiagram-v2\n");
    for i in 0..64 {
        deep.push_str(&format!("  state s{i} {{\n"));
    }
    deep.push_str("  A --> B\n");
    for _ in 0..64 {
        deep.push_str("  }\n");
    }

    let mut wide = String::from("stateDiagram-v2\n");
    for i in 0..600 {
        wide.push_str(&format!("  n{i} --> n{}\n", i + 1));
    }

    let cases: &[&str] = &[
        "",
        "   \n\n   ",
        "stateDiagram-v2",
        "stateDiagram-v2\n   ",
        "stateDiagram-v2\n  [*]",
        "stateDiagram-v2\n  [*] --> [*]",
        "stateDiagram-v2\n  -->",
        "stateDiagram-v2\n  A -->",
        "stateDiagram-v2\n  --> B",
        "stateDiagram-v2\n  A --> B --> C",
        "stateDiagram-v2\n  :\n  A --> B",
        "stateDiagram-v2\n  :::\n  A --> B",
        "stateDiagram-v2\n  A::: --> B",
        "stateDiagram-v2\n  }\n  A --> B",
        "stateDiagram-v2\n  }}}}\n  A --> B",
        "stateDiagram-v2\n  {\n  A --> B\n  }",
        "stateDiagram-v2\n  state {\n  A --> B\n  }",
        "stateDiagram-v2\n  state S { A --> B }",
        "stateDiagram-v2\n  state S {}\n  A --> B",
        "stateDiagram-v2\n  state S {}\n  S --> A",
        "stateDiagram-v2\n  state s <<nonsense>>\n  A --> s",
        "stateDiagram-v2\n  state \"\" as s\n  A --> s",
        "stateDiagram-v2\n  s : \n  A --> s",
        "stateDiagram-v2\n  note right of : text\n  A --> B",
        "stateDiagram-v2\n  note of A : text\n  A --> B",
        "stateDiagram-v2\n  note right of A : \n  A --> B",
        "stateDiagram-v2\n  end note\n  A --> B",
        "stateDiagram-v2\n  --\n  --\n  A --> B",
        "stateDiagram-v2\n  state S {\n    --\n    --\n  }\n  A --> B",
        "stateDiagram-v2\n  A --> B\n  A --> B\n  A --> B",
        "stateDiagram-v2\n  一 --> 二 : 🎉",
        "stateDiagram-v2\n  A[[fork]] --> B",
        "stateDiagram-v2\n  accDescr {\n  A --> B",
        "stateDiagram-v2\n  direction\n  A --> B",
        "stateDiagram-v2\n  direction NOPE\n  A --> B",
        "stateDiagram-v2\n  \\あ --> B",
        &deep,
        &wide,
    ];
    for src in cases {
        match parse(src) {
            Err(_) => {}
            Ok(d) => {
                assert!(
                    !d.states.is_empty(),
                    "{src:?}: an Ok diagram must have something in it"
                );
                for s in &d.states {
                    assert!(!s.id.is_empty(), "{src:?}: a state with no id");
                }
                for t in &d.transitions {
                    assert!(
                        d.state(&t.from).is_some() && d.state(&t.to).is_some(),
                        "{src:?}: {} -> {} names a state that does not exist",
                        t.from,
                        t.to
                    );
                }
            }
        }
    }
}

/// The ceiling on transitions is a guard, not a style rule — but it has to actually hold.
#[test]
fn the_transition_ceiling_holds() {
    let mut wide = String::from("stateDiagram-v2\n");
    for i in 0..900 {
        wide.push_str(&format!("  n{i} --> n{}\n", i + 1));
    }
    let d = parse(&wide).expect("parses");
    assert_eq!(
        d.transitions.len(),
        super::parser::MAX_TRANSITIONS_FOR_TESTS
    );
}
