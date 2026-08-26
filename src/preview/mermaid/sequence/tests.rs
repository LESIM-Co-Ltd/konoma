//! What the sequence parser is checked with.
//!
//! The corpus is built from the **cases in the grammar and in mermaid's own documentation page**,
//! not from bugs already found (memory `corpus-from-spec-not-from-bugs`): every statement the
//! `sequenceDiagram.jison` has a production for appears here in the spelling the documentation
//! uses, and every one konoma deliberately does *not* draw has a test asserting the drop — because
//! "not drawn" and "not parsed" are different things, and the second one invents a participant.

use super::model::{
    AutoNumber, BlockKind, Event, Head, Line, ParseError, ParticipantKind, Placement, SectionKind,
    SequenceDiagram,
};
use super::parser::{is_sequence_diagram, parse, MAX_MESSAGES_FOR_TESTS};

fn ok(src: &str) -> SequenceDiagram {
    parse(src).unwrap_or_else(|e| panic!("should parse: {e}\n{src}"))
}

fn ids(d: &SequenceDiagram) -> Vec<&str> {
    d.participants.iter().map(|p| p.id.as_str()).collect()
}

fn labels(d: &SequenceDiagram) -> Vec<&str> {
    d.participants.iter().map(|p| p.label.as_str()).collect()
}

fn texts(d: &SequenceDiagram) -> Vec<&str> {
    d.messages().map(|m| m.text.as_str()).collect()
}

// ---------------------------------------------------------------------------------------------
// Header and routing
// ---------------------------------------------------------------------------------------------

#[test]
fn the_header_is_case_insensitive_and_needs_a_word_boundary() {
    for good in [
        "sequenceDiagram\n  A->>B: x",
        "SEQUENCEDIAGRAM\n  A->>B: x",
        "sequencediagram\n  A->>B: x",
        "  sequenceDiagram  \n  A->>B: x",
    ] {
        assert!(is_sequence_diagram(good), "{good:?}");
        assert!(parse(good).is_ok(), "{good:?}");
    }
    for bad in [
        "sequenceDiagrams\n  A->>B: x",
        "flowchart TD\n  A --> B",
        "classDiagram\n  A --> B",
        "erDiagram\n  A ||--|| B : x",
        "stateDiagram-v2\n  [*] --> A",
    ] {
        assert!(!is_sequence_diagram(bad), "{bad:?}");
    }
    assert_eq!(parse(""), Err(ParseError::Empty));
    assert_eq!(parse("   \n\n  "), Err(ParseError::Empty));
    assert_eq!(parse("%% only a comment\n"), Err(ParseError::Empty));
}

/// A statement written on the header's own line still counts — `sequenceDiagram; A->>B: x` is one
/// line, because `;` is a NEWLINE in this lexer.
#[test]
fn the_header_line_can_carry_the_first_statement() {
    let d = ok("sequenceDiagram; Alice->>Bob: hi");
    assert_eq!(ids(&d), ["Alice", "Bob"]);
    assert_eq!(texts(&d), ["hi"]);
}

// ---------------------------------------------------------------------------------------------
// Participants
// ---------------------------------------------------------------------------------------------

/// The columns are in **first-appearance order**, and `participant` before any message is how an
/// author reorders them. That is the whole documented reason the statement exists.
#[test]
fn participants_are_ordered_by_first_appearance() {
    let d = ok("sequenceDiagram\n  Bob->>Alice: Hi Alice\n  Alice->>Bob: Hi Bob");
    assert_eq!(ids(&d), ["Bob", "Alice"]);

    let d = ok("sequenceDiagram\n  participant Alice\n  participant Bob\n  Bob->>Alice: Hi Alice");
    assert_eq!(ids(&d), ["Alice", "Bob"]);
}

#[test]
fn an_alias_is_what_is_drawn_and_the_id_is_what_messages_use() {
    let d =
        ok("sequenceDiagram\n  participant A as Alice\n  participant J as John\n  A->>J: Hello");
    assert_eq!(ids(&d), ["A", "J"]);
    assert_eq!(labels(&d), ["Alice", "John"]);
    assert_eq!(d.messages().next().unwrap().from, "A");
}

/// `as` is a whole word: a participant whose name contains "as" is not cut in half.
#[test]
fn as_is_a_word_and_not_a_substring() {
    let d = ok("sequenceDiagram\n  participant Cassandra\n  Cassandra->>B: x");
    assert_eq!(ids(&d), ["Cassandra", "B"]);
    assert_eq!(labels(&d), ["Cassandra", "B"]);
    // A name with spaces is legal — the ID lexer state takes everything up to `as` or the newline.
    let d = ok("sequenceDiagram\n  participant Alice Smith as The boss\n");
    assert_eq!(ids(&d), ["Alice Smith"]);
    assert_eq!(labels(&d), ["The boss"]);
}

