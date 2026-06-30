//! Speed / performance smoke-regression guards (NOT microbenchmarks).
//!
//! Each test asserts a documented hot path completes under a **generous** bound (chosen ~5-10x
//! typical so normal CI noise won't flake it). They catch gross regressions — accidental O(n²),
//! a lost cache, re-highlighting every frame — not small deltas. The most timing-sensitive ones are
//! `#[ignore]`d (run them with `cargo test -- --ignored`); a `//` note on each says what it guards.
//! Bounds are deliberately loose because these run in an unoptimized debug build.

use crate::config::Config;
use std::path::Path;
use std::time::{Duration, Instant};

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
