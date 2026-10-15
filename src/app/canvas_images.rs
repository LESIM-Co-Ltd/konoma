//! The pictures a plugin places on its canvas (`CanvasView::images`), on the host side.
//!
//! A canvas picture goes through the same machinery as a Markdown inline image and a media-diff
//! side: its pixels sit in `App::md_image_cache` under a synthetic key, the encode worker turns
//! them into a protocol for the cell rectangle, and `ui/canvas.rs` places the result over the
//! cells. What is particular to a canvas is **who decodes and when**, and **what a picture is**:
//!
//! - **A picture is (plugin show, image id)**; its content is what that id holds *now*. The cache
//!   key is `canvas://<plugin id>/<image id>/<content hash>`
//!   ([`konoma_widgets::markdown::canvas_url`]): the same id with the same content is the same
//!   entry (the decode, the encode and the terminal's stored copy are reused), the same id with
//!   other content is a new entry that **replaces** the old one. The things that belong to the
//!   picture rather than to its content are keyed by the slot, `canvas://<plugin id>/<image id>`
//!   ([`konoma_widgets::markdown::canvas_slot_url`]): the kitty image ids (so new content is
//!   transmitted over the same terminal image, never added next to it) and the overlay signature
//!   (so changing content never clears the screen).
//! - **No blink**: while the new content has no protocol yet the previous entry of the id keeps
//!   being drawn (`App::canvas_shown_key`); when the new one can be drawn it takes over and the
//!   old entry goes. A new content that cannot be decoded ends the old picture (the cells show).
//! - **`image: None`** (`CanvasImage::unchanged`) means "the same picture as the previous answer
//!   for this id": it resolves to the key the last answer named for the id, with no hashing and no
//!   decoding. An id the host does not hold is a failed placement (the cells show), and so is one
//!   whose picture the host dropped. A plugin should send `None` for pictures that did not change
//!   (the content of a `Some` is hashed on every answer; that is linear in its size).
//! - **The worker decodes**, in the same job that ran the plugin's `view` ([`decode_canvas`],
//!   called from `PluginWork::run_inner`), never the UI thread, and only what the cache does not
//!   hold yet ([`CanvasHeld`]). A GIF decode observes the job's cancellation through the decode
//!   ticket, an untrusted SVG through its supervised child; the decode of the other kinds is
//!   short and checks the cancellation between pictures.
//! - **Failure is final for that content**: a picture that cannot be decoded (damaged, too large,
//!   an SVG the guard refused) gets a `failed` entry, so it is not asked for again on every frame
//!   and the plugin's cells under it stay. A picture that is only **not decoded yet** (the work
//!   limits of one answer) or **does not fit the memory bound for now** is not failed: it has no
//!   entry and is decoded on a following answer (the host asks for one when work was left over).
//! - **SVG from a plugin is untrusted**: it is drawn by the supervised child process
//!   (`decode_image_view`), except where the host believes the plugin's `trusted` claim
//!   (`plugins::draws_own_svg`).
//! - **Files**: an `ImageData::File` names a path the host opens for the plugin; only a regular
//!   file is opened (never a FIFO or a device, which would block the worker or read the user's
//!   keystrokes). A sandboxed plugin should send bytes (stage 3 may restrict this further).
//! - **Animation** (`ImageData::Frames`, and a GIF given as a file or as bytes) uses the Markdown
//!   GIF machinery as it is: the frames are kept on the cache entry and `advance_md_gifs_if_due`
//!   steps them, re-encoding under the same kitty id. Each frame of `Frames` is shown for at least
//!   [`MIN_FRAME_DELAY`] (a GIF's own rule), and the decoded frames are bounded by the inline GIF
//!   byte budget.
//! - **No pictures without an encoder**: when the app has no image backend (a terminal that cannot
//!   show pictures at all, a headless run) nothing is decoded and the plugin's cells stay; with a
//!   picker that only has halfblocks the pictures are drawn as halfblocks, like Markdown images.
//! - **Bounds** (canvas pixels cannot be rebuilt once dropped, so they are never evicted by the
//!   cache budget; these are what keep them bounded):
//!   - the pictures a canvas holds *in total*, over all its ids, are at most
//!     [`MAX_CANVAS_HELD_BYTES`] of decoded pixels. A picture that would not fit is refused for
//!     now (its cells show) and tried again on a later answer; what the canvas no longer names is
//!     dropped before anything is refused;
//!   - one answer adds at most [`MAX_CANVAS_BYTES`] of pixels, decodes at most
//!     [`MAX_DECODES_PER_ANSWER`] new pictures and reads at most
//!     [`MAX_INPUT_BYTES_PER_ANSWER`] of their input; the rest waits for the next answer;
//!   - one canvas names at most [`MAX_CANVAS_IMAGES`] distinct ids (the API refuses more; this is
//!     the second line).

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use konoma_plugin::view::{CanvasView, CellRect, ImageData, ImageView};

