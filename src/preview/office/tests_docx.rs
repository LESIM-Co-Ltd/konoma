//! Tests of the Word reader (`docx.rs`).

use std::path::PathBuf;

use super::docx::*;
use super::tests::{deflated, tmp, write};
use super::*;
use crate::preview::markdown::{render_markdown, CodeStyle};

// ---------------------------------------------------------------------------------------------
// builders
// ---------------------------------------------------------------------------------------------

pub(super) const REL_BASE: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

/// A tiny PNG (1x1) for picture tests.
pub(super) fn tiny_png(seed: u8) -> Vec<u8> {
    let mut v = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    v.extend_from_slice(&[seed; 16]);
    v
}

#[derive(Default, Clone)]
pub(super) struct Dx {
    pub body: String,
    pub styles: Option<String>,
    pub numbering: Option<String>,
    pub footnotes: Option<String>,
    pub endnotes: Option<String>,
    /// `(id, type tail, target, external)` of `word/_rels/document.xml.rels`.
    pub rels: Vec<(String, String, String, bool)>,
    pub footnote_rels: Vec<(String, String, String, bool)>,
    pub media: Vec<(String, Vec<u8>)>,
    pub extra: Vec<(String, Vec<u8>)>,
}

pub(super) fn ns() -> String {
    format!(
        r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="{REL_BASE}" xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:pic="http://schemas.openxmlformats.org/drawingml/2006/picture" xmlns:v="urn:schemas-microsoft-com:vml" xmlns:o="urn:schemas-microsoft-com:office:office" xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006" xmlns:m="http://schemas.openxmlformats.org/officeDocument/2006/math" xmlns:wps="http://schemas.microsoft.com/office/word/2010/wordprocessingShape""#
    )
}

fn rels_xml(rels: &[(String, String, String, bool)]) -> String {
    let mut s = String::from(
        r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">"#,
    );
    for (id, ty, target, ext) in rels {
        s += &format!(
            r#"<Relationship Id="{id}" Type="{REL_BASE}/{ty}" Target="{}"{}/>"#,
            target.replace('&', "&amp;"),
            if *ext {
                r#" TargetMode="External""#
            } else {
                ""
            }
        );
    }
    s + "</Relationships>"
}

