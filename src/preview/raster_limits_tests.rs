//! Raster-image limits, second pass: why a picture could not be shown, how much memory a decode
//! really needs (and so what the shared decode budget must be told), the order in which waiting
//! decodes are served, and the limit on GIF work.
//!
//! The hostile-file cases themselves live in `hostile_tests`; this file is about the accounting and
//! scheduling around them.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use image::{DynamicImage, GenericImageView};

use crate::preview::image::{
    decode_gif_bytes_inline, decode_static_capped, decode_static_why, dimensions_within_limits,
    gif_work_within_limit, shrink_exact, shrink_to_fit, with_decode_ticket, DecodeGate,
    DecodeTicket, DecodeWish, ImageFailure,
};
use crate::test_support::unique_tmp;

const QUICK: Duration = Duration::from_secs(5);

/// A deterministic picture with edges, gradients and noise (so a filter bug shows).
fn pattern(w: u32, h: u32) -> image::RgbaImage {
    image::RgbaImage::from_fn(w, h, |x, y| {
        let n = (x.wrapping_mul(7919) ^ y.wrapping_mul(104_729)) % 41;
        let edge = if (x / 9 + y / 11) % 2 == 0 { 200 } else { 20 };
        image::Rgba([
            ((x * 255) / w.max(1)) as u8,
            ((y * 255) / h.max(1)) as u8,
            (edge + n) as u8,
            if x % 17 == 0 { 128 } else { 255 },
        ])
    })
}

fn max_diff(a: &DynamicImage, b: &DynamicImage) -> u8 {
    assert_eq!(a.dimensions(), b.dimensions());
    a.to_rgba8()
        .as_raw()
        .iter()
        .zip(b.to_rgba8().as_raw())
        .map(|(x, y)| x.abs_diff(*y))
        .max()
        .unwrap_or(0)
}

// ---- shrinking --------------------------------------------------------------------------------

/// The streaming shrink gives the picture `DynamicImage::resize_exact(.., Triangle)` used to give
/// (the filter and its normalisation are the same), for every sample layout a decoder returns.
#[test]
fn the_streaming_shrink_matches_the_triangle_resize() {
    let rgba = pattern(300, 200);
    let cases: Vec<(&str, DynamicImage)> = vec![
        ("rgba8", DynamicImage::ImageRgba8(rgba.clone())),
        (
            "rgb8",
            DynamicImage::ImageRgb8(DynamicImage::ImageRgba8(rgba.clone()).to_rgb8()),
        ),
        (
            "luma8",
            DynamicImage::ImageLuma8(DynamicImage::ImageRgba8(rgba.clone()).to_luma8()),
        ),
        (
            "luma-alpha8",
            DynamicImage::ImageLumaA8(DynamicImage::ImageRgba8(rgba.clone()).to_luma_alpha8()),
        ),
        (
            "rgba16",
            DynamicImage::ImageRgba16(DynamicImage::ImageRgba8(rgba.clone()).to_rgba16()),
        ),
    ];
    for (name, img) in cases {
        for (nw, nh) in [(100, 67), (150, 100), (299, 199), (37, 24)] {
            let got = shrink_exact(&img, nw, nh);
            // The reference works in the picture's own depth; compare in 8-bit RGBA.
            let want = DynamicImage::ImageRgba8(img.to_rgba8()).resize_exact(
                nw,
                nh,
                image::imageops::FilterType::Triangle,
            );
            let d = max_diff(&got, &want);
            assert!(d <= 2, "{name} {nw}x{nh}: largest sample difference {d}");
        }
    }
}

/// A one-pixel-wide line keeps its weight across a shrink (the filter widens with the ratio; a
/// point sample or an uneven box would drop or double it).
#[test]
fn the_streaming_shrink_does_not_lose_thin_lines() {
    let mut im = image::RgbImage::from_pixel(400, 40, image::Rgb([255, 255, 255]));
    for y in 0..40 {
        im.put_pixel(200, y, image::Rgb([0, 0, 0]));
    }
    let small = shrink_exact(&DynamicImage::ImageRgb8(im), 100, 10).to_rgb8();
    let darkest = (0..100).map(|x| small.get_pixel(x, 5)[0]).min().unwrap();
    assert!(
        darkest < 230,
        "the line is still visible: darkest {darkest}"
    );
    let dark_cols = (0..100).filter(|x| small.get_pixel(*x, 5)[0] < 250).count();
    assert!(dark_cols <= 3, "and still thin: {dark_cols} columns");
}

#[test]
fn the_longer_side_decides_the_size() {
    use crate::preview::image::fit_within;
    assert_eq!(fit_within(8000, 6000, 4096), (4096, 3072));
    assert_eq!(fit_within(6000, 8000, 4096), (3072, 4096));
    assert_eq!(fit_within(5000, 5000, 4096), (4096, 4096));
    assert_eq!(
        fit_within(10_000, 1, 4096),
        (4096, 1),
        "never below one pixel"
    );
    assert_eq!(fit_within(1, 10_000, 4096), (1, 4096));
    let out = shrink_to_fit(
        &DynamicImage::ImageRgb8(image::RgbImage::new(300, 200)),
        150,
    );
    assert_eq!(out.dimensions(), (150, 100));
}

