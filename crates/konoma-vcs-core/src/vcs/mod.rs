//! Which version-control system backs a directory, and the one read surface every backend
//! implements.
//!
//! konoma draws **one** set of views (tree markers, diff, gutter, graph, hub). What changes between
//! version-control systems is where the data comes from, never how it is drawn — so a backend
//! answers questions and returns the shared types (`FileStatus`, `DiffLine`, `GraphRow`, …), and the
//! `ui` layer stays unaware of which one answered.
//!
//! `backend_for` selects Git or Jj without changing the host's views.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::git::{BranchInfo, ChangeEntry, CommitInfo, CommitMeta, DiffLine, FileStatus, GraphRow};

mod git_screen;

#[cfg(any(test, feature = "test-hooks"))]
thread_local! {
    static WATCH_DIR_CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static POINT_READ_CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(any(test, feature = "test-hooks"))]
pub fn count_point_read_calls<T>(f: impl FnOnce() -> T) -> (T, usize) {
    let before = POINT_READ_CALLS.with(|c| c.get());
    let result = f();
    (result, POINT_READ_CALLS.with(|c| c.get()) - before)
}

#[cfg(any(test, feature = "test-hooks"))]
pub fn count_watch_dir_calls<T>(f: impl FnOnce() -> T) -> (T, usize) {
    let before = WATCH_DIR_CALLS.with(|c| c.get());
    let result = f();
    (result, WATCH_DIR_CALLS.with(|c| c.get()) - before)
}

#[cfg(feature = "git")]
pub mod jj;

/// What a backend can do beyond answering questions.
///
/// konoma's views are shared, but not every action behind them exists in every system: offering one
/// that does not is worse than hiding it, because a key that is advertised gets pressed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Caps {
    /// Whether konoma can change the repository through this backend. False for jj, where every
    /// read deliberately carries `--ignore-working-copy` and nothing writes.
    pub write: bool,
}

/// The read surface a version-control backend answers.
///
/// `Send + Sync` because the status scan runs on a worker thread (`spawn_or_sync_statuses`).
///
/// Every method that takes a `root` has the same "quietly return nothing" contract as the free
/// functions in [`crate::git`]: outside a repository, on failure, or when the integration is
/// switched off, they return an empty/None answer rather than an error, so a caller never has to
/// special-case "no repository here". [`Vcs::caps`] is the one exception — it takes no `root` at
/// all, because it answers a static question about the backend already chosen for it, not about
/// any particular directory.
pub trait Vcs: Send + Sync {
    /// What this backend can do beyond reading. See [`Caps`].
    fn caps(&self) -> Caps;

    /// The repository's working directory (the directory the VCS commands run from), or None when
    /// `root` is not inside a repository.
    fn workdir(&self, root: &Path) -> Option<PathBuf>;

    /// Per-path status of everything that differs from the committed state.
    fn statuses(&self, root: &Path) -> HashMap<PathBuf, FileStatus>;

    /// Paths excluded by the repository's ignore rules. A fully ignored directory collapses to one
    /// entry rather than being recursed into.
    fn ignored(&self, root: &Path) -> HashSet<PathBuf>;

    /// Label for the chip in the tree's title (git: the current branch).
    fn branch(&self, root: &Path) -> Option<String>;

    /// For a linked worktree, the name it was created from; None for the main one.
    fn worktree_origin(&self, root: &Path) -> Option<String>;

    /// Line diff of one file against the committed state it should be compared with (git: HEAD,
    /// jj: the working-copy commit's parent). A file the committed state does not have reads as
    /// all-added.
    fn file_diff(&self, root: &Path, file: &Path) -> Vec<DiffLine>;

    /// The committed bytes of `file` that [`Vcs::file_diff`] compares the working copy against
    /// (git: HEAD's blob; jj: the working-copy commit's parent `@-`). `None` when the committed
    /// state has no such file (untracked / added since), `root` is not inside a repository, or the
    /// backend cannot answer at all — the same "quietly render blank" contract as `file_diff`.
    ///
    /// This is the state-layer half of the Markdown rendered-diff feature
    /// (`docs/FEATURE-MD-RENDERED-DIFF.md` §2): it, `file_diff`, and the follow-baseline snapshot
    /// (`App::follow_baseline_contents`) are the three ways konoma learns what a file used to look
    /// like, and this is the one a caller reaches for when it wants the *bytes* rather than an
    /// already-diffed line list — used by `App::preview_diff_baseline`/`diff_rendered_sources`
    /// (`src/app/md_render.rs`) to run their own comparison (a block-level Markdown diff) against
    /// the current content instead of git's/jj's line-level one.
    fn base_contents(&self, root: &Path, file: &Path) -> Option<Vec<u8>>;

    /// An immutable revision identifying the current state (git HEAD; jj's last snapshot of `@`).
    /// Follow separately captures dirty working files, so recording a point never writes.
    fn current_point(&self, root: &Path) -> Option<String>;

    /// Bytes at a previously recorded point, independent of later commits or working-copy edits.
    fn contents_at(&self, root: &Path, point: &str, file: &Path) -> Option<Vec<u8>>;

    /// Public repository metadata directory to watch when it lies outside the host's root watch.
    fn watch_dir(&self, root: &Path) -> Option<PathBuf>;

    /// The changed files, one entry per file, sorted by path — what the changed-file list and the
    /// jumps between changes walk.
    fn changed_files(&self, root: &Path) -> Vec<ChangeEntry>;

    /// Recent commits, newest first.
    fn log(&self, root: &Path, max: usize) -> Vec<CommitInfo>;

