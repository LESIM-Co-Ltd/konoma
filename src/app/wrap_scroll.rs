//! The windowed preview while it wraps: its position is (first line, screen rows of that line
//! scrolled past), and every move counts screen rows ([`konoma_widgets::wrap_view`]). A preview
//! that does not wrap never comes here for its arithmetic: [`konoma_widgets::caret::Caret`] and
//! the line-head bytes are all it needs.

use std::collections::HashMap;

use konoma_widgets::wrap_view::{byte_in_line, drawn, Drawn, FileLines, Pos, Wrapped};

use super::*;

/// The lines a made text plugin handed over already colored: they are drawn as they are (no line
/// is cut), so they are what is counted too. The file the reader windows holds the same lines.
fn handed(
    plugin: &Option<plugin_host::PluginShow>,
    generated: bool,
) -> Option<&[konoma_plugin::Line]> {
    if !generated {
        return None;
    }
    match plugin.as_ref()?.view.as_ref()? {
        konoma_plugin::view::View::Text(t) => match &t.source {
            konoma_plugin::view::TextSource::Lines(lines) => Some(lines),
            _ => None,
        },
        _ => None,
    }
}

/// The text of a handed line, as it is drawn.
fn handed_text(line: &konoma_plugin::Line) -> String {
    super::text_plugin::ratatui_line(line.clone())
        .spans
        .iter()
        .map(|s| s.content.as_ref())
        .collect()
}

/// What is put on a line before it is wrapped: the same tab expansion and gutters the draw adds.
struct Dress {
    tab_width: usize,
    numbers: bool,
    marks: HashMap<u32, GutterMark>,
    /// The rows of the view and the lines of the text: the number gutter's width depends on them.
    height: usize,
    total: usize,
}

impl Dress {
    fn body(&self, raw: String) -> Line<'static> {
        crate::preview::code::expand_tabs(vec![Line::from(raw)], self.tab_width)
            .pop()
            .unwrap_or_default()
    }

    /// Line `line` as it is drawn in a window that starts at line `top`.
    fn line(&self, raw: String, top: usize, line: usize) -> Drawn {
        drawn(self.body(raw), |body| {
            let mut lines = vec![body];
            if self.numbers {
                let last = self.total.min(top + self.height);
                lines = with_line_numbers_up_to(lines, line, last);
            }
            with_git_gutter(lines, line, &self.marks)
                .pop()
                .unwrap_or_default()
        })
    }
}

impl App {
    /// The `count` lines from line `first` (at the byte `byte`), as the window draws them.
    fn window_raw(&mut self, byte: u64, first: usize, count: usize) -> Option<Vec<String>> {
        let generated = self.text_plugin_is_generated();
        if let Some(lines) = handed(&self.plugin, generated) {
            return Some(
                lines
                    .iter()
                    .skip(first)
                    .take(count)
                    .map(handed_text)
                    .collect(),
            );
        }
        self.preview_win.as_mut()?.read_lines_kept(byte, count).ok()
    }

    /// Whether the windowed preview wraps and its size is known (it has been drawn). Before the
    /// first draw there are no screen rows to count, so a line is a row.
    pub(super) fn windowed_wraps(&self) -> bool {
        self.cfg.ui.wrap
            && self.tab.preview_viewport_width > 0
            && self.tab.preview_viewport > 0
            && self.preview_win.is_some()
    }

    fn wrap_pos(&self) -> Pos {
        Pos {
            line: self.tab.preview_top_line,
            skip: self.tab.preview_top_skip,
        }
    }

    fn dress(&mut self, total: usize) -> Dress {
        Dress {
            tab_width: self.cfg.ui.tab_width,
            numbers: self.cfg.ui.line_numbers,
            marks: self.git_gutter_marks(),
            height: usize::from(self.tab.preview_viewport.max(1)),
            total,
        }
    }

