//! The per-picture state a host keeps for the canvas on screen, and the landing of a decoded
//! answer into the host's inline-image cache.
//!
//! Both hosts (konoma and the standalone plugin app) keep one [`CanvasPictureHost`] next to one
//! [`InlineImageCache`] and pass the cache to every call, so the cache stays the host's (it also
//! holds the Markdown pictures in konoma). What the landing does:
//!
//! - **No blink**: while the new content of an id has no protocol yet, the previous entry of the id
//!   keeps being drawn ([`CanvasPictureHost::shown_key`]); when the new one can be drawn it takes
//!   over. A frame only moves that pointer: the entries nobody names or draws from go in the prune
//!   when an answer lands (a job in flight holds a snapshot of the cache, and an entry dropped by a
//!   frame meanwhile would be named by its answer without being there). A new content that cannot
//!   be decoded ends the old picture (the cells show).
//! - **Landing** ([`CanvasPictureHost::landed`]): when an answer lands its fresh pictures are
//!   filed in the cache, the kitty ids of ids the canvas stopped naming are released, and what the
//!   work limits left is kept by its source.
//! - The key of a picture's cache entry and the things keyed by its slot (the kitty ids, the
//!   overlay signature) are described in the [module](super) doc.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use konoma_svg::image::ImageFailure;
use konoma_widgets::inline_images::{DecodedImage, Filed, InlineImageCache, MD_PROTO_SLOTS};
use konoma_widgets::markdown::{canvas_slot_of, is_canvas_url};

use super::{
    decoded_bytes, source_bytes, CanvasDecoded, CanvasHeld, CanvasPictures, CanvasPlace,
    KeptPicture, MAX_CANVAS_HELD_BYTES, MAX_CANVAS_KEPT_BYTES,
};

/// What a landing tells the host.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Landed {
    /// Another answer should be asked for ([`CanvasPictures::more`], or room grew for a picture
    /// that was refused).
    pub ask_again: bool,
    /// A picture was filed as failed: the host rebuilds whatever it laid out around the picture
    /// (konoma: its Markdown decoration cache).
    pub redecorate: bool,
}

/// The canvas pictures of one host: where they are placed, which entry each id is drawn from, the
/// sources still to be decoded and the read scope they were made under. The pixels, encodes and
/// kitty ids are in the host's [`InlineImageCache`], passed to each call.
#[derive(Default)]
pub struct CanvasPictureHost {
    /// The placements of the canvas on screen, in the order of `CanvasView::images`, with the
    /// cache key made once when the answer landed.
    pub places: Vec<CanvasPlace>,
    /// How many placements of the canvas on screen each picture (by `canvas_slot_of`) has: the
    /// number of encodings it can need at once (one per placement, a clipped one has its own).
    /// Built with `places`; the cap of a canvas picture's encode slots.
    pub slot_counts: HashMap<String, usize>,
    /// The cache entry each canvas picture (by id) is drawn from now. It stays while a newer
    /// content of the id has no protocol yet, so a changing picture does not blink.
    pub front: HashMap<u64, Arc<str>>,
    /// The read scope identity ([`crate::ReadScope::fingerprint`]) the canvas pictures held were
    /// made under; a job under another scope does not resolve `unchanged` to them.
    pub scope_fp: u64,
    /// The sources of the canvas pictures deferred or refused and not decoded yet, by id (kept
    /// from the answer that sent them, so the plugin need not send them again).
    pub kept: HashMap<u64, KeptPicture>,
}

impl CanvasPictureHost {
    /// What the cache holds of the canvas pictures (given to the next job so it decodes only what
    /// is new).
    pub fn known_keys(&self, cache: &InlineImageCache) -> CanvasHeld {
        CanvasHeld {
            keys: cache
                .cache
                .iter()
                .filter(|(k, _)| is_canvas_url(&k.to_string_lossy()))
                .map(|(k, e)| (k.clone(), e.pixel_bytes()))
                .collect(),
            by_id: self
                .places
                .iter()
                .filter_map(|p| p.key.clone().map(|k| (p.id, k)))
                .collect(),
            front: self.front.clone(),
            kept: self.kept.clone(),
        }
    }

