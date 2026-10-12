//! What the class-diagram parser is checked with.
//!
//! The corpus is built from **mermaid's own documentation** — every `classDiagram` fence in
//! `docs/syntax/classDiagram.md`, in the form it is written there — plus the cases the *grammar*
//! distinguishes that the documentation happens not to show. That order matters: memory
//! `corpus-from-spec-not-from-bugs` records that a corpus grown from past bugs only ever prevents
//! the bugs already found.
//!
//! Two kinds of assertion appear here and they are not interchangeable:
//!
//! * what the parser **must** produce, which is what the renderer draws;
//! * what it **must drop**, which is §2-3's rule. A statement konoma fails to recognise does not
//!   disappear — it becomes a phantom box. So `classDef`, `click`, `style` and their relatives are
//!   asserted to leave *no* class behind, rather than merely not crashing.

use super::model::{Line, Marker, MemberKind};
use super::parser::{parse_generic_types, parse_member, MAX_RELATIONS_FOR_TESTS};
use super::*;

/// Parses, or panics with the reason.
fn ok(src: &str) -> ClassDiagram {
    parse(src).unwrap_or_else(|e| panic!("should parse: {e}\n{src}"))
}

/// The text each of a class's rows is drawn with.
fn rows(d: &ClassDiagram, id: &str) -> (Vec<String>, Vec<String>) {
    let c = d.class(id).unwrap_or_else(|| panic!("no class {id}"));
    (
        c.attributes.iter().map(|m| m.display()).collect(),
        c.methods.iter().map(|m| m.display()).collect(),
    )
}

// ---------------------------------------------------------------------------------------------
// 1. The header decides, and only the header
// ---------------------------------------------------------------------------------------------

#[test]
fn the_header_decides_and_only_the_header() {
    assert!(parse("classDiagram\n  A : +x").is_ok());
    assert!(parse("classDiagram-v2\n  A : +x").is_ok());
    assert!(parse("  \n\n  classDiagram\n  A : +x").is_ok());
    assert!(parse("%% a comment\nclassDiagram\n  A : +x").is_ok());
    assert!(parse("---\ntitle: t\n---\nclassDiagram\n  A : +x").is_ok());
    for (src, header) in [
        ("flowchart TD\n  A --> B", "flowchart"),
        ("stateDiagram-v2\n  [*] --> A", "stateDiagram-v2"),
        ("erDiagram\n  A ||--|| B : x", "erDiagram"),
        ("sequenceDiagram\n  A->>B: hi", "sequenceDiagram"),
        // `classDiagrams` is a word, not a header — the same guard the other parsers have.
        ("classDiagrams --> B", "classDiagrams"),
    ] {
        assert_eq!(
            parse(src).unwrap_err(),
            ParseError::NotAClassDiagram {
                header: header.to_string()
            },
            "{src:?}"
        );
    }
    assert_eq!(parse("").unwrap_err(), ParseError::Empty);
    assert_eq!(parse("   \n\n  ").unwrap_err(), ParseError::Empty);
    assert_eq!(parse("classDiagram").unwrap_err(), ParseError::NoClasses);
}

#[test]
fn front_matter_is_lifted_out_and_line_numbers_survive_it() {
    let d = ok("---\ntitle: Animal example\n---\nclassDiagram\n  A : +x");
    assert_eq!(d.title.as_deref(), Some("Animal example"));
    // The `class B {` is on line 6 of the file the user is looking at, and the error has to say
    // so — the whole reason konoma is writing these parsers (§8).
    let e = parse("---\ntitle: t\n---\nclassDiagram\n  A : +x\n  class B {\n    +y").unwrap_err();
    assert_eq!(e.to_string(), "unclosed `class B {` opened at line 6");
}

// ---------------------------------------------------------------------------------------------
// 2. Declaring a class
// ---------------------------------------------------------------------------------------------

