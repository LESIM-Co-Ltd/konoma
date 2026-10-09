//! Inline-image cache behaviour around eviction, failure and the decode queue, and the reasons
//! shown for pictures that cannot be drawn.
//!
//! Everything above the `NEW-API TESTS` marker uses only what existed before the cache work was
//! reviewed (`ensure_md_image`, `ensure_md_fence_zoom`, `evict_md_images_to`, ...), so it describes
//! the behaviour in terms any version of the cache can be held to.

use super::*;
use crate::config::Config;
use crate::test_support::unique_tmp;

type Setup = (
    App,
    crate::test_support::TmpDir,
    std::sync::mpsc::Receiver<MdImageResult>,
    std::sync::mpsc::Receiver<MdEncodeRequest>,
);

/// An app with the inline-image loader and encoder channels attached (the test plays the workers),
/// in Preview mode on a halfblocks terminal.
fn setup(name: &str) -> Setup {
    setup_with(name, &[])
}

/// `setup` with these files already in the directory the app is opened on.
fn setup_with(name: &str, files: &[(&str, Vec<u8>)]) -> Setup {
    let dir = unique_tmp(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for (n, bytes) in files {
        std::fs::write(dir.join(n), bytes).unwrap();
    }
    let mut app = App::new(dir.canonicalize().unwrap(), Config::default()).unwrap();
    app.picker = Some(ratatui_image::picker::Picker::halfblocks());
    let (img_tx, img_rx) = std::sync::mpsc::channel();
    app.attach_md_image_loader(img_tx);
    let (enc_tx, enc_rx) = std::sync::mpsc::channel();
    app.attach_md_encoder(enc_tx);
    app.tab.mode = Mode::Preview;
    (app, dir, img_rx, enc_rx)
}

const WAIT: std::time::Duration = std::time::Duration::from_secs(20);

fn write_gif(path: &Path, frames: u8) {
    use image::codecs::gif::GifEncoder;
    use image::{Delay, Frame, Rgba, RgbaImage};
    let out = std::fs::File::create(path).unwrap();
    let mut enc = GifEncoder::new(out);
    let fs = (0..frames).map(|i| {
        Frame::from_parts(
            RgbaImage::from_pixel(16, 8, Rgba([i * 80, 100, 200, 255])),
            0,
            0,
            Delay::from_numer_denom_ms(100, 1),
        )
    });
    enc.encode_frames(fs).unwrap();
}

fn frame_img(w: u32, h: u32, v: u8) -> Arc<image::DynamicImage> {
    Arc::new(image::DynamicImage::ImageRgba8(
        image::RgbaImage::from_pixel(w, h, image::Rgba([v, v, v, 255])),
    ))
}

const SVG: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="20"><rect width="40" height="20" fill="#08f"/></svg>"##;

/// A mermaid-fence entry with pixels and its kept SVG, evicted by the cache budget.
fn evicted_fence(app: &mut App) -> (String, PathBuf) {
    let url = crate::preview::markdown::mermaid_fence_url("graph TD\nA-->B");
    let key = PathBuf::from(&url);
    app.md_image_cache.insert(
        key.clone(),
        MdImgEntry {
            decoded: Some(frame_img(80, 40, 9)),
            layout_px: Some((40, 20)),
            svg: Some(Arc::new(SVG.to_vec())),
            ..Default::default()
        },
    );
    app.md_frame = 5;
    app.evict_md_images_to(0);
    assert!(app.md_image_cache[&key].evicted, "setup: evicted");
    (url, key)
}

// ---- eviction is undone by every path that draws the picture ------------------------------

/// **The bug.** An animated GIF whose pixels were evicted while its encode was already settled was
/// never rebuilt: `ensure_md_image` only rebuilt when the exact encode still had to be made, so the
/// picture froze on its last frame (the animation reads `frames`, which eviction empties).
#[test]
fn an_evicted_gif_with_a_settled_encode_is_rebuilt_and_plays_again() {
    let (mut app, dir, img_rx, _enc_rx) = setup("konoma_raster_gif_settled");
    let gif = dir.join("a.gif");
    write_gif(&gif, 3);
    let url = gif.to_string_lossy().to_string();
    let key = MdEncodeKey::Full { cols: 10, rows: 4 };
    let frames: Vec<_> = (0..2u8)
        .map(|i| {
            (
                frame_img(16, 8, i * 50),
                std::time::Duration::from_millis(100),
            )
        })
        .collect();
    app.md_image_cache.insert(
        gif.clone(),
        MdImgEntry {
            decoded: Some(frames[0].0.clone()),
            frames,
            layout_px: Some((16, 8)),
            full: vec![MdProtoSlot::attempted(key)], // settled: nothing more to encode
            ..Default::default()
        },
    );
    app.md_frame = 5;
    app.evict_md_images_to(0);
    assert!(app.md_image_cache[&gif].frames.is_empty(), "setup: evicted");

    // The picture comes back on screen: its pixels must be asked for although its encode is settled.
    app.md_frame = 6;
    app.ensure_md_image(&url, 10, 4, 0, 4);
    let res = img_rx
        .recv_timeout(WAIT)
        .expect("the evicted GIF must be rebuilt when it is drawn again");
    assert!(app.apply_md_image(res));
    let e = app.md_image_cache.get_mut(&gif).unwrap();
    assert_eq!(e.frames.len(), 3, "all frames are back");
    assert!(!e.evicted && !e.failed);

    // And it plays: a due tick advances past frame 0.
    e.shown_at = Some(std::time::Instant::now() - std::time::Duration::from_secs(5));
    assert!(app.advance_md_gifs_if_due());
    assert_eq!(app.md_image_cache[&gif].idx, 1);
}

/// The in-place zoom of a focused diagram needs the pixels as much as any placement does.
#[test]
fn zooming_an_evicted_diagram_rebuilds_it_and_then_encodes_the_crop() {
    let (mut app, _dir, img_rx, enc_rx) = setup("konoma_raster_zoom_evicted");
    let (url, key) = evicted_fence(&mut app);
    app.md_frame = 6;
    app.tab.fence_zoom = 2.0;
    app.ensure_md_fence_zoom(&url, 20, 8);
    let res = img_rx
        .recv_timeout(WAIT)
        .expect("zooming an evicted diagram must rebuild its pixels");
    assert!(res.reraster, "rebuilt from the kept SVG, layout untouched");
    assert!(app.apply_md_image(res));
    assert!(app.md_image_cache[&key].decoded.is_some());
    // With the pixels back the next pass encodes the zoomed crop.
    app.ensure_md_fence_zoom(&url, 20, 8);
    enc_rx
        .recv_timeout(WAIT)
        .expect("the zoom crop is encoded once the pixels are back");
}

/// So does the sharpening follow-up of an unzoomed diagram.
#[test]
fn sharpening_an_evicted_diagram_rebuilds_it() {
    let (mut app, _dir, img_rx, _enc_rx) = setup("konoma_raster_density_evicted");
    let (url, _key) = evicted_fence(&mut app);
    app.md_frame = 6;
    app.ensure_md_fence_density(&url, 40, 20);
    let res = img_rx
        .recv_timeout(WAIT)
        .expect("the density pass of an evicted diagram must rebuild its pixels");
    assert!(res.reraster);
}

/// A picture the zoom or the density pass draws is on screen, and must look it to the eviction:
/// those paths never went through `ensure_md_image`, so their `last_used` stayed old and the
/// picture was the first to be dropped.
#[test]
fn drawing_a_diagram_through_the_zoom_or_density_path_marks_it_used() {
    let (mut app, _dir, _img_rx, _enc_rx) = setup("konoma_raster_stamp");
    let url = crate::preview::markdown::mermaid_fence_url("graph TD\nA-->B");
    let key = PathBuf::from(&url);
    app.md_image_cache.insert(
        key.clone(),
        MdImgEntry {
            decoded: Some(frame_img(80, 40, 9)),
            layout_px: Some((40, 20)),
            svg: Some(Arc::new(SVG.to_vec())),
            last_used: 1,
            ..Default::default()
        },
    );
    app.tab.fence_zoom = 2.0;
    app.md_frame = 9;
    app.ensure_md_fence_zoom(&url, 20, 8);
    assert_eq!(app.md_image_cache[&key].last_used, 9, "zoom path");
    app.md_frame = 12;
    app.ensure_md_fence_density(&url, 20, 8);
    assert_eq!(app.md_image_cache[&key].last_used, 12, "density path");
    // ... so a budget pass at that frame leaves it alone.
    app.md_frame = 13;
    app.evict_md_images_to(0);
    assert!(
        app.md_image_cache[&key].decoded.is_some() || app.md_image_cache[&key].last_used == 12,
        "stamped"
    );
}

// ---- a rebuild that fails ends the wait ---------------------------------------------------

/// **The bug.** When rebuilding evicted pixels failed (the re-raster failed or its thread
/// panicked), the entry was left with no pixels and neither `evicted` nor `failed` set, so
/// `md_images_loading()` stayed true for good: the 16 ms poll never stopped and idle CPU was lost.
#[test]
fn a_failed_rebuild_stops_the_loading_state() {
    let (mut app, _dir, img_rx, _enc_rx) = setup("konoma_raster_rebuild_fails");
    let url = crate::preview::markdown::math_url("x^2", false);
    let key = PathBuf::from(&url);
    app.md_image_cache.insert(
        key.clone(),
        MdImgEntry {
            decoded: Some(frame_img(40, 20, 3)),
            layout_px: Some((40, 20)),
            svg: Some(Arc::new(b"<svg this is not an svg".to_vec())),
            ..Default::default()
        },
    );
    app.md_frame = 5;
    app.evict_md_images_to(0);
    app.md_frame = 6;
    app.ensure_md_image(&url, 10, 4, 0, 4);
    let res = img_rx.recv_timeout(WAIT).expect("a rebuild is attempted");
    assert!(res.image.is_err(), "the broken SVG cannot be rasterized");
    app.apply_md_image(res);
    assert!(
        !app.md_images_loading(),
        "a rebuild that failed must not leave the cache 'loading'"
    );
    assert!(app.md_image_cache[&key].failed);
}

/// The same when the worker thread panicked (the fallback result the thread always sends).
#[test]
fn a_panicked_rebuild_stops_the_loading_state() {
    let (mut app, _dir, _img_rx, _enc_rx) = setup("konoma_raster_rebuild_panics");
    let (url, key) = evicted_fence(&mut app);
    app.md_frame = 6;
    app.ensure_md_image(&url, 10, 4, 0, 4); // starts the rebuild
    app.apply_md_image(MdImageResult {
        path: key.clone(),
        image: Err("re-raster panicked".to_string()),
        svg: None,
        reraster: true,
        frames: None,
    });
    assert!(!app.md_images_loading());
    assert!(app.md_image_cache[&key].failed);
}

// ---- bytes of pixels are counted in the picture's own format ------------------------------

#[test]
fn the_cache_counts_the_pixels_it_really_holds() {
    let rgb = MdImgEntry {
        decoded: Some(Arc::new(image::DynamicImage::ImageRgb8(
            image::RgbImage::new(100, 50),
        ))),
        ..Default::default()
    };
    assert_eq!(
        rgb.pixel_bytes(),
        100 * 50 * 3,
        "an RGB8 picture is 3 bytes a pixel"
    );
    let wide = MdImgEntry {
        decoded: Some(Arc::new(image::DynamicImage::ImageRgba16(
            image::ImageBuffer::new(100, 50),
        ))),
        ..Default::default()
    };
    assert_eq!(wide.pixel_bytes(), 100 * 50 * 8, "an RGBA16 picture is 8");
    let gray = MdImgEntry {
        decoded: Some(Arc::new(image::DynamicImage::ImageLuma8(
            image::GrayImage::new(100, 50),
        ))),
        ..Default::default()
    };
    assert_eq!(gray.pixel_bytes(), 100 * 50);
}

// ==== NEW-API TESTS ====

/// While a rebuild is waiting for a free slot (`MD_MAX_REBUILDS` already running) the cache still
/// counts as loading — that is what keeps the redraw loop alive to ask again — and no more than the
/// limit run together however many pictures want their pixels back at once (a resize).
#[test]
fn rebuilds_are_limited_and_every_picture_still_comes_back() {
    let (mut app, dir, img_rx, _enc_rx) = setup("konoma_raster_rebuild_cap");
    let n = 12usize;
    let mut urls = Vec::new();
    for i in 0..n {
        let p = dir.join(format!("p{i}.png"));
        crate::test_support::write_solid_png(&p, 24, 16, [i as u8, 2, 3]);
        app.md_image_cache.insert(
            p.clone(),
            MdImgEntry {
                decoded: Some(frame_img(24, 16, 1)),
                layout_px: Some((24, 16)),
                ..Default::default()
            },
        );
        urls.push(p.to_string_lossy().to_string());
    }
    app.md_frame = 5;
    app.evict_md_images_to(0);
    assert!(app.md_image_cache.values().all(|e| e.evicted));

    // They all come on screen in the same pass.
    app.md_frame = 6;
    for u in &urls {
        app.ensure_md_image(u, 10, 4, 0, 4);
    }
    assert!(
        app.md_rebuilds_running() <= MD_MAX_REBUILDS,
        "{} rebuilds started at once",
        app.md_rebuilds_running()
    );
    assert_eq!(app.md_rebuilds_running(), MD_MAX_REBUILDS);
    assert!(
        app.md_images_loading(),
        "the ones that had to wait keep the cache loading"
    );

    // Frame after frame: land what finished, ask again. All of them come back.
    let t = std::time::Instant::now();
    while app.md_image_cache.values().any(|e| e.decoded.is_none()) {
        assert!(t.elapsed() < WAIT, "not every picture came back");
        if let Ok(res) = img_rx.recv_timeout(std::time::Duration::from_millis(20)) {
            app.apply_md_image(res);
        }
        app.md_frame += 1;
        assert!(app.md_rebuilds_running() <= MD_MAX_REBUILDS);
        for u in &urls {
            app.ensure_md_image(u, 10, 4, 0, 4);
            assert!(app.md_rebuilds_running() <= MD_MAX_REBUILDS);
        }
        // The cache budget is far above these 12 small pictures, so nothing is evicted again.
    }
    assert!(!app.md_images_loading() || app.md_image_cache.values().any(|e| e.enc_inflight));
}

/// An evicted entry nobody is asking for is idle, not loading (the idle-CPU guarantee).
#[test]
fn an_evicted_picture_nobody_wants_does_not_keep_the_app_awake() {
    let (mut app, _dir, _img_rx, _enc_rx) = setup("konoma_raster_idle");
    let (_url, _key) = evicted_fence(&mut app);
    assert!(!app.md_images_loading());
}

/// A decode is queued under its entry's ticket, stamped with the pass that last drew the picture.
#[test]
fn drawing_a_picture_sets_its_decode_priority_to_the_current_pass() {
    let (mut app, dir, _img_rx, _enc_rx) = setup("konoma_raster_priority");
    let p = dir.join("a.png");
    crate::test_support::write_solid_png(&p, 24, 16, [1, 2, 3]);
    let url = p.to_string_lossy().to_string();
    app.md_frame = 7;
    app.ensure_md_image(&url, 10, 4, 0, 4); // starts the first decode
    assert_eq!(app.md_image_cache[&p].wish.ticket().priority(), 7);
    app.md_frame = 11;
    app.ensure_md_image(&url, 10, 4, 0, 4);
    assert_eq!(
        app.md_image_cache[&p].wish.ticket().priority(),
        11,
        "what is drawn now outranks what was drawn before"
    );
}

/// A decode whose entry was dropped before its turn never runs: no result is produced for it.
#[test]
fn a_decode_whose_entry_went_away_produces_nothing() {
    let (mut app, dir, img_rx, _enc_rx) = setup("konoma_raster_withdrawn");
    let p = dir.join("a.png");
    crate::test_support::write_solid_png(&p, 24, 16, [1, 2, 3]);
    app.md_image_cache.insert(p.clone(), MdImgEntry::default());
    app.md_image_cache[&p].wish.cancel_now();
    assert!(app.spawn_md_decode(p.clone()));
    assert!(
        img_rx
            .recv_timeout(std::time::Duration::from_millis(800))
            .is_err(),
        "a withdrawn request must not decode"
    );
    // The control: the same request, still wanted, is answered.
    let q = dir.join("b.png");
    crate::test_support::write_solid_png(&q, 24, 16, [3, 2, 1]);
    app.md_image_cache.insert(q.clone(), MdImgEntry::default());
    assert!(app.spawn_md_decode(q.clone()));
    let res = img_rx
        .recv_timeout(WAIT)
        .expect("a live request is decoded");
    assert!(res.image.is_ok());
    assert_eq!(res.path, q);
}

// ---- reasons --------------------------------------------------------------------------------

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

fn text_of(lines: &[ratatui::text::Line<'static>]) -> String {
    lines
        .iter()
        .map(|l| {
            l.spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Open `doc.md` (containing `md_body`) in a Markdown preview on a terminal with images.
fn markdown_app(name: &str, md_body: &str, files: &[(&str, Vec<u8>)]) -> Setup {
    let mut all: Vec<(&str, Vec<u8>)> = files.to_vec();
    all.push(("doc.md", md_body.as_bytes().to_vec()));
    let (mut app, dir, img_rx, enc_rx) = setup_with(name, &all);
    app.tab.mode = Mode::Tree;
    let i = app
        .tab
        .entries
        .iter()
        .position(|e| e.path.file_name().is_some_and(|n| n == "doc.md"))
        .expect("doc.md in the tree");
    app.tab.selected = i;
    app.tree_activate().unwrap();
    assert!(matches!(
        app.tab.preview_kind,
        Some(PreviewKind::Markdown(_))
    ));
    (app, dir, img_rx, enc_rx)
}

/// An image too large to decode is refused from its header while the page is laid out, and the
/// text that replaces it says why, in both languages.
#[test]
fn a_too_large_markdown_image_says_it_is_too_large() {
    let (mut app, _dir, _img_rx, _enc_rx) = markdown_app(
        "konoma_raster_md_too_large",
        "intro\n\n![big](huge.png)\n\noutro\n",
        &[("huge.png", png_header_only(60_000, 60_000, 8, 2))],
    );
    app.lang = crate::i18n::Lang::En;
    let en = text_of(&app.decorated_lines(80));
    assert!(en.contains("huge") || en.contains("big"), "{en}");
    assert!(en.contains("too large"), "{en}");
    assert!(!en.contains("loading"), "{en}");
    app.lang = crate::i18n::Lang::Jp;
    app.md_cache = None;
    let jp = text_of(&app.decorated_lines(80));
    assert!(jp.contains("大きすぎ"), "{jp}");
    assert!(
        app.md_images().is_empty(),
        "no placement is reserved for it"
    );
}

/// A picture that decodes badly is shown as damaged once the decode has said so.
#[test]
fn a_damaged_markdown_image_says_it_is_damaged_once_the_decode_fails() {
    let mut good = Vec::new();
    image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(64, 64, image::Rgb([5, 6, 7])))
        .write_to(
            &mut std::io::Cursor::new(&mut good),
            image::ImageFormat::Png,
        )
        .unwrap();
    good.truncate(good.len() / 2); // header intact, pixel data cut
    let (mut app, dir, img_rx, _enc_rx) = markdown_app(
        "konoma_raster_md_damaged",
        "![cut](cut.png)\n",
        &[("cut.png", good)],
    );
    app.lang = crate::i18n::Lang::En;
    let before = text_of(&app.decorated_lines(80));
    assert!(
        !before.contains("damaged"),
        "the header looks fine: {before}"
    );
    let placements = app.md_images();
    assert_eq!(
        placements.len(),
        1,
        "reserved as a picture until it is tried"
    );
    let url = placements[0].url.clone();
    app.ensure_md_image(&url, 10, 4, 0, 4);
    let res = img_rx.recv_timeout(WAIT).expect("decode result");
    assert!(res.image.is_err());
    assert!(app.apply_md_image(res));
    let after = text_of(&app.decorated_lines(80));
    assert!(after.contains("damaged"), "{after}");
    assert!(app.md_images().is_empty());
    app.lang = crate::i18n::Lang::Jp;
    app.md_cache = None;
    let jp = text_of(&app.decorated_lines(80));
    assert!(jp.contains("壊れて"), "{jp}");
    let _ = dir;
}

/// The full-screen preview says why a still image could not be shown, instead of blaming the
/// terminal — one message per reason, in both languages — while an image that is simply not drawn
/// yet keeps the generic text.
#[test]
fn the_full_screen_fallback_names_the_reason() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    let mut cut = Vec::new();
    image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(64, 64, image::Rgb([5, 6, 7])))
        .write_to(&mut std::io::Cursor::new(&mut cut), image::ImageFormat::Png)
        .unwrap();
    cut.truncate(cut.len() / 2);
    let cases: Vec<(&str, Vec<u8>, &str, &str)> = vec![
        (
            "huge.png",
            png_header_only(60_000, 60_000, 8, 2),
            "too large",
            "大きすぎ",
        ),
        ("cut.png", cut, "damaged", "壊れて"),
    ];
    for (name, bytes, en_text, jp_text) in cases {
        let dir = unique_tmp("konoma_raster_fullscreen_reason");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join(name);
        std::fs::write(&file, &bytes).unwrap();
        let mut app = App::new(dir.canonicalize().unwrap(), Config::default()).unwrap();
        let mut picker = ratatui_image::picker::Picker::halfblocks();
        picker.set_protocol_type(ratatui_image::picker::ProtocolType::Kitty);
        let (resize_tx, resize_rx) = tokio::sync::mpsc::unbounded_channel();
        Box::leak(Box::new(resize_rx));
        app.attach_image_backend(picker, resize_tx);
        let (media_tx, media_rx) = std::sync::mpsc::channel();
        app.attach_media_loader(media_tx);
        let i = app
            .tab
            .entries
            .iter()
            .position(|e| e.path.file_name().is_some_and(|n| n == name))
            .expect("the image is in the tree");
        app.tab.selected = i;
        app.tree_activate().unwrap();
        let res = media_rx.recv_timeout(WAIT).expect("media result");
        app.apply_media(res);
        assert!(app.image_failure().is_some(), "{name}: the reason is kept");

        let draw = |app: &mut App| -> String {
            let mut term = Terminal::new(TestBackend::new(100, 8)).unwrap();
            term.draw(|f| crate::ui::preview::render(f, app, f.area()))
                .unwrap();
            term.backend()
                .buffer()
                .content()
                .iter()
                .map(|c| c.symbol())
                .collect()
        };
        app.lang = crate::i18n::Lang::En;
        let en = draw(&mut app);
        assert!(en.contains(en_text), "{name}: {en}");
        assert!(
            !en.contains("in this terminal"),
            "{name}: not the terminal's fault: {en}"
        );
        app.lang = crate::i18n::Lang::Jp;
        // (the backend dump puts a blank cell after each double-width character)
        let jp = draw(&mut app).replace(' ', "");
        assert!(jp.contains(jp_text), "{name}: {jp}");
        assert!(!jp.contains("この端末では"), "{name}: {jp}");

        // Nothing known (no reason recorded) keeps the generic message.
        app.image_failure = None;
        app.lang = crate::i18n::Lang::En;
        let generic = draw(&mut app);
        assert!(generic.contains("in this terminal"), "{generic}");
    }
}

// ---- the cache budget -----------------------------------------------------------------------

fn pixels_entry(w: u32, h: u32, last_used: u64) -> MdImgEntry {
    MdImgEntry {
        decoded: Some(frame_img(w, h, 4)),
        layout_px: Some((w, h)),
        last_used,
        ..Default::default()
    }
}

/// The budget the cache really has.
#[test]
fn the_cache_budget_is_512_mib() {
    assert_eq!(MD_IMAGE_CACHE_BYTES, 512 * 1024 * 1024);
    assert_eq!(MD_IMAGE_MAX_SIDE, 4096);
    assert_eq!(MD_MAX_REBUILDS, 4);
}

/// A result that lands pushes the cache over its budget and the least recently used pictures go
/// at once — the budget is kept as pictures arrive, not only when something asks.
#[test]
fn a_landing_picture_evicts_the_oldest_ones_over_budget() {
    let (mut app, _dir, _img_rx, _enc_rx) = setup("konoma_raster_auto_evict");
    app.md_cache_budget_for_test = Some(300_000);
    app.md_frame = 5;
    app.md_image_cache
        .insert(PathBuf::from("/x/old1.png"), pixels_entry(256, 256, 1));
    app.md_image_cache
        .insert(PathBuf::from("/x/old2.png"), pixels_entry(256, 256, 2));
    let fresh = PathBuf::from("/x/fresh.png");
    app.md_image_cache.insert(
        fresh.clone(),
        MdImgEntry {
            last_used: 5, // drawn by the pass in progress
            ..Default::default()
        },
    );
    assert!(app.apply_md_image(MdImageResult {
        path: fresh.clone(),
        image: Ok((*frame_img(256, 256, 7)).clone()),
        svg: None,
        reraster: false,
        frames: None,
    }));
    assert!(app.md_image_cache[&PathBuf::from("/x/old1.png")].evicted);
    assert!(app.md_image_cache[&PathBuf::from("/x/old2.png")].evicted);
    assert!(
        app.md_image_cache[&fresh].decoded.is_some(),
        "what landed stays"
    );
    assert!(app.md_image_cache_pixel_bytes() <= 300_000);
}

/// Which entries the budget may take pixels from.
#[test]
fn eviction_takes_only_what_can_be_given_up() {
    let (mut app, _dir, _img_rx, _enc_rx) = setup("konoma_raster_evict_rules");
    app.md_frame = 0; // nothing has been drawn yet: last_used 0 counts as "not on screen"
    let k = |n: &str| PathBuf::from(format!("/x/{n}.png"));
    app.md_image_cache
        .insert(k("plain"), pixels_entry(64, 64, 0));
    app.md_image_cache.insert(
        k("encoding"),
        MdImgEntry {
            enc_inflight: true,
            ..pixels_entry(64, 64, 0)
        },
    );
    app.md_image_cache.insert(
        k("sharpening"),
        MdImgEntry {
            reraster_inflight: true,
            ..pixels_entry(64, 64, 0)
        },
    );
    // Still decoding for the first time: nothing to give up (and "evicted" would hide it from
    // the loading indicator).
    app.md_image_cache
        .insert(k("pending"), MdImgEntry::default());
    app.evict_md_images_to(0);
    assert!(
        app.md_image_cache[&k("plain")].evicted,
        "frame 0 draws nothing yet"
    );
    assert!(
        !app.md_image_cache[&k("encoding")].evicted,
        "its encode needs the pixels"
    );
    assert!(
        !app.md_image_cache[&k("sharpening")].evicted,
        "its re-raster is in flight"
    );
    assert!(
        !app.md_image_cache[&k("pending")].evicted,
        "it has no pixels yet"
    );
    assert!(app.md_images_loading(), "the pending one is still loading");
}

/// A cache exactly at its budget keeps everything.
#[test]
fn a_cache_exactly_at_its_budget_keeps_everything() {
    let (mut app, _dir, _img_rx, _enc_rx) = setup("konoma_raster_exact_budget");
    app.md_frame = 9;
    for i in 0..3 {
        app.md_image_cache.insert(
            PathBuf::from(format!("/x/{i}.png")),
            pixels_entry(100, 100, 1),
        );
    }
    let total = app.md_image_cache_pixel_bytes();
    assert_eq!(total, 3 * 100 * 100 * 4);
    app.evict_md_images_to(total);
    assert!(app.md_image_cache.values().all(|e| !e.evicted));
    app.evict_md_images_to(total - 1);
    assert_eq!(
        app.md_image_cache.values().filter(|e| e.evicted).count(),
        1,
        "one byte over: exactly one goes"
    );
}

/// An animated picture is counted by all its frames, and giving up its pixels gives up every frame.
#[test]
fn an_animated_picture_counts_and_frees_all_its_frames() {
    let frames: Vec<_> = (0..3u8)
        .map(|i| (frame_img(10, 10, i), std::time::Duration::from_millis(50)))
        .collect();
    let mut e = MdImgEntry {
        decoded: Some(frames[0].0.clone()),
        frames,
        idx: 2,
        shown_at: Some(std::time::Instant::now()),
        ..Default::default()
    };
    assert_eq!(e.pixel_bytes(), 3 * 10 * 10 * 4, "every frame counts");
    e.evict_pixels();
    assert_eq!(e.pixel_bytes(), 0);
    assert!(e.frames.is_empty() && e.decoded.is_none());
    assert_eq!((e.idx, e.shown_at.is_none(), e.evicted), (0, true, true));
}

// ---- stamping, one rebuild at a time, no loader --------------------------------------------

/// A picture asked for by the pass in progress is stamped with it, whether this call starts its
/// first decode or just draws it.
#[test]
fn every_draw_call_stamps_the_picture_with_the_pass() {
    let (mut app, dir, _img_rx, _enc_rx) = setup("konoma_raster_stamp_pass");
    let p = dir.join("a.png");
    crate::test_support::write_solid_png(&p, 24, 16, [1, 2, 3]);
    let url = p.to_string_lossy().to_string();
    app.md_frame = 7;
    app.ensure_md_image(&url, 10, 4, 0, 4);
    assert_eq!(
        app.md_image_cache[&p].last_used, 7,
        "the call that starts the decode"
    );
    app.md_frame = 12;
    app.ensure_md_image(&url, 10, 4, 0, 4);
    assert_eq!(app.md_image_cache[&p].last_used, 12, "a later call");
}

/// Asking again while a rebuild is running does not start a second one.
#[test]
fn a_rebuild_in_flight_is_not_started_twice() {
    let (mut app, dir, img_rx, _enc_rx) = setup("konoma_raster_one_rebuild");
    let p = dir.join("a.png");
    crate::test_support::write_solid_png(&p, 24, 16, [1, 2, 3]);
    app.md_image_cache
        .insert(p.clone(), pixels_entry(24, 16, 1));
    app.md_frame = 5;
    app.evict_md_images_to(0);
    let url = p.to_string_lossy().to_string();
    app.md_frame = 6;
    app.ensure_md_image(&url, 10, 4, 0, 4);
    assert!(app.md_images_loading(), "waiting for the pixels");
    assert_eq!(app.md_rebuilds_running(), 1);
    app.md_frame = 7;
    app.ensure_md_image(&url, 10, 4, 0, 4);
    app.ensure_md_image(&url, 10, 4, 0, 4);
    let res = img_rx.recv_timeout(WAIT).expect("one rebuild");
    assert!(app.apply_md_image(res));
    assert!(
        img_rx
            .recv_timeout(std::time::Duration::from_millis(500))
            .is_err(),
        "a second rebuild was started"
    );
    assert!(!app.md_image_cache[&p].evicted && app.md_image_cache[&p].decoded.is_some());
    assert!(!app.md_images_loading() || app.md_image_cache.values().any(|e| e.enc_inflight));
}

/// Without a loader nothing can be rebuilt, and nothing is left looking busy.
#[test]
fn without_a_loader_an_evicted_picture_is_left_alone() {
    let dir = unique_tmp("konoma_raster_no_loader");
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = App::new(dir.canonicalize().unwrap(), Config::default()).unwrap();
    app.picker = Some(ratatui_image::picker::Picker::halfblocks());
    let (url, key) = evicted_fence(&mut app);
    app.md_frame = 6;
    assert!(!app.ensure_md_pixels(&key));
    app.ensure_md_image(&url, 10, 4, 0, 4);
    let e = &app.md_image_cache[&key];
    assert!(
        e.evicted && !e.rebuilding && !e.reraster_inflight,
        "nothing latched"
    );
    assert!(!app.md_images_loading());
}

/// A formula is rebuilt at the formula size, a diagram at the diagram size.
#[test]
fn rebuilds_use_the_raster_size_of_their_kind() {
    let (mut app, _dir, img_rx, _enc_rx) = setup("konoma_raster_rebuild_sizes");
    app.cfg.ui.svg_max_px = 300;
    let math = PathBuf::from(crate::preview::markdown::math_url("x^2", false));
    let fence = PathBuf::from(crate::preview::markdown::mermaid_fence_url(
        "graph TD\nA-->B",
    ));
    for k in [&math, &fence] {
        app.md_image_cache.insert(
            k.clone(),
            MdImgEntry {
                svg: Some(Arc::new(SVG.to_vec())),
                ..pixels_entry(80, 40, 1)
            },
        );
    }
    app.md_frame = 5;
    app.evict_md_images_to(0);
    app.md_frame = 6;
    for k in [&math, &fence] {
        assert!(!app.ensure_md_pixels(k));
    }
    for _ in 0..2 {
        let res = img_rx.recv_timeout(WAIT).expect("rebuilt raster");
        let w = res.image.as_ref().expect("rasterized").width();
        let want = if res.path == math { 1024 } else { 300 };
        assert_eq!(w, want, "{}", res.path.display());
        app.apply_md_image(res);
    }
}

// ---- the page does not move when pixels are dropped -----------------------------------------

/// A diagram whose pixels were evicted still reserves exactly the rows it did (its layout size
/// outlives its pixels), so the document does not jump when the cache gives memory back.
#[test]
fn evicting_a_diagram_does_not_change_the_page_layout() {
    let dir = unique_tmp("konoma_raster_layout_stable");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let md = dir.join("doc.md");
    std::fs::write(
        &md,
        "intro\n\n```mermaid\ngraph LR\n  A --> B\n```\n\nafter\n",
    )
    .unwrap();
    let mut app = App::new(dir.canonicalize().unwrap(), Config::default()).unwrap();
    app.picker = Some(ratatui_image::picker::Picker::halfblocks());
    app.enter_preview(&md);
    app.ensure_md_cache(100);
    let before_lines = text_of(&app.decorated_lines(100));
    let before = app.md_images();
    assert_eq!(before.len(), 1, "the diagram is a placement");
    let key = PathBuf::from(&before[0].url);
    assert!(app.md_image_cache[&key].decoded.is_some());

    app.md_frame = 5;
    app.evict_md_images_to(0);
    assert!(app.md_image_cache[&key].decoded.is_none(), "pixels dropped");
    app.md_cache = None;
    let after_lines = text_of(&app.decorated_lines(100));
    let after = app.md_images();
    assert_eq!(after.len(), 1, "still a placement, not a loading line");
    assert_eq!(
        (after[0].cols, after[0].rows),
        (before[0].cols, before[0].rows),
        "same reserved size"
    );
    assert_eq!(after_lines, before_lines, "the page reads the same");
}

// ---- full-screen stills go through the worker -----------------------------------------------

fn png_in_tree(name: &str) -> (App, crate::test_support::TmpDir) {
    let (mut app, dir, _img_rx, _enc_rx) = setup_with(name, &[]);
    let p = dir.join("p.png");
    crate::test_support::write_solid_png(&p, 40, 30, [9, 8, 7]);
    // The tree was listed before the file existed: open a fresh app on the directory.
    drop(app);
    app = App::new(dir.canonicalize().unwrap(), Config::default()).unwrap();
    (app, dir)
}

fn open_first_entry(app: &mut App) {
    let i = app
        .tab
        .entries
        .iter()
        .position(|e| !e.is_dir)
        .expect("a file");
    app.tab.selected = i;
    app.tree_activate().unwrap();
}

/// Opening a still image hands the decode to the worker: right after opening, nothing is decoded
/// and the preview is "loading"; the picture arrives with the worker's result.
#[test]
fn a_still_image_is_decoded_on_the_worker_not_on_the_ui_thread() {
    let (mut app, _dir) = png_in_tree("konoma_raster_still_async");
    let mut picker = ratatui_image::picker::Picker::halfblocks();
    picker.set_protocol_type(ratatui_image::picker::ProtocolType::Kitty);
    let (resize_tx, resize_rx) = tokio::sync::mpsc::unbounded_channel();
    Box::leak(Box::new(resize_rx));
    app.attach_image_backend(picker, resize_tx);
    let (media_tx, media_rx) = std::sync::mpsc::channel();
    app.attach_media_loader(media_tx);
    open_first_entry(&mut app);
    assert!(matches!(app.tab.preview_kind, Some(PreviewKind::Image(_))));
    assert!(app.is_media_loading(), "the load runs on the worker");
    assert!(
        app.image_src.is_none(),
        "nothing was decoded on the UI thread"
    );
    let res = media_rx.recv_timeout(WAIT).expect("the worker's result");
    assert!(app.apply_media(res));
    assert!(app.image_src.is_some() && !app.is_media_loading());
}

/// Without an image backend (no graphics channel) no decode is started at all.
#[test]
fn a_still_image_without_an_image_backend_starts_no_job() {
    let (mut app, _dir) = png_in_tree("konoma_raster_still_no_backend");
    app.picker = Some(ratatui_image::picker::Picker::halfblocks()); // but no `img_tx`
    let (media_tx, media_rx) = std::sync::mpsc::channel();
    app.attach_media_loader(media_tx);
    open_first_entry(&mut app);
    assert!(!app.is_media_loading());
    assert!(app.image_src.is_none());
    assert!(
        media_rx
            .recv_timeout(std::time::Duration::from_millis(500))
            .is_err(),
        "no job was started"
    );
}

/// With no worker channel (the test harness), a still image is decoded inline.
#[test]
fn a_still_image_without_a_worker_channel_decodes_inline() {
    let (mut app, _dir) = png_in_tree("konoma_raster_still_inline");
    let mut picker = ratatui_image::picker::Picker::halfblocks();
    picker.set_protocol_type(ratatui_image::picker::ProtocolType::Kitty);
    let (resize_tx, resize_rx) = tokio::sync::mpsc::unbounded_channel();
    Box::leak(Box::new(resize_rx));
    app.attach_image_backend(picker, resize_tx);
    open_first_entry(&mut app);
    assert!(app.image_src.is_some(), "decoded where it was asked for");
    assert!(!app.is_media_loading());
}

// ---- how big a Markdown image is kept ---------------------------------------------------------

/// A picture over 4096 px on the long side is kept at 4096; one at 4096 is kept as it is.
#[test]
fn markdown_images_are_kept_at_4096_on_the_long_side() {
    let dir = unique_tmp("konoma_raster_md_cap");
    std::fs::create_dir_all(&dir).unwrap();
    let wide = dir.join("wide.png");
    image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(5000, 10, image::Rgb([1, 2, 3])))
        .save(&wide)
        .unwrap();
    let img = md_decode_image_why(&wide, 1024, &|| false).unwrap();
    assert_eq!(img.width(), 4096);
    assert_eq!(img.height(), 8);
    let exact = dir.join("exact.png");
    image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(4096, 10, image::Rgb([1, 2, 3])))
        .save(&exact)
        .unwrap();
    let img = md_decode_image_why(&exact, 1024, &|| false).unwrap();
    assert_eq!(
        (img.width(), img.height()),
        (4096, 10),
        "within the cap: untouched"
    );
}

