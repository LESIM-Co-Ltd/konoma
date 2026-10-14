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

use std::alloc::{GlobalAlloc, Layout};
use std::cell::Cell;
use std::path::Path;

use crate::test_support::unique_tmp;

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
        let p = crate::preview::svg_proc::GuardAlloc.alloc(layout);
        if !p.is_null() {
            let _ =
                THREAD_ALLOCATED.try_with(|c| c.set(c.get().wrapping_add(layout.size() as u64)));
        }
        p
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        crate::preview::svg_proc::GuardAlloc.dealloc(ptr, layout);
    }
}

#[global_allocator]
static GLOBAL: CountingAlloc = CountingAlloc;

/// Bytes allocated on this thread while running `f`. `pub(crate)` so other `#[cfg(test)]` modules
/// (`speed_tests`, `preview::code`'s tests) can replace a wall-clock bound with a deterministic
/// byte-count one — allocation is a CPU/load-independent proxy for "how much work happened", so it
/// doesn't flake under shared-CI-runner contention the way `Instant`-based bounds do.
pub(crate) fn allocated_by(f: impl FnOnce()) -> u64 {
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
    eprintln!("size_of PerTab     = {}", crate::app::sizeof_pertab());
    eprintln!("size_of Config     = {}", std::mem::size_of::<Config>());

    // md scroll's decoration cost vs re-scroll cost (to prove cache reuse).
    let dir = unique_tmp("konoma_mem_calib_md");
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
    let mut app = crate::app::App::new(dir.to_path_buf(), Config::default()).unwrap();
    app.tab.selected = app
        .tab
        .entries
        .iter()
        .position(|e| e.path.ends_with("big.md"))
        .unwrap();
    app.tree_activate().unwrap();
    let mut term = Terminal::new(TestBackend::new(80, 30)).unwrap();
    let first_draw = allocated_by(|| {
        term.draw(|f| crate::ui::render(f, &mut app)).unwrap(); // build the decoration cache
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

    // Preview of a huge file: does it scale with file size (scaling = O(file) = a windowed violation)?
    let open_alloc = |lines: usize| -> (u64, u64) {
        let mut body = String::new();
        for i in 0..lines {
            body.push_str(&format!("line {i} with some content here\n"));
        }
        let f = dir.join(format!("huge_{lines}.txt"));
        std::fs::write(&f, &body).unwrap();
        let mut a = crate::app::App::new(dir.to_path_buf(), Config::default()).unwrap();
        a.tab.selected = a.tab.entries.iter().position(|e| e.path == f).unwrap();
        let mut t2 = Terminal::new(TestBackend::new(80, 30)).unwrap();
        let alloc = allocated_by(|| {
            a.tree_activate().unwrap();
            t2.draw(|f| crate::ui::render(f, &mut a)).unwrap();
        });
        (body.len() as u64, alloc)
    };
    let (sz1, al1) = open_alloc(50_000);
    let (sz2, al2) = open_alloc(200_000); // 4x the line count
    eprintln!("huge 50k : size={sz1} open_alloc={al1}");
    eprintln!(
        "huge 200k: size={sz2} open_alloc={al2}  (alloc ratio {:.2}, size ratio {:.2})",
        al2 as f64 / al1 as f64,
        sz2 as f64 / sz1 as f64
    );
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
    app.tab.selected = app
        .tab
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
    let dir = unique_tmp("konoma_mem_md_reuse");
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
    let mut app = crate::app::App::new(dir.to_path_buf(), Config::default()).unwrap();
    app.tab.selected = app
        .tab
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
}

// GUARDS: a windowed text preview reads only the visible window — a 16x-larger file does NOT allocate
// ~16x more on open (it stays windowed, not "load the whole file"). Warm up first so the one-time
// syntect grammar / theme init (process-global) is not attributed to the measured opens.
#[test]
fn windowed_preview_does_not_scale_with_file_size() {
    let dir = unique_tmp("konoma_mem_window_scale");
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

    // Warm-up (excludes the first-time syntect/theme init from the measurement).
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
}

// GUARDS: a windowed read is bounded **per line**, not just per window. A file with no newlines
// (minified JS, a one-line JSON log) used to be materialized whole by `read_lines` —
// `MAX_LINE_BYTES` caps it and skips the remainder without storing it. Measured on the same axis
// the sibling guard above uses: double the file and the allocation must NOT double. `read_lines`
// runs inside `terminal.draw`, so this is a principle-#4 guard as much as a memory one.
// (Uncapped, the 8 MiB case allocated ~42 MB and the 16 MiB case ~84 MB = an exact 2.0x scaling.)
#[test]
fn windowed_read_is_bounded_per_line() {
    let dir = unique_tmp("konoma_mem_long_line");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // One enormous line, then ordinary ones (so `read_lines` also has to resynchronize past it).
    let one_line_file = |name: &str, mib: usize| -> std::path::PathBuf {
        let mut body = vec![b'x'; mib * 1024 * 1024];
        body.push(b'\n');
        for i in 0..60 {
            body.extend_from_slice(format!("tail {i}\n").as_bytes());
        }
        let p = dir.join(name);
        std::fs::write(&p, &body).unwrap();
        p
    };
    let small = one_line_file("small.txt", 8);
    let large = one_line_file("large.txt", 16); // exactly 2x

    let read_alloc = |p: &Path| -> u64 {
        let mut w = crate::preview::window::FileWindow::open(p).unwrap();
        allocated_by(|| {
            // 50 lines = a typical viewport height, i.e. one frame's worth of reading.
            let lines = w.read_lines(0, 50).unwrap();
            std::hint::black_box(&lines);
        })
    };
    let small_alloc = read_alloc(&small);
    let large_alloc = read_alloc(&large);

    assert!(
        large_alloc < small_alloc.saturating_mul(2),
        "read_lines の確保が行の長さに比例している(2倍のファイルで alloc {large_alloc} が {small_alloc} の2倍以上=1行を丸ごと実体化?)"
    );
    // Absolute bound too: one call must stay in the hundreds of KB regardless of the line length
    // (worst case is the per-line cap times the requested line count).
    assert!(
        large_alloc < 1024 * 1024,
        "1回の read_lines が 1MiB 以上確保した: {large_alloc} バイト"
    );

    // The same must hold for the search scan, which reads line by line as well.
    let find_alloc = |p: &Path| -> u64 {
        let mut w = crate::preview::window::FileWindow::open(p).unwrap();
        allocated_by(|| {
            let hits = w.find_all_matches("tail 5", 100).unwrap();
            std::hint::black_box(&hits);
        })
    };
    let (fs_small, fs_large) = (find_alloc(&small), find_alloc(&large));
    assert!(
        fs_large < fs_small.saturating_mul(2),
        "find_all_matches の確保が行の長さに比例している({fs_large} vs {fs_small})"
    );
}

// GUARDS: cloned-per-tab / core structs stay small. PerTab is cloned on every tab snapshot/restore,
// so a large buffer accidentally inlined into it multiplies memory by the tab count; App is held once
// but a runaway struct is still a smell. Generous bounds (measured: App ~4.6KB, PerTab ~1KB).
#[test]
fn per_tab_and_app_structs_stay_small() {
    let app = crate::app::sizeof_app();
    let tab = crate::app::sizeof_pertab();
    assert!(app < 16_384, "App が肥大した: {app} バイト");
    assert!(
        tab < 4_096,
        "PerTab が肥大した(タブ数ぶん倍増する): {tab} バイト"
    );
}

// GUARDS: the CSV/TSV parser caps at MAX_ROWS so a pathological huge table cannot exhaust memory
// (the rest is dropped and `truncated` is set). Complements the app-level table tests.
#[test]
fn table_rows_are_capped() {
    let dir = unique_tmp("konoma_mem_table_cap");
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
}

/// Resident set size of this process in KiB (`ps`), for the printed numbers of the guard below.
fn rss_kib() -> u64 {
    std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .ok()
        .and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse().ok())
        .unwrap_or(0)
}

// GUARDS: on the **worker path** (`attach_media_loader`, as `main` has it) opening big CSV files in a
// burst does not leave every old read running to the end. A plugin with no exclusive work group
// starts one thread per open; each one looks at its `Cancel` while it reads, so the reads the user
// has already left stop after a block of rows instead of each holding a whole 100,000-row grid (the
// speed and memory tests above only ever measure the on-the-spot path).
//
// Nothing here depends on the speed of the machine: every read is held at row `PAUSE_ROW` (a hook of
// the table crate that exists in tests only) until all the files are open, so every old read has
// been left by the time it looks at its `Cancel` there, and must end on that look.
#[test]
fn a_burst_of_big_csv_opens_cancels_the_stale_reads() {
    use crate::preview::table::{PauseGate, CANCEL_CHECK_ROWS};
    use std::sync::mpsc::channel;
    use std::time::{Duration, Instant};
    const FILES: usize = 16;
    const PAUSE_ROW: usize = 2 * CANCEL_CHECK_ROWS;
    let dir = unique_tmp("konoma_mem_csv_burst");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut body = String::from("a,b,c,d,e,f,g,h\n");
    for i in 0..(crate::preview::table::MAX_ROWS + 1_000) {
        body.push_str(&format!("{i},xxxxxxxx,yyyyyyyy,zzzzzzzz,{i},{i},{i},{i}\n"));
    }
    let files: Vec<_> = (0..FILES)
        .map(|i| {
            let f = dir.join(format!("big{i:02}.csv"));
            std::fs::write(&f, &body).unwrap();
            f
        })
        .collect();

    let gate = PauseGate::install(&dir, PAUSE_ROW);
    let mut app =
        crate::app::App::new(dir.to_path_buf(), crate::config::Config::default()).unwrap();
    let (tx, rx) = channel();
    app.attach_media_loader(tx);
    let rss_before = rss_kib();
    let mut peak = rss_before;
    for f in &files {
        // Select the file in the tree and open it, as Enter does; back to the tree for the next.
        app.tab.mode = crate::app::Mode::Tree;
        app.tab.selected = app.tab.entries.iter().position(|e| &e.path == f).unwrap();
        app.tree_activate().unwrap();
        peak = peak.max(rss_kib());
    }
    // All sixteen are open and the last one is the current one: let the held reads go on, and apply
    // whatever lands until every read has ended (they log when they end).
    gate.open();
    let ended = |dir: &Path| {
        let log = crate::preview::table::PARSE_LOG.lock().unwrap();
        log.iter().filter(|(p, _, _)| p.starts_with(dir)).count()
    };
    let deadline = Instant::now() + Duration::from_secs(120);
    while ended(&dir) < FILES && Instant::now() < deadline {
        if let Ok(res) = rx.recv_timeout(Duration::from_millis(20)) {
            app.apply_media(res);
            peak = peak.max(rss_kib());
        }
    }
    while let Ok(res) = rx.recv_timeout(Duration::from_millis(200)) {
        app.apply_media(res);
        peak = peak.max(rss_kib());
    }
    let log = crate::preview::table::PARSE_LOG.lock().unwrap();
    let ours: Vec<_> = log.iter().filter(|(p, _, _)| p.starts_with(&dir)).collect();
    let finished = ours.iter().filter(|(_, _, cancelled)| !cancelled).count();
    let rows: usize = ours.iter().map(|(_, n, _)| *n).sum();
    eprintln!(
        "csv burst: {} parses, {finished} ran to the end, {rows} rows read in all; \
         RSS {rss_before} KiB -> peak {peak} KiB",
        ours.len()
    );
    assert_eq!(ours.len(), FILES, "every open started a read");
    let last = files.last().unwrap();
    for (path, n, cancelled) in &ours {
        if path == last {
            assert!(
                !cancelled && *n == crate::preview::table::MAX_ROWS,
                "the current file is read to the end: {n} rows, cancelled {cancelled}"
            );
        } else {
            assert!(
                *cancelled,
                "a read the user had left ran to the end: {}",
                path.display()
            );
            assert!(
                *n <= PAUSE_ROW + CANCEL_CHECK_ROWS,
                "a left read went on {n} rows (held at {PAUSE_ROW}): {}",
                path.display()
            );
        }
    }
    assert_eq!(finished, 1, "only the current file is read to the end");
}
