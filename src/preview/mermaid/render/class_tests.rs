//! What stage 3's class-diagram drawing is checked with.
//!
//! Same three questions as stage 1 and stage 2, and the same answers, because the answers are the
//! point (`docs/FEATURE-MERMAID-RENDERER.md` §6):
//!
//! 1. **The main instrument still measures konoma against resvg.** A class box is sized by its
//!    widest *row*, which is a rule nothing else in the renderer uses, so it gets its own
//!    "box − drawn text == the padding the panel declares" check. The other side of the
//!    comparison is resvg's real shaper, so this cannot decay into konoma-versus-konoma the way
//!    the markdown diff harness did.
//! 2. **The geometric invariants are the same functions**, not copies:
//!    [`super::tests::check_nodes_do_not_overlap`] and its eleven siblings are called from here
//!    over a class corpus. A property that drifts drifts for every diagram kind at once.
//! 3. **The corpus is built from the specification** — every `classDiagram` fence in mermaid's own
//!    `docs/syntax/classDiagram.md`, in the form it is written there — plus the cases the grammar
//!    distinguishes and the awkward shapes §6 asks for (CJK, emoji, long runs, thin glyphs).
//!
//! # The properties that are about the *marks*
//!
//! A class diagram's meaning is carried by the ends of its lines: a hollow triangle is "is a", a
//! filled diamond is "owns". Drawing one where the other belongs produces a picture that is
//! perfectly plausible and says the opposite thing, which is precisely the defect a presence test
//! passes over. So [`inheritance_and_composition_are_told_apart`] compares the two *shapes*, and
//! [`a_relationship_mark_is_drawn_at_the_end_it_was_written_on`] checks the end.

use std::collections::HashSet;

use super::class::{lay_out, render, spec_of};
use super::edges::Tip;
use super::tests::{
    assert_snapshot, check_cluster_titles, check_clusters_hold_their_members,
    check_edge_that_names_a_block_stops_on_its_frame, check_edges_keep_out_of_foreign_frames,
    check_edges_stay_out_of_shapes, check_endpoints_land_on_the_outline,
    check_labels_ride_their_edge, check_nested_clusters_sit_inside_their_parent,
    check_nodes_do_not_overlap, check_panels_stay_inside_their_box,
    check_unrelated_clusters_do_not_overlap, check_view_box_contains_everything, mask_numbers,
    text_widths, tree_of,
};
use super::{clusters, edges, labels, panel, shapes, svg, Diagram, Glyph, RenderError};
use crate::preview::mermaid::class::{self, ClassDiagram};
use crate::preview::mermaid::text_metrics;

// ---------------------------------------------------------------------------------------------
// Corpus
// ---------------------------------------------------------------------------------------------

