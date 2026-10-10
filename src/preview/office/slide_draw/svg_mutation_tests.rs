//! Tests from mutation testing of the SVG writer (`svg.rs`, `svg_effects.rs`).

use std::cell::Cell;

use super::super::model::*;
use super::super::svg::{esc, sniff, visible_len, Sniffed};
use super::super::tests::{e, no_media, scene, text_shape};
use super::super::{render_svg, render_svg_cancellable};

#[test]
fn sniff_emf_needs_both_the_record_type_and_the_signature() {
    // A long buffer that is not an EMF header must not be taken for one (`len > 44 || ...`).
    assert_eq!(sniff(&[0u8; 100]), Sniffed::Unknown);
    assert_eq!(sniff(&[7u8; 100]), Sniffed::Unknown);
    // Type 1 but no " EMF" signature.
    let mut b = vec![0u8; 100];
    b[0] = 1;
    assert_eq!(sniff(&b), Sniffed::Unknown);
    // Signature but wrong record type.
    let mut b = vec![0u8; 100];
    b[40..44].copy_from_slice(b" EMF");
    assert_eq!(sniff(&b), Sniffed::Unknown);
    // Both: an EMF, at any length above the header offset.
    let mut b = vec![0u8; 100];
    b[0] = 1;
    b[40..44].copy_from_slice(b" EMF");
    assert_eq!(sniff(&b), Sniffed::Emf);
    // A short buffer starting like an EMF is not one and must not panic.
    assert_eq!(sniff(&[1, 0, 0, 0, 0, 0]), Sniffed::Unknown);
}

#[test]
fn sniff_wmf_and_bmp_lengths() {
    // Non-placeable WMF header (type 1/2, header size 9 words).
    for t in [1u8, 2] {
        let mut b = vec![0u8; 40];
        b[0] = t;
        b[2] = 9;
        assert_eq!(sniff(&b), Sniffed::Wmf, "type {t}");
    }
    // Same prefix on a short buffer is not enough.
    assert_eq!(sniff(&[1, 0, 9, 0, 0, 0]), Sniffed::Unknown);
    // Wrong prefix on a long one.
    assert_eq!(sniff(&[3u8; 40]), Sniffed::Unknown);
    // BM needs a header's worth of bytes.
    let mut bmp = vec![0u8; 60];
    bmp[0] = b'B';
    bmp[1] = b'M';
    assert_eq!(sniff(&bmp), Sniffed::Bmp);
    assert_eq!(sniff(b"BM"), Sniffed::Unknown);
    assert_eq!(sniff(b"BMxx"), Sniffed::Unknown);
}

#[test]
fn sniff_svg_needs_an_svg_element_and_a_known_prologue() {
    assert_eq!(sniff(b"<?xml version='1.0'?><html/>"), Sniffed::Unknown);
    assert_eq!(sniff(b"<!-- c --><svg/>"), Sniffed::Svg);
    assert_eq!(sniff(b"\xEF\xBB\xBF<svg/>"), Sniffed::Svg);
    assert_eq!(sniff(b"<!DOCTYPE svg><svg/>"), Sniffed::Svg);
    assert_eq!(sniff(b"hello <svg/>"), Sniffed::Unknown);
}

#[test]
fn sniff_length_floors_are_exact() {
    // BMP: a 14-byte file header alone has no image after it.
    let mut b = vec![0u8; 14];
    b[..2].copy_from_slice(b"BM");
    assert_eq!(sniff(&b), Sniffed::Unknown);
    b.push(0);
    assert_eq!(sniff(&b), Sniffed::Bmp);
    // EMF: the signature ends at byte 44; a buffer that ends there has nothing after the header.
    let mut b = vec![0u8; 44];
    b[0] = 1;
    b[40..44].copy_from_slice(b" EMF");
    assert_eq!(sniff(&b), Sniffed::Unknown);
    b.push(0);
    assert_eq!(sniff(&b), Sniffed::Emf);
    // WMF without the placeable key: an 18-byte header alone has no records.
    let mut b = vec![0u8; 18];
    b[0] = 1;
    b[2] = 9;
    assert_eq!(sniff(&b), Sniffed::Unknown);
    b.push(0);
    assert_eq!(sniff(&b), Sniffed::Wmf);
}

#[test]
fn esc_keeps_space_and_drops_every_other_control_character() {
    assert_eq!(esc(" "), " ");
    assert_eq!(esc("a b"), "a b");
    for c in 0u32..0x20 {
        let s = char::from_u32(c).unwrap().to_string();
        let want = if matches!(c, 9 | 10 | 13) { " " } else { "" };
        assert_eq!(esc(&s), want, "U+{c:04X}");
    }
    assert_eq!(esc("\u{7F}\u{80}\u{9F}\u{FFFE}\u{FFFF}"), "");
    assert_eq!(esc("\u{A0}\u{FFFD}"), "\u{A0}\u{FFFD}");
}

#[test]
fn visible_len_uses_the_windows_own_origin() {
    // a window that does not start at the origin
    let w = Some((10.0, 20.0, 100.0, 50.0));
    let at = |x: f64, y: f64| Pt::new(x, y);
    // horizontal, across the window's width: x from 10 to 110
    assert!((visible_len(at(-100.0, 30.0), at(300.0, 30.0), w, 1e9) - 100.0).abs() < 1e-9);
    // vertical, across the window's height: y from 20 to 70
    assert!((visible_len(at(50.0, -100.0), at(50.0, 200.0), w, 1e9) - 50.0).abs() < 1e-9);
    // wholly left of / above the window although it would be inside a window at the origin
    assert_eq!(visible_len(at(-5.0, 30.0), at(5.0, 30.0), w, 1e9), 0.0);
    assert_eq!(visible_len(at(50.0, 5.0), at(50.0, 15.0), w, 1e9), 0.0);
}

