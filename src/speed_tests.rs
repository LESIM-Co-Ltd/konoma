//! Speed / performance smoke-regression guards (NOT microbenchmarks).
//!
//! Each test asserts a documented hot path completes under a **generous** bound (chosen ~5-10x
//! typical so normal CI noise won't flake it). They catch gross regressions — accidental O(n²),
//! a lost cache, re-highlighting every frame — not small deltas. The most timing-sensitive ones are
//! `#[ignore]`d (run them with `cargo test -- --ignored`); a `//` note on each says what it guards.
//! Bounds are deliberately loose because these run in an unoptimized debug build.
//!
//! ## Goal: guard the perf-critical surface, not the whole codebase
//! These tests aim to cover the **perf-critical set** — the parse / render / diff / decode hot paths
//! that run per-keystroke on the previewed content or on large inputs: `preview/*` (code, markdown,
//! table, window, text, gitdiff, image, kitty), `git.rs` (statuses / ignored / diff / graph), and the
//! per-frame render modules `ui/{tree,preview,git,table}.rs`. app.rs is mostly cold state/action code,
//! so its hot functions (build_dir, ensure_md_cache, md_slice, load_active, file_diff) are guarded
//! individually but app.rs is **not** part of the coverage denominator. Each guard feeds a large,
//! feature-complete input so it doubles as a coverage driver for the module it protects. The point is
//! that a regression in any of these — a lost cache, an O(n²), re-parsing every frame — trips a guard.

use crate::config::Config;
use std::path::Path;
use std::time::{Duration, Instant};

/// Build an App previewing `path` (tree cursor on it + activate), rendered once into an 80x30
/// TestBackend so the decoration / windowing / table caches are built exactly as in the run loop.
#[cfg(test)]
fn preview_app(dir: &Path, name: &str) -> crate::app::App {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    let mut app = crate::app::App::new(dir.to_path_buf(), Config::default()).unwrap();
    app.selected = app
        .entries
        .iter()
        .position(|e| e.path.ends_with(name))
        .unwrap();
    app.tree_activate().unwrap();
    let mut term = Terminal::new(TestBackend::new(80, 30)).unwrap();
    term.draw(|f| crate::ui::render(f, &mut app)).unwrap();
    app
}

/// Generate `n` lines of plausible Rust source (for highlight/markdown timing).
fn rust_source(n: usize) -> String {
    let mut s = String::with_capacity(n * 32);
    for i in 0..n {
        s.push_str(&format!(
            "fn item_{i}(x: i32) -> i32 {{ let y = x + {i}; y * 2 }} // comment {i}\n"
        ));
    }
    s
}

// GUARDS: preview::code::highlight_lang must stay roughly linear on big inputs (no per-line
// re-compilation of the grammar). 5000 lines of Rust under a very loose 10s debug bound.
#[test]
#[ignore] // 重い(debug の syntect 5000 行)。cargo test -- --ignored で実行。
fn highlight_lang_large_source_is_bounded() {
    let src = rust_source(5000);
    let t = Instant::now();
    let lines = crate::preview::code::highlight_lang(&src, "rust", "TwoDark");
    let dt = t.elapsed();
    assert_eq!(lines.len(), 5000, "全行ハイライトされる");
    assert!(
        dt < Duration::from_secs(10),
        "5000 行ハイライトが遅すぎる(回帰?): {dt:?}"
    );
}

