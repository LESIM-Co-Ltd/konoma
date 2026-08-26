//! What the ER parser is checked with.
//!
//! The corpus is every `erDiagram` fence in mermaid's own
//! `docs/syntax/entityRelationshipDiagram.md`, in the form it is written there, plus the cases the
//! *grammar* distinguishes that the documentation does not show — the four symbol cardinalities
//! mirrored, the word spellings that mean the same thing, `.-` and `-.`, and case.
//!
//! As in the class parser's tests, "must drop" is asserted as strongly as "must produce": a
//! `classDef` konoma failed to recognise becomes a phantom entity (§2-3).

use super::model::{Cardinality, Identification};
use super::parser::{read_relationship, MAX_RELATIONSHIPS_FOR_TESTS};
use super::*;

fn ok(src: &str) -> ErDiagram {
    parse(src).unwrap_or_else(|e| panic!("should parse: {e}\n{src}"))
}

/// The cardinality at each end of one relationship, as a pair.
fn cards(src: &str) -> Vec<(Cardinality, Cardinality)> {
    ok(src)
        .relationships
        .iter()
        .map(|r| (r.from_cardinality, r.to_cardinality))
        .collect()
}

// ---------------------------------------------------------------------------------------------
// 1. The header decides, and only the header
// ---------------------------------------------------------------------------------------------

/// The ER lexer is `%options case-insensitive`, which is unique among the four grammars konoma
/// reads. A case-sensitive header check silently refuses a real diagram.
#[test]
fn the_header_decides_and_is_case_insensitive() {
    for src in [
        "erDiagram\n  A ||--|| B : x",
        "ERDIAGRAM\n  A ||--|| B : x",
        "erdiagram\n  A ||--|| B : x",
        "ErDiagram\n  A ||--|| B : x",
        "  \n\n  erDiagram\n  A",
        "%% a comment\nerDiagram\n  A",
        "---\ntitle: t\n---\nerDiagram\n  A",
    ] {
        assert!(parse(src).is_ok(), "{src:?}");
    }
    for (src, header) in [
        ("flowchart TD\n  A --> B", "flowchart"),
        ("classDiagram\n  A --> B", "classDiagram"),
        ("stateDiagram-v2\n  [*] --> A", "stateDiagram-v2"),
        ("erDiagrams --> B", "erDiagrams"),
    ] {
        assert_eq!(
            parse(src).unwrap_err(),
            ParseError::NotAnErDiagram {
                header: header.to_string()
            },
            "{src:?}"
        );
    }
    assert_eq!(parse("").unwrap_err(), ParseError::Empty);
    assert_eq!(parse("erDiagram").unwrap_err(), ParseError::NoEntities);
}

#[test]
fn front_matter_is_lifted_out_and_line_numbers_survive_it() {
    let d = ok("---\ntitle: Order example\n---\nerDiagram\n  A ||--|| B : x");
    assert_eq!(d.title.as_deref(), Some("Order example"));
    let e = parse("---\ntitle: t\n---\nerDiagram\n  A\n  B {\n    string x").unwrap_err();
    assert_eq!(e.to_string(), "unclosed `B {` opened at line 6");
}

// ---------------------------------------------------------------------------------------------
// 2. Entities
// ---------------------------------------------------------------------------------------------

#[test]
fn an_entity_can_be_declared_in_every_documented_form() {
    // Bare, including the `-` that `UNICODE_TEXT` allows.
    let d = ok("erDiagram\n  CUSTOMER\n  LINE-ITEM\n  DELIVERY-ADDRESS");
    assert_eq!(
        d.entity_ids(),
        ["CUSTOMER", "LINE-ITEM", "DELIVERY-ADDRESS"]
    );
    // Quoted — `ENTITY_NAME` allows spaces where `UNICODE_TEXT` does not.
    let d = ok("erDiagram\n    \"This ❤ Unicode\"");
    assert_eq!(d.entity_ids(), ["This ❤ Unicode"]);
    // Aliased: the id stays short and the *label* is what is drawn.
    let d = ok("erDiagram\n  p[Person] {\n    string firstName\n  }\n  a[\"Customer Account\"] {\n    string email\n  }\n  p ||--o| a : has");
    assert_eq!(d.entity_ids(), ["p", "a"]);
    assert_eq!(d.entity("p").unwrap().label, "Person");
    assert_eq!(d.entity("a").unwrap().label, "Customer Account");
    // With a css class.
    let d = ok("erDiagram\n  CAR:::someclass {\n    string make\n  }\n  HOUSE:::someclass\n  classDef someclass fill:#f96");
    assert_eq!(d.entity("CAR").unwrap().css_classes, ["someclass"]);
    assert_eq!(d.entity("HOUSE").unwrap().css_classes, ["someclass"]);
    // …including on a relationship's endpoint.
    let d = ok("erDiagram\n  PERSON:::foo ||--|| CAR : owns\n  classDef foo stroke:#f00");
    assert_eq!(d.entity("PERSON").unwrap().css_classes, ["foo"]);
    assert_eq!(d.entity_ids(), ["PERSON", "CAR"]);
}