/// Memory: shrinking a 4000 x 3000 picture to 2000 x 1500 takes the output and a few rows. The
/// `resize` this replaces built an f32 RGBA intermediate of 4000 x 1500 x 16 bytes (96 MB) first.
#[test]
fn the_shrink_allocates_the_output_and_a_few_rows_not_a_float_copy_of_the_picture() {
    let img = DynamicImage::ImageRgb8(DynamicImage::ImageRgba8(pattern(4000, 3000)).to_rgb8());
    let out_bytes = 2000u64 * 1500 * 3;
    let used = crate::mem_tests::allocated_by(|| {
        let _ = shrink_exact(&img, 2000, 1500);
    });
    assert!(
        used < out_bytes * 2,
        "allocated {used} bytes for a {out_bytes}-byte result"
    );
    // The thing it replaces, as the yardstick that proves this test can tell the difference.
    let old = crate::mem_tests::allocated_by(|| {
        let _ = img.resize_exact(2000, 1500, image::imageops::FilterType::Triangle);
    });
    assert!(old > out_bytes * 10, "resize allocated only {old}");
}

/// The decode-and-shrink an inline Markdown image goes through allocates little beyond the decode
/// itself. (Runs against the old code too: there the shrink alone added 32 MB here.)
#[test]
fn a_capped_decode_costs_barely_more_than_the_full_decode() {
    let dir = unique_tmp("konoma_raster_capped_alloc");
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("big.png");
    DynamicImage::ImageRgb8(DynamicImage::ImageRgba8(pattern(3000, 2000)).to_rgb8())
        .save(&p)
        .unwrap();
    let full = crate::mem_tests::allocated_by(|| {
        let _ = crate::preview::image::decode_static(&p).unwrap();
    });
    let capped = crate::mem_tests::allocated_by(|| {
        let r = decode_static_capped(&p, 1000).unwrap();
        assert_eq!(r.width(), 1000);
    });
    assert!(
        capped < full + 8 * 1024 * 1024,
        "capped {capped} bytes vs full {full}: the shrink added {}",
        capped.saturating_sub(full)
    );
}

// ---- reasons ---------------------------------------------------------------------------------

fn png_header_only(w: u32, h: u32, depth: u8, color: u8) -> Vec<u8> {
    fn crc32_update(mut c: u32, d: &[u8]) -> u32 {
        for &b in d {
            c ^= u32::from(b);
            for _ in 0..8 {
                c = if c & 1 != 0 {
                    0xedb8_8320 ^ (c >> 1)
                } else {
                    c >> 1
                };
            }
        }
        c
    }
    fn chunk(t: &[u8; 4], d: &[u8]) -> Vec<u8> {
        let mut c = Vec::new();
        c.extend_from_slice(&(d.len() as u32).to_be_bytes());
        c.extend_from_slice(t);
        c.extend_from_slice(d);
        let crc = crc32_update(crc32_update(0xffff_ffff, t), d);
        c.extend_from_slice(&(crc ^ 0xffff_ffff).to_be_bytes());
        c
    }
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[depth, color, 0, 0, 0]);
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    out.extend(chunk(b"IHDR", &ihdr));
    out.extend(chunk(b"IDAT", &[0x78, 0x01]));
    out
}

fn write(dir: &std::path::Path, name: &str, bytes: &[u8]) -> std::path::PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, bytes).unwrap();
    p
}

/// "Too large", "damaged" and "not an image" are told apart — they used to be one `None`, shown as
/// a problem with the terminal.
#[test]
fn a_failed_decode_says_why() {
    let dir = unique_tmp("konoma_raster_reasons");
    std::fs::create_dir_all(&dir).unwrap();

    // Over the pixel limit, over the side limit, and over the decoded-size limit while under the
    // pixel limit (16-bit RGBA, 11,000 x 11,000: 969 MB).
    for (w, h, depth, color) in [
        (60_000u32, 60_000u32, 8u8, 2u8),
        (40_000, 1, 8, 2),
        (11_000, 11_000, 16, 6),
    ] {
        let p = write(&dir, "huge.png", &png_header_only(w, h, depth, color));
        let t = Instant::now();
        assert_eq!(
            decode_static_why(&p).err(),
            Some(ImageFailure::TooLarge),
            "{w}x{h} depth {depth}"
        );
        assert!(t.elapsed() < QUICK);
    }

    // Damaged: a real PNG cut short, and a header-only PNG of ordinary size.
    let real = dir.join("ok.png");
    pattern(64, 64).save(&real).unwrap();
    let bytes = std::fs::read(&real).unwrap();
    let cut = write(&dir, "cut.png", &bytes[..bytes.len() / 2]);
    assert_eq!(decode_static_why(&cut).err(), Some(ImageFailure::Corrupt));
    let hollow = write(&dir, "hollow.png", &png_header_only(64, 64, 8, 2));
    assert_eq!(
        decode_static_why(&hollow).err(),
        Some(ImageFailure::Corrupt)
    );
    assert_eq!(
        decode_static_why(&dir.join("missing.png")).err(),
        Some(ImageFailure::Corrupt),
        "unreadable"
    );

    // Not an image at all: with no extension to go by the format is unknown ...
    let text = write(&dir, "notes.bin", b"definitely not an image, just words");
    assert_eq!(
        decode_static_why(&text).err(),
        Some(ImageFailure::UnsupportedFormat)
    );
    // ... and under a `.png` name it is a PNG that is not one (damaged).
    let fake = write(&dir, "notes.png", b"definitely not an image, just words");
    assert_eq!(decode_static_why(&fake).err(), Some(ImageFailure::Corrupt));

    // And a good one still decodes.
    assert!(decode_static_why(&real).is_ok());
}

