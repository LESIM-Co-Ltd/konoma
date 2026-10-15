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
//!   being drawn (`App::canvas_shown_key`); when the new one can be drawn it takes over. A frame
//!   only moves that pointer: the entries nobody names or draws from go in the prune when an
//!   answer lands (a job in flight holds a snapshot of the cache, and an entry dropped by a frame
//!   meanwhile would be named by its answer without being there). A new content that cannot be decoded ends the old picture (the cells show).
//! - **`image: None`** (`CanvasImage::unchanged`) means "the same picture as the previous answer
//!   for this id": it resolves to the key the last answer named for the id, with no hashing and no
//!   decoding. An id the host does not hold is a failed placement (the cells show). A plugin should
//!   send `None` for pictures that did not change (the content of a `Some` is hashed on every
//!   answer, each distinct buffer once; that is linear in its size).
//! - **The host keeps what it has not decoded** ([`KeptPicture`], `App::canvas_kept`): a picture
//!   the work limits of one answer left for later, or the memory bound refused for now, keeps its
//!   `ImageData` (an `Arc` clone) per id, and a later job decodes it even when the later answer
//!   says `None`. Newer `Some` content for the id replaces it; it is released when the canvas
//!   stops naming the id or the show goes. The plugin never has to send a picture again.
//! - **The worker decodes**, in the same job that ran the plugin's `view` ([`decode_canvas`],
//!   called from `PluginWork::run_inner`), never the UI thread, and only what the cache does not
//!   hold yet ([`CanvasHeld`]). A GIF decode observes the job's cancellation through the decode
//!   ticket, an untrusted SVG through its supervised child; the decode of the other kinds is
//!   short and checks the cancellation between pictures.
//! - **Failure is final for that content**: a picture that cannot be decoded (damaged, too large,
//!   an SVG the guard refused) gets a `failed` entry, so it is not asked for again on every frame
//!   and the plugin's cells under it stay. A picture that is only **not decoded yet** (the work
//!   limits of one answer) or **does not fit the memory bound for now** is not failed: it has no
//!   entry and is decoded on a following answer (the host asks for one when work was left over,
//!   at most once per `MIN_REDRAW_AFTER` and only while the canvas is on screen). One refused for
//!   the memory bound is not decoded again until that much room is free.
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
    /// The source of every picture this answer had to decode or defer, by id (`Arc` clones, no
    /// copy of the data). The app keeps those that are still not in the cache after landing
    /// ([`KeptPicture`]), so the plugin never has to send them again.
    pub(super) sources: Vec<(u64, KeptPicture)>,
}

/// The source of a picture the host has not turned into a cache entry yet because the work limits
/// of an answer left it for later, or the memory bound refused it for now. The host keeps it (per
/// canvas id) and decodes it on a later job even when the later answer says `unchanged`.
#[derive(Clone, Debug)]
pub(super) struct KeptPicture {
    /// The cache key of the content; `None` when it was not even hashed yet (the per-answer input
    /// budget left it).
    pub(super) key: Option<Arc<str>>,
    /// The content hash (the part of the key), when it was made.
    pub(super) hash: Option<u64>,
    /// What the plugin sent.
    pub(super) data: ImageData,
    /// The decoded bytes it needs when the memory bound refused it: it is not decoded again until
    /// that much room is free.
    pub(super) needs: Option<u64>,
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
    /// The pictures deferred or refused earlier, by id (what `unchanged` resolves to for them).
    pub(super) kept: HashMap<u64, KeptPicture>,
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

/// The identity of a buffer a plugin handed over: where its bytes are and how many.
type BufId = (usize, usize);

fn buf_id(b: &[u8]) -> BufId {
    (b.as_ptr() as usize, b.len())
}

/// How many bytes are hashed between two looks at the cancellation.
const HASH_CHUNK: usize = 1 << 20;

/// The hashing of one answer: each distinct buffer (one `Arc` cloned into many places, as an
/// animation of one frame repeated does) is hashed once, the cancellation is looked at while a
/// large buffer is hashed, and the bytes hashed are counted (the per-answer input budget is
/// applied before a picture is hashed).
struct Hasher<'a> {
    /// The hash of the bytes of each buffer hashed in this answer.
    memo: HashMap<BufId, u64>,
    /// The bytes of the distinct buffers hashed so far.
    bytes: u64,
    cancel: Option<&'a crate::preview::office::Cancel>,
}

