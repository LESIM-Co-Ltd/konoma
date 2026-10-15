//! A plugin's free-drawing view (`View::Canvas`): the cells the plugin drew, copied into the frame.
//!
//! The plugin returns plain data (`CanvasView`: a grid of symbols and styles), so drawing it is a
//! copy: each cell goes to the same place inside the frame's content area, with the plugin's
//! style mapped onto ratatui's. The host does not interpret the cells or convert their colors (a
//! plugin reads `TermCaps::colors` and picks colors the terminal shows).
//!
//! - A canvas larger than the area is clipped at the right and bottom edges; a wide character
//!   whose right half would fall outside is left blank (never half a character).
//! - A canvas smaller than the area leaves the rest of the area untouched: the terminal's own
//!   background.
//! - A continuation cell (the empty right half of a wide character) is skipped: the wide
//!   character in the cell to its left covers both columns.
//!
//! The picture placements (`CanvasView::images`) are drawn over the cells by [`draw_images`], with
//! the machinery Markdown inline images use (`ensure_md_image`, `md_image_proto`; the pictures are
//! decoded on the worker, `app/canvas_images.rs`):
//!
//! - A picture is placed at its cell rectangle inside the content area. One that starts outside
//!   the area is skipped; one that reaches below the bottom edge is clipped to the visible band
//!   (as Markdown images are at the viewport edge); one that reaches past the right edge is
//!   skipped whole (the machinery clips bands of rows only; the cells show, and the next frame
//!   after the plugin was asked for the area's size has a canvas that fits).
//! - While a picture is not decoded or encoded yet, or cannot be (damaged, too large), the
//!   plugin's cells under it stay. Where it is drawn, the cells under it lose their text styles
//!   (a kitty placeholder wants a clean state) and keep their background.
//! - On a terminal with no image backend nothing is decoded and the cells stay; with a
//!   halfblocks-only backend the pictures are drawn as halfblocks, as Markdown images are.
//! - The rectangles drawn this frame (with the picture ids, never the content) are reported as the
//!   overlay signature, so the run loop's one-time clear (placeholder debris) also covers a canvas
//!   picture that moved or vanished, and a picture whose pixels change in place does not clear.
//! - A picture whose content changed keeps drawing its previous entry until the new one can be
//!   drawn (`App::canvas_shown_key`).
//!
//! Text input and the redraw timer are the app's (`App::canvas_view` tells what the view asks
//! for).

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::widgets::Block;
use ratatui::Frame;
use std::sync::Arc;

use konoma_plugin::view::CanvasView;

use crate::app::{plugin_style_of, App};

/// Draw the canvas of the plugin show into `area` (the preview frame and its content).
pub fn render(frame: &mut Frame, app: &mut App, area: Rect) {
    let title = app
        .tab
        .preview_path
        .clone()
        .map(|p| format!(" {} ", app.format_path(&p)))
        .unwrap_or_else(|| " preview ".to_string());
    let block = Block::bordered().title(title);
    let inner = block.inner(area);
    // The size the plugin is asked for (`App::plugin_view_req`) and the page step.
    app.tab.preview_viewport = inner.height;
    app.tab.preview_cols = inner.width;
    app.tab.preview_viewport_drawn = true;
    frame.render_widget(block, area);

    if let Some(canvas) = app.canvas_view() {
        paint(frame.buffer_mut(), inner, canvas);
    }
    draw_images(frame, app, inner);

    // The area is not the size of the request that produced this canvas (a resize, or the first
    // frame that measured it): ask again. After the drawing, so the borrow of the view is over.
    app.refit_canvas();
}

/// Copy the cells of `canvas` into `buf` inside `inner`, clipped to it.
///
/// `CanvasView::check` is the contract and the host refuses a canvas that fails it; this is the
/// second line: whatever the cells hold, a control character is never written to the buffer (it
/// would reach the terminal as an escape sequence), it becomes a space. All positions are
/// computed in `u32`.
pub(crate) fn paint(buf: &mut Buffer, inner: Rect, canvas: &CanvasView) {
    let cols = canvas.cols as usize;
    if cols == 0 {
        return;
    }
    let show_cols = (canvas.cols).min(inner.width) as u32;
    let show_rows = (canvas.rows).min(inner.height) as u32;
    for y in 0..show_rows {
        for x in 0..show_cols {
            let Some(cell) = canvas.cells.get(y as usize * cols + x as usize) else {
                return;
            };
            if cell.is_continuation() {
                continue;
            }
            // A wide character whose right half is outside the area is not drawn.
            let width = konoma_plugin::symbol_width(&cell.symbol).max(1) as u32;
            if x + width > show_cols && width > 1 {
                continue;
            }
            let (Ok(tx), Ok(ty)) = (
                u16::try_from(inner.x as u32 + x),
                u16::try_from(inner.y as u32 + y),
            ) else {
                continue;
            };
            let Some(target) = buf.cell_mut((tx, ty)) else {
                continue;
            };
            let style = plugin_style_of(cell.style);
            if cell.symbol.chars().any(char::is_control) {
                target.set_symbol(" ").set_style(style);
            } else {
                target.set_symbol(&cell.symbol).set_style(style);
            }
        }
    }
}