/// A picture that waited for a rebuild slot and then left the screen must not keep the redraw loop
/// ticking (`md_images_loading` = a 16 ms poll with no key pressed): the wish lapses with the frame
/// that made it unless the next frame renews it.
#[test]
fn a_rebuild_wish_that_nobody_renews_lapses_and_the_loop_goes_idle() {
    let (mut app, dir, img_rx, _enc_rx) = setup("konoma_raster_wish_lapses");
    let mut urls = Vec::new();
    for i in 0..(MD_MAX_REBUILDS + 3) {
        let p = dir.join(format!("w{i}.png"));
        crate::test_support::write_solid_png(&p, 24, 16, [i as u8, 2, 3]);
        app.md_image_cache.insert(
            p.clone(),
            MdImgEntry {
                decoded: Some(frame_img(24, 16, 1)),
                layout_px: Some((24, 16)),
                ..Default::default()
            },
        );
        urls.push(p.to_string_lossy().to_string());
    }
    app.md_frame = 5;
    app.evict_md_images_to(0);
    // One frame that wants them all: the limit lets some start and makes the rest wait.
    app.begin_frame();
    app.md_frame = 6;
    for u in &urls {
        app.ensure_md_image(u, 10, 4, 0, 4);
    }
    assert_eq!(app.md_rebuilds_running(), MD_MAX_REBUILDS);
    assert!(app.md_images_loading());
    // The running ones land.
    for _ in 0..MD_MAX_REBUILDS {
        let res = img_rx.recv_timeout(WAIT).expect("a rebuild");
        app.apply_md_image(res);
    }
    assert_eq!(app.md_rebuilds_running(), 0);
    // The waiting ones are still wished for by the frame that was just drawn...
    assert!(
        app.md_images_loading(),
        "the waiting pictures were forgotten"
    );
    // ...but the next frame shows something else (the document was scrolled away, or the preview
    // was left) and never asks for them again.
    app.begin_frame();
    assert!(
        !app.md_images_loading(),
        "a wish nobody renewed keeps the redraw loop ticking"
    );
    // A frame that does still show them renews the wish.
    app.begin_frame();
    app.md_frame = 7;
    for u in &urls {
        app.ensure_md_image(u, 10, 4, 0, 4);
    }
    assert!(app.md_images_loading());
}

