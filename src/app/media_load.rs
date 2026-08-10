use super::*;

impl App {
    /// Whether any inline Markdown image is still loading — a remote download in flight, a decode not yet
    /// finished, or an encode in flight (used by the run loop to keep ticking so results are applied
    /// promptly and the image appears without waiting for the next key press).
    pub fn md_images_loading(&self) -> bool {
        !self.md_remote_inflight.is_empty()
            || self
                .md_image_cache
                .values()
                .any(|e| (e.decoded.is_none() && !e.failed) || e.enc_inflight)
    }

    /// Drop tx and the image state on exit (to terminate the worker thread).
    pub fn detach_image_backend(&mut self) {
        self.clear_image();
        self.img_tx = None;
    }

    /// Decode a still image and place it in image_src (the ThreadProtocol is built asynchronously at render time).
    /// Still images (PNG/JPG) decode fast, so this stays synchronous. On decode failure / no backend, image_src=None.
    pub(super) fn load_image(&mut self, path: &Path) {
        if self.picker.is_none() || self.img_tx.is_none() {
            return; // no backend: the render side falls back to text
        }
        let Some(dyn_img) = crate::preview::image::decode_static(path) else {
            return;
        };
        self.set_static_image(dyn_img);
    }

    /// Common processing to set a still image (including SVG raster results / single-frame GIFs) as the display source.
    /// zoom/center are left untouched (enter_preview has the preceding clear_image set defaults, and tab restore has already
    /// overwritten them with the restored values; so that a late asynchronous result does not break the restored zoom/center).
    pub(super) fn set_static_image(&mut self, img: image::DynamicImage) {
        self.image_src = Some(std::sync::Arc::new(img));
        self.image_crop = None; // let the next render (prepare_image) build the protocol
                                // The "rebuild" signal for the kitty path. A path that swaps **only the pixels** rather than
                                // zooming (PDF page navigation / a video re-thumbnail) leaves both the crop and the display
                                // cell size unchanged, so `kitty_want` doesn't change and the old raster would linger.
                                // Invalidate want/shown here and stale-out any in-flight build (gen bump) = the swap always
                                // gets rebuilt.
                                // (An SVG reraster gets separately rebuilt on crop_rect change; fresh/reload is harmless even
                                // if this fires twice, via clear_image.)
        self.kitty_gen = self.kitty_gen.wrapping_add(1);
        self.kitty_want = None;
        self.kitty_shown = None;
    }

    /// Common processing to set all GIF frames into the display state (used from both the synchronous and asynchronous paths).
    /// zoom/center are left untouched for the same reason as set_static_image.
    pub(super) fn set_gif_frames(
        &mut self,
        frames: Vec<(image::DynamicImage, std::time::Duration)>,
    ) {
        self.gif_frames = frames;
        self.gif_idx = 0;
        self.gif_shown_at = None; // start timing on the first tick
        self.gif_protocol = None;
        self.gif_proto_key = None;
        self.image_crop = None;
    }