#[test]
fn the_limits_are_checked_from_a_header_size() {
    assert!(dimensions_within_limits((12_247, 12_247)), "150 MP square");
    assert!(!dimensions_within_limits((12_248, 12_248)));
    assert!(dimensions_within_limits((32_768, 1)));
    assert!(!dimensions_within_limits((32_769, 1)));
}

/// Every failure that is shown has a message in both languages, and the numbers in the "too
/// large" one are the limits actually enforced.
#[test]
fn every_shown_reason_has_an_english_and_a_japanese_text() {
    use crate::i18n::Lang;
    for f in [
        ImageFailure::TooLarge,
        ImageFailure::Corrupt,
        ImageFailure::UnsupportedFormat,
    ] {
        let (en, jp) = (f.message(Lang::En).unwrap(), f.message(Lang::Jp).unwrap());
        assert!(!en.is_empty() && !jp.is_empty() && en != jp, "{f:?}");
        assert!(
            !jp.is_ascii(),
            "the Japanese text for {f:?} is really Japanese: {jp}"
        );
    }
    assert_eq!(
        ImageFailure::Cancelled.message(Lang::En),
        None,
        "never shown"
    );
    let en = ImageFailure::TooLarge.message(Lang::En).unwrap();
    let jp = ImageFailure::TooLarge.message(Lang::Jp).unwrap();
    for t in [en, jp] {
        assert!(t.contains("32,768"), "{t}");
        assert!(t.contains("512 MiB"), "{t}");
    }
    assert!(en.contains("150 megapixels"), "{en}");
    assert!(jp.contains("1.5 億"), "{jp}");
    assert_eq!(super::image::MAX_IMAGE_SIDE, 32_768);
    assert_eq!(super::image::MAX_IMAGE_PIXELS, 150_000_000);
}

#[test]
fn a_failure_survives_the_string_it_travels_in() {
    for f in [
        ImageFailure::TooLarge,
        ImageFailure::Corrupt,
        ImageFailure::UnsupportedFormat,
        ImageFailure::Cancelled,
    ] {
        assert_eq!(ImageFailure::from_code(f.code()), Some(f));
    }
    assert_eq!(ImageFailure::from_code("decode failed"), None);
}

// ---- the decode budget ----------------------------------------------------------------------

/// What the budget is told must cover what the decode really takes. The multiples are what was
/// measured (peak resident size over the decoded pixel bytes, then shrunk to 4096 px): baseline
/// JPEG 1.1-1.75, progressive JPEG 2.4, WebP 2.1, TIFF 2.5, PNG 1.2, BMP/TGA/PNM/QOI 1.5.
#[test]
fn the_budget_claim_covers_the_measured_peak() {
    let native = 100_000_000u64;
    let claim = |f| super::image::decode_peak_bytes(Some(f), native);
    assert!(claim(image::ImageFormat::Jpeg) as f64 >= 2.5 * native as f64);
    assert!(claim(image::ImageFormat::Tiff) as f64 >= 2.5 * native as f64);
    assert!(claim(image::ImageFormat::WebP) as f64 >= 2.2 * native as f64);
    assert!(claim(image::ImageFormat::Bmp) as f64 >= 1.6 * native as f64);
    assert!(claim(image::ImageFormat::Png) as f64 >= 1.3 * native as f64);
    assert!(
        super::image::decode_peak_bytes(None, native) as f64 >= 2.5 * native as f64,
        "an unknown format gets the large multiple"
    );
}

/// 20-megapixel JPEGs, eight at once: the claims that are admitted together add up to what fits in
/// the budget, using the measured peak of 1.75x for each (359 MB used to be claimed as 160 MB, and
/// eight of them reached 2 GB against a 1 GiB budget).
#[test]
fn eight_twenty_megapixel_jpegs_stay_inside_the_budget() {
    let native = 20_000_000u64 * 3;
    let claim = super::image::decode_peak_bytes(Some(image::ImageFormat::Jpeg), native);
    let budget = 1024 * 1024 * 1024u64;
    let at_once = (budget / claim).max(1);
    let measured_peak = (native as f64 * 1.75) as u64 + 16 * 1024 * 1024;
    assert!(
        at_once < 8,
        "eight cannot run together: {at_once} fit the budget"
    );
    assert!(
        at_once * measured_peak <= budget,
        "{at_once} decodes at {measured_peak} bytes each are within {budget}"
    );
}

