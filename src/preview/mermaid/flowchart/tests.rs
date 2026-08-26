//! The corpus for the flowchart parser.
//!
//! Built from the **specification** — mermaid's `flow.jison` and `flowDb.ts`, distilled into
//! `docs/FEATURE-MERMAID-RENDERER.md` §2 — and not from a list of bugs. A corpus grown from
//! bugs only ever prevents the bugs it grew from (memory `corpus-from-spec-not-from-bugs`), and
//! this parser has no bug history yet, only a grammar.
//!
//! The axes are the case splits of that grammar: five directions x fourteen classic shapes x ten
//! link alphabets x two label notations x chains x `&` expansion x subgraphs in five arrangements
//! x quoted / CJK / emoji labels x four `<br>` spellings x entities x the thirteen statements
//! that must be recognised and dropped x `@{ … }` x edge ids x rejected headers x broken input.
//!
//! Two properties are asserted everywhere rather than in one place, because they are the two
//! ways this parser could be quietly wrong:
//!
//! * **no phantom nodes** — the exact failure §2-3 is about, so `node_ids()` is compared as a
//!   whole list rather than probed for the nodes a test happens to care about, and
//! * **no panic** — konoma's design principle #3. The sweep at the bottom is deliberately
//!   hostile.

use super::model::{Arrow, Direction, LinkStyleTarget, ParseError, Shape, Stroke};
use super::parser::parse;
use super::preprocess::preprocess;
use super::shapes::{parse_shape_data, resolve_shape, SHAPE_NAMES};
use super::text::decode_label;
use super::Flowchart;

/// Parses, or fails the test with the parser's own message.
fn ok(src: &str) -> Flowchart {
    match parse(src) {
        Ok(c) => c,
        Err(e) => panic!("expected a flowchart, got: {e}\n---\n{src}\n---"),
    }
}

fn err(src: &str) -> ParseError {
    match parse(src) {
        Ok(c) => panic!(
            "expected an error, got {} nodes / {} edges\n---\n{src}\n---",
            c.nodes.len(),
            c.edges.len()
        ),
        Err(e) => e,
    }
}

fn labels(c: &Flowchart) -> Vec<&str> {
    c.nodes.iter().map(|n| n.label.as_str()).collect()
}

fn edge_pairs(c: &Flowchart) -> Vec<(&str, &str)> {
    c.edges
        .iter()
        .map(|e| (e.from.as_str(), e.to.as_str()))
        .collect()
}

// =============================================================================================
// Header: keyword, direction, and the two failures that must not draw
// =============================================================================================

#[test]
fn both_diagram_keywords_are_accepted() {
    // `graph` outnumbers `flowchart` 52 to 16 in the corpus behind §2-1; dropping it would lose
    // most AI-written diagrams.
    for kw in ["graph", "flowchart", "flowchart-elk"] {
        let c = ok(&format!("{kw} TD\n  A --> B\n"));
        assert_eq!(c.node_ids(), ["A", "B"], "{kw}");
        assert_eq!(c.direction, Direction::TopToBottom, "{kw}");
    }
}

#[test]
fn all_five_direction_spellings_parse_and_td_folds_into_tb() {
    let cases = [
        ("TB", Direction::TopToBottom),
        ("TD", Direction::TopToBottom),
        ("BT", Direction::BottomToTop),
        ("LR", Direction::LeftToRight),
        ("RL", Direction::RightToLeft),
    ];
    for (spelling, want) in cases {
        let c = ok(&format!("flowchart {spelling}\n  A --> B\n"));
        assert_eq!(c.direction, want, "{spelling}");
    }
    assert_eq!(Direction::TopToBottom.as_str(), "TB");
}

#[test]
fn a_missing_direction_means_top_to_bottom() {
    let c = ok("graph\n  A --> B\n");
    assert_eq!(c.direction, Direction::TopToBottom);
}

#[test]
fn the_header_may_be_preceded_by_blank_lines_and_followed_by_a_semicolon() {
    let c = ok("\n\n   graph LR; A --> B;\n");
    assert_eq!(c.direction, Direction::LeftToRight);
    assert_eq!(c.node_ids(), ["A", "B"]);
    assert_eq!(c.edges.len(), 1);
}

#[test]
fn an_unknown_header_is_an_error_rather_than_an_invented_diagram() {
    // The shipping crate answers `venn-beta` with three boxes reading "venn", "beta", "sets"
    // (§6). Anything that is not a flowchart must refuse to draw.
    for src in [
        "venn-beta\n  sets: [a, b]\n",
        "sequenceDiagram\n  A ->> B: hi\n",
        "erDiagram\n  A ||--o{ B : has\n",
        "pie title X\n  \"a\" : 10\n",
        "classDiagram\n  A <|-- B\n",
        "stateDiagram-v2\n  [*] --> A\n",
        "gantt\n  title X\n",
        "mindmap\n  root\n",
        "swimlane-beta\n  A --> B\n",
        "graphs are nice\n",
    ] {
        assert!(
            matches!(parse(src), Err(ParseError::NotAFlowchart { .. })),
            "should not have parsed: {src:?}"
        );
    }
}

#[test]
fn an_empty_source_is_an_error() {
    for src in ["", "   ", "\n\n\n", "%% only a comment\n"] {
        assert_eq!(err(src), ParseError::Empty, "{src:?}");
    }
}

#[test]
fn a_flowchart_with_no_nodes_is_an_error() {
    // The shipping crate answers a bare `graph TD` with a 16x16 transparent SVG that is then
    // blown up to the width of the pane (§6).
    for src in [
        "graph TD\n",
        "flowchart LR\n",
        "graph TD\n%% nothing but a comment\n",
        "graph TD\nclassDef hot fill:#f9f\n",
        "graph TD\naccTitle: only a title\n",
    ] {
        assert_eq!(err(src), ParseError::NoNodes, "{src:?}");
    }
}

// =============================================================================================
// Node shapes
// =============================================================================================

#[test]
fn every_classic_shape_notation_is_recognised() {
    // §2-6 rule 3: the shape is decided by the *closing* bracket, which is why the last four
    // rows share two opening brackets between them.
    let cases = [
        ("A[text]", Shape::Rect),
        ("A(text)", Shape::RoundedRect),
        ("A([text])", Shape::Stadium),
        ("A[[text]]", Shape::Subroutine),
        ("A[(text)]", Shape::Cylinder),
        ("A((text))", Shape::Circle),
        ("A(((text)))", Shape::DoubleCircle),
        ("A{text}", Shape::Diamond),
        ("A{{text}}", Shape::Hexagon),
        ("A>text]", Shape::Odd),
        ("A[/text/]", Shape::LeanRight),
        ("A[\\text\\]", Shape::LeanLeft),
        ("A[/text\\]", Shape::Trapezoid),
        ("A[\\text/]", Shape::InvTrapezoid),
    ];
    for (src, want) in cases {
        let c = ok(&format!("flowchart TD\n  {src}\n"));
        assert_eq!(c.node_ids(), ["A"], "{src}");
        assert_eq!(c.nodes[0].shape, want, "{src}");
        assert_eq!(c.nodes[0].label, "text", "{src}");
    }
}

#[test]
fn a_node_that_is_only_mentioned_is_a_rectangle_labelled_with_its_id() {
    let c = ok("flowchart TD\n  A --> B\n");
    assert_eq!(labels(&c), ["A", "B"]);
    assert!(c.nodes.iter().all(|n| n.shape == Shape::Rect));
}

#[test]
fn a_later_declaration_gives_a_node_its_label_and_a_bare_mention_does_not_take_it_away() {
    let c = ok("flowchart TD\n  A --> B\n  A[Real]\n  A\n");
    assert_eq!(c.node("A").unwrap().label, "Real");
    // …and a shape survives a later bare mention too.
    let c = ok("flowchart TD\n  A{Q}\n  A --> B\n");
    assert_eq!(c.node("A").unwrap().shape, Shape::Diamond);
}

#[test]
fn brackets_and_quotes_nest_inside_a_label() {
    let c = ok("flowchart TD\n  A[outer [inner] tail]\n");
    assert_eq!(c.nodes[0].label, "outer [inner] tail");
    // A quoted label may hold the very bracket that would otherwise close it.
    let c = ok("flowchart TD\n  A[\"a ] b\"] --> B\n");
    assert_eq!(c.nodes[0].label, "a ] b");
    assert_eq!(c.node_ids(), ["A", "B"]);
}