#[test]
fn an_attribute_block_is_read_column_by_column() {
    let d = ok("erDiagram\n  PERSON {\n    string driversLicense PK \"The license #\"\n    string(99) firstName \"Only 99 characters are allowed\"\n    string lastName\n    string phone UK\n    string[] parts\n    string? middleName\n    int age\n  }");
    let a = &d.entity("PERSON").unwrap().attributes;
    let seen: Vec<(&str, &str, Vec<&str>, &str)> = a
        .iter()
        .map(|x| {
            (
                x.kind.as_str(),
                x.name.as_str(),
                x.keys.iter().map(String::as_str).collect(),
                x.comment.as_str(),
            )
        })
        .collect();
    assert_eq!(
        seen,
        [
            ("string", "driversLicense", vec!["PK"], "The license #"),
            (
                "string(99)",
                "firstName",
                vec![],
                "Only 99 characters are allowed"
            ),
            ("string", "lastName", vec![], ""),
            ("string", "phone", vec!["UK"], ""),
            ("string[]", "parts", vec![], ""),
            ("string?", "middleName", vec![], ""),
            ("int", "age", vec![], ""),
        ]
    );
}

#[test]
fn several_keys_on_one_attribute_are_read_as_several() {
    let d = ok("erDiagram\n  NAMED-DRIVER {\n    string carRegistrationNumber PK, FK\n    string other PK,UK\n    string lower pk\n  }");
    let a = &d.entity("NAMED-DRIVER").unwrap().attributes;
    assert_eq!(a[0].keys, ["PK", "FK"]);
    assert_eq!(a[1].keys, ["PK", "UK"]);
    // Case-insensitive, like everything else in this grammar.
    assert_eq!(a[2].keys, ["PK"]);
}

/// `attribute : attributeType attributeName` — two words minimum. A one-word line is not an
/// attribute, and inventing a nameless row for it would put a blank line in the box.
#[test]
fn a_one_word_line_is_not_an_attribute() {
    let d = ok("erDiagram\n  A {\n    string\n    string name\n  }");
    assert_eq!(d.entity("A").unwrap().attributes.len(), 1);
    assert_eq!(d.entity("A").unwrap().attributes[0].name, "name");
}

#[test]
fn an_empty_attribute_block_leaves_an_entity_with_no_rows() {
    let d = ok("erDiagram\n  A { }\n  B {\n  }");
    assert_eq!(d.entity_ids(), ["A", "B"]);
    assert!(d.entity("A").unwrap().attributes.is_empty());
    assert!(d.entity("B").unwrap().attributes.is_empty());
}

// ---------------------------------------------------------------------------------------------
// 3. Relationships
// ---------------------------------------------------------------------------------------------

/// **The mirrored spelling is the picture.** The character nearest an entity is the mark drawn
/// nearest that entity, so `||--o{` is "one" at the left and "zero or more" at the right. All
/// four cardinalities, in both mirrorings.
#[test]
fn every_symbol_cardinality_reads_at_the_end_it_was_written_on() {
    assert_eq!(
        cards("erDiagram\n  A ||--|| B : x"),
        [(Cardinality::OnlyOne, Cardinality::OnlyOne)]
    );
    assert_eq!(
        cards("erDiagram\n  A |o--o| B : x"),
        [(Cardinality::ZeroOrOne, Cardinality::ZeroOrOne)]
    );
    assert_eq!(
        cards("erDiagram\n  A }o--o{ B : x"),
        [(Cardinality::ZeroOrMore, Cardinality::ZeroOrMore)]
    );
    assert_eq!(
        cards("erDiagram\n  A }|--|{ B : x"),
        [(Cardinality::OneOrMore, Cardinality::OneOrMore)]
    );
    // Mixed, which is the documented shape.
    assert_eq!(
        cards("erDiagram\n  CUSTOMER ||--o{ ORDER : places\n  ORDER ||--|{ LINE-ITEM : contains\n  CUSTOMER }|..|{ DELIVERY-ADDRESS : uses"),
        [
            (Cardinality::OnlyOne, Cardinality::ZeroOrMore),
            (Cardinality::OnlyOne, Cardinality::OneOrMore),
            (Cardinality::OneOrMore, Cardinality::OneOrMore),
        ]
    );
}

