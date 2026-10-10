//! Shape effects (glow, soft edge, reflection, inner and outer shadow) and rectangular gradients,
//! through the project's rasterizer.

use super::model::*;
use super::render_svg;
use super::tests::{at, e, no_media, raster, scene, RED};

fn shape(x: f64, y: f64, w: f64, h: f64, fx: Effects) -> Item {
    let mut s = ShapeItem::new(Xfrm::rect(e(x), e(y), e(w), e(h)), Geometry::Rect);
    s.fill = Fill::Solid(RED);
    s.effects = fx;
    Item::Shape(s)
}

fn draw(item: Item) -> image::RgbaImage {
    raster(&render_svg(&scene(vec![item]), &no_media))
}

fn white(p: [u8; 3]) -> bool {
    p.iter().all(|c| *c >= 250)
}

fn black_shadow(blur: f64, dist: f64, dir: f64) -> Shadow {
    Shadow {
        blur_rad: e(blur),
        dist: e(dist),
        dir_deg: dir,
        color: Rgba::new(0, 0, 0, 1.0),
        sx: 1.0,
        sy: 1.0,
        rot_with_shape: true,
    }
}

#[test]
fn no_effects_write_no_extra_layers() {
    let r = render_svg(
        &scene(vec![shape(100.0, 100.0, 100.0, 100.0, Effects::default())]),
        &no_media,
    );
    assert!(
        !r.svg.contains("<use") && !r.svg.contains("<filter"),
        "{}",
        r.svg
    );
    // Transparent effects draw nothing either.
    let fx = Effects {
        glow: Some(Glow {
            rad: e(10.0),
            color: Rgba::new(255, 255, 0, 0.0),
        }),
        outer_shadow: Some(Shadow {
            color: Rgba::new(0, 0, 0, 0.0),
            ..black_shadow(0.0, 5.0, 0.0)
        }),
        soft_edge: Some(0.0),
        ..Effects::default()
    };
    let r = render_svg(
        &scene(vec![shape(100.0, 100.0, 100.0, 100.0, fx)]),
        &no_media,
    );
    assert!(!r.svg.contains("<use"), "{}", r.svg);
}

