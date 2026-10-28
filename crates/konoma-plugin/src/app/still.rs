//! The still picture of a plugin's `View::Image` in the standalone app: the host side of
//! [`crate::canvas_pictures::decode_picture_view`], the part konoma keeps in `src/app/media_*.rs`
//! and `src/ui/preview.rs::render_image`.
//!
//! What is decoded and which failure says what is the shared code ([`decode_picture_view`],
//! [`image_failure_message`]); how it is zoomed, panned and put on the terminal is the shared
//! [`ImageViewer`]. This module only wires them to the app's loop, with the same machinery as
//! konoma:
//!
//! - **The UI never decodes.** A new picture starts a worker thread that runs
//!   [`decode_picture_view`] (an SVG is drawn in this process, [`InProcessSvg`]); a newer picture
//!   cancels the one in flight. Until it lands the frame shows the spinner and the loading word.
//! - **The picture is drawn by [`ImageViewer`]**: konoma's own compressed kitty transfer on a
//!   kitty terminal (the first build on the spot, later ones on a thread), ratatui-image's
//!   protocol elsewhere (resized on a worker, [`ResizeChannel`]).
//! - **A failure says why** (too large, damaged, unsupported format, ...) in konoma's words; with
//!   no reason known the terminal is the suspect.
//! - **An animation** shows its first frame (playing it is not done here yet).
//! - The kitty images of a still are never deleted while the app runs, as in konoma: a placement
//!   with no placeholder cell left shows nothing, and the terminal drops them at exit.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use image::DynamicImage;
use konoma_host_core::help_msg::CoreMsg;
use konoma_host_core::lang::Lang;
use konoma_svg::image::ImageFailure;
use konoma_widgets::image_view::{
    ImageBackend, ImageSource, ImageViewState, ImageViewer, KittyResult, ResizeChannel, ZOOM_FACTOR,
};
use konoma_widgets::loading::{render_spinner_line, spinner_glyph};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::widgets::{Paragraph, Widget};
use ratatui_image::picker::Picker;

use crate::canvas_pictures::{decode_picture_view, image_failure_message, PictureView, SvgDraw};
use crate::view::{ImageData, ImageView};
use crate::ReadScope;

/// How long the loop waits while the picture is being decoded, a kitty build is out or the first
/// resize has not come back (the short poll konoma keeps in the same cases).
const BUSY_POLL: Duration = Duration::from_millis(16);
/// How long a resize that has not come back keeps the short poll going (a resize that failed
/// never comes back).
const RESIZE_WAIT: Duration = Duration::from_secs(2);

/// What a picture is, as far as the keys and the footer care.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StillShape {
    /// Being decoded.
    Loading,
    /// On screen (it can be zoomed).
    Ready,
    /// It cannot be shown.
    Failed,
}

enum State {
    Loading,
    Ready {
        img: Arc<DynamicImage>,
        /// For a vector source: the size of its first raster, so the layout never moves.
        logical: Option<(u32, u32)>,
    },
    /// `None`: no reason is known.
    Failed(Option<ImageFailure>),
}

struct Done {
    gen: u64,
    /// `None`: the worker was cancelled or panicked.
    view: Option<PictureView>,
}

/// The picture on screen and everything that gets it there.
pub struct Still {
    picker: Picker,
    use_kitty: bool,
    is_tmux: bool,
    render_scale: f64,
    plugin_id: String,
    path: PathBuf,
    raster_px: u32,
    svg: Arc<dyn SvgDraw + Send + Sync>,
    resize: ResizeChannel,
    kitty_tx: Sender<KittyResult>,
    kitty_rx: Receiver<KittyResult>,
    done_tx: Sender<Done>,
    done_rx: Receiver<Done>,
    gen: u64,
    cancel: Option<Arc<AtomicBool>>,
    /// The picture the state belongs to (what the session returned for the last view).
    key: Option<ImageView>,
    state: State,
    pose: ImageViewState,
    viewer: ImageViewer,
    spinner: usize,
    /// When a resize was last asked for (the non-kitty path), until its answer is applied.
    resize_asked: Option<Instant>,
    /// The crop at the last draw, to see when a draw asked for a new resize.
    drawn_crop: Option<(u32, u32, u32, u32)>,
}

