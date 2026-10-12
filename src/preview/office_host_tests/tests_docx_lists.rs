//! Moved from `konoma-office-core/src/tests_docx_lists.rs`: the tests that need the host's Markdown renderer.

use super::tests_docx::*;
pub use konoma_office_core::tests_docx_lists::*;

#[test]
fn a_list_starting_at_five_is_written_with_five() {
    let numbering = abs(0, &lvl_start(0, "decimal", "%1.", 5)) + &num(1, 0);
    let body = num_p(1, 0, "a") + &num_p(1, 0, "b");
    let m = list_md(&numbering, &body);
    assert_eq!(m, "5. a\n6. b");
    assert_eq!(visible(&m), "5. a\n6. b");
}

#[test]
fn an_interrupted_list_continues_its_number_after_the_paragraph() {
    let numbering = abs(0, &lvl(0, "decimal", "%1.")) + &num(1, 0);
    let body = num_p(1, 0, "a") + &num_p(1, 0, "b") + &p("between") + &num_p(1, 0, "c");
    let m = list_md(&numbering, &body);
    assert_eq!(m, "1. a\n2. b\n\nbetween\n\n3. c");
    assert_eq!(visible(&m), "1. a\n2. b\n\nbetween\n\n3. c");
}

#[test]
fn nested_bullets_indent_by_the_parent_marker() {
    let numbering = abs(
        0,
        &(lvl(0, "bullet", "o") + &lvl(1, "bullet", "o") + &lvl(2, "bullet", "o")),
    ) + &num(1, 0);
    let body = num_p(1, 0, "a")
        + &num_p(1, 1, "b")
        + &num_p(1, 2, "c")
        + &num_p(1, 1, "d")
        + &num_p(1, 0, "e");
    let m = list_md(&numbering, &body);
    assert_eq!(m, "- a\n  - b\n    - c\n  - d\n- e");
    let v = visible(&m);
    assert!(
        v.contains("- a") && v.contains("- b") && v.contains("- c"),
        "{v}"
    );
}

#[test]
fn letters_are_the_documents_own_label_text() {
    let m = one_level("lowerLetter", "%1)", 3);
    assert_eq!(m, "a) item1  \nb) item2  \nc) item3");
    assert_eq!(visible(&m), "a) item1\nb) item2\nc) item3");
    assert_eq!(one_level("upperLetter", "%1.", 2), "A. item1  \nB. item2");
}

#[test]
fn decimal_with_a_paren_is_escaped_so_it_stays_text() {
    let m = one_level("decimal", "%1)", 2);
    assert_eq!(m, "1\\) item1  \n2\\) item2");
    assert_eq!(visible(&m), "1) item1\n2) item2");
}

#[test]
fn literal_items_are_indented_with_no_break_spaces_per_level() {
    let numbering = abs(
        0,
        &(lvl(0, "upperRoman", "%1.") + &lvl(1, "lowerLetter", "(%2)")),
    ) + &num(1, 0);
    let body = num_p(1, 0, "top") + &num_p(1, 1, "sub1") + &num_p(1, 1, "sub2");
    let m = list_md(&numbering, &body);
    assert_eq!(m, "I. top  \n\u{a0}\u{a0}(a) sub1  \n\u{a0}\u{a0}(b) sub2");
    assert_eq!(
        visible(&m).replace('\u{a0}', " "),
        "I. top\n  (a) sub1\n  (b) sub2"
    );
}

#[test]
fn start_override_restarts_a_second_instance_of_the_same_list() {
    let numbering = abs(0, &lvl(0, "decimal", "%1."))
        + &num(1, 0)
        + r#"<w:num w:numId="2"><w:abstractNumId w:val="0"/><w:lvlOverride w:ilvl="0"><w:startOverride w:val="1"/></w:lvlOverride></w:num>"#;
    let body = num_p(1, 0, "a") + &num_p(1, 0, "b") + &num_p(2, 0, "c") + &num_p(2, 0, "d");
    let m = list_md(&numbering, &body);
    assert_eq!(m, "1. a\n2. b\n\n<!-- -->\n\n1. c\n2. d");
    assert_eq!(visible(&m), "1. a\n2. b\n1. c\n2. d");
}