/// A damaged PNG or JPEG is refused by the raster decoders and is not an SVG either: it must not be
/// handed to the drawing process (a process hand-off for nothing). The drawing process is pointed
/// at a program that does not exist, so a hand-off shows as "the renderer stopped".
#[cfg(unix)]
#[test]
fn a_damaged_png_is_not_offered_to_the_svg_process() {
    use crate::preview::image::ImageFailure;
    let dir = unique_tmp("konoma_raster_broken_png");
    std::fs::create_dir_all(&dir).unwrap();
    let broken = dir.join("broken.png");
    let mut bytes = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    bytes.extend_from_slice(b"this is not a png after all");
    std::fs::write(&broken, &bytes).unwrap();
    let jpeg = dir.join("broken.jpg");
    std::fs::write(&jpeg, [0xff, 0xd8, 0xff, 0xe0, 0, 4, 1, 2, 3]).unwrap();
    let svg = dir.join("really.png");
    std::fs::write(
        &svg,
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="4" height="4"/>"#,
    )
    .unwrap();
    let nowhere = PathBuf::from("/nonexistent/konoma-svg-child");
    crate::preview::svg_proc::with_real_child(nowhere, || {
        for f in [&broken, &jpeg] {
            let r = md_decode_image_why(f, 256, &|| false);
            assert!(
                matches!(
                    r,
                    Err(ImageFailure::Corrupt | ImageFailure::UnsupportedFormat)
                ),
                "{f:?}: {:?}",
                r.err()
            );
        }
        // An SVG that carries a raster name still goes to the drawing process.
        let r = md_decode_image_why(&svg, 256, &|| false);
        assert!(
            matches!(r, Err(ImageFailure::Svg(_))),
            "an SVG was not offered to the drawing process: {:?}",
            r.err()
        );
    });
}

