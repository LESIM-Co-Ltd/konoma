//! Tests of the OpenDocument text reader (`odt.rs`): paragraphs, styles, tables, pictures, notes,
//! links, frames, tracked changes and the documents LibreOffice wrote. Lists, formulas, packages
//! and hostile input are in `tests_odt_more.rs`.

use std::path::PathBuf;

use super::docx::*;
use super::tests::{deflated, tmp, write};
use super::tests_docx::{tiny_png, visible};
use super::OfficeError;

// ---------------------------------------------------------------------------------------------
// builders
// ---------------------------------------------------------------------------------------------

pub(super) const MIME_TEXT: &str = "application/vnd.oasis.opendocument.text";

pub(super) fn ns() -> String {
    [
        ("office", "urn:oasis:names:tc:opendocument:xmlns:office:1.0"),
        ("text", "urn:oasis:names:tc:opendocument:xmlns:text:1.0"),
        ("table", "urn:oasis:names:tc:opendocument:xmlns:table:1.0"),
        ("draw", "urn:oasis:names:tc:opendocument:xmlns:drawing:1.0"),
        ("style", "urn:oasis:names:tc:opendocument:xmlns:style:1.0"),
        (
            "fo",
            "urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0",
        ),
        (
            "svg",
            "urn:oasis:names:tc:opendocument:xmlns:svg-compatible:1.0",
        ),
        ("xlink", "http://www.w3.org/1999/xlink"),
        ("dc", "http://purl.org/dc/elements/1.1/"),
        ("xml", "http://www.w3.org/XML/1998/namespace"),
        (
            "loext",
            "urn:org:documentfoundation:names:experimental:office:xmlns:loext:1.0",
        ),
    ]
    .iter()
    .filter(|(p, _)| *p != "xml")
    .map(|(p, u)| format!("xmlns:{p}=\"{u}\""))
    .collect::<Vec<_>>()
    .join(" ")
}

/// An OpenDocument text package under construction.
#[derive(Clone)]
pub(super) struct Ox {
    pub body: String,
    /// Inside `office:automatic-styles` of `content.xml`.
    pub auto: String,
    /// Inside `office:styles` of `styles.xml`.
    pub styles: Option<String>,
    /// Inside `office:master-styles` of `styles.xml` (headers and footers).
    pub master: Option<String>,
    pub mime: String,
    pub manifest: Option<String>,
    pub extra: Vec<(String, Vec<u8>)>,
}

impl Ox {
    pub fn new(body: &str) -> Ox {
        Ox {
            body: body.to_string(),
            auto: String::new(),
            styles: None,
            master: None,
            mime: MIME_TEXT.to_string(),
            manifest: None,
            extra: Vec::new(),
        }
    }
    pub fn auto(mut self, s: &str) -> Ox {
        self.auto = s.to_string();
        self
    }
    pub fn styles(mut self, s: &str) -> Ox {
        self.styles = Some(s.to_string());
        self
    }
    pub fn master(mut self, s: &str) -> Ox {
        self.master = Some(s.to_string());
        self
    }
    pub fn mime(mut self, m: &str) -> Ox {
        self.mime = m.to_string();
        self
    }
    pub fn manifest(mut self, m: &str) -> Ox {
        self.manifest = Some(m.to_string());
        self
    }
    pub fn part(mut self, name: &str, bytes: &[u8]) -> Ox {
        self.extra.push((name.to_string(), bytes.to_vec()));
        self
    }
    pub fn picture(self, name: &str, seed: u8) -> Ox {
        self.part(&format!("Pictures/{name}"), &tiny_png(seed))
    }
    /// A formula object `dir` holding MathML `xml`.
    pub fn object(self, dir: &str, xml: &str) -> Ox {
        self.part(&format!("{dir}/content.xml"), xml.as_bytes())
    }
    pub fn content_xml(&self) -> String {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?><office:document-content {} office:version="1.3"><office:automatic-styles>{}</office:automatic-styles><office:body><office:text>{}</office:text></office:body></office:document-content>"#,
            ns(),
            self.auto,
            self.body
        )
    }
    pub fn entries(&self) -> Vec<(String, Vec<u8>)> {
        let mut e: Vec<(String, Vec<u8>)> = vec![
            ("mimetype".into(), self.mime.clone().into_bytes()),
            (
                "META-INF/manifest.xml".into(),
                self.manifest
                    .clone()
                    .unwrap_or_else(|| {
                        r#"<?xml version="1.0"?><manifest:manifest xmlns:manifest="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0"/>"#.to_string()
                    })
                    .into_bytes(),
            ),
            ("content.xml".into(), self.content_xml().into_bytes()),
        ];
        if self.styles.is_some() || self.master.is_some() {
            e.push((
                "styles.xml".into(),
                format!(
                    r#"<?xml version="1.0" encoding="UTF-8"?><office:document-styles {}><office:styles>{}</office:styles><office:master-styles>{}</office:master-styles></office:document-styles>"#,
                    ns(),
                    self.styles.clone().unwrap_or_default(),
                    self.master.clone().unwrap_or_default()
                )
                .into_bytes(),
            ));
        }
        e.extend(self.extra.iter().cloned());
        e
    }
    pub fn bytes(&self) -> Vec<u8> {
        let e = self.entries();
        let refs: Vec<(&str, &[u8])> = e.iter().map(|(n, b)| (n.as_str(), b.as_slice())).collect();
        deflated(&refs)
    }
}

/// The default options with `f` applied (a struct update would need each field named at the call).
pub(super) fn with_opts(f: impl FnOnce(&mut DocOptions)) -> DocOptions {
    let mut o = DocOptions::default();
    f(&mut o);
    o
}

pub(super) fn conv_with(o: &Ox, opts: &DocOptions) -> Result<Document, OfficeError> {
    let dir = tmp("odt");
    let p = write(&dir, "t.odt", &o.bytes());
    load_document(&p, opts)
}

pub(super) fn conv(o: &Ox) -> Document {
    conv_with(o, &DocOptions::default()).unwrap()
}

/// The Markdown of a body.
pub(super) fn md(body: &str) -> String {
    conv(&Ox::new(body)).markdown
}

/// The Markdown of a body with automatic styles.
pub(super) fn md_auto(body: &str, auto: &str) -> String {
    conv(&Ox::new(body).auto(auto)).markdown
}

pub(super) fn p(text: &str) -> String {
    format!("<text:p>{text}</text:p>")
}

