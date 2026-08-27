//! What stage 3's ER drawing is checked with.
//!
//! The shape is stage 1's and stage 2's — one instrument that measures konoma against resvg, the
//! shared geometric invariants over a corpus built from mermaid's own documentation, a masked
//! golden through the production entry point, and an exact one built by hand — with one property
//! that only an ER diagram has:
//!
//! **which end a crow's foot goes on.** `CUSTOMER ||--o{ ORDER` means "one customer, many
//! orders", and the marks are drawn where the spelling puts them. Swapping them leaves a picture
//! that is entirely plausible and says the opposite thing, so
//! [`a_crows_foot_is_drawn_at_the_end_that_asked_for_it`] checks the geometry rather than the
//! model — the mark's own toes against the two boxes.

use std::collections::HashSet;

use super::edges::Tip;
use super::er::{lay_out, render, spec_of};
use super::tests::{
    assert_snapshot, check_cluster_titles, check_clusters_hold_their_members,
    check_edge_that_names_a_block_stops_on_its_frame, check_edges_keep_out_of_foreign_frames,
    check_edges_stay_out_of_shapes, check_endpoints_land_on_the_outline,
    check_labels_ride_their_edge, check_nested_clusters_sit_inside_their_parent,
    check_nodes_do_not_overlap, check_panels_stay_inside_their_box,
    check_unrelated_clusters_do_not_overlap, check_view_box_contains_everything, mask_numbers,
    measured_texts, text_widths, tree_of,
};
use super::{
    clusters, edges, labels, panel, shapes, svg, Diagram, Glyph, Label, PlacedCluster, PlacedEdge,
    PlacedEdgeLabel, PlacedNode, RenderError, Size,
};
use crate::preview::mermaid::er::{self, ErDiagram};
use crate::preview::mermaid::flowchart::{Shape, Stroke};
use crate::preview::mermaid::layout::Point;
use crate::preview::mermaid::text_metrics;

// ---------------------------------------------------------------------------------------------
// Corpus
// ---------------------------------------------------------------------------------------------

/// The diagrams every structural test runs over, and the ones [`gallery`] writes out.
pub const CASES: &[(&str, &str)] = &[
    (
        "order",
        "erDiagram\n    CUSTOMER ||--o{ ORDER : places\n    ORDER ||--|{ LINE-ITEM : contains\n    \
         CUSTOMER }|..|{ DELIVERY-ADDRESS : uses",
    ),
    (
        "attributes",
        "erDiagram\n    CUSTOMER ||--o{ ORDER : places\n    CUSTOMER {\n        string name\n        \
         string custNumber\n        string sector\n    }\n    ORDER ||--|{ LINE-ITEM : contains\n    \
         ORDER {\n        int orderNumber\n        string deliveryAddress\n    }\n    LINE-ITEM {\n        \
         string productCode\n        int quantity\n        float pricePerUnit\n    }",
    ),
    (
        "keys-and-comments",
        "erDiagram\n    CAR ||--o{ NAMED-DRIVER : allows\n    CAR {\n        \
         string registrationNumber PK\n        string make\n        string model\n        \
         string[] parts\n    }\n    PERSON ||--o{ NAMED-DRIVER : is\n    PERSON {\n        \
         string driversLicense PK \"The license #\"\n        \
         string(99) firstName \"Only 99 characters are allowed\"\n        string lastName\n        \
         string phone UK\n        int age\n    }\n    NAMED-DRIVER {\n        \
         string carRegistrationNumber PK, FK\n        string driverLicence PK, FK\n    }\n    \
         MANUFACTURER only one to zero or more CAR : makes",
    ),
    (
        "no-attributes",
        "erDiagram\n    CAR ||--o{ NAMED-DRIVER : allows\n    PERSON }o..o{ NAMED-DRIVER : is",
    ),
    (
        "words",
        "erDiagram\n    CAR 1 to zero or more NAMED-DRIVER : allows\n    \
         PERSON many(0) optionally to 0+ NAMED-DRIVER : is",
    ),
    (
        // Every cardinality symbol, mirrored, on one page. The case a person looks at to see that
        // the marks are the right way round.
        "cardinalities",
        "erDiagram\n  A ||--|| B : one\n  C |o--o| D : zeroOrOne\n  E }o--o{ F : zeroOrMore\n  \
         G }|--|{ H : oneOrMore\n  I ||--o{ J : mixed",
    ),
    (
        "aliases",
        "erDiagram\n    p[Person] {\n        string firstName\n        string lastName\n    }\n    \
         a[\"Customer Account\"] {\n        string email\n    }\n    p ||--o| a : has",
    ),
    (
        // Comments but no keys: the case that decides whether the *third* column can be dropped
        // while the fourth stays. Every other combination is covered by `keys-and-comments`.
        "comments-only",
        "erDiagram\n  E {\n    string a \"first\"\n    int b \"second\"\n  }\n  \
         F {\n    string c PK\n  }\n  E ||--o{ F : has",
    ),
    (
        "optional-types",
        "erDiagram\n    PERSON {\n        string firstName\n        string? middleName\n        \
         string lastName\n    }",
    ),
    (
        "unicode-name",
        "erDiagram\n    \"This ❤ Unicode\" ||--|| OTHER : links",
    ),
    (
        "direction-lr",
        "erDiagram\n    direction LR\n    CUSTOMER ||--o{ ORDER : places\n    CUSTOMER {\n        \
         string name\n    }\n    ORDER {\n        int orderNumber\n    }",
    ),
    (
        "subgraph",
        "erDiagram\n    subgraph title1\n        CUSTOMER\n        CUSTOMER {\n            \
         string name\n            string custNumber\n        }\n    end\n    subgraph title2\n        \
         CAR ||--o{ NAMED-DRIVER : allows\n        subgraph title3\n            PERSON\n            \
         PERSON {\n                string firstName\n                int age\n            }\n        \
         end\n    end",
    ),
    (
        "subgraph-endpoint",
        "erDiagram\n    subgraph title1\n        A1 ||--|| A2 : links\n    end\n\n    \
         subgraph title2\n        B1 ||--|| B2 : links\n    end\n\n    title1 ||--|| title2 : links",
    ),
    (
        "subgraph-titled",
        "erDiagram\n    subgraph id1 [title 1]\n        CUSTOMER\n    end\n    \
         subgraph \"Customer Domain\"\n        ORDER\n    end",
    ),
    (
        "styled",
        "erDiagram\n    direction TB\n    CAR:::someclass {\n        string registrationNumber\n        \
         string make\n    }\n    PERSON:::someclass {\n        string firstName\n    }\n    \
         HOUSE:::someclass\n\n    classDef someclass fill:#f96",
    ),
    (
        "cjk",
        "erDiagram\n    顧客 ||--o{ 注文 : 発注する\n    顧客 {\n        string 名前 PK \"表示名\"\n        \
         string 電話番号 UK\n    }\n    注文 {\n        int 注文番号\n        string 配送先住所\n    }",
    ),
    (
        "self",
        "erDiagram\n  NODE ||--o{ NODE : parent-of",
    ),
    (
        // **Every legal thing that can follow a `[…]`**, which nothing else here draws: `aliases`
        // always pairs its alias with an attribute block, so a bare alias, an alias carrying a
        // style separator, and a title with a `]` of its own were all undrawn. That is the corpus
        // gap the bracket defect fell through — it was found while writing a sample, not by a
        // test.
        //
        // **Be exact about what the golden of this case can see.** It sees that these forms lay
        // out and emit, and that the `]` inside a quoted title reaches the box rather than cutting
        // the name short. It does *not* see the `:::vip` — the ER renderer draws no `classDef`
        // styling at all (the `styled` case above is the same), so the class is a parse-level fact
        // and `er::tests::a_bracket_alias_is_only_a_declaration` is what states it.
        "alias-forms",
        "erDiagram\n    p[Person]\n    a[\"Customer Account\"]:::vip\n             b[\"Bracket ] In Title\"]\n    p ||--o| a : has\n    p ||--o{ b : notes\n             classDef vip fill:#f96",
    ),
];