/// Draw the pictures of the canvas over the cells already painted (see the module comment).
fn draw_images(frame: &mut Frame, app: &mut App, inner: Rect) {
    let count = app.canvas_place_count();
    if count == 0 {
        return;
    }
    // One pass = one frame for the slot bookkeeping: every `ensure_md_image` below stamps this
    // number, which keeps the encodes of the pictures on screen from being recycled.
    app.begin_md_image_frame();
    let mut sig = OverlaySig::new(inner);
    let mut drawn = 0usize;
    for n in 0..count {
        // The placement and its key were made when the answer landed: no string is built here.
        let Some(place) = app.canvas_place(n) else {
            continue;
        };
        let Some(wanted) = place.key else {
            continue; // `unchanged` for a picture the host does not hold: the cells stay
        };
        let r = place.rect;
        let target = match visible_rect(inner, r) {
            Some(t) => t,
            None => continue,
        };
        // Asked every frame (cheap when settled): decoding is already done, the encode is the
        // worker's.
        app.ensure_md_image(&wanted, r.cols, r.rows, 0, target.height);
        // The content the plugin named once it can be drawn; until then the one drawn before (so
        // a picture whose content changes does not blink).
        let shown = app.canvas_shown_key(place.id, &wanted, r.cols, r.rows, target.height);
        if !Arc::ptr_eq(&shown, &wanted) {
            app.ensure_md_image(&shown, r.cols, r.rows, 0, target.height);
        }
        let Some(img) = app.md_image_proto(&shown, r.cols, r.rows, 0, target.height) else {
            continue; // not encoded yet, or failed: the plugin's cells stay
        };
        // A kitty placeholder assumes a clean SGR state: reset the attributes and the foreground
        // of the cells under the picture, keeping the background (a transparent picture is
        // composited over it).
        let buf = frame.buffer_mut();
        for y in target.top()..target.bottom() {
            for x in target.left()..target.right() {
                if let Some(cell) = buf.cell_mut((x, y)) {
                    let bg = cell.style().bg;
                    let mut s = ratatui::style::Style::reset();
                    if let Some(bg) = bg {
                        s = s.bg(bg);
                    }
                    cell.set_style(s);
                }
            }
        }
        img.render(target, buf);
        // The signature follows what is where, never the content: a picture whose pixels change
        // under the same id must not look like a picture that moved (that clears the screen).
        sig.add(place.id, target);
        drawn += 1;
    }
    // A frame that drew no picture records nothing: the finish side then clears once when
    // pictures were drawn just before ("they left"), and not again.
    if drawn > 0 {
        app.note_md_overlay(sig.finish());
    }
}

/// The signature of what the canvas overlay drew in one frame: the area, and for each picture its
/// id and where it was drawn. The run loop clears the screen once when it changes (debris of a
/// picture that moved or left), so it follows **what is where**, never the content of a picture.
pub(crate) struct OverlaySig(std::collections::hash_map::DefaultHasher);

impl OverlaySig {
    pub(crate) fn new(inner: Rect) -> OverlaySig {
        use std::hash::Hash;
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (inner.x, inner.y, inner.width, inner.height).hash(&mut h);
        OverlaySig(h)
    }

    pub(crate) fn add(&mut self, id: u64, at: Rect) {
        use std::hash::Hash;
        (id, at.x, at.y, at.width, at.height).hash(&mut self.0);
    }

    pub(crate) fn finish(&self) -> u64 {
        use std::hash::Hasher;
        self.0.finish()
    }
}

