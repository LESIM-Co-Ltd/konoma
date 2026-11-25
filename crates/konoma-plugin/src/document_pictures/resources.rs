//! The pictures a document carries inside it (`Reopen::resources`: the images of a Word or
//! OpenDocument file, the slides of a presentation): where their pixels come from, how big the
//! page reserves them, and the worker that decodes them.
//!
//! The same code in every host. What differs is only what the host hands in: the SVG drawing
//! (konoma's supervised child process or the standalone app's in-process one), the pixel size a
//! slide is wanted at, and the channel the result goes to.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use konoma_svg::image::ImageFailure;
use konoma_svg::svg_guard::SvgFail;
use konoma_widgets::inline_images::{DecodedImage, InlineImageCache};
use konoma_widgets::markdown::{is_office_image_url, md_image_cells, slide_cells, ImageSlot};

use super::files::md_decode_bytes_why;
use super::requests::DocPictures;
use super::slots::MD_IMAGE_MAX_ROWS;
use crate::canvas_pictures::SvgDraw;
use crate::input::{Resource, Resources};
use crate::LazyResource;

/// Rows around a slide picture that are not the picture: its heading line and the blank line
/// between the heading and the picture.
pub const SLIDE_CHROME_ROWS: u16 = 2;

/// Where the pixels of a picture of the open document come from.
#[derive(Clone)]
pub enum PictureSource {
    /// A file inside the package (png, jpeg, gif, bmp, webp, tiff or svg): its bytes, shared with
    /// decode threads (and with the plugin's session).
    Bytes(Arc<[u8]>),
    /// A picture the plugin makes when asked (a slide of a presentation): no bytes exist yet. The
    /// decode thread asks it for an SVG and hands that to the untrusted-SVG drawing path.
    Lazy {
        lazy: Arc<dyn LazyResource>,
        /// Set by the decode thread when drawing the picture had to leave something out (a size
        /// budget of the writer); shared by all the pictures of the document and read by
        /// `App::document_truncated`, so the title says so like it does for a reader's budget.
        render_truncated: Arc<std::sync::atomic::AtomicBool>,
        /// The longest side, in px, this picture can be drawn at inside the drawing process's work
        /// budget, learned by the decode thread after it drew; the wish for a sharper redraw is
        /// cut to it, so a heavy slide on a huge terminal is not redrawn again and again for a
        /// size it cannot have.
        raster_cap: Arc<SlideRasterCap>,
    },
}

/// How long a raster size refused for taking too long stays out of reach. A time-out measures the
/// machine's load as much as the slide, so it is not a fact about the slide: after this long a
/// revisit tries the sharp size again (a revisit within it does not wait out another time-out).
/// Refusals for cost or memory are the same on every run and never expire.
pub const TIMEOUT_CAP_LIFETIME: std::time::Duration = std::time::Duration::from_secs(60);

/// The longest raster side, in px, a slide is known to be drawable at, shared by everything that
/// draws the slide. Two kinds of knowledge with different lifetimes: what the slide's own cost
/// allows and what the drawing process refused for cost or memory (the same every time, kept for
/// good), and what it refused only by the clock (expires, see [`TIMEOUT_CAP_LIFETIME`]).
#[derive(Debug, Default)]
pub struct SlideRasterCap {
    /// Permanent limit; 0 = none known yet.
    permanent: std::sync::atomic::AtomicU32,
    /// A limit that came from a time-out, with when it was learned.
    timed_out: std::sync::Mutex<Option<(u32, std::time::Instant)>>,
}

