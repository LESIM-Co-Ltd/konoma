// 内蔵 画像レンダラ (M2 実装済み。実体は分離配置)。
//
// 方針: `ratatui-image` の StatefulImage で領域いっぱいに表示する (kitty graphics)。
// リサイズ/エンコードは UI スレッドをブロックしないよう別スレッド(tokio)へオフロードする。
//
// 実装の所在:
//   - 状態(デコード/ThreadProtocol 保持・反映): `app.rs` の load_image / apply_image_resize。
//   - 描画(枠＋StatefulImage): `ui/preview.rs` の render_image。
//   - オフロード(ワーカースレッド・チャネル): `main.rs` の resize_worker / poll ループ。
//
// GIF アニメーション(M6): 全フレームを RGBA に展開し、各フレームの表示時間とともに返す。
// app 側が poll ティックで期限の来たフレームへ進め、image_src を差し替えて再エンコードさせる。

use std::path::Path;
use std::time::Duration;

use image::{AnimationDecoder, DynamicImage};

/// Decode a still image (PNG/JPG/the first frame of a GIF, etc.). None on failure.
/// A pure function used both by media loading on a separate thread and by load_image on the UI thread.
pub fn decode_static(path: &Path) -> Option<DynamicImage> {
    image::ImageReader::open(path)
        .ok()?
        .with_guessed_format()
        .ok()?
        .decode()
        .ok()
}

/// Read only the pixel dimensions of an image, sniffing the format from the file's content (not its
/// extension). Used for inline Markdown images — including fetched remote images cached without an
/// extension — to reserve layout rows without decoding the whole file. None if it is not an image.
pub fn dimensions(path: &Path) -> Option<(u32, u32)> {
    image::ImageReader::open(path)
        .ok()?
        .with_guessed_format()
        .ok()?
        .into_dimensions()
        .ok()
}

/// Minimum display time for each GIF frame. Anything below this (including 0) is treated as 100ms
/// (a GIF delay=0 means "as fast as possible"; like browsers, snap to 100ms to prevent runaway).
const MIN_FRAME_DELAY: Duration = Duration::from_millis(20);
const DEFAULT_FRAME_DELAY: Duration = Duration::from_millis(100);

/// Upper bound on the RGBA bytes kept resident for an animated GIF (all frames stay expanded for
/// smooth cycling — that is the animation design). When the total would exceed this, every frame is
/// downscaled in halving steps: the animation keeps all frames, trading pixels for memory. Typical
/// GIFs are far below the bound and stay untouched; only pathological ones (e.g. 1080p × hundreds
/// of frames ≈ 500MB+) are reduced instead of ballooning resident memory.
const MAX_GIF_BYTES: usize = 128 * 1024 * 1024;

/// Expand a GIF into all frames (composited RGBA) plus their display times.
/// Returns None if it is not a GIF / decoding fails / there is only one frame (= treated as a still image),
/// and the caller falls back to the normal still-image loader (load_image).
pub fn decode_gif(path: &Path) -> Option<Vec<(DynamicImage, Duration)>> {
    decode_gif_with_budget(path, MAX_GIF_BYTES)
}

/// The frame target for shrink factor `shrink` against the original canvas (min 1px per side).
fn shrink_target(w: u32, h: u32, shrink: u32) -> (u32, u32) {
    ((w / shrink).max(1), (h / shrink).max(1))
}