pub(super) fn sp(style: &str, text: &str) -> String {
    format!(r#"<text:p text:style-name="{style}">{text}</text:p>"#)
}

pub(super) fn span(style: &str, text: &str) -> String {
    format!(r#"<text:span text:style-name="{style}">{text}</text:span>"#)
}

pub(super) fn text_style(name: &str, props: &str, parent: Option<&str>) -> String {
    let parent = parent
        .map(|p| format!(r#" style:parent-style-name="{p}""#))
        .unwrap_or_default();
    format!(
        r#"<style:style style:name="{name}" style:family="text"{parent}><style:text-properties {props}/></style:style>"#
    )
}

pub(super) fn para_style(name: &str, props: &str, parent: Option<&str>, extra: &str) -> String {
    let parent = parent
        .map(|p| format!(r#" style:parent-style-name="{p}""#))
        .unwrap_or_default();
    format!(
        r#"<style:style style:name="{name}" style:family="paragraph"{parent}{extra}><style:text-properties {props}/></style:style>"#
    )
}

const BOLD: &str = r#"fo:font-weight="bold""#;
const ITALIC: &str = r#"fo:font-style="italic""#;
const STRIKE: &str =
    r#"style:text-line-through-style="solid" style:text-line-through-type="single""#;

// ---------------------------------------------------------------------------------------------
// paragraphs, headings, emphasis
// ---------------------------------------------------------------------------------------------

#[test]
fn a_paragraph_is_a_paragraph_and_paragraphs_are_separated_by_blank_lines() {
    assert_eq!(md(&(p("one") + &p("two"))), "one\n\ntwo");
}

#[test]
fn empty_and_whitespace_paragraphs_vanish() {
    assert_eq!(
        md(&(p("one") + &p("") + &p("   ") + "<text:p/>" + &p("two"))),
        "one\n\ntwo"
    );
}

#[test]
fn text_h_is_a_heading_at_its_outline_level() {
    let body = (1..=7)
        .map(|n| format!(r#"<text:h text:outline-level="{n}">H{n}</text:h>"#))
        .collect::<String>();
    // Deeper than six is clamped (Markdown has no seventh level).
    assert_eq!(
        md(&body),
        "# H1\n\n## H2\n\n### H3\n\n#### H4\n\n##### H5\n\n###### H6\n\n###### H7"
    );
}

#[test]
fn a_heading_without_a_level_is_level_one_and_a_bad_level_is_clamped() {
    assert_eq!(md("<text:h>A</text:h>"), "# A");
    assert_eq!(md(r#"<text:h text:outline-level="0">A</text:h>"#), "# A");
    assert_eq!(md(r#"<text:h text:outline-level="x">A</text:h>"#), "# A");
    assert_eq!(
        md(r#"<text:h text:outline-level="99">A</text:h>"#),
        "###### A"
    );
}

#[test]
fn a_paragraph_whose_style_has_an_outline_level_is_a_heading() {
    let auto = r#"<style:style style:name="Heading_20_1" style:family="paragraph" style:default-outline-level="1"/><style:style style:name="Sub" style:family="paragraph" style:parent-style-name="Heading_20_1"/><style:style style:name="Deeper" style:family="paragraph" style:parent-style-name="Sub" style:default-outline-level="3"/>"#;
    assert_eq!(
        md_auto(
            &(sp("Heading_20_1", "one") + &sp("Sub", "inherits") + &sp("Deeper", "own")),
            auto
        ),
        "# one\n\n# inherits\n\n### own"
    );
}

#[test]
fn heading_by_style_name_in_english_and_japanese_without_an_outline_level() {
    let auto = r#"<style:style style:name="Heading_20_2" style:family="paragraph"/><style:style style:name="s3" style:display-name="見出し 3" style:family="paragraph"/><style:style style:name="Title" style:family="paragraph"/><style:style style:name="Heading" style:family="paragraph"/><style:style style:name="Table_20_Heading" style:family="paragraph"/>"#;
    assert_eq!(
        md_auto(
            &(sp("Heading_20_2", "two")
                + &sp("s3", "three")
                + &sp("Title", "doc title")
                + &sp("Heading", "not numbered")
                + &sp("Table_20_Heading", "not a heading")),
            auto
        ),
        "## two\n\n### three\n\n# doc title\n\nnot numbered\n\nnot a heading"
    );
}

#[test]
fn heading_emphasis_from_the_style_is_not_repeated_but_a_span_still_shows() {
    let auto = para_style("H", BOLD, None, r#" style:default-outline-level="1""#)
        + &text_style("T1", BOLD, None)
        + &text_style("T2", ITALIC, None);
    let body = format!(
        r#"<text:h text:style-name="H" text:outline-level="1">Plain {} and {}</text:h>"#,
        span("T1", "bold"),
        span("T2", "italic")
    );
    assert_eq!(md_auto(&body, &auto), "# Plain **bold** and *italic*");
}

#[test]
fn bold_italic_and_strike() {
    let auto = text_style("B", BOLD, None)
        + &text_style("I", ITALIC, None)
        + &text_style("S", STRIKE, None)
        + &text_style("BI", &format!("{BOLD} {ITALIC}"), None);
    let body = p(&format!(
        "a {} b {} c {} d {} e",
        span("B", "bold"),
        span("I", "italic"),
        span("S", "strike"),
        span("BI", "both")
    ));
    assert_eq!(
        md_auto(&body, &auto),
        "a **bold** b *italic* c ~~strike~~ d ***both*** e"
    );
}

#[test]
fn emphasis_comes_through_the_style_chain_and_can_be_switched_off() {
    let auto = text_style("B", BOLD, None)
        + &text_style("B2", ITALIC, Some("B"))
        + &text_style("NoB", r#"fo:font-weight="normal""#, Some("B2"))
        + &text_style("W700", r#"fo:font-weight="700""#, None)
        + &text_style("W500", r#"fo:font-weight="500""#, None)
        + &text_style("Asian", r#"style:font-weight-asian="bold""#, None)
        + &text_style("Obl", r#"fo:font-style="oblique""#, None)
        + &text_style(
            "StrikeNone",
            r#"style:text-line-through-style="none""#,
            Some("Strk"),
        )
        + &text_style("Strk", STRIKE, None)
        + &text_style("TypeOnly", r#"style:text-line-through-type="double""#, None);
    let body = p(&format!(
        "{} {} {} {} {} {} {} {} {}",
        span("B2", "inherits"),
        span("NoB", "off"),
        span("W700", "seven"),
        span("W500", "five"),
        span("Asian", "asian"),
        span("Obl", "oblique"),
        span("StrikeNone", "kept"),
        span("Strk", "gone"),
        span("TypeOnly", "dbl")
    ));
    assert_eq!(
        md_auto(&body, &auto),
        "***inherits*** *off* **seven** five **asian** *oblique* kept ~~gone~~ ~~dbl~~"
    );
}

#[test]
fn a_paragraph_style_gives_its_text_emphasis_and_a_span_overrides_it() {
    let auto = para_style("PB", BOLD, None, "")
        + &para_style("PB2", ITALIC, Some("PB"), "")
        + &text_style("Off", r#"fo:font-weight="normal""#, None);
    let body = sp("PB2", &format!("all {} rest", span("Off", "off")));
    assert_eq!(md_auto(&body, &auto), "***all*** *off* ***rest***");
}

#[test]
fn styles_in_styles_xml_apply_and_automatic_ones_may_be_based_on_them() {
    let named = para_style("Quote", ITALIC, None, "");
    let auto = r#"<style:style style:name="P1" style:family="paragraph" style:parent-style-name="Quote"/>"#;
    let o = Ox::new(&sp("P1", "quoted")).auto(auto).styles(&named);
    assert_eq!(conv(&o).markdown, "*quoted*");
}

#[test]
fn a_cyclic_or_missing_parent_does_not_hang_or_fail() {
    let auto = text_style("A", BOLD, Some("B"))
        + &text_style("B", ITALIC, Some("A"))
        + &text_style("C", BOLD, Some("Nope"));
    let body = p(&format!("{} {}", span("A", "x"), span("C", "y")));
    assert_eq!(md_auto(&body, &auto), "***x*** **y**");
    // A style that names itself as its parent.
    let auto = text_style("A", BOLD, Some("A"));
    assert_eq!(md_auto(&p(&span("A", "x")), &auto), "**x**");
    // A long chain is followed only so far, and does not overflow.
    let mut auto = String::new();
    for i in 0..200 {
        auto += &text_style(&format!("S{i}"), "", Some(&format!("S{}", i + 1)));
    }
    let _ = md_auto(&p(&span("S0", "x")), &auto);
}

#[test]
fn an_unknown_style_name_is_plain_text() {
    assert_eq!(md(&sp("Nope", &span("AlsoNope", "x"))), "x");
}

#[test]
fn whitespace_collapses_as_the_standard_says() {
    assert_eq!(md(&p("a   b\n\t c")), "a b c");
    assert_eq!(md(&p("   leading and trailing   ")), "leading and trailing");
    // Across element boundaries too.
    assert_eq!(
        md(&p("a <text:span>b</text:span> <text:span> c</text:span>")),
        "a b c"
    );
    // The pretty-printing between elements of a paragraph is a space.
    assert_eq!(md(&p("one\n<text:span>two</text:span>\n")), "one two");
}

#[test]
fn text_s_keeps_spaces_and_text_tab_is_a_tab() {
    assert_eq!(md(&p(r#"a<text:s text:c="3"/>b"#)), "a   b");
    assert_eq!(md(&p("a<text:s/>b")), "a b");
    // A huge count is capped, not allocated.
    let m = md(&p(r#"a<text:s text:c="99999999999"/>b"#));
    assert!(m.len() < 400, "{}", m.len());
    assert_eq!(
        md(&p("a<text:tab/>b")),
        "a\u{00A0}\u{00A0}\u{00A0}\u{00A0}b"
    );
}

#[test]
fn line_break_is_a_hard_break_and_a_soft_page_break_is_nothing() {
    assert_eq!(md(&p("a<text:line-break/>b")), "a  \nb");
    assert_eq!(md(&p("a<text:soft-page-break/>b")), "ab");
    assert_eq!(md(&format!("<text:soft-page-break/>{}", p("x"))), "x");
}

#[test]
fn entities_and_markdown_looking_text_are_literal() {
    assert_eq!(
        md(&p("a &amp; b &lt;c&gt; &#x41; &quot;q&quot;")),
        "a & b \\<c> A \"q\""
    );
    assert_eq!(
        md(&p(
            "*star* _under_ [x] &lt;tag> $5 | `tick` # 1. ~~tilde~~ &amp;"
        )),
        "\\*star\\* \\_under\\_ \\[x] \\<tag> \\$5 \\| \\`tick\\` # 1. \\~\\~tilde\\~\\~ &"
    );
    // Block markers at the start of a paragraph are escaped; a rule is guarded.
    assert_eq!(md(&p("# not a heading")), "\\# not a heading");
    assert_eq!(md(&p("1. not a list")), "1\\. not a list");
    assert_eq!(md(&p("- not a list")), "\\- not a list");
    assert_eq!(md(&p("> not a quote")), "\\> not a quote");
    let rule = md(&p("---"));
    assert!(rule.starts_with('\u{200B}'), "{rule:?}");
    // Control characters are dropped and the private-use code points the converter uses internally
    // are replaced.
    assert_eq!(md(&p("a\u{1}b\u{E000}c")), "ab\u{FFFD}c");
}

#[test]
fn text_with_markup_characters_next_to_emphasis_keeps_the_emphasis_valid() {
    let auto = text_style("B", BOLD, None);
    // Punctuation that starts or ends a bold run goes outside the markers (CommonMark flanking).
    assert_eq!(
        md_auto(&p(&format!("x{}y", span("B", "(bold)"))), &auto),
        "x(**bold**)y"
    );
    assert_eq!(
        md_auto(&p(&format!("{} z", span("B", " spaced "))), &auto),
        "**spaced** z"
    );
}

#[test]
fn hidden_text_style_is_not_shown() {
    let auto = text_style("H", r#"text:display="none""#, None)
        + &text_style("HB", BOLD, Some("H"))
        + &para_style("PH", r#"text:display="none""#, None, "");
    assert_eq!(
        md_auto(
            &p(&format!("a{}b{}c", span("H", "SECRET"), span("HB", "S2"))),
            &auto
        ),
        "abc"
    );
    assert_eq!(
        md_auto(&(sp("PH", "hidden para") + &p("shown")), &auto),
        "shown"
    );
}

#[test]
fn a_code_paragraph_style_gives_a_code_block() {
    let auto = para_style("Preformatted_20_Text", "", None, "")
        + &para_style("P1", "", Some("Preformatted_20_Text"), "")
        + &para_style("Src", "", None, r#" style:display-name="Source Code""#);
    let body = sp(
        "P1",
        r#"def f(x):<text:line-break/> <text:s text:c="3"/>return x"#,
    ) + &sp("Preformatted_20_Text", "second")
        + &p("after")
        + &sp("Src", "a * b");
    assert_eq!(
        md_auto(&body, &auto),
        "```\ndef f(x):\n    return x\nsecond\n```\n\nafter\n\n```\na * b\n```"
    );
}

#[test]
fn a_code_block_holding_backticks_gets_a_longer_fence() {
    let auto = para_style("Preformatted_20_Text", "", None, "");
    assert_eq!(
        md_auto(&sp("Preformatted_20_Text", "```x"), &auto),
        "````\n```x\n````"
    );
}

// ---------------------------------------------------------------------------------------------
// outline numbering (text:outline-style)
// ---------------------------------------------------------------------------------------------

fn outline_style(levels: &str) -> String {
    format!(r#"<text:outline-style style:name="Outline">{levels}</text:outline-style>"#)
}

#[test]
fn numbered_headings_show_the_number_the_document_shows() {
    let styles = outline_style(
        r#"<text:outline-level-style text:level="1" style:num-format="1" style:num-suffix="."/><text:outline-level-style text:level="2" style:num-format="1" text:display-levels="2"/><text:outline-level-style text:level="3" style:num-format=""/>"#,
    );
    let body = r#"<text:h text:outline-level="1">A</text:h><text:h text:outline-level="2">B</text:h><text:h text:outline-level="2">C</text:h><text:h text:outline-level="3">D</text:h><text:h text:outline-level="1">E</text:h><text:h text:outline-level="2">F</text:h>"#;
    let o = Ox::new(body).styles(&styles);
    assert_eq!(
        conv(&o).markdown,
        "# 1\\. A\n\n## 1.1 B\n\n## 1.2 C\n\n### D\n\n# 2\\. E\n\n## 2.1 F"
    );
}

#[test]
fn headings_without_outline_numbering_stay_plain() {
    let styles = outline_style(r#"<text:outline-level-style text:level="1" style:num-format=""/>"#);
    let o = Ox::new(r#"<text:h text:outline-level="1">A</text:h>"#).styles(&styles);
    assert_eq!(conv(&o).markdown, "# A");
    let o = Ox::new(r#"<text:h text:outline-level="1">A</text:h>"#);
    assert_eq!(conv(&o).markdown, "# A");
}

#[test]
fn heading_numbering_restarts_and_list_headers_are_not_numbered() {
    let styles = outline_style(
        r#"<text:outline-level-style text:level="1" style:num-format="1" style:num-suffix="."/>"#,
    );
    let body = r#"<text:h text:outline-level="1">A</text:h><text:h text:outline-level="1" text:restart-numbering="true" text:start-value="7">B</text:h><text:h text:outline-level="1" text:is-list-header="true">C</text:h><text:h text:outline-level="1">D</text:h>"#;
    assert_eq!(
        conv(&Ox::new(body).styles(&styles)).markdown,
        "# 1\\. A\n\n# 7\\. B\n\n# C\n\n# 8\\. D"
    );
}

#[test]
fn a_numbered_heading_label_is_part_of_its_anchor() {
    let styles = outline_style(
        r#"<text:outline-level-style text:level="1" style:num-format="1" style:num-suffix="."/>"#,
    );
    let body = r##"<text:h text:outline-level="1">Intro</text:h><text:p><text:a xlink:href="#Intro|outline">go</text:a></text:p>"##;
    let m = conv(&Ox::new(body).styles(&styles)).markdown;
    // The slug is built from the label and the text, as the renderer builds it for the heading.
    assert!(m.contains("[go](#1-intro)"), "{m}");
    let shown = visible(&m);
    assert!(shown.contains("1. Intro"), "{shown}");
}

// ---------------------------------------------------------------------------------------------
// tables
// ---------------------------------------------------------------------------------------------

fn cell(text: &str) -> String {
    format!(
        r#"<table:table-cell office:value-type="string"><text:p>{text}</text:p></table:table-cell>"#
    )
}

fn row(cells: &[&str]) -> String {
    format!(
        "<table:table-row>{}</table:table-row>",
        cells.iter().map(|c| cell(c)).collect::<String>()
    )
}

fn table(rows: &str) -> String {
    format!(
        r#"<table:table table:name="T"><table:table-column table:number-columns-repeated="3"/>{rows}</table:table>"#
    )
}

#[test]
fn a_table_is_a_gfm_table() {
    let t = table(&(row(&["a", "b"]) + &row(&["c", "d"])));
    assert_eq!(md(&t), "| a | b |\n| --- | --- |\n| c | d |");
}

#[test]
fn a_table_between_paragraphs() {
    let t = table(&row(&["a"]));
    assert_eq!(
        md(&(p("before") + &t + &p("after"))),
        "before\n\n| a |\n| --- |\n\nafter"
    );
}

#[test]
fn merged_cells_keep_their_value_top_left_and_covered_cells_are_empty() {
    let t = table(
        r#"<table:table-row><table:table-cell table:number-columns-spanned="2"><text:p>wide</text:p></table:table-cell><table:covered-table-cell/><table:table-cell><text:p>x</text:p></table:table-cell></table:table-row><table:table-row><table:table-cell table:number-rows-spanned="2"><text:p>tall</text:p></table:table-cell><table:table-cell><text:p>b</text:p></table:table-cell><table:table-cell><text:p>c</text:p></table:table-cell></table:table-row><table:table-row><table:covered-table-cell/><table:table-cell><text:p>d</text:p></table:table-cell><table:table-cell><text:p>e</text:p></table:table-cell></table:table-row>"#,
    );
    assert_eq!(
        md(&t),
        "| wide |   | x |\n| --- | --- | --- |\n| tall | b | c |\n|   | d | e |"
    );
}

#[test]
fn repeated_cells_and_rows_are_expanded_within_a_cap() {
    let t = r#"<table:table><table:table-row table:number-rows-repeated="2"><table:table-cell table:number-columns-repeated="3"><text:p>x</text:p></table:table-cell></table:table-row></table:table>"#;
    assert_eq!(md(t), "| x | x | x |\n| --- | --- | --- |\n| x | x | x |");
    // An empty row repeated to the size of a sheet is one row, a trailing run of empty cells is kept
    // as columns (bounded).
    let t = r#"<table:table><table:table-row><table:table-cell><text:p>a</text:p></table:table-cell><table:table-cell table:number-columns-repeated="1000000"/></table:table-row><table:table-row table:number-rows-repeated="1000000"><table:table-cell table:number-columns-repeated="1000000"/></table:table-row></table:table>"#;
    let m = conv(&Ox::new(t)).markdown;
    assert!(m.len() < 20_000, "{}", m.len());
    assert!(m.starts_with("| a |"), "{}", &m[..20.min(m.len())]);
}

#[test]
fn header_rows_row_groups_and_collapsed_rows() {
    let t = format!(
        r#"<table:table><table:table-header-rows>{}</table:table-header-rows><table:table-rows>{}</table:table-rows><table:table-row-group>{}<table:table-row-group>{}</table:table-row-group></table:table-row-group><table:table-row table:visibility="collapse"><table:table-cell><text:p>hidden</text:p></table:table-cell></table:table-row><table:table-row table:visibility="filter"><table:table-cell><text:p>filtered</text:p></table:table-cell></table:table-row></table:table>"#,
        row(&["h"]),
        row(&["r"]),
        row(&["g1"]),
        row(&["g2"])
    );
    assert_eq!(md(&t), "| h |\n| --- |\n| r |\n| g1 |\n| g2 |");
}

#[test]
fn a_table_cell_holds_paragraphs_lists_and_formatting() {
    let auto = text_style("B", BOLD, None);
    let t = format!(
        r#"<table:table><table:table-row><table:table-cell><text:p>one</text:p><text:p>two</text:p></table:table-cell><table:table-cell><text:list><text:list-item><text:p>x</text:p></text:list-item><text:list-item><text:p>y</text:p></text:list-item></text:list></table:table-cell><table:table-cell><text:p>{}</text:p></table:table-cell></table:table-row></table:table>"#,
        span("B", "bold")
    );
    assert_eq!(
        md_auto(&t, &auto),
        "| one<br>two | • x<br>• y | **bold** |\n| --- | --- | --- |"
    );
}

#[test]
fn pipes_markup_and_breaks_in_cells_are_safe() {
    let t = table(&format!(
        "<table:table-row><table:table-cell><text:p>a|b *x* *y*</text:p></table:table-cell><table:table-cell><text:p>l1<text:line-break/>l2</text:p></table:table-cell></table:table-row>{}",
        row(&["`a` `b`", "~~x~~ ~~y~~"])
    ));
    let m = md(&t);
    assert!(m.contains("a\\|b ∗x∗ ∗y∗"), "{m}");
    assert!(m.contains("l1<br>l2"), "{m}");
    assert!(m.contains("ˋaˋ ˋbˋ"), "{m}");
    assert!(m.contains("∼∼x∼∼ ∼∼y∼∼"), "{m}");
    // Every row has as many cells as the header.
    let rows: Vec<&str> = m.lines().collect();
    assert_eq!(rows.len(), 3, "{m}");
}

#[test]
fn nested_tables_are_flattened_into_their_cell() {
    let inner = "<table:table><table:table-row><table:table-cell><text:p>i1</text:p></table:table-cell><table:table-cell><text:p>i2</text:p></table:table-cell></table:table-row></table:table>".to_string();
    let t = format!(
        "<table:table><table:table-row><table:table-cell>{inner}<text:p>after</text:p></table:table-cell><table:table-cell><text:p>b</text:p></table:table-cell></table:table-row></table:table>"
    );
    assert_eq!(md(&t), "| i1 / i2<br>after | b |\n| --- | --- |");
}

#[test]
fn a_table_over_the_cell_budget_is_cut_and_says_so() {
    let opts = with_opts(|o| o.max_table_cells = 10);
    let rows: String = (0..20).map(|i| row(&[&i.to_string(), "x"])).collect();
    let d = conv_with(&Ox::new(&table(&rows)), &opts).unwrap();
    assert!(d.truncated);
    assert!(d.markdown.lines().count() <= 2 + 5, "{}", d.markdown);
}

#[test]
fn an_empty_table_is_nothing() {
    assert_eq!(md(&(p("a") + "<table:table/>" + &p("b"))), "a\n\nb");
    assert_eq!(md("<table:table><table:table-row/></table:table>"), "");
}

// ---------------------------------------------------------------------------------------------
// pictures
// ---------------------------------------------------------------------------------------------

fn frame_image(href: &str, alt: &str) -> String {
    format!(
        r#"<draw:frame draw:name="i" text:anchor-type="as-char"><draw:image xlink:href="{href}" xlink:type="simple"/>{alt}</draw:frame>"#
    )
}

#[test]
fn a_picture_is_an_image_with_its_alt_text() {
    let o = Ox::new(&p(&frame_image(
        "Pictures/a.png",
        "<svg:title>Title</svg:title><svg:desc>The description</svg:desc>",
    )))
    .picture("a.png", 1);
    let d = conv(&o);
    assert_eq!(d.images.len(), 1);
    assert_eq!(d.images[0].bytes, tiny_png(1));
    assert_eq!(d.images[0].name, "a.png");
    assert!(d.images[0].key.starts_with("office-img://"));
    assert_eq!(
        d.markdown,
        format!("![The description]({})", d.images[0].key)
    );
}

#[test]
fn the_alt_text_falls_back_to_the_title_then_to_nothing_and_is_made_safe() {
    let o = Ox::new(
        &(p(&frame_image(
            "Pictures/a.png",
            "<svg:title>Only title</svg:title>",
        )) + &p(&frame_image("./Pictures/b.png", ""))
            + &p(&frame_image(
                "Pictures/a.png",
                "<svg:desc>a [b](c) *d* `e` &lt;f&gt; $g | h</svg:desc>",
            ))),
    )
    .picture("a.png", 1)
    .picture("b.png", 2);
    let d = conv(&o);
    let lines: Vec<&str> = d.markdown.split("\n\n").collect();
    assert!(
        lines[0].starts_with("![Only title](office-img://"),
        "{lines:?}"
    );
    assert!(lines[1].starts_with("![](office-img://"), "{lines:?}");
    assert!(
        lines[2].starts_with("![a b c d e f g h](office-img://"),
        "{lines:?}"
    );
}

#[test]
fn the_same_picture_twice_is_stored_once() {
    let o =
        Ox::new(&(p(&frame_image("Pictures/a.png", "")) + &p(&frame_image("Pictures/a.png", ""))))
            .picture("a.png", 1);
    let d = conv(&o);
    assert_eq!(d.images.len(), 1);
    assert_eq!(d.markdown.matches("office-img://").count(), 2);
}

#[test]
fn pictures_that_cannot_be_shown_are_placeholders() {
    let o = Ox::new(
        &(p(&frame_image("Pictures/missing.png", "<svg:desc>gone</svg:desc>"))
            + &p(&frame_image("https://example.com/x.png", "<svg:desc>remote</svg:desc>"))
            + &p(&frame_image("../outside.png", ""))
            + &p(&frame_image("/abs.png", ""))
            + &p(&frame_image("Pictures/odd.xyz", "<svg:desc>odd</svg:desc>"))
            + &p(r#"<draw:frame><draw:image><office:binary-data>AAAA</office:binary-data></draw:image></draw:frame>"#)),
    )
    .part("Pictures/odd.xyz", b"data");
    let d = conv(&o);
    assert!(d.images.is_empty());
    assert_eq!(
        d.markdown,
        "\\[gone]\n\n\\[remote]\n\n\\[image]\n\n\\[image]\n\n\\[odd]\n\n\\[image]"
    );
}

#[test]
fn a_placeholder_label_with_markup_is_escaped() {
    let o = Ox::new(&p(&frame_image(
        "https://e/x.png",
        "<svg:desc>[a]*b*</svg:desc>",
    )));
    let m = conv(&o).markdown;
    assert_eq!(m, "\\[\\[a]\\*b\\*]");
}

#[test]
fn picture_count_size_and_total_budgets() {
    let opts = with_opts(|o| o.max_images = 2);
    let body: String = (0..4)
        .map(|i| p(&frame_image(&format!("Pictures/{i}.png"), "")))
        .collect();
    let mut o = Ox::new(&body);
    for i in 0..4 {
        o = o.picture(&format!("{i}.png"), i as u8);
    }
    let d = conv_with(&o, &opts).unwrap();
    assert_eq!(d.images.len(), 2);
    assert!(d.truncated);

    let opts = with_opts(|o| o.max_image_bytes = 20);
    let d = conv_with(
        &Ox::new(&p(&frame_image("Pictures/a.png", ""))).picture("a.png", 1),
        &opts,
    )
    .unwrap();
    assert!(d.images.is_empty());
    assert!(d.truncated);
    assert_eq!(d.markdown, "\\[image]");

    let opts = with_opts(|o| o.max_total_image_bytes = 30);
    let o =
        Ox::new(&(p(&frame_image("Pictures/a.png", "")) + &p(&frame_image("Pictures/b.png", ""))))
            .picture("a.png", 1)
            .picture("b.png", 2);
    let d = conv_with(&o, &opts).unwrap();
    assert_eq!(d.images.len(), 1);
    assert!(d.truncated);
}

#[test]
fn a_picture_alone_in_a_table_cell_and_in_a_note() {
    let o = Ox::new(&format!(
        "<table:table><table:table-row><table:table-cell><text:p>{}</text:p></table:table-cell></table:table-row></table:table>",
        frame_image("Pictures/a.png", "<svg:desc>cell</svg:desc>")
    ))
    .picture("a.png", 1);
    let d = conv(&o);
    assert!(
        d.markdown.starts_with("| ![cell](office-img://"),
        "{}",
        d.markdown
    );
}

// ---------------------------------------------------------------------------------------------
// footnotes and endnotes
// ---------------------------------------------------------------------------------------------

fn note(class: &str, text: &str) -> String {
    format!(
        r#"<text:note text:note-class="{class}"><text:note-citation>1</text:note-citation><text:note-body><text:p>{text}</text:p></text:note-body></text:note>"#
    )
}

#[test]
fn footnotes_and_endnotes_are_numbered_in_order_of_reference() {
    let body = p(&format!("one{}", note("footnote", "first note")))
        + &p(&format!("two{}", note("endnote", "an endnote")))
        + &p(&format!("three{}", note("footnote", "third")));
    assert_eq!(
        md(&body),
        "one[^1]\n\ntwo[^2]\n\nthree[^3]\n\n[^1]: first note\n[^2]: an endnote\n[^3]: third"
    );
}

#[test]
fn a_note_body_keeps_its_formatting_and_several_paragraphs_become_one_line() {
    let auto = text_style("B", BOLD, None);
    let body = p(&format!(
        r#"x<text:note text:note-class="footnote"><text:note-body><text:p>a {} *s*</text:p><text:p>second</text:p></text:note-body></text:note>"#,
        span("B", "bold")
    ));
    assert_eq!(
        md_auto(&body, &auto),
        "x[^1]\n\n[^1]: a **bold** \\*s\\* second"
    );
}

#[test]
fn an_empty_note_is_a_dash_and_a_nested_note_is_ignored() {
    let body = p(&format!(
        r#"a<text:note text:note-class="footnote"><text:note-body><text:p/></text:note-body></text:note>b<text:note text:note-class="footnote"><text:note-body><text:p>outer{}</text:p></text:note-body></text:note>"#,
        note("footnote", "inner")
    ));
    let m = md(&body);
    assert_eq!(m, "a[^1]b[^2]\n\n[^1]: \u{2014}\n[^2]: outer");
}

#[test]
fn notes_in_headings_and_cells_and_a_note_with_no_body() {
    let body = format!(
        r#"<text:h text:outline-level="1">H{}</text:h><table:table><table:table-row><table:table-cell><text:p>c{}</text:p></table:table-cell></table:table-row></table:table>{}"#,
        note("footnote", "in heading"),
        note("endnote", "in cell"),
        p(r#"x<text:note text:note-class="footnote"/>y"#)
    );
    let m = md(&body);
    assert!(m.contains("# H[^1]"), "{m}");
    assert!(m.contains("| c[^2] |"), "{m}");
    assert!(m.contains("xy"), "{m}");
    assert!(m.ends_with("[^1]: in heading\n[^2]: in cell"), "{m}");
}

#[test]
fn the_note_budget_stops_the_numbering() {
    let opts = with_opts(|o| o.max_notes = 2);
    let body: String = (0..4)
        .map(|i| p(&format!("p{i}{}", note("footnote", &format!("n{i}")))))
        .collect();
    let d = conv_with(&Ox::new(&body), &opts).unwrap();
    assert!(d.truncated);
    assert_eq!(d.notes, 2);
    assert!(d.markdown.contains("p3\n"), "{}", d.markdown);
}

#[test]
fn the_notes_that_do_not_fit_the_markdown_budget_are_cut() {
    let opts = with_opts(|o| o.max_markdown_bytes = 400);
    let body: String = (0..30)
        .map(|i| p(&format!("p{i}{}", note("footnote", &"n".repeat(60)))))
        .collect();
    let d = conv_with(&Ox::new(&body), &opts).unwrap();
    assert!(d.truncated);
    assert!(d.markdown.len() <= 400, "{}", d.markdown.len());
}

// ---------------------------------------------------------------------------------------------
// links
// ---------------------------------------------------------------------------------------------

fn link(href: &str, text: &str) -> String {
    format!(r#"<text:a xlink:type="simple" xlink:href="{href}">{text}</text:a>"#)
}

#[test]
fn external_links_are_links_and_only_safe_schemes() {
    let body = p(&format!(
        "{} {} {} {} {} {} {}",
        link("https://example.com/a?x=1&amp;y=2", "web"),
        link("http://e.example/", "plain"),
        link("mailto:a@b.example", "mail"),
        link("javascript:alert(1)", "js"),
        link("file:///etc/passwd", "file"),
        link("other.odt", "relative"),
        link("", "empty")
    ));
    assert_eq!(
        md(&body),
        "[web](https://example.com/a?x=1&y=2) [plain](http://e.example/) [mail](mailto:a@b.example) js file relative empty"
    );
}

#[test]
fn a_link_with_no_href_or_no_text_is_just_its_text_or_nothing() {
    assert_eq!(
        md(&p(
            r#"<text:a>no href</text:a> and <text:a xlink:href="https://e.example/"></text:a>x"#
        )),
        "no href and x"
    );
}

#[test]
fn link_text_keeps_emphasis_and_brackets_are_safe_in_the_label() {
    let auto = text_style("B", BOLD, None);
    let body = p(&link(
        "https://e.example/",
        &format!("a [b] {}", span("B", "bold")),
    ));
    assert_eq!(
        md_auto(&body, &auto),
        "[a &#91;b&#93; **bold**](https://e.example/)"
    );
}

#[test]
fn an_internal_link_goes_to_the_heading_that_holds_the_bookmark() {
    let body = format!(
        r##"{}<text:h text:outline-level="2"><text:bookmark-start text:name="bm1"/>Target heading<text:bookmark-end text:name="bm1"/></text:h><text:p><text:bookmark text:name="bm2"/>x</text:p>"##,
        p(&(link("#bm1", "go to heading")
            + " "
            + &link("#bm2", "go to paragraph")
            + " "
            + &link("#Nowhere", "dead")))
    );
    let m = md(&body);
    assert!(m.contains("[go to heading](#target-heading)"), "{m}");
    // A bookmark in a paragraph has no heading: the name is the slug (the same as docx does).
    assert!(m.contains("[go to paragraph](#bm2)"), "{m}");
    assert!(m.contains("[dead](#nowhere)"), "{m}");
}

#[test]
fn libreoffice_cross_references_with_a_kind_suffix_are_anchors() {
    let body = format!(
        r##"<text:h text:outline-level="1">Intro</text:h>{}"##,
        p(&(link("#Intro|outline", "a") + &link("#Intro|text", "b") + &link("#a|b", "c")))
    );
    let m = md(&body);
    assert!(m.contains("[a](#intro)"), "{m}");
    assert!(m.contains("[b](#intro)"), "{m}");
    // An unknown suffix is part of the name.
    assert!(m.contains("[c](#ab)"), "{m}");
    assert_eq!(md(&p(&link("#", "empty"))), "empty");
}

#[test]
fn heading_slugs_are_unique_and_a_link_finds_the_second_of_two_equal_headings() {
    let body = r##"<text:h text:outline-level="1">Same</text:h><text:h text:outline-level="1"><text:bookmark-start text:name="second"/>Same</text:h><text:p><text:a xlink:href="#second">to second</text:a></text:p>"##;
    let m = md(body);
    assert!(m.contains("[to second](#same-1)"), "{m}");
}

#[test]
fn the_text_of_a_link_through_a_span_and_a_link_inside_a_heading() {
    let auto = text_style("Link", "", None);
    let body = format!(
        r#"<text:h text:outline-level="1">See {}</text:h>"#,
        link("https://e.example/", &span("Link", "here"))
    );
    assert_eq!(md_auto(&body, &auto), "# See [here](https://e.example/)");
}

// ---------------------------------------------------------------------------------------------
// table of contents, fields, sections, text boxes
// ---------------------------------------------------------------------------------------------

#[test]
fn a_table_of_contents_is_its_saved_text_and_the_source_is_never_shown() {
    let body = r##"<text:table-of-content text:name="T"><text:table-of-content-source text:outline-level="3"><text:index-title-template>SOURCE-TITLE</text:index-title-template></text:table-of-content-source><text:index-body><text:index-title text:name="T_Head"><text:p>Contents</text:p></text:index-title><text:p><text:a xlink:href="#__h1">One<text:tab/>1</text:a></text:p><text:p><text:a xlink:href="#__h2">Two<text:tab/>2</text:a></text:p></text:index-body></text:table-of-content><text:h text:outline-level="1"><text:bookmark-start text:name="__h1"/>One</text:h><text:h text:outline-level="1"><text:bookmark-start text:name="__h2"/>Two</text:h>"##;
    let m = md(body);
    assert!(!m.contains("SOURCE-TITLE"), "{m}");
    assert!(m.starts_with("Contents\n\n[One\u{00A0}\u{00A0}\u{00A0}\u{00A0}1](#one)\n\n[Two\u{00A0}\u{00A0}\u{00A0}\u{00A0}2](#two)\n\n# One"), "{m}");
}

#[test]
fn other_indexes_are_read_the_same_way() {
    for tag in [
        "alphabetical-index",
        "illustration-index",
        "table-index",
        "object-index",
        "user-index",
        "bibliography",
    ] {
        let body = format!(
            r#"<text:{tag}><text:{tag}-source>SRC</text:{tag}-source><text:index-body><text:p>entry in {tag}</text:p></text:index-body></text:{tag}>"#
        );
        let m = md(&body);
        assert_eq!(m, format!("entry in {tag}"), "{tag}");
    }
}

#[test]
fn fields_show_their_saved_text() {
    let body = p(
        r#"Page <text:page-number text:select-page="current">3</text:page-number> of <text:page-count>9</text:page-count>, <text:date style:data-style-name="D" text:date-value="2020-01-02">01/02/2020</text:date>, <text:title>My Title</text:title>, <text:sequence text:name="Table" text:formula="ooow:Table+1">Table 1</text:sequence>, <text:bookmark-ref text:ref-name="x" text:reference-format="text">ref text</text:bookmark-ref>, <text:user-field-get text:name="u">uf</text:user-field-get>, <text:meta><text:span>meta</text:span></text:meta>"#,
    );
    assert_eq!(
        md(&body),
        "Page 3 of 9, 01/02/2020, My Title, Table 1, ref text, uf, meta"
    );
}

#[test]
fn text_a_conditional_or_hidden_field_does_not_leak_its_condition() {
    let body = p(
        r#"a<text:hidden-text text:condition="x" text:string-value="SECRET">SECRET</text:hidden-text>b<text:hidden-paragraph text:condition="y">HP</text:hidden-paragraph>c"#,
    );
    assert_eq!(md(&body), "abc");
}

#[test]
fn ruby_shows_the_base_text_only() {
    let body = p(
        r#"<text:ruby><text:ruby-base>漢字</text:ruby-base><text:ruby-text>かんじ</text:ruby-text></text:ruby>です"#,
    );
    assert_eq!(md(&body), "漢字です");
}

#[test]
fn a_section_is_its_content_and_a_hidden_section_is_not_shown() {
    let body = format!(
        r#"{}<text:section text:name="S1">{}<text:section text:name="Inner">{}</text:section></text:section><text:section text:name="H" text:display="none">{}</text:section>{}"#,
        p("before"),
        p("in section"),
        p("nested"),
        p("HIDDEN"),
        p("after")
    );
    assert_eq!(md(&body), "before\n\nin section\n\nnested\n\nafter");
}

#[test]
fn a_document_wrapped_in_one_section_is_still_read_block_by_block() {
    // The section is entered, not held as one tree: a block budget smaller than the section works.
    let opts = with_opts(|o| o.max_block_nodes = 30);
    let paras: String = (0..200).map(|i| p(&format!("para {i}"))).collect();
    let d = conv_with(
        &Ox::new(&format!("<text:section>{paras}</text:section>")),
        &opts,
    )
    .unwrap();
    assert!(!d.truncated);
    assert_eq!(d.markdown.matches("para ").count(), 200);
    // The same paragraphs in one list are one block and exceed the budget.
    let items: String = (0..200)
        .map(|i| format!("<text:list-item><text:p>item {i}</text:p></text:list-item>"))
        .collect();
    let d = conv_with(&Ox::new(&format!("<text:list>{items}</text:list>")), &opts).unwrap();
    assert!(d.truncated);
}

#[test]
fn a_section_inside_a_cell_is_read() {
    let body = format!(
        "<table:table><table:table-row><table:table-cell><text:section>{}</text:section></table:table-cell></table:table-row></table:table>",
        p("in cell")
    );
    assert_eq!(md(&body), "| in cell |\n| --- |");
}

#[test]
fn a_text_box_goes_after_the_paragraph_that_holds_it() {
    let body = p(
        r#"Anchor paragraph.<draw:frame text:anchor-type="paragraph"><draw:text-box><text:p>BOX-TEXT</text:p><text:p>second box line</text:p></draw:text-box></draw:frame> tail"#,
    );
    assert_eq!(
        md(&body),
        "Anchor paragraph. tail\n\nBOX-TEXT\n\nsecond box line"
    );
}

#[test]
fn shapes_with_text_and_groups_are_read_in_order() {
    let body = p(
        r#"A<draw:custom-shape><text:p>shape text</text:p></draw:custom-shape><draw:g><draw:rect><text:p>in group</text:p></draw:rect><draw:g><draw:ellipse><text:p>deep</text:p></draw:ellipse></draw:g></draw:g>B"#,
    );
    assert_eq!(md(&body), "AB\n\nshape text\n\nin group\n\ndeep");
    // A text box inside a cell is flattened into it.
    let t = r#"<table:table><table:table-row><table:table-cell><text:p>c<draw:frame><draw:text-box><text:p>boxed</text:p></draw:text-box></draw:frame></text:p></table:table-cell></table:table-row></table:table>"#.to_string();
    assert_eq!(md(&t), "| c<br>boxed |\n| --- |");
}

#[test]
fn a_frame_at_block_level_and_a_picture_in_a_shape() {
    let o = Ox::new(&format!(
        "{}<draw:custom-shape><draw:frame>{}</draw:frame></draw:custom-shape>",
        frame_image("Pictures/a.png", "<svg:desc>top</svg:desc>"),
        r#"<draw:image xlink:href="Pictures/a.png"/><svg:desc>inner</svg:desc>"#
    ))
    .picture("a.png", 1);
    let d = conv(&o);
    assert_eq!(d.markdown.matches("![").count(), 2, "{}", d.markdown);
}

// ---------------------------------------------------------------------------------------------
// tracked changes, comments, headers and footers
// ---------------------------------------------------------------------------------------------

fn regions(deleted: &[&str], inserted: &[&str]) -> String {
    let mut s = String::from(r#"<text:tracked-changes text:track-changes="false">"#);
    for id in deleted {
        s += &format!(
            r#"<text:changed-region xml:id="{id}" text:id="{id}"><text:deletion><office:change-info><dc:creator>A</dc:creator></office:change-info><text:p>DELETED-{id}</text:p></text:deletion></text:changed-region>"#
        );
    }
    for id in inserted {
        s += &format!(
            r#"<text:changed-region xml:id="{id}" text:id="{id}"><text:insertion><office:change-info><dc:creator>A</dc:creator></office:change-info></text:insertion></text:changed-region>"#
        );
    }
    s + "</text:tracked-changes>"
}

#[test]
fn tracked_changes_show_the_final_version() {
    // What LibreOffice writes: the deleted text only inside the region, a `text:change` mark in the
    // body, the inserted text between change-start and change-end.
    let body = format!(
        r#"{}{}"#,
        regions(&["d1"], &["i1"]),
        p(
            r#"Kept. <text:change text:change-id="d1"/><text:change-start text:change-id="i1"/>Inserted.<text:change-end text:change-id="i1"/>"#
        )
    );
    let m = md(&body);
    assert_eq!(m, "Kept. Inserted.");
}

#[test]
fn text_between_the_marks_of_a_deletion_is_not_shown() {
    // Other producers keep the deleted text in the body between the marks.
    let body = format!(
        r#"{}{}{}{}"#,
        regions(&["d1"], &["i1"]),
        p(
            r#"Before <text:change-start text:change-id="d1"/>GONE <text:span>and gone</text:span><text:change-end text:change-id="d1"/>after, <text:change-start text:change-id="i1"/>kept<text:change-end text:change-id="i1"/>."#
        ),
        // A deletion that spans paragraphs.
        p(r#"start <text:change-start text:change-id="d1"/>X"#)
            + &p("whole paragraph gone")
            + &p(r#"Y<text:change-end text:change-id="d1"/> end"#),
        table(&row(&["cell"]))
    );
    let m = md(&body);
    assert!(m.starts_with("Before after, kept."), "{m}");
    assert!(!m.contains("GONE") && !m.contains("gone"), "{m}");
    assert!(m.contains("start") && m.contains("end"), "{m}");
    assert!(m.contains("| cell |"), "{m}");
}

#[test]
fn an_unterminated_deletion_hides_only_what_follows_it_and_a_stray_end_is_harmless() {
    let body = format!(
        "{}{}{}",
        regions(&["d1"], &[]),
        p("visible"),
        p(r#"a<text:change-end text:change-id="d1"/>b<text:change-end text:change-id="zzz"/>c"#)
    );
    assert_eq!(md(&body), "visible\n\nabc");
    let body = format!(
        "{}{}{}",
        regions(&["d1"], &[]),
        p(r#"a<text:change-start text:change-id="d1"/>b"#),
        p("c")
    );
    assert_eq!(md(&body), "a");
}

#[test]
fn a_change_id_that_is_not_a_deletion_never_hides_text() {
    let body = format!(
        "{}{}",
        regions(&[], &["i1"]),
        p(r#"a<text:change-start text:change-id="i1"/>b<text:change-end text:change-id="i1"/>c"#)
    );
    assert_eq!(md(&body), "abc");
    // No tracked-changes element at all.
    assert_eq!(md(&p(r#"a<text:change-start text:change-id="x"/>b"#)), "ab");
}

#[test]
fn a_deleted_picture_note_and_table_are_not_shown() {
    let o = Ox::new(&format!(
        r#"{}{}"#,
        regions(&["d1"], &[]),
        p(&format!(
            r#"a<text:change-start text:change-id="d1"/>{}{}<text:change-end text:change-id="d1"/>b"#,
            frame_image("Pictures/a.png", ""),
            note("footnote", "DELETED NOTE")
        ))
    ))
    .picture("a.png", 1);
    let d = conv(&o);
    assert_eq!(d.markdown, "ab");
    assert!(d.images.is_empty());
}

#[test]
fn comments_are_not_shown_and_neither_is_the_end_mark() {
    let body = p(
        r#"Text<office:annotation office:name="c1"><dc:creator>Tester</dc:creator><dc:date>2020-01-01T00:00:00</dc:date><text:p>SECRET-COMMENT</text:p></office:annotation> more<office:annotation-end office:name="c1"/>."#,
    );
    assert_eq!(md(&body), "Text more.");
}

#[test]
fn headers_and_footers_in_the_master_page_are_not_shown() {
    let master = r#"<style:master-page style:name="Standard" style:page-layout-name="pm1"><style:header><text:p>RUNNING-HEADER</text:p></style:header><style:footer><text:p>RUNNING-FOOTER <text:page-number>1</text:page-number></text:p></style:footer></style:master-page>"#;
    let o = Ox::new(&p("Body")).master(master);
    assert_eq!(conv(&o).markdown, "Body");
}

// ---------------------------------------------------------------------------------------------
// the documents LibreOffice wrote
// ---------------------------------------------------------------------------------------------

pub(super) fn lo_path(name: &str) -> Option<PathBuf> {
    lo(name)
}

fn lo(name: &str) -> Option<PathBuf> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("testdata/office")
        .join(name);
    p.exists().then_some(p)
}

fn read_lo(name: &str) -> Option<Document> {
    let p = lo(name)?;
    Some(load_document(&p, &DocOptions::default()).unwrap())
}

/// The picture line of a document differs by what the two formats store (the alt text LibreOffice
/// writes into a docx joins the title and the description; the file name inside the package):
/// everything else must be the same text.
fn without_pictures(m: &str) -> String {
    m.lines()
        .map(|l| {
            if l.starts_with("![") {
                "![picture]".to_string()
            } else {
                l.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn the_same_document_saved_as_docx_and_odt_converts_to_the_same_markdown() {
    for (docx, odt) in [("word.docx", "word.odt"), ("word-ja.docx", "word-ja.odt")] {
        let (Some(a), Some(b)) = (read_lo(docx), read_lo(odt)) else {
            return;
        };
        assert!(!a.truncated && !b.truncated);
        assert_eq!(
            without_pictures(&a.markdown),
            without_pictures(&b.markdown),
            "{docx} vs {odt}"
        );
        // Same pictures, same formulas, same notes.
        assert_eq!(a.images.len(), b.images.len());
        assert_eq!(a.images[0].bytes, b.images[0].bytes);
        assert_eq!((a.math_total, a.math_latex), (b.math_total, b.math_latex));
        assert_eq!(a.notes, b.notes);
    }
}

#[test]
fn libreoffice_english_odt() {
    let Some(d) = read_lo("word.odt") else {
        return;
    };
    let m = &d.markdown;
    assert!(m.starts_with("# Word reader sample\n"), "{m}");
    for want in [
        "# Introduction",
        "## Lists",
        "### Bullets and numbers",
        "## Pictures and notes",
        "Plain text, **bold** *italic* ~~strike~~ ***bold italic*** and underline.",
        "\\*star\\* \\_under\\_ \\[x] \\<tag> \\$5 \\| \\`tick\\` # 1. \\~\\~tilde\\~\\~ &",
        "A line  \nafter a line break",
        "- bullet one\n  - bullet nested\n- bullet two",
        "1. first\n2. second\n   1. second-a\n3. third",
        "a. alpha  \nb. beta  \n\u{a0}\u{a0}a. beta one",
        "I. one  \nII. two  \n\u{a0}\u{a0}i. two a",
        "| **Name** | **Merged across two columns** |   |",
        "| tall cell | 2 | two<br>lines |",
        "[example site](https://example.com/a?x=1&y=2)",
        "[go to Introduction](#introduction)",
        "Kept text. This sentence is inserted.",
        "TEXTBOX-TEXT inside a frame",
        "```\ndef f(x):\n    return x * 2\n```",
        "[^1]: Footnote text with \\*star\\*.",
        "[^2]: Endnote text.",
        "$E=m{c}^{2}$",
        "$\\frac{a+b}{c}=\\sqrt{{x}_{1}^{2}+{y}_{1}^{2}}$",
    ] {
        assert!(m.contains(want), "{want:?} in\n{m}");
    }
    // Not shown: the deleted sentence, the comment, the header and footer, the contents source.
    for not in [
        "deleted",
        "SECRET-COMMENT",
        "RUNNING-HEADER",
        "RUNNING-FOOTER",
    ] {
        assert!(!m.contains(not), "{not:?} in\n{m}");
    }
    // The table of contents is its saved text, with links to the headings.
    assert!(
        m.contains("[Introduction\u{00A0}\u{00A0}\u{00A0}\u{00A0}1](#introduction)"),
        "{m}"
    );
    assert_eq!(d.images.len(), 1);
    assert_eq!((d.math_total, d.math_latex), (2, 2));
    assert_eq!(d.notes, 2);
}

#[test]
fn libreoffice_japanese_odt() {
    let Some(d) = read_lo("word-ja.odt") else {
        return;
    };
    let m = &d.markdown;
    assert!(m.starts_with("# Word 読み込みサンプル\n"), "{m}");
    for want in [
        "# はじめに",
        "ふつうの文、**太字** *斜体* ~~取り消し線~~ ***太字の斜体*** と下線。",
        "ア. あ  \nイ. い  \n\u{a0}\u{a0}イ. いろは  \nウ. う",
        "①. 丸数字一  \n②. 丸数字二",
        "1. 条文一\n2. 条文二",
        "[はじめにへ](#はじめに)",
        "[^1]: 脚注の本文(\\*star\\* を含む)。",
        "枠の中の文字(TEXTBOX-TEXT)",
        "$E=m{c}^{2}$",
    ] {
        assert!(m.contains(want), "{want:?} in\n{m}");
    }
    for not in ["削除されます", "秘密のコメント本文", "RUNNING-HEADER"] {
        assert!(!m.contains(not), "{not:?} in\n{m}");
    }
}

#[test]
fn the_converted_odt_draws_through_konomas_markdown_renderer() {
    let Some(d) = read_lo("word.odt") else {
        return;
    };
    let shown = visible(&d.markdown);
    for want in [
        "Word reader sample",
        "Introduction",
        "- bullet one",
        "1. first",
        "a. alpha",
        "Merged across two columns",
        "TEXTBOX-TEXT inside a frame",
    ] {
        assert!(shown.contains(want), "{want:?} in\n{shown}");
    }
    assert!(!shown.contains("# Introduction"), "{shown}");
}