    /// The commit graph, already laid out into rows. `all` asks for every revision rather than the
    /// range the backend considers current.
    fn graph(
        &self,
        all: bool,
        root: &Path,
        base: Option<&str>,
        labels: &crate::git::WorktreeLabels,
        refs: Option<&[String]>,
    ) -> Vec<GraphRow>;

    /// The named pointers into history: git branches, jj bookmarks.
    fn refs(&self, root: &Path) -> Vec<BranchInfo>;

    /// The commit a named pointer points at.
    fn ref_tip(&self, root: &Path, name: &str) -> Option<String>;

    /// Detail for one revision.
    fn commit_meta(&self, root: &Path, id: &str) -> Option<CommitMeta>;

    /// Everything one revision changed, file by file.
    fn commit_diff(&self, root: &Path, id: &str) -> Vec<DiffLine>;
}

/// The git backend. It delegates to [`crate::git`], which still holds the implementation.
pub struct Git;

impl Vcs for Git {
    fn caps(&self) -> Caps {
        Caps { write: true }
    }

    fn workdir(&self, root: &Path) -> Option<PathBuf> {
        crate::git::workdir(root)
    }

    fn statuses(&self, root: &Path) -> HashMap<PathBuf, FileStatus> {
        crate::git::statuses(root)
    }

    fn ignored(&self, root: &Path) -> HashSet<PathBuf> {
        crate::git::ignored(root)
    }

    fn branch(&self, root: &Path) -> Option<String> {
        crate::git::branch(root)
    }

    fn worktree_origin(&self, root: &Path) -> Option<String> {
        crate::git::worktree_origin(root)
    }

    fn file_diff(&self, root: &Path, file: &Path) -> Vec<DiffLine> {
        crate::git::file_diff(root, file)
    }

    fn base_contents(&self, root: &Path, file: &Path) -> Option<Vec<u8>> {
        crate::git::base_contents(root, file)
    }

    fn current_point(&self, root: &Path) -> Option<String> {
        #[cfg(any(test, feature = "test-hooks"))]
        POINT_READ_CALLS.with(|c| c.set(c.get() + 1));
        #[cfg(feature = "git")]
        {
            crate::git::head_commit_id(root)
        }
        #[cfg(not(feature = "git"))]
        {
            let _ = root;
            None
        }
    }

    fn contents_at(&self, root: &Path, point: &str, file: &Path) -> Option<Vec<u8>> {
        #[cfg(any(test, feature = "test-hooks"))]
        POINT_READ_CALLS.with(|c| c.set(c.get() + 1));
        #[cfg(feature = "git")]
        {
            crate::git::blob_at(root, point, file)
        }
        #[cfg(not(feature = "git"))]
        {
            let _ = (root, point, file);
            None
        }
    }

    fn watch_dir(&self, root: &Path) -> Option<PathBuf> {
        #[cfg(any(test, feature = "test-hooks"))]
        WATCH_DIR_CALLS.with(|c| c.set(c.get() + 1));
        crate::git::git_dir(root)
    }

    fn changed_files(&self, root: &Path) -> Vec<ChangeEntry> {
        crate::git::changed_files(root)
    }

    fn log(&self, root: &Path, max: usize) -> Vec<CommitInfo> {
        crate::git::log(root, max)
    }

    fn graph(
        &self,
        _all: bool,
        root: &Path,
        base: Option<&str>,
        labels: &crate::git::WorktreeLabels,
        refs: Option<&[String]>,
    ) -> Vec<GraphRow> {
        // git's graph is already every reachable commit; there is no narrower range to widen from.
        crate::git::graph_with_base(root, base, labels, refs)
    }

    fn refs(&self, root: &Path) -> Vec<BranchInfo> {
        crate::git::branches(root)
    }

    fn ref_tip(&self, root: &Path, name: &str) -> Option<String> {
        crate::git::branch_tip(root, name)
    }

    fn commit_meta(&self, root: &Path, id: &str) -> Option<CommitMeta> {
        crate::git::commit_meta(root, id)
    }

    fn commit_diff(&self, root: &Path, id: &str) -> Vec<DiffLine> {
        crate::git::commit_diff(root, id)
    }
}

/// Which backend the user asked for, from `[project] vcs`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preference {
    /// **git wherever git can answer; jj only where it cannot.** A repository with a `.git` keeps
    /// showing what it always showed — upgrading konoma must not change the views of a repository
    /// that already worked — while one jj created without a colocated `.git`, and every
    /// `jj workspace`, gets the jj backend instead of nothing at all.
    Auto,
    /// Always git, even where a `.jj` sits beside the `.git`.
    Git,
    /// **jj wherever a `.jj` exists**, colocated or not: ask for this when jj is the system you
    /// actually work in, and git's view of your repository — a detached HEAD, an index you never
    /// stage to, a graph full of the commits jj rewrote — is describing something else.
    Jj,
}

impl Preference {
    /// Anything unrecognised means `auto`: a typo in the config should not silently take the tree's
    /// markers away.
    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "git" => Self::Git,
            "jj" => Self::Jj,
            _ => Self::Auto,
        }
    }
}

/// Process-wide, set once from the config at startup. Unlike git's own on/off switch this cannot be
/// thread-local: the status scan runs on a worker thread, and it has to detect the same backend the
/// UI thread does.
static PREFERENCE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

/// Applies `[project] vcs`.
pub fn set_preference(p: Preference) {
    let v = match p {
        Preference::Auto => 0,
        Preference::Git => 1,
        Preference::Jj => 2,
    };
    PREFERENCE.store(v, std::sync::atomic::Ordering::Relaxed);
}

