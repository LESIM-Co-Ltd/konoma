//! Shared test-only utilities used across `src/**` test modules (`#[cfg(test)]` only — never
//! compiled into the shipped binary).
//!
//! This module exists to de-duplicate a helper that had drifted into six byte-identical (or
//! near-identical) copies across `git.rs`, `fileops.rs`, `app/tests.rs`, `mem_tests.rs`,
//! `main.rs` (as `watch_test_unique_tmp`), and `e2e_tests.rs` (inlined into `sandbox()`).

use std::cell::Cell;
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

/// A unique temp directory *path* per call (pid + a process-global counter). Tests that build a
/// fixture under `std::env::temp_dir()` must never use a fixed name: two `cargo test` binaries
/// (git-feature and no-git-feature builds, or two concurrent CI/dev runs) sharing one machine's
/// temp directory will otherwise race on `create_dir_all` / `remove_dir_all` / file writes on the
/// identical path — one process's fixture reset can delete or overwrite another process's
/// still-running test. The pid disambiguates across processes; the counter disambiguates
/// multiple calls with the same prefix within one process (parallel test threads, or a helper
/// called more than once in the same test).
///
/// Returns a path only — callers are responsible for `create_dir_all` / `File::create` / etc. as
/// appropriate for their fixture.
pub(crate) fn unique_tmp(prefix: &str) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("{prefix}_{}_{n}", std::process::id()))
}

// ---------------------------------------------------------------------------------------------
// Regression guard: nothing outside this module should call `std::env::temp_dir()` to build a
// *fixed*-name path ever again (that's exactly the bug this module was written to fix — see the
// module doc). Scans `src/**/*.rs`'s own text, so it catches a reintroduced fixed name wherever it
// lands, not just in the files that had one at the time this guard was written.
#[cfg(test)]
mod guard {
    use std::path::{Path, PathBuf};

    /// Files this guard does not scan at all:
    ///   - `test_support.rs` itself: the one legitimate `std::env::temp_dir()` call is
    ///     `unique_tmp`'s own implementation above.
    ///   - Three **production** (non-test) one-shot temp-file paths that are already unique on
    ///     their own terms (pid + an atomic/monotonic counter baked into the filename itself —
    ///     `konoma-pdf-…` / `konoma-cmd-…` / `konoma-vthumb-…`), just not routed through this
    ///     `#[cfg(test)]`-only helper (they run in the shipped binary, where `test_support` does
    ///     not exist). Editing these to use `unique_tmp` is explicitly out of scope for this
    ///     guard — it must never ask for it.
    const EXEMPT_FILES: &[&str] = &[
        "test_support.rs",
        "preview/pdf.rs",
        "preview/command.rs",
        "preview/video.rs",
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
    fn find_offenders(src_dir: &Path) -> (usize, Vec<String>) {
        let mut rs_files = Vec::new();
        collect_rs_files(src_dir, &mut rs_files);
        let mut files_scanned = 0usize;
        let mut offenders = Vec::new();
        for path in &rs_files {
            let rel = path
                .strip_prefix(src_dir)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            if EXEMPT_FILES.iter().any(|f| rel == *f) {
                continue;
            }
            files_scanned += 1;
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

    /// Safety valve for the scan itself: if the directory walk broke (wrong path, `src/` moved),
    /// it must fail LOUD by finding too few files — not silently scan zero and vacuously pass.
    /// `src/` currently has 60+ `.rs` files (minus the 4 exempt ones); 20 is a conservative floor.
    #[test]
    fn scan_finds_at_least_20_source_files() {
        let src_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let (files_scanned, _) = find_offenders(&src_dir);
        assert!(
            files_scanned >= 20,
            "スキャン対象が少なすぎる(安全弁): {files_scanned} 件 — src/ の探索が壊れている可能性"
        );
    }

    /// The guard itself: every `temp_dir()` call that targets a specific path, outside the exempt
    /// files, must prove its own uniqueness (or — the normal case — go through `unique_tmp`
    /// instead, which doesn't call `temp_dir()` by name at the call site at all).
    #[test]
    fn no_fixed_name_temp_dirs_outside_unique_tmp() {
        let src_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let (files_scanned, offenders) = find_offenders(&src_dir);
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
