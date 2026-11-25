//! The top edge of a framed project list: `┌`, the title at the left, the screen tabs at the
//! right end, `┐`. The expected text and the tab columns come from the display widths of the
//! title and the tabs, so no test writes a column number. When title and tabs do not both fit
//! between the corners the tabs are dropped; the title never is.
//!
//! One implementation for the host's tests and the standalone app's tests.
use ratatui::{buffer::Buffer, layout::Rect};
use std::ops::Range;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Display width of a text in cells.
pub fn width(text: &str) -> u16 {
    text.width() as u16
}

/// Whether tabs of this text are drawn on an edge `edge_width` cells wide (corners included).
pub fn tabs_fit(edge_width: u16, title: &str, tabs: &str) -> bool {
    !tabs.is_empty() && width(title) + width(tabs) <= edge_width.saturating_sub(2)
}

/// The columns the tabs occupy on the edge of `area` (immediately left of the right corner),
/// `None` when they are dropped.
pub fn tabs_columns(area: Rect, title: &str, tabs: &str) -> Option<Range<u16>> {
    tabs_columns_of_width(area, width(title), width(tabs))
}

/// [`tabs_columns`] for tabs whose drawn width is known (read off a frozen row) and a title of
/// a known width.
pub fn tabs_columns_of_width(area: Rect, title_width: u16, tabs_width: u16) -> Option<Range<u16>> {
    (tabs_width > 0 && title_width + tabs_width <= area.width.saturating_sub(2)).then(|| {
        let end = area.right() - 1;
        end - tabs_width..end
    })
}

/// The title cut to `room` cells, as a bordered block cuts it.
fn clipped(title: &str, room: u16) -> String {
    let (mut out, mut used) = (String::new(), 0u16);
    for c in title.chars() {
        let w = c.width().unwrap_or(0) as u16;
        if used + w > room {
            break;
        }
        used += w;
        out.push(c);
    }
    out
}

/// The whole edge as text: corners, title, `─` filler, tabs.
pub fn expected_text(edge_width: u16, title: &str, tabs: &str) -> String {
    let room = edge_width.saturating_sub(2);
    // The host decides whether the tabs fit from the title's full width, before the block cuts
    // the title to the room; a title cut to fit never makes room for tabs.
    let tabs = if tabs_fit(edge_width, title, tabs) {
        tabs
    } else {
        ""
    };
    let title = clipped(title, room);
    let fill = room - width(&title) - width(tabs);
    format!("┌{title}{}{tabs}┐", "─".repeat(usize::from(fill)))
}

/// Row `y` of the buffer over `xs` as text; a wide glyph counts once (the cell after it is
/// its continuation and the test backend may keep a stale symbol there).
pub fn row_text(buf: &Buffer, y: u16, xs: Range<u16>) -> String {
    let mut out = String::new();
    let mut x = xs.start;
    while x < xs.end {
        let symbol = buf[(x, y)].symbol();
        out.push_str(symbol);
        x += symbol.width().max(1) as u16;
    }
    out
}

/// The top row of `area` is the edge of a frame with this title and these tabs.
pub fn check(buf: &Buffer, area: Rect, title: &str, tabs: &str) -> Result<(), String> {
    let actual = row_text(buf, area.y, area.x..area.right());
    let expected = expected_text(area.width, title, tabs);
    if actual == expected {
        Ok(())
    } else {
        Err(format!(
            "top edge\nexpected: {expected:?}\nactual:   {actual:?}"
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tabs_fit_is_judged_on_the_full_title_width() {
        // Width 8 title, 7 cells of room: the cut title (6 cells) would leave room for a 1-cell
        // tab, but the host judged the full title, so the tab is dropped.
        assert_eq!(expected_text(9, "ああああ", "t"), "┌あああ─┐");
        // The same title with room for everything keeps the tab.
        assert_eq!(expected_text(12, "ああああ", "t"), "┌ああああ─t┐");
        // Tabs of nearly no width still need the full title to fit.
        assert_eq!(expected_text(10, "ああああ", "t"), "┌ああああ┐");
        assert_eq!(expected_text(11, "ああああ", "t"), "┌ああああt┐");
        assert_eq!(expected_text(9, "ああああ", ""), "┌あああ─┐");
    }
}
