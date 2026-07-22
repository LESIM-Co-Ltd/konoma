//! Memory smoke-regression guards + a thread-local counting allocator (test-only).
//!
//! Two complementary techniques, both **deterministic** (no flaky RSS sampling):
//!   1. A `#[cfg(test)] #[global_allocator]` that counts bytes allocated **per thread**. Because each
//!      test runs on its own thread, `allocated_by(|| …)` measures exactly what an operation allocated,
//!      unaffected by other tests running in parallel. We count cumulative allocation (a good proxy for
//!      "how much work did this do"), not live/peak — enough to catch "re-decorate every frame" or
//!      "load the whole file" regressions where the number jumps by orders of magnitude.
//!   2. Plain size / cap assertions: cloned-per-tab structs must not balloon; bounded caches must honor
//!      their cap (table rows, GIF byte budget).
//!
//! Bounds are **relative** where possible (scroll-cost ≪ decorate-cost) so they don't flake on
//! allocator or platform differences; absolute bounds are generous.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::path::Path;

thread_local! {
    /// Cumulative bytes allocated on the current thread (const-init so touching it never allocates,
    /// which would recurse through the allocator).
    static THREAD_ALLOCATED: Cell<u64> = const { Cell::new(0) };
}

struct CountingAlloc;

// SAFETY: delegates every real allocation to the System allocator; the only added work is a
// thread-local counter bump on success. The default `realloc` / `alloc_zeroed` route through `alloc`,
// so growth (Vec reallocation) is counted too. `try_with` tolerates thread teardown (returns Err).
unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let p = System.alloc(layout);
        if !p.is_null() {
            let _ =
                THREAD_ALLOCATED.try_with(|c| c.set(c.get().wrapping_add(layout.size() as u64)));
        }
        p
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        System.dealloc(ptr, layout);
    }
}

#[global_allocator]
static GLOBAL: CountingAlloc = CountingAlloc;

/// Bytes allocated on this thread while running `f`.
fn allocated_by(f: impl FnOnce()) -> u64 {
    let before = THREAD_ALLOCATED.with(|c| c.get());
    f();
    THREAD_ALLOCATED.with(|c| c.get()).wrapping_sub(before)
}

