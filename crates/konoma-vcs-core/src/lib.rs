//! konoma's version-control reading, below the views.
//!
//! - [`git`]: git status, diffs, log and the commit-graph layout (libgit2 plus the git CLI).
//! - [`vcs`]: which backend (git or jj) answers for a directory, and the one read surface both
//!   implement ([`vcs::jj`] is the Jujutsu backend).
//! - [`glyph`]: the glyph that draws a commit-graph node.
//!
//! Nothing here knows the host's UI or its translations: the words the graph writes into its
//! "uncommitted changes" row come in as [`git::WorktreeLabels`].

pub mod git;
pub mod glyph;
pub mod vcs;

#[cfg(any(test, feature = "test-hooks"))]
pub mod testing;

/// The libgit2 bindings this crate is built on, for hosts whose tests build repositories.
#[cfg(feature = "git")]
pub use git2;
