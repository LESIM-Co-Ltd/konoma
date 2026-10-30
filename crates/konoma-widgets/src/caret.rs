//! The caret of a windowed text (the line cursor, the column cursor and the window that follows
//! them), its highlight, and the line `e` opens the editor at. Pure functions over plain values:
//! the konoma host (`App`) and the standalone app both call them, so the same keys move the same
//! caret and `e` names the same line in both.

use ratatui::text::{Line, Span};

/// The active visual selection in a windowed preview, normalized so `start <= end`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Selection {
    /// Not selecting (only the block caret is drawn).
    None,
    /// Charwise (`v`): an inclusive `(line, col)` character range across one or more lines.
    Char {
        start: (usize, usize),
        end: (usize, usize),
    },
    /// Linewise (`V`): whole logical lines `lo..=hi`.
    Line { lo: usize, hi: usize },
}

/// Where the caret and the window are in a text of `total` lines shown `rows` at a time. Lines
/// and columns count from 0. The window `top` is the first line shown.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Caret {
    /// The first line shown.
    pub top: usize,
    /// The line the caret is on.
    pub line: usize,
    /// The column (a char index) of the caret; `usize::MAX` is "the last one" (the draw settles it).
    pub col: usize,
}

impl Caret {
    /// `j` / `k`: the caret moves `delta` lines (kept inside the text) and the window scrolls only
    /// as far as keeps the caret on screen (an always-visible caret).
    pub fn step(&mut self, delta: i32, total: usize, rows: usize) {
        let maxl = total.saturating_sub(1) as i64;
        self.line = (self.line as i64 + i64::from(delta)).clamp(0, maxl.max(0)) as usize;
        self.follow(rows);
    }

    /// Scroll the window just enough that the caret is on screen.
    pub fn follow(&mut self, rows: usize) {
        let vh = rows.max(1);
        if self.line < self.top {
            self.top = self.line;
        } else if self.line >= self.top + vh {
            self.top = self.line + 1 - vh;
        }
    }

    /// A page or a half page (`delta` lines): the window moves by `delta` (stopping at the first
    /// and the last page) and the caret keeps its row on screen.
    pub fn page(&mut self, delta: i32, total: usize, rows: usize) {
        let row_in_view = self.line.saturating_sub(self.top);
        let max_top = total.saturating_sub(rows.max(1)) as i64;
        self.top = (self.top as i64 + i64::from(delta)).clamp(0, max_top.max(0)) as usize;
        self.line = self
            .top
            .saturating_add(row_in_view)
            .min(total.saturating_sub(1));
    }

    /// `g`: the first line.
    pub fn to_top(&mut self) {
        self.line = 0;
        self.top = 0;
    }

    /// `G`: the last line, the window on the last page.
    pub fn to_bottom(&mut self, total: usize, rows: usize) {
        self.top = total.saturating_sub(rows.max(1));
        self.line = total.saturating_sub(1);
    }

    /// `h` / `l`: the column moves `dir` chars (the draw clamps it to the line).
    pub fn col_move(&mut self, dir: i32) {
        self.col = (self.col as i64 + i64::from(dir)).max(0) as usize;
    }

    /// `0`: the first column.
    pub fn col_home(&mut self) {
        self.col = 0;
    }

    /// `$`: the last column (the draw clamps the sentinel to the line).
    pub fn col_end(&mut self) {
        self.col = usize::MAX;
    }
}