#[test]
fn a_second_instance_without_override_continues_the_count() {
    let numbering = abs(0, &lvl(0, "decimal", "%1.")) + &num(1, 0) + &num(2, 0);
    let body = num_p(1, 0, "a") + &num_p(1, 0, "b") + &num_p(2, 0, "c");
    let m = list_md(&numbering, &body);
    assert_eq!(m, "1. a\n2. b\n\n<!-- -->\n\n3. c");
    assert_eq!(visible(&m), "1. a\n2. b\n3. c");
}

#[test]
fn a_list_after_a_different_kind_of_list_is_a_separate_list() {
    let numbering = abs(0, &lvl(0, "bullet", "o"))
        + &abs(1, &lvl(0, "decimal", "%1."))
        + &num(1, 0)
        + &num(2, 1);
    let m = list_md(
        &numbering,
        &(num_p(1, 0, "dot") + &num_p(2, 0, "one") + &num_p(2, 0, "two")),
    );
    assert_eq!(m, "- dot\n1. one\n2. two");
    assert_eq!(visible(&m), "- dot\n\n1. one\n2. two");
}

#[test]
fn a_list_item_with_a_line_break_keeps_the_item_indent() {
    let numbering = abs(0, &lvl(0, "bullet", "o")) + &num(1, 0);
    let body = r#"<w:p><w:pPr><w:numPr><w:numId w:val="1"/></w:numPr></w:pPr><w:r><w:t>one</w:t><w:br/><w:t>two</w:t></w:r></w:p>"#;
    let m = list_md(&numbering, body);
    assert_eq!(m, "- one  \n  two");
    assert_eq!(visible(&m), "- one\ntwo");
}

#[test]
fn list_text_that_looks_like_markdown_is_escaped() {
    let numbering = abs(0, &lvl(0, "bullet", "o")) + &num(1, 0);
    let m = list_md(
        &numbering,
        &(num_p(1, 0, "- nested?") + &num_p(1, 0, "1. num?") + &num_p(1, 0, "# h?")),
    );
    assert_eq!(visible(&m), "- - nested?\n- 1. num?\n- # h?");
}

#[test]
fn konoma_draws_the_table() {
    let t = tbl(&[tr(&[tc("名前"), tc("値")]), tr(&[tc("x"), tc("1")])]);
    let lines = rendered(&md(&t), 40);
    assert!(lines.iter().any(|l| l.contains('┌')), "{lines:?}");
    assert!(
        lines.iter().any(|l| l.contains("名前") && l.contains("値")),
        "{lines:?}"
    );
    assert!(
        lines.iter().any(|l| l.contains('x') && l.contains('1')),
        "{lines:?}"
    );
}

#[test]
fn a_cell_with_several_paragraphs_joins_them_with_br_and_konoma_shows_a_space() {
    let t = tbl(&[tr(&[
        format!("<w:tc>{}{}</w:tc>", p("l1"), p("l2")),
        tc("b"),
    ])]);
    let m = md(&t);
    assert!(m.starts_with("| l1<br>l2 | b |"), "{m}");
    let lines = rendered(&m, 40);
    assert!(lines.iter().any(|l| l.contains("l1 l2")), "{lines:?}");
}

#[test]
fn pipes_and_markup_in_cells_are_escaped() {
    let t = tbl(&[tr(&[tc("a|b"), tc("*x*")]), tr(&[tc("`c`"), tc("[d]")])]);
    let m = md(&t);
    assert!(
        m.contains("a\\|b") && m.contains("\u{2217}x\u{2217}"),
        "{m}"
    );
    let lines = rendered(&m, 40);
    let all = lines.join("\n");
    // A table cell cannot hold a literal `*` pair or backtick pair: look-alikes stand in.
    assert!(
        all.contains("a|b") && all.contains("\u{2217}x\u{2217}") && all.contains("[d]"),
        "{all}"
    );
    assert!(!all.contains('\\'), "{all}");
    // Two columns, not three.
    let row = lines.iter().find(|l| l.contains("a|b")).unwrap();
    assert_eq!(row.matches('│').count(), 3, "{row}");
}
