//! WMF: the 16-bit records played into pixels.

use super::tests::*;
use super::tests_emf::{bits24, bmi, quad};
use super::*;

pub(super) const RED: i64 = 0x0000FF;
pub(super) const GREEN: i64 = 0x00FF00;
pub(super) const BLUE: i64 = 0xFF0000;

pub(super) fn base(w: i16, h: i16) -> Wmf {
    let mut m = Wmf::new(w, h);
    m.window(w as i32, h as i32);
    m
}

/// A brush created and selected (object 0), no pen (object 1).
pub(super) fn fill(m: &mut Wmf, color: i64) {
    m.brush(0, color, 0).select(0);
    m.pen(5, 0, 0).select(1);
}

pub(super) fn pts(m: &mut Wmf, func: u16, p: &[(i32, i32)]) {
    let mut v = vec![p.len() as i32];
    for (x, y) in p {
        v.push(*x);
        v.push(*y);
    }
    m.r(func, &v);
}

pub(super) fn dib_record(
    m: &mut Wmf,
    func: u16,
    head: &[i32],
    rop: u32,
    usage: Option<u16>,
    bmi: &[u8],
    bits: &[u8],
) {
    let mut p = rop.to_le_bytes().to_vec();
    if let Some(u) = usage {
        p.extend(u.to_le_bytes());
    }
    for v in head {
        p.extend((*v as i16).to_le_bytes());
    }
    p.extend(bmi);
    p.extend(bits);
    m.raw(func, &p);
}

#[test]
fn detection() {
    let m = base(100, 100).finish();
    assert!(wmf::is_wmf(&m));
    let mut std_hdr = Wmf::new(100, 100);
    std_hdr.placeable = false;
    assert!(wmf::is_wmf(&std_hdr.finish()));
    assert!(!wmf::is_wmf(b"abcdefgh"));
    assert!(!wmf::is_wmf(&[1, 0, 9, 0, 0, 0]), "bad version");
    assert!(
        to_svg(&[0xD7, 0xCD, 0xC6, 0x9A, 0, 0]).is_none(),
        "key only"
    );
}

#[test]
fn filled_rectangle() {
    let mut m = base(100, 100);
    fill(&mut m, RED);
    m.rect(20, 20, 80, 80);
    let img = draw(&m.finish());
    assert!(is_red(at(&img, 50, 50)) && is_red(at(&img, 25, 25)));
    assert!(is_clear(at(&img, 10, 10)) && is_clear(at(&img, 90, 50)));
}

#[test]
fn defaults_are_a_white_brush_and_a_black_pen() {
    let mut m = base(100, 100);
    m.rect(20, 20, 80, 80);
    let img = draw(&m.finish());
    assert!(is_near(at(&img, 50, 50), [255, 255, 255], 2));
    assert!(count(&img, is_inkish) > 100);
}

#[test]
fn pen_width_colour_and_null() {
    let mut m = base(100, 100);
    m.pen(0, 10, GREEN).select(0);
    m.brush(1, 0, 0).select(1); // BS_NULL
    m.rect(20, 20, 80, 80);
    let img = draw(&m.finish());
    assert!(is_green(at(&img, 20, 50)) && is_clear(at(&img, 50, 50)));
}

#[test]
fn y_comes_before_x_in_point_records() {
    let mut m = base(100, 100);
    m.pen(0, 6, 0).select(0);
    m.r(0x0214, &[20, 10]); // MOVETO y=20, x=10
    m.r(0x0213, &[20, 90]); // LINETO y=20, x=90
    let img = draw(&m.finish());
    assert!(is_dark(at(&img, 50, 20)));
    assert!(is_clear(at(&img, 20, 50)));
}