#[test]
fn a_label_may_span_source_lines_because_bracket_text_is_its_own_lexer_state() {
    let c = ok("flowchart TD\n  A[one\ntwo] --> B\n");
    assert_eq!(c.nodes[0].label, "one\ntwo");
    assert_eq!(c.node_ids(), ["A", "B"]);
}

// =============================================================================================
// Links
// =============================================================================================

#[test]
fn every_link_alphabet_parses_with_the_right_arrow_and_stroke() {
    let cases = [
        ("-->", Arrow::Point, Stroke::Normal),
        ("---", Arrow::None, Stroke::Normal),
        ("--o", Arrow::Circle, Stroke::Normal),
        ("--x", Arrow::Cross, Stroke::Normal),
        ("-.->", Arrow::Point, Stroke::Dotted),
        ("-.-", Arrow::None, Stroke::Dotted),
        ("==>", Arrow::Point, Stroke::Thick),
        ("===", Arrow::None, Stroke::Thick),
        ("<-->", Arrow::DoublePoint, Stroke::Normal),
        ("o--o", Arrow::DoubleCircle, Stroke::Normal),
        ("x--x", Arrow::DoubleCross, Stroke::Normal),
        ("~~~", Arrow::None, Stroke::Invisible),
        ("<==>", Arrow::DoublePoint, Stroke::Thick),
        ("o-.-o", Arrow::DoubleCircle, Stroke::Dotted),
    ];
    for (link, arrow, stroke) in cases {
        let c = ok(&format!("flowchart LR\n  A {link} B\n"));
        assert_eq!(c.edges.len(), 1, "{link}");
        assert_eq!(c.edges[0].arrow, arrow, "{link}");
        assert_eq!(c.edges[0].stroke, stroke, "{link}");
        assert_eq!(edge_pairs(&c), [("A", "B")], "{link}");
    }
}

#[test]
fn link_length_is_the_dash_count_except_for_dotted_links_where_it_is_the_dot_count() {
    // §2-6 rule 5. Getting this wrong changes how many ranks the layout spans, so it moves every
    // node below the link.
    let cases = [
        ("-->", 1),
        ("--->", 2),
        ("---->", 3),
        ("---", 1),
        ("----", 2),
        ("==>", 1),
        ("===>", 2),
        ("-.->", 1),
        ("-..->", 2),
        ("-...->", 3),
        ("~~~", 1),
        ("~~~~", 2),
    ];
    for (link, want) in cases {
        let c = ok(&format!("flowchart LR\n  A {link} B\n"));
        assert_eq!(c.edges[0].length, want, "{link}");
    }
}

#[test]
fn link_length_is_clamped_at_ten() {
    let c = ok("flowchart LR\n  A ---------------> B\n");
    assert_eq!(c.edges[0].length, 10);
}

#[test]
fn both_label_notations_produce_the_same_edge() {
    let piped = ok("flowchart LR\n  A -->|yes| B\n");
    let infix = ok("flowchart LR\n  A -- yes --> B\n");
    for c in [&piped, &infix] {
        assert_eq!(c.edges[0].label.as_deref(), Some("yes"));
        assert_eq!(c.edges[0].arrow, Arrow::Point);
        assert_eq!(c.edges[0].stroke, Stroke::Normal);
        assert_eq!(c.node_ids(), ["A", "B"]);
    }
}

#[test]
fn the_infix_notation_works_in_every_stroke_it_exists_in() {
    let cases = [
        ("A -- t --> B", Arrow::Point, Stroke::Normal),
        ("A -- t --- B", Arrow::None, Stroke::Normal),
        ("A == t ==> B", Arrow::Point, Stroke::Thick),
        ("A == t === B", Arrow::None, Stroke::Thick),
        ("A -. t .-> B", Arrow::Point, Stroke::Dotted),
        ("A -. t .- B", Arrow::None, Stroke::Dotted),
        ("A x-- t --x B", Arrow::DoubleCross, Stroke::Normal),
        ("A o-- t --o B", Arrow::DoubleCircle, Stroke::Normal),
        ("A <-- t --> B", Arrow::DoublePoint, Stroke::Normal),
    ];
    for (src, arrow, stroke) in cases {
        let c = ok(&format!("flowchart LR\n  {src}\n"));
        assert_eq!(c.edges.len(), 1, "{src}");
        assert_eq!(c.edges[0].label.as_deref(), Some("t"), "{src}");
        assert_eq!(c.edges[0].arrow, arrow, "{src}");
        assert_eq!(c.edges[0].stroke, stroke, "{src}");
    }
}

#[test]
fn halves_of_an_infix_link_that_disagree_are_invalid_rather_than_guessed() {
    // §2-6 rule 6: mermaid returns `INVALID` for both the type and the stroke. The edge still
    // exists — the author asked for a connection — but nothing pretends to know its arrow.
    for src in ["A x-- t --> B", "A o-- t --x B", "A <-- t --o B"] {
        let c = ok(&format!("flowchart LR\n  {src}\n"));
        assert_eq!(c.edges[0].arrow, Arrow::Invalid, "{src}");
        assert_eq!(c.edges[0].stroke, Stroke::Invalid, "{src}");
    }
    // Disagreeing on the stroke alone is equally invalid.
    let c = ok("flowchart LR\n  A -. t .-> B\n  A -. t .- B\n");
    assert_eq!(c.edges.len(), 2);
}

#[test]
fn an_edge_label_may_be_quoted_and_may_hold_the_delimiter_it_would_otherwise_end_at() {
    let c = ok("flowchart LR\n  A -->|\"a|b\"| B\n");
    assert_eq!(c.edges[0].label.as_deref(), Some("a|b"));
    let c = ok("flowchart LR\n  A -- \"a--b\" --> B\n");
    assert_eq!(c.edges[0].label.as_deref(), Some("a--b"));
}

#[test]
fn a_link_with_no_label_reports_none_rather_than_an_empty_string() {
    let c = ok("flowchart LR\n  A --> B\n");
    assert_eq!(c.edges[0].label, None);
}

#[test]
fn three_dashes_followed_by_o_or_x_read_as_a_circle_or_cross_arrow() {
    // A documented mermaid trap (§2-6 rule 8) that is kept, not fixed: the author saw this
    // behaviour when they wrote the file, so reading it any other way would change their diagram.
    let c = ok("flowchart LR\n  A ---oB\n");
    assert_eq!(c.edges[0].arrow, Arrow::Circle);
    assert_eq!(edge_pairs(&c), [("A", "B")]);
    let c = ok("flowchart LR\n  A ---xB\n");
    assert_eq!(c.edges[0].arrow, Arrow::Cross);
    // With a space it is an ordinary `---` to a node whose id starts with `o`.
    let c = ok("flowchart LR\n  A --- oB\n");
    assert_eq!(c.edges[0].arrow, Arrow::None);
    assert_eq!(edge_pairs(&c), [("A", "oB")]);
}

// =============================================================================================
// Chains and `&` expansion
// =============================================================================================

#[test]
fn a_chain_links_each_neighbouring_pair() {
    let c = ok("flowchart LR\n  A --> B --> C --> D\n");
    assert_eq!(c.node_ids(), ["A", "B", "C", "D"]);
    assert_eq!(edge_pairs(&c), [("A", "B"), ("B", "C"), ("C", "D")]);
}

#[test]
fn a_chain_may_change_link_style_at_every_step() {
    let c = ok("flowchart LR\n  A --> B -.-> C ==> D --- E\n");
    let strokes: Vec<Stroke> = c.edges.iter().map(|e| e.stroke).collect();
    assert_eq!(
        strokes,
        [
            Stroke::Normal,
            Stroke::Dotted,
            Stroke::Thick,
            Stroke::Normal
        ]
    );
}

#[test]
fn ampersand_groups_multiply_out() {
    let c = ok("flowchart LR\n  A & B --> C & D\n");
    assert_eq!(c.node_ids(), ["A", "B", "C", "D"]);
    assert_eq!(
        edge_pairs(&c),
        [("A", "C"), ("A", "D"), ("B", "C"), ("B", "D")]
    );
}

