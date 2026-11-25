//! EMF: every record family played into pixels, plus the DIB decoder.

use super::tests::*;
use super::*;

pub(super) const RED: i64 = 0x0000FF;
pub(super) const GREEN: i64 = 0x00FF00;
pub(super) const BLUE: i64 = 0xFF0000;

pub(super) fn pts32(e: &mut Emf, ty: u32, pts: &[(i32, i32)]) {
    let mut p = words(&[0, 0, 0, 0, pts.len() as i64]);
    for (x, y) in pts {
        p.extend(i32::to_le_bytes(*x));
        p.extend(i32::to_le_bytes(*y));
    }
    e.raw(ty, &p);
}

pub(super) fn pts16(e: &mut Emf, ty: u32, pts: &[(i32, i32)]) {
    let mut p = words(&[0, 0, 0, 0, pts.len() as i64]);
    for (x, y) in pts {
        p.extend((*x as i16).to_le_bytes());
        p.extend((*y as i16).to_le_bytes());
    }
    e.raw(ty, &p);
}

pub(super) fn text_rec(
    e: &mut Emf,
    wide: bool,
    x: i32,
    y: i32,
    s: &str,
    dx: Option<&[i32]>,
    opts: u32,
) {
    let body: Vec<u8> = if wide {
        s.encode_utf16().flat_map(|u| u.to_le_bytes()).collect()
    } else {
        s.bytes().collect()
    };
    let n = if wide { body.len() / 2 } else { body.len() };
    let padded = body.len().div_ceil(4) * 4;
    let off_dx = if dx.is_some() { 76 + padded } else { 0 };
    let mut p = words(&[
        0,
        0,
        0,
        0,
        1,
        fl(1.0),
        fl(1.0),
        x as i64,
        y as i64,
        n as i64,
        76,
        opts as i64,
        0,
        0,
        0,
        0,
        off_dx as i64,
    ]);
    p.extend(&body);
    p.resize(p.len() - body.len() + padded, 0);
    if let Some(d) = dx {
        for v in d {
            p.extend(v.to_le_bytes());
        }
    }
    e.raw(if wide { 84 } else { 83 }, &p);
}

#[allow(clippy::too_many_arguments)]
pub(super) fn font(
    e: &mut Emf,
    idx: u32,
    height: i32,
    esc: i32,
    weight: i32,
    flags: [u8; 3],
    charset: u8,
    face: &str,
) {
    let mut p = words(&[
        idx as i64,
        height as i64,
        0,
        esc as i64,
        esc as i64,
        weight as i64,
    ]);
    p.extend([flags[0], flags[1], flags[2], charset, 0, 0, 0, 0x22]);
    let mut f: Vec<u8> = face.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
    f.resize(64, 0);
    p.extend(f);
    e.raw(82, &p);
}

/// A BITMAPINFOHEADER (+ masks / palette) and its bits.
pub(super) fn bmi(
    w: i32,
    h: i32,
    bpp: u16,
    compression: u32,
    palette: &[[u8; 3]],
    masks: &[u32],
) -> Vec<u8> {
    let mut b = words(&[40, w as i64, h as i64]);
    b.extend(1u16.to_le_bytes());
    b.extend(bpp.to_le_bytes());
    b.extend(words(&[
        compression as i64,
        0,
        0,
        0,
        palette.len() as i64,
        0,
    ]));
    for m in masks {
        b.extend(m.to_le_bytes());
    }
    for c in palette {
        b.extend([c[2], c[1], c[0], 0]);
    }
    b
}

/// 24 bpp rows (given top to bottom as RGB), stored bottom-up unless `top_down`.
pub(super) fn bits24(w: usize, rows: &[Vec<[u8; 3]>], top_down: bool) -> Vec<u8> {
    let stride = (w * 3).div_ceil(4) * 4;
    let order: Vec<&Vec<[u8; 3]>> = if top_down {
        rows.iter().collect()
    } else {
        rows.iter().rev().collect()
    };
    let mut out = Vec::new();
    for r in order {
        let mut row = Vec::new();
        for c in r {
            row.extend([c[2], c[1], c[0]]);
        }
        row.resize(stride, 0);
        out.extend(row);
    }
    out
}

/// A 2 x 2 picture: red green / blue white.
pub(super) fn quad() -> Vec<Vec<[u8; 3]>> {
    vec![
        vec![[255, 0, 0], [0, 255, 0]],
        vec![[0, 0, 255], [255, 255, 255]],
    ]
}

pub(super) fn stretchdib(
    e: &mut Emf,
    dest: (i32, i32, i32, i32),
    src: (i32, i32, i32, i32),
    bmi: &[u8],
    bits: &[u8],
    rop: u32,
) {
    let off_bmi = 80;
    let bmi_pad = bmi.len().div_ceil(4) * 4;
    let mut p = words(&[
        0,
        0,
        0,
        0,
        dest.0 as i64,
        dest.1 as i64,
        src.0 as i64,
        src.1 as i64,
        src.2 as i64,
        src.3 as i64,
        off_bmi,
        bmi.len() as i64,
        (off_bmi as usize + bmi_pad) as i64,
        bits.len() as i64,
        0,
        rop as i64,
        dest.2 as i64,
        dest.3 as i64,
    ]);
    p.extend(bmi);
    p.resize(72 + bmi_pad, 0);
    p.extend(bits);
    e.raw(81, &p);
}

pub(super) fn bitblt(
    e: &mut Emf,
    ty: u32,
    dest: (i32, i32, i32, i32),
    src: (i32, i32, i32, i32),
    bmi: &[u8],
    bits: &[u8],
    rop: u32,
) {
    let off_bmi = if ty == 77 { 108 } else { 100 };
    let bmi_pad = bmi.len().div_ceil(4) * 4;
    let mut p = words(&[
        0,
        0,
        0,
        0,
        dest.0 as i64,
        dest.1 as i64,
        dest.2 as i64,
        dest.3 as i64,
        rop as i64,
        src.0 as i64,
        src.1 as i64, // xSrc ySrc
        0,
        0,
        0,
        0,
        0,
        0, // xform
        0,
        0, // bk, usage
        off_bmi as i64,
        bmi.len() as i64,
        (off_bmi + bmi_pad) as i64,
        bits.len() as i64,
    ]);
    if ty == 77 {
        p.extend(words(&[src.2 as i64, src.3 as i64]));
    }
    p.extend(bmi);
    let fixed = off_bmi - 8;
    p.resize(fixed + bmi_pad, 0);
    p.extend(bits);
    e.raw(ty, &p);
}

// ----- fills and strokes ---------------------------------------------------------------------

#[test]
fn a_filled_rectangle_is_red_inside_and_clear_outside() {
    let mut e = Emf::new(100, 100);
    e.fill_with(RED).rect(20, 20, 80, 80);
    let img = draw(&e.finish());
    assert!(is_red(at(&img, 50, 50)));
    assert!(is_red(at(&img, 25, 25)));
    assert!(is_clear(at(&img, 10, 10)));
    assert!(is_clear(at(&img, 90, 50)));
}

#[test]
fn the_default_brush_is_white_and_the_default_pen_black() {
    let mut e = Emf::new(100, 100);
    e.rect(20, 20, 80, 80);
    let img = draw(&e.finish());
    assert!(is_near(at(&img, 50, 50), [255, 255, 255], 2));
    assert!(count(&img, is_inkish) > 100, "the outline is drawn");
}

#[test]
fn a_wide_pen_strokes_the_outline_only() {
    let mut e = Emf::new(100, 100);
    e.pen(1, 0, 10, 0).select(1).stock(5).rect(20, 20, 80, 80);
    let img = draw(&e.finish());
    assert!(is_dark(at(&img, 20, 50)));
    assert!(is_dark(at(&img, 17, 50)));
    assert!(is_clear(at(&img, 50, 50)), "null brush");
    assert!(is_clear(at(&img, 10, 50)));
}

#[test]
fn pen_colour_and_null_pen() {
    let mut e = Emf::new(100, 100);
    e.pen(1, 0, 8, GREEN)
        .select(1)
        .stock(5)
        .rect(20, 20, 80, 80);
    let img = draw(&e.finish());
    assert!(is_green(at(&img, 20, 50)));
    let mut e = Emf::new(100, 100);
    e.stock(5).stock(8).rect(20, 20, 80, 80);
    let img = draw(&e.finish());
    assert_eq!(
        count(&img, |p| p[3] > 0),
        0,
        "null pen and null brush draw nothing"
    );
}

#[test]
fn dashed_pens_carry_a_dash_array_and_a_solid_one_does_not() {
    for (style, dashed) in [(0u32, false), (1, true), (2, true), (3, true), (4, true)] {
        let mut e = Emf::new(100, 100);
        e.pen(1, style, 1, 0).select(1);
        pts32(&mut e, 4, &[(10, 50), (90, 50)]);
        let svg = conv(&e.finish()).svg;
        assert_eq!(svg.contains("stroke-dasharray"), dashed, "style {style}");
    }
}

#[test]
fn a_dashed_line_has_gaps() {
    let mut e = Emf::new(100, 100);
    e.pen(1, 1, 1, 0).select(1);
    pts32(&mut e, 4, &[(0, 50), (100, 50)]);
    let img = draw(&e.finish());
    let row: Vec<bool> = (0..100)
        .map(|x| img.get_pixel(x, 50).0[3] > 40 || img.get_pixel(x, 49).0[3] > 40)
        .collect();
    assert!(row.iter().any(|b| *b) && row.iter().any(|b| !*b), "{row:?}");
}

#[test]
fn ext_create_pen_caps_and_user_style() {
    let mut e = Emf::new(100, 100);
    // PS_GEOMETRIC | PS_ENDCAP_FLAT | PS_JOIN_MITER, width 12.
    let p = words(&[1, 0, 0, 0, 0, 0x10000 | 0x200 | 0x2000, 12, 0, 0, 0, 0]);
    e.raw(95, &p);
    e.select(1);
    pts32(&mut e, 4, &[(20, 50), (80, 50)]);
    let svg = conv(&e.finish()).svg;
    // Flat caps and mitred joins are the SVG defaults: nothing round is written.
    assert!(!svg.contains("round") && !svg.contains("bevel"), "{svg}");
    assert!(svg.contains("stroke-miterlimit"));
    // A round pen says so.
    let mut e2 = Emf::new(100, 100);
    e2.pen(1, 0x10000, 12, 0).select(1);
    pts32(&mut e2, 4, &[(20, 50), (80, 50)]);
    assert!(conv(&e2.finish()).svg.contains("stroke-linecap=\"round\""));
    // User style with dashes.
    let mut e = Emf::new(100, 100);
    let p = words(&[1, 0, 0, 0, 0, 0x10000 | 7, 4, 0, 0, 0, 2, 10, 5]);
    e.raw(95, &p);
    e.select(1);
    pts32(&mut e, 4, &[(20, 50), (80, 50)]);
    let svg = conv(&e.finish()).svg;
    assert!(svg.contains("stroke-dasharray=\"10 5\""), "{svg}");
}

