// 動画サムネイルプレビュー: 外部ツール(ffmpegthumbnailer 優先・無ければ ffmpeg)で代表フレームを
// 1枚だけ抽出し、その PNG を読み込んで DynamicImage を返す。返り値は SVG と同じく app の image_src
// に載せ、以降は通常の画像経路(prepare_image→ワーカー再エンコード→kitty graphics)へそのまま流す。
// **端末内での動画"再生"はしない(サムネイルのみ)** — kitty graphics でのリアルタイム再生は
// CPU 過大・Ghostty では parser 律速で破綻するため(2026-06-27 調査・docs/AUDIT 参照)。
//
// 外部ツールはあくまで任意依存。どちらも無い/抽出失敗なら None を返し、呼び出し側は安全な
// フォールバック(ヒント表示)へ降格する(PRD §5 配布容易性・原則#3「未対応は安全に」)。
// ツール実行はメディアワーカースレッドで行うため、子プロセスのブロッキングは UI を塞がない。

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use image::DynamicImage;

/// Maximum side (px) of the extracted thumbnail. Since the image path shrinks it further to terminal cells, large enough to look crisp is sufficient.
const THUMB_MAX_PX: u32 = 1024;

/// Extract and return one representative frame from the video at `path`. Returns None if external tools are missing or extraction fails
/// (the caller degrades to a safe fallback with a hint).
pub fn thumbnail(path: &Path) -> Option<DynamicImage> {
    let out = temp_png_path();
    // ffmpegthumbnailer(専用・高速・代表フレーム自動選択)を優先。無ければ ffmpeg にフォールバック。
    let ok = run_ffmpegthumbnailer(path, &out) || run_ffmpeg(path, &out);
    let img = if ok {
        image::ImageReader::open(&out)
            .ok()
            .and_then(|r| r.with_guessed_format().ok())
            .and_then(|r| r.decode().ok())
    } else {
        None
    };
    let _ = std::fs::remove_file(&out); // 一時ファイルは即削除(成否によらず)
    img
}

/// Extract with ffmpegthumbnailer. `-s` = max side px / `-q` = quality (1-10) / `-c png`. The representative frame is auto-selected
/// by default (playback position ~10%). Returns true if it succeeds and the output is non-empty.
fn run_ffmpegthumbnailer(path: &Path, out: &Path) -> bool {
    let status = Command::new("ffmpegthumbnailer")
        .arg("-i")
        .arg(path)
        .arg("-o")
        .arg(out)
        .arg("-s")
        .arg(THUMB_MAX_PX.to_string())
        .arg("-q")
        .arg("8")
        .arg("-c")
        .arg("png")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    matches!(status, Ok(s) if s.success()) && out_is_nonempty(out)
}

/// Extract with ffmpeg. The `thumbnail` filter picks one representative frame from the first batch, and `scale` caps the max side
/// (`-2` = codec-compatible even dimensions). `-frames:v 1` = one frame / `-y` = overwrite / `-loglevel error` = quiet.
fn run_ffmpeg(path: &Path, out: &Path) -> bool {
    let vf = format!("thumbnail,scale='min({THUMB_MAX_PX},iw)':-2");
    let status = Command::new("ffmpeg")
        .arg("-y")
        .arg("-loglevel")
        .arg("error")
        .arg("-i")
        .arg(path)
        .arg("-frames:v")
        .arg("1")
        .arg("-vf")
        .arg(vf)
        .arg(out)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    matches!(status, Ok(s) if s.success()) && out_is_nonempty(out)
}

/// Whether the output file exists and is non-empty (verify by the actual file, since the content can be empty even with exit code 0).
fn out_is_nonempty(out: &Path) -> bool {
    std::fs::metadata(out).map(|m| m.len() > 0).unwrap_or(false)
}

/// Return a temp PNG path that does not collide within the process. Made unique with pid + an atomic counter (no dependence on randomness/time).
fn temp_png_path() -> PathBuf {
    static N: AtomicU64 = AtomicU64::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("konoma-vthumb-{}-{}.png", std::process::id(), n))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Returns None when external tools are missing or the target is not a video (does not crash; safe fallback).
    #[test]
    fn nonexistent_or_nonvideo_returns_none() {
        assert!(thumbnail(Path::new("/no/such/video.mp4")).is_none());
    }

    /// If ffmpeg is on PATH, verify that a thumbnail can actually be extracted from a generated tiny video, down to **the extracted frame's
    /// content (color) matching the source video** (not "an image appeared" but "the correct frame appeared").
    /// Skipped on environments without ffmpeg (an optional dependency, so it does not break CI).
    #[test]
    fn extracts_correct_frame_when_ffmpeg_available() {
        let has_ffmpeg = Command::new("ffmpeg")
            .arg("-version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !has_ffmpeg {
            eprintln!("skip: ffmpeg 不在");
            return;
        }
        // lavfi で 64x64・1秒の「緑一色」動画を生成し、抽出フレーム中央が緑であることを確認する。
        let vid = std::env::temp_dir().join("konoma-vthumb-test-green.mp4");
        let _ = std::fs::remove_file(&vid);
        let made = Command::new("ffmpeg")
            .args(["-y", "-loglevel", "error", "-f", "lavfi", "-i"])
            .arg("color=c=green:s=64x64:d=1")
            .arg(&vid)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        assert!(made, "テスト用動画の生成に失敗");

        let img = thumbnail(&vid).expect("ffmpeg があればサムネイルが取れるはず");
        assert!(img.width() > 0 && img.height() > 0, "サムネイル寸法が 0");
        // 中央ピクセルが緑優勢(g > r かつ g > b)＝抽出したのが確かに元動画のフレーム。
        let rgba = img.to_rgba8();
        let px = rgba.get_pixel(rgba.width() / 2, rgba.height() / 2);
        let (r, g, b) = (px[0], px[1], px[2]);
        assert!(
            g > r && g > b && g > 60,
            "中央が緑でない(抽出フレームが元動画と不一致?): rgb=({r},{g},{b})"
        );
        std::fs::remove_file(&vid).ok();
    }
}
