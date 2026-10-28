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
    /// The picture in `state` is the one of the page before: the new page is being decoded and the
    /// old picture stays on screen, at fit and still zoomable, until it lands (as the host keeps
    /// its old page).
    held: bool,
    pose: ImageViewState,
    viewer: ImageViewer,
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
            held: false,
            pose: ImageViewState::default(),
            viewer: ImageViewer::new(),
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
        if keep_old && matches!(self.state, State::Ready { .. }) {
            // The page before stays on screen in the pose it has (the loop has already put it back
            // to fit, [`Still::turn_page`], and the keys move it from there as on the host).
            self.held = true;
        } else {
            self.pose.reset();
            self.held = false;
            self.state = State::Loading;
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
        self.pose.reset();
        self.viewer.clear();
        self.drawn_geometry = None;
        self.resize_asked = None;
    }

    fn cancel_job(&mut self) {
        if let Some(c) = self.cancel.take() {
            c.store(true, Ordering::Relaxed);
        }
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
            if self.held {
                // The new page replaces the one kept on screen: nothing of the old protocol is
                // reused (the pose is, as the host's: it is not touched when a picture lands).
                self.held = false;
                self.viewer.clear();
                self.drawn_geometry = None;
                self.resize_asked = None;
            }
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
        if self.loading() {
            self.spinner = self.spinner.wrapping_add(1);
            changed = true;
        }
        changed
    }

    /// How long the loop may wait before it has to look at the picture again.
    pub fn wake_in(&self) -> Option<Duration> {
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
}
