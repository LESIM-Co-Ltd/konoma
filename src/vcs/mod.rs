//! Which version-control system backs a directory, and the one read surface every backend
//! implements.
//!
//! konoma draws **one** set of views (tree markers, diff, gutter, graph, hub). What changes between
//! version-control systems is where the data comes from, never how it is drawn — so a backend
//! answers questions and returns the shared types (`FileStatus`, `DiffLine`, `GraphRow`, …), and the
//! `ui` layer stays unaware of which one answered.
//!
//! Today there is exactly one backend, and `backend_for` always returns it. It exists as a seam so
//! the jj (Jujutsu) backend can be added without a second copy of the views; see
//! `docs/FEATURE-JJ-SUPPORT.md`.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::git::{BranchInfo, ChangeEntry, CommitInfo, CommitMeta, DiffLine, FileStatus, GraphRow};

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
/// Every method has the same "quietly return nothing" contract as the free functions in
/// [`crate::git`]: outside a repository, on failure, or when the integration is switched off, they
/// return an empty/None answer rather than an error, so a caller never has to special-case
/// "no repository here".
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
        lang: crate::i18n::Lang,
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
        lang: crate::i18n::Lang,
        refs: Option<&[String]>,
    ) -> Vec<GraphRow> {
        // git's graph is already every reachable commit; there is no narrower range to widen from.
        crate::git::graph_with_base(root, base, lang, refs)
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

/// Which backend the user asked for, from `[external] vcs`.
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

/// Applies `[external] vcs`.
pub fn set_preference(p: Preference) {
    let v = match p {
        Preference::Auto => 0,
        Preference::Git => 1,
        Preference::Jj => 2,
    };
    PREFERENCE.store(v, std::sync::atomic::Ordering::Relaxed);
}

#[cfg_attr(not(feature = "git"), allow(dead_code))]
fn preference() -> Preference {
    match PREFERENCE.load(std::sync::atomic::Ordering::Relaxed) {
        1 => Preference::Git,
        2 => Preference::Jj,
        _ => Preference::Auto,
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
        _lang: crate::i18n::Lang,
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

/// Which version-control system answers for `root`. See [`Preference`] for what decides it.
///
/// git also answers where a `.jj` exists but the `jj` binary does not: falling back shows something
/// rather than nothing.
pub fn detect(root: &Path) -> VcsKind {
    #[cfg(feature = "git")]
    {
        let pref = preference();
        if pref == Preference::Git {
            return VcsKind::Git;
        }
        // Under `auto`, git keeps whatever it can already answer; jj fills the gap where there is no
        // git repository to ask. `jj` asks for jj wherever it can answer at all.
        let git_answers = pref == Preference::Auto && crate::git::workdir(root).is_some();
        if !git_answers && jj::workspace_root(root).is_some() && jj::available() {
            return VcsKind::Jj;
        }
    }
    let _ = root;
    VcsKind::Git
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
    backend_for(root).file_diff(root, file)
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
    lang: crate::i18n::Lang,
    refs: Option<&[String]>,
) -> Vec<GraphRow> {
    backend_for(root).graph(all, root, base, lang, refs)
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

    /// The seam must not change what a caller sees: going through the backend has to agree with the
    /// free functions it delegates to, for a real repository and for a plain directory alike.
    #[test]
    fn backend_agrees_with_the_free_functions() {
        for root in [
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")),
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

    /// A worker thread owns the backend while it scans, so it has to be `Send + Sync`.
    #[test]
    fn backend_can_cross_a_thread() {
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let handle = std::thread::spawn(move || backend_for(&root).workdir(&root));
        assert!(handle.join().is_ok());
    }
}