#[test]
fn a_class_can_be_declared_in_every_documented_form() {
    // Bare.
    assert_eq!(ok("classDiagram\n  class Animal").class_ids(), ["Animal"]);
    // With a label.
    let d = ok("classDiagram\n  class Animal[\"Animal with a label\"]\n  class Car[\"Car with *! symbols\"]\n  Animal --> Car");
    assert_eq!(d.class("Animal").unwrap().label, "Animal with a label");
    assert_eq!(d.class("Car").unwrap().label, "Car with *! symbols");
    // Backticked, which is the only way to get a space into a class name.
    let d = ok("classDiagram\n  class `Animal Class!`\n  class `Car Class`\n  `Animal Class!` --> `Car Class`");
    assert_eq!(d.class_ids(), ["Animal Class!", "Car Class"]);
    assert_eq!(d.relations.len(), 1);
    assert_eq!(d.relations[0].from, "Animal Class!");
    // With a css class.
    let d = ok("classDiagram\n  class Animal:::someclass\n  classDef someclass fill:#f96");
    assert_eq!(d.class("Animal").unwrap().css_classes, ["someclass"]);
    // With a body on the same line as the brace.
    let d = ok("classDiagram\n  class A { +x\n    +y() }");
    assert_eq!(rows(&d, "A"), (vec!["+x".into()], vec!["+y()".into()]));
}

#[test]
fn a_generic_is_split_off_the_name_and_kept_in_the_label() {
    let d = ok("classDiagram\n  class Square~Shape~{\n    int id\n  }\n  Square --> Foo");
    // `splitClassNameAndType`: the class is `Square`, so the relationship reaches the same box.
    assert_eq!(d.class_ids(), ["Square", "Foo"]);
    let square = d.class("Square").unwrap();
    assert_eq!(square.generic, "Shape");
    assert_eq!(square.label, "Square<Shape>");
    assert_eq!(d.relations[0].from, "Square");
}

#[test]
fn an_annotation_is_recognised_in_all_three_documented_places() {
    // Inline.
    let d = ok("classDiagram\n  class Shape <<interface>>");
    assert_eq!(d.class("Shape").unwrap().annotations, ["interface"]);
    // On its own line, after the class.
    let d = ok("classDiagram\nclass Shape\n<<interface>> Shape\nShape : draw()");
    assert_eq!(d.class("Shape").unwrap().annotations, ["interface"]);
    assert_eq!(rows(&d, "Shape").1, ["draw()"]);
    // Inside the body.
    let d = ok("classDiagram\nclass Color{\n    <<enumeration>>\n    RED\n    BLUE\n}");
    assert_eq!(d.class("Color").unwrap().annotations, ["enumeration"]);
    assert_eq!(rows(&d, "Color").0, ["RED", "BLUE"]);
    // Inline *and* a body.
    let d = ok("classDiagram\n  class Shape <<service>> {\n    +run()\n  }");
    assert_eq!(d.class("Shape").unwrap().annotations, ["service"]);
    assert_eq!(rows(&d, "Shape").1, ["+run()"]);
}

#[test]
fn a_label_may_contain_the_characters_an_annotation_is_made_of() {
    // The `<<` scan runs on the name only. Running it on the whole statement would take a bite
    // out of the label instead.
    let d = ok("classDiagram\n  class A[\"a << b >> c\"]");
    assert_eq!(d.class("A").unwrap().label, "a << b >> c");
    assert!(d.class("A").unwrap().annotations.is_empty());
}

// ---------------------------------------------------------------------------------------------
// 3. Members
// ---------------------------------------------------------------------------------------------

#[test]
fn a_member_reaches_the_same_class_by_either_route() {
    let one_line = ok("classDiagram\nclass BankAccount\nBankAccount : +String owner\nBankAccount : +deposit(amount)");
    let body = ok("classDiagram\nclass BankAccount{\n    +String owner\n    +deposit(amount)\n}");
    assert_eq!(rows(&one_line, "BankAccount"), rows(&body, "BankAccount"));
    assert_eq!(
        rows(&body, "BankAccount"),
        (
            vec!["+String owner".into()],
            vec!["+deposit(amount)".into()]
        )
    );
}