#[test]
fn an_ampersand_without_spaces_is_part_of_the_id() {
    // mermaid's NODE_STRING includes `&`, and its longest-match rule therefore reads `A&B` as one
    // id. `A & B` is two nodes because the spaces break the token.
    let c = ok("flowchart LR\n  A&B --> C\n");
    assert_eq!(c.node_ids(), ["A&B", "C"]);
}

#[test]
fn ampersand_groups_combine_with_shapes_and_chains() {
    let c = ok("flowchart LR\n  A[a] & B((b)) --> C{c} --> D & E\n");
    assert_eq!(c.node_ids(), ["A", "B", "C", "D", "E"]);
    assert_eq!(
        edge_pairs(&c),
        [("A", "C"), ("B", "C"), ("C", "D"), ("C", "E")]
    );
    assert_eq!(c.node("B").unwrap().shape, Shape::Circle);
}

// =============================================================================================
// Subgraphs
// =============================================================================================

#[test]
fn a_named_subgraph_keeps_its_id_and_title() {
    let c = ok("flowchart TB\n  subgraph one\n    a --> b\n  end\n");
    assert_eq!(c.subgraphs.len(), 1);
    let sg = &c.subgraphs[0];
    assert_eq!(sg.id, "one");
    assert_eq!(sg.title, "one");
    assert!(!sg.auto_id);
    assert_eq!(sg.members, ["a", "b"]);
}

#[test]
fn an_explicit_id_with_a_bracketed_title_keeps_both() {
    let c = ok("flowchart TB\n  subgraph one [My Title]\n    a\n  end\n");
    let sg = &c.subgraphs[0];
    assert_eq!(sg.id, "one");
    assert_eq!(sg.title, "My Title");
    assert!(!sg.auto_id);
}

#[test]
fn a_title_with_spaces_and_no_id_gets_a_generated_id() {
    // mermaid throws the id away when the same text is both id and title and holds whitespace.
    let c = ok("flowchart TB\n  subgraph My Title\n    a\n  end\n");
    let sg = &c.subgraphs[0];
    assert_eq!(sg.id, "subGraph0");
    assert_eq!(sg.title, "My Title");
    assert!(sg.auto_id);
}

#[test]
fn an_untitled_subgraph_is_numbered_and_has_an_empty_title() {
    let c = ok("flowchart TB\n  subgraph\n    a\n  end\n");
    assert_eq!(c.subgraphs[0].id, "subGraph0");
    assert_eq!(c.subgraphs[0].title, "");
}

#[test]
fn the_generated_number_counts_every_block_not_just_the_unnamed_ones() {
    // mermaid bumps `subCount` for every subgraph, so a named block that closes first shifts the
    // numbering of the unnamed one after it.
    let c = ok("flowchart TB\n  subgraph one\n    a\n  end\n  subgraph Two Words\n    b\n  end\n");
    assert_eq!(c.subgraphs[0].id, "one");
    assert_eq!(c.subgraphs[1].id, "subGraph1");
}

#[test]
fn nested_subgraphs_register_inner_first_and_the_parent_names_the_child_by_id() {
    let c =
        ok("flowchart TB\n  subgraph outer\n    subgraph inner\n      a\n    end\n    b\n  end\n");
    assert_eq!(c.subgraphs.len(), 2);
    assert_eq!(c.subgraphs[0].id, "inner");
    assert_eq!(c.subgraphs[0].members, ["a"]);
    assert_eq!(c.subgraphs[1].id, "outer");
    // `a` belongs to `inner` and must not appear again in `outer` (mermaid's `makeUniq`).
    assert_eq!(c.subgraphs[1].members, ["inner", "b"]);
}

#[test]
fn three_levels_of_nesting_stay_a_tree() {
    let src = "flowchart TB\n\
               subgraph l1\n\
                 subgraph l2\n\
                   subgraph l3\n\
                     a\n\
                   end\n\
                   b\n\
                 end\n\
                 c\n\
               end\n";
    let c = ok(src);
    assert_eq!(
        c.subgraphs
            .iter()
            .map(|s| s.id.as_str())
            .collect::<Vec<_>>(),
        ["l3", "l2", "l1"]
    );
    assert_eq!(c.subgraph("l3").unwrap().members, ["a"]);
    assert_eq!(c.subgraph("l2").unwrap().members, ["l3", "b"]);
    assert_eq!(c.subgraph("l1").unwrap().members, ["l2", "c"]);
}

#[test]
fn a_subgraph_may_override_the_chart_direction() {
    let c = ok("flowchart TB\n  subgraph one\n    direction LR\n    a --> b\n  end\n");
    assert_eq!(c.direction, Direction::TopToBottom);
    assert_eq!(c.subgraphs[0].direction, Some(Direction::LeftToRight));
    assert_eq!(c.subgraphs[0].members, ["a", "b"]);
}

#[test]
fn a_top_level_direction_statement_is_ignored_as_mermaid_ignores_it() {
    // mermaid pushes it onto the top-level document array, which nothing reads. Honouring it
    // would silently turn every such diagram sideways compared with what its author saw.
    let c = ok("flowchart TB\n  direction LR\n  A --> B\n");
    assert_eq!(c.direction, Direction::TopToBottom);
    assert_eq!(c.node_ids(), ["A", "B"]);
}

#[test]
fn a_subgraph_can_be_an_edge_endpoint_without_becoming_a_box() {
    let c = ok("flowchart TB\n  subgraph one\n    a\n  end\n  one --> b\n");
    assert_eq!(c.node_ids(), ["a", "b"]);
    assert_eq!(edge_pairs(&c), [("one", "b")]);
    assert_eq!(c.subgraphs[0].id, "one");
}

#[test]
fn a_subgraph_used_as_an_endpoint_before_it_is_declared_still_does_not_become_a_box() {
    let c = ok("flowchart TB\n  b --> one\n  subgraph one\n    a\n  end\n");
    assert_eq!(c.node_ids(), ["b", "a"]);
    assert_eq!(edge_pairs(&c), [("b", "one")]);
}

#[test]
fn an_unclosed_subgraph_is_closed_at_the_end_rather_than_rejected() {
    let c = ok("flowchart TB\n  subgraph one\n    a --> b\n");
    assert_eq!(c.subgraphs.len(), 1);
    assert_eq!(c.subgraphs[0].members, ["a", "b"]);
}

#[test]
fn a_stray_end_is_ignored() {
    let c = ok("flowchart TB\n  end\n  end\n  A --> B\n  end\n");
    assert_eq!(c.node_ids(), ["A", "B"]);
    assert!(c.subgraphs.is_empty());
}

#[test]
fn a_quoted_subgraph_title_is_unquoted() {
    let c = ok("flowchart TB\n  subgraph \"Some Title\"\n    a\n  end\n");
    assert_eq!(c.subgraphs[0].title, "Some Title");
    assert!(c.subgraphs[0].auto_id);
}

// =============================================================================================
// Label text: quotes, CJK, emoji, line breaks, entities
// =============================================================================================

#[test]
fn a_quoted_label_loses_its_quotes() {
    let c = ok("flowchart TD\n  A[\"hello, world\"] --> B\n");
    assert_eq!(c.nodes[0].label, "hello, world");
}

#[test]
fn cjk_punctuation_needs_no_quotes_because_bracket_text_is_its_own_state() {
    // §2-6 rule 4. This is the shape most Japanese diagrams take.
    let c = ok("flowchart TD\n  A[開始、処理。「確認」] --> B[終了]\n");
    assert_eq!(labels(&c), ["開始、処理。「確認」", "終了"]);
}

#[test]
fn cjk_may_also_be_an_id() {
    let c = ok("flowchart TD\n  開始 --> 終了\n");
    assert_eq!(c.node_ids(), ["開始", "終了"]);
    assert_eq!(labels(&c), ["開始", "終了"]);
}

#[test]
fn emoji_survive_in_labels_and_in_ids() {
    let c = ok("flowchart TD\n  A[🚀 deploy] --> B[✅ done]\n");
    assert_eq!(labels(&c), ["🚀 deploy", "✅ done"]);
    let c = ok("flowchart TD\n  A --> 🚀\n");
    assert_eq!(c.node_ids(), ["A", "🚀"]);
}

