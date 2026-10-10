//! Colour effects of pictures (`a:blip` children: duotone, grayscale, bi-level, colour change,
//! brightness / contrast, tint, colour replacement, HSL).
//!
//! The effects are applied to the *decoded pixels* before the picture is embedded in the SVG
//! (an SVG filter could do the linear ones exactly, but not the thresholds and colour keys, and a
//! filter on a tiled pattern costs per tile). To keep the cost bounded a picture with effects is
//! first reduced to at most [`MAX_FX_SIDE`] pixels on its longer side (a slide is drawn at about
//! 1280 px wide), and the decode runs under the same limits as the BMP / TIFF conversion.
//!
//! Formulas follow what LibreOffice does for the same elements (the reference we compare with),
//! which agrees with PowerPoint for `duotone`, `grayscl`, `biLevel`, `clrChange` and `clrRepl`:
//! `duotone` maps the luminance `L` (`0.299 R + 0.587 G + 0.114 B`) linearly from the first colour
//! (`L = 0`) to the second (`L = 1`). `lum`, `hsl` and `tint` are approximations of PowerPoint's
//! own filters (documented at each item). Vector pictures (SVG, EMF, WMF) are not recoloured.

use std::io::Cursor;

use image::imageops;
#[cfg(test)]
use image::RgbaImage;

use super::color::{hsl_to_rgb, rgb_to_hsl};
use super::model::PicFx;

/// Longest side, in pixels, of a picture that has colour effects (larger ones are reduced).
pub const MAX_FX_SIDE: u32 = 2048;
/// Decoding limit per side for a picture with effects.
const MAX_DECODE_SIDE: u32 = 8192;
/// Decoding memory limit in bytes.
const MAX_DECODE_ALLOC: u64 = super::svg::MAX_DECODE_PEAK_BYTES;

/// Decodes `bytes` (a raster picture), applies `fx` in order and re-encodes the result as PNG.
/// `None` when the picture cannot be decoded under the limits.
#[cfg(test)]
pub fn recolor_png(bytes: &[u8], fx: &[PicFx]) -> Option<Vec<u8>> {
    recolor_png_cancellable(bytes, fx, &|| false)
}

/// Pixels between two looks at the cancellation callback while the effects are applied (about
/// 1 ms of work).
const CANCEL_EVERY_PX: usize = 1 << 20;

/// [`recolor_png`] that stops (`None`) between steps and every [`CANCEL_EVERY_PX`] pixels once
/// `cancel` says so.
pub fn recolor_png_cancellable(
    bytes: &[u8],
    fx: &[PicFx],
    cancel: &dyn Fn() -> bool,
) -> Option<Vec<u8>> {
    let mut reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .ok()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_DECODE_SIDE);
    limits.max_image_height = Some(MAX_DECODE_SIDE);
    limits.max_alloc = Some(MAX_DECODE_ALLOC);
    reader.limits(limits);
    if cancel() {
        return None;
    }
    let mut img = reader.decode().ok()?.into_rgba8();
    if cancel() {
        return None;
    }
    let longest = img.width().max(img.height());
    if longest > MAX_FX_SIDE {
        let k = f64::from(MAX_FX_SIDE) / f64::from(longest);
        let w = ((f64::from(img.width()) * k).round() as u32).max(1);
        let h = ((f64::from(img.height()) * k).round() as u32).max(1);
        img = imageops::resize(&img, w, h, imageops::FilterType::Triangle);
    }
    for e in fx {
        for chunk in img.chunks_mut(CANCEL_EVERY_PX * 4) {
            if cancel() {
                return None;
            }
            let (pixels, _) = chunk.as_chunks_mut::<4>();
            for p in pixels {
                apply_px(p, e);
            }
        }
    }
    let mut out = Vec::new();
    let enc = image::codecs::png::PngEncoder::new_with_quality(
        &mut out,
        image::codecs::png::CompressionType::Fast,
        image::codecs::png::FilterType::Sub,
    );
    image::ImageEncoder::write_image(
        enc,
        img.as_raw(),
        img.width(),
        img.height(),
        image::ExtendedColorType::Rgba8,
    )
    .ok()?;
    Some(out)
}

/// Applies the effects in order to every pixel.
#[cfg(test)]
pub fn apply(img: &mut RgbaImage, fx: &[PicFx]) {
    for e in fx {
        for p in img.pixels_mut() {
            apply_px(&mut p.0, e);
        }
    }
}

fn luma(p: &[u8; 4]) -> f64 {
    (0.299 * f64::from(p[0]) + 0.587 * f64::from(p[1]) + 0.114 * f64::from(p[2])) / 255.0
}

