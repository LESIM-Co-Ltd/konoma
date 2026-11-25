//! Tests added after a mutation audit of the Word / OpenDocument text preview: each one pins a
//! rule that a mutated build (a flipped flag, an off-by-one bound, a dropped branch) used to get
//! away with. The comment on each test says what it catches. Expected values come from the file
//! format rules (ECMA-376 / OpenDocument / OMML) and from what Word and LibreOffice show.

use super::omml::to_latex as omml_latex;
use super::tests_docx::{conv, md, md_styled, p, para, run, runp, styled_p, tiny_png, Dx};
use super::tests_odt::{conv as oconv, md_auto, p as op, span, text_style, Ox};

fn fld(instr: &str, result: &str) -> String {
    format!(
        r#"<w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> {instr} </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>{result}</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r>"#
    )
}

// ---------------------------------------------------------------------------------------------
// Word: fields, links, anchors
// ---------------------------------------------------------------------------------------------

/// A complex `HYPERLINK` field ends at its `end` mark: the text after it is not part of the link
/// (catches `link_open` never being set, so the closing `)` was never written).
#[test]
fn text_after_a_complex_hyperlink_field_is_outside_the_link() {
    let body = para(&(fld(r#"HYPERLINK "https://a.test""#, "L") + &run(" tail")));
    assert_eq!(md(&body), "[L](https://a.test) tail");
}

/// `HYPERLINK "url" \l "section"` is the url plus `#section` (catches the `#` being dropped).
#[test]
fn a_hyperlink_field_with_url_and_bookmark_joins_them_with_a_hash() {
    let body = para(&fld(r#"HYPERLINK "https://x.test/p" \l "sec""#, "t"));
    assert_eq!(md(&body), "[t](https://x.test/p#sec)");
}

/// An OpenDocument cross reference to a heading nobody registered (`#Missing|outline`) loses the
/// `|outline` suffix and is slugged like a heading (catches the suffix being kept).
#[test]
fn an_unregistered_outline_reference_is_slugged_without_its_suffix() {
    let m = oconv(&Ox::new(&op(
        r##"<text:a xlink:href="#Missing|outline">x</text:a>"##,
    )))
    .markdown;
    assert_eq!(m, "[x](#missing)");
}

// ---------------------------------------------------------------------------------------------
// Word: paragraphs and headings
// ---------------------------------------------------------------------------------------------

/// A paragraph mark moved away (`w:moveFrom`) joins the next paragraph exactly like a deleted one
/// (catches `moveFrom` being ignored: the two paragraphs would stay apart).
#[test]
fn a_moved_away_paragraph_mark_joins_the_next_paragraph() {
    let a = r#"<w:p><w:pPr><w:rPr><w:moveFrom w:id="1"/></w:rPr></w:pPr><w:r><w:t xml:space="preserve">joined </w:t></w:r></w:p>"#;
    assert_eq!(md(&(a.to_string() + &p("with next"))), "joined with next");
}

/// A direct `outlineLvl` of 8 is the ninth level (clamped to `######`), 9 is body text (catches
/// the bound `v < 9` moving to `v < 8`).
#[test]
fn a_direct_outline_level_of_8_is_a_heading_and_9_is_body_text() {
    let lvl = |v: u8| {
        format!(r#"<w:p><w:pPr><w:outlineLvl w:val="{v}"/></w:pPr><w:r><w:t>x</w:t></w:r></w:p>"#)
    };
    assert_eq!(md(&lvl(8)), "###### x");
    assert_eq!(md(&lvl(9)), "x");
}

/// A heading whose style is italic (as Word's own Heading styles often are) does not wrap the
/// text in `*`: the heading already carries the look (catches the italic reset being dropped).
#[test]
fn heading_italic_from_the_style_is_not_repeated() {
    let styles = r#"<w:style w:type="paragraph" w:styleId="HI"><w:name w:val="heading 1"/><w:pPr><w:outlineLvl w:val="0"/></w:pPr><w:rPr><w:i/></w:rPr></w:style>"#;
    assert_eq!(md_styled(&styled_p("HI", "Title"), styles), "# Title");
}

// ---------------------------------------------------------------------------------------------
// Word: escaping and emphasis edges
// ---------------------------------------------------------------------------------------------

/// A bold run ending in punctuation, followed by a punctuation-only run, keeps its closing mark
/// after the punctuation (`**A.**。`): a closing `**` must follow punctuation only when the next
/// character is not itself punctuation (catches the trailing flank test losing that condition).
#[test]
fn bold_ending_in_punctuation_before_more_punctuation_keeps_the_mark_outside() {
    let body = para(&(runp("<w:b/>", "A.") + &run("。")));
    assert_eq!(md(&body), "**A.**。");
}

/// `123456789. x` would start an ordered list (up to nine digits): the dot is escaped. Ten digits
/// is not a list marker and stays as typed (catches the `<= 9` bound).
#[test]
fn a_nine_digit_number_before_a_dot_is_escaped_and_ten_digits_are_not() {
    assert_eq!(md(&p("123456789. x")), "123456789\\. x");
    assert_eq!(md(&p("1234567890. x")), "1234567890. x");
}

// ---------------------------------------------------------------------------------------------
// Word: styles
// ---------------------------------------------------------------------------------------------

/// `w:dstrike w:val="0"` switches off a strike-through the style set (catches the second operand
/// of the off-switch being lost).
#[test]
fn dstrike_val_0_turns_the_style_strike_off() {
    let styles = r#"<w:style w:type="paragraph" w:styleId="S"><w:name w:val="S"/><w:rPr><w:strike/></w:rPr></w:style>"#;
    let body = r#"<w:p><w:pPr><w:pStyle w:val="S"/></w:pPr><w:r><w:rPr><w:dstrike w:val="0"/></w:rPr><w:t>t</w:t></w:r></w:p>"#;
    assert_eq!(md_styled(body, styles), "t");
    // Control: without the switch the style's strike shows.
    assert_eq!(md_styled(&styled_p("S", "t"), styles), "~~t~~");
}

/// A run's `rStyle` may name a paragraph-type style (Word allows it): its bold applies (catches
/// the `paragraph` kind being excluded from `char_fmt`).
#[test]
fn a_run_style_naming_a_paragraph_style_still_applies_its_emphasis() {
    let styles = r#"<w:style w:type="paragraph" w:styleId="PB"><w:name w:val="PB"/><w:rPr><w:b/></w:rPr></w:style>"#;
    let body = para(r#"<w:r><w:rPr><w:rStyle w:val="PB"/></w:rPr><w:t>t</w:t></w:r>"#);
    assert_eq!(md_styled(&body, styles), "**t**");
}

/// A paragraph in a style named `Plain Text` is code (LibreOffice / Word's monospace default;
/// catches the name being dropped from the list).
#[test]
fn a_plain_text_paragraph_style_is_a_code_block() {
    let styles =
        r#"<w:style w:type="paragraph" w:styleId="PT"><w:name w:val="Plain Text"/></w:style>"#;
    assert_eq!(
        md_styled(&styled_p("PT", "x = 1"), styles),
        "```\nx = 1\n```"
    );
}

// ---------------------------------------------------------------------------------------------
// Word: pictures
// ---------------------------------------------------------------------------------------------

/// A `.webp` picture is a picture: it is registered and linked (catches `webp` leaving the
/// accepted extensions).
#[test]
fn a_webp_picture_is_loaded_as_an_image() {
    let drawing = r#"<w:r><w:drawing><wp:inline><wp:docPr id="1" name="P" descr="alt"/><a:graphic><a:graphicData><pic:pic><pic:blipFill><a:blip r:embed="rId9"/></pic:blipFill></pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing></w:r>"#;
    let d = conv(
        &Dx::new(&para(drawing))
            .rel("rId9", "image", "media/image1.webp", false)
            .media("image1.webp", tiny_png(3)),
    );
    assert_eq!(d.images.len(), 1);
    assert_eq!(d.images[0].name, "image1.webp");
    assert!(
        d.images[0].key.ends_with("/image1.webp"),
        "{}",
        d.images[0].key
    );
    assert_eq!(d.markdown, format!("![alt]({})", d.images[0].key));
}

// ---------------------------------------------------------------------------------------------
// OpenDocument text
// ---------------------------------------------------------------------------------------------

/// `fo:font-weight` is bold from 600 up: 600 is bold, 599 is not (catches `>=` becoming `>`).
#[test]
fn font_weight_600_is_bold_and_599_is_not() {
    let auto = text_style("W600", r#"fo:font-weight="600""#, None)
        + &text_style("W599", r#"fo:font-weight="599""#, None);
    let body = op(&format!("{} {}", span("W600", "six"), span("W599", "five")));
    assert_eq!(md_auto(&body, &auto), "**six** five");
}

/// A run of empty repeated rows (a table padded to the size of a sheet) is one row and does not
/// make the document "truncated" (catches the empty-row collapse being switched off).
#[test]
fn a_long_run_of_empty_repeated_rows_is_one_row_and_not_a_truncation() {
    let t = r#"<table:table><table:table-row><table:table-cell><text:p>a</text:p></table:table-cell></table:table-row><table:table-row table:number-rows-repeated="500"><table:table-cell/></table:table-row></table:table>"#;
    let d = oconv(&Ox::new(t));
    assert!(!d.truncated, "an empty run is not a budget overrun");
    assert_eq!(
        d.markdown.lines().count(),
        3,
        "the row `a`, the rule, one empty row: {}",
        d.markdown
    );
}

// ---------------------------------------------------------------------------------------------
// OMML formulas
// ---------------------------------------------------------------------------------------------

fn omml(inner: &str) -> String {
    format!(
        r#"<m:oMath xmlns:m="http://schemas.openxmlformats.org/officeDocument/2006/math">{inner}</m:oMath>"#
    )
}

fn mr(t: &str) -> String {
    format!("<m:r><m:t>{t}</m:t></m:r>")
}

/// `m:degHide m:val="off"` is ST_OnOff false: the degree stays (catches `off` being read as true).
#[test]
fn deg_hide_off_keeps_the_root_degree() {
    let f = omml(&format!(
        r#"<m:rad><m:radPr><m:degHide m:val="off"/></m:radPr><m:deg>{}</m:deg><m:e>{}</m:e></m:rad>"#,
        mr("3"),
        mr("x")
    ));
    assert_eq!(omml_latex(&f, false).as_deref(), Some(r"\sqrt[3]{x}"));
}

/// Every integral sign U+222B..U+2233 keeps its limits beside it under `limLoc=subSup` (no
/// `\nolimits`), the last of them (the anticlockwise contour integral) included (catches the
/// range ending one short).
#[test]
fn sub_sup_integrals_never_get_nolimits_up_to_u2233() {
    for ch in ['\u{222B}', '\u{222C}', '\u{222E}', '\u{2232}', '\u{2233}'] {
        let f = omml(&format!(
            r#"<m:nary><m:naryPr><m:chr m:val="{ch}"/><m:limLoc m:val="subSup"/></m:naryPr><m:sub>{}</m:sub><m:sup>{}</m:sup><m:e>{}</m:e></m:nary>"#,
            mr("0"),
            mr("1"),
            mr("f")
        ));
        let l = omml_latex(&f, false).unwrap_or_default();
        assert!(!l.contains("nolimits"), "U+{:04X}: {l}", ch as u32);
    }
}
