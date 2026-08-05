//! Shared test-only utilities used across `src/**` test modules (`#[cfg(test)]` only — never
//! compiled into the shipped binary).
//!
//! This module exists to de-duplicate a helper that had drifted into six byte-identical (or
//! near-identical) copies across `git.rs`, `fileops.rs`, `app/tests.rs`, `mem_tests.rs`,
//! `main.rs` (as `watch_test_unique_tmp`), and `e2e_tests.rs` (inlined into `sandbox()`).

use std::path::PathBuf;

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

    /// A line "targets a specific path" (as opposed to using `temp_dir()` bare — e.g. as a
    /// `Command`'s `current_dir` when the actual target path is built and uniqued elsewhere, as
    /// `git.rs`/`e2e_tests.rs` do for a `git clone --bare`'s cwd) if it chains `.join(` or
    /// `.push(` onto it. Only those need to prove per-call uniqueness.
    fn targets_a_path(line: &str) -> bool {
        line.contains("temp_dir().join(") || line.contains(".push(")
    }

    /// Proof of per-call uniqueness for a production temp-file path: `std::process::id()`
    /// somewhere in the call (the same signal `unique_tmp` itself relies on, plus a counter). The
    /// scan allows this a few-line window after the `temp_dir()` line, not just the same line,
    /// since `format!(...)` args are commonly wrapped onto their own lines.
    const UNIQUENESS_PROOF: &str = "process::id()";
    const PROOF_WINDOW: usize = 6;

    /// Scan result: every `temp_dir()` occurrence outside `EXEMPT_FILES` that neither is a bare
    /// (non-path-targeting) reference nor has a nearby uniqueness proof.
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
                if !line.contains("temp_dir()") {
                    continue;
                }
                if !targets_a_path(line) {
                    continue; // bare reference (e.g. a Command's cwd) — nothing to collide on
                }
                let window_end = (i + PROOF_WINDOW).min(lines.len());
                let proven = lines[i..window_end]
                    .iter()
                    .any(|l| l.contains(UNIQUENESS_PROOF));
                if !proven {
                    offenders.push(format!("{rel}:{}: {}", i + 1, line.trim()));
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