#[test]
fn all_four_br_spellings_become_a_newline_in_any_case() {
    for br in [
        "<br>", "<br/>", "<br />", "</br>", "<BR>", "<Br />", "</BR>",
    ] {
        let c = ok(&format!("flowchart TD\n  A[one{br}two]\n"));
        assert_eq!(c.nodes[0].label, "one\ntwo", "{br}");
    }
}

#[test]
fn a_literal_backslash_n_is_also_a_line_break() {
    let c = ok("flowchart TD\n  A[\"one\\ntwo\"]\n");
    assert_eq!(c.nodes[0].label, "one\ntwo");
}

#[test]
fn line_breaks_work_in_edge_labels_and_subgraph_titles_too() {
    let c = ok("flowchart TD\n  A -->|one<br>two| B\n");
    assert_eq!(c.edges[0].label.as_deref(), Some("one\ntwo"));
    let c = ok("flowchart TD\n  subgraph s [one<br/>two]\n    a\n  end\n");
    assert_eq!(c.subgraphs[0].title, "one\ntwo");
}

#[test]
fn numeric_and_named_entities_decode() {
    // §2-6 rule 7: `#35;` is how a `#` gets past mermaid's own lexer.
    let c = ok("flowchart TD\n  A[a #35; b] --> B[#quot;q#quot;]\n");
    assert_eq!(labels(&c), ["a # b", "\"q\""]);
    let c = ok("flowchart TD\n  A[#8594; and #x2192;]\n");
    assert_eq!(c.nodes[0].label, "→ and →");
}

#[test]
fn an_unrecognised_entity_name_is_left_exactly_as_written() {
    let c = ok("flowchart TD\n  A[#notanentity; stays]\n");
    assert_eq!(c.nodes[0].label, "#notanentity; stays");
}

#[test]
fn a_markdown_string_label_keeps_its_text_and_loses_its_fences() {
    let c = ok("flowchart TD\n  A[\"`**bold**`\"] --> B\n");
    assert_eq!(c.nodes[0].label, "**bold**");
    assert_eq!(c.node_ids(), ["A", "B"]);
}

#[test]
fn decode_label_is_the_whole_pipeline_in_one_place() {
    assert_eq!(decode_label("  spaced  "), "spaced");
    assert_eq!(decode_label("\"quoted\""), "quoted");
    assert_eq!(decode_label("a<br>b"), "a\nb");
    assert_eq!(decode_label("#35;"), "#");
    assert_eq!(decode_label("\"`md`\""), "md");
    // A lone quote is not a pair and must survive.
    assert_eq!(decode_label("\""), "\"");
}

// =============================================================================================
// The statements that must be recognised and dropped (§2-3)
// =============================================================================================

/// Every statement kind from §2-3, each appended to the same two-node diagram.
///
/// The assertion is on the *whole* node list, because the failure this guards against is an
/// extra box, not a missing one: a line-based parser that treats "no arrow" as "node
/// declaration" turns `classDef hot fill:#f9f` into a node called `classDef`.
#[test]
fn no_recognised_but_undrawn_statement_becomes_a_phantom_node() {
    let cases: &[(&str, &str)] = &[
        ("classDef", "classDef hot fill:#f9f,stroke:#333"),
        ("classDef default", "classDef default fill:#eee"),
        ("class", "class A hot"),
        ("class list", "class A,B hot"),
        ("style", "style A fill:#bbf,stroke:#333,stroke-width:4px"),
        ("linkStyle", "linkStyle 0 stroke:red,stroke-width:2px"),
        ("linkStyle default", "linkStyle default stroke:#999"),
        (
            "linkStyle interpolate",
            "linkStyle 0 interpolate basis stroke:red",
        ),
        ("inline class", "A:::hot"),
        (
            "click href",
            "click A \"https://example.com\" \"a tooltip\"",
        ),
        ("click call", "click A callback \"a tooltip\""),
        (
            "click target",
            "click A href \"https://example.com\" _blank",
        ),
        ("comment", "%% this line is a comment"),
        ("indented comment", "    %% so is this one"),
        ("init directive", "%%{init: {\"theme\": \"dark\"}}%%"),
        ("accTitle", "accTitle: An accessible title"),
        ("accDescr", "accDescr: An accessible description"),
        ("accDescr block", "accDescr {\n  a longer description\n}"),
        ("edge metadata", "e1@{ animate: true }"),
    ];
    for (name, stmt) in cases {
        let src = format!("flowchart TD\n  A e1@--> B\n  {stmt}\n");
        let c = ok(&src);
        assert_eq!(c.node_ids(), ["A", "B"], "{name}: {stmt}");
        assert_eq!(c.edges.len(), 1, "{name}: {stmt}");
    }
}

#[test]
fn a_front_matter_block_is_lifted_out_and_its_title_kept() {
    let c = ok("---\ntitle: My Diagram\nconfig:\n  theme: dark\n---\nflowchart TD\n  A --> B\n");
    assert_eq!(c.node_ids(), ["A", "B"]);
    assert_eq!(c.title.as_deref(), Some("My Diagram"));
}

#[test]
fn a_front_matter_block_without_a_title_still_disappears() {
    let c = ok("---\nconfig:\n  theme: forest\n---\ngraph LR\n  A --> B\n");
    assert_eq!(c.node_ids(), ["A", "B"]);
    assert_eq!(c.title, None);
}

#[test]
fn a_nested_title_key_is_not_taken_for_the_diagram_title() {
    let c = ok("---\nconfig:\n  title: not this one\n---\ngraph LR\n  A --> B\n");
    assert_eq!(c.title, None);
}

#[test]
fn class_and_style_statements_are_kept_as_structure() {
    let c = ok("flowchart TD\n\
         A --> B\n\
         classDef hot fill:#f9f,stroke:#333\n\
         class A hot\n\
         style B fill:#bbf\n\
         linkStyle 0 stroke:red\n");
    assert_eq!(c.class_defs.len(), 1);
    assert_eq!(c.class_defs[0].name, "hot");
    assert_eq!(c.class_defs[0].styles, ["fill:#f9f", "stroke:#333"]);
    assert_eq!(c.node("A").unwrap().classes, ["hot"]);
    assert_eq!(c.node("B").unwrap().styles, ["fill:#bbf"]);
    assert_eq!(c.link_styles.len(), 1);
    assert_eq!(c.link_styles[0].styles, ["stroke:red"]);
}

#[test]
fn the_inline_class_operator_attaches_a_class_and_never_leaks_into_the_id() {
    let c = ok("flowchart TD\n  A:::hot --> B[x]:::cold\n");
    assert_eq!(c.node_ids(), ["A", "B"]);
    assert_eq!(c.node("A").unwrap().classes, ["hot"]);
    assert_eq!(c.node("B").unwrap().classes, ["cold"]);
    assert_eq!(c.node("B").unwrap().label, "x");
}

#[test]
fn a_class_statement_may_name_an_edge_or_a_subgraph() {
    let c = ok(
        "flowchart TD\n  subgraph s\n    a\n  end\n  a e1@--> b\n  class e1 hot\n  class s cold\n",
    );
    assert_eq!(c.edge("e1").unwrap().classes, ["hot"]);
    assert_eq!(c.subgraph("s").unwrap().classes, ["cold"]);
}

#[test]
fn a_style_or_class_naming_an_unknown_id_creates_nothing() {
    // A deliberate divergence: mermaid's `addVertex` creates the node and logs a warning nobody
    // reads, so a typo silently adds a box.
    let c = ok("flowchart TD\n  A --> B\n  style ghost fill:red\n  class phantom hot\n");
    assert_eq!(c.node_ids(), ["A", "B"]);
}

#[test]
fn a_semicolon_inside_a_style_declaration_does_not_split_off_a_node() {
    // In mermaid `stroke:blue` here becomes a node. This is the phantom-node failure §2-3 names.
    let c = ok("flowchart TD\n  A --> B\n  style A fill:#f9f;stroke:blue\n");
    assert_eq!(c.node_ids(), ["A", "B"]);
    let c = ok("flowchart TD\n  A --> B\n  classDef hot fill:#f9f;\n");
    assert_eq!(c.node_ids(), ["A", "B"]);
    assert_eq!(c.class_defs[0].styles, ["fill:#f9f"]);
}

