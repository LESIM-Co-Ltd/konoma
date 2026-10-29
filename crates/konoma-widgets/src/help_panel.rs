//! The `?` help panel and the footer's key hints, drawn the same way by every host.
//!
//! konoma and the standalone plugin app both put the help in a bordered panel at most 66 cells
//! wide, centred over the screen they draw, and both list their key hints in one footer line that
//! keeps the first tokens when the terminal is narrow. The geometry, the look of the rows and the
//! fitting live here; what the rows say comes from the host.

use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph, Widget};
use unicode_width::UnicodeWidthStr;

/// The widest the help panel gets, in cells.
pub const PANEL_WIDTH: u16 = 66;
/// The width of the key column of a help row, in cells.
const KEY_COLUMN: usize = 16;

/// `s` padded on the right to `width` cells (a full-width character is two).
fn pad_cells(s: &str, width: usize) -> String {
    let w = s.width();
    format!("{s}{}", " ".repeat(width.saturating_sub(w)))
}

/// The help body as lines: each section is a bold underlined heading and its rows
/// (`  key (16 cells)  description (dim)`), with a blank line between sections.
pub fn help_lines(sections: &[(&str, &[(String, String)])]) -> Vec<Line<'static>> {
    let mut out: Vec<Line<'static>> = Vec::new();
    for (title, rows) in sections {
        if !out.is_empty() {
            out.push(Line::from(""));
        }
        out.push(Line::from(
            Span::from((*title).to_string())
                .add_modifier(Modifier::BOLD)
                .underlined(),
        ));
        for (key, desc) in *rows {
            // A key column wider than its 16 cells (`Ctrl-f / Ctrl-b  ·  PageDown / PageUp`)
            // still leaves a gap before the description.
            let pad = if key.width() >= KEY_COLUMN { "  " } else { "" };
            out.push(Line::from(vec![
                Span::from(format!("  {}{pad}", pad_cells(key, KEY_COLUMN))).bold(),
                Span::from(desc.clone()).dim(),
            ]));
        }
    }
    out
}

/// The help panel's frame and the number of body rows inside it. The width caps at
/// [`PANEL_WIDTH`], the height is the content plus the border; both are fitted to `area` and the
/// panel is centred in it.
pub fn popup_layout(line_count: usize, area: Rect) -> (Rect, u16) {
    let w = PANEL_WIDTH.min(area.width.saturating_sub(2)).max(20);
    let h = (line_count as u16 + 2)
        .min(area.height.saturating_sub(2))
        .max(3);
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    (
        Rect {
            x,
            y,
            width: w,
            height: h,
        },
        h.saturating_sub(2),
    )
}

/// How far a help of `line_count` lines can scroll in `area`.
pub fn max_scroll(line_count: usize, area: Rect) -> u16 {
    let (_, inner_h) = popup_layout(line_count, area);
    (line_count as u16).saturating_sub(inner_h)
}

/// Draw the help over what is already in `buf`: the cells under the panel are cleared first, so a
/// picture or text beneath does not show through. `area` is the whole screen the panel is
/// centred in; `scroll` is clamped to what the content allows.
pub fn render_panel(
    buf: &mut Buffer,
    area: Rect,
    lines: Vec<Line<'static>>,
    scroll: u16,
    title: String,
) {
    let (popup, _) = popup_layout(lines.len(), area);
    let scroll = scroll.min(max_scroll(lines.len(), area));
    let block = Block::bordered()
        .title(title)
        .border_style(Style::new().fg(Color::Cyan));
    let para = Paragraph::new(lines)
        .block(block)
        .scroll((scroll, 0))
        .alignment(Alignment::Left);
    blank_wide_before(buf, popup);
    Clear.render(popup, buf);
    para.render(popup, buf);
}

/// Blank a wide character that stands left of `popup` with its second half under the panel's
/// first column. The buffer keeps the character whole, so ratatui would skip the panel's border
/// cell there (it is the wide character's second half) and the terminal would show half of a
/// character where the border should be.
fn blank_wide_before(buf: &mut Buffer, popup: Rect) {
    let area = buf.area;
    if popup.x == 0 || popup.x > area.right() {
        return;
    }
    for y in popup.top().max(area.top())..popup.bottom().min(area.bottom()) {
        let cell = &mut buf[(popup.x - 1, y)];
        if cell.symbol().width() > 1 {
            cell.reset();
        }
    }
}

