//! The pictures of a canvas in the standalone app: the host side of
//! [`crate::canvas_pictures`], the part konoma keeps in `src/app/canvas_images.rs`,
//! `src/app/plugin_host.rs` and `src/ui/canvas.rs`.
//!
//! What is decoded, how a picture is keyed and what bounds the work is the shared code
//! ([`decode_canvas`], [`CanvasPictureHost`], [`draw_images`]); this module only wires it to the
//! app's loop, with the same machinery as konoma:
//!
//! - **The UI never decodes.** A canvas the session returns is handed to [`Pictures::submit`],
//!   which starts a worker thread that runs [`decode_canvas`]. A newer canvas that differs
//!   cancels the job in flight (its result is dropped); one that names the same pictures keeps
//!   it. The result is filed with [`CanvasPictureHost::landed`] in [`Pictures::poll`].
//! - **The encode worker** is konoma's ([`md_encode_worker`]) on its own thread; the draw only
//!   asks for an encode and the result is applied in [`Pictures::poll`].
//! - **No blink**: the shared drawing keeps the previous picture of an id until the new content
//!   has a protocol.
//! - **No picker, no pictures**: without a [`Picker`] there is no `Pictures` at all; nothing is
//!   decoded and the plugin's cells stay. A picker that only has halfblocks draws halfblocks.
//! - **SVG** is drawn in this process ([`InProcessSvg`]): the app has no supervised child to
//!   hand it to, and the work limits of the shared code and of the drawing apply.
//! - **Leaving** deletes every kitty image the app placed, on every way out: `Drop` of
//!   [`Pictures`] (normal exit, `q`, an error, an unwinding panic) and
//!   [`delete_placed_pictures`] (the panic hook, which runs before the stack unwinds, while the
//!   alternate screen the images were placed on is still the screen).

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use image::DynamicImage;
use konoma_svg::svg::RasterError;
use konoma_widgets::inline_images::{
    md_encode_worker, InlineImageCache, MdEncodeResult, OverlayMoved, LIVE_CANVAS_KITTY,
    MD_IMAGE_CACHE_BYTES,
};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui_image::picker::Picker;

use crate::canvas_pictures::{
    decode_canvas, draw_images, CanvasPictureHost, CanvasPictures, CanvasPolicy, SvgDraw,
};
use crate::view::{CanvasImage, CanvasView, ImageData};
use crate::ReadScope;

/// How long the loop waits while a decode or an encode is out (so its result is drawn as soon as
/// it lands; the same short poll konoma keeps while pictures are loading).
const BUSY_POLL: Duration = Duration::from_millis(16);
/// The least time between two follow-up jobs for pictures the work limits left for a later answer
/// (the floor konoma applies to a canvas redraw timer).
const AGAIN_AFTER: Duration = Duration::from_millis(33);

/// Draws an SVG in this process. The app is a single program with no supervised child to draw an
/// untrusted document in, so it applies the same guard rails (size and depth refusals, the
/// drawing budget, the read roots) to a drawing made on the decode worker. A drawing cannot be
/// interrupted once it runs: `cancelled` is looked at before it starts.
pub struct InProcessSvg;

