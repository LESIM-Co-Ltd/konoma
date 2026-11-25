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
#[cfg(feature = "git")]
use konoma_plugin::host::project::diff_blocks::source_diff_edit_line;
use konoma_plugin::host::project::diff_blocks::{
    block_starts, source_diff_blocks, step_change_block, DiffBlockCursor, DiffBlocks as Blocks,
};

impl App {
    /// The caret line (0-based) of a windowed preview, for the tests.
    #[cfg(all(test, feature = "git"))]
    pub(crate) fn caret_line_for_test(&self) -> usize {
        self.tab.preview_cursor_line
    }

    /// The first line (1-based) of each change block of the git change gutter of the windowed
    /// preview, from the cache the drawing filled.
    fn gutter_block_starts(marks: &std::collections::HashMap<u32, GutterMark>) -> Vec<u32> {
        konoma_plugin::host::project::diff_blocks::marked_line_block_starts(marks)
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
        self.block_cursor().current_block(b)
    }

    fn block_cursor(&self) -> DiffBlockCursor {
        DiffBlockCursor {
            scroll: self.tab.preview_scroll as usize,
            block: self
                .change_at
                .as_ref()
                .filter(|(path, _)| Some(path) == self.tab.preview_path.as_ref())
                .map(|(_, index)| *index),
        }
    }

    /// Scroll to the next / previous block of `b` (wrapping) and remember which it is.
    fn scroll_to_block(&mut self, b: &Blocks, dir: i32) {
        let cursor = DiffBlockCursor {
            block: self.current_block(b),
            ..self.block_cursor()
        };
        let Some(target) = step_change_block(b, cursor, dir) else {
            self.flash = Some(tr(self.lang, crate::i18n::Msg::NoChangesInFile).into());
            return;
        };
        self.tab.preview_scroll = target.scroll.min(u16::MAX as usize) as u16;
        self.change_at = self.tab.preview_path.clone().zip(target.block);
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
        source_diff_blocks(
            &c.lines,
            self.current_preview_ext(),
            self.diff_split_drawn,
            self.tab.preview_viewport as usize,
        )
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
        let blocks = self.diff_blocks();
        source_diff_edit_line(&c.lines, &blocks, self.block_cursor())
    }
}

#[cfg(test)]
#[path = "change_jump_tests.rs"]
mod tests;

#[cfg(all(test, feature = "git"))]
#[path = "file_diff_blocks_bridge_tests.rs"]
mod bridge_tests;
