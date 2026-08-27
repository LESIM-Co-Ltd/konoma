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
///
/// **One database, for measuring and for drawing.** konoma's own mermaid renderer sizes every box
/// from the width of its label (`preview::mermaid::text_metrics`) and then hands the SVG to resvg
/// to draw; the two agree only because they read the *same* `Arc`, and that identity is what the
/// renderer's main instrument checks. A second database anywhere would break it silently, so the
/// fallback below is installed here rather than beside any one caller.
pub(crate) fn shared_fontdb() -> Arc<usvg::fontdb::Database> {
    static DB: OnceLock<Arc<usvg::fontdb::Database>> = OnceLock::new();
    DB.get_or_init(|| {
        let mut db = usvg::fontdb::Database::new();
        // System fonts first, and they always win: the fallback below is a no-op whenever the
        // machine resolves `sans-serif` on its own, so an ordinary desktop draws exactly what it
        // drew before this existed.
        db.load_system_fonts();
        install_fallback_sans_serif(&mut db);
        Arc::new(db)
    })
    .clone()
}

/// The face konoma falls back to when the machine resolves `sans-serif` to nothing.
///
/// KaTeX's sans-serif regular. **These bytes are already in the binary**: `Cargo.toml` builds
/// `ratex-svg` with `embed-fonts` for the math previewer, which embeds the whole KaTeX set, so
/// reading one of them here costs no new package and no extra size.
const FALLBACK_SANS_SERIF_FACE: &str = "KaTeX_SansSerif-Regular.ttf";

/// Whether `db` resolves the generic `sans-serif` family to a face that is actually installed.
///
/// Asked the way usvg asks it, which is not the same question as "does this machine have any
/// sans-serif font". `fontdb` turns `Family::SansSerif` into a *family name* — `"Arial"` by
/// default, replaced on Linux by whatever the last `sans-serif` alias in the fontconfig
/// configuration prefers — and then matches that name exactly. A machine can therefore hold a
/// shelf of sans-serif faces and still answer `false` here, and that is the honest answer,
/// because it is the answer usvg will give itself when it draws.
fn resolves_sans_serif(db: &usvg::fontdb::Database) -> bool {
    db.query(&usvg::fontdb::Query {
        families: &[usvg::fontdb::Family::SansSerif],
        weight: usvg::fontdb::Weight::NORMAL,
        stretch: usvg::fontdb::Stretch::Normal,
        style: usvg::fontdb::Style::Normal,
    })
    .is_some()
}

