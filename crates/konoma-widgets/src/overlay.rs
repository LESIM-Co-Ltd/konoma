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
/// second half is the first column of `area`).
fn blank_wide_before(buf: &mut Buffer, area: Rect) {
    let screen = buf.area;
    if area.x == 0 || area.x > screen.right() {
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
}
