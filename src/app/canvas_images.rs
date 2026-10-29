//! The pictures a plugin places on its canvas (`CanvasView::images`), on the host side of konoma.
//!
//! What is decoded, how a picture is keyed and what bounds the work is host-neutral and lives in
//! [`konoma_plugin::canvas_pictures`] (the identity of a picture, `image: None`, the kept
//! pictures, failure being final for a content, the bounds, the untrusted SVG, files and
//! animation; see its module doc). This module is the part bound to [`App`]: the landing of
//! a decoded answer into the app's caches (the cache entries, the kitty image ids, the held-bytes
//! count, the prune and the once-per-reason flash of a scope refusal), the cell placements
//! `ui/canvas.rs` draws, and the re-exports of the shared items the rest of the app and its tests
//! use. What the landing does is described below.
//!
//! A canvas picture goes through the same machinery as a Markdown inline image and a media-diff
//! side: its pixels sit in `App::md_images` under a synthetic key, the encode worker turns
//! them into a protocol for the cell rectangle, and `ui/canvas.rs` places the result over the
//! cells.
//!
//! - **The cache key** is `canvas://<plugin id>/<image id>/<content hash>`
//!   ([`konoma_widgets::markdown::canvas_url`]). The things that belong to the picture rather than
//!   to its content are keyed by the slot, `canvas://<plugin id>/<image id>`
//!   ([`konoma_widgets::markdown::canvas_slot_url`]): the kitty image ids (so new content is
//!   transmitted over the same terminal image, never added next to it) and the overlay signature
//!   (so changing content never clears the screen).
//! - **No blink**: while the new content has no protocol yet the previous entry of the id keeps
//!   being drawn (`CanvasPictureHost::shown_key`); when the new one can be drawn it takes over. A frame
//!   only moves that pointer: the entries nobody names or draws from go in the prune when an
//!   answer lands (a job in flight holds a snapshot of the cache, and an entry dropped by a frame
//!   meanwhile would be named by its answer without being there). A new content that cannot be
//!   decoded ends the old picture (the cells show).
//! - **The worker decodes**, in the same job that ran the plugin's `view`
//!   ([`konoma_plugin::canvas_pictures::decode_canvas`], called from `PluginWork::run_inner` with
//!   the policy of this host: its read scope, its supervised-child SVG drawer
//!   (`SvgChild`) and the decode ticket's cancellation), never the UI thread.
//! - **SVG from a plugin is untrusted**: it is drawn by the supervised child process, except where
//!   the host believes the plugin's `trusted` claim (`plugins::draws_own_svg`).
//! - **Animation** uses the Markdown GIF machinery as it is: the frames are kept on the cache
//!   entry and `advance_md_gifs_if_due` steps them, re-encoding under the same kitty id.
//! - **No pictures without an encoder**: when the app has no image backend (a terminal that cannot
//!   show pictures at all, a headless run) nothing is decoded and the plugin's cells stay; with a
//!   picker that only has halfblocks the pictures are drawn as halfblocks, like Markdown images.
//! - **Landing**: when an answer lands its fresh pictures are filed in the cache, the kitty ids of
//!   ids the canvas stopped naming are released, and a scope refusal is flashed once per
//!   (plugin id, reason).

#[cfg(test)]
use std::collections::HashMap;
#[cfg(test)]
use std::sync::Arc;

use super::*;

// The table of the kitty ids on the terminal (and the key they are filed under) lives with the
// cache that allocates them, in the display crate.
#[cfg(test)]
pub(crate) use konoma_widgets::inline_images::kitty_key;
pub(crate) use konoma_widgets::inline_images::{LiveKittyIds, LIVE_CANVAS_KITTY};

/// Write to `out` the deletes of every id `table` holds, and flush; the table is emptied. Every
/// failure is dropped: the terminal may be gone (SIGHUP).
pub(crate) fn delete_kitty_images_in(table: &LiveKittyIds, out: &mut dyn std::io::Write) {
    table.delete_to(out);
}

