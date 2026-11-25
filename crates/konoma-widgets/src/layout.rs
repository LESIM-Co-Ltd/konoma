//! Image layout arithmetic: where an image of a given size goes inside a cell area (fit, zoom,
//! pan crop). Pure functions over `ratatui::layout::Rect` and pixel sizes.

use ratatui::layout::Rect;
use ratatui_image::Resize;

/// The rectangle that centers an image of size `cells` (natural size) within `inner` while preserving the aspect ratio.
/// With `allow_upscale=false`, downscale only (scale<=1=fit behavior); with `true`, it may upscale to fill the area.
pub fn centered_rect(cells: (u16, u16), inner: Rect, allow_upscale: bool) -> Rect {
    let (cw, ch) = cells;
    if cw == 0 || ch == 0 || inner.width == 0 || inner.height == 0 {
        return inner;
    }
    let mut scale = (inner.width as f64 / cw as f64).min(inner.height as f64 / ch as f64);
    if !allow_upscale {
        scale = scale.min(1.0);
    }
    let w = ((cw as f64 * scale).round() as u16).clamp(1, inner.width);
    let h = ((ch as f64 * scale).round() as u16).clamp(1, inner.height);
    let x = inner.x + (inner.width - w) / 2;
    let y = inner.y + (inner.height - h) / 2;
    Rect {
        x,
        y,
        width: w,
        height: h,
    }
}

/// Source-pixel crop rectangle (x, y, w, h).
pub type PxRect = (u32, u32, u32, u32);

/// Crop window for the in-place zoom of an inline diagram. `f` is the visible fraction, i.e.
/// 1/zoom, of the source; `center` is clamped so the window stays inside the image. Returns the
/// crop px rect plus the clamped center. Fractions are scale-free, so the same zoom and center
/// map onto a sharper re-raster unchanged.
pub fn fence_crop(dims: (u32, u32), f: f64, center: (f64, f64)) -> (PxRect, (f64, f64)) {
    let (sw, sh) = dims;
    let f = f.clamp(0.0, 1.0);
    let cx = center.0.clamp(f / 2.0, 1.0 - f / 2.0);
    let cy = center.1.clamp(f / 2.0, 1.0 - f / 2.0);
    let cw = ((sw as f64 * f).round() as u32).clamp(1, sw);
    let ch = ((sh as f64 * f).round() as u32).clamp(1, sh);
    let x0 = ((cx * sw as f64) as i64 - cw as i64 / 2).clamp(0, (sw - cw) as i64) as u32;
    let y0 = ((cy * sh as f64) as i64 - ch as i64 / 2).clamp(0, (sh - ch) as i64) as u32;
    ((x0, y0, cw, ch), (cx, cy))
}

/// Result of [`image_layout`]. `center` and `frac` are both `(f64, f64)` pairs with different
/// meanings (image-space center vs. per-axis visible fraction), so this struct keeps them from
/// being swapped at the call site.
///
/// - `target`  : the render-target rectangle (centered; z=1=fit, grows when zooming, clips when exceeding the viewport)
/// - `crop_rect`: the source-image window of the visible portion (px: x,y,w,h)
/// - `center`  : the center clamped to where the visible window fits within the image [0,1]
/// - `frac`    : the visible fraction per axis (<1=clipped=pan possible)
#[derive(Clone, Copy)]
pub struct ImageLayout {
    pub target: Rect,
    pub crop_rect: PxRect,
    pub center: (f64, f64),
    pub frac: (f64, f64),
}