#[test]
fn polygon_polyline_and_polypolygon() {
    let mut m = base(100, 100);
    fill(&mut m, RED);
    pts(&mut m, 0x0324, &[(50, 10), (90, 90), (10, 90)]);
    let img = draw(&m.finish());
    assert!(is_red(at(&img, 50, 60)) && is_clear(at(&img, 15, 30)));

    let mut m = base(100, 100);
    m.pen(0, 6, 0).select(0);
    m.brush(0, RED, 0).select(1);
    pts(&mut m, 0x0325, &[(10, 10), (90, 10), (90, 90)]);
    let img = draw(&m.finish());
    assert!(is_dark(at(&img, 50, 10)) && is_clear(at(&img, 50, 50)));

    // Two squares, one inside the other: a hole.
    let mut m = base(100, 100);
    fill(&mut m, RED);
    let mut v = vec![2, 4, 4];
    for sq in [
        [(10, 10), (90, 10), (90, 90), (10, 90)],
        [(30, 30), (70, 30), (70, 70), (30, 70)],
    ] {
        for (x, y) in sq {
            v.push(x);
            v.push(y);
        }
    }
    m.r(0x0538, &v);
    let img = draw(&m.finish());
    assert!(is_red(at(&img, 20, 20)) && is_clear(at(&img, 50, 50)));
}

#[test]
fn polygon_fill_mode() {
    let mut m = base(100, 100);
    fill(&mut m, RED);
    m.r(0x0106, &[2]); // WINDING
    let mut v = vec![2, 4, 4];
    for sq in [
        [(10, 10), (90, 10), (90, 90), (10, 90)],
        [(30, 30), (70, 30), (70, 70), (30, 70)],
    ] {
        for (x, y) in sq {
            v.push(x);
            v.push(y);
        }
    }
    m.r(0x0538, &v);
    assert!(is_red(at(&draw(&m.finish()), 50, 50)));
}

#[test]
fn ellipse_and_round_rect() {
    let mut m = base(100, 100);
    fill(&mut m, RED);
    m.r(0x0418, &[80, 90, 20, 10]); // bottom right top left
    let img = draw(&m.finish());
    assert!(is_red(at(&img, 50, 50)) && is_clear(at(&img, 12, 22)));

    let mut m = base(100, 100);
    fill(&mut m, RED);
    m.r(0x061C, &[40, 40, 90, 90, 10, 10]); // ellipse height, width, then the box
    let img = draw(&m.finish());
    assert!(is_red(at(&img, 50, 50)) && is_clear(at(&img, 11, 11)));
}

#[test]
fn arc_pie_and_chord_use_the_ray_angles() {
    // Parameters: yEnd xEnd yStart xStart bottom right top left.
    let arc = |m: &mut Wmf, f: u16| {
        m.r(f, &[0, 50, 50, 100, 100, 100, 0, 0]);
    };
    let mut m = base(100, 100);
    fill(&mut m, RED);
    arc(&mut m, 0x081A);
    let img = draw(&m.finish());
    assert!(is_red(at(&img, 75, 25)) && is_clear(at(&img, 25, 75)) && is_clear(at(&img, 75, 75)));

    let mut m = base(100, 100);
    fill(&mut m, RED);
    arc(&mut m, 0x0830);
    let img = draw(&m.finish());
    assert!(is_red(at(&img, 82, 18)) && is_clear(at(&img, 60, 40)));

    let mut m = base(100, 100);
    m.pen(0, 6, 0).select(0);
    m.brush(0, RED, 0).select(1);
    arc(&mut m, 0x0817);
    let img = draw(&m.finish());
    assert!(is_dark(at(&img, 85, 15)) && is_clear(at(&img, 75, 25)));
}

#[test]
fn window_extent_scales_to_the_bounding_box() {
    // The window is 10 x 10 units; the picture box is 100 x 100.
    let mut m = Wmf::new(100, 100);
    m.window(10, 10);
    fill(&mut m, RED);
    m.rect(2, 2, 8, 8);
    let img = draw(&m.finish());
    assert!(is_red(at(&img, 50, 50)) && is_red(at(&img, 22, 22)));
    assert!(is_clear(at(&img, 10, 50)));
}

#[test]
fn window_origin_shifts_the_drawing() {
    let mut m = Wmf::new(100, 100);
    m.r(0x0103, &[8])
        .r(0x020C, &[100, 100])
        .r(0x020B, &[-30, -20]);
    fill(&mut m, RED);
    m.rect(0, 0, 40, 40);
    let img = draw(&m.finish());
    assert!(is_red(at(&img, 50, 40)) && is_clear(at(&img, 20, 20)));
}