#[test]
fn every_visibility_marker_is_read_and_drawn() {
    let d = ok("classDiagram\nclass A{\n  +pub\n  -priv\n  #prot\n  ~pkg\n  none\n}");
    let a = d.class("A").unwrap();
    let seen: Vec<Option<char>> = a.attributes.iter().map(|m| m.visibility).collect();
    assert_eq!(
        seen,
        [Some('+'), Some('-'), Some('#'), Some('~'), None],
        "the four UML visibilities, plus a member that declares none"
    );
    assert_eq!(
        rows(&d, "A").0,
        ["+pub", "-priv", "#prot", "~pkg", "none"],
        "and the marker is part of what is drawn — dropping it loses the meaning"
    );
}

#[test]
fn a_method_is_taken_apart_and_rebuilt() {
    let d = ok("classDiagram\nclass A{\n  +deposit(amount) bool\n  +withdrawal(amount) int\n  draw()\n  getPoints() List~int~\n}");
    assert_eq!(
        rows(&d, "A").1,
        [
            "+deposit(amount) : bool",
            "+withdrawal(amount) : int",
            "draw()",
            "getPoints() : List<int>"
        ],
        "mermaid puts a ` : ` before the return type; printing the line as written does not"
    );
    let m = &d.class("A").unwrap().methods[0];
    assert_eq!(m.kind, MemberKind::Method);
    assert_eq!(m.name, "deposit");
    assert_eq!(m.parameters, "amount");
    assert_eq!(m.return_type, "bool");
}

/// `indexOf(')') > 0` is upstream's test, and the `> 0` is load-bearing: a member whose *first*
/// character is a `)` is not a method.
#[test]
fn what_counts_as_a_method_is_the_bracket_and_where_it_is() {
    let d = ok("classDiagram\nclass A{\n  int amount\n  draw()\n  )odd\n}");
    assert_eq!(rows(&d, "A").0, ["int amount", ")odd"]);
    assert_eq!(rows(&d, "A").1, ["draw()"]);
}

#[test]
fn both_classifiers_are_read_in_both_documented_positions() {
    let d = ok("classDiagram\nclass A{\n  +run()*\n  +run2() int*\n  +go()$\n  +go2() String$\n  String field$\n}");
    let a = d.class("A").unwrap();
    let method_classifiers: Vec<Option<char>> = a.methods.iter().map(|m| m.classifier).collect();
    assert_eq!(
        method_classifiers,
        [Some('*'), Some('*'), Some('$'), Some('$')],
        "`someAbstractMethod()*` and `someAbstractMethod() int*` are the two documented spellings"
    );
    assert_eq!(a.attributes[0].classifier, Some('$'));
    assert_eq!(a.attributes[0].name, "String field");
    // konoma keeps the character rather than italicising: see `Member::display`.
    assert_eq!(
        rows(&d, "A").1,
        ["+run()*", "+run2() : int*", "+go()$", "+go2() : String$"]
    );
    assert_eq!(rows(&d, "A").0, ["String field$"]);
}

/// The greedy-regex shape of upstream's method rule, which decides where the name ends when the
/// parameter list has brackets of its own.
#[test]
fn a_parameter_list_with_brackets_splits_where_upstream_splits_it() {
    let m = parse_member("f(g(x))", true);
    assert_eq!(m.name, "f(g");
    assert_eq!(m.parameters, "x)");
    let m = parse_member("setPoints(List~int~ points)", true);
    assert_eq!(m.name, "setPoints");
    assert_eq!(m.parameters, "List<int> points");
}

