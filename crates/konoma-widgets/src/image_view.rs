//! A full-screen still-image view for any ratatui app: zoom, pan, and drawing the visible part of
//! a decoded image through the protocol the terminal speaks (konoma's own compressed kitty
//! placement, or ratatui-image's sixel / iTerm2 / halfblocks).
//!
//! Two pieces of state, because they live at different levels in an app:
//!
//! - [`ImageViewState`]: the *pose* (zoom and the center of the visible window). Small and `Copy`,
//!   so an app keeps one per tab and restores it with the tab.
//! - [`ImageViewer`]: the *drawing state* (the protocol built for the current crop, the kitty
//!   build bookkeeping). One per app; it is thrown away with the image.
//!
//! Everything the app owns is passed in: the decoded image ([`ImageSource`]), the terminal's
//! `Picker`, the worker channels and the filter ([`ImageBackend`]). The viewer never reads
//! configuration and never knows a language.

use std::sync::mpsc::Sender;
use std::sync::Arc;

use image::DynamicImage;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::widgets::StatefulWidget;
use ratatui_image::picker::Picker;
use ratatui_image::thread::{ResizeRequest, ResizeResponse, ThreadProtocol};
use ratatui_image::{FilterType, Resize, StatefulImage};
use tokio::sync::mpsc::UnboundedSender;

use crate::kitty::{self, KittyImage};
use crate::layout::{image_layout, ImageLayout, PxRect};

/// The smallest zoom: the whole image fits.
pub const ZOOM_MIN: f64 = 1.0;
/// The largest zoom.
pub const ZOOM_MAX: f64 = 16.0;
/// One pan step, as a fraction of the visible window.
pub const PAN_STEP: f64 = 0.25;

/// The geometry a kitty image targets: `(crop rect (x,y,w,h) in source px, display cols, rows)`.
pub type KittyGeom = (PxRect, u16, u16);

/// Zoom and pan of one image: what an app keeps per tab.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ImageViewState {
    /// 1.0 = the whole image fits; larger zooms in (an overflowing image is clipped and can pan).
    pub zoom: f64,
    /// The center of the visible window in the image, each axis in `[0, 1]`. The drawing clamps it
    /// so the window stays inside the image.
    pub center: (f64, f64),
}

impl Default for ImageViewState {
    fn default() -> Self {
        Self {
            zoom: 1.0,
            center: (0.5, 0.5),
        }
    }
}

impl ImageViewState {
    /// Multiply the zoom by `factor`, clamped to [`ZOOM_MIN`]..=[`ZOOM_MAX`]. The crop is applied
    /// when the view is prepared for drawing.
    pub fn zoom_by(&mut self, factor: f64) {
        self.zoom = (self.zoom * factor).clamp(ZOOM_MIN, ZOOM_MAX);
    }

    /// Back to fit and centered.
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Move the center by one step in direction `(dx, dy)` (each -1, 0 or +1). `vis_frac` is the
    /// fraction of the image visible on each axis ([`ImageViewer::vis_frac`]): a step is a quarter
    /// of the visible window, and an axis that is not clipped (fraction 1) is clamped back by the
    /// next [`ImageViewer::prepare`].
    pub fn pan(&mut self, dx: f64, dy: f64, vis_frac: (f64, f64)) {
        self.center.0 += dx * PAN_STEP * vis_frac.0;
        self.center.1 += dy * PAN_STEP * vis_frac.1;
    }
}

/// The decoded image to draw.
#[derive(Clone, Copy)]
pub struct ImageSource<'a> {
    pub image: &'a Arc<DynamicImage>,
    /// For a picture rasterized from vector art: the size of its first raster. Layout then works in
    /// this size, so swapping in a sharper raster never moves the picture or its crop window.
    pub logical: Option<(u32, u32)>,
}

/// What the host provides for drawing: the terminal's `Picker`, which protocol to use, the worker
/// channels and the resize filter.
#[derive(Clone, Copy)]
pub struct ImageBackend<'a> {
    pub picker: &'a Picker,
    /// Draw with konoma's own compressed kitty transfer instead of ratatui-image's protocol.
    pub use_kitty: bool,
    /// The terminal is behind tmux, so kitty escapes need its passthrough wrapping.
    pub is_tmux: bool,
    /// Where ratatui-image's resize jobs go (the non-kitty path). `None` = nothing can be drawn.
    pub resize_tx: Option<&'a UnboundedSender<ResizeRequest>>,
    /// Where finished kitty builds are sent. `None` = every kitty build runs on the calling thread.
    pub kitty_tx: Option<&'a Sender<KittyResult>>,
    /// `ImageLayout` render scale (`[ui] image_render_scale`): 1.0 = full size.
    pub render_scale: f64,
    /// The filter ratatui-image resizes with.
    pub filter: FilterType,
}

/// Result of an async kitty build (resize + `o=z` compress) from a worker thread. Matched by
/// `gen` (latest-wins): a build superseded by a newer zoom/pan is discarded. `None` = failed.
pub struct KittyResult {
    pub gen: u64,
    pub image: Option<KittyImage>,
}

