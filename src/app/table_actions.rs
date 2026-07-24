use super::*;

impl App {
    // ---- CSV/TSV テーブルプレビュー ------------------------------------------

    /// Parse the current Table-kind preview into `table_data` (None on failure = raw-text fallback).
    /// Does not touch the cursor/scroll (callers reset or restore those as appropriate).
    pub(super) fn load_table(&mut self) {
        self.table_data = None;
        if let Some(PreviewKind::Table { path, delimiter }) = self.preview_kind.clone() {
            if let Ok(t) = crate::preview::table::parse(&path, delimiter) {
                self.table_data = Some(t);
            }
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

    /// Whether a CSV/TSV table preview is active **and parsed** (routes the PreviewTable surface / renderer).
    /// A Table kind whose parse failed returns false → the preview degrades to raw text.
    pub fn is_table_preview(&self) -> bool {
        matches!(self.preview_kind, Some(PreviewKind::Table { .. })) && self.table_data.is_some()
    }

    /// The field-separator byte of the active table (`,` by default).
    fn table_delimiter(&self) -> u8 {
        match self.preview_kind {
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
}