#[test]
fn ext_create_pen_with_a_null_brush_draws_nothing() {
    let mut e = Emf::new(100, 100);
    // lopnStyle PS_GEOMETRIC, brush style BS_NULL.
    e.raw(95, &words(&[1, 0, 0, 0, 0, 0x10000, 6, 1, 0, 0, 0]));
    e.select(1).stock(5);
    pts32(&mut e, 4, &[(20, 50), (80, 50)]);
    assert_eq!(count(&draw(&e.finish()), |p| p[3] > 0), 0);
}

#[test]
fn hatch_brushes_make_a_pattern() {
    for style in 0..6u32 {
        let mut e = Emf::new(100, 100);
        e.brush(1, 2, rgb(0, 0, 255), style).select(1).stock(8);
        e.r(18, &[1]); // transparent background
        e.rect(10, 10, 90, 90);
        let m = conv(&e.finish());
        assert!(m.svg.contains("<pattern"), "style {style}");
        let img = raster_svg(&m.svg, 100);
        let inside = count(&img, is_bluish);
        assert!(
            inside > 20 && inside < 80 * 80 / 2,
            "style {style}: {inside}"
        );
        assert!(is_clear(at(&img, 5, 5)));
    }
}

#[test]
fn an_opaque_background_fills_the_gaps_of_a_hatch() {
    let mut e = Emf::new(100, 100);
    e.brush(1, 2, rgb(0, 0, 255), 0).select(1).stock(8);
    e.r(25, &[rgb(255, 255, 0)]); // bk colour yellow
    e.rect(10, 10, 90, 90);
    let img = draw(&e.finish());
    assert!(
        count(&img, |p| p[3] > 200
            && p[0] > 200
            && p[1] > 200
            && p[2] < 60)
            > 1000
    );
}

#[test]
fn stock_objects() {
    let mut e = Emf::new(100, 100);
    e.stock(4).stock(8).rect(10, 10, 90, 90); // black brush, null pen
    assert!(is_dark(at(&draw(&e.finish()), 50, 50)));
    let mut e = Emf::new(100, 100);
    e.stock(2).stock(8).rect(10, 10, 90, 90); // grey brush
    assert!(is_near(at(&draw(&e.finish()), 50, 50), [128, 128, 128], 2));
    let mut e = Emf::new(100, 100);
    e.stock(1).stock(8).rect(10, 10, 90, 90);
    assert!(is_near(at(&draw(&e.finish()), 50, 50), [192, 192, 192], 2));
    let mut e = Emf::new(100, 100);
    e.stock(3).stock(8).rect(10, 10, 90, 90);
    assert!(is_near(at(&draw(&e.finish()), 50, 50), [64, 64, 64], 2));
}

// ----- the object table ----------------------------------------------------------------------

#[test]
fn objects_are_reused_deleted_and_recreated() {
    let mut e = Emf::new(100, 100);
    e.solid(1, RED).select(1).stock(8);
    e.rect(0, 0, 50, 100);
    e.r(40, &[1]); // delete
    e.select(1); // no longer exists: the red brush stays selected
    e.rect(50, 0, 100, 50);
    e.solid(1, BLUE).select(1); // the index is free again
    e.rect(50, 50, 100, 100);
    let img = draw(&e.finish());
    assert!(is_red(at(&img, 25, 50)));
    assert!(is_red(at(&img, 75, 25)));
    assert!(is_blue(at(&img, 75, 75)));
}

#[test]
fn two_objects_selected_in_turn() {
    let mut e = Emf::new(100, 100);
    e.solid(1, RED).solid(2, GREEN).stock(8);
    e.select(1)
        .rect(0, 0, 50, 100)
        .select(2)
        .rect(50, 0, 100, 100);
    let img = draw(&e.finish());
    assert!(is_red(at(&img, 25, 50)) && is_green(at(&img, 75, 50)));
}

#[test]
fn index_zero_and_stock_range_cannot_be_created() {
    let mut e = Emf::new(100, 100);
    e.solid(0, RED).select(0).stock(8).rect(10, 10, 90, 90);
    // Brush 0 was refused; the default white brush is used.
    assert!(is_near(at(&draw(&e.finish()), 50, 50), [255, 255, 255], 2));
}

// ----- shapes --------------------------------------------------------------------------------

#[test]
fn polygon_triangle() {
    for wide in [true, false] {
        let mut e = Emf::new(100, 100);
        e.fill_with(RED);
        let t = [(50, 10), (90, 90), (10, 90)];
        if wide {
            pts32(&mut e, 3, &t);
        } else {
            pts16(&mut e, 86, &t);
        }
        let img = draw(&e.finish());
        assert!(is_red(at(&img, 50, 60)), "wide {wide}");
        assert!(is_clear(at(&img, 15, 30)));
        assert!(is_clear(at(&img, 85, 30)));
        assert!(is_clear(at(&img, 50, 95)));
    }
}

#[test]
fn polyline_is_not_filled_and_polylineto_continues_from_the_position() {
    let mut e = Emf::new(100, 100);
    e.pen(1, 0, 6, 0).select(1).solid(2, RED).select(2);
    pts32(&mut e, 4, &[(10, 10), (90, 10), (90, 90)]);
    let img = draw(&e.finish());
    assert!(is_dark(at(&img, 50, 10)) && is_dark(at(&img, 90, 50)));
    assert!(
        is_clear(at(&img, 50, 50)),
        "an open polyline is stroke only"
    );

    let mut e = Emf::new(100, 100);
    e.pen(1, 0, 6, 0).select(1);
    e.r(27, &[10, 50]); // MOVETOEX
    pts32(&mut e, 6, &[(50, 50), (90, 50)]); // POLYLINETO
    let img = draw(&e.finish());
    assert!(is_dark(at(&img, 30, 50)) && is_dark(at(&img, 70, 50)));
}

#[test]
fn polyline16_and_polylineto16() {
    let mut e = Emf::new(100, 100);
    e.pen(1, 0, 6, 0).select(1);
    pts16(&mut e, 87, &[(10, 30), (90, 30)]);
    e.r(27, &[10, 70]);
    pts16(&mut e, 89, &[(90, 70)]);
    let img = draw(&e.finish());
    assert!(is_dark(at(&img, 50, 30)) && is_dark(at(&img, 50, 70)));
}

#[test]
fn move_to_and_line_to_make_one_connected_line() {
    let mut e = Emf::new(100, 100);
    e.pen(1, 0, 6, 0).select(1);
    e.r(27, &[10, 10])
        .r(54, &[90, 10])
        .r(54, &[90, 90])
        .r(54, &[10, 90]);
    let img = draw(&e.finish());
    assert!(is_dark(at(&img, 50, 10)) && is_dark(at(&img, 90, 50)) && is_dark(at(&img, 50, 90)));
    assert!(is_clear(at(&img, 10, 50)));
    // The segments are collected into one element.
    let mut e = Emf::new(100, 100);
    e.pen(1, 0, 6, 0).select(1);
    e.r(27, &[10, 10]).r(54, &[90, 10]).r(54, &[90, 90]);
    let svg = conv(&e.finish()).svg;
    assert_eq!(svg.matches("<path").count(), 1, "{svg}");
}

#[test]
fn a_pen_change_splits_a_line_into_two_elements() {
    let mut e = Emf::new(100, 100);
    e.pen(1, 0, 6, 0).pen(2, 0, 6, RED).select(1);
    e.r(27, &[10, 10]).r(54, &[90, 10]);
    e.select(2).r(54, &[90, 90]);
    let img = draw(&e.finish());
    assert!(is_dark(at(&img, 50, 10)));
    assert!(is_red(at(&img, 90, 50)));
}

#[test]
fn polybezier_passes_through_the_curve_midpoint() {
    for ty in [2u32, 85] {
        let mut e = Emf::new(100, 100);
        e.pen(1, 0, 6, 0).select(1);
        let c = [(10, 90), (10, 10), (90, 10), (90, 90)];
        if ty == 2 {
            pts32(&mut e, ty, &c);
        } else {
            pts16(&mut e, ty, &c);
        }
        let img = draw(&e.finish());
        assert!(is_dark(at(&img, 50, 30)), "type {ty}");
        assert!(is_clear(at(&img, 50, 60)));
        assert!(is_clear(at(&img, 50, 90)));
    }
}

#[test]
fn polybezierto_starts_at_the_current_position() {
    let mut e = Emf::new(100, 100);
    e.pen(1, 0, 6, 0).select(1);
    e.r(27, &[10, 90]);
    pts32(&mut e, 5, &[(10, 10), (90, 10), (90, 90)]);
    assert!(is_dark(at(&draw(&e.finish()), 50, 30)));
}

#[test]
fn polypolygon_with_a_hole() {
    for wide in [true, false] {
        let mut e = Emf::new(100, 100);
        e.fill_with(RED);
        let polys = [
            vec![(10, 10), (90, 10), (90, 90), (10, 90)],
            vec![(30, 30), (70, 30), (70, 70), (30, 70)],
        ];
        let total: usize = polys.iter().map(|p| p.len()).sum();
        let mut p = words(&[0, 0, 0, 0, polys.len() as i64, total as i64]);
        for poly in &polys {
            p.extend(words(&[poly.len() as i64]));
        }
        for poly in &polys {
            for (x, y) in poly {
                if wide {
                    p.extend(i32::to_le_bytes(*x));
                    p.extend(i32::to_le_bytes(*y));
                } else {
                    p.extend((*x as i16).to_le_bytes());
                    p.extend((*y as i16).to_le_bytes());
                }
            }
        }
        e.raw(if wide { 8 } else { 91 }, &p);
        let img = draw(&e.finish());
        assert!(is_red(at(&img, 20, 20)), "wide {wide}");
        // Alternate fill: the inner square is a hole.
        assert!(is_clear(at(&img, 50, 50)), "wide {wide}");
    }
}

#[test]
fn winding_fill_mode_fills_the_hole_when_orientation_agrees() {
    let mut e = Emf::new(100, 100);
    e.fill_with(RED);
    e.r(19, &[2]); // WINDING
    let polys = [
        vec![(10, 10), (90, 10), (90, 90), (10, 90)],
        vec![(30, 30), (70, 30), (70, 70), (30, 70)],
    ];
    let mut p = words(&[0, 0, 0, 0, 2, 8, 4, 4]);
    for poly in &polys {
        for (x, y) in poly {
            p.extend(i32::to_le_bytes(*x));
            p.extend(i32::to_le_bytes(*y));
        }
    }
    e.raw(8, &p);
    assert!(is_red(at(&draw(&e.finish()), 50, 50)));
}

