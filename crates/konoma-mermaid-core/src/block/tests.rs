//! What the block reader is checked with.
//!
//! The cases come from `block.md`'s own examples — thirty of them — because this is the one
//! language whose documentation *is* its specification for the grid.

use super::*;

fn ok(src: &str) -> BlockDiagram {
    parse(src).unwrap_or_else(|e| panic!("{e}\n{src}"))
}

fn ids(items: &[Item]) -> Vec<&str> {
    items.iter().filter_map(Item::id).collect()
}

/// The grammar's whitespace rule: `a b c` on one line is three blocks.
#[test]
fn whitespace_is_not_a_statement_separator() {
    let one = ok("block-beta\n  a b c\n");
    let three = ok("block-beta\n  a\n  b\n  c\n");
    assert_eq!(ids(&one.items), ["a", "b", "c"]);
    assert_eq!(one.items, three.items);
}

#[test]
fn columns_and_widths_are_read() {
    let d = ok("block-beta\n  columns 3\n  a[\"A label\"] b:2 c:2 d\n");
    assert_eq!(d.columns, 3);
    assert_eq!(d.items[0].width(), 1);
    assert_eq!(d.items[1].width(), 2);
    assert_eq!(d.items[2].width(), 2);
    assert_eq!(d.items[3].width(), 1);
    let Item::Node(a) = &d.items[0] else {
        panic!("a block")
    };
    assert_eq!(a.label, "A label");
}

#[test]
fn columns_auto_is_minus_one_and_is_the_default() {
    assert_eq!(ok("block-beta\n  columns auto\n  a b\n").columns, AUTO);
    assert_eq!(ok("block-beta\n  a b\n").columns, AUTO);
}

#[test]
fn space_takes_the_cells_it_names() {
    let d = ok("block-beta\n  columns 3\n  a space b\n  c   d   e\n");
    assert!(matches!(d.items[1], Item::Space { width: 1 }));
    let d = ok("block-beta\n  ida space:3 idb idc\n");
    assert!(matches!(d.items[1], Item::Space { width: 3 }));
    // …and `spaceship` is a block, not a space.
    let d = ok("block-beta\n  spaceship\n");
    assert_eq!(ids(&d.items), ["spaceship"]);
}

/// Every shape `block.md` documents, in the spelling it documents.
#[test]
fn every_documented_shape_is_read_as_that_shape() {
    for (src, want) in [
        ("id1(\"t\")", Shape::RoundedRect),
        ("id1([\"t\"])", Shape::Stadium),
        ("id1[[\"t\"]]", Shape::Subroutine),
        ("id1[(\"t\")]", Shape::Cylinder),
        ("id1((\"t\"))", Shape::Circle),
        ("id1>\"t\"]", Shape::Odd),
        ("id1{\"t\"}", Shape::Diamond),
        ("id1{{\"t\"}}", Shape::Hexagon),
        ("id1[/\"t\"/]", Shape::LeanRight),
        ("id1[\\\"t\"\\]", Shape::LeanLeft),
        ("id1[/\"t\"\\]", Shape::Trapezoid),
        ("id1[\\\"t\"/]", Shape::InvTrapezoid),
        ("id1(((\"t\")))", Shape::DoubleCircle),
        ("id1[\"t\"]", Shape::Rect),
    ] {
        let d = ok(&format!("block-beta\n  {src}\n"));
        let Item::Node(n) = &d.items[0] else {
            panic!("a block: {src}")
        };
        assert_eq!(n.shape, want, "{src}");
        assert_eq!(n.label, "t", "{src}");
    }
}

#[test]
fn a_block_with_no_shape_is_labelled_with_its_own_id() {
    let d = ok("block-beta\n  alpha\n");
    let Item::Node(n) = &d.items[0] else {
        panic!("a block")
    };
    assert_eq!(n.label, "alpha");
    assert_eq!(n.shape, Shape::Rect);
}

#[test]
fn a_named_composite_holds_its_children_and_keeps_its_own_columns() {
    let d = ok("block-beta\n  columns 3\n  a:3\n  block:group1:2\n    columns 2\n    h i j k\n  end\n  g\n");
    assert_eq!(d.columns, 3);
    let Item::Composite(c) = &d.items[1] else {
        panic!("a composite")
    };
    assert_eq!(c.id, "group1");
    assert!(c.named);
    assert_eq!(c.width, 2);
    assert_eq!(c.columns, 2);
    assert_eq!(ids(&c.children), ["h", "i", "j", "k"]);
    assert_eq!(ids(&d.items), ["a", "group1", "g"]);
}

#[test]
fn an_anonymous_composite_gets_an_id_of_its_own() {
    let d = ok("block-beta\n    block\n      D\n    end\n    A[\"A: I am a wide one\"]\n");
    let Item::Composite(c) = &d.items[0] else {
        panic!("a composite")
    };
    assert!(!c.named);
    assert_eq!(ids(&c.children), ["D"]);
    assert_eq!(ids(&d.items)[1], "A");
}

