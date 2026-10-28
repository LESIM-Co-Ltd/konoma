//! The inline-image cache: decoded pictures, their per-size encoded protocols, the kitty image ids
//! pinned to those encodes, and the encode worker.
//!
//! Markdown pictures and a plugin canvas's pictures ride on this one machinery, in konoma and in the
//! standalone plugin app alike, so a picture looks the same in both. The cache holds no host
//! policy: where pixels come from (files, SVG, Office documents, downloads), what a failure is
//! called and when a document changes stay with the host, which fills `MdImgEntry` through its
//! public fields and asks this module for the rest (encode requests, drawable protocols, frame
//! stepping, kitty deletes).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use ratatui::layout::Rect;
use ratatui_image::picker::Picker;
use ratatui_image::protocol::Protocol;
use ratatui_image::{FilterType, Resize};

use crate::layout::PxRect;

/// Who vouches for a retained SVG, and so how it is drawn again (sharp zoom, rebuilding evicted
/// pixels). Decided by the host from where the SVG came from, never by what a plugin claims. How
/// each kind is drawn is the host's business (it owns the supervised child process).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SvgTrust {
    /// Made by the host's own code (a mermaid diagram, a formula): drawn in this process. The path
    /// is the base for relative resources (the diagrams have none).
    Trusted(PathBuf),
    /// From a file: drawn by the supervised child process, reading files relative to this path.
    File(PathBuf),
    /// From a file of a plugin the user added: like `File`, but every file the drawing reads must
    /// be inside these real directories (the plugin's read scope), whatever the SVG references.
    FileWithin(PathBuf, Arc<[PathBuf]>),
    /// From a document, a plugin or git, with no file to be relative to: drawn by the supervised
    /// child process, which reads no file.
    Embedded,
}

impl Default for SvgTrust {
    /// The safe answer for an entry whose origin was never stated: no file read, child process.
    fn default() -> Self {
        SvgTrust::Embedded
    }
}

/// One drawable inline Markdown image: either konoma's own compressed kitty transmit or
/// ratatui-image's protocol for every other graphics backend.
///
/// The split exists because a kitty image is **addressable**: it carries an id, and re-transmitting
/// that id replaces the picture instead of adding a second one. Everything else (sixel, iTerm2,
/// halfblocks) is written as cell content — the terminal keeps nothing between frames — so those
/// backends can keep re-encoding freely and stay on the ratatui-image path unchanged.
#[derive(Clone)]
pub enum InlineImage {
    /// konoma's zlib-compressed (`o=z`) transmit + Unicode placeholders, on a fixed image id.
    Kitty(crate::kitty::KittyImage),
    /// ratatui-image's fixed-size protocol (sixel / iTerm2 / halfblocks).
    Proto(Protocol),
}

impl InlineImage {
    /// Draw into `area`. Both arms refuse to draw when the encoded image is **larger** than the
    /// area, matching `ratatui_image::Image`'s no-clipping rule: while a freshly scrolled band is
    /// still encoding, the renderer deliberately hands the previous (taller) encode to this
    /// function, and drawing it clipped would show the wrong slice of the picture rather than
    /// simply nothing for one frame. The full-screen path keeps its own clamping behavior — this
    /// rule is about matching what the inline path did before, not about `KittyImage` in general.
    pub fn render(&self, area: Rect, buf: &mut ratatui::buffer::Buffer) {
        match self {
            Self::Kitty(ki) => {
                let (cols, rows) = ki.cell_size();
                if cols > area.width || rows > area.height {
                    return;
                }
                ki.render(area, buf);
            }
            Self::Proto(p) => {
                ratatui::widgets::Widget::render(ratatui_image::Image::new(p), area, buf)
            }
        }
    }

    /// Whether this went through konoma's compressed kitty transmit. Test-only: it is the one
    /// difference the two arms are allowed to have, so a test can pin which backend a terminal got.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn is_kitty(&self) -> bool {
        matches!(self, Self::Kitty(_))
    }

    /// The encoded size in cells. Test-only, and deliberately readable from *both* arms: the kitty
    /// arm reimplements `Picker::new_protocol`'s sizing, so the way to keep it honest is to run the
    /// same request through both and compare this.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn cell_size(&self) -> (u16, u16) {
        match self {
            Self::Kitty(ki) => ki.cell_size(),
            Self::Proto(p) => {
                let s = p.size();
                (s.width, s.height)
            }
        }
    }

    /// Bytes of this image's one-shot transmit escape (0 for a protocol that is re-sent as cell
    /// content every frame). Test-only, to state the compression win in real numbers.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn transmit_len(&self) -> usize {
        match self {
            Self::Kitty(ki) => ki.transmit_len(),
            Self::Proto(_) => 0,
        }
    }
}

/// How many distinct encode keys of **one picture** konoma keeps encoded protocols (and kitty image
/// ids) for, within one protocol family.
///
/// The same file can be on screen at several sizes at once: a block image is laid out at the page
/// width while a copy inside a table cell is shaved to that column's width, and one document can
/// hold several tables whose columns are all different widths. Holding a single encode per picture
/// is what made those placements fight — each one saw the other's size recorded, asked for its own,
/// and the answer overwrote the first — so `draw → request → apply → draw → request → …` never
/// stopped, with nobody touching the keyboard (measured at 40 columns: 0.31s of CPU across 30 idle
/// seconds, and the losing placement never drew a picture at all).
///
/// The number is a **soft floor, not a hard ceiling**: `reserve_proto_slot` never recycles a slot
/// the frame being drawn right now is using, and grows the family instead when every slot is live.
/// That is what makes convergence unconditional — a document showing one picture at more
/// simultaneous sizes than this keeps all of them rather than thrashing between them — and it is
/// also what bounds the memory, because sizes that are on screen *at the same moment* occupy
/// disjoint parts of the viewport: their cell areas sum to at most one screenful however many they
/// are, and a protocol's bytes track its cell area.
///
/// So the constant only decides how many **off-screen** encodes stay warm, i.e. how much scrolling
/// back (or dragging a pane edge back to a previous width) redraws instantly instead of re-encoding.
/// Three covers the shapes that recur in practice — the page width plus two table column widths, or
/// one width plus the previous one still lingering right after a resize — without paying for more.
pub const MD_PROTO_SLOTS: usize = 3;

/// Slots for the zoom family. Only one inline diagram can be focused at a time (`Tab` moves the
/// focus, and the host only asks for the zoom of the focused fence), so one crop is all
/// that is ever *wanted* — but two are kept, because a slot is claimed when its encode is requested
/// and every `+`/`-` step asks for a different crop. With one slot the step would blank the crop on
/// screen and fall back to the unzoomed diagram until the new one landed; with two, the previous
/// crop stays visible right up to the moment it is replaced, and the pair simply alternates.
pub const MD_ZOOM_SLOTS: usize = 2;

/// One encoded protocol of an inline image, together with the exact request it answers.
///
/// A slot's **position** in its family is its identity: on a kitty terminal the image id at the same
/// index of `InlineImageCache::kitty_ids` is pinned to it, so re-encoding into a slot hands the terminal a
/// *replacement* rather than an additional picture — the property that keeps an animated GIF, and a
/// document reopened all day long, at a constant cost in terminal image storage.
pub struct MdProtoSlot {
    /// The request this slot answers. Re-keying a slot is how it gets recycled, and is exactly when
    /// its pinned kitty id starts holding a different picture.
    pub key: MdEncodeKey,
    /// None = the encode has been requested but has not landed yet, **or** it failed. Either way the
    /// key stays recorded, which is what stops the renderer from asking again on every frame.
    pub image: Option<InlineImage>,
    /// `InlineImageCache::frame` at the last render pass that asked for this key. Slots the current pass is
    /// drawing from are never recycled — see `MD_PROTO_SLOTS`.
    pub used: u64,
    /// The stored image belongs to a superseded animation frame: keep drawing it (clearing it would
    /// flash a blank), but re-encode into this same slot so the id — and the terminal's picture
    /// under it — is replaced rather than duplicated.
    pub stale: bool,
}

