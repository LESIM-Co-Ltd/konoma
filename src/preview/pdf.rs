// PDF preview: rasterizes the given page (1-based) and returns a DynamicImage. Like video/svg, the
// return value is put on the app's image_src, and from there flows straight through the normal image
// path (prepare_image → worker re-encode → kitty graphics). Pages are rasterized one at a time, on
// demand (prioritizing display speed, no read-ahead).
//
// **Renderer priority order**:
//   1. `hayro` (pure Rust, no external process) — the default and first choice, on every platform.
//      Measured: 20-30x faster than the poppler CLI it replaced for light documents, and 1.8-12x
//      faster even for photo-heavy ones (15 consecutive pages rendered = 143ms, versus 60-70ms for
//      a single page), and it renders Japanese (embedded fonts) correctly too. Encrypted PDFs,
//      corrupt PDFs, and other parse/render failures return `None` and **degrade** to step 2
//      (principle #3, "unsupported degrades safely").
//   2. **macOS only**: `qlmanage` (Quick Look) → `sips`. Both are part of the OS, so this fallback
//      costs the user no install at all. Both can only produce the *first* page, so it is gated on
//      `page == 1`. On every other platform there is no external chain: `render_page_external`
//      returns `None` immediately without spawning anything.
//
// **Why poppler was dropped (2026-08).** It used to sit between the two steps above. Measured over
// the 1,628 real PDFs present on a development machine, run through exactly this dispatch: `hayro`
// rendered 1,604 of them (98.53%) with zero panics, and 24 (1.47%) fell through to the external
// chain. **poppler rescued none of those 24** — 18 were genuinely blank (poppler produced a
// perfectly uniform image for them as well) and 6 were not PDFs at all (a gzip magic number, or a
// fragment starting partway into a document). So the only thing keeping poppler bought was having
// to tell users "install poppler to view PDFs" when their stated requirement was to install nothing
// but `git` — in exchange for zero recovered documents. The two macOS tools stay precisely because
// they are already on the machine.
//
// **Deliberate consequence of that removal**: `qlmanage`/`sips` are first-page-only, so when `hayro`
// fails on page 2 or later there is now no fallback at all and the preview degrades to
// `[can not preview]`. poppler could rasterize an arbitrary page, so this is a real (if, per the
// measurement above, unexercised) reduction. Linux has behaved exactly this way all along, since
// neither macOS tool exists there.
//
// `allow_external` (= `[external] pdf`) gates step 2 only: on macOS it decides whether qlmanage/sips
// may launch, and on every other platform it is effectively a no-op because there is nothing left to
// launch. `hayro` never spawns a process, so PDF preview keeps working with the flag off (PRD §5,
// ease of distribution). If everything fails, this returns None and the caller degrades to a safe
// fallback (a hint display).
// hayro's per-page render takes only single-digit milliseconds, so calling it synchronously is fine,
// but the caller (app.rs) calls it through the existing media-worker thread, so even blocking on an
// external tool never blocks the UI.
//
// Page count ([`page_count`]) never launches an external process at all: it reads the page tree
// directly with `hayro-syntax` (pure Rust). It used to call poppler's `pdfinfo` as a child process,
// but even after removing that command, getting the page count itself became possible. Now that
// hayro also handles the actual page rendering, "knowing the page count ⟹ can render any page" holds
// almost universally (there's no "first-page-only" constraint like qlmanage/sips have), so the caller
// (app.rs's `enter_preview`/`reload_media_if_changed`) no longer gates page navigation on any
// external tool being installed. The utility that used to live here checking only "is poppler on
// PATH" (`arbitrary_page_renderer_available`) has been deleted since it has no more callers (rather
// than tagging it `#[allow(dead_code)]` needlessly — delete it once it's unused).

use std::path::Path;
use std::sync::Arc;

use hayro::hayro_interpret::font::{FontData, FontQuery};
use hayro::hayro_interpret::hayro_cmap::{CharacterCollection, CidFamily};
use hayro::hayro_interpret::InterpreterSettings;
use hayro::vello_cpu::color::palette::css::TRANSPARENT;
use hayro::vello_cpu::Pixmap;
use image::DynamicImage;

/// Maximum side (px) of the rasterized page. The image path shrinks it further to terminal cells,
/// so this only needs to be large enough that text stays crisp.
const PAGE_MAX_PX: u32 = 1600;

/// Rasterize page `page` (1-based) of the PDF at `path` and return it as an image. `hayro` (pure
/// Rust, in-process) is tried first; only when it returns `None` (encrypted/corrupt/unsupported PDF,
/// or a rendering panic caught by [`crate::preview::markdown::catch_silent`]) does this fall back to
/// [`render_page_external`] — macOS's bundled `qlmanage`/`sips`, and only when `allow_external` is
/// true (`[external] pdf`). Returns `None` if nothing could render the page (the caller degrades to
/// a safe fallback with a hint); off macOS, and for any page past the first, that means `hayro`
/// declining is final (see the module doc comment on why poppler is no longer in this chain).
pub fn render_page(path: &Path, page: u32, allow_external: bool) -> Option<DynamicImage> {
    // A nonexistent path launches nothing and returns None immediately (hayro would also naturally
    // end up None from failing to read the file, but this explicit early return specifically avoids launching qlmanage's QuickLook).
    if !path.is_file() {
        return None;
    }
    let page = page.max(1);
    if let Some(img) = render_page_native(path, page) {
        return Some(img);
    }
    if !allow_external {
        // `[external] pdf = false`: never launch qlmanage/sips. (Off macOS this branch is
        // indistinguishable from the one below, which has nothing left to launch anyway.)
        return None;
    }
    render_page_external(path, page)
}

/// Rasterize with `hayro` — pure Rust, no child process at all. `None` means "hayro couldn't do it"
/// (encrypted, corrupt, page out of range, or a caught panic), never a crash: [`render_page`] then
/// tries the macOS fallback tools (if allowed, and if this is page 1).
fn render_page_native(path: &Path, page: u32) -> Option<DynamicImage> {
    // hayro is pre-1.0 and (like resvg/mermaid-rs-renderer) gets the same defense-in-depth as
    // `render_mermaid_safe`/`decode_gif`: a hostile/malformed PDF must degrade to the fallback
    // chain, not take down the whole preview. `Pdf::new` itself already returned zero panics across
    // a 3000-iteration byte-mutation fuzz of the bundled sample (see `page_count`'s doc), but this
    // wraps the *rendering* step too, which walks far more of the file's content streams.
    crate::preview::markdown::catch_silent(|| render_page_native_inner(path, page)).flatten()
}

