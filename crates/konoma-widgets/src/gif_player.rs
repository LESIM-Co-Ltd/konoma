//! A full-screen animated-GIF player for any ratatui app.
//!
//! An animation differs from a still image ([`crate::image_view`]) in that the frame changes on a
//! timer. Building a protocol asynchronously for every frame would show "an unencoded protocol ->
//! nothing" churn, so the current frame is **encoded synchronously** to the display size and swapped
//! into [`GifPlayer::protocol`] atomically. Zoom and pan are shared with the still-image view: the
//! player reads and clamps the same [`ImageViewState`] and records the crop and visible fraction in
//! the same [`ImageViewer`], so panning works the same way.
//!
//! The player never reads the clock itself: the app passes `now`, so tests can fix the time.

use std::time::{Duration, Instant};

use image::DynamicImage;
use ratatui::layout::{Rect, Size};
use ratatui_image::picker::Picker;
use ratatui_image::protocol::Protocol;
use ratatui_image::{FilterType, Resize};

use crate::image_view::{ImageViewState, ImageViewer};
use crate::layout::{image_layout, ImageLayout, PxRect};

/// The shortest wait the run loop is asked to sleep while a GIF plays.
pub const POLL_MIN: Duration = Duration::from_millis(10);
/// The longest wait (the same as a normal idle tick's upper bound).
pub const POLL_MAX: Duration = Duration::from_millis(100);

/// One decoded frame (composited RGBA) and how long it is shown.
pub type GifFrame = (DynamicImage, Duration);

/// All frames of one GIF plus the playback position and the encoded current frame.
#[derive(Default)]
pub struct GifPlayer {
    frames: Vec<GifFrame>,
    /// Index of the frame on screen.
    idx: usize,
    /// When the current frame began showing. `None` = before the first tick.
    shown_at: Option<Instant>,
    /// The current frame encoded to the display size.
    protocol: Option<Protocol>,
    /// The (frame index, crop) `protocol` was built from; re-encoded only when it changes.
    proto_key: Option<(usize, PxRect)>,
}