use super::*;

/// How many distinct pictures (ids) of one canvas the host looks at.
pub(super) const MAX_CANVAS_IMAGES: usize = 256;
/// How many bytes of decoded pixels one answer of the plugin may add to the cache; past it the
/// rest of the answer waits for the next one.
pub(super) const MAX_CANVAS_BYTES: u64 = 128 * 1024 * 1024;
/// How many bytes of decoded pixels all the pictures of the canvas may hold together.
pub(super) const MAX_CANVAS_HELD_BYTES: u64 = 256 * 1024 * 1024;
/// How many pictures one answer decodes (the ones the cache does not hold); the rest waits.
pub(super) const MAX_DECODES_PER_ANSWER: usize = 32;
/// How many bytes of picture input (encoded bytes, SVG, raw pixels, a file's size) one answer
/// decodes; the rest waits. A first picture is always decoded, however large, so an answer makes
/// progress.
pub(super) const MAX_INPUT_BYTES_PER_ANSWER: u64 = 64 * 1024 * 1024;
/// The shortest time a frame of an animation given as `ImageData::Frames` is shown (the GIF
/// rule, so an animation cannot ask for a re-encode every few milliseconds).
pub(super) const MIN_FRAME_DELAY: Duration = Duration::from_millis(20);

/// The limits of the canvas pictures (the constants above; a test uses small ones).
#[derive(Clone, Copy, Debug)]
pub(super) struct CanvasLimits {
    /// [`MAX_CANVAS_HELD_BYTES`]
    pub(super) held_bytes: u64,
    /// [`MAX_CANVAS_BYTES`]
    pub(super) answer_bytes: u64,
    /// [`MAX_DECODES_PER_ANSWER`]
    pub(super) decodes: usize,
    /// [`MAX_INPUT_BYTES_PER_ANSWER`]
    pub(super) input_bytes: u64,
}

impl CanvasLimits {
    pub(super) const PRODUCTION: CanvasLimits = CanvasLimits {
        held_bytes: MAX_CANVAS_HELD_BYTES,
        answer_bytes: MAX_CANVAS_BYTES,
        decodes: MAX_DECODES_PER_ANSWER,
        input_bytes: MAX_INPUT_BYTES_PER_ANSWER,
    };
}

/// What the worker made of one picture.
pub(super) enum CanvasDecoded {
    /// A still picture.
    Still(image::DynamicImage),
    /// An animation (at least one frame; the first is shown first).
    Frames(Vec<(image::DynamicImage, Duration)>),
    /// It could not be shown, with the reason.
    Failed(crate::preview::image::ImageFailure),
}

/// One placement of a canvas, as the host keeps it between the answer and the frames: what is
/// drawn where, with the cache key made once when the canvas landed (the frame loop allocates
/// nothing for it).
#[derive(Clone, Debug)]
pub(crate) struct CanvasPlace {
    /// The plugin's name of the picture.
    pub(crate) id: u64,
    /// The cells it covers in the canvas.
    pub(crate) rect: CellRect,
    /// The cache key of the content the plugin named for it; `None` when the plugin sent
    /// `unchanged` for an id the host does not hold (a failed placement).
    pub(crate) key: Option<Arc<str>>,
}

/// The pictures of a canvas, decoded on the worker.
pub(super) struct CanvasPictures {
    /// The placements of the canvas, in the order of `CanvasView::images`.
    pub(super) places: Vec<CanvasPlace>,
    /// The pictures decoded for this answer (the keys the cache did not hold yet), each once.
    pub(super) fresh: Vec<(PathBuf, CanvasDecoded)>,
    /// Pictures left for a following answer because of the work limits of this one.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) waiting: usize,
    /// Pictures decoded but refused for now because they would not fit the memory bound.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) refused: usize,
    /// Another answer is worth asking for: pictures wait and this answer made progress (so the
    /// next one does too, and the asking ends).
    pub(super) more: bool,
}