/// Compute the display layout for an image/GIF frame (a pure function shared by prepare_image / prepare_gif).
/// From (source image src, font_size, zoom, center[0,1], display area inner, render_scale), returns
/// an [`ImageLayout`] (None if size 0).
pub fn image_layout(
    src: &image::DynamicImage,
    font_size: ratatui_image::FontSize,
    zoom: f64,
    center: (f64, f64),
    inner: Rect,
    render_scale: f64,
    logical: Option<(u32, u32)>,
) -> Option<ImageLayout> {
    use image::GenericImageView;
    let (sw, sh) = src.dimensions();
    if sw == 0 || sh == 0 || inner.width == 0 || inner.height == 0 {
        return None;
    }
    // The z=1 fit display size (cells, never enlarged). A vector-derived preview is measured by its
    // **logical size** (the initial raster's dimensions), so swapping in a sharper raster never
    // moves the on-screen size or the crop window at all (the HiDPI backing-store approach — the
    // crop itself is mapped to real pixels by ratio).
    let (nw, nh) = match logical {
        Some((lw, lh)) if lw > 0 && lh > 0 => (
            (lw as u16).div_ceil(font_size.width.max(1)),
            (lh as u16).div_ceil(font_size.height.max(1)),
        ),
        _ => {
            let n = Resize::natural_size(src, font_size);
            (n.width, n.height)
        }
    };
    let base = centered_rect((nw, nh), inner, false);
    // The logical display size after zoom.
    let disp_w = (base.width as f64 * zoom).max(1.0);
    let disp_h = (base.height as f64 * zoom).max(1.0);
    // The on-screen size = min(display size, display area).
    let tw = (disp_w.min(inner.width as f64).round() as u16).max(1);
    let th = (disp_h.min(inner.height as f64).round() as u16).max(1);
    // The visible fraction on each axis (<1 means it's clipped = pan is possible).
    let fw = (tw as f64 / disp_w).min(1.0);
    let fh = (th as f64 / disp_h).min(1.0);
    // Clamp the center so the visible window stays within the image.
    let cx = center.0.clamp(fw / 2.0, 1.0 - fw / 2.0);
    let cy = center.1.clamp(fh / 2.0, 1.0 - fh / 2.0);
    // Crop the visible window out of the original image (px).
    let cw = ((sw as f64 * fw).round() as u32).clamp(1, sw);
    let ch = ((sh as f64 * fh).round() as u32).clamp(1, sh);
    let x0 = ((cx * sw as f64) as i64 - cw as i64 / 2).clamp(0, (sw - cw) as i64) as u32;
    let y0 = ((cy * sh as f64) as i64 - ch as i64 / 2).clamp(0, (sh - ch) as i64) as u32;
    let crop_rect = (x0, y0, cw, ch);
    // The display rect (centered). Optionally downscaled by render_scale (reduces transfer size = less render wait).
    let scale = render_scale.clamp(0.1, 1.0);
    let (tw, th) = if scale >= 0.999 {
        (tw, th)
    } else {
        (
            ((tw as f64 * scale).round() as u16).max(1),
            ((th as f64 * scale).round() as u16).max(1),
        )
    };
    let target = Rect {
        x: inner.x + (inner.width - tw.min(inner.width)) / 2,
        y: inner.y + (inner.height - th.min(inner.height)) / 2,
        width: tw.min(inner.width),
        height: th.min(inner.height),
    };
    Some(ImageLayout {
        target,
        crop_rect,
        center: (cx, cy),
        frac: (fw, fh),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn centered_fit_centers_and_downscales() {
        let inner = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        };
        // A small image: centered at native (40x15) without upscaling (allow_upscale=false).
        let r = centered_rect((40, 15), inner, false);
        assert_eq!((r.width, r.height), (40, 15));
        assert_eq!((r.x, r.y), (20, 4));

        // A large image: downscaled preserving aspect ratio and centered (200x100 → scale 0.24).
        let r = centered_rect((200, 100), inner, false);
        assert_eq!((r.width, r.height), (48, 24));
        assert_eq!((r.x, r.y), (16, 0));

        // When zoomed (allow_upscale=true): even a small image is upscaled to fill the area and centered.
        let r = centered_rect((40, 15), inner, true);
        // scale = min(80/40, 24/15)=min(2.0,1.6)=1.6 → 64x24, x=(80-64)/2=8.
        assert_eq!((r.width, r.height), (64, 24));
        assert_eq!((r.x, r.y), (8, 0));
    }

    /// fence_crop: pure-function verification of the visible fraction and clamping (ratio-based =
    /// maps to the same window even after re-rasterizing).
    #[test]
    fn fence_crop_clamps_and_scales() {
        // 2x zoom (f=0.5), centered: a half-size window at the center.
        let ((x, y, w, h), c) = fence_crop((800, 400), 0.5, (0.5, 0.5));
        assert_eq!((w, h), (400, 200));
        assert_eq!((x, y), (200, 100));
        assert_eq!(c, (0.5, 0.5));
        // Pan to the edge: the center is clamped and the window stays within the image.
        let ((x, _y, w, _h), c) = fence_crop((800, 400), 0.5, (1.0, 0.5));
        assert_eq!(x + w, 800, "右端で止まる");
        assert!((c.0 - 0.75).abs() < 1e-9, "中心は f/2 でクランプ");
        // A raster at 4x density (f=0.25): the same ratio window maps to 2x the pixels.
        let ((x1, _, w1, _), _) = fence_crop((800, 400), 0.25, (0.5, 0.5));
        let ((x2, _, w2, _), _) = fence_crop((1600, 800), 0.25, (0.5, 0.5));
        assert_eq!((x2, w2), (x1 * 2, w1 * 2));
    }
}