/// Write to `out` the deletes of every canvas picture still on the terminal. What the ways out of
/// the program do (`preview::command::leave_for_good`), before the terminal is restored.
pub(crate) fn delete_canvas_kitty_images(out: &mut dyn std::io::Write) {
    delete_kitty_images_in(&LIVE_CANVAS_KITTY, out);
}

#[cfg(test)]
pub(super) use konoma_plugin::canvas_pictures::CanvasLimits;
#[cfg(test)]
pub(crate) use konoma_plugin::canvas_pictures::{
    content_hash, decode_one, header_pixels, mix_scope, seen_for_tests,
};
pub(super) use konoma_plugin::canvas_pictures::{
    decode_canvas, CanvasDecoded, CanvasHeld, CanvasPictureHost, CanvasPictures,
};
#[cfg(test)]
pub(super) use konoma_plugin::canvas_pictures::{decoded_bytes, CanvasPlace, KeptPicture};
#[cfg(test)]
pub(super) use konoma_plugin::view::{CellRect, ImageData};
#[cfg(test)]
pub(super) use std::time::Duration;

impl App {
    /// Tell the user, once per (plugin id, reason), that the read scope refused a file the plugin
    /// named. The reason names the plugin and the `read_dirs` key to change. Repeated redraws and
    /// further refused pictures of the same plugin do not flash again.
    pub(super) fn flash_scope_refusal(&mut self, plugin_id: &str) {
        use crate::preview::image::{ImageFailure, ImageFailureMessage};
        let why = ImageFailure::OutsideReadDirs;
        if !self
            .scope_flashed
            .insert((plugin_id.to_string(), why.code()))
        {
            return;
        }
        if let Some(t) = why.message(self.lang, plugin_id) {
            self.flash = Some(t);
        }
    }

    /// Flash the document pictures the read scope refused while the page was built.
    pub(super) fn flash_md_scope_refusals(&mut self) {
        let ids = std::mem::take(&mut *self.md_scope_refusals.borrow_mut());
        for id in ids {
            self.flash_scope_refusal(&id);
        }
    }

    /// Flash the read-scope refusals among the pictures a canvas answer decoded (see
    /// [`Self::flash_scope_refusal`]).
    pub(super) fn canvas_flash_refusals(&mut self, plugin_id: &str, pics: &CanvasPictures) {
        let refused = pics.fresh.iter().any(|(_, d)| {
            matches!(
                d,
                CanvasDecoded::Failed(crate::preview::image::ImageFailure::OutsideReadDirs)
            )
        });
        if refused {
            self.flash_scope_refusal(plugin_id);
        }
    }

    /// Whether canvas pictures can be shown at all: an image backend and the encode worker are
    /// attached. Without them no picture is decoded and the plugin's cells stay.
    pub(super) fn canvas_pictures_drawable(&self) -> bool {
        self.picker.is_some() && self.md_images.enc_tx.is_some()
    }

    /// What the cache holds of the canvas pictures (given to the next job so it decodes only
    /// what is new). `None` when pictures cannot be shown.
    pub(super) fn canvas_known_keys(&self) -> Option<CanvasHeld> {
        self.canvas_pictures_drawable()
            .then(|| self.canvas.known_keys(&self.md_images))
    }

    /// Put the pictures a landed canvas answer decoded into the cache
    /// ([`CanvasPictureHost::landed`]). Returns whether another answer should be asked for.
    pub(super) fn canvas_pictures_landed(&mut self, pics: CanvasPictures) -> bool {
        let budget = self.md_cache_budget();
        let landed = self.canvas.landed(&mut self.md_images, pics, budget);
        self.canvas_landing_done(landed)
    }

    /// What the app does with the outcome of a landing: a picture filed as failed changes what the
    /// page reserves for it, so the decoration is rebuilt.
    fn canvas_landing_done(&mut self, landed: konoma_plugin::canvas_pictures::Landed) -> bool {
        if landed.redecorate {
            self.md_cache = None;
        }
        landed.ask_again
    }