impl SlideRasterCap {
    /// The limit in force at `now`; None = nothing known (draw at the wanted size).
    pub fn get(&self, now: std::time::Instant) -> Option<u32> {
        let permanent =
            Some(self.permanent.load(std::sync::atomic::Ordering::Relaxed)).filter(|c| *c > 0);
        let timed_out = self
            .timed_out
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .filter(|(_, at)| now.saturating_duration_since(*at) < TIMEOUT_CAP_LIFETIME)
            .map(|(px, _)| px);
        match (permanent, timed_out) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    /// Lowers the permanent limit to `px` (never raises it; at least 1).
    pub fn lower_permanent(&self, px: u32) {
        use std::sync::atomic::Ordering::Relaxed;
        let px = px.max(1);
        let mut current = self.permanent.load(Relaxed);
        // compare_exchange_weak loop (fetch_update is deprecated on 1.99, try_update needs
        // newer than the MSRV): stop as soon as the stored limit is already lower.
        while current == 0 || px < current {
            match self
                .permanent
                .compare_exchange_weak(current, px, Relaxed, Relaxed)
            {
                Ok(_) => break,
                Err(seen) => current = seen,
            }
        }
    }

    /// Lowers the limit to `px` for [`TIMEOUT_CAP_LIFETIME`] from `now`. A limit that is still
    /// in force and lower stays (its own clock keeps running); an expired one is replaced.
    pub fn lower_timed_out(&self, px: u32, now: std::time::Instant) {
        let px = px.max(1);
        let mut slot = self
            .timed_out
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let live = slot.filter(|(_, at)| now.saturating_duration_since(*at) < TIMEOUT_CAP_LIFETIME);
        if live.is_none_or(|(old, _)| px < old) {
            *slot = Some((px, now));
        }
    }
}

/// How many pictures of the open document are being decoded for the first time right now (cached,
/// not yet decoded, not failed, pixels never dropped). An entry whose pixels the cache dropped
/// (`evicted`) has no decode running for it: counting it as in flight would fill the cap with idle
/// entries after enough pictures had been seen, and no new picture would ever be started.
pub fn office_pictures_in_flight(cache: &InlineImageCache) -> usize {
    cache
        .cache
        .iter()
        .filter(|(k, e)| {
            e.decoded.is_none()
                && !e.failed
                && !e.evicted
                && is_office_image_url(&k.to_string_lossy())
        })
        .count()
}

/// One picture of a document, held in memory only.
pub struct DocPicture {
    pub source: PictureSource,
    /// Pixel size read from the header (raster) or the intrinsic size (SVG, slide), without
    /// decoding. `None` = not an image the host can size: it is drawn as its alt text.
    pub dims: Option<(u32, u32)>,
    /// `dims` came from a raster header (not an SVG's intrinsic size): only those are refused
    /// from the header alone; an SVG is guarded by its drawing process instead.
    pub raster: bool,
}

/// Pixel size of an in-memory picture without decoding it: raster formats first, then SVG (the same
/// order as `md_image_dims` for a file).
fn picture_dims(bytes: &[u8]) -> (Option<(u32, u32)>, bool) {
    let raster = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .ok()
        .and_then(|r| r.into_dimensions().ok());
    match raster {
        Some(d) => (Some(d), true),
        None => (konoma_svg::svg::intrinsic_size_bytes(bytes), false),
    }
}

impl DocPicture {
    /// The picture a document resource is. Pixel sizes are read from image headers only, never a
    /// decode. `render_truncated` is shared by all the lazy pictures of one document.
    pub fn from_resource(resource: &Resource, render_truncated: &Arc<AtomicBool>) -> DocPicture {
        match resource {
            Resource::Bytes(bytes) => {
                let (dims, raster) = picture_dims(bytes);
                DocPicture {
                    source: PictureSource::Bytes(bytes.clone()),
                    dims,
                    raster,
                }
            }
            Resource::Lazy(lazy) => DocPicture {
                dims: lazy.natural_size(),
                source: PictureSource::Lazy {
                    lazy: lazy.clone(),
                    render_truncated: render_truncated.clone(),
                    raster_cap: Arc::new(SlideRasterCap::default()),
                },
                raster: false,
            },
        }
    }

    /// Every picture of `resources`, by its `office-img://...` URL.
    pub fn from_resources(
        resources: &Resources,
        render_truncated: &Arc<AtomicBool>,
    ) -> HashMap<String, DocPicture> {
        resources
            .iter()
            .map(|(k, r)| (k.clone(), DocPicture::from_resource(r, render_truncated)))
            .collect()
    }