fn byte(v: f64) -> u8 {
    if v.is_finite() {
        (v.clamp(0.0, 1.0) * 255.0).round() as u8
    } else {
        0
    }
}

fn set_alpha_mul(p: &mut [u8; 4], a: f64) {
    p[3] = (f64::from(p[3]) * a.clamp(0.0, 1.0)).round() as u8;
}

fn apply_px(p: &mut [u8; 4], e: &PicFx) {
    match e {
        PicFx::Grayscale => {
            let v = byte(luma(p));
            p[0] = v;
            p[1] = v;
            p[2] = v;
        }
        PicFx::BiLevel { thresh } => {
            let v = if luma(p) >= *thresh { 255 } else { 0 };
            p[0] = v;
            p[1] = v;
            p[2] = v;
        }
        PicFx::Duotone { dark, light } => {
            let l = luma(p);
            let mix =
                |a: u8, b: u8| byte((f64::from(a) + (f64::from(b) - f64::from(a)) * l) / 255.0);
            p[0] = mix(dark.r, light.r);
            p[1] = mix(dark.g, light.g);
            p[2] = mix(dark.b, light.b);
            set_alpha_mul(p, dark.a + (light.a - dark.a) * l);
        }
        PicFx::ClrChange {
            from,
            to,
            use_alpha,
        } => {
            // Exact match on the colour (a lossy JPEG gets no match, like in PowerPoint).
            if p[0] == from.r && p[1] == from.g && p[2] == from.b {
                p[0] = to.r;
                p[1] = to.g;
                p[2] = to.b;
                if *use_alpha {
                    p[3] = (f64::from(p[3]) * to.a.clamp(0.0, 1.0)).round() as u8;
                }
            }
        }
        PicFx::ClrRepl(c) => {
            p[0] = c.r;
            p[1] = c.g;
            p[2] = c.b;
            set_alpha_mul(p, c.a);
        }
        PicFx::Lum { bright, contrast } => {
            // LibreOffice's `BitmapEx::Adjust`: contrast scales around the middle grey, brightness
            // adds.
            let c = contrast.clamp(-1.0, 1.0) * 100.0;
            let m = if c >= 0.0 {
                128.0 / (128.0 - 1.27 * c).max(1.0)
            } else {
                (128.0 + 1.27 * c) / 128.0
            };
            let off = (128.0 - m * 128.0) / 255.0 + bright.clamp(-1.0, 1.0);
            for ch in p.iter_mut().take(3) {
                *ch = byte(f64::from(*ch) / 255.0 * m + off);
            }
        }
        PicFx::Hsl { hue, sat, lum } => {
            let (h, s, l) = rgb_to_hsl(p[0], p[1], p[2]);
            let (r, g, b) = hsl_to_rgb(
                (h + hue).rem_euclid(360.0),
                (s + sat).clamp(0.0, 1.0),
                (l + lum).clamp(0.0, 1.0),
            );
            p[0] = r;
            p[1] = g;
            p[2] = b;
        }
        PicFx::Tint { hue, amt } => {
            // Approximation: PowerPoint's tint shifts the picture toward (or away from) a hue. The
            // pixel keeps its lightness; its hue moves toward `hue`, and its saturation toward the
            // saturation a tinted grey would have, by |amt|.
            let (h, s, l) = rgb_to_hsl(p[0], p[1], p[2]);
            let k = amt.abs().clamp(0.0, 1.0);
            let target = if *amt >= 0.0 {
                *hue
            } else {
                (hue + 180.0).rem_euclid(360.0)
            };
            // Shortest way round the hue circle.
            let mut d = (target - h).rem_euclid(360.0);
            if d > 180.0 {
                d -= 360.0;
            }
            let (r, g, b) = hsl_to_rgb(
                (h + d * k).rem_euclid(360.0),
                (s + (1.0 - s) * k * 0.5).clamp(0.0, 1.0),
                l,
            );
            p[0] = r;
            p[1] = g;
            p[2] = b;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::model::Rgba;
    use super::*;

    fn px(fx: PicFx, p: [u8; 4]) -> [u8; 4] {
        let mut p = p;
        apply_px(&mut p, &fx);
        p
    }

    #[test]
    fn duotone_maps_black_and_white_to_its_two_colours_and_grey_between() {
        let fx = || PicFx::Duotone {
            dark: Rgba::rgb(100, 0, 0),
            light: Rgba::rgb(200, 100, 50),
        };
        assert_eq!(px(fx(), [0, 0, 0, 255]), [100, 0, 0, 255]);
        assert_eq!(px(fx(), [255, 255, 255, 255]), [200, 100, 50, 255]);
        let mid = px(fx(), [128, 128, 128, 255]);
        assert!(
            (148..=152).contains(&mid[0]) && (49..=51).contains(&mid[1]),
            "{mid:?}"
        );
    }

    #[test]
    fn duotone_keeps_the_pictures_alpha() {
        let fx = PicFx::Duotone {
            dark: Rgba::BLACK,
            light: Rgba::WHITE,
        };
        assert_eq!(px(fx, [255, 255, 255, 100])[3], 100);
    }

    #[test]
    fn grayscale_uses_the_luminance_weights() {
        let g = px(PicFx::Grayscale, [255, 0, 0, 255]);
        assert_eq!(g, [76, 76, 76, 255]);
        assert_eq!(px(PicFx::Grayscale, [10, 20, 30, 7])[3], 7);
    }

    #[test]
    fn bilevel_splits_at_the_threshold() {
        let f = || PicFx::BiLevel { thresh: 0.5 };
        assert_eq!(px(f(), [200, 200, 200, 255]), [255, 255, 255, 255]);
        assert_eq!(px(f(), [100, 100, 100, 255]), [0, 0, 0, 255]);
        // at the threshold itself: white
        assert_eq!(px(f(), [128, 128, 128, 255])[0], 255);
    }

    #[test]
    fn clr_change_replaces_only_the_exact_colour_and_alpha_only_with_use_alpha() {
        let f = |use_alpha| PicFx::ClrChange {
            from: Rgba::rgb(255, 255, 255),
            to: Rgba::new(10, 20, 30, 0.0),
            use_alpha,
        };
        assert_eq!(px(f(true), [255, 255, 255, 255]), [10, 20, 30, 0]);
        assert_eq!(px(f(false), [255, 255, 255, 255]), [10, 20, 30, 255]);
        assert_eq!(px(f(true), [254, 255, 255, 255]), [254, 255, 255, 255]);
    }

    #[test]
    fn clr_repl_paints_everything_keeping_alpha_times_colour_alpha() {
        let f = PicFx::ClrRepl(Rgba::new(1, 2, 3, 0.5));
        assert_eq!(px(f, [9, 9, 9, 200]), [1, 2, 3, 100]);
    }

    #[test]
    fn lum_brightness_adds_and_contrast_pivots_on_mid_grey() {
        let b = px(
            PicFx::Lum {
                bright: 0.2,
                contrast: 0.0,
            },
            [100, 100, 100, 255],
        );
        assert_eq!(b[0], 151);
        let c = px(
            PicFx::Lum {
                bright: 0.0,
                contrast: 0.5,
            },
            [128, 128, 128, 255],
        );
        assert!((127..=129).contains(&c[0]), "{c:?}");
        let hi = px(
            PicFx::Lum {
                bright: 0.0,
                contrast: 0.5,
            },
            [200, 200, 200, 255],
        );
        let lo = px(
            PicFx::Lum {
                bright: 0.0,
                contrast: 0.5,
            },
            [60, 60, 60, 255],
        );
        assert!(hi[0] > 200 && lo[0] < 60, "{hi:?} {lo:?}");
        let flat = px(
            PicFx::Lum {
                bright: 0.0,
                contrast: -0.5,
            },
            [200, 200, 200, 255],
        );
        assert!(flat[0] < 200 && flat[0] > 128);
        // never out of range, never NaN
        let wild = px(
            PicFx::Lum {
                bright: f64::NAN,
                contrast: 9.0,
            },
            [255, 0, 7, 255],
        );
        assert_eq!(wild[3], 255);
    }

    #[test]
    fn hsl_shifts_hue_and_clamps_saturation_and_lightness() {
        let red = px(
            PicFx::Hsl {
                hue: 120.0,
                sat: 0.0,
                lum: 0.0,
            },
            [255, 0, 0, 255],
        );
        assert_eq!(red, [0, 255, 0, 255]);
        let grey = px(
            PicFx::Hsl {
                hue: 0.0,
                sat: -1.0,
                lum: 0.0,
            },
            [255, 0, 0, 255],
        );
        assert_eq!(grey[0], grey[1]);
        let white = px(
            PicFx::Hsl {
                hue: 0.0,
                sat: 0.0,
                lum: 1.0,
            },
            [255, 0, 0, 255],
        );
        assert_eq!(white, [255, 255, 255, 255]);
    }

    #[test]
    fn tint_moves_the_hue_toward_the_target_the_short_way() {
        let t = px(
            PicFx::Tint {
                hue: 120.0,
                amt: 1.0,
            },
            [255, 0, 0, 255],
        );
        assert!(t[1] > t[0], "{t:?}");
        // negative amount goes to the opposite hue (red -> cyan side)
        let n = px(
            PicFx::Tint {
                hue: 0.0,
                amt: -1.0,
            },
            [255, 0, 0, 255],
        );
        assert!(n[0] < 255 && n[1] > 0 || n[2] > 0, "{n:?}");
        // zero changes nothing
        assert_eq!(
            px(
                PicFx::Tint {
                    hue: 33.0,
                    amt: 0.0
                },
                [10, 200, 90, 255]
            ),
            [10, 200, 90, 255]
        );
    }

    #[test]
    fn effects_apply_in_order() {
        let mut img = RgbaImage::from_pixel(1, 1, image::Rgba([255, 0, 0, 255]));
        apply(
            &mut img,
            &[
                PicFx::Grayscale,
                PicFx::Duotone {
                    dark: Rgba::rgb(0, 0, 100),
                    light: Rgba::rgb(0, 0, 200),
                },
            ],
        );
        // 0.299 -> blue 100 + 100 * 0.299
        let p = img.get_pixel(0, 0).0;
        assert_eq!(p[0], 0);
        assert!((129..=131).contains(&p[2]), "{p:?}");
    }

    #[test]
    fn recolor_png_decodes_applies_and_reencodes_and_rejects_garbage() {
        let img = RgbaImage::from_pixel(2, 2, image::Rgba([255, 255, 255, 255]));
        let mut png = Vec::new();
        img.write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let out = recolor_png(
            &png,
            &[PicFx::Duotone {
                dark: Rgba::BLACK,
                light: Rgba::rgb(10, 20, 30),
            }],
        )
        .unwrap();
        let back = image::load_from_memory(&out).unwrap().to_rgba8();
        assert_eq!(back.get_pixel(1, 1).0, [10, 20, 30, 255]);
        assert!(recolor_png(b"not a picture", &[PicFx::Grayscale]).is_none());
    }

    #[test]
    fn recolor_png_reduces_a_large_picture_to_the_side_limit() {
        let img = RgbaImage::from_pixel(MAX_FX_SIDE + 500, 40, image::Rgba([1, 2, 3, 255]));
        let mut png = Vec::new();
        img.write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let out = recolor_png(&png, &[PicFx::Grayscale]).unwrap();
        let back = image::load_from_memory(&out).unwrap();
        assert_eq!(back.width(), MAX_FX_SIDE);
        assert!(back.height() >= 1 && back.height() <= 40);
    }

    // ---- through the SVG writer ---------------------------------------------------------------

    use super::super::{render_svg, Fill, ImageFill, ImageMode, Item, ShapeItem, SlideScene, Xfrm};
    use super::super::{Geometry, RectAlign, TileFlip, EMU_PER_PX};
    use std::sync::Arc;

    fn png_of(img: &RgbaImage) -> Vec<u8> {
        let mut out = Vec::new();
        img.write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png)
            .unwrap();
        out
    }

    /// A 100 x 100 px slide with one 100 x 100 px rectangle filled with `fill`, drawn with `media`.
    fn paint(fill: ImageFill, media: Vec<u8>) -> (String, RgbaImage) {
        let mut sh = ShapeItem::new(
            Xfrm::rect(0.0, 0.0, 100.0 * EMU_PER_PX, 100.0 * EMU_PER_PX),
            Geometry::Rect,
        );
        sh.fill = Fill::Image(fill);
        let scene = SlideScene {
            width: 100.0 * EMU_PER_PX,
            height: 100.0 * EMU_PER_PX,
            background: Fill::Solid(Rgba::BLACK),
            underlay: Vec::new(),
            items: vec![Item::Shape(sh)],
            truncated: false,
        };
        let bytes = Arc::new(media);
        let r = render_svg(&scene, &move |_| Some(bytes.clone()));
        let img = crate::preview::svg::rasterize_trusted(
            r.svg.as_bytes(),
            std::path::Path::new("/s.svg"),
            100,
        )
        .expect("rasterizes")
        .to_rgba8();
        (r.svg, img)
    }

    fn duo() -> Vec<PicFx> {
        vec![PicFx::Duotone {
            dark: Rgba::rgb(0, 0, 255),
            light: Rgba::rgb(255, 0, 0),
        }]
    }

    #[test]
    fn a_stretched_picture_with_effects_is_recoloured_when_drawn() {
        let white = RgbaImage::from_pixel(8, 8, image::Rgba([255, 255, 255, 255]));
        let mut f = ImageFill::stretch("k");
        f.fx = duo();
        let (svg, img) = paint(f, png_of(&white));
        assert!(svg.contains("data:image/png"));
        let p = img.get_pixel(50, 50).0;
        assert!(p[0] > 240 && p[1] < 15 && p[2] < 15, "{p:?}");
        // without the effect it stays white
        let (_, plain) = paint(ImageFill::stretch("k"), png_of(&white));
        assert_eq!(plain.get_pixel(50, 50).0[..3], [255, 255, 255]);
    }

    #[test]
    fn a_tiled_picture_with_effects_is_recoloured_in_every_tile() {
        // a black and white checker, 2 x 2 px tiles of 10 px: duotone black -> blue, white -> red
        let mut chk = RgbaImage::from_pixel(2, 2, image::Rgba([0, 0, 0, 255]));
        chk.put_pixel(1, 0, image::Rgba([255, 255, 255, 255]));
        chk.put_pixel(0, 1, image::Rgba([255, 255, 255, 255]));
        let mut f = ImageFill::stretch("k");
        f.fx = duo();
        f.mode = ImageMode::Tile {
            sx: 5.0,
            sy: 5.0,
            tx: 0.0,
            ty: 0.0,
            align: RectAlign::TopLeft,
            flip: TileFlip::None,
        };
        let (_, img) = paint(f, png_of(&chk));
        let blue = img.get_pixel(2, 2).0;
        let red = img.get_pixel(7, 2).0;
        assert!(blue[2] > 200 && blue[0] < 60, "{blue:?}");
        assert!(red[0] > 200 && red[2] < 60, "{red:?}");
        // the next tile repeats
        let red2 = img.get_pixel(17, 2).0;
        assert!(red2[0] > 200 && red2[2] < 60, "{red2:?}");
    }

    #[test]
    fn effects_keep_the_alpha_of_the_fill_and_the_picture() {
        let mut half = RgbaImage::from_pixel(4, 4, image::Rgba([255, 255, 255, 255]));
        half.put_pixel(0, 0, image::Rgba([255, 255, 255, 0]));
        let mut f = ImageFill::stretch("k");
        f.fx = duo();
        f.alpha = 0.5;
        let (svg, img) = paint(f, png_of(&half));
        assert!(svg.contains("opacity=\"0.5\""), "{svg}");
        // red at half opacity over the black slide
        let p = img.get_pixel(60, 60).0;
        assert!((100..=150).contains(&p[0]) && p[2] < 30, "{p:?}");
    }

    #[test]
    fn a_picture_that_cannot_be_decoded_is_embedded_unchanged() {
        let junk = b"\x89PNG\r\n\x1a\nnot a real picture".to_vec();
        let mut f = ImageFill::stretch("k");
        f.fx = duo();
        let mut sh = ShapeItem::new(
            Xfrm::rect(0.0, 0.0, 50.0 * EMU_PER_PX, 50.0 * EMU_PER_PX),
            Geometry::Rect,
        );
        sh.fill = Fill::Image(f);
        let scene = SlideScene {
            width: 50.0 * EMU_PER_PX,
            height: 50.0 * EMU_PER_PX,
            background: Fill::Solid(Rgba::WHITE),
            underlay: Vec::new(),
            items: vec![Item::Shape(sh)],
            truncated: false,
        };
        let bytes = Arc::new(junk.clone());
        let r = render_svg(&scene, &move |_| Some(bytes.clone()));
        // the original bytes are in the data URI, not the grey placeholder
        use base64::Engine as _;
        let b64 = base64::engine::general_purpose::STANDARD.encode(&junk);
        assert!(r.svg.contains(&b64));
        assert!(!r.svg.contains("#bfbfbf"));
    }

    #[test]
    fn vector_pictures_keep_their_colours() {
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><rect width="10" height="10" fill="#00ff00"/></svg>"##.to_vec();
        let mut f = ImageFill::stretch("k");
        f.fx = duo();
        let (out, img) = paint(f, svg);
        assert!(out.contains("data:image/svg+xml"));
        let p = img.get_pixel(50, 50).0;
        assert!(p[1] > 200 && p[0] < 60, "{p:?}");
    }

    #[test]
    fn a_big_picture_with_effects_is_reduced_and_still_drawn_with_the_same_aspect() {
        let big = RgbaImage::from_pixel(MAX_FX_SIDE * 2, 100, image::Rgba([255, 255, 255, 255]));
        let mut f = ImageFill::stretch("k");
        f.fx = duo();
        let (svg, img) = paint(f, png_of(&big));
        assert!(svg.contains("data:image/png"));
        let p = img.get_pixel(50, 50).0;
        assert!(p[0] > 240 && p[2] < 15, "{p:?}");
    }
}