impl Dx {
    pub fn new(body: &str) -> Dx {
        Dx {
            body: body.to_string(),
            ..Dx::default()
        }
    }
    pub fn styles(mut self, s: &str) -> Dx {
        self.styles = Some(format!(
            r#"<?xml version="1.0" encoding="UTF-8"?><w:styles {}>{s}</w:styles>"#,
            ns()
        ));
        self
    }
    pub fn numbering(mut self, s: &str) -> Dx {
        self.numbering = Some(format!(
            r#"<?xml version="1.0" encoding="UTF-8"?><w:numbering {}>{s}</w:numbering>"#,
            ns()
        ));
        self
    }
    pub fn footnotes(mut self, s: &str) -> Dx {
        self.footnotes = Some(format!(
            r#"<?xml version="1.0" encoding="UTF-8"?><w:footnotes {}>{s}</w:footnotes>"#,
            ns()
        ));
        self
    }
    pub fn endnotes(mut self, s: &str) -> Dx {
        self.endnotes = Some(format!(
            r#"<?xml version="1.0" encoding="UTF-8"?><w:endnotes {}>{s}</w:endnotes>"#,
            ns()
        ));
        self
    }
    pub fn rel(mut self, id: &str, ty: &str, target: &str, ext: bool) -> Dx {
        self.rels.push((id.into(), ty.into(), target.into(), ext));
        self
    }
    pub fn media(mut self, name: &str, bytes: Vec<u8>) -> Dx {
        self.media.push((name.into(), bytes));
        self
    }
    pub fn part(mut self, name: &str, bytes: &[u8]) -> Dx {
        self.extra.push((name.into(), bytes.to_vec()));
        self
    }
    pub fn document_xml(&self) -> String {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?><w:document {}><w:body>{}</w:body></w:document>"#,
            ns(),
            self.body
        )
    }
    pub fn bytes(&self) -> Vec<u8> {
        let doc = self.document_xml();
        let mut rels = self.rels.clone();
        if self.styles.is_some() {
            rels.push((
                "rIdStyles".into(),
                "styles".into(),
                "styles.xml".into(),
                false,
            ));
        }
        if self.numbering.is_some() {
            rels.push((
                "rIdNum".into(),
                "numbering".into(),
                "numbering.xml".into(),
                false,
            ));
        }
        if self.footnotes.is_some() {
            rels.push((
                "rIdFn".into(),
                "footnotes".into(),
                "footnotes.xml".into(),
                false,
            ));
        }
        if self.endnotes.is_some() {
            rels.push((
                "rIdEn".into(),
                "endnotes".into(),
                "endnotes.xml".into(),
                false,
            ));
        }
        let rels = rels_xml(&rels);
        let root = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{REL_BASE}/officeDocument" Target="word/document.xml"/></Relationships>"#
        );
        let ct = r#"<?xml version="1.0" encoding="UTF-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"/>"#;
        let fn_rels = rels_xml(&self.footnote_rels);
        let mut entries: Vec<(String, Vec<u8>)> = vec![
            ("[Content_Types].xml".into(), ct.as_bytes().to_vec()),
            ("_rels/.rels".into(), root.into_bytes()),
            ("word/document.xml".into(), doc.into_bytes()),
            ("word/_rels/document.xml.rels".into(), rels.into_bytes()),
        ];
        if let Some(s) = &self.styles {
            entries.push(("word/styles.xml".into(), s.clone().into_bytes()));
        }
        if let Some(s) = &self.numbering {
            entries.push(("word/numbering.xml".into(), s.clone().into_bytes()));
        }
        if let Some(s) = &self.footnotes {
            entries.push(("word/footnotes.xml".into(), s.clone().into_bytes()));
            if !self.footnote_rels.is_empty() {
                entries.push((
                    "word/_rels/footnotes.xml.rels".into(),
                    fn_rels.clone().into_bytes(),
                ));
            }
        }
        if let Some(s) = &self.endnotes {
            entries.push(("word/endnotes.xml".into(), s.clone().into_bytes()));
        }
        for (n, b) in &self.media {
            entries.push((format!("word/media/{n}"), b.clone()));
        }
        for (n, b) in &self.extra {
            entries.push((n.clone(), b.clone()));
        }
        let refs: Vec<(&str, &[u8])> = entries
            .iter()
            .map(|(n, b)| (n.as_str(), b.as_slice()))
            .collect();
        deflated(&refs)
    }
}

pub(super) fn conv_with(d: &Dx, opts: &DocOptions) -> Result<Document, OfficeError> {
    let dir = tmp("docx");
    let p = write(&dir, "t.docx", &d.bytes());
    load_document(&p, opts)
}

pub(super) fn conv(d: &Dx) -> Document {
    conv_with(d, &DocOptions::default()).unwrap()
}

/// The Markdown of a body.
pub(super) fn md(body: &str) -> String {
    conv(&Dx::new(body)).markdown
}

pub(super) fn md_styled(body: &str, styles: &str) -> String {
    conv(&Dx::new(body).styles(styles)).markdown
}