#[test]
fn actor_declares_a_stick_figure_and_participant_does_not() {
    let d = ok("sequenceDiagram\n  actor Alice\n  participant Bob\n  Alice->>Bob: Hi");
    assert_eq!(d.participant("Alice").unwrap().kind, ParticipantKind::Actor);
    assert_eq!(
        d.participant("Bob").unwrap().kind,
        ParticipantKind::Participant
    );
}

/// A later `participant A` must not null the label an earlier one set, nor demote an actor.
#[test]
fn a_second_declaration_never_takes_a_label_away() {
    let d = ok("sequenceDiagram\n  actor A as Alice\n  participant A\n  A->>B: x");
    assert_eq!(labels(&d)[0], "Alice");
    assert_eq!(d.participant("A").unwrap().kind, ParticipantKind::Actor);
}

/// Every documented `@{ "type": … }` stereotype is recognised, and only `actor` changes the
/// drawing (divergence 6). The point of the test is that the block never leaks into the name.
#[test]
fn a_participant_config_block_is_read_and_never_becomes_part_of_the_name() {
    for kind in [
        "boundary",
        "control",
        "entity",
        "database",
        "collections",
        "queue",
        "participant",
    ] {
        let src = format!(
            "sequenceDiagram\n  participant Alice@{{ \"type\" : \"{kind}\" }}\n  Alice->>Bob: x"
        );
        let d = ok(&src);
        assert_eq!(ids(&d), ["Alice", "Bob"], "{kind}");
        assert_eq!(
            d.participant("Alice").unwrap().kind,
            ParticipantKind::Participant,
            "{kind}: only `actor` changes the glyph"
        );
    }
    let d = ok("sequenceDiagram\n  participant Alice@{ \"type\" : \"actor\" }\n  Alice->>Bob: x");
    assert_eq!(d.participant("Alice").unwrap().kind, ParticipantKind::Actor);
}

/// Upstream's own precedence rule: an external `as` alias beats an inline `"alias"`.
#[test]
fn an_external_alias_beats_an_inline_one() {
    let d = ok(
        "sequenceDiagram\n  participant API@{ \"type\": \"boundary\", \"alias\": \"Internal\" } as External\n  API->>B: x",
    );
    assert_eq!(labels(&d)[0], "External");
    let d = ok("sequenceDiagram\n  participant API@{ \"type\": \"boundary\", \"alias\": \"Public API\" }\n  API->>B: x");
    assert_eq!(labels(&d)[0], "Public API");
}

// ---------------------------------------------------------------------------------------------
// Messages
// ---------------------------------------------------------------------------------------------

/// Every arrow spelling in the documentation's two tables, and what each draws.
#[test]
fn every_arrow_spelling_is_read() {
    let cases: &[(&str, Line, Head, Head)] = &[
        ("->", Line::Solid, Head::None, Head::None),
        ("-->", Line::Dotted, Head::None, Head::None),
        ("->>", Line::Solid, Head::None, Head::Arrow),
        ("-->>", Line::Dotted, Head::None, Head::Arrow),
        ("<<->>", Line::Solid, Head::Arrow, Head::Arrow),
        ("<<-->>", Line::Dotted, Head::Arrow, Head::Arrow),
        ("-x", Line::Solid, Head::None, Head::Cross),
        ("--x", Line::Dotted, Head::None, Head::Cross),
        ("-)", Line::Solid, Head::None, Head::Async),
        ("--)", Line::Dotted, Head::None, Head::Async),
        // Half-arrows: folded onto a full head, `_REVERSE` at the sending end (divergence 4).
        ("-|\\", Line::Solid, Head::None, Head::Arrow),
        ("--|\\", Line::Dotted, Head::None, Head::Arrow),
        ("-|/", Line::Solid, Head::None, Head::Arrow),
        ("--|/", Line::Dotted, Head::None, Head::Arrow),
        ("/|-", Line::Solid, Head::Arrow, Head::None),
        ("/|--", Line::Dotted, Head::Arrow, Head::None),
        ("\\|-", Line::Solid, Head::Arrow, Head::None),
        ("\\|--", Line::Dotted, Head::Arrow, Head::None),
        ("-\\\\", Line::Solid, Head::None, Head::Arrow),
        ("--\\\\", Line::Dotted, Head::None, Head::Arrow),
        ("-//", Line::Solid, Head::None, Head::Arrow),
        ("--//", Line::Dotted, Head::None, Head::Arrow),
        ("//-", Line::Solid, Head::Arrow, Head::None),
        ("//--", Line::Dotted, Head::Arrow, Head::None),
        ("\\\\-", Line::Solid, Head::Arrow, Head::None),
        ("\\\\--", Line::Dotted, Head::Arrow, Head::None),
    ];
    for (spelling, line, start, end) in cases {
        let src = format!("sequenceDiagram\n  A{spelling}B: hi");
        let d = ok(&src);
        assert_eq!(
            ids(&d),
            ["A", "B"],
            "{spelling}: a spelling that is not read becomes part of a name"
        );
        let m = d
            .messages()
            .next()
            .unwrap_or_else(|| panic!("{spelling}: no message"));
        assert_eq!(m.signal.line, *line, "{spelling}: line style");
        assert_eq!(
            m.signal.start, *start,
            "{spelling}: head at the sending end"
        );
        assert_eq!(m.signal.end, *end, "{spelling}: head at the receiving end");
        assert_eq!(m.text, "hi", "{spelling}");
    }
}