fn render_page_native_inner(path: &Path, page: u32) -> Option<DynamicImage> {
    let bytes = std::fs::read(path).ok()?;
    // Err = encrypted (no password supplied — konoma never prompts for one) or malformed. Either way
    // this just becomes `None` here; render_page falls back to the external tool chain.
    let pdf = hayro::hayro_syntax::Pdf::new(bytes).ok()?;
    let pages: Vec<_> = pdf.pages().iter().collect();
    let idx = usize::try_from(page).ok()?.checked_sub(1)?;
    let page_ref = *pages.get(idx)?;
    let (w, h) = page_ref.render_dimensions();
    if !(w.is_finite() && h.is_finite() && w > 0.0 && h > 0.0) {
        return None;
    }
    let scale = PAGE_MAX_PX as f32 / w.max(h);
    let settings = pdf_interpreter_settings();
    let render_settings = hayro::RenderSettings {
        x_scale: scale,
        y_scale: scale,
        // Transparent, same treatment as the mermaid/math SVGs (`preview::svg`/`preview::math`):
        // only the page's own painted content stays opaque, so unpainted margins let the terminal
        // background show through instead of forcing a white background regardless of theme.
        bg_color: TRANSPARENT,
        ..Default::default()
    };
    // A fresh cache per call. Measured: reusing a `RenderCache` across page turns (or even across
    // two renders of the *same* page) made no measurable difference for realistic PDFs — a 15-page
    // walk was 138.92ms shared-cache vs 151.47ms fresh-cache-per-page (noise-level, ~8%), and a
    // single heavy page was 92.18ms warm vs 97.77ms cold (~6%). The expensive part (embedded image
    // decode / path rasterization) isn't what the cache holds (font/glyph-outline data), so
    // persisting `Pdf`+`RenderCache` across keypresses — which would require a self-referential
    // struct, since `RenderCache<'a>` borrows from the `Pdf` it was built against — isn't worth the
    // `unsafe`/extra-dependency complexity for a gain that doesn't show up in practice. See the
    // report for the full numbers; re-measure if this ever shows up as a bottleneck.
    let cache = hayro::RenderCache::new();
    let pixmap = hayro::render(page_ref, &cache, &settings, &render_settings);
    pixmap_to_dynamic_image(pixmap)
}

/// `hayro`'s `Pixmap` is premultiplied-alpha RGBA8; `image::DynamicImage` expects straight alpha
/// (same reason `preview::svg::rasterize_bytes` demultiplies tiny-skia's output).
///
/// **Also the point where a technically-successful-but-empty render is caught.** `hayro::render`
/// returns a `Pixmap`, never a `Result` — there is no separate "I actually failed" signal from
/// this call. A page whose content stream draws nothing (a genuinely blank page, or — measured
/// separately — a truncated/corrupted stream that hayro didn't notice was broken) produces a
/// `Pixmap` that is valid in every structural sense but **fully transparent everywhere**, since
/// the render background is `TRANSPARENT` (see the caller) and nothing painted over it. Treating
/// that the same as every other hayro failure (`None`, not `Some(blank image)`) buys two things:
/// on macOS `render_page` can still fall through to Quick Look for page 1, and everywhere else the
/// user gets an honest `[can not preview]` instead of an empty rectangle that looks like a
/// successful render of a document that isn't actually empty.
///
/// Historical note: this used to be justified by a damage-location sweep showing poppler could
/// recover content a damaged stream hid from hayro. The larger corpus measurement that removed
/// poppler from the chain (module doc comment) did not reproduce that: of the 24 documents out of
/// 1,628 that reached the fallback, 18 hit exactly this all-transparent signature and poppler
/// rendered them as a perfectly uniform image too — i.e. they were genuinely blank.
fn pixmap_to_dynamic_image(pixmap: Pixmap) -> Option<DynamicImage> {
    let (w, h) = (u32::from(pixmap.width()), u32::from(pixmap.height()));
    if w == 0 || h == 0 {
        return None;
    }
    let straight = pixmap.take_unpremultiplied();
    let mut rgba = Vec::with_capacity(straight.len() * 4);
    for px in straight {
        rgba.extend_from_slice(&px.to_u8_array());
    }
    // Checked on the raw bytes (before `RgbaImage::from_raw` below), so a normal, ink-bearing page
    // pays nothing extra: `.all()` short-circuits at the first non-transparent pixel, which real
    // content reaches almost immediately. Only a genuinely all-transparent buffer forces the full
    // scan — and that is exactly the case that is about to pay for an external-tool process spawn
    // anyway, so one extra linear pass over already-resident memory is noise beside it.
    if rgba.chunks_exact(4).all(|px| px[3] == 0) {
        return None;
    }
    let buf = image::RgbaImage::from_raw(w, h, rgba)?;
    Some(DynamicImage::ImageRgba8(buf))
}

/// `InterpreterSettings` with a `font_resolver` that rescues non-embedded CJK CID fonts (e.g. a
/// PDF referencing the predefined `HeiseiMin-W3`/Adobe-Japan1 font by name, never embedding it) by
/// substituting a real system CJK font. Without this, `hayro`'s own default resolver substitutes one
/// of the 14 Latin standard fonts for *any* non-embedded font — which has no CJK glyphs at all, so
/// the text renders with every glyph silently missing (a blank page, confirmed by measurement).
/// Falls through to `hayro`'s own standard-font substitution (`pick_standard_font`) when no CJK
/// candidate is installed, or for ordinary non-CJK fallback queries — this is exactly what already
/// renders plain non-embedded Helvetica/Times-Roman text correctly (verified).
fn pdf_interpreter_settings() -> InterpreterSettings {
    InterpreterSettings {
        font_resolver: Arc::new(|query| match query {
            FontQuery::Standard(s) => Some(s.get_font_data()),
            FontQuery::Fallback(f) => cjk_fallback_font(f.character_collection.as_ref())
                .or_else(|| Some(f.pick_standard_font().get_font_data())),
        }),
        ..InterpreterSettings::default()
    }
}

