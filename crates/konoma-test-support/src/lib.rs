//! Test-only helpers shared by the root package's tests and the tests of the crates under
//! `crates/` (a dev-dependency by path only; never published).
//!
//! Moved unchanged from `src/test_support.rs` (the root keeps that module and re-exports these).

use std::path::PathBuf;

/// Owning guard for a path returned by [`unique_tmp`]: removes it (recursively, if it turned out to
/// be a directory; as a single file otherwise) when dropped, so a fixture can never outlive the test
/// that created it. Derefs to `Path`, so the overwhelming majority of call sites (`.join(...)`,
/// `&dir`, `dir.exists()`, handing `&dir` to a function expecting `impl AsRef<Path>`, ...) need no
/// change at all: the fixture simply lives exactly as long as the guard stays in scope, which for a
/// `#[test]` fn that binds it with `let dir = unique_tmp(...)` and uses `dir` for the rest of the
/// function is already "until the test returns" — correct with zero extra code.
///
/// **The one thing this type deliberately does not provide is a way to detach the path from the
/// guard by value** — no `Into<PathBuf>`, no `From<TmpDir> for PathBuf`. A helper that builds a
/// fixture with `unique_tmp` and hands the path back to its caller for further use must return the
/// `TmpDir` itself (or a struct/tuple that holds it), never just the inner path — returning a bare
/// `PathBuf` extracted from a local `TmpDir` would drop the guard (and `rm -rf` the fixture) at the
/// end of the helper, before the caller ever touches it. This is enforced by the type system: any
/// call site that needs an owned, detached `PathBuf` must call `.to_path_buf()` (available via
/// `Deref`) *while the guard is still alive*, which makes the lifetime dependency visible at the
/// call site instead of silently wrong.
pub struct TmpDir {
    path: PathBuf,
}

impl TmpDir {
    /// Mirrors `PathBuf::as_path` (`Path` itself has no `as_path` method — without this, method
    /// resolution falls off the end of the `Deref` chain and rustc's "did you mean" fallback lands on
    /// an unrelated, nightly-gated `as_str`, producing a confusing `E0658` instead of a straightforward
    /// "no method" error).
    pub fn as_path(&self) -> &std::path::Path {
        &self.path
    }

    /// A new guard for `self`'s path with a different extension (mirrors `Path::with_extension`, but
    /// returns an owned, cleanup-guarded path rather than a bare `PathBuf`). Exists for fixtures that
    /// reserve a unique base name via `unique_tmp` and then need a specific extension (for
    /// extension-based preview-rule matching) before creating anything on disk — the un-extended path
    /// `self` held is never created in those call sites, so `self`'s own `Drop` is a harmless no-op.
    pub fn with_extension(&self, ext: impl AsRef<std::ffi::OsStr>) -> TmpDir {
        TmpDir {
            path: self.path.with_extension(ext),
        }
    }

    /// Resolves this guard's path through `Path::canonicalize` (e.g. macOS's `/var` ->
    /// `/private/var` symlink) and returns a *new* guard for the resolved path — consuming `self`
    /// **without** running its `Drop` (`std::mem::forget`). The canonical path names the exact same
    /// directory on disk as `self` did, so responsibility for removing it transfers to the returned
    /// guard: running both `Drop` impls would either remove the same directory twice (racy against
    /// anything else touching it in between) or delete it out from under the caller if `self` were a
    /// temporary that dropped first. Used by fixtures that need the canonical form of their root (to
    /// compare against `App::new`'s own canonicalized `tab.root`, or to pass to `git`/`jj`).
    ///
    /// Deliberately **not** named `canonicalize`: `Path` already has that method (reachable
    /// through `Deref`, borrowing `&self` and returning a bare `PathBuf`), and giving this one the
    /// same name would make inherent-method lookup pick this by-value, `TmpDir`-returning version
    /// at *every* `some_tmp_dir.canonicalize()` call site in the whole crate — including the ~280
    /// that rely on the borrowing `Path` version and never meant to consume the guard.
    // Its only two call sites (`app::tests::git_repo_with_one_change`, `git_view::tests::
    // sandbox_with_worktree`) are both `#[cfg(feature = "git")]`, so this is unused — not
    // unreachable — on a no-`git` test build.
    pub fn into_canonical(self) -> std::io::Result<TmpDir> {
        let canon = self.path.canonicalize()?;
        std::mem::forget(self);
        Ok(TmpDir { path: canon })
    }
}

impl std::ops::Deref for TmpDir {
    type Target = std::path::Path;
    fn deref(&self) -> &std::path::Path {
        &self.path
    }
}

impl AsRef<std::path::Path> for TmpDir {
    fn as_ref(&self) -> &std::path::Path {
        &self.path
    }
}

impl AsRef<std::ffi::OsStr> for TmpDir {
    fn as_ref(&self) -> &std::ffi::OsStr {
        self.path.as_os_str()
    }
}

impl std::fmt::Debug for TmpDir {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(&self.path, f)
    }
}

impl PartialEq<std::path::Path> for TmpDir {
    fn eq(&self, other: &std::path::Path) -> bool {
        self.path == *other
    }
}

