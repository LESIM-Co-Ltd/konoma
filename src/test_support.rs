//! Shared test-only utilities used across `src/**` test modules (`#[cfg(test)]` only — never
//! compiled into the shipped binary).
//!
//! This module exists to de-duplicate a helper that had drifted into six byte-identical (or
//! near-identical) copies across `git.rs`, `fileops.rs`, `app/tests.rs`, `mem_tests.rs`,
//! `main.rs` (as `watch_test_unique_tmp`), and `e2e_tests.rs` (inlined into `sandbox()`).

use std::cell::{Cell, RefCell};
use std::path::PathBuf;

thread_local! {
    /// How many stat syscalls the instrumented directory-walk helpers (`app::stat_follow`) issued
    /// on **this thread**.
    ///
    /// Thread-local on purpose. A process-wide `AtomicUsize` would also count the calls made by
    /// every other test running in parallel, so any assertion on an exact count would pass alone
    /// and fail in a full run — the exact flake `git::STATUS_CALLS` produced before it was
    /// switched to a per-run measurement. Each test runs on its own thread, so a thread-local
    /// counter measures exactly the walk under test. Const-init so touching it never allocates.
    static STAT_CALLS: Cell<usize> = const { Cell::new(0) };
}

/// Called by the walk helpers right before they issue a stat. Cheap enough to be unconditional in
/// test builds; compiled out entirely otherwise (the call sites are `#[cfg(test)]`).
pub(crate) fn note_stat_call() {
    // `try_with` tolerates thread teardown, matching `mem_tests`' allocator counter.
    let _ = STAT_CALLS.try_with(|c| c.set(c.get() + 1));
}

/// Runs `f` and returns `(its value, how many stat syscalls the walk helpers made while it ran)`.
pub(crate) fn count_stat_calls<T>(f: impl FnOnce() -> T) -> (T, usize) {
    let before = STAT_CALLS.with(|c| c.get());
    let out = f();
    (out, STAT_CALLS.with(|c| c.get()).saturating_sub(before))
}

thread_local! {
    /// How many times `App::apply_diff_view` ran **on this thread** — `App::open_git_diff_with`'s
    /// own doc comment is the "why": before it existed, `follow_jump`/`diff_jump_changed` each
    /// opened a diff via a fresh `open_git_diff` (validating the presentation once, against the
    /// wrong scope) and then re-validated a second time once the real scope was known, and the
    /// first, wrongly-scoped validation's flash was never cleared. Thread-local for the same reason
    /// `STAT_CALLS` is (see its own doc comment) — each test runs on its own thread.
    static APPLY_DIFF_VIEW_CALLS: Cell<usize> = const { Cell::new(0) };
}

/// Called by `App::apply_diff_view` right before it does anything. Cheap enough to be
/// unconditional in test builds; compiled out entirely otherwise (the call site is `#[cfg(test)]`).
pub(crate) fn note_apply_diff_view_call() {
    let _ = APPLY_DIFF_VIEW_CALLS.try_with(|c| c.set(c.get() + 1));
}

/// Runs `f` and returns `(its value, how many times apply_diff_view ran while it did)`. Only ever
/// called from `#[cfg(feature = "git")]` tests (the presentation state this counts only ever moves
/// on a `git`-feature build) — unused, not unreachable, on a no-`git` test build.
#[cfg_attr(not(feature = "git"), allow(dead_code))]
pub(crate) fn count_apply_diff_view_calls<T>(f: impl FnOnce() -> T) -> (T, usize) {
    let before = APPLY_DIFF_VIEW_CALLS.with(|c| c.get());
    let out = f();
    (
        out,
        APPLY_DIFF_VIEW_CALLS
            .with(|c| c.get())
            .saturating_sub(before),
    )
}

// The `diff_align` call counter lives in `konoma-widgets` (the function does); the host's tests
// reach it through this path.
#[allow(unused_imports)]
pub(crate) use konoma_widgets::markdown::count_diff_align_calls;