    /// The bytes of decoded pixels the canvas entries of the cache hold.
    pub fn held_bytes(cache: &InlineImageCache) -> u64 {
        cache
            .cache
            .iter()
            .filter(|(k, _)| is_canvas_url(&k.to_string_lossy()))
            .map(|(_, e)| e.pixel_bytes())
            .sum()
    }

    /// Put the pictures a landed canvas answer decoded into the cache. What the canvas no longer
    /// names goes first (so the memory bound is judged against what it keeps), then each new
    /// picture is added unless it would not fit; one refused for that is left out (not failed) and
    /// comes with a later answer. `budget` is the host's bound on the decoded pixels of its whole
    /// cache, applied after each picture is filed (canvas pictures cannot be rebuilt, so it never
    /// drops one; it keeps the host's other pictures within bounds).
    pub fn landed(
        &mut self,
        cache: &mut InlineImageCache,
        pics: CanvasPictures,
        budget: u64,
    ) -> Landed {
        self.landed_with(
            cache,
            pics,
            budget,
            MAX_CANVAS_HELD_BYTES,
            MAX_CANVAS_KEPT_BYTES,
        )
    }

    /// [`Self::landed`] under the given memory bound (tests).
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn landed_within(
        &mut self,
        cache: &mut InlineImageCache,
        pics: CanvasPictures,
        budget: u64,
        held_bytes: u64,
    ) -> Landed {
        self.landed_with(cache, pics, budget, held_bytes, MAX_CANVAS_KEPT_BYTES)
    }

    /// [`Self::landed`] under the given memory bound and bound on the kept sources (tests).
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn landed_limits(
        &mut self,
        cache: &mut InlineImageCache,
        pics: CanvasPictures,
        budget: u64,
        held_bytes: u64,
        kept_source_bytes: u64,
    ) -> Landed {
        self.landed_with(cache, pics, budget, held_bytes, kept_source_bytes)
    }