/// Best-effort system CJK font for a non-embedded fallback query. Reuses konoma's shared
/// `usvg`/`resvg` font database (`preview::svg::shared_fontdb`) instead of enumerating system fonts
/// a second time (that enumeration alone costs hundreds of ms on macOS). `None` when nothing in
/// [`cjk_family_candidates`] is installed — the caller then falls back to hayro's own Latin
/// standard-font substitution, so the glyphs are simply missing rather than the preview crashing.
fn cjk_fallback_font(collection: Option<&CharacterCollection>) -> Option<(FontData, u32)> {
    let db = crate::preview::svg::shared_fontdb();
    let names = cjk_family_candidates(collection.map(|c| &c.family));
    let families: Vec<resvg::usvg::fontdb::Family<'_>> = names
        .iter()
        .map(|n| resvg::usvg::fontdb::Family::Name(n))
        .collect();
    let query = resvg::usvg::fontdb::Query {
        families: &families,
        ..Default::default()
    };
    let id = db.query(&query)?;
    let (_source, face_index) = db.face_source(id)?;
    let bytes = db.with_face_data(id, |data, _face_index| data.to_vec())?;
    let font_data: FontData = Arc::new(bytes);
    Some((font_data, face_index))
}

/// Priority-ordered system font family names to try for a non-embedded CJK CID font, based on its
/// Adobe character collection (`hayro_cmap::CidFamily`). Ordered "the font for this exact script
/// first, then every other CJK script as a backstop" — Han unification means a Chinese/Korean font
/// still covers a fair amount of Japanese text and vice versa, and *some* glyphs beats the blank page
/// this replaces. `fontdb::Database::query` matches by exact family-name string (no glyph-coverage
/// probing), so this only needs real font family names — no risk of it ever matching an unrelated
/// Latin font by accident. Tried unconditionally (even when `collection` is `None`, e.g. a plain
/// non-CID fallback query) since a name-exact match against this CJK-only list can never misfire.
fn cjk_family_candidates(family: Option<&CidFamily>) -> Vec<&'static str> {
    // macOS ships Hiragino/PingFang/Apple SD Gothic Neo unconditionally; the rest cover the common
    // Linux CJK font packages (fonts-noto-cjk, fonts-wqy-*, fonts-droid-fallback, ipafont, otf-takao).
    const JP: &[&str] = &[
        "Hiragino Sans",
        "Hiragino Kaku Gothic ProN",
        "Hiragino Kaku Gothic Pro",
        "Hiragino Mincho ProN",
        "Noto Sans CJK JP",
        "Noto Serif CJK JP",
        "Noto Sans JP",
        "Source Han Sans JP",
        "Source Han Sans",
        "IPAGothic",
        "IPAPGothic",
        "TakaoGothic",
        "TakaoPGothic",
        "VL Gothic",
        "VL PGothic",
    ];
    const SC: &[&str] = &[
        "PingFang SC",
        "Hiragino Sans GB",
        "Noto Sans CJK SC",
        "Noto Serif CJK SC",
        "Noto Sans SC",
        "Source Han Sans SC",
        "Source Han Sans CN",
        "WenQuanYi Zen Hei",
        "WenQuanYi Micro Hei",
        "Droid Sans Fallback",
        "Microsoft YaHei",
        "SimHei",
        "SimSun",
    ];
    const TC: &[&str] = &[
        "PingFang TC",
        "PingFang HK",
        "Noto Sans CJK TC",
        "Noto Serif CJK TC",
        "Noto Sans TC",
        "Source Han Sans TC",
        "Source Han Sans TW",
        "Microsoft JhengHei",
        "AR PL UMing TW",
        "AR PL UKai TW",
    ];
    const KR: &[&str] = &[
        "Apple SD Gothic Neo",
        "Noto Sans CJK KR",
        "Noto Serif CJK KR",
        "Noto Sans KR",
        "Source Han Sans KR",
        "Malgun Gothic",
        "NanumGothic",
        "NanumBarunGothic",
        "Baekmuk Dotum",
        "UnDotum",
    ];
    let primary: &[&str] = match family {
        Some(CidFamily::AdobeJapan1) => JP,
        Some(CidFamily::AdobeGB1) => SC,
        Some(CidFamily::AdobeCNS1) => TC,
        Some(CidFamily::AdobeKorea1) => KR,
        // AdobeIdentity/Custom/unknown: no script hint available, so just try every script below.
        _ => &[],
    };
    primary
        .iter()
        .chain(JP)
        .chain(SC)
        .chain(TC)
        .chain(KR)
        .copied()
        .collect()
}

/// The external-tool fallback, on the one platform where it costs the user nothing to have:
/// macOS's bundled `qlmanage` (Quick Look) and `sips`. Used only when `hayro` declined *and* the
/// caller allows it (`[external] pdf`).
///
/// Both tools can only produce the **first** page, so anything past page 1 returns immediately
/// without spawning a process — that early return is what used to be an inline `page == 1` guard on
/// the last two rungs of a longer ladder whose upper rungs (poppler) could render an arbitrary
/// page. See the module doc comment for why those upper rungs are gone and what it costs.
#[cfg(target_os = "macos")]
fn render_page_external(path: &Path, page: u32) -> Option<DynamicImage> {
    if page != 1 {
        return None;
    }
    let out = macos::temp_png_path();
    let ok = macos::run_qlmanage(path, &out) || macos::run_sips(path, &out);
    let img = if ok {
        image::ImageReader::open(&out)
            .ok()
            .and_then(|r| r.with_guessed_format().ok())
            .and_then(|r| r.decode().ok())
    } else {
        None
    };
    let _ = std::fs::remove_file(&out); // delete the temp file right away (regardless of success/failure)
    img
}

/// Off macOS there is no external fallback at all, so this spawns nothing and returns `None`
/// immediately — `hayro` declining is the final answer.
///
/// This is deliberately a hard compile-time absence rather than "try `qlmanage`, let the spawn fail
/// with ENOENT": launching a program we know for a fact is not installed is pure waste (a process
/// spawn attempt, a `PATH` walk, and — with the timeout poll loop below — a needlessly reachable
/// code path), and it also means the Linux build compiles none of the child-process machinery at
/// all. The unused `page`/`path` are named with a leading underscore because there is genuinely
/// nothing left to do with them here.
#[cfg(not(target_os = "macos"))]
fn render_page_external(_path: &Path, _page: u32) -> Option<DynamicImage> {
    None
}

