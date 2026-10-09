//! Tests of charts, SmartArt and embedded objects on slides (`frames.rs`): hand-built packages,
//! one path at a time.

use super::super::{load_presentation, DocOptions, Document};
use super::frames::text_box;
use super::tests::{shapes_of, xf, D};
use super::*;
use crate::preview::office::slide_draw::{self as sd, Fill, Item, Rgba};
use crate::preview::office::tests::{deflated, tmp, write};
use crate::preview::office::tests_docx::tiny_png;
use crate::preview::office::tests_pptx::PNS;

const NS_C: &str = r#"xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main""#;

/// The deck of `d` with extra parts, loaded.
fn load(d: &D, extra: &[(&str, Vec<u8>)], opts: &DocOptions) -> Document {
    let dir = tmp("pptxf");
    let mut e = d.entries();
    for (n, b) in extra {
        e.push(((*n).to_string(), b.clone()));
    }
    let refs: Vec<(&str, &[u8])> = e.iter().map(|(n, b)| (n.as_str(), b.as_slice())).collect();
    let p = write(&dir, "t.pptx", &deflated(&refs));
    load_presentation(&p, opts).unwrap()
}

fn frame(uri: &str, inner: &str, xfrm: &str) -> String {
    format!(
        r#"<p:graphicFrame><p:nvGraphicFramePr><p:cNvPr id="4" name="F"/><p:cNvGraphicFramePr/><p:nvPr/></p:nvGraphicFramePr><p:xfrm>{xfrm}</p:xfrm><a:graphic><a:graphicData uri="{uri}">{inner}</a:graphicData></a:graphic></p:graphicFrame>"#
    )
}