/// A decoded inline Markdown image plus its background-encoded render protocol(s).
#[derive(Default)]
pub struct MdImgEntry {
    /// The decoded source image (None while the background decode is in flight). Shared with the encode
    /// worker via `Arc` (cheap clone) so cropping/encoding happens off the UI thread. Deliberately
    /// **one per picture, not one per size**: the protocols below are what a size costs, while the
    /// RGBA source is the big allocation and every size crops/resizes from the same one.
    pub decoded: Option<Arc<image::DynamicImage>>,
    /// Graphics protocols for the fully-visible image, one slot per (cols, rows) cell box it is
    /// drawn at. See `MD_PROTO_SLOTS` for why this is a list and not a single encode.
    pub full: Vec<MdProtoSlot>,
    /// Graphics protocols for a partially-scrolled image: only the visible vertical band, cropped
    /// and encoded so the image renders clipped instead of being hidden. One slot per
    /// (cols, full_rows, row_off, vis_rows) band — a list for the same reason `full` is one, since
    /// two placements of the same picture can be cut by the top and the bottom viewport edge at the
    /// same time. A family of its own because the renderer keeps drawing the full-image encode until
    /// a freshly scrolled band lands: the two are on screen across consecutive frames, so a band's
    /// transmit must not land on the id the full image is still drawn from.
    pub clip: Vec<MdProtoSlot>,
    /// An encode request is in flight on the worker thread (at most one per image, so scrolling does not
    /// queue a backlog — when it returns, the next render requests the then-current band).
    pub enc_inflight: bool,
    /// Decode or encode failed — do not retry; the placeholder/text fallback stays visible.
    pub failed: bool,
    /// In-place zoom protocols for a **focused inline mermaid diagram** (`+`/`-` while focused):
    /// a crop of the source encoded into the same (cols, rows) cell area (the layout never moves).
    /// Capped at `MD_ZOOM_SLOTS`.
    pub zoom: Vec<MdProtoSlot>,
    /// SVG source of a rendered mermaid fence — kept so in-place zoom can re-rasterize at a higher
    /// density (sharp zoom, same mechanism as the full-screen vector zoom).
    pub svg: Option<std::sync::Arc<Vec<u8>>>,
    /// Where `svg` came from, and so how it is drawn again (`SvgTrust`): konoma's own in this
    /// process, a file's by the supervised child process with the file's directory as the base
    /// for relative references (as the first drawing did), anything else by the child process
    /// reading no file. Set where the entry is made.
    pub svg_trust: SvgTrust,
    /// Layout size, fixed at the **first** result so a later sharpening re-raster never changes the
    /// reserved area (the user-visible size is fixed). For a mermaid fence or a math expression
    /// (vector-backed, synthetic key), this is the source SVG's **intrinsic size** (px user units,
    /// the same domain the renderer laid glyphs out in — see `mermaid_cells`/`math_cells`), not the
    /// raster's pixel dimensions (those are scaled by `[ui] svg_max_px`, an unrelated setting).
    /// Regular images use the raster's own dimensions, since there is no separate intrinsic size.
    pub layout_px: Option<(u32, u32)>,
    /// A sharpening re-raster is in flight (at most one per diagram).
    pub reraster_inflight: bool,
    /// The pixels changed (a re-raster landed, a GIF advanced) while an encode of the **previous**
    /// pixels was still running. That encode holds the old picture, so when it lands its slot must
    /// stay stale and be encoded again from the current pixels: clearing `stale` there froze the
    /// picture at the old density for good (a bottom band cut from the 800 px raster instead of
    /// the sharpened one, depending only on which thread finished first).
    pub outdated_encode_in_flight: bool,
    /// All frames of an animated inline GIF (empty = not a GIF, or a static/single-frame one — those
    /// already decode through the plain `decoded` path above and never populate this). Kept as `Arc`s
    /// so advancing to the next frame (`InlineImageCache::advance_gifs_if_due`) is a cheap pointer clone into
    /// `decoded` rather than a full image copy — mirrors the full-screen `GifPlayer` mechanism
    /// but per cache entry, since several inline GIFs can animate at once.
    pub frames: Vec<(Arc<image::DynamicImage>, std::time::Duration)>,
    /// Index of the currently-displayed frame within `frames`.
    pub idx: usize,
    /// The time the current frame began showing (mirrors the player's `shown_at`). None = before the
    /// first tick (frame 0 is already shown via `decoded`; timing starts on the next tick).
    pub shown_at: Option<std::time::Instant>,
    /// `InlineImageCache::frame` of the last overlay pass that asked for this picture. The cache evicts the
    /// pixels of the pictures asked for longest ago first (the host's eviction pass; the cache itself never evicts).
    pub last_used: u64,
    /// The decoded pixels were dropped to stay within `MD_IMAGE_CACHE_BYTES`. The entry itself stays
    /// (its layout size, SVG source, kitty ids and any finished encodes), so nothing on the page
    /// moves; the pixels are rebuilt from the file / SVG the next time a placement needs them.
    pub evicted: bool,
    /// A rebuild of the evicted pixels is running (the host started it, its result
    /// has not landed). Together with `evicted` this is the whole life of a rebuild: waiting
    /// (`evicted`, not `rebuilding`) -> running (`rebuilding`) -> back (`!evicted`) or `failed`.
    pub rebuilding: bool,
    /// A placement asked for the evicted pixels but the rebuild limit (`MD_MAX_REBUILDS`) was
    /// reached: ask again on the next frame, and keep the loading indicator (and so the redraw
    /// loop) alive until it starts. It holds the host's draw-pass number of the frame that asked and
    /// counts only while that is the last frame drawn, so a wish nobody renews (the picture scrolled out
    /// of view, the preview was left) lapses by itself instead of keeping the loop ticking.
    pub rebuild_wanted: Option<u64>,
    /// For a slide picture: the longest edge, in pixels, its frame needs (0 = not asked yet). The
    /// first decode, a rebuild of evicted pixels and the re-raster of a frame that outgrew the
    /// raster all draw at this size (set by the host).
    pub want_px: u32,
    /// Why the decode failed, for the text that replaces the picture (None = no specific reason).
    pub fail: Option<konoma_svg::image::ImageFailure>,
    /// This entry's place in the decode queue: its priority is the last overlay pass that asked for
    /// the picture, and dropping the entry (document closed, file changed) withdraws a decode that
    /// has not run yet.
    pub wish: konoma_svg::image::DecodeWish,
    /// Id of the one background decode / rebuild / re-raster that is still wanted for this entry
    /// (assigned by the host when it starts one; 0 = none was ever started, as for an entry placed by hand in a
    /// test). A result carrying any other id belongs to an earlier life of the same path (the entry
    /// was dropped, then asked for again) and is ignored by the host when it applies the result, so a stale
    /// "cancelled" cannot forget the new entry and a stale picture cannot fill it. At most one
    /// request runs per entry, so this is also the identity of the in-flight one.
    pub request: u64,
    /// Id of the first request ever started for this entry, constant for its life: an encode
    /// result carries it so that one computed for a dropped entry is not applied to a new one of
    /// the same path (`request` cannot do this: a re-raster legitimately changes it while an
    /// encode of the previous pixels may still be running). 0 = never started.
    pub born: u64,
}

impl MdImgEntry {
    /// Bytes of decoded pixels this entry keeps resident (frames of an animated GIF count once;
    /// the first frame is shared with `decoded`).
    pub fn pixel_bytes(&self) -> u64 {
        use image::GenericImageView;
        // The picture's own pixel format (3 bytes for an RGB8, 8 for an RGBA16), not an assumed 4.
        let px = |im: &image::DynamicImage| {
            let (w, h) = im.dimensions();
            u64::from(w) * u64::from(h) * u64::from(im.color().bytes_per_pixel())
        };
        if self.frames.is_empty() {
            self.decoded.as_deref().map_or(0, px)
        } else {
            self.frames.iter().map(|(im, _)| px(im)).sum()
        }
    }

    /// Drop the decoded pixels (keeping everything that lets them be rebuilt).
    pub fn evict_pixels(&mut self) {
        self.decoded = None;
        self.frames.clear();
        self.idx = 0;
        self.shown_at = None;
        self.evicted = true;
    }

    /// The slot family a request belongs to (full / clip / zoom — `MdEncodeKey::slot`).
    pub fn slots(&self, key: &MdEncodeKey) -> &[MdProtoSlot] {
        match key.slot() {
            0 => &self.full,
            1 => &self.clip,
            _ => &self.zoom,
        }
    }

    pub fn slots_mut(&mut self, key: &MdEncodeKey) -> &mut Vec<MdProtoSlot> {
        match key.slot() {
            0 => &mut self.full,
            1 => &mut self.clip,
            _ => &mut self.zoom,
        }
    }

    /// Record that the pass numbered `frame` wants this exact request, and say whether it is
    /// already **settled** — encoded, or attempted and failed, so it must not be requested again.
    /// A `stale` slot is deliberately not settled: its picture belongs to an animation frame that
    /// has been superseded, so it needs re-encoding (into that same slot, keeping its id).
    ///
    /// Stamping happens even for a request that is not settled and cannot be sent yet (another
    /// encode holds the single in-flight ticket). That is the whole point: a placement the renderer
    /// is drawing right now must keep its slot out of the recycling pool while it waits its turn,
    /// or two placements of one picture take turns evicting each other and never converge.
    pub fn touch(&mut self, key: &MdEncodeKey, frame: u64) -> bool {
        match self.slots_mut(key).iter_mut().find(|s| s.key == *key) {
            Some(s) => {
                s.used = frame;
                !s.stale
            }
            None => false,
        }
    }

    /// The image already encoded for exactly this request, if there is one. `None` also covers "the
    /// slot exists but holds no image" (requested and still in flight, or failed) — the caller then
    /// falls back to something drawable rather than blanking the picture.
    pub fn proto(&self, key: &MdEncodeKey) -> Option<&InlineImage> {
        self.slots(key)
            .iter()
            .find(|s| s.key == *key)?
            .image
            .as_ref()
    }

    /// The most recently wanted slot carrying an actual image, among `order`'s families. This is
    /// what the renderer draws while the encode for the current position is still in flight, so
    /// scrolling — or a resize, or a second placement at another width claiming the encoder — never
    /// blinks an image out. Ties (both wanted by the same pass) go to the earlier family in `order`.
    pub fn newest_in<'a>(&'a self, order: &[&'a [MdProtoSlot]]) -> Option<&'a InlineImage> {
        let mut best: Option<&MdProtoSlot> = None;
        for fam in order {
            for s in fam.iter().filter(|s| s.image.is_some()) {
                if best.is_none_or(|b| s.used > b.used) {
                    best = Some(s);
                }
            }
        }
        best.and_then(|s| s.image.as_ref())
    }

    /// Fallback for a plain inline placement: the freshest band, else the freshest full image.
    pub fn newest_proto(&self) -> Option<&InlineImage> {
        self.newest_in(&[&self.clip, &self.full])
    }

    /// Fallback for the focused-diagram zoom: the freshest **unzoomed** full image (a zoom crop of
    /// some other cell area would be drawn at the wrong scale).
    pub fn newest_full(&self) -> Option<&InlineImage> {
        self.newest_in(&[&self.full])
    }

    /// The source pixels changed underneath every encode — a GIF advanced a frame, or a diagram was
    /// re-rasterized at a higher density. Mark each slot for re-encoding **without** dropping the
    /// picture it holds: the old one keeps drawing until the new one lands (clearing it would flash
    /// a blank), and re-encoding into the surviving slot reuses its kitty id, so the terminal
    /// replaces that picture instead of collecting one per frame.
    pub fn mark_stale(&mut self) {
        // The single in-flight encode (if any) was cut from the pixels that just changed.
        if self.enc_inflight {
            self.outdated_encode_in_flight = true;
        }
        for slot in self
            .full
            .iter_mut()
            .chain(self.clip.iter_mut())
            .chain(self.zoom.iter_mut())
        {
            slot.stale = true;
        }
    }
}

#[cfg(any(test, feature = "test-hooks"))]
impl MdProtoSlot {
    /// A slot holding a finished encode of `key` — the state [`InlineImageCache::apply_encode`] leaves behind.
    pub fn encoded(key: MdEncodeKey, image: InlineImage) -> Self {
        Self {
            key,
            image: Some(image),
            used: 0,
            stale: false,
        }
    }

