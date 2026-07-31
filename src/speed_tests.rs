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
    app.tab.selected = app
        .tab
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
#[ignore] // heavy (5000 lines of syntect in a debug build). Run with cargo test -- --ignored.
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
#[ignore] // on the heavier side (a large md including code fences). Run with cargo test -- --ignored.
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

    // Confirm the tree rebuild (1000 entries) is fast.
    let t = Instant::now();
    app.rebuild_tree().unwrap();
    let build_dt = t.elapsed();
    assert_eq!(app.tab.entries.len(), 1000, "全件ツリー化");
    assert!(
        build_dt < Duration::from_secs(2),
        "1000 件のツリー再構築が遅すぎる: {build_dt:?}"
    );

    // Confirm one render pass with the last entry selected (including visible-range computation) is fast.
    app.tab.selected = app.tab.entries.len() - 1;
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
        return; // skip in an environment where samples are excluded
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
#[ignore] // heavy (builds a git repo with many files). Run with cargo test -- --ignored.
fn same_repo_navigation_does_not_rescan_git_status() {
    use std::process::Command;
    let dir = std::env::temp_dir().join("konoma_speed_status_cache");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    // Many files = a single `git status` scan takes a measurable amount of time.
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
        std::fs::write(dir.join(format!("f{i:04}.txt")), b"y\n").unwrap(); // change = non-empty status
    }

    let root = dir.canonicalize().unwrap();
    let mut app = crate::app::App::new(root.clone(), Config::default()).unwrap();
    app.refresh_git_if_needed(); // establish the cache with an initial scan (outside the timer)
    assert!(
        app.git_has_changes(),
        "変更が status に反映(スキャンが走った)"
    );

    // **Deterministic** guard: bouncing root back and forth 40 times within the same repo must never
    // re-call `git::statuses` (a whole-worktree scan) even once (the workdir cache is reused). A
    // wall-clock comparison is noisy across git's warm/cold timing and would miss a regression
    // (measured: ~2.1s for 40 scans when the cache is lost, vs ~3ms when reused), so we judge by
    // the actual call count instead.
    crate::git::STATUS_CALLS.store(0, std::sync::atomic::Ordering::SeqCst);
    let t = Instant::now();
    for i in 0..40 {
        app.tab.root = if i % 2 == 0 {
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
    // A loose bound as a sanity check (if 40 scans actually ran, it'd be second-scale = would definitely exceed this).
    assert!(
        dt < Duration::from_secs(1),
        "同一 repo 往復が遅すぎる: {dt:?}"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// GUARD: the linked-worktree chip's origin repo name (`App::worktree_origin`, shown by
/// `ui/status.rs::context_spans`) rides along with `git_branch`/`git status` in the background
/// scan (`scan_statuses`) and must be computed **once per root change**, never recomputed on every
/// render (the context bar draws every frame).
///
/// Counted by **scan dispatches landing on the channel**, not `git::WORKTREE_ORIGIN_CALLS` — that
/// counter is process-shared, and any *other* test's `App::refresh_git_if_needed()` running
/// concurrently (the default test harness runs tests in parallel threads) also increments it, the
/// same pitfall `STATUS_CALLS`'s doc comment already warns about (confirmed: this test flaked when
/// run alongside the other worktree tests before switching to the channel). Since `worktree_origin`
/// is computed *only* inside `scan_statuses` — the very function `refresh_git_if_needed` dispatches
/// — "no extra scan dispatched" is equivalent to "no extra `worktree_origin` computed", and this
/// count is immune to unrelated tests' git activity.
#[cfg(feature = "git")]
#[test]
fn worktree_chip_is_not_recomputed_on_every_render() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    let dir = std::env::temp_dir().join("konoma_speed_worktree_chip_main");
    let linked = std::env::temp_dir().join("konoma_speed_worktree_chip_linked");
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&linked);
    std::fs::create_dir_all(&dir).unwrap();
    let repo = git2::Repository::init(&dir).unwrap();
    {
        let mut c = repo.config().unwrap();
        c.set_str("user.name", "T").unwrap();
        c.set_str("user.email", "t@t").unwrap();
        c.set_str("commit.gpgsign", "false").ok();
    }
    std::fs::write(dir.join("a.txt"), b"x\n").unwrap();
    let git = |cwd: &Path, a: &[&str]| {
        let out = std::process::Command::new("git")
            .current_dir(cwd)
            .args(a)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {a:?}: {out:?}");
    };
    git(&dir, &["add", "-A"]);
    git(&dir, &["commit", "-qm", "init"]);
    let main_root = dir.canonicalize().unwrap();
    git(
        &main_root,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "feat",
            linked.to_str().unwrap(),
        ],
    );
    let linked_root = linked.canonicalize().unwrap();

    let mut app = crate::app::App::new(linked_root, Config::default()).unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    app.attach_status_loader(tx);
    app.refresh_git_if_needed(); // dispatch the 1st (only) scan
    let res = rx
        .recv_timeout(std::time::Duration::from_secs(30))
        .expect("1本目の結果");
    assert!(app.apply_statuses(res));
    assert!(
        app.worktree_origin().is_some(),
        "linked worktree のはずが origin が取れていない"
    );

    let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
    for _ in 0..50 {
        // Exercises the real render path: `ui::tree::render` calls `refresh_git_if_needed()` itself
        // every frame (the same call the run loop makes before drawing).
        term.draw(|f| crate::ui::render(f, &mut app)).unwrap();
    }
    assert_eq!(
        rx.try_iter().count(),
        0,
        "同じ root への描画50回で worktree_origin(scan_statuses)を再計算してはいけない"
    );
    assert!(
        app.worktree_origin().is_some(),
        "50回の描画後も表示は保たれているはず"
    );
    std::fs::remove_dir_all(&linked).ok();
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
#[ignore] // heavy (building a large decorated md). Run with cargo test -- --ignored.
fn preview_large_markdown_is_bounded() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    let dir = std::env::temp_dir().join("konoma_speed_md_preview");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("big.md"), feature_markdown(120)).unwrap();

    // The initial preview (building the decoration cache + 1 render) stays within a loose bound.
    let t = Instant::now();
    let mut app = preview_app(&dir, "big.md");
    let build_dt = t.elapsed();
    assert!(
        build_dt < Duration::from_secs(10),
        "大きな装飾 md の初回プレビューが遅すぎる: {build_dt:?}"
    );

    // 60 renders while scrolling: reusing the MdCache means we don't re-decorate every frame (slowness would suggest re-decoration).
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

    // 80 rounds of cell move + re-render (including horizontal scroll / column-width recomputation) stay within a loose bound.
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

    // 100 page-downs + render: each step reads only the visible window (a full-file scan would be O(n) and slow).
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

    // Search (FileWindow::find_all_matches is capped by CAP). Stays within a loose bound even with many matches.
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
    // build_from_source: the path that crops+resizes+compresses a large source image (screenshot-sized 1600x1000) down to display cells.
    let mut big = RgbaImage::new(1600, 1000);
    for (x, y, px) in big.enumerate_pixels_mut() {
        *px = Rgba([(x % 256) as u8, (y % 256) as u8, 128, 255]);
    }
    let src = DynamicImage::ImageRgba8(big);
    // Confirm the crop → resize_exact → compress path runs. The guard is restricted to
    // **deterministic** items (display cell size, o=z, compression ratio). Image resize's
    // wall-clock time depends on the image crate and is prone to flaking under shared-CI-runner
    // load variance, so we deliberately don't set a time bound here (like STATUS_CALLS, we catch
    // regressions via deterministic invariants instead).
    let ki =
        crate::preview::kitty::build_from_source(&src, (0, 0, 1600, 1000), 120, 40, font, false)
            .expect("kitty 画像を構築");
    assert_eq!(ki.cell_size(), (120, 40), "表示セルサイズで構築");

    // build_transmit: send the display-size (120*8 x 40*16) RGBA as an o=z compressed transfer. Confirm compression is actually working.
    let (pw, ph) = (120u32 * 8, 40u32 * 16);
    let mut disp = RgbaImage::new(pw, ph);
    for (x, y, px) in disp.enumerate_pixels_mut() {
        *px = Rgba([(x % 256) as u8, (y % 256) as u8, 128, 255]);
    }
    let esc = crate::preview::kitty::build_transmit(&disp, 1, false);
    assert!(esc.contains("o=z"), "o=z 圧縮チャンクを含む");
    let raw = (pw * ph * 4) as usize; // raw RGBA byte count
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
#[ignore] // heavy (builds a git repo with history + a large change). Run with cargo test -- --ignored.
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
    // History (several commits for log / graph).
    for c in 0..8 {
        std::fs::write(dir.join(format!("f{c}.txt")), format!("v{c}\n")).unwrap();
        git(&["add", "-A"]);
        git(&["commit", "-qm", &format!("commit {c}")]);
    }
    // Commit a large file → rewrite every line to create a large uncommitted diff.
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

    // A large file_diff stays within a loose bound (fetching a diff is a near-per-keystroke hot path).
    let t = Instant::now();
    let dl = crate::git::file_diff(&root, &root.join("big.txt"));
    let diff_dt = t.elapsed();
    assert!(!dl.is_empty(), "差分が取れる");
    assert!(
        diff_dt < Duration::from_secs(3),
        "3000 行の file_diff が遅すぎる: {diff_dt:?}"
    );

    // preview/gitdiff.rs: hit the diff-lines → colored Line generation (stacked/side-by-side + horizontal-scroll cap) directly.
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

    // Open and render each git view (hub / log / graph / branches / the large diff).
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
// the whole PerTab on every switch would blow this up. Diverse preview kinds so each switch rebuilds
// a different cache (markdown decoration / table / windowed code).
#[test]
#[ignore] // on the heavier side (multiple tabs + preview rebuilds). Run with cargo test -- --ignored.
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
    // Open each file in a separate tab (switching between them rebuilds a different preview cache each time).
    for name in ["a.md", "b.csv", "c.txt", "d.rs"] {
        app.tab.selected = app
            .tab
            .entries
            .iter()
            .position(|e| e.path.ends_with(name))
            .unwrap();
        app.tab_new_from_selection().unwrap();
        term.draw(|f| crate::ui::render(f, &mut app)).unwrap();
    }
    assert!(app.tab_count() >= 4, "4 つ以上のタブ");

    // Cycle through tabs 40 times: each switch = save_active + load_active (mem::take) + preview rebuild + render.
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

