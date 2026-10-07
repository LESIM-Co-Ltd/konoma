// Built-in image renderer (implemented in M2; the actual code lives in a separate file).
//
// Policy: display it filling the whole area with `ratatui-image`'s StatefulImage (kitty graphics).
// Resize/encode are offloaded to a separate thread (tokio) so the UI thread is never blocked.
//
// Where the implementation lives:
//   - State (decode / holding & applying ThreadProtocol): `app.rs`'s load_image / apply_image_resize.
//   - Rendering (border + StatefulImage): `ui/preview.rs`'s render_image.
//   - Offload (worker thread, channel): `main.rs`'s resize_worker / poll loop.
//
// GIF animation (M6): expand all frames to RGBA and return them along with each frame's display
// time. The app side advances to the frame whose deadline has arrived on each poll tick, swapping
// image_src to trigger re-encoding.

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use image::{AnimationDecoder, DynamicImage, ImageDecoder};

/// A decoded animated GIF's frames, each paired with its own display time.
type GifFrames = Vec<(DynamicImage, Duration)>;

/// Largest image side konoma decodes, in pixels. PNG and JPEG can declare up to 2^31 / 65535; real
/// images top out far below (a 100-megapixel medium-format photo is 11,600 px wide, a stitched
/// panorama or a map 30,000 px). The image crate checks this against the header **before** it
/// allocates anything.
pub(crate) const MAX_IMAGE_SIDE: u32 = 32_768;

/// Largest image area konoma decodes, in pixels. A 48-megapixel photo is 4.8e7 and an 11,000 x
/// 11,000 screenshot of a wall of monitors 1.2e8 — both must keep working; 1.5e8 (a 12,200 px
/// square) is the line past which a single image is a memory attack rather than a picture. Checked
/// from the header, before the decode allocates. It is not the only line: the decoded size is
/// limited too (`MAX_DECODE_ALLOC`, 512 MiB), which for the usual 8-bit RGBA means about 1.34e8
/// pixels (an 11,500 px square), so this count is what limits 8-bit gray and RGB images, and
/// RGBA ones are held a little lower by the byte limit.
pub(crate) const MAX_IMAGE_PIXELS: u64 = 150_000_000;

/// The decoder's own allocation limit, bytes — the image crate's default, stated here so it is
/// pinned instead of inherited (a change in the crate must not silently loosen it).
pub(crate) const MAX_DECODE_ALLOC: u64 = 512 * 1024 * 1024;

fn decode_limits() -> image::Limits {
    let mut l = image::Limits::default();
    l.max_image_width = Some(MAX_IMAGE_SIDE);
    l.max_image_height = Some(MAX_IMAGE_SIDE);
    l.max_alloc = Some(MAX_DECODE_ALLOC);
    l
}

/// Why a raster image could not be shown. Carried to the screen so that "too large" and "damaged"
/// are not both reported as a problem with the terminal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageFailure {
    /// Over `MAX_IMAGE_SIDE` / `MAX_IMAGE_PIXELS` / the decoder's allocation limit (decided from the
    /// header, before anything is allocated).
    TooLarge,
    /// The file is damaged, truncated, or could not be read.
    Corrupt,
    /// The format is not one konoma can decode (or the bytes are not an image at all).
    UnsupportedFormat,
    /// The request was dropped before it ran because nobody wants the result any more (the
    /// document was closed, or the preview moved on). Never shown.
    Cancelled,
    /// An SVG file that was refused or stopped by the guard or by the supervisor of its drawing
    /// process (nested too deeply, took too long, ...): one reason type for every picture.
    Svg(super::svg_guard::SvgFail),
}

impl ImageFailure {
    /// A stable text form, for the places that carry a failure as a `String`.
    pub fn code(self) -> &'static str {
        match self {
            Self::TooLarge => "too-large",
            Self::Corrupt => "corrupt",
            Self::UnsupportedFormat => "unsupported-format",
            Self::Cancelled => "cancelled",
            Self::Svg(f) => f.code_text(),
        }
    }

    /// The inverse of `code` (None for any other text).
    pub fn from_code(s: &str) -> Option<Self> {
        [
            Self::TooLarge,
            Self::Corrupt,
            Self::UnsupportedFormat,
            Self::Cancelled,
        ]
        .into_iter()
        .chain(super::svg_guard::SvgFail::ALL.into_iter().map(Self::Svg))
        .find(|f| f.code() == s)
    }

    /// The translated reason shown in place of the picture (None for `Cancelled`, which is never
    /// shown).
    pub fn message(self, lang: crate::i18n::Lang) -> Option<&'static str> {
        use crate::i18n::{tr, Msg};
        Some(tr(
            lang,
            match self {
                Self::TooLarge => Msg::ImageReasonTooLarge,
                Self::Corrupt => Msg::ImageReasonCorrupt,
                Self::UnsupportedFormat => Msg::ImageReasonUnsupportedFormat,
                Self::Cancelled => return None,
                Self::Svg(f) => f.msg(),
            },
        ))
    }

    fn from_image_error(e: &image::ImageError) -> Self {
        match e {
            image::ImageError::Limits(_) => Self::TooLarge,
            image::ImageError::Unsupported(_) => Self::UnsupportedFormat,
            _ => Self::Corrupt,
        }
    }
}

/// How urgently a queued decode is wanted, and whether it is still wanted at all. One ticket
/// belongs to one request (an inline Markdown picture, a full-screen load) and is shared with the
/// thread that decodes it: the decode gate reads it while the request waits its turn.
///
/// * `priority` — larger runs first. An inline picture stamps the number of the overlay pass that
///   last drew it (so what is on screen now beats what scrolled away); a full-screen load uses
///   `u64::MAX`.
/// * stale — the request was `cancel`led (its cache entry was dropped) or the generation it was
///   made under has moved on. A stale request leaves the queue without decoding.
pub(crate) struct DecodeTicket {
    priority: AtomicU64,
    cancelled: AtomicBool,
    generation: Option<(Arc<AtomicU64>, u64)>,
}

impl DecodeTicket {
    pub(crate) fn new(priority: u64) -> Self {
        Self {
            priority: AtomicU64::new(priority),
            cancelled: AtomicBool::new(false),
            generation: None,
        }
    }

