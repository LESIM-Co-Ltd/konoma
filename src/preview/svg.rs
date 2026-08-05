// SVG preview: rasterize in pure Rust (resvg/usvg/tiny-skia) and return an RGBA DynamicImage.
// The result is put onto the app's image_src, and from there flows through the normal image path
// (prepare_image → worker re-encode → kitty graphics) unchanged. No external C library needed,
// just like chafa/onig (PRD §5).
//
// v1's zoom is "crop the raster" (treated the same as a photo). Infinite vector zoom (re-rasterize
// on every zoom) is future work. So that a small-intrinsic-size SVG doesn't look blocky in the
// terminal, we upscale it to draw at up to the target px on the max side.

use std::path::Path;
use std::sync::{Arc, OnceLock};

use image::DynamicImage;
use resvg::tiny_skia;
use resvg::usvg;

/// Safe upper bound (px) for the pixmap. Clamps each side so memory does not explode for SVGs with a huge viewBox.
/// Even if the `ui.svg_max_px` setting is large, it does not exceed this bound.
const HARD_MAX_PX: u32 = 4096;

/// Build the system font DB once and share it.
/// Because load_system_fonts can take several hundred ms on macOS, it is not re-enumerated each time an SVG is opened
/// (to avoid UI-thread stutter). Fonts are only needed for text inside SVGs.
///
/// `pub(crate)`: also reused by `preview::pdf`'s hayro `font_resolver` to pick a system CJK font for
/// PDFs that reference a non-embedded CJK font (Adobe-Japan1/GB1/CNS1/Korea1 predefined CMaps).
/// Sharing this one DB avoids a second several-hundred-ms `load_system_fonts()` enumeration.
pub(crate) fn shared_fontdb() -> Arc<usvg::fontdb::Database> {
    static DB: OnceLock<Arc<usvg::fontdb::Database>> = OnceLock::new();
    DB.get_or_init(|| {
        let mut db = usvg::fontdb::Database::new();
        db.load_system_fonts();
        Arc::new(db)
    })
    .clone()
}

/// **Warm up** the system font DB in advance (call from a separate thread at startup).
/// Hides the tens-of-ms font-enumeration freeze on the first SVG display behind startup.
pub fn warm_fontdb() {
    let _ = shared_fontdb();
}

/// Rasterize the SVG at `path` with a max side of `max_px` and return an RGBA image. Returns None on parse/render failure
/// (the caller falls back to text (raw XML) display).
pub fn rasterize(path: &Path, max_px: u32) -> Option<DynamicImage> {
    let data = std::fs::read(path).ok()?;
    rasterize_bytes(&data, path, max_px)
}

/// Parse the SVG at `path` and return its intrinsic pixel size (rounded up), without rasterizing.
/// Cheap enough for the UI thread (no pixmap allocation / rendering) — used to reserve layout rows for
/// an inline SVG image and to validate that a fetched remote file is really an SVG. None if not an SVG.
pub fn intrinsic_size(path: &Path) -> Option<(u32, u32)> {
    let data = std::fs::read(path).ok()?;
    let opt = usvg::Options {
        resources_dir: path.parent().map(Path::to_path_buf),
        fontdb: shared_fontdb(),
        ..usvg::Options::default()
    };
    let tree = usvg::Tree::from_data(&data, &opt).ok()?;
    let size = tree.size();
    let (w, h) = (size.width(), size.height());
    if !(w > 0.0 && h > 0.0) {
        return None;
    }
    Some((w.ceil() as u32, h.ceil() as u32))
}

/// Intrinsic size (rounded up) of an in-memory SVG, without rasterizing. Same as `intrinsic_size` but
/// from bytes — used for a synthesized SVG (e.g. a RaTeX math render) whose em units drive layout.
pub fn intrinsic_size_bytes(data: &[u8]) -> Option<(u32, u32)> {
    let opt = usvg::Options {
        fontdb: shared_fontdb(),
        ..usvg::Options::default()
    };
    let tree = usvg::Tree::from_data(data, &opt).ok()?;
    let size = tree.size();
    let (w, h) = (size.width(), size.height());
    if !(w > 0.0 && h > 0.0) {
        return None;
    }
    Some((w.ceil() as u32, h.ceil() as u32))
}

