use super::office_doc::PictureSource;
use super::*;

use konoma_plugin::document_pictures::{
    DecodeJob, DocPictures, Requested, MAX_SYNTHETIC_RENDERS_IN_FLIGHT,
};

// The tmux detection lives with the cache that reads it (`InlineImageCache::attach_picker`).
#[cfg(test)]
use konoma_widgets::inline_images::is_tmux_from_env;

/// Test seam for [`kitty_id_for`] (the allocation itself lives with the cache, in
/// `konoma_widgets::inline_images`).
#[cfg(test)]
pub(super) fn kitty_id_for_test(
    ids: &mut std::collections::HashMap<PathBuf, [Vec<u32>; 3]>,
    path: &Path,
    key: &MdEncodeKey,
    slot: usize,
    use_kitty: bool,
) -> Option<u32> {
    kitty_id_for(ids, path, key, slot, use_kitty)
}

#[cfg(test)]
pub(super) use konoma_widgets::inline_images::is_rebuildable_key as is_rebuildable_md_key;

impl App {
    /// Attach the image backend (terminal Picker and the offload tx) at startup.
    pub fn attach_image_backend(&mut self, picker: Picker, tx: UnboundedSender<ResizeRequest>) {
        self.md_images.attach_picker(&picker);
        self.picker = Some(picker);
        self.img_tx = Some(tx);
    }

    /// Attach the sending end that offloads heavy media loading (SVG/GIF) to a separate thread.
    pub fn attach_media_loader(&mut self, tx: std::sync::mpsc::Sender<MediaResult>) {
        self.media_tx = Some(tx);
    }

    /// Attach the sender that offloads the kitty resize+compress (zoom/pan) to a worker thread.
    /// Without it (tests), kitty builds run synchronously.
    pub fn attach_kitty_loader(&mut self, tx: std::sync::mpsc::Sender<KittyResult>) {
        self.kitty_tx = Some(tx);
    }

    /// Attach the sender that offloads inline Markdown image decoding to a background thread.
    pub fn attach_md_image_loader(&mut self, tx: std::sync::mpsc::Sender<MdImageResult>) {
        self.md_img_tx = Some(tx);
    }

    /// Attach the sender that offloads inline-image encoding (resize + protocol) to the encode worker.
    pub fn attach_md_encoder(&mut self, tx: std::sync::mpsc::Sender<MdEncodeRequest>) {
        self.md_images.enc_tx = Some(tx);
    }

    /// Begin a frame of any kind (`ui::render`): wishes recorded by an earlier frame
    /// (`MdImgEntry::rebuild_wanted`) are now history unless this frame renews them.
    pub fn begin_frame(&mut self) {
        self.draw_seq = self.draw_seq.wrapping_add(1);
    }

    /// Begin one inline-image overlay pass — see `App::md_frame`. Called by the renderer before it
    /// walks the placements, so that every encode request the pass makes is stamped with the same
    /// number and the slots it is drawing from are protected from being recycled underneath it.
    pub fn begin_md_image_frame(&mut self) {
        self.md_images.begin_pass();
    }

    /// Start a new request (decode, rebuild of evicted pixels or re-raster) for the existing entry
    /// `path` and return its id, which the worker puts into its `MdImageResult` (and encodes carry
    /// the entry's `born`). The entry then answers to this id alone: whatever an earlier request of
    /// the same path is still running is stale from here on (`MdImgEntry::request`).
    pub(super) fn begin_md_request(&mut self, path: &Path) -> u64 {
        self.md_images.next_request(path)
    }

    /// Test-only: the request id an entry currently answers to (0 = none / no entry).
    #[cfg(test)]
    pub fn md_request_for_test(&self, path: &Path) -> u64 {
        self.md_images.cache.get(path).map_or(0, |e| e.request)
    }