// ---------------------------------------------------------------------------------------------
// Calibration: prints sizes and allocation costs. Run with:
//   cargo test --features git -- --ignored mem_calibration --nocapture
// Not a guard (no assertions beyond sanity) — used to pick the bounds below.
#[test]
#[ignore]
fn mem_calibration() {
    use crate::config::Config;
    eprintln!("size_of App        = {}", crate::app::sizeof_app());
    eprintln!("size_of TabState   = {}", crate::app::sizeof_tabstate());
    eprintln!("size_of Config     = {}", std::mem::size_of::<Config>());

    // md スクロールの装飾コスト vs 再スクロールコスト(キャッシュ再利用の証明用)。
    let dir = std::env::temp_dir().join("konoma_mem_calib_md");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut md = String::from("# Top\n\n");
    for i in 0..300 {
        md.push_str(&format!(
            "## H {i}\n\nBody *x* `c` [l](https://e/{i}).\n\n- [ ] t\n\n"
        ));
    }
    std::fs::write(dir.join("big.md"), &md).unwrap();

    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    let mut app = crate::app::App::new(dir.clone(), Config::default()).unwrap();
    app.selected = app
        .entries
        .iter()
        .position(|e| e.path.ends_with("big.md"))
        .unwrap();
    app.tree_activate().unwrap();
    let mut term = Terminal::new(TestBackend::new(80, 30)).unwrap();
    let first_draw = allocated_by(|| {
        term.draw(|f| crate::ui::render(f, &mut app)).unwrap(); // 装飾キャッシュ構築
    });
    let one_scroll = allocated_by(|| {
        app.preview_scroll(3);
        term.draw(|f| crate::ui::render(f, &mut app)).unwrap();
    });
    eprintln!("md first-draw alloc= {first_draw}");
    eprintln!(
        "md one-scroll alloc= {one_scroll}  (ratio {:.2})",
        one_scroll as f64 / first_draw as f64
    );

    // 巨大ファイルのプレビュー: ファイルサイズに比例するか(比例=O(file)=windowed 違反)。
    let open_alloc = |lines: usize| -> (u64, u64) {
        let mut body = String::new();
        for i in 0..lines {
            body.push_str(&format!("line {i} with some content here\n"));
        }
        let f = dir.join(format!("huge_{lines}.txt"));
        std::fs::write(&f, &body).unwrap();
        let mut a = crate::app::App::new(dir.clone(), Config::default()).unwrap();
        a.selected = a.entries.iter().position(|e| e.path == f).unwrap();
        let mut t2 = Terminal::new(TestBackend::new(80, 30)).unwrap();
        let alloc = allocated_by(|| {
            a.tree_activate().unwrap();
            t2.draw(|f| crate::ui::render(f, &mut a)).unwrap();
        });
        (body.len() as u64, alloc)
    };
    let (sz1, al1) = open_alloc(50_000);
    let (sz2, al2) = open_alloc(200_000); // 4x の行数
    eprintln!("huge 50k : size={sz1} open_alloc={al1}");
    eprintln!(
        "huge 200k: size={sz2} open_alloc={al2}  (alloc ratio {:.2}, size ratio {:.2})",
        al2 as f64 / al1 as f64,
        sz2 as f64 / sz1 as f64
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// Build an App previewing `name` in `dir`, rendered once (caches built), returning the App + Terminal.
#[cfg(test)]
fn mem_preview(
    dir: &Path,
    name: &str,
) -> (
    crate::app::App,
    ratatui::Terminal<ratatui::backend::TestBackend>,
) {
    use crate::config::Config;
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
    (app, term)
}

// GUARDS: scrolling a decorated Markdown preview reuses the MdCache — a later scroll allocates a tiny
// fraction of the initial whole-document decoration. A regression that re-decorates every frame would
// make the per-scroll allocation ≈ the decoration (measured: decoration ~8.7MB, one scroll ~30KB).
#[test]
fn md_scroll_reuses_cache_not_redecorate() {
    let dir = std::env::temp_dir().join("konoma_mem_md_reuse");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut md = String::from("# Top\n\n");
    for i in 0..300 {
        md.push_str(&format!(
            "## H {i}\n\nBody *x* `c` [l](https://e/{i}).\n\n- [ ] t\n\n"
        ));
    }
    std::fs::write(dir.join("big.md"), &md).unwrap();

    use crate::config::Config;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    let mut app = crate::app::App::new(dir.clone(), Config::default()).unwrap();
    app.selected = app
        .entries
        .iter()
        .position(|e| e.path.ends_with("big.md"))
        .unwrap();
    app.tree_activate().unwrap();
    let mut term = Terminal::new(TestBackend::new(80, 30)).unwrap();
    let first_draw = allocated_by(|| {
        term.draw(|f| crate::ui::render(f, &mut app)).unwrap();
    });
    let one_scroll = allocated_by(|| {
        app.preview_scroll(3);
        term.draw(|f| crate::ui::render(f, &mut app)).unwrap();
    });
    assert!(
        one_scroll.saturating_mul(20) < first_draw,
        "スクロールが装飾キャッシュを再利用していない(1スクロール {one_scroll} が初回装飾 {first_draw} に近い=毎フレーム再装飾?)"
    );
    std::fs::remove_dir_all(&dir).ok();
}

// GUARDS: a windowed text preview reads only the visible window — a 16x-larger file does NOT allocate
// ~16x more on open (it stays windowed, not "load the whole file"). Warm up first so the one-time
// syntect grammar / theme init (process-global) is not attributed to the measured opens.
#[test]
fn windowed_preview_does_not_scale_with_file_size() {
    let dir = std::env::temp_dir().join("konoma_mem_window_scale");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let write_lines = |name: &str, n: usize| {
        let mut b = String::new();
        for i in 0..n {
            b.push_str(&format!("line {i} with some content here\n"));
        }
        std::fs::write(dir.join(name), b).unwrap();
    };
    write_lines("warm.txt", 500);
    write_lines("small.txt", 5_000);
    write_lines("large.txt", 80_000); // 16x small

    // ウォームアップ(初回 syntect/テーマ init を計測から除外)。
    let _ = mem_preview(&dir, "warm.txt");

    let small_alloc = allocated_by(|| {
        let _ = mem_preview(&dir, "small.txt");
    });
    let large_alloc = allocated_by(|| {
        let _ = mem_preview(&dir, "large.txt");
    });
    assert!(
        large_alloc < small_alloc.saturating_mul(3),
        "windowed プレビューがファイルサイズに比例している(16x のファイルで alloc {large_alloc} が小 {small_alloc} の 3x 超=全文読み込み?)"
    );
    std::fs::remove_dir_all(&dir).ok();
}

// GUARDS: cloned-per-tab / core structs stay small. TabState is cloned on every tab snapshot/restore,
// so a large buffer accidentally inlined into it multiplies memory by the tab count; App is held once
// but a runaway struct is still a smell. Generous bounds (measured: App ~4.6KB, TabState ~1KB).
#[test]
fn per_tab_and_app_structs_stay_small() {
    let app = crate::app::sizeof_app();
    let tab = crate::app::sizeof_tabstate();
    assert!(app < 16_384, "App が肥大した: {app} バイト");
    assert!(
        tab < 4_096,
        "TabState が肥大した(タブ数ぶん倍増する): {tab} バイト"
    );
}

// GUARDS: the CSV/TSV parser caps at MAX_ROWS so a pathological huge table cannot exhaust memory
// (the rest is dropped and `truncated` is set). Complements the app-level table tests.
#[test]
fn table_rows_are_capped() {
    let dir = std::env::temp_dir().join("konoma_mem_table_cap");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("huge.csv");
    let mut b = String::from("a,b,c\n");
    for i in 0..(crate::preview::table::MAX_ROWS + 5_000) {
        b.push_str(&format!("{i},x,y\n"));
    }
    std::fs::write(&f, b).unwrap();
    let td = crate::preview::table::parse(&f, b',').unwrap();
    assert!(
        td.nrows() <= crate::preview::table::MAX_ROWS,
        "行数が MAX_ROWS を超えた: {}",
        td.nrows()
    );
    assert!(td.truncated, "上限超過は truncated=true で通知");
    std::fs::remove_dir_all(&dir).ok();
}
