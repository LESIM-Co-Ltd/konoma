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

/// The kitty image ids of canvas pictures that are on the terminal now, **process-wide**: the
/// places that end the program (the normal exit, the panic hook, the signal thread) have no `App`
/// to ask, and an image left in the terminal's storage outlives the program (it is counted
/// against the terminal's quota until the terminal evicts something). Ids are noted when a canvas
/// picture is given one (`md_media::kitty_id_for`) and forgotten when their delete is handed out
/// (`App::take_kitty_deletes`).
pub(crate) struct LiveKittyIds {
    ids: std::sync::Mutex<std::collections::BTreeSet<u32>>,
    tmux: std::sync::atomic::AtomicBool,
}

impl LiveKittyIds {
    pub(crate) const fn new() -> LiveKittyIds {
        LiveKittyIds {
            ids: std::sync::Mutex::new(std::collections::BTreeSet::new()),
            tmux: std::sync::atomic::AtomicBool::new(false),
        }
    }

    pub(crate) fn note(&self, id: u32) {
        self.ids
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id);
    }

    pub(crate) fn forget(&self, id: u32) {
        self.ids
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&id);
    }

    pub(crate) fn set_tmux(&self, tmux: bool) {
        self.tmux.store(tmux, std::sync::atomic::Ordering::SeqCst);
    }

    /// The ids noted now (for a test).
    #[cfg(test)]
    pub(crate) fn snapshot(&self) -> Vec<u32> {
        self.ids
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .copied()
            .collect()
    }

    /// The escapes that delete every id noted, and the table emptied. Never blocks on a lock that
    /// a panicking thread poisoned.
    pub(crate) fn take_delete_escapes(&self) -> Vec<String> {
        let tmux = self.tmux.load(std::sync::atomic::Ordering::SeqCst);
        std::mem::take(&mut *self.ids.lock().unwrap_or_else(|e| e.into_inner()))
            .into_iter()
            .map(|id| crate::preview::kitty::build_delete(id, tmux))
            .collect()
    }
}

/// The ids of the canvas pictures on the terminal now (see [`LiveKittyIds`]).
pub(crate) static LIVE_CANVAS_KITTY: LiveKittyIds = LiveKittyIds::new();

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

/// How many distinct pictures (ids) of one canvas the host looks at.
pub(super) const MAX_CANVAS_IMAGES: usize = 256;
/// How many bytes of decoded pixels one answer of the plugin may add to the cache; past it the
/// rest of the answer waits for the next one.
pub(super) const MAX_CANVAS_BYTES: u64 = 128 * 1024 * 1024;
/// How many bytes of decoded pixels all the pictures of the canvas may hold together.
pub(super) const MAX_CANVAS_HELD_BYTES: u64 = 256 * 1024 * 1024;
/// How many bytes of picture *source* data (encoded bytes, SVG, raw pixels) the host keeps for the
/// pictures it has not turned into cache entries yet ([`KeptPicture`]), over all the ids of the
/// canvas. Past it the oldest kept source is dropped: its id shows its cells until the plugin sends
/// the picture again.
pub(super) const MAX_CANVAS_KEPT_BYTES: u64 = 256 * 1024 * 1024;
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
    /// next one does too, and the asking ends), or pictures were left unhashed (the next answer
    /// takes the kept ones first, so it moves on whatever this one did). Pictures refused for the
    /// memory bound do not come back by asking (the landing asks when room grew for one).
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
    /// What the plugin sent. Shared: a copy of the table (one per job sent to the worker) and the
    /// table itself hold the same data, and a `Frames` of thousands of frames is not copied for
    /// each job or freed on the UI thread by every landing.
    pub(super) data: Arc<ImageData>,
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
    /// The file being previewed: a relative picture path is relative to its directory.
    shown: Option<&'a Path>,
}