thread_local! {
    /// How many times `config::resolve_preview_kind` ran **on this thread** — the call both
    /// `Config::resolve_preview`/`resolve_preview_with` and the media-diff worker's own request-time
    /// classification (`App::compute_media_diff`) funnel through. For an extension-less or
    /// MIME-sniffed rule this is real I/O (`infer::get_from_path` opens and reads the file), and
    /// several diff-surface predicates (`App::diff_media_active`/`diff_binary_summary_eligible`/
    /// `diff_representations`) used to call it fresh on **every frame** a media diff was on screen —
    /// `App::diff_target_kind_cache` (`app/media_diff.rs`) exists to collapse that back down to one
    /// resolve per (re)target/invalidation, which is exactly what this counter proves. Thread-local
    /// for the same reason `STAT_CALLS` is.
    static RESOLVE_PREVIEW_CALLS: Cell<usize> = const { Cell::new(0) };
}

/// Called by `config::resolve_preview_kind` right before it walks the rule list. Cheap enough to be
/// unconditional in test builds; compiled out entirely otherwise (the call site is `#[cfg(test)]`).
pub(crate) fn note_resolve_preview_call() {
    let _ = RESOLVE_PREVIEW_CALLS.try_with(|c| c.set(c.get() + 1));
}

/// Runs `f` and returns `(its value, how many times resolve_preview_kind ran while it did)`. Only
/// ever called from `#[cfg(feature = "git")]` tests (the one caller, `App::diff_target_kind_cache`'s
/// own perf test, needs `App::open_git_diff` to set up a real diff target) — unused, not
/// unreachable, on a no-`git` test build.
#[cfg_attr(not(feature = "git"), allow(dead_code))]
pub(crate) fn count_resolve_preview_calls<T>(f: impl FnOnce() -> T) -> (T, usize) {
    let before = RESOLVE_PREVIEW_CALLS.with(|c| c.get());
    let out = f();
    (
        out,
        RESOLVE_PREVIEW_CALLS
            .with(|c| c.get())
            .saturating_sub(before),
    )
}

thread_local! {
    /// How many times `App::dispatch_media_diff` actually dispatched a media-diff request (spawned a
    /// worker thread, or ran the synchronous no-`Sender` fallback) **on this thread** —
    /// `App::kick_media_diff`/`App::apply_media_diff`'s own "at most one worker in flight, latest
    /// wins" coalescing is exactly what this counter proves: a burst of `App::poll_media_diff`/
    /// invalidation calls while a worker is already busy must coalesce into the single
    /// `media_diff_queued` slot rather than each spawning its own concurrent decode. Thread-local for
    /// the same reason `STAT_CALLS` is.
    static MEDIA_DIFF_DISPATCH_CALLS: Cell<usize> = const { Cell::new(0) };
}

/// Called by `App::dispatch_media_diff` right before it builds the request. Cheap enough to be
/// unconditional in test builds; compiled out entirely otherwise (the call site is `#[cfg(test)]`).
pub(crate) fn note_media_diff_dispatch_call() {
    let _ = MEDIA_DIFF_DISPATCH_CALLS.try_with(|c| c.set(c.get() + 1));
}

/// Runs `f` and returns `(its value, how many times dispatch_media_diff actually dispatched while
/// it did)`.
pub(crate) fn count_media_diff_dispatch_calls<T>(f: impl FnOnce() -> T) -> (T, usize) {
    let before = MEDIA_DIFF_DISPATCH_CALLS.with(|c| c.get());
    let out = f();
    (
        out,
        MEDIA_DIFF_DISPATCH_CALLS
            .with(|c| c.get())
            .saturating_sub(before),
    )
}

