//! The pictures a plugin places on its canvas (`CanvasView::images`), on the host side.
//!
//! A canvas picture goes through the same machinery as a Markdown inline image and a media-diff
//! side: its pixels sit in `App::md_image_cache` under a synthetic key, the encode worker turns
//! them into a protocol for the cell rectangle, and `ui/canvas.rs` places the result over the
//! cells. What is particular to a canvas is **who decodes and when**:
//!
//! - **The worker decodes**, in the same job that ran the plugin's `view` ([`decode_canvas`],
//!   called from `PluginWork::run_inner`), never the UI thread. Only the pictures the cache does
//!   not hold yet are decoded: the job is given the keys the cache already has
//!   (`PluginWork::canvas_known`), so an animation that sends the same sprite on every redraw
//!   decodes it once.
//! - **The key says what the picture is**: `canvas://<plugin id>/<image id>/<content hash>`
//!   ([`konoma_widgets::markdown::canvas_url`]). The same id with the same content is the same
//!   key (the decode, the encode and the terminal's stored copy under its kitty id are reused);
//!   the same id with other content is another key, and the entry of the old one is dropped when
//!   the new canvas lands ([`App::canvas_pictures_landed`]). Ids are scoped to the plugin show:
//!   the entries are dropped with it (`App::drop_plugin`).
//! - **Failure is final for that content**: a picture that cannot be decoded (damaged, too large,
//!   an SVG the guard refused) gets a `failed` entry, so it is not asked for again on every frame
//!   and the plugin's cells under it stay visible.
//! - **SVG from a plugin is untrusted**: it is drawn by the supervised child process
//!   (`decode_image_view`), except where the host believes the plugin's `trusted` claim
//!   (`plugins::draws_own_svg`).
//! - **Animation** (`ImageData::Frames`, and a GIF given as a file or as bytes) uses the Markdown
//!   GIF machinery as it is: the frames are kept on the cache entry and `advance_md_gifs_if_due`
//!   steps them, re-encoding under the same kitty id. The inline GIF byte budget applies.
//! - **No pictures without an encoder**: when the app has no image backend (a terminal that cannot
//!   show pictures at all, a headless run) nothing is decoded and the plugin's cells stay; with a
//!   picker that only has halfblocks the pictures are drawn as halfblocks, like Markdown images.
//! - **Bounds**: at most [`MAX_CANVAS_IMAGES`] pictures are decoded for one canvas and at most
//!   [`MAX_CANVAS_BYTES`] of decoded pixels per answer; the rest fail as too large (their cells
//!   stay). Canvas pixels cannot be rebuilt once dropped (the bytes were the plugin's), so they
//!   are never evicted by the cache budget, which is why this bound exists.

use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Duration;

use konoma_plugin::view::{CanvasView, ImageData, ImageView};

use super::*;

/// How many pictures of one canvas are decoded; the rest fail as too large.
pub(super) const MAX_CANVAS_IMAGES: usize = 256;
/// How many bytes of decoded pixels one answer of the plugin may add to the cache; a picture past
/// it fails as too large.
pub(super) const MAX_CANVAS_BYTES: u64 = 128 * 1024 * 1024;

/// What the worker made of one picture.
pub(super) enum CanvasDecoded {
    /// A still picture.
    Still(image::DynamicImage),
    /// An animation (at least one frame; the first is shown first).
    Frames(Vec<(image::DynamicImage, Duration)>),
    /// It could not be shown, with the reason.
    Failed(crate::preview::image::ImageFailure),
}

/// The pictures of a canvas, decoded on the worker.
pub(super) struct CanvasPictures {
    /// The cache key of each picture of the canvas, in the order of `CanvasView::images`.
    pub(super) keys: Vec<PathBuf>,
    /// The pictures decoded for this answer (the keys the cache did not hold yet), each once.
    pub(super) fresh: Vec<(PathBuf, CanvasDecoded)>,
}

/// FNV-1a 64 over several pieces (the same constants as `media_diff_url`'s hash).
struct Fnv(u64);