#[test]
fn generic_types_pair_from_the_outside_in() {
    // The nested case is the whole point: pairing tildes left-to-right gives `List<List>int>>`.
    assert_eq!(parse_generic_types("List~List~int~~"), "List<List<int>>");
    assert_eq!(parse_generic_types("List~int~"), "List<int>");
    assert_eq!(parse_generic_types("plain"), "plain");
    // An odd number of tildes with a leading one is the package-visibility member `~field`,
    // which upstream keeps as written.
    assert_eq!(parse_generic_types("~K~V~"), "~K<V>");
    // A comma between two halves that each hold one tilde is one set, not two.
    assert_eq!(parse_generic_types("Map~K, V~"), "Map<K, V>");
}

// ---------------------------------------------------------------------------------------------
// 4. Relationships
// ---------------------------------------------------------------------------------------------

/// The eight documented relationship types, in both directions — the table in `classDiagram.md`
/// twice over. The mark is per end, so `<|--` and `--|>` are the same mark on opposite ends.
#[test]
fn every_documented_relationship_reads_as_the_grammar_says() {
    let d = ok(
        "classDiagram\nclassA <|-- classB\nclassC *-- classD\nclassE o-- classF\n\
                classG <-- classH\nclassI -- classJ\nclassK <.. classL\nclassM <|.. classN\n\
                classO .. classP",
    );
    let seen: Vec<(Marker, Marker, Line)> = d
        .relations
        .iter()
        .map(|r| (r.start, r.end, r.line))
        .collect();
    assert_eq!(
        seen,
        [
            (Marker::Extension, Marker::None, Line::Solid),
            (Marker::Composition, Marker::None, Line::Solid),
            (Marker::Aggregation, Marker::None, Line::Solid),
            (Marker::Dependency, Marker::None, Line::Solid),
            (Marker::None, Marker::None, Line::Solid),
            (Marker::Dependency, Marker::None, Line::Dotted),
            (Marker::Extension, Marker::None, Line::Dotted),
            (Marker::None, Marker::None, Line::Dotted),
        ]
    );

    let d = ok(
        "classDiagram\nclassA --|> classB\nclassC --* classD\nclassE --o classF\n\
                classG --> classH\nclassK ..> classL\nclassM ..|> classN",
    );
    let seen: Vec<(Marker, Marker, Line)> = d
        .relations
        .iter()
        .map(|r| (r.start, r.end, r.line))
        .collect();
    assert_eq!(
        seen,
        [
            (Marker::None, Marker::Extension, Line::Solid),
            (Marker::None, Marker::Composition, Line::Solid),
            (Marker::None, Marker::Aggregation, Line::Solid),
            (Marker::None, Marker::Dependency, Line::Solid),
            (Marker::None, Marker::Dependency, Line::Dotted),
            (Marker::None, Marker::Extension, Line::Dotted),
        ]
    );
}

#[test]
fn a_relationship_can_carry_a_mark_at_both_ends() {
    let d = ok("classDiagram\n  Animal <|--|> Zebra\n  A *--* B\n  C o--o D");
    let seen: Vec<(Marker, Marker)> = d.relations.iter().map(|r| (r.start, r.end)).collect();
    assert_eq!(
        seen,
        [
            (Marker::Extension, Marker::Extension),
            (Marker::Composition, Marker::Composition),
            (Marker::Aggregation, Marker::Aggregation),
        ]
    );
}

#[test]
fn a_lollipop_is_read_at_either_end() {
    let d = ok("classDiagram\n  bar ()-- foo\n  Class01 --() baz");
    assert_eq!(d.relations[0].start, Marker::Lollipop);
    assert_eq!(d.relations[0].end, Marker::None);
    assert_eq!(d.relations[1].start, Marker::None);
    assert_eq!(d.relations[1].end, Marker::Lollipop);
    // …and neither end grew an invisible interface node: konoma draws the ring on the line.
    assert_eq!(d.class_ids(), ["bar", "foo", "Class01", "baz"]);
}