fn wait_until(what: &str, mut ok: impl FnMut() -> bool) {
    let t = Instant::now();
    while !ok() {
        assert!(t.elapsed() < Duration::from_secs(10), "timed out: {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// The picture on screen goes first: of three waiting requests the one with the highest priority
/// (the latest overlay pass that drew it) is served first, whatever the arrival order.
#[test]
fn waiting_decodes_are_served_best_priority_first() {
    static GATE: DecodeGate = DecodeGate::new(100);
    let order = Arc::new(Mutex::new(Vec::<&'static str>::new()));
    let hold = GATE.reserve(100);
    let mut threads = Vec::new();
    for (name, prio) in [
        ("scrolled-away", 1u64),
        ("on-screen", 50),
        ("just-left", 20),
    ] {
        let order = order.clone();
        threads.push(std::thread::spawn(move || {
            let r = GATE
                .reserve_for(60, Some(Arc::new(DecodeTicket::new(prio))))
                .expect("admitted");
            order.lock().unwrap().push(name);
            std::thread::sleep(Duration::from_millis(30));
            drop(r);
        }));
        // Arrive one by one so that arrival order is not what decides.
        let n = threads.len();
        wait_until("waiter queued", || GATE.waiting() == n);
    }
    drop(hold);
    for t in threads {
        t.join().unwrap();
    }
    assert_eq!(
        *order.lock().unwrap(),
        ["on-screen", "just-left", "scrolled-away"]
    );
    assert_eq!(GATE.in_use(), 0);
}

/// Equal priority is first come, first served — and a large request at the head is not starved by
/// small ones that would fit beside what is running (they wait behind it). Written with plain
/// `reserve`, so it also describes the gate before priorities existed.
#[test]
fn a_large_request_is_not_starved_by_small_ones_arriving_after_it() {
    static GATE: DecodeGate = DecodeGate::new(100);
    static BIG_IN: AtomicBool = AtomicBool::new(false);
    static SMALL_IN: AtomicBool = AtomicBool::new(false);
    let hold = GATE.reserve(30);
    let big = std::thread::spawn(|| {
        let r = GATE.reserve(80); // 30 + 80 > 100: waits for `hold`
        BIG_IN.store(true, Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(100));
        drop(r);
    });
    std::thread::sleep(Duration::from_millis(200)); // `big` is queued
    let small = std::thread::spawn(|| {
        let r = GATE.reserve(10); // 30 + 10 fits — but `big` is ahead of it
        SMALL_IN.store(true, Ordering::SeqCst);
        drop(r);
    });
    std::thread::sleep(Duration::from_millis(400));
    assert!(
        !SMALL_IN.load(Ordering::SeqCst),
        "a small request jumped the queue ahead of the large one"
    );
    assert!(!BIG_IN.load(Ordering::SeqCst), "nothing fits yet");
    drop(hold);
    big.join().unwrap();
    small.join().unwrap();
    assert!(BIG_IN.load(Ordering::SeqCst) && SMALL_IN.load(Ordering::SeqCst));
    assert_eq!(GATE.in_use(), 0);
}

/// A request that nobody wants any more leaves the queue without decoding, even while the pool
/// stays full — whether it was cancelled (its cache entry dropped) or its generation moved on.
#[test]
fn a_request_nobody_wants_leaves_the_queue_without_running() {
    static GATE: DecodeGate = DecodeGate::new(100);
    let hold = GATE.reserve(100);

    let cancelled = Arc::new(DecodeTicket::new(5));
    let c2 = cancelled.clone();
    let t1 = std::thread::spawn(move || GATE.reserve_for(10, Some(c2)).err());

    let gen = Arc::new(std::sync::atomic::AtomicU64::new(7));
    let stale_gen = Arc::new(DecodeTicket::for_generation(5, gen.clone(), 7));
    let t2 = std::thread::spawn(move || GATE.reserve_for(10, Some(stale_gen)).err());

    wait_until("both queued", || GATE.waiting() == 2);
    cancelled.cancel();
    gen.store(8, Ordering::SeqCst);
    assert_eq!(t1.join().unwrap(), Some(ImageFailure::Cancelled));
    assert_eq!(t2.join().unwrap(), Some(ImageFailure::Cancelled));
    assert_eq!(GATE.waiting(), 0, "both left the queue");
    assert_eq!(GATE.in_use(), 100, "and neither took anything");
    drop(hold);
}

/// A stale request does not hold up the ones behind it: the head of the queue is the best request
/// somebody still wants.
#[test]
fn a_cancelled_request_at_the_head_does_not_block_the_next() {
    static GATE: DecodeGate = DecodeGate::new(100);
    let hold = GATE.reserve(100);
    let head = Arc::new(DecodeTicket::new(99));
    let h2 = head.clone();
    let t_head = std::thread::spawn(move || GATE.reserve_for(90, Some(h2)).err());
    wait_until("head queued", || GATE.waiting() == 1);
    let admitted = Arc::new(AtomicUsize::new(0));
    let a2 = admitted.clone();
    let t_next = std::thread::spawn(move || {
        let r = GATE
            .reserve_for(10, Some(Arc::new(DecodeTicket::new(1))))
            .expect("admitted");
        a2.fetch_add(1, Ordering::SeqCst);
        drop(r);
    });
    wait_until("next queued", || GATE.waiting() == 2);
    head.cancel();
    drop(hold);
    assert_eq!(t_head.join().unwrap(), Some(ImageFailure::Cancelled));
    t_next.join().unwrap();
    assert_eq!(admitted.load(Ordering::SeqCst), 1);
}

/// A decode that runs under a stale ticket does not decode at all.
#[test]
fn a_decode_under_a_cancelled_ticket_is_dropped_before_it_starts() {
    let dir = unique_tmp("konoma_raster_cancelled");
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("a.png");
    pattern(32, 32).save(&p).unwrap();
    let t = Arc::new(DecodeTicket::new(1));
    t.cancel();
    let r = with_decode_ticket(t, || decode_static_why(&p));
    assert_eq!(r.err(), Some(ImageFailure::Cancelled));
    // The same file under a live ticket decodes.
    let r = with_decode_ticket(Arc::new(DecodeTicket::new(1)), || decode_static_why(&p));
    assert!(r.is_ok());
}

/// Dropping the entry that wanted a picture cancels its request.
#[test]
fn dropping_the_wish_cancels_the_ticket() {
    let wish = DecodeWish::default();
    let ticket = wish.ticket();
    assert!(!ticket.is_stale());
    wish.set_priority(9);
    assert_eq!(ticket.priority(), 9);
    drop(wish);
    assert!(ticket.is_stale());
}

// ---- GIF work ---------------------------------------------------------------------------------

/// A valid GIF: a 1x1 frame repeated `frames` times on a `canvas` logical screen.
fn gif(canvas: (u16, u16), frames: usize) -> Vec<u8> {
    let mut out = b"GIF89a".to_vec();
    out.extend_from_slice(&canvas.0.to_le_bytes());
    out.extend_from_slice(&canvas.1.to_le_bytes());
    out.extend_from_slice(&[0x80, 0, 0, 0, 0, 0, 255, 255, 255]);
    for _ in 0..frames {
        out.extend_from_slice(&[0x21, 0xf9, 0x04, 0x00, 0x0a, 0x00, 0x00, 0x00]);
        out.extend_from_slice(&[0x2c, 0, 0, 0, 0, 1, 0, 1, 0, 0]);
        out.extend_from_slice(&[0x02, 0x02, 0x44, 0x01, 0x00]);
    }
    out.push(0x3b);
    out
}

/// The work limit, exactly. 400 million pixel-writes is 1080p for 192 frames, 720p for 434, 4K
/// for 48 (see `MAX_GIF_WORK_PIXELS` for how it was chosen).
#[test]
fn the_gif_work_limit_has_an_exact_boundary() {
    let limit = super::image::MAX_GIF_WORK_PIXELS;
    assert!(
        limit <= 500_000_000,
        "a GIF's work is bounded to seconds: {limit}"
    );
    let canvas = 1920 * 1080u64;
    let n = (limit / canvas) as usize;
    assert!(gif_work_within_limit(canvas, n));
    assert!(!gif_work_within_limit(canvas, n + 1));
    assert!(gif_work_within_limit(1, 5_000));
    assert!(
        !gif_work_within_limit(1, 5_001),
        "the frame cap holds at any size"
    );
    // The animations timed when it was chosen are inside it: 800 x 600 x 300 frames, 1080p x 150,
    // 4K x 40 (and a 720p capture of 400 frames).
    assert!(gif_work_within_limit(800 * 600, 300));
    assert!(gif_work_within_limit(1920 * 1080, 150));
    assert!(gif_work_within_limit(3840 * 2160, 40));
    assert!(gif_work_within_limit(1280 * 720, 400));
    // And the old limit's 1000 x 1000 x 4000 frames (53-71 s) is not.
    assert!(!gif_work_within_limit(1000 * 1000, 4000));
}

/// A GIF past the limit is refused from its block structure, in milliseconds — before any frame
/// is composited. (It used to composite up to the old, much larger, limit first.)
#[test]
fn a_gif_over_the_work_limit_is_refused_before_compositing() {
    let limit = super::image::MAX_GIF_WORK_PIXELS;
    let canvas = (1000u16, 1000u16);
    let frames = (limit / (u64::from(canvas.0) * u64::from(canvas.1))) as usize + 1;
    let bytes = gif(canvas, frames);
    let t = Instant::now();
    assert!(decode_gif_bytes_inline(&bytes).is_none());
    assert!(
        t.elapsed() < Duration::from_secs(2),
        "refused in {:?}",
        t.elapsed()
    );
}

/// The frame count comes from the block structure and agrees with the decoder.
#[test]
fn an_ordinary_gif_still_decodes_all_its_frames() {
    let (frames, canvas) = decode_gif_bytes_inline(&gif((64, 48), 7)).expect("decodes");
    assert_eq!(frames.len(), 7);
    assert_eq!(canvas, (64, 48));
}

/// Not a GIF, or cut off in the middle of its blocks: refused, not a panic or a hang.
#[test]
fn a_gif_with_broken_blocks_is_refused() {
    let good = gif((8, 8), 3);
    for cut in [0, 3, 12, 14, 20, good.len() - 1] {
        let _ = decode_gif_bytes_inline(&good[..cut]);
    }
    assert!(decode_gif_bytes_inline(&good[..good.len() - 1]).is_none());
    let mut bad_block = good.clone();
    bad_block[19] = 0x55; // not a block introducer
    assert!(decode_gif_bytes_inline(&bad_block).is_none());
}

// ---- exact limits ---------------------------------------------------------------------------------

/// Every limit at its exact boundary, and each constant pinned (changing one is a decision, not an
/// accident).
#[test]
fn the_decode_limits_are_exact_at_every_boundary() {
    use crate::preview::image::{
        within_decode_limits, DECODE_MEMORY_BUDGET, MAX_DECODE_ALLOC, MAX_IMAGE_PIXELS,
        MAX_IMAGE_SIDE,
    };
    assert_eq!(MAX_IMAGE_SIDE, 32_768);
    assert_eq!(MAX_IMAGE_PIXELS, 150_000_000);
    assert_eq!(MAX_DECODE_ALLOC, 512 * 1024 * 1024);
    assert_eq!(DECODE_MEMORY_BUDGET, 1024 * 1024 * 1024);
    // Side.
    assert!(within_decode_limits(32_768, 1, 1));
    assert!(!within_decode_limits(32_769, 1, 1));
    assert!(within_decode_limits(1, 32_768, 1));
    assert!(!within_decode_limits(1, 32_769, 1));
    // Pixel count: exactly 150 million is in, one more is out.
    assert!(within_decode_limits(15_000, 10_000, 1));
    assert!(!within_decode_limits(15_000, 10_001, 1));
    // Decoded size: exactly 512 MiB is in, one byte more is out.
    assert!(within_decode_limits(100, 100, MAX_DECODE_ALLOC));
    assert!(!within_decode_limits(100, 100, MAX_DECODE_ALLOC + 1));
}

/// The image crate's own limits are installed on the reader (a 32,769 px side is refused by the
/// decoder from the header; 32,768 decodes).
#[test]
fn a_real_png_at_the_side_limit_decodes_and_one_pixel_more_is_refused() {
    let dir = unique_tmp("konoma_raster_side_limit");
    std::fs::create_dir_all(&dir).unwrap();
    for (w, h, ok) in [
        (32_768u32, 1u32, true),
        (32_769, 1, false),
        (1, 32_768, true),
        (1, 32_769, false),
    ] {
        let p = dir.join(format!("{w}x{h}.png"));
        DynamicImage::ImageRgb8(image::RgbImage::from_pixel(w, h, image::Rgb([9, 9, 9])))
            .save(&p)
            .unwrap();
        let r = decode_static_why(&p);
        if ok {
            assert!(r.is_ok(), "{w}x{h} must decode: {:?}", r.err());
        } else {
            assert_eq!(r.err(), Some(ImageFailure::TooLarge), "{w}x{h}");
        }
    }
}

/// A picture inside the cap is returned as it was decoded (no shrink pass, so its sample depth is
/// kept); one pixel over is shrunk.
#[test]
fn the_cap_touches_only_what_is_over_it() {
    let dir = unique_tmp("konoma_raster_cap_edge");
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("deep.png");
    DynamicImage::ImageRgba16(image::ImageBuffer::from_pixel(
        40,
        10,
        image::Rgba([9u16; 4]),
    ))
    .save(&p)
    .unwrap();
    let at = decode_static_capped(&p, 40).unwrap();
    assert_eq!(at.dimensions(), (40, 10));
    assert_eq!(at.color(), image::ColorType::Rgba16, "untouched at the cap");
    let over = decode_static_capped(&p, 39).unwrap();
    assert_eq!(over.width(), 39);
    assert_eq!(over.color(), image::ColorType::Rgba8, "shrunk past it");
}

/// The size a decode claims from the shared budget is exactly what `decode_peak_bytes` says: a pool
/// one byte too full makes the decode wait, a pool with exactly that much room lets it run. (Uses
/// private gates, so the rest of the test run is not held up.)
#[test]
fn a_decode_claims_its_measured_peak_from_the_shared_budget() {
    use crate::preview::image::{with_test_gate, MAX_GIF_BYTES_INLINE};
    let dir = unique_tmp("konoma_raster_claims");
    std::fs::create_dir_all(&dir).unwrap();
    let png = dir.join("a.png");
    crate::test_support::write_solid_png(&png, 64, 64, [1, 2, 3]);

    fn run(
        gate: &'static DecodeGate,
        budget: u64,
        claim: u64,
        work: Arc<dyn Fn() -> bool + Send + Sync>,
    ) {
        // One byte too little room: the decode must wait for the pool.
        let hold = gate.reserve(budget - claim + 1);
        let (tx, rx) = std::sync::mpsc::channel();
        let w = work.clone();
        let t = std::thread::spawn(move || {
            let _ = tx.send(with_test_gate(gate, || w()));
        });
        assert!(
            rx.recv_timeout(Duration::from_millis(400)).is_err(),
            "the decode ran although the pool was one byte too full for its claim of {claim}"
        );
        drop(hold);
        assert_eq!(rx.recv_timeout(Duration::from_secs(20)), Ok(true));
        t.join().unwrap();
        // Exactly enough room: it runs while the pool is otherwise held.
        let hold = gate.reserve(budget - claim);
        let (tx, rx) = std::sync::mpsc::channel();
        let t = std::thread::spawn(move || {
            let _ = tx.send(with_test_gate(gate, || work()));
        });
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(20)),
            Ok(true),
            "a claim of {claim} fits a pool with exactly that much room"
        );
        t.join().unwrap();
        drop(hold);
    }

    // A PNG: twice its decoded bytes (64 x 64 x 3).
    static PNG_GATE: DecodeGate = DecodeGate::new(1_000_000);
    let p = png.clone();
    run(
        &PNG_GATE,
        1_000_000,
        64 * 64 * 3 * 2,
        Arc::new(move || decode_static_why(&p).is_ok()),
    );
    // A GIF: three canvases and the frames it may keep.
    static GIF_GATE: DecodeGate = DecodeGate::new(100_000_000);
    let bytes = gif((64, 48), 2);
    run(
        &GIF_GATE,
        100_000_000,
        64 * 48 * 4 * 3 + MAX_GIF_BYTES_INLINE as u64,
        Arc::new(move || decode_gif_bytes_inline(&bytes).is_some()),
    );
}

/// The gate counts a thread's reservation again after it released the previous one, and a nested
/// request does not release the outer one when it ends.
#[test]
fn the_gate_does_not_forget_who_holds_what() {
    static GATE: DecodeGate = DecodeGate::new(100);
    let a = GATE.reserve(30);
    drop(a);
    assert_eq!(GATE.in_use(), 0);
    let b = GATE.reserve(40);
    assert_eq!(
        GATE.in_use(),
        40,
        "the second reservation on this thread is counted"
    );
    // Nested: free, and ending it leaves the outer one in force.
    let nested = GATE.reserve(10);
    assert_eq!(GATE.in_use(), 40);
    drop(nested);
    let again = GATE.reserve(5);
    assert_eq!(
        GATE.in_use(),
        40,
        "still nested under the outer claim, so still free"
    );
    drop(again);
    drop(b);
    assert_eq!(GATE.in_use(), 0);
}

/// A request that exactly fills what is left is admitted at once (the boundary is "fits", not
/// "fits with room to spare").
#[test]
fn a_request_that_exactly_fills_the_pool_is_admitted() {
    static GATE: DecodeGate = DecodeGate::new(100);
    let a = GATE.reserve(60);
    let (tx, rx) = std::sync::mpsc::channel();
    let t = std::thread::spawn(move || {
        let b = GATE.reserve(40);
        let _ = tx.send(GATE.in_use());
        drop(b);
    });
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(2)),
        Ok(100),
        "60 + 40 fills a pool of 100 exactly"
    );
    t.join().unwrap();
    drop(a);
}

/// A GIF whose logical screen is wider than the decoder allows is refused by the decoder's own
/// limits even though its pixel count is tiny; 32,768 wide decodes.
#[test]
fn a_gif_screen_wider_than_the_side_limit_is_refused() {
    assert!(decode_gif_bytes_inline(&gif((33_000, 1), 2)).is_none());
    assert!(decode_gif_bytes_inline(&gif((32_768, 1), 2)).is_some());
}

/// The exact work boundary on a canvas that divides the limit evenly.
#[test]
fn the_gif_work_limit_admits_exactly_the_limit() {
    let limit = super::image::MAX_GIF_WORK_PIXELS;
    assert_eq!(limit, 400_000_000);
    assert!(
        gif_work_within_limit(1_000_000, 400),
        "exactly at the limit"
    );
    assert!(!gif_work_within_limit(1_000_000, 401), "one frame over");
    assert!(
        gif_work_within_limit(80_000, 5_000),
        "the frame cap and the limit meet here"
    );
}

// ---- review fixes: GIF block scan, zero-byte claims, empty pictures -------------------------------

/// A reader that counts the bytes pulled out of it, to see how much a `BufReader` over it re-reads.
struct Counting<R> {
    inner: R,
    pulled: Arc<AtomicUsize>,
}

impl<R: std::io::Read> std::io::Read for Counting<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.pulled.fetch_add(n, Ordering::SeqCst);
        Ok(n)
    }
}

