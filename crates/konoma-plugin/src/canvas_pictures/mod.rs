//! The pictures a plugin places on its canvas (`CanvasView::images`), host-neutral: what is decoded,
//! how it is keyed and what bounds the work. Both hosts (konoma and the standalone plugin app) use
//! this one implementation, so a canvas picture looks and behaves the same in either.
//!
//! The host owns the cache the pixels go into, the encode worker and the drawing; this module owns
//! the decisions that do not depend on the host:
//!
//! - **A picture is (plugin show, image id)**; its content is what that id holds *now*. The cache
//!   key is `canvas://<plugin id>/<image id>/<content hash>`
//!   ([`konoma_widgets::markdown::canvas_url`]): the same id with the same content is the same
//!   entry, the same id with other content is a new entry that replaces the old one. The hash
//!   mixes in the identity of the read scope ([`mix_scope`]), so a file judged under one scope is
//!   never served for another.
//! - **`image: None`** (`CanvasImage::unchanged`) means "the same picture as the previous answer
//!   for this id": it resolves to the key the last answer named for the id ([`CanvasHeld::by_id`]),
//!   with no hashing and no decoding. An id the host does not hold is a failed placement. A plugin
//!   should send `None` for pictures that did not change (the content of a `Some` is hashed on every
//!   answer, each distinct buffer once; that is linear in its size).
//! - **The host keeps what it has not decoded** ([`KeptPicture`]): a picture the work limits of one
//!   answer left for later, or the memory bound refused for now, keeps its `ImageData` (an `Arc`
//!   clone) per id, and a later [`decode_canvas`] decodes it even when the later answer says
//!   `None`. The plugin never has to send a picture again.
//! - **Decoding** ([`decode_canvas`]) runs on a worker, never the UI thread, and only what the
//!   cache does not hold yet ([`CanvasHeld`]). A GIF decode and the hashing observe the job's
//!   cancellation; the decode of the other kinds is short and checks it between pictures.
//! - **Failure is final for that content**: a picture that cannot be decoded (damaged, too large,
//!   outside the read scope, an SVG the guard refused) gets a `Failed` entry, so it is not asked
//!   for again on every frame and the plugin's cells under it stay. A picture that is only **not
//!   decoded yet** (the work limits of one answer) or **does not fit the memory bound for now** is
//!   not failed: it has no entry and is decoded on a following answer
//!   ([`CanvasPictures::more`] says when to ask).
//! - **SVG from a plugin is untrusted**: it is drawn through the host's [`SvgDraw`] (konoma: a
//!   supervised child process), except where the host believes the plugin's `trusted` claim
//!   ([`CanvasPolicy::trust_own_svg`]), when it is drawn in this process.
//! - **Files**: an `ImageData::File` names a path the host opens for the plugin; only a regular
//!   file is opened (never a FIFO or a device, which would block the worker or read the user's
//!   keystrokes), and for a plugin that is not a standard one only inside the read scope
//!   ([`files`]).
//! - **Animation** (`ImageData::Frames`, and a GIF given as a file or as bytes): each frame of
//!   `Frames` is shown for at least [`MIN_FRAME_DELAY`] (a GIF's own rule), and the decoded
//!   frames are bounded by the inline GIF byte budget.
//! - **Bounds** (canvas pixels cannot be rebuilt once dropped, so a host never evicts them by the
//!   cache budget; these are what keep them bounded):
//!   - the pictures a canvas holds *in total*, over all its ids, are at most
//!     [`MAX_CANVAS_HELD_BYTES`] of decoded pixels. A picture that would not fit is refused for
//!     now (its cells show) and tried again on a later answer;
//!   - one answer adds at most [`MAX_CANVAS_BYTES`] of pixels, decodes at most
//!     [`MAX_DECODES_PER_ANSWER`] new pictures, reads at most [`MAX_INPUT_BYTES_PER_ANSWER`] of
//!     their input and starts at most [`MAX_HEADER_PIXELS_PER_ANSWER`] of header-announced work;
//!     the rest waits for the next answer;
//!   - one canvas names at most [`MAX_CANVAS_IMAGES`] distinct ids (the API refuses more; this is
//!     the second line).
//!
//! What the decoders, size caps and frame delays are is fixed here (they decide how a picture
//! looks); what differs between hosts is the [`CanvasPolicy`]: the read scope, the limits (tests use
//! small ones), the cancellation and the SVG drawer.
//!
//! The overlay arithmetic (where a picture lands in the content area, the signature of what is
//! where) is in [`overlay`].

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use konoma_svg::image::ImageFailure;

use crate::view::{CellRect, ImageData};

