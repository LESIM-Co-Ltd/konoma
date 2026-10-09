//! Tests of the SVG writer: structure, pixels (through the project's trusted rasterizer),
//! escaping, budgets and hostile numbers. Text layout has its own file (`text_tests.rs`).

use std::path::Path;
use std::sync::Arc;

use super::model::*;
use super::{render_svg, Rendered};

/// A 960 x 540 px slide.
pub(super) const W_PX: f64 = 960.0;
pub(super) const H_PX: f64 = 540.0;

pub(super) fn e(px: f64) -> f64 {
    px * EMU_PER_PX
}

pub(super) fn scene(items: Vec<Item>) -> SlideScene {
    SlideScene {
        width: e(W_PX),
        height: e(H_PX),
        background: Fill::Solid(Rgba::WHITE),
        items,
        truncated: false,
    }
}

pub(super) fn no_media(_: &str) -> Option<Arc<Vec<u8>>> {
    None
}

pub(super) fn rect_item(x: f64, y: f64, w: f64, h: f64, fill: Rgba) -> Item {
    let mut s = ShapeItem::new(Xfrm::rect(e(x), e(y), e(w), e(h)), Geometry::Rect);
    s.fill = Fill::Solid(fill);
    Item::Shape(s)
}

pub(super) fn raster(r: &Rendered) -> image::RgbaImage {
    crate::preview::svg::rasterize_trusted(r.svg.as_bytes(), Path::new("/slide.svg"), 960)
        .unwrap_or_else(|| {
            panic!(
                "the SVG does not rasterize:\n{}",
                &r.svg[..r.svg.len().min(3000)]
            )
        })
        .to_rgba8()
}

pub(super) fn at(img: &image::RgbaImage, x: u32, y: u32) -> [u8; 3] {
    let p = img.get_pixel(x, y).0;
    [p[0], p[1], p[2]]
}

pub(super) const RED: Rgba = Rgba::rgb(255, 0, 0);
pub(super) const BLUE: Rgba = Rgba::rgb(0, 0, 255);
pub(super) const GREEN: Rgba = Rgba::rgb(0, 160, 0);
pub(super) const WHITE: [u8; 3] = [255, 255, 255];

fn near(a: [u8; 3], b: [u8; 3], tol: i32) -> bool {
    (0..3).all(|i| (a[i] as i32 - b[i] as i32).abs() <= tol)
}

fn is_red(p: [u8; 3]) -> bool {
    near(p, [255, 0, 0], 12)
}
fn is_blue(p: [u8; 3]) -> bool {
    near(p, [0, 0, 255], 12)
}
fn is_white(p: [u8; 3]) -> bool {
    near(p, WHITE, 6)
}

fn draw(items: Vec<Item>) -> image::RgbaImage {
    raster(&render_svg(&scene(items), &no_media))
}

fn png(w: u32, h: u32, f: impl Fn(u32, u32) -> [u8; 4]) -> Vec<u8> {
    let mut img = image::RgbaImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            img.put_pixel(x, y, image::Rgba(f(x, y)));
        }
    }
    let mut out = Vec::new();
    image::DynamicImage::ImageRgba8(img)
        .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
        .unwrap();
    out
}

fn media_of(bytes: Vec<u8>) -> impl Fn(&str) -> Option<Arc<Vec<u8>>> {
    let a = Arc::new(bytes);
    move |_| Some(a.clone())
}

// ---- structure ------------------------------------------------------------------------------

