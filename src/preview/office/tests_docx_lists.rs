//! Word reader tests: lists and numbering, tables.

use super::docx::DocOptions;
use super::tests_docx::*;

fn lvl(i: u8, fmt: &str, text: &str) -> String {
    format!(
        r#"<w:lvl w:ilvl="{i}"><w:start w:val="1"/><w:numFmt w:val="{fmt}"/><w:lvlText w:val="{text}"/></w:lvl>"#
    )
}

fn lvl_start(i: u8, fmt: &str, text: &str, start: u32) -> String {
    format!(
        r#"<w:lvl w:ilvl="{i}"><w:start w:val="{start}"/><w:numFmt w:val="{fmt}"/><w:lvlText w:val="{text}"/></w:lvl>"#
    )
}

fn abs(id: u32, levels: &str) -> String {
    format!(r#"<w:abstractNum w:abstractNumId="{id}">{levels}</w:abstractNum>"#)
}

fn num(id: u32, abs_id: u32) -> String {
    format!(r#"<w:num w:numId="{id}"><w:abstractNumId w:val="{abs_id}"/></w:num>"#)
}

fn list_md(numbering: &str, body: &str) -> String {
    conv(&Dx::new(body).numbering(numbering)).markdown
}

/// One list of `n` items at level 0 in format `fmt` / text `text` (numId 1).
fn one_level(fmt: &str, text: &str, n: usize) -> String {
    let body: String = (1..=n).map(|i| num_p(1, 0, &format!("item{i}"))).collect();
    list_md(&(abs(0, &lvl(0, fmt, text)) + &num(1, 0)), &body)
}

#[test]
fn bullets_are_a_real_list() {
    assert_eq!(
        one_level("bullet", "\u{F0B7}", 3),
        "- item1\n- item2\n- item3"
    );
}

#[test]
fn decimal_percent_n_dot_is_a_real_ordered_list() {
    assert_eq!(
        one_level("decimal", "%1.", 3),
        "1. item1\n2. item2\n3. item3"
    );
}

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
fn nested_decimal_lists_indent_by_the_number_width() {
    let numbering = abs(0, &(lvl(0, "decimal", "%1.") + &lvl(1, "decimal", "%2."))) + &num(1, 0);
    let mut body = String::new();
    for i in 1..=10 {
        body += &num_p(1, 0, &format!("p{i}"));
        if i == 10 {
            body += &num_p(1, 1, "child");
        }
    }
    let m = list_md(&numbering, &body);
    assert!(m.ends_with("10. p10\n    1. child"), "{m}");
    assert!(m.starts_with("1. p1\n2. p2"), "{m}");
}

#[test]
fn a_deeper_item_after_a_skipped_level_still_nests_under_its_parent() {
    let numbering = abs(
        0,
        &(lvl(0, "bullet", "o") + &lvl(1, "bullet", "o") + &lvl(2, "bullet", "o")),
    ) + &num(1, 0);
    let body = num_p(1, 0, "a") + &num_p(1, 2, "deep") + &num_p(1, 0, "b");
    assert_eq!(list_md(&numbering, &body), "- a\n  - deep\n- b");
}

#[test]
fn letters_are_the_documents_own_label_text() {
    let m = one_level("lowerLetter", "%1)", 3);
    assert_eq!(m, "a) item1  \nb) item2  \nc) item3");
    assert_eq!(visible(&m), "a) item1\nb) item2\nc) item3");
    assert_eq!(one_level("upperLetter", "%1.", 2), "A. item1  \nB. item2");
}

#[test]
fn letters_repeat_after_z_like_word_does() {
    let body: String = (1..=28).map(|i| num_p(1, 0, &format!("i{i}"))).collect();
    let m = list_md(&(abs(0, &lvl(0, "lowerLetter", "%1.")) + &num(1, 0)), &body);
    assert!(m.contains("z. i26  \naa. i27  \nbb. i28"), "{m}");
}

#[test]
fn roman_numerals() {
    assert_eq!(
        one_level("upperRoman", "%1.", 4),
        "I. item1  \nII. item2  \nIII. item3  \nIV. item4"
    );
    assert_eq!(
        one_level("lowerRoman", "(%1)", 2),
        "(i) item1  \n(ii) item2"
    );
}

#[test]
fn decimal_with_a_paren_is_escaped_so_it_stays_text() {
    let m = one_level("decimal", "%1)", 2);
    assert_eq!(m, "1\\) item1  \n2\\) item2");
    assert_eq!(visible(&m), "1) item1\n2) item2");
}

#[test]
fn decimal_zero_and_ordinal() {
    assert_eq!(
        one_level("decimalZero", "%1.", 2),
        "01\\. item1  \n02\\. item2"
    );
    assert_eq!(
        one_level("ordinal", "%1", 3),
        "1st item1  \n2nd item2  \n3rd item3"
    );
}

#[test]
fn enclosed_circle_numbers() {
    assert_eq!(
        one_level("decimalEnclosedCircle", "%1", 3),
        "① item1  \n② item2  \n③ item3"
    );
}

#[test]
fn japanese_counting_formats() {
    assert_eq!(
        one_level("aiueo", "%1.", 3),
        "ア. item1  \nイ. item2  \nウ. item3"
    );
    assert_eq!(
        one_level("iroha", "%1)", 3),
        "イ) item1  \nロ) item2  \nハ) item3"
    );
    assert_eq!(
        one_level("ideographDigital", "%1", 2),
        "一 item1  \n二 item2"
    );
    let body: String = (1..=21).map(|i| num_p(1, 0, &format!("i{i}"))).collect();
    let m = list_md(
        &(abs(0, &lvl(0, "japaneseCounting", "%1")) + &num(1, 0)),
        &body,
    );
    assert!(
        m.contains("十 i10")
            && m.contains("十一 i11")
            && m.contains("二十 i20")
            && m.contains("二十一 i21"),
        "{m}"
    );
    assert_eq!(
        one_level("decimal", "第%1条", 2),
        "第1条 item1  \n第2条 item2"
    );
    assert_eq!(
        one_level("decimalFullWidth", "%1.", 2),
        "１. item1  \n２. item2"
    );
}

#[test]
fn an_unknown_number_format_is_decimal() {
    // Not a format konoma can reproduce as a Markdown list: written as text (the `.` escaped so
    // that the line does not become a list item).
    assert_eq!(
        one_level("somethingNew", "%1.", 2),
        "1\\. item1  \n2\\. item2"
    );
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
fn multi_level_numbers_show_the_whole_label() {
    let numbering = abs(
        0,
        &(lvl(0, "decimal", "%1.") + &lvl(1, "decimal", "%1.%2") + &lvl(2, "decimal", "%1.%2.%3")),
    ) + &num(1, 0);
    let body = num_p(1, 0, "a")
        + &num_p(1, 1, "a1")
        + &num_p(1, 1, "a2")
        + &num_p(1, 2, "a2x")
        + &num_p(1, 0, "b")
        + &num_p(1, 1, "b1");
    let m = list_md(&numbering, &body);
    assert!(
        m.contains("1.1 a1")
            && m.contains("1.2 a2")
            && m.contains("1.2.1 a2x")
            && m.contains("2.1 b1"),
        "{m}"
    );
    assert!(m.starts_with("1. a\n\n"), "{m}");
}

#[test]
fn legal_numbering_forces_decimal_in_the_label() {
    let numbering = abs(
        0,
        &(lvl(0, "upperRoman", "%1.")
            + r#"<w:lvl w:ilvl="1"><w:start w:val="1"/><w:numFmt w:val="decimal"/><w:isLgl/><w:lvlText w:val="%1.%2"/></w:lvl>"#),
    ) + &num(1, 0);
    let body = num_p(1, 0, "a") + &num_p(1, 0, "b") + &num_p(1, 1, "c");
    let m = list_md(&numbering, &body);
    assert!(m.contains("2.1 c"), "{m}");
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
fn a_start_override_to_another_number() {
    let numbering = abs(0, &lvl(0, "decimal", "%1."))
        + r#"<w:num w:numId="1"><w:abstractNumId w:val="0"/><w:lvlOverride w:ilvl="0"><w:startOverride w:val="7"/></w:lvlOverride></w:num>"#;
    assert_eq!(
        list_md(&numbering, &(num_p(1, 0, "a") + &num_p(1, 0, "b"))),
        "7. a\n8. b"
    );
}

#[test]
fn a_level_override_replaces_the_level_definition() {
    let numbering = abs(0, &lvl(0, "decimal", "%1."))
        + r#"<w:num w:numId="1"><w:abstractNumId w:val="0"/><w:lvlOverride w:ilvl="0"><w:lvl w:ilvl="0"><w:start w:val="1"/><w:numFmt w:val="upperLetter"/><w:lvlText w:val="%1)"/></w:lvl></w:lvlOverride></w:num>"#;
    assert_eq!(
        list_md(&numbering, &(num_p(1, 0, "a") + &num_p(1, 0, "b"))),
        "A) a  \nB) b"
    );
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
fn list_membership_from_the_paragraph_style() {
    let styles = r#"<w:style w:type="paragraph" w:styleId="LB"><w:name w:val="List Bullet"/><w:pPr><w:numPr><w:numId w:val="1"/></w:numPr></w:pPr></w:style>"#;
    let numbering = abs(0, &lvl(0, "bullet", "o")) + &num(1, 0);
    let body = styled_p("LB", "a") + &styled_p("LB", "b");
    let d = Dx::new(&body).styles(styles).numbering(&numbering);
    assert_eq!(conv(&d).markdown, "- a\n- b");
}

#[test]
fn num_id_zero_in_the_paragraph_switches_the_style_numbering_off() {
    let styles = r#"<w:style w:type="paragraph" w:styleId="LB"><w:name w:val="List Bullet"/><w:pPr><w:numPr><w:numId w:val="1"/></w:numPr></w:pPr></w:style>"#;
    let numbering = abs(0, &lvl(0, "bullet", "o")) + &num(1, 0);
    let body = r#"<w:p><w:pPr><w:pStyle w:val="LB"/><w:numPr><w:numId w:val="0"/></w:numPr></w:pPr><w:r><w:t>plain</w:t></w:r></w:p>"#;
    let d = Dx::new(body).styles(styles).numbering(&numbering);
    assert_eq!(conv(&d).markdown, "plain");
}

#[test]
fn an_unknown_list_id_or_a_none_format_is_a_plain_paragraph() {
    assert_eq!(
        list_md(
            &(abs(0, &lvl(0, "decimal", "%1.")) + &num(1, 0)),
            &num_p(9, 0, "orphan")
        ),
        "orphan"
    );
    assert_eq!(one_level("none", "", 1), "item1");
    assert_eq!(
        md(&num_p(1, 0, "no numbering part at all")),
        "no numbering part at all"
    );
}

#[test]
fn num_style_link_is_followed_to_the_real_definition() {
    let styles = r#"<w:style w:type="numbering" w:styleId="MyList"><w:name w:val="MyList"/><w:pPr><w:numPr><w:numId w:val="2"/></w:numPr></w:pPr></w:style>"#;
    let numbering = abs(0, &lvl(0, "decimal", "%1."))
        + r#"<w:abstractNum w:abstractNumId="5"><w:numStyleLink w:val="MyList"/></w:abstractNum>"#
        + &num(2, 0)
        + &num(1, 5);
    let d = Dx::new(&(num_p(1, 0, "a") + &num_p(1, 0, "b")))
        .styles(styles)
        .numbering(&numbering);
    assert_eq!(conv(&d).markdown, "1. a\n2. b");
}

#[test]
fn a_numbering_loop_through_style_links_ends() {
    let styles = r#"<w:style w:type="numbering" w:styleId="L"><w:name w:val="L"/><w:pPr><w:numPr><w:numId w:val="1"/></w:numPr></w:pPr></w:style>"#;
    let numbering =
        r#"<w:abstractNum w:abstractNumId="1"><w:numStyleLink w:val="L"/></w:abstractNum>"#
            .to_string()
            + &num(1, 1);
    let d = Dx::new(&num_p(1, 0, "a"))
        .styles(styles)
        .numbering(&numbering);
    let _ = conv(&d);
}

#[test]
fn a_numbered_heading_shows_its_number_and_the_slug_includes_it() {
    let styles = format!(
        "{HEADING_STYLES}{}",
        r#"<w:style w:type="paragraph" w:styleId="H1n"><w:name w:val="H1n"/><w:basedOn w:val="Heading1"/><w:pPr><w:numPr><w:numId w:val="1"/></w:numPr></w:pPr></w:style>"#
    );
    let numbering = abs(
        0,
        r#"<w:lvl w:ilvl="0"><w:start w:val="1"/><w:numFmt w:val="decimal"/><w:pStyle w:val="H1n"/><w:lvlText w:val="%1"/></w:lvl>"#,
    ) + &num(1, 0);
    let body = styled_p("H1n", "Intro")
        + &styled_p("H1n", "Next")
        + &para(r#"<w:hyperlink w:anchor="bm"><w:r><w:t>go</w:t></w:r></w:hyperlink>"#);
    let d = Dx::new(&body).styles(&styles).numbering(&numbering);
    let m = conv(&d).markdown;
    assert!(m.starts_with("# 1 Intro\n\n# 2 Next"), "{m}");
}

#[test]
fn a_style_that_gives_only_the_list_takes_its_level_from_the_level_that_names_it() {
    let styles = r#"<w:style w:type="paragraph" w:styleId="L2"><w:name w:val="L2"/><w:pPr><w:numPr><w:numId w:val="1"/></w:numPr></w:pPr></w:style>"#;
    let numbering = abs(
        0,
        &(lvl(0, "bullet", "o")
            + r#"<w:lvl w:ilvl="1"><w:start w:val="1"/><w:numFmt w:val="bullet"/><w:pStyle w:val="L2"/><w:lvlText w:val="o"/></w:lvl>"#),
    ) + &num(1, 0);
    let body = num_p(1, 0, "top") + &styled_p("L2", "sub");
    let d = Dx::new(&body).styles(styles).numbering(&numbering);
    assert_eq!(conv(&d).markdown, "- top\n  - sub");
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
fn an_empty_list_item_shows_nothing() {
    let numbering = abs(0, &lvl(0, "bullet", "o")) + &num(1, 0);
    assert_eq!(
        list_md(
            &numbering,
            &(num_p(1, 0, "a") + &num_p(1, 0, "") + &num_p(1, 0, "b"))
        ),
        "- a\n- b"
    );
}

#[test]
fn a_huge_counter_does_not_overflow() {
    let numbering = abs(0, &lvl_start(0, "decimal", "%1.", 4_000_000_000)) + &num(1, 0);
    let m = list_md(&numbering, &(num_p(1, 0, "a") + &num_p(1, 0, "b")));
    assert!(m.starts_with("999999999. a"), "{m}");
}

#[test]
fn lists_inside_a_table_cell_are_text_with_their_labels() {
    let numbering = abs(0, &lvl(0, "decimal", "%1.")) + &num(1, 0);
    let body = format!(
        "<w:tbl><w:tr><w:tc>{}{}</w:tc></w:tr></w:tbl>",
        num_p(1, 0, "x"),
        num_p(1, 0, "y")
    );
    let m = list_md(&numbering, &body);
    assert!(m.contains("1. x<br>2. y"), "{m}");
}

// ---------------------------------------------------------------------------------------------
// tables
// ---------------------------------------------------------------------------------------------

fn tc(text: &str) -> String {
    format!("<w:tc>{}</w:tc>", p(text))
}

fn tr(cells: &[String]) -> String {
    format!("<w:tr>{}</w:tr>", cells.concat())
}

fn tbl(rows: &[String]) -> String {
    format!("<w:tbl><w:tblPr/><w:tblGrid/>{}</w:tbl>", rows.concat())
}

#[test]
fn a_table_is_a_gfm_table_with_the_first_row_as_header() {
    let t = tbl(&[
        tr(&[tc("a"), tc("b")]),
        tr(&[tc("1"), tc("2")]),
        tr(&[tc("3"), tc("4")]),
    ]);
    assert_eq!(md(&t), "| a | b |\n| --- | --- |\n| 1 | 2 |\n| 3 | 4 |");
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
fn a_line_break_in_a_cell_is_br() {
    let cell = format!(
        "<w:tc>{}</w:tc>",
        para("<w:r><w:t>a</w:t><w:br/><w:t>b</w:t></w:r>")
    );
    assert!(md(&tbl(&[tr(&[cell])])).contains("| a<br>b |"));
}

#[test]
fn grid_span_puts_the_value_left_and_leaves_covered_cells_empty() {
    let span = format!(
        r#"<w:tc><w:tcPr><w:gridSpan w:val="2"/></w:tcPr>{}</w:tc>"#,
        p("wide")
    );
    let t = tbl(&[tr(&[span, tc("c")]), tr(&[tc("1"), tc("2"), tc("3")])]);
    assert_eq!(
        md(&t),
        "| wide |   | c |\n| --- | --- | --- |\n| 1 | 2 | 3 |"
    );
}

#[test]
fn vertical_merge_keeps_the_first_cell_and_blanks_the_continuation() {
    let first = format!(
        r#"<w:tc><w:tcPr><w:vMerge w:val="restart"/></w:tcPr>{}</w:tc>"#,
        p("tall")
    );
    let cont = format!(
        r#"<w:tc><w:tcPr><w:vMerge/></w:tcPr>{}</w:tc>"#,
        p("ignored")
    );
    let t = tbl(&[
        tr(&[tc("h1"), tc("h2")]),
        tr(&[first, tc("x")]),
        tr(&[cont, tc("y")]),
    ]);
    assert_eq!(
        md(&t),
        "| h1 | h2 |\n| --- | --- |\n| tall | x |\n|   | y |"
    );
}

#[test]
fn horizontal_merge_continuation_cells_are_empty() {
    let first = format!(
        r#"<w:tc><w:tcPr><w:hMerge w:val="restart"/></w:tcPr>{}</w:tc>"#,
        p("h")
    );
    let cont = format!(r#"<w:tc><w:tcPr><w:hMerge/></w:tcPr>{}</w:tc>"#, p("x"));
    let t = tbl(&[tr(&[first, cont, tc("z")])]);
    assert!(md(&t).starts_with("| h |   | z |"));
}

#[test]
fn ragged_rows_are_padded() {
    let t = tbl(&[tr(&[tc("a"), tc("b"), tc("c")]), tr(&[tc("1")])]);
    assert_eq!(md(&t), "| a | b | c |\n| --- | --- | --- |\n| 1 |   |   |");
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

#[test]
fn a_nested_table_is_flattened_into_its_cell() {
    let inner = tbl(&[tr(&[tc("i1"), tc("i2")]), tr(&[tc("i3"), tc("i4")])]);
    let outer = tbl(&[tr(&[
        format!("<w:tc>{}{}</w:tc>", p("before"), inner),
        tc("b"),
    ])]);
    let m = md(&outer);
    assert!(m.starts_with("| before<br>i1 / i2<br>i3 / i4 | b |"), "{m}");
}

#[test]
fn deleted_table_rows_are_gone() {
    let del = r#"<w:tr><w:trPr><w:del w:id="1"/></w:trPr><w:tc><w:p><w:r><w:t>old</w:t></w:r></w:p></w:tc></w:tr>"#;
    let t = format!("<w:tbl>{}{}{}</w:tbl>", tr(&[tc("h")]), del, tr(&[tc("v")]));
    assert_eq!(md(&t), "| h |\n| --- |\n| v |");
}

#[test]
fn a_table_between_paragraphs_has_blank_lines_around_it() {
    let t = tbl(&[tr(&[tc("a")])]);
    assert_eq!(
        md(&(p("before") + &t + &p("after"))),
        "before\n\n| a |\n| --- |\n\nafter"
    );
}

#[test]
fn an_empty_table_adds_nothing() {
    assert_eq!(md(&(p("a") + "<w:tbl/>" + &p("b"))), "a\n\nb");
}

#[test]
fn a_table_in_a_content_control_and_row_level_content_controls() {
    let t = format!(
        "<w:sdt><w:sdtContent>{}</w:sdtContent></w:sdt>",
        tbl(&[tr(&[tc("a")])])
    );
    assert!(md(&t).starts_with("| a |"));
    let row = format!(
        "<w:tbl><w:sdt><w:sdtContent>{}</w:sdtContent></w:sdt></w:tbl>",
        tr(&[tc("r")])
    );
    assert!(md(&row).starts_with("| r |"));
}

#[test]
fn a_heading_in_a_cell_is_bold_not_a_heading() {
    let cell = format!("<w:tc>{}</w:tc>", styled_p("Heading1", "Head"));
    let d = Dx::new(&tbl(&[tr(&[cell])])).styles(HEADING_STYLES);
    assert!(conv(&d).markdown.starts_with("| **Head** |"));
}

#[test]
fn a_huge_table_stops_at_the_cell_budget() {
    let row = tr(&[tc("a"), tc("b"), tc("c"), tc("d")]);
    let t = tbl(&vec![row; 50]);
    let opts = DocOptions {
        max_table_cells: 40,
        ..DocOptions::default()
    };
    let d = conv_with(&Dx::new(&t), &opts).unwrap();
    assert!(d.truncated);
    assert_eq!(d.markdown.matches("\n|").count(), 1 + 9, "{}", d.markdown);
}
