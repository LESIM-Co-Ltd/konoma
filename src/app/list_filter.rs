//! The `/` filter of the version-control list screens: one component for all five lists.
//!
//! The branch and worktree lists had a filter each, written twice (five functions apiece); the
//! changes list, the log and the graph had none. They now share this: a list has a query and an
//! "input is active" flag, the query narrows the rows to those whose **visible text** contains it
//! (case-insensitive), `Enter` keeps the query and goes back to moving, `Esc` clears it. The
//! per-list public functions (`git_branch_filter_push`, ...) remain as one-line wrappers.
//!
//! How the narrowed rows are held differs by list, on purpose:
//!
//! - the branch and worktree lists build a filtered copy (`git_branch_view`, `git_worktree_view`)
//!   and their cursor indexes that copy (as it always did);
//! - the changes list, the log and the graph keep their rows whole and their cursor an index into
//!   all of them, so every other operation (stage this file, open this commit) keeps working on
//!   the right row, and the screen shows only the rows [`App::list_visible`] names. A cursor that
//!   the query hides moves to the nearest row that is still shown.

use super::*;

/// The lists that have a `/` filter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ListId {
    Changes,
    Log,
    Graph,
    Branches,
    Worktrees,
}

impl ListId {
    pub(crate) const ALL: [ListId; 5] = [
        ListId::Changes,
        ListId::Log,
        ListId::Graph,
        ListId::Branches,
        ListId::Worktrees,
    ];
}

/// Move `delta` rows along `visible` from the row `cur` (clamped to its ends; `i32::MIN` /
/// `i32::MAX` jump to the first / last). `cur` that is not in `visible` counts from the nearest
/// shown row below it.
pub(crate) fn step_visible(visible: &[usize], cur: usize, delta: i32) -> usize {
    if visible.is_empty() {
        return cur;
    }
    let pos = visible
        .iter()
        .position(|&i| i >= cur)
        .unwrap_or(visible.len() - 1);
    let next = if delta == i32::MIN {
        0
    } else if delta == i32::MAX {
        visible.len() - 1
    } else {
        (pos as i64 + delta as i64).clamp(0, visible.len() as i64 - 1) as usize
    };
    visible[next]
}

impl App {
    fn list_filter_parts(&mut self, l: ListId) -> (&mut String, &mut bool) {
        match l {
            ListId::Changes => (
                &mut self.tab.git_view_filter,
                &mut self.tab.git_view_filtering,
            ),
            ListId::Log => (
                &mut self.tab.git_log_filter,
                &mut self.tab.git_log_filtering,
            ),
            ListId::Graph => (
                &mut self.tab.git_graph_filter,
                &mut self.tab.git_graph_filtering,
            ),
            ListId::Branches => (
                &mut self.tab.git_branch_filter,
                &mut self.tab.git_branch_filtering,
            ),
            ListId::Worktrees => (
                &mut self.tab.git_worktree_filter,
                &mut self.tab.git_worktree_filtering,
            ),
        }
    }

    /// The query of list `l` (empty = not filtered).
    pub(crate) fn list_filter_query(&self, l: ListId) -> &str {
        match l {
            ListId::Changes => &self.tab.git_view_filter,
            ListId::Log => &self.tab.git_log_filter,
            ListId::Graph => &self.tab.git_graph_filter,
            ListId::Branches => &self.tab.git_branch_filter,
            ListId::Worktrees => &self.tab.git_worktree_filter,
        }
    }

    /// Whether the query of list `l` is being typed (keys are captured as characters).
    pub(crate) fn list_filtering(&self, l: ListId) -> bool {
        match l {
            ListId::Changes => self.tab.git_view_filtering,
            ListId::Log => self.tab.git_log_filtering,
            ListId::Graph => self.tab.git_graph_filtering,
            ListId::Branches => self.tab.git_branch_filtering,
            ListId::Worktrees => self.tab.git_worktree_filtering,
        }
    }

    /// The list whose query is being typed, if any (at most one can be: only the front screen
    /// takes keys).
    pub(crate) fn filtering_list(&self) -> Option<ListId> {
        ListId::ALL.into_iter().find(|&l| self.list_filtering(l))
    }

    /// The list on the front screen, if it has a filter.
    #[cfg(feature = "git")]
    pub(crate) fn front_list(&self) -> Option<ListId> {
        if self.is_git_detail() {
            None
        } else if self.is_git_log() {
            Some(ListId::Log)
        } else if self.is_git_graph_picker() {
            None
        } else if self.is_git_graph() {
            Some(ListId::Graph)
        } else if self.is_git_branches() {
            Some(ListId::Branches)
        } else if self.is_git_worktrees() {
            Some(ListId::Worktrees)
        } else if self.is_git_view() {
            Some(ListId::Changes)
        } else {
            None
        }
    }