#[test]
fn polypolyline_is_stroke_only() {
    let mut e = Emf::new(100, 100);
    e.pen(1, 0, 6, 0).select(1);
    let mut p = words(&[0, 0, 0, 0, 2, 4, 2, 2]);
    for (x, y) in [(10, 20), (90, 20), (10, 80), (90, 80)] {
        p.extend(i32::to_le_bytes(x));
        p.extend(i32::to_le_bytes(y));
    }
    e.raw(7, &p);
    let img = draw(&e.finish());
    assert!(is_dark(at(&img, 50, 20)) && is_dark(at(&img, 50, 80)));
    assert!(is_clear(at(&img, 50, 50)));
}

#[test]
fn ellipse_is_filled_in_the_middle_and_clear_in_the_corners() {
    let mut e = Emf::new(100, 100);
    e.fill_with(RED).r(42, &[10, 20, 90, 80]);
    let img = draw(&e.finish());
    assert!(is_red(at(&img, 50, 50)));
    assert!(is_red(at(&img, 15, 50)));
    assert!(
        is_clear(at(&img, 12, 22)),
        "the box corner is outside the ellipse"
    );
    assert!(is_clear(at(&img, 50, 10)));
    // The extremes touch the box.
    assert!(is_red(at(&img, 50, 22)) && is_red(at(&img, 50, 78)));
}

#[test]
fn round_rect_has_round_corners() {
    let mut e = Emf::new(100, 100);
    e.fill_with(RED).r(44, &[10, 10, 90, 90, 40, 40]);
    let img = draw(&e.finish());
    assert!(is_red(at(&img, 50, 12)) && is_red(at(&img, 12, 50)));
    assert!(is_red(at(&img, 30, 30)));
    assert!(is_clear(at(&img, 11, 11)), "the corner is cut");
    assert!(is_clear(at(&img, 88, 12)));
}

#[test]
fn round_rect_with_huge_corners_is_clamped_to_an_ellipse() {
    let mut e = Emf::new(100, 100);
    e.fill_with(RED).r(44, &[10, 10, 90, 90, 1000, 1000]);
    let img = draw(&e.finish());
    assert!(is_red(at(&img, 50, 50)));
    assert!(is_clear(at(&img, 14, 14)));
}

#[test]
fn rectangle_corners_may_be_given_in_any_order() {
    let mut e = Emf::new(100, 100);
    e.fill_with(RED).r(43, &[80, 80, 20, 20]);
    let img = draw(&e.finish());
    assert!(is_red(at(&img, 50, 50)) && is_clear(at(&img, 10, 10)));
}

// ----- arcs ----------------------------------------------------------------------------------

pub(super) fn arc_rec(e: &mut Emf, ty: u32, s: (i32, i32), en: (i32, i32)) {
    e.r(
        ty,
        &[
            0,
            0,
            100,
            100,
            s.0 as i64,
            s.1 as i64,
            en.0 as i64,
            en.1 as i64,
        ],
    );
}

#[test]
fn pie_default_direction_is_counter_clockwise() {
    // From 3 o'clock to 12 o'clock counter-clockwise: the upper right quadrant.
    let mut e = Emf::new(100, 100);
    e.fill_with(RED);
    arc_rec(&mut e, 47, (100, 50), (50, 0));
    let img = draw(&e.finish());
    assert!(is_red(at(&img, 75, 25)));
    assert!(is_clear(at(&img, 25, 75)) && is_clear(at(&img, 75, 75)) && is_clear(at(&img, 25, 25)));
}

#[test]
fn pie_clockwise_takes_the_other_three_quarters() {
    let mut e = Emf::new(100, 100);
    e.fill_with(RED);
    e.r(57, &[2]); // AD_CLOCKWISE
    arc_rec(&mut e, 47, (100, 50), (50, 0));
    let img = draw(&e.finish());
    assert!(is_red(at(&img, 25, 75)) && is_red(at(&img, 75, 75)) && is_red(at(&img, 25, 25)));
    assert!(is_clear(at(&img, 75, 25)));
}

#[test]
fn pie_start_and_end_are_rays_not_points() {
    // Points far outside the box give the same angles as points on it.
    let mut e = Emf::new(100, 100);
    e.fill_with(RED);
    arc_rec(&mut e, 47, (1000, 50), (50, -1000));
    let img = draw(&e.finish());
    assert!(is_red(at(&img, 75, 25)) && is_clear(at(&img, 25, 75)));
}

#[test]
fn chord_is_the_segment_between_the_arc_and_its_chord() {
    let mut e = Emf::new(100, 100);
    e.fill_with(RED);
    arc_rec(&mut e, 46, (100, 50), (50, 0));
    let img = draw(&e.finish());
    assert!(is_red(at(&img, 82, 18)), "between the arc and the chord");
    assert!(
        is_clear(at(&img, 60, 40)),
        "on the centre side of the chord"
    );
    assert!(is_clear(at(&img, 25, 75)));
}

#[test]
fn arc_is_a_stroke_without_a_fill() {
    let mut e = Emf::new(100, 100);
    e.pen(1, 0, 6, 0).select(1).solid(2, RED).select(2);
    arc_rec(&mut e, 45, (100, 50), (50, 0));
    let img = draw(&e.finish());
    // The arc at 45 degrees.
    assert!(is_dark(at(&img, 85, 15)));
    assert!(is_clear(at(&img, 75, 25)), "no fill");
    assert!(is_clear(at(&img, 15, 85)), "only the quarter is drawn");
}

#[test]
fn arc_to_joins_the_current_position_to_the_arc_start() {
    let mut e = Emf::new(100, 100);
    e.pen(1, 0, 6, 0).select(1);
    e.r(27, &[50, 50]);
    arc_rec(&mut e, 55, (100, 50), (50, 0));
    let img = draw(&e.finish());
    assert!(
        is_dark(at(&img, 75, 50)),
        "the line from the centre to 3 o'clock"
    );
    assert!(is_dark(at(&img, 85, 15)));
}

#[test]
fn a_full_circle_when_start_equals_end() {
    let mut e = Emf::new(100, 100);
    e.pen(1, 0, 6, 0).select(1);
    arc_rec(&mut e, 45, (100, 50), (100, 50));
    let img = draw(&e.finish());
    assert!(is_dark(at(&img, 50, 1)) && is_dark(at(&img, 1, 50)) && is_dark(at(&img, 50, 98)));
}

#[test]
fn angle_arc_draws_a_circle_part_after_a_line() {
    let mut e = Emf::new(100, 100);
    e.pen(1, 0, 6, 0).select(1);
    e.r(27, &[50, 50]);
    // Centre (50,50) radius 40, from 0 degrees sweeping 90 counter-clockwise.
    e.r(41, &[50, 50, 40, fl(0.0), fl(90.0)]);
    let img = draw(&e.finish());
    assert!(is_dark(at(&img, 70, 50)), "line to the arc start");
    // The arc at 45 degrees: (50 + 28.3, 50 - 28.3).
    assert!(is_dark(at(&img, 78, 22)));
    assert!(is_clear(at(&img, 22, 78)));
}

// ----- mapping ------------------------------------------------------------------------------

#[test]
fn anisotropic_window_to_viewport() {
    let mut e = Emf::new(100, 100);
    e.fill_with(RED);
    e.r(17, &[8]).r(9, &[10, 10]).r(11, &[100, 100]);
    e.rect(2, 2, 8, 8);
    let img = draw(&e.finish());
    assert!(is_red(at(&img, 50, 50)) && is_red(at(&img, 22, 22)) && is_red(at(&img, 78, 78)));
    assert!(is_clear(at(&img, 15, 50)) && is_clear(at(&img, 85, 50)));
}

#[test]
fn different_horizontal_and_vertical_scale() {
    let mut e = Emf::new(100, 100);
    e.fill_with(RED);
    e.r(17, &[8]).r(9, &[10, 10]).r(11, &[100, 50]);
    e.rect(0, 0, 10, 10);
    let img = draw(&e.finish());
    assert!(is_red(at(&img, 95, 45)));
    assert!(is_clear(at(&img, 95, 60)), "the picture is half as tall");
}

#[test]
fn window_and_viewport_origins_shift_the_picture() {
    let mut e = Emf::new(100, 100);
    e.fill_with(RED);
    e.r(17, &[8]).r(9, &[100, 100]).r(11, &[100, 100]);
    e.r(10, &[-30, -20]); // window origin
    e.rect(0, 0, 40, 40);
    let img = draw(&e.finish());
    // Logical (0,0) is 30 right and 20 down of the window origin.
    assert!(is_red(at(&img, 50, 40)));
    assert!(is_clear(at(&img, 20, 20)));
    let mut e = Emf::new(100, 100);
    e.fill_with(RED);
    e.r(17, &[8]).r(9, &[100, 100]).r(11, &[100, 100]);
    e.r(12, &[60, 60]); // viewport origin
    e.rect(0, 0, 30, 30);
    let img = draw(&e.finish());
    assert!(is_red(at(&img, 75, 75)) && is_clear(at(&img, 30, 30)));
}

#[test]
fn text_mapping_ignores_the_extents() {
    let mut e = Emf::new(100, 100);
    e.fill_with(RED);
    e.r(17, &[1]).r(9, &[10, 10]).r(11, &[500, 500]);
    e.rect(20, 20, 80, 80);
    let img = draw(&e.finish());
    assert!(is_red(at(&img, 50, 50)) && is_clear(at(&img, 10, 10)));
}

#[test]
fn lometric_has_y_up_and_tenths_of_a_millimetre() {
    let mut e = Emf::new(100, 100);
    e.fill_with(RED);
    e.r(17, &[2]);
    e.rect(100, -100, 200, -200);
    let img = draw(&e.finish());
    assert!(is_red(at(&img, 56, 56)));
    assert!(is_clear(at(&img, 20, 20)) && is_clear(at(&img, 90, 90)));
}

#[test]
fn every_fixed_mapping_mode_scales() {
    // 100 px = 26.46 mm: the unit counts per mode.
    for (mode, size) in [(2, 264), (3, 2646), (4, 104), (5, 1041), (6, 1500)] {
        let mut e = Emf::new(100, 100);
        e.fill_with(RED);
        e.r(17, &[mode]);
        // y up: from the origin downwards.
        e.rect(0, 0, size / 2, -size / 2);
        let img = draw(&e.finish());
        assert!(is_red(at(&img, 25, 25)), "mode {mode}");
        assert!(is_clear(at(&img, 75, 75)), "mode {mode}");
    }
}

#[test]
fn isotropic_keeps_the_smaller_scale() {
    let mut e = Emf::new(100, 100);
    e.fill_with(RED);
    e.r(17, &[7]).r(9, &[10, 10]).r(11, &[100, 40]);
    e.rect(0, 0, 10, 10);
    let img = draw(&e.finish());
    // Scale 4 on both axes: the rectangle is 40 x 40.
    assert!(is_red(at(&img, 35, 35)));
    assert!(is_clear(at(&img, 60, 20)));
}

