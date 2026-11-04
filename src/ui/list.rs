//! Styled line lists shared by the bookmark, tab and project screens.

use ratatui::layout::{Alignment, Rect};
use ratatui::text::Line;
use ratatui::widgets::{Block, Paragraph};
use ratatui::Frame;

pub(super) fn render(
    frame: &mut Frame,
    area: Rect,
    lines: Vec<Line<'static>>,
    scroll: u16,
    block: Block<'_>,
) {
    frame.render_widget(
        Paragraph::new(lines)
            .block(block)
            .scroll((scroll, 0))
            .alignment(Alignment::Left),
        area,
    );
}