    fn landed_with(
        &mut self,
        cache: &mut InlineImageCache,
        pics: CanvasPictures,
        budget: u64,
        held_bytes: u64,
        kept_source_bytes: u64,
    ) -> Landed {
        self.scope_fp = pics.scope_fp;
        let mut redecorate = false;
        // The ids of the canvas, the entries it names and the entries its ids are drawn from.
        let ids: HashSet<u64> = pics.places.iter().map(|p| p.id).collect();
        self.front.retain(|id, _| ids.contains(id));
        let named: HashSet<PathBuf> = pics
            .places
            .iter()
            .filter_map(|p| p.key.as_deref().map(PathBuf::from))
            .collect();
        // A new content of an id the memory bound refused (the worker knew it did not fit) ends the
        // old picture of that id: its cells show rather than a stale picture.
        for (id, k) in &pics.sources {
            if k.needs.is_some() {
                self.front_gives_way(*id, k.key.as_deref());
            }
        }
        let mut live: HashSet<PathBuf> = named.clone();
        live.extend(self.front.values().map(|k| PathBuf::from(&**k)));
        for (key, _) in &pics.fresh {
            live.insert(key.clone());
        }
        self.prune(cache, &live);
        let mut held = Self::held_bytes(cache);
        let mut added_any = false;
        // The decoded bytes of the pictures this landing refused (for the bound), by key.
        let mut refused_here: HashMap<PathBuf, u64> = HashMap::new();
        for (key, decoded) in pics.fresh {
            // Already held (a job that was out before the previous answer landed): keep the
            // entry, with its encodes and its kitty ids, as it is.
            if cache.cache.contains_key(&key) {
                continue;
            }
            let bytes = decoded_bytes(&decoded);
            let (image, frames) = match decoded {
                CanvasDecoded::Still(img) => (Ok(img), None),
                CanvasDecoded::Frames(f) if !f.is_empty() => (Ok(f[0].0.clone()), Some(f)),
                CanvasDecoded::Frames(_) => (
                    Err(ImageFailure::UnsupportedFormat.code().to_string()),
                    None,
                ),
                CanvasDecoded::Failed(why) => {
                    if why == ImageFailure::Cancelled {
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
                    self.front_gives_way(*id, Some(&key.to_string_lossy()));
                }
                self.prune_to_named(cache, &named);
                held = Self::held_bytes(cache);
            }
            if held + bytes > held_bytes {
                // Refused for now, and not remembered as failed: it is decoded again when there
                // is room (the host keeps its source, `kept`). Its ids show their cells.
                refused_here.insert(key, bytes);
                continue;
            }
            held += bytes;
            added_any = true;
            // Pre-placed, as every synthetic key is: `file_decoded` drops a result whose entry is
            // not there.
            cache.cache.entry(key.clone()).or_default();
            let request = cache.next_request(&key);
            let filed = cache.file_decoded(
                DecodedImage {
                    path: key,
                    image,
                    svg: None,
                    reraster: false,
                    frames,
                    request,
                },
                None,
            );
            match filed {
                Filed::Pixels => cache.evict_to(budget),
                // A picture that cannot be shown changes what the host reserved for it.
                Filed::Failed => redecorate = true,
                Filed::Dropped | Filed::Forgotten | Filed::Kept => {}
            }
        }
        // What is still not in the cache (deferred by the work limits or refused for the bound) is
        // kept by its source, per id, so a later answer that says `unchanged` still gets it
        // decoded. Replaced as a whole: an id the canvas no longer names, or a picture that landed,
        // is released.
        let unhashed_pending = pics.sources.iter().any(|(_, k)| k.key.is_none());
        let before = std::mem::take(&mut self.kept);
        let mut kept: Vec<(u64, KeptPicture)> = pics
            .sources
            .into_iter()
            .filter(|(_, k)| {
                k.key
                    .as_deref()
                    .is_none_or(|key| !cache.cache.contains_key(Path::new(key)))
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
        self.kept = rest.into_iter().collect();
        // What goes (the table of the answer before, the sources dropped for the bound) may be
        // the last holder of a large animation: it is freed off the UI thread.
        konoma_base::discard::discard_in_background((before, kept));
        self.prune_to_named(cache, &named);
        self.set_places(pics.places);
        self.sweep_kitty_ids(cache);
        // Ask again for what the work limits left (`more`), and for a refused picture that fits now
        // that room grew (an entry went): the next answer decodes it from the kept source.
        let room = held_bytes.saturating_sub(Self::held_bytes(cache));
        let room_for_refused = self
            .kept
            .values()
            .any(|k| k.needs.is_some_and(|n| n <= room));
        Landed {
            ask_again: (pics.more && (added_any || unhashed_pending)) || room_for_refused,
            redecorate,
        }
    }

    /// The entry the canvas id `id` is drawn from gives way to its new content `new` (which is
    /// not there to be drawn): the pointer goes, so the cells show and the entry is dropped by the
    /// next [`Self::prune_to_named`] unless something else names or draws from it. Nothing
    /// happens when `new` is the entry itself.
    fn front_gives_way(&mut self, id: u64, new: Option<&str>) {
        if self
            .front
            .get(&id)
            .is_some_and(|front| new != Some(&**front))
        {
            self.front.remove(&id);
        }
    }

    /// Drop the canvas entries that neither `named` (the keys the canvas names) nor a front
    /// pointer keeps.
    fn prune_to_named(&mut self, cache: &mut InlineImageCache, named: &HashSet<PathBuf>) {
        let mut live = named.clone();
        live.extend(self.front.values().map(|k| PathBuf::from(&**k)));
        self.prune(cache, &live);
    }

    /// Drop the cache entries of the canvas pictures not in `live` (and the drawn-from pointer of
    /// an id that points at one).
    pub fn prune(&mut self, cache: &mut InlineImageCache, live: &HashSet<PathBuf>) {
        cache
            .cache
            .retain(|k, _| !is_canvas_url(&k.to_string_lossy()) || live.contains(k));
        let entries = &cache.cache;
        self.front
            .retain(|_, k| entries.contains_key(Path::new(&**k)));
    }

    /// Replaces the placements of the canvas on screen and counts the placements of each picture.
    fn set_places(&mut self, places: Vec<CanvasPlace>) {
        let mut counts = HashMap::<String, usize>::new();
        for p in &places {
            if let Some(slot) = p.key.as_deref().and_then(canvas_slot_of) {
                *counts.entry(slot.to_string()).or_default() += 1;
            }
        }
        self.places = places;
        self.slot_counts = counts;
    }

    /// How many encodings the canvas picture behind cache key `path` can need at once: one per
    /// placement of it in the canvas on screen (never fewer than `MD_PROTO_SLOTS`). A picture
    /// placed at several rows of a canvas taller than the area is clipped differently at each, and
    /// every clip is an encoding of its own: a cap below the placements would leave some of them
    /// to the cells for good, or evict each other every frame.
    pub fn slot_cap(&self, path: &str) -> usize {
        let placed = canvas_slot_of(path)
            .and_then(|s| self.slot_counts.get(s))
            .copied()
            .unwrap_or(0);
        placed.max(MD_PROTO_SLOTS)
    }

    /// Forget every canvas picture (the show went, or its view is not a canvas any more): the
    /// entries, the placements, and every kitty image the canvas pinned (queued for deletion in
    /// the terminal).
    pub fn forget(&mut self, cache: &mut InlineImageCache) {
        self.set_places(Vec::new());
        self.front.clear();
        self.kept.clear();
        self.prune(cache, &HashSet::new());
        self.sweep_kitty_ids(cache);
    }

    /// Delete from the terminal the images of the canvas pictures that are gone: a slot
    /// (`canvas://<plugin>/<id>`) with no entry left in the cache gives up its kitty ids (queued
    /// for `InlineImageCache::take_kitty_deletes`). The slot of a picture whose content changed
    /// still has its entry, so its ids stay and the new content is transmitted over them.
    pub fn sweep_kitty_ids(&self, cache: &mut InlineImageCache) {
        let alive: HashSet<&str> = cache
            .cache
            .keys()
            .filter_map(|k| canvas_slot_of(k.to_str()?))
            .collect();
        let dead: Vec<PathBuf> = cache
            .kitty_ids
            .keys()
            .filter(|k| {
                let s = k.to_string_lossy();
                is_canvas_url(&s) && !alive.contains(&*s)
            })
            .cloned()
            .collect();
        for k in dead {
            if let Some(families) = cache.kitty_ids.remove(&k) {
                cache.kitty_deletes.extend(families.into_iter().flatten());
            }
        }
    }

    /// How many placements the canvas on screen has.
    pub fn place_count(&self) -> usize {
        self.places.len()
    }

    /// Placement `n` of the canvas on screen.
    pub fn place(&self, n: usize) -> Option<CanvasPlace> {
        self.places.get(n).cloned()
    }

    /// The key to draw picture `id` from, the content the plugin named (`wanted`) once it can be
    /// drawn, and until then the entry the id was drawn from before (so a picture whose content
    /// changes does not blink). The first time `wanted` can be drawn it becomes the entry of the
    /// id; a `wanted` that failed for good ends the old picture (the cells show). Called by the
    /// frame for each placement, after the encode of `wanted` was asked for.
    ///
    /// **This only moves the pointer (`front`); it never removes a cache entry.** The entries that
    /// are no longer named or drawn from go in one place, the prune when an answer lands
    /// ([`Self::landed`]): a job in flight holds a snapshot of what the cache has, and an entry
    /// dropped by a frame meanwhile would be named by the answer without being there (a picture
    /// that flips between two contents would show the old one).
    pub fn shown_key(
        &mut self,
        cache: &InlineImageCache,
        id: u64,
        wanted: &Arc<str>,
        cols: u16,
        rows: u16,
        vis_rows: u16,
    ) -> Arc<str> {
        if cache
            .proto(Path::new(&**wanted), cols, rows, 0, vis_rows)
            .is_some()
        {
            if self.front.get(&id).map(|k| &**k) != Some(&**wanted) {
                self.front.insert(id, wanted.clone());
            }
            return wanted.clone();
        }
        if cache
            .cache
            .get(Path::new(&**wanted))
            .is_some_and(|e| e.failed)
        {
            self.front.remove(&id);
            return wanted.clone();
        }
        match self.front.get(&id) {
            Some(front) if **front != **wanted && cache.cache.contains_key(Path::new(&**front)) => {
                front.clone()
            }
            _ => wanted.clone(),
        }
    }
}
