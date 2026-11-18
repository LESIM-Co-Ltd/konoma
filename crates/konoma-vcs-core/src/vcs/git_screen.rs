//! Temporary Git-only boundary until the git screens move into the VCS plugin.
//!
//! Screen-only reads: binary availability, worktrees, merge-base time, diff-since,
//! whole-worktree diff, branch recency and graph legends. Screen-only writes: stage,
//! unstage, stage-all, unstage-all, discard, commit, checkout, create/delete branch,
//! and add worktree. Shared host features must use Vcs instead.
//! The startup integration switch also lives here because it configures this backend.

use super::{DiffLine, Git, GraphRow, Path};

impl Git {
    #[cfg(feature = "git")]
    pub fn set_external_git_enabled(enabled: bool) {
        crate::git::set_external_git_enabled(enabled)
    }

    #[cfg(feature = "git")]
    pub fn git_binary_available() -> bool {
        crate::git::git_binary_available()
    }

    pub fn worktrees(root: &Path) -> Vec<crate::git::WorktreeInfo> {
        crate::git::worktrees(root)
    }

    pub fn merge_base_time(root: &Path, base: &str) -> Option<i64> {
        crate::git::merge_base_time(root, base)
    }

    pub fn diff_since(root: &Path, base: &str) -> Vec<DiffLine> {
        crate::git::diff_since(root, base)
    }

    pub fn worktree_diff(root: &Path) -> Vec<DiffLine> {
        crate::git::worktree_diff(root)
    }

    pub fn branches_by_recency(root: &Path) -> Vec<(String, bool, i64)> {
        crate::git::branches_by_recency(root)
    }

    pub fn legend_from_rows(
        rows: &[GraphRow],
        root: &Path,
        base: Option<&str>,
    ) -> Vec<crate::git::LegendEntry> {
        crate::git::legend_from_rows(rows, root, base)
    }

    pub fn stage(root: &Path, file: &Path) -> anyhow::Result<()> {
        crate::git::stage(root, file)
    }

    pub fn unstage(root: &Path, file: &Path) -> anyhow::Result<()> {
        crate::git::unstage(root, file)
    }

    pub fn stage_all(root: &Path) -> anyhow::Result<()> {
        crate::git::stage_all(root)
    }

    pub fn unstage_all(root: &Path) -> anyhow::Result<()> {
        crate::git::unstage_all(root)
    }

    pub fn discard(root: &Path, file: &Path) -> anyhow::Result<()> {
        crate::git::discard(root, file)
    }

    pub fn commit(root: &Path, message: &str) -> anyhow::Result<()> {
        crate::git::commit(root, message)
    }

    pub fn checkout(root: &Path, name: &str) -> anyhow::Result<()> {
        crate::git::checkout(root, name)
    }

    pub fn create_branch(root: &Path, name: &str) -> anyhow::Result<()> {
        crate::git::create_branch(root, name)
    }

    pub fn delete_branch(root: &Path, name: &str, force: bool) -> anyhow::Result<()> {
        crate::git::delete_branch(root, name, force)
    }

    pub fn worktree_add(
        root: &Path,
        path: &Path,
        branch: &str,
        create: bool,
    ) -> anyhow::Result<()> {
        crate::git::worktree_add(root, path, branch, create)
    }
}