    /// A slot reserved for `key` that holds no picture: the encode is either still in flight or it
    /// failed. Both look the same from here, and both mean "do not ask for this again".
    pub fn attempted(key: MdEncodeKey) -> Self {
        Self {
            key,
            image: None,
            used: 0,
            stale: false,
        }
    }
}

/// Choose the slot a fresh encode of `key` will be answered into, and return its index — which is
/// also the index of the kitty image id pinned to it (`InlineImageCache::kitty_ids`).
///
/// Preference order: the slot already holding this key (a re-encode — same id, so the terminal
/// *replaces* its picture instead of accumulating one), then a brand-new slot while the family is
/// still under `cap`, then the least-recently-wanted slot. A slot the pass numbered `frame` is
/// drawing from is never recycled: when every slot is live the family grows by one instead, which
/// is what makes convergence unconditional (see `MD_PROTO_SLOTS`).
pub fn reserve_proto_slot(
    slots: &mut Vec<MdProtoSlot>,
    key: MdEncodeKey,
    frame: u64,
    cap: usize,
) -> usize {
    reserve_proto_slot_inner(slots, key, frame, cap, false).expect("the soft family always grows")
}

/// [`reserve_proto_slot`] with a **hard** ceiling: when every slot of the family is live in this
/// frame (and the family is already at `cap`), there is no slot to give (`None`) instead of the
/// family growing. For a canvas picture, whose plugin is not trusted to place one picture at few
/// sizes: its encodes can never outgrow `MD_PROTO_SLOTS`. (An honest canvas places an id at one
/// size, `CanvasView::check`, so it never gets here.)
pub fn reserve_proto_slot_hard(
    slots: &mut Vec<MdProtoSlot>,
    key: MdEncodeKey,
    frame: u64,
    cap: usize,
) -> Option<usize> {
    reserve_proto_slot_inner(slots, key, frame, cap, true)
}

fn reserve_proto_slot_inner(
    slots: &mut Vec<MdProtoSlot>,
    key: MdEncodeKey,
    frame: u64,
    cap: usize,
    hard: bool,
) -> Option<usize> {
    if let Some(i) = slots.iter().position(|s| s.key == key) {
        slots[i].used = frame;
        return Some(i);
    }
    let fresh = MdProtoSlot {
        key,
        image: None,
        used: frame,
        stale: false,
    };
    if slots.len() >= cap {
        let lru = slots
            .iter()
            .enumerate()
            .filter(|(_, s)| s.used < frame)
            .min_by_key(|(i, s)| (s.used, *i))
            .map(|(i, _)| i);
        if let Some(i) = lru {
            slots[i] = fresh;
            return Some(i);
        }
        if hard {
            return None;
        }
    }
    slots.push(fresh);
    Some(slots.len() - 1)
}

/// Which protocol a background encode produces (so the result is stored in the right slot).
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum MdEncodeKey {
    /// The whole image at (cols, rows) cells.
    Full { cols: u16, rows: u16 },
    /// A cropped vertical band, keyed by (cols, full_rows, row_off, vis_rows).
    Clip {
        cols: u16,
        full_rows: u16,
        row_off: u16,
        vis_rows: u16,
    },
    /// An in-place zoom crop of a focused inline mermaid diagram, encoded into the same
    /// (cols, rows) area. Keyed by the pixel crop so pan/zoom/re-raster all re-encode naturally.
    Zoom { cols: u16, rows: u16, crop: PxRect },
}

impl MdEncodeKey {
    /// Which of `MdImgEntry`'s three protocol slots this result belongs in — and therefore which of
    /// its three fixed kitty ids the encode must be transmitted under.
    pub fn slot(&self) -> usize {
        match self {
            Self::Full { .. } => 0,
            Self::Clip { .. } => 1,
            Self::Zoom { .. } => 2,
        }
    }
}

/// A request to encode an inline image (or a cropped band of it) off the UI thread (principle #4).
pub struct MdEncodeRequest {
    pub path: PathBuf,
    pub key: MdEncodeKey,
    pub image: Arc<image::DynamicImage>,
    /// Pixel band to crop before encoding; None encodes the whole image.
    pub crop: Option<PxRect>,
    pub cols: u16,
    pub rows: u16,
    /// `Some((id, is_tmux))` on a kitty terminal: build konoma's own compressed transmit under this
    /// slot's **fixed** id (see `InlineImageCache::kitty_ids`). None everywhere else = ratatui-image.
    pub kitty: Option<(u32, bool)>,
    /// `MdImgEntry::born` of the entry the request was made for.
    pub born: u64,
}

/// Result of a background inline-image encode, delivered to the run loop.
pub struct MdEncodeResult {
    pub path: PathBuf,
    pub key: MdEncodeKey,
    /// None = the encode failed. The worker **always** sends a result: swallowing failures left
    /// `enc_inflight` latched on, freezing all re-encodes of that image and keeping
    /// the host's "pictures are loading" state true forever (busy spinner + 16ms polling — idle CPU 0% broken).
    pub image: Option<InlineImage>,
    /// `MdImgEntry::born` of the entry the request was made for; [`InlineImageCache::apply_encode`] drops a result
    /// whose entry has since been replaced.
    pub born: u64,
}

/// Background worker that encodes inline-image protocols (the whole image or a cropped band) with a
/// cloned Picker, so the UI thread never blocks on the resize/encode (principle #4). Exits when the
/// request sender is dropped (the host shutting down).
pub fn md_encode_worker(
    picker: Picker,
    rx: std::sync::mpsc::Receiver<MdEncodeRequest>,
    tx: std::sync::mpsc::Sender<MdEncodeResult>,
) {
    while let Ok(req) = rx.recv() {
        let (path, key, born) = (req.path.clone(), req.key, req.born);
        let picker = &picker; // borrow: so the move closure doesn't consume picker across loop iterations
                              // Wrap the whole request handling in a panic catch: even if one new_protocol/crop
                              // panics, don't kill the worker thread (killing it would stop every future encode,
                              // latching enc_inflight forever).
        let image = konoma_base::panic::catch_silent(move || {
            let img = match req.crop {
                Some((x, y, w, h)) => req.image.crop_imm(x, y, w, h),
                None => (*req.image).clone(),
            };
            let size = ratatui::layout::Size::new(req.cols, req.rows);
            // A fence diagram (vector-derived) is **also scaled up** to the reserved grid (Scale):
            // even if the raster is small, it fills the area instead of leaving a top-left-packed
            // margin band — sharpness is handled by density-following re-rasterization instead.
            // A photo keeps the previous Fit behavior (never scaled up past its natural size = never blurred).
            //
            // A media-diff picture is **also** `Scale`, for a different reason than the mermaid case:
            // `preview::media_diff::layout`'s "shared scale" math already computed the *exact* target
            // cell box (`req.cols`/`req.rows`, clamped to never exceed the natural size), using
            // PDF-page **points** or a decode-budget-downscaled GIF frame as the "natural" unit —
            // either of which differs from this image's own *actual* pixel dimensions. Letting `Fit`
            // independently re-derive a fit from those actual dimensions doesn't reliably reproduce
            // the same box: it can come out smaller (a real bug) or larger (silently blocked entirely
            // — both `Image`/`KittyImage`'s `Widget::render` refuse to draw when the protocol's size
            // exceeds the render area). `Scale` sidesteps both by always resizing to exactly `size`,
            // matching what the caller then renders into.
            let is_media_diff_picture =
                crate::markdown::is_media_diff_url(&req.path.to_string_lossy());
            let resize = if crate::markdown::is_mermaid_fence_url(&req.path.to_string_lossy())
                || is_media_diff_picture
            {
                Resize::Scale(Some(FilterType::Lanczos3))
            } else {
                Resize::Fit(Some(FilterType::Lanczos3))
            };
            // Always send back a result even on failure (Err/None): swallowing it would leave
            // enc_inflight stuck true forever.
            match req.kitty {
                Some((id, is_tmux)) => encode_inline_kitty(picker, img, size, &resize, id, is_tmux)
                    .map(InlineImage::Kitty),
                None => picker
                    .new_protocol(img, size, resize)
                    .ok()
                    .map(InlineImage::Proto),
            }
        })
        .flatten();
        if tx
            .send(MdEncodeResult {
                path,
                key,
                image,
                born,
            })
            .is_err()
        {
            break;
        }
    }
}

/// Encode one inline image as konoma's own **compressed** kitty transmit under the fixed `id`.
///
/// The sizing is deliberately not reinvented: it replays `Picker::new_protocol`'s decision through
/// `Resize`'s public API, so this lands on the exact same cell area — and the same transparent
/// padding when the aspect ratio does not divide evenly into cells — that the ratatui-image path it
/// replaces produced. Getting that wrong would not fail loudly; it would quietly stretch every
/// diagram and equation by a few percent.
///
/// The one thing that does change is the payload: RGBA compressed with `o=z` (see `preview::kitty`),
/// which is what makes re-encoding an animated frame affordable in the first place.
fn encode_inline_kitty(
    picker: &Picker,
    img: image::DynamicImage,
    target: ratatui::layout::Size,
    resize: &Resize,
    id: u32,
    is_tmux: bool,
) -> Option<crate::kitty::KittyImage> {
    let font = picker.font_size();
    let natural = Resize::natural_size(&img, font);
    // `new_protocol` skips the resize entirely when the image already fits the target at its
    // natural cell size and lines up with the font grid on at least one axis (`needs_resize`'s
    // early `None`). `Scale` never takes that shortcut — it is defined as "resize even when
    // smaller". konoma passes no background color, so the padding color is ratatui-image's default.
    let fits = natural.width <= target.width && natural.height <= target.height;
    let on_grid = img.width() == u32::from(natural.width) * u32::from(font.width)
        || img.height() == u32::from(natural.height) * u32::from(font.height);
    let (img, area) = if !matches!(resize, Resize::Scale(_)) && fits && on_grid {
        (img, natural)
    } else {
        let area = resize.size_for(&img, font, target);
        (resize.resize(&img, font, area, None), area)
    };
    if area.width == 0 || area.height == 0 {
        return None;
    }
    Some(crate::kitty::KittyImage::build(
        &img.to_rgba8(),
        area.width,
        area.height,
        id,
        is_tmux,
    ))
}

/// The source-pixel band `(y0, height)` of an image `dh` pixels tall that corresponds to the visible
/// cell rows `[row_off, row_off + vis_rows)` out of `full_rows` total. The result is always within
/// `[0, dh]` (so `crop_imm(0, y0, _, height)` never exceeds the image and never panics), and the height
/// is at least 1.
pub fn md_band_pixels(full_rows: u16, row_off: u16, vis_rows: u16, dh: u32) -> (u32, u32) {
    let fr = full_rows.max(1) as u32;
    let y0 = (row_off as u32 * dh) / fr;
    let y1 = ((row_off as u32 + vis_rows as u32).min(fr) * dh) / fr;
    (y0, y1.saturating_sub(y0).max(1))
}

/// The kitty image ids of canvas pictures that are on the terminal now, **process-wide**: the
/// places that end the program (the normal exit, the panic hook, the signal thread) have no cache
/// to ask, and an image left in the terminal's storage outlives the program (it is counted
/// against the terminal's quota until the terminal evicts something). Ids are noted when a canvas
/// picture is given one ([`kitty_id_for`]) and forgotten when their delete is handed out
/// ([`InlineImageCache::take_kitty_deletes`]).
pub struct LiveKittyIds {
    ids: std::sync::Mutex<std::collections::BTreeSet<u32>>,
    tmux: std::sync::atomic::AtomicBool,
}

impl LiveKittyIds {
    pub const fn new() -> LiveKittyIds {
        LiveKittyIds {
            ids: std::sync::Mutex::new(std::collections::BTreeSet::new()),
            tmux: std::sync::atomic::AtomicBool::new(false),
        }
    }

