//! Drawing a frame with ratatui.

use ratatui::layout::Rect;
use ratatui::style::{Modifier as RModifier, Style as RStyle};
use ratatui::text::Line as RLine;
use ratatui::widgets::{Paragraph, Widget};

use crate::ratatui_draw;
use crate::style::Line;
use crate::view::CanvasView;

/// What is drawn in the body.
pub enum Frame {
    /// Lines from the top of the body.
    Lines(Vec<Line>),
    /// Cells copied one to one.
    Canvas(CanvasView),
}

/// The footer: the keys first (a narrow terminal cuts the end), where the text is (only for a
/// frame that scrolls: `scrolls`), the title.
pub fn footer(
    hints: &[String],
    title: &str,
    frame: &Frame,
    scroll: usize,
    scrolls: bool,
) -> String {
    let pos = match frame {
        Frame::Lines(l) if scrolls && !l.is_empty() => format!("  line {}", scroll + 1),
        _ => String::new(),
    };
    format!(" {}{pos}  {title}", hints.join("  "))
}

/// The part of `area` the body is drawn in: all of it but the footer row.
pub fn body_of(area: Rect) -> Rect {
    Rect::new(area.x, area.y, area.width, area.height.saturating_sub(1))
}

pub fn draw(f: &mut ratatui::Frame, frame: &Frame, footer: &str) {
    let area = f.area();
    if area.height == 0 {
        return;
    }
    let body = body_of(area);
    match frame {
        Frame::Lines(lines) => {
            let text: Vec<RLine> = lines.iter().map(ratatui_draw::line).collect();
            Paragraph::new(text).render(body, f.buffer_mut());
        }
        Frame::Canvas(c) => ratatui_draw::paint_canvas(f.buffer_mut(), body, c),
    }
    let foot = Rect::new(area.x, area.y + area.height - 1, area.width, 1);
    Paragraph::new(footer.to_string())
        .style(RStyle::default().add_modifier(RModifier::REVERSED))
        .render(foot, f.buffer_mut());
}

#[cfg(test)]
mod tests {
    use ratatui::buffer::Buffer;

    use super::*;
    use crate::style::Style;

    #[test]
    fn a_canvas_is_clipped_to_its_area_and_touches_nothing_outside_it() {
        let mut c = CanvasView::new(6, 5);
        for y in 0..5 {
            c.set_string(0, y, "xxxxxx", Style::new());
        }
        // The area is smaller than the canvas and sits inside a bigger buffer, so a cell that
        // is not clipped would land on the buffer (the rows and columns around the area).
        let mut buf = Buffer::empty(Rect::new(0, 0, 8, 8));
        ratatui_draw::paint_canvas(&mut buf, Rect::new(1, 1, 3, 2), &c);
        for y in 0..8u16 {
            for x in 0..8u16 {
                let inside = (1..4).contains(&x) && (1..3).contains(&y);
                let want = if inside { "x" } else { " " };
                assert_eq!(buf[(x, y)].symbol(), want, "cell {x},{y}");
            }
        }
    }
}