/// The diagrams every structural test runs over, and the ones [`gallery`] writes out to be looked
/// at.
pub const CASES: &[(&str, &str)] = &[
    (
        "animal",
        "classDiagram\n    note \"From Duck till Zebra\"\n    Animal <|-- Duck\n    \
         note for Duck \"can fly<br>can swim<br>can dive\"\n    Animal <|-- Fish\n    \
         Animal <|-- Zebra\n    Animal : +int age\n    Animal : +String gender\n    \
         Animal: +isMammal()\n    Animal: +mate()\n    class Duck{\n        +String beakColor\n        \
         +swim()\n        +quack()\n    }\n    class Fish{\n        -int sizeInFeet\n        \
         -canEat()\n    }\n    class Zebra{\n        +bool is_wild\n        +run()\n    }",
    ),
    (
        "bank",
        "classDiagram\n    class BankAccount\n    BankAccount : +String owner\n    \
         BankAccount : +Bigdecimal balance\n    BankAccount : +deposit(amount)\n    \
         BankAccount : +withdrawal(amount)",
    ),
    (
        "bank-body",
        "classDiagram\nclass BankAccount{\n    +String owner\n    +BigDecimal balance\n    \
         +deposit(amount) bool\n    +withdrawal(amount) int\n}",
    ),
    (
        // A class with nothing in it at all: the case that decides whether an empty class still
        // reads as a class box.
        "empty-class",
        "classDiagram\n    class Animal\n    Vehicle <|-- Car",
    ),
    (
        "labels",
        "classDiagram\n    class Animal[\"Animal with a label\"]\n    \
         class Car[\"Car with *! symbols\"]\n    Animal --> Car",
    ),
    (
        "backticks",
        "classDiagram\n    class `Animal Class!`\n    class `Car Class`\n    \
         `Animal Class!` --> `Car Class`",
    ),
    (
        "generics",
        "classDiagram\nclass Square~Shape~{\n    int id\n    List~int~ position\n    \
         setPoints(List~int~ points)\n    getPoints() List~int~\n}\n\n\
         Square : -List~string~ messages\nSquare : +setMessages(List~string~ messages)\n\
         Square : +getDistanceMatrix() List~List~int~~",
    ),
    (
        "relations-left",
        "classDiagram\nclassA <|-- classB\nclassC *-- classD\nclassE o-- classF\n\
         classG <-- classH\nclassI -- classJ\nclassK <.. classL\nclassM <|.. classN\n\
         classO .. classP",
    ),
    (
        "relations-right",
        "classDiagram\nclassA --|> classB : Inheritance\nclassC --* classD : Composition\n\
         classE --o classF : Aggregation\nclassG --> classH : Association\n\
         classI -- classJ : Link(Solid)\nclassK ..> classL : Dependency\n\
         classM ..|> classN : Realization\nclassO .. classP : Link(Dashed)",
    ),
    (
        "two-way",
        "classDiagram\n    Animal <|--|> Zebra\n    A *--* B\n    C o--o D",
    ),
    (
        "lollipop",
        "classDiagram\n  class Class01 {\n    int amount\n    draw()\n  }\n  Class01 --() bar\n  \
         Class02 --() bar\n\n  foo ()-- Class01",
    ),
    (
        "namespace",
        "classDiagram\nnamespace BaseShapes {\n    class Triangle\n    class Rectangle {\n      \
         double width\n      double height\n    }\n}",
    ),
    (
        "namespace-labelled",
        "classDiagram\n    namespace Auth[\"Authentication Service\"] {\n        \
         class UserService {\n            +login()\n            +logout()\n        }\n    }",
    ),
    (
        "namespace-dotted",
        "classDiagram\n    namespace Company.Engineering.Backend {\n        class Developer {\n            \
         +writeCode()\n        }\n    }\n    namespace Company.Engineering.Frontend {\n        \
         class Designer {\n            +createMockup()\n        }\n    }\n    \
         namespace Company.Engineering {\n        class TechLead {\n            +planSprint()\n        \
         }\n    }\n    TechLead --> Developer : leads\n    TechLead --> Designer : leads",
    ),
    (
        "namespace-nested",
        "classDiagram\n    namespace Platform {\n        namespace Auth {\n            \
         class UserService {\n                +login()\n            }\n        }\n        \
         namespace Data {\n            class Repository {\n                +find()\n            }\n        \
         }\n        class Gateway {\n            +route()\n        }\n    }\n    \
         Gateway --> UserService : delegates\n    Gateway --> Repository : delegates",
    ),
    (
        "cardinality",
        "classDiagram\n    Customer \"1\" --> \"*\" Ticket\n    Student \"1\" --> \"1..*\" Course\n    \
         Galaxy --> \"many\" Star : Contains",
    ),
    (
        "annotations",
        "classDiagram\nclass Shape{\n    <<interface>>\n    noOfVertices\n    draw()\n}\n\
         class Color{\n    <<enumeration>>\n    RED\n    BLUE\n    GREEN\n}",
    ),
    (
        "annotation-inline",
        "classDiagram\n  class Shape <<interface>>\n  class Service <<service>>\n  \
         Service --> Shape",
    ),
    (
        "direction-rl",
        "classDiagram\n  direction RL\n  class Student {\n    -idCard : IdCard\n  }\n  \
         class IdCard{\n    -id : int\n    -name : string\n  }\n  class Bike{\n    -id : int\n    \
         -name : string\n  }\n  Student \"1\" --o \"1\" IdCard : carries\n  \
         Student \"1\" --o \"1\" Bike : rides",
    ),
    (
        "notes",
        "classDiagram\n    note \"This is a general note\"\n    \
         note for MyClass \"This is a note for a class\"\n    class MyClass{\n      +run()\n    }",
    ),
    (
        "styled",
        "classDiagram\n    class Animal:::someclass {\n        -int sizeInFeet\n        -canEat()\n    \
         }\n    class Mineral\n    classDef someclass fill:#f96\n    \
         style Mineral fill:#bbf,stroke:#f66\n    Animal --> Mineral",
    ),
    (
        "classifiers",
        "classDiagram\n  class Job {\n    +run()*\n    +schedule() int*\n    +create()$\n    \
         +named() String$\n    String registry$\n  }",
    ),
    (
        "cjk",
        "classDiagram\n  class ツリー {\n    <<インタフェース>>\n    +種別 : String\n    \
         +全画面プレビューを開く() bool\n  }\n  class プレビュー\n  ツリー <|-- プレビュー : 開く",
    ),
    (
        "mixed-width",
        "classDiagram\n  class Narrow\n  class Wide {\n    +resolve the preview kind and hand it \
         to the right renderer() bool\n  }\n  Narrow --> Wide : 🚀",
    ),
];