#[test]
fn there_is_no_end_of_line_comment() {
    // §2-6 rule 2. `%%` only starts a comment at the beginning of a line; the trailing text is
    // part of the statement, and must not be mistaken for one.
    let c = ok("flowchart TD\n  A --> B %% not a comment\n");
    assert_eq!(c.node_ids(), ["A", "B"]);
    assert_eq!(c.edges.len(), 1);
}

#[test]
fn accessibility_text_is_captured_in_both_forms() {
    let c = ok("flowchart TD\n  accTitle: The title\n  accDescr: The description\n  A --> B\n");
    assert_eq!(c.acc_title.as_deref(), Some("The title"));
    assert_eq!(c.acc_descr.as_deref(), Some("The description"));
    let c = ok("flowchart TD\n  accDescr {\n    a longer one\n  }\n  A --> B\n");
    assert_eq!(c.acc_descr.as_deref(), Some("a longer one"));
}

// =============================================================================================
// `@{ … }` shape data and edge ids
// =============================================================================================

#[test]
fn the_new_shape_syntax_sets_the_shape_and_the_label_without_adding_a_node() {
    // The one gap that exists in the crate konoma ships today: it turns this into a second node.
    let c = ok("flowchart TD\n  A@{ shape: cyl, label: \"Store\" } --> B\n");
    assert_eq!(c.node_ids(), ["A", "B"]);
    assert_eq!(c.node("A").unwrap().shape, Shape::Cylinder);
    assert_eq!(c.node("A").unwrap().label, "Store");
}

#[test]
fn the_new_shape_syntax_also_works_as_a_statement_of_its_own_and_across_lines() {
    let c = ok("flowchart TD\n  A --> B\n  A@{ shape: diam }\n");
    assert_eq!(c.node_ids(), ["A", "B"]);
    assert_eq!(c.node("A").unwrap().shape, Shape::Diamond);

    let c = ok(
        "flowchart TD\n  A@{\n    shape: rounded\n    label: \"multi line form\"\n  }\n  A --> B\n",
    );
    assert_eq!(c.node_ids(), ["A", "B"]);
    assert_eq!(c.node("A").unwrap().shape, Shape::RoundedRect);
    assert_eq!(c.node("A").unwrap().label, "multi line form");
}

#[test]
fn every_documented_shape_name_resolves() {
    // A sample across the whole table, including the aliases that share one drawing.
    let cases = [
        ("rect", Shape::Rect),
        ("process", Shape::Rect),
        ("rounded", Shape::RoundedRect),
        ("stadium", Shape::Stadium),
        ("subproc", Shape::Subroutine),
        ("cyl", Shape::Cylinder),
        ("db", Shape::Cylinder),
        ("circle", Shape::Circle),
        ("dbl-circ", Shape::DoubleCircle),
        ("diam", Shape::Diamond),
        ("decision", Shape::Diamond),
        ("hex", Shape::Hexagon),
        ("lean-r", Shape::LeanRight),
        ("lean-l", Shape::LeanLeft),
        ("trap-b", Shape::Trapezoid),
        ("trap-t", Shape::InvTrapezoid),
        ("odd", Shape::Odd),
        ("text", Shape::Text),
        ("manual-input", Shape::Rect),
        ("paper-tape", Shape::Rect),
    ];
    for (name, want) in cases {
        assert_eq!(resolve_shape(name), Some(want), "{name}");
        let c = ok(&format!("flowchart TD\n  A@{{ shape: {name} }}\n"));
        assert_eq!(c.node_ids(), ["A"], "{name}");
        assert_eq!(c.nodes[0].shape, want, "{name}");
    }
}

#[test]
fn no_name_in_the_shape_table_would_be_refused_by_mermaids_own_spelling_rule() {
    // The rule (lowercase, no `_`) is applied before the table is consulted, so a table entry
    // that broke it would be unreachable — a name konoma believes it supports and always
    // refuses. mermaid's own map does contain such entries (`squareRect`, `lean_right`); this is
    // what keeps them from being copied in.
    for (name, _) in super::shapes::SHAPE_NAMES {
        assert!(
            !name.contains('_') && !name.chars().any(|c| c.is_uppercase()),
            "{name} could never resolve"
        );
        assert!(resolve_shape(name).is_some(), "{name} does not resolve");
    }
    // And the table really does carry mermaid's whole documented set.
    assert!(SHAPE_NAMES.len() >= 53, "{} names", SHAPE_NAMES.len());
}

#[test]
fn an_unknown_or_badly_spelled_shape_name_is_an_error() {
    // mermaid is deliberately strict here: lowercase only, no underscores, no unknown names.
    for name in [
        "bogus",
        "Rect",
        "lean_right",
        "CYL",
        "squareRect",
        "stateStart",
    ] {
        let src = format!("flowchart TD\n  A@{{ shape: {name} }}\n");
        assert!(
            matches!(parse(&src), Err(ParseError::UnknownShape { .. })),
            "{name} should have been refused"
        );
        assert_eq!(resolve_shape(name), None, "{name}");
    }
}

#[test]
fn shape_data_keys_that_konoma_does_not_draw_are_read_and_dropped() {
    let c = ok("flowchart TD\n  A@{ shape: rect, label: \"x\", w: 40, h: 20, constraint: on }\n");
    assert_eq!(c.node_ids(), ["A"]);
    assert_eq!(c.nodes[0].label, "x");
    let d = parse_shape_data("shape: cyl, label: \"a, b\"");
    assert_eq!(d.shape.as_deref(), Some("cyl"));
    assert_eq!(d.label.as_deref(), Some("a, b"));
}

#[test]
fn an_edge_id_is_read_and_never_leaks_into_a_node_name() {
    let c = ok("flowchart TD\n  A e1@--> B\n");
    assert_eq!(c.node_ids(), ["A", "B"]);
    assert_eq!(c.edges[0].id, "e1");
    assert!(c.edges[0].user_defined_id);
}

#[test]
fn an_edge_id_survives_a_labelled_link_and_a_group() {
    let c = ok("flowchart TD\n  A e1@-- yes --> B\n");
    assert_eq!(c.node_ids(), ["A", "B"]);
    assert_eq!(c.edges[0].id, "e1");
    assert_eq!(c.edges[0].label.as_deref(), Some("yes"));

    // In a group form only the edge between the last source and the first target keeps the id.
    let c = ok("flowchart TD\n  A e1@--> B & C\n");
    assert_eq!(c.node_ids(), ["A", "B", "C"]);
    assert_eq!(c.edges[0].id, "e1");
    assert!(!c.edges[1].user_defined_id);
}

#[test]
fn generated_edge_ids_follow_mermaids_own_counter() {
    let c = ok("flowchart TD\n  A --> B\n  A --> B\n  A --> C\n");
    let ids: Vec<&str> = c.edges.iter().map(|e| e.id.as_str()).collect();
    assert_eq!(ids, ["L_A_B_0", "L_A_B_2", "L_A_C_0"]);
    assert!(c.edges.iter().all(|e| !e.user_defined_id));
}

// =============================================================================================
// Preprocessing (§2-6 rule 1)
// =============================================================================================

#[test]
fn crlf_and_lone_cr_both_become_lf() {
    let c = ok("flowchart TD\r\n  A --> B\r\n");
    assert_eq!(c.node_ids(), ["A", "B"]);
    let c = ok("flowchart TD\r  A --> B\r");
    assert_eq!(c.node_ids(), ["A", "B"]);
}

#[test]
fn double_quotes_inside_an_html_attribute_become_single_quotes() {
    // Without this rewrite the `"` opens a string that swallows the rest of the diagram.
    let p = preprocess("<span class=\"x\">hi</span>");
    assert_eq!(p.text, "<span class='x'>hi</span>");
    let c = ok("flowchart TD\n  A[<span class=\"x\">hi</span>] --> B\n");
    assert_eq!(c.node_ids(), ["A", "B"]);
}