/// **The defect this language is measured against.** `docs/FEATURE-MERMAID-RENDERER.md` §6-A
/// records the crate drawing `a --> e` as a stub between `a` and `b`. Here the link names the
/// two blocks it was written between, whatever is on the grid in between.
#[test]
fn a_link_joins_the_two_blocks_it_names_and_no_others() {
    let d = ok("block-beta\n  columns 5\n  a b c d e\n  a --> e\n");
    assert_eq!(ids(&d.items), ["a", "b", "c", "d", "e"]);
    assert_eq!(d.edges.len(), 1);
    assert_eq!(d.edges[0].from, "a");
    assert_eq!(d.edges[0].to, "e");
}

#[test]
fn a_link_may_carry_a_label() {
    let d = ok("block-beta\n  A space:2 B\n  A-- \"X\" -->B\n");
    assert_eq!(d.edges.len(), 1);
    assert_eq!(d.edges[0].label.as_deref(), Some("X"));
    assert_eq!(d.edges[0].from, "A");
    assert_eq!(d.edges[0].to, "B");
}

#[test]
fn every_documented_link_style_is_read() {
    for (src, arrow, stroke) in [
        ("a --> b", Arrow::Point, Stroke::Normal),
        ("a --- b", Arrow::None, Stroke::Normal),
        ("a ==> b", Arrow::Point, Stroke::Thick),
        ("a -.-> b", Arrow::Point, Stroke::Dotted),
        ("a ~~~ b", Arrow::None, Stroke::Invisible),
        ("a --o b", Arrow::Circle, Stroke::Normal),
        ("a --x b", Arrow::Cross, Stroke::Normal),
        ("a <--> b", Arrow::DoublePoint, Stroke::Normal),
    ] {
        let d = ok(&format!("block-beta\n  {src}\n"));
        assert_eq!(d.edges.len(), 1, "{src}");
        assert_eq!(d.edges[0].arrow, arrow, "{src}");
        assert_eq!(d.edges[0].stroke, stroke, "{src}");
    }
}

#[test]
fn a_link_that_names_a_composite_names_the_composite() {
    let d = ok("block-beta\ncolumns 1\n  db((\"DB\"))\n  block:ID\n    A\n    B[\"A wide one in the middle\"]\n    C\n  end\n  space\n  D\n  ID --> D\n  C --> D\n");
    assert_eq!(d.edges.len(), 2);
    assert_eq!(d.edges[0].from, "ID");
    assert_eq!(d.edges[0].to, "D");
    assert_eq!(d.edges[1].from, "C");
    assert_eq!(d.edges[1].to, "D");
}

#[test]
fn every_documented_block_arrow_direction_is_read() {
    let d = ok("block-beta\n  blockArrowId<[\"Label\"]>(right)\n  blockArrowId2<[\"Label\"]>(left)\n  blockArrowId3<[\"Label\"]>(up)\n  blockArrowId4<[\"Label\"]>(down)\n  blockArrowId5<[\"Label\"]>(x)\n  blockArrowId6<[\"Label\"]>(y)\n  blockArrowId7<[\"Label\"]>(x, down)\n");
    let dirs: Vec<Vec<Dir>> = d
        .items
        .iter()
        .map(|i| match i {
            Item::Node(n) => n.arrow.clone().unwrap_or_default(),
            _ => Vec::new(),
        })
        .collect();
    assert_eq!(
        dirs,
        vec![
            vec![Dir::Right],
            vec![Dir::Left],
            vec![Dir::Up],
            vec![Dir::Down],
            vec![Dir::X],
            vec![Dir::Y],
            vec![Dir::X, Dir::Down],
        ]
    );
}

#[test]
fn an_unknown_block_arrow_direction_is_refused() {
    assert!(matches!(
        parse("block-beta\n  a<[\"L\"]>(sideways)\n"),
        Err(ParseError::Invalid { .. })
    ));
}

#[test]
fn the_appearance_statements_are_read_and_do_not_become_blocks() {
    let d = ok("block-beta\n  id1 space id2\n  id1(\"Start\")-->id2(\"Stop\")\n  style id1 fill:#636,stroke:#333,stroke-width:4px\n  style id2 fill:#bbf\n  classDef hot fill:#f00\n  class id1 hot\n");
    assert_eq!(ids(&d.items), ["id1", "id2"]);
    assert_eq!(d.edges.len(), 1);
}

#[test]
fn a_composite_that_is_never_closed_is_refused() {
    assert!(matches!(
        parse("block-beta\n  block:ID\n    a\n"),
        Err(ParseError::Invalid { .. })
    ));
}

#[test]
fn a_stray_end_is_refused() {
    assert!(matches!(
        parse("block-beta\n  a\n  end\n"),
        Err(ParseError::Unexpected { .. })
    ));
}

#[test]
fn a_header_with_no_blocks_is_refused() {
    assert!(matches!(
        parse("block-beta\n"),
        Err(ParseError::NoData { .. })
    ));
}

#[test]
fn the_routing_predicate_and_the_parser_agree() {
    for src in [
        "block-beta\n  a b c",
        "block\n  a b c",
        "BLOCK-BETA\n  a",
        "block-beta",
        "blocks\n  a",
        "gantt\n  title G",
        "",
        "%% only a comment\n",
    ] {
        let refused = matches!(parse(src), Err(ParseError::NotThisChart { .. }));
        assert_eq!(is_block_diagram(src), !refused, "{src:?}");
    }
}