// GUARDS: main.rs's `BurstPaths` (the fs-watcher burst de-duplicator) stays fast for a burst of tens
// of thousands of *distinct* paths. The historical bug: the run loop's fs-event drain used to
// de-duplicate a burst with `Vec::contains` (an O(n) scan per path, so O(n^2) overall, no cap at
// all). Measured on a real burst of 72,000 newly created files (three `cp -R` of a 24,000-file
// tree), that pinned konoma at 99% CPU with a **completely frozen UI for over 3 minutes** —
// `sample(1)` showed the main thread stuck in `Component::PartialEq` called from this loop. This is
// konoma's core use case (an AI agent generating files, a build writing `target/`/`node_modules/`),
// so it matters a lot. Not `#[ignore]`d: it guards a bug severe enough to freeze the app for minutes,
// so it must run in the normal suite, not just on demand.
#[test]
fn burst_paths_handles_large_distinct_burst_without_quadratic_blowup() {
    let t = Instant::now();
    let mut burst = crate::BurstPaths::default();
    for i in 0..50_000 {
        let p = std::path::PathBuf::from(format!("/repo/generated/f{i:06}.txt"));
        burst.push(&p);
    }
    let dt = t.elapsed();
    // 50,000 is far past MAX_BURST_PATHS (1024) = it gets rounded down to "unknown" (a safe-side fallback).
    assert_eq!(
        burst.finish(),
        None,
        "上限を超える distinct バーストは「パス不明」として扱われる"
    );
    // 2 seconds is a wide safety margin (normally a few milliseconds). With the old implementation
    // (Vec::contains, no cap), de-duplicating 50,000 entries would be O(n^2) ≈ 1.25 billion PathBuf
    // comparisons, taking tens of seconds to minutes — the same order as the measured 72,000-entry
    // case (over 3 minutes, 99% CPU).
    assert!(
        dt < Duration::from_secs(2),
        "distinct な大量パスの重複除去が遅すぎる(線形走査への回帰?): {dt:?}"
    );
}