impl PartialEq<PathBuf> for TmpDir {
    fn eq(&self, other: &PathBuf) -> bool {
        self.path == *other
    }
}

// The reverse direction too (`assert_eq!(some_pathbuf, tmp_dir_guard)`) — `PartialEq` is not
// symmetric at the trait level, so both directions need their own impl.
impl PartialEq<TmpDir> for PathBuf {
    fn eq(&self, other: &TmpDir) -> bool {
        *self == other.path
    }
}

impl PartialEq<TmpDir> for std::path::Path {
    fn eq(&self, other: &TmpDir) -> bool {
        *self == other.path
    }
}

impl Drop for TmpDir {
    fn drop(&mut self) {
        // Best effort: a fixture that was never created (a test that only ever probes "missing
        // path" behaviour), already removed itself, or lost a filesystem race with a sibling
        // cleanup is not an error here — ignore it exactly like the ad-hoc `let _ =
        // std::fs::remove_dir_all(...)` calls this type replaces used to. A handful of fixtures
        // are a single file (not a directory) rather than a tree, so try both.
        if self.path.is_dir() {
            let _ = std::fs::remove_dir_all(&self.path);
        } else {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// A unique temp directory *path* per call (pid + a process-global counter), wrapped in a guard
/// that removes it when dropped (see [`TmpDir`]). Tests that build a fixture under
/// `std::env::temp_dir()` must never use a fixed name: two `cargo test` binaries (git-feature and
/// no-git-feature builds, or two concurrent CI/dev runs) sharing one machine's temp directory would
/// otherwise race on `create_dir_all` / `remove_dir_all` / file writes on the identical path — one
/// process's fixture reset could delete or overwrite another process's still-running test. The pid
/// disambiguates across processes; the counter disambiguates multiple calls with the same prefix
/// within one process (parallel test threads, or a helper called more than once in the same test).
///
/// Callers are responsible for `create_dir_all` / `File::create` / etc. as appropriate for their
/// fixture — this only reserves a unique path and guarantees its eventual cleanup.
pub fn unique_tmp(prefix: &str) -> TmpDir {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    TmpDir {
        path: std::env::temp_dir().join(format!("{prefix}_{}_{n}", std::process::id())),
    }
}

/// The repository root (`crates/konoma-test-support/../..`), for tests of the crates under
/// `crates/` that read `samples/` or write under `docs/` / `site/`: their working directory is
/// the crate's own directory, not the repository root.
pub fn repo_root() -> PathBuf {
    let here = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    here.parent()
        .and_then(std::path::Path::parent)
        .map(std::path::Path::to_path_buf)
        .unwrap_or(here)
}

/// Resolves a fixture bundled under the repo's `samples/` directory, anchored at
/// `CARGO_MANIFEST_DIR` (baked in at compile time) rather than a bare relative path — a plain
/// `Path::new("samples/…")` resolves against the test binary's **cwd**, which is only the crate
/// root by convention (`cargo test` run from elsewhere, e.g. `cd /tmp && cargo test --manifest-path
/// …`, is a real, supported invocation), so a bare relative path would silently miss the fixture
/// and silently skip every assertion in every test that used it.
///
/// Returns `None` when the fixture is legitimately absent: `samples/` is excluded from the
/// published crate (`Cargo.toml`'s `exclude`), so `cargo test` against the packaged tarball (not a
/// git checkout) has nothing to read. Says so loudly (visible with `--nocapture`, or in the
/// captured-output dump whenever the process later exits non-zero for any reason) instead of
/// quietly passing zero assertions with no trace anywhere in the test output.
pub fn sample_path_or_skip(name: &str) -> Option<PathBuf> {
    // This crate lives at `<repo>/crates/konoma-test-support`; `samples/` is at the repo root.
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let root = root
        .parent()
        .and_then(std::path::Path::parent)
        .unwrap_or(&root);
    let p = root.join("samples").join(name);
    if p.exists() {
        Some(p)
    } else {
        eprintln!(
            "SKIP: samples/{name} not found (excluded from the published crate) — this test verifies nothing this run"
        );
        None
    }
}

/// A PNG header that declares `w` x `h` with no pixel data behind it.
pub fn png_header_only(w: u32, h: u32) -> Vec<u8> {
    fn chunk(t: &[u8; 4], d: &[u8]) -> Vec<u8> {
        let mut c = Vec::new();
        c.extend_from_slice(&(d.len() as u32).to_be_bytes());
        c.extend_from_slice(t);
        c.extend_from_slice(d);
        let mut crc = crc32(t);
        crc = crc32_update(crc, d);
        c.extend_from_slice(&(crc ^ 0xffff_ffff).to_be_bytes());
        c
    }
    fn crc32(d: &[u8]) -> u32 {
        crc32_update(0xffff_ffff, d)
    }
    fn crc32_update(mut c: u32, d: &[u8]) -> u32 {
        for &b in d {
            c ^= u32::from(b);
            for _ in 0..8 {
                c = if c & 1 != 0 {
                    0xedb8_8320 ^ (c >> 1)
                } else {
                    c >> 1
                };
            }
        }
        c
    }
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    out.extend(chunk(b"IHDR", &ihdr));
    out.extend(chunk(b"IDAT", &[0x78, 0x01]));
    out
}