    pub fn note(&self, id: u32) {
        self.ids
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id);
    }

    pub fn forget(&self, id: u32) {
        self.ids
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&id);
    }

    pub fn set_tmux(&self, tmux: bool) {
        self.tmux.store(tmux, std::sync::atomic::Ordering::SeqCst);
    }

    /// The ids noted now (for a test).
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn snapshot(&self) -> Vec<u32> {
        self.ids
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .copied()
            .collect()
    }

    /// The escapes that delete every id noted, and the table emptied. Never blocks on a lock that
    /// a panicking thread poisoned.
    pub fn take_delete_escapes(&self) -> Vec<String> {
        let tmux = self.tmux.load(std::sync::atomic::Ordering::SeqCst);
        std::mem::take(&mut *self.ids.lock().unwrap_or_else(|e| e.into_inner()))
            .into_iter()
            .map(|id| crate::kitty::build_delete(id, tmux))
            .collect()
    }
}

impl Default for LiveKittyIds {
    fn default() -> Self {
        Self::new()
    }
}

/// The ids of the canvas pictures on the terminal now (see [`LiveKittyIds`]).
pub static LIVE_CANVAS_KITTY: LiveKittyIds = LiveKittyIds::new();

/// The key the kitty image ids of a cache key are filed under: the picture's identity
/// (`canvas://<plugin>/<id>`) for a canvas picture, so new content takes over the ids of the old
/// and the terminal's copy is replaced; the key itself for everything else.
pub fn kitty_key(path: &Path) -> PathBuf {
    path.to_str()
        .and_then(crate::markdown::canvas_slot_of)
        .map_or_else(|| path.to_path_buf(), PathBuf::from)
}

/// The fixed kitty image id of slot `slot` in the family `key` belongs to, for the picture filed
/// under `path`, allocating it on first use. `None` when this terminal is not a kitty one — then
/// the encode goes back through ratatui-image, which writes the picture as cell content and leaves
/// the terminal holding nothing between frames, so there is no id to pin and none is burned.
///
/// `slot` is the index `reserve_proto_slot` just handed out, so an id is pinned to a slot and not
/// to the size that slot currently holds: recycling the slot re-points its id at the new size, and
/// the terminal replaces that one picture instead of gaining another. Ids are only ever appended,
/// never dropped (except the ids of a canvas picture, which the host deletes when the picture is
/// gone; the ids of a canvas picture are filed under its identity, not its content: [`kitty_key`]).
///
/// Takes the map rather than the cache so a caller can hold a `&mut` borrow of a cache entry at
/// the same time (two disjoint fields).
pub fn kitty_id_for(
    ids: &mut HashMap<PathBuf, [Vec<u32>; 3]>,
    path: &Path,
    key: &MdEncodeKey,
    slot: usize,
    use_kitty: bool,
) -> Option<u32> {
    if !use_kitty {
        return None;
    }
    let kitty_key = kitty_key(path);
    // A canvas picture's ids are noted process-wide so that every way out of the program can
    // delete them from the terminal (`LIVE_CANVAS_KITTY`).
    let canvas = crate::markdown::is_canvas_url(&kitty_key.to_string_lossy());
    let family = &mut ids.entry(kitty_key).or_default()[key.slot()];
    while family.len() <= slot {
        let id = crate::kitty::next_id();
        if canvas {
            LIVE_CANVAS_KITTY.note(id);
        }
        family.push(id);
    }
    family.get(slot).copied()
}

/// What [`InlineImageCache::apply_encode`] did with a result.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EncodeApplied {
    /// The result was for an entry that is gone or was replaced: nothing changed.
    Dropped,
    /// Applied (or a failed encode of one size left the last good state alone).
    Applied,
    /// Nothing drawable ever came back for this picture at any size: the entry is now `failed`,
    /// and the host must lay the picture out as its text fallback.
    Degraded,
}

/// The inline-image cache of one host: the entries by path, the kitty image ids pinned to their
/// encodes, the draw-pass counter and the handle to the encode worker.
#[derive(Default)]
pub struct InlineImageCache {
    /// The pictures by cache key (a path, or a synthetic URL such as `math://…`).
    pub cache: HashMap<PathBuf, MdImgEntry>,
    /// The kitty image ids pinned to the encodes of each picture: one id per slot of each of the
    /// three families (`MdEncodeKey::slot`), at the matching index. An id stays with its slot for
    /// good, so re-encoding into a slot replaces the terminal's picture instead of adding one.
    /// This lives here rather than on `MdImgEntry` because the cache may drop an entry (a changed
    /// file) while the terminal still holds its images: the ids outlive the entry and are reused.
    /// Filed under [`kitty_key`].
    pub kitty_ids: HashMap<PathBuf, [Vec<u32>; 3]>,
    /// Kitty image ids whose pictures are gone and must be deleted from the terminal
    /// ([`Self::take_kitty_deletes`]).
    pub kitty_deletes: Vec<u32>,
    /// The number of the draw pass in progress. Stamped onto the slot a pass wants, which is how
    /// `reserve_proto_slot` tells a slot the picture being drawn uses (never recycled) from an
    /// old one.
    pub frame: u64,
    /// The sender to the encode worker (`md_encode_worker`); `None` = no worker (tests).
    pub enc_tx: Option<std::sync::mpsc::Sender<MdEncodeRequest>>,
    /// Encode through konoma's own compressed kitty transmit (the terminal speaks kitty graphics).
    pub use_kitty: bool,
    /// The kitty escapes must be wrapped for tmux's passthrough.
    pub kitty_is_tmux: bool,
}

