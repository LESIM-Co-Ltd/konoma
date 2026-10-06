// Built-in image renderer (implemented in M2; the actual code lives in a separate file).
//
// Policy: display it filling the whole area with `ratatui-image`'s StatefulImage (kitty graphics).
// Resize/encode are offloaded to a separate thread (tokio) so the UI thread is never blocked.
//
// Where the implementation lives:
//   - State (decode / holding & applying ThreadProtocol): `app.rs`'s load_image / apply_image_resize.
//   - Rendering (border + StatefulImage): `ui/preview.rs`'s render_image.
//   - Offload (worker thread, channel): `main.rs`'s resize_worker / poll loop.
//
// GIF animation (M6): expand all frames to RGBA and return them along with each frame's display
// time. The app side advances to the frame whose deadline has arrived on each poll tick, swapping
// image_src to trigger re-encoding.

use std::path::Path;
use std::time::Duration;

use image::{AnimationDecoder, DynamicImage, ImageDecoder};

/// A decoded animated GIF's frames, each paired with its own display time.
type GifFrames = Vec<(DynamicImage, Duration)>;

/// Largest image side konoma decodes, in pixels. PNG and JPEG can declare up to 2^31 / 65535; real
/// images top out far below (a 100-megapixel medium-format photo is 11,600 px wide, a stitched
/// panorama or a map 30,000 px). The image crate checks this against the header **before** it
/// allocates anything.
const MAX_IMAGE_SIDE: u32 = 32_768;

/// Largest image area konoma decodes, in pixels. A 48-megapixel photo is 4.8e7 and an 11,000 x
/// 11,000 screenshot of a wall of monitors 1.2e8 — both must keep working; 1.5e8 (a 12,200 px
/// square, 600 MB as RGBA) is the line past which a single image is a memory attack rather than a
/// picture. Checked from the header, before the decode allocates.
const MAX_IMAGE_PIXELS: u64 = 150_000_000;

/// The decoder's own allocation limit, bytes — the image crate's default, stated here so it is
/// pinned instead of inherited (a change in the crate must not silently loosen it).
const MAX_DECODE_ALLOC: u64 = 512 * 1024 * 1024;

fn decode_limits() -> image::Limits {
    let mut l = image::Limits::default();
    l.max_image_width = Some(MAX_IMAGE_SIDE);
    l.max_image_height = Some(MAX_IMAGE_SIDE);
    l.max_alloc = Some(MAX_DECODE_ALLOC);
    l
}

/// Total decoded-pixel memory that background decodes may hold at once, in bytes. One 48-megapixel
/// photo needs about 400 MB while it decodes, so this lets two of those or a few dozen screenshots
/// run side by side but not sixteen 11,000 px images (the case that used to reach 10 GB). A single
/// request larger than the budget is let through alone rather than refused.
const DECODE_MEMORY_BUDGET: u64 = 1024 * 1024 * 1024;

/// A pool of bytes that decodes claim while they run. A struct (not just statics) so tests can
/// use a small private pool.
pub(crate) struct DecodeGate {
    used: std::sync::Mutex<u64>,
    freed: std::sync::Condvar,
    budget: u64,
}

static DECODE_GATE: DecodeGate = DecodeGate::new(DECODE_MEMORY_BUDGET);

