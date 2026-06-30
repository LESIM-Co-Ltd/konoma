// SVG プレビュー: 純 Rust(resvg/usvg/tiny-skia)でラスタライズし、RGBA の DynamicImage を返す。
// 返り値は app の image_src に載せ、以降は通常の画像経路(prepare_image→ワーカー再エンコード→
// kitty graphics)へそのまま流す。chafa/onig と同じく外部 C ライブラリ不要(PRD §5)。
//
// 拡大は v1 では「ラスタを crop」(写真と同じ扱い)。ベクタの無限ズーム(ズーム毎に再ラスタ)は将来。
// そのため自然サイズが小さい SVG でも端末で粗くならないよう、最大辺を target px まで引き上げて描く。

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
fn shared_fontdb() -> Arc<usvg::fontdb::Database> {
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

/// Rasterize directly from a byte slice (for tests / future embedding). `max_px` = target px for the max side.
pub fn rasterize_bytes(data: &[u8], path: &Path, max_px: u32) -> Option<DynamicImage> {
    let opt = usvg::Options {
        // 相対参照(外部画像等)の基準ディレクトリを SVG の親に。
        resources_dir: path.parent().map(Path::to_path_buf),
        // fontdb は公開フィールド(Arc<Database>)。共有 DB を差し込んで毎回の再列挙を避ける。
        fontdb: shared_fontdb(),
        ..usvg::Options::default()
    };

    let tree = usvg::Tree::from_data(data, &opt).ok()?;
    let size = tree.size();
    let (w0, h0) = (size.width(), size.height());
    if !(w0 > 0.0 && h0 > 0.0) {
        return None;
    }
    // 小さい SVG は端末でくっきり出るよう最大辺を max_px まで拡大(縮小はしない)。
    let target = (max_px.max(1) as f32).min(HARD_MAX_PX as f32);
    let scale = (target / w0.max(h0)).max(1.0);
    let pw = ((w0 * scale).ceil() as u32).clamp(1, HARD_MAX_PX);
    let ph = ((h0 * scale).ceil() as u32).clamp(1, HARD_MAX_PX);

    let mut pixmap = tiny_skia::Pixmap::new(pw, ph)?;
    let transform = tiny_skia::Transform::from_scale(scale, scale);
    resvg::render(&tree, transform, &mut pixmap.as_mut());

    // tiny-skia は premultiplied alpha。image クレートは straight alpha なので demultiply して渡す
    // (半透明の縁が暗くならないように)。透明部分は kitty graphics 上で端末背景が透ける=テーマ追従。
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

    const TINY_SVG: &[u8] =
        br##"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10"><rect width="20" height="10" fill="#f00"/></svg>"##;

    #[test]
    fn rasterizes_small_svg_upscaled_and_opaque() {
        // 20x10 の最大辺 20 を max_px=800 まで拡大 → 800x400。
        let img = rasterize_bytes(TINY_SVG, Path::new("t.svg"), 800).expect("should rasterize");
        assert_eq!(img.width(), 800);
        assert_eq!(img.height(), 400);
        // 中央の塗りは不透明な赤。
        let p = img.to_rgba8();
        let px = p.get_pixel(400, 200);
        assert_eq!(px[3], 255, "fill should be opaque");
        assert!(
            px[0] > 200 && px[1] < 60 && px[2] < 60,
            "fill should be red"
        );
    }

    #[test]
    fn svg_max_px_controls_raster_size() {
        // max_px を変えると最大辺が追従する(設定で px を制御できることの確認)。
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
    fn warm_fontdb_primes_shared_singleton() {
        // warm_fontdb は共有 fontdb を1度だけ初期化する。以降 shared_fontdb() は同一 Arc を返す。
        warm_fontdb();
        let a = shared_fontdb();
        let b = shared_fontdb();
        assert!(
            std::sync::Arc::ptr_eq(&a, &b),
            "shared_fontdb はキャッシュした同一インスタンスを返す"
        );
        // ウォーム後はテキスト入り SVG もラスタライズできる(フォント DB が利用可能)。
        let with_text = br##"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="20"><text x="2" y="14">hi</text></svg>"##;
        assert!(
            rasterize_bytes(with_text, Path::new("t.svg"), 200).is_some(),
            "フォント DB 準備後はテキスト SVG も描ける"
        );
    }
}