// GUARDS: after warm_dir/warm_file primes the grammar, a subsequent highlight is fast (the warm
// cache works). We don't assert a cold/warm ratio (process-global warm state); just that both the
// warm step and the highlight finish under loose bounds.
#[test]
fn warm_then_highlight_is_fast() {
    let dir = std::env::temp_dir().join("konoma_speed_warm_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("a.rs");
    std::fs::write(&f, rust_source(300).as_bytes()).unwrap();

    let t = Instant::now();
    crate::preview::code::warm_file("rs", &f);
    let warm_dt = t.elapsed();
    assert!(
        warm_dt < Duration::from_secs(5),
        "warm_file が遅すぎる: {warm_dt:?}"
    );

    let src = std::fs::read_to_string(&f).unwrap();
    let t = Instant::now();
    let lines = crate::preview::code::highlight(&src, &f, "TwoDark");
    let hot_dt = t.elapsed();
    assert_eq!(lines.len(), 300);
    assert!(
        hot_dt < Duration::from_secs(2),
        "ウォーム後のハイライトが遅すぎる: {hot_dt:?}"
    );
    std::fs::remove_dir_all(&dir).ok();
}

// GUARDS: preview::markdown::render_markdown on a large document stays bounded (table/heading
// post-processing and tui-markdown parse don't blow up). ~1000 mixed lines.
#[test]
#[ignore] // 大きめ md(コードフェンス込み)で重め。cargo test -- --ignored で実行。
fn render_markdown_large_doc_is_bounded() {
    let mut md = String::new();
    for i in 0..400 {
        md.push_str(&format!(
            "## Heading {i}\n\nSome *body* text with `code` and a [link](https://x/{i}).\n\n"
        ));
    }
    md.push_str("```rust\n");
    md.push_str(&rust_source(200));
    md.push_str("```\n");
    let t = Instant::now();
    let lines = crate::preview::markdown::render_markdown(
        &md,
        80,
        crate::preview::markdown::CodeStyle::default(),
        "TwoDark",
        false,
    );
    let dt = t.elapsed();
    assert!(!lines.is_empty());
    assert!(
        dt < Duration::from_secs(10),
        "大きい md のレンダリングが遅すぎる: {dt:?}"
    );
}

// GUARDS: building the tree and computing the on-screen visible range for a directory with many
// entries stays responsive (visible-range rendering must not be O(entries) heavy per frame).
// ~1000 files. The file creation is setup; we time the rebuild + a render pass separately.
#[test]
fn tree_build_and_visible_range_many_entries_is_bounded() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    let dir = std::env::temp_dir().join("konoma_speed_tree_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for i in 0..1000 {
        std::fs::write(dir.join(format!("file_{i:04}.txt")), b"x").unwrap();
    }
    let mut app = crate::app::App::new(dir.clone(), Config::default()).unwrap();

    // ツリー再構築(1000 件)が速いこと。
    let t = Instant::now();
    app.rebuild_tree().unwrap();
    let build_dt = t.elapsed();
    assert_eq!(app.entries.len(), 1000, "全件ツリー化");
    assert!(
        build_dt < Duration::from_secs(2),
        "1000 件のツリー再構築が遅すぎる: {build_dt:?}"
    );

    // 末尾を選んだ状態で1回描画(可視範囲計算を含む)が速いこと。
    app.selected = app.entries.len() - 1;
    let mut term = Terminal::new(TestBackend::new(60, 30)).unwrap();
    let t = Instant::now();
    term.draw(|f| crate::ui::render(f, &mut app)).unwrap();
    let draw_dt = t.elapsed();
    assert!(
        draw_dt < Duration::from_secs(1),
        "可視範囲を含む1描画が遅すぎる: {draw_dt:?}"
    );
    std::fs::remove_dir_all(&dir).ok();
}

// GUARDS: preview::image::decode_gif on the bundled sample GIF stays bounded (full-frame expand).
// Skips when samples are excluded from the package.
#[test]
fn decode_gif_sample_is_bounded() {
    let p = Path::new("samples/sample.gif");
    if !p.exists() {
        return; // samples 除外環境ではスキップ
    }
    let t = Instant::now();
    let frames = crate::preview::image::decode_gif(p);
    let dt = t.elapsed();
    assert!(
        frames.map(|f| f.len() > 1).unwrap_or(false),
        "アニメ GIF として展開"
    );
    assert!(
        dt < Duration::from_secs(5),
        "sample.gif のデコードが遅すぎる: {dt:?}"
    );
}