thread_local! {
    /// This thread already holds a reservation (a nested request must not wait on itself).
    static HOLDING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// A claim on a `DecodeGate`; the bytes return to the pool when it is dropped.
pub(crate) struct DecodeReservation {
    gate: &'static DecodeGate,
    bytes: u64,
}

impl DecodeGate {
    pub(crate) const fn new(budget: u64) -> Self {
        Self {
            used: std::sync::Mutex::new(0),
            freed: std::sync::Condvar::new(),
            budget,
        }
    }

    /// Wait until `bytes` fit and claim them. A request larger than the whole budget waits for an
    /// empty pool and then runs alone. Only call from a worker thread — it can block.
    pub(crate) fn reserve(&'static self, bytes: u64) -> DecodeReservation {
        if HOLDING.with(|h| h.get()) {
            return DecodeReservation {
                gate: self,
                bytes: 0,
            };
        }
        let want = bytes.min(self.budget);
        let mut used = self.used.lock().unwrap_or_else(|e| e.into_inner());
        while *used != 0 && *used + want > self.budget {
            used = self.freed.wait(used).unwrap_or_else(|e| e.into_inner());
        }
        *used += want;
        HOLDING.with(|h| h.set(true));
        DecodeReservation {
            gate: self,
            bytes: want,
        }
    }

    #[cfg(test)]
    pub(crate) fn in_use(&self) -> u64 {
        *self.used.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl Drop for DecodeReservation {
    fn drop(&mut self) {
        if self.bytes == 0 {
            return;
        }
        HOLDING.with(|h| h.set(false));
        let mut used = self.gate.used.lock().unwrap_or_else(|e| e.into_inner());
        *used = used.saturating_sub(self.bytes);
        self.gate.freed.notify_all();
    }
}

/// Claim `bytes` of the shared decode budget (see `DECODE_MEMORY_BUDGET`).
pub(crate) fn reserve_decode_memory(bytes: u64) -> DecodeReservation {
    DECODE_GATE.reserve(bytes)
}

/// Decode a still image from `reader`. The header is read first: an image whose declared size is
/// over the limits is refused before anything is allocated, and the shared decode budget is
/// claimed for the decode. `max_side`, when set, shrinks the result (still inside the claim) so a
/// caller that keeps many images does not keep their full size.
fn decode_reader<R: std::io::BufRead + std::io::Seek>(
    mut reader: image::ImageReader<R>,
    max_side: Option<u32>,
) -> Option<DynamicImage> {
    reader.limits(decode_limits());
    let decoder = reader.into_decoder().ok()?;
    let (w, h) = decoder.dimensions();
    if u64::from(w) * u64::from(h) > MAX_IMAGE_PIXELS {
        return None;
    }
    let _claim = reserve_decode_memory(u64::from(w) * u64::from(h) * 8);
    let img = DynamicImage::from_decoder(decoder).ok()?;
    Some(match max_side {
        Some(m) if img.width().max(img.height()) > m => {
            img.resize(m, m, image::imageops::FilterType::Triangle)
        }
        _ => img,
    })
}

/// Decode a still image (PNG/JPG/the first frame of a GIF, etc.). None on failure.
/// A pure function used both by media loading on a separate thread and by load_image on the UI thread.
pub fn decode_static(path: &Path) -> Option<DynamicImage> {
    let reader = image::ImageReader::open(path)
        .ok()?
        .with_guessed_format()
        .ok()?;
    decode_reader(reader, None)
}

/// `decode_static` for an inline Markdown image: the result is shrunk so its longer side is at most
/// `max_side`. A document keeps every decoded image for as long as it is open, so what is kept is
/// what a terminal can show (`MD_IMAGE_MAX_SIDE`), not the 100-megapixel original. Images already
/// within the bound are returned untouched.
pub fn decode_static_capped(path: &Path, max_side: u32) -> Option<DynamicImage> {
    let reader = image::ImageReader::open(path)
        .ok()?
        .with_guessed_format()
        .ok()?;
    decode_reader(reader, Some(max_side))
}

/// `decode_static`, from bytes already in memory rather than a path — used by the media-diff worker
/// (`app/media_diff.rs::decode_image_side`) to decode a side whose bytes came from git/jj rather than
/// the filesystem. Format is guessed from the content, exactly like the path version's
/// `with_guessed_format` (never from an extension — there may be none to go by).
pub fn decode_static_bytes(bytes: &[u8]) -> Option<DynamicImage> {
    let reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .ok()?;
    decode_reader(reader, None)
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

/// Largest GIF logical screen, in pixels. A screen recording of a 4K display is 8.3e6; 3.6e7
/// (6000 x 6000) leaves room, and a crafted header of 65535 x 65535 (4.3e9 px, 17 GB per frame as
/// RGBA) is far past it.
const MAX_GIF_CANVAS_PIXELS: u64 = 36_000_000;

/// Whether a GIF logical screen of `(w, h)` is within `MAX_GIF_CANVAS_PIXELS`.
pub(crate) fn gif_canvas_within_limit((w, h): (u32, u32)) -> bool {
    u64::from(w) * u64::from(h) <= MAX_GIF_CANVAS_PIXELS
}

/// Most frames of one GIF konoma expands. Real animations are a few hundred at most.
const MAX_GIF_FRAMES: usize = 5_000;

/// Frames x canvas pixels konoma will composite for one GIF (about 4e9 pixel writes, several
/// seconds on a worker thread): a 1080p GIF may run to 1,900 frames, a 4K one to 480.
const MAX_GIF_WORK_PIXELS: u64 = 4_000_000_000;

/// Expand a GIF into all frames (composited RGBA) plus their display times.
/// Returns None if it is not a GIF / decoding fails / there is only one frame (= treated as a still image),
/// and the caller falls back to the normal still-image loader (load_image).
pub fn decode_gif(path: &Path) -> Option<GifFrames> {
    decode_gif_with_budget(path, MAX_GIF_BYTES)
}

/// The frame target for shrink factor `shrink` against the original canvas (min 1px per side).
fn shrink_target(w: u32, h: u32, shrink: u32) -> (u32, u32) {
    ((w / shrink).max(1), (h / shrink).max(1))
}

/// Upper bound on the RGBA bytes kept resident for an animated GIF **embedded inline in a Markdown
/// document** (`decode_gif_inline`). Smaller than the full-screen bound (`MAX_GIF_BYTES`): a single
/// document can embed several GIFs at once, each decoded independently and kept expanded for as
/// long as the document is open, so per-image memory needs to stay tighter to keep the total bounded.
const MAX_GIF_BYTES_INLINE: usize = 32 * 1024 * 1024;

/// `decode_gif`, budgeted for an inline Markdown image (see `MAX_GIF_BYTES_INLINE`). Same semantics:
/// None for a non-GIF / undecodable / single-frame GIF — the caller (the inline-image decode worker)
/// falls back to the normal still-image decode, which already handles those cases.
pub fn decode_gif_inline(path: &Path) -> Option<GifFrames> {
    decode_gif_with_budget(path, MAX_GIF_BYTES_INLINE)
}

/// `decode_gif` with an explicit byte budget (separated so tests can force the shrink path with a
/// tiny budget). Frames are decoded one at a time; when the running total exceeds the budget the
/// shrink factor doubles and the already-kept frames are downscaled to the same target, so every
/// frame ends up with identical dimensions (as the animation cycler expects).
fn decode_gif_with_budget(path: &Path, budget: usize) -> Option<GifFrames> {
    let file = std::fs::File::open(path).ok()?;
    decode_gif_from_reader(std::io::BufReader::new(file), budget).map(|(frames, _canvas)| frames)
}

/// `decode_gif_inline`, from bytes already in memory — used by the media-diff worker to animate an
/// old (git/jj) version of a GIF, without a path to read from. Same semantics: None for a non-GIF /
/// undecodable / single-frame GIF. Also returns the GIF's own logical-screen size (read from the
/// header) — distinct from any frame's own pixel size once the decode budget has downscaled frames
/// for memory — which `app::media_diff::decode_image_side` needs as this side's *intrinsic* size
/// (see `MediaDiffPictureDecoded::natural_px`); `decode_gif`/`decode_gif_inline` have no such need
/// (no other side's size to stay comparable with) and so drop it.
pub fn decode_gif_bytes_inline(bytes: &[u8]) -> Option<(GifFrames, (u32, u32))> {
    decode_gif_from_reader(std::io::Cursor::new(bytes), MAX_GIF_BYTES_INLINE)
}

/// The shared body of `decode_gif_with_budget`/`decode_gif_bytes_inline`, generic over the reader so
/// neither has to duplicate the frame/shrink loop. The `(u32, u32)` alongside the frames is the GIF's
/// own logical-screen size, read from the header before any frame is decoded — see
/// `decode_gif_bytes_inline`'s own doc comment for why that's the intrinsic size a caller comparing
/// this GIF's size against something else needs.
fn decode_gif_from_reader<R: std::io::Read + std::io::BufRead + std::io::Seek>(
    reader: R,
    budget: usize,
) -> Option<(GifFrames, (u32, u32))> {
    let mut decoder = image::codecs::gif::GifDecoder::new(reader).ok()?;
    let header_px = decoder.dimensions(); // before `into_frames()` consumes `decoder` below.
                                          // The logical screen is what every frame is composited onto, so it — not the frames' own
                                          // rectangles — decides the memory. Refuse an oversized one before the first frame allocates.
    let canvas_px = u64::from(header_px.0) * u64::from(header_px.1);
    if !gif_canvas_within_limit(header_px) {
        return None;
    }
    decoder.set_limits(decode_limits()).ok()?;
    // A frame being composited, the previous canvas, and the resized copy.
    let _claim = reserve_decode_memory(canvas_px * 4 * 3);
    let mut out: GifFrames = Vec::new();
    let mut canvas: Option<(u32, u32)> = None; // original canvas dimensions (baseline for the shrink factor)
    let mut shrink = 1u32;
    let mut bytes = 0usize;
    for (n, f) in decoder.into_frames().enumerate() {
        // Frames x canvas is the compositing work however small the kept copies become.
        if n >= MAX_GIF_FRAMES || (n as u64 + 1) * canvas_px > MAX_GIF_WORK_PIXELS {
            return None;
        }
        // Same as the old collect_frames: if even one frame is corrupt, return None = fall back
        // to a still image.
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
        // Over budget: double the shrink factor and re-shrink the existing frames to the same
        // target dimensions too.
        // (The 1<<16 guard means it never loops forever even with a pathological budget. Once it
        // has shrunk down to 1px, give up and keep it as-is.)
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
        return None; // a single frame = no animation needed. Let the caller treat it as a still image.
    }
    Some((out, header_px))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{sample_path_or_skip, unique_tmp};

    #[test]
    fn decode_gif_real_sample_has_multiple_frames() {
        let Some(p) = sample_path_or_skip("sample.gif") else {
            return;
        };
        let frames = decode_gif(&p).expect("sample.gif はアニメーションとしてデコードできるはず");
        assert!(frames.len() > 1, "アニメ GIF は 2 フレーム以上");
        // Each frame is already composited to the same canvas size, and delay is at or above the
        // rounded floor.
        let (w0, h0) = (frames[0].0.width(), frames[0].0.height());
        assert!(w0 > 0 && h0 > 0);
        assert!(frames
            .iter()
            .all(|(img, d)| { img.width() == w0 && img.height() == h0 && *d >= MIN_FRAME_DELAY }));
    }

    #[test]
    fn decode_gif_inline_real_sample_has_multiple_frames() {
        // Same fixture/skip-guard as decode_gif_real_sample_has_multiple_frames — this exercises
        // the smaller inline budget (MAX_GIF_BYTES_INLINE) used for Markdown-embedded GIFs.
        let Some(p) = sample_path_or_skip("sample.gif") else {
            return;
        };
        let frames = decode_gif_inline(&p)
            .expect("sample.gif はインライン経路でもアニメとしてデコードできるはず");
        assert!(frames.len() > 1, "アニメ GIF は 2 フレーム以上");
        let (w0, h0) = (frames[0].0.width(), frames[0].0.height());
        assert!(w0 > 0 && h0 > 0);
    }

    #[test]
    fn decode_gif_budget_downscales_but_keeps_all_frames() {
        // A GIF that's over budget doesn't "drop frames" — it "shrinks every frame to the same
        // size". Force the shrink path with a tiny synthetic GIF plus a tiny budget, and pin down
        // that the frame count is kept, the dimensions match, and it stays within budget.
        use image::codecs::gif::GifEncoder;
        use image::{Delay, Frame, Rgba, RgbaImage};
        let dir = unique_tmp("konoma_gif_budget_test");
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
        // 64x64 RGBA = 16,384B/frame × 4 frames = 65,536B. With a 20,000B budget, it should shrink
        // to 32x32 (4,096B/frame).
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
        // With the default budget, a small GIF is left untouched (not shrunk).
        let frames = decode_gif(&p).expect("既定予算でもデコードできる");
        assert_eq!((frames[0].0.width(), frames[0].0.height()), (64, 64));
    }

    #[test]
    fn decode_static_reads_png_and_rejects_non_image() {
        let dir = unique_tmp("konoma_decode_static_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // Write out a real PNG and decode it (dimensions should match).
        let png = dir.join("tiny.png");
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(7, 3, image::Rgb([9, 9, 9])))
            .save(&png)
            .unwrap();
        let img = decode_static(&png).expect("PNG はデコードできる");
        assert_eq!((img.width(), img.height()), (7, 3));
        // Non-image data returns None (does not crash).
        let bad = dir.join("notimg.png");
        std::fs::write(&bad, b"definitely not an image").unwrap();
        assert!(decode_static(&bad).is_none(), "非画像は None");
        // A missing file also returns None.
        assert!(decode_static(&dir.join("missing.png")).is_none());
    }

    #[test]
    fn decode_static_bytes_matches_the_path_version() {
        let dir = unique_tmp("konoma_decode_static_bytes_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let png = dir.join("tiny.png");
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(7, 3, image::Rgb([9, 9, 9])))
            .save(&png)
            .unwrap();
        let bytes = std::fs::read(&png).unwrap();
        let from_path = decode_static(&png).unwrap();
        let from_bytes = decode_static_bytes(&bytes).unwrap();
        assert_eq!(
            (from_path.width(), from_path.height()),
            (from_bytes.width(), from_bytes.height())
        );
        assert_eq!(
            from_path.to_rgba8().into_raw(),
            from_bytes.to_rgba8().into_raw()
        );
    }

    #[test]
    fn decode_static_bytes_rejects_garbage() {
        assert!(decode_static_bytes(b"definitely not an image").is_none());
        assert!(decode_static_bytes(b"").is_none());
    }

    #[test]
    fn decode_static_bytes_uses_the_bundled_samples() {
        for name in ["sample.png", "sample.jpg"] {
            let Some(p) = sample_path_or_skip(name) else {
                continue;
            };
            let bytes = std::fs::read(&p).unwrap();
            let from_path = decode_static(&p).expect("path 版はデコードできる");
            let from_bytes = decode_static_bytes(&bytes).expect("bytes 版もデコードできる");
            assert_eq!(
                (from_path.width(), from_path.height()),
                (from_bytes.width(), from_bytes.height()),
                "{name}: サイズが path 版と一致するはず"
            );
        }
    }

    #[test]
    fn decode_gif_bytes_inline_matches_the_path_version() {
        let Some(p) = sample_path_or_skip("sample.gif") else {
            return;
        };
        let bytes = std::fs::read(&p).unwrap();
        let from_path = decode_gif_inline(&p).expect("path 版はアニメとしてデコードできる");
        let (from_bytes, header_px) =
            decode_gif_bytes_inline(&bytes).expect("bytes 版もアニメとしてデコードできる");
        assert_eq!(
            from_path.len(),
            from_bytes.len(),
            "フレーム数が一致するはず"
        );
        for ((pi, pd), (bi, bd)) in from_path.iter().zip(from_bytes.iter()) {
            assert_eq!((pi.width(), pi.height()), (bi.width(), bi.height()));
            assert_eq!(pd, bd, "各フレームの表示時間も一致するはず");
        }
        // The header size must match the (unshrunk, at this small budget) first frame's own size —
        // this fixture is far below `MAX_GIF_BYTES_INLINE`, so no downscale should have happened.
        let (fw, fh) = image::GenericImageView::dimensions(&from_bytes[0].0);
        assert_eq!(
            header_px,
            (fw, fh),
            "予算内なので header サイズ==実デコードサイズのはず"
        );
    }

    #[test]
    fn decode_gif_bytes_inline_rejects_garbage_and_non_gif() {
        assert!(decode_gif_bytes_inline(b"not a gif at all").is_none());
        // A real PNG (not a GIF, and not animated) also returns None — falls back to the still-image
        // decode at the call site, exactly like the path version's own contract.
        let dir = unique_tmp("konoma_decode_gif_bytes_inline_png_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let png = dir.join("tiny.png");
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(4, 4, image::Rgb([1, 2, 3])))
            .save(&png)
            .unwrap();
        let bytes = std::fs::read(&png).unwrap();
        assert!(decode_gif_bytes_inline(&bytes).is_none());
    }
}
