//! Drawing the pictures of a canvas over the cells already painted: shared by konoma and the
//! standalone plugin app, so both show a canvas picture the same way.
//!
//! - A picture is placed at its cell rectangle inside the content area. One that starts outside
//!   the area is skipped; one that reaches below the bottom edge is clipped to the visible band;
//!   one that reaches past the right edge is skipped whole (the machinery clips bands of rows
//!   only; the cells show, and the next frame after the plugin was asked for the area's size has a
//!   canvas that fits).
//! - While a picture is not decoded or encoded yet, or cannot be (damaged, too large), the
//!   plugin's cells under it stay. Where it is drawn, the cells under it lose their text styles
//!   (a kitty placeholder wants a clean state) and keep their background.
//! - The rectangles drawn this frame (with the picture ids, never the content) are returned as the
//!   overlay signature, so the host's one-time clear (placeholder debris) also covers a picture
//!   that moved or vanished, and a picture whose pixels change in place does not clear.

use std::path::Path;
use std::sync::Arc;

use konoma_widgets::inline_images::InlineImageCache;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::overlay::{visible_rect, OverlaySig, MAX_PICTURE_AREA_FACTOR};
use super::CanvasPictureHost;

/// Ask the cache for the encode of canvas picture `key` at the given size (cheap when settled).
/// Decoding is already done (the landing filed the pixels); the encode is the worker's. The entry
/// is stamped as drawn by the current pass, which keeps its encodes from being recycled, and its
/// encode slots are capped at the number of placements the picture has.
fn ensure_picture(
    host: &CanvasPictureHost,
    cache: &mut InlineImageCache,
    key: &str,
    cols: u16,
    rows: u16,
    vis_rows: u16,
) {
    let path = Path::new(key);
    if !cache.stamp_drawn(path) {
        return; // no entry: a canvas picture is never decoded here (the landing files it)
    }
    cache.ensure_encode(path, cols, rows, 0, vis_rows, Some(host.slot_cap(key)));
}

/// Draw the pictures of the canvas into `buf` inside `inner`. Returns the overlay signature when
/// at least one picture was drawn; a frame that drew none records nothing, so the host's finish
/// side clears once when pictures were drawn just before ("they left"), and not again.
pub fn draw_images(
    buf: &mut Buffer,
    inner: Rect,
    host: &mut CanvasPictureHost,
    cache: &mut InlineImageCache,
) -> Option<u64> {
    let count = host.place_count();
    if count == 0 {
        return None;
    }
    // One pass = one frame for the slot bookkeeping: every encode request below stamps this
    // number, which keeps the encodes of the pictures on screen from being recycled.
    cache.begin_pass();
    let mut sig = OverlaySig::new(inner);
    let mut drawn = 0usize;
    // The total area the pictures may be encoded at: each placement is its own encoding (a
    // protocol payload the size of its cells), and the plugin may stack many ids over the whole
    // area. Placements past the budget, in the plugin's order, keep their cells.
    let mut area_left = u64::from(inner.width) * u64::from(inner.height) * MAX_PICTURE_AREA_FACTOR;
    for n in 0..count {
        // The placement and its key were made when the answer landed: no string is built here.
        let Some(place) = host.place(n) else {
            continue;
        };
        let Some(wanted) = place.key else {
            continue; // `unchanged` for a picture the host does not hold: the cells stay
        };
        let r = place.rect;
        let Some(target) = visible_rect(inner, r) else {
            continue;
        };
        let area = u64::from(target.width) * u64::from(target.height);
        if area > area_left {
            continue; // over the budget: the cells stay
        }
        area_left -= area;
        ensure_picture(host, cache, &wanted, r.cols, r.rows, target.height);
        // The content the plugin named once it can be drawn; until then the one drawn before (so
        // a picture whose content changes does not blink).
        let shown = host.shown_key(cache, place.id, &wanted, r.cols, r.rows, target.height);
        if !Arc::ptr_eq(&shown, &wanted) {
            ensure_picture(host, cache, &shown, r.cols, r.rows, target.height);
        }
        let Some(img) = cache.proto(Path::new(&*shown), r.cols, r.rows, 0, target.height) else {
            continue; // not encoded yet, or failed: the plugin's cells stay
        };
        // A kitty placeholder assumes a clean SGR state: reset the attributes and the foreground
        // of the cells under the picture, keeping the background (a transparent picture is
        // composited over it).
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
    (drawn > 0).then(|| sig.finish())
}