fn box_xfrm(x: i64, y: i64, w: i64, h: i64) -> String {
    format!(r#"<a:off x="{x}" y="{y}"/><a:ext cx="{w}" cy="{h}"/>"#)
}

const CHART_URI: &str = "http://schemas.openxmlformats.org/drawingml/2006/chart";
const DGM_URI: &str = "http://schemas.openxmlformats.org/drawingml/2006/diagram";

fn chart_frame(x: i64, y: i64, w: i64, h: i64) -> String {
    frame(
        CHART_URI,
        r#"<c:chart r:id="rIdC"/>"#,
        &box_xfrm(x, y, w, h),
    )
}

/// A one-series column chart; `space` is put in `c:chartSpace` after `c:chart`, `ser` in the
/// series.
fn chart_xml(space: &str, ser: &str) -> Vec<u8> {
    format!(
        r#"<c:chartSpace {NS_C}><c:chart><c:plotArea><c:barChart><c:barDir val="col"/><c:grouping val="clustered"/><c:ser><c:idx val="0"/><c:order val="0"/><c:tx><c:strRef><c:f>x</c:f><c:strCache><c:ptCount val="1"/><c:pt idx="0"><c:v>Sales</c:v></c:pt></c:strCache></c:strRef></c:tx>{ser}<c:cat><c:strRef><c:f>c</c:f><c:strCache><c:ptCount val="2"/><c:pt idx="0"><c:v>North</c:v></c:pt><c:pt idx="1"><c:v>South</c:v></c:pt></c:strCache></c:strRef></c:cat><c:val><c:numRef><c:f>v</c:f><c:numCache><c:formatCode>General</c:formatCode><c:ptCount val="2"/><c:pt idx="0"><c:v>3</c:v></c:pt><c:pt idx="1"><c:v>5</c:v></c:pt></c:numCache></c:numRef></c:val></c:ser><c:axId val="1"/><c:axId val="2"/></c:barChart><c:catAx><c:axId val="1"/><c:scaling><c:orientation val="minMax"/></c:scaling><c:delete val="0"/><c:axPos val="b"/><c:crossAx val="2"/></c:catAx><c:valAx><c:axId val="2"/><c:scaling><c:orientation val="minMax"/></c:scaling><c:delete val="0"/><c:axPos val="l"/><c:crossAx val="1"/></c:valAx></c:plotArea></c:chart>{space}</c:chartSpace>"#
    )
    .into_bytes()
}

fn chart_deck(frame_xml: &str) -> D {
    let mut d = D::new(frame_xml);
    d.slides[0]
        .rels
        .push(("rIdC".into(), "chart".into(), "../charts/chart1.xml".into()));
    d
}

fn the_group(sc: &sd::SlideScene) -> &sd::GroupItem {
    match &sc.items[0] {
        Item::Group(g) => g,
        other => panic!("not a group: {other:?}"),
    }
}

fn texts(items: &[Item]) -> Vec<String> {
    shapes_of(items)
        .iter()
        .filter_map(|s| s.text.as_ref())
        .flat_map(|t| t.paragraphs.iter())
        .flat_map(|p| p.runs.iter())
        .map(|r| r.text.clone())
        .collect()
}

// ---------------------------------------------------------------------------------------------
// charts
// ---------------------------------------------------------------------------------------------

#[test]
fn a_chart_is_drawn_in_a_group_at_the_frame() {
    let d = chart_deck(&chart_frame(100_000, 200_000, 4_000_000, 3_000_000));
    let doc = load(
        &d,
        &[("ppt/charts/chart1.xml", chart_xml("", ""))],
        &DocOptions::default(),
    );
    let sc = &doc.slide_scenes[0];
    assert_eq!(sc.items.len(), 1);
    let g = the_group(sc);
    assert_eq!(
        g.xfrm,
        sd::Xfrm::rect(100_000.0, 200_000.0, 4_000_000.0, 3_000_000.0)
    );
    assert_eq!(g.child_off, (0.0, 0.0));
    assert_eq!(g.child_ext, (4_000_000.0, 3_000_000.0));
    assert!(!g.items.is_empty());
    // The category labels are the chart's text.
    let t = texts(&g.items);
    assert!(t.iter().any(|s| s == "North"), "{t:?}");
    assert!(t.iter().any(|s| s == "South"), "{t:?}");
    assert!(!sc.truncated);
    // Everything stays inside the frame.
    for s in shapes_of(&g.items) {
        assert!(
            s.xfrm.x >= -1.0 && s.xfrm.x + s.xfrm.w <= 4_000_001.0,
            "{:?}",
            s.xfrm
        );
    }
}

#[test]
fn a_chart_takes_the_themes_accents() {
    // accent1 4472C4 is the first series colour of the theme.
    let d = chart_deck(&chart_frame(0, 0, 4_000_000, 3_000_000));
    let doc = load(
        &d,
        &[("ppt/charts/chart1.xml", chart_xml("", ""))],
        &DocOptions::default(),
    );
    let g = the_group(&doc.slide_scenes[0]);
    let blue = Rgba::rgb(0x44, 0x72, 0xC4);
    assert!(
        shapes_of(&g.items)
            .iter()
            .any(|s| matches!(&s.fill, Fill::Solid(c) if *c == blue)),
        "no accent1 bar"
    );
}

#[test]
fn chart_colours_go_through_the_slides_colour_map() {
    // The map turns bg1 into dk1 (black): the chart area fill `bg1` is black.
    let mut d = chart_deck(&chart_frame(0, 0, 4_000_000, 3_000_000));
    d.master_map = r#"<p:clrMap bg1="dk1" tx1="lt1" bg2="dk2" tx2="lt2" accent1="accent1" accent2="accent2" accent3="accent3" accent4="accent4" accent5="accent5" accent6="accent6" hlink="hlink" folHlink="folHlink"/>"#.into();
    let space = r#"<c:spPr><a:solidFill><a:schemeClr val="bg1"/></a:solidFill></c:spPr>"#;
    let doc = load(
        &d,
        &[("ppt/charts/chart1.xml", chart_xml(space, ""))],
        &DocOptions::default(),
    );
    let g = the_group(&doc.slide_scenes[0]);
    let area = shapes_of(&g.items)
        .into_iter()
        .find(|s| s.xfrm.w >= 3_999_000.0 && s.xfrm.h >= 2_999_000.0)
        .expect("chart area");
    assert_eq!(area.fill, Fill::Solid(Rgba::rgb(0, 0, 0)));
    // And with the identity map it is white.
    let mut d = chart_deck(&chart_frame(0, 0, 4_000_000, 3_000_000));
    d.master_map = D::new("").master_map;
    let doc = load(
        &d,
        &[("ppt/charts/chart1.xml", chart_xml(space, ""))],
        &DocOptions::default(),
    );
    let g = the_group(&doc.slide_scenes[0]);
    let area = shapes_of(&g.items)
        .into_iter()
        .find(|s| s.xfrm.w >= 3_999_000.0 && s.xfrm.h >= 2_999_000.0)
        .expect("chart area");
    assert_eq!(area.fill, Fill::Solid(Rgba::WHITE));
}

#[test]
fn a_chart_series_colour_can_be_a_scheme_colour_with_transforms() {
    let d = chart_deck(&chart_frame(0, 0, 4_000_000, 3_000_000));
    let ser = r#"<c:spPr><a:solidFill><a:schemeClr val="accent2"/></a:solidFill></c:spPr>"#;
    let doc = load(
        &d,
        &[("ppt/charts/chart1.xml", chart_xml("", ser))],
        &DocOptions::default(),
    );
    let g = the_group(&doc.slide_scenes[0]);
    let orange = Rgba::rgb(0xED, 0x7D, 0x31);
    assert!(shapes_of(&g.items)
        .iter()
        .any(|s| matches!(&s.fill, Fill::Solid(c) if *c == orange)));
}

#[test]
fn a_frame_rotation_is_not_applied_to_a_chart() {
    // PowerPoint never rotates a graphic frame; the group is upright whatever `rot` says.
    let f = frame(
        CHART_URI,
        r#"<c:chart r:id="rIdC"/>"#,
        r#"<a:off x="0" y="0"/><a:ext cx="4000000" cy="3000000"/>"#,
    )
    .replace("<p:xfrm>", r#"<p:xfrm rot="5400000" flipH="1">"#);
    let d = chart_deck(&f);
    let doc = load(
        &d,
        &[("ppt/charts/chart1.xml", chart_xml("", ""))],
        &DocOptions::default(),
    );
    let g = the_group(&doc.slide_scenes[0]);
    assert_eq!(g.xfrm.rot_deg, 0.0);
    assert!(!g.xfrm.flip_h);
}

#[test]
fn a_chart_that_does_not_parse_draws_nothing_and_is_not_a_truncation() {
    for bytes in [
        b"not xml at all".to_vec(),
        b"<c:chartSpace".to_vec(),
        Vec::new(),
    ] {
        let d = chart_deck(&chart_frame(0, 0, 4_000_000, 3_000_000));
        let doc = load(
            &d,
            &[("ppt/charts/chart1.xml", bytes)],
            &DocOptions::default(),
        );
        assert!(doc.slide_scenes[0].items.is_empty());
        assert!(!doc.slide_scenes[0].truncated);
    }
}

#[test]
fn a_chart_whose_part_is_missing_or_whose_relationship_is_unknown_draws_nothing() {
    let d = chart_deck(&chart_frame(0, 0, 4_000_000, 3_000_000));
    let doc = load(&d, &[], &DocOptions::default());
    assert!(doc.slide_scenes[0].items.is_empty());
    // No `r:id`, an unknown one, and an external one.
    for inner in [
        "<c:chart/>",
        r#"<c:chart r:id="rIdNone"/>"#,
        r#"<c:chart r:id="rIdX"/>"#,
    ] {
        let mut d = D::new(&frame(
            CHART_URI,
            inner,
            &box_xfrm(0, 0, 4_000_000, 3_000_000),
        ));
        d.slides[0].rels.push((
            "rIdX".into(),
            "chart".into(),
            "http://example.com/c.xml".into(),
        ));
        let doc = load(
            &d,
            &[("ppt/charts/chart1.xml", chart_xml("", ""))],
            &DocOptions::default(),
        );
        assert!(doc.slide_scenes[0].items.is_empty(), "{inner}");
    }
}

#[test]
fn a_chart_in_a_frame_of_no_size_draws_nothing() {
    for (w, h) in [(0, 3_000_000), (4_000_000, 0)] {
        let d = chart_deck(&chart_frame(0, 0, w, h));
        let doc = load(
            &d,
            &[("ppt/charts/chart1.xml", chart_xml("", ""))],
            &DocOptions::default(),
        );
        assert!(doc.slide_scenes[0].items.is_empty());
    }
}

#[test]
fn a_chartex_chart_in_alternate_content_draws_its_fallback_picture() {
    let png = tiny_png(7);
    let inner = format!(
        r#"<mc:AlternateContent><mc:Choice Requires="cx1">{}</mc:Choice><mc:Fallback><p:pic><p:nvPicPr><p:cNvPr id="5" name="Fb"/><p:cNvPicPr/><p:nvPr/></p:nvPicPr><p:blipFill><a:blip r:embed="rIdFb"/></p:blipFill><p:spPr>{}<a:prstGeom prst="rect"><a:avLst/></a:prstGeom></p:spPr></p:pic></mc:Fallback></mc:AlternateContent>"#,
        frame(
            "http://schemas.microsoft.com/office/drawing/2014/chartex",
            r#"<cx:chart xmlns:cx="http://schemas.microsoft.com/office/drawing/2014/chartex" r:id="rIdC"/>"#,
            &box_xfrm(10, 20, 3_000_000, 2_000_000)
        ),
        xf(10, 20, 3_000_000, 2_000_000)
    );
    let mut d = chart_deck(&inner);
    d.media.push(("fb.png".into(), png));
    d.slides[0]
        .rels
        .push(("rIdFb".into(), "image".into(), "../media/fb.png".into()));
    let doc = load(
        &d,
        &[("ppt/charts/chart1.xml", b"<cx:chartSpace/>".to_vec())],
        &DocOptions::default(),
    );
    let sc = &doc.slide_scenes[0];
    assert_eq!(sc.items.len(), 1, "{:?}", sc.items);
    let Item::Picture(p) = &sc.items[0] else {
        panic!("not the fallback picture: {:?}", sc.items[0])
    };
    assert_eq!(p.xfrm.w, 3_000_000.0);
    assert_ne!(p.image.key, shapes::MISSING_PICTURE);
}

#[test]
fn a_chartex_frame_without_a_fallback_draws_nothing() {
    let inner = frame(
        "http://schemas.microsoft.com/office/drawing/2014/chartex",
        r#"<cx:chart xmlns:cx="http://schemas.microsoft.com/office/drawing/2014/chartex" r:id="rIdC"/>"#,
        &box_xfrm(0, 0, 3_000_000, 2_000_000),
    );
    let d = chart_deck(&inner);
    let cx = br#"<cx:chartSpace xmlns:cx="http://schemas.microsoft.com/office/drawing/2014/chartex"><cx:chart/></cx:chartSpace>"#;
    let doc = load(
        &d,
        &[("ppt/charts/chart1.xml", cx.to_vec())],
        &DocOptions::default(),
    );
    assert!(doc.slide_scenes[0].items.is_empty());
    assert!(!doc.slide_scenes[0].truncated);
}

#[test]
fn a_chart_over_the_part_budget_is_not_read_and_the_document_is_truncated() {
    let mut big = chart_xml("", "");
    big.extend(std::iter::repeat_n(b' ', 30_000));
    let d = chart_deck(&chart_frame(0, 0, 4_000_000, 3_000_000));
    let opts = DocOptions {
        max_slide_part_bytes: 20_000,
        ..DocOptions::default()
    };
    let doc = load(&d, &[("ppt/charts/chart1.xml", big)], &opts);
    assert!(doc.slide_scenes[0].items.is_empty());
    assert!(doc.truncated);
}

#[test]
fn chart_shapes_count_toward_the_slide_item_budget() {
    let d = chart_deck(&chart_frame(0, 0, 4_000_000, 3_000_000));
    let opts = DocOptions {
        max_slide_shapes: 4,
        ..DocOptions::default()
    };
    let doc = load(&d, &[("ppt/charts/chart1.xml", chart_xml("", ""))], &opts);
    let sc = &doc.slide_scenes[0];
    assert!(sc.truncated);
    assert!(doc.truncated);
    assert!(shapes_of(&the_group(sc).items).len() <= 4);
}

// ---------------------------------------------------------------------------------------------
// SmartArt
// ---------------------------------------------------------------------------------------------

fn dgm_frame(x: i64, y: i64, w: i64, h: i64) -> String {
    frame(
        DGM_URI,
        r#"<dgm:relIds r:dm="rIdDm" r:lo="rIdLo" r:qs="rIdQs" r:cs="rIdCs"/>"#,
        &box_xfrm(x, y, w, h),
    )
}

fn dgm_data(rel_id: Option<&str>) -> Vec<u8> {
    let ext = rel_id.map_or(String::new(), |r| {
        format!(
            r#"<a:extLst><a:ext uri="http://schemas.microsoft.com/office/drawing/2008/diagram"><dsp:dataModelExt relId="{r}" minVer="http://schemas.openxmlformats.org/drawingml/2006/diagram"/></a:ext></a:extLst>"#
        )
    });
    format!(
        r#"<dgm:dataModel {PNS}><dgm:ptLst/><dgm:cxnLst/><dgm:bg/><dgm:whole/>{ext}</dgm:dataModel>"#,
    )
    .into_bytes()
}

/// A `dsp:sp` of a rectangle with a fill and a text.
fn dsp_sp(
    (x, y, w, h): (i64, i64, i64, i64),
    extra_xfrm: &str,
    fill: &str,
    tx_xfrm: &str,
    text: &str,
) -> String {
    format!(
        r#"<dsp:sp modelId="{{1}}"><dsp:nvSpPr><dsp:cNvPr id="0" name=""/><dsp:cNvSpPr/></dsp:nvSpPr><dsp:spPr><a:xfrm{extra_xfrm}><a:off x="{x}" y="{y}"/><a:ext cx="{w}" cy="{h}"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom>{fill}</dsp:spPr><dsp:style><a:lnRef idx="0"><a:scrgbClr r="0" g="0" b="0"/></a:lnRef><a:fillRef idx="0"><a:scrgbClr r="0" g="0" b="0"/></a:fillRef><a:effectRef idx="0"><a:scrgbClr r="0" g="0" b="0"/></a:effectRef><a:fontRef idx="minor"><a:schemeClr val="lt1"/></a:fontRef></dsp:style><dsp:txBody><a:bodyPr lIns="0" tIns="0" rIns="0" bIns="0"/><a:lstStyle/><a:p><a:r><a:rPr lang="en-US" sz="1800"/><a:t>{text}</a:t></a:r></a:p></dsp:txBody>{tx_xfrm}</dsp:sp>"#
    )
}

fn dsp_drawing(shapes: &str) -> Vec<u8> {
    format!(
        r#"<dsp:drawing {PNS}><dsp:spTree><dsp:nvGrpSpPr><dsp:cNvPr id="0" name=""/><dsp:cNvGrpSpPr/></dsp:nvGrpSpPr><dsp:grpSpPr/>{shapes}</dsp:spTree></dsp:drawing>"#,
    )
    .into_bytes()
}

fn dgm_deck(frame_xml: &str) -> D {
    let mut d = D::new(frame_xml);
    for (id, ty, t) in [
        ("rIdDm", "diagramData", "../diagrams/data1.xml"),
        ("rIdLo", "diagramLayout", "../diagrams/layout1.xml"),
        ("rIdQs", "diagramQuickStyle", "../diagrams/quickStyle1.xml"),
        ("rIdCs", "diagramColors", "../diagrams/colors1.xml"),
        ("rIdDr", "diagramDrawing", "../diagrams/drawing1.xml"),
    ] {
        d.slides[0].rels.push((id.into(), ty.into(), t.into()));
    }
    d
}

fn solid(hex: &str) -> String {
    format!(r#"<a:solidFill><a:srgbClr val="{hex}"/></a:solidFill>"#)
}

#[test]
fn smartart_shapes_come_from_the_saved_drawing_in_the_frames_space() {
    let d = dgm_deck(&dgm_frame(1_000_000, 500_000, 6_000_000, 2_000_000));
    let shapes = dsp_sp(
        ((0), (0), (2_000_000), (1_000_000)),
        "",
        &solid("FF0000"),
        "",
        "One",
    ) + &dsp_sp(
        ((3_000_000), (500_000), (2_000_000), (1_000_000)),
        "",
        &solid("00FF00"),
        "",
        "Two",
    );
    let doc = load(
        &d,
        &[
            ("ppt/diagrams/data1.xml", dgm_data(Some("rIdDr"))),
            ("ppt/diagrams/drawing1.xml", dsp_drawing(&shapes)),
        ],
        &DocOptions::default(),
    );
    let sc = &doc.slide_scenes[0];
    let g = the_group(sc);
    assert_eq!(
        g.xfrm,
        sd::Xfrm::rect(1_000_000.0, 500_000.0, 6_000_000.0, 2_000_000.0)
    );
    assert_eq!(g.child_off, (0.0, 0.0));
    assert_eq!(g.child_ext, (6_000_000.0, 2_000_000.0));
    let s = shapes_of(&g.items);
    assert_eq!(s.len(), 2);
    assert_eq!(s[0].fill, Fill::Solid(Rgba::rgb(255, 0, 0)));
    assert_eq!(s[1].xfrm.x, 3_000_000.0);
    assert_eq!(texts(&g.items), ["One", "Two"]);
    assert!(!sc.truncated);
}

#[test]
fn a_smartart_text_box_replaces_the_shapes_own() {
    // txXfrm: the right half of a 2,000,000 x 1,000,000 shape at (500,000, 100,000).
    let tx = r#"<dsp:txXfrm><a:off x="1500000" y="100000"/><a:ext cx="1000000" cy="1000000"/></dsp:txXfrm>"#;
    let shapes = dsp_sp(
        ((500_000), (100_000), (2_000_000), (1_000_000)),
        "",
        &solid("112233"),
        tx,
        "Half",
    );
    let d = dgm_deck(&dgm_frame(0, 0, 4_000_000, 2_000_000));
    let doc = load(
        &d,
        &[
            ("ppt/diagrams/data1.xml", dgm_data(Some("rIdDr"))),
            ("ppt/diagrams/drawing1.xml", dsp_drawing(&shapes)),
        ],
        &DocOptions::default(),
    );
    let g = the_group(&doc.slide_scenes[0]);
    let s = shapes_of(&g.items)[0];
    let (l, t, r, b) = s.text_rect.expect("text rect");
    assert!(
        (l - 1_000_000.0).abs() < 1.0 && (r - 2_000_000.0).abs() < 1.0,
        "{l} {r}"
    );
    assert!(t.abs() < 1.0 && (b - 1_000_000.0).abs() < 1.0, "{t} {b}");
    assert_eq!(s.text.as_ref().unwrap().rot_deg, 0.0);
}

#[test]
fn a_smartart_shape_without_a_text_box_keeps_the_whole_shape() {
    let shapes = dsp_sp(
        ((0), (0), (2_000_000), (1_000_000)),
        "",
        &solid("112233"),
        "",
        "All",
    );
    let d = dgm_deck(&dgm_frame(0, 0, 4_000_000, 2_000_000));
    let doc = load(
        &d,
        &[
            ("ppt/diagrams/data1.xml", dgm_data(Some("rIdDr"))),
            ("ppt/diagrams/drawing1.xml", dsp_drawing(&shapes)),
        ],
        &DocOptions::default(),
    );
    let s = shapes_of(&the_group(&doc.slide_scenes[0]).items)[0].clone();
    // No txXfrm: the text area is the shape's own (the rect preset's text rectangle is the whole box).
    assert!(
        s.text_rect.is_none() || s.text_rect == Some((0.0, 0.0, 2_000_000.0, 1_000_000.0)),
        "{:?}",
        s.text_rect
    );
}

#[test]
fn text_box_of_a_rotated_shape_lands_where_the_text_box_says() {
    // A square shape turned 90 degrees clockwise about its centre; the text box (upright) is
    // the strip over its top half in the drawing's space.
    let shape = sd::Xfrm {
        rot_deg: 90.0,
        ..sd::Xfrm::rect(0.0, 0.0, 2_000_000.0, 2_000_000.0)
    };
    let tx = sd::Xfrm {
        rot_deg: -90.0,
        ..sd::Xfrm::rect(0.0, 0.0, 2_000_000.0, 1_000_000.0)
    };
    let (rect, turn) = text_box(&shape, &tx);
    // The text box centre (1e6, 5e5) is 5e5 above the shape centre; undoing a 90 degree clockwise
    // turn puts that offset 5e5 to the left of the centre in the shape's own box.
    let (cx, cy) = ((rect.0 + rect.2) / 2.0, (rect.1 + rect.3) / 2.0);
    assert!(
        (cx - 500_000.0).abs() < 1.0 && (cy - 1_000_000.0).abs() < 1.0,
        "{cx} {cy}"
    );
    assert!((rect.2 - rect.0 - 2_000_000.0).abs() < 1.0);
    assert!((rect.3 - rect.1 - 1_000_000.0).abs() < 1.0);
    // `txXfrm@rot` is relative to the shape: the text turns by exactly that.
    assert!((turn + 90.0).abs() < 1e-9, "{turn}");
}

#[test]
fn text_box_of_a_flipped_shape_is_mirrored_back() {
    let shape = sd::Xfrm {
        flip_h: true,
        ..sd::Xfrm::rect(0.0, 0.0, 2_000_000.0, 1_000_000.0)
    };
    // Text box over the shape's right half on the page: in the (mirrored) shape box it is the left.
    let tx = sd::Xfrm::rect(1_000_000.0, 0.0, 1_000_000.0, 1_000_000.0);
    let (rect, turn) = text_box(&shape, &tx);
    assert!(
        rect.0.abs() < 1.0 && (rect.2 - 1_000_000.0).abs() < 1.0,
        "{rect:?}"
    );
    assert_eq!(turn, 0.0);
    let shape = sd::Xfrm {
        flip_v: true,
        ..sd::Xfrm::rect(0.0, 0.0, 1_000_000.0, 2_000_000.0)
    };
    let tx = sd::Xfrm::rect(0.0, 1_000_000.0, 1_000_000.0, 1_000_000.0);
    let (rect, _) = text_box(&shape, &tx);
    assert!(
        rect.1.abs() < 1.0 && (rect.3 - 1_000_000.0).abs() < 1.0,
        "{rect:?}"
    );
}

#[test]
fn a_smartart_text_box_rotation_is_relative_to_the_shape() {
    let tx = r#"<dsp:txXfrm rot="-5400000"><a:off x="0" y="0"/><a:ext cx="1000000" cy="1000000"/></dsp:txXfrm>"#;
    let shapes = dsp_sp(
        ((0), (0), (1_000_000), (1_000_000)),
        r#" rot="5400000""#,
        &solid("112233"),
        tx,
        "Up",
    );
    let d = dgm_deck(&dgm_frame(0, 0, 2_000_000, 2_000_000));
    let doc = load(
        &d,
        &[
            ("ppt/diagrams/data1.xml", dgm_data(Some("rIdDr"))),
            ("ppt/diagrams/drawing1.xml", dsp_drawing(&shapes)),
        ],
        &DocOptions::default(),
    );
    let s = shapes_of(&the_group(&doc.slide_scenes[0]).items)[0].clone();
    assert_eq!(s.xfrm.rot_deg, 90.0);
    assert!((s.text.as_ref().unwrap().rot_deg + 90.0).abs() < 1e-9);
}

/// A case of a deck without a usable drawing: name, data model part, other parts.
type Case = (&'static str, Option<Vec<u8>>, Vec<(&'static str, Vec<u8>)>);

#[test]
fn smartart_without_a_usable_drawing_draws_nothing() {
    let drawing = dsp_drawing(&dsp_sp(
        ((0), (0), (1000), (1000)),
        "",
        &solid("FF0000"),
        "",
        "x",
    ));
    let cases: [Case; 5] = [
        // The data model names no drawing (LibreOffice writes none).
        (
            "no ext",
            Some(dgm_data(None)),
            vec![("ppt/diagrams/drawing1.xml", drawing.clone())],
        ),
        // It names a relationship the slide does not have.
        (
            "unknown relId",
            Some(dgm_data(Some("rIdNope"))),
            vec![("ppt/diagrams/drawing1.xml", drawing.clone())],
        ),
        // The drawing part is not in the package.
        ("missing part", Some(dgm_data(Some("rIdDr"))), vec![]),
        // The data part is not in the package.
        (
            "missing data",
            None,
            vec![("ppt/diagrams/drawing1.xml", drawing.clone())],
        ),
        // The drawing is not a drawing.
        (
            "wrong root",
            Some(dgm_data(Some("rIdDr"))),
            vec![(
                "ppt/diagrams/drawing1.xml",
                b"<x:y xmlns:x=\"u\"/>".to_vec(),
            )],
        ),
    ];
    for (name, data, mut extra) in cases {
        let d = dgm_deck(&dgm_frame(0, 0, 4_000_000, 2_000_000));
        if let Some(data) = data {
            extra.push(("ppt/diagrams/data1.xml", data));
        }
        let doc = load(&d, &extra, &DocOptions::default());
        assert!(doc.slide_scenes[0].items.is_empty(), "{name}");
        assert!(!doc.slide_scenes[0].truncated, "{name}");
    }
}

#[test]
fn smartart_whose_drawing_rel_is_another_kind_is_not_used() {
    // `relId` must name a diagramDrawing relationship, not any part.
    let mut d = dgm_deck(&dgm_frame(0, 0, 4_000_000, 2_000_000));
    d.slides[0].rels.push((
        "rIdOther".into(),
        "image".into(),
        "../diagrams/drawing1.xml".into(),
    ));
    let drawing = dsp_drawing(&dsp_sp(
        ((0), (0), (1000), (1000)),
        "",
        &solid("FF0000"),
        "",
        "x",
    ));
    let doc = load(
        &d,
        &[
            ("ppt/diagrams/data1.xml", dgm_data(Some("rIdOther"))),
            ("ppt/diagrams/drawing1.xml", drawing),
        ],
        &DocOptions::default(),
    );
    assert!(doc.slide_scenes[0].items.is_empty());
}

#[test]
fn smartart_in_a_frame_of_no_size_or_without_relids_draws_nothing() {
    let drawing = dsp_drawing(&dsp_sp(
        ((0), (0), (1000), (1000)),
        "",
        &solid("FF0000"),
        "",
        "x",
    ));
    let extra = [
        ("ppt/diagrams/data1.xml", dgm_data(Some("rIdDr"))),
        ("ppt/diagrams/drawing1.xml", drawing),
    ];
    let d = dgm_deck(&dgm_frame(0, 0, 0, 2_000_000));
    assert!(load(&d, &extra, &DocOptions::default()).slide_scenes[0]
        .items
        .is_empty());
    let d = dgm_deck(&frame(DGM_URI, "<dgm:other/>", &box_xfrm(0, 0, 1000, 1000)));
    assert!(load(&d, &extra, &DocOptions::default()).slide_scenes[0]
        .items
        .is_empty());
    // A `dm` that is not a relationship of the slide.
    let d = dgm_deck(&frame(
        DGM_URI,
        r#"<dgm:relIds r:dm="rIdZ"/>"#,
        &box_xfrm(0, 0, 1000, 1000),
    ));
    assert!(load(&d, &extra, &DocOptions::default()).slide_scenes[0]
        .items
        .is_empty());
}

#[test]
fn smartart_pictures_resolve_through_the_drawings_own_relationships() {
    // In the drawing, rId1 is pic-a; in the slide, rId1 is pic-b: the drawing's wins.
    let pic = |rid: &str| {
        format!(
            r#"<dsp:sp modelId="{{2}}"><dsp:nvSpPr><dsp:cNvPr id="0" name=""/><dsp:cNvSpPr/></dsp:nvSpPr><dsp:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="1000" cy="1000"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom><a:blipFill><a:blip r:embed="{rid}"/></a:blipFill></dsp:spPr></dsp:sp>"#
        )
    };
    let mut d = dgm_deck(&dgm_frame(0, 0, 4_000_000, 2_000_000));
    d.media.push(("pic-a.png".into(), tiny_png(1)));
    d.media.push(("pic-b.png".into(), tiny_png(2)));
    d.slides[0]
        .rels
        .push(("rId1".into(), "image".into(), "../media/pic-b.png".into()));
    let rels = br#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="../media/pic-a.png"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="http://example.com/x.png" TargetMode="External"/></Relationships>"#;
    let doc = load(
        &d,
        &[
            ("ppt/diagrams/data1.xml", dgm_data(Some("rIdDr"))),
            (
                "ppt/diagrams/drawing1.xml",
                dsp_drawing(&(pic("rId1") + &pic("rId2") + &pic("rId9"))),
            ),
            ("ppt/diagrams/_rels/drawing1.xml.rels", rels.to_vec()),
        ],
        &DocOptions::default(),
    );
    let g = the_group(&doc.slide_scenes[0]);
    let keys: Vec<String> = shapes_of(&g.items)
        .iter()
        .map(|s| match &s.fill {
            Fill::Image(i) => i.key.clone(),
            other => format!("{other:?}"),
        })
        .collect();
    // (A shape whose picture cannot be found has nothing to show and is left out, like any
    // shape of the slide.)
    assert_eq!(keys.len(), 1, "{keys:?}");
    let name_of = |k: &str| {
        doc.images
            .iter()
            .find(|i| i.key == k)
            .map(|i| i.name.clone())
    };
    // The drawing's rId1 is pic-a, not the slide's pic-b; the external and the unknown id
    // never fall back to the slide's relationships.
    assert_eq!(name_of(&keys[0]).as_deref(), Some("pic-a.png"));
}

#[test]
fn smartart_shapes_count_toward_the_slide_item_budget() {
    let shapes: String = (0..10)
        .map(|i| {
            dsp_sp(
                ((i * 1000), (0), (900), (900)),
                "",
                &solid("445566"),
                "",
                "s",
            )
        })
        .collect();
    let d = dgm_deck(&dgm_frame(0, 0, 40_000, 2_000));
    let opts = DocOptions {
        max_slide_shapes: 4,
        ..DocOptions::default()
    };
    let doc = load(
        &d,
        &[
            ("ppt/diagrams/data1.xml", dgm_data(Some("rIdDr"))),
            ("ppt/diagrams/drawing1.xml", dsp_drawing(&shapes)),
        ],
        &opts,
    );
    let sc = &doc.slide_scenes[0];
    assert!(sc.truncated);
    // (The frame itself is the first item.)
    assert_eq!(shapes_of(&the_group(sc).items).len(), 3);
}

#[test]
fn a_smartart_drawing_over_the_node_budget_is_not_drawn_and_says_so() {
    // Each shape is about 25 elements: 6,000 of them are past the 100,000-element budget.
    let one = dsp_sp(((0), (0), (10), (10)), "", &solid("445566"), "", "s");
    let shapes: String = std::iter::repeat_n(one.as_str(), 6_000).collect();
    let d = dgm_deck(&dgm_frame(0, 0, 40_000, 2_000));
    let doc = load(
        &d,
        &[
            ("ppt/diagrams/data1.xml", dgm_data(Some("rIdDr"))),
            ("ppt/diagrams/drawing1.xml", dsp_drawing(&shapes)),
        ],
        &DocOptions::default(),
    );
    assert!(doc.slide_scenes[0].items.is_empty());
    assert!(doc.slide_scenes[0].truncated);
}

#[test]
fn a_smartart_drawing_part_is_read_once_per_frame_under_the_read_budget() {
    // The data model and the drawing are charged to the deck's total read budget.
    let shapes = dsp_sp(((0), (0), (1000), (1000)), "", &solid("FF0000"), "", "x");
    let d = dgm_deck(&dgm_frame(0, 0, 4_000_000, 2_000_000));
    let opts = DocOptions {
        max_pptx_read_total: 20_000,
        ..DocOptions::default()
    };
    let mut big = dgm_data(Some("rIdDr"));
    big.extend(std::iter::repeat_n(b' ', 30_000));
    let doc = load(
        &d,
        &[
            ("ppt/diagrams/data1.xml", big),
            ("ppt/diagrams/drawing1.xml", dsp_drawing(&shapes)),
        ],
        &opts,
    );
    assert!(doc.slide_scenes[0].items.is_empty());
    assert!(doc.truncated);
}

// ---------------------------------------------------------------------------------------------
// embedded objects
// ---------------------------------------------------------------------------------------------

#[test]
fn an_embedded_object_is_drawn_as_its_preview_picture() {
    let ole = format!(
        r#"<p:graphicFrame><p:nvGraphicFramePr><p:cNvPr id="4" name="Obj"/><p:cNvGraphicFramePr/><p:nvPr/></p:nvGraphicFramePr><p:xfrm>{}</p:xfrm><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/presentationml/2006/ole"><mc:AlternateContent><mc:Choice Requires="v"><p:oleObj spid="_x0000_s1026" name="Worksheet" r:id="rIdO" imgW="100" imgH="100" progId="Excel.Sheet.12"><p:embed/></p:oleObj></mc:Choice><mc:Fallback><p:oleObj name="Worksheet" r:id="rIdO" imgW="100" imgH="100" progId="Excel.Sheet.12"><p:embed/><p:pic><p:nvPicPr><p:cNvPr id="0" name=""/><p:cNvPicPr/><p:nvPr/></p:nvPicPr><p:blipFill><a:blip r:embed="rIdImg"/><a:stretch><a:fillRect/></a:stretch></p:blipFill><p:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="1" cy="1"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></p:spPr></p:pic></p:oleObj></mc:Fallback></mc:AlternateContent></a:graphicData></a:graphic></p:graphicFrame>"#,
        box_xfrm(111, 222, 3_000_000, 1_500_000)
    );
    let mut d = D::new(&ole);
    d.media.push(("preview.png".into(), tiny_png(3)));
    d.slides[0].rels.push((
        "rIdImg".into(),
        "image".into(),
        "../media/preview.png".into(),
    ));
    let doc = load(&d, &[], &DocOptions::default());
    let sc = &doc.slide_scenes[0];
    assert_eq!(sc.items.len(), 1, "{:?}", sc.items);
    let Item::Picture(p) = &sc.items[0] else {
        panic!("{:?}", sc.items[0])
    };
    assert_eq!(p.xfrm.x, 111.0);
    assert_eq!(p.xfrm.w, 3_000_000.0);
    assert_eq!(
        doc.images
            .iter()
            .find(|i| i.key == p.image.key)
            .unwrap()
            .name,
        "preview.png"
    );
}

// ---------------------------------------------------------------------------------------------
// the chart part's own relationships, the chart's colour map, and the fallback of a failed Choice
// ---------------------------------------------------------------------------------------------

const CHART_RELS: &str = r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rIdP" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="../media/bg.png"/></Relationships>"#;

fn image_fills(items: &[Item]) -> Vec<String> {
    shapes_of(items)
        .iter()
        .filter_map(|s| match &s.fill {
            Fill::Image(i) => Some(i.key.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn a_picture_fill_in_a_chart_resolves_through_the_chart_parts_relationships() {
    let sp = r#"<c:spPr><a:blipFill xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><a:blip r:embed="rIdP"/><a:stretch><a:fillRect/></a:stretch></a:blipFill></c:spPr>"#;
    let mut d = chart_deck(&chart_frame(0, 0, 4_000_000, 3_000_000));
    d.media.push(("bg.png".into(), tiny_png(3)));
    let doc = load(
        &d,
        &[
            ("ppt/charts/chart1.xml", chart_xml(sp, "")),
            (
                "ppt/charts/_rels/chart1.xml.rels",
                CHART_RELS.as_bytes().to_vec(),
            ),
        ],
        &DocOptions::default(),
    );
    let g = the_group(&doc.slide_scenes[0]);
    let keys = image_fills(&g.items);
    assert_eq!(keys.len(), 1, "{keys:?}");
    assert_ne!(keys[0], shapes::MISSING_PICTURE);
    assert!(doc.images.iter().any(|i| i.key == keys[0]));
}

#[test]
fn a_chart_picture_fill_does_not_resolve_through_the_slides_relationships() {
    // `rIdP` exists on the slide but the chart part has no relationship of that id.
    let sp = r#"<c:spPr><a:blipFill xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><a:blip r:embed="rIdP"/></a:blipFill></c:spPr>"#;
    let mut d = chart_deck(&chart_frame(0, 0, 4_000_000, 3_000_000));
    d.media.push(("bg.png".into(), tiny_png(3)));
    d.slides[0]
        .rels
        .push(("rIdP".into(), "image".into(), "../media/bg.png".into()));
    let doc = load(
        &d,
        &[("ppt/charts/chart1.xml", chart_xml(sp, ""))],
        &DocOptions::default(),
    );
    let g = the_group(&doc.slide_scenes[0]);
    assert!(image_fills(&g.items).is_empty());
}

#[test]
fn a_chart_with_its_own_colour_map_does_not_use_the_slides() {
    // The chart area is filled with `bg1`; the chart's own map says bg1 = dk1 (black in the
    // default theme), which is not what the slide's map (bg1 = lt1) gives.
    let fill = r#"<c:spPr><a:solidFill><a:schemeClr val="bg1"/></a:solidFill></c:spPr>"#;
    let over = format!(
        r#"{fill}<c:clrMapOvr><a:overrideClrMapping bg1="dk1" tx1="lt1" bg2="dk2" tx2="lt2" accent1="accent1" accent2="accent2" accent3="accent3" accent4="accent4" accent5="accent5" accent6="accent6" hlink="hlink" folHlink="folHlink"/></c:clrMapOvr>"#
    );
    let colour_of = |space: &str| {
        let d = chart_deck(&chart_frame(0, 0, 4_000_000, 3_000_000));
        let doc = load(
            &d,
            &[("ppt/charts/chart1.xml", chart_xml(space, ""))],
            &DocOptions::default(),
        );
        let g = the_group(&doc.slide_scenes[0]);
        let first = shapes_of(&g.items)[0].fill.clone();
        first
    };
    let slide_map = colour_of(fill);
    let chart_map = colour_of(&over);
    assert_ne!(slide_map, chart_map);
    assert_eq!(chart_map, Fill::Solid(Rgba::BLACK));
}

/// A graphic frame in `mc:AlternateContent` whose Choice holds a chart and whose Fallback a picture.
fn chart_with_fallback(requires: &str) -> String {
    format!(
        r#"<mc:AlternateContent><mc:Choice Requires="{requires}">{}</mc:Choice><mc:Fallback><p:pic><p:nvPicPr><p:cNvPr id="5" name="Fb"/><p:cNvPicPr/><p:nvPr/></p:nvPicPr><p:blipFill><a:blip r:embed="rIdFb"/></p:blipFill><p:spPr>{}<a:prstGeom prst="rect"><a:avLst/></a:prstGeom></p:spPr></p:pic></mc:Fallback></mc:AlternateContent>"#,
        chart_frame(10, 20, 3_000_000, 2_000_000),
        xf(10, 20, 3_000_000, 2_000_000)
    )
}

fn fallback_deck(requires: &str) -> D {
    let mut d = chart_deck(&chart_with_fallback(requires));
    d.media.push(("fb.png".into(), tiny_png(7)));
    d.slides[0]
        .rels
        .push(("rIdFb".into(), "image".into(), "../media/fb.png".into()));
    d
}

#[test]
fn a_choice_chart_that_does_not_parse_gives_way_to_the_fallback_picture() {
    let d = fallback_deck("a14");
    let doc = load(
        &d,
        &[("ppt/charts/chart1.xml", b"<not a chart".to_vec())],
        &DocOptions::default(),
    );
    let sc = &doc.slide_scenes[0];
    assert_eq!(sc.items.len(), 1, "{:?}", sc.items);
    assert!(
        matches!(&sc.items[0], Item::Picture(_)),
        "{:?}",
        sc.items[0]
    );
}

#[test]
fn a_choice_chart_that_is_missing_gives_way_to_the_fallback_picture() {
    let d = fallback_deck("a14");
    let doc = load(&d, &[], &DocOptions::default());
    assert!(matches!(&doc.slide_scenes[0].items[0], Item::Picture(_)));
}

#[test]
fn a_choice_chart_that_draws_keeps_the_choice_and_not_the_fallback() {
    let d = fallback_deck("a14");
    let doc = load(
        &d,
        &[("ppt/charts/chart1.xml", chart_xml("", ""))],
        &DocOptions::default(),
    );
    let sc = &doc.slide_scenes[0];
    assert_eq!(sc.items.len(), 1);
    assert!(matches!(&sc.items[0], Item::Group(_)), "{:?}", sc.items[0]);
}

const THEME_OVERRIDE_RELS: &str = r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/themeOverride" Target="../theme/themeOverride1.xml"/></Relationships>"#;

fn theme_override(accent1: &str, font: &str) -> Vec<u8> {
    format!(
        r#"<a:themeOverride xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><a:clrScheme name="o"><a:dk1><a:srgbClr val="000000"/></a:dk1><a:lt1><a:srgbClr val="FFFFFF"/></a:lt1><a:dk2><a:srgbClr val="1F497D"/></a:dk2><a:lt2><a:srgbClr val="EEECE1"/></a:lt2><a:accent1><a:srgbClr val="{accent1}"/></a:accent1><a:accent2><a:srgbClr val="C0504D"/></a:accent2><a:accent3><a:srgbClr val="9BBB59"/></a:accent3><a:accent4><a:srgbClr val="8064A2"/></a:accent4><a:accent5><a:srgbClr val="4BACC6"/></a:accent5><a:accent6><a:srgbClr val="F79646"/></a:accent6><a:hlink><a:srgbClr val="0000FF"/></a:hlink><a:folHlink><a:srgbClr val="800080"/></a:folHlink></a:clrScheme><a:fontScheme name="o"><a:majorFont><a:latin typeface="{font}"/><a:ea typeface=""/><a:cs typeface=""/></a:majorFont><a:minorFont><a:latin typeface="{font}"/><a:ea typeface=""/><a:cs typeface=""/></a:minorFont></a:fontScheme></a:themeOverride>"#
    )
    .into_bytes()
}

fn has_fill(g: &sd::GroupItem, c: Rgba) -> bool {
    shapes_of(&g.items)
        .iter()
        .any(|s| matches!(&s.fill, Fill::Solid(x) if *x == c))
}

#[test]
fn a_chart_with_a_theme_override_takes_its_colours_not_the_slides() {
    // The slide's theme gives accent1 4472C4; the chart's own theme says FF0000.
    let d = chart_deck(&chart_frame(0, 0, 4_000_000, 3_000_000));
    let doc = load(
        &d,
        &[
            ("ppt/charts/chart1.xml", chart_xml("", "")),
            (
                "ppt/charts/_rels/chart1.xml.rels",
                THEME_OVERRIDE_RELS.as_bytes().to_vec(),
            ),
            (
                "ppt/theme/themeOverride1.xml",
                theme_override("FF0000", "Arial"),
            ),
        ],
        &DocOptions::default(),
    );
    let g = the_group(&doc.slide_scenes[0]);
    assert!(
        has_fill(g, Rgba::rgb(0xFF, 0, 0)),
        "override accent1 missing"
    );
    assert!(
        !has_fill(g, Rgba::rgb(0x44, 0x72, 0xC4)),
        "slide accent1 used"
    );
}

#[test]
fn a_chart_whose_theme_override_part_is_missing_keeps_the_slides_theme() {
    let d = chart_deck(&chart_frame(0, 0, 4_000_000, 3_000_000));
    let doc = load(
        &d,
        &[
            ("ppt/charts/chart1.xml", chart_xml("", "")),
            (
                "ppt/charts/_rels/chart1.xml.rels",
                THEME_OVERRIDE_RELS.as_bytes().to_vec(),
            ),
        ],
        &DocOptions::default(),
    );
    let g = the_group(&doc.slide_scenes[0]);
    assert!(has_fill(g, Rgba::rgb(0x44, 0x72, 0xC4)));
}

#[test]
fn a_chart_without_a_theme_override_is_unchanged() {
    let d = chart_deck(&chart_frame(0, 0, 4_000_000, 3_000_000));
    let doc = load(
        &d,
        &[("ppt/charts/chart1.xml", chart_xml("", ""))],
        &DocOptions::default(),
    );
    let g = the_group(&doc.slide_scenes[0]);
    assert!(has_fill(g, Rgba::rgb(0x44, 0x72, 0xC4)));
    assert!(!has_fill(g, Rgba::rgb(0xFF, 0, 0)));
}