/// The word spellings mean exactly what the symbols mean. mermaid's own documentation writes
/// `CAR 1 to zero or more NAMED-DRIVER : allows` next to `CAR ||--o{ NAMED-DRIVER : allows` and
/// calls them the same diagram, so the two have to parse the same way.
#[test]
fn the_word_spellings_mean_what_the_symbols_mean() {
    assert_eq!(
        cards("erDiagram\n  CAR 1 to zero or more NAMED-DRIVER : allows"),
        cards("erDiagram\n  CAR ||--o{ NAMED-DRIVER : allows")
    );
    assert_eq!(
        cards("erDiagram\n  PERSON many(0) optionally to 0+ NAMED-DRIVER : is"),
        [(Cardinality::ZeroOrMore, Cardinality::ZeroOrMore)]
    );
    let d = ok("erDiagram\n  PERSON many(0) optionally to 0+ NAMED-DRIVER : is");
    assert_eq!(
        d.relationships[0].identification,
        Identification::NonIdentifying,
        "`optionally to` is the word form of `..`"
    );
    for (word, expected) in [
        ("one or zero", Cardinality::ZeroOrOne),
        ("one or more", Cardinality::OneOrMore),
        ("one or many", Cardinality::OneOrMore),
        ("zero or one", Cardinality::ZeroOrOne),
        ("zero or more", Cardinality::ZeroOrMore),
        ("zero or many", Cardinality::ZeroOrMore),
        ("only one", Cardinality::OnlyOne),
        ("many(1)", Cardinality::OneOrMore),
        ("many", Cardinality::ZeroOrMore),
        ("one", Cardinality::OnlyOne),
        ("1+", Cardinality::OneOrMore),
        ("0+", Cardinality::ZeroOrMore),
        ("1", Cardinality::OnlyOne),
    ] {
        let right = format!("erDiagram\n  A 1 to {word} B : x");
        assert_eq!(
            cards(&right)[0].1,
            expected,
            "the word form {word:?} on the right"
        );
        let left = format!("erDiagram\n  A {word} to 1 B : x");
        assert_eq!(cards(&left)[0].0, expected, "…and on the left: {word:?}");
    }
}

#[test]
fn identifying_and_non_identifying_have_four_spellings_between_them() {
    let seen: Vec<Identification> =
        ok("erDiagram\n  A ||--|| B : w\n  C ||..|| D : x\n  E ||.-|| F : y\n  G ||-.|| H : z")
            .relationships
            .iter()
            .map(|r| r.identification)
            .collect();
    assert_eq!(
        seen,
        [
            Identification::Identifying,
            Identification::NonIdentifying,
            Identification::NonIdentifying,
            Identification::NonIdentifying,
        ]
    );
}

#[test]
fn a_relationship_needs_no_spaces_around_it() {
    let d = ok("erDiagram\n  id1||--||id2 : label");
    assert_eq!(d.entity_ids(), ["id1", "id2"]);
    assert_eq!(d.relationships[0].label.as_deref(), Some("label"));
}

/// The `: role` is required by the grammar, and `: ""` is the documented way to write "no role".
#[test]
fn an_empty_role_is_no_label_rather_than_an_empty_one() {
    let d = ok("erDiagram\n  A ||--|| B : \"\"\n  C ||--|| D : \"has\"\n  E ||--|| F : plain");
    let labels: Vec<Option<&str>> = d.relationships.iter().map(|r| r.label.as_deref()).collect();
    assert_eq!(labels, [None, Some("has"), Some("plain")]);
}

/// `u` is `MD_PARENT`, which has no documented meaning and no marker upstream. It is parsed so
/// that it cannot end up inside the entity's name, and drawn as nothing.
#[test]
fn the_undocumented_md_parent_is_parsed_and_kept_out_of_the_name() {
    let d = ok("erDiagram\n  G u--|| H : w");
    assert_eq!(d.entity_ids(), ["G", "H"]);
    assert_eq!(d.relationships[0].from_cardinality, Cardinality::MdParent);
    // A `u` that is part of a name is not a mark.
    let d = ok("erDiagram\n  MENU ||--|| H : w");
    assert_eq!(d.entity_ids(), ["MENU", "H"]);
    assert_eq!(d.relationships[0].from_cardinality, Cardinality::OnlyOne);
}

