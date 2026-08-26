//! What the ZenUML reader is checked with.
//!
//! Every case is one of `zenuml.md`'s own sixteen examples, or the refusal of something outside
//! the statement layer — which is the half that matters most here, because the whole design of
//! this module is "read what is documented and refuse the rest".

use super::*;
use crate::preview::mermaid::sequence::Event;

fn ok(src: &str) -> SequenceDiagram {
    parse(src).unwrap_or_else(|e| panic!("{e}\n{src}"))
}

fn messages(d: &SequenceDiagram) -> Vec<(String, String, String)> {
    d.events
        .iter()
        .filter_map(|e| match e {
            Event::Message(m) => Some((m.from.clone(), m.to.clone(), m.text.clone())),
            _ => None,
        })
        .collect()
}

fn names(d: &SequenceDiagram) -> Vec<String> {
    d.participants.iter().map(|p| p.label.clone()).collect()
}

#[test]
fn the_documented_async_example_reads_as_three_messages() {
    let d = ok("zenuml\n    title Demo\n    Alice->John: Hello John, how are you?\n    John->Alice: Great!\n    Alice->John: See you later!\n");
    assert_eq!(d.title.as_deref(), Some("Demo"));
    assert_eq!(names(&d), ["Alice", "John"]);
    assert_eq!(
        messages(&d),
        [
            (
                "Alice".into(),
                "John".into(),
                "Hello John, how are you?".into()
            ),
            ("John".into(), "Alice".into(), "Great!".into()),
            ("Alice".into(), "John".into(), "See you later!".into()),
        ]
    );
}

#[test]
fn a_bare_name_declares_a_participant_and_fixes_its_order() {
    let d = ok("zenuml\n    title Declare participant (optional)\n    Bob\n    Alice\n    Alice->Bob: Hi Bob\n    Bob->Alice: Hi Alice\n");
    assert_eq!(names(&d), ["Bob", "Alice"]);
}

#[test]
fn an_annotator_declares_a_participant_and_actor_gets_the_stick_figure() {
    let d = ok("zenuml\n    title Annotators\n    @Actor Alice\n    @Database Bob\n    Alice->Bob: Hi Bob\n");
    assert_eq!(names(&d), ["Alice", "Bob"]);
    assert_eq!(d.participants[0].kind, ParticipantKind::Actor);
}

#[test]
fn an_alias_is_the_label_and_the_short_name_is_the_id() {
    let d = ok("zenuml\n    title Aliases\n    A as Alice\n    J as John\n    A->J: Hello John, how are you?\n");
    assert_eq!(d.participants[0].id, "A");
    assert_eq!(d.participants[0].label, "Alice");
    assert_eq!(messages(&d)[0].0, "A");
}

/// A top-level sync call comes from somewhere, and that somewhere gets a lifeline.
#[test]
fn a_sync_message_is_drawn_from_the_starter_and_nests() {
    let d = ok("zenuml\n    title Sync message\n    A.SyncMessage\n    A.SyncMessage(with, parameters) {\n      B.nestedSyncMessage()\n    }\n");
    assert_eq!(names(&d)[0], STARTER);
    assert_eq!(
        messages(&d),
        [
            (STARTER.into(), "A".into(), "SyncMessage".into()),
            (
                STARTER.into(),
                "A".into(),
                "SyncMessage(with, parameters)".into()
            ),
            ("A".into(), "B".into(), "nestedSyncMessage()".into()),
        ]
    );
    // …and the callee is active for the length of its block.
    let activates = d
        .events
        .iter()
        .filter(|e| matches!(e, Event::Activate(_) | Event::Deactivate(_)))
        .count();
    assert_eq!(activates, 2);
}

#[test]
fn a_named_starter_replaces_the_default_one_and_comes_first() {
    let d = ok("zenuml\n  @Starter(Client)\n  A.method()\n");
    assert_eq!(names(&d), ["Client", "A"]);
    assert_eq!(messages(&d)[0].0, "Client");
}

#[test]
fn a_creation_message_declares_the_participant_it_creates() {
    let d = ok("zenuml\n    new A1\n    new A2(with, parameters)\n");
    assert!(d
        .events
        .iter()
        .any(|e| matches!(e, Event::Create(n) if n == "A1")));
    assert_eq!(
        messages(&d),
        [
            (STARTER.into(), "A1".into(), "new".into()),
            (STARTER.into(), "A2".into(), "new(with, parameters)".into()),
        ]
    );
}

/// All three of the documented ways to show a reply.
#[test]
fn every_documented_reply_form_is_a_dotted_message_back_to_the_caller() {
    let d = ok("zenuml\n    a = A.SyncMessage()\n    SomeType b = A.SyncMessage()\n    A.SyncMessage() {\n    return result\n    }\n    @return\n    A->B: result\n");
    let m = messages(&d);
    assert_eq!(m[1], (String::from("A"), STARTER.into(), "a".into()));
    assert_eq!(m[3], (String::from("A"), STARTER.into(), "b".into()));
    assert_eq!(m[5], (String::from("A"), STARTER.into(), "result".into()));
    // The `@return` one is an async message drawn dotted.
    let last = d
        .events
        .iter()
        .rev()
        .find_map(|e| match e {
            Event::Message(m) => Some(m.clone()),
            _ => None,
        })
        .expect("a message");
    assert_eq!(last.signal.line, Line::Dotted);
    assert_eq!(last.from, "A");
    assert_eq!(last.to, "B");
}

