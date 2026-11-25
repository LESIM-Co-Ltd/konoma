//! The shared centered dialog frame for ordinary operations and project prompts.

use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Rect},
    style::{Color, Style},
    text::Line,
    widgets::{Block, Paragraph, Widget},
};

pub fn draw_dialog_box(
    buf: &mut Buffer,
    area: Rect,
    lines: Vec<Line<'static>>,
    title: &str,
    accent: Color,
    selected: usize,
) {
    // The legacy dialogs and project prompts share exactly the same frame.
    let w = 66.min(area.width.saturating_sub(2)).max(24);
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

    let block = Block::bordered()
        .title(title)
        .border_style(Style::new().fg(accent));
    let para = Paragraph::new(lines)
        .block(block)
        .scroll((
            selected
                .saturating_sub(usize::from(h.saturating_sub(3)))
                .min(u16::MAX as usize) as u16,
            0,
        ))
        .alignment(Alignment::Left);

    crate::overlay::clear_overlay(buf, popup);
    para.render(popup, buf);
}