/// Where the picture `r` (in canvas cells) lands inside `inner`, or `None` when none of it is
/// shown: it starts outside, or reaches past the right edge.
pub(crate) fn visible_rect(inner: Rect, r: konoma_plugin::view::CellRect) -> Option<Rect> {
    if r.cols == 0 || r.rows == 0 || r.x >= inner.width || r.y >= inner.height {
        return None;
    }
    if r.x as u32 + r.cols as u32 > inner.width as u32 {
        return None;
    }
    let rows = r.rows.min(inner.height - r.y);
    Some(Rect {
        x: inner.x + r.x,
        y: inner.y + r.y,
        width: r.cols,
        height: rows,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use konoma_plugin::view::CellRect;

    fn canvas_of(cols: u16, symbols: &[&str]) -> CanvasView {
        let mut c = CanvasView::new(cols, 1);
        for (i, s) in symbols.iter().enumerate() {
            c.cells[i].symbol = s.to_string();
        }
        c
    }

    fn painted(canvas: &CanvasView, inner: Rect) -> Buffer {
        let mut buf = Buffer::empty(Rect::new(0, 0, 12, 4));
        paint(&mut buf, inner, canvas);
        buf
    }

    #[test]
    fn paint_never_writes_a_control_character_even_if_check_was_bypassed() {
        // None of these would pass `CanvasView::check`; the host paints them anyway as spaces.
        let bad = [
            "\u{1b}[31m",
            "\u{1b}",
            "\u{7}",
            "\n",
            "\r",
            "\u{7f}",
            "\u{9b}",
            "a\u{1b}",
            "\u{90}",
        ];
        let canvas = canvas_of(bad.len() as u16, &bad);
        assert!(canvas.check().is_err());
        let buf = painted(&canvas, Rect::new(1, 1, 10, 2));
        for cell in &buf.content {
            assert!(
                !cell.symbol().chars().any(char::is_control),
                "{:?} reached the buffer",
                cell.symbol()
            );
        }
        for i in 0..bad.len() as u16 {
            assert_eq!(buf[(1 + i, 1)].symbol(), " ", "cell {i}");
        }
    }

    #[test]
    fn paint_still_draws_ordinary_symbols_and_styles() {
        let mut canvas = canvas_of(3, &["a", "日", ""]);
        canvas.cells[0].style = konoma_plugin::Style::new().fg(konoma_plugin::Color::Red);
        let buf = painted(&canvas, Rect::new(2, 1, 8, 2));
        assert_eq!(buf[(2, 1)].symbol(), "a");
        assert_eq!(buf[(2, 1)].fg, ratatui::style::Color::Red);
        assert_eq!(buf[(3, 1)].symbol(), "日");
    }

    #[test]
    fn paint_leaves_the_trailing_cell_of_a_wide_character_alone() {
        // The right half of a wide character belongs to the left cell: its own style (the plugin
        // may have set one) is not painted over the terminal's two-column cell.
        let mut canvas = canvas_of(3, &["日", "", "a"]);
        canvas.cells[1].style = konoma_plugin::Style::new().bg(konoma_plugin::Color::Red);
        let buf = painted(&canvas, Rect::new(0, 0, 3, 1));
        assert_eq!(buf[(0, 0)].symbol(), "日");
        assert_eq!(buf[(1, 0)].symbol(), " ");
        assert_eq!(buf[(1, 0)].bg, ratatui::style::Color::Reset);
        assert_eq!(buf[(2, 0)].symbol(), "a");
    }

    #[test]
    fn paint_counts_a_voiced_halfwidth_kana_as_two_columns_like_ratatui() {
        // "ｶﾞ" is two columns for ratatui (the voiced mark has a column of its own), so at the right
        // edge of the area it is cut like any wide character instead of being drawn half over.
        let canvas = canvas_of(3, &["a", "ｶﾞ", ""]);
        assert_eq!(canvas.check(), Ok(()));
        let buf = painted(&canvas, Rect::new(0, 0, 2, 1));
        assert_eq!(buf[(0, 0)].symbol(), "a");
        assert_eq!(buf[(1, 0)].symbol(), " ");
        let buf = painted(&canvas, Rect::new(0, 0, 3, 1));
        assert_eq!(buf[(1, 0)].symbol(), "ｶﾞ");
    }

    #[test]
    fn paint_stops_at_the_cells_it_has_for_a_huge_claimed_canvas() {
        // A canvas that claims 65535 columns but holds 3 cells: positions are summed in u32,
        // nothing wraps or panics, and painting stops where the cells end.
        let mut canvas = CanvasView::new(0, 0);
        canvas.cols = u16::MAX;
        canvas.rows = 1;
        canvas.cells = vec![konoma_plugin::view::CanvasCell::new("日", Default::default()); 3];
        let inner = Rect::new(0, 0, u16::MAX, 1);
        let mut buf = Buffer::empty(Rect::new(0, 0, 4, 1));
        paint(&mut buf, inner, &canvas);
        assert_eq!(buf[(0, 0)].symbol(), "日");
    }

    fn sig(inner: Rect, items: &[(u64, Rect)]) -> u64 {
        let mut s = OverlaySig::new(inner);
        for (id, r) in items {
            s.add(*id, *r);
        }
        s.finish()
    }

    #[test]
    fn the_overlay_signature_follows_every_input() {
        let area = Rect::new(1, 2, 30, 10);
        let at = Rect::new(5, 3, 8, 4);
        let base = sig(area, &[(1, at)]);
        assert_eq!(base, sig(area, &[(1, at)]), "stable");
        assert_ne!(base, sig(Rect::new(2, 2, 30, 10), &[(1, at)]), "area x");
        assert_ne!(base, sig(Rect::new(1, 3, 30, 10), &[(1, at)]), "area y");
        assert_ne!(base, sig(Rect::new(1, 2, 31, 10), &[(1, at)]), "area width");
        assert_ne!(
            base,
            sig(Rect::new(1, 2, 30, 11), &[(1, at)]),
            "area height"
        );
        assert_ne!(
            base,
            sig(area, &[(2, at)]),
            "another picture in the same place"
        );
        assert_ne!(base, sig(area, &[(1, Rect::new(6, 3, 8, 4))]), "x");
        assert_ne!(base, sig(area, &[(1, Rect::new(5, 4, 8, 4))]), "y");
        assert_ne!(base, sig(area, &[(1, Rect::new(5, 3, 9, 4))]), "width");
        assert_ne!(base, sig(area, &[(1, Rect::new(5, 3, 8, 3))]), "height");
        assert_ne!(base, sig(area, &[]), "gone");
        assert_ne!(base, sig(area, &[(1, at), (2, at)]), "another one");
    }

    const INNER: Rect = Rect {
        x: 3,
        y: 2,
        width: 20,
        height: 10,
    };

    #[test]
    fn visible_rect_offsets_into_the_area() {
        assert_eq!(
            visible_rect(INNER, CellRect::new(4, 1, 5, 3)),
            Some(Rect::new(7, 3, 5, 3))
        );
    }

    #[test]
    fn visible_rect_clips_the_bottom_to_a_band_and_keeps_the_top() {
        // Starts at row 8 of 10, 4 rows tall: the top 2 rows are shown.
        assert_eq!(
            visible_rect(INNER, CellRect::new(0, 8, 5, 4)),
            Some(Rect::new(3, 10, 5, 2))
        );
        // Ends exactly at the edge: whole.
        assert_eq!(
            visible_rect(INNER, CellRect::new(0, 6, 5, 4)),
            Some(Rect::new(3, 8, 5, 4))
        );
    }

    #[test]
    fn visible_rect_skips_what_starts_outside_or_does_not_fit_across() {
        assert_eq!(
            visible_rect(INNER, CellRect::new(20, 0, 2, 2)),
            None,
            "starts at the right edge"
        );
        assert_eq!(
            visible_rect(INNER, CellRect::new(0, 10, 2, 2)),
            None,
            "starts at the bottom edge"
        );
        assert_eq!(
            visible_rect(INNER, CellRect::new(19, 0, 2, 2)),
            None,
            "reaches past the right edge"
        );
        assert_eq!(
            visible_rect(INNER, CellRect::new(0, 0, 0, 2)),
            None,
            "no columns"
        );
        assert_eq!(
            visible_rect(INNER, CellRect::new(0, 0, 2, 0)),
            None,
            "no rows"
        );
        assert_eq!(
            visible_rect(INNER, CellRect::new(18, 0, 2, 1)),
            Some(Rect::new(21, 2, 2, 1)),
            "ends exactly at the right edge"
        );
        // No overflow of u16 arithmetic.
        assert_eq!(
            visible_rect(INNER, CellRect::new(u16::MAX, u16::MAX, u16::MAX, u16::MAX)),
            None
        );
        assert_eq!(visible_rect(INNER, CellRect::new(1, 0, u16::MAX, 1)), None);
    }
}
