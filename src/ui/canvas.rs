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
//! Seams for the next steps of stage 2.5 (not drawn yet): the picture placements
//! (`CanvasView::images`) are drawn over the cells at the marked place in [`render`]; text input
//! and the redraw timer are the app's (`App::canvas_view` tells what the view asks for).

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
        // Seam (T2b): the picture placements `canvas.images` are drawn here, over the cells.
    }

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
