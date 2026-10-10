//! Regression tests for what the whole-corpus ranking against LibreOffice found: group child
//! spaces (line widths and effects keep their size), legacy VML pictures of embedded objects,
//! ActiveX controls, a gradient with no stops, and the colour map of a chart.

use super::super::{load_presentation, DocOptions, Document};
use super::tests::{shape, shapes_of, xf, D, RECT};
use crate::preview::office::slide_draw::{self as sd, Fill, Item, Rgba};
use crate::preview::office::tests::{deflated, tmp, write};
use crate::preview::office::tests_docx::tiny_png;

/// The deck of `d` with `patch` applied to its parts (name, bytes) and extra parts added, loaded.
fn load_with_parts(
    d: &D,
    extra: &[(&str, Vec<u8>)],
    patch: &dyn Fn(&str, Vec<u8>) -> Vec<u8>,
) -> Document {
    let dir = tmp("pptxg1");
    let mut e = d.entries();
    for (n, b) in extra {
        e.push(((*n).to_string(), b.clone()));
    }
    let e: Vec<(String, Vec<u8>)> = e
        .into_iter()
        .map(|(n, b)| {
            let b = patch(&n, b);
            (n, b)
        })
        .collect();
    let refs: Vec<(&str, &[u8])> = e.iter().map(|(n, b)| (n.as_str(), b.as_slice())).collect();
    let p = write(&dir, "t.pptx", &deflated(&refs));
    load_presentation(&p, &DocOptions::default()).unwrap()
}

fn group(off: (i64, i64, i64, i64), ch: (i64, i64, i64, i64), inner: &str) -> String {
    format!(
        r#"<p:grpSp><p:nvGrpSpPr><p:cNvPr id="30" name="G"/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr><a:xfrm><a:off x="{}" y="{}"/><a:ext cx="{}" cy="{}"/><a:chOff x="{}" y="{}"/><a:chExt cx="{}" cy="{}"/></a:xfrm></p:grpSpPr>{inner}</p:grpSp>"#,
        off.0, off.1, off.2, off.3, ch.0, ch.1, ch.2, ch.3
    )
}

/// A red box of `w` x `h` at `x`, `y` with a 9525 EMU outline and a shadow of 38100 EMU.
fn boxed(x: i64, y: i64, w: i64, h: i64) -> String {
    shape(
        "",
        &format!(
            r#"{}{RECT}<a:solidFill><a:srgbClr val="FF0000"/></a:solidFill><a:ln w="9525"><a:solidFill><a:srgbClr val="000000"/></a:solidFill></a:ln><a:effectLst><a:outerShdw blurRad="50800" dist="38100" dir="2700000"><a:srgbClr val="000000"/></a:outerShdw></a:effectLst>"#,
            xf(x, y, w, h)
        ),
        "",
        "",
        "",
        "",
    )
}

#[test]
fn a_group_maps_its_members_but_not_their_line_widths_or_shadows() {
    // Child space 10 x 10 on a box of 3000 x 4000: the factors are 300 and 400. (A group of the
    // file of Apache POI bug 63200 maps 735058 x 544514 onto 10923834 x 5538047: a scale in the
    // drawing multiplied the 12700 EMU outline by 15 and gave the shape a thick black outline.)
    let d = D::new(&group(
        (1000, 2000, 3000, 4000),
        (0, 0, 10, 10),
        &boxed(1, 2, 2, 3),
    ));
    let sc = d.scene();
    let Item::Group(g) = &sc.items[0] else {
        panic!("{:?}", sc.items)
    };
    assert_eq!(g.xfrm, sd::Xfrm::rect(1000.0, 2000.0, 3000.0, 4000.0));
    // The members already sit in the box: the drawn group does not scale.
    assert_eq!(g.child_off, (1000.0, 2000.0));
    assert_eq!(g.child_ext, (3000.0, 4000.0));
    let s = shapes_of(&sc.items)[0];
    assert_eq!(
        (s.xfrm.x, s.xfrm.y, s.xfrm.w, s.xfrm.h),
        (1300.0, 2800.0, 600.0, 1200.0)
    );
    assert_eq!(s.line.as_ref().unwrap().width, 9525.0);
    assert_eq!(s.effects.outer_shadow.as_ref().unwrap().dist, 38100.0);
    assert_eq!(s.effects.outer_shadow.as_ref().unwrap().blur_rad, 50800.0);
}