/// A `-` inside a name is not an arrow: the scan takes the leftmost position where a whole
/// spelling matches, so `NAMED-DRIVER` survives.
#[test]
fn a_dash_inside_a_name_is_not_an_arrow() {
    let d = ok("sequenceDiagram\n  NAMED-DRIVER->>CAR: drives");
    assert_eq!(ids(&d), ["NAMED-DRIVER", "CAR"]);
}

/// An arrow inside a message's text is text.
#[test]
fn an_arrow_in_the_text_does_not_split_the_message() {
    let d = ok("sequenceDiagram\n  A->>B: see A-->C for the reply");
    assert_eq!(ids(&d), ["A", "B"]);
    assert_eq!(texts(&d), ["see A-->C for the reply"]);
}

/// Divergence 3: upstream needs the `:`; konoma draws the message with no text rather than
/// refusing the diagram.
#[test]
fn a_message_with_no_colon_is_a_message_with_no_text() {
    let d = ok("sequenceDiagram\n  Alice->>John\n  John-->>Alice: reply");
    assert_eq!(texts(&d), ["", "reply"]);
}

/// Divergence 1: a keyword only counts as the whole first word of a statement. Upstream's
/// first-match lexer breaks all four of these.
#[test]
fn a_word_that_merely_starts_with_a_keyword_is_not_a_keyword() {
    for (src, expected) in [
        ("sequenceDiagram\n  looper->>B: x", ["looper", "B"]),
        ("sequenceDiagram\n  ending->>B: x", ["ending", "B"]),
        ("sequenceDiagram\n  parser->>B: x", ["parser", "B"]),
        ("sequenceDiagram\n  optimist->>B: x", ["optimist", "B"]),
    ] {
        let d = ok(src);
        assert_eq!(ids(&d), expected, "{src:?}");
        assert_eq!(d.messages().count(), 1, "{src:?}");
    }
    // …and the word `end` inside a label, which mermaid's own documentation warns about.
    let d = ok(
        "sequenceDiagram\n  A->>B: at the end of the day\n  loop forever\n    A->>B: again\n  end",
    );
    assert_eq!(texts(&d), ["at the end of the day", "again"]);
}

#[test]
fn a_self_message_is_one_message_and_one_participant() {
    let d = ok("sequenceDiagram\n  John->>John: Fight against hypochondria");
    assert_eq!(ids(&d), ["John"]);
    assert_eq!(d.messages().count(), 1);
}

/// `<br/>` and `\n` become real newlines, in messages, notes and aliases alike — a label that
/// still says `<br/>` is one box too wide and one line too short.
#[test]
fn line_breaks_are_resolved_everywhere_text_is_read() {
    let d = ok(
        "sequenceDiagram\n  participant Alice as Alice<br/>Johnson\n  Alice->John: Hello John,<br/>how are you?\n  Note over Alice,John: A typical interaction<br>But now in two lines",
    );
    assert_eq!(labels(&d)[0], "Alice\nJohnson");
    assert_eq!(texts(&d), ["Hello John,\nhow are you?"]);
    assert_eq!(
        d.notes().next().unwrap().text,
        "A typical interaction\nBut now in two lines"
    );
}

// ---------------------------------------------------------------------------------------------
// Comments, `;`, entity codes
// ---------------------------------------------------------------------------------------------

/// `%%` on its own line, `#` to end of line, and `;` as a statement separator — the three things
/// this grammar does that the other four do not.
#[test]
fn comments_and_semicolons_follow_this_grammars_own_rules() {
    let d = ok(
        "sequenceDiagram\n  Alice->>John: Hello\n  %% this is a comment\n  John-->>Alice: Great!",
    );
    assert_eq!(texts(&d), ["Hello", "Great!"]);

    let d = ok("sequenceDiagram\n  # a whole-line comment\n  Alice->>John: Hello");
    assert_eq!(texts(&d), ["Hello"]);
    assert_eq!(ids(&d), ["Alice", "John"]);

    let d = ok("sequenceDiagram\n  Alice->>John: Hello # trailing note\n");
    assert_eq!(texts(&d), ["Hello"]);

    let d = ok("sequenceDiagram; A->>B: one; B->>A: two");
    assert_eq!(texts(&d), ["one", "two"]);
}