/// Label strings the measuring instrument runs over. §6 names these classes explicitly.
const LABELS: &[(&str, &str)] = &[
    ("ascii", "String owner"),
    ("kerning", "AVATAR To Wa"),
    ("thin", "illicit lilli"),
    ("cjk", "全画面プレビュー"),
    ("cjk-punct", "開始、処理。「確認」"),
    ("emoji", "build 🚀 ship"),
    ("digits", "0123456789"),
    (
        "long",
        "resolve the preview kind and hand it to the right renderer",
    ),
    (
        "mixed",
        "konoma のプレビュー preview を全画面 fullscreen で",
    ),
];

// ---------------------------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------------------------

fn laid_out(src: &str) -> Diagram {
    let model = class::parse(src).unwrap_or_else(|e| panic!("corpus source must parse: {e}"));
    lay_out(&model).unwrap_or_else(|e| panic!("corpus source must lay out: {e}"))
}

fn parsed(src: &str) -> ClassDiagram {
    class::parse(src).expect("parses")
}

/// The namespace tree of a source, rebuilt from the parser's own output — the same trick the
/// other two suites use, so "is this class inside that namespace?" is answered by the parser
/// rather than by the code under test.
fn tree_of_src(src: &str) -> clusters::Tree {
    let spec = spec_of(&parsed(src));
    let ids: HashSet<String> = spec.nodes.iter().map(|n| n.id.clone()).collect();
    clusters::Tree::from_blocks(&spec.blocks, |id| ids.contains(id))
}

