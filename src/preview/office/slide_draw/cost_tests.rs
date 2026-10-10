//! The time budget of the SVG writer (see [`super::cost`]): each test is the small form of a deck
//! that kept the drawing process busy for seconds although it was far under every byte budget,
//! and states that the writer predicts what it wrote to be inside [`cost::MAX_CHILD_MS`] and says
//! so when it left something out.

use super::cost::{Features, MAX_CHILD_MS};
use super::hardening_tests::{
    by_key, compound_line, fx_shape, guard_accepts, guard_accepts_at, guard_work, model_work,
    noise_png, pic, shadow,
};
use super::path::Seg;
use super::tests::{e, no_media, png, rect_item, scene, text_shape, RED};
use super::*;

/// The time the writer predicts for `r` at the model raster.
fn predicted_ms(r: &Rendered) -> f64 {
    r.features.predict().at(1.0)
}

fn assert_within_budget(r: &Rendered, what: &str) {
    let ms = predicted_ms(r);
    assert!(
        ms <= MAX_CHILD_MS * 1.0001,
        "{what}: predicted {ms:.0} ms, over the {MAX_CHILD_MS} ms budget"
    );
}

/// `n` shapes each holding `chars` characters of text in a box of its own.
fn text_slide(n: usize, chars: usize) -> SlideScene {
    let text = "ab ".repeat(chars / 3 + 1);
    let items: Vec<Item> = (0..n)
        .map(|i| {
            Item::Shape(text_shape(
                (i % 12) as f64 * 78.0,
                (i / 12 % 20) as f64 * 26.0,
                76.0,
                24.0,
                &text[..chars],
                6.0,
            ))
        })
        .collect();
    scene(items)
}

// ---- 1. an item is checked with its own work (rr2 1) --------------------------------------------

fn every_effect() -> Effects {
    let mut fx = shadow(60.0);
    fx.inner_shadow = fx.outer_shadow;
    fx.glow = Some(Glow {
        rad: e(30.0),
        color: Rgba::new(255, 0, 0, 0.5),
    });
    fx.soft_edge = Some(e(20.0));
    fx.reflection = Some(Reflection {
        blur_rad: e(10.0),
        start_alpha: 0.5,
        end_alpha: 0.0,
        start_pos: 0.0,
        end_pos: 1.0,
        dist: e(5.0),
        dir_deg: 90.0,
        fade_dir_deg: 90.0,
        sx: 1.0,
        sy: -1.0,
    });
    fx
}

#[test]
fn one_shape_with_every_effect_cannot_take_the_slide_past_the_work_allowance() {
    let one_shadow = || fx_shape(900.0, 500.0, shadow(30.0));
    let all = || fx_shape(900.0, 500.0, every_effect());
    render_svg(&scene(vec![one_shadow()]), &no_media);
    let w_shadow = model_work();
    render_svg(&scene(vec![all()]), &no_media);
    let w_all = model_work();
    // The setup: three shadows are inside the allowance, and the shape with every effect would
    // take the total past it (the old check looked only at what came *before* a shape).
    let n = 3;
    let allowance = super::svg::MAX_FILTER_WORK;
    assert!(n as f64 * w_shadow <= allowance, "{w_shadow:.3e}");
    assert!(n as f64 * w_shadow + w_all > allowance, "{w_all:.3e}");
    let mut items: Vec<Item> = (0..n).map(|_| one_shadow()).collect();
    items.push(all());
    let r = render_svg(&scene(items), &no_media);
    assert!(r.truncated);
    assert!(model_work() <= allowance, "{:.3e}", model_work());
    assert!(guard_accepts(&r), "the drawing process accepts it");
    // the three shadows have their filters, the last shape is there without any
    assert_eq!(r.svg.matches("<filter ").count(), n);
    assert!(r.svg.contains(&hex6(RED)), "the shape is drawn plain");
}

fn hex6(c: Rgba) -> String {
    format!("#{:02x}{:02x}{:02x}", c.r, c.g, c.b)
}

#[test]
fn a_compound_line_is_checked_with_its_own_mask_work_before_it_is_added() {
    let line = || compound_line(0.0, 0.0, 960.0, 540.0, 100.0, Compound::Tri);
    render_svg(&scene(vec![line()]), &no_media);
    let w = model_work();
    assert!(w > 0.0);
    let allowance = super::svg::MAX_FILTER_WORK;
    let fit = (allowance / w).floor() as usize;
    assert!(fit >= 1, "{w:.3e}");
    let n = fit + 3;
    let r = render_svg(&scene((0..n).map(|_| line()).collect()), &no_media);
    assert!(r.truncated);
    // no more masks than the allowance holds (the time budget may hold fewer); the others are
    // plain lines, and every line is still drawn
    let masks = r.svg.matches("<mask ").count();
    assert!(masks >= 1 && masks <= fit, "{masks} of {fit}");
    assert!(model_work() <= allowance, "{:.3e}", model_work());
    assert_eq!(r.svg.matches("stroke-linecap").count(), n);
}

// ---- 2. pictures used many times (rr2 2) ---------------------------------------------------------

/// A PNG of `w` x `h` with an ancillary chunk of `pad` bytes in front of its end: a picture of a
/// few pixels that is megabytes long.
fn padded_png(w: u32, h: u32, pad: usize) -> Vec<u8> {
    let mut out = png(w, h, |_, _| [10, 200, 30, 255]);
    let iend = out.len() - 12;
    let mut chunk = Vec::new();
    let mut body = b"tEXt".to_vec();
    body.extend_from_slice(b"k\0");
    body.resize(body.len() + pad, b'a');
    chunk.extend_from_slice(&((body.len() - 4) as u32).to_be_bytes());
    let mut crc = flate2::Crc::new();
    crc.update(&body);
    chunk.extend_from_slice(&body);
    chunk.extend_from_slice(&crc.sum().to_be_bytes());
    out.splice(iend..iend, chunk);
    out
}