/// Divergence 2. `#9829;` is an entity code and neither its `#` nor its `;` breaks the statement,
/// which is what makes the documented example read as documented.
#[test]
fn an_entity_code_is_not_a_comment_and_its_semicolon_is_not_a_separator() {
    let d =
        ok("sequenceDiagram\n  A->>B: I #9829; you!\n  B->>A: I #9829; you #infin; times more!");
    assert_eq!(texts(&d), ["I ♥ you!", "I ♥ you ∞ times more!"]);
    assert_eq!(d.messages().count(), 2);
    // `#59;` is the documented way to write a semicolon in message text.
    let d = ok("sequenceDiagram\n  A->>B: one#59; two");
    assert_eq!(texts(&d), ["one; two"]);
}

// ---------------------------------------------------------------------------------------------
// Activation
// ---------------------------------------------------------------------------------------------

/// `+` activates the **receiver** and `-` deactivates the **sender**. Getting this pair backwards
/// puts every bar on the wrong lifeline while leaving the diagram looking plausible.
#[test]
fn plus_activates_the_receiver_and_minus_deactivates_the_sender() {
    let d = ok("sequenceDiagram\n  Alice->>+John: Hello\n  John-->>-Alice: Great!");
    let kinds: Vec<&Event> = d.events.iter().collect();
    assert!(matches!(kinds[0], Event::Message(m) if m.to == "John"));
    assert_eq!(kinds[1], &Event::Activate("John".to_string()));
    assert!(matches!(kinds[2], Event::Message(m) if m.to == "Alice"));
    assert_eq!(kinds[3], &Event::Deactivate("John".to_string()));
}

#[test]
fn activate_and_deactivate_statements_do_the_same_thing() {
    let d = ok("sequenceDiagram\n  Alice->>John: Hello\n  activate John\n  John-->>Alice: Great!\n  deactivate John");
    assert_eq!(d.events[1], Event::Activate("John".to_string()));
    assert_eq!(d.events[3], Event::Deactivate("John".to_string()));
}

/// `activate` does **not** bring a participant into existence — the grammar drops the actor object
/// that production builds. A parser that creates one here invents a column.
#[test]
fn activate_and_destroy_do_not_declare_a_participant() {
    let d = ok("sequenceDiagram\n  A->>B: x\n  activate Ghost\n  deactivate Ghost");
    assert_eq!(ids(&d), ["A", "B"]);
    let d = ok("sequenceDiagram\n  A->>B: x\n  destroy Ghost");
    assert_eq!(ids(&d), ["A", "B"]);
}

/// …but a note and an actor menu do.
#[test]
fn a_note_and_an_actor_menu_do_declare_a_participant() {
    let d = ok("sequenceDiagram\n  participant John\n  Note right of John: Text in note");
    assert_eq!(ids(&d), ["John"]);
    let d = ok("sequenceDiagram\n  Note right of Solo: alone");
    assert_eq!(ids(&d), ["Solo"]);
    let d = ok("sequenceDiagram\n  links Alice: {\"Dashboard\": \"https://example.test/a\"}");
    assert_eq!(ids(&d), ["Alice"]);
    let d = ok("sequenceDiagram\n  link Bob: Dashboard @ https://example.test/b");
    assert_eq!(ids(&d), ["Bob"]);
}

#[test]
fn stacked_activations_nest() {
    let d = ok(
        "sequenceDiagram\n  Alice->>+John: one\n  Alice->>+John: two\n  John-->>-Alice: three\n  John-->>-Alice: four",
    );
    let opens = d
        .events
        .iter()
        .filter(|e| matches!(e, Event::Activate(_)))
        .count();
    let closes = d
        .events
        .iter()
        .filter(|e| matches!(e, Event::Deactivate(_)))
        .count();
    assert_eq!((opens, closes), (2, 2));
}

/// mermaid throws "Trying to inactivate an inactive participant"; konoma refuses too, and says
/// which participant and which line.
#[test]
fn deactivating_something_that_is_not_active_is_refused() {
    assert_eq!(
        parse("sequenceDiagram\n  A->>B: x\n  deactivate B"),
        Err(ParseError::NotActive {
            name: "B".to_string(),
            line: 3
        })
    );
    assert_eq!(
        parse("sequenceDiagram\n  A->>-B: x"),
        Err(ParseError::NotActive {
            name: "A".to_string(),
            line: 2
        })
    );
    // Closing twice what was opened once is the same mistake.
    assert!(parse("sequenceDiagram\n  A->>+B: x\n  deactivate B\n  deactivate B").is_err());
}

