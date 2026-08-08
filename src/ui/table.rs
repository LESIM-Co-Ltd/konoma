//! CSV/TSV table preview rendering.
//!
//! Draws an aligned grid with a fixed header row, rainbow (per-column) colors, and a highlighted
//! cell cursor. Column widths are measured from the header plus the currently visible rows (like
//! csvlens), so opening is instant even for large files. Vertical/horizontal scrolling is done by
//! adjusting the scroll offsets to keep the cursor visible, then writing them back to `App`.

use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph, Wrap};
use ratatui::Frame;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::app::{App, TableCellView};
use crate::i18n::tr;
use crate::preview::table::TableData;

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
const MAX_COL_W: usize = 40;
/// Min column width so short/empty columns still show a cell.
const MIN_COL_W: usize = 3;
/// Spaces between columns.
const COL_GAP: usize = 1;

pub fn render(frame: &mut Frame, app: &mut App, area: Rect) {
    let (cur_row, cur_col) = app.table_cursor();
    let (mut top, mut left) = app.table_scroll();

    let Some(t) = app.table_data() else {
        return; // The caller (preview::render) has already confirmed is_table_preview. Defensive no-op.
    };
    let nrows = t.nrows();
    let ncols = t.ncols;

    let title = build_title(app, nrows, ncols, cur_row, cur_col);
    let block = Block::bordered().title(title);
    let inner = block.inner(area);

    // When there are no columns (an empty file), show only a placeholder.
    if ncols == 0 || inner.width == 0 || inner.height == 0 {
        let para = Paragraph::new(Line::from(Span::styled(
            " (empty) ",
            Style::default().fg(Color::DarkGray),
        )))
        .block(block);
        frame.render_widget(para, area);
        return;
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
    let (cols, _) = loop {
        let fitted = fit_columns(
            t,
            left,
            row_end.saturating_sub(top).max(1),
            top,
            inner.width,
        );
        let last = fitted.last().map(|(c, _)| *c).unwrap_or(left);
        if cur_col <= last || left >= cur_col {
            break (fitted, last);
        }
        left += 1;
    };

    // --- Assembling rows (all owned String = 'static) ---
    let mut lines: Vec<Line<'static>> = Vec::with_capacity(visible_rows + 2);

    // Header row (bold, column color).
    lines.push(compose_line(&cols, |col, w| {
        let text = fit_to_width(t.header(col), w);
        Span::styled(
            text,
            Style::default()
                .fg(column_color(app, col))
                .add_modifier(Modifier::BOLD),
        )
    }));

    // Separator line (dim).
    let sep_w: usize =
        cols.iter().map(|(_, w)| *w).sum::<usize>() + COL_GAP * cols.len().saturating_sub(1);
    lines.push(Line::from(Span::styled(
        "─".repeat(sep_w.min(inner.width as usize)),
        Style::default().fg(Color::DarkGray),
    )));

    // Data rows. The cursor cell is reversed. A search-match cell is shown underlined + bold (using a
    // modifier rather than a background so as not to blot out the rainbow column color). The current match is at the cursor, so reversal makes it clear.
    for r in top..row_end {
        lines.push(compose_line(&cols, |col, w| {
            let text = fit_to_width(t.cell(r, col), w);
            let mut style = Style::default().fg(column_color(app, col));
            if app.table_cell_is_hit(r, col) {
                style = style
                    .add_modifier(Modifier::UNDERLINED)
                    .add_modifier(Modifier::BOLD);
            }
            if r == cur_row && col == cur_col {
                style = style.add_modifier(Modifier::REVERSED);
            }
            Span::styled(text, style)
        }));
    }

    // The borrow of t ends here (lines holds only owned Strings) → App can now be updated mutably.
    let viewport_rows = visible_rows as u16;
    app.set_table_view(top, left, viewport_rows);

    let para = Paragraph::new(lines).block(block);
    frame.render_widget(para, area);
}

/// Fraction of the screen the full-cell popup occupies (generous — the point is reading long
/// content comfortably, not a small dialog like Outline/Info).
const CELL_POPUP_FRACTION: u32 = 4; // out of 5 (80%)
const CELL_POPUP_DENOM: u32 = 5;

/// `Enter` in a table preview: a centered popup showing the **cursor cell's untruncated text**
/// (wrapped, scrollable with j/k/g/G/PageUp/PageDown). This is the read counterpart to `y → c`
/// (which already copies the full, un-truncated cell) — the grid above always shows `…`-truncated
/// cells, with no way to read the rest on screen until now. Reads the cell live from `App` each
/// frame (so an external edit reloading the table, or the popup staying open across an unrelated
/// redraw, always shows the current value) and writes the clamped scroll back, mirroring the
/// render/`set_table_view` convention used by the grid above.
pub fn render_cell_popup(frame: &mut Frame, app: &mut App, area: Rect) {
    let Some(view): Option<TableCellView> = app.table_cell_view() else {
        return; // The caller (ui::render) has already confirmed is_table_cell_open. Defensive no-op.
    };

    let title = format!(
        " {} · {}  r{}/{} c{}/{} ",
        tr(app.lang, crate::i18n::Msg::TableCellTitle),
        view.header,
        view.row,
        view.nrows.max(1),
        view.col,
        view.ncols
    );

    // Popup size: base it on 80% of the screen, fitting it with margin reserved (larger than
    // Outline/Info because this one is for the "read long content comfortably" use case).
    //
    // `.min(upper).max(lower)` — not `.clamp(lower, upper)` — because `upper` is a runtime value
    // (terminal size) that can fall below the fixed `lower` floor on a small terminal, and
    // `Ord::clamp` asserts `min <= max` (a real `assert!`, not `debug_assert!`, so it panics in
    // release builds too). `.min(..).max(..)` has no such ordering to violate; it's the same
    // convention `ui/help.rs`/`ui/dialog.rs` already use for their popups.
    let w = ((area.width as u32 * CELL_POPUP_FRACTION / CELL_POPUP_DENOM) as u16)
        .min(area.width.saturating_sub(2))
        .max(20);
    let h = ((area.height as u32 * CELL_POPUP_FRACTION / CELL_POPUP_DENOM) as u16)
        .min(area.height.saturating_sub(2))
        .max(6);
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    let popup = Rect {
        x,
        y,
        width: w,
        height: h,
    };

    let block = Block::bordered()
        .title(title)
        .title_bottom(
            Line::from(tr(app.lang, crate::i18n::Msg::TableCellActions))
                .centered()
                .dim(),
        )
        .border_style(Style::new().fg(Color::Cyan));
    let inner = block.inner(popup);

    // ratatui doesn't expand tab characters to the terminal's tab stops (which would mismatch
    // unicode-width's width calculation) and instead draws them as a single column as-is, so we
    // flatten them to spaces before wrapping. Real newlines (e.g. a CSV cell with an embedded newline
    // inside quotes) are already line-split by Text::raw (via String::from), so they're left untouched here.
    let body = if view.text.contains('\t') {
        view.text.replace('\t', " ")
    } else {
        view.text
    };

    let mut para = Paragraph::new(body).wrap(Wrap { trim: false });
    // Have ratatui compute the total display row count (wrapping included), and clamp so it never
    // scrolls past the end (the same convention as render_windowed/render_decorated).
    let total_rows = para.line_count(inner.width);
    let max_scroll = total_rows.saturating_sub(inner.height as usize) as u16;
    let scroll = app.table_cell_scroll().min(max_scroll);
    app.set_table_cell_view(scroll, inner.height);

    para = para
        .block(block)
        .scroll((scroll, 0))
        .alignment(Alignment::Left);

    frame.render_widget(Clear, popup); // clear the background before drawing
    frame.render_widget(para, popup);
}

/// Title with path + dimensions + cursor position (+ a truncation note when the file was capped).
fn build_title(app: &App, nrows: usize, ncols: usize, cur_row: usize, cur_col: usize) -> String {
    let path = app
        .tab
        .preview_path
        .clone()
        .map(|p| app.format_path(&p))
        .unwrap_or_else(|| "table".to_string());
    let truncated = app.table_data().map(|t| t.truncated).unwrap_or(false);
    let cap = if truncated { "  (capped)" } else { "" };
    // e.g. " data.csv  r3/100 c2/5  100×5  (capped) "
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

/// Column color: rainbow when enabled, else the terminal default foreground.
fn column_color(app: &App, col: usize) -> Color {
    if app.cfg.ui.csv_rainbow {
        RAINBOW[col % RAINBOW.len()]
    } else {
        Color::Reset
    }
}

/// Which columns (starting at `left`) fit in `width`, and each column's display width.
/// Column width = max(header, visible cells), clamped to [MIN_COL_W, MAX_COL_W]. Always yields at
/// least one column (even if it overflows) so something is always drawn.
fn fit_columns(
    t: &TableData,
    left: usize,
    visible_rows: usize,
    top: usize,
    width: u16,
) -> Vec<(usize, usize)> {
    let mut out: Vec<(usize, usize)> = Vec::new();
    let mut used = 0usize;
    let avail = width as usize;
    for col in left..t.ncols {
        let mut w = t.header(col).width();
        for r in top..(top + visible_rows).min(t.nrows()) {
            w = w.max(flat_width(t.cell(r, col)));
        }
        let w = w.clamp(MIN_COL_W, MAX_COL_W);
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

/// Display width of a cell after flattening embedded newlines/tabs to spaces.
fn flat_width(s: &str) -> usize {
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
    let s = flatten(s);
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

    #[test]
    fn fit_pads_short_and_truncates_long() {
        assert_eq!(fit_to_width("ab", 5), "ab   ");
        assert_eq!(fit_to_width("abcdef", 4), "abc…");
        assert_eq!(fit_to_width("", 3), "   ");
    }

    #[test]
    fn fit_flattens_newlines() {
        assert_eq!(fit_to_width("a\nb", 4), "a b ");
    }

    #[test]
    fn fit_handles_full_width_cjk() {
        // 2 full-width characters = width 4. Fits exactly into width 4.
        assert_eq!(fit_to_width("あい", 4).width(), 4);
        // Truncated to width 3: 1 full-width char (width 2) + … (width 1) = 3.
        assert_eq!(fit_to_width("あい", 3).width(), 3);
    }

    #[test]
    fn fit_columns_always_yields_one() {
        let t = TableData {
            headers: vec!["a".into(), "b".into()],
            rows: vec![vec!["1".into(), "2".into()]],
            ncols: 2,
            truncated: false,
        };
        // At least 1 column even when extremely narrow.
        let cols = fit_columns(&t, 0, 1, 0, 1);
        assert!(!cols.is_empty());
        // Both columns when wide enough.
        let cols = fit_columns(&t, 0, 1, 0, 80);
        assert_eq!(cols.len(), 2);
    }

    // ---- FIXED CRASH: `render_cell_popup` used to panic on a small terminal ----------------
    //
    // `Ord::clamp` is `assert!(min <= max)` + branch — **not** `debug_assert!` — so it panicked
    // in release builds too. The popup width used to be `.clamp(20, area.width.saturating_sub(2))`:
    // once `area.width <= 21`, `saturating_sub(2) <= 19 < 20` and the assert fired. Same story for
    // height with the 6-row floor once `area.height <= 7`. `area` here is the *entire terminal*
    // (`ui/mod.rs` passes `frame.area()`), so simply resizing the terminal small enough while the
    // cell popup was open (CSV/TSV/archive → `Enter` → `Enter`) crashed the whole app.
    //
    // `render_cell_popup` now uses `.min(..).max(..)` (the same convention `ui/help.rs` and
    // `ui/dialog.rs` already used), which has no `min <= max` invariant to violate: the popup
    // rect can legitimately end up larger than the terminal, and `Clear`/`Block`/`Paragraph` all
    // intersect their draw area against the buffer's area before writing a single cell (verified
    // against the `ratatui-widgets` 0.3.2 source: `clear.rs`, `block.rs`, `paragraph.rs` each do
    // `area.intersection(buf.area)` first), so an oversized popup just gets silently clipped
    // instead of panicking. These tests assert that drawing at each size does not panic — the
    // previous, broken behavior is preserved only in git history.

    /// Build an App with a tiny CSV opened as a table preview, with the full-cell popup already
    /// open (mirrors `Sim`'s `select` + `enter` + `enter` in `e2e_tests.rs`, but self-contained
    /// here — that harness is private to another module). Drives the real `tree_activate`/
    /// `toggle_table_cell_view` App methods rather than poking private fields directly.
    fn app_with_open_cell_popup() -> App {
        let dir = crate::test_support::unique_tmp("konoma_table_popup_panic_test");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("t.csv"), "h1,h2\na,b\n").unwrap();
        let mut app = App::new(dir.clone(), crate::config::Config::default()).unwrap();
        let idx = app
            .tab
            .entries
            .iter()
            .position(|e| e.path.ends_with("t.csv"))
            .expect("t.csv should be in the freshly-built tree");
        app.tab.selected = idx;
        app.tree_activate().unwrap();
        assert!(
            app.is_table_preview(),
            "setup precondition: t.csv should open as a table preview"
        );
        app.toggle_table_cell_view();
        assert!(
            app.is_table_cell_open(),
            "setup precondition: the full-cell popup should be open"
        );
        app
    }

    /// Draw just the cell popup (the same call `ui::render` makes: `area` = the whole frame) into
    /// a `w`×`h` `TestBackend` terminal, returning a snapshot of the rendered buffer so callers
    /// can assert on its content — not just that drawing didn't panic. This is what used to
    /// trigger the crash on a small terminal (see the module note above).
    fn draw_cell_popup(app: &mut App, w: u16, h: u16) -> ratatui::buffer::Buffer {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        let completed = term
            .draw(|f| {
                let area = f.area();
                render_cell_popup(f, app, area);
            })
            .unwrap();
        completed.buffer.clone()
    }

    /// True if the buffer has at least one non-blank cell. Used to confirm that a tiny terminal
    /// still gets a *clipped fragment* of the popup (border corner/side) rather than either a
    /// panic or a silent blank screen: `Block`/`Paragraph`/`Clear` all intersect the popup's `Rect`
    /// against the buffer's actual area before writing a single cell (verified against the
    /// `ratatui-widgets` 0.3.2 source — `block.rs`, `paragraph.rs`, `clear.rs` each do
    /// `area.intersection(buf.area)` up front), so an oversized popup degrades to "draw whatever
    /// sliver fits" instead of corrupting the screen.
    fn has_visible_content(buf: &ratatui::buffer::Buffer) -> bool {
        buf.content.iter().any(|c| c.symbol() != " ")
    }

    #[test]
    fn cell_popup_survives_width_20() {
        // area.width.saturating_sub(2) = 18 < 20 → used to be clamp(20, 18), which panicked.
        let mut app = app_with_open_cell_popup();
        draw_cell_popup(&mut app, 20, 30);
    }

    #[test]
    fn cell_popup_survives_width_21_boundary() {
        // area.width.saturating_sub(2) = 19 < 20 → used to be clamp(20, 19), which panicked. This
        // is the exact boundary: one column narrower than the first size that survived even
        // before the fix.
        let mut app = app_with_open_cell_popup();
        draw_cell_popup(&mut app, 21, 30);
    }

    #[test]
    fn cell_popup_survives_width_22() {
        // area.width.saturating_sub(2) = 20 → used to be clamp(20, 20), which was valid even
        // before the fix (min == max is allowed). Non-regression control.
        let mut app = app_with_open_cell_popup();
        draw_cell_popup(&mut app, 22, 30);
    }

    #[test]
    fn cell_popup_survives_height_6() {
        // area.height.saturating_sub(2) = 4 < 6 → used to be clamp(6, 4), which panicked. Width is
        // 80, comfortably above the width floor, so only the height side is under test here.
        let mut app = app_with_open_cell_popup();
        draw_cell_popup(&mut app, 80, 6);
    }

    #[test]
    fn cell_popup_survives_height_7_boundary() {
        // area.height.saturating_sub(2) = 5 < 6 → used to be clamp(6, 5), which panicked.
        let mut app = app_with_open_cell_popup();
        draw_cell_popup(&mut app, 80, 7);
    }

    #[test]
    fn cell_popup_survives_height_8() {
        // area.height.saturating_sub(2) = 6 → used to be clamp(6, 6), which was valid even before
        // the fix. Non-regression control.
        let mut app = app_with_open_cell_popup();
        draw_cell_popup(&mut app, 80, 8);
    }

    #[test]
    fn cell_popup_survives_at_normal_terminal_size() {
        // Non-regression control at an ordinary terminal size (matches `Sim`'s default 90×26 in
        // `e2e_tests.rs`): must not panic, before or after the fix.
        let mut app = app_with_open_cell_popup();
        draw_cell_popup(&mut app, 90, 26);
    }

    // ---- Extreme terminal sizes: below the old crash floor on *both* axes at once, or with one
    // axis reduced to the smallest possible value (1). Each of these also asserts
    // `has_visible_content` — the point isn't only "doesn't panic" but "still draws a clipped,
    // legible fragment of the popup" (per the task: confirm the render isn't corrupted, not just
    // that it didn't crash). Before the fix, all four of these panicked (both the width and the
    // height clamp are violated at once, or at the very least the width one, since `saturating_sub`
    // never lets `area.width - 2` or `area.height - 2` climb back up to the 20/6 floor).

    #[test]
    fn cell_popup_survives_width_1_height_1() {
        let mut app = app_with_open_cell_popup();
        let buf = draw_cell_popup(&mut app, 1, 1);
        assert_eq!(buf.area.width, 1);
        assert_eq!(buf.area.height, 1);
        assert!(
            has_visible_content(&buf),
            "a 1x1 terminal should still show a clipped fragment of the popup border (its \
             top-left corner), not a blank screen"
        );
    }

    #[test]
    fn cell_popup_survives_width_2_height_1() {
        let mut app = app_with_open_cell_popup();
        let buf = draw_cell_popup(&mut app, 2, 1);
        assert_eq!(buf.area.width, 2);
        assert_eq!(buf.area.height, 1);
        assert!(has_visible_content(&buf));
    }

    #[test]
    fn cell_popup_survives_width_1_height_30() {
        let mut app = app_with_open_cell_popup();
        let buf = draw_cell_popup(&mut app, 1, 30);
        assert_eq!(buf.area.width, 1);
        assert_eq!(buf.area.height, 30);
        assert!(
            has_visible_content(&buf),
            "a 1-column-wide terminal should still show a clipped sliver of the popup's left \
             border running down the single visible column, not a blank screen"
        );
    }

    #[test]
    fn cell_popup_survives_width_30_height_1() {
        let mut app = app_with_open_cell_popup();
        let buf = draw_cell_popup(&mut app, 30, 1);
        assert_eq!(buf.area.width, 30);
        assert_eq!(buf.area.height, 1);
        assert!(
            has_visible_content(&buf),
            "a 1-row-tall terminal should still show a clipped sliver of the popup's top border \
             running across the single visible row, not a blank screen"
        );
    }
}