    /// Forget every canvas picture (the show went, or its view is not a canvas any more): the
    /// entries, the placements, and every kitty image the canvas pinned (deleted in the terminal).
    pub(super) fn forget_canvas_pictures(&mut self) {
        self.canvas.forget(&mut self.md_images);
    }

    /// The escapes that delete from a kitty-graphics terminal the images of the canvas pictures
    /// that went (to be written to the terminal after the frame). Empty on any other terminal:
    /// no id is ever pinned there.
    pub fn take_kitty_deletes(&mut self) -> Vec<String> {
        self.md_images.take_kitty_deletes()
    }

    /// Test-only: the bytes of decoded pixels the canvas entries of the cache hold.
    #[cfg(test)]
    pub(super) fn canvas_held_bytes(&self) -> u64 {
        CanvasPictureHost::held_bytes(&self.md_images)
    }

    /// Test-only: how many placements the canvas on screen has.
    #[cfg(test)]
    pub(crate) fn canvas_place_count(&self) -> usize {
        self.canvas.place_count()
    }

    /// Test-only: placement `n` of the canvas on screen.
    #[cfg(test)]
    pub(crate) fn canvas_place(&self, n: usize) -> Option<CanvasPlace> {
        self.canvas.place(n)
    }

    /// Draw the pictures of the canvas on screen into `buf` inside `inner`
    /// ([`konoma_plugin::canvas_pictures::draw_images`]); returns the overlay signature when a
    /// picture was drawn.
    pub(crate) fn draw_canvas_images(
        &mut self,
        buf: &mut ratatui::buffer::Buffer,
        inner: ratatui::layout::Rect,
    ) -> Option<u64> {
        konoma_plugin::canvas_pictures::draw_images(
            buf,
            inner,
            &mut self.canvas,
            &mut self.md_images,
        )
    }

    /// Test-only: the wanted cache key and rectangle of each picture of the canvas on screen,
    /// in drawing order. Empty when no canvas is on screen.
    #[cfg(test)]
    pub(crate) fn canvas_image_placements(&self) -> Vec<(String, CellRect)> {
        if self.canvas_view().is_none() {
            return Vec::new();
        }
        self.canvas
            .places
            .iter()
            .filter_map(|p| Some((p.key.as_deref()?.to_string(), p.rect)))
            .collect()
    }

    /// Test-only: pretend every animating entry has shown its current frame for a long time, so
    /// the next `advance_md_gifs_if_due` steps it (no waiting for a frame's display time).
    #[cfg(test)]
    pub(crate) fn age_md_gif_frames_for_test(&mut self) {
        let long_ago = std::time::Instant::now()
            .checked_sub(Duration::from_secs(3600))
            .unwrap_or_else(std::time::Instant::now);
        for e in self.md_images.cache.values_mut() {
            if e.frames.len() >= 2 && e.shown_at.is_some() {
                e.shown_at = Some(long_ago);
            }
        }
    }

    /// Test-only: the inline-image overlay pass number, and the pass that last asked for `key`.
    #[cfg(test)]
    pub(crate) fn md_frame_and_last_used_for_test(&self, key: &Path) -> (u64, Option<u64>) {
        (
            self.md_images.frame,
            self.md_images.cache.get(key).map(|e| e.last_used),
        )
    }

    /// Test-only: the identity of the read scope the held canvas pictures were made under.
    #[cfg(test)]
    pub(crate) fn canvas_scope_fp_for_test(&self) -> u64 {
        self.canvas.scope_fp
    }

    /// Test-only: what the next job would be told about the cache (as `canvas_known_keys`, which
    /// needs an image backend).
    #[cfg(test)]
    pub(super) fn canvas_known_keys_for_test(&self) -> CanvasHeld {
        self.canvas.known_keys(&self.md_images)
    }

    /// Test-only: the kitty image ids pinned for a cache key, per slot family.
    #[cfg(test)]
    pub(crate) fn md_kitty_ids_for_test(&self, key: &Path) -> Option<[Vec<u32>; 3]> {
        self.md_images.kitty_ids.get(&kitty_key(key)).cloned()
    }
}

#[cfg(test)]
#[path = "canvas_images_tests.rs"]
mod tests;
