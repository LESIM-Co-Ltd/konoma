//! Tests of the differences from LibreOffice found in review: the default of an undefined
//! gradient / hatch / bitmap, the text of plain shapes, mirrored and rotated custom shapes,
//! turned gradients, the caption pointer and current-date fields. Each rule was measured on
//! LibreOffice's own rendering of a small deck written for the purpose.

use super::dates::{self, format, moment_of, Moment};
use super::*;

fn filled_rect(props: &str, res: &str) -> ShapeItem {
    with_resource(res, props)
}

fn page_op(drawing_page_props: &str, res: &str) -> Op {
    let styles = format!(
        r#"{res}<style:style style:name="Mdp1" style:family="drawing-page"><style:drawing-page-properties {drawing_page_props}/></style:style>"#
    );
    op(&slide_page("")).styles(&styles)
}

// ---- undefined gradient, hatch and bitmap -----------------------------------------------------------

#[test]
fn an_undefined_gradient_is_black_to_white_on_a_page_and_blue_to_white_on_a_shape() {
    for name in [
        r#"draw:fill-gradient-name="nope""#,
        r#"draw:fill-gradient-name="""#,
        "",
    ] {
        let sc = scene(&page_op(&format!(r#"draw:fill="gradient" {name}"#), ""));
        let Fill::Gradient(g) = sc.background else {
            panic!("{:?}", sc.background)
        };
        assert_eq!(g.stops[0], (0.0, Rgba::BLACK));
        assert_eq!(g.stops[1], (1.0, Rgba::WHITE));
        assert_eq!(
            g.kind,
            sd::GradKind::Linear {
                angle_deg: 90.0,
                scaled: false
            }
        );
        let s = filled_rect(
            &format!(r##"draw:fill="gradient" {name} draw:fill-color="#ff0000""##),
            "",
        );
        let Fill::Gradient(g) = s.fill else { panic!() };
        assert_eq!(g.stops[0].1, Rgba::rgb(0x34, 0x65, 0xa4));
        assert_eq!(g.stops[1].1, Rgba::WHITE);
    }
}

#[test]
fn an_undefined_gradient_takes_the_opacity() {
    let s = filled_rect(
        r#"draw:fill="gradient" draw:fill-gradient-name="nope" draw:opacity="40%""#,
        "",
    );
    let Fill::Gradient(g) = s.fill else { panic!() };
    assert!(g.stops.iter().all(|s| (s.1.a - 0.4).abs() < 1e-9));
}

#[test]
fn a_dark_default_gradient_gives_a_page_white_automatic_text() {
    // the title of such a slide is white in LibreOffice
    let o = page_op(r#"draw:fill="gradient" draw:fill-gradient-name="nope""#, "")
        .auto(&gr(r#"draw:fill="none""#));
    let o = Op {
        pages: slide_page(&tb("<text:p>x</text:p>")),
        ..o
    };
    let s = only_shape(&o);
    let r = &s.text.unwrap().paragraphs[0].runs[0];
    assert_eq!(r.fill, Fill::Solid(Rgba::WHITE));
}

#[test]
fn an_undefined_hatch_is_pale_blue_lines_or_just_the_background() {
    let s = filled_rect(
        r##"draw:fill="hatch" draw:fill-hatch-name="nope" draw:fill-color="#00aa00""##,
        "",
    );
    let Fill::Pattern { preset, fg, bg } = s.fill else {
        panic!("{:?}", s.fill)
    };
    assert!(preset.contains("Horz"), "{preset}");
    assert_eq!(fg, Rgba::rgb(0x34, 0x65, 0xa4));
    assert_eq!(bg, Rgba::TRANSPARENT);
    // with a solid background and no hatch: the background colour only
    let s = filled_rect(
        r##"draw:fill="hatch" draw:fill-hatch-name="" draw:fill-hatch-solid="true" draw:fill-color="#00aa00""##,
        "",
    );
    assert_eq!(s.fill, Fill::Solid(Rgba::rgb(0, 0xaa, 0)));
}

#[test]
fn an_undefined_bitmap_is_no_fill_on_a_shape_but_keeps_the_colour_on_a_page() {
    for name in [
        r#"draw:fill-image-name="nope""#,
        r#"draw:fill-image-name="""#,
        "",
    ] {
        // (an outline keeps the shape in the scene)
        let s = filled_rect(
            &format!(
                r##"draw:fill="bitmap" {name} draw:fill-color="#0000ff" draw:stroke="solid" svg:stroke-color="#000000""##
            ),
            "",
        );
        assert_eq!(s.fill, Fill::None, "{name}");
        let sc = scene(&page_op(
            &format!(r##"draw:fill="bitmap" {name} draw:fill-color="#0000ff""##),
            "",
        ));
        assert_eq!(sc.background, Fill::Solid(Rgba::rgb(0, 0, 255)), "{name}");
    }
    // a bitmap that is defined but cannot be shown keeps the colour
    let res = r#"<draw:fill-image draw:name="B" xlink:href="Pictures/missing.png"/>"#;
    let s = filled_rect(
        r##"draw:fill="bitmap" draw:fill-image-name="B" draw:fill-color="#0000ff""##,
        res,
    );
    assert_eq!(s.fill, Fill::Solid(Rgba::rgb(0, 0, 255)));
}

#[test]
fn an_opacity_outside_zero_to_one_hundred_percent_is_ignored() {
    for v in ["-4900%", "250%", "nonsense"] {
        let s = filled_rect(
            &format!(r##"draw:fill="solid" draw:fill-color="#808080" draw:opacity="{v}""##),
            "",
        );
        assert_eq!(s.fill, Fill::Solid(Rgba::rgb(0x80, 0x80, 0x80)), "{v}");
    }
}

// ---- the text of plain shapes ------------------------------------------------------------------------

fn text_shape(tag: &str, extra: &str) -> sd::TextBody {
    let sh = format!(
        r#"<draw:{tag} draw:style-name="gr1" svg:x="1cm" svg:y="1cm" svg:width="2cm" svg:height="2cm" {extra}><text:p>LOGO LOGO</text:p></draw:{tag}>"#
    );
    only_shape(&op(&slide_page(&sh)).auto(&gr(
        r##"draw:fill="solid" draw:fill-color="#ff0000" fo:wrap-option="wrap""##,
    )))
    .text
    .unwrap()
}

#[test]
fn the_text_of_a_plain_rectangle_ellipse_or_circle_does_not_wrap() {
    assert!(!text_shape("rect", "").wrap);
    assert!(!text_shape("ellipse", "").wrap);
    assert!(!text_shape("circle", "").wrap);
}

#[test]
fn custom_shapes_and_text_boxes_keep_wrapping() {
    let cs = r#"<draw:custom-shape draw:style-name="gr1" svg:x="1cm" svg:y="1cm" svg:width="2cm" svg:height="2cm"><text:p>LOGO LOGO</text:p><draw:enhanced-geometry svg:viewBox="0 0 21600 21600" draw:type="rectangle" draw:enhanced-path="M 0 0 L 21600 0 21600 21600 0 21600 Z N"/></draw:custom-shape>"#;
    let s = only_shape(&op(&slide_page(cs)).auto(&gr(r#"draw:fill="solid""#)));
    assert!(s.text.unwrap().wrap);
    let s = only_shape(&text_op("<text:p>LOGO LOGO</text:p>", ""));
    assert!(s.text.unwrap().wrap);
}

#[test]
fn a_no_wrap_circle_centres_its_text_on_the_circle() {
    let t = text_shape("circle", "");
    // (centred by its paragraph or by the text area, never wrapped)
    assert!(!t.wrap);
}

// ---- mirrored and rotated custom shapes -------------------------------------------------------------------

fn rotated(mirror: &str, angle: &str) -> ShapeItem {
    let sh = format!(
        r#"<draw:custom-shape draw:style-name="gr1" svg:width="2cm" svg:height="4cm" draw:transform="skewX (-3.7E-017) rotate ({angle}) translate (5cm 2cm)"><draw:enhanced-geometry svg:viewBox="0 0 640 861" {mirror} draw:type="non-primitive" draw:enhanced-path="M 640 233 L 221 293 506 12 367 0 Z N"/></draw:custom-shape>"#
    );
    only_shape(&op(&slide_page(&sh)).auto(&filled()))
}

#[test]
fn a_shape_mirrored_one_way_turns_the_other_way_about_the_same_centre() {
    let plain = rotated("", "0.5");
    let v = rotated(r#"draw:mirror-vertical="true""#, "0.5");
    let h = rotated(r#"draw:mirror-horizontal="true""#, "0.5");
    let vh = rotated(
        r#"draw:mirror-vertical="true" draw:mirror-horizontal="true""#,
        "0.5",
    );
    let off = rotated(
        r#"draw:mirror-vertical="false" draw:mirror-horizontal="false""#,
        "0.5",
    );
    assert!(
        (plain.xfrm.rot_deg - -28.6479).abs() < 1e-3,
        "{}",
        plain.xfrm.rot_deg
    );
    assert!((v.xfrm.rot_deg + plain.xfrm.rot_deg).abs() < 1e-9);
    assert!((h.xfrm.rot_deg + plain.xfrm.rot_deg).abs() < 1e-9);
    // mirrored both ways it is the plain rotation, as is a mirror that says "false"
    assert!((vh.xfrm.rot_deg - plain.xfrm.rot_deg).abs() < 1e-9);
    assert!((off.xfrm.rot_deg - plain.xfrm.rot_deg).abs() < 1e-9);
    // the centre does not move
    for s in [&v, &h, &vh, &off] {
        assert!(close(s.xfrm.x, plain.xfrm.x) && close(s.xfrm.y, plain.xfrm.y));
    }
}

#[test]
fn an_unrotated_mirrored_shape_is_not_turned() {
    let v = rotated(r#"draw:mirror-vertical="true""#, "0");
    assert_eq!(v.xfrm.rot_deg, 0.0);
}

// ---- turned gradients ----------------------------------------------------------------------------------------

fn gradient_with(style: &str, angle: &str) -> sd::Gradient {
    let res = format!(
        r##"<draw:gradient draw:name="G" draw:style="{style}" draw:start-color="#ff8800" draw:end-color="#2244aa" draw:angle="{angle}"/>"##
    );
    let s = with_resource(&res, r#"draw:fill="gradient" draw:fill-gradient-name="G""#);
    let Fill::Gradient(g) = s.fill else { panic!() };
    g
}

#[test]
fn ellipsoid_square_and_rectangular_gradients_are_turned_by_their_angle() {
    // ODF angles run counter-clockwise, the model's clockwise
    let g = gradient_with("ellipsoid", "30deg");
    match g.kind {
        sd::GradKind::Ellipsoid { angle_deg } => assert!((angle_deg - 330.0).abs() < 1e-9),
        k => panic!("{k:?}"),
    }
    for style in ["square", "rectangular"] {
        match gradient_with(style, "45deg").kind {
            sd::GradKind::RectRotated { angle_deg } => assert!((angle_deg - 315.0).abs() < 1e-9),
            k => panic!("{style}: {k:?}"),
        }
    }
    // tenths of a degree (the old form) too
    match gradient_with("square", "1200").kind {
        sd::GradKind::RectRotated { angle_deg } => assert!((angle_deg - 240.0).abs() < 1e-9),
        k => panic!("{k:?}"),
    }
}

#[test]
fn an_angle_of_zero_and_a_circular_gradient_stay_as_they_were() {
    // (an ellipsoid at angle 0 is not a circle: it is LibreOffice's stretched shape)
    assert_eq!(
        gradient_with("ellipsoid", "0deg").kind,
        sd::GradKind::Ellipsoid { angle_deg: 0.0 }
    );
    assert_eq!(
        gradient_with("ellipsoid", "360deg").kind,
        sd::GradKind::Ellipsoid { angle_deg: 0.0 }
    );
    assert_eq!(gradient_with("square", "0").kind, sd::GradKind::Rect);
    assert_eq!(
        gradient_with("rectangular", "0deg").kind,
        sd::GradKind::Rect
    );
    // a circle turned is the same circle
    assert_eq!(gradient_with("radial", "30deg").kind, sd::GradKind::Radial);
}

fn rendered(g: sd::Gradient) -> String {
    let mut sc = sd::SlideScene {
        width: 9_144_000.0,
        height: 5_143_500.0,
        ..sd::SlideScene::default()
    };
    let mut s = sd::ShapeItem::new(
        sd::Xfrm::rect(914_400.0, 914_400.0, 3_600_000.0, 1_800_000.0),
        Geometry::Rect,
    );
    s.fill = Fill::Gradient(g);
    sc.items.push(Item::Shape(s));
    sd::render_svg(&sc, &|_| None).svg
}

/// The colour of the pixel at the fraction `(fx, fy)` of the box of the shape of [`rendered`]
/// (3.6 x 1.8 million EMU at 0.9144 million, on a 9144000 x 5143500 slide).
fn at_box(svg: &str, fx: f64, fy: f64) -> [u8; 4] {
    let img = crate::preview::svg::rasterize_trusted(svg.as_bytes(), Path::new("/s.svg"), 1000)
        .unwrap()
        .to_rgba8();
    // box: x 914_400..4_514_400 of 9_144_000 wide, y 914_400..2_714_400 of 5_143_500 tall
    let px = (914_400.0 + fx * 3_600_000.0) / 9_144_000.0 * f64::from(img.width());
    let py = (914_400.0 + fy * 1_800_000.0) / 5_143_500.0 * f64::from(img.height());
    img.get_pixel(px as u32, py as u32).0
}

/// How far the pixel is along the orange (#ff8800, the start) to blue (#2244aa, the end) ramp:
/// 0 is the end colour (blue), 1 the start colour (orange).
fn orange_ness(p: [u8; 4]) -> f64 {
    (f64::from(p[0]) - 0x22 as f64) / (0xff as f64 - 0x22 as f64)
}

#[test]
fn an_ellipsoid_gradient_is_a_distance_from_a_segment_not_a_scaled_circle() {
    // The box is 3.6 x 1.8 (long x short): the radius is 1.8 * sqrt(2) / 2 = 1.273, the
    // segment 0.9 * sqrt(2) = 1.273 either side of the centre, in millions of EMU.
    let svg = rendered(gradient_with("ellipsoid", "0deg"));
    // blue all along the segment: the centre, and 1.2 to the left and right (of 1.8 half width)
    for fx in [0.5, 0.5 - 1.2 / 3.6, 0.5 + 1.2 / 3.6] {
        let t = orange_ness(at_box(&svg, fx, 0.5));
        assert!(t < 0.08, "fx {fx}: {t}");
    }
    // across it the colour runs to orange at the radius: half the height is 0.9 of 1.273
    let top = orange_ness(at_box(&svg, 0.5, 0.03));
    assert!(top > 0.6 && top < 0.85, "{top}");
    // a circle would be orange at the same distance along the length (1.2 of 1.273): it is blue
    let side = orange_ness(at_box(&svg, 0.5 + 1.7 / 3.6, 0.5));
    assert!(side < 0.5, "{side}");
}

#[test]
fn an_ellipsoid_turned_90_degrees_runs_the_other_way_and_a_tall_box_runs_down() {
    // turned a quarter: the blue band is vertical, so the middle of the top edge is blue and
    // the left and right ends (a radius away) are orange
    let svg = rendered(gradient_with("ellipsoid", "90deg"));
    assert!(orange_ness(at_box(&svg, 0.5, 0.04)) < 0.1);
    assert!(orange_ness(at_box(&svg, 0.5, 0.96)) < 0.1);
    assert!(orange_ness(at_box(&svg, 0.5 + 1.5 / 3.6, 0.5)) > 0.55);
    assert!(orange_ness(at_box(&svg, 0.5 - 1.5 / 3.6, 0.5)) > 0.55);
}

#[test]
fn a_turned_ellipsoid_is_turned_about_the_centre_of_the_box() {
    let svg = rendered(gradient_with("ellipsoid", "30deg"));
    assert!(svg.contains("<pattern"), "{svg}");
    // 330 degrees clockwise about the centre of the box (x 189, y 94.5 px of the tile)
    assert!(svg.contains("rotate(330 "), "{svg}");
    // it paints, and the centre is the end colour
    assert!(orange_ness(at_box(&svg, 0.5, 0.5)) < 0.08);
}

#[test]
fn a_square_ellipsoid_is_a_circle() {
    let res = r##"<draw:gradient draw:name="G" draw:style="ellipsoid" draw:start-color="#ff8800" draw:end-color="#2244aa" draw:angle="0deg"/>"##;
    let s = with_resource(res, r#"draw:fill="gradient" draw:fill-gradient-name="G""#);
    let Fill::Gradient(g) = s.fill else { panic!() };
    // a 1.8 x 1.8 box: no segment, so the colour is the distance from the centre / 1.273
    let mut sc = sd::SlideScene {
        width: 9_144_000.0,
        height: 5_143_500.0,
        ..sd::SlideScene::default()
    };
    let mut sh = sd::ShapeItem::new(
        sd::Xfrm::rect(914_400.0, 914_400.0, 1_800_000.0, 1_800_000.0),
        Geometry::Rect,
    );
    sh.fill = Fill::Gradient(g);
    sc.items.push(Item::Shape(sh));
    let svg = sd::render_svg(&sc, &|_| None).svg;
    let img = crate::preview::svg::rasterize_trusted(svg.as_bytes(), Path::new("/s.svg"), 1000)
        .unwrap()
        .to_rgba8();
    let at = |fx: f64, fy: f64| {
        let px = (914_400.0 + fx * 1_800_000.0) / 9_144_000.0 * f64::from(img.width());
        let py = (914_400.0 + fy * 1_800_000.0) / 5_143_500.0 * f64::from(img.height());
        orange_ness(img.get_pixel(px as u32, py as u32).0)
    };
    assert!(at(0.5, 0.5) < 0.08, "{} {svg}", at(0.5, 0.5));
    // the same distance left and up gives the same colour (and no seam where the halves meet)
    assert!((at(0.1, 0.5) - at(0.5, 0.1)).abs() < 0.06);
}

#[test]
fn a_turned_rect_gradient_is_a_pattern_in_user_space_turned_about_the_centre() {
    let svg = rendered(gradient_with("square", "45deg"));
    assert!(svg.contains(r#"patternUnits="userSpaceOnUse""#), "{svg}");
    assert!(svg.contains("rotate(315 "), "{svg}");
    // what the turned rectangle does not reach is the last colour of the model's stops (the
    // outer one: the gradient's start colour)
    assert!(
        svg.contains(r##"<rect width="377.953" height="188.976" fill="#ff8800""##),
        "{svg}"
    );
    // the pattern paints: the corner (outside a rotated rectangle) is the outer colour, the
    // middle the first
    let img =
        crate::preview::svg::rasterize_trusted(svg.as_bytes(), Path::new("/s.svg"), 800).unwrap();
    let rgb = img.to_rgba8();
    let s = f64::from(rgb.width()) / 960.0;
    let at = |x: f64, y: f64| *rgb.get_pixel((x * s) as u32, (y * s) as u32);
    let centre = at(96.0 + 189.0, 96.0 + 94.5);
    let corner = at(96.0 + 2.0, 96.0 + 2.0);
    assert!(centre[2] > centre[0] || centre[0] > 0, "{centre:?}");
    assert_ne!(centre, corner);
}

#[test]
fn an_unturned_gradient_renders_as_before() {
    let svg = rendered(gradient_with("square", "0"));
    assert!(svg.contains(r#"patternUnits="objectBoundingBox""#));
    // An ellipsoid at angle 0 is not turned: the pattern writes a rotation of nothing.
    let svg = rendered(gradient_with("ellipsoid", "0"));
    assert!(
        svg.contains("<pattern") && svg.contains("rotate(0 "),
        "{svg}"
    );
}

// ---- the caption pointer ---------------------------------------------------------------------------------------

fn caption(attrs: &str, style: &str) -> Vec<ShapeItem> {
    let sh = format!(
        r#"<draw:caption draw:style-name="gr1" svg:x="3cm" svg:y="3cm" svg:width="6cm" svg:height="2cm" {attrs}><text:p>Cap</text:p></draw:caption>"#
    );
    let sc = scene(&op(&slide_page(&sh)).auto(&gr(&format!(
        r##"draw:fill="solid" draw:fill-color="#ffeeaa" draw:stroke="solid" svg:stroke-color="#aa0000" svg:stroke-width="0.06cm" {style}"##
    ))));
    shapes_of(&sc).into_iter().cloned().collect()
}

/// The ends of the pointer line (the one shape that is a line) in cm.
fn pointer(shapes: &[ShapeItem]) -> ((f64, f64), (f64, f64)) {
    let l = shapes
        .iter()
        .find(|s| s.geom == Geometry::Line)
        .expect("a pointer");
    let (x0, y0) = (l.xfrm.x / EMU_CM, l.xfrm.y / EMU_CM);
    let (x1, y1) = (
        (l.xfrm.x + l.xfrm.w) / EMU_CM,
        (l.xfrm.y + l.xfrm.h) / EMU_CM,
    );
    let a = if l.xfrm.flip_h { x1 } else { x0 };
    let b = if l.xfrm.flip_h { x0 } else { x1 };
    let c = if l.xfrm.flip_v { y1 } else { y0 };
    let d = if l.xfrm.flip_v { y0 } else { y1 };
    ((a, c), (b, d))
}

fn near2(a: (f64, f64), b: (f64, f64)) -> bool {
    (a.0 - b.0).abs() < 0.01 && (a.1 - b.1).abs() < 0.01
}

#[test]
fn a_caption_is_a_rectangle_and_a_pointer_to_its_point() {
    // beside the rectangle on the left: from the middle of the left side to the point
    let s = caption(
        r#"draw:caption-point-x="-2.5cm" draw:caption-point-y="4.5cm""#,
        "",
    );
    assert_eq!(s.len(), 2);
    let (a, b) = pointer(&s);
    assert!(near2(a, (3.0, 4.0)), "{a:?}");
    assert!(near2(b, (0.5, 7.5)), "{b:?}");
    // the pointer is under the box and in the caption's own outline
    assert_eq!(s[0].geom, Geometry::Line);
    assert_eq!(s[0].line, s[1].line);
    assert!(s[1].text.is_some());
}

#[test]
fn the_pointer_leaves_from_the_side_that_faces_the_point() {
    let from = |px: &str, py: &str| {
        let s = caption(
            &format!(r#"draw:caption-point-x="{px}" draw:caption-point-y="{py}""#),
            "",
        );
        pointer(&s).0
    };
    assert!(near2(from("8.5cm", "1.5cm"), (9.0, 4.0))); // right
    assert!(near2(from("3cm", "5.5cm"), (6.0, 5.0))); // below
    assert!(near2(from("3cm", "-3.5cm"), (6.0, 3.0))); // above
                                                       // beside and below: the side wins
    assert!(near2(from("-2cm", "9cm"), (3.0, 4.0)));
}

#[test]
fn the_caption_gap_keeps_the_pointer_off_the_box() {
    let s = caption(
        r#"draw:caption-point-x="8.5cm" draw:caption-point-y="1.5cm""#,
        r#"draw:caption-gap="0.2cm""#,
    );
    assert!(near2(pointer(&s).0, (9.2, 4.0)));
}

#[test]
fn a_point_inside_the_box_or_no_point_has_no_pointer() {
    let s = caption(
        r#"draw:caption-point-x="3cm" draw:caption-point-y="1cm""#,
        "",
    );
    assert_eq!(s.len(), 1);
    assert_eq!(caption("", "").len(), 1);
    let s = caption(r#"draw:caption-point-x="3cm""#, "");
    assert_eq!(s.len(), 1);
}

#[test]
fn the_pointer_follows_a_transform() {
    let s = caption(
        r#"draw:transform="translate (1cm 1cm)" draw:caption-point-x="8.5cm" draw:caption-point-y="1.5cm""#,
        "",
    );
    // the transform puts the box (x, y) at its origin: the offsets are the same, shifted by 1 cm
    let ((_, _), (bx, _)) = pointer(&s);
    assert!(bx > 9.0);
}

// ---- date fields ------------------------------------------------------------------------------------------------------

fn field_text(inner: &str, auto: &str) -> String {
    let o = text_op(&format!("<text:p>{inner}</text:p>"), auto);
    text_of_shape(&only_shape(&o))
}

const DATE_STYLES: &str = r##"<number:date-style style:name="D1"><number:day number:style="long"/><number:text>.</number:text><number:month number:style="long"/><number:text>.</number:text><number:year number:style="long"/></number:date-style>
<number:date-style style:name="D2"><number:day-of-week number:style="long"/><number:text>, </number:text><number:month number:textual="true" number:style="long"/><number:text> </number:text><number:day/><number:text>, </number:text><number:year/></number:date-style>
<number:date-style style:name="D3" number:language="ja"><number:year number:style="long"/><number:text>年</number:text><number:month number:textual="true"/><number:day/><number:text>日 </number:text><number:day-of-week/></number:date-style>
<number:time-style style:name="T1"><number:hours number:style="long"/><number:text>:</number:text><number:minutes number:style="long"/><number:text>:</number:text><number:seconds number:style="long"/><number:am-pm/></number:time-style>"##;

#[test]
fn a_date_that_is_not_fixed_shows_the_current_date_in_its_style() {
    dates::set_now_for_tests(None);
    let t = |attrs: &str| {
        field_text(
            &format!(r#"<text:date style:data-style-name="D1" {attrs}>stale</text:date>"#),
            DATE_STYLES,
        )
    };
    assert_eq!(t(""), "09.10.2026");
    assert_eq!(t(r#"text:fixed="false""#), "09.10.2026");
    // fixed: the stored text
    assert_eq!(t(r#"text:fixed="true""#), "stale");
    // without a style: the ISO date
    assert_eq!(
        field_text(r#"<text:date>1999</text:date>"#, DATE_STYLES),
        "2026-10-09"
    );
    // an unknown style: the same
    assert_eq!(
        field_text(r#"<text:date style:data-style-name="nope"/>"#, DATE_STYLES),
        "2026-10-09"
    );
}

#[test]
fn a_time_that_is_not_fixed_shows_the_current_time_in_its_style() {
    dates::set_now_for_tests(None);
    let t = |attrs: &str| {
        field_text(
            &format!(r#"<text:time style:data-style-name="T1" {attrs}>10:58:52</text:time>"#),
            DATE_STYLES,
        )
    };
    assert_eq!(t(""), "12:34:56PM");
    assert_eq!(t(r#"text:fixed="true""#), "10:58:52");
}

#[test]
fn the_date_style_writes_names_and_the_number_of_digits() {
    dates::set_now_for_tests(None);
    assert_eq!(
        field_text(r#"<text:date style:data-style-name="D2"/>"#, DATE_STYLES),
        "Friday, October 9, 26"
    );
    assert_eq!(
        field_text(r#"<text:date style:data-style-name="D3"/>"#, DATE_STYLES),
        "2026年10月9日 金"
    );
}

#[test]
fn an_injected_moment_is_what_the_field_shows() {
    dates::set_now_for_tests(Some(moment_of(951_782_400))); // 2000-02-29
    assert_eq!(
        field_text(r#"<text:date style:data-style-name="D1"/>"#, DATE_STYLES),
        "29.02.2000"
    );
    dates::set_now_for_tests(None);
}

#[test]
fn moments_are_calendar_dates() {
    let m = moment_of(0);
    assert_eq!(
        (m.year, m.month, m.day, m.weekday, m.hour),
        (1970, 1, 1, 3, 0)
    );
    let m = moment_of(951_782_400);
    assert_eq!((m.year, m.month, m.day), (2000, 2, 29));
    let m = moment_of(951_782_400 + 86_400);
    assert_eq!((m.year, m.month, m.day), (2000, 3, 1));
    let m = moment_of(1_791_549_296); // 2026-10-09 12:34:56 UTC
    assert_eq!(
        (m.year, m.month, m.day, m.weekday, m.hour, m.minute, m.second),
        (2026, 10, 9, 4, 12, 34, 56)
    );
    // before 1970
    let m = moment_of(-1);
    assert_eq!(
        (m.year, m.month, m.day, m.hour, m.minute, m.second),
        (1969, 12, 31, 23, 59, 59)
    );
    // far away values do not panic
    let _ = moment_of(i64::MAX / 2);
    let _ = moment_of(i64::MIN / 2);
}

#[test]
fn time_styles_and_the_twelve_hour_clock() {
    let m = Moment {
        year: 2026,
        month: 1,
        day: 2,
        weekday: 4,
        hour: 0,
        minute: 5,
        second: 7,
    };
    let node = |xml: &str| -> crate::preview::office::docx_xml::Node {
        let x = format!(
            "<root {}>{xml}</root>",
            crate::preview::office::tests_odp::pns()
        );
        super::chart_read::read_tree(x.as_bytes())
            .unwrap()
            .nodes()
            .next()
            .unwrap()
            .clone()
    };
    let t = node(
        r#"<number:time-style><number:hours number:style="long"/><number:text>:</number:text><number:minutes number:style="long"/><number:text>:</number:text><number:seconds number:style="long"/><number:am-pm/></number:time-style>"#,
    );
    assert_eq!(format(Some(&t), &m), "12:05:07AM");
    let pm = Moment { hour: 15, ..m };
    assert_eq!(format(Some(&t), &pm), "03:05:07PM");
    let plain = node(
        r#"<number:time-style><number:hours/><number:text>h</number:text><number:minutes/></number:time-style>"#,
    );
    assert_eq!(format(Some(&plain), &pm), "15h5");
    assert_eq!(format(None, &pm), "2026-01-02");
}

#[test]
fn a_current_date_declaration_shows_today_in_the_declarations_format() {
    dates::set_now_for_tests(None);
    let decl = r#"<presentation:date-time-decl presentation:name="dtd1" presentation:source="current-date" style:data-style-name="D1"/>"#;
    let page = r#"<draw:page draw:name="p" draw:style-name="dp1" draw:master-page-name="Default" presentation:use-date-time-name="dtd1"></draw:page>"#;
    let dp = r#"<style:style style:name="dp1" style:family="drawing-page"><style:drawing-page-properties presentation:display-date-time="true"/></style:style>"#;
    let master = r#"<draw:frame presentation:style-name="Mpr1" draw:layer="backgroundobjects" svg:width="6cm" svg:height="1cm" svg:x="1cm" svg:y="14cm" presentation:class="date-time"><draw:text-box><text:p><presentation:date-time/></text:p></draw:text-box></draw:frame>"#;
    let o = Op::new(&format!("{decl}{page}"))
        .styles_auto(LAYOUT)
        .master(&master_with(master, MDP))
        .styles(&format!(
            r#"<style:style style:name="Mdp1" style:family="drawing-page"/>{DATE_STYLES}"#
        ))
        .auto(&format!("{}{dp}", filled()));
    let sc = scene(&o);
    let texts: Vec<String> = shapes_of(&sc).iter().map(|s| text_of_shape(s)).collect();
    assert!(texts.contains(&"09.10.2026".to_string()), "{texts:?}");
    // a fixed declaration still shows its text
    let fixed = r#"<presentation:date-time-decl presentation:name="dtd1" presentation:source="fixed">Autumn</presentation:date-time-decl>"#;
    let o = Op::new(&format!("{fixed}{page}"))
        .styles_auto(LAYOUT)
        .master(&master_with(master, MDP))
        .styles(r#"<style:style style:name="Mdp1" style:family="drawing-page"/>"#)
        .auto(&format!("{}{dp}", filled()));
    let texts: Vec<String> = shapes_of(&scene(&o))
        .iter()
        .map(|s| text_of_shape(s))
        .collect();
    assert!(texts.contains(&"Autumn".to_string()), "{texts:?}");
}

// ---- local time and automatic order -----------------------------------------------------------------------------------

#[cfg(unix)]
#[test]
fn the_current_moment_is_in_the_local_time_zone() {
    // The offset the platform's own `date` reports for now.
    let out = std::process::Command::new("date")
        .arg("+%z")
        .output()
        .expect("date runs");
    let z = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let sign = if z.starts_with('-') { -1 } else { 1 };
    let digits: i64 = z.trim_start_matches(['+', '-']).parse().expect("+hhmm");
    let expected = sign * ((digits / 100) * 3600 + (digits % 100) * 60);
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    assert_eq!(dates::local_offset(secs), expected);
    assert!(expected.abs() <= 14 * 3600);
}

#[test]
fn a_moment_shifted_by_a_zone_crosses_midnight() {
    // 2026-10-08 20:00 UTC is already the 9th in Japan (+9 h): the date field must say 9.
    let utc = 1_791_489_600; // 2026-10-08 20:00:00 UTC
    assert_eq!(moment_of(utc).day, 8);
    let jst = moment_of(utc + 9 * 3600);
    assert_eq!((jst.month, jst.day, jst.hour), (10, 9, 5));
    // and a zone behind UTC goes back across the midnight
    let la = moment_of(1_791_504_000 - 7 * 3600); // 2026-10-09 00:00 UTC at -7 h
    assert_eq!((la.day, la.hour), (8, 17));
}

use crate::preview::office::docx_xml::{Kid, Node};

fn el(name: &str, attrs: &[(&str, &str)], kids: Vec<Kid>) -> Kid {
    Kid::N(Node {
        prefix: "number".into(),
        name: name.into(),
        attrs: attrs
            .iter()
            .map(|(k, v)| (format!("number:{k}"), (*v).to_string()))
            .collect(),
        kids,
    })
}

fn txt(s: &str) -> Kid {
    el("text", &[], vec![Kid::T(s.into())])
}

fn style_of(name: &str, attrs: &[(&str, &str)], kids: Vec<Kid>) -> Node {
    match el(name, attrs, kids) {
        Kid::N(n) => n,
        Kid::T(_) => unreachable!(),
    }
}

fn dmy(extra: &[(&str, &str)]) -> Node {
    style_of(
        "date-style",
        extra,
        vec![
            el("day", &[("style", "long")], vec![]),
            txt("."),
            el("month", &[("style", "long")], vec![]),
            txt("."),
            el("year", &[("style", "long")], vec![]),
        ],
    )
}

const T: i64 = 1_791_549_296; // 2026-10-09 12:34:56 UTC

fn auto(extra: &[(&str, &str)]) -> String {
    let mut a = vec![("automatic-order", "true")];
    a.extend_from_slice(extra);
    format(Some(&dmy(&a)), &moment_of(T))
}

#[test]
fn automatic_order_follows_the_locale_of_the_style() {
    dates::set_system_locale_for_tests(None);
    // en-US: month first; en-GB / de / it: day first (the style's own order); zh / ko: year first
    assert_eq!(auto(&[("language", "en"), ("country", "US")]), "10.09.2026");
    assert_eq!(auto(&[("language", "en"), ("country", "GB")]), "09.10.2026");
    assert_eq!(auto(&[("language", "de"), ("country", "DE")]), "09.10.2026");
    assert_eq!(auto(&[("language", "zh"), ("country", "CN")]), "2026.10.09");
    assert_eq!(auto(&[("language", "ko")]), "2026.10.09");
    // English without a country is the US order
    assert_eq!(auto(&[("language", "en")]), "10.09.2026");
}

#[test]
fn automatic_order_in_japanese_is_libreoffices_short_form() {
    assert_eq!(
        auto(&[("language", "ja"), ("country", "JP")]),
        "26年10月9日"
    );
}

#[test]
fn a_style_without_a_language_follows_the_system_locale() {
    dates::set_system_locale_for_tests(Some("ja_JP.UTF-8"));
    assert_eq!(auto(&[]), "26年10月9日");
    dates::set_system_locale_for_tests(Some("de-DE"));
    assert_eq!(auto(&[]), "09.10.2026");
    dates::set_system_locale_for_tests(Some("C"));
    assert_eq!(auto(&[]), "10.09.2026");
    dates::set_system_locale_for_tests(None);
    assert_eq!(auto(&[]), "10.09.2026");
}

#[test]
fn a_style_without_automatic_order_keeps_its_own_order() {
    dates::set_system_locale_for_tests(Some("ja-JP"));
    assert_eq!(format(Some(&dmy(&[])), &moment_of(T)), "09.10.2026");
    assert_eq!(
        format(Some(&dmy(&[("automatic-order", "false")])), &moment_of(T)),
        "09.10.2026"
    );
    dates::set_system_locale_for_tests(None);
}

#[test]
fn automatic_order_leaves_a_style_without_each_of_day_month_year_once() {
    dates::set_system_locale_for_tests(None);
    let a = [("automatic-order", "true")];
    let two = style_of(
        "date-style",
        &a,
        vec![el("day", &[], vec![]), txt("/"), el("month", &[], vec![])],
    );
    assert_eq!(format(Some(&two), &moment_of(T)), "9/10");
    let twice = style_of(
        "date-style",
        &a,
        vec![
            el("day", &[], vec![]),
            txt("/"),
            el("day", &[], vec![]),
            txt("/"),
            el("year", &[], vec![]),
        ],
    );
    assert_eq!(format(Some(&twice), &moment_of(T)), "9/9/26");
    // a time style is never reordered
    let time = style_of(
        "time-style",
        &[("automatic-order", "true"), ("language", "ja")],
        vec![
            el("minutes", &[], vec![]),
            txt(":"),
            el("hours", &[], vec![]),
        ],
    );
    assert_eq!(format(Some(&time), &moment_of(T)), "34:12");
}

#[test]
fn automatic_text_colour_reads_on_the_highlight_it_is_on() {
    let run_colour = |span_style: &str, text_style: &str| {
        let sh = format!(
            r#"<draw:rect draw:style-name="gr1" svg:x="1cm" svg:y="1cm" svg:width="6cm" svg:height="3cm"><text:p><text:span {span_style}>x</text:span></text:p></draw:rect>"#
        );
        let o = op(&slide_page(&sh)).auto(&format!(
            r##"{}{text_style}"##,
            gr(r##"draw:fill="solid" draw:fill-color="#000000""##)
        ));
        let s = only_shape(&o);
        s.text.unwrap().paragraphs[0].runs[0].fill.clone()
    };
    // on a black shape the automatic colour is white ...
    assert_eq!(run_colour("", ""), Fill::Solid(Rgba::WHITE));
    // ... but on a white highlight over it, black
    let hl = r##"<style:style style:name="T1" style:family="text"><style:text-properties fo:background-color="#ffffff"/></style:style>"##;
    assert_eq!(
        run_colour(r#"text:style-name="T1""#, hl),
        Fill::Solid(Rgba::BLACK)
    );
    // a colour of its own is kept
    let own = r##"<style:style style:name="T1" style:family="text"><style:text-properties fo:background-color="#ffffff" fo:color="#ff0000"/></style:style>"##;
    assert_eq!(
        run_colour(r#"text:style-name="T1""#, own),
        Fill::Solid(Rgba::rgb(255, 0, 0))
    );
}