    /// Start loading image-type media according to the preview kind (from enter_preview / tab restore).
    /// Still images are synchronous, while **heavy SVG rasterization / GIF full-frame decode are offloaded to a separate thread**
    /// (to start display fast without blocking the UI thread). With no media_tx (tests, etc.), a synchronous fallback is used.
    pub(super) fn start_media_load(&mut self, kind: &PreviewKind, path: &Path) {
        // Record the baseline time for auto-reloading media (`reload_media_if_changed` uses it for
        // the mtime comparison).
        self.preview_media_mtime = file_mtime(path);
        match kind {
            PreviewKind::Image(_) if Self::looks_like_gif(path) => {
                self.spawn_or_sync_media(MediaJob::Gif(path.to_path_buf()))
            }
            // Still images (PNG/JPG etc.) decode fast, so this stays synchronous (the encode is
            // made async by a worker at render time).
            PreviewKind::Image(_) => self.load_image(path),
            PreviewKind::Svg(_) => {
                self.spawn_or_sync_media(MediaJob::Svg(path.to_path_buf(), self.cfg.ui.svg_max_px))
            }
            // `[external] video = false`: never spawn the job (never runs ffmpegthumbnailer/ffmpeg).
            // `image_src` stays None, so the render side falls back exactly like "tool not installed"
            // (VideoThumbUnavailable hint) — no new UI state needed.
            PreviewKind::Video(_) if self.cfg.external.video => {
                self.spawn_or_sync_media(MediaJob::Video(path.to_path_buf()))
            }
            PreviewKind::Video(_) => {}
            // PDF: always spawn the job — `hayro` (the primary renderer, `preview::pdf::render_page`)
            // is pure Rust and never touches an external process, so it must work regardless of
            // `[external] pdf`. That flag now only controls whether the job's *fallback*
            // (macOS's bundled qlmanage/sips, tried only if hayro itself fails on page 1) may run.
            PreviewKind::Pdf(_) => self.spawn_or_sync_media(MediaJob::Pdf(
                path.to_path_buf(),
                self.tab.pdf_page,
                self.cfg.external.pdf,
            )),
            // A standalone .mmd/.mermaid: in image mode, convert to SVG in pure Rust → rasterize
            // (on a separate thread). In text mode / with no backend, do nothing — the decorated
            // text path draws it instead (principle #3).
            PreviewKind::Mermaid(_) if self.mermaid_image_mode() => {
                self.spawn_or_sync_media(MediaJob::Mermaid(
                    path.to_path_buf(),
                    self.mermaid_px(),
                    self.cfg.ui.mermaid_theme.clone(),
                ))
            }
            // Full-screen fence display: re-fetch the fence body from the md by ordinal (count-guard,
            // the same shape as code copy).
            PreviewKind::MermaidFence(ord) => {
                if let Some(code) = self.mermaid_fence_code(path, *ord) {
                    self.spawn_or_sync_media(MediaJob::MermaidSrc(
                        code,
                        self.mermaid_px(),
                        self.cfg.ui.mermaid_theme.clone(),
                    ));
                }
            }
            // External command delegation. `detached` spawns-and-forgets right here, synchronously
            // (Command::spawn returns immediately — it doesn't wait for the child — so this can't
            // block the UI either); it is reached **only from `enter_preview`**, since
            // `kind_loads_media` deliberately excludes `detached=true` so a tab switch / an
            // unrelated fs-triggered reload never relaunches it (opening mpv again every time you
            // flip back to that tab). A non-detached command (image or text `render_as`) runs on
            // the worker like every other media kind above.
            PreviewKind::Command {
                template,
                render_as,
                detached,
                ..
            } => {
                let out = crate::preview::command::temp_out_path();
                let argv = crate::preview::command::build_argv(template, path, &out);
                if *detached {
                    if let Err(e) = crate::preview::command::run_detached(&argv) {
                        self.command_err = Some(e.to_string());
                    }
                } else {
                    let as_image = render_as.as_deref() == Some("image");
                    let uses_out = template.contains("{out}");
                    self.spawn_or_sync_media_gated(
                        MediaJob::Command {
                            argv,
                            out,
                            uses_out,
                            as_image,
                        },
                        // Only image mode needs a graphics backend — text mode shows through the
                        // ordinary windowed reader, same as Code/Text, and must work on any terminal.
                        as_image,
                    );
                }
            }
            _ => {}
        }
    }

    /// Whether mermaid renders as an image: config says so **and** the terminal has an image backend.
    /// Everything else (config `"text"`, no backend, render failure) degrades to the text diagram.
    pub fn mermaid_image_mode(&self) -> bool {
        self.cfg.ui.mermaid != "text" && self.picker.is_some()
    }

    /// Base raster target (max edge px) for mermaid diagrams — same knob as SVG previews.
    pub(super) fn mermaid_px(&self) -> u32 {
        self.cfg.ui.svg_max_px
    }

    /// Whether math ($…$ / $$…$$) renders as an image (`[ui] math` != "text" and a graphics backend).
    /// Off → math stays as raw LaTeX inline (the delimiters are left in the text).
    pub fn math_image_mode(&self) -> bool {
        self.cfg.ui.math != "text" && self.picker.is_some()
    }

    /// Raster target (max edge px) for a math equation. Generous so the once-rendered raster stays
    /// crisp when the encode worker fits it into the (usually small) reserved cell area.
    pub(super) fn math_px(&self) -> u32 {
        1024
    }

    /// Validated `[ui] mermaid_rows`: max rows of an inline diagram (0/invalid → default 24).
    fn mermaid_rows_cap(&self) -> u16 {
        match self.cfg.ui.mermaid_rows {
            0 => 24,
            v => v,
        }
    }

