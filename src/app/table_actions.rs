use super::*;
use crate::preview::office::{CellType, NumFmtRef};
use crate::preview::table::{cell_address, Grid, TableData};

/// What the table renderer shows right now, built from borrowed *fields* (not `&self`) so callers
/// can keep reading the grid while writing a different field such as `tab.table_cur_row`.
/// A loaded workbook wins (it is only ever set while a spreadsheet preview is active); otherwise
/// the CSV/TSV/archive table. `None` while loading, after a failed load, or with no visible sheet.
fn grid_of<'a>(
    table: &'a Option<TableData>,
    workbook: &'a Option<Box<crate::preview::office::Workbook>>,
    sheet_idx: usize,
) -> Option<Grid<'a>> {
    if let Some(wb) = workbook {
        return wb.sheets.get(sheet_idx).map(Grid::Sheet);
    }
    table.as_ref().map(Grid::Csv)
}

/// A search query for table cells: case-insensitive "contains", without allocating per cell.
/// (`cell.to_lowercase().contains(..)` allocated a string for every cell: 60-100 ms on a
/// maximum-size sheet, on the UI thread.)
struct Needle {
    /// The query lower-cased, as characters.
    chars: Vec<char>,
    /// The same as bytes when it is all ASCII (the common case: a byte search, no decoding).
    ascii: Option<Vec<u8>>,
}

impl Needle {
    fn new(q: &str) -> Needle {
        let lower = q.to_lowercase();
        let ascii = lower.is_ascii().then(|| lower.clone().into_bytes());
        Needle {
            chars: lower.chars().collect(),
            ascii,
        }
    }

    /// Whether `hay` contains the query, ignoring case. An all-ASCII haystack is searched as
    /// bytes; otherwise characters are lower-cased one at a time (full Unicode lower-casing, so
    /// `É` matches `é` and the Kelvin sign matches `k`).
    fn found_in(&self, hay: &str) -> bool {
        if self.chars.is_empty() {
            return true;
        }
        if hay.is_ascii() {
            // A non-ASCII query cannot match an ASCII-only text.
            let Some(n) = self.ascii.as_deref() else {
                return false;
            };
            let h = hay.as_bytes();
            let first = n[0];
            if h.len() < n.len() {
                return false;
            }
            return (0..=h.len() - n.len()).any(|i| {
                h[i].to_ascii_lowercase() == first && h[i..i + n.len()].eq_ignore_ascii_case(n)
            });
        }
        hay.char_indices().any(|(i, _)| self.starts_at(&hay[i..]))
    }