/// Label strings the measuring instrument runs over.
const LABELS: &[(&str, &str)] = &[
    ("ascii", "registrationNumber"),
    ("kerning", "AVATARToWa"),
    ("thin", "illicitlilli"),
    ("cjk", "全画面プレビュー"),
    ("emoji", "build🚀ship"),
    ("digits", "0123456789"),
    ("long", "resolveThePreviewKindAndHandItToTheRightRenderer"),
];

// ---------------------------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------------------------

fn laid_out(src: &str) -> Diagram {
    let model = er::parse(src).unwrap_or_else(|e| panic!("corpus source must parse: {e}"));
    lay_out(&model).unwrap_or_else(|e| panic!("corpus source must lay out: {e}"))
}

fn parsed(src: &str) -> ErDiagram {
    er::parse(src).expect("parses")
}

fn tree_of_src(src: &str) -> clusters::Tree {
    let spec = spec_of(&parsed(src));
    let ids: HashSet<String> = spec.nodes.iter().map(|n| n.id.clone()).collect();
    clusters::Tree::from_blocks(&spec.blocks, |id| ids.contains(id))
}

/// The grid an entity draws, row by row and cell by cell.
fn grid(d: &Diagram, id: &str) -> Vec<Vec<String>> {
    let node = d.node(id).unwrap_or_else(|| panic!("no node {id}"));
    let panel = node
        .panel
        .as_ref()
        .unwrap_or_else(|| panic!("{id} has no panel"));
    panel
        .rows
        .iter()
        .map(|r| r.cells.iter().map(|c| c.label.lines.join(" ")).collect())
        .collect()
}

// ---------------------------------------------------------------------------------------------
// 1. The main instrument: what konoma measured vs what resvg drew
// ---------------------------------------------------------------------------------------------