#[test]
fn scale_extents_records() {
    let mut e = Emf::new(100, 100);
    e.fill_with(RED);
    e.r(17, &[8]).r(9, &[10, 10]).r(11, &[10, 10]);
    e.r(31, &[5, 1, 5, 1]); // viewport x5: 50 x 50 for 10 units
    e.rect(0, 0, 10, 10);
    let img = draw(&e.finish());
    assert!(is_red(at(&img, 45, 45)) && is_clear(at(&img, 60, 60)));
    let mut e = Emf::new(100, 100);
    e.fill_with(RED);
    e.r(17, &[8]).r(9, &[10, 10]).r(11, &[100, 100]);
    e.r(32, &[1, 2, 1, 2]); // window / 2 -> 5: scale doubles
    e.rect(0, 0, 5, 5);
    let img = draw(&e.finish());
    assert!(is_red(at(&img, 90, 90)));
}

// ----- the world transform ------------------------------------------------------------------

#[test]
fn world_transform_translates_and_scales() {
    let mut e = Emf::new(100, 100);
    e.fill_with(RED);
    e.r(
        35,
        &[fl(2.0), fl(0.0), fl(0.0), fl(2.0), fl(10.0), fl(20.0)],
    );
    e.rect(0, 0, 20, 20);
    let img = draw(&e.finish());
    assert!(is_red(at(&img, 30, 40)) && is_red(at(&img, 48, 58)));
    assert!(is_clear(at(&img, 5, 5)) && is_clear(at(&img, 55, 65)));
}

#[test]
fn world_transform_rotates() {
    let mut e = Emf::new(100, 100);
    e.fill_with(RED);
    e.r(
        35,
        &[fl(0.0), fl(1.0), fl(-1.0), fl(0.0), fl(100.0), fl(0.0)],
    );
    e.rect(10, 10, 30, 50);
    let img = draw(&e.finish());
    // x' = 100 - y, y' = x: the rectangle becomes x 50..90, y 10..30.
    assert!(is_red(at(&img, 70, 20)));
    assert!(is_clear(at(&img, 20, 40)));
}

#[test]
fn modify_world_transform_multiplication_order() {
    let scale = [fl(2.0), fl(0.0), fl(0.0), fl(2.0), fl(0.0), fl(0.0)];
    let shift = [fl(1.0), fl(0.0), fl(0.0), fl(1.0), fl(10.0), fl(0.0)];
    for (mode, x_lo, x_hi) in [(2, 20, 40), (3, 10, 30)] {
        let mut e = Emf::new(100, 100);
        e.fill_with(RED);
        e.r(35, &scale);
        let mut p = shift.to_vec();
        p.push(mode);
        e.r(36, &p);
        e.rect(0, 0, 10, 10);
        let img = draw(&e.finish());
        assert!(is_red(at(&img, x_lo + 2, 5)), "mode {mode}");
        assert!(is_red(at(&img, x_hi - 2, 5)), "mode {mode}");
        assert!(is_clear(at(&img, x_lo - 4, 5)), "mode {mode}");
        assert!(is_clear(at(&img, x_hi + 4, 5)), "mode {mode}");
    }
}

#[test]
fn modify_world_transform_identity_and_set() {
    let mut e = Emf::new(100, 100);
    e.fill_with(RED);
    e.r(35, &[fl(3.0), fl(0.0), fl(0.0), fl(3.0), fl(0.0), fl(0.0)]);
    e.r(
        36,
        &[fl(1.0), fl(0.0), fl(0.0), fl(1.0), fl(0.0), fl(0.0), 1],
    ); // identity
    e.rect(0, 0, 20, 20);
    let img = draw(&e.finish());
    assert!(is_red(at(&img, 10, 10)) && is_clear(at(&img, 40, 40)));
    let mut e = Emf::new(100, 100);
    e.fill_with(RED);
    e.r(
        36,
        &[fl(1.0), fl(0.0), fl(0.0), fl(1.0), fl(50.0), fl(50.0), 4],
    ); // set
    e.rect(0, 0, 20, 20);
    assert!(is_red(at(&draw(&e.finish()), 60, 60)));
}

#[test]
fn a_non_finite_or_huge_world_transform_is_ignored() {
    for bad in [f32::NAN, f32::INFINITY, 1.0e30] {
        let mut e = Emf::new(100, 100);
        e.fill_with(RED);
        e.r(35, &[fl(bad), fl(0.0), fl(0.0), fl(1.0), fl(0.0), fl(0.0)]);
        e.rect(20, 20, 80, 80);
        let m = conv(&e.finish());
        assert!(!m.svg.contains("NaN") && !m.svg.contains("inf"), "{bad}");
        raster_svg(&m.svg, 100);
    }
}

// ----- save / restore -----------------------------------------------------------------------

#[test]
fn restore_dc_brings_back_the_brush() {
    let mut e = Emf::new(100, 100);
    e.solid(1, RED).solid(2, BLUE).select(1).stock(8);
    e.r(33, &[]); // SAVEDC
    e.select(2).rect(0, 0, 50, 50);
    e.r(34, &[-1i64]); // RESTOREDC
    e.rect(50, 50, 100, 100);
    let img = draw(&e.finish());
    assert!(is_blue(at(&img, 25, 25)));
    assert!(is_red(at(&img, 75, 75)));
}

#[test]
fn restore_dc_restores_the_mapping_and_the_clip() {
    let mut e = Emf::new(100, 100);
    e.fill_with(RED);
    e.r(33, &[]);
    e.r(17, &[8]).r(9, &[10, 10]).r(11, &[200, 200]);
    e.r(30, &[0, 0, 5, 5]);
    e.r(34, &[-1i64]);
    e.rect(10, 10, 60, 60); // 1:1 again, no clip
    let img = draw(&e.finish());
    assert!(is_red(at(&img, 50, 50)) && is_red(at(&img, 15, 15)));
}

#[test]
fn restore_dc_absolute_and_out_of_range_indexes() {
    let mut e = Emf::new(100, 100);
    e.solid(1, RED).solid(2, BLUE).select(1).stock(8);
    e.r(33, &[]);
    e.select(2);
    e.r(33, &[]);
    e.r(34, &[1]); // absolute: back to the first saved state
    e.rect(0, 0, 100, 100);
    assert!(is_red(at(&draw(&e.finish()), 50, 50)));
    // An index beyond the stack is ignored.
    let mut e = Emf::new(100, 100);
    e.solid(1, RED)
        .select(1)
        .stock(8)
        .r(34, &[-5i64])
        .r(34, &[9]);
    e.rect(0, 0, 100, 100);
    assert!(is_red(at(&draw(&e.finish()), 50, 50)));
}

#[test]
fn the_current_position_survives_restore_dc() {
    let mut e = Emf::new(100, 100);
    e.pen(1, 0, 6, 0).select(1);
    e.r(33, &[]);
    e.r(27, &[10, 50]);
    e.r(34, &[-1i64]);
    e.r(54, &[90, 50]);
    assert!(is_dark(at(&draw(&e.finish()), 50, 50)));
}

// ----- paths ---------------------------------------------------------------------------------

pub(super) fn triangle_path(e: &mut Emf) {
    e.r(59, &[]);
    e.r(27, &[10, 10]).r(54, &[90, 10]).r(54, &[50, 90]);
    e.r(61, &[]);
    e.r(60, &[]);
}

#[test]
fn fill_path() {
    let mut e = Emf::new(100, 100);
    e.fill_with(RED);
    triangle_path(&mut e);
    e.r(62, &[0, 0, 0, 0]);
    let img = draw(&e.finish());
    assert!(is_red(at(&img, 50, 30)));
    assert!(is_clear(at(&img, 15, 80)) && is_clear(at(&img, 85, 80)));
}

#[test]
fn stroke_path_does_not_fill() {
    let mut e = Emf::new(100, 100);
    e.pen(1, 0, 6, 0).select(1).solid(2, RED).select(2);
    triangle_path(&mut e);
    e.r(64, &[0, 0, 0, 0]);
    let img = draw(&e.finish());
    assert!(is_dark(at(&img, 50, 10)));
    assert!(is_clear(at(&img, 50, 40)));
}

#[test]
fn stroke_and_fill_path() {
    let mut e = Emf::new(100, 100);
    e.pen(1, 0, 6, 0).select(1).solid(2, RED).select(2);
    triangle_path(&mut e);
    e.r(63, &[0, 0, 0, 0]);
    let img = draw(&e.finish());
    assert!(is_dark(at(&img, 50, 10)) && is_red(at(&img, 50, 40)));
}

#[test]
fn shapes_inside_a_path_are_part_of_it() {
    let mut e = Emf::new(100, 100);
    e.fill_with(RED);
    e.r(59, &[]);
    e.r(43, &[10, 10, 40, 40]);
    e.r(42, &[60, 60, 90, 90]);
    e.r(60, &[]);
    e.r(62, &[0, 0, 0, 0]);
    let img = draw(&e.finish());
    assert!(is_red(at(&img, 25, 25)) && is_red(at(&img, 75, 75)));
    assert!(is_clear(at(&img, 50, 50)));
    assert_eq!(
        conv(&{
            let mut e = Emf::new(100, 100);
            e.fill_with(RED);
            e.r(59, &[]).r(43, &[10, 10, 40, 40]).r(60, &[]);
            e.finish()
        })
        .svg
        .matches("<path")
        .count(),
        0,
        "nothing is drawn until the path is used"
    );
}

#[test]
fn abort_path_discards_it() {
    let mut e = Emf::new(100, 100);
    e.fill_with(RED);
    triangle_path(&mut e);
    e.r(68, &[]);
    e.r(62, &[0, 0, 0, 0]);
    assert_eq!(count(&draw(&e.finish()), |p| p[3] > 0), 0);
}

#[test]
fn select_clip_path_clips_later_drawing() {
    let mut e = Emf::new(100, 100);
    e.fill_with(RED);
    triangle_path(&mut e);
    e.r(67, &[5]); // RGN_COPY
    e.rect(0, 0, 100, 100);
    let img = draw(&e.finish());
    assert!(is_red(at(&img, 50, 30)));
    assert!(is_clear(at(&img, 15, 80)) && is_clear(at(&img, 85, 50)));
}

#[test]
fn select_clip_path_and_intersects_with_the_previous_clip() {
    let mut e = Emf::new(100, 100);
    e.fill_with(RED);
    e.r(30, &[0, 0, 100, 40]);
    triangle_path(&mut e);
    e.r(67, &[1]); // RGN_AND
    e.rect(0, 0, 100, 100);
    let img = draw(&e.finish());
    assert!(is_red(at(&img, 50, 30)));
    assert!(is_clear(at(&img, 50, 60)), "outside the first clip");
}

// ----- clipping -----------------------------------------------------------------------------