#[test]
fn a_small_picture_that_is_megabytes_long_is_not_decoded_thousands_of_times() {
    let bytes = padded_png(8, 8, 1 << 20);
    assert!(bytes.len() > 1 << 20);
    let media = by_key(vec![("p", bytes)]);
    let items: Vec<Item> = (0..3000)
        .map(|i| {
            pic(
                "p",
                (i % 60) as f64 * 15.0,
                (i / 60) as f64 * 10.0,
                14.0,
                9.0,
            )
        })
        .collect();
    let r = render_svg(&scene(items), &*media);
    assert!(r.truncated, "3000 uses of 1 MiB are a lot of work");
    let uses = r.svg.matches("<use ").count();
    assert!(uses > 0 && uses < 3000, "{uses}");
    assert_within_budget(&r, "bytes times uses");
    // what the process decodes: bytes times uses, inside what the budget allows at all
    assert!(r.features.pic_bytes >= uses as f64 * (1 << 20) as f64 * 0.99);
}

fn svg_picture(paths: usize) -> Vec<u8> {
    let mut s = String::from(r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100">"#);
    for i in 0..paths {
        s.push_str(&format!(
            r##"<path d="M{} 1 L{} 99 L50 {}" fill="#336699"/>"##,
            i % 90,
            (i * 7) % 90,
            i % 50
        ));
    }
    s.push_str("</svg>");
    s.into_bytes()
}

#[test]
fn a_vector_picture_used_many_times_is_budgeted_by_its_bytes_and_uses() {
    let bytes = svg_picture(4000);
    assert!(bytes.len() > 150_000, "{}", bytes.len());
    let media = by_key(vec![("v", bytes)]);
    let items: Vec<Item> = (0..300)
        .map(|i| {
            pic(
                "v",
                (i % 20) as f64 * 45.0,
                (i / 20) as f64 * 35.0,
                40.0,
                30.0,
            )
        })
        .collect();
    let r = render_svg(&scene(items), &*media);
    assert!(r.truncated);
    let uses = r.svg.matches("<use ").count();
    assert!(uses < 300, "{uses}");
    assert_within_budget(&r, "vector picture times uses");
    assert!(r.features.svg_bytes > 0.0, "vector pictures are counted");
}

#[test]
fn many_strokes_wider_than_their_picture_make_the_picture_cost_its_area_many_times() {
    // 30 000 paths each 100 000 units wide: 30 000 times the whole picture, in 1.5 MB of markup
    let mut s = String::from(r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10">"#);
    for i in 0..30_000 {
        s.push_str(&format!(
            r##"<path d="M{} 1L98 50" fill="none" stroke="#f00" stroke-width="100000"/>"##,
            i % 40
        ));
    }
    s.push_str("</svg>");
    let media = by_key(vec![("v", s.into_bytes())]);
    let r = render_svg(&scene(vec![pic("v", 0.0, 0.0, 960.0, 540.0)]), &*media);
    assert_within_budget(&r, "fat strokes");
    assert!(r.truncated, "the picture is given up");
    assert!(
        !r.svg.contains("<use "),
        "it costs more than the whole budget however it is drawn: it is left out"
    );
    // the same picture drawn small is a small cost
    let small = render_svg(&scene(vec![pic("v", 0.0, 0.0, 8.0, 4.0)]), &*media);
    assert!(
        small.features.vec_px2 < r.features.vec_px2 || r.features.vec_px2 == 0.0,
        "the area grows with the size it is drawn at"
    );
}

// ---- 3. effects on a huge picture (rr1 3) --------------------------------------------------------

/// The signature and header chunk of a PNG that says it is `w` x `h` and has no data: enough for
/// the writer to see its size, never enough to decode.
fn png_header_only(w: u32, h: u32) -> Vec<u8> {
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = b"IHDR".to_vec();
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    out.extend_from_slice(&13u32.to_be_bytes());
    let mut crc = flate2::Crc::new();
    crc.update(&ihdr);
    out.extend_from_slice(&ihdr);
    out.extend_from_slice(&crc.sum().to_be_bytes());
    // an empty data chunk: the decoder reads the header up to the first one
    out.extend_from_slice(&0u32.to_be_bytes());
    out.extend_from_slice(b"IDAT");
    let mut crc = flate2::Crc::new();
    crc.update(b"IDAT");
    out.extend_from_slice(&crc.sum().to_be_bytes());
    out
}

fn gray_pic(key: &str) -> Item {
    let mut p = PictureItem::new(
        Xfrm::rect(e(10.0), e(10.0), e(100.0), e(100.0)),
        key.to_string(),
    );
    p.image.fx = vec![PicFx::Grayscale];
    Item::Picture(p)
}

#[test]
fn a_picture_too_big_to_decode_for_a_colour_effect_is_drawn_as_it_is_and_the_result_says_so() {
    // 36 Mpx: past what is decoded for an effect (the 8192 x 8192 picture of 67 Mpx that took the
    // writer to 740 MB is past what a slide may use at all, and is not embedded either)
    let side = 6000;
    let bytes = png_header_only(side, side);
    let media = by_key(vec![("p", bytes.clone())]);
    let r = render_svg(&scene(vec![gray_pic("p")]), &*media);
    assert!(r.truncated);
    let huge = by_key(vec![("p", png_header_only(8192, 8192))]);
    let r8 = render_svg(&scene(vec![gray_pic("p")]), &*huge);
    assert!(r8.truncated && r8.svg.matches("<image ").count() == 0 && r8.svg.contains("#e6e6e6"));
    // not decoded and not recoloured: the original bytes are the ones embedded
    use base64::Engine as _;
    let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
    assert!(r.svg.contains(&b64), "the picture is embedded as it is");
}

#[test]
fn the_decode_peak_per_picture_is_stated_and_the_cap_sits_between_4096_and_8192_squared() {
    let per_px = 8u64;
    let cap_px = super::svg::MAX_DECODE_PEAK_BYTES / per_px;
    assert!(
        cap_px >= 4096 * 4096,
        "4096 x 4096 pictures keep their effects"
    );
    assert!(cap_px < 8192 * 8192, "8192 x 8192 pictures do not");
    // just under the cap the writer tries (and, with no data behind the header, cannot decode:
    // that is not a reason to say it left something out)
    let side = 4096;
    let media = by_key(vec![("p", png_header_only(side, side))]);
    let r = render_svg(&scene(vec![gray_pic("p")]), &*media);
    assert!(!r.truncated, "within the cap");
}

// ---- 4. the child's CPU is predicted (rr1 4) -----------------------------------------------------

#[test]
fn thousands_of_text_runs_are_cut_where_the_predicted_time_is_used_up() {
    let r = render_svg(&text_slide(4500, 60), &no_media);
    assert!(r.truncated);
    assert_within_budget(&r, "4500 text boxes");
    assert!(
        r.svg.matches("<tspan ").count() > 1000,
        "most of the budget is spent on text, not given up at the first line"
    );
}

#[test]
fn one_enormous_run_of_text_costs_more_than_its_length() {
    // 100 000 characters without a break (a long word, tabs and spaces in one run): the process
    // took 2.8 s for this where the same text in 20 000-character pieces takes 0.18 s each
    let long = "x".repeat(100_000);
    let s = text_shape(0.0, 0.0, 900.0, 500.0, &long, 8.0);
    let mut body = s.text.clone().unwrap();
    body.wrap = false;
    let mut s = s;
    s.text = Some(body);
    let r = render_svg(&scene(vec![Item::Shape(s)]), &no_media);
    assert_within_budget(&r, "one long run");
    assert!(
        r.features.glyphs_sq >= r.features.glyphs,
        "squares are counted"
    );
}

#[test]
fn a_path_of_hundreds_of_thousands_of_long_lines_is_left_out_not_sent_to_the_process() {
    // alternating corners of the slide: every segment crosses it
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
    s.fill = Fill::Solid(RED);
    s.line = Some(Line::solid(e(1.0), Rgba::BLACK));
    let r = render_svg(&scene(vec![Item::Shape(s)]), &no_media);
    assert!(r.truncated);
    assert_within_budget(&r, "300 000 long lines");
}

#[test]
fn compound_lines_with_dashes_and_arrows_are_inside_the_budget_too() {
    let items: Vec<Item> = (0..600)
        .map(|i| {
            let mut s = ShapeItem::new(
                Xfrm::rect(e(0.0), e((i % 500) as f64), e(960.0), e(30.0)),
                Geometry::Line,
            );
            let mut l = Line::solid(e(40.0), Rgba::BLACK);
            l.compound = Compound::Tri;
            l.dash = Dash::SysDot;
            l.tail = Some(Arrow {
                kind: ArrowKind::Triangle,
                w: ArrowSize::Lg,
                len: ArrowSize::Lg,
            });
            s.line = Some(l);
            Item::Shape(s)
        })
        .collect();
    let r = render_svg(&scene(items), &no_media);
    assert_within_budget(&r, "compound dashed lines with arrows");
}

/// Items of text boxes whose predicted time is just under the budget: the most boxes (found by
/// halving the interval) that leave `room_ms` free.
fn text_up_to_budget(room_ms: f64) -> Vec<Item> {
    let fits = |n: usize| {
        let r = render_svg(&text_slide(n, 60), &no_media);
        !r.truncated && predicted_ms(&r) <= MAX_CHILD_MS - room_ms
    };
    let (mut lo, mut hi) = (20usize, 20usize);
    while fits(hi) {
        lo = hi;
        hi *= 2;
    }
    while hi - lo > 8 {
        let mid = (lo + hi) / 2;
        if fits(mid) {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    text_slide(lo, 60).items
}

#[test]
fn a_shape_that_fits_only_without_its_effects_is_drawn_without_them() {
    let texts = text_up_to_budget(100.0);
    let before = render_svg(&scene(texts.clone()), &no_media);
    let room = MAX_CHILD_MS - predicted_ms(&before);
    assert!(room > 0.0 && room < 200.0, "{room}");
    // the shape alone: with its shadow it costs more than the room, plain it costs less
    let shaped = |fx: Effects| {
        let mut s = ShapeItem::new(
            Xfrm::rect(e(10.0), e(10.0), e(900.0), e(500.0)),
            Geometry::Rect,
        );
        s.fill = Fill::Solid(RED);
        s.effects = fx;
        Item::Shape(s)
    };
    let cost_of =
        |fx: Effects| predicted_ms(&render_svg(&scene(vec![shaped(fx)]), &no_media)) - 40.0;
    assert!(cost_of(shadow(30.0)) > room, "the shadow does not fit");
    assert!(cost_of(Effects::default()) < room, "the plain shape does");
    let mut items = texts;
    items.push(shaped(shadow(30.0)));
    let r = render_svg(&scene(items), &no_media);
    assert!(r.truncated);
    assert_within_budget(&r, "text and a shadow");
    assert!(r.svg.contains(&hex6(RED)), "the shape is drawn");
    assert_eq!(r.svg.matches("<filter ").count(), 0, "without its shadow");
}

#[test]
fn a_try_that_did_not_fit_leaves_no_dangling_reference_and_the_picture_stays_defined() {
    // The picture is first used by a shape whose shadow does not fit: that try is taken back and
    // the shape is drawn again plain; the picture it embedded is still there for the second try.
    let media = by_key(vec![("p", noise_png(16, 16, 7))]);
    let mut items = text_up_to_budget(100.0);
    let mut p = PictureItem::new(
        Xfrm::rect(e(10.0), e(10.0), e(900.0), e(500.0)),
        "p".to_string(),
    );
    p.effects = shadow(30.0);
    items.push(Item::Picture(p));
    let r = render_svg(&scene(items), &*media);
    assert!(r.truncated);
    assert_eq!(r.svg.matches("<filter ").count(), 0);
    assert_eq!(r.svg.matches("<image ").count(), 1);
    for cap in r.svg.split("href=\"#").skip(1) {
        let id = cap.split('"').next().unwrap();
        assert!(
            r.svg.contains(&format!("id=\"{id}\"")),
            "dangling reference to {id}"
        );
    }
    assert!(r.svg.contains("<use "), "the picture is drawn");
}

// ---- 5. costs that grow with the raster (rr1 5) ---------------------------------------------------

fn rendered_with(f: Features) -> Rendered {
    Rendered {
        svg: String::new(),
        truncated: false,
        cancelled: false,
        filter_work: 0.0,
        model_px: 1280.0,
        features: f,
    }
}

#[test]
fn the_raster_cap_follows_the_predicted_time_and_never_goes_below_the_model_size() {
    let light = rendered_with(Features::default());
    assert_eq!(light.max_raster_px(), u32::MAX);
    // area-bound work: the cap shrinks as the area grows, down to the floor
    let mut last = u32::MAX;
    for a in [1e6, 1e7, 1e8, 1e9, 1e10, 1e12] {
        let cap = rendered_with(Features {
            grad_px2: a,
            ..Default::default()
        })
        .max_raster_px();
        assert!(cap <= last, "{a:e}: {cap} after {last}");
        assert!(cap >= 1280);
        last = cap;
    }
    assert_eq!(last, 1280, "the floor");
    // at the cap the predicted time is the budget
    let f = Features {
        grad_px2: 2e8,
        ..Default::default()
    };
    let r = rendered_with(f);
    let cap = f64::from(r.max_raster_px());
    if cap > 1280.0 && cap < f64::from(u32::MAX) {
        let at = f.predict().at(cap / 1280.0);
        assert!((at - MAX_CHILD_MS).abs() < MAX_CHILD_MS * 0.01, "{at}");
    }
    // fixed time over the budget: no room at all
    let heavy = rendered_with(Features {
        els: 1e12,
        ..Default::default()
    });
    assert_eq!(heavy.max_raster_px(), 1280);
}

#[test]
fn a_raster_limit_that_cannot_be_computed_is_the_model_size_not_no_limit() {
    // `f64::min` ignores a NaN, so a NaN limit used to leave the other one (or none) in charge.
    let model = 1280;
    let nan = f64::NAN;
    for features in [
        Features {
            grad_px2: nan,
            ..Default::default()
        },
        Features {
            els: nan,
            ..Default::default()
        },
        Features {
            edge_px: nan,
            ..Default::default()
        },
        Features {
            grad_px2: -1e9,
            ..Default::default()
        },
        Features {
            edge_px: -1e9,
            ..Default::default()
        },
        Features {
            els: -1e12,
            ..Default::default()
        },
    ] {
        assert_eq!(
            rendered_with(features).max_raster_px(),
            model,
            "{features:?}"
        );
        // ... also when the filter work alone would give a bigger size.
        let mut r = rendered_with(features);
        r.filter_work = 1.0;
        assert_eq!(r.max_raster_px(), model, "{features:?}");
    }
    // A NaN or negative filter work.
    for work in [nan, -1.0, f64::NEG_INFINITY] {
        let mut r = rendered_with(Features::default());
        r.filter_work = work;
        assert_eq!(r.max_raster_px(), model, "{work}");
    }
    // Infinite work is the floor as well, not unlimited.
    let mut r = rendered_with(Features::default());
    r.filter_work = f64::INFINITY;
    assert_eq!(r.max_raster_px(), model);
    // Sane inputs are unchanged: no work at all is no limit.
    assert_eq!(rendered_with(Features::default()).max_raster_px(), u32::MAX);
}

#[test]
fn tiled_fills_over_the_whole_slide_are_budgeted_by_their_area_and_tile_count() {
    // 600 shapes of the whole slide filled with a 1 x 1 px tile: a lot of pixels to fill, and
    // half a million tiles each
    let media = by_key(vec![("t", png(2, 2, |_, _| [1, 2, 3, 255]))]);
    let items: Vec<Item> = (0..600)
        .map(|_| {
            let mut s = ShapeItem::new(Xfrm::rect(0.0, 0.0, e(960.0), e(540.0)), Geometry::Rect);
            let mut f = ImageFill::stretch("t".to_string());
            f.mode = ImageMode::Tile {
                sx: 0.01,
                sy: 0.01,
                tx: 0.0,
                ty: 0.0,
                align: RectAlign::TopLeft,
                flip: TileFlip::None,
            };
            s.fill = Fill::Image(f);
            Item::Shape(s)
        })
        .collect();
    let r = render_svg(&scene(items), &*media);
    assert!(r.truncated, "{:?}", r.features);
    assert!(r.features.tiles > 1e6, "{:?}", r.features);
    assert_within_budget(&r, "tiles");
    let drawn = r.svg.matches("<pattern ").count();
    assert!(drawn > 0 && drawn < 600, "{drawn}");
    // the raster it is drawn at keeps the same budget
    let cap = r.max_raster_px();
    let at_cap = r.features.predict().at(f64::from(cap) / r.model_px);
    assert!(
        f64::from(cap) <= r.model_px || at_cap <= MAX_CHILD_MS * 1.001,
        "{cap}: {at_cap}"
    );
}

#[test]
fn the_raster_cap_keeps_the_predicted_time_inside_the_budget_for_a_slide_of_big_fills() {
    let items: Vec<Item> = (0..400)
        .map(|i| {
            let mut s = ShapeItem::new(
                Xfrm::rect(e(0.0), e(0.0), e(960.0), e(540.0)),
                Geometry::Rect,
            );
            s.fill = Fill::Gradient(Gradient::linear(
                (i % 90) as f64,
                vec![(0.0, RED), (1.0, Rgba::BLACK)],
            ));
            Item::Shape(s)
        })
        .collect();
    let r = render_svg(&scene(items), &no_media);
    assert_within_budget(&r, "gradients");
    let cap = f64::from(r.max_raster_px());
    assert!(cap >= r.model_px);
    if cap < f64::from(u32::MAX) {
        let at = r.features.predict().at(cap / r.model_px);
        assert!(
            cap == r.model_px || at <= MAX_CHILD_MS * 1.001,
            "{cap}: {at}"
        );
    }
}

// ---- 6. cancellation inside long loops (rr1 6) ---------------------------------------------------

#[test]
fn a_long_path_is_not_written_to_the_end_once_the_render_is_cancelled() {
    let mut cmds = vec![PathCmd::MoveTo(Pt::new(0.0, 0.0))];
    for i in 0..150_000 {
        cmds.push(PathCmd::LineTo(Pt::new(
            e((i % 900) as f64),
            e((i % 500) as f64),
        )));
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
    let sc = scene(vec![Item::Shape(s)]);
    let full = render_svg_cancellable(&sc, &no_media, &|| false);
    assert!(!full.cancelled);
    // told to stop on the second look: the writing of the path stops with it
    let looks = std::cell::Cell::new(0usize);
    let stop = || {
        looks.set(looks.get() + 1);
        looks.get() >= 2
    };
    let cut = render_svg_cancellable(&sc, &no_media, &stop);
    assert!(cut.cancelled);
    assert!(
        cut.svg.len() < full.svg.len() / 2,
        "{} of {} bytes",
        cut.svg.len(),
        full.svg.len()
    );
}

#[test]
fn a_long_path_is_not_resolved_to_the_end_once_it_is_told_to_stop() {
    let mut cmds = vec![PathCmd::MoveTo(Pt::new(0.0, 0.0))];
    for _ in 0..50_000 {
        cmds.push(PathCmd::ArcTo {
            wr: 100.0,
            hr: 50.0,
            st_deg: 10.0,
            sw_deg: 120.0,
        });
    }
    let p = GeomPath {
        w: 100.0,
        h: 100.0,
        fill_mode: PathFill::Norm,
        stroke: true,
        cmds,
    };
    let all = super::path::resolve_path_capped(&p, 0.0, 0.0, 1e6, 1e6, usize::MAX);
    let polls = std::cell::Cell::new(0usize);
    let cut = super::path::resolve_path_polled(&p, 0.0, 0.0, 1e6, 1e6, usize::MAX, &|| {
        polls.set(polls.get() + 1);
        true
    });
    assert_eq!(polls.get(), 1, "it stops at the first look that says yes");
    assert!(cut.truncated && cut.segs.len() < all.segs.len() / 10);
    // a stop that never comes changes nothing
    let same = super::path::resolve_path_polled(&p, 0.0, 0.0, 1e6, 1e6, usize::MAX, &|| false);
    assert_eq!(same.segs.len(), all.segs.len());
    assert!(!same.truncated);
}

// ---- 7. a picture bullet keeps its aspect (low) ------------------------------------------------------

#[test]
fn a_picture_bullet_whose_size_the_writer_does_not_know_is_fitted_by_the_renderer() {
    // an SVG with no size at all: the writer cannot fit it into the square, so the image is
    // written to fit itself (`meet`), never stretched to the square
    let svg = br##"<svg xmlns="http://www.w3.org/2000/svg"><rect width="40" height="10" fill="#f00"/></svg>"##.to_vec();
    let media = by_key(vec![("p", svg)]);
    let mut body = TextBody::default();
    let mut para = Paragraph {
        runs: vec![Run::text("x", 24.0)],
        ..Default::default()
    };
    para.bullet = Some(Bullet {
        kind: BulletKind::Picture(ImageFill::stretch("p".to_string())),
        font: None,
        color: None,
        size: BulletSize::Pct(1.0),
    });
    body.paragraphs = vec![para];
    let mut s = ShapeItem::new(
        Xfrm::rect(e(0.0), e(0.0), e(300.0), e(100.0)),
        Geometry::Rect,
    );
    s.text = Some(body);
    let r = render_svg(&scene(vec![Item::Shape(s)]), &*media);
    assert_eq!(r.svg.matches("<image ").count(), 1, "{}", r.svg);
    assert!(
        r.svg.contains(r#"preserveAspectRatio="xMidYMid meet""#),
        "{}",
        r.svg
    );
    assert!(!r.svg.contains(r#"preserveAspectRatio="none""#));
    // a picture of known size keeps the stretching image and the fit is the writer's
    let media = by_key(vec![("p", png(8, 4, |_, _| [255, 0, 0, 255]))]);
    let mut para = Paragraph {
        runs: vec![Run::text("x", 24.0)],
        ..Default::default()
    };
    para.bullet = Some(Bullet {
        kind: BulletKind::Picture(ImageFill::stretch("p".to_string())),
        font: None,
        color: None,
        size: BulletSize::Pct(1.0),
    });
    let body = TextBody {
        paragraphs: vec![para],
        ..Default::default()
    };
    let mut s = ShapeItem::new(
        Xfrm::rect(e(0.0), e(0.0), e(300.0), e(100.0)),
        Geometry::Rect,
    );
    s.text = Some(body);
    let r = render_svg(&scene(vec![Item::Shape(s)]), &*media);
    assert!(r.svg.contains(r#"preserveAspectRatio="none""#));
    assert!(!r.svg.contains("meet"));
}

// ---- 9. what is measured: visible outlines and the layers of effects -----------------------------------------

#[test]
fn a_line_counts_for_the_part_of_it_that_is_inside_the_window() {
    use super::svg::visible_len;
    let w = Some((0.0, 0.0, 100.0, 50.0));
    let at = |x: f64, y: f64| Pt::new(x, y);
    // inside: all of it; outside on one side: none; across: the part inside
    assert!((visible_len(at(10.0, 10.0), at(40.0, 50.0), w, 1e9) - 50.0).abs() < 1e-6);
    assert_eq!(visible_len(at(-50.0, 10.0), at(-10.0, 40.0), w, 1e9), 0.0);
    assert_eq!(
        visible_len(at(-100.0, 25.0), at(200.0, 25.0), w, 1e9),
        100.0
    );
    assert!((visible_len(at(50.0, -1e9), at(50.0, 1e9), w, 1e9) - 50.0).abs() < 1e-3);
    // along an edge of the window, and just outside it
    assert_eq!(visible_len(at(0.0, 0.0), at(100.0, 0.0), w, 1e9), 100.0);
    assert_eq!(visible_len(at(0.0, -0.001), at(100.0, -0.001), w, 1e9), 0.0);
    // no window: the length up to the cap; not a number: the cap; no length: none
    assert_eq!(visible_len(at(0.0, 0.0), at(3.0, 4.0), None, 1e9), 5.0);
    assert_eq!(visible_len(at(0.0, 0.0), at(3.0, 4.0), None, 2.0), 2.0);
    assert_eq!(visible_len(at(0.0, 0.0), at(f64::NAN, 4.0), None, 7.0), 7.0);
    assert_eq!(
        visible_len(at(0.0, 0.0), at(f64::INFINITY, 4.0), w, 7.0),
        7.0
    );
    assert_eq!(visible_len(at(5.0, 5.0), at(5.0, 5.0), w, 1e9), 0.0);
    // the cap applies to the visible part too
    assert_eq!(
        visible_len(at(-100.0, 25.0), at(200.0, 25.0), w, 30.0),
        30.0
    );
}

#[test]
fn the_outline_of_a_polygon_is_its_closed_control_polygon_inside_the_window() {
    use super::svg::polyline_px;
    let e = |v: f64| v * EMU_PER_PX;
    let sq = |a: f64, b: f64| {
        vec![
            Seg::M(Pt::new(e(a), e(a))),
            Seg::L(Pt::new(e(b), e(a))),
            Seg::L(Pt::new(e(b), e(b))),
            Seg::L(Pt::new(e(a), e(b))),
            Seg::Z,
        ]
    };
    let win = Some((0.0, 0.0, e(1000.0), e(1000.0)));
    assert!((polyline_px(&sq(10.0, 20.0), win, 1e9) - 40.0).abs() < 1e-9);
    // a square far outside, and one only half inside
    assert_eq!(polyline_px(&sq(2000.0, 3000.0), win, 1e9), 0.0);
    assert!(polyline_px(&sq(900.0, 1100.0), win, 1e9) < 0.5 * 800.0 + 1.0);
    // a curve counts by its control polygon
    let c = vec![
        Seg::M(Pt::new(0.0, 0.0)),
        Seg::C(
            Pt::new(e(30.0), 0.0),
            Pt::new(e(30.0), e(40.0)),
            Pt::new(e(30.0), e(80.0)),
        ),
        Seg::Q(Pt::new(e(30.0), e(100.0)), Pt::new(e(60.0), e(100.0))),
    ];
    let len = polyline_px(&c, win, 1e9);
    assert!(
        (len - (30.0 + 40.0 + 40.0 + 20.0 + 30.0)).abs() < 1e-9,
        "{len}"
    );
    assert_eq!(polyline_px(&[], win, 1e9), 0.0);
}

fn long_line_shape(x: f64, rot: f64) -> Item {
    let mut cmds = vec![PathCmd::MoveTo(Pt::new(0.0, 0.0))];
    for i in 0..2000 {
        cmds.push(PathCmd::LineTo(Pt::new(
            e(if i % 2 == 0 { 100.0 } else { 0.0 }),
            e(10.0 + i as f64 / 10.0),
        )));
    }
    let path = GeomPath {
        w: 0.0,
        h: 0.0,
        fill_mode: PathFill::None,
        stroke: true,
        cmds,
    };
    let mut xf = Xfrm::rect(e(x), 0.0, e(100.0), e(300.0));
    xf.rot_deg = rot;
    let mut s = ShapeItem::new(xf, Geometry::Paths(vec![path]));
    s.line = Some(Line::solid(e(1.0), Rgba::BLACK));
    Item::Shape(s)
}

#[test]
fn outlines_off_the_slide_cost_nothing_where_the_writer_knows_where_they_are() {
    let on = render_svg(&scene(vec![long_line_shape(100.0, 0.0)]), &no_media);
    let off = render_svg(&scene(vec![long_line_shape(5000.0, 0.0)]), &no_media);
    assert!(on.features.edge_px > 1e5, "{:?}", on.features);
    assert_eq!(off.features.edge_px, 0.0, "{:?}", off.features);
    // a turned shape's coordinates are not the slide's: it is measured whole
    let turned = render_svg(&scene(vec![long_line_shape(5000.0, 30.0)]), &no_media);
    assert!(turned.features.edge_px > 1e5, "{:?}", turned.features);
    // so is a shape inside a group
    let g = GroupItem {
        xfrm: Xfrm::rect(0.0, 0.0, e(960.0), e(540.0)),
        child_off: (0.0, 0.0),
        child_ext: (e(960.0), e(540.0)),
        items: vec![long_line_shape(5000.0, 0.0)],
    };
    let grouped = render_svg(&scene(vec![Item::Group(g)]), &no_media);
    assert!(grouped.features.edge_px > 1e5, "{:?}", grouped.features);
}

#[test]
fn the_layers_of_effects_repeat_what_a_shape_paints() {
    let plain = render_svg(
        &scene(vec![fx_shape(100.0, 50.0, Effects::default())]),
        &no_media,
    );
    let mut fx = shadow(4.0);
    fx.glow = Some(Glow {
        rad: e(5.0),
        color: Rgba::new(255, 0, 0, 0.5),
    });
    let layered = render_svg(&scene(vec![fx_shape(100.0, 50.0, fx)]), &no_media);
    // itself, the shadow and the glow
    assert_eq!(layered.features.segs, 3.0 * plain.features.segs);
    assert!((layered.features.fill_px2 - 3.0 * plain.features.fill_px2).abs() < 1e-6);
    assert!((layered.features.edge_px - 3.0 * plain.features.edge_px).abs() < 1e-6);
    assert!(layered.features.filter_units > 0.0);
}

// ---- 10. pictures the drawing process would refuse the slide for ------------------------------------------------

#[test]
fn a_picture_whose_header_claims_billions_of_pixels_is_a_placeholder_not_a_slide_the_process_refuses(
) {
    // A header of 2 147 483 647 pixels a side that the image decoder cannot read: the process
    // reads it by the header alone and refuses the whole slide; the writer used to count it as
    // no pixels at all.
    let mut hostile = png_header_only(8, 8);
    hostile[16..20].copy_from_slice(&0x7fff_ffffu32.to_be_bytes());
    hostile[20..24].copy_from_slice(&0x7fff_ffffu32.to_be_bytes());
    let mut gif = b"GIF89a".to_vec();
    gif.extend_from_slice(&[0xff, 0xff, 0xff, 0xff, 0, 0, 0]);
    let media = by_key(vec![
        ("hostile", hostile),
        ("gif", gif),
        ("junk", b"\x89PNG\r\n\x1a\n not a png".to_vec()),
    ]);
    let r = render_svg(
        &scene(vec![
            pic("hostile", 0.0, 0.0, 100.0, 100.0),
            pic("gif", 110.0, 0.0, 100.0, 100.0),
            pic("junk", 220.0, 0.0, 100.0, 100.0),
        ]),
        &*media,
    );
    assert!(r.truncated);
    // the two that claim too much are placeholders; the one that claims nothing is left to the
    // process, which skips what it cannot decode
    assert_eq!(r.svg.matches("#e6e6e6").count(), 2);
    assert_eq!(r.svg.matches("<image ").count(), 1);
    assert!(guard_accepts(&r));
}

#[test]
fn the_size_a_header_claims_is_read_for_png_and_gif_only() {
    assert_eq!(super::svg::claimed_px(&png_header_only(30, 20)), Some(600));
    let mut gif = b"GIF87a".to_vec();
    gif.extend_from_slice(&[7, 0, 5, 0, 0, 0, 0]);
    assert_eq!(super::svg::claimed_px(&gif), Some(35));
    for short in [
        &b"\x89PNG\r\n\x1a\nIHDR"[..],
        b"GIF89a\x01",
        b"",
        b"RIFF....WEBP",
        b"\xff\xd8\xff\xe0",
    ] {
        assert_eq!(super::svg::claimed_px(short), None);
    }
}

fn blurred_svg(side: u32) -> Vec<u8> {
    format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{side}" height="{side}"><filter id="f"><feGaussianBlur stdDeviation="{}"/></filter><rect width="{side}" height="{side}" filter="url(#f)"/></svg>"#,
        side / 2
    )
    .into_bytes()
}

#[test]
fn filters_inside_an_svg_picture_count_towards_the_work_the_process_will_do() {
    // The process measures a picture's filters in the picture's own units, whatever box it is
    // placed in, and counts them twice (as the image and again among its sub-trees).
    let media = by_key(vec![
        ("small", blurred_svg(2000)),
        ("big", blurred_svg(100_000)),
    ]);
    let r = render_svg(&scene(vec![pic("small", 0.0, 0.0, 200.0, 200.0)]), &*media);
    assert!(r.filter_work > 0.0, "the picture's blur is counted");
    let guard = guard_work(&r);
    assert!(
        (r.filter_work - guard).abs() <= 0.02 * guard,
        "model {:.4e} against the process's {guard:.4e}",
        r.filter_work
    );
    assert!(!r.truncated && r.svg.contains("<use "));
    assert!(guard_accepts(&r));
    // the raster it may be drawn at keeps it inside the process's budget at every size
    let cap = f64::from(r.max_raster_px());
    assert!(cap >= r.model_px && cap < 4096.0);
    assert!(guard_accepts_at(&r, cap), "accepted at {cap}");
    assert!(!guard_accepts_at(&r, 4096.0), "the cap is not for nothing");
    // a picture that declares itself 100 000 units wide is more than the allowance on its own:
    // it is left out (the process refused the whole slide for it), the rest is drawn
    let r = render_svg(
        &scene(vec![
            pic("big", 0.0, 0.0, 200.0, 200.0),
            rect_item(300.0, 0.0, 50.0, 50.0, RED),
        ]),
        &*media,
    );
    assert!(r.truncated);
    assert_eq!(r.filter_work, 0.0);
    assert!(!r.svg.contains("<use "));
    assert!(r.svg.contains(&hex6(RED)), "the rest of the slide is drawn");
    assert!(guard_accepts(&r));
}

// ---- 11. ordinary metafile pictures are not left out ---------------------------------------------

/// The shape of what the metafile converter makes of a table drawn by an Office program (a WMF
/// preview of an OLE object, 38 KB): a picture thousands of units wide, a white background, and
/// 130 small texts and some boxes, each text in a group of its own.
fn table_like_metafile_svg(clip_each: bool) -> Vec<u8> {
    let mut s = String::from(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 2938 2203" width="2938" height="2203" preserveAspectRatio="none">"#,
    );
    if clip_each {
        s.push_str("<defs>");
        for i in 0..56 {
            s.push_str(&format!(
                r#"<clipPath id="c{i}"><path d="M7 7H2930V2195H7Z"/></clipPath>"#
            ));
        }
        s.push_str("</defs>");
    }
    s.push_str(r##"<path d="M0 0L2938 0L2938 2203L0 2203Z" fill="#ffffff" fill-rule="evenodd"/>"##);
    for i in 0..136 {
        let clip = if clip_each {
            format!(r#" clip-path="url(#c{})""#, i % 56)
        } else {
            String::new()
        };
        s.push_str(&format!(
            r##"<g transform="translate({} {}) rotate(0)"{clip}><text xml:space="preserve" x="0" y="0" font-family="'Arial', sans-serif" font-size="41" fill="#333399" font-weight="700">cell {i}</text></g>"##,
            1286 + (i % 5) * 300,
            100 + (i / 5) * 70
        ));
    }
    for i in 0..30 {
        s.push_str(&format!(
            r##"<path d="M{} 814L{} 814L{} 1745L{} 1745Z" fill="none" stroke="#ff0000" stroke-width="12" stroke-linecap="round" stroke-linejoin="round"/>"##,
            100 + i * 10,
            700 + i * 10,
            700 + i * 10,
            100 + i * 10
        ));
    }
    s.push_str("</svg>");
    s.into_bytes()
}

#[test]
fn a_metafile_sized_table_picture_is_drawn_not_left_out() {
    // Each of its texts used to count as a whole picture painted over (130 times the area).
    for clip_each in [false, true] {
        let media = by_key(vec![("m", table_like_metafile_svg(clip_each))]);
        let r = render_svg(&scene(vec![pic("m", 50.0, 50.0, 860.0, 440.0)]), &*media);
        assert!(!r.truncated, "clips: {clip_each}");
        assert!(r.svg.contains("<use "), "the picture is drawn");
        assert!(
            predicted_ms(&r) < 300.0,
            "{:.0} ms for 130 small texts: {:?}",
            predicted_ms(&r),
            r.features
        );
        // a full-slide copy of it, a few times over, is still inside the budget
        let items: Vec<Item> = (0..4)
            .map(|i| pic("m", 0.0, i as f64, 960.0, 540.0))
            .collect();
        let r = render_svg(&scene(items), &*media);
        assert!(!r.truncated, "four full-slide copies, clips: {clip_each}");
    }
}

#[test]
fn what_a_clip_path_holds_does_not_make_a_picture_dearer() {
    // The same elements, once painted and once inside a clip: only the first paints.
    let painted = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1000 1000"><path d="M0 0H1000V1000H0Z"/><path d="M0 0H1000V1000H0Z"/></svg>"#.to_vec();
    let clipped = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1000 1000"><clipPath id="a"><path d="M0 0H1000V1000H0Z"/><path d="M0 0H1000V1000H0Z"/></clipPath></svg>"#.to_vec();
    let media = by_key(vec![("p", painted), ("c", clipped)]);
    let p = render_svg(&scene(vec![pic("p", 0.0, 0.0, 960.0, 540.0)]), &*media);
    let c = render_svg(&scene(vec![pic("c", 0.0, 0.0, 960.0, 540.0)]), &*media);
    assert!(p.features.vec_px2 > 0.0);
    assert_eq!(c.features.vec_px2, 0.0);
    assert!(
        c.features.segs >= 10.0,
        "but its segments are still counted"
    );
}