#[test]
fn a_cardinality_is_read_at_either_end_and_a_label_after_the_colon() {
    let d = ok("classDiagram\n    Customer \"1\" --> \"*\" Ticket\n    \
                Student \"1\" --> \"1..*\" Course\n    Galaxy --> \"many\" Star : Contains");
    let seen: Vec<(Option<&str>, Option<&str>, Option<&str>)> = d
        .relations
        .iter()
        .map(|r| {
            (
                r.start_cardinality.as_deref(),
                r.end_cardinality.as_deref(),
                r.label.as_deref(),
            )
        })
        .collect();
    assert_eq!(
        seen,
        [
            (Some("1"), Some("*"), None),
            (Some("1"), Some("1..*"), None),
            (None, Some("many"), Some("Contains")),
        ]
    );
    assert_eq!(
        d.class_ids(),
        ["Customer", "Ticket", "Student", "Course", "Galaxy", "Star"]
    );
}

#[test]
fn a_relationship_label_may_hold_the_characters_a_relationship_is_made_of() {
    // `Link(Solid)` has brackets, `50%% done` has a comment marker, and neither is structural
    // once the `:` has been passed: `LABEL` runs to the end of the line.
    let d = ok(
        "classDiagram\n  classI -- classJ : Link(Solid)\n  A --> B : 50%% done\n  C --> D : a--b",
    );
    let labels: Vec<Option<&str>> = d.relations.iter().map(|r| r.label.as_deref()).collect();
    assert_eq!(
        labels,
        [Some("Link(Solid)"), Some("50%% done"), Some("a--b")]
    );
    assert_eq!(d.class_ids(), ["classI", "classJ", "A", "B", "C", "D"]);
}

/// **A recorded ambiguity, not a bug konoma introduced.** `\s*o` is its own token upstream, so
/// `A --o orange` is an aggregation reaching a class called `range` there and a class called
/// `orange` here — neither reading is derivable from the text.
///
/// What konoma does fix is the case where the `o` is followed by another word character:
/// `A --oops` reaches `oops`, where mermaid reaches `ops`.
#[test]
fn the_o_of_an_aggregation_is_also_a_letter() {
    let d = ok("classDiagram\n  A --o orange");
    assert_eq!(d.relations[0].end, Marker::Aggregation);
    assert_eq!(
        d.relations[0].to, "orange",
        "konoma keeps the whole word; mermaid's longest-match lexer takes the `o` and leaves `range`"
    );

    let d = ok("classDiagram\n  A --oops");
    assert_eq!(
        d.relations[0].end,
        Marker::None,
        "an `o` followed by a word character is part of the name"
    );
    assert_eq!(d.relations[0].to, "oops");

    // And the same rule at the start of the line: the `o` of `Zoo` is not an aggregation.
    let d = ok("classDiagram\n  Zoo -- Bar");
    assert_eq!(d.relations[0].start, Marker::None);
    assert_eq!(d.relations[0].from, "Zoo");
}

/// A run of three or more is one line token. mermaid reads `A --- B` as a line plus a `MINUS`
/// that joins the next name, which draws a class called `-B`.
#[test]
fn a_long_run_of_dashes_is_one_line() {
    let d = ok("classDiagram\n  A --- B\n  C ... D");
    assert_eq!(d.class_ids(), ["A", "B", "C", "D"]);
    assert_eq!(d.relations[0].line, Line::Solid);
    assert_eq!(d.relations[1].line, Line::Dotted);
}

#[test]
fn the_relation_ceiling_holds() {
    let mut src = String::from("classDiagram\n");
    for i in 0..(MAX_RELATIONS_FOR_TESTS + 50) {
        src.push_str(&format!("  n{i} --> m{i}\n"));
    }
    let d = ok(&src);
    assert_eq!(d.relations.len(), MAX_RELATIONS_FOR_TESTS);
}

// ---------------------------------------------------------------------------------------------
// 5. Namespaces
// ---------------------------------------------------------------------------------------------

