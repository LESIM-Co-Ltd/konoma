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

use crate::git::FileStatus;

#[cfg(feature = "git")]
pub mod jj;

/// The read surface a version-control backend answers.
///
/// `Send + Sync` because the status scan runs on a worker thread (`spawn_or_sync_statuses`).
///
/// Every method has the same "quietly return nothing" contract as the free functions in
/// [`crate::git`]: outside a repository, on failure, or when the integration is switched off, they
/// return an empty/None answer rather than an error, so a caller never has to special-case
/// "no repository here".
pub trait Vcs: Send + Sync {
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
}

/// The git backend. It delegates to [`crate::git`], which still holds the implementation.
pub struct Git;

impl Vcs for Git {
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
}

/// Which version-control system answers for `root`.
///
/// **jj answers only where git cannot.** The eventual rule is the opposite — a directory holding
/// `.jj` belongs to whoever created it — but flipping that while the jj backend is still filling in
/// would take working views away from colocated repositories, where git answers everything today.
/// The preference flips once jj covers the same surface; see `docs/FEATURE-JJ-SUPPORT.md`.
pub fn detect(root: &Path) -> VcsKind {
    #[cfg(feature = "git")]
    if crate::git::workdir(root).is_none() && jj::workspace_root(root).is_some() && jj::available()
    {
        return VcsKind::Jj;
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