#[test]
fn intersect_clip_rect() {
    let mut e = Emf::new(100, 100);
    e.fill_with(RED);
    e.r(30, &[20, 20, 60, 60]);
    e.rect(0, 0, 100, 100);
    let img = draw(&e.finish());
    assert!(is_red(at(&img, 40, 40)));
    assert!(is_clear(at(&img, 70, 40)) && is_clear(at(&img, 10, 10)));
}

#[test]
fn two_clips_intersect() {
    let mut e = Emf::new(100, 100);
    e.fill_with(RED);
    e.r(30, &[0, 0, 60, 100]).r(30, &[40, 0, 100, 100]);
    e.rect(0, 0, 100, 100);
    let img = draw(&e.finish());
    assert!(is_red(at(&img, 50, 50)));
    assert!(is_clear(at(&img, 20, 50)) && is_clear(at(&img, 80, 50)));
}

#[test]
fn exclude_clip_rect_leaves_a_hole() {
    let mut e = Emf::new(100, 100);
    e.fill_with(RED);
    e.r(29, &[30, 30, 70, 70]);
    e.rect(0, 0, 100, 100);
    let img = draw(&e.finish());
    assert!(is_red(at(&img, 10, 10)) && is_red(at(&img, 90, 50)));
    assert!(is_clear(at(&img, 50, 50)));
}

#[test]
fn ext_select_clip_rgn_sets_and_resets() {
    // A region of two rectangles in device units.
    let mut rgn = words(&[32, 1, 2, 32, 10, 10, 90, 90]);
    rgn.extend(words(&[10, 10, 40, 40, 60, 60, 90, 90]));
    let mut p = words(&[rgn.len() as i64, 5]);
    p.extend(&rgn);
    let mut e = Emf::new(100, 100);
    e.fill_with(RED);
    e.raw(75, &p);
    e.rect(0, 0, 100, 100);
    let img = draw(&e.finish());
    assert!(is_red(at(&img, 25, 25)) && is_red(at(&img, 75, 75)));
    assert!(is_clear(at(&img, 50, 50)) && is_clear(at(&img, 5, 5)));

    // RGN_COPY with no data restores the default clip.
    let mut e = Emf::new(100, 100);
    e.fill_with(RED);
    e.raw(75, &p);
    e.r(75, &[0, 5]);
    e.rect(0, 0, 100, 100);
    let img = draw(&e.finish());
    assert!(is_red(at(&img, 50, 50)) && is_red(at(&img, 5, 5)));
}

#[test]
fn clip_depth_is_bounded_but_drawing_continues() {
    let mut e = Emf::new(100, 100);
    e.fill_with(RED);
    for i in 0..200 {
        e.r(30, &[0, 0, 100 - (i % 2), 100]);
    }
    e.rect(0, 0, 100, 100);
    let m = conv(&e.finish());
    assert!(m.truncated);
    assert!(m.svg.matches("<g clip-path").count() <= 16);
    assert!(is_red(at(&raster_svg(&m.svg, 100), 50, 50)));
}

// ----- ROP2 and modes -----------------------------------------------------------------------

#[test]
fn rop2_nop_draws_nothing() {
    let mut e = Emf::new(100, 100);
    e.fill_with(RED).r(20, &[11]).rect(10, 10, 90, 90);
    assert_eq!(count(&draw(&e.finish()), |p| p[3] > 0), 0);
}

#[test]
fn set_pixel() {
    let mut e = Emf::new(100, 100);
    e.r(15, &[50, 50, GREEN]);
    let img = draw(&e.finish());
    assert!(is_green(at(&img, 50, 50)));
}

// ----- text ---------------------------------------------------------------------------------

fn text_svg(build: impl FnOnce(&mut Emf)) -> String {
    let mut e = Emf::new(200, 100);
    build(&mut e);
    let m = conv(&e.finish());
    raster_svg(&m.svg, 200);
    m.svg
}

#[test]
fn text_is_svg_text_at_the_reference_point() {
    let svg = text_svg(|e| {
        font(e, 1, -20, 0, 400, [0; 3], 0, "Arial");
        e.select(1);
        e.r(24, &[rgb(255, 0, 0)]);
        text_rec(e, true, 30, 60, "Hello", None, 0);
    });
    assert!(svg.contains(">Hello</text>"), "{svg}");
    assert!(
        svg.contains("translate(30 60"),
        "top-left alignment still puts the reference there: {svg}"
    );
    assert!(svg.contains("fill=\"#ff0000\""));
    assert!(svg.contains("font-size=\"20\""));
    assert!(svg.contains("&apos;Arial&apos;"), "{svg}");
}

#[test]
fn text_height_sign_and_weight_italic_underline() {
    let svg = text_svg(|e| {
        font(e, 1, -20, 0, 700, [1, 1, 1], 0, "Times New Roman");
        e.select(1);
        text_rec(e, true, 10, 50, "X", None, 0);
    });
    assert!(svg.contains("font-weight=\"700\""), "{svg}");
    assert!(svg.contains("font-style=\"italic\""));
    assert!(svg.contains("text-decoration=\"underline line-through\""));
    // A positive height is the cell height: the em is smaller.
    let svg = text_svg(|e| {
        font(e, 1, 20, 0, 400, [0; 3], 0, "Arial");
        e.select(1);
        text_rec(e, true, 10, 50, "X", None, 0);
    });
    assert!(svg.contains("font-size=\"18\""), "{svg}");
}

#[test]
fn text_alignment() {
    // TA_CENTER | TA_BASELINE
    let svg = text_svg(|e| {
        font(e, 1, -20, 0, 400, [0; 3], 0, "Arial");
        e.select(1).r(22, &[6 | 24]);
        text_rec(e, true, 100, 50, "Mid", None, 0);
    });
    assert!(svg.contains("text-anchor=\"middle\""), "{svg}");
    assert!(svg.contains("y=\"0\""), "baseline: {svg}");
    let svg = text_svg(|e| {
        font(e, 1, -20, 0, 400, [0; 3], 0, "Arial");
        e.select(1).r(22, &[2 | 8]);
        text_rec(e, true, 100, 50, "End", None, 0);
    });
    assert!(svg.contains("text-anchor=\"end\""), "{svg}");
    // TA_TOP: the baseline is below the reference point by the ascent.
    let svg = text_svg(|e| {
        font(e, 1, -20, 0, 400, [0; 3], 0, "Arial");
        e.select(1);
        text_rec(e, true, 100, 50, "Top", None, 0);
    });
    assert!(svg.contains("y=\"18\""), "{svg}");
}

#[test]
fn text_rotation_from_the_font_and_the_world_transform() {
    let svg = text_svg(|e| {
        font(e, 1, -20, 900, 400, [0; 3], 0, "Arial");
        e.select(1);
        text_rec(e, true, 100, 50, "Up", None, 0);
    });
    assert!(svg.contains("rotate(-90)"), "{svg}");
    let svg = text_svg(|e| {
        font(e, 1, -20, 0, 400, [0; 3], 0, "Arial");
        e.select(1);
        // 90 degrees clockwise on screen.
        e.r(
            35,
            &[fl(0.0), fl(1.0), fl(-1.0), fl(0.0), fl(100.0), fl(0.0)],
        );
        text_rec(e, true, 10, 10, "Down", None, 0);
    });
    assert!(svg.contains("rotate(90)"), "{svg}");
}

#[test]
fn text_with_a_dx_array_positions_each_character() {
    let svg = text_svg(|e| {
        font(e, 1, -20, 0, 400, [0; 3], 0, "Arial");
        e.select(1);
        text_rec(e, true, 10, 50, "abc", Some(&[10, 12, 14]), 0);
    });
    assert!(svg.contains("x=\"0 10 22\""), "{svg}");
}

#[test]
fn right_aligned_text_with_a_dx_array_starts_left_of_the_reference() {
    let svg = text_svg(|e| {
        font(e, 1, -20, 0, 400, [0; 3], 0, "Arial");
        e.select(1).r(22, &[2]);
        text_rec(e, true, 100, 50, "ab", Some(&[10, 10]), 0);
    });
    assert!(svg.contains("x=\"-20 -10\""), "{svg}");
}

#[test]
fn ansi_text_uses_the_charset_of_the_font() {
    // Shift_JIS "あ" is 0x82 0xA0.
    let svg = text_svg(|e| {
        font(e, 1, -20, 0, 400, [0; 3], 128, "MS Gothic");
        e.select(1);
        let mut p = words(&[
            0,
            0,
            0,
            0,
            1,
            fl(1.0),
            fl(1.0),
            10,
            50,
            2,
            76,
            0,
            0,
            0,
            0,
            0,
            0,
        ]);
        p.extend([0x82, 0xA0, 0, 0]);
        e.raw(83, &p);
    });
    assert!(svg.contains(">あ</text>"), "{svg}");
    // Windows-1252 0xE9 is "é".
    let svg = text_svg(|e| {
        font(e, 1, -20, 0, 400, [0; 3], 0, "Arial");
        e.select(1);
        text_rec(e, false, 10, 50, "caf\u{e9}", None, 0);
    });
    assert!(svg.contains("caf") && svg.contains("</text>"));
}

#[test]
fn text_is_escaped_and_control_characters_become_spaces() {
    let svg = text_svg(|e| {
        font(e, 1, -20, 0, 400, [0; 3], 0, "Arial");
        e.select(1);
        text_rec(e, true, 10, 50, "<a&b>\u{1}x", None, 0);
    });
    assert!(svg.contains("&lt;a&amp;b&gt; x</text>"), "{svg}");
}

#[test]
fn surrogate_pairs_are_one_character() {
    let svg = text_svg(|e| {
        font(e, 1, -20, 0, 400, [0; 3], 0, "Arial");
        e.select(1);
        text_rec(e, true, 10, 50, "a\u{1F600}b", Some(&[5, 7, 9, 11]), 0);
    });
    assert!(svg.contains("\u{1F600}"), "{svg}");
    // The pair's two units advance together: a=5, emoji=7+9, b=11.
    assert!(svg.contains("x=\"0 5 21\""), "{svg}");
}

#[test]
fn glyph_index_text_is_skipped() {
    let svg = text_svg(|e| {
        font(e, 1, -20, 0, 400, [0; 3], 0, "Arial");
        e.select(1);
        text_rec(e, true, 10, 50, "abc", None, 0x10);
    });
    assert!(!svg.contains("<text"));
}