// ---------------------------------------------------------------------------------------------
// Notes
// ---------------------------------------------------------------------------------------------

#[test]
fn every_note_placement_is_read() {
    let d = ok(
        "sequenceDiagram\n  Note left of A: left\n  Note right of A: right\n  Note over A: over\n  Note over A,B: both",
    );
    let notes: Vec<_> = d.notes().collect();
    assert_eq!(notes[0].placement, Placement::LeftOf);
    assert_eq!(notes[1].placement, Placement::RightOf);
    assert_eq!(notes[2].placement, Placement::Over);
    assert_eq!(notes[3].placement, Placement::Over);
    assert_eq!(notes[3].actors, ["A", "B"]);
    assert_eq!(notes[0].actors, ["A"]);
    // `note` is case-insensitive like every other keyword here.
    let d = ok("sequenceDiagram\n  note over A: x");
    assert_eq!(d.notes().count(), 1);
}

/// `note over A,B,C` is `[].concat($3,$3).slice(0,2)` upstream — only the first two count.
#[test]
fn a_note_over_more_than_two_participants_keeps_the_first_two() {
    let d = ok("sequenceDiagram\n  Note over A,B,C: three named");
    assert_eq!(d.notes().next().unwrap().actors, ["A", "B"]);
    // …but all three are still declared, because the actor list is read before it is truncated.
    assert_eq!(ids(&d), ["A", "B", "C"]);
}

// ---------------------------------------------------------------------------------------------
// Blocks
// ---------------------------------------------------------------------------------------------

#[test]
fn every_block_keyword_opens_and_end_closes() {
    let cases: &[(&str, BlockKind)] = &[
        ("loop Every minute", BlockKind::Loop),
        ("alt is sick", BlockKind::Alt),
        ("opt Extra response", BlockKind::Opt),
        ("par Alice to Bob", BlockKind::Par),
        ("par_over Overlapping", BlockKind::ParOver),
        ("critical Establish a connection", BlockKind::Critical),
        ("break when it fails", BlockKind::Break),
        ("rect rgb(191, 223, 255)", BlockKind::Rect),
    ];
    for (opener, kind) in cases {
        let src = format!("sequenceDiagram\n  {opener}\n    A->>B: x\n  end");
        let d = ok(&src);
        match &d.events[0] {
            Event::BlockStart { kind: k, .. } => assert_eq!(k, kind, "{opener}"),
            other => panic!("{opener}: {other:?}"),
        }
        assert_eq!(d.events.last(), Some(&Event::BlockEnd), "{opener}");
    }
}

#[test]
fn a_block_title_is_the_rest_of_the_line() {
    let d = ok("sequenceDiagram\n  loop Every minute\n    A->>B: x\n  end");
    assert_eq!(
        d.events[0],
        Event::BlockStart {
            kind: BlockKind::Loop,
            title: "Every minute".to_string()
        }
    );
    // …and an untitled block is untitled rather than absent.
    let d = ok("sequenceDiagram\n  loop\n    A->>B: x\n  end");
    assert_eq!(
        d.events[0],
        Event::BlockStart {
            kind: BlockKind::Loop,
            title: String::new()
        }
    );
}

#[test]
fn sections_divide_the_block_they_are_in() {
    let d = ok(
        "sequenceDiagram\n  alt is sick\n    A->>B: bad\n  else is well\n    A->>B: good\n  end",
    );
    assert_eq!(
        d.events[2],
        Event::Section {
            kind: SectionKind::Else,
            title: "is well".to_string()
        }
    );
    let d = ok("sequenceDiagram\n  par one\n    A->>B: x\n  and two\n    A->>C: y\n  end");
    assert!(matches!(
        d.events[2],
        Event::Section {
            kind: SectionKind::And,
            ..
        }
    ));
    let d = ok(
        "sequenceDiagram\n  critical connect\n    A->>B: x\n  option timeout\n    A->>B: y\n  end",
    );
    assert!(matches!(
        d.events[2],
        Event::Section {
            kind: SectionKind::Option,
            ..
        }
    ));
}

#[test]
fn blocks_nest() {
    let d = ok(
        "sequenceDiagram\n  par Alice to Bob\n    Alice->>Bob: a\n  and Alice to John\n    par John to Charlie\n      John->>Charlie: b\n    end\n  end",
    );
    let starts = d
        .events
        .iter()
        .filter(|e| matches!(e, Event::BlockStart { .. }))
        .count();
    let ends = d.events.iter().filter(|e| **e == Event::BlockEnd).count();
    assert_eq!((starts, ends), (2, 2));
}

