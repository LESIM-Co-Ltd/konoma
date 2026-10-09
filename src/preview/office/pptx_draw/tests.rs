//! Tests of the pptx drawing model builder: small hand-written packages with a theme, a master, a
//! layout and slides, one inheritance step at a time, then the contract of the picture view, the
//! budgets, and (ignored) the dumps that put konoma's picture next to LibreOffice's.

use std::collections::HashMap;
use std::path::PathBuf;

use super::super::{load_presentation, DocOptions, Document};
use super::*;
use crate::i18n::Lang;
use crate::preview::office::slide_draw::{self as sd, Fill, Item, Rgba};
use crate::preview::office::tests::{deflated, tmp, write};
use crate::preview::office::tests_docx::{tiny_png, REL_BASE};
use crate::preview::office::tests_pptx::PNS;

mod dump;
mod dump_e3;
mod pic_fx;
mod shapes_e3;
mod tabs;

// ---------------------------------------------------------------------------------------------
// the package builder
// ---------------------------------------------------------------------------------------------

const HDR: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#;

/// A theme in the form PowerPoint writes it (Office theme, with Japanese script fonts).
pub(super) const THEME: &str = r#"<a:theme xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" name="T"><a:themeElements>
<a:clrScheme name="C"><a:dk1><a:sysClr val="windowText" lastClr="000000"/></a:dk1><a:lt1><a:sysClr val="window" lastClr="FFFFFF"/></a:lt1><a:dk2><a:srgbClr val="44546A"/></a:dk2><a:lt2><a:srgbClr val="E7E6E6"/></a:lt2><a:accent1><a:srgbClr val="4472C4"/></a:accent1><a:accent2><a:srgbClr val="ED7D31"/></a:accent2><a:accent3><a:srgbClr val="A5A5A5"/></a:accent3><a:accent4><a:srgbClr val="FFC000"/></a:accent4><a:accent5><a:srgbClr val="5B9BD5"/></a:accent5><a:accent6><a:srgbClr val="70AD47"/></a:accent6><a:hlink><a:srgbClr val="0563C1"/></a:hlink><a:folHlink><a:srgbClr val="954F72"/></a:folHlink></a:clrScheme>
<a:fontScheme name="F"><a:majorFont><a:latin typeface="Calibri Light"/><a:ea typeface=""/><a:cs typeface=""/><a:font script="Jpan" typeface="游ゴシック Light"/></a:majorFont><a:minorFont><a:latin typeface="Calibri"/><a:ea typeface=""/><a:cs typeface=""/><a:font script="Jpan" typeface="游ゴシック"/></a:minorFont></a:fontScheme>
<a:fmtScheme name="M"><a:fillStyleLst><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:gradFill rotWithShape="1"><a:gsLst><a:gs pos="0"><a:schemeClr val="phClr"><a:lumMod val="110000"/></a:schemeClr></a:gs><a:gs pos="100000"><a:schemeClr val="phClr"><a:lumMod val="90000"/></a:schemeClr></a:gs></a:gsLst><a:lin ang="5400000" scaled="0"/></a:gradFill><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:fillStyleLst>
<a:lnStyleLst><a:ln w="6350" cap="flat" cmpd="sng" algn="ctr"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:prstDash val="solid"/><a:miter lim="800000"/></a:ln><a:ln w="12700" cap="flat" cmpd="sng" algn="ctr"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:prstDash val="solid"/><a:miter lim="800000"/></a:ln><a:ln w="19050" cap="flat" cmpd="sng" algn="ctr"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:prstDash val="solid"/><a:miter lim="800000"/></a:ln></a:lnStyleLst>
<a:effectStyleLst><a:effectStyle><a:effectLst/></a:effectStyle><a:effectStyle><a:effectLst/></a:effectStyle><a:effectStyle><a:effectLst><a:outerShdw blurRad="57150" dist="19050" dir="5400000" algn="ctr" rotWithShape="0"><a:srgbClr val="000000"><a:alpha val="63000"/></a:srgbClr></a:outerShdw></a:effectLst></a:effectStyle></a:effectStyleLst>
<a:bgFillStyleLst><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:solidFill><a:schemeClr val="phClr"><a:tint val="95000"/></a:schemeClr></a:solidFill><a:gradFill rotWithShape="1"><a:gsLst><a:gs pos="0"><a:schemeClr val="phClr"><a:tint val="93000"/></a:schemeClr></a:gs><a:gs pos="100000"><a:schemeClr val="phClr"><a:shade val="94000"/></a:schemeClr></a:gs></a:gsLst><a:lin ang="5400000" scaled="0"/></a:gradFill></a:bgFillStyleLst></a:fmtScheme></a:themeElements></a:theme>"#;

const CLR_MAP: &str = r#"<p:clrMap bg1="lt1" tx1="dk1" bg2="lt2" tx2="dk2" accent1="accent1" accent2="accent2" accent3="accent3" accent4="accent4" accent5="accent5" accent6="accent6" hlink="hlink" folHlink="folHlink"/>"#;

type Row = (String, String, String);

/// One slide of the deck: the attributes of `p:sld`, the `p:bg`, the shapes, the `p:clrMapOvr`.
#[derive(Clone)]
pub(super) struct S {
    pub attrs: String,
    pub bg: String,
    pub shapes: String,
    pub ovr: String,
    pub rels: Vec<Row>,
    pub notes: Option<String>,
}

pub(super) fn slide(shapes: &str) -> S {
    S {
        attrs: String::new(),
        bg: String::new(),
        shapes: shapes.to_string(),
        ovr: "<p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr>".to_string(),
        rels: Vec::new(),
        notes: None,
    }
}

/// A deck: the parts that matter to the drawing, as the raw XML each one holds.
#[derive(Clone)]
pub(super) struct D {
    pub size: (i64, i64),
    /// Extra children of `p:presentation` (`p:defaultTextStyle` ..) and attributes.
    pub pres_extra: String,
    pub pres_attrs: String,
    pub theme: Option<String>,
    pub master_bg: String,
    pub master_shapes: String,
    pub master_map: String,
    pub master_tx: String,
    pub master_rels: Vec<Row>,
    pub layout_attrs: String,
    pub layout_bg: String,
    pub layout_shapes: String,
    pub layout_ovr: String,
    pub layout_rels: Vec<Row>,
    pub slides: Vec<S>,
    pub media: Vec<(String, Vec<u8>)>,
}

impl D {
    pub fn new(shapes: &str) -> D {
        D {
            size: (9_144_000, 6_858_000),
            pres_extra: String::new(),
            pres_attrs: String::new(),
            theme: Some(THEME.to_string()),
            master_bg: String::new(),
            master_shapes: String::new(),
            master_map: CLR_MAP.to_string(),
            master_tx: String::new(),
            master_rels: Vec::new(),
            layout_attrs: String::new(),
            layout_bg: String::new(),
            layout_shapes: String::new(),
            layout_ovr: "<p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr>".to_string(),
            layout_rels: Vec::new(),
            slides: vec![slide(shapes)],
            media: Vec::new(),
        }
    }

    fn rels_xml(rows: &[Row]) -> String {
        let mut s = format!(
            r#"{HDR}<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">"#
        );
        for (id, ty, target) in rows {
            let ext = if target.starts_with("http") {
                r#" TargetMode="External""#
            } else {
                ""
            };
            s += &format!(
                r#"<Relationship Id="{id}" Type="{REL_BASE}/{ty}" Target="{}"{ext}/>"#,
                target.replace('&', "&amp;")
            );
        }
        s + "</Relationships>"
    }

