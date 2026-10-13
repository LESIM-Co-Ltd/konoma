//! Test-only seams of the VCS core, compiled for this crate's own tests and, with the
//! `test-hooks` feature, for the host's tests (which count backend calls and build repositories).

use std::cell::Cell;

thread_local! {
    /// How many times `vcs::base_contents` ran **on this thread** — the call this measures is a
    /// real backend invocation (a `jj log` + `jj file show` subprocess pair under jj, ~20-25ms), so
    /// a caller that reaches it more than once per logical "what did the committed version look
    /// like" question is doing real, avoidable work. Since `docs/STATUS.md` ★未修正 item 4, the
    /// only production caller is `App::compute_md_diff` (`src/app/md_diff.rs`), which runs on a
    /// background worker thread — a test attaching `md_diff_tx` and driving the UI thread through
    /// `count_base_contents_calls` should therefore see **zero** calls on the UI thread's own copy
    /// of this counter (the worker's calls land on its own thread's copy instead). Thread-local for
    /// the same reason `STAT_CALLS` is.
    static BASE_CONTENTS_CALLS: Cell<usize> = const { Cell::new(0) };
}

/// Called by `vcs::base_contents` right before it dispatches to a backend. Cheap enough to be
/// unconditional in test builds; compiled out entirely otherwise (the call site is `#[cfg(test)]`).
pub fn note_base_contents_call() {
    let _ = BASE_CONTENTS_CALLS.try_with(|c| c.set(c.get() + 1));
}

/// Runs `f` and returns `(its value, how many times base_contents ran while it did)`. Only ever
/// called from `#[cfg(feature = "git")]` tests (`vcs::base_contents` itself only does real work on
/// a `git`-feature build) — unused, not unreachable, on a no-`git` test build.
pub fn count_base_contents_calls<T>(f: impl FnOnce() -> T) -> (T, usize) {
    let before = BASE_CONTENTS_CALLS.with(|c| c.get());
    let out = f();
    (
        out,
        BASE_CONTENTS_CALLS.with(|c| c.get()).saturating_sub(before),
    )
}

thread_local! {
    /// How many times `vcs::file_diff` ran **on this thread** — the unified line-diff (a real
    /// backend invocation, distinct from `base_contents`/`diff_align`'s block-diff machinery above).
    /// `App::open_git_diff_with`'s own `seeded_diff` exists precisely so a diff the caller already
    /// computed (`follow_jump`'s own `compute_gitdiff_lines`, `diff_jump_changed`'s `n`/`N`) is
    /// reused rather than recomputed the moment `App::git_diff_lines()` is first asked for it —
    /// this counter is what `count_file_diff_calls` uses to pin that reuse. Thread-local for the
    /// same reason `STAT_CALLS` is.
    static FILE_DIFF_CALLS: Cell<usize> = const { Cell::new(0) };
}

/// Called by `vcs::file_diff` right before it dispatches to a backend. Cheap enough to be
/// unconditional in test builds; compiled out entirely otherwise (the call site is `#[cfg(test)]`).
pub fn note_file_diff_call() {
    let _ = FILE_DIFF_CALLS.try_with(|c| c.set(c.get() + 1));
}

/// Runs `f` and returns `(its value, how many times file_diff ran while it did)`. Only ever called
/// from `#[cfg(feature = "git")]` tests (`vcs::file_diff` itself only does real work on a
/// `git`-feature build) — unused, not unreachable, on a no-`git` test build.
pub fn count_file_diff_calls<T>(f: impl FnOnce() -> T) -> (T, usize) {
    let before = FILE_DIFF_CALLS.with(|c| c.get());
    let out = f();
    (
        out,
        FILE_DIFF_CALLS.with(|c| c.get()).saturating_sub(before),
    )
}

/// Initializes a bare git repository at `dir` via `git2` (no commits) with a fixed, sandboxed
/// identity (`user.name`/`user.email`) and `commit.gpgsign` forced off, so a test-machine's real
/// gpg-signing configuration (or lack of one) never affects a commit a test makes. `dir` must
/// already exist. Distinct from `app::follow::tests::init_git_repo`, a shell (`git init` +
/// `git config`) variant that fixture also needs — kept separate rather than folded in here (see
/// that module for why).
#[cfg(feature = "git")]
pub fn init_git_repo(dir: &std::path::Path) {
    let repo = git2::Repository::init(dir).unwrap();
    let mut cfg = repo.config().unwrap();
    cfg.set_str("user.name", "Test").unwrap();
    cfg.set_str("user.email", "test@example.com").unwrap();
    cfg.set_str("commit.gpgsign", "false").ok();
}