#[cfg(feature = "git")]
thread_local! {
    /// Worker-local or test-local override of the preference above.
    ///
    /// The real one has to be process-wide, but that makes it unusable *from* a test: every other
    /// test's `App::new` writes it too, so a test that pinned `jj` would have it overwritten
    /// mid-run by an unrelated test starting up in parallel — measured, not theoretical. Writing
    /// the global from a test is the same hazard pointed the other way, since the pinned value
    /// then leaks into whatever else is running.
    ///
    /// `WorkerConfig` copies this selection to status/ignored/diff workers, including workers
    /// attached by tests, without changing another caller's process-wide preference.
    static PREFERENCE_OVERRIDE: std::cell::Cell<Option<Preference>> =
        const { std::cell::Cell::new(None) };
}

/// Pin the backend preference on the current thread (`None` = follow the process-wide setting).
/// Tests must reset it to `None` when done.
#[cfg(all(any(test, feature = "test-hooks"), feature = "git"))]
pub fn set_preference_for_test(p: Option<Preference>) {
    PREFERENCE_OVERRIDE.with(|c| c.set(p));
}

#[cfg_attr(not(feature = "git"), allow(dead_code))]
fn preference() -> Preference {
    #[cfg(feature = "git")]
    if let Some(p) = PREFERENCE_OVERRIDE.with(|c| c.get()) {
        return p;
    }
    match PREFERENCE.load(std::sync::atomic::Ordering::Relaxed) {
        1 => Preference::Git,
        2 => Preference::Jj,
        _ => Preference::Auto,
    }
}

/// Carry the caller's backend selection into a new worker thread. In particular, a disabled Git
/// backend must stay disabled in a colocated workspace, where `auto` then selects Jj.
#[derive(Clone, Copy)]
pub struct WorkerConfig {
    #[cfg(feature = "git")]
    preference: Preference,
    #[cfg(feature = "git")]
    git_enabled: bool,
}

impl WorkerConfig {
    pub fn capture() -> Self {
        Self {
            #[cfg(feature = "git")]
            preference: preference(),
            #[cfg(feature = "git")]
            git_enabled: crate::git::external_git_enabled(),
        }
    }

    /// Install only on a newly spawned worker; it owns these settings for its entire lifetime.
    pub fn install(self) {
        #[cfg(feature = "git")]
        {
            PREFERENCE_OVERRIDE.with(|c| c.set(Some(self.preference)));
            crate::git::set_external_git_enabled(self.git_enabled);
        }
    }
}

/// Which backend answers for a directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VcsKind {
    Git,
    /// jj (Jujutsu). See [`jj`].
    #[cfg(feature = "git")]
    Jj,
}

/// The jj backend. See [`jj`] for why every read is safe.
#[cfg(feature = "git")]
pub struct Jj;

#[cfg(feature = "git")]
impl Vcs for Jj {
    fn caps(&self) -> Caps {
        // Reading is all konoma does here, on purpose: see the module docs on `jj`.
        Caps { write: false }
    }

    fn workdir(&self, root: &Path) -> Option<PathBuf> {
        jj::workspace_root(root)
    }

    fn statuses(&self, root: &Path) -> HashMap<PathBuf, FileStatus> {
        jj::statuses(root)
    }

    fn ignored(&self, root: &Path) -> HashSet<PathBuf> {
        jj::ignored(root)
    }

    fn branch(&self, root: &Path) -> Option<String> {
        jj::branch(root)
    }

    fn worktree_origin(&self, _root: &Path) -> Option<String> {
        // jj's equivalent is `jj workspace`, which is deliberately out of scope for now.
        None
    }

    fn file_diff(&self, root: &Path, file: &Path) -> Vec<DiffLine> {
        jj::file_diff(root, file)
    }

    fn base_contents(&self, root: &Path, file: &Path) -> Option<Vec<u8>> {
        jj::base_contents(root, file)
    }

    fn current_point(&self, root: &Path) -> Option<String> {
        #[cfg(any(test, feature = "test-hooks"))]
        POINT_READ_CALLS.with(|c| c.set(c.get() + 1));
        jj::current_point(root)
    }

    fn contents_at(&self, root: &Path, point: &str, file: &Path) -> Option<Vec<u8>> {
        #[cfg(any(test, feature = "test-hooks"))]
        POINT_READ_CALLS.with(|c| c.set(c.get() + 1));
        jj::contents_at(root, point, file)
    }

    fn watch_dir(&self, root: &Path) -> Option<PathBuf> {
        #[cfg(any(test, feature = "test-hooks"))]
        WATCH_DIR_CALLS.with(|c| c.set(c.get() + 1));
        jj::watch_dir(root)
    }

    fn changed_files(&self, root: &Path) -> Vec<ChangeEntry> {
        jj::changed_files(root)
    }

    fn log(&self, root: &Path, max: usize) -> Vec<CommitInfo> {
        jj::log(root, max)
    }

    fn graph(
        &self,
        all: bool,
        root: &Path,
        _base: Option<&str>,
        _labels: &crate::git::WorktreeLabels,
        _refs: Option<&[String]>,
    ) -> Vec<GraphRow> {
        // Pinning a base branch and filtering by branch are both git-shaped questions; jj asks them
        // with a revset instead, which is its own step.
        jj::graph(root, all.then_some("all()"), 400)
    }

    fn refs(&self, root: &Path) -> Vec<BranchInfo> {
        jj::bookmarks(root)
    }