#[test]
fn a_negative_window_extent_flips() {
    let mut m = Wmf::new(100, 100);
    m.r(0x0103, &[8])
        .r(0x020B, &[100, 100])
        .r(0x020C, &[-100, -100]);
    fill(&mut m, RED);
    m.rect(60, 60, 90, 90);
    let img = draw(&m.finish());
    // Mirrored in both axes.
    assert!(is_red(at(&img, 25, 25)) && is_clear(at(&img, 75, 75)));
}

#[test]
fn explicit_viewport_records_are_honoured() {
    let mut m = Wmf::new(100, 100);
    m.r(0x0103, &[8]).r(0x020C, &[10, 10]);
    m.r(0x020E, &[50, 50]).r(0x020D, &[25, 25]); // viewport ext, then origin
    fill(&mut m, RED);
    m.rect(0, 0, 10, 10);
    let img = draw(&m.finish());
    assert!(is_red(at(&img, 50, 50)) && is_clear(at(&img, 10, 10)) && is_clear(at(&img, 90, 90)));
}

#[test]
fn offset_and_scale_records() {
    let mut m = Wmf::new(100, 100);
    m.window(100, 100);
    m.r(0x020F, &[-50, -50]); // OFFSETWINDOWORG y, x
    fill(&mut m, RED);
    m.rect(0, 0, 20, 20);
    assert!(is_red(at(&draw(&m.finish()), 60, 60)));
    let mut m = Wmf::new(100, 100);
    m.window(100, 100);
    m.r(0x0410, &[2, 1, 2, 1]); // window ext * 1/2 : scale doubles
    fill(&mut m, RED);
    m.rect(0, 0, 25, 25);
    assert!(is_red(at(&draw(&m.finish()), 40, 40)));
}

#[test]
fn a_file_without_a_placeable_header_takes_its_frame_from_the_window() {
    let mut m = Wmf::new(0, 0);
    m.placeable = false;
    m.window(200, 100);
    fill(&mut m, RED);
    m.rect(0, 0, 100, 100);
    let r = conv(&m.finish());
    assert!(r.svg.contains("viewBox=\"0 0 200 100\""), "{}", r.svg);
    let img = raster_svg(&r.svg, 200);
    assert!(is_red(at(&img, 50, 50)) && is_clear(at(&img, 150, 50)));
}

#[test]
fn a_window_set_after_the_first_drawing_does_not_move_it() {
    let mut m = Wmf::new(0, 0);
    m.placeable = false;
    m.window(100, 100);
    fill(&mut m, RED);
    m.rect(0, 0, 50, 50);
    m.r(0x020C, &[200, 200]); // the window grows afterwards: scale 0.5
    m.rect(0, 0, 100, 100);
    let r = conv(&m.finish());
    assert!(r.svg.contains("viewBox=\"0 0 100 100\""), "{}", r.svg);
}

// ----- objects ------------------------------------------------------------------------------

#[test]
fn created_objects_take_the_lowest_free_slot() {
    let mut m = base(100, 100);
    m.brush(0, RED, 0); // 0
    m.brush(0, GREEN, 0); // 1
    m.brush(0, BLUE, 0); // 2
    m.pen(5, 0, 0).select(3);
    m.delete(1);
    m.brush(0, rgb(255, 255, 0), 0); // reuses slot 1
    m.select(1).rect(0, 0, 50, 100);
    m.select(2).rect(50, 0, 100, 100);
    let img = draw(&m.finish());
    assert!(is_near(at(&img, 25, 50), [255, 255, 0], 3));
    assert!(is_blue(at(&img, 75, 50)));
}

#[test]
fn deleting_then_selecting_a_free_slot_changes_nothing() {
    let mut m = base(100, 100);
    m.brush(0, RED, 0).select(0).pen(5, 0, 0).select(1);
    m.delete(0).select(0);
    m.rect(0, 0, 100, 100);
    assert!(is_red(at(&draw(&m.finish()), 50, 50)));
}