/// The column the caret rests on in `lines` (the window starting at line `top`, `rows` rows
/// high): kept on a real character of its line (0 on an empty line, and on a row of the window
/// the reader gave no text for: a file's last empty line). A caret line outside the window keeps
/// `col`.
pub fn settled_col(
    lines: &[Line<'static>],
    top: usize,
    rows: usize,
    line: usize,
    col: usize,
) -> usize {
    match line.checked_sub(top) {
        Some(i) => match lines.get(i) {
            Some(ln) => {
                let len: usize = ln.spans.iter().map(|s| s.content.chars().count()).sum();
                col.min(len.saturating_sub(1))
            }
            None if i < rows => 0,
            None => col,
        },
        None => col,
    }
}

/// The `(display_column, row_index)` of the caret in drawn `lines`, or None if it is not drawn
/// (an empty line). The caret is the span carrying the REVERSED modifier, so its start column is
/// the sum of the widths of the spans before it (a gutter included).
pub fn caret_display_col(lines: &[Line<'static>]) -> Option<(usize, usize)> {
    use ratatui::style::Modifier;
    for (row, line) in lines.iter().enumerate() {
        let mut disp = 0usize;
        for span in &line.spans {
            if span.style.add_modifier.contains(Modifier::REVERSED) {
                return Some((disp, row));
            }
            disp += span.width();
        }
    }
    None
}

/// The sideways scroll of a text that does not wrap, following the caret so it is never off
/// screen: a caret line that fits in `width` is shown from its start; a longer one follows with the
/// least movement (the caret at the edge). `hscroll` is the scroll before; the result is not yet
/// clamped to the widest line.
pub fn follow_hscroll(lines: &[Line<'static>], width: usize, hscroll: u16) -> u16 {
    let Some((caret_disp, caret_row)) = caret_display_col(lines) else {
        return hscroll;
    };
    if lines[caret_row].width() <= width {
        return 0;
    }
    let h = hscroll as usize;
    if caret_disp < h {
        caret_disp as u16
    } else if caret_disp >= h + width {
        (caret_disp + 1 - width) as u16
    } else {
        hscroll
    }
}

/// The character range `[lo, hi)` to highlight on absolute line `abs` for a charwise selection, or None.
/// `usize::MAX` for `hi` means "to end of line". End is inclusive, so the last covered char is `end.1`.
fn char_range_for_line(
    abs: usize,
    start: (usize, usize),
    end: (usize, usize),
) -> Option<(usize, usize)> {
    if abs < start.0 || abs > end.0 {
        return None;
    }
    let lo = if abs == start.0 { start.1 } else { 0 };
    let hi = if abs == end.0 {
        end.1.saturating_add(1)
    } else {
        usize::MAX
    };
    Some((lo, hi))
}

/// Paint the 2D caret and selection onto the visible content lines (char-column based, applied BEFORE the
/// line-number/git gutters so column indices align with the file text). The whole-line cases also set the
/// line's base style so the gutter cells inherit the tint; charwise partial ranges tint only the spanned chars.
pub fn apply_caret(
    lines: Vec<Line<'static>>,
    top_line: usize,
    cursor_line: usize,
    cursor_col: usize,
    sel: Selection,
) -> Vec<Line<'static>> {
    use ratatui::style::Color;
    // For dark themes: a subtle current-line background plus a slightly stronger blue-ish
    // background for the selection.
    const CURSOR_BG: Color = Color::Rgb(55, 60, 74);
    const SEL_BG: Color = Color::Rgb(40, 66, 104);
    lines
        .into_iter()
        .enumerate()
        .map(|(i, line)| {
            let abs = top_line + i;
            let caret = if abs == cursor_line {
                Some(cursor_col)
            } else {
                None
            };
            let (range, whole): (Option<(usize, usize, Color)>, bool) = match sel {
                Selection::None => {
                    if abs == cursor_line {
                        (Some((0, usize::MAX, CURSOR_BG)), true)
                    } else {
                        (None, false)
                    }
                }
                Selection::Line { lo, hi } => {
                    if abs >= lo && abs <= hi {
                        (Some((0, usize::MAX, SEL_BG)), true)
                    } else {
                        (None, false)
                    }
                }
                Selection::Char { start, end } => match char_range_for_line(abs, start, end) {
                    Some((lo, hi)) => (Some((lo, hi, SEL_BG)), lo == 0 && hi == usize::MAX),
                    None => (None, false),
                },
            };
            if range.is_none() && caret.is_none() {
                return line;
            }
            restyle_line(line, range, whole, caret)
        })
        .collect()
}

/// Rebuild a line applying a background to chars in `range` `(lo, hi, color)` and the REVERSED modifier to the
/// `caret` char. When `whole` is set, the line's base style also gets the bg (so gutters inherit it). Consecutive
/// chars with the same resulting style are coalesced into one span to keep the span count low.
fn restyle_line(
    line: Line<'static>,
    range: Option<(usize, usize, ratatui::style::Color)>,
    whole: bool,
    caret: Option<usize>,
) -> Line<'static> {
    use ratatui::style::Modifier;
    let base = line.style;
    let mut out: Vec<Span<'static>> = Vec::new();
    let mut idx = 0usize; // char index within the line content
    let mut cur = String::new();
    let mut cur_style: Option<ratatui::style::Style> = None;
    for span in line.spans.into_iter() {
        for ch in span.content.chars() {
            let mut style = span.style;
            if let Some((lo, hi, color)) = range {
                if idx >= lo && idx < hi {
                    style = style.bg(color);
                }
            }
            if caret == Some(idx) {
                style = style.add_modifier(Modifier::REVERSED);
            }
            if cur_style != Some(style) {
                if !cur.is_empty() {
                    out.push(Span::styled(std::mem::take(&mut cur), cur_style.unwrap()));
                }
                cur_style = Some(style);
            }
            cur.push(ch);
            idx += 1;
        }
    }
    if !cur.is_empty() {
        out.push(Span::styled(cur, cur_style.unwrap_or(base)));
    }
    let mut line = Line::from(out);
    line.style = if whole {
        if let Some((_, _, color)) = range {
            base.bg(color)
        } else {
            base
        }
    } else {
        base
    };
    line
}

// ---- The line `e` opens the editor at --------------------------------------------------------

/// What `e` needs to place the editor on a rendered Markdown document: where the view is and the
/// layout it is drawn from.
pub struct MdView<'a> {
    /// The first display row shown (`preview_scroll`).
    pub scroll: usize,
    /// The display rows of the whole document (what `scroll` is clamped against).
    pub view_rows: usize,
    /// The number of lines of the source file.
    pub src_lines: usize,
    /// The decorated logical lines, as drawn.
    pub lines: &'a [Line<'static>],
    /// The first display row of each logical line plus the total at the end (empty when the
    /// text does not wrap, or the width is not known).
    pub row_prefix: &'a [usize],
    /// The text wraps (`ui.wrap`).
    pub wrap: bool,
    /// The layout width is known (not 0).
    pub width_known: bool,
    /// The on-screen focus (a Tab-selected item): its logical line and its first display row.
    pub focus: Option<(usize, usize)>,
}

