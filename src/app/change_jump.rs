//! `n` / `N` as "the next / previous change" and the line `e` opens a diff at.
//!
//! The keys are layer 1 (`docs/FEATURE-PLUGIN-ARCHITECTURE.md` §10-3-3): while a search is active
//! they step through its matches; otherwise they step through the changes of what is on screen.
//!
//! - the tree steps through the changed files (`jump_changed`, as always);
//! - a windowed text / code preview steps through the change blocks of the git change gutter
//!   (a run of marked lines is one block), moving the caret to the first line of the next block;
//! - a decorated document steps through the blocks that carry a change mark in its gutter,
//!   scrolling to the top of the next block;
//! - a diff steps through its own change blocks (the runs of added / removed lines, or the marked
//!   blocks of its rendered presentation).
//!
//! A step wraps at the end of the file. "Is there anything to step to" is answered from the caches
//! the drawing already filled (the footer asks every frame, and a hint is shown only when the key
//! acts); the key handler computes what it needs itself.

use super::*;

/// Merge change marks (logical-line ranges) into blocks: touching or overlapping ranges are one
/// change. Returns the first logical line of each block, ascending.
fn block_starts(mut ranges: Vec<std::ops::Range<usize>>) -> Vec<usize> {
    ranges.sort_by_key(|r| r.start);
    let mut out: Vec<usize> = Vec::new();
    let mut end = 0usize;
    for r in ranges {
        if out.is_empty() || r.start > end {
            out.push(r.start);
        }
        end = end.max(r.end);
    }
    out
}

/// The change blocks of what is on screen as the scroll rows they start at (ascending), and the
/// last position the view can scroll to (the renderer clamps the scroll to it).
#[derive(Default)]
struct Blocks {
    rows: Vec<usize>,
    max_v: usize,
}

impl App {
    /// The caret line (0-based) of a windowed preview, for the tests.
    #[cfg(all(test, feature = "git"))]
    pub(crate) fn caret_line_for_test(&self) -> usize {
        self.tab.preview_cursor_line
    }

    /// The first line (1-based) of each change block of the git change gutter of the windowed
    /// preview, from the cache the drawing filled.
    fn gutter_block_starts(marks: &std::collections::HashMap<u32, GutterMark>) -> Vec<u32> {
        let mut v: Vec<u32> = marks
            .keys()
            .copied()
            .filter(|&n| n <= 1 || !marks.contains_key(&(n - 1)))
            .collect();
        v.sort_unstable();
        v
    }

    /// Whether the decorated document has any change block: the cheap answer to "is `doc_blocks`
    /// non-empty" (the footer asks every frame, so no range is cloned or sorted for it).
    fn doc_has_changes(&self) -> bool {
        let Some(c) = self.md_cache.as_ref() else {
            return false;
        };
        if self.tab.preview_path.as_ref() != Some(&c.path) {
            return false;
        }
        match c.source {
            MdCacheSource::File => !c.preview_gutter_marks.is_empty(),
            MdCacheSource::Diff => !c.diff_marks.is_empty(),
        }
    }

    /// The decorated document's change blocks as the scroll rows they start at (needs the layout
    /// the drawing built), and the last scroll position the view can take.
    fn doc_blocks(&self) -> Blocks {
        let Some(c) = self.md_cache.as_ref() else {
            return Blocks::default();
        };
        if self.tab.preview_path.as_ref() != Some(&c.path) {
            return Blocks::default();
        }
        let ranges: Vec<_> = match c.source {
            MdCacheSource::File => c
                .preview_gutter_marks
                .iter()
                .map(|(r, _)| r.clone())
                .collect(),
            MdCacheSource::Diff => c.diff_marks.iter().map(|(r, _)| r.clone()).collect(),
        };
        Blocks {
            rows: block_starts(ranges)
                .into_iter()
                .map(|l| self.md_visual_span(l).0)
                .collect(),
            max_v: self.slide_scroll_limit(self.md_view_rows, self.tab.preview_viewport as usize),
        }
    }

    /// The block the view is on: the one the last `n` / `N` landed on while the view has not moved
    /// since (the last screenful cannot scroll a late block to the top, so the position alone
    /// would not tell two such blocks apart), else the last block at or above the top of the view.
    fn current_block(&self, b: &Blocks) -> Option<usize> {
        let scroll = self.tab.preview_scroll as usize;
        if let Some((p, i)) = &self.change_at {
            if Some(p) == self.tab.preview_path.as_ref()
                && b.rows.get(*i).is_some_and(|&r| r.min(b.max_v) == scroll)
            {
                return Some(*i);
            }
        }
        b.rows.iter().rposition(|&r| r <= scroll)
    }