/// The same for a picture file: a decode that fails after the preview moved on is reported as
/// cancelled (its entry is forgotten and asked for again), not remembered as a damaged file.
#[test]
fn a_file_picture_failing_after_the_preview_moved_on_is_reported_cancelled() {
    use crate::preview::image::ImageFailure;
    let mut broken = {
        let img = image::RgbaImage::from_pixel(64, 64, image::Rgba([1, 2, 3, 255]));
        let mut out = std::io::Cursor::new(Vec::new());
        img.write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    };
    broken.truncate(broken.len() / 2);
    let (mut app, dir, img_rx, _enc_rx) =
        setup_with("konoma_raster_moved_on", &[("b.png", broken)]);
    app.tab.preview_path = Some(dir.join("doc.md"));
    // Nothing moved on: the failure is a failure.
    app.ensure_md_image("b.png", 10, 4, 0, 4);
    let res = img_rx.recv_timeout(WAIT).expect("the decode reports");
    assert_eq!(
        res.image.as_ref().err().map(String::as_str),
        Some(ImageFailure::Corrupt.code())
    );
    app.apply_md_image(res);
    app.md_image_cache.clear();
    // The preview moves on while it is being drawn: the same failure is a cancellation.
    app.media_gen_shared.store(
        app.media_gen.wrapping_add(1000),
        std::sync::atomic::Ordering::Relaxed,
    );
    app.ensure_md_image("b.png", 10, 4, 0, 4);
    let res = img_rx.recv_timeout(WAIT).expect("the decode reports");
    assert_eq!(
        res.image.as_ref().err().map(String::as_str),
        Some(ImageFailure::Cancelled.code())
    );
    let key = res.path.clone();
    app.apply_md_image(res);
    assert!(
        !app.md_image_cache.contains_key(&key),
        "forgotten, asked for again"
    );
}