    /// The document's picture state as the shared rules see it: the host's cache and download
    /// sets, the channels results come back on, and the remote-images setting. Build it after
    /// everything else the call needs from `self` has been read (it borrows three fields).
    pub(super) fn doc_pictures(&mut self) -> DocPictures<'_> {
        #[cfg(test)]
        let fetcher =
            konoma_plugin::document_pictures::get_test_fetcher().unwrap_or(fetch_remote_image);
        #[cfg(not(test))]
        let fetcher = fetch_remote_image;
        DocPictures {
            cache: &mut self.md_images,
            remote_inflight: &mut self.md_remote_inflight,
            remote_failed: &mut self.md_remote_failed,
            results: self.md_img_tx.clone(),
            fetches: self.md_remote_tx.clone(),
            remote_images: self.cfg.external.remote_images,
            fetcher,
        }
    }

    /// Apply a completed background decode of an inline Markdown image. Returns whether to redraw.
    pub fn apply_md_image(&mut self, res: MdImageResult) -> bool {
        let applied = self.doc_pictures().apply_result(res);
        if applied.relayout {
            self.md_cache = None;
        }
        if applied.pixels {
            self.evict_md_images_over_budget();
        }
        applied.redraw
    }

    /// Drive the **in-place zoom** of the focused inline mermaid diagram: clamp the pan center,
    /// kick a sharpening re-raster when the zoom outgrows the raster density (the layout size stays
    /// fixed via `layout_px`), and request an encode of the current crop into the same (cols, rows)
    /// cell area. All heavy work runs on worker threads; at most one of each is in flight.
    pub fn ensure_md_fence_zoom(&mut self, url: &str, cols: u16, rows: u16) {
        use image::GenericImageView;
        let key_path = PathBuf::from(url);
        // On screen now: stamp it, and bring back the pixels if the cache dropped them.
        if !self.ensure_md_pixels(&key_path) {
            return;
        }
        let zoom = self.tab.fence_zoom;
        let font = self.picker.as_ref().map(|p| p.font_size());
        let Some(entry) = self.md_images.cache.get_mut(&key_path) else {
            return;
        };
        let Some(img) = entry.decoded.clone() else {
            return;
        };
        let (sw, sh) = img.dimensions();
        // Sharpen: once the density needed for display (cells × font px × zoom) exceeds the
        // current raster, re-raster the retained SVG at high density (same as the full-screen
        // sharp zoom; the layout stays fixed via layout_px).
        if let Some(f) = font {
            let disp = ((cols as f64 * f.width as f64).max(rows as f64 * f.height as f64) * zoom)
                .ceil() as u32;
            if self.fence_sharpen_if_needed(&key_path, disp) == FenceSharpen::AppliedSync {
                // Synchronous fallback (tests): start over from scratch with the new raster.
                drop(img);
                return self.ensure_md_fence_zoom(url, cols, rows);
            }
        }
        // Cut the visible window out of the current raster (ratio-based, so it's the same window
        // after a re-raster). Clamp the center at the edges and write it back.
        let f = (1.0 / zoom.max(1.0)).clamp(0.0, 1.0);
        let (crop, center) = fence_crop((sw, sh), f, self.tab.fence_center);
        self.tab.fence_center = center;
        self.md_images
            .ensure_zoom_encode(&key_path, img, cols, rows, crop);
    }

    /// The image for the focused inline diagram's current zoom, if encoded for this (cols, rows).
    /// While a fresh crop is still encoding, the previous zoom crop (or the unzoomed full image)
    /// stays visible instead of blinking out.
    pub fn md_fence_zoom_proto(&self, url: &str, cols: u16, rows: u16) -> Option<&InlineImage> {
        let entry = self.md_images.cache.get(&PathBuf::from(url))?;
        entry
            .zoom
            .iter()
            // Only a crop encoded into *this* cell area may stand in: one made for another area
            // would be drawn at the wrong scale. A slot claimed by an in-flight encode holds no
            // picture yet, so it is skipped rather than blanking the diagram mid-zoom.
            .filter(|s| {
                s.image.is_some()
                    && matches!(s.key, MdEncodeKey::Zoom { cols: c, rows: r, .. } if (c, r) == (cols, rows))
            })
            .max_by_key(|s| s.used)
            .and_then(|s| s.image.as_ref())
            .or_else(|| entry.newest_full())
    }

    /// Kick a sharpening re-raster of a fence diagram when `needed_px` exceeds the current raster
    /// density (shared by the unzoomed density follow-up and the in-place zoom). Returns what
    /// happened so the synchronous test fallback can re-run with the fresh raster.
    fn fence_sharpen_if_needed(&mut self, key_path: &PathBuf, needed_px: u32) -> FenceSharpen {
        use image::GenericImageView;
        let img_tx = self.md_img_tx.clone();
        let Some(entry) = self.md_images.cache.get_mut(key_path) else {
            return FenceSharpen::NotNeeded;
        };
        let (Some(img), Some(svg)) = (entry.decoded.as_ref(), entry.svg.clone()) else {
            return FenceSharpen::NotNeeded;
        };
        let trust = entry.svg_trust.clone();
        let (sw, sh) = img.dimensions();
        let cur = sw.max(sh);
        if needed_px <= cur + cur / 8 || cur >= 4096 || entry.reraster_inflight {
            return FenceSharpen::NotNeeded;
        }
        entry.reraster_inflight = true;
        let request = self.begin_md_request(key_path);
        let target = needed_px.min(4096);
        let kp = key_path.clone();
        // A second copy for the panic-fallback result below: `job` (built next) moves its own copy
        // of `kp` into the `MdImageResult` it builds on success, so the failure path needs its own.
        let kp_on_panic = kp.clone();
        let job = move || {
            let image = redraw_svg(&svg, &trust, target);
            MdImageResult {
                path: kp,
                image,
                svg: None,
                reraster: true,
                frames: None,
                request,
            }
        };
        if let Some(tx) = img_tx {
            std::thread::spawn(move || {
                // Same `compute_or_fallback` safety net as `ensure_mermaid_fence_render`/
                // `ensure_math_render` (principle #3): `rasterize_trusted` (resvg) is the exact same
                // panic-prone call those already guard, and the `tx.send(..)` below is unconditional.
                // Without this, a panic here would kill the thread before it sends anything, leaving
                // `entry.reraster_inflight` latched `true` forever — this fence could never sharpen
                // again. `reraster: true` on the fallback makes `apply_md_image` take its "a
                // re-raster failure leaves the current raster in place" branch (clears
                // `reraster_inflight`, changes nothing else) rather than wrongly degrading the whole
                // diagram to the text fallback (that branch is reserved for a genuinely first-ever
                // failure).
                let res = crate::preview::markdown::compute_or_fallback(job, || MdImageResult {
                    path: kp_on_panic,
                    image: Err("re-raster panicked".to_string()),
                    svg: None,
                    reraster: true,
                    frames: None,
                    request,
                });
                let _ = tx.send(res);
            });
            FenceSharpen::Spawned
        } else {
            let res = job();
            // A failure must not claim AppliedSync: the caller (ensure_md_fence_zoom) recurses on
            // AppliedSync to "start over with the new raster", so repeated failures would loop
            // forever. On success the recursion always converges in one step (the next pass hits
            // NotNeeded, either because needed<=cur or the 4096 cap).
            let ok = res.image.is_ok();
            self.apply_md_image(res);
            if ok {
                FenceSharpen::AppliedSync
            } else {
                FenceSharpen::NotNeeded
            }
        }
    }

    /// Keep an inline diagram's raster density up with its **display size** (called per visible
    /// mermaid placement). `mermaid_rows` can size diagrams beyond the base raster (svg_max_px),
    /// so without this a large setting would show upscaled-blurry pixels.
    pub fn ensure_md_fence_density(&mut self, url: &str, cols: u16, rows: u16) {
        let Some(f) = self.picker.as_ref().map(|p| p.font_size()) else {
            return;
        };
        if !self.ensure_md_pixels(&PathBuf::from(url)) {
            return;
        }
        let needed =
            ((cols as f64 * f.width as f64).max(rows as f64 * f.height as f64)).ceil() as u32;
        let _ = self.fence_sharpen_if_needed(&PathBuf::from(url), needed);
    }

    /// Frame bookkeeping for the inline-image overlay (called by `ui::render`): reset at frame
    /// start, recorded by the overlay, compared at frame end. A change after a drawn state marks
    /// the frame "moved" so the run loop clears the terminal once (placeholder-orphan sweep).
    pub fn begin_md_overlay_frame(&mut self) {
        self.md_overlay.begin();
    }

    /// Record the overlay signature for this frame (urls + screen rects of drawn inline images).
    pub fn note_md_overlay(&mut self, sig: u64) {
        self.md_overlay.note(sig);
    }

    /// Compare this frame's overlay against the previous frame; latch "moved" on any change away
    /// from a previously drawn state (position shift, or the images left the screen entirely).
    pub fn finish_md_overlay_frame(&mut self) {
        self.md_overlay.finish();
    }

    /// Whether the run loop should full-clear once to sweep orphaned placeholder rows (resets).
    pub fn take_md_overlay_moved(&mut self) -> bool {
        self.md_overlay.take_moved()
    }

    /// How many mermaid-fence/math renders have a placeholder but no result yet.
    #[cfg(test)]
    pub(super) fn synthetic_renders_in_flight(&self) -> usize {
        konoma_plugin::document_pictures::synthetic_in_flight(&self.md_images)
    }

    /// Ensure a background mermaid render is in flight (or cached) for one ```mermaid fence.
    /// With no loader tx (tests), renders synchronously so behavior stays observable. Returns
    /// true when the render completed **synchronously**: the caller must rebuild its just-built
    /// decoration, which still shows the loading line.
    pub(super) fn ensure_mermaid_fence_render(&mut self, code: String) -> bool {
        // Cached already (rendered or in flight): nothing to do, and the plugin lookup below
        // (registry and settings) must not run on the UI thread at every re-layout.
        let key = PathBuf::from(crate::preview::markdown::mermaid_fence_url(&code));
        if self.doc_pictures().is_cached(&key) {
            return false;
        }
        let max_px = self.mermaid_px();
        // The plugin that takes the `fence:mermaid` kind draws the SVG (`embed_plugin.rs`).
        let embed = self.embed_render(crate::app::embed_plugin::FENCE_MERMAID);
        self.doc_pictures().request_fence(
            code,
            embed,
            crate::app::embed_plugin::FENCE_MERMAID,
            max_px,
        ) == Requested::Done
    }

    /// Ensure a background render is in flight (or cached) for one math expression, keyed by latex +
    /// display. Returns true on a synchronous completion (no loader tx = tests).
    pub(super) fn ensure_math_render(&mut self, latex: String, display: bool) -> bool {
        let key = PathBuf::from(crate::preview::markdown::math_url(&latex, display));
        if self.doc_pictures().is_cached(&key) {
            return false;
        }
        let max_px = self.math_px();
        // The plugin that takes the `math` kind draws the SVG; its `color` setting is the glyph color.
        let embed = self.embed_render(crate::app::embed_plugin::MATH);
        self.doc_pictures().request_math(
            latex,
            display,
            embed,
            crate::app::embed_plugin::MATH,
            max_px,
        ) == Requested::Done
    }

    /// Apply a completed background encode into the slot that was reserved for it. Returns redraw.
    /// A failed encode (`image: None`) leaves that slot's stored image alone — the last good state
    /// stays visible — while the slot keeps holding the key, which is what stops the same doomed
    /// request being re-sent every frame. Only when the whole picture was never encodable at any
    /// size does the entry degrade to the text fallback (principle #3).
    ///
    /// A result whose slot has since been recycled (its key no longer matches) is simply dropped:
    /// the position it answered is not on screen any more, and writing it back would evict a slot
    /// the current frame *is* drawing from.
    pub fn apply_md_encode(&mut self, res: MdEncodeResult) -> bool {
        match self.md_images.apply_encode(res) {
            EncodeApplied::Dropped => false,
            EncodeApplied::Applied => true,
            EncodeApplied::Degraded => {
                // An image that has newly degraded to failed gets re-laid-out as a text row (never
                // left as an invisible blank).
                self.md_cache = None;
                true
            }
        }
    }

    /// Attach the sender that reports background remote-image download completions to the run loop.
    pub fn attach_remote_md_loader(&mut self, tx: std::sync::mpsc::Sender<RemoteFetch>) {
        self.md_remote_tx = Some(tx);
    }

    /// Apply a completed remote-image download. On success the file is now cached, so drop the
    /// decoration cache to re-lay the image out inline; on failure remember the URL so it is not
    /// retried and shows a text placeholder instead. Returns whether to redraw.
    pub fn apply_remote_fetch(&mut self, res: RemoteFetch) -> bool {
        self.doc_pictures().apply_fetch(res);
        // Re-decorate so a now-cached image is laid out (or a failed one degrades to text).
        self.md_cache = None;
        true
    }

    /// Ensure a background download is in flight for the remote image `url` (deduplicated).
    /// Returns true on a *synchronous* failure (remote images off) so the caller
    /// (`ensure_md_cache`) can resync its already-built `decorated` before storing it.
    pub(super) fn ensure_remote_md_fetch(&mut self, url: &str) -> bool {
        // A test build must never spawn a download that writes under the real cache: the test
        // has to opt a sandboxed root in first (`test_support::set_test_cache_root`). This is
        // checked here, on the calling thread (the override is thread-local), under the same
        // conditions in which the shared rule starts a download.
        #[cfg(test)]
        if self.cfg.external.remote_images
            && crate::preview::markdown::is_remote_image_url(url)
            && resolve_md_image_path(url, None).is_none()
            && !self.md_remote_failed.contains(url)
            && !self.md_remote_inflight.contains(url)
            && self.md_remote_tx.is_some()
        {
            assert!(
                crate::test_support::get_test_cache_root().is_some(),
                "about to spawn a real background download for {url:?} that would write under \
                 the real cache directory: call test_support::set_test_cache_root() before \
                 triggering a remote fetch in a test"
            );
        }
        self.doc_pictures().request_remote(url)
    }

    /// Start the background decode of the inline image file at `path` (its cache entry already
    /// exists). With no loader channel attached (tests) nothing is started. Returns false when
    /// the thread could not be started (the caller must not leave the entry waiting).
    pub(super) fn spawn_md_decode(&mut self, path: PathBuf) -> bool {
        let job = self.md_decode_job(&path);
        self.doc_pictures().spawn_decode(path, job)
    }

    /// What a decode of the picture file `path` needs from the host.
    fn md_decode_job(&self, path: &Path) -> DecodeJob {
        // A document of a plugin the user added: an SVG picture reads only files in its scope.
        let roots: Option<Vec<PathBuf>> = self
            .md_image_scope()
            .and_then(|(_, s)| s.dirs().map(<[PathBuf]>::to_vec));
        // The file read is the real path the scope judged, not the spelling that was asked for.
        let read_path = self.md_decode_read_path(path);
        DecodeJob {
            svg_max_px: self.cfg.ui.svg_max_px,
            roots,
            read_path,
            svg: Arc::new(media_plugin::SvgChild),
            // The media generation changes on every file/tab change.
            generation: (self.media_gen, self.media_gen_shared.clone()),
        }
    }

    /// The one place that makes an inline picture ready to be drawn: stamp it as wanted by the
    /// overlay pass being drawn (`last_used` for the cache's eviction order, the ticket's priority
    /// for the decode queue) and, when the cache had dropped its pixels, start getting them back.
    /// Returns whether the pixels are there now.
    ///
    /// Every path that needs the pixels of a picture it is drawing calls this — the encode of a
    /// placement (`ensure_md_image`, whether or not that exact encode is already settled), the
    /// in-place zoom, the density follow-up — so that none of them can miss an eviction, and none
    /// can leave a picture that is on screen looking unused to the eviction.
    ///
    /// A rebuild that cannot start yet (`MD_MAX_REBUILDS` already running) is remembered
    /// (`rebuild_wanted`), which keeps the loading indicator, and with it the redraw loop, alive
    /// until a later frame starts it.
    pub(super) fn ensure_md_pixels(&mut self, path: &Path) -> bool {
        let frame = self.md_images.frame;
        let seq = self.draw_seq;
        let has_loader = self.md_img_tx.is_some();
        let Some(entry) = self.md_images.cache.get_mut(path) else {
            return false;
        };
        entry.last_used = frame;
        entry.wish.set_priority(frame);
        if entry.decoded.is_some() {
            return true;
        }
        if !entry.evicted || entry.failed || entry.rebuilding || !has_loader {
            return false;
        }
        let running = self
            .md_images
            .cache
            .values()
            .filter(|e| e.rebuilding)
            .count();
        let Some(entry) = self.md_images.cache.get_mut(path) else {
            return false;
        };
        if running >= MD_MAX_REBUILDS {
            entry.rebuild_wanted = Some(seq);
            return false;
        }
        entry.rebuild_wanted = None;
        entry.rebuilding = true;
        if !self.rebuild_evicted_md_image(path.to_path_buf()) {
            if let Some(e) = self.md_images.cache.get_mut(path) {
                // The thread could not even start: end the wait instead of latching "loading".
                e.rebuilding = false;
                e.failed = true;
            }
        }
        false
    }

    /// Rebuild the pixels of an entry that `evict_md_images_over_budget` dropped. A regular image
    /// is decoded from its file again; a mermaid diagram or formula is rasterized again from the
    /// SVG its entry kept (as a re-raster, so the reserved layout is untouched). Returns false when
    /// the thread could not be started. Only `ensure_md_pixels` calls this (it owns the
    /// bookkeeping and the limit on how many run at once).
    fn rebuild_evicted_md_image(&mut self, path: PathBuf) -> bool {
        let Some(entry) = self.md_images.cache.get_mut(&path) else {
            return true;
        };
        let trust = entry.svg_trust.clone();
        let Some(svg) = entry.svg.clone() else {
            // A picture of the open document: decoded again from the bytes it keeps, through the
            // same entry as the first decode (the defences of the first decode apply again).
            let url = path.to_string_lossy().to_string();
            if crate::preview::markdown::is_office_image_url(&url) {
                return self.start_office_picture_thread(&url, path, false);
            }
            return self.spawn_md_decode(path);
        };
        let s = path.to_string_lossy();
        let max_px = if crate::preview::markdown::is_math_url(&s) {
            self.math_px()
        } else {
            self.mermaid_px()
        };
        let Some(tx) = self.md_img_tx.clone() else {
            return true;
        };
        if let Some(e) = self.md_images.cache.get_mut(&path) {
            e.reraster_inflight = true;
        }
        let request = self.begin_md_request(&path);
        let kp = path.clone();
        let started = std::thread::Builder::new()
            .name("konoma-md-rebuild".into())
            .spawn(move || {
                let job = {
                    let kp = kp.clone();
                    move || MdImageResult {
                        path: kp,
                        image: redraw_svg(&svg, &trust, max_px),
                        svg: None,
                        reraster: true,
                        frames: None,
                        request,
                    }
                };
                let res = crate::preview::markdown::compute_or_fallback(job, || MdImageResult {
                    path: kp,
                    image: Err("re-raster panicked".to_string()),
                    svg: None,
                    reraster: true,
                    frames: None,
                    request,
                });
                let _ = tx.send(res);
            })
            .is_ok();
        if !started {
            if let Some(e) = self.md_images.cache.get_mut(&path) {
                e.reraster_inflight = false;
            }
        }
        started
    }

    /// Keep the decoded pixels of the inline-image cache within `MD_IMAGE_CACHE_BYTES`: drop those
    /// of the pictures asked for longest ago, never one the latest overlay pass drew, and never one
    /// that cannot be rebuilt (media-diff pictures come from git bytes held nowhere else). Their
    /// entries stay, so layout does not move; a picture needed again is rebuilt on demand
    /// (`rebuild_evicted_md_image`).
    pub(super) fn evict_md_images_over_budget(&mut self) {
        self.md_images.evict_to(self.md_cache_budget());
    }

    /// The bytes of decoded pixels the inline-image cache may keep right now.
    pub(super) fn md_cache_budget(&self) -> u64 {
        #[cfg(test)]
        if let Some(b) = self.md_cache_budget_for_test {
            return b;
        }
        if self.deck_picture_view() {
            MD_SLIDE_CACHE_BYTES
        } else {
            MD_IMAGE_CACHE_BYTES
        }
    }

    /// `evict_md_images_over_budget` with the budget as a parameter (tests use a small one).
    #[cfg(test)]
    pub(super) fn evict_md_images_to(&mut self, budget: u64) {
        self.md_images.evict_to(budget);
    }

    /// Test-only: how many rebuilds of evicted pixels are running right now.
    #[cfg(test)]
    pub(crate) fn md_rebuilds_running(&self) -> usize {
        self.md_images
            .cache
            .values()
            .filter(|e| e.rebuilding)
            .count()
    }

    /// Test-only: total bytes of decoded pixels the inline-image cache holds.
    #[cfg(test)]
    pub(crate) fn md_image_cache_pixel_bytes(&self) -> u64 {
        self.md_images
            .cache
            .values()
            .map(MdImgEntry::pixel_bytes)
            .sum()
    }

    /// Run `f` on the read-scope memo of the plugin whose document is on screen: `None` for
    /// konoma's own Markdown preview and the standard plugins (unrestricted, nothing to build).
    /// The scope is built once and kept until what it is made from changes (the plugin, the tab
    /// root, the shown file), so a redraw never walks the disk for it again.
    fn with_scope_memo<R>(&self, f: impl FnOnce(&mut ScopeMemo) -> R) -> Option<R> {
        let show = self.plugin.as_ref()?;
        let registry = crate::plugins::shared_registry(&self.cfg.plugin_disabled());
        if crate::plugins::read_scope::is_host_owned(&registry, &show.id) {
            return None;
        }
        Some(self.scope_memo_for(&show.id, &show.path, f))
    }

    /// Run `f` on the memo of the (non-standard) plugin `id` for the file `shown`, building the
    /// scope only when the plugin, the tab root or the file differs from what is memoised. Shared
    /// by the Markdown pictures and by every job sent to a plugin (a key press, a timed redraw).
    pub(super) fn scope_memo_for<R>(
        &self,
        id: &str,
        shown: &Path,
        f: impl FnOnce(&mut ScopeMemo) -> R,
    ) -> R {
        use crate::plugins::read_scope::{home_dir, scope_for};
        let mut slot = self.read_scope_memo.borrow_mut();
        let fresh = slot
            .as_ref()
            .is_some_and(|m| m.id == id && m.root == self.tab.root && m.shown == shown);
        if !fresh {
            *slot = Some(ScopeMemo {
                id: id.to_string(),
                root: self.tab.root.clone(),
                shown: shown.to_path_buf(),
                scope: scope_for(
                    false,
                    self.cfg.plugins.get(id),
                    &self.tab.root,
                    shown,
                    home_dir().as_deref(),
                ),
                verdicts: std::collections::HashMap::new(),
            });
        }
        f(slot.as_mut().expect("memo was just filled"))
    }

    /// The path a decode of the local picture `p` must read: the real path the scope judged
    /// (so the file judged is the file read), or `p` itself when no scope applies or `p` cannot
    /// be resolved (reading it fails the same way; the ordinary missing-picture treatment shows
    /// that). `None` when the scope says the file is outside: **nothing may be read** for it, so
    /// the caller has no path to read.
    pub(super) fn md_decode_read_path(&self, p: &Path) -> Option<PathBuf> {
        self.with_scope_memo(|m| match m.judge(p) {
            Ok(real) => Some(real),
            Err(konoma_plugin::ScopeError::Outside) => None,
            Err(konoma_plugin::ScopeError::Unresolved) => Some(p.to_path_buf()),
        })
        .unwrap_or_else(|| Some(p.to_path_buf()))
    }

    /// The read scope for the local pictures a document names, when the document is a plugin's:
    /// the plugin's id and its scope. `None` for konoma's own Markdown preview and the standard
    /// plugins (unrestricted).
    pub(super) fn md_image_scope(&self) -> Option<(String, konoma_plugin::ReadScope)> {
        self.with_scope_memo(|m| (m.id.clone(), m.scope.clone()))
    }

    /// Forget the verdicts on single pictures (the page is built again, so a file that was
    /// replaced by a link meanwhile is judged anew). The scope itself stays.
    pub(super) fn forget_scope_verdicts(&self) {
        if let Some(m) = self.read_scope_memo.borrow_mut().as_mut() {
            m.verdicts.clear();
        }
    }

    /// Whether the read scope refuses the local picture `p` that `url` of the open document named:
    /// the id of the plugin whose scope refuses it. Only a file that is really **outside** the
    /// scope is refused here: one that does not exist or cannot be resolved is not a scope matter
    /// (the ordinary missing-picture treatment shows it). A remote picture (konoma's own download
    /// cache) is never refused. The verdict is kept per path until the page is built again.
    pub(super) fn md_image_refused(&self, url: &str, p: &Path) -> Option<String> {
        if crate::preview::markdown::is_remote_image_url(url.trim()) {
            return None;
        }
        self.with_scope_memo(|m| {
            (m.judge(p) == Err(konoma_plugin::ScopeError::Outside)).then(|| m.id.clone())
        })
        .flatten()
    }

    /// Ensure the inline image for `url` is decoding in the background and that the protocol for the
    /// currently-visible portion (whole image, or a cropped band when partially scrolled) is encoding on
    /// the worker thread. Called from the renderer for each visible inline image. Both decoding and
    /// encoding are off-thread (principle #4) so this never blocks the UI; the protocol appears a frame
    /// or two later. At most one encode is in flight per image, so scrolling never queues a backlog.
    ///
    /// The (cols, rows) box is part of what is being asked for, not just how to draw the answer: the
    /// same picture can be on screen at several sizes at once (a block image and a copy shaved to a
    /// table column), and each of those keeps an encode of its own — see `MD_PROTO_SLOTS`. Every call
    /// also stamps its position as wanted by the frame currently being drawn, which is what stops
    /// those placements from evicting one another.
    pub fn ensure_md_image(
        &mut self,
        url: &str,
        cols: u16,
        full_rows: u16,
        row_off: u16,
        vis_rows: u16,
    ) {
        // A synthetic key (fence diagram / math expression) is used as-is for the cache key
        // (decoding was already done by ensure_mermaid_fence_render / ensure_math_render). Only a
        // real file image gets resolved on disk. Treating the math `math://` key as a real file
        // made resolve return None, so an encode was never requested and the reserved row stayed
        // blank.
        let path = if crate::preview::markdown::is_synthetic_md_url(url) {
            PathBuf::from(url)
        } else {
            let base = self
                .tab
                .preview_path
                .as_ref()
                .and_then(|p| p.parent())
                .map(|p| p.to_path_buf());
            let Some(p) = resolve_md_image_path(url, base.as_deref()) else {
                return;
            };
            // A plugin's document: a picture outside its read scope is not read. (The path is
            // resolved once, here, and `md_image_refused` does nothing at all when no scope
            // applies, so konoma's own Markdown pays no extra file access per frame.)
            if let Some(id) = self.md_image_refused(url, &p) {
                self.flash_scope_refusal(&id);
                return;
            }
            p
        };
        // A slide is drawn at the size of the frame it is placed in (`cols` x `full_rows` cells).
        let slide_px = self.slide_raster_px(url, cols, full_rows);
        // A picture of a Word document: decode it from the bytes held in memory (never a file).
        if crate::preview::markdown::is_office_image_url(url)
            && !self.md_images.cache.contains_key(&path)
        {
            // At most MAX_SYNTHETIC_RENDERS_IN_FLIGHT decodes at once, like the fence/math renders:
            // a tall terminal over a picture-heavy document would otherwise start one thread per
            // visible picture. Skipping leaves no entry, so the next draw (a landed decode always
            // causes one) asks again.
            if self.office_pictures_in_flight() >= MAX_SYNTHETIC_RENDERS_IN_FLIGHT {
                return;
            }
            self.spawn_office_picture_decode(url, path, slide_px);
            return;
        }
        if let Some(px) = slide_px {
            self.note_slide_raster_wish(url, &path, px);
        }
        // Kick off a one-time background decode.
        if !self.md_images.cache.contains_key(&path) {
            // A synthetic key (fence diagram / math expression) cannot be built here (the original
            // code/latex is needed). If a placement exists it should already be cached, so this is
            // a defensive guard for the case it never arrives (the next re-decoration has
            // ensure_mermaid_fence_render / ensure_math_render rebuild it). Never let `math://` be
            // decoded as a real file.
            if crate::preview::markdown::is_synthetic_md_url(url) {
                return;
            }
            let job = self.md_decode_job(&path);
            self.doc_pictures().begin_decode(path, job);
            return;
        }
        // Stamp the picture as drawn this pass and get its pixels back if the cache dropped them —
        // before anything else, **settled or not**: an encode that is already settled still needs
        // the pixels once the picture is animated, zoomed or sharpened.
        self.ensure_md_pixels(&path);
        // (A canvas picture is not asked for here: `canvas_pictures::draw_images` does, with the
        // cap of its encode slots.)
        self.md_images
            .ensure_encode(&path, cols, full_rows, row_off, vis_rows, None);
    }

    /// How many pictures of the open document are being decoded for the first time right now
    /// (cached, not yet decoded, not failed, pixels never dropped).
    pub(super) fn office_pictures_in_flight(&self) -> usize {
        self.md_images
            .cache
            .iter()
            .filter(|(k, e)| {
                // An entry whose pixels the cache dropped (`evicted`) has no decode running for
                // it: it waits for a rebuild (limited by `MD_MAX_REBUILDS`). Counting it as
                // "in flight" filled the cap with idle entries after enough pictures had been
                // seen, and no new picture was ever started.
                e.decoded.is_none()
                    && !e.failed
                    && !e.evicted
                    && crate::preview::markdown::is_office_image_url(&k.to_string_lossy())
            })
            .count()
    }

    /// The longest edge, in pixels, slide picture `url` should be drawn at to fill a frame of
    /// `cols` x `rows` cells without the terminal enlarging it: the frame's pixel size, never below
    /// `[ui] svg_max_px` and never above `SLIDE_RASTER_MAX_PX`. `None` for anything that is not a
    /// slide of the open presentation, or while the cell size is unknown (no image backend).
    pub(super) fn slide_raster_px(&self, url: &str, cols: u16, rows: u16) -> Option<u32> {
        if !crate::preview::markdown::is_office_image_url(url)
            || !self.document_picture_is_slide(url)
        {
            return None;
        }
        let font = self.picker.as_ref()?.font_size();
        let frame_w = u32::from(cols) * u32::from(font.width.max(1));
        let frame_h = u32::from(rows) * u32::from(font.height.max(1));
        Some(
            frame_w
                .max(frame_h)
                .max(self.cfg.ui.svg_max_px)
                .min(SLIDE_RASTER_MAX_PX),
        )
    }

    /// [`Self::slide_raster_px`] for the end-to-end tests.
    #[cfg(test)]
    pub fn slide_raster_px_for_test(&self, url: &str, cols: u16, rows: u16) -> Option<u32> {
        self.slide_raster_px(url, cols, rows)
    }

    /// Records the pixel size slide `path` is wanted at and, when its pixels are already there but
    /// smaller than the frame now needs (the terminal grew, the font shrank), starts a sharper
    /// redraw of the slide's scene. One redraw at a time per slide; the old pixels keep showing
    /// until the new ones land, and a failed redraw leaves them.
    fn note_slide_raster_wish(&mut self, url: &str, path: &Path, px: u32) {
        use image::GenericImageView;
        let cap = self.slide_raster_cap(url);
        let Some(entry) = self.md_images.cache.get_mut(path) else {
            return;
        };
        entry.want_px = px;
        // A heavy slide cannot be drawn as big as the frame wants (see `Rendered::max_raster_px`).
        let px = cap.map_or(px, |c| px.min(c));
        let Some(decoded) = entry.decoded.as_ref() else {
            return; // evicted or still decoding: the rebuild / first decode draws at `want_px`
        };
        let (w, h) = decoded.dimensions();
        let cur = w.max(h);
        if cur + cur / 8 >= px
            || entry.reraster_inflight
            || entry.rebuilding
            || entry.failed
            || self.md_img_tx.is_none()
        {
            return;
        }
        entry.reraster_inflight = true;
        if !self.start_office_picture_thread(url, path.to_path_buf(), true) {
            if let Some(e) = self.md_images.cache.get_mut(path) {
                e.reraster_inflight = false;
            }
        }
    }

    /// Starts the one-time background decode of a Word document's picture from its in-memory
    /// bytes. The cache entry is placed first (its presence is the "decode in flight" marker, as
    /// for a file image); a picture the document does not hold is left uncached (nothing to draw).
    fn spawn_office_picture_decode(&mut self, url: &str, key: PathBuf, slide_px: Option<u32>) {
        if self.document_picture_source(url).is_none() {
            return;
        }
        self.md_images.cache.insert(
            key.clone(),
            MdImgEntry {
                want_px: slide_px.unwrap_or(0),
                ..MdImgEntry::default()
            },
        );
        if !self.start_office_picture_thread(url, key.clone(), false) {
            // The thread could not start: end the wait instead of latching "loading".
            if let Some(e) = self.md_images.cache.get_mut(&key) {
                e.failed = true;
            }
        }
    }

    /// Starts the thread that decodes picture `url` of the open document into the cache entry
    /// `key` (which already exists: a first decode just placed it, a rebuild of dropped pixels
    /// keeps its own). Returns false when there is nothing to decode (the document no longer
    /// holds the picture) or the thread could not be started; the caller ends the wait.
    fn start_office_picture_thread(&mut self, url: &str, key: PathBuf, reraster: bool) -> bool {
        let Some(source) = self.document_picture_source(url) else {
            return false;
        };
        let Some(tx) = self.md_img_tx.clone() else {
            return true;
        };
        // A slide is drawn at the pixel size its frame needs (never below `svg_max_px`); any other
        // picture of a document at `svg_max_px`.
        let want_px = self.md_images.cache.get(&key).map_or(0, |e| e.want_px);
        // The smallest size a failed slide is retried at: the raster already on screen when this
        // is a sharpening redraw (a retry must never replace it by something smaller), else the
        // configured base size.
        let retry_floor = if reraster {
            use image::GenericImageView;
            self.md_images
                .cache
                .get(&key)
                .and_then(|e| e.decoded.as_ref())
                .map(|d| {
                    let (w, h) = d.dimensions();
                    w.max(h)
                })
                .unwrap_or(0)
                .max(self.cfg.ui.svg_max_px)
        } else {
            self.cfg.ui.svg_max_px
        };
        let svg_max_px = if want_px > 0 {
            want_px.max(self.cfg.ui.svg_max_px).min(SLIDE_RASTER_MAX_PX)
        } else {
            self.cfg.ui.svg_max_px
        };
        let ticket = self
            .md_images
            .cache
            .get(&key)
            .map(|e| e.wish.ticket())
            .unwrap_or_default();
        let request = self.begin_md_request(&key);
        let (gen, latest) = (self.media_gen, self.media_gen_shared.clone());
        let stale = ticket.clone();
        std::thread::Builder::new()
            .name("konoma-office-pic".into())
            .spawn(move || {
                use crate::preview::image::ImageFailure;
                // The same contract as the file decode: queued in the shared decode gate under the
                // entry's ticket, an SVG drawn by the supervised child process (stopped when the
                // document is left), a panic on a pathological picture is a failed decode, and a
                // result is always sent (not sending would latch the busy indicator).
                let moved_on =
                    || latest.load(std::sync::atomic::Ordering::Relaxed) != gen || stale.is_stale();
                let (still, frames) = crate::preview::markdown::catch_silent(|| {
                    crate::preview::image::with_decode_ticket(ticket, || {
                        let mut svg_px = svg_max_px;
                        let mut slide_cap = None;
                        // A slide has no bytes yet: draw its scene to an SVG here (off the UI
                        // thread, under the decode gate) and let the supervised drawing process
                        // rasterize it like any other untrusted SVG of a document.
                        let bytes: Arc<[u8]> = match source {
                            PictureSource::Bytes(b) => b,
                            PictureSource::Lazy {
                                lazy,
                                render_truncated,
                                raster_cap,
                            } => {
                                use konoma_plugin::view::ImageData;
                                let cancel = konoma_plugin::Cancel::from_fn({
                                    let (latest, stale) = (latest.clone(), stale.clone());
                                    move || {
                                        latest.load(std::sync::atomic::Ordering::Relaxed) != gen
                                            || stale.is_stale()
                                    }
                                });
                                let req = konoma_plugin::ResourceReq { max_px: svg_px };
                                let drawn = match lazy.render(&req, &cancel) {
                                    Ok(ImageData::Svg { data, .. } | ImageData::Encoded(data)) => {
                                        data
                                    }
                                    // The preview moved on: nothing to show, nothing to remember.
                                    Err(f) if f.kind == konoma_plugin::FailureKind::Cancelled => {
                                        return (Err(ImageFailure::Cancelled), None);
                                    }
                                    Ok(_) | Err(_) => return (Err(ImageFailure::Corrupt), None),
                                };
                                if lazy.truncated() {
                                    render_truncated
                                        .store(true, std::sync::atomic::Ordering::Relaxed);
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
                                crate::preview::image::decode_gif_bytes_inline(&bytes)
                            {
                                let first = frames[0].0.clone();
                                return (Ok(first), Some(frames));
                            }
                        }
                        // A slide whose picture the drawing process refuses as too heavy or
                        // too slow at this size is drawn again smaller before it is given up on;
                        // the size that worked becomes the slide's raster cap.
                        let res = super::decode_with_cap(
                            slide_cap.as_deref(),
                            svg_px,
                            retry_floor,
                            &moved_on,
                            &std::time::Instant::now,
                            |px| md_decode_bytes_why(&bytes, px, &moved_on),
                        );
                        (res, None)
                    })
                })
                .unwrap_or((Err(ImageFailure::Corrupt), None));
                // A cancelled decode reports like any other result, first decode and re-raster
                // alike: the entry may still be in the cache (a stale ticket does not remove it),
                // and the receiving side (`apply_md_image`) forgets it or ignores a gone one.
                let still = match still {
                    Err(_) if moved_on() => Err(ImageFailure::Cancelled),
                    other => other,
                };
                let _ = tx.send(MdImageResult {
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

    /// The image to draw for the visible portion of inline image `url`, **at this placement's own
    /// cell size**. Prefers the encode made for exactly this position and size (the whole image when
    /// fully visible, or the band matching `(cols, full_rows, row_off, vis_rows)`); while that one is
    /// still encoding it returns the freshest other encode, so the image stays on screen — at another
    /// size or another band for a frame or two — and snaps to the exact one on arrival, rather than
    /// blinking out. None only until the very first encode for this image is ready.
    pub fn md_image_proto(
        &self,
        url: &str,
        cols: u16,
        full_rows: u16,
        row_off: u16,
        vis_rows: u16,
    ) -> Option<&InlineImage> {
        let path = if crate::preview::markdown::is_synthetic_md_url(url) {
            PathBuf::from(url)
        } else {
            let base = self
                .tab
                .preview_path
                .as_ref()
                .and_then(|p| p.parent())
                .map(|p| p.to_path_buf());
            let p = resolve_md_image_path(url, base.as_deref())?;
            self.md_image_refused(url, &p).is_none().then_some(p)?
        };
        self.md_images
            .proto(&path, cols, full_rows, row_off, vis_rows)
    }

    /// Test-only: how many **band** (clip) encodes are currently retained for `url`, i.e. how many
    /// partially-scrolled placements of that one picture are being kept apart from each other.
    #[cfg(test)]
    pub fn md_clip_slot_count(&self, url: &str) -> usize {
        let base = self
            .tab
            .preview_path
            .as_ref()
            .and_then(|p| p.parent())
            .map(|p| p.to_path_buf());
        let path = if crate::preview::markdown::is_synthetic_md_url(url) {
            PathBuf::from(url)
        } else {
            match resolve_md_image_path(url, base.as_deref()) {
                Some(p) => p,
                None => return 0,
            }
        };
        self.md_images
            .cache
            .get(&path)
            .map(|e| e.clip.len())
            .unwrap_or(0)
    }

    /// Drive every animated inline Markdown GIF (the per-entry analog of `App::advance_gif_if_due`
    /// for the full-screen GIF path). Each cache entry with ≥2 frames advances independently once
    /// its current frame's display time has elapsed. Advancing swaps `decoded` to the new frame and
    /// marks every encoded slot **stale** — but does **not** drop the encoded protocols themselves,
    /// which stay visible until `ensure_md_image` (called from the renderer,
    /// only for placements actually drawn) requests and receives a fresh encode of the new frame.
    /// Advancing an off-screen entry therefore costs only an index bump, never a re-encode.
    /// Returns true if any entry advanced (the caller re-renders).
    ///
    /// Gated on `Mode::Preview`: `md_image_cache` is a flat, path-keyed cache that is only emptied
    /// by `enter_preview`'s `if !same_file` branch (switching to a genuinely different file), plus
    /// the per-entry eviction `drop_changed_md_images` does for a picture the filesystem reports as
    /// changed — it is deliberately left alone by `back_to_tree` (re-entering the *same* file must
    /// resume playback without a re-decode). Without this check, leaving Preview via `q` left every
    /// animating entry ticking forever: this kept requesting a fresh encode (the staleness marked
    /// below) and redrawing a *tree* screen where nothing is even visible, defeating the idle-CPU
    /// invariant.
    pub fn advance_md_gifs_if_due(&mut self) -> bool {
        if !matches!(self.tab.mode, Mode::Preview) {
            return false;
        }
        self.md_images.advance_gifs_if_due()
    }

    /// Every filesystem path the Markdown preview currently draws an inline image from.
    ///
    /// This is what makes an FS event about `diagram.png` count as an event about the *document*
    /// showing it — konoma's headline Agent Watch case, where an agent regenerates a picture while
    /// the `.md` embedding it stays open. Two sources, because neither is complete on its own:
    ///
    /// * the **keys of `md_image_cache`** — the pictures decoded right now. A path key does *not*
    ///   go stale by itself (unlike the content-hash key a mermaid fence / math expression is filed
    ///   under, which changes with every edit), so these are exactly the entries whose bytes can
    ///   silently rot underneath them, and exactly the ones that have to be thrown away.
    /// * the **image URLs of the current decoration** (`md_cache.images`), resolved against the
    ///   document's own directory but *without* asking whether the file is still there. That covers
    ///   the two cases the cache alone misses: a picture below the fold (never drawn, so
    ///   `ensure_md_image` never made an entry for it — yet the layout already reserves a box sized
    ///   from the file on disk), and a picture that has just been **deleted** (no longer resolvable,
    ///   and its box has to collapse back to the `🖼 alt` label).
    ///
    /// Deliberately *not* in the set:
    /// * **synthetic keys** (`mermaid-fence://` / `math://`) — content-hashed, so they invalidate
    ///   themselves, and they are not filesystem paths at all;
    /// * **remote images** — their bytes sit in konoma's download cache, keyed by a hash of the URL
    ///   and outside the watched tree; `md_remote_failed` / `ensure_remote_md_fetch` own their
    ///   freshness.
    pub(super) fn md_image_source_paths(&self) -> std::collections::HashSet<PathBuf> {
        let mut out: std::collections::HashSet<PathBuf> = self
            .md_images
            .cache
            .keys()
            .filter(|k| !crate::preview::markdown::is_synthetic_md_url(&k.to_string_lossy()))
            .cloned()
            .collect();
        if let Some(cache) = self.md_cache.as_ref() {
            let base = cache.path.parent();
            for img in &cache.images {
                if crate::preview::markdown::is_synthetic_md_url(&img.url) {
                    continue;
                }
                if let Some(p) = md_image_local_path(&img.url, base) {
                    out.insert(p);
                }
            }
        }
        out
    }

    /// Drop the inline-image cache entries the filesystem event named — **only** those, so a
    /// document full of generated charts does not blink wholesale every time an agent rewrites one
    /// of them. An entry is keyed by path, so the key survives a rewrite while the decoded raster
    /// and the encoded protocol behind it do not; removing the entry is what makes the next render
    /// re-read the file (and re-measure it, which is how a picture regenerated at different
    /// proportions gets a correctly-sized box).
    ///
    /// An **empty** `changed` ("unknown, or `.git`-only" — see `preview_affected_by`) drops nothing
    /// on purpose. Nearly every empty burst is a `git` command touching repository internals, and
    /// blanking every picture on each of those would be a visible flicker bought for nothing.
    ///
    /// Known narrow window, unchanged in shape by this function: dropping an entry also drops the
    /// "a decode is already in flight for this path" marker (the entry's mere presence is that
    /// marker), so the next render kicks a second decode of the same path and `apply_md_image`
    /// accepts whichever finishes last. Two decodes of the same small file completing out of order
    /// would leave the older raster showing until the next event. The identical window has always
    /// existed on the file-switch path (leave a document and come back while its image is still
    /// decoding), and it self-heals on the next filesystem event or reopen, so it is recorded here
    /// rather than paid for with a generation counter through every `MdImageResult` producer.
    pub(super) fn drop_changed_md_images(&mut self, changed: &[PathBuf]) {
        for p in changed {
            // A synthetic key can never equal an absolute filesystem path, so this is a statement
            // of intent rather than a live guard: fences and equations are reclaimed by
            // `ensure_md_cache`'s own retain, never by a watcher event.
            if crate::preview::markdown::is_synthetic_md_url(&p.to_string_lossy()) {
                continue;
            }
            self.md_images.cache.remove(p);
        }
    }

    /// Test-only: how many entries currently sit in `md_image_cache` (private to `app`, so an e2e
    /// test outside the module needs an accessor to observe it). Used to confirm — by state, not by
    /// wall-clock timing — that a document with many distinct fence/math expressions does not have
    /// them all land in the cache (= all spawned) in a single pass.
    #[cfg(test)]
    pub fn md_image_cache_len(&self) -> usize {
        self.md_images.cache.len()
    }

    /// Wait time until the soonest inline-GIF frame change, across every animating cache entry (for
    /// the run loop's poll timeout — mirrors `App::gif_poll_timeout` for the full-screen GIF path).
    /// None when no inline GIF is currently animating.
    ///
    /// Gated on `Mode::Preview` for the same reason as `advance_md_gifs_if_due` (which this mirrors
    /// exactly): outside Preview there is nothing on screen to animate, so this must not keep the
    /// run loop waking up every ≤100ms — that showed up as measured `poll=Some(100ms)` while sitting
    /// on the tree, forcing a full redraw each time for a screen that never actually changes.
    pub fn md_gif_poll_timeout(&self) -> Option<std::time::Duration> {
        if !matches!(self.tab.mode, Mode::Preview) {
            return None;
        }
        self.md_images.gif_poll_timeout()
    }
}

/// Draw a retained SVG again at `max_px`, the way its origin drew it the first time
/// (`MdImgEntry::svg_trust`): konoma's own in this process, a file's by the supervised child
/// process relative to the file, anything else by the child process reading no file.
pub(crate) fn redraw_svg(
    svg: &[u8],
    trust: &SvgTrust,
    max_px: u32,
) -> Result<image::DynamicImage, String> {
    trust
        .draw(svg, max_px, &|| false)
        .ok_or_else(|| "rasterize failed".to_string())
}

#[cfg(test)]
#[path = "md_media_mutation_tests.rs"]
mod mutation_tests;

#[cfg(test)]
#[path = "md_media_slide_cap_tests.rs"]
mod slide_cap_tests;

/// The read scope of the plugin whose document is on screen, kept between frames (see
/// `App::with_scope_memo`), with the verdicts on single pictures judged under it.
pub(super) struct ScopeMemo {
    id: String,
    root: PathBuf,
    shown: PathBuf,
    pub(super) scope: konoma_plugin::ReadScope,
    verdicts: std::collections::HashMap<PathBuf, Result<PathBuf, konoma_plugin::ScopeError>>,
}

impl ScopeMemo {
    /// The scope's verdict on `p` (the real path, or why not), judged once per path.
    fn judge(&mut self, p: &Path) -> Result<PathBuf, konoma_plugin::ScopeError> {
        if let Some(v) = self.verdicts.get(p) {
            return v.clone();
        }
        let v = self.scope.resolve(p);
        self.verdicts.insert(p.to_path_buf(), v.clone());
        v
    }
}

#[cfg(test)]
mod tmux_detection_tests {
    use super::is_tmux_from_env;

    /// Baseline: nothing suggests tmux at all → no wrapping.
    #[test]
    fn no_signal_is_not_tmux() {
        assert!(!is_tmux_from_env(false, None, None));
        assert!(!is_tmux_from_env(false, Some("xterm-256color"), None));
    }

    /// Pre-3.2 tmux (no `TERM_PROGRAM`, `TERM` still `screen-*`): `$TMUX` alone already covered
    /// this and must keep doing so — this is the "harmless" mismatch direction (picker would see
    /// `Halfblocks` here anyway, so konoma's flag is never read), not something the fix should break.
    #[test]
    fn tmux_var_alone_is_tmux() {
        assert!(is_tmux_from_env(true, Some("screen-256color"), None));
    }

    /// The bug: ssh'd out of a tmux pane (or `sudo` inside one) forwards `TERM` but drops `$TMUX`.
    /// The picker's own detection (`TERM.starts_with("tmux")`) already gets this right; before the
    /// fix, konoma's `$TMUX`-only check did not.
    #[test]
    fn term_starts_with_tmux_without_tmux_var_is_tmux() {
        assert!(is_tmux_from_env(false, Some("tmux-256color"), None));
    }

    /// The other half of the picker's rule: `TERM_PROGRAM == "tmux"` with `$TMUX` unset.
    #[test]
    fn term_program_tmux_without_tmux_var_is_tmux() {
        assert!(is_tmux_from_env(false, None, Some("tmux")));
    }

    /// A `TERM` that merely contains "tmux" without *starting* with it must not match (mirrors the
    /// picker's `starts_with`, not a substring check).
    #[test]
    fn term_containing_but_not_starting_with_tmux_is_not_tmux() {
        assert!(!is_tmux_from_env(false, Some("xterm-tmux-ish"), None));
    }
}
