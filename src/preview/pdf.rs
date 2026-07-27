// PDF プレビュー: 指定ページ(1始まり)をラスタライズし、DynamicImage を返す。返り値は video/svg と
// 同じく app の image_src に載せ、以降は通常の画像経路(prepare_image→ワーカー再エンコード→
// kitty graphics)へそのまま流す。ページは1枚ずつ都度ラスタライズする(表示速度優先・先読みしない)。
//
// **レンダラの優先順(2026-07 変更)**:
//   1. `hayro`(純Rust・外部プロセス無し) ―― 既定かつ第一候補。実測で軽い文書は poppler の
//      20〜30倍・写真主体でも 1.8〜12倍速く(15ページ連続描画=143ms 対 poppler 1ページ60〜70ms)、
//      日本語(埋め込みフォント)も正しく描ける。暗号化 PDF・破損 PDF・その他の解析/描画失敗は
//      `None` を返し、下の外部ツールへ**降格**する(原則#3「未対応は安全に」)。
//   2. pdftocairo (poppler・最高品質・アンチエイリアス)
//   3. pdftoppm   (poppler)
//   4. qlmanage   (macOS Quick Look・標準搭載=導入不要)
//   5. sips       (macOS 標準搭載)
// 外部ツール群は `allow_external`(=`[external] pdf`)が false なら一切試さない――hayro は外部
// プロセスを起動しないので、この設定が false でも hayro によるプレビューは動く。全て失敗したら
// None を返し、呼び出し側は安全なフォールバック(ヒント表示)へ降格する(PRD §5 配布容易性)。
// hayro はワンページ描画がミリ秒台なので同期的に呼んでも問題ないが、呼び出し元(app.rs)は既存の
// メディアワーカースレッド経由で呼ぶため、外部ツールのブロッキングも含めて UI を塞がない。
//
// ページ数([`page_count`])は外部プロセスを一切起動しない: `hayro-syntax`(純Rust)でページツリーを
// 直接読む。以前は poppler の `pdfinfo` を子プロセスで呼んでいたが、そのコマンドを消しても
// ページ数取得そのものはできるようになった。hayro が実際のページ描画も担うようになった今は、
// 「ページ数が分かる ⟹ 任意ページを描ける」がほぼ成立する(qlmanage/sips のような「1ページ目専用」
// 制約が無い)ので、呼び出し側(app.rs の `enter_preview`/`reload_media_if_changed`)はもう
// poppler(pdftocairo/pdftoppm)の有無でページナビゲーションを絞り込まない。以前ここにあった
// 「poppler が PATH にあるか」だけを調べるユーティリティ(`arbitrary_page_renderer_available`)は、
// 呼ぶ場所が無くなったので削除した(不要な `#[allow(dead_code)]` は付けず、使われなくなったら消す)。

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
    // 存在しないパスでは何も起動せず即 None(hayro もファイル読み込みで自然に None になるが、
    // 特に qlmanage の QuickLook 起動を避けるため明示的に早期リターンする)。
    if !path.is_file() {
        return None;
    }
    let page = page.max(1);
    if let Some(img) = render_page_native(path, page) {
        return Some(img);
    }
    if !allow_external {
        return None; // `[external] pdf = false`: pdftocairo/pdftoppm/qlmanage/sips を一切起動しない。
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
        // qlmanage/sips は1ページ目しか出せないので page==1 のフォールバックに限る。
        || (page == 1 && (run_qlmanage(path, &out) || run_sips(path, &out)));
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
    let status = Command::new(tool)
        .arg("-png")
        .arg("-f")
        .arg(&page)
        .arg("-l")
        .arg(&page)
        .arg("-singlefile")
        .arg("-scale-to")
        .arg(PAGE_MAX_PX.to_string())
        .arg(path)
        .arg(&prefix)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    matches!(status, Ok(s) if s.success()) && out_is_nonempty(out)
}

/// macOS `qlmanage -t` (Quick Look thumbnail). It writes `<outdir>/<filename>.png`, so we render into a
/// private temp dir and copy the produced PNG to `out`. Always present on macOS = the reliable fallback.
fn run_qlmanage(path: &Path, out: &Path) -> bool {
    let dir = temp_dir_path();
    if std::fs::create_dir_all(&dir).is_err() {
        return false;
    }
    let status = Command::new("qlmanage")
        .arg("-t")
        .arg("-s")
        .arg(PAGE_MAX_PX.to_string())
        .arg("-o")
        .arg(&dir)
        .arg(path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    let mut ok = false;
    if matches!(status, Ok(s) if s.success()) {
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
    let status = Command::new("sips")
        .arg("-s")
        .arg("format")
        .arg("png")
        .arg("--resampleHeightWidthMax")
        .arg(PAGE_MAX_PX.to_string())
        .arg(path)
        .arg("--out")
        .arg(out)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    matches!(status, Ok(s) if s.success()) && out_is_nonempty(out)
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
    if !path.is_file() {
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

/// A temp PNG path that does not collide within the process (pid + atomic counter; no randomness/time dependency).
fn temp_png_path() -> PathBuf {
    std::env::temp_dir().join(format!(
        "konoma-pdf-{}-{}.png",
        std::process::id(),
        next_id()
    ))
}

/// A temp directory path for qlmanage output (same uniqueness scheme).
fn temp_dir_path() -> PathBuf {
    std::env::temp_dir().join(format!("konoma-pdf-{}-{}.d", std::process::id(), next_id()))
}

fn next_id() -> u64 {
    static N: AtomicU64 = AtomicU64::new(0);
    N.fetch_add(1, Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;

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
            return; // samples 除外環境ではスキップ
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
            return; // samples 除外環境ではスキップ(フィクスチャ無し)
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
        let dir = std::env::temp_dir().join(format!("konoma-pdf-badinput-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        // 空ファイル。
        let empty = dir.join("empty.pdf");
        std::fs::write(&empty, b"").unwrap();
        assert_eq!(page_count(&empty), None, "空ファイルは None");

        // PDF でないファイル(拡張子だけ .pdf でも中身はテキスト)。
        let not_pdf = dir.join("notes.pdf");
        std::fs::write(&not_pdf, b"hello, this is not a PDF file at all\n").unwrap();
        assert_eq!(page_count(&not_pdf), None, "PDF でない中身は None");

        // %PDF マジックはあるが中身が壊れている(構造がでたらめ)ファイル。パースに成功してもしなくても
        // よいが、panic だけは絶対にしない(テストが完走すること自体がその確認)。
        let mut corrupt_bytes = b"%PDF-1.7\n".to_vec();
        corrupt_bytes.extend((0u32..2000).map(|i| (i % 256) as u8));
        let corrupt = dir.join("corrupt.pdf");
        std::fs::write(&corrupt, &corrupt_bytes).unwrap();
        let _ = page_count(&corrupt); // どちらの結果でも良い・panic しないことのみ確認

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
        // 範囲外ページは None(hayro 側は pages.get(idx) が None・外部ツールもファイルが生成されない)。
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

        // ヒント無し(None・AdobeIdentity/Custom も同経路)でも全スクリプトを一通り試す。
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
}