/// **An entity box is as wide as its columns**, and a column is its widest cell plus a pad on each
/// side. Compared against the widths resvg actually laid the two cells out at, which is the one
/// comparison in this file that is not konoma against konoma.
///
/// The precondition — that the columns are wider than the name band needs — is asserted rather
/// than assumed, because a test that silently takes the other branch measures nothing.
#[test]
fn entity_box_width_matches_resvg() {
    if !text_metrics::fonts_available() {
        eprintln!("no sans-serif face — skipping (the renderer refuses to draw here too)");
        return;
    }
    let expected = panel::ER_CELL_PAD_X * 4.0;
    for (name, label) in LABELS {
        let src = format!("erDiagram\n  E {{\n    {label} {label}\n  }}");
        let d = laid_out(&src);
        let svg = render(&src, "dark").expect("renders");
        let node = d.node("E").expect("the entity");

        let mut widths = Vec::new();
        text_widths(tree_of(&svg).root(), &mut widths);
        assert_eq!(
            widths.len(),
            3,
            "{name}: the entity name and the two cells should all survive usvg"
        );
        // The two cells hold the same text, so the two widest of the three are the columns.
        let mut sorted = widths.clone();
        sorted.sort_by(|a, b| b.partial_cmp(a).expect("no NaN"));
        let cells = (sorted[0] + sorted[1]) as f64;
        let name_band = Label::measure("E").width + panel::ER_NAME_PAD_X * 2.0;
        assert!(
            cells + expected >= name_band.max(panel::ER_MIN_WIDTH),
            "{name}: the columns must be the wider of the two, or this measures the name band"
        );
        let slack = node.size.w - cells;
        assert!(
            (slack - expected).abs() <= 1.0,
            "{name} {label:?}: box is {} wide, resvg drew the two cells {} wide → {} of padding, \
             but the panel declares {}",
            svg::num(node.size.w),
            svg::num(cells),
            svg::num(slack),
            svg::num(expected)
        );
    }
}

/// The same instrument for a name that carries **two spaces in a row**.
///
/// It cannot ride [`LABELS`]: those are used as `type name` inside `{ }`, where the ER grammar
/// splits on whitespace, so a doubled space there would be a parse error rather than a
/// measurement. An aliased name — `e["…"]` — is where an ER diagram can actually hold one, and it
/// has to be written **as its own declaration**: `SQS entityName SQE` is in the declaration rules
/// only, so `e["…"] ||--|| B : x` on one line is a statement mermaid throws on and konoma refuses
/// (`a_bracket_alias_is_only_a_declaration`). This fixture used to be written that way and drew a
/// one-entity diagram with the relationship silently gone.
///
/// usvg folds runs of whitespace *before* it shapes, so this name is drawn one space narrower
/// than its bytes. Measuring the bytes would size a band the text does not fill, and every
/// coordinate downstream would move with it. `docs/STATUS.md` recorded that hole as belonging to
/// every diagram kind, which is why each kind's instrument now carries a case of it.
#[test]
fn entity_name_band_with_a_doubled_space_matches_resvg() {
    if !text_metrics::fonts_available() {
        return;
    }
    let src = "erDiagram\n  e[\"Customer  Account\"]\n  e ||--|| B : x";
    let d = laid_out(src);
    let svg = render(src, "dark").expect("renders");
    let node = d.node("e").expect("the aliased entity");

    // The string konoma emits is the folded one — `Label::measure` folds once, up front, so what
    // was measured and what is drawn cannot drift apart.
    let mut texts = Vec::new();
    measured_texts(tree_of(&svg).root(), &mut texts);
    assert!(
        texts.iter().any(|(s, _)| s == "Customer Account"),
        "the emitted name should be the folded one; got {:?}",
        texts.iter().map(|(s, _)| s).collect::<Vec<_>>()
    );
    let drawn = texts
        .iter()
        .find(|(s, _)| s == "Customer Account")
        .map(|(_, w)| *w)
        .expect("resvg drew the name");
    let slack = node.size.w - drawn;
    assert!(
        (slack - panel::ER_NAME_PAD_X * 2.0).abs() <= 1.0,
        "name band {} vs resvg's {} → {} of padding, declared {}",
        svg::num(node.size.w),
        svg::num(drawn),
        svg::num(slack),
        svg::num(panel::ER_NAME_PAD_X * 2.0)
    );
}

/// An entity with no attributes is the other branch, and it has its own exact rule: the name band
/// plus its padding, never narrower than [`panel::ER_MIN_WIDTH`].
#[test]
fn an_entity_with_no_attributes_is_sized_by_its_name() {
    if !text_metrics::fonts_available() {
        return;
    }
    let d = laid_out("erDiagram\n  SHORT ||--|| A_VERY_LONG_ENTITY_NAME_INDEED : x");
    let short = d.node("SHORT").expect("SHORT");
    assert!(
        (short.size.w - panel::ER_MIN_WIDTH).abs() < 0.01,
        "a short name is held at the minimum width, not squeezed: {}",
        svg::num(short.size.w)
    );
    let long = d
        .node("A_VERY_LONG_ENTITY_NAME_INDEED")
        .expect("the long one");
    let expected =
        Label::measure("A_VERY_LONG_ENTITY_NAME_INDEED").width + panel::ER_NAME_PAD_X * 2.0;
    assert!(
        (long.size.w - expected).abs() < 0.01,
        "a long name decides the width: {} vs {}",
        svg::num(long.size.w),
        svg::num(expected)
    );
}

