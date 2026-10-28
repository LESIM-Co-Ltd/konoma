//! A table view for any ratatui app: an aligned grid with a fixed header row, per-column
//! ("rainbow") colors, a highlighted cell cursor, an optional row-number gutter, and a popup that
//! shows one cell's full text.
//!
//! The data comes through [`TableSource`]; what the app keeps between frames is a
//! [`TableViewState`]. Column widths are measured from the header plus the rows on screen (like
//! csvlens), so opening is instant for a large table. The drawing settles the scroll offsets so the
//! cursor is visible and writes them back into the state. Everything the widget shows that is text
//! (title, empty-table label, popup title and footer) is passed in, so the widget knows nothing of
//! the app's language or configuration.

use std::borrow::Cow;

use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph, Widget, Wrap};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Rotating palette for rainbow columns. Mid-tone hues that read on both light and dark terminals
/// (the same idea as Rainbow CSV / csvlens: color = which column).
const RAINBOW: [Color; 7] = [
    Color::Rgb(102, 178, 255), // blue
    Color::Rgb(120, 200, 120), // green
    Color::Rgb(224, 176, 92),  // amber
    Color::Rgb(200, 130, 220), // purple
    Color::Rgb(110, 200, 200), // teal
    Color::Rgb(226, 138, 138), // red
    Color::Rgb(190, 190, 120), // olive
];

/// Max display width any single column is given (wider cells are truncated with `…`).
pub const MAX_COL_W: usize = 40;
/// Min column width so short/empty columns still show a cell.
pub const MIN_COL_W: usize = 3;
/// Spaces between columns.
pub const COL_GAP: usize = 1;

/// A read-only table the view draws. "Rows" are data rows: a header record is exposed through
/// [`TableSource::header`], not as row 0.
pub trait TableSource {
    /// Number of data rows.
    fn nrows(&self) -> usize;
    /// Number of columns.
    fn ncols(&self) -> usize;
    /// The header text of `col` (empty when there is none).
    fn header(&self, col: usize) -> Cow<'_, str>;
    /// The text of a data cell (`""` for an empty or out-of-range cell). Borrow it when the table
    /// keeps its cells as text: every visible cell is read at every frame.
    fn cell(&self, row: usize, col: usize) -> Cow<'_, str>;
}

/// What the app keeps between frames: the cell cursor and the scroll position.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TableViewState {
    /// Cursor data row, 0-based.
    pub cursor_row: usize,
    /// Cursor column, 0-based.
    pub cursor_col: usize,
    /// First data row on screen.
    pub top: usize,
    /// First column on screen.
    pub left: usize,
    /// How many data rows the last drawing showed at most (the PageUp/Down step).
    pub viewport_rows: u16,
}

impl TableViewState {
    /// Moves the cursor by `(drow, dcol)`, clamped to a `nrows` x `ncols` table. An empty table
    /// (no rows or no columns) leaves it where it is. The drawing scrolls to follow.
    pub fn move_cursor(&mut self, drow: i32, dcol: i32, nrows: usize, ncols: usize) {
        if nrows == 0 || ncols == 0 {
            return;
        }
        let r = (self.cursor_row as i64 + drow as i64).clamp(0, nrows as i64 - 1);
        let c = (self.cursor_col as i64 + dcol as i64).clamp(0, ncols as i64 - 1);
        self.cursor_row = r as usize;
        self.cursor_col = c as usize;
    }
}

/// One step of the cell cursor (a table's `Motion`s).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TableStep {
    Up,
    Down,
    Left,
    Right,
    /// First data row.
    FirstRow,
    /// Last data row.
    LastRow,
    /// First column.
    FirstCol,
    /// Last column.
    LastCol,
    PageDown,
    PageUp,
    HalfDown,
    HalfUp,
}

impl TableViewState {
    /// Moves the cursor by one `step` in a `nrows` x `ncols` table. A page is the rows the last
    /// drawing showed (`viewport_rows`, at least 1) and half a page is half of that (at least 1).
    /// `FirstRow` / `LastRow` / `FirstCol` / `LastCol` jump even when the table is empty (they land
    /// on 0 then); the other steps leave an empty table alone.
    pub fn step(&mut self, step: TableStep, nrows: usize, ncols: usize) {
        let page = i32::from(self.viewport_rows).max(1);
        let half = (i32::from(self.viewport_rows) / 2).max(1);
        match step {
            TableStep::Up => self.move_cursor(-1, 0, nrows, ncols),
            TableStep::Down => self.move_cursor(1, 0, nrows, ncols),
            TableStep::Left => self.move_cursor(0, -1, nrows, ncols),
            TableStep::Right => self.move_cursor(0, 1, nrows, ncols),
            TableStep::FirstRow => self.cursor_row = 0,
            TableStep::LastRow => self.cursor_row = nrows.saturating_sub(1),
            TableStep::FirstCol => self.cursor_col = 0,
            TableStep::LastCol => self.cursor_col = ncols.saturating_sub(1),
            TableStep::PageDown => self.move_cursor(page, 0, nrows, ncols),
            TableStep::PageUp => self.move_cursor(-page, 0, nrows, ncols),
            TableStep::HalfDown => self.move_cursor(half, 0, nrows, ncols),
            TableStep::HalfUp => self.move_cursor(-half, 0, nrows, ncols),
        }
    }
}

/// Spreadsheet column letters for a 0-based column: `0 -> A`, `25 -> Z`, `26 -> AA`, `16383 -> XFD`.
pub fn column_letters(col: usize) -> String {
    let mut n = col + 1;
    let mut out = Vec::new();
    while n > 0 {
        n -= 1;
        out.push(b'A' + (n % 26) as u8);
        n /= 26;
    }
    out.reverse();
    String::from_utf8(out).unwrap_or_default()
}

/// The A1-style address of a 0-based cell (`(2, 1) -> "B3"`).
pub fn cell_address(row: usize, col: usize) -> String {
    format!("{}{}", column_letters(col), row + 1)
}

/// The frame title of a table: `path`, the size, the cursor and a note when the table was cut.
///
/// `sheet` is `Some(note)` for a table with spreadsheet addresses (the note is the sheet's name
/// and position, or what the plugin gave): the title then names the cursor cell by its address
/// (` book.xlsx  Sales (1/3) +1 hidden  B3  5×6  (capped) `). Else it counts rows and columns
/// (` data.csv  r3/100 c2/5  100×5  (capped) `). `capped` is the word for a table cut at a limit.
pub fn table_title(
    path: &str,
    (nrows, ncols): (usize, usize),
    (cur_row, cur_col): (usize, usize),
    sheet: Option<&str>,
    capped: Option<&str>,
) -> String {
    let cap = capped.map(|w| format!("  ({w})")).unwrap_or_default();
    if let Some(note) = sheet {
        let addr = if nrows > 0 && ncols > 0 {
            format!("  {}", cell_address(cur_row, cur_col))
        } else {
            String::new()
        };
        return format!(" {path}  {note}{addr}  {nrows}×{ncols}{cap} ");
    }
    format!(
        " {path}  r{}/{} c{}/{}  {}×{}{cap} ",
        cur_row + 1,
        nrows.max(1),
        cur_col + 1,
        ncols,
        nrows,
        ncols,
    )
}

/// The title of the full-cell popup: ` Cell · B3  r3/10 c2/5 `. `row` and `col` are 1-based.
pub fn cell_popup_title(
    label: &str,
    header: &str,
    (row, col): (usize, usize),
    (nrows, ncols): (usize, usize),
) -> String {
    format!(" {label} · {header}  r{row}/{} c{col}/{ncols} ", nrows.max(1))
}