/// What the app tells the worker about the canvas pictures it holds.
#[derive(Clone, Debug, Default)]
pub(super) struct CanvasHeld {
    /// The cache keys held, with the decoded bytes of each.
    pub(super) keys: HashMap<PathBuf, u64>,
    /// The key the previous answer named for each id (what `unchanged` resolves to).
    pub(super) by_id: HashMap<u64, Arc<str>>,
    /// The entry each id is drawn from now (it stays while a newer content has no protocol).
    pub(super) front: HashMap<u64, Arc<str>>,
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

/// The key the kitty image ids of a cache key are filed under: the picture's identity
/// (`canvas://<plugin>/<id>`) for a canvas picture, so new content takes over the ids of the old
/// and the terminal's copy is replaced; the key itself for everything else.
pub(super) fn kitty_key(path: &Path) -> PathBuf {
    path.to_str()
        .and_then(crate::preview::markdown::canvas_slot_of)
        .map_or_else(|| path.to_path_buf(), PathBuf::from)
}

/// Whether `p` is a regular file: the only kind the host opens for a plugin (a FIFO would block
/// the worker for good, a device such as `/dev/tty` would read the user's keystrokes).
fn regular_file(p: &Path) -> bool {
    std::fs::metadata(p).is_ok_and(|m| m.is_file())
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
        ImageData::File(p) => {
            if !regular_file(p) {
                return Err(ImageFailure::Corrupt);
            }
            md_decode_image_why(p, default_raster_px, &cancelled)
        }
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
        ImageData::File(p) if regular_file(p) && App::looks_like_gif(p) => {
            crate::preview::image::decode_gif_inline(p)
        }
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
        let mut frames = Vec::with_capacity(list.len().min(256));
        for (f, d) in list {
            if cancel.is_some_and(|c| c.is_cancelled()) {
                return CanvasDecoded::Failed(ImageFailure::Cancelled);
            }
            match decode_still(f, path, default_raster_px, cancel, trust_own_svg) {
                Ok(img) => {
                    bytes += img.width() as u64 * img.height() as u64 * 4;
                    if bytes > crate::preview::image::MAX_GIF_BYTES_INLINE as u64 {
                        return CanvasDecoded::Failed(ImageFailure::TooLarge);
                    }
                    frames.push((img, (*d).max(MIN_FRAME_DELAY)));
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

/// A copy of a decoded picture (for a second id that holds the same content: decoded once).
fn copy_decoded(d: &CanvasDecoded) -> CanvasDecoded {
    match d {
        CanvasDecoded::Still(i) => CanvasDecoded::Still(i.clone()),
        CanvasDecoded::Frames(f) => CanvasDecoded::Frames(f.clone()),
        CanvasDecoded::Failed(w) => CanvasDecoded::Failed(*w),
    }
}

/// The bytes of input a decode of `data` reads (what the per-answer input budget counts).
fn input_bytes(data: &ImageData) -> u64 {
    match data {
        ImageData::File(p) => std::fs::metadata(p).map_or(0, |m| m.len()),
        ImageData::Encoded(b) => b.len() as u64,
        ImageData::Svg { data, .. } => data.len() as u64,
        ImageData::Rgba { pixels, .. } => pixels.len() as u64,
        ImageData::Frames(f) => f.iter().map(|(d, _)| input_bytes(d)).sum(),
        ImageData::Lazy(_) => 0,
    }
}

/// Decode the pictures of `canvas` that `held` (what the cache holds) does not have. Runs on the
/// worker. `path` is the file shown (relative references of an SVG resolve from it).
///
/// Two passes: the first names every placement (hashing each id's content once; an `unchanged`
/// picture resolves through `held` and is not hashed), which also tells how much of the memory
/// bound the canvas keeps anyway; the second decodes what is new within the limits of one answer.
pub(super) fn decode_canvas(
    plugin_id: &str,
    canvas: &CanvasView,
    held: &CanvasHeld,
    path: &Path,
    default_raster_px: u32,
    cancel: Option<&crate::preview::office::Cancel>,
    trust_own_svg: bool,
) -> CanvasPictures {
    decode_canvas_within(
        plugin_id,
        canvas,
        held,
        path,
        default_raster_px,
        cancel,
        trust_own_svg,
        CanvasLimits::PRODUCTION,
    )
}

/// [`decode_canvas`] under the given limits.
#[allow(clippy::too_many_arguments)]
pub(super) fn decode_canvas_within(
    plugin_id: &str,
    canvas: &CanvasView,
    held: &CanvasHeld,
    path: &Path,
    default_raster_px: u32,
    cancel: Option<&crate::preview::office::Cancel>,
    trust_own_svg: bool,
    limits: CanvasLimits,
) -> CanvasPictures {
    use crate::preview::image::ImageFailure;
    // Pass 1: the placements. The first placement of an id decides its key (an id is one picture).
    let mut places = Vec::with_capacity(canvas.images.len());
    let mut by_id: HashMap<u64, Arc<str>> = HashMap::new();
    // The pictures to decode: (key, hash, data), each key once, in order of appearance.
    let mut todo: Vec<(PathBuf, u64, &ImageData)> = Vec::new();
    for im in &canvas.images {
        let key = if let Some(k) = by_id.get(&im.id) {
            Some(k.clone())
        } else if by_id.len() >= MAX_CANVAS_IMAGES {
            None // past the second line of defence: not looked at
        } else {
            let key = match &im.image {
                None => held.by_id.get(&im.id).cloned(),
                Some(data) => {
                    let hash = content_hash(data);
                    let key: Arc<str> =
                        crate::preview::markdown::canvas_url(plugin_id, im.id, hash).into();
                    if !held.keys.contains_key(Path::new(&*key)) {
                        todo.push((PathBuf::from(&*key), hash, data));
                    }
                    Some(key)
                }
            };
            if let Some(k) = &key {
                by_id.insert(im.id, k.clone());
            }
            key
        };
        places.push(CanvasPlace {
            id: im.id,
            rect: im.rect(),
            key,
        });
    }
    // What the canvas keeps held whatever happens: the entries it names, and the entry each of
    // its ids is drawn from now (until the newer content can be drawn). The rest goes when the
    // answer lands, so only this counts against the memory bound.
    let mut kept: HashSet<&str> = by_id.values().map(|k| &**k).collect();
    for id in by_id.keys() {
        if let Some(f) = held.front.get(id) {
            kept.insert(&**f);
        }
    }
    let kept_bytes: u64 = kept
        .iter()
        .filter_map(|k| held.keys.get(Path::new(k)))
        .sum();
    // Pass 2: decode within the limits.
    let mut fresh: Vec<(PathBuf, CanvasDecoded)> = Vec::new();
    // content hash -> index in `fresh` (an id that holds content already decoded in this answer
    // gets a copy; nothing is decoded twice).
    let mut by_hash: HashMap<u64, usize> = HashMap::new();
    let mut added = 0u64;
    let (mut decodes, mut input) = (0usize, 0u64);
    // Left for a following answer by the work limits / refused for the memory bound for now.
    let (mut waiting, mut refused) = (0usize, 0usize);
    for (key, hash, data) in todo {
        let cancelled = cancel.is_some_and(|c| c.is_cancelled());
        let room = limits.held_bytes.saturating_sub(kept_bytes + added);
        let decoded = if cancelled {
            // The preview moved on: the answer is dropped on arrival; do not spend time on it.
            CanvasDecoded::Failed(ImageFailure::Cancelled)
        } else if let Some(&i) = by_hash.get(&hash) {
            let d = copy_decoded(&fresh[i].1);
            if decoded_bytes(&d) > room {
                refused += 1;
                continue;
            }
            d
        } else if decodes >= limits.decodes
            || input >= limits.input_bytes
            || added >= limits.answer_bytes
        {
            waiting += 1;
            continue;
        } else {
            #[cfg(test)]
            seen_for_tests::note(hash);
            decodes += 1;
            input += input_bytes(data);
            let d = decode_one(data, path, default_raster_px, cancel, trust_own_svg);
            let bytes = decoded_bytes(&d);
            if bytes > limits.held_bytes {
                // It can never fit.
                CanvasDecoded::Failed(ImageFailure::TooLarge)
            } else if bytes > room {
                // It does not fit next to what the canvas keeps: not final, tried again later.
                refused += 1;
                continue;
            } else {
                d
            }
        };
        added += decoded_bytes(&decoded);
        if !matches!(decoded, CanvasDecoded::Failed(_)) {
            by_hash.entry(hash).or_insert(fresh.len());
        }
        fresh.push((key, decoded));
    }
    let more = waiting > 0 && !fresh.is_empty();
    CanvasPictures {
        places,
        fresh,
        // Asking again helps when the work limits left pictures and this answer made progress;
        // pictures refused for the memory bound do not come back by asking.
        more,
        waiting,
        refused,
    }
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

    /// What the cache holds of the canvas pictures (given to the next job so it decodes only
    /// what is new). `None` when pictures cannot be shown.
    pub(super) fn canvas_known_keys(&self) -> Option<CanvasHeld> {
        self.canvas_pictures_drawable().then(|| CanvasHeld {
            keys: self
                .md_image_cache
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
        })
    }

    /// The bytes of decoded pixels the canvas entries of the cache hold.
    pub(super) fn canvas_held_bytes(&self) -> u64 {
        self.md_image_cache
            .iter()
            .filter(|(k, _)| crate::preview::markdown::is_canvas_url(&k.to_string_lossy()))
            .map(|(_, e)| e.pixel_bytes())
            .sum()
    }

    /// Put the pictures a landed canvas answer decoded into the cache. What the canvas no longer
    /// names goes first (so the memory bound is judged against what it keeps), then each new
    /// picture is added unless it would not fit; one refused for that is left out (not failed) and
    /// comes with a later answer. Returns whether another answer should be asked for
    /// ([`CanvasPictures::more`]).
    pub(super) fn canvas_pictures_landed(&mut self, pics: CanvasPictures) -> bool {
        self.canvas_pictures_landed_within(pics, MAX_CANVAS_HELD_BYTES)
    }

    /// [`Self::canvas_pictures_landed`] under the given memory bound.
    pub(super) fn canvas_pictures_landed_within(
        &mut self,
        pics: CanvasPictures,
        held_bytes: u64,
    ) -> bool {
        // The ids of the canvas, the entries it names and the entries its ids are drawn from.
        let ids: HashSet<u64> = pics.places.iter().map(|p| p.id).collect();
        self.canvas_front.retain(|id, _| ids.contains(id));
        let mut live: HashSet<PathBuf> = pics
            .places
            .iter()
            .filter_map(|p| p.key.as_deref().map(PathBuf::from))
            .collect();
        live.extend(self.canvas_front.values().map(|k| PathBuf::from(&**k)));
        for (key, _) in &pics.fresh {
            live.insert(key.clone());
        }
        self.prune_canvas_pictures(&live);
        let mut held = self.canvas_held_bytes();
        let mut added_any = false;
        for (key, decoded) in pics.fresh {
            // Already held (a job that was out before the previous answer landed): keep the
            // entry, with its encodes and its kitty ids, as it is.
            if self.md_image_cache.contains_key(&key) {
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
            if held + bytes > held_bytes {
                // Refused for now, and not remembered as failed: the next answer decodes it again.
                continue;
            }
            held += bytes;
            added_any = true;
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
        self.canvas_places = pics.places;
        self.sweep_canvas_kitty_ids();
        pics.more && added_any
    }

    /// Drop the cache entries of the canvas pictures not in `live` (and the drawn-from pointer of
    /// an id that points at one).
    pub(super) fn prune_canvas_pictures(&mut self, live: &HashSet<PathBuf>) {
        self.md_image_cache.retain(|k, _| {
            !crate::preview::markdown::is_canvas_url(&k.to_string_lossy()) || live.contains(k)
        });
        let cache = &self.md_image_cache;
        self.canvas_front
            .retain(|_, k| cache.contains_key(Path::new(&**k)));
    }

    /// Forget every canvas picture (the show went, or its view is not a canvas any more): the
    /// entries, the placements, and every kitty image the canvas pinned (deleted in the terminal).
    pub(super) fn forget_canvas_pictures(&mut self) {
        self.canvas_places.clear();
        self.canvas_front.clear();
        self.prune_canvas_pictures(&HashSet::new());
        self.sweep_canvas_kitty_ids();
    }

    /// Delete from the terminal the images of the canvas pictures that are gone: a slot
    /// (`canvas://<plugin>/<id>`) with no entry left in the cache gives up its kitty ids (queued
    /// for `take_kitty_deletes`). The slot of a picture whose content changed still has its entry,
    /// so its ids stay and the new content is transmitted over them.
    pub(super) fn sweep_canvas_kitty_ids(&mut self) {
        let alive: HashSet<&str> = self
            .md_image_cache
            .keys()
            .filter_map(|k| crate::preview::markdown::canvas_slot_of(k.to_str()?))
            .collect();
        let dead: Vec<PathBuf> = self
            .md_kitty_ids
            .keys()
            .filter(|k| {
                let s = k.to_string_lossy();
                crate::preview::markdown::is_canvas_url(&s) && !alive.contains(&*s)
            })
            .cloned()
            .collect();
        for k in dead {
            if let Some(families) = self.md_kitty_ids.remove(&k) {
                self.kitty_deletes.extend(families.into_iter().flatten());
            }
        }
    }

    /// The escapes that delete from a kitty-graphics terminal the images of the canvas pictures
    /// that went (to be written to the terminal after the frame). Empty on any other terminal:
    /// no id is ever pinned there.
    pub fn take_kitty_deletes(&mut self) -> Vec<String> {
        let ids = std::mem::take(&mut self.kitty_deletes);
        if !self.use_kitty {
            return Vec::new();
        }
        ids.into_iter()
            .map(|id| crate::preview::kitty::build_delete(id, self.kitty_is_tmux))
            .collect()
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
    /// id and the previous one is dropped from the cache; a `wanted` that failed for good ends the
    /// old picture (the cells show). Called by the frame for each placement, after
    /// `ensure_md_image` asked for `wanted`.
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
                if let Some(old) = self.canvas_front.insert(id, wanted.clone()) {
                    self.canvas_drop_entry(&old);
                }
            }
            return wanted.clone();
        }
        if self
            .md_image_cache
            .get(Path::new(&**wanted))
            .is_some_and(|e| e.failed)
        {
            if let Some(old) = self.canvas_front.remove(&id) {
                if *old != **wanted {
                    self.canvas_drop_entry(&old);
                }
            }
            return wanted.clone();
        }
        match self.canvas_front.get(&id) {
            Some(front)
                if **front != **wanted && self.md_image_cache.contains_key(Path::new(&**front)) =>
            {
                front.clone()
            }
            _ => wanted.clone(),
        }
    }

    /// Remove the cache entry `key` of a canvas picture that was replaced, unless a placement of
    /// the canvas still names it.
    fn canvas_drop_entry(&mut self, key: &Arc<str>) {
        if self
            .canvas_places
            .iter()
            .any(|p| p.key.as_deref() == Some(&**key))
        {
            return;
        }
        self.md_image_cache.remove(Path::new(&**key));
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
        for e in self.md_image_cache.values_mut() {
            if e.frames.len() >= 2 && e.shown_at.is_some() {
                e.shown_at = Some(long_ago);
            }
        }
    }

    /// Test-only: the inline-image overlay pass number, and the pass that last asked for `key`.
    #[cfg(test)]
    pub(crate) fn md_frame_and_last_used_for_test(&self, key: &Path) -> (u64, Option<u64>) {
        (
            self.md_frame,
            self.md_image_cache.get(key).map(|e| e.last_used),
        )
    }

    /// Test-only: what the next job would be told about the cache (as `canvas_known_keys`, which
    /// needs an image backend).
    #[cfg(test)]
    pub(super) fn canvas_known_keys_for_test(&self) -> CanvasHeld {
        let mut h = CanvasHeld {
            keys: HashMap::new(),
            by_id: HashMap::new(),
            front: self.canvas_front.clone(),
        };
        for (k, e) in &self.md_image_cache {
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
        self.md_kitty_ids.get(&kitty_key(key)).cloned()
    }
}

#[cfg(test)]
#[path = "canvas_images_tests.rs"]
mod tests;