/// An entity box grows by exactly one row per attribute.
#[test]
fn an_entity_box_grows_by_one_row_per_attribute() {
    if !text_metrics::fonts_available() {
        return;
    }
    let one = laid_out("erDiagram\n  E {\n    string a\n  }");
    let two = laid_out("erDiagram\n  E {\n    string a\n    string b\n  }");
    let grew = two.node("E").expect("E").size.h - one.node("E").expect("E").size.h;
    let row = labels::line_height() + panel::ER_ROW_PAD_Y * 2.0;
    assert!(
        (grew - row).abs() < 1e-9,
        "a second attribute added {} px where one row is {}",
        svg::num(grew),
        svg::num(row)
    );
}

// ---------------------------------------------------------------------------------------------
// 2. The geometric invariants — the same functions stage 1 runs
// ---------------------------------------------------------------------------------------------

#[test]
fn the_geometry_invariants_hold_for_er_diagrams() {
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

#[test]
fn every_entity_and_relationship_survives() {
    if !text_metrics::fonts_available() {
        return;
    }
    for (name, src) in CASES {
        let model = parsed(src);
        let d = laid_out(src);
        assert_eq!(d.nodes.len(), model.entities.len(), "{name}: box count");
        let tree = tree_of_src(src);
        let blocks: HashSet<&str> = tree.iter().map(|c| c.id.as_str()).collect();
        let drawn: HashSet<&str> = d.clusters.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(drawn, blocks, "{name}: frames and subgraphs disagree");
        assert_eq!(
            d.edges.len(),
            model.relationships.len(),
            "{name}: line count"
        );
    }
}

// ---------------------------------------------------------------------------------------------
// 3. The attribute grid
// ---------------------------------------------------------------------------------------------

/// **An entity is a framed grid, not a list of strings.** The name band on top, a rule under it,
/// one row per attribute, and a rule down each column boundary — which is what puts `PK` under
/// `PK` instead of wherever the previous row happened to end.
#[test]
fn an_entity_box_is_a_framed_grid() {
    if !text_metrics::fonts_available() {
        return;
    }
    let src = "erDiagram\n  PERSON {\n    string driversLicense PK \"The license #\"\n    \
               string lastName\n    string phone UK\n  }";
    let d = laid_out(src);
    assert_eq!(
        grid(&d, "PERSON"),
        vec![
            vec!["PERSON".to_string()],
            vec![
                "string".to_string(),
                "driversLicense".to_string(),
                "PK".to_string(),
                "The license #".to_string()
            ],
            vec!["string".to_string(), "lastName".to_string()],
            vec!["string".to_string(), "phone".to_string(), "UK".to_string()],
        ]
    );
    let panel = d.node("PERSON").unwrap().panel.as_ref().unwrap();
    assert_eq!(
        panel.rules.len(),
        3,
        "one rule under the name and one between each pair of rows"
    );
    assert_eq!(
        panel.columns.len(),
        3,
        "four columns are present, so three boundaries are drawn"
    );
    // Every row's cells start on their column, so a column really is a column.
    let starts: Vec<f64> = panel.rows[1].cells.iter().map(|c| c.x).collect();
    for row in &panel.rows[2..] {
        for (i, cell) in row.cells.iter().enumerate() {
            assert!(
                (cell.x - starts[i]).abs() < 1e-9,
                "cell {i} of a later row starts at {} rather than {}",
                svg::num(cell.x),
                svg::num(starts[i])
            );
        }
    }

    let svg = render(src, "dark").expect("renders");
    assert_eq!(
        svg.matches("<line ").count(),
        6,
        "three horizontal rules and three column rules reach the page:\n{svg}"
    );
    for text in ["driversLicense", "PK", "The license #", "UK"] {
        assert!(svg.contains(text), "{text:?} is missing from:\n{svg}");
    }
}

/// **A column that nothing fills is not drawn.** `erBox.ts` drops a column whose widest cell is
/// empty, and it has to: the common `string name` entity would otherwise be drawn with two blank
/// columns and two rules through the middle of it.
#[test]
fn an_empty_column_is_left_out() {
    if !text_metrics::fonts_available() {
        return;
    }
    let plain = laid_out("erDiagram\n  E {\n    string a\n    string b\n  }");
    let panel = plain.node("E").unwrap().panel.as_ref().unwrap();
    assert_eq!(panel.columns.len(), 1, "type | name, so one boundary");

    let keyed = laid_out("erDiagram\n  E {\n    string a PK\n    string b\n  }");
    assert_eq!(
        keyed
            .node("E")
            .unwrap()
            .panel
            .as_ref()
            .unwrap()
            .columns
            .len(),
        2,
        "one attribute with a key brings the keys column back for the whole entity"
    );

    // Comments with no keys: the *third* column goes and the fourth stays, which is the
    // combination a naive "drop the trailing empties" would get wrong.
    let commented = laid_out("erDiagram\n  E {\n    string a \"why\"\n  }");
    let panel = commented.node("E").unwrap().panel.as_ref().unwrap();
    assert_eq!(panel.columns.len(), 2, "type | name | comment");
    assert_eq!(
        panel.rows[1].cells.len(),
        3,
        "and the row has three cells, not four"
    );
    // The comment starts on the last column, not where the keys column would have been.
    let last = *panel.columns.last().expect("a boundary");
    assert!(
        (panel.rows[1].cells[2].x - (last + panel::ER_CELL_PAD_X)).abs() < 1e-9,
        "the comment starts at {} but its column begins at {}",
        svg::num(panel.rows[1].cells[2].x),
        svg::num(last)
    );
}

/// An entity with no attributes has no rules at all: it is a plain named box, which is what
/// `erBox.ts` falls back to.
#[test]
fn an_entity_with_no_attributes_is_a_plain_box() {
    if !text_metrics::fonts_available() {
        return;
    }
    let d = laid_out("erDiagram\n  A ||--|| B : x");
    let panel = d.node("A").unwrap().panel.as_ref().unwrap();
    assert!(panel.rules.is_empty());
    assert!(panel.columns.is_empty());
    assert_eq!(grid(&d, "A"), vec![vec!["A".to_string()]]);
}

/// An alias is what is drawn; the id is what relationships use.
#[test]
fn an_alias_is_drawn_and_the_id_is_not() {
    if !text_metrics::fonts_available() {
        return;
    }
    let d = laid_out("erDiagram\n  p[Person] {\n    string firstName\n  }\n  a[\"Customer Account\"] {\n    string email\n  }\n  p ||--o| a : has");
    assert_eq!(grid(&d, "p")[0], vec!["Person".to_string()]);
    assert_eq!(grid(&d, "a")[0], vec!["Customer Account".to_string()]);
    assert_eq!(d.edges[0].from, "p");
}

// ---------------------------------------------------------------------------------------------
// 4. The marks at the ends of the lines
// ---------------------------------------------------------------------------------------------

/// **The crow's foot goes on the entity whose spelling asked for it.**
///
/// `CUSTOMER ||--o{ ORDER` reads left to right: `||` next to CUSTOMER, `{` next to ORDER. Swap
/// them and the diagram says "many customers place one order" while looking exactly as
/// convincing. So this is checked on the *drawn geometry* — the foot's own prongs against the two
/// boxes — rather than on the model, which is the same thing said twice.
#[test]
fn a_crows_foot_is_drawn_at_the_end_that_asked_for_it() {
    if !text_metrics::fonts_available() {
        return;
    }
    let d = laid_out("erDiagram\n  CUSTOMER ||--o{ ORDER : places");
    let e = &d.edges[0];
    assert_eq!(e.tip_start, Tip::ErOnlyOne);
    assert_eq!(e.tip_end, Tip::ErZeroOrMore);

    // The drawn prongs open onto ORDER's box and touch nothing of CUSTOMER's.
    let points = e.drawn_points();
    let n = points.len();
    let foot = edges::crows_foot(&points[n - 2], &points[n - 1]);
    let order = d.node("ORDER").expect("ORDER").bounds();
    let customer = d.node("CUSTOMER").expect("CUSTOMER").bounds();
    let on = |p: &Point, r: (f64, f64, f64, f64)| {
        p.x >= r.0 - 0.5 && p.x <= r.2 + 0.5 && p.y >= r.1 - 0.5 && p.y <= r.3 + 0.5
    };
    for (_, toe) in &foot {
        assert!(
            on(toe, order),
            "a prong ends at ({}, {}), which is not on ORDER",
            svg::num(toe.x),
            svg::num(toe.y)
        );
        assert!(
            !on(toe, customer),
            "a prong reaches CUSTOMER, which asked for `||`"
        );
    }

    // …and the `||` end really is two bars and no foot.
    let svg = render("erDiagram\n  CUSTOMER ||--o{ ORDER : places", "dark").expect("renders");
    assert_eq!(
        svg.matches("<line ").count(),
        2,
        "the `||` end draws two bars and nothing else:\n{svg}"
    );
    assert_eq!(
        svg.matches("<circle").count(),
        1,
        "the `o` of `o{{` is one ring:\n{svg}"
    );
}

/// Every cardinality draws a different set of marks, and the mirrored spellings draw the same
/// thing on opposite ends.
#[test]
fn each_cardinality_draws_its_own_marks() {
    if !text_metrics::fonts_available() {
        return;
    }
    let seen: Vec<(Tip, Tip)> = laid_out(
        "erDiagram\n  A ||--|| B : x\n  C |o--o| D : x\n  E }o--o{ F : x\n  G }|--|{ H : x",
    )
    .edges
    .iter()
    .map(|e| (e.tip_start, e.tip_end))
    .collect();
    assert_eq!(
        seen,
        [
            (Tip::ErOnlyOne, Tip::ErOnlyOne),
            (Tip::ErZeroOrOne, Tip::ErZeroOrOne),
            (Tip::ErZeroOrMore, Tip::ErZeroOrMore),
            (Tip::ErOneOrMore, Tip::ErOneOrMore),
        ]
    );

    // The four kinds are drawn differently: bars, ring, foot, in the four documented combinations.
    let marks = |src: &str| -> (usize, usize, usize) {
        let svg = render(src, "dark").expect("renders");
        let feet = svg
            .lines()
            .filter(|l| l.starts_with("<path") && l.matches(" L").count() == 3)
            .count();
        (
            svg.matches("<line ").count(),
            svg.matches("<circle").count(),
            feet,
        )
    };
    assert_eq!(marks("erDiagram\n  A ||--|| B : x"), (4, 0, 0), "four bars");
    assert_eq!(
        marks("erDiagram\n  A |o--o| B : x"),
        (2, 2, 0),
        "a bar and a ring at each end"
    );
    assert_eq!(
        marks("erDiagram\n  A }o--o{ B : x"),
        (0, 2, 2),
        "a foot and a ring at each end"
    );
    assert_eq!(
        marks("erDiagram\n  A }|--|{ B : x"),
        (2, 0, 2),
        "a foot and a bar at each end"
    );
}

/// Non-identifying is a dashed line, which is the only thing that tells `..` from `--`.
#[test]
fn a_non_identifying_relationship_is_dashed() {
    if !text_metrics::fonts_available() {
        return;
    }
    assert_eq!(
        laid_out("erDiagram\n  A ||--|| B : x").edges[0].stroke,
        Stroke::Normal
    );
    assert_eq!(
        laid_out("erDiagram\n  A ||..|| B : x").edges[0].stroke,
        Stroke::Dotted
    );
    let svg = render("erDiagram\n  A ||..|| B : x", "dark").expect("renders");
    assert!(svg.contains(&format!("stroke-dasharray=\"{}\"", svg::DOTTED_DASH)));
}

/// A role is drawn on the middle of the line, exactly as a flowchart's edge label is — the same
/// code, so the same guarantee.
#[test]
fn a_role_rides_the_middle_of_its_line() {
    if !text_metrics::fonts_available() {
        return;
    }
    let d = laid_out("erDiagram\n  CUSTOMER ||--o{ ORDER : places");
    let label = d.edges[0].label.as_ref().expect("a role");
    assert_eq!(label.label.lines, vec!["places".to_string()]);
    let off = super::tests::dist_to_polyline(&label.center, &d.edges[0].points);
    assert!(
        off <= 0.5,
        "the role sits {} px off its line",
        svg::num(off)
    );
}

// ---------------------------------------------------------------------------------------------
// 5. Subgraphs
// ---------------------------------------------------------------------------------------------

#[test]
fn a_subgraph_is_a_frame_and_may_be_an_endpoint() {
    if !text_metrics::fonts_available() {
        return;
    }
    let src = "erDiagram\n  subgraph title1\n    A1 ||--|| A2 : links\n  end\n  \
               subgraph title2\n    B1 ||--|| B2 : links\n  end\n  title1 ||--|| title2 : links";
    let d = laid_out(src);
    assert!(d.node("title1").is_none(), "a subgraph is not a box");
    assert_eq!(
        d.cluster("title1").expect("the frame").title.lines,
        vec!["title1".to_string()]
    );
    let tree = tree_of_src(src);
    check_edge_that_names_a_block_stops_on_its_frame("subgraph-endpoint", &d, &tree);
}

/// **A recorded gap, not something papered over.** A relationship drawn between two *frames* that
/// sit side by side has almost no line to put its marks on, so its own label covers them.
///
/// `title1 ||--|| title2 : links` is cut back to each frame's boundary, and dagre leaves adjacent
/// siblings about half a `nodesep` apart. The remaining line is shorter than the room the two
/// `||` marks need ([`edges::ER_FAR`] from each end), and the role's opaque patch sits on the
/// middle of it — so a reader sees two frames and a word, with the cardinality gone.
///
/// It is not the recorded "an edge naming a block is routed between stand-in leaves" gap: the
/// route here is correct and the cut is correct. What is missing is that neither the terminator
/// room nor the label patch has any say in how far apart the layout puts two frames. Closing it
/// means either giving a cluster-to-cluster edge a minimum length before layout or leaving the
/// patch off a line this short, and both are changes to the shared pipeline rather than to stage
/// 3 — so it is written down here and left failing, the way konoma recorded the nested
/// `<details>` ordinal bug.
#[test]
#[ignore = "known gap: two frames laid out side by side leave a line too short for its own cardinality marks, and the role's patch covers what is left. Needs a minimum length for a cluster-to-cluster edge, which is a change to the shared layout."]
fn a_relationship_between_two_frames_has_room_for_its_marks() {
    if !text_metrics::fonts_available() {
        return;
    }
    let src = CASES
        .iter()
        .find(|(n, _)| *n == "subgraph-endpoint")
        .expect("the case is in the corpus")
        .1;
    let d = laid_out(src);
    for e in &d.edges {
        if d.cluster(&e.from).is_none() && d.cluster(&e.to).is_none() {
            continue;
        }
        let needed = e.tip_start.room() + e.tip_end.room() + edges::ER_FAR * 2.0;
        let len = edges::length(&e.points);
        assert!(
            len >= needed,
            "{} -> {} is {} px long but its two marks need {}",
            e.from,
            e.to,
            svg::num(len),
            svg::num(needed)
        );
    }
}

// ---------------------------------------------------------------------------------------------
// 6. Degrading rather than lying (§6)
// ---------------------------------------------------------------------------------------------

#[test]
fn another_diagram_kind_is_refused() {
    for src in [
        "flowchart TD\n  A --> B",
        "stateDiagram-v2\n  [*] --> A",
        "classDiagram\n  A --> B",
        "sequenceDiagram\n  A->>B: hi",
    ] {
        assert!(
            matches!(render(src, "dark"), Err(RenderError::ErParse(_))),
            "{src:?}"
        );
    }
}

/// **A bracket that is not an alias refuses the whole diagram**, which is how the fence ends up
/// showing the text diagram instead of a drawing that disagrees with it.
///
/// The parser's own statement of the rule is `er::tests::a_bracket_alias_is_only_a_declaration`;
/// what this adds is that the refusal survives the trip through `render`, since that is the entry
/// `preview::markdown::mermaid_to_svg` calls and the only one whose `None` reaches a reader.
#[test]
fn a_bracket_outside_a_declaration_is_refused() {
    for src in [
        "erDiagram\n  p[Person] ||--o| a[\"Customer Account\"] : has",
        "erDiagram\n  p[Person] ||--o| a : has",
        "erDiagram\n  A[one] B[two]",
        "erDiagram\n  A\n  B[unclosed",
        "erDiagram\n  subgraph a[b] c[d]\n    Z\n  end",
    ] {
        let e = render(src, "dark").unwrap_err();
        assert!(matches!(e, RenderError::ErParse(_)), "{src:?} gave {e}");
    }
}

#[test]
fn an_er_diagram_with_no_entities_is_refused() {
    let e = render("erDiagram", "dark").unwrap_err();
    assert!(matches!(e, RenderError::ErParse(_)));
    assert_eq!(e.to_string(), "ER diagram declares no entities");
}

#[test]
fn awkward_sources_produce_a_diagram_or_an_error_and_never_a_panic() {
    if !text_metrics::fonts_available() {
        return;
    }
    let mut wide = String::from("erDiagram\n");
    for i in 0..120 {
        wide.push_str(&format!("  n{i} ||--o{{ n{} : r\n", i + 1));
    }
    wide.push_str("  n0 ||--|| n120 : r\n");

    let mut tall = String::from("erDiagram\n  BIG {\n");
    for i in 0..200 {
        tall.push_str(&format!("    string field{i} PK \"comment {i}\"\n"));
    }
    tall.push_str("  }\n");

    let mut deep = String::from("erDiagram\n");
    for i in 0..12 {
        deep.push_str(&format!("subgraph s{i}\n"));
    }
    deep.push_str("A\n");
    for _ in 0..12 {
        deep.push_str("end\n");
    }

    let cases: &[&str] = &[
        "erDiagram\n  A",
        "erDiagram\n  A { }",
        "erDiagram\n  A ||--|| A : self",
        "erDiagram\n  A ||--|| B : \"\"",
        "erDiagram\n  A {\n    string \"\"\n  }",
        "erDiagram\n  A {\n    string x \"\"\n  }",
        "erDiagram\n  \"\" ||--|| B : x",
        "erDiagram\n  \"a\" ||--|| \"b\" : x",
        "erDiagram\n  一 ||--|| 二 : 🎉",
        "erDiagram\n  ❤ {\n    string 名前 PK \"日本語のコメント\"\n  }",
        "erDiagram\n  A u--|| B : x",
        "erDiagram\n  subgraph S\n  end\n  A",
        "erDiagram\n  subgraph S\n    A\n  end\n  S ||--|| A : x",
        "erDiagram\n  subgraph S\n    A\n  end\n  B ||--|| S : x",
        "erDiagram\n  A[\"\"] {\n    string x\n  }",
        "erDiagram\n  direction LR\n  A ||--|| B : x\n  B ||--|| A : y",
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
                "{name}/{theme_name}"
            );
            let opaque_background = svg
                .lines()
                .any(|l| l.starts_with("<rect width=") && !l.contains("fill=\"none\""));
            assert!(!opaque_background, "{name}/{theme_name}: opaque background");
        }
    }
}