    /// Effective target rows for an inline diagram: the `mermaid_rows` cap, shrunk so the whole
    /// fence block (caption + diagram + bottom margin) fits the preview viewport — the initial
    /// view shows the entire diagram without scrolling (fit-to-view). Viewport 0 (no render yet /
    /// tests without a render pass) keeps the cap.
    pub(super) fn mermaid_fit_rows(&self) -> u16 {
        let cap = self.mermaid_rows_cap();
        if self.tab.preview_viewport == 0 {
            return cap;
        }
        cap.min(self.tab.preview_viewport.saturating_sub(2)).max(4)
    }

    /// The Nth ```mermaid fence body of the Markdown file at `md`, re-extracted fresh so an external
    /// edit between focusing and opening can't render a stale diagram (None = gone/shifted).
    pub(super) fn mermaid_fence_code(&self, md: &Path, ord: usize) -> Option<String> {
        let src = std::fs::read_to_string(md).ok()?;
        crate::preview::markdown::collect_mermaid_fences(&src)
            .into_iter()
            .nth(ord)
    }

    /// Run a media-load job on a separate thread (when media_tx is present). Otherwise run it synchronously.
    /// If there is no backend (picker), do nothing (the render side falls back).
    fn spawn_or_sync_media(&mut self, job: MediaJob) {
        self.spawn_or_sync_media_gated(job, true);
    }

    /// Same as `spawn_or_sync_media`, but the picker requirement is a parameter instead of always
    /// on. Every existing job kind (SVG/GIF/video/PDF/mermaid) only exists to feed the image
    /// pipeline, so needing a picker is correct for them — but a **text-mode** delegated command
    /// (`MediaJob::Command` with `as_image=false`) shows through the ordinary windowed text reader,
    /// exactly like Code/Text, and must run on any terminal, image backend or not.
    fn spawn_or_sync_media_gated(&mut self, job: MediaJob, requires_picker: bool) {
        if requires_picker && self.picker.is_none() {
            return; // terminal doesn't support it: the render side falls back to text/a message
        }
        let Some(tx) = self.media_tx.clone() else {
            // Synchronous fallback (tests / no channel attached).
            if let Some(payload) = job.run() {
                self.apply_payload(payload);
            }
            return;
        };
        self.media_gen = self.media_gen.wrapping_add(1);
        self.media_loading = true;
        let gen = self.media_gen;
        std::thread::spawn(move || {
            // Even if job.run() panics (a pathological input to the resvg raster / image decode),
            // don't kill the thread — always return a result: without one, `media_loading` would
            // stay stuck at true until the next preview transition, keeping the "Loading…" display
            // and the run loop's 16ms polling going forever (breaking the idle-0% guarantee).
            let payload = crate::preview::markdown::catch_silent(move || job.run()).flatten();
            let _ = tx.send(MediaResult { gen, payload });
        });
    }

    /// Apply a media-load result from another thread. Stale results (from after moving to another file) are discarded.
    /// Returns true if the state changes from applying / staleness judgment (the caller re-renders).
    pub fn apply_media(&mut self, result: MediaResult) -> bool {
        if result.gen != self.media_gen {
            return false; // stale: we've already moved on to another file
        }
        self.media_loading = false;
        // The current generation's result arrived = the reraster in-flight flag is resolved (clear
        // it even on a None failure so it doesn't get stuck).
        self.vector_reraster_inflight = false;
        match result.payload {
            Some(payload) => {
                self.apply_payload(payload);
                true
            }
            None => true, // failure: clear loading and let the render side fall back (raw XML/message)
        }
    }

    fn apply_payload(&mut self, payload: MediaPayload) {
        match payload {
            MediaPayload::Static(img) => self.set_static_image(img),
            MediaPayload::Gif(frames) => self.set_gif_frames(frames),
            MediaPayload::Vector { img, svg } => {
                use image::GenericImageView;
                // The first raster's arrival = this size becomes the logical (layout) size. Later
                // sharp rerasters swap only the pixel density (clear_image clears `logical` = reset
                // on switching to another file).
                if self.image_logical.is_none() {
                    self.image_logical = Some(img.dimensions());
                }
                self.vector_svg = Some(svg);
                self.set_static_image(img);
                // If the zoom advanced further while the job was running, fire off one more to
                // converge (returns immediately on the not-needed side if it's already sufficient /
                // at the 4096 cap).
                self.maybe_sharpen_vector();
            }
            MediaPayload::CommandText(p) => {
                // Replace, not reuse: a source-file reload or a tab-switch-triggered re-run both
                // regenerate rather than keep the previous artifact, so the old one (if any) is done for.
                self.clear_command_out();
                self.tab.command_out = Some(p);
                self.command_err = None;
                self.setup_windowed(); // opens the windowed reader onto the fresh command_out
            }
            MediaPayload::CommandFailed(msg) => {
                self.command_err = Some(msg);
            }
        }
    }