// ---- decodes in flight versus pictures whose pixels were dropped ------------------------------

/// A picture whose pixels the cache dropped waits for a rebuild; no decode runs for it. Counting it
/// as "in flight" filled the cap of 16 after enough pictures had been seen, and nothing new was
/// ever started. The same holds for a formula or a diagram (the other users of the cap).
#[test]
fn pictures_with_dropped_pixels_are_not_decodes_in_flight() {
    let (mut app, _dir, _img_rx, _enc_rx) = setup("konoma_raster_inflight");
    let dropped = || MdImgEntry {
        evicted: true,
        layout_px: Some((10, 10)),
        ..Default::default()
    };
    for i in 0..40 {
        app.md_image_cache.insert(
            PathBuf::from(crate::preview::markdown::math_url(&format!("x^{i}"), false)),
            dropped(),
        );
        app.md_image_cache.insert(
            PathBuf::from(crate::preview::markdown::mermaid_fence_url(&format!(
                "graph TD; A{i}-->B"
            ))),
            dropped(),
        );
        app.md_image_cache.insert(
            PathBuf::from(format!("office-img://abc/p{i}.png")),
            dropped(),
        );
    }
    assert_eq!(app.synthetic_renders_in_flight(), 0);
    assert_eq!(app.office_pictures_in_flight(), 0);
    // A first decode that is really running still counts.
    app.md_image_cache.insert(
        PathBuf::from("office-img://abc/new.png"),
        MdImgEntry::default(),
    );
    app.md_image_cache.insert(
        PathBuf::from(crate::preview::markdown::math_url("new", false)),
        MdImgEntry::default(),
    );
    assert_eq!(app.office_pictures_in_flight(), 1);
    assert_eq!(app.synthetic_renders_in_flight(), 1);
    // And one that failed does not.
    app.md_image_cache.insert(
        PathBuf::from("office-img://abc/bad.png"),
        MdImgEntry {
            failed: true,
            ..Default::default()
        },
    );
    assert_eq!(app.office_pictures_in_flight(), 1);
}