impl<'a> Hasher<'a> {
    fn new(cancel: Option<&'a crate::preview::office::Cancel>) -> Hasher<'a> {
        Hasher {
            memo: HashMap::new(),
            bytes: 0,
            cancel,
        }
    }

    /// The hash of the bytes of `b`; `None` when the job was cancelled meanwhile.
    fn buffer(&mut self, b: &[u8]) -> Option<u64> {
        let id = buf_id(b);
        if let Some(h) = self.memo.get(&id) {
            return Some(*h);
        }
        let mut f = Fnv::new();
        for chunk in b.chunks(HASH_CHUNK) {
            if self.cancel.is_some_and(|c| c.is_cancelled()) {
                return None;
            }
            f.bytes(chunk);
        }
        self.memo.insert(id, f.0);
        self.bytes += b.len() as u64;
        Some(f.0)
    }

    /// A hash of what the picture is: its bytes or pixels, or for a file its path, size and
    /// modification time (a file that changed under the same id is another picture). `None` when
    /// the job was cancelled meanwhile.
    fn content(&mut self, data: &ImageData) -> Option<u64> {
        let mut h = Fnv::new();
        self.feed(&mut h, data)?;
        Some(h.0)
    }

    fn feed(&mut self, h: &mut Fnv, data: &ImageData) -> Option<()> {
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
                h.num(self.buffer(b)?);
                h.num(b.len() as u64);
            }
            ImageData::Svg {
                data,
                base_dir,
                trusted,
            } => {
                h.num(3);
                h.num(self.buffer(data)?);
                h.num(data.len() as u64);
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
                h.num(self.buffer(pixels)?);
                h.num(pixels.len() as u64);
            }
            ImageData::Frames(frames) => {
                h.num(5);
                for (f, d) in frames {
                    self.feed(h, f)?;
                    h.num(d.as_millis() as u64);
                }
            }
            ImageData::Lazy(_) => h.num(6),
        }
        Some(())
    }
}

/// A hash of what the picture is (see [`Hasher::content`]); every buffer is hashed in full.
#[cfg(test)]
pub(crate) fn content_hash(data: &ImageData) -> u64 {
    Hasher::new(None).content(data).unwrap_or_default()
}