mod budget;
mod decode;
mod draw;
#[cfg(feature = "config")]
mod failure_msg;
pub mod files;
mod hash;
mod host;
pub mod overlay;
mod picture_view;
mod policy;
mod sharp_zoom;
mod worker;

#[cfg(any(test, feature = "test-hooks"))]
pub use budget::header_pixels;
pub use budget::{decoded_bytes, source_bytes};
#[cfg(any(test, feature = "test-hooks"))]
pub use decode::decode_one;
pub use decode::draw_plugin_svg;
pub use draw::draw_images;
#[cfg(feature = "config")]
pub use failure_msg::{
    image_failure_for, image_failure_message, image_failure_msg, plain_failure_text, svg_fail_msg,
    unavailable_text,
};
#[cfg(any(test, feature = "test-hooks"))]
pub use hash::content_hash;
pub use host::{CanvasPictureHost, Landed};
pub use picture_view::{decode_picture_view, PictureFrames, PictureView};
pub use policy::{CanvasPolicy, SvgDraw, DEFAULT_SVG_MAX_PX};
pub use sharp_zoom::{redraw_svg, sharp_zoom_target, sharp_zoom_target_for, SHARP_MAX_SIDE};
pub use worker::decode_canvas;
#[cfg(any(test, feature = "test-hooks"))]
pub use worker::seen_for_tests;

/// How many distinct pictures (ids) of one canvas the host looks at.
pub const MAX_CANVAS_IMAGES: usize = 256;
/// How many bytes of decoded pixels one answer of the plugin may add to the cache; past it the
/// rest of the answer waits for the next one.
pub const MAX_CANVAS_BYTES: u64 = 128 * 1024 * 1024;
/// How many bytes of decoded pixels all the pictures of the canvas may hold together.
pub const MAX_CANVAS_HELD_BYTES: u64 = 256 * 1024 * 1024;
/// How many bytes of picture *source* data (encoded bytes, SVG, raw pixels) the host keeps for the
/// pictures it has not turned into cache entries yet ([`KeptPicture`]), over all the ids of the
/// canvas. Past it the oldest kept source is dropped: its id shows its cells until the plugin sends
/// the picture again.
pub const MAX_CANVAS_KEPT_BYTES: u64 = 256 * 1024 * 1024;
/// How many pictures one answer decodes (the ones the cache does not hold); the rest waits.
pub const MAX_DECODES_PER_ANSWER: usize = 32;
/// How many bytes of picture input (encoded bytes, SVG, raw pixels, a file's size) one answer
/// decodes; the rest waits. A first picture is always decoded, however large, so an answer makes
/// progress.
pub const MAX_INPUT_BYTES_PER_ANSWER: u64 = 64 * 1024 * 1024;
/// How many pixels of work, **read from the headers before any decode** (width x height of each
/// picture, times the frames of a GIF), one answer may start. A picture is decoded at full size and
/// only then shrunk to the cache's bound, so the work of a decode follows the header, not the
/// shrunk result an answer used to be counted by (`MAX_CANVAS_BYTES`): a 1.5e8-pixel PNG of a few
/// KB costs the decoder about a second and ends up a few MB. The number is the one the GIF decoder
/// is calibrated with (`MAX_GIF_WORK_PIXELS`, 12-25 ns a pixel: the worst answer is 5-10 s on the
/// worker); it is more than twice the largest still image (`MAX_IMAGE_PIXELS`), so an ordinary
/// answer (a few screenfuls of pictures) never meets it. As for the other work limits, the first
/// picture of an answer is decoded whatever it costs (an answer makes progress); a picture that
/// does not fit next to what the answer has already started waits for the next answer.
pub const MAX_HEADER_PIXELS_PER_ANSWER: u64 = 400_000_000;
/// The shortest time a frame of an animation given as `ImageData::Frames` is shown (the GIF
/// rule, so an animation cannot ask for a re-encode every few milliseconds).
pub const MIN_FRAME_DELAY: Duration = Duration::from_millis(20);

/// The limits of the canvas pictures (the constants above; a test uses small ones).
#[derive(Clone, Copy, Debug)]
pub struct CanvasLimits {
    /// [`MAX_CANVAS_HELD_BYTES`]
    pub held_bytes: u64,
    /// [`MAX_CANVAS_BYTES`]
    pub answer_bytes: u64,
    /// [`MAX_DECODES_PER_ANSWER`]
    pub decodes: usize,
    /// [`MAX_INPUT_BYTES_PER_ANSWER`]
    pub input_bytes: u64,
    /// [`MAX_HEADER_PIXELS_PER_ANSWER`]
    pub header_pixels: u64,
}