    /// Whether waiting on another thread's media load (used by the render side to decide on the "Loading…" display).
    pub fn is_media_loading(&self) -> bool {
        self.media_loading
    }

    /// Decide whether it is a GIF by the leading bytes (looks at the magic, not the extension).
    pub(super) fn looks_like_gif(path: &Path) -> bool {
        use std::io::Read;
        let Ok(mut f) = std::fs::File::open(path) else {
            return false;
        };
        let mut head = [0u8; 6];
        if f.read_exact(&mut head).is_err() {
            return false;
        }
        &head[..4] == b"GIF8" // GIF87a / GIF89a
    }

    /// Drive the GIF animation. If the current frame's display time has elapsed, advance to the next frame index.
    /// The actual re-encode is done by prepare_gif on the next render (detecting the gif_idx change).
    /// Returns true if advanced (the caller re-renders). Always false if not a GIF.
    pub fn advance_gif_if_due(&mut self) -> bool {
        if self.gif_frames.len() < 2 {
            return false;
        }
        let now = std::time::Instant::now();
        let Some(shown_at) = self.gif_shown_at else {
            // First tick: just start timing (the first frame is already shown).
            self.gif_shown_at = Some(now);
            return false;
        };
        let delay = self.gif_frames[self.gif_idx].1;
        if now.duration_since(shown_at) < delay {
            return false;
        }
        self.gif_idx = (self.gif_idx + 1) % self.gif_frames.len();
        self.gif_shown_at = Some(now);
        true
    }

    /// Whether a GIF animation is active (used for branching in the render path and footer/zoom checks).
    pub fn is_gif_active(&self) -> bool {
        self.gif_frames.len() >= 2
    }

    /// The render protocol of the current GIF frame synchronously encoded to the display size (referenced by the render side).
    pub fn gif_protocol(&self) -> Option<&Protocol> {
        self.gif_protocol.as_ref()
    }

    /// Call just before rendering (for GIF). Compute the display rectangle target and crop from the current (gif_idx, zoom, center, inner), and
    /// if the frame/crop has changed, **synchronously encode** the current frame and swap it into gif_protocol.
    /// Being synchronous, "render an unencoded protocol → empty" does not happen, and the frame switches atomically (no churn).
    /// The return value is the render-target target. None when there is no backend / size 0.
    pub fn prepare_gif(&mut self, inner: Rect) -> Option<Rect> {
        let picker = self.picker.as_ref()?;
        let scale = self.cfg.ui.image_render_scale;
        let src = &self.gif_frames.get(self.gif_idx)?.0;
        let ImageLayout {
            target,
            crop_rect,
            center,
            frac,
        } = image_layout(
            src,
            picker.font_size(),
            self.tab.image_zoom,
            self.tab.image_center,
            inner,
            scale,
            None, // GIF stays at its raw raster size (it isn't vector-derived)
        )?;
        let key = (self.gif_idx, crop_rect);
        // Re-encode only when the frame or the crop (zoom/pan/resize) changes.
        let built = if self.gif_proto_key != Some(key) {
            let (x0, y0, cw, ch) = crop_rect;
            let crop = src.crop_imm(x0, y0, cw, ch);
            let size = ratatui::layout::Size::new(target.width.max(1), target.height.max(1));
            // The src/picker borrows are fully consumed by this expression (the result is owned) →
            // self can be mutated afterward.
            // Since a GIF is synchronously encoded on the UI thread every frame, use the light and
            // smooth Triangle (bilinear) filter (nearest-neighbor `None` would make each animated
            // frame jagged; the highest-quality Lanczos3 is used for still images).
            Some(picker.new_protocol(crop, size, Resize::Scale(Some(FilterType::Triangle))))
        } else {
            None
        };
        if let Some(res) = built {
            match res {
                Ok(p) => {
                    self.gif_protocol = Some(p);
                    self.gif_proto_key = Some(key);
                }
                Err(_) => {
                    // Encode failure: the render side falls back to a message. Retry on the next frame.
                    self.gif_protocol = None;
                    self.gif_proto_key = None;
                }
            }
        }
        self.tab.image_center = center;
        self.image_vis_frac = frac;
        self.image_crop = Some(crop_rect);
        Some(target)
    }

