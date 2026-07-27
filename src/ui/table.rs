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
        return; // 呼び出し側(preview::render)が is_table_preview を確認済み。防御的に無処理。
    };
    let nrows = t.nrows();
    let ncols = t.ncols;

    let title = build_title(app, nrows, ncols, cur_row, cur_col);
    let block = Block::bordered().title(title);
    let inner = block.inner(area);

    // 列が無い(空ファイル)ときはプレースホルダのみ。
    if ncols == 0 || inner.width == 0 || inner.height == 0 {
        let para = Paragraph::new(Line::from(Span::styled(
            " (empty) ",
            Style::default().fg(Color::DarkGray),
        )))
        .block(block);
        frame.render_widget(para, area);
        return;
    }

    // レイアウト: 0=ヘッダ / 1=区切り線 / 2.. =データ行。高さが足りなければデータ0行。
    let visible_rows = (inner.height as usize).saturating_sub(2);

    // --- 縦スクロール: カーソル行が見えるよう top を調整 ---
    if visible_rows > 0 {
        if cur_row < top {
            top = cur_row;
        } else if cur_row >= top + visible_rows {
            top = cur_row + 1 - visible_rows;
        }
        // 末尾側の無駄なスクロールを抑える(下端に空きを作らない)。ただしカーソルは見える範囲に保つ。
        let max_top = nrows.saturating_sub(visible_rows);
        top = top.min(max_top).min(cur_row);
    } else {
        top = cur_row.min(nrows.saturating_sub(1));
    }
    let row_end = (top + visible_rows).min(nrows);

    // --- 横スクロール: カーソル列が見えるよう left を調整して、収まる列と幅を確定 ---
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

    // --- 行の組み立て(すべて所有 String = 'static) ---
    let mut lines: Vec<Line<'static>> = Vec::with_capacity(visible_rows + 2);

    // ヘッダ行(太字・列色)。
    lines.push(compose_line(&cols, |col, w| {
        let text = fit_to_width(t.header(col), w);
        Span::styled(
            text,
            Style::default()
                .fg(column_color(app, col))
                .add_modifier(Modifier::BOLD),
        )
    }));

    // 区切り線(dim)。
    let sep_w: usize =
        cols.iter().map(|(_, w)| *w).sum::<usize>() + COL_GAP * cols.len().saturating_sub(1);
    lines.push(Line::from(Span::styled(
        "─".repeat(sep_w.min(inner.width as usize)),
        Style::default().fg(Color::DarkGray),
    )));

    // データ行。カーソルセルは反転。検索中の一致セルは下線 + 太字で示す(列レインボーの色を
    // 潰さないよう、背景ではなく修飾で差をつける)。現在の一致はカーソルがそこに居るので反転で分かる。
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

    // ここで t の借用が終わる(lines は所有 String のみ) → App を可変で更新できる。
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
        return; // 呼び出し側(ui::render)が is_table_cell_open を確認済み。防御的に無処理。
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

    // ポップアップサイズ: 画面の80%を基本に、余白を確保して収める(Outline/Info より大きい =
    // 「長い内容を読む」用途のため)。
    let w = ((area.width as u32 * CELL_POPUP_FRACTION / CELL_POPUP_DENOM) as u16)
        .clamp(20, area.width.saturating_sub(2));
    let h = ((area.height as u32 * CELL_POPUP_FRACTION / CELL_POPUP_DENOM) as u16)
        .clamp(6, area.height.saturating_sub(2));
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

    // ratatui はタブ文字を端末のタブストップへ展開せず(unicode-width の幅計算と食い違う)そのまま
    // 1桁として描くので、折返し前に空白へ均す。実改行(引用符内に改行を持つ CSV セル等)は
    // Text::raw(String::from 経由)がそのまま行分割するので、ここでは触らない。
    let body = if view.text.contains('\t') {
        view.text.replace('\t', " ")
    } else {
        view.text
    };

    let mut para = Paragraph::new(body).wrap(Wrap { trim: false });
    // 総表示行数(折返し込み)を ratatui に計算させ、末尾を超えてスクロールしないようクランプする
    // (render_windowed/render_decorated と同じ流儀)。
    let total_rows = para.line_count(inner.width);
    let max_scroll = total_rows.saturating_sub(inner.height as usize) as u16;
    let scroll = app.table_cell_scroll().min(max_scroll);
    app.set_table_cell_view(scroll, inner.height);

    para = para
        .block(block)
        .scroll((scroll, 0))
        .alignment(Alignment::Left);

    frame.render_widget(Clear, popup); // 下地を消してから描く
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
    // 例: " data.csv  r3/100 c2/5  100×5  (capped) "
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
        // 幅が極端に狭くても先頭列だけは出す。
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
        let budget = w.saturating_sub(1); // '…' の1桁分を残す
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
        // 全角2文字=幅4。幅4にちょうど収まる。
        assert_eq!(fit_to_width("あい", 4).width(), 4);
        // 幅3に切り詰め: 全角1(幅2)+…(幅1)=3。
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
        // 極端に狭くても最低1列。
        let cols = fit_columns(&t, 0, 1, 0, 1);
        assert!(!cols.is_empty());
        // 十分広ければ両列。
        let cols = fit_columns(&t, 0, 1, 0, 80);
        assert_eq!(cols.len(), 2);
    }
}
