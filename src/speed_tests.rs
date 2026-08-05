//! Speed / performance smoke-regression guards (NOT microbenchmarks).
//!
//! Each test asserts a documented hot path completes under a **generous** bound. They catch gross
//! regressions — accidental O(n²), a lost cache, re-highlighting every frame — not small deltas.
//!
//! Prefer a **deterministic byte-allocation count** (via `crate::mem_tests::allocated_by`) over
//! `Instant`/`Duration` wherever the guard can be phrased that way: bytes allocated are a CPU/load
//! -independent proxy for "how much work happened", so a scaling ratio (e.g. "2x input → <3x alloc")
//! or a dispatch/channel count catches the same regressions a wall-clock bound would, without flaking
//! under a busy shared CI runner — which is also what lets these run in the normal suite instead of
//! behind `#[ignore]`. A few genuinely can't be phrased that way (a one-time, allocation-heavy
//! grammar-compile *itself* being slow, not a lost cache around it) and stay `Instant`-based with a
//! loose (~5-10x typical) bound as a smoke check.
//!
//! When the *ratio* a guard would need shrinks close to 1 (the ratio between the normal case and the
//! regressed case), a relative-allocation bound stops being reliable — environment noise (allocator
//! internals, path-length differences between platforms, ...) can eat the whole margin. When that
//! happens, prefer asserting the underlying structural invariant directly (e.g. "this slot was moved
//! out of, not cloned") over chasing a tighter ratio; see `tab_switch_reloads_are_bounded` below for a
//! concrete case where both techniques are combined (a loose allocation ceiling as a coarse net, plus
//! a per-cycle structural assertion as the precise one).
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
// re-compilation of the grammar / no accidental O(n^2)). Converted from a wall-clock bound (which
// flakes under shared-CI-runner load — see the top-of-file note and the module doc) to a
// **deterministic allocation-scaling** check: 2x the lines should allocate roughly 2x, not ~4x
// (quadratic) or worse. Bytes allocated are a CPU/load-independent proxy for "how much work
// happened", so — unlike `Instant` — this doesn't need CI-noise headroom, which is what lets it run
// in the normal suite instead of behind `#[ignore]`.
#[test]
fn highlight_lang_large_source_is_bounded() {
    // Warm the "rust" grammar once outside the measurement: the very first highlight of a language
    // in the whole test binary compiles its regex patterns (a one-time, allocation-heavy cost —
    // see `warm_dir_makes_subsequent_highlight_fast`'s doc in preview/code.rs), which would otherwise
    // land on whichever of the two measurements below runs first and skew the ratio.
    let _ = crate::preview::code::highlight_lang(&rust_source(10), "rust", "TwoDark");

    let small = rust_source(1500);
    let large = rust_source(3000); // 2x lines
    let small_alloc = crate::mem_tests::allocated_by(|| {
        let lines = crate::preview::code::highlight_lang(&small, "rust", "TwoDark");
        assert_eq!(lines.len(), 1500, "全行ハイライトされる");
    });
    let large_alloc = crate::mem_tests::allocated_by(|| {
        let lines = crate::preview::code::highlight_lang(&large, "rust", "TwoDark");
        assert_eq!(lines.len(), 3000, "全行ハイライトされる");
    });
    assert!(
        large_alloc < small_alloc.saturating_mul(3),
        "2倍の行数で確保バイト数が3倍を超えた(回帰: O(n^2)?): small={small_alloc} large={large_alloc}"
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

/// `n` heading+body+link blocks of Markdown, plus a Rust fence sized proportionally to `n` (table/
/// heading post-processing, tui-markdown parse, and code-fence highlighting all get exercised, and —
/// since every part scales with `n` — doubling `n` should double the total work, keeping the
/// allocation-scaling check below sensitive instead of being diluted by a fixed-size fence).
fn md_doc(blocks: usize) -> String {
    let mut md = String::new();
    for i in 0..blocks {
        md.push_str(&format!(
            "## Heading {i}\n\nSome *body* text with `code` and a [link](https://x/{i}).\n\n"
        ));
    }
    md.push_str("```rust\n");
    md.push_str(&rust_source(blocks));
    md.push_str("```\n");
    md
}

// GUARDS: preview::markdown::render_markdown on a large document stays bounded (table/heading
// post-processing and tui-markdown parse don't blow up). Converted from a wall-clock bound to a
// **deterministic allocation-scaling** check (same reasoning as `highlight_lang_large_source_is_bounded`
// above): doubling the block count should roughly double the allocation, not quadruple it.
#[test]
fn render_markdown_large_doc_is_bounded() {
    let render = |src: &str| {
        crate::preview::markdown::render_markdown(
            src,
            80,
            crate::preview::markdown::CodeStyle::default(),
            "TwoDark",
            false,
        )
    };
    // Warm the "rust" fence's grammar outside the measurement (same reasoning as the highlight_lang guard).
    let _ = render(&md_doc(1));

    let small = md_doc(100);
    let large = md_doc(200); // 2x blocks
    let small_alloc = crate::mem_tests::allocated_by(|| {
        assert!(!render(&small).is_empty());
    });
    let large_alloc = crate::mem_tests::allocated_by(|| {
        assert!(!render(&large).is_empty());
    });
    assert!(
        large_alloc < small_alloc.saturating_mul(3),
        "2倍のブロック数で確保バイト数が3倍を超えた(回帰: O(n^2)?): small={small_alloc} large={large_alloc}"
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
// `h`/`l` root change within the same repo — the per-workdir cache reuses it. Converted from
// wall-clock timing + the process-shared `git::STATUS_CALLS` counter (heavy setup: 4000 files, so
// `#[ignore]`d; also a process-shared `AtomicUsize` flakes if another test's `refresh_git_if_needed()`
// runs concurrently — the same pitfall `worktree_chip_is_not_recomputed_on_every_render`'s doc
// comment below warns about) to a **channel dispatch count**: attach a `Sender` and assert nothing
// lands on it after the cache is established — deterministic, thread-local to this test's own
// channel, and immune to both CI load and parallel-test interference. Since the guard no longer
// needs a slow scan to be *measurable*, the fixture can shrink drastically (still real git activity).
#[cfg(feature = "git")]
#[test]
fn same_repo_navigation_does_not_rescan_git_status() {
    use std::process::Command;
    let dir = crate::mem_tests::unique_tmp("konoma_speed_status_cache");
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    for i in 0..20 {
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
    std::fs::write(dir.join("f0000.txt"), b"y\n").unwrap(); // change = non-empty status

    let root = dir.canonicalize().unwrap();
    let mut app = crate::app::App::new(root.clone(), Config::default()).unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    app.attach_status_loader(tx);
    app.refresh_git_if_needed(); // dispatch the 1st (only) scan, establishing the workdir cache
    let res = rx
        .recv_timeout(Duration::from_secs(30))
        .expect("1本目の結果");
    assert!(app.apply_statuses(res));
    assert!(
        app.git_has_changes(),
        "変更が status に反映(スキャンが走った)"
    );

    // **Deterministic** guard: bouncing root back and forth 40 times within the same repo must never
    // dispatch another scan (the workdir cache is reused) — checked by counting results landing on
    // this test's own channel, not a process-shared counter.
    for i in 0..40 {
        app.tab.root = if i % 2 == 0 {
            root.join("sub")
        } else {
            root.clone()
        };
        app.refresh_git_if_needed();
    }
    assert_eq!(
        rx.try_iter().count(),
        0,
        "同一 repo の 40 往復で git status を再スキャンした(workdir キャッシュ喪失)"
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

// GUARDS: previewing a large feature-complete Markdown file builds the decoration cache once, and
// scrolling reuses it instead of re-decorating every frame. Converted from a wall-clock bound to a
// **deterministic allocation-ratio** check (same technique as `md_scroll_reuses_cache_not_redecorate`
// in mem_tests.rs, whose simpler 300-heading fixture this complements — this one keeps
// `feature_markdown`'s GFM-feature-complete surface, exercising more of preview/markdown.rs + the app
// decorated path (ensure_md_cache / md_slice) + ui/preview.rs). A lost cache would make one scroll's
// allocation approach the initial decoration's, instead of staying a small fraction of it.
//
// Measures **one** steady-state scroll (after a few unmeasured warm-up scrolls), not a sum over many
// — summing N scrolls' *marginal* per-frame cost and comparing it against a single one-time build
// cost isn't apples-to-apples (confirmed while calibrating: summing 60 scrolls made the "scroll" side
// scale with N regardless of caching, eventually overtaking any fixed multiple of the one-time build
// cost — that's arithmetic, not a caching regression). One scroll against one build mirrors what
// `md_scroll_reuses_cache_not_redecorate` already checks.
#[test]
fn preview_large_markdown_is_bounded() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    let dir = crate::mem_tests::unique_tmp("konoma_speed_md_preview");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("big.md"), feature_markdown(30)).unwrap();

    // Warm the "rust" grammar outside the measurement: `feature_markdown`'s embedded fence highlights
    // via the same process-global, one-time-per-language syntect compile as the guards above. Without
    // this, whether that first-time compile cost lands inside `build_alloc` below is a coin flip that
    // depends on **test execution order** — parallel `cargo test` runs may have already warmed "rust"
    // from another test's fixture, or not (confirmed while calibrating: `build_alloc` measured ~65MB
    // running this test alone vs ~6.8MB inside the full suite, where an earlier test had already
    // warmed it — the same pitfall documented for `STATUS_CALLS`, but for warm state rather than a
    // counter).
    let _ = crate::preview::code::highlight_lang(&rust_source(1), "rust", "TwoDark");

    // The initial preview (building the decoration cache + 1 render).
    let mut app = None;
    let build_alloc = crate::mem_tests::allocated_by(|| {
        app = Some(preview_app(&dir, "big.md"));
    });
    let mut app = app.unwrap();

    let mut term = Terminal::new(TestBackend::new(80, 30)).unwrap();
    // A few unmeasured warm-up scrolls (in case the very first post-build scroll does extra one-time
    // work), then measure a single steady-state scroll+render.
    for i in 0..5 {
        app.preview_scroll((i % 7) + 1);
        term.draw(|f| crate::ui::render(f, &mut app)).unwrap();
    }
    let one_scroll_alloc = crate::mem_tests::allocated_by(|| {
        app.preview_scroll(3);
        term.draw(|f| crate::ui::render(f, &mut app)).unwrap();
    });
    assert!(
        one_scroll_alloc.saturating_mul(20) < build_alloc,
        "装飾 md の1スクロールがキャッシュを再利用していない(スクロール {one_scroll_alloc} が初回装飾 {build_alloc} に近い=毎フレーム再装飾?)"
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

/// `n` lines of `"{marker} {i}\n"` (a synthetic large-file body for diff generation).
#[cfg(feature = "git")]
fn diff_body(n: usize, marker: &str) -> String {
    let mut s = String::with_capacity(n * (marker.len() + 8));
    for i in 0..n {
        s.push_str(&format!("{marker} {i}\n"));
    }
    s
}

// GUARDS: (1) `file_diff` + diff-line colorization (stacked/side-by-side) stay roughly linear in the
// diff size, and (2) opening + rendering every git view (hub / log / graph / branches / a large
// diff) stays within a generous, deterministic ceiling. Covers git.rs (statuses / file_diff / log /
// graph / branches) + preview/gitdiff.rs + ui/git.rs + the diff render in ui/preview.rs. Converted
// from wall-clock bounds to **deterministic allocation** checks — (1) an allocation-scaling ratio
// (same technique as the markdown/highlight guards above: doubling the changed lines should roughly
// double the allocation, not quadruple it), (2) a generous absolute allocation ceiling (bytes
// allocated are CPU/load-independent, unlike `Instant`, so this doesn't need CI-noise headroom).
// The fixture shrank (8 commits/3000 lines → 3 commits/2000 lines) since the guard no longer needs a
// slow operation to be *measurable* — only correctness-of-scaling matters, not absolute size.
#[cfg(feature = "git")]
#[test]
fn git_views_and_large_diff_render_is_bounded() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    let dir = crate::mem_tests::unique_tmp("konoma_speed_git_views");
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
    // A little history (log/graph exercise the DAG walk).
    for c in 0..3 {
        std::fs::write(dir.join(format!("f{c}.txt")), format!("v{c}\n")).unwrap();
        git(&["add", "-A"]);
        git(&["commit", "-qm", &format!("commit {c}")]);
    }
    // Commit a **tiny** baseline for big.txt: `diff_alloc_for` below replaces its whole content, and
    // if the committed baseline were itself large, the diff's removed-line count would stay ~fixed
    // (dominated by the baseline) while only the added-line count grew with `n` — diluting the
    // small-vs-large ratio below regardless of any real regression (confirmed while calibrating: with
    // a 2000-line baseline, `n=500` vs `n=1000` produced diffs of 2500 vs 3000 total lines — only a
    // 1.2x change, not 2x, because ~2000 of each diff was the constant removed baseline).
    std::fs::write(dir.join("big.txt"), diff_body(5, "line")).unwrap();
    git(&["add", "big.txt"]);
    git(&["commit", "-qm", "add big"]);

    let root = dir.canonicalize().unwrap();

    // (1) file_diff + diff_lines + diff_lines_side_by_side scale near-linearly with the diff size.
    let diff_alloc_for = |n: usize| -> u64 {
        std::fs::write(dir.join("big.txt"), diff_body(n, "LINE changed")).unwrap();
        crate::mem_tests::allocated_by(|| {
            let dl = crate::git::file_diff(&root, &root.join("big.txt"));
            assert!(!dl.is_empty(), "差分が取れる");
            let lines = crate::preview::gitdiff::diff_lines(&dl, "txt", "TwoDark", 90);
            assert!(!lines.is_empty(), "縦 diff 行を生成");
            let _maxh = crate::preview::gitdiff::side_by_side_max_hscroll(&dl, 90);
            let sbs =
                crate::preview::gitdiff::diff_lines_side_by_side(&dl, "txt", "TwoDark", 90, 40);
            assert!(!sbs.is_empty(), "横並び diff 行を生成");
        })
    };
    let small_alloc = diff_alloc_for(500);
    let large_alloc = diff_alloc_for(1000); // 2x changed lines
    assert!(
        large_alloc < small_alloc.saturating_mul(5) / 2, // 2.5x margin (measured ratio ~1.9x)
        "2倍の差分行数で確保バイト数が2.5倍を超えた(回帰: O(n^2)?): small={small_alloc} large={large_alloc}"
    );

    // (2) Open and render each git view (hub / log / graph / branches / the large diff left by the
    // last diff_alloc_for(1000) call above) within a generous absolute allocation ceiling. Each
    // sub-view is **closed** (mirroring the real `q`/Esc navigation back to the hub) before the next
    // is opened: the render dispatcher (ui/mod.rs) picks the view to draw by priority
    // (detail > graph_picker > **graph** > log > branches > worktrees > hub > preview/tree) purely
    // from which `tab.git_*` fields are `Some` — `open_git_branches`/`open_git_diff` don't clear
    // `tab.git_graph`, so without closing first, `is_git_graph()` stays true and every subsequent
    // render keeps drawing the graph instead of what was actually just opened (confirmed while
    // calibrating: without the `close_git_*` calls, `git_diff_lines` — and so the diff render this
    // guard is meant to exercise — was never even called).
    let mut app = crate::app::App::new(root.clone(), Config::default()).unwrap();
    let mut term = Terminal::new(TestBackend::new(100, 40)).unwrap();
    let views_alloc = crate::mem_tests::allocated_by(|| {
        app.open_git_view();
        term.draw(|f| crate::ui::render(f, &mut app)).unwrap();
        app.open_git_log();
        term.draw(|f| crate::ui::render(f, &mut app)).unwrap();
        app.close_git_log();
        app.open_git_graph();
        term.draw(|f| crate::ui::render(f, &mut app)).unwrap();
        app.close_git_graph();
        app.open_git_branches();
        term.draw(|f| crate::ui::render(f, &mut app)).unwrap();
        app.close_git_branches();
        app.open_git_diff(&root.join("big.txt"));
        term.draw(|f| crate::ui::render(f, &mut app)).unwrap();
    });
    assert!(
        views_alloc < 60_000_000, // measured ~23MB (5 real git-view opens/closes + a 1005-line diff); ~2.6x headroom
        "git ビュー群+大きな diff の描画の確保バイト数が上限を超えた(回帰?): {views_alloc}"
    );
    std::fs::remove_dir_all(&dir).ok();
}

// GUARDS: cycling through tabs (save_active + load_active + preview rebuild + render) stays bounded,
// and — the primary, stronger guard — `load_active` moves each tab's `PerTab` out of its slot via
// `std::mem::take` rather than cloning it (a regression to `.clone()` would double the per-switch cost
// and leave a stale copy sitting in `self.tabs`).
//
// Two checks, deliberately different in character:
//   1. A **structural** per-cycle assertion (`active_tab_slot_is_vacated_for_test`): right after each
//      `tab_cycle`, the active tab's own slot in `self.tabs` must equal `PerTab::default()` — the
//      direct, byte-for-byte signature of `mem::take` having fired (a `.clone()` leaves the slot's
//      prior, non-empty contents untouched). This is a plain boolean read of live state, not a byte
//      count, so it's immune to allocator/platform/CI-load noise — it's the check this test actually
//      relies on to catch a `.clone()` regression.
//   2. A **loose** allocation ceiling (20 cycles < 2.0x the cost of 20 from-scratch tree rebuilds),
//      kept as a coarse net for a *different* class of regression: some large allocation added to the
//      per-cycle path that check 1 wouldn't notice (it only inspects one boolean, not overall cost).
//
// Why not a tight allocation ratio, as used elsewhere in this file? This guard used to assert
// `cycle_alloc < rebuild_alloc * 1.45`, measured (byte-exact, deterministic) at 1.31x running this test
// alone. But **inside the full suite** — other tests' allocations leaving the allocator/caches in a
// different state, plus (measured on Linux CI vs. macOS) a shorter `/tmp/…` vs. `/var/folders/…/T/…`
// temp path, which this fixture's 3000 entries × 20 cycles' worth of `PathBuf`s makes non-negligible —
// the measured ratio drifted to 1.410–1.435 across repeated full-suite runs: within 1% of the 1.45
// ceiling. A `.clone()` regression measures 1.60x. A *loose* bound wide enough to absorb that
// full-suite drift (2.0x, check 2 above) can no longer tell "normal" from "regressed" by ratio alone
// (1.60 < 2.0 — it would not catch it). Hence check 1: it asserts the invariant `load_active` is
// actually supposed to uphold, directly, instead of inferring it from a byte count that also moves for
// reasons unrelated to take-vs-clone. A test that only ever flakes with the environment — without
// having caught the regression it was written for — is worse than no test.
//
// The fixture is still deliberately shaped so a `.clone()` regression is *visible* in check 2's loose
// net too, and so both checks exercise real work: every switch already does real, unavoidable work
// regardless of take-vs-clone (`refresh_fs_after_tab_switch` rebuilds `tab.entries` from disk every
// time — see its doc comment — so a small fixture with only a handful of tree entries would bury a
// clone regression's cost under that dominant, constant "reparse small files" work, as confirmed while
// calibrating: with only 4 top-level files, doubling `load_active`'s `mem::take` into an *extra* clone
// barely moved the total). Giving the directory **thousands of entries** (while keeping the previewed
// files themselves small) makes `entries`/`filter_pool` the dominant cost carried in `PerTab`, so an
// accidental extra clone of it shows up clearly in check 2: it's compared against `rebuild_tree`'s own
// allocation on the same directory, measured directly as the reference for "how much a from-scratch
// tree rebuild alone costs".
#[test]
fn tab_switch_reloads_are_bounded() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    let dir = crate::mem_tests::unique_tmp("konoma_speed_tabswitch");
    std::fs::create_dir_all(&dir).unwrap();
    for i in 0..3000 {
        std::fs::write(dir.join(format!("f{i:05}.txt")), b"x").unwrap();
    }
    std::fs::write(dir.join("a.md"), "# T\n\nbody [l](https://e/1)\n").unwrap();
    std::fs::write(dir.join("b.csv"), "x,y,z\n1,b,c\n2,b,c\n").unwrap();
    std::fs::write(dir.join("c.txt"), rust_source(20)).unwrap();
    std::fs::write(dir.join("d.rs"), rust_source(20)).unwrap();

    // Warm the "rust" grammar outside the measurement: `d.rs`'s windowed preview highlights via the
    // same process-global, one-time-per-language syntect compile as the guards above, whose cost
    // would otherwise land unpredictably depending on **test execution order** (see
    // `preview_large_markdown_is_bounded`'s doc comment for the concrete flake this caused there).
    let _ = crate::preview::code::highlight_lang(&rust_source(1), "rust", "TwoDark");

    let mut app = crate::app::App::new(dir.clone(), Config::default()).unwrap();

    // Reference: the allocation of a from-scratch `rebuild_tree()` on this same 3000-entry directory
    // — the "real work" every switch must pay regardless of take-vs-clone correctness.
    let rebuild_alloc = crate::mem_tests::allocated_by(|| {
        app.rebuild_tree().unwrap();
    });
    assert_eq!(app.tab.entries.len(), 3004, "全件ツリー化(3000+4ファイル)");

    let mut term = Terminal::new(TestBackend::new(80, 30)).unwrap();
    // Open each small file in a separate tab (switching between them rebuilds a different preview cache each time).
    for name in ["a.md", "b.csv", "c.txt", "d.rs"] {
        app.tab.selected = app
            .tab
            .entries
            .iter()
            .position(|e| e.path.ends_with(name))
            .unwrap();
        app.tab_new_from_selection().unwrap();
        // `selection` (the multi-select set) is carried by take/clone but, unlike `entries`, is
        // **not** touched by `refresh_fs_after_tab_switch` beyond a cheap `retain` (a stat() per
        // still-selected path — see bookmark_actions.rs) — it is never rebuilt from scratch. Marking
        // (almost) every entry selected in every tab makes it the dominant carried payload, so an
        // accidental extra clone of it (instead of `mem::take`) isn't diluted by the also-real,
        // also-unavoidable `entries` rebuild the way a plain "many tree entries" fixture alone would
        // be (confirmed while calibrating: with only `entries` made large, doubling `load_active`'s
        // `mem::take` into an extra clone moved the cycle/rebuild ratio only 1.18x → 1.34x — `entries`
        // itself dominates via its own rebuild cost regardless of clone-vs-take, diluting the signal).
        // This still matters even though check 1 (the structural assertion) is the primary regression
        // signal now: it's what keeps check 2's loose allocation ceiling a meaningful secondary net
        // rather than one so diluted by `entries`' own unavoidable rebuild cost that it couldn't catch
        // *any* clone-shaped regression.
        app.enter_visual();
        app.visual_select_scope(true);
        term.draw(|f| crate::ui::render(f, &mut app)).unwrap();
    }
    assert!(app.tab_count() >= 4, "4 つ以上のタブ");

    // Cycle through tabs 20 times: each switch = save_active (1 PerTab clone) + load_active
    // (mem::take) + tree rebuild + selection retain + small preview rebuild + render. Check 1 (see the
    // GUARDS comment above the test): right after *every* `tab_cycle`, the active tab's own slot must
    // already be back to `PerTab::default()` — the structural signature of `mem::take`, checked
    // deterministically instead of inferred from a byte count.
    let cycle_alloc = crate::mem_tests::allocated_by(|| {
        for _ in 0..20 {
            app.tab_cycle(1);
            term.draw(|f| crate::ui::render(f, &mut app)).unwrap();
            assert!(
                app.active_tab_slot_is_vacated_for_test(),
                "tab_cycle 直後、アクティブタブの self.tabs スロットが既定値(空)に戻っていない = load_active が mem::take でなく clone している疑い"
            );
        }
    });
    // Check 2: a loose net (see the GUARDS comment above the test for why 1.45x was too tight to
    // survive full-suite noise, and why 2.0x is still narrow enough to catch a large unrelated
    // allocation added to this path — it's just not narrow enough, alone, to distinguish a
    // `mem::take`-vs-`.clone()` regression, which check 1 above is what actually catches).
    assert!(
        cycle_alloc < rebuild_alloc.saturating_mul(40), // 20 * 2.0
        "20 回のタブ切替の確保バイト数がツリー再構築だけの20倍換算の2.0倍を超えた(回帰: 想定外の大きな確保が追加された?): cycle={cycle_alloc} rebuild_x40={}",
        rebuild_alloc.saturating_mul(40)
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