    /// Wait time until the next frame while a GIF is playing (for the poll timeout). None when not playing.
    /// Clamped to [10ms, 100ms] for smoothness (100ms is the same as the normal idle-tick upper bound).
    pub fn gif_poll_timeout(&self) -> Option<std::time::Duration> {
        use std::time::Duration;
        if self.gif_frames.len() < 2 {
            return None;
        }
        let remaining = match self.gif_shown_at {
            None => Duration::ZERO, // not timing yet: run the next tick right away to start timing
            Some(t) => self.gif_frames[self.gif_idx]
                .1
                .checked_sub(t.elapsed())
                .unwrap_or(Duration::ZERO),
        };
        Some(remaining.clamp(Duration::from_millis(10), Duration::from_millis(100)))
    }

    /// Whether the current preview is a (renderable) image. Used to route image-only keys (zoom/pan).
    /// Still images are judged by image_src, GIFs by gif_frames (image_src is not used). SVGs are already rasterized.
    pub fn is_image_preview(&self) -> bool {
        (self.image_src.is_some() || self.is_gif_active())
            && matches!(
                self.tab.preview_kind,
                Some(
                    PreviewKind::Image(_)
                        | PreviewKind::Svg(_)
                        | PreviewKind::Video(_)
                        | PreviewKind::Pdf(_)
                        | PreviewKind::Mermaid(_)
                        | PreviewKind::MermaidFence(_)
                        // A delegated command counts only when it actually produced image_src —
                        // text-mode ones never do (they never call set_static_image), so the
                        // `image_src.is_some()` half of this && is what actually disambiguates them.
                        | PreviewKind::Command { .. }
                )
            )
    }

    /// Whether the current preview is a PDF (used to enable page-navigation keys / footer hints).
    pub fn is_pdf_preview(&self) -> bool {
        matches!(self.tab.preview_kind, Some(PreviewKind::Pdf(_)))
    }

    /// (current, total) page for the PDF preview, or None when not a PDF or the page count is
    /// unknown (page_count/hayro-syntax failed to parse the file at all — single-page fallback,
    /// navigation disabled). Used for the footer/status indicator.
    pub fn pdf_page_indicator(&self) -> Option<(u32, u32)> {
        if !self.is_pdf_preview() {
            return None;
        }
        let total = self.tab.pdf_pages?;
        Some((self.tab.pdf_page.min(total), total))
    }

    /// Whether PDF page navigation is possible (a known multi-page PDF). The page count comes from
    /// `page_count` (`hayro-syntax`, pure Rust, nothing installed needed), and rendering any given
    /// page is `hayro`'s job too — the only fallback left (macOS `qlmanage`/`sips`) is first-page-
    /// only, so navigating past page 1 depends on `hayro` alone, never on an installed tool.
    pub fn pdf_can_navigate(&self) -> bool {
        matches!(self.tab.pdf_pages, Some(n) if n > 1)
    }

    /// Go to the next PDF page (clamped to the last page). Re-rasterizes that page on demand (one at a time).
    pub fn pdf_next_page(&mut self) {
        self.pdf_goto(self.tab.pdf_page.saturating_add(1));
    }

    /// Go to the previous PDF page (clamped to page 1).
    pub fn pdf_prev_page(&mut self) {
        self.pdf_goto(self.tab.pdf_page.saturating_sub(1));
    }

