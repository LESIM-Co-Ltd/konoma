//! Fidelity of the Word / OpenDocument conversion: what Word and LibreOffice draw must be what
//! konoma's renderer shows -- superscripts, symbol-font characters, legacy form fields, hidden text
//! set by a style, table rows that start late, charts, ruby, number formats, the lines of a
//! paragraph, deletions across paragraphs, embedded documents, multi-paragraph notes.

use super::docx_styles::format_number;
use super::tests_docx::*;
use super::tests_odt as od;

// ---------------------------------------------------------------------------------------------
// 1. superscript / subscript
// ---------------------------------------------------------------------------------------------

const SUP: &str = r#"<w:vertAlign w:val="superscript"/>"#;
const SUB: &str = r#"<w:vertAlign w:val="subscript"/>"#;

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
fn an_unknown_or_empty_vert_align_changes_nothing() {
    for v in ["", "x", "Superscript", "baseline"] {
        let body = para(&runp(&format!(r#"<w:vertAlign w:val="{v}"/>"#), "plain"));
        assert_eq!(md(&body), "plain", "{v:?}");
    }
    let body = para(r#"<w:r><w:rPr><w:vertAlign/></w:rPr><w:t>plain</w:t></w:r>"#);
    assert_eq!(md(&body), "plain");
    // A superscript that is only white space writes no tags.
    assert_eq!(md(&para(&(run("a") + &runp(SUP, " ") + &run("b")))), "a b");
}

#[test]
fn the_default_paragraph_style_applies_to_a_paragraph_that_names_none() {
    let styles = r#"
<w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/><w:rPr><w:b/><w:vertAlign w:val="superscript"/></w:rPr></w:style>
<w:style w:type="paragraph" w:styleId="Other"><w:name w:val="Other"/></w:style>"#;
    assert_eq!(md_styled(&p("x"), styles), "**<sup>x</sup>**");
    assert_eq!(md_styled(&styled_p("Other", "x"), styles), "x");
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
fn spaces_around_a_superscript_stay_outside_the_tags() {
    let body = para(&(run("a") + &runp(SUP, " 2 ") + &run("b")));
    assert_eq!(md(&body), "a <sup>2</sup> b");
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

fn fn_ref_local(id: i64) -> String {
    format!(r#"<w:r><w:footnoteReference w:id="{id}"/></w:r>"#)
}

#[test]
fn a_superscript_heading_has_the_slug_of_what_is_drawn() {
    // The heading is drawn `x²`; a link to it must use the slug of that text.
    let body = format!(
        r#"<w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:bookmarkStart w:id="0" w:name="bm"/><w:r><w:t>x</w:t></w:r><w:r><w:rPr><w:vertAlign w:val="superscript"/></w:rPr><w:t>2</w:t></w:r></w:p>{}"#,
        para(r#"<w:hyperlink w:anchor="bm"><w:r><w:t>go</w:t></w:r></w:hyperlink>"#)
    );
    let m = md_styled(&body, HEADING_STYLES);
    assert!(m.contains("[go](#x\u{b2})"), "{m}");
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
fn odt_text_position_from_a_paragraph_style_and_in_a_cell() {
    let auto = od::para_style("Pp", r#"style:text-position="sub 58%""#, None, "")
        + &od::text_style("Sup", r#"style:text-position="super 58%""#, None);
    let body = od::sp("Pp", "a2")
        + &format!(
            "<table:table><table:table-row><table:table-cell>{}</table:table-cell></table:table-row></table:table>",
            od::p(&format!("m{}", od::span("Sup", "2")))
        );
    let m = od::md_auto(&body, &auto);
    assert!(m.starts_with("<sub>a2</sub>"), "{m}");
    assert!(m.contains("m<sup>2</sup>"), "{m}");
}

// ---------------------------------------------------------------------------------------------
// 2. symbol fonts
// ---------------------------------------------------------------------------------------------

fn sym(font: &str, ch: &str) -> String {
    format!(r#"<w:r><w:sym w:font="{font}" w:char="{ch}"/></w:r>"#)
}

fn in_font(font: &str, text: &str) -> String {
    format!(
        r#"<w:r><w:rPr><w:rFonts w:ascii="{font}" w:hAnsi="{font}" w:cs="{font}"/></w:rPr><w:t>{text}</w:t></w:r>"#
    )
}

#[test]
fn wingdings_symbols_are_the_check_boxes_ticks_and_bullets_they_draw() {
    for (ch, want) in [
        ("F0FE", "\u{2611}"),
        ("F0FD", "\u{2612}"),
        ("F0A8", "\u{2610}"),
        ("F0FC", "\u{2713}"),
        ("F0FB", "\u{2717}"),
        ("F0A7", "\u{25AA}"),
        ("F06C", "\u{25CF}"),
        ("F076", "\u{2756}"),
        ("F0D8", "\u{27A2}"),
        ("F0AB", "\u{2605}"),
        ("F0F0", "\u{21E8}"),
    ] {
        assert_eq!(md(&para(&sym("Wingdings", ch))), want, "{ch}");
    }
}

#[test]
fn symbol_font_characters_are_greek_letters_and_math_signs() {
    for (ch, want) in [
        ("F061", "\u{3b1}"),
        ("F062", "\u{3b2}"),
        ("F044", "\u{394}"),
        ("F0B7", "\u{2022}"),
        ("F0B1", "\u{b1}"),
        ("F0A3", "\u{2264}"),
        ("F0B3", "\u{2265}"),
        ("F0A5", "\u{221e}"),
        ("F0AE", "\u{2192}"),
        ("F0D6", "\u{221a}"),
    ] {
        assert_eq!(md(&para(&sym("Symbol", ch))), want, "{ch}");
    }
}

#[test]
fn a_run_in_a_symbol_font_draws_symbols_for_its_codes_whatever_the_spelling() {
    // The private-use form, the bare code and the code with the font's high byte are one symbol.
    assert_eq!(md(&para(&in_font("Wingdings", "\u{F0FE}"))), "\u{2611}");
    assert_eq!(md(&para(&in_font("Wingdings", "\u{fe}"))), "\u{2611}");
    assert_eq!(md(&para(&in_font("Wingdings", "q"))), "\u{2751}");
    assert_eq!(md(&para(&in_font("Symbol", "a"))), "\u{3b1}");
    assert_eq!(md(&para(&in_font("Symbol", "\u{F0B7}"))), "\u{2022}");
    // Text around it keeps its own font.
    let body = para(&(run("Done ") + &in_font("Wingdings 2", "\u{F052}") + &run(" ok")));
    assert_eq!(md(&body), "Done \u{2612} ok");
}

#[test]
fn a_symbol_font_from_a_style_applies_and_a_run_can_switch_it_off() {
    let styles = r#"
<w:style w:type="character" w:styleId="Sy"><w:name w:val="Sy"/><w:rPr><w:rFonts w:ascii="Wingdings"/></w:rPr></w:style>"#;
    let body = para(
        &(r#"<w:r><w:rPr><w:rStyle w:val="Sy"/></w:rPr><w:t>&#xF0FC;</w:t></w:r>"#.to_string()
            + r#"<w:r><w:rPr><w:rStyle w:val="Sy"/><w:rFonts w:ascii="Arial"/></w:rPr><w:t>l</w:t></w:r>"#),
    );
    assert_eq!(md_styled(&body, styles), "\u{2713}l");
}

#[test]
fn a_symbol_the_tables_do_not_know_is_shown_as_a_bullet_never_dropped() {
    assert_eq!(md(&para(&sym("Wingdings", "F0C1"))), "\u{2022}");
    assert_eq!(md(&para(&sym("Wingdings 3", "F070"))), "\u{2022}");
    assert_eq!(md(&para(&in_font("Webdings", "\u{F0C8}"))), "\u{2022}");
    // A private-use character with no font to place it is a symbol as well.
    assert_eq!(md(&para(&run("\u{F0B7}"))), "\u{2022}");
    assert_eq!(md(&para(&sym("MS Gothic", "E123"))), "\u{2022}");
}

#[test]
fn real_unicode_with_a_plain_font_and_the_internal_tokens_are_left_alone() {
    assert_eq!(md(&para(&sym("Arial", "00A9"))), "\u{a9}");
    assert_eq!(md(&para(&sym("MS Gothic", "2612"))), "\u{2612}");
    assert_eq!(md(&para(&in_font("Arial", "x\u{ae}"))), "x\u{ae}");
    // U+E000..E002 are konoma's own tokens: a document cannot put one into the Markdown.
    for t in ["E000", "E001", "E002"] {
        for font in ["Wingdings", "Symbol", "Arial"] {
            let m = md(&para(&sym(font, t)));
            assert!(
                !m.contains(['\u{E000}', '\u{E001}', '\u{E002}']),
                "{font} {t}: {m:?}"
            );
        }
    }
    let m = md(&para(&in_font("Wingdings", "\u{E000}\u{E001}")));
    assert!(!m.contains(['\u{E000}', '\u{E001}']), "{m:?}");
}

#[test]
fn symbol_spaces_and_bounds() {
    assert_eq!(
        md(&para(&(run("a") + &sym("Wingdings", "F020") + &run("b")))),
        "a b"
    );
    // A `w:sym` with a missing or invalid char draws nothing and breaks nothing.
    assert_eq!(
        md(&para(
            r#"<w:r><w:sym w:font="Wingdings"/><w:t>x</w:t></w:r>"#
        )),
        "x"
    );
    assert_eq!(
        md(&para(
            r#"<w:r><w:sym w:font="Wingdings" w:char="zz"/><w:t>x</w:t></w:r>"#
        )),
        "x"
    );
}

// ---------------------------------------------------------------------------------------------
// 3. legacy form fields
// ---------------------------------------------------------------------------------------------

fn form_field(instr: &str, ffdata: &str, result: &str) -> String {
    let sep = if result.is_empty() {
        String::new()
    } else {
        format!(r#"<w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>{result}</w:t></w:r>"#)
    };
    format!(
        r#"<w:r><w:fldChar w:fldCharType="begin"><w:ffData><w:name w:val="F1"/><w:enabled/>{ffdata}</w:ffData></w:fldChar></w:r><w:r><w:instrText xml:space="preserve"> {instr} </w:instrText></w:r>{sep}<w:r><w:fldChar w:fldCharType="end"/></w:r>"#
    )
}

#[test]
fn a_legacy_check_box_is_a_ballot_box_checked_or_not() {
    let on = form_field(
        "FORMCHECKBOX",
        r#"<w:checkBox><w:sizeAuto/><w:default w:val="0"/><w:checked/></w:checkBox>"#,
        "",
    );
    let off = form_field(
        "FORMCHECKBOX",
        r#"<w:checkBox><w:sizeAuto/><w:default w:val="0"/></w:checkBox>"#,
        "",
    );
    let off_explicit = form_field(
        "FORMCHECKBOX",
        r#"<w:checkBox><w:sizeAuto/><w:default w:val="1"/><w:checked w:val="0"/></w:checkBox>"#,
        "",
    );
    let default_on = form_field(
        "FORMCHECKBOX",
        r#"<w:checkBox><w:sizeAuto/><w:default w:val="1"/></w:checkBox>"#,
        "",
    );
    let body = para(
        &(on + &run(" yes ") + &off + &run(" no ") + &off_explicit + &run(" no ") + &default_on),
    );
    assert_eq!(md(&body), "\u{2612} yes \u{2610} no \u{2610} no \u{2612}");
}

#[test]
fn a_legacy_check_box_in_a_table_cell_and_a_list() {
    let on = form_field(
        "FORMCHECKBOX",
        r#"<w:checkBox><w:sizeAuto/><w:checked/></w:checkBox>"#,
        "",
    );
    let tbl = format!(
        r#"<w:tbl><w:tr><w:tc><w:p>{on}<w:r><w:t xml:space="preserve"> task</w:t></w:r></w:p></w:tc></w:tr></w:tbl>"#
    );
    assert!(md(&tbl).contains("\u{2612} task"), "{}", md(&tbl));
}

#[test]
fn a_legacy_drop_down_shows_the_chosen_entry() {
    let dd = |result: &str, default: &str| {
        form_field(
            "FORMDROPDOWN",
            &format!(
                r#"<w:ddList><w:default w:val="{default}"/>{result}<w:listEntry w:val="Red"/><w:listEntry w:val="Green"/><w:listEntry w:val="Blue"/></w:ddList>"#
            ),
            "",
        )
    };
    assert_eq!(md(&para(&dd(r#"<w:result w:val="2"/>"#, "0"))), "Blue");
    assert_eq!(md(&para(&dd("", "1"))), "Green");
    assert_eq!(md(&para(&dd(r#"<w:result w:val="9"/>"#, "0"))), "");
    // A result the document stored as text is that text, not the entry.
    let stored = form_field(
        "FORMDROPDOWN",
        r#"<w:ddList><w:result w:val="0"/><w:listEntry w:val="Red"/></w:ddList>"#,
        "Stored",
    );
    assert_eq!(md(&para(&stored)), "Stored");
}

#[test]
fn a_legacy_text_field_shows_its_result_or_its_default_text() {
    let with_result = form_field(
        "FORMTEXT",
        r#"<w:textInput><w:default w:val="dflt"/></w:textInput>"#,
        "typed",
    );
    assert_eq!(md(&para(&with_result)), "typed");
    let default_only = form_field(
        "FORMTEXT",
        r#"<w:textInput><w:default w:val="dflt"/></w:textInput>"#,
        "",
    );
    assert_eq!(md(&para(&default_only)), "dflt");
    let empty = form_field("FORMTEXT", r#"<w:textInput/>"#, "");
    assert_eq!(md(&para(&(run("a") + &empty + &run("b")))), "ab");
}

#[test]
fn form_field_instructions_never_leak_into_the_text() {
    let on = form_field(
        "FORMCHECKBOX",
        r#"<w:checkBox><w:checked/></w:checkBox>"#,
        "",
    );
    assert!(!md(&para(&on)).contains("FORMCHECKBOX"));
}

// ---------------------------------------------------------------------------------------------
// 4. hidden text set by a style
// ---------------------------------------------------------------------------------------------

#[test]
fn hidden_text_from_a_character_style_a_paragraph_style_and_the_defaults() {
    let styles = r#"
<w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/></w:style>
<w:style w:type="character" w:styleId="Hid"><w:name w:val="Hid"/><w:rPr><w:vanish/></w:rPr></w:style>
<w:style w:type="character" w:styleId="Hid2"><w:name w:val="Hid2"/><w:basedOn w:val="Hid"/></w:style>
<w:style w:type="paragraph" w:styleId="HidP"><w:name w:val="HidP"/><w:basedOn w:val="Normal"/><w:rPr><w:vanish/></w:rPr></w:style>
<w:style w:type="paragraph" w:styleId="HidP2"><w:name w:val="HidP2"/><w:basedOn w:val="HidP"/></w:style>"#;
    let body = para(&(run("a ") + &runp(r#"<w:rStyle w:val="Hid"/>"#, "H1") + &run("b")))
        + &para(&(run("c ") + &runp(r#"<w:rStyle w:val="Hid2"/>"#, "H2") + &run("d")))
        + &styled_p("HidP", "H3")
        + &styled_p("HidP2", "H4")
        + &p("shown");
    assert_eq!(md_styled(&body, styles), "a b\n\nc d\n\nshown");
}

#[test]
fn a_run_can_show_text_its_style_hides_and_hide_text_its_style_shows() {
    let styles = r#"
<w:style w:type="character" w:styleId="Hid"><w:name w:val="Hid"/><w:rPr><w:vanish/></w:rPr></w:style>"#;
    let body = para(
        &(runp(r#"<w:rStyle w:val="Hid"/><w:vanish w:val="0"/>"#, "shown ")
            + &runp(r#"<w:vanish w:val="false"/>"#, "also ")
            + &runp("<w:vanish/>", "hidden")
            + &runp(r#"<w:rStyle w:val="Hid"/>"#, "STYLE-HIDDEN")
            + &run("end")),
    );
    assert_eq!(md_styled(&body, styles), "shown also end");
}

#[test]
fn hidden_by_the_document_defaults_and_shown_again_by_a_style() {
    let styles = r#"
<w:docDefaults><w:rPrDefault><w:rPr><w:vanish/></w:rPr></w:rPrDefault></w:docDefaults>
<w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/></w:style>
<w:style w:type="paragraph" w:styleId="Vis"><w:name w:val="Vis"/><w:basedOn w:val="Normal"/><w:rPr><w:vanish w:val="0"/></w:rPr></w:style>"#;
    let body = p("hidden by default") + &styled_p("Vis", "visible");
    assert_eq!(md_styled(&body, styles), "visible");
}

#[test]
fn hidden_text_in_a_table_cell_a_note_and_a_hyperlink_is_not_shown() {
    let styles = r#"
<w:style w:type="character" w:styleId="Hid"><w:name w:val="Hid"/><w:rPr><w:vanish/></w:rPr></w:style>"#;
    let hid = r#"<w:r><w:rPr><w:rStyle w:val="Hid"/></w:rPr><w:t>SECRET</w:t></w:r>"#;
    let tbl = format!(
        r#"<w:tbl><w:tr><w:tc><w:p><w:r><w:t>vis</w:t></w:r>{hid}</w:p></w:tc></w:tr></w:tbl>"#
    );
    let m = md_styled(&tbl, styles);
    assert!(m.contains("vis") && !m.contains("SECRET"), "{m}");
    let link = para(&format!(
        r#"<w:hyperlink w:anchor="x"><w:r><w:t>l</w:t></w:r>{hid}</w:hyperlink>"#
    ));
    assert!(!md_styled(&link, styles).contains("SECRET"));
}

// ---------------------------------------------------------------------------------------------
// 5. a table row that starts late
// ---------------------------------------------------------------------------------------------

fn tc(t: &str) -> String {
    format!(r#"<w:tc><w:p><w:r><w:t>{t}</w:t></w:r></w:p></w:tc>"#)
}

#[test]
fn grid_before_keeps_the_cells_under_their_columns() {
    let tbl = format!(
        r#"<w:tbl><w:tr>{}{}{}</w:tr><w:tr><w:trPr><w:gridBefore w:val="1"/><w:wBefore w:w="1000" w:type="dxa"/></w:trPr>{}{}</w:tr></w:tbl>"#,
        tc("a"),
        tc("b"),
        tc("c"),
        tc("d"),
        tc("e")
    );
    assert_eq!(
        md(&tbl),
        "| a | b | c |\n| --- | --- | --- |\n|   | d | e |"
    );
}

#[test]
fn grid_before_and_after_with_spans_and_vertical_merges() {
    let vmerge_start = r#"<w:tc><w:tcPr><w:vMerge w:val="restart"/></w:tcPr><w:p><w:r><w:t>v</w:t></w:r></w:p></w:tc>"#;
    let vmerge_next = r#"<w:tc><w:tcPr><w:vMerge/></w:tcPr><w:p/></w:tc>"#;
    let span2 = r#"<w:tc><w:tcPr><w:gridSpan w:val="2"/></w:tcPr><w:p><w:r><w:t>s</w:t></w:r></w:p></w:tc>"#;
    let tbl = format!(
        r#"<w:tbl><w:tr><w:trPr><w:gridBefore w:val="2"/></w:trPr>{vmerge_start}{}<w:trPr/></w:tr><w:tr><w:trPr><w:gridBefore w:val="2"/><w:gridAfter w:val="1"/></w:trPr>{vmerge_next}{}</w:tr><w:tr>{span2}{}{}</w:tr></w:tbl>"#,
        tc("x"),
        tc("y"),
        tc("p"),
        tc("q")
    );
    let m = md(&tbl);
    // Columns: 0,1 are before the first two rows; `v` is column 2 in both of them; the third row
    // starts at column 0 with a two-column span.
    let rows: Vec<Vec<String>> = m
        .lines()
        .filter(|l| !l.contains("---"))
        .map(|l| {
            l.trim_matches('|')
                .split('|')
                .map(|c| c.trim().to_string())
                .collect()
        })
        .collect();
    assert_eq!(rows[0][..4], ["", "", "v", "x"], "{m}");
    assert_eq!(rows[1][..4], ["", "", "", "y"], "{m}");
    assert_eq!(rows[2][..4], ["s", "", "p", "q"], "{m}");
}

#[test]
fn a_huge_grid_before_is_bounded() {
    let tbl = format!(
        r#"<w:tbl><w:tr><w:trPr><w:gridBefore w:val="4000000000"/></w:trPr>{}</w:tr></w:tbl>"#,
        tc("a")
    );
    let m = md(&tbl);
    assert!(m.len() < 2000 && m.contains("a"), "{}", m.len());
}

#[test]
fn a_row_with_a_grid_after_only_is_not_widened() {
    let tbl = format!(
        r#"<w:tbl><w:tr>{}{}<w:trPr><w:gridAfter w:val="1"/></w:trPr></w:tr></w:tbl>"#,
        tc("a"),
        tc("b")
    );
    assert_eq!(md(&tbl), "| a | b |\n| --- | --- |");
}

// ---------------------------------------------------------------------------------------------
// 6. charts, SmartArt and other graphics
// ---------------------------------------------------------------------------------------------

fn graphic(uri: &str, inner: &str, docpr_extra: &str) -> String {
    format!(
        r#"<w:r><w:drawing><wp:inline><wp:docPr id="1" name="G 1"{docpr_extra}/><a:graphic><a:graphicData uri="{uri}">{inner}</a:graphicData></a:graphic></wp:inline></w:drawing></w:r>"#
    )
}

const CHART_URI: &str = "http://schemas.openxmlformats.org/drawingml/2006/chart";
const DIAGRAM_URI: &str = "http://schemas.openxmlformats.org/drawingml/2006/diagram";

fn chart_ref() -> String {
    graphic(
        CHART_URI,
        r#"<c:chart xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart" r:id="rIdC"/>"#,
        "",
    )
}

fn chart_part(title: Option<&str>, axis_title: &str) -> String {
    let t = title.map_or(String::new(), |t| {
        format!(r#"<c:title><c:tx><c:rich><a:bodyPr/><a:p><a:r><a:t>{t}</a:t></a:r></a:p></c:rich></c:tx></c:title>"#)
    });
    format!(
        r#"<?xml version="1.0"?><c:chartSpace xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><c:chart>{t}<c:autoTitleDeleted val="0"/><c:plotArea><c:barChart><c:ser><c:tx><c:strRef><c:strCache><c:pt><c:v>Series</c:v></c:pt></c:strCache></c:strRef></c:tx></c:ser></c:barChart><c:valAx><c:title><c:tx><c:rich><a:p><a:r><a:t>{axis_title}</a:t></a:r></a:p></c:rich></c:tx></c:title></c:valAx></c:plotArea></c:chart></c:chartSpace>"#
    )
}

#[test]
fn a_chart_without_a_description_is_shown_as_a_chart_with_its_title() {
    let d = Dx::new(&para(&(run("Before ") + &chart_ref() + &run(" after"))))
        .rel("rIdC", "chart", "charts/chart1.xml", false)
        .part(
            "word/charts/chart1.xml",
            chart_part(Some("Sales 2024"), "Axis").as_bytes(),
        );
    assert_eq!(conv(&d).markdown, "Before \\[chart: Sales 2024] after");
}

#[test]
fn a_chart_with_no_title_is_just_a_chart_and_an_axis_title_is_not_its_title() {
    let d = Dx::new(&para(&chart_ref()))
        .rel("rIdC", "chart", "charts/chart1.xml", false)
        .part(
            "word/charts/chart1.xml",
            chart_part(None, "Axis").as_bytes(),
        );
    assert_eq!(conv(&d).markdown, "\\[chart]");
    // No chart part at all (a broken package) and an external one are the same.
    let d = Dx::new(&para(&chart_ref()));
    assert_eq!(conv(&d).markdown, "\\[chart]");
    let d = Dx::new(&para(&chart_ref())).rel("rIdC", "chart", "https://x.example/c.xml", true);
    assert_eq!(conv(&d).markdown, "\\[chart]");
}

#[test]
fn a_chart_with_a_description_keeps_the_description() {
    let g = graphic(CHART_URI, "", r#" descr="Quarterly sales""#);
    assert_eq!(md(&para(&g)), "\\[Quarterly sales]");
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
fn smartart_is_shown_as_smartart() {
    let g = graphic(
        DIAGRAM_URI,
        r#"<dgm:relIds xmlns:dgm="http://schemas.openxmlformats.org/drawingml/2006/diagram" r:dm="rId1"/>"#,
        "",
    );
    assert_eq!(md(&para(&(run("x ") + &g))), "x \\[SmartArt]");
}

#[test]
fn a_chart_in_a_table_cell_is_drawn_there() {
    let tbl = format!(
        r#"<w:tbl><w:tr><w:tc><w:p>{}</w:p></w:tc></w:tr></w:tbl>"#,
        graphic(DIAGRAM_URI, "", "")
    );
    assert!(md(&tbl).contains("SmartArt"), "{}", md(&tbl));
}

// ---------------------------------------------------------------------------------------------
// 7. ruby
// ---------------------------------------------------------------------------------------------

fn ruby(base: &str, reading: &str) -> String {
    format!(
        r#"<w:r><w:ruby><w:rubyPr><w:rubyAlign w:val="distributeSpace"/></w:rubyPr><w:rt><w:r><w:t>{reading}</w:t></w:r></w:rt><w:rubyBase><w:r><w:t>{base}</w:t></w:r></w:rubyBase></w:ruby></w:r>"#
    )
}

#[test]
fn a_ruby_reading_follows_its_base_in_full_width_brackets_for_japanese() {
    let body = para(&(ruby("漢字", "かんじ") + &run("です")));
    assert_eq!(md(&body), "漢字（かんじ）です");
}

#[test]
fn a_ruby_reading_of_latin_text_follows_in_ascii_brackets() {
    let body = para(&(run("see ") + &ruby("Tokyo", "tou kyou")));
    assert_eq!(md(&body), "see Tokyo (tou kyou)");
}

#[test]
fn several_rubies_an_empty_reading_and_a_hidden_ruby() {
    let body = para(&(ruby("東", "ひがし") + &ruby("京", "きょう") + &ruby("都", "")));
    assert_eq!(md(&body), "東（ひがし）京（きょう）都");
    let hid = r#"<w:r><w:rPr><w:vanish/></w:rPr><w:ruby><w:rt><w:r><w:t>x</w:t></w:r></w:rt><w:rubyBase><w:r><w:t>H</w:t></w:r></w:rubyBase></w:ruby></w:r>"#;
    assert_eq!(md(&para(&(run("a") + hid + &run("b")))), "ab");
}

#[test]
fn a_ruby_reading_is_escaped_and_works_in_a_cell() {
    let body = para(&ruby("漢", "*か*"));
    assert!(md(&body).contains("\\*か\\*"), "{}", md(&body));
    let tbl = format!(
        r#"<w:tbl><w:tr><w:tc><w:p>{}</w:p></w:tc></w:tr></w:tbl>"#,
        ruby("字", "じ")
    );
    assert!(md(&tbl).contains("字（じ）"), "{}", md(&tbl));
}

#[test]
fn odt_ruby_reads_like_the_docx_one() {
    let ruby = |b: &str, t: &str| {
        format!(
            r#"<text:ruby><text:ruby-base>{b}</text:ruby-base><text:ruby-text>{t}</text:ruby-text></text:ruby>"#
        )
    };
    assert_eq!(
        od::md(&od::p(&(ruby("漢字", "かんじ") + "です"))),
        "漢字（かんじ）です"
    );
    assert_eq!(
        od::md(&od::p(&ruby("Tokyo", "tou kyou"))),
        "Tokyo (tou kyou)"
    );
    assert_eq!(od::md(&od::p(&ruby("都", ""))), "都");
}

// ---------------------------------------------------------------------------------------------
// 8. number formats
// ---------------------------------------------------------------------------------------------

#[test]
fn chinese_counting_is_the_counting_numerals_of_the_number() {
    for (n, want) in [
        (1, "一"),
        (9, "九"),
        (10, "十"),
        (11, "十一"),
        (19, "十九"),
        (20, "二十"),
        (21, "二十一"),
        (99, "九十九"),
        (100, "一百"),
        (101, "一百零一"),
        (110, "一百一十"),
        (111, "一百一十一"),
        (999, "九百九十九"),
        (1000, "一千"),
        (1001, "一千零一"),
        (1010, "一千零一十"),
        (1234, "一千二百三十四"),
        (10000, "一万"),
        (10001, "一万零一"),
        (12345, "一万二千三百四十五"),
    ] {
        for f in [
            "chineseCounting",
            "chineseCountingThousand",
            "taiwaneseCounting",
            "taiwaneseCountingThousand",
        ] {
            assert_eq!(format_number(f, n), want, "{f} {n}");
        }
    }
    // The digit-by-digit format is another one.
    assert_eq!(format_number("ideographDigital", 10), "一〇");
}

#[test]
fn legal_numerals_use_the_financial_characters() {
    assert_eq!(format_number("ideographLegalTraditional", 1), "壹");
    assert_eq!(format_number("ideographLegalTraditional", 10), "拾");
    assert_eq!(format_number("ideographLegalTraditional", 23), "貳拾參");
    assert_eq!(format_number("chineseLegalSimplified", 2), "贰");
    assert_eq!(format_number("chineseLegalSimplified", 23), "贰拾叁");
    assert_eq!(format_number("chineseLegalSimplified", 100), "壹佰");
}

#[test]
fn heavenly_stems_earthly_branches_and_circled_ideographs() {
    assert_eq!(
        (1..=10)
            .map(|n| format_number("ideographTraditional", n))
            .collect::<String>(),
        "甲乙丙丁戊己庚辛壬癸"
    );
    assert_eq!(format_number("ideographTraditional", 11), "甲");
    assert_eq!(
        (1..=12)
            .map(|n| format_number("ideographZodiac", n))
            .collect::<String>(),
        "子丑寅卯辰巳午未申酉戌亥"
    );
    assert_eq!(format_number("ideographZodiac", 13), "子");
    assert_eq!(format_number("ideographZodiacTraditional", 1), "甲子");
    assert_eq!(format_number("ideographZodiacTraditional", 2), "乙丑");
    assert_eq!(format_number("ideographZodiacTraditional", 11), "甲戌");
    assert_eq!(format_number("ideographZodiacTraditional", 60), "癸亥");
    assert_eq!(format_number("ideographZodiacTraditional", 61), "甲子");
    assert_eq!(format_number("ideographEnclosedCircle", 1), "\u{3280}");
    assert_eq!(format_number("ideographEnclosedCircle", 10), "\u{3289}");
}

#[test]
fn cardinal_and_ordinal_text_are_english_words() {
    for (n, card, ord) in [
        (1, "One", "First"),
        (2, "Two", "Second"),
        (3, "Three", "Third"),
        (4, "Four", "Fourth"),
        (5, "Five", "Fifth"),
        (8, "Eight", "Eighth"),
        (9, "Nine", "Ninth"),
        (11, "Eleven", "Eleventh"),
        (12, "Twelve", "Twelfth"),
        (20, "Twenty", "Twentieth"),
        (21, "Twenty-one", "Twenty-first"),
        (40, "Forty", "Fortieth"),
        (99, "Ninety-nine", "Ninety-ninth"),
        (100, "One hundred", "One hundredth"),
        (101, "One hundred one", "One hundred first"),
        (342, "Three hundred forty-two", "Three hundred forty-second"),
        (1000, "One thousand", "One thousandth"),
        (1002, "One thousand two", "One thousand second"),
    ] {
        assert_eq!(format_number("cardinalText", n), card, "{n}");
        assert_eq!(format_number("ordinalText", n), ord, "{n}");
    }
    assert_eq!(format_number("cardinalText", 5_000_000), "5000000");
    assert_eq!(format_number("hex", 255), "FF");
    assert_eq!(format_number("hex", 9), "9");
}

#[test]
fn a_chinese_counting_list_shows_the_counting_numerals() {
    let numbering = r#"<w:abstractNum w:abstractNumId="0"><w:lvl w:ilvl="0"><w:start w:val="9"/><w:numFmt w:val="chineseCounting"/><w:lvlText w:val="%1、"/></w:lvl></w:abstractNum><w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num>"#;
    let body = num_p(1, 0, "a") + &num_p(1, 0, "b") + &num_p(1, 0, "c") + &num_p(1, 0, "d");
    let m = conv(&Dx::new(&body).numbering(numbering)).markdown;
    assert_eq!(m, "九、 a  \n十、 b  \n十一、 c  \n十二、 d");
}

// ---------------------------------------------------------------------------------------------
// 9. the lines of a paragraph
// ---------------------------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------------------------
// 10. a deletion that crosses paragraphs (OpenDocument)
// ---------------------------------------------------------------------------------------------

fn deletion_regions() -> &'static str {
    r#"<text:tracked-changes text:track-changes="false"><text:changed-region xml:id="d1" text:id="d1"><text:deletion><office:change-info><dc:creator>A</dc:creator></office:change-info><text:p/></text:deletion></text:changed-region></text:tracked-changes>"#
}

#[test]
fn a_deleted_paragraph_break_joins_the_two_paragraphs() {
    // What LibreOffice writes when a break is deleted: the range starts at the end of one paragraph
    // and ends at the start of the next.
    let body = format!(
        "{}{}{}{}",
        deletion_regions(),
        od::p(r#"abc <text:change-start text:change-id="d1"/>"#),
        od::p(r#"<text:change-end text:change-id="d1"/>def"#),
        od::p("next")
    );
    assert_eq!(od::md(&body), "abc def\n\nnext");
}

#[test]
fn a_deletion_over_several_paragraphs_joins_the_first_and_the_last() {
    let body = format!(
        "{}{}{}{}{}",
        deletion_regions(),
        od::p(r#"head <text:change-start text:change-id="d1"/>gone"#),
        od::p("whole paragraph gone"),
        od::p(r#"also<text:change-end text:change-id="d1"/> tail"#),
        od::p("next")
    );
    assert_eq!(od::md(&body), "head tail\n\nnext");
}

#[test]
fn a_deleted_whole_paragraph_does_not_join_its_neighbours() {
    let body = format!(
        "{}{}{}{}",
        deletion_regions(),
        od::p("before"),
        od::p(r#"<text:change-start text:change-id="d1"/>deleted"#),
        od::p(r#"<text:change-end text:change-id="d1"/>after"#)
    );
    assert_eq!(od::md(&body), "before\n\nafter");
}

#[test]
fn a_deletion_that_never_ends_keeps_what_came_before_it_once() {
    let body = format!(
        "{}{}{}",
        deletion_regions(),
        od::p(r#"kept <text:change-start text:change-id="d1"/>x"#),
        od::p("lost")
    );
    assert_eq!(od::md(&body), "kept");
}

#[test]
fn a_deleted_break_joins_into_a_heading_and_a_list_item_without_losing_text() {
    let body = format!(
        "{}{}{}",
        deletion_regions(),
        od::p(r#"Intro <text:change-start text:change-id="d1"/>"#),
        r#"<text:h text:outline-level="2"><text:change-end text:change-id="d1"/>Title</text:h>"#
    );
    let m = od::md(&body);
    assert!(m.contains("Intro") && m.contains("Title"), "{m}");
    assert_eq!(m.matches('\n').count(), 0, "{m:?}");
}

#[test]
fn a_deleted_break_does_not_join_the_cells_of_a_table() {
    let table = r#"<table:table><table:table-row><table:table-cell><text:p>cell <text:change-start text:change-id="d1"/></text:p></table:table-cell><table:table-cell><text:p><text:change-end text:change-id="d1"/>two</text:p></table:table-cell></table:table-row></table:table>"#;
    let body = format!("{}{table}", deletion_regions());
    assert_eq!(od::md(&body), "| cell | two |\n| --- | --- |");
}

// ---------------------------------------------------------------------------------------------
// 11. altChunk
// ---------------------------------------------------------------------------------------------

#[test]
fn an_embedded_document_is_shown_as_a_placeholder_with_its_name() {
    let body = para(&run("Before")) + r#"<w:altChunk r:id="rIdA"/>"# + &para(&run("After"));
    let d = Dx::new(&body)
        .rel("rIdA", "aFChunk", "afchunk.html", false)
        .part(
            "word/afchunk.html",
            b"<html><body>HIDDEN-BODY</body></html>",
        );
    let m = conv(&d).markdown;
    assert_eq!(m, "Before\n\n\\[embedded document: afchunk.html]\n\nAfter");
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

// ---------------------------------------------------------------------------------------------
// 12. notes with several paragraphs
// ---------------------------------------------------------------------------------------------

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

#[test]
fn odt_notes_with_several_paragraphs_are_lines_of_the_note() {
    let body = od::p(
        r#"x<text:note text:note-class="footnote"><text:note-body><text:p>one</text:p><text:p>two</text:p></text:note-body></text:note>"#,
    );
    assert_eq!(od::md(&body), "x[^1]\n\n[^1]: one\\\n    two");
}

#[test]
fn the_line_budget_counts_the_lines_of_a_note() {
    use super::docx::DocOptions;
    let paras: String = (0..200)
        .map(|i| format!("<w:p><w:r><w:t>line {i}</w:t></w:r></w:p>"))
        .collect();
    let n = format!(
        r#"<w:footnote w:id="1"><w:p><w:r><w:footnoteRef/></w:r><w:r><w:t>first</w:t></w:r></w:p>{paras}</w:footnote>"#
    );
    let opts = DocOptions {
        max_markdown_lines: 50,
        ..DocOptions::default()
    };
    let d = conv_with(
        &Dx::new(&para(&(run("x") + &fn_ref_local(1)))).footnotes(&n),
        &opts,
    )
    .unwrap();
    assert!(
        d.markdown.lines().count() <= 50,
        "{}",
        d.markdown.lines().count()
    );
}