    fn ref_tip(&self, root: &Path, name: &str) -> Option<String> {
        jj::bookmark_tip(root, name)
    }

    fn commit_meta(&self, root: &Path, id: &str) -> Option<CommitMeta> {
        jj::commit_meta(root, id)
    }

    fn commit_diff(&self, root: &Path, id: &str) -> Vec<DiffLine> {
        jj::commit_diff(root, id)
    }
}

/// Pure decision function behind [`detect`]: given already-resolved inputs, decides which backend
/// answers for `root`. This precedence must never change — see the inline comment below — but the
/// logic is worth being able to drive from a test with an arbitrary combination of inputs, which is
/// why it takes them as parameters rather than reading them itself:
///
/// - [`preference()`] reads a process-wide `AtomicU8` (`PREFERENCE`) shared by every test binary
///   runs concurrently; a test that called `set_preference` to exercise one case would leak that
///   choice into every other test running in parallel.
/// - [`jj::available()`] caches its probe in a `OnceLock` for the life of the process — once it has
///   observed a working `jj` binary it can never report `false` again, so "no jj binary on this
///   machine" could never be exercised from a process where the probe already ran and succeeded.
///
/// `root` has neither problem (a fixture directory is not shared state), so it is passed straight
/// through unchanged; `detect` is a thin wrapper that supplies the two global reads.
#[cfg(feature = "git")]
pub(crate) fn detect_with(pref: Preference, root: &Path, jj_available: bool) -> VcsKind {
    if pref == Preference::Git {
        return VcsKind::Git;
    }
    // Under `auto`, git keeps whatever it can already answer; jj fills the gap where there is no
    // git repository to ask. `jj` asks for jj wherever it can answer at all.
    let git_answers = pref == Preference::Auto && crate::git::workdir(root).is_some();
    if !git_answers && jj::workspace_root(root).is_some() && jj_available {
        return VcsKind::Jj;
    }
    VcsKind::Git
}

/// Which version-control system answers for `root`. See [`Preference`] for what decides it.
///
/// git also answers where a `.jj` exists but the `jj` binary does not: falling back shows something
/// rather than nothing.
pub fn detect(root: &Path) -> VcsKind {
    #[cfg(feature = "git")]
    {
        detect_with(preference(), root, jj::available())
    }
    #[cfg(not(feature = "git"))]
    {
        let _ = root;
        VcsKind::Git
    }
}

/// The backend that answers for `root`.
pub fn backend_for(root: &Path) -> &'static dyn Vcs {
    match detect(root) {
        VcsKind::Git => &Git,
        #[cfg(feature = "git")]
        VcsKind::Jj => &Jj,
    }
}

// --- The window callers use ---------------------------------------------------------------------
// Callers ask these, never a backend directly: the dispatch stays in one place, and a call site
// reads the same as it did when there was only git.

/// Pure line comparison shared by all backends; no repository access.
pub use crate::git::diff_contents;

/// See [`Vcs::current_point`].
pub fn current_point(root: &Path) -> Option<String> {
    backend_for(root).current_point(root)
}

/// See [`Vcs::contents_at`].
pub fn contents_at(root: &Path, point: &str, file: &Path) -> Option<Vec<u8>> {
    backend_for(root).contents_at(root, point, file)
}

/// See [`Vcs::watch_dir`].
pub fn watch_dir(root: &Path) -> Option<PathBuf> {
    backend_for(root).watch_dir(root)
}

/// See [`Vcs::workdir`].
pub fn workdir(root: &Path) -> Option<PathBuf> {
    backend_for(root).workdir(root)
}

/// See [`Vcs::statuses`].
pub fn statuses(root: &Path) -> HashMap<PathBuf, FileStatus> {
    backend_for(root).statuses(root)
}

/// See [`Vcs::ignored`].
pub fn ignored(root: &Path) -> HashSet<PathBuf> {
    backend_for(root).ignored(root)
}

/// See [`Vcs::branch`].
pub fn branch(root: &Path) -> Option<String> {
    backend_for(root).branch(root)
}

/// See [`Vcs::worktree_origin`].
pub fn worktree_origin(root: &Path) -> Option<String> {
    backend_for(root).worktree_origin(root)
}

/// See [`Vcs::file_diff`].
pub fn file_diff(root: &Path, file: &Path) -> Vec<DiffLine> {
    #[cfg(any(test, feature = "test-hooks"))]
    crate::testing::note_file_diff_call();
    backend_for(root).file_diff(root, file)
}

/// See [`Vcs::base_contents`].
pub fn base_contents(root: &Path, file: &Path) -> Option<Vec<u8>> {
    #[cfg(any(test, feature = "test-hooks"))]
    crate::testing::note_base_contents_call();
    backend_for(root).base_contents(root, file)
}

/// See [`Vcs::log`].
pub fn log(root: &Path, max: usize) -> Vec<CommitInfo> {
    backend_for(root).log(root, max)
}

/// See [`Vcs::graph`].
pub fn graph(
    all: bool,
    root: &Path,
    base: Option<&str>,
    labels: &crate::git::WorktreeLabels,
    refs: Option<&[String]>,
) -> Vec<GraphRow> {
    backend_for(root).graph(all, root, base, labels, refs)
}

/// See [`Vcs::refs`].
pub fn refs(root: &Path) -> Vec<BranchInfo> {
    backend_for(root).refs(root)
}

/// See [`Vcs::ref_tip`].
pub fn ref_tip(root: &Path, name: &str) -> Option<String> {
    backend_for(root).ref_tip(root, name)
}

