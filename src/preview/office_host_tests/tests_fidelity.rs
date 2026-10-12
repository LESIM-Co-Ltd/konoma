//! Moved from `konoma-office-core/src/tests_fidelity.rs`: the tests that need the host's Markdown renderer.

use super::tests_docx::*;
use super::tests_odt as od;
pub use konoma_office_core::tests_fidelity::*;

#[test]
fn a_superscript_run_is_written_as_sup_and_drawn_raised() {
    let body = para(&(run("10") + &runp(SUP, "6") + &run(" m") + &runp(SUP, "2")));
    let m = md(&body);
    assert_eq!(m, "10<sup>6</sup> m<sup>2</sup>");
    assert_eq!(visible(&m), "10\u{2076} m\u{b2}");
}

#[test]
fn a_subscript_run_is_written_as_sub_and_drawn_lowered() {
    let body = para(&(run("H") + &runp(SUB, "2") + &run("O")));
    let m = md(&body);
    assert_eq!(m, "H<sub>2</sub>O");
    assert_eq!(visible(&m), "H\u{2082}O");
}

#[test]
fn a_superscript_without_a_unicode_form_keeps_its_text() {
    // konoma's renderer shows `<sup>th</sup>` as `th` (no raised letters exist for all text).
    let body = para(&(run("5") + &runp(SUP, "th") + &run(" place")));
    let m = md(&body);
    assert_eq!(m, "5<sup>th</sup> place");
    assert_eq!(visible(&m), "5th place");
}

