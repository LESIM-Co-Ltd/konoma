//! Moved from `konoma-office-core/src/tests_docx_more.rs`: the tests that need the host's Markdown renderer.

use super::tests_docx::*;
pub use konoma_office_core::tests_docx_more::*;

#[test]
fn a_picture_in_a_table_cell_and_konoma_draws_it() {
    let cell = format!("<w:tc>{}</w:tc>", para(&drawing("rId9", "fig")));
    let d = conv(&with_image(
        &format!("<w:tbl><w:tr>{cell}</w:tr></w:tbl>"),
        "image1.png",
        tiny_png(1),
    ));
    assert!(
        d.markdown.starts_with("| ![fig](office-img://"),
        "{}",
        d.markdown
    );
    let lines = rendered(&d.markdown, 40);
    assert!(lines.iter().any(|l| l.contains("fig")), "{lines:?}");
}

#[test]
fn footnotes_become_numbered_references_and_definitions() {
    let body = para(&(run("first") + &fn_ref(2) + &run(" second") + &fn_ref(5)));
    let d = conv(&Dx::new(&body).footnotes(
        &(SEPARATORS.to_string() + &note(2, "two") + &note(5, "five") + &note(9, "unused")),
    ));
    assert_eq!(d.markdown, "first[^1] second[^2]\n\n[^1]: two\n[^2]: five");
    assert_eq!(d.notes, 2);
    let v = visible(&d.markdown);
    assert!(v.contains("first¹ second²"), "{v}");
    assert!(v.contains("1. two") && v.contains("2. five"), "{v}");
    assert!(!v.contains("unused"));
}

#[test]
fn footnote_text_with_markup_is_escaped() {
    let d = conv(
        &Dx::new(&para(&(run("x") + &fn_ref(1))))
            .footnotes(&note(1, "1. not a list *star* [^1] $5")),
    );
    let v = visible(&d.markdown);
    assert!(
        v.contains("1. 1. not a list *star* [^1] $5") || v.contains("1. not a list *star*"),
        "{v}"
    );
    assert!(v.contains("*star*") && v.contains("$5"), "{v}");
}

#[test]
fn a_literal_bracket_caret_in_the_text_does_not_collide_with_a_real_note() {
    let body = para(&(run("literal [^1] text") + &fn_ref(1)));
    let d = conv(&Dx::new(&body).footnotes(&note(1, "real")));
    let v = visible(&d.markdown);
    assert!(v.contains("literal [^1] text") && v.contains('¹'), "{v}");
    assert!(v.contains("1. real"), "{v}");
}

#[test]
fn link_text_keeps_its_formatting_and_brackets_do_not_end_it() {
    let inner = r#"<w:hyperlink r:id="rId7"><w:r><w:rPr><w:b/></w:rPr><w:t>bold</w:t></w:r><w:r><w:t xml:space="preserve"> and [b] c</w:t></w:r></w:hyperlink>"#;
    let d = conv(&with_link(&para(inner), "https://e.example/"));
    assert_eq!(
        d.markdown,
        "[**bold** and &#91;b&#93; c](https://e.example/)"
    );
    assert!(visible(&d.markdown).contains("bold and [b] c"));
}

#[test]
fn a_display_formula_is_its_own_block() {
    let body = para(
        &(run("before") + &format!("<m:oMathPara>{}</m:oMathPara>", omml("y")) + &run("after")),
    );
    let d = conv_with(&Dx::new(&body), &with_math(fake_latex)).unwrap();
    assert_eq!(d.markdown, "before\n\n$$\nX_1\n$$\n\nafter");
    assert!(visible(&d.markdown).contains("$$ X_1 $$"));
}

fn shown(text: &str) -> String {
    // The text as one paragraph, through the reader and konoma's renderer.
    let d = conv(&Dx::new(&p(&text
        .replace('&', "&amp;")
        .replace('<', "&lt;"))));
    visible(&d.markdown).replace('\u{200B}', "")
}

#[test]
fn inline_markup_characters_in_text_are_shown_literally() {
    for t in [
        "*star*",
        "**bold**",
        "a * b * c",
        "_under_",
        "snake_case_name",
        "__dunder__",
        "`code`",
        "``dbl``",
        "[link](http://example.com)",
        "![img](x.png)",
        "[^1]",
        "[x]",
        "[ ]",
        "<b>tag</b>",
        "<div>",
        "<!-- c -->",
        "$5",
        "$a$",
        "$$b$$",
        "a|b",
        "~~s~~",
        "~x~",
        "^up^",
        "back\\slash",
        "\\*",
        "&amp;",
        "&#35;",
        "&copy;",
        "a:b:c",
        ":smile:",
        "{#id}",
        "<http://x.com>",
        "x \\[a\\] y",
        "x \\(a\\) y",
        "50% off",
        "a & b",
        "(a) [b] {c}",
        "**",
        "*",
        "_",
        "``",
        "~~~",
        "[",
        "]",
        "![",
        "](",
        "<",
        ">",
    ] {
        let line = format!("before {t} after");
        assert_eq!(shown(&line), line, "{t:?}");
    }
}