#[test]
fn nested_groups_map_through_each_other() {
    // Outer: child space 100 x 100 onto 1000 x 1000 at (500, 500) (factor 10). Inner: its box
    // (10, 20, 30, 40) of that space, with its own child space 6 x 8 (factors 5 / 5 of that box).
    let inner = group((10, 20, 30, 40), (0, 0, 6, 8), &boxed(1, 2, 2, 2));
    let d = D::new(&group((500, 500, 1000, 1000), (0, 0, 100, 100), &inner));
    let sc = d.scene();
    let Item::Group(g) = &sc.items[0] else {
        panic!()
    };
    let Item::Group(g2) = &g.items[0] else {
        panic!()
    };
    // The inner box on the slide: (500 + 10 * 10, 500 + 20 * 10, 300, 400).
    assert_eq!(g2.xfrm, sd::Xfrm::rect(600.0, 700.0, 300.0, 400.0));
    assert_eq!(g2.child_off, (600.0, 700.0));
    assert_eq!(g2.child_ext, (300.0, 400.0));
    // The member: x = 600 + 1 * 50, y = 700 + 2 * 50, size 100 x 100.
    let s = shapes_of(&sc.items)[0];
    assert_eq!(
        (s.xfrm.x, s.xfrm.y, s.xfrm.w, s.xfrm.h),
        (650.0, 800.0, 100.0, 100.0)
    );
}

#[test]
fn a_group_keeps_the_rotation_and_flips_of_its_box() {
    let g = format!(
        r#"<p:grpSp><p:nvGrpSpPr><p:cNvPr id="3" name="G"/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr><a:xfrm rot="5400000" flipH="1"><a:off x="0" y="0"/><a:ext cx="200" cy="100"/><a:chOff x="0" y="0"/><a:chExt cx="20" cy="10"/></a:xfrm></p:grpSpPr>{}</p:grpSp>"#,
        boxed(2, 1, 4, 3)
    );
    let sc = D::new(&g).scene();
    let Item::Group(g) = &sc.items[0] else {
        panic!()
    };
    assert_eq!((g.xfrm.rot_deg, g.xfrm.flip_h), (90.0, true));
    let s = shapes_of(&sc.items)[0];
    assert_eq!(
        (s.xfrm.x, s.xfrm.y, s.xfrm.w, s.xfrm.h),
        (20.0, 10.0, 40.0, 30.0)
    );
    // (A member's own rotation and flips are the member's.)
    assert_eq!((s.xfrm.rot_deg, s.xfrm.flip_h), (0.0, false));
}

#[test]
fn a_group_with_a_zero_child_extent_maps_onto_itself_and_a_flat_box_collapses_its_members() {
    let g = group((10, 20, 300, 400), (10, 20, 0, 0), &boxed(10, 20, 30, 40));
    let sc = D::new(&g).scene();
    let s = shapes_of(&sc.items)[0];
    assert_eq!(
        (s.xfrm.x, s.xfrm.y, s.xfrm.w, s.xfrm.h),
        (10.0, 20.0, 30.0, 40.0)
    );
    // A group box of no width puts every member's width at 0 on that axis (no NaN, no panic).
    let g = group((10, 20, 0, 400), (0, 0, 10, 10), &boxed(1, 1, 5, 5));
    let sc = D::new(&g).scene();
    let s = shapes_of(&sc.items)[0];
    assert_eq!((s.xfrm.x, s.xfrm.w), (10.0, 0.0));
    assert_eq!(s.xfrm.h, 200.0);
}

#[test]
fn a_preset_inside_a_group_is_evaluated_at_its_final_size() {
    // The corner radius of a rounded rectangle follows the final box, not the child space.
    let rr = shape(
        "",
        &format!(
            r#"{}<a:prstGeom prst="roundRect"><a:avLst><a:gd name="adj" fmla="val 50000"/></a:avLst></a:prstGeom><a:solidFill><a:srgbClr val="00FF00"/></a:solidFill>"#,
            xf(0, 0, 10, 10)
        ),
        "",
        "",
        "",
        "",
    );
    let sc = D::new(&group((0, 0, 1000, 1000), (0, 0, 10, 10), &rr)).scene();
    let s = shapes_of(&sc.items)[0];
    assert_eq!((s.xfrm.w, s.xfrm.h), (1000.0, 1000.0));
    let sd::Geometry::Paths(ps) = &s.geom else {
        panic!("{:?}", s.geom)
    };
    // The paths are evaluated for 1000 x 1000: some coordinate reaches the 500 radius.
    let mut max = 0.0f64;
    for p in ps {
        for c in &p.cmds {
            if let sd::PathCmd::MoveTo(q) | sd::PathCmd::LineTo(q) = c {
                max = max.max(q.x).max(q.y);
            }
        }
    }
    assert!(max >= 499.0, "paths still in the child space: {max}");
}