/// Everything needed to drive macOS's bundled rasterizers, compiled only on macOS. Grouped into a
/// module so the whole set is gated by a single `cfg` rather than a dozen attributes, and so it is
/// obvious at a glance that no other platform builds any child-process code in this file.
#[cfg(target_os = "macos")]
mod macos {
    use super::PAGE_MAX_PX;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};
    use std::sync::atomic::{AtomicU64, Ordering};

    /// macOS `qlmanage -t` (Quick Look thumbnail). It writes `<outdir>/<filename>.png`, so we render into a
    /// private temp dir and copy the produced PNG to `out`. Always present on macOS = the reliable fallback.
    pub(super) fn run_qlmanage(path: &Path, out: &Path) -> bool {
        let dir = temp_dir_path();
        if std::fs::create_dir_all(&dir).is_err() {
            return false;
        }
        {
            // Defense-in-depth on top of `private_temp_dir`'s already-`0700` parent (an ancestor
            // directory's search permission is what actually gates access — see `private_temp_dir`'s
            // doc comment — but restricting this nested dir too costs nothing).
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
        }
        let mut cmd = Command::new("qlmanage");
        cmd.arg("-t")
            .arg("-s")
            .arg(PAGE_MAX_PX.to_string())
            .arg("-o")
            .arg(&dir)
            .arg(path)
            .stdin(Stdio::null()) // never let a helper read the terminal out from under crossterm
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let status = spawn_and_wait_with_timeout(&mut cmd, TOOL_TIMEOUT);
        let mut ok = false;
        if matches!(status, Some(s) if s.success()) {
            if let Some(name) = path.file_name() {
                let produced = dir.join(format!("{}.png", name.to_string_lossy()));
                ok = out_is_nonempty(&produced) && std::fs::copy(&produced, out).is_ok();
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
        ok
    }

    /// macOS `sips` (renders the first page of a PDF to PNG). Writes directly to `out`. Last-resort fallback.
    pub(super) fn run_sips(path: &Path, out: &Path) -> bool {
        let mut cmd = Command::new("sips");
        cmd.arg("-s")
            .arg("format")
            .arg("png")
            .arg("--resampleHeightWidthMax")
            .arg(PAGE_MAX_PX.to_string())
            .arg(path)
            .arg("--out")
            .arg(out)
            .stdin(Stdio::null()) // never let a helper read the terminal out from under crossterm
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        matches!(spawn_and_wait_with_timeout(&mut cmd, TOOL_TIMEOUT), Some(s) if s.success())
            && out_is_nonempty(out)
    }

    /// How long an external rendering tool (`qlmanage`/`sips`) is allowed to run before being
    /// killed and treated as a failure. Generous on purpose — a large/complex page can legitimately
    /// take a while — this exists only to turn "never returns" into "eventually gives up," not to
    /// police normal runtimes. Without this, a hung tool blocked `Command::status()` indefinitely:
    /// since these run on the media worker thread (see the module doc comment above), that leaked
    /// the worker thread *and* the child process forever, and left the busy-indicator spinner stuck
    /// spinning. (`hayro`, the pure-Rust primary renderer, is unaffected — it never spawns a process
    /// at all; this only guards the fallback below it.)
    const TOOL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

    /// How often [`spawn_and_wait_with_timeout`]'s poll loop re-checks the child. Small enough to
    /// notice the deadline promptly without busy-polling.
    const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(20);

    /// Spawns `cmd` and waits for it to exit, **polling** rather than blocking indefinitely
    /// (`Command::status()`'s behavior) — past `timeout` the child is killed and this returns `None`,
    /// exactly like "the tool failed" to every caller here (principle #3: this degrades to `[can not
    /// preview]`, it never hangs the app). `timeout` is a parameter (not a call to the `TOOL_TIMEOUT`
    /// constant baked in here) purely for testability: production call sites above always pass
    /// `TOOL_TIMEOUT`; tests pass something short so a deliberately-hanging fixture command doesn't
    /// make the test suite itself slow.
    pub(super) fn spawn_and_wait_with_timeout(
        cmd: &mut Command,
        timeout: std::time::Duration,
    ) -> Option<std::process::ExitStatus> {
        let mut child = cmd.spawn().ok()?;
        let deadline = std::time::Instant::now() + timeout;
        loop {
            match child.try_wait() {
                Ok(Some(status)) => return Some(status),
                Ok(None) => {
                    if std::time::Instant::now() >= deadline {
                        let _ = child.kill();
                        let _ = child.wait(); // reap; the (now-meaningless, killed) status is discarded
                        return None;
                    }
                    std::thread::sleep(POLL_INTERVAL);
                }
                Err(_) => return None,
            }
        }
    }

    /// Whether the output file exists and is non-empty (verify by the actual file, since content can be empty even with exit code 0).
    fn out_is_nonempty(out: &Path) -> bool {
        std::fs::metadata(out).map(|m| m.len() > 0).unwrap_or(false)
    }

    /// Lazily creates (once per process), and returns the path to, a private `0700` (owner-only)
    /// subdirectory under the system temp dir for this module's rasterized-page PNGs (and qlmanage's
    /// scratch directory). A rasterized page would otherwise briefly sit at a
    /// pid-and-counter-predictable path readable by any other local user on a shared box, since the
    /// PNG is written by `qlmanage`/`sips` — processes we don't control — under whatever mode their
    /// own default umask picks (typically `0644`). Restricting the *directory* (rather than trying to
    /// `chmod` a file after an external tool writes it, which we have no reliable hook for anyway) is
    /// what actually closes this: POSIX requires execute/search permission on every ancestor
    /// directory to open a file by path, so `0700` here blocks other users regardless of the mode the
    /// external tool used for the file itself.
    ///
    /// (macOS's own `std::env::temp_dir()` — a per-user `/var/folders/.../T/` — already happens to be
    /// `0700`, so this is belt-and-braces here rather than the only line of defense it would be under
    /// a world-writable `/tmp`. It is kept because relying on that being true of every macOS release
    /// and every `TMPDIR` override is exactly the kind of assumption that quietly stops holding.)
    fn private_temp_dir() -> PathBuf {
        static DIR: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
        DIR.get_or_init(|| {
            let dir = std::env::temp_dir().join(format!("konoma-pdf-{}", std::process::id()));
            use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
            // The mode is applied atomically by `mkdir(2)` itself (masked by umask, but `0o700`
            // has no group/other bits for umask to strip), so there's no "create, then chmod" gap
            // where a wider-permission window briefly exists.
            let _ = std::fs::DirBuilder::new().mode(0o700).create(&dir);
            // Defense-in-depth for the unlikely case the directory already existed with looser
            // permissions (e.g. a stale leftover from an earlier process that reused this pid).
            let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
            dir
        })
        .clone()
    }

    /// A temp PNG path that does not collide within the process (pid + atomic counter; no randomness/time dependency).
    pub(super) fn temp_png_path() -> PathBuf {
        private_temp_dir().join(format!("page-{}.png", next_id()))
    }

    /// A temp directory path for qlmanage output (same uniqueness scheme).
    pub(super) fn temp_dir_path() -> PathBuf {
        private_temp_dir().join(format!("ql-{}.d", next_id()))
    }

    fn next_id() -> u64 {
        static N: AtomicU64 = AtomicU64::new(0);
        N.fetch_add(1, Ordering::Relaxed)
    }
}

/// Number of pages in the PDF at `path`, parsed **natively** — no external process at all (replaces
/// the former `pdfinfo` child process). Uses `hayro-syntax`, a pure-Rust "read the PDF syntax" crate,
/// to load the document and read its page tree. Returns None when the file is missing, empty, not a
/// PDF, or too corrupt to parse — this is a read-only structural query, so it needs nothing
/// installed at all (unlike before, where no page count was possible without poppler on the machine).
///
/// Wrapped in [`crate::preview::markdown::catch_silent`] as defense-in-depth (the same treatment
/// `render_mermaid_safe`/`decode_gif` get): PDF parsers are a classic panic surface for hostile input,
/// and even though `hayro-syntax`'s `Pdf::new` already returns `Result` for malformed documents (a
/// 3000-iteration byte-mutation fuzz of the bundled sample produced zero panics), principle #3
/// ("unsupported degrades safely") means we don't want a single crash-inducing PDF to take down the
/// whole preview instead of just showing `[can not preview: pdf]`.
pub fn page_count(path: &Path) -> Option<u32> {
    page_count_impl(path, PAGE_COUNT_MAX_BYTES)
}

/// Above this size, `page_count` treats the PDF as "page count unknown" instead of reading the
/// whole file into memory synchronously. `page_count` runs on the UI thread (it's called from
/// `enter_preview`/`reload_media_if_changed`, per the module doc comment above), and — unlike
/// `render_page_native`, which is only ever called from the media worker thread — had no size
/// limit at all here: a multi-hundred-MB PDF would pull the entire file into memory and block the
/// UI while `hayro_syntax::Pdf::new` parses it. 64 MiB comfortably covers the overwhelming
/// majority of real-world PDFs (even fairly long text/photo documents) while keeping the worst
/// case for this synchronous call bounded. The caller already has a graceful degrade for "page
/// count unknown" (documented on `page_count` below: unknown total ⟹ single-page treatment,
/// navigation disabled) — that's exactly the intended behavior here too, matching principle #3.
const PAGE_COUNT_MAX_BYTES: u64 = 64 * 1024 * 1024;

/// `page_count`'s real body, parameterized on the size cap purely for testability — production
/// (`page_count` above) always passes `PAGE_COUNT_MAX_BYTES`; tests pass an artificially small cap
/// against the small, real, bundled sample PDF (see `page_count_treats_oversized_files_as_unknown_
/// rather_than_reading_them` below) rather than needing to construct or store an actual multi-MB
/// fixture file.
fn page_count_impl(path: &Path, max_bytes: u64) -> Option<u32> {
    if !path.is_file() {
        return None;
    }
    let meta = std::fs::metadata(path).ok()?;
    if meta.len() > max_bytes {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    crate::preview::markdown::catch_silent(|| {
        let pdf = hayro_syntax::Pdf::new(bytes).ok()?;
        let n = u32::try_from(pdf.pages().len()).ok()?;
        (n >= 1).then_some(n)
    })
    .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::unique_tmp;

    /// Resolves a fixture bundled under the repo's `samples/` directory, anchored at
    /// `CARGO_MANIFEST_DIR` (baked in at compile time) rather than a bare relative path — a plain
    /// `Path::new("samples/…")` resolves against the test binary's **cwd**, which is only the crate
    /// root by convention (`cargo test` run from elsewhere, e.g. `cd /tmp && cargo test
    /// --manifest-path …`, or a built test binary invoked directly from a different cwd, is a real,
    /// supported invocation — measured: running this crate's built test binary directly with
    /// cwd=`/tmp` turned every `samples/sample.pdf`-gated test here into a silent, instant "0.00s,
    /// N passed" that verified nothing), so it silently missed the fixture and silently skipped
    /// every assertion in every test that used it. Tolerant of the one case where the fixture is
    /// legitimately absent — `samples/` is excluded from the published crate (`Cargo.toml`'s
    /// `exclude`) — by returning `None` (same early-return as before) but saying so loudly
    /// (`eprintln!`, visible with `--nocapture` or in the captured-output dump whenever the process
    /// later exits non-zero for any reason) instead of silently passing zero assertions. Mirrors
    /// `preview/archive.rs`/`preview/image.rs`/`e2e_tests.rs`/`app/tests.rs`'s identical helper
    /// (all four already fixed by `8087219`; this module was the one left behind).
    fn sample_path_or_skip(name: &str) -> Option<std::path::PathBuf> {
        let p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("samples")
            .join(name);
        if p.exists() {
            Some(p)
        } else {
            eprintln!(
                "SKIP: samples/{name} not found (excluded from the published crate) — this test verifies nothing this run"
            );
            None
        }
    }

    /// Returns None for a missing/non-PDF file (does not crash; safe fallback), regardless of
    /// `allow_external`. page_count too.
    #[test]
    fn nonexistent_returns_none() {
        assert!(render_page(Path::new("/no/such/file.pdf"), 1, true).is_none());
        assert!(render_page(Path::new("/no/such/file.pdf"), 3, true).is_none());
        assert!(render_page(Path::new("/no/such/file.pdf"), 1, false).is_none());
        assert!(page_count(Path::new("/no/such/file.pdf")).is_none());
    }

    /// `hayro` renders the bundled sample **without any external tool** — this is the core of the
    /// 2026-07 switch: `allow_external: false` (`[external] pdf = false`) must not prevent PDF
    /// preview from working at all, only prevent the (macOS-only) qlmanage/sips fallback.
    ///
    /// Both values of `allow_external` must produce the *identical* result here, because `hayro`
    /// succeeds and `render_page` returns before the flag is ever consulted — which is also why
    /// asserting the `true` case spawns nothing and is safe to run in this suite.
    #[test]
    fn renders_sample_pdf_via_hayro_with_no_external_tool_allowed() {
        let Some(p) = sample_path_or_skip("sample.pdf") else {
            return;
        };
        let img = render_page(&p, 1, false).expect("hayro renders without any external process");
        assert!(
            img.width() > 0 && img.height() > 0,
            "ラスタライズ結果の寸法が 0"
        );
        let with_external =
            render_page(&p, 1, true).expect("hayro still answers first when external is allowed");
        assert_eq!(
            (with_external.width(), with_external.height()),
            (img.width(), img.height()),
            "allow_external は hayro 経路に一切影響しない(フラグを見る前に返っている)"
        );
    }

    /// The rendered page has actual opaque ink (not just an all-transparent blank canvas) and its
    /// unpainted margins are transparent (background = `TRANSPARENT`, same treatment as the
    /// mermaid/math SVGs — the terminal background should show through, not force white).
    #[test]
    fn render_page_has_transparent_margins_and_opaque_ink() {
        let Some(p) = sample_path_or_skip("sample.pdf") else {
            return;
        };
        let img = render_page(&p, 1, false).expect("renders");
        let rgba = img.to_rgba8();
        assert_eq!(
            rgba.get_pixel(0, 0)[3],
            0,
            "unpainted corner is fully transparent (terminal bg would show through)"
        );
        let opaque = rgba.pixels().filter(|p| p[3] > 200).count();
        assert!(opaque > 50, "page has visible opaque ink (opaque={opaque})");
    }

    /// `page_count` is pure Rust (`hayro-syntax`) and needs no external tool at all — unlike the
    /// former `pdfinfo` call, this must succeed deterministically regardless of what is installed on
    /// the machine running the test. The bundled sample is a known 3-page document.
    #[test]
    fn page_count_is_pure_rust_and_always_available() {
        let Some(p) = sample_path_or_skip("sample.pdf") else {
            return;
        };
        assert_eq!(
            page_count(&p),
            Some(3),
            "外部ツールの有無に関わらずページ数が取れる"
        );
    }

    /// A missing/empty/non-PDF/corrupt file must never panic — `page_count` degrades to None
    /// (principle #3). `catch_silent` is defense-in-depth on top of `hayro-syntax`'s own `Result`.
    #[test]
    fn page_count_handles_bad_input_without_panicking() {
        let dir = unique_tmp("konoma-pdf-badinput");
        std::fs::create_dir_all(&dir).unwrap();

        // An empty file.
        let empty = dir.join("empty.pdf");
        std::fs::write(&empty, b"").unwrap();
        assert_eq!(page_count(&empty), None, "空ファイルは None");

        // A file that isn't a PDF (only the .pdf extension, the content is text).
        let not_pdf = dir.join("notes.pdf");
        std::fs::write(&not_pdf, b"hello, this is not a PDF file at all\n").unwrap();
        assert_eq!(page_count(&not_pdf), None, "PDF でない中身は None");

        // A file with the %PDF magic but corrupt (structurally garbage) content. Whether parsing
        // succeeds or not is fine either way, but it must never panic (the test completing at all is the proof).
        let mut corrupt_bytes = b"%PDF-1.7\n".to_vec();
        corrupt_bytes.extend((0u32..2000).map(|i| (i % 256) as u8));
        let corrupt = dir.join("corrupt.pdf");
        std::fs::write(&corrupt, &corrupt_bytes).unwrap();
        let _ = page_count(&corrupt); // either result is fine — only confirming it doesn't panic

        std::fs::remove_dir_all(&dir).ok();
    }

    /// `hayro` renders **any** page natively — every page of the bundled multi-page sample
    /// rasterizes with `allow_external: false`. This is what makes it acceptable for the fallback
    /// chain to be first-page-only: pages past the first have no fallback at all now, so the
    /// primary renderer has to handle them, and it does. Out-of-range pages degrade to `None`,
    /// never a panic/garbage page.
    #[test]
    fn multipage_sample_counts_and_renders_each_page_via_hayro() {
        let Some(p) = sample_path_or_skip("sample.pdf") else {
            return;
        };
        let pages =
            page_count(&p).expect("hayro-syntax parses the bundled sample without any tool");
        assert!(pages >= 1, "ページ数は 1 以上");
        for pg in 1..=pages {
            let img = render_page(&p, pg, false).expect("hayro renders every page without a tool");
            assert!(img.width() > 0 && img.height() > 0, "page {pg} 寸法が 0");
        }
        // An out-of-range page is None: hayro's `pages.get(idx)` is None, and the fallback declines
        // without spawning anything (page != 1 off the top of `render_page_external`, and off macOS
        // there is no fallback at all).
        assert!(
            render_page(&p, pages + 1, true).is_none(),
            "範囲外ページは None"
        );
    }

    /// Pure mapping logic: which system font family names are tried for a non-embedded CJK CID font,
    /// per `CidFamily`. `AdobeJapan1` puts the Japanese-specific names first; every script's list is
    /// still present afterward as a backstop (Han unification means partial coverage from any CJK
    /// font beats the blank page this replaces). `None` (no character-collection hint) still tries
    /// every script — `fontdb::Database::query` is an exact family-name match, so this can never
    /// misfire onto an unrelated Latin font.
    #[test]
    fn cjk_family_candidates_prioritizes_the_matching_script() {
        let jp = cjk_family_candidates(Some(&CidFamily::AdobeJapan1));
        assert_eq!(
            jp.first(),
            Some(&"Hiragino Sans"),
            "JP は日本語系フォントが先頭"
        );
        assert!(
            jp.contains(&"PingFang SC"),
            "他スクリプトもバックストップとして含む"
        );
        assert!(jp.contains(&"Apple SD Gothic Neo"));

        let sc = cjk_family_candidates(Some(&CidFamily::AdobeGB1));
        assert_eq!(sc.first(), Some(&"PingFang SC"), "簡体字は先頭");

        let tc = cjk_family_candidates(Some(&CidFamily::AdobeCNS1));
        assert_eq!(tc.first(), Some(&"PingFang TC"), "繁体字は先頭");

        let kr = cjk_family_candidates(Some(&CidFamily::AdobeKorea1));
        assert_eq!(kr.first(), Some(&"Apple SD Gothic Neo"), "韓国語は先頭");

        // Even with no hint (None; AdobeIdentity/Custom take the same path), every script is tried in turn.
        let unknown = cjk_family_candidates(None);
        assert!(unknown.contains(&"Hiragino Sans"));
        assert!(unknown.contains(&"PingFang SC"));
        assert!(unknown.contains(&"PingFang TC"));
        assert!(unknown.contains(&"Apple SD Gothic Neo"));
    }

    /// End-to-end (minus an actual PDF): given a real `CharacterCollection`, `cjk_fallback_font`
    /// either finds real font bytes via konoma's shared `usvg`/`resvg` font database, or returns
    /// `None` — never panics. Best-effort: not every machine running this test has a CJK font
    /// installed (e.g. a stripped-down Linux CI runner), so a `None` is a skip, not a failure —
    /// but when something *is* found, it must look like real font data, not garbage.
    #[test]
    fn cjk_fallback_font_finds_real_font_bytes_when_available() {
        let cc = CharacterCollection {
            family: CidFamily::AdobeJapan1,
            supplement: 0,
        };
        match cjk_fallback_font(Some(&cc)) {
            Some((data, _face_index)) => {
                let dynref: &(dyn AsRef<[u8]> + Send + Sync) = &*data;
                let bytes: &[u8] = dynref.as_ref();
                assert!(
                    bytes.len() > 1000,
                    "見つかったフォントは実データを持つ(len={})",
                    bytes.len()
                );
            }
            None => eprintln!("skip: このマシンに CJK フォントが見つからない"),
        }
    }

    /// Root cause: `temp_png_path`/`temp_dir_path` built their paths directly under `std::env::
    /// temp_dir()` — the rasterized page briefly sits at a pid-and-counter-predictable path
    /// readable by any other local user on a shared box, since the PNG (in the external-tool
    /// fallback path) is written by `qlmanage`/`sips` — processes we don't control — under whatever
    /// mode their own default umask picks (typically `0644`). Restricting the **directory** is what
    /// actually closes this — POSIX requires execute/search permission on every ancestor directory
    /// to open a file by path, so `0700` blocks other users regardless of the mode the external
    /// tool used for the file itself.
    ///
    /// macOS-only along with the code it covers: these temp paths only exist to receive output from
    /// the two bundled macOS tools, and no other platform compiles them at all now that poppler is
    /// gone from the chain.
    #[cfg(target_os = "macos")]
    #[test]
    fn temp_paths_live_in_an_owner_only_directory() {
        use std::os::unix::fs::PermissionsExt;

        for path in [macos::temp_png_path(), macos::temp_dir_path()] {
            let dir = path.parent().expect("temp path has a parent dir");
            // The discriminating check: this must be *our own* dedicated subdirectory, not the
            // shared system temp dir directly. Checking only the resulting mode below isn't enough
            // to prove our own code sets it — on macOS, `std::env::temp_dir()` itself already
            // happens to be `0700` (a per-user `/var/folders/.../T/`), which would make a mode-only
            // assertion pass by coincidence even without this fix. (A `TMPDIR` pointed at a
            // world-writable directory, which is what the fix actually defends against here, would
            // not be so lucky — hence asserting on *our own* subdirectory, not just its mode.)
            assert_ne!(
                dir,
                std::env::temp_dir(),
                "システム共有の一時ディレクトリ直下に出力している(専用サブディレクトリを持っていない): {path:?}"
            );
            let meta = std::fs::metadata(dir).expect("private temp dir should exist by now");
            assert!(meta.is_dir());
            let mode = meta.permissions().mode() & 0o777;
            assert_eq!(
                mode, 0o700,
                "ラスタライズ結果の置き場が owner-only(0700) になっていない: {dir:?} mode={mode:#o}"
            );
        }
    }

    /// Root cause: `page_count` read the *entire* PDF into memory unconditionally
    /// (`std::fs::read(path)`), with no size limit at all, and runs synchronously on the UI thread
    /// (per the module doc comment: "the caller ... `enter_preview`/`reload_media_if_changed`").
    /// A very large PDF would pull the whole file into memory and block the UI while doing so.
    ///
    /// Proven without needing to construct or store an actual multi-MB fixture file: the size
    /// guard is exercised directly (`page_count_impl`) with an artificially small cap against the
    /// real, small, known-valid bundled `samples/sample.pdf` — the same file
    /// `page_count_is_pure_rust_and_always_available` proves parses to `Some(3)` under the real
    /// (64 MiB) cap. Capping just *below* that file's actual size must turn the identical valid
    /// content into `None` — this isolates "too large" from "not a valid PDF": a garbage-bytes
    /// fixture couldn't distinguish the two, since it would return `None` either way regardless of
    /// whether a size guard exists at all.
    #[test]
    fn page_count_treats_oversized_files_as_unknown_rather_than_reading_them() {
        let Some(p) = sample_path_or_skip("sample.pdf") else {
            return;
        };
        let real_size = std::fs::metadata(&p).unwrap().len();
        assert_eq!(
            page_count_impl(&p, real_size + 1),
            Some(3),
            "cap がファイルサイズより大きければ通常どおり数えられる"
        );
        assert_eq!(
            page_count_impl(&p, real_size - 1),
            None,
            "cap をファイルサイズ未満にすると(内容は妥当なままなのに)数えない=サイズガードが機能していない"
        );
    }

    /// Root cause: `hayro::render` returns a `Pixmap`, not a `Result` — so a technically
    /// successful render that drew **nothing at all** (a genuinely empty content stream, and,
    /// measured separately via a damage-location sweep of the bundled sample: ~8% of truncation
    /// points that corrupt the content stream without hayro noticing) was indistinguishable from
    /// "hayro legitimately drew a blank page", and `render_page_native_inner` returned
    /// `Some(blank image)` for both. That silently skipped the fallback chain even with
    /// `[external] pdf = true`, and — more importantly now that the chain is only macOS Quick Look
    /// for page 1 — it showed the user a blank rectangle indistinguishable from a real render
    /// instead of an honest `[can not preview]`.
    ///
    /// This fixture is a **valid, well-formed, single-page PDF with a zero-length content
    /// stream** — the PDF-syntax-level shape of "genuinely nothing to draw", as opposed to a
    /// corrupted/truncated one (`page_count_handles_bad_input_without_panicking` above covers the
    /// corrupt-bytes case; this one is deliberately parseable). `render_page_native_inner` must
    /// treat "hayro drew nothing" the same way it already treats every other hayro failure:
    /// return `None`, so `render_page` degrades instead of presenting a blank page as if it were
    /// confirmed real content.
    #[test]
    fn native_render_returns_none_when_hayro_draws_nothing() {
        let dir = unique_tmp("konoma-pdf-blank-content-stream");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("blank.pdf");
        std::fs::write(&p, minimal_blank_pdf_bytes()).unwrap();

        assert!(
            render_page_native_inner(&p, 1).is_none(),
            "hayro が何も描かなかった Pixmap は None を返し、外部ツールへ降格すべき"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// `[external] pdf = false` never shows a blank page as if it were confirmed real content: a
    /// document `hayro` declines (the fixture above) reports "can't preview" (`None`) rather than
    /// the technically-successful empty raster.
    ///
    /// **Deliberately no assertion about the `allow_external: true` path on macOS.** That path
    /// would launch `qlmanage`, i.e. Quick Look — a GUI subsystem — from `cargo test`. Nothing in
    /// this suite may do that: it is slow, it depends on machine state no CI runner shares, and it
    /// puts UI on the developer's screen. The gate itself is covered without spawning anything by
    /// `external_fallback_declines_pages_past_the_first` and
    /// `render_page_external_is_absent_off_macos` below, which exercise both early returns in
    /// `render_page_external` (page ≠ 1, and the non-macOS stub).
    #[test]
    fn render_page_reports_no_preview_when_hayro_draws_nothing_and_external_is_off() {
        let dir = unique_tmp("konoma-pdf-blank-content-stream-fallback");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("blank.pdf");
        std::fs::write(&p, minimal_blank_pdf_bytes()).unwrap();

        assert!(
            render_page(&p, 1, false).is_none(),
            "allow_external=false は blank を real content として見せず None のまま"
        );

        // Off macOS there is no fallback chain left at all, so the flag makes no difference —
        // and this still spawns nothing, so it is safe to assert unconditionally here.
        #[cfg(not(target_os = "macos"))]
        assert!(
            render_page(&p, 1, true).is_none(),
            "macOS 以外では外部フォールバックが存在しないので allow_external=true でも None"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// The fallback tools are first-page-only, so `render_page_external` must decline anything past
    /// page 1 **before** spawning a process. Safe to assert on every platform precisely because it
    /// returns at the top: on macOS this is the branch that keeps a page-2 miss from launching Quick
    /// Look pointlessly, and off macOS the whole function is the stub below.
    ///
    /// This is the guard that used to be an inline `page == 1` condition on the last two rungs of a
    /// longer ladder; poppler occupied the rungs above it and could render an arbitrary page. With
    /// poppler gone (see the module doc comment: it recovered 0 of the 24 documents that reached the
    /// fallback across a 1,628-document corpus), a page-2 failure in `hayro` has no fallback at all —
    /// that is the intended trade, and this test pins the resulting behavior.
    #[test]
    fn external_fallback_declines_pages_past_the_first() {
        let Some(p) = sample_path_or_skip("sample.pdf") else {
            return;
        };
        assert!(
            render_page_external(&p, 2).is_none(),
            "2ページ目以降は外部フォールバック無し(プロセスも起動しない)"
        );
        assert!(render_page_external(&p, 7).is_none());
    }

    /// Off macOS, `render_page_external` is a compile-time stub: no child process, no `PATH` walk,
    /// no timeout poll loop — just `None`. Before poppler was dropped this platform did reach the
    /// external chain (poppler is not macOS-specific); now there is deliberately nothing left for it
    /// to reach, and `hayro` declining is the final answer.
    ///
    /// Asserting on `Path::new("/")` (a directory, not a PDF, and certainly not renderable) makes
    /// the point sharper than a real PDF would: even for input no rasterizer could ever handle,
    /// there is nothing here that could try.
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn render_page_external_is_absent_off_macos() {
        assert!(render_page_external(Path::new("/"), 1).is_none());
        assert!(render_page_external(Path::new("/no/such/file.pdf"), 1).is_none());
    }

    /// A minimal, valid, single-page PDF whose page has an **empty content stream** (`/Length 0`,
    /// no operators at all) — this is what a genuinely blank page looks like at the PDF-syntax
    /// level, as opposed to a corrupted/truncated file. Hand-built (not via an external tool) so
    /// this test needs nothing beyond the Rust toolchain.
    fn minimal_blank_pdf_bytes() -> Vec<u8> {
        let objs: [&[u8]; 4] = [
            b"<< /Type /Catalog /Pages 2 0 R >>",
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 4 0 R /Resources << >> >>",
            b"<< /Length 0 >>\nstream\n\nendstream",
        ];
        let mut out = Vec::new();
        out.extend_from_slice(b"%PDF-1.4\n");
        let mut offsets = vec![0usize];
        for (i, body) in objs.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
            out.extend_from_slice(body);
            out.extend_from_slice(b"\nendobj\n");
        }
        let xref_offset = out.len();
        let n = objs.len() + 1;
        out.extend_from_slice(format!("xref\n0 {n}\n").as_bytes());
        out.extend_from_slice(b"0000000000 65535 f \n");
        for off in &offsets[1..] {
            out.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(
            format!("trailer\n<< /Size {n} /Root 1 0 R >>\nstartxref\n{xref_offset}\n%%EOF\n")
                .as_bytes(),
        );
        out
    }

    /// Root cause: `run_qlmanage`/`run_sips` (and the poppler runners that used to sit above them)
    /// used a blocking `Command::status()` with no upper bound at all — a hung external tool (a
    /// pathological/crafted PDF that makes it loop forever, say) blocked the media worker thread
    /// permanently, leaking both the thread and the child process, and left the busy-indicator
    /// spinner stuck spinning. (`hayro`, the primary pure-Rust renderer, is unaffected — it never
    /// spawns a process at all.)
    ///
    /// Confirmed for real before this fix existed (pasted into the PR/commit description, not kept
    /// as a permanent test): `Command::new("tail").arg("-f").arg("/dev/null").stdout(Stdio::null())
    /// .stderr(Stdio::null()).status()` — the exact shape every function in this module's external
    /// fallback chain used before this fix — was still blocked after 3 real seconds waiting on a
    /// `recv_timeout`.
    ///
    /// This permanent test instead proves it deterministically and fast, via `spawn_and_wait_
    /// with_timeout` directly with a short injected timeout (production always uses the real,
    /// generous `TOOL_TIMEOUT`). The fixture (`tail -f /dev/null`) genuinely never exits on its
    /// own — unlike e.g. `sleep N`, which eventually completes regardless of whether the timeout
    /// mechanism works, and so wouldn't actually prove anything. This project avoids asserting
    /// *exact* wall-clock durations (a prior CI break — see `docs/STATUS.md`), so the bound below
    /// is a large, deliberately loose multiple of the injected timeout: it exists only so a real
    /// regression (no timeout at all) fails this test instead of hanging the whole test run.
    ///
    /// macOS-only now, along with the machinery it guards: `spawn_and_wait_with_timeout` only exists
    /// to drive `qlmanage`/`sips`, and no other platform compiles it. (The fixture is `tail`, a
    /// plain coreutil that touches nothing outside this process — not one of the PDF tools, which
    /// this suite never launches.)
    #[cfg(target_os = "macos")]
    #[test]
    fn spawn_and_wait_with_timeout_kills_a_command_that_never_exits() {
        use std::process::{Command, Stdio};

        let mut cmd = Command::new("tail");
        cmd.arg("-f")
            .arg("/dev/null")
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let started = std::time::Instant::now();
        let status =
            macos::spawn_and_wait_with_timeout(&mut cmd, std::time::Duration::from_millis(200));
        let elapsed = started.elapsed();
        assert!(
            status.is_none(),
            "ハングするコマンドが None(タイムアウト)を返さなかった: {status:?}"
        );
        assert!(
            elapsed < std::time::Duration::from_secs(10),
            "タイムアウトが機能せずブロックし続けた: elapsed={elapsed:?}"
        );
    }
}