#[test]
fn root_element_and_size() {
    let r = render_svg(&scene(vec![]), &no_media);
    assert!(r
        .svg
        .starts_with(r#"<svg xmlns="http://www.w3.org/2000/svg""#));
    assert!(r.svg.contains(r#"viewBox="0 0 960 540""#));
    assert!(r.svg.contains(r#"width="960" height="540""#));
    assert!(r.svg.ends_with("</svg>"));
    assert!(!r.truncated);
    // the background is painted first
    assert!(
        r.svg
            .contains(r##"<path d="M0 0 L960 0 L960 540 L0 540 Z" fill="#ffffff""##),
        "{}",
        r.svg
    );
    // painted before everything else
    let bg = r.svg.find("#ffffff").unwrap();
    let s = scene(vec![rect_item(1.0, 1.0, 5.0, 5.0, RED)]);
    let r = render_svg(&s, &no_media);
    assert!(r.svg.find("#ff0000").unwrap() > r.svg.find("#ffffff").unwrap());
    let _ = bg;
}

#[test]
fn background_kinds() {
    for (bg, expect) in [
        (Fill::Solid(Rgba::rgb(10, 200, 30)), [10, 200, 30]),
        (Fill::None, [255, 255, 255]),
    ] {
        let mut s = scene(vec![]);
        s.background = bg;
        let img = raster(&render_svg(&s, &no_media));
        assert_eq!(at(&img, 480, 270), expect);
        assert_eq!(at(&img, 3, 3), expect);
    }
    let mut s = scene(vec![]);
    s.background = Fill::Gradient(Gradient::linear(0.0, vec![(0.0, RED), (1.0, BLUE)]));
    let img = raster(&render_svg(&s, &no_media));
    assert!(at(&img, 5, 270)[0] > 230 && at(&img, 5, 270)[2] < 25);
    assert!(at(&img, 954, 270)[2] > 230 && at(&img, 954, 270)[0] < 25);
}

#[test]
fn scene_truncation_is_carried() {
    let mut s = scene(vec![]);
    s.truncated = true;
    assert!(render_svg(&s, &no_media).truncated);
}

#[test]
fn tiny_or_hostile_slide_sizes_are_clamped() {
    for (w, h) in [
        (0.0, 0.0),
        (f64::NAN, f64::INFINITY),
        (-5.0, 1e30),
        (1e300, 1e300),
    ] {
        let mut s = scene(vec![rect_item(0.0, 0.0, 10.0, 10.0, RED)]);
        s.width = w;
        s.height = h;
        let r = render_svg(&s, &no_media);
        assert!(!r.svg.contains("NaN") && !r.svg.contains("inf"), "{w} {h}");
        assert!(r.svg.contains("viewBox=\"0 0 "));
    }
}

// ---- shapes ---------------------------------------------------------------------------------

#[test]
fn filled_rect_centre_and_outside() {
    let img = draw(vec![rect_item(100.0, 100.0, 200.0, 100.0, RED)]);
    assert!(is_red(at(&img, 200, 150)));
    assert!(is_red(at(&img, 105, 105)));
    assert!(is_white(at(&img, 90, 150)));
    assert!(is_white(at(&img, 310, 150)));
    assert!(is_white(at(&img, 200, 90)));
    assert!(is_white(at(&img, 200, 210)));
}

#[test]
fn ellipse_corner_is_background() {
    let mut s = ShapeItem::new(
        Xfrm::rect(e(100.0), e(100.0), e(200.0), e(100.0)),
        Geometry::Ellipse,
    );
    s.fill = Fill::Solid(BLUE);
    let img = draw(vec![Item::Shape(s)]);
    assert!(is_blue(at(&img, 200, 150)));
    assert!(is_white(at(&img, 104, 104)), "{:?}", at(&img, 104, 104));
    assert!(is_white(at(&img, 296, 196)));
    assert!(is_blue(at(&img, 120, 150)));
}

#[test]
fn rotated_rect_corner() {
    // 200 x 40 centred on (480, 270), rotated 90 degrees: becomes 40 wide and 200 tall
    let mut s = ShapeItem::new(
        Xfrm {
            rot_deg: 90.0,
            ..Xfrm::rect(e(380.0), e(250.0), e(200.0), e(40.0))
        },
        Geometry::Rect,
    );
    s.fill = Fill::Solid(RED);
    let img = draw(vec![Item::Shape(s)]);
    assert!(is_red(at(&img, 480, 270 - 90)));
    assert!(is_red(at(&img, 480, 270 + 90)));
    assert!(is_white(at(&img, 480 + 60, 270)));
    assert!(is_white(at(&img, 480 - 60, 270)));
    // 45 degrees: the unrotated corner is no longer covered
    let mut s = ShapeItem::new(
        Xfrm {
            rot_deg: 45.0,
            ..Xfrm::rect(e(380.0), e(250.0), e(200.0), e(40.0))
        },
        Geometry::Rect,
    );
    s.fill = Fill::Solid(RED);
    let img = draw(vec![Item::Shape(s)]);
    assert!(is_white(at(&img, 385, 253)), "{:?}", at(&img, 385, 253));
    assert!(is_red(at(&img, 480 + 50, 270 + 50)));
    assert!(is_red(at(&img, 480 - 50, 270 - 50)));
}

#[test]
fn flipped_shape_with_asymmetric_geometry() {
    // A right triangle with its right angle at the bottom-left; flip_h moves it to the bottom-right.
    let tri = Geometry::Paths(vec![GeomPath {
        w: 100.0,
        h: 100.0,
        fill_mode: PathFill::Norm,
        stroke: true,
        cmds: vec![
            PathCmd::MoveTo(Pt::new(0.0, 0.0)),
            PathCmd::LineTo(Pt::new(0.0, 100.0)),
            PathCmd::LineTo(Pt::new(100.0, 100.0)),
            PathCmd::Close,
        ],
    }]);
    for (flip_h, flip_v, inside, outside) in [
        (false, false, (110, 190), (190, 110)),
        (true, false, (190, 190), (110, 110)),
        (false, true, (110, 110), (190, 190)),
        (true, true, (190, 110), (110, 190)),
    ] {
        let mut s = ShapeItem::new(
            Xfrm {
                flip_h,
                flip_v,
                ..Xfrm::rect(e(100.0), e(100.0), e(100.0), e(100.0))
            },
            tri.clone(),
        );
        s.fill = Fill::Solid(GREEN);
        let img = draw(vec![Item::Shape(s)]);
        assert!(!is_white(at(&img, inside.0, inside.1)), "{flip_h} {flip_v}");
        assert!(
            is_white(at(&img, outside.0, outside.1)),
            "{flip_h} {flip_v}"
        );
    }
}

#[test]
fn path_scaling_to_the_box() {
    // a triangle in a 10 x 10 path space on a 200 x 100 px box
    let tri = Geometry::Paths(vec![GeomPath {
        w: 10.0,
        h: 10.0,
        fill_mode: PathFill::Norm,
        stroke: false,
        cmds: vec![
            PathCmd::MoveTo(Pt::new(0.0, 10.0)),
            PathCmd::LineTo(Pt::new(5.0, 0.0)),
            PathCmd::LineTo(Pt::new(10.0, 10.0)),
            PathCmd::Close,
        ],
    }]);
    let mut s = ShapeItem::new(Xfrm::rect(e(100.0), e(100.0), e(200.0), e(100.0)), tri);
    s.fill = Fill::Solid(RED);
    let img = draw(vec![Item::Shape(s)]);
    assert!(is_red(at(&img, 200, 180)));
    assert!(is_white(at(&img, 105, 120)));
    assert!(is_white(at(&img, 295, 120)));
    assert!(is_red(at(&img, 200, 110)));
}

#[test]
fn arc_to_draws_a_half_disc() {
    // M(0,50) arcTo wr=50 hr=50 st=180 sw=180 closes a half disc over a 100x50 box region
    let g = Geometry::Paths(vec![GeomPath {
        w: 100.0,
        h: 100.0,
        fill_mode: PathFill::Norm,
        stroke: false,
        cmds: vec![
            PathCmd::MoveTo(Pt::new(0.0, 50.0)),
            PathCmd::ArcTo {
                wr: 50.0,
                hr: 50.0,
                st_deg: 180.0,
                sw_deg: 180.0,
            },
            PathCmd::Close,
        ],
    }]);
    let mut s = ShapeItem::new(Xfrm::rect(e(100.0), e(100.0), e(200.0), e(200.0)), g);
    s.fill = Fill::Solid(BLUE);
    let img = draw(vec![Item::Shape(s)]);
    // the upper half of the circle: (200, 150) inside, (200, 250) outside, (110, 110) outside
    assert!(is_blue(at(&img, 200, 150)));
    assert!(is_white(at(&img, 200, 250)));
    assert!(is_white(at(&img, 105, 105)));
    // the arc's extreme is the top of the box
    assert!(is_blue(at(&img, 200, 105)));
}

#[test]
fn stroke_false_path_has_no_outline() {
    let g = |stroke| {
        Geometry::Paths(vec![GeomPath {
            w: 0.0,
            h: 0.0,
            fill_mode: PathFill::None,
            stroke,
            cmds: vec![
                PathCmd::MoveTo(Pt::new(0.0, e(50.0))),
                PathCmd::LineTo(Pt::new(e(200.0), e(50.0))),
            ],
        }])
    };
    for (stroke, drawn) in [(true, true), (false, false)] {
        let mut s = ShapeItem::new(
            Xfrm::rect(e(100.0), e(100.0), e(200.0), e(100.0)),
            g(stroke),
        );
        s.line = Some(Line::solid(e(6.0), RED));
        let img = draw(vec![Item::Shape(s)]);
        assert_eq!(!is_white(at(&img, 200, 150)), drawn);
    }
}

#[test]
fn fill_modes_lighten_and_darken() {
    let mk = |mode| {
        let g = Geometry::Paths(vec![GeomPath {
            w: 0.0,
            h: 0.0,
            fill_mode: mode,
            stroke: false,
            cmds: vec![
                PathCmd::MoveTo(Pt::new(0.0, 0.0)),
                PathCmd::LineTo(Pt::new(e(100.0), 0.0)),
                PathCmd::LineTo(Pt::new(e(100.0), e(100.0))),
                PathCmd::LineTo(Pt::new(0.0, e(100.0))),
                PathCmd::Close,
            ],
        }]);
        let mut s = ShapeItem::new(Xfrm::rect(e(100.0), e(100.0), e(100.0), e(100.0)), g);
        s.fill = Fill::Solid(Rgba::rgb(100, 100, 100));
        at(&draw(vec![Item::Shape(s)]), 150, 150)
    };
    let norm = mk(PathFill::Norm)[0];
    assert!((norm as i32 - 100).abs() <= 1);
    assert!(mk(PathFill::Lighten)[0] > mk(PathFill::LightenLess)[0]);
    assert!(mk(PathFill::LightenLess)[0] > norm);
    assert!(mk(PathFill::Darken)[0] < mk(PathFill::DarkenLess)[0]);
    assert!(mk(PathFill::DarkenLess)[0] < norm);
    // a `none` fill mode paints nothing
    let g = Geometry::Paths(vec![GeomPath {
        fill_mode: PathFill::None,
        stroke: false,
        cmds: vec![
            PathCmd::MoveTo(Pt::new(0.0, 0.0)),
            PathCmd::LineTo(Pt::new(e(100.0), e(100.0))),
        ],
        ..Default::default()
    }]);
    let mut s = ShapeItem::new(Xfrm::rect(e(100.0), e(100.0), e(100.0), e(100.0)), g);
    s.fill = Fill::Solid(RED);
    assert!(is_white(at(&draw(vec![Item::Shape(s)]), 110, 190)));
}

#[test]
fn alpha_fill_blends_with_the_background() {
    let mut s = ShapeItem::new(
        Xfrm::rect(e(100.0), e(100.0), e(100.0), e(100.0)),
        Geometry::Rect,
    );
    s.fill = Fill::Solid(Rgba::new(0, 0, 0, 0.5));
    let img = draw(vec![Item::Shape(s)]);
    let p = at(&img, 150, 150);
    assert!((p[0] as i32 - 128).abs() <= 3, "{p:?}");
    let mut s = ShapeItem::new(
        Xfrm::rect(e(100.0), e(100.0), e(100.0), e(100.0)),
        Geometry::Rect,
    );
    s.fill = Fill::Solid(Rgba::new(0, 0, 0, 0.0));
    assert!(is_white(at(&draw(vec![Item::Shape(s)]), 150, 150)));
}

// ---- groups ---------------------------------------------------------------------------------

fn group(xfrm: Xfrm, off: (f64, f64), ext: (f64, f64), items: Vec<Item>) -> Item {
    Item::Group(GroupItem {
        xfrm,
        child_off: off,
        child_ext: ext,
        items,
    })
}

#[test]
fn group_maps_child_space_onto_its_box() {
    // child space 100 x 50 (EMU-px) mapped onto a 200 x 100 box at (100, 100): scale 2
    let g = group(
        Xfrm::rect(e(100.0), e(100.0), e(200.0), e(100.0)),
        (0.0, 0.0),
        (e(100.0), e(50.0)),
        vec![rect_item(10.0, 10.0, 10.0, 10.0, RED)],
    );
    let img = draw(vec![g]);
    assert!(is_red(at(&img, 100 + 30, 100 + 30)));
    assert!(is_white(at(&img, 100 + 15, 100 + 15)));
    assert!(is_white(at(&img, 100 + 45, 100 + 45)));
}

#[test]
fn group_with_child_offset() {
    // child origin at (500, 500): a child rect at (500, 500) lands at the group's top-left
    let g = group(
        Xfrm::rect(e(100.0), e(100.0), e(100.0), e(100.0)),
        (e(500.0), e(500.0)),
        (e(100.0), e(100.0)),
        vec![rect_item(500.0, 500.0, 20.0, 20.0, BLUE)],
    );
    let img = draw(vec![g]);
    assert!(is_blue(at(&img, 110, 110)));
    assert!(is_white(at(&img, 130, 130)));
}

#[test]
fn group_flip_moves_children() {
    let g = |fh, fv| {
        group(
            Xfrm {
                flip_h: fh,
                flip_v: fv,
                ..Xfrm::rect(e(100.0), e(100.0), e(400.0), e(200.0))
            },
            (0.0, 0.0),
            (e(400.0), e(200.0)),
            vec![rect_item(0.0, 0.0, 100.0, 50.0, RED)],
        )
    };
    let img = draw(vec![g(true, false)]);
    assert!(is_red(at(&img, 450, 120)));
    assert!(is_white(at(&img, 150, 120)));
    let img = draw(vec![g(false, true)]);
    assert!(is_red(at(&img, 150, 280)));
    assert!(is_white(at(&img, 150, 120)));
    let img = draw(vec![g(true, true)]);
    assert!(is_red(at(&img, 450, 280)));
    assert!(is_white(at(&img, 150, 120)));
}

#[test]
fn nested_groups_with_rotation() {
    // outer group rotated 90 degrees about its centre (300, 200); the child rect at (200, 100)
    // size 100 x 50 (top-left of the group) moves to the top-right: centre (250,125) -> (375,150)
    let inner = group(
        Xfrm::rect(e(200.0), e(100.0), e(200.0), e(200.0)),
        (e(200.0), e(100.0)),
        (e(200.0), e(200.0)),
        vec![rect_item(200.0, 100.0, 100.0, 50.0, RED)],
    );
    let outer = group(
        Xfrm {
            rot_deg: 90.0,
            ..Xfrm::rect(e(200.0), e(100.0), e(200.0), e(200.0))
        },
        (e(200.0), e(100.0)),
        (e(200.0), e(200.0)),
        vec![inner],
    );
    let img = draw(vec![outer]);
    assert!(is_red(at(&img, 375, 150)), "{:?}", at(&img, 375, 150));
    assert!(is_white(at(&img, 250, 125)));
    assert!(is_white(at(&img, 225, 275)));
}

#[test]
fn nested_flip_and_rotation_compose() {
    // inner group is flipped horizontally inside an outer group that is rotated 180 degrees:
    // the two together put a left-hand child back on the left, at the bottom
    let inner = group(
        Xfrm {
            flip_h: true,
            ..Xfrm::rect(e(100.0), e(100.0), e(200.0), e(100.0))
        },
        (0.0, 0.0),
        (e(200.0), e(100.0)),
        vec![rect_item(0.0, 0.0, 50.0, 50.0, BLUE)],
    );
    // alone: the child is at the right-hand top corner of the box (250..300, 100..150)
    let img = draw(vec![inner.clone()]);
    assert!(is_blue(at(&img, 275, 125)));
    let outer = group(
        Xfrm {
            rot_deg: 180.0,
            ..Xfrm::rect(e(100.0), e(100.0), e(200.0), e(100.0))
        },
        (e(100.0), e(100.0)),
        (e(200.0), e(100.0)),
        vec![inner],
    );
    let img = draw(vec![outer]);
    // rotated by 180 about (200, 150): (275,125) -> (125,175)
    assert!(is_blue(at(&img, 125, 175)));
    assert!(is_white(at(&img, 275, 125)));
}

#[test]
fn group_depth_limit() {
    let mut item = rect_item(0.0, 0.0, 10.0, 10.0, RED);
    for _ in 0..(super::svg::MAX_GROUP_DEPTH + 40) {
        item = group(
            Xfrm::rect(0.0, 0.0, e(100.0), e(100.0)),
            (0.0, 0.0),
            (e(100.0), e(100.0)),
            vec![item],
        );
    }
    let r = render_svg(&scene(vec![item]), &no_media);
    assert!(r.truncated);
    // never more nesting in the output than the limit
    assert!(r.svg.matches("<g ").count() <= super::svg::MAX_GROUP_DEPTH + 1);
}

#[test]
fn degenerate_group_extents_do_not_divide_by_zero() {
    for ext in [(0.0, 0.0), (f64::NAN, 5.0), (-1.0, -1.0)] {
        let g = group(
            Xfrm::rect(e(10.0), e(10.0), e(100.0), e(100.0)),
            (f64::NAN, 0.0),
            ext,
            vec![rect_item(0.0, 0.0, 10.0, 10.0, RED)],
        );
        let r = render_svg(&scene(vec![g]), &no_media);
        assert!(!r.svg.contains("NaN") && !r.svg.contains("inf"));
        raster(&r);
    }
}

// ---- fills ----------------------------------------------------------------------------------

fn filled(fill: Fill) -> image::RgbaImage {
    let mut s = ShapeItem::new(
        Xfrm::rect(e(100.0), e(100.0), e(400.0), e(200.0)),
        Geometry::Rect,
    );
    s.fill = fill;
    draw(vec![Item::Shape(s)])
}

#[test]
fn linear_gradient_directions() {
    let g = |a| Fill::Gradient(Gradient::linear(a, vec![(0.0, RED), (1.0, BLUE)]));
    let img = filled(g(0.0));
    assert!(at(&img, 105, 200)[0] > 230 && at(&img, 105, 200)[2] < 30);
    assert!(at(&img, 495, 200)[2] > 230 && at(&img, 495, 200)[0] < 30);
    let mid = at(&img, 300, 200);
    assert!(
        (mid[0] as i32 - 128).abs() < 20 && (mid[2] as i32 - 128).abs() < 20,
        "{mid:?}"
    );
    // 90 degrees: top to bottom
    let img = filled(g(90.0));
    assert!(at(&img, 300, 103)[0] > 230);
    assert!(at(&img, 300, 297)[2] > 230);
    // 45 degrees: top-left red corner, bottom-right blue corner
    let img = filled(g(45.0));
    assert!(at(&img, 101, 101)[0] > 230);
    assert!(at(&img, 498, 298)[2] > 230);
    // the other corners are the middle colour
    let c = at(&img, 498, 102);
    assert!(c[0] > 60 && c[2] > 60, "{c:?}");
}

#[test]
fn scaled_gradient_and_unsorted_stops() {
    let mut g = Gradient::linear(45.0, vec![(1.0, BLUE), (0.0, RED)]);
    g.kind = GradKind::Linear {
        angle_deg: 45.0,
        scaled: true,
    };
    let img = filled(Fill::Gradient(g));
    // scaled: the 45 degree line follows the box diagonal's aspect: corners get the end colours
    assert!(at(&img, 101, 101)[0] > 230);
    assert!(at(&img, 498, 298)[2] > 230);
}

#[test]
fn radial_rect_and_path_gradients_run_centre_to_edge() {
    for kind in [GradKind::Radial, GradKind::Rect, GradKind::Path] {
        let g = Gradient {
            kind,
            stops: vec![(0.0, RED), (1.0, BLUE)],
            fill_to_rect: (0.5, 0.5, 0.5, 0.5),
            rot_with_shape: true,
        };
        let img = filled(Fill::Gradient(g));
        assert!(
            at(&img, 300, 200)[0] > 220,
            "{kind:?} {:?}",
            at(&img, 300, 200)
        );
        assert!(
            at(&img, 102, 102)[2] > 200,
            "{kind:?} {:?}",
            at(&img, 102, 102)
        );
    }
}

#[test]
fn one_stop_gradient_is_solid_and_empty_is_nothing() {
    let img = filled(Fill::Gradient(Gradient::linear(0.0, vec![(0.5, RED)])));
    assert!(is_red(at(&img, 300, 200)));
    let img = filled(Fill::Gradient(Gradient::linear(0.0, vec![])));
    assert!(is_white(at(&img, 300, 200)));
}

#[test]
fn gradient_with_alpha_stops() {
    let g = Gradient::linear(
        0.0,
        vec![
            (0.0, Rgba::new(0, 0, 0, 1.0)),
            (1.0, Rgba::new(0, 0, 0, 0.0)),
        ],
    );
    let img = filled(Fill::Gradient(g));
    assert!(at(&img, 105, 200)[0] < 30);
    assert!(at(&img, 495, 200)[0] > 225);
}

fn fg_ratio(img: &image::RgbaImage, x0: u32, y0: u32, n: u32, fg: [u8; 3]) -> usize {
    let mut c = 0;
    for y in y0..y0 + n {
        for x in x0..x0 + n {
            if near(at(img, x, y), fg, 40) {
                c += 1;
            }
        }
    }
    c
}

#[test]
fn pattern_percentages() {
    for (preset, want) in [
        ("pct5", 3),
        ("pct10", 6),
        ("pct25", 16),
        ("pct50", 32),
        ("pct75", 48),
        ("pct90", 58),
    ] {
        let img = filled(Fill::Pattern {
            preset: preset.into(),
            fg: Rgba::BLACK,
            bg: Rgba::WHITE,
        });
        // the 8x8 tile repeats: count dark pixels in an aligned 8x8 window (tile origin is the slide's)
        let n = fg_ratio(&img, 104, 104, 8, [0, 0, 0]);
        assert!(
            (n as i32 - want).abs() <= 2,
            "{preset}: {n} dark of 64, want about {want}"
        );
    }
}

#[test]
fn pattern_lines_and_unknown() {
    let h = filled(Fill::Pattern {
        preset: "ltHorz".into(),
        fg: Rgba::BLACK,
        bg: Rgba::WHITE,
    });
    // one dark row in four
    let dark_rows = (104..112)
        .filter(|&y| near(at(&h, 200, y), [0, 0, 0], 40))
        .count();
    assert_eq!(dark_rows, 2);
    let v = filled(Fill::Pattern {
        preset: "dkVert".into(),
        fg: Rgba::BLACK,
        bg: Rgba::WHITE,
    });
    let dark_cols = (104..112)
        .filter(|&x| near(at(&v, x, 200), [0, 0, 0], 40))
        .count();
    assert_eq!(dark_cols, 4);
    // an unknown preset is a 50 % blend
    let u = filled(Fill::Pattern {
        preset: "nonsense<>\"".into(),
        fg: Rgba::BLACK,
        bg: Rgba::WHITE,
    });
    assert_eq!(fg_ratio(&u, 104, 104, 8, [0, 0, 0]), 32);
    // diagonals cross
    for p in [
        "dnDiag",
        "upDiag",
        "diagCross",
        "cross",
        "smGrid",
        "lgGrid",
        "dotGrid",
        "smCheck",
        "lgCheck",
        "horzBrick",
        "plaid",
        "weave",
        "zigZag",
        "wave",
        "trellis",
        "smConfetti",
        "lgConfetti",
    ] {
        let i = filled(Fill::Pattern {
            preset: p.into(),
            fg: Rgba::BLACK,
            bg: Rgba::WHITE,
        });
        let n = fg_ratio(&i, 104, 104, 8, [0, 0, 0]);
        assert!(n > 2 && n < 62, "{p}: {n}");
    }
}

#[test]
fn bayer_matrix_is_a_permutation() {
    let mut seen = [false; 64];
    for y in 0..8 {
        for x in 0..8 {
            let v = super::svg::bayer8(x, y);
            assert!(v < 64 && !seen[v], "({x},{y}) -> {v} repeats");
            seen[v] = true;
        }
    }
    assert!(seen.iter().all(|b| *b));
    // the first thresholds are spread out: the 4 lowest cells are in different quadrants
    let low: Vec<(usize, usize)> = (0..8)
        .flat_map(|y| (0..8).map(move |x| (x, y)))
        .filter(|&(x, y)| super::svg::bayer8(x, y) < 4)
        .collect();
    let quads: std::collections::HashSet<_> = low.iter().map(|&(x, y)| (x / 4, y / 4)).collect();
    assert_eq!(quads.len(), 4, "{low:?}");
    // pctN has exactly N * 64 / 100 set pixels
    for n in [5usize, 10, 20, 25, 30, 40, 50, 60, 70, 75, 80, 90] {
        let p = super::svg::pattern_pixels(&format!("pct{n}"));
        let set = p.iter().flatten().filter(|b| **b).count();
        assert_eq!(set, ((64.0 * n as f64 / 100.0).round()) as usize, "pct{n}");
    }
    // nested: every pixel of pct25 is also in pct50
    let (a, b) = (
        super::svg::pattern_pixels("pct25"),
        super::svg::pattern_pixels("pct50"),
    );
    for y in 0..8 {
        for x in 0..8 {
            assert!(!a[y][x] || b[y][x]);
        }
    }
}

#[test]
fn image_fill_stretch_crop_and_tile() {
    // left half red, right half blue
    let pic = png(8, 4, |x, _| {
        if x < 4 {
            [255, 0, 0, 255]
        } else {
            [0, 0, 255, 255]
        }
    });
    let media = media_of(pic);
    let mut s = ShapeItem::new(
        Xfrm::rect(e(100.0), e(100.0), e(400.0), e(200.0)),
        Geometry::Rect,
    );
    s.fill = Fill::Image(ImageFill::stretch("k"));
    let img = raster(&render_svg(&scene(vec![Item::Shape(s.clone())]), &media));
    assert!(is_red(at(&img, 150, 200)));
    assert!(is_blue(at(&img, 450, 200)));
    // crop away the left half
    let mut f = ImageFill::stretch("k");
    f.crop = (0.5, 0.0, 0.0, 0.0);
    s.fill = Fill::Image(f);
    let img = raster(&render_svg(&scene(vec![Item::Shape(s.clone())]), &media));
    // (bilinear smoothing blends within one source pixel of the old boundary)
    assert!(
        is_blue(at(&img, 250, 200)) && is_blue(at(&img, 450, 200)),
        "{:?} {:?}",
        at(&img, 250, 200),
        at(&img, 450, 200)
    );
    // fill_rect insets the image inside the shape (clipped to the shape)
    let mut f = ImageFill::stretch("k");
    f.mode = ImageMode::Stretch {
        fill_rect: (0.25, 0.0, 0.25, 0.0),
    };
    s.fill = Fill::Image(f);
    let img = raster(&render_svg(&scene(vec![Item::Shape(s.clone())]), &media));
    assert!(is_white(at(&img, 120, 200)));
    assert!(is_red(at(&img, 250, 200)));
    assert!(is_blue(at(&img, 350, 200)));
    // tile at native size: 8 x 4 px tiles repeat red/blue columns of 4 px
    let mut f = ImageFill::stretch("k");
    f.mode = ImageMode::Tile {
        sx: 1.0,
        sy: 1.0,
        tx: 0.0,
        ty: 0.0,
        align: RectAlign::TopLeft,
        flip: TileFlip::None,
    };
    s.fill = Fill::Image(f);
    let img = raster(&render_svg(&scene(vec![Item::Shape(s.clone())]), &media));
    assert!(is_red(at(&img, 102, 102)));
    assert!(is_blue(at(&img, 106, 102)));
    assert!(is_red(at(&img, 110, 102)));
    // flipped tiles mirror every other cell: red blue | blue red
    let mut f = ImageFill::stretch("k");
    f.mode = ImageMode::Tile {
        sx: 1.0,
        sy: 1.0,
        tx: 0.0,
        ty: 0.0,
        align: RectAlign::TopLeft,
        flip: TileFlip::X,
    };
    s.fill = Fill::Image(f);
    let img = raster(&render_svg(&scene(vec![Item::Shape(s)]), &media));
    assert!(is_red(at(&img, 102, 102)));
    assert!(is_blue(at(&img, 106, 102)));
    assert!(is_blue(at(&img, 110, 102)));
    assert!(is_red(at(&img, 114, 102)));
}

// ---- lines ----------------------------------------------------------------------------------

fn hline(l: Line) -> image::RgbaImage {
    let mut s = ShapeItem::new(
        Xfrm::rect(e(100.0), e(200.0), e(600.0), 0.0),
        Geometry::Line,
    );
    s.line = Some(l);
    draw(vec![Item::Shape(s)])
}

#[test]
fn solid_line_width_and_colour() {
    let img = hline(Line::solid(e(10.0), RED));
    assert!(is_red(at(&img, 300, 200)));
    assert!(is_red(at(&img, 300, 196)));
    assert!(is_white(at(&img, 300, 190)));
    assert!(is_white(at(&img, 300, 210)));
    assert!(is_white(at(&img, 90, 200)));
    // zero width is a hairline, not nothing
    let img = hline(Line::solid(0.0, Rgba::BLACK));
    assert!(!is_white(at(&img, 300, 200)) || !is_white(at(&img, 300, 199)));
}

#[test]
fn dashes_leave_gaps() {
    let mut l = Line::solid(e(4.0), BLUE);
    l.dash = Dash::Dash;
    let img = hline(l);
    let row: Vec<bool> = (100..400).map(|x| !is_white(at(&img, x, 200))).collect();
    let on = row.iter().filter(|b| **b).count();
    assert!(on > 100 && on < 220, "{on}");
    // dash 4w : gap 3w with w = 4: period 28 px
    let transitions = row.windows(2).filter(|w| w[0] != w[1]).count();
    assert!(transitions >= 18, "{transitions}");
    // every preset renders and has gaps
    for d in [
        Dash::Dot,
        Dash::LgDash,
        Dash::DashDot,
        Dash::LgDashDot,
        Dash::LgDashDotDot,
        Dash::SysDash,
        Dash::SysDot,
        Dash::SysDashDot,
        Dash::SysDashDotDot,
        Dash::Custom(vec![(2.0, 2.0), (4.0, 1.0)]),
    ] {
        let mut l = Line::solid(e(4.0), BLUE);
        l.dash = d.clone();
        let img = hline(l);
        let n = (100..700).filter(|&x| is_white(at(&img, x, 200))).count();
        assert!(n > 20, "{d:?}: {n}");
    }
    // a custom dash that sums to zero is solid
    let mut l = Line::solid(e(4.0), BLUE);
    l.dash = Dash::Custom(vec![(0.0, 0.0)]);
    let img = hline(l);
    assert!((100..700).all(|x| !is_white(at(&img, x, 200))));
}

#[test]
fn round_caps_extend_the_line() {
    let mut l = Line::solid(e(20.0), RED);
    l.cap = Cap::Flat;
    let flat = hline(l.clone());
    l.cap = Cap::Round;
    let round = hline(l.clone());
    l.cap = Cap::Square;
    let square = hline(l);
    assert!(is_white(at(&flat, 95, 200)));
    assert!(is_red(at(&round, 95, 200)));
    assert!(is_red(at(&square, 95, 200)));
    // the round cap's corner is cut, the square's is not
    assert!(is_white(at(&round, 91, 191)));
    assert!(is_red(at(&square, 91, 191)));
}

#[test]
fn compound_double_line_has_a_gap() {
    let mut l = Line::solid(e(30.0), RED);
    l.compound = Compound::Dbl;
    let img = hline(l);
    assert!(
        is_white(at(&img, 300, 200)),
        "centre is the gap: {:?}",
        at(&img, 300, 200)
    );
    assert!(is_red(at(&img, 300, 190)));
    assert!(is_red(at(&img, 300, 210)));
    assert!(is_white(at(&img, 300, 180)));
    // triple: centre line, gaps, outer lines
    let mut l = Line::solid(e(50.0), RED);
    l.compound = Compound::Tri;
    let img = hline(l);
    assert!(is_red(at(&img, 300, 200)));
    assert!(is_white(at(&img, 300, 190)));
    assert!(is_red(at(&img, 300, 177)));
    assert!(is_red(at(&img, 300, 223)));
    // thick/thin variants render (approximated as double)
    for c in [Compound::ThickThin, Compound::ThinThick] {
        let mut l = Line::solid(e(30.0), RED);
        l.compound = c;
        let img = hline(l);
        assert!(is_white(at(&img, 300, 200)));
    }
}

#[test]
fn arrow_heads_all_kinds() {
    for kind in [
        ArrowKind::Triangle,
        ArrowKind::Stealth,
        ArrowKind::Diamond,
        ArrowKind::Oval,
        ArrowKind::Arrow,
        ArrowKind::Custom(vec![GeomPath {
            w: 10.0,
            h: 10.0,
            fill_mode: PathFill::Norm,
            stroke: false,
            cmds: vec![
                PathCmd::MoveTo(Pt::new(0.0, 0.0)),
                PathCmd::LineTo(Pt::new(10.0, 5.0)),
                PathCmd::LineTo(Pt::new(0.0, 10.0)),
                PathCmd::Close,
            ],
        }]),
    ] {
        let mut l = Line::solid(e(4.0), RED);
        l.tail = Some(Arrow {
            kind: kind.clone(),
            w: ArrowSize::Lg,
            len: ArrowSize::Lg,
        });
        let img = hline(l);
        // Lg = 5 x 4 px = 20 px wide head just before the end of the line at x = 700
        let above = (680..700).any(|x| !is_white(at(&img, x, 200 - 7)));
        assert!(above, "{kind:?}: head must be wider than the line");
        // nothing behind the start
        assert!(is_white(at(&img, 90, 200)));
    }
}

#[test]
fn arrow_head_at_the_start_and_sizes() {
    let width_of = |size| {
        let mut l = Line::solid(e(4.0), RED);
        l.head = Some(Arrow {
            kind: ArrowKind::Triangle,
            w: size,
            len: size,
        });
        let img = hline(l);
        // the thickest column of the head (the line itself is 4 px)
        (100..140)
            .map(|x| (150..250).filter(|&y| !is_white(at(&img, x, y))).count())
            .max()
            .unwrap()
    };
    let (s, m, l) = (
        width_of(ArrowSize::Sm),
        width_of(ArrowSize::Med),
        width_of(ArrowSize::Lg),
    );
    assert!(s < m && m < l, "{s} {m} {l}");
    // sm = 2x: about 8 px at a 4 px line (min 2.67 px of width applies only below that)
    assert!((l as f64) / (m as f64) > 1.2);
}

#[test]
fn line_does_not_poke_out_beyond_a_thick_arrow_tip() {
    let mut l = Line::solid(e(12.0), RED);
    l.tail = Some(Arrow {
        kind: ArrowKind::Triangle,
        w: ArrowSize::Med,
        len: ArrowSize::Med,
    });
    let img = hline(l);
    // the tip is at x = 700; the line was shortened, so the corners of its end (700, 194) are empty
    assert!(is_white(at(&img, 699, 194)), "{:?}", at(&img, 699, 194));
}

#[test]
fn line_with_gradient_and_miter() {
    let mut l = Line::solid(e(10.0), RED);
    l.fill = Fill::Gradient(Gradient::linear(0.0, vec![(0.0, RED), (1.0, BLUE)]));
    l.join = Join::Miter(4.0);
    let img = hline(l);
    assert!(at(&img, 110, 200)[0] > at(&img, 690, 200)[0]);
    let mut l = Line::solid(e(10.0), RED);
    l.join = Join::Bevel;
    hline(l);
}

// ---- shadow ---------------------------------------------------------------------------------

#[test]
fn outer_shadow_is_offset_and_tinted() {
    let mut s = ShapeItem::new(
        Xfrm::rect(e(100.0), e(100.0), e(100.0), e(100.0)),
        Geometry::Rect,
    );
    s.fill = Fill::Solid(RED);
    s.effects.outer_shadow = Some(Shadow {
        blur_rad: 0.0,
        dist: e(30.0),
        dir_deg: 0.0,
        color: Rgba::new(0, 0, 0, 1.0),
        sx: 1.0,
        sy: 1.0,
        rot_with_shape: false,
    });
    let img = draw(vec![Item::Shape(s)]);
    assert!(is_red(at(&img, 150, 150)));
    // shifted 30 px to the right: black strip right of the shape
    assert!(
        near(at(&img, 215, 150), [0, 0, 0], 20),
        "{:?}",
        at(&img, 215, 150)
    );
    assert!(is_white(at(&img, 150, 250)));
    assert!(is_white(at(&img, 240, 150)));
}

// ---- pictures -------------------------------------------------------------------------------

#[test]
fn picture_pixels_crop_and_clip() {
    let pic = png(8, 8, |x, _| {
        if x < 4 {
            [255, 0, 0, 255]
        } else {
            [0, 0, 255, 255]
        }
    });
    let media = media_of(pic);
    let p = PictureItem::new(Xfrm::rect(e(100.0), e(100.0), e(200.0), e(100.0)), "img");
    let img = raster(&render_svg(&scene(vec![Item::Picture(p.clone())]), &media));
    assert!(is_red(at(&img, 150, 150)));
    assert!(is_blue(at(&img, 250, 150)));
    assert!(is_white(at(&img, 90, 150)));
    // cropping 75 % from the left shows only blue
    let mut c = p.clone();
    c.image.crop = (0.75, 0.0, 0.0, 0.0);
    let img = raster(&render_svg(&scene(vec![Item::Picture(c)]), &media));
    assert!(is_blue(at(&img, 110, 150)) && is_blue(at(&img, 290, 150)));
    // an ellipse clip cuts the corners
    let mut c = p.clone();
    c.geom = Geometry::Ellipse;
    let img = raster(&render_svg(&scene(vec![Item::Picture(c)]), &media));
    assert!(is_red(at(&img, 150, 150)));
    assert!(is_white(at(&img, 103, 103)));
    // an outline
    let mut c = p.clone();
    c.line = Some(Line::solid(e(8.0), GREEN.with_alpha(1.0)));
    let img = raster(&render_svg(&scene(vec![Item::Picture(c)]), &media));
    assert!(
        near(at(&img, 101, 150), [0, 160, 0], 20),
        "{:?}",
        at(&img, 101, 150)
    );
    // alpha
    let mut c = p;
    c.image.alpha = 0.5;
    let img = raster(&render_svg(&scene(vec![Item::Picture(c)]), &media));
    assert!(at(&img, 150, 150)[1] > 100);
}

#[test]
fn picture_crop_with_negative_values_adds_space_and_is_clipped() {
    let pic = png(4, 4, |_, _| [255, 0, 0, 255]);
    let media = media_of(pic);
    let mut p = PictureItem::new(Xfrm::rect(e(100.0), e(100.0), e(200.0), e(100.0)), "img");
    p.image.crop = (-0.5, 0.0, -0.5, 0.0);
    let img = raster(&render_svg(&scene(vec![Item::Picture(p)]), &media));
    // the image occupies the middle half only
    assert!(is_white(at(&img, 110, 150)));
    assert!(is_red(at(&img, 200, 150)));
    assert!(is_white(at(&img, 290, 150)));
    // degenerate crop is survivable
    let mut p = PictureItem::new(Xfrm::rect(e(100.0), e(100.0), e(200.0), e(100.0)), "img");
    p.image.crop = (0.7, 0.0, 0.7, f64::NAN);
    let r = render_svg(&scene(vec![Item::Picture(p)]), &media);
    assert!(!r.svg.contains("NaN") && !r.svg.contains("inf"));
}

#[test]
fn picture_formats() {
    let png_bytes = png(4, 4, |_, _| [255, 0, 0, 255]);
    let mut bmp = Vec::new();
    let mut tif = Vec::new();
    let im =
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(4, 4, image::Rgb([255, 0, 0])));
    im.write_to(&mut std::io::Cursor::new(&mut bmp), image::ImageFormat::Bmp)
        .unwrap();
    im.write_to(
        &mut std::io::Cursor::new(&mut tif),
        image::ImageFormat::Tiff,
    )
    .unwrap();
    let mut jpg = Vec::new();
    im.write_to(
        &mut std::io::Cursor::new(&mut jpg),
        image::ImageFormat::Jpeg,
    )
    .unwrap();
    let mut gif = Vec::new();
    im.write_to(&mut std::io::Cursor::new(&mut gif), image::ImageFormat::Gif)
        .unwrap();
    let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><rect width="10" height="10" fill="#ff0000"/></svg>"##.to_vec();
    for (name, bytes, mime) in [
        ("png", png_bytes, "image/png"),
        ("bmp", bmp, "image/png"),
        ("tiff", tif, "image/png"),
        ("jpeg", jpg, "image/jpeg"),
        ("gif", gif, "image/gif"),
        ("svg", svg, "image/svg+xml"),
    ] {
        let p = PictureItem::new(Xfrm::rect(e(100.0), e(100.0), e(200.0), e(100.0)), "img");
        let r = render_svg(&scene(vec![Item::Picture(p)]), &media_of(bytes));
        assert!(r.svg.contains(&format!("data:{mime};base64,")), "{name}");
        let img = raster(&r);
        let px = at(&img, 200, 150);
        assert!(px[0] > 200 && px[1] < 60 && px[2] < 60, "{name}: {px:?}");
    }
}

#[test]
fn sniffing() {
    use super::svg::{sniff, Sniffed::*};
    assert_eq!(sniff(&png(2, 2, |_, _| [0; 4])), Png);
    assert_eq!(sniff(b"\xFF\xD8\xFF\xE0xxxx"), Jpeg);
    assert_eq!(sniff(b"GIF89a...."), Gif);
    assert_eq!(sniff(b"RIFF\0\0\0\0WEBPVP8 "), Webp);
    assert_eq!(sniff(b"<?xml version='1.0'?><svg/>"), Svg);
    assert_eq!(sniff(b"  <svg xmlns=''/>"), Svg);
    assert_eq!(sniff(b"<html><svg/></html>"), Unknown);
    assert_eq!(sniff(b"\xD7\xCD\xC6\x9Axxxxxxxxxxxxxxxxxxxx"), Wmf);
    let mut emf = vec![0u8; 60];
    emf[0] = 1;
    emf[40..44].copy_from_slice(b" EMF");
    assert_eq!(sniff(&emf), Emf);
    assert_eq!(sniff(b"II*\0...."), Tiff);
    assert_eq!(sniff(b""), Unknown);
    assert_eq!(sniff(b"\0\0\0"), Unknown);
}

#[test]
fn missing_garbage_and_metafile_pictures_get_a_placeholder() {
    let p = PictureItem::new(Xfrm::rect(e(100.0), e(100.0), e(200.0), e(100.0)), "img");
    let mut emf = vec![0u8; 100];
    emf[0] = 1;
    emf[40..44].copy_from_slice(b" EMF");
    type Media = Box<dyn Fn(&str) -> Option<Arc<Vec<u8>>>>;
    let cases: Vec<Media> = vec![
        Box::new(|_| None),
        Box::new(|_| Some(Arc::new(vec![]))),
        Box::new(|_| Some(Arc::new(b"definitely not an image".to_vec()))),
        Box::new(move |_| Some(Arc::new(emf.clone()))),
        // a PNG header followed by garbage embeds (decoding is the rasterizer's problem later)
    ];
    for m in cases {
        let r = render_svg(&scene(vec![Item::Picture(p.clone())]), &*m);
        let img = raster(&r);
        assert!(
            near(at(&img, 120, 130), [230, 230, 230], 4),
            "{:?}",
            at(&img, 120, 130)
        );
        assert!(!r.truncated);
    }
    // an undecodable BMP is a placeholder too
    let mut bad = b"BM".to_vec();
    bad.extend_from_slice(&[0u8; 100]);
    let r = render_svg(&scene(vec![Item::Picture(p)]), &media_of(bad));
    assert!(near(at(&raster(&r), 120, 130), [230, 230, 230], 4));
}

// ---- text (pixels) --------------------------------------------------------------------------

fn text_shape(x: f64, y: f64, w: f64, h: f64, text: &str, size_pt: f64) -> ShapeItem {
    let mut s = ShapeItem::new(Xfrm::rect(e(x), e(y), e(w), e(h)), Geometry::Rect);
    s.text = Some(TextBody {
        paragraphs: vec![Paragraph {
            runs: vec![Run::text(text, size_pt)],
            ..Default::default()
        }],
        ..Default::default()
    });
    s
}

fn dark_bounds(img: &image::RgbaImage) -> Option<(u32, u32, u32, u32)> {
    let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0, 0);
    for y in 0..img.height() {
        for x in 0..img.width() {
            if !near(at(img, x, y), WHITE, 60) {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x);
                y1 = y1.max(y);
            }
        }
    }
    (x1 >= x0 && y1 >= y0).then_some((x0, y0, x1, y1))
}

#[test]
fn text_pixels_appear_inside_the_box() {
    let s = text_shape(100.0, 100.0, 400.0, 100.0, "Hello Slide", 24.0);
    let img = draw(vec![Item::Shape(s)]);
    let (x0, y0, x1, y1) = dark_bounds(&img).expect("text must produce pixels");
    // default insets: 0.1in = 9.6 px left, 0.05in = 4.8 px top
    assert!(x0 >= 100 + 8 && x1 <= 500, "{x0}..{x1}");
    assert!(y0 >= 100 && y1 <= 200, "{y0}..{y1}");
    assert!(x1 - x0 > 60);
}

#[test]
fn text_insets_and_anchor_move_the_text() {
    let bounds = |t: &dyn Fn(&mut TextBody)| {
        let mut s = text_shape(100.0, 100.0, 400.0, 200.0, "Hello", 20.0);
        t(s.text.as_mut().unwrap());
        dark_bounds(&draw(vec![Item::Shape(s)])).unwrap()
    };
    let base = bounds(&|_| {});
    let inset = bounds(&|t| t.insets = (e(60.0), e(40.0), e(0.0), e(0.0)));
    assert!(inset.0 >= base.0 + 40, "{inset:?} {base:?}");
    assert!(inset.1 >= base.1 + 30);
    let mid = bounds(&|t| t.anchor = Anchor::Middle);
    let bot = bounds(&|t| t.anchor = Anchor::Bottom);
    assert!(mid.1 > base.1 + 60 && mid.3 < 300);
    assert!(bot.1 > mid.1 + 40 && bot.3 <= 300);
    let ctr = bounds(&|t| t.paragraphs[0].align = Align::Center);
    let cx = (ctr.0 + ctr.2) as f64 / 2.0;
    assert!((cx - 300.0).abs() < 15.0, "{cx}");
    let right = bounds(&|t| t.paragraphs[0].align = Align::Right);
    assert!(right.2 >= 480 && right.2 <= 500, "{right:?}");
}

#[test]
fn rotated_shape_rotates_its_text() {
    let mut s = text_shape(380.0, 250.0, 200.0, 40.0, "ROTATED", 20.0);
    s.xfrm.rot_deg = 90.0;
    let (x0, y0, x1, y1) = dark_bounds(&draw(vec![Item::Shape(s)])).unwrap();
    // 200 x 40 box rotated: the text runs vertically
    assert!(y1 - y0 > 2 * (x1 - x0), "{x0},{y0},{x1},{y1}");
    // upright keeps it horizontal
    let mut s = text_shape(380.0, 250.0, 200.0, 40.0, "ROTATED", 20.0);
    s.xfrm.rot_deg = 90.0;
    s.text.as_mut().unwrap().upright = true;
    let (x0, y0, x1, y1) = dark_bounds(&draw(vec![Item::Shape(s)])).unwrap();
    assert!(x1 - x0 > y1 - y0, "{x0},{y0},{x1},{y1}");
}

#[test]
fn flipped_shapes_do_not_mirror_their_text() {
    // Mirrored text would put the heavy vertical stroke of the "F" on the other side; compare
    // against the unflipped rendering of the same text in the same place: identical bounds.
    let base = text_shape(100.0, 100.0, 300.0, 100.0, "FFFF", 30.0);
    let b0 = draw(vec![Item::Shape(base.clone())]);
    let mut f = base;
    f.xfrm.flip_h = true;
    let b1 = draw(vec![Item::Shape(f)]);
    // the flipped shape's text rectangle is mirrored in place (the box is symmetric), the text
    // itself reads the same way: the left half of both pictures carries the same ink
    assert_eq!(dark_bounds(&b0).unwrap(), dark_bounds(&b1).unwrap());
    let ink = |img: &image::RgbaImage| {
        let mut n = 0;
        for y in 100..200 {
            for x in 100..400 {
                if !is_white(at(img, x, y)) {
                    n += 1;
                }
            }
        }
        n
    };
    assert_eq!(ink(&b0), ink(&b1));
    // flipV turns the text upside down: the ink bounds stay in the box but differ vertically
    let mut v = text_shape(100.0, 100.0, 300.0, 100.0, "FFFF", 30.0);
    v.xfrm.flip_v = true;
    let b2 = draw(vec![Item::Shape(v)]);
    let (_, y0, _, y1) = dark_bounds(&b2).unwrap();
    assert!(y0 >= 100 && y1 <= 200);
}

#[test]
fn vertical_text_modes_render() {
    for vert in [Vert::Vert, Vert::Vert270, Vert::EaVert] {
        let mut s = text_shape(100.0, 100.0, 80.0, 300.0, "縦書きテスト ABC", 20.0);
        s.text.as_mut().unwrap().vert = vert;
        let b =
            dark_bounds(&draw(vec![Item::Shape(s)])).unwrap_or_else(|| panic!("{vert:?}: no ink"));
        assert!(
            b.0 >= 95 && b.2 <= 185 && b.1 >= 95 && b.3 <= 405,
            "{vert:?} {b:?}"
        );
        if vert != Vert::EaVert {
            assert!(
                b.3 - b.1 > b.2 - b.0,
                "{vert:?} {b:?}: rotated lines run vertically"
            );
        }
    }
}

#[test]
fn japanese_text_renders_ink() {
    let s = text_shape(100.0, 100.0, 400.0, 100.0, "日本語のテキスト表示です", 24.0);
    let b = dark_bounds(&draw(vec![Item::Shape(s)])).unwrap();
    assert!(b.2 - b.0 > 150, "{b:?}");
}

#[test]
fn text_decorations_highlight_and_bullets_render() {
    let mut s = text_shape(100.0, 100.0, 400.0, 200.0, "decorated", 24.0);
    {
        let t = s.text.as_mut().unwrap();
        let r = &mut t.paragraphs[0].runs[0];
        r.underline = Underline::Double;
        r.strike = Strike::Single;
        r.highlight = Some(Rgba::rgb(255, 255, 0));
        t.paragraphs[0].bullet = Some(Bullet {
            kind: BulletKind::Char("•".into()),
            font: None,
            color: Some(RED),
            size: BulletSize::Pct(1.0),
        });
        t.paragraphs[0].mar_l = e(30.0);
        t.paragraphs[0].indent = -e(30.0);
    }
    let img = draw(vec![Item::Shape(s)]);
    // yellow highlight pixels exist
    let yellow = (100..500).any(|x| (100..160).any(|y| near(at(&img, x, y), [255, 255, 0], 10)));
    assert!(yellow);
    // the bullet is red
    let red = (100..140).any(|x| (100..160).any(|y| is_red(at(&img, x, y))));
    assert!(red);
}

#[test]
fn picture_bullets_draw_the_picture() {
    let pic = png(4, 4, |_, _| [0, 0, 255, 255]);
    let mut s = text_shape(100.0, 100.0, 400.0, 100.0, "item", 24.0);
    {
        let p = &mut s.text.as_mut().unwrap().paragraphs[0];
        p.bullet = Some(Bullet {
            kind: BulletKind::Picture(ImageFill::stretch("b")),
            font: None,
            color: None,
            size: BulletSize::FollowText,
        });
        p.mar_l = e(40.0);
        p.indent = -e(40.0);
    }
    let img = raster(&render_svg(&scene(vec![Item::Shape(s)]), &media_of(pic)));
    assert!((100..145).any(|x| (100..160).any(|y| is_blue(at(&img, x, y)))));
}

// ---- escaping -------------------------------------------------------------------------------

#[test]
fn hostile_strings_are_escaped() {
    let nasty = "a<b>&\"'c ]]> d\u{0}\u{1}\u{7}e\u{8}\u{b}\u{c}\u{1f}\u{7f}</text><script>alert(1)</script>";
    let mut s = text_shape(100.0, 100.0, 400.0, 100.0, nasty, 20.0);
    {
        let r = &mut s.text.as_mut().unwrap().paragraphs[0].runs[0];
        r.font = FontSpec {
            latin: Some("Evil\" onload=\"x' /><script/>".into()),
            east_asian: Some("<>&".into()),
            complex: None,
            symbol: None,
        };
    }
    let mut pic = PictureItem::new(Xfrm::rect(0.0, 0.0, e(10.0), e(10.0)), "k\"/><script>");
    pic.image.key = "k\"/><script>".into();
    let fill = Fill::Pattern {
        preset: "\"><script>".into(),
        fg: RED,
        bg: BLUE,
    };
    let mut t = ShapeItem::new(
        Xfrm::rect(e(50.0), e(300.0), e(60.0), e(60.0)),
        Geometry::Rect,
    );
    t.fill = fill;
    let r = render_svg(
        &scene(vec![Item::Shape(s), Item::Picture(pic), Item::Shape(t)]),
        &no_media,
    );
    assert!(!r.svg.contains("<script"), "{}", r.svg);
    assert!(!r.svg.contains("]]>"));
    assert!(!r.svg.contains("onload=\""), "an attribute was injected");
    assert!(r.svg.contains("&lt;b&gt;&amp;"));
    assert!(r.svg.contains("]]&gt;"));
    for c in 0..0x20u8 {
        if c != b'\n' && c != b'\t' && c != b'\r' {
            assert!(!r.svg.contains(c as char), "control {c} leaked");
        }
    }
    assert!(!r.svg.contains('\u{7f}'));
    // and the document still parses and draws
    raster(&r);
}

#[test]
fn esc_and_num_unit() {
    use super::svg::{esc, num};
    assert_eq!(esc("a&b<c>\"d'"), "a&amp;b&lt;c&gt;&quot;d&apos;");
    assert_eq!(esc("tab\tnl\nx"), "tab nl x");
    assert_eq!(esc("日本語"), "日本語");
    assert_eq!(esc("\u{fffe}\u{ffff}x"), "x");
    assert_eq!(num(1.0), "1");
    assert_eq!(num(1.23456), "1.235");
    assert_eq!(num(-0.0001), "0");
    assert_eq!(num(f64::NAN), "0");
    assert_eq!(num(f64::INFINITY), "0");
    assert_eq!(num(1e300), "10000000");
    assert_eq!(num(-1e300), "-10000000");
    assert_eq!(num(0.5), "0.5");
}

// ---- hostile numbers and budgets ------------------------------------------------------------

#[test]
fn nan_and_infinity_never_reach_the_output() {
    let bad = [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, 1e300, -1e300];
    for &b in &bad {
        let mut items = Vec::new();
        let mut s = ShapeItem::new(
            Xfrm {
                x: b,
                y: b,
                w: b,
                h: b,
                rot_deg: b,
                flip_h: true,
                flip_v: false,
            },
            Geometry::Paths(vec![GeomPath {
                w: b,
                h: b,
                fill_mode: PathFill::Norm,
                stroke: true,
                cmds: vec![
                    PathCmd::MoveTo(Pt::new(b, b)),
                    PathCmd::LineTo(Pt::new(b, 0.0)),
                    PathCmd::CubicTo(Pt::new(b, b), Pt::new(b, b), Pt::new(b, b)),
                    PathCmd::QuadTo(Pt::new(b, b), Pt::new(b, b)),
                    PathCmd::ArcTo {
                        wr: b,
                        hr: b,
                        st_deg: b,
                        sw_deg: b,
                    },
                    PathCmd::Close,
                ],
            }]),
        );
        s.fill = Fill::Gradient(Gradient {
            kind: GradKind::Linear {
                angle_deg: b,
                scaled: false,
            },
            stops: vec![(b, Rgba::new(1, 2, 3, b)), (0.5, Rgba::new(3, 2, 1, b))],
            fill_to_rect: (b, b, b, b),
            rot_with_shape: true,
        });
        let mut l = Line::solid(b, Rgba::new(0, 0, 0, b));
        l.dash = Dash::Custom(vec![(b, b), (1.0, 1.0)]);
        l.join = Join::Miter(b);
        l.head = Some(Arrow {
            kind: ArrowKind::Triangle,
            w: ArrowSize::Med,
            len: ArrowSize::Med,
        });
        l.tail = l.head.clone();
        s.line = Some(l);
        s.effects.outer_shadow = Some(Shadow {
            blur_rad: b,
            dist: b,
            dir_deg: b,
            color: Rgba::new(0, 0, 0, b),
            sx: b,
            sy: b,
            rot_with_shape: false,
        });
        s.text_rect = Some((b, b, b, b));
        s.text = Some(TextBody {
            insets: (b, b, b, b),
            rot_deg: b,
            paragraphs: vec![Paragraph {
                mar_l: b,
                indent: b,
                spc_before: Spacing::Pts(b),
                spc_after: Spacing::Pct(b),
                line_spacing: Spacing::Pct(b),
                end_size_pt: b,
                runs: vec![Run {
                    size_pt: b,
                    spacing_pt: b,
                    baseline_pct: b,
                    text: "text".into(),
                    ..Run::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        });
        items.push(Item::Shape(s));
        items.push(Item::Picture(PictureItem::new(
            Xfrm {
                x: b,
                y: b,
                w: b,
                h: b,
                ..Default::default()
            },
            "k",
        )));
        items.push(group(
            Xfrm {
                x: b,
                y: b,
                w: b,
                h: b,
                rot_deg: b,
                flip_h: false,
                flip_v: true,
            },
            (b, b),
            (b, b),
            vec![rect_item(b, b, b, b, RED)],
        ));
        let mut sc = scene(items);
        sc.background = Fill::Solid(Rgba::new(0, 0, 0, b));
        let r = render_svg(&sc, &media_of(png(2, 2, |_, _| [1, 2, 3, 255])));
        let lower = r.svg.to_ascii_lowercase();
        // base64 data may contain such letters; look outside the data URIs
        let outside: String = lower
            .split("data:")
            .enumerate()
            .map(|(i, part)| {
                if i == 0 {
                    part.to_string()
                } else {
                    part.split_once('"')
                        .map(|x| x.1.to_string())
                        .unwrap_or_default()
                }
            })
            .collect();
        assert!(
            !outside.contains("nan") && !outside.contains("inf"),
            "{b}: NaN/inf in output"
        );
        // no exponent notation in numbers: a digit followed by e and a sign or digit
        let bytes = outside.as_bytes();
        for i in 1..bytes.len().saturating_sub(1) {
            if bytes[i] == b'e'
                && bytes[i - 1].is_ascii_digit()
                && (bytes[i + 1] == b'+' || bytes[i + 1] == b'-' || bytes[i + 1].is_ascii_digit())
            {
                let ctx = &outside[i.saturating_sub(15)..(i + 10).min(outside.len())];
                // colours and ids contain such sequences ("#1e2f3a", "gr12e3"); numbers do not
                assert!(
                    ctx.contains('#') || ctx.contains("url(") || ctx.contains("id="),
                    "{b}: exponent in {ctx:?}"
                );
            }
        }
    }
}

#[test]
fn many_items_stop_at_the_budget() {
    // gradient fills write a definition each: about 450 bytes an item
    let items: Vec<Item> = (0..60_000)
        .map(|i| {
            let mut s = ShapeItem::new(
                Xfrm::rect(e((i % 900) as f64), e((i % 500) as f64), e(5.0), e(5.0)),
                Geometry::Rect,
            );
            s.fill = Fill::Gradient(Gradient::linear(0.0, vec![(0.0, RED), (1.0, BLUE)]));
            Item::Shape(s)
        })
        .collect();
    let r = render_svg(&scene(items), &no_media);
    assert!(r.truncated, "{} bytes", r.svg.len());
    assert!(
        r.svg.len() < super::svg::MAX_SVG_TEXT_BYTES + 1_000_000,
        "{}",
        r.svg.len()
    );
    assert!(r.svg.ends_with("</svg>"));
}

#[test]
fn huge_text_is_truncated_not_fatal() {
    let big = "word ".repeat(80_000);
    let s = text_shape(0.0, 0.0, 900.0, 500.0, &big, 8.0);
    let r = render_svg(&scene(vec![Item::Shape(s)]), &no_media);
    assert!(r.truncated);
    assert!(r.svg.len() < super::svg::MAX_SVG_TEXT_BYTES + 1_000_000);
    assert!(r.svg.ends_with("</svg>"));
}

#[test]
fn embedded_image_budget() {
    // three "PNG"s of 22 MiB: two fit under 64 MiB, the third does not
    let mut bytes = png(2, 2, |_, _| [9, 9, 9, 255]);
    bytes.resize(22 * 1024 * 1024, 0);
    let media = media_of(bytes);
    let items: Vec<Item> = (0..3)
        .map(|i| {
            Item::Picture(PictureItem::new(
                Xfrm::rect(e(10.0 + 100.0 * i as f64), e(10.0), e(50.0), e(50.0)),
                "k",
            ))
        })
        .collect();
    let r = render_svg(&scene(items), &media);
    assert!(r.truncated);
    assert_eq!(r.svg.matches("data:image/png").count(), 2);
    // the third became a placeholder
    assert!(r.svg.contains("#e6e6e6"));
    // image data does not count against the markup budget
    assert!(r.svg.len() > super::svg::MAX_SVG_TEXT_BYTES);
}

#[test]
fn path_command_limit_marks_truncation() {
    let g = Geometry::Paths(vec![GeomPath {
        cmds: vec![PathCmd::LineTo(Pt::new(1.0, 1.0)); super::path::MAX_PATH_CMDS + 5],
        ..Default::default()
    }]);
    let mut s = ShapeItem::new(Xfrm::rect(0.0, 0.0, e(10.0), e(10.0)), g);
    s.fill = Fill::Solid(RED);
    assert!(render_svg(&scene(vec![Item::Shape(s)]), &no_media).truncated);
}

#[test]
fn empty_and_odd_scenes_do_not_panic() {
    let mut s = ShapeItem::new(Xfrm::default(), Geometry::Paths(vec![]));
    s.text = Some(TextBody::default());
    s.fill = Fill::Pattern {
        preset: String::new(),
        fg: RED,
        bg: BLUE,
    };
    s.line = Some(Line {
        fill: Fill::Image(ImageFill::stretch("x")),
        ..Line::solid(1.0, RED)
    });
    let items = vec![
        Item::Shape(s),
        group(Xfrm::default(), (0.0, 0.0), (0.0, 0.0), vec![]),
        Item::Picture(PictureItem::new(Xfrm::default(), "")),
    ];
    let r = render_svg(&scene(items), &no_media);
    raster(&r);
}

// ---- dump for the eye -----------------------------------------------------------------------

fn styled(text: &str, size: f64, f: impl Fn(&mut Run)) -> Run {
    let mut r = Run::text(text, size);
    f(&mut r);
    r
}

fn para(runs: Vec<Run>) -> Paragraph {
    Paragraph {
        runs,
        ..Default::default()
    }
}

fn dump_scenes() -> Vec<(&'static str, SlideScene, Vec<u8>)> {
    let photo = png(64, 48, |x, y| [(x * 4) as u8, (y * 5) as u8, 150, 255]);
    let mut out = Vec::new();
    // 1: text styles
    let mut s = ShapeItem::new(
        Xfrm::rect(e(40.0), e(30.0), e(880.0), e(480.0)),
        Geometry::Rect,
    );
    s.fill = Fill::Solid(Rgba::rgb(245, 247, 250));
    s.line = Some(Line::solid(e(2.0), Rgba::rgb(60, 90, 160)));
    s.text = Some(TextBody {
        insets: (e(24.0), e(16.0), e(24.0), e(16.0)),
        paragraphs: vec![
            Paragraph {
                align: Align::Center,
                runs: vec![styled("Quarterly Review", 40.0, |r| {
                    r.bold = true;
                    r.fill = Fill::Solid(Rgba::rgb(30, 60, 130));
                })],
                ..Default::default()
            },
            para(vec![
                Run::text("Normal, ", 20.0),
                styled("bold ", 20.0, |r| r.bold = true),
                styled("italic ", 20.0, |r| r.italic = true),
                styled("underline ", 20.0, |r| r.underline = Underline::Single),
                styled("strike ", 20.0, |r| r.strike = Strike::Single),
                styled("red ", 20.0, |r| r.fill = Fill::Solid(RED)),
                styled("highlight", 20.0, |r| r.highlight = Some(Rgba::rgb(255, 235, 59))),
                Run::text(" and E=mc", 20.0),
                styled("2", 20.0, |r| r.baseline_pct = 30.0),
                Run::text(" H", 20.0),
                styled("2", 20.0, |r| r.baseline_pct = -25.0),
                Run::text("O.", 20.0),
            ]),
            Paragraph {
                bullet: Some(Bullet {
                    kind: BulletKind::Char("•".into()),
                    font: None,
                    color: Some(Rgba::rgb(200, 60, 40)),
                    size: BulletSize::Pct(1.0),
                }),
                mar_l: e(32.0),
                indent: -e(32.0),
                runs: vec![Run::text(
                    "A bullet whose text is long enough that it must wrap around to a second line under the hanging indent.",
                    20.0,
                )],
                ..Default::default()
            },
            Paragraph {
                bullet: Some(Bullet {
                    kind: BulletKind::AutoNum {
                        scheme: "arabicPeriod".into(),
                        start: 1,
                    },
                    font: None,
                    color: None,
                    size: BulletSize::FollowText,
                }),
                mar_l: e(32.0),
                indent: -e(32.0),
                runs: vec![Run::text("First numbered item", 20.0)],
                ..Default::default()
            },
            Paragraph {
                bullet: Some(Bullet {
                    kind: BulletKind::AutoNum {
                        scheme: "arabicPeriod".into(),
                        start: 1,
                    },
                    font: None,
                    color: None,
                    size: BulletSize::FollowText,
                }),
                mar_l: e(32.0),
                indent: -e(32.0),
                runs: vec![Run::text("Second numbered item", 20.0)],
                ..Default::default()
            },
            Paragraph {
                align: Align::Justify,
                runs: vec![Run::text(
                    "Justified paragraph: the words of every line but the last are spread so that both edges are straight, as in a newspaper column of text.",
                    18.0,
                )],
                spc_before: Spacing::Pts(10.0),
                ..Default::default()
            },
            Paragraph {
                align: Align::Right,
                runs: vec![Run::text("Right aligned, 150% line spacing", 18.0)],
                line_spacing: Spacing::Pct(1.5),
                ..Default::default()
            },
        ],
        ..Default::default()
    });
    out.push(("text-styles", scene(vec![Item::Shape(s)]), vec![]));

    // 2: Japanese
    let mut a = ShapeItem::new(
        Xfrm::rect(e(40.0), e(30.0), e(560.0), e(300.0)),
        Geometry::Rect,
    );
    a.fill = Fill::Solid(Rgba::WHITE);
    a.line = Some(Line::solid(e(1.0), Rgba::rgb(120, 120, 120)));
    let jp = |t: &str, sz: f64| Run {
        font: FontSpec {
            latin: Some("Calibri".into()),
            east_asian: Some("游ゴシック".into()),
            ..Default::default()
        },
        ..Run::text(t, sz)
    };
    a.text = Some(TextBody {
        paragraphs: vec![
            para(vec![jp("日本語の見出し", 32.0)]),
            para(vec![jp(
                "これは禁則処理の確認です。行頭に句読点や閉じ括弧が来ない（はず）ですし、「かぎ括弧」も行末に残りません。English words mix in too, wrapping at spaces.",
                20.0,
            )]),
            para(vec![Run {
                font: FontSpec {
                    east_asian: Some("游明朝".into()),
                    ..Default::default()
                },
                ..Run::text("明朝体の段落です。吾輩は猫である。名前はまだ無い。", 20.0)
            }]),
        ],
        ..Default::default()
    });
    let mut b = ShapeItem::new(
        Xfrm::rect(e(640.0), e(30.0), e(100.0), e(400.0)),
        Geometry::Rect,
    );
    b.line = Some(Line::solid(e(1.0), Rgba::rgb(120, 120, 120)));
    b.text = Some(TextBody {
        vert: Vert::EaVert,
        paragraphs: vec![para(vec![jp("縦書きのテキスト、ABC を含む。", 22.0)])],
        ..Default::default()
    });
    let mut c = ShapeItem::new(
        Xfrm::rect(e(780.0), e(30.0), e(120.0), e(400.0)),
        Geometry::Rect,
    );
    c.line = Some(Line::solid(e(1.0), Rgba::rgb(120, 120, 120)));
    c.text = Some(TextBody {
        vert: Vert::Vert,
        paragraphs: vec![para(vec![Run::text("Rotated text (vert)", 22.0)])],
        ..Default::default()
    });
    out.push((
        "japanese",
        scene(vec![Item::Shape(a), Item::Shape(b), Item::Shape(c)]),
        vec![],
    ));

    // 3: shapes, lines, arrows
    let mut items = Vec::new();
    let mut r = ShapeItem::new(
        Xfrm::rect(e(40.0), e(40.0), e(200.0), e(120.0)),
        Geometry::Rect,
    );
    r.fill = Fill::Solid(Rgba::rgb(66, 133, 244));
    r.line = Some(Line::solid(e(3.0), Rgba::rgb(20, 60, 140)));
    r.effects.outer_shadow = Some(Shadow {
        blur_rad: e(8.0),
        dist: e(8.0),
        dir_deg: 45.0,
        color: Rgba::new(0, 0, 0, 0.4),
        sx: 1.0,
        sy: 1.0,
        rot_with_shape: false,
    });
    items.push(Item::Shape(r));
    let mut el = ShapeItem::new(
        Xfrm::rect(e(280.0), e(40.0), e(200.0), e(120.0)),
        Geometry::Ellipse,
    );
    el.fill = Fill::Gradient(Gradient {
        kind: GradKind::Radial,
        stops: vec![
            (0.0, Rgba::rgb(255, 235, 150)),
            (1.0, Rgba::rgb(240, 150, 30)),
        ],
        fill_to_rect: (0.5, 0.5, 0.5, 0.5),
        rot_with_shape: true,
    });
    items.push(Item::Shape(el));
    let mut rot = ShapeItem::new(
        Xfrm {
            rot_deg: 20.0,
            ..Xfrm::rect(e(540.0), e(40.0), e(160.0), e(100.0))
        },
        Geometry::Rect,
    );
    rot.fill = Fill::Pattern {
        preset: "dnDiag".into(),
        fg: Rgba::rgb(40, 140, 80),
        bg: Rgba::WHITE,
    };
    rot.line = Some(Line::solid(e(2.0), Rgba::rgb(40, 140, 80)));
    rot.text = Some(TextBody {
        anchor: Anchor::Middle,
        paragraphs: vec![Paragraph {
            align: Align::Center,
            runs: vec![styled("Rotated 20", 20.0, |r| r.bold = true)],
            ..Default::default()
        }],
        ..Default::default()
    });
    items.push(Item::Shape(rot));
    let heart = Geometry::Paths(vec![GeomPath {
        w: 100.0,
        h: 100.0,
        fill_mode: PathFill::Norm,
        stroke: true,
        cmds: vec![
            PathCmd::MoveTo(Pt::new(50.0, 30.0)),
            PathCmd::CubicTo(Pt::new(50.0, 0.0), Pt::new(0.0, 0.0), Pt::new(0.0, 35.0)),
            PathCmd::CubicTo(
                Pt::new(0.0, 60.0),
                Pt::new(40.0, 85.0),
                Pt::new(50.0, 100.0),
            ),
            PathCmd::CubicTo(
                Pt::new(60.0, 85.0),
                Pt::new(100.0, 60.0),
                Pt::new(100.0, 35.0),
            ),
            PathCmd::CubicTo(Pt::new(100.0, 0.0), Pt::new(50.0, 0.0), Pt::new(50.0, 30.0)),
            PathCmd::Close,
        ],
    }]);
    let mut h = ShapeItem::new(Xfrm::rect(e(740.0), e(30.0), e(160.0), e(150.0)), heart);
    h.fill = Fill::Solid(Rgba::rgb(220, 50, 70));
    h.line = Some(Line::solid(e(2.0), Rgba::rgb(120, 20, 40)));
    items.push(Item::Shape(h));
    let pie = Geometry::Paths(vec![GeomPath {
        w: 100.0,
        h: 100.0,
        fill_mode: PathFill::Norm,
        stroke: true,
        cmds: vec![
            PathCmd::MoveTo(Pt::new(50.0, 50.0)),
            PathCmd::LineTo(Pt::new(100.0, 50.0)),
            PathCmd::ArcTo {
                wr: 50.0,
                hr: 50.0,
                st_deg: 0.0,
                sw_deg: 270.0,
            },
            PathCmd::Close,
        ],
    }]);
    let mut p = ShapeItem::new(Xfrm::rect(e(40.0), e(200.0), e(140.0), e(140.0)), pie);
    p.fill = Fill::Gradient(Gradient::linear(
        60.0,
        vec![
            (0.0, Rgba::rgb(120, 200, 255)),
            (1.0, Rgba::rgb(30, 60, 160)),
        ],
    ));
    p.line = Some(Line::solid(e(2.0), Rgba::rgb(20, 40, 100)));
    items.push(Item::Shape(p));
    for (i, (dash, cmp)) in [
        (Dash::Solid, Compound::Sng),
        (Dash::Dash, Compound::Sng),
        (Dash::Dot, Compound::Sng),
        (Dash::DashDot, Compound::Sng),
        (Dash::Solid, Compound::Dbl),
        (Dash::Solid, Compound::Tri),
    ]
    .into_iter()
    .enumerate()
    {
        let mut l = ShapeItem::new(
            Xfrm::rect(e(230.0), e(215.0 + 24.0 * i as f64), e(300.0), 0.0),
            Geometry::Line,
        );
        let mut ln = Line::solid(
            e(if cmp == Compound::Sng { 3.0 } else { 9.0 }),
            Rgba::rgb(60, 60, 60),
        );
        ln.dash = dash;
        ln.compound = cmp;
        ln.cap = Cap::Round;
        ln.tail = Some(Arrow {
            kind: ArrowKind::Triangle,
            w: ArrowSize::Med,
            len: ArrowSize::Med,
        });
        if i == 1 {
            ln.head = Some(Arrow {
                kind: ArrowKind::Oval,
                w: ArrowSize::Med,
                len: ArrowSize::Med,
            });
        }
        l.line = Some(ln);
        items.push(Item::Shape(l));
    }
    for (i, kind) in [
        ArrowKind::Stealth,
        ArrowKind::Diamond,
        ArrowKind::Arrow,
        ArrowKind::Oval,
    ]
    .into_iter()
    .enumerate()
    {
        let mut l = ShapeItem::new(
            Xfrm::rect(e(580.0), e(220.0 + 28.0 * i as f64), e(300.0), 0.0),
            Geometry::Line,
        );
        let mut ln = Line::solid(e(3.0), Rgba::rgb(160, 40, 40));
        ln.tail = Some(Arrow {
            kind,
            w: ArrowSize::Lg,
            len: ArrowSize::Lg,
        });
        l.line = Some(ln);
        items.push(Item::Shape(l));
    }
    // 4: pictures, groups
    out.push(("shapes-lines", scene(items), vec![]));

    let mut items = Vec::new();
    let mut pic = PictureItem::new(Xfrm::rect(e(40.0), e(40.0), e(240.0), e(180.0)), "photo");
    pic.line = Some(Line::solid(e(4.0), Rgba::rgb(40, 40, 40)));
    items.push(Item::Picture(pic.clone()));
    let mut cropped = pic.clone();
    cropped.xfrm = Xfrm::rect(e(320.0), e(40.0), e(240.0), e(180.0));
    cropped.image.crop = (0.25, 0.25, 0.25, 0.25);
    items.push(Item::Picture(cropped));
    let mut round = pic.clone();
    round.xfrm = Xfrm {
        rot_deg: -10.0,
        ..Xfrm::rect(e(620.0), e(40.0), e(240.0), e(180.0))
    };
    round.geom = Geometry::Ellipse;
    items.push(Item::Picture(round));
    let child = |x, c: Rgba| {
        let mut s = ShapeItem::new(Xfrm::rect(e(x), e(0.0), e(60.0), e(40.0)), Geometry::Rect);
        s.fill = Fill::Solid(c);
        s.text = Some(TextBody {
            anchor: Anchor::Middle,
            paragraphs: vec![Paragraph {
                align: Align::Center,
                runs: vec![Run::text("ab", 14.0)],
                ..Default::default()
            }],
            ..Default::default()
        });
        Item::Shape(s)
    };
    let inner = group(
        Xfrm {
            flip_h: true,
            ..Xfrm::rect(e(0.0), e(0.0), e(200.0), e(40.0))
        },
        (0.0, 0.0),
        (e(200.0), e(40.0)),
        vec![
            child(0.0, Rgba::rgb(230, 80, 80)),
            child(70.0, Rgba::rgb(80, 180, 90)),
            child(140.0, Rgba::rgb(80, 120, 230)),
        ],
    );
    for (i, (rot, fh, fv)) in [
        (0.0, false, false),
        (0.0, true, false),
        (30.0, false, false),
        (30.0, true, true),
    ]
    .into_iter()
    .enumerate()
    {
        let g = group(
            Xfrm {
                rot_deg: rot,
                flip_h: fh,
                flip_v: fv,
                ..Xfrm::rect(e(60.0 + 220.0 * i as f64), e(330.0), e(200.0), e(40.0))
            },
            (0.0, 0.0),
            (e(200.0), e(40.0)),
            vec![inner.clone()],
        );
        items.push(g);
    }
    out.push(("pictures-groups", scene(items), photo));
    out
}

#[test]
#[ignore = "writes docs/render-check/slide-draw/*.png for a human to look at"]
fn slide_draw_dump_render_check() {
    let dir = Path::new("/Users/shuhei/work/konoma/docs/render-check/slide-draw");
    let _ = std::fs::create_dir_all(dir);
    if let Ok(rd) = std::fs::read_dir(dir) {
        for f in rd.flatten() {
            let _ = std::fs::remove_file(f.path());
        }
    }
    for (name, sc, photo) in dump_scenes() {
        let media = if photo.is_empty() {
            media_of(vec![])
        } else {
            media_of(photo)
        };
        let r = render_svg(&sc, &media);
        let img =
            crate::preview::svg::rasterize_trusted(r.svg.as_bytes(), Path::new("/slide.svg"), 1280)
                .expect("rasterizes");
        img.save(dir.join(format!("{name}.png"))).unwrap();
    }
}