/// `decode_gif` with an explicit byte budget (separated so tests can force the shrink path with a
/// tiny budget). Frames are decoded one at a time; when the running total exceeds the budget the
/// shrink factor doubles and the already-kept frames are downscaled to the same target, so every
/// frame ends up with identical dimensions (as the animation cycler expects).
fn decode_gif_with_budget(path: &Path, budget: usize) -> Option<Vec<(DynamicImage, Duration)>> {
    let file = std::fs::File::open(path).ok()?;
    let decoder = image::codecs::gif::GifDecoder::new(std::io::BufReader::new(file)).ok()?;
    let mut out: Vec<(DynamicImage, Duration)> = Vec::new();
    let mut canvas: Option<(u32, u32)> = None; // 元キャンバス寸法(縮小率の基準)
    let mut shrink = 1u32;
    let mut bytes = 0usize;
    for f in decoder.into_frames() {
        // 従来の collect_frames と同じく、1 フレームでも壊れていたら None=静止画へフォールバック。
        let f = f.ok()?;
        let delay: Duration = f.delay().into();
        let delay = if delay < MIN_FRAME_DELAY {
            DEFAULT_FRAME_DELAY
        } else {
            delay
        };
        let mut img = DynamicImage::ImageRgba8(f.into_buffer());
        let (cw, ch) = *canvas.get_or_insert((img.width(), img.height()));
        if shrink > 1 {
            let (tw, th) = shrink_target(cw, ch, shrink);
            img = img.resize_exact(tw, th, image::imageops::FilterType::Triangle);
        }
        bytes += (img.width() as usize) * (img.height() as usize) * 4;
        out.push((img, delay));
        // 予算超過: 縮小率を倍にし、既存フレームも同じターゲット寸法へ縮小し直す。
        // (1<<16 ガード=病的な予算でも無限ループしない。1px まで縮んだら諦めて保持。)
        while bytes > budget && shrink < (1 << 16) {
            shrink *= 2;
            let (tw, th) = shrink_target(cw, ch, shrink);
            bytes = 0;
            for (im, _) in out.iter_mut() {
                if im.width() != tw || im.height() != th {
                    *im = im.resize_exact(tw, th, image::imageops::FilterType::Triangle);
                }
                bytes += (im.width() as usize) * (im.height() as usize) * 4;
            }
        }
    }
    if out.len() < 2 {
        return None; // 単一フレーム = アニメ不要。静止画として扱わせる。
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_gif_real_sample_has_multiple_frames() {
        let p = Path::new("samples/sample.gif");
        if !p.exists() {
            return; // パッケージから samples が除外された環境ではスキップ
        }
        let frames = decode_gif(p).expect("sample.gif はアニメーションとしてデコードできるはず");
        assert!(frames.len() > 1, "アニメ GIF は 2 フレーム以上");
        // 各フレームは合成済みで同一キャンバスサイズ・delay は丸め済み下限以上。
        let (w0, h0) = (frames[0].0.width(), frames[0].0.height());
        assert!(w0 > 0 && h0 > 0);
        assert!(frames
            .iter()
            .all(|(img, d)| { img.width() == w0 && img.height() == h0 && *d >= MIN_FRAME_DELAY }));
    }

    #[test]
    fn decode_gif_budget_downscales_but_keeps_all_frames() {
        // 予算超過の GIF は「フレームを捨てる」のではなく「全フレームを同寸法へ縮小」する。
        // 小さな合成 GIF + 極小予算で縮小経路を強制し、フレーム数維持・寸法一致・予算内を固定。
        use image::codecs::gif::GifEncoder;
        use image::{Delay, Frame, Rgba, RgbaImage};
        let dir = std::env::temp_dir().join("konoma_gif_budget_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("big.gif");
        {
            let out = std::fs::File::create(&p).unwrap();
            let mut enc = GifEncoder::new(out);
            let frames = (0..4u8).map(|i| {
                Frame::from_parts(
                    RgbaImage::from_pixel(64, 64, Rgba([i * 60, 100, 200, 255])),
                    0,
                    0,
                    Delay::from_numer_denom_ms(100, 1),
                )
            });
            enc.encode_frames(frames).unwrap();
        }
        // 64x64 RGBA = 16,384B/枚 × 4枚 = 65,536B。予算 20,000B → 32x32(4,096B/枚)へ縮小されるはず。
        let frames = decode_gif_with_budget(&p, 20_000).expect("アニメとしてデコードできる");
        assert_eq!(frames.len(), 4, "フレームは捨てない");
        let (w, h) = (frames[0].0.width(), frames[0].0.height());
        assert!(w < 64 && h < 64, "予算超過で縮小される: {w}x{h}");
        assert!(
            frames
                .iter()
                .all(|(im, _)| im.width() == w && im.height() == h),
            "全フレーム同寸法(アニメ巡回の前提)"
        );
        let total: usize = frames
            .iter()
            .map(|(im, _)| im.width() as usize * im.height() as usize * 4)
            .sum();
        assert!(total <= 20_000, "合計バイトが予算内: {total}");
        // 既定予算では小さな GIF は無傷(縮小されない)。
        let frames = decode_gif(&p).expect("既定予算でもデコードできる");
        assert_eq!((frames[0].0.width(), frames[0].0.height()), (64, 64));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn decode_static_reads_png_and_rejects_non_image() {
        let dir = std::env::temp_dir().join("konoma_decode_static_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // 本物の PNG を書き出してデコード(寸法一致)。
        let png = dir.join("tiny.png");
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(7, 3, image::Rgb([9, 9, 9])))
            .save(&png)
            .unwrap();
        let img = decode_static(&png).expect("PNG はデコードできる");
        assert_eq!((img.width(), img.height()), (7, 3));
        // 画像でないデータは None(クラッシュしない)。
        let bad = dir.join("notimg.png");
        std::fs::write(&bad, b"definitely not an image").unwrap();
        assert!(decode_static(&bad).is_none(), "非画像は None");
        // 存在しないファイルも None。
        assert!(decode_static(&dir.join("missing.png")).is_none());
        std::fs::remove_dir_all(&dir).ok();
    }
}