#[test]
fn opaque_text_fills_its_rectangle_with_the_background_colour() {
    let mut e = Emf::new(200, 100);
    font(&mut e, 1, -20, 0, 400, [0; 3], 0, "Arial");
    e.select(1).r(25, &[GREEN]);
    let mut p = words(&[
        0,
        0,
        0,
        0,
        1,
        fl(1.0),
        fl(1.0),
        10,
        50,
        1,
        0,
        2,
        0,
        0,
        0,
        0,
        0,
    ]);
    // rcl at word 12..16: 10,20 - 150,80 ; string follows at offset 76.
    let rcl = [10i32, 20, 150, 80];
    for (i, v) in rcl.iter().enumerate() {
        p[48 + i * 4..52 + i * 4].copy_from_slice(&v.to_le_bytes());
    }
    p[40..44].copy_from_slice(&76u32.to_le_bytes());
    p.extend([b'a', 0, 0, 0]);
    e.raw(83, &p);
    let m = conv(&e.finish());
    let img = raster_svg(&m.svg, 200);
    assert!(is_green(at(&img, 100, 70)), "{}", m.svg);
}

#[test]
fn fonts_fall_back_to_a_generic_family() {
    let svg = text_svg(|e| {
        font(e, 1, -20, 0, 400, [0; 3], 0, "Some Unknown Face");
        e.select(1);
        text_rec(e, true, 10, 50, "x", None, 0);
    });
    assert!(svg.contains("sans-serif"), "{svg}");
}

// ----- bitmaps ------------------------------------------------------------------------------

#[test]
fn stretch_dibits_draws_the_picture_the_right_way_up() {
    for top_down in [false, true] {
        let mut e = Emf::new(100, 100);
        let h = if top_down { -2 } else { 2 };
        stretchdib(
            &mut e,
            (0, 0, 100, 100),
            (0, 0, 2, 2),
            &bmi(2, h, 24, 0, &[], &[]),
            &bits24(2, &quad(), top_down),
            0x00CC0020,
        );
        let img = draw(&e.finish());
        assert!(is_red(at(&img, 25, 25)), "top_down {top_down}");
        assert!(is_green(at(&img, 75, 25)));
        assert!(is_blue(at(&img, 25, 75)));
        // The picture is smoothed when enlarged: allow for the neighbours.
        assert!(is_near(at(&img, 75, 75), [255, 255, 255], 40));
    }
}

#[test]
fn stretch_dibits_source_rectangle_counts_from_the_bottom_of_a_bottom_up_dib() {
    // Take only the bottom-left pixel (blue): ySrc = 0 from the bottom, 1 row high.
    let mut e = Emf::new(100, 100);
    stretchdib(
        &mut e,
        (0, 0, 100, 100),
        (0, 0, 1, 1),
        &bmi(2, 2, 24, 0, &[], &[]),
        &bits24(2, &quad(), false),
        0x00CC0020,
    );
    let img = draw(&e.finish());
    assert!(is_blue(at(&img, 50, 50)) && is_blue(at(&img, 5, 5)));
    // For a top-down DIB the origin is the top-left.
    let mut e = Emf::new(100, 100);
    stretchdib(
        &mut e,
        (0, 0, 100, 100),
        (1, 0, 1, 1),
        &bmi(2, -2, 24, 0, &[], &[]),
        &bits24(2, &quad(), true),
        0x00CC0020,
    );
    assert!(is_green(at(&draw(&e.finish()), 50, 50)));
}

#[test]
fn stretch_dibits_negative_destination_extents_mirror() {
    let mut e = Emf::new(100, 100);
    stretchdib(
        &mut e,
        (100, 0, -100, 100),
        (0, 0, 2, 2),
        &bmi(2, 2, 24, 0, &[], &[]),
        &bits24(2, &quad(), false),
        0x00CC0020,
    );
    let img = draw(&e.finish());
    assert!(is_green(at(&img, 25, 25)) && is_red(at(&img, 75, 25)));
}

#[test]
fn stretch_dibits_into_part_of_the_picture() {
    let mut e = Emf::new(100, 100);
    stretchdib(
        &mut e,
        (50, 50, 40, 40),
        (0, 0, 2, 2),
        &bmi(2, 2, 24, 0, &[], &[]),
        &bits24(2, &quad(), false),
        0x00CC0020,
    );
    let img = draw(&e.finish());
    assert!(is_red(at(&img, 60, 60)) && is_clear(at(&img, 20, 20)) && is_clear(at(&img, 95, 95)));
}

#[test]
fn bitblt_and_stretchblt_with_srccopy() {
    for ty in [76u32, 77] {
        let mut e = Emf::new(100, 100);
        bitblt(
            &mut e,
            ty,
            (0, 0, 100, 100),
            (0, 0, 2, 2),
            &bmi(2, 2, 24, 0, &[], &[]),
            &bits24(2, &quad(), false),
            0x00CC0020,
        );
        let img = draw(&e.finish());
        assert!(is_red(at(&img, 25, 25)), "type {ty}");
        assert!(is_blue(at(&img, 25, 75)));
    }
}

#[test]
fn bitblt_without_a_bitmap_uses_the_raster_op() {
    let blt = |e: &mut Emf, rop: u32| {
        bitblt(e, 76, (10, 10, 80, 80), (0, 0, 0, 0), &[], &[], rop);
    };
    let mut e = Emf::new(100, 100);
    e.fill_with(RED);
    blt(&mut e, 0x00F00021); // PATCOPY
    assert!(is_red(at(&draw(&e.finish()), 50, 50)));
    let mut e = Emf::new(100, 100);
    blt(&mut e, 0x00000042); // BLACKNESS
    assert!(is_dark(at(&draw(&e.finish()), 50, 50)));
    let mut e = Emf::new(100, 100);
    blt(&mut e, 0x00FF0062); // WHITENESS
    assert!(is_near(at(&draw(&e.finish()), 50, 50), [255, 255, 255], 2));
    let mut e = Emf::new(100, 100);
    blt(&mut e, 0x00550009); // DSTINVERT: not drawn
    assert_eq!(count(&draw(&e.finish()), |p| p[3] > 0), 0);
}

#[test]
fn source_and_uses_multiply_and_notsrccopy_inverts() {
    let mut e = Emf::new(100, 100);
    bitblt(
        &mut e,
        76,
        (0, 0, 100, 100),
        (0, 0, 2, 2),
        &bmi(2, 2, 24, 0, &[], &[]),
        &bits24(2, &quad(), false),
        0x008800C6,
    );
    assert!(conv(&e.finish()).svg.contains("mix-blend-mode:multiply"));
    let mut e = Emf::new(100, 100);
    bitblt(
        &mut e,
        76,
        (0, 0, 100, 100),
        (0, 0, 2, 2),
        &bmi(2, 2, 24, 0, &[], &[]),
        &bits24(2, &quad(), false),
        0x00EE0086,
    );
    assert!(conv(&e.finish()).svg.contains("mix-blend-mode:screen"));
    let mut e = Emf::new(100, 100);
    bitblt(
        &mut e,
        76,
        (0, 0, 100, 100),
        (0, 0, 2, 2),
        &bmi(2, 2, 24, 0, &[], &[]),
        &bits24(2, &quad(), false),
        0x00330008,
    );
    let img = draw(&e.finish());
    // Red inverted is cyan.
    assert!(is_near(at(&img, 25, 25), [0, 255, 255], 40));
}

#[test]
fn set_dibits_to_device() {
    let mut e = Emf::new(100, 100);
    let bmi = bmi(2, 2, 24, 0, &[], &[]);
    let bits = bits24(2, &quad(), false);
    let bmi_pad = bmi.len().div_ceil(4) * 4;
    let mut p = words(&[
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        2,
        2,
        76,
        bmi.len() as i64,
        (76 + bmi_pad) as i64,
        bits.len() as i64,
        0,
        0,
        2,
    ]);
    p.extend(&bmi);
    p.resize(68 + bmi_pad, 0);
    p.extend(&bits);
    // The 2 x 2 picture covers 2 x 2 logical units: scale them up to the whole picture.
    e.r(17, &[8]).r(9, &[2, 2]).r(11, &[100, 100]);
    e.raw(80, &p);
    let img = draw(&e.finish());
    assert!(is_red(at(&img, 25, 25)) && is_green(at(&img, 75, 25)));
    assert!(is_blue(at(&img, 25, 75)));
}

#[test]
fn alpha_blend_applies_the_constant_alpha() {
    let mut e = Emf::new(100, 100);
    e.fill_with(rgb(255, 255, 255)).rect(0, 0, 100, 100);
    let bmi = bmi(1, 1, 24, 0, &[], &[]);
    let bits = bits24(1, &[vec![[255, 0, 0]]], false);
    let mut p = words(&[
        0,
        0,
        0,
        0,
        0,
        0,
        100,
        100,
        // BLENDFUNCTION: AC_SRC_OVER, 0, alpha 128, format 0.
        (128 << 16) as i64,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        108,
        bmi.len() as i64,
        (108 + bmi.len()) as i64,
        bits.len() as i64,
        1,
        1,
    ]);
    p.extend(&bmi);
    p.extend(&bits);
    e.raw(114, &p);
    let img = draw(&e.finish());
    let c = at(&img, 50, 50);
    assert!(
        c[0] > 240 && c[1] > 100 && c[1] < 160 && c[2] > 100 && c[2] < 160,
        "{c:?}"
    );
}

#[test]
fn transparent_blt_removes_the_key_colour() {
    let mut e = Emf::new(100, 100);
    let bmi = bmi(2, 2, 24, 0, &[], &[]);
    let bits = bits24(2, &quad(), false);
    let mut p = words(&[
        0,
        0,
        0,
        0,
        0,
        0,
        100,
        100,
        GREEN,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        108,
        bmi.len() as i64,
        (108 + bmi.len()) as i64,
        bits.len() as i64,
        2,
        2,
    ]);
    p.extend(&bmi);
    p.extend(&bits);
    e.raw(116, &p);
    let img = draw(&e.finish());
    assert!(is_red(at(&img, 25, 25)));
    assert!(at(&img, 92, 8)[3] < 100, "the key colour is transparent");
}

#[test]
fn pattern_brush_is_the_average_colour() {
    let mut e = Emf::new(100, 100);
    let bmi = bmi(2, 2, 24, 0, &[], &[]);
    let bits = bits24(
        2,
        &[vec![[200, 0, 0], [200, 0, 0]], vec![[0, 0, 0], [0, 0, 0]]],
        false,
    );
    let mut p = words(&[
        1,
        0,
        32,
        bmi.len() as i64,
        32 + bmi.len() as i64,
        bits.len() as i64,
    ]);
    p.extend(&bmi);
    p.extend(&bits);
    e.raw(94, &p);
    e.select(1).stock(8).rect(0, 0, 100, 100);
    let c = at(&draw(&e.finish()), 50, 50);
    assert!(is_near(c, [100, 0, 0], 3), "{c:?}");
}

#[test]
fn an_undecodable_pattern_brush_is_mid_grey() {
    let mut e = Emf::new(100, 100);
    e.r(94, &[1, 0, 24, 4, 28, 0, 1, 2, 3, 4]);
    e.select(1).stock(8).rect(0, 0, 100, 100);
    assert!(is_near(at(&draw(&e.finish()), 50, 50), [128, 128, 128], 2));
}