    /// Whether this picture is made on demand (the drawing of a slide).
    pub fn is_slide(&self) -> bool {
        matches!(self.source, PictureSource::Lazy { .. })
    }
}

/// How much room the pictures of a document take on the page: the rules of both hosts for an
/// `office-img://` picture (a Word document's embedded one, a slide).
pub struct ResourceSlots<'a> {
    pub pictures: &'a HashMap<String, DocPicture>,
    /// The cache the decodes are filed in: a decode that failed keeps its reason here.
    pub cache: &'a InlineImageCache,
    /// The terminal's cell size in pixels.
    pub font: (u16, u16),
    /// The width, in cells, the page leaves a picture.
    pub avail: u16,
    /// The most rows a slide takes (what the viewport leaves after its heading).
    pub slide_rows: u16,
    /// The text of a failure in the user's language.
    pub message: &'a dyn Fn(ImageFailure) -> Option<String>,
}

impl ResourceSlots<'_> {
    /// The slot of `url`, or `None` when it is not a picture of the document (the page's other
    /// rules apply).
    pub fn slot(&self, url: &str, max_cols: Option<u16>) -> Option<ImageSlot> {
        if !is_office_image_url(url) {
            return None;
        }
        let failed = |f: ImageFailure| match (self.message)(f) {
            Some(why) => ImageSlot::Failed(why),
            None => ImageSlot::Unavailable,
        };
        // Said once, like a file picture: a decode that failed keeps its reason, and a
        // picture whose header is already over the limits is refused before any decode.
        if let Some(e) = self
            .cache
            .cache
            .get(&PathBuf::from(url))
            .filter(|e| e.failed)
        {
            return Some(match e.fail {
                Some(f) => failed(f),
                None => ImageSlot::Unavailable,
            });
        }
        let (fw, fh) = self.font;
        let pic = self.pictures.get(url);
        Some(match pic.and_then(|p| p.dims.map(|d| (p, d))) {
            // A slide: the full text width (it is a vector drawing, so it may be enlarged),
            // as tall as the viewport leaves room for after its heading.
            Some((p, (pw, ph))) if p.is_slide() => {
                let (cols, rows) = slide_cells(
                    pw,
                    ph,
                    fw,
                    fh,
                    max_cols.unwrap_or(self.avail),
                    self.slide_rows,
                );
                ImageSlot::Inline { cols, rows }
            }
            // Only a raster is refused from its header: an SVG's intrinsic size is not a
            // pixel count (it is drawn at `svg_max_px`, by the supervised process), the
            // same as a file SVG.
            Some((p, d)) if p.raster && !konoma_svg::image::dimensions_within_limits(d) => {
                failed(ImageFailure::TooLarge)
            }
            Some((_, (pw, ph))) => {
                let (cols, rows) = md_image_cells(
                    pw,
                    ph,
                    fw,
                    fh,
                    max_cols.unwrap_or(self.avail),
                    MD_IMAGE_MAX_ROWS,
                );
                ImageSlot::Inline { cols, rows }
            }
            None => ImageSlot::Unavailable,
        })
    }
}

/// What one decode of a document picture needs of the host (`DocPictures::spawn_resource_decode`).
pub struct ResourceJob {
    pub source: PictureSource,
    /// The longest side an SVG (or a slide) is drawn at.
    pub svg_max_px: u32,
    /// The smallest size a failed slide is retried at: the raster already on screen when this is
    /// a sharpening redraw (a retry must never replace it by something smaller), else the base
    /// size.
    pub retry_floor: u32,
    /// Whether this is a sharpening redraw of pixels already shown.
    pub reraster: bool,
    /// Draws the SVGs (and the slides, which are SVGs).
    pub svg: Arc<dyn SvgDraw + Send + Sync>,
    /// The preview's generation when the job started, and the host's latest: the decode is
    /// stopped once they differ.
    pub generation: (u64, Arc<std::sync::atomic::AtomicU64>),
}

