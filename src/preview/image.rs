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

/// Minimum display time for each GIF frame. Anything below this (including 0) is treated as 100ms
/// (a GIF delay=0 means "as fast as possible"; like browsers, snap to 100ms to prevent runaway).
const MIN_FRAME_DELAY: Duration = Duration::from_millis(20);
const DEFAULT_FRAME_DELAY: Duration = Duration::from_millis(100);

/// Expand a GIF into all frames (composited RGBA) plus their display times.
/// Returns None if it is not a GIF / decoding fails / there is only one frame (= treated as a still image),
/// and the caller falls back to the normal still-image loader (load_image).
pub fn decode_gif(path: &Path) -> Option<Vec<(DynamicImage, Duration)>> {
    let file = std::fs::File::open(path).ok()?;
    let decoder = image::codecs::gif::GifDecoder::new(std::io::BufReader::new(file)).ok()?;
    let frames = decoder.into_frames().collect_frames().ok()?;
    if frames.len() < 2 {
        return None; // 単一フレーム = アニメ不要。静止画として扱わせる。
    }
    let out: Vec<(DynamicImage, Duration)> = frames
        .into_iter()
        .map(|f| {
            let delay: Duration = f.delay().into();
            let delay = if delay < MIN_FRAME_DELAY {
                DEFAULT_FRAME_DELAY
            } else {
                delay
            };
            (DynamicImage::ImageRgba8(f.into_buffer()), delay)
        })
        .collect();
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