    /// Moves a wrapping window: `f` is given the screen-row arithmetic, the window and the caret
    /// (line, column), and answers the new ones. False (nothing done) when the preview does not
    /// wrap or its lines cannot be counted.
    pub(super) fn wrap_move(
        &mut self,
        f: impl FnOnce(&mut Wrapped, Pos, usize, usize) -> (Pos, usize, usize),
    ) -> bool {
        if !self.windowed_wraps() {
            return false;
        }
        let Some(total) = self.win_total() else {
            return false;
        };
        let dress = self.dress(total);
        let pos = self.wrap_pos();
        let (line, col) = (self.tab.preview_cursor_line, self.tab.preview_cursor_col);
        let width = self.tab.preview_viewport_width;
        let top_byte = self.tab.preview_byte_top;
        let generated = self.text_plugin_is_generated();
        let made = handed(&self.plugin, generated);
        let Some(win) = self.preview_win.as_mut() else {
            return false;
        };
        let mut lines = FileLines::new(win, total, (pos.line, top_byte));
        let (next, line, col) = {
            let mut source = |top: usize, i: usize| {
                let raw = match made {
                    Some(made) => made.get(i).map(handed_text).unwrap_or_default(),
                    None => lines.get(i),
                };
                dress.line(raw, top, i)
            };
            let mut w = Wrapped::new(total, dress.height, width, &mut source);
            f(&mut w, pos, line, col)
        };
        if next.line != pos.line {
            self.tab.preview_byte_top = lines.offset_of(next.line);
        }
        self.tab.preview_top_line = next.line;
        self.tab.preview_top_skip = next.skip;
        self.tab.preview_cursor_line = line;
        self.tab.preview_cursor_col = col;
        true
    }

    /// `j` / `k` and the jumps to a line: the window follows the caret on line `line`.
    pub(super) fn wrap_follow_line(&mut self, line: usize) -> bool {
        let total = self.win_total().unwrap_or(0);
        self.wrap_move(|w, pos, _, col| {
            let target = line.min(total.saturating_sub(1));
            (w.follow_line(pos, target, col), line, col)
        })
    }

    /// The column keys: the window follows the screen row the caret is on.
    pub(super) fn wrap_follow_column(&mut self) {
        self.wrap_move(|w, pos, line, col| (w.follow_row(pos, line, col), line, col));
    }

    /// A search match in a wrapping preview: its screen row is brought on screen. The window was
    /// already put at the match's line.
    pub(super) fn wrap_show_match(&mut self) {
        if !self.windowed_wraps() {
            return;
        }
        let Some(&(off, line, _)) = self.tab.search_matches.get(self.tab.search_idx) else {
            return;
        };
        let rank = self.tab.search_matches[..self.tab.search_idx]
            .iter()
            .filter(|(_, l, _)| *l == line)
            .count();
        let query = self
            .tab
            .preview_search
            .as_deref()
            .unwrap_or_default()
            .to_lowercase();
        let tab_width = self.cfg.ui.tab_width;
        let Some(raw) = self.window_raw(off, line, 1).and_then(|mut l| l.pop()) else {
            return;
        };
        // The match is found again in the line as drawn (tabs expanded), by its rank in the line,
        // the way the highlight finds it.
        let shown: String = crate::preview::code::expand_tabs(vec![Line::from(raw)], tab_width)
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect::<String>()
            .to_lowercase();
        let found = shown
            .match_indices(&query)
            .nth(rank)
            .map_or(0, |(byte, _)| shown[..byte].chars().count());
        self.wrap_move(|w, pos, cursor, col| (w.follow_row(pos, line, found), cursor, col));
    }

    /// The last window of a wrapping preview: how many lines from the end it starts, the byte of
    /// that line and the rows of it scrolled past. Only the last screenful of lines is read.
    fn wrap_last_window(&mut self) -> Option<(usize, u64, usize)> {
        let height = usize::from(self.tab.preview_viewport.max(1));
        let width = self.tab.preview_viewport_width;
        let generated = self.text_plugin_is_generated();
        let handed_total = handed(&self.plugin, generated).map(<[_]>::len);
        let known_total = handed_total.or(self.preview_total_lines);
        let off = self.preview_win.as_mut()?.last_page_top(height).ok()?;
        // Handed lines are found by number; the file holds as many lines as they are.
        let first_handed = handed_total.map_or(0, |t| t.saturating_sub(height));
        let raw = self.window_raw(off, first_handed, height)?;
        // The number gutter of the last lines is as wide as the line count (a stand-in until the
        // lines have been counted: every key that can reach the end counts them).
        let total = known_total.unwrap_or(self.tab.preview_top_line + height);
        let dress = self.dress(total);
        let first = total.saturating_sub(raw.len());
        let count = raw.len();
        let window_top = total.saturating_sub(height);
        let mut source = |_: usize, i: usize| dress.line(raw[i].clone(), window_top, first + i);
        let pos = Wrapped::new(count, height, width, &mut source).bottom();
        let byte = self
            .preview_win
            .as_mut()?
            .advance_kept(off, pos.line)
            .ok()?
            .0;
        Some((count - pos.line, byte, pos.skip))
    }

    /// [`App::wrap_settle`] when the preview wraps (the draw does it before measuring the extent).
    pub(crate) fn wrap_settle_if_wrapping(&mut self) {
        if self.windowed_wraps() {
            self.wrap_settle();
        }
    }