impl<R: std::io::Seek> std::io::Seek for Counting<R> {
    fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
        self.inner.seek(pos)
    }
}

/// One image whose data is `sub_blocks` sub-blocks of a single byte each (the shape that costs the
/// most per byte of file to scan: every block is a length byte and one data byte).
fn gif_of_tiny_sub_blocks(sub_blocks: usize) -> Vec<u8> {
    let mut out = b"GIF89a".to_vec();
    out.extend_from_slice(&[1, 0, 1, 0, 0, 0, 0]);
    out.extend_from_slice(&[0x2c, 0, 0, 0, 0, 1, 0, 1, 0, 0]);
    out.push(0x02);
    for _ in 0..sub_blocks {
        out.extend_from_slice(&[0x01, 0xaa]);
    }
    out.push(0x00);
    out.push(0x3b);
    out
}

/// Skipping sub-blocks used to be a seek on a `BufReader`, which throws its buffer away every time:
/// a million one-byte blocks made the scan read 8 KiB a block (a 20 MB file took six seconds, a
/// 100 MB one half a minute). Counted in bytes pulled from the file, so it does not depend on speed.
#[test]
fn scanning_a_gif_of_tiny_sub_blocks_reads_the_file_once() {
    let bytes = gif_of_tiny_sub_blocks(500_000);
    let pulled = Arc::new(AtomicUsize::new(0));
    let mut reader = std::io::BufReader::new(Counting {
        inner: std::io::Cursor::new(bytes.clone()),
        pulled: Arc::clone(&pulled),
    });
    let frames = crate::preview::image::count_gif_frames(&mut reader, 5_000, &|| false);
    assert_eq!(frames, Some(1));
    let total = pulled.load(Ordering::SeqCst);
    assert!(
        total <= 2 * bytes.len(),
        "the scan pulled {total} bytes out of a {} byte file",
        bytes.len()
    );
}

