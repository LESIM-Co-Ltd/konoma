//! Preset and custom geometry through the reader, connectors with line ends, symbol-font bullets,
//! picture clips and metafile pictures.

use super::*;

fn render(sc: &sd::SlideScene) -> (String, image::RgbaImage) {
    let r = sd::render_svg(sc, &|_| None);
    let img = crate::preview::svg::rasterize_trusted(
        r.svg.as_bytes(),
        std::path::Path::new("/slide.svg"),
        960,
    )
    .expect("the slide rasterizes")
    .to_rgba8();
    (r.svg, img)
}

fn px(img: &image::RgbaImage, x: u32, y: u32) -> [u8; 3] {
    let p = img.get_pixel(x, y).0;
    [p[0], p[1], p[2]]
}

fn sp_with(geom: &str, extra: &str, w: i64, h: i64, paras: &str) -> String {
    shape(
        "",
        &format!(
            r#"{}{geom}<a:solidFill><a:srgbClr val="000000"/></a:solidFill>{extra}"#,
            xf(0, 0, w, h)
        ),
        "",
        "",
        paras,
        "",
    )
}

fn prst(name: &str, av: &str) -> String {
    format!(r#"<a:prstGeom prst="{name}"><a:avLst>{av}</a:avLst></a:prstGeom>"#)
}

#[test]
fn a_rounded_rectangle_is_evaluated_and_its_text_rectangle_comes_with_it() {
    let (w, h) = (1_000_000i64, 500_000i64);
    let d = D::new(&sp_with(&prst("roundRect", ""), "", w, h, &para("x")));
    let sc = d.scene();
    let s = first_shape(&sc);
    let sd::Geometry::Paths(p) = &s.geom else {
        panic!("{:?}", s.geom)
    };
    assert_eq!(p.len(), 1);
    // adj = 16667: the corner radius is 1/6 of the short side (path coordinates in EMU).
    let r = 500_000.0 * 0.16667;
    assert!(matches!(
        p[0].cmds[0],
        sd::PathCmd::MoveTo(pt) if pt.x == 0.0 && (pt.y - r).abs() < 10.0
    ));
    // The text rectangle is inset by r * (1 - cos 45).
    let (l, t, rr, b) = s.text_rect.expect("the preset has a text rectangle");
    let inset = r * 0.29289;
    assert!(
        (l - inset).abs() < 10.0 && (t - inset).abs() < 10.0,
        "{l} {t}"
    );
    assert!((rr - (w as f64 - inset)).abs() < 10.0 && (b - (h as f64 - inset)).abs() < 10.0);
    // An adjust value changes the shape.
    let d = D::new(&sp_with(
        &prst("roundRect", r#"<a:gd name="adj" fmla="val 50000"/>"#),
        "",
        w,
        h,
        &para("x"),
    ));
    let sc = d.scene();
    let sd::Geometry::Paths(p2) = &first_shape(&sc).geom else {
        panic!()
    };
    assert_ne!(p2, p);
}

#[test]
fn an_ellipse_keeps_its_form_and_gets_the_inscribed_text_rectangle() {
    let d = D::new(&sp_with(
        &prst("ellipse", ""),
        "",
        1_000_000,
        1_000_000,
        &para("x"),
    ));
    let sc = d.scene();
    let s = first_shape(&sc);
    assert_eq!(s.geom, sd::Geometry::Ellipse);
    let (l, t, r, b) = s.text_rect.unwrap();
    assert!((l - 146_447.0).abs() < 100.0 && (t - l).abs() < 1.0);
    assert!((r - 853_553.0).abs() < 100.0 && (b - r).abs() < 1.0);
    // A rectangle keeps the box and its (whole-box) text rectangle.
    let d = D::new(&sp_with(
        &prst("rect", ""),
        "",
        1_000_000,
        1_000_000,
        &para("x"),
    ));
    let sc = d.scene();
    assert_eq!(first_shape(&sc).geom, sd::Geometry::Rect);
}

#[test]
fn the_text_of_a_preset_is_laid_out_in_its_text_rectangle() {
    // The ellipse's text rectangle starts 14.6 % in; red text on the black shape.
    let d = D::new(&sp_with(
        &prst("ellipse", ""),
        "",
        6_000_000,
        3_000_000,
        r#"<a:p><a:r><a:rPr lang="en-US"><a:solidFill><a:srgbClr val="FF0000"/></a:solidFill></a:rPr><a:t>Hello</a:t></a:r></a:p>"#,
    ));
    let sc = d.scene();
    let s = first_shape(&sc);
    assert!(s.text_rect.unwrap().0 > 800_000.0);
    let (svg, img) = render(&sc);
    assert!(svg.contains("Hello"));
    // 6_000_000 EMU = 630 px: the text's red pixels start right of the inscribed rectangle's left
    // edge (92 px) plus the 7.2 pt inset.
    let first_red_x = (0..630)
        .find(|&x| (0..315).any(|y| is_red(px(&img, x, y))))
        .expect("red text");
    assert!(first_red_x > 92 + 9, "{first_red_x}");
}

#[test]
fn a_custom_geometry_is_evaluated_with_its_guides_and_text_rectangle() {
    let cust = r#"<a:custGeom><a:avLst/><a:gdLst><a:gd name="half" fmla="*/ w 1 2"/></a:gdLst><a:rect l="0" t="0" r="half" b="b"/><a:pathLst><a:path w="100" h="100"><a:moveTo><a:pt x="0" y="100"/></a:moveTo><a:lnTo><a:pt x="50" y="0"/></a:lnTo><a:lnTo><a:pt x="100" y="100"/></a:lnTo><a:close/></a:path></a:pathLst></a:custGeom>"#;
    let d = D::new(&sp_with(cust, "", 2_000_000, 1_000_000, &para("x")));
    let sc = d.scene();
    let s = first_shape(&sc);
    let sd::Geometry::Paths(p) = &s.geom else {
        panic!("{:?}", s.geom)
    };
    assert_eq!(p.len(), 1);
    assert_eq!((p[0].w, p[0].h), (100.0, 100.0));
    assert_eq!(p[0].cmds.len(), 4);
    assert_eq!(s.text_rect, Some((0.0, 0.0, 1_000_000.0, 1_000_000.0)));
    // It draws a triangle: apex at the top middle, empty top corners.
    let (_, img) = render(&sc);
    // 2_000_000 EMU = 210 px wide, 105 px tall
    assert!(px(&img, 105, 80)[0] < 40, "{:?}", px(&img, 105, 80));
    assert_eq!(px(&img, 5, 5), [255, 255, 255]);
}

#[test]
fn a_custom_geometry_with_too_many_guides_is_the_box() {
    let mut gd = String::new();
    for i in 0..4200 {
        gd.push_str(&format!(r#"<a:gd name="g{i}" fmla="val {i}"/>"#));
    }
    let cust = format!(r#"<a:custGeom><a:avLst/><a:gdLst>{gd}</a:gdLst><a:pathLst/></a:custGeom>"#);
    let d = D::new(&sp_with(&cust, "", 1_000_000, 1_000_000, &para("x")));
    let sc = d.scene();
    // (The reader may also have cut the shape's tree short; what it keeps is the box.)
    assert!(
        sc.truncated || first_shape(&sc).geom == sd::Geometry::Rect,
        "{:?}",
        first_shape(&sc).geom
    );
}

fn connector(prst_name: &str, ln: &str) -> String {
    format!(
        r#"<p:cxnSp><p:nvCxnSpPr><p:cNvPr id="3" name="C"/><p:cNvCxnSpPr/><p:nvPr/></p:nvCxnSpPr><p:spPr>{}{}<a:ln w="38100"><a:solidFill><a:srgbClr val="FF0000"/></a:solidFill>{ln}</a:ln></p:spPr></p:cxnSp>"#,
        xf(1_905_000, 1_905_000, 3_810_000, 1_905_000),
        prst(prst_name, "")
    )
}

fn is_red(p: [u8; 3]) -> bool {
    p[0] > 200 && p[1] < 80 && p[2] < 80
}

#[test]
fn a_bent_connector_with_a_tail_end_has_the_arrow_on_its_last_segment() {
    // The box is (200, 200) .. (600, 400) px; the path goes right, down, right and ends at the
    // bottom right corner pointing right.
    let d = D::new(&connector(
        "bentConnector3",
        r#"<a:tailEnd type="triangle"/>"#,
    ));
    let sc = d.scene();
    let s = first_shape(&sc);
    assert!(matches!(&s.geom, sd::Geometry::Paths(p) if p.len() == 1));
    assert!(s.line.as_ref().unwrap().tail.is_some() && s.line.as_ref().unwrap().head.is_none());
    let (svg, img) = render(&sc);
    assert!(!svg.is_empty());
    // The line itself (4 px wide): first segment, the middle bar, the last segment.
    assert!(is_red(px(&img, 250, 200)), "{:?}", px(&img, 250, 200));
    assert!(is_red(px(&img, 400, 300)), "{:?}", px(&img, 400, 300));
    assert!(is_red(px(&img, 500, 400)), "{:?}", px(&img, 500, 400));
    // The head (a triangle 12 px long and wide, base at x = 588) is wider than the line (4 px):
    // red 4 px off the line near the base, none further back or on the other end.
    assert!(is_red(px(&img, 589, 396)), "{:?}", px(&img, 589, 396));
    assert!(is_red(px(&img, 589, 404)), "{:?}", px(&img, 589, 404));
    assert_eq!(px(&img, 560, 395), [255, 255, 255]);
    assert_eq!(px(&img, 205, 195), [255, 255, 255]);
}

#[test]
fn a_head_end_sits_on_the_start_of_a_curved_connector_and_points_backwards() {
    let d = D::new(&connector(
        "curvedConnector3",
        r#"<a:headEnd type="triangle"/>"#,
    ));
    let sc = d.scene();
    let (_, img) = render(&sc);
    // The curve starts at (200, 200) heading right: the head is at the start, wider than the line.
    assert!(is_red(px(&img, 211, 196)), "{:?}", px(&img, 211, 196));
    assert!(is_red(px(&img, 211, 204)), "{:?}", px(&img, 211, 204));
    assert_eq!(px(&img, 590, 395), [255, 255, 255]);
}

#[test]
fn a_straight_connector_is_a_line_and_has_its_ends() {
    let d = D::new(&connector(
        "straightConnector1",
        r#"<a:tailEnd type="triangle"/>"#,
    ));
    let sc = d.scene();
    assert_eq!(first_shape(&sc).geom, sd::Geometry::Line);
    let r = sd::render_svg(&sc, &|_| None);
    assert!(r.svg.matches("<path").count() >= 3, "{}", r.svg);
}

#[test]
fn symbol_font_bullets_show_the_characters_of_the_font() {
    let bullet = |font: &str, ch: &str| {
        let d = D::new(&sp_with(
            &prst("rect", ""),
            "",
            4_000_000,
            1_000_000,
            &format!(
                r#"<a:p><a:pPr marL="342900" indent="-342900"><a:buFont typeface="{font}" charset="2"/><a:buChar char="{ch}"/></a:pPr><a:r><a:rPr lang="en-US"><a:solidFill><a:srgbClr val="FFFFFF"/></a:solidFill></a:rPr><a:t>item</a:t></a:r></a:p>"#
            ),
        ));
        let sc = d.scene();
        sd::render_svg(&sc, &|_| None).svg
    };
    assert!(bullet("Symbol", "&#xF02D;").contains('\u{2212}'));
    assert!(bullet("Symbol", "&#xF0B7;").contains('\u{2022}'));
    assert!(bullet("Wingdings", "&#xF06C;").contains('\u{25CF}'));
    assert!(bullet("Wingdings", "&#xF0A7;").contains('\u{25AA}'));
    assert!(bullet("Wingdings", "&#xF0D8;").contains('\u{27A2}'));
    assert!(bullet("Wingdings", "&#xF0FC;").contains('\u{2714}'));
    assert!(bullet("Wingdings", "&#xF076;").contains('\u{2756}'));
    // A text font keeps its character.
    assert!(bullet("Arial", "&#x2013;").contains('\u{2013}'));
}

#[test]
fn a_picture_is_clipped_to_a_preset_geometry() {
    let pic = |geom: &str| {
        let mut d = D::new(&pic_xml(
            "rIdP",
            &format!("{}{geom}", xf(0, 0, 1_000_000, 1_000_000)),
            "",
            "",
        ));
        d.media.push(("p1.png".into(), tiny_png(1)));
        d.slides[0]
            .rels
            .push(("rIdP".into(), "image".into(), "../media/p1.png".into()));
        let doc = d.load();
        let Item::Picture(p) = &doc.slide_scenes[0].items[0] else {
            panic!()
        };
        p.geom.clone()
    };
    assert_eq!(pic(&prst("ellipse", "")), sd::Geometry::Ellipse);
    assert!(matches!(
        pic(&prst("roundRect", "")),
        sd::Geometry::Paths(_)
    ));
    assert!(matches!(pic(&prst("star5", "")), sd::Geometry::Paths(_)));
    assert_eq!(pic(&prst("rect", "")), sd::Geometry::Rect);
}

#[test]
fn an_emf_is_registered_for_drawing_and_an_svg_beside_it_is_preferred() {
    let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"/>"#.to_vec();
    let ext = r#"<a:extLst><a:ext uri="{96DAC541-7B7A-43D3-8B79-37D633B846F1}"><asvg:svgBlip xmlns:asvg="http://schemas.microsoft.com/office/drawing/2016/SVG/main" r:embed="rIdS"/></a:ext></a:extLst>"#;
    let mut d = D::new(&pic_xml("rIdE", &xf(0, 0, 10, 10), "", ext));
    d.media.push(("icon.svg".into(), svg));
    d.media.push(("old.emf".into(), vec![1, 0, 0, 0, 7]));
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
    let used = doc.images.iter().find(|i| i.key == p.image.key).unwrap();
    assert_eq!(used.name, "icon.svg");
    // Without the SVG the EMF is the picture.
    let mut d = D::new(&pic_xml("rIdE", &xf(0, 0, 10, 10), "", ""));
    d.media.push(("old.wmf".into(), vec![1, 0, 0, 0, 7]));
    d.slides[0]
        .rels
        .push(("rIdE".into(), "image".into(), "../media/old.wmf".into()));
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
        "old.wmf"
    );
    // The text view does not show it.
    assert!(!doc.markdown.contains("old.wmf"));
    assert!(!doc.markdown.contains(&p.image.key));
}

const ACCENT_STYLE: &str = r#"<p:style><a:lnRef idx="2"><a:schemeClr val="lt1"/></a:lnRef><a:fillRef idx="1"><a:schemeClr val="accent1"/></a:fillRef><a:effectRef idx="0"><a:schemeClr val="accent1"/></a:effectRef><a:fontRef idx="minor"><a:schemeClr val="lt1"/></a:fontRef></p:style>"#;

#[test]
fn a_placeholder_with_a_style_of_its_own_keeps_the_style_over_the_masters_no_fill() {
    // The master's title placeholder says no fill and no line; the slide's title has a style
    // (accent1 fill, white line): PowerPoint and LibreOffice show the styled box.
    let mut d = D::new(&shape(
        r#"type="title""#,
        &xf(0, 0, 4_000_000, 1_000_000),
        "",
        "",
        &para("T"),
        ACCENT_STYLE,
    ));
    d.master_shapes = shape(
        r#"type="title""#,
        &format!(
            "{}{RECT}<a:noFill/><a:ln><a:noFill/></a:ln>",
            xf(0, 0, 4_000_000, 1_000_000)
        ),
        "",
        "",
        &para("M"),
        "",
    );
    let sc = d.scene();
    let s = first_shape(&sc);
    assert_eq!(s.fill, Fill::Solid(Rgba::rgb(0x44, 0x72, 0xC4)));
    let line = s.line.as_ref().expect("the style's line");
    assert_eq!(line.fill, Fill::Solid(Rgba::WHITE));
    // Without a style of its own the placeholder keeps the master's no fill.
    let mut d = D::new(&shape(
        r#"type="title""#,
        &xf(0, 0, 4_000_000, 1_000_000),
        "",
        "",
        &para("T"),
        "",
    ));
    d.master_shapes = shape(
        r#"type="title""#,
        &format!(
            "{}{RECT}<a:noFill/><a:ln><a:noFill/></a:ln>",
            xf(0, 0, 4_000_000, 1_000_000)
        ),
        "",
        "",
        &para("M"),
        "",
    );
    let sc = d.scene();
    let s = first_shape(&sc);
    assert!(!s.fill.is_visible() && s.line.is_none());
}

#[test]
fn a_shape_of_the_layout_keeps_its_own_fill_over_its_style() {
    // (A layout's shape is "own" in its part: its explicit white fill beats the accent style.)
    let mut d = D::new("");
    d.layout_shapes = shape(
        "",
        &format!(
            r#"{}{RECT}<a:solidFill><a:srgbClr val="FFFFFF"/></a:solidFill><a:ln><a:noFill/></a:ln>"#,
            xf(0, 0, 4_000_000, 1_000_000)
        ),
        "",
        "",
        "",
        ACCENT_STYLE,
    );
    let sc = d.scene();
    let s = first_shape(&sc);
    assert_eq!(s.fill, Fill::Solid(Rgba::WHITE));
    assert!(s.line.is_none());
}