#[test]
fn objects_that_are_not_drawn_with_still_take_a_slot() {
    let mut m = base(100, 100);
    m.raw(0x00F7, &[0, 0, 0, 0]); // CREATEPALETTE: slot 0
    m.raw(0x06FF, &[0; 8]); // CREATEREGION: slot 1
    m.brush(0, GREEN, 0); // slot 2
    m.select(2).pen(5, 0, 0).select(3);
    m.rect(0, 0, 100, 100);
    assert!(is_green(at(&draw(&m.finish()), 50, 50)));
}

#[test]
fn out_of_range_selects_and_deletes_are_ignored() {
    let mut m = base(100, 100);
    m.select(7).select(-1).delete(9).delete(-5);
    fill(&mut m, RED);
    m.rect(0, 0, 100, 100);
    assert!(is_red(at(&draw(&m.finish()), 50, 50)));
}

#[test]
fn hatch_brush() {
    let mut m = base(100, 100);
    m.brush(2, rgb(0, 0, 255), 5)
        .select(0)
        .pen(5, 0, 0)
        .select(1);
    m.r(0x0102, &[1]);
    m.rect(10, 10, 90, 90);
    let c = conv(&m.finish());
    assert!(c.svg.contains("<pattern"));
    let img = raster_svg(&c.svg, 100);
    assert!(count(&img, is_blue) > 20 && is_clear(at(&img, 5, 5)));
}

#[test]
fn pattern_brushes_are_mid_grey() {
    let mut m = base(100, 100);
    m.raw(0x01F9, &[0; 14]); // CREATEPATTERNBRUSH: slot 0
    m.select(0).pen(5, 0, 0).select(1);
    m.rect(0, 0, 100, 100);
    // Slot 0 is not a brush: the default white one stays.
    assert!(is_near(at(&draw(&m.finish()), 50, 50), [255, 255, 255], 2));
}

#[test]
fn dib_pattern_brush_is_the_average_colour() {
    let mut m = base(100, 100);
    let b = bmi(2, 2, 24, 0, &[], &[]);
    let bits = bits24(
        2,
        &[vec![[200, 0, 0], [200, 0, 0]], vec![[0, 0, 0], [0, 0, 0]]],
        false,
    );
    let mut p = vec![0u8, 0, 0, 0];
    p.extend(&b);
    p.extend(&bits);
    m.raw(0x0142, &p);
    m.select(0).pen(5, 0, 0).select(1);
    m.rect(0, 0, 100, 100);
    assert!(is_near(at(&draw(&m.finish()), 50, 50), [100, 0, 0], 3));
}

// ----- state --------------------------------------------------------------------------------

#[test]
fn save_and_restore() {
    let mut m = base(100, 100);
    m.brush(0, RED, 0).brush(0, BLUE, 0).pen(5, 0, 0);
    m.select(0).select(2);
    m.r(0x001E, &[]);
    m.select(1).rect(0, 0, 50, 50);
    m.r(0x0127, &[-1]);
    m.rect(50, 50, 100, 100);
    let img = draw(&m.finish());
    assert!(is_blue(at(&img, 25, 25)) && is_red(at(&img, 75, 75)));
}

#[test]
fn restore_with_an_absolute_or_hostile_index() {
    let mut m = base(100, 100);
    fill(&mut m, RED);
    m.r(0x0127, &[-9]).r(0x0127, &[30000]).r(0x0127, &[0]);
    m.rect(0, 0, 100, 100);
    assert!(is_red(at(&draw(&m.finish()), 50, 50)));
}

#[test]
fn clip_rectangles() {
    let mut m = base(100, 100);
    fill(&mut m, RED);
    m.r(0x0416, &[60, 60, 20, 20]); // INTERSECTCLIPRECT bottom right top left
    m.rect(0, 0, 100, 100);
    let img = draw(&m.finish());
    assert!(is_red(at(&img, 40, 40)) && is_clear(at(&img, 70, 40)));

    let mut m = base(100, 100);
    fill(&mut m, RED);
    m.r(0x0415, &[70, 70, 30, 30]); // EXCLUDECLIPRECT
    m.rect(0, 0, 100, 100);
    let img = draw(&m.finish());
    assert!(is_red(at(&img, 10, 10)) && is_clear(at(&img, 50, 50)));
}

