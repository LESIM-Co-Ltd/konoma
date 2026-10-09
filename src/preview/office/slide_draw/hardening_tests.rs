//! Resource-safety tests of the SVG writer: every picture is written once, the budgets fit what
//! the drawing process accepts, effects and conversions are bounded and cached, and a render
//! stops when it is told it is not wanted. Each test is the small form of a hostile deck that
//! used to cost the parent seconds, gigabytes or the child its life.

use std::cell::Cell;
use std::sync::Arc;

use super::tests::{at, e, no_media, png, raster, rect_item, scene, RED};
use super::*;

type Media = Arc<dyn Fn(&str) -> Option<Arc<Vec<u8>>>>;

/// A picture that is red on the left half and blue on the right half.
fn halves() -> Vec<u8> {
    png(4, 2, |x, _| {
        if x < 2 {
            [255, 0, 0, 255]
        } else {
            [0, 0, 255, 255]
        }
    })
}

/// Incompressible RGB noise as a PNG of about `w * h * 3` bytes.
fn noise_png(w: u32, h: u32, seed: u64) -> Vec<u8> {
    let mut state = seed | 1;
    let mut img = image::RgbImage::new(w, h);
    for p in img.pixels_mut() {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        *p = image::Rgb([state as u8, (state >> 8) as u8, (state >> 16) as u8]);
    }
    let mut out = Vec::new();
    image::DynamicImage::ImageRgb8(img)
        .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
        .unwrap();
    out
}

fn by_key(files: Vec<(&'static str, Vec<u8>)>) -> Media {
    let files: Vec<(String, Arc<Vec<u8>>)> = files
        .into_iter()
        .map(|(k, v)| (k.to_string(), Arc::new(v)))
        .collect();
    Arc::new(move |k: &str| files.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone()))
}

fn pic(key: &str, x: f64, y: f64, w: f64, h: f64) -> Item {
    Item::Picture(PictureItem::new(
        Xfrm::rect(e(x), e(y), e(w), e(h)),
        key.to_string(),
    ))
}

// ---- each picture once (M2) --------------------------------------------------------------------

#[test]
fn a_picture_used_many_times_is_written_once() {
    let media = by_key(vec![("p", halves())]);
    let items: Vec<Item> = (0..200)
        .map(|i| {
            pic(
                "p",
                (i % 20) as f64 * 40.0,
                (i / 20) as f64 * 40.0,
                36.0,
                36.0,
            )
        })
        .collect();
    let r = render_svg(&scene(items), &*media);
    assert_eq!(r.svg.matches("<image ").count(), 1);
    assert_eq!(r.svg.matches("<use ").count(), 200);
    assert!(!r.truncated);
    // the bytes are in the markup once
    assert_eq!(r.svg.matches("base64,").count(), 1);
}

#[test]
fn a_shared_picture_is_placed_by_each_use_with_its_own_size_crop_clip_and_alpha() {
    let media = by_key(vec![("p", halves())]);
    // 1. big, plain
    let big = pic("p", 0.0, 0.0, 200.0, 100.0);
    // 2. small, plain
    let small = pic("p", 300.0, 0.0, 40.0, 20.0);
    // 3. cropped to the right half (blue only), clipped to an ellipse
    let mut cropped = PictureItem::new(
        Xfrm::rect(e(400.0), e(0.0), e(100.0), e(100.0)),
        "p".to_string(),
    );
    cropped.image.crop = (0.5, 0.0, 0.0, 0.0);
    cropped.geom = Geometry::Ellipse;
    // 4. half transparent over white
    let mut faint = PictureItem::new(
        Xfrm::rect(e(600.0), e(0.0), e(100.0), e(100.0)),
        "p".to_string(),
    );
    faint.image.alpha = 0.5;
    let r = render_svg(
        &scene(vec![
            big,
            small,
            Item::Picture(cropped),
            Item::Picture(faint),
        ]),
        &*media,
    );
    assert_eq!(r.svg.matches("<image ").count(), 1);
    let img = raster(&r);
    // big: red left half, blue right half
    assert_eq!(at(&img, 50, 50), [255, 0, 0]);
    assert_eq!(at(&img, 150, 50), [0, 0, 255]);
    // small: the same split at its own size
    assert_eq!(at(&img, 310, 10), [255, 0, 0]);
    assert_eq!(at(&img, 330, 10), [0, 0, 255]);
    // cropped: all blue where the ellipse is, the corner of its box is not painted
    assert_eq!(at(&img, 450, 50), [0, 0, 255]);
    assert_eq!(at(&img, 403, 3), [255, 255, 255]);
    // faint: blended with the page
    let p = at(&img, 680, 50);
    assert!(p[0] > 100 && p[0] < 160 && p[2] > 200, "{p:?}");
}