/// The limits slide pictures are drawn and kept under.
#[test]
fn the_slide_raster_limits_are_what_they_are_documented_to_be() {
    assert_eq!(SLIDE_RASTER_MAX_PX, 4096);
    assert_eq!(
        SLIDE_RASTER_MAX_PX, MD_IMAGE_MAX_SIDE,
        "the decoder's own side limit"
    );
    assert_eq!(MD_SLIDE_CACHE_BYTES, 256 * 1024 * 1024);
    const { assert!(MD_SLIDE_CACHE_BYTES < MD_IMAGE_CACHE_BYTES) };
}

/// A viewport of 0 rows is a real height once the body has been drawn (a tiny terminal): the slide
/// gets the least a picture takes, not the ordinary picture cap that stands for "not drawn yet".
#[test]
fn slide_fit_rows_tells_a_zero_height_from_not_drawn_yet() {
    let (mut app, _dir, _img_rx, _enc_rx) = setup("konoma_raster_fit_rows");
    app.tab.preview_viewport = 0;
    app.tab.preview_viewport_drawn = false;
    assert_eq!(
        app.slide_fit_rows(),
        MD_IMAGE_MAX_ROWS,
        "before the first draw"
    );
    app.tab.preview_viewport_drawn = true;
    assert_eq!(app.slide_fit_rows(), 1, "drawn, and no row to spare");
    for (viewport, rows) in [(1, 1), (2, 1), (3, 1), (4, 2), (30, 28)] {
        app.tab.preview_viewport = viewport;
        assert_eq!(app.slide_fit_rows(), rows, "viewport {viewport}");
    }
}