#[test]
fn a_namespace_holds_the_classes_written_inside_it() {
    let d = ok("classDiagram\nnamespace BaseShapes {\n    class Triangle\n    class Rectangle {\n      double width\n    }\n}");
    assert_eq!(d.namespaces.len(), 1);
    assert_eq!(d.namespaces[0].id, "BaseShapes");
    assert_eq!(d.namespaces[0].members, ["Triangle", "Rectangle"]);
    assert_eq!(
        d.class("Triangle").unwrap().parent.as_deref(),
        Some("BaseShapes")
    );
    assert_eq!(rows(&d, "Rectangle").0, ["double width"]);
}

#[test]
fn a_namespace_can_carry_a_label() {
    let d = ok("classDiagram\n  namespace Auth[\"Authentication Service\"] {\n    class UserService {\n      +login()\n    }\n  }");
    assert_eq!(d.namespaces[0].id, "Auth");
    assert_eq!(d.namespaces[0].label, "Authentication Service");
}

/// A dotted name creates the whole chain, and a namespace written inside another is qualified by
/// it. Both are `addNamespace`'s doing, and both change how many frames come out.
#[test]
fn a_dotted_namespace_becomes_a_chain_of_frames() {
    let d = ok("classDiagram\n    namespace Company.Engineering.Backend {\n        class Developer\n    }\n    namespace Company.Engineering {\n        class TechLead\n    }\n    TechLead --> Developer : leads");
    let ids: Vec<&str> = d.namespaces.iter().map(|n| n.id.as_str()).collect();
    assert_eq!(
        ids,
        [
            "Company",
            "Company.Engineering",
            "Company.Engineering.Backend"
        ]
    );
    assert_eq!(d.namespaces[0].label, "Company");
    assert_eq!(d.namespaces[1].label, "Engineering");
    assert_eq!(d.namespaces[2].label, "Backend");
    assert_eq!(d.namespaces[1].parent.as_deref(), Some("Company"));
    assert!(d.namespaces[1]
        .members
        .contains(&"Company.Engineering.Backend".to_string()));
    assert!(d.namespaces[1].members.contains(&"TechLead".to_string()));
    assert_eq!(
        d.class("Developer").unwrap().parent.as_deref(),
        Some("Company.Engineering.Backend")
    );
}

#[test]
fn a_namespace_written_inside_another_is_qualified_by_it() {
    let d = ok("classDiagram\n    namespace Platform {\n        namespace Auth {\n            class UserService\n        }\n        class Gateway\n    }");
    let ids: Vec<&str> = d.namespaces.iter().map(|n| n.id.as_str()).collect();
    assert_eq!(ids, ["Platform", "Platform.Auth"]);
    assert_eq!(
        d.class("UserService").unwrap().parent.as_deref(),
        Some("Platform.Auth")
    );
    assert_eq!(
        d.class("Gateway").unwrap().parent.as_deref(),
        Some("Platform")
    );
}

/// A `namespace X` with no `{` draws nothing — the grammar has no such production, so registering
/// a frame for it would put an empty box on the page.
#[test]
fn a_namespace_with_no_body_is_not_a_frame() {
    let d = ok("classDiagram\n  namespace Empty\n  class A");
    assert!(d.namespaces.is_empty());
    assert_eq!(d.class_ids(), ["A"]);
}

// ---------------------------------------------------------------------------------------------
// 6. Notes
// ---------------------------------------------------------------------------------------------

#[test]
fn both_note_forms_are_read_and_a_target_binds_in_either_order() {
    // The documented example writes `note for MyClass` *above* `class MyClass`, so a parser that
    // resolved the target when it read the note would drop the tie.
    let d = ok("classDiagram\n    note \"This is a general note\"\n    note for MyClass \"This is a note for a class\"\n    class MyClass{\n    }");
    assert_eq!(d.notes.len(), 2);
    assert_eq!(d.notes[0].text, "This is a general note");
    assert_eq!(d.notes[0].target, None);
    assert_eq!(d.notes[1].target.as_deref(), Some("MyClass"));

    // …and the other way round.
    let d = ok("classDiagram\n  class MyClass\n  note for MyClass \"after\"");
    assert_eq!(d.notes[0].target.as_deref(), Some("MyClass"));
}