// `TmpDir`, `unique_tmp` and `sample_path_or_skip` live in the `konoma-test-support` crate (so the
// tests of the crates under `crates/` can use them too); re-exported so every `crate::test_support::...`
// path in this package keeps working.
#[cfg(unix)]
pub(crate) use konoma_test_support::write_executable;
pub(crate) use konoma_test_support::{sample_path_or_skip, unique_tmp, TmpDir};
// Only the end-to-end tests (all `git`-gated) build PDFs by hand now that the PDF renderer's own
// tests live in `konoma-media-core`.
#[cfg(feature = "git")]
pub(crate) use konoma_test_support::build_minimal_one_page_pdf;

// The VCS core owns the backend-call counters (its `vcs::file_diff` / `base_contents` bump them) and
// the repository fixture; re-exported so every `crate::test_support::...` path keeps working.
#[cfg(feature = "git")]
pub(crate) use konoma_vcs_core::testing::init_git_repo;
#[cfg(test)]
#[cfg_attr(not(feature = "git"), allow(unused_imports))]
pub(crate) use konoma_vcs_core::testing::{count_base_contents_calls, count_file_diff_calls};

/// Runs the `git` CLI in `dir` and asserts it succeeded (panicking with `git`'s own stderr
/// otherwise) — for the handful of operations (`add`, `commit`, ...) that are simplest done via the
/// real CLI once [`init_git_repo`] has already set up the repository and identity through `git2`.
#[cfg(feature = "git")]
pub(crate) fn run_git(dir: &std::path::Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Writes a solid `w`×`h` RGB PNG to `path` — the smallest fixture an image-preview/media-diff test
/// needs (what regenerating a chart on disk looks like, for the Agent Watch scenarios; a plain
/// filled picture for media-diff fixtures). Deliberately not `#[cfg(feature = "git")]`-gated: some
/// callers (an ordinary image preview with no diff involved at all) need it without the `git`
/// feature enabled.
pub(crate) fn write_solid_png(path: &std::path::Path, w: u32, h: u32, px: [u8; 3]) {
    image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(w, h, image::Rgb(px)))
        .save(path)
        .unwrap();
}