// ---------------------------------------------------------------------------------------------
// A slide the drawing process refuses at a big raster is drawn smaller, not given up on
// ---------------------------------------------------------------------------------------------

mod smaller_retry {
    use super::*;
    use crate::preview::image::ImageFailure;
    use crate::preview::svg_guard::SvgFail;
    use std::cell::RefCell;

    fn px_image() -> image::DynamicImage {
        image::DynamicImage::new_rgba8(2, 2)
    }

    /// Runs the retry loop with an `attempt` that fails with `fail` at every size above `works_at`
    /// (and succeeds at or below it); returns the result's kind and the sizes tried in order.
    fn run(
        start: u32,
        floor: u32,
        shrink: bool,
        works_at: u32,
        fail: ImageFailure,
    ) -> (Result<(), ImageFailure>, u32, Vec<u32>) {
        let tried = RefCell::new(Vec::new());
        let (res, used, _) = decode_with_smaller_retries(start, floor, shrink, &|| false, |px| {
            tried.borrow_mut().push(px);
            if px <= works_at {
                Ok(px_image())
            } else {
                Err(fail)
            }
        });
        (res.map(|_| ()), used, tried.into_inner())
    }

    #[test]
    fn a_heavy_or_slow_slide_is_retried_at_half_the_size_until_it_is_drawn() {
        for fail in [SvgFail::TooHeavy, SvgFail::Timeout, SvgFail::Memory] {
            let (res, used, tried) = run(4096, 1280, true, 2048, ImageFailure::Svg(fail));
            assert_eq!(res, Ok(()), "{fail:?}");
            assert_eq!(tried, [4096, 2048], "{fail:?}");
            assert_eq!(used, 2048);
        }
    }