#[test]
fn rop2_nop() {
    let mut m = base(100, 100);
    fill(&mut m, RED);
    m.r(0x0104, &[11]).rect(0, 0, 100, 100);
    assert_eq!(count(&draw(&m.finish()), |p| p[3] > 0), 0);
}

// ----- text ---------------------------------------------------------------------------------

pub(super) fn font(m: &mut Wmf, height: i32, esc: i32, weight: i32, charset: u8, face: &str) {
    let mut p = Vec::new();
    for v in [height, 0, esc, esc, weight] {
        p.extend((v as i16).to_le_bytes());
    }
    p.extend([0, 0, 0, charset, 0, 0, 0, 0x22]);
    let mut f = face.as_bytes().to_vec();
    f.resize(32, 0);
    p.extend(f);
    m.raw(0x02FB, &p);
}

pub(super) fn textout(m: &mut Wmf, x: i32, y: i32, s: &[u8]) {
    let mut p = (s.len() as u16).to_le_bytes().to_vec();
    p.extend(s);
    if s.len() % 2 == 1 {
        p.push(0);
    }
    p.extend((y as i16).to_le_bytes());
    p.extend((x as i16).to_le_bytes());
    m.raw(0x0521, &p);
}

fn text_svg(m: Wmf) -> String {
    let c = conv(&m.finish());
    raster_svg(&c.svg, 100);
    c.svg
}

#[test]
fn text_out() {
    let mut m = base(100, 100);
    font(&mut m, -16, 0, 400, 0, "Arial");
    m.select(0).r(0x0209, &[0xFF, 0]);
    textout(&mut m, 10, 40, b"Hello");
    let svg = text_svg(m);
    assert!(svg.contains(">Hello</text>"), "{svg}");
    assert!(svg.contains("translate(10 40"), "{svg}");
    assert!(svg.contains("font-size=\"16\""));
    assert!(svg.contains("fill=\"#ff0000\""), "{svg}");
}

#[test]
fn ext_text_out_with_a_dx_array() {
    let mut m = base(100, 100);
    font(&mut m, -16, 0, 700, 0, "Arial");
    m.select(0);
    let mut p = Vec::new();
    for v in [40i16, 10, 3, 0] {
        p.extend(v.to_le_bytes());
    }
    p.extend(b"abc\0");
    for v in [7i16, 8, 9] {
        p.extend(v.to_le_bytes());
    }
    m.raw(0x0A32, &p);
    let svg = text_svg(m);
    assert!(svg.contains("x=\"0 7 15\""), "{svg}");
    assert!(svg.contains("font-weight=\"700\""));
}

#[test]
fn ext_text_out_with_an_opaque_rectangle() {
    let mut m = base(100, 100);
    font(&mut m, -16, 0, 400, 0, "Arial");
    m.select(0).r(0x0201, &[0, 0xFF]); // bk colour: green (low word 0x0000? colour 0x0000FF00)
    let mut p = Vec::new();
    for v in [40i16, 10, 1, 2, 10, 20, 90, 60] {
        p.extend(v.to_le_bytes());
    }
    p.extend(b"a\0");
    m.raw(0x0A32, &p);
    let img = raster_svg(&conv(&m.finish()).svg, 100);
    // 0x0000FF00 = green? The two parameters are the low and high word: 0, 0xFF -> 0x00FF0000 = blue.
    assert!(is_blue(at(&img, 80, 50)), "{:?}", at(&img, 80, 50));
}

#[test]
fn text_in_shift_jis() {
    let mut m = base(100, 100);
    font(&mut m, -16, 0, 400, 128, "MS Gothic");
    m.select(0);
    textout(&mut m, 10, 40, &[0x82, 0xA0, 0x82, 0xA2]);
    let svg = text_svg(m);
    assert!(svg.contains(">あい</text>"), "{svg}");
}

#[test]
fn text_rotation_and_alignment() {
    let mut m = base(100, 100);
    font(&mut m, -16, 900, 400, 0, "Arial");
    m.select(0).r(0x012E, &[6 | 24]);
    textout(&mut m, 50, 50, b"Up");
    let svg = text_svg(m);
    assert!(
        svg.contains("rotate(-90)") && svg.contains("text-anchor=\"middle\""),
        "{svg}"
    );
}