/// Join `tokens` with two spaces and keep only as many as fit in `max` cells. When some are cut,
/// ` …` ends the line to say there are more. The first tokens win.
pub fn fit_tokens(tokens: &[String], max: u16) -> String {
    const SEP: &str = "  ";
    let max = max as usize;
    let ellipsis = " …";
    let mut out = String::new();
    let mut used = 0usize;
    let mut shown = 0usize;
    for tok in tokens {
        let add = if out.is_empty() {
            tok.width()
        } else {
            SEP.width() + tok.width()
        };
        // If tokens remain, reserve room for the ellipsis and check whether it still fits.
        let reserve = if shown + 1 < tokens.len() {
            ellipsis.width()
        } else {
            0
        };
        if used + add + reserve > max {
            break;
        }
        if !out.is_empty() {
            out.push_str(SEP);
        }
        out.push_str(tok);
        used += add;
        shown += 1;
    }
    if shown < tokens.len() && shown > 0 {
        out.push_str(ellipsis);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A wide character whose second half is under the panel's first column must not stay in the
    /// buffer: ratatui would skip the border cell as that half.
    #[test]
    fn a_wide_character_across_the_panels_left_edge_is_blanked() {
        let area = Rect::new(0, 0, 100, 40);
        let (popup, _) = popup_layout(10, area);
        let mut buf = Buffer::empty(area);
        for y in 0..area.height {
            // Wide characters ending on the panel's first column, and one clear of the panel.
            buf[(popup.x - 1, y)].set_symbol("日");
            buf[(2, y)].set_symbol("本");
        }
        render_panel(&mut buf, area, vec![Line::raw("x"); 10], 0, "Help".into());
        for y in popup.top()..popup.bottom() {
            assert_eq!(buf[(popup.x - 1, y)].symbol(), " ", "row {y}");
        }
        assert_eq!(buf[(popup.x, popup.y)].symbol(), "┌");
        // Outside the panel's rows the character stays.
        assert_eq!(buf[(popup.x - 1, 0)].symbol(), "日");
        assert_eq!(buf[(2, popup.y)].symbol(), "本");
    }

    #[test]
    fn the_panel_is_centred_and_capped() {
        let (p, inner) = popup_layout(10, Rect::new(0, 0, 100, 40));
        assert_eq!((p.width, p.height, inner), (66, 12, 10));
        assert_eq!((p.x, p.y), (17, 14));
        // A narrow screen leaves a column on each side; a short one two rows.
        let (p, inner) = popup_layout(50, Rect::new(0, 0, 30, 10));
        assert_eq!((p.x, p.y, p.width, p.height, inner), (1, 1, 28, 8, 6));
        // Never narrower than 20 or lower than 3.
        let (p, _) = popup_layout(0, Rect::new(0, 0, 10, 2));
        assert_eq!((p.width, p.height), (20, 3));
    }

    #[test]
    fn rows_have_a_sixteen_cell_key_column_and_a_gap_after_a_wide_key() {
        let rows = vec![
            ("j".to_string(), "down".to_string()),
            (
                "Ctrl-f / Ctrl-b  ·  PageDown / PageUp".to_string(),
                "page".to_string(),
            ),
        ];
        let lines = help_lines(&[("Move", &rows), ("Other", &rows[..1])]);
        let text: Vec<String> = lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        assert_eq!(text[0], "Move");
        assert_eq!(text[1], "  j               down");
        assert_eq!(text[2], "  Ctrl-f / Ctrl-b  ·  PageDown / PageUp  page");
        assert_eq!(text[3], "");
        assert_eq!(text[4], "Other");
    }

    #[test]
    fn tokens_fit_by_priority_and_an_ellipsis_says_more() {
        let toks: Vec<String> = ["aaa", "bbb", "ccc", "ddd"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(fit_tokens(&toks, 80), "aaa  bbb  ccc  ddd");
        let some = fit_tokens(&toks, 12);
        assert!(some.starts_with("aaa") && some.ends_with('…'), "{some}");
        assert!(!some.contains("ddd"), "{some}");
        assert_eq!(fit_tokens(&toks, 0), "");
    }
}