#[test]
fn a_shared_picture_mirrors_and_tiles_like_before() {
    let media = by_key(vec![("p", halves())]);
    // a tile fill with X mirroring: the tile is 4 x 2 px at scale 10 = 40 x 20, mirrored
    let mut shape = ShapeItem::new(
        Xfrm::rect(e(0.0), e(0.0), e(160.0), e(20.0)),
        Geometry::Rect,
    );
    let mut fill = ImageFill::stretch("p".to_string());
    fill.mode = ImageMode::Tile {
        sx: 10.0,
        sy: 10.0,
        tx: 0.0,
        ty: 0.0,
        align: RectAlign::TopLeft,
        flip: TileFlip::X,
    };
    shape.fill = Fill::Image(fill);
    let r = render_svg(&scene(vec![Item::Shape(shape)]), &*media);
    assert_eq!(r.svg.matches("<image ").count(), 1);
    let img = raster(&r);
    // 0..20 red, 20..40 blue, then mirrored: 40..60 blue, 60..80 red
    assert_eq!(at(&img, 10, 10), [255, 0, 0]);
    assert_eq!(at(&img, 30, 10), [0, 0, 255]);
    assert_eq!(at(&img, 50, 10), [0, 0, 255]);
    assert_eq!(at(&img, 70, 10), [255, 0, 0]);
}

#[test]
fn a_picture_bullet_is_fitted_into_its_square() {
    // the geometry of a bullet picture is made by the text layout; the fit is the writer's
    let media = by_key(vec![("p", png(8, 4, |_, _| [255, 0, 0, 255]))]);
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
    assert!(r.svg.contains("<use "), "the bullet is a use");
    let img = raster(&r);
    // some red is drawn left of the text, and it is twice as wide as tall (8 x 4)
    let (mut x0, mut x1, mut y0, mut y1) = (u32::MAX, 0, u32::MAX, 0);
    for y in 0..100 {
        for x in 0..60 {
            let p = at(&img, x, y);
            if p[0] > 200 && p[1] < 60 && p[2] < 60 {
                x0 = x0.min(x);
                x1 = x1.max(x);
                y0 = y0.min(y);
                y1 = y1.max(y);
            }
        }
    }
    assert!(x1 > x0 && y1 > y0, "a red bullet");
    let (w, h) = (f64::from(x1 - x0 + 1), f64::from(y1 - y0 + 1));
    assert!((w / h - 2.0).abs() < 0.3, "{w} x {h}");
}

// ---- colour effects: cached, budgeted (H3) ----------------------------------------------------------

fn gray_fx() -> Vec<PicFx> {
    vec![PicFx::Grayscale]
}

#[test]
fn a_picture_with_effects_is_recoloured_once_however_often_it_is_used() {
    let media = by_key(vec![("p", halves())]);
    let items: Vec<Item> = (0..100)
        .map(|i| {
            let mut p = PictureItem::new(
                Xfrm::rect(
                    e((i % 10) as f64 * 90.0),
                    e((i / 10) as f64 * 50.0),
                    e(80.0),
                    e(40.0),
                ),
                "p".to_string(),
            );
            p.image.fx = gray_fx();
            Item::Picture(p)
        })
        .collect();
    let r = render_svg(&scene(items), &*media);
    assert_eq!(r.svg.matches("<image ").count(), 1);
    assert_eq!(r.svg.matches("data:image/png").count(), 1);
    // and it is grey
    let img = raster(&r);
    let p = at(&img, 20, 20);
    assert!(p[0] == p[1] && p[1] == p[2], "{p:?}");
}

#[test]
fn the_same_picture_with_other_effects_is_another_entry() {
    let media = by_key(vec![("p", halves())]);
    let mk = |fx: Vec<PicFx>, x: f64| {
        let mut p = PictureItem::new(Xfrm::rect(e(x), e(0.0), e(80.0), e(40.0)), "p".to_string());
        p.image.fx = fx;
        Item::Picture(p)
    };
    let r = render_svg(
        &scene(vec![
            mk(vec![], 0.0),
            mk(gray_fx(), 100.0),
            mk(gray_fx(), 200.0),
            mk(vec![PicFx::BiLevel { thresh: 0.5 }], 300.0),
        ]),
        &*media,
    );
    assert_eq!(r.svg.matches("<image ").count(), 3);
}