/// See [`Vcs::commit_meta`].
pub fn commit_meta(root: &Path, id: &str) -> Option<CommitMeta> {
    backend_for(root).commit_meta(root, id)
}

/// See [`Vcs::commit_diff`].
pub fn commit_diff(root: &Path, id: &str) -> Vec<DiffLine> {
    backend_for(root).commit_diff(root, id)
}

/// See [`Vcs::caps`].
pub fn caps(root: &Path) -> Caps {
    backend_for(root).caps()
}

/// See [`Vcs::changed_files`].
pub fn changed_files(root: &Path) -> Vec<ChangeEntry> {
    backend_for(root).changed_files(root)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "git")]
    #[test]
    fn vcs_git_point_and_contents_are_pinned() {
        if !Git::git_binary_available() {
            eprintln!("skipping: no working git binary");
            return;
        }
        let dir = konoma_test_support::unique_tmp("vcs_git_point");
        std::fs::create_dir_all(&dir).unwrap();
        let root = dir.canonicalize().unwrap();
        let repo = git2::Repository::init(&root).unwrap();
        let file = root.join("a.txt");
        let names = ["a.txt", "a [literal].txt", "資料 一覧.txt"];
        let commit = |contents: &[u8]| {
            for name in names {
                std::fs::write(root.join(name), contents).unwrap();
            }
            let mut index = repo.index().unwrap();
            for name in names {
                index.add_path(Path::new(name)).unwrap();
            }
            let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
            let sig =
                git2::Signature::new("fixture", "fixture@example.invalid", &git2::Time::new(0, 0))
                    .unwrap();
            let parent = repo.head().ok().and_then(|h| h.peel_to_commit().ok());
            repo.commit(
                Some("HEAD"),
                &sig,
                &sig,
                "fixture",
                &tree,
                &parent.iter().collect::<Vec<_>>(),
            )
            .unwrap()
        };
        assert!(Git.current_point(&root).is_none());
        let first = commit(b"before\n");
        let point = Git.current_point(&root).unwrap();
        assert_eq!(point, first.to_string());
        commit(b"after\n");
        assert_ne!(Git.current_point(&root).unwrap(), point);
        for name in names {
            assert_eq!(
                Git.contents_at(&root, &point, &root.join(name)).unwrap(),
                b"before\n",
                "{name}"
            );
        }
        assert!(Git
            .contents_at(&root, &point, &root.join("new.txt"))
            .is_none());
        assert!(Git.contents_at(&root, "invalid", &file).is_none());
        assert_eq!(Git.watch_dir(&root), Some(root.join(".git")));
    }

    #[cfg(not(feature = "git"))]
    #[test]
    fn vcs_point_capability_without_git_answers_empty() {
        let dir = konoma_test_support::unique_tmp("vcs_point_disabled");
        std::fs::create_dir_all(&dir).unwrap();
        assert!(Git.current_point(&dir).is_none());
        assert!(Git.contents_at(&dir, "point", &dir.join("a.txt")).is_none());
        assert!(current_point(&dir).is_none());
        assert!(contents_at(&dir, "point", &dir.join("a.txt")).is_none());
        assert!(watch_dir(&dir).is_none());
    }

    #[cfg(feature = "git")]
    #[test]
    fn vcs_worker_keeps_colocated_selection_with_git_disabled() {
        let dir = detect_with_colocated_dir();
        for (pref, expected) in [
            (Preference::Auto, VcsKind::Jj),
            (Preference::Git, VcsKind::Git),
            (Preference::Jj, VcsKind::Jj),
        ] {
            set_preference_for_test(Some(pref));
            Git::set_external_git_enabled(false);
            let config = WorkerConfig::capture();
            let root = dir.to_path_buf();
            let actual = std::thread::spawn(move || {
                config.install();
                let kind = detect_with(preference(), &root, true);
                (kind, Git.workdir(&root))
            })
            .join()
            .unwrap();
            assert_eq!(actual, (expected, None), "{pref:?}");
        }
        Git::set_external_git_enabled(true);
        set_preference_for_test(None);
    }

    /// The seam must not change what a caller sees: going through the backend has to agree with the
    /// free functions it delegates to, for a real repository and for a plain directory alike.
    ///
    /// **This is a git-only safety net.** Both `root`s below resolve through `detect`/`detect_with`
    /// to `VcsKind::Git` (the crate's own directory has a `.git` and no `.jj`; `/` has neither), so
    /// this test never exercises the jj branch of `backend_for`'s routing — see
    /// `detect_with_covers_every_combination_of_pref_form_and_jj_availability` and
    /// `backend_for_a_jj_only_directory_is_read_only` for that.
    #[test]
    fn backend_agrees_with_the_free_functions() {
        for root in [
            konoma_test_support::repo_root().as_path(),
            std::path::Path::new("/"),
        ] {
            let b = backend_for(root);
            assert_eq!(
                b.workdir(root),
                crate::git::workdir(root),
                "workdir {root:?}"
            );
            assert_eq!(
                b.statuses(root).len(),
                crate::git::statuses(root).len(),
                "statuses {root:?}"
            );
            assert_eq!(b.branch(root), crate::git::branch(root), "branch {root:?}");
            assert_eq!(
                b.worktree_origin(root),
                crate::git::worktree_origin(root),
                "worktree_origin {root:?}"
            );
        }
    }

    /// `base_contents` needs a `file` argument that only makes sense per-root (unlike the four
    /// `root`-only methods `backend_agrees_with_the_free_functions` loops over above), so it gets
    /// its own test rather than folding into that loop: konoma's own `Cargo.toml` (tracked, present
    /// at HEAD) for the real repository, and a path that cannot exist for the plain directory.
    ///
    /// `#[cfg(feature = "git")]`: it asserts real (non-empty) content for the real-repo case, which
    /// the no-git stub can never produce — see `backend_base_contents_is_the_no_git_stub_when_the_
    /// feature_is_off` below for that build's own version of this promise.
    #[cfg(feature = "git")]
    #[test]
    fn backend_base_contents_agrees_with_the_free_function() {
        let root_buf = konoma_test_support::repo_root();
        let root = root_buf.as_path();
        let file = root.join("Cargo.toml");
        assert_eq!(
            backend_for(root).base_contents(root, &file),
            crate::git::base_contents(root, &file),
            "base_contents (real repo)"
        );
        assert!(
            backend_for(root)
                .base_contents(root, &file)
                .is_some_and(|b| !b.is_empty()),
            "比較が空同士で成立していない"
        );

        let plain = std::path::Path::new("/");
        let missing = plain.join("no-such-konoma-fixture-file.txt");
        assert_eq!(
            backend_for(plain).base_contents(plain, &missing),
            crate::git::base_contents(plain, &missing),
            "base_contents (plain directory)"
        );
    }

    /// The no-git build's counterpart to `backend_base_contents_agrees_with_the_free_function`:
    /// with the `git` feature off, `crate::git::base_contents` is unconditionally the `None` stub
    /// (see its doc in `git.rs`), and `backend_for` can only ever resolve to `Git` (there is no `Jj`
    /// variant without the feature — see `VcsKind`), so both the free function and the
    /// trait-dispatched call must answer `None` even for konoma's own tracked `Cargo.toml`, which a
    /// `--features git` build resolves to real content (see the test above).
    #[cfg(not(feature = "git"))]
    #[test]
    fn backend_base_contents_is_the_no_git_stub_when_the_feature_is_off() {
        let root_buf = konoma_test_support::repo_root();
        let root = root_buf.as_path();
        let file = root.join("Cargo.toml");
        assert!(
            crate::git::base_contents(root, &file).is_none(),
            "no-git スタブは常に None"
        );
        assert!(
            backend_for(root).base_contents(root, &file).is_none(),
            "backend_for 経由でも同じスタブに落ちる"
        );
        assert_eq!(
            backend_for(root).base_contents(root, &file),
            base_contents(root, &file),
            "自由関数の窓口も同じ答え"
        );
    }

    /// A worker thread owns the backend while it scans, so it has to be `Send + Sync`.
    #[test]
    fn backend_can_cross_a_thread() {
        let root = konoma_test_support::repo_root();
        let handle = std::thread::spawn(move || backend_for(&root).workdir(&root));
        assert!(handle.join().is_ok());
    }

    // --- Preference::parse ---------------------------------------------------------------------

    /// Anything not literally `"git"` or `"jj"` (after trim + lowercase) must fall back to `Auto` —
    /// a typo in `[external] vcs` must never silently take a repository's markers away.
    #[test]
    fn preference_parse_table() {
        let cases: &[(&str, Preference)] = &[
            ("git", Preference::Git),
            ("jj", Preference::Jj),
            ("auto", Preference::Auto),
            ("GIT", Preference::Git),
            ("Jj", Preference::Jj),
            ("AUTO", Preference::Auto),
            (" git\n", Preference::Git),
            ("\tjj\t", Preference::Jj),
            ("  auto  ", Preference::Auto),
            ("", Preference::Auto),
            ("   ", Preference::Auto),
            ("gti", Preference::Auto),       // typo
            ("mercurial", Preference::Auto), // unrelated VCS name
            ("Git ", Preference::Git),
            ("\njj", Preference::Jj),
        ];
        for (input, expected) in cases.iter().copied() {
            assert_eq!(
                Preference::parse(input),
                expected,
                "Preference::parse({input:?})"
            );
        }
    }

    // --- detect_with -----------------------------------------------------------------------------
    // Every fixture below is built through `test_support::unique_tmp`, which is rooted at
    // `std::env::temp_dir()` — never a fixed path, and (checked empirically before writing these:
    // `git -C "$(dirname "$(mktemp -u)")" rev-parse --show-toplevel` fails with "not a git
    // repository") never itself inside a git repository. That matters here specifically: unlike a
    // path under the repository root (`repo_root()`, which IS konoma's own git repository — `crate::git::workdir`
    // walks up through ancestors via libgit2 `Repository::discover`), a fixture under `temp_dir()`
    // is guaranteed not to be swallowed by some unrelated ancestor repository, so a "plain
    // directory" fixture really does resolve to `workdir(..) == None`.

    #[cfg(feature = "git")]
    fn detect_with_empty_dir(prefix: &str) -> konoma_test_support::TmpDir {
        let dir = konoma_test_support::unique_tmp(prefix);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A directory with only `.git` — a real (tiny) repository, via the same `git2::Repository::init`
    /// pattern `git::tests::init_repo` uses.
    #[cfg(feature = "git")]
    fn detect_with_git_only_dir() -> konoma_test_support::TmpDir {
        let dir = detect_with_empty_dir("konoma_vcs_detect_git_only");
        git2::Repository::init(&dir).unwrap();
        dir
    }

    /// A directory with only `.jj`. No `jj git init` needed: `jj::workspace_root` is a pure
    /// filesystem walk for the nearest ancestor holding `.jj` (see its own doc comment) — jj is
    /// never actually launched to build this fixture.
    #[cfg(feature = "git")]
    fn detect_with_jj_only_dir() -> konoma_test_support::TmpDir {
        let dir = detect_with_empty_dir("konoma_vcs_detect_jj_only");
        std::fs::create_dir_all(dir.join(".jj")).unwrap();
        dir
    }

    /// Colocated: `.git` and `.jj` side by side — the shape `jj git init --colocate` produces, and
    /// the one real-world onboarding path this repository had zero fixtures for before this.
    #[cfg(feature = "git")]
    fn detect_with_colocated_dir() -> konoma_test_support::TmpDir {
        let dir = detect_with_empty_dir("konoma_vcs_detect_colocated");
        git2::Repository::init(&dir).unwrap();
        std::fs::create_dir_all(dir.join(".jj")).unwrap();
        dir
    }

    /// Neither — a plain directory with no VCS marker at all.
    #[cfg(feature = "git")]
    fn detect_with_neither_dir() -> konoma_test_support::TmpDir {
        detect_with_empty_dir("konoma_vcs_detect_neither")
    }

    /// The full 3 (pref) x 4 (form) x 2 (jj_available) = 24-case table. Column meanings:
    /// - pref: `[external] vcs` as parsed by `Preference::parse`.
    /// - form: which VCS marker(s) the fixture directory has.
    /// - jj_available: whether `jj::available()` would have reported true.
    /// - expected: the backend `detect_with` must choose.
    #[cfg(feature = "git")]
    #[test]
    fn detect_with_covers_every_combination_of_pref_form_and_jj_availability() {
        let git_only = detect_with_git_only_dir();
        let jj_only = detect_with_jj_only_dir();
        let colocated = detect_with_colocated_dir();
        let neither = detect_with_neither_dir();

        use Preference::{Auto, Git as G, Jj as J};
        use VcsKind::{Git, Jj};
        let cases: &[(Preference, &str, &Path, bool, VcsKind)] = &[
            // --- vcs = "git": always git, no matter the directory or jj availability.
            (G, "git_only", git_only.as_path(), true, Git),
            (G, "git_only", git_only.as_path(), false, Git),
            (G, "jj_only", jj_only.as_path(), true, Git),
            (G, "jj_only", jj_only.as_path(), false, Git),
            (G, "colocated", colocated.as_path(), true, Git),
            (G, "colocated", colocated.as_path(), false, Git),
            (G, "neither", neither.as_path(), true, Git),
            (G, "neither", neither.as_path(), false, Git),
            // --- vcs = "auto": git wherever git can answer; jj only fills the gap.
            (Auto, "git_only", git_only.as_path(), true, Git),
            (Auto, "git_only", git_only.as_path(), false, Git),
            (Auto, "jj_only", jj_only.as_path(), true, Jj),
            (Auto, "jj_only", jj_only.as_path(), false, Git), // no jj binary -> falls back to git
            (Auto, "colocated", colocated.as_path(), true, Git), // existing git users: unchanged
            (Auto, "colocated", colocated.as_path(), false, Git),
            (Auto, "neither", neither.as_path(), true, Git),
            (Auto, "neither", neither.as_path(), false, Git),
            // --- vcs = "jj": jj wherever a `.jj` exists, colocated or not.
            (J, "git_only", git_only.as_path(), true, Git), // no `.jj` at all -> can't be jj
            (J, "git_only", git_only.as_path(), false, Git),
            (J, "jj_only", jj_only.as_path(), true, Jj),
            (J, "jj_only", jj_only.as_path(), false, Git), // no jj binary -> falls back to git
            (J, "colocated", colocated.as_path(), true, Jj), // pinned to jj despite the `.git`
            (J, "colocated", colocated.as_path(), false, Git),
            (J, "neither", neither.as_path(), true, Git),
            (J, "neither", neither.as_path(), false, Git),
        ];

        for (pref, form, root, jj_available, expected) in cases.iter().copied() {
            assert_eq!(
                detect_with(pref, root, jj_available),
                expected,
                "pref={pref:?} form={form} jj_available={jj_available}"
            );
        }
    }

    /// The single most important promise in this file: upgrading konoma must never change what an
    /// existing git repository shows, even after `jj git init --colocate` was run inside it and even
    /// when jj is installed and working. `auto` keeps git wherever git can already answer.
    #[cfg(feature = "git")]
    #[test]
    fn detect_with_colocated_under_auto_stays_git() {
        let dir = detect_with_colocated_dir();
        assert_eq!(detect_with(Preference::Auto, &dir, true), VcsKind::Git);
        assert_eq!(detect_with(Preference::Auto, &dir, false), VcsKind::Git);
    }

    /// The matching promise on the other side: `[external] vcs = "jj"` pins a colocated repository
    /// to jj, overriding what `auto` would have chosen.
    #[cfg(feature = "git")]
    #[test]
    fn detect_with_colocated_under_explicit_jj_pref_becomes_jj() {
        let dir = detect_with_colocated_dir();
        assert_eq!(detect_with(Preference::Jj, &dir, true), VcsKind::Jj);
    }

    /// `vcs = "git"` always wins, even in a `.jj`-only directory that has no git repository to fall
    /// back to (there is nothing else `detect_with` could return, but this pins that it does not
    /// e.g. panic or treat the absence of git as license to use jj anyway).
    #[cfg(feature = "git")]
    #[test]
    fn detect_with_jj_only_under_explicit_git_pref_stays_git() {
        let dir = detect_with_jj_only_dir();
        assert_eq!(detect_with(Preference::Git, &dir, true), VcsKind::Git);
    }

    /// Without a working `jj` binary, `auto` must fall back to git rather than choosing a backend it
    /// cannot actually run — "falling back shows something rather than nothing" (see `detect`'s doc).
    #[cfg(feature = "git")]
    #[test]
    fn detect_with_jj_only_under_auto_without_jj_binary_stays_git() {
        let dir = detect_with_jj_only_dir();
        assert_eq!(detect_with(Preference::Auto, &dir, false), VcsKind::Git);
    }

    // --- [external] git = false's effect on detect_with's `auto` gap-filling ----------------------
    // The 24-case table above (`detect_with_covers_every_combination_of_pref_form_and_jj_availability`)
    // never turns `external_git_enabled` off, so this combination — the one place `[external] git`
    // can change which *backend* answers, not just whether git itself answers — went unexercised.

    /// `crate::git::set_external_git_enabled(false)` (config `[external] git = false`) makes
    /// `crate::git::workdir` answer `None` for every directory, including a real repository — see
    /// `external_git_disabled_returns_empty_for_a_real_repo` in `git.rs` for the read-side proof.
    /// `detect_with`'s `git_answers` check under `auto` is exactly `crate::git::workdir(root).is_some()`,
    /// so with git integration off, a **colocated** directory (`.git` and `.jj` side by side — the
    /// one `detect_with_colocated_under_auto_stays_git` pins as staying git under normal conditions)
    /// silently falls through to jj instead, as long as a jj binary is available.
    ///
    /// This is the **current, intended** behavior — konoma has one config knob for turning off git
    /// integration, and `auto` has always meant "git wherever git can answer"; if git cannot answer
    /// because the user turned it off, jj filling the gap is the same rule already governing every
    /// other case in the table above, just triggered by a different reason `workdir` returned `None`.
    /// This test exists to pin that reading with a test, not to bless it as unquestionably right —
    /// see this task's report for the reasoning spelled out.
    #[cfg(feature = "git")]
    #[test]
    fn detect_with_colocated_falls_back_to_jj_when_git_integration_is_disabled() {
        let dir = detect_with_colocated_dir();
        // Thread-local (see `EXTERNAL_GIT_ENABLED`'s doc in git.rs): scoped to this test's own
        // thread, so it cannot leak into a concurrently running test.
        crate::git::set_external_git_enabled(false);
        assert_eq!(
            detect_with(Preference::Auto, &dir, true),
            VcsKind::Jj,
            "with git integration off, crate::git::workdir reads None, so `auto` falls through to \
             jj even though `.git` is right there"
        );
        crate::git::set_external_git_enabled(true);
        assert_eq!(
            detect_with(Preference::Auto, &dir, true),
            VcsKind::Git,
            "sanity: re-enabling restores the usual colocated-stays-git answer, proving the flag \
             is not stuck off"
        );
    }

    /// The matching guardrail on the other side: an explicit `[external] vcs = "git"` preference
    /// must keep winning even with git integration disabled. `detect_with`'s `Preference::Git`
    /// branch returns before it ever calls `crate::git::workdir`, so it cannot be swayed by this
    /// flag — unlike `auto`, above.
    #[cfg(feature = "git")]
    #[test]
    fn detect_with_colocated_under_explicit_git_pref_ignores_the_disabled_flag() {
        let dir = detect_with_colocated_dir();
        crate::git::set_external_git_enabled(false);
        assert_eq!(
            detect_with(Preference::Git, &dir, true),
            VcsKind::Git,
            "an explicit git preference must not be swayed by [external] git"
        );
        crate::git::set_external_git_enabled(true);
    }

    // --- backend_for's routing, not just detect's answer -----------------------------------------

    /// The asymmetric counterpart to `backend_for_a_jj_only_directory_is_read_only` below: jj's
    /// `caps().write == false` is asserted there (and `src/e2e_tests.rs` separately covers the
    /// reverse pin, git after having been jj), but nothing previously asserted git's own
    /// `caps().write == true` directly — it was only ever true "by construction", never checked.
    /// No fixture needed: `Git::caps` answers the same way regardless of `root`.
    #[test]
    fn git_backend_caps_reports_write_true() {
        assert!(
            Git.caps().write,
            "git can change the repository — unlike jj, see Jj::caps"
        );
    }

    /// `backend_for` must not just *decide* jj, it must actually *route* to a backend whose `caps()`
    /// reports read-only — the property the rest of the app (write-gating menus, etc.) depends on.
    /// `backend_agrees_with_the_free_functions` never exercises this (see its doc comment): both its
    /// roots resolve to git. This needs a real `jj` binary (`detect` calls `jj::available()`
    /// directly, not through the injectable `detect_with`), so it skips itself where jj is not
    /// installed rather than asserting anything about an environment it cannot probe.
    #[cfg(feature = "git")]
    #[test]
    fn backend_for_a_jj_only_directory_is_read_only() {
        if !jj::available() {
            eprintln!("skipping: no working `jj` binary on this machine");
            return;
        }
        let dir = detect_with_jj_only_dir();
        // `backend_for` reads the preference, which every `App::new` in the suite writes. Pin it
        // for this thread only, so neither direction of that can reach across tests.
        set_preference_for_test(Some(Preference::Auto));
        assert_eq!(
            detect(&dir),
            VcsKind::Jj,
            "sanity: detect must pick jj here"
        );
        let backend = backend_for(&dir);
        assert!(
            !backend.caps().write,
            "a .jj-only directory's backend must be read-only"
        );
        set_preference_for_test(None);
    }
}