#[test]
fn removed_lines_keep_their_place_so_line_numbers_stay_true() {
    // The error message is the point of writing this parser (§1: the current crate throws away
    // messages as good as `unknown participant 'API' at line 3`), so the numbers must be the
    // reader's numbers.
    let src = "---\ntitle: t\n---\n%% a comment\nflowchart TD\n  A@{ shape: bogus }\n";
    assert_eq!(
        err(src),
        ParseError::UnknownShape {
            name: "bogus".to_string(),
            line: 6,
        }
    );
}

#[test]
fn a_directive_is_removed_even_when_it_spans_lines() {
    let c = ok("%%{init: {\n  \"theme\": \"dark\"\n}}%%\nflowchart TD\n  A --> B\n");
    assert_eq!(c.node_ids(), ["A", "B"]);
}

#[test]
fn a_comment_line_is_removed_but_a_directive_line_is_not_mistaken_for_one() {
    let p = preprocess("%% comment\n%%{init: {}}%%\nA\n");
    assert!(!p.text.contains("comment"));
    assert!(!p.text.contains("init"));
    assert!(p.text.contains('A'));
    // Both removals keep the line count.
    assert_eq!(p.text.matches('\n').count(), 3);
}

#[test]
fn a_horizontal_rule_further_down_is_not_taken_for_front_matter() {
    let c = ok("flowchart TD\n  A --- B\n");
    assert_eq!(edge_pairs(&c), [("A", "B")]);
    assert_eq!(c.title, None);
}

// =============================================================================================
// Deliberate divergences
// =============================================================================================

#[test]
fn direction_is_only_a_direction_statement_when_the_statement_starts_with_it() {
    // mermaid's lexer matches `.*direction\s+TB[^\n]*`, so this line becomes a direction
    // statement there and the node disappears from the diagram.
    let c = ok("flowchart LR\n  A[go direction TB now] --> B\n");
    assert_eq!(c.direction, Direction::LeftToRight);
    assert_eq!(c.node_ids(), ["A", "B"]);
    assert_eq!(labels(&c), ["go direction TB now", "B"]);
}

// =============================================================================================
// Broken input
// =============================================================================================

#[test]
fn an_unclosed_shape_bracket_is_an_error_rather_than_a_guessed_label() {
    for src in [
        "flowchart TD\n  A[foo\n",
        "flowchart TD\n  A(foo\n",
        "flowchart TD\n  A{foo\n",
        "flowchart TD\n  A --> B[bar\n",
    ] {
        assert!(
            matches!(parse(src), Err(ParseError::UnclosedShape { .. })),
            "{src:?}"
        );
    }
}

#[test]
fn an_unclosed_quote_is_an_error_rather_than_a_swallowed_diagram() {
    let e = err("flowchart TD\n  A[\"foo] --> B\n  C --> D\n");
    assert!(matches!(e, ParseError::UnclosedString { .. }), "{e:?}");
}

#[test]
fn an_unclosed_shape_data_block_is_an_error() {
    // A `{` that never closes is caught while the source is being cut into statements, because
    // that is the pass that has to know whether the newline after it ends the statement.
    let e = err("flowchart TD\n  A@{ shape: cyl\n");
    assert!(matches!(e, ParseError::UnclosedShape { .. }), "{e:?}");
    // A value quote that never closes is caught by the shape-data reader itself.
    let e = err("flowchart TD\n  A@{ shape: 'cyl }\n");
    assert!(matches!(e, ParseError::UnclosedShapeData { .. }), "{e:?}");
}

#[test]
fn a_dangling_link_keeps_the_nodes_that_were_readable() {
    let c = ok("flowchart TD\n  A -->\n  B --> C\n");
    assert_eq!(c.node_ids(), ["A", "B", "C"]);
    assert_eq!(edge_pairs(&c), [("B", "C")]);
}

#[test]
fn tag_soup_in_a_label_is_carried_through_as_text() {
    let c = ok("flowchart TD\n  A[<b>bold</b> <i>x</i>] --> B\n");
    assert_eq!(c.node_ids(), ["A", "B"]);
    assert_eq!(c.nodes[0].label, "<b>bold</b> <i>x</i>");
}

#[test]
fn error_messages_name_what_went_wrong() {
    assert!(err("sequenceDiagram\n  A ->> B: x\n")
        .to_string()
        .contains("sequenceDiagram"));
    assert!(err("graph TD\n").to_string().contains("no nodes"));
    assert!(err("flowchart TD\n  A@{ shape: nope }\n")
        .to_string()
        .contains("nope"));
}

#[test]
fn a_group_may_sit_in_the_middle_of_a_chain() {
    let c = ok("flowchart LR\n  A --> B & C --> D\n");
    assert_eq!(c.node_ids(), ["A", "B", "C", "D"]);
    assert_eq!(
        edge_pairs(&c),
        [("A", "B"), ("A", "C"), ("B", "D"), ("C", "D")]
    );
}

#[test]
fn links_and_labels_need_no_surrounding_spaces() {
    let c = ok("flowchart LR\n  A-->|x|B\n");
    assert_eq!(c.node_ids(), ["A", "B"]);
    assert_eq!(c.edges[0].label.as_deref(), Some("x"));
    let c = ok("flowchart LR\n  A-- text ---B\n");
    assert_eq!(c.node_ids(), ["A", "B"]);
    assert_eq!(c.edges[0].label.as_deref(), Some("text"));
    assert_eq!(c.edges[0].arrow, Arrow::None);
}

#[test]
fn semicolons_separate_statements_on_one_line() {
    let c = ok("flowchart LR\n  A-->B;C-->D;\n");
    assert_eq!(c.node_ids(), ["A", "B", "C", "D"]);
    assert_eq!(edge_pairs(&c), [("A", "B"), ("C", "D")]);
    // ...including a whole subgraph.
    let c = ok("flowchart LR\n  subgraph s; a --> b; end; s --> c\n");
    assert_eq!(c.subgraph("s").unwrap().members, ["a", "b"]);
    assert_eq!(edge_pairs(&c), [("a", "b"), ("s", "c")]);
}

#[test]
fn a_node_claimed_by_a_nested_block_leaves_its_parents_list() {
    // mermaid's `makeUniq`, seen from the other side: the parent mentioned `a` first, but the
    // nested block that closed first owns it.
    let c =
        ok("flowchart TB\n  subgraph outer\n    a\n    subgraph inner\n      a\n    end\n  end\n");
    assert_eq!(c.subgraph("inner").unwrap().members, ["a"]);
    assert_eq!(c.subgraph("outer").unwrap().members, ["inner"]);
}

#[test]
fn a_node_named_by_two_sibling_blocks_belongs_to_the_first_one_only() {
    let c = ok("flowchart TB\n  subgraph s1\n    a\n  end\n  subgraph s2\n    a\n  end\n");
    assert_eq!(c.subgraph("s1").unwrap().members, ["a"]);
    assert!(c.subgraph("s2").unwrap().members.is_empty());
    assert_eq!(c.node_ids(), ["a"]);
}

#[test]
fn link_style_reads_its_index_list_its_default_form_and_its_interpolation() {
    let c = ok("flowchart LR\n\
         A --> B\n\
         A --> C\n\
         linkStyle 0,1 stroke:red,stroke-width:2px\n\
         linkStyle default stroke:#999\n\
         linkStyle 1 interpolate basis stroke:blue\n");
    assert_eq!(c.node_ids(), ["A", "B", "C"]);
    assert_eq!(c.link_styles.len(), 3);
    assert_eq!(
        c.link_styles[0].target,
        LinkStyleTarget::Indices(vec![0, 1])
    );
    assert_eq!(c.link_styles[0].styles, ["stroke:red", "stroke-width:2px"]);
    assert_eq!(c.link_styles[1].target, LinkStyleTarget::Default);
    assert_eq!(c.link_styles[2].interpolate.as_deref(), Some("basis"));
    assert_eq!(c.link_styles[2].styles, ["stroke:blue"]);
}

#[test]
fn a_class_statement_may_name_several_nodes_at_once() {
    let c = ok("flowchart TD\n  A --> B --> C\n  class A,B,missing hot\n");
    assert_eq!(c.node_ids(), ["A", "B", "C"]);
    assert_eq!(c.node("A").unwrap().classes, ["hot"]);
    assert_eq!(c.node("B").unwrap().classes, ["hot"]);
    assert!(c.node("C").unwrap().classes.is_empty());
}