impl DocPictures<'_> {
    /// Starts the one-time decode of a document picture whose cache entry does not exist yet:
    /// files the entry, stamps it as wanted by the pass being drawn and starts the worker. A
    /// thread that could not start ends the wait (`failed`) instead of latching "loading".
    pub fn begin_resource_decode(&mut self, key: PathBuf, job: ResourceJob) {
        let frame = self.cache.frame;
        let entry = self.cache.cache.entry(key.clone()).or_default();
        entry.last_used = frame;
        entry.wish.set_priority(frame);
        if !self.spawn_resource_decode(key.clone(), job) {
            if let Some(e) = self.cache.cache.get_mut(&key) {
                e.failed = true;
            }
        }
    }

    /// Starts the thread that decodes the picture of `job` into the cache entry `key` (which
    /// exists already: a first decode just placed it, a rebuild of dropped pixels keeps its
    /// own). Returns false when the thread could not be started; the caller ends the wait. With
    /// no results channel nothing is started.
    ///
    /// The decode queues in the shared decode gate under the entry's own ticket, an SVG is drawn
    /// by the job's drawing (stopped when the preview moves on), a panic on a pathological
    /// picture is a failed decode, and a result is always sent (not sending would latch the busy
    /// indicator).
    pub fn spawn_resource_decode(&mut self, key: PathBuf, job: ResourceJob) -> bool {
        let Some(tx) = self.results.clone() else {
            return true;
        };
        let ticket = self
            .cache
            .cache
            .get(&key)
            .map(|e| e.wish.ticket())
            .unwrap_or_default();
        let request = self.cache.next_request(&key);
        let ResourceJob {
            source,
            svg_max_px,
            retry_floor,
            reraster,
            svg,
            generation: (gen, latest),
        } = job;
        let stale = ticket.clone();
        std::thread::Builder::new()
            .name("konoma-office-pic".into())
            .spawn(move || {
                let moved_on = || latest.load(Ordering::Relaxed) != gen || stale.is_stale();
                let (still, frames) = konoma_base::panic::catch_silent(|| {
                    konoma_svg::image::with_decode_ticket(ticket, || {
                        let mut svg_px = svg_max_px;
                        let mut slide_cap = None;
                        // A slide has no bytes yet: draw its scene to an SVG here (off the UI
                        // thread, under the decode gate) and let the drawing rasterize it like
                        // any other untrusted SVG of a document.
                        let bytes: Arc<[u8]> = match source {
                            PictureSource::Bytes(b) => b,
                            PictureSource::Lazy {
                                lazy,
                                render_truncated,
                                raster_cap,
                            } => {
                                use crate::view::ImageData;
                                let cancel = crate::Cancel::from_fn({
                                    let (latest, stale) = (latest.clone(), stale.clone());
                                    move || {
                                        latest.load(Ordering::Relaxed) != gen || stale.is_stale()
                                    }
                                });
                                let req = crate::ResourceReq { max_px: svg_px };
                                let drawn = match lazy.render(&req, &cancel) {
                                    Ok(ImageData::Svg { data, .. } | ImageData::Encoded(data)) => {
                                        data
                                    }
                                    // The preview moved on: nothing to show, nothing to remember.
                                    Err(f) if f.kind == crate::FailureKind::Cancelled => {
                                        return (Err(ImageFailure::Cancelled), None);
                                    }
                                    Ok(_) | Err(_) => return (Err(ImageFailure::Corrupt), None),
                                };
                                if lazy.truncated() {
                                    render_truncated.store(true, Ordering::Relaxed);
                                }
                                // The raster size is cut to what the drawing process's work
                                // budget allows for this slide's effects (they are counted at the
                                // square of the scale): effects stay, the slide is just drawn
                                // smaller. A size an earlier attempt found too heavy stays out
                                // (see below), and one that only timed out stays out for a while.
                                if let Some(px) = lazy.max_raster_px() {
                                    raster_cap.lower_permanent(px);
                                }
                                if let Some(cap) = raster_cap.get(std::time::Instant::now()) {
                                    svg_px = svg_px.min(cap);
                                }
                                slide_cap = Some(raster_cap);
                                drawn
                            }
                        };
                        if bytes.starts_with(b"GIF8") {
                            if let Some((frames, _)) =
                                konoma_svg::image::decode_gif_bytes_inline(&bytes)
                            {
                                let first = frames[0].0.clone();
                                return (Ok(first), Some(frames));
                            }
                        }
                        // A slide whose picture the drawing process refuses as too heavy or
                        // too slow at this size is drawn again smaller before it is given up on;
                        // the size that worked becomes the slide's raster cap.
                        let res = decode_with_cap(
                            slide_cap.as_deref(),
                            svg_px,
                            retry_floor,
                            &moved_on,
                            &std::time::Instant::now,
                            |px| md_decode_bytes_why(&bytes, px, svg.as_ref(), &moved_on),
                        );
                        (res, None)
                    })
                })
                .unwrap_or((Err(ImageFailure::Corrupt), None));
                // A cancelled decode reports like any other result, first decode and re-raster
                // alike: the entry may still be in the cache (a stale ticket does not remove it),
                // and the receiving side forgets it or ignores a gone one.
                let still = match still {
                    Err(_) if moved_on() => Err(ImageFailure::Cancelled),
                    other => other,
                };
                let _ = tx.send(DecodedImage {
                    path: key,
                    image: still.map_err(|f| f.code().to_string()),
                    svg: None,
                    reraster,
                    frames,
                    request,
                });
            })
            .is_ok()
    }
}