// ---------------------------------------------------------------------------------------------
// embedded objects with a legacy VML picture, ActiveX controls
// ---------------------------------------------------------------------------------------------

const VML: &str = r##"<xml xmlns:v="urn:schemas-microsoft-com:vml" xmlns:o="urn:schemas-microsoft-com:office:office"><v:shapetype id="_x0000_t75"/><v:shape id="_x0000_s17410" type="#_x0000_t75" style='position:absolute'>
  <v:imagedata o:relid="rId1" o:title=""/>
 </v:shape><v:shape id="_x0000_s17412" type="#_x0000_t75"><v:imagedata o:relid="rId2"/></v:shape></xml>"##;

fn vml_rels() -> Vec<u8> {
    br#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="../media/v1.png"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="../media/v2.png"/></Relationships>"#.to_vec()
}

fn ole_frame(spid: &str, x: i64) -> String {
    format!(
        r#"<p:graphicFrame><p:nvGraphicFramePr><p:cNvPr id="4" name="Obj"/><p:cNvGraphicFramePr/><p:nvPr/></p:nvGraphicFramePr><p:xfrm><a:off x="{x}" y="20"/><a:ext cx="300" cy="400"/></p:xfrm><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/presentationml/2006/ole"><p:oleObj spid="{spid}" name="Doc" r:id="rIdO" imgW="1" imgH="1" progId="Word.Document.8"><p:embed/></p:oleObj></a:graphicData></a:graphic></p:graphicFrame>"#
    )
}

fn vml_deck(frames: &str) -> D {
    let mut d = D::new(frames);
    d.slides[0].rels.push((
        "rIdV".into(),
        "vmlDrawing".into(),
        "../drawings/vmlDrawing1.vml".into(),
    ));
    d
}

fn vml_parts() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        ("ppt/drawings/vmlDrawing1.vml", VML.as_bytes().to_vec()),
        ("ppt/drawings/_rels/vmlDrawing1.vml.rels", vml_rels()),
        ("ppt/media/v1.png", tiny_png(1)),
        ("ppt/media/v2.png", tiny_png(2)),
    ]
}

#[test]
fn an_embedded_object_without_a_picture_is_drawn_as_the_vml_picture_of_its_shape_id() {
    let d = vml_deck(&(ole_frame("_x0000_s17410", 10) + &ole_frame("_x0000_s17412", 500)));
    let doc = load_with_parts(&d, &vml_parts(), &|_, b| b);
    let sc = &doc.slide_scenes[0];
    let pics: Vec<&sd::PictureItem> = sc
        .items
        .iter()
        .filter_map(|i| match i {
            Item::Picture(p) => Some(p),
            _ => None,
        })
        .collect();
    assert_eq!(pics.len(), 2, "{:?}", sc.items);
    assert_eq!((pics[0].xfrm.x, pics[0].xfrm.w), (10.0, 300.0));
    assert_eq!((pics[1].xfrm.x, pics[1].xfrm.w), (500.0, 300.0));
    // Two different pictures, both loaded into the document.
    assert_ne!(pics[0].image.key, pics[1].image.key);
    for p in &pics {
        assert!(doc.images.iter().any(|i| i.key == p.image.key));
    }
}

#[test]
fn an_embedded_object_whose_vml_picture_is_missing_draws_nothing() {
    // No `vmlDrawing` relationship, a shape id the drawing does not have, a relid it cannot
    // resolve.
    let none = D::new(&ole_frame("_x0000_s17410", 10));
    assert!(load_with_parts(&none, &[], &|_, b| b).slide_scenes[0]
        .items
        .is_empty());
    let other = vml_deck(&ole_frame("_x0000_s999", 10));
    assert!(
        load_with_parts(&other, &vml_parts(), &|_, b| b).slide_scenes[0]
            .items
            .is_empty()
    );
    let mut parts = vml_parts();
    parts[1].1 =
        br#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"/>"#
            .to_vec();
    let lost = vml_deck(&ole_frame("_x0000_s17410", 10));
    assert!(load_with_parts(&lost, &parts, &|_, b| b).slide_scenes[0]
        .items
        .is_empty());
}