// ---------------------------------------------------------------------------------------------
// 7. Goldens
// ---------------------------------------------------------------------------------------------

/// The regression net over the ER corpus, through [`render`].
#[test]
fn er_corpus_golden() {
    if !text_metrics::fonts_available() {
        return;
    }
    let mut out = String::new();
    for (name, src) in CASES {
        out.push_str(&format!("=== {name} ===\n"));
        out.push_str(&mask_numbers(&render(src, "dark").expect("renders")));
        out.push('\n');
    }
    assert_snapshot("mermaid_er", &out);
}

/// **A diagram assembled by hand, so no font is involved and every number is pinned exactly**:
/// both of stage 3's glyphs with their panels, every one of its new line terminators, and a
/// cardinality label — in all five palettes.
///
/// This is the golden that would catch a changed constant, which the masked corpus goldens
/// deliberately cannot see.
#[test]
fn panel_emit_golden() {
    let d = synthetic_panel_diagram();
    let mut out = String::new();
    for t in super::theme::ALL {
        out.push_str(&format!("=== {} ===\n", t.name));
        out.push_str(&svg::emit(&d, t));
        out.push('\n');
    }
    assert_snapshot("mermaid_panel_emit", &out);
}

/// One class box, one entity box, and one edge per terminator, at coordinates chosen by hand.
fn synthetic_panel_diagram() -> Diagram {
    let label = |text: &str, w: f64| Label {
        lines: text.split('\n').map(str::to_string).collect(),
        width: w,
        height: text.split('\n').count() as f64 * labels::line_height(),
    };

    let class = panel::class_panel(&[
        panel::Compartment {
            lines: vec![label("«interface»", 70.0), label("Shape", 40.0)],
            centered: true,
        },
        panel::Compartment {
            lines: vec![label("+int sides", 66.0), label("-String name", 84.0)],
            centered: false,
        },
        panel::Compartment {
            lines: vec![label("+draw() : bool", 92.0)],
            centered: false,
        },
    ]);
    let entity = panel::er_panel(
        &label("PERSON", 55.0),
        &[
            panel::AttributeRow {
                kind: label("string", 40.0),
                name: label("driversLicense", 92.0),
                keys: label("PK", 18.0),
                comment: label("The license #", 84.0),
            },
            panel::AttributeRow {
                kind: label("int", 18.0),
                name: label("age", 24.0),
                keys: label("", 0.0),
                comment: label("", 0.0),
            },
        ],
    );

    let mut nodes = vec![
        PlacedNode {
            id: "Shape".to_string(),
            shape: Glyph::ClassBox,
            center: Point::new(120.0, 80.0),
            size: class.size,
            label: label("", 0.0),
            panel: Some(class),
            series: None,
            mark: None,
        },
        PlacedNode {
            id: "PERSON".to_string(),
            shape: Glyph::ErBox,
            center: Point::new(420.0, 80.0),
            size: entity.size,
            label: label("", 0.0),
            panel: Some(entity),
            series: None,
            mark: None,
        },
    ];
    // Two plain boxes for the terminator strip to point at.
    for (i, id) in ["left", "right"].into_iter().enumerate() {
        nodes.push(PlacedNode {
            id: id.to_string(),
            shape: Glyph::Flow(Shape::Rect),
            center: Point::new(60.0 + i as f64 * 520.0, 200.0),
            size: Size::new(60.0, 30.0),
            label: label(id, 30.0),
            panel: None,
            series: None,
            mark: None,
        });
    }

    let tips = [
        Tip::HollowTriangle,
        Tip::FilledDiamond,
        Tip::HollowDiamond,
        Tip::Lollipop,
        Tip::ErOnlyOne,
        Tip::ErZeroOrOne,
        Tip::ErOneOrMore,
        Tip::ErZeroOrMore,
    ];
    let mut edges = Vec::new();
    for (i, tip) in tips.into_iter().enumerate() {
        let y = 240.0 + i as f64 * 34.0;
        let points = vec![Point::new(60.0, y), Point::new(580.0, y)];
        edges.push(PlacedEdge {
            from: "left".to_string(),
            to: "right".to_string(),
            points,
            tip_start: tip,
            tip_end: tip,
            stroke: if i % 2 == 0 {
                Stroke::Normal
            } else {
                Stroke::Dotted
            },
            label: None,
            start_label: Some(PlacedEdgeLabel {
                center: Point::new(90.0, y - 12.0),
                size: Size::new(20.0 + super::LABEL_PAD_X * 2.0, 20.0),
                label: label("1", 20.0),
            }),
            end_label: Some(PlacedEdgeLabel {
                center: Point::new(550.0, y - 12.0),
                size: Size::new(30.0 + super::LABEL_PAD_X * 2.0, 20.0),
                label: label("0..*", 30.0),
            }),
            badge: None,
            series: None,
            straight: false,
            overlay: false,
        });
    }

    let clusters = vec![PlacedCluster {
        id: "ns".to_string(),
        title: label("A namespace", 110.0),
        center: Point::new(320.0, 560.0),
        size: Size::new(500.0, 140.0),
        parent: None,
        depth: 0,
        dashed: false,
        filled: true,
        sections: Vec::new(),
    }];

    Diagram {
        width: 660.0,
        height: 660.0,
        nodes,
        edges,
        clusters,
        lifelines: Vec::new(),
    }
}