    /// Whether the lower-cased characters of `s` begin with the query.
    fn starts_at(&self, s: &str) -> bool {
        let mut lowered = s.chars().flat_map(char::to_lowercase);
        self.chars.iter().all(|&n| lowered.next() == Some(n))
    }
}

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
        // A spreadsheet that is still loading (or failed) has nothing to clamp against yet: keep the
        // saved sheet number and cursor so they apply when the workbook lands.
        if matches!(self.tab.preview_kind, Some(PreviewKind::Spreadsheet(_)))
            && self.workbook.is_none()
        {
            return;
        }
        if let Some(wb) = &self.workbook {
            self.tab.sheet_idx = self.tab.sheet_idx.min(wb.sheets.len().saturating_sub(1));
        }
        let dims = self.grid().map(|g| (g.nrows(), g.ncols()));
        match dims {
            Some((nr, nc)) if nr > 0 && nc > 0 => {
                self.tab.table_cur_row = self.tab.table_cur_row.min(nr - 1);
                self.tab.table_cur_col = self.tab.table_cur_col.min(nc - 1);
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
            Some(
                PreviewKind::Table { .. }
                    | PreviewKind::Archive { .. }
                    | PreviewKind::Spreadsheet(_)
            )
        ) && self.grid().is_some()
    }

    /// Sets what the preview shows. **The one place a preview kind is assigned**: the parsed
    /// spreadsheet is App-level state that can hold hundreds of MB, so whenever the preview is
    /// not a spreadsheet (a git diff, the tree, a diagram, another file) it is released here.
    /// This keeps the invariant "`workbook` is `Some` only while the kind is `Spreadsheet`"
    /// (`App::workbook_matches_preview`) true after every operation, instead of each caller
    /// remembering to reset it.
    pub(super) fn set_preview_kind(&mut self, kind: Option<PreviewKind>) {
        if !matches!(kind, Some(PreviewKind::Spreadsheet(_))) {
            self.workbook = None;
            self.workbook_error = None;
        }
        self.tab.preview_kind = kind;
    }

    /// The invariant [`App::set_preview_kind`] keeps: a loaded workbook (or its error) exists only
    /// while the preview is a spreadsheet.
    #[cfg(test)]
    pub(crate) fn workbook_matches_preview(&self) -> bool {
        matches!(self.tab.preview_kind, Some(PreviewKind::Spreadsheet(_)))
            || (self.workbook.is_none() && self.workbook_error.is_none())
    }

    /// The field-separator byte of the active table (`,` by default). A spreadsheet never gets
    /// here: its copy is built from the cells (tab-joined) in `table_copy_text`.
    fn table_delimiter(&self) -> u8 {
        match self.tab.preview_kind {
            Some(PreviewKind::Table { delimiter, .. }) => delimiter,
            _ => b',',
        }
    }

    /// The parsed CSV/TSV/archive table (None for a spreadsheet or when not a table preview).
    /// Prefer [`App::grid`] in code that should work for both.
    #[cfg(test)]
    pub fn table_data(&self) -> Option<&crate::preview::table::TableData> {
        self.table_data.as_ref()
    }

    /// What the table renderer shows: the CSV/TSV/archive table, or the current sheet of the loaded
    /// spreadsheet. None while a workbook is still loading / failed to load / has no visible sheet.
    pub fn grid(&self) -> Option<Grid<'_>> {
        grid_of(&self.table_data, &self.workbook, self.tab.sheet_idx)
    }

    /// Whether a spreadsheet preview is on screen (a sheet is showing).
    pub fn is_sheet_preview(&self) -> bool {
        matches!(self.tab.preview_kind, Some(PreviewKind::Spreadsheet(_)))
            && matches!(self.grid(), Some(Grid::Sheet(_)))
    }

    /// True while a spreadsheet's worker has not delivered yet and nothing is showing: the
    /// "loading" screen (a reload keeps the previous workbook on screen instead).
    pub fn is_sheet_loading(&self) -> bool {
        matches!(self.tab.preview_kind, Some(PreviewKind::Spreadsheet(_)))
            && self.workbook.is_none()
            && self.workbook_error.is_none()
            && self.media_loading
    }

    /// Why the spreadsheet could not be shown (None while loading, or on success).
    pub fn sheet_error(&self) -> Option<&crate::preview::office::OfficeError> {
        self.workbook_error.as_ref()
    }

    /// Whether the loaded spreadsheet has more than one visible sheet — the single predicate behind
    /// `J`/`K`, the footer hint and the help row (a hint shows only when its key acts).
    pub fn sheet_can_switch(&self) -> bool {
        self.is_sheet_preview() && self.workbook.as_ref().is_some_and(|w| w.sheets.len() > 1)
    }

    /// `(name, 1-based index, sheet count, hidden sheet count)` of the sheet on screen.
    pub fn sheet_info(&self) -> Option<(&str, usize, usize, usize)> {
        let wb = self.workbook.as_ref()?;
        let sheet = wb.sheets.get(self.tab.sheet_idx)?;
        Some((
            sheet.name.as_str(),
            self.tab.sheet_idx + 1,
            wb.sheets.len(),
            wb.hidden_sheets,
        ))
    }

    /// `J`: the next sheet (stops at the last one, like the PDF page keys). The cursor and scroll
    /// go back to A1 — a sheet is a different grid.
    pub fn sheet_next(&mut self) {
        self.sheet_goto(self.tab.sheet_idx.saturating_add(1));
    }

    /// `K`: the previous sheet (stops at the first one).
    pub fn sheet_prev(&mut self) {
        self.sheet_goto(self.tab.sheet_idx.saturating_sub(1));
    }

    fn sheet_goto(&mut self, idx: usize) {
        if !self.sheet_can_switch() {
            return;
        }
        let Some(wb) = &self.workbook else {
            return;
        };
        let idx = idx.min(wb.sheets.len() - 1);
        if idx == self.tab.sheet_idx {
            return;
        }
        self.tab.sheet_idx = idx;
        self.tab.table_cur_row = 0;
        self.tab.table_cur_col = 0;
        self.tab.table_top_row = 0;
        self.tab.table_left_col = 0;
        self.table_cell_open = false;
        // The old sheet's match cells mean nothing here: re-run an active search on the new sheet.
        self.tab.search_matches.clear();
        self.tab.search_idx = 0;
        if let Some(q) = self.tab.preview_search.clone() {
            self.table_search_scan(&q);
        }
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
        let Some(g) = self.grid() else {
            return;
        };
        let (nr, nc) = (g.nrows(), g.ncols());
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
        let Some(nr) = self.grid().map(|g| g.nrows()) else {
            return;
        };
        self.tab.table_cur_row = if bottom { nr.saturating_sub(1) } else { 0 };
    }

    /// Jump to the first (`end=false`) or last (`end=true`) column.
    pub fn table_col_to(&mut self, end: bool) {
        let Some(nc) = self.grid().map(|g| g.ncols()) else {
            return;
        };
        self.tab.table_cur_col = if end { nc.saturating_sub(1) } else { 0 };
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
        let g = self.grid()?;
        let (r, c) = (self.tab.table_cur_row, self.tab.table_cur_col);
        // A sheet copies the text Excel shows (never the raw value, never the row-number gutter or
        // the column letters — those are not data).
        let t = match g {
            Grid::Sheet(sheet) => {
                return Some(match kind {
                    TableCopyKind::Cell => sheet.display(r, c).to_string(),
                    // Tab-separated, up to the last non-empty cell (no trailing tabs).
                    TableCopyKind::Row => match sheet
                        .row_cells(r)
                        .iter()
                        .rev()
                        .find(|(_, cell)| !cell.display().is_empty())
                    {
                        Some(&(last, _)) => (0..=last as usize)
                            .map(|c| sheet.display(r, c))
                            .collect::<Vec<_>>()
                            .join("\t"),
                        None => String::new(),
                    },
                    TableCopyKind::Column => (0..sheet.nrows)
                        .map(|r| sheet.display(r, c))
                        .collect::<Vec<_>>()
                        .join("\n"),
                });
            }
            Grid::Csv(t) => t,
        };
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
        self.tab.search_matches.clear();
        let needle = Needle::new(q);
        let Some(g) = grid_of(&self.table_data, &self.workbook, self.tab.sheet_idx) else {
            return;
        };
        match g {
            // A sheet is sparse (up to 16k columns): walk only the cells that exist, in reading
            // order, and match the displayed text.
            Grid::Sheet(sheet) => {
                for r in 0..sheet.nrows {
                    for (c, cell) in sheet.row_cells(r) {
                        let shown = cell.display();
                        if !shown.is_empty() && needle.found_in(shown) {
                            self.tab.search_matches.push((0, r, *c as usize));
                        }
                    }
                }
            }
            Grid::Csv(t) => {
                for r in 0..t.nrows() {
                    for c in 0..t.ncols {
                        if needle.found_in(t.cell(r, c)) {
                            self.tab.search_matches.push((0, r, c));
                        }
                    }
                }
            }
        }
    }

    /// Whether this data cell matched the active search (renderer lookup). The matches of a table
    /// are recorded in reading order (row-major), so this is a binary search of `search_matches`
    /// itself: no second copy of a (possibly multi-million) result set to build, and nothing that
    /// could outlive a tab switch (`search_matches` is per tab).
    pub fn table_cell_is_hit(&self, row: usize, col: usize) -> bool {
        matches!(
            self.tab.preview_kind,
            Some(
                PreviewKind::Table { .. }
                    | PreviewKind::Archive { .. }
                    | PreviewKind::Spreadsheet(_)
            )
        ) && self
            .tab
            .search_matches
            .binary_search(&(0, row, col))
            .is_ok()
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
        match self.grid() {
            Some(g) if g.ncols() > 0 => {}
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
        let g = self.grid()?;
        if g.ncols() == 0 {
            return None;
        }
        let (r, c) = (self.tab.table_cur_row, self.tab.table_cur_col);
        let text = match g {
            // A spreadsheet cell: the address, what Excel shows, and everything behind it.
            Grid::Sheet(sheet) => self.sheet_cell_detail(sheet, r, c),
            Grid::Csv(t) if t.nrows() == 0 => String::new(), // a header-only file: the content is empty.
            Grid::Csv(t) => t.cell(r, c).to_string(),
        };
        Some(TableCellView {
            // For a sheet the "header" is the cell address (`B3`), the natural name of the cell.
            header: match g {
                Grid::Sheet(_) => cell_address(r, c),
                Grid::Csv(_) => g.header(c).to_string(),
            },
            row: r + 1,
            col: c + 1,
            nrows: g.nrows(),
            ncols: g.ncols(),
            text,
        })
    }

    /// The spreadsheet cell popup body: address, displayed text, raw value, type, formula (when the
    /// cell has one) and number-format code (when it is not General). One `label: value` per line.
    fn sheet_cell_detail(
        &self,
        sheet: &crate::preview::office::Sheet,
        r: usize,
        c: usize,
    ) -> String {
        use crate::i18n::Msg;
        let lang = self.lang;
        let mut lines = vec![
            format!(
                "{}: {}",
                tr(lang, Msg::SheetCellAddress),
                cell_address(r, c)
            ),
            format!(
                "{}: {}",
                tr(lang, Msg::SheetCellDisplayed),
                sheet.display(r, c)
            ),
        ];
        if let Some(cell) = sheet.cell(r, c) {
            lines.push(format!(
                "{}: {}",
                tr(lang, Msg::SheetCellRaw),
                cell.raw_text()
            ));
            let ty = match cell.cell_type() {
                CellType::Number => Msg::SheetTypeNumber,
                CellType::Text => Msg::SheetTypeText,
                CellType::Bool => Msg::SheetTypeBool,
                CellType::Error => Msg::SheetTypeError,
                CellType::DateTime => Msg::SheetTypeDateTime,
            };
            lines.push(format!(
                "{}: {}",
                tr(lang, Msg::SheetCellType),
                tr(lang, ty)
            ));
            if let Some(f) = sheet.formula(r, c) {
                lines.push(format!("{}: ={f}", tr(lang, Msg::SheetCellFormula)));
            }
            let locale = match lang {
                crate::i18n::Lang::Jp => crate::preview::office::Locale::Ja,
                crate::i18n::Lang::En => crate::preview::office::Locale::En,
            };
            let code = self
                .workbook
                .as_ref()
                .and_then(|wb| match wb.format_of(cell) {
                    NumFmtRef::General => None,
                    NumFmtRef::Custom(code) => Some(code.to_string()),
                    NumFmtRef::Builtin(n) => Some(
                        crate::preview::office::numfmt::builtin_format_code(u32::from(*n), locale)
                            .map(str::to_string)
                            .unwrap_or_else(|| format!("#{n}")),
                    ),
                });
            if let Some(code) = code.filter(|c| c != "General") {
                lines.push(format!("{}: {code}", tr(lang, Msg::SheetCellFormat)));
            }
        } else {
            // No stored cell: still say it is empty (the type line is the one place that does).
            lines.push(format!(
                "{}: {}",
                tr(lang, Msg::SheetCellType),
                tr(lang, Msg::SheetTypeEmpty)
            ));
        }
        lines.join("\n")
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

#[cfg(test)]
mod needle_tests {
    use super::Needle;

    /// What the search used to do (allocating per cell).
    fn old(hay: &str, q: &str) -> bool {
        hay.to_lowercase().contains(&q.to_lowercase())
    }

    #[test]
    fn needle_matches_exactly_what_to_lowercase_contains_did() {
        // (A word-final `Σ` is the one place where `str::to_lowercase` looks at context (`ς`);
        // lower-casing one character at a time gives `σ`. Not in the corpus on purpose.)
        let hays = [
            "",
            "a",
            "Hello World",
            "HELLO",
            "hello",
            "x-Ray 123",
            "ÉCOLE",
            "école",
            "Straße",
            "STRASSE",
            "Kelvin \u{212A}",
            "İstanbul",
            "日本語テキスト",
            "ＡＢＣ ａｂｃ",
            "Ωmega ω",
            "mixed ÀÉÎ and abc",
            "tab\tnew\nline",
            "😀 smile",
            "ǅ title",
        ];
        let needles = [
            "",
            "a",
            "A",
            "hello",
            "WORLD",
            "o w",
            "ray",
            "123",
            "é",
            "É",
            "ecole",
            "école",
            "ß",
            "ss",
            "k",
            "K",
            "i",
            "i\u{307}",
            "istanbul",
            "語",
            "テキ",
            "ａｂｃ",
            "ＡＢＣ",
            "ω",
            "Ω",
            "àéî",
            "AND",
            "\t",
            "\n",
            "😀",
            "ǆ",
            "Ǆ",
            "zzzz",
            "hello world and more",
        ];
        for h in hays {
            for q in needles {
                assert_eq!(
                    Needle::new(q).found_in(h),
                    old(h, q),
                    "hay {h:?} needle {q:?}"
                );
            }
        }
    }

    #[test]
    fn needle_handles_edges() {
        let n = Needle::new("ab");
        assert!(!n.found_in("a"));
        assert!(n.found_in("ab"));
        assert!(n.found_in("xxAB"));
        assert!(n.found_in("ABxx"));
        assert!(!n.found_in("aXb"));
        // A non-ASCII query never matches ASCII text; the empty query matches everything.
        assert!(!Needle::new("é").found_in("e"));
        assert!(Needle::new("").found_in("anything"));
        assert!(Needle::new("").found_in(""));
    }
}
