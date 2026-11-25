//! The `/` filter of the overlay lists (layer 1 of the key design: `/` filters a list).
//!
//! A list has a query and an "input is active" flag, the query narrows the rows to those whose
//! **visible text** contains it (case-insensitive), `Enter` keeps the query and goes back to
//! moving, `Esc` clears it. The overlay lists (the outline / page list, the tab list and the
//! bookmark list) keep their rows whole and their cursor an index into all of them; the screen
//! shows only the rows [`App::list_visible`] names. A cursor that the query hides moves to the
//! nearest row that is still shown. They keep their query only while they are open: opening one
//! starts it empty.

use super::*;

/// The lists that have a `/` filter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ListId {
    /// The bookmark list overlay.
    Bookmarks,
    /// The tab list overlay.
    Tabs,
    /// The heading outline / page list overlay.
    Outline,
}

impl ListId {
    pub(crate) const ALL: [ListId; 3] = [ListId::Bookmarks, ListId::Tabs, ListId::Outline];
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
            ListId::Bookmarks => (
                &mut self.bookmark_list_filter,
                &mut self.bookmark_list_filtering,
            ),
            ListId::Tabs => (&mut self.tab_list_filter, &mut self.tab_list_filtering),
            ListId::Outline => (&mut self.outline_filter, &mut self.outline_filtering),
        }
    }

    /// The query of list `l` (empty = not filtered).
    pub(crate) fn list_filter_query(&self, l: ListId) -> &str {
        match l {
            ListId::Bookmarks => &self.bookmark_list_filter,
            ListId::Tabs => &self.tab_list_filter,
            ListId::Outline => &self.outline_filter,
        }
    }

    /// Whether the query of list `l` is being typed (keys are captured as characters).
    pub(crate) fn list_filtering(&self, l: ListId) -> bool {
        match l {
            // An overlay that closed under the typing (follow mode opening another file) takes
            // its typing with it.
            ListId::Bookmarks => self.bookmark_list_filtering && self.bookmark_list,
            ListId::Tabs => self.tab_list_filtering && self.tab_list,
            ListId::Outline => self.outline_filtering && self.outline_open,
        }
    }

    /// The list whose query is being typed, if any (at most one can be: only the front screen
    /// takes keys).
    pub(crate) fn filtering_list(&self) -> Option<ListId> {
        ListId::ALL.into_iter().find(|&l| self.list_filtering(l))
    }

    /// The list on the front screen, if it has a filter.
    pub(crate) fn front_list(&self) -> Option<ListId> {
        if self.is_bookmark_list() {
            Some(ListId::Bookmarks)
        } else if self.is_tab_list() {
            Some(ListId::Tabs)
        } else if self.is_outline() {
            Some(ListId::Outline)
        } else {
            None
        }
    }

    /// Forget the query of list `l` (an overlay list starts empty each time it opens).
    pub(crate) fn list_filter_reset(&mut self, l: ListId) {
        let (q, on) = self.list_filter_parts(l);
        q.clear();
        *on = false;
    }

    /// `/`: start typing a query (it starts empty, so every row is shown again).
    pub(crate) fn list_filter_start(&mut self, l: ListId) {
        let (q, on) = self.list_filter_parts(l);
        q.clear();
        *on = true;
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
            ListId::Bookmarks => {
                if let Some(v) = self.list_visible(l) {
                    self.bookmark_list_sel = step_visible(&v, self.bookmark_list_sel, 0);
                }
            }
            ListId::Tabs => {
                if let Some(v) = self.list_visible(l) {
                    self.tab_list_sel = step_visible(&v, self.tab_list_sel, 0);
                }
            }
            ListId::Outline => {
                if let Some(v) = self.list_visible(l) {
                    self.outline_sel = step_visible(&v, self.outline_sel, 0);
                }
            }
        }
    }

    /// The rows of an overlay list that the query leaves shown, as
    /// indexes into all of them; `None` while there is no query (every row is shown). A row that
    /// cannot be named by text (a graph's connector line) is hidden by any query. Ascending.
    pub(crate) fn list_visible(&self, l: ListId) -> Option<Vec<usize>> {
        let q = self.list_filter_query(l).to_lowercase();
        if q.is_empty() {
            return None;
        }
        let hit = |text: &str| text.to_lowercase().contains(&q);
        Some(match l {
            ListId::Bookmarks => self
                .bookmark_list_items()
                .iter()
                .enumerate()
                .filter(|(_, (local, key, p))| hit(&self.bookmark_row_text(*local, *key, p)))
                .map(|(i, _)| i)
                .collect(),
            ListId::Tabs => (0..self.tab_count())
                .filter(|&i| hit(&self.tab_row_text(i)))
                .collect(),
            ListId::Outline => self
                .outline_rows()
                .iter()
                .enumerate()
                .filter(|(_, (_, text))| hit(text))
                .map(|(i, _)| i)
                .collect(),
        })
    }

    /// The text of a tab-list row as the screen shows it (number, name, root).
    pub(crate) fn tab_row_text(&self, i: usize) -> String {
        let root = crate::app::home_relative(&self.tab_root(i));
        format!("{}:{}  —  {root}", i + 1, self.tab_label(i))
    }

    /// The text of a bookmark-list row as the screen shows it (key, path).
    pub(crate) fn bookmark_row_text(&self, is_local: bool, key: char, path: &Path) -> String {
        format!("{key}  {}", self.bookmark_display_path(is_local, path))
    }

    /// Whether row `sel` of an overlay list is shown under the query (every row is, without
    /// one). What acts on "the selected row" asks first, so a query that hides the whole list
    /// leaves nothing to jump to, close or delete.
    pub(crate) fn list_row_shown(&self, l: ListId, sel: usize) -> bool {
        self.list_visible(l).is_none_or(|v| v.contains(&sel))
    }

    /// Move the cursor of an overlay list `delta` rows among the rows that are shown, wrapping
    /// at the ends (as these lists always did). `sel` is the cursor, `n` the number of rows.
    pub(crate) fn overlay_step(&self, l: ListId, sel: usize, n: usize, delta: i32) -> usize {
        if n == 0 {
            return sel;
        }
        match self.list_visible(l) {
            None => (sel as i32 + delta).rem_euclid(n as i32) as usize,
            Some(v) if v.is_empty() => sel,
            Some(v) => {
                let pos = v.iter().position(|&i| i >= sel).unwrap_or(v.len() - 1);
                v[(pos as i64 + delta as i64).rem_euclid(v.len() as i64) as usize]
            }
        }
    }

    /// The first (`last` = false) or last row of an overlay list that is shown.
    pub(crate) fn overlay_edge(&self, l: ListId, n: usize, last: bool) -> usize {
        let v = self
            .list_visible(l)
            .unwrap_or_else(|| (0..n).collect::<Vec<_>>());
        let pick = if last { v.last() } else { v.first() };
        pick.copied().unwrap_or(0)
    }
}

#[cfg(test)]
#[path = "list_filter_tests.rs"]
mod tests;