    /// Jump to a 1-based page (clamped to [1, total]). No-op if not a navigable PDF or already there.
    /// Kicks off an off-thread rasterization of the new page and resets the view to fit (each page shows whole).
    fn pdf_goto(&mut self, page: u32) {
        if !self.pdf_can_navigate() {
            return;
        }
        let Some(total) = self.tab.pdf_pages else {
            return;
        };
        let page = page.clamp(1, total);
        if page == self.tab.pdf_page {
            return;
        }
        self.tab.pdf_page = page;
        // Reset to fit so the whole new page is visible (set_static_image doesn't touch zoom/center,
        // so reset it explicitly).
        self.tab.image_zoom = 1.0;
        self.tab.image_center = (0.5, 0.5);
        self.image_crop = None;
        if let Some(PreviewKind::Pdf(p)) = self.tab.preview_kind.clone() {
            // Keep showing the old page's image until the new one arrives (staleness judged by
            // media_gen; the spinner overlays on top).
            self.spawn_or_sync_media(MediaJob::Pdf(p, self.tab.pdf_page, self.cfg.external.pdf));
        }
    }

    /// Zoom (multiply the magnification by `factor`; clamped to 1.0–16.0). The actual crop is applied at render time.
    /// With no full-screen image showing, `+`/`-` instead zoom the **focused inline mermaid diagram**
    /// in place (the reserved area in the document never changes — the zoom crops within it).
    pub fn image_zoom_by(&mut self, factor: f64) {
        if self.image_src.is_none() && !self.is_gif_active() {
            if self.focused_mermaid_ordinal().is_some() {
                self.tab.fence_zoom = (self.tab.fence_zoom * factor).clamp(1.0, 16.0);
            }
            return;
        }
        self.tab.image_zoom = (self.tab.image_zoom * factor).clamp(1.0, 16.0);
        self.maybe_sharpen_vector();
    }

    /// Sharp zoom for vector-backed previews (SVG / mermaid): when the zoom outgrows the current
    /// raster's density, re-rasterize the retained SVG at the needed max-edge px on a worker thread
    /// and swap it in on arrival (latest-wins via media_gen). The pixel zoom shows instantly in the
    /// meantime, so this behaves like a map app: briefly soft, then crisp. The geometry never moves
    /// because `image_layout` works in the logical size (`image_logical`), not the raster size.
    /// Memory stays bounded by the rasterizer's HARD_MAX (4096px side ≈ 64 MiB RGBA).
    pub(super) fn maybe_sharpen_vector(&mut self) {
        use image::GenericImageView;
        // Only one at a time: don't spawn multiple copies of the same reraster on repeated `+` key
        // presses (the needed density keeps growing before the job finishes) — one job costs ~a few
        // hundred ms and ~128MiB transiently. On arrival (apply_media → apply_payload) this function
        // runs again and converges to the zoom level current at that point.
        if self.vector_reraster_inflight {
            return;
        }
        let (Some(svg), Some(src), Some((lw, lh))) =
            (&self.vector_svg, &self.image_src, self.image_logical)
        else {
            return;
        };
        let cur_side = src.dimensions().0.max(src.dimensions().1);
        let want = ((lw.max(lh) as f64) * self.tab.image_zoom).ceil() as u32;
        // Do nothing if the current raster is already sufficient (+12% margin) or already at the cap (4096).
        if want <= cur_side + cur_side / 8 || cur_side >= 4096 {
            return;
        }
        let base = self.tab.preview_path.clone().unwrap_or_default();
        // Don't spawn for the synchronous fallback (no media_tx = tests) — there's no room for
        // duplication there, so don't set the flag either.
        if self.media_tx.is_some() {
            self.vector_reraster_inflight = true;
        }
        self.spawn_or_sync_media(MediaJob::SvgReraster(svg.clone(), base, want));
    }

    /// Reset to 1x (fit). Zoom=1 and recenter. Applies to the focused inline diagram when no
    /// full-screen image is showing (same dual role as `image_zoom_by`).
    pub fn image_zoom_reset(&mut self) {
        if self.image_src.is_none() && !self.is_gif_active() {
            if self.focused_mermaid_ordinal().is_some() {
                self.tab.fence_zoom = 1.0;
                self.tab.fence_center = (0.5, 0.5);
            }
            return;
        }
        self.tab.image_zoom = 1.0;
        self.tab.image_center = (0.5, 0.5);
    }

    /// Current in-place zoom of the focused inline diagram (renderer/footer cue).
    pub fn fence_zoom_level(&self) -> f64 {
        self.tab.fence_zoom
    }