#[test]
fn superscripts_from_a_character_style_a_paragraph_style_and_the_defaults_apply() {
    let styles = r#"
<w:docDefaults><w:rPrDefault><w:rPr><w:vertAlign w:val="subscript"/></w:rPr></w:rPrDefault></w:docDefaults>
<w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/></w:style>
<w:style w:type="character" w:styleId="Up"><w:name w:val="Up"/><w:rPr><w:vertAlign w:val="superscript"/></w:rPr></w:style>
<w:style w:type="character" w:styleId="Up2"><w:name w:val="Up2"/><w:basedOn w:val="Up"/></w:style>
<w:style w:type="paragraph" w:styleId="P"><w:name w:val="P"/><w:basedOn w:val="Normal"/><w:rPr><w:vertAlign w:val="superscript"/></w:rPr></w:style>"#;
    // The document default is a subscript (a contrived one); a run's own `baseline` ends it.
    let body = para(&runp(r#"<w:vertAlign w:val="baseline"/>"#, "base"))
        + &para(&runp(r#"<w:rStyle w:val="Up2"/>"#, "2"))
        + &styled_p("P", "3")
        + &p("4");
    let m = md_styled(&body, styles);
    assert_eq!(m, "base\n\n<sup>2</sup>\n\n<sup>3</sup>\n\n<sub>4</sub>");
    assert_eq!(visible(&m), "base\n\n\u{b2}\n\n\u{b3}\n\n\u{2084}");
}

#[test]
fn a_superscript_keeps_bold_italic_links_and_escapes() {
    let body = para(
        &(runp(&format!("<w:b/>{SUP}"), "2")
            + &runp(&format!("<w:i/>{SUB}"), "i")
            + &runp(SUP, "*a_b")
            + r#"<w:hyperlink w:anchor="x"><w:r><w:rPr><w:vertAlign w:val="superscript"/></w:rPr><w:t>7</w:t></w:r></w:hyperlink>"#),
    );
    let m = md(&body);
    assert!(m.starts_with("**<sup>2</sup>***<sub>i</sub>*"), "{m}");
    // The text inside the tags is escaped like any other (a `*` is not markup).
    assert!(m.contains("<sup>\\*a_b</sup>"), "{m}");
    assert!(m.contains("<sup>7</sup>"), "{m}");
    let v = visible(&m);
    assert!(v.contains("\u{b2}") && v.contains("*a_b"), "{v}");
}

#[test]
fn superscripts_in_a_table_cell_a_heading_and_a_note_are_drawn_raised() {
    let tbl = r#"<w:tbl><w:tr><w:tc><w:p><w:r><w:t>m</w:t></w:r><w:r><w:rPr><w:vertAlign w:val="superscript"/></w:rPr><w:t>2</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>H</w:t></w:r><w:r><w:rPr><w:vertAlign w:val="subscript"/></w:rPr><w:t>2</w:t></w:r><w:r><w:t>O</w:t></w:r></w:p></w:tc></w:tr></w:tbl>"#;
    let m = md(tbl);
    assert!(
        m.contains("m<sup>2</sup>") && m.contains("H<sub>2</sub>O"),
        "{m}"
    );
    let v = visible(&m);
    assert!(v.contains("m\u{b2}") && v.contains("H\u{2082}O"), "{v}");

    let h = md_styled(
        &styled_p("H1", "x") .replace("<w:t xml:space=\"preserve\">x</w:t></w:r>", "<w:t>x</w:t></w:r><w:r><w:rPr><w:vertAlign w:val=\"superscript\"/></w:rPr><w:t>2</w:t></w:r>"),
        HEADING_STYLES,
    );
    assert!(h.contains("<sup>2</sup>"), "{h}");

    let note = r#"<w:footnote w:id="1"><w:p><w:r><w:footnoteRef/></w:r><w:r><w:t xml:space="preserve"> E=mc</w:t></w:r><w:r><w:rPr><w:vertAlign w:val="superscript"/></w:rPr><w:t>2</w:t></w:r></w:p></w:footnote>"#;
    let d = conv(&Dx::new(&para(&(run("x") + &fn_ref_local(1)))).footnotes(note));
    assert!(
        d.markdown.contains("[^1]: E=mc<sup>2</sup>"),
        "{}",
        d.markdown
    );
}

#[test]
fn the_vertical_forms_are_the_ones_the_renderer_draws() {
    // `docx.rs` keeps its own copy of the renderer's super/subscript tables (for heading slugs):
    // every character must come out the same either way.
    use crate::preview::markdown::process_inline_html;
    for c in (' '..='~').filter(|c| !matches!(c, '<' | '>' | '&' | '`' | '\\' | '[' | ']')) {
        for (tag, which) in [("sup", true), ("sub", false)] {
            let drawn = process_inline_html(&format!("<{tag}>{c}</{tag}>\n"));
            let drawn = drawn.strip_suffix('\n').unwrap_or(&drawn);
            let mine = super::docx::vertical_for_test(&c.to_string(), which);
            assert_eq!(drawn, mine, "<{tag}>{c}</{tag}>");
        }
    }
}

#[test]
fn odt_text_position_is_a_superscript_or_subscript() {
    let auto = od::text_style("Sup", r#"style:text-position="super 58%""#, None)
        + &od::text_style("Sub", r#"style:text-position="sub 58%""#, None)
        + &od::text_style("Up", r#"style:text-position="33% 58%""#, None)
        + &od::text_style("Down", r#"style:text-position="-25% 58%""#, None)
        + &od::text_style("Zero", r#"style:text-position="0% 100%""#, None)
        + &od::text_style("Kid", r#"style:text-position="super 58%""#, None)
        + &od::text_style("Kid2", "", Some("Kid"));
    let body = od::p(&format!(
        "10{}m{}H{}x{}y{}z{}",
        od::span("Sup", "6"),
        od::span("Up", "2"),
        od::span("Sub", "2"),
        od::span("Down", "i"),
        od::span("Zero", "0"),
        od::span("Kid2", "9")
    ));
    let m = od::md_auto(&body, &auto);
    assert_eq!(
        m,
        "10<sup>6</sup>m<sup>2</sup>H<sub>2</sub>x<sub>i</sub>y0z<sup>9</sup>"
    );
    assert_eq!(visible(&m), "10\u{2076}m\u{b2}H\u{2082}xiy0z\u{2079}");
}

#[test]
fn a_chart_title_is_bounded_and_made_safe() {
    let long = "T".repeat(5000);
    let d = Dx::new(&para(&chart_ref()))
        .rel("rIdC", "chart", "charts/chart1.xml", false)
        .part(
            "word/charts/chart1.xml",
            chart_part(Some(&long), "").as_bytes(),
        );
    let m = conv(&d).markdown;
    assert!(m.len() < 400, "{}", m.len());
    let d = Dx::new(&para(&chart_ref()))
        .rel("rIdC", "chart", "charts/chart1.xml", false)
        .part(
            "word/charts/chart1.xml",
            chart_part(Some("a *b* `c` [d](e)"), "").as_bytes(),
        );
    let v = visible(&conv(&d).markdown);
    assert_eq!(v, "[chart: a *b* `c` [d](e)]");
}

#[test]
fn a_rule_after_a_line_break_is_text_not_a_rule() {
    for rule in ["---", "***", "___", "- - -", "* * *", "-----", "_ _ _"] {
        let m = md(&br_lines(&["before", rule, "after"]));
        let v = visible(&m);
        assert!(!v.contains('\u{2500}'), "{rule}: {m:?} -> {v}");
        let want: String = rule.chars().filter(|c| !c.is_whitespace()).collect();
        let shown: String = v
            .lines()
            .nth(1)
            .unwrap()
            .chars()
            .filter(|c| !c.is_whitespace() && *c != '\u{200B}')
            .collect();
        assert_eq!(shown, want, "{rule}: {v}");
        assert_eq!(v.lines().count(), 3, "{rule}: {v}");
    }
}

#[test]
fn every_line_start_after_a_break_is_guarded_like_the_first() {
    // The same text as a paragraph of its own and as a later line of a paragraph looks alike.
    for line in [
        "# not a heading",
        "## h",
        "> not a quote",
        "- not a bullet",
        "+ not a bullet",
        "* not a bullet",
        "1. not a list",
        "2) not a list",
        "===",
        "=",
        "-",
        "--",
        "```",
        "~~~",
        "|a|b|",
        "| --- | --- |",
        "<div>",
        "    indented",
        "[^1]: def",
        "[x]: http://e.example",
        "$$",
    ] {
        let alone = visible(&md(&p(&xml_escape(line))));
        let later = visible(&md(&br_lines(&["first", line, "last"])));
        let lines: Vec<&str> = later.lines().collect();
        assert_eq!(lines.len(), 3, "{line}: {later:?}");
        assert_eq!(
            lines[1].trim_start(),
            alone.trim_start(),
            "{line}: {later:?}"
        );
        assert_eq!(lines[0], "first");
        assert_eq!(lines[2], "last");
    }
}

#[test]
fn several_rules_in_a_row_and_a_rule_as_the_first_line() {
    let m = md(&br_lines(&["---", "x", "---", "***", "y"]));
    let v = visible(&m);
    assert!(!v.contains('\u{2500}'), "{m:?} -> {v}");
    assert_eq!(v.lines().count(), 5, "{v}");
}

#[test]
fn a_rule_after_a_break_in_a_list_item_and_a_table_cell() {
    let numbering = r#"<w:abstractNum w:abstractNumId="0"><w:lvl w:ilvl="0"><w:numFmt w:val="decimal"/><w:lvlText w:val="%1."/></w:lvl></w:abstractNum><w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num>"#;
    let item = r#"<w:p><w:pPr><w:numPr><w:ilvl w:val="0"/><w:numId w:val="1"/></w:numPr></w:pPr><w:r><w:t>item</w:t></w:r><w:r><w:br/></w:r><w:r><w:t>---</w:t></w:r></w:p>"#;
    let m = conv(&Dx::new(item).numbering(numbering)).markdown;
    assert!(!visible(&m).contains('\u{2500}'), "{m:?}");
    let tbl = r#"<w:tbl><w:tr><w:tc><w:p><w:r><w:t>a</w:t></w:r><w:r><w:br/></w:r><w:r><w:t>---</w:t></w:r></w:p></w:tc></w:tr></w:tbl>"#;
    let m = md(tbl);
    assert!(
        !visible(&m).contains('\u{2500}') || m.contains('\u{250C}') || m.contains("<br>"),
        "{m:?}"
    );
    assert!(m.contains("a<br>"), "{m:?}");
}

#[test]
fn a_rule_after_a_line_break_in_an_odt_paragraph() {
    for rule in ["---", "***", "___"] {
        let m = od::md(&od::p(&format!(
            "before<text:line-break/>{rule}<text:line-break/>after"
        )));
        let v = visible(&m);
        assert!(!v.contains('\u{2500}'), "{rule}: {m:?} -> {v}");
        assert_eq!(v.lines().count(), 3, "{rule}: {v}");
    }
    let m = od::md(&od::p(
        "a<text:line-break/># h<text:line-break/>&gt; q<text:line-break/>1. x",
    ));
    assert_eq!(visible(&m), "a\n# h\n> q\n1. x");
}

#[test]
fn an_embedded_document_without_a_known_relationship_is_still_marked() {
    let body = r#"<w:altChunk r:id="rIdX"/>"#;
    assert_eq!(md(body), "\\[embedded document]");
    // A hostile name is made safe and bounded.
    let d = Dx::new(r#"<w:altChunk r:id="rIdA"/>"#).rel(
        "rIdA",
        "aFChunk",
        &format!("a*_[x]({})`b.rtf", "n".repeat(500)),
        false,
    );
    let m = conv(&d).markdown;
    assert!(m.len() < 200, "{m}");
    // Shown as text, not turned into a link.
    let v = visible(&m);
    assert!(v.starts_with("[embedded document: a*_[x](nnn"), "{v}");
}

#[test]
fn the_paragraphs_of_a_note_are_lines_of_it() {
    let n = r#"<w:footnote w:id="1"><w:p><w:r><w:footnoteRef/></w:r><w:r><w:t>First</w:t></w:r></w:p><w:p><w:r><w:t>Second</w:t></w:r></w:p><w:p><w:r><w:t>Third</w:t></w:r></w:p></w:footnote>"#;
    let d = conv(&Dx::new(&para(&(run("x") + &fn_ref_local(1)))).footnotes(n));
    assert_eq!(
        d.markdown,
        "x[^1]\n\n[^1]: First\\\n    Second\\\n    Third"
    );
    let v = rendered(&d.markdown, 80);
    let joined = v.join("\n");
    assert!(joined.contains("First\nSecond\nThird"), "{joined}");
    assert!(
        joined.contains('\u{b9}'),
        "the reference is numbered: {joined}"
    );
}

#[test]
fn a_table_and_a_list_in_a_note_keep_their_lines() {
    let n = r#"<w:footnote w:id="1"><w:p><w:r><w:footnoteRef/></w:r><w:r><w:t>Intro</w:t></w:r></w:p><w:tbl><w:tr><w:tc><w:p><w:r><w:t>a</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>b</w:t></w:r></w:p></w:tc></w:tr><w:tr><w:tc><w:p><w:r><w:t>c</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>d</w:t></w:r></w:p></w:tc></w:tr></w:tbl><w:p><w:r><w:t>End</w:t></w:r></w:p></w:footnote>"#;
    let d = conv(&Dx::new(&para(&(run("x") + &fn_ref_local(1)))).footnotes(n));
    assert_eq!(
        d.markdown,
        "x[^1]\n\n[^1]: Intro\\\n    a / b\\\n    c / d\\\n    End"
    );
    let joined = rendered(&d.markdown, 80).join("\n");
    assert!(joined.contains("Intro\na / b\nc / d\nEnd"), "{joined}");
}

#[test]
fn a_line_start_inside_a_note_is_guarded() {
    let n = r#"<w:footnote w:id="1"><w:p><w:r><w:footnoteRef/></w:r><w:r><w:t>First</w:t></w:r></w:p><w:p><w:r><w:t>---</w:t></w:r></w:p><w:p><w:r><w:t>1. x</w:t></w:r></w:p><w:p><w:r><w:t># h</w:t></w:r></w:p></w:footnote>"#;
    let d = conv(&Dx::new(&para(&(run("x") + &fn_ref_local(1)))).footnotes(n));
    let joined = rendered(&d.markdown, 80).join("\n");
    assert!(
        !joined.contains('\u{2500}') || joined.matches('\u{2500}').count() == 80,
        "{joined}"
    );
    assert!(
        joined.contains("First\n") && joined.contains("1. x") && joined.contains("# h"),
        "{joined}"
    );
}

const SUB: &str = r#"<w:vertAlign w:val="subscript"/>"#;

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn br_lines(lines: &[&str]) -> String {
    let mut s = String::new();
    for (i, l) in lines.iter().enumerate() {
        if i > 0 {
            s.push_str("<w:r><w:br/></w:r>");
        }
        s.push_str(&format!(
            r#"<w:r><w:t xml:space="preserve">{}</w:t></w:r>"#,
            xml_escape(l)
        ));
    }
    para(&s)
}