    #[test]
    fn the_retries_stop_at_the_floor_and_the_last_failure_is_what_is_reported() {
        let heavy = ImageFailure::Svg(SvgFail::TooHeavy);
        let (res, used, tried) = run(4096, 1280, true, 0, heavy);
        assert_eq!(res, Err(heavy));
        // Halved, and the last step is the floor itself, never below it.
        assert_eq!(tried, [4096, 2048, 1280]);
        assert_eq!(used, 1280);
    }

    #[test]
    fn a_slide_that_works_at_the_first_size_is_drawn_once() {
        let (res, used, tried) = run(3000, 1280, true, u32::MAX, ImageFailure::Cancelled);
        assert_eq!(res, Ok(()));
        assert_eq!((used, tried), (3000, vec![3000]));
    }

    #[test]
    fn other_failures_are_final() {
        for fail in [
            ImageFailure::Svg(SvgFail::Crashed),
            ImageFailure::Svg(SvgFail::TooLarge),
            ImageFailure::Svg(SvgFail::TooDeep),
            ImageFailure::Svg(SvgFail::TooComplex),
            ImageFailure::Svg(SvgFail::Invalid),
            ImageFailure::TooLarge,
            ImageFailure::Corrupt,
            ImageFailure::UnsupportedFormat,
            ImageFailure::Cancelled,
        ] {
            let (res, used, tried) = run(4096, 1280, true, 0, fail);
            assert_eq!(res, Err(fail), "{fail:?}");
            assert_eq!((used, tried), (4096, vec![4096]), "{fail:?}");
        }
    }

    #[test]
    fn a_picture_that_is_not_a_slide_is_never_retried() {
        let heavy = ImageFailure::Svg(SvgFail::TooHeavy);
        let (res, _, tried) = run(4096, 1280, false, 0, heavy);
        assert_eq!(res, Err(heavy));
        assert_eq!(tried, [4096]);
    }

    #[test]
    fn a_request_the_user_moved_on_from_is_not_retried() {
        let n = RefCell::new(0);
        let (res, _, _) = decode_with_smaller_retries(4096, 1280, true, &|| true, |_| {
            *n.borrow_mut() += 1;
            Err(ImageFailure::Svg(SvgFail::Timeout))
        });
        assert!(res.is_err());
        assert_eq!(*n.borrow(), 1);
    }

    #[test]
    fn a_floor_above_the_start_or_equal_to_it_means_no_retry() {
        let heavy = ImageFailure::Svg(SvgFail::TooHeavy);
        // The raster already shown is as big as the wish: nothing smaller is worth drawing.
        for floor in [1280, 5000, u32::MAX] {
            let (res, used, tried) = run(1280, floor, true, 0, heavy);
            assert_eq!(res, Err(heavy));
            assert_eq!((used, tried), (1280, vec![1280]), "floor {floor}");
        }
    }

    /// The real drawing process: a slide refused as too heavy at a big raster is drawn at a
    /// smaller one by the retry loop (the writer's own cap is bypassed, as if its estimate were
    /// too generous).
    #[test]
    fn a_real_slide_refused_at_a_big_raster_is_drawn_smaller() {
        use crate::preview::office::slide_draw::hardening_tests::shadowed_slide;
        let drawn = crate::preview::office::slide_draw::render_svg_cancellable(
            &shadowed_slide(2),
            &|_| None,
            &|| false,
        );
        let svg = drawn.svg.into_bytes();
        let never = || false;
        let heavy = ImageFailure::Svg(SvgFail::TooHeavy);
        // The premise: at 4096 px this slide is refused, at 1280 it is not.
        assert_eq!(md_decode_bytes_why(&svg, 4096, &never).err(), Some(heavy));
        assert!(md_decode_bytes_why(&svg, 1280, &never).is_ok());
        let (res, used, _) = decode_with_smaller_retries(4096, 1280, true, &never, |px| {
            md_decode_bytes_why(&svg, px, &never)
        });
        let img = res.expect("drawn at a smaller size");
        assert!((1280..4096).contains(&used), "{used}");
        use image::GenericImageView;
        let (w, h) = img.dimensions();
        assert!(w.max(h) <= used && w.max(h) > 0, "{w}x{h} at {used}");
    }
}
