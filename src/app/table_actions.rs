use super::*;

impl App {
    // ---- CSV/TSV table preview ------------------------------------------

    /// Parse the current Table/Archive-kind preview into `table_data` (None on failure = raw-text
    /// fallback / can-not-preview). Does not touch the cursor/scroll (callers reset or restore
    /// those as appropriate).
    pub(super) fn load_table(&mut self) {
        self.table_data = None;
        match self.tab.preview_kind.clone() {
            Some(PreviewKind::Table { path, delimiter }) => {
                if let Ok(t) = crate::preview::table::parse(&path, delimiter) {
                    self.table_data = Some(t);
                }
            }
            // Listing an archive relies on third-party crates (zip/tar). Even an unlikely panic is
            // swallowed by catch_silent, and a failure becomes None (→ safely degrades to [can not preview]).
            Some(PreviewKind::Archive { path, kind }) => {
                self.table_data = crate::preview::markdown::catch_silent(|| {
                    crate::preview::archive::list(&path, kind)
                })
                .and_then(Result::ok);
            }
            _ => {}
        }
    }

    /// Clamp the cell cursor into the table's bounds (after a reload/restore that may have shrunk it).
    pub(super) fn clamp_table_cursor(&mut self) {
        match &self.table_data {
            Some(t) if t.nrows() > 0 && t.ncols > 0 => {
                self.tab.table_cur_row = self.tab.table_cur_row.min(t.nrows() - 1);
                self.tab.table_cur_col = self.tab.table_cur_col.min(t.ncols - 1);
            }
            _ => {
                self.tab.table_cur_row = 0;
                self.tab.table_cur_col = 0;
            }
        }
    }

    /// Whether a CSV/TSV/archive table preview is active **and parsed** (routes the PreviewTable
    /// surface / renderer). A Table/Archive kind whose parse failed returns false → the preview
    /// degrades to raw text (CSV/TSV) or a can-not-preview-style hint (archive).
    pub fn is_table_preview(&self) -> bool {
        matches!(
            self.tab.preview_kind,
            Some(PreviewKind::Table { .. }) | Some(PreviewKind::Archive { .. })
        ) && self.table_data.is_some()
    }

    /// The field-separator byte of the active table (`,` by default).
    fn table_delimiter(&self) -> u8 {
        match self.tab.preview_kind {
            Some(PreviewKind::Table { delimiter, .. }) => delimiter,
            _ => b',',
        }
    }

    /// The parsed table (for the renderer). None when not a table preview.
    pub fn table_data(&self) -> Option<&crate::preview::table::TableData> {
        self.table_data.as_ref()
    }

    /// The cell cursor as (data-row, column), both 0-based.
    pub fn table_cursor(&self) -> (usize, usize) {
        (self.tab.table_cur_row, self.tab.table_cur_col)
    }

    /// The current (top data row, left column) scroll offsets.
    pub fn table_scroll(&self) -> (usize, usize) {
        (self.tab.table_top_row, self.tab.table_left_col)
    }

    /// Renderer feedback: store the scroll offsets it settled on (to keep the cursor visible) plus the
    /// visible data-row count (used as the PageUp/Down step). Mirrors how `preview_scroll`/`preview_viewport`
    /// are clamped/recorded at render time.
    pub fn set_table_view(&mut self, top_row: usize, left_col: usize, viewport_rows: u16) {
        self.tab.table_top_row = top_row;
        self.tab.table_left_col = left_col;
        self.table_viewport_rows = viewport_rows;
    }

    /// Move the cell cursor by (drow, dcol), clamped to the table. The renderer scrolls to follow.
    pub fn table_cursor_move(&mut self, drow: i32, dcol: i32) {
        let Some(t) = &self.table_data else {
            return;
        };
        let (nr, nc) = (t.nrows(), t.ncols);
        if nr == 0 || nc == 0 {
            return;
        }
        let r = (self.tab.table_cur_row as i64 + drow as i64).clamp(0, nr as i64 - 1);
        let c = (self.tab.table_cur_col as i64 + dcol as i64).clamp(0, nc as i64 - 1);
        self.tab.table_cur_row = r as usize;
        self.tab.table_cur_col = c as usize;
    }