#[test]
fn an_unbalanced_block_is_refused_with_the_line_that_caused_it() {
    assert_eq!(
        parse("sequenceDiagram\n  A->>B: x\n  end"),
        Err(ParseError::UnmatchedEnd { line: 3 })
    );
    assert_eq!(
        parse("sequenceDiagram\n  loop forever\n    A->>B: x"),
        Err(ParseError::UnclosedBlock {
            keyword: "loop",
            line: 2
        })
    );
    assert_eq!(
        parse("sequenceDiagram\n  A->>B: x\n  else nothing"),
        Err(ParseError::SectionOutsideBlock {
            keyword: "else",
            line: 3
        })
    );
}

// ---------------------------------------------------------------------------------------------
// box
// ---------------------------------------------------------------------------------------------

#[test]
fn a_box_groups_the_participants_declared_inside_it() {
    let d = ok(
        "sequenceDiagram\n  box Purple Alice & John\n  participant A\n  participant J\n  end\n  box Another Group\n  participant B\n  end\n  A->>J: x",
    );
    assert_eq!(d.boxes.len(), 2);
    assert_eq!(d.boxes[0].title, "Alice & John");
    assert_eq!(d.boxes[0].color.as_deref(), Some("purple"));
    assert_eq!(d.boxes[0].members, ["A", "J"]);
    // "Another" is not a colour, so the whole line is the title.
    assert_eq!(d.boxes[1].title, "Another Group");
    assert_eq!(d.boxes[1].color, None);
    assert_eq!(d.participant("A").unwrap().box_id.as_deref(), Some("box0"));
    assert_eq!(d.participant("B").unwrap().box_id.as_deref(), Some("box1"));
}

#[test]
fn a_functional_colour_is_taken_off_the_front_of_a_box_title() {
    for (line, color, title) in [
        (
            "box rgb(33,66,99) Servers",
            Some("rgb(33,66,99)"),
            "Servers",
        ),
        ("box rgba(33,66,99,0.5)", Some("rgba(33,66,99,0.5)"), ""),
        (
            "box hsl(10, 40%, 90%) Warm",
            Some("hsl(10, 40%, 90%)"),
            "Warm",
        ),
        ("box transparent Aqua", Some("transparent"), "Aqua"),
        (
            "box Group without description",
            None,
            "Group without description",
        ),
    ] {
        let src = format!("sequenceDiagram\n  {line}\n  participant A\n  end\n  A->>B: x");
        let d = ok(&src);
        assert_eq!(d.boxes[0].color.as_deref(), color, "{line}");
        assert_eq!(d.boxes[0].title, title, "{line}");
    }
}

// ---------------------------------------------------------------------------------------------
// create / destroy
// ---------------------------------------------------------------------------------------------

#[test]
fn create_and_destroy_are_recorded_against_the_event_stream() {
    let d = ok(
        "sequenceDiagram\n  Alice->>Bob: Hello\n  create participant Carl\n  Alice->>Carl: Hi Carl!\n  create actor D as Donald\n  Carl->>D: Hi!\n  destroy Carl\n  Alice-xCarl: too many\n  destroy Bob\n  Bob->>Alice: I agree",
    );
    assert_eq!(ids(&d), ["Alice", "Bob", "Carl", "D"]);
    assert_eq!(labels(&d)[3], "Donald");
    assert_eq!(d.participant("D").unwrap().kind, ParticipantKind::Actor);
    assert!(d.participant("Carl").unwrap().created_at.is_some());
    assert!(d.participant("Carl").unwrap().destroyed_at.is_some());
    assert!(d.participant("Bob").unwrap().created_at.is_none());
    assert!(d.participant("Bob").unwrap().destroyed_at.is_some());
}

/// mermaid throws here, because the two would share one column.
#[test]
fn creating_a_participant_that_already_exists_is_refused() {
    assert_eq!(
        parse("sequenceDiagram\n  A->>B: x\n  create participant B\n  A->>B: y"),
        Err(ParseError::DuplicateCreate {
            name: "B".to_string(),
            line: 3
        })
    );
}

// ---------------------------------------------------------------------------------------------
// autonumber, title, accessibility
// ---------------------------------------------------------------------------------------------

#[test]
fn autonumber_reads_all_four_forms() {
    let d = ok("sequenceDiagram\n  autonumber\n  A->>B: x");
    assert_eq!(
        d.events[0],
        Event::AutoNumber(AutoNumber::On {
            start: 1.0,
            step: 1.0
        })
    );
    let d = ok("sequenceDiagram\n  autonumber off\n  A->>B: x");
    assert_eq!(d.events[0], Event::AutoNumber(AutoNumber::Off));
    let d = ok("sequenceDiagram\n  autonumber 10\n  A->>B: x");
    assert_eq!(
        d.events[0],
        Event::AutoNumber(AutoNumber::On {
            start: 10.0,
            step: 1.0
        })
    );
    // Two decimals are legal — the NUM token is `[0-9]+(\.[0-9]{1,2})?`.
    let d = ok("sequenceDiagram\n  autonumber 1.5 0.25\n  A->>B: x");
    assert_eq!(
        d.events[0],
        Event::AutoNumber(AutoNumber::On {
            start: 1.5,
            step: 0.25
        })
    );
}