impl Still {
    pub fn new(
        picker: &Picker,
        use_kitty: bool,
        is_tmux: bool,
        plugin_id: &str,
        path: &Path,
        raster_px: u32,
        svg: Arc<dyn SvgDraw + Send + Sync>,
    ) -> Still {
        let (kitty_tx, kitty_rx) = channel();
        let (done_tx, done_rx) = channel();
        Still {
            picker: picker.clone(),
            use_kitty,
            is_tmux,
            render_scale: 1.0,
            plugin_id: plugin_id.to_string(),
            path: path.to_path_buf(),
            raster_px,
            svg,
            resize: ResizeChannel::spawn(),
            kitty_tx,
            kitty_rx,
            done_tx,
            done_rx,
            gen: 0,
            cancel: None,
            key: None,
            state: State::Loading,
            pose: ImageViewState::default(),
            viewer: ImageViewer::new(),
            spinner: 0,
            resize_asked: None,
            drawn_crop: None,
        }
    }

    /// The session returned `iv`: start decoding it unless it is the picture already held.
    pub fn submit(&mut self, iv: &ImageView) {
        if self.key.as_ref().is_some_and(|k| same_picture(k, iv)) {
            return;
        }
        self.cancel_job();
        self.key = Some(iv.clone());
        self.state = State::Loading;
        self.pose.reset();
        self.viewer.clear();
        self.drawn_crop = None;
        self.resize_asked = None;
        self.gen += 1;
        let gen = self.gen;
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel = Some(Arc::clone(&cancel));
        let (iv, path, raster_px) = (iv.clone(), self.path.clone(), self.raster_px);
        let svg = Arc::clone(&self.svg);
        let tx = self.done_tx.clone();
        std::thread::spawn(move || {
            let cancelled = || cancel.load(Ordering::Relaxed);
            let view = konoma_base::panic::catch_silent(move || {
                let scope = ReadScope::unrestricted();
                decode_picture_view(iv, &path, raster_px, &cancelled, true, &scope, &*svg)
            })
            .flatten();
            // Always answered, even after a panic: the loop waits on the job.
            let _ = tx.send(Done { gen, view });
        });
    }

    /// The session's view is not a picture (any more): drop it and cancel the decode.
    pub fn forget(&mut self) {
        self.cancel_job();
        self.key = None;
        self.state = State::Loading;
        self.pose.reset();
        self.viewer.clear();
        self.drawn_crop = None;
        self.resize_asked = None;
    }

    fn cancel_job(&mut self) {
        if let Some(c) = self.cancel.take() {
            c.store(true, Ordering::Relaxed);
        }
    }

    /// Whether a picture (loading, shown or failed) is held.
    pub fn holds(&self) -> bool {
        self.key.is_some()
    }

    pub fn shape(&self) -> StillShape {
        match self.state {
            State::Loading => StillShape::Loading,
            State::Ready { .. } => StillShape::Ready,
            State::Failed(_) => StillShape::Failed,
        }
    }

    /// The zoom (1.0 = the whole picture fits).
    pub fn zoom(&self) -> f64 {
        self.pose.zoom
    }

    /// The centre of the visible window, each axis in 0..=1.
    pub fn center(&self) -> (f64, f64) {
        self.pose.center
    }

    /// Zoom in by one step (`+1`) or out (`-1`). Nothing happens unless a picture is shown.
    pub fn zoom_steps(&mut self, dir: i32) {
        if self.shape() != StillShape::Ready {
            return;
        }
        self.pose.zoom_by(if dir >= 0 {
            ZOOM_FACTOR
        } else {
            1.0 / ZOOM_FACTOR
        });
    }

    /// Back to fit and centred.
    pub fn zoom_reset(&mut self) {
        if self.shape() == StillShape::Ready {
            self.pose.reset();
        }
    }

    /// Move the visible window one step in `(dx, dy)` (each -1, 0 or 1).
    pub fn pan(&mut self, dx: f64, dy: f64) {
        if self.shape() == StillShape::Ready {
            self.viewer.pan(&mut self.pose, dx, dy);
        }
    }

