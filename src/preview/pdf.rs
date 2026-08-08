// PDF preview: rasterizes the given page (1-based) and returns a DynamicImage. Like video/svg, the
// return value is put on the app's image_src, and from there flows straight through the normal image
// path (prepare_image → worker re-encode → kitty graphics). Pages are rasterized one at a time, on
// demand (prioritizing display speed, no read-ahead).
//
// **Renderer priority order (changed 2026-07)**:
//   1. `hayro` (pure Rust, no external process) — the default and first choice. Measured: 20-30x
//      faster than poppler for light documents, and 1.8-12x faster even for photo-heavy ones (15
//      consecutive pages rendered = 143ms vs poppler's 60-70ms per single page), and renders Japanese
//      (embedded fonts) correctly too. Encrypted PDFs, corrupt PDFs, and other parse/render failures
//      return `None` and **degrade** to the external tools below (principle #3, "unsupported degrades safely").
//   2. pdftocairo (poppler, best quality, anti-aliased)
//   3. pdftoppm   (poppler)
//   4. qlmanage   (macOS Quick Look, bundled = no install needed)
//   5. sips       (bundled with macOS)
// None of the external tools are tried at all if `allow_external` (= `[external] pdf`) is false —
// hayro never launches an external process, so PDF preview via hayro still works even with this
// setting false. If everything fails, returns None, and the caller degrades to a safe fallback (a
// hint display) (PRD §5, ease of distribution).
// hayro's per-page render takes only single-digit milliseconds, so calling it synchronously is fine,
// but the caller (app.rs) calls it through the existing media-worker thread, so even blocking on an
// external tool never blocks the UI.
//
// Page count ([`page_count`]) never launches an external process at all: it reads the page tree
// directly with `hayro-syntax` (pure Rust). It used to call poppler's `pdfinfo` as a child process,
// but even after removing that command, getting the page count itself became possible. Now that
// hayro also handles the actual page rendering, "knowing the page count ⟹ can render any page" holds
// almost universally (there's no "first-page-only" constraint like qlmanage/sips have), so the caller
// (app.rs's `enter_preview`/`reload_media_if_changed`) no longer gates page navigation on whether
// poppler (pdftocairo/pdftoppm) is present. The utility that used to live here checking only "is
// poppler on PATH" (`arbitrary_page_renderer_available`) has been deleted since it has no more
// callers (rather than tagging it `#[allow(dead_code)]` needlessly — delete it once it's unused).

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
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
/// external tools — and only when `allow_external` is true (`[external] pdf`). Returns `None` if
/// nothing could render the page (the caller degrades to a safe fallback with a hint).
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
        return None; // `[external] pdf = false`: never launch pdftocairo/pdftoppm/qlmanage/sips.
    }
    render_page_external(path, page)
}

/// Rasterize with `hayro` — pure Rust, no child process at all. `None` means "hayro couldn't do it"
/// (encrypted, corrupt, page out of range, or a caught panic), never a crash: [`render_page`] then
/// tries the external tools (if allowed).
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