#[test]
fn shape_data_attached_to_a_subgraph_id_does_not_become_a_node() {
    let c = ok("flowchart TD\n  subgraph s\n    a\n  end\n  s@{ label: \"Renamed\" }\n");
    assert_eq!(c.node_ids(), ["a"]);
    assert_eq!(c.subgraphs.len(), 1);
}

#[test]
fn an_invisible_link_can_join_two_blocks() {
    let c = ok(
        "flowchart LR\n  subgraph one\n    a\n  end\n  subgraph two\n    b\n  end\n  one ~~~ two\n",
    );
    assert_eq!(c.node_ids(), ["a", "b"]);
    assert_eq!(edge_pairs(&c), [("one", "two")]);
    assert_eq!(c.edges[0].stroke, Stroke::Invisible);
}

#[test]
fn front_matter_is_removed_before_anything_looks_for_a_directive_or_a_comment() {
    // The order in section 2-6 rule 1 matters: a `%%` inside the YAML must never be read as
    // either a directive or a comment.
    let c = ok("---\ntitle: t\nnote: \"%%{init: {}}%%\"\n---\nflowchart TD\n  A --> B\n");
    assert_eq!(c.title.as_deref(), Some("t"));
    assert_eq!(c.node_ids(), ["A", "B"]);
}

#[test]
fn front_matter_and_crlf_survive_each_other() {
    let c = ok("---\r\ntitle: t\r\n---\r\nflowchart TD\r\n  A --> B\r\n");
    assert_eq!(c.title.as_deref(), Some("t"));
    assert_eq!(c.node_ids(), ["A", "B"]);
}

// =============================================================================================
// Whole diagrams
// =============================================================================================

#[test]
fn a_realistic_diagram_reads_end_to_end() {
    let src = "---\n\
               title: Deploy\n\
               ---\n\
               %%{init: {\"theme\": \"dark\"}}%%\n\
               flowchart LR\n\
                 %% the happy path\n\
                 subgraph build [Build stage]\n\
                   direction TB\n\
                   src[/Source/] --> compile[[Compile]]\n\
                   compile --> test{Tests pass?}\n\
                 end\n\
                 test -->|no| fail((Fail))\n\
                 test == yes ==> ship\n\
                 subgraph ship\n\
                   pkg[(Artifacts)] -.-> cdn[Publish<br/>to CDN]\n\
                 end\n\
                 cdn e1@--> done(((Done)))\n\
                 classDef bad fill:#f99\n\
                 class fail bad\n\
                 click done \"https://example.com\"\n\
                 style done fill:#9f9\n";
    let c = ok(src);
    assert_eq!(c.title.as_deref(), Some("Deploy"));
    assert_eq!(c.direction, Direction::LeftToRight);
    assert_eq!(
        c.node_ids(),
        ["src", "compile", "test", "fail", "pkg", "cdn", "done"]
    );
    assert_eq!(
        edge_pairs(&c),
        [
            ("src", "compile"),
            ("compile", "test"),
            ("test", "fail"),
            ("test", "ship"),
            ("pkg", "cdn"),
            ("cdn", "done"),
        ]
    );
    assert_eq!(c.node("src").unwrap().shape, Shape::LeanRight);
    assert_eq!(c.node("compile").unwrap().shape, Shape::Subroutine);
    assert_eq!(c.node("test").unwrap().shape, Shape::Diamond);
    assert_eq!(c.node("fail").unwrap().shape, Shape::Circle);
    assert_eq!(c.node("pkg").unwrap().shape, Shape::Cylinder);
    assert_eq!(c.node("done").unwrap().shape, Shape::DoubleCircle);
    assert_eq!(c.node("cdn").unwrap().label, "Publish\nto CDN");
    assert_eq!(c.node("fail").unwrap().classes, ["bad"]);
    assert_eq!(c.node("done").unwrap().styles, ["fill:#9f9"]);
    assert_eq!(c.edge("e1").unwrap().from, "cdn");
    assert_eq!(c.subgraphs.len(), 2);
    assert_eq!(c.subgraph("build").unwrap().title, "Build stage");
    assert_eq!(
        c.subgraph("build").unwrap().direction,
        Some(Direction::TopToBottom)
    );
    assert_eq!(
        c.subgraph("build").unwrap().members,
        ["src", "compile", "test"]
    );
    assert_eq!(c.subgraph("ship").unwrap().members, ["pkg", "cdn"]);
    // `ship` is a block used as an edge endpoint, so it must not also be a box.
    assert!(c.node("ship").is_none());
}

#[test]
fn a_typical_ai_written_graph_td_reads() {
    let src = "graph TD;\n\
               A[Client]-->B{Load balancer};\n\
               B-->C[Server 1];\n\
               B-->D[Server 2];\n\
               C-->E[(Database)];\n\
               D-->E;\n";
    let c = ok(src);
    assert_eq!(c.node_ids(), ["A", "B", "C", "D", "E"]);
    assert_eq!(c.edges.len(), 5);
    assert_eq!(c.node("E").unwrap().shape, Shape::Cylinder);
}

#[test]
fn konomas_own_demo_diagram_reads_in_both_languages() {
    // The mermaid fence in `samples/markdown.md` and its Japanese companion — the diagram every
    // konoma user meets first, so it is the one that must never regress.
    let en = "graph TD\n\
              A[Tree] -->|Enter| B{Resolve kind}\n\
              B -->|.md| C[block-model renderer]\n\
              B -->|mermaid fence| D[SVG to pixels]\n\
              C --> E[Full-screen preview]\n\
              D --> E\n";
    let ja = "graph TD\n\
              A[ツリー] -->|Enter| B{種別解決}\n\
              B -->|.md| C[ブロックモデルレンダラ]\n\
              B -->|mermaid fence| D[SVG → 実ピクセル]\n\
              C --> E[全画面プレビュー]\n\
              D --> E\n";
    for src in [en, ja] {
        let c = ok(src);
        assert_eq!(c.direction, Direction::TopToBottom);
        assert_eq!(c.node_ids(), ["A", "B", "C", "D", "E"]);
        assert_eq!(
            edge_pairs(&c),
            [("A", "B"), ("B", "C"), ("B", "D"), ("C", "E"), ("D", "E"),]
        );
        assert_eq!(c.node("B").unwrap().shape, Shape::Diamond);
        assert_eq!(
            c.edges
                .iter()
                .map(|e| e.label.as_deref())
                .collect::<Vec<_>>(),
            [
                Some("Enter"),
                Some(".md"),
                Some("mermaid fence"),
                None,
                None
            ]
        );
    }
    // The Japanese labels are the point: no quotes, full-width text, an arrow glyph.
    let c = ok(ja);
    assert_eq!(c.node("A").unwrap().label, "ツリー");
    assert_eq!(c.node("D").unwrap().label, "SVG → 実ピクセル");
}

// =============================================================================================
// Never panic (konoma design principle #3)
// =============================================================================================