#[test]
fn an_embedded_png_is_passed_through() {
    let mut png = Vec::new();
    let img = image::RgbaImage::from_pixel(4, 4, image::Rgba([0, 0, 255, 255]));
    img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
    let mut b = bmi(4, 4, 0, 5, &[], &[]);
    b.truncate(40);
    let mut e = Emf::new(100, 100);
    stretchdib(&mut e, (0, 0, 100, 100), (0, 0, 4, 4), &b, &png, 0x00CC0020);
    let m = conv(&e.finish());
    assert!(m.svg.contains("data:image/png;base64,"));
    assert!(is_blue(at(&raster_svg(&m.svg, 100), 50, 50)));
}

// ----- DIB formats --------------------------------------------------------------------------

fn dec(bmi: &[u8], bits: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    let mut budget = 1 << 24;
    let b = super::dib::decode(bmi, bits, &mut budget, super::dib::Opts::default())?;
    match b.data {
        super::dib::BitmapData::Rgba(d) => Some((b.w, b.h, d)),
        _ => None,
    }
}

fn px_at(d: &(u32, u32, Vec<u8>), x: usize, y: usize) -> [u8; 4] {
    let o = (y * d.0 as usize + x) * 4;
    [d.2[o], d.2[o + 1], d.2[o + 2], d.2[o + 3]]
}

#[test]
fn dib_1bpp_with_a_palette() {
    // 8 x 2, bottom row first: 10101010 then 01010101.
    let pal = [[255, 0, 0], [0, 0, 255]];
    let bits = [0x55, 0, 0, 0, 0xAA, 0, 0, 0];
    let d = dec(&bmi(8, 2, 1, 0, &pal, &[]), &bits).unwrap();
    // Top row = the second stored row 0xAA = 1010..: blue, red, blue ...
    assert_eq!(px_at(&d, 0, 0), [0, 0, 255, 255]);
    assert_eq!(px_at(&d, 1, 0), [255, 0, 0, 255]);
    assert_eq!(px_at(&d, 0, 1), [255, 0, 0, 255]);
}

#[test]
fn dib_4bpp_and_8bpp_palettes() {
    let pal4: Vec<[u8; 3]> = (0..16).map(|i| [i * 16, 0, 255 - i * 16]).collect();
    let d = dec(&bmi(2, 1, 4, 0, &pal4, &[]), &[0x1F, 0, 0, 0]).unwrap();
    assert_eq!(px_at(&d, 0, 0), [16, 0, 239, 255]);
    assert_eq!(px_at(&d, 1, 0), [240, 0, 15, 255]);
    let pal8: Vec<[u8; 3]> = (0..256).map(|i| [i as u8, 0, 0]).collect();
    let d = dec(&bmi(3, 1, 8, 0, &pal8, &[]), &[10, 20, 30, 0]).unwrap();
    assert_eq!(px_at(&d, 2, 0), [30, 0, 0, 255]);
}

#[test]
fn dib_palette_index_beyond_the_table_is_black() {
    let pal = [[255, 0, 0]];
    let d = dec(&bmi(2, 1, 8, 0, &pal, &[]), &[0, 7, 0, 0]).unwrap();
    assert_eq!(px_at(&d, 1, 0), [0, 0, 0, 255]);
}

#[test]
fn dib_16bpp_555_and_565() {
    // 555: red = 0x7C00.
    let d = dec(&bmi(1, 1, 16, 0, &[], &[]), &[0x00, 0x7C, 0, 0]).unwrap();
    assert_eq!(px_at(&d, 0, 0), [255, 0, 0, 255]);
    // 565 via BI_BITFIELDS: green = 0x07E0.
    let d = dec(
        &bmi(1, 1, 16, 3, &[], &[0xF800, 0x07E0, 0x001F]),
        &[0xE0, 0x07, 0, 0],
    )
    .unwrap();
    assert_eq!(px_at(&d, 0, 0), [0, 255, 0, 255]);
}

#[test]
fn dib_32bpp_ignores_the_unused_byte_and_bitfields_work() {
    let d = dec(&bmi(1, 1, 32, 0, &[], &[]), &[10, 20, 30, 77]).unwrap();
    assert_eq!(px_at(&d, 0, 0), [30, 20, 10, 255]);
    // BI_BITFIELDS 32 bpp with RGB masks swapped.
    let d = dec(
        &bmi(1, 1, 32, 3, &[], &[0xFF, 0xFF00, 0xFF0000]),
        &[10, 20, 30, 0],
    )
    .unwrap();
    assert_eq!(px_at(&d, 0, 0), [10, 20, 30, 255]);
}

#[test]
fn dib_32bpp_premultiplied_alpha_is_undone_on_request() {
    let mut budget = 1 << 20;
    let b = super::dib::decode(
        &bmi(1, 1, 32, 0, &[], &[]),
        &[64, 0, 0, 128],
        &mut budget,
        super::dib::Opts {
            alpha: true,
            ..Default::default()
        },
    )
    .unwrap();
    let super::dib::BitmapData::Rgba(d) = b.data else {
        panic!()
    };
    assert_eq!(d[3], 128);
    assert!((d[2] as i32 - 127).abs() <= 1, "{d:?}");
}

#[test]
fn dib_rle8() {
    // 4 x 2 bottom-up: row 0 (bottom): run of 4 index-1 ; EOL ; row 1: absolute 3 pixels, EOF.
    let pal = [[0, 0, 0], [255, 0, 0], [0, 255, 0], [0, 0, 255]];
    let bits = [4, 1, 0, 0, 0, 3, 1, 2, 3, 0, 0, 1];
    let b = bmi(4, 2, 8, 1, &pal, &[]);
    let d = dec(&b, &bits).unwrap();
    assert_eq!(px_at(&d, 0, 1), [255, 0, 0, 255], "bottom row");
    assert_eq!(px_at(&d, 3, 1), [255, 0, 0, 255]);
    assert_eq!(px_at(&d, 0, 0), [255, 0, 0, 255]);
    assert_eq!(px_at(&d, 1, 0), [0, 255, 0, 255]);
    assert_eq!(px_at(&d, 2, 0), [0, 0, 255, 255]);
    assert_eq!(px_at(&d, 3, 0)[3], 0, "never written: transparent");
}

#[test]
fn dib_rle4_and_delta() {
    let pal: Vec<[u8; 3]> = vec![[0, 0, 0], [255, 0, 0], [0, 255, 0]];
    // 4 x 1: encoded run of 4 pixels alternating 1,2 ; then EOF.
    let d = dec(&bmi(4, 1, 4, 2, &pal, &[]), &[4, 0x12, 0, 1]).unwrap();
    assert_eq!(px_at(&d, 0, 0), [255, 0, 0, 255]);
    assert_eq!(px_at(&d, 1, 0), [0, 255, 0, 255]);
    assert_eq!(px_at(&d, 3, 0), [0, 255, 0, 255]);
    // A delta skips pixels.
    let pal8 = [[0, 0, 0], [255, 0, 0]];
    let d = dec(&bmi(4, 1, 8, 1, &pal8, &[]), &[0, 2, 2, 0, 1, 1, 0, 1]).unwrap();
    assert_eq!(px_at(&d, 0, 0)[3], 0);
    assert_eq!(px_at(&d, 2, 0), [255, 0, 0, 255]);
}

#[test]
fn dib_core_header() {
    // BITMAPCOREHEADER: 12 bytes, 24 bpp.
    let mut b = words(&[12]);
    b.extend(1u16.to_le_bytes());
    b.extend(1u16.to_le_bytes());
    b.extend(1u16.to_le_bytes());
    b.extend(24u16.to_le_bytes());
    let d = dec(&b, &[1, 2, 3, 0]).unwrap();
    assert_eq!(px_at(&d, 0, 0), [3, 2, 1, 255]);
}

#[test]
fn dib_refuses_nonsense() {
    assert!(dec(&[], &[]).is_none());
    assert!(dec(&bmi(0, 5, 24, 0, &[], &[]), &[0; 64]).is_none());
    assert!(dec(&bmi(5, 0, 24, 0, &[], &[]), &[0; 64]).is_none());
    assert!(
        dec(&bmi(4, 4, 24, 0, &[], &[]), &[0; 10]).is_none(),
        "bits too short"
    );
    assert!(
        dec(&bmi(4, 4, 7, 0, &[], &[]), &[0; 64]).is_none(),
        "bad depth"
    );
    assert!(
        dec(&bmi(4, 4, 24, 99, &[], &[]), &[0; 64]).is_none(),
        "bad compression"
    );
    assert!(dec(&bmi(i32::MAX, i32::MAX, 24, 0, &[], &[]), &[0; 64]).is_none());
    assert!(
        dec(&bmi(100_000, 1, 24, 0, &[], &[]), &[0; 64]).is_none(),
        "side limit"
    );
    assert!(
        dec(&bmi(5000, 5000, 24, 0, &[], &[]), &[0; 64]).is_none(),
        "pixel limit"
    );
    assert!(dec(&bmi(1, i32::MIN, 24, 0, &[], &[]), &[0; 64]).is_none());
}

#[test]
fn dib_budget_is_charged_and_enforced() {
    use super::dib::ATTEMPT_PIXELS;
    let b = bmi(10, 10, 24, 0, &[], &[]);
    let bits = vec![0u8; 10 * 32];
    // A success costs its pixels.
    let mut budget = ATTEMPT_PIXELS + 150;
    assert!(super::dib::decode(&b, &bits, &mut budget, Default::default()).is_some());
    assert_eq!(budget, ATTEMPT_PIXELS + 50);
    // Another success, then too little is left to make an attempt at all.
    let mut budget = 50 + ATTEMPT_PIXELS;
    assert!(super::dib::decode(&b, &bits, &mut budget, Default::default()).is_some());
    assert_eq!(budget, ATTEMPT_PIXELS - 50);
    assert!(super::dib::decode(&b, &bits, &mut budget, Default::default()).is_none());
    assert_eq!(
        budget,
        ATTEMPT_PIXELS - 50,
        "below the price of an attempt: refused, free"
    );
    // A bitmap bigger than the allowance is refused and pays for the attempt.
    let big = bmi(100, 100, 24, 0, &[], &[]);
    let bits = vec![0u8; 100 * 300];
    let mut budget = ATTEMPT_PIXELS + 99;
    assert!(super::dib::decode(&big, &bits, &mut budget, Default::default()).is_none());
    assert_eq!(budget, 99, "a refused bitmap pays for the attempt");
}