#[test]
fn the_relationship_ceiling_holds() {
    let mut src = String::from("erDiagram\n");
    for i in 0..(MAX_RELATIONSHIPS_FOR_TESTS + 50) {
        src.push_str(&format!("  n{i} ||--|| m{i} : r\n"));
    }
    assert_eq!(ok(&src).relationships.len(), MAX_RELATIONSHIPS_FOR_TESTS);
}

// ---------------------------------------------------------------------------------------------
// 4. Subgraphs
// ---------------------------------------------------------------------------------------------

#[test]
fn a_subgraph_holds_what_is_written_inside_it_and_can_nest() {
    let d = ok("erDiagram\n    subgraph title1\n        CUSTOMER\n        CUSTOMER {\n            string name\n        }\n    end\n    subgraph title2\n        CAR ||--o{ NAMED-DRIVER : allows\n        subgraph title3\n            PERSON\n        end\n    end");
    let ids: Vec<&str> = d.subgraphs.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(ids, ["title1", "title2", "title3"]);
    assert_eq!(d.subgraphs[0].members, ["CUSTOMER"]);
    assert_eq!(d.subgraphs[1].members, ["CAR", "NAMED-DRIVER", "title3"]);
    assert_eq!(d.subgraphs[2].parent.as_deref(), Some("title2"));
    assert_eq!(
        d.entity("PERSON").unwrap().parent.as_deref(),
        Some("title3")
    );
    assert_eq!(d.entity("CUSTOMER").unwrap().attributes.len(), 1);
}

#[test]
fn a_subgraph_can_be_titled_and_quoted() {
    let d = ok("erDiagram\n  subgraph id1 [title 1]\n    CUSTOMER\n  end");
    assert_eq!(d.subgraphs[0].id, "id1");
    assert_eq!(d.subgraphs[0].title, "title 1");

    let d = ok("erDiagram\n  subgraph \"Customer Domain\"\n    CUSTOMER\n  end");
    assert_eq!(d.subgraphs[0].id, "Customer Domain");
    assert_eq!(d.subgraphs[0].title, "Customer Domain");
}

#[test]
fn a_relationship_may_name_a_subgraph() {
    let d = ok("erDiagram\n    subgraph title1\n        A1 ||--|| A2 : links\n    end\n    subgraph title2\n        B1 ||--|| B2 : links\n    end\n    title1 ||--|| title2 : links\n    title2 ||--|| B1 : links");
    assert_eq!(d.entity_ids(), ["A1", "A2", "B1", "B2"]);
    assert_eq!(d.relationships[2].from, "title1");
    assert_eq!(d.relationships[2].to, "title2");
    assert_eq!(d.relationships[3].to, "B1");
}

#[test]
fn a_direction_inside_a_subgraph_does_not_turn_the_diagram() {
    let d = ok(
        "erDiagram\n  direction TB\n  subgraph TOP\n    direction LR\n    A ||--|| B : x\n  end",
    );
    assert_eq!(d.direction, Direction::TopToBottom);
}

// ---------------------------------------------------------------------------------------------
// 5. Parsed and deliberately not drawn (§2-3)
// ---------------------------------------------------------------------------------------------

#[test]
fn nothing_that_is_not_an_entity_becomes_one() {
    let d = ok("erDiagram\n\
                REAL\n\
                classDef someclass fill:#f96\n\
                classDef default fill:#f9f,stroke-width:4px\n\
                class REAL someclass\n\
                style REAL fill:#f9f,stroke:#333,stroke-width:4px\n\
                accTitle: an accessible title\n\
                accDescr: an accessible description\n\
                %% a whole-line comment\n\
                direction LR\n");
    assert_eq!(d.entity_ids(), ["REAL"]);
    assert_eq!(d.acc_title.as_deref(), Some("an accessible title"));
    assert_eq!(d.acc_descr.as_deref(), Some("an accessible description"));
    assert_eq!(d.direction, Direction::LeftToRight);
    assert!(d.relationships.is_empty());
}

#[test]
fn acc_descr_in_braces_is_swallowed_whole() {
    let d = ok("erDiagram\n  accDescr {\n    a description\n    over two lines\n  }\n  A");
    assert_eq!(
        d.acc_descr.as_deref(),
        Some("a description\nover two lines")
    );
    assert_eq!(d.entity_ids(), ["A"]);
}

