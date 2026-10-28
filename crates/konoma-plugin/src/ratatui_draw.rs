//! Drawing the display types with ratatui: the one place that turns a plugin's
//! [`Style`] and [`Line`] into ratatui's, and copies a [`CanvasView`] into a ratatui buffer.
//!
//! Direction: **display types -> ratatui** (for a host that draws). The sibling crate
//! `konoma-plugin-ratatui` goes the other way (a ratatui `Buffer` -> a `CanvasView`, for a plugin
//! that draws with ratatui widgets). The host (konoma) and the standalone app share this module, so
//! both draw a canvas under the same safety rules.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color as RColor, Modifier as RModifier, Style as RStyle};
use ratatui::text::{Line as RLine, Span as RSpan};

use crate::style::{Color, Line, Modifier, Style};
use crate::view::CanvasView;

/// A plugin color as ratatui's.
pub fn color(c: Color) -> RColor {
    match c {
        Color::Reset => RColor::Reset,
        Color::Black => RColor::Black,
        Color::Red => RColor::Red,
        Color::Green => RColor::Green,
        Color::Yellow => RColor::Yellow,
        Color::Blue => RColor::Blue,
        Color::Magenta => RColor::Magenta,
        Color::Cyan => RColor::Cyan,
        Color::Gray => RColor::Gray,
        Color::DarkGray => RColor::DarkGray,
        Color::LightRed => RColor::LightRed,
        Color::LightGreen => RColor::LightGreen,
        Color::LightYellow => RColor::LightYellow,
        Color::LightBlue => RColor::LightBlue,
        Color::LightMagenta => RColor::LightMagenta,
        Color::LightCyan => RColor::LightCyan,
        Color::White => RColor::White,
        Color::Indexed(i) => RColor::Indexed(i),
        Color::Rgb(r, g, b) => RColor::Rgb(r, g, b),
    }
}

fn modifier(m: Modifier) -> RModifier {
    // The bits are at the same positions (the API promises it); bits ratatui does not know are
    // dropped.
    RModifier::from_bits_truncate(m.bits())
}

/// A plugin style as ratatui's.
pub fn style(s: &Style) -> RStyle {
    let mut out = RStyle::default()
        .add_modifier(modifier(s.add))
        .remove_modifier(modifier(s.sub));
    out.fg = s.fg.map(color);
    out.bg = s.bg.map(color);
    out
}

/// A plugin line as ratatui's (copies the text; see [`into_line`] to move it).
pub fn line(l: &Line) -> RLine<'static> {
    RLine::from(
        l.spans
            .iter()
            .map(|s| RSpan::styled(s.text.clone(), style(&s.style)))
            .collect::<Vec<_>>(),
    )
    .style(style(&l.style))
}

/// A plugin line as ratatui's, moving the text.
pub fn into_line(l: Line) -> RLine<'static> {
    let line_style = style(&l.style);
    let spans: Vec<RSpan<'static>> = l
        .spans
        .into_iter()
        .map(|s| RSpan::styled(s.text, style(&s.style)))
        .collect();
    RLine::from(spans).style(line_style)
}

