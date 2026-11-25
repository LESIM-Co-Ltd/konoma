// SVG preview: rasterize in pure Rust (resvg/usvg/tiny-skia) and return an RGBA DynamicImage.
// The result is put onto the app's image_src, and from there flows through the normal image path
// (prepare_image → worker re-encode → kitty graphics) unchanged. No external C library needed,
// just like chafa/onig (PRD §5).
//
// v1's zoom is "crop the raster" (treated the same as a photo). Infinite vector zoom (re-rasterize
// on every zoom) is future work. So that a small-intrinsic-size SVG doesn't look blocky in the
// terminal, we upscale it to draw at up to the target px on the max side.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use std::io::Read;

use image::DynamicImage;
use resvg::tiny_skia;

use super::svg_guard::{self, SvgFail};
use super::svg_size;
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
pub fn shared_fontdb() -> Arc<usvg::fontdb::Database> {
    if FONTLESS.load(Ordering::Relaxed) {
        return EMPTY_FONTDB
            .get_or_init(|| Arc::new(usvg::fontdb::Database::new()))
            .clone();
    }
    FONTDB
        .get_or_init(|| {
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

static FONTDB: OnceLock<Arc<usvg::fontdb::Database>> = OnceLock::new();
static EMPTY_FONTDB: OnceLock<Arc<usvg::fontdb::Database>> = OnceLock::new();
static FONTLESS: AtomicBool = AtomicBool::new(false);

/// For the drawing process only (`svg_proc`), set before each drawing: this drawing is of an SVG
/// that has no text, so it must not make the process enumerate the system's fonts (about 17 ms of
/// the 30 ms a fresh drawing process otherwise costs). Fonts are only ever read for `<text>`, and
/// the nested SVG images that could hide a `<text>` are excluded by the caller's check. The
/// process draws one thing at a time, and the parent process never sets this.
pub fn set_fontless(fontless: bool) {
    FONTLESS.store(fontless, Ordering::Relaxed);
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
pub fn resolves_sans_serif(db: &usvg::fontdb::Database) -> bool {
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
pub fn install_fallback_sans_serif(db: &mut usvg::fontdb::Database) -> bool {
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

/// Read an SVG file, refusing anything that is not a regular file of at most
/// `svg_guard::MAX_SVG_BYTES` (an svgz is checked again after decompression). `/dev/zero` or a
/// multi-gigabyte file named `.svg` must not be read into memory. Says why it refused.
pub fn read_limited(path: &Path) -> Result<Vec<u8>, SvgFail> {
    let meta = std::fs::metadata(path).map_err(|_| SvgFail::Invalid)?;
    if !meta.is_file() {
        return Err(SvgFail::Invalid);
    }
    if meta.len() > svg_guard::MAX_SVG_BYTES as u64 {
        return Err(SvgFail::TooLarge);
    }
    std::fs::read(path).map_err(|_| SvgFail::Invalid)
}

// ---- who draws what ------------------------------------------------------------------------
//
// There are two ways in, and the choice is made here and nowhere else:
//
//   - `rasterize_trusted`: SVG that konoma generated itself (mermaid diagrams, formulas). Drawn in
//     this process, because that is fast and the input is ours.
//   - `rasterize_untrusted` / `rasterize`: SVG that came from a file or from git (anything a user
//     or a document supplied). Drawn by a supervised child process (`svg_proc`), which can be
//     stopped and cannot take this process down whatever the file does.
//
// Both go through the same `rasterize_guarded`, which is also what the child runs.

/// Whether the first bytes of a file (`head`) can begin an SVG document: XML (after an optional
/// byte-order mark and whitespace, a `<`) or gzip (an `.svgz`). A PNG, JPEG, WebP or any other
/// binary format never does, so a file that such a decoder refused is not worth a drawing process.
pub fn can_begin_svg(head: &[u8]) -> bool {
    if head.starts_with(&[0x1f, 0x8b]) {
        return true;
    }
    let head = head.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(head);
    head.iter()
        .find(|b| !b.is_ascii_whitespace())
        .is_some_and(|&b| b == b'<')
}

/// `can_begin_svg` for the file at `path` (its first 512 bytes; false when it cannot be read).
pub fn file_can_be_svg(path: &Path) -> bool {
    use std::io::Read;
    let mut head = [0u8; 512];
    let Ok(mut f) = std::fs::File::open(path) else {
        return false;
    };
    let n = f.read(&mut head).unwrap_or(0);
    can_begin_svg(&head[..n])
}

/// Why a supervised rasterization produced no picture: the SVG was refused or failed
/// ([`SvgFail`]), or the caller stopped waiting (`cancelled` turned true). A cancellation is not a
/// verdict on the SVG, so it is a separate case that no layer may fold into a failure reason
/// (folding it into `Invalid` once made a request the user scrolled away from look like a
/// damaged picture).
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum RasterError {
    /// The SVG could not be drawn, for this reason.
    Failed(SvgFail),
    /// The caller no longer wanted the picture; the drawing process was stopped.
    Cancelled,
}

impl From<SvgFail> for RasterError {
    fn from(f: SvgFail) -> Self {
        RasterError::Failed(f)
    }
}

impl From<RasterError> for super::image::ImageFailure {
    fn from(e: RasterError) -> Self {
        match e {
            RasterError::Failed(f) => Self::Svg(f),
            RasterError::Cancelled => Self::Cancelled,
        }
    }
}

/// Draws SVG bytes that konoma did not write, with every cheap check already done: the host
/// supplies this (it owns the supervised child process; see `svg_proc` in the `konoma` package).
/// Arguments: the bytes, the directory relative references resolve from (`None`: no file may be
/// read), the max side in px, and the `cancelled` poll.
pub type UntrustedRunner<'a> =
    &'a dyn Fn(&[u8], Option<&Path>, u32, &dyn Fn() -> bool) -> Result<DynamicImage, RasterError>;

/// [`UntrustedRunner`] for a drawing whose file reads are limited: the extra argument is the list
/// of real directories every `<image href>` must resolve into (`None`: no limit).
pub type ScopedRunner<'a> = &'a dyn Fn(
    &[u8],
    Option<&Path>,
    Option<&[PathBuf]>,
    u32,
    &dyn Fn() -> bool,
) -> Result<DynamicImage, RasterError>;

/// [`rasterize_untrusted_via`] for SVG bytes of a plugin the user added: besides `path`'s folder
/// being where relative references start, **every** file the drawing reads (relative or absolute
/// reference, in this document or in an SVG it references) must be inside one of `roots`.
pub fn rasterize_untrusted_within_via(
    data: &[u8],
    path: &Path,
    roots: Option<&[PathBuf]>,
    max_px: u32,
    cancelled: &dyn Fn() -> bool,
    remote_cache_dir: Option<&Path>,
    run: ScopedRunner,
) -> Result<DynamicImage, RasterError> {
    svg_guard::precheck(data)?;
    run(
        data,
        resources_dir_of(path, remote_cache_dir).as_deref(),
        roots,
        max_px,
        cancelled,
    )
}

/// Rasterize the SVG file at `path` with a max side of `max_px` in a supervised child process
/// (`run`). Blocks: call from a worker thread. `cancelled` is polled while waiting; when it turns
/// true the child is stopped and the result is `Err(RasterError::Cancelled)`.
///
/// `remote_cache_dir` is the directory that holds downloaded (remote) images: a file in it is
/// drawn with **no** file readable beside it. It is a parameter, not a registration, so a caller
/// cannot forget to supply it.
pub fn rasterize_via(
    path: &Path,
    max_px: u32,
    cancelled: &dyn Fn() -> bool,
    remote_cache_dir: Option<&Path>,
    run: UntrustedRunner,
) -> Result<DynamicImage, RasterError> {
    let data = read_limited(path)?;
    rasterize_untrusted_via(&data, path, max_px, cancelled, remote_cache_dir, run)
}

/// Rasterize SVG bytes that konoma did not write (a file, a git blob, a cached download), in a
/// supervised child process (`run`). `path` is only where relative references resolve from. Blocks.
///
/// The cheap checks that need no parser (size, nesting depth) run here first, so an obviously
/// hostile file is refused without starting a process.
pub fn rasterize_untrusted_via(
    data: &[u8],
    path: &Path,
    max_px: u32,
    cancelled: &dyn Fn() -> bool,
    remote_cache_dir: Option<&Path>,
    run: UntrustedRunner,
) -> Result<DynamicImage, RasterError> {
    rasterize_untrusted_in(
        data,
        resources_dir_of(path, remote_cache_dir).as_deref(),
        max_px,
        cancelled,
        run,
    )
}

/// Rasterize SVG bytes that are not a file on disk (a picture embedded in a Word / OpenDocument
/// file), in a supervised child process (`run`). The document has no directory its references
/// could be relative to, so the drawing reads **no** file (`<image href="/Users/x/private.png">`
/// paints nothing). Blocks.
pub fn rasterize_embedded_via(
    data: &[u8],
    max_px: u32,
    cancelled: &dyn Fn() -> bool,
    run: UntrustedRunner,
) -> Result<DynamicImage, RasterError> {
    rasterize_untrusted_in(data, None, max_px, cancelled, run)
}

/// Whether `path` is a file in the remote-image cache directory `remote_cache_dir` (i.e.
/// downloaded from the network). `None` (no such directory known) is never a cache file.
fn is_remote_cache_file(path: &Path, remote_cache_dir: Option<&Path>) -> bool {
    let Some(dir) = remote_cache_dir else {
        return false;
    };
    path.parent() == Some(dir)
}

/// The base directory for an on-disk SVG's relative references; `None` (no file may be read) when
/// the file is a cached remote download, so `href="/Users/x/private.png"` in a downloaded SVG
/// cannot paint a local picture.
fn resources_dir_of(path: &Path, remote_cache_dir: Option<&Path>) -> Option<PathBuf> {
    if is_remote_cache_file(path, remote_cache_dir) {
        return None;
    }
    path.parent().map(Path::to_path_buf)
}

fn rasterize_untrusted_in(
    data: &[u8],
    base: Option<&Path>,
    max_px: u32,
    cancelled: &dyn Fn() -> bool,
    run: UntrustedRunner,
) -> Result<DynamicImage, RasterError> {
    svg_guard::precheck(data)?;
    run(data, base, max_px, cancelled)
}

/// How much of the start of a document is read to find its size: the root element's opening tag
/// is in the first few hundred bytes of any real file; this leaves room for a long prolog and a
/// tag with many attributes. Reading no more is what keeps sizing cheap whatever the file holds.
const SIZE_HEAD_BYTES: usize = 64 << 10;

/// The first `SIZE_HEAD_BYTES` of an SVG file (decompressed, for an svgz), after the regular-file
/// and size checks. Never reads the rest of the file.
fn head_of_file(path: &Path) -> Result<Vec<u8>, SvgFail> {
    let meta = std::fs::metadata(path).map_err(|_| SvgFail::Invalid)?;
    if !meta.is_file() {
        return Err(SvgFail::Invalid);
    }
    if meta.len() > svg_guard::MAX_SVG_BYTES as u64 {
        return Err(SvgFail::TooLarge);
    }
    let mut raw = Vec::new();
    std::fs::File::open(path)
        .and_then(|f| f.take(SIZE_HEAD_BYTES as u64 + 2).read_to_end(&mut raw))
        .map_err(|_| SvgFail::Invalid)?;
    head_of_bytes(&raw)
}

/// The first `SIZE_HEAD_BYTES` of `data`, decompressed when it is an svgz.
fn head_of_bytes(data: &[u8]) -> Result<Vec<u8>, SvgFail> {
    if data.starts_with(&[0x1f, 0x8b]) {
        // A truncated or corrupt stream still yields what was decoded before the error.
        let mut out = Vec::new();
        let _ = flate2::read::GzDecoder::new(data)
            .take(SIZE_HEAD_BYTES as u64)
            .read_to_end(&mut out);
        return Ok(out);
    }
    Ok(data[..data.len().min(SIZE_HEAD_BYTES)].to_vec())
}

/// Whether the file is an acceptable SVG document, whether or not it declares a size that can be
/// read without drawing it (used to check that a downloaded file really is an image).
pub fn is_svg(path: &Path) -> bool {
    head_of_file(path).is_ok_and(|head| svg_size::declared_size(&head).is_ok())
}

/// Intrinsic size of the SVG file at `path` in pixels (rounded up), read from the root element
/// only: the start of the file is read and nothing else, so it is cheap enough for the UI thread
/// whatever the file holds (a 200 KB `<text>` used to stall it for 16 s on a Mac with 1,000 fonts installed, milliseconds with one font). Says why there is
/// none: too large, not an SVG, or a size that only drawing reveals (see `svg_size`). A document
/// that is too deeply nested or too heavy still has a size here; the drawing process refuses it.
pub fn intrinsic_size(path: &Path) -> Result<(u32, u32), SvgFail> {
    size_of_head(&head_of_file(path)?)
}

/// Intrinsic size (rounded up) of an in-memory SVG, without rasterizing. Same as `intrinsic_size` but
/// from bytes — used for a synthesized SVG (e.g. a RaTeX math render) whose em units drive layout.
pub fn intrinsic_size_bytes(data: &[u8]) -> Option<(u32, u32)> {
    if data.len() > svg_guard::MAX_SVG_BYTES {
        return None;
    }
    size_of_head(&head_of_bytes(data).ok()?).ok()
}

fn size_of_head(head: &[u8]) -> Result<(u32, u32), SvgFail> {
    match svg_size::declared_size(head)? {
        svg_size::Declared::Size(w, h) => Ok((w.ceil() as u32, h.ceil() as u32)),
        // Valid, but the size is only known by drawing it.
        svg_size::Declared::Unknown => Err(SvgFail::Invalid),
    }
}

/// Rasterize SVG that konoma generated (a mermaid diagram, a formula), in this process. `path` is
/// the base for relative references. Never for a file or a document's picture: see the note above.
pub fn rasterize_trusted(data: &[u8], path: &Path, max_px: u32) -> Option<DynamicImage> {
    rasterize_guarded(data, path.parent(), max_px).ok()
}

/// Bytes a drawing of `pw` x `ph` px claims from the shared decode budget: the pixmap and the
/// straight-alpha copy made from it, 4 bytes a pixel each.
fn reserve_for_render(pw: u32, ph: u32) -> super::image::DecodeReservation {
    let bytes = u64::from(pw) * u64::from(ph) * 8;
    #[cfg(test)]
    LAST_RENDER_RESERVATION.with(|c| c.set(bytes));
    super::image::reserve_decode_memory(bytes)
}

// Test-only: what the last drawing on this thread reserved.
#[cfg(test)]
thread_local! {
    pub static LAST_RENDER_RESERVATION: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Rasterize `data` in this process with every structural limit and the render budget applied
/// (`svg_guard`), with a max side of `max_px`. This is what the child process runs, and what
/// trusted SVG goes through directly.
pub fn rasterize_guarded(
    data: &[u8],
    base: Option<&Path>,
    max_px: u32,
) -> Result<DynamicImage, SvgFail> {
    rasterize_guarded_within(data, base, None, max_px)
}

/// [`rasterize_guarded`] with the files the drawing may read limited to `roots` (real
/// directories; `None` is no limit). See [`svg_guard::options_within`].
pub fn rasterize_guarded_within(
    data: &[u8],
    base: Option<&Path>,
    roots: Option<&[PathBuf]>,
    max_px: u32,
) -> Result<DynamicImage, SvgFail> {
    let roots = roots.map(|r| std::sync::Arc::<[PathBuf]>::from(r.to_vec()));
    svg_guard::load_tree_within(data, base.map(Path::to_path_buf), roots, |tree| {
        let size = tree.size();
        let (w0, h0) = (size.width(), size.height());
        if !(w0 > 0.0 && h0 > 0.0) {
            return Err(SvgFail::Invalid);
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

        if !svg_guard::check_render_budget(&tree, scale) {
            return Err(SvgFail::TooHeavy);
        }
        // Pixmap + the straight-alpha copy below; wait here if other decodes already hold the budget.
        let _mem = reserve_for_render(pw, ph);

        let mut pixmap = tiny_skia::Pixmap::new(pw, ph).ok_or(SvgFail::Invalid)?;
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
        let buf = image::RgbaImage::from_raw(pw, ph, rgba).ok_or(SvgFail::Invalid)?;
        Ok(DynamicImage::ImageRgba8(buf))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use konoma_test_support::unique_tmp;

    // The drawing process lives in the host (`svg_proc`); here the same guarded drawing runs in
    // this process, which is also what the host's own unit tests do when no child binary is set.
    fn in_process(
        data: &[u8],
        base: Option<&Path>,
        max_px: u32,
        _cancelled: &dyn Fn() -> bool,
    ) -> Result<DynamicImage, RasterError> {
        Ok(rasterize_guarded(data, base, max_px)?)
    }

    fn rasterize(
        path: &Path,
        max_px: u32,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<DynamicImage, RasterError> {
        rasterize_via(path, max_px, cancelled, None, &in_process)
    }

    fn rasterize_untrusted(
        data: &[u8],
        path: &Path,
        max_px: u32,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<DynamicImage, RasterError> {
        rasterize_untrusted_via(data, path, max_px, cancelled, None, &in_process)
    }

    fn rasterize_embedded(
        data: &[u8],
        max_px: u32,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<DynamicImage, RasterError> {
        rasterize_embedded_via(data, max_px, cancelled, &in_process)
    }

    const TINY_SVG: &[u8] =
        br##"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10"><rect width="20" height="10" fill="#f00"/></svg>"##;

    #[test]
    fn rasterizes_small_svg_upscaled_and_opaque() {
        // 20x10's max side of 20 is upscaled to max_px=800 → 800x400.
        let img = rasterize_trusted(TINY_SVG, Path::new("t.svg"), 800).expect("should rasterize");
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
        let img = rasterize_trusted(wide, Path::new("t.svg"), 800).expect("should rasterize");
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
        let small = rasterize_trusted(TINY_SVG, Path::new("t.svg"), 400).unwrap();
        assert_eq!(small.width(), 400, "max_px=400 → 最大辺 400");
        let big = rasterize_trusted(TINY_SVG, Path::new("t.svg"), 1200).unwrap();
        assert_eq!(big.width(), 1200, "max_px=1200 → 最大辺 1200");
    }

    #[test]
    fn invalid_svg_returns_none() {
        assert!(rasterize_trusted(b"not an svg at all", Path::new("x.svg"), 800).is_none());
    }

    #[test]
    fn intrinsic_size_reads_declared_size_without_rasterizing() {
        let dir = unique_tmp("konoma_svg_intrinsic_test");
        let _ = std::fs::create_dir_all(&dir);
        let svg = dir.join("badge.svg");
        std::fs::write(&svg, TINY_SVG).unwrap();
        assert_eq!(intrinsic_size(&svg), Ok((20, 10)), "declared 20x10");
        let bad = dir.join("not.svg");
        std::fs::write(&bad, b"not an svg at all").unwrap();
        assert!(intrinsic_size(&bad).is_err(), "非 SVG は Err");
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
            rasterize_trusted(with_text, Path::new("t.svg"), 200).is_some(),
            "フォント DB 準備後はテキスト SVG も描ける"
        );
    }

    // ---- reading an SVG file safely -------------------------------------------------------------

    #[test]
    fn only_regular_files_up_to_the_size_limit_are_read() {
        let dir = unique_tmp("konoma_svg_read_limited");
        std::fs::create_dir_all(&dir).unwrap();
        // Exactly the limit is read; one byte more is refused without reading it.
        let at = dir.join("at.svg");
        std::fs::File::create(&at)
            .unwrap()
            .set_len(svg_guard::MAX_SVG_BYTES as u64)
            .unwrap();
        assert_eq!(
            read_limited(&at).map(|d| d.len()),
            Ok(svg_guard::MAX_SVG_BYTES)
        );
        let over = dir.join("over.svg");
        std::fs::File::create(&over)
            .unwrap()
            .set_len(svg_guard::MAX_SVG_BYTES as u64 + 1)
            .unwrap();
        assert_eq!(read_limited(&over), Err(SvgFail::TooLarge));
        // A directory, a missing file and a device are not regular files.
        std::fs::create_dir_all(dir.join("d.svg")).unwrap();
        assert_eq!(read_limited(&dir.join("d.svg")), Err(SvgFail::Invalid));
        assert_eq!(read_limited(&dir.join("nope.svg")), Err(SvgFail::Invalid));
        let t = std::time::Instant::now();
        assert_eq!(read_limited(Path::new("/dev/zero")), Err(SvgFail::Invalid));
        assert_eq!(head_of_file(Path::new("/dev/zero")), Err(SvgFail::Invalid));
        assert!(intrinsic_size(Path::new("/dev/zero")).is_err());
        assert!(!is_svg(Path::new("/dev/zero")));
        assert_eq!(
            rasterize(Path::new("/dev/zero"), 100, &|| false).err(),
            Some(RasterError::Failed(SvgFail::Invalid))
        );
        assert!(t.elapsed() < std::time::Duration::from_secs(2));
    }

    #[cfg(unix)]
    #[test]
    fn a_named_pipe_named_svg_does_not_block_anything_that_reads_svg_files() {
        let dir = unique_tmp("konoma_svg_fifo");
        std::fs::create_dir_all(&dir).unwrap();
        let fifo = dir.join("pipe.svg");
        let c = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
        // SAFETY: a valid NUL-terminated path.
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        let t = std::time::Instant::now();
        assert_eq!(read_limited(&fifo), Err(SvgFail::Invalid));
        assert_eq!(head_of_file(&fifo), Err(SvgFail::Invalid));
        assert!(intrinsic_size(&fifo).is_err());
        assert!(!is_svg(&fifo));
        assert_eq!(
            rasterize(&fifo, 100, &|| false).err(),
            Some(RasterError::Failed(SvgFail::Invalid))
        );
        assert!(
            t.elapsed() < std::time::Duration::from_secs(2),
            "{:?}",
            t.elapsed()
        );
    }

    #[test]
    fn an_oversized_file_is_refused_by_every_way_in() {
        let dir = unique_tmp("konoma_svg_oversized");
        std::fs::create_dir_all(&dir).unwrap();
        let over = dir.join("over.svg");
        std::fs::File::create(&over)
            .unwrap()
            .set_len(svg_guard::MAX_SVG_BYTES as u64 + 1)
            .unwrap();
        assert_eq!(intrinsic_size(&over), Err(SvgFail::TooLarge));
        assert!(!is_svg(&over));
        assert_eq!(
            rasterize(&over, 100, &|| false).err(),
            Some(RasterError::Failed(SvgFail::TooLarge))
        );
        // The same size, in memory.
        let big = vec![b' '; svg_guard::MAX_SVG_BYTES + 1];
        assert!(intrinsic_size_bytes(&big).is_none());
        assert_eq!(
            rasterize_untrusted(&big, Path::new("x.svg"), 100, &|| false).err(),
            Some(RasterError::Failed(SvgFail::TooLarge))
        );
    }

    #[test]
    fn the_start_of_a_file_is_all_that_is_read_to_size_it() {
        let dir = unique_tmp("konoma_svg_head");
        std::fs::create_dir_all(&dir).unwrap();
        // A real header followed by 30 MB of nothing in particular: sized from the first bytes.
        let mut doc =
            br#"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="30">"#.to_vec();
        doc.extend(std::iter::repeat_n(b'x', 30 << 20));
        let p = dir.join("long.svg");
        std::fs::write(&p, &doc).unwrap();
        let t = std::time::Instant::now();
        assert_eq!(intrinsic_size(&p), Ok((40, 30)));
        assert!(
            t.elapsed() < std::time::Duration::from_millis(200),
            "{:?}",
            t.elapsed()
        );
        assert_eq!(intrinsic_size_bytes(&doc), Some((40, 30)));
        // An opening tag that does not end within the head is not an SVG that can be sized.
        let mut long_tag = br#"<svg xmlns="http://www.w3.org/2000/svg" "#.to_vec();
        long_tag.extend(std::iter::repeat_n(b' ', SIZE_HEAD_BYTES * 2));
        long_tag.extend_from_slice(br#"width="40" height="30"/>"#);
        assert!(intrinsic_size_bytes(&long_tag).is_none());
        // An svgz is sized from its decompressed start, however long the rest is.
        let mut z = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        std::io::Write::write_all(&mut z, &doc).unwrap();
        let z = z.finish().unwrap();
        let pz = dir.join("long.svgz");
        std::fs::write(&pz, &z).unwrap();
        assert_eq!(intrinsic_size(&pz), Ok((40, 30)));
        assert_eq!(intrinsic_size_bytes(&z), Some((40, 30)));
    }

    #[test]
    fn a_drawing_reserves_the_decode_budget_for_its_pixmap_and_its_copy() {
        let doc = br#"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10"/>"#;
        LAST_RENDER_RESERVATION.with(|c| c.set(0));
        let img = rasterize_guarded(doc, None, 400).unwrap();
        assert_eq!((img.width(), img.height()), (400, 200));
        assert_eq!(LAST_RENDER_RESERVATION.with(|c| c.get()), 400 * 200 * 8);
    }

    #[test]
    fn a_drawing_over_the_render_budget_is_refused_as_too_heavy_before_it_is_drawn() {
        // 400 octaves of turbulence over an 800 px canvas: far past the filter work limit.
        let doc = br##"<svg xmlns="http://www.w3.org/2000/svg" width="800" height="800"><filter id="f" x="0" y="0" width="1" height="1"><feTurbulence baseFrequency="0.01" numOctaves="400"/></filter><rect width="800" height="800" filter="url(#f)"/></svg>"##;
        let t = std::time::Instant::now();
        assert_eq!(
            rasterize_guarded(doc, None, 800).err(),
            Some(SvgFail::TooHeavy)
        );
        assert!(
            t.elapsed() < std::time::Duration::from_secs(2),
            "{:?}",
            t.elapsed()
        );
        // Through the trusted door too: the same guard.
        assert!(rasterize_trusted(doc, Path::new("x.svg"), 800).is_none());
    }

    // ---- an SVG may not read files it was not given (the Word preview's review, 2026-10) -------

    /// A 20x20 solid-red PNG written to `dir/name`.
    fn red_png(dir: &Path, name: &str) -> PathBuf {
        let path = dir.join(name);
        image::RgbaImage::from_pixel(20, 20, image::Rgba([255, 0, 0, 255]))
            .save(&path)
            .unwrap();
        path
    }

    /// An SVG that draws the image at `href` over a 20x20 canvas.
    fn svg_with_image(href: &str) -> String {
        format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="20" height="20"><image xlink:href="{href}" width="20" height="20"/></svg>"#
        )
    }

    fn center_alpha(img: &image::DynamicImage) -> u8 {
        let rgba = img.to_rgba8();
        rgba.get_pixel(rgba.width() / 2, rgba.height() / 2)[3]
    }

    fn never() -> bool {
        false
    }

    #[test]
    fn a_scoped_drawing_reads_a_file_only_inside_its_folders() {
        let dir = unique_tmp("konoma_svg_scope");
        // The folders of a scope are real paths (`ReadScope` canonicalizes them).
        std::fs::create_dir_all(&dir).unwrap();
        let base = std::fs::canonicalize(&dir).unwrap_or(dir.to_path_buf());
        let (inside, outside) = (base.join("in"), base.join("out"));
        std::fs::create_dir_all(&inside).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        let a = red_png(&inside, "a.png");
        let b = red_png(&outside, "b.png");
        let draw = |href: &str, roots: &[PathBuf]| {
            let svg = svg_with_image(href);
            center_alpha(
                &rasterize_guarded_within(svg.as_bytes(), Some(&inside), Some(roots), 100).unwrap(),
            )
        };
        let scope = vec![inside.clone()];
        assert_eq!(draw(&a.display().to_string(), &scope), 255);
        assert_eq!(draw(&b.display().to_string(), &scope), 0, "outside");
        // A relative href climbing out, and a link inside that points out, are judged by the
        // real path.
        assert_eq!(draw("../out/b.png", &scope), 0, "dot-dot");
        std::os::unix::fs::symlink(&b, inside.join("l.png")).unwrap();
        assert_eq!(draw("l.png", &scope), 0, "symlink out");
        // A folder whose name only starts like the scope's is outside it.
        let sibling = base.join("in2");
        std::fs::create_dir_all(&sibling).unwrap();
        let c = red_png(&sibling, "c.png");
        assert_eq!(draw(&c.display().to_string(), &scope), 0, "prefix trap");
        // A nested SVG needs no case: usvg gives a referenced SVG no image loading of its own.
    }

    #[test]
    fn an_embedded_svg_reads_no_file() {
        let dir = unique_tmp("konoma_svg_nofile");
        std::fs::create_dir_all(&dir).unwrap();
        let png = red_png(&dir, "secret.png");
        let svg = svg_with_image(&png.display().to_string());
        // A picture of a Word document: no directory at all.
        let img = rasterize_embedded(svg.as_bytes(), 100, &never).unwrap();
        assert_eq!(center_alpha(&img), 0, "the local picture must not be drawn");
        // A caller's placeholder name with no real directory behind it reads nothing either.
        for placeholder in ["image.svg", "office-img://abc/e.svg"] {
            let img =
                rasterize_untrusted(svg.as_bytes(), Path::new(placeholder), 100, &never).unwrap();
            assert_eq!(center_alpha(&img), 0, "{placeholder}");
        }
        // Sizing from bytes reads nothing (it must not even try `/dev/zero`).
        let zero = svg_with_image("/dev/zero");
        assert_eq!(intrinsic_size_bytes(zero.as_bytes()), Some((20, 20)));
        assert_eq!(
            center_alpha(&rasterize_embedded(zero.as_bytes(), 100, &never).unwrap()),
            0
        );
    }

    #[test]
    fn an_svg_file_reads_a_regular_picture_beside_it_but_no_device() {
        let dir = unique_tmp("konoma_svg_file");
        std::fs::create_dir_all(&dir).unwrap();
        red_png(&dir, "red.png");
        let ok = dir.join("ok.svg");
        std::fs::write(&ok, svg_with_image("red.png")).unwrap();
        assert_eq!(center_alpha(&rasterize(&ok, 100, &never).unwrap()), 255);
        // A device, read to the end, would never finish: it is skipped (and quickly).
        let bad = dir.join("bad.svg");
        std::fs::write(&bad, svg_with_image("/dev/zero")).unwrap();
        let t = std::time::Instant::now();
        assert_eq!(center_alpha(&rasterize(&bad, 100, &never).unwrap()), 0);
        assert!(t.elapsed() < std::time::Duration::from_secs(20));
    }

    #[test]
    fn a_fifo_is_never_read_as_an_svg() {
        let dir = unique_tmp("konoma_svg_fifo");
        std::fs::create_dir_all(&dir).unwrap();
        let fifo = dir.join("pipe.svg");
        let c = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
        // SAFETY: `c` is a valid NUL-terminated path.
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0, "mkfifo");
        // Nobody writes: a build that opened the pipe would block here for ever.
        assert!(rasterize(&fifo, 100, &never).is_err());
        assert!(intrinsic_size(&fifo).is_err());
    }
}