    /// Whether the focused inline diagram's reserved block is fully visible in the preview viewport
    /// (mirrors the renderer's in-place-zoom condition: `row_off == 0 && vis_rows >= rows`).
    /// Viewport 0 (no render pass yet — tests) counts as visible.
    fn focused_fence_fully_visible(&self) -> bool {
        let Some(ord) = self.focused_mermaid_ordinal() else {
            return false;
        };
        let Some((line, rows)) = self.mermaid_placement(ord) else {
            return false;
        };
        let vh = self.tab.preview_viewport as usize;
        if vh == 0 {
            return true;
        }
        let (top, _) = self.md_visual_span(line);
        let scroll = self.tab.preview_scroll as usize;
        top >= scroll && top + rows as usize <= scroll + vh
    }

    /// hjkl/arrows while an inline diagram is focused **and zoomed**: pan the diagram instead of
    /// scrolling the document; `0` fits (zoom reset — the image-view key). Returns true when
    /// consumed. At 1x nothing is consumed, so all keys scroll/navigate the document as usual.
    /// The diagram must also be on screen: after zooming and paging away (Ctrl-f/G), hjkl/j/k
    /// would otherwise pan an invisible diagram and the keys would appear dead.
    pub fn fence_pan_motion(&mut self, m: crate::keymap::Motion) -> bool {
        use crate::keymap::Motion as M;
        if self.tab.fence_zoom <= 1.001 || !self.focused_fence_fully_visible() {
            return false;
        }
        // 0 = fit (the same key as the full-screen image view). Consumed only while zoomed (at 1x it
        // stays "go to line home" as before).
        if matches!(m, M::LineHome) {
            self.tab.fence_zoom = 1.0;
            self.tab.fence_center = (0.5, 0.5);
            return true;
        }
        let (dx, dy) = match m {
            M::Left => (-1.0, 0.0),
            M::Right => (1.0, 0.0),
            M::Up => (0.0, -1.0),
            M::Down => (0.0, 1.0),
            _ => return false,
        };
        // One step = 1/4 of the visible window (the same feel as full-screen image pan). Edge
        // clamping is done in the render side's crop calculation.
        let f = 1.0 / self.tab.fence_zoom;
        self.tab.fence_center.0 = (self.tab.fence_center.0 + dx * f * 0.25).clamp(0.0, 1.0);
        self.tab.fence_center.1 = (self.tab.fence_center.1 + dy * f * 0.25).clamp(0.0, 1.0);
        true
    }

    /// Pan. dx/dy are directions (-1/0/+1). Only clipped axes move the center, scaled by the visible fraction.
    /// Clamping is done at render time (prepare_image), looking at the visible fraction.
    pub fn image_pan(&mut self, dx: f64, dy: f64) {
        if self.image_src.is_none() && !self.is_gif_active() {
            return;
        }
        let (fw, fh) = self.image_vis_frac;
        // About 25% of the visible window per call. Moving an axis that isn't clipped (frac>=1) is
        // clamped at render time anyway.
        self.tab.image_center.0 += dx * 0.25 * fw;
        self.tab.image_center.1 += dy * 0.25 * fh;
    }

    /// Call just before rendering. From the current (zoom, center, display area inner), compute the image's display rectangle target
    /// (centered; z=1=fit, grows when zooming and clips once it exceeds the viewport) and the source-image crop of
    /// the visible portion; rebuild the protocol if the crop changed. The return value is the render-target target.
    /// Realizes "fit the render area to the image size (fit) → grow when zooming → clip + pan once exceeded".
    pub fn prepare_image(&mut self, inner: Rect) -> Option<Rect> {
        let src = self.image_src.as_ref()?;
        let picker = self.picker.as_ref()?;
        let scale = self.cfg.ui.image_render_scale;
        let ImageLayout {
            target,
            crop_rect,
            center,
            frac,
        } = image_layout(
            src,
            picker.font_size(),
            self.tab.image_zoom,
            self.tab.image_center,
            inner,
            scale,
            self.image_logical,
        )?;
        let crop_changed = self.image_crop != Some(crop_rect);
        if self.use_kitty {
            // konoma-native compressed transmit. Request a rebuild only when the wanted geometry —
            // the crop (zoom/pan) or the display cell size (a terminal resize at fit) — changes;
            // otherwise the current/in-flight image already targets it, so a static image just
            // re-emits cheap placeholders every frame. The cell-size part matters because at fit zoom
            // a resize changes the area without the crop, and ratatui-image re-encodes on area change
            // internally, so konoma's path must too.
            let want = (crop_rect, target.width, target.height);
            if self.kitty_want != Some(want) {
                self.kitty_want = Some(want);
                self.request_kitty_build(crop_rect, target, picker.font_size());
            }
        } else {
            // ratatui-image path (sixel/iterm2/halfblocks, or kitty when disabled): rebuild the
            // ThreadProtocol only on crop change to avoid a per-frame re-encode.
            let new_tp = if crop_changed {
                let (x0, y0, cw, ch) = crop_rect;
                let crop = src.crop_imm(x0, y0, cw, ch);
                let proto = picker.new_resize_protocol(crop);
                Some(ThreadProtocol::new(
                    self.img_tx.as_ref()?.clone(),
                    Some(proto),
                ))
            } else {
                None
            };
            if let Some(tp) = new_tp {
                self.image = Some(tp);
            }
        }
        self.tab.image_center = center;
        self.image_vis_frac = frac;
        self.image_crop = Some(crop_rect);
        Some(target)
    }