/// The popup's scroll after moving it by `delta` wrapped rows (the upper bound is clamped when
/// drawn).
pub fn popup_scroll_by(scroll: u16, delta: i32) -> u16 {
    i32::from(scroll).saturating_add(delta).clamp(0, i32::from(u16::MAX)) as u16
}

/// The scroll of the popup's top (`bottom = false`) or bottom (clamped when drawn).
pub fn popup_scroll_to(bottom: bool) -> u16 {
    if bottom {
        u16::MAX
    } else {
        0
    }
}

/// How many wrapped rows a page of the popup moves: one less than it shows, at least 1.
pub fn popup_page_step(viewport: u16) -> i32 {
    i32::from(viewport.saturating_sub(1).max(1))
}

/// How one drawing looks. The texts and the choices belong to the app.
pub struct TableOptions<'a> {
    /// The frame's title (the app builds it: file name, cursor position, ...).
    pub title: String,
    /// Shown in the middle of the frame when the table has no columns (without parentheses).
    pub empty_label: String,
    /// Color the columns in turn.
    pub rainbow: bool,
    /// Draw a 1-based row-number gutter (a spreadsheet's), which stays put while columns scroll.
    pub gutter: bool,
    /// Whether a cell is a search hit (drawn underlined and bold).
    pub is_hit: &'a dyn Fn(usize, usize) -> bool,
}

/// Draws the table into `area` of `buf`, settling `state.top`, `state.left` and
/// `state.viewport_rows` so the cursor cell is visible.
///
/// Returns whether the grid was laid out. It is `false` when the table has no columns or the
/// frame's inner area has no width or height: only the placeholder is drawn and `state` is left
/// untouched. The caller must write the settled `top`/`left`/`viewport_rows` back to its own
/// state only when this returns `true`, so a draw into a collapsed area cannot clobber the last
/// real page size.
#[must_use]
pub fn render_table(
    buf: &mut Buffer,
    area: Rect,
    t: &dyn TableSource,
    state: &mut TableViewState,
    opts: &TableOptions<'_>,
) -> bool {
    let (cur_row, cur_col) = (state.cursor_row, state.cursor_col);
    let (mut top, mut left) = (state.top, state.left);
    let nrows = t.nrows();
    let ncols = t.ncols();
    // A spreadsheet sheet gets a row-number gutter on the left (1-based, like Excel); it is not
    // data and stays put while the columns scroll sideways.
    let gutter_w = if opts.gutter {
        nrows.max(1).to_string().len() + COL_GAP
    } else {
        0
    };
    let rainbow = opts.rainbow;
    let block = Block::bordered().title(opts.title.clone());
    let inner = block.inner(area);

    // When there are no columns (an empty file), show only a placeholder.
    if ncols == 0 || inner.width == 0 || inner.height == 0 {
        let para = Paragraph::new(Line::from(Span::styled(
            format!(" ({}) ", opts.empty_label),
            Style::default().fg(Color::DarkGray),
        )))
        .block(block);
        para.render(area, buf);
        return false;
    }

    // Layout: 0 = header / 1 = separator line / 2.. = data rows. 0 data rows if the height doesn't allow it.
    let visible_rows = (inner.height as usize).saturating_sub(2);

    // --- Vertical scroll: adjust top so the cursor row is visible ---
    if visible_rows > 0 {
        if cur_row < top {
            top = cur_row;
        } else if cur_row >= top + visible_rows {
            top = cur_row + 1 - visible_rows;
        }
        // Suppress wasted scroll toward the end (don't leave empty space at the bottom edge). But keep the cursor within the visible range.
        let max_top = nrows.saturating_sub(visible_rows);
        top = top.min(max_top).min(cur_row);
    } else {
        top = cur_row.min(nrows.saturating_sub(1));
    }
    let row_end = (top + visible_rows).min(nrows);

    // --- Horizontal scroll: adjust left so the cursor column is visible, and settle which columns fit and at what width ---
    if cur_col < left {
        left = cur_col;
    }
    let data_rows = row_end.saturating_sub(top).max(1);
    let avail = inner.width.saturating_sub(gutter_w as u16);
    // The smallest `left` (not before the current one) from which the cursor column still fits:
    // count the columns that fit leftwards from the cursor. (Stepping `left` forward one column
    // at a time and re-fitting every time cost 400 ms at the last of 16,384 columns.)
    left = leftmost_visible(t, cur_col, left, data_rows, top, avail);
    let cols = fit_columns(t, left, data_rows, top, avail);

    // --- Assembling rows (all owned String = 'static) ---
    let mut lines: Vec<Line<'static>> = Vec::with_capacity(visible_rows + 2);

    // Header row (bold, column color).
    let mut header = compose_line(&cols, |col, w| {
        let text = fit_to_width(&t.header(col), w);
        Span::styled(
            text,
            Style::default()
                .fg(column_color(rainbow, col))
                .add_modifier(Modifier::BOLD),
        )
    });
    if gutter_w > 0 {
        header.spans.insert(0, Span::raw(" ".repeat(gutter_w)));
    }
    lines.push(header);

    // Separator line (dim).
    let sep_w: usize = gutter_w
        + cols.iter().map(|(_, w)| *w).sum::<usize>()
        + COL_GAP * cols.len().saturating_sub(1);
    lines.push(Line::from(Span::styled(
        "─".repeat(sep_w.min(inner.width as usize)),
        Style::default().fg(Color::DarkGray),
    )));

    // Data rows. The cursor cell is reversed. A search-match cell is shown underlined + bold (using a
    // modifier rather than a background so as not to blot out the rainbow column color). The current match is at the cursor, so reversal makes it clear.
    for r in top..row_end {
        let mut line = compose_line(&cols, |col, w| {
            let text = fit_to_width(&t.cell(r, col), w);
            let mut style = Style::default().fg(column_color(rainbow, col));
            if (opts.is_hit)(r, col) {
                style = style
                    .add_modifier(Modifier::UNDERLINED)
                    .add_modifier(Modifier::BOLD);
            }
            if r == cur_row && col == cur_col {
                style = style.add_modifier(Modifier::REVERSED);
            }
            Span::styled(text, style)
        });
        if gutter_w > 0 {
            // Muted so it reads as chrome, not data; the cursor's row number stands out a little.
            let num_style = if r == cur_row {
                Style::default().add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            line.spans.insert(
                0,
                Span::styled(
                    format!(
                        "{:>w$}{}",
                        r + 1,
                        " ".repeat(COL_GAP),
                        w = gutter_w - COL_GAP
                    ),
                    num_style,
                ),
            );
        }
        lines.push(line);
    }

    state.top = top;
    state.left = left;
    state.viewport_rows = visible_rows as u16;

    let para = Paragraph::new(lines).block(block);
    para.render(area, buf);
    true
}

/// Fraction of the area the full-cell popup occupies (generous: the point is reading long content
/// comfortably, not a small dialog).
const CELL_POPUP_FRACTION: u32 = 4; // out of 5 (80%)
const CELL_POPUP_DENOM: u32 = 5;

/// What the full-cell popup shows.
pub struct CellPopup {
    /// Top border title.
    pub title: String,
    /// Centered dim line on the bottom border (the keys that act in the popup).
    pub footer: String,
    /// The cell's untruncated text; wrapped, with real newlines kept.
    pub text: String,
    /// Wrapped rows scrolled off the top (clamped when drawn).
    pub scroll: u16,
}

/// Draws a centered popup with one cell's full text over `area` and returns the clamped scroll and
/// the popup's visible row count (the PageUp/Down step). The rect can be larger than a small
/// `area`: the drawing clips it instead of panicking.
pub fn render_cell_popup(buf: &mut Buffer, area: Rect, popup: CellPopup) -> (u16, u16) {
    // Popup size: 80% of the area, with margin reserved.
    //
    // `.min(upper).max(lower)` - not `.clamp(lower, upper)` - because `upper` is a runtime value
    // (terminal size) that can fall below the fixed `lower` floor on a small terminal, and
    // `Ord::clamp` asserts `min <= max` (a real `assert!`, so it panics in release builds too).
    let w = ((area.width as u32 * CELL_POPUP_FRACTION / CELL_POPUP_DENOM) as u16)
        .min(area.width.saturating_sub(2))
        .max(20);
    let h = ((area.height as u32 * CELL_POPUP_FRACTION / CELL_POPUP_DENOM) as u16)
        .min(area.height.saturating_sub(2))
        .max(6);
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    let rect = Rect {
        x,
        y,
        width: w,
        height: h,
    };

    let block = Block::bordered()
        .title(popup.title)
        .title_bottom(Line::from(popup.footer).centered().dim())
        .border_style(Style::new().fg(Color::Cyan));
    let inner = block.inner(rect);

    // ratatui doesn't expand tab characters to the terminal's tab stops (which would mismatch
    // unicode-width's width calculation) and draws them as a single column as-is, so they are
    // flattened to spaces before wrapping. Real newlines are already line-split by Text::raw.
    let body = if popup.text.contains('\t') {
        popup.text.replace('\t', " ")
    } else {
        popup.text
    };

    let mut para = Paragraph::new(body).wrap(Wrap { trim: false });
    // Have ratatui compute the total display row count (wrapping included), and clamp so it never
    // scrolls past the end.
    let total_rows = para.line_count(inner.width);
    let max_scroll = total_rows.saturating_sub(inner.height as usize) as u16;
    let scroll = popup.scroll.min(max_scroll);

    para = para
        .block(block)
        .scroll((scroll, 0))
        .alignment(Alignment::Left);

    Clear.render(rect, buf); // clear the background before drawing
    para.render(rect, buf);
    (scroll, inner.height)
}

/// Column color: rainbow when enabled, else the terminal default foreground.
fn column_color(rainbow: bool, col: usize) -> Color {
    if rainbow {
        RAINBOW[col % RAINBOW.len()]
    } else {
        Color::Reset
    }
}

/// The smallest `left >= min_left` such that the columns `left..=cur_col` fit in `width` (or
/// `cur_col` itself when even that column alone does not). Same answer as advancing `left` one
/// column at a time until `fit_columns` shows `cur_col`, in time proportional to the columns that
/// fit rather than to the distance scrolled.
fn leftmost_visible(
    t: &dyn TableSource,
    cur_col: usize,
    min_left: usize,
    visible_rows: usize,
    top: usize,
    width: u16,
) -> usize {
    let avail = width as usize;
    let mut used = col_width(t, cur_col, visible_rows, top);
    let mut left = cur_col;
    while left > min_left {
        let w = col_width(t, left - 1, visible_rows, top);
        if used + COL_GAP + w > avail {
            break;
        }
        used += COL_GAP + w;
        left -= 1;
    }
    left
}

/// A column's display width: the widest of its header and the visible cells, clamped.
fn col_width(t: &dyn TableSource, col: usize, visible_rows: usize, top: usize) -> usize {
    let mut w = head(&t.header(col)).width();
    for r in top..(top + visible_rows).min(t.nrows()) {
        w = w.max(flat_width(&t.cell(r, col)));
    }
    w.clamp(MIN_COL_W, MAX_COL_W)
}

/// Which columns (starting at `left`) fit in `width`, and each column's display width.
/// Column width = max(header, visible cells), clamped to [MIN_COL_W, MAX_COL_W]. Always yields at
/// least one column (even if it overflows) so something is always drawn.
fn fit_columns(
    t: &dyn TableSource,
    left: usize,
    visible_rows: usize,
    top: usize,
    width: u16,
) -> Vec<(usize, usize)> {
    let mut out: Vec<(usize, usize)> = Vec::new();
    let mut used = 0usize;
    let avail = width as usize;
    for col in left..t.ncols() {
        let w = col_width(t, col, visible_rows, top);
        let gap = if out.is_empty() { 0 } else { COL_GAP };
        if !out.is_empty() && used + gap + w > avail {
            break;
        }
        out.push((col, w));
        used += gap + w;
    }
    if out.is_empty() {
        // Show at least the first column even when the width is extremely narrow.
        out.push((left, MIN_COL_W.min(avail.max(1))));
    }
    out
}

/// Build a `Line` by rendering each fitted column via `cell` and joining with the column gap.
fn compose_line(
    cols: &[(usize, usize)],
    mut cell: impl FnMut(usize, usize) -> Span<'static>,
) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::with_capacity(cols.len() * 2);
    for (i, (col, w)) in cols.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw(" ".repeat(COL_GAP)));
        }
        spans.push(cell(*col, *w));
    }
    Line::from(spans)
}

