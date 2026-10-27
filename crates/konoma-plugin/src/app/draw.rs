//! Drawing a frame with ratatui.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color as RColor, Modifier as RModifier, Style as RStyle};
use ratatui::text::{Line as RLine, Span as RSpan};
use ratatui::widgets::{Paragraph, Widget};

use crate::style::{Color, Line, Modifier, Style};
use crate::view::CanvasView;

/// What is drawn in the body.
pub enum Frame {
    /// Lines from the top of the body.
    Lines(Vec<Line>),
    /// Cells copied one to one.
    Canvas(CanvasView),
}

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
    // The bits are at the same positions (documented on `Modifier`).
    RModifier::from_bits_truncate(m.bits())
}

pub fn style(s: &Style) -> RStyle {
    let mut out = RStyle::default();
    if let Some(c) = s.fg {
        out = out.fg(color(c));
    }
    if let Some(c) = s.bg {
        out = out.bg(color(c));
    }
    out.add_modifier(modifier(s.add))
        .remove_modifier(modifier(s.sub))
}

fn line(l: &Line) -> RLine<'static> {
    RLine::from(
        l.spans
            .iter()
            .map(|s| RSpan::styled(s.text.clone(), style(&s.style)))
            .collect::<Vec<_>>(),
    )
    .style(style(&l.style))
}

/// The footer: the keys first (a narrow terminal cuts the end), where the text is, the title.
pub fn footer(title: &str, frame: &Frame, scroll: usize) -> String {
    let pos = match frame {
        Frame::Lines(l) if !l.is_empty() => format!("  line {}", scroll + 1),
        _ => String::new(),
    };
    format!(" q quit  j/k scroll{pos}  {title}")
}

pub fn draw(f: &mut ratatui::Frame, frame: &Frame, footer: &str) {
    let area = f.area();
    if area.height == 0 {
        return;
    }
    let body = Rect::new(area.x, area.y, area.width, area.height - 1);
    match frame {
        Frame::Lines(lines) => {
            let text: Vec<RLine> = lines.iter().map(line).collect();
            Paragraph::new(text).render(body, f.buffer_mut());
        }
        Frame::Canvas(c) => draw_canvas(c, body, f.buffer_mut()),
    }
    let foot = Rect::new(area.x, area.y + area.height - 1, area.width, 1);
    Paragraph::new(footer.to_string())
        .style(RStyle::default().add_modifier(RModifier::REVERSED))
        .render(foot, f.buffer_mut());
}

/// Copy the cells of a canvas to the buffer, clipped to `area`. (Pictures are not drawn here.)
fn draw_canvas(c: &CanvasView, area: Rect, buf: &mut Buffer) {
    let cols = usize::from(c.cols);
    if cols == 0 {
        return;
    }
    for (i, cell) in c.cells.iter().enumerate() {
        let (x, y) = ((i % cols) as u16, (i / cols) as u16);
        if x >= area.width || y >= area.height {
            continue;
        }
        if let Some(dst) = buf.cell_mut((area.x + x, area.y + y)) {
            dst.set_symbol(&cell.symbol).set_style(style(&cell.style));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_canvas_is_clipped_to_its_area_and_touches_nothing_outside_it() {
        let mut c = CanvasView::new(6, 5);
        for y in 0..5 {
            c.set_string(0, y, "xxxxxx", Style::new());
        }
        // The area is smaller than the canvas and sits inside a bigger buffer, so a cell that
        // is not clipped would land on the buffer (the rows and columns around the area).
        let mut buf = Buffer::empty(Rect::new(0, 0, 8, 8));
        draw_canvas(&c, Rect::new(1, 1, 3, 2), &mut buf);
        for y in 0..8u16 {
            for x in 0..8u16 {
                let inside = (1..4).contains(&x) && (1..3).contains(&y);
                let want = if inside { "x" } else { " " };
                assert_eq!(buf[(x, y)].symbol(), want, "cell {x},{y}");
            }
        }
    }
}
