//! Tests of the OpenDocument slide drawing: hand-written ODF snippets in the form LibreOffice
//! writes them, one rule at a time, then the budgets and hostile input.

use crate::preview::office::slide_draw as sd;
use crate::preview::office::tests_odp::{doc, load_op, Op};
use sd::{Fill, Geometry, Item, Rgba, ShapeItem};

use super::units::{draw_transform, parse_points, parse_svg_path};
use super::*;

const EMU_CM: f64 = 360_000.0;

/// A real PNG of `w` x `h` pixels (the pixel size is read from the header).
fn png(w: u32, h: u32) -> Vec<u8> {
    let img = image::RgbaImage::from_pixel(w, h, image::Rgba([200, 30, 30, 255]));
    let mut out = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
        .unwrap();
    out
}

/// The page layouts and the master page every test slide uses: 28 x 15.75 cm.
const LAYOUT: &str = r#"<style:page-layout style:name="PM1"><style:page-layout-properties fo:page-width="28cm" fo:page-height="15.75cm"/></style:page-layout>"#;

fn master_with(shapes: &str, mdp: &str) -> String {
    format!(
        r#"<style:master-page style:name="Default" style:page-layout-name="PM1" draw:style-name="Mdp1">{shapes}</style:master-page>{mdp}"#
    )
}

const MDP: &str = r#""#;

/// An `Op` with the 28 x 15.75 cm master page.
fn op(pages: &str) -> Op {
    Op::new(pages)
        .styles_auto(LAYOUT)
        .master(&master_with("", MDP))
        .styles(r#"<style:style style:name="Mdp1" style:family="drawing-page"/>"#)
}

fn slide_page(inner: &str) -> String {
    format!(
        r#"<draw:page draw:name="p" draw:style-name="dp1" draw:master-page-name="Default">{inner}</draw:page>"#
    )
}

fn scenes(o: &Op) -> Vec<sd::SlideScene> {
    doc(o).slide_scenes
}

fn scene(o: &Op) -> sd::SlideScene {
    scenes(o).remove(0)
}

fn shapes_of(sc: &sd::SlideScene) -> Vec<&ShapeItem> {
    fn walk<'a>(items: &'a [Item], out: &mut Vec<&'a ShapeItem>) {
        for i in items {
            match i {
                Item::Shape(s) => out.push(s),
                Item::Group(g) => walk(&g.items, out),
                Item::Picture(_) => {}
            }
        }
    }
    let mut v = Vec::new();
    for u in &sc.underlay {
        walk(u, &mut v);
    }
    walk(&sc.items, &mut v);
    v
}

fn only_shape(o: &Op) -> ShapeItem {
    let sc = scene(o);
    let v = shapes_of(&sc);
    assert_eq!(v.len(), 1, "{:#?}", sc.items);
    v[0].clone()
}

/// One graphic style `gr1` with `props` (attributes of `style:graphic-properties`).
fn gr(props: &str) -> String {
    format!(
        r#"<style:style style:name="gr1" style:family="graphic"><style:graphic-properties {props}/></style:style>"#
    )
}

fn rect(extra: &str) -> String {
    format!(
        r#"<draw:rect draw:style-name="gr1" draw:layer="layout" svg:x="2cm" svg:y="1cm" svg:width="6cm" svg:height="3cm" {extra}/>"#
    )
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1.0
}