    /// Jump to the first (`bottom=false`) or last (`bottom=true`) data row.
    pub fn table_row_to(&mut self, bottom: bool) {
        let Some(t) = &self.table_data else {
            return;
        };
        self.tab.table_cur_row = if bottom {
            t.nrows().saturating_sub(1)
        } else {
            0
        };
    }

    /// Jump to the first (`end=false`) or last (`end=true`) column.
    pub fn table_col_to(&mut self, end: bool) {
        let Some(t) = &self.table_data else {
            return;
        };
        self.tab.table_cur_col = if end { t.ncols.saturating_sub(1) } else { 0 };
    }

    /// Move the cursor down/up by whole pages (`dir` = +1 / -1). The page size is the last render's visible rows.
    pub fn table_page(&mut self, dir: i32) {
        let page = self.table_viewport_rows.max(1) as i32;
        self.table_cursor_move(dir * page, 0);
    }

    /// Move the cursor down/up by half a page (`dir` = +1 / -1).
    pub fn table_half_page(&mut self, dir: i32) {
        let half = (self.table_viewport_rows / 2).max(1) as i32;
        self.table_cursor_move(dir * half, 0);
    }

    /// Build the text a table copy would place on the clipboard (None when there is no table).
    /// Cell = the current cell's value; Row = the current row's cells joined by the delimiter;
    /// Column = the column's header + every cell value, one per line.
    pub(super) fn table_copy_text(&self, kind: TableCopyKind) -> Option<String> {
        let t = self.table_data.as_ref()?;
        let (r, c) = (self.tab.table_cur_row, self.tab.table_cur_col);
        let sep = (self.table_delimiter() as char).to_string();
        Some(match kind {
            TableCopyKind::Cell => {
                if t.nrows() == 0 {
                    t.header(c).to_string()
                } else {
                    t.cell(r, c).to_string()
                }
            }
            TableCopyKind::Row => {
                if t.nrows() == 0 {
                    t.headers.join(&sep)
                } else {
                    t.rows.get(r).map(|row| row.join(&sep)).unwrap_or_default()
                }
            }
            TableCopyKind::Column => {
                let mut vals = vec![t.header(c).to_string()];
                vals.extend(
                    t.rows
                        .iter()
                        .map(|row| row.get(c).cloned().unwrap_or_default()),
                );
                vals.join("\n")
            }
        })
    }

    /// Copy the current cell / row / column to the clipboard and flash the result.
    pub fn table_copy(&mut self, kind: TableCopyKind) {
        let Some(text) = self.table_copy_text(kind) else {
            self.flash = Some(tr(self.lang, crate::i18n::Msg::NoCopyTarget).into());
            return;
        };
        self.set_clipboard_flash(&text);
    }

    /// Case-insensitive cell scan for a table preview, in reading order (row-major).
    /// Only data cells are searched: the cell cursor addresses data rows, so a header-only hit
    /// would have nowhere to jump to.
    pub(super) fn table_search_scan(&mut self, q: &str) {
        self.table_search_hits.clear();
        self.tab.search_matches.clear();
        let needle = q.to_lowercase();
        let Some(t) = self.table_data.as_ref() else {
            return;
        };
        for r in 0..t.nrows() {
            for c in 0..t.ncols {
                if t.cell(r, c).to_lowercase().contains(&needle) {
                    self.tab.search_matches.push((0, r, c));
                    self.table_search_hits.insert((r, c));
                }
            }
        }
    }

    /// Whether this data cell matched the active search (renderer lookup — O(1) per cell).
    pub fn table_cell_is_hit(&self, row: usize, col: usize) -> bool {
        self.table_search_hits.contains(&(row, col))
    }