/// How much of a cell the grid ever looks at, in characters. A column is at most [`MAX_COL_W`]
/// (40) wide, so only the start of a cell can be on screen; a cell can be megabytes (one is up to
/// 128 MiB), and every visible cell is measured and cut at every frame, so each of those walks
/// the head and nothing else. 256 is six times the widest column, which leaves room for the
/// zero-width characters (combining marks, joiners) that take no column.
const HEAD_CHARS: usize = 256;

/// The first [`HEAD_CHARS`] characters of `s`: all the grid needs of a cell. Costs the head, not
/// the length of `s`.
fn head(s: &str) -> &str {
    match s.char_indices().nth(HEAD_CHARS) {
        Some((i, _)) => &s[..i],
        None => s,
    }
}

/// Display width of the head of a cell after flattening embedded newlines/tabs to spaces (the
/// width is only ever compared with [`MAX_COL_W`]).
fn flat_width(s: &str) -> usize {
    let s = head(s);
    if s.contains(['\n', '\r', '\t']) {
        flatten(s).width()
    } else {
        s.width()
    }
}

/// Replace embedded newlines/tabs with spaces so a multi-line cell stays on one grid row.
fn flatten(s: &str) -> String {
    s.replace(['\n', '\r', '\t'], " ")
}

/// Truncate `s` to exactly `w` display columns (adding `…` when cut) and right-pad with spaces.
/// CJK-aware (full-width glyphs count as 2). Embedded newlines/tabs are flattened first.
fn fit_to_width(s: &str, w: usize) -> String {
    let s = flatten(head(s));
    let total = s.width();
    let mut out = String::new();
    let mut used = 0usize;
    if total <= w {
        out.push_str(&s);
        used = total;
    } else {
        let budget = w.saturating_sub(1); // reserve 1 column for '…'
        for ch in s.chars() {
            let cw = ch.width().unwrap_or(0);
            if used + cw > budget {
                break;
            }
            out.push(ch);
            used += cw;
        }
        out.push('…');
        used += 1;
    }
    while used < w {
        out.push(' ');
        used += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    /// A tiny in-test table (ragged rows read as empty cells).
    struct Tbl {
        headers: Vec<String>,
        rows: Vec<Vec<String>>,
        ncols: usize,
    }

    impl Tbl {
        fn new(headers: &[&str], rows: &[&[&str]]) -> Self {
            Tbl {
                headers: headers.iter().map(|s| s.to_string()).collect(),
                rows: rows
                    .iter()
                    .map(|r| r.iter().map(|s| s.to_string()).collect())
                    .collect(),
                ncols: headers.len(),
            }
        }
    }

    impl TableSource for Tbl {
        fn nrows(&self) -> usize {
            self.rows.len()
        }
        fn ncols(&self) -> usize {
            self.ncols
        }
        fn header(&self, col: usize) -> Cow<'_, str> {
            Cow::Borrowed(self.headers.get(col).map_or("", |s| s.as_str()))
        }
        fn cell(&self, row: usize, col: usize) -> Cow<'_, str> {
            Cow::Borrowed(
                self.rows
                    .get(row)
                    .and_then(|r| r.get(col))
                    .map_or("", |s| s.as_str()),
            )
        }
    }

    fn no_hit(_: usize, _: usize) -> bool {
        false
    }

    fn opts<'a>(hit: &'a dyn Fn(usize, usize) -> bool) -> TableOptions<'a> {
        TableOptions {
            title: " t ".into(),
            empty_label: "empty".into(),
            rainbow: false,
            gutter: false,
            is_hit: hit,
        }
    }

    /// Draws into a `w`x`h` terminal and returns the buffer.
    fn draw(
        t: &dyn TableSource,
        st: &mut TableViewState,
        o: &TableOptions<'_>,
        w: u16,
        h: u16,
    ) -> Buffer {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        let done = term
            .draw(|f| {
                let area = f.area();
                let _ = render_table(f.buffer_mut(), area, t, st, o);
            })
            .unwrap();
        done.buffer.clone()
    }

    /// The text of row `y` of the buffer as the screen reads it: the cell after a full-width glyph
    /// is skipped, trailing blanks trimmed.
    fn row_text(b: &Buffer, y: u16) -> String {
        let mut out = String::new();
        let mut x = 0;
        while x < b.area.width {
            let sym = b[(x, y)].symbol();
            out.push_str(sym);
            x += sym.width().max(1) as u16;
        }
        out.trim_end().to_string()
    }

    fn screen(b: &Buffer) -> Vec<String> {
        (0..b.area.height).map(|y| row_text(b, y)).collect()
    }

    fn style_at(b: &Buffer, x: u16, y: u16) -> Style {
        b[(x, y)].style()
    }

    #[test]
    fn reports_whether_the_grid_was_laid_out() {
        let t = Tbl::new(&["a"], &[&["1"], &["2"], &["3"], &["4"], &["5"], &["6"]]);
        let sentinel = TableViewState {
            cursor_row: 4,
            cursor_col: 0,
            top: 3,
            left: 0,
            viewport_rows: 77,
        };
        let o = opts(&no_hit);
        // Normal draw: laid out, state settled.
        let mut st = sentinel;
        let mut term = Terminal::new(TestBackend::new(20, 6)).unwrap();
        let mut ok = false;
        term.draw(|f| {
            let a = f.area();
            ok = render_table(f.buffer_mut(), a, &t, &mut st, &o);
        })
        .unwrap();
        assert!(ok);
        assert_eq!(st.viewport_rows, 2);
        // Zero inner width, zero inner height, and no columns: not laid out, state untouched.
        let empty = Tbl::new(&[], &[]);
        let cases: [(&Tbl, u16, u16); 4] = [(&t, 2, 8), (&t, 20, 2), (&t, 1, 1), (&empty, 20, 6)];
        for (tbl, w, h) in cases {
            let mut st = sentinel;
            let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
            let mut ok = true;
            term.draw(|f| {
                let a = f.area();
                ok = render_table(f.buffer_mut(), a, tbl, &mut st, &o);
            })
            .unwrap();
            assert!(!ok, "{w}x{h}");
            assert_eq!(st, sentinel, "{w}x{h}");
        }
    }

    #[test]
    fn draws_title_header_separator_and_rows() {
        let t = Tbl::new(&["name", "n"], &[&["alpha", "1"], &["b", "22"]]);
        let mut st = TableViewState::default();
        let b = draw(&t, &mut st, &opts(&no_hit), 20, 7);
        assert_eq!(
            screen(&b),
            [
                "┌ t ───────────────┐",
                "│name  n           │",
                "│─────────         │",
                "│alpha 1           │",
                "│b     22          │",
                "│                  │",
                "└──────────────────┘",
            ]
        );
        assert_eq!(st.viewport_rows, 5 - 2);
    }

    #[test]
    fn cjk_cells_count_two_columns_and_truncate_on_a_glyph_boundary() {
        let t = Tbl::new(&["名前", "n"], &[&["あいうえお", "1"], &["b", "22"]]);
        let mut st = TableViewState::default();
        let b = draw(&t, &mut st, &opts(&no_hit), 14, 6);
        assert_eq!(
            screen(&b),
            [
                "┌ t ─────────┐",
                "│名前        │",
                "│──────────  │",
                "│あいうえお  │",
                "│b           │",
                "└────────────┘",
            ]
        );
        // Narrower than the column: the cell is clipped by the frame, never mid-glyph.
        let b = draw(&t, &mut st, &opts(&no_hit), 9, 6);
        assert_eq!(
            screen(&b),
            [
                "┌ t ────┐",
                "│名前   │",
                "│───────│",
                "│あいう │",
                "│b      │",
                "└───────┘",
            ]
        );
        // One column wide per glyph pair: 2 inner columns still draw the first full-width glyph.
        let b = draw(&t, &mut st, &opts(&no_hit), 4, 6);
        assert_eq!(row_text(&b, 1), "│名│");
    }

    #[test]
    fn a_table_without_columns_shows_only_the_label() {
        let t = Tbl::new(&[], &[]);
        let mut st = TableViewState::default();
        let b = draw(&t, &mut st, &opts(&no_hit), 20, 4);
        assert_eq!(
            screen(&b),
            [
                "┌ t ───────────────┐",
                "│ (empty)          │",
                "│                  │",
                "└──────────────────┘",
            ]
        );
        // The scroll state is untouched: nothing was settled.
        assert_eq!(st, TableViewState::default());
    }

    #[test]
    fn a_table_with_columns_but_no_rows_shows_the_header_and_separator() {
        let t = Tbl::new(&["a", "bb"], &[]);
        let mut st = TableViewState::default();
        let b = draw(&t, &mut st, &opts(&no_hit), 12, 5);
        assert_eq!(
            screen(&b),
            [
                "┌ t ───────┐",
                "│a   bb    │",
                "│───────   │",
                "│          │",
                "└──────────┘",
            ]
        );
    }

    #[test]
    fn tiny_areas_do_not_panic() {
        let t = Tbl::new(&["a", "b"], &[&["1", "2"]]);
        for (w, h) in [(0, 0), (1, 1), (2, 2), (3, 3), (5, 1), (1, 5), (4, 4)] {
            let mut st = TableViewState {
                cursor_row: 3,
                cursor_col: 7,
                top: 9,
                left: 9,
                viewport_rows: 0,
            };
            // A cursor outside the table must not panic either.
            let _ = draw(&t, &mut st, &opts(&no_hit), w, h);
        }
    }

    #[test]
    fn the_cursor_cell_is_reversed_at_every_corner() {
        let t = Tbl::new(&["ab", "cd", "ef"], &[&["1", "2", "3"], &["4", "5", "6"]]);
        // Inner origin is (1, 1); header row 1, separator 2, data from y = 3; columns are 3 wide
        // with a 1-column gap, so column c starts at x = 1 + 4c.
        for (row, col) in [(0, 0), (0, 2), (1, 0), (1, 2)] {
            let mut st = TableViewState {
                cursor_row: row,
                cursor_col: col,
                ..Default::default()
            };
            let b = draw(&t, &mut st, &opts(&no_hit), 16, 6);
            for r in 0..2usize {
                for c in 0..3usize {
                    let x = 1 + 4 * c as u16;
                    let y = 3 + r as u16;
                    let rev = style_at(&b, x, y).add_modifier.contains(Modifier::REVERSED);
                    assert_eq!(
                        rev,
                        (r, c) == (row, col),
                        "cursor ({row},{col}) cell ({r},{c})"
                    );
                }
            }
        }
    }

    #[test]
    fn the_cursor_below_the_screen_scrolls_the_rows_to_keep_it_visible() {
        let rows: Vec<Vec<String>> = (0..30).map(|r| vec![format!("r{r}")]).collect();
        let t = Tbl {
            headers: vec!["h".into()],
            rows,
            ncols: 1,
        };
        // 8 high: 6 inner, 4 data rows on screen.
        let mut st = TableViewState {
            cursor_row: 29,
            ..Default::default()
        };
        let b = draw(&t, &mut st, &opts(&no_hit), 10, 8);
        assert_eq!((st.top, st.viewport_rows), (26, 4));
        assert_eq!(row_text(&b, 3), "│r26     │");
        assert_eq!(row_text(&b, 6), "│r29     │");
        assert!(style_at(&b, 1, 6).add_modifier.contains(Modifier::REVERSED));
        // Back to the top.
        st.cursor_row = 0;
        let b = draw(&t, &mut st, &opts(&no_hit), 10, 8);
        assert_eq!(st.top, 0);
        assert_eq!(row_text(&b, 3), "│r0      │");
        // A stale offset past the end is pulled back so the bottom edge has no wasted space.
        st.cursor_row = 29;
        st.top = 29;
        draw(&t, &mut st, &opts(&no_hit), 10, 8);
        assert_eq!(st.top, 26);
    }

    #[test]
    fn the_cursor_column_far_right_scrolls_the_columns() {
        let t = Tbl::new(
            &["aaaaa", "bbbbb", "ccccc", "ddddd"],
            &[&["1", "2", "3", "4"]],
        );
        // 12 wide: 10 inner = one 5-wide column + gap + 4 left over, so one column at a time...
        // two columns need 11.
        let mut st = TableViewState {
            cursor_col: 3,
            ..Default::default()
        };
        let b = draw(&t, &mut st, &opts(&no_hit), 12, 5);
        assert_eq!(st.left, 3);
        assert_eq!(row_text(&b, 1), "│ddddd     │");
        // 13 wide: 11 inner fits two columns, so the cursor column sits at the right edge.
        let mut st = TableViewState {
            cursor_col: 3,
            ..Default::default()
        };
        let b = draw(&t, &mut st, &opts(&no_hit), 13, 5);
        assert_eq!(st.left, 2);
        assert_eq!(row_text(&b, 1), "│ccccc ddddd│");
        assert!(style_at(&b, 7, 3).add_modifier.contains(Modifier::REVERSED));
        // Moving the cursor back left scrolls back only as far as needed.
        st.cursor_col = 1;
        draw(&t, &mut st, &opts(&no_hit), 13, 5);
        assert_eq!(st.left, 1);
    }

    #[test]
    fn search_hits_are_underlined_and_bold_and_the_cursor_adds_reverse() {
        let t = Tbl::new(&["a", "b"], &[&["x", "y"], &["z", "w"]]);
        let hit = |r: usize, c: usize| (r, c) == (1, 1) || (r, c) == (0, 0);
        let mut st = TableViewState::default();
        let b = draw(&t, &mut st, &opts(&hit), 12, 6);
        // Column 1 starts at x = 1 + 3 + 1.
        let m = style_at(&b, 5, 4).add_modifier;
        assert!(m.contains(Modifier::UNDERLINED | Modifier::BOLD));
        assert!(!m.contains(Modifier::REVERSED));
        let m = style_at(&b, 1, 3).add_modifier;
        assert!(m.contains(Modifier::UNDERLINED | Modifier::BOLD | Modifier::REVERSED));
        // Not a hit: plain.
        assert_eq!(style_at(&b, 5, 3).add_modifier, Modifier::empty());
    }

    #[test]
    fn the_gutter_numbers_rows_and_stays_while_columns_scroll() {
        let rows: Vec<Vec<String>> = (0..12).map(|r| vec![format!("v{r}"), "w".into()]).collect();
        let t = Tbl {
            headers: vec!["A".into(), "B".into()],
            rows,
            ncols: 2,
        };
        let mut o = opts(&no_hit);
        o.gutter = true;
        let mut st = TableViewState::default();
        let b = draw(&t, &mut st, &o, 14, 6);
        // 12 rows -> a 2-digit gutter plus the gap.
        assert_eq!(row_text(&b, 1), "│   A   B    │");
        assert_eq!(row_text(&b, 3), "│ 1 v0  w    │");
        assert_eq!(row_text(&b, 4), "│ 2 v1  w    │");
        // The cursor row's number is bold, the others muted.
        assert!(style_at(&b, 2, 3).add_modifier.contains(Modifier::BOLD));
        assert_eq!(style_at(&b, 2, 4).fg, Some(Color::DarkGray));
        // Scrolling right keeps the gutter.
        st.cursor_col = 1;
        st.cursor_row = 11;
        let b = draw(&t, &mut st, &o, 9, 6);
        assert_eq!(st.left, 1);
        assert_eq!(row_text(&b, 4), "│12 w   │");
    }

    #[test]
    fn rainbow_colors_columns_in_turn_and_off_uses_the_default_color() {
        let t = Tbl::new(&["a", "b"], &[&["x", "y"]]);
        let mut o = opts(&no_hit);
        o.rainbow = true;
        let mut st = TableViewState::default();
        let b = draw(&t, &mut st, &o, 12, 5);
        assert_eq!(style_at(&b, 1, 1).fg, Some(RAINBOW[0]));
        assert_eq!(style_at(&b, 5, 1).fg, Some(RAINBOW[1]));
        assert!(style_at(&b, 1, 1).add_modifier.contains(Modifier::BOLD));
        assert_eq!(style_at(&b, 5, 3).fg, Some(RAINBOW[1]));
        let b = draw(&t, &mut st, &opts(&no_hit), 12, 5);
        assert_eq!(style_at(&b, 5, 1).fg, Some(Color::Reset));
        // The palette wraps after its last color.
        assert_eq!(column_color(true, RAINBOW.len()), RAINBOW[0]);
        assert_eq!(column_color(true, RAINBOW.len() + 2), RAINBOW[2]);
    }

    #[test]
    fn move_cursor_clamps_to_the_table() {
        let mut st = TableViewState::default();
        st.move_cursor(-1, -1, 3, 2);
        assert_eq!((st.cursor_row, st.cursor_col), (0, 0));
        st.move_cursor(1, 1, 3, 2);
        assert_eq!((st.cursor_row, st.cursor_col), (1, 1));
        st.move_cursor(100, 100, 3, 2);
        assert_eq!((st.cursor_row, st.cursor_col), (2, 1));
        st.move_cursor(i32::MAX, i32::MAX, 3, 2);
        assert_eq!((st.cursor_row, st.cursor_col), (2, 1));
        st.move_cursor(i32::MIN, i32::MIN, 3, 2);
        assert_eq!((st.cursor_row, st.cursor_col), (0, 0));
        // An empty table (no rows, or no columns) does not move the cursor.
        st.cursor_row = 5;
        st.cursor_col = 6;
        st.move_cursor(1, 1, 0, 4);
        st.move_cursor(1, 1, 4, 0);
        assert_eq!((st.cursor_row, st.cursor_col), (5, 6));
        // One cell: always that cell.
        st.move_cursor(3, -3, 1, 1);
        assert_eq!((st.cursor_row, st.cursor_col), (0, 0));
    }

    // ---- step ------------------------------------------------------------------------------

    fn at(row: usize, col: usize, viewport_rows: u16) -> TableViewState {
        TableViewState {
            cursor_row: row,
            cursor_col: col,
            viewport_rows,
            ..Default::default()
        }
    }

    /// Steps `from` once in a 20 x 5 table and returns where the cursor lands.
    fn stepped(from: (usize, usize), vp: u16, step: TableStep) -> (usize, usize) {
        let mut st = at(from.0, from.1, vp);
        st.step(step, 20, 5);
        (st.cursor_row, st.cursor_col)
    }

    #[test]
    fn step_moves_one_cell_and_clamps_at_every_edge() {
        assert_eq!(stepped((5, 2), 8, TableStep::Up), (4, 2));
        assert_eq!(stepped((5, 2), 8, TableStep::Down), (6, 2));
        assert_eq!(stepped((5, 2), 8, TableStep::Left), (5, 1));
        assert_eq!(stepped((5, 2), 8, TableStep::Right), (5, 3));
        assert_eq!(stepped((0, 0), 8, TableStep::Up), (0, 0));
        assert_eq!(stepped((0, 0), 8, TableStep::Left), (0, 0));
        assert_eq!(stepped((19, 4), 8, TableStep::Down), (19, 4));
        assert_eq!(stepped((19, 4), 8, TableStep::Right), (19, 4));
    }

    #[test]
    fn step_jumps_to_the_ends() {
        assert_eq!(stepped((5, 2), 8, TableStep::FirstRow), (0, 2));
        assert_eq!(stepped((5, 2), 8, TableStep::LastRow), (19, 2));
        assert_eq!(stepped((5, 2), 8, TableStep::FirstCol), (5, 0));
        assert_eq!(stepped((5, 2), 8, TableStep::LastCol), (5, 4));
        // Already there: stays.
        assert_eq!(stepped((0, 0), 8, TableStep::FirstRow), (0, 0));
        assert_eq!(stepped((19, 4), 8, TableStep::LastCol), (19, 4));
    }

    #[test]
    fn step_pages_by_the_rows_the_last_drawing_showed() {
        assert_eq!(stepped((0, 1), 7, TableStep::PageDown), (7, 1));
        assert_eq!(stepped((10, 1), 7, TableStep::PageUp), (3, 1));
        assert_eq!(stepped((0, 1), 7, TableStep::HalfDown), (3, 1));
        assert_eq!(stepped((10, 1), 7, TableStep::HalfUp), (7, 1));
        // Clamped to the table.
        assert_eq!(stepped((15, 1), 7, TableStep::PageDown), (19, 1));
        assert_eq!(stepped((3, 1), 7, TableStep::PageUp), (0, 1));
        assert_eq!(stepped((18, 1), 7, TableStep::HalfDown), (19, 1));
        assert_eq!(stepped((1, 1), 7, TableStep::HalfUp), (0, 1));
    }

    #[test]
    fn step_pages_move_at_least_one_row_before_anything_was_drawn() {
        // viewport_rows is 0 until the first drawing: a page is 1 row, half a page is 1 row.
        assert_eq!(stepped((5, 0), 0, TableStep::PageDown), (6, 0));
        assert_eq!(stepped((5, 0), 0, TableStep::PageUp), (4, 0));
        assert_eq!(stepped((5, 0), 0, TableStep::HalfDown), (6, 0));
        assert_eq!(stepped((5, 0), 1, TableStep::HalfUp), (4, 0));
        assert_eq!(stepped((5, 0), 3, TableStep::HalfDown), (6, 0), "3 / 2 = 1");
        assert_eq!(stepped((5, 0), 2, TableStep::HalfDown), (6, 0));
        assert_eq!(stepped((5, 0), 4, TableStep::HalfDown), (7, 0));
    }

    #[test]
    fn step_in_an_empty_table_leaves_the_cursor_but_the_jumps_land_on_zero() {
        for step in [
            TableStep::Up,
            TableStep::Down,
            TableStep::Left,
            TableStep::Right,
            TableStep::PageDown,
            TableStep::PageUp,
            TableStep::HalfDown,
            TableStep::HalfUp,
        ] {
            for (nr, nc) in [(0, 0), (0, 3), (3, 0)] {
                let mut st = at(2, 2, 5);
                st.step(step, nr, nc);
                assert_eq!((st.cursor_row, st.cursor_col), (2, 2), "{step:?} {nr}x{nc}");
            }
        }
        let mut st = at(2, 2, 5);
        st.step(TableStep::FirstRow, 0, 0);
        st.step(TableStep::FirstCol, 0, 0);
        assert_eq!((st.cursor_row, st.cursor_col), (0, 0));
        let mut st = at(2, 2, 5);
        st.step(TableStep::LastRow, 0, 0);
        st.step(TableStep::LastCol, 0, 0);
        assert_eq!((st.cursor_row, st.cursor_col), (0, 0), "saturates at 0");
    }

    #[test]
    fn step_in_a_one_cell_table_stays_on_it() {
        for step in [
            TableStep::Up,
            TableStep::Down,
            TableStep::Left,
            TableStep::Right,
            TableStep::FirstRow,
            TableStep::LastRow,
            TableStep::FirstCol,
            TableStep::LastCol,
            TableStep::PageDown,
            TableStep::PageUp,
            TableStep::HalfDown,
            TableStep::HalfUp,
        ] {
            let mut st = at(0, 0, 9);
            st.step(step, 1, 1);
            assert_eq!((st.cursor_row, st.cursor_col), (0, 0), "{step:?}");
        }
    }

    #[test]
    fn step_leaves_the_viewport_and_the_scroll_alone() {
        let mut st = at(3, 1, 6);
        st.step(TableStep::PageDown, 50, 4);
        assert_eq!(st.viewport_rows, 6);
        assert_eq!(st.cursor_row, 9);
    }

    // ---- titles and addresses --------------------------------------------------------------

    #[test]
    fn column_letters_carry_like_a_spreadsheet() {
        for (col, want) in [
            (0, "A"),
            (1, "B"),
            (25, "Z"),
            (26, "AA"),
            (27, "AB"),
            (51, "AZ"),
            (52, "BA"),
            (701, "ZZ"),
            (702, "AAA"),
            (16383, "XFD"),
        ] {
            assert_eq!(column_letters(col), want, "{col}");
        }
    }

    #[test]
    fn cell_address_is_a_column_then_a_one_based_row() {
        assert_eq!(cell_address(0, 0), "A1");
        assert_eq!(cell_address(2, 1), "B3");
        assert_eq!(cell_address(99, 26), "AA100");
    }

    #[test]
    fn the_title_of_a_plain_table_counts_rows_and_columns() {
        assert_eq!(
            table_title("d.csv", (100, 5), (2, 1), None, None),
            " d.csv  r3/100 c2/5  100×5 "
        );
        assert_eq!(
            table_title("d.csv", (100, 5), (0, 0), None, Some("capped")),
            " d.csv  r1/100 c1/5  100×5  (capped) "
        );
    }

    #[test]
    fn the_title_of_an_empty_plain_table_never_shows_row_zero_of_zero() {
        assert_eq!(
            table_title("d.csv", (0, 2), (0, 0), None, None),
            " d.csv  r1/1 c1/2  0×2 "
        );
        assert_eq!(
            table_title("d.csv", (0, 0), (0, 0), None, None),
            " d.csv  r1/1 c1/0  0×0 "
        );
    }

    #[test]
    fn the_title_of_a_sheet_names_the_cursor_cell_by_its_address() {
        assert_eq!(
            table_title("b.xlsx", (5, 6), (2, 1), Some("Sales (1/3) +1 hidden"), None),
            " b.xlsx  Sales (1/3) +1 hidden  B3  5×6 "
        );
        assert_eq!(
            table_title("b.xlsx", (5, 6), (0, 0), Some("S"), Some("capped")),
            " b.xlsx  S  A1  5×6  (capped) "
        );
        // A sheet with no cells has no address to name.
        assert_eq!(
            table_title("b.xlsx", (0, 6), (0, 0), Some("S"), None),
            " b.xlsx  S  0×6 "
        );
        assert_eq!(
            table_title("b.xlsx", (4, 0), (0, 0), Some("S"), None),
            " b.xlsx  S  4×0 "
        );
    }

    #[test]
    fn the_popup_title_names_the_cell_and_its_place() {
        assert_eq!(
            cell_popup_title("Cell", "name", (3, 2), (10, 5)),
            " Cell · name  r3/10 c2/5 "
        );
        assert_eq!(
            cell_popup_title("Cell", "B3", (1, 1), (0, 4)),
            " Cell · B3  r1/1 c1/4 ",
            "an empty table counts one row"
        );
    }

    // ---- popup scroll ----------------------------------------------------------------------

    #[test]
    fn popup_scroll_by_clamps_at_the_top_and_saturates_at_the_bottom() {
        assert_eq!(popup_scroll_by(5, 3), 8);
        assert_eq!(popup_scroll_by(5, -3), 2);
        assert_eq!(popup_scroll_by(5, -100), 0);
        assert_eq!(popup_scroll_by(0, -1), 0);
        // After a jump to the bottom (`u16::MAX`) a further step must not wrap back to the top.
        assert_eq!(popup_scroll_by(u16::MAX, 1), u16::MAX);
        assert_eq!(popup_scroll_by(u16::MAX, 100_000), u16::MAX);
        assert_eq!(popup_scroll_by(u16::MAX - 1, 5), u16::MAX);
        assert_eq!(popup_scroll_by(7, i32::MIN), 0);
        assert_eq!(popup_scroll_by(7, i32::MAX), u16::MAX);
    }

    #[test]
    fn popup_scroll_to_is_the_top_or_the_clamped_bottom() {
        assert_eq!(popup_scroll_to(false), 0);
        assert_eq!(popup_scroll_to(true), u16::MAX);
    }

    #[test]
    fn popup_page_step_is_one_less_than_the_viewport_and_at_least_one() {
        assert_eq!(popup_page_step(0), 1);
        assert_eq!(popup_page_step(1), 1);
        assert_eq!(popup_page_step(2), 1);
        assert_eq!(popup_page_step(3), 2);
        assert_eq!(popup_page_step(10), 9);
        assert_eq!(popup_page_step(u16::MAX), i32::from(u16::MAX) - 1);
    }

    // ---- cell popup ----------------------------------------------------------------------

    fn popup_screen(text: &str, scroll: u16, w: u16, h: u16) -> (Vec<String>, u16, u16) {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        let mut out = (0, 0);
        let done = term
            .draw(|f| {
                let area = f.area();
                out = render_cell_popup(
                    f.buffer_mut(),
                    area,
                    CellPopup {
                        title: " cell ".into(),
                        footer: "q close".into(),
                        text: text.into(),
                        scroll,
                    },
                );
            })
            .unwrap();
        (screen(done.buffer), out.0, out.1)
    }

    #[test]
    fn the_popup_is_centered_wraps_and_shows_its_footer() {
        let (lines, scroll, viewport) = popup_screen("hello\tworld\nあいう", 0, 30, 10);
        // 80% of 30x10 = 24x8, centered at (3, 1).
        assert_eq!(
            lines,
            [
                "",
                "   ┌ cell ────────────────┐",
                "   │hello world           │",
                "   │あいう                │",
                "   │                      │",
                "   │                      │",
                "   │                      │",
                "   │                      │",
                "   └───────q close────────┘",
                "",
            ]
        );
        assert_eq!((scroll, viewport), (0, 6));
    }

    #[test]
    fn the_popup_clamps_its_scroll_to_the_last_wrapped_row() {
        let text = (0..40)
            .map(|i| format!("line{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let (lines, scroll, viewport) = popup_screen(&text, u16::MAX, 30, 10);
        assert_eq!(viewport, 6);
        assert_eq!(scroll, 40 - 6);
        assert_eq!(lines[2], "   │line34                │");
        assert_eq!(lines[7], "   │line39                │");
        // Scrolling back by one row from there.
        let (lines, scroll, _) = popup_screen(&text, 33, 30, 10);
        assert_eq!(scroll, 33);
        assert_eq!(lines[2], "   │line33                │");
        // Short text: no scroll at all.
        let (_, scroll, _) = popup_screen("a", 5, 30, 10);
        assert_eq!(scroll, 0);
    }

    #[test]
    fn the_popup_wraps_long_lines() {
        let (lines, scroll, _) = popup_screen(&"x".repeat(50), 0, 30, 10);
        assert_eq!(lines[2], format!("   │{}│", "x".repeat(22)));
        assert_eq!(lines[3], format!("   │{}│", "x".repeat(22)));
        assert_eq!(lines[4], format!("   │{:<22}│", "x".repeat(6)));
        assert_eq!(scroll, 0);
    }

    #[test]
    fn the_popup_survives_every_small_size() {
        for w in [0u16, 1, 2, 19, 20, 21, 22, 23, 80] {
            for h in [0u16, 1, 2, 5, 6, 7, 8, 9, 30] {
                let _ = popup_screen("abc\ndef", 0, w, h);
            }
        }
        // A 1x1 terminal still gets a clipped sliver of the border.
        let (lines, _, _) = popup_screen("abc", 0, 1, 1);
        assert_eq!(lines, ["┌"]);
    }

    // ---- column fitting ---------------------------------------------------------------

    #[test]
    fn fit_pads_short_and_truncates_long() {
        assert_eq!(fit_to_width("ab", 5), "ab   ");
        assert_eq!(fit_to_width("abcdef", 4), "abc…");
        assert_eq!(fit_to_width("", 3), "   ");
        assert_eq!(fit_to_width("abcd", 4), "abcd");
        assert_eq!(fit_to_width("abcde", 4), "abc…");
    }

    #[test]
    fn fit_flattens_newlines() {
        assert_eq!(fit_to_width("a\nb", 4), "a b ");
        assert_eq!(fit_to_width("a\r\n\tb", 6), "a   b ");
    }

    #[test]
    fn fit_handles_full_width_cjk() {
        // 2 full-width characters = width 4. Fits exactly into width 4.
        assert_eq!(fit_to_width("あい", 4).width(), 4);
        // Truncated to width 3: 1 full-width char (width 2) + … (width 1) = 3.
        assert_eq!(fit_to_width("あい", 3), "あ…");
        // A glyph that would straddle the budget is dropped, the width is still exact.
        assert_eq!(fit_to_width("あいう", 4), "あ… ");
        assert_eq!(fit_to_width("あいう", 4).width(), 4);
    }

    #[test]
    fn column_width_is_the_widest_visible_cell_clamped() {
        let long = "x".repeat(100);
        let t = Tbl::new(
            &["", "headerr", "h"],
            &[&["", "a", &long], &["", "bbb", "c"]],
        );
        // Empty column: the floor.
        assert_eq!(col_width(&t, 0, 2, 0), MIN_COL_W);
        // The header is the widest.
        assert_eq!(col_width(&t, 1, 2, 0), 7);
        // A cell past the cap: the cap.
        assert_eq!(col_width(&t, 2, 2, 0), MAX_COL_W);
        // Only the rows on screen count: row 0 is off screen when the window starts at 1.
        assert_eq!(col_width(&t, 2, 1, 1), MIN_COL_W);
        // Exactly at the cap and one past it.
        let at = "y".repeat(MAX_COL_W);
        let t = Tbl::new(&["h"], &[&[&at]]);
        assert_eq!(col_width(&t, 0, 1, 0), MAX_COL_W);
        let past = "y".repeat(MAX_COL_W + 1);
        let t = Tbl::new(&["h"], &[&[&past]]);
        assert_eq!(col_width(&t, 0, 1, 0), MAX_COL_W);
    }

    /// The old way: step `left` forward until `fit_columns` shows the cursor column.
    fn leftmost_by_stepping(
        t: &dyn TableSource,
        cur_col: usize,
        mut left: usize,
        rows: usize,
        top: usize,
        width: u16,
    ) -> usize {
        loop {
            let fitted = fit_columns(t, left, rows, top, width);
            let last = fitted.last().map(|(c, _)| *c).unwrap_or(left);
            if cur_col <= last || left >= cur_col {
                return left;
            }
            left += 1;
        }
    }

    #[test]
    fn leftmost_visible_matches_stepping_left_forward() {
        // Columns of many different widths (header and cell widths from 1 to 60, so some are
        // clamped), several rows.
        let ncols = 90;
        let headers: Vec<String> = (0..ncols).map(|c| "h".repeat(1 + (c * 7) % 23)).collect();
        let rows: Vec<Vec<String>> = (0..4)
            .map(|r| {
                (0..ncols)
                    .map(|c| "x".repeat(((c * 13 + r * 5) % 61).max(1)))
                    .collect()
            })
            .collect();
        let g = Tbl {
            headers,
            rows,
            ncols,
        };
        let mut checked = 0;
        for width in [1u16, 5, 20, 40, 80, 200, 1000] {
            for cur in (0..ncols).step_by(3).chain([ncols - 1]) {
                // The caller has already moved `left` back to the cursor when it was past it.
                for min_left in [0, 1.min(cur), cur / 2, cur.saturating_sub(1), cur] {
                    let want = leftmost_by_stepping(&g, cur, min_left, 4, 0, width);
                    let got = leftmost_visible(&g, cur, min_left, 4, 0, width);
                    assert_eq!(got, want, "width {width} cur {cur} min_left {min_left}");
                    checked += 1;
                }
            }
        }
        assert!(checked > 500);
    }

    #[test]
    fn fit_columns_always_yields_one() {
        let t = Tbl::new(&["a", "b"], &[&["1", "2"]]);
        // At least 1 column even when extremely narrow.
        let cols = fit_columns(&t, 0, 1, 0, 1);
        assert!(!cols.is_empty());
        // Both columns when wide enough.
        let cols = fit_columns(&t, 0, 1, 0, 80);
        assert_eq!(cols.len(), 2);
        // The first column is kept even when it alone is wider than the width; the narrow width
        // shrinks its drawn width to the room there is (never below 1).
        let wide = Tbl::new(&["hhhhhhhhhh"], &[&["x"]]);
        assert_eq!(fit_columns(&wide, 0, 1, 0, 4), [(0, 10)]);
        assert_eq!(fit_columns(&wide, 5, 1, 0, 0), [(5, 1)]);
    }

    /// Six columns, each exactly 5 cells wide. Columns that *exactly* fill the width fit
    /// (`fit_columns` keeps a column when `used + gap + w == avail`), so `leftmost_visible` must
    /// reach as far left as `fit_columns` allows: a `>=` instead of `>` leaves one column out
    /// whenever the width is an exact fit, and the cursor scrolls one column too early.
    #[test]
    fn leftmost_visible_counts_columns_that_exactly_fill_the_width() {
        let ncols = 6;
        let g = Tbl {
            headers: (0..ncols).map(|_| "hhhhh".to_string()).collect(),
            rows: vec![(0..ncols).map(|_| "xxxxx".to_string()).collect()],
            ncols,
        };
        let cur = ncols - 1;
        for k in 1..=ncols {
            let exact = (5 * k + COL_GAP * (k - 1)) as u16;
            let left = leftmost_visible(&g, cur, 0, 1, 0, exact);
            assert_eq!(left, ncols - k, "{k} columns fill {exact} exactly");
            // The same answer `fit_columns` gives: from `left` the cursor column is shown...
            let shown: Vec<usize> = fit_columns(&g, left, 1, 0, exact)
                .iter()
                .map(|c| c.0)
                .collect();
            assert_eq!(shown.last(), Some(&cur), "{k}: {shown:?}");
            // ...and one column to the left of it would not fit.
            if left > 0 {
                let shown: Vec<usize> = fit_columns(&g, left - 1, 1, 0, exact)
                    .iter()
                    .map(|c| c.0)
                    .collect();
                assert_ne!(shown.last(), Some(&cur), "{k}: {shown:?}");
            }
            // One cell narrower: one column less.
            if k > 1 {
                assert_eq!(
                    leftmost_visible(&g, cur, 0, 1, 0, exact - 1),
                    ncols - k + 1,
                    "{k}"
                );
            }
        }
    }

    #[test]
    fn a_huge_cell_costs_its_head_not_its_length() {
        let huge = "あ\n".repeat(2_000_000); // 8 MB
        let t = std::time::Instant::now();
        for _ in 0..200 {
            // 128 wide characters (2 columns) and 128 flattened newlines (1 column).
            assert_eq!(flat_width(&huge), 128 * 3);
            assert_eq!(fit_to_width(&huge, 10).width(), 10);
        }
        assert!(t.elapsed().as_millis() < 500, "{:?}", t.elapsed());
        // The head is what `fit_to_width` shows, so the result is the one it gave for the whole.
        assert_eq!(fit_to_width(&huge, 10), "あ あ あ …");
    }

    #[test]
    fn head_cuts_on_a_character_boundary_and_keeps_a_short_text_whole() {
        assert_eq!(head("abc"), "abc");
        assert_eq!(head(""), "");
        let s = "é".repeat(HEAD_CHARS + 10);
        assert_eq!(head(&s).chars().count(), HEAD_CHARS);
        let exact = "x".repeat(HEAD_CHARS);
        assert_eq!(head(&exact), exact);
        // Past the head, the width is still a width of at least a column (what is compared).
        assert!(flat_width(&s) >= MAX_COL_W);
    }
}