impl CanvasLimits {
    pub const PRODUCTION: CanvasLimits = CanvasLimits {
        held_bytes: MAX_CANVAS_HELD_BYTES,
        answer_bytes: MAX_CANVAS_BYTES,
        decodes: MAX_DECODES_PER_ANSWER,
        input_bytes: MAX_INPUT_BYTES_PER_ANSWER,
        header_pixels: MAX_HEADER_PIXELS_PER_ANSWER,
    };
}

/// What the worker made of one picture.
pub enum CanvasDecoded {
    /// A still picture.
    Still(image::DynamicImage),
    /// An animation (at least one frame; the first is shown first).
    Frames(Vec<(image::DynamicImage, Duration)>),
    /// It could not be shown, with the reason.
    Failed(ImageFailure),
}

/// One placement of a canvas, as the host keeps it between the answer and the frames: what is
/// drawn where, with the cache key made once when the canvas landed (the frame loop allocates
/// nothing for it).
#[derive(Clone, Debug)]
pub struct CanvasPlace {
    /// The plugin's name of the picture.
    pub id: u64,
    /// The cells it covers in the canvas.
    pub rect: CellRect,
    /// The cache key of the content the plugin named for it; `None` when the plugin sent
    /// `unchanged` for an id the host does not hold (a failed placement).
    pub key: Option<Arc<str>>,
}

/// The pictures of a canvas, decoded on the worker.
pub struct CanvasPictures {
    /// The identity of the read scope these keys were made under ([`crate::ReadScope::fingerprint`]); `0` for no limit. The app remembers it so a job under another scope does
    /// not resolve an `unchanged` picture to a key judged under the old one.
    pub scope_fp: u64,
    /// The placements of the canvas, in the order of `CanvasView::images`.
    pub places: Vec<CanvasPlace>,
    /// The pictures decoded for this answer (the keys the cache did not hold yet), each once.
    pub fresh: Vec<(PathBuf, CanvasDecoded)>,
    /// Pictures left for a following answer because of the work limits of this one.
    pub waiting: usize,
    /// Pictures decoded but refused for now because they would not fit the memory bound.
    pub refused: usize,
    /// Another answer is worth asking for: pictures wait and this answer made progress (so the
    /// next one does too, and the asking ends), or pictures were left unhashed (the next answer
    /// takes the kept ones first, so it moves on whatever this one did). Pictures refused for the
    /// memory bound do not come back by asking (the landing asks when room grew for one).
    pub more: bool,
    /// The source of every picture this answer had to decode or defer, by id (`Arc` clones, no
    /// copy of the data). The app keeps those that are still not in the cache after landing
    /// ([`KeptPicture`]), so the plugin never has to send them again.
    pub sources: Vec<(u64, KeptPicture)>,
}

/// The source of a picture the host has not turned into a cache entry yet because the work limits
/// of an answer left it for later, or the memory bound refused it for now. The host keeps it (per
/// canvas id) and decodes it on a later job even when the later answer says `unchanged`.
#[derive(Clone, Debug)]
pub struct KeptPicture {
    /// The cache key of the content; `None` when it was not even hashed yet (the per-answer input
    /// budget left it).
    pub key: Option<Arc<str>>,
    /// The content hash (the part of the key), when it was made.
    pub hash: Option<u64>,
    /// What the plugin sent. Shared: a copy of the table (one per job sent to the worker) and the
    /// table itself hold the same data, and a `Frames` of thousands of frames is not copied for
    /// each job or freed on the UI thread by every landing.
    pub data: Arc<ImageData>,
    /// The decoded bytes it needs when the memory bound refused it: it is not decoded again until
    /// that much room is free.
    pub needs: Option<u64>,
}

/// What the app tells the worker about the canvas pictures it holds.
#[derive(Clone, Debug, Default)]
pub struct CanvasHeld {
    /// The cache keys held, with the decoded bytes of each.
    pub keys: HashMap<PathBuf, u64>,
    /// The key the previous answer named for each id (what `unchanged` resolves to).
    pub by_id: HashMap<u64, Arc<str>>,
    /// The entry each id is drawn from now (it stays while a newer content has no protocol).
    pub front: HashMap<u64, Arc<str>>,
    /// The pictures deferred or refused earlier, by id (what `unchanged` resolves to for them).
    pub kept: HashMap<u64, KeptPicture>,
}

/// The hash part of a canvas cache key: the content hash with the identity of the read scope
/// ([`crate::ReadScope::fingerprint`]) mixed in. An involution (`mix_scope(mix_scope(h, f),
/// f) == h`), and the identity for no limit (`f == 0`).
pub fn mix_scope(content_hash: u64, scope_fp: u64) -> u64 {
    content_hash ^ scope_fp.rotate_left(17)
}

#[cfg(test)]
mod tests;
