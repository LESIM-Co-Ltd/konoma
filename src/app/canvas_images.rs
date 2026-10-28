//! The pictures a plugin places on its canvas (`CanvasView::images`), on the host side of konoma.
//!
//! What is decoded, how a picture is keyed and what bounds the work is host-neutral and lives in
//! [`konoma_plugin::canvas_pictures`] (the identity of a picture, `image: None`, the kept
//! pictures, failure being final for a content, the bounds, the untrusted SVG, files and
//! animation; see its module doc). This module is the part bound to [`App`]:
//!
//! A canvas picture goes through the same machinery as a Markdown inline image and a media-diff
//! side: its pixels sit in `App::md_image_cache` under a synthetic key, the encode worker turns
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
//!   being drawn (`App::canvas_shown_key`); when the new one can be drawn it takes over. A frame
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

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
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
    let escapes = table.take_delete_escapes();
    if escapes.is_empty() {
        return;
    }
    for e in escapes {
        let _ = out.write_all(e.as_bytes());
    }
    let _ = out.flush();
}

/// Write to `out` the deletes of every canvas picture still on the terminal. What the ways out of
/// the program do (`preview::command::leave_for_good`), before the terminal is restored.
pub(crate) fn delete_canvas_kitty_images(out: &mut dyn std::io::Write) {
    delete_kitty_images_in(&LIVE_CANVAS_KITTY, out);
}

