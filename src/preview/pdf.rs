// PDF プレビュー: 外部ツールで指定ページ(1始まり)をラスタライズし、その PNG を読み込んで
// DynamicImage を返す。返り値は video/svg と同じく app の image_src に載せ、以降は通常の画像経路
// (prepare_image→ワーカー再エンコード→kitty graphics)へそのまま流す。ページは1枚ずつ都度
// ラスタライズする(表示速度優先・先読みしない)。複数ページのナビゲーションは poppler 必須
// (qlmanage/sips は1ページ目専用)。ページ数は pdfinfo(poppler)で取得する。
//
// ツールの優先順:
//   1. pdftocairo (poppler・最高品質・アンチエイリアス)
//   2. pdftoppm   (poppler)
//   3. qlmanage   (macOS Quick Look・標準搭載=導入不要)
//   4. sips       (macOS 標準搭載)
// poppler が入っていればそれを使い、無くても macOS では qlmanage/sips が常在するため事実上どの
// 環境でも 1 ページ目が出る。全て失敗したら None を返し、呼び出し側は安全なフォールバック
// (ヒント表示)へ降格する(PRD §5 配布容易性・原則#3「未対応は安全に」)。
// ツール実行はメディアワーカースレッドで行うため、子プロセスのブロッキングは UI を塞がない。

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use image::DynamicImage;

/// Maximum side (px) of the rasterized page. The image path shrinks it further to terminal cells,
/// so this only needs to be large enough that text stays crisp.
const PAGE_MAX_PX: u32 = 1600;

/// Rasterize page `page` (1-based) of the PDF at `path` and return it as an image. Returns None if no
/// rasterizer is available or rendering fails (the caller degrades to a safe fallback with a hint).
///
/// Only poppler (pdftocairo/pdftoppm) can target an arbitrary page; `qlmanage`/`sips` always produce
/// the first page, so they are used only as the page-1 fallback. Page navigation (page > 1) is gated on
/// [`page_count`] succeeding, which also requires poppler — so the two capabilities stay coupled.
pub fn render_page(path: &Path, page: u32) -> Option<DynamicImage> {
    // 存在しないパスでは外部ツール(特に qlmanage の QuickLook 起動)を回さず即 None。
    // ツリーから開くファイルは常に実在するため実用上の影響は無く、無駄な子プロセス起動を防ぐ。
    if !path.is_file() {
        return None;
    }
    let page = page.max(1);
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

/// Number of pages, via poppler `pdfinfo` (parses the `Pages: N` line). Returns None when pdfinfo is
/// unavailable or parsing fails — the caller then treats the PDF as single-page (no page navigation).
/// This deliberately uses poppler only: pdfinfo present ⟺ pdftocairo/pdftoppm present, so "we know the
/// count" and "we can render an arbitrary page" hold together (qlmanage/sips can render page 1 only).
pub fn page_count(path: &Path) -> Option<u32> {
    if !path.is_file() {
        return None;
    }
    let output = Command::new("pdfinfo")
        .arg(path)
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("Pages:") {
            if let Ok(n) = rest.trim().parse::<u32>() {
                return (n >= 1).then_some(n);
            }
        }
    }
    None
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

    /// Returns None for a missing/non-PDF file (does not crash; safe fallback). page_count too.
    #[test]
    fn nonexistent_returns_none() {
        assert!(render_page(Path::new("/no/such/file.pdf"), 1).is_none());
        assert!(render_page(Path::new("/no/such/file.pdf"), 3).is_none());
        assert!(page_count(Path::new("/no/such/file.pdf")).is_none());
    }

    /// On macOS at least one rasterizer (qlmanage/sips) is always present, so the bundled sample PDF
    /// must render to a non-empty image. Skipped only if no tool is available (none → safe None).
    #[test]
    fn renders_sample_pdf_when_a_tool_is_available() {
        let p = Path::new("samples/sample.pdf");
        if !p.exists() {
            return; // samples 除外環境ではスキップ
        }
        match render_page(p, 1) {
            Some(img) => assert!(
                img.width() > 0 && img.height() > 0,
                "ラスタライズ結果の寸法が 0"
            ),
            None => eprintln!("skip: PDF ラスタライザ不在(qlmanage/sips/poppler いずれも不可)"),
        }
    }

    /// With poppler present, the bundled sample is multi-page and each page rasterizes. Skipped when
    /// pdfinfo is absent (page_count = None → single-page fallback, navigation disabled).
    #[test]
    fn multipage_sample_counts_and_renders_each_page() {
        let p = Path::new("samples/sample.pdf");
        if !p.exists() {
            return;
        }
        let Some(pages) = page_count(p) else {
            eprintln!("skip: pdfinfo 不在(poppler 無し)= ページ数不明・単ページ扱い");
            return;
        };
        assert!(pages >= 1, "ページ数は 1 以上");
        // 各ページが非空にラスタライズできること(poppler があるので page>1 も出る)。
        for pg in 1..=pages {
            let img = render_page(p, pg).expect("poppler があれば各ページがラスタライズできる");
            assert!(img.width() > 0 && img.height() > 0, "page {pg} 寸法が 0");
        }
        // 範囲外ページは None(ファイルが生成されない)。
        assert!(render_page(p, pages + 1).is_none(), "範囲外ページは None");
    }
}
