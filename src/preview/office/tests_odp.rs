//! Tests of the OpenDocument presentation reader (`odp.rs`): each case is a presentation written by
//! hand in the form LibreOffice Impress writes it (the real decks are in `tests_pptx_real.rs`,
//! where they are compared with the PowerPoint versions). Limits and hostile input are in
//! `tests_odp_robust.rs`.

use super::docx::pptx::*;
use super::docx::*;
use super::tests::{deflated, tmp, write};
use super::tests_docx::tiny_png;
use super::tests_odt::ns;
use super::tests_pptx::check_headings;
use super::OfficeError;
use crate::i18n::Lang;

pub(super) const MIME_PRES: &str = "application/vnd.oasis.opendocument.presentation";

/// The namespaces of an Impress `content.xml` (the text reader's, plus presentation and chart).
pub(super) fn pns() -> String {
    format!(
        r#"{} xmlns:presentation="urn:oasis:names:tc:opendocument:xmlns:presentation:1.0" xmlns:chart="urn:oasis:names:tc:opendocument:xmlns:chart:1.0" xmlns:office2="x""#,
        ns()
    )
}

/// An OpenDocument presentation package under construction.
#[derive(Clone)]
pub(super) struct Op {
    /// The `draw:page` elements.
    pub pages: String,
    /// Inside `office:automatic-styles` of `content.xml`.
    pub auto: String,
    /// Inside `office:styles` of `styles.xml`.
    pub styles: Option<String>,
    /// Inside `office:master-styles` of `styles.xml`.
    pub master: Option<String>,
    pub mime: String,
    pub manifest: Option<String>,
    pub extra: Vec<(String, Vec<u8>)>,
}