impl SvgDraw for InProcessSvg {
    fn untrusted(
        &self,
        data: &[u8],
        base: &Path,
        roots: Option<&[PathBuf]>,
        max_px: u32,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<DynamicImage, RasterError> {
        konoma_svg::svg_guard::precheck(data)?;
        if cancelled() {
            return Err(RasterError::Cancelled);
        }
        Ok(konoma_svg::svg::rasterize_guarded_within(
            data,
            base.parent(),
            roots,
            max_px,
        )?)
    }

    fn embedded(
        &self,
        data: &[u8],
        max_px: u32,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<DynamicImage, RasterError> {
        konoma_svg::svg_guard::precheck(data)?;
        if cancelled() {
            return Err(RasterError::Cancelled);
        }
        Ok(konoma_svg::svg::rasterize_guarded_within(
            data, None, None, max_px,
        )?)
    }
}

/// Write to `out` the deletes of every canvas picture still on the terminal, and flush (every
/// failure is dropped: the terminal may be gone). What the panic hook calls before it leaves the
/// alternate screen; a table already emptied by [`Pictures`] writes nothing.
pub fn delete_placed_pictures(out: &mut dyn Write) {
    LIVE_CANVAS_KITTY.delete_to(out);
}

/// The result of one decode job.
struct Done {
    gen: u64,
    /// `None`: the worker panicked (caught); the pictures stay as they were.
    pics: Option<CanvasPictures>,
}

/// The decode job in flight.
struct Job {
    gen: u64,
    cancel: Arc<AtomicBool>,
    /// The pictures it was given, with the data of every `unchanged` one the app still held.
    images: Vec<CanvasImage>,
}

/// The pictures of the canvas on screen, and everything that gets them there.
pub struct Pictures {
    cache: InlineImageCache,
    host: CanvasPictureHost,
    enc_rx: Receiver<MdEncodeResult>,
    done_tx: Sender<Done>,
    done_rx: Receiver<Done>,
    overlay: OverlayMoved,
    plugin_id: String,
    path: PathBuf,
    raster_px: u32,
    svg: Arc<dyn SvgDraw + Send + Sync>,
    /// Where the kitty deletes are written (standard output; a buffer in a test).
    out: Box<dyn Write>,
    gen: u64,
    job: Option<Job>,
    /// The pictures of the last job that landed (or was given up on).
    last: Option<Vec<CanvasImage>>,
    /// The latest canvas, as the session sent it (what a follow-up job is made from).
    latest: Option<(u16, u16, Vec<CanvasImage>)>,
    /// The data of pictures the session named once and has since called `unchanged`, kept until a
    /// job that carried it lands: a job superseded before it landed took the data with it, and the
    /// session will not send it again.
    unlanded: HashMap<u64, ImageData>,
    /// When to ask for the pictures the work limits left for a later answer.
    again_at: Option<Instant>,
    /// Whether the last frame drawn was the canvas (not the help or a message). Nothing of the
    /// pictures is on screen otherwise, so an animation then needs no wake (konoma polls GIFs
    /// only in the preview).
    on_canvas: bool,
}

impl Pictures {
    /// The pictures for `plugin_id` showing `path`, drawn through `picker`. Starts the encode
    /// worker; the SVGs are drawn in this process at `raster_px` (`ui.svg_max_px`) and the
    /// kitty deletes go to standard output.
    pub fn new(picker: Picker, plugin_id: &str, path: &Path, raster_px: u32) -> Pictures {
        let (enc_tx, req_rx) = channel();
        let (res_tx, enc_rx) = channel();
        let mut cache = InlineImageCache::default();
        cache.attach_picker(&picker);
        cache.enc_tx = Some(enc_tx);
        std::thread::spawn(move || md_encode_worker(picker, req_rx, res_tx));
        let (done_tx, done_rx) = channel();
        Pictures {
            cache,
            host: CanvasPictureHost::default(),
            enc_rx,
            done_tx,
            done_rx,
            overlay: OverlayMoved::new(),
            plugin_id: plugin_id.to_string(),
            path: path.to_path_buf(),
            raster_px,
            svg: Arc::new(InProcessSvg),
            out: Box::new(std::io::stdout()),
            gen: 0,
            job: None,
            last: None,
            latest: None,
            unlanded: HashMap::new(),
            again_at: None,
            on_canvas: false,
        }
    }

    /// The same drawing SVGs with `svg` (a test seam).
    pub fn with_svg(mut self, svg: Arc<dyn SvgDraw + Send + Sync>) -> Pictures {
        self.svg = svg;
        self
    }

    /// The same writing the kitty deletes to `out` (a test seam).
    pub fn with_out(mut self, out: Box<dyn Write>) -> Pictures {
        self.out = out;
        self
    }

    /// The session returned `canvas`: decode what it places that the app does not hold yet.
    pub fn submit(&mut self, canvas: &CanvasView) {
        self.latest = Some((canvas.cols, canvas.rows, canvas.images.clone()));
        self.start_for(canvas.cols, canvas.rows, &canvas.images, false);
    }

    /// The session's view is not a canvas (any more): drop every picture, and queue the delete of
    /// every kitty image they pinned.
    pub fn forget(&mut self) {
        if let Some(job) = self.job.take() {
            job.cancel.store(true, Ordering::Relaxed);
        }
        self.last = None;
        self.latest = None;
        self.unlanded.clear();
        self.again_at = None;
        self.host.forget(&mut self.cache);
    }

    /// Apply what the workers finished: encodes, decodes (filed with
    /// [`CanvasPictureHost::landed`]), the GIF frames that are due, and the follow-up job for
    /// pictures that waited.
    pub fn poll(&mut self) {
        while let Ok(res) = self.enc_rx.try_recv() {
            self.cache.apply_encode(res);
        }
        while let Ok(done) = self.done_rx.try_recv() {
            // The result of a job that was superseded: dropped.
            if self.job.as_ref().map(|j| j.gen) != Some(done.gen) {
                continue;
            }
            let Some(job) = self.job.take() else { continue };
            if let Some(pics) = done.pics {
                let landed = self
                    .host
                    .landed(&mut self.cache, pics, MD_IMAGE_CACHE_BYTES);
                if landed.ask_again {
                    self.again_at = Some(Instant::now() + AGAIN_AFTER);
                }
            }
            for img in &job.images {
                if img.image.is_some() {
                    self.unlanded.remove(&img.id);
                }
            }
            // Even a job that panicked is not asked for again with the same canvas.
            self.last = Some(job.images);
        }
        self.cache.advance_gifs_if_due();
        if let Some(at) = self.again_at {
            if Instant::now() >= at && self.job.is_none() {
                self.again_at = None;
                if let Some((cols, rows, images)) = self.latest.clone() {
                    self.start_for(cols, rows, &images, true);
                }
            }
        }
    }

    /// How long the loop may wait before it has to look at the pictures again: a short poll while
    /// a decode or an encode is out, until a follow-up job is due (only when no job is out: the
    /// job in flight wakes the loop at the short poll, and [`Pictures::poll`] starts the follow-up
    /// once it landed), and until the next GIF frame (only while the canvas is the frame shown).
    /// `None` when nothing is going on.
    pub fn wake_in(&self) -> Option<Duration> {
        let mut wake: Option<Duration> = None;
        let mut want = |d: Duration| wake = Some(wake.map_or(d, |w| w.min(d)));
        if self.job.is_some() || self.cache.encodes_in_flight() {
            want(BUSY_POLL);
        }
        if self.job.is_none() {
            if let Some(at) = self.again_at {
                want(at.saturating_duration_since(Instant::now()));
            }
        }
        if self.on_canvas {
            if let Some(g) = self.cache.gif_poll_timeout() {
                want(g);
            }
        }
        wake
    }

    /// Draw the pictures over the cells already painted in `body`. `on_canvas` is false when the
    /// frame is not the canvas (the help, a message): nothing is drawn, and the overlay then
    /// reports that the pictures left.
    pub fn draw(&mut self, buf: &mut Buffer, body: Rect, on_canvas: bool) {
        self.on_canvas = on_canvas;
        self.overlay.begin();
        if on_canvas && body.width > 0 && body.height > 0 {
            if let Some(sig) = draw_images(buf, body, &mut self.host, &mut self.cache) {
                self.overlay.note(sig);
            }
        }
        self.overlay.finish();
    }

    /// Whether the pictures moved or went since the last time this was asked (the frame then has
    /// to be cleared and drawn again, or the old placement stays on screen).
    pub fn take_moved(&mut self) -> bool {
        self.overlay.take_moved()
    }

    /// Write the deletes of the kitty images of the pictures that went, and flush. Nothing is
    /// written when there are none.
    pub fn flush_deletes(&mut self) {
        let deletes = self.cache.take_kitty_deletes();
        if deletes.is_empty() {
            return;
        }
        for escape in deletes {
            if self.out.write_all(escape.as_bytes()).is_err() {
                return;
            }
        }
        let _ = self.out.flush();
    }

    /// How many placements the canvas on screen has (a test reads it).
    pub fn place_count(&self) -> usize {
        self.host.place_count()
    }

    /// Whether a decode job is out (a test reads it).
    pub fn decoding(&self) -> bool {
        self.job.is_some()
    }

    /// Start the job for `images` unless the pictures are the ones already out or on screen
    /// (`force` is the follow-up of pictures that waited, which is the same canvas again).
    fn start_for(&mut self, cols: u16, rows: u16, images: &[CanvasImage], force: bool) {
        let resolved = self.resolve(images);
        if !force {
            let held = match &self.job {
                Some(job) => Some(&job.images),
                None => self.last.as_ref(),
            };
            if held.is_some_and(|h| same_pictures(h, images)) {
                return;
            }
        }
        if let Some(job) = self.job.take() {
            job.cancel.store(true, Ordering::Relaxed);
        }
        self.gen += 1;
        let gen = self.gen;
        let cancel = Arc::new(AtomicBool::new(false));
        let held = self.host.known_keys(&self.cache);
        let (plugin_id, path, raster_px) =
            (self.plugin_id.clone(), self.path.clone(), self.raster_px);
        let svg = Arc::clone(&self.svg);
        let tx = self.done_tx.clone();
        let (flag, job_images) = (Arc::clone(&cancel), resolved.clone());
        std::thread::spawn(move || {
            let cancelled = || flag.load(Ordering::Relaxed);
            let pics = konoma_base::panic::catch_silent(move || {
                let scope = ReadScope::unrestricted();
                let policy = CanvasPolicy::new(&scope, &*svg, raster_px)
                    .cancelled(&cancelled)
                    .trust_own_svg(true);
                let mut canvas = CanvasView::new(cols, rows);
                canvas.images = job_images;
                decode_canvas(&plugin_id, &canvas, &held, &path, &policy)
            });
            // Always answered, even after a panic: the loop waits on the job.
            let _ = tx.send(Done { gen, pics });
        });
        self.job = Some(Job {
            gen,
            cancel,
            images: resolved,
        });
    }

    /// `images` as they will be decoded: an `unchanged` picture whose data never landed gets its
    /// data back, and the data of the others is remembered until it lands.
    fn resolve(&mut self, images: &[CanvasImage]) -> Vec<CanvasImage> {
        let out: Vec<CanvasImage> = images
            .iter()
            .map(|img| {
                let mut img = img.clone();
                match &img.image {
                    Some(data) => {
                        self.unlanded.insert(img.id, data.clone());
                    }
                    None => img.image = self.unlanded.get(&img.id).cloned(),
                }
                img
            })
            .collect();
        self.unlanded
            .retain(|id, _| images.iter().any(|i| i.id == *id));
        out
    }
}

impl Drop for Pictures {
    fn drop(&mut self) {
        if let Some(job) = self.job.take() {
            job.cancel.store(true, Ordering::Relaxed);
        }
        // The right to write for the whole delete, as a frame has it: a signal's cleanup waits
        // for it. Not waited for long, a stuck frame must not hold the exit.
        let _writing = konoma_host_core::term::terminal_write_within(Duration::from_millis(200));
        self.flush_deletes();
        delete_placed_pictures(&mut *self.out);
    }
}

/// Whether the pictures `held` (what is out or on screen, `unchanged` ones resolved) are what
/// `new` (as the session sent it) asks for: the same ids in the same places, and for each one
/// either `unchanged` or the very data (the same shared buffer) already held. A file or an
/// animation sent again is read again: it is never the same.
fn same_pictures(held: &[CanvasImage], new: &[CanvasImage]) -> bool {
    held.len() == new.len()
        && held.iter().zip(new).all(|(h, n)| {
            h.id == n.id
                && (h.x, h.y, h.cols, h.rows) == (n.x, n.y, n.cols, n.rows)
                && match (&h.image, &n.image) {
                    (_, None) => true,
                    (Some(h), Some(n)) => same_data(h, n),
                    (None, Some(_)) => false,
                }
        })
}

fn same_data(a: &ImageData, b: &ImageData) -> bool {
    match (a, b) {
        (ImageData::Encoded(a), ImageData::Encoded(b)) => Arc::ptr_eq(a, b),
        (
            ImageData::Svg {
                data: da,
                base_dir: ba,
                trusted: ta,
            },
            ImageData::Svg {
                data: db,
                base_dir: bb,
                trusted: tb,
            },
        ) => Arc::ptr_eq(da, db) && ba == bb && ta == tb,
        (
            ImageData::Rgba {
                width: wa,
                height: ha,
                pixels: pa,
            },
            ImageData::Rgba {
                width: wb,
                height: hb,
                pixels: pb,
            },
        ) => wa == wb && ha == hb && Arc::ptr_eq(pa, pb),
        (ImageData::Lazy(a), ImageData::Lazy(b)) => {
            std::ptr::eq(Arc::as_ptr(a) as *const (), Arc::as_ptr(b) as *const ())
        }
        _ => false,
    }
}