// GUARDS: refresh_git_if_needed must NOT re-run `git status` (a whole-worktree scan) on every
// `h`/`l` root change within the same repo — the per-workdir cache reuses it. Asserted by counting
// actual `git::statuses` calls (deterministic): a lost cache re-scans on every move, which measured
// ~2.1s for 40 moves on this repo vs ~3ms reused. (A wall-clock-only bound flakes on git's warm/cold
// timing, so the call count is the primary guard.)
#[cfg(feature = "git")]
#[test]
#[ignore] // 重い(多数ファイルの git リポジトリを作る)。cargo test -- --ignored で実行。
fn same_repo_navigation_does_not_rescan_git_status() {
    use std::process::Command;
    let dir = std::env::temp_dir().join("konoma_speed_status_cache");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    // 多数ファイル = 1 回の `git status` スキャンが測定可能な時間になる。
    for i in 0..4000 {
        std::fs::write(dir.join(format!("f{i:04}.txt")), b"x\n").unwrap();
    }
    let repo = git2::Repository::init(&dir).unwrap();
    {
        let mut c = repo.config().unwrap();
        c.set_str("user.name", "T").unwrap();
        c.set_str("user.email", "t@t").unwrap();
        c.set_str("commit.gpgsign", "false").ok();
    }
    let git = |a: &[&str]| {
        Command::new("git")
            .current_dir(&dir)
            .args(a)
            .output()
            .unwrap();
    };
    git(&["add", "-A"]);
    git(&["commit", "-qm", "init"]);
    for i in 0..200 {
        std::fs::write(dir.join(format!("f{i:04}.txt")), b"y\n").unwrap(); // 変更 = status 非空
    }

    let root = dir.canonicalize().unwrap();
    let mut app = crate::app::App::new(root.clone(), Config::default()).unwrap();
    app.refresh_git_if_needed(); // 初回スキャンでキャッシュ確立(タイマ外)。
    assert!(
        app.git_has_changes(),
        "変更が status に反映(スキャンが走った)"
    );

    // **決定的な**ガード: 同一 repo 内で root を 40 回往復しても `git::statuses`(全 worktree の
    // スキャン)を1回も呼び直さない(workdir キャッシュ流用)。wall-clock 比較は git のウォーム/
    // コールド差でブレて回帰を見逃す(実測: キャッシュ喪失時は 40 スキャン=約2.1s、流用時は約3ms)
    // ので、実呼び出し回数で判定する。
    crate::git::STATUS_CALLS.store(0, std::sync::atomic::Ordering::SeqCst);
    let t = Instant::now();
    for i in 0..40 {
        app.root = if i % 2 == 0 {
            root.join("sub")
        } else {
            root.clone()
        };
        app.refresh_git_if_needed();
    }
    let dt = t.elapsed();

    assert_eq!(
        crate::git::STATUS_CALLS.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "同一 repo の 40 往復で git status を再スキャンした(workdir キャッシュ喪失)"
    );
    // 参考の緩い上限(スキャンが40回走れば秒単位=必ず超える)。
    assert!(
        dt < Duration::from_secs(1),
        "同一 repo 往復が遅すぎる: {dt:?}"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// Generate a large, GFM-feature-complete Markdown document (headings, tables, alerts, footnotes,
/// task lists, `<details>`, autolink, inline HTML, nested lists, blockquotes, code fences). Feeding
/// this through the decorated preview exercises most of `preview/markdown.rs` — so it guards the whole
/// Markdown render surface, not just a happy path.
#[cfg(test)]
fn feature_markdown(blocks: usize) -> String {
    let mut s = String::from("---\ntitle: Speed\ntags: [a, b]\n---\n\n# Top\n\n");
    for i in 0..blocks {
        s.push_str(&format!("## Section {i}\n\n"));
        s.push_str("Body with *italic*, **bold**, `code`, ~~strike~~, and a bare URL https://example.com/x plus :rocket: emoji.[^note]\n\n");
        s.push_str("[^note]: A footnote body.\n\n");
        s.push_str("> [!NOTE]\n> An alert with a [link](https://example.com) inside.\n\n");
        s.push_str(
            "| Col A | Col B | Col C |\n|:--|:-:|--:|\n| a | b | c |\n| 日本語 | x | y |\n\n",
        );
        s.push_str("- [ ] task one\n- [x] task two\n  - nested\n\n");
        s.push_str("<details>\n<summary>More</summary>\n\nHidden body text.\n\n</details>\n\n");
        s.push_str("```rust\nfn demo() { let _ = 1 + 2; }\n```\n\n");
    }
    s
}

// GUARDS: previewing a large feature-complete Markdown file (decoration cache build + first render)
// stays responsive, and scrolling reuses the MdCache instead of re-decorating every frame. Covers
// preview/markdown.rs + the app decorated path (ensure_md_cache / md_slice) + ui/preview.rs.
#[test]
#[ignore] // 重い(大きな装飾 md のビルド)。cargo test -- --ignored で実行。
fn preview_large_markdown_is_bounded() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    let dir = std::env::temp_dir().join("konoma_speed_md_preview");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("big.md"), feature_markdown(120)).unwrap();

    // 初回プレビュー(装飾キャッシュ構築 + 1 描画)が緩い上限内。
    let t = Instant::now();
    let mut app = preview_app(&dir, "big.md");
    let build_dt = t.elapsed();
    assert!(
        build_dt < Duration::from_secs(10),
        "大きな装飾 md の初回プレビューが遅すぎる: {build_dt:?}"
    );

    // スクロールしながら 60 描画: MdCache 流用で毎フレーム再装飾しない(遅ければ再装飾疑い)。
    let mut term = Terminal::new(TestBackend::new(80, 30)).unwrap();
    let t = Instant::now();
    for i in 0..60 {
        app.preview_scroll((i % 7) + 1);
        term.draw(|f| crate::ui::render(f, &mut app)).unwrap();
    }
    let scroll_dt = t.elapsed();
    assert!(
        scroll_dt < Duration::from_secs(3),
        "装飾 md のスクロール 60 描画が遅すぎる(毎フレーム再装飾?): {scroll_dt:?}"
    );
    std::fs::remove_dir_all(&dir).ok();
}