    /// Build the compressed kitty image for the given crop/target. The **first** build after opening
    /// a file (or after a failure left `kitty_image` None) runs synchronously so the image appears
    /// without a blank frame. Subsequent builds (zoom/pan/resize) run on a worker thread so rapid
    /// input does not hitch — the previous image keeps showing until the new one arrives (`apply_kitty`).
    /// With no `kitty_tx` (tests), everything runs synchronously.
    fn request_kitty_build(
        &mut self,
        crop_rect: (u32, u32, u32, u32),
        target: Rect,
        font: ratatui_image::FontSize,
    ) {
        let Some(src) = self.image_src.clone() else {
            return; // no source yet → render falls back
        };
        let (cols, rows) = (target.width, target.height);
        let is_tmux = self.kitty_is_tmux;
        // Every build bumps the generation so a stale async result is discarded by `apply_kitty`.
        self.kitty_gen = self.kitty_gen.wrapping_add(1);

        // Synchronous: the very first build (nothing to show yet) or when there is no worker channel.
        if self.kitty_image.is_none() || self.kitty_tx.is_none() {
            self.kitty_image = crate::preview::kitty::build_from_source(
                &src, crop_rect, cols, rows, font, is_tmux,
            );
            self.kitty_shown = self.kitty_want; // now showing the wanted geometry
            return;
        }

        // Async: keep the previous image visible; the newest spawn's result wins.
        let tx = self.kitty_tx.clone().unwrap();
        let gen = self.kitty_gen;
        std::thread::spawn(move || {
            // A pathological image must not kill the worker (else nothing is ever sent and the
            // previous image lingers); catch and report a failed build instead.
            let image = crate::preview::markdown::catch_silent(move || {
                crate::preview::kitty::build_from_source(&src, crop_rect, cols, rows, font, is_tmux)
            })
            .flatten();
            let _ = tx.send(KittyResult { gen, image });
        });
    }

    /// Apply a completed async kitty build. Stale results (superseded by a newer zoom/pan) and failed
    /// builds are dropped, keeping whatever image is currently shown rather than blanking it.
    pub fn apply_kitty(&mut self, result: KittyResult) -> bool {
        if result.gen != self.kitty_gen {
            return false; // superseded by a newer geometry
        }
        // Either way the newest generation has now settled — mark the wanted geometry as shown so
        // `kitty_build_pending()` clears. Otherwise a failed build (None) would leave it stuck true,
        // spinning the run loop at 16 ms forever (until the next geometry change). On failure the
        // previous image simply stays; on success we swap in the new one.
        self.kitty_shown = self.kitty_want;
        match result.image {
            Some(ki) => {
                self.kitty_image = Some(ki);
                true
            }
            None => false,
        }
    }

    /// Whether an async kitty build is in flight: the wanted geometry differs from what is shown.
    /// Covers pan (same cell size, different crop). The run loop polls faster while this is true so
    /// the result is picked up promptly.
    pub fn kitty_build_pending(&self) -> bool {
        self.use_kitty && self.kitty_want != self.kitty_shown
    }

    /// Whether still images are drawn via konoma's own compressed kitty transmit (kitty terminal).
    pub fn uses_kitty_image(&self) -> bool {
        self.use_kitty
    }

    /// The prepared compressed kitty image for the current crop, if built.
    pub fn kitty_image_ref(&self) -> Option<&crate::preview::kitty::KittyImage> {
        self.kitty_image.as_ref()
    }
}
