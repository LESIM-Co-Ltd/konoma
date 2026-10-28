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
//! - **An animation plays** as in konoma: the shared [`GifPlayer`] steps the frames on their own
//!   delays (looping), each frame encoded synchronously to the display size and drawn through
//!   ratatui-image's protocol on every terminal (konoma does the same, kitty included). Zoom and
//!   pan are the still picture's own [`ImageViewState`] and [`ImageViewer`]. The loop sleeps until
//!   the next frame is due ([`Still::wake_in_at`]) and not shorter; a one-frame GIF is a still.
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
use konoma_widgets::gif_player::GifPlayer;
use konoma_widgets::image_view::{
    ImageBackend, ImageSource, ImageViewState, ImageViewer, KittyResult, ResizeChannel, ZOOM_FACTOR,
};
use konoma_widgets::inline_images::SvgTrust;
use konoma_widgets::loading::{render_spinner_line, spinner_glyph};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::widgets::{Paragraph, Widget};
use ratatui_image::picker::Picker;
use ratatui_image::Image;

use crate::canvas_pictures::{
    decode_picture_view, image_failure_message, redraw_svg, sharp_zoom_target, PictureView, SvgDraw,
};
use crate::view::{ImageData, ImageView};
use crate::ReadScope;

/// What the protocol was last resized for: the crop of the source and the cells it is drawn in.
type Geometry = (Option<(u32, u32, u32, u32)>, Option<Rect>);

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
    /// An animation (two frames or more), held by [`Still::gif`].
    Gif,
    /// `None`: no reason is known.
    Failed(Option<ImageFailure>),
}

struct Done {
    gen: u64,
    /// `None`: the worker was cancelled or panicked.
    view: Option<PictureView>,
}

/// The answer of a sharp-zoom redraw.
struct SharpDone {
    gen: u64,
    /// `None`: the drawing failed, was cancelled or panicked.
    img: Option<DynamicImage>,
}