    /// Apply what the workers finished: the decode, the resizes, the kitty builds; advance the
    /// spinner. Returns whether anything changed on screen.
    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        while let Ok(done) = self.done_rx.try_recv() {
            if done.gen != self.gen {
                continue; // superseded
            }
            self.cancel = None;
            self.state = match done.view {
                Some(PictureView::Still(img)) => State::Ready {
                    img: Arc::new(img),
                    logical: None,
                },
                Some(PictureView::Vector { img, .. }) => {
                    let logical = Some((img.width(), img.height()));
                    State::Ready {
                        img: Arc::new(img),
                        logical,
                    }
                }
                // The first frame (the animation is not played here yet).
                Some(PictureView::Gif(frames)) => match frames.into_iter().next() {
                    Some((img, _)) => State::Ready {
                        img: Arc::new(img),
                        logical: None,
                    },
                    None => State::Failed(None),
                },
                Some(PictureView::Failed(why)) => State::Failed(Some(why)),
                None => State::Failed(None),
            };
            changed = true;
        }
        while let Some(resp) = self.resize.try_recv() {
            if self.viewer.apply_resize(resp) {
                self.resize_asked = None;
                changed = true;
            }
        }
        while let Ok(res) = self.kitty_rx.try_recv() {
            changed |= self.viewer.apply_kitty(res);
        }
        if matches!(self.state, State::Loading) && self.key.is_some() {
            self.spinner = self.spinner.wrapping_add(1);
            changed = true;
        }
        changed
    }

    /// How long the loop may wait before it has to look at the picture again.
    pub fn wake_in(&self) -> Option<Duration> {
        let loading = self.key.is_some() && matches!(self.state, State::Loading);
        let resizing = self
            .resize_asked
            .is_some_and(|at| at.elapsed() < RESIZE_WAIT);
        (loading || self.viewer.kitty_build_pending() || resizing).then_some(BUSY_POLL)
    }

    /// Draw into `inner` (the inside of the frame's border): the spinner while it loads, the
    /// reason when it failed, else the picture; when nothing could be drawn the failure line.
    pub fn draw(&mut self, buf: &mut Buffer, inner: Rect, lang: Lang) {
        if inner.width == 0 || inner.height == 0 {
            return;
        }
        match &self.state {
            State::Loading => {
                render_spinner_line(
                    buf,
                    inner,
                    spinner_glyph(self.spinner),
                    CoreMsg::Loading.tr(lang),
                );
                return;
            }
            State::Failed(why) => {
                let msg = failure_line(why.as_ref(), lang, &self.plugin_id);
                Paragraph::new(msg).render(inner, buf);
                return;
            }
            State::Ready { .. } => {}
        }
        let State::Ready { img, logical } = &self.state else {
            return;
        };
        let backend = ImageBackend {
            picker: &self.picker,
            use_kitty: self.use_kitty,
            is_tmux: self.is_tmux,
            resize_tx: Some(&self.resize.tx),
            kitty_tx: Some(&self.kitty_tx),
            render_scale: self.render_scale,
            // The filter konoma resizes with (nearest-neighbour aliases and blocks).
            filter: ratatui_image::FilterType::Lanczos3,
        };
        let source = ImageSource {
            image: img,
            logical: *logical,
        };
        let target = self.viewer.prepare(&mut self.pose, source, &backend, inner);
        if !self.use_kitty && self.viewer.crop() != self.drawn_crop {
            self.drawn_crop = self.viewer.crop();
            self.resize_asked = Some(Instant::now());
        }
        let drawn = target.is_some_and(|t| self.viewer.render(t, buf));
        if !drawn {
            let msg = failure_line(None, lang, &self.plugin_id);
            Paragraph::new(msg).render(inner, buf);
        }
    }
}

/// The line shown in place of a picture that cannot be shown: `[image] <reason>`, or, when no
/// reason is known, konoma's sentence about the terminal.
pub fn failure_line(why: Option<&ImageFailure>, lang: Lang, plugin_id: &str) -> String {
    match why.and_then(|f| image_failure_message(*f, lang, plugin_id)) {
        Some(why) => format!("[image] {why}"),
        None => CoreMsg::ImageUnsupported.tr(lang).to_string(),
    }
}

/// Whether the session returned the picture `a` already holds when it returns `b`: the same file,
/// the same drawn bytes. (A file is not read again on every key.)
fn same_picture(a: &ImageView, b: &ImageView) -> bool {
    a.sharp_zoom == b.sharp_zoom
        && a.raster_max_px == b.raster_max_px
        && match (&a.data, &b.data) {
            (ImageData::File(x), ImageData::File(y)) => x == y,
            (ImageData::Encoded(x), ImageData::Encoded(y)) => Arc::ptr_eq(x, y) || x == y,
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
            ) => ba == bb && ta == tb && (Arc::ptr_eq(da, db) || da == db),
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
            ) => wa == wb && ha == hb && (Arc::ptr_eq(pa, pb) || pa == pb),
            // Animations and lazy sources have no cheap identity: never the same.
            _ => false,
        }
}

impl Drop for Still {
    fn drop(&mut self) {
        self.cancel_job();
    }
}