/// Rasterize directly from a byte slice (for tests / future embedding). `max_px` = target px for the max side.
pub fn rasterize_bytes(data: &[u8], path: &Path, max_px: u32) -> Option<DynamicImage> {
    let opt = usvg::Options {
        // Base directory for relative references (external images etc.) is the SVG's parent.
        resources_dir: path.parent().map(Path::to_path_buf),
        // fontdb is a public field (Arc<Database>). Plug in the shared DB to avoid re-enumerating every time.
        fontdb: shared_fontdb(),
        ..usvg::Options::default()
    };

    let tree = usvg::Tree::from_data(data, &opt).ok()?;
    let size = tree.size();
    let (w0, h0) = (size.width(), size.height());
    if !(w0 > 0.0 && h0 > 0.0) {
        return None;
    }
    // A small SVG is upscaled so its max side reaches max_px, for a crisp terminal display. A huge
    // diagram whose intrinsic size exceeds HARD_MAX is instead **shrunk to fit the whole thing**
    // (with a per-axis clamp at 1:1 scale, the transform stays 1:1 while only the pixmap is capped
    // at 4096px, silently cropping off the right/bottom).
    let target = (max_px.max(1) as f32).min(HARD_MAX_PX as f32);
    let m = w0.max(h0);
    let scale = (target / m).max(1.0).min(HARD_MAX_PX as f32 / m);
    let pw = ((w0 * scale).ceil() as u32).clamp(1, HARD_MAX_PX);
    let ph = ((h0 * scale).ceil() as u32).clamp(1, HARD_MAX_PX);

    let mut pixmap = tiny_skia::Pixmap::new(pw, ph)?;
    let transform = tiny_skia::Transform::from_scale(scale, scale);
    resvg::render(&tree, transform, &mut pixmap.as_mut());

    // tiny-skia uses premultiplied alpha. The image crate uses straight alpha, so we demultiply
    // before handing it over (so semi-transparent edges don't darken). Transparent areas let the
    // terminal background show through on kitty graphics = follows the theme.
    let mut rgba = Vec::with_capacity((pw * ph * 4) as usize);
    for px in pixmap.pixels() {
        let c = px.demultiply();
        rgba.push(c.red());
        rgba.push(c.green());
        rgba.push(c.blue());
        rgba.push(c.alpha());
    }
    let buf = image::RgbaImage::from_raw(pw, ph, rgba)?;
    Some(DynamicImage::ImageRgba8(buf))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::unique_tmp;

    const TINY_SVG: &[u8] =
        br##"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10"><rect width="20" height="10" fill="#f00"/></svg>"##;

    #[test]
    fn rasterizes_small_svg_upscaled_and_opaque() {
        // 20x10's max side of 20 is upscaled to max_px=800 → 800x400.
        let img = rasterize_bytes(TINY_SVG, Path::new("t.svg"), 800).expect("should rasterize");
        assert_eq!(img.width(), 800);
        assert_eq!(img.height(), 400);
        // The center fill is opaque red.
        let p = img.to_rgba8();
        let px = p.get_pixel(400, 200);
        assert_eq!(px[3], 255, "fill should be opaque");
        assert!(
            px[0] > 200 && px[1] < 60 && px[2] < 60,
            "fill should be red"
        );
    }

    #[test]
    fn oversize_svg_shrinks_to_fit_instead_of_cropping() {
        // Intrinsic size 8000x100 (past HARD_MAX=4096). Regression 2026-07-18: scale was
        // upscale-only (just max(..,1.0)), so it stayed at 1:1 and got drawn into a 4096px pixmap,
        // silently cropping off content at x>4096 (this rightmost blue rect). Confirm shrink-to-fit
        // makes the whole thing fit.
        let wide: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="8000" height="100"><rect width="8000" height="100" fill="#f00"/><rect x="7900" width="100" height="100" fill="#00f"/></svg>"##;
        let img = rasterize_bytes(wide, Path::new("t.svg"), 800).expect("should rasterize");
        assert!(img.width() <= HARD_MAX_PX, "最大辺は HARD_MAX 以下");
        // Roughly 4096x51 while preserving aspect ratio.
        assert!(img.width() >= 4000, "縮小フィット(切り落としでなく)");
        let p = img.to_rgba8();
        // The blue rect near the right edge survives = the right side isn't missing.
        let px = p.get_pixel(img.width() - 5, img.height() / 2);
        assert_eq!(px[3], 255, "右端まで描画されている");
        assert!(px[2] > 200 && px[0] < 60, "右端の青矩形が残る: {px:?}");
    }

    #[test]
    fn svg_max_px_controls_raster_size() {
        // The max side follows when max_px changes (confirms the setting controls px).
        let small = rasterize_bytes(TINY_SVG, Path::new("t.svg"), 400).unwrap();
        assert_eq!(small.width(), 400, "max_px=400 → 最大辺 400");
        let big = rasterize_bytes(TINY_SVG, Path::new("t.svg"), 1200).unwrap();
        assert_eq!(big.width(), 1200, "max_px=1200 → 最大辺 1200");
    }

    #[test]
    fn invalid_svg_returns_none() {
        assert!(rasterize_bytes(b"not an svg at all", Path::new("x.svg"), 800).is_none());
    }

    #[test]
    fn intrinsic_size_reads_declared_size_without_rasterizing() {
        let dir = unique_tmp("konoma_svg_intrinsic_test");
        let _ = std::fs::create_dir_all(&dir);
        let svg = dir.join("badge.svg");
        std::fs::write(&svg, TINY_SVG).unwrap();
        assert_eq!(intrinsic_size(&svg), Some((20, 10)), "declared 20x10");
        let bad = dir.join("not.svg");
        std::fs::write(&bad, b"not an svg at all").unwrap();
        assert!(intrinsic_size(&bad).is_none(), "非 SVG は None");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn warm_fontdb_primes_shared_singleton() {
        // warm_fontdb initializes the shared fontdb exactly once; shared_fontdb() returns the same Arc afterward.
        warm_fontdb();
        let a = shared_fontdb();
        let b = shared_fontdb();
        assert!(
            std::sync::Arc::ptr_eq(&a, &b),
            "shared_fontdb はキャッシュした同一インスタンスを返す"
        );
        // After warming, an SVG with text can also be rasterized (the font DB is available).
        let with_text = br##"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="20"><text x="2" y="14">hi</text></svg>"##;
        assert!(
            rasterize_bytes(with_text, Path::new("t.svg"), 200).is_some(),
            "フォント DB 準備後はテキスト SVG も描ける"
        );
    }
}