/// Register [`FALLBACK_SANS_SERIF_FACE`] and point `sans-serif` at it — but **only** when nothing
/// else answers to that name. Returns whether the fallback engaged.
///
/// # Why this exists
///
/// konoma's mermaid renderer refuses to draw a diagram at all when `sans-serif` resolves to
/// nothing (`text_metrics::fonts_available`), because usvg does not fail on a missing font: it
/// drops the glyphs and leaves a picture of empty boxes. Refusing is right — a text diagram is
/// better than a lying one — but until v0.28.0 the question never came up, because the mermaid
/// crate konoma used carried fonts of its own. Now that konoma draws diagrams itself, a machine
/// with no resolvable `sans-serif` (a slim container, a minimal image, and — measured — a plain
/// Debian/Ubuntu box whose fontconfig aliases `sans-serif` to a family nobody installed) gets no
/// diagrams where it used to get them. This puts them back.
///
/// # What it deliberately does not do
///
/// It does not touch a database that already resolves the family, so a machine with fonts is
/// byte-for-byte what it was: same faces, same generic mapping, same pixels. It also leaves
/// `serif`, `monospace` and the rest alone — only the family konoma's own diagrams ask for.
///
/// # The limit, stated plainly
///
/// **The KaTeX faces are Latin-only.** On a font-less machine a diagram whose labels are CJK
/// (or Greek, Cyrillic, Arabic, …) still cannot be drawn: those labels come out blank, and only
/// the Latin ones read. That is a real limit, and it is still better than drawing nothing at all —
/// the diagram's shape, arrows and structure survive. Bundling a font that covers CJK would add
/// megabytes to every download for a case the user can fix by installing a font, which is the
/// trade PRD §5 (distribution ease) already decided in the other direction.
fn install_fallback_sans_serif(db: &mut usvg::fontdb::Database) -> bool {
    if resolves_sans_serif(db) {
        return false;
    }
    // Where the bytes come from, because it is not the same in every build: `ratex-katex-fonts`
    // uses `rust-embed` without its `debug-embed` feature, which means the bytes are compiled into
    // the binary only when `debug_assertions` is off. A release build — every shipped binary, and
    // anything `cargo install` produces — therefore carries the face and needs nothing from disk
    // (verified by finding the TTF's bytes inside a release binary and not inside a debug one). A
    // debug build instead reads the file from the registry checkout it was compiled against, which
    // is present wherever the code was built, so tests and development are unaffected. `None` is
    // then the case of a debug binary carried somewhere that path does not exist — the renderer
    // goes back to refusing, which is the behaviour that predates this function, not a new failure.
    let Some(bytes) = ratex_katex_fonts::ttf_bytes(FALLBACK_SANS_SERIF_FACE) else {
        return false;
    };
    let first_new_face = db.len();
    db.load_font_data(bytes.into_owned());
    // The family name comes from the face's own name table rather than a string written here, so
    // the mapping cannot drift from whatever the embedded file actually calls itself.
    let Some(family) = db
        .faces()
        .skip(first_new_face)
        .find_map(|face| face.families.first().map(|(name, _)| name.clone()))
    else {
        return false;
    };
    db.set_sans_serif_family(family);
    true
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

    /// Draws `svg` against `db` through the same `resvg::render` call the previewer makes, and
    /// returns the raw pixels.
    ///
    /// The fallback tests need a fact that markup cannot fake: whether glyphs really landed on the
    /// page. **Counting *painted* pixels would not show it** — a mermaid label sits inside a filled
    /// node box, so every pixel it touches is already opaque and the alpha count is identical with
    /// and without a font (measured: 8,949 either way). What separates them is *colour*, so the
    /// tests compare two rasters of the same SVG instead.
    fn raster(svg: &str, db: Arc<usvg::fontdb::Database>) -> Vec<u8> {
        let opt = usvg::Options {
            fontdb: db,
            ..usvg::Options::default()
        };
        let tree = usvg::Tree::from_str(svg, &opt).expect("the renderer's own SVG must parse");
        let size = tree.size();
        let w = (size.width().ceil() as u32).max(1);
        let h = (size.height().ceil() as u32).max(1);
        let mut pixmap = tiny_skia::Pixmap::new(w, h).expect("pixmap should allocate");
        resvg::render(
            &tree,
            tiny_skia::Transform::identity(),
            &mut pixmap.as_mut(),
        );
        pixmap.take()
    }

    /// The one path neither this machine nor CI can reach on its own, so it is built by hand:
    /// a database that never saw a system font, which is what a slim container has.
    #[test]
    fn the_embedded_face_draws_a_diagram_on_a_machine_with_no_fonts() {
        use crate::preview::mermaid::render;
        use crate::preview::mermaid::text_metrics::{TextMetrics, FONT_SIZE};

        // Without the fallback: exactly the state the Linux CI runner was in. The gate the
        // renderer asks before it builds anything says "do not draw", and 45 tests said so too.
        let bare = usvg::fontdb::Database::new();
        assert_eq!(bare.len(), 0, "the stand-in machine has no fonts at all");
        assert!(!resolves_sans_serif(&bare));
        assert!(
            TextMetrics::resolve(Arc::new(bare)).is_err(),
            "with no font the renderer's gate must refuse — this is the regression"
        );

        // With it: the same empty machine, through the very function `shared_fontdb` calls.
        let mut fallback = usvg::fontdb::Database::new();
        assert!(
            install_fallback_sans_serif(&mut fallback),
            "the embedded face must register when nothing else answers to sans-serif"
        );
        assert!(
            resolves_sans_serif(&fallback),
            "and sans-serif now resolves"
        );
        let fallback = Arc::new(fallback);

        // Measurement — the input every box size is derived from.
        //
        // "It resolved" is not enough to check here, and the difference is not academic: swapping
        // the embedded file for `KaTeX_Size4-Regular.ttf` — a face of brackets, with no letters at
        // all — resolves, measures, and even moves pixels. So what is asserted is that the face
        // has the glyphs, and that the width it reports is the width of five *letters* (measured:
        // 29.4px) rather than of five stand-ins. `text_metrics` falls back to the `.notdef` glyph
        // for a character nothing in the database can draw, which for this face is a quarter of an
        // em — 17.5px for these five — so the lower bound has to sit above that to say anything.
        let metrics = TextMetrics::resolve(fallback.clone())
            .expect("the gate must pass once the embedded face is registered");
        assert!(
            !metrics.has_missing_glyph("Start"),
            "the embedded face must carry Latin glyphs, not merely answer to the family name"
        );
        let width = metrics.measure("Start", FONT_SIZE);
        assert!(
            (20.0..60.0).contains(&width),
            "five letters at {FONT_SIZE}px should measure like letters, not like stand-ins: {width}px"
        );

        // Drawing — through the renderer's own entry point, so what is checked is what a caller
        // gets rather than a hand-written `<text>`.
        let svg = render::render("flowchart LR\n  A[Start] --> B[Stop]\n", "dark")
            .expect("the diagram must lay out");
        let unlabelled = raster(&svg, Arc::new(usvg::fontdb::Database::new()));
        let labelled = raster(&svg, fallback);
        assert!(
            unlabelled.iter().any(|&b| b != 0),
            "the boxes and arrows are drawn either way — otherwise the comparison means nothing"
        );
        let (labelled_px, _) = labelled.as_chunks::<4>();
        let (unlabelled_px, _) = unlabelled.as_chunks::<4>();
        let changed = labelled_px
            .iter()
            .zip(unlabelled_px)
            .filter(|(a, b)| a != b)
            .count();
        // Two five-letter labels at 14px cover a few hundred pixels; the bound only has to be far
        // enough above zero that a stray antialiasing pixel could not carry the test.
        assert!(
            changed > 100,
            "the labels must put glyphs on the page: only {changed} pixels differ between the \
             diagram drawn with the embedded face and the same diagram drawn with no font at all"
        );
    }

    /// The other half of the promise: a machine that has a sans-serif of its own is untouched.
    ///
    /// The "has one" case is staged rather than borrowed from the host, so the test asserts the
    /// same thing on a developer's Mac, on CI, and inside a font-less container.
    #[test]
    fn the_fallback_stays_out_of_the_way_when_sans_serif_already_resolves() {
        let mut db = usvg::fontdb::Database::new();
        db.load_font_data(
            ratex_katex_fonts::ttf_bytes("KaTeX_Main-Regular.ttf")
                .expect("the embedded set carries the main face")
                .into_owned(),
        );
        let system_family = db
            .faces()
            .next()
            .and_then(|f| f.families.first().map(|(name, _)| name.clone()))
            .expect("the staged face registers a family");
        // Stand in for what fontconfig does on a real machine: point the generic at a real face.
        db.set_sans_serif_family(system_family.clone());
        let faces_before = db.len();

        assert!(!install_fallback_sans_serif(&mut db), "must not engage");
        assert_eq!(db.len(), faces_before, "no face may be registered");
        assert_eq!(
            db.family_name(&usvg::fontdb::Family::SansSerif),
            system_family,
            "the machine's own sans-serif mapping must be left exactly as it was"
        );
    }

    /// And the same promise for *this* machine, whichever kind it is.
    ///
    /// Both branches assert something, so the test cannot go quiet by running somewhere it has
    /// nothing to say: on a machine with fonts it pins that the shared DB is the system DB and
    /// nothing more (which is why no drawn output can have moved), and on one without, that the
    /// single embedded face is there and the generic resolves.
    #[test]
    fn the_shared_db_adds_the_fallback_only_where_the_system_needs_it() {
        let mut system = usvg::fontdb::Database::new();
        system.load_system_fonts();
        let shared = shared_fontdb();
        if resolves_sans_serif(&system) {
            assert_eq!(
                shared.len(),
                system.len(),
                "a machine that resolves sans-serif gets no extra face"
            );
            assert_eq!(
                shared.family_name(&usvg::fontdb::Family::SansSerif),
                system.family_name(&usvg::fontdb::Family::SansSerif),
                "and keeps its own generic mapping"
            );
        } else {
            assert_eq!(
                shared.len(),
                system.len() + 1,
                "a machine that does not gets exactly one embedded face"
            );
            assert!(
                resolves_sans_serif(&shared),
                "and sans-serif resolves after it"
            );
        }
    }

    /// Asserts a fact about the **host**, which is why it is ignored by default: a font-less
    /// container is a perfectly legitimate place to run konoma's tests, and there it would be a
    /// false alarm.
    ///
    /// CI's Linux job runs it by name, right after installing fonts, and that is the whole point.
    /// The fallback means a missing font package no longer turns anything red — so without this,
    /// a wrong package list would silently retire the "system fonts present" path from Linux
    /// testing and nobody would hear about it. Measured while writing this: the obvious choice,
    /// `fonts-dejavu-core`, is one of those wrong lists (see the workflow's comment).
    #[test]
    #[ignore = "asserts the host has fonts of its own; CI's Linux job runs it by name"]
    fn the_system_resolves_sans_serif_without_the_fallback() {
        let mut system = usvg::fontdb::Database::new();
        system.load_system_fonts();
        assert!(
            resolves_sans_serif(&system),
            "this host resolves the generic sans-serif family to nothing ({} faces loaded, \
             generic points at {:?}), so konoma's embedded fallback would be doing the drawing",
            system.len(),
            system.family_name(&usvg::fontdb::Family::SansSerif),
        );
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
