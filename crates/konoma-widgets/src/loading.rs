//! The spinner and the "loading" line both hosts (konoma and the standalone app) show while a
//! picture is decoded on another thread: the braille spinner glyphs and the centred
//! "{glyph}  {message}" row inside a frame.

use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Rect};
use ratatui::widgets::{Paragraph, Widget};

/// The classic braille-pattern spinner (no emoji: single width, colour free).
pub const SPINNER_FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// The glyph of spinner step `frame` (the step wraps).
pub fn spinner_glyph(frame: usize) -> &'static str {
    SPINNER_FRAMES[frame % SPINNER_FRAMES.len()]
}

/// Draw "{spinner}  {msg}" centred on the middle row of `inner`.
pub fn render_spinner_line(buf: &mut Buffer, inner: Rect, spinner: &str, msg: &str) {
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    let line_area = Rect {
        x: inner.x,
        y: inner.y + inner.height / 2,
        width: inner.width,
        height: 1,
    };
    Paragraph::new(format!("{spinner}  {msg}"))
        .alignment(Alignment::Center)
        .render(line_area, buf);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(buf: &Buffer, y: u16, w: u16) -> String {
        (0..w).map(|x| buf[(x, y)].symbol().to_string()).collect()
    }

    #[test]
    fn the_glyph_wraps_around_the_ten_frames() {
        assert_eq!(spinner_glyph(0), "⠋");
        assert_eq!(spinner_glyph(9), "⠏");
        assert_eq!(spinner_glyph(10), "⠋");
        assert_eq!(spinner_glyph(23), SPINNER_FRAMES[3]);
    }

    #[test]
    fn the_line_is_centred_on_the_middle_row_of_the_area() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 7));
        // Inner area: rows 1..6 (5 rows), middle row = 1 + 5 / 2 = 3.
        render_spinner_line(&mut buf, Rect::new(1, 1, 18, 5), "⠋", "loading");
        let text = row(&buf, 3, 20);
        assert!(text.contains("⠋  loading"), "{text:?}");
        let start = text.find('⠋').map(|b| text[..b].chars().count()).unwrap();
        // 10 cells of text in 18: 4 on the left, centred within the area that starts at x = 1.
        assert_eq!(start, 1 + (18 - 10) / 2);
        for y in [0, 1, 2, 4, 5, 6] {
            assert!(
                row(&buf, y, 20).trim().is_empty(),
                "row {y} must stay empty"
            );
        }
    }

    #[test]
    fn an_empty_area_draws_nothing_and_does_not_panic() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 4, 4));
        render_spinner_line(&mut buf, Rect::new(0, 0, 0, 3), "⠋", "x");
        render_spinner_line(&mut buf, Rect::new(0, 0, 3, 0), "⠋", "x");
        assert!(row(&buf, 0, 4).trim().is_empty());
    }
}