pub(super) fn p(text: &str) -> String {
    format!(r#"<w:p><w:r><w:t xml:space="preserve">{text}</w:t></w:r></w:p>"#)
}

pub(super) fn run(text: &str) -> String {
    format!(r#"<w:r><w:t xml:space="preserve">{text}</w:t></w:r>"#)
}

pub(super) fn runp(rpr: &str, text: &str) -> String {
    format!(r#"<w:r><w:rPr>{rpr}</w:rPr><w:t xml:space="preserve">{text}</w:t></w:r>"#)
}

pub(super) fn para(inner: &str) -> String {
    format!("<w:p>{inner}</w:p>")
}

pub(super) fn styled_p(style: &str, text: &str) -> String {
    format!(
        r#"<w:p><w:pPr><w:pStyle w:val="{style}"/></w:pPr><w:r><w:t xml:space="preserve">{text}</w:t></w:r></w:p>"#
    )
}

pub(super) fn num_p(num_id: u32, lvl: u8, text: &str) -> String {
    format!(
        r#"<w:p><w:pPr><w:numPr><w:ilvl w:val="{lvl}"/><w:numId w:val="{num_id}"/></w:numPr></w:pPr><w:r><w:t xml:space="preserve">{text}</w:t></w:r></w:p>"#
    )
}

pub(super) const HEADING_STYLES: &str = r#"
<w:style w:type="paragraph" w:styleId="Heading1"><w:name w:val="heading 1"/><w:pPr><w:outlineLvl w:val="0"/></w:pPr><w:rPr><w:b/></w:rPr></w:style>
<w:style w:type="paragraph" w:styleId="Heading2"><w:name w:val="heading 2"/><w:basedOn w:val="Heading1"/><w:pPr><w:outlineLvl w:val="1"/></w:pPr></w:style>
<w:style w:type="paragraph" w:styleId="Heading3"><w:name w:val="heading 3"/><w:basedOn w:val="Heading2"/><w:pPr><w:outlineLvl w:val="2"/></w:pPr></w:style>"#;

/// The text konoma's renderer shows for a Markdown string (width 200, lines joined by `\n`,
/// trailing blanks trimmed).
pub(super) fn visible(markdown: &str) -> String {
    rendered(markdown, 200)
        .iter()
        .map(|l| l.trim_end().to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Renders the way the app does (front matter, footnotes, inline HTML, then the block renderer).
pub(super) fn rendered(src: &str, width: u16) -> Vec<String> {
    use crate::preview::markdown::{process_footnotes, process_inline_html, strip_front_matter};
    let body = strip_front_matter(src).1;
    let pre = process_inline_html(&process_footnotes(&body));
    render_markdown(&pre, width, CodeStyle::default(), "TwoDark", false)
        .iter()
        .map(|l| {
            l.spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>()
        })
        .collect()
}

#[allow(dead_code)]
pub(super) fn tmp_path(name: &str) -> PathBuf {
    let dir = tmp("docxp");
    let p = dir.join(name);
    std::mem::forget(dir);
    p
}

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
        md(&(p("one") + &p("") + &p("   ") + "<w:p/>" + &p("two"))),
        "one\n\ntwo"
    );
}

#[test]
fn heading_by_outline_level_in_the_style_chain() {
    let body = styled_p("Heading1", "Title")
        + &styled_p("Heading2", "Sub")
        + &styled_p("Heading3", "Subsub")
        + &p("text");
    assert_eq!(
        md_styled(&body, HEADING_STYLES),
        "# Title\n\n## Sub\n\n### Subsub\n\ntext"
    );
}

#[test]
fn heading_bold_from_the_style_is_not_repeated() {
    // Heading 1 above is bold in its style; `# **Title**` would be noise.
    assert_eq!(
        md_styled(&styled_p("Heading1", "Title"), HEADING_STYLES),
        "# Title"
    );
}

#[test]
fn heading_levels_deeper_than_six_are_clamped() {
    let styles = r#"<w:style w:type="paragraph" w:styleId="H8"><w:name w:val="x"/><w:pPr><w:outlineLvl w:val="7"/></w:pPr></w:style>"#;
    assert_eq!(md_styled(&styled_p("H8", "deep"), styles), "###### deep");
}

#[test]
fn heading_by_english_and_japanese_style_name_without_outline_level() {
    let styles = r#"
<w:style w:type="paragraph" w:styleId="a1"><w:name w:val="heading 1"/></w:style>
<w:style w:type="paragraph" w:styleId="a2"><w:name w:val="見出し 2"/></w:style>
<w:style w:type="paragraph" w:styleId="a3"><w:name w:val="見出し　３"/></w:style>
<w:style w:type="paragraph" w:styleId="a4"><w:name w:val="標題 4"/></w:style>
<w:style w:type="paragraph" w:styleId="a5"><w:name w:val="Heading 5"/></w:style>"#;
    let body = styled_p("a1", "e1")
        + &styled_p("a2", "j2")
        + &styled_p("a3", "j3")
        + &styled_p("a4", "z4")
        + &styled_p("a5", "e5");
    assert_eq!(
        md_styled(&body, styles),
        "# e1\n\n## j2\n\n### j3\n\n#### z4\n\n##### e5"
    );
}

#[test]
fn heading_by_style_id_when_the_package_has_no_styles_part() {
    // A generator that wrote `Heading1` but no styles.xml: the id's spelling is the only hint.
    assert_eq!(md(&styled_p("Heading2", "t")), "## t");
}

#[test]
fn a_direct_outline_level_makes_a_heading_and_9_unmakes_one() {
    let direct =
        r#"<w:p><w:pPr><w:outlineLvl w:val="1"/></w:pPr><w:r><w:t>direct</w:t></w:r></w:p>"#;
    assert_eq!(md(direct), "## direct");
    let off = r#"<w:p><w:pPr><w:pStyle w:val="Heading1"/><w:outlineLvl w:val="9"/></w:pPr><w:r><w:t>plain</w:t></w:r></w:p>"#;
    // No longer a heading: the style's own bold is then ordinary bold text.
    assert_eq!(md_styled(off, HEADING_STYLES), "**plain**");
}

#[test]
fn a_style_based_on_a_heading_is_a_heading_and_a_cycle_ends() {
    let styles = format!(
        "{HEADING_STYLES}{}",
        r#"<w:style w:type="paragraph" w:styleId="Mine"><w:name w:val="Mine"/><w:basedOn w:val="Heading2"/></w:style>
<w:style w:type="paragraph" w:styleId="CycA"><w:name w:val="CycA"/><w:basedOn w:val="CycB"/></w:style>
<w:style w:type="paragraph" w:styleId="CycB"><w:name w:val="CycB"/><w:basedOn w:val="CycA"/></w:style>
<w:style w:type="paragraph" w:styleId="Self"><w:name w:val="Self"/><w:basedOn w:val="Self"/></w:style>"#
    );
    let body = styled_p("Mine", "m") + &styled_p("CycA", "a") + &styled_p("Self", "s");
    assert_eq!(md_styled(&body, &styles), "## m\n\na\n\ns");
}

#[test]
fn the_outline_level_is_inherited_from_a_base_style_whatever_its_name() {
    let styles = r#"
<w:style w:type="paragraph" w:styleId="Base"><w:name w:val="Chapter base"/><w:pPr><w:outlineLvl w:val="2"/></w:pPr></w:style>
<w:style w:type="paragraph" w:styleId="Mid"><w:name w:val="Mid"/><w:basedOn w:val="Base"/></w:style>
<w:style w:type="paragraph" w:styleId="Leaf"><w:name w:val="Leaf"/><w:basedOn w:val="Mid"/></w:style>
<w:style w:type="paragraph" w:styleId="Off"><w:name w:val="Off"/><w:basedOn w:val="Base"/><w:pPr><w:outlineLvl w:val="9"/></w:pPr></w:style>"#;
    let body = styled_p("Leaf", "leaf") + &styled_p("Off", "off");
    assert_eq!(md_styled(&body, styles), "### leaf\n\noff");
}

#[test]
fn title_style_is_a_level_one_heading() {
    let styles =
        r#"<w:style w:type="paragraph" w:styleId="Title"><w:name w:val="Title"/></w:style>"#;
    assert_eq!(md_styled(&styled_p("Title", "T"), styles), "# T");
}

#[test]
fn heading_text_with_braces_does_not_become_heading_attributes() {
    assert_eq!(
        md_styled(&styled_p("Heading1", "A {#x}"), HEADING_STYLES),
        "# A \\{#x\\}"
    );
}

#[test]
fn bold_italic_strike() {
    let body = para(
        &(runp("<w:b/>", "bold")
            + &run(" ")
            + &runp("<w:i/>", "ital")
            + &run(" ")
            + &runp("<w:strike/>", "gone")
            + &run(" ")
            + &runp("<w:dstrike/>", "dg")
            + &run(" ")
            + &runp("<w:b/><w:i/>", "bi")),
    );
    assert_eq!(md(&body), "**bold** *ital* ~~gone~~ ~~dg~~ ***bi***");
}

#[test]
fn explicit_false_values_turn_formatting_off() {
    let styles = r#"<w:style w:type="paragraph" w:styleId="B"><w:name w:val="B"/><w:rPr><w:b/><w:i/></w:rPr></w:style>"#;
    let body = format!(
        r#"<w:p><w:pPr><w:pStyle w:val="B"/></w:pPr>{}{}{}</w:p>"#,
        runp(r#"<w:b w:val="0"/>"#, "i-only"),
        runp(r#"<w:b w:val="0"/><w:i w:val="0"/>"#, " | "),
        runp(r#"<w:b w:val="false"/><w:i w:val="off"/>"#, "plain")
    );
    assert_eq!(md_styled(&body, styles), "*i-only* \\| plain");
}

#[test]
fn character_styles_and_their_chain_apply() {
    let styles = r#"
<w:style w:type="character" w:styleId="Strong"><w:name w:val="Strong"/><w:rPr><w:b/></w:rPr></w:style>
<w:style w:type="character" w:styleId="Emph"><w:name w:val="Emph"/><w:rPr><w:i/></w:rPr></w:style>
<w:style w:type="character" w:styleId="Both"><w:name w:val="Both"/><w:basedOn w:val="Strong"/><w:rPr><w:i/></w:rPr></w:style>"#;
    let body = para(
        &(runp(r#"<w:rStyle w:val="Strong"/>"#, "s")
            + &run(" ")
            + &runp(r#"<w:rStyle w:val="Both"/>"#, "bi")
            + &run(" ")
            + &runp(r#"<w:rStyle w:val="Emph"/><w:b/>"#, "mixed")),
    );
    assert_eq!(md_styled(&body, styles), "**s** ***bi*** ***mixed***");
}

#[test]
fn paragraph_style_formatting_applies_to_its_runs() {
    let styles = r#"<w:style w:type="paragraph" w:styleId="Q"><w:name w:val="Quote"/><w:rPr><w:i/></w:rPr></w:style>"#;
    assert_eq!(md_styled(&styled_p("Q", "quoted"), styles), "*quoted*");
}

#[test]
fn emphasis_never_wraps_leading_or_trailing_spaces() {
    let body = para(&(run("a") + &runp("<w:b/>", " bold ") + &run("b")));
    assert_eq!(md(&body), "a **bold** b");
    let only_spaces = para(&(run("a") + &runp("<w:b/>", "   ") + &run("b")));
    assert_eq!(md(&only_spaces), "a   b");
}

#[test]
fn adjacent_runs_with_the_same_format_are_one_emphasis() {
    let body = para(&(runp("<w:b/>", "one") + &runp("<w:b/>", " two") + &runp("<w:b/>", " three")));
    assert_eq!(md(&body), "**one two three**");
}

#[test]
fn emphasis_ending_in_punctuation_before_a_word_still_renders() {
    // CommonMark: `**done.**next` is not emphasis (the closer follows punctuation and precedes a
    // letter); the reader moves the punctuation out so the bold survives.
    let body = para(
        &(runp("<w:b/>", "日本語。")
            + &run("次の文")
            + &run(" and ")
            + &runp("<w:b/>", "「引用」")
            + &run("次")),
    );
    let m = md(&body);
    assert_eq!(
        m,
        "**日本語**。次の文 and 「**引用**」次".replace("「**引用**」", "**「引用**」")
    );
    assert_eq!(visible(&m), "日本語。次の文 and 「引用」次");
}

#[test]
fn emphasis_starting_with_punctuation_after_a_word_still_renders() {
    let body = para(&(run("前") + &runp("<w:b/>", "「太字」")));
    let m = md(&body);
    assert_eq!(visible(&m), "前「太字」");
    assert!(m.contains("**"), "{m}");
    assert!(!visible(&m).contains('*'), "{m}");
}

// ---------------------------------------------------------------------------------------------
// breaks and special characters
// ---------------------------------------------------------------------------------------------

#[test]
fn a_line_break_is_a_hard_break() {
    let body =
        para(&(run("a") + "<w:r><w:br/></w:r>" + &run("b") + "<w:r><w:cr/></w:r>" + &run("c")));
    assert_eq!(md(&body), "a  \nb  \nc");
    assert_eq!(visible(&md(&body)), "a\nb\nc");
}

#[test]
fn trailing_spaces_before_a_break_are_trimmed() {
    let body = para(&(run("a   ") + "<w:r><w:br/></w:r>" + &run("   b")));
    assert_eq!(md(&body), "a  \nb");
}

#[test]
fn a_page_break_splits_the_paragraph() {
    let body = para(&(run("before") + r#"<w:r><w:br w:type="page"/></w:r>"# + &run("after")));
    assert_eq!(md(&body), "before\n\nafter");
}

#[test]
fn a_column_break_splits_the_paragraph() {
    let body = para(&(run("c1") + r#"<w:r><w:br w:type="column"/></w:r>"# + &run("c2")));
    assert_eq!(md(&body), "c1\n\nc2");
}

#[test]
fn a_page_break_alone_adds_nothing() {
    let body = p("a") + &para(r#"<w:r><w:br w:type="page"/></w:r>"#) + &p("b");
    assert_eq!(md(&body), "a\n\nb");
}

#[test]
fn tabs_become_no_break_spaces() {
    let body = para(&(run("a") + "<w:r><w:tab/></w:r>" + &run("b")));
    assert_eq!(md(&body), "a\u{a0}\u{a0}\u{a0}\u{a0}b");
}

#[test]
fn no_break_hyphen_and_soft_hyphen() {
    let body =
        para("<w:r><w:t>a</w:t><w:noBreakHyphen/><w:t>b</w:t><w:softHyphen/><w:t>c</w:t></w:r>");
    assert_eq!(md(&body), "a\u{2011}bc");
}

#[test]
fn symbols_in_the_private_use_area_are_dropped_and_plain_ones_kept() {
    let body = para(
        r#"<w:r><w:sym w:font="Wingdings" w:char="F0A8"/><w:t>x</w:t><w:sym w:font="Arial" w:char="00A9"/></w:r>"#,
    );
    assert_eq!(md(&body), "x©");
}

#[test]
fn text_without_xml_space_preserve_is_trimmed_like_word_does() {
    let body = para("<w:r><w:t> a </w:t></w:r><w:r><w:t xml:space=\"preserve\"> b </w:t></w:r>");
    assert_eq!(md(&body), "a b");
}

#[test]
fn control_characters_and_zero_width_marks_are_removed() {
    let body = para("<w:r><w:t>a&#x1;b&#x200B;c&#xE000;d</w:t></w:r>");
    assert_eq!(md(&body), "abc\u{FFFD}d");
}

#[test]
fn hidden_text_is_not_shown() {
    let body = para(&(run("shown ") + &runp("<w:vanish/>", "hidden") + &run("!")));
    assert_eq!(md(&body), "shown !");
}

#[test]
fn ruby_shows_the_base_text() {
    let body = para(
        r#"<w:r><w:ruby><w:rubyPr/><w:rt><w:r><w:t>かん</w:t></w:r></w:rt><w:rubyBase><w:r><w:t>漢</w:t></w:r></w:rubyBase></w:ruby></w:r>"#,
    );
    assert_eq!(md(&body), "漢");
}

#[test]
fn entities_in_text_are_decoded_and_dtd_entities_are_never_expanded() {
    let xml = format!(
        r#"<?xml version="1.0"?><!DOCTYPE d [<!ENTITY boom "BOOM">]><w:document {}><w:body><w:p><w:r><w:t>a &amp; b &lt;c&gt; &boom;</w:t></w:r></w:p></w:body></w:document>"#,
        ns()
    );
    let root = format!(
        r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{REL_BASE}/officeDocument" Target="word/document.xml"/></Relationships>"#
    );
    let bytes = deflated(&[
        ("_rels/.rels", root.as_bytes()),
        ("word/document.xml", xml.as_bytes()),
    ]);
    let dir = tmp("docx_dtd");
    let path = write(&dir, "dtd.docx", &bytes);
    let m = load_document(&path, &DocOptions::default())
        .unwrap()
        .markdown;
    assert!(m.starts_with("a & b \\<c> "), "{m}");
    assert!(!m.contains("BOOM"), "{m}");
}