    // ---- Table cell full-text popup (`Enter` in a table preview) --------------
    // konoma's table grid truncates any cell that doesn't fit the column width with `…`, so while
    // `y→c` can copy it to the clipboard, it can't be read on screen (the copy target is always the
    // raw cell value = no truncation). To fill this gap: an App-global (not per-tab) toggle
    // overlay, the same shape as Outline/Info.

    /// `Enter`: open/close the full-cell popup (mirrors `toggle_outline`/`toggle_info`). The real
    /// trigger is the fixed `Enter` key (`handle_enter`/`handle_esc` in main.rs); `Action::ToggleTableCell`
    /// exists so `q` inside the popup can also close it through the ordinary keymap. Flashes and
    /// stays closed when the table has no columns (an empty file — nothing to show).
    pub fn toggle_table_cell_view(&mut self) {
        if self.table_cell_open {
            self.table_cell_open = false;
            return;
        }
        match &self.table_data {
            Some(t) if t.ncols > 0 => {}
            _ => {
                self.flash = Some(tr(self.lang, crate::i18n::Msg::TableCellEmpty).into());
                return;
            }
        }
        self.table_cell_scroll = 0;
        self.table_cell_open = true;
    }

    /// Whether the full-cell popup is showing.
    pub fn is_table_cell_open(&self) -> bool {
        self.table_cell_open
    }

    /// The full-cell popup's content (the cursor cell, read live so it reflects any reload/move
    /// while open). None when there is no table, or it has no columns (nothing to show).
    pub fn table_cell_view(&self) -> Option<TableCellView> {
        let t = self.table_data.as_ref()?;
        if t.ncols == 0 {
            return None;
        }
        let (r, c) = (self.tab.table_cur_row, self.tab.table_cur_col);
        let text = if t.nrows() == 0 {
            String::new() // a header-only file: the content is empty.
        } else {
            t.cell(r, c).to_string()
        };
        Some(TableCellView {
            header: t.header(c).to_string(),
            row: r + 1,
            col: c + 1,
            nrows: t.nrows(),
            ncols: t.ncols,
            text,
        })
    }

    /// The popup's current vertical scroll offset (wrapped-row units).
    pub fn table_cell_scroll(&self) -> u16 {
        self.table_cell_scroll
    }

    /// Renderer feedback: the clamped scroll and the popup's visible row count (used as the
    /// PageUp/Down step). Mirrors `set_table_view`.
    pub fn set_table_cell_view(&mut self, scroll: u16, viewport: u16) {
        self.table_cell_scroll = scroll;
        self.table_cell_viewport = viewport;
    }

    /// Scroll the popup by `delta` wrapped rows (the upper bound is clamped at render time,
    /// mirroring `preview_scroll`).
    pub fn table_cell_scroll_by(&mut self, delta: i32) {
        let v = self.table_cell_scroll as i32 + delta;
        self.table_cell_scroll = v.max(0) as u16;
    }

    /// To the top (`bottom=false`) or bottom (`bottom=true`, clamped at render time via `u16::MAX`) of the popup.
    pub fn table_cell_scroll_to(&mut self, bottom: bool) {
        self.table_cell_scroll = if bottom { u16::MAX } else { 0 };
    }

    /// Page the popup up/down (`dir` = -1/+1). The page size is the last render's visible rows.
    pub fn table_cell_page(&mut self, dir: i32) {
        let page = self.table_cell_viewport.saturating_sub(1).max(1) as i32;
        self.table_cell_scroll_by(dir * page);
    }
}

/// The full-cell popup's content: the cursor cell's header/position/untruncated text.
/// `row`/`col` are 1-based (display convention, matching the table grid's own title).
pub struct TableCellView {
    pub header: String,
    pub row: usize,
    pub col: usize,
    pub nrows: usize,
    pub ncols: usize,
    pub text: String,
}