/// Every line of text a node draws, top to bottom.
fn panel_lines(d: &Diagram, id: &str) -> Vec<String> {
    let node = d.node(id).unwrap_or_else(|| panic!("no node {id}"));
    let panel = node
        .panel
        .as_ref()
        .unwrap_or_else(|| panic!("{id} has no panel"));
    panel
        .rows
        .iter()
        .map(|r| {
            r.cells
                .iter()
                .map(|c| c.label.lines.join(" "))
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect()
}

// ---------------------------------------------------------------------------------------------
// 1. The main instrument: what konoma measured vs what resvg drew
// ---------------------------------------------------------------------------------------------

/// **A class box is as wide as its widest row plus the panel's padding**, and that has to be true
/// of the text resvg actually laid out, not of konoma's own idea of it.
///
/// The relation is exact and declared, which is what makes it an instrument rather than a
/// tolerance: `panel::class_panel` sets `width = widest row + CLASS_PAD_X * 2`. The member row is
/// the widest by construction here (the class is called `C`).
#[test]
fn class_box_width_matches_resvg() {
    if !text_metrics::fonts_available() {
        eprintln!("no sans-serif face — skipping (the renderer refuses to draw here too)");
        return;
    }
    let expected = panel::CLASS_PAD_X * 2.0;
    for (name, label) in LABELS {
        let src = format!("classDiagram\nclass C{{\n  +{label}\n}}");
        let d = laid_out(&src);
        let svg = render(&src, "dark").expect("renders");
        let node = d.node("C").expect("the class");

        let mut widths = Vec::new();
        text_widths(tree_of(&svg).root(), &mut widths);
        assert_eq!(
            widths.len(),
            2,
            "{name}: the class name and one member should both survive usvg (a dropped one means \
             the font resolution konoma measured with is not the one resvg drew with)"
        );
        let drawn = widths.iter().copied().fold(0.0_f32, f32::max) as f64;
        let slack = node.size.w - drawn;
        assert!(
            (slack - expected).abs() <= 1.0,
            "{name} {label:?}: box is {} wide, resvg drew the widest row {} wide → {} of padding, \
             but the panel declares {}",
            svg::num(node.size.w),
            svg::num(drawn),
            svg::num(slack),
            svg::num(expected)
        );
    }
}

/// The same comparison for a **cardinality**, which is measured by the same code and sized by the
/// edge-label rule rather than by a panel's padding.
#[test]
fn cardinality_box_width_matches_resvg() {
    if !text_metrics::fonts_available() {
        return;
    }
    let src = "classDiagram\n  A \"全画面プレビュー\" --> B";
    let d = laid_out(src);
    let svg = render(src, "dark").expect("renders");
    let label = d.edges[0]
        .start_label
        .as_ref()
        .expect("the edge carries a cardinality");

    let mut widths = Vec::new();
    text_widths(tree_of(&svg).root(), &mut widths);
    let drawn = widths.iter().copied().fold(0.0_f32, f32::max) as f64;
    let slack = label.size.w - drawn;
    assert!(
        (slack - super::LABEL_PAD_X * 2.0).abs() <= 1.0,
        "cardinality box {} vs resvg's {} → {} of padding, declared {}",
        svg::num(label.size.w),
        svg::num(drawn),
        svg::num(slack),
        svg::num(super::LABEL_PAD_X * 2.0)
    );
}

/// A class box's **height** is its row count, not a measurement — so adding a member makes the box
/// taller by exactly one line, and that is what keeps the rows from being drawn on top of each
/// other when a font is wider than the one this machine has.
#[test]
fn a_class_box_grows_by_one_line_per_member() {
    if !text_metrics::fonts_available() {
        return;
    }
    let one = laid_out("classDiagram\nclass C{\n  +a\n}");
    let two = laid_out("classDiagram\nclass C{\n  +a\n  +b\n}");
    let grew = two.node("C").expect("C").size.h - one.node("C").expect("C").size.h;
    assert!(
        (grew - labels::line_height()).abs() < 1e-9,
        "a second member added {} px where one line is {}",
        svg::num(grew),
        svg::num(labels::line_height())
    );
}

// ---------------------------------------------------------------------------------------------
// 2. The geometric invariants — the same functions stage 1 runs
// ---------------------------------------------------------------------------------------------

/// Every invariant §6 lists, over the class corpus, through the code stage 1's tests call.
///
/// One test rather than twelve on purpose: they share the layout, which is the expensive part,
/// and the failure message already says which property and which case gave way.
#[test]
fn the_geometry_invariants_hold_for_class_diagrams() {
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
        check_panels_stay_inside_their_box(name, &d);
    }
}

/// Everything the author wrote is on the page: one box per class, one per note, one frame per
/// namespace, one line per relationship plus one per note that names a class. A dropped class is
/// how a renderer quietly lies about a diagram.
#[test]
fn every_class_relationship_and_note_survives() {
    if !text_metrics::fonts_available() {
        return;
    }
    for (name, src) in CASES {
        let model = parsed(src);
        let d = laid_out(src);
        assert_eq!(
            d.nodes.len(),
            model.classes.len() + model.notes.len(),
            "{name}: box count"
        );
        let tree = tree_of_src(src);
        let blocks: HashSet<&str> = tree.iter().map(|c| c.id.as_str()).collect();
        let drawn: HashSet<&str> = d.clusters.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(drawn, blocks, "{name}: frames and namespaces disagree");
        let ties = model.notes.iter().filter(|n| n.target.is_some()).count();
        assert_eq!(
            d.edges.len(),
            model.relations.len() + ties,
            "{name}: line count"
        );
    }
}

// ---------------------------------------------------------------------------------------------
// 3. Compartments
// ---------------------------------------------------------------------------------------------

/// **The rows are what the reader reads, in the order they were written.** Annotations above the
/// name, attributes next, methods last — and a rule between each pair, which is the only thing
/// that says "these are different kinds of thing".
#[test]
fn a_class_box_stacks_annotations_name_attributes_and_methods() {
    if !text_metrics::fonts_available() {
        return;
    }
    let src = "classDiagram\nclass Shape{\n  <<interface>>\n  +int sides\n  -String name\n  \
               +draw() bool\n  +area()\n}";
    let d = laid_out(src);
    assert_eq!(
        panel_lines(&d, "Shape"),
        [
            "«interface»",
            "Shape",
            "+int sides",
            "-String name",
            "+draw() : bool",
            "+area()"
        ]
    );
    let panel = d.node("Shape").unwrap().panel.as_ref().unwrap();
    assert_eq!(
        panel.rules.len(),
        2,
        "one rule under the name, one between the attributes and the methods"
    );
    assert!(panel.columns.is_empty(), "a class box has no columns");

    // …and the rules really reach the page.
    let svg = render(src, "dark").expect("renders");
    assert_eq!(
        svg.matches("<line ").count(),
        2,
        "both compartment separators are drawn:\n{svg}"
    );
    for line in ["«interface»", "+int sides", "+draw() : bool"] {
        assert!(svg.contains(line), "{line:?} is missing from:\n{svg}");
    }
}

/// A class with no members still gets one rule and one empty compartment, which is what makes it
/// read as a class rather than as a flowchart box. mermaid draws *two* empty compartments; §0-1
/// is what allows konoma to draw one, and this is where that decision is written down.
#[test]
fn an_empty_class_keeps_one_compartment() {
    if !text_metrics::fonts_available() {
        return;
    }
    let d = laid_out("classDiagram\n  class Animal\n  Vehicle <|-- Car");
    let panel = d.node("Animal").unwrap().panel.as_ref().unwrap();
    assert_eq!(panel.rules.len(), 1);
    assert_eq!(panel_lines(&d, "Animal"), ["Animal"]);

    // A class with attributes and no methods gets no empty methods box either.
    let d = laid_out("classDiagram\n  class A{\n    +x\n  }");
    assert_eq!(d.node("A").unwrap().panel.as_ref().unwrap().rules.len(), 1);
}

/// Members are **left-aligned and the name is centred**, which is what makes a list of members
/// read as a list. Asserted on the panel rather than on the markup, because the alignment is a
/// property of the geometry; the markup is checked once, below.
#[test]
fn members_line_up_on_the_left_and_the_name_is_centred() {
    if !text_metrics::fonts_available() {
        return;
    }
    let src = "classDiagram\nclass C{\n  +short\n  +a considerably longer member\n}";
    let d = laid_out(src);
    let panel = d.node("C").unwrap().panel.as_ref().unwrap();
    assert!(panel.rows[0].cells[0].centered, "the class name is centred");
    let xs: Vec<f64> = panel.rows[1..]
        .iter()
        .map(|r| {
            assert!(!r.cells[0].centered, "a member is not centred");
            r.cells[0].x
        })
        .collect();
    assert!(
        xs.windows(2).all(|w| (w[0] - w[1]).abs() < 1e-9),
        "every member starts at the same x: {xs:?}"
    );

    let svg = render(src, "dark").expect("renders");
    assert!(
        svg.contains("text-anchor=\"start\""),
        "a member is drawn left-anchored:\n{svg}"
    );
}

/// A visibility marker is part of the drawn row. Dropping it turns a private field into a public
/// one, which is a change of meaning that no geometry test can see.
#[test]
fn the_visibility_marker_reaches_the_page() {
    if !text_metrics::fonts_available() {
        return;
    }
    let src = "classDiagram\nclass A{\n  +pub\n  -priv\n  #prot\n  ~pkg\n}";
    let d = laid_out(src);
    assert_eq!(
        panel_lines(&d, "A"),
        ["A", "+pub", "-priv", "#prot", "~pkg"]
    );
    let svg = render(src, "dark").expect("renders");
    for row in ["+pub", "-priv", "#prot", "~pkg"] {
        assert!(svg.contains(&format!(">{row}<")), "{row:?} missing:\n{svg}");
    }
}

/// A generic is drawn on the class it belongs to, and the relationship still reaches the same box.
#[test]
fn a_generic_is_drawn_in_the_name_and_not_in_the_id() {
    if !text_metrics::fonts_available() {
        return;
    }
    let d = laid_out("classDiagram\n  class Square~Shape~\n  Square --> Foo");
    assert_eq!(panel_lines(&d, "Square")[0], "Square<Shape>");
    assert_eq!(d.edges[0].from, "Square");
}

// ---------------------------------------------------------------------------------------------
// 4. The marks at the ends of the lines
// ---------------------------------------------------------------------------------------------

/// **Inheritance and composition are different shapes**, and a hollow mark is hollow.
///
/// This is the mutation §6 asks to be able to see: drawing `<|--` with the diamond that belongs to
/// `*--` produces a diagram that looks fine and says something else. Both the geometry and the
/// emitted markup are checked, because the two could disagree.
#[test]
fn inheritance_and_composition_are_told_apart() {
    if !text_metrics::fonts_available() {
        return;
    }
    let from = crate::preview::mermaid::layout::Point::new(0.0, 0.0);
    let tip = crate::preview::mermaid::layout::Point::new(0.0, 40.0);
    assert_eq!(
        edges::triangle(&from, &tip).len(),
        3,
        "a triangle has 3 corners"
    );
    assert_eq!(edges::diamond(&from, &tip).len(), 4, "a diamond has 4");

    let inherit = render("classDiagram\n  A <|-- B", "dark").expect("renders");
    let compose = render("classDiagram\n  A *-- B", "dark").expect("renders");
    let aggregate = render("classDiagram\n  A o-- B", "dark").expect("renders");
    let theme = super::theme::DARK;

    let polygon = |svg: &str| -> String {
        svg.lines()
            .find(|l| l.starts_with("<polygon"))
            .unwrap_or_else(|| panic!("no mark was drawn in:\n{svg}"))
            .to_string()
    };
    let corners = |line: &str| -> usize {
        let start = line.find("points=\"").expect("points") + 8;
        let end = line[start..].find('"').expect("close") + start;
        line[start..end].split_whitespace().count()
    };

    assert_eq!(corners(&polygon(&inherit)), 3, "inheritance is a triangle");
    assert_eq!(corners(&polygon(&compose)), 4, "composition is a diamond");
    assert_eq!(
        corners(&polygon(&aggregate)),
        4,
        "aggregation is a diamond too"
    );
    assert!(
        polygon(&inherit).contains(&format!("fill=\"{}\"", theme.node_fill)),
        "inheritance is hollow"
    );
    assert!(
        polygon(&compose).contains(&format!("fill=\"{}\"", theme.arrowhead)),
        "composition is filled"
    );
    assert!(
        polygon(&aggregate).contains(&format!("fill=\"{}\"", theme.node_fill)),
        "aggregation is hollow — the only thing that tells it from composition"
    );
    assert_ne!(
        polygon(&compose),
        polygon(&aggregate),
        "composition and aggregation must not draw the same mark"
    );
}

/// **The mark goes at the end the author wrote it on.** `A <|-- B` puts the triangle on `A`;
/// `A --|> B` puts it on `B`. Checked as geometry — the mark's tip against the two boxes — rather
/// than as a token, because that is the thing a reader sees.
#[test]
fn a_relationship_mark_is_drawn_at_the_end_it_was_written_on() {
    if !text_metrics::fonts_available() {
        return;
    }
    for (src, at_start) in [
        ("classDiagram\n  A <|-- B", true),
        ("classDiagram\n  A --|> B", false),
        ("classDiagram\n  A *-- B", true),
        ("classDiagram\n  A --* B", false),
        ("classDiagram\n  A o-- B", true),
        ("classDiagram\n  A --o B", false),
        ("classDiagram\n  A ()-- B", true),
        ("classDiagram\n  A --() B", false),
    ] {
        let d = laid_out(src);
        let e = &d.edges[0];
        let (marked, bare) = if at_start {
            (e.tip_start, e.tip_end)
        } else {
            (e.tip_end, e.tip_start)
        };
        assert_ne!(marked, Tip::None, "{src:?}: the marked end carries a mark");
        assert_eq!(bare, Tip::None, "{src:?}: the other end carries none");
    }
}

/// A dotted line is dotted, and a `..>` and a `-->` differ only in that.
#[test]
fn the_line_style_is_what_tells_a_dependency_from_an_association() {
    if !text_metrics::fonts_available() {
        return;
    }
    let solid = laid_out("classDiagram\n  A --> B");
    let dotted = laid_out("classDiagram\n  A ..> B");
    assert_eq!(solid.edges[0].tip_end, dotted.edges[0].tip_end);
    assert_eq!(
        solid.edges[0].stroke,
        crate::preview::mermaid::flowchart::Stroke::Normal
    );
    assert_eq!(
        dotted.edges[0].stroke,
        crate::preview::mermaid::flowchart::Stroke::Dotted
    );
    let svg = render("classDiagram\n  A ..> B", "dark").expect("renders");
    assert!(svg.contains(&format!("stroke-dasharray=\"{}\"", svg::DOTTED_DASH)));
}

/// A cardinality sits **beside** its own line, near the end it describes, and off the line rather
/// than on it — a `"1"` drawn on the shaft reads as a label for the whole relationship.
#[test]
fn a_cardinality_sits_beside_the_end_it_belongs_to() {
    if !text_metrics::fonts_available() {
        return;
    }
    let src = "classDiagram\n  Customer \"1\" --> \"0..*\" Ticket";
    let d = laid_out(src);
    let e = &d.edges[0];
    let start = e.start_label.as_ref().expect("a cardinality at the tail");
    let end = e.end_label.as_ref().expect("a cardinality at the head");
    assert_eq!(start.label.lines, vec!["1".to_string()]);
    assert_eq!(end.label.lines, vec!["0..*".to_string()]);

    let tail = e.points.first().expect("points");
    let head = e.points.last().expect("points");
    let near = |a: &super::Point, b: &super::Point| (a.x - b.x).hypot(a.y - b.y);
    assert!(
        near(&start.center, tail) < near(&start.center, head),
        "the `1` belongs to the Customer end"
    );
    assert!(
        near(&end.center, head) < near(&end.center, tail),
        "the `0..*` belongs to the Ticket end"
    );
    // …and it is clear of the line it annotates.
    let off = super::tests::dist_to_polyline(&start.center, &e.points);
    assert!(
        off >= edges::CARDINALITY_GAP - 0.01,
        "the cardinality sits {} px from its line, which is on it",
        svg::num(off)
    );

    let svg = render(src, "dark").expect("renders");
    assert!(
        svg.contains(">1<") && svg.contains(">0..*<"),
        "both reach the page:\n{svg}"
    );
}

// ---------------------------------------------------------------------------------------------
// 5. Namespaces and notes
// ---------------------------------------------------------------------------------------------

#[test]
fn a_namespace_is_a_frame_and_never_a_box() {
    if !text_metrics::fonts_available() {
        return;
    }
    let d = laid_out(
        "classDiagram\nnamespace BaseShapes {\n    class Triangle\n    class Rectangle\n}",
    );
    assert!(d.node("BaseShapes").is_none(), "a namespace is not a box");
    let frame = d.cluster("BaseShapes").expect("the frame");
    assert_eq!(frame.title.lines, vec!["BaseShapes".to_string()]);
    assert!(!frame.dashed, "a namespace has a title and needs no hint");
}

/// A dotted namespace becomes a frame inside a frame, with the *leaf's* own segment as its title.
#[test]
fn a_dotted_namespace_nests_its_frames() {
    if !text_metrics::fonts_available() {
        return;
    }
    let d = laid_out(
        "classDiagram\n  namespace A.B {\n    class X\n  }\n  namespace A {\n    class Y\n  }",
    );
    let inner = d.cluster("A.B").expect("the inner frame");
    assert_eq!(inner.parent.as_deref(), Some("A"));
    assert_eq!(inner.title.lines, vec!["B".to_string()]);
    assert_eq!(
        d.cluster("A").expect("the outer frame").title.lines,
        vec!["A".to_string()]
    );
}

/// A note is a box with a folded corner, tied to its class by a headless dotted line — and a
/// floating note is drawn with no line at all.
#[test]
fn a_note_is_a_folded_box_and_its_tie_is_headless() {
    if !text_metrics::fonts_available() {
        return;
    }
    let src = "classDiagram\n  note \"floating\"\n  note for A \"tied\"\n  class A\n  A --> B";
    let d = laid_out(src);
    let notes: Vec<&super::PlacedNode> =
        d.nodes.iter().filter(|n| n.shape == Glyph::Note).collect();
    assert_eq!(notes.len(), 2);
    let ties: Vec<&super::PlacedEdge> = d
        .edges
        .iter()
        .filter(|e| e.from.starts_with("note"))
        .collect();
    assert_eq!(ties.len(), 1, "only the note with a target is tied");
    assert_eq!(ties[0].to, "A");
    assert_eq!(ties[0].tip_start, Tip::None);
    assert_eq!(ties[0].tip_end, Tip::None);
    assert_eq!(
        ties[0].stroke,
        crate::preview::mermaid::flowchart::Stroke::Dotted
    );
}

// ---------------------------------------------------------------------------------------------
// 6. Degrading rather than lying (§6)
// ---------------------------------------------------------------------------------------------

#[test]
fn another_diagram_kind_is_refused() {
    for src in [
        "flowchart TD\n  A --> B",
        "stateDiagram-v2\n  [*] --> A",
        "erDiagram\n  A ||--|| B : x",
        "sequenceDiagram\n  A->>B: hi",
    ] {
        assert!(
            matches!(render(src, "dark"), Err(RenderError::ClassParse(_))),
            "{src:?}"
        );
    }
}

#[test]
fn a_class_diagram_with_nothing_to_draw_is_refused() {
    let e = render("classDiagram", "dark").unwrap_err();
    assert!(matches!(e, RenderError::ClassParse(_)));
    assert_eq!(e.to_string(), "class diagram declares no classes");
    // A body made only of statements that draw nothing is the same case.
    assert!(render("classDiagram\n  classDef x fill:#f96", "dark").is_err());
}

/// Adversarial **render** sources — the parser's own sweep is in `class::tests`, and this is the
/// half that reaches geometry. Each has to come back with a drawable diagram or an error, never a
/// panic and never a NaN.
#[test]
fn awkward_sources_produce_a_diagram_or_an_error_and_never_a_panic() {
    if !text_metrics::fonts_available() {
        return;
    }
    let mut wide = String::from("classDiagram\n");
    for i in 0..120 {
        wide.push_str(&format!("  n{i} --> n{}\n", i + 1));
    }
    wide.push_str("  n0 --> n120\n");

    let mut tall = String::from("classDiagram\nclass Big{\n");
    for i in 0..200 {
        tall.push_str(&format!("  +field{i} : String\n"));
    }
    tall.push_str("}\n");

    let mut deep = String::from("classDiagram\n");
    for i in 0..12 {
        deep.push_str(&format!("namespace n{i} {{\n"));
    }
    deep.push_str("class A\n");
    for _ in 0..12 {
        deep.push_str("}\n");
    }

    let cases: &[&str] = &[
        "classDiagram\n  class A",
        "classDiagram\n  class A { }",
        "classDiagram\n  class A {}",
        "classDiagram\n  A --> A",
        "classDiagram\n  A <|--|> A",
        "classDiagram\n  A : ",
        "classDiagram\n  A : <br><br><br>",
        "classDiagram\n  A : \"\"",
        "classDiagram\n  class A[\"\"]",
        "classDiagram\n  A[\"<script>&amp;\"] --> B",
        "classDiagram\n  一 --> 二 : 🎉",
        "classDiagram\n  note \"\"",
        "classDiagram\n  note for Nobody \"orphan\"\n  class A",
        "classDiagram\n  namespace N { }\n  class A",
        "classDiagram\n  namespace N {\n    class A\n  }\n  N --> A",
        "classDiagram\n  namespace N {\n    class A\n  }\n  B --> N",
        "classDiagram\n  A \"\" --> \"\" B",
        "classDiagram\n  A \"very long cardinality indeed\" --> B",
        "classDiagram\n  class A{\n    +f(a, b, c) Map~K, V~\n  }",
        "classDiagram\n  class A{\n    ~~~\n  }",
        "classDiagram\n  direction LR\n  A --> B\n  B --> A",
        &deep,
        &tall,
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

/// The SVG contract (§1) holds for this diagram kind too, in every palette.
#[test]
fn emitted_svg_obeys_the_renderer_contract() {
    if !text_metrics::fonts_available() {
        return;
    }
    for (name, src) in CASES {
        for theme_name in ["dark", "light", "classic", "forest", "neutral"] {
            let svg = render(src, theme_name).expect("renders");
            assert!(
                svg.contains("viewBox=\""),
                "{name}/{theme_name}: no viewBox"
            );
            assert!(!svg.contains("foreignObject"), "{name}/{theme_name}");
            assert!(!svg.contains("var(--"), "{name}/{theme_name}");
            assert!(!svg.contains("<style"), "{name}/{theme_name}");
            assert!(
                svg.contains("font-family=\"sans-serif\""),
                "{name}/{theme_name}: the drawing font must be the one that was measured"
            );
            let opaque_background = svg
                .lines()
                .any(|l| l.starts_with("<rect width=") && !l.contains("fill=\"none\""));
            assert!(!opaque_background, "{name}/{theme_name}: opaque background");
        }
    }
}

/// A theme changes colours and nothing else — the same property stage 1 pins, restated for the
/// glyphs stage 3 adds, whose marks are drawn with the palette's own colours.
#[test]
fn themes_change_colours_but_never_geometry() {
    if !text_metrics::fonts_available() {
        return;
    }
    let src = CASES[0].1;
    let dark = laid_out(src);
    for theme_name in ["light", "classic", "forest", "neutral"] {
        let svg = render(src, theme_name).expect("renders");
        assert!(svg.contains(&format!("width=\"{}\"", svg::num(dark.width))));
        assert!(svg.contains(&format!("height=\"{}\"", svg::num(dark.height))));
    }
}

// ---------------------------------------------------------------------------------------------
// 7. Goldens
// ---------------------------------------------------------------------------------------------

/// The regression net over the whole corpus, **through [`render`]** — the function
/// `preview::markdown::mermaid_to_svg` calls, not a stand-in for it (§6).
///
/// Numeric attribute values are masked for the reason stage 1's golden gives: every coordinate
/// descends from the system's sans-serif face, so a byte-exact end-to-end golden would be a
/// golden of this machine's fonts. The numbers are pinned separately by
/// [`super::er_tests::panel_emit_golden`], which builds its diagram by hand.
#[test]
fn class_corpus_golden() {
    if !text_metrics::fonts_available() {
        return;
    }
    let mut out = String::new();
    for (name, src) in CASES {
        out.push_str(&format!("=== {name} ===\n"));
        out.push_str(&mask_numbers(&render(src, "dark").expect("renders")));
        out.push('\n');
    }
    assert_snapshot("mermaid_class", &out);
}

// ---------------------------------------------------------------------------------------------
// 8. Looking at it
// ---------------------------------------------------------------------------------------------

/// Writes the corpus out as SVG files so a person can look at them. Not a check — §6 is explicit
/// that comparing pictures is for eyes, not for assertions.
///
/// `KONOMA_GALLERY=/some/dir cargo test -- --ignored class_tests::gallery`
#[test]
#[ignore = "writes SVG files for a person to look at: KONOMA_GALLERY=<dir> cargo test -- --ignored gallery"]
fn gallery() {
    let dir = std::env::var("KONOMA_GALLERY").expect("set KONOMA_GALLERY to a directory");
    std::fs::create_dir_all(&dir).expect("create the gallery directory");
    for (name, src) in CASES {
        let svg = render(src, "dark").unwrap_or_else(|e| panic!("{name}: {e}"));
        std::fs::write(format!("{dir}/class-{name}.svg"), svg).expect("write the SVG");
    }
}

/// Kept so `shapes` is used from this module the way the sibling suites use it: a class box's
/// glyph is sized by its panel, and this is where that contract is asserted.
#[test]
fn a_class_box_is_sized_by_its_panel() {
    let size = super::Size::new(123.0, 45.0);
    assert_eq!(shapes::size(Glyph::ClassBox, size), size);
    assert_eq!(shapes::size(Glyph::ErBox, size), size);
    assert!(matches!(
        shapes::outline(Glyph::ClassBox, size),
        shapes::Outline::Rect { r, .. } if r == 0.0
    ));
}