    /// Scroll to the next / previous block of `b` (wrapping) and remember which it is.
    fn scroll_to_block(&mut self, b: &Blocks, dir: i32) {
        let n = b.rows.len();
        if n == 0 {
            self.flash = Some(tr(self.lang, crate::i18n::Msg::NoChangesInFile).into());
            return;
        }
        let scroll = self.tab.preview_scroll as usize;
        let known = self
            .current_block(b)
            .filter(|&i| b.rows[i].min(b.max_v) == scroll);
        let target = match known {
            Some(i) => (i as i64 + dir.signum() as i64).rem_euclid(n as i64) as usize,
            None if dir >= 0 => b.rows.iter().position(|&r| r > scroll).unwrap_or(0),
            None => b.rows.iter().rposition(|&r| r < scroll).unwrap_or(n - 1),
        };
        self.tab.preview_scroll = b.rows[target].min(b.max_v).min(u16::MAX as usize) as u16;
        self.change_at = self.tab.preview_path.clone().map(|p| (p, target));
    }

    /// Whether the preview on screen has a change `n` / `N` can step to, answered from cached state
    /// only (no process, no file system).
    pub(crate) fn preview_has_changes(&self) -> bool {
        if self.is_windowed() {
            let Some(path) = self.tab.preview_path.as_ref() else {
                return false;
            };
            return self
                .gutter_cache
                .as_ref()
                .is_some_and(|c| &c.path == path && !c.marks.is_empty());
        }
        if self.shows_document() {
            return self.doc_has_changes();
        }
        false
    }

    /// `n` / `N` of a text or document preview while no search is active: move to the next /
    /// previous change block (`dir` = +1 / -1), wrapping. Flashes when the file has no change.
    pub fn change_jump(&mut self, dir: i32) {
        if self.is_windowed() {
            let marks = self.git_gutter_marks();
            let starts = Self::gutter_block_starts(&marks);
            let cur = (self.tab.preview_cursor_line + 1) as u32;
            let target = if dir >= 0 {
                starts
                    .iter()
                    .copied()
                    .find(|&s| s > cur)
                    .or(starts.first().copied())
            } else {
                starts
                    .iter()
                    .rev()
                    .copied()
                    .find(|&s| s < cur)
                    .or(starts.last().copied())
            };
            match target {
                Some(l) => self.preview_goto_line(l as usize),
                None => self.flash = Some(tr(self.lang, crate::i18n::Msg::NoChangesInFile).into()),
            }
            return;
        }
        let blocks = self.doc_blocks();
        self.scroll_to_block(&blocks, dir);
    }

    // --- diff ---

    /// The change blocks of the diff on screen as scroll rows, and the last scroll position.
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    fn diff_blocks(&self) -> Blocks {
        if self.diff_rendered_active() {
            return self.doc_blocks();
        }
        let Some(c) = self.diff_cache.as_ref() else {
            return Blocks::default();
        };
        let split = self.diff_split_drawn;
        let total = if split {
            crate::preview::gitdiff::side_by_side_row_count(&c.lines, self.current_preview_ext())
        } else {
            c.lines.len()
        };
        Blocks {
            rows: crate::preview::gitdiff::change_block_starts(&c.lines)
                .into_iter()
                .map(|i| crate::preview::gitdiff::scroll_row_of_line(&c.lines, i, split))
                .collect(),
            max_v: total.saturating_sub(self.tab.preview_viewport as usize),
        }
    }

    /// The scroll rows `n` / `N` step through in the diff on screen, for the tests.
    #[cfg(all(test, feature = "git"))]
    pub(crate) fn diff_block_rows_for_test(&self) -> Vec<usize> {
        self.diff_blocks().rows
    }

    /// Whether the diff on screen has a change block `n` / `N` can step to (cheap; never the side
    /// by side view of pictures, which has nothing to scroll).
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub(crate) fn diff_has_change_blocks(&self) -> bool {
        if self.diff_media_active() {
            return false;
        }
        if self.diff_rendered_active() {
            return self.doc_has_changes();
        }
        self.diff_cache.as_ref().is_some_and(|c| {
            c.lines
                .iter()
                .any(|l| l.kind != crate::git::DiffLineKind::Context)
        })
    }

    /// `n` / `N` in a diff: scroll to the next / previous change block of the file (wraps).
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn diff_change_jump(&mut self, dir: i32) {
        if self.diff_media_active() {
            return;
        }
        let blocks = self.diff_blocks();
        self.scroll_to_block(&blocks, dir);
    }

    /// The line (1-based, new file) `e` opens the editor at in a diff: the change block at or
    /// above the top of the view (the first block when the view is above them all) — the block
    /// `n` / `N` last landed on. `None` for the rendered and picture presentations, which have no
    /// line of the file to name.
    #[cfg(feature = "git")]
    pub(super) fn diff_edit_line(&self) -> Option<usize> {
        if self.diff_media_active() || self.diff_rendered_active() {
            return None;
        }
        let c = self.diff_cache.as_ref()?;
        let starts = crate::preview::gitdiff::change_block_starts(&c.lines);
        let blocks = self.diff_blocks();
        let pick = self
            .current_block(&blocks)
            .or(Some(0))
            .filter(|&i| i < starts.len())?;
        crate::preview::gitdiff::block_new_line(&c.lines, starts[pick]).map(|n| n as usize)
    }
}

#[cfg(test)]
#[path = "change_jump_tests.rs"]
mod tests;
