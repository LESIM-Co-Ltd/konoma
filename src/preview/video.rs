// Video thumbnail preview: extract a single representative frame with an external tool
// (ffmpegthumbnailer preferred, falling back to ffmpeg), load that PNG, and return a DynamicImage.
// The result is put onto the app's image_src just like SVG, and from there flows through the normal
// image path (prepare_image → worker re-encode → kitty graphics) unchanged.
// **We do not "play" video inside the terminal (thumbnail only)** — real-time playback via kitty
// graphics would be CPU-prohibitive and is parser-bound to the point of breaking down in Ghostty
// (investigated 2026-06-27; see docs/AUDIT).
//
// External tools are strictly an optional dependency. If neither is present or extraction fails,
// we return None and the caller degrades to a safe fallback (a hint message) — PRD §5 ease of
// distribution, principle #3 "unsupported must fail safely".
// Tool execution runs on the media worker thread, so a blocking child process never blocks the UI.

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
    // Prefer ffmpegthumbnailer (dedicated, fast, auto-picks a representative frame); fall back to ffmpeg.
    let ok = run_ffmpegthumbnailer(path, &out) || run_ffmpeg(path, &out);
    let img = if ok {
        image::ImageReader::open(&out)
            .ok()
            .and_then(|r| r.with_guessed_format().ok())
            .and_then(|r| r.decode().ok())
    } else {
        None
    };
    let _ = std::fs::remove_file(&out); // delete the temp file right away (regardless of success)
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
        // Generate a 64x64, 1-second solid-green video with lavfi and check the extracted frame's center is green.
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
        // The center pixel is green-dominant (g > r and g > b) = the extracted frame really is from the source video.
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