    /// The screen rows of the first line of the window that are not drawn.
    pub(crate) fn windowed_row_skip(&self) -> u16 {
        u16::try_from(self.tab.preview_top_skip).unwrap_or(u16::MAX)
    }

    /// Keeps a wrapping window inside the text before it is drawn (a resize, a changed file, a
    /// jump near the end). Only the lines of one screen are read unless the end is on screen.
    pub(super) fn wrap_settle(&mut self) {
        let height = usize::from(self.tab.preview_viewport.max(1));
        let width = self.tab.preview_viewport_width;
        let top_byte = self.tab.preview_byte_top;
        let top = self.tab.preview_top_line;
        let Some(raw) = self.window_raw(top_byte, top, height) else {
            return;
        };
        let dress = self.dress(top + raw.len());
        let mut source = |_: usize, i: usize| dress.line(raw[i - top].clone(), top, i);
        let mut w = Wrapped::new(top + raw.len(), height, width, &mut source);
        let rows = w.rows(top, top);
        let skip = self.tab.preview_top_skip.min(rows - 1);
        // Enough rows from here to fill the view: the position stands.
        let mut have = rows - skip;
        let mut next = top + 1;
        while have < height && next < top + raw.len() {
            have += w.rows(top, next);
            next += 1;
        }
        if !raw.is_empty() && (have >= height || (top == 0 && skip == 0)) {
            self.tab.preview_top_skip = skip;
            return;
        }
        // Past the last window.
        let Some((from_end, byte, skip)) = self.wrap_last_window() else {
            return;
        };
        let Some(total) = self.win_total() else {
            return;
        };
        self.tab.preview_byte_top = byte;
        self.tab.preview_top_line = total.saturating_sub(from_end);
        self.tab.preview_top_skip = skip;
    }

    /// The scroll extent of a wrapping window, in bytes with the rows scrolled past a line as the
    /// matching share of its bytes.
    pub(super) fn wrap_scroll_extent(&mut self) -> Option<ScrollExtent> {
        let len = self.preview_win.as_ref()?.len();
        let (from_end, end_byte, end_skip) = self.wrap_last_window()?;
        let generated = self.text_plugin_is_generated();
        let end_line = handed(&self.plugin, generated)
            .map(<[_]>::len)
            .or(self.preview_total_lines)
            .map_or(0, |t| t.saturating_sub(from_end));
        let end = self.wrap_byte_at(end_byte, end_line, end_skip)?;
        let (byte, line) = (self.tab.preview_byte_top, self.tab.preview_top_line);
        let pos = self.wrap_byte_at(byte, line, self.tab.preview_top_skip)?;
        Some(ScrollExtent::new(pos, end, len.saturating_sub(end)))
    }

    fn wrap_byte_at(&mut self, line_byte: u64, line: usize, skip: usize) -> Option<u64> {
        if skip == 0 {
            return Some(line_byte);
        }
        let width = self.tab.preview_viewport_width;
        let height = usize::from(self.tab.preview_viewport.max(1));
        let total = self.preview_total_lines.unwrap_or(0);
        let dress = self.dress(total);
        let next = self
            .preview_win
            .as_mut()?
            .advance_kept(line_byte, 1)
            .ok()?
            .0;
        let raw = self
            .window_raw(line_byte, line, 1)?
            .pop()
            .unwrap_or_default();
        let mut source = |_: usize, _: usize| dress.line(raw.clone(), 0, 0);
        let rows = Wrapped::new(1, height, width, &mut source).rows(0, 0);
        Some(byte_in_line(line_byte, next, skip, rows))
    }

    /// The size the windowed text is about to be drawn at. A wrapping preview whose size changed
    /// keeps the caret on screen (the lines take another number of rows).
    pub(crate) fn set_windowed_size(&mut self, rows: u16, cols: u16) {
        let changed = (self.tab.preview_viewport, self.tab.preview_viewport_width) != (rows, cols);
        self.tab.preview_viewport = rows;
        self.tab.preview_viewport_width = cols;
        if changed {
            self.wrap_keep_caret();
        }
    }

    /// Brings the caret back on screen after the rows of the lines changed, if it was among the
    /// lines of the window.
    pub(super) fn wrap_keep_caret(&mut self) {
        let (top, line) = (self.tab.preview_top_line, self.tab.preview_cursor_line);
        // The first row of the window is on screen as it is (the usual state at a first draw).
        if line == top && self.tab.preview_top_skip == 0 && self.tab.preview_cursor_col == 0 {
            return;
        }
        let height = usize::from(self.tab.preview_viewport.max(1));
        if self.windowed_wraps() && line >= top && line < top + height {
            self.wrap_follow_line(line);
        }
    }
}