#[test]
fn a_title_is_read_in_all_three_spellings() {
    assert_eq!(
        ok("sequenceDiagram\n  title Checkout flow\n  A->>B: x")
            .title
            .as_deref(),
        Some("Checkout flow")
    );
    assert_eq!(
        ok("sequenceDiagram\n  title: Checkout flow\n  A->>B: x")
            .title
            .as_deref(),
        Some("Checkout flow")
    );
    assert_eq!(
        ok("---\ntitle: Checkout flow\n---\nsequenceDiagram\n  A->>B: x")
            .title
            .as_deref(),
        Some("Checkout flow")
    );
}

#[test]
fn accessibility_statements_are_read_and_never_drawn() {
    let d = ok("sequenceDiagram\n  accTitle: The flow\n  accDescr: What happens\n  A->>B: x");
    assert_eq!(d.acc_title.as_deref(), Some("The flow"));
    assert_eq!(d.acc_descr.as_deref(), Some("What happens"));
    assert_eq!(
        ids(&d),
        ["A", "B"],
        "an acc statement must not become a participant"
    );

    let d = ok("sequenceDiagram\n  accDescr {\n    two lines\n    of description\n  }\n  A->>B: x");
    assert_eq!(
        d.acc_descr.as_deref(),
        Some("two lines\n    of description")
    );
    assert_eq!(ids(&d), ["A", "B"]);
}

// ---------------------------------------------------------------------------------------------
// Parsed and deliberately not drawn — "描画不要 ≠ パース不要"
// ---------------------------------------------------------------------------------------------

/// Every statement konoma reads and throws away. The assertion that matters in each case is that
/// **no phantom participant appears** and no message is invented: a statement a line-based parser
/// fails to recognise is exactly how `classDef hot fill:#f9f` became a box in the flowchart that
/// this project already had to fix once.
#[test]
fn statements_that_are_parsed_and_dropped_leave_nothing_behind() {
    let cases: &[(&str, &str)] = &[
        (
            "actor menus (advanced)",
            "sequenceDiagram\n  participant Alice\n  participant John\n  links Alice: {\"Dashboard\": \"https://d.test/a\", \"Wiki\": \"https://w.test/a\"}\n  links John: {\"Dashboard\": \"https://d.test/j\"}\n  Alice->>John: Hello",
        ),
        (
            "actor menus (simple)",
            "sequenceDiagram\n  participant Alice\n  link Alice: Dashboard @ https://d.test/a\n  Alice->>John: Hello",
        ),
        (
            "properties",
            "sequenceDiagram\n  participant Alice\n  properties Alice: {\"class\": \"internal-service-actor\"}\n  Alice->>John: Hello",
        ),
        (
            "details",
            "sequenceDiagram\n  participant Alice\n  details Alice: {\"class\": \"x\"}\n  Alice->>John: Hello",
        ),
        (
            "accessibility",
            "sequenceDiagram\n  accTitle: t\n  accDescr: d\n  Alice->>John: Hello",
        ),
        (
            "directive and front matter",
            "---\nconfig:\n  theme: forest\n---\n%%{init: {'theme':'dark'}}%%\nsequenceDiagram\n  Alice->>John: Hello",
        ),
        (
            "comments",
            "sequenceDiagram\n  %% a comment\n  # another comment\n  Alice->>John: Hello",
        ),
    ];
    for (name, src) in cases {
        let d = ok(src);
        assert!(
            d.participants
                .iter()
                .all(|p| p.id == "Alice" || p.id == "John"),
            "{name}: a dropped statement invented a participant: {:?}",
            ids(&d)
        );
        assert_eq!(d.messages().count(), 1, "{name}: message count");
        assert_eq!(texts(&d), ["Hello"], "{name}");
    }
}

/// `()` central connections and `par_over` are read; both draw as their plain counterparts.
#[test]
fn central_connections_are_read_and_drawn_as_plain_messages() {
    let d = ok("sequenceDiagram\n  participant Alice\n  participant John\n  Alice->>()John: Hello John\n  Alice()->>John: How are you?\n  John()->>()Alice: Great!");
    assert_eq!(
        ids(&d),
        ["Alice", "John"],
        "`()` must not become part of a name"
    );
    assert_eq!(texts(&d), ["Hello John", "How are you?", "Great!"]);
}

// ---------------------------------------------------------------------------------------------
// Adversarial
// ---------------------------------------------------------------------------------------------