/// The SVG of the picture on screen and how it was drawn, kept for a sharp-zoom redraw.
struct VectorSource {
    svg: Arc<Vec<u8>>,
    trust: SvgTrust,
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
    sharp_tx: Sender<SharpDone>,
    sharp_rx: Receiver<SharpDone>,
    gen: u64,
    cancel: Option<Arc<AtomicBool>>,
    /// The retained SVG of a vector picture that asks for sharp zoom (`ImageView::sharp_zoom`).
    vector: Option<VectorSource>,
    /// A sharp redraw is out (one at a time, as in konoma); its answer carries the `gen` it was
    /// started under, so a newer picture makes it stale.
    sharp_inflight: bool,
    sharp_cancel: Option<Arc<AtomicBool>>,
    /// The picture the state belongs to (what the session returned for the last view).
    key: Option<ImageView>,
    state: State,
    /// The picture in `state` is the one of the page before: the new page is being decoded and the
    /// old picture stays on screen, at fit and still zoomable, until it lands (as the host keeps
    /// its old page).
    held: bool,
    pose: ImageViewState,
    viewer: ImageViewer,
    /// The frames and the playback position of a [`State::Gif`]; empty in every other state.
    gif: GifPlayer,
    spinner: usize,
    /// When a resize was last asked for (the non-kitty path), until its answer is applied.
    resize_asked: Option<Instant>,
    /// The geometry at the last draw, to see when a draw asked for a new resize.
    drawn_geometry: Option<Geometry>,
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
        let (sharp_tx, sharp_rx) = channel();
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
            sharp_tx,
            sharp_rx,
            gen: 0,
            cancel: None,
            vector: None,
            sharp_inflight: false,
            sharp_cancel: None,
            key: None,
            state: State::Loading,
            held: false,
            pose: ImageViewState::default(),
            viewer: ImageViewer::new(),
            gif: GifPlayer::new(),
            spinner: 0,
            resize_asked: None,
            drawn_geometry: None,
        }
    }

    /// The session returned `iv`: start decoding it unless it is the picture already held.
    ///
    /// `keep_old` is a page turn: a picture on screen stays there, back at fit, until the new one
    /// lands (the spinner shows meanwhile). Without it, or with no picture on screen, the frame
    /// shows the loading line.
    pub fn submit(&mut self, iv: &ImageView, keep_old: bool) {
        if self.key.as_ref().is_some_and(|k| same_picture(k, iv)) {
            return;
        }
        self.cancel_job();
        self.key = Some(iv.clone());
        if keep_old && matches!(self.state, State::Ready { .. } | State::Gif) {
            // The page before stays on screen in the pose it has (the loop has already put it back
            // to fit, [`Still::turn_page`], and the keys move it from there as on the host).
            self.held = true;
        } else {
            self.pose.reset();
            self.held = false;
            self.state = State::Loading;
            self.gif.clear();
            self.viewer.clear();
        }
        if !self.held {
            self.drawn_geometry = None;
            self.resize_asked = None;
        }
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
        self.held = false;
        self.state = State::Loading;
        self.gif.clear();
        self.pose.reset();
        self.viewer.clear();
        self.drawn_geometry = None;
        self.resize_asked = None;
    }

    fn cancel_job(&mut self) {
        if let Some(c) = self.cancel.take() {
            c.store(true, Ordering::Relaxed);
        }
        // A redraw of the picture that is going away is dropped with it (its answer is stale by
        // `gen` as well).
        if let Some(c) = self.sharp_cancel.take() {
            c.store(true, Ordering::Relaxed);
        }
        self.sharp_inflight = false;
        self.vector = None;
    }

    /// Sharp zoom, as konoma's `maybe_sharpen_vector`: when the zoom has outgrown the density of
    /// the raster on screen, draw the retained SVG again at the size the zoom needs, on a worker,
    /// and swap it in when it lands. Until then the pixel zoom shows. One redraw at a time; the
    /// answer is judged again against the zoom of that moment.
    fn maybe_sharpen(&mut self) {
        if self.sharp_inflight || self.held {
            return;
        }
        let (
            State::Ready {
                img,
                logical: Some(logical),
            },
            Some(v),
        ) = (&self.state, &self.vector)
        else {
            return;
        };
        let Some(want) = sharp_zoom_target((img.width(), img.height()), *logical, self.pose.zoom)
        else {
            return;
        };
        self.sharp_inflight = true;
        let cancel = Arc::new(AtomicBool::new(false));
        self.sharp_cancel = Some(Arc::clone(&cancel));
        let (svg, trust, gen) = (Arc::clone(&v.svg), v.trust.clone(), self.gen);
        let draw = Arc::clone(&self.svg);
        let tx = self.sharp_tx.clone();
        std::thread::spawn(move || {
            let cancelled = || cancel.load(Ordering::Relaxed);
            let img = konoma_base::panic::catch_silent(move || {
                redraw_svg(&svg, &trust, want, &*draw, &cancelled).ok()
            })
            .flatten();
            // Always answered, even after a panic: the in-flight flag waits on it.
            let _ = tx.send(SharpDone { gen, img });
        });
    }

    /// A page key was pressed: the picture on screen goes back to fit and centred at once (the
    /// host's `goto_page`), before the new page is even asked for. What is zoomed or panned
    /// while the new page is awaited stays when it lands.
    pub fn turn_page(&mut self) {
        self.pose.reset();
    }

    /// Whether a picture (loading, shown or failed) is held.
    pub fn holds(&self) -> bool {
        self.key.is_some()
    }

    /// Whether a picture is being decoded: the loading line, or a page turn that keeps the page
    /// before on screen.
    pub fn loading(&self) -> bool {
        self.key.is_some() && (self.held || matches!(self.state, State::Loading))
    }

    /// The spinner's glyph now.
    pub fn spinner_now(&self) -> &'static str {
        spinner_glyph(self.spinner)
    }

    pub fn shape(&self) -> StillShape {
        match self.state {
            State::Loading => StillShape::Loading,
            State::Ready { .. } | State::Gif => StillShape::Ready,
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
        self.maybe_sharpen();
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
    /// spinner; step the animation if its frame is due at `now` (the animation's clock, which
    /// tests set themselves). Returns whether anything changed on screen.
    pub fn poll_at(&mut self, now: Instant) -> bool {
        let mut changed = false;
        while let Ok(done) = self.done_rx.try_recv() {
            if done.gen != self.gen {
                continue; // superseded
            }
            self.cancel = None;
            if self.held {
                // The new page replaces the one kept on screen: nothing of the old protocol is
                // reused (the pose is, as the host's: it is not touched when a picture lands).
                self.held = false;
                self.viewer.clear();
                self.drawn_geometry = None;
                self.resize_asked = None;
            }
            // Whatever lands replaces an animation that was held: its frames go with it.
            self.gif.clear();
            let mut vector = None;
            self.state = match done.view {
                Some(PictureView::Still(img)) => State::Ready {
                    img: Arc::new(img),
                    logical: None,
                },
                Some(PictureView::Vector { img, svg, trust }) => {
                    let logical = Some((img.width(), img.height()));
                    // The decoder returns a vector only for a picture that asks for sharp zoom.
                    vector = Some(VectorSource { svg, trust });
                    State::Ready {
                        img: Arc::new(img),
                        logical,
                    }
                }
                // Two frames or more play; one frame is a still picture.
                Some(PictureView::Gif(frames)) => match frames.len() {
                    0 => State::Failed(None),
                    1 => State::Ready {
                        img: Arc::new(frames.into_iter().next().map(|f| f.0).unwrap_or_default()),
                        logical: None,
                    },
                    _ => {
                        // No `set_crop(None)` as konoma's `set_gif_frames` has: the viewer
                        // was cleared on `submit` (or just above, for a held page), so its crop
                        // is already `None` whenever a picture lands.
                        self.gif.set_frames(frames);
                        State::Gif
                    }
                },
                Some(PictureView::Failed(why)) => State::Failed(Some(why)),
                None => State::Failed(None),
            };
            self.vector = vector;
            // The zoom may have advanced while the picture was being decoded (a page turn keeps
            // the pose): converge on it.
            self.maybe_sharpen();
            changed = true;
        }
        while let Ok(done) = self.sharp_rx.try_recv() {
            if done.gen != self.gen || !self.sharp_inflight {
                continue; // a picture came after it started
            }
            self.sharp_inflight = false;
            self.sharp_cancel = None;
            // A failed redraw leaves the raster as it is and is not retried until the next zoom.
            let Some(sharp) = done.img else { continue };
            if let State::Ready { img, .. } = &mut self.state {
                // Only the pixels change: `logical` stays, so the layout never moves.
                *img = Arc::new(sharp);
                self.viewer.set_crop(None);
                self.viewer.invalidate();
                self.drawn_geometry = None;
                changed = true;
                self.maybe_sharpen();
            }
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
        // The frame that is due, once per call (a late tick advances one frame, as konoma's).
        if matches!(self.state, State::Gif) && self.gif.advance_if_due(now) {
            changed = true;
        }
        if self.loading() {
            self.spinner = self.spinner.wrapping_add(1);
            changed = true;
        }
        changed
    }

    /// How long the loop may wait before it has to look at the picture again, at the time `now`.
    /// While an animation plays the wait is the time left of the frame on screen (konoma's
    /// `gif_poll_timeout`); with none playing it adds nothing.
    pub fn wake_in_at(&self, now: Instant) -> Option<Duration> {
        let busy = self.busy_wake();
        let frame = matches!(self.state, State::Gif)
            .then(|| self.gif.poll_timeout(now))
            .flatten();
        match (busy, frame) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    fn busy_wake(&self) -> Option<Duration> {
        let loading = self.loading();
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
            State::Gif => {
                // Prepared (laid out and encoded to the display size) and drawn as one piece,
                // as konoma's `render_image` does for an animation.
                let target = self.gif.prepare(
                    &mut self.pose,
                    &mut self.viewer,
                    &self.picker,
                    self.render_scale,
                    inner,
                );
                match (target, self.gif.protocol()) {
                    (Some(target), Some(proto)) => Image::new(proto).render(target, buf),
                    _ => {
                        let msg = failure_line(None, lang, &self.plugin_id);
                        Paragraph::new(msg).render(inner, buf);
                    }
                }
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
        // The protocol is resized on the worker whenever the crop or the size it is drawn at
        // changes (a zoom at fit changes only the size), and the loop must wake to take the answer.
        let geometry = (self.viewer.crop(), target);
        if !self.use_kitty && Some(geometry) != self.drawn_geometry {
            self.drawn_geometry = Some(geometry);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::pictures::InProcessSvg;
    use konoma_test_support::{build_animated_gif, unique_tmp as unique, TmpDir};

    fn iv(data: ImageData) -> ImageView {
        ImageView::new(data)
    }

    fn svg(data: &[u8], base: Option<&str>, trusted: bool) -> ImageData {
        ImageData::Svg {
            data: Arc::from(data),
            base_dir: base.map(PathBuf::from),
            trusted,
        }
    }

    fn rgba(w: u32, h: u32, fill: u8) -> ImageData {
        ImageData::Rgba {
            width: w,
            height: h,
            pixels: Arc::new(vec![fill; (w * h * 4) as usize]),
        }
    }

    /// The same bytes are the same picture whichever allocation holds them; any differing field
    /// is another picture. Counts how many comparisons were made.
    #[test]
    fn a_picture_is_the_same_only_when_every_part_that_is_drawn_is_the_same() {
        let mut same = 0;
        let mut other = 0;
        let mut check = |a: ImageView, b: ImageView, want: bool, what: &str| {
            assert_eq!(same_picture(&a, &b), want, "{what}");
            assert_eq!(same_picture(&b, &a), want, "{what} (swapped)");
            if want {
                same += 1;
            } else {
                other += 1;
            }
        };
        let file = |p: &str| iv(ImageData::File(PathBuf::from(p)));
        check(file("/a.png"), file("/a.png"), true, "file, same path");
        check(file("/a.png"), file("/b.png"), false, "file, other path");

        let shared: Arc<[u8]> = Arc::from(&b"png bytes"[..]);
        let enc = |a: &Arc<[u8]>| iv(ImageData::Encoded(Arc::clone(a)));
        check(enc(&shared), enc(&shared), true, "encoded, same allocation");
        check(
            enc(&shared),
            iv(ImageData::Encoded(Arc::from(&b"png bytes"[..]))),
            true,
            "encoded, equal bytes",
        );
        check(
            enc(&shared),
            iv(ImageData::Encoded(Arc::from(&b"png bytez"[..]))),
            false,
            "encoded, other bytes",
        );

        let s = |d: &[u8], b: Option<&str>, t: bool| iv(svg(d, b, t));
        check(
            s(b"<svg/>", Some("/d"), false),
            s(b"<svg/>", Some("/d"), false),
            true,
            "svg, equal",
        );
        check(
            s(b"<svg/>", Some("/d"), false),
            s(b"<svg />", Some("/d"), false),
            false,
            "svg, other data",
        );
        check(
            s(b"<svg/>", Some("/d"), false),
            s(b"<svg/>", Some("/e"), false),
            false,
            "svg, other base_dir",
        );
        check(
            s(b"<svg/>", Some("/d"), false),
            s(b"<svg/>", None, false),
            false,
            "svg, base_dir dropped",
        );
        check(
            s(b"<svg/>", Some("/d"), false),
            s(b"<svg/>", Some("/d"), true),
            false,
            "svg, other trust",
        );

        check(iv(rgba(2, 2, 7)), iv(rgba(2, 2, 7)), true, "rgba, equal");
        check(
            iv(rgba(2, 2, 7)),
            iv(rgba(2, 2, 8)),
            false,
            "rgba, other pixel",
        );
        check(
            iv(rgba(2, 2, 7)),
            iv(rgba(4, 1, 7)),
            false,
            "rgba, other shape",
        );

        let mut a = file("/a.png");
        let mut b = file("/a.png");
        b.sharp_zoom = true;
        check(a.clone(), b, false, "other sharp_zoom");
        let mut c = file("/a.png");
        c.raster_max_px = Some(512);
        check(a.clone(), c.clone(), false, "other raster_max_px");
        a.raster_max_px = Some(512);
        check(a, c, true, "equal raster_max_px");

        check(file("/a.png"), enc(&shared), false, "file against encoded");
        check(
            iv(rgba(1, 1, 0)),
            file("/a.png"),
            false,
            "rgba against file",
        );
        // No cheap identity: an animation is never held.
        let frames = || {
            iv(ImageData::Frames(vec![(
                rgba(1, 1, 0),
                Duration::from_millis(10),
            )]))
        };
        check(frames(), frames(), false, "frames, never the same");

        assert!(same >= 6 && other >= 13, "same {same}, other {other}");
    }

    // ---- the animation ----

    fn unique_tmp(name: &str) -> TmpDir {
        let dir = unique(name);
        std::fs::create_dir_all(dir.as_path()).unwrap();
        dir
    }

    const RED: [u8; 3] = [255, 0, 0];
    const BLUE: [u8; 3] = [0, 0, 255];
    const GREEN: [u8; 3] = [0, 255, 0];
    const TMO: Duration = Duration::from_secs(20);

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    fn new_still(use_kitty: bool, path: &Path) -> Still {
        Still::new(
            &Picker::halfblocks(),
            use_kitty,
            false,
            "t",
            path,
            800,
            Arc::new(InProcessSvg),
        )
    }

    /// Submit `file` and let the worker land it (the clock is `t0` throughout, so the
    /// animation's timing starts there and nothing waits on real time).
    fn land(s: &mut Still, file: &Path, keep_old: bool, t0: Instant) {
        s.submit(
            &ImageView::new(ImageData::File(file.to_path_buf())),
            keep_old,
        );
        let started = Instant::now();
        while s.loading() {
            s.poll_at(t0);
            assert!(started.elapsed() < TMO, "the picture never landed");
            std::thread::sleep(ms(2));
        }
    }

    fn gif_file(dir: &TmpDir, name: &str, w: u32, frames: &[([u8; 3], u32)]) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, build_animated_gif(w, w, frames)).unwrap();
        p
    }

    fn png_file(dir: &TmpDir, name: &str) -> PathBuf {
        let p = dir.join(name);
        image::RgbaImage::from_pixel(60, 60, image::Rgba([0, 255, 0, 255]))
            .save(&p)
            .unwrap();
        p
    }

    /// The colors the picture cells show: the (fg, bg) of every cell of `buf` that has one.
    fn colors(buf: &Buffer) -> std::collections::BTreeSet<String> {
        let mut out = std::collections::BTreeSet::new();
        for c in buf.content() {
            out.insert(format!("{:?}/{:?}", c.fg, c.bg));
        }
        out
    }

    fn drawn(s: &mut Still) -> Buffer {
        let area = Rect::new(0, 0, 40, 12);
        let mut buf = Buffer::empty(area);
        s.draw(&mut buf, area, Lang::En);
        buf
    }

    fn has_rgb(buf: &Buffer, want: [u8; 3]) -> bool {
        buf.content().iter().any(|c| {
            c.fg == ratatui::style::Color::Rgb(want[0], want[1], want[2])
                || c.bg == ratatui::style::Color::Rgb(want[0], want[1], want[2])
        })
    }

    #[test]
    fn an_animation_steps_its_frames_on_their_own_delays_and_loops() {
        let dir = unique_tmp("still_gif_steps");
        let f = gif_file(&dir, "a.gif", 40, &[(RED, 100), (BLUE, 200), (GREEN, 100)]);
        let mut s = new_still(false, &f);
        let t0 = Instant::now();
        land(&mut s, &f, false, t0);
        assert!(matches!(s.state, State::Gif));
        assert_eq!(s.shape(), StillShape::Ready);
        assert_eq!(s.gif.frame_index(), 0);
        // The clock started at t0: 99 ms is early, 100 ms is due.
        assert!(!s.poll_at(t0 + ms(99)));
        assert_eq!(s.gif.frame_index(), 0);
        assert!(s.poll_at(t0 + ms(100)), "a new frame is a change on screen");
        assert_eq!(s.gif.frame_index(), 1);
        // The second frame is shown for 200 ms from the tick that advanced.
        assert!(!s.poll_at(t0 + ms(299)));
        assert!(s.poll_at(t0 + ms(300)));
        assert_eq!(s.gif.frame_index(), 2);
        assert!(!s.poll_at(t0 + ms(399)));
        assert!(s.poll_at(t0 + ms(400)));
        assert_eq!(
            s.gif.frame_index(),
            0,
            "after the last frame the first returns"
        );
        // A late tick advances one frame, not several.
        assert!(s.poll_at(t0 + ms(10_000)));
        assert_eq!(s.gif.frame_index(), 1);
    }

    #[test]
    fn each_frame_is_drawn_and_a_kitty_terminal_draws_the_same_cells() {
        let dir = unique_tmp("still_gif_draw");
        let f = gif_file(&dir, "a.gif", 40, &[(RED, 100), (BLUE, 100)]);
        let t0 = Instant::now();
        let mut plain = new_still(false, &f);
        let mut kitty = new_still(true, &f);
        land(&mut plain, &f, false, t0);
        land(&mut kitty, &f, false, t0);
        let (p0, k0) = (drawn(&mut plain), drawn(&mut kitty));
        assert!(has_rgb(&p0, RED) && !has_rgb(&p0, BLUE), "the first frame");
        assert_eq!(p0, k0, "kitty and the others draw an animation alike");
        plain.poll_at(t0);
        plain.poll_at(t0 + ms(100));
        let p1 = drawn(&mut plain);
        assert!(has_rgb(&p1, BLUE) && !has_rgb(&p1, RED), "the second frame");
        assert_ne!(colors(&p0), colors(&p1));
        plain.poll_at(t0 + ms(200));
        assert_eq!(drawn(&mut plain), p0, "and around again to the first");
    }

    #[test]
    fn the_loop_wakes_for_the_next_frame_and_for_nothing_else() {
        let dir = unique_tmp("still_gif_wake");
        let t0 = Instant::now();
        let g = gif_file(&dir, "a.gif", 40, &[(RED, 60), (BLUE, 60)]);
        let mut s = new_still(false, &g);
        land(&mut s, &g, false, t0);
        // Timing has started at t0: the time left of the frame on screen.
        assert_eq!(s.wake_in_at(t0), Some(ms(60)));
        assert_eq!(s.wake_in_at(t0 + ms(35)), Some(ms(25)));
        // Never shorter than konoma's floor, never longer than its ceiling.
        assert_eq!(
            s.wake_in_at(t0 + ms(58)),
            Some(konoma_widgets::gif_player::POLL_MIN)
        );
        let slow = gif_file(&dir, "slow.gif", 40, &[(RED, 5000), (BLUE, 5000)]);
        land(&mut s, &slow, false, t0);
        assert_eq!(s.wake_in_at(t0), Some(konoma_widgets::gif_player::POLL_MAX));
        // A still picture, a failure and a held-nothing Still need no wake at all.
        let png = png_file(&dir, "a.png");
        land(&mut s, &png, false, t0);
        assert_eq!(s.wake_in_at(t0), None, "a still never wakes the loop");
        assert_eq!(s.wake_in_at(t0 + ms(100_000)), None);
        let junk = dir.join("junk.gif");
        std::fs::write(&junk, b"GIF89a-not-a-gif").unwrap();
        land(&mut s, &junk, false, t0);
        assert_eq!(s.shape(), StillShape::Failed);
        assert_eq!(s.wake_in_at(t0), None, "a failure never wakes the loop");
    }

    #[test]
    fn zoom_and_pan_move_the_window_over_an_animation() {
        let dir = unique_tmp("still_gif_zoom");
        let f = gif_file(&dir, "a.gif", 400, &[(RED, 100), (BLUE, 100)]);
        let t0 = Instant::now();
        let mut s = new_still(false, &f);
        land(&mut s, &f, false, t0);
        drawn(&mut s);
        let fit = s.viewer.crop().expect("laid out at fit");
        assert_eq!(s.zoom(), 1.0);
        s.zoom_steps(1);
        s.zoom_steps(1);
        assert!(s.zoom() > 1.0);
        drawn(&mut s);
        let zoomed = s.viewer.crop().expect("laid out zoomed");
        assert!(zoomed.2 * zoomed.3 < fit.2 * fit.3, "{fit:?} -> {zoomed:?}");
        let c0 = s.center();
        s.pan(1.0, 1.0);
        drawn(&mut s);
        // Only the clipped axis (the vertical one here) moves.
        assert!(s.center().1 > c0.1, "panned");
        assert_eq!(s.center().0, c0.0, "the unclipped axis stays");
        assert_ne!(s.viewer.crop(), Some(zoomed));
        // A frame change keeps the pose.
        let pose = (s.zoom(), s.center());
        s.poll_at(t0);
        assert!(s.poll_at(t0 + ms(100)));
        drawn(&mut s);
        assert_eq!(pose, (s.zoom(), s.center()));
        s.zoom_reset();
        assert_eq!((s.zoom(), s.center()), (1.0, (0.5, 0.5)));
    }

    #[test]
    fn leaving_an_animation_stops_it_and_leaves_no_frame_behind() {
        let dir = unique_tmp("still_gif_leave");
        let g = gif_file(&dir, "a.gif", 40, &[(RED, 100), (BLUE, 100)]);
        let png = png_file(&dir, "a.png");
        let junk = dir.join("junk.png");
        std::fs::write(&junk, b"\x89PNG\r\n\x1a\nnot a picture").unwrap();
        let g2 = gif_file(&dir, "b.gif", 40, &[(GREEN, 100), (BLUE, 100)]);
        let t0 = Instant::now();
        // Each way out: a still, a failure, the next page (kept on screen meanwhile), a
        // different animation, and the session dropping the picture.
        for (way, next) in [("still", &png), ("failure", &junk), ("held page", &png)] {
            let mut s = new_still(false, &g);
            land(&mut s, &g, false, t0);
            s.poll_at(t0);
            assert!(s.poll_at(t0 + ms(100)), "{way}: it plays before");
            land(&mut s, next, way == "held page", t0);
            assert!(!s.gif.has_frames(), "{way}: the frames are gone");
            assert!(!s.poll_at(t0 + ms(50_000)), "{way}: nothing advances");
            assert_eq!(s.gif.frame_index(), 0, "{way}");
            assert_eq!(s.wake_in_at(t0 + ms(50_000)), None, "{way}");
            let buf = drawn(&mut s);
            assert!(
                !has_rgb(&buf, BLUE) && !has_rgb(&buf, RED),
                "{way}: no frame of the old animation is drawn"
            );
        }
        // Another animation starts from its own first frame.
        let mut s = new_still(false, &g);
        land(&mut s, &g, false, t0);
        s.poll_at(t0);
        s.poll_at(t0 + ms(100));
        assert_eq!(s.gif.frame_index(), 1);
        land(&mut s, &g2, true, t0);
        assert_eq!(s.gif.frame_index(), 0);
        assert!(has_rgb(&drawn(&mut s), GREEN));
        // The session's view is no picture any more.
        s.forget();
        assert!(!s.holds() && !s.gif.has_frames());
        assert!(!s.poll_at(t0 + ms(50_000)));
        assert_eq!(s.wake_in_at(t0 + ms(50_000)), None);
    }

    #[test]
    fn a_new_picture_without_a_turn_drops_the_old_frames_at_once() {
        // Not held: the old animation's frames are freed when the next picture is asked for,
        // not only when it lands (a big animation would sit in memory under the spinner).
        let dir = unique_tmp("still_gif_free");
        let g = gif_file(&dir, "a.gif", 40, &[(RED, 100), (BLUE, 100)]);
        let png = png_file(&dir, "a.png");
        let t0 = Instant::now();
        let mut s = new_still(false, &g);
        land(&mut s, &g, false, t0);
        assert!(s.gif.has_frames());
        s.submit(&ImageView::new(ImageData::File(png)), false);
        assert!(!s.gif.has_frames(), "freed while the next one loads");
        assert!(matches!(s.state, State::Loading));
        // The session dropping the picture frees them too.
        let mut s = new_still(false, &g);
        land(&mut s, &g, false, t0);
        s.forget();
        assert!(!s.gif.has_frames());
    }

    #[test]
    fn an_animation_landing_over_a_held_still_starts_at_fit_with_no_crop_of_the_old_one() {
        let dir = unique_tmp("still_gif_crop");
        let png = png_file(&dir, "a.png");
        let g = gif_file(&dir, "a.gif", 400, &[(RED, 100), (BLUE, 100)]);
        let t0 = Instant::now();
        let mut s = new_still(false, &png);
        land(&mut s, &png, false, t0);
        s.zoom_steps(2);
        // Draw until the zoomed still has laid itself out (its resize runs on a worker).
        let started = Instant::now();
        while s.viewer.crop().is_none() {
            drawn(&mut s);
            s.poll_at(t0);
            assert!(started.elapsed() < TMO, "the still never laid out");
            std::thread::sleep(ms(2));
        }
        s.turn_page();
        land(&mut s, &g, true, t0);
        assert!(matches!(s.state, State::Gif));
        assert_eq!(s.viewer.crop(), None, "the old picture's crop is gone");
        assert_eq!(s.zoom(), 1.0);
    }

    #[test]
    fn a_page_waiting_keeps_the_playing_animation_on_screen_until_it_lands() {
        let dir = unique_tmp("still_gif_held");
        let g = gif_file(&dir, "a.gif", 40, &[(RED, 100), (BLUE, 100)]);
        let png = png_file(&dir, "a.png");
        let t0 = Instant::now();
        let mut s = new_still(false, &g);
        land(&mut s, &g, false, t0);
        s.poll_at(t0);
        s.submit(&ImageView::new(ImageData::File(png.clone())), true);
        // Until the new page lands the old one stays and still plays.
        assert!(s.loading());
        // The wait is the nearer of the busy poll and the frame still on screen.
        assert!(s.gif.is_active() && s.held);
        assert_eq!(
            s.wake_in_at(t0),
            Some(BUSY_POLL),
            "busy poll is nearer than 100 ms"
        );
        assert_eq!(
            s.wake_in_at(t0 + ms(99)),
            Some(konoma_widgets::gif_player::POLL_MIN),
            "the frame is nearer than the busy poll"
        );
        let started = Instant::now();
        while s.loading() {
            s.poll_at(t0);
            assert!(started.elapsed() < TMO);
            std::thread::sleep(ms(2));
        }
        assert!(matches!(s.state, State::Ready { .. }));
        assert!(!s.gif.has_frames());
    }

    #[test]
    fn a_one_frame_gif_is_a_still_picture() {
        let dir = unique_tmp("still_gif_one");
        let f = gif_file(&dir, "one.gif", 40, &[(RED, 100)]);
        let t0 = Instant::now();
        let mut s = new_still(false, &f);
        land(&mut s, &f, false, t0);
        assert!(matches!(s.state, State::Ready { .. }), "drawn as a still");
        assert!(!s.gif.has_frames() && !s.gif.is_active());
        assert!(!s.poll_at(t0 + ms(50_000)));
        assert_eq!(s.wake_in_at(t0), None, "nothing to wait for");
        // A still is resized on a worker: draw and take the answer until the picture is there.
        let started = Instant::now();
        while !has_rgb(&drawn(&mut s), RED) {
            s.poll_at(t0);
            assert!(started.elapsed() < TMO, "the still never appeared");
            std::thread::sleep(ms(2));
        }
    }

    #[test]
    fn a_decoded_animation_with_one_frame_or_none_is_handled_like_a_still_or_a_failure() {
        // `PictureView::Gif` straight from the decoder with 1 frame -> a still; 0 -> failed.
        let dir = unique_tmp("still_gif_len");
        let f = gif_file(&dir, "a.gif", 40, &[(RED, 100), (BLUE, 100)]);
        let mut s = new_still(false, &f);
        s.done_tx
            .send(Done {
                gen: 0,
                view: Some(PictureView::Gif(vec![(
                    image::DynamicImage::new_rgba8(8, 8),
                    ms(10),
                )])),
            })
            .unwrap();
        s.poll_at(Instant::now());
        assert!(matches!(s.state, State::Ready { .. }));
        s.done_tx
            .send(Done {
                gen: 0,
                view: Some(PictureView::Gif(Vec::new())),
            })
            .unwrap();
        s.poll_at(Instant::now());
        assert!(matches!(s.state, State::Failed(None)));
    }

    // ---- sharp zoom of a vector picture ----

    const SVG_2X1: &[u8] = b"<svg xmlns='http://www.w3.org/2000/svg' width='200' height='100' \
        viewBox='0 0 200 100'><rect width='200' height='100' fill='#c33'/></svg>";
    const SVG_1X1: &[u8] = b"<svg xmlns='http://www.w3.org/2000/svg' width='100' height='100' \
        viewBox='0 0 100 100'><rect width='100' height='100' fill='#36c'/></svg>";

    /// Counts the drawings the app asks of its SVG drawer.
    struct CountingSvg(Arc<std::sync::atomic::AtomicUsize>);

    impl SvgDraw for CountingSvg {
        fn untrusted(
            &self,
            data: &[u8],
            base: &Path,
            roots: Option<&[PathBuf]>,
            max_px: u32,
            cancelled: &dyn Fn() -> bool,
        ) -> Result<DynamicImage, konoma_svg::svg::RasterError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            InProcessSvg.untrusted(data, base, roots, max_px, cancelled)
        }
        fn embedded(
            &self,
            data: &[u8],
            max_px: u32,
            cancelled: &dyn Fn() -> bool,
        ) -> Result<DynamicImage, konoma_svg::svg::RasterError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            InProcessSvg.embedded(data, max_px, cancelled)
        }
    }

    fn counting_still() -> (Still, Arc<std::sync::atomic::AtomicUsize>) {
        let n = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let s = Still::new(
            &Picker::halfblocks(),
            false,
            false,
            "t",
            Path::new("/x/a.svg"),
            800,
            Arc::new(CountingSvg(Arc::clone(&n))),
        );
        (s, n)
    }

    fn svg_view(data: &[u8], sharp: bool) -> ImageView {
        let mut v = iv(svg(data, None, false));
        v.sharp_zoom = sharp;
        v
    }

    fn land_view(s: &mut Still, v: &ImageView, keep_old: bool) {
        s.submit(v, keep_old);
        let started = Instant::now();
        while s.loading() {
            s.poll_at(Instant::now());
            assert!(started.elapsed() < TMO, "the picture never landed");
            std::thread::sleep(ms(2));
        }
    }

    /// Poll until no sharp redraw is out.
    fn settle(s: &mut Still) {
        let started = Instant::now();
        while s.sharp_inflight {
            s.poll_at(Instant::now());
            assert!(started.elapsed() < TMO, "the redraw never came back");
            std::thread::sleep(ms(2));
        }
    }

    fn raster(s: &Still) -> (u32, u32) {
        match &s.state {
            State::Ready { img, .. } => (img.width(), img.height()),
            _ => panic!("not ready"),
        }
    }

    fn logical(s: &Still) -> Option<(u32, u32)> {
        match &s.state {
            State::Ready { logical, .. } => *logical,
            _ => panic!("not ready"),
        }
    }

    #[test]
    fn zooming_a_sharp_vector_draws_it_again_bigger_and_the_layout_stays() {
        let (mut s, n) = counting_still();
        land_view(&mut s, &svg_view(SVG_2X1, true), false);
        assert_eq!(raster(&s), (800, 400));
        assert_eq!(logical(&s), Some((800, 400)));
        assert_eq!(n.load(Ordering::SeqCst), 1);
        // One step (1.25): want 1000 > 900.
        s.zoom_steps(1);
        assert!(s.sharp_inflight);
        settle(&mut s);
        assert_eq!(raster(&s), (1000, 500));
        assert_eq!(logical(&s), Some((800, 400)), "the layout size never moves");
        assert_eq!(n.load(Ordering::SeqCst), 2);
        // Zoom out again: the denser raster stays, nothing is drawn.
        s.zoom_steps(-1);
        assert!(!s.sharp_inflight);
        assert_eq!(raster(&s), (1000, 500));
        // The drawn picture still lays out in the logical size.
        let buf = drawn(&mut s);
        assert!(!colors(&buf).is_empty());
        assert_eq!(logical(&s), Some((800, 400)));
    }

    #[test]
    fn a_zoom_inside_the_margin_draws_nothing() {
        let (mut s, n) = counting_still();
        land_view(&mut s, &svg_view(SVG_2X1, true), false);
        // 1.0 -> 1.25 is over the margin; pose at 1.125 is not reachable by steps, so set it.
        s.pose.zoom = 1.125;
        s.maybe_sharpen();
        assert!(!s.sharp_inflight);
        assert_eq!(n.load(Ordering::SeqCst), 1);
        s.pose.zoom = 1.1251;
        s.maybe_sharpen();
        assert!(s.sharp_inflight, "just over the margin asks");
        settle(&mut s);
    }

    #[test]
    fn a_vector_that_does_not_ask_for_sharp_zoom_is_never_drawn_again() {
        let (mut s, n) = counting_still();
        land_view(&mut s, &svg_view(SVG_2X1, false), false);
        for _ in 0..4 {
            s.zoom_steps(1);
        }
        assert!(!s.sharp_inflight);
        assert!(s.vector.is_none());
        std::thread::sleep(ms(50));
        s.poll_at(Instant::now());
        assert_eq!(raster(&s), (800, 400));
        assert_eq!(n.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_still_picture_is_never_drawn_again() {
        let dir = unique_tmp("still_sharp_png");
        let f = png_file(&dir, "a.png");
        let mut s = new_still(false, &f);
        land(&mut s, &f, false, Instant::now());
        for _ in 0..4 {
            s.zoom_steps(1);
        }
        assert!(!s.sharp_inflight);
        assert_eq!(raster(&s), (60, 60));
    }

    #[test]
    fn repeated_zoom_keeps_one_redraw_out_and_converges_on_the_last_zoom() {
        let (mut s, n) = counting_still();
        land_view(&mut s, &svg_view(SVG_2X1, true), false);
        for _ in 0..4 {
            s.zoom_steps(1);
        }
        // Nothing was polled meanwhile: only the first redraw can have been started.
        settle(&mut s);
        let zoom = s.zoom();
        let want = (800.0 * zoom).ceil() as u32;
        // (The drawer rounds the scale up, so it may be one pixel over.)
        assert!(
            raster(&s).0.abs_diff(want) <= 1,
            "converged on the zoom of the moment it landed: {} vs {want}",
            raster(&s).0
        );
        // The first drawing, the redraw for the first step, the one that converged. Not one per key.
        assert_eq!(n.load(Ordering::SeqCst), 3);
        assert_eq!(logical(&s), Some((800, 400)));
    }

    #[test]
    fn a_redraw_never_goes_past_the_cap() {
        let (mut s, _n) = counting_still();
        land_view(&mut s, &svg_view(SVG_2X1, true), false);
        for _ in 0..30 {
            s.zoom_steps(1);
        }
        settle(&mut s);
        assert!(raster(&s).0 <= 4096, "{:?}", raster(&s));
        assert!(!s.sharp_inflight);
        // At the cap nothing more is asked.
        s.zoom_steps(1);
        if raster(&s).0 >= 4096 {
            assert!(!s.sharp_inflight);
        }
        settle(&mut s);
    }

    #[test]
    fn a_new_picture_drops_the_redraw_of_the_old_one() {
        let (mut s, _n) = counting_still();
        land_view(&mut s, &svg_view(SVG_2X1, true), false);
        s.zoom_steps(1);
        assert!(s.sharp_inflight);
        // Another picture before the redraw is polled.
        land_view(&mut s, &svg_view(SVG_1X1, true), false);
        std::thread::sleep(ms(100));
        s.poll_at(Instant::now());
        assert_eq!(
            raster(&s),
            (800, 800),
            "the old redraw did not replace the new picture"
        );
        assert_eq!(logical(&s), Some((800, 800)));
        assert!(!s.sharp_inflight);
        assert_eq!(s.zoom(), 1.0, "a new picture starts at fit");
    }

    #[test]
    fn an_answer_of_an_older_generation_is_not_applied() {
        let (mut s, _n) = counting_still();
        land_view(&mut s, &svg_view(SVG_1X1, true), false);
        s.pose.zoom = 2.0;
        s.maybe_sharpen();
        assert!(s.sharp_inflight);
        // A stale answer (an older picture's) arrives while the current redraw is out.
        s.sharp_tx
            .send(SharpDone {
                gen: s.gen - 1,
                img: Some(DynamicImage::new_rgba8(3000, 3000)),
            })
            .unwrap();
        s.poll_at(Instant::now());
        // (The real answer may or may not have been polled as well; the stale one never lands.)
        assert_ne!(raster(&s), (3000, 3000));
        settle(&mut s);
        assert_eq!(raster(&s), (1600, 1600));
    }

    /// Draws the first picture, then fails every drawing.
    struct FailAfterFirst(Arc<std::sync::atomic::AtomicUsize>);

    impl SvgDraw for FailAfterFirst {
        fn untrusted(
            &self,
            data: &[u8],
            base: &Path,
            roots: Option<&[PathBuf]>,
            max_px: u32,
            cancelled: &dyn Fn() -> bool,
        ) -> Result<DynamicImage, konoma_svg::svg::RasterError> {
            InProcessSvg.untrusted(data, base, roots, max_px, cancelled)
        }
        fn embedded(
            &self,
            data: &[u8],
            max_px: u32,
            cancelled: &dyn Fn() -> bool,
        ) -> Result<DynamicImage, konoma_svg::svg::RasterError> {
            if self.0.fetch_add(1, Ordering::SeqCst) > 0 {
                return Err(konoma_svg::svg::RasterError::Cancelled);
            }
            InProcessSvg.embedded(data, max_px, cancelled)
        }
    }

    #[test]
    fn a_redraw_that_fails_leaves_the_raster_and_is_not_retried_by_itself() {
        let n = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut s = Still::new(
            &Picker::halfblocks(),
            false,
            false,
            "t",
            Path::new("/x/a.svg"),
            800,
            Arc::new(FailAfterFirst(Arc::clone(&n))),
        );
        land_view(&mut s, &svg_view(SVG_2X1, true), false);
        s.zoom_steps(1);
        assert!(s.sharp_inflight);
        settle(&mut s);
        assert_eq!(raster(&s), (800, 400), "the failed redraw swapped nothing");
        assert_eq!(logical(&s), Some((800, 400)));
        std::thread::sleep(ms(50));
        s.poll_at(Instant::now());
        assert_eq!(n.load(Ordering::SeqCst), 2, "no retry by itself");
        // The next zoom asks again.
        s.zoom_steps(1);
        settle(&mut s);
        assert_eq!(n.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn a_trusted_svg_is_drawn_again_in_process_not_by_the_drawer() {
        let (mut s, n) = counting_still();
        let mut v = iv(svg(SVG_2X1, Some("/x"), true));
        v.sharp_zoom = true;
        land_view(&mut s, &v, false);
        assert_eq!(
            n.load(Ordering::SeqCst),
            0,
            "a trusted drawing is konoma's own"
        );
        assert!(matches!(
            s.vector.as_ref().map(|v| &v.trust),
            Some(SvgTrust::Trusted(_))
        ));
        s.zoom_steps(1);
        settle(&mut s);
        assert_eq!(n.load(Ordering::SeqCst), 0);
        assert_eq!(raster(&s), (1000, 500));
    }

    #[test]
    fn an_svg_with_a_folder_is_drawn_again_by_the_drawer_the_way_it_was_the_first_time() {
        let (mut s, n) = counting_still();
        let dir = unique_tmp("still_sharp_dir");
        let mut v = iv(svg(SVG_2X1, Some(dir.as_path().to_str().unwrap()), false));
        v.sharp_zoom = true;
        land_view(&mut s, &v, false);
        assert!(matches!(
            s.vector.as_ref().map(|v| &v.trust),
            Some(SvgTrust::File(_))
        ));
        assert_eq!(n.load(Ordering::SeqCst), 1);
        s.zoom_steps(1);
        settle(&mut s);
        assert_eq!(n.load(Ordering::SeqCst), 2);
        assert_eq!(raster(&s), (1000, 500));
    }

    #[test]
    fn a_page_turn_keeps_the_old_picture_and_does_not_redraw_it() {
        let (mut s, n) = counting_still();
        land_view(&mut s, &svg_view(SVG_2X1, true), false);
        // The next page: submitted, not landed (the old one is held).
        s.submit(&svg_view(SVG_1X1, true), true);
        assert!(s.held);
        s.zoom_steps(1);
        assert!(!s.sharp_inflight, "the held picture is not redrawn");
        let started = Instant::now();
        while s.loading() {
            s.poll_at(Instant::now());
            assert!(started.elapsed() < TMO);
            std::thread::sleep(ms(2));
        }
        settle(&mut s);
        // The pose was kept: the page that landed converges on it.
        assert_eq!(logical(&s), Some((800, 800)));
        assert_eq!(raster(&s), (1000, 1000));
        assert!(n.load(Ordering::SeqCst) >= 3);
    }
}