/// The 0-based logical line at the top of the view, resolving wrapping so it matches what the
/// renderer draws (`scroll` is in display rows).
pub fn md_top_logical_line(v: &MdView) -> Option<usize> {
    if v.lines.is_empty() {
        return None;
    }
    if !v.wrap || !v.width_known || v.row_prefix.len() != v.lines.len() + 1 {
        return Some(v.scroll.min(v.lines.len() - 1));
    }
    // The prefix is each line's starting display row (increasing): the line holding `scroll` is
    // the last whose start is not past it.
    let i = v
        .row_prefix
        .partition_point(|&p| p <= v.scroll)
        .saturating_sub(1);
    Some(i.min(v.lines.len() - 1))
}

/// Refine the proportional estimate by content: the longest visible span of logical line
/// `logical` (a single span carries no Markdown markers, so its text is a verbatim piece of the
/// source) is looked for in the `source` lines; the line containing it closest to `est` wins.
/// Returns a 0-based source line, or None to fall back to `est`.
pub fn md_content_anchor_line(
    lines: &[Line<'static>],
    logical: usize,
    est: usize,
    source: &[String],
) -> Option<usize> {
    use ratatui::style::Modifier;
    let line = lines.get(logical)?;
    // The longest non-hidden, non-blank span (hidden spans carry link targets, not on-screen text).
    let key = line
        .spans
        .iter()
        .filter(|s| !s.style.add_modifier.contains(Modifier::HIDDEN))
        .map(|s| s.content.trim())
        .filter(|t| !t.is_empty())
        .max_by_key(|t| t.chars().count())?;
    if key.chars().count() < 4 {
        return None;
    }
    let mut best: Option<usize> = None;
    let mut best_d = usize::MAX;
    for (i, sl) in source.iter().enumerate() {
        if sl.contains(key) {
            let d = i.abs_diff(est);
            if d < best_d {
                best_d = d;
                best = Some(i);
            }
        }
    }
    best
}

/// The 1-based source line `e` opens a rendered Markdown document at. Reflow means there is no
/// exact scroll-row to source-line mapping, so the top of the view (or the visible focus) is
/// mapped proportionally (`row / view_rows` of the source) and then refined by content
/// ([`md_content_anchor_line`]). `source` reads the file's lines when the refinement needs them
/// (it must match what the editor will open). None when the layout is empty.
pub fn md_edit_line(v: &MdView, source: &dyn Fn() -> Option<Vec<String>>) -> Option<usize> {
    if v.src_lines == 0 || v.view_rows == 0 {
        return None;
    }
    let (logical, row) = match v.focus {
        Some(hit) => hit,
        None => (md_top_logical_line(v).unwrap_or(0), v.scroll),
    };
    let est = (row * v.src_lines / v.view_rows).min(v.src_lines - 1);
    let refined = source().and_then(|s| md_content_anchor_line(v.lines, logical, est, &s));
    Some(refined.unwrap_or(est) + 1)
}

/// The 1-based line `e` opens a windowed text at: the caret line.
pub fn text_edit_line(caret_line: usize) -> usize {
    caret_line + 1
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Modifier;

    fn ln(s: &str) -> Line<'static> {
        Line::from(s.to_string())
    }

    #[test]
    fn step_moves_the_caret_and_the_window_follows() {
        let mut c = Caret::default();
        c.step(1, 100, 5);
        assert_eq!((c.top, c.line), (0, 1));
        for _ in 0..10 {
            c.step(1, 100, 5);
        }
        // Line 11 with a 5-row window: the window ends on the caret.
        assert_eq!((c.top, c.line), (7, 11));
        c.step(-1, 100, 5);
        assert_eq!((c.top, c.line), (7, 10));
        for _ in 0..10 {
            c.step(-1, 100, 5);
        }
        assert_eq!((c.top, c.line), (0, 0));
        c.step(-1, 100, 5);
        assert_eq!((c.top, c.line), (0, 0));
    }

    #[test]
    fn step_stops_on_the_last_line_and_an_empty_text() {
        let mut c = Caret::default();
        c.step(50, 3, 5);
        assert_eq!((c.top, c.line), (0, 2));
        let mut e = Caret::default();
        e.step(1, 0, 5);
        assert_eq!((e.top, e.line), (0, 0));
    }

    #[test]
    fn page_keeps_the_row_and_stops_at_the_ends() {
        let mut c = Caret {
            top: 0,
            line: 2,
            col: 0,
        };
        c.page(4, 50, 5);
        assert_eq!((c.top, c.line), (4, 6));
        c.page(1000, 50, 5);
        assert_eq!((c.top, c.line), (45, 47));
        c.page(-1000, 50, 5);
        assert_eq!((c.top, c.line), (0, 2));
        // A text shorter than the window: nothing moves but the caret stays inside.
        let mut s = Caret {
            top: 0,
            line: 1,
            col: 0,
        };
        s.page(4, 3, 5);
        assert_eq!((s.top, s.line), (0, 1));
    }

    #[test]
    fn ends_and_columns() {
        let mut c = Caret::default();
        c.to_bottom(100, 10);
        assert_eq!((c.top, c.line), (90, 99));
        c.to_top();
        assert_eq!((c.top, c.line), (0, 0));
        c.to_bottom(3, 10);
        assert_eq!((c.top, c.line), (0, 2));
        c.col_move(-1);
        assert_eq!(c.col, 0);
        c.col_move(1);
        c.col_move(1);
        assert_eq!(c.col, 2);
        c.col_end();
        assert_eq!(c.col, usize::MAX);
        c.col_home();
        assert_eq!(c.col, 0);
    }

    #[test]
    fn settled_col_clamps_inside_the_content_only() {
        let lines = vec![ln("abc"), ln(""), ln("日本")];
        // Inside: clamped to the last char; an empty line gives 0.
        assert_eq!(settled_col(&lines, 10, 5, 10, usize::MAX), 2);
        assert_eq!(settled_col(&lines, 10, 5, 11, 7), 0);
        assert_eq!(settled_col(&lines, 10, 5, 12, 7), 1);
        assert_eq!(settled_col(&lines, 10, 5, 10, 1), 1);
        // In the window but no content (the file's trailing empty line): 0.
        assert_eq!(settled_col(&lines, 10, 5, 13, 7), 0);
        // Outside the window: the column is kept.
        assert_eq!(settled_col(&lines, 10, 5, 15, 7), 7);
        assert_eq!(settled_col(&lines, 10, 5, 3, 7), 7);
    }

    #[test]
    fn text_edit_line_is_one_based() {
        assert_eq!(text_edit_line(0), 1);
        assert_eq!(text_edit_line(41), 42);
    }

    fn md<'a>(
        lines: &'a [Line<'static>],
        prefix: &'a [usize],
        scroll: usize,
        view_rows: usize,
        src_lines: usize,
    ) -> MdView<'a> {
        MdView {
            scroll,
            view_rows,
            src_lines,
            lines,
            row_prefix: prefix,
            wrap: true,
            width_known: true,
            focus: None,
        }
    }

    #[test]
    fn md_top_logical_line_resolves_wrapping() {
        let lines = vec![ln("a"), ln("b"), ln("c")];
        // Line 0 takes rows 0..4, line 1 rows 4..6, line 2 rows 6..7.
        let prefix = [0, 4, 6, 7];
        assert_eq!(md_top_logical_line(&md(&lines, &prefix, 0, 7, 3)), Some(0));
        assert_eq!(md_top_logical_line(&md(&lines, &prefix, 3, 7, 3)), Some(0));
        assert_eq!(md_top_logical_line(&md(&lines, &prefix, 4, 7, 3)), Some(1));
        assert_eq!(md_top_logical_line(&md(&lines, &prefix, 6, 7, 3)), Some(2));
        // Without a usable prefix the scroll is the logical line, clamped.
        assert_eq!(md_top_logical_line(&md(&lines, &[], 5, 7, 3)), Some(2));
        assert_eq!(md_top_logical_line(&md(&[], &[], 5, 7, 3)), None);
    }

    #[test]
    fn md_edit_line_is_proportional_and_refined_by_content() {
        let lines: Vec<_> = (0..4).map(|i| ln(&format!("unique-{i}-text"))).collect();
        let prefix = [0, 10, 20, 30, 40];
        let src: Vec<String> = (0..40).map(|i| format!("row {i}")).collect();
        let none = || None::<Vec<String>>;
        // Top: the first line.
        assert_eq!(
            md_edit_line(&md(&lines, &prefix, 0, 40, 40), &none),
            Some(1)
        );
        // Scrolled to row 20 of 40 rows over 40 source lines: the estimate is 20 (1-based 21),
        // not pinned to the top.
        assert_eq!(
            md_edit_line(&md(&lines, &prefix, 20, 40, 40), &none),
            Some(21)
        );
        // Content refinement moves the estimate to the line holding the text.
        let mut src2 = src.clone();
        src2[17] = "x unique-2-text y".to_string();
        let read = || Some(src2.clone());
        assert_eq!(
            md_edit_line(&md(&lines, &prefix, 20, 40, 40), &read),
            Some(18)
        );
        // Empty layouts give None.
        assert_eq!(md_edit_line(&md(&lines, &prefix, 0, 0, 40), &none), None);
        assert_eq!(md_edit_line(&md(&lines, &prefix, 0, 40, 0), &none), None);
        let _ = src;
    }

    #[test]
    fn md_content_anchor_ignores_hidden_and_short_text() {
        let src = vec!["alpha".to_string(), "see link-target here".to_string()];
        let hidden = Line::from(vec![
            Span::styled(
                "link-target",
                ratatui::style::Style::new().add_modifier(Modifier::HIDDEN),
            ),
            Span::raw("ab"),
        ]);
        assert_eq!(md_content_anchor_line(&[hidden], 0, 0, &src), None);
        let long = Line::from(vec![Span::raw("alpha")]);
        assert_eq!(md_content_anchor_line(&[long], 0, 1, &src), Some(0));
    }

    #[test]
    fn follow_hscroll_keeps_the_caret_in_view() {
        let mk = |pad: usize| {
            let mut spans = vec![Span::raw(" ".repeat(pad))];
            spans.push(Span::styled(
                "x",
                ratatui::style::Style::new().add_modifier(Modifier::REVERSED),
            ));
            vec![Line::from(spans)]
        };
        assert_eq!(follow_hscroll(&mk(3), 20, 0), 0);
        let far = follow_hscroll(&mk(50), 20, 0);
        assert!(
            far > 0 && 50 >= far as usize && 50 < far as usize + 20,
            "{far}"
        );
        // Scrolled too far right: it comes back.
        let back = follow_hscroll(&mk(3), 20, 30);
        assert!(back <= 3, "{back}");
    }
}