/// Whether `a` and `b` are the very same buffers (the same allocation, so the same bytes): a
/// plugin that keeps one `Arc` and clones it into each answer is recognised without hashing. A
/// file is never "the same" (it may have changed on disk).
fn same_source(a: &ImageData, b: &ImageData) -> bool {
    match (a, b) {
        (ImageData::Encoded(x), ImageData::Encoded(y)) => buf_id(x) == buf_id(y),
        (
            ImageData::Svg {
                data: x,
                base_dir: bx,
                trusted: tx,
            },
            ImageData::Svg {
                data: y,
                base_dir: by,
                trusted: ty,
            },
        ) => buf_id(x) == buf_id(y) && bx == by && tx == ty,
        (
            ImageData::Rgba {
                width: wx,
                height: hx,
                pixels: x,
            },
            ImageData::Rgba {
                width: wy,
                height: hy,
                pixels: y,
            },
        ) => wx == wy && hx == hy && buf_id(x) == buf_id(y),
        (ImageData::Frames(x), ImageData::Frames(y)) => {
            x.len() == y.len()
                && x.iter()
                    .zip(y)
                    .all(|((fx, dx), (fy, dy))| dx == dy && same_source(fx, fy))
        }
        _ => false,
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
                    bytes += pixel_bytes_of(&img);
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

/// The bytes of pixels a decoded image holds: in the image's own pixel format (3 bytes for an
/// RGB8, 8 for an RGBA16, 16 for a float image), the measure of `MdImgEntry::pixel_bytes`, so the
/// worker, the landing and the cache all count the same.
fn pixel_bytes_of(i: &image::DynamicImage) -> u64 {
    use image::GenericImageView;
    let (w, h) = i.dimensions();
    u64::from(w) * u64::from(h) * u64::from(i.color().bytes_per_pixel())
}

/// The pixel bytes a decoded picture holds in the cache.
fn decoded_bytes(d: &CanvasDecoded) -> u64 {
    match d {
        CanvasDecoded::Still(i) => pixel_bytes_of(i),
        CanvasDecoded::Frames(f) => f.iter().map(|(i, _)| pixel_bytes_of(i)).sum(),
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
    let is_cancelled = || cancel.is_some_and(|c| c.is_cancelled());
    // Pass 1: the placements. The first placement of an id decides its key (an id is one picture).
    // Each id is resolved to a key without decoding anything: a picture the cache holds, one to
    // decode (`todo`), or one left for later. The pictures the host kept from an earlier answer
    // go first, so a picture deferred before is not starved by the new ones.
    let mut hasher = Hasher::new(cancel);
    // (order of appearance, id, the placement that decided it)
    let mut ids: Vec<(usize, u64, &konoma_plugin::view::CanvasImage)> = Vec::new();
    {
        let mut seen: HashSet<u64> = HashSet::new();
        for im in &canvas.images {
            if seen.len() >= MAX_CANVAS_IMAGES && !seen.contains(&im.id) {
                continue; // past the second line of defence: not looked at
            }
            if seen.insert(im.id) {
                ids.push((ids.len(), im.id, im));
            }
        }
    }
    ids.sort_by_key(|(order, id, _)| (!held.kept.contains_key(id), *order));
    let mut key_of: HashMap<u64, Arc<str>> = HashMap::new();
    // The pictures to decode, each key once: (order, id, key, hash, data, needs).
    let mut todo: Vec<Todo> = Vec::new();
    // The pictures not hashed because the input budget of this answer was used.
    let mut unhashed = 0usize;
    // The kept sources not turned into `todo` (not hashed yet).
    let mut sources: Vec<(u64, KeptPicture)> = Vec::new();
    let mut hashed_any = false;
    for (order, id, im) in ids {
        let kept = held.kept.get(&id);
        // The data this id is about: what the plugin sent, or for `unchanged` what the host kept.
        let data = match (&im.image, kept) {
            (Some(d), _) => d,
            (None, Some(k)) => &k.data,
            (None, None) => {
                // Nothing kept: the picture is in the cache or the id is not held.
                if let Some(k) = held.by_id.get(&id) {
                    key_of.insert(id, k.clone());
                }
                continue;
            }
        };
        // A hash made earlier for the very same buffers is reused (no hashing).
        let known = kept
            .filter(|k| k.hash.is_some() && same_source(&k.data, data))
            .and_then(|k| k.hash);
        let hash = match known {
            Some(h) => h,
            None => {
                // The input budget, before the hashing (as for the decode: a picture starts while
                // less than the budget is used): the first picture is always hashed, so an answer
                // makes progress; the others wait for the next answer (unhashed: the picture the
                // id was drawn from stays on screen).
                if hashed_any && hasher.bytes >= limits.input_bytes {
                    unhashed += 1;
                    if let Some(k) = held.by_id.get(&id) {
                        key_of.insert(id, k.clone());
                    }
                    sources.push((
                        id,
                        KeptPicture {
                            key: None,
                            hash: None,
                            data: data.clone(),
                            needs: None,
                        },
                    ));
                    continue;
                }
                match hasher.content(data) {
                    Some(h) => {
                        hashed_any = true;
                        h
                    }
                    None => {
                        // Cancelled while hashing: the answer is dropped on arrival.
                        return CanvasPictures {
                            places: canvas
                                .images
                                .iter()
                                .map(|im| CanvasPlace {
                                    id: im.id,
                                    rect: im.rect(),
                                    key: None,
                                })
                                .collect(),
                            fresh: Vec::new(),
                            waiting: 0,
                            refused: 0,
                            more: false,
                            sources: Vec::new(),
                        };
                    }
                }
            }
        };
        let key: Arc<str> = crate::preview::markdown::canvas_url(plugin_id, id, hash).into();
        key_of.insert(id, key.clone());
        if !held.keys.contains_key(Path::new(&*key)) {
            // The refusal of the memory bound is remembered with the picture, so it is not decoded
            // again to be refused again.
            let needs = kept
                .filter(|k| k.key.as_deref() == Some(&*key))
                .and_then(|k| k.needs);
            todo.push(Todo {
                order,
                id,
                key: key.clone(),
                hash,
                data: data.clone(),
                needs,
            });
        }
    }
    todo.sort_by_key(|t| t.order);
    let places: Vec<CanvasPlace> = canvas
        .images
        .iter()
        .map(|im| CanvasPlace {
            id: im.id,
            rect: im.rect(),
            key: key_of.get(&im.id).cloned(),
        })
        .collect();
    // What the canvas keeps held whatever happens: the entries it names, and the entry each of
    // its ids is drawn from now (until the newer content can be drawn). The rest goes when the
    // answer lands, so only this counts against the memory bound.
    let mut kept: HashSet<&str> = key_of.values().map(|k| &**k).collect();
    for id in key_of.keys() {
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
    let (mut waiting, mut refused) = (unhashed, 0usize);
    let keep = |t: Todo, needs: Option<u64>| {
        (
            t.id,
            KeptPicture {
                key: Some(t.key),
                hash: Some(t.hash),
                data: t.data,
                needs,
            },
        )
    };
    for t in todo {
        let cancelled = is_cancelled();
        let room = limits.held_bytes.saturating_sub(kept_bytes + added);
        let decoded = if cancelled {
            // The preview moved on: the answer is dropped on arrival; do not spend time on it.
            CanvasDecoded::Failed(ImageFailure::Cancelled)
        } else if let Some(&i) = by_hash.get(&t.hash) {
            let d = copy_decoded(&fresh[i].1);
            let bytes = decoded_bytes(&d);
            if bytes > room {
                refused += 1;
                sources.push(keep(t, Some(bytes)));
                continue;
            }
            d
        } else if t.needs.is_some_and(|n| n > room) {
            // Refused before, and the room has not grown to what it needs: nothing is decoded.
            refused += 1;
            let needs = t.needs;
            sources.push(keep(t, needs));
            continue;
        } else if decodes >= limits.decodes
            || input >= limits.input_bytes
            || added >= limits.answer_bytes
        {
            waiting += 1;
            let needs = t.needs;
            sources.push(keep(t, needs));
            continue;
        } else {
            #[cfg(test)]
            seen_for_tests::note(t.hash);
            decodes += 1;
            input += input_bytes(&t.data);
            let d = decode_one(&t.data, path, default_raster_px, cancel, trust_own_svg);
            let bytes = decoded_bytes(&d);
            if bytes > limits.held_bytes {
                // It can never fit.
                CanvasDecoded::Failed(ImageFailure::TooLarge)
            } else if bytes > room {
                // It does not fit next to what the canvas keeps: not final, tried again later.
                refused += 1;
                sources.push(keep(t, Some(bytes)));
                continue;
            } else {
                d
            }
        };
        added += decoded_bytes(&decoded);
        if !matches!(decoded, CanvasDecoded::Failed(_)) {
            by_hash.entry(t.hash).or_insert(fresh.len());
        }
        let key = PathBuf::from(&*t.key);
        sources.push(keep(t, None));
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
        sources,
    }
}

/// A picture to decode in pass 2.
struct Todo {
    /// Where the id first appears in the canvas (the order of decoding).
    order: usize,
    id: u64,
    key: Arc<str>,
    hash: u64,
    data: ImageData,
    /// The decoded bytes it needed when the memory bound refused it.
    needs: Option<u64>,
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
            kept: self.canvas_kept.clone(),
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
        // The decoded bytes of the pictures this landing refused (for the bound), by key.
        let mut refused_here: HashMap<PathBuf, u64> = HashMap::new();
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
                // Refused for now, and not remembered as failed: it is decoded again when there
                // is room (the host keeps its source, `canvas_kept`).
                refused_here.insert(key, bytes);
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
        // What is still not in the cache (deferred by the work limits or refused for the bound) is
        // kept by its source, per id, so a later answer that says `unchanged` still gets it
        // decoded. Replaced as a whole: an id the canvas no longer names, or a picture that landed,
        // is released.
        self.canvas_kept = pics
            .sources
            .into_iter()
            .filter(|(_, k)| {
                k.key
                    .as_deref()
                    .is_none_or(|key| !self.md_image_cache.contains_key(Path::new(key)))
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
            .md_image_cache
            .get(Path::new(&**wanted))
            .is_some_and(|e| e.failed)
        {
            self.canvas_front.remove(&id);
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
            kept: self.canvas_kept.clone(),
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