    /// A ticket that goes stale when `shared` no longer equals `gen` (the media worker's
    /// `media_gen`: moving to another file bumps it).
    pub(crate) fn for_generation(priority: u64, shared: Arc<AtomicU64>, gen: u64) -> Self {
        Self {
            priority: AtomicU64::new(priority),
            cancelled: AtomicBool::new(false),
            generation: Some((shared, gen)),
        }
    }

    pub(crate) fn set_priority(&self, p: u64) {
        self.priority.store(p, Ordering::Relaxed);
    }

    pub(crate) fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }

    pub(crate) fn is_stale(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
            || self
                .generation
                .as_ref()
                .is_some_and(|(shared, gen)| shared.load(Ordering::Relaxed) != *gen)
    }

    pub(crate) fn priority(&self) -> u64 {
        self.priority.load(Ordering::Relaxed)
    }
}

impl Default for DecodeTicket {
    fn default() -> Self {
        Self::new(0)
    }
}

/// Owner of a request's `DecodeTicket`: dropping it (the cache entry that wanted the picture went
/// away) cancels the request, so a decode still waiting in the gate's queue never runs.
#[derive(Default)]
pub(crate) struct DecodeWish(Arc<DecodeTicket>);

impl DecodeWish {
    pub(crate) fn ticket(&self) -> Arc<DecodeTicket> {
        self.0.clone()
    }

    pub(crate) fn set_priority(&self, p: u64) {
        self.0.set_priority(p);
    }

    /// Test-only: withdraw the request as dropping the owning entry would.
    #[cfg(test)]
    pub(crate) fn cancel_now(&self) {
        self.0.cancel();
    }
}

impl Drop for DecodeWish {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

thread_local! {
    /// The ticket of the request this worker thread is running (see `with_decode_ticket`).
    static CURRENT_TICKET: std::cell::RefCell<Option<Arc<DecodeTicket>>> =
        const { std::cell::RefCell::new(None) };
}

/// Run `f` as the decode of the request `ticket` stands for: every claim on the decode memory it
/// makes waits in the gate's queue under that ticket (priority order, dropped when stale).
pub(crate) fn with_decode_ticket<R>(ticket: Arc<DecodeTicket>, f: impl FnOnce() -> R) -> R {
    let prev = CURRENT_TICKET.with(|c| c.replace(Some(ticket)));
    let out = f();
    CURRENT_TICKET.with(|c| *c.borrow_mut() = prev);
    out
}

fn current_ticket() -> Option<Arc<DecodeTicket>> {
    CURRENT_TICKET.with(|c| c.borrow().clone())
}

/// Total decoded-pixel memory that background decodes may hold at once, in bytes. The claim of one
/// decode is its **measured peak** (`decode_peak_bytes`), so this admits two 48-megapixel photos
/// or a few dozen screenshots side by side but not sixteen 11,000 px images (the case that used to
/// reach 10 GB). A single request larger than the budget is let through alone rather than refused.
pub(crate) const DECODE_MEMORY_BUDGET: u64 = 1024 * 1024 * 1024;

/// How long a waiting decode sleeps before it looks again at whether it is still wanted and
/// whether its priority changed (nothing signals those).
const GATE_RECHECK: Duration = Duration::from_millis(50);

/// A pool of bytes that decodes claim while they run, handed out **in priority order**. A struct
/// (not just statics) so tests can use a small private pool.
///
/// Waiting requests are served best-first: highest `DecodeTicket` priority, then arrival order. A
/// request that does not fit yet holds the head of the queue — smaller requests behind it do not
/// jump past it, so a large picture is never starved by a stream of small ones.
pub(crate) struct DecodeGate {
    state: std::sync::Mutex<GateState>,
    freed: std::sync::Condvar,
    budget: u64,
}

struct GateState {
    used: u64,
    next_seq: u64,
    waiting: Vec<Waiter>,
}

struct Waiter {
    seq: u64,
    ticket: Option<Arc<DecodeTicket>>,
}

impl Waiter {
    fn priority(&self) -> u64 {
        self.ticket.as_ref().map_or(u64::MAX, |t| t.priority())
    }

    fn stale(&self) -> bool {
        self.ticket.as_ref().is_some_and(|t| t.is_stale())
    }
}

static DECODE_GATE: DecodeGate = DecodeGate::new(DECODE_MEMORY_BUDGET);

#[cfg(test)]
thread_local! {
    /// Test-only: a private gate this thread's decodes use instead of the process-wide one, so a
    /// test can fill the pool exactly without stalling every other test that decodes.
    static TEST_GATE: std::cell::Cell<Option<&'static DecodeGate>> =
        const { std::cell::Cell::new(None) };
}

/// Run `f` with this thread's decodes claiming from `gate`.
#[cfg(test)]
pub(crate) fn with_test_gate<R>(gate: &'static DecodeGate, f: impl FnOnce() -> R) -> R {
    TEST_GATE.with(|c| c.set(Some(gate)));
    let out = f();
    TEST_GATE.with(|c| c.set(None));
    out
}

/// The gate decodes on this thread claim from.
fn gate() -> &'static DecodeGate {
    #[cfg(test)]
    if let Some(g) = TEST_GATE.with(|c| c.get()) {
        return g;
    }
    &DECODE_GATE
}