// GUARDS: previewing a large CSV (parse + aligned-grid render with cursor moves) stays responsive.
// Covers preview/table.rs + ui/table.rs + the app table path.
#[test]
fn preview_large_csv_is_bounded() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    let dir = std::env::temp_dir().join("konoma_speed_csv");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut body = String::from("c0,c1,c2,c3,c4,c5,c6,c7\n");
    for i in 0..2000 {
        body.push_str(&format!("r{i}a,r{i}b,r{i}c,日本語{i},e,f,g,h\n"));
    }
    std::fs::write(dir.join("big.csv"), body).unwrap();

    let t = Instant::now();
    let mut app = preview_app(&dir, "big.csv");
    let load_dt = t.elapsed();
    assert!(app.is_table_preview(), "テーブル面で開く");
    assert!(
        load_dt < Duration::from_secs(5),
        "大きな CSV のパース+初回描画が遅すぎる: {load_dt:?}"
    );

    // セル移動 + 再描画 80 回(横スクロール/列幅再計算を含む)が緩い上限内。
    let mut term = Terminal::new(TestBackend::new(60, 24)).unwrap();
    let t = Instant::now();
    for i in 0..80 {
        app.table_cursor_move(i % 5, (i % 3) - 1);
        term.draw(|f| crate::ui::render(f, &mut app)).unwrap();
    }
    let nav_dt = t.elapsed();
    assert!(
        nav_dt < Duration::from_secs(2),
        "CSV のセル移動 80 描画が遅すぎる: {nav_dt:?}"
    );
    std::fs::remove_dir_all(&dir).ok();
}

// GUARDS: windowed preview of a large text file (open + page down/up many times) reads only the
// visible window each step (no whole-file scan per frame). Covers preview/window.rs + preview/text.rs
// + ui/preview.rs. find_all_matches is bounded by its CAP.
#[test]
fn preview_large_file_windowing_is_bounded() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    let dir = std::env::temp_dir().join("konoma_speed_window");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut body = String::new();
    for i in 0..20000 {
        body.push_str(&format!("line {i} with some MATCHME content 日本語\n"));
    }
    std::fs::write(dir.join("big.txt"), &body).unwrap();

    let t = Instant::now();
    let mut app = preview_app(&dir, "big.txt");
    let open_dt = t.elapsed();
    assert!(
        open_dt < Duration::from_secs(2),
        "大きなテキストの初回プレビューが遅すぎる(全文走査?): {open_dt:?}"
    );

    // ページ送り 100 回 + 描画: 各ステップは可視窓のみ読む(全文走査なら O(n) で遅い)。
    let mut term = Terminal::new(TestBackend::new(80, 30)).unwrap();
    let t = Instant::now();
    for _ in 0..100 {
        app.preview_scroll(25);
        term.draw(|f| crate::ui::render(f, &mut app)).unwrap();
    }
    let page_dt = t.elapsed();
    assert!(
        page_dt < Duration::from_secs(3),
        "ページ送り 100 描画が遅すぎる: {page_dt:?}"
    );

    // 検索(FileWindow::find_all_matches は CAP で上限)。多数一致でも緩い上限内。
    let mut win = crate::preview::window::FileWindow::open(&dir.join("big.txt")).unwrap();
    let t = Instant::now();
    let hits = win.find_all_matches("MATCHME", 5000).unwrap();
    let find_dt = t.elapsed();
    assert!(!hits.is_empty(), "一致を見つける");
    assert!(
        find_dt < Duration::from_secs(3),
        "20000 行の検索が遅すぎる: {find_dt:?}"
    );
    std::fs::remove_dir_all(&dir).ok();
}