#[test]
fn a_note_for_a_class_that_never_appears_still_draws_but_ties_to_nothing() {
    let d = ok("classDiagram\n  class A\n  note for Nobody \"orphan\"");
    assert_eq!(d.notes.len(), 1);
    assert_eq!(
        d.notes[0].target, None,
        "no tie-line to a class that is not there"
    );
    assert_eq!(d.class_ids(), ["A"], "and no class was invented for it");
}

#[test]
fn a_note_carries_line_breaks_the_way_every_other_label_does() {
    let d = ok("classDiagram\n  class Duck\n  note for Duck \"can fly<br>can swim\"");
    assert_eq!(d.notes[0].text, "can fly\ncan swim");
}

// ---------------------------------------------------------------------------------------------
// 7. Parsed and deliberately not drawn (§2-3)
// ---------------------------------------------------------------------------------------------

/// **The rule that stops phantom boxes.** Every one of these is a statement mermaid has and
/// konoma draws nothing for; the assertion is that no class is left behind, not that nothing
/// crashed.
#[test]
fn nothing_that_is_not_a_class_becomes_one() {
    let d = ok("classDiagram\n\
                class Real\n\
                classDef someclass fill:#f96\n\
                classDef default fill:#f96,color:red\n\
                cssClass \"Real\" someclass\n\
                style Real fill:#f9f,stroke:#333,stroke-width:4px\n\
                click Real href \"https://example.com\" \"tip\"\n\
                click Real call callbackFunction() \"tip\"\n\
                callback Real \"callbackFunction\" \"tip\"\n\
                link Real \"https://example.com\" \"tip\"\n\
                accTitle: an accessible title\n\
                accDescr: an accessible description\n\
                %% a whole-line comment\n\
                direction LR\n\
                Stray\n\
                namespace Empty\n");
    assert_eq!(
        d.class_ids(),
        ["Real"],
        "only the `class Real` line declares anything"
    );
    assert_eq!(d.acc_title.as_deref(), Some("an accessible title"));
    assert_eq!(d.acc_descr.as_deref(), Some("an accessible description"));
    assert_eq!(d.direction, Direction::LeftToRight);
    assert!(d.relations.is_empty());
    assert!(d.notes.is_empty());
}

/// A bare identifier is `memberStatement : className`, whose action upstream is empty. This is
/// the case that stops a fenced block of prose from becoming a row of boxes.
#[test]
fn a_bare_word_declares_nothing() {
    assert_eq!(
        parse("classDiagram\n  Animal").unwrap_err(),
        ParseError::NoClasses
    );
    assert_eq!(
        ok("classDiagram\n  class Real\n  Animal\n  Vehicle").class_ids(),
        ["Real"]
    );
}

#[test]
fn acc_descr_in_braces_is_swallowed_whole() {
    let d = ok("classDiagram\n  accDescr {\n    a description\n    over two lines\n  }\n  class A");
    assert_eq!(
        d.acc_descr.as_deref(),
        Some("a description\nover two lines")
    );
    assert_eq!(d.class_ids(), ["A"]);
}

/// A trailing `%%` is a comment **only before the `:` that starts a `LABEL`**. The token is
/// `":"{1}[^:\n;]+`, which excludes `:` and `;` and nothing else, so everything after the colon —
/// percent signs included — is text. Measured against the grammar rather than assumed: the first
/// version of this test expected the comment to be stripped from the member too.
#[test]
fn a_comment_ends_a_statement_but_not_a_label() {
    let d =
        ok("classDiagram\n  class A %% this is a comment\n  A : +x %% and this\n  B : 50%% done");
    assert_eq!(d.class_ids(), ["A", "B"]);
    assert_eq!(rows(&d, "A").0, ["+x %% and this"]);
    assert_eq!(rows(&d, "B").0, ["50%% done"]);
}