// ---------------------------------------------------------------------------------------------
// 8. Looking at it
// ---------------------------------------------------------------------------------------------

/// `KONOMA_GALLERY=/some/dir cargo test -- --ignored er_tests::gallery`
#[test]
#[ignore = "writes SVG files for a person to look at: KONOMA_GALLERY=<dir> cargo test -- --ignored gallery"]
fn gallery() {
    let dir = std::env::var("KONOMA_GALLERY").expect("set KONOMA_GALLERY to a directory");
    std::fs::create_dir_all(&dir).expect("create the gallery directory");
    for (name, src) in CASES {
        let svg = render(src, "dark").unwrap_or_else(|e| panic!("{name}: {e}"));
        std::fs::write(format!("{dir}/er-{name}.svg"), svg).expect("write the SVG");
    }
    // Plus the hand-built one, which is where every terminator appears at once.
    std::fs::write(
        format!("{dir}/er-terminators.svg"),
        svg::emit(&synthetic_panel_diagram(), &super::theme::DARK),
    )
    .expect("write the SVG");
}

/// An entity box uses the plain-rectangle intersection, like every other box konoma draws.
#[test]
fn an_entity_box_clips_against_its_rectangle() {
    let size = Size::new(100.0, 50.0);
    let center = Point::new(0.0, 0.0);
    let p = shapes::intersect(Glyph::ErBox, center.clone(), size, &Point::new(200.0, 0.0));
    assert!((p.x - 50.0).abs() < 1e-9 && p.y.abs() < 1e-9, "{p:?}");
}