impl GifPlayer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Load all frames and restart from the first one. Timing starts on the first tick.
    pub fn set_frames(&mut self, frames: Vec<GifFrame>) {
        self.frames = frames;
        self.idx = 0;
        self.shown_at = None;
        self.protocol = None;
        self.proto_key = None;
    }

    /// Forget everything (a new file, another tab).
    pub fn clear(&mut self) {
        self.set_frames(Vec::new());
    }

    /// Whether an animation is loaded and plays (a one-frame GIF is a still image).
    pub fn is_active(&self) -> bool {
        self.frames.len() >= 2
    }

    /// Whether any frame is loaded (true for a one-frame GIF too, which is not [`is_active`](Self::is_active)).
    pub fn has_frames(&self) -> bool {
        !self.frames.is_empty()
    }

    /// Index of the frame on screen.
    pub fn frame_index(&self) -> usize {
        self.idx
    }

    /// The current frame encoded to the display size (drawn by the render side).
    pub fn protocol(&self) -> Option<&Protocol> {
        self.protocol.as_ref()
    }

    /// Test seam: jump to a frame without waiting for its time.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn force_frame_index(&mut self, idx: usize) {
        self.idx = idx;
    }

    /// Test seam: set when the current frame began showing.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn force_shown_at(&mut self, at: Option<Instant>) {
        self.shown_at = at;
    }

    /// Advance to the next frame if the current one's display time has elapsed at `now`. The
    /// re-encode happens in the next [`prepare`](Self::prepare), which sees the index change.
    /// Returns true when advanced (the caller redraws); always false when not animated.
    pub fn advance_if_due(&mut self, now: Instant) -> bool {
        if !self.is_active() {
            return false;
        }
        let Some(shown_at) = self.shown_at else {
            // First tick: only start timing (the first frame is already shown).
            self.shown_at = Some(now);
            return false;
        };
        let delay = self.frames[self.idx].1;
        if now.duration_since(shown_at) < delay {
            return false;
        }
        self.idx = (self.idx + 1) % self.frames.len();
        self.shown_at = Some(now);
        true
    }

    /// How long the run loop may wait for the next frame while playing; `None` when not playing.
    /// Clamped to [`POLL_MIN`]..=[`POLL_MAX`] for smoothness.
    pub fn poll_timeout(&self, now: Instant) -> Option<Duration> {
        if !self.is_active() {
            return None;
        }
        let remaining = match self.shown_at {
            // Not timing yet: tick right away to start timing.
            None => Duration::ZERO,
            Some(t) => self.frames[self.idx]
                .1
                .checked_sub(now.saturating_duration_since(t))
                .unwrap_or(Duration::ZERO),
        };
        Some(remaining.clamp(POLL_MIN, POLL_MAX))
    }

    /// Call right before drawing. From the current (frame, zoom, center) and the area `inner`, work
    /// out the display rectangle and the visible crop, and **synchronously encode** the frame when
    /// the frame index or the crop changed. The state's center is clamped and the crop and visible
    /// fraction are recorded in `viewer`. Returns the rectangle to draw into; `None` when there is
    /// no frame or nothing to draw.
    pub fn prepare(
        &mut self,
        state: &mut ImageViewState,
        viewer: &mut ImageViewer,
        picker: &Picker,
        render_scale: f64,
        inner: Rect,
    ) -> Option<Rect> {
        let src = &self.frames.get(self.idx)?.0;
        let ImageLayout {
            target,
            crop_rect,
            center,
            frac,
        } = image_layout(
            src,
            picker.font_size(),
            state.zoom,
            state.center,
            inner,
            render_scale,
            None, // a GIF stays at its raw raster size (it is not vector-derived)
        )?;
        let key = (self.idx, crop_rect);
        // Re-encode only when the frame or the crop (zoom / pan / resize) changes.
        let built = if self.proto_key != Some(key) {
            let (x0, y0, cw, ch) = crop_rect;
            let crop = src.crop_imm(x0, y0, cw, ch);
            let size = Size::new(target.width.max(1), target.height.max(1));
            // Encoded on the UI thread every frame, so a light, smooth filter: nearest-neighbor
            // would make each frame jagged, and Lanczos3 (used for stills) is too heavy here.
            Some(picker.new_protocol(crop, size, Resize::Scale(Some(FilterType::Triangle))))
        } else {
            None
        };
        if let Some(res) = built {
            match res {
                Ok(p) => {
                    self.protocol = Some(p);
                    self.proto_key = Some(key);
                }
                Err(_) => {
                    // Encode failure: the render side shows its fallback. Retry on the next frame.
                    self.protocol = None;
                    self.proto_key = None;
                }
            }
        }
        state.center = center;
        viewer.set_layout(crop_rect, frac);
        Some(target)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgba, RgbaImage};

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    fn frame(w: u32, h: u32, delay: Duration) -> GifFrame {
        (
            DynamicImage::ImageRgba8(RgbaImage::from_pixel(w, h, Rgba([10, 20, 30, 255]))),
            delay,
        )
    }

    /// `n` square frames of 40 px, each shown for `delay`.
    fn player(n: usize, delay: Duration) -> GifPlayer {
        let mut p = GifPlayer::new();
        p.set_frames((0..n).map(|_| frame(40, 40, delay)).collect());
        p
    }

    /// Like [`player`] but with big frames, so a zoom clips them in a small area.
    fn big_player(n: usize, delay: Duration) -> GifPlayer {
        let mut p = GifPlayer::new();
        p.set_frames((0..n).map(|_| frame(800, 800, delay)).collect());
        p
    }

    fn area(w: u16, h: u16) -> Rect {
        Rect::new(0, 0, w, h)
    }

    // --- activity ---------------------------------------------------------------------------

    #[test]
    fn empty_and_one_frame_players_are_not_active() {
        let p = GifPlayer::new();
        assert!(!p.is_active() && !p.has_frames());
        let one = player(1, ms(50));
        assert!(!one.is_active(), "a one-frame GIF is a still image");
        assert!(one.has_frames(), "but its frames are still held");
        assert!(player(2, ms(50)).is_active());
    }

    #[test]
    fn an_inactive_player_never_advances_and_has_no_poll_timeout() {
        let t0 = Instant::now();
        for mut p in [GifPlayer::new(), player(1, ms(10))] {
            assert!(!p.advance_if_due(t0));
            assert!(!p.advance_if_due(t0 + ms(10_000)));
            assert_eq!(p.poll_timeout(t0), None);
            assert_eq!(p.frame_index(), 0);
        }
    }

    // --- frame advance timing -----------------------------------------------------------------

    #[test]
    fn the_first_tick_only_starts_timing() {
        let t0 = Instant::now();
        let mut p = player(3, ms(50));
        assert!(!p.advance_if_due(t0), "the first frame is already shown");
        assert_eq!(p.frame_index(), 0);
        // The clock started at t0, not at some later tick.
        assert!(!p.advance_if_due(t0 + ms(49)));
        assert!(p.advance_if_due(t0 + ms(50)));
    }

    #[test]
    fn the_delay_boundary_is_inclusive() {
        let t0 = Instant::now();
        let mut p = player(2, ms(50));
        p.advance_if_due(t0);
        assert!(!p.advance_if_due(t0 + ms(49)), "one ms early");
        assert_eq!(p.frame_index(), 0);
        assert!(p.advance_if_due(t0 + ms(50)), "exactly the delay");
        assert_eq!(p.frame_index(), 1);
    }

    #[test]
    fn the_next_deadline_counts_from_the_tick_that_advanced() {
        let t0 = Instant::now();
        let mut p = player(3, ms(50));
        p.advance_if_due(t0);
        // A late tick (120ms) advances once, not twice, and re-bases the clock there.
        assert!(p.advance_if_due(t0 + ms(120)));
        assert_eq!(p.frame_index(), 1);
        assert!(!p.advance_if_due(t0 + ms(169)));
        assert!(p.advance_if_due(t0 + ms(170)));
        assert_eq!(p.frame_index(), 2);
    }

    #[test]
    fn frames_wrap_around_and_each_uses_its_own_delay() {
        let t0 = Instant::now();
        let mut p = GifPlayer::new();
        p.set_frames(vec![frame(8, 8, ms(10)), frame(8, 8, ms(30))]);
        p.advance_if_due(t0);
        assert!(p.advance_if_due(t0 + ms(10))); // frame 0 (10ms) -> 1
        assert!(!p.advance_if_due(t0 + ms(39))); // frame 1 needs 30ms from t0+10
        assert!(p.advance_if_due(t0 + ms(40)));
        assert_eq!(p.frame_index(), 0, "wrapped to the first frame");
    }

    #[test]
    fn a_zero_delay_frame_advances_on_every_tick() {
        let t0 = Instant::now();
        let mut p = player(2, Duration::ZERO);
        p.advance_if_due(t0);
        assert!(p.advance_if_due(t0));
        assert_eq!(p.frame_index(), 1);
        assert!(p.advance_if_due(t0));
        assert_eq!(p.frame_index(), 0);
        assert!(p.advance_if_due(t0));
        assert_eq!(p.frame_index(), 1);
    }

    // --- poll timeout ---------------------------------------------------------------------------

    #[test]
    fn poll_timeout_before_timing_starts_is_the_minimum() {
        let p = player(2, ms(500));
        assert_eq!(p.poll_timeout(Instant::now()), Some(POLL_MIN));
    }

    #[test]
    fn poll_timeout_is_the_remaining_time_clamped() {
        let t0 = Instant::now();
        let mut p = player(2, ms(60));
        p.advance_if_due(t0);
        assert_eq!(p.poll_timeout(t0), Some(ms(60)), "within the range as is");
        assert_eq!(p.poll_timeout(t0 + ms(35)), Some(ms(25)));
        assert_eq!(p.poll_timeout(t0 + ms(55)), Some(POLL_MIN), "5ms -> floor");
        assert_eq!(p.poll_timeout(t0 + ms(60)), Some(POLL_MIN), "due");
        assert_eq!(p.poll_timeout(t0 + ms(900)), Some(POLL_MIN), "overdue");
        let mut slow = player(2, ms(2000));
        slow.advance_if_due(t0);
        assert_eq!(slow.poll_timeout(t0), Some(POLL_MAX), "ceiling");
        assert_eq!(slow.poll_timeout(t0 + ms(1850)), Some(POLL_MAX), "150 left");
        assert_eq!(slow.poll_timeout(t0 + ms(1930)), Some(ms(70)));
    }

    #[test]
    fn poll_timeout_constants_are_10_and_100_ms() {
        assert_eq!(POLL_MIN, ms(10));
        assert_eq!(POLL_MAX, ms(100));
    }

    // --- set_frames / clear ---------------------------------------------------------------------

    #[test]
    fn set_frames_restarts_playback_and_drops_the_encoded_frame() {
        let t0 = Instant::now();
        let picker = Picker::halfblocks();
        let mut p = player(3, ms(10));
        let mut st = ImageViewState::default();
        let mut v = ImageViewer::new();
        p.advance_if_due(t0);
        p.advance_if_due(t0 + ms(10));
        p.prepare(&mut st, &mut v, &picker, 1.0, area(20, 10));
        assert_eq!(p.frame_index(), 1);
        assert!(p.protocol().is_some());
        p.set_frames(vec![frame(8, 8, ms(10)), frame(8, 8, ms(10))]);
        assert_eq!(p.frame_index(), 0);
        assert!(p.protocol().is_none());
        // Timing starts over: the next tick is a "first tick" again.
        assert!(!p.advance_if_due(t0 + ms(5000)));
    }

    #[test]
    fn clear_forgets_frames_protocol_and_timing() {
        let t0 = Instant::now();
        let picker = Picker::halfblocks();
        let mut p = player(2, ms(10));
        let mut st = ImageViewState::default();
        let mut v = ImageViewer::new();
        p.advance_if_due(t0);
        p.prepare(&mut st, &mut v, &picker, 1.0, area(20, 10));
        p.clear();
        assert!(!p.is_active() && !p.has_frames());
        assert!(p.protocol().is_none());
        assert_eq!(p.poll_timeout(t0), None);
        assert_eq!(p.frame_index(), 0);
        assert!(p.proto_key.is_none() && p.shown_at.is_none());
        assert!(p
            .prepare(&mut st, &mut v, &picker, 1.0, area(20, 10))
            .is_none());
    }

    // --- prepare: re-encode conditions ----------------------------------------------------------

    #[test]
    fn prepare_encodes_once_per_frame_and_crop_pair() {
        let picker = Picker::halfblocks();
        let t0 = Instant::now();
        let mut p = player(2, ms(10));
        let mut st = ImageViewState::default();
        let mut v = ImageViewer::new();
        let a = area(20, 10);

        let target = p.prepare(&mut st, &mut v, &picker, 1.0, a).unwrap();
        assert!(p.protocol().is_some());
        let key0 = p.proto_key;
        assert_eq!(key0.map(|k| k.0), Some(0));

        // Same frame, same crop: nothing is re-encoded. Drop the protocol to prove it: a re-encode
        // would put a new one back.
        p.protocol = None;
        assert_eq!(p.prepare(&mut st, &mut v, &picker, 1.0, a), Some(target));
        assert!(
            p.protocol().is_none(),
            "unchanged (frame, crop): no re-encode"
        );
        assert_eq!(p.proto_key, key0);

        // The frame index changes: re-encoded.
        p.advance_if_due(t0);
        assert!(p.advance_if_due(t0 + ms(10)));
        p.prepare(&mut st, &mut v, &picker, 1.0, a);
        assert!(p.protocol().is_some(), "a new frame is encoded");
        assert_eq!(p.proto_key.map(|k| k.0), Some(1));
    }

    #[test]
    fn prepare_re_encodes_when_only_the_crop_changes() {
        let picker = Picker::halfblocks();
        let mut p = big_player(2, ms(10));
        let mut st = ImageViewState::default();
        let mut v = ImageViewer::new();
        let a = area(20, 10);
        p.prepare(&mut st, &mut v, &picker, 1.0, a).unwrap();
        let key_fit = p.proto_key;
        p.protocol = None;
        // Zoom in: the crop shrinks while the frame index stays 0.
        st.zoom_by(4.0);
        p.prepare(&mut st, &mut v, &picker, 1.0, a).unwrap();
        assert!(p.protocol().is_some(), "a zoom changes the crop");
        assert_ne!(p.proto_key, key_fit);
        assert_eq!(p.proto_key.map(|k| k.0), Some(0));
        // Pan along the clipped axis: the crop moves again.
        p.protocol = None;
        v.pan(&mut st, 1.0, 0.0);
        p.prepare(&mut st, &mut v, &picker, 1.0, a).unwrap();
        assert!(p.protocol().is_some(), "a pan changes the crop");
    }

    #[test]
    fn prepare_records_the_layout_in_the_viewer_and_clamps_the_center() {
        let picker = Picker::halfblocks();
        let mut p = big_player(2, ms(10));
        let mut st = ImageViewState::default();
        let mut v = ImageViewer::new();
        assert_eq!(v.crop(), None);
        p.prepare(&mut st, &mut v, &picker, 1.0, area(20, 10))
            .unwrap();
        let fit_crop = v.crop().expect("crop recorded");
        assert_eq!(v.vis_frac(), (1.0, 1.0), "fit: nothing clipped");
        st.zoom_by(4.0);
        st.center = (5.0, -5.0); // far outside
        p.prepare(&mut st, &mut v, &picker, 1.0, area(20, 10))
            .unwrap();
        assert!(v.vis_frac().0 < 1.0, "zoomed: clipped");
        assert_ne!(v.crop(), Some(fit_crop));
        assert!((0.0..=1.0).contains(&st.center.0) && (0.0..=1.0).contains(&st.center.1));
        // `set_crop(None)` (what a host does when it loads new frames) is not undone by itself.
        v.set_crop(None);
        assert_eq!(v.crop(), None);
    }

    #[test]
    fn prepare_returns_none_without_frames_or_room() {
        let picker = Picker::halfblocks();
        let mut st = ImageViewState::default();
        let mut v = ImageViewer::new();
        let mut empty = GifPlayer::new();
        assert!(empty
            .prepare(&mut st, &mut v, &picker, 1.0, area(20, 10))
            .is_none());
        let mut p = player(2, ms(10));
        assert!(p
            .prepare(&mut st, &mut v, &picker, 1.0, area(0, 0))
            .is_none());
        assert!(p.protocol().is_none());
        assert_eq!(v.crop(), None, "a failed layout records nothing");
    }
}