#[test]
fn failed_attempts_drain_the_budget_and_cost_no_allocation() {
    use super::dib::ATTEMPT_PIXELS;
    // A header that promises 16 Mpx over 64 bytes of data: refused before the 64 MiB RGBA buffer
    // exists, and each refusal is charged, so the allowance ends the storm.
    let b = bmi(4096, 4096, 24, 0, &[], &[]);
    let bits = [0u8; 64];
    let mut budget = super::gdi::PIXEL_BUDGET;
    let mut charged = 0u64;
    let t = std::time::Instant::now();
    loop {
        let before = budget;
        assert!(super::dib::decode(&b, &bits, &mut budget, Default::default()).is_none());
        if budget == before {
            break;
        }
        charged += 1;
        assert!(charged <= 100_000, "the storm never ended");
    }
    assert_eq!(charged, super::gdi::PIXEL_BUDGET / ATTEMPT_PIXELS);
    assert!(
        t.elapsed() < std::time::Duration::from_secs(2),
        "{:?}",
        t.elapsed()
    );
    // the same for run-length data and for 32-bit data
    for (bpp, comp) in [(8, 1), (4, 2), (32, 0), (1, 0)] {
        let b = bmi(4096, 4096, bpp, comp, &[], &[]);
        let mut budget = ATTEMPT_PIXELS * 3;
        for _ in 0..3 {
            assert!(super::dib::decode(&b, &bits, &mut budget, Default::default()).is_none());
        }
        assert_eq!(budget, 0, "bpp {bpp} comp {comp}");
    }
}

#[test]
fn run_length_data_must_be_able_to_fill_the_bitmap() {
    // 32 x 32 pixels over 2 bytes cannot be real run-length data (a run is 255 pixels at most per
    // two bytes), so it is refused without allocating; honest data of the same size decodes.
    let b = bmi(32, 32, 8, 1, &[], &[]);
    let mut budget = 1 << 20;
    assert!(super::dib::decode(&b, &[0, 0], &mut budget, Default::default()).is_none());
    // 16 runs of 64 pixels, each followed by an end-of-line marker
    let mut honest = Vec::new();
    for _ in 0..32 {
        honest.extend_from_slice(&[32, 7, 0, 0]);
    }
    honest.extend_from_slice(&[0, 1]);
    let mut budget = 1 << 20;
    assert!(super::dib::decode(&b, &honest, &mut budget, Default::default()).is_some());
}

#[test]
fn many_bitmaps_hit_the_image_count_limit() {
    let mut e = Emf::new(100, 100);
    for _ in 0..5000 {
        stretchdib(
            &mut e,
            (0, 0, 10, 10),
            (0, 0, 1, 1),
            &bmi(1, 1, 24, 0, &[], &[]),
            &bits24(1, &[vec![[255, 0, 0]]], false),
            0x00CC0020,
        );
    }
    let m = conv(&e.finish());
    assert!(m.truncated);
    assert!(m.svg.matches("<image").count() <= 4096);
}

// ----- gradient fills -----------------------------------------------------------------------

pub(super) fn gradient_rec(
    e: &mut Emf,
    mode: u32,
    verts: &[(i32, i32, [u16; 3])],
    elems: &[Vec<u32>],
) {
    let mut p = words(&[
        0,
        0,
        0,
        0,
        verts.len() as i64,
        elems.len() as i64,
        mode as i64,
    ]);
    for (x, y, c) in verts {
        p.extend(i32::to_le_bytes(*x));
        p.extend(i32::to_le_bytes(*y));
        for v in c {
            p.extend(v.to_le_bytes());
        }
        p.extend(0u16.to_le_bytes());
    }
    for el in elems {
        for i in el {
            p.extend(i.to_le_bytes());
        }
    }
    e.raw(118, &p);
}

#[test]
fn horizontal_gradient_rect() {
    let mut e = Emf::new(100, 100);
    gradient_rec(
        &mut e,
        0,
        &[(0, 0, [0xFF00, 0, 0]), (100, 100, [0, 0, 0xFF00])],
        &[vec![0, 1]],
    );
    let img = draw(&e.finish());
    let (l, r) = (at(&img, 5, 50), at(&img, 95, 50));
    assert!(l[0] > 220 && l[2] < 40, "{l:?}");
    assert!(r[2] > 220 && r[0] < 40, "{r:?}");
    let m = at(&img, 50, 50);
    assert!(m[0] > 90 && m[2] > 90, "{m:?}");
}

#[test]
fn vertical_gradient_rect() {
    let mut e = Emf::new(100, 100);
    gradient_rec(
        &mut e,
        1,
        &[(0, 0, [0xFF00, 0, 0]), (100, 100, [0, 0xFF00, 0])],
        &[vec![0, 1]],
    );
    let img = draw(&e.finish());
    assert!(at(&img, 50, 5)[0] > 220 && at(&img, 50, 95)[1] > 220);
}

#[test]
fn triangle_gradient_is_flat() {
    let mut e = Emf::new(100, 100);
    gradient_rec(
        &mut e,
        2,
        &[
            (10, 10, [0xFF00, 0, 0]),
            (90, 10, [0xFF00, 0, 0]),
            (50, 90, [0xFF00, 0, 0]),
        ],
        &[vec![0, 1, 2]],
    );
    assert!(is_red(at(&draw(&e.finish()), 50, 30)));
}

#[test]
fn gradient_vertex_index_out_of_range_is_dropped() {
    let mut e = Emf::new(100, 100);
    gradient_rec(&mut e, 0, &[(0, 0, [0; 3])], &[vec![0, 7]]);
    conv(&e.finish());
}

// ----- regions ------------------------------------------------------------------------------

#[test]
fn paint_rgn_and_fill_rgn() {
    let mut rgn = words(&[32, 1, 1, 16, 10, 10, 90, 90]);
    rgn.extend(words(&[20, 20, 60, 60]));
    let mut p = words(&[0, 0, 0, 0, rgn.len() as i64]);
    p.extend(&rgn);
    let mut e = Emf::new(100, 100);
    e.fill_with(RED);
    e.raw(74, &p);
    assert!(is_red(at(&draw(&e.finish()), 40, 40)));

    let mut p = words(&[0, 0, 0, 0, rgn.len() as i64, 2]);
    p.extend(&rgn);
    let mut e = Emf::new(100, 100);
    e.solid(2, BLUE);
    e.raw(71, &p);
    assert!(is_blue(at(&draw(&e.finish()), 40, 40)));
}

// ----- EMF+ ---------------------------------------------------------------------------------

fn plus_comment(e: &mut Emf) {
    let mut c = words(&[12]);
    c.extend(b"EMF+\0\0\0\0");
    e.raw(70, &c);
}

#[test]
fn an_emf_plus_only_file_is_refused() {
    let mut e = Emf::new(100, 100);
    plus_comment(&mut e);
    e.r(17, &[8]);
    assert!(to_svg(&e.finish()).is_none());
}

#[test]
fn a_dual_emf_plus_file_uses_its_gdi_records() {
    let mut e = Emf::new(100, 100);
    plus_comment(&mut e);
    e.fill_with(RED).rect(10, 10, 90, 90);
    plus_comment(&mut e);
    let img = draw(&e.finish());
    assert!(is_red(at(&img, 50, 50)));
}

#[test]
fn other_comments_are_ignored() {
    let mut e = Emf::new(100, 100);
    let mut c = words(&[8]);
    c.extend(b"GDIC\0\0\0\0");
    e.raw(70, &c);
    e.fill_with(RED).rect(10, 10, 90, 90);
    assert!(is_red(at(&draw(&e.finish()), 50, 50)));
}

#[test]
fn unknown_records_are_skipped() {
    let mut e = Emf::new(100, 100);
    e.r(999, &[1, 2, 3]);
    e.r(104, &[1, 2, 3]);
    e.fill_with(RED).rect(10, 10, 90, 90);
    let m = conv(&e.finish());
    assert!(!m.truncated);
    assert!(is_red(at(&raster_svg(&m.svg, 100), 50, 50)));
}

#[test]
fn drawing_stops_at_the_eof_record() {
    let mut e = Emf::new(100, 100);
    e.fill_with(RED).rect(0, 0, 50, 100);
    e.r(14, &[0, 0, 0]);
    e.rect(50, 0, 100, 100);
    let img = draw(&e.finish());
    assert!(is_red(at(&img, 25, 50)) && is_clear(at(&img, 75, 50)));
}

#[test]
fn clip_is_closed_properly_for_each_clipped_group() {
    let mut e = Emf::new(100, 100);
    e.fill_with(RED);
    e.r(33, &[])
        .r(30, &[0, 0, 50, 50])
        .rect(0, 0, 100, 100)
        .r(34, &[-1i64]);
    e.rect(60, 60, 90, 90);
    let m = conv(&e.finish());
    // The SVG parses as XML with balanced tags.
    let doc = resvg::usvg::roxmltree::Document::parse(&m.svg).expect("well-formed");
    assert!(doc.root_element().has_tag_name("svg"));
}

#[test]
fn a_logical_palette_colours_pal_colors_dibs() {
    let mut e = Emf::new(100, 100);
    // CREATEPALETTE 1: version 0x300, two entries.
    let mut p = words(&[1]);
    p.extend([0, 3, 2, 0, 0, 255, 0, 0, 255, 0, 255, 0]);
    e.raw(49, &p);
    e.r(48, &[1]);
    // 2 x 1, 8 bpp, DIB_PAL_COLORS: the table holds the indices 1 and 0.
    let mut hdr = bmi(2, 1, 8, 0, &[], &[]);
    hdr[32..36].copy_from_slice(&2u32.to_le_bytes());
    hdr.extend([1, 0, 0, 0]);
    let off_bmi = 80;
    let mut rec = words(&[
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        2,
        1,
        off_bmi,
        hdr.len() as i64,
        (off_bmi + hdr.len() as i64),
        4,
        1,
        0x00CC0020,
        100,
        100,
    ]);
    rec.extend(&hdr);
    rec.resize(72 + hdr.len(), 0);
    rec.extend([0, 1, 0, 0]);
    e.raw(81, &rec);
    let img = draw(&e.finish());
    // Entries are R G B flags: index 0 green, index 1 magenta; the DIB's table maps pixel 0 to
    // index 1 and pixel 1 to index 0.
    assert!(
        is_near(at(&img, 15, 50), [255, 0, 255], 60),
        "{:?}",
        at(&img, 15, 50)
    );
    assert!(
        is_near(at(&img, 85, 50), [0, 255, 0], 60),
        "{:?}",
        at(&img, 85, 50)
    );
}

#[test]
fn an_opaque_rectangle_is_filled_even_without_text() {
    for glyph_index in [false, true] {
        let mut e = Emf::new(100, 100);
        e.r(25, &[GREEN]);
        let opts = if glyph_index { 0x12 } else { 0x2 };
        let mut p = words(&[
            0,
            0,
            0,
            0,
            1,
            fl(1.0),
            fl(1.0),
            10,
            50,
            0,
            76,
            opts,
            10,
            20,
            90,
            60,
            0,
        ]);
        p.extend([0, 0, 0, 0]);
        e.raw(84, &p);
        assert!(
            is_green(at(&draw(&e.finish()), 50, 40)),
            "glyph index {glyph_index}"
        );
    }
}