/// The drawing state of the image on screen.
pub struct ImageViewer {
    /// The ratatui-image protocol of the current crop (sixel / iTerm2 / halfblocks path).
    protocol: Option<ThreadProtocol>,
    /// The compressed kitty image for the current crop (kitty path).
    kitty_image: Option<KittyImage>,
    /// Latest-wins generation for async kitty builds: only the newest spawn's result is applied.
    kitty_gen: u64,
    /// The geometry the current kitty image (or the build in flight) targets. Prevents spawning
    /// the same build every frame while the geometry is unchanged.
    kitty_want: Option<KittyGeom>,
    /// The geometry the **shown** kitty image was built for. A build is in flight exactly when
    /// this differs from `kitty_want` (this covers pan, where the cell size is unchanged).
    kitty_shown: Option<KittyGeom>,
    /// The crop (source px) the protocol was last built for. `None` = build on the next prepare.
    crop: Option<PxRect>,
    /// The visible fraction of the image per axis. Below 1 = clipped = pan is possible.
    vis_frac: (f64, f64),
    /// Which path the last [`prepare`](Self::prepare) took (the backend's `use_kitty`). Drawing
    /// and [`kitty_build_pending`](Self::kitty_build_pending) follow it, so the backend is the one
    /// place that decides and a draw can never disagree with the build it draws.
    use_kitty: bool,
    /// The resize filter of the last prepare (the backend's `filter`), used when drawing.
    filter: FilterType,
}

impl Default for ImageViewer {
    fn default() -> Self {
        Self {
            protocol: None,
            kitty_image: None,
            kitty_gen: 0,
            kitty_want: None,
            kitty_shown: None,
            crop: None,
            vis_frac: (1.0, 1.0),
            use_kitty: false,
            filter: FilterType::Nearest,
        }
    }
}