/// Runs `attempt` at `start_px` and, when `shrink` is set and the drawing process refused the
/// picture as too heavy, too slow or too memory-hungry (costs that grow with the square of the
/// raster size), again at a smaller size, never below `floor_px`. Cost and memory refusals are
/// the same on every run and fail fast, so they halve the side each time. A time-out is the
/// expensive kind (it holds the decode gate for the whole time limit) and says as much about the
/// machine's load as about the slide: the first one is retried once at half the side (a quarter
/// of the work), and a second goes straight to the floor, so a loaded machine is held for at most
/// three time limits, not one per halving. Any other failure is final, and so is a request the
/// user moved on from. A crash is not retried smaller: it is a stack overflow (the same at any
/// size) or an allocation failure under memory pressure (an immediate retry meets the same
/// pressure); [`cap_lesson`] makes its cap expire so the slide is tried again later.
///
/// Returns the last result, the size it was made at, and the refusals on the way as
/// `(size, failure)` (the last failure included), which [`record_slide_attempts`] turns into what
/// the slide remembers.
pub fn decode_with_smaller_retries(
    start_px: u32,
    floor_px: u32,
    shrink: bool,
    cancelled: &dyn Fn() -> bool,
    mut attempt: impl FnMut(u32) -> Result<image::DynamicImage, ImageFailure>,
) -> (
    Result<image::DynamicImage, ImageFailure>,
    u32,
    Vec<(u32, ImageFailure)>,
) {
    let floor = floor_px.min(start_px);
    let mut px = start_px;
    let mut timeouts = 0u32;
    let mut trail = Vec::new();
    loop {
        let res = attempt(px);
        if let Err(f) = &res {
            trail.push((px, *f));
        }
        let refused = match &res {
            Err(ImageFailure::Svg(
                f @ (SvgFail::TooHeavy | SvgFail::Timeout | SvgFail::Memory),
            )) => Some(*f),
            _ => None,
        };
        let Some(why) = refused.filter(|_| shrink && px > floor && !cancelled()) else {
            return (res, px, trail);
        };
        px = if why == SvgFail::Timeout {
            timeouts += 1;
            if timeouts == 1 {
                (px / 2).max(floor)
            } else {
                floor
            }
        } else {
            (px / 2).max(floor)
        };
    }
}

/// Decodes a picture with `attempt`, and when it is a slide (`cap` is Some) draws it again
/// smaller if the drawing process refuses it, then teaches `cap` what happened (see
/// [`decode_with_smaller_retries`] and [`record_slide_attempts`]). Anything that is not a slide
/// is attempted once at `start_px`. `clock` is read when a refusal is recorded (not before the
/// drawing starts), so an expiring cap counts its lifetime from the refusal.
pub fn decode_with_cap(
    cap: Option<&SlideRasterCap>,
    start_px: u32,
    floor_px: u32,
    moved_on: &dyn Fn() -> bool,
    clock: &dyn Fn() -> std::time::Instant,
    attempt: impl FnMut(u32) -> Result<image::DynamicImage, ImageFailure>,
) -> Result<image::DynamicImage, ImageFailure> {
    let (res, used, trail) =
        decode_with_smaller_retries(start_px, floor_px, cap.is_some(), moved_on, attempt);
    if let Some(cap) = cap {
        record_slide_attempts(cap, &trail, &res, used, floor_px, moved_on(), clock);
    }
    res
}