#[test]
fn colour_effects_are_dropped_past_the_decode_allowance_and_the_result_says_so() {
    // 40 distinct pictures of 100 x 100 with effects against an allowance of 10 pictures
    let files: Vec<(&'static str, Vec<u8>)> = vec![("a", png(100, 100, |_, _| [255, 0, 0, 255]))];
    let media = by_key(files);
    // distinct keys resolving to the same bytes
    let media2 = move |k: &str| {
        let _ = k;
        media("a")
    };
    let items: Vec<Item> = (0..40)
        .map(|i| {
            let mut p = PictureItem::new(
                Xfrm::rect(
                    e((i % 10) as f64 * 90.0),
                    e((i / 10) as f64 * 50.0),
                    e(80.0),
                    e(40.0),
                ),
                format!("k{i}"),
            );
            p.image.fx = gray_fx();
            Item::Picture(p)
        })
        .collect();
    let r = super::svg::render_with(&scene(items), &media2, &|| false, 10 * 100 * 100, u64::MAX);
    assert!(r.truncated);
    let img = raster(&r);
    // the first row is grey (effects applied), the last is still red (drawn as it is)
    let first = at(&img, 20, 20);
    assert!(first[0] == first[1] && first[1] == first[2], "{first:?}");
    assert_eq!(at(&img, 20, 170), [255, 0, 0]);
}

#[test]
fn recolouring_a_huge_picture_many_times_is_fast() {
    // 100 uses of a picture that would take seconds to recolour 100 times: once is enough
    let media = by_key(vec![(
        "p",
        png(1500, 1500, |x, y| {
            [(x % 251) as u8, (y % 241) as u8, 9, 255]
        }),
    )]);
    let items: Vec<Item> = (0..100)
        .map(|i| {
            let mut p = PictureItem::new(
                Xfrm::rect(
                    e((i % 10) as f64 * 90.0),
                    e((i / 10) as f64 * 50.0),
                    e(80.0),
                    e(40.0),
                ),
                "p".to_string(),
            );
            p.image.fx = vec![PicFx::Hsl {
                hue: 30.0,
                sat: 0.1,
                lum: 0.0,
            }];
            Item::Picture(p)
        })
        .collect();
    let t = std::time::Instant::now();
    let r = render_svg(&scene(items), &*media);
    assert_eq!(r.svg.matches("<image ").count(), 1);
    // (a debug build recolours 1500 x 1500 in a few seconds; 100 times would be minutes)
    assert!(
        t.elapsed() < std::time::Duration::from_secs(60),
        "{:?}",
        t.elapsed()
    );
}

// ---- the pixels the drawing process will decode ----------------------------------------------------------

#[test]
fn pictures_used_past_the_pixel_allowance_become_placeholders() {
    // each use is 100 x 100 = 10 000 px; the allowance is 5 uses
    let media = by_key(vec![("p", png(100, 100, |_, _| [0, 128, 0, 255]))]);
    let items: Vec<Item> = (0..12)
        .map(|i| {
            pic(
                "p",
                (i % 6) as f64 * 100.0,
                (i / 6) as f64 * 100.0,
                90.0,
                90.0,
            )
        })
        .collect();
    let r = super::svg::render_with(&scene(items), &*media, &|| false, u64::MAX, 5 * 10_000);
    assert!(r.truncated);
    assert_eq!(r.svg.matches("<use ").count(), 5);
    assert_eq!(
        r.svg.matches("#e6e6e6").count(),
        7,
        "the rest are placeholders"
    );
}

#[test]
fn a_shape_with_effects_counts_its_picture_in_each_layer() {
    let media = by_key(vec![("p", png(100, 100, |_, _| [0, 128, 0, 255]))]);
    let mk = |effects: Effects| {
        let mut p = PictureItem::new(
            Xfrm::rect(e(10.0), e(10.0), e(90.0), e(90.0)),
            "p".to_string(),
        );
        p.effects = effects;
        Item::Picture(p)
    };
    let shadow = Effects {
        outer_shadow: Some(Shadow {
            blur_rad: e(4.0),
            dist: e(4.0),
            dir_deg: 45.0,
            color: Rgba::new(0, 0, 0, 0.5),
            sx: 1.0,
            sy: 1.0,
            rot_with_shape: true,
        }),
        glow: Some(Glow {
            rad: e(4.0),
            color: Rgba::new(255, 0, 0, 0.5),
        }),
        ..Default::default()
    };
    // three layers (shape, shadow, glow) of 10 000 px = 30 000: an allowance of 25 000 refuses
    let r = super::svg::render_with(
        &scene(vec![mk(shadow)]),
        &*media,
        &|| false,
        u64::MAX,
        25_000,
    );
    assert!(r.truncated && r.svg.contains("#e6e6e6"));
    let r = super::svg::render_with(
        &scene(vec![mk(shadow)]),
        &*media,
        &|| false,
        u64::MAX,
        30_000,
    );
    assert!(!r.truncated && !r.svg.contains("#e6e6e6"));
    let r = super::svg::render_with(
        &scene(vec![mk(Effects::default())]),
        &*media,
        &|| false,
        u64::MAX,
        10_000,
    );
    assert!(!r.truncated);
}

// ---- budgets agree with the drawing process (M3) -----------------------------------------------------

#[test]
fn photos_that_do_not_fit_are_reduced_and_the_svg_stays_under_the_childs_limit() {
    // four incompressible pictures of about 3.2 MB each: 13 MB, over the 12 MiB the slide embeds
    let media = by_key(vec![
        ("a", noise_png(1200, 900, 1)),
        ("b", noise_png(1200, 900, 2)),
        ("c", noise_png(1200, 900, 3)),
        ("d", noise_png(1200, 900, 4)),
    ]);
    let items = vec![
        pic("a", 0.0, 0.0, 400.0, 300.0),
        pic("b", 450.0, 0.0, 400.0, 300.0),
        pic("c", 0.0, 320.0, 400.0, 200.0),
        pic("d", 450.0, 320.0, 400.0, 200.0),
    ];
    let r = render_svg(&scene(items), &*media);
    assert!(r.truncated, "a reduced picture is reported");
    assert!(
        r.svg.len() < crate::preview::svg_guard::MAX_SVG_BYTES,
        "{} bytes",
        r.svg.len()
    );
    assert_eq!(r.svg.matches("<image ").count(), 4, "all four are shown");
    assert!(!r.svg.contains("#e6e6e6"), "no placeholder");
    assert!(
        r.svg.contains("data:image/jpeg"),
        "the later ones were re-encoded"
    );
    // and the picture is the picture: noise is noise, but it is drawn where it belongs
    let img = raster(&r);
    assert_ne!(at(&img, 200, 150), [255, 255, 255]);
    assert_ne!(at(&img, 650, 450), [255, 255, 255]);
}

#[test]
fn the_largest_possible_svg_is_under_the_childs_limit_with_a_margin() {
    let most =
        super::svg::MAX_SVG_TEXT_BYTES + super::svg::MAX_EMBEDDED_IMAGE_BYTES.div_ceil(3) * 4;
    assert!(
        most <= crate::preview::svg_guard::MAX_SVG_BYTES / 4 * 3,
        "{most}"
    );
}

#[test]
fn a_vector_picture_that_does_not_fit_is_a_placeholder() {
    let mut svg =
        String::from(r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><!--"#);
    svg.push_str(&"x".repeat(13 * 1024 * 1024));
    svg.push_str("--></svg>");
    let media = by_key(vec![("v", svg.into_bytes())]);
    let r = render_svg(&scene(vec![pic("v", 10.0, 10.0, 100.0, 100.0)]), &*media);
    assert!(r.truncated);
    assert!(r.svg.contains("#e6e6e6"));
    assert!(r.svg.len() < 100_000);
}

#[test]
fn a_raster_that_cannot_be_reduced_is_a_placeholder() {
    // a PNG signature and a header that claims a picture, followed by padding: it passes for a
    // PNG, does not fit, and cannot be decoded to reduce it
    let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
    bytes.resize(13 * 1024 * 1024, 7);
    let media = by_key(vec![("g", bytes)]);
    let r = render_svg(&scene(vec![pic("g", 10.0, 10.0, 100.0, 100.0)]), &*media);
    assert!(r.truncated && r.svg.contains("#e6e6e6"));
    assert!(r.svg.len() < 100_000);
}

// ---- long paths (M1) ---------------------------------------------------------------------------------------

fn arc_shape(n: usize, sw: f64) -> Item {
    let mut cmds = vec![PathCmd::MoveTo(Pt::new(50.0, 100.0))];
    for _ in 0..n {
        cmds.push(PathCmd::ArcTo {
            wr: 50.0,
            hr: 50.0,
            st_deg: 180.0,
            sw_deg: sw,
        });
    }
    let g = Geometry::Paths(vec![GeomPath {
        w: 100.0,
        h: 100.0,
        stroke: true,
        cmds,
        ..Default::default()
    }]);
    let mut s = ShapeItem::new(Xfrm::rect(e(0.0), e(0.0), e(100.0), e(100.0)), g);
    s.fill = Fill::Solid(RED);
    s.line = Some(Line::solid(e(1.0), Rgba::BLACK));
    Item::Shape(s)
}

#[test]
fn a_hundred_thousand_arcs_make_a_bounded_svg_quickly() {
    let t = std::time::Instant::now();
    let r = render_svg(&scene(vec![arc_shape(100_000, 3600.0)]), &no_media);
    assert!(r.truncated);
    assert!(
        r.svg.len() <= super::svg::MAX_SVG_TEXT_BYTES + 1_000_000,
        "{} bytes",
        r.svg.len()
    );
    assert!(r.svg.ends_with("</svg>"));
    assert!(
        t.elapsed() < std::time::Duration::from_secs(20),
        "{:?}",
        t.elapsed()
    );
}

#[test]
fn the_markup_of_one_path_stops_at_the_text_budget() {
    // 190 000 cubic segments in ONE shape: under the segment limit but 11 MB of `d` for the
    // fill and as much again for the outline, all inside one item (nothing between items could
    // stop it)
    let mut cmds = vec![PathCmd::MoveTo(Pt::new(0.0, 0.0))];
    for i in 0..190_000 {
        let v = f64::from(i % 97) * 1.234567;
        let w = f64::from(i % 89) * 1.7654321;
        cmds.push(PathCmd::CubicTo(
            Pt::new(v, w),
            Pt::new(w + 1.111, v + 2.222),
            Pt::new(v + 3.333, w + 4.444),
        ));
    }
    let g = Geometry::Paths(vec![GeomPath {
        w: 100.0,
        h: 100.0,
        stroke: true,
        cmds,
        ..Default::default()
    }]);
    let mut s = ShapeItem::new(Xfrm::rect(e(0.0), e(0.0), e(100.0), e(100.0)), g);
    s.fill = Fill::Solid(RED);
    s.line = Some(Line::solid(e(1.0), Rgba::BLACK));
    let r = render_svg(&scene(vec![Item::Shape(s)]), &no_media);
    assert!(r.truncated);
    assert!(
        r.svg.len() <= super::svg::MAX_SVG_TEXT_BYTES + 1_000_000,
        "{}",
        r.svg.len()
    );
    assert!(r.svg.ends_with("</svg>"));
}

#[test]
fn a_shape_of_many_paths_shares_one_segment_allowance() {
    let one = GeomPath {
        w: 100.0,
        h: 100.0,
        cmds: vec![PathCmd::LineTo(Pt::new(1.0, 1.0)); 100_000],
        ..Default::default()
    };
    let g = Geometry::Paths(vec![one; 8]);
    let (paths, truncated) = super::path::resolve_geometry(&g, &Xfrm::rect(0.0, 0.0, 1.0, 1.0));
    let total: usize = paths.iter().map(|(r, _, _)| r.segs.len()).sum();
    assert_eq!(total, super::path::MAX_PATH_CMDS);
    assert!(truncated);
}

// ---- filters (M5) -------------------------------------------------------------------------------------------

#[test]
fn shapes_past_the_filter_area_allowance_are_drawn_without_effects() {
    let shadow = Effects {
        outer_shadow: Some(Shadow {
            blur_rad: e(10.0),
            dist: e(5.0),
            dir_deg: 45.0,
            color: Rgba::new(0, 0, 0, 0.5),
            sx: 1.0,
            sy: 1.0,
            rot_with_shape: true,
        }),
        ..Default::default()
    };
    // a full-slide shadow is 1e8 units of work; 400 of them are far past the allowance (4.5e8)
    let items: Vec<Item> = (0..400)
        .map(|_| {
            let mut s = ShapeItem::new(
                Xfrm::rect(e(0.0), e(0.0), e(960.0), e(540.0)),
                Geometry::Rect,
            );
            s.fill = Fill::Solid(RED);
            s.effects = shadow;
            Item::Shape(s)
        })
        .collect();
    let r = render_svg(&scene(items), &no_media);
    assert!(r.truncated);
    let filters = r.svg.matches("<filter ").count();
    assert!((3..=10).contains(&filters), "{filters}");
}

#[test]
fn a_slide_of_ordinary_shadows_is_not_limited() {
    let shadow = Effects {
        outer_shadow: Some(Shadow {
            blur_rad: e(6.0),
            dist: e(4.0),
            dir_deg: 45.0,
            color: Rgba::new(0, 0, 0, 0.4),
            sx: 1.0,
            sy: 1.0,
            rot_with_shape: true,
        }),
        ..Default::default()
    };
    let items: Vec<Item> = (0..60)
        .map(|i| {
            let mut s = ShapeItem::new(
                Xfrm::rect(
                    e((i % 10) as f64 * 90.0),
                    e((i / 10) as f64 * 80.0),
                    e(80.0),
                    e(60.0),
                ),
                Geometry::Rect,
            );
            s.fill = Fill::Solid(RED);
            s.effects = shadow;
            Item::Shape(s)
        })
        .collect();
    let r = render_svg(&scene(items), &no_media);
    assert!(!r.truncated);
    assert_eq!(r.svg.matches("<filter ").count(), 60);
}

// ---- cancellation -------------------------------------------------------------------------------------------

#[test]
fn a_cancelled_render_stops_between_items_and_says_so() {
    let items: Vec<Item> = (0..5000)
        .map(|i| {
            rect_item(
                (i % 50) as f64 * 18.0,
                (i / 50) as f64 * 5.0,
                10.0,
                4.0,
                RED,
            )
        })
        .collect();
    let polls = Cell::new(0usize);
    let cancel = || {
        polls.set(polls.get() + 1);
        polls.get() > 50
    };
    let r = render_svg_cancellable(&scene(items), &no_media, &cancel);
    assert!(r.cancelled);
    assert!(polls.get() < 200, "it stopped soon after: {}", polls.get());
    assert!(r.svg.matches("<path").count() < 100);
    // not cancelled: every item, and the flag is off
    let items: Vec<Item> = (0..100)
        .map(|i| rect_item(i as f64 * 5.0, 0.0, 4.0, 4.0, RED))
        .collect();
    let r = render_svg_cancellable(&scene(items), &no_media, &|| false);
    assert!(!r.cancelled && !r.truncated);
}

#[test]
fn a_render_cancelled_from_the_start_does_no_picture_work() {
    let media = by_key(vec![(
        "p",
        png(1500, 1500, |x, y| {
            [(x % 251) as u8, (y % 241) as u8, 9, 255]
        }),
    )]);
    let mut p = PictureItem::new(
        Xfrm::rect(e(0.0), e(0.0), e(80.0), e(40.0)),
        "p".to_string(),
    );
    p.image.fx = gray_fx();
    let t = std::time::Instant::now();
    let r = render_svg_cancellable(&scene(vec![Item::Picture(p)]), &*media, &|| true);
    assert!(r.cancelled);
    assert!(!r.svg.contains("<image"));
    assert!(
        t.elapsed() < std::time::Duration::from_millis(500),
        "{:?}",
        t.elapsed()
    );
}

#[test]
fn a_render_cancelled_during_a_picture_effect_stops_inside_it() {
    // cancel after the picture has been handed over: the recolouring notices
    let media = by_key(vec![(
        "p",
        png(1500, 1500, |x, y| {
            [(x % 251) as u8, (y % 241) as u8, 9, 255]
        }),
    )]);
    let mut p = PictureItem::new(
        Xfrm::rect(e(0.0), e(0.0), e(80.0), e(40.0)),
        "p".to_string(),
    );
    p.image.fx = gray_fx();
    let polls = Cell::new(0usize);
    let cancel = || {
        polls.set(polls.get() + 1);
        polls.get() > 4
    };
    let r = render_svg_cancellable(&scene(vec![Item::Picture(p)]), &*media, &cancel);
    assert!(r.cancelled);
    assert!(!r.svg.contains("<image"));
}

#[test]
fn recolouring_stops_when_cancelled() {
    let bytes = png(64, 64, |_, _| [10, 20, 30, 255]);
    assert!(super::pic_fx::recolor_png_cancellable(&bytes, &gray_fx(), &|| false).is_some());
    assert!(super::pic_fx::recolor_png_cancellable(&bytes, &gray_fx(), &|| true).is_none());
    // cancelled only after the decode: still stops, before the effects
    let polls = Cell::new(0);
    let cancel = || {
        polls.set(polls.get() + 1);
        polls.get() >= 2
    };
    assert!(super::pic_fx::recolor_png_cancellable(&bytes, &gray_fx(), &cancel).is_none());
}

// ---- the filter model against the drawing process's own count --------------------------------------

/// The drawing process's estimate of the filter work of `r` at the model's raster size.
fn guard_work(r: &Rendered) -> f64 {
    let scale = (super::svg::MODEL_RASTER_PX / 960.0) as f32;
    crate::preview::svg_guard::load_tree(r.svg.as_bytes(), None, |tree| {
        Ok(crate::preview::svg_guard::filter_work_estimate(
            &tree, scale,
        ))
    })
    .expect("the SVG loads")
}

/// Whether the drawing process would draw `r` at the model's raster size (its work and memory
/// budget accepts the tree).
fn guard_accepts(r: &Rendered) -> bool {
    guard_accepts_at(r, super::svg::MODEL_RASTER_PX)
}

/// [`guard_accepts`] with the slide (960 px wide) rasterised at `px` on its longer side.
fn guard_accepts_at(r: &Rendered, px: f64) -> bool {
    let scale = (px / 960.0) as f32;
    crate::preview::svg_guard::load_tree(r.svg.as_bytes(), None, |tree| {
        Ok(crate::preview::svg_guard::check_render_budget(&tree, scale))
    })
    .expect("the SVG loads")
}

fn model_work() -> f64 {
    super::svg::LAST_FILTER_WORK.with(|c| c.get())
}

fn fx_shape(w: f64, h: f64, effects: Effects) -> Item {
    let mut s = ShapeItem::new(Xfrm::rect(e(20.0), e(20.0), e(w), e(h)), Geometry::Rect);
    s.fill = Fill::Solid(RED);
    s.effects = effects;
    Item::Shape(s)
}

fn shadow(blur: f64) -> Effects {
    Effects {
        outer_shadow: Some(Shadow {
            blur_rad: e(blur),
            dist: e(4.0),
            dir_deg: 45.0,
            color: Rgba::new(0, 0, 0, 0.5),
            sx: 1.0,
            sy: 1.0,
            rot_with_shape: true,
        }),
        ..Default::default()
    }
}

#[test]
fn the_filter_model_counts_what_the_drawing_process_counts() {
    let cases: Vec<(&str, Vec<Item>)> = vec![
        (
            "small shadow",
            (0..100)
                .map(|_| fx_shape(94.0, 42.0, shadow(4.0)))
                .collect(),
        ),
        (
            "big shadow",
            (0..3)
                .map(|_| fx_shape(900.0, 500.0, shadow(30.0)))
                .collect(),
        ),
        (
            "huge blur",
            (0..2)
                .map(|_| fx_shape(900.0, 500.0, shadow(420.0)))
                .collect(),
        ),
        (
            "glow",
            vec![fx_shape(
                300.0,
                200.0,
                Effects {
                    glow: Some(Glow {
                        rad: e(60.0),
                        color: Rgba::new(255, 0, 0, 0.5),
                    }),
                    ..Default::default()
                },
            )],
        ),
        (
            "soft edge",
            vec![fx_shape(
                300.0,
                200.0,
                Effects {
                    soft_edge: Some(e(30.0)),
                    ..Default::default()
                },
            )],
        ),
        (
            "inner shadow",
            vec![fx_shape(
                300.0,
                200.0,
                Effects {
                    inner_shadow: shadow(10.0).outer_shadow,
                    ..Default::default()
                },
            )],
        ),
        (
            "reflection",
            vec![fx_shape(
                300.0,
                100.0,
                Effects {
                    reflection: Some(Reflection {
                        blur_rad: e(5.0),
                        start_alpha: 0.5,
                        start_pos: 0.0,
                        end_alpha: 0.0,
                        end_pos: 0.9,
                        dist: 0.0,
                        dir_deg: 90.0,
                        fade_dir_deg: 90.0,
                        sx: 1.0,
                        sy: -1.0,
                    }),
                    ..Default::default()
                },
            )],
        ),
    ];
    for (name, items) in cases {
        let r = render_svg(&scene(items), &no_media);
        let (model, guard) = (model_work(), guard_work(&r));
        assert!(guard > 0.0, "{name}: the guard sees filters");
        assert!(
            (model - guard).abs() <= 0.02 * guard,
            "{name}: model {model:.4e} vs the drawing process {guard:.4e}"
        );
    }
}

#[test]
fn what_the_writer_emits_passes_the_drawing_process_however_many_effects_are_asked_for() {
    // 300 full-slide shadows are 30 times what the process accepts: the writer stops early and
    // what it wrote is accepted, where the same scene without the limit is refused
    let items: Vec<Item> = (0..300)
        .map(|_| fx_shape(900.0, 500.0, shadow(30.0)))
        .collect();
    let r = render_svg(&scene(items), &no_media);
    assert!(r.truncated);
    assert!(
        guard_accepts(&r),
        "the writer's budget must stay under the process's"
    );
    assert!(
        model_work() <= super::svg::MAX_FILTER_WORK * 1.3,
        "{:.3e}",
        model_work()
    );
}

#[test]
fn the_pixel_allowance_keeps_a_slide_under_what_the_drawing_process_decodes() {
    let media = by_key(vec![("p", png(1000, 1000, |_, _| [0, 128, 0, 255]))]);
    let items = || -> Vec<Item> {
        (0..100)
            .map(|i| {
                pic(
                    "p",
                    (i % 10) as f64 * 90.0,
                    (i / 10) as f64 * 50.0,
                    80.0,
                    40.0,
                )
            })
            .collect()
    };
    // 100 uses of 1 Mpx are 100 Mpx: the process refuses that (64 Mpx) ...
    let unlimited =
        super::svg::render_with(&scene(items()), &*media, &|| false, u64::MAX, u64::MAX);
    assert!(!unlimited.truncated);
    assert!(
        !guard_accepts(&unlimited),
        "the model is only worth anything if this is refused"
    );
    // ... and the writer keeps to 48 uses
    let r = render_svg(&scene(items()), &*media);
    assert!(r.truncated);
    assert_eq!(r.svg.matches("<use ").count(), 48);
    assert!(guard_accepts(&r));
}

#[test]
fn a_picture_inside_effects_is_counted_once_per_layer_like_the_drawing_process_does() {
    let media = by_key(vec![("p", png(1000, 1000, |_, _| [0, 128, 0, 255]))]);
    let mk = |i: usize| {
        let mut p = PictureItem::new(
            Xfrm::rect(
                e((i % 5) as f64 * 180.0),
                e((i / 5) as f64 * 100.0),
                e(150.0),
                e(90.0),
            ),
            "p".to_string(),
        );
        p.effects = shadow(3.0);
        Item::Picture(p)
    };
    // 40 pictures with a shadow: two layers each = 80 Mpx for the process
    let items: Vec<Item> = (0..40).map(mk).collect();
    let unlimited = super::svg::render_with(
        &scene(items.clone()),
        &*media,
        &|| false,
        u64::MAX,
        u64::MAX,
    );
    assert!(!guard_accepts(&unlimited));
    let r = render_svg(&scene(items), &*media);
    assert!(r.truncated && guard_accepts(&r));
}

// ---- compound-line masks and big SVG pictures -------------------------------------------------------------

fn compound_line(x: f64, y: f64, w: f64, h: f64, width_px: f64, compound: Compound) -> Item {
    let mut s = ShapeItem::new(Xfrm::rect(e(x), e(y), e(w), e(h)), Geometry::Line);
    let mut l = Line::solid(e(width_px), Rgba::BLACK);
    l.compound = compound;
    s.line = Some(l);
    Item::Shape(s)
}

#[test]
fn compound_lines_past_the_work_allowance_are_drawn_as_plain_lines() {
    // full-slide diagonals 100 px wide: each mask is a whole canvas of work
    let items: Vec<Item> = (0..400)
        .map(|_| compound_line(0.0, 0.0, 960.0, 540.0, 100.0, Compound::Tri))
        .collect();
    let r = render_svg(&scene(items), &no_media);
    assert!(r.truncated);
    let masks = r.svg.matches("<mask ").count();
    assert!((20..=200).contains(&masks), "{masks} masks");
    // the background, three strokes inside each mask, and one stroke per line
    assert_eq!(
        r.svg.matches("<path ").count(),
        1 + 3 * masks + 400,
        "every line is still drawn"
    );
    assert!(
        model_work() <= super::svg::MAX_FILTER_WORK * 1.3,
        "{:.3e}",
        model_work()
    );
}

#[test]
fn many_small_compound_lines_are_not_limited() {
    let items: Vec<Item> = (0..2000)
        .map(|i| {
            compound_line(
                (i % 80) as f64 * 11.0,
                (i / 80) as f64 * 20.0,
                9.0,
                4.0,
                3.0,
                if i % 2 == 0 {
                    Compound::Dbl
                } else {
                    Compound::Tri
                },
            )
        })
        .collect();
    let r = render_svg(&scene(items), &no_media);
    assert!(!r.truncated);
    assert_eq!(r.svg.matches("<mask ").count(), 2000);
}

#[test]
fn a_big_svg_picture_is_not_embedded() {
    let body = |n: usize| {
        let mut svg =
            String::from(r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">"#);
        svg.push_str(&"<rect width=\"1\" height=\"1\"/>".repeat(n));
        svg.push_str("</svg>");
        svg
    };
    let ok = body(100_000); // 2.9 MB
    let big = body(160_000); // 4.6 MB
    assert!(ok.len() < super::svg::MAX_SVG_PICTURE_BYTES);
    assert!(big.len() > super::svg::MAX_SVG_PICTURE_BYTES);
    let media = by_key(vec![("ok", ok.into_bytes()), ("big", big.into_bytes())]);
    let r = render_svg(&scene(vec![pic("ok", 0.0, 0.0, 100.0, 100.0)]), &*media);
    assert!(!r.truncated && r.svg.contains("<image "));
    let r = render_svg(&scene(vec![pic("big", 0.0, 0.0, 100.0, 100.0)]), &*media);
    assert!(r.truncated && r.svg.contains("#e6e6e6") && !r.svg.contains("<image "));
}

// ---- the raster size against the drawing process's work budget ----------------------------------

/// A 960 x 540 slide with `n` full-slide shadows (about 1.1e8 units of filter work each).
pub(crate) fn shadowed_slide(n: usize) -> SlideScene {
    scene(
        (0..n)
            .map(|_| fx_shape(900.0, 500.0, shadow(30.0)))
            .collect(),
    )
}

#[test]
fn a_slide_without_effects_has_no_raster_limit() {
    let r = render_svg(
        &scene(vec![rect_item(10.0, 10.0, 100.0, 50.0, RED)]),
        &no_media,
    );
    assert_eq!(r.filter_work, 0.0);
    assert_eq!(r.max_raster_px(), u32::MAX);
}

#[test]
fn the_model_size_of_a_slide_is_its_own_when_it_is_bigger_than_the_model_raster() {
    let small = render_svg(&scene(Vec::new()), &no_media);
    assert_eq!(small.model_px, super::svg::MODEL_RASTER_PX);
    let mut big = scene(Vec::new());
    big.width = e(2000.0);
    big.height = e(1000.0);
    let r = render_svg(&big, &no_media);
    assert_eq!(r.model_px, 2000.0, "the process never draws below 1:1");
}

#[test]
fn the_raster_limit_shrinks_as_the_work_grows_and_never_goes_below_the_model_size() {
    let mut last = u32::MAX;
    let mut seen_above_model = false;
    for n in [1usize, 2, 3, 4, 5, 10, 300] {
        let r = render_svg(&shadowed_slide(n), &no_media);
        assert!(r.filter_work > 0.0, "{n}");
        let cap = r.max_raster_px();
        assert!(cap <= last, "{n}: {cap} > {last}: not monotonic");
        assert!(
            f64::from(cap) >= r.model_px,
            "{n}: {cap} is below the model size"
        );
        // The work at the limit is inside the writer's allowance (scale squared).
        let at_cap = r.filter_work * (f64::from(cap) / r.model_px).powi(2);
        assert!(
            at_cap <= super::svg::MAX_FILTER_WORK * 1.001 || f64::from(cap) == r.model_px,
            "{n}: {at_cap:.3e} at {cap}"
        );
        seen_above_model |= f64::from(cap) > r.model_px + 1.0;
        last = cap;
    }
    assert!(
        seen_above_model,
        "a light slide is allowed more than the model size"
    );
}

#[test]
fn the_cap_formula_on_given_work() {
    let r = |filter_work: f64, model_px: f64| Rendered {
        svg: String::new(),
        truncated: false,
        cancelled: false,
        filter_work,
        model_px,
    };
    let w = super::svg::MAX_FILTER_WORK;
    assert_eq!(r(w, 1280.0).max_raster_px(), 1280);
    assert_eq!(r(w / 4.0, 1280.0).max_raster_px(), 2560);
    assert_eq!(
        r(w * 2.0, 1280.0).max_raster_px(),
        1280,
        "never below the model size"
    );
    assert_eq!(r(0.0, 1280.0).max_raster_px(), u32::MAX);
    assert_eq!(r(f64::NAN, 1280.0).max_raster_px(), u32::MAX);
    assert_eq!(r(f64::INFINITY, 1280.0).max_raster_px(), u32::MAX);
    assert_eq!(r(-1.0, 1280.0).max_raster_px(), u32::MAX);
    // work so small the result passes u32: saturates, does not wrap
    assert_eq!(r(1e-300, 1280.0).max_raster_px(), u32::MAX);
}

#[test]
fn a_slide_the_process_refuses_at_a_big_raster_is_accepted_at_its_limit() {
    // Two full-slide shadows pass the process's work limit at 2960 px (the long side of a
    // 300-column terminal's frame) and not at the limit this slide reports.
    let r = render_svg(&shadowed_slide(2), &no_media);
    assert!(!r.truncated);
    assert!(guard_accepts(&r), "inside the budget at the model size");
    assert!(
        !guard_accepts_at(&r, 2960.0),
        "refused at 2960 px: the problem"
    );
    let cap = r.max_raster_px();
    assert!(f64::from(cap) < 2960.0 && f64::from(cap) > r.model_px);
    assert!(guard_accepts_at(&r, f64::from(cap)), "accepted at {cap}");
}