impl ImageViewer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Forget everything about the current image: the protocols, the pending kitty builds (the
    /// generation moves on so a build still in flight is discarded) and the crop.
    pub fn clear(&mut self) {
        self.protocol = None;
        self.kitty_image = None;
        self.invalidate();
        self.crop = None;
        self.vis_frac = (1.0, 1.0);
    }

    /// The image pixels were replaced (a new PDF page, a sharper raster) without the geometry
    /// changing: the next prepare must rebuild for sure, and a kitty build in flight is stale.
    pub fn invalidate(&mut self) {
        self.kitty_gen = self.kitty_gen.wrapping_add(1);
        self.kitty_want = None;
        self.kitty_shown = None;
    }

    /// The crop the protocol was last built for.
    pub fn crop(&self) -> Option<PxRect> {
        self.crop
    }

    /// Set the crop; `None` makes the next [`prepare`](Self::prepare) rebuild the protocol.
    pub fn set_crop(&mut self, crop: Option<PxRect>) {
        self.crop = crop;
    }

    /// Record the crop and visible fraction of a frame the app laid out itself (an animation
    /// frame goes through [`image_layout`] directly), so panning and the next prepare see them.
    pub fn set_layout(&mut self, crop: PxRect, vis_frac: (f64, f64)) {
        self.crop = Some(crop);
        self.vis_frac = vis_frac;
    }

    /// The visible fraction of the image per axis, as of the last prepare.
    pub fn vis_frac(&self) -> (f64, f64) {
        self.vis_frac
    }

    /// The generation of the newest kitty build (a [`KittyResult`] with another one is stale).
    pub fn kitty_gen(&self) -> u64 {
        self.kitty_gen
    }

    /// The ratatui-image protocol, once the first prepare built it.
    pub fn protocol(&self) -> Option<&ThreadProtocol> {
        self.protocol.as_ref()
    }

    /// The compressed kitty image for the current crop, if built.
    pub fn kitty_image(&self) -> Option<&KittyImage> {
        self.kitty_image.as_ref()
    }

    /// Pan by one step from this viewer's last visible fraction. See [`ImageViewState::pan`].
    pub fn pan(&self, state: &mut ImageViewState, dx: f64, dy: f64) {
        state.pan(dx, dy, self.vis_frac);
    }

    /// Whether an async kitty build is in flight. The run loop polls faster while this is true so
    /// the result is picked up promptly.
    pub fn kitty_build_pending(&self) -> bool {
        self.use_kitty && self.kitty_want != self.kitty_shown
    }

    /// Apply a resize result from the worker. Returns true if it was applied.
    pub fn apply_resize(&mut self, resp: ResizeResponse) -> bool {
        match self.protocol.as_mut() {
            Some(p) => p.update_resized_protocol(resp),
            None => false,
        }
    }

    /// Apply a finished async kitty build. A stale result (superseded by a newer zoom/pan) and a
    /// failed build are dropped, keeping the image that is shown rather than blanking it.
    pub fn apply_kitty(&mut self, result: KittyResult) -> bool {
        if result.gen != self.kitty_gen {
            return false; // superseded by a newer geometry
        }
        // Either way the newest generation has settled: mark the wanted geometry as shown so
        // `kitty_build_pending` clears. A failed build would otherwise leave it true forever and
        // spin the run loop.
        self.kitty_shown = self.kitty_want;
        match result.image {
            Some(ki) => {
                self.kitty_image = Some(ki);
                true
            }
            None => false,
        }
    }

    /// Call right before drawing. From the pose (zoom, center) and the area `inner`, work out
    /// where the picture goes (centered; fit at zoom 1, larger when zoomed, clipped once it
    /// exceeds the area) and which part of the source is visible; clamp the pose's center to what
    /// that window allows; start the protocol build when the geometry changed. Returns the
    /// rectangle to draw into, or `None` when there is nothing to draw (empty area, or the
    /// non-kitty path has no resize channel).
    pub fn prepare(
        &mut self,
        state: &mut ImageViewState,
        source: ImageSource<'_>,
        backend: &ImageBackend<'_>,
        inner: Rect,
    ) -> Option<Rect> {
        let src = source.image;
        self.use_kitty = backend.use_kitty;
        self.filter = backend.filter;
        let font = backend.picker.font_size();
        let ImageLayout {
            target,
            crop_rect,
            center,
            frac,
        } = image_layout(
            src,
            font,
            state.zoom,
            state.center,
            inner,
            backend.render_scale,
            source.logical,
        )?;
        let crop_changed = self.crop != Some(crop_rect);
        if backend.use_kitty {
            // Request a rebuild only when the wanted geometry (the crop, or the display cell size
            // after a terminal resize at fit) changes; otherwise the current or in-flight image
            // already targets it, and a static image just re-emits cheap placeholders every frame.
            let want = (crop_rect, target.width, target.height);
            if self.kitty_want != Some(want) {
                self.kitty_want = Some(want);
                self.request_kitty_build(src, crop_rect, target, font, backend);
            }
        } else {
            // sixel / iTerm2 / halfblocks: rebuild the protocol only on a crop change, to avoid a
            // re-encode every frame.
            let new_tp = if crop_changed {
                let (x0, y0, cw, ch) = crop_rect;
                let crop = src.crop_imm(x0, y0, cw, ch);
                let proto = backend.picker.new_resize_protocol(crop);
                Some(ThreadProtocol::new(backend.resize_tx?.clone(), Some(proto)))
            } else {
                None
            };
            if let Some(tp) = new_tp {
                self.protocol = Some(tp);
            }
        }
        state.center = center;
        self.vis_frac = frac;
        self.crop = Some(crop_rect);
        Some(target)
    }

    /// Build the compressed kitty image for the given crop and target. The **first** build (or
    /// one after a failure left no image) runs on the calling thread so the picture appears
    /// without a blank frame. Later builds (zoom/pan/resize) run on a worker thread so rapid input
    /// does not hitch; the previous image keeps showing until the new one arrives
    /// ([`apply_kitty`](Self::apply_kitty)). Without a `kitty_tx` everything runs synchronously.
    fn request_kitty_build(
        &mut self,
        src: &Arc<DynamicImage>,
        crop_rect: PxRect,
        target: Rect,
        font: ratatui_image::FontSize,
        backend: &ImageBackend<'_>,
    ) {
        let (cols, rows) = (target.width, target.height);
        let is_tmux = backend.is_tmux;
        // Every build moves the generation on so a stale async result is discarded.
        self.kitty_gen = self.kitty_gen.wrapping_add(1);

        let Some(tx) = backend.kitty_tx.filter(|_| self.kitty_image.is_some()) else {
            self.kitty_image = kitty::build_from_source(src, crop_rect, cols, rows, font, is_tmux);
            self.kitty_shown = self.kitty_want; // now showing the wanted geometry
            return;
        };

        let tx = tx.clone();
        let src = Arc::clone(src);
        let gen = self.kitty_gen;
        std::thread::spawn(move || {
            // A pathological image must not kill the worker (nothing would be sent and the
            // previous image would linger): a panic is reported as a failed build.
            let image = konoma_base::panic::catch_silent(move || {
                kitty::build_from_source(&src, crop_rect, cols, rows, font, is_tmux)
            })
            .flatten();
            let _ = tx.send(KittyResult { gen, image });
        });
    }

    /// Draw the picture into `target` (the rectangle [`prepare`](Self::prepare) returned), through
    /// the path and filter of the backend that prepare was given. Returns false when nothing could
    /// be drawn (no image built yet); the caller then shows its fallback.
    pub fn render(&mut self, target: Rect, buf: &mut Buffer) -> bool {
        let filter = self.filter;
        if self.use_kitty {
            // One transfer on a crop change, placeholders only afterwards.
            return match &self.kitty_image {
                Some(ki) => {
                    ki.render(target, buf);
                    true
                }
                None => false,
            };
        }
        match self.protocol.as_mut() {
            Some(state) => {
                // Nearest-neighbor aliases when shrinking and blocks when enlarging; the resize
                // runs on the host's worker thread, so a better filter does not block the UI.
                StatefulImage::new()
                    .resize(Resize::Scale(Some(filter)))
                    .render(target, buf, state);
                true
            }
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgba, RgbaImage};
    use ratatui::style::Color;
    use std::sync::mpsc::channel;
    use tokio::sync::mpsc::unbounded_channel;

    /// A solid-color picture of `w` x `h` pixels.
    fn solid(w: u32, h: u32, rgb: [u8; 3]) -> Arc<DynamicImage> {
        Arc::new(DynamicImage::ImageRgba8(RgbaImage::from_pixel(
            w,
            h,
            Rgba([rgb[0], rgb[1], rgb[2], 255]),
        )))
    }

    fn area(w: u16, h: u16) -> Rect {
        Rect::new(0, 0, w, h)
    }

    /// The halfblocks picker has the fixed 10x20 font, so layouts are deterministic.
    fn picker() -> Picker {
        Picker::halfblocks()
    }

    struct Rig {
        picker: Picker,
        resize_tx: Option<UnboundedSender<ResizeRequest>>,
        kitty_tx: Option<Sender<KittyResult>>,
        use_kitty: bool,
    }

    impl Rig {
        fn halfblocks() -> (Self, tokio::sync::mpsc::UnboundedReceiver<ResizeRequest>) {
            let (tx, rx) = unbounded_channel();
            (
                Self {
                    picker: picker(),
                    resize_tx: Some(tx),
                    kitty_tx: None,
                    use_kitty: false,
                },
                rx,
            )
        }
        fn kitty(kitty_tx: Option<Sender<KittyResult>>) -> Self {
            Self {
                picker: picker(),
                resize_tx: None,
                kitty_tx,
                use_kitty: true,
            }
        }
        fn backend(&self) -> ImageBackend<'_> {
            ImageBackend {
                picker: &self.picker,
                use_kitty: self.use_kitty,
                is_tmux: false,
                resize_tx: self.resize_tx.as_ref(),
                kitty_tx: self.kitty_tx.as_ref(),
                render_scale: 1.0,
                filter: FilterType::Nearest,
            }
        }
    }

    fn src(image: &Arc<DynamicImage>) -> ImageSource<'_> {
        ImageSource {
            image,
            logical: None,
        }
    }

    // --- ImageViewState: pure zoom / pan arithmetic -------------------------------------

    #[test]
    fn default_state_is_fit_and_centered() {
        let s = ImageViewState::default();
        assert_eq!(s.zoom, 1.0);
        assert_eq!(s.center, (0.5, 0.5));
    }

    #[test]
    fn zoom_by_multiplies_and_clamps_to_the_range() {
        let mut s = ImageViewState::default();
        s.zoom_by(2.0);
        assert_eq!(s.zoom, 2.0);
        s.zoom_by(1.25);
        assert!((s.zoom - 2.5).abs() < 1e-12);
        // Zooming out never goes below fit.
        s.zoom_by(0.01);
        assert_eq!(s.zoom, ZOOM_MIN);
        s.zoom_by(0.5);
        assert_eq!(s.zoom, ZOOM_MIN);
        // Zooming in stops at the cap, however large the factor.
        s.zoom_by(1e9);
        assert_eq!(s.zoom, ZOOM_MAX);
        // The documented range, spelled out (not through the constants).
        assert_eq!((ZOOM_MIN, ZOOM_MAX), (1.0, 16.0));
        assert_eq!(s.zoom, 16.0);
        s.zoom_by(2.0);
        assert_eq!(s.zoom, ZOOM_MAX);
        // A zero or negative factor cannot produce a zoom below fit either.
        s.zoom_by(0.0);
        assert_eq!(s.zoom, ZOOM_MIN);
        s.zoom_by(-3.0);
        assert_eq!(s.zoom, ZOOM_MIN);
    }

    #[test]
    fn reset_returns_to_fit_and_center() {
        let mut s = ImageViewState {
            zoom: 8.0,
            center: (0.1, 0.9),
        };
        s.reset();
        assert_eq!(s, ImageViewState::default());
    }

    #[test]
    fn pan_steps_a_quarter_of_the_visible_window_per_axis() {
        let mut s = ImageViewState::default();
        s.pan(1.0, 0.0, (0.5, 1.0));
        assert!((s.center.0 - (0.5 + 0.25 * 0.5)).abs() < 1e-12);
        assert_eq!(s.center.1, 0.5);
        s.pan(-1.0, 1.0, (0.5, 0.2));
        assert!((s.center.0 - 0.5).abs() < 1e-12);
        assert!((s.center.1 - (0.5 + 0.25 * 0.2)).abs() < 1e-12);
        // A direction of zero does not move the axis.
        let before = s.center;
        s.pan(0.0, 0.0, (0.3, 0.3));
        assert_eq!(s.center, before);
    }

    // --- prepare: the layout it drives and the clamping it applies ----------------------

    #[test]
    fn prepare_fits_a_small_image_at_natural_size_centered() {
        // 40x40 px at the 10x20 font = 4x2 cells, inside a 20x10 area: never enlarged.
        let img = solid(40, 40, [255, 0, 0]);
        let (rig, _rx) = Rig::halfblocks();
        let mut v = ImageViewer::new();
        let mut st = ImageViewState::default();
        let t = v
            .prepare(&mut st, src(&img), &rig.backend(), area(20, 10))
            .unwrap();
        assert_eq!((t.width, t.height), (4, 2));
        assert_eq!((t.x, t.y), (8, 4));
        assert_eq!(v.crop(), Some((0, 0, 40, 40)));
        assert_eq!(v.vis_frac(), (1.0, 1.0));
        assert_eq!(st.center, (0.5, 0.5));
    }

    #[test]
    fn prepare_shrinks_a_large_image_to_the_area() {
        let img = solid(4000, 4000, [0, 255, 0]);
        let (rig, _rx) = Rig::halfblocks();
        let mut v = ImageViewer::new();
        let mut st = ImageViewState::default();
        let t = v
            .prepare(&mut st, src(&img), &rig.backend(), area(30, 10))
            .unwrap();
        assert!(t.width <= 30 && t.height <= 10, "{t:?}");
        assert!(t.width >= 1 && t.height >= 1);
        // At fit the whole source is visible.
        assert_eq!(v.crop(), Some((0, 0, 4000, 4000)));
        assert_eq!(v.vis_frac(), (1.0, 1.0));
    }

    #[test]
    fn prepare_zoomed_in_crops_and_clamps_the_center_inside_the_image() {
        let img = solid(400, 200, [0, 0, 255]);
        let (rig, _rx) = Rig::halfblocks();
        let mut v = ImageViewer::new();
        let mut st = ImageViewState::default();
        st.zoom_by(4.0);
        // Far outside the image on both axes: the window is pulled back to its edge.
        st.center = (9.0, -9.0);
        let t = v
            .prepare(&mut st, src(&img), &rig.backend(), area(20, 10))
            .unwrap();
        let (fw, fh) = v.vis_frac();
        assert!(
            fw < 1.0 || fh < 1.0,
            "zoom 4 must clip something: {fw} {fh}"
        );
        assert!(
            (st.center.0 - (1.0 - fw / 2.0)).abs() < 1e-9,
            "{:?}",
            st.center
        );
        assert!((st.center.1 - fh / 2.0).abs() < 1e-9, "{:?}", st.center);
        let (x0, y0, cw, ch) = v.crop().unwrap();
        assert!(x0 + cw <= 400 && y0 + ch <= 200, "{x0} {y0} {cw} {ch}");
        assert!(cw >= 1 && ch >= 1);
        assert!(t.width <= 20 && t.height <= 10);
    }

    #[test]
    fn prepare_at_extreme_zoom_keeps_a_non_empty_crop_inside_the_image() {
        for (w, h) in [(1u32, 1u32), (3, 500), (500, 3), (64, 64)] {
            let img = solid(w, h, [10, 20, 30]);
            let (rig, _rx) = Rig::halfblocks();
            let mut v = ImageViewer::new();
            for center in [(0.0, 0.0), (1.0, 1.0), (0.5, 0.5), (7.0, -7.0)] {
                let mut st = ImageViewState::default();
                st.zoom_by(1e9);
                st.center = center;
                let t = v
                    .prepare(&mut st, src(&img), &rig.backend(), area(40, 20))
                    .unwrap();
                let (x0, y0, cw, ch) = v.crop().unwrap();
                assert!(cw >= 1 && ch >= 1, "{w}x{h} {center:?}");
                assert!(x0 + cw <= w && y0 + ch <= h, "{w}x{h} {center:?}");
                assert!(t.width >= 1 && t.height >= 1);
                assert!((0.0..=1.0).contains(&st.center.0));
                assert!((0.0..=1.0).contains(&st.center.1));
            }
        }
    }

    #[test]
    fn prepare_in_tiny_areas_never_exceeds_the_area_and_empty_areas_draw_nothing() {
        let img = solid(300, 300, [1, 2, 3]);
        let (rig, _rx) = Rig::halfblocks();
        let mut v = ImageViewer::new();
        for (w, h) in [(1u16, 1u16), (1, 8), (8, 1), (2, 2)] {
            for zoom in [1.0, 3.0, ZOOM_MAX] {
                let mut st = ImageViewState {
                    zoom,
                    center: (0.5, 0.5),
                };
                let a = Rect::new(3, 2, w, h);
                let t = v
                    .prepare(&mut st, src(&img), &rig.backend(), a)
                    .unwrap_or_else(|| panic!("{w}x{h} zoom {zoom}"));
                assert!(t.width >= 1 && t.height >= 1);
                assert!(t.x >= a.x && t.y >= a.y, "{t:?} in {a:?}");
                assert!(t.right() <= a.right() && t.bottom() <= a.bottom());
            }
        }
        let mut st = ImageViewState::default();
        assert!(v
            .prepare(&mut st, src(&img), &rig.backend(), area(0, 5))
            .is_none());
        assert!(v
            .prepare(&mut st, src(&img), &rig.backend(), area(5, 0))
            .is_none());
    }

    #[test]
    fn pan_after_zoom_moves_the_crop_and_stops_at_the_edge() {
        let img = solid(800, 800, [9, 9, 9]);
        let (rig, _rx) = Rig::halfblocks();
        let mut v = ImageViewer::new();
        let mut st = ImageViewState::default();
        st.zoom_by(4.0);
        v.prepare(&mut st, src(&img), &rig.backend(), area(20, 10));
        let first = v.crop().unwrap();
        v.pan(&mut st, 1.0, 0.0);
        v.prepare(&mut st, src(&img), &rig.backend(), area(20, 10));
        let second = v.crop().unwrap();
        assert!(second.0 > first.0, "panning right moves the window right");
        for _ in 0..50 {
            v.pan(&mut st, 1.0, 1.0);
            v.prepare(&mut st, src(&img), &rig.backend(), area(20, 10));
        }
        let (x0, y0, cw, ch) = v.crop().unwrap();
        assert_eq!((x0 + cw, y0 + ch), (800, 800), "pinned to the far corner");
        let center = st.center;
        v.pan(&mut st, 1.0, 1.0);
        v.prepare(&mut st, src(&img), &rig.backend(), area(20, 10));
        assert_eq!(st.center, center, "no movement past the edge");
    }

    #[test]
    fn a_vector_logical_size_keeps_the_layout_when_the_raster_is_swapped() {
        // The same drawing at two rasterization densities lays out identically.
        let (rig, _rx) = Rig::halfblocks();
        let small = solid(100, 50, [5, 5, 5]);
        let sharp = solid(400, 200, [5, 5, 5]);
        let mut v = ImageViewer::new();
        let mut st = ImageViewState::default();
        let a = v
            .prepare(
                &mut st,
                ImageSource {
                    image: &small,
                    logical: Some((100, 50)),
                },
                &rig.backend(),
                area(30, 12),
            )
            .unwrap();
        let b = v
            .prepare(
                &mut st,
                ImageSource {
                    image: &sharp,
                    logical: Some((100, 50)),
                },
                &rig.backend(),
                area(30, 12),
            )
            .unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn the_protocol_path_needs_a_resize_channel() {
        let img = solid(40, 40, [1, 1, 1]);
        let rig = Rig {
            picker: picker(),
            resize_tx: None,
            kitty_tx: None,
            use_kitty: false,
        };
        let mut v = ImageViewer::new();
        let mut st = ImageViewState::default();
        assert!(v
            .prepare(&mut st, src(&img), &rig.backend(), area(20, 10))
            .is_none());
        assert!(v.protocol().is_none());
        assert!(
            v.crop().is_none(),
            "nothing was recorded for a failed prepare"
        );
    }

    #[test]
    fn the_protocol_is_rebuilt_only_when_the_crop_changes() {
        let img = solid(800, 800, [1, 1, 1]);
        let (rig, _rx) = Rig::halfblocks();
        let mut v = ImageViewer::new();
        let mut st = ImageViewState::default();
        st.zoom_by(2.0);
        v.prepare(&mut st, src(&img), &rig.backend(), area(20, 10));
        let first = v.protocol().unwrap() as *const ThreadProtocol;
        v.prepare(&mut st, src(&img), &rig.backend(), area(20, 10));
        assert_eq!(
            v.protocol().unwrap() as *const _,
            first,
            "same crop, same protocol"
        );
        v.pan(&mut st, 1.0, 0.0);
        v.prepare(&mut st, src(&img), &rig.backend(), area(20, 10));
        // A new crop builds a new ThreadProtocol (the old one is dropped; the address may be
        // reused, so compare the recorded crop as well).
        assert!(v.crop().is_some());
        v.set_crop(None);
        assert!(v.crop().is_none());
        v.prepare(&mut st, src(&img), &rig.backend(), area(20, 10));
        assert!(
            v.crop().is_some(),
            "set_crop(None) makes the next prepare rebuild"
        );
    }

    // --- draw: halfblocks through a real buffer ------------------------------------------

    fn cell_colors(buf: &Buffer, r: Rect) -> Vec<(String, Color, Color)> {
        let mut out = Vec::new();
        for y in r.top()..r.bottom() {
            for x in r.left()..r.right() {
                let c = &buf[(x, y)];
                out.push((c.symbol().to_string(), c.fg, c.bg));
            }
        }
        out
    }

    #[test]
    fn halfblocks_draw_a_solid_picture_into_the_target_only() {
        let img = solid(40, 40, [255, 0, 0]);
        let (rig, mut rx) = Rig::halfblocks();
        let mut v = ImageViewer::new();
        let mut st = ImageViewState::default();
        let inner = area(20, 10);
        let target = v
            .prepare(&mut st, src(&img), &rig.backend(), inner)
            .unwrap();
        let mut buf = Buffer::empty(inner);

        // The first draw asks the worker to encode; nothing is on screen yet but it is not an
        // error: the protocol exists.
        assert!(v.render(target, &mut buf));
        // Serve the job exactly like the host's resize worker does.
        let req = rx.try_recv().expect("a resize job was requested");
        let resp = req.resize_encode().expect("encode");
        assert!(v.apply_resize(resp));
        assert!(v.render(target, &mut buf));

        for (sym, fg, bg) in cell_colors(&buf, target) {
            // A uniform cell may be written as a plain space on the background color.
            assert!(sym == "▀" || sym == " ", "{sym:?}");
            assert_eq!(fg, Color::Rgb(255, 0, 0));
            assert_eq!(bg, Color::Rgb(255, 0, 0));
        }
        // Everything outside the target stays untouched.
        for y in 0..inner.height {
            for x in 0..inner.width {
                let inside = x >= target.left()
                    && x < target.right()
                    && y >= target.top()
                    && y < target.bottom();
                if !inside {
                    assert_eq!(buf[(x, y)].bg, Color::Reset, "({x},{y})");
                    assert_eq!(buf[(x, y)].fg, Color::Reset, "({x},{y})");
                }
            }
        }
    }

    #[test]
    fn halfblocks_draw_through_a_test_terminal() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;
        let img = solid(40, 40, [0, 128, 255]);
        let (rig, mut rx) = Rig::halfblocks();
        let mut v = ImageViewer::new();
        let mut st = ImageViewState::default();
        let mut term = Terminal::new(TestBackend::new(20, 10)).unwrap();
        let mut frame = |v: &mut ImageViewer, st: &mut ImageViewState| {
            let mut target = None;
            term.draw(|f| {
                let t = v.prepare(st, src(&img), &rig.backend(), f.area());
                target = t;
                if let Some(t) = t {
                    v.render(t, f.buffer_mut());
                }
            })
            .unwrap();
            target.unwrap()
        };
        let target = frame(&mut v, &mut st);
        let resp = rx.try_recv().unwrap().resize_encode().unwrap();
        assert!(v.apply_resize(resp));
        let target2 = frame(&mut v, &mut st);
        assert_eq!(target, target2);
        let buf = term.backend().buffer();
        assert_eq!(buf[(target.x, target.y)].fg, Color::Rgb(0, 128, 255));
    }

    #[test]
    fn render_reports_nothing_drawn_before_anything_is_built() {
        let mut v = ImageViewer::new();
        let mut buf = Buffer::empty(area(4, 4));
        assert!(!v.render(area(4, 4), &mut buf));
    }

    #[test]
    fn the_prepared_backend_decides_the_drawing_path() {
        // One backend value picks the build and the draw: after a halfblocks prepare the viewer
        // draws the protocol and reports no kitty build pending, whatever else the host holds.
        let img = solid(40, 40, [9, 9, 9]);
        let (rig, _rx) = Rig::halfblocks();
        let mut v = ImageViewer::new();
        let mut st = ImageViewState::default();
        let t = v
            .prepare(&mut st, src(&img), &rig.backend(), area(20, 10))
            .unwrap();
        assert!(!v.kitty_build_pending());
        let mut buf = Buffer::empty(area(20, 10));
        assert!(v.render(t, &mut buf));
        assert!(v.kitty_image().is_none(), "the kitty path was not taken");
        // Prepared again through a kitty backend: now the kitty image is what is drawn.
        let krig = Rig::kitty(None);
        let t2 = v
            .prepare(&mut st, src(&img), &krig.backend(), area(20, 10))
            .unwrap();
        assert!(v.kitty_image().is_some());
        assert!(v.render(t2, &mut buf));
    }

    #[test]
    fn a_pending_kitty_build_is_not_reported_after_switching_to_a_non_kitty_backend() {
        let img = solid(400, 400, [1, 2, 3]);
        let (tx, _rx) = channel();
        let krig = Rig::kitty(Some(tx));
        let mut v = ImageViewer::new();
        let mut st = ImageViewState::default();
        v.prepare(&mut st, src(&img), &krig.backend(), area(20, 10))
            .unwrap();
        st.zoom_by(4.0);
        v.prepare(&mut st, src(&img), &krig.backend(), area(20, 10))
            .unwrap();
        assert!(
            v.kitty_build_pending(),
            "the zoom's async build is in flight"
        );
        let (hrig, _rx) = Rig::halfblocks();
        v.prepare(&mut st, src(&img), &hrig.backend(), area(20, 10))
            .unwrap();
        assert!(
            !v.kitty_build_pending(),
            "the backend now draws without kitty"
        );
    }

    #[test]
    fn apply_resize_without_a_protocol_is_not_applied() {
        let img = solid(40, 40, [1, 1, 1]);
        let (rig, mut rx) = Rig::halfblocks();
        let mut v = ImageViewer::new();
        let mut st = ImageViewState::default();
        let t = v
            .prepare(&mut st, src(&img), &rig.backend(), area(20, 10))
            .unwrap();
        let mut buf = Buffer::empty(area(20, 10));
        v.render(t, &mut buf);
        let resp = rx.try_recv().unwrap().resize_encode().unwrap();
        v.clear();
        assert!(v.protocol().is_none());
        assert!(!v.apply_resize(resp));
    }

    // --- the kitty path -------------------------------------------------------------------

    #[test]
    fn kitty_without_a_worker_builds_synchronously_and_draws_placeholders() {
        let img = solid(40, 40, [255, 0, 0]);
        let rig = Rig::kitty(None);
        let mut v = ImageViewer::new();
        let mut st = ImageViewState::default();
        let t = v
            .prepare(&mut st, src(&img), &rig.backend(), area(20, 10))
            .unwrap();
        assert!(v.kitty_image().is_some(), "built before prepare returned");
        assert!(!v.kitty_build_pending());
        let mut buf = Buffer::empty(area(20, 10));
        assert!(v.render(t, &mut buf));
        assert_eq!(v.kitty_image().unwrap().cell_size(), (t.width, t.height));
        // Same geometry again: no rebuild, so the generation does not move.
        let gen = v.kitty_gen();
        v.prepare(&mut st, src(&img), &rig.backend(), area(20, 10));
        assert_eq!(v.kitty_gen(), gen);
        // The non-kitty protocol was never touched.
        assert!(v.protocol().is_none());
    }

    #[test]
    fn kitty_first_build_is_synchronous_then_zoom_and_pan_go_async() {
        let img = solid(800, 800, [0, 255, 0]);
        let (tx, rx) = channel();
        let rig = Rig::kitty(Some(tx));
        let mut v = ImageViewer::new();
        let mut st = ImageViewState::default();
        v.prepare(&mut st, src(&img), &rig.backend(), area(20, 10));
        assert!(v.kitty_image().is_some(), "the first build is synchronous");
        assert!(!v.kitty_build_pending());
        let size_before = v.kitty_image().unwrap().cell_size();

        st.zoom_by(4.0);
        v.prepare(&mut st, src(&img), &rig.backend(), area(20, 10));
        assert!(v.kitty_build_pending(), "the rebuild is in flight");
        assert_eq!(
            v.kitty_image().unwrap().cell_size(),
            size_before,
            "the previous image keeps showing"
        );
        let res = rx.recv_timeout(std::time::Duration::from_secs(30)).unwrap();
        assert_eq!(res.gen, v.kitty_gen());
        assert!(v.apply_kitty(res));
        assert!(!v.kitty_build_pending());

        // Pan changes only the crop (same cell size): still a new build.
        v.pan(&mut st, 1.0, 0.0);
        v.prepare(&mut st, src(&img), &rig.backend(), area(20, 10));
        assert!(v.kitty_build_pending());
        let res = rx.recv_timeout(std::time::Duration::from_secs(30)).unwrap();
        assert!(v.apply_kitty(res));
        assert!(!v.kitty_build_pending());
    }

    #[test]
    fn a_stale_or_failed_kitty_build_is_dropped_without_blanking() {
        let img = solid(800, 800, [0, 0, 255]);
        let (tx, rx) = channel();
        let rig = Rig::kitty(Some(tx));
        let mut v = ImageViewer::new();
        let mut st = ImageViewState::default();
        v.prepare(&mut st, src(&img), &rig.backend(), area(20, 10));
        st.zoom_by(2.0);
        v.prepare(&mut st, src(&img), &rig.backend(), area(20, 10));
        let first = rx.recv_timeout(std::time::Duration::from_secs(30)).unwrap();
        st.zoom_by(2.0);
        v.prepare(&mut st, src(&img), &rig.backend(), area(20, 10));
        // The first result is now older than the newest request.
        assert!(!v.apply_kitty(first));
        assert!(v.kitty_build_pending(), "a stale result does not settle");
        let newest = rx.recv_timeout(std::time::Duration::from_secs(30)).unwrap();
        assert_eq!(newest.gen, v.kitty_gen());
        // A failed build settles the generation (no endless polling) but keeps the old image.
        let shown = v.kitty_image().unwrap().cell_size();
        assert!(!v.apply_kitty(KittyResult {
            gen: newest.gen,
            image: None
        }));
        assert!(!v.kitty_build_pending());
        assert_eq!(v.kitty_image().unwrap().cell_size(), shown);
    }

    #[test]
    fn invalidate_forces_a_rebuild_even_when_the_geometry_is_unchanged() {
        let img = solid(40, 40, [1, 1, 1]);
        let rig = Rig::kitty(None);
        let mut v = ImageViewer::new();
        let mut st = ImageViewState::default();
        v.prepare(&mut st, src(&img), &rig.backend(), area(20, 10));
        let gen = v.kitty_gen();
        v.invalidate();
        assert_ne!(v.kitty_gen(), gen, "an in-flight build becomes stale");
        v.prepare(&mut st, src(&img), &rig.backend(), area(20, 10));
        assert!(v.kitty_gen() != gen);
        assert!(v.kitty_image().is_some());
        assert!(!v.kitty_build_pending());
    }

    #[test]
    fn clear_forgets_images_crop_and_visible_fraction() {
        let img = solid(800, 800, [1, 1, 1]);
        let rig = Rig::kitty(None);
        let mut v = ImageViewer::new();
        let mut st = ImageViewState::default();
        st.zoom_by(4.0);
        v.prepare(&mut st, src(&img), &rig.backend(), area(20, 10));
        assert!(v.vis_frac().0 < 1.0 || v.vis_frac().1 < 1.0);
        let gen = v.kitty_gen();
        v.clear();
        assert!(v.kitty_image().is_none());
        assert!(v.crop().is_none());
        assert_eq!(v.vis_frac(), (1.0, 1.0));
        assert_ne!(v.kitty_gen(), gen);
        assert!(!v.kitty_build_pending());
    }

    #[test]
    fn set_layout_records_what_an_app_laid_out_itself() {
        let mut v = ImageViewer::new();
        v.set_layout((1, 2, 3, 4), (0.5, 0.25));
        assert_eq!(v.crop(), Some((1, 2, 3, 4)));
        assert_eq!(v.vis_frac(), (0.5, 0.25));
        let mut st = ImageViewState::default();
        v.pan(&mut st, 1.0, 1.0);
        assert!((st.center.0 - (0.5 + 0.25 * 0.5)).abs() < 1e-12);
        assert!((st.center.1 - (0.5 + 0.25 * 0.25)).abs() < 1e-12);
    }
}
