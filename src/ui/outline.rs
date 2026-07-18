//! Centered popup for the heading outline (`o` in a Markdown preview). One row per heading, indented
//! by level, with the selected row reversed. `j`/`k`/`g`/`G` move, `Enter` jumps the preview to the
//! heading, `o`/`q`/`Esc` close. Follows the tab-list popup layout.

use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::Line;
use ratatui::widgets::{Block, Clear, Paragraph};
use ratatui::Frame;

use crate::app::App;
use crate::i18n::tr;

pub fn render(frame: &mut Frame, app: &App, area: Rect) {
    let sel = app.outline_sel();
    let items = app.md_outline();
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut sel_line: u16 = 0;

    for (i, (level, text, _)) in items.iter().enumerate() {
        // Indent by level (H1 flush-left); a bullet marks the heading.
        let indent = "  ".repeat((*level).saturating_sub(1) as usize);
        let row = format!(" {indent}• {text}");
        if i == sel {
            sel_line = lines.len() as u16;
            lines.push(Line::from(row).reversed());
        } else {
            lines.push(Line::from(row));
        }
    }
    lines.push(Line::from(""));
    lines.push(Line::from(tr(app.lang, crate::i18n::Msg::OutlineActions)).dim());

    // 幅=最長行(枠込み)、上限70。高さ=内容+枠。画面に収める。
    let content_w = lines.iter().map(Line::width).max().unwrap_or(20) as u16;
    let w = (content_w + 2)
        .clamp(24, 70)
        .min(area.width.saturating_sub(2));
    let h = (lines.len() as u16 + 2)
        .min(area.height.saturating_sub(2))
        .max(3);
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    let popup = Rect {
        x,
        y,
        width: w,
        height: h,
    };

    // 選択行が見えるよう縦スクロール。
    let inner_h = h.saturating_sub(2);
    let max_scroll = (lines.len() as u16).saturating_sub(inner_h);
    let scroll = sel_line
        .saturating_sub(inner_h.saturating_sub(1))
        .min(max_scroll);

    let block = Block::bordered()
        .title(tr(app.lang, crate::i18n::Msg::OutlineTitle))
        .border_style(Style::new().fg(Color::Cyan));
    let para = Paragraph::new(lines)
        .block(block)
        .scroll((scroll, 0))
        .alignment(Alignment::Left);

    frame.render_widget(Clear, popup);
    frame.render_widget(para, popup);
}