impl InlineImageCache {
    /// Ask for the encode of the placement `(cols, full_rows, row_off, vis_rows)` of the picture
    /// filed under `path`, unless it is settled, failed, or another encode is in flight (one at a
    /// time, so scrolling never queues a backlog). Fully visible → the whole image at
    /// `(cols, full_rows)`; partially scrolled → just the visible pixel band, so the picture
    /// renders clipped to the viewport rather than being hidden. `canvas_cap` is the most encodes
    /// a canvas picture may hold (`None`: not a canvas picture; its family grows by one when every
    /// slot is live).
    ///
    /// Without an encode worker nothing is ever encoded, so the call does nothing at all.
    /// Otherwise it stamps the position as wanted by the pass being drawn, **before** any other
    /// early return: a placement that has to wait for the in-flight ticket still needs its slot kept out
    /// of the recycling pool, otherwise two placements of one picture at two sizes take turns
    /// evicting each other and the draw→request→apply→draw loop never stops.
    pub fn ensure_encode(
        &mut self,
        path: &Path,
        cols: u16,
        full_rows: u16,
        row_off: u16,
        vis_rows: u16,
        canvas_cap: Option<usize>,
    ) {
        let Some(enc_tx) = self.enc_tx.clone() else {
            return;
        };
        let (use_kitty, is_tmux) = (self.use_kitty, self.kitty_is_tmux);
        let frame = self.frame;
        let ids = &mut self.kitty_ids;
        let Some(entry) = self.cache.get_mut(path) else {
            return;
        };
        let full_vis = row_off == 0 && vis_rows >= full_rows;
        let enc_key = if full_vis {
            MdEncodeKey::Full {
                cols,
                rows: full_rows,
            }
        } else {
            MdEncodeKey::Clip {
                cols,
                full_rows,
                row_off,
                vis_rows,
            }
        };
        let settled = entry.touch(&enc_key, frame);
        // Wait if it failed, is still decoding, or already has an encode in flight (one at a time).
        if settled || entry.failed || entry.enc_inflight {
            return;
        }
        let Some(img) = entry.decoded.clone() else {
            return;
        };
        let (crop, rows) = if full_vis {
            (None, full_rows)
        } else {
            let (dw, dh) = (img.width(), img.height());
            let (y0, h) = md_band_pixels(full_rows, row_off, vis_rows, dh);
            (Some((0, y0, dw, h)), vis_rows)
        };
        // A canvas picture's family is capped for good at its number of placements in the canvas
        // (its plugin is not trusted to place one picture at few sizes); a document's grows by one
        // when every slot is live.
        let slot = if let Some(cap) = canvas_cap {
            reserve_proto_slot_hard(entry.slots_mut(&enc_key), enc_key, frame, cap)
        } else {
            Some(reserve_proto_slot(
                entry.slots_mut(&enc_key),
                enc_key,
                frame,
                MD_PROTO_SLOTS,
            ))
        };
        let Some(slot) = slot else {
            return; // the cells stay under this placement
        };
        let kitty = kitty_id_for(ids, path, &enc_key, slot, use_kitty).map(|id| (id, is_tmux));
        entry.enc_inflight = true;
        let born = entry.born;
        let _ = enc_tx.send(MdEncodeRequest {
            path: path.to_path_buf(),
            key: enc_key,
            image: img,
            crop,
            cols,
            rows,
            kitty,
            born,
        });
    }

    /// Ask for the encode of the in-place zoom crop `crop` of `img` (the decoded pixels of the
    /// picture filed under `path`) into the `(cols, rows)` cell area. The zoom family is
    /// `MD_ZOOM_SLOTS` wide; at most one encode is in flight per picture.
    pub fn ensure_zoom_encode(
        &mut self,
        path: &Path,
        img: Arc<image::DynamicImage>,
        cols: u16,
        rows: u16,
        crop: PxRect,
    ) {
        let enc_tx = self.enc_tx.clone();
        let (use_kitty, is_tmux) = (self.use_kitty, self.kitty_is_tmux);
        let frame = self.frame;
        let ids = &mut self.kitty_ids;
        let Some(entry) = self.cache.get_mut(path) else {
            return;
        };
        let enc_key = MdEncodeKey::Zoom { cols, rows, crop };
        let settled = entry.touch(&enc_key, frame);
        if settled || entry.enc_inflight {
            return;
        }
        let Some(tx) = enc_tx else { return };
        let slot = reserve_proto_slot(&mut entry.zoom, enc_key, frame, MD_ZOOM_SLOTS);
        let kitty = kitty_id_for(ids, path, &enc_key, slot, use_kitty).map(|id| (id, is_tmux));
        entry.enc_inflight = true;
        let born = entry.born;
        let _ = tx.send(MdEncodeRequest {
            path: path.to_path_buf(),
            key: enc_key,
            image: img,
            crop: Some(crop),
            cols,
            rows,
            kitty,
            born,
        });
    }

    /// The image to draw for the visible portion of the picture filed under `path`, **at this
    /// placement's own cell size**. Prefers the encode made for exactly this position and size (the
    /// whole image when fully visible, or the band matching `(cols, full_rows, row_off,
    /// vis_rows)`); while that one is still encoding it returns the freshest other encode, so the
    /// image stays on screen — at another size or another band for a frame or two — and snaps to
    /// the exact one on arrival, rather than blinking out. `None` only until the very first encode
    /// for this image is ready.
    pub fn proto(
        &self,
        path: &Path,
        cols: u16,
        full_rows: u16,
        row_off: u16,
        vis_rows: u16,
    ) -> Option<&InlineImage> {
        let entry = self.cache.get(path)?;
        // Exact match for the current position, at this placement's own cell size.
        let key = if row_off == 0 && vis_rows >= full_rows {
            MdEncodeKey::Full {
                cols,
                rows: full_rows,
            }
        } else {
            MdEncodeKey::Clip {
                cols,
                full_rows,
                row_off,
                vis_rows,
            }
        };
        // Not yet encoded for this exact position (or that encode failed): keep the freshest band —
        // or full image — visible rather than leaving the reserved rows blank.
        entry.proto(&key).or_else(|| entry.newest_proto())
    }

    /// Apply a completed background encode into the slot that was reserved for it. A failed encode
    /// (`image: None`) leaves that slot's stored image alone — the last good state stays visible —
    /// while the slot keeps holding the key, which is what stops the same doomed request being
    /// re-sent every frame. Only when the whole picture was never encodable at any size does the
    /// entry degrade to the text fallback ([`EncodeApplied::Degraded`]).
    ///
    /// A result whose slot has since been recycled (its key no longer matches) is simply dropped:
    /// the position it answered is not on screen any more, and writing it back would evict a slot
    /// the current frame *is* drawing from.
    pub fn apply_encode(&mut self, res: MdEncodeResult) -> EncodeApplied {
        let Some(entry) = self.cache.get_mut(&res.path) else {
            return EncodeApplied::Dropped;
        };
        // Encoded for an entry that has since been dropped and re-created under the same path:
        // it must neither clear the new entry's in-flight flag nor fill (or degrade) its slots.
        if entry.born != res.born {
            return EncodeApplied::Dropped;
        }
        entry.enc_inflight = false;
        // An encode cut from pixels that have been replaced since it was requested still shows
        // (the old picture keeps drawing until the new one lands) but is not final: its slot stays
        // stale, so the next pass encodes it again from the current pixels.
        let outdated = std::mem::take(&mut entry.outdated_encode_in_flight);
        let key = res.key;
        let mut stored = false;
        if let Some(slot) = entry.slots_mut(&key).iter_mut().find(|s| s.key == key) {
            slot.stale = outdated;
            if let Some(p) = res.image {
                slot.image = Some(p);
                stored = true;
            }
        }
        // Nothing drawable has ever come back for this picture at any size → text fallback.
        let degrade = !stored
            && matches!(key, MdEncodeKey::Full { .. })
            && !entry.failed
            && entry.full.iter().all(|s| s.image.is_none());
        if degrade {
            entry.failed = true;
            return EncodeApplied::Degraded;
        }
        EncodeApplied::Applied
    }

    /// Drive every animated inline GIF (the per-entry analog of the full-screen `GifPlayer`). Each
    /// cache entry with at least two frames advances independently once its current frame's
    /// display time has elapsed. Advancing swaps `decoded` to the new frame and marks every encoded
    /// slot **stale** — but does **not** drop the encoded protocols themselves, which stay visible
    /// until `ensure_encode` (called from the renderer, only for placements actually drawn)
    /// requests and receives a fresh encode of the new frame. Advancing an off-screen entry
    /// therefore costs only an index bump, never a re-encode. Returns true if any entry advanced
    /// (the caller re-renders). Whether to tick at all (the host's mode) is the caller's call.
    pub fn advance_gifs_if_due(&mut self) -> bool {
        let now = std::time::Instant::now();
        let mut advanced = false;
        for entry in self.cache.values_mut() {
            if entry.frames.len() < 2 {
                continue;
            }
            let Some(shown_at) = entry.shown_at else {
                // First tick: just start the timer (the first frame is already shown in decoded).
                entry.shown_at = Some(now);
                continue;
            };
            let delay = entry.frames[entry.idx].1;
            if now.duration_since(shown_at) < delay {
                continue;
            }
            entry.idx = (entry.idx + 1) % entry.frames.len();
            entry.shown_at = Some(now);
            entry.decoded = Some(entry.frames[entry.idx].0.clone());
            // The next frame needs a re-encode: mark every slot stale rather than dropping it. The
            // old protocol stays displayed until the new encode arrives (clearing it would leave a
            // momentary blank — the same convention as the fence re-raster), and, because the slot
            // survives, the re-encode lands on the **same** kitty id and replaces the terminal's
            // picture instead of adding one per animation frame.
            entry.mark_stale();
            advanced = true;
        }
        advanced
    }