    /// `/`: start typing a query (it starts empty, so every row is shown again).
    pub(crate) fn list_filter_start(&mut self, l: ListId) {
        let (q, on) = self.list_filter_parts(l);
        q.clear();
        *on = true;
        match l {
            ListId::Branches => self.tab.git_branch_sel = 0,
            ListId::Worktrees => self.tab.git_worktree_sel = 0,
            _ => {}
        }
    }

    pub(crate) fn list_filter_push(&mut self, l: ListId, c: char) {
        self.list_filter_parts(l).0.push(c);
        self.list_filter_changed(l);
    }

    pub(crate) fn list_filter_backspace(&mut self, l: ListId) {
        self.list_filter_parts(l).0.pop();
        self.list_filter_changed(l);
    }

    /// `Enter`: keep the query, go back to moving.
    pub(crate) fn list_filter_commit(&mut self, l: ListId) {
        *self.list_filter_parts(l).1 = false;
    }

    /// `Esc`: drop the query, show every row.
    pub(crate) fn list_filter_clear(&mut self, l: ListId) {
        let (q, on) = self.list_filter_parts(l);
        q.clear();
        *on = false;
        self.list_filter_changed(l);
    }

    /// Put the cursor back on a shown row after the query changed.
    pub(super) fn list_filter_changed(&mut self, l: ListId) {
        match l {
            ListId::Branches => self.clamp_git_branch_sel(),
            ListId::Worktrees => self.clamp_git_worktree_sel(),
            ListId::Changes => {
                if let Some(v) = self.list_visible(l) {
                    self.tab.git_view_sel = step_visible(&v, self.tab.git_view_sel, 0);
                }
            }
            ListId::Log => {
                if let Some(v) = self.list_visible(l) {
                    self.tab.git_log_sel = step_visible(&v, self.tab.git_log_sel, 0);
                }
            }
            ListId::Graph => {
                if let Some(v) = self.list_visible(l) {
                    self.tab.git_graph_sel = step_visible(&v, self.tab.git_graph_sel, 0);
                }
            }
        }
    }

    /// The text of a changes-list row as the screen shows it (staged mark, status letter, path).
    pub(crate) fn changes_row_text(&self, e: &crate::git::ChangeEntry) -> String {
        format!(
            "{}{} {}",
            if e.staged { "● " } else { "  " },
            e.status.marker(),
            self.format_path(&e.path)
        )
    }

    /// The text of a log row as the screen shows it.
    pub(crate) fn log_row_text(c: &crate::git::CommitInfo) -> String {
        let date = crate::fileops::format_epoch_short(c.time_epoch.max(0) as u64);
        format!("{} {} {} {date}", c.short, c.summary, c.author)
    }

    /// The text of a graph row as the screen shows it; `None` for a connector-only row (just
    /// lanes, no text).
    pub(crate) fn graph_row_text(r: &crate::git::GraphRow) -> Option<String> {
        if r.worktree {
            Some(format!("{} {}", r.subject, r.date))
        } else if r.commit.is_some() {
            Some(format!(
                "{} {} {} {} {}",
                r.refs, r.subject, r.short, r.author, r.date
            ))
        } else {
            None
        }
    }

    /// The rows of the changes list, the log or the graph that the query leaves shown, as
    /// indexes into all of them; `None` while there is no query (every row is shown). A row that
    /// cannot be named by text (a graph's connector line) is hidden by any query. Ascending.
    pub(crate) fn list_visible(&self, l: ListId) -> Option<Vec<usize>> {
        let q = self.list_filter_query(l).to_lowercase();
        if q.is_empty() {
            return None;
        }
        let hit = |text: &str| text.to_lowercase().contains(&q);
        Some(match l {
            ListId::Changes => self
                .tab
                .git_view_entries
                .iter()
                .enumerate()
                .filter(|(_, e)| hit(&self.changes_row_text(e)))
                .map(|(i, _)| i)
                .collect(),
            ListId::Log => self
                .tab
                .git_log
                .iter()
                .flatten()
                .enumerate()
                .filter(|(_, c)| hit(&Self::log_row_text(c)))
                .map(|(i, _)| i)
                .collect(),
            ListId::Graph => self
                .tab
                .git_graph
                .iter()
                .flatten()
                .enumerate()
                .filter(|(_, r)| Self::graph_row_text(r).is_some_and(|t| hit(&t)))
                .map(|(i, _)| i)
                .collect(),
            // These two narrow a copy of the list, not the list itself.
            ListId::Branches | ListId::Worktrees => return None,
        })
    }
}