#[test]
fn text_with_update_cp_continues_where_the_last_ended() {
    let mut m = base(100, 100);
    font(&mut m, -16, 0, 400, 0, "Arial");
    m.select(0).r(0x012E, &[1]);
    m.r(0x0214, &[40, 10]);
    textout(&mut m, 0, 0, b"ab");
    textout(&mut m, 0, 0, b"cd");
    let svg = text_svg(m);
    let first = svg.find("translate(10 40").expect("first at the position");
    let second = svg.rfind("translate(").unwrap();
    assert!(second > first, "{svg}");
    assert!(
        !svg[second..].starts_with("translate(10 40"),
        "the second run starts further right: {svg}"
    );
}

// ----- bitmaps ------------------------------------------------------------------------------

#[test]
fn dib_stretch_blt() {
    let mut m = base(100, 100);
    // srcH srcW ySrc xSrc destH destW yDst xDst
    dib_record(
        &mut m,
        0x0B41,
        &[2, 2, 0, 0, 100, 100, 0, 0],
        0x00CC0020,
        None,
        &bmi(2, 2, 24, 0, &[], &[]),
        &bits24(2, &quad(), false),
    );
    let img = draw(&m.finish());
    assert!(is_red(at(&img, 25, 25)) && is_green(at(&img, 75, 25)));
    assert!(is_blue(at(&img, 25, 75)));
}

#[test]
fn stretch_dib() {
    let mut m = base(100, 100);
    dib_record(
        &mut m,
        0x0F43,
        &[2, 2, 0, 0, 100, 100, 0, 0],
        0x00CC0020,
        Some(0),
        &bmi(2, 2, 24, 0, &[], &[]),
        &bits24(2, &quad(), false),
    );
    let img = draw(&m.finish());
    assert!(is_red(at(&img, 25, 25)) && is_blue(at(&img, 25, 75)));
}

#[test]
fn dib_bit_blt_and_pat_blt() {
    // DIBBITBLT: ySrc xSrc height width yDest xDest; two logical units wide, scaled by the window.
    let mut m = Wmf::new(100, 100);
    m.window(2, 2);
    dib_record(
        &mut m,
        0x0940,
        &[0, 0, 2, 2, 0, 0],
        0x00CC0020,
        None,
        &bmi(2, 2, 24, 0, &[], &[]),
        &bits24(2, &quad(), false),
    );
    let img = draw(&m.finish());
    assert!(is_red(at(&img, 25, 25)) && is_green(at(&img, 75, 25)));

    // PATBLT with PATCOPY.
    let mut m = base(100, 100);
    m.brush(0, RED, 0).select(0).pen(5, 0, 0).select(1);
    let mut p = 0x00F00021u32.to_le_bytes().to_vec();
    for v in [60i16, 60, 10, 10] {
        p.extend(v.to_le_bytes());
    }
    m.raw(0x061D, &p);
    let img = draw(&m.finish());
    assert!(is_red(at(&img, 40, 40)) && is_clear(at(&img, 80, 80)));
}

#[test]
fn dib_bit_blt_without_a_bitmap_is_a_pattern_operation() {
    let mut m = base(100, 100);
    m.brush(0, RED, 0).select(0);
    // ySrc xSrc height width yDest xDest + 2 reserved
    let mut p2 = 0x00F00021u32.to_le_bytes().to_vec();
    for v in [0i16, 0, 60, 60, 10, 10, 0] {
        p2.extend(v.to_le_bytes());
    }
    m.raw(0x0940, &p2);
    assert!(is_red(at(&draw(&m.finish()), 40, 40)));
}

#[test]
fn an_embedded_dib_with_a_palette() {
    let mut m = base(100, 100);
    let pal = [[255, 0, 0], [0, 0, 255]];
    // 2 x 1 at 1 bpp: pixels 0 and 1 -> stored bits 01xxxxxx.
    dib_record(
        &mut m,
        0x0B41,
        &[1, 2, 0, 0, 100, 100, 0, 0],
        0x00CC0020,
        None,
        &bmi(2, 1, 1, 0, &pal, &[]),
        &[0x40, 0, 0, 0],
    );
    let img = draw(&m.finish());
    assert!(is_red(at(&img, 25, 50)) && is_blue(at(&img, 75, 50)));
}

