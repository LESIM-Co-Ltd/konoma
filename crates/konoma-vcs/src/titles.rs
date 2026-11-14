//! The words of each list's title. The host draws them at the left of the frame's top edge and
//! fills in the filter query and the shown count (`ListHeading`); the words stay English in every
//! language, as the old hub's titles did.

use super::Backend;
use konoma_plugin::prelude::*;
use konoma_plugin::view::ListHeading;
use konoma_vcs_core::vcs::Vcs;
use std::path::Path;

/// How the title names the system answering it.
pub(super) fn name(backend: &Backend) -> &'static str {
    match backend {
        Backend::Git => "Git",
        Backend::Jj => "jj",
    }
}

/// The marker in front of a branch name: git points at a branch with a glyph (or `br:` when icons
/// are off); jj's label already opens with `@`, so it gets none.
fn branch_marker(backend: &Backend, icons: bool) -> &'static str {
    match backend {
        Backend::Git if icons => "\u{2387}",
        Backend::Git => "br:",
        Backend::Jj => "",
    }
}

/// ` Git ⎇ <branch>`, or just ` Git` while there is no branch.
fn named_branch(prefix: &str, backend: &Backend, root: &Path, icons: bool) -> String {
    let mut text = format!(" {prefix}");
    // The plugin's own backend answers, not the one the directory would pick: a colocated
    // repository holds both, and jj's label is its working copy, not git's HEAD.
    let branch = match backend {
        Backend::Git => konoma_vcs_core::vcs::Git.branch(root),
        #[cfg(feature = "git")]
        Backend::Jj => konoma_vcs_core::vcs::Jj.branch(root),
        #[cfg(not(feature = "git"))]
        Backend::Jj => None,
    };
    if let Some(branch) = branch {
        match branch_marker(backend, icons) {
            "" => text.push_str(&format!(" {branch}")),
            marker => text.push_str(&format!(" {marker} {branch}")),
        }
    }
    text
}

fn heading(before: String, gap: u8, after: &str) -> ListHeading {
    ListHeading::new(Line::raw(before))
        .query_gap(gap)
        .after(Line::raw(after.to_owned()))
}

pub(super) fn changes(backend: &Backend, root: &Path, icons: bool) -> ListHeading {
    heading(named_branch(name(backend), backend, root, icons), 1, " ")
}

pub(super) fn log(backend: &Backend, root: &Path, icons: bool) -> ListHeading {
    heading(
        named_branch(&format!("{} log", name(backend)), backend, root, icons),
        1,
        " ",
    )
}

pub(super) fn branches(backend: &Backend) -> ListHeading {
    heading(
        match backend {
            Backend::Git => " Git branches".into(),
            Backend::Jj => " jj bookmarks".into(),
        },
        2,
        " ",
    )
}

pub(super) fn worktrees() -> ListHeading {
    heading(" Git worktrees".into(), 2, " ")
}

/// The graph's title, with a pinned base after the count (git only).
pub(super) fn graph(backend: &Backend, base: Option<&str>, icons: bool) -> ListHeading {
    let after = match (backend, base) {
        (Backend::Git, Some(base)) => {
            format!("  {}base: {base} ", if icons { "\u{2316} " } else { "" })
        }
        _ => " ".into(),
    };
    heading(format!(" {} graph", name(backend)), 1, &after)
}