#[test]
fn a_slide_larger_than_the_limit_is_clamped_to_it() {
    // 100 000 px is the most a side can have; 1 px the least
    let mut s = scene(vec![]);
    s.width = e(300_000.0);
    s.height = e(540.0);
    let r = render_svg(&s, &no_media);
    assert!(
        r.svg.contains(r#"viewBox="0 0 100000 540""#),
        "{}",
        &r.svg[..200]
    );
    let mut s = scene(vec![]);
    s.width = e(0.25);
    s.height = e(-5.0);
    let r = render_svg(&s, &no_media);
    assert!(r.svg.contains(r#"viewBox="0 0 1 1""#), "{}", &r.svg[..200]);
    // a side that is not a number is the width of a 13.333 inch (16:9) slide
    let mut s = scene(vec![]);
    s.width = f64::NAN;
    s.height = f64::INFINITY;
    let r = render_svg(&s, &no_media);
    assert!(
        r.svg.contains(r#"viewBox="0 0 1279.968 1279.968""#),
        "{}",
        &r.svg[..200]
    );
}

/// The number of segments in the longest `d` attribute of `svg`.
fn longest_path_segs(svg: &str) -> usize {
    svg.split(r#" d=""#)
        .skip(1)
        .map(|p| {
            let d = p.split('"').next().unwrap_or("");
            d.split_whitespace()
                .filter(|t| t.starts_with(['M', 'L', 'Q', 'C']))
                .count()
        })
        .max()
        .unwrap_or(0)
}

/// A stroked path of 2 000 arcs, each of four cubics: 8 001 segments from fewer commands than the
/// resolver looks at the cancel callback for, so only the writer's own look can cut it.
fn arc_heavy_path() -> SlideScene {
    let mut cmds = vec![PathCmd::MoveTo(Pt::new(e(100.0), e(100.0)))];
    for _ in 0..2000 {
        cmds.push(PathCmd::ArcTo {
            wr: e(20.0),
            hr: e(20.0),
            st_deg: 0.0,
            sw_deg: 270.0,
        });
    }
    let path = GeomPath {
        w: 0.0,
        h: 0.0,
        fill_mode: PathFill::None,
        stroke: true,
        cmds,
    };
    let mut s = ShapeItem::new(
        Xfrm::rect(0.0, 0.0, e(960.0), e(540.0)),
        Geometry::Paths(vec![path]),
    );
    s.line = Some(Line::solid(e(1.0), Rgba::BLACK));
    scene(vec![Item::Shape(s)])
}

#[test]
fn the_path_writer_looks_at_the_cancel_callback_every_4096th_segment() {
    let sc = arc_heavy_path();
    let calls = Cell::new(0usize);
    let full = render_svg_cancellable(&sc, &no_media, &|| {
        calls.set(calls.get() + 1);
        false
    });
    assert!(!full.cancelled);
    assert_eq!(longest_path_segs(&full.svg), 8001);
    let total = calls.get();
    assert!(total >= 4, "{total}");
    // Say yes exactly once, at each call in turn: one of the calls is the writer's own look, before
    // segment 4095 (counting from 0), and it ends the path after the 4095 segments before it.
    let mut counts = Vec::new();
    for k in 1..=total {
        let calls = Cell::new(0usize);
        let r = render_svg_cancellable(&sc, &no_media, &|| {
            calls.set(calls.get() + 1);
            calls.get() == k
        });
        counts.push(longest_path_segs(&r.svg));
    }
    assert!(
        counts.contains(&4095),
        "no call of {total} cut the path at 4095: {counts:?}"
    );
    // and no look cuts it anywhere else (the writer looks only at every 4096th segment)
    assert!(
        counts.iter().all(|&n| n == 4095 || n == 8001 || n <= 8),
        "{counts:?}"
    );
}

#[test]
fn a_path_whose_points_differ_by_the_tolerance_is_still_axis_aligned() {
    // |dx| == |dy| == 1e-6 EMU is "not more than the tolerance" on both axes: a straight line.
    let thin = |dx: f64, dy: f64| {
        let path = GeomPath {
            w: 0.0,
            h: 0.0,
            fill_mode: PathFill::None,
            stroke: true,
            cmds: vec![
                PathCmd::MoveTo(Pt::new(0.0, 0.0)),
                PathCmd::LineTo(Pt::new(dx, dy)),
            ],
        };
        let mut s = ShapeItem::new(
            Xfrm::rect(0.0, 0.0, e(200.0), e(100.0)),
            Geometry::Paths(vec![path]),
        );
        s.line = Some(Line::solid(e(1.0), Rgba::BLACK));
        render_svg(&scene(vec![Item::Shape(s)]), &no_media)
    };
    assert!(thin(1e-6, 1e-6).svg.contains("crispEdges"));
    assert!(thin(1e-6, 0.0).svg.contains("crispEdges"));
    assert!(thin(0.0, e(100.0)).svg.contains("crispEdges"), "vertical");
    assert!(
        !thin(e(100.0), e(3.0)).svg.contains("crispEdges"),
        "slanted"
    );
    assert!(!thin(2e-6, 2e-6).svg.contains("crispEdges"));
    // the tolerance holds on each axis alone
    assert!(
        thin(1e-6, e(100.0)).svg.contains("crispEdges"),
        "vertical within tolerance"
    );
    assert!(
        thin(e(100.0), 1e-6).svg.contains("crispEdges"),
        "horizontal within tolerance"
    );
    assert!(!thin(2e-6, e(100.0)).svg.contains("crispEdges"));
    assert!(!thin(e(100.0), 2e-6).svg.contains("crispEdges"));
}

#[test]
fn a_vertical_line_is_crisp_too() {
    let mut s = ShapeItem::new(
        Xfrm::rect(e(100.0), e(100.0), 0.0, e(200.0)),
        Geometry::Line,
    );
    s.line = Some(Line::solid(e(1.0), Rgba::BLACK));
    let r = render_svg(&scene(vec![Item::Shape(s)]), &no_media);
    assert!(r.svg.contains("crispEdges"), "{}", r.svg);
}

#[test]
fn a_group_counts_as_one_element_in_the_predicted_work() {
    let empty = render_svg(&scene(vec![]), &no_media).features.els;
    let g = Item::Group(GroupItem {
        xfrm: Xfrm::rect(0.0, 0.0, e(10.0), e(10.0)),
        child_off: (0.0, 0.0),
        child_ext: (e(10.0), e(10.0)),
        items: vec![],
    });
    let one = render_svg(&scene(vec![g.clone()]), &no_media).features.els;
    assert_eq!(one - empty, 1.0);
    let three = render_svg(&scene(vec![g.clone(), g.clone(), g]), &no_media)
        .features
        .els;
    assert_eq!(three - empty, 3.0);
}

#[test]
fn the_features_count_what_the_shapes_wrote() {
    use super::super::cost::scan;
    let curvy = {
        let path = GeomPath {
            w: 0.0,
            h: 0.0,
            fill_mode: PathFill::Norm,
            stroke: true,
            cmds: vec![
                PathCmd::MoveTo(Pt::new(0.0, 0.0)),
                PathCmd::LineTo(Pt::new(e(50.0), 0.0)),
                PathCmd::QuadTo(Pt::new(e(80.0), e(10.0)), Pt::new(e(50.0), e(40.0))),
                PathCmd::CubicTo(
                    Pt::new(e(30.0), e(60.0)),
                    Pt::new(e(10.0), e(60.0)),
                    Pt::new(0.0, e(40.0)),
                ),
                PathCmd::Close,
            ],
        };
        let mut s = ShapeItem::new(
            Xfrm::rect(e(300.0), e(100.0), e(100.0), e(100.0)),
            Geometry::Paths(vec![path]),
        );
        s.fill = Fill::Solid(Rgba::rgb(0, 128, 0));
        s.line = Some(Line::solid(e(2.0), Rgba::BLACK));
        Item::Shape(s)
    };
    let items = vec![
        Item::Shape(text_shape(10.0, 10.0, 200.0, 40.0, "hello", 12.0)),
        curvy,
        Item::Shape(text_shape(10.0, 200.0, 300.0, 40.0, "twelve chars", 12.0)),
    ];
    let empty = render_svg(&scene(vec![]), &no_media);
    let r = render_svg(&scene(items), &no_media);
    let (a, b) = (scan(&r.svg), scan(&empty.svg));
    let f = r.features;
    // the path segments are counted from the markup written (the text writes none)
    assert_eq!(f.segs - empty.features.segs, a.segs - b.segs);
    assert_eq!(
        f.segs, 10.0,
        "M L Q C Z, once for the fill and once for the outline"
    );
    // elements: the two paths of the curvy shape (fill and outline) from the markup, and for each
    // text body one element for the body and five per run, as the text estimates itself
    assert_eq!(f.els, 2.0 + 2.0 * (1.0 + 5.0));
    // and the absolute numbers, from the text itself: two runs of 5 and 12 characters
    assert_eq!(f.glyphs, 17.0);
    assert_eq!(f.glyphs_sq, 25.0 + 144.0);
    assert_eq!(f.spans, 2.0);
}

fn svg_pic(body: &str) -> Vec<u8> {
    format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="50" viewBox="0 0 100 50">{body}</svg>"#
    )
    .into_bytes()
}

#[test]
fn an_svg_picture_is_charged_by_its_area_and_outline_scaled_to_its_box() {
    use super::super::hardening_tests::{by_key, pic};
    // 960 px wide slide, model raster 1280 px: 4/3 device px per slide px.
    let k = 1280.0 / 960.0;
    // 100 x 50 units placed in a 300 x 150 px box: 3 px per unit on both axes.
    let media = by_key(vec![
        ("rect", svg_pic(r#"<rect width="100" height="50"/>"#)),
        (
            "line",
            svg_pic(
                r##"<rect width="100" height="50"/><path d="M0 0L100 0" stroke="#000" stroke-width="2"/>"##,
            ),
        ),
    ]);
    let draw = |key: &str, w: f64, h: f64| {
        render_svg(&scene(vec![pic(key, 100.0, 100.0, w, h)]), &*media).features
    };
    let a = draw("rect", 300.0, 150.0);
    // the rectangle paints 100 x 50 units^2, each unit 3 px * 4/3 device px on a side
    let want_area = 5000.0 * (3.0 * k) * (3.0 * k);
    assert!(
        (a.vec_px2 - want_area).abs() < 1e-6 * want_area,
        "{} vs {want_area}",
        a.vec_px2
    );
    // different scales on the two axes: 6 px per unit across, 1.5 down
    let b = draw("rect", 600.0, 75.0);
    let want_area = 5000.0 * (6.0 * k) * (1.5 * k);
    assert!(
        (b.vec_px2 - want_area).abs() < 1e-6 * want_area,
        "{} vs {want_area}",
        b.vec_px2
    );
    // the outline of the added line is 100 units long (2 * the longer side at most), drawn at the
    // mean of the two scales
    let c = draw("line", 600.0, 75.0);
    let line_len = c.edge_px - b.edge_px;
    let want = 100.0 * (6.0 * k + 1.5 * k) / 2.0;
    assert!(
        (line_len - want).abs() < 1e-6 * want,
        "{line_len} vs {want}"
    );
    // one more element and two more segments than the plain rectangle
    assert_eq!(c.els - b.els, 1.0 + 0.0 * c.els);
    assert_eq!(c.segs - b.segs, 2.0);
    // the same picture twice costs its area twice
    let twice = render_svg(
        &scene(vec![
            pic("rect", 100.0, 100.0, 300.0, 150.0),
            pic("rect", 100.0, 300.0, 300.0, 150.0),
        ]),
        &*media,
    )
    .features;
    assert!((twice.vec_px2 - 2.0 * a.vec_px2).abs() < 1e-6 * a.vec_px2);
}

#[test]
fn an_outline_counts_inside_the_slide_and_a_twentieth_around_it_and_not_beyond() {
    // 960 x 540 px slide: the margin is 5 % of the longer side, 48 px, on every side.
    let k = 1280.0 / 960.0;
    let line = |x: f64, y: f64, w: f64, h: f64| {
        let mut s = ShapeItem::new(Xfrm::rect(e(x), e(y), e(w), e(h)), Geometry::Line);
        s.line = Some(Line::solid(e(1.0), Rgba::BLACK));
        render_svg(&scene(vec![Item::Shape(s)]), &no_media)
            .features
            .edge_px
    };
    // a vertical line in the right margin counts in full, one beyond it not at all
    let inside = line(990.0, 0.0, 0.0, 540.0);
    assert!((inside - 540.0 * k).abs() < 1e-6, "{inside}");
    assert_eq!(line(1030.0, 0.0, 0.0, 540.0), 0.0);
    // a horizontal one in the bottom margin, and beyond it
    let inside = line(0.0, 560.0, 960.0, 0.0);
    assert!((inside - 960.0 * k).abs() < 1e-6, "{inside}");
    assert_eq!(line(0.0, 600.0, 960.0, 0.0), 0.0);
    // and in the top and left margins
    assert!((line(-40.0, 0.0, 0.0, 540.0) - 540.0 * k).abs() < 1e-6);
    assert!((line(0.0, -40.0, 960.0, 0.0) - 960.0 * k).abs() < 1e-6);
    assert_eq!(line(-60.0, 0.0, 0.0, 540.0), 0.0);
}

#[test]
fn a_fill_is_charged_by_its_area_on_the_model_raster_and_never_more_than_the_canvas() {
    let k2 = (1280.0f64 / 960.0).powi(2);
    // (the white background of the slide is a fill of its own, charged with the first shape: it is
    // taken off, measured with a shape that paints nothing)
    let nothing = ShapeItem::new(Xfrm::rect(0.0, 0.0, e(1.0), e(1.0)), Geometry::Rect);
    let base = render_svg(&scene(vec![Item::Shape(nothing)]), &no_media).features;
    assert!((base.fill_px2 - 960.0 * 540.0 * k2).abs() < 1e-6);
    let filled = |w: f64, h: f64, fill: Fill| {
        let mut s = ShapeItem::new(Xfrm::rect(0.0, 0.0, e(w), e(h)), Geometry::Rect);
        s.fill = fill;
        let mut f = render_svg(&scene(vec![Item::Shape(s)]), &no_media).features;
        f.fill_px2 -= base.fill_px2;
        f
    };
    let solid = Fill::Solid(Rgba::rgb(10, 20, 30));
    let f = filled(100.0, 50.0, solid.clone());
    assert!((f.fill_px2 - 5000.0 * k2).abs() < 1e-6, "{}", f.fill_px2);
    assert_eq!(f.grad_px2, 0.0);
    // a box of any size is at most the canvas
    let f = filled(50_000.0, 50_000.0, solid);
    assert!(
        (f.fill_px2 - 960.0 * 540.0 * k2).abs() < 1e-6,
        "{}",
        f.fill_px2
    );
    // a transparent solid paints nothing
    let f = filled(100.0, 50.0, Fill::Solid(Rgba::new(10, 20, 30, 0.0)));
    assert_eq!(f.fill_px2, 0.0);
    // gradients of two and more stops are gradient area, a single stop is not
    let red = Rgba::rgb(255, 0, 0);
    let blue = Rgba::rgb(0, 0, 255);
    let two = Fill::Gradient(Gradient::linear(0.0, vec![(0.0, red), (1.0, blue)]));
    let three = Fill::Gradient(Gradient::linear(
        0.0,
        vec![(0.0, red), (0.5, blue), (1.0, red)],
    ));
    let one = Fill::Gradient(Gradient::linear(0.0, vec![(0.5, red)]));
    for (name, g, want) in [
        ("two", two, 5000.0),
        ("three", three, 5000.0),
        ("one", one, 0.0),
    ] {
        let f = filled(100.0, 50.0, g);
        assert!(
            (f.grad_px2 - want * k2).abs() < 1e-6,
            "{name}: {}",
            f.grad_px2
        );
        assert_eq!(f.fill_px2, 0.0, "{name}");
    }
    // a pattern is gradient area too
    let f = filled(
        100.0,
        50.0,
        Fill::Pattern {
            preset: "ltHorz".to_string(),
            fg: red,
            bg: blue,
        },
    );
    assert!((f.grad_px2 - 5000.0 * k2).abs() < 1e-6, "{}", f.grad_px2);
}

#[test]
fn a_group_keeps_the_axes_of_its_children_through_quarter_turns_and_flips() {
    let thin_line = || {
        let mut s = ShapeItem::new(Xfrm::rect(e(10.0), e(10.0), e(100.0), 0.0), Geometry::Line);
        s.line = Some(Line::solid(e(1.0), Rgba::BLACK));
        Item::Shape(s)
    };
    let group = |rot: f64, flip_h: bool, items: Vec<Item>| {
        let mut x = Xfrm::rect(0.0, 0.0, e(200.0), e(200.0));
        x.rot_deg = rot;
        x.flip_h = flip_h;
        Item::Group(GroupItem {
            xfrm: x,
            child_off: (0.0, 0.0),
            child_ext: (e(200.0), e(200.0)),
            items,
        })
    };
    let crisp = |item: Item| {
        render_svg(&scene(vec![item]), &no_media)
            .svg
            .contains("crispEdges")
    };
    // a quarter turn of the group keeps the child's line on the axes; one degree off does not
    assert!(crisp(group(90.0, false, vec![thin_line()])));
    assert!(!crisp(group(91.0, false, vec![thin_line()])));
    assert!(!crisp(group(1.0, false, vec![thin_line()])));
    // inside a mirrored group the turn counts the other way round, still a quarter turn
    let inner = group(90.0, false, vec![thin_line()]);
    assert!(crisp(group(0.0, true, vec![inner])));
    let inner = group(89.0, false, vec![thin_line()]);
    assert!(!crisp(group(0.0, true, vec![inner])));
    // two turns that add up to a quarter turn
    let inner = group(60.0, false, vec![thin_line()]);
    assert!(crisp(group(30.0, false, vec![inner])));
}

#[test]
fn a_group_whose_child_extent_is_exactly_the_epsilon_falls_back_to_its_own_size() {
    // child extents of 1e-9 are "no extent": the children are not scaled (1e-9 would blow them up)
    let mut inner = ShapeItem::new(
        Xfrm::rect(e(10.0), e(10.0), e(10.0), e(10.0)),
        Geometry::Rect,
    );
    inner.fill = Fill::Solid(Rgba::rgb(255, 0, 0));
    let g = |w: f64, h: f64| {
        Item::Group(GroupItem {
            xfrm: Xfrm::rect(e(100.0), e(100.0), e(200.0), e(100.0)),
            child_off: (0.0, 0.0),
            child_ext: (w, h),
            items: vec![Item::Shape(inner.clone())],
        })
    };
    let plain = render_svg(&scene(vec![g(0.0, 0.0)]), &no_media).svg;
    let eps = render_svg(&scene(vec![g(1e-9, 1e-9)]), &no_media).svg;
    assert_eq!(plain, eps);
    let eps_h = render_svg(&scene(vec![g(e(200.0), 1e-9)]), &no_media).svg;
    let plain_h = render_svg(&scene(vec![g(e(200.0), 0.0)]), &no_media).svg;
    assert_eq!(plain_h, eps_h);
}

fn morph_svg(side: u32, radius: u32) -> Vec<u8> {
    format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{side}" height="{side}"><filter id="f"><feMorphology operator="dilate" radius="{radius}"/></filter><rect width="{side}" height="{side}" filter="url(#f)"/></svg>"#
    )
    .into_bytes()
}

#[test]
fn a_morphology_filter_in_a_picture_is_charged_as_the_process_charges_it() {
    use super::super::hardening_tests::{by_key, guard_work, pic};
    let media = by_key(vec![
        ("m1", morph_svg(400, 1)),
        ("m5", morph_svg(400, 3)),
        ("m20", morph_svg(400, 8)),
    ]);
    let mut works = Vec::new();
    for key in ["m1", "m5", "m20"] {
        let r = render_svg(&scene(vec![pic(key, 0.0, 0.0, 100.0, 100.0)]), &*media);
        assert!(r.filter_work > 0.0, "{key}");
        let guard = guard_work(&r);
        assert!(
            (r.filter_work - guard).abs() <= 0.02 * guard,
            "{key}: model {:.4e} against the process's {guard:.4e}",
            r.filter_work
        );
        works.push(r.filter_work);
    }
    // a bigger radius is more work
    assert!(works[0] < works[1] && works[1] < works[2], "{works:?}");
}

fn gradient_shape(w: f64, h: f64, g: Gradient) -> SlideScene {
    let mut s = ShapeItem::new(Xfrm::rect(e(100.0), e(100.0), e(w), e(h)), Geometry::Rect);
    s.fill = Fill::Gradient(g);
    scene(vec![Item::Shape(s)])
}

fn pixel(img: &image::RgbaImage, x: f64, y: f64) -> [u8; 3] {
    super::super::tests::at(img, x as u32, y as u32)
}

fn is_near(p: [u8; 3], want: [u8; 3], tol: i32) -> bool {
    (0..3).all(|i| (p[i] as i32 - want[i] as i32).abs() <= tol)
}

#[test]
fn an_ellipsoid_gradient_on_a_wide_box_is_constant_along_the_long_axis_inside_the_segment() {
    // 400 x 100 box: radius 70.7, the segment reaches (400 - 100) * sqrt(2) / 2 = 212 px either
    // side of the centre -- longer than the box, so the whole middle line is the first colour.
    let white = Rgba::rgb(255, 255, 255);
    let red = Rgba::rgb(255, 0, 0);
    let mut g = Gradient::linear(0.0, vec![(0.0, white), (1.0, red)]);
    g.kind = GradKind::Ellipsoid { angle_deg: 0.0 };
    let img = super::super::tests::raster(&render_svg(&gradient_shape(400.0, 100.0, g), &no_media));
    // the middle line (y = 150) at the centre and off to the sides is the first colour
    for x in [200.0, 250.0, 300.0, 350.0, 400.0] {
        assert!(
            is_near(pixel(&img, x, 150.0), [255, 255, 255], 12),
            "x {x}: {:?}",
            pixel(&img, x, 150.0)
        );
    }
    // 35 px above the line (half the radius) is half way to the last colour, at any x inside
    for x in [250.0, 350.0] {
        let p = pixel(&img, x, 115.0);
        assert!(p[0] > 240 && p[1] > 100 && p[1] < 190, "x {x}: {p:?}");
    }
    // the same box, tall: the long axis is vertical
    let mut g = Gradient::linear(0.0, vec![(0.0, white), (1.0, red)]);
    g.kind = GradKind::Ellipsoid { angle_deg: 0.0 };
    let img = super::super::tests::raster(&render_svg(&gradient_shape(100.0, 400.0, g), &no_media));
    for y in [250.0, 300.0, 400.0] {
        assert!(is_near(pixel(&img, 150.0, y), [255, 255, 255], 12), "y {y}");
    }
    let p = pixel(&img, 115.0, 300.0);
    assert!(p[0] > 240 && p[1] > 100 && p[1] < 190, "{p:?}");
}

#[test]
fn an_ellipsoid_gradient_follows_its_focus_rectangle() {
    // focus on the left edge, middle: the centre of the circle moves there
    let white = Rgba::rgb(255, 255, 255);
    let red = Rgba::rgb(255, 0, 0);
    let mut g = Gradient::linear(0.0, vec![(0.0, white), (1.0, red)]);
    g.kind = GradKind::Ellipsoid { angle_deg: 0.0 };
    g.fill_to_rect = (0.0, 0.5, 1.0, 0.5);
    let img = super::super::tests::raster(&render_svg(&gradient_shape(200.0, 200.0, g), &no_media));
    // square 200 x 200: radius 141 px around (100, 200) -- left edge, middle
    assert!(
        is_near(pixel(&img, 102.0, 200.0), [255, 255, 255], 20),
        "{:?}",
        pixel(&img, 102.0, 200.0)
    );
    // half the radius away, to the right and straight up
    for (x, y) in [(170.0, 200.0), (100.0 + 1.0, 130.0)] {
        let p = pixel(&img, x, y);
        assert!(p[0] > 240 && p[1] > 90 && p[1] < 200, "{x},{y}: {p:?}");
    }
    // the far right edge is nearly the last colour
    let p = pixel(&img, 298.0, 200.0);
    assert!(p[0] > 240 && p[1] < 40, "{p:?}");
}

#[test]
fn a_rect_gradient_has_no_wedge_on_a_side_the_focus_rectangle_touches() {
    let red = Rgba::rgb(255, 0, 0);
    let blue = Rgba::rgb(0, 0, 255);
    // focus rectangle from the middle to the right edge
    let mut g = Gradient::linear(0.0, vec![(0.0, red), (1.0, blue)]);
    g.kind = GradKind::Rect;
    g.fill_to_rect = (0.5, 0.5, 0.0, 0.5);
    let svg = render_svg(&gradient_shape(200.0, 200.0, g), &no_media).svg;
    // wedges: l, t, b exist; r would have a vector of no length
    assert!(svg.contains("linearGradient id=\"gr1l\""), "{svg}");
    assert!(svg.contains("linearGradient id=\"gr1t\""));
    assert!(svg.contains("linearGradient id=\"gr1b\""));
    assert!(!svg.contains("linearGradient id=\"gr1r\""), "{svg}");
    // the same for every side: a focus rectangle that reaches an edge leaves that wedge out
    for (rect, side) in [
        ((0.0, 0.5, 0.5, 0.5), 'l'),
        ((0.5, 0.0, 0.5, 0.5), 't'),
        ((0.5, 0.5, 0.0, 0.5), 'r'),
        ((0.5, 0.5, 0.5, 0.0), 'b'),
    ] {
        let mut g = Gradient::linear(0.0, vec![(0.0, red), (1.0, blue)]);
        g.kind = GradKind::Rect;
        g.fill_to_rect = rect;
        let svg = render_svg(&gradient_shape(200.0, 200.0, g), &no_media).svg;
        for other in ['l', 't', 'r', 'b'] {
            let has = svg.contains(&format!("linearGradient id=\"gr1{other}\""));
            assert_eq!(has, other != side, "focus {rect:?}, wedge {other}");
        }
    }
}

#[test]
fn a_rect_gradient_paints_the_focus_rectangle_in_the_first_colour() {
    let red = Rgba::rgb(255, 0, 0);
    let blue = Rgba::rgb(0, 0, 255);
    let mut g = Gradient::linear(0.0, vec![(0.0, red), (1.0, blue)]);
    g.kind = GradKind::Rect;
    g.fill_to_rect = (0.25, 0.25, 0.25, 0.25);
    let img = super::super::tests::raster(&render_svg(&gradient_shape(400.0, 400.0, g), &no_media));
    // inside the focus rectangle (100..300 of the box), also away from its centre
    for (x, y) in [
        (200.0, 200.0),
        (230.0, 300.0),
        (370.0, 250.0),
        (250.0, 220.0),
    ] {
        assert!(
            is_near(pixel(&img, x, y), [255, 0, 0], 12),
            "{x},{y}: {:?}",
            pixel(&img, x, y)
        );
    }
    // and the edge of the box is the last colour
    assert!(
        is_near(pixel(&img, 102.0, 300.0), [0, 0, 255], 20),
        "{:?}",
        pixel(&img, 102.0, 300.0)
    );
    // a focus rectangle with no height has no rectangle to paint (and the wedges meet in a line)
    let mut g = Gradient::linear(0.0, vec![(0.0, red), (1.0, blue)]);
    g.kind = GradKind::Rect;
    g.fill_to_rect = (0.25, 0.5, 0.25, 0.5);
    let svg = render_svg(&gradient_shape(400.0, 400.0, g), &no_media).svg;
    assert!(!svg.contains("fill-opacity=\"1\"/></pattern>"), "{svg}");
}

#[test]
fn a_turned_rect_gradient_is_drawn_in_the_bounding_box_of_the_turned_box() {
    let red = Rgba::rgb(255, 0, 0);
    let blue = Rgba::rgb(0, 0, 255);
    for angle in [0.0f64, 30.0, 45.0, 90.0] {
        let mut g = Gradient::linear(0.0, vec![(0.0, red), (1.0, blue)]);
        g.kind = GradKind::RectRotated { angle_deg: angle };
        let svg = render_svg(&gradient_shape(200.0, 100.0, g), &no_media).svg;
        let (c, s) = (
            angle.to_radians().cos().abs(),
            angle.to_radians().sin().abs(),
        );
        let (gw, gh) = (200.0 * c + 100.0 * s, 200.0 * s + 100.0 * c);
        let want = format!(
            "translate({} {}) scale({} {})",
            super::super::svg::num((200.0 - gw) / 2.0),
            super::super::svg::num((100.0 - gh) / 2.0),
            super::super::svg::num(gw),
            super::super::svg::num(gh),
        );
        assert!(svg.contains(&want), "{angle}: wanted {want} in {svg}");
    }
}

#[test]
fn a_pattern_fill_writes_the_runs_of_its_foreground_pixels() {
    use super::super::patterns::pattern_pixels;
    let fg = Rgba::rgb(255, 0, 0);
    let bg = Rgba::rgb(0, 0, 255);
    for preset in [
        "pct5", "pct50", "horz", "ltVert", "dnDiag", "smCheck", "dotGrid", "cross", "trellis",
    ] {
        let rows = pattern_pixels(preset);
        // the same runs, found a different way: one move per maximal run of true in a row
        let mut want = String::new();
        for (y, row) in rows.iter().enumerate() {
            let mut x = 0;
            while x < 8 {
                if !row[x] {
                    x += 1;
                    continue;
                }
                let end = (x..8).find(|&i| !row[i]).unwrap_or(8);
                want.push_str(&format!("M{x} {y}h{}v1h-{}z", end - x, end - x));
                x = end;
            }
        }
        let mut s = ShapeItem::new(Xfrm::rect(0.0, 0.0, e(64.0), e(64.0)), Geometry::Rect);
        s.fill = Fill::Pattern {
            preset: preset.to_string(),
            fg,
            bg,
        };
        let svg = render_svg(&scene(vec![Item::Shape(s)]), &no_media).svg;
        let found = svg
            .split(r#"<pattern id="pt"#)
            .nth(1)
            .and_then(|p| p.split(r#"<path d=""#).nth(1))
            .and_then(|p| p.split('"').next())
            .unwrap_or("");
        assert_eq!(found, want, "{preset}");
    }
}

fn two_halves_png(w: u32, h: u32) -> Vec<u8> {
    super::super::tests::png(w, h, |x, _| {
        if x < w / 2 {
            [255, 0, 0, 255]
        } else {
            [0, 0, 255, 255]
        }
    })
}

fn image_shape(fill: ImageFill, w: f64, h: f64) -> SlideScene {
    let mut s = ShapeItem::new(Xfrm::rect(e(100.0), e(100.0), e(w), e(h)), Geometry::Rect);
    s.fill = Fill::Image(fill);
    scene(vec![Item::Shape(s)])
}

#[test]
fn a_pixelated_fill_is_drawn_without_smoothing_from_twice_its_pixels_taking_crop_and_inset_into_account(
) {
    use super::super::hardening_tests::by_key;
    let media = by_key(vec![
        ("w", two_halves_png(100, 20)),
        ("t", two_halves_png(20, 100)),
    ]);
    let nearest = |fill: ImageFill, w: f64, h: f64| {
        render_svg(&image_shape(fill, w, h), &*media)
            .svg
            .contains("optimizeSpeed")
    };
    let base = |key: &str| ImageFill {
        pixelated: true,
        ..ImageFill::stretch(key)
    };
    // width: 100 px image in a 150 px box = 1.5 times, in 200 px = 2 times
    assert!(!nearest(base("w"), 150.0, 20.0));
    assert!(nearest(base("w"), 200.0, 20.0));
    // cropping half of it away doubles the enlargement
    let mut f = base("w");
    f.crop = (0.5, 0.0, 0.0, 0.0);
    assert!(nearest(f, 150.0, 20.0));
    let mut f = base("w");
    f.crop = (0.0, 0.0, 0.5, 0.0);
    assert!(nearest(f, 150.0, 20.0));
    // an inset (fill rect) of half the width halves it
    let mut f = base("w");
    f.mode = ImageMode::Stretch {
        fill_rect: (0.5, 0.0, 0.0, 0.0),
    };
    assert!(!nearest(f, 300.0, 20.0));
    let mut f = base("w");
    f.mode = ImageMode::Stretch {
        fill_rect: (0.0, 0.0, 0.5, 0.0),
    };
    assert!(!nearest(f, 300.0, 20.0));
    let mut f = base("w");
    f.mode = ImageMode::Stretch {
        fill_rect: (0.5, 0.0, 0.0, 0.0),
    };
    assert!(nearest(f, 400.0, 20.0));
    // the same for the height of a tall image
    assert!(!nearest(base("t"), 20.0, 150.0));
    assert!(nearest(base("t"), 20.0, 200.0));
    let mut f = base("t");
    f.crop = (0.0, 0.5, 0.0, 0.0);
    assert!(nearest(f, 20.0, 150.0));
    let mut f = base("t");
    f.crop = (0.0, 0.0, 0.0, 0.5);
    assert!(nearest(f, 20.0, 150.0));
    let mut f = base("t");
    f.mode = ImageMode::Stretch {
        fill_rect: (0.0, 0.5, 0.0, 0.0),
    };
    assert!(!nearest(f, 20.0, 300.0));
    let mut f = base("t");
    f.mode = ImageMode::Stretch {
        fill_rect: (0.0, 0.0, 0.0, 0.5),
    };
    assert!(!nearest(f, 20.0, 300.0));
    let mut f = base("t");
    f.mode = ImageMode::Stretch {
        fill_rect: (0.0, 0.5, 0.0, 0.0),
    };
    assert!(nearest(f, 20.0, 400.0));
    // a shrunk picture is never "enlarged"
    assert!(!nearest(base("w"), 50.0, 10.0));
}

#[test]
fn a_tiled_fill_is_placed_by_its_alignment_scale_and_offset_and_counts_its_tiles() {
    use super::super::hardening_tests::by_key;
    let media = by_key(vec![("p", two_halves_png(40, 20))]);
    let tile = |sx: f64, sy: f64, tx: f64, ty: f64, align: RectAlign, flip: TileFlip| {
        let f = ImageFill {
            mode: ImageMode::Tile {
                sx,
                sy,
                tx,
                ty,
                align,
                flip,
            },
            ..ImageFill::stretch("p")
        };
        render_svg(&image_shape(f, 400.0, 200.0), &*media)
    };
    // the pattern's own x / y / width / height
    let geom = |svg: &str| -> Vec<f64> {
        let p = svg.split(r#"<pattern id="pi"#).nth(1).unwrap();
        let attrs = p.split('>').next().unwrap();
        ["x", "y", "width", "height"]
            .iter()
            .map(|k| {
                let a = format!(r#" {k}=""#);
                attrs
                    .split(&a)
                    .nth(1)
                    .unwrap()
                    .split('"')
                    .next()
                    .unwrap()
                    .parse()
                    .unwrap()
            })
            .collect()
    };
    // box at (100, 100), 400 x 200 px; tile 40 x 20 px scaled 2 x 3 = 80 x 60 px
    let r = tile(2.0, 3.0, 0.0, 0.0, RectAlign::TopLeft, TileFlip::None);
    assert_eq!(geom(&r.svg), vec![100.0, 100.0, 80.0, 60.0]);
    let r = tile(2.0, 3.0, e(5.0), e(7.0), RectAlign::TopLeft, TileFlip::None);
    assert_eq!(geom(&r.svg), vec![105.0, 107.0, 80.0, 60.0]);
    // aligned by the other corners and edges: the tile's own fraction of its size comes off
    let r = tile(2.0, 3.0, 0.0, 0.0, RectAlign::BottomRight, TileFlip::None);
    assert_eq!(
        geom(&r.svg),
        vec![100.0 + 400.0 - 80.0, 100.0 + 200.0 - 60.0, 80.0, 60.0]
    );
    let r = tile(2.0, 3.0, 0.0, 0.0, RectAlign::Center, TileFlip::None);
    assert_eq!(
        geom(&r.svg),
        vec![100.0 + 200.0 - 40.0, 100.0 + 100.0 - 30.0, 80.0, 60.0]
    );
    let r = tile(
        2.0,
        3.0,
        e(4.0),
        e(6.0),
        RectAlign::BottomRight,
        TileFlip::None,
    );
    assert_eq!(
        geom(&r.svg),
        vec![
            100.0 + 400.0 - 80.0 + 4.0,
            100.0 + 200.0 - 60.0 + 6.0,
            80.0,
            60.0
        ]
    );
    // flipped in both directions the period is twice as long on each axis
    let r = tile(2.0, 3.0, 0.0, 0.0, RectAlign::TopLeft, TileFlip::Xy);
    assert_eq!(geom(&r.svg), vec![100.0, 100.0, 160.0, 120.0]);
    // a tile count: the box area over the tile area
    let r = tile(2.0, 3.0, 0.0, 0.0, RectAlign::TopLeft, TileFlip::None);
    assert!(
        (r.features.tiles - 400.0 * 200.0 / (80.0 * 60.0)).abs() < 1e-9,
        "{}",
        r.features.tiles
    );
    // a scale of no use is 1, and a tile is at least 1 px
    let r = tile(0.0, -3.0, 0.0, 0.0, RectAlign::TopLeft, TileFlip::None);
    assert_eq!(geom(&r.svg)[2..], [40.0, 20.0]);
    let r = tile(0.001, 0.001, 0.0, 0.0, RectAlign::TopLeft, TileFlip::None);
    assert_eq!(geom(&r.svg)[2..], [1.0, 1.0]);
}

#[test]
fn the_cells_of_a_tiled_svg_picture_are_charged_by_their_size() {
    use super::super::hardening_tests::by_key;
    let media = by_key(vec![("v", svg_pic(r#"<rect width="100" height="50"/>"#))]);
    let k = 1280.0 / 960.0;
    let tile = |sx: f64, flip: TileFlip| {
        let f = ImageFill {
            mode: ImageMode::Tile {
                sx,
                sy: sx,
                tx: 0.0,
                ty: 0.0,
                align: RectAlign::TopLeft,
                flip,
            },
            ..ImageFill::stretch("v")
        };
        render_svg(&image_shape(f, 400.0, 200.0), &*media).features
    };
    // a 100 x 50 px picture at scale 2: 200 x 100 px; the rect paints all of it
    let one = tile(2.0, TileFlip::None);
    let want = 200.0 * 100.0 * k * k;
    assert!(
        (one.vec_px2 - want).abs() < 1e-6 * want,
        "{} vs {want}",
        one.vec_px2
    );
    // four cells when flipped both ways
    let four = tile(2.0, TileFlip::Xy);
    assert!(
        (four.vec_px2 - 4.0 * want).abs() < 1e-6 * want,
        "{}",
        four.vec_px2
    );
    let two = tile(2.0, TileFlip::X);
    assert!(
        (two.vec_px2 - 2.0 * want).abs() < 1e-6 * want,
        "{}",
        two.vec_px2
    );
}

#[test]
fn a_bitmap_is_decoded_only_within_the_decode_allowance() {
    use super::super::svg::render_with;
    let mut bmp = Vec::new();
    image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
        40,
        30,
        image::Rgba([255, 0, 0, 255]),
    ))
    .write_to(&mut std::io::Cursor::new(&mut bmp), image::ImageFormat::Bmp)
    .unwrap();
    let media = super::super::tests::media_of(bmp);
    let sc = image_shape(ImageFill::stretch("b"), 100.0, 100.0);
    // 40 x 30 = 1200 px: exactly enough, then one short
    let enough = render_with(&sc, &media, &|| false, 1200, u64::MAX);
    assert!(enough.svg.contains("data:image/png"), "decoded");
    assert!(!enough.truncated);
    let short = render_with(&sc, &media, &|| false, 1199, u64::MAX);
    assert!(!short.svg.contains("data:image/png"), "not decoded");
    assert!(short.truncated);
}

fn timed_text_boxes(n: usize) -> Vec<Item> {
    (0..n)
        .map(|i| {
            Item::Shape(text_shape(
                (i % 12) as f64 * 78.0,
                (i / 12 % 20) as f64 * 26.0,
                76.0,
                24.0,
                "ab ab ab ab ab ab ab ab ab ab ab ab ab ab ab ab ab ab ab ab",
                6.0,
            ))
        })
        .collect()
}

/// A path of 300 000 slide-crossing lines: not even its plain form fits the time budget.
fn hostile_path() -> Item {
    let mut cmds = vec![PathCmd::MoveTo(Pt::new(0.0, 0.0))];
    for i in 0..300_000 {
        let (x, y) = if i % 2 == 0 {
            (960.0, 540.0)
        } else {
            (0.0, 0.0)
        };
        cmds.push(PathCmd::LineTo(Pt::new(e(x), e(y))));
    }
    let path = GeomPath {
        w: 0.0,
        h: 0.0,
        fill_mode: PathFill::Norm,
        stroke: true,
        cmds,
    };
    let mut s = ShapeItem::new(
        Xfrm::rect(0.0, 0.0, e(960.0), e(540.0)),
        Geometry::Paths(vec![path]),
    );
    s.line = Some(Line::solid(e(1.0), Rgba::BLACK));
    Item::Shape(s)
}

fn small_red() -> Item {
    let mut s = ShapeItem::new(Xfrm::rect(e(5.0), e(500.0), e(8.0), e(8.0)), Geometry::Rect);
    s.fill = Fill::Solid(Rgba::rgb(255, 0, 0));
    Item::Shape(s)
}

#[test]
fn once_the_time_budget_is_as_good_as_used_up_nothing_after_the_left_out_shape_is_drawn() {
    let ms = |items: Vec<Item>| {
        let r = render_svg(&scene(items), &no_media);
        (r.features.predict().at(1.0), r)
    };
    // Control: on an empty slide the hostile shape is left out, and the small square after it fits.
    let (_, r) = ms(vec![hostile_path(), small_red()]);
    assert!(r.truncated);
    assert!(r.svg.contains("#ff0000"), "the square after it is drawn");
    // Text boxes up to more than 90 % of the budget: the cost of a box is found on a small slide
    // and the count is scaled to 95 % of the budget (the cost is linear in the number of boxes).
    let max = super::super::cost::MAX_CHILD_MS;
    let base = ms(timed_text_boxes(1)).0;
    let per_box = (ms(timed_text_boxes(101)).0 - base) / 100.0;
    let lo = (1.0 + (0.95 * max - base) / per_box) as usize;
    let (t, r) = ms(timed_text_boxes(lo));
    assert!(t > max * 0.9 && t < max && !r.truncated, "{t} of {max}");
    let mut items = timed_text_boxes(lo);
    items.push(hostile_path());
    items.push(small_red());
    let r = render_svg(&scene(items), &no_media);
    assert!(r.truncated);
    assert!(
        !r.svg.contains("#ff0000"),
        "with the budget used up the square after the left-out shape is not drawn"
    );
}

/// An SVG picture of exactly `len` bytes (a comment pads it), 10 x 10 px.
fn padded_svg(len: usize) -> Vec<u8> {
    let (head, tail) = (
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><!--"#,
        r#"--><rect width="10" height="10"/></svg>"#,
    );
    let mut s = String::from(head);
    s.push_str(&"x".repeat(len - head.len() - tail.len()));
    s.push_str(tail);
    assert_eq!(s.len(), len);
    s.into_bytes()
}

/// The slide the shrinking tests draw: three vector pictures that take all of the embedded-image
/// allowance but `room` bytes, then the big raster picture `big` (key "big"). Returns the media,
/// the items and the room that is really left (the three fillers are of equal size).
fn slide_with_room_left(
    big: Vec<u8>,
    room: usize,
) -> (super::super::hardening_tests::Media, Vec<Item>, usize) {
    use super::super::hardening_tests::{by_key, pic};
    let each = (super::super::svg::MAX_EMBEDDED_IMAGE_BYTES - room) / 3;
    let left = super::super::svg::MAX_EMBEDDED_IMAGE_BYTES - 3 * each;
    let media = by_key(vec![
        ("f0", padded_svg(each)),
        ("f1", padded_svg(each)),
        ("f2", padded_svg(each)),
        ("big", big),
    ]);
    let items = vec![
        pic("f0", 0.0, 0.0, 10.0, 10.0),
        pic("f1", 20.0, 0.0, 10.0, 10.0),
        pic("f2", 40.0, 0.0, 10.0, 10.0),
        pic("big", 100.0, 100.0, 400.0, 300.0),
    ];
    (media, items, left)
}

/// The raster picture embedded last in `svg`: its MIME type and its decoded size.
fn last_embedded_raster(svg: &str) -> Option<(String, (u32, u32), usize)> {
    use base64::Engine;
    let at = svg.rfind("data:image/")?;
    let rest = &svg[at + "data:image/".len()..];
    let (mime, rest) = rest.split_once(";base64,")?;
    if mime == "svg+xml" {
        return None;
    }
    let b64 = &rest[..rest.find('"')?];
    let bytes = base64::engine::general_purpose::STANDARD.decode(b64).ok()?;
    let img = image::load_from_memory(&bytes).ok()?;
    Some((mime.to_string(), (img.width(), img.height()), bytes.len()))
}

#[test]
fn an_embedded_raster_is_reduced_to_the_first_size_that_fits_and_never_enlarged() {
    use super::super::hardening_tests::noise_png;
    // 800 x 600 noise: about 700 KB as a JPEG at full size, about 320 KB at 640 x 480.
    let (media, items, left) = slide_with_room_left(noise_png(800, 600, 7), 500_000);
    let r = render_svg(&scene(items), &*media);
    assert!(r.truncated, "a reduced picture is reported");
    assert!(!r.svg.contains("#e6e6e6"), "reduced, not given up");
    let (mime, (w, h), bytes) = last_embedded_raster(&r.svg).expect("the reduced picture");
    assert_eq!(mime, "jpeg", "no transparency: a JPEG");
    assert!(bytes <= left, "{bytes} bytes in the {left} that are left");
    // the longer side is brought down to the first of 2048, 1280, 640 that makes it fit: the first
    // two are no reduction at all for a picture of 800 px (never enlarged), only 640 fits
    assert_eq!((w, h), (640, 480));
}

#[test]
fn a_raster_that_fits_after_no_reduction_is_embedded_at_its_own_size() {
    use super::super::hardening_tests::noise_png;
    // Room for the full-size JPEG (702 KB) but not for the PNG (1.4 MB): the first size tried is
    // the picture's own, not a reduced one.
    let (media, items, left) = slide_with_room_left(noise_png(800, 600, 7), 900_000);
    let r = render_svg(&scene(items), &*media);
    assert!(r.truncated);
    let (mime, (w, h), bytes) = last_embedded_raster(&r.svg).expect("the re-encoded picture");
    assert_eq!(mime, "jpeg");
    assert!(bytes <= left);
    assert_eq!((w, h), (800, 600));
}

#[test]
fn reducing_a_raster_is_charged_to_the_decode_allowance_by_its_pixels() {
    use super::super::hardening_tests::noise_png;
    use super::super::svg::render_with;
    let (media, items, _) = slide_with_room_left(noise_png(800, 600, 7), 500_000);
    let sc = scene(items);
    let px = 800 * 600;
    // exactly the pixels it decodes to: reduced
    let enough = render_with(&sc, &*media, &|| false, px, u64::MAX);
    assert!(enough.truncated);
    assert!(!enough.svg.contains("#e6e6e6"), "reduced");
    assert!(last_embedded_raster(&enough.svg).is_some());
    // one pixel short: the picture is not decoded and is given up
    let short = render_with(&sc, &*media, &|| false, px - 1, u64::MAX);
    assert!(short.truncated);
    assert!(short.svg.contains("#e6e6e6"), "a placeholder");
    assert!(last_embedded_raster(&short.svg).is_none());
    // the pixels are width times height (not their sum or ratio): a tenth of the allowance
    let tenth = render_with(&sc, &*media, &|| false, px / 10, u64::MAX);
    assert!(tenth.svg.contains("#e6e6e6"));
}

#[test]
fn reducing_a_raster_stops_when_the_slide_is_cancelled() {
    use super::super::hardening_tests::noise_png;
    use super::super::svg::render_with;
    let (media, items, _) = slide_with_room_left(noise_png(800, 600, 7), 500_000);
    let sc = scene(items);
    let r = render_with(&sc, &*media, &|| true, u64::MAX, u64::MAX);
    assert!(r.cancelled);
    assert!(last_embedded_raster(&r.svg).is_none());
}

/// One line of `chars` letters that is not wrapped, `lines` times.
fn long_line_text(lines: usize, chars: usize) -> SlideScene {
    let mut s = ShapeItem::new(Xfrm::rect(0.0, 0.0, e(900.0), e(500.0)), Geometry::Rect);
    s.text = Some(TextBody {
        wrap: false,
        paragraphs: (0..lines)
            .map(|_| Paragraph {
                runs: vec![Run::text("x".repeat(chars), 6.0)],
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    });
    scene(vec![Item::Shape(s)])
}

#[test]
fn a_text_box_is_cut_to_the_lines_that_fit_the_time_budget() {
    let max = super::super::cost::MAX_CHILD_MS;
    let ms = |r: &super::super::Rendered| r.features.predict().at(1.0);
    // two lines of 40 000 letters fit (about 1260 ms), a third would not
    let two = render_svg(&long_line_text(2, 40_000), &no_media);
    assert!(!two.truncated && ms(&two) < max, "{}", ms(&two));
    assert_eq!(two.svg.matches("<tspan").count(), 2);
    let three = render_svg(&long_line_text(3, 40_000), &no_media);
    assert!(three.truncated, "the third line is left out and said so");
    // the lines that fit are drawn (the shape is not given up for the one line too many)
    assert_eq!(three.svg.matches("<tspan").count(), 2);
    assert!(ms(&three) <= max * 1.0001, "{}", ms(&three));
    // and the same when there are many more
    let many = render_svg(&long_line_text(30, 40_000), &no_media);
    assert!(many.truncated);
    assert_eq!(many.svg.matches("<tspan").count(), 2);
}

#[test]
fn pictures_past_the_time_budget_are_placeholders_not_missing() {
    use super::super::hardening_tests::{by_key, pic};
    // a picture of a million bytes costs the process 50 ms to parse each time it is used
    let media = by_key(vec![("v", padded_svg(1_000_000))]);
    let items: Vec<Item> = (0..40)
        .map(|i| {
            pic(
                "v",
                (i % 10) as f64 * 90.0,
                (i / 10) as f64 * 90.0,
                80.0,
                80.0,
            )
        })
        .collect();
    let r = render_svg(&scene(items), &*media);
    let max = super::super::cost::MAX_CHILD_MS;
    let (drawn, placeholders) = (
        r.svg.matches("<use ").count(),
        r.svg.matches("#e6e6e6").count(),
    );
    assert!(r.truncated);
    assert!(drawn >= 20, "what fits is drawn: {drawn}");
    assert!(placeholders >= 1, "the rest is shown as placeholders");
    assert_eq!(drawn + placeholders, 40, "every picture is accounted for");
    assert!(r.features.predict().at(1.0) <= max, "inside the budget");
    // the same pictures in less than the budget are all drawn
    let few: Vec<Item> = (0..10)
        .map(|i| pic("v", i as f64 * 90.0, 0.0, 80.0, 80.0))
        .collect();
    let r = render_svg(&scene(few), &*media);
    assert!(!r.truncated);
    assert_eq!(r.svg.matches("<use ").count(), 10);
    assert!(!r.svg.contains("#e6e6e6"));
}

#[test]
fn the_filter_allowance_cannot_be_passed_without_the_time_budget_noticing() {
    // `leaf` checks the filter work against its allowance and the predicted time against its
    // budget. The first check can never be the only one to refuse a shape: the work allowance
    // alone already costs more than the whole time budget. If the constants are changed so
    // that it can, `W::over_filter_work` needs a test of its own.
    let at_allowance = super::super::cost::Features {
        filter_units: super::super::svg::MAX_FILTER_WORK,
        ..Default::default()
    };
    assert!(
        at_allowance.predict().at(1.0) > super::super::cost::MAX_CHILD_MS,
        "{}",
        at_allowance.predict().at(1.0)
    );
}

/// A single-rectangle EMF, `w` x `h` px, filled red.
fn red_emf(w: i32, h: i32) -> Vec<u8> {
    let words =
        |v: &[i64]| -> Vec<u8> { v.iter().flat_map(|x| (*x as u32).to_le_bytes()).collect() };
    let rec = |ty: u32, payload: &[i64]| -> Vec<u8> {
        let p = words(payload);
        let mut r = ty.to_le_bytes().to_vec();
        r.extend(((p.len() + 8) as u32).to_le_bytes());
        r.extend(p);
        r
    };
    let mut recs = Vec::new();
    recs.extend(rec(39, &[1, 0, 0x0000FF, 0])); // red brush
    recs.extend(rec(37, &[1])); // select it
    recs.extend(rec(37, &[0x8000_0008])); // no pen
    recs.extend(rec(43, &[0, 0, w as i64, h as i64])); // rectangle
    recs.extend(rec(14, &[0, 20, 0])); // end of file
    let frame = [
        0,
        0,
        (w as f64 * 26.4583) as i64,
        (h as f64 * 26.4583) as i64,
    ];
    let mut out = words(&[1, 88, 0, 0, (w - 1) as i64, (h - 1) as i64]);
    out.extend(words(&frame));
    out.extend(words(&[0x464D_4520, 0x10000, 0, 6, 0, 0, 0, 0]));
    out.extend(words(&[960, 960, 254, 254]));
    assert_eq!(out.len(), 88);
    let total = (88 + recs.len()) as u32;
    out[48..52].copy_from_slice(&total.to_le_bytes());
    out.extend(recs);
    out
}

#[test]
fn a_metafile_picture_is_converted_and_drawn_not_a_placeholder() {
    use super::super::tests::{at, media_of, raster};
    let emf = red_emf(100, 50);
    // stretched into a box
    let p = PictureItem::new(Xfrm::rect(e(100.0), e(100.0), e(200.0), e(100.0)), "m");
    let r = render_svg(&scene(vec![Item::Picture(p)]), &media_of(emf.clone()));
    assert!(
        r.svg.contains("data:image/svg+xml;base64,"),
        "converted to an SVG"
    );
    assert!(!r.svg.contains("#e6e6e6"), "no placeholder");
    let px = at(&raster(&r), 200, 150);
    assert!(px[0] > 200 && px[1] < 60 && px[2] < 60, "{px:?}");
    // tiled: the cell is the metafile's own size, which only the conversion knows
    let f = ImageFill {
        mode: ImageMode::Tile {
            sx: 1.0,
            sy: 1.0,
            tx: 0.0,
            ty: 0.0,
            align: RectAlign::TopLeft,
            flip: TileFlip::None,
        },
        ..ImageFill::stretch("m")
    };
    let r = render_svg(&image_shape(f, 400.0, 200.0), &media_of(emf));
    assert!(
        !r.svg.contains("#e6e6e6"),
        "a tile of a metafile is not a placeholder"
    );
    assert!(r.svg.contains("<use "));
}

#[test]
fn a_picture_of_no_size_or_an_unmeasurable_one_draws_nothing() {
    use super::super::hardening_tests::by_key;
    let media = by_key(vec![("p", two_halves_png(8, 8))]);
    let draw = |w: f64, h: f64, fill_rect: Rect4| {
        let mut p = PictureItem::new(Xfrm::rect(e(10.0), e(10.0), w, h), "p");
        p.image.mode = ImageMode::Stretch { fill_rect };
        render_svg(&scene(vec![Item::Picture(p)]), &*media)
    };
    let none = (0.0, 0.0, 0.0, 0.0);
    let ok = draw(e(100.0), e(50.0), none);
    assert_eq!(
        ok.svg.matches("<use ").count(),
        1,
        "control: an ordinary picture"
    );
    let cases = [
        ("zero width", e(0.0), e(50.0), none),
        ("zero height", e(100.0), e(0.0), none),
        ("negative width", e(-100.0), e(50.0), none),
        ("negative height", e(100.0), e(-50.0), none),
        ("infinite width", f64::INFINITY, e(50.0), none),
        ("infinite height", e(100.0), f64::INFINITY, none),
        (
            "negative infinite height",
            e(100.0),
            f64::NEG_INFINITY,
            none,
        ),
        ("NaN width", f64::NAN, e(50.0), none),
        ("NaN height", e(100.0), f64::NAN, none),
        // an inset that takes more than the box has
        (
            "inset wider than the box",
            e(100.0),
            e(50.0),
            (1.5, 0.0, 0.0, 0.0),
        ),
        (
            "inset taller than the box",
            e(100.0),
            e(50.0),
            (0.0, 1.5, 0.0, 0.0),
        ),
    ];
    for (what, w, h, fill_rect) in cases {
        let r = draw(w, h, fill_rect);
        assert_eq!(
            r.svg.matches("<use ").count(),
            0,
            "{what}: nothing is drawn"
        );
        assert!(!r.svg.contains("NaN") && !r.svg.contains("inf"), "{what}");
    }
}

// ---- picture placement (`image_el`) ------------------------------------------------------------

/// The first `<use>` of a picture in `svg`.
fn first_use(svg: &str) -> &str {
    let at = svg.find("<use href=\"#im").expect("a picture is drawn");
    let rest = &svg[at..];
    &rest[..rest.find("/>").unwrap()]
}

/// The matrix `[width, height, x, y]` of the first picture in `svg`, in slide px.
fn use_matrix(svg: &str) -> [f64; 4] {
    let el = first_use(svg);
    let m = el.find("matrix(").expect("with a matrix") + "matrix(".len();
    let n: Vec<f64> = el[m..el[m..].find(')').unwrap() + m]
        .split_whitespace()
        .map(|v| v.parse().unwrap())
        .collect();
    assert_eq!((n[1], n[2]), (0.0, 0.0), "no skew: {n:?}");
    [n[0], n[3], n[4], n[5]]
}

/// The picture "p" in the frame (100, 100, 200 x 100 px), changed by `change`.
fn placed(change: impl FnOnce(&mut PictureItem)) -> super::super::Rendered {
    use super::super::hardening_tests::by_key;
    let media = by_key(vec![("p", two_halves_png(8, 8))]);
    let mut p = PictureItem::new(Xfrm::rect(e(100.0), e(100.0), e(200.0), e(100.0)), "p");
    change(&mut p);
    render_svg(&scene(vec![Item::Picture(p)]), &*media)
}

fn assert_matrix(r: &super::super::Rendered, want: [f64; 4], what: &str) {
    let got = use_matrix(&r.svg);
    for (g, w) in got.iter().zip(want) {
        assert!((g - w).abs() < 0.01, "{what}: {got:?} against {want:?}");
    }
}

#[test]
fn a_picture_is_placed_by_its_frame_its_crop_and_its_stretch_inset() {
    let crop = |c: Rect4| placed(|p| p.image.crop = c);
    let inset = |f: Rect4| placed(|p| p.image.mode = ImageMode::Stretch { fill_rect: f });
    // Frame 200 x 100 at (100, 100); without crop the picture is the frame.
    assert_matrix(&placed(|_| {}), [200.0, 100.0, 100.0, 100.0], "plain");
    // srcRect: the part that is shown is what is left after cutting a fraction off each side, so
    // the whole picture is the frame divided by that part and shifted back by the cut at the
    // left / top.
    let fw = 200.0 / 0.75;
    assert_matrix(
        &crop((0.25, 0.0, 0.0, 0.0)),
        [fw, 100.0, 100.0 - 0.25 * fw, 100.0],
        "crop left",
    );
    let fh = 100.0 / 0.9;
    assert_matrix(
        &crop((0.0, 0.1, 0.0, 0.0)),
        [200.0, fh, 100.0, 100.0 - 0.1 * fh],
        "crop top",
    );
    assert_matrix(
        &crop((0.0, 0.0, 0.5, 0.0)),
        [400.0, 100.0, 100.0, 100.0],
        "crop right",
    );
    assert_matrix(
        &crop((0.0, 0.0, 0.0, 0.2)),
        [200.0, 125.0, 100.0, 100.0],
        "crop bottom",
    );
    // a negative crop adds empty space round the picture: it is smaller than the frame
    assert_matrix(
        &crop((-0.25, 0.0, 0.0, 0.0)),
        [160.0, 100.0, 140.0, 100.0],
        "pad left",
    );
    let fh = 100.0 / 1.5;
    assert_matrix(
        &crop((0.0, -0.5, 0.0, 0.0)),
        [200.0, fh, 100.0, 100.0 + 0.5 * fh],
        "pad top",
    );
    assert_matrix(
        &crop((0.0, 0.0, -1.0, 0.0)),
        [100.0, 100.0, 100.0, 100.0],
        "pad right",
    );
    assert_matrix(
        &crop((0.0, 0.0, 0.0, -1.0)),
        [200.0, 50.0, 100.0, 100.0],
        "pad bottom",
    );
    // fillRect: the picture fills the frame less an inset (fractions of the frame) on each side
    assert_matrix(
        &inset((0.1, 0.0, 0.0, 0.0)),
        [180.0, 100.0, 120.0, 100.0],
        "inset left",
    );
    assert_matrix(
        &inset((0.0, 0.2, 0.0, 0.0)),
        [200.0, 80.0, 100.0, 120.0],
        "inset top",
    );
    assert_matrix(
        &inset((0.0, 0.0, 0.3, 0.0)),
        [140.0, 100.0, 100.0, 100.0],
        "inset right",
    );
    assert_matrix(
        &inset((0.0, 0.0, 0.0, 0.1)),
        [200.0, 90.0, 100.0, 100.0],
        "inset bottom",
    );
    assert_matrix(
        &inset((0.1, 0.2, 0.3, 0.1)),
        [120.0, 70.0, 120.0, 120.0],
        "inset all",
    );
    // both: the crop is of the inset area (120 x 70 at (120, 120))
    let r = placed(|p| {
        p.image.crop = (0.25, 0.1, 0.0, 0.2);
        p.image.mode = ImageMode::Stretch {
            fill_rect: (0.1, 0.2, 0.3, 0.1),
        };
    });
    assert_matrix(
        &r,
        [160.0, 100.0, 120.0 - 0.25 * 160.0, 120.0 - 0.1 * 100.0],
        "both",
    );
    // an infinite inset or crop counts as none
    assert_matrix(
        &crop((f64::NAN, f64::INFINITY, 0.0, 0.0)),
        [200.0, 100.0, 100.0, 100.0],
        "not a number",
    );
    // a crop that leaves nothing is stretched from a thousandth of the picture, never divided by zero
    assert_matrix(
        &crop((0.5, 0.0, 0.5, 0.0)),
        [200.0 / 0.001, 100.0, 100.0 - 0.5 * 200.0 / 0.001, 100.0],
        "nothing left",
    );
}

#[test]
fn a_flipped_picture_keeps_its_placement_inside_a_flipped_group() {
    let plain = placed(|_| {});
    let flipped = placed(|p| p.xfrm.flip_h = true);
    assert!(flipped.svg.contains("scale(-1 1)"), "{}", flipped.svg);
    assert!(!plain.svg.contains("scale(-1 1)"));
    // the matrix is in the frame's own system: the flip is the group's
    assert_eq!(use_matrix(&plain.svg), use_matrix(&flipped.svg));
    let v = placed(|p| p.xfrm.flip_v = true);
    assert!(v.svg.contains("scale(1 -1)"));
    assert_eq!(use_matrix(&plain.svg), use_matrix(&v.svg));
}

#[test]
fn a_picture_is_charged_for_the_area_it_is_drawn_over_at_the_model_raster() {
    let k2 = (1280.0f64 / 960.0).powi(2);
    let area = |r: &super::super::Rendered| r.features.pic_px2;
    let r = placed(|_| {});
    assert!(
        (area(&r) - 200.0 * 100.0 * k2).abs() < 1e-6 * area(&r),
        "{}",
        area(&r)
    );
    // the whole picture counts, not just the part of it in the frame
    let r = placed(|p| p.image.crop = (0.25, 0.0, 0.0, 0.0));
    let want = 200.0 / 0.75 * 100.0 * k2;
    assert!((area(&r) - want).abs() < 1e-6 * want, "{}", area(&r));
    let r = placed(|p| {
        p.image.mode = ImageMode::Stretch {
            fill_rect: (0.1, 0.2, 0.3, 0.1),
        }
    });
    let want = 120.0 * 70.0 * k2;
    assert!((area(&r) - want).abs() < 1e-6 * want, "{}", area(&r));
    // two pictures add up
    let media = super::super::hardening_tests::by_key(vec![("p", two_halves_png(8, 8))]);
    let two = vec![
        super::super::hardening_tests::pic("p", 0.0, 0.0, 200.0, 100.0),
        super::super::hardening_tests::pic("p", 300.0, 0.0, 200.0, 100.0),
    ];
    let r = render_svg(&scene(two), &*media);
    assert!(
        (area(&r) - 2.0 * 200.0 * 100.0 * k2).abs() < 1e-6 * area(&r),
        "{}",
        area(&r)
    );
}

#[test]
fn a_picture_is_translucent_only_below_full_opacity() {
    let opacity = |alpha: f64| {
        let r = placed(|p| p.image.alpha = alpha);
        let el = first_use(&r.svg).to_string();
        el.split(" opacity=\"")
            .nth(1)
            .map(|rest| rest[..rest.find('"').unwrap()].to_string())
    };
    assert_eq!(opacity(0.5).as_deref(), Some("0.5"));
    assert_eq!(opacity(0.0).as_deref(), Some("0"));
    assert_eq!(opacity(-1.0).as_deref(), Some("0"), "clamped to 0");
    assert_eq!(opacity(1.0), None, "opaque: no attribute");
    assert_eq!(opacity(2.0), None, "clamped to 1");
    assert_eq!(opacity(f64::NAN), None, "not a number is opaque");
}

/// [`red_emf`] with a record that is shorter than a record header after the rectangle: the
/// converter drops what follows and says so.
fn cut_emf() -> Vec<u8> {
    let mut b = red_emf(100, 50);
    // overwrite the end-of-file record (its first 8 bytes) with a record of size 4
    let n = b.len();
    b[n - 20..n - 16].copy_from_slice(&99u32.to_le_bytes());
    b[n - 16..n - 12].copy_from_slice(&4u32.to_le_bytes());
    b
}

#[test]
fn a_metafile_that_was_cut_marks_the_picture_truncated() {
    use super::super::tests::media_of;
    let scene_of = |emf: Vec<u8>| {
        let p = PictureItem::new(Xfrm::rect(e(100.0), e(100.0), e(200.0), e(100.0)), "m");
        render_svg(&scene(vec![Item::Picture(p)]), &media_of(emf))
    };
    let whole = scene_of(red_emf(100, 50));
    assert!(!whole.truncated, "control: a whole metafile");
    let cut = scene_of(cut_emf());
    assert!(cut.svg.contains("data:image/svg+xml"), "still drawn");
    assert!(cut.truncated, "what the converter dropped is said");
    // the same through a tile (the size of the cell comes from the conversion)
    let f = ImageFill {
        mode: ImageMode::Tile {
            sx: 1.0,
            sy: 1.0,
            tx: 0.0,
            ty: 0.0,
            align: RectAlign::TopLeft,
            flip: TileFlip::None,
        },
        ..ImageFill::stretch("m")
    };
    let tiled = render_svg(&image_shape(f, 400.0, 200.0), &media_of(cut_emf()));
    assert!(tiled.truncated);
}

/// A box in px: x, y, width, height.
type Bx = (f64, f64, f64, f64);
/// A line from one point to another, relative to its box.
type Run2 = ((f64, f64), (f64, f64));

/// The markup of the arrow head drawn at the tail (the end) of a line of 4 px, from `(x, y)` of
/// `w` x `h` px, flipped as given; the head is 20 px long and 12 px wide.
fn tail_head(kind: ArrowKind, (x, y, w, h): Bx, (from, to): Run2) -> String {
    let at = |(px, py): (f64, f64)| Pt::new(e(px), e(py));
    let path = GeomPath {
        w: 0.0,
        h: 0.0,
        stroke: true,
        cmds: vec![PathCmd::MoveTo(at(from)), PathCmd::LineTo(at(to))],
        ..Default::default()
    };
    let xf = Xfrm::rect(e(x), e(y), e(w), e(h));
    let mut s = ShapeItem::new(xf, Geometry::Paths(vec![path]));
    let mut l = Line::solid(e(4.0), Rgba::rgb(255, 0, 0));
    l.tail = Some(Arrow {
        kind,
        w: ArrowSize::Med,
        len: ArrowSize::Lg,
    });
    s.line = Some(l);
    let r = render_svg(&scene(vec![Item::Shape(s)]), &no_media);
    // the markup of the head comes after the line: the last element that is filled with the line
    // colour at its own opacity (or the stroked chevron, or the ellipse)
    let at = r
        .svg
        .rfind("<ellipse")
        .or_else(|| {
            r.svg.rfind("<path d=\"M").filter(|_| {
                r.svg
                    .contains("stroke-linecap=\"round\" stroke-linejoin=\"round\"/>")
            })
        })
        .or_else(|| r.svg.rfind("<path "))
        .expect("a head");
    r.svg[at..].split("/>").next().unwrap().to_string()
}

fn numbers(s: &str) -> Vec<f64> {
    s.split(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-'))
        .filter(|t| !t.is_empty() && *t != "-" && *t != ".")
        .filter_map(|t| t.parse().ok())
        .collect()
}

/// The corners of a head path, whichever one the path starts at.
fn corners(head: &str) -> Vec<(f64, f64)> {
    let d = head
        .split("d=\"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap();
    let mut pts: Vec<(f64, f64)> = numbers(d).chunks(2).map(|c| (c[0], c[1])).collect();
    pts.sort_by(|a, b| a.partial_cmp(b).unwrap());
    pts
}

/// The head's corners expected when its tip is `tip` and it points along `dir`: `(along, across)`
/// per corner, along being measured from the tip backwards, across to either side.
fn expected(tip: (f64, f64), dir: (f64, f64), local: &[(f64, f64)]) -> Vec<(f64, f64)> {
    let across = (-dir.1, dir.0);
    let mut pts: Vec<(f64, f64)> = local
        .iter()
        .map(|(u, v)| {
            (
                tip.0 + dir.0 * u + across.0 * v,
                tip.1 + dir.1 * u + across.1 * v,
            )
        })
        .collect();
    pts.sort_by(|a, b| a.partial_cmp(b).unwrap());
    pts
}

fn assert_corners(got: &[(f64, f64)], want: &[(f64, f64)], what: &str) {
    assert_eq!(got.len(), want.len(), "{what}: {got:?} against {want:?}");
    for (g, w) in got.iter().zip(want) {
        assert!(
            (g.0 - w.0).abs() < 0.02 && (g.1 - w.1).abs() < 0.02,
            "{what}: {got:?} against {want:?}"
        );
    }
}

/// The lines the head tests draw: the box, the line in it, where its tip is and which way it
/// points.
#[allow(clippy::type_complexity)]
fn head_lines() -> Vec<(&'static str, Bx, Run2, (f64, f64), (f64, f64))> {
    vec![
        (
            "right",
            (100.0, 200.0, 600.0, 10.0),
            ((0.0, 0.0), (600.0, 0.0)),
            (700.0, 200.0),
            (1.0, 0.0),
        ),
        (
            "down",
            (200.0, 100.0, 10.0, 400.0),
            ((0.0, 0.0), (0.0, 400.0)),
            (200.0, 500.0),
            (0.0, 1.0),
        ),
        (
            "left",
            (100.0, 200.0, 600.0, 10.0),
            ((600.0, 0.0), (0.0, 0.0)),
            (100.0, 200.0),
            (-1.0, 0.0),
        ),
        (
            "up",
            (200.0, 100.0, 10.0, 400.0),
            ((0.0, 400.0), (0.0, 0.0)),
            (200.0, 100.0),
            (0.0, -1.0),
        ),
        (
            "slanted",
            (100.0, 100.0, 300.0, 400.0),
            ((0.0, 0.0), (300.0, 400.0)),
            (400.0, 500.0),
            (0.6, 0.8),
        ),
    ]
}

#[test]
fn filled_arrow_heads_are_drawn_from_the_tip_back_along_the_line() {
    // 20 px long (Lg x a 4 px line), 12 px wide (Med x 4)
    type Corners = &'static [(f64, f64)];
    let kinds: [(&str, ArrowKind, Corners); 3] = [
        (
            "triangle",
            ArrowKind::Triangle,
            &[(0.0, 0.0), (-20.0, -6.0), (-20.0, 6.0)],
        ),
        (
            "stealth",
            ArrowKind::Stealth,
            &[(0.0, 0.0), (-20.0, -6.0), (-14.0, 0.0), (-20.0, 6.0)],
        ),
        (
            "diamond",
            ArrowKind::Diamond,
            &[(0.0, 0.0), (-10.0, -6.0), (-20.0, 0.0), (-10.0, 6.0)],
        ),
    ];
    for (name, kind, local) in kinds {
        for (dir_name, rect, path, tip, dir) in head_lines() {
            let head = tail_head(kind.clone(), rect, path);
            assert_corners(
                &corners(&head),
                &expected(tip, dir, local),
                &format!("{name} {dir_name}"),
            );
        }
    }
}

#[test]
fn an_oval_head_is_an_ellipse_behind_the_tip_turned_to_the_line() {
    for (dir_name, rect, path, tip, dir) in head_lines() {
        let head = tail_head(ArrowKind::Oval, rect, path);
        let n = numbers(&head);
        // cx, cy, rx, ry, then the rotation (degrees) about the centre
        let centre = expected(tip, dir, &[(-10.0, 0.0)])[0];
        assert!(
            (n[0] - centre.0).abs() < 0.02 && (n[1] - centre.1).abs() < 0.02,
            "{dir_name}: {n:?}"
        );
        assert!(
            (n[2] - 10.0).abs() < 0.02 && (n[3] - 6.0).abs() < 0.02,
            "{dir_name}: {n:?}"
        );
        let turn = dir.1.atan2(dir.0).to_degrees();
        assert!(
            (n[4] - turn).abs() < 0.02,
            "{dir_name}: {n:?} turned {turn}"
        );
        assert!(
            (n[5] - centre.0).abs() < 0.02 && (n[6] - centre.1).abs() < 0.02,
            "{dir_name}"
        );
    }
}

#[test]
fn a_chevron_head_is_the_open_path_from_one_side_through_the_tip_to_the_other() {
    for (dir_name, rect, path, tip, dir) in head_lines() {
        let head = tail_head(ArrowKind::Arrow, rect, path);
        let d = head
            .split("d=\"")
            .nth(1)
            .unwrap()
            .split('"')
            .next()
            .unwrap();
        let n = numbers(d);
        let want = expected(tip, dir, &[(-20.0, -6.0), (0.0, 0.0), (-20.0, 6.0)]);
        // in order: the path is not closed, so the order is the drawing
        let across = (-dir.1, dir.0);
        let first = (
            tip.0 - dir.0 * 20.0 - across.0 * 6.0,
            tip.1 - dir.1 * 20.0 - across.1 * 6.0,
        );
        assert!(
            (n[0] - first.0).abs() < 0.02 && (n[1] - first.1).abs() < 0.02,
            "{dir_name}: {n:?}"
        );
        assert!(
            (n[2] - tip.0).abs() < 0.02 && (n[3] - tip.1).abs() < 0.02,
            "{dir_name}: {n:?}"
        );
        let mut got: Vec<(f64, f64)> = n.chunks(2).map(|c| (c[0], c[1])).collect();
        got.sort_by(|a, b| a.partial_cmp(b).unwrap());
        assert_corners(&got, &want, dir_name);
    }
}

#[test]
fn a_custom_head_is_its_path_scaled_to_the_head_with_its_x_axis_towards_the_tip() {
    let tri = |w: f64, h: f64, (x, y): (f64, f64)| {
        // a triangle with its tip on the right middle of a path space of x by y
        ArrowKind::Custom(vec![GeomPath {
            w,
            h,
            fill_mode: PathFill::Norm,
            stroke: false,
            cmds: vec![
                PathCmd::MoveTo(Pt::new(0.0, 0.0)),
                PathCmd::LineTo(Pt::new(x, y / 2.0)),
                PathCmd::LineTo(Pt::new(0.0, y)),
                PathCmd::Close,
            ],
        }])
    };
    let local = [(0.0, 0.0), (-20.0, -6.0), (-20.0, 6.0)];
    // a path space of 10 x 10 as declared, and of 1 x 1 when no size (or a negative one) is
    // declared; the commands of the second are in that unit space
    let cases = [
        ("10 x 10", tri(10.0, 10.0, (10.0, 10.0))),
        ("no size", tri(0.0, 0.0, (1.0, 1.0))),
        ("no width", tri(0.0, 1.0, (1.0, 1.0))),
        ("no height", tri(1.0, 0.0, (1.0, 1.0))),
        ("negative", tri(-3.0, -3.0, (1.0, 1.0))),
        ("20 x 4", tri(20.0, 4.0, (20.0, 4.0))),
    ];
    for (name, kind) in cases {
        for (dir_name, rect, path, tip, dir) in head_lines() {
            let head = tail_head(kind.clone(), rect, path);
            assert_corners(
                &corners(&head),
                &expected(tip, dir, &local),
                &format!("{name} {dir_name}"),
            );
        }
    }
}