    /// The escapes that delete from a kitty-graphics terminal the images of the pictures that went
    /// (to be written to the terminal after the frame). Empty on any other terminal: no id is ever
    /// pinned there.
    pub fn take_kitty_deletes(&mut self) -> Vec<String> {
        let ids = std::mem::take(&mut self.kitty_deletes);
        if !self.use_kitty {
            return Vec::new();
        }
        for id in &ids {
            LIVE_CANVAS_KITTY.forget(*id);
        }
        ids.into_iter()
            .map(|id| crate::kitty::build_delete(id, self.kitty_is_tmux))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn full(cols: u16) -> MdEncodeKey {
        MdEncodeKey::Full { cols, rows: 4 }
    }

    fn px() -> Arc<image::DynamicImage> {
        Arc::new(image::DynamicImage::new_rgba8(2, 2))
    }

    #[test]
    fn kitty_ids_are_pinned_to_the_slot_and_reused() {
        let mut ids = HashMap::new();
        let p = Path::new("/x/a.png");
        let a = kitty_id_for(&mut ids, p, &full(10), 0, true).expect("an id");
        // The same slot gets the same id however often it is asked, and another slot another one.
        assert_eq!(kitty_id_for(&mut ids, p, &full(20), 0, true), Some(a));
        let b = kitty_id_for(&mut ids, p, &full(10), 1, true).expect("an id");
        assert_ne!(a, b);
        // The families of one picture have ids of their own.
        let clip = MdEncodeKey::Clip {
            cols: 10,
            full_rows: 4,
            row_off: 1,
            vis_rows: 2,
        };
        let c = kitty_id_for(&mut ids, p, &clip, 0, true).expect("an id");
        assert!(c != a && c != b);
        // Not a kitty terminal: no id is allocated.
        assert_eq!(kitty_id_for(&mut ids, p, &full(10), 0, false), None);
    }

    #[test]
    fn a_hard_family_never_grows_past_its_cap() {
        let mut slots = Vec::new();
        let given = (0..6u16)
            .filter(|n| reserve_proto_slot_hard(&mut slots, full(*n + 1), 5, 3).is_some())
            .count();
        assert_eq!(given, 3);
        assert_eq!(slots.len(), 3);
        // A soft family grows by one when every slot is live in this frame.
        let mut soft = Vec::new();
        for n in 0..5u16 {
            reserve_proto_slot(&mut soft, full(n + 1), 5, 3);
        }
        assert_eq!(soft.len(), 5);
    }

    use std::sync::mpsc::{channel, Receiver, TryRecvError};
    use std::time::{Duration, Instant};

    /// An instant `d` in the past (the cache reads `Instant::now()` itself, so a test places the
    /// timer relative to it instead of sleeping).
    fn ago(d: Duration) -> Instant {
        Instant::now().checked_sub(d).expect("uptime is longer")
    }

    /// A kitty image whose `cell_size` is `(cols, 1)`: tests tell images apart by it.
    fn kimg(cols: u16) -> InlineImage {
        InlineImage::Kitty(crate::kitty::KittyImage::build(
            &image::RgbaImage::new(1, 1),
            cols,
            1,
            1,
            false,
        ))
    }

    fn nothing_sent(rx: &Receiver<MdEncodeRequest>) {
        assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));
    }

    /// A cache with a worker channel and one decoded entry (`born` 3) filed under `path`.
    fn cache_with_entry(path: &str) -> (InlineImageCache, Receiver<MdEncodeRequest>) {
        let (tx, rx) = channel();
        let mut c = InlineImageCache {
            enc_tx: Some(tx),
            ..Default::default()
        };
        let e = c.cache.entry(PathBuf::from(path)).or_default();
        e.decoded = Some(px());
        e.born = 3;
        (c, rx)
    }

    #[test]
    fn a_gif_steps_one_frame_per_elapsed_delay_and_marks_the_slots_stale() {
        let hour = Duration::from_secs(3600);
        let mut c = InlineImageCache::default();
        let path = PathBuf::from("/x/a.gif");
        let (f0, f1) = (px(), px());
        let e = c.cache.entry(path.clone()).or_default();
        e.decoded = Some(f0.clone());
        e.frames = vec![(f0.clone(), hour), (f1.clone(), hour)];
        e.full.push(MdProtoSlot::encoded(full(10), kimg(1)));
        e.clip.push(MdProtoSlot::encoded(full(11), kimg(1)));
        e.zoom.push(MdProtoSlot::encoded(full(12), kimg(1)));
        e.enc_inflight = true;
        // The first tick only starts the timer: nothing advances, nothing is marked.
        let before = Instant::now();
        assert!(!c.advance_gifs_if_due());
        let started = c.cache[&path].shown_at.expect("the timer started");
        assert!(started >= before);
        assert_eq!(c.cache[&path].idx, 0);
        assert!(!c.cache[&path].full[0].stale && !c.cache[&path].outdated_encode_in_flight);
        // Before the deadline (the frame has just begun, its delay is an hour): no step.
        assert!(!c.advance_gifs_if_due());
        assert_eq!(c.cache[&path].idx, 0);
        assert!(!c.cache[&path].full[0].stale);
        // Past the deadline: one step, however far past it is.
        c.cache.get_mut(&path).unwrap().shown_at = Some(ago(hour * 5));
        assert!(c.advance_gifs_if_due());
        let e = &c.cache[&path];
        assert_eq!(e.idx, 1);
        assert!(Arc::ptr_eq(e.decoded.as_ref().unwrap(), &f1));
        // Every family is marked stale (not dropped), and the encode in flight is outdated.
        assert!(e.full[0].stale && e.clip[0].stale && e.zoom[0].stale);
        assert!(
            e.full[0].image.is_some() && e.clip[0].image.is_some() && e.zoom[0].image.is_some()
        );
        assert!(e.outdated_encode_in_flight);
        // The timer restarted: the next call is before the new deadline.
        assert!(e.shown_at.unwrap() >= started);
        assert!(!c.advance_gifs_if_due());
        assert_eq!(c.cache[&path].idx, 1);
        // The index wraps back to the first frame.
        c.cache.get_mut(&path).unwrap().shown_at = Some(ago(hour * 5));
        assert!(c.advance_gifs_if_due());
        let e = &c.cache[&path];
        assert_eq!(e.idx, 0);
        assert!(Arc::ptr_eq(e.decoded.as_ref().unwrap(), &f0));
        // The delay that counts is the one of the frame being shown: frame 0 is short here.
        let e = c.cache.get_mut(&path).unwrap();
        e.frames[0].1 = Duration::from_millis(10);
        e.shown_at = Some(ago(Duration::from_secs(1)));
        assert!(c.advance_gifs_if_due());
        assert_eq!(c.cache[&path].idx, 1);
        // A still picture never advances, nor does a one-frame "animation".
        let mut still = InlineImageCache::default();
        still.cache.entry(path.clone()).or_default().decoded = Some(px());
        assert!(!still.advance_gifs_if_due());
        let e = still.cache.get_mut(&path).unwrap();
        e.frames = vec![(px(), Duration::ZERO)];
        e.shown_at = Some(ago(hour));
        assert!(!still.advance_gifs_if_due());
        assert_eq!(still.cache[&path].idx, 0);
    }

    #[test]
    fn an_encode_result_is_applied_dropped_or_degrades_the_picture() {
        let mut c = InlineImageCache::default();
        let path = PathBuf::from("/x/a.png");
        let e = c.cache.entry(path.clone()).or_default();
        e.born = 7;
        e.enc_inflight = true;
        e.full.push(MdProtoSlot::attempted(full(10)));
        let res = |born, key, image| MdEncodeResult {
            path: PathBuf::from("/x/a.png"),
            key,
            image,
            born,
        };
        // Answered for an earlier life of the path: nothing is touched.
        assert_eq!(
            c.apply_encode(res(6, full(10), Some(kimg(5)))),
            EncodeApplied::Dropped
        );
        assert!(c.cache[&path].enc_inflight);
        assert!(c.cache[&path].full[0].image.is_none());
        // Nothing drawable ever came back at any size: the picture degrades to its text.
        assert_eq!(
            c.apply_encode(res(7, full(10), None)),
            EncodeApplied::Degraded
        );
        assert!(c.cache[&path].failed && !c.cache[&path].enc_inflight);
        // Once failed it is not reported as degraded again.
        assert_eq!(
            c.apply_encode(res(7, full(10), None)),
            EncodeApplied::Applied
        );
        // An unknown path is dropped.
        let mut other = InlineImageCache::default();
        assert_eq!(
            other.apply_encode(res(7, full(10), None)),
            EncodeApplied::Dropped
        );
    }

    #[test]
    fn an_applied_encode_is_stored_and_updates_the_stale_state() {
        let path = PathBuf::from("/x/a.png");
        let res = |key, image| MdEncodeResult {
            path: PathBuf::from("/x/a.png"),
            key,
            image,
            born: 7,
        };
        let fresh = || {
            let mut c = InlineImageCache::default();
            let e = c.cache.entry(path.clone()).or_default();
            e.born = 7;
            e.enc_inflight = true;
            let mut stale = MdProtoSlot::attempted(full(10));
            stale.stale = true;
            e.full.push(stale);
            c
        };
        // A normal landing: stored, no longer stale, the ticket is given back.
        let mut c = fresh();
        assert_eq!(
            c.apply_encode(res(full(10), Some(kimg(5)))),
            EncodeApplied::Applied
        );
        let e = &c.cache[&path];
        assert_eq!(e.full[0].image.as_ref().unwrap().cell_size(), (5, 1));
        assert!(!e.full[0].stale && !e.enc_inflight && !e.failed);
        // The encode was cut from pixels replaced since: it is stored (drawn meanwhile) but the
        // slot stays stale, and the flag is consumed so the next landing is final.
        let mut c = fresh();
        c.cache.get_mut(&path).unwrap().outdated_encode_in_flight = true;
        assert_eq!(
            c.apply_encode(res(full(10), Some(kimg(6)))),
            EncodeApplied::Applied
        );
        let e = &c.cache[&path];
        assert_eq!(e.full[0].image.as_ref().unwrap().cell_size(), (6, 1));
        assert!(e.full[0].stale && !e.outdated_encode_in_flight && !e.enc_inflight);
        assert_eq!(
            c.apply_encode(res(full(10), Some(kimg(7)))),
            EncodeApplied::Applied
        );
        assert!(!c.cache[&path].full[0].stale);
        // A failed encode of one size leaves the last good picture of that slot alone (and the
        // stale state follows the flag), without degrading.
        let mut c = fresh();
        c.apply_encode(res(full(10), Some(kimg(5))));
        c.cache.get_mut(&path).unwrap().full[0].stale = true;
        assert_eq!(c.apply_encode(res(full(10), None)), EncodeApplied::Applied);
        let e = &c.cache[&path];
        assert_eq!(e.full[0].image.as_ref().unwrap().cell_size(), (5, 1));
        assert!(!e.full[0].stale && !e.failed);
        // A result whose slot was recycled (another key now) writes nothing.
        let mut c = fresh();
        c.cache.get_mut(&path).unwrap().full[0] = MdProtoSlot::encoded(full(20), kimg(9));
        assert_eq!(
            c.apply_encode(res(full(10), Some(kimg(5)))),
            EncodeApplied::Applied
        );
        let e = &c.cache[&path];
        assert_eq!(e.full.len(), 1);
        assert_eq!(e.full[0].key, full(20));
        assert_eq!(e.full[0].image.as_ref().unwrap().cell_size(), (9, 1));
        assert!(!e.enc_inflight);
        // A failed Full of one size does not degrade while another size has a picture.
        let mut c = fresh();
        c.cache
            .get_mut(&path)
            .unwrap()
            .full
            .push(MdProtoSlot::encoded(full(20), kimg(9)));
        assert_eq!(c.apply_encode(res(full(10), None)), EncodeApplied::Applied);
        assert!(!c.cache[&path].failed);
    }

    #[test]
    fn a_failed_clip_or_zoom_encode_never_degrades_the_picture() {
        let path = PathBuf::from("/x/a.png");
        let clip = MdEncodeKey::Clip {
            cols: 10,
            full_rows: 4,
            row_off: 1,
            vis_rows: 2,
        };
        let zoom = MdEncodeKey::Zoom {
            cols: 10,
            rows: 4,
            crop: (0, 0, 1, 1),
        };
        for key in [clip, zoom] {
            let mut c = InlineImageCache::default();
            let e = c.cache.entry(path.clone()).or_default();
            e.born = 7;
            e.enc_inflight = true;
            // No full picture exists at all, which is what would degrade a failed Full.
            e.full.push(MdProtoSlot::attempted(full(10)));
            e.slots_mut(&key).push(MdProtoSlot::attempted(key));
            let r = MdEncodeResult {
                path: path.clone(),
                key,
                image: None,
                born: 7,
            };
            assert_eq!(c.apply_encode(r), EncodeApplied::Applied, "{key:?}");
            let e = &c.cache[&path];
            assert!(!e.failed, "{key:?}");
            assert!(!e.enc_inflight, "the ticket is given back: {key:?}");
        }
    }

    #[test]
    fn take_kitty_deletes_is_empty_off_kitty_and_builds_escapes_on_it() {
        // `LIVE_CANVAS_KITTY` is process-wide: these ids are far from the ones `kitty::next_id`
        // hands out, and only they are asserted on.
        const A: u32 = 3_900_001;
        const B: u32 = 3_900_002;
        const KEEP: u32 = 3_900_003;
        const T: u32 = 3_900_004;
        for id in [A, B, KEEP, T] {
            LIVE_CANVAS_KITTY.note(id);
        }
        let live = |id: u32| LIVE_CANVAS_KITTY.snapshot().contains(&id);

        // Off kitty: nothing to write, the list is taken anyway, and the ids stay noted.
        let mut c = InlineImageCache {
            kitty_deletes: vec![A],
            ..Default::default()
        };
        assert!(c.take_kitty_deletes().is_empty());
        assert!(c.kitty_deletes.is_empty(), "taken either way");
        assert!(live(A), "not deleted from the terminal, so not forgotten");

        // On kitty: the escapes, in order, and the ids are forgotten process-wide.
        c.use_kitty = true;
        c.kitty_deletes = vec![A, B];
        assert_eq!(
            c.take_kitty_deletes(),
            vec![
                crate::kitty::build_delete(A, false),
                crate::kitty::build_delete(B, false)
            ]
        );
        assert!(!live(A) && !live(B));
        assert!(live(KEEP) && live(T), "only the deleted ids are forgotten");
        assert!(c.take_kitty_deletes().is_empty());

        // Under tmux the escapes are wrapped for the passthrough.
        c.kitty_is_tmux = true;
        c.kitty_deletes = vec![T];
        let wrapped = c.take_kitty_deletes();
        assert_eq!(wrapped, vec![crate::kitty::build_delete(T, true)]);
        assert_ne!(wrapped[0], crate::kitty::build_delete(T, false));
        assert!(wrapped[0].starts_with("\x1bPtmux;"));
        assert!(!live(T));

        LIVE_CANVAS_KITTY.forget(KEEP);
    }

    #[test]
    fn without_a_worker_ensure_encode_changes_nothing() {
        let (mut c, _rx) = cache_with_entry("/x/a.png");
        c.enc_tx = None;
        c.use_kitty = true;
        c.frame = 4;
        c.ensure_encode(Path::new("/x/a.png"), 10, 4, 0, 4, None);
        let e = &c.cache[Path::new("/x/a.png")];
        assert!(e.full.is_empty() && e.clip.is_empty() && !e.enc_inflight);
        assert!(
            c.kitty_ids.is_empty(),
            "no id is burned for a request never sent"
        );
        // An unknown path is ignored as well.
        let (mut c, rx) = cache_with_entry("/x/a.png");
        c.ensure_encode(Path::new("/x/none.png"), 10, 4, 0, 4, None);
        nothing_sent(&rx);
    }

    #[test]
    fn ensure_encode_sends_one_request_with_its_key_crop_and_kitty_id() {
        let path = Path::new("/x/a.png");
        let (mut c, rx) = cache_with_entry("/x/a.png");
        c.frame = 5;
        // Not a kitty terminal: the whole image, no id.
        c.ensure_encode(path, 10, 4, 0, 4, None);
        let r = rx.try_recv().expect("a request");
        assert_eq!(r.path, path);
        assert_eq!(r.key, MdEncodeKey::Full { cols: 10, rows: 4 });
        assert!(r.crop.is_none());
        assert_eq!((r.cols, r.rows, r.born), (10, 4, 3));
        assert!(r.kitty.is_none());
        let e = &c.cache[path];
        assert!(e.enc_inflight);
        assert_eq!((e.full.len(), e.full[0].used), (1, 5));
        assert!(c.kitty_ids.is_empty());
        // The single ticket is taken: asking again (even for another size) sends nothing, but
        // still stamps the slot it already holds.
        c.frame = 6;
        c.ensure_encode(path, 10, 4, 0, 4, None);
        c.ensure_encode(path, 20, 4, 0, 4, None);
        nothing_sent(&rx);
        let e = &c.cache[path];
        assert_eq!(
            (e.full.len(), e.full[0].used),
            (1, 6),
            "no slot for the waiting size"
        );

        // A partially scrolled placement asks for the visible band only.
        let (mut c, rx) = cache_with_entry("/x/a.png");
        c.use_kitty = true;
        c.kitty_is_tmux = true;
        c.ensure_encode(path, 10, 4, 1, 2, None);
        let r = rx.try_recv().expect("a request");
        assert_eq!(
            r.key,
            MdEncodeKey::Clip {
                cols: 10,
                full_rows: 4,
                row_off: 1,
                vis_rows: 2
            }
        );
        let (y0, h) = md_band_pixels(4, 1, 2, 2);
        assert_eq!(r.crop, Some((0, y0, 2, h)));
        assert_eq!((r.cols, r.rows), (10, 2));
        let id = c.kitty_ids[path][1][0];
        assert_eq!(r.kitty, Some((id, true)));
        let e = &c.cache[path];
        assert!(e.enc_inflight && e.full.is_empty() && e.clip.len() == 1);
        // A band cut off at the bottom (row_off 0, fewer rows visible) is a band too.
        let (mut c, rx) = cache_with_entry("/x/a.png");
        c.ensure_encode(path, 10, 4, 0, 3, None);
        assert!(matches!(
            rx.try_recv().unwrap().key,
            MdEncodeKey::Clip { .. }
        ));
    }

    #[test]
    fn ensure_encode_sends_nothing_for_a_settled_failed_or_undecoded_picture() {
        let path = Path::new("/x/a.png");
        // Settled: already encoded.
        let (mut c, rx) = cache_with_entry("/x/a.png");
        c.cache
            .get_mut(path)
            .unwrap()
            .full
            .push(MdProtoSlot::encoded(full(10), kimg(1)));
        c.frame = 8;
        c.ensure_encode(path, 10, 4, 0, 4, None);
        nothing_sent(&rx);
        let e = &c.cache[path];
        assert!(!e.enc_inflight);
        assert_eq!(e.full[0].used, 8, "stamped");
        // Attempted and failed: settled as well (never asked again).
        let (mut c, rx) = cache_with_entry("/x/a.png");
        c.cache
            .get_mut(path)
            .unwrap()
            .full
            .push(MdProtoSlot::attempted(full(10)));
        c.ensure_encode(path, 10, 4, 0, 4, None);
        nothing_sent(&rx);
        // Stale is not settled: it is asked again, into the same slot and the same id.
        let (mut c, rx) = cache_with_entry("/x/a.png");
        c.use_kitty = true;
        let mut s = MdProtoSlot::encoded(full(10), kimg(1));
        s.stale = true;
        c.cache.get_mut(path).unwrap().full.push(s);
        c.ensure_encode(path, 10, 4, 0, 4, None);
        let r = rx.try_recv().expect("re-encode of the stale slot");
        assert_eq!(r.kitty.unwrap().0, c.kitty_ids[path][0][0]);
        assert_eq!(c.cache[path].full.len(), 1);
        // The picture failed to decode/encode for good.
        let (mut c, rx) = cache_with_entry("/x/a.png");
        c.cache.get_mut(path).unwrap().failed = true;
        c.ensure_encode(path, 10, 4, 0, 4, None);
        nothing_sent(&rx);
        assert!(c.cache[path].full.is_empty());
        // Still decoding: no pixels to send, and no slot is reserved for them.
        let (mut c, rx) = cache_with_entry("/x/a.png");
        c.cache.get_mut(path).unwrap().decoded = None;
        c.ensure_encode(path, 10, 4, 0, 4, None);
        nothing_sent(&rx);
        let e = &c.cache[path];
        assert!(e.full.is_empty() && !e.enc_inflight);
    }

    #[test]
    fn a_document_family_grows_when_every_slot_is_live_and_recycles_otherwise() {
        let path = Path::new("/x/a.png");
        let (mut c, rx) = cache_with_entry("/x/a.png");
        c.use_kitty = true;
        c.frame = 1;
        // Five sizes in one pass: all live, so the soft family grows past MD_PROTO_SLOTS.
        for n in 0..5u16 {
            c.ensure_encode(path, 10 + n, 4, 0, 4, None);
            assert!(rx.try_recv().is_ok(), "size {n} is requested");
            c.cache.get_mut(path).unwrap().enc_inflight = false;
        }
        assert_eq!(c.cache[path].full.len(), 5);
        assert_eq!(c.kitty_ids[path][0].len(), 5);
        // A later pass wants one more size: the least recently wanted slot is recycled (and so is
        // its id), the family does not grow.
        c.frame = 2;
        for n in 1..5u16 {
            c.ensure_encode(path, 10 + n, 4, 0, 4, None); // stamp all but the first
        }
        let id0 = c.kitty_ids[path][0][0];
        c.ensure_encode(path, 99, 4, 0, 4, None);
        let r = rx.try_recv().expect("a request");
        assert_eq!(r.kitty.unwrap().0, id0);
        let e = &c.cache[path];
        assert_eq!(e.full.len(), 5);
        assert_eq!(e.full[0].key, MdEncodeKey::Full { cols: 99, rows: 4 });
    }

    #[test]
    fn a_canvas_cap_refuses_a_slot_without_taking_the_ticket() {
        let path = Path::new("/x/a.png");
        let (mut c, rx) = cache_with_entry("/x/a.png");
        c.use_kitty = true;
        c.frame = 1;
        for n in 0..2u16 {
            c.ensure_encode(path, 10 + n, 4, 0, 4, Some(2));
            assert!(rx.try_recv().is_ok());
            c.cache.get_mut(path).unwrap().enc_inflight = false;
        }
        let ids_before = c.kitty_ids[path][0].clone();
        // A third size while both slots are live in this pass: refused.
        c.ensure_encode(path, 12, 4, 0, 4, Some(2));
        nothing_sent(&rx);
        let e = &c.cache[path];
        assert!(!e.enc_inflight, "a refusal does not hold the ticket");
        assert_eq!(e.full.len(), 2);
        assert_eq!(c.kitty_ids[path][0], ids_before, "no id is burned");
        // Since the ticket is free, the next pass recycles a slot for it.
        c.frame = 2;
        c.ensure_encode(path, 12, 4, 0, 4, Some(2));
        let r = rx.try_recv().expect("a request once a slot is free");
        assert!(ids_before.contains(&r.kitty.unwrap().0));
        assert_eq!(c.cache[path].full.len(), 2);
        // The same pass, under a document (no cap), would have grown instead.
        let (mut c, rx) = cache_with_entry("/x/a.png");
        c.frame = 1;
        for n in 0..4u16 {
            c.ensure_encode(path, 10 + n, 4, 0, 4, None);
            c.cache.get_mut(path).unwrap().enc_inflight = false;
        }
        assert_eq!(c.cache[path].full.len(), 4);
        drop(rx);
    }

    #[test]
    fn a_canvas_picture_notes_its_ids_process_wide() {
        let path = Path::new("canvas://plug-widgets-test/7/00000000000000aa");
        let (mut c, rx) = cache_with_entry("canvas://plug-widgets-test/7/00000000000000aa");
        c.use_kitty = true;
        c.ensure_encode(path, 10, 4, 0, 4, Some(2));
        let id = rx.try_recv().unwrap().kitty.unwrap().0;
        assert!(LIVE_CANVAS_KITTY.snapshot().contains(&id));
        // Filed under the picture's identity, so new content takes over the id.
        let slot_key = PathBuf::from("canvas://plug-widgets-test/7");
        assert_eq!(c.kitty_ids[&slot_key][0], vec![id]);
        LIVE_CANVAS_KITTY.forget(id);
    }

    #[test]
    fn ensure_zoom_encode_sends_one_crop_and_keeps_its_slot_budget() {
        let path = Path::new("/x/a.png");
        let crop = |n: u32| (n, 0, 1, 1);
        // No worker: the position is still stamped, nothing is reserved or sent.
        let (mut c, _rx) = cache_with_entry("/x/a.png");
        c.enc_tx = None;
        c.frame = 9;
        c.cache
            .get_mut(path)
            .unwrap()
            .zoom
            .push(MdProtoSlot::attempted(MdEncodeKey::Zoom {
                cols: 10,
                rows: 4,
                crop: crop(0),
            }));
        c.ensure_zoom_encode(path, px(), 10, 4, crop(0));
        c.ensure_zoom_encode(path, px(), 10, 4, crop(1));
        let e = &c.cache[path];
        assert_eq!(
            (e.zoom.len(), e.zoom[0].used, e.enc_inflight),
            (1, 9, false)
        );
        // An unknown path is ignored.
        let (mut c, rx) = cache_with_entry("/x/a.png");
        c.ensure_zoom_encode(Path::new("/x/none.png"), px(), 10, 4, crop(0));
        nothing_sent(&rx);

        // A request: key, crop, size, id of the zoom family.
        let (mut c, rx) = cache_with_entry("/x/a.png");
        c.use_kitty = true;
        c.kitty_is_tmux = true;
        c.frame = 2;
        c.ensure_zoom_encode(path, px(), 10, 4, crop(0));
        let r = rx.try_recv().expect("a request");
        assert_eq!(
            r.key,
            MdEncodeKey::Zoom {
                cols: 10,
                rows: 4,
                crop: crop(0)
            }
        );
        assert_eq!((r.crop, r.cols, r.rows, r.born), (Some(crop(0)), 10, 4, 3));
        assert_eq!(r.kitty, Some((c.kitty_ids[path][2][0], true)));
        assert!(c.cache[path].enc_inflight);
        // One in flight at a time.
        c.ensure_zoom_encode(path, px(), 10, 4, crop(1));
        nothing_sent(&rx);
        assert_eq!(c.cache[path].zoom.len(), 1);
        // Settled: not asked again; a stale one is.
        let e = c.cache.get_mut(path).unwrap();
        e.enc_inflight = false;
        e.zoom[0].image = Some(kimg(1));
        c.ensure_zoom_encode(path, px(), 10, 4, crop(0));
        nothing_sent(&rx);
        c.cache.get_mut(path).unwrap().zoom[0].stale = true;
        c.ensure_zoom_encode(path, px(), 10, 4, crop(0));
        assert!(rx.try_recv().is_ok());
        // The family is MD_ZOOM_SLOTS wide: a third crop in a later pass recycles the oldest.
        c.cache.get_mut(path).unwrap().enc_inflight = false;
        c.frame = 3;
        c.ensure_zoom_encode(path, px(), 10, 4, crop(1));
        assert!(rx.try_recv().is_ok());
        c.cache.get_mut(path).unwrap().enc_inflight = false;
        c.frame = 4;
        c.ensure_zoom_encode(path, px(), 10, 4, crop(2));
        assert!(rx.try_recv().is_ok());
        assert_eq!(c.cache[path].zoom.len(), MD_ZOOM_SLOTS);
        assert!(c.kitty_ids[path][2].len() <= MD_ZOOM_SLOTS);
    }

    #[test]
    fn proto_is_the_exact_encode_else_the_freshest_one() {
        let path = Path::new("/x/a.png");
        let clip = MdEncodeKey::Clip {
            cols: 10,
            full_rows: 4,
            row_off: 1,
            vis_rows: 2,
        };
        let (mut c, _rx) = cache_with_entry("/x/a.png");
        // Nothing encoded yet, and an unknown path: nothing to draw.
        assert!(c.proto(path, 10, 4, 0, 4).is_none());
        assert!(c.proto(Path::new("/x/none.png"), 10, 4, 0, 4).is_none());
        let e = c.cache.get_mut(path).unwrap();
        let mut a = MdProtoSlot::encoded(full(10), kimg(11));
        a.used = 5;
        let mut b = MdProtoSlot::encoded(full(20), kimg(22));
        b.used = 9;
        e.full.push(a);
        e.full.push(b);
        e.full.push(MdProtoSlot::attempted(full(30))); // in flight / failed: holds no picture
        let mut band = MdProtoSlot::encoded(clip, kimg(33));
        band.used = 7;
        e.clip.push(band);
        let size = |i: Option<&InlineImage>| i.map(|i| i.cell_size().0);
        // The exact key wins even when another one is newer.
        assert_eq!(size(c.proto(path, 10, 4, 0, 4)), Some(11));
        assert_eq!(size(c.proto(path, 20, 4, 0, 4)), Some(22));
        assert_eq!(size(c.proto(path, 10, 4, 1, 2)), Some(33));
        // A slot that holds no picture falls back to the freshest band-or-full one (the band, 7,
        // beats the full 5; the full 20 at 9 is newer than both and is a full image too).
        assert_eq!(size(c.proto(path, 30, 4, 0, 4)), Some(22));
        // A size never encoded falls back as well, rather than blanking the picture.
        assert_eq!(size(c.proto(path, 40, 4, 0, 4)), Some(22));
        // A band that was never encoded falls back too.
        assert_eq!(size(c.proto(path, 10, 4, 2, 2)), Some(22));
        // The position decides the family: fully visible is a Full key, the rest a Clip key.
        c.cache.get_mut(path).unwrap().full[1].used = 1;
        assert_eq!(
            size(c.proto(path, 40, 4, 0, 4)),
            Some(33),
            "the band is the freshest now"
        );
        assert_eq!(
            size(c.proto(path, 20, 4, 0, 3)),
            Some(33),
            "cut at the bottom: a Clip key"
        );
        assert_eq!(
            size(c.proto(path, 20, 4, 0, 5)),
            Some(22),
            "taller than the box is still full"
        );
    }
}