/// The external-tool fallback chain (used only when `hayro` fails and the caller allows it).
fn render_page_external(path: &Path, page: u32) -> Option<DynamicImage> {
    let out = temp_png_path();
    let ok = run_pdftocairo(path, &out, page)
        || run_pdftoppm(path, &out, page)
        // qlmanage/sips can only produce the first page, so the fallback is limited to page==1.
        || (page == 1 && (run_qlmanage(path, &out) || run_sips(path, &out)));
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

/// poppler `pdftocairo`. `-singlefile` writes `<prefix>.png` (so prefix = `out` without the extension
/// lands the file exactly at `out`). `-scale-to` caps the longer side. Renders page `page` (`-f/-l page`).
fn run_pdftocairo(path: &Path, out: &Path, page: u32) -> bool {
    run_poppler("pdftocairo", path, out, page)
}

/// poppler `pdftoppm`. Same `-singlefile`/`-scale-to`/single-page convention as pdftocairo.
fn run_pdftoppm(path: &Path, out: &Path, page: u32) -> bool {
    run_poppler("pdftoppm", path, out, page)
}

/// Shared driver for the two poppler tools (identical flags). `out` must end in `.png`; the prefix
/// passed to the tool is `out` with the extension stripped, so the produced `<prefix>.png` == `out`.
/// Renders only page `page` (`-f page -l page`); an out-of-range page produces no file → returns false.
fn run_poppler(tool: &str, path: &Path, out: &Path, page: u32) -> bool {
    let prefix = out.with_extension("");
    let page = page.to_string();
    let mut cmd = Command::new(tool);
    cmd.arg("-png")
        .arg("-f")
        .arg(&page)
        .arg("-l")
        .arg(&page)
        .arg("-singlefile")
        .arg("-scale-to")
        .arg(PAGE_MAX_PX.to_string())
        .arg(path)
        .arg(&prefix)
        .stdin(Stdio::null()) // never let a helper read the terminal out from under crossterm
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    matches!(spawn_and_wait_with_timeout(&mut cmd, TOOL_TIMEOUT), Some(s) if s.success())
        && out_is_nonempty(out)
}

/// macOS `qlmanage -t` (Quick Look thumbnail). It writes `<outdir>/<filename>.png`, so we render into a
/// private temp dir and copy the produced PNG to `out`. Always present on macOS = the reliable fallback.
fn run_qlmanage(path: &Path, out: &Path) -> bool {
    let dir = temp_dir_path();
    if std::fs::create_dir_all(&dir).is_err() {
        return false;
    }
    #[cfg(unix)]
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
fn run_sips(path: &Path, out: &Path) -> bool {
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

/// How long an external rendering tool (`pdftocairo`/`pdftoppm`/`qlmanage`/`sips`) is allowed to
/// run before being killed and treated as a failure. Generous on purpose — a large/complex page
/// can legitimately take a while — this exists only to turn "never returns" into "eventually gives
/// up," not to police normal runtimes. Without this, a hung tool blocked `Command::status()`
/// indefinitely: since these run on the media worker thread (see the module doc comment above),
/// that leaked the worker thread *and* the child process forever, and left the busy-indicator
/// spinner stuck spinning. (`hayro`, the pure-Rust primary renderer above, is unaffected — it never
/// spawns a process at all; this only guards the external-tool fallback chain.)
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
fn spawn_and_wait_with_timeout(
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

/// Number of pages in the PDF at `path`, parsed **natively** — no external process at all (replaces
/// the former `pdfinfo` child process). Uses `hayro-syntax`, a pure-Rust "read the PDF syntax" crate,
/// to load the document and read its page tree. Returns None when the file is missing, empty, not a
/// PDF, or too corrupt to parse — this is a read-only structural query, so it works even without
/// poppler installed (unlike before, where no page count was possible at all without it).
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

/// Whether the output file exists and is non-empty (verify by the actual file, since content can be empty even with exit code 0).
fn out_is_nonempty(out: &Path) -> bool {
    std::fs::metadata(out).map(|m| m.len() > 0).unwrap_or(false)
}

/// Lazily creates (once per process), and returns the path to, a private `0700` (owner-only)
/// subdirectory under the system temp dir for this module's rasterized-page PNGs (and qlmanage's
/// scratch directory). `std::env::temp_dir()` is world-writable/world-readable on Linux (`/tmp` is
/// mode `1777`) — a rasterized page would otherwise briefly sit at a pid-and-counter-predictable
/// path readable by any other local user on a shared box, since the PNG (in the external-tool
/// fallback path) is written by `pdftocairo`/`pdftoppm`/`qlmanage`/`sips` — processes we don't
/// control — under whatever mode their own default umask picks (typically `0644`). Restricting the
/// *directory* (rather than trying to `chmod` a file after an external tool writes it, which we
/// have no reliable hook for anyway) is what actually closes this: POSIX requires execute/search
/// permission on every ancestor directory to open a file by path, so `0700` here blocks other users
/// regardless of the mode the external tool used for the file itself.
fn private_temp_dir() -> PathBuf {
    static DIR: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    DIR.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!("konoma-pdf-{}", std::process::id()));
        #[cfg(unix)]
        {
            use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
            // The mode is applied atomically by `mkdir(2)` itself (masked by umask, but `0o700`
            // has no group/other bits for umask to strip), so there's no "create, then chmod" gap
            // where a wider-permission window briefly exists.
            let _ = std::fs::DirBuilder::new().mode(0o700).create(&dir);
            // Defense-in-depth for the unlikely case the directory already existed with looser
            // permissions (e.g. a stale leftover from an earlier process that reused this pid).
            let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
        }
        #[cfg(not(unix))]
        {
            let _ = std::fs::create_dir(&dir);
        }
        dir
    })
    .clone()
}

/// A temp PNG path that does not collide within the process (pid + atomic counter; no randomness/time dependency).
fn temp_png_path() -> PathBuf {
    private_temp_dir().join(format!("page-{}.png", next_id()))
}

/// A temp directory path for qlmanage output (same uniqueness scheme).
fn temp_dir_path() -> PathBuf {
    private_temp_dir().join(format!("ql-{}.d", next_id()))
}

fn next_id() -> u64 {
    static N: AtomicU64 = AtomicU64::new(0);
    N.fetch_add(1, Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::unique_tmp;

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
    /// preview from working at all, only prevent the pdftocairo/pdftoppm/qlmanage/sips fallback.
    #[test]
    fn renders_sample_pdf_via_hayro_with_no_external_tool_allowed() {
        let p = Path::new("samples/sample.pdf");
        if !p.exists() {
            return; // skip when samples are excluded from the build environment
        }
        let img = render_page(p, 1, false).expect("hayro renders without any external process");
        assert!(
            img.width() > 0 && img.height() > 0,
            "ラスタライズ結果の寸法が 0"
        );
    }

    /// The rendered page has actual opaque ink (not just an all-transparent blank canvas) and its
    /// unpainted margins are transparent (background = `TRANSPARENT`, same treatment as the
    /// mermaid/math SVGs — the terminal background should show through, not force white).
    #[test]
    fn render_page_has_transparent_margins_and_opaque_ink() {
        let p = Path::new("samples/sample.pdf");
        if !p.exists() {
            return;
        }
        let img = render_page(p, 1, false).expect("renders");
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
    /// former `pdfinfo` call, this must succeed deterministically regardless of whether poppler is
    /// installed on the machine running the test. The bundled sample is a known 3-page document.
    #[test]
    fn page_count_is_pure_rust_and_always_available() {
        let p = Path::new("samples/sample.pdf");
        if !p.exists() {
            return; // skip when samples are excluded from the build environment (no fixture)
        }
        assert_eq!(
            page_count(p),
            Some(3),
            "poppler の有無に関わらずページ数が取れる"
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

    /// `hayro` renders **any** page natively (no external tool needed for page > 1, unlike the old
    /// qlmanage/sips-only fallback) — every page of the bundled multi-page sample rasterizes with
    /// `allow_external: false`. Out-of-range pages degrade to `None`, never a panic/garbage page.
    #[test]
    fn multipage_sample_counts_and_renders_each_page_via_hayro() {
        let p = Path::new("samples/sample.pdf");
        if !p.exists() {
            return;
        }
        let pages = page_count(p).expect("hayro-syntax parses the bundled sample without any tool");
        assert!(pages >= 1, "ページ数は 1 以上");
        for pg in 1..=pages {
            let img = render_page(p, pg, false).expect("hayro renders every page without a tool");
            assert!(img.width() > 0 && img.height() > 0, "page {pg} 寸法が 0");
        }
        // An out-of-range page is None (on the hayro side pages.get(idx) is None; external tools also produce no file).
        assert!(
            render_page(p, pages + 1, true).is_none(),
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
    /// temp_dir()`, which is world-writable/world-readable on Linux (`/tmp` is mode `1777`) — the
    /// rasterized page briefly sits at a pid-and-counter-predictable path readable by any other
    /// local user on a shared box, since the PNG (in the external-tool fallback path) is written by
    /// `pdftocairo`/`pdftoppm`/`qlmanage`/`sips` — processes we don't control — under whatever mode
    /// their own default umask picks (typically `0644`). Restricting the **directory** is what
    /// actually closes this — POSIX requires execute/search permission on every ancestor directory
    /// to open a file by path, so `0700` blocks other users regardless of the mode the external
    /// tool used for the file itself.
    #[cfg(unix)]
    #[test]
    fn temp_paths_live_in_an_owner_only_directory() {
        use std::os::unix::fs::PermissionsExt;

        for path in [temp_png_path(), temp_dir_path()] {
            let dir = path.parent().expect("temp path has a parent dir");
            // The discriminating check: this must be *our own* dedicated subdirectory, not the
            // shared system temp dir directly. Checking only the resulting mode below isn't enough
            // to prove our own code sets it — on macOS, `std::env::temp_dir()` itself already
            // happens to be `0700` (a per-user `/var/folders/.../T/`), which would make a mode-only
            // assertion pass by coincidence even without this fix. Linux's `/tmp` (mode `1777`,
            // world-writable) has no such luck, which is exactly the platform this bug targets.
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
        let p = Path::new("samples/sample.pdf");
        if !p.exists() {
            return; // skip when samples are excluded from the build environment
        }
        let real_size = std::fs::metadata(p).unwrap().len();
        assert_eq!(
            page_count_impl(p, real_size + 1),
            Some(3),
            "cap がファイルサイズより大きければ通常どおり数えられる"
        );
        assert_eq!(
            page_count_impl(p, real_size - 1),
            None,
            "cap をファイルサイズ未満にすると(内容は妥当なままなのに)数えない=サイズガードが機能していない"
        );
    }

    /// Root cause: `run_poppler`/`run_qlmanage`/`run_sips` used a blocking `Command::status()`
    /// with no upper bound at all — a hung external tool (a pathological/crafted PDF that makes it
    /// loop forever, say) blocked the media worker thread permanently, leaking both the thread and
    /// the child process, and left the busy-indicator spinner stuck spinning. (`hayro`, the primary
    /// pure-Rust renderer, is unaffected — it never spawns a process at all.)
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
    #[cfg(unix)]
    #[test]
    fn spawn_and_wait_with_timeout_kills_a_command_that_never_exits() {
        let mut cmd = Command::new("tail");
        cmd.arg("-f")
            .arg("/dev/null")
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let started = std::time::Instant::now();
        let status = spawn_and_wait_with_timeout(&mut cmd, std::time::Duration::from_millis(200));
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