/// Runs `jj` inside `dir`, isolated from the running machine's real jj configuration (`HOME`
/// pointed at the scratch dir itself; user identity set via env vars) — returns whether it exited
/// successfully. `pub(crate)` (not just used internally by [`jj_scratch_seeded`]): a fixture whose
/// seed content genuinely differs from `jj_scratch_seeded`'s (e.g. a Markdown file instead of
/// `a.txt`) builds on [`jj_scratch_bare`] and calls this directly for its own commit, rather than
/// re-implementing the jj-invocation boilerplate.
#[cfg(feature = "git")]
pub(crate) fn run_jj(dir: &std::path::Path, args: &[&str]) -> bool {
    std::process::Command::new("jj")
        .current_dir(dir)
        .env("HOME", dir) // never touch the running machine's own jj config
        .env("JJ_USER", "konoma test")
        .env("JJ_EMAIL", "test@example.invalid")
        .args(args)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// A throwaway jj workspace (no colocated `.git`), freshly `jj git init`'d and otherwise empty —
/// `None` when this machine has no `jj` binary or setup failed (konoma falls back to git there, so
/// a jj-specific test must skip, not fail, without one).
#[cfg(feature = "git")]
pub(crate) fn jj_scratch_bare(name: &str) -> Option<TmpDir> {
    if !crate::vcs::jj::available() {
        return None;
    }
    let dir = unique_tmp(name);
    std::fs::create_dir_all(&dir).ok()?;
    if !run_jj(&dir, &["git", "init", "--no-colocate", "."]) {
        return None;
    }
    Some(dir)
}

/// [`jj_scratch_bare`], plus a seed commit (`a.txt` = `"one\n"`) and an uncommitted working-copy
/// change on top (`a.txt` rewritten to `"two\n"`) — the shape a jj diff/status test needs.
#[cfg(feature = "git")]
pub(crate) fn jj_scratch_seeded(name: &str) -> Option<TmpDir> {
    let dir = jj_scratch_bare(name)?;
    std::fs::write(dir.join("a.txt"), b"one\n").ok()?;
    if !run_jj(&dir, &["commit", "-m", "seed"]) {
        return None;
    }
    std::fs::write(dir.join("a.txt"), b"two\n").ok()?;
    Some(dir)
}

// The test-only stand-in for the OS clipboard lives in `konoma_host_core::clipboard` (the real
// `set_clipboard` / `read_clipboard` go through it in test builds, so `cargo test` never touches
// the developer's clipboard). These wrappers keep the tests' call sites unchanged. The sink is
// thread-local: a test asserting an "empty clipboard" must call `clear_test_clipboard()` first.

/// Write side of the sink (`set_clipboard` writes here in test builds).
pub(crate) fn set_test_clipboard(text: &str) {
    konoma_host_core::clipboard::set_test_clipboard(text);
}

/// Read side of the sink (`P` / paste-jump reads here in test builds).
pub(crate) fn get_test_clipboard() -> Option<String> {
    konoma_host_core::clipboard::get_test_clipboard()
}

/// Reset the sink to "empty" (an environment where the clipboard is unavailable).
pub(crate) fn clear_test_clipboard() {
    konoma_host_core::clipboard::clear_test_clipboard();
}

/// Test-only override for the remote-image cache root (`app::cache_root()`). Call with a sandboxed
/// directory (e.g. `unique_tmp(...)`) before triggering any code path that downloads a remote
/// document image. The override itself lives with the cache-root rule in
/// `konoma_plugin::document_pictures` (thread-local there, like the clipboard sink above); without
/// one the root is computed from the environment, and the download path refuses to start (see
/// `ensure_remote_md_fetch`), so a test can never write under the real cache.
pub(crate) fn set_test_cache_root(path: PathBuf) {
    konoma_plugin::document_pictures::set_test_cache_root(path);
}

/// The override [`set_test_cache_root`] set on this thread, if any.
pub(crate) fn get_test_cache_root() -> Option<PathBuf> {
    konoma_plugin::document_pictures::get_test_cache_root()
}

thread_local! {
    /// Test-only record of every path `fileops::move_to_trash`'s test-build body has "trashed" (see
    /// that function's `#[cfg(test)]` twin, which never calls the real `trash` crate).
    ///
    /// Before that seam existed, `cargo test` on macOS sent real files to the developer's actual
    /// `~/.Trash` on every run — the same class of bug the `CLIPBOARD` seam above fixed for the
    /// system clipboard, caught by inspecting `~/.Trash`'s entry count before/after a test run. The
    /// test double removes each target from disk directly (so the "gone from the original location"
    /// invariant the existing tests already assert still holds) and records what it removed here,
    /// so a test can additionally assert *that the seam engaged* rather than only that files
    /// happened to disappear (which a bug in the double itself, or an accidental real-trash call,
    /// could equally produce).
    ///
    /// Thread-local, not process-wide, for the same reason as `CLIPBOARD`/`CACHE_ROOT`: never
    /// cleared automatically between tests reusing the same worker thread, so a test that cares
    /// about a clean slate should call `clear_test_trashed()` first, or simply check its own paths
    /// are present in the record rather than asserting its exact contents.
    static TRASHED: RefCell<Vec<PathBuf>> = const { RefCell::new(Vec::new()) };
}

/// Record one path as "trashed" by the test double. Called only from `fileops::move_to_trash`'s
/// `#[cfg(test)]` body.
pub(crate) fn record_trashed(path: PathBuf) {
    TRASHED.with(|t| t.borrow_mut().push(path));
}

/// Every path recorded via `record_trashed` on this thread so far (oldest first).
pub(crate) fn get_trashed() -> Vec<PathBuf> {
    TRASHED.with(|t| t.borrow().clone())
}

/// Reset the trashed-paths record to empty. See the thread-reuse caveat on `TRASHED` above.
pub(crate) fn clear_test_trashed() {
    TRASHED.with(|t| t.borrow_mut().clear());
}

/// A buffer as text a snapshot test can pin: the rows as the terminal shows them (a wide glyph
/// takes its cell and the next one counts as covered; trailing blanks are cut, `|` ends a row),
/// then one line per run of equally styled cells whose style is not the default. A run is
/// `row col..col fg bg mods |text|`, so a snapshot sees colors and attributes as well as glyphs.
pub(crate) fn buffer_dump(buf: &ratatui::buffer::Buffer) -> String {
    use ratatui::style::{Color, Modifier};
    use unicode_width::UnicodeWidthStr;
    let area = buf.area;
    let mut rows = String::new();
    let mut runs = String::new();
    for y in area.top()..area.bottom() {
        let mut line = String::new();
        // (start col, style key, text) of the run being built.
        let mut run: Option<(u16, (Color, Color, Modifier), String)> = None;
        let mut flush = |run: &mut Option<(u16, (Color, Color, Modifier), String)>, end: u16| {
            if let Some((start, (fg, bg, m), text)) = run.take() {
                if (fg, bg, m) != (Color::Reset, Color::Reset, Modifier::empty()) {
                    runs.push_str(&format!(
                        "r{y} c{start}..{} fg={fg:?} bg={bg:?} mod={m:?} |{}|\n",
                        end - 1,
                        text.trim_end()
                    ));
                }
            }
        };
        let mut x = area.left();
        while x < area.right() {
            let cell = &buf[(x, y)];
            let sym = cell.symbol();
            let w = sym.width().max(1) as u16;
            line.push_str(sym);
            let key = (cell.fg, cell.bg, cell.modifier);
            match &mut run {
                Some((_, k, text)) if *k == key => text.push_str(sym),
                _ => {
                    flush(&mut run, x);
                    run = Some((x, key, sym.to_string()));
                }
            }
            x += w;
        }
        flush(&mut run, x);
        rows.push_str(&format!("{y:>2}|{}|\n", line.trim_end()));
    }
    format!("{rows}--- styles ---\n{runs}")
}

// ---------------------------------------------------------------------------------------------
// Regression guard: nothing outside this module should call `std::env::temp_dir()` to build a
// *fixed*-name path ever again (that's exactly the bug this module was written to fix — see the
// module doc). Scans `src/**/*.rs`'s own text, so it catches a reintroduced fixed name wherever it
// lands, not just in the files that had one at the time this guard was written. The scan covers
// the host's `src/` and every `crates/*/src` (the code that moved out of `src/` is still code).
#[cfg(test)]
mod guard {
    use std::path::{Path, PathBuf};

    /// Files this guard does not scan (paths relative to the repository root):
    ///   - `src/test_support.rs` and `crates/konoma-test-support/src/lib.rs`: the one legitimate
    ///     `std::env::temp_dir()` call is `unique_tmp`'s own implementation (it moved into the
    ///     latter; the former re-exports it).
    ///   - Three **production** (non-test) one-shot temp-file paths that are already unique on
    ///     their own terms (pid + an atomic/monotonic counter baked into the filename itself —
    ///     `konoma-pdf-…` / `konoma-cmd-…` / `konoma-vthumb-…`), just not routed through this
    ///     `#[cfg(test)]`-only helper (they run in the shipped binary, where `test_support` does
    ///     not exist). Editing these to use `unique_tmp` is explicitly out of scope for this
    ///     guard — it must never ask for it.
    const EXEMPT_FILES: &[&str] = &[
        "src/test_support.rs",
        "crates/konoma-test-support/src/lib.rs",
        // (`pdf.rs` and `video.rs` live in `konoma-media-core` now; `src/preview/{pdf,video}.rs`
        // are one-line re-exports with nothing to scan.)
        "crates/konoma-media-core/src/pdf.rs",
        "src/preview/command.rs",
        "crates/konoma-media-core/src/video.rs",
    ];

    /// Recursively collect every `.rs` file under `dir` (plain `std::fs`, no extra dependency —
    /// mirrors the project's other self-scanning meta tests, e.g.
    /// `e2e_tests::extract_ui_config_field_names`, which read their own source text directly
    /// rather than parse with a crate).
    fn collect_rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in rd.flatten() {
            let path = entry.path();
            if path.is_dir() {
                collect_rs_files(&path, out);
            } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
                out.push(path);
            }
        }
    }

    /// Turning a bare *reference* to a temp path into a *specific, collidable* one: chaining
    /// `.join(`/`.push(` onto it (appends a fixed sub-path — the original form this guard checked),
    /// or handing it straight to a call that actually touches the filesystem at that exact name (no
    /// `.join` needed for a literal fixed name like `"/tmp/konoma_fixed"` to collide).
    const PATH_BUILDERS: [&str; 2] = [".join(", ".push("];
    const FS_MUTATORS: [&str; 8] = [
        "create_dir_all(",
        "create_dir(",
        "remove_dir_all(",
        "remove_file(",
        "File::create(",
        "fs::write(",
        "OpenOptions::new(",
        "set_current_dir(",
    ];

    fn contains_any(line: &str, needles: &[&str]) -> bool {
        needles.iter().any(|n| line.contains(n))
    }

    fn is_ident_byte(b: u8) -> bool {
        b.is_ascii_alphanumeric() || b == b'_'
    }

    /// `word` occurs in `line` as a whole identifier — not merely as a substring of a longer one
    /// (so an identifier `d` doesn't match inside `id` or `mkdir`).
    fn contains_word(line: &str, word: &str) -> bool {
        if word.is_empty() {
            return false;
        }
        let bytes = line.as_bytes();
        let mut start = 0;
        while let Some(rel) = line[start..].find(word) {
            let at = start + rel;
            let before_ok = at == 0 || !is_ident_byte(bytes[at - 1]);
            let after = at + word.len();
            let after_ok = after >= bytes.len() || !is_ident_byte(bytes[after]);
            if before_ok && after_ok {
                return true;
            }
            start = at + 1;
        }
        false
    }

    /// A line "targets a specific path" (as opposed to a bare reference — e.g. a `Command`'s
    /// `current_dir` when the actual target path is built and uniqued elsewhere, as
    /// `git.rs`/`e2e_tests.rs` do for a `git clone --bare`'s cwd) if it chains a path-builder or a
    /// filesystem mutator onto whatever risky base is on it.
    fn line_targets_a_path(line: &str) -> bool {
        contains_any(line, &PATH_BUILDERS) || contains_any(line, &FS_MUTATORS)
    }

    /// Given an identifier bound to a risky base a few lines up, does `line` use it to build or
    /// touch a specific path — either chaining `.join(`/`.push(` straight onto it, or passing it (as
    /// a whole word) to a filesystem-mutating call? Covers the case split across two statements:
    /// `let base = std::env::temp_dir(); ... base.join("fixed")` / `... base.push("fixed")` / `...
    /// std::fs::create_dir_all(&base)`.
    fn line_uses_ident_as_path(line: &str, ident: &str) -> bool {
        PATH_BUILDERS
            .iter()
            .any(|m| line.contains(&format!("{ident}{m}")))
            || (contains_any(line, &FS_MUTATORS) && contains_word(line, ident))
    }

    /// If `line` is a `let [mut] IDENT = ...` binding, return `IDENT`. Best-effort: an unrecognised
    /// binding form just means the walk below doesn't try to trace that identifier forward, which
    /// degrades to "unproven bare reference" (a miss), never to a false accusation.
    fn let_binding_ident(line: &str) -> Option<&str> {
        let after_let = line.find("let ")?;
        let mut rest = line[after_let + 4..].trim_start();
        rest = rest.strip_prefix("mut ").unwrap_or(rest).trim_start();
        let end = rest
            .find(|c: char| !(c.is_alphanumeric() || c == '_'))
            .unwrap_or(rest.len());
        if end == 0 {
            None
        } else {
            Some(&rest[..end])
        }
    }

    /// Best-effort function-boundary heuristic: bounds how far a bound identifier is traced forward,
    /// so a same-named variable in the *next*, unrelated test isn't blamed for this one's binding.
    fn is_fn_start(line: &str) -> bool {
        let mut t = line.trim_start();
        for prefix in ["pub(crate) ", "pub ", "async ", "unsafe "] {
            if let Some(s) = t.strip_prefix(prefix) {
                t = s;
            }
        }
        t.starts_with("fn ")
    }

    /// How far past a `let` binding of a risky base the scan traces the bound identifier, capped
    /// (whichever comes first) by hitting the next function signature.
    const IDENT_TRACE_WINDOW: usize = 200;

    /// Proof of per-call uniqueness for a production temp-file path: `std::process::id()`
    /// somewhere in the call (the same signal `unique_tmp` itself relies on, plus a counter). The
    /// scan allows this a few-line window after the risky-base line, not just the same line,
    /// since `format!(...)` args are commonly wrapped onto their own lines.
    const UNIQUENESS_PROOF: &str = "process::id()";
    const PROOF_WINDOW: usize = 6;

    fn has_nearby_proof(lines: &[&str], at: usize) -> bool {
        let end = (at + PROOF_WINDOW).min(lines.len());
        lines[at..end].iter().any(|l| l.contains(UNIQUENESS_PROOF))
    }

    /// Everything this guard treats as naming a filesystem temp directory without per-call
    /// uniqueness baked in at the call site: `temp_dir()` / `env::temp_dir()`, the `TMPDIR`
    /// environment variable, or a literal `"/tmp/...` path.
    fn line_has_risky_base(line: &str) -> bool {
        line.contains("temp_dir()") || line.contains("TMPDIR") || line.contains("\"/tmp/")
    }

    /// Scan result: every risky-base occurrence outside `EXEMPT_FILES` that is used to build or
    /// touch a specific path — either right there on the same line, or a few lines later via a
    /// `let`-bound identifier — without a nearby uniqueness proof.
    ///
    /// Deliberately not a full parser (a plain text scan, like the project's other self-scanning
    /// meta tests): it can miss a disguised violation, but it must never *falsely* accuse existing
    /// source, which is why every heuristic here (word-boundary identifier matching, an explicit
    /// path-builder/mutator vocabulary, a function-boundary trace cutoff) errs toward under- rather
    /// than over-detection.
    ///
    /// `src_dir` is one source directory and `label` its path relative to the repository root
    /// (`src` or `crates/<name>/src`), so the exemptions mean the same thing wherever the code
    /// lives. The first number is how many `.rs` files the walk found (exempt ones included).
    fn find_offenders(src_dir: &Path, label: &str) -> (usize, Vec<String>) {
        let mut rs_files = Vec::new();
        collect_rs_files(src_dir, &mut rs_files);
        let files_scanned = rs_files.len();
        let mut offenders = Vec::new();
        for path in &rs_files {
            let rel = format!(
                "{label}/{}",
                path.strip_prefix(src_dir)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            );
            if EXEMPT_FILES.iter().any(|f| rel == *f) {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(path) else {
                continue;
            };
            let lines: Vec<&str> = text.lines().collect();
            for (i, line) in lines.iter().enumerate() {
                if !line_has_risky_base(line) {
                    continue;
                }
                // Form 1: used to build/touch a path right here on this line (the original check,
                // now also covering a literal "/tmp/..." or TMPDIR combined with a mutator).
                if line_targets_a_path(line) {
                    if !has_nearby_proof(&lines, i) {
                        offenders.push(format!("{rel}:{}: {}", i + 1, line.trim()));
                    }
                    continue; // already flagged this line; no need to also trace it as a binding
                }
                // Form 2: bound to a variable, used a few lines later — the risky base and its use
                // split across statements (`let base = temp_dir(); ... base.join(...)`, or the same
                // shape for a literal `"/tmp/..."` path).
                let Some(ident) = let_binding_ident(line) else {
                    continue;
                };
                let mut trace_end = (i + 1 + IDENT_TRACE_WINDOW).min(lines.len());
                for (j, l) in lines.iter().enumerate().take(trace_end).skip(i + 1) {
                    if is_fn_start(l) {
                        trace_end = j;
                        break;
                    }
                }
                for (j, l) in lines.iter().enumerate().take(trace_end).skip(i + 1) {
                    if line_uses_ident_as_path(l, ident) && !has_nearby_proof(&lines, j) {
                        offenders.push(format!(
                            "{rel}:{}: {} (bound at {}: {})",
                            j + 1,
                            l.trim(),
                            i + 1,
                            line.trim()
                        ));
                        break; // one report per binding is enough
                    }
                }
            }
        }
        (files_scanned, offenders)
    }

    /// Every source directory the guard covers, as `(label relative to the repository root, path)`:
    /// the host's `src` first, then `crates/<name>/src` for each crate.
    fn source_dirs() -> Vec<(String, PathBuf)> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut dirs = vec![("src".to_string(), root.join("src"))];
        let mut crates: Vec<String> = std::fs::read_dir(root.join("crates"))
            .map(|rd| {
                rd.flatten()
                    .filter(|e| e.path().join("Cargo.toml").is_file())
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        crates.sort();
        for name in crates {
            dirs.push((
                format!("crates/{name}/src"),
                root.join("crates").join(&name).join("src"),
            ));
        }
        dirs
    }

    /// Safety valve for the scan itself: if the directory walk broke (wrong path, `src/` or
    /// `crates/` moved), it must fail LOUD by finding too few files — not silently scan zero and
    /// vacuously pass. Two independent floors, so that either search breaking is caught on its own:
    /// the host's `src/` alone has 60+ `.rs` files (20 is a conservative floor), and `crates/` must
    /// yield at least one crate, each with at least one `.rs` file.
    #[test]
    fn scan_finds_at_least_20_source_files() {
        let dirs = source_dirs();
        let (host_label, host_dir) = &dirs[0];
        let (host_files, _) = find_offenders(host_dir, host_label);
        assert!(
            host_files >= 20,
            "スキャン対象が少なすぎる(安全弁): {host_files} 件 — src/ の探索が壊れている可能性"
        );
        assert!(
            dirs.len() > 1,
            "crates/*/ が1つも見つからない(安全弁) — crates/ の探索が壊れている可能性"
        );
        for (label, dir) in &dirs[1..] {
            let (n, _) = find_offenders(dir, label);
            assert!(n >= 1, "{label} に .rs ファイルが見つからない(安全弁)");
        }
    }

    /// The guard itself: every `temp_dir()` call that targets a specific path, outside the exempt
    /// files, must prove its own uniqueness (or — the normal case — go through `unique_tmp`
    /// instead, which doesn't call `temp_dir()` by name at the call site at all).
    #[test]
    fn no_fixed_name_temp_dirs_outside_unique_tmp() {
        let dirs = source_dirs();
        let mut files_scanned = 0usize;
        let mut offenders = Vec::new();
        for (label, dir) in &dirs {
            let (n, mut o) = find_offenders(dir, label);
            files_scanned += n;
            offenders.append(&mut o);
        }
        assert!(
            files_scanned >= 20,
            "スキャン対象が少なすぎる(安全弁): {files_scanned} 件"
        );
        assert!(
            offenders.is_empty(),
            "共有ヘルパー unique_tmp を経由しない固定名 temp_dir() 呼び出しを検出\n\
             (固定名は並行実行中の2プロセスが衝突する — このモジュールの docコメント参照):\n{}",
            offenders.join("\n")
        );
    }
}