/// Copy the cells of `canvas` into `buf` inside `inner`, clipped to it.
///
/// `CanvasView::check` is the contract and the host refuses a canvas that fails it; this is the
/// second line: whatever the cells hold, a control character is never written to the buffer (it
/// would reach the terminal as an escape sequence), it becomes a space. All positions are
/// computed in `u32`.
pub fn paint_canvas(buf: &mut Buffer, inner: Rect, canvas: &CanvasView) {
    let cols = canvas.cols as usize;
    if cols == 0 {
        return;
    }
    let show_cols = (canvas.cols).min(inner.width) as u32;
    let show_rows = (canvas.rows).min(inner.height) as u32;
    for y in 0..show_rows {
        for x in 0..show_cols {
            let Some(cell) = canvas.cells.get(y as usize * cols + x as usize) else {
                return;
            };
            if cell.is_continuation() {
                continue;
            }
            // A wide character whose right half is outside the area is not drawn.
            let width = crate::symbol_width(&cell.symbol).max(1) as u32;
            if x + width > show_cols && width > 1 {
                continue;
            }
            let (Ok(tx), Ok(ty)) = (
                u16::try_from(inner.x as u32 + x),
                u16::try_from(inner.y as u32 + y),
            ) else {
                continue;
            };
            let Some(target) = buf.cell_mut((tx, ty)) else {
                continue;
            };
            let style = style(&cell.style);
            if cell
                .symbol
                .chars()
                .any(|c| c.is_control() || c == crate::view::KITTY_PLACEHOLDER)
            {
                target.set_symbol(" ").set_style(style);
            } else {
                target.set_symbol(&cell.symbol).set_style(style);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A buffer as text a snapshot test can pin: the rows as the terminal shows them (a wide glyph
    /// takes its cell and the next one counts as covered; trailing blanks are cut, `|` ends a row),
    /// then one line per run of equally styled cells whose style is not the default.
    fn buffer_dump(buf: &Buffer) -> String {
        use unicode_width::UnicodeWidthStr;
        let area = buf.area;
        let mut rows = String::new();
        let mut runs = String::new();
        for y in area.top()..area.bottom() {
            let mut line = String::new();
            type Key = (RColor, RColor, RModifier);
            let mut run: Option<(u16, Key, String)> = None;
            let mut flush = |run: &mut Option<(u16, Key, String)>, end: u16| {
                if let Some((start, (fg, bg, m), text)) = run.take() {
                    if (fg, bg, m) != (RColor::Reset, RColor::Reset, RModifier::empty()) {
                        runs.push_str(&format!(
                            "r{y} c{start}..{} fg={fg:?} bg={bg:?} mod={m:?} |{}|
",
                            end - 1,
                            text.trim_end()
                        ));
                    }
                }
            };
            let mut x = area.left();
            while x < area.right() {
                let cell = &buf[(x, y)];
                let sym = cell.symbol();
                let w = sym.width().max(1) as u16;
                line.push_str(sym);
                let key = (cell.fg, cell.bg, cell.modifier);
                match &mut run {
                    Some((_, k, text)) if *k == key => text.push_str(sym),
                    _ => {
                        flush(&mut run, x);
                        run = Some((x, key, sym.to_string()));
                    }
                }
                x += w;
            }
            flush(&mut run, x);
            rows.push_str(&format!(
                "{y:>2}|{}|
",
                line.trim_end()
            ));
        }
        format!(
            "{rows}--- styles ---
{runs}"
        )
    }

    fn canvas_of(cols: u16, symbols: &[&str]) -> CanvasView {
        let mut c = CanvasView::new(cols, 1);
        for (i, s) in symbols.iter().enumerate() {
            c.cells[i].symbol = s.to_string();
        }
        c
    }

    fn painted(canvas: &CanvasView, inner: Rect) -> Buffer {
        let mut buf = Buffer::empty(Rect::new(0, 0, 12, 4));
        paint_canvas(&mut buf, inner, canvas);
        buf
    }

    /// The whole painted buffer (glyphs and styles) for a canvas that mixes plain ASCII, wide and
    /// half-width kana, a control character, a style on a wide glyph and on its trailing cell.
    fn mixed_canvas() -> CanvasView {
        use crate::{Color as C, Style as S};
        let mut c = canvas_of(
            12,
            &[
                "A", "日", "", "ｶﾞ", "", "\u{1b}", "b", "本", "", "c", "d", "e",
            ],
        );
        c.cells[0].style = S::new().fg(C::Red).bold();
        c.cells[1].style = S::new().fg(C::Rgb(10, 20, 30)).bg(C::Blue);
        c.cells[2].style = S::new().bg(C::Red); // trailing half: must not be painted
        c.cells[6].style = S::new().underlined();
        c.cells[7].style = S::new().fg(C::Green);
        c
    }

    #[test]
    fn paint_snapshot_mixed_content_fully_inside_the_area() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 14, 3));
        paint_canvas(&mut buf, Rect::new(1, 1, 12, 1), &mixed_canvas());
        let got = buffer_dump(&buf);
        assert_eq!(
            got,
            r####" 0||
 1| A日ｶﾞ  b本cde|
 2||
--- styles ---
r1 c1..1 fg=Red bg=Reset mod=BOLD |A|
r1 c2..3 fg=Rgb(10, 20, 30) bg=Blue mod=NONE |日|
r1 c7..7 fg=Reset bg=Reset mod=UNDERLINED |b|
r1 c8..9 fg=Green bg=Reset mod=NONE |本|
"####,
            "paint output changed"
        );
    }

    #[test]
    fn paint_snapshot_mixed_content_cut_by_a_narrow_area() {
        // 7 columns: the half-width kana pair ends at column 5, the wide "本" would need 2 and
        // does not fit in the last one.
        let mut buf = Buffer::empty(Rect::new(0, 0, 9, 3));
        paint_canvas(&mut buf, Rect::new(1, 1, 7, 1), &mixed_canvas());
        let got = buffer_dump(&buf);
        assert_eq!(
            got,
            r####" 0||
 1| A日ｶﾞ  b|
 2||
--- styles ---
r1 c1..1 fg=Red bg=Reset mod=BOLD |A|
r1 c2..3 fg=Rgb(10, 20, 30) bg=Blue mod=NONE |日|
r1 c7..7 fg=Reset bg=Reset mod=UNDERLINED |b|
"####,
            "paint output changed"
        );
    }

    #[test]
    fn paint_never_writes_a_control_character_even_if_check_was_bypassed() {
        // None of these would pass `CanvasView::check`; the host paints them anyway as spaces.
        let bad = [
            "\u{1b}[31m",
            "\u{1b}",
            "\u{7}",
            "\n",
            "\r",
            "\u{7f}",
            "\u{9b}",
            "a\u{1b}",
            "\u{90}",
        ];
        let canvas = canvas_of(bad.len() as u16, &bad);
        assert!(canvas.check().is_err());
        let buf = painted(&canvas, Rect::new(1, 1, 10, 2));
        for cell in &buf.content {
            assert!(
                !cell.symbol().chars().any(char::is_control),
                "{:?} reached the buffer",
                cell.symbol()
            );
        }
        for i in 0..bad.len() as u16 {
            assert_eq!(buf[(1 + i, 1)].symbol(), " ", "cell {i}");
        }
    }

    #[test]
    fn paint_replaces_the_kitty_placeholder_with_a_space() {
        let bad = ["\u{10EEEE}", "\u{10EEEE}\u{0305}\u{0305}"];
        let canvas = canvas_of(bad.len() as u16, &bad);
        assert!(canvas.check().is_err());
        let buf = painted(&canvas, Rect::new(1, 1, 10, 2));
        for cell in &buf.content {
            assert!(!cell.symbol().contains('\u{10EEEE}'), "{:?}", cell.symbol());
        }
        assert_eq!(buf[(1, 1)].symbol(), " ");
        assert_eq!(buf[(2, 1)].symbol(), " ");
    }

    #[test]
    fn paint_still_draws_ordinary_symbols_and_styles() {
        let mut canvas = canvas_of(3, &["a", "日", ""]);
        canvas.cells[0].style = crate::Style::new().fg(crate::Color::Red);
        let buf = painted(&canvas, Rect::new(2, 1, 8, 2));
        assert_eq!(buf[(2, 1)].symbol(), "a");
        assert_eq!(buf[(2, 1)].fg, ratatui::style::Color::Red);
        assert_eq!(buf[(3, 1)].symbol(), "日");
    }

    #[test]
    fn paint_leaves_the_trailing_cell_of_a_wide_character_alone() {
        // The right half of a wide character belongs to the left cell: its own style (the plugin
        // may have set one) is not painted over the terminal's two-column cell.
        let mut canvas = canvas_of(3, &["日", "", "a"]);
        canvas.cells[1].style = crate::Style::new().bg(crate::Color::Red);
        let buf = painted(&canvas, Rect::new(0, 0, 3, 1));
        assert_eq!(buf[(0, 0)].symbol(), "日");
        assert_eq!(buf[(1, 0)].symbol(), " ");
        assert_eq!(buf[(1, 0)].bg, ratatui::style::Color::Reset);
        assert_eq!(buf[(2, 0)].symbol(), "a");
    }

    #[test]
    fn paint_counts_a_voiced_halfwidth_kana_as_two_columns_like_ratatui() {
        // "ｶﾞ" is two columns for ratatui (the voiced mark has a column of its own), so at the right
        // edge of the area it is cut like any wide character instead of being drawn half over.
        let canvas = canvas_of(3, &["a", "ｶﾞ", ""]);
        assert_eq!(canvas.check(), Ok(()));
        let buf = painted(&canvas, Rect::new(0, 0, 2, 1));
        assert_eq!(buf[(0, 0)].symbol(), "a");
        assert_eq!(buf[(1, 0)].symbol(), " ");
        let buf = painted(&canvas, Rect::new(0, 0, 3, 1));
        assert_eq!(buf[(1, 0)].symbol(), "ｶﾞ");
    }

    #[test]
    fn paint_stops_at_the_cells_it_has_for_a_huge_claimed_canvas() {
        // A canvas that claims 65535 columns but holds 3 cells: positions are summed in u32,
        // nothing wraps or panics, and painting stops where the cells end.
        let mut canvas = CanvasView::new(0, 0);
        canvas.cols = u16::MAX;
        canvas.rows = 1;
        canvas.cells = vec![crate::view::CanvasCell::new("日", Default::default()); 3];
        let inner = Rect::new(0, 0, u16::MAX, 1);
        let mut buf = Buffer::empty(Rect::new(0, 0, 4, 1));
        paint_canvas(&mut buf, inner, &canvas);
        assert_eq!(buf[(0, 0)].symbol(), "日");
    }
}