#[test]
fn an_activex_control_is_drawn_as_its_fallback_picture_over_the_shapes() {
    let mut d = D::new(&shape(
        "",
        &format!(
            "{}{RECT}<a:solidFill><a:srgbClr val=\"0000FF\"/></a:solidFill>",
            xf(0, 0, 50, 50)
        ),
        "",
        "",
        "",
        "",
    ));
    d.media.push(("c1.png".into(), tiny_png(1)));
    d.slides[0]
        .rels
        .push(("rIdP".into(), "image".into(), "../media/c1.png".into()));
    let controls = r#"</p:spTree><p:controls><mc:AlternateContent xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006"><mc:Choice xmlns:v="urn:schemas-microsoft-com:vml" Requires="v"><p:control spid="1140" name="B1" r:id="rIdX" imgW="1" imgH="1"/></mc:Choice><mc:Fallback><p:control name="B1" r:id="rIdX" imgW="1" imgH="1"><p:pic><p:nvPicPr><p:cNvPr id="8" name="B1"/><p:cNvPicPr><a:picLocks/></p:cNvPicPr><p:nvPr/></p:nvPicPr><p:blipFill><a:blip r:embed="rIdP"/><a:stretch><a:fillRect/></a:stretch></p:blipFill><p:spPr><a:xfrm><a:off x="100" y="200"/><a:ext cx="300" cy="400"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></p:spPr></p:pic></p:control></mc:Fallback></mc:AlternateContent></p:controls>"#;
    let doc = load_with_parts(&d, &[], &|n, b| {
        if n == "ppt/slides/slide1.xml" {
            String::from_utf8(b)
                .unwrap()
                .replacen("</p:spTree>", controls, 1)
                .into_bytes()
        } else {
            b
        }
    });
    let sc = &doc.slide_scenes[0];
    // The shape first, the control's picture after it (the controls are over the shape tree).
    assert!(matches!(&sc.items[0], Item::Shape(_)), "{:?}", sc.items);
    let Item::Picture(p) = &sc.items[1] else {
        panic!("{:?}", sc.items)
    };
    assert_eq!(
        (p.xfrm.x, p.xfrm.y, p.xfrm.w, p.xfrm.h),
        (100.0, 200.0, 300.0, 400.0)
    );
    assert!(doc.images.iter().any(|i| i.key == p.image.key));
    assert_eq!(sc.items.len(), 2);
}

// ---------------------------------------------------------------------------------------------
// a gradient with no stops
// ---------------------------------------------------------------------------------------------

#[test]
fn a_gradient_without_stops_gives_way_to_the_fill_of_the_style() {
    let style = r#"<p:style><a:lnRef idx="0"><a:schemeClr val="accent1"/></a:lnRef><a:fillRef idx="1"><a:schemeClr val="accent1"/></a:fillRef><a:effectRef idx="0"><a:schemeClr val="accent1"/></a:effectRef><a:fontRef idx="minor"/></p:style>"#;
    let s = shape(
        "",
        &format!(
            r#"{}{RECT}<a:gradFill flip="none" rotWithShape="1"><a:lin ang="2700000" scaled="1"/><a:tileRect/></a:gradFill>"#,
            xf(0, 0, 10, 10)
        ),
        "",
        "",
        "",
        style,
    );
    let sc = D::new(&s).scene();
    assert_eq!(
        shapes_of(&sc.items)[0].fill,
        Fill::Solid(Rgba::rgb(0x44, 0x72, 0xC4))
    );
    // With stops it is the gradient itself.
    let with = s.replace(
        "<a:lin ",
        r#"<a:gsLst><a:gs pos="0"><a:srgbClr val="FF0000"/></a:gs><a:gs pos="100000"><a:srgbClr val="0000FF"/></a:gs></a:gsLst><a:lin "#,
    );
    let sc = D::new(&with).scene();
    assert!(matches!(shapes_of(&sc.items)[0].fill, Fill::Gradient(_)));
}

// ---------------------------------------------------------------------------------------------
// the colour map of a chart
// ---------------------------------------------------------------------------------------------