impl Fnv {
    fn new() -> Fnv {
        Fnv(0xcbf2_9ce4_8422_2325)
    }
    fn bytes(&mut self, b: &[u8]) {
        for &x in b {
            self.0 ^= x as u64;
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    fn num(&mut self, n: u64) {
        self.bytes(&n.to_le_bytes());
    }
}

/// A hash of what the picture is: its bytes or pixels, or for a file its path, size and
/// modification time (a file that changed under the same id is another picture).
pub(crate) fn content_hash(data: &ImageData) -> u64 {
    let mut h = Fnv::new();
    feed(&mut h, data);
    h.0
}

fn feed(h: &mut Fnv, data: &ImageData) {
    match data {
        ImageData::File(p) => {
            h.num(1);
            h.bytes(p.as_os_str().as_encoded_bytes());
            if let Ok(m) = std::fs::metadata(p) {
                h.num(m.len());
                let t = m
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map_or(0, |d| d.as_nanos() as u64);
                h.num(t);
            }
        }
        ImageData::Encoded(b) => {
            h.num(2);
            h.bytes(b);
        }
        ImageData::Svg {
            data,
            base_dir,
            trusted,
        } => {
            h.num(3);
            h.bytes(data);
            h.num(*trusted as u64);
            if let Some(d) = base_dir {
                h.bytes(d.as_os_str().as_encoded_bytes());
            }
        }
        ImageData::Rgba {
            width,
            height,
            pixels,
        } => {
            h.num(4);
            h.num(*width as u64);
            h.num(*height as u64);
            h.bytes(pixels);
        }
        ImageData::Frames(frames) => {
            h.num(5);
            for (f, d) in frames {
                feed(h, f);
                h.num(d.as_millis() as u64);
            }
        }
        ImageData::Lazy(_) => h.num(6),
    }
}

/// Decode one still picture (no animation): the reason when it cannot be shown.
fn decode_still(
    data: &ImageData,
    path: &Path,
    default_raster_px: u32,
    cancel: Option<&crate::preview::office::Cancel>,
    trust_own_svg: bool,
) -> Result<image::DynamicImage, crate::preview::image::ImageFailure> {
    use crate::preview::image::ImageFailure;
    let cancelled = || cancel.is_some_and(|c| c.is_cancelled());
    match data {
        ImageData::File(p) => md_decode_image_why(p, default_raster_px, &cancelled),
        ImageData::Encoded(b) => {
            crate::preview::image::decode_static_bytes_capped_why(b, MD_IMAGE_MAX_SIDE)
        }
        ImageData::Svg { .. } | ImageData::Rgba { .. } => {
            match super::media_plugin::decode_image_view(
                ImageView::new(data.clone()),
                path,
                default_raster_px,
                cancel,
                trust_own_svg,
            ) {
                Some(MediaPayload::Static(img)) => Ok(img),
                Some(MediaPayload::Vector { img, .. }) => Ok(img),
                Some(MediaPayload::ImageFailed(why)) => Err(why),
                _ => Err(ImageFailure::Corrupt),
            }
        }
        ImageData::Frames(_) | ImageData::Lazy(_) => Err(ImageFailure::UnsupportedFormat),
    }
}

/// Decode one picture of a canvas, an animation included.
fn decode_one(
    data: &ImageData,
    path: &Path,
    default_raster_px: u32,
    cancel: Option<&crate::preview::office::Cancel>,
    trust_own_svg: bool,
) -> CanvasDecoded {
    use crate::preview::image::ImageFailure;
    let animated = match data {
        // A GIF is an animation when it has more than one frame; anything else (a single frame,
        // a damaged one) falls through to the still decode, as an inline Markdown image does.
        ImageData::File(p) if App::looks_like_gif(p) => crate::preview::image::decode_gif_inline(p),
        ImageData::Encoded(b) if b.starts_with(b"GIF8") => {
            crate::preview::image::decode_gif_bytes_inline(b).map(|(frames, _)| frames)
        }
        _ => None,
    };
    if let Some(frames) = animated.filter(|f| !f.is_empty()) {
        return CanvasDecoded::Frames(frames);
    }
    if let ImageData::Frames(list) = data {
        if list.is_empty() {
            return CanvasDecoded::Failed(ImageFailure::UnsupportedFormat);
        }
        let mut bytes = 0u64;
        let mut frames = Vec::with_capacity(list.len());
        for (f, d) in list {
            match decode_still(f, path, default_raster_px, cancel, trust_own_svg) {
                Ok(img) => {
                    bytes += img.width() as u64 * img.height() as u64 * 4;
                    if bytes > crate::preview::image::MAX_GIF_BYTES_INLINE as u64 {
                        return CanvasDecoded::Failed(ImageFailure::TooLarge);
                    }
                    frames.push((img, *d));
                }
                Err(why) => return CanvasDecoded::Failed(why),
            }
        }
        return CanvasDecoded::Frames(frames);
    }
    match decode_still(data, path, default_raster_px, cancel, trust_own_svg) {
        Ok(img) => CanvasDecoded::Still(img),
        Err(why) => CanvasDecoded::Failed(why),
    }
}

/// The pixel bytes a decoded picture holds in the cache.
fn decoded_bytes(d: &CanvasDecoded) -> u64 {
    let of = |i: &image::DynamicImage| i.width() as u64 * i.height() as u64 * 4;
    match d {
        CanvasDecoded::Still(i) => of(i),
        CanvasDecoded::Frames(f) => f.iter().map(|(i, _)| of(i)).sum(),
        CanvasDecoded::Failed(_) => 0,
    }
}

/// Decode the pictures of `canvas` that `known` (the keys the cache holds) does not have. Runs on
/// the worker. `path` is the file shown (relative references of an SVG resolve from it).
pub(super) fn decode_canvas(
    plugin_id: &str,
    canvas: &CanvasView,
    known: &HashSet<PathBuf>,
    path: &Path,
    default_raster_px: u32,
    cancel: Option<&crate::preview::office::Cancel>,
    trust_own_svg: bool,
) -> CanvasPictures {
    use crate::preview::image::ImageFailure;
    let mut keys = Vec::with_capacity(canvas.images.len());
    let mut fresh: Vec<(PathBuf, CanvasDecoded)> = Vec::new();
    let mut budget = MAX_CANVAS_BYTES;
    for (n, im) in canvas.images.iter().enumerate() {
        let key = PathBuf::from(crate::preview::markdown::canvas_url(
            plugin_id,
            im.id,
            content_hash(&im.image),
        ));
        keys.push(key.clone());
        if known.contains(&key) || fresh.iter().any(|(k, _)| *k == key) {
            continue;
        }
        let decoded = if n >= MAX_CANVAS_IMAGES {
            CanvasDecoded::Failed(ImageFailure::TooLarge)
        } else if cancel.is_some_and(|c| c.is_cancelled()) {
            // The preview moved on: the answer is dropped on arrival; do not spend time on it.
            CanvasDecoded::Failed(ImageFailure::Cancelled)
        } else {
            #[cfg(test)]
            seen_for_tests::note(content_hash(&im.image));
            let d = decode_one(&im.image, path, default_raster_px, cancel, trust_own_svg);
            let bytes = decoded_bytes(&d);
            if bytes > budget {
                CanvasDecoded::Failed(ImageFailure::TooLarge)
            } else {
                budget -= bytes;
                d
            }
        };
        fresh.push((key, decoded));
    }
    CanvasPictures { keys, fresh }
}

/// Test-only record of which pictures the worker actually decoded (by content hash), so a test
/// can say that a picture sent again was not decoded again.
#[cfg(test)]
pub(crate) mod seen_for_tests {
    use std::sync::Mutex;
    static SEEN: Mutex<Vec<u64>> = Mutex::new(Vec::new());
    pub(super) fn note(hash: u64) {
        if let Ok(mut v) = SEEN.lock() {
            v.push(hash);
        }
    }
    /// How many times the picture with this content hash was decoded.
    pub(crate) fn decoded_count(hash: u64) -> usize {
        SEEN.lock()
            .map(|v| v.iter().filter(|h| **h == hash).count())
            .unwrap_or(0)
    }
}

impl App {
    /// Whether canvas pictures can be shown at all: an image backend and the encode worker are
    /// attached. Without them no picture is decoded and the plugin's cells stay.
    pub(super) fn canvas_pictures_drawable(&self) -> bool {
        self.picker.is_some() && self.md_enc_tx.is_some()
    }

    /// The keys of the canvas pictures the cache holds (given to the next job so it decodes only
    /// what is new). `None` when pictures cannot be shown.
    pub(super) fn canvas_known_keys(&self) -> Option<HashSet<PathBuf>> {
        self.canvas_pictures_drawable().then(|| {
            self.md_image_cache
                .keys()
                .filter(|k| crate::preview::markdown::is_canvas_url(&k.to_string_lossy()))
                .cloned()
                .collect()
        })
    }

    /// Put the pictures a landed canvas answer decoded into the cache and drop the entries of the
    /// pictures the canvas no longer has. `None` pictures with a canvas = no change to the keys
    /// (a canvas without pictures drops them all).
    pub(super) fn canvas_pictures_landed(&mut self, pics: CanvasPictures) {
        for (key, decoded) in pics.fresh {
            // Already held (a job that was out before the previous answer landed): keep the
            // entry, with its encodes and its kitty ids, as it is.
            if self.md_image_cache.contains_key(&key) {
                continue;
            }
            let (image, frames) = match decoded {
                CanvasDecoded::Still(img) => (Ok(img), None),
                CanvasDecoded::Frames(f) => (Ok(f[0].0.clone()), Some(f)),
                CanvasDecoded::Failed(why) => {
                    if why == crate::preview::image::ImageFailure::Cancelled {
                        continue;
                    }
                    (Err(why.code().to_string()), None)
                }
            };
            // Pre-placed, as every synthetic key is: `apply_md_image` drops a result whose entry
            // is not there.
            self.md_image_cache.entry(key.clone()).or_default();
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
        let live: HashSet<PathBuf> = pics.keys.iter().cloned().collect();
        self.prune_canvas_pictures(&live);
        self.canvas_keys = pics.keys;
    }

    /// Drop the cache entries of the canvas pictures not in `live`.
    pub(super) fn prune_canvas_pictures(&mut self, live: &HashSet<PathBuf>) {
        self.md_image_cache.retain(|k, _| {
            !crate::preview::markdown::is_canvas_url(&k.to_string_lossy()) || live.contains(k)
        });
    }

    /// Forget every canvas picture (the show went, or its view is not a canvas any more).
    pub(super) fn forget_canvas_pictures(&mut self) {
        self.canvas_keys.clear();
        self.prune_canvas_pictures(&HashSet::new());
    }

    /// Where each picture of the canvas on screen goes: its cache key and its cell rectangle in
    /// the canvas, in drawing order. Empty when no canvas is on screen.
    pub(crate) fn canvas_image_placements(&self) -> Vec<(String, konoma_plugin::view::CellRect)> {
        let Some(canvas) = self.canvas_view() else {
            return Vec::new();
        };
        canvas
            .images
            .iter()
            .zip(&self.canvas_keys)
            .map(|(im, key)| {
                (
                    key.to_string_lossy().into_owned(),
                    konoma_plugin::view::CellRect::new(im.x, im.y, im.cols, im.rows),
                )
            })
            .collect()
    }

    /// Test-only: the kitty image ids pinned for a cache key, per slot family.
    #[cfg(test)]
    pub(crate) fn md_kitty_ids_for_test(&self, key: &Path) -> Option<[Vec<u32>; 3]> {
        self.md_kitty_ids.get(key).cloned()
    }
}

#[cfg(test)]
#[path = "canvas_images_tests.rs"]
mod tests;
