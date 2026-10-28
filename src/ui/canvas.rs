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
//! The picture placements (`CanvasView::images`) are drawn over the cells by [`draw_images`], which is
//! `konoma_plugin::canvas_pictures::draw_images` over the inline-image cache Markdown images use
//! (the pictures are decoded on the worker, `app/canvas_images.rs`, the decoding in
//! `konoma_plugin::canvas_pictures`):
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
//!   drawn (`CanvasPictureHost::shown_key`).
//!
//! Text input and the redraw timer are the app's (`App::canvas_view` tells what the view asks
//! for).

use ratatui::layout::Rect;
use ratatui::widgets::Block;
use ratatui::Frame;

use crate::app::App;

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
        konoma_plugin::ratatui_draw::paint_canvas(frame.buffer_mut(), inner, canvas);
    }
    draw_images(frame, app, inner);

    // The area is not the size of the request that produced this canvas (a resize, or the first
    // frame that measured it): ask again. After the drawing, so the borrow of the view is over.
    app.refit_canvas();
}

/// Draw the pictures of the canvas over the cells already painted (see the module comment).
fn draw_images(frame: &mut Frame, app: &mut App, inner: Rect) {
    if let Some(sig) = app.draw_canvas_images(frame.buffer_mut(), inner) {
        app.note_md_overlay(sig);
    }
}