impl<'a> Hasher<'a> {
    fn new(cancel: Option<&'a crate::preview::office::Cancel>) -> Hasher<'a> {
        Hasher {
            memo: HashMap::new(),
            bytes: 0,
            cancel,
            shown: None,
        }
    }

    /// Resolve the relative picture files of a plugin against `shown`'s directory.
    fn relative_to(mut self, shown: &'a Path) -> Hasher<'a> {
        self.shown = Some(shown);
        self
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
                let abs = self
                    .shown
                    .map_or_else(|| p.to_path_buf(), |shown| resolve_picture_path(shown, p));
                if let Ok(m) = std::fs::metadata(abs) {
                    use std::os::unix::fs::MetadataExt;
                    h.num(m.len());
                    let t = m
                        .modified()
                        .ok()
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map_or(0, |d| d.as_nanos() as u64);
                    h.num(t);
                    // The same material as the stamp of the shown file (`FileStamp`): a file
                    // replaced with its size and modification time kept (`cp -p`, `rsync -t`, a
                    // rename) differs in inode, and one rewritten in place with the time put
                    // back differs in change time.
                    h.num(m.dev());
                    h.num(m.ino());
                    h.num(m.ctime() as u64);
                    h.num(m.ctime_nsec() as u64);
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
            // A kind this host does not know (`ImageData` is non-exhaustive): hashed as one
            // value; decoding refuses it.
            _ => h.num(7),
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

/// The most bytes of a picture file the host reads for a plugin (`ImageData::File`).
pub(super) const MAX_PICTURE_FILE_BYTES: u64 = 64 * 1024 * 1024;

/// Where the picture file `p` of a plugin is: an absolute path is itself, a relative one is
/// relative to the directory of the file being previewed (`shown`), never to the directory
/// konoma happens to run in.
pub(super) fn resolve_picture_path(shown: &Path, p: &Path) -> PathBuf {
    // The previewed file itself (the standard image plugin hands its own path back) is not
    // joined onto its own folder.
    if p.is_absolute() || p == shown {
        return p.to_path_buf();
    }
    shown
        .parent()
        .map_or_else(|| p.to_path_buf(), |d| d.join(p))
}

/// Open the picture file at `abs` for reading, refusing anything but a regular file **without
/// opening it first** where that can be told from the path (`metadata`: a FIFO, a device or a
/// directory is never opened, so opening cannot have a side effect such as raising DTR on a
/// serial line or waking a FIFO's writer), and then deciding again on the handle that was
/// opened (`fstat`: a path checked first and opened afterwards could be swapped in between). The
/// file is opened without blocking (`O_NONBLOCK`) and without becoming a controlling terminal
/// (`O_NOCTTY`). The one door every path that opens a plugin's (or the previewed) picture file
/// goes through.
pub(super) fn open_regular_picture(
    abs: &Path,
) -> Result<std::fs::File, crate::preview::image::ImageFailure> {
    use crate::preview::image::ImageFailure;
    use std::os::unix::fs::OpenOptionsExt;
    match std::fs::metadata(abs) {
        Ok(m) if m.is_file() => {}
        _ => return Err(ImageFailure::Corrupt),
    }
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_NOCTTY)
        .open(abs)
        .map_err(|_| ImageFailure::Corrupt)?;
    match file.metadata() {
        Ok(m) if m.is_file() => Ok(file),
        _ => Err(ImageFailure::Corrupt),
    }
}

/// [`open_regular_picture`] for the file `p` a plugin named, resolved against `shown`
/// ([`resolve_picture_path`]): the handle and the resolved path.
pub(super) fn open_picture_file(
    shown: &Path,
    p: &Path,
) -> Result<(std::fs::File, PathBuf), crate::preview::image::ImageFailure> {
    let abs = resolve_picture_path(shown, p);
    let file = open_regular_picture(&abs)?;
    Ok((file, abs))
}

/// Read the picture file `p` (resolved against `shown`, see [`resolve_picture_path`]) that a plugin
/// named, and say what it holds as data the decoders take: an SVG (by its first bytes) drawn
/// from the directory it is in, anything else as encoded bytes.
///
/// **Only a regular file is read, and it is decided on the file that was opened**, not on the path:
/// the file is opened without blocking (`O_NONBLOCK`: opening a FIFO that nobody writes to would
/// hold the worker for good, and a cancellation cannot reach it) and without becoming a
/// controlling terminal, and `fstat` of the opened handle must say "regular file" (a device such
/// as `/dev/tty` would read the user's keystrokes). A path checked first and opened afterwards
/// could be swapped for a FIFO in between. The bytes are read from the handle and decoded from
/// memory, so nothing opens the path a second time. Any file the user can read is read -- konoma
/// shows what the user could `cat` -- up to [`MAX_PICTURE_FILE_BYTES`].
fn read_picture_file(
    shown: &Path,
    p: &Path,
) -> Result<ImageData, crate::preview::image::ImageFailure> {
    use crate::preview::image::ImageFailure;
    use std::io::Read;
    let (file, abs) = open_picture_file(shown, p)?;
    let meta = file.metadata().map_err(|_| ImageFailure::Corrupt)?;
    if meta.len() > MAX_PICTURE_FILE_BYTES {
        return Err(ImageFailure::TooLarge);
    }
    let mut bytes = Vec::with_capacity(meta.len() as usize);
    file.take(MAX_PICTURE_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| ImageFailure::Corrupt)?;
    if bytes.len() as u64 > MAX_PICTURE_FILE_BYTES {
        return Err(ImageFailure::TooLarge);
    }
    if crate::preview::svg::can_begin_svg(&bytes[..bytes.len().min(512)]) {
        return Ok(ImageData::Svg {
            data: Arc::from(bytes),
            base_dir: Some(
                abs.parent()
                    .map_or_else(|| PathBuf::from("/"), Path::to_path_buf),
            ),
            trusted: false,
        });
    }
    Ok(ImageData::Encoded(Arc::from(bytes)))
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
    match data {
        ImageData::File(p) => {
            let read = read_picture_file(path, p)?;
            decode_still(&read, path, default_raster_px, cancel, trust_own_svg)
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
        _ => Err(ImageFailure::UnsupportedFormat),
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
    // A file is read once, from the handle that was opened (`read_picture_file`), and decoded from
    // memory: a GIF file is an animation like GIF bytes are.
    let read;
    let data = if let ImageData::File(p) = data {
        match read_picture_file(path, p) {
            Ok(d) => {
                read = d;
                &read
            }
            Err(why) => return CanvasDecoded::Failed(why),
        }
    } else {
        data
    };
    let animated = match data {
        // A GIF is an animation when it has more than one frame; anything else (a single frame,
        // a damaged one) falls through to the still decode, as an inline Markdown image does.
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

/// How many decodes the picture `data` costs the budget of an answer. An animation given as
/// `ImageData::Frames` costs one per frame that runs a decoder: an SVG (drawn in a child
/// process), a file (read, and possibly an SVG) or an `Encoded` image (a full decode of whatever
/// size its header admits, however small its bytes); only a ready `Rgba` frame costs nothing
/// (bounded by the input budget). Any picture costs at least one.
fn decode_cost(data: &ImageData) -> usize {
    fn frame(d: &ImageData) -> usize {
        match d {
            ImageData::Svg { .. } | ImageData::File(_) | ImageData::Encoded(_) => 1,
            ImageData::Frames(f) => f.iter().map(|(d, _)| frame(d)).sum(),
            _ => 0,
        }
    }
    match data {
        ImageData::Frames(f) => f.iter().map(|(d, _)| frame(d)).sum::<usize>().max(1),
        _ => 1,
    }
}

/// The bytes of input a decode of `data` reads (what the per-answer input budget counts).
fn input_bytes(data: &ImageData, shown: &Path) -> u64 {
    match data {
        ImageData::File(p) => {
            std::fs::metadata(resolve_picture_path(shown, p)).map_or(0, |m| m.len())
        }
        ImageData::Encoded(b) => b.len() as u64,
        ImageData::Svg { data, .. } => data.len() as u64,
        ImageData::Rgba { pixels, .. } => pixels.len() as u64,
        ImageData::Frames(f) => f.iter().map(|(d, _)| input_bytes(d, shown)).sum(),
        ImageData::Lazy(_) => 0,
        _ => 0,
    }
}

/// The bytes of memory the source `data` of a picture holds (a file is not held: it is read when
/// decoded).
fn source_bytes(data: &ImageData) -> u64 {
    match data {
        ImageData::File(_) | ImageData::Lazy(_) => 0,
        ImageData::Encoded(b) => b.len() as u64,
        ImageData::Svg { data, .. } => data.len() as u64,
        ImageData::Rgba { pixels, .. } => pixels.len() as u64,
        ImageData::Frames(f) => f.iter().map(|(d, _)| source_bytes(d)).sum(),
        // A kind this host does not know (`ImageData` is non-exhaustive).
        _ => 0,
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
    let mut hasher = Hasher::new(cancel).relative_to(path);
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
        let (data, shared): (&ImageData, Option<&Arc<ImageData>>) = match (&im.image, kept) {
            (Some(d), _) => (d, None),
            (None, Some(k)) => (&*k.data, Some(&k.data)),
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
                            data: shared.cloned().unwrap_or_else(|| Arc::new(data.clone())),
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
                data: shared.cloned().unwrap_or_else(|| Arc::new(data.clone())),
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
    for (id, key) in &key_of {
        if let Some(f) = held.front.get(id) {
            // The entry an id is drawn from is replaced by its new content when that lands (the
            // landing frees it for the room, `canvas_pictures_landed_within`), so it is not extra
            // held next to the new content. One whose new content is already in the cache stays
            // counted: the cache holds both until the next landing's prune.
            let being_replaced = **f != **key && !held.keys.contains_key(Path::new(&**key));
            if !being_replaced {
                kept.insert(&**f);
            }
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
        } else if matches!(t.data.as_ref(), ImageData::Frames(_))
            && (decode_cost(&t.data) > limits.decodes
                || input_bytes(&t.data, path) > limits.input_bytes)
        {
            // An animation that alone costs more than one answer may do (more SVG / file frames than
            // the decode budget, more input than the input budget; only ready `Rgba` frames are not counted) can never be decoded: it is
            // refused for good (the cells stay) instead of being decoded without a bound as "the
            // first picture of an answer" (5000 SVG frames are 5000 child processes). Every animation that is not refused here costs at most the budget
            // by itself, so the first picture of an answer always fits and an answer makes
            // progress, the work of one answer bounded by the budgets whatever order the
            // pictures come in. A single image is the exception that keeps an answer moving: it
            // is decoded even when it is larger than the input budget, bounded by its own limits
            // (64 MiB of file, the decoders' size limits).
            CanvasDecoded::Failed(ImageFailure::TooLarge)
        } else if decodes >= limits.decodes
            // Every frame of an animation is a decode (a picture of N frames costs N): one that
            // does not fit next to what this answer decoded waits (kept) for a later answer, where
            // it may come first.
            || decodes + decode_cost(&t.data) > limits.decodes
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
            decodes += decode_cost(&t.data);
            input += input_bytes(&t.data, path);
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
    // Asking again helps when the work limits left pictures and this answer made progress, or when
    // pictures were not even hashed (the input budget): the host keeps those first in line, so the
    // next answer always moves on, whether or not this one decoded anything.
    let more = (waiting > 0 && !fresh.is_empty()) || unhashed > 0;
    CanvasPictures {
        places,
        fresh,
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
    data: Arc<ImageData>,
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
        let unhashed_pending = pics.sources.iter().any(|(_, k)| k.key.is_none());
        let before = std::mem::take(&mut self.canvas_kept);
        let mut kept: Vec<(u64, KeptPicture)> = pics
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
        self.md_image_cache.retain(|k, _| {
            !crate::preview::markdown::is_canvas_url(&k.to_string_lossy()) || live.contains(k)
        });
        let cache = &self.md_image_cache;
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
        for id in &ids {
            LIVE_CANVAS_KITTY.forget(*id);
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