/// A `layout: elk` in front matter changes nothing. §0-4 settles that ELK is not implemented and
/// that not falling over is the whole requirement — GitHub itself draws such a diagram with dagre.
#[test]
fn an_elk_layout_directive_changes_nothing() {
    let plain = ok("erDiagram\n  CUSTOMER ||--o{ ORDER : places");
    let elk = ok("---\ntitle: Order example\nconfig:\n    layout: elk\n---\nerDiagram\n  CUSTOMER ||--o{ ORDER : places");
    assert_eq!(elk.entity_ids(), plain.entity_ids());
    assert_eq!(elk.relationships.len(), plain.relationships.len());
    assert_eq!(elk.direction, plain.direction);
}

/// A prose line is not an entity: `UNICODE_TEXT` has no spaces in it. This is §2-3 in its most
/// literal form — a fence that happens to contain a sentence must not grow a box per sentence.
#[test]
fn a_line_with_spaces_in_it_is_not_a_bare_entity() {
    let d = ok("erDiagram\n  REAL\n  this is a sentence, not an entity\n");
    assert_eq!(d.entity_ids(), ["REAL"]);
}

// ---------------------------------------------------------------------------------------------
// 6. Refusing rather than guessing
// ---------------------------------------------------------------------------------------------

#[test]
fn errors_say_what_was_wrong() {
    assert_eq!(
        parse("erDiagram\n  A {\n    string x")
            .unwrap_err()
            .to_string(),
        "unclosed `A {` opened at line 2"
    );
    assert_eq!(
        parse("erDiagram\n  subgraph S\n    A")
            .unwrap_err()
            .to_string(),
        "unclosed `subgraph S` opened at line 2"
    );
    assert_eq!(
        parse("erDiagram\n  A {\n    string x \"unclosed\n  }")
            .unwrap_err()
            .to_string(),
        "unclosed `\"` at line 3"
    );
    assert_eq!(
        parse("erDiagram").unwrap_err().to_string(),
        "ER diagram declares no entities"
    );
}

/// The relationship reader on its own, over statements that are not relationships. It is called
/// for every statement, so a false positive here becomes an edge between two things that were
/// never related.
#[test]
fn the_relationship_reader_says_no_when_there_is_no_relationship() {
    for src in [
        "CUSTOMER",
        "LINE-ITEM",
        "A ||-- : x",
        "-- B : x",
        "||--|| : x",
        "\"quoted name\"",
    ] {
        assert!(
            read_relationship(src).is_none(),
            "{src:?} is not a relationship"
        );
    }
    assert!(read_relationship("A ||--|| B : x").is_some());
}

/// Sources chosen to be awkward rather than realistic. Each has to come back with a model or an
/// error, never a panic — the renderer runs inside `catch_unwind`, which is a net and not a
/// licence (§1).
#[test]
fn awkward_sources_produce_a_model_or_an_error_and_never_a_panic() {
    let mut deep = String::from("erDiagram\n");
    for i in 0..40 {
        deep.push_str(&format!("subgraph s{i}\n"));
    }
    deep.push_str("A\n");
    for _ in 0..40 {
        deep.push_str("end\n");
    }
    let cases: &[&str] = &[
        "erDiagram\n  {",
        "erDiagram\n  }",
        "erDiagram\n  }}}}",
        "erDiagram\n  A {",
        "erDiagram\n  A }",
        "erDiagram\n  A { }",
        "erDiagram\n  A {} B {}",
        "erDiagram\n  ||--||",
        "erDiagram\n  A ||--|| : x",
        "erDiagram\n  A ||--||",
        "erDiagram\n  : x",
        "erDiagram\n  \"",
        "erDiagram\n  \"\"",
        "erDiagram\n  A[\"",
        "erDiagram\n  subgraph",
        "erDiagram\n  subgraph\n  end",
        "erDiagram\n  end",
        "erDiagram\n  end\n  end\n  end",
        "erDiagram\n  一 ||--|| 二 : 🎉",
        "erDiagram\n  ❤ {\n    string 名前 PK \"日本語のコメント\"\n  }",
        "erDiagram\n  A u--u B : x",
        "erDiagram\n  A 1 to 1 B : x",
        "erDiagram\n  to to to : x",
        "erDiagram\n  many many many : x",
        "erDiagram\n  A ||--|| A : self",
        "erDiagram\n  A {\n    string x PK, FK, UK, ZZ\n  }",
        "erDiagram\n  direction",
        "erDiagram\n  direction NOPE\n  A",
        &deep,
    ];
    for src in cases {
        let _ = parse(src);
    }
}
