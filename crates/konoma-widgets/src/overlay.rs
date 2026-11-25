//! Clearing the ground under an overlay (a dialog, a menu, a popup, the help panel).
//!
//! `Clear` resets the cells of the overlay's rectangle, but the buffer keeps a wide character
//! whole in its first cell, with the cell after it as the "second half". When such a character
//! stands just left of the overlay with its second half under the overlay's first column, ratatui
//! sees that column as the second half and does not send what the overlay draws there. The
//! terminal then shows half of a character where the border should be. Every overlay clears
//! through [`clear_overlay`] so this is handled in one place.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::widgets::{Clear, Widget};
use unicode_width::UnicodeWidthStr;

/// Blank a wide character cut by the left edge of `area`, then clear `area`.
pub fn clear_overlay(buf: &mut Buffer, area: Rect) {
    blank_wide_before(buf, area);
    Clear.render(area, buf);
}

/// Reset the cell left of `area` in each of its rows when it holds a wide character (whose
/// second half is the first column of `area`). A buffer need not start at column 0: no cell is
/// left of an overlay that starts at the buffer's own left edge.
fn blank_wide_before(buf: &mut Buffer, area: Rect) {
    let screen = buf.area;
    if area.x <= screen.left() || area.x > screen.right() {
        return;
    }
    for y in area.top().max(screen.top())..area.bottom().min(screen.bottom()) {
        let cell = &mut buf[(area.x - 1, y)];
        if cell.symbol().width() > 1 {
            cell.reset();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use konoma_test_support::assert_overlay_over_wide_background as check;
    use ratatui::widgets::Block;

    fn bordered(f: &mut ratatui::Frame, area: Rect) {
        clear_overlay(f.buffer_mut(), area);
        Block::bordered().title("t").render(area, f.buffer_mut());
    }

    #[test]
    fn a_bordered_overlay_looks_the_same_over_wide_characters() {
        let r = check("clear_overlay", 40, 10, |f| {
            bordered(f, Rect::new(11, 2, 17, 5))
        });
        assert_eq!(r, Rect::new(11, 2, 17, 5));
    }

    /// At the screen's edges (no cell to the left, none to the right) nothing is out of range.
    #[test]
    fn an_overlay_at_the_screens_edges_is_drawn() {
        check("left edge", 40, 10, |f| bordered(f, Rect::new(0, 0, 10, 4)));
        check("right edge", 40, 10, |f| {
            bordered(f, Rect::new(30, 5, 10, 5))
        });
    }

    /// A buffer whose origin is not column 0 (a sub-area drawn into): an overlay on its left edge
    /// has no cell to its left, and one inside blanks the wide character it cuts.
    #[test]
    fn a_buffer_that_does_not_start_at_column_zero_is_handled() {
        let area = Rect::new(5, 2, 20, 4);
        let mut buf = Buffer::empty(area);
        buf.set_string(9, 3, "あ", ratatui::style::Style::default());
        // On the left edge of the buffer: nothing is left of it (this used to index column 4).
        clear_overlay(&mut buf, Rect::new(5, 2, 3, 3));
        assert_eq!(buf[(9, 3)].symbol(), "あ", "outside the overlay: untouched");
        // Its second half is the overlay's first column: the first half is blanked.
        clear_overlay(&mut buf, Rect::new(10, 2, 8, 3));
        assert_eq!(buf[(9, 3)].symbol(), " ");
    }
}