impl Op {
    pub fn new(pages: &str) -> Op {
        Op {
            pages: pages.to_string(),
            auto: String::new(),
            styles: None,
            master: None,
            mime: MIME_PRES.to_string(),
            manifest: None,
            extra: Vec::new(),
        }
    }
    pub fn auto(mut self, s: &str) -> Op {
        self.auto = s.to_string();
        self
    }
    pub fn styles(mut self, s: &str) -> Op {
        self.styles = Some(s.to_string());
        self
    }
    pub fn master(mut self, s: &str) -> Op {
        self.master = Some(s.to_string());
        self
    }
    pub fn mime(mut self, m: &str) -> Op {
        self.mime = m.to_string();
        self
    }
    pub fn manifest(mut self, m: &str) -> Op {
        self.manifest = Some(m.to_string());
        self
    }
    pub fn part(mut self, name: &str, bytes: &[u8]) -> Op {
        self.extra.push((name.to_string(), bytes.to_vec()));
        self
    }
    pub fn picture(self, name: &str, seed: u8) -> Op {
        self.part(&format!("Pictures/{name}"), &tiny_png(seed))
    }
    pub fn object(self, dir: &str, xml: &str) -> Op {
        self.part(&format!("{dir}/content.xml"), xml.as_bytes())
    }
    pub fn content_xml(&self) -> String {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?><office:document-content {} office:version="1.3"><office:automatic-styles>{}</office:automatic-styles><office:body><office:presentation>{}</office:presentation></office:body></office:document-content>"#,
            pns(),
            self.auto,
            self.pages
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
                    pns(),
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

pub(super) fn load_op(o: &Op, opts: &DocOptions) -> Result<Document, OfficeError> {
    let dir = tmp("odp");
    let p = write(&dir, "t.odp", &o.bytes());
    load_presentation(&p, opts)
}

pub(super) fn doc(o: &Op) -> Document {
    load_op(o, &DocOptions::default()).unwrap()
}

/// The Markdown of one slide of shapes.
pub(super) fn md1(shapes: &str) -> String {
    doc(&Op::new(&page(shapes))).markdown
}

/// The one-slide Markdown without the `## Slide 1` heading line.
pub(super) fn body1(shapes: &str) -> String {
    let m = md1(shapes);
    match m.split_once("\n\n") {
        Some((_, rest)) => rest.to_string(),
        None => String::new(),
    }
}

// ---------------------------------------------------------------------------------------------
// builders
// ---------------------------------------------------------------------------------------------

pub(super) fn page(inner: &str) -> String {
    format!(r#"<draw:page draw:name="p" draw:style-name="dp1">{inner}</draw:page>"#)
}

pub(super) fn page_with(style: &str, master: &str, inner: &str) -> String {
    format!(
        r#"<draw:page draw:name="p" draw:style-name="{style}" draw:master-page-name="{master}">{inner}</draw:page>"#
    )
}

/// A `draw:frame` of `class` (empty: none) at a position in cm.
pub(super) fn fr(class: &str, x: f64, y: f64, w: f64, h: f64, inner: &str) -> String {
    let class = if class.is_empty() {
        String::new()
    } else {
        format!(r#" presentation:class="{class}""#)
    };
    format!(
        r#"<draw:frame draw:layer="layout" svg:width="{w}cm" svg:height="{h}cm" svg:x="{x}cm" svg:y="{y}cm"{class}>{inner}</draw:frame>"#
    )
}

/// A frame with no position at all (a placeholder that follows its master).
pub(super) fn fr_nopos(class: &str, inner: &str) -> String {
    format!(r#"<draw:frame draw:layer="layout" presentation:class="{class}">{inner}</draw:frame>"#)
}

pub(super) fn tbx(inner: &str) -> String {
    format!("<draw:text-box>{inner}</draw:text-box>")
}

pub(super) fn tp(text: &str) -> String {
    format!("<text:p>{text}</text:p>")
}

/// A frame of text at a position.
pub(super) fn tf(x: f64, y: f64, w: f64, h: f64, text: &str) -> String {
    fr("", x, y, w, h, &tbx(&tp(text)))
}

pub(super) fn title(text: &str) -> String {
    fr("title", 1.0, 0.5, 20.0, 2.0, &tbx(&tp(text)))
}

pub(super) const L1: &str = r#"<text:list-style style:name="L1"><text:list-level-style-bullet text:level="1" text:bullet-char="x"/><text:list-level-style-bullet text:level="2" text:bullet-char="x"/><text:list-level-style-bullet text:level="3" text:bullet-char="x"/></text:list-style>"#;

pub(super) fn notes(inner: &str) -> String {
    format!(
        r#"<presentation:notes draw:style-name="dp2"><draw:page-thumbnail draw:layer="layout" svg:width="18cm" svg:height="10cm" svg:x="1cm" svg:y="2cm" draw:page-number="1" presentation:class="page"/>{inner}</presentation:notes>"#
    )
}

pub(super) fn notes_frame(text: &str) -> String {
    fr("notes", 2.0, 13.0, 17.0, 12.0, &tbx(&tp(text)))
}

pub(super) const HIDDEN: &str = r#"<style:style style:name="dpH" style:family="drawing-page"><style:drawing-page-properties presentation:visibility="hidden"/></style:style>"#;

fn heading_lines(d: &Document) -> Vec<String> {
    d.markdown
        .lines()
        .filter(|l| l.starts_with("## "))
        .map(str::to_string)
        .collect()
}

// ---------------------------------------------------------------------------------------------
// slides, headings, hidden
// ---------------------------------------------------------------------------------------------

#[test]
fn one_titled_slide() {
    let d = doc(&Op::new(&page(&title("Hello"))));
    assert_eq!(d.markdown, "## Slide 1: Hello");
    assert_eq!(d.slides.len(), 1);
    assert_eq!(d.slides[0].title, "Hello");
    assert!(!d.slides[0].hidden);
    assert!(!d.truncated);
    check_headings(&d);
}

#[test]
fn slides_are_in_the_order_of_the_draw_pages() {
    let pages: String = (1..=4)
        .map(|i| page(&title(&format!("T{i}"))))
        .collect::<Vec<_>>()
        .join("");
    let d = doc(&Op::new(&pages));
    assert_eq!(
        heading_lines(&d),
        [
            "## Slide 1: T1",
            "## Slide 2: T2",
            "## Slide 3: T3",
            "## Slide 4: T4"
        ]
    );
    check_headings(&d);
}

#[test]
fn a_slide_without_a_title_has_a_bare_heading() {
    let d = doc(&Op::new(&page(&tf(1.0, 1.0, 5.0, 1.0, "just text"))));
    assert_eq!(d.markdown, "## Slide 1\n\njust text");
    assert_eq!(d.slides[0].title, "");
}

#[test]
fn an_empty_presentation_and_an_empty_slide() {
    let d = doc(&Op::new(""));
    assert_eq!(d.markdown, "");
    assert!(d.slides.is_empty());
    let d = doc(&Op::new(&page("")));
    assert_eq!(d.markdown, "## Slide 1");
}

#[test]
fn a_document_with_no_body_at_all_reads_as_empty() {
    let e = [
        ("mimetype".to_string(), MIME_PRES.as_bytes().to_vec()),
        (
            "content.xml".to_string(),
            b"<office:document-content/>".to_vec(),
        ),
    ];
    let refs: Vec<(&str, &[u8])> = e.iter().map(|(n, b)| (n.as_str(), b.as_slice())).collect();
    let dir = tmp("odpempty");
    let p = write(&dir, "x.odp", &deflated(&refs));
    let d = load_presentation(&p, &DocOptions::default()).unwrap();
    assert_eq!(d.markdown, "");
}

#[test]
fn a_hidden_slide_is_marked() {
    let pages = page(&title("Shown"))
        + &page_with("dpH", "Default", &title("Backup"))
        + &page(&title("After"));
    let d = doc(&Op::new(&pages).auto(HIDDEN));
    assert_eq!(
        heading_lines(&d),
        [
            "## Slide 1: Shown",
            "## Slide 2: Backup (hidden)",
            "## Slide 3: After"
        ]
    );
    assert_eq!(
        d.slides.iter().map(|s| s.hidden).collect::<Vec<_>>(),
        [false, true, false]
    );
    check_headings(&d);
}

#[test]
fn hidden_comes_from_the_style_chain_and_the_nearest_setting_wins() {
    // dpB is based on a hidden style; dpC is based on it and shows the slide again; the parents
    // may live in the common styles (`styles.xml`).
    let auto = r#"<style:style style:name="dpB" style:family="drawing-page" style:parent-style-name="dpHidden"/>
        <style:style style:name="dpC" style:family="drawing-page" style:parent-style-name="dpHidden"><style:drawing-page-properties presentation:visibility="visible"/></style:style>
        <style:style style:name="dpD" style:family="drawing-page"><style:drawing-page-properties presentation:background-visible="true"/></style:style>"#;
    let styles = r#"<style:style style:name="dpHidden" style:family="drawing-page"><style:drawing-page-properties presentation:visibility="hidden"/></style:style>"#;
    let pages = page_with("dpB", "m", &title("b"))
        + &page_with("dpC", "m", &title("c"))
        + &page_with("dpD", "m", &title("d"))
        + &page_with("dpMissing", "m", &title("e"));
    let d = doc(&Op::new(&pages).auto(auto).styles(styles));
    assert_eq!(
        d.slides.iter().map(|s| s.hidden).collect::<Vec<_>>(),
        [true, false, false, false]
    );
}

#[test]
fn a_style_cycle_in_the_page_styles_is_harmless() {
    let auto = r#"<style:style style:name="a" style:family="drawing-page" style:parent-style-name="b"/>
        <style:style style:name="b" style:family="drawing-page" style:parent-style-name="a"/>"#;
    let d = doc(&Op::new(&page_with("a", "m", &title("x"))).auto(auto));
    assert_eq!(d.markdown, "## Slide 1: x");
}

#[test]
fn a_paragraph_style_named_like_a_page_style_does_not_hide_the_slide() {
    // (The families are separate: a hidden *text* style called dpH is not a hidden slide.)
    let auto = r#"<style:style style:name="dpH" style:family="text"><style:text-properties text:display="none"/></style:style>"#;
    let d = doc(&Op::new(&page_with("dpH", "m", &title("x"))).auto(auto));
    assert!(!d.slides[0].hidden);
}

#[test]
fn the_template_type_is_read_too() {
    let o = Op::new(&page(&title("T")))
        .mime("application/vnd.oasis.opendocument.presentation-template");
    assert_eq!(doc(&o).markdown, "## Slide 1: T");
}

#[test]
fn headings_are_in_japanese_for_a_japanese_ui() {
    let pages = page(&title("題")) + &page_with("dpH", "m", &title("予備"));
    let opts = DocOptions {
        lang: Lang::Jp,
        ..DocOptions::default()
    };
    let d = load_op(&Op::new(&pages).auto(HIDDEN), &opts).unwrap();
    assert_eq!(
        heading_lines(&d),
        ["## スライド 1: 題", "## スライド 2: 予備（非表示）"]
    );
}

// ---------------------------------------------------------------------------------------------
// titles
// ---------------------------------------------------------------------------------------------

#[test]
fn only_the_first_title_with_text_is_the_heading() {
    let s = fr("title", 1.0, 0.5, 10.0, 1.0, &tbx(&tp("")))
        + &fr("title", 1.0, 1.5, 10.0, 1.0, &tbx(&tp("Real title")))
        + &fr("title", 1.0, 2.5, 10.0, 1.0, &tbx(&tp("Second title")));
    let d = doc(&Op::new(&page(&s)));
    assert_eq!(d.markdown, "## Slide 1: Real title\n\nSecond title");
}

#[test]
fn a_title_with_no_text_is_not_the_heading_but_is_still_shown() {
    // A title frame that holds only a picture: the slide has no title, and the picture is kept.
    let s = fr(
        "title",
        1.0,
        0.5,
        10.0,
        1.0,
        r#"<draw:image xlink:href="Pictures/none.png"/>"#,
    ) + &tf(1.0, 3.0, 5.0, 1.0, "body");
    let d = doc(&Op::new(&page(&s)));
    assert_eq!(d.markdown, "## Slide 1\n\n\\[image]\n\nbody");
    assert_eq!(d.slides[0].title, "");
}

#[test]
fn an_empty_title_placeholder_is_nothing() {
    let s = fr("title", 1.0, 0.5, 10.0, 1.0, "<draw:text-box/>") + &tf(1.0, 3.0, 5.0, 1.0, "body");
    assert_eq!(doc(&Op::new(&page(&s))).markdown, "## Slide 1\n\nbody");
}

#[test]
fn a_title_is_one_line_of_plain_text() {
    let auto = r#"<style:style style:name="B" style:family="text"><style:text-properties fo:font-weight="bold"/></style:style>"#;
    let t = fr(
        "title",
        1.0,
        1.0,
        10.0,
        1.0,
        &tbx(
            r#"<text:p>Big <text:span text:style-name="B">bold</text:span><text:line-break/>two<text:s text:c="3"/>words</text:p><text:p>third <text:a xlink:href="https://x.example/">linked</text:a></text:p>"#,
        ),
    );
    let d = doc(&Op::new(&page(&t)).auto(auto));
    assert_eq!(d.slides[0].title, "Big bold two words third linked");
    assert_eq!(d.markdown, "## Slide 1: Big bold two words third linked");
}

#[test]
fn markdown_in_a_title_is_escaped() {
    let d = doc(&Op::new(&page(&title("# not *bold* [x](y) `c` &lt;b&gt;"))));
    let h = &heading_lines(&d)[0];
    assert!(!h.contains("*bold*") || h.contains("\\*bold\\*"), "{h}");
    assert_eq!(heading_lines(&d).len(), 1);
    assert_eq!(d.markdown.lines().count(), 1, "{}", d.markdown);
    assert_eq!(d.slides[0].title, "# not *bold* [x](y) `c` <b>");
}

#[test]
fn a_very_long_title_is_cut() {
    let d = doc(&Op::new(&page(&title(&"w ".repeat(1000)))));
    assert!(d.slides[0].title.chars().count() <= 200);
    assert_eq!(d.markdown.lines().count(), 1);
}

#[test]
fn the_title_is_first_wherever_it_stands() {
    // Title below the body: still the heading; the subtitle comes first among the rest.
    let s = tf(1.0, 1.0, 5.0, 1.0, "body")
        + &fr("subtitle", 1.0, 20.0, 5.0, 1.0, &tbx(&tp("sub")))
        + &fr("title", 1.0, 25.0, 5.0, 1.0, &tbx(&tp("Low title")));
    assert_eq!(
        doc(&Op::new(&page(&s))).markdown,
        "## Slide 1: Low title\n\nsub\n\nbody"
    );
}

// ---------------------------------------------------------------------------------------------
// text, lists, formatting
// ---------------------------------------------------------------------------------------------

#[test]
fn bullets_and_nested_bullets() {
    let nested = format!(
        r#"<text:list text:style-name="L1"><text:list-item>{}<text:list><text:list-item>{}</text:list-item></text:list></text:list-item><text:list-item>{}</text:list-item></text:list>"#,
        tp("one"),
        tp("deep"),
        tp("two")
    );
    let d = doc(&Op::new(&page(&fr("outline", 1.0, 3.0, 20.0, 9.0, &tbx(&nested)))).auto(L1));
    assert_eq!(d.markdown, "## Slide 1\n\n- one\n  - deep\n- two");
}

#[test]
fn numbered_lists_use_the_documents_numbers() {
    let auto = r#"<text:list-style style:name="N1"><text:list-level-style-number text:level="1" style:num-suffix="." style:num-format="1"/></text:list-style>
        <text:list-style style:name="N2"><text:list-level-style-number text:level="1" style:num-suffix=")" style:num-format="a"/></text:list-style>"#;
    let l1 = format!(
        r#"<text:list text:style-name="N1"><text:list-item>{}</text:list-item><text:list-item>{}</text:list-item></text:list>"#,
        tp("first"),
        tp("second")
    );
    let l2 = format!(
        r#"<text:list text:style-name="N2"><text:list-item>{}</text:list-item><text:list-item>{}</text:list-item></text:list>"#,
        tp("alpha"),
        tp("beta")
    );
    let s = fr("outline", 1.0, 1.0, 10.0, 3.0, &tbx(&l1))
        + &fr("outline", 1.0, 6.0, 10.0, 3.0, &tbx(&l2));
    let b = body1_with(&s, auto);
    assert_eq!(b, "1. first\n2. second\n\na) alpha  \nb) beta");
}

fn body1_with(shapes: &str, auto: &str) -> String {
    let m = doc(&Op::new(&page(shapes)).auto(auto)).markdown;
    m.split_once("\n\n")
        .map(|x| x.1.to_string())
        .unwrap_or_default()
}

#[test]
fn text_formatting_is_kept() {
    let auto = r#"<style:style style:name="B" style:family="text"><style:text-properties fo:font-weight="bold"/></style:style>
        <style:style style:name="I" style:family="text"><style:text-properties fo:font-style="italic"/></style:style>
        <style:style style:name="S" style:family="text"><style:text-properties style:text-line-through-style="solid"/></style:style>
        <style:style style:name="U" style:family="text"><style:text-properties style:text-position="super 58%"/></style:style>"#;
    let t = r#"<text:p>a <text:span text:style-name="B">bold</text:span> <text:span text:style-name="I">it</text:span> <text:span text:style-name="S">gone</text:span> x<text:span text:style-name="U">2</text:span></text:p>"#;
    let b = body1_with(&fr("", 1.0, 1.0, 10.0, 1.0, &tbx(t)), auto);
    assert_eq!(b, "a **bold** *it* ~~gone~~ x<sup>2</sup>");
}

#[test]
fn a_paragraph_style_bold_applies() {
    let auto = r#"<style:style style:name="PB" style:family="paragraph"><style:text-properties fo:font-weight="bold"/></style:style>"#;
    let t = r#"<text:p text:style-name="PB">strong</text:p>"#;
    assert_eq!(
        body1_with(&fr("", 1.0, 1.0, 10.0, 1.0, &tbx(t)), auto),
        "**strong**"
    );
}

#[test]
fn links_are_kept_when_safe() {
    let t = r#"<text:p>See <text:a xlink:href="https://example.com/a b">site</text:a>, <text:a xlink:href="javascript:alert(1)">bad</text:a> and <text:a xlink:href="file:///etc/passwd">file</text:a>.</text:p>"#;
    let b = body1(&fr("", 1.0, 1.0, 10.0, 1.0, &tbx(t)));
    assert!(b.contains("[site](https://example.com/a"), "{b}");
    assert!(!b.contains("javascript"), "{b}");
    assert!(!b.contains("file:///"), "{b}");
    assert!(b.contains("bad") && b.contains("file"), "{b}");
}

#[test]
fn a_text_h_in_a_slide_is_a_paragraph_never_a_heading() {
    let t = r#"<text:h text:outline-level="1">Heading one</text:h><text:h text:outline-level="2">Heading two</text:h><text:p>after</text:p>"#;
    let d = doc(&Op::new(&page(
        &(title("T") + &fr("", 1.0, 3.0, 10.0, 4.0, &tbx(t))),
    )));
    assert_eq!(heading_lines(&d), ["## Slide 1: T"]);
    assert_eq!(
        d.markdown,
        "## Slide 1: T\n\nHeading one\n\nHeading two\n\nafter"
    );
    check_headings(&d);
}

#[test]
fn a_paragraph_style_named_heading_is_not_a_heading_either() {
    let auto = r#"<style:style style:name="Heading_20_1" style:family="paragraph" style:display-name="Heading 1" style:default-outline-level="1"/>"#;
    let t = r#"<text:p text:style-name="Heading_20_1">looks like a heading</text:p>"#;
    let d = doc(&Op::new(&page(&fr("", 1.0, 1.0, 10.0, 1.0, &tbx(t)))).auto(auto));
    assert_eq!(d.markdown, "## Slide 1\n\nlooks like a heading");
}

#[test]
fn markdown_looking_text_is_escaped() {
    let ps = [
        "# h1",
        "## h2",
        "- dash",
        "1. one",
        "> quote",
        "---",
        "***",
        "```",
        "| a | b |",
        "[x](y)",
        "![i](j)",
        "$x$ and $$y$$",
        "*em* _u_ ~~s~~",
        "Setext\n===",
    ];
    let t: String = ps.iter().map(|p| tp(&p.replace('\n', " "))).collect();
    let d = doc(&Op::new(&page(
        &(title("T") + &fr("", 1.0, 3.0, 10.0, 10.0, &tbx(&t))),
    )));
    check_headings(&d);
    assert_eq!(heading_lines(&d).len(), 1, "{}", d.markdown);
    let shown = super::tests_docx::visible(&d.markdown);
    for p in ps {
        let p = p.replace('\n', " ");
        assert!(shown.contains(p.as_str()), "{p:?} not shown in\n{shown}");
    }
}

#[test]
fn private_use_markers_and_control_characters_cannot_forge_anything() {
    let t = tp("a\u{E000}link\u{E001}b \u{0}\u{7}\u{1b}[31mred");
    let d = doc(&Op::new(&page(&fr("", 1.0, 1.0, 10.0, 1.0, &tbx(&t)))));
    assert!(!d.markdown.contains('\u{E000}') && !d.markdown.contains('\u{E001}'));
    assert!(!d.markdown.contains('\u{1b}') && !d.markdown.contains('\u{0}'));
    check_headings(&d);
}

#[test]
fn comments_are_not_shown() {
    let t = r#"<text:p>kept<office:annotation><dc:creator>Eve</dc:creator><text:p>SECRET comment</text:p></office:annotation> text</text:p>"#;
    let b = body1(&fr("", 1.0, 1.0, 10.0, 1.0, &tbx(t)));
    assert!(
        b.contains("kept") && !b.contains("SECRET") && !b.contains("Eve"),
        "{b}"
    );
}

#[test]
fn hidden_text_is_not_shown() {
    let auto = r#"<style:style style:name="X" style:family="text"><style:text-properties text:display="none"/></style:style>"#;
    let t = r#"<text:p>a<text:span text:style-name="X">HIDDENTEXT</text:span>b</text:p>"#;
    let d = doc(&Op::new(&page(&fr("", 1.0, 1.0, 10.0, 1.0, &tbx(t)))).auto(auto));
    assert_eq!(d.markdown, "## Slide 1\n\nab");
}

#[test]
fn a_footnote_in_a_slide_keeps_the_headings_intact() {
    let t = r#"<text:p>text<text:note text:note-class="footnote"><text:note-citation>1</text:note-citation><text:note-body><text:p>the note</text:p></text:note-body></text:note></text:p>"#;
    let d = doc(&Op::new(&page(
        &(title("T") + &fr("", 1.0, 3.0, 10.0, 1.0, &tbx(t))),
    )));
    assert!(d.markdown.contains("[^1]") && d.markdown.contains("the note"));
    check_headings(&d);
}

// ---------------------------------------------------------------------------------------------
// reading order
// ---------------------------------------------------------------------------------------------

#[test]
fn shapes_are_read_in_position_order_not_file_order() {
    let s = tf(1.0, 12.0, 5.0, 1.0, "bottom")
        + &tf(1.0, 3.0, 5.0, 1.0, "top")
        + &tf(1.0, 7.0, 5.0, 1.0, "middle");
    assert_eq!(body1(&s), "top\n\nmiddle\n\nbottom");
}

#[test]
fn a_row_reads_left_to_right() {
    let s = tf(12.0, 3.0, 5.0, 1.0, "right")
        + &tf(1.0, 3.0, 5.0, 1.0, "left")
        + &tf(6.5, 3.05, 5.0, 1.0, "centre");
    assert_eq!(body1(&s), "left\n\ncentre\n\nright");
}

#[test]
fn two_columns_read_column_by_column() {
    let col =
        |x: f64, a: &str, b: &str| -> String { tf(x, 3.0, 8.0, 4.0, a) + &tf(x, 8.0, 8.0, 4.0, b) };
    let s = col(14.0, "R1", "R2") + &col(1.0, "L1", "L2");
    assert_eq!(body1(&s), "L1\n\nL2\n\nR1\n\nR2");
}

#[test]
fn a_shape_with_no_position_comes_last_in_file_order() {
    let s = fr_nopos("", &tbx(&tp("loose one")))
        + &tf(1.0, 9.0, 5.0, 1.0, "placed")
        + &fr_nopos("", &tbx(&tp("loose two")));
    assert_eq!(body1(&s), "placed\n\nloose one\n\nloose two");
}

#[test]
fn lengths_in_every_unit_are_compared_in_one_scale() {
    // 2in = 5.08cm = 50.8mm = 144pt = 12pc = 192px: the shapes are stated in different units, the
    // order is the one of the positions (B is above A by a hair, the same row would sort by x).
    let at = |x: &str, y: &str, t: &str| -> String {
        format!(
            r#"<draw:frame svg:width="1cm" svg:height="1cm" svg:x="{x}" svg:y="{y}"><draw:text-box>{}</draw:text-box></draw:frame>"#,
            tp(t)
        )
    };
    let s = at("0cm", "6cm", "z-in") // 6cm
        + &at("0cm", "2.3in", "y") // 5.842cm
        + &at("0cm", "150pt", "x") // 5.29cm
        + &at("0cm", "13pc", "w") // 5.5cm
        + &at("0cm", "180px", "v") // 4.76cm
        + &at("0cm", "45mm", "u"); // 4.5cm
    assert_eq!(body1(&s), "u\n\nv\n\nx\n\nw\n\ny\n\nz-in");
}

#[test]
fn a_rotated_shape_is_placed_by_its_bounding_box() {
    // A 10cm x 2cm bar rotated a quarter turn about the origin and moved: its box is 2cm wide and
    // 10cm tall. Placed at y=3cm it still comes before a shape at y=5cm, though its unrotated
    // rectangle (x=1..11, y=14..16 here) would come after.
    let rot = r#"<draw:custom-shape draw:layer="layout" svg:width="10cm" svg:height="2cm" draw:transform="rotate (1.5707963267949) translate (3cm 1cm)"><text:p>bar</text:p></draw:custom-shape>"#;
    let s = tf(1.0, 5.0, 5.0, 1.0, "after") + rot;
    assert_eq!(body1(&s), "bar\n\nafter");
}

// ---------------------------------------------------------------------------------------------
// the master page
// ---------------------------------------------------------------------------------------------

fn master_with(frames: &str) -> String {
    format!(
        r#"<style:master-page style:name="Default" style:page-layout-name="PM1">{frames}</style:master-page>"#
    )
}

#[test]
fn a_placeholder_with_no_position_takes_its_masters() {
    // The outline of the master is at the top: the position-less outline of the slide is placed
    // there and comes first; without the master it would be loose (last).
    let m = master_with(&fr("outline", 1.0, 2.0, 20.0, 5.0, &tbx("")));
    let s = tf(1.0, 9.0, 5.0, 1.0, "placed") + &fr_nopos("outline", &tbx(&tp("from master")));
    let with = doc(&Op::new(&page_with("dp1", "Default", &s)).master(&m)).markdown;
    assert_eq!(with, "## Slide 1\n\nfrom master\n\nplaced");
    let without = doc(&Op::new(&page_with("dp1", "Default", &s))).markdown;
    assert_eq!(without, "## Slide 1\n\nplaced\n\nfrom master");
    // A slide of another master gets nothing.
    let other = doc(&Op::new(&page_with("dp1", "Other", &s)).master(&m)).markdown;
    assert_eq!(other, without);
}

#[test]
fn the_masters_position_is_by_class_and_the_first_one_counts() {
    let m = master_with(
        &(fr("title", 0.0, 0.0, 5.0, 1.0, &tbx(""))
            + &fr("outline", 1.0, 12.0, 5.0, 1.0, &tbx(""))
            + &fr("outline", 1.0, 1.0, 5.0, 1.0, &tbx(""))),
    );
    let s = tf(1.0, 9.0, 5.0, 1.0, "mid") + &fr_nopos("outline", &tbx(&tp("low")));
    let d = doc(&Op::new(&page_with("dp1", "Default", &s)).master(&m));
    assert_eq!(d.markdown, "## Slide 1\n\nmid\n\nlow");
}

#[test]
fn the_footer_frames_of_a_slide_are_not_shown() {
    let s = title("T")
        + &fr("footer", 1.0, 20.0, 5.0, 1.0, &tbx(&tp("FOOTER TEXT")))
        + &fr("date-time", 8.0, 20.0, 5.0, 1.0, &tbx(&tp("DATE TEXT")))
        + &fr("page-number", 16.0, 20.0, 5.0, 1.0, &tbx(&tp("PAGENUM")))
        + &fr("header", 16.0, 0.0, 5.0, 1.0, &tbx(&tp("HEADER TEXT")))
        + &tf(1.0, 3.0, 5.0, 1.0, "content");
    assert_eq!(md1(&s), "## Slide 1: T\n\ncontent");
}

#[test]
fn a_damaged_styles_part_costs_the_inheritance_not_the_deck() {
    let o = Op::new(&page_with(
        "dp1",
        "Default",
        &(title("T") + &fr_nopos("outline", &tbx(&tp("x")))),
    ))
    .styles("<<<broken");
    let d = doc(&o);
    assert_eq!(d.markdown, "## Slide 1: T\n\nx");
}

// ---------------------------------------------------------------------------------------------
// speaker notes
// ---------------------------------------------------------------------------------------------

#[test]
fn the_notes_follow_the_slide_as_a_quote() {
    let s = title("T") + &notes(&notes_frame("Say this."));
    assert_eq!(md1(&s), "## Slide 1: T\n\n> **Notes**  \n> Say this.");
}

#[test]
fn notes_paragraphs_markdown_and_bullets() {
    let nf = fr(
        "notes",
        2.0,
        13.0,
        17.0,
        12.0,
        &tbx(&(tp("first")
            + &tp("# second")
            + &format!(
                r#"<text:list text:style-name="L1"><text:list-item>{}</text:list-item></text:list>"#,
                tp("listed")
            ))),
    );
    let d = doc(&Op::new(&page(&(title("T") + &notes(&nf)))).auto(L1));
    assert_eq!(
        d.markdown,
        "## Slide 1: T\n\n> **Notes**  \n> first  \n> \\# second  \n> \u{2022} listed"
    );
    check_headings(&d);
}

#[test]
fn empty_notes_the_thumbnail_and_other_note_frames_show_nothing() {
    let s = title("T")
        + &notes(
            &(fr("notes", 2.0, 13.0, 17.0, 12.0, "<draw:text-box/>")
                + &fr("header", 0.0, 0.0, 5.0, 1.0, &tbx(&tp("NOTE HEADER")))
                + &fr("", 0.0, 5.0, 5.0, 1.0, &tbx(&tp("NOTE STRAY")))),
        );
    assert_eq!(md1(&s), "## Slide 1: T");
}

#[test]
fn notes_are_not_in_the_slides_reading_order() {
    // The notes frame is positioned above the content; it is still under the slide.
    let nf = fr("notes", 0.0, 0.0, 5.0, 1.0, &tbx(&tp("NOTE")));
    let s = tf(1.0, 3.0, 5.0, 1.0, "content") + &notes(&nf);
    assert_eq!(md1(&s), "## Slide 1\n\ncontent\n\n> **Notes**  \n> NOTE");
}

#[test]
fn notes_in_japanese() {
    let opts = DocOptions {
        lang: Lang::Jp,
        ..DocOptions::default()
    };
    let d = load_op(
        &Op::new(&page(&(title("題") + &notes(&notes_frame("話す"))))),
        &opts,
    )
    .unwrap();
    assert_eq!(d.markdown, "## スライド 1: 題\n\n> **ノート**  \n> 話す");
}

// ---------------------------------------------------------------------------------------------
// groups
// ---------------------------------------------------------------------------------------------

#[test]
fn a_group_is_one_block_ordered_inside() {
    // The group's two members are at y=3 and y=12; a shape at y=7 sits between them in a flat
    // order, but the group is read as a block placed at its bounding box (starting at y=3).
    let g = format!(
        "<draw:g>{}{}</draw:g>",
        tf(12.0, 12.0, 5.0, 1.0, "g-bottom"),
        tf(12.0, 3.0, 5.0, 1.0, "g-top")
    );
    let s = tf(1.0, 7.0, 5.0, 1.0, "left-mid") + &g;
    // Columns: the left shape, then the group (its members top to bottom).
    assert_eq!(body1(&s), "left-mid\n\ng-top\n\ng-bottom");
}

#[test]
fn nested_groups_and_hyperlinked_shapes_are_read() {
    let inner = format!("<draw:g>{}</draw:g>", tf(1.0, 5.0, 5.0, 1.0, "deep"));
    let linked = format!(
        r#"<draw:a xlink:href="https://x.example/">{}</draw:a>"#,
        tf(1.0, 9.0, 5.0, 1.0, "linked shape")
    );
    let s = format!(
        "<draw:g>{}{}</draw:g>",
        tf(1.0, 3.0, 5.0, 1.0, "top"),
        inner
    ) + &linked;
    assert_eq!(body1(&s), "top\n\ndeep\n\nlinked shape");
}

#[test]
fn a_group_without_content_is_nothing() {
    let g = r#"<draw:g><draw:line svg:x1="0cm" svg:y1="0cm" svg:x2="3cm" svg:y2="3cm"/><draw:connector svg:x1="0cm" svg:y1="0cm" svg:x2="3cm" svg:y2="3cm"/></draw:g>"#;
    assert_eq!(md1(&(title("T") + g)), "## Slide 1: T");
}

#[test]
fn shapes_that_hold_text_are_read_and_lines_are_not() {
    let s = r#"<draw:custom-shape svg:width="4cm" svg:height="2cm" svg:x="1cm" svg:y="3cm"><text:p>in a box</text:p><draw:enhanced-geometry draw:type="rectangle"/></draw:custom-shape>
        <draw:rect svg:width="4cm" svg:height="2cm" svg:x="1cm" svg:y="8cm"><text:p>rect text</text:p></draw:rect>
        <draw:ellipse svg:width="4cm" svg:height="2cm" svg:x="1cm" svg:y="12cm"/>
        <draw:line svg:x1="0cm" svg:y1="0cm" svg:x2="9cm" svg:y2="0cm"/>
        <draw:custom-shape svg:width="4cm" svg:height="2cm" svg:x="1cm" svg:y="16cm"><text:list text:style-name="L1"><text:list-item><text:p>listed in shape</text:p></text:list-item></text:list></draw:custom-shape>"#;
    let d = doc(&Op::new(&page(s)).auto(L1));
    assert_eq!(
        d.markdown,
        "## Slide 1\n\nin a box\n\nrect text\n\n- listed in shape"
    );
}

// ---------------------------------------------------------------------------------------------
// tables
// ---------------------------------------------------------------------------------------------

fn cell(t: &str) -> String {
    format!("<table:table-cell>{}</table:table-cell>", tp(t))
}

#[test]
fn a_table_is_a_gfm_table_and_its_preview_picture_is_not_shown() {
    let t = format!(
        r#"<table:table><table:table-column table:number-columns-repeated="2"/><table:table-row>{}{}</table:table-row><table:table-row>{}{}</table:table-row></table:table><draw:image xlink:href="Pictures/TablePreview1.svm"/>"#,
        cell("Name"),
        cell("A|B"),
        cell("x"),
        cell("*y*")
    );
    let d = doc(
        &Op::new(&page(&(title("T") + &fr("", 2.0, 5.0, 20.0, 5.0, &t))))
            .picture("TablePreview1.svm", 1),
    );
    assert_eq!(
        d.markdown,
        "## Slide 1: T\n\n| Name | A\\|B |\n| --- | --- |\n| x | \u{2217}y\u{2217} |"
    );
    assert!(d.images.is_empty());
}

#[test]
fn merged_cells_and_repeated_columns() {
    let t = format!(
        r#"<table:table><table:table-row><table:table-cell table:number-columns-spanned="2">{}</table:table-cell><table:covered-table-cell/><table:table-cell>{}</table:table-cell></table:table-row><table:table-row><table:table-cell table:number-columns-repeated="3">{}</table:table-cell></table:table-row></table:table>"#,
        tp("wide"),
        tp("c"),
        tp("r")
    );
    let b = body1(&fr("", 2.0, 5.0, 20.0, 5.0, &t));
    assert_eq!(b, "| wide |   | c |\n| --- | --- | --- |\n| r | r | r |");
}

#[test]
fn text_in_a_table_cell_is_flat_and_escaped() {
    let t = format!(
        r#"<table:table><table:table-row><table:table-cell>{}{}</table:table-cell></table:table-row></table:table>"#,
        tp("line one"),
        tp("# two | x")
    );
    let b = body1(&fr("", 2.0, 5.0, 20.0, 5.0, &t));
    assert!(b.contains("line one<br>"), "{b}");
    assert!(!b.contains("\n# "), "{b}");
    assert_eq!(b.lines().count(), 2, "{b}");
}

// ---------------------------------------------------------------------------------------------
// pictures, formulas, charts
// ---------------------------------------------------------------------------------------------

fn pic_frame(href: &str, alt: &str) -> String {
    fr(
        "",
        2.0,
        4.0,
        8.0,
        4.0,
        &format!(
            r#"<draw:image xlink:href="{href}" xlink:type="simple"><text:p/></draw:image><svg:desc>{alt}</svg:desc>"#
        ),
    )
}

#[test]
fn a_picture_is_shown_with_its_alternative_text() {
    let o =
        Op::new(&page(&pic_frame("Pictures/a.png", "A gradient *picture*"))).picture("a.png", 3);
    let d = doc(&o);
    assert_eq!(d.images.len(), 1);
    assert!(
        d.markdown
            .starts_with("## Slide 1\n\n![A gradient picture](office-img://"),
        "{}",
        d.markdown
    );
    assert!(d.markdown.ends_with("/a.png)"));
    assert_eq!(d.images[0].name, "a.png");
}

#[test]
fn the_same_picture_twice_is_stored_once() {
    let s = pic_frame("Pictures/a.png", "one")
        + &fr(
            "",
            2.0,
            10.0,
            8.0,
            4.0,
            r#"<draw:image xlink:href="./Pictures/a.png"/>"#,
        );
    let d = doc(&Op::new(&page(&s)).picture("a.png", 3));
    assert_eq!(d.images.len(), 1);
    assert_eq!(d.markdown.matches("office-img://").count(), 2);
}

#[test]
fn a_missing_external_or_odd_picture_is_a_placeholder() {
    for href in [
        "Pictures/missing.png",
        "https://example.com/x.png",
        "../outside.png",
        "/abs.png",
        "Pictures/a.txt",
    ] {
        let o = Op::new(&page(&pic_frame(href, "alt text")))
            .picture("a.txt", 1)
            .part("../outside.png", &tiny_png(2));
        let d = doc(&o);
        assert_eq!(d.markdown, "## Slide 1\n\n\\[alt text]", "{href}");
        assert!(d.images.is_empty());
    }
}

#[test]
fn a_picture_without_alternative_text_is_an_image_placeholder_when_unreadable() {
    let d = doc(&Op::new(&page(&fr(
        "",
        1.0,
        1.0,
        5.0,
        5.0,
        r#"<draw:image xlink:href="Pictures/none.png"/>"#,
    ))));
    assert_eq!(d.markdown, "## Slide 1\n\n\\[image]");
}

const MML: &str = r#"<?xml version="1.0" encoding="UTF-8"?><math xmlns="http://www.w3.org/1998/Math/MathML" display="block"><semantics><mrow><mi>E</mi><mo stretchy="false">=</mo><mi>m</mi><msup><mi>c</mi><mn>2</mn></msup></mrow><annotation encoding="StarMath 5.0">E = m c^2</annotation></semantics></math>"#;

fn object_frame(href: &str, x: f64, y: f64) -> String {
    fr(
        "",
        x,
        y,
        6.0,
        2.0,
        &format!(
            r#"<draw:object xlink:href="{href}" xlink:type="simple"/><draw:image xlink:href="./ObjectReplacements/{href}"/>"#
        ),
    )
}

#[test]
fn a_formula_object_is_math() {
    let d = doc(&Op::new(&page(&object_frame("./Object 1", 2.0, 4.0))).object("Object 1", MML));
    assert_eq!(d.markdown, "## Slide 1\n\n$$\nE=m{c}^{2}\n$$");
    assert_eq!((d.math_total, d.math_latex), (1, 1));
}

fn chart_xml(title: Option<&str>, axis: Option<&str>) -> String {
    let t = title
        .map(|t| format!("<chart:title><text:p>{t}</text:p></chart:title>"))
        .unwrap_or_default();
    let a = axis
        .map(|t| format!("<chart:plot-area><chart:axis chart:dimension=\"x\"><chart:title><text:p>{t}</text:p></chart:title></chart:axis></chart:plot-area>"))
        .unwrap_or_default();
    format!(
        r#"<?xml version="1.0"?><office:document-content {}><office:automatic-styles/><office:body><office:chart><chart:chart chart:class="chart:bar">{t}{a}</chart:chart></office:chart></office:body></office:document-content>"#,
        pns()
    )
}

#[test]
fn a_chart_object_is_a_labelled_placeholder() {
    let o = Op::new(&page(&object_frame("./Object 1", 2.0, 4.0)))
        .object("Object 1", &chart_xml(Some("Revenue *2025*"), Some("Axis")));
    assert_eq!(
        doc(&o).markdown,
        "## Slide 1\n\n\\[chart: Revenue \\*2025\\*]"
    );
}

#[test]
fn a_chart_with_only_an_axis_title_has_no_title() {
    let o = Op::new(&page(&object_frame("./Object 1", 2.0, 4.0)))
        .object("Object 1", &chart_xml(None, Some("Only the axis")));
    assert_eq!(doc(&o).markdown, "## Slide 1\n\n\\[chart]");
}

#[test]
fn a_chart_whose_part_is_missing_or_not_a_chart_is_a_plain_object() {
    // (An unreadable object falls back to what a Writer frame does: the picture, else a placeholder.)
    let missing = doc(&Op::new(&page(&object_frame("./Object 9", 2.0, 4.0))));
    assert_eq!(missing.markdown, "## Slide 1\n\n\\[image]");
    let text_doc = format!(
        r#"<office:document-content {}><office:body><office:text><text:p>x</text:p></office:text></office:body></office:document-content>"#,
        pns()
    );
    let o =
        doc(&Op::new(&page(&object_frame("./Object 1", 2.0, 4.0))).object("Object 1", &text_doc));
    assert_eq!(o.markdown, "## Slide 1\n\n\\[image]");
}

#[test]
fn a_frame_with_text_and_a_chart_title_with_markdown_is_safe() {
    let o = Op::new(&page(&(title("T") + &object_frame("./Object 1", 2.0, 4.0)))).object(
        "Object 1",
        &chart_xml(Some("# ![x](y) [z](javascript:1)"), None),
    );
    let d = doc(&o);
    check_headings(&d);
    assert_eq!(
        d.markdown.lines().filter(|l| l.starts_with('#')).count(),
        1,
        "{}",
        d.markdown
    );
    // (Every bracket is escaped: no link and no image is made of the title.)
    assert!(
        d.markdown.contains("!\\[x](y) \\[z](javascript:1)"),
        "{}",
        d.markdown
    );
}

// ---------------------------------------------------------------------------------------------
// the package
// ---------------------------------------------------------------------------------------------

#[test]
fn other_opendocument_types_and_other_formats_are_not_presentations() {
    let o = Op::new(&page(&title("T")));
    for mime in [
        "application/vnd.oasis.opendocument.text",
        "application/vnd.oasis.opendocument.spreadsheet",
        "application/vnd.oasis.opendocument.graphics",
        "application/zip",
        "",
    ] {
        assert_eq!(
            load_op(&o.clone().mime(mime), &DocOptions::default()).unwrap_err(),
            OfficeError::Unsupported,
            "{mime:?}"
        );
    }
    // And a presentation is not a document of the text reader.
    let dir = tmp("odpastext");
    let p = write(&dir, "t.odt", &o.bytes());
    assert_eq!(
        load_document(&p, &DocOptions::default()).unwrap_err(),
        OfficeError::Unsupported
    );
}

#[test]
fn a_package_without_mimetype_or_content_is_refused() {
    let o = Op::new(&page(&title("T")));
    let no_mime: Vec<_> = o
        .entries()
        .into_iter()
        .filter(|(n, _)| n != "mimetype")
        .collect();
    let refs: Vec<(&str, &[u8])> = no_mime
        .iter()
        .map(|(n, b)| (n.as_str(), b.as_slice()))
        .collect();
    let dir = tmp("odpnomime");
    let p = write(&dir, "x.odp", &deflated(&refs));
    assert_eq!(
        load_presentation(&p, &DocOptions::default()).unwrap_err(),
        OfficeError::Unsupported
    );
    // (Without a mimetype the package is not an OpenDocument one: it falls to the PowerPoint reader.)
    let no_content: Vec<_> = o
        .entries()
        .into_iter()
        .filter(|(n, _)| n != "content.xml")
        .collect();
    let refs: Vec<(&str, &[u8])> = no_content
        .iter()
        .map(|(n, b)| (n.as_str(), b.as_slice()))
        .collect();
    let p = write(&dir, "y.odp", &deflated(&refs));
    assert!(matches!(
        load_presentation(&p, &DocOptions::default()).unwrap_err(),
        OfficeError::Corrupt(_)
    ));
}

#[test]
fn an_encrypted_presentation_is_reported() {
    let enc = r#"<?xml version="1.0"?><manifest:manifest xmlns:manifest="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0"><manifest:file-entry manifest:full-path="content.xml" manifest:media-type="text/xml"><manifest:encryption-data manifest:checksum-type="SHA1/1K"><manifest:algorithm manifest:algorithm-name="Blowfish CFB"/></manifest:encryption-data></manifest:file-entry></manifest:manifest>"#;
    let r = load_op(
        &Op::new(&page(&title("T"))).manifest(enc),
        &DocOptions::default(),
    );
    assert_eq!(r.unwrap_err(), OfficeError::Encrypted);
}

#[test]
fn a_content_that_is_not_a_presentation_body_is_unsupported() {
    // A text body in a package that says "presentation".
    let content = format!(
        r#"<office:document-content {}><office:body><office:text><text:p>x</text:p></office:text></office:body></office:document-content>"#,
        pns()
    );
    let e = [
        ("mimetype".to_string(), MIME_PRES.as_bytes().to_vec()),
        ("content.xml".to_string(), content.into_bytes()),
    ];
    let refs: Vec<(&str, &[u8])> = e.iter().map(|(n, b)| (n.as_str(), b.as_slice())).collect();
    let dir = tmp("odpbody");
    let p = write(&dir, "x.odp", &deflated(&refs));
    assert_eq!(
        load_presentation(&p, &DocOptions::default()).unwrap_err(),
        OfficeError::Unsupported
    );
}

#[test]
fn other_elements_in_the_presentation_are_skipped() {
    let pages = format!(
        r#"<presentation:header-decl presentation:name="h1">Header</presentation:header-decl>{}<presentation:settings presentation:mouse-visible="false"><presentation:show presentation:name="x" presentation:pages="a"/></presentation:settings>{}"#,
        page(&title("A")),
        page(&title("B"))
    );
    let d = doc(&Op::new(&pages));
    assert_eq!(heading_lines(&d), ["## Slide 1: A", "## Slide 2: B"]);
}