#[test]
fn block_markup_at_the_start_of_a_paragraph_is_shown_literally() {
    for t in [
        "# heading",
        "## h2",
        "#hash",
        "- item",
        "+ item",
        "* item",
        "1. one",
        "1) one",
        "10. ten",
        "2023. in the year",
        "> quote",
        "=== ",
        "| a | b |",
        "```fence",
        "~~~",
        "- [ ] task",
        "[ ] box",
        "- - -",
        "<div>x</div>",
        "[x]: http://ref",
        "***bold*** start",
        "Term\\",
        "1.5 million",
        "3) c",
        "-5 degrees",
        "+1 plus",
    ] {
        assert_eq!(shown(t), t.trim_end(), "{t:?}");
    }
}

#[test]
fn a_paragraph_that_is_only_a_rule_looking_run_stays_text() {
    for t in ["---", "***", "___", "- - -", "* * *", "-----"] {
        assert_eq!(shown(t), t, "{t:?}");
    }
}

#[test]
fn a_front_matter_looking_start_is_text() {
    let d = conv(&Dx::new(
        &(p("---") + &p("title: x") + &p("---") + &p("body")),
    ));
    let v = visible(&d.markdown);
    assert!(v.contains("title: x") && v.contains("body"), "{v}");
}

#[test]
fn table_cells_show_markup_characters_literally_apart_from_look_alikes() {
    for (t, want) in [
        ("*star*", "\u{2217}star\u{2217}"),
        ("a*b*c", "a\u{2217}b\u{2217}c"),
        ("`code`", "\u{02CB}code\u{02CB}"),
        ("~~s~~", "\u{223C}\u{223C}s\u{223C}\u{223C}"),
        ("[l](u)", "[l](u)"),
        ("[x]", "[x]"),
        ("a|b", "a|b"),
        ("<b>x</b>", "<b>x</b>"),
        ("<br>", "<br>"),
        ("[^1]", "[^1]"),
        ("$5 and $6", "$5 and $6"),
        ("2 * 3", "2 * 3"),
        ("*", "*"),
        ("under_score_x", "under_score_x"),
        ("back\\slash", "back\\slash"),
        ("# not heading", "# not heading"),
    ] {
        let cell = format!(
            "<w:tc>{}</w:tc>",
            p(&t.replace('&', "&amp;").replace('<', "&lt;"))
        );
        let d = conv(&Dx::new(&format!(
            "<w:tbl><w:tr><w:tc>{}</w:tc></w:tr><w:tr>{cell}</w:tr></w:tbl>",
            p("h")
        )));
        let lines = rendered(&d.markdown, 80);
        let row = lines
            .iter()
            .find(|l| l.starts_with('│') && !l.contains(" h "))
            .unwrap_or_else(|| panic!("{t}: {lines:?}"));
        let got = row
            .trim_matches(|c| c == '│' || c == ' ')
            .replace('\u{200B}', "");
        assert_eq!(got, want, "{t:?}\n{d:?}");
    }
}

#[test]
fn a_literal_html_tag_in_a_cell_is_not_converted_by_the_inline_html_pass() {
    let cell = format!(
        "<w:tc>{}</w:tc>",
        p("press &lt;kbd&gt;Ctrl&lt;/kbd&gt; x&lt;br&gt;y")
    );
    let d = conv(&Dx::new(&format!(
        "<w:tbl><w:tr><w:tc>{}</w:tc></w:tr><w:tr>{cell}</w:tr></w:tbl>",
        p("h")
    )));
    let lines = rendered(&d.markdown, 80).join("\n").replace('\u{200B}', "");
    assert!(
        lines.contains("<kbd>Ctrl</kbd>") && lines.contains("x<br>y"),
        "{lines}"
    );
}

#[test]
fn bold_text_with_markup_characters_inside() {
    let body = para(&runp("<w:b/>", "a*b and `c`"));
    let m = md(&body);
    assert_eq!(visible(&m), "a*b and `c`");
}

#[test]
fn text_in_a_heading_is_escaped_too() {
    let m = md_styled(&styled_p("Heading1", "C# and *x* and [y]"), HEADING_STYLES);
    assert_eq!(visible(&m).lines().next().unwrap(), "C# and *x* and [y]");
}