/// A scan whose request was abandoned stops reading (it looks every 64 Ki sub-blocks).
#[test]
fn a_stale_request_stops_scanning_a_gif() {
    let bytes = gif_of_tiny_sub_blocks(300_000);
    let looks = AtomicUsize::new(0);
    let stale = || {
        looks.fetch_add(1, Ordering::SeqCst);
        true
    };
    let got =
        crate::preview::image::count_gif_frames(&mut std::io::Cursor::new(&bytes), 5_000, &stale);
    assert_eq!(got, None, "an abandoned scan went on to the end");
    assert_eq!(
        looks.load(Ordering::SeqCst),
        1,
        "it stopped at the first look"
    );
    // The same file for a request that is still wanted.
    let got =
        crate::preview::image::count_gif_frames(&mut std::io::Cursor::new(&bytes), 5_000, &|| {
            false
        });
    assert_eq!(got, Some(1));
}

/// A claim of no bytes used to leave its thread marked as holding for good, so every later
/// claim of that thread went through the gate unaccounted.
#[test]
fn a_claim_of_no_bytes_does_not_leave_the_thread_marked_as_holding() {
    static GATE: DecodeGate = DecodeGate::new(1000);
    std::thread::spawn(|| {
        drop(GATE.reserve(0));
        let claim = GATE.reserve(400);
        assert_eq!(
            GATE.in_use(),
            400,
            "the second claim was waved through as nested"
        );
        drop(claim);
        assert_eq!(GATE.in_use(), 0);
        // And a genuinely nested claim still waits for nothing.
        let outer = GATE.reserve(900);
        let inner = GATE.reserve(900);
        assert_eq!(GATE.in_use(), 900);
        drop(inner);
        assert_eq!(
            GATE.in_use(),
            900,
            "the nested claim must not release the outer one"
        );
        drop(outer);
        assert_eq!(GATE.in_use(), 0);
        let again = GATE.reserve(300);
        assert_eq!(GATE.in_use(), 300);
        drop(again);
    })
    .join()
    .unwrap();
}

/// A picture with no pixels (a decoder can return one) must not panic the shrink.
#[test]
fn shrinking_a_picture_with_no_pixels_does_not_panic() {
    for (w, h) in [(0, 0), (0, 7), (7, 0)] {
        let img = DynamicImage::new_rgba8(w, h);
        let out = shrink_exact(&img, 4, 3);
        assert_eq!(out.dimensions(), (4, 3), "{w}x{h}");
        let out = shrink_to_fit(&img, 16);
        assert!(out.width() >= 1 && out.height() >= 1, "{w}x{h}");
    }
    let rgb = DynamicImage::new_rgb8(0, 3);
    assert_eq!(shrink_exact(&rgb, 2, 2).dimensions(), (2, 2));
    let luma16 = DynamicImage::new_luma16(5, 0);
    assert_eq!(shrink_exact(&luma16, 2, 2).dimensions(), (2, 2));
}