#[test]
fn the_shape_is_written_once_and_used_per_layer() {
    let fx = Effects {
        outer_shadow: Some(black_shadow(4.0, 10.0, 45.0)),
        glow: Some(Glow {
            rad: e(8.0),
            color: Rgba::new(0, 255, 0, 0.5),
        }),
        ..Effects::default()
    };
    let r = render_svg(
        &scene(vec![shape(100.0, 100.0, 100.0, 100.0, fx)]),
        &no_media,
    );
    // one definition of the painted shape, three uses (shadow, glow, the shape)
    assert_eq!(r.svg.matches(r##"<g id="fx"##).count(), 1, "{}", r.svg);
    assert_eq!(r.svg.matches("<use").count(), 3, "{}", r.svg);
    let img = raster(&r);
    assert!(at(&img, 150, 150)[0] > 240 && at(&img, 150, 150)[1] < 20);
}

#[test]
fn a_glow_surrounds_the_shape_and_fades_out() {
    let fx = Effects {
        glow: Some(Glow {
            rad: e(20.0),
            color: Rgba::new(0, 200, 0, 1.0),
        }),
        ..Effects::default()
    };
    let img = draw(shape(200.0, 200.0, 100.0, 100.0, fx));
    // the shape itself is untouched
    assert!(at(&img, 250, 250)[0] > 240);
    // just outside every side: greenish (not white)
    for (x, y) in [(195, 250), (305, 250), (250, 195), (250, 305)] {
        let p = at(&img, x, y);
        assert!(p[0] < 200 && p[1] > p[0], "({x},{y}) {p:?}");
    }
    // the glow is weaker further out and gone beyond its radius
    let near = at(&img, 190, 250)[0];
    let far = at(&img, 178, 250)[0];
    assert!(far > near, "{far} {near}");
    assert!(white(at(&img, 140, 250)));
}

#[test]
fn a_glow_holds_over_two_fifths_of_the_radius_then_falls_to_nothing_at_one_and_a_half() {
    // radius 30 px, black at full alpha: the profile along a line leaving the right side
    let fx = Effects {
        glow: Some(Glow {
            rad: e(30.0),
            color: Rgba::new(0, 0, 0, 1.0),
        }),
        ..Effects::default()
    };
    let img = draw(shape(200.0, 200.0, 100.0, 100.0, fx));
    let dark = |d: u32| 255 - u32::from(at(&img, 300 + d, 250)[0]);
    // strong at the outline and 0.4 radius out (plateau), about half at the radius, and
    // nothing at 1.5 radii (a little tail from the blur is allowed)
    assert!(dark(2) > 215, "{}", dark(2));
    assert!(dark(8) > 190, "{}", dark(8));
    assert!((60..190).contains(&dark(30)), "{}", dark(30));
    assert!(dark(45) < 30, "{}", dark(45));
    assert!(dark(60) < 6, "{}", dark(60));
}

#[test]
fn a_glow_uses_its_colours_alpha() {
    let glow = |a: f64| Effects {
        glow: Some(Glow {
            rad: e(20.0),
            color: Rgba::new(0, 0, 0, a),
        }),
        ..Effects::default()
    };
    let strong = at(
        &draw(shape(200.0, 200.0, 100.0, 100.0, glow(1.0))),
        192,
        250,
    )[0];
    let weak = at(
        &draw(shape(200.0, 200.0, 100.0, 100.0, glow(0.3))),
        192,
        250,
    )[0];
    assert!(weak > strong + 40, "{weak} {strong}");
}

#[test]
fn a_soft_edge_fades_the_rim_but_not_the_middle() {
    let fx = Effects {
        soft_edge: Some(e(20.0)),
        ..Effects::default()
    };
    let img = draw(shape(200.0, 200.0, 100.0, 100.0, fx));
    // the middle is the fill
    assert!(at(&img, 250, 250)[0] > 240 && at(&img, 250, 250)[1] < 20);
    // the outermost pixels are mostly the background
    let rim = at(&img, 201, 250);
    assert!(rim[1] > 150, "{rim:?}");
    // nothing is drawn outside the shape
    assert!(white(at(&img, 195, 250)));
    // fading is monotonic towards the middle
    let a = at(&img, 205, 250)[1];
    let b = at(&img, 215, 250)[1];
    assert!(a > b, "{a} {b}");
}

#[test]
fn a_reflection_is_a_faded_mirror_below_the_shape() {
    let fx = Effects {
        reflection: Some(Reflection {
            blur_rad: 0.0,
            start_alpha: 0.6,
            end_alpha: 0.0,
            start_pos: 0.0,
            end_pos: 1.0,
            dist: e(10.0),
            dir_deg: 90.0,
            fade_dir_deg: 90.0,
            sx: 1.0,
            sy: -1.0,
        }),
        ..Effects::default()
    };
    let img = draw(shape(200.0, 200.0, 100.0, 100.0, fx));
    // the shape
    assert!(at(&img, 250, 250)[0] > 240 && at(&img, 250, 250)[1] < 20);
    // the gap (10 px) is empty, the mirror starts under it, strongest at its top, gone at its end
    assert!(white(at(&img, 250, 305)));
    let top = at(&img, 250, 312);
    let mid = at(&img, 250, 360);
    let end = at(&img, 250, 405);
    assert!(top[1] < 140 && top[0] > 240, "{top:?}");
    assert!(mid[1] > top[1] + 30, "{mid:?} {top:?}");
    assert!(end[1] > 240, "{end:?}");
    // nothing above the shape, nothing beside the mirror
    assert!(white(at(&img, 250, 195)));
    assert!(white(at(&img, 180, 350)));
}

#[test]
fn a_reflection_fades_over_the_range_it_names() {
    let refl = |end_pos: f64| Effects {
        reflection: Some(Reflection {
            blur_rad: 0.0,
            start_alpha: 1.0,
            end_alpha: 0.0,
            start_pos: 0.0,
            end_pos,
            dist: 0.0,
            dir_deg: 90.0,
            fade_dir_deg: 90.0,
            sx: 1.0,
            sy: -1.0,
        }),
        ..Effects::default()
    };
    // with endPos 0.5 the mirror is gone by half its height
    let img = draw(shape(200.0, 200.0, 100.0, 100.0, refl(0.5)));
    assert!(white(at(&img, 250, 360)), "{:?}", at(&img, 250, 360));
    let img = draw(shape(200.0, 200.0, 100.0, 100.0, refl(1.0)));
    assert!(!white(at(&img, 250, 340)), "{:?}", at(&img, 250, 340));
}

#[test]
fn a_blurred_reflection_stays_in_bounds() {
    let fx = Effects {
        reflection: Some(Reflection {
            blur_rad: e(6.0),
            start_alpha: 0.5,
            end_alpha: 0.0,
            start_pos: 0.0,
            end_pos: 0.5,
            dist: 0.0,
            dir_deg: 90.0,
            fade_dir_deg: 90.0,
            sx: 1.0,
            sy: -1.0,
        }),
        ..Effects::default()
    };
    let r = render_svg(
        &scene(vec![shape(200.0, 200.0, 100.0, 100.0, fx)]),
        &no_media,
    );
    let img = raster(&r);
    assert!(!white(at(&img, 250, 310)));
    assert!(white(at(&img, 250, 395)));
}

#[test]
fn an_inner_shadow_darkens_the_inside_of_the_rim() {
    let fx = Effects {
        inner_shadow: Some(black_shadow(20.0, 0.0, 0.0)),
        ..Effects::default()
    };
    let img = draw(shape(200.0, 200.0, 100.0, 100.0, fx));
    let rim = at(&img, 203, 250);
    let mid = at(&img, 250, 250);
    assert!(rim[0] < mid[0] - 40, "{rim:?} {mid:?}");
    assert!(mid[0] > 240 && mid[1] < 20);
    // nothing outside the shape
    assert!(white(at(&img, 195, 250)));
}

#[test]
fn an_inner_shadow_with_an_offset_falls_on_the_far_side() {
    // dir 0 (to the right), 15 px, no blur: the left rim is dark, the right rim is not.
    let fx = Effects {
        inner_shadow: Some(black_shadow(0.0, 15.0, 0.0)),
        ..Effects::default()
    };
    let img = draw(shape(200.0, 200.0, 100.0, 100.0, fx));
    assert!(at(&img, 207, 250)[0] < 60, "{:?}", at(&img, 207, 250));
    assert!(at(&img, 292, 250)[0] > 240, "{:?}", at(&img, 292, 250));
}

#[test]
fn an_outer_shadow_has_its_blur_distance_and_direction() {
    let sh = |blur: f64| Effects {
        outer_shadow: Some(black_shadow(blur, 30.0, 90.0)),
        ..Effects::default()
    };
    // sharp: a solid copy 30 px lower
    let img = draw(shape(200.0, 200.0, 100.0, 100.0, sh(0.0)));
    assert!(at(&img, 250, 320)[0] < 20, "{:?}", at(&img, 250, 320));
    assert!(white(at(&img, 250, 335)));
    assert!(white(at(&img, 150, 320)));
    // blurred: the edge of the copy is soft
    let img = draw(shape(200.0, 200.0, 100.0, 100.0, sh(16.0)));
    let edge = at(&img, 250, 330)[0];
    assert!(edge > 40 && edge < 230, "{edge}");
    // the sharp one is darker at the same place
    assert!(at(&img, 250, 325)[0] > 20);
}

#[test]
fn an_outer_shadow_is_scaled_about_the_bottom_centre() {
    let fx = Effects {
        outer_shadow: Some(Shadow {
            sx: 2.0,
            sy: 0.5,
            ..black_shadow(0.0, 0.0, 0.0)
        }),
        ..Effects::default()
    };
    let img = draw(shape(200.0, 200.0, 100.0, 100.0, fx));
    // 200 px wide, 50 px tall, sitting on the bottom edge: x 150..350, y 250..300
    assert!(at(&img, 160, 280)[0] < 20, "{:?}", at(&img, 160, 280));
    assert!(at(&img, 340, 280)[0] < 20);
    assert!(white(at(&img, 160, 240)));
    assert!(white(at(&img, 360, 280)));
}

#[test]
fn a_shadow_that_does_not_rotate_with_the_shape_keeps_its_direction() {
    let item = |rot_with_shape: bool| {
        let mut s = ShapeItem::new(
            Xfrm {
                rot_deg: 90.0,
                ..Xfrm::rect(e(300.0), e(300.0), e(100.0), e(60.0))
            },
            Geometry::Rect,
        );
        s.fill = Fill::Solid(RED);
        s.effects.outer_shadow = Some(Shadow {
            rot_with_shape,
            ..black_shadow(0.0, 40.0, 0.0)
        });
        Item::Shape(s)
    };
    // The rotated shape is 60 x 100 px around (350, 330). Direction 0 = to the right on the slide
    // when the shadow does not turn with the shape; with the shape it points down.
    let img = draw(item(false));
    assert!(at(&img, 400, 330)[0] < 20, "{:?}", at(&img, 400, 330));
    assert!(white(at(&img, 350, 400)));
    let img = draw(item(true));
    assert!(at(&img, 350, 400)[0] < 20, "{:?}", at(&img, 350, 400));
    assert!(white(at(&img, 400, 330)));
}

#[test]
fn effects_of_a_picture_are_drawn() {
    let png = {
        let mut img = image::RgbaImage::new(4, 4);
        for p in img.pixels_mut() {
            *p = image::Rgba([0, 0, 255, 255]);
        }
        let mut out = Vec::new();
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
            .unwrap();
        out
    };
    let mut pic = PictureItem::new(Xfrm::rect(e(200.0), e(200.0), e(100.0), e(100.0)), "k");
    pic.effects.glow = Some(Glow {
        rad: e(16.0),
        color: Rgba::new(255, 0, 0, 1.0),
    });
    let a = std::sync::Arc::new(png);
    let r = render_svg(&scene(vec![Item::Picture(pic)]), &move |_| Some(a.clone()));
    let img = raster(&r);
    assert!(at(&img, 250, 250)[2] > 240 && at(&img, 250, 250)[0] < 20);
    let halo = at(&img, 194, 250);
    assert!(halo[0] > 100 && halo[2] < 200, "{halo:?}");
}

#[test]
fn hostile_effect_numbers_do_not_break_the_picture() {
    let fx = Effects {
        outer_shadow: Some(Shadow {
            blur_rad: f64::NAN,
            dist: f64::INFINITY,
            dir_deg: f64::NAN,
            color: Rgba::new(0, 0, 0, 1.0),
            sx: f64::NAN,
            sy: f64::INFINITY,
            rot_with_shape: false,
        }),
        glow: Some(Glow {
            rad: 1e30,
            color: Rgba::new(0, 0, 0, 1.0),
        }),
        soft_edge: Some(f64::INFINITY),
        reflection: Some(Reflection {
            blur_rad: 1e30,
            start_alpha: f64::NAN,
            end_alpha: f64::NAN,
            start_pos: f64::NAN,
            end_pos: f64::NAN,
            dist: f64::NAN,
            dir_deg: f64::INFINITY,
            fade_dir_deg: 0.0,
            sx: f64::NAN,
            sy: 1e30,
        }),
        inner_shadow: Some(Shadow {
            blur_rad: 1e30,
            ..black_shadow(0.0, 1e30, f64::NAN)
        }),
    };
    let r = render_svg(&scene(vec![shape(100.0, 100.0, 50.0, 50.0, fx)]), &no_media);
    assert!(
        !r.svg.contains("NaN") && !r.svg.contains("inf"),
        "{}",
        r.svg
    );
    let _ = raster(&r);
}

// ---- rectangular gradients -----------------------------------------------------------------

fn rect_gradient(fill_to_rect: Rect4) -> image::RgbaImage {
    let g = Gradient {
        kind: GradKind::Rect,
        stops: vec![(0.0, RED), (1.0, Rgba::rgb(0, 0, 255))],
        fill_to_rect,
        rot_with_shape: true,
    };
    let mut s = ShapeItem::new(
        Xfrm::rect(e(100.0), e(100.0), e(400.0), e(200.0)),
        Geometry::Rect,
    );
    s.fill = Fill::Gradient(g);
    raster(&render_svg(&scene(vec![Item::Shape(s)]), &no_media))
}

#[test]
fn a_rect_gradient_has_rectangular_iso_lines() {
    let img = rect_gradient((0.5, 0.5, 0.5, 0.5));
    // centre red, every edge middle blue, corners blue
    assert!(at(&img, 300, 200)[0] > 240);
    for (x, y) in [(101, 200), (498, 200), (300, 101), (300, 298), (102, 102)] {
        assert!(at(&img, x, y)[2] > 225, "({x},{y}) {:?}", at(&img, x, y));
    }
    // Iso-lines are rectangles: half way (t = 0.5) is 100 px from the centre horizontally and
    // 50 px vertically, and both have the same colour (an ellipse would not).
    let h = at(&img, 400, 200);
    let v = at(&img, 300, 150);
    for c in 0..3 {
        assert!((h[c] as i32 - v[c] as i32).abs() < 14, "{h:?} {v:?}");
    }
    assert!((h[0] as i32 - 128).abs() < 20, "{h:?}");
    // On a diagonal from the centre to a corner the colour follows the same rule.
    let d = at(&img, 400, 250);
    assert!((d[0] as i32 - 128).abs() < 24, "{d:?}");
}

#[test]
fn a_rect_gradient_from_a_band_runs_from_the_band() {
    // fillToRect l = 0, r = 0 (the full width), t = b = 0.5: a horizontal line through the middle
    let img = rect_gradient((0.0, 0.5, 0.0, 0.5));
    assert!(at(&img, 120, 200)[0] > 240, "{:?}", at(&img, 120, 200));
    assert!(at(&img, 480, 200)[0] > 240);
    assert!(at(&img, 300, 102)[2] > 225);
    assert!(at(&img, 300, 298)[2] > 225);
    // vertical position decides the colour, not the horizontal one
    let a = at(&img, 150, 150);
    let b = at(&img, 450, 150);
    assert!((a[0] as i32 - b[0] as i32).abs() < 14, "{a:?} {b:?}");
}

#[test]
fn a_rect_gradient_off_centre_is_red_at_its_focus() {
    let img = rect_gradient((0.25, 0.25, 0.75, 0.75));
    assert!(at(&img, 200, 150)[0] > 235, "{:?}", at(&img, 200, 150));
    assert!(at(&img, 498, 298)[2] > 200);
}

#[test]
fn a_rect_gradient_with_a_stroke_and_hostile_focus_rectangles() {
    for r in [
        (f64::NAN, 0.5, 0.5, 0.5),
        (2.0, -1.0, 9.0, 9.0),
        (0.9, 0.9, 0.9, 0.9),
        (0.0, 0.0, 0.0, 0.0),
        (1.0, 1.0, 1.0, 1.0),
    ] {
        let img = rect_gradient(r);
        // it draws something inside the box and nothing outside
        assert!(!white(at(&img, 300, 200)), "{r:?}");
        assert!(white(at(&img, 90, 200)), "{r:?}");
    }
}

#[test]
fn glows_of_every_size_rasterize() {
    for rad in [1.0, 20.0, 60.0, 80.0, 100.0, 150.0, 300.0, 1e6] {
        let fx = Effects {
            glow: Some(Glow {
                rad: e(rad),
                color: Rgba::new(0, 0, 0, 1.0),
            }),
            ..Effects::default()
        };
        let img = draw(shape(300.0, 200.0, 50.0, 50.0, fx));
        assert!(at(&img, 325, 225)[0] > 240, "{rad}");
    }
}