// GUARDS: konoma's own kitty transmit (crop → resize_exact → zlib-compress) on a large image stays
// bounded, and the o=z compression actually shrinks the payload (a lost compression path would
// balloon the escape stream = the ~50x slower full-image display we fixed). Covers preview/kitty.rs.
#[test]
fn kitty_transmit_large_image_is_bounded() {
    use image::{DynamicImage, Rgba, RgbaImage};
    let font = ratatui_image::FontSize {
        width: 8,
        height: 16,
    };
    // build_from_source: 大きな元画像(スクショ大 1600x1000)を表示セルへ crop+resize+圧縮する経路。
    let mut big = RgbaImage::new(1600, 1000);
    for (x, y, px) in big.enumerate_pixels_mut() {
        *px = Rgba([(x % 256) as u8, (y % 256) as u8, 128, 255]);
    }
    let src = DynamicImage::ImageRgba8(big);
    // crop → resize_exact → 圧縮の経路が動くこと。ガードは**決定的**な項目（表示セルサイズ・o=z・
    // 圧縮率）に絞る。画像 resize の wall-clock は image crate 依存で共有 CI ランナーの負荷変動に弱く
    // flake するため、時間上限は敢えて置かない（STATUS_CALLS 同様、決定的な不変量で回帰を捕まえる）。
    let ki =
        crate::preview::kitty::build_from_source(&src, (0, 0, 1600, 1000), 120, 40, font, false)
            .expect("kitty 画像を構築");
    assert_eq!(ki.cell_size(), (120, 40), "表示セルサイズで構築");

    // build_transmit: 表示サイズ(120*8 x 40*16)の RGBA を o=z 圧縮転送に。圧縮が効いていること。
    let (pw, ph) = (120u32 * 8, 40u32 * 16);
    let mut disp = RgbaImage::new(pw, ph);
    for (x, y, px) in disp.enumerate_pixels_mut() {
        *px = Rgba([(x % 256) as u8, (y % 256) as u8, 128, 255]);
    }
    let esc = crate::preview::kitty::build_transmit(&disp, 1, false);
    assert!(esc.contains("o=z"), "o=z 圧縮チャンクを含む");
    let raw = (pw * ph * 4) as usize; // 生 RGBA バイト数
    assert!(
        esc.len() < raw / 2,
        "圧縮が効いていない(esc {} vs raw {})",
        esc.len(),
        raw
    );
}

// GUARDS: a large `git diff` (3000-line rewrite) and rendering each git view (hub / log / graph /
// branches / diff) stay bounded. Covers git.rs (statuses / file_diff / log / graph / branches) +
// preview/gitdiff.rs + ui/git.rs + the diff render in ui/preview.rs.
#[cfg(feature = "git")]
#[test]
#[ignore] // git リポジトリ(履歴 + 大きな変更)を作るので重い。cargo test -- --ignored で実行。
fn git_views_and_large_diff_render_is_bounded() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    let dir = std::env::temp_dir().join("konoma_speed_git_views");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let git = |a: &[&str]| {
        std::process::Command::new("git")
            .current_dir(&dir)
            .args(a)
            .output()
            .unwrap();
    };
    git(&["init", "-q", "."]);
    git(&["config", "user.email", "t@t"]);
    git(&["config", "user.name", "t"]);
    git(&["config", "commit.gpgsign", "false"]);
    // 履歴(log / graph 用の複数コミット)。
    for c in 0..8 {
        std::fs::write(dir.join(format!("f{c}.txt")), format!("v{c}\n")).unwrap();
        git(&["add", "-A"]);
        git(&["commit", "-qm", &format!("commit {c}")]);
    }
    // 大きなファイルをコミット → 全行書換で大きな未コミット diff を作る。
    let mut big = String::new();
    for i in 0..3000 {
        big.push_str(&format!("line {i}\n"));
    }
    std::fs::write(dir.join("big.txt"), &big).unwrap();
    git(&["add", "big.txt"]);
    git(&["commit", "-qm", "add big"]);
    let mut big2 = String::new();
    for i in 0..3000 {
        big2.push_str(&format!("LINE {i} changed\n"));
    }
    std::fs::write(dir.join("big.txt"), &big2).unwrap();

    let root = dir.canonicalize().unwrap();
    let mut app = crate::app::App::new(root.clone(), Config::default()).unwrap();

    // 大きな file_diff が緩い上限内(diff 取得は per-keystroke に近いホットパス)。
    let t = Instant::now();
    let dl = crate::git::file_diff(&root, &root.join("big.txt"));
    let diff_dt = t.elapsed();
    assert!(!dl.is_empty(), "差分が取れる");
    assert!(
        diff_dt < Duration::from_secs(3),
        "3000 行の file_diff が遅すぎる: {diff_dt:?}"
    );

    // preview/gitdiff.rs: 差分行 → 着色 Line 生成(縦/横並び + 横スクロール上限)を直接叩く。
    let t = Instant::now();
    let lines = crate::preview::gitdiff::diff_lines(&dl, "txt", "TwoDark", 90);
    assert!(!lines.is_empty(), "縦 diff 行を生成");
    let _maxh = crate::preview::gitdiff::side_by_side_max_hscroll(&dl, 90);
    let sbs = crate::preview::gitdiff::diff_lines_side_by_side(&dl, "txt", "TwoDark", 90, 40);
    assert!(!sbs.is_empty(), "横並び diff 行を生成");
    let gen_dt = t.elapsed();
    assert!(
        gen_dt < Duration::from_secs(2),
        "3000 行の diff 着色生成が遅すぎる: {gen_dt:?}"
    );

    // 各 git ビューを開いて描画(ハブ / log / graph / branches / 大きな diff)。
    let mut term = Terminal::new(TestBackend::new(100, 40)).unwrap();
    let t = Instant::now();
    app.open_git_view();
    term.draw(|f| crate::ui::render(f, &mut app)).unwrap();
    app.open_git_log();
    term.draw(|f| crate::ui::render(f, &mut app)).unwrap();
    app.open_git_graph();
    term.draw(|f| crate::ui::render(f, &mut app)).unwrap();
    app.open_git_branches();
    term.draw(|f| crate::ui::render(f, &mut app)).unwrap();
    app.open_git_diff(&root.join("big.txt"));
    term.draw(|f| crate::ui::render(f, &mut app)).unwrap();
    let views_dt = t.elapsed();
    assert!(
        views_dt < Duration::from_secs(3),
        "git ビュー群 + 大きな diff の描画が遅すぎる: {views_dt:?}"
    );
    std::fs::remove_dir_all(&dir).ok();
}