    pub fn entries(&self) -> Vec<(String, Vec<u8>)> {
        let tree = |inner: &str| {
            format!(
                r#"<p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/>{inner}</p:spTree>"#
            )
        };
        let mut out: Vec<(String, Vec<u8>)> = vec![
            (
                "[Content_Types].xml".into(),
                format!(r#"{HDR}<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"/>"#).into_bytes(),
            ),
            (
                "_rels/.rels".into(),
                Self::rels_xml(&[("rId1".into(), "officeDocument".into(), "ppt/presentation.xml".into())]).into_bytes(),
            ),
        ];
        let mut prels: Vec<Row> = vec![(
            "rIdM".into(),
            "slideMaster".into(),
            "slideMasters/slideMaster1.xml".into(),
        )];
        let mut ids = String::new();
        for i in 0..self.slides.len() {
            prels.push((
                format!("rIdS{i}"),
                "slide".into(),
                format!("slides/slide{}.xml", i + 1),
            ));
            ids += &format!(r#"<p:sldId id="{}" r:id="rIdS{i}"/>"#, 256 + i);
        }
        out.push((
            "ppt/presentation.xml".into(),
            format!(
                r#"{HDR}<p:presentation {PNS}{}><p:sldMasterIdLst><p:sldMasterId id="2147483648" r:id="rIdM"/></p:sldMasterIdLst><p:sldIdLst>{ids}</p:sldIdLst><p:sldSz cx="{}" cy="{}"/>{}</p:presentation>"#,
                self.pres_attrs, self.size.0, self.size.1, self.pres_extra
            )
            .into_bytes(),
        ));
        out.push((
            "ppt/_rels/presentation.xml.rels".into(),
            Self::rels_xml(&prels).into_bytes(),
        ));
        out.push((
            "ppt/slideMasters/slideMaster1.xml".into(),
            format!(
                r#"{HDR}<p:sldMaster {PNS}><p:cSld>{}{}</p:cSld>{}{}</p:sldMaster>"#,
                self.master_bg,
                tree(&self.master_shapes),
                self.master_map,
                self.master_tx
            )
            .into_bytes(),
        ));
        let mut mrels: Vec<Row> = vec![(
            "rIdL".into(),
            "slideLayout".into(),
            "../slideLayouts/slideLayout1.xml".into(),
        )];
        if let Some(t) = &self.theme {
            mrels.push(("rIdT".into(), "theme".into(), "../theme/theme1.xml".into()));
            out.push((
                "ppt/theme/theme1.xml".into(),
                format!("{HDR}{t}").into_bytes(),
            ));
        }
        mrels.extend(self.master_rels.iter().cloned());
        out.push((
            "ppt/slideMasters/_rels/slideMaster1.xml.rels".into(),
            Self::rels_xml(&mrels).into_bytes(),
        ));
        out.push((
            "ppt/slideLayouts/slideLayout1.xml".into(),
            format!(
                r#"{HDR}<p:sldLayout {PNS}{}><p:cSld>{}{}</p:cSld>{}</p:sldLayout>"#,
                self.layout_attrs,
                self.layout_bg,
                tree(&self.layout_shapes),
                self.layout_ovr
            )
            .into_bytes(),
        ));
        let mut lrels: Vec<Row> = vec![(
            "rId1".into(),
            "slideMaster".into(),
            "../slideMasters/slideMaster1.xml".into(),
        )];
        lrels.extend(self.layout_rels.iter().cloned());
        out.push((
            "ppt/slideLayouts/_rels/slideLayout1.xml.rels".into(),
            Self::rels_xml(&lrels).into_bytes(),
        ));
        for (i, s) in self.slides.iter().enumerate() {
            let n = i + 1;
            out.push((
                format!("ppt/slides/slide{n}.xml"),
                format!(
                    r#"{HDR}<p:sld {PNS}{}><p:cSld>{}{}</p:cSld>{}</p:sld>"#,
                    s.attrs,
                    s.bg,
                    tree(&s.shapes),
                    s.ovr
                )
                .into_bytes(),
            ));
            let mut rows: Vec<Row> = vec![(
                "rIdLay".into(),
                "slideLayout".into(),
                "../slideLayouts/slideLayout1.xml".into(),
            )];
            if s.notes.is_some() {
                rows.push((
                    "rIdN".into(),
                    "notesSlide".into(),
                    format!("../notesSlides/notesSlide{n}.xml"),
                ));
            }
            rows.extend(s.rels.iter().cloned());
            out.push((
                format!("ppt/slides/_rels/slide{n}.xml.rels"),
                Self::rels_xml(&rows).into_bytes(),
            ));
            if let Some(notes) = &s.notes {
                out.push((
                    format!("ppt/notesSlides/notesSlide{n}.xml"),
                    format!(
                        r#"{HDR}<p:notes {PNS}><p:cSld>{}</p:cSld></p:notes>"#,
                        tree(notes)
                    )
                    .into_bytes(),
                ));
            }
        }
        for (n, b) in &self.media {
            out.push((format!("ppt/media/{n}"), b.clone()));
        }
        out
    }

    pub fn load_with(&self, opts: &DocOptions) -> Document {
        let dir = tmp("pptxd");
        let e = self.entries();
        let refs: Vec<(&str, &[u8])> = e.iter().map(|(n, b)| (n.as_str(), b.as_slice())).collect();
        let p = write(&dir, "t.pptx", &deflated(&refs));
        load_presentation(&p, opts).unwrap()
    }

    pub fn load(&self) -> Document {
        self.load_with(&DocOptions::default())
    }

    /// The scene of the first slide.
    pub fn scene(&self) -> sd::SlideScene {
        let d = self.load();
        assert!(!d.slide_scenes.is_empty(), "no scene");
        d.slide_scenes[0].clone()
    }
}

// ---------------------------------------------------------------------------------------------
// shape builders
// ---------------------------------------------------------------------------------------------

pub(super) fn xf(x: i64, y: i64, w: i64, h: i64) -> String {
    format!(r#"<a:xfrm><a:off x="{x}" y="{y}"/><a:ext cx="{w}" cy="{h}"/></a:xfrm>"#)
}

pub(super) const RECT: &str = r#"<a:prstGeom prst="rect"><a:avLst/></a:prstGeom>"#;

/// A shape: `ph` (attributes of `p:ph`, empty for none), `sp_pr` (the inside of `p:spPr`),
/// `body_pr`/`lst` (the inside of `a:bodyPr` and of `a:lstStyle`, whole elements for bodyPr), the
/// paragraphs, and an optional `p:style`.
pub(super) fn shape(
    ph: &str,
    sp_pr: &str,
    body_pr: &str,
    lst: &str,
    paras: &str,
    style: &str,
) -> String {
    let ph = if ph.is_empty() {
        String::new()
    } else {
        format!("<p:ph {ph}/>")
    };
    let tx = if paras.is_empty() && body_pr.is_empty() && lst.is_empty() {
        String::new()
    } else {
        let bp = if body_pr.is_empty() {
            "<a:bodyPr/>"
        } else {
            body_pr
        };
        format!("<p:txBody>{bp}<a:lstStyle>{lst}</a:lstStyle>{paras}</p:txBody>")
    };
    format!(
        r#"<p:sp><p:nvSpPr><p:cNvPr id="2" name="S"/><p:cNvSpPr/><p:nvPr>{ph}</p:nvPr></p:nvSpPr><p:spPr>{sp_pr}</p:spPr>{style}{tx}</p:sp>"#
    )
}

/// A text box with a position.
pub(super) fn tbox(x: i64, y: i64, w: i64, h: i64, paras: &str) -> String {
    shape("", &format!("{}{RECT}", xf(x, y, w, h)), "", "", paras, "")
}

pub(super) fn para(text: &str) -> String {
    format!(r#"<a:p><a:r><a:rPr lang="en-US"/><a:t>{text}</a:t></a:r></a:p>"#)
}

pub(super) fn para_with(ppr: &str, rpr: &str, text: &str) -> String {
    format!(r#"<a:p>{ppr}<a:r>{rpr}<a:t>{text}</a:t></a:r></a:p>"#)
}

/// The shapes of a scene, flattened through groups.
pub(super) fn shapes_of(items: &[Item]) -> Vec<&sd::ShapeItem> {
    let mut out = Vec::new();
    for i in items {
        match i {
            Item::Shape(s) => out.push(s),
            Item::Group(g) => out.extend(shapes_of(&g.items)),
            Item::Picture(_) => {}
        }
    }
    out
}

pub(super) fn first_shape(sc: &sd::SlideScene) -> &sd::ShapeItem {
    match &sc.items[0] {
        Item::Shape(s) => s,
        other => panic!("not a shape: {other:?}"),
    }
}

pub(super) fn run0(s: &sd::ShapeItem) -> &sd::Run {
    &s.text.as_ref().expect("text").paragraphs[0].runs[0]
}

fn solid(r: &sd::Run) -> Rgba {
    match r.fill {
        Fill::Solid(c) => c,
        ref f => panic!("not solid: {f:?}"),
    }
}

const RED: Rgba = Rgba::rgb(255, 0, 0);

// ---------------------------------------------------------------------------------------------
// colours
// ---------------------------------------------------------------------------------------------

fn elem(xml: &str) -> Node {
    use crate::preview::office::docx_xml::{read_element, Budget, Tree};
    use crate::preview::office::fmt_xlsx::XmlReader;
    use quick_xml::events::Event;
    let mut rd = XmlReader::new(xml.as_bytes());
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match rd.read_event_into(&mut buf).unwrap() {
            Event::Start(e) => {
                let e = e.into_owned();
                let mut b = Budget::new(10_000, 1 << 20);
                match read_element(&mut rd, &e, false, &mut b).unwrap() {
                    Tree::Ok(n) => return n,
                    Tree::TooBig => panic!("too big"),
                }
            }
            Event::Empty(e) => {
                let e = e.into_owned();
                let mut b = Budget::new(10_000, 1 << 20);
                match read_element(&mut rd, &e, true, &mut b).unwrap() {
                    Tree::Ok(n) => return n,
                    Tree::TooBig => panic!("too big"),
                }
            }
            Event::Eof => panic!("no element"),
            _ => {}
        }
    }
}

fn colors<'a>(theme: &'a Theme, map: &'a ClrMap) -> theme::Colors<'a> {
    theme::Colors { theme, map }
}

fn clr(inner: &str) -> Option<Rgba> {
    let theme = Theme::parse(
        format!("{HDR}{THEME}").as_bytes(),
        &DocOptions::default(),
        HashMap::new(),
    );
    let map = ClrMap::default();
    let n = elem(&format!(
        r#"<a:solidFill xmlns:a="x">{}</a:solidFill>"#,
        inner
    ));
    colors(&theme, &map).first_in(&n, Some(Rgba::rgb(1, 2, 3)))
}

#[test]
fn every_colour_form_resolves() {
    assert_eq!(
        clr(r#"<a:srgbClr val="FF8000"/>"#),
        Some(Rgba::rgb(255, 128, 0))
    );
    assert_eq!(
        clr(r#"<a:sysClr val="window" lastClr="123456"/>"#),
        Some(Rgba::rgb(0x12, 0x34, 0x56))
    );
    // (No lastClr: the Windows default of the name.)
    assert_eq!(
        clr(r#"<a:sysClr val="windowText"/>"#),
        Some(Rgba::rgb(0, 0, 0))
    );
    assert_eq!(clr(r#"<a:prstClr val="red"/>"#), Some(RED));
    assert_eq!(
        clr(r#"<a:prstClr val="dkBlue"/>"#),
        Some(Rgba::rgb(0, 0, 139))
    );
    assert_eq!(clr(r#"<a:scrgbClr r="100000" g="0" b="0"/>"#), Some(RED));
    assert_eq!(
        clr(r#"<a:hslClr hue="0" sat="100000" lum="50000"/>"#),
        Some(RED)
    );
    assert_eq!(
        clr(r#"<a:hslClr hue="10800000" sat="100000" lum="50000"/>"#),
        Some(Rgba::rgb(0, 255, 255))
    );
    // Scheme colours go through the colour map: bg1 -> lt1, tx1 -> dk1.
    assert_eq!(clr(r#"<a:schemeClr val="bg1"/>"#), Some(Rgba::WHITE));
    assert_eq!(clr(r#"<a:schemeClr val="tx1"/>"#), Some(Rgba::BLACK));
    assert_eq!(
        clr(r#"<a:schemeClr val="accent2"/>"#),
        Some(Rgba::rgb(0xED, 0x7D, 0x31))
    );
    assert_eq!(
        clr(r#"<a:schemeClr val="dk2"/>"#),
        Some(Rgba::rgb(0x44, 0x54, 0x6A))
    );
    // phClr is the colour the style supplies.
    assert_eq!(
        clr(r#"<a:schemeClr val="phClr"/>"#),
        Some(Rgba::rgb(1, 2, 3))
    );
    // Transforms apply in order; alpha is carried.
    let c = clr(r#"<a:srgbClr val="FF0000"><a:alpha val="50000"/></a:srgbClr>"#).unwrap();
    assert_eq!((c.r, c.a), (255, 0.5));
    let c = clr(r#"<a:srgbClr val="808080"><a:lumMod val="50000"/></a:srgbClr>"#).unwrap();
    assert!(c.r < 0x50, "{c:?}");
    assert_eq!(clr(r#"<a:schemeClr val="nonsense"/>"#), None);
    assert_eq!(clr(r#"<a:srgbClr val="zz"/>"#), None);
}

#[test]
fn a_colour_map_can_swap_text_and_background() {
    let theme = Theme::default();
    let mut m = ClrMap::default();
    let ovr = elem(
        r#"<p:clrMapOvr xmlns:p="x" xmlns:a="y"><a:overrideClrMapping bg1="dk1" tx1="lt1" bg2="dk2" tx2="lt2" accent1="accent1"/></p:clrMapOvr>"#,
    );
    m = ClrMap::with_override(&m, Some(&ovr));
    let c = colors(&theme, &m);
    assert_eq!(c.scheme("bg1", None), Some(Rgba::BLACK));
    assert_eq!(c.scheme("tx1", None), Some(Rgba::WHITE));
    // `a:masterClrMapping` keeps the map.
    let keep = elem(r#"<p:clrMapOvr xmlns:p="x" xmlns:a="y"><a:masterClrMapping/></p:clrMapOvr>"#);
    let base = ClrMap::default();
    assert_eq!(ClrMap::with_override(&base, Some(&keep)), base);
    assert_eq!(ClrMap::with_override(&base, None), base);
}

#[test]
fn the_theme_is_read_and_a_missing_one_gives_the_office_defaults() {
    let t = Theme::parse(
        format!("{HDR}{THEME}").as_bytes(),
        &DocOptions::default(),
        HashMap::new(),
    );
    assert_eq!(t.color("accent1"), Some(Rgba::rgb(0x44, 0x72, 0xC4)));
    assert_eq!(t.color("dk1"), Some(Rgba::BLACK));
    assert_eq!(t.major.latin, "Calibri Light");
    assert_eq!(t.minor.latin, "Calibri");
    assert_eq!(
        t.minor.scripts,
        vec![("Jpan".to_string(), "游ゴシック".to_string())]
    );
    assert_eq!(
        (
            t.fills.len(),
            t.lines.len(),
            t.effects.len(),
            t.bg_fills.len()
        ),
        (3, 3, 3, 3)
    );
    let d = Theme::parse(b"not xml at all", &DocOptions::default(), HashMap::new());
    assert_eq!(d.color("accent1"), Some(Rgba::rgb(0x4F, 0x81, 0xBD)));
    // A deck with no theme part still has a scene, with the defaults.
    let mut deck = D::new(&tbox(0, 0, 1000, 1000, &para("x")));
    deck.theme = None;
    let sc = deck.scene();
    assert_eq!(solid(run0(first_shape(&sc))), Rgba::BLACK);
}

// ---------------------------------------------------------------------------------------------
// text: the inheritance chain
// ---------------------------------------------------------------------------------------------

const TITLE_STYLE: &str = r#"<p:txStyles><p:titleStyle><a:lvl1pPr algn="ctr"><a:lnSpc><a:spcPct val="90000"/></a:lnSpc><a:defRPr sz="4400" b="1"><a:solidFill><a:srgbClr val="FF0000"/></a:solidFill><a:latin typeface="+mj-lt"/><a:ea typeface="+mj-ea"/></a:defRPr></a:lvl1pPr></p:titleStyle><p:bodyStyle><a:lvl1pPr marL="228600" indent="-228600"><a:spcBef><a:spcPts val="1000"/></a:spcBef><a:buFont typeface="Arial"/><a:buChar char="&#8226;"/><a:defRPr sz="2800"><a:solidFill><a:schemeClr val="tx1"/></a:solidFill><a:latin typeface="+mn-lt"/></a:defRPr></a:lvl1pPr><a:lvl2pPr marL="685800" indent="-228600"><a:buChar char="-"/><a:defRPr sz="2400"/></a:lvl2pPr></p:bodyStyle><p:otherStyle><a:lvl1pPr><a:defRPr sz="1800"><a:solidFill><a:schemeClr val="tx1"/></a:solidFill></a:defRPr></a:lvl1pPr></p:otherStyle></p:txStyles>"#;

fn titled() -> D {
    let mut d = D::new("");
    d.master_tx = TITLE_STYLE.to_string();
    d.master_shapes = shape(
        r#"type="title""#,
        &format!("{}{RECT}", xf(100, 200, 8000, 900)),
        r#"<a:bodyPr lIns="0" tIns="0" rIns="0" bIns="0" anchor="ctr"><a:normAutofit/></a:bodyPr>"#,
        "",
        &para("click"),
        "",
    ) + &shape(
        r#"type="body" idx="1""#,
        &format!("{}{RECT}", xf(100, 1500, 8000, 4000)),
        "",
        "",
        &para("click"),
        "",
    );
    d
}

#[test]
fn a_title_takes_colour_size_font_and_alignment_from_the_master_text_style() {
    let mut d = titled();
    d.slides = vec![slide(&shape(
        "type=\"title\"",
        "",
        "",
        "",
        &para("Hello"),
        "",
    ))];
    let sc = d.scene();
    let s = first_shape(&sc);
    let r = run0(s);
    assert_eq!(solid(r), RED);
    assert_eq!(r.size_pt, 44.0);
    assert!(r.bold);
    // +mj-lt is the theme's major font.
    assert_eq!(r.font.latin.as_deref(), Some("Calibri Light"));
    let p = &s.text.as_ref().unwrap().paragraphs[0];
    assert_eq!(p.align, sd::Align::Center);
    assert_eq!(p.line_spacing, sd::Spacing::Pct(0.9));
    // The position and the body properties come from the master's placeholder.
    assert_eq!(
        (s.xfrm.x, s.xfrm.y, s.xfrm.w, s.xfrm.h),
        (100.0, 200.0, 8000.0, 900.0)
    );
    let t = s.text.as_ref().unwrap();
    assert_eq!(t.insets, (0.0, 0.0, 0.0, 0.0));
    assert_eq!(t.anchor, sd::Anchor::Middle);
    assert!(matches!(t.autofit, sd::AutoFit::Normal { .. }));
}

#[test]
fn the_position_of_a_placeholder_comes_from_the_layout_before_the_master() {
    let mut d = titled();
    d.layout_shapes = shape(
        r#"type="title""#,
        &xf(500, 600, 7000, 800),
        "",
        "",
        &para("layout"),
        "",
    );
    d.slides = vec![slide(&shape(
        "type=\"title\"",
        "",
        "",
        "",
        &para("Hello"),
        "",
    ))];
    let sc = d.scene();
    let s = first_shape(&sc);
    assert_eq!(
        (s.xfrm.x, s.xfrm.y, s.xfrm.w, s.xfrm.h),
        (500.0, 600.0, 7000.0, 800.0)
    );
    // The layout gave no geometry or bodyPr: the master's placeholder still does.
    assert_eq!(s.text.as_ref().unwrap().anchor, sd::Anchor::Middle);
}

#[test]
fn a_slide_placeholder_with_its_own_position_keeps_it() {
    let mut d = titled();
    d.slides = vec![slide(&shape(
        r#"type="title""#,
        &xf(7, 8, 9, 10),
        "",
        "",
        &para("Hello"),
        "",
    ))];
    let sc = d.scene();
    let s = first_shape(&sc);
    assert_eq!(
        (s.xfrm.x, s.xfrm.y, s.xfrm.w, s.xfrm.h),
        (7.0, 8.0, 9.0, 10.0)
    );
}

#[test]
fn level_two_of_a_body_takes_its_size_from_the_layout_list_style() {
    let mut d = titled();
    d.layout_shapes = shape(
        r#"type="body" idx="1""#,
        "",
        "",
        r#"<a:lvl2pPr><a:defRPr sz="2000"/></a:lvl2pPr>"#,
        &para("layout"),
        "",
    );
    d.slides = vec![slide(&shape(
        r#"idx="1""#,
        "",
        "",
        "",
        &(para("top")
            + r#"<a:p><a:pPr lvl="1"/><a:r><a:rPr lang="en-US"/><a:t>second</a:t></a:r></a:p>"#),
        "",
    ))];
    let sc = d.scene();
    let s = first_shape(&sc);
    let ps = &s.text.as_ref().unwrap().paragraphs;
    // Level 1: the master's body style.
    assert_eq!(ps[0].runs[0].size_pt, 28.0);
    assert!(
        matches!(&ps[0].bullet, Some(sd::Bullet { kind: sd::BulletKind::Char(c), .. }) if c == "\u{2022}")
    );
    assert_eq!(ps[0].mar_l, 228_600.0);
    assert_eq!(ps[0].indent, -228_600.0);
    assert_eq!(ps[0].spc_before, sd::Spacing::Pts(10.0));
    // Level 2: the layout's size (it beats the master's 24), the master's margin and bullet.
    assert_eq!(ps[1].runs[0].size_pt, 20.0);
    assert_eq!(ps[1].level, 1);
    assert_eq!(ps[1].mar_l, 685_800.0);
    assert!(
        matches!(&ps[1].bullet, Some(sd::Bullet { kind: sd::BulletKind::Char(c), .. }) if c == "-")
    );
}

#[test]
fn a_run_override_beats_every_list_style() {
    let mut d = titled();
    d.slides = vec![slide(&shape(
        r#"type="title""#,
        "",
        "",
        r#"<a:lvl1pPr><a:defRPr sz="3000"/></a:lvl1pPr>"#,
        &para_with(
            "",
            r#"<a:rPr lang="en-US" sz="1200" b="0" i="1" u="sng" strike="sngStrike" baseline="30000" spc="300" cap="all"><a:solidFill><a:srgbClr val="00FF00"/></a:solidFill><a:highlight><a:srgbClr val="FFFF00"/></a:highlight><a:latin typeface="Arial"/><a:ea typeface="Meiryo"/><a:cs typeface="Tahoma"/><a:sym typeface="Wingdings"/></a:rPr>"#,
            "Hi",
        ),
        "",
    ))];
    let sc = d.scene();
    let r = run0(first_shape(&sc)).clone();
    assert_eq!(r.size_pt, 12.0);
    assert!(!r.bold && r.italic);
    assert_eq!(r.underline, sd::Underline::Single);
    assert_eq!(r.strike, sd::Strike::Single);
    assert_eq!(r.baseline_pct, 30.0);
    assert_eq!(r.spacing_pt, 3.0);
    assert_eq!(r.caps, sd::Caps::All);
    assert_eq!(solid(&r), Rgba::rgb(0, 255, 0));
    assert_eq!(r.highlight, Some(Rgba::rgb(255, 255, 0)));
    assert_eq!(r.font.latin.as_deref(), Some("Arial"));
    assert_eq!(r.font.east_asian.as_deref(), Some("Meiryo"));
    assert_eq!(r.font.complex.as_deref(), Some("Tahoma"));
    assert_eq!(r.font.symbol.as_deref(), Some("Wingdings"));
    assert_eq!(r.lang.as_deref(), Some("en-US"));
}

#[test]
fn the_list_style_of_the_shape_beats_the_master_but_not_the_run() {
    let mut d = titled();
    d.slides = vec![slide(&shape(
        r#"type="title""#,
        "",
        "",
        r#"<a:lvl1pPr algn="r"><a:defRPr sz="3000"><a:solidFill><a:srgbClr val="0000FF"/></a:solidFill></a:defRPr></a:lvl1pPr>"#,
        &para("Hi"),
        "",
    ))];
    let sc = d.scene();
    let s = first_shape(&sc);
    assert_eq!(run0(s).size_pt, 30.0);
    assert_eq!(solid(run0(s)), Rgba::rgb(0, 0, 255));
    assert_eq!(
        s.text.as_ref().unwrap().paragraphs[0].align,
        sd::Align::Right
    );
    // (Bold still comes from the master: nothing nearer says.)
    assert!(run0(s).bold);
}

#[test]
fn theme_font_references_are_resolved_and_the_east_asian_font_follows_the_language() {
    let mut d = titled();
    // +mj-ea: the theme's major ea font is empty, so the Japanese script font is used for ja text.
    d.slides = vec![slide(&shape(
        r#"type="title""#,
        "",
        "",
        "",
        &para_with("", r#"<a:rPr lang="ja-JP"/>"#, "日本語"),
        "",
    ))];
    let sc = d.scene();
    let r = run0(first_shape(&sc)).clone();
    assert_eq!(r.font.latin.as_deref(), Some("Calibri Light"));
    assert_eq!(r.font.east_asian.as_deref(), Some("游ゴシック"));
    // An explicit +mn-ea with a theme that names one.
    let mut t = THEME.to_string();
    t = t.replace(
        r#"<a:minorFont><a:latin typeface="Calibri"/><a:ea typeface=""/>"#,
        r#"<a:minorFont><a:latin typeface="Calibri"/><a:ea typeface="MS Gothic"/>"#,
    );
    let mut d2 = D::new(&tbox(
        0,
        0,
        1000,
        1000,
        &para_with(
            "",
            r#"<a:rPr lang="en-US"><a:ea typeface="+mn-ea"/><a:latin typeface="+mn-lt"/></a:rPr>"#,
            "x",
        ),
    ));
    d2.theme = Some(t);
    let r = run0(first_shape(&d2.scene())).clone();
    assert_eq!(r.font.east_asian.as_deref(), Some("MS Gothic"));
    assert_eq!(r.font.latin.as_deref(), Some("Calibri"));
}

#[test]
fn a_plain_shape_takes_the_other_style_then_the_default_text_style() {
    let mut d = titled();
    d.slides = vec![slide(&tbox(0, 0, 1000, 1000, &para("plain")))];
    let sc = d.scene();
    assert_eq!(run0(first_shape(&sc)).size_pt, 18.0);
    // No master style at all: presentation.xml's default text style.
    let mut d = D::new(&tbox(0, 0, 1000, 1000, &para("plain")));
    d.pres_extra = r#"<p:defaultTextStyle><a:lvl1pPr marL="0" algn="l"><a:defRPr sz="2100"><a:solidFill><a:srgbClr val="123456"/></a:solidFill></a:defRPr></a:lvl1pPr></p:defaultTextStyle>"#.into();
    let sc = d.scene();
    let r = run0(first_shape(&sc));
    assert_eq!(r.size_pt, 21.0);
    assert_eq!(solid(r), Rgba::rgb(0x12, 0x34, 0x56));
    // Nothing anywhere: 18 pt in the text colour.
    let d = D::new(&tbox(0, 0, 1000, 1000, &para("plain")));
    let sc = d.scene();
    assert_eq!(run0(first_shape(&sc)).size_pt, 18.0);
    assert_eq!(solid(run0(first_shape(&sc))), Rgba::BLACK);
}

#[test]
fn the_font_reference_of_a_shape_style_colours_its_text() {
    let mut d = titled();
    let style = r#"<p:style><a:lnRef idx="0"><a:schemeClr val="accent1"/></a:lnRef><a:fillRef idx="1"><a:schemeClr val="accent1"/></a:fillRef><a:effectRef idx="0"><a:schemeClr val="accent1"/></a:effectRef><a:fontRef idx="minor"><a:schemeClr val="lt1"/></a:fontRef></p:style>"#;
    d.slides = vec![slide(&shape(
        "",
        &format!("{}{RECT}", xf(0, 0, 1000, 1000)),
        "",
        "",
        &para("white"),
        style,
    ))];
    let sc = d.scene();
    let s = first_shape(&sc);
    // The style's font reference beats the master's other style (tx1).
    assert_eq!(solid(run0(s)), Rgba::WHITE);
    // fillRef idx 1 is the first fill of the theme with phClr = accent1.
    assert_eq!(s.fill, Fill::Solid(Rgba::rgb(0x44, 0x72, 0xC4)));
    // lnRef idx 0 is no line.
    assert!(s.line.is_none());
}

#[test]
fn a_run_in_a_hyperlink_gets_the_link_colour_and_an_underline() {
    let mut d = D::new("");
    let link = r#"<a:rPr lang="en-US"><a:hlinkClick r:id="rIdH"/></a:rPr>"#;
    let own = r#"<a:rPr lang="en-US"><a:solidFill><a:srgbClr val="00FF00"/></a:solidFill><a:hlinkClick r:id="rIdH"/></a:rPr>"#;
    d.slides = vec![slide(&tbox(
        0,
        0,
        1000,
        1000,
        &(para_with("", link, "link") + &para_with("", own, "own") + &para("plain")),
    ))];
    d.slides[0].rels.push((
        "rIdH".into(),
        "hyperlink".into(),
        "https://example.com/".into(),
    ));
    let sc = d.scene();
    let ps = &first_shape(&sc).text.as_ref().unwrap().paragraphs;
    assert_eq!(solid(&ps[0].runs[0]), Rgba::rgb(0x05, 0x63, 0xC1));
    assert_eq!(ps[0].runs[0].underline, sd::Underline::Single);
    // A colour of the run's own wins (it is still underlined).
    assert_eq!(solid(&ps[1].runs[0]), Rgba::rgb(0, 255, 0));
    assert_eq!(ps[1].runs[0].underline, sd::Underline::Single);
    assert_eq!(ps[2].runs[0].underline, sd::Underline::None);
}

#[test]
fn the_slide_number_field_shows_the_number_of_its_slide() {
    let field = r#"<a:p><a:fld id="{1}" type="slidenum"><a:rPr lang="en-US"/><a:t>&#8249;#&#8250;</a:t></a:fld></a:p>"#;
    let date = r#"<a:p><a:fld id="{2}" type="datetime1"><a:rPr lang="en-US"/><a:t>10/9/2026</a:t></a:fld></a:p>"#;
    let mut d = D::new("");
    d.pres_attrs = r#" firstSlideNum="5""#.into();
    d.slides = vec![
        slide(&tbox(0, 0, 1000, 1000, &(field.to_string() + date))),
        slide(&tbox(0, 0, 1000, 1000, field)),
    ];
    let doc = d.load();
    let r = |i: usize, p: usize| -> sd::Run { run0_of(&doc.slide_scenes[i], p) };
    assert_eq!(
        (r(0, 0).text.as_str(), r(0, 0).kind),
        ("5", sd::RunKind::Field)
    );
    assert_eq!(r(1, 0).text, "6");
    // Another field shows the text stored in the file.
    assert_eq!(
        (r(0, 1).text.as_str(), r(0, 1).kind),
        ("10/9/2026", sd::RunKind::Field)
    );
}

fn run0_of(sc: &sd::SlideScene, para: usize) -> sd::Run {
    first_shape(sc).text.as_ref().unwrap().paragraphs[para].runs[0].clone()
}

#[test]
fn a_slide_number_placeholder_of_the_slide_is_drawn_but_the_masters_is_not() {
    let mut d = titled();
    d.master_shapes += &shape(
        r#"type="sldNum" sz="quarter" idx="4""#,
        &xf(7000, 6400, 1000, 300),
        "",
        "",
        r#"<a:p><a:fld id="{1}" type="slidenum"><a:rPr lang="en-US"/><a:t>&#8249;#&#8250;</a:t></a:fld></a:p>"#,
        "",
    );
    // (The layout has the placeholder too, without a position of its own.)
    d.layout_shapes += &shape(
        r#"type="sldNum" sz="quarter" idx="4""#,
        "",
        "",
        "",
        r#"<a:p><a:fld id="{1}" type="slidenum"><a:rPr lang="en-US"/><a:t>&#8249;#&#8250;</a:t></a:fld></a:p>"#,
        "",
    );
    d.slides = vec![slide("")];
    assert!(d.scene().items.is_empty());
    d.slides = vec![slide(&shape(
        r#"type="sldNum" sz="quarter" idx="4""#,
        "",
        "",
        "",
        r#"<a:p><a:fld id="{1}" type="slidenum"><a:rPr lang="en-US"/><a:t>1</a:t></a:fld></a:p>"#,
        "",
    ))];
    let sc = d.scene();
    let s = first_shape(&sc);
    assert_eq!(run0(s).text, "1");
    assert_eq!((s.xfrm.x, s.xfrm.y), (7000.0, 6400.0));
}

#[test]
fn paragraph_properties_and_bullets() {
    let mut d = D::new("");
    let ppr = |attrs: &str, inner: &str| format!("<a:pPr {attrs}>{inner}</a:pPr>");
    let paras = [
        para_with(&ppr(r#"algn="just" marL="457200" indent="-171450" rtl="1""#, r#"<a:lnSpc><a:spcPts val="2400"/></a:lnSpc><a:spcBef><a:spcPct val="20000"/></a:spcBef><a:spcAft><a:spcPts val="600"/></a:spcAft><a:buClr><a:srgbClr val="FF0000"/></a:buClr><a:buSzPct val="80000"/><a:buFont typeface="Wingdings"/><a:buChar char="&#167;"/>"#), "", "a"),
        para_with(&ppr("", r#"<a:buSzPts val="1400"/><a:buAutoNum type="romanLcPeriod" startAt="3"/>"#), "", "b"),
        para_with(&ppr("", "<a:buNone/>"), "", "c"),
        para_with(&ppr("", r#"<a:buBlip><a:blip r:embed="rIdB"/></a:buBlip>"#), "", "d"),
    ]
    .concat();
    d.slides = vec![slide(&tbox(0, 0, 1000, 1000, &paras))];
    d.slides[0]
        .rels
        .push(("rIdB".into(), "image".into(), "../media/b.png".into()));
    d.media.push(("b.png".into(), tiny_png(1)));
    let doc = d.load();
    let ps = &first_shape(&doc.slide_scenes[0])
        .text
        .as_ref()
        .unwrap()
        .paragraphs;
    let p = &ps[0];
    assert_eq!(p.align, sd::Align::Justify);
    assert!(p.rtl);
    assert_eq!((p.mar_l, p.indent), (457_200.0, -171_450.0));
    assert_eq!(p.line_spacing, sd::Spacing::Pts(24.0));
    assert_eq!(p.spc_before, sd::Spacing::Pct(0.2));
    assert_eq!(p.spc_after, sd::Spacing::Pts(6.0));
    let b = p.bullet.as_ref().unwrap();
    assert_eq!(b.kind, sd::BulletKind::Char("\u{a7}".into()));
    assert_eq!(b.color, Some(RED));
    assert_eq!(b.size, sd::BulletSize::Pct(0.8));
    assert_eq!(b.font.as_ref().unwrap().latin.as_deref(), Some("Wingdings"));
    let b = ps[1].bullet.as_ref().unwrap();
    assert_eq!(
        b.kind,
        sd::BulletKind::AutoNum {
            scheme: "romanLcPeriod".into(),
            start: 3
        }
    );
    assert_eq!(b.size, sd::BulletSize::Pts(14.0));
    assert!(ps[2].bullet.is_none());
    match &ps[3].bullet.as_ref().unwrap().kind {
        sd::BulletKind::Picture(img) => assert!(doc.images.iter().any(|i| i.key == img.key)),
        k => panic!("{k:?}"),
    }
}

#[test]
fn body_properties() {
    let body = r#"<a:bodyPr lIns="10" tIns="20" rIns="30" bIns="40" anchor="b" anchorCtr="1" wrap="none" vert="vert270" rot="5400000" upright="1" numCol="3"><a:normAutofit fontScale="92500" lnSpcReduction="10000"/></a:bodyPr>"#;
    let d = D::new(&shape(
        "",
        &format!("{}{RECT}", xf(0, 0, 1000, 1000)),
        body,
        "",
        &para("x"),
        "",
    ));
    let sc = d.scene();
    let t = first_shape(&sc).text.clone().unwrap();
    assert_eq!(t.insets, (10.0, 20.0, 30.0, 40.0));
    assert_eq!(t.anchor, sd::Anchor::Bottom);
    assert!(t.anchor_ctr && !t.wrap && t.upright);
    assert_eq!(t.vert, sd::Vert::Vert270);
    assert_eq!(t.rot_deg, 90.0);
    assert_eq!(t.columns, 3);
    assert_eq!(
        t.autofit,
        sd::AutoFit::Normal {
            font_scale: 0.925,
            ln_spc_reduction: 0.1
        }
    );
    // Defaults.
    let d = D::new(&tbox(0, 0, 10, 10, &para("x")));
    let t = first_shape(&d.scene()).text.clone().unwrap();
    assert_eq!(t.insets, (91_440.0, 45_720.0, 91_440.0, 45_720.0));
    assert_eq!(
        (t.anchor, t.wrap, t.vert, t.columns, t.autofit),
        (sd::Anchor::Top, true, sd::Vert::Horz, 1, sd::AutoFit::None)
    );
    // The other autofit forms and the East-Asian vertical.
    let one = |bp: &str| {
        let d = D::new(&shape("", &xf(0, 0, 10, 10), bp, "", &para("x"), ""));
        d.scene()
            .items
            .iter()
            .find_map(|i| match i {
                Item::Shape(s) => s.text.clone(),
                _ => None,
            })
            .unwrap()
    };
    assert_eq!(
        one("<a:bodyPr><a:spAutoFit/></a:bodyPr>").autofit,
        sd::AutoFit::Shape
    );
    assert_eq!(
        one("<a:bodyPr><a:noAutofit/></a:bodyPr>").autofit,
        sd::AutoFit::None
    );
    assert_eq!(one(r#"<a:bodyPr vert="eaVert"/>"#).vert, sd::Vert::EaVert);
    assert_eq!(one(r#"<a:bodyPr vert="vert"/>"#).vert, sd::Vert::Vert);
    assert_eq!(
        one("<a:bodyPr><a:normAutofit/></a:bodyPr>").autofit,
        sd::AutoFit::Normal {
            font_scale: 1.0,
            ln_spc_reduction: 0.0
        }
    );
}

#[test]
fn line_breaks_vertical_tabs_empty_paragraphs_and_the_size_of_an_empty_one() {
    let d = D::new(&tbox(
        0,
        0,
        1000,
        1000,
        &(r#"<a:p><a:r><a:rPr lang="en-US" sz="2000"/><a:t>one</a:t></a:r><a:br><a:rPr lang="en-US" sz="2000"/></a:br><a:r><a:rPr lang="en-US"/><a:t>two&#11;three</a:t></a:r></a:p>"#.to_string()
            + r#"<a:p><a:endParaRPr lang="en-US" sz="3200"/></a:p>"#),
    ));
    let sc = d.scene();
    let ps = &first_shape(&sc).text.as_ref().unwrap().paragraphs;
    let kinds: Vec<(sd::RunKind, &str)> = ps[0]
        .runs
        .iter()
        .map(|r| (r.kind, r.text.as_str()))
        .collect();
    assert_eq!(
        kinds,
        vec![
            (sd::RunKind::Text, "one"),
            (sd::RunKind::LineBreak, ""),
            (sd::RunKind::Text, "two"),
            (sd::RunKind::LineBreak, ""),
            (sd::RunKind::Text, "three"),
        ]
    );
    assert_eq!(ps[0].runs[1].size_pt, 20.0);
    assert!(ps[1].runs.is_empty());
    assert_eq!(ps[1].end_size_pt, 32.0);
}

#[test]
fn a_shape_without_text_or_visible_paint_is_not_an_item() {
    // An empty placeholder, an empty text box and a box with only whitespace draw nothing.
    let mut d = titled();
    d.slides = vec![slide(
        &(shape(
            r#"type="title""#,
            "",
            "",
            "",
            r#"<a:p><a:endParaRPr lang="en-US"/></a:p>"#,
            "",
        ) + &tbox(0, 0, 100, 100, &para("   "))
            + &shape("", &xf(0, 0, 10, 10), "", "", "", "")),
    )];
    assert!(d.scene().items.is_empty());
}

// ---------------------------------------------------------------------------------------------
// background, master and layout shapes
// ---------------------------------------------------------------------------------------------

#[test]
fn the_background_is_the_slides_then_the_layouts_then_the_masters() {
    let bg = |c: &str| {
        format!(
            r#"<p:bg><p:bgPr><a:solidFill><a:srgbClr val="{c}"/></a:solidFill><a:effectLst/></p:bgPr></p:bg>"#
        )
    };
    let mut d = D::new("");
    d.master_bg = bg("111111");
    assert_eq!(
        d.scene().background,
        Fill::Solid(Rgba::rgb(0x11, 0x11, 0x11))
    );
    d.layout_bg = bg("222222");
    assert_eq!(
        d.scene().background,
        Fill::Solid(Rgba::rgb(0x22, 0x22, 0x22))
    );
    d.slides[0].bg = bg("333333");
    assert_eq!(
        d.scene().background,
        Fill::Solid(Rgba::rgb(0x33, 0x33, 0x33))
    );
    // Nothing anywhere: the background colour of the scheme (white).
    assert_eq!(D::new("").scene().background, Fill::Solid(Rgba::WHITE));
}

#[test]
fn a_background_reference_points_into_the_themes_background_fills() {
    let mut d = D::new("");
    d.slides[0].bg =
        r#"<p:bg><p:bgRef idx="1001"><a:schemeClr val="bg2"/></p:bgRef></p:bg>"#.into();
    assert_eq!(
        d.scene().background,
        Fill::Solid(Rgba::rgb(0xE7, 0xE6, 0xE6))
    );
    // Index 1003 is a gradient whose stops carry phClr = accent1 with tint / shade.
    d.slides[0].bg =
        r#"<p:bg><p:bgRef idx="1003"><a:schemeClr val="accent1"/></p:bgRef></p:bg>"#.into();
    match d.scene().background {
        Fill::Gradient(g) => {
            assert_eq!(g.stops.len(), 2);
            assert!(matches!(g.kind, sd::GradKind::Linear { angle_deg, .. } if angle_deg == 90.0));
            // Tinted: lighter than accent1 at the start, shaded: darker at the end.
            assert!(
                g.stops[0].1.r > 0x44 && g.stops[1].1.r < 0x44,
                "{:?}",
                g.stops
            );
        }
        f => panic!("{f:?}"),
    }
}

#[test]
fn a_colour_map_override_changes_the_scheme_colours_of_a_slide() {
    let mut d = D::new("");
    d.slides[0].bg = r#"<p:bg><p:bgPr><a:solidFill><a:schemeClr val="bg1"/></a:solidFill><a:effectLst/></p:bgPr></p:bg>"#.into();
    assert_eq!(d.scene().background, Fill::Solid(Rgba::WHITE));
    d.slides[0].ovr = r#"<p:clrMapOvr><a:overrideClrMapping bg1="dk1" tx1="lt1" bg2="dk2" tx2="lt2" accent1="accent1" accent2="accent2" accent3="accent3" accent4="accent4" accent5="accent5" accent6="accent6" hlink="hlink" folHlink="folHlink"/></p:clrMapOvr>"#.into();
    d.slides[0].shapes = tbox(0, 0, 100, 100, &para("x"));
    let sc = d.scene();
    assert_eq!(sc.background, Fill::Solid(Rgba::BLACK));
    // The text colour tx1 is now white.
    assert_eq!(solid(run0(first_shape(&sc))), Rgba::WHITE);
}

fn marker(id: &str, x: i64) -> String {
    shape(
        "",
        &format!(
            "{}{RECT}<a:solidFill><a:srgbClr val=\"{id}\"/></a:solidFill>",
            xf(x, 0, 10, 10)
        ),
        "",
        "",
        "",
        "",
    )
}

fn fills(sc: &sd::SlideScene) -> Vec<Rgba> {
    shapes_of(&sc.items)
        .iter()
        .map(|s| match s.fill {
            Fill::Solid(c) => c,
            ref f => panic!("{f:?}"),
        })
        .collect()
}

#[test]
fn master_then_layout_then_slide_shapes_are_painted_in_that_order() {
    let mut d = D::new(&marker("000003", 3));
    d.master_shapes = marker("000001", 1);
    d.layout_shapes = marker("000002", 2);
    let c = fills(&d.scene());
    assert_eq!(
        c,
        vec![Rgba::rgb(0, 0, 1), Rgba::rgb(0, 0, 2), Rgba::rgb(0, 0, 3)]
    );
}

#[test]
fn show_master_shapes_off_hides_what_the_slide_and_the_layout_hide() {
    let mut d = D::new(&marker("000003", 3));
    d.master_shapes = marker("000001", 1);
    d.layout_shapes = marker("000002", 2);
    // The slide hides the layout's shapes and the master's.
    d.slides[0].attrs = r#" showMasterSp="0""#.into();
    assert_eq!(fills(&d.scene()), vec![Rgba::rgb(0, 0, 3)]);
    // The layout hides the master's only.
    d.slides[0].attrs = String::new();
    d.layout_attrs = r#" showMasterSp="0""#.into();
    assert_eq!(
        fills(&d.scene()),
        vec![Rgba::rgb(0, 0, 2), Rgba::rgb(0, 0, 3)]
    );
    d.layout_attrs = r#" showMasterSp="false""#.into();
    assert_eq!(
        fills(&d.scene()),
        vec![Rgba::rgb(0, 0, 2), Rgba::rgb(0, 0, 3)]
    );
    // "1" and absent show them.
    d.layout_attrs = r#" showMasterSp="1""#.into();
    assert_eq!(fills(&d.scene()).len(), 3);
}

#[test]
fn placeholders_of_the_master_and_the_layout_are_not_drawn_on_the_slide() {
    let mut d = titled();
    d.layout_shapes = shape(r#"type="title""#, "", "", "", &para("layout title"), "");
    d.slides = vec![slide(&tbox(0, 0, 100, 100, &para("mine")))];
    let sc = d.scene();
    assert_eq!(shapes_of(&sc.items).len(), 1);
}

#[test]
fn a_placeholder_inherits_fill_line_and_effects_from_the_layout() {
    let mut d = titled();
    d.layout_shapes = shape(
        r#"type="body" idx="1""#,
        r#"<a:solidFill><a:srgbClr val="00FF00"/></a:solidFill><a:ln w="19050"><a:solidFill><a:srgbClr val="0000FF"/></a:solidFill><a:prstDash val="dash"/></a:ln><a:effectLst><a:outerShdw blurRad="50800" dist="38100" dir="2700000"><a:srgbClr val="000000"><a:alpha val="40000"/></a:srgbClr></a:outerShdw></a:effectLst>"#,
        "",
        "",
        &para("layout"),
        "",
    );
    d.slides = vec![slide(&shape(r#"idx="1""#, "", "", "", &para("x"), ""))];
    let sc = d.scene();
    let s = first_shape(&sc);
    assert_eq!(s.fill, Fill::Solid(Rgba::rgb(0, 255, 0)));
    let l = s.line.as_ref().unwrap();
    assert_eq!((l.width, l.dash.clone()), (19050.0, sd::Dash::Dash));
    assert_eq!(l.fill, Fill::Solid(Rgba::rgb(0, 0, 255)));
    let sh = s.effects.outer_shadow.unwrap();
    assert_eq!((sh.blur_rad, sh.dist, sh.dir_deg), (50800.0, 38100.0, 45.0));
    assert_eq!(sh.color.a, 0.4);
    // A fill of the slide's own wins.
    d.slides = vec![slide(&shape(
        r#"idx="1""#,
        r#"<a:noFill/>"#,
        "",
        "",
        &para("x"),
        "",
    ))];
    assert_eq!(first_shape(&d.scene()).fill, Fill::None);
}

#[test]
fn a_placeholder_whose_idx_the_layout_lacks_inherits_nothing() {
    let mut d = titled();
    d.slides = vec![slide(&shape(
        r#"type="body" idx="9""#,
        "",
        "",
        "",
        &para("orphan"),
        "",
    ))];
    // No position anywhere: the shape cannot be placed and is not drawn.
    assert!(d.scene().items.is_empty());
}

// ---------------------------------------------------------------------------------------------
// shapes: groups, hidden, alternate content, connectors, frames
// ---------------------------------------------------------------------------------------------

fn group(off: (i64, i64, i64, i64), ch: (i64, i64, i64, i64), attrs: &str, inner: &str) -> String {
    format!(
        r#"<p:grpSp><p:nvGrpSpPr><p:cNvPr id="30" name="G"/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr><a:xfrm{attrs}><a:off x="{}" y="{}"/><a:ext cx="{}" cy="{}"/><a:chOff x="{}" y="{}"/><a:chExt cx="{}" cy="{}"/></a:xfrm></p:grpSpPr>{inner}</p:grpSp>"#,
        off.0, off.1, off.2, off.3, ch.0, ch.1, ch.2, ch.3
    )
}

#[test]
fn a_group_keeps_its_child_space_and_nests() {
    let inner = group(
        (10, 20, 30, 40),
        (0, 0, 60, 80),
        r#" rot="5400000" flipH="1""#,
        &marker("000001", 5),
    );
    let d = D::new(&group(
        (1000, 2000, 3000, 4000),
        (100, 200, 600, 800),
        "",
        &inner,
    ));
    let sc = d.scene();
    let Item::Group(g) = &sc.items[0] else {
        panic!()
    };
    assert_eq!(g.xfrm, sd::Xfrm::rect(1000.0, 2000.0, 3000.0, 4000.0));
    // (The members are mapped when read: the drawn group has no scale of its own.)
    assert_eq!(g.child_off, (1000.0, 2000.0));
    assert_eq!(g.child_ext, (3000.0, 4000.0));
    let Item::Group(g2) = &g.items[0] else {
        panic!()
    };
    assert_eq!(
        (g2.xfrm.rot_deg, g2.xfrm.flip_h, g2.xfrm.flip_v),
        (90.0, true, false)
    );
    // The inner box (10, 20, 30, 40) of the child space (100, 200, 600, 800) on 3000 x 4000
    // (factor 5) is (550, 1100, 150, 200); the member at 5 of its 60 x 80 space is at 550 + 5 * 2.5.
    assert_eq!(g2.xfrm.x, 550.0);
    assert_eq!(g2.xfrm.y, 1100.0);
    assert_eq!((g2.xfrm.w, g2.xfrm.h), (150.0, 200.0));
    assert_eq!(
        (g2.child_off, g2.child_ext),
        ((550.0, 1100.0), (150.0, 200.0))
    );
    assert!(matches!(&g2.items[0], Item::Shape(s) if s.xfrm.x == 562.5));
    // A group with no child extent maps onto itself.
    let d = D::new(&format!(
        r#"<p:grpSp><p:nvGrpSpPr><p:cNvPr id="3" name="G"/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr><a:xfrm><a:off x="10" y="20"/><a:ext cx="300" cy="400"/></a:xfrm></p:grpSpPr>{}</p:grpSp>"#,
        marker("000001", 5)
    ));
    let Item::Group(g) = &d.scene().items[0] else {
        panic!()
    };
    assert_eq!((g.child_off, g.child_ext), ((10.0, 20.0), (300.0, 400.0)));
    // An empty group is nothing.
    let empty = group((0, 0, 10, 10), (0, 0, 10, 10), "", "");
    assert!(D::new(&empty).scene().items.is_empty());
}

#[test]
fn a_group_fill_is_the_fill_of_its_members() {
    let member = shape(
        "",
        &format!("{}{RECT}<a:grpFill/>", xf(0, 0, 10, 10)),
        "",
        "",
        "",
        "",
    );
    let g = format!(
        r#"<p:grpSp><p:nvGrpSpPr><p:cNvPr id="3" name="G"/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="10" cy="10"/><a:chOff x="0" y="0"/><a:chExt cx="10" cy="10"/></a:xfrm><a:solidFill><a:srgbClr val="ABCDEF"/></a:solidFill></p:grpSpPr>{member}</p:grpSp>"#
    );
    assert_eq!(
        fills(&D::new(&g).scene()),
        vec![Rgba::rgb(0xAB, 0xCD, 0xEF)]
    );
}

#[test]
fn hidden_shapes_groups_and_pictures_are_skipped() {
    let hide = |s: String| s.replacen(r#"<p:cNvPr id="2""#, r#"<p:cNvPr hidden="1" id="2""#, 1);
    let sp = hide(marker("000001", 1));
    let gr = group((0, 0, 10, 10), (0, 0, 10, 10), "", &marker("000002", 2)).replacen(
        r#"<p:cNvPr id="30""#,
        r#"<p:cNvPr hidden="true" id="30""#,
        1,
    );
    let pic = pic_xml("rIdP", &xf(0, 0, 10, 10), "", "").replacen(
        "<p:cNvPr ",
        r#"<p:cNvPr hidden="1" "#,
        1,
    );
    let visible = marker("000009", 9);
    let sc = D::new(&(sp + &gr + &pic + &visible)).scene();
    assert_eq!(fills(&sc), vec![Rgba::rgb(0, 0, 9)]);
}

#[test]
fn alternate_content_reads_a_readable_choice_else_the_fallback() {
    let ac = |req: &str| {
        format!(
            r#"<mc:AlternateContent><mc:Choice Requires="{req}">{}</mc:Choice><mc:Fallback>{}</mc:Fallback></mc:AlternateContent>"#,
            marker("0000C1", 1),
            marker("0000F1", 2)
        )
    };
    assert_eq!(
        fills(&D::new(&ac("a14")).scene()),
        vec![Rgba::rgb(0, 0, 0xC1)]
    );
    assert_eq!(
        fills(&D::new(&ac("p14")).scene()),
        vec![Rgba::rgb(0, 0, 0xF1)]
    );
    assert_eq!(
        fills(&D::new(&ac("a14 p14")).scene()),
        vec![Rgba::rgb(0, 0, 0xF1)]
    );
}

#[test]
fn a_connector_is_a_line_shape_with_flips_and_arrow_heads() {
    let cxn = r#"<p:cxnSp><p:nvCxnSpPr><p:cNvPr id="4" name="K"/><p:cNvCxnSpPr/><p:nvPr/></p:nvCxnSpPr><p:spPr><a:xfrm flipV="1"><a:off x="100" y="200"/><a:ext cx="3000" cy="1000"/></a:xfrm><a:prstGeom prst="straightConnector1"><a:avLst/></a:prstGeom><a:ln w="28575" cap="rnd"><a:solidFill><a:srgbClr val="FF0000"/></a:solidFill><a:round/><a:headEnd type="oval" w="sm" len="lg"/><a:tailEnd type="triangle"/></a:ln></p:spPr></p:cxnSp>"#;
    let sc = D::new(cxn).scene();
    let s = first_shape(&sc);
    assert_eq!(s.geom, sd::Geometry::Line);
    assert!(s.xfrm.flip_v && !s.xfrm.flip_h);
    let l = s.line.as_ref().unwrap();
    assert_eq!(
        (l.width, l.cap, l.join),
        (28575.0, sd::Cap::Round, sd::Join::Round)
    );
    assert_eq!(
        l.head,
        Some(sd::Arrow {
            kind: sd::ArrowKind::Oval,
            w: sd::ArrowSize::Sm,
            len: sd::ArrowSize::Lg
        })
    );
    assert_eq!(
        l.tail,
        Some(sd::Arrow {
            kind: sd::ArrowKind::Triangle,
            w: sd::ArrowSize::Med,
            len: sd::ArrowSize::Med
        })
    );
}

#[test]
fn a_connector_takes_its_line_from_the_style() {
    let cxn = r#"<p:cxnSp><p:nvCxnSpPr><p:cNvPr id="4" name="K"/><p:cNvCxnSpPr/><p:nvPr/></p:nvCxnSpPr><p:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="3000" cy="0"/></a:xfrm><a:prstGeom prst="line"><a:avLst/></a:prstGeom></p:spPr><p:style><a:lnRef idx="2"><a:schemeClr val="accent3"/></a:lnRef><a:fillRef idx="0"><a:schemeClr val="accent3"/></a:fillRef><a:effectRef idx="0"><a:schemeClr val="accent3"/></a:effectRef><a:fontRef idx="minor"><a:schemeClr val="tx1"/></a:fontRef></p:style></p:cxnSp>"#;
    let sc = D::new(cxn).scene();
    let l = first_shape(&sc).line.clone().unwrap();
    assert_eq!(l.width, 12700.0);
    assert_eq!(l.fill, Fill::Solid(Rgba::rgb(0xA5, 0xA5, 0xA5)));
    assert_eq!(first_shape(&sc).geom, sd::Geometry::Line);
}

#[test]
fn style_references_fill_line_and_effect_with_the_style_colour() {
    let style = r#"<p:style><a:lnRef idx="3"><a:schemeClr val="accent1"><a:shade val="50000"/></a:schemeClr></a:lnRef><a:fillRef idx="2"><a:schemeClr val="accent2"/></a:fillRef><a:effectRef idx="3"><a:schemeClr val="accent2"/></a:effectRef><a:fontRef idx="minor"><a:schemeClr val="lt1"/></a:fontRef></p:style>"#;
    let d = D::new(&shape(
        "",
        &format!("{}{RECT}", xf(0, 0, 100, 100)),
        "",
        "",
        "",
        style,
    ));
    let sc = d.scene();
    let s = first_shape(&sc);
    match &s.fill {
        Fill::Gradient(g) => {
            // phClr = accent2 with lumMod 110% / 90%.
            assert_eq!(g.stops.len(), 2);
            assert!(g.stops[0].1.r >= g.stops[1].1.r);
        }
        f => panic!("{f:?}"),
    }
    let l = s.line.as_ref().unwrap();
    assert_eq!(l.width, 19050.0);
    match l.fill {
        Fill::Solid(c) => assert!(c.b < 0xC4 && c.r < 0x44, "shade darkens accent1: {c:?}"),
        ref f => panic!("{f:?}"),
    }
    let sh = s.effects.outer_shadow.unwrap();
    assert_eq!((sh.blur_rad, sh.dist, sh.dir_deg), (57150.0, 19050.0, 90.0));
    assert!(!sh.rot_with_shape);
    // The spPr's own line replaces the style's width but keeps its colour.
    let d = D::new(&shape(
        "",
        &format!("{}{RECT}<a:ln w=\"3175\"/>", xf(0, 0, 100, 100)),
        "",
        "",
        "",
        style,
    ));
    let l = first_shape(&d.scene()).line.clone().unwrap();
    assert_eq!(l.width, 3175.0);
    // An explicit noFill line removes it.
    let d = D::new(&shape(
        "",
        &format!("{}{RECT}<a:ln><a:noFill/></a:ln>", xf(0, 0, 100, 100)),
        "",
        "",
        "",
        style,
    ));
    assert!(first_shape(&d.scene()).line.is_none());
    // An explicit empty effectLst removes the style's shadow.
    let d = D::new(&shape(
        "",
        &format!("{}{RECT}<a:effectLst/>", xf(0, 0, 100, 100)),
        "",
        "",
        "",
        style,
    ));
    assert_eq!(first_shape(&d.scene()).effects, sd::Effects::default());
}

// ---------------------------------------------------------------------------------------------
// fills and lines
// ---------------------------------------------------------------------------------------------

fn with_fill(fill: &str) -> sd::ShapeItem {
    let d = D::new(&shape(
        "",
        &format!("{}{RECT}{fill}", xf(0, 0, 100, 100)),
        "",
        "",
        "",
        "",
    ));
    first_shape(&d.scene()).clone()
}

fn with_line(ln: &str) -> sd::Line {
    let d = D::new(&shape(
        "",
        &format!("{}{RECT}{ln}", xf(0, 0, 100, 100)),
        "",
        "",
        "",
        "",
    ));
    first_shape(&d.scene()).line.clone().expect("line")
}

#[test]
fn gradient_fills() {
    let f = with_fill(r#"<a:gradFill rotWithShape="0"><a:gsLst><a:gs pos="100000"><a:srgbClr val="0000FF"/></a:gs><a:gs pos="0"><a:srgbClr val="FF0000"/></a:gs><a:gs pos="50000"><a:srgbClr val="00FF00"><a:alpha val="50000"/></a:srgbClr></a:gs></a:gsLst><a:lin ang="2700000" scaled="1"/></a:gradFill>"#).fill;
    let Fill::Gradient(g) = f else { panic!() };
    assert_eq!(
        g.kind,
        sd::GradKind::Linear {
            angle_deg: 45.0,
            scaled: true
        }
    );
    assert!(!g.rot_with_shape);
    let pos: Vec<f64> = g.stops.iter().map(|s| s.0).collect();
    assert_eq!(pos, vec![0.0, 0.5, 1.0]);
    assert_eq!(g.stops[0].1, RED);
    assert_eq!(g.stops[1].1.a, 0.5);
    // Path gradients.
    let path = |kind: &str| {
        let Fill::Gradient(g) = with_fill(&format!(r#"<a:gradFill><a:gsLst><a:gs pos="0"><a:srgbClr val="FF0000"/></a:gs><a:gs pos="100000"><a:srgbClr val="0000FF"/></a:gs></a:gsLst><a:path path="{kind}"><a:fillToRect l="50000" t="25000" r="50000" b="75000"/></a:path></a:gradFill>"#)).fill else { panic!() };
        g
    };
    assert_eq!(path("circle").kind, sd::GradKind::Radial);
    assert_eq!(path("rect").kind, sd::GradKind::Rect);
    assert_eq!(path("shape").kind, sd::GradKind::Path);
    assert_eq!(path("circle").fill_to_rect, (0.5, 0.25, 0.5, 0.75));
    // One stop is a plain colour.
    assert_eq!(with_fill(r#"<a:gradFill><a:gsLst><a:gs pos="0"><a:srgbClr val="FF0000"/></a:gs></a:gsLst></a:gradFill>"#).fill, Fill::Solid(RED));
}

#[test]
fn pattern_and_no_fills() {
    let f = with_fill(r#"<a:pattFill prst="dnDiag"><a:fgClr><a:srgbClr val="FF0000"/></a:fgClr><a:bgClr><a:schemeClr val="bg1"/></a:bgClr></a:pattFill>"#).fill;
    assert_eq!(
        f,
        Fill::Pattern {
            preset: "dnDiag".into(),
            fg: RED,
            bg: Rgba::WHITE
        }
    );
    // (A shape that paints nothing at all is no item: the line keeps this one.)
    let s = with_fill(
        r#"<a:noFill/><a:ln w="12700"><a:solidFill><a:srgbClr val="000000"/></a:solidFill></a:ln>"#,
    );
    assert_eq!(s.fill, Fill::None);
    assert!(s.line.is_some());
}

fn pic_xml(rid: &str, sp_pr: &str, blip_fill_extra: &str, blip_extra: &str) -> String {
    format!(
        r#"<p:pic><p:nvPicPr><p:cNvPr id="40" name="Pic" descr="a picture"/><p:cNvPicPr/><p:nvPr/></p:nvPicPr><p:blipFill><a:blip r:embed="{rid}">{blip_extra}</a:blip>{blip_fill_extra}</p:blipFill><p:spPr>{sp_pr}</p:spPr></p:pic>"#
    )
}

fn with_media(mut d: D) -> D {
    d.media.push(("p1.png".into(), tiny_png(1)));
    d.media.push(("p2.png".into(), tiny_png(2)));
    d.slides[0]
        .rels
        .push(("rIdP".into(), "image".into(), "../media/p1.png".into()));
    d.slides[0]
        .rels
        .push(("rIdQ".into(), "image".into(), "../media/p2.png".into()));
    d
}

#[test]
fn picture_fills_stretch_and_tile() {
    let d = with_media(D::new(&shape(
        "",
        &format!(
            "{}{RECT}{}",
            xf(0, 0, 100, 100),
            r#"<a:blipFill><a:blip r:embed="rIdP"/><a:srcRect l="10000" t="20000" r="30000" b="40000"/><a:stretch><a:fillRect l="1000" t="2000" r="3000" b="4000"/></a:stretch></a:blipFill>"#
        ),
        "",
        "",
        "",
        "",
    )));
    let doc = d.load();
    let Fill::Image(img) = &first_shape(&doc.slide_scenes[0]).fill else {
        panic!()
    };
    assert!(doc.images.iter().any(|i| i.key == img.key));
    assert_eq!(img.crop, (0.1, 0.2, 0.3, 0.4));
    assert_eq!(
        img.mode,
        sd::ImageMode::Stretch {
            fill_rect: (0.01, 0.02, 0.03, 0.04)
        }
    );
    assert_eq!(img.alpha, 1.0);
    let d = with_media(D::new(&shape(
        "",
        &format!(
            "{}{RECT}{}",
            xf(0, 0, 100, 100),
            r#"<a:blipFill><a:blip r:embed="rIdP"><a:alphaModFix amt="40000"/></a:blip><a:tile tx="100" ty="200" sx="50000" sy="150000" flip="xy" algn="ctr"/></a:blipFill>"#
        ),
        "",
        "",
        "",
        "",
    )));
    let Fill::Image(img) = first_shape(&d.scene()).fill.clone() else {
        panic!()
    };
    assert_eq!(
        img.mode,
        sd::ImageMode::Tile {
            sx: 0.5,
            sy: 1.5,
            tx: 100.0,
            ty: 200.0,
            align: sd::RectAlign::Center,
            flip: sd::TileFlip::Xy
        }
    );
    assert_eq!(img.alpha, 0.4);
    // A picture fill whose picture is missing paints nothing.
    let d = D::new(&shape(
        "",
        &format!(
            "{}{RECT}{}",
            xf(0, 0, 100, 100),
            r#"<a:blipFill><a:blip r:embed="rIdNone"/></a:blipFill>"#
        ),
        "",
        "",
        "",
        "",
    ));
    assert!(d.scene().items.is_empty());
}

#[test]
fn line_properties() {
    let l = with_line(
        r#"<a:ln w="38100" cap="sq" cmpd="dbl"><a:gradFill><a:gsLst><a:gs pos="0"><a:srgbClr val="FF0000"/></a:gs><a:gs pos="100000"><a:srgbClr val="0000FF"/></a:gs></a:gsLst><a:lin ang="0"/></a:gradFill><a:prstDash val="sysDashDot"/><a:bevel/></a:ln>"#,
    );
    assert_eq!(
        (l.width, l.cap, l.compound, l.join),
        (38100.0, sd::Cap::Square, sd::Compound::Dbl, sd::Join::Bevel)
    );
    assert_eq!(l.dash, sd::Dash::SysDashDot);
    assert!(matches!(l.fill, Fill::Gradient(_)));
    let l = with_line(
        r#"<a:ln w="12700"><a:solidFill><a:srgbClr val="000000"/></a:solidFill><a:custDash><a:ds d="200000" sp="100000"/><a:ds d="50000" sp="300000"/></a:custDash><a:miter lim="400000"/></a:ln>"#,
    );
    assert_eq!(l.dash, sd::Dash::Custom(vec![(2.0, 1.0), (0.5, 3.0)]));
    assert_eq!(l.join, sd::Join::Miter(4.0));
    // No width: a hairline; the default join of the model.
    let l = with_line(r#"<a:ln><a:solidFill><a:srgbClr val="000000"/></a:solidFill></a:ln>"#);
    assert_eq!(l.width, 0.0);
    assert_eq!(l.dash, sd::Dash::Solid);
    for (v, want) in [
        ("solid", sd::Dash::Solid),
        ("dot", sd::Dash::Dot),
        ("dash", sd::Dash::Dash),
        ("lgDash", sd::Dash::LgDash),
        ("dashDot", sd::Dash::DashDot),
        ("lgDashDot", sd::Dash::LgDashDot),
        ("lgDashDotDot", sd::Dash::LgDashDotDot),
        ("sysDash", sd::Dash::SysDash),
        ("sysDot", sd::Dash::SysDot),
        ("sysDashDotDot", sd::Dash::SysDashDotDot),
    ] {
        let l = with_line(&format!(
            r#"<a:ln><a:solidFill><a:srgbClr val="000000"/></a:solidFill><a:prstDash val="{v}"/></a:ln>"#
        ));
        assert_eq!(l.dash, want, "{v}");
    }
    for (v, want) in [
        ("thickThin", sd::Compound::ThickThin),
        ("thinThick", sd::Compound::ThinThick),
        ("tri", sd::Compound::Tri),
        ("sng", sd::Compound::Sng),
    ] {
        let l = with_line(&format!(
            r#"<a:ln cmpd="{v}"><a:solidFill><a:srgbClr val="000000"/></a:solidFill></a:ln>"#
        ));
        assert_eq!(l.compound, want, "{v}");
    }
    for (kind, want) in [
        ("stealth", sd::ArrowKind::Stealth),
        ("diamond", sd::ArrowKind::Diamond),
        ("arrow", sd::ArrowKind::Arrow),
    ] {
        let l = with_line(&format!(
            r#"<a:ln><a:solidFill><a:srgbClr val="000000"/></a:solidFill><a:tailEnd type="{kind}" w="lg" len="sm"/></a:ln>"#
        ));
        assert_eq!(
            l.tail,
            Some(sd::Arrow {
                kind: want,
                w: sd::ArrowSize::Lg,
                len: sd::ArrowSize::Sm
            })
        );
        assert_eq!(l.head, None);
    }
    // `type="none"` is no arrow.
    let l = with_line(
        r#"<a:ln><a:solidFill><a:srgbClr val="000000"/></a:solidFill><a:headEnd type="none"/></a:ln>"#,
    );
    assert_eq!(l.head, None);
}

#[test]
fn effects() {
    let s = with_fill(
        r#"<a:effectLst><a:outerShdw blurRad="63500" dist="50800" dir="2700000" sx="90000" sy="80000" rotWithShape="0"><a:srgbClr val="000000"><a:alpha val="35000"/></a:srgbClr></a:outerShdw><a:innerShdw blurRad="1000" dist="2000" dir="5400000"><a:schemeClr val="accent1"/></a:innerShdw><a:glow rad="101600"><a:srgbClr val="FF0000"><a:alpha val="40000"/></a:srgbClr></a:glow><a:softEdge rad="63500"/><a:reflection blurRad="6350" stA="50000" endA="300" endPos="55000" dist="12700" dir="5400000" fadeDir="5400000" sy="-100000" algn="bl" rotWithShape="0"/></a:effectLst>"#,
    );
    let e = s.effects;
    let sh = e.outer_shadow.unwrap();
    assert_eq!(
        (
            sh.blur_rad,
            sh.dist,
            sh.dir_deg,
            sh.sx,
            sh.sy,
            sh.rot_with_shape
        ),
        (63500.0, 50800.0, 45.0, 0.9, 0.8, false)
    );
    assert_eq!(sh.color.a, 0.35);
    let inner = e.inner_shadow.unwrap();
    assert_eq!(
        (inner.blur_rad, inner.dist, inner.dir_deg),
        (1000.0, 2000.0, 90.0)
    );
    assert_eq!(inner.color, Rgba::rgb(0x44, 0x72, 0xC4));
    let g = e.glow.unwrap();
    assert_eq!((g.rad, g.color.r, g.color.a), (101600.0, 255, 0.4));
    assert_eq!(e.soft_edge, Some(63500.0));
    let r = e.reflection.unwrap();
    assert_eq!(
        (r.start_alpha, r.dist, r.dir_deg, r.fade_dir_deg, r.sy),
        (0.5, 12700.0, 90.0, 90.0, -1.0)
    );
    assert!((r.end_alpha - 0.003).abs() < 1e-9);
}

// ---------------------------------------------------------------------------------------------
// geometry
// ---------------------------------------------------------------------------------------------

#[test]
fn exact_presets_keep_their_cheap_forms_and_the_others_are_evaluated() {
    let g = |prst: &str| {
        let d = D::new(&shape(
            "",
            &format!(
                r#"{}<a:prstGeom prst="{prst}"><a:avLst/></a:prstGeom><a:solidFill><a:srgbClr val="000000"/></a:solidFill>"#,
                xf(0, 0, 100, 100)
            ),
            "",
            "",
            "",
            "",
        ));
        first_shape(&d.scene()).geom.clone()
    };
    assert_eq!(g("rect"), sd::Geometry::Rect);
    assert_eq!(g("ellipse"), sd::Geometry::Ellipse);
    assert_eq!(g("line"), sd::Geometry::Line);
    assert_eq!(g("straightConnector1"), sd::Geometry::Line);
    // Everything else is evaluated by the preset engine; a name that is no preset is the box.
    assert!(matches!(g("roundRect"), sd::Geometry::Paths(_)));
    assert_eq!(g("nonsense"), sd::Geometry::Rect);
    // A custom geometry with no path is evaluated too (it draws nothing); one that is not
    // readable (too many guides) is the box -- see `shapes_e3`.
    let d = D::new(&shape(
        "",
        &format!(
            r#"{}<a:custGeom><a:pathLst/></a:custGeom><a:solidFill><a:srgbClr val="000000"/></a:solidFill>"#,
            xf(0, 0, 100, 100)
        ),
        "",
        "",
        "",
        "",
    ));
    assert_eq!(first_shape(&d.scene()).geom, sd::Geometry::Paths(vec![]));
}

#[test]
fn adjust_values_are_read_from_the_value_formulas() {
    let n = elem(
        r#"<a:avLst xmlns:a="x"><a:gd name="adj1" fmla="val 25000"/><a:gd name="adj2" fmla="val -5000"/><a:gd name="adj3" fmla="*/ w 1 2"/><a:gd name="adj4" fmla="val abc"/><a:gd fmla="val 1"/></a:avLst>"#,
    );
    assert_eq!(
        geom::adjust_values(&n),
        vec![("adj1".to_string(), 25000.0), ("adj2".to_string(), -5000.0)]
    );
    let many: String = (0..100)
        .map(|i| format!(r#"<a:gd name="a{i}" fmla="val {i}"/>"#))
        .collect();
    let n = elem(&format!(r#"<a:avLst xmlns:a="x">{many}</a:avLst>"#));
    assert_eq!(geom::adjust_values(&n).len(), 32);
}

// ---------------------------------------------------------------------------------------------
// pictures
// ---------------------------------------------------------------------------------------------

#[test]
fn a_picture_has_its_crop_clip_line_and_alpha() {
    let sp_pr = format!(
        "{}<a:prstGeom prst=\"ellipse\"><a:avLst/></a:prstGeom><a:ln w=\"12700\"><a:solidFill><a:srgbClr val=\"FF0000\"/></a:solidFill></a:ln>",
        xf(10, 20, 300, 400)
    );
    let d = with_media(D::new(&pic_xml(
        "rIdP",
        &sp_pr,
        r#"<a:srcRect l="25000" r="25000"/><a:stretch><a:fillRect/></a:stretch>"#,
        r#"<a:alphaModFix amt="50000"/>"#,
    )));
    let doc = d.load();
    let Item::Picture(p) = &doc.slide_scenes[0].items[0] else {
        panic!()
    };
    assert_eq!(
        (p.xfrm.x, p.xfrm.y, p.xfrm.w, p.xfrm.h),
        (10.0, 20.0, 300.0, 400.0)
    );
    assert_eq!(p.image.crop, (0.25, 0.0, 0.25, 0.0));
    assert_eq!(p.image.alpha, 0.5);
    assert_eq!(p.geom, sd::Geometry::Ellipse);
    assert_eq!(p.line.as_ref().unwrap().width, 12700.0);
    assert!(doc.images.iter().any(|i| i.key == p.image.key));
    // The text view's picture and the scene's are the same key.
    assert!(doc.markdown.is_empty() || doc.markdown.contains(&p.image.key));
}

#[test]
fn a_picture_prefers_the_embedded_blip_and_keeps_the_svg_only_when_it_is_all_there_is() {
    let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"/>"#.to_vec();
    let ext = |rid: &str| {
        format!(
            r#"<a:extLst><a:ext uri="{{96DAC541-7B7A-43D3-8B79-37D633B846F1}}"><asvg:svgBlip xmlns:asvg="http://schemas.microsoft.com/office/drawing/2016/SVG/main" r:embed="{rid}"/></a:ext></a:extLst>"#
        )
    };
    let mut d = with_media(D::new(&pic_xml(
        "rIdP",
        &xf(0, 0, 10, 10),
        "",
        &ext("rIdS"),
    )));
    d.media.push(("icon.svg".into(), svg.clone()));
    d.slides[0]
        .rels
        .push(("rIdS".into(), "image".into(), "../media/icon.svg".into()));
    let doc = d.load();
    let Item::Picture(p) = &doc.slide_scenes[0].items[0] else {
        panic!()
    };
    let used = doc.images.iter().find(|i| i.key == p.image.key).unwrap();
    assert_eq!(used.name, "p1.png");
    // An embed that is not a picture konoma loads (an EMF): the SVG beside it.
    let mut d = D::new(&pic_xml("rIdE", &xf(0, 0, 10, 10), "", &ext("rIdS")));
    d.media.push(("icon.svg".into(), svg));
    d.media.push(("old.emf".into(), vec![1, 0, 0, 0]));
    d.slides[0]
        .rels
        .push(("rIdS".into(), "image".into(), "../media/icon.svg".into()));
    d.slides[0]
        .rels
        .push(("rIdE".into(), "image".into(), "../media/old.emf".into()));
    let doc = d.load();
    let Item::Picture(p) = &doc.slide_scenes[0].items[0] else {
        panic!()
    };
    assert_eq!(
        doc.images
            .iter()
            .find(|i| i.key == p.image.key)
            .unwrap()
            .name,
        "icon.svg"
    );
}

#[test]
fn a_picture_that_cannot_be_shown_is_drawn_as_a_placeholder() {
    // Linked, missing and unsupported pictures keep their place.
    let d = D::new(
        &(pic_xml("rIdNone", &xf(0, 0, 10, 10), "", "")
            + &pic_xml("rIdE", &xf(20, 0, 10, 10), "", "")),
    );
    let mut d = d;
    d.media.push(("old.emf".into(), vec![1, 0, 0, 0]));
    d.slides[0]
        .rels
        .push(("rIdE".into(), "image".into(), "../media/old.emf".into()));
    let doc = d.load();
    assert_eq!(doc.slide_scenes[0].items.len(), 2);
    // The picture with no relationship keeps the placeholder key...
    let Item::Picture(p) = &doc.slide_scenes[0].items[0] else {
        panic!()
    };
    assert_eq!(p.image.key, shapes::MISSING_PICTURE);
    assert!(doc.images.iter().all(|im| im.key != p.image.key));
    // ...an EMF has a key of its own (the renderer converts metafiles; bytes it cannot convert
    // are drawn as the placeholder), and the text view never refers to it.
    let Item::Picture(p) = &doc.slide_scenes[0].items[1] else {
        panic!()
    };
    let emf = doc.images.iter().find(|im| im.key == p.image.key).unwrap();
    assert_eq!(emf.name, "old.emf");
    assert!(!doc.markdown.contains(&emf.key));
    let r = sd::render_svg(&doc.slide_scenes[0], &|k| {
        doc.images
            .iter()
            .find(|i| i.key == k)
            .map(|i| std::sync::Arc::new(i.bytes.clone()))
    });
    assert!(r.svg.contains("<path"), "{}", r.svg);
}

#[test]
fn master_and_layout_pictures_are_loaded_once_and_shared_with_the_text_view() {
    let png = tiny_png(5);
    let mut d = D::new(&pic_xml("rIdP", &xf(0, 0, 10, 10), "", ""));
    d.media.push(("logo.png".into(), png.clone()));
    d.master_shapes = pic_xml("rIdM1", &xf(1, 1, 5, 5), "", "");
    d.master_rels
        .push(("rIdM1".into(), "image".into(), "../media/logo.png".into()));
    d.layout_shapes = pic_xml("rIdL1", &xf(2, 2, 5, 5), "", "");
    d.layout_rels
        .push(("rIdL1".into(), "image".into(), "../media/logo.png".into()));
    d.slides[0]
        .rels
        .push(("rIdP".into(), "image".into(), "../media/logo.png".into()));
    let doc = d.load();
    let keys: Vec<String> = doc.slide_scenes[0]
        .items
        .iter()
        .map(|i| match i {
            Item::Picture(p) => p.image.key.clone(),
            _ => panic!(),
        })
        .collect();
    assert_eq!(keys.len(), 3);
    // The same picture, the same key: one image in the document.
    assert!(keys.iter().all(|k| *k == keys[0]), "{keys:?}");
    assert_eq!(doc.images.len(), 1);
    // The text view shows the slide's picture with that very key.
    assert!(doc.markdown.contains(&keys[0]), "{}", doc.markdown);
}

#[test]
fn an_embedded_object_is_drawn_as_its_picture_and_tables_charts_and_diagrams_are_not_yet() {
    let frame = |data: &str| {
        format!(
            r#"<p:graphicFrame><p:nvGraphicFramePr><p:cNvPr id="5" name="F"/><p:cNvGraphicFramePr/><p:nvPr/></p:nvGraphicFramePr><p:xfrm><a:off x="10" y="20"/><a:ext cx="300" cy="400"/></p:xfrm><a:graphic>{data}</a:graphic></p:graphicFrame>"#
        )
    };
    let ole = frame(
        r#"<a:graphicData uri="http://schemas.openxmlformats.org/presentationml/2006/ole"><mc:AlternateContent><mc:Choice Requires="v"><p:oleObj spid="_x0000_s1" name="Obj" r:id="rIdO" imgW="1" imgH="1"><p:embed/></p:oleObj></mc:Choice><mc:Fallback><p:oleObj name="Obj" r:id="rIdO"><p:embed/><p:pic><p:nvPicPr><p:cNvPr id="0" name=""/><p:cNvPicPr/><p:nvPr/></p:nvPicPr><p:blipFill><a:blip r:embed="rIdP"/><a:stretch><a:fillRect/></a:stretch></p:blipFill><p:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="1" cy="1"/></a:xfrm></p:spPr></p:pic></p:oleObj></mc:Fallback></mc:AlternateContent></a:graphicData>"#,
    );
    let d = with_media(D::new(&ole));
    let doc = d.load();
    let Item::Picture(p) = &doc.slide_scenes[0].items[0] else {
        panic!("{:?}", doc.slide_scenes[0])
    };
    assert_eq!(
        (p.xfrm.x, p.xfrm.y, p.xfrm.w, p.xfrm.h),
        (10.0, 20.0, 300.0, 400.0)
    );
    assert!(doc.images.iter().any(|i| i.key == p.image.key));
    let table = frame(
        r#"<a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/table"><a:tbl><a:tblPr/><a:tblGrid/><a:tr h="1"><a:tc><a:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:t>cell</a:t></a:r></a:p></a:txBody><a:tcPr/></a:tc></a:tr></a:tbl></a:graphicData>"#,
    );
    let chart = frame(
        r#"<a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/chart"><c:chart r:id="rIdC"/></a:graphicData>"#,
    );
    let dgm = frame(
        r#"<a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/diagram"><dgm:relIds r:dm="rIdD" r:lo="rIdL" r:qs="rIdQ" r:cs="rIdCs"/></a:graphicData>"#,
    );
    // (Plugged in by later tasks: nothing is drawn for them today, and nothing breaks.)
    let sc = D::new(&(table + &chart + &dgm)).scene();
    assert!(sc.items.is_empty());
    assert!(!sc.truncated);
}

// ---------------------------------------------------------------------------------------------
// the picture view
// ---------------------------------------------------------------------------------------------

fn notes_deck() -> D {
    let mut d = D::new("");
    d.master_tx = TITLE_STYLE.to_string();
    d.master_shapes = shape(r#"type="title""#, &xf(1, 2, 3, 4), "", "", &para("c"), "");
    let mut s1 = slide(&shape(
        r#"type="title""#,
        "",
        "",
        "",
        &para("First *deck*"),
        "",
    ));
    s1.notes = Some(shape(
        r#"type="body" idx="1""#,
        &xf(0, 0, 1, 1),
        "",
        "",
        &(para("say this") + &para("# and that")),
        "",
    ));
    let s2 = slide(&tbox(0, 0, 100, 100, &para("## not a heading")));
    let mut s3 = slide(&shape(r#"type="title""#, "", "", "", &para("Hidden"), ""));
    s3.attrs = r#" show="0""#.into();
    d.slides = vec![s1, s2, s3];
    d
}

#[test]
fn the_picture_view_has_the_same_headings_and_notes_as_the_text_view() {
    for lang in [Lang::En, Lang::Jp] {
        let d = notes_deck();
        let doc = d.load_with(&DocOptions {
            lang,
            ..DocOptions::default()
        });
        assert_eq!(doc.slides.len(), 3);
        assert_eq!(doc.slide_scenes.len(), 3);
        assert_eq!(doc.slide_keys.len(), 3);
        let text_heads: Vec<&str> = doc
            .markdown
            .lines()
            .filter(|l| l.starts_with("## "))
            .collect();
        let pic_heads: Vec<&str> = doc
            .picture_markdown
            .lines()
            .filter(|l| l.starts_with("## "))
            .collect();
        assert_eq!(text_heads.len(), 3, "{}", doc.markdown);
        assert_eq!(pic_heads, text_heads);
        // The notes block is the very text of the text view's.
        let notes_start = doc.markdown.find("> **").expect("notes in the text view");
        let notes = &doc.markdown[notes_start..];
        let notes = notes.split("\n\n## ").next().unwrap().trim_end();
        assert!(
            doc.picture_markdown.contains(notes),
            "{notes:?} in {}",
            doc.picture_markdown
        );
        // Each picture is referenced once, right after its heading.
        for (i, k) in doc.slide_keys.iter().enumerate() {
            assert_eq!(doc.picture_markdown.matches(k.as_str()).count(), 1);
            let head = text_heads[i];
            assert!(
                doc.picture_markdown.contains(&format!("{head}\n\n![")),
                "{head}"
            );
            assert!(
                k.starts_with("office-img://") && k.ends_with(&format!("/slide-{}.svg", i + 1)),
                "{k}"
            );
        }
        // The text of the slides is not in the picture view (but the notes are).
        assert!(!doc.picture_markdown.contains("not a heading"));
        assert!(doc.picture_markdown.contains("say this"));
        // Slide 2's text paragraph that looks like a heading is escaped in both.
        assert_eq!(
            doc.picture_markdown
                .lines()
                .filter(|l| l.starts_with("## "))
                .count(),
            3
        );
    }
}

#[test]
fn picture_keys_are_unique_across_decks_and_loads() {
    let d = notes_deck();
    let a = d.load();
    let b = d.load();
    assert_eq!(a.slide_keys.len(), 3);
    let mut all: Vec<&String> = a.slide_keys.iter().chain(b.slide_keys.iter()).collect();
    all.sort();
    all.dedup();
    assert_eq!(all.len(), 6);
    let prefix = |k: &str| k.rsplit_once('/').unwrap().0.to_string();
    assert_eq!(prefix(&a.slide_keys[0]), prefix(&a.slide_keys[2]));
    assert_ne!(prefix(&a.slide_keys[0]), prefix(&b.slide_keys[0]));
    // The 12 hex digits.
    let p = prefix(&a.slide_keys[0]);
    let hex = p.strip_prefix("office-img://").unwrap();
    assert_eq!(hex.len(), 12);
    assert!(hex.chars().all(|c| c.is_ascii_hexdigit()));
    // Many calls never repeat.
    let mut seen = std::collections::HashSet::new();
    for _ in 0..2000 {
        assert!(seen.insert(slide_keys(1).remove(0)));
    }
}

#[test]
fn the_picture_markdown_of_a_text_view_that_does_not_fit_is_empty() {
    let slides = vec![SlideInfo {
        number: 1,
        title: String::new(),
        hidden: false,
    }];
    let keys = vec!["office-img://aaaaaaaaaaaa/slide-1.svg".to_string()];
    // No heading at all, two headings for one slide, mismatched key count.
    assert_eq!(picture_markdown("text", &slides, &keys, Lang::En), "");
    assert_eq!(
        picture_markdown("## a\n\n## b", &slides, &keys, Lang::En),
        ""
    );
    assert_eq!(picture_markdown("## a", &slides, &[], Lang::En), "");
    let ok = picture_markdown("## Slide 1\n\nbody", &slides, &keys, Lang::En);
    assert_eq!(
        ok,
        "## Slide 1\n\n![Slide 1](office-img://aaaaaaaaaaaa/slide-1.svg)"
    );
    // The alt text is safe Markdown whatever the title holds.
    let slides = vec![SlideInfo {
        number: 2,
        title: "a ](x) [b] *c*".into(),
        hidden: false,
    }];
    let s = picture_markdown("## Slide 2: t", &slides, &keys, Lang::En);
    assert_eq!(
        s,
        "## Slide 2: t\n\n![Slide 2: a x b c](office-img://aaaaaaaaaaaa/slide-1.svg)"
    );
    // Notes: only the block that starts a paragraph counts; the label inside text does not.
    let md = "## Slide 1\n\ntext > **Notes** here\n\n> **Notes**  \n> real  \n> second";
    let s = picture_markdown(md, &slides[..1], &keys, Lang::En);
    assert!(s.ends_with("\n\n> **Notes**  \n> real  \n> second"), "{s}");
    assert!(!s.contains("text >"));
}

#[test]
fn there_is_a_scene_for_every_slide_even_a_hidden_or_unreadable_one() {
    let d = notes_deck();
    let mut e = d.entries();
    // Slide 2's part is damaged: it is a bare heading and an empty, truncated scene.
    for (n, b) in e.iter_mut() {
        if n == "ppt/slides/slide2.xml" {
            *b = b"<p:sld".to_vec();
        }
    }
    let dir = tmp("pptxd");
    let refs: Vec<(&str, &[u8])> = e.iter().map(|(n, b)| (n.as_str(), b.as_slice())).collect();
    let p = write(&dir, "t.pptx", &deflated(&refs));
    let doc = load_presentation(&p, &DocOptions::default()).unwrap();
    assert_eq!(doc.slides.len(), 3);
    assert_eq!(doc.slide_scenes.len(), 3);
    assert!(doc.truncated);
    assert!(doc.slide_scenes[1].items.is_empty());
    assert!(doc.slide_scenes[1].truncated);
    assert_eq!(
        (doc.slide_scenes[1].width, doc.slide_scenes[1].height),
        (9_144_000.0, 6_858_000.0)
    );
    assert!(!doc.slide_scenes[2].items.is_empty());
}

#[test]
fn the_slide_size_is_the_scene_size() {
    let mut d = D::new(&tbox(0, 0, 10, 10, &para("x")));
    d.size = (12_192_000, 6_858_000);
    let sc = d.scene();
    assert_eq!((sc.width, sc.height), (12_192_000.0, 6_858_000.0));
}

// ---------------------------------------------------------------------------------------------
// budgets
// ---------------------------------------------------------------------------------------------

fn many_markers(n: usize) -> String {
    (0..n).map(|i| marker("000001", i as i64)).collect()
}

#[test]
fn too_many_items_set_truncated_on_the_scene_and_the_document() {
    let opts = DocOptions {
        max_slide_shapes: 10,
        ..DocOptions::default()
    };
    let d = D::new(&many_markers(30));
    let doc = d.load_with(&opts);
    assert!(doc.slide_scenes[0].truncated && doc.truncated);
    assert!(doc.slide_scenes[0].items.len() <= 10);
    // Master and layout shapes count against the same budget.
    let mut d = D::new(&many_markers(3));
    d.master_shapes = many_markers(8);
    let doc = d.load_with(&opts);
    assert!(doc.slide_scenes[0].truncated);
    let doc = D::new(&many_markers(5)).load_with(&opts);
    assert!(!doc.slide_scenes[0].truncated && !doc.truncated);
}

#[test]
fn groups_nest_twenty_deep_and_a_deeper_one_is_reported() {
    let nest = |depth: usize| {
        let mut s = marker("000001", 1);
        for _ in 0..depth {
            s = group((0, 0, 10, 10), (0, 0, 10, 10), "", &s);
        }
        s
    };
    let count = |sc: &sd::SlideScene| shapes_of(&sc.items).len();
    let at = D::new(&nest(20)).scene();
    assert_eq!(count(&at), 1);
    assert!(!at.truncated);
    let past = D::new(&nest(21)).scene();
    assert_eq!(count(&past), 0);
    assert!(past.truncated);
}

#[test]
fn text_over_the_per_slide_budget_is_cut_and_reported() {
    let big = "x".repeat(60_000);
    let d = D::new(
        &(tbox(0, 0, 100, 100, &para(&big))
            + &tbox(0, 0, 100, 100, &para(&big))
            + &tbox(0, 0, 100, 100, &para("tail"))),
    );
    let sc = d.scene();
    assert!(sc.truncated);
    let total: usize = shapes_of(&sc.items)
        .iter()
        .flat_map(|s| s.text.iter())
        .flat_map(|t| t.paragraphs.iter())
        .flat_map(|p| p.runs.iter())
        .map(|r| r.text.chars().count())
        .sum();
    assert!(total <= text::MAX_TEXT_CHARS, "{total}");
    // Within the budget nothing is reported.
    let ok = D::new(&tbox(0, 0, 100, 100, &para(&"y".repeat(5000)))).scene();
    assert!(!ok.truncated);
}

#[test]
fn gradient_stops_and_custom_dashes_are_capped() {
    let stops: String = (0..200)
        .map(|i| {
            format!(
                r#"<a:gs pos="{}"><a:srgbClr val="FF0000"/></a:gs>"#,
                i * 500
            )
        })
        .collect();
    let d = D::new(&shape(
        "",
        &format!(
            r#"{}{RECT}<a:gradFill><a:gsLst>{stops}</a:gsLst><a:lin ang="0"/></a:gradFill>"#,
            xf(0, 0, 10, 10)
        ),
        "",
        "",
        "",
        "",
    ));
    let sc = d.scene();
    let Fill::Gradient(g) = &first_shape(&sc).fill else {
        panic!()
    };
    assert_eq!(g.stops.len(), style::MAX_GRAD_STOPS);
    assert!(sc.truncated);
    let ds: String = (0..100)
        .map(|_| r#"<a:ds d="100000" sp="100000"/>"#)
        .collect();
    let d = D::new(&shape(
        "",
        &format!(
            r#"{}{RECT}<a:ln w="1"><a:solidFill><a:srgbClr val="000000"/></a:solidFill><a:custDash>{ds}</a:custDash></a:ln>"#,
            xf(0, 0, 10, 10)
        ),
        "",
        "",
        "",
        "",
    ));
    let sc = d.scene();
    let sd::Dash::Custom(v) = &first_shape(&sc).line.as_ref().unwrap().dash else {
        panic!()
    };
    assert_eq!(v.len(), style::MAX_CUST_DASH);
    assert!(sc.truncated);
    // Colour transforms are capped too.
    let mods: String = (0..100).map(|_| r#"<a:lumMod val="99000"/>"#).collect();
    let n = elem(&format!(
        r#"<a:srgbClr xmlns:a="x" val="FFFFFF">{mods}</a:srgbClr>"#
    ));
    assert_eq!(theme::color_mods(&n).len(), theme::MAX_COLOR_MODS);
}

#[test]
fn a_layout_with_too_many_shapes_keeps_what_fits_and_reports() {
    let mut d = D::new(&tbox(0, 0, 10, 10, &para("x")));
    d.layout_shapes = many_markers(MAX_PART_SHAPES + 50);
    let doc = d.load_with(&DocOptions {
        max_slide_shapes: 10_000,
        ..DocOptions::default()
    });
    assert!(doc.slide_scenes[0].truncated);
    assert!(doc.truncated);
    assert!(doc.slide_scenes[0].items.len() > 100);
}

#[test]
fn the_scene_is_built_under_the_read_budgets_of_the_deck() {
    // The drawing reads no part the text view does not (plus the theme and the master's
    // relationships): a budget too small for them truncates and still gives a scene.
    let d = D::new(&tbox(0, 0, 10, 10, &para("x")));
    let doc = d.load_with(&DocOptions {
        max_pptx_read_total: 100,
        ..DocOptions::default()
    });
    assert!(doc.truncated);
    assert_eq!(doc.slide_scenes.len(), doc.slides.len());
}

#[test]
fn hostile_numbers_do_not_panic() {
    let big = i64::MAX;
    let xml = format!(
        r#"<p:sp><p:nvSpPr><p:cNvPr id="2" name="S"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm rot="{big}"><a:off x="{big}" y="-{big}"/><a:ext cx="-5" cy="{big}"/></a:xfrm><a:prstGeom prst="rect"><a:avLst><a:gd name="adj" fmla="val {big}"/></a:avLst></a:prstGeom><a:solidFill><a:srgbClr val="000000"/></a:solidFill><a:ln w="{big}"/></p:spPr><p:txBody><a:bodyPr lIns="{big}" numCol="{big}" rot="{big}"/><a:lstStyle/><a:p><a:pPr marL="{big}" indent="-{big}" lvl="{big}"><a:spcBef><a:spcPts val="{big}"/></a:spcBef></a:pPr><a:r><a:rPr sz="{big}" spc="{big}" baseline="{big}"/><a:t>x</a:t></a:r></a:p></p:txBody></p:sp>"#
    );
    let sc = D::new(&xml).scene();
    let s = first_shape(&sc);
    assert!(s.xfrm.x.is_finite() && s.xfrm.w >= 0.0);
    assert!(run0(s).size_pt <= 4000.0);
}

// ---------------------------------------------------------------------------------------------
// the text view is unchanged: the repository's decks against the goldens live in
// `tests_pptx_golden.rs`; here the decks must also draw something.
// ---------------------------------------------------------------------------------------------

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn the_decks_of_the_repository_have_one_scene_and_picture_per_slide() {
    for deck in [
        "testdata/office/slides.pptx",
        "testdata/office/slides-ja.pptx",
        "samples/sample.pptx",
        "samples/sample.ja.pptx",
    ] {
        for lang in [Lang::En, Lang::Jp] {
            let doc = load_presentation(
                &root().join(deck),
                &DocOptions {
                    lang,
                    ..DocOptions::default()
                },
            )
            .unwrap();
            assert_eq!(doc.slide_scenes.len(), doc.slides.len(), "{deck}");
            assert_eq!(doc.slide_keys.len(), doc.slides.len(), "{deck}");
            assert!(!doc.picture_markdown.is_empty(), "{deck}");
            let a: Vec<&str> = doc
                .markdown
                .lines()
                .filter(|l| l.starts_with("## "))
                .collect();
            let b: Vec<&str> = doc
                .picture_markdown
                .lines()
                .filter(|l| l.starts_with("## "))
                .collect();
            assert_eq!(a, b, "{deck}");
            assert!(
                doc.slide_scenes
                    .iter()
                    .all(|s| s.width > 0.0 && s.height > 0.0),
                "{deck}"
            );
            assert!(
                doc.slide_scenes.iter().any(|s| !s.items.is_empty()),
                "{deck}"
            );
            assert!(!doc.truncated, "{deck}");
        }
    }
}

#[test]
fn damaged_decks_never_panic() {
    // A scene is built from whatever the parts hold: every cut of a real deck loads or fails
    // cleanly.
    let bytes = std::fs::read(root().join("testdata/office/slides.pptx")).unwrap();
    let dir = tmp("pptxcut");
    for cut in [
        0usize,
        10,
        100,
        bytes.len() / 3,
        bytes.len() / 2,
        bytes.len() - 30,
    ] {
        let p = write(&dir, "cut.pptx", &bytes[..cut.min(bytes.len())]);
        let _ = load_presentation(&p, &DocOptions::default());
    }
    // Garbage in every XML part of a real package.
    let mut d = notes_deck();
    d.master_shapes = "<p:sp><a:oops>".into();
    let _ = d.load();
}
