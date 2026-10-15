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
//! - The rectangles drawn this frame are reported as the overlay signature, so the run loop's
//!   one-time clear (placeholder debris) also covers a canvas picture that moved or vanished.
//!
//! Text input and the redraw timer are the app's (`App::canvas_view` tells what the view asks
//! for).

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::widgets::Block;
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

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
pub(crate) fn paint(buf: &mut Buffer, inner: Rect, canvas: &CanvasView) {
    let cols = canvas.cols as usize;
    if cols == 0 {
        return;
    }
    let show_cols = (canvas.cols).min(inner.width);
    let show_rows = (canvas.rows).min(inner.height);
    for y in 0..show_rows {
        for x in 0..show_cols {
            let Some(cell) = canvas.cells.get(y as usize * cols + x as usize) else {
                return;
            };
            if cell.is_continuation() {
                continue;
            }
            // A wide character whose right half is outside the area is not drawn.
            let width = cell.symbol.width().max(1) as u16;
            if x + width > show_cols && width > 1 {
                continue;
            }
            let Some(target) = buf.cell_mut((inner.x + x, inner.y + y)) else {
                continue;
            };
            target
                .set_symbol(&cell.symbol)
                .set_style(plugin_style_of(cell.style));
        }
    }
}

/// Draw the pictures of the canvas over the cells already painted (see the module comment).
fn draw_images(frame: &mut Frame, app: &mut App, inner: Rect) {
    let placements = app.canvas_image_placements();
    if placements.is_empty() {
        return;
    }
    // One pass = one frame for the slot bookkeeping: every `ensure_md_image` below stamps this
    // number, which keeps the encodes of the pictures on screen from being recycled.
    app.begin_md_image_frame();
    use std::hash::{Hash, Hasher};
    let mut sig = std::collections::hash_map::DefaultHasher::new();
    (inner.x, inner.y, inner.width, inner.height).hash(&mut sig);
    let mut drawn = 0usize;
    for (key, r) in placements {
        let target = match visible_rect(inner, r) {
            Some(t) => t,
            None => continue,
        };
        // Asked every frame (cheap when settled): decoding is already done, the encode is the
        // worker's.
        app.ensure_md_image(&key, r.cols, r.rows, 0, target.height);
        let Some(img) = app.md_image_proto(&key, r.cols, r.rows, 0, target.height) else {
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
        (
            key.as_str(),
            target.x,
            target.y,
            target.width,
            target.height,
        )
            .hash(&mut sig);
        drawn += 1;
    }
    // A frame that drew no picture records nothing: the finish side then clears once when
    // pictures were drawn just before ("they left"), and not again.
    if drawn > 0 {
        app.note_md_overlay(sig.finish());
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