// GUARDS: cycling through tabs (save_active + load_active + preview rebuild + render) stays bounded.
// load_active uses mem::take (perf refactor C: measured 39ms → 6.6ms); a regression that deep-clones
// the whole TabState on every switch would blow this up. Diverse preview kinds so each switch rebuilds
// a different cache (markdown decoration / table / windowed code).
#[test]
#[ignore] // 複数タブ + プレビュー再構築で重め。cargo test -- --ignored で実行。
fn tab_switch_reloads_are_bounded() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    let dir = std::env::temp_dir().join("konoma_speed_tabswitch");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut md = String::from("# T\n\n");
    for i in 0..200 {
        md.push_str(&format!("## H{i}\n\nbody [l](https://e/{i})\n\n"));
    }
    std::fs::write(dir.join("a.md"), &md).unwrap();
    let mut csv = String::from("x,y,z\n");
    for i in 0..2000 {
        csv.push_str(&format!("{i},b,c\n"));
    }
    std::fs::write(dir.join("b.csv"), &csv).unwrap();
    std::fs::write(dir.join("c.txt"), rust_source(3000)).unwrap();
    std::fs::write(dir.join("d.rs"), rust_source(2000)).unwrap();

    let mut app = crate::app::App::new(dir.clone(), Config::default()).unwrap();
    let mut term = Terminal::new(TestBackend::new(80, 30)).unwrap();
    // 各ファイルを別タブで開く(切替で別々のプレビューキャッシュを再構築させる)。
    for name in ["a.md", "b.csv", "c.txt", "d.rs"] {
        app.selected = app
            .entries
            .iter()
            .position(|e| e.path.ends_with(name))
            .unwrap();
        app.tab_new_from_selection().unwrap();
        term.draw(|f| crate::ui::render(f, &mut app)).unwrap();
    }
    assert!(app.tab_count() >= 4, "4 つ以上のタブ");

    // 40 回タブを巡回: 各切替 = save_active + load_active(mem::take) + プレビュー再構築 + 描画。
    let t = Instant::now();
    for _ in 0..40 {
        app.tab_cycle(1);
        term.draw(|f| crate::ui::render(f, &mut app)).unwrap();
    }
    let dt = t.elapsed();
    assert!(
        dt < Duration::from_secs(3),
        "40 回のタブ切替(再読込込み)が遅すぎる(全複製回帰?): {dt:?}"
    );
    std::fs::remove_dir_all(&dir).ok();
}
