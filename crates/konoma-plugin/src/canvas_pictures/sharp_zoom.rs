//! Sharp zoom for vector-backed pictures (SVG, mermaid): when the zoom outgrows the density of
//! the raster on screen, the retained SVG is drawn again at the size the zoom needs. Whether to
//! draw again, and at what size, is decided here, once, for both hosts; running the drawing
//! (thread, cancellation, generation) is each host's own.

use std::path::PathBuf;

use image::{DynamicImage, GenericImageView};
use konoma_svg::svg::RasterError;
use konoma_widgets::inline_images::SvgTrust;

use super::SvgDraw;

/// The longest side, in pixels, a sharp redraw may reach (the rasterizer's hard cap; about 64 MiB
/// of RGBA).
pub const SHARP_MAX_SIDE: u32 = 4096;

/// The longest side, in pixels, the picture must be drawn at for `zoom`, or `None` when the
/// raster on screen is dense enough already.
///
/// `raster` is the size of the raster now shown and `logical` the size the layout is made in (the
/// first raster). The need is `ceil(max(logical) * zoom)`; nothing is asked while the raster
/// holds that within a 12.5% margin (`want <= cur + cur / 8`) or has reached
/// [`SHARP_MAX_SIDE`].
pub fn sharp_zoom_target(raster: (u32, u32), logical: (u32, u32), zoom: f64) -> Option<u32> {
    let cur_side = raster.0.max(raster.1);
    let want = ((logical.0.max(logical.1) as f64) * zoom).ceil() as u32;
    if want <= cur_side + cur_side / 8 || cur_side >= SHARP_MAX_SIDE {
        return None;
    }
    Some(want)
}

/// [`sharp_zoom_target`] for the raster `img`.
pub fn sharp_zoom_target_for(img: &DynamicImage, logical: (u32, u32), zoom: f64) -> Option<u32> {
    sharp_zoom_target(img.dimensions(), logical, zoom)
}

/// Draw `data` again at `max_px` the way `trust` says it was drawn the first time (an SVG the
/// host made itself in this process; any other through `svg`, which the host owns), so the same
/// SVG never changes how it looks. Blocks: call from a worker.
pub fn redraw_svg(
    data: &[u8],
    trust: &SvgTrust,
    max_px: u32,
    svg: &dyn SvgDraw,
    cancelled: &dyn Fn() -> bool,
) -> Result<DynamicImage, RasterError> {
    match trust {
        SvgTrust::Trusted(base) => {
            if cancelled() {
                return Err(RasterError::Cancelled);
            }
            Ok(konoma_svg::svg::rasterize_guarded(
                data,
                base.parent(),
                max_px,
            )?)
        }
        SvgTrust::File(base) => svg.untrusted(data, base, None, max_px, cancelled),
        SvgTrust::FileWithin(base, roots) => {
            let roots: Vec<PathBuf> = roots.to_vec();
            svg.untrusted(data, base, Some(&roots), max_px, cancelled)
        }
        SvgTrust::Embedded => svg.embedded(data, max_px, cancelled),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_is_asked_at_fit_or_inside_the_margin() {
        assert_eq!(sharp_zoom_target((800, 400), (800, 400), 1.0), None);
        // exactly cur + cur/8 = 900: still enough
        assert_eq!(
            sharp_zoom_target((800, 400), (800, 400), 900.0 / 800.0),
            None
        );
        assert_eq!(sharp_zoom_target((800, 400), (800, 400), 1.125), None);
    }

    #[test]
    fn one_pixel_over_the_margin_asks_for_that_size() {
        // want = ceil(800 * 1.1251) = 901 > 900
        assert_eq!(
            sharp_zoom_target((800, 400), (800, 400), 901.0 / 800.0),
            Some(901)
        );
        assert_eq!(sharp_zoom_target((800, 400), (800, 400), 2.0), Some(1600));
        // Portrait: the longer side counts, on both sizes.
        assert_eq!(sharp_zoom_target((400, 800), (400, 800), 2.0), Some(1600));
        assert_eq!(sharp_zoom_target((800, 400), (400, 800), 2.0), Some(1600));
    }

    #[test]
    fn the_cap_stops_asking() {
        assert_eq!(sharp_zoom_target((4096, 100), (800, 100), 16.0), None);
        assert_eq!(sharp_zoom_target((100, 5000), (800, 100), 16.0), None);
        // One short of the cap still asks.
        assert_eq!(
            sharp_zoom_target((4095, 100), (800, 100), 16.0),
            Some(12800)
        );
    }

    #[test]
    fn a_raster_already_denser_than_the_zoom_needs_asks_nothing() {
        assert_eq!(sharp_zoom_target((3200, 1600), (800, 400), 2.0), None);
        assert_eq!(sharp_zoom_target((1600, 800), (800, 400), 0.5), None);
    }

    #[test]
    fn the_image_form_reads_the_raster_size() {
        let img = DynamicImage::new_rgba8(800, 400);
        assert_eq!(sharp_zoom_target_for(&img, (800, 400), 2.0), Some(1600));
        assert_eq!(sharp_zoom_target_for(&img, (800, 400), 1.0), None);
    }
}