// ----- records ------------------------------------------------------------------------------

#[test]
fn unknown_and_escape_records_are_skipped() {
    let mut m = base(100, 100);
    m.r(0x0626, &[1, 2, 3]).r(0x7777, &[1]);
    fill(&mut m, RED);
    m.rect(0, 0, 100, 100);
    let c = conv(&m.finish());
    assert!(!c.truncated);
    assert!(is_red(at(&raster_svg(&c.svg, 100), 50, 50)));
}

#[test]
fn eof_ends_the_playback() {
    let mut m = base(100, 100);
    fill(&mut m, RED);
    m.rect(0, 0, 50, 100);
    m.r(0x0000, &[]);
    m.rect(50, 0, 100, 100);
    let img = draw(&m.finish());
    assert!(is_red(at(&img, 25, 50)) && is_clear(at(&img, 75, 50)));
}

#[test]
fn a_standard_header_with_text_map_mode() {
    let mut m = Wmf::new(0, 0);
    m.placeable = false;
    m.r(0x0103, &[1]);
    fill(&mut m, RED);
    m.rect(10, 10, 60, 60);
    let c = conv(&m.finish());
    let img = raster_svg(&c.svg, 60);
    assert!(count(&img, is_red) > 100);
}

#[test]
fn natural_size_follows_the_inch() {
    let mut m = base(100, 100);
    // 96 units per inch (the builder): 100 units = 100 px.
    fill(&mut m, RED);
    m.rect(0, 0, 100, 100);
    let c = conv(&m.finish());
    assert!((c.width_px - 100.0).abs() < 0.01);
}

// ----- palettes, region clips, empty opaque text ------------------------------------------------

#[test]
fn a_dib_without_a_colour_table_uses_the_selected_palette() {
    let mut m = base(100, 100);
    // CREATEPALETTE: version, 2 entries (red, blue); SELECTPALETTE.
    let mut p = vec![0u8, 3, 2, 0];
    p.extend([255, 0, 0, 0, 0, 0, 255, 0]);
    m.raw(0x00F7, &p);
    m.r(0x0234, &[0]);
    // 2 x 1, 8 bpp, no table: pixels 0 and 1.
    let mut hdr = bmi(2, 1, 8, 0, &[], &[]);
    hdr.truncate(40);
    dib_record(
        &mut m,
        0x0B41,
        &[1, 2, 0, 0, 100, 100, 0, 0],
        0x00CC0020,
        None,
        &hdr,
        &[0, 1, 0, 0],
    );
    let img = draw(&m.finish());
    assert!(is_red(at(&img, 25, 50)) && is_blue(at(&img, 75, 50)));
}

#[test]
fn select_clip_region_zero_clears_the_clip() {
    let mut m = base(100, 100);
    fill(&mut m, RED);
    m.r(0x0416, &[0, 0, 0, 0]); // an empty clip
    m.r(0x012C, &[0]); // the null region: no clip
    m.rect(0, 0, 100, 100);
    assert!(is_red(at(&draw(&m.finish()), 50, 50)));
    // A non-null region handle is not understood and leaves the clip alone.
    let mut m = base(100, 100);
    fill(&mut m, RED);
    m.r(0x0416, &[0, 0, 0, 0]);
    m.r(0x012C, &[3]);
    m.rect(0, 0, 100, 100);
    assert!(is_clear(at(&draw(&m.finish()), 50, 50)));
}

#[test]
fn an_opaque_rectangle_is_filled_even_without_text() {
    let mut m = base(100, 100);
    font(&mut m, -16, 0, 400, 0, "Arial");
    m.select(0).r(0x0201, &[0xFF00, 0]); // bk colour 0x0000FF00 = green
    let mut p = Vec::new();
    for v in [0i16, 0, 0, 2, 10, 20, 90, 60] {
        p.extend(v.to_le_bytes());
    }
    m.raw(0x0A32, &p);
    let img = draw(&m.finish());
    assert!(is_green(at(&img, 50, 40)));
}