fn text_of_shape(s: &ShapeItem) -> String {
    s.text
        .as_ref()
        .map(|t| {
            t.paragraphs
                .iter()
                .map(|p| p.runs.iter().map(|r| r.text.as_str()).collect::<String>())
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------------------------
// page and contract
// ---------------------------------------------------------------------------------------------

#[test]
fn page_size_comes_from_the_master_pages_layout() {
    let sc = scene(&op(&slide_page("")));
    assert!(close(sc.width, 28.0 * EMU_CM) && close(sc.height, 15.75 * EMU_CM));
}

#[test]
fn a_border_sized_background_stays_inside_the_layouts_margins() {
    // 28 x 15.75 cm with 2 cm at the left and top, 1 cm at the right, none at the bottom
    let layout = r#"<style:page-layout style:name="PM1"><style:page-layout-properties fo:page-width="28cm" fo:page-height="15.75cm" fo:margin-left="2cm" fo:margin-top="2cm" fo:margin-right="1cm" fo:margin-bottom="0cm"/></style:page-layout>"#;
    let dp = |size: &str| {
        format!(
            r##"<style:style style:name="dp1" style:family="drawing-page"><style:drawing-page-properties draw:background-size="{size}" draw:fill="solid" draw:fill-color="#729fcf"/></style:style>"##
        )
    };
    let o = op(&slide_page("")).styles_auto(layout).auto(&dp("border"));
    let sc = scene(&o);
    // the page itself is white, the colour is a rectangle inside the margins, first of all
    assert_eq!(sc.background, Fill::Solid(Rgba::WHITE));
    let Some(sd::Item::Shape(s)) = sc.drawn_items().next() else {
        panic!("{:?}", sc.drawn_items().next())
    };
    assert_eq!(s.fill, Fill::Solid(Rgba::rgb(0x72, 0x9f, 0xcf)));
    assert!(close(s.xfrm.x, 2.0 * EMU_CM) && close(s.xfrm.y, 2.0 * EMU_CM));
    assert!(close(s.xfrm.w, 25.0 * EMU_CM) && close(s.xfrm.h, 13.75 * EMU_CM));
    // "full" (and a layout without margins) colours the whole page
    let o = op(&slide_page("")).styles_auto(layout).auto(&dp("full"));
    assert_eq!(
        scene(&o).background,
        Fill::Solid(Rgba::rgb(0x72, 0x9f, 0xcf))
    );
    let o = op(&slide_page("")).auto(&dp("border"));
    assert_eq!(
        scene(&o).background,
        Fill::Solid(Rgba::rgb(0x72, 0x9f, 0xcf))
    );
}

#[test]
fn a_page_without_a_master_gets_the_default_size() {
    let sc = scene(&Op::new(&crate::preview::office::tests_odp::page("")));
    assert_eq!((sc.width, sc.height), DEFAULT_SIZE);
}

#[test]
fn portrait_and_other_units_in_the_layout() {
    let layout = r#"<style:page-layout style:name="PM1"><style:page-layout-properties fo:page-width="5in" fo:page-height="720pt"/></style:page-layout>"#;
    let o = op(&slide_page("")).styles_auto(layout);
    let sc = scene(&o);
    assert!(close(sc.width, 5.0 * 914_400.0) && close(sc.height, 10.0 * 914_400.0));
}

#[test]
fn one_scene_one_key_one_heading_per_slide() {
    let p = format!("{}{}", slide_page(""), slide_page(""));
    let d = doc(&op(&p));
    assert_eq!(d.slide_scenes.len(), 2);
    assert_eq!(d.slide_keys.len(), 2);
    assert_eq!(d.slides.len(), 2);
    assert_eq!(
        d.markdown.lines().filter(|l| l.starts_with("## ")).count(),
        2
    );
    assert_eq!(
        d.picture_markdown
            .lines()
            .filter(|l| l.starts_with("## "))
            .count(),
        2
    );
    assert!(d.picture_markdown.contains(&d.slide_keys[1]));
}

#[test]
fn a_hidden_slide_is_drawn() {
    let auto = r#"<style:style style:name="dp1" style:family="drawing-page"><style:drawing-page-properties presentation:visibility="hidden"/></style:style>"#;
    let o = op(&slide_page(&rect(""))).auto(&format!(
        "{auto}{}",
        gr(r##"draw:fill="solid" draw:fill-color="#ff0000""##)
    ));
    let d = doc(&o);
    assert!(d.markdown.contains("(hidden)"), "{}", d.markdown);
    assert_eq!(shapes_of(&d.slide_scenes[0]).len(), 1);
}

// ---------------------------------------------------------------------------------------------
// backgrounds
// ---------------------------------------------------------------------------------------------

#[test]
fn background_from_the_master_then_the_slide() {
    let styles = r##"<style:style style:name="Mdp1" style:family="drawing-page"><style:drawing-page-properties draw:fill="solid" draw:fill-color="#00ff00"/></style:style>"##;
    let o = op(&slide_page("")).styles(styles);
    assert_eq!(scene(&o).background, Fill::Solid(Rgba::rgb(0, 255, 0)));
    // The slide's own style wins.
    let auto = r##"<style:style style:name="dp1" style:family="drawing-page"><style:drawing-page-properties draw:fill="solid" draw:fill-color="#0000ff"/></style:style>"##;
    let o = o.auto(auto);
    assert_eq!(scene(&o).background, Fill::Solid(Rgba::rgb(0, 0, 255)));
}

#[test]
fn a_translucent_background_lies_over_white() {
    // 58 % of #729fcf over white (measured against LibreOffice's picture of such a page)
    let auto = r##"<style:style style:name="dp1" style:family="drawing-page"><style:drawing-page-properties draw:fill="solid" draw:fill-color="#729fcf" draw:opacity="58%"/></style:style>"##;
    assert_eq!(
        scene(&op(&slide_page("")).auto(auto)).background,
        Fill::Solid(Rgba::rgb(173, 199, 227))
    );
    // fully transparent: white
    let auto = r##"<style:style style:name="dp1" style:family="drawing-page"><style:drawing-page-properties draw:fill="solid" draw:fill-color="#729fcf" draw:opacity="0%"/></style:style>"##;
    assert_eq!(
        scene(&op(&slide_page("")).auto(auto)).background,
        Fill::Solid(Rgba::WHITE)
    );
}

#[test]
fn background_none_and_hidden_background_are_white() {
    let auto = r#"<style:style style:name="dp1" style:family="drawing-page"><style:drawing-page-properties draw:fill="none"/></style:style>"#;
    assert_eq!(
        scene(&op(&slide_page("")).auto(auto)).background,
        Fill::Solid(Rgba::WHITE)
    );
    let styles = r##"<style:style style:name="Mdp1" style:family="drawing-page"><style:drawing-page-properties draw:fill="solid" draw:fill-color="#00ff00"/></style:style>"##;
    let auto = r#"<style:style style:name="dp1" style:family="drawing-page"><style:drawing-page-properties presentation:background-visible="false"/></style:style>"#;
    assert_eq!(
        scene(&op(&slide_page("")).styles(styles).auto(auto)).background,
        Fill::Solid(Rgba::WHITE)
    );
}

#[test]
fn background_gradient_and_bitmap() {
    let styles = r##"<draw:gradient draw:name="G" draw:style="linear" draw:start-color="#ff0000" draw:end-color="#0000ff" draw:angle="0deg" draw:border="0%"/><style:style style:name="Mdp1" style:family="drawing-page"><style:drawing-page-properties draw:fill="gradient" draw:fill-gradient-name="G"/></style:style>"##;
    match scene(&op(&slide_page("")).styles(styles)).background {
        Fill::Gradient(g) => {
            assert_eq!(g.stops.len(), 2);
            assert_eq!(g.stops[0].1, Rgba::rgb(255, 0, 0));
        }
        other => panic!("{other:?}"),
    }
    let styles = r#"<draw:fill-image draw:name="B" xlink:href="Pictures/a.png"/><style:style style:name="Mdp1" style:family="drawing-page"><style:drawing-page-properties draw:fill="bitmap" draw:fill-image-name="B" style:repeat="stretch"/></style:style>"#;
    let o = op(&slide_page("")).styles(styles).picture("a.png", 1);
    match scene(&o).background {
        Fill::Image(i) => assert!(i.key.starts_with("office-img://")),
        other => panic!("{other:?}"),
    }
}

// ---------------------------------------------------------------------------------------------
// styles
// ---------------------------------------------------------------------------------------------

#[test]
fn solid_fill_and_stroke_of_a_rectangle() {
    let g = gr(
        r##"draw:fill="solid" draw:fill-color="#ff8000" draw:stroke="solid" svg:stroke-color="#112233" svg:stroke-width="0.1cm""##,
    );
    let s = only_shape(&op(&slide_page(&rect(""))).auto(&g));
    assert_eq!(s.fill, Fill::Solid(Rgba::rgb(255, 128, 0)));
    let l = s.line.expect("line");
    assert!(close(l.width, 36_000.0));
    assert_eq!(l.fill, Fill::Solid(Rgba::rgb(0x11, 0x22, 0x33)));
    assert!(close(s.xfrm.x, 2.0 * EMU_CM) && close(s.xfrm.y, EMU_CM));
    assert!(close(s.xfrm.w, 6.0 * EMU_CM) && close(s.xfrm.h, 3.0 * EMU_CM));
    assert!(matches!(s.geom, Geometry::Rect));
}

#[test]
fn nothing_is_drawn_without_fill_line_or_text() {
    let sc = scene(&op(&slide_page(&rect(""))).auto(&gr(r#"draw:fill="none" draw:stroke="none""#)));
    assert!(sc.items.is_empty());
}

#[test]
fn style_parents_and_the_default_style() {
    let styles = r##"<style:default-style style:family="graphic"><style:graphic-properties draw:fill="solid" draw:fill-color="#010101" draw:stroke="none"/></style:default-style><style:style style:name="base" style:family="graphic"><style:graphic-properties draw:fill-color="#020202"/></style:style>"##;
    let auto = r##"<style:style style:name="gr1" style:family="graphic" style:parent-style-name="base"><style:graphic-properties draw:stroke="solid" svg:stroke-color="#030303"/></style:style>"##;
    let s = only_shape(&op(&slide_page(&rect(""))).styles(styles).auto(auto));
    // fill kind from the default style, colour from the parent, stroke from the style itself
    assert_eq!(s.fill, Fill::Solid(Rgba::rgb(2, 2, 2)));
    assert_eq!(s.line.unwrap().fill, Fill::Solid(Rgba::rgb(3, 3, 3)));
}

#[test]
fn a_parent_style_cycle_ends() {
    let styles = r##"<style:style style:name="a" style:family="graphic" style:parent-style-name="b"><style:graphic-properties draw:fill="solid"/></style:style><style:style style:name="b" style:family="graphic" style:parent-style-name="a"><style:graphic-properties draw:fill-color="#0a0a0a"/></style:style>"##;
    let auto =
        r#"<style:style style:name="gr1" style:family="graphic" style:parent-style-name="a"/>"#;
    let s = only_shape(&op(&slide_page(&rect(""))).styles(styles).auto(auto));
    assert_eq!(s.fill, Fill::Solid(Rgba::rgb(10, 10, 10)));
}

#[test]
fn a_long_parent_chain_is_cut_not_followed_forever() {
    let mut styles = String::new();
    for i in 0..200 {
        styles.push_str(&format!(
            r#"<style:style style:name="s{i}" style:family="graphic" style:parent-style-name="s{}"/>"#,
            i + 1
        ));
    }
    styles.push_str(
        r##"<style:style style:name="s200" style:family="graphic"><style:graphic-properties draw:fill="solid" draw:fill-color="#ff0000"/></style:style>"##,
    );
    let auto =
        r#"<style:style style:name="gr1" style:family="graphic" style:parent-style-name="s0"/>"#;
    let sc = scene(&op(&slide_page(&rect(""))).styles(&styles).auto(auto));
    // (the chain is longer than MAX_CHAIN: the root's fill is not reached, nothing hangs; the
    // style says nothing, so the shape has LibreOffice's default fill)
    let shapes = shapes_of(&sc);
    assert_eq!(shapes.len(), 1);
    assert_eq!(shapes[0].fill, Fill::Solid(Rgba::rgb(0x72, 0x9f, 0xcf)));
}

#[test]
fn a_style_that_never_says_fill_fills_solid_like_libreoffice() {
    // no style at all: the default blue
    let s = only_shape(&op(&slide_page(&rect(""))));
    assert_eq!(s.fill, Fill::Solid(Rgba::rgb(0x72, 0x9f, 0xcf)));
    // the default style's colour without a kind: still solid
    let styles = r##"<style:default-style style:family="graphic"><style:graphic-properties draw:fill-color="#102030"/></style:default-style>"##;
    let s = only_shape(&op(&slide_page(&rect(""))).styles(styles));
    assert_eq!(s.fill, Fill::Solid(Rgba::rgb(0x10, 0x20, 0x30)));
    // an explicit none stays none
    let styles = r##"<style:default-style style:family="graphic"><style:graphic-properties draw:fill="none"/></style:default-style>"##;
    let sc = scene(&op(&slide_page(&rect(""))).styles(styles));
    assert!(shapes_of(&sc).is_empty());
}

#[test]
fn presentation_style_then_graphic_style() {
    let styles = r##"<style:style style:name="P" style:family="presentation"><style:graphic-properties draw:fill="solid" draw:fill-color="#0000ff" draw:stroke="none"/></style:style>"##;
    let auto = gr(r##"draw:fill-color="#00ff00""##);
    let shape = r#"<draw:rect presentation:style-name="P" draw:style-name="gr1" svg:x="1cm" svg:y="1cm" svg:width="2cm" svg:height="2cm"/>"#;
    let s = only_shape(&op(&slide_page(shape)).styles(styles).auto(&auto));
    assert_eq!(s.fill, Fill::Solid(Rgba::rgb(0, 255, 0)));
}

#[test]
fn solid_opacity_is_the_alpha() {
    let g = gr(r##"draw:fill="solid" draw:fill-color="#ff0000" draw:opacity="40%""##);
    let s = only_shape(&op(&slide_page(&rect(""))).auto(&g));
    match s.fill {
        Fill::Solid(c) => assert!((c.a - 0.4).abs() < 1e-9),
        other => panic!("{other:?}"),
    }
}

fn with_resource(res: &str, g: &str) -> ShapeItem {
    only_shape(&op(&slide_page(&rect(""))).styles(res).auto(&gr(g)))
}

#[test]
fn linear_gradient_angle_and_border() {
    let res = r##"<draw:gradient draw:name="G" draw:style="linear" draw:start-color="#ff0000" draw:end-color="#0000ff" draw:angle="30deg" draw:border="20%"/>"##;
    let s = with_resource(res, r#"draw:fill="gradient" draw:fill-gradient-name="G""#);
    let Fill::Gradient(g) = s.fill else { panic!() };
    match g.kind {
        sd::GradKind::Linear { angle_deg, .. } => assert!((angle_deg - 60.0).abs() < 1e-9),
        k => panic!("{k:?}"),
    }
    // the border is a solid start colour first
    assert_eq!(g.stops[0].0, 0.0);
    assert_eq!(g.stops[1].0, 0.2);
    assert_eq!(g.stops[0].1, g.stops[1].1);
    assert_eq!(g.stops.last().unwrap().1, Rgba::rgb(0, 0, 255));
}

#[test]
fn gradient_intensity_scales_the_colour() {
    let res = r##"<draw:gradient draw:name="G" draw:style="linear" draw:start-color="#ffffff" draw:end-color="#ffffff" draw:start-intensity="50%" draw:end-intensity="100%"/>"##;
    let s = with_resource(res, r#"draw:fill="gradient" draw:fill-gradient-name="G""#);
    let Fill::Gradient(g) = s.fill else { panic!() };
    assert_eq!(g.stops[0].1, Rgba::rgb(128, 128, 128));
    assert_eq!(g.stops[1].1, Rgba::rgb(255, 255, 255));
}

#[test]
fn radial_rect_and_axial_gradients() {
    for (style, expect_radial, expect_rect) in [
        ("radial", true, false),
        ("ellipsoid", false, false),
        ("square", false, true),
        ("rectangular", false, true),
    ] {
        let res = format!(
            r##"<draw:gradient draw:name="G" draw:style="{style}" draw:cx="25%" draw:cy="75%" draw:start-color="#ff0000" draw:end-color="#0000ff"/>"##
        );
        let s = with_resource(&res, r#"draw:fill="gradient" draw:fill-gradient-name="G""#);
        let Fill::Gradient(g) = s.fill else { panic!() };
        assert_eq!(matches!(g.kind, sd::GradKind::Radial), expect_radial);
        assert_eq!(matches!(g.kind, sd::GradKind::Rect), expect_rect);
        assert_eq!(g.fill_to_rect, (0.25, 0.75, 0.75, 0.25));
        // the end colour is in the middle
        assert_eq!(g.stops[0].1, Rgba::rgb(0, 0, 255));
        assert_eq!(g.stops.last().unwrap().1, Rgba::rgb(255, 0, 0));
    }
    let res = r##"<draw:gradient draw:name="G" draw:style="axial" draw:start-color="#ff0000" draw:end-color="#0000ff"/>"##;
    let s = with_resource(res, r#"draw:fill="gradient" draw:fill-gradient-name="G""#);
    let Fill::Gradient(g) = s.fill else { panic!() };
    // mirrored: end, start (middle), end
    assert_eq!(g.stops.first().unwrap().1, Rgba::rgb(0, 0, 255));
    assert_eq!(g.stops.last().unwrap().1, Rgba::rgb(0, 0, 255));
    assert!(g
        .stops
        .iter()
        .any(|s| s.0 == 0.5 && s.1 == Rgba::rgb(255, 0, 0)));
}

#[test]
fn multi_stop_gradients_use_their_stops() {
    let res = r##"<draw:gradient draw:name="G" draw:style="linear" draw:start-color="#000000" draw:end-color="#ffffff"><loext:gradient-stop svg:offset="0" loext:color-value="#ff0000"/><loext:gradient-stop svg:offset="0.5" loext:color-value="#00ff00"/><loext:gradient-stop svg:offset="1" loext:color-value="#0000ff"/></draw:gradient>"##;
    let s = with_resource(res, r#"draw:fill="gradient" draw:fill-gradient-name="G""#);
    let Fill::Gradient(g) = s.fill else { panic!() };
    assert_eq!(g.stops.len(), 3);
    assert_eq!(g.stops[1], (0.5, Rgba::rgb(0, 255, 0)));
}

#[test]
fn transparency_gradient_over_a_solid_fill_is_a_gradient_of_alpha() {
    let res = r#"<draw:opacity draw:name="T" draw:style="linear" draw:start="100%" draw:end="0%" draw:angle="0deg"/>"#;
    let s = with_resource(
        res,
        r##"draw:fill="solid" draw:fill-color="#ff0000" draw:opacity-name="T""##,
    );
    let Fill::Gradient(g) = s.fill else {
        panic!("{:?}", s.fill)
    };
    assert_eq!(g.stops[0].1.r, 255);
    assert!((g.stops[0].1.a - 1.0).abs() < 1e-9);
    assert!(g.stops.last().unwrap().1.a < 1e-9);
}

#[test]
fn transparency_gradient_matching_the_colour_gradient_is_multiplied_in() {
    let res = r##"<draw:gradient draw:name="G" draw:style="linear" draw:start-color="#ff0000" draw:end-color="#0000ff" draw:angle="0deg"/><draw:opacity draw:name="T" draw:style="linear" draw:start="100%" draw:end="50%" draw:angle="0deg"/>"##;
    let s = with_resource(
        res,
        r#"draw:fill="gradient" draw:fill-gradient-name="G" draw:opacity-name="T""#,
    );
    let Fill::Gradient(g) = s.fill else { panic!() };
    assert!((g.stops[0].1.a - 1.0).abs() < 1e-9);
    assert!((g.stops[1].1.a - 0.5).abs() < 1e-9);
}

#[test]
fn transparency_gradient_of_another_shape_is_its_mean() {
    let res = r##"<draw:gradient draw:name="G" draw:style="linear" draw:start-color="#ff0000" draw:end-color="#0000ff" draw:angle="0deg"/><draw:opacity draw:name="T" draw:style="radial" draw:start="100%" draw:end="0%"/>"##;
    let s = with_resource(
        res,
        r#"draw:fill="gradient" draw:fill-gradient-name="G" draw:opacity-name="T""#,
    );
    let Fill::Gradient(g) = s.fill else { panic!() };
    assert!(g.stops.iter().all(|s| (s.1.a - 0.5).abs() < 1e-9));
}

#[test]
fn a_gradient_with_a_uniform_opacity_value() {
    let res = r##"<draw:gradient draw:name="G" draw:style="linear" draw:start-color="#ff0000" draw:end-color="#0000ff"/>"##;
    let s = with_resource(
        res,
        r#"draw:fill="gradient" draw:fill-gradient-name="G" draw:opacity="60%""#,
    );
    let Fill::Gradient(g) = s.fill else { panic!() };
    assert!(g.stops.iter().all(|s| (s.1.a - 0.6).abs() < 1e-9));
}

#[test]
fn a_missing_gradient_is_the_default_one() {
    let s = with_resource(
        "",
        r##"draw:fill="gradient" draw:fill-gradient-name="nope" draw:fill-color="#123456""##,
    );
    let Fill::Gradient(g) = s.fill else {
        panic!("{:?}", s.fill)
    };
    // LibreOffice's default: its line blue to white, from the top; the fill colour is not used
    assert_eq!(g.stops[0].1, Rgba::rgb(0x34, 0x65, 0xa4));
    assert_eq!(g.stops[1].1, Rgba::WHITE);
    assert_eq!(
        g.kind,
        sd::GradKind::Linear {
            angle_deg: 90.0,
            scaled: false
        }
    );
}

#[test]
fn hatches_become_patterns() {
    let cases = [
        ("single", "0deg", "0.3cm", "horz"),
        ("single", "900", "0.07cm", "narVert"),
        ("single", "450", "0.15cm", "ltUpDiag"),
        ("single", "1350", "0.3cm", "dnDiag"),
        ("double", "0deg", "0.3cm", "cross"),
        ("triple", "45deg", "0.3cm", "diagCross"),
    ];
    for (style, rot, dist, want) in cases {
        let res = format!(
            r##"<draw:hatch draw:name="H" draw:style="{style}" draw:color="#112233" draw:distance="{dist}" draw:rotation="{rot}"/>"##
        );
        let s = with_resource(
            &res,
            r##"draw:fill="hatch" draw:fill-hatch-name="H" draw:fill-color="#ffff00" draw:fill-hatch-solid="true""##,
        );
        let Fill::Pattern { preset, fg, bg } = s.fill else {
            panic!()
        };
        assert_eq!(preset, want, "{style} {rot} {dist}");
        assert_eq!(fg, Rgba::rgb(0x11, 0x22, 0x33));
        assert_eq!(bg, Rgba::rgb(255, 255, 0));
    }
    // without a background fill the pattern is transparent behind its lines
    let res = r##"<draw:hatch draw:name="H" draw:style="single" draw:color="#000000" draw:distance="0.3cm" draw:rotation="0"/>"##;
    let s = with_resource(res, r#"draw:fill="hatch" draw:fill-hatch-name="H""#);
    let Fill::Pattern { bg, .. } = s.fill else {
        panic!()
    };
    assert_eq!(bg.a, 0.0);
}

#[test]
fn bitmap_fills_stretch_or_tile() {
    let res = r#"<draw:fill-image draw:name="B" xlink:href="Pictures/a.png"/>"#;
    let o = |g: &str| {
        only_shape(
            &op(&slide_page(&rect("")))
                .styles(res)
                .auto(&gr(g))
                .part("Pictures/a.png", &png(16, 8)),
        )
    };
    let s = o(r#"draw:fill="bitmap" draw:fill-image-name="B" style:repeat="stretch""#);
    assert!(matches!(
        s.fill,
        Fill::Image(sd::ImageFill {
            mode: sd::ImageMode::Stretch { .. },
            ..
        })
    ));
    let s = o(
        r#"draw:fill="bitmap" draw:fill-image-name="B" style:repeat="repeat" draw:fill-image-width="1cm" draw:fill-image-height="50%" draw:fill-image-ref-point="center""#,
    );
    let Fill::Image(i) = s.fill else { panic!() };
    match i.mode {
        // tiny_png is a few pixels: the scale is tile size over native size
        sd::ImageMode::Tile { sx, sy, align, .. } => {
            assert!(sx > 1.0 && sy > 1.0, "{sx} {sy}");
            assert_eq!(align, sd::RectAlign::Center);
        }
        m => panic!("{m:?}"),
    }
    // a bitmap whose picture is missing leaves the fill colour
    let s = only_shape(
        &op(&slide_page(&rect("")))
            .styles(res)
            .auto(&gr(r##"draw:fill="bitmap" draw:fill-image-name="B" draw:fill-color="#336699" draw:stroke="solid""##)),
    );
    assert_eq!(s.fill, Fill::Solid(Rgba::rgb(0x33, 0x66, 0x99)));
}

#[test]
fn bitmap_fills_are_enlarged_without_smoothing() {
    let res = r#"<draw:fill-image draw:name="B" xlink:href="Pictures/a.png"/>"#;
    for repeat in ["stretch", "repeat", "no-repeat"] {
        let g = format!(
            r#"draw:fill="bitmap" draw:fill-image-name="B" style:repeat="{repeat}" draw:fill-image-width="1cm" draw:fill-image-height="1cm""#
        );
        let s = only_shape(
            &op(&slide_page(&rect("")))
                .styles(res)
                .auto(&gr(&g))
                .part("Pictures/a.png", &png(16, 8)),
        );
        let Fill::Image(i) = s.fill else {
            panic!("{repeat}")
        };
        assert!(i.pixelated, "{repeat}");
    }
}

#[test]
fn a_no_repeat_bitmap_is_one_copy_at_the_reference_point() {
    let res = r#"<draw:fill-image draw:name="B" xlink:href="Pictures/a.png"/>"#;
    // The shape is 6 cm x 3 cm; the picture 2 cm x 1 cm.
    let fill_rect = |extra: &str| {
        let g = format!(
            r##"draw:fill="bitmap" draw:fill-image-name="B" style:repeat="no-repeat" draw:fill-image-width="2cm" draw:fill-image-height="1cm" draw:fill-color="#336699" {extra}"##
        );
        let s = only_shape(
            &op(&slide_page(&rect("")))
                .styles(res)
                .auto(&gr(&g))
                .part("Pictures/a.png", &png(16, 8)),
        );
        let Fill::Image(i) = s.fill else { panic!() };
        let sd::ImageMode::Stretch { fill_rect } = i.mode else {
            panic!("{:?}", i.mode)
        };
        fill_rect
    };
    let close4 = |a: (f64, f64, f64, f64), b: (f64, f64, f64, f64)| {
        (a.0 - b.0).abs() < 1e-6
            && (a.1 - b.1).abs() < 1e-6
            && (a.2 - b.2).abs() < 1e-6
            && (a.3 - b.3).abs() < 1e-6
    };
    // top-left (the default): the picture occupies the left third and the top third
    let r = fill_rect("");
    assert!(close4(r, (0.0, 0.0, 2.0 / 3.0, 2.0 / 3.0)), "{r:?}");
    let r = fill_rect(r#"draw:fill-image-ref-point="center""#);
    assert!(
        close4(r, (1.0 / 3.0, 1.0 / 3.0, 1.0 / 3.0, 1.0 / 3.0)),
        "{r:?}"
    );
    let r = fill_rect(r#"draw:fill-image-ref-point="bottom-right""#);
    assert!(close4(r, (2.0 / 3.0, 2.0 / 3.0, 0.0, 0.0)), "{r:?}");
    let r = fill_rect(r#"draw:fill-image-ref-point="top""#);
    assert!(close4(r, (1.0 / 3.0, 0.0, 1.0 / 3.0, 2.0 / 3.0)), "{r:?}");
    // an offset of 50 % of the picture's width moves it 1 cm right
    let r = fill_rect(r#"draw:fill-image-ref-point-x="50%""#);
    assert!(close4(r, (1.0 / 6.0, 0.0, 0.5, 2.0 / 3.0)), "{r:?}");
}

#[test]
fn strokes_dash_cap_join_opacity() {
    let res = r##"<draw:stroke-dash draw:name="D" draw:style="rect" draw:dots1="2" draw:dots1-length="200%" draw:dots2="1" draw:dots2-length="0.4cm" draw:distance="100%"/>"##;
    let s = with_resource(
        res,
        r##"draw:stroke="dash" draw:stroke-dash="D" svg:stroke-width="0.1cm" svg:stroke-linecap="round" draw:stroke-linejoin="bevel" svg:stroke-opacity="50%" svg:stroke-color="#ff0000""##,
    );
    let l = s.line.unwrap();
    assert_eq!(l.cap, sd::Cap::Round);
    assert_eq!(l.join, sd::Join::Bevel);
    match l.fill {
        Fill::Solid(c) => assert!((c.a - 0.5).abs() < 1e-9),
        o => panic!("{o:?}"),
    }
    match l.dash {
        sd::Dash::Custom(p) => {
            assert_eq!(p.len(), 3);
            assert_eq!(p[0], (2.0, 1.0));
            // 0.4 cm over a 0.1 cm line
            assert!((p[2].0 - 4.0).abs() < 1e-9, "{p:?}");
        }
        d => panic!("{d:?}"),
    }
    // a dash style that does not exist is the ordinary dash; miter join
    let s = with_resource("", r#"draw:stroke="dash" draw:stroke-linejoin="miter""#);
    let l = s.line.unwrap();
    assert_eq!(l.dash, sd::Dash::Dash);
    assert!(matches!(l.join, sd::Join::Miter(_)));
    // a zero-length dot is one line width
    let res = r##"<draw:stroke-dash draw:name="D" draw:style="round" draw:dots1="1" draw:dots1-length="0cm" draw:distance="0.2cm"/>"##;
    let s = with_resource(
        res,
        r#"draw:stroke="dash" draw:stroke-dash="D" svg:stroke-width="0.1cm""#,
    );
    let sd::Dash::Custom(p) = s.line.unwrap().dash else {
        panic!()
    };
    assert_eq!(p[0], (1.0, 2.0));
}

#[test]
fn stroke_none_means_no_line() {
    let s = with_resource(
        "",
        r##"draw:fill="solid" draw:fill-color="#ff0000" draw:stroke="none""##,
    );
    assert!(s.line.is_none());
}

#[test]
fn markers_have_the_stated_size_and_orientation() {
    let res =
        r#"<draw:marker draw:name="Arrow" svg:viewBox="0 0 20 30" svg:d="M10 0l-10 30h20z"/>"#;
    let g = r##"draw:stroke="solid" svg:stroke-width="0cm" draw:marker-end="Arrow" draw:marker-end-width="0.4cm" draw:marker-start="Arrow" draw:marker-start-width="0.4cm" draw:marker-start-center="true""##;
    let line =
        r#"<draw:line draw:style-name="gr1" svg:x1="1cm" svg:y1="1cm" svg:x2="9cm" svg:y2="1cm"/>"#;
    let s = only_shape(&op(&slide_page(line)).styles(res).auto(&gr(g)));
    let l = s.line.unwrap();
    let tail = l.tail.expect("end marker");
    let head = l.head.expect("start marker");
    for a in [&tail, &head] {
        let sd::ArrowKind::Custom(paths) = &a.kind else {
            panic!()
        };
        let p = &paths[0];
        // the tip is at the right edge (x = w) in the middle (y = h / 2)
        let sd::PathCmd::MoveTo(tip) = p.cmds[0] else {
            panic!()
        };
        assert!((tip.y - p.h / 2.0).abs() < 1e-6, "{tip:?} {} {}", p.w, p.h);
    }
    let sd::ArrowKind::Custom(t) = &tail.kind else {
        panic!()
    };
    let sd::ArrowKind::Custom(h) = &head.kind else {
        panic!()
    };
    let (sd::PathCmd::MoveTo(tt), sd::PathCmd::MoveTo(ht)) = (t[0].cmds[0], h[0].cmds[0]) else {
        panic!()
    };
    // centred marker: its tip is half a marker length beyond the end of the line
    assert!(ht.x > tt.x);
    // the rendered width equals the stated 0.4 cm: path unit * (class box / path space)
    let unit = 0.4 * EMU_CM / 20.0;
    let eff = 2.0 * 12_700.0;
    let (wr, class_w) = match tail.w {
        sd::ArrowSize::Sm => (t[0].h, 2.0),
        sd::ArrowSize::Med => (t[0].h, 3.0),
        sd::ArrowSize::Lg => (t[0].h, 5.0),
    };
    assert!((class_w * eff / wr - unit).abs() / unit < 1e-6);
}

#[test]
fn a_marker_that_is_not_defined_is_ignored() {
    let s = with_resource(
        "",
        r##"draw:stroke="solid" draw:marker-end="Nope" draw:fill="solid" draw:fill-color="#000000""##,
    );
    assert!(s.line.unwrap().tail.is_none());
}

#[test]
fn shadows() {
    let s = with_resource(
        "",
        r##"draw:fill="solid" draw:fill-color="#ffffff" draw:shadow="visible" draw:shadow-offset-x="0.3cm" draw:shadow-offset-y="0.4cm" draw:shadow-color="#102030" draw:shadow-opacity="60%" draw:shadow-blur="0.1cm""##,
    );
    let sh = s.effects.outer_shadow.expect("shadow");
    assert!(close(sh.dist, 0.5 * EMU_CM));
    assert!((sh.dir_deg - 53.13).abs() < 0.01);
    assert!((sh.color.a - 0.6).abs() < 1e-9);
    assert_eq!((sh.color.r, sh.color.g, sh.color.b), (0x10, 0x20, 0x30));
    assert!(close(sh.blur_rad, 36_000.0));
    let s = with_resource(
        "",
        r##"draw:fill="solid" draw:fill-color="#ffffff" draw:shadow="hidden""##,
    );
    assert!(s.effects.outer_shadow.is_none());
}

// ---------------------------------------------------------------------------------------------
// geometry
// ---------------------------------------------------------------------------------------------

fn filled() -> String {
    gr(r##"draw:fill="solid" draw:fill-color="#ff0000" draw:stroke="solid""##)
}

#[test]
fn transforms_apply_in_written_order_and_rotate_counter_clockwise() {
    // `rotate (a) translate (x y)`: the shape is turned about its own origin first, then moved
    let m = draw_transform("rotate (1.5707963267949) translate (10cm 5cm)").unwrap();
    let (x, y) = units::apply(m, 0.0, 0.0);
    assert!(close(x, 10.0 * EMU_CM) && close(y, 5.0 * EMU_CM), "{x} {y}");
    // a quarter turn counter-clockwise takes "right" to "up"
    let (rx, ry) = units::apply(
        draw_transform("rotate (1.5707963267949)").unwrap(),
        1.0,
        0.0,
    );
    assert!(rx.abs() < 1e-9 && (ry + 1.0).abs() < 1e-9);
    let (ux, uy) = units::apply(m, 1.0, 0.0);
    assert!(close(ux, 10.0 * EMU_CM) && close(uy, 5.0 * EMU_CM - 1.0));
}

#[test]
fn a_rotated_shape_keeps_its_size_and_turns_about_its_corner() {
    let shape = r#"<draw:custom-shape draw:style-name="gr1" svg:width="4cm" svg:height="2cm" draw:transform="rotate (0.5235987755983) translate (10cm 5cm)"><draw:enhanced-geometry svg:viewBox="0 0 21600 21600" draw:enhanced-path="M 0 0 L 21600 0 21600 21600 0 21600 Z N"/></draw:custom-shape>"#;
    let s = only_shape(&op(&slide_page(shape)).auto(&filled()));
    assert!(close(s.xfrm.w, 4.0 * EMU_CM) && close(s.xfrm.h, 2.0 * EMU_CM));
    assert!((s.xfrm.rot_deg + 30.0).abs() < 1e-6, "{}", s.xfrm.rot_deg);
    // the top-left corner is at (10, 5) cm; the centre is that plus the turned half diagonal
    let (sn, cs) = (-std::f64::consts::FRAC_PI_6).sin_cos();
    let (hx, hy) = (2.0 * EMU_CM, 1.0 * EMU_CM);
    let (cx, cy) = (
        10.0 * EMU_CM + cs * hx - sn * hy,
        5.0 * EMU_CM + sn * hx + cs * hy,
    );
    assert!(close(s.xfrm.x + s.xfrm.w / 2.0, cx), "{} {cx}", s.xfrm.x);
    assert!(close(s.xfrm.y + s.xfrm.h / 2.0, cy));
}

#[test]
fn x_y_are_the_unrotated_position_a_scale_scales_and_a_negative_scale_flips() {
    let shape = r#"<draw:rect draw:style-name="gr1" svg:x="1cm" svg:y="1cm" svg:width="2cm" svg:height="2cm" draw:transform="scale (2 -1) translate (1cm 0cm)"/>"#;
    let s = only_shape(&op(&slide_page(shape)).auto(&filled()));
    assert!(close(s.xfrm.w, 4.0 * EMU_CM) && close(s.xfrm.h, 2.0 * EMU_CM));
    assert!(s.xfrm.flip_v && !s.xfrm.flip_h);
    // scaled first (x: 1..3 becomes 2..6), then moved by 1 cm: 3 .. 7 cm
    assert!(close(s.xfrm.x, 3.0 * EMU_CM));
}

#[test]
fn matrix_and_translate_only() {
    let shape = r#"<draw:rect draw:style-name="gr1" svg:width="2cm" svg:height="1cm" draw:transform="matrix (1 0 0 1 3cm 4cm)"/>"#;
    let s = only_shape(&op(&slide_page(shape)).auto(&filled()));
    assert!(close(s.xfrm.x, 3.0 * EMU_CM) && close(s.xfrm.y, 4.0 * EMU_CM));
}

#[test]
fn a_skew_is_dropped_and_a_collapsing_matrix_draws_nothing() {
    let skew = r#"<draw:rect draw:style-name="gr1" svg:width="2cm" svg:height="1cm" draw:transform="skewX (0.3) translate (3cm 4cm)"/>"#;
    let s = only_shape(&op(&slide_page(skew)).auto(&filled()));
    assert!(close(s.xfrm.w, 2.0 * EMU_CM));
    let flat = r#"<draw:rect draw:style-name="gr1" svg:width="2cm" svg:height="1cm" draw:transform="scale (0 1)"/>"#;
    assert!(scene(&op(&slide_page(flat)).auto(&filled()))
        .items
        .is_empty());
}

#[test]
fn a_broken_transform_leaves_the_shape_unplaced_without_a_position() {
    let shape = r#"<draw:rect draw:style-name="gr1" svg:width="2cm" svg:height="1cm" draw:transform="rotate (zzz)"/>"#;
    assert!(scene(&op(&slide_page(shape)).auto(&filled()))
        .items
        .is_empty());
}

#[test]
fn ellipse_circle_and_arcs() {
    let el = r#"<draw:ellipse draw:style-name="gr1" svg:x="1cm" svg:y="1cm" svg:width="4cm" svg:height="2cm"/>"#;
    assert!(matches!(
        only_shape(&op(&slide_page(el)).auto(&filled())).geom,
        Geometry::Ellipse
    ));
    let ci = r#"<draw:circle draw:style-name="gr1" svg:x="1cm" svg:y="1cm" svg:width="2cm" svg:height="2cm"/>"#;
    assert!(matches!(
        only_shape(&op(&slide_page(ci)).auto(&filled())).geom,
        Geometry::Ellipse
    ));
    for (kind, has_line_to, closed, filled_path) in [
        ("section", true, true, true),
        ("cut", false, true, true),
        ("arc", false, false, false),
    ] {
        let sh = format!(
            r#"<draw:ellipse draw:style-name="gr1" svg:x="1cm" svg:y="1cm" svg:width="4cm" svg:height="4cm" draw:kind="{kind}" draw:start-angle="0" draw:end-angle="90"/>"#
        );
        let s = only_shape(&op(&slide_page(&sh)).auto(&filled()));
        let Geometry::Paths(p) = s.geom else {
            panic!("{kind}")
        };
        let cmds = &p[0].cmds;
        assert_eq!(
            cmds.iter().any(|c| matches!(c, sd::PathCmd::LineTo(_))),
            has_line_to
        );
        assert_eq!(cmds.iter().any(|c| matches!(c, sd::PathCmd::Close)), closed);
        assert_eq!(p[0].fill_mode == sd::PathFill::Norm, filled_path);
        let sd::PathCmd::MoveTo(p0) = cmds[0] else {
            panic!()
        };
        // starts at three o'clock
        assert!(
            close(p0.x, 4.0 * EMU_CM) && close(p0.y, 2.0 * EMU_CM),
            "{p0:?}"
        );
        let sd::PathCmd::ArcTo { sw_deg, st_deg, .. } = cmds[1] else {
            panic!()
        };
        // counter-clockwise on the page is negative in the model
        assert!((sw_deg + 90.0).abs() < 1e-9 && st_deg.abs() < 1e-9);
    }
}

#[test]
fn lines_flip_to_keep_their_direction() {
    let l = |x1: f64, y1: f64, x2: f64, y2: f64| {
        format!(
            r#"<draw:line draw:style-name="gr1" svg:x1="{x1}cm" svg:y1="{y1}cm" svg:x2="{x2}cm" svg:y2="{y2}cm"/>"#
        )
    };
    for ((x1, y1, x2, y2), fh, fv) in [
        ((1.0, 1.0, 5.0, 3.0), false, false),
        ((5.0, 1.0, 1.0, 3.0), true, false),
        ((1.0, 3.0, 5.0, 1.0), false, true),
        ((5.0, 3.0, 1.0, 1.0), true, true),
    ] {
        let s = only_shape(&op(&slide_page(&l(x1, y1, x2, y2))).auto(&filled()));
        assert!(matches!(s.geom, Geometry::Line));
        assert_eq!((s.xfrm.flip_h, s.xfrm.flip_v), (fh, fv));
        assert!(close(s.xfrm.w, 4.0 * EMU_CM) && close(s.xfrm.h, 2.0 * EMU_CM));
        // a line is never filled
        assert_eq!(s.fill, Fill::None);
    }
    // a horizontal line has no height
    let s = only_shape(&op(&slide_page(&l(1.0, 1.0, 5.0, 1.0))).auto(&filled()));
    assert!(s.xfrm.h == 0.0);
}

#[test]
fn polylines_polygons_and_paths_use_their_view_box() {
    let pl = r#"<draw:polyline draw:style-name="gr1" svg:x="2cm" svg:y="1cm" svg:width="4cm" svg:height="2cm" svg:viewBox="0 0 400 200" draw:points="0,0 400,200 0,200"/>"#;
    let s = only_shape(&op(&slide_page(pl)).auto(&filled()));
    let Geometry::Paths(p) = &s.geom else {
        panic!()
    };
    assert_eq!((p[0].w, p[0].h), (400.0, 200.0));
    assert_eq!(p[0].cmds.len(), 3);
    assert_eq!(p[0].fill_mode, sd::PathFill::None);
    assert_eq!(s.fill, Fill::None);
    let pg = pl.replace("polyline", "polygon");
    let s = only_shape(&op(&slide_page(&pg)).auto(&filled()));
    let Geometry::Paths(p) = &s.geom else {
        panic!()
    };
    assert!(matches!(p[0].cmds.last(), Some(sd::PathCmd::Close)));
    assert_ne!(s.fill, Fill::None);
    // a view box with an origin is shifted to zero
    let path = r#"<draw:path draw:style-name="gr1" svg:x="2cm" svg:y="1cm" svg:width="4cm" svg:height="2cm" svg:viewBox="100 100 400 200" svg:d="M100 100L500 100L500 300z"/>"#;
    let s = only_shape(&op(&slide_page(path)).auto(&filled()));
    let Geometry::Paths(p) = &s.geom else {
        panic!()
    };
    assert_eq!(p[0].cmds[0], sd::PathCmd::MoveTo(sd::Pt::new(0.0, 0.0)));
    assert_eq!(p[0].cmds[1], sd::PathCmd::LineTo(sd::Pt::new(400.0, 0.0)));
}

#[test]
fn regular_polygons_and_stars() {
    let rp = |extra: &str| {
        format!(
            r#"<draw:regular-polygon draw:style-name="gr1" svg:x="1cm" svg:y="1cm" svg:width="4cm" svg:height="4cm" draw:corners="5" {extra}/>"#
        )
    };
    let s = only_shape(&op(&slide_page(&rp(""))).auto(&filled()));
    let Geometry::Paths(p) = &s.geom else {
        panic!()
    };
    // five corners, a close; the first corner is at the top middle
    assert_eq!(p[0].cmds.len(), 6);
    let sd::PathCmd::MoveTo(top) = p[0].cmds[0] else {
        panic!()
    };
    assert!(close(top.x, 2.0 * EMU_CM) && close(top.y, 0.0), "{top:?}");
    let s = only_shape(
        &op(&slide_page(&rp(
            r#"draw:concave="true" draw:sharpness="50%""#,
        )))
        .auto(&filled()),
    );
    let Geometry::Paths(p) = &s.geom else {
        panic!()
    };
    assert_eq!(p[0].cmds.len(), 11);
    let sd::PathCmd::LineTo(inner) = p[0].cmds[1] else {
        panic!()
    };
    let r = ((inner.x - 2.0 * EMU_CM).powi(2) + (inner.y - 2.0 * EMU_CM).powi(2)).sqrt();
    assert!((r - EMU_CM).abs() < 2.0, "{r}");
}

#[test]
fn custom_shapes_use_their_enhanced_path() {
    let sh = r#"<draw:custom-shape draw:style-name="gr1" svg:x="1cm" svg:y="1cm" svg:width="4cm" svg:height="2cm"><draw:enhanced-geometry svg:viewBox="0 0 21600 21600" draw:type="rectangle" draw:enhanced-path="M 0 0 L 21600 0 21600 21600 0 21600 Z N"/></draw:custom-shape>"#;
    let s = only_shape(&op(&slide_page(sh)).auto(&filled()));
    assert!(matches!(s.geom, Geometry::Paths(_)));
}

#[test]
fn ooxml_shapes_without_a_path_are_drawn_from_the_preset() {
    let sh = |ty: &str, m: &str| {
        format!(
            r#"<draw:custom-shape draw:style-name="gr1" svg:x="1cm" svg:y="1cm" svg:width="4cm" svg:height="2cm"><draw:enhanced-geometry draw:type="{ty}" {m}/></draw:custom-shape>"#
        )
    };
    let s = only_shape(&op(&slide_page(&sh("ooxml-rightArrow", ""))).auto(&filled()));
    let Geometry::Paths(p) = &s.geom else {
        panic!()
    };
    assert!(!p.is_empty());
    // the modifiers are the adjust values in order: a thinner shaft gives a different outline
    let a = only_shape(
        &op(&slide_page(&sh(
            "ooxml-rightArrow",
            r#"draw:modifiers="20000 50000""#,
        )))
        .auto(&filled()),
    );
    let b = only_shape(
        &op(&slide_page(&sh(
            "ooxml-rightArrow",
            r#"draw:modifiers="80000 50000""#,
        )))
        .auto(&filled()),
    );
    assert_ne!(a.geom, b.geom);
    // a legacy name through the table
    let l = only_shape(&op(&slide_page(&sh("diamond", ""))).auto(&filled()));
    assert!(matches!(l.geom, Geometry::Paths(_)));
    // an unknown name is the box
    let u = only_shape(&op(&slide_page(&sh("no-such-shape", ""))).auto(&filled()));
    assert!(matches!(u.geom, Geometry::Rect));
    // a shape with no geometry element at all is the box too
    let n = r#"<draw:custom-shape draw:style-name="gr1" svg:x="1cm" svg:y="1cm" svg:width="4cm" svg:height="2cm"/>"#;
    assert!(matches!(
        only_shape(&op(&slide_page(n)).auto(&filled())).geom,
        Geometry::Rect
    ));
}

#[test]
fn legacy_names_map_to_presets_that_exist() {
    for name in [
        "rectangle",
        "round-rectangle",
        "ellipse",
        "diamond",
        "isosceles-triangle",
        "right-triangle",
        "parallelogram",
        "trapezoid",
        "hexagon",
        "octagon",
        "pentagon",
        "cross",
        "ring",
        "can",
        "cube",
        "paper",
        "star4",
        "star5",
        "star6",
        "star8",
        "star12",
        "star24",
        "heart",
        "sun",
        "moon",
        "cloud",
        "lightning-bolt",
        "smiley",
        "forbidden",
        "bevel",
        "frame",
        "right-arrow",
        "left-arrow",
        "down-arrow",
        "left-right-arrow",
        "up-down-arrow",
        "chevron",
        "left-bracket",
        "right-bracket",
        "left-brace",
        "right-brace",
        "flowchart-process",
        "flowchart-decision",
        "flowchart-terminator",
        "flowchart-document",
        "flowchart-connector",
        "rectangular-callout",
        "round-rectangular-callout",
        "round-callout",
    ] {
        let p = legacy_preset(name).unwrap_or_else(|| panic!("{name}"));
        assert!(
            sd::geom::preset(p, &[], 100.0, 100.0).is_some(),
            "{name} -> {p}"
        );
    }
    assert!(legacy_preset("zzz").is_none());
}

#[test]
fn connectors() {
    let c = |attrs: &str| {
        format!(
            r#"<draw:connector draw:style-name="gr1" svg:x1="1cm" svg:y1="1cm" svg:x2="9cm" svg:y2="5cm" {attrs}/>"#
        )
    };
    // with svg:d
    let with_d = c(
        r#"svg:x="1cm" svg:y="1cm" svg:width="8cm" svg:height="4cm" svg:viewBox="0 0 800 400" svg:d="M0 0L400 0L400 400L800 400""#,
    );
    let s = only_shape(&op(&slide_page(&with_d)).auto(&filled()));
    let Geometry::Paths(p) = &s.geom else {
        panic!()
    };
    assert_eq!(p[0].cmds.len(), 4);
    assert_eq!(s.fill, Fill::None);
    // straight
    let s = only_shape(&op(&slide_page(&c(r#"draw:type="line""#))).auto(&filled()));
    assert!(matches!(s.geom, Geometry::Line));
    // standard: an elbow of three segments
    let s = only_shape(&op(&slide_page(&c(r#"draw:type="standard""#))).auto(&filled()));
    let Geometry::Paths(p) = &s.geom else {
        panic!()
    };
    assert_eq!(p[0].cmds.len(), 4);
    // curve
    let s = only_shape(&op(&slide_page(&c(r#"draw:type="curve""#))).auto(&filled()));
    let Geometry::Paths(p) = &s.geom else {
        panic!()
    };
    assert!(matches!(p[0].cmds[1], sd::PathCmd::CubicTo(..)));
    // aligned ends are a straight line
    let straight = r#"<draw:connector draw:style-name="gr1" svg:x1="1cm" svg:y1="1cm" svg:x2="9cm" svg:y2="1cm"/>"#;
    let s = only_shape(&op(&slide_page(straight)).auto(&filled()));
    assert!(matches!(s.geom, Geometry::Line));
}

#[test]
fn measures_are_drawn_as_their_line() {
    let m = r#"<draw:measure draw:style-name="gr1" svg:x1="1cm" svg:y1="1cm" svg:x2="9cm" svg:y2="1cm"><text:p>8 cm</text:p></draw:measure>"#;
    let s = only_shape(&op(&slide_page(m)).auto(&filled()));
    assert!(matches!(s.geom, Geometry::Line));
}

#[test]
fn rounded_rectangles() {
    let s = only_shape(&op(&slide_page(&rect(r#"draw:corner-radius="0.5cm""#))).auto(&filled()));
    assert!(matches!(s.geom, Geometry::Paths(_)));
    // a radius bigger than half the side is capped, not a failure
    let s = only_shape(&op(&slide_page(&rect(r#"draw:corner-radius="50cm""#))).auto(&filled()));
    assert!(matches!(s.geom, Geometry::Paths(_)));
}

#[test]
fn groups_hold_their_children_in_page_coordinates() {
    let g = format!(
        "<draw:g>{}{}</draw:g>",
        rect(""),
        r#"<draw:ellipse draw:style-name="gr1" svg:x="9cm" svg:y="1cm" svg:width="2cm" svg:height="2cm"/>"#
    );
    let sc = scene(&op(&slide_page(&g)).auto(&filled()));
    let [Item::Group(gr_)] = &sc.items[..] else {
        panic!("{:?}", sc.items)
    };
    assert_eq!(gr_.items.len(), 2);
    assert_eq!(gr_.child_off, (0.0, 0.0));
    assert_eq!((gr_.child_ext.0, gr_.child_ext.1), (sc.width, sc.height));
    assert_eq!(gr_.xfrm, sd::Xfrm::rect(0.0, 0.0, sc.width, sc.height));
    // empty groups vanish
    assert!(scene(&op(&slide_page("<draw:g/>"))).items.is_empty());
}

#[test]
fn z_index_orders_the_shapes() {
    let a = r#"<draw:rect draw:style-name="gr1" draw:z-index="2" svg:x="1cm" svg:y="1cm" svg:width="1cm" svg:height="1cm"/>"#;
    let b = r#"<draw:rect draw:style-name="gr1" draw:z-index="1" svg:x="5cm" svg:y="1cm" svg:width="1cm" svg:height="1cm"/>"#;
    let sc = scene(&op(&slide_page(&format!("{a}{b}"))).auto(&filled()));
    let v = shapes_of(&sc);
    assert!(close(v[0].xfrm.x, 5.0 * EMU_CM) && close(v[1].xfrm.x, EMU_CM));
}

// ---------------------------------------------------------------------------------------------
// masters
// ---------------------------------------------------------------------------------------------

fn master_shapes() -> String {
    format!(
        r#"{}<draw:frame draw:layer="backgroundobjects" svg:x="1cm" svg:y="14cm" svg:width="5cm" svg:height="1cm" presentation:class="footer"><draw:text-box><text:p><presentation:footer/></text:p></draw:text-box></draw:frame><draw:frame draw:layer="backgroundobjects" svg:x="20cm" svg:y="14cm" svg:width="5cm" svg:height="1cm" presentation:class="page-number"><draw:text-box><text:p><text:page-number>&lt;number&gt;</text:page-number></text:p></draw:text-box></draw:frame><draw:frame draw:layer="backgroundobjects" svg:x="1cm" svg:y="1cm" svg:width="5cm" svg:height="1cm" presentation:class="title" presentation:placeholder="true"><draw:text-box><text:p>Click to edit</text:p></draw:text-box></draw:frame>"#,
        r#"<draw:rect draw:style-name="gr1" svg:x="0cm" svg:y="0cm" svg:width="28cm" svg:height="0.5cm"/>"#
    )
}

fn master_op(pages: &str, dp: &str) -> Op {
    Op::new(pages)
        .styles_auto(LAYOUT)
        .master(&master_with(&master_shapes(), MDP))
        .styles(r#"<style:style style:name="Mdp1" style:family="drawing-page"/>"#)
        .auto(&format!("{}{dp}", filled()))
}

#[test]
fn master_shapes_are_under_the_slides_and_placeholders_are_not_drawn() {
    let dp = r#"<style:style style:name="dp1" style:family="drawing-page"><style:drawing-page-properties presentation:display-footer="true" presentation:display-page-number="false" presentation:use-footer-name="ftr1"/></style:style>"#;
    let own = rect("");
    let decl =
        r#"<presentation:footer-decl presentation:name="ftr1">Foot</presentation:footer-decl>"#;
    let sc = scene(&master_op(&format!("{decl}{}", slide_page(&own)), dp));
    let v = shapes_of(&sc);
    // the master's bar, its footer (shown), then the slide's rectangle; no title placeholder, no page number
    assert_eq!(v.len(), 3, "{v:#?}");
    assert!(close(v[0].xfrm.h, 0.5 * EMU_CM));
    assert!(close(v[2].xfrm.x, 2.0 * EMU_CM));
    assert!(v.iter().all(|s| text_of_shape(s) != "Click to edit"));
}

#[test]
fn the_footer_and_page_number_fields_take_the_slides_values() {
    let dp = r#"<style:style style:name="dp1" style:family="drawing-page"><style:drawing-page-properties presentation:display-footer="true" presentation:display-page-number="true" presentation:use-footer-name="ftr1"/></style:style>"#;
    let decl = r#"<presentation:footer-decl presentation:name="ftr1">Company confidential</presentation:footer-decl>"#;
    let pages = format!("{decl}{}{}", slide_page(""), slide_page(""));
    let ds = doc(&master_op(&pages, dp));
    for (i, sc) in ds.slide_scenes.iter().enumerate() {
        let texts: Vec<String> = shapes_of(sc).iter().map(|s| text_of_shape(s)).collect();
        assert!(
            texts.contains(&"Company confidential".to_string()),
            "{texts:?}"
        );
        assert!(texts.contains(&(i + 1).to_string()), "{texts:?}");
    }
}

#[test]
fn display_flags_switch_the_footer_line_off() {
    let dp = r#"<style:style style:name="dp1" style:family="drawing-page"><style:drawing-page-properties presentation:display-footer="false" presentation:display-page-number="false"/></style:style>"#;
    let sc = scene(&master_op(&slide_page(""), dp));
    assert_eq!(shapes_of(&sc).len(), 1);
}

#[test]
fn master_objects_can_be_hidden_by_the_slide() {
    let dp = r#"<style:style style:name="dp1" style:family="drawing-page"><style:drawing-page-properties presentation:background-objects-visible="false"/></style:style>"#;
    let sc = scene(&master_op(&slide_page(""), dp));
    assert!(sc.items.is_empty());
}

#[test]
fn a_placeholder_without_a_position_takes_the_masters_and_an_empty_one_is_not_drawn() {
    let master = r#"<draw:frame svg:x="3cm" svg:y="4cm" svg:width="10cm" svg:height="2cm" presentation:class="title" presentation:placeholder="true"><draw:text-box/></draw:frame>"#;
    let o = Op::new(&slide_page(
        r#"<draw:frame presentation:class="title"><draw:text-box><text:p>Hello</text:p></draw:text-box></draw:frame><draw:frame svg:x="1cm" svg:y="1cm" svg:width="3cm" svg:height="1cm" presentation:class="outline" presentation:placeholder="true"><draw:text-box/></draw:frame>"#,
    ))
    .styles_auto(LAYOUT)
    .master(&master_with(master, MDP))
    .styles(r#"<style:style style:name="Mdp1" style:family="drawing-page"/>"#);
    let s = only_shape(&o);
    assert!(close(s.xfrm.x, 3.0 * EMU_CM) && close(s.xfrm.w, 10.0 * EMU_CM));
    assert_eq!(text_of_shape(&s), "Hello");
}

// ---------------------------------------------------------------------------------------------
// text
// ---------------------------------------------------------------------------------------------

fn tb(inner: &str) -> String {
    format!(
        r#"<draw:frame draw:style-name="gr1" svg:x="1cm" svg:y="1cm" svg:width="20cm" svg:height="5cm"><draw:text-box>{inner}</draw:text-box></draw:frame>"#
    )
}

fn text_op(inner: &str, auto: &str) -> Op {
    op(&slide_page(&tb(inner))).auto(&format!(
        "{}{auto}",
        gr(r#"draw:fill="none" draw:stroke="none" fo:padding-left="0.1cm" fo:padding-top="0.2cm" fo:padding-right="0.3cm" fo:padding-bottom="0.4cm""#)
    ))
}

#[test]
fn a_text_box_is_a_rectangle_with_a_text_body() {
    let s = only_shape(&text_op("<text:p>Hi</text:p>", ""));
    assert_eq!(text_of_shape(&s), "Hi");
    let t = s.text.unwrap();
    assert_eq!(t.insets, (36_000.0, 72_000.0, 108_000.0, 144_000.0));
    assert!(t.wrap);
    assert_eq!(t.anchor, sd::Anchor::Top);
    assert!(matches!(s.geom, Geometry::Rect));
}

#[test]
fn an_empty_text_box_draws_nothing() {
    assert!(scene(&text_op("", "")).items.is_empty());
    assert!(scene(&text_op("<text:p/>", "")).items.is_empty());
}

#[test]
fn libreoffices_text_distances_hold_when_no_style_states_them() {
    let o = op(&slide_page(&tb("<text:p>x</text:p>"))).auto(&gr(r#"draw:fill="none""#));
    let t = only_shape(&o).text.unwrap();
    assert_eq!(t.insets, (90_000.0, 45_000.0, 90_000.0, 45_000.0));
}

#[test]
fn body_properties_anchor_wrap_autofit_and_vertical_text() {
    let g = |p: &str| {
        let o = op(&slide_page(&tb("<text:p>x</text:p>")))
            .auto(&gr(&format!(r#"draw:fill="none" {p}"#)));
        only_shape(&o).text.unwrap()
    };
    assert_eq!(
        g(r#"draw:textarea-vertical-align="middle""#).anchor,
        sd::Anchor::Middle
    );
    assert_eq!(
        g(r#"draw:textarea-vertical-align="bottom""#).anchor,
        sd::Anchor::Bottom
    );
    assert!(!g(r#"fo:wrap-option="no-wrap""#).wrap);
    assert!(g(r#"draw:textarea-horizontal-align="center""#).anchor_ctr);
    assert_eq!(
        g(r#"draw:auto-grow-height="true""#).autofit,
        sd::AutoFit::Shape
    );
    assert!(matches!(
        g(r#"style:shrink-to-fit="true""#).autofit,
        sd::AutoFit::Normal { .. }
    ));
    let o = op(&slide_page(&tb("<text:p>x</text:p>"))).auto(&format!(
        r#"{}<style:style style:name="gr1" style:family="graphic"><style:graphic-properties draw:fill="none"/><style:paragraph-properties style:writing-mode="tb-rl"/></style:style>"#,
        ""
    ));
    assert_eq!(only_shape(&o).text.unwrap().vert, sd::Vert::EaVert);
}

#[test]
fn shrink_to_fit_shrinks_text_that_overflows() {
    let long = "word ".repeat(400);
    let o = op(&slide_page(&format!(
        r#"<draw:frame draw:style-name="gr1" svg:x="1cm" svg:y="1cm" svg:width="6cm" svg:height="2cm"><draw:text-box><text:p text:style-name="P1">{long}</text:p></draw:text-box></draw:frame>"#
    )))
    .auto(&format!(
        r#"{}<style:style style:name="P1" style:family="paragraph"><style:text-properties fo:font-size="24pt"/></style:style>"#,
        gr(r#"draw:fill="none" style:shrink-to-fit="true""#)
    ));
    let t = only_shape(&o).text.unwrap();
    let sd::AutoFit::Normal { font_scale, .. } = t.autofit else {
        panic!()
    };
    assert!((0.25..0.9).contains(&font_scale), "{font_scale}");
    // text that fits keeps scale 1
    let o = op(&slide_page(&tb("<text:p>short</text:p>")))
        .auto(&gr(r#"draw:fill="none" style:shrink-to-fit="true""#));
    let sd::AutoFit::Normal { font_scale, .. } = only_shape(&o).text.unwrap().autofit else {
        panic!()
    };
    assert_eq!(font_scale, 1.0);
}

#[test]
fn fit_to_size_scales_the_text_to_the_box() {
    let o = op(&slide_page(
        r#"<draw:frame draw:style-name="gr1" svg:x="1cm" svg:y="1cm" svg:width="20cm" svg:height="10cm"><draw:text-box><text:p text:style-name="P1">Big</text:p></draw:text-box></draw:frame>"#,
    ))
    .auto(&format!(
        r#"{}<style:style style:name="P1" style:family="paragraph"><style:text-properties fo:font-size="10pt"/></style:style>"#,
        gr(r#"draw:fill="none" draw:fit-to-size="true""#)
    ));
    let t = only_shape(&o).text.unwrap();
    assert!(
        t.paragraphs[0].runs[0].size_pt > 40.0,
        "{}",
        t.paragraphs[0].runs[0].size_pt
    );
}

#[test]
fn runs_carry_their_character_properties() {
    let auto = r##"<style:style style:name="T1" style:family="text"><style:text-properties fo:font-weight="bold" fo:font-style="italic" style:text-underline-style="solid" style:text-line-through-style="solid" fo:color="#ff0000" fo:font-size="150%" style:font-name="Liberation Serif" fo:letter-spacing="0.1cm" fo:background-color="#ffff00" fo:font-variant="small-caps"/></style:style><style:style style:name="P1" style:family="paragraph"><style:text-properties fo:font-size="20pt"/></style:style>"##;
    let fonts = r#"<style:font-face style:name="Liberation Serif" svg:font-family="&apos;Liberation Serif&apos;"/>"#;
    let _ = fonts;
    let o = text_op(
        r#"<text:p text:style-name="P1">a <text:span text:style-name="T1">b</text:span></text:p>"#,
        auto,
    );
    let s = only_shape(&o);
    let runs = &s.text.unwrap().paragraphs[0].runs;
    assert_eq!(runs.len(), 2);
    let b = &runs[1];
    assert!(b.bold && b.italic);
    assert_eq!(b.underline, sd::Underline::Single);
    assert_eq!(b.strike, sd::Strike::Single);
    assert_eq!(b.fill, Fill::Solid(Rgba::rgb(255, 0, 0)));
    // 150 % of the paragraph's 20 pt
    assert!((b.size_pt - 30.0).abs() < 1e-9, "{}", b.size_pt);
    assert_eq!(b.font.latin.as_deref(), Some("Liberation Serif"));
    assert!((b.spacing_pt - 0.1 * 360_000.0 / 12_700.0).abs() < 1e-6);
    assert_eq!(b.highlight, Some(Rgba::rgb(255, 255, 0)));
    assert_eq!(b.caps, sd::Caps::Small);
    assert!(!runs[0].bold);
    assert_eq!(runs[0].size_pt, 20.0);
}

#[test]
fn font_faces_are_resolved_and_asian_runs_use_the_asian_size() {
    let o = Op::new(&slide_page(&tb(
        r#"<text:p text:style-name="P1">日本語</text:p><text:p text:style-name="P1">abc</text:p>"#,
    )))
    .styles_auto(LAYOUT)
    .master(&master_with("", MDP))
    .styles(r#"<style:style style:name="Mdp1" style:family="drawing-page"/>"#)
    .auto(&format!(
        r#"{}<style:style style:name="P1" style:family="paragraph"><style:text-properties fo:font-size="10pt" style:font-size-asian="30pt" style:font-name-asian="Noto Sans CJK JP" fo:font-weight="normal" style:font-weight-asian="bold"/></style:style>"#,
        gr(r#"draw:fill="none""#)
    ));
    let s = only_shape(&o);
    let ps = &s.text.unwrap().paragraphs;
    assert_eq!(ps[0].runs[0].size_pt, 30.0);
    assert!(ps[0].runs[0].bold);
    assert_eq!(
        ps[0].runs[0].font.east_asian.as_deref(),
        Some("Noto Sans CJK JP")
    );
    assert_eq!(ps[1].runs[0].size_pt, 10.0);
    assert!(!ps[1].runs[0].bold);
}

#[test]
fn super_and_subscript_and_text_transform() {
    let auto = r#"<style:style style:name="T1" style:family="text"><style:text-properties style:text-position="super 58%"/></style:style><style:style style:name="T2" style:family="text"><style:text-properties style:text-position="sub 58%"/></style:style><style:style style:name="T3" style:family="text"><style:text-properties fo:text-transform="uppercase"/></style:style><style:style style:name="T4" style:family="text"><style:text-properties fo:text-transform="capitalize"/></style:style><style:style style:name="T5" style:family="text"><style:text-properties style:text-position="-25% 58%"/></style:style>"#;
    let o = text_op(
        r#"<text:p><text:span text:style-name="T1">a</text:span><text:span text:style-name="T2">b</text:span><text:span text:style-name="T3">c</text:span><text:span text:style-name="T4">hello world</text:span><text:span text:style-name="T5">d</text:span></text:p>"#,
        auto,
    );
    let r = only_shape(&o).text.unwrap().paragraphs.remove(0).runs;
    assert_eq!(r[0].baseline_pct, 33.0);
    assert_eq!(r[1].baseline_pct, -33.0);
    assert_eq!(r[2].caps, sd::Caps::All);
    assert_eq!(r[3].text, "Hello World");
    assert_eq!(r[4].baseline_pct, -25.0);
}

#[test]
fn white_space_spaces_tabs_and_breaks() {
    let o = text_op(
        "<text:p>  a \n   b<text:s text:c=\"3\"/>c<text:tab/>d<text:line-break/>e  </text:p>",
        "",
    );
    let s = only_shape(&o);
    let p = &s.text.unwrap().paragraphs[0];
    let text: String = p
        .runs
        .iter()
        .map(|r| {
            if r.kind == sd::RunKind::LineBreak {
                "|".to_string()
            } else {
                r.text.clone()
            }
        })
        .collect();
    assert_eq!(text, "a b   c    d|e");
}

#[test]
fn paragraph_properties() {
    let auto = r#"<style:style style:name="P1" style:family="paragraph"><style:paragraph-properties fo:text-align="center" fo:margin-left="1cm" fo:text-indent="-0.5cm" fo:margin-top="0.2cm" fo:margin-bottom="0.1cm" fo:line-height="150%"/></style:style><style:style style:name="P2" style:family="paragraph"><style:paragraph-properties fo:text-align="end" fo:line-height="1cm" style:writing-mode="rl-tb"/></style:style><style:style style:name="P3" style:family="paragraph"><style:paragraph-properties fo:text-align="justify"/></style:style>"#;
    let o = text_op(
        r#"<text:p text:style-name="P1">a</text:p><text:p text:style-name="P2">b</text:p><text:p text:style-name="P3">c</text:p>"#,
        auto,
    );
    let ps = only_shape(&o).text.unwrap().paragraphs;
    assert_eq!(ps[0].align, sd::Align::Center);
    assert!(close(ps[0].mar_l, EMU_CM) && close(ps[0].indent, -180_000.0));
    assert_eq!(ps[0].line_spacing, sd::Spacing::Pct(1.5));
    assert!(
        matches!(ps[0].spc_before, sd::Spacing::Pts(p) if (p - 0.2 * 360_000.0 / 12_700.0).abs() < 1e-9)
    );
    assert_eq!(ps[1].align, sd::Align::Left); // "end" of a right-to-left paragraph
    assert!(ps[1].rtl);
    assert!(matches!(ps[1].line_spacing, sd::Spacing::Pts(_)));
    assert_eq!(ps[2].align, sd::Align::Justify);
}

#[test]
fn automatic_text_colour_follows_the_background() {
    let dark = gr(r##"draw:fill="solid" draw:fill-color="#101010""##);
    let o = op(&slide_page(&tb("<text:p>x</text:p>"))).auto(&dark);
    let r = only_shape(&o)
        .text
        .unwrap()
        .paragraphs
        .remove(0)
        .runs
        .remove(0);
    assert_eq!(r.fill, Fill::Solid(Rgba::WHITE));
    let light = gr(r##"draw:fill="solid" draw:fill-color="#f0f0f0""##);
    let o = op(&slide_page(&tb("<text:p>x</text:p>"))).auto(&light);
    let r = only_shape(&o)
        .text
        .unwrap()
        .paragraphs
        .remove(0)
        .runs
        .remove(0);
    assert_eq!(r.fill, Fill::Solid(Rgba::BLACK));
    // an explicit colour is kept
    let o = op(&slide_page(&tb(r#"<text:p text:style-name="P1">x</text:p>"#)))
        .auto(&format!(
            r##"{dark}<style:style style:name="P1" style:family="paragraph"><style:text-properties fo:color="#00ff00"/></style:style>"##
        ));
    let r = only_shape(&o)
        .text
        .unwrap()
        .paragraphs
        .remove(0)
        .runs
        .remove(0);
    assert_eq!(r.fill, Fill::Solid(Rgba::rgb(0, 255, 0)));
    // on a dark slide background without a fill on the shape
    let styles = r##"<style:style style:name="Mdp1" style:family="drawing-page"><style:drawing-page-properties draw:fill="solid" draw:fill-color="#000000"/></style:style>"##;
    let o = op(&slide_page(&tb("<text:p>x</text:p>")))
        .styles(styles)
        .auto(&gr(r#"draw:fill="none""#));
    let r = only_shape(&o)
        .text
        .unwrap()
        .paragraphs
        .remove(0)
        .runs
        .remove(0);
    assert_eq!(r.fill, Fill::Solid(Rgba::WHITE));
}

const BULLETS: &str = r##"<text:list-style style:name="L1"><text:list-level-style-bullet text:level="1" text:bullet-char="●"><style:list-level-properties text:space-before="0.3cm" text:min-label-width="0.9cm"/><style:text-properties fo:font-family="OpenSymbol" fo:font-size="45%" fo:color="#ff0000"/></text:list-level-style-bullet><text:list-level-style-number text:level="2" style:num-suffix=")" style:num-format="a" text:start-value="3"><style:list-level-properties text:list-level-position-and-space-mode="label-alignment"><style:list-level-label-alignment text:label-followed-by="listtab" fo:text-indent="-1cm" fo:margin-left="2cm"/></style:list-level-properties></text:list-level-style-number><text:list-level-style-number text:level="3" style:num-prefix="(" style:num-suffix=")" style:num-format="I"/></text:list-style>"##;

#[test]
fn list_levels_bullets_numbers_and_indents() {
    let o = text_op(
        r#"<text:list text:style-name="L1"><text:list-item><text:p>one</text:p><text:list><text:list-item><text:p>two</text:p><text:list><text:list-item><text:p>three</text:p></text:list-item></text:list></text:list-item></text:list></text:list-item><text:list-header><text:p>head</text:p></text:list-header></text:list>"#,
        BULLETS,
    );
    let ps = only_shape(&o).text.unwrap().paragraphs;
    assert_eq!(ps.len(), 4);
    // level 1: a bullet, indent from the old-style properties
    let b = ps[0].bullet.as_ref().unwrap();
    assert_eq!(b.kind, sd::BulletKind::Char("●".into()));
    // 45 % of OpenSymbol's circle is drawn twice as big (see OPEN_SYMBOL_DOT_SCALE)
    assert_eq!(b.size, sd::BulletSize::Pct(0.9));
    assert_eq!(b.color, Some(Rgba::rgb(255, 0, 0)));
    assert!(close(ps[0].mar_l, 1.2 * EMU_CM) && close(ps[0].indent, -0.9 * EMU_CM));
    // level 2: a number with the label-alignment indent
    let b = ps[1].bullet.as_ref().unwrap();
    assert_eq!(
        b.kind,
        sd::BulletKind::AutoNum {
            scheme: "alphaLcParenR".into(),
            start: 3
        }
    );
    assert!(close(ps[1].mar_l, 2.0 * EMU_CM) && close(ps[1].indent, -EMU_CM));
    assert_eq!(ps[1].level, 1);
    // level 3: "(I)"
    assert_eq!(
        ps[2].bullet.as_ref().unwrap().kind,
        sd::BulletKind::AutoNum {
            scheme: "romanUcParenBoth".into(),
            start: 1
        }
    );
    // a list header has the indent and no bullet
    assert!(ps[3].bullet.is_none());
}

#[test]
fn the_paragraphs_own_margin_wins_over_the_label_alignment() {
    let auto = format!(
        r#"{BULLETS}<style:style style:name="P1" style:family="paragraph"><style:paragraph-properties fo:margin-left="5cm"/></style:style>"#
    );
    let o = text_op(
        r#"<text:list text:style-name="L1"><text:list-item><text:list><text:list-item><text:p text:style-name="P1">x</text:p></text:list-item></text:list></text:list-item></text:list>"#,
        &auto,
    );
    let ps = only_shape(&o).text.unwrap().paragraphs;
    assert!(close(ps[0].mar_l, 5.0 * EMU_CM));
}

#[test]
fn picture_bullets_and_the_other_numbering_formats() {
    let ls = r#"<text:list-style style:name="L1"><text:list-level-style-image text:level="1" xlink:href="Pictures/a.png"/><text:list-level-style-number text:level="2" style:num-format="A" style:num-suffix="."/><text:list-level-style-number text:level="3" style:num-format="i" style:num-suffix=":"/><text:list-level-style-number text:level="4" style:num-format="1"/></text:list-style>"#;
    let o = text_op(
        r#"<text:list text:style-name="L1"><text:list-item><text:p>a</text:p><text:list><text:list-item><text:p>b</text:p><text:list><text:list-item><text:p>c</text:p><text:list><text:list-item><text:p>d</text:p></text:list-item></text:list></text:list-item></text:list></text:list-item></text:list></text:list-item></text:list>"#,
        ls,
    )
    .picture("a.png", 7);
    let ps = only_shape(&o).text.unwrap().paragraphs;
    assert!(matches!(
        ps[0].bullet.as_ref().unwrap().kind,
        sd::BulletKind::Picture(_)
    ));
    let scheme = |i: usize| match &ps[i].bullet.as_ref().unwrap().kind {
        sd::BulletKind::AutoNum { scheme, .. } => scheme.clone(),
        k => panic!("{k:?}"),
    };
    assert_eq!(scheme(1), "alphaUcPeriod");
    assert_eq!(scheme(2), "romanLcPlain");
    assert_eq!(scheme(3), "arabicPlain");
}

#[test]
fn outline_levels_use_the_outline_styles_of_the_master() {
    let styles = r##"<style:style style:name="Default-outline1" style:family="presentation"><style:graphic-properties draw:fill="none"><text:list-style style:name="Default-outline1"><text:list-level-style-bullet text:level="1" text:bullet-char="●"><style:list-level-properties text:space-before="0.3cm" text:min-label-width="0.9cm"/></text:list-level-style-bullet><text:list-level-style-bullet text:level="2" text:bullet-char="–"><style:list-level-properties text:space-before="1.5cm" text:min-label-width="0.9cm"/></text:list-level-style-bullet></text:list-style></style:graphic-properties><style:text-properties fo:font-size="32pt"/></style:style><style:style style:name="Default-outline2" style:family="presentation" style:parent-style-name="Default-outline1"><style:text-properties fo:font-size="28pt"/></style:style>"##;
    let auto = r#"<style:style style:name="pr1" style:family="presentation" style:parent-style-name="Default-outline1"/>"#;
    let frame = r#"<draw:frame presentation:style-name="pr1" svg:x="1cm" svg:y="1cm" svg:width="20cm" svg:height="8cm" presentation:class="outline"><draw:text-box><text:list><text:list-item><text:p>one</text:p><text:list><text:list-item><text:p>two</text:p></text:list-item></text:list></text:list-item></text:list></draw:text-box></draw:frame>"#;
    let o = op(&slide_page(frame))
        .styles(&format!(
            "{styles}{}",
            r#"<style:style style:name="Mdp1" style:family="drawing-page"/>"#
        ))
        .auto(auto);
    let s = only_shape(&o);
    let ps = s.text.unwrap().paragraphs;
    assert_eq!(ps[0].runs[0].size_pt, 32.0);
    assert_eq!(ps[1].runs[0].size_pt, 28.0);
    // the bullets come from the list style nested in the presentation style
    assert_eq!(
        ps[1].bullet.as_ref().unwrap().kind,
        sd::BulletKind::Char("–".into())
    );
}

#[test]
fn page_number_date_and_other_fields() {
    let o = text_op(
        r#"<text:p>Page <text:page-number>1</text:page-number> of <text:page-count>9</text:page-count>, <text:date text:fixed="true">2020-01-02</text:date><text:note><text:note-body><text:p>foot</text:p></text:note-body></text:note></text:p>"#,
        "",
    );
    let pages = format!("{}{}", slide_page(&tb("<text:p>x</text:p>")), slide_page(&tb(
        "<text:p>Page <text:page-number>1</text:page-number> of <text:page-count>9</text:page-count>, <text:date text:fixed=\"true\">2020-01-02</text:date><text:note><text:note-body><text:p>foot</text:p></text:note-body></text:note></text:p>"
    )));
    let _ = o;
    let d = doc(&op(&pages).auto(&gr(r#"draw:fill="none""#)));
    let sc = &d.slide_scenes[1];
    let v = shapes_of(sc);
    assert_eq!(text_of_shape(v[0]), "Page 2 of 2, 2020-01-02");
    let kinds: Vec<sd::RunKind> = v[0].text.as_ref().unwrap().paragraphs[0]
        .runs
        .iter()
        .map(|r| r.kind)
        .collect();
    assert!(kinds.contains(&sd::RunKind::Field));
}

#[test]
fn hidden_text_and_unknown_inline_elements() {
    let auto = r#"<style:style style:name="T1" style:family="text"><style:text-properties text:display="none"/></style:style>"#;
    let o = text_op(
        r#"<text:p>a<text:span text:style-name="T1">HIDDEN</text:span><text:a xlink:href="http://x">link</text:a><text:bookmark text:name="b"/><text:ruby><text:ruby-base>base</text:ruby-base><text:ruby-text>RT</text:ruby-text></text:ruby></text:p>"#,
        auto,
    );
    assert_eq!(text_of_shape(&only_shape(&o)), "alinkbase");
}

#[test]
fn text_in_drawing_shapes_and_the_text_rectangle_of_a_preset() {
    let sh = r#"<draw:custom-shape draw:style-name="gr1" svg:x="1cm" svg:y="1cm" svg:width="4cm" svg:height="4cm"><text:p>in</text:p><draw:enhanced-geometry draw:type="ooxml-ellipse"/></draw:custom-shape>"#;
    let s = only_shape(&op(&slide_page(sh)).auto(&filled()));
    assert_eq!(text_of_shape(&s), "in");
    let (l, t, r, b) = s.text_rect.expect("the ellipse has a text rectangle");
    assert!(l > 0.0 && t > 0.0 && r < s.xfrm.w && b < s.xfrm.h);
    // text in a rect and in a polygon
    let r = rect("").replace("/>", "><text:p>rect</text:p></draw:rect>");
    assert_eq!(
        text_of_shape(&only_shape(&op(&slide_page(&r)).auto(&filled()))),
        "rect"
    );
}

// ---------------------------------------------------------------------------------------------
// pictures
// ---------------------------------------------------------------------------------------------

fn pic(inner: &str) -> String {
    format!(
        r#"<draw:frame draw:style-name="gr1" svg:x="2cm" svg:y="1cm" svg:width="6cm" svg:height="3cm">{inner}</draw:frame>"#
    )
}

fn pictures_of(sc: &sd::SlideScene) -> Vec<&sd::PictureItem> {
    sc.drawn_items()
        .filter_map(|i| match i {
            Item::Picture(p) => Some(p),
            _ => None,
        })
        .collect()
}

#[test]
fn a_picture_frame_is_a_picture_item_whose_bytes_are_in_the_document() {
    let o = op(&slide_page(&pic(
        r#"<draw:image xlink:href="Pictures/a.png"/>"#,
    )))
    .auto(&gr(""))
    .picture("a.png", 5);
    let d = doc(&o);
    let sc = &d.slide_scenes[0];
    let p = pictures_of(sc);
    assert_eq!(p.len(), 1);
    assert!(d.images.iter().any(|i| i.key == p[0].image.key));
    assert!(close(p[0].xfrm.x, 2.0 * EMU_CM));
}

#[test]
fn the_first_picture_that_can_be_shown_wins() {
    let o = op(&slide_page(&pic(
        r#"<draw:image xlink:href="Pictures/a.svm"/><draw:image xlink:href="Pictures/b.png"/>"#,
    )))
    .auto(&gr(""))
    .part("Pictures/a.svm", b"VCLMTF....")
    .picture("b.png", 9);
    let d = doc(&o);
    let p = pictures_of(&d.slide_scenes[0]);
    assert!(d.images.iter().any(|i| i.key == p[0].image.key));
    assert!(p[0].image.key.ends_with("b.png"));
}

#[test]
fn a_picture_that_cannot_be_shown_is_the_placeholder() {
    for href in [
        "Pictures/missing.png",
        "http://example.com/x.png",
        "../x.png",
        "Pictures/a.svm",
    ] {
        let o = op(&slide_page(&pic(&format!(
            r#"<draw:image xlink:href="{href}"/>"#
        ))))
        .auto(&gr(""))
        .part("Pictures/a.svm", b"VCLMTF");
        let sc = scene(&o);
        let p = pictures_of(&sc);
        assert_eq!(p.len(), 1, "{href}");
        assert_eq!(p[0].image.key, MISSING_PICTURE);
    }
}

#[test]
fn emf_and_wmf_pictures_are_loaded_for_the_drawing_only() {
    let o = op(&slide_page(&pic(
        r#"<draw:image xlink:href="Pictures/a.emf"/>"#,
    )))
    .auto(&gr(""))
    .part("Pictures/a.emf", &[1, 0, 0, 0, 0, 0, 0, 0]);
    let d = doc(&o);
    let p = pictures_of(&d.slide_scenes[0]);
    assert!(p[0].image.key.ends_with("a.emf"));
    assert!(d.images.iter().any(|i| i.key == p[0].image.key));
    // the text view does not show it (the Markdown is unchanged by the drawing)
    assert!(!d.markdown.contains("a.emf"), "{}", d.markdown);
}

#[test]
fn clip_mirror_and_opacity() {
    // tiny_png is 1 x 1? The crop is relative to the picture's own size at its resolution.
    let g = gr(
        r#"fo:clip="rect(0.1cm, 0cm, 0cm, 0.1cm)" style:mirror="horizontal vertical" draw:image-opacity="50%""#,
    );
    let o = op(&slide_page(&pic(
        r#"<draw:image xlink:href="Pictures/a.png"/>"#,
    )))
    .auto(&g)
    .part("Pictures/a.png", &png(16, 8));
    let sc = scene(&o);
    let p = pictures_of(&sc);
    assert!(p[0].xfrm.flip_h && p[0].xfrm.flip_v);
    assert!((p[0].image.alpha - 0.5).abs() < 1e-9);
    let c = p[0].image.crop;
    assert!(c.0 > 0.0 && c.1 > 0.0 && c.2 == 0.0 && c.3 == 0.0, "{c:?}");
}

#[test]
fn an_object_frame_draws_its_replacement_picture() {
    let o = op(&slide_page(&pic(
        r#"<draw:object xlink:href="./Object 1"/><draw:image xlink:href="Pictures/a.png"/>"#,
    )))
    .auto(&gr(""))
    .picture("a.png", 5);
    assert_eq!(pictures_of(&scene(&o)).len(), 1);
}

// ---------------------------------------------------------------------------------------------
// units and parsers
// ---------------------------------------------------------------------------------------------

#[test]
fn svg_path_data() {
    let (c, cut) = parse_svg_path("M10 10 L20 10 H30 V20 l-5 5 z").unwrap();
    assert!(!cut);
    assert_eq!(c[0], sd::PathCmd::MoveTo(sd::Pt::new(10.0, 10.0)));
    assert_eq!(c[2], sd::PathCmd::LineTo(sd::Pt::new(30.0, 10.0)));
    assert_eq!(c[3], sd::PathCmd::LineTo(sd::Pt::new(30.0, 20.0)));
    assert_eq!(c[4], sd::PathCmd::LineTo(sd::Pt::new(25.0, 25.0)));
    assert_eq!(c[5], sd::PathCmd::Close);
    // implicit line-tos after a move, commas, exponents, numbers glued by signs
    let (c, _) = parse_svg_path("m1,1 2,2 3e0-1").unwrap();
    assert_eq!(c[1], sd::PathCmd::LineTo(sd::Pt::new(3.0, 3.0)));
    assert_eq!(c[2], sd::PathCmd::LineTo(sd::Pt::new(6.0, 2.0)));
    // curves, smooth curves, quads and arcs
    let (c, _) =
        parse_svg_path("M0 0 C1 1 2 1 3 0 S5 -1 6 0 Q7 1 8 0 T10 0 A5 5 0 0 1 20 0").unwrap();
    assert!(matches!(c[1], sd::PathCmd::CubicTo(..)));
    let sd::PathCmd::CubicTo(c1, _, _) = c[2] else {
        panic!()
    };
    assert_eq!(c1, sd::Pt::new(4.0, -1.0));
}

#[test]
fn svg_arcs_end_at_their_end_point() {
    let (c, _) = parse_svg_path("M0 0 A10 10 0 0 1 20 0").unwrap();
    let sd::PathCmd::CubicTo(_, _, e) = *c.last().unwrap() else {
        panic!()
    };
    assert!((e.x - 20.0).abs() < 1e-9 && e.y.abs() < 1e-9);
    // a half circle with sweep flag 1 goes clockwise on the y-down page: over the top, through (10, -10)
    let mid = c.len() / 2;
    let _ = mid;
    let sd::PathCmd::CubicTo(_, _, m) = c[1] else {
        panic!()
    };
    assert!(m.y < -5.0, "{m:?}");
    // a radius too small is grown, a zero radius is a line, coincident points draw nothing
    assert!(parse_svg_path("M0 0 A1 1 0 0 1 20 0").is_some());
    let (c, _) = parse_svg_path("M0 0 A0 5 0 0 1 20 0").unwrap();
    assert!(matches!(c[1], sd::PathCmd::LineTo(_)));
    assert!(parse_svg_path("M0 0 A5 5 0 0 1 0 0").is_none());
}

#[test]
fn svg_path_garbage_never_panics() {
    for d in [
        "",
        "M",
        "M 1",
        "L 1 1",
        "M 1 1 L",
        "M 1 1 L 2",
        "M 1 1 X 2 2",
        "M 1 1 C 1 2 3",
        "M 1e999 1e999 L 1 1",
        "M NaN 1 L 2 2",
        "M 0 0 A 1 1 0 2 2 5 5",
        "z",
        "M 0 0 z 5 5",
        "M0 0 L1 1 Z L 2 2",
    ] {
        let _ = parse_svg_path(d);
    }
    assert!(parse_svg_path(&"M0 0 L1 1 ".repeat(200_000)).is_none());
    // over the command budget: the path is cut and says so
    let long = format!("M0 0 {}", "L1 1 ".repeat(units::MAX_PATH_CMDS + 10));
    let (c, cut) = parse_svg_path(&long).unwrap();
    assert!(cut && c.len() <= units::MAX_PATH_CMDS);
    assert!(parse_points("1,2", false).is_none());
    assert!(parse_points("1,2 3,4 5", false).unwrap().len() == 2);
    assert!(parse_points("1,2 3,4", true).unwrap().len() == 3);
}

#[test]
fn values() {
    use super::units::{angle_deg, color, pct, view_box};
    assert_eq!(pct("50%"), Some(0.5));
    assert_eq!(pct("50"), None);
    assert_eq!(angle_deg("30deg"), Some(30.0));
    assert_eq!(angle_deg("450"), Some(45.0));
    assert!((angle_deg("3.14159265358979rad").unwrap() - 180.0).abs() < 1e-6);
    assert_eq!(angle_deg("100grad"), Some(90.0));
    assert_eq!(angle_deg("x"), None);
    assert_eq!(color("#ff8000"), Some(Rgba::rgb(255, 128, 0)));
    assert_eq!(color("#f80"), Some(Rgba::rgb(255, 136, 0)));
    assert_eq!(color("transparent"), None);
    assert_eq!(color("#12"), None);
    assert_eq!(color("#éé0000"), None);
    assert_eq!(view_box("0 0 10 20"), Some((0.0, 0.0, 10.0, 20.0)));
    assert_eq!(view_box("0 0 0 20"), None);
    assert_eq!(view_box("0 0 10"), None);
    assert_eq!(draw_transform("rotate (1) bogus (2)"), None);
    assert_eq!(draw_transform(&"scale (1) ".repeat(20)), None);
    assert_eq!(draw_transform("").unwrap(), units::IDENTITY);
}

#[test]
fn boxes_under_matrices() {
    use super::units::{box_under, mul};
    let t = [1.0, 0.0, 0.0, 1.0, 100.0, 50.0];
    let x = box_under(t, 0.0, 0.0, 10.0, 20.0).unwrap();
    assert_eq!(
        (x.x, x.y, x.w, x.h, x.rot_deg),
        (100.0, 50.0, 10.0, 20.0, 0.0)
    );
    let r = [0.0, 1.0, -1.0, 0.0, 0.0, 0.0]; // 90 degrees clockwise on a y-down page
    let x = box_under(mul(r, t), 0.0, 0.0, 10.0, 20.0).unwrap();
    assert!((x.rot_deg - 90.0).abs() < 1e-9);
    assert!(box_under([0.0, 0.0, 0.0, 0.0, 0.0, 0.0], 0.0, 0.0, 1.0, 1.0).is_none());
    assert!(box_under([f64::NAN, 0.0, 0.0, 1.0, 0.0, 0.0], 0.0, 0.0, 1.0, 1.0).is_none());
}

// ---------------------------------------------------------------------------------------------
// budgets and hostile input
// ---------------------------------------------------------------------------------------------

#[test]
fn too_many_shapes_truncate_the_scene() {
    let opts = crate::preview::office::docx::DocOptions {
        max_slide_shapes: 5,
        ..Default::default()
    };
    let shapes: String = (0..20).map(|_| rect("")).collect();
    let o = op(&slide_page(&shapes)).auto(&filled());
    let d = load_op(&o, &opts).unwrap();
    assert!(d.slide_scenes[0].truncated);
    assert!(d.truncated);
    assert!(shapes_of(&d.slide_scenes[0]).len() <= 5);
}

#[test]
fn deep_groups_are_cut() {
    let mut g = rect("");
    for _ in 0..(MAX_GROUP_DEPTH + 10) {
        g = format!("<draw:g>{g}</draw:g>");
    }
    let d = doc(&op(&slide_page(&g)).auto(&filled()));
    assert!(d.slide_scenes[0].truncated);
}

#[test]
fn too_much_text_is_cut_and_flagged() {
    let big = "x".repeat(body::MAX_SLIDE_CHARS + 5_000);
    let d = doc(&text_op(&format!("<text:p>{big}</text:p>"), ""));
    assert!(d.slide_scenes[0].truncated);
    let s = shapes_of(&d.slide_scenes[0])[0];
    assert!(text_of_shape(s).len() <= body::MAX_SLIDE_CHARS);
}

#[test]
fn the_text_budget_counts_characters_not_bytes() {
    // 100,000 Japanese characters are 300,000 bytes but half of the 200,000-character budget.
    let fits = "あ".repeat(100_000);
    let d = doc(&text_op(&format!("<text:p>{fits}</text:p>"), ""));
    let sc = &d.slide_scenes[0];
    assert!(!sc.truncated);
    assert_eq!(text_of_shape(shapes_of(sc)[0]).chars().count(), 100_000);
    // Over the budget in characters: cut at it, to the character.
    let big = "あ".repeat(body::MAX_SLIDE_CHARS + 7);
    let d = doc(&text_op(&format!("<text:p>{big}</text:p>"), ""));
    let sc = &d.slide_scenes[0];
    assert!(sc.truncated);
    assert_eq!(
        text_of_shape(shapes_of(sc)[0]).chars().count(),
        body::MAX_SLIDE_CHARS
    );
    // The two readers agree on the limit.
    assert_eq!(body::MAX_SLIDE_CHARS, sd::text::MAX_TEXT_CHARS);
}

#[test]
fn many_paragraphs_and_deep_lists_are_bounded() {
    let paras = "<text:p>a</text:p>".repeat(body::MAX_PARAGRAPHS + 50);
    let d = doc(&text_op(&paras, ""));
    assert!(d.slide_scenes[0].truncated);
    let mut l = "<text:p>deep</text:p>".to_string();
    for _ in 0..(body::MAX_LIST_DEPTH + 5) {
        l = format!("<text:list><text:list-item>{l}</text:list-item></text:list>");
    }
    let d = doc(&text_op(&l, ""));
    assert!(d.slide_scenes[0].truncated);
}

#[test]
fn hostile_numbers_and_attributes_do_not_panic() {
    let shapes = [
        r#"<draw:rect draw:style-name="gr1" svg:x="1e999cm" svg:y="NaNcm" svg:width="99999999999999999cm" svg:height="-5cm"/>"#,
        r#"<draw:ellipse draw:style-name="gr1" svg:x="0cm" svg:y="0cm" svg:width="0cm" svg:height="0cm" draw:kind="section" draw:start-angle="1e999" draw:end-angle="nan"/>"#,
        r#"<draw:line draw:style-name="gr1" svg:x1="a" svg:y1="b" svg:x2="c" svg:y2="d"/>"#,
        r#"<draw:path draw:style-name="gr1" svg:x="0cm" svg:y="0cm" svg:width="1cm" svg:height="1cm" svg:viewBox="0 0 -1 NaN" svg:d="M0"/>"#,
        r#"<draw:regular-polygon draw:style-name="gr1" svg:x="0cm" svg:y="0cm" svg:width="1cm" svg:height="1cm" draw:corners="99999999999"/>"#,
        r#"<draw:custom-shape draw:style-name="gr1" svg:x="0cm" svg:y="0cm" svg:width="1cm" svg:height="1cm"><draw:enhanced-geometry draw:type="ooxml-rightArrow" draw:modifiers="nan inf -inf 1e999"/></draw:custom-shape>"#,
        r#"<draw:frame svg:x="0cm" svg:y="0cm" svg:width="1cm" svg:height="1cm"><draw:image/><draw:text-box/></draw:frame>"#,
    ];
    let auto = gr(
        r##"draw:fill="solid" draw:fill-color="#nonsense" draw:stroke="solid" svg:stroke-width="1e999cm" draw:opacity="x%" draw:shadow="visible" draw:shadow-offset-x="zzz" draw:marker-end="m" draw:fill-hatch-name="h" fo:padding="x""##,
    );
    let styles = r#"<draw:marker draw:name="m" svg:viewBox="0 0 0 0" svg:d="M0"/>"#;
    for s in shapes {
        let d = doc(&op(&slide_page(s)).auto(&auto).styles(styles));
        assert_eq!(d.slide_scenes.len(), 1);
    }
}

#[test]
fn a_page_that_is_too_big_to_read_still_has_a_scene() {
    let opts = crate::preview::office::docx::DocOptions {
        max_block_nodes: 3,
        ..Default::default()
    };
    let shapes: String = (0..20).map(|_| rect("")).collect();
    let d = load_op(&op(&slide_page(&shapes)).auto(&filled()), &opts).unwrap();
    assert_eq!(d.slide_scenes.len(), d.slides.len());
    assert!(d.slide_scenes[0].truncated);
}

#[test]
fn every_scene_draws() {
    // the scenes of a busy slide go through the writer and come out as an SVG
    let shapes = format!(
        "{}{}{}",
        rect(""),
        tb("<text:p>text</text:p>"),
        r#"<draw:ellipse draw:style-name="gr1" svg:x="9cm" svg:y="1cm" svg:width="2cm" svg:height="2cm"/>"#
    );
    let sc = scene(&op(&slide_page(&shapes)).auto(&filled()));
    let r = sd::render_svg(&sc, &|_| None);
    assert!(r.svg.starts_with("<svg") || r.svg.contains("<svg"));
    assert!(r.svg.contains("text"));
}

#[test]
fn the_text_view_is_unchanged_by_the_drawing() {
    // the Markdown of a deck with pictures of every kind has no mention of metafiles and the same
    // headings as the picture view
    let o = op(&slide_page(&pic(
        r#"<draw:image xlink:href="Pictures/a.png"/>"#,
    )))
    .auto(&gr(""))
    .picture("a.png", 5);
    let d = doc(&o);
    assert_eq!(
        d.markdown.lines().filter(|l| l.starts_with("## ")).count(),
        d.picture_markdown
            .lines()
            .filter(|l| l.starts_with("## "))
            .count()
    );
}

// ---------------------------------------------------------------------------------------------
// found by looking at LibreOffice's renderings
// ---------------------------------------------------------------------------------------------

#[test]
fn page_name_field_is_the_name_or_slide_n() {
    let named = r#"<draw:page draw:name="Intro" draw:style-name="dp1" draw:master-page-name="Default"><draw:frame draw:style-name="gr1" svg:x="1cm" svg:y="1cm" svg:width="9cm" svg:height="2cm"><draw:text-box><text:p><text:page-name>&lt;slide-name&gt;</text:page-name></text:p></draw:text-box></draw:frame></draw:page>"#;
    let default_named = named.replace("Intro", "page2");
    let d = doc(&op(&format!("{named}{default_named}")).auto(&gr(r#"draw:fill="none""#)));
    let text = |i: usize| text_of_shape(shapes_of(&d.slide_scenes[i])[0]);
    assert_eq!(text(0), "Intro");
    assert_eq!(text(1), "Slide 2");
}

#[test]
fn a_tile_size_of_zero_is_the_pictures_own_size() {
    let res = r#"<draw:fill-image draw:name="B" xlink:href="Pictures/a.png"/>"#;
    let s = only_shape(
        &op(&slide_page(&rect("")))
            .styles(res)
            .auto(&gr(
                r#"draw:fill="bitmap" draw:fill-image-name="B" draw:fill-image-width="0cm" draw:fill-image-height="0cm" style:repeat="repeat""#,
            ))
            .part("Pictures/a.png", &png(16, 8)),
    );
    let Fill::Image(i) = s.fill else { panic!() };
    match i.mode {
        sd::ImageMode::Tile { sx, sy, .. } => assert_eq!((sx, sy), (1.0, 1.0)),
        m => panic!("{m:?}"),
    }
}

#[test]
fn a_tile_of_a_300_dpi_picture_is_a_third_of_a_96_dpi_one() {
    let res = r#"<draw:fill-image draw:name="B" xlink:href="Pictures/a.png"/>"#;
    let at = |bytes: &[u8]| {
        let s = only_shape(
            &op(&slide_page(&rect("")))
                .styles(res)
                .auto(&gr(
                    r#"draw:fill="bitmap" draw:fill-image-name="B" style:repeat="repeat""#,
                ))
                .part("Pictures/a.png", bytes),
        );
        let Fill::Image(i) = s.fill else { panic!() };
        match i.mode {
            sd::ImageMode::Tile { sx, sy, .. } => (sx, sy),
            m => panic!("{m:?}"),
        }
    };
    let (sx, sy) = at(&with_phys(&png(16, 8), 11_811));
    assert!(
        (sx - 0.32).abs() < 0.01 && (sy - 0.32).abs() < 0.01,
        "{sx} {sy}"
    );
    // no resolution: 96 dpi
    assert_eq!(at(&png(16, 8)), (1.0, 1.0));
}

#[test]
fn a_hairline_is_thinner_than_a_pixel() {
    let s = with_resource(
        "",
        r##"draw:stroke="solid" svg:stroke-width="0cm" svg:stroke-color="#000000""##,
    );
    let w = s.line.unwrap().width;
    assert!(w > 0.0 && w < sd::EMU_PER_PX, "{w}");
}

#[test]
fn picture_bullets_have_the_levels_height() {
    let ls = r#"<text:list-style style:name="L1"><text:list-level-style-image text:level="1" xlink:href="Pictures/a.png"><style:list-level-properties fo:width="0.4cm" fo:height="0.4cm"/></text:list-level-style-image></text:list-style>"#;
    let o = text_op(
        r#"<text:list text:style-name="L1"><text:list-item><text:p>a</text:p></text:list-item></text:list>"#,
        ls,
    )
    .part("Pictures/a.png", &png(4, 4));
    let ps = only_shape(&o).text.unwrap().paragraphs;
    let b = ps[0].bullet.as_ref().unwrap();
    match b.size {
        sd::BulletSize::Pts(p) => assert!((p - 0.4 * 360_000.0 / 12_700.0).abs() < 1e-6),
        s => panic!("{s:?}"),
    }
}

#[test]
fn a_page_background_that_names_no_picture_keeps_the_fill_colour() {
    let styles = r##"<style:style style:name="Mdp1" style:family="drawing-page"><style:drawing-page-properties draw:fill="bitmap" draw:fill-image-name="" style:repeat="stretch"/></style:style><style:default-style style:family="graphic"><style:graphic-properties draw:fill-color="#729fcf"/></style:default-style>"##;
    let sc = scene(&op(&slide_page("")).styles(styles));
    assert_eq!(sc.background, Fill::Solid(Rgba::rgb(0x72, 0x9f, 0xcf)));
}

/// A PNG with a `pHYs` chunk of `ppm` pixels per metre inserted after the signature.
fn with_phys(png: &[u8], ppm: u32) -> Vec<u8> {
    // (after the 25-byte IHDR chunk)
    let mut out = png[..33].to_vec();
    out.extend_from_slice(&9u32.to_be_bytes());
    out.extend_from_slice(b"pHYs");
    out.extend_from_slice(&ppm.to_be_bytes());
    out.extend_from_slice(&ppm.to_be_bytes());
    out.push(1);
    out.extend_from_slice(&[0, 0, 0, 0]);
    out.extend_from_slice(&png[33..]);
    out
}

#[test]
fn image_dimensions_and_resolution_come_from_the_header() {
    use super::{image_dimensions, image_dpi};
    let p = png(30, 20);
    assert_eq!(image_dimensions(&p), Some((30, 20)));
    assert_eq!(image_dpi(&p), None);
    // 2835 pixels per metre is 72 dpi
    assert!((image_dpi(&with_phys(&p, 2835)).unwrap() - 72.0).abs() < 0.01);
    // a JFIF header: 300 dpi
    let jfif = [
        0xFF, 0xD8, 0xFF, 0xE0, 0, 16, b'J', b'F', b'I', b'F', 0, 1, 1, 1, 0x01, 0x2C, 0x01, 0x2C,
        0, 0,
    ];
    assert_eq!(image_dpi(&jfif), Some(300.0));
    // garbage never panics
    for g in [
        &b""[..],
        b"\x89PNG",
        b"\xFF\xD8",
        b"\xFF\xD8\xFF\xE0\xFF\xFF",
        &[0u8; 40],
    ] {
        let _ = image_dpi(g);
        let _ = image_dimensions(g);
    }
    assert_eq!(image_dimensions(b"not an image"), None);
}

#[test]
fn a_picture_made_at_72_dpi_is_larger_so_a_clip_is_a_smaller_fraction() {
    let g = gr(r#"fo:clip="rect(0cm, 0cm, 0cm, 0.1cm)""#);
    let at = |bytes: &[u8]| {
        let o = op(&slide_page(&pic(
            r#"<draw:image xlink:href="Pictures/a.png"/>"#,
        )))
        .auto(&g)
        .part("Pictures/a.png", bytes);
        pictures_of(&scene(&o))[0].image.crop.0
    };
    let c72 = at(&with_phys(&png(16, 8), 2835));
    let c96 = at(&png(16, 8));
    assert!(c72 < c96 && c72 > 0.0, "{c72} {c96}");
}

#[test]
fn a_background_picture_placed_once_is_a_picture_item_at_the_reference_point() {
    let styles = r##"<draw:fill-image draw:name="B" xlink:href="Pictures/a.png"/><style:style style:name="Mdp1" style:family="drawing-page"><style:drawing-page-properties draw:fill="bitmap" draw:fill-image-name="B" style:repeat="no-repeat" draw:fill-image-width="4cm" draw:fill-image-height="2cm" draw:fill-image-ref-point="center"/></style:style>"##;
    let o = op(&slide_page(""))
        .styles(styles)
        .part("Pictures/a.png", &png(16, 8));
    let sc = scene(&o);
    assert_eq!(sc.background, Fill::Solid(Rgba::WHITE));
    let p = pictures_of(&sc);
    assert_eq!(p.len(), 1);
    assert!(close(p[0].xfrm.w, 4.0 * EMU_CM) && close(p[0].xfrm.h, 2.0 * EMU_CM));
    // centred on the 28 x 15.75 cm page
    assert!(close(p[0].xfrm.x, 12.0 * EMU_CM));
    assert!(close(p[0].xfrm.y, 6.875 * EMU_CM));
    // a repeated one stays a fill
    let o = op(&slide_page(""))
        .styles(&styles.replace("no-repeat", "repeat"))
        .part("Pictures/a.png", &png(16, 8));
    assert!(matches!(scene(&o).background, Fill::Image(_)));
}

#[test]
fn footer_references_on_the_page_element_are_followed() {
    let decl = r#"<presentation:footer-decl presentation:name="ftr1">From the page</presentation:footer-decl>"#;
    let page = r#"<draw:page draw:name="p" draw:style-name="dp1" draw:master-page-name="Default" presentation:use-footer-name="ftr1"></draw:page>"#;
    let dp = r#"<style:style style:name="dp1" style:family="drawing-page"><style:drawing-page-properties presentation:display-footer="true"/></style:style>"#;
    let sc = scene(&master_op(&format!("{decl}{page}"), dp));
    let texts: Vec<String> = shapes_of(&sc).iter().map(|s| text_of_shape(s)).collect();
    assert!(texts.contains(&"From the page".to_string()), "{texts:?}");
}

#[test]
fn the_preview_picture_beside_a_table_is_not_drawn() {
    let frame = pic(
        r#"<table:table table:name="T"><table:table-column/><table:table-row><table:table-cell><text:p>x</text:p></table:table-cell></table:table-row></table:table><draw:image xlink:href="Pictures/TablePreview1.svm"/>"#,
    );
    let o = op(&slide_page(&frame))
        .auto(&gr(""))
        .part("Pictures/TablePreview1.svm", b"VCLMTF");
    assert!(pictures_of(&scene(&o)).is_empty());
}

mod tests_chart;
mod tests_fixes;
mod tests_fontwork;
mod tests_shared;
mod tests_table;