#[test]
fn a_caller_written_on_the_left_of_a_sync_call_is_the_sender() {
    let d = ok("zenuml\n    Client->A.method() {\n      B.method() {\n        return x1\n      }\n      return x2\n    }\n");
    let m = messages(&d);
    assert_eq!(m[0], ("Client".into(), "A".into(), "method()".into()));
    assert_eq!(m[1], ("A".into(), "B".into(), "method()".into()));
    assert_eq!(m[2], ("B".into(), "A".into(), "x1".into()));
    assert_eq!(m[3], ("A".into(), "Client".into(), "x2".into()));
}

#[test]
fn while_becomes_a_loop_and_if_becomes_an_alt() {
    let d = ok("zenuml\n    Alice->John: Hello John, how are you?\n    while(true) {\n      John->Alice: Great!\n    }\n");
    assert!(d.events.iter().any(
        |e| matches!(e, Event::BlockStart { kind, title } if *kind == BlockKind::Loop && title == "true")
    ));

    let d = ok("zenuml\n    Alice->Bob: Hello Bob, how are you?\n    if(is_sick) {\n      Bob->Alice: Not so good :(\n    } else {\n      Bob->Alice: Feeling fresh like a daisy\n    }\n");
    assert!(d.events.iter().any(
        |e| matches!(e, Event::BlockStart { kind, title } if *kind == BlockKind::Alt && title == "is_sick")
    ));
    assert!(d
        .events
        .iter()
        .any(|e| matches!(e, Event::Section { kind, .. } if *kind == SectionKind::Else)));
}

#[test]
fn else_if_keeps_its_condition_on_the_section() {
    let d = ok("zenuml\n  if(a) {\n    X->Y: one\n  } else if(b) {\n    X->Y: two\n  } else {\n    X->Y: three\n  }\n");
    let sections: Vec<String> = d
        .events
        .iter()
        .filter_map(|e| match e {
            Event::Section { title, .. } => Some(title.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(sections, ["if b", ""]);
}

#[test]
fn opt_and_par_become_the_frames_of_the_same_name() {
    let d = ok("zenuml\n    Alice->Bob: Hello Bob, how are you?\n    opt {\n      Bob->Alice: Thanks for asking\n    }\n");
    assert!(d
        .events
        .iter()
        .any(|e| matches!(e, Event::BlockStart { kind, .. } if *kind == BlockKind::Opt)));
    let d = ok("zenuml\n    par {\n        Alice->Bob: Hello guys!\n        Alice->John: Hello guys!\n    }\n");
    assert!(d
        .events
        .iter()
        .any(|e| matches!(e, Event::BlockStart { kind, .. } if *kind == BlockKind::Par)));
}

/// A `try` frame says "try", not "critical" — which is why the sequence model grew a variant
/// rather than borrowing one.
#[test]
fn try_catch_finally_becomes_a_try_frame_with_its_own_sections() {
    let d = ok("zenuml\n    try {\n      Consumer->API: Book something\n    } catch {\n      API->Consumer: show failure\n    } finally {\n      API->BookingService: rollback status\n    }\n");
    assert!(d
        .events
        .iter()
        .any(|e| matches!(e, Event::BlockStart { kind, .. } if *kind == BlockKind::Try)));
    let sections: Vec<SectionKind> = d
        .events
        .iter()
        .filter_map(|e| match e {
            Event::Section { kind, .. } => Some(*kind),
            _ => None,
        })
        .collect();
    assert_eq!(sections, [SectionKind::Catch, SectionKind::Finally]);
    assert_eq!(BlockKind::Try.keyword(), "try");
    assert_eq!(SectionKind::Catch.keyword(), "catch");
}

#[test]
fn a_comment_is_not_a_participant() {
    let d = ok("zenuml\n    // a comment on a participant will not be rendered\n    BookService\n    // a comment on a message.\n    BookService.getBook()\n");
    assert_eq!(names(&d), [STARTER, "BookService"]);
    assert_eq!(messages(&d).len(), 1);
}

/// **The point of the whole module.** Everything outside the statement layer is refused, so it
/// degrades to the text diagram rather than becoming a participant named after a brace.
#[test]
fn what_is_outside_the_statement_layer_is_refused_rather_than_guessed_at() {
    for src in [
        // A method chain is expression syntax.
        "zenuml\n  A.b().c()\n",
        // An arithmetic condition is too.
        "zenuml\n  x = 1 + 2\n",
        // A divider.
        "zenuml\n  ==Section==\n",
        // A stray closing brace.
        "zenuml\n  A->B: x\n  }\n",
        // An `else` with no `if`.
        "zenuml\n  while(a) {\n    A->B: x\n  } else {\n  }\n",
        // A `catch` on something that is not a `try`.
        "zenuml\n  if(a) {\n    A->B: x\n  } catch {\n  }\n",
        // Two words that are neither a message nor an alias.
        "zenuml\n  some words here\n",
        // An unclosed block.
        "zenuml\n  if(a) {\n    A->B: x\n",
        // A `return` with nothing to return to.
        "zenuml\n  A->B: x\n  return y\n",
    ] {
        assert!(
            parse(src).is_err(),
            "should be refused so it degrades to the text diagram: {src:?}"
        );
    }
}

#[test]
fn a_header_with_nothing_in_it_is_refused() {
    assert!(matches!(parse("zenuml\n"), Err(ParseError::NoData { .. })));
}

#[test]
fn the_routing_predicate_and_the_parser_agree() {
    for src in [
        "zenuml\n  A->B: x",
        "ZENUML\n  A->B: x",
        "zenuml",
        "zenumls\n  a",
        "sequenceDiagram\n  A->>B: hi",
        "",
        "%% only a comment\n",
    ] {
        let refused = matches!(parse(src), Err(ParseError::NotThisChart { .. }));
        assert_eq!(is_zenuml(src), !refused, "{src:?}");
    }
}