/// A `direction` is only a statement when the statement starts with it. Upstream's
/// `.*direction\s+TB[^\n]*` would swallow the member and turn the diagram sideways.
#[test]
fn direction_is_a_statement_only_at_the_start_of_one() {
    let d = ok("classDiagram\n  A : go direction TB\n  direction LR");
    assert_eq!(d.direction, Direction::LeftToRight);
    assert_eq!(rows(&d, "A").0, ["go direction TB"]);
}

/// A `direction` inside a namespace is read and ignored — the decision `render::lay_out_spec`
/// spells out, applied to the third language that has the statement.
#[test]
fn a_direction_inside_a_namespace_does_not_turn_the_diagram() {
    let d = ok("classDiagram\n  direction TB\n  namespace N {\n    direction LR\n    class A\n  }");
    assert_eq!(d.direction, Direction::TopToBottom);
}

// ---------------------------------------------------------------------------------------------
// 8. Refusing rather than guessing
// ---------------------------------------------------------------------------------------------

#[test]
fn errors_say_what_was_wrong() {
    assert_eq!(
        parse("classDiagram\n  class A {\n    +x")
            .unwrap_err()
            .to_string(),
        "unclosed `class A {` opened at line 2"
    );
    assert_eq!(
        parse("classDiagram\n  namespace N {\n    class A")
            .unwrap_err()
            .to_string(),
        "unclosed `namespace N {` opened at line 2"
    );
    assert_eq!(
        parse("classDiagram\n  class A[\"unclosed")
            .unwrap_err()
            .to_string(),
        "unclosed `\"` at line 2"
    );
    assert_eq!(
        parse("classDiagram").unwrap_err().to_string(),
        "class diagram declares no classes"
    );
}

/// Sources chosen to be awkward rather than realistic. The parser runs on a worker thread wrapped
/// in `catch_unwind` (§1) — a safety net, not a licence — so each of these has to come back with a
/// model or an error, never a panic.
#[test]
fn awkward_sources_produce_a_model_or_an_error_and_never_a_panic() {
    let mut deep = String::from("classDiagram\n");
    for i in 0..40 {
        deep.push_str(&format!("namespace n{i} {{\n"));
    }
    deep.push_str("class A\n");
    for _ in 0..40 {
        deep.push_str("}\n");
    }
    let cases: &[&str] = &[
        "classDiagram\n  class",
        "classDiagram\n  class ",
        "classDiagram\n  class ~~~",
        "classDiagram\n  class A~",
        "classDiagram\n  class ``",
        "classDiagram\n  class A[]",
        "classDiagram\n  class A[\"\"]",
        "classDiagram\n  --",
        "classDiagram\n  -->",
        "classDiagram\n  A -->",
        "classDiagram\n  --> B",
        "classDiagram\n  A --> A",
        "classDiagram\n  A <|--|> A",
        "classDiagram\n  }",
        "classDiagram\n  }}}}",
        "classDiagram\n  { }",
        "classDiagram\n  class A { }",
        "classDiagram\n  class A {}",
        "classDiagram\n  A : ",
        "classDiagram\n  A : ()",
        "classDiagram\n  A : )(",
        "classDiagram\n  A : +f(",
        "classDiagram\n  A : +f)",
        "classDiagram\n  A : ~",
        "classDiagram\n  A : $",
        "classDiagram\n  A : *",
        "classDiagram\n  <<>>",
        "classDiagram\n  <<x>>",
        "classDiagram\n  note",
        "classDiagram\n  note \"",
        "classDiagram\n  note for",
        "classDiagram\n  note for X",
        "classDiagram\n  一 --> 二 : 🎉",
        "classDiagram\n  class 全画面プレビュー {\n    +種別を解決()\n  }",
        "classDiagram\n  namespace N { }",
        "classDiagram\n  A : #35;hash#quot;quote",
        &deep,
    ];
    for src in cases {
        let _ = parse(src);
    }
}