pub(super) use konoma_plugin::canvas_pictures::{
    decode_canvas, decoded_bytes, source_bytes, CanvasDecoded, CanvasHeld, CanvasPictures,
    CanvasPlace, KeptPicture, MAX_CANVAS_HELD_BYTES, MAX_CANVAS_KEPT_BYTES,
};
// What the tests in this module's tree name through it.
pub(super) use konoma_plugin::canvas_pictures::files::{open_picture_file, open_regular_picture};
#[cfg(test)]
pub(crate) use konoma_plugin::canvas_pictures::{
    content_hash, decode_one, header_pixels, mix_scope, seen_for_tests,
};
#[cfg(test)]
pub(super) use konoma_plugin::canvas_pictures::{CanvasLimits, MAX_DECODES_PER_ANSWER};
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
        self.canvas_pictures_drawable().then(|| CanvasHeld {
            keys: self
                .md_images
                .cache
                .iter()
                .filter(|(k, _)| crate::preview::markdown::is_canvas_url(&k.to_string_lossy()))
                .map(|(k, e)| (k.clone(), e.pixel_bytes()))
                .collect(),
            by_id: self
                .canvas_places
                .iter()
                .filter_map(|p| p.key.clone().map(|k| (p.id, k)))
                .collect(),
            front: self.canvas_front.clone(),
            kept: self.canvas_kept.clone(),
        })
    }

    /// The bytes of decoded pixels the canvas entries of the cache hold.
    pub(super) fn canvas_held_bytes(&self) -> u64 {
        self.md_images
            .cache
            .iter()
            .filter(|(k, _)| crate::preview::markdown::is_canvas_url(&k.to_string_lossy()))
            .map(|(_, e)| e.pixel_bytes())
            .sum()
    }

    /// Put the pictures a landed canvas answer decoded into the cache. What the canvas no longer
    /// names goes first (so the memory bound is judged against what it keeps), then each new
    /// picture is added unless it would not fit; one refused for that is left out (not failed) and
    /// comes with a later answer. Returns whether another answer should be asked for
    /// ([`CanvasPictures::more`], or room grew for a picture that was refused).
    pub(super) fn canvas_pictures_landed(&mut self, pics: CanvasPictures) -> bool {
        self.canvas_pictures_landed_within(pics, MAX_CANVAS_HELD_BYTES)
    }

    /// [`Self::canvas_pictures_landed`] under the given memory bound.
    pub(super) fn canvas_pictures_landed_within(
        &mut self,
        pics: CanvasPictures,
        held_bytes: u64,
    ) -> bool {
        self.canvas_pictures_landed_limits(pics, held_bytes, MAX_CANVAS_KEPT_BYTES)
    }

    /// [`Self::canvas_pictures_landed`] under the given memory bound and bound on the kept
    /// sources.
    pub(super) fn canvas_pictures_landed_limits(
        &mut self,
        pics: CanvasPictures,
        held_bytes: u64,
        kept_source_bytes: u64,
    ) -> bool {
        self.canvas_scope_fp = pics.scope_fp;
        // The ids of the canvas, the entries it names and the entries its ids are drawn from.
        let ids: HashSet<u64> = pics.places.iter().map(|p| p.id).collect();
        self.canvas_front.retain(|id, _| ids.contains(id));
        let named: HashSet<PathBuf> = pics
            .places
            .iter()
            .filter_map(|p| p.key.as_deref().map(PathBuf::from))
            .collect();
        // A new content of an id the memory bound refused (the worker knew it did not fit) ends the
        // old picture of that id: its cells show rather than a stale picture.
        for (id, k) in &pics.sources {
            if k.needs.is_some() {
                self.canvas_front_gives_way(*id, k.key.as_deref());
            }
        }
        let mut live: HashSet<PathBuf> = named.clone();
        live.extend(self.canvas_front.values().map(|k| PathBuf::from(&**k)));
        for (key, _) in &pics.fresh {
            live.insert(key.clone());
        }
        self.prune_canvas_pictures(&live);
        let mut held = self.canvas_held_bytes();
        let mut added_any = false;
        // The decoded bytes of the pictures this landing refused (for the bound), by key.
        let mut refused_here: HashMap<PathBuf, u64> = HashMap::new();
        for (key, decoded) in pics.fresh {
            // Already held (a job that was out before the previous answer landed): keep the
            // entry, with its encodes and its kitty ids, as it is.
            if self.md_images.cache.contains_key(&key) {
                continue;
            }
            let bytes = decoded_bytes(&decoded);
            let (image, frames) = match decoded {
                CanvasDecoded::Still(img) => (Ok(img), None),
                CanvasDecoded::Frames(f) if !f.is_empty() => (Ok(f[0].0.clone()), Some(f)),
                CanvasDecoded::Frames(_) => (
                    Err(crate::preview::image::ImageFailure::UnsupportedFormat
                        .code()
                        .to_string()),
                    None,
                ),
                CanvasDecoded::Failed(why) => {
                    if why == crate::preview::image::ImageFailure::Cancelled {
                        continue;
                    }
                    (Err(why.code().to_string()), None)
                }
            };
            let naming: Vec<u64> = pics
                .places
                .iter()
                .filter(|p| p.key.as_deref().is_some_and(|k| Path::new(k) == key))
                .map(|p| p.id)
                .collect();
            if held + bytes > held_bytes {
                // The entries these ids are drawn from are the ones their new content replaces:
                // free them for the room (the tight case is the only one where a picture may
                // blink), and look again.
                for id in &naming {
                    self.canvas_front_gives_way(*id, Some(&key.to_string_lossy()));
                }
                self.prune_canvas_to_named(&named);
                held = self.canvas_held_bytes();
            }
            if held + bytes > held_bytes {
                // Refused for now, and not remembered as failed: it is decoded again when there
                // is room (the host keeps its source, `canvas_kept`). Its ids show their cells.
                refused_here.insert(key, bytes);
                continue;
            }
            held += bytes;
            added_any = true;
            // Pre-placed, as every synthetic key is: `apply_md_image` drops a result whose entry
            // is not there.
            self.md_images.cache.entry(key.clone()).or_default();
            let request = self.begin_md_request(&key);
            self.apply_md_image(MdImageResult {
                path: key,
                image,
                svg: None,
                reraster: false,
                frames,
                request,
            });
        }
        // What is still not in the cache (deferred by the work limits or refused for the bound) is
        // kept by its source, per id, so a later answer that says `unchanged` still gets it
        // decoded. Replaced as a whole: an id the canvas no longer names, or a picture that landed,
        // is released.
        let unhashed_pending = pics.sources.iter().any(|(_, k)| k.key.is_none());
        let before = std::mem::take(&mut self.canvas_kept);
        let mut kept: Vec<(u64, KeptPicture)> = pics
            .sources
            .into_iter()
            .filter(|(_, k)| {
                k.key
                    .as_deref()
                    .is_none_or(|key| !self.md_images.cache.contains_key(Path::new(key)))
            })
            .map(|(id, mut k)| {
                if let Some(n) = k
                    .key
                    .as_deref()
                    .and_then(|key| refused_here.get(Path::new(key)))
                {
                    k.needs = Some(*n);
                }
                (id, k)
            })
            .collect();
        // The kept sources are bounded too: past `kept_source_bytes` of source data the oldest
        // (the ones kept longest, then the first deferred) are dropped, and their ids show their
        // cells until the plugin sends the picture again.
        kept.sort_by_key(|(id, _)| !before.contains_key(id));
        let mut total: u64 = kept.iter().map(|(_, k)| source_bytes(&k.data)).sum();
        let mut dropped = 0;
        while total > kept_source_bytes && dropped < kept.len() {
            total -= source_bytes(&kept[dropped].1.data);
            dropped += 1;
        }
        let rest = kept.split_off(dropped);
        self.canvas_kept = rest.into_iter().collect();
        // What goes (the table of the answer before, the sources dropped for the bound) may be
        // the last holder of a large animation: it is freed off the UI thread.
        super::table_actions::discard_in_background((before, kept));
        self.prune_canvas_to_named(&named);
        self.set_canvas_places(pics.places);
        self.sweep_canvas_kitty_ids();
        // Ask again for what the work limits left (`more`), and for a refused picture that fits now
        // that room grew (an entry went): the next answer decodes it from the kept source.
        let room = held_bytes.saturating_sub(self.canvas_held_bytes());
        let room_for_refused = self
            .canvas_kept
            .values()
            .any(|k| k.needs.is_some_and(|n| n <= room));
        (pics.more && (added_any || unhashed_pending)) || room_for_refused
    }

    /// The entry the canvas id `id` is drawn from gives way to its new content `new` (which is
    /// not there to be drawn): the pointer goes, so the cells show and the entry is dropped by the
    /// next [`Self::prune_canvas_to_named`] unless something else names or draws from it. Nothing
    /// happens when `new` is the entry itself.
    fn canvas_front_gives_way(&mut self, id: u64, new: Option<&str>) {
        if self
            .canvas_front
            .get(&id)
            .is_some_and(|front| new != Some(&**front))
        {
            self.canvas_front.remove(&id);
        }
    }

    /// Drop the canvas entries that neither `named` (the keys the canvas names) nor a front
    /// pointer keeps.
    fn prune_canvas_to_named(&mut self, named: &HashSet<PathBuf>) {
        let mut live = named.clone();
        live.extend(self.canvas_front.values().map(|k| PathBuf::from(&**k)));
        self.prune_canvas_pictures(&live);
    }

    /// Drop the cache entries of the canvas pictures not in `live` (and the drawn-from pointer of
    /// an id that points at one).
    pub(super) fn prune_canvas_pictures(&mut self, live: &HashSet<PathBuf>) {
        self.md_images.cache.retain(|k, _| {
            !crate::preview::markdown::is_canvas_url(&k.to_string_lossy()) || live.contains(k)
        });
        let cache = &self.md_images.cache;
        self.canvas_front
            .retain(|_, k| cache.contains_key(Path::new(&**k)));
    }

    /// Replaces the placements of the canvas on screen and counts the placements of each picture.
    fn set_canvas_places(&mut self, places: Vec<CanvasPlace>) {
        let mut counts = std::collections::HashMap::<String, usize>::new();
        for p in &places {
            if let Some(slot) = p
                .key
                .as_deref()
                .and_then(crate::preview::markdown::canvas_slot_of)
            {
                *counts.entry(slot.to_string()).or_default() += 1;
            }
        }
        self.canvas_places = places;
        self.canvas_slot_counts = counts;
    }

    /// How many encodings the canvas picture behind cache key `path` can need at once: one per
    /// placement of it in the canvas on screen (never fewer than `MD_PROTO_SLOTS`). A picture
    /// placed at several rows of a canvas taller than the area is clipped differently at each, and
    /// every clip is an encoding of its own: a cap below the placements would leave some of them
    /// to the cells for good, or evict each other every frame.
    pub(super) fn canvas_slot_cap(&self, path: &str) -> usize {
        let placed = crate::preview::markdown::canvas_slot_of(path)
            .and_then(|s| self.canvas_slot_counts.get(s))
            .copied()
            .unwrap_or(0);
        placed.max(super::MD_PROTO_SLOTS)
    }

    /// Forget every canvas picture (the show went, or its view is not a canvas any more): the
    /// entries, the placements, and every kitty image the canvas pinned (deleted in the terminal).
    pub(super) fn forget_canvas_pictures(&mut self) {
        self.set_canvas_places(Vec::new());
        self.canvas_front.clear();
        self.canvas_kept.clear();
        self.prune_canvas_pictures(&HashSet::new());
        self.sweep_canvas_kitty_ids();
    }

    /// Delete from the terminal the images of the canvas pictures that are gone: a slot
    /// (`canvas://<plugin>/<id>`) with no entry left in the cache gives up its kitty ids (queued
    /// for `take_kitty_deletes`). The slot of a picture whose content changed still has its entry,
    /// so its ids stay and the new content is transmitted over them.
    pub(super) fn sweep_canvas_kitty_ids(&mut self) {
        let alive: HashSet<&str> = self
            .md_images
            .cache
            .keys()
            .filter_map(|k| crate::preview::markdown::canvas_slot_of(k.to_str()?))
            .collect();
        let dead: Vec<PathBuf> = self
            .md_images
            .kitty_ids
            .keys()
            .filter(|k| {
                let s = k.to_string_lossy();
                crate::preview::markdown::is_canvas_url(&s) && !alive.contains(&*s)
            })
            .cloned()
            .collect();
        for k in dead {
            if let Some(families) = self.md_images.kitty_ids.remove(&k) {
                self.md_images
                    .kitty_deletes
                    .extend(families.into_iter().flatten());
            }
        }
    }

    /// The escapes that delete from a kitty-graphics terminal the images of the canvas pictures
    /// that went (to be written to the terminal after the frame). Empty on any other terminal:
    /// no id is ever pinned there.
    pub fn take_kitty_deletes(&mut self) -> Vec<String> {
        self.md_images.take_kitty_deletes()
    }

    /// How many placements the canvas on screen has.
    pub(crate) fn canvas_place_count(&self) -> usize {
        self.canvas_places.len()
    }

    /// Placement `n` of the canvas on screen.
    pub(crate) fn canvas_place(&self, n: usize) -> Option<CanvasPlace> {
        self.canvas_places.get(n).cloned()
    }

    /// The key to draw picture `id` from, the content the plugin named (`wanted`) once it can be
    /// drawn, and until then the entry the id was drawn from before (so a picture whose content
    /// changes does not blink). The first time `wanted` can be drawn it becomes the entry of the
    /// id; a `wanted` that failed for good ends the old picture (the cells show). Called by the
    /// frame for each placement, after `ensure_md_image` asked for `wanted`.
    ///
    /// **This only moves the pointer (`canvas_front`); it never removes a cache entry.** The
    /// entries that are no longer named or drawn from go in one place, the prune when an answer
    /// lands (`canvas_pictures_landed`): a job in flight holds a snapshot of what the cache has, and
    /// an entry dropped by a frame meanwhile would be named by the answer without being there (a
    /// picture that flips between two contents would show the old one).
    pub(crate) fn canvas_shown_key(
        &mut self,
        id: u64,
        wanted: &Arc<str>,
        cols: u16,
        rows: u16,
        vis_rows: u16,
    ) -> Arc<str> {
        if self
            .md_image_proto(wanted, cols, rows, 0, vis_rows)
            .is_some()
        {
            if self.canvas_front.get(&id).map(|k| &**k) != Some(&**wanted) {
                self.canvas_front.insert(id, wanted.clone());
            }
            return wanted.clone();
        }
        if self
            .md_images
            .cache
            .get(Path::new(&**wanted))
            .is_some_and(|e| e.failed)
        {
            self.canvas_front.remove(&id);
            return wanted.clone();
        }
        match self.canvas_front.get(&id) {
            Some(front)
                if **front != **wanted
                    && self.md_images.cache.contains_key(Path::new(&**front)) =>
            {
                front.clone()
            }
            _ => wanted.clone(),
        }
    }

    /// Test-only: the wanted cache key and rectangle of each picture of the canvas on screen,
    /// in drawing order. Empty when no canvas is on screen.
    #[cfg(test)]
    pub(crate) fn canvas_image_placements(&self) -> Vec<(String, CellRect)> {
        if self.canvas_view().is_none() {
            return Vec::new();
        }
        self.canvas_places
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
        self.canvas_scope_fp
    }

    /// Test-only: what the next job would be told about the cache (as `canvas_known_keys`, which
    /// needs an image backend).
    #[cfg(test)]
    pub(super) fn canvas_known_keys_for_test(&self) -> CanvasHeld {
        let mut h = CanvasHeld {
            keys: HashMap::new(),
            by_id: HashMap::new(),
            front: self.canvas_front.clone(),
            kept: self.canvas_kept.clone(),
        };
        for (k, e) in &self.md_images.cache {
            if crate::preview::markdown::is_canvas_url(&k.to_string_lossy()) {
                h.keys.insert(k.clone(), e.pixel_bytes());
            }
        }
        for p in &self.canvas_places {
            if let Some(k) = &p.key {
                h.by_id.insert(p.id, k.clone());
            }
        }
        h
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
