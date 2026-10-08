//! Tests of the PowerPoint reader (`pptx.rs`): the corpus is built from the cases of the design
//! (section 11), each one a presentation written by hand in the form PowerPoint writes.

use super::docx::pptx::*;
use super::docx::*;
use super::tests::{deflated, tmp, write};
use super::tests_docx::{rendered, tiny_png, REL_BASE};
use super::*;
use crate::i18n::Lang;

pub(super) const I: i64 = 914_400;

pub(super) const PNS: &str = r#"xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006" xmlns:a14="http://schemas.microsoft.com/office/drawing/2010/main" xmlns:m="http://schemas.openxmlformats.org/officeDocument/2006/math" xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart" xmlns:dgm="http://schemas.openxmlformats.org/drawingml/2006/diagram" xmlns:dsp="http://schemas.microsoft.com/office/drawing/2008/diagram""#;

// ---------------------------------------------------------------------------------------------
// shape builders
// ---------------------------------------------------------------------------------------------

/// `a:xfrm` for a rectangle.
pub(super) fn xf(x: i64, y: i64, w: i64, h: i64) -> String {
    format!(r#"<a:xfrm><a:off x="{x}" y="{y}"/><a:ext cx="{w}" cy="{h}"/></a:xfrm>"#)
}

/// A paragraph of one plain run.
pub(super) fn pa(text: &str) -> String {
    format!(r#"<a:p><a:r><a:rPr lang="en-US"/><a:t>{text}</a:t></a:r></a:p>"#)
}

/// A paragraph at a level.
pub(super) fn pl(lvl: u8, text: &str) -> String {
    format!(r#"<a:p><a:pPr lvl="{lvl}"/><a:r><a:rPr lang="en-US"/><a:t>{text}</a:t></a:r></a:p>"#)
}

/// A paragraph with paragraph properties (children of `a:pPr`, attributes).
pub(super) fn pp(attrs: &str, inner: &str, text: &str) -> String {
    format!(r#"<a:p><a:pPr {attrs}>{inner}</a:pPr><a:r><a:t>{text}</a:t></a:r></a:p>"#)
}

/// A run with run-property attributes.
pub(super) fn ru(attrs: &str, text: &str) -> String {
    format!(r#"<a:r><a:rPr lang="en-US" {attrs}/><a:t>{text}</a:t></a:r>"#)
}

pub(super) fn paras(inner: &str) -> String {
    format!("<a:p>{inner}</a:p>")
}

pub(super) fn sp_raw(id: u32, ph: &str, spp: &str, tx: &str) -> String {
    format!(
        r#"<p:sp><p:nvSpPr><p:cNvPr id="{id}" name="S{id}"/><p:cNvSpPr/><p:nvPr>{ph}</p:nvPr></p:nvSpPr><p:spPr>{spp}</p:spPr><p:txBody><a:bodyPr/><a:lstStyle/>{tx}</p:txBody></p:sp>"#
    )
}

/// A text box at a position (not a placeholder: no bullets unless asked).
pub(super) fn tb(x: i64, y: i64, w: i64, h: i64, tx: &str) -> String {
    sp_raw(10, "", &xf(x, y, w, h), tx)
}

/// A text box with no position at all.
pub(super) fn tb_nopos(tx: &str) -> String {
    sp_raw(11, "", "", tx)
}

/// A placeholder (`attrs` of `p:ph`), with a position or inheriting one.
pub(super) fn ph(attrs: &str, pos: Option<(i64, i64, i64, i64)>, tx: &str) -> String {
    let spp = pos.map(|(x, y, w, h)| xf(x, y, w, h)).unwrap_or_default();
    sp_raw(20, &format!("<p:ph {attrs}/>"), &spp, tx)
}

pub(super) fn title(text: &str) -> String {
    ph(r#"type="title""#, Some((I, 0, 8 * I, I)), &pa(text))
}

pub(super) fn body(pos: (i64, i64, i64, i64), tx: &str) -> String {
    ph(r#"idx="1""#, Some(pos), tx)
}

pub(super) fn group(off: (i64, i64, i64, i64), ch: (i64, i64, i64, i64), inner: &str) -> String {
    format!(
        r#"<p:grpSp><p:nvGrpSpPr><p:cNvPr id="30" name="G"/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr><a:xfrm><a:off x="{}" y="{}"/><a:ext cx="{}" cy="{}"/><a:chOff x="{}" y="{}"/><a:chExt cx="{}" cy="{}"/></a:xfrm></p:grpSpPr>{inner}</p:grpSp>"#,
        off.0, off.1, off.2, off.3, ch.0, ch.1, ch.2, ch.3
    )
}

pub(super) fn pic(rid: &str, alt: &str, pos: Option<(i64, i64, i64, i64)>) -> String {
    let spp = pos.map(|(x, y, w, h)| xf(x, y, w, h)).unwrap_or_default();
    format!(
        r#"<p:pic><p:nvPicPr><p:cNvPr id="40" name="Pic" descr="{alt}"/><p:cNvPicPr/><p:nvPr/></p:nvPicPr><p:blipFill><a:blip r:embed="{rid}"/></p:blipFill><p:spPr>{spp}</p:spPr></p:pic>"#
    )
}

/// A table graphic frame; `rows` are `<a:tr>` strings.
pub(super) fn table(pos: (i64, i64, i64, i64), rows: &str) -> String {
    format!(
        r#"<p:graphicFrame><p:nvGraphicFramePr><p:cNvPr id="50" name="T"/><p:cNvGraphicFramePr/><p:nvPr/></p:nvGraphicFramePr><p:xfrm><a:off x="{}" y="{}"/><a:ext cx="{}" cy="{}"/></p:xfrm><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/table"><a:tbl><a:tblPr/><a:tblGrid/>{rows}</a:tbl></a:graphicData></a:graphic></p:graphicFrame>"#,
        pos.0, pos.1, pos.2, pos.3
    )
}

pub(super) fn tr(cells: &[&str]) -> String {
    let mut s = String::from("<a:tr h=\"370840\">");
    for c in cells {
        s += &format!(
            r#"<a:tc><a:txBody><a:bodyPr/><a:lstStyle/>{}</a:txBody><a:tcPr/></a:tc>"#,
            pa(c)
        );
    }
    s + "</a:tr>"
}

pub(super) fn chart_frame(rid: &str, pos: (i64, i64, i64, i64)) -> String {
    format!(
        r#"<p:graphicFrame><p:nvGraphicFramePr><p:cNvPr id="60" name="C"/><p:cNvGraphicFramePr/><p:nvPr/></p:nvGraphicFramePr><p:xfrm><a:off x="{}" y="{}"/><a:ext cx="{}" cy="{}"/></p:xfrm><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/chart"><c:chart r:id="{rid}"/></a:graphicData></a:graphic></p:graphicFrame>"#,
        pos.0, pos.1, pos.2, pos.3
    )
}

pub(super) fn smart_frame(dm: &str, pos: (i64, i64, i64, i64)) -> String {
    format!(
        r#"<p:graphicFrame><p:nvGraphicFramePr><p:cNvPr id="70" name="D"/><p:cNvGraphicFramePr/><p:nvPr/></p:nvGraphicFramePr><p:xfrm><a:off x="{}" y="{}"/><a:ext cx="{}" cy="{}"/></p:xfrm><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/diagram"><dgm:relIds r:dm="{dm}" r:lo="rIdL" r:qs="rIdQ" r:cs="rIdC"/></a:graphicData></a:graphic></p:graphicFrame>"#,
        pos.0, pos.1, pos.2, pos.3
    )
}

/// A connector (never shown).
pub(super) fn connector() -> String {
    r#"<p:cxnSp><p:nvCxnSpPr><p:cNvPr id="80" name="K"/><p:cNvCxnSpPr/><p:nvPr/></p:nvCxnSpPr><p:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="100" cy="100"/></a:xfrm></p:spPr></p:cxnSp>"#.to_string()
}

// ---------------------------------------------------------------------------------------------
// the package
// ---------------------------------------------------------------------------------------------

type RelRow = (String, String, String, bool);

#[derive(Clone, Default)]
pub(super) struct Sl {
    /// The shapes inside `p:spTree`.
    pub shapes: String,
    /// The raw `show` attribute of `p:sld`.
    pub show: Option<String>,
    /// Which layout (index into [`Px::layouts`]).
    pub layout: usize,
    /// The shapes of the notes page, when the slide has one.
    pub notes: Option<String>,
    pub notes_rels: Vec<RelRow>,
    /// More relationships of the slide (`id`, type tail, target, external).
    pub rels: Vec<RelRow>,
    /// The slide's file name under `ppt/slides/` (default `slide<n>.xml`).
    pub file: Option<String>,
}

pub(super) fn sl(shapes: &str) -> Sl {
    Sl {
        shapes: shapes.to_string(),
        ..Sl::default()
    }
}

impl Sl {
    pub fn hidden(mut self) -> Sl {
        self.show = Some("0".into());
        self
    }
    pub fn show(mut self, v: &str) -> Sl {
        self.show = Some(v.into());
        self
    }
    pub fn notes(mut self, shapes: &str) -> Sl {
        self.notes = Some(shapes.to_string());
        self
    }
    pub fn rel(mut self, id: &str, ty: &str, target: &str, ext: bool) -> Sl {
        self.rels.push((id.into(), ty.into(), target.into(), ext));
        self
    }
    pub fn file(mut self, f: &str) -> Sl {
        self.file = Some(f.to_string());
        self
    }
    pub fn layout(mut self, i: usize) -> Sl {
        self.layout = i;
        self
    }
}

/// The master's text styles: a bullet on every level of the body.
pub(super) const BODY_BULLETS: &str = r#"<p:txStyles><p:titleStyle><a:lvl1pPr><a:buNone/></a:lvl1pPr></p:titleStyle><p:bodyStyle><a:lvl1pPr><a:buChar char="&#8226;"/></a:lvl1pPr><a:lvl2pPr><a:buChar char="-"/></a:lvl2pPr><a:lvl3pPr><a:buChar char="&#8226;"/></a:lvl3pPr><a:lvl4pPr><a:buChar char="&#8226;"/></a:lvl4pPr></p:bodyStyle><p:otherStyle><a:lvl1pPr><a:buNone/></a:lvl1pPr></p:otherStyle></p:txStyles>"#;

#[derive(Clone)]
pub(super) struct Px {
    pub slides: Vec<Sl>,
    /// The `spTree` contents of each layout.
    pub layouts: Vec<String>,
    pub master: String,
    pub tx_styles: String,
    pub media: Vec<(String, Vec<u8>)>,
    pub extra: Vec<(String, Vec<u8>)>,
}

impl Px {
    pub fn new(slides: Vec<Sl>) -> Px {
        Px {
            slides,
            layouts: vec![String::new()],
            master: [
                ph(r#"type="title""#, Some((I, I / 2, 8 * I, I)), &pa("Click")),
                ph(
                    r#"type="body" idx="1""#,
                    Some((I, 2 * I, 8 * I, 4 * I)),
                    &pa("Click"),
                ),
            ]
            .concat(),
            tx_styles: BODY_BULLETS.to_string(),
            media: Vec::new(),
            extra: Vec::new(),
        }
    }

    pub fn layout(mut self, i: usize, shapes: &str) -> Px {
        while self.layouts.len() <= i {
            self.layouts.push(String::new());
        }
        self.layouts[i] = shapes.to_string();
        self
    }

    pub fn master(mut self, shapes: &str, styles: &str) -> Px {
        self.master = shapes.to_string();
        self.tx_styles = styles.to_string();
        self
    }

    pub fn media(mut self, name: &str, bytes: Vec<u8>) -> Px {
        self.media.push((name.into(), bytes));
        self
    }

    pub fn part(mut self, name: &str, bytes: &[u8]) -> Px {
        self.extra.push((name.into(), bytes.to_vec()));
        self
    }

    pub fn entries(&self) -> Vec<(String, Vec<u8>)> {
        let hdr = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#;
        let rels_xml = |rows: &[RelRow]| {
            let mut s = format!(
                r#"{hdr}<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">"#
            );
            for (id, ty, target, ext) in rows {
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
        };
        let tree = |inner: &str| {
            format!(
                r#"<p:cSld><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/>{inner}</p:spTree></p:cSld>"#
            )
        };
        let mut out: Vec<(String, Vec<u8>)> = vec![
            (
                "[Content_Types].xml".into(),
                format!(r#"{hdr}<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"/>"#)
                    .into_bytes(),
            ),
            (
                "_rels/.rels".into(),
                rels_xml(&[(
                    "rId1".into(),
                    "officeDocument".into(),
                    "ppt/presentation.xml".into(),
                    false,
                )])
                .into_bytes(),
            ),
        ];
        // presentation.xml: the order of `sldId` is the order of the slides.
        let mut prels: Vec<RelRow> = vec![(
            "rIdM".into(),
            "slideMaster".into(),
            "slideMasters/slideMaster1.xml".into(),
            false,
        )];
        let mut ids = String::new();
        for (i, s) in self.slides.iter().enumerate() {
            let file = s
                .file
                .clone()
                .unwrap_or_else(|| format!("slide{}.xml", i + 1));
            prels.push((
                format!("rIdS{i}"),
                "slide".into(),
                format!("slides/{file}"),
                false,
            ));
            ids += &format!(r#"<p:sldId id="{}" r:id="rIdS{i}"/>"#, 256 + i);
        }
        out.push((
            "ppt/presentation.xml".into(),
            format!(
                r#"{hdr}<p:presentation {PNS}><p:sldMasterIdLst><p:sldMasterId id="2147483648" r:id="rIdM"/></p:sldMasterIdLst><p:sldIdLst>{ids}</p:sldIdLst><p:sldSz cx="9144000" cy="6858000"/></p:presentation>"#
            )
            .into_bytes(),
        ));
        out.push((
            "ppt/_rels/presentation.xml.rels".into(),
            rels_xml(&prels).into_bytes(),
        ));
        out.push((
            "ppt/slideMasters/slideMaster1.xml".into(),
            format!(
                r#"{hdr}<p:sldMaster {PNS}>{}{}</p:sldMaster>"#,
                tree(&self.master),
                self.tx_styles
            )
            .into_bytes(),
        ));
        for (i, l) in self.layouts.iter().enumerate() {
            out.push((
                format!("ppt/slideLayouts/slideLayout{}.xml", i + 1),
                format!(r#"{hdr}<p:sldLayout {PNS}>{}</p:sldLayout>"#, tree(l)).into_bytes(),
            ));
            out.push((
                format!("ppt/slideLayouts/_rels/slideLayout{}.xml.rels", i + 1),
                rels_xml(&[(
                    "rId1".into(),
                    "slideMaster".into(),
                    "../slideMasters/slideMaster1.xml".into(),
                    false,
                )])
                .into_bytes(),
            ));
        }
        for (i, s) in self.slides.iter().enumerate() {
            let file = s
                .file
                .clone()
                .unwrap_or_else(|| format!("slide{}.xml", i + 1));
            let show = s
                .show
                .as_ref()
                .map(|v| format!(r#" show="{v}""#))
                .unwrap_or_default();
            out.push((
                format!("ppt/slides/{file}"),
                format!(
                    r#"{hdr}<p:sld {PNS}{show}>{}<p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:sld>"#,
                    tree(&s.shapes)
                )
                .into_bytes(),
            ));
            let mut rows: Vec<RelRow> = vec![(
                "rIdLay".into(),
                "slideLayout".into(),
                format!("../slideLayouts/slideLayout{}.xml", s.layout + 1),
                false,
            )];
            if s.notes.is_some() {
                rows.push((
                    "rIdN".into(),
                    "notesSlide".into(),
                    format!("../notesSlides/notesSlide{}.xml", i + 1),
                    false,
                ));
            }
            rows.extend(s.rels.iter().cloned());
            out.push((
                format!("ppt/slides/_rels/{file}.rels"),
                rels_xml(&rows).into_bytes(),
            ));
            if let Some(n) = &s.notes {
                out.push((
                    format!("ppt/notesSlides/notesSlide{}.xml", i + 1),
                    format!(r#"{hdr}<p:notes {PNS}>{}</p:notes>"#, tree(n)).into_bytes(),
                ));
                if !s.notes_rels.is_empty() {
                    out.push((
                        format!("ppt/notesSlides/_rels/notesSlide{}.xml.rels", i + 1),
                        rels_xml(&s.notes_rels).into_bytes(),
                    ));
                }
            }
        }
        for (n, b) in &self.media {
            out.push((format!("ppt/media/{n}"), b.clone()));
        }
        for (n, b) in &self.extra {
            out.push((n.clone(), b.clone()));
        }
        out
    }

    pub fn bytes(&self) -> Vec<u8> {
        let e = self.entries();
        let refs: Vec<(&str, &[u8])> = e.iter().map(|(n, b)| (n.as_str(), b.as_slice())).collect();
        deflated(&refs)
    }
}

pub(super) fn load_px(px: &Px, opts: &DocOptions) -> Result<Document, OfficeError> {
    let dir = tmp("pptx");
    let p = write(&dir, "t.pptx", &px.bytes());
    load_presentation(&p, opts)
}

pub(super) fn doc(px: &Px) -> Document {
    load_px(px, &DocOptions::default()).unwrap()
}

/// The Markdown of one slide of shapes.
pub(super) fn md1(shapes: &str) -> String {
    doc(&Px::new(vec![sl(shapes)])).markdown
}

/// The one-slide Markdown without the `## Slide 1` heading line.
pub(super) fn body1(shapes: &str) -> String {
    let m = md1(shapes);
    match m.split_once("\n\n") {
        Some((_, rest)) => rest.to_string(),
        None => String::new(),
    }
}

/// The invariant of the App: one `## ` heading per slide, in order, and no other.
pub(super) fn check_headings(d: &Document) {
    let h2: Vec<&str> = d
        .markdown
        .lines()
        .filter(|l| l.starts_with("## "))
        .collect();
    assert_eq!(h2.len(), d.slides.len(), "{}", d.markdown);
    for (h, s) in h2.iter().zip(&d.slides) {
        assert!(
            h.starts_with(&format!("## Slide {}", s.number)),
            "{h:?} vs {s:?}"
        );
    }
    let numbers: Vec<usize> = d.slides.iter().map(|s| s.number).collect();
    let mut sorted = numbers.clone();
    sorted.sort_unstable();
    assert_eq!(numbers, sorted);
}

// ---------------------------------------------------------------------------------------------
// the order of slides, headings
// ---------------------------------------------------------------------------------------------

#[test]
fn one_titled_slide() {
    let d = doc(&Px::new(vec![sl(&title("Hello"))]));
    assert_eq!(d.markdown, "## Slide 1: Hello");
    assert_eq!(
        d.slides,
        vec![SlideInfo {
            number: 1,
            title: "Hello".into(),
            hidden: false
        }]
    );
    assert!(!d.truncated);
    check_headings(&d);
}

#[test]
fn a_slide_without_a_title_has_a_bare_heading() {
    let d = doc(&Px::new(vec![sl(&tb(0, 0, I, I, &pa("just text")))]));
    assert_eq!(d.markdown, "## Slide 1\n\njust text");
    assert_eq!(d.slides[0].title, "");
}

#[test]
fn an_empty_presentation_has_no_slides() {
    let d = doc(&Px::new(vec![]));
    assert_eq!(d.markdown, "");
    assert!(d.slides.is_empty());
}

#[test]
fn an_empty_slide_is_only_its_heading() {
    let d = doc(&Px::new(vec![sl("")]));
    assert_eq!(d.markdown, "## Slide 1");
    check_headings(&d);
}

#[test]
fn the_order_is_the_slide_id_list_not_the_file_names() {
    // The first slide of the deck lives in slide9.xml, the second in slide2.xml, the third in slide10.xml.
    let px = Px::new(vec![
        sl(&title("first")).file("slide9.xml"),
        sl(&title("second")).file("slide2.xml"),
        sl(&title("third")).file("slide10.xml"),
    ]);
    let d = doc(&px);
    assert_eq!(
        d.markdown,
        "## Slide 1: first\n\n## Slide 2: second\n\n## Slide 3: third"
    );
    check_headings(&d);
}

#[test]
fn hidden_slides_are_marked_in_both_languages() {
    let px = Px::new(vec![
        sl(&title("a")),
        sl(&title("b")).hidden(),
        sl("").hidden(),
    ]);
    let d = doc(&px);
    assert_eq!(
        d.markdown,
        "## Slide 1: a\n\n## Slide 2: b (hidden)\n\n## Slide 3 (hidden)"
    );
    assert_eq!(
        d.slides.iter().map(|s| s.hidden).collect::<Vec<_>>(),
        vec![false, true, true]
    );
    let opts = DocOptions {
        lang: Lang::Jp,
        ..DocOptions::default()
    };
    let d = load_px(&px, &opts).unwrap();
    assert_eq!(
        d.markdown,
        "## スライド 1: a\n\n## スライド 2: b (非表示)\n\n## スライド 3 (非表示)"
    );
}

#[test]
fn show_values_other_than_off_do_not_hide() {
    let px = Px::new(vec![
        sl(&title("a")).show("false"),
        sl(&title("b")).show("1"),
        sl(&title("c")).show("true"),
    ]);
    let d = doc(&px);
    assert_eq!(
        d.slides.iter().map(|s| s.hidden).collect::<Vec<_>>(),
        vec![true, false, false]
    );
}

#[test]
fn a_title_is_one_line_with_markdown_made_literal() {
    for (t, want) in [
        ("# not a heading", "## Slide 1: \\# not a heading"),
        ("> quote", "## Slide 1: \\> quote"),
        ("- item", "## Slide 1: \\- item"),
        ("a | b", "## Slide 1: a \\| b"),
        ("$5 and $10", "## Slide 1: \\$5 and \\$10"),
        ("*x* `y`", "## Slide 1: \\*x\\* \\`y\\`"),
        ("1. one", "## Slide 1: 1\\. one"),
        ("<b>x</b>", "## Slide 1: \\<b>x\\</b>"),
        ("tail #", "## Slide 1: tail \\#"),
    ] {
        let d = doc(&Px::new(vec![sl(&title(
            &t.replace('<', "&lt;").replace('>', "&gt;"),
        ))]));
        assert_eq!(d.markdown, want, "{t:?}");
        assert_eq!(d.slides[0].title, t);
        check_headings(&d);
    }
}

#[test]
fn a_title_with_a_line_break_or_two_paragraphs_is_one_line() {
    let t = format!(
        "{}{}",
        paras(&format!("{}<a:br/>{}", ru("", "one"), ru("", "two"))),
        pa("three")
    );
    let d = doc(&Px::new(vec![sl(&ph(
        r#"type="title""#,
        Some((0, 0, I, I)),
        &t,
    ))]));
    assert_eq!(d.markdown, "## Slide 1: one two three");
}

#[test]
fn a_very_long_title_is_cut() {
    let long = "w".repeat(1000);
    let d = doc(&Px::new(vec![sl(&title(&long))]));
    assert_eq!(d.slides[0].title.chars().count(), 200);
    assert!(d.markdown.len() < 300);
}

#[test]
fn a_centred_title_is_the_heading_and_a_second_title_is_text() {
    let s = [
        ph(r#"type="ctrTitle""#, Some((0, I, I, I)), &pa("Main")),
        ph(r#"type="title""#, Some((0, 0, I, I)), &pa("Other")),
    ]
    .concat();
    let d = doc(&Px::new(vec![sl(&s)]));
    assert_eq!(d.markdown, "## Slide 1: Main\n\nOther");
    check_headings(&d);
}

#[test]
fn nothing_inside_a_slide_is_ever_a_heading() {
    let s = [
        title("T"),
        tb(
            0,
            I,
            I,
            I,
            &(pa("## sneaky") + &pa("# another") + &pa("###### six")),
        ),
        body((0, 2 * I, I, I), &pa("## in a bullet")),
    ]
    .concat();
    let d = doc(&Px::new(vec![sl(&s), sl(&title("U"))]));
    check_headings(&d);
    assert_eq!(d.slides.len(), 2);
    assert!(d.markdown.contains("\\## sneaky"), "{}", d.markdown);
}

// ---------------------------------------------------------------------------------------------
// text, bullets, numbering
// ---------------------------------------------------------------------------------------------

#[test]
fn a_body_placeholder_gets_the_masters_bullets() {
    let s = [
        title("T"),
        body(
            (I, 2 * I, 8 * I, 3 * I),
            &(pa("one") + &pa("two") + &pl(1, "nested") + &pa("three")),
        ),
    ]
    .concat();
    assert_eq!(
        md1(&s),
        "## Slide 1: T\n\n- one\n- two\n  - nested\n- three"
    );
}

#[test]
fn a_text_box_has_no_bullets_and_paragraphs_are_separate() {
    assert_eq!(
        body1(&tb(0, 0, I, I, &(pa("one") + &pa("two")))),
        "one\n\ntwo"
    );
}

#[test]
fn the_shapes_own_bullets_override_the_masters() {
    let tx = format!(
        r#"<a:bodyPr/><a:lstStyle><a:lvl1pPr><a:buNone/></a:lvl1pPr></a:lstStyle>{}"#,
        pa("plain")
    );
    let s = format!(
        r#"<p:sp><p:nvSpPr><p:cNvPr id="2" name="b"/><p:cNvSpPr/><p:nvPr><p:ph idx="1"/></p:nvPr></p:nvSpPr><p:spPr>{}</p:spPr><p:txBody>{tx}</p:txBody></p:sp>"#,
        xf(0, 0, I, I)
    );
    assert_eq!(body1(&s), "plain");
}

#[test]
fn a_paragraph_can_switch_its_bullet_off_or_on() {
    let s = body(
        (0, 0, I, I),
        &(pp("", "<a:buNone/>", "no bullet") + &pa("bullet")),
    );
    assert_eq!(body1(&s), "no bullet\n\n- bullet");
    let s = tb(
        0,
        0,
        I,
        I,
        &(pp("", r#"<a:buChar char="*"/>"#, "yes") + &pa("no")),
    );
    assert_eq!(body1(&s), "- yes\n\nno");
}

#[test]
fn the_layouts_placeholder_style_comes_before_the_masters() {
    // The layout's idx=1 placeholder turns bullets off (as a subtitle layout does).
    let layout = ph(r#"idx="1""#, Some((0, 0, I, I)), "").replace(
        "<a:lstStyle/>",
        r#"<a:lstStyle><a:lvl1pPr><a:buNone/></a:lvl1pPr></a:lstStyle>"#,
    );
    let px = Px::new(vec![sl(&body((0, 0, I, I), &pa("flat")))]).layout(0, &layout);
    assert_eq!(doc(&px).markdown, "## Slide 1\n\nflat");
}

#[test]
fn a_subtitle_does_not_inherit_the_body_bullets_of_the_master() {
    let s = ph(r#"type="subTitle" idx="1""#, Some((0, 0, I, I)), &pa("sub"));
    assert_eq!(body1(&s), "sub");
}

fn num_p(lvl: u8, ty: &str, start: Option<u32>, t: &str) -> String {
    let st = start
        .map(|s| format!(r#" startAt="{s}""#))
        .unwrap_or_default();
    format!(
        r#"<a:p><a:pPr lvl="{lvl}"><a:buAutoNum type="{ty}"{st}/></a:pPr><a:r><a:t>{t}</a:t></a:r></a:p>"#
    )
}

#[test]
fn auto_numbering_counts_and_restarts() {
    let tx = [
        num_p(0, "arabicPeriod", None, "a"),
        num_p(0, "arabicPeriod", None, "b"),
        num_p(1, "arabicPeriod", None, "b1"),
        num_p(1, "arabicPeriod", None, "b2"),
        num_p(0, "arabicPeriod", None, "c"),
        num_p(1, "arabicPeriod", None, "c1"),
    ]
    .concat();
    assert_eq!(
        body1(&tb(0, 0, I, I, &tx)),
        "1. a\n2. b\n   1. b1\n   2. b2\n3. c\n   1. c1"
    );
    let tx = [
        num_p(0, "arabicPeriod", Some(5), "five"),
        num_p(0, "arabicPeriod", Some(5), "six"),
    ]
    .concat();
    assert_eq!(body1(&tb(0, 0, I, I, &tx)), "5. five\n6. six");
    // A plain paragraph in between restarts the count.
    let tx = [
        num_p(0, "arabicPeriod", None, "a"),
        pa("break"),
        num_p(0, "arabicPeriod", None, "b"),
    ]
    .concat();
    let m = body1(&tb(0, 0, I, I, &tx));
    assert!(m.starts_with("1. a\n\nbreak\n\n"), "{m:?}");
    assert!(m.ends_with("1. b"), "{m:?}");
}

#[test]
fn other_number_formats_keep_the_label_as_text() {
    let tx = [
        num_p(0, "alphaLcPeriod", None, "x"),
        num_p(0, "alphaLcPeriod", None, "y"),
        num_p(0, "romanUcPeriod", None, "z"),
        num_p(0, "arabicParenR", None, "p"),
        num_p(0, "arabicParenBoth", None, "q"),
        num_p(0, "alphaUcParenR", None, "r"),
        num_p(0, "circleNumDbPlain", None, "s"),
    ]
    .concat();
    let m = body1(&tb(0, 0, I, I, &tx));
    for want in [
        "a. x",
        "b. y",
        "I. z",
        "1\\) p",
        "(1) q",
        "A) r",
        "\u{2460} s",
    ] {
        assert!(m.contains(want), "{want:?} in {m:?}");
    }
}

#[test]
fn formatting_becomes_markdown() {
    let tx = paras(
        &[
            ru(r#"b="1""#, "bold"),
            ru("", " "),
            ru(r#"i="1""#, "italic"),
            ru("", " "),
            ru(r#"strike="sngStrike""#, "gone"),
            ru("", " x"),
            ru(r#"baseline="30000""#, "2"),
            ru("", " H"),
            ru(r#"baseline="-25000""#, "2"),
            ru("", "O"),
            ru(r#"b="0" i="false" strike="noStrike""#, " plain"),
        ]
        .concat(),
    );
    assert_eq!(
        body1(&tb(0, 0, I, I, &tx)),
        "**bold** *italic* ~~gone~~ x<sup>2</sup> H<sub>2</sub>O plain"
    );
}

#[test]
fn line_breaks_and_vertical_tabs() {
    let tx = paras(&format!("{}<a:br/>{}", ru("", "one"), ru("", "two")));
    assert_eq!(body1(&tb(0, 0, I, I, &tx)), "one  \ntwo");
    let tx = paras(&ru("", "a&#xB;b"));
    assert_eq!(body1(&tb(0, 0, I, I, &tx)), "a  \nb");
}

#[test]
fn fields_other_than_the_slide_number_keep_their_stored_text() {
    let tx = paras(&format!(
        r#"{}<a:fld id="{{1}}" type="datetime1"><a:rPr/><a:t>7/4/2026</a:t></a:fld>"#,
        ru("", "on ")
    ));
    assert_eq!(body1(&tb(0, 0, I, I, &tx)), "on 7/4/2026");
}

#[test]
fn empty_paragraphs_vanish_and_do_not_break_numbering() {
    let tx = [
        num_p(0, "arabicPeriod", None, "a"),
        pa(""),
        "<a:p/>".to_string(),
        pa("  "),
        num_p(0, "arabicPeriod", None, "b"),
    ]
    .concat();
    assert_eq!(body1(&tb(0, 0, I, I, &tx)), "1. a\n2. b");
}

#[test]
fn markdown_in_text_is_made_literal_at_every_line_start() {
    for (t, want) in [
        ("# h", "\\# h"),
        ("> q", "\\> q"),
        ("- i", "\\- i"),
        ("+ i", "\\+ i"),
        ("1. n", "1\\. n"),
        ("| a | b |", "\\| a \\| b \\|"),
        ("---", "\u{200B}\\---"),
        ("```", "\\`\\`\\`"),
        ("$$", "\\$\\$"),
        ("<div>", "\\<div>"),
        ("[x](y)", "\\[x](y)"),
        ("a_b_c *d*", "a_b_c \\*d\\*"),
    ] {
        let tx = pa(&t.replace('<', "&lt;").replace('>', "&gt;"));
        let got = body1(&tb(0, 0, I, I, &tx));
        assert_eq!(got, want, "{t:?}");
        // Whatever it is, it is one paragraph and the slide still has exactly one heading.
        check_headings(&doc(&Px::new(vec![sl(&tb(0, 0, I, I, &tx))])));
    }
}

#[test]
fn markdown_after_a_line_break_is_also_at_a_line_start() {
    let tx = paras(&format!("{}<a:br/>{}", ru("", "ok"), ru("", "# h")));
    assert_eq!(body1(&tb(0, 0, I, I, &tx)), "ok  \n\\# h");
    let tx = paras(&format!("{}<a:br/>{}", ru("", "ok"), ru("", "---")));
    let m = body1(&tb(0, 0, I, I, &tx));
    assert!(m.contains("\u{200B}\\---"), "{m:?}");
}

#[test]
fn a_bullet_with_markdown_like_text_stays_a_bullet() {
    let s = body((0, 0, I, I), &(pa("# h") + &pa("- x") + &pa("> q")));
    assert_eq!(body1(&s), "- \\# h\n- \\- x\n- \\> q");
}

#[test]
fn superscript_at_a_line_start_cannot_open_a_block() {
    let tx = paras(&ru(r#"baseline="30000""#, "# x"));
    let m = body1(&tb(0, 0, I, I, &tx));
    assert!(m.contains("\\# x"), "{m:?}");
}

// ---------------------------------------------------------------------------------------------
// links
// ---------------------------------------------------------------------------------------------

fn link_run(rid: &str, text: &str) -> String {
    format!(r#"<a:r><a:rPr><a:hlinkClick r:id="{rid}"/></a:rPr><a:t>{text}</a:t></a:r>"#)
}

#[test]
fn external_links_and_the_scheme_allow_list() {
    let tx = paras(
        &(link_run("rIdA", "web")
            + &ru("", " ")
            + &link_run("rIdB", "mail")
            + &ru("", " ")
            + &link_run("rIdC", "js")
            + &ru("", " ")
            + &link_run("rIdD", "file")),
    );
    let s = sl(&tb(0, 0, I, I, &tx))
        .rel("rIdA", "hyperlink", "https://example.com/a b", true)
        .rel("rIdB", "hyperlink", "mailto:x@example.com", true)
        .rel("rIdC", "hyperlink", "javascript:alert(1)", true)
        .rel("rIdD", "hyperlink", "file:///etc/passwd", true);
    let m = doc(&Px::new(vec![s])).markdown;
    assert!(m.contains("[web](https://example.com/a%20b)"), "{m}");
    assert!(m.contains("[mail](mailto:x@example.com)"), "{m}");
    assert!(m.contains(" js "), "{m}");
    assert!(!m.contains("javascript"), "{m}");
    assert!(!m.contains("file:"), "{m}");
}

#[test]
fn adjacent_runs_of_one_link_are_one_link() {
    let tx = paras(&(link_run("rIdA", "one ") + &link_run("rIdA", "two")));
    let s = sl(&tb(0, 0, I, I, &tx)).rel("rIdA", "hyperlink", "https://e.example/", true);
    let m = doc(&Px::new(vec![s])).markdown;
    assert!(m.contains("[one two](https://e.example/)"), "{m}");
}

#[test]
fn a_link_to_another_slide_or_a_missing_relationship_is_text() {
    let tx = paras(
        &(r#"<a:r><a:rPr><a:hlinkClick r:id="" action="ppaction://hlinksldjump"/></a:rPr><a:t>jump</a:t></a:r>"#
            .to_string()
            + &link_run("rIdNone", " gone")
            + &link_run("rIdInt", " internal")),
    );
    let s = sl(&tb(0, 0, I, I, &tx)).rel("rIdInt", "slide", "slide2.xml", false);
    assert_eq!(
        doc(&Px::new(vec![s])).markdown,
        "## Slide 1\n\njump gone internal"
    );
}

#[test]
fn link_text_with_brackets_cannot_end_the_link() {
    let tx = paras(&link_run("rIdA", "a ] b [ c"));
    let s = sl(&tb(0, 0, I, I, &tx)).rel("rIdA", "hyperlink", "https://e.example/", true);
    let m = doc(&Px::new(vec![s])).markdown;
    assert!(m.contains("](https://e.example/)"), "{m}");
    assert_eq!(m.matches("](").count(), 1, "{m}");
}

// ---------------------------------------------------------------------------------------------
// reading order and positions
// ---------------------------------------------------------------------------------------------

pub(super) fn lines(m: &str) -> Vec<&str> {
    m.lines().filter(|l| !l.is_empty()).collect()
}

#[test]
fn shapes_are_read_by_position_not_by_file_order() {
    // The file lists the bottom box first.
    let s = [
        tb(0, 5 * I, 4 * I, I, &pa("bottom")),
        tb(0, 0, 4 * I, I, &pa("top")),
        tb(0, 2 * I, 4 * I, I, &pa("middle")),
    ]
    .concat();
    assert_eq!(
        lines(&md1(&s)),
        vec!["## Slide 1", "top", "middle", "bottom"]
    );
}

#[test]
fn the_title_comes_first_and_the_subtitle_before_the_rest() {
    let s = [
        tb(0, 0, 4 * I, I, &pa("body above")),
        ph(
            r#"type="subTitle" idx="1""#,
            Some((0, 6 * I, 4 * I, I)),
            &pa("subtitle low"),
        ),
        ph(
            r#"type="title""#,
            Some((0, 7 * I, 4 * I, I)),
            &pa("Title Low"),
        ),
    ]
    .concat();
    assert_eq!(
        lines(&md1(&s)),
        vec!["## Slide 1: Title Low", "subtitle low", "body above"]
    );
}

#[test]
fn two_columns_are_read_column_by_column() {
    let s = [
        tb(5 * I, I, 4 * I, 3 * I, &pa("right")),
        tb(0, 3 * I, 4 * I, 2 * I, &pa("left lower")),
        tb(0, 0, 4 * I, 2 * I, &pa("left upper")),
    ]
    .concat();
    assert_eq!(
        lines(&md1(&s)),
        vec!["## Slide 1", "left upper", "left lower", "right"]
    );
}

#[test]
fn a_left_right_comparison_is_read_one_side_after_the_other() {
    // Headers with their texts below: the columns are 1 inch apart, the rows 0.5 inch.
    let s = [
        tb(5 * I, 0, 4 * I, I, &pa("Right header")),
        tb(0, 0, 4 * I, I, &pa("Left header")),
        tb(5 * I, I + I / 2, 4 * I, 3 * I, &pa("Right text")),
        tb(0, I + I / 2, 4 * I, 3 * I, &pa("Left text")),
    ]
    .concat();
    assert_eq!(
        lines(&md1(&s)),
        vec![
            "## Slide 1",
            "Left header",
            "Left text",
            "Right header",
            "Right text"
        ]
    );
}

#[test]
fn a_grid_with_wide_row_gaps_is_read_row_by_row() {
    // The same four boxes, the rows 1.5 inches apart and the columns 1 inch: rows.
    let s = [
        tb(5 * I, 0, 4 * I, I, &pa("B")),
        tb(0, 0, 4 * I, I, &pa("A")),
        tb(5 * I, 2 * I + I / 2, 4 * I, I, &pa("D")),
        tb(0, 2 * I + I / 2, 4 * I, I, &pa("C")),
    ]
    .concat();
    assert_eq!(lines(&md1(&s)), vec!["## Slide 1", "A", "B", "C", "D"]);
}

#[test]
fn a_header_over_two_columns_comes_first() {
    let s = [
        tb(5 * I, 2 * I, 4 * I, 3 * I, &pa("right")),
        tb(0, 2 * I, 4 * I, 3 * I, &pa("left")),
        tb(0, 0, 9 * I, I, &pa("wide header")),
    ]
    .concat();
    assert_eq!(
        lines(&md1(&s)),
        vec!["## Slide 1", "wide header", "left", "right"]
    );
}

#[test]
fn a_picture_with_its_caption_and_a_text_column() {
    let s = [
        tb(5 * I, 0, 4 * I, 4 * I, &pa("text column")),
        tb(0, 3 * I + 100, 4 * I, I / 2, &pa("caption")),
        pic("rId5", "figure", Some((0, 0, 4 * I, 3 * I))),
    ]
    .concat();
    let px = Px::new(vec![sl(&s).rel("rId5", "image", "../media/f.png", false)])
        .media("f.png", tiny_png(1));
    let m = doc(&px).markdown;
    let l = lines(&m);
    assert!(l[1].starts_with("![figure](office-img://"), "{m}");
    assert_eq!(&l[2..], &["caption", "text column"], "{m}");
}

#[test]
fn rows_use_the_tolerance_for_the_same_top() {
    // The right box starts a hair above the left one; they are one row, left first.
    let s = [
        tb(5 * I, 0, 4 * I, 4 * I, &pa("R")),
        tb(0, 30_000, 4 * I, 4 * I, &pa("L")),
    ]
    .concat();
    assert_eq!(lines(&md1(&s)), vec!["## Slide 1", "L", "R"]);
}

#[test]
fn shapes_with_no_position_come_last_in_file_order() {
    let s = [
        tb_nopos(&pa("loose one")),
        tb(0, 5 * I, I, I, &pa("low")),
        tb_nopos(&pa("loose two")),
        tb(0, 0, I, I, &pa("high")),
    ]
    .concat();
    assert_eq!(
        lines(&md1(&s)),
        vec!["## Slide 1", "high", "low", "loose one", "loose two"]
    );
}

#[test]
fn a_placeholder_without_xfrm_takes_the_layouts_position() {
    // The layout puts idx=2 on the left and idx=1 on the right; the slide lists them the other way.
    let layout = [
        ph(r#"idx="1""#, Some((5 * I, I, 4 * I, 4 * I)), ""),
        ph(r#"idx="2""#, Some((0, I, 4 * I, 4 * I)), ""),
    ]
    .concat();
    let s = [
        ph(r#"idx="1""#, None, &pa("right (idx 1)")),
        ph(r#"idx="2""#, None, &pa("left (idx 2)")),
    ]
    .concat();
    let px = Px::new(vec![sl(&s)]).layout(0, &layout);
    assert_eq!(
        lines(&doc(&px).markdown),
        vec![
            "## Slide 1",
            "- left (idx 2)",
            "<!-- -->",
            "- right (idx 1)"
        ]
    );
}

#[test]
fn the_position_falls_back_to_the_masters_placeholder_of_that_type() {
    // The layout has the body placeholder (idx 1) but leaves its position to the master.
    let layout = ph(r#"type="body" idx="1""#, None, "");
    let s = [
        ph(r#"type="body" idx="1""#, None, &pa("master body")),
        tb(0, 0, I, I, &pa("above the master body")),
    ]
    .concat();
    // The master's body is at y = 2 inches: the text box (y = 0) is first.
    let px = Px::new(vec![sl(&s)]).layout(0, &layout);
    let m = doc(&px).markdown;
    assert_eq!(
        lines(&m),
        vec!["## Slide 1", "above the master body", "- master body"]
    );
    // And with the box below the master's body the order flips: the position was really used.
    let s = [
        ph(r#"type="body" idx="1""#, None, &pa("master body")),
        tb(0, 5 * I, I, I, &pa("below")),
    ]
    .concat();
    let px = Px::new(vec![sl(&s)]).layout(0, &layout);
    let m = doc(&px).markdown;
    assert_eq!(lines(&m), vec!["## Slide 1", "- master body", "below"]);
    // A placeholder with no `idx` finds the master's body by its type alone, whatever the layout.
    let s = [
        ph(r#"type="body""#, None, &pa("master body")),
        tb(0, 5 * I, I, I, &pa("below")),
    ]
    .concat();
    let m = doc(&Px::new(vec![sl(&s)])).markdown;
    assert_eq!(lines(&m), vec!["## Slide 1", "- master body", "below"]);
}

#[test]
fn a_layout_placeholder_wins_over_the_master_for_the_position() {
    let layout = ph(r#"type="body" idx="1""#, Some((0, 6 * I, I, I)), "");
    let s = [
        ph(r#"type="body" idx="1""#, None, &pa("laid out low")),
        tb(0, 4 * I, I, I, &pa("middle")),
    ]
    .concat();
    let px = Px::new(vec![sl(&s)]).layout(0, &layout);
    // The layout says y = 6 (the master 2): the text box (y = 4) is first.
    assert_eq!(
        lines(&doc(&px).markdown),
        vec!["## Slide 1", "middle", "- laid out low"]
    );
}

#[test]
fn different_slides_use_their_own_layouts() {
    let l0 = ph(r#"idx="1""#, Some((0, 0, I, I)), "");
    let l1 = ph(r#"idx="1""#, Some((0, 8 * I, I, I)), "");
    let s = |t: &str| {
        [
            ph(r#"idx="1""#, None, &pa(t)),
            tb(0, 4 * I, I, I, &pa("mid")),
        ]
        .concat()
    };
    let px = Px::new(vec![sl(&s("a")).layout(0), sl(&s("b")).layout(1)])
        .layout(0, &l0)
        .layout(1, &l1);
    let d = doc(&px);
    assert_eq!(
        lines(&d.markdown),
        vec!["## Slide 1", "- a", "mid", "## Slide 2", "mid", "- b"]
    );
}

#[test]
fn a_group_is_placed_at_its_bounding_box_and_read_inside() {
    // The group sits between "above" and "below"; inside, the file order is reversed.
    let g = group(
        (0, 2 * I, 4 * I, 2 * I),
        (0, 0, 4 * I, 2 * I),
        &[
            tb(0, I, 4 * I, I / 2, &pa("inner lower")),
            tb(0, 0, 4 * I, I / 2, &pa("inner upper")),
        ]
        .concat(),
    );
    let s = [
        tb(0, 6 * I, I, I, &pa("below")),
        g,
        tb(0, 0, I, I, &pa("above")),
    ]
    .concat();
    assert_eq!(
        lines(&md1(&s)),
        vec!["## Slide 1", "above", "inner upper", "inner lower", "below"]
    );
}

#[test]
fn group_scaling_moves_the_members() {
    // Child space 0..100 wide mapped to 8 inches: the member at child x = 90 is far right.
    let g = group(
        (0, 0, 8 * I, I),
        (0, 0, 100, 100),
        &[tb(90, 0, 10, 10, &pa("far")), tb(0, 0, 10, 10, &pa("near"))].concat(),
    );
    let s = [g, tb(0, 3 * I, I, I, &pa("lower"))].concat();
    assert_eq!(lines(&md1(&s)), vec!["## Slide 1", "near", "far", "lower"]);
}

#[test]
fn nested_groups_compose() {
    let inner = group(
        (0, 0, 4 * I, 4 * I),
        (0, 0, 4 * I, 4 * I),
        &[
            tb(0, 3 * I, I, I, &pa("deep low")),
            tb(0, 0, I, I, &pa("deep high")),
        ]
        .concat(),
    );
    let outer = group((0, 2 * I, 4 * I, 4 * I), (0, 0, 4 * I, 4 * I), &inner);
    let s = [tb(0, I, I, I, &pa("first")), outer].concat();
    // After the outer offset "deep high" is at y = 2, "deep low" at y = 5.
    let m = md1(&s);
    assert_eq!(
        lines(&m),
        vec!["## Slide 1", "first", "deep high", "deep low"],
        "{m}"
    );
}

#[test]
fn decoration_connectors_and_empty_shapes_are_not_shown() {
    let s = [
        connector(),
        sp_raw(12, "", &xf(0, 0, I, I), &pa("")),
        sp_raw(13, "", &xf(0, 0, I, I), ""),
        tb(0, 0, I, I, &pa("real")),
        ph(
            r#"type="dt" sz="half" idx="10""#,
            Some((0, 0, I, I)),
            &pa("2026-10-08"),
        ),
        ph(
            r#"type="ftr" sz="quarter" idx="11""#,
            Some((0, 0, I, I)),
            &pa("Footer text"),
        ),
        ph(
            r#"type="sldNum" sz="quarter" idx="12""#,
            Some((0, 0, I, I)),
            &pa("12"),
        ),
    ]
    .concat();
    assert_eq!(md1(&s), "## Slide 1\n\nreal");
}

#[test]
fn alternate_content_shapes_are_read_from_the_choice() {
    let s = format!(
        r#"<mc:AlternateContent><mc:Choice Requires="a14">{}</mc:Choice><mc:Fallback>{}</mc:Fallback></mc:AlternateContent>"#,
        tb(0, 0, I, I, &pa("choice")),
        tb(0, 0, I, I, &pa("fallback"))
    );
    assert_eq!(md1(&s), "## Slide 1\n\nchoice");
}

#[test]
fn rotated_shapes_are_placed_by_their_turned_box() {
    // A tall box rotated a quarter turn becomes wide: its top moves down to 2.5 inches.
    let rot =
        r#"<a:xfrm rot="5400000"><a:off x="0" y="0"/><a:ext cx="4572000" cy="9144000"/></a:xfrm>"#;
    let s = [
        sp_raw(14, "", rot, &pa("turned")),
        tb(0, 3 * I, I, I, &pa("other")),
    ]
    .concat();
    // Turned box: centre (2.5in, 5in) size 10x5 in -> top 2.5in; "other" top 3in -> second.
    assert_eq!(lines(&md1(&s)), vec!["## Slide 1", "turned", "other"]);
}

// ---------------------------------------------------------------------------------------------
// tables
// ---------------------------------------------------------------------------------------------

#[test]
fn a_table_is_a_gfm_table_in_its_place() {
    let s = [
        tb(0, 5 * I, I, I, &pa("after")),
        table((0, I, 8 * I, 2 * I), &(tr(&["A", "B"]) + &tr(&["1", "2"]))),
    ]
    .concat();
    assert_eq!(body1(&s), "| A | B |\n| --- | --- |\n| 1 | 2 |\n\nafter");
}

fn tc(attrs: &str, text: &str) -> String {
    let p = if text.is_empty() {
        "<a:p><a:endParaRPr/></a:p>".to_string()
    } else {
        pa(text)
    };
    format!(r#"<a:tc {attrs}><a:txBody><a:bodyPr/><a:lstStyle/>{p}</a:txBody></a:tc>"#)
}

#[test]
fn merged_cells_keep_the_value_top_left() {
    let row1 = format!(
        "<a:tr>{}{}{}</a:tr>",
        tc(r#"gridSpan="2""#, "wide"),
        tc(r#"hMerge="1""#, ""),
        tc("", "c")
    );
    let row2 = format!(
        "<a:tr>{}{}{}</a:tr>",
        tc(r#"rowSpan="2""#, "tall"),
        tc("", "x"),
        tc("", "y")
    );
    let row3 = format!(
        "<a:tr>{}{}{}</a:tr>",
        tc(r#"vMerge="1""#, ""),
        tc("", "z"),
        tc("", "w")
    );
    let m = body1(&table((0, 0, I, I), &[row1, row2, row3].concat()));
    assert_eq!(
        m,
        "| wide |   | c |\n| --- | --- | --- |\n| tall | x | y |\n|   | z | w |"
    );
}

#[test]
fn table_cells_hold_formatting_paragraphs_and_literal_markdown() {
    let cell = format!(
        r#"<a:tc><a:txBody><a:bodyPr/><a:lstStyle/>{}{}</a:txBody></a:tc>"#,
        paras(&ru(r#"b="1""#, "bold")),
        pa("a | b *c* `d`")
    );
    let m = body1(&table((0, 0, I, I), &format!("<a:tr>{cell}</a:tr>")));
    assert!(m.starts_with("| **bold**<br>a \\| b"), "{m}");
    assert!(!m.contains("*c*") && !m.contains("`d`"), "{m}");
    // One row: the header row and the separator, nothing else.
    assert_eq!(m.lines().count(), 2, "{m}");
}

#[test]
fn bullets_in_a_table_cell_are_flat_text() {
    let cell = format!(
        r#"<a:tc><a:txBody><a:bodyPr/><a:lstStyle/>{}</a:txBody></a:tc>"#,
        pp("", r#"<a:buChar char="x"/>"#, "item")
    );
    let m = body1(&table((0, 0, I, I), &format!("<a:tr>{cell}</a:tr>")));
    assert!(m.starts_with("| item |"), "{m}");
}

#[test]
fn a_table_over_the_cell_budget_is_cut_and_flagged() {
    let row = tr(&["a", "b", "c", "d"]);
    let s = table((0, 0, I, I), &row.repeat(10));
    let opts = DocOptions {
        max_table_cells: 12,
        ..DocOptions::default()
    };
    let d = load_px(&Px::new(vec![sl(&s)]), &opts).unwrap();
    assert!(d.truncated);
    assert_eq!(d.markdown.matches("| a | b | c | d |").count(), 3);
}

// ---------------------------------------------------------------------------------------------
// pictures, charts, SmartArt, math
// ---------------------------------------------------------------------------------------------

#[test]
fn a_picture_becomes_an_image_with_its_alt_text() {
    let s = pic("rId5", "A [graph] of $x$", Some((0, 0, I, I)));
    let px = Px::new(vec![sl(&s).rel("rId5", "image", "../media/g.png", false)])
        .media("g.png", tiny_png(2));
    let d = doc(&px);
    assert_eq!(d.images.len(), 1);
    assert!(
        d.markdown
            .contains(&format!("![A graph of x]({})", d.images[0].key)),
        "{}",
        d.markdown
    );
    assert_eq!(d.images[0].name, "g.png");
}

#[test]
fn the_same_picture_twice_is_one_image() {
    let s = [
        pic("rId5", "a", Some((0, 0, I, I))),
        pic("rId5", "b", Some((0, 2 * I, I, I))),
    ]
    .concat();
    let px = Px::new(vec![sl(&s).rel("rId5", "image", "../media/g.png", false)])
        .media("g.png", tiny_png(3));
    let d = doc(&px);
    assert_eq!(d.images.len(), 1);
    assert_eq!(d.markdown.matches("office-img://").count(), 2);
}

#[test]
fn pictures_that_cannot_be_shown_leave_a_placeholder() {
    let s = [
        pic("rIdMissing", "no relationship", Some((0, 0, I, I))),
        pic("rIdExt", "linked", Some((0, I, I, I))),
        pic("rIdEmf", "metafile", Some((0, 2 * I, I, I))),
        pic("rIdGone", "no part", Some((0, 3 * I, I, I))),
    ]
    .concat();
    let sld = sl(&s)
        .rel("rIdExt", "image", "https://tracker.example/p.png", true)
        .rel("rIdEmf", "image", "../media/m.emf", false)
        .rel("rIdGone", "image", "../media/none.png", false);
    let d = doc(&Px::new(vec![sld]).media("m.emf", vec![1, 2, 3]));
    assert!(d.images.is_empty());
    for alt in ["no relationship", "linked", "metafile", "no part"] {
        assert!(
            d.markdown.contains(&format!("\\[{alt}]")),
            "{alt}: {}",
            d.markdown
        );
    }
    assert!(!d.markdown.contains("tracker.example"));
}

#[test]
fn the_picture_budgets_apply() {
    let s: String = (0..5)
        .map(|i| pic(&format!("rId{i}"), "p", Some((0, i * I, I, I))))
        .collect();
    let mut sld = sl(&s);
    let mut px = Px::new(vec![]);
    for i in 0..5 {
        sld = sld.rel(
            &format!("rId{i}"),
            "image",
            &format!("../media/i{i}.png"),
            false,
        );
        px = px.media(&format!("i{i}.png"), tiny_png(10 + i as u8));
    }
    px.slides = vec![sld];
    let opts = DocOptions {
        max_images: 3,
        ..DocOptions::default()
    };
    let d = load_px(&px, &opts).unwrap();
    assert_eq!(d.images.len(), 3);
    assert!(d.truncated);
    let opts = DocOptions {
        max_image_bytes: 10,
        ..DocOptions::default()
    };
    let d = load_px(&px, &opts).unwrap();
    assert!(d.images.is_empty() && d.truncated);
}

pub(super) fn chart_part(title: Option<&str>) -> Vec<u8> {
    let t = title
        .map(|t| {
            format!(
                r#"<c:title><c:tx><c:rich><a:bodyPr/><a:p><a:r><a:t>{t}</a:t></a:r></a:p></c:rich></c:tx></c:title>"#
            )
        })
        .unwrap_or_default();
    format!(
        r#"<?xml version="1.0"?><c:chartSpace {PNS}><c:chart>{t}<c:plotArea><c:barChart><c:ser><c:tx><c:v>series name</c:v></c:tx></c:ser></c:barChart></c:plotArea></c:chart></c:chartSpace>"#
    )
    .into_bytes()
}

#[test]
fn a_chart_shows_its_title() {
    let s = chart_frame("rIdC1", (0, 0, 4 * I, 3 * I));
    let px = Px::new(vec![sl(&s).rel(
        "rIdC1",
        "chart",
        "../charts/chart1.xml",
        false,
    )])
    .part("ppt/charts/chart1.xml", &chart_part(Some("Sales *2026*")));
    assert_eq!(
        doc(&px).markdown,
        "## Slide 1\n\n\\[chart: Sales \\*2026\\*]"
    );
}

#[test]
fn a_chart_without_a_title_or_part_is_just_a_chart() {
    let s = [
        chart_frame("rIdC1", (0, 0, 4 * I, I)),
        chart_frame("rIdC2", (0, 2 * I, 4 * I, I)),
        chart_frame("rIdC3", (0, 4 * I, 4 * I, I)),
    ]
    .concat();
    let px = Px::new(vec![sl(&s)
        .rel("rIdC1", "chart", "../charts/chart1.xml", false)
        .rel("rIdC2", "chart", "../charts/none.xml", false)])
    .part("ppt/charts/chart1.xml", &chart_part(None));
    assert_eq!(
        doc(&px).markdown,
        "## Slide 1\n\n\\[chart]\n\n\\[chart]\n\n\\[chart]"
    );
}

pub(super) fn dm(texts: &[&str], ext: Option<&str>) -> Vec<u8> {
    let mut pts = String::from(
        r#"<dgm:pt modelId="0" type="doc"><dgm:t><a:bodyPr/><a:p><a:r><a:t>DOC</a:t></a:r></a:p></dgm:t></dgm:pt>"#,
    );
    for (i, t) in texts.iter().enumerate() {
        pts += &format!(
            r#"<dgm:pt modelId="{}"><dgm:prSet/><dgm:spPr/><dgm:t><a:bodyPr/><a:p><a:r><a:t>{t}</a:t></a:r></a:p></dgm:t></dgm:pt>"#,
            i + 1
        );
    }
    pts += r#"<dgm:pt modelId="99" type="pres"><dgm:t><a:bodyPr/><a:p><a:r><a:t>PRES</a:t></a:r></a:p></dgm:t></dgm:pt>"#;
    let e = ext
        .map(|r| {
            format!(
                r#"<dgm:extLst><a:ext uri="x"><dsp:dataModelExt relId="{r}" minVer="x"/></a:ext></dgm:extLst>"#
            )
        })
        .unwrap_or_default();
    format!(
        r#"<?xml version="1.0"?><dgm:dataModel {PNS}><dgm:ptLst>{pts}</dgm:ptLst>{e}</dgm:dataModel>"#
    )
    .into_bytes()
}

pub(super) fn drawing(shapes: &[(i64, i64, &str)]) -> Vec<u8> {
    let mut s = String::new();
    for (x, y, t) in shapes {
        s += &format!(
            r#"<dsp:sp modelId="1"><dsp:nvSpPr><dsp:cNvPr id="0" name=""/><dsp:cNvSpPr/></dsp:nvSpPr><dsp:spPr><a:xfrm><a:off x="{x}" y="{y}"/><a:ext cx="100000" cy="100000"/></a:xfrm></dsp:spPr><dsp:txBody><a:bodyPr/><a:p><a:r><a:t>{t}</a:t></a:r></a:p></dsp:txBody></dsp:sp>"#
        );
    }
    format!(r#"<?xml version="1.0"?><dsp:drawing {PNS}><dsp:spTree>{s}</dsp:spTree></dsp:drawing>"#)
        .into_bytes()
}

#[test]
fn smartart_shows_the_text_of_its_drawing_in_reading_order() {
    let s = smart_frame("rIdDm", (0, 0, 4 * I, 3 * I));
    let px = Px::new(vec![sl(&s)
        .rel("rIdDm", "diagramData", "../diagrams/data1.xml", false)
        .rel("rIdDr", "diagramDrawing", "../diagrams/drawing1.xml", false)])
    .part(
        "ppt/diagrams/data1.xml",
        &dm(&["data one", "data two"], Some("rIdDr")),
    )
    .part(
        "ppt/diagrams/drawing1.xml",
        &drawing(&[(500_000, 0, "second"), (0, 0, "first")]),
    );
    assert_eq!(doc(&px).markdown, "## Slide 1\n\n- first\n- second");
}

#[test]
fn smartart_without_a_drawing_shows_the_data_text() {
    let s = smart_frame("rIdDm", (0, 0, 4 * I, 3 * I));
    let px = Px::new(vec![sl(&s).rel(
        "rIdDm",
        "diagramData",
        "../diagrams/data1.xml",
        false,
    )])
    .part(
        "ppt/diagrams/data1.xml",
        &dm(&["data one", "data two"], None),
    );
    // (the document point and the presentation point are not content)
    assert_eq!(doc(&px).markdown, "## Slide 1\n\n- data one\n- data two");
}

#[test]
fn smartart_with_nothing_readable_is_a_placeholder() {
    let s = [
        smart_frame("rIdNone", (0, 0, I, I)),
        smart_frame("rIdEmpty", (0, 2 * I, I, I)),
    ]
    .concat();
    let px = Px::new(vec![sl(&s).rel(
        "rIdEmpty",
        "diagramData",
        "../diagrams/data1.xml",
        false,
    )])
    .part("ppt/diagrams/data1.xml", &dm(&[], None));
    assert_eq!(
        doc(&px).markdown,
        "## Slide 1\n\n\\[SmartArt]\n\n\\[SmartArt]"
    );
}

#[test]
fn smartart_text_is_literal() {
    let s = smart_frame("rIdDm", (0, 0, I, I));
    let px = Px::new(vec![sl(&s).rel(
        "rIdDm",
        "diagramData",
        "../diagrams/data1.xml",
        false,
    )])
    .part("ppt/diagrams/data1.xml", &dm(&["# x", "---"], None));
    let m = doc(&px).markdown;
    assert!(m.contains("- \\# x"), "{m}");
    check_headings(&doc(&px));
}

fn math_para(inner: &str) -> String {
    format!(
        r#"<a:p><a:r><a:t>E = </a:t></a:r><mc:AlternateContent><mc:Choice Requires="a14"><a14:m>{inner}</a14:m></mc:Choice><mc:Fallback><a:r><a:t>fallback text</a:t></a:r></mc:Fallback></mc:AlternateContent></a:p>"#
    )
}

#[test]
fn inline_math_becomes_latex() {
    let tx = math_para(r#"<m:oMath><m:r><m:t>mc</m:t></m:r></m:oMath>"#);
    let m = body1(&tb(0, 0, I, I, &tx));
    assert!(m.starts_with("E = $"), "{m}");
    assert!(m.ends_with('$'), "{m}");
    assert!(!m.contains("fallback"), "{m}");
}

#[test]
fn display_math_is_a_block_of_its_own() {
    let tx = math_para(r#"<m:oMathPara><m:oMath><m:r><m:t>x</m:t></m:r></m:oMath></m:oMathPara>"#);
    let m = body1(&tb(0, 0, I, I, &tx));
    assert!(m.contains("\n\n$$\n"), "{m}");
    check_headings(&doc(&Px::new(vec![sl(&tb(0, 0, I, I, &tx))])));
}

// ---------------------------------------------------------------------------------------------
// notes
// ---------------------------------------------------------------------------------------------

pub(super) fn notes_body(tx: &str) -> String {
    ph(r#"type="body" idx="1""#, Some((0, 0, I, I)), tx)
}

#[test]
fn notes_go_under_the_slide_in_a_quote() {
    let sld = sl(&[title("T"), tb(0, I, I, I, &pa("content"))].concat())
        .notes(&notes_body(&(pa("first note") + &pa("second note"))));
    let d = doc(&Px::new(vec![sld, sl(&title("U"))]));
    assert_eq!(
        d.markdown,
        "## Slide 1: T\n\ncontent\n\n> **Notes**  \n> first note  \n> second note\n\n## Slide 2: U"
    );
    check_headings(&d);
}

#[test]
fn the_notes_label_follows_the_language() {
    let sld = sl(&title("T")).notes(&notes_body(&pa("memo")));
    let opts = DocOptions {
        lang: Lang::Jp,
        ..DocOptions::default()
    };
    let d = load_px(&Px::new(vec![sld]), &opts).unwrap();
    assert!(d.markdown.contains("> **ノート**"), "{}", d.markdown);
}

#[test]
fn only_the_body_of_the_notes_page_is_shown() {
    let n = [
        ph(r#"type="sldImg""#, Some((0, 0, I, I)), &pa("IMG")),
        notes_body(&pa("the note")),
        ph(r#"type="sldNum" idx="5""#, Some((0, 0, I, I)), &pa("3")),
        ph(r#"type="hdr""#, Some((0, 0, I, I)), &pa("HEADER")),
        tb(0, 0, I, I, &pa("stray text box")),
    ]
    .concat();
    let d = doc(&Px::new(vec![sl(&title("T")).notes(&n)]));
    assert!(d.markdown.contains("> the note"));
    for bad in ["IMG", "HEADER", "stray", "> 3"] {
        assert!(!d.markdown.contains(bad), "{bad}: {}", d.markdown);
    }
}

#[test]
fn empty_notes_are_not_shown() {
    for n in [notes_body(""), notes_body(&pa("   ")), String::new()] {
        let d = doc(&Px::new(vec![sl(&title("T")).notes(&n)]));
        assert_eq!(d.markdown, "## Slide 1: T");
    }
}

#[test]
fn notes_text_is_literal_and_cannot_leave_the_quote() {
    let n = notes_body(&(pa("# h") + &pa("---") + &pa("```") + &pa("- x") + &pa("1. n")));
    let d = doc(&Px::new(vec![sl(&title("T")).notes(&n), sl(&title("U"))]));
    check_headings(&d);
    for l in d.markdown.lines().skip(2) {
        if l.starts_with("## ") || l.is_empty() {
            continue;
        }
        assert!(
            l.starts_with('>'),
            "{l:?} escaped the quote:\n{}",
            d.markdown
        );
    }
    assert!(d.markdown.contains("> \\# h"));
    assert!(d.markdown.contains("> \u{200B}\\---"));
}

#[test]
fn notes_with_links_formatting_and_breaks() {
    let tx = paras(&format!(
        "{}{}<a:br/>{}",
        ru(r#"b="1""#, "bold"),
        link_run("rIdL", " site"),
        ru("", "next line")
    ));
    let mut sld = sl(&title("T")).notes(&notes_body(&tx));
    sld.notes_rels = vec![(
        "rIdL".into(),
        "hyperlink".into(),
        "https://n.example/".into(),
        true,
    )];
    let d = doc(&Px::new(vec![sld]));
    assert!(
        d.markdown
            .contains("> **bold** [site](https://n.example/)  \n> next line"),
        "{}",
        d.markdown
    );
}

#[test]
fn notes_display_math_stays_in_the_quote() {
    let tx = math_para(r#"<m:oMathPara><m:oMath><m:r><m:t>x</m:t></m:r></m:oMath></m:oMathPara>"#);
    let d = doc(&Px::new(vec![sl(&title("T")).notes(&notes_body(&tx))]));
    for l in d.markdown.lines().skip(2) {
        assert!(l.is_empty() || l.starts_with('>'), "{l:?}\n{}", d.markdown);
    }
}

#[test]
fn comments_are_never_shown() {
    let px = Px::new(vec![sl(&title("T"))]).part(
        "ppt/comments/comment1.xml",
        br#"<p:cmLst xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"><p:cm><p:text>SECRET COMMENT</p:text></p:cm></p:cmLst>"#,
    );
    assert!(!doc(&px).markdown.contains("SECRET"));
}

// ---------------------------------------------------------------------------------------------
// rendering with konoma's Markdown renderer
// ---------------------------------------------------------------------------------------------

#[test]
fn the_renderer_shows_slides_as_headings_bullets_and_a_quote() {
    let sld = sl(&[
        title("Plan"),
        body((0, 2 * I, 4 * I, 2 * I), &(pa("first") + &pl(1, "nested"))),
    ]
    .concat())
    .notes(&notes_body(&pa("say hello")));
    let d = doc(&Px::new(vec![sld, sl(&title("Next")).hidden()]));
    let shown = rendered(&d.markdown, 80).join("\n");
    for want in [
        "Slide 1: Plan",
        "first",
        "nested",
        "Notes",
        "say hello",
        "Slide 2: Next (hidden)",
    ] {
        assert!(shown.contains(want), "{want:?} in\n{shown}");
    }
    assert!(!shown.contains("**"), "{shown}");
    assert!(!shown.contains("## "), "{shown}");
}

#[test]
fn the_renderer_shows_a_table_and_an_image_placeholder() {
    let s = [
        table((0, 0, I, I), &(tr(&["A", "B"]) + &tr(&["1", "2"]))),
        pic("rIdMissing", "chart pic", Some((0, 3 * I, I, I))),
    ]
    .concat();
    let d = doc(&Px::new(vec![sl(&s)]));
    let shown = rendered(&d.markdown, 80).join("\n");
    assert!(shown.contains('A') && shown.contains('2'), "{shown}");
    assert!(shown.contains("[chart pic]"), "{shown}");
}

// ---------------------------------------------------------------------------------------------
// a deck with everything
// ---------------------------------------------------------------------------------------------

#[test]
fn a_deck_with_hostile_text_keeps_its_headings_in_step_with_its_slides() {
    let mut slides = Vec::new();
    for i in 0..12usize {
        let s = match i % 4 {
            0 => title(&format!("T{i}")),
            1 => [
                title("x"),
                body((0, 2 * I, I, I), &(pa("# a") + &pa("## b"))),
            ]
            .concat(),
            2 => table((0, 0, I, I), &tr(&["## c", "d"])),
            _ => tb(0, 0, I, I, &pa("## e")),
        };
        let mut sld = sl(&s);
        if i % 3 == 0 {
            sld = sld.hidden();
        }
        if i % 5 == 0 {
            sld = sld.notes(&notes_body(&pa("## n")));
        }
        slides.push(sld);
    }
    let d = doc(&Px::new(slides));
    assert_eq!(d.slides.len(), 12);
    check_headings(&d);
}

#[test]
fn the_masters_text_styles_decide_the_bullets_of_a_body() {
    // A master whose body style numbers its first level and has no bullets below it.
    let styles = r#"<p:txStyles><p:bodyStyle><a:lvl1pPr><a:buAutoNum type="arabicPeriod"/></a:lvl1pPr></p:bodyStyle></p:txStyles>"#;
    let px = Px::new(vec![sl(&body(
        (0, 0, I, I),
        &(pa("a") + &pa("b") + &pl(1, "no bullet below")),
    ))])
    .master("", styles);
    let m = doc(&px).markdown;
    assert!(m.contains("1. a\n2. b"), "{m}");
    assert!(m.contains("no bullet below"), "{m}");
}

#[test]
fn a_master_placeholders_list_style_beats_the_text_styles() {
    let master = ph(r#"type="body" idx="1""#, Some((0, 0, I, I)), "").replace(
        "<a:lstStyle/>",
        r#"<a:lstStyle><a:lvl1pPr><a:buNone/></a:lvl1pPr></a:lstStyle>"#,
    );
    let px = Px::new(vec![sl(&body((0, 0, I, I), &pa("flat")))]).master(&master, BODY_BULLETS);
    assert_eq!(doc(&px).markdown, "## Slide 1\n\nflat");
}

#[test]
fn whitespace_around_a_link_stays_outside_it() {
    let tx = paras(&(ru("", "see") + &link_run("rIdA", " here ") + &ru("", "now")));
    let s = sl(&tb(0, 0, I, I, &tx)).rel("rIdA", "hyperlink", "https://e.example/", true);
    assert!(doc(&Px::new(vec![s]))
        .markdown
        .ends_with("see [here](https://e.example/) now"),);
}

#[test]
fn a_line_break_ends_a_link() {
    let tx = paras(&(link_run("rIdA", "one") + "<a:br/>" + &link_run("rIdA", "two")));
    let s = sl(&tb(0, 0, I, I, &tx)).rel("rIdA", "hyperlink", "https://e.example/", true);
    assert!(doc(&Px::new(vec![s]))
        .markdown
        .ends_with("[one](https://e.example/)  \n[two](https://e.example/)"),);
}