#[test]
fn parse_never_panics_on_adversarial_input() {
    let mut cases: Vec<String> = vec![
        // Nothing, and almost nothing.
        String::new(),
        " ".to_string(),
        "\n".to_string(),
        "flowchart".to_string(),
        "graph".to_string(),
        "graph ".to_string(),
        "flowchart TD".to_string(),
        // Every delimiter, unbalanced, at the start and at the end of a statement.
        "graph TD\nA[".to_string(),
        "graph TD\nA]".to_string(),
        "graph TD\nA(".to_string(),
        "graph TD\nA)".to_string(),
        "graph TD\nA{".to_string(),
        "graph TD\nA}".to_string(),
        "graph TD\nA[[".to_string(),
        "graph TD\nA((".to_string(),
        "graph TD\nA(((".to_string(),
        "graph TD\nA[/".to_string(),
        "graph TD\nA[\\".to_string(),
        "graph TD\nA>".to_string(),
        "graph TD\nA|".to_string(),
        "graph TD\nA\"".to_string(),
        "graph TD\nA\"`".to_string(),
        "graph TD\n]]]]]]".to_string(),
        "graph TD\n))))))".to_string(),
        "graph TD\n}}}}}}".to_string(),
        // Links with nothing on either side, and every partial link token.
        "graph TD\n-->".to_string(),
        "graph TD\nA -->".to_string(),
        "graph TD\n--> B".to_string(),
        "graph TD\nA -".to_string(),
        "graph TD\nA --".to_string(),
        "graph TD\nA -.".to_string(),
        "graph TD\nA =".to_string(),
        "graph TD\nA ==".to_string(),
        "graph TD\nA ~".to_string(),
        "graph TD\nA ~~".to_string(),
        "graph TD\nA <".to_string(),
        "graph TD\nA -- x".to_string(),
        "graph TD\nA -- x ==> B".to_string(),
        "graph TD\nA -.- .-> B".to_string(),
        "graph TD\nA-->B-->".to_string(),
        // Ampersands with nothing to group.
        "graph TD\n&".to_string(),
        "graph TD\nA &".to_string(),
        "graph TD\n& B".to_string(),
        "graph TD\nA & & B".to_string(),
        "graph TD\nA & B & --> C".to_string(),
        // Subgraph structure that does not add up.
        "graph TD\nsubgraph".to_string(),
        "graph TD\nend".to_string(),
        "graph TD\nsubgraph a\nsubgraph b\n".to_string(),
        "graph TD\nsubgraph [".to_string(),
        "graph TD\nsubgraph a [".to_string(),
        "graph TD\nsubgraph a []\nend".to_string(),
        // Shape data that does not add up.
        "graph TD\nA@".to_string(),
        "graph TD\nA@{".to_string(),
        "graph TD\nA@{}".to_string(),
        "graph TD\nA@{ shape }".to_string(),
        "graph TD\nA@{ shape: }".to_string(),
        "graph TD\nA@{ label: \"unclosed }".to_string(),
        "graph TD\nA@{ shape: cyl".to_string(),
        "graph TD\n@{ shape: cyl }".to_string(),
        "graph TD\nA e1@".to_string(),
        "graph TD\nA e1@ --> B".to_string(),
        "graph TD\nA @ B".to_string(),
        // The "drop me" statements with their arguments missing.
        "graph TD\nclassDef".to_string(),
        "graph TD\nclassDef x".to_string(),
        "graph TD\nclass".to_string(),
        "graph TD\nclass x".to_string(),
        "graph TD\nstyle".to_string(),
        "graph TD\nstyle x".to_string(),
        "graph TD\nlinkStyle".to_string(),
        "graph TD\nlinkStyle x".to_string(),
        "graph TD\nlinkStyle 99 stroke:red".to_string(),
        "graph TD\nclick".to_string(),
        "graph TD\naccTitle:".to_string(),
        "graph TD\naccDescr".to_string(),
        "graph TD\naccDescr {".to_string(),
        "graph TD\ndirection".to_string(),
        "graph TD\ndirection XX".to_string(),
        "graph TD\nA:::".to_string(),
        "graph TD\nA:::: x".to_string(),
        // Front matter and directives that do not close.
        "---\ntitle: x\nflowchart TD\nA-->B\n".to_string(),
        "---\n---\n".to_string(),
        "---\n---\nflowchart TD\nA-->B\n".to_string(),
        "%%{init:\nflowchart TD\nA-->B\n".to_string(),
        "%%{".to_string(),
        "%%".to_string(),
        // Multibyte text at every boundary a byte-index bug would land on. konoma has panicked
        // on exactly this shape before (`\\` + CJK in the markdown renderer).
        "graph TD\nあ-->い".to_string(),
        "graph TD\nA[あ".to_string(),
        "graph TD\nA[🎉".to_string(),
        "graph TD\nA[\\あ]".to_string(),
        "graph TD\nA[/🎉/]".to_string(),
        "graph TD\nA[あ\u{0301}\u{200B}] --> B".to_string(),
        "graph TD\n🎉 --> 🚀".to_string(),
        "graph TD\nA[#".to_string(),
        "graph TD\nA[#;]".to_string(),
        "graph TD\nA[#999999999999;]".to_string(),
        "graph TD\nA[#xZZ;]".to_string(),
        "graph TD\nA[<br".to_string(),
        "graph TD\nA[</".to_string(),
        "graph TD\nA[<]".to_string(),
        // Control characters, tabs and a BOM.
        "graph TD\nA\u{0007}-->B".to_string(),
        "\u{FEFF}graph TD\nA-->B".to_string(),
        "graph TD\n\tA\t-->\tB".to_string(),
        "graph TD\nA[\u{0000}]".to_string(),
    ];
    // Scale: one very long line, many statements, deep nesting, and a wide `&` product.
    cases.push(format!("graph TD\n{}", "x".repeat(200_000)));
    cases.push(format!("graph TD\nA[{}]", "あ".repeat(50_000)));
    cases.push(format!("graph TD\n{}", "A-->B\n".repeat(5_000)));
    cases.push(format!("graph TD\n{}", "subgraph s\n".repeat(500)));
    cases.push(format!(
        "graph TD\n{}",
        "subgraph s\n".repeat(200) + &"end\n".repeat(200)
    ));
    cases.push(format!("graph TD\n{}", "end\n".repeat(5_000)));
    cases.push(format!(
        "graph TD\n{} --> {}",
        (0..200)
            .map(|i| format!("n{i}"))
            .collect::<Vec<_>>()
            .join(" & "),
        (0..200)
            .map(|i| format!("m{i}"))
            .collect::<Vec<_>>()
            .join(" & ")
    ));
    cases.push(format!("graph TD\nA {} B", "-".repeat(2_000)));
    cases.push(format!("graph TD\nA {} B", ".".repeat(2_000)));
    cases.push(format!("graph TD\nA -{}-> B", ".".repeat(2_000)));
    cases.push(format!("graph TD\n{}", "[".repeat(2_000)));
    cases.push(format!(
        "graph TD\nA{}x{}",
        "[".repeat(500),
        "]".repeat(500)
    ));
    cases.push(format!("graph TD\nA@{{ {} }}", "k: v, ".repeat(2_000)));
    cases.push("graph TD\n".to_string() + &"%% comment\n".repeat(5_000));

    for src in &cases {
        // The contract is only "does not panic"; either answer is acceptable.
        let _ = parse(src);
    }
}

#[test]
fn the_adversarial_sweep_never_produces_an_edge_endpoint_with_no_home() {
    // A structural invariant that holds for every input the parser accepts: an edge names either
    // a node or a subgraph. If it named neither, the layout stage would draw an edge into empty
    // space — the "phantom" failure in its other direction.
    let cases = [
        "graph TD\nA-->B",
        "graph TD\nsubgraph s\na\nend\ns-->b",
        "graph TD\nb-->s\nsubgraph s\na\nend",
        "graph TD\nA & B --> C & D",
        "graph TD\nA e1@--> B\ne1@{ animate: true }",
        "graph TD\nA-->B-->C-->A",
        "graph TD\nA-.->B==>C~~~D",
    ];
    for src in cases {
        let Ok(c) = parse(src) else {
            continue;
        };
        for e in &c.edges {
            assert!(
                c.node(&e.from).is_some() || c.subgraph(&e.from).is_some(),
                "{src:?}: edge from {} has no home",
                e.from
            );
            assert!(
                c.node(&e.to).is_some() || c.subgraph(&e.to).is_some(),
                "{src:?}: edge to {} has no home",
                e.to
            );
        }
    }
}

#[test]
fn every_subgraph_member_is_a_node_or_another_subgraph() {
    let cases = [
        "graph TD\nsubgraph s\na-->b\nend",
        "graph TD\nsubgraph o\nsubgraph i\na\nend\nb\nend",
        "graph TD\nsubgraph s\ndirection LR\na & b --> c\nend",
        "graph TD\nsubgraph s\nA@{ shape: cyl }\nend",
    ];
    for src in cases {
        let c = ok(src);
        for sg in &c.subgraphs {
            for m in &sg.members {
                assert!(
                    c.node(m).is_some() || c.subgraph(m).is_some(),
                    "{src:?}: member {m} of {} is neither",
                    sg.id
                );
            }
        }
    }
}