fn pie_chart(space: &str) -> Vec<u8> {
    format!(
        r#"<c:chartSpace xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><c:chart><c:plotArea><c:barChart><c:barDir val="col"/><c:grouping val="clustered"/><c:ser><c:idx val="0"/><c:order val="0"/><c:spPr><a:solidFill><a:schemeClr val="bg1"><a:lumMod val="85000"/></a:schemeClr></a:solidFill></c:spPr><c:cat><c:strLit><c:ptCount val="1"/><c:pt idx="0"><c:v>a</c:v></c:pt></c:strLit></c:cat><c:val><c:numLit><c:ptCount val="1"/><c:pt idx="0"><c:v>3</c:v></c:pt></c:numLit></c:val></c:ser><c:axId val="1"/><c:axId val="2"/></c:barChart><c:catAx><c:axId val="1"/><c:scaling><c:orientation val="minMax"/></c:scaling><c:delete val="0"/><c:axPos val="b"/><c:crossAx val="2"/></c:catAx><c:valAx><c:axId val="2"/><c:scaling><c:orientation val="minMax"/></c:scaling><c:delete val="0"/><c:axPos val="l"/><c:crossAx val="1"/></c:valAx></c:plotArea></c:chart>{space}</c:chartSpace>"#
    )
    .into_bytes()
}

fn chart_on_dark_slide(chart: Vec<u8>) -> sd::SlideScene {
    let mut d = D::new(
        r#"<p:graphicFrame><p:nvGraphicFramePr><p:cNvPr id="4" name="F"/><p:cNvGraphicFramePr/><p:nvPr/></p:nvGraphicFramePr><p:xfrm><a:off x="0" y="0"/><a:ext cx="4000000" cy="3000000"/></p:xfrm><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/chart"><c:chart r:id="rIdC"/></a:graphicData></a:graphic></p:graphicFrame>"#,
    );
    d.slides[0]
        .rels
        .push(("rIdC".into(), "chart".into(), "../charts/chart1.xml".into()));
    // The slide maps bg1 to dk1 (a dark variant of the slide).
    d.slides[0].ovr = r#"<p:clrMapOvr><a:overrideClrMapping bg1="dk1" tx1="lt1" bg2="dk2" tx2="lt2" accent1="accent1" accent2="accent2" accent3="accent3" accent4="accent4" accent5="accent5" accent6="accent6" hlink="hlink" folHlink="folHlink"/></p:clrMapOvr>"#.into();
    load_with_parts(&d, &[("ppt/charts/chart1.xml", chart)], &|_, b| b).slide_scenes[0].clone()
}

#[test]
fn a_chart_uses_the_masters_colour_map_not_the_slides_override() {
    // LibreOffice's testTdf153012 (a PowerPoint file): "bg1 is mapped in the slide to dk1, but in
    // the chart to lt1": the data point of `bg1` darker by 15 % is D9D9D9.
    let sc = chart_on_dark_slide(pie_chart(""));
    let g = match &sc.items[0] {
        Item::Group(g) => g,
        o => panic!("{o:?}"),
    };
    let grey = Rgba::rgb(0xD9, 0xD9, 0xD9);
    assert!(
        shapes_of(&g.items)
            .iter()
            .any(|s| matches!(&s.fill, Fill::Solid(c) if *c == grey)),
        "no D9D9D9 bar"
    );
    // The slide itself keeps its own map: the background is the dark one.
    assert_eq!(sc.background, Fill::Solid(Rgba::rgb(0, 0, 0)));
}

#[test]
fn a_chart_with_its_own_colour_map_override_uses_it() {
    let ovr = r#"<c:clrMapOvr bg1="dk1" tx1="lt1" bg2="dk2" tx2="lt2" accent1="accent1" accent2="accent2" accent3="accent3" accent4="accent4" accent5="accent5" accent6="accent6" hlink="hlink" folHlink="folHlink"/>"#;
    let sc = chart_on_dark_slide(pie_chart(ovr));
    let Item::Group(g) = &sc.items[0] else {
        panic!()
    };
    // dk1 darkened... lumMod 85 % of black is black.
    assert!(
        shapes_of(&g.items)
            .iter()
            .any(|s| matches!(&s.fill, Fill::Solid(c) if *c == Rgba::rgb(0, 0, 0))),
        "no black bar"
    );
}