thread_local! {
    /// This thread already holds a reservation (a nested request must not wait on itself).
    static HOLDING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// A claim on a `DecodeGate`; the bytes return to the pool when it is dropped.
pub(crate) struct DecodeReservation {
    gate: &'static DecodeGate,
    bytes: u64,
    /// This claim is the one that marked its thread as holding (`HOLDING`), so dropping it
    /// unmarks it — even when it claimed no bytes (a zero-byte request). A nested claim, which
    /// waits for nothing, is not.
    marks_thread: bool,
}

impl DecodeGate {
    pub(crate) const fn new(budget: u64) -> Self {
        Self {
            state: std::sync::Mutex::new(GateState {
                used: 0,
                next_seq: 0,
                waiting: Vec::new(),
            }),
            freed: std::sync::Condvar::new(),
            budget,
        }
    }

    /// Wait until `bytes` fit and claim them, with no ticket (never stale, top priority). A request
    /// larger than the whole budget waits for an empty pool and then runs alone. Only call from a
    /// worker thread — it can block.
    pub(crate) fn reserve(&'static self, bytes: u64) -> DecodeReservation {
        match self.reserve_for(bytes, None) {
            Ok(r) => r,
            // Unreachable without a ticket; an empty claim is the harmless answer.
            Err(_) => DecodeReservation {
                gate: self,
                bytes: 0,
                marks_thread: false,
            },
        }
    }

    /// `reserve` for the request `ticket` stands for. `Err(Cancelled)` when the request went stale
    /// while it waited (it never claimed anything).
    pub(crate) fn reserve_for(
        &'static self,
        bytes: u64,
        ticket: Option<Arc<DecodeTicket>>,
    ) -> Result<DecodeReservation, ImageFailure> {
        if HOLDING.with(|h| h.get()) {
            return Ok(DecodeReservation {
                gate: self,
                bytes: 0,
                marks_thread: false,
            });
        }
        let want = bytes.min(self.budget);
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let seq = st.next_seq;
        st.next_seq += 1;
        st.waiting.push(Waiter {
            seq,
            ticket: ticket.clone(),
        });
        loop {
            let stale = ticket.as_ref().is_some_and(|t| t.is_stale());
            if stale {
                st.waiting.retain(|w| w.seq != seq);
                self.freed.notify_all();
                return Err(ImageFailure::Cancelled);
            }
            // Requests nobody wants leave the queue so they cannot hold the head.
            st.waiting.retain(|w| w.seq == seq || !w.stale());
            let head = st
                .waiting
                .iter()
                .min_by_key(|w| (std::cmp::Reverse(w.priority()), w.seq))
                .map(|w| w.seq);
            if head == Some(seq) && (st.used == 0 || st.used + want <= self.budget) {
                st.waiting.retain(|w| w.seq != seq);
                st.used += want;
                HOLDING.with(|h| h.set(true));
                // The next in line may fit beside this one.
                self.freed.notify_all();
                return Ok(DecodeReservation {
                    gate: self,
                    bytes: want,
                    marks_thread: true,
                });
            }
            st = self
                .freed
                .wait_timeout(st, GATE_RECHECK)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }

    #[cfg(test)]
    pub(crate) fn in_use(&self) -> u64 {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).used
    }

    #[cfg(test)]
    pub(crate) fn waiting(&self) -> usize {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .waiting
            .len()
    }
}

impl Drop for DecodeReservation {
    fn drop(&mut self) {
        if self.marks_thread {
            HOLDING.with(|h| h.set(false));
        }
        if self.bytes == 0 {
            return;
        }
        let mut st = self.gate.state.lock().unwrap_or_else(|e| e.into_inner());
        st.used = st.used.saturating_sub(self.bytes);
        self.gate.freed.notify_all();
    }
}

/// Claim `bytes` of the shared decode budget (see `DECODE_MEMORY_BUDGET`).
pub(crate) fn reserve_decode_memory(bytes: u64) -> DecodeReservation {
    gate().reserve(bytes)
}

/// `reserve_decode_memory` under the ticket of the request this thread is running
/// (`with_decode_ticket`): waits in priority order and gives up when the request went stale.
fn try_reserve_decode_memory(bytes: u64) -> Result<DecodeReservation, ImageFailure> {
    gate().reserve_for(bytes, current_ticket())
}

/// Peak resident bytes of decoding an image whose decoded pixels take `native` bytes, measured
/// (release build, `/usr/bin/time -l`, then shrunk to 4096 px with `shrink_exact`): baseline JPEG
/// 1.4-1.75x, progressive JPEG 2.4x (the decoder keeps every coefficient), WebP 1.85x, TIFF 2.15x,
/// PNG 1.1-1.2x (8- and 16-bit). PNG is claimed at 2x and every other format at 3x, so no measured
/// case exceeds its claim; the shrink adds only a few rows on top (it used to add 17 bytes per
/// source pixel).
pub(crate) fn decode_peak_bytes(format: Option<image::ImageFormat>, native: u64) -> u64 {
    let factor = if format == Some(image::ImageFormat::Png) {
        2
    } else {
        3
    };
    native.saturating_mul(factor)
}

/// Decode a still image from `reader`. The header is read first: an image whose declared size is
/// over the limits is refused before anything is allocated, and the shared decode budget is
/// claimed (waiting its turn, by priority) for the decode. `max_side`, when set, shrinks the
/// result (still inside the claim) so a caller that keeps many images does not keep their full
/// size. The refusal says why (`ImageFailure`).
fn decode_reader<R: std::io::BufRead + std::io::Seek>(
    mut reader: image::ImageReader<R>,
    max_side: Option<u32>,
) -> Result<DynamicImage, ImageFailure> {
    reader.limits(decode_limits());
    let format = reader.format();
    let decoder = reader
        .into_decoder()
        .map_err(|e| ImageFailure::from_image_error(&e))?;
    let (w, h) = decoder.dimensions();
    // The decoded size is decided by the header too, whichever decoder it is (not every decoder
    // applies `max_alloc` to its output buffer: a 16-bit PNG used to slip past it).
    if !within_decode_limits(w, h, decoder.total_bytes()) {
        return Err(ImageFailure::TooLarge);
    }
    let _claim = try_reserve_decode_memory(decode_peak_bytes(format, decoder.total_bytes()))?;
    let img =
        DynamicImage::from_decoder(decoder).map_err(|e| ImageFailure::from_image_error(&e))?;
    Ok(match max_side {
        Some(m) if img.width().max(img.height()) > m => shrink_to_fit(&img, m),
        _ => img,
    })
}

// ---- Shrinking ------------------------------------------------------------------------------

/// The size `(w, h)` scaled down to fit a `max_side` square, aspect ratio kept (at least 1 px).
pub(crate) fn fit_within(w: u32, h: u32, max_side: u32) -> (u32, u32) {
    if w == 0 || h == 0 {
        // Nothing to scale (a decoder can hand back an empty picture): the smallest picture.
        return (1, 1);
    }
    if w >= h {
        let nh = (u64::from(h) * u64::from(max_side) + u64::from(w) / 2) / u64::from(w);
        (max_side, (nh as u32).max(1))
    } else {
        let nw = (u64::from(w) * u64::from(max_side) + u64::from(h) / 2) / u64::from(h);
        ((nw as u32).max(1), max_side)
    }
}

/// `img` shrunk so its longer side is `max_side` (Triangle filter, 8-bit result).
pub(crate) fn shrink_to_fit(img: &DynamicImage, max_side: u32) -> DynamicImage {
    let (nw, nh) = fit_within(img.width(), img.height(), max_side);
    shrink_exact(img, nw, nh)
}

/// The Triangle (bilinear, box widened by the scale) weights of output index `out_i` over `in_len`
/// input samples at `ratio` input per output: `(first input index, weights)`. The same filter and
/// normalisation as `image::imageops::resize`, so the picture matches what it used to give.
fn triangle_weights(out_i: u32, ratio: f32, in_len: u32) -> (u32, Vec<f32>) {
    let sratio = ratio.max(1.0);
    let support = sratio;
    let center = (out_i as f32 + 0.5) * ratio;
    let left = ((center - support).floor() as i64).clamp(0, i64::from(in_len) - 1) as u32;
    let right =
        ((center + support).ceil() as i64).clamp(i64::from(left) + 1, i64::from(in_len)) as u32;
    let mut w: Vec<f32> = (left..right)
        .map(|i| {
            let x = (i as f32 - center + 0.5) / sratio;
            (1.0 - x.abs()).max(0.0)
        })
        .collect();
    let sum: f32 = w.iter().sum();
    if sum > 0.0 {
        for v in &mut w {
            *v /= sum;
        }
    } else {
        // Degenerate (cannot happen for a shrink): nearest sample.
        let n = w.len();
        for (k, v) in w.iter_mut().enumerate() {
            *v = if k == n / 2 { 1.0 } else { 0.0 };
        }
    }
    (left, w)
}

/// The samples of a decoded image as a flat slice plus the number of channels, whatever the sample
/// type: shrinking reads them one row at a time and never converts the whole picture.
enum Samples<'a> {
    U8(&'a [u8]),
    U16(&'a [u16]),
    F32(&'a [f32]),
}

impl Samples<'_> {
    /// Sample `i` as a value in 0..=255.
    #[inline]
    fn get(&self, i: usize) -> f32 {
        match self {
            Samples::U8(s) => f32::from(s[i]),
            Samples::U16(s) => f32::from(s[i]) / 257.0,
            Samples::F32(s) => s[i] * 255.0,
        }
    }
}

/// Shrink `img` to exactly `nw` x `nh` with the Triangle filter, **streaming**: the picture is
/// read one row at a time and only a few rows of f32 working data exist besides the 8-bit result.
/// (`DynamicImage::resize` first builds a whole f32 RGBA intermediate — 16 bytes per source column
/// per output row, 17 bytes per pixel in all for a 48-megapixel photo.) The result is 8-bit
/// whatever the source depth — it is only ever drawn.
pub(crate) fn shrink_exact(img: &DynamicImage, nw: u32, nh: u32) -> DynamicImage {
    let (w, h) = (img.width(), img.height());
    let (nw, nh) = (nw.max(1), nh.max(1));
    if w == 0 || h == 0 {
        // No samples to read (the weights below would divide by the empty length): a blank picture
        // of the size asked for.
        return DynamicImage::ImageRgba8(image::RgbaImage::new(nw, nh));
    }
    let (channels, samples) = match img {
        DynamicImage::ImageLuma8(b) => (1, Samples::U8(b.as_raw())),
        DynamicImage::ImageLumaA8(b) => (2, Samples::U8(b.as_raw())),
        DynamicImage::ImageRgb8(b) => (3, Samples::U8(b.as_raw())),
        DynamicImage::ImageRgba8(b) => (4, Samples::U8(b.as_raw())),
        DynamicImage::ImageLuma16(b) => (1, Samples::U16(b.as_raw())),
        DynamicImage::ImageLumaA16(b) => (2, Samples::U16(b.as_raw())),
        DynamicImage::ImageRgb16(b) => (3, Samples::U16(b.as_raw())),
        DynamicImage::ImageRgba16(b) => (4, Samples::U16(b.as_raw())),
        DynamicImage::ImageRgb32F(b) => (3, Samples::F32(b.as_raw())),
        DynamicImage::ImageRgba32F(b) => (4, Samples::F32(b.as_raw())),
        // A sample type this does not know: go through RGBA8 (a copy, but the only way).
        other => return shrink_exact(&DynamicImage::ImageRgba8(other.to_rgba8()), nw, nh),
    };
    let c = channels as usize;
    let (xr, yr) = (w as f32 / nw as f32, h as f32 / nh as f32);
    let xw: Vec<(u32, Vec<f32>)> = (0..nw).map(|x| triangle_weights(x, xr, w)).collect();
    let row_len = nw as usize * c;
    // Source rows already resampled horizontally, oldest first: `(source row, nw*c values)`.
    let mut rows: std::collections::VecDeque<(u32, Vec<f32>)> = std::collections::VecDeque::new();
    // Row buffers that fell out of the window, reused for the next rows (no allocation per row).
    let mut spare: Vec<Vec<f32>> = Vec::new();
    let mut out = vec![0u8; nh as usize * row_len];
    let mut acc = vec![0f32; row_len];
    // One source row as f32 (the sample type is matched once per sample here, not per tap).
    let mut src_row = vec![0f32; w as usize * c];
    for y in 0..nh {
        let (top, yw) = triangle_weights(y, yr, h);
        while rows.front().is_some_and(|(r, _)| *r < top) {
            if let Some((_, buf)) = rows.pop_front() {
                spare.push(buf);
            }
        }
        acc.iter_mut().for_each(|v| *v = 0.0);
        for (k, wy) in yw.iter().enumerate() {
            let r = top + k as u32;
            if rows.back().is_none_or(|(last, _)| *last < r) {
                let base = r as usize * w as usize * c;
                for (i, v) in src_row.iter_mut().enumerate() {
                    *v = samples.get(base + i);
                }
                let mut line = spare.pop().unwrap_or_else(|| vec![0f32; row_len]);
                for (x, (left, ws)) in xw.iter().enumerate() {
                    for ch in 0..c {
                        let mut s = 0f32;
                        for (j, wx) in ws.iter().enumerate() {
                            s += wx * src_row[(*left as usize + j) * c + ch];
                        }
                        line[x * c + ch] = s;
                    }
                }
                rows.push_back((r, line));
            }
            let line = &rows
                .iter()
                .find(|(rr, _)| *rr == r)
                .expect("row was just resampled")
                .1;
            for (a, v) in acc.iter_mut().zip(line) {
                *a += wy * v;
            }
        }
        let dst = &mut out[y as usize * row_len..(y as usize + 1) * row_len];
        for (d, a) in dst.iter_mut().zip(&acc) {
            *d = a.round().clamp(0.0, 255.0) as u8;
        }
    }
    match channels {
        1 => DynamicImage::ImageLuma8(image::GrayImage::from_raw(nw, nh, out).expect("sized")),
        2 => {
            DynamicImage::ImageLumaA8(image::GrayAlphaImage::from_raw(nw, nh, out).expect("sized"))
        }
        3 => DynamicImage::ImageRgb8(image::RgbImage::from_raw(nw, nh, out).expect("sized")),
        _ => DynamicImage::ImageRgba8(image::RgbaImage::from_raw(nw, nh, out).expect("sized")),
    }
}

/// Whether an image of `(w, h)` pixels is within what konoma decodes (`MAX_IMAGE_SIDE`,
/// `MAX_IMAGE_PIXELS`) — decided from a header alone, so a caller can refuse before queueing.
pub fn dimensions_within_limits((w, h): (u32, u32)) -> bool {
    w <= MAX_IMAGE_SIDE && h <= MAX_IMAGE_SIDE && u64::from(w) * u64::from(h) <= MAX_IMAGE_PIXELS
}

/// Whether an image of `w` x `h` pixels that decodes to `decoded_bytes` is within every limit
/// (side, pixel count, decoded size) — what `decode_reader` asks of a header.
pub(crate) fn within_decode_limits(w: u32, h: u32, decoded_bytes: u64) -> bool {
    dimensions_within_limits((w, h)) && decoded_bytes <= MAX_DECODE_ALLOC
}

/// Decode a still image (PNG/JPG/the first frame of a GIF, etc.). None on failure.
/// A pure function used both by media loading on a separate thread and by load_image on the UI thread.
pub fn decode_static(path: &Path) -> Option<DynamicImage> {
    decode_static_why(path).ok()
}

/// `decode_static` that says why it failed.
pub fn decode_static_why(path: &Path) -> Result<DynamicImage, ImageFailure> {
    decode_reader(open_reader(path)?, None)
}

/// `decode_static` for an inline Markdown image: the result is shrunk so its longer side is at most
/// `max_side`. A document keeps every decoded image for as long as it is open, so what is kept is
/// what a terminal can show (`MD_IMAGE_MAX_SIDE`), not the 100-megapixel original. Images already
/// within the bound are returned untouched.
#[cfg(test)]
pub fn decode_static_capped(path: &Path, max_side: u32) -> Option<DynamicImage> {
    decode_static_capped_why(path, max_side).ok()
}

/// `decode_static_capped` that says why it failed.
pub fn decode_static_capped_why(path: &Path, max_side: u32) -> Result<DynamicImage, ImageFailure> {
    decode_reader(open_reader(path)?, Some(max_side))
}

fn open_reader(
    path: &Path,
) -> Result<image::ImageReader<std::io::BufReader<std::fs::File>>, ImageFailure> {
    image::ImageReader::open(path)
        .map_err(|_| ImageFailure::Corrupt)?
        .with_guessed_format()
        .map_err(|_| ImageFailure::Corrupt)
}

/// `decode_static`, from bytes already in memory rather than a path — used by the media-diff worker
/// (`app/media_diff.rs::decode_image_side`) to decode a side whose bytes came from git/jj rather than
/// the filesystem. Format is guessed from the content, exactly like the path version's
/// `with_guessed_format` (never from an extension — there may be none to go by).
pub fn decode_static_bytes(bytes: &[u8]) -> Option<DynamicImage> {
    let reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .ok()?;
    decode_reader(reader, None).ok()
}

/// `decode_static_capped_why`, from bytes already in memory (a picture inside a Word /
/// OpenDocument file): the same header checks, memory claim and shrink to `max_side`, and the
/// reason when it cannot be shown.
pub fn decode_static_bytes_capped_why(
    bytes: &[u8],
    max_side: u32,
) -> Result<DynamicImage, ImageFailure> {
    let reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|_| ImageFailure::Corrupt)?;
    decode_reader(reader, Some(max_side))
}

/// Read only the pixel dimensions of an image, sniffing the format from the file's content (not its
/// extension). Used for inline Markdown images — including fetched remote images cached without an
/// extension — to reserve layout rows without decoding the whole file. None if it is not an image.
pub fn dimensions(path: &Path) -> Option<(u32, u32)> {
    image::ImageReader::open(path)
        .ok()?
        .with_guessed_format()
        .ok()?
        .into_dimensions()
        .ok()
}

/// Minimum display time for each GIF frame. Anything below this (including 0) is treated as 100ms
/// (a GIF delay=0 means "as fast as possible"; like browsers, snap to 100ms to prevent runaway).
const MIN_FRAME_DELAY: Duration = Duration::from_millis(20);
const DEFAULT_FRAME_DELAY: Duration = Duration::from_millis(100);

/// Upper bound on the RGBA bytes kept resident for an animated GIF (all frames stay expanded for
/// smooth cycling — that is the animation design). When the total would exceed this, every frame is
/// downscaled in halving steps: the animation keeps all frames, trading pixels for memory. Typical
/// GIFs are far below the bound and stay untouched; only pathological ones (e.g. 1080p × hundreds
/// of frames ≈ 500MB+) are reduced instead of ballooning resident memory.
const MAX_GIF_BYTES: usize = 128 * 1024 * 1024;

/// Largest GIF logical screen, in pixels. A screen recording of a 4K display is 8.3e6; 3.6e7
/// (6000 x 6000) leaves room, and a crafted header of 65535 x 65535 (4.3e9 px, 17 GB per frame as
/// RGBA) is far past it.
const MAX_GIF_CANVAS_PIXELS: u64 = 36_000_000;

/// Whether a GIF logical screen of `(w, h)` is within `MAX_GIF_CANVAS_PIXELS`.
pub(crate) fn gif_canvas_within_limit((w, h): (u32, u32)) -> bool {
    u64::from(w) * u64::from(h) <= MAX_GIF_CANVAS_PIXELS
}

/// Most frames of one GIF konoma expands. Real animations are a few hundred at most.
const MAX_GIF_FRAMES: usize = 5_000;

/// Frames x canvas pixels konoma will composite for one GIF. Calibrated by timing the decode on a
/// release build (Apple M-series, one worker thread): 12-17 ns per pixel for 800 x 600 and 1080p
/// frames, up to 25 ns for 4K ones (800 x 600 x 300 frames = 1.4e8 took 1.7 s, 1080p x 150 = 3.1e8
/// 3.5 s — 5.4 s through the inline path — and 4K x 40 = 3.3e8 8.3 s). 4e8 lets all of those through
/// with margin (1080p up to 192 frames, 720p 434, 4K 48 — far past what a GIF of a sensible file
/// size holds) and keeps the worst accepted file to about 5-10 seconds on a worker thread. The
/// earlier 4e9 let a 1000 x 1000 x 4000-frame file run for 53-71 seconds.
pub(crate) const MAX_GIF_WORK_PIXELS: u64 = 400_000_000;

/// Number of images (frames) in the GIF `r` holds, counted from the block structure alone — the
/// compressed pixel data is skipped, not decoded, so this takes milliseconds where decoding every
/// frame of a hostile file takes as long as `MAX_GIF_WORK_PIXELS` allows. Counting stops once it
/// passes `cap` (the answer is then `cap + 1`). None when the stream is not a GIF or ends early —
/// the decoder gets to say what is wrong with it. The reader is left at an unspecified position.
pub(crate) fn count_gif_frames<R: std::io::Read + std::io::Seek>(
    r: &mut R,
    cap: usize,
    stale: &dyn Fn() -> bool,
) -> Option<usize> {
    use std::io::SeekFrom;
    fn byte<R: std::io::Read>(r: &mut R) -> Option<u8> {
        let mut b = [0u8; 1];
        r.read_exact(&mut b).ok()?;
        Some(b[0])
    }
    // Read and drop `n` bytes. Read, not seek: a `BufReader` throws its buffer away on every seek,
    // so skipping a million one-byte sub-blocks by seeking re-reads 8 KiB a time (a 20 MB file took
    // six seconds); reading keeps the buffer and the cost proportional to the file.
    fn skip<R: std::io::Read>(r: &mut R, n: usize) -> Option<()> {
        let mut sink = [0u8; 768]; // the largest colour table: 3 << 8
        r.read_exact(&mut sink[..n]).ok()
    }
    // Skip a chain of data sub-blocks (each a length byte then that many bytes, ended by 0). The
    // caller's `stale` is looked at now and then, so a request that was abandoned stops reading.
    fn skip_sub_blocks<R: std::io::Read>(
        r: &mut R,
        steps: &mut u32,
        stale: &dyn Fn() -> bool,
    ) -> Option<()> {
        loop {
            let n = byte(r)?;
            if n == 0 {
                return Some(());
            }
            skip(r, usize::from(n))?;
            *steps = steps.wrapping_add(1);
            if (*steps).is_multiple_of(65_536) && stale() {
                return None;
            }
        }
    }
    r.seek(SeekFrom::Start(0)).ok()?;
    let mut head = [0u8; 13]; // signature, version, logical screen descriptor
    r.read_exact(&mut head).ok()?;
    if &head[..3] != b"GIF" {
        return None;
    }
    if head[10] & 0x80 != 0 {
        skip(r, 3usize << ((head[10] & 7) + 1))?;
    }
    let mut frames = 0usize;
    let mut steps = 0u32;
    loop {
        match byte(r)? {
            0x3B => return Some(frames),
            0x21 => {
                byte(r)?; // extension label
                skip_sub_blocks(r, &mut steps, stale)?;
            }
            0x2C => {
                let mut desc = [0u8; 9];
                r.read_exact(&mut desc).ok()?;
                if desc[8] & 0x80 != 0 {
                    skip(r, 3usize << ((desc[8] & 7) + 1))?;
                }
                byte(r)?; // LZW minimum code size
                skip_sub_blocks(r, &mut steps, stale)?;
                frames += 1;
                if frames > cap {
                    return Some(frames);
                }
            }
            _ => return None,
        }
    }
}

/// Whether compositing `frames` frames of a `canvas_px`-pixel logical screen is within
/// `MAX_GIF_WORK_PIXELS`, and the frame count within `MAX_GIF_FRAMES`.
pub(crate) fn gif_work_within_limit(canvas_px: u64, frames: usize) -> bool {
    frames <= MAX_GIF_FRAMES && (frames as u64).saturating_mul(canvas_px) <= MAX_GIF_WORK_PIXELS
}

/// Expand a GIF into all frames (composited RGBA) plus their display times.
/// Returns None if it is not a GIF / decoding fails / there is only one frame (= treated as a still image),
/// and the caller falls back to the normal still-image loader (load_image).
pub fn decode_gif(path: &Path) -> Option<GifFrames> {
    decode_gif_with_budget(path, MAX_GIF_BYTES)
}

/// The frame target for shrink factor `shrink` against the original canvas (min 1px per side).
fn shrink_target(w: u32, h: u32, shrink: u32) -> (u32, u32) {
    ((w / shrink).max(1), (h / shrink).max(1))
}

/// Upper bound on the RGBA bytes kept resident for an animated GIF **embedded inline in a Markdown
/// document** (`decode_gif_inline`). Smaller than the full-screen bound (`MAX_GIF_BYTES`): a single
/// document can embed several GIFs at once, each decoded independently and kept expanded for as
/// long as the document is open, so per-image memory needs to stay tighter to keep the total bounded.
pub(crate) const MAX_GIF_BYTES_INLINE: usize = 32 * 1024 * 1024;

/// `decode_gif`, budgeted for an inline Markdown image (see `MAX_GIF_BYTES_INLINE`). Same semantics:
/// None for a non-GIF / undecodable / single-frame GIF — the caller (the inline-image decode worker)
/// falls back to the normal still-image decode, which already handles those cases.
pub fn decode_gif_inline(path: &Path) -> Option<GifFrames> {
    decode_gif_with_budget(path, MAX_GIF_BYTES_INLINE)
}

/// `decode_gif` with an explicit byte budget (separated so tests can force the shrink path with a
/// tiny budget). Frames are decoded one at a time; when the running total exceeds the budget the
/// shrink factor doubles and the already-kept frames are downscaled to the same target, so every
/// frame ends up with identical dimensions (as the animation cycler expects).
fn decode_gif_with_budget(path: &Path, budget: usize) -> Option<GifFrames> {
    let file = std::fs::File::open(path).ok()?;
    decode_gif_from_reader(std::io::BufReader::new(file), budget).map(|(frames, _canvas)| frames)
}

/// `decode_gif_inline`, from bytes already in memory — used by the media-diff worker to animate an
/// old (git/jj) version of a GIF, without a path to read from. Same semantics: None for a non-GIF /
/// undecodable / single-frame GIF. Also returns the GIF's own logical-screen size (read from the
/// header) — distinct from any frame's own pixel size once the decode budget has downscaled frames
/// for memory — which `app::media_diff::decode_image_side` needs as this side's *intrinsic* size
/// (see `MediaDiffPictureDecoded::natural_px`); `decode_gif`/`decode_gif_inline` have no such need
/// (no other side's size to stay comparable with) and so drop it.
pub fn decode_gif_bytes_inline(bytes: &[u8]) -> Option<(GifFrames, (u32, u32))> {
    decode_gif_from_reader(std::io::Cursor::new(bytes), MAX_GIF_BYTES_INLINE)
}

/// The shared body of `decode_gif_with_budget`/`decode_gif_bytes_inline`, generic over the reader so
/// neither has to duplicate the frame/shrink loop. The `(u32, u32)` alongside the frames is the GIF's
/// own logical-screen size, read from the header before any frame is decoded — see
/// `decode_gif_bytes_inline`'s own doc comment for why that's the intrinsic size a caller comparing
/// this GIF's size against something else needs.
fn decode_gif_from_reader<R: std::io::Read + std::io::BufRead + std::io::Seek>(
    reader: R,
    budget: usize,
) -> Option<(GifFrames, (u32, u32))> {
    let mut reader = reader;
    let ticket = current_ticket();
    let scanned_frames = count_gif_frames(&mut reader, MAX_GIF_FRAMES, &|| {
        ticket.as_ref().is_some_and(|t| t.is_stale())
    })?;
    reader.seek(std::io::SeekFrom::Start(0)).ok()?;
    let mut decoder = image::codecs::gif::GifDecoder::new(reader).ok()?;
    let header_px = decoder.dimensions(); // before `into_frames()` consumes `decoder` below.
                                          // The logical screen is what every frame is composited onto, so it — not the frames' own
                                          // rectangles — decides the memory. Refuse an oversized one before the first frame allocates.
    let canvas_px = u64::from(header_px.0) * u64::from(header_px.1);
    if !gif_canvas_within_limit(header_px) {
        return None;
    }
    // Frames x canvas is the compositing work however small the kept copies become. The frames
    // were counted from the block structure above, so a GIF over the limit is refused here, in
    // milliseconds, instead of after `MAX_GIF_WORK_PIXELS` of compositing.
    if !gif_work_within_limit(canvas_px, scanned_frames) {
        return None;
    }
    decoder.set_limits(decode_limits()).ok()?;
    // A frame being composited, the previous canvas, and the resized copy — plus the frames kept
    // so far, which fill up to `budget` before the shrink halves them.
    let _claim = try_reserve_decode_memory(canvas_px * 4 * 3 + budget as u64).ok()?;
    let mut out: GifFrames = Vec::new();
    let mut canvas: Option<(u32, u32)> = None; // original canvas dimensions (baseline for the shrink factor)
    let mut shrink = 1u32;
    let mut bytes = 0usize;
    for (n, f) in decoder.into_frames().enumerate() {
        // Nobody wants this GIF any more (document closed, preview moved on): stop.
        if ticket.as_ref().is_some_and(|t| t.is_stale()) {
            return None;
        }
        if n >= MAX_GIF_FRAMES || !gif_work_within_limit(canvas_px, n + 1) {
            return None;
        }
        // Same as the old collect_frames: if even one frame is corrupt, return None = fall back
        // to a still image.
        let f = f.ok()?;
        let delay: Duration = f.delay().into();
        let delay = if delay < MIN_FRAME_DELAY {
            DEFAULT_FRAME_DELAY
        } else {
            delay
        };
        let mut img = DynamicImage::ImageRgba8(f.into_buffer());
        let (cw, ch) = *canvas.get_or_insert((img.width(), img.height()));
        if shrink > 1 {
            let (tw, th) = shrink_target(cw, ch, shrink);
            img = shrink_exact(&img, tw, th);
        }
        bytes += (img.width() as usize) * (img.height() as usize) * 4;
        out.push((img, delay));
        // Over budget: double the shrink factor and re-shrink the existing frames to the same
        // target dimensions too.
        // (The 1<<16 guard means it never loops forever even with a pathological budget. Once it
        // has shrunk down to 1px, give up and keep it as-is.)
        while bytes > budget && shrink < (1 << 16) {
            shrink *= 2;
            let (tw, th) = shrink_target(cw, ch, shrink);
            bytes = 0;
            for (im, _) in out.iter_mut() {
                if im.width() != tw || im.height() != th {
                    *im = shrink_exact(im, tw, th);
                }
                bytes += (im.width() as usize) * (im.height() as usize) * 4;
            }
        }
    }
    if out.len() < 2 {
        return None; // a single frame = no animation needed. Let the caller treat it as a still image.
    }
    Some((out, header_px))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{sample_path_or_skip, unique_tmp};

    #[test]
    fn decode_gif_real_sample_has_multiple_frames() {
        let Some(p) = sample_path_or_skip("sample.gif") else {
            return;
        };
        let frames = decode_gif(&p).expect("sample.gif はアニメーションとしてデコードできるはず");
        assert!(frames.len() > 1, "アニメ GIF は 2 フレーム以上");
        // Each frame is already composited to the same canvas size, and delay is at or above the
        // rounded floor.
        let (w0, h0) = (frames[0].0.width(), frames[0].0.height());
        assert!(w0 > 0 && h0 > 0);
        assert!(frames
            .iter()
            .all(|(img, d)| { img.width() == w0 && img.height() == h0 && *d >= MIN_FRAME_DELAY }));
    }

    #[test]
    fn decode_gif_inline_real_sample_has_multiple_frames() {
        // Same fixture/skip-guard as decode_gif_real_sample_has_multiple_frames — this exercises
        // the smaller inline budget (MAX_GIF_BYTES_INLINE) used for Markdown-embedded GIFs.
        let Some(p) = sample_path_or_skip("sample.gif") else {
            return;
        };
        let frames = decode_gif_inline(&p)
            .expect("sample.gif はインライン経路でもアニメとしてデコードできるはず");
        assert!(frames.len() > 1, "アニメ GIF は 2 フレーム以上");
        let (w0, h0) = (frames[0].0.width(), frames[0].0.height());
        assert!(w0 > 0 && h0 > 0);
    }

    #[test]
    fn decode_gif_budget_downscales_but_keeps_all_frames() {
        // A GIF that's over budget doesn't "drop frames" — it "shrinks every frame to the same
        // size". Force the shrink path with a tiny synthetic GIF plus a tiny budget, and pin down
        // that the frame count is kept, the dimensions match, and it stays within budget.
        use image::codecs::gif::GifEncoder;
        use image::{Delay, Frame, Rgba, RgbaImage};
        let dir = unique_tmp("konoma_gif_budget_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("big.gif");
        {
            let out = std::fs::File::create(&p).unwrap();
            let mut enc = GifEncoder::new(out);
            let frames = (0..4u8).map(|i| {
                Frame::from_parts(
                    RgbaImage::from_pixel(64, 64, Rgba([i * 60, 100, 200, 255])),
                    0,
                    0,
                    Delay::from_numer_denom_ms(100, 1),
                )
            });
            enc.encode_frames(frames).unwrap();
        }
        // 64x64 RGBA = 16,384B/frame × 4 frames = 65,536B. With a 20,000B budget, it should shrink
        // to 32x32 (4,096B/frame).
        let frames = decode_gif_with_budget(&p, 20_000).expect("アニメとしてデコードできる");
        assert_eq!(frames.len(), 4, "フレームは捨てない");
        let (w, h) = (frames[0].0.width(), frames[0].0.height());
        assert!(w < 64 && h < 64, "予算超過で縮小される: {w}x{h}");
        assert!(
            frames
                .iter()
                .all(|(im, _)| im.width() == w && im.height() == h),
            "全フレーム同寸法(アニメ巡回の前提)"
        );
        let total: usize = frames
            .iter()
            .map(|(im, _)| im.width() as usize * im.height() as usize * 4)
            .sum();
        assert!(total <= 20_000, "合計バイトが予算内: {total}");
        // With the default budget, a small GIF is left untouched (not shrunk).
        let frames = decode_gif(&p).expect("既定予算でもデコードできる");
        assert_eq!((frames[0].0.width(), frames[0].0.height()), (64, 64));
    }

    #[test]
    fn decode_static_reads_png_and_rejects_non_image() {
        let dir = unique_tmp("konoma_decode_static_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // Write out a real PNG and decode it (dimensions should match).
        let png = dir.join("tiny.png");
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(7, 3, image::Rgb([9, 9, 9])))
            .save(&png)
            .unwrap();
        let img = decode_static(&png).expect("PNG はデコードできる");
        assert_eq!((img.width(), img.height()), (7, 3));
        // Non-image data returns None (does not crash).
        let bad = dir.join("notimg.png");
        std::fs::write(&bad, b"definitely not an image").unwrap();
        assert!(decode_static(&bad).is_none(), "非画像は None");
        // A missing file also returns None.
        assert!(decode_static(&dir.join("missing.png")).is_none());
    }

    #[test]
    fn decode_static_bytes_matches_the_path_version() {
        let dir = unique_tmp("konoma_decode_static_bytes_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let png = dir.join("tiny.png");
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(7, 3, image::Rgb([9, 9, 9])))
            .save(&png)
            .unwrap();
        let bytes = std::fs::read(&png).unwrap();
        let from_path = decode_static(&png).unwrap();
        let from_bytes = decode_static_bytes(&bytes).unwrap();
        assert_eq!(
            (from_path.width(), from_path.height()),
            (from_bytes.width(), from_bytes.height())
        );
        assert_eq!(
            from_path.to_rgba8().into_raw(),
            from_bytes.to_rgba8().into_raw()
        );
    }

    #[test]
    fn decode_static_bytes_rejects_garbage() {
        assert!(decode_static_bytes(b"definitely not an image").is_none());
        assert!(decode_static_bytes(b"").is_none());
    }

    #[test]
    fn decode_static_bytes_uses_the_bundled_samples() {
        for name in ["sample.png", "sample.jpg"] {
            let Some(p) = sample_path_or_skip(name) else {
                continue;
            };
            let bytes = std::fs::read(&p).unwrap();
            let from_path = decode_static(&p).expect("path 版はデコードできる");
            let from_bytes = decode_static_bytes(&bytes).expect("bytes 版もデコードできる");
            assert_eq!(
                (from_path.width(), from_path.height()),
                (from_bytes.width(), from_bytes.height()),
                "{name}: サイズが path 版と一致するはず"
            );
        }
    }

    #[test]
    fn decode_gif_bytes_inline_matches_the_path_version() {
        let Some(p) = sample_path_or_skip("sample.gif") else {
            return;
        };
        let bytes = std::fs::read(&p).unwrap();
        let from_path = decode_gif_inline(&p).expect("path 版はアニメとしてデコードできる");
        let (from_bytes, header_px) =
            decode_gif_bytes_inline(&bytes).expect("bytes 版もアニメとしてデコードできる");
        assert_eq!(
            from_path.len(),
            from_bytes.len(),
            "フレーム数が一致するはず"
        );
        for ((pi, pd), (bi, bd)) in from_path.iter().zip(from_bytes.iter()) {
            assert_eq!((pi.width(), pi.height()), (bi.width(), bi.height()));
            assert_eq!(pd, bd, "各フレームの表示時間も一致するはず");
        }
        // The header size must match the (unshrunk, at this small budget) first frame's own size —
        // this fixture is far below `MAX_GIF_BYTES_INLINE`, so no downscale should have happened.
        let (fw, fh) = image::GenericImageView::dimensions(&from_bytes[0].0);
        assert_eq!(
            header_px,
            (fw, fh),
            "予算内なので header サイズ==実デコードサイズのはず"
        );
    }

    #[test]
    fn decode_gif_bytes_inline_rejects_garbage_and_non_gif() {
        assert!(decode_gif_bytes_inline(b"not a gif at all").is_none());
        // A real PNG (not a GIF, and not animated) also returns None — falls back to the still-image
        // decode at the call site, exactly like the path version's own contract.
        let dir = unique_tmp("konoma_decode_gif_bytes_inline_png_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let png = dir.join("tiny.png");
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(4, 4, image::Rgb([1, 2, 3])))
            .save(&png)
            .unwrap();
        let bytes = std::fs::read(&png).unwrap();
        assert!(decode_gif_bytes_inline(&bytes).is_none());
    }
}