/// What one refused attempt of a slide's draw says about the slide's raster size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CapLesson {
    /// The same at this size on every run: the cap is kept for good.
    Permanent,
    /// Says as much about the machine's load as about the slide: the cap expires.
    Expiring,
    /// Does not depend on the raster size (drawing smaller would not help). It says nothing
    /// about what size the slide can be drawn at, so it is not retried smaller and teaches no
    /// size, but when the draw never worked the same request must not be made again at once (a
    /// failed sharpening redraw is asked for again by every frame otherwise): the cap is held at
    /// the floor for a while, like a time-out.
    SizeIndependent,
    /// Nobody wants the result (cancelled): nothing is learned.
    Nothing,
}

/// Classifies a failure of a slide's draw. An exhaustive match on purpose (no catch-all): a new
/// failure kind has to be given a decision here.
fn cap_lesson(failure: ImageFailure) -> CapLesson {
    match failure {
        // Costs that grow with the raster size and are the same on every run.
        ImageFailure::Svg(SvgFail::TooHeavy | SvgFail::Memory) => CapLesson::Permanent,
        // Timeout: the clock, i.e. the load. Crashed: the drawing process died by itself, which is
        // an allocation failure (transient under memory pressure) or a stack overflow (not size
        // dependent, but then the next try after the lifetime costs one more crash, no more).
        ImageFailure::Svg(SvgFail::Timeout | SvgFail::Crashed) => CapLesson::Expiring,
        // Decided by the picture itself, whatever size it is drawn at.
        ImageFailure::Svg(
            SvgFail::TooDeep | SvgFail::TooLarge | SvgFail::TooComplex | SvgFail::Invalid,
        )
        | ImageFailure::TooLarge
        | ImageFailure::Corrupt
        | ImageFailure::UnsupportedFormat
        | ImageFailure::OutsideReadDirs => CapLesson::SizeIndependent,
        ImageFailure::Cancelled => CapLesson::Nothing,
    }
}

/// Teaches `cap` what the attempts of one slide draw showed. Each refused size lowers the cap to
/// the size tried next (the one that worked, or the one after it also failed); the last failure
/// of a draw that never worked caps it at `floor_px` (the raster already shown, or the base
/// size), so a sharpening redraw does not ask for more again and again. What each failure means
/// is decided by [`cap_lesson`]; an expiring cap starts its lifetime when it is recorded
/// (`clock`); so does a failure that does not depend on the size (see [`CapLesson::SizeIndependent`]). A cancelled attempt says nothing about the slide, and neither does the last
/// attempt of a request the user moved on from (the refusals confirmed before it still count).
fn record_slide_attempts(
    cap: &SlideRasterCap,
    trail: &[(u32, ImageFailure)],
    result: &Result<image::DynamicImage, ImageFailure>,
    used: u32,
    floor_px: u32,
    moved_on: bool,
    clock: &dyn Fn() -> std::time::Instant,
) {
    let floor = floor_px.min(trail.first().map_or(used, |t| t.0));
    // The user moved on during the last attempt: it is not a confirmed refusal.
    let confirmed = trail.len().saturating_sub(usize::from(moved_on));
    for (i, (_, failure)) in trail.iter().enumerate().take(confirmed) {
        let next = match trail.get(i + 1) {
            Some((px, _)) => *px,
            None if result.is_ok() => used,
            None => floor,
        };
        match cap_lesson(*failure) {
            CapLesson::Permanent => cap.lower_permanent(next),
            CapLesson::Expiring => cap.lower_timed_out(next, clock()),
            // Only the final failure of a draw that never worked (`next` is then the floor, the
            // raster already shown): the sizes before it were refusals, not this.
            CapLesson::SizeIndependent => cap.lower_timed_out(next, clock()),
            CapLesson::Nothing => {}
        }
    }
}