#[test]
fn the_message_ceiling_holds() {
    let mut src = String::from("sequenceDiagram\n");
    for i in 0..(MAX_MESSAGES_FOR_TESTS + 1) {
        src.push_str(&format!("  A->>B: m{i}\n"));
    }
    assert_eq!(
        parse(&src),
        Err(ParseError::TooManyMessages {
            limit: MAX_MESSAGES_FOR_TESTS
        })
    );
}

/// Sources chosen to be awkward rather than realistic: each must come back with a diagram or an
/// error, never a panic. The renderer runs inside `catch_unwind` (§1) — a safety net, not a
/// licence.
#[test]
fn awkward_sources_never_panic() {
    let mut deep = String::from("sequenceDiagram\n");
    for i in 0..40 {
        deep.push_str(&format!("  loop level {i}\n"));
    }
    deep.push_str("  A->>B: x\n");
    for _ in 0..40 {
        deep.push_str("  end\n");
    }
    let mut wide = String::from("sequenceDiagram\n");
    for i in 0..100 {
        wide.push_str(&format!("  p{i}->>p{}: m{i}\n", i + 1));
    }

    let cases: &[&str] = &[
        "sequenceDiagram",
        "sequenceDiagram\n",
        "sequenceDiagram\n   \n\n  ",
        "sequenceDiagram\n  ->>: ",
        "sequenceDiagram\n  A->>: no target",
        "sequenceDiagram\n  ->>B: no source",
        "sequenceDiagram\n  participant",
        "sequenceDiagram\n  participant as",
        "sequenceDiagram\n  actor",
        "sequenceDiagram\n  note",
        "sequenceDiagram\n  note over",
        "sequenceDiagram\n  note over : text",
        "sequenceDiagram\n  activate",
        "sequenceDiagram\n  deactivate",
        "sequenceDiagram\n  destroy",
        "sequenceDiagram\n  create",
        "sequenceDiagram\n  box\n  end",
        "sequenceDiagram\n  box\n  box\n  end\n  end",
        "sequenceDiagram\n  autonumber nonsense\n  A->>B: x",
        "sequenceDiagram\n  autonumber 1e400 1\n  A->>B: x",
        "sequenceDiagram\n  A->>B: 全画面プレビュー\n  note over A,B: 端末が画像を話せば",
        "sequenceDiagram\n  参加者->>ファイル: 読む",
        "sequenceDiagram\n  A->>B: 🎉🚀\n",
        "sequenceDiagram\n  A@{->>B: x",
        "sequenceDiagram\n  participant A@{ broken\n  A->>B: x",
        "sequenceDiagram\n  A->>B: <script>alert(1)</script>",
        "sequenceDiagram\n  A->>B: a < b & c > \"d\"",
        "sequenceDiagram\n  #\n  A->>B: x",
        "sequenceDiagram\n  ;;;;\n  A->>B: x",
        "sequenceDiagram\n  A->>B: #",
        "sequenceDiagram\n  A->>B: #;",
        "sequenceDiagram\n  loop\n  else\n  end",
        &deep,
        &wide,
    ];
    for src in cases {
        if let Ok(d) = parse(src) {
            // Whatever came back has to be self-consistent: every message names a participant
            // that exists, because that is the invariant the renderer relies on.
            for m in d.messages() {
                assert!(d.has(&m.from), "{src:?}: message from unknown {:?}", m.from);
                assert!(d.has(&m.to), "{src:?}: message to unknown {:?}", m.to);
            }
            for n in d.notes() {
                for a in &n.actors {
                    assert!(d.has(a), "{src:?}: note on unknown {a:?}");
                }
            }
        }
    }
}

/// Every error carries the line it happened on, because that is the whole reason konoma is
/// writing these parsers (§8: the crate throws its messages away).
#[test]
fn errors_say_what_was_wrong_and_where() {
    let cases: &[(&str, &str)] = &[
        (
            "flowchart TD\n  A --> B",
            "not a sequence diagram: `flowchart`",
        ),
        ("", "sequence diagram is empty"),
        (
            "sequenceDiagram\n  A->>B: x\n  end",
            "`end` with no open block at line 3",
        ),
        (
            "sequenceDiagram\n  loop x\n    A->>B: y",
            "`loop` at line 2 was never closed with `end`",
        ),
        (
            "sequenceDiagram\n  A->>B: x\n  deactivate B",
            "`B` is not active at line 3",
        ),
        (
            "sequenceDiagram\n  A->>B: x\n  create participant B",
            "`B` already exists and cannot be created at line 3",
        ),
    ];
    for (src, expected) in cases {
        let e = parse(src).expect_err(src);
        assert_eq!(e.to_string(), *expected, "{src:?}");
    }
}
