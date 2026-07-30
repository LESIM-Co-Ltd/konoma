use super::*;

impl App {
    /// Attach the image backend (terminal Picker and the offload tx) at startup.
    pub fn attach_image_backend(&mut self, picker: Picker, tx: UnboundedSender<ResizeRequest>) {
        // Use konoma's own compressed transmit only on a kitty-graphics terminal; other protocols
        // (sixel/iterm2/halfblocks) keep the ratatui-image path. tmux is detected via $TMUX (the
        // picker's own is_tmux is private), matching how the escapes must be passthrough-wrapped.
        self.use_kitty = matches!(
            picker.protocol_type(),
            ratatui_image::picker::ProtocolType::Kitty
        );
        self.kitty_is_tmux = std::env::var_os("TMUX").is_some();
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
        self.md_enc_tx = Some(tx);
    }

    /// Apply a completed background decode of an inline Markdown image. Returns whether to redraw.
    pub fn apply_md_image(&mut self, res: MdImageResult) -> bool {
        // The entry is always pre-placed at kick time (ensure_mermaid_fence_render /
        // ensure_md_image). Missing = a stale result (already evicted from the cache on a file
        // switch / already pruned), so drop it instead of reviving it (with or_default, the old
        // diagram's raster would be re-inserted and linger until the next enter_preview, and on
        // top of that the md_cache invalidation below would needlessly do a full rebuild once for
        // **an unrelated current document**).
        if !self.md_image_cache.contains_key(&res.path) {
            return false;
        }
        // For a fence diagram / math expression (synthetic key), the dimensions are decided here
        // for the first time = the loading row needs to be rebuilt into the real placement, so
        // invalidate the decoration cache (same convention as remote images' apply_remote_fetch).
        // **Excludes a sharpening re-raster**: that's just swapping pixel density, and since the
        // layout (reserved cell count) is pinned by layout_px, decoration is not rebuilt (= the
        // display size on the page stays fixed — a user requirement).
        let url_str = res.path.to_string_lossy().to_string();
        let is_math = crate::preview::markdown::is_math_url(&url_str);
        if !res.reraster && (crate::preview::markdown::is_mermaid_fence_url(&url_str) || is_math) {
            self.md_cache = None;
        }
        let entry = self.md_image_cache.entry(res.path).or_default();
        if res.reraster {
            entry.reraster_inflight = false;
        }
        match res.image {
            Ok(img) => {
                use image::GenericImageView;
                if entry.layout_px.is_none() {
                    // For math, load the SVG's **intrinsic size (in units)** into layout_px
                    // (rather than raster px): rows are derived from the em height and columns
                    // from the intrinsic aspect, sizing it to balance against the text
                    // (math_cells). Fall back to the raster dimensions only when the intrinsic
                    // size can't be obtained. Fences / regular images work as before.
                    entry.layout_px = if is_math {
                        res.svg
                            .as_deref()
                            .and_then(|d| crate::preview::svg::intrinsic_size_bytes(d))
                            .or_else(|| Some(img.dimensions()))
                    } else {
                        Some(img.dimensions())
                    };
                }
                // Animated GIF: keep all frames (as cheap-to-clone Arcs from here on) and start the
                // cycle at frame 0; `advance_md_gifs_if_due` swaps `decoded` to the current frame as
                // playback advances. `res.frames` only arrives on the initial decode — GIFs never go
                // through the sharpening re-raster path (that's for vector sources), so this can't
                // race a later reraster result and clobber mid-playback state.
                if let Some(frames) = res.frames {
                    entry.frames = frames
                        .into_iter()
                        .map(|(im, d)| (Arc::new(im), d))
                        .collect();
                    entry.idx = 0;
                    entry.shown_at = None;
                    entry.decoded = entry.frames.first().map(|(im, _)| im.clone());
                } else {
                    entry.decoded = Some(Arc::new(img));
                }
                entry.failed = false;
                if let Some(svg) = res.svg {
                    entry.svg = Some(svg);
                }
                if res.reraster {
                    // Trigger a re-encode with the high-density raster: invalidate only the key,
                    // and keep the old protocol displayed until the new encode arrives (clearing
                    // it would leave a momentary blank).
                    entry.proto_size = None;
                    entry.clip_key = None;
                    entry.zoom_key = None;
                }
            }
            // A re-raster failure leaves the current raster in place (the display stays alive).
            // Only an initial failure degrades to text.
            Err(_) if res.reraster => {}
            Err(_) => entry.failed = true,
        }
        true
    }

    /// Drive the **in-place zoom** of the focused inline mermaid diagram: clamp the pan center,
    /// kick a sharpening re-raster when the zoom outgrows the raster density (the layout size stays
    /// fixed via `layout_px`), and request an encode of the current crop into the same (cols, rows)
    /// cell area. All heavy work runs on worker threads; at most one of each is in flight.
    pub fn ensure_md_fence_zoom(&mut self, url: &str, cols: u16, rows: u16) {
        use image::GenericImageView;
        let key_path = PathBuf::from(url);
        let zoom = self.tab.fence_zoom;
        let font = self.picker.as_ref().map(|p| p.font_size());
        let enc_tx = self.md_enc_tx.clone();
        let Some(entry) = self.md_image_cache.get_mut(&key_path) else {
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
        let Some(entry) = self.md_image_cache.get_mut(&key_path) else {
            return;
        };
        let zkey = (cols, rows, crop);
        if entry.zoom_key == Some(zkey) || entry.enc_inflight {
            return;
        }
        let Some(tx) = enc_tx else { return };
        entry.enc_inflight = true;
        let _ = tx.send(MdEncodeRequest {
            path: key_path,
            key: MdEncodeKey::Zoom { cols, rows, crop },
            image: img,
            crop: Some(crop),
            cols,
            rows,
        });
    }

    /// The protocol for the focused inline diagram's current zoom, if encoded for this (cols, rows).
    /// While a fresh crop is still encoding, the previous zoom crop (or the unzoomed full protocol)
    /// stays visible instead of blinking out.
    pub fn md_fence_zoom_proto(&self, url: &str, cols: u16, rows: u16) -> Option<&Protocol> {
        let entry = self.md_image_cache.get(&PathBuf::from(url))?;
        match entry.zoom_key {
            Some((c, r, _)) if (c, r) == (cols, rows) => {
                entry.zoom_protocol.as_ref().or(entry.protocol.as_ref())
            }
            _ => entry.protocol.as_ref(),
        }
    }

    /// Kick a sharpening re-raster of a fence diagram when `needed_px` exceeds the current raster
    /// density (shared by the unzoomed density follow-up and the in-place zoom). Returns what
    /// happened so the synchronous test fallback can re-run with the fresh raster.
    fn fence_sharpen_if_needed(&mut self, key_path: &PathBuf, needed_px: u32) -> FenceSharpen {
        use image::GenericImageView;
        let img_tx = self.md_img_tx.clone();
        let Some(entry) = self.md_image_cache.get_mut(key_path) else {
            return FenceSharpen::NotNeeded;
        };
        let (Some(img), Some(svg)) = (entry.decoded.as_ref(), entry.svg.clone()) else {
            return FenceSharpen::NotNeeded;
        };
        let (sw, sh) = img.dimensions();
        let cur = sw.max(sh);
        if needed_px <= cur + cur / 8 || cur >= 4096 || entry.reraster_inflight {
            return FenceSharpen::NotNeeded;
        }
        entry.reraster_inflight = true;
        let target = needed_px.min(4096);
        let kp = key_path.clone();
        let job = move || {
            let image =
                crate::preview::svg::rasterize_bytes(&svg, Path::new("mermaid.svg"), target)
                    .ok_or_else(|| "rasterize failed".to_string());
            MdImageResult {
                path: kp,
                image,
                svg: None,
                reraster: true,
                frames: None,
            }
        };
        if let Some(tx) = img_tx {
            std::thread::spawn(move || {
                let _ = tx.send(job());
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
        let needed =
            ((cols as f64 * f.width as f64).max(rows as f64 * f.height as f64)).ceil() as u32;
        let _ = self.fence_sharpen_if_needed(&PathBuf::from(url), needed);
    }

    /// Frame bookkeeping for the inline-image overlay (called by `ui::render`): reset at frame
    /// start, recorded by the overlay, compared at frame end. A change after a drawn state marks
    /// the frame "moved" so the run loop clears the terminal once (placeholder-orphan sweep).
    pub fn begin_md_overlay_frame(&mut self) {
        self.md_overlay_seen = None;
    }

    /// Record the overlay signature for this frame (urls + screen rects of drawn inline images).
    pub fn note_md_overlay(&mut self, sig: u64) {
        self.md_overlay_seen = Some(sig);
    }

    /// Compare this frame's overlay against the previous frame; latch "moved" on any change away
    /// from a previously drawn state (position shift, or the images left the screen entirely).
    pub fn finish_md_overlay_frame(&mut self) {
        if self.md_overlay_last != self.md_overlay_seen {
            if self.md_overlay_last.is_some() {
                self.md_overlay_moved = true;
            }
            self.md_overlay_last = self.md_overlay_seen;
        }
    }

    /// Whether the run loop should full-clear once to sweep orphaned placeholder rows (resets).
    pub fn take_md_overlay_moved(&mut self) -> bool {
        std::mem::take(&mut self.md_overlay_moved)
    }

    /// Ensure a background mermaid render is in flight (or cached) for one ```mermaid fence.
    /// Keyed by content hash, so an edited fence renders fresh while unchanged fences reuse their
    /// raster. With no loader tx (tests), renders synchronously so behavior stays observable.
    /// Returns true when the render completed **synchronously** (no loader tx = tests): the caller
    /// must rebuild its just-built decoration, which still shows the loading line.
    pub(super) fn ensure_mermaid_fence_render(&mut self, code: String) -> bool {
        let key = PathBuf::from(crate::preview::markdown::mermaid_fence_url(&code));
        if self.md_image_cache.contains_key(&key) {
            return false;
        }
        self.md_image_cache
            .insert(key.clone(), MdImgEntry::default());
        let max_px = self.mermaid_px();
        let theme = self.cfg.ui.mermaid_theme.clone();
        let render = move || -> (Result<image::DynamicImage, String>, Option<std::sync::Arc<Vec<u8>>>) {
            let Some(svg) = crate::preview::markdown::mermaid_to_svg(&code, &theme) else {
                return (Err("mermaid render failed".to_string()), None);
            };
            let data = std::sync::Arc::new(svg.into_bytes());
            let img =
                crate::preview::svg::rasterize_bytes(&data, Path::new("mermaid.svg"), max_px)
                    .ok_or_else(|| "rasterize failed".to_string());
            (img, Some(data))
        };
        if let Some(tx) = self.md_img_tx.clone() {
            std::thread::spawn(move || {
                // Even if render() (rasterize_bytes = resvg) panics, don't kill the thread — always
                // return a result: otherwise the entry stays stuck at decoded=None && !failed and
                // busy latches true forever.
                let (image, svg) = crate::preview::markdown::catch_silent(render)
                    .unwrap_or_else(|| (Err("mermaid render panicked".to_string()), None));
                let _ = tx.send(MdImageResult {
                    path: key,
                    image,
                    svg,
                    reraster: false,
                    frames: None,
                });
            });
            false
        } else {
            let (image, svg) = render();
            self.apply_md_image(MdImageResult {
                path: key,
                image,
                svg,
                reraster: false,
                frames: None,
            });
            true
        }
    }

    /// Ensure a background render is in flight (or cached) for one math expression, keyed by latex +
    /// display so an edited equation renders fresh while unchanged ones reuse their raster. Mirrors
    /// `ensure_mermaid_fence_render`; the SVG is kept so `apply_md_image` can record its intrinsic (em)
    /// size for layout. Returns true on a synchronous completion (no loader tx = tests).
    pub(super) fn ensure_math_render(&mut self, latex: String, display: bool) -> bool {
        let key = PathBuf::from(crate::preview::markdown::math_url(&latex, display));
        if self.md_image_cache.contains_key(&key) {
            return false;
        }
        self.md_image_cache
            .insert(key.clone(), MdImgEntry::default());
        let max_px = self.math_px();
        // Move the glyph color (already sanitized) into the worker. RaTeX's default is pure black,
        // which is invisible on a dark terminal, so paint it a color that stands out against the
        // terminal background (config `[ui] math_color`, default a light gray).
        let color = self.cfg.ui.math_color().to_string();
        let render =
            move || -> (Result<image::DynamicImage, String>, Option<std::sync::Arc<Vec<u8>>>) {
                let Some(svg) = crate::preview::math::latex_to_svg(&latex, display, &color) else {
                    return (Err("math render failed".to_string()), None);
                };
                let data = std::sync::Arc::new(svg.into_bytes());
                let img = crate::preview::svg::rasterize_bytes(&data, Path::new("math.svg"), max_px)
                    .ok_or_else(|| "rasterize failed".to_string());
                (img, Some(data))
            };
        if let Some(tx) = self.md_img_tx.clone() {
            std::thread::spawn(move || {
                let (image, svg) = crate::preview::markdown::catch_silent(render)
                    .unwrap_or_else(|| (Err("math render panicked".to_string()), None));
                let _ = tx.send(MdImageResult {
                    path: key,
                    image,
                    svg,
                    reraster: false,
                    frames: None,
                });
            });
            false
        } else {
            let (image, svg) = render();
            self.apply_md_image(MdImageResult {
                path: key,
                image,
                svg,
                reraster: false,
                frames: None,
            });
            true
        }
    }

    /// Apply a completed background encode: store the protocol in its full or clip slot. Returns redraw.
    /// A failed encode (`protocol: None`) records the attempted key **without** touching the stored
    /// protocols — the last good state stays visible and the same request is not retried every frame.
    /// Only when nothing was ever encodable (no full protocol at all) does the entry degrade to the
    /// text fallback (principle #3).
    pub fn apply_md_encode(&mut self, res: MdEncodeResult) -> bool {
        let Some(entry) = self.md_image_cache.get_mut(&res.path) else {
            return false;
        };
        entry.enc_inflight = false;
        let mut degrade = false;
        match (res.key, res.protocol) {
            (MdEncodeKey::Full { cols, rows }, Some(p)) => {
                entry.protocol = Some(p);
                entry.proto_size = Some((cols, rows));
            }
            (MdEncodeKey::Full { cols, rows }, None) => {
                entry.proto_size = Some((cols, rows));
                if entry.protocol.is_none() {
                    entry.failed = true;
                    degrade = true;
                }
            }
            (
                MdEncodeKey::Clip {
                    cols,
                    full_rows,
                    row_off,
                    vis_rows,
                },
                p,
            ) => {
                if let Some(p) = p {
                    entry.clip_protocol = Some(p);
                }
                entry.clip_key = Some((cols, full_rows, row_off, vis_rows));
            }
            (MdEncodeKey::Zoom { cols, rows, crop }, p) => {
                if let Some(p) = p {
                    entry.zoom_protocol = Some(p);
                }
                entry.zoom_key = Some((cols, rows, crop));
            }
        }
        if degrade {
            // An image that has newly degraded to failed gets re-laid-out as a text row (never
            // left as an invisible blank).
            self.md_cache = None;
        }
        true
    }

    /// Attach the sender that reports background remote-image download completions to the run loop.
    pub fn attach_remote_md_loader(&mut self, tx: std::sync::mpsc::Sender<RemoteFetch>) {
        self.md_remote_tx = Some(tx);
    }

    /// Apply a completed remote-image download. On success the file is now cached, so drop the
    /// decoration cache to re-lay the image out inline; on failure remember the URL so it is not
    /// retried and shows a text placeholder instead. Returns whether to redraw.
    pub fn apply_remote_fetch(&mut self, res: RemoteFetch) -> bool {
        self.md_remote_inflight.remove(&res.url);
        if !res.ok {
            self.md_remote_failed.insert(res.url);
        }
        // Re-decorate so a now-cached image is laid out (or a failed one degrades to text).
        self.md_cache = None;
        true
    }

    /// Ensure a background download is in flight for the remote image `url` (deduplicated). Skips URLs
    /// that are already cached, already downloading, or known to have failed. The download runs off the
    /// UI thread (principle #4) via `curl`; on completion it reports through `md_remote_tx`. Returns
    /// true on a *synchronous* failure (mirrors `ensure_mermaid_fence_render`/`ensure_math_render`'s
    /// "no loader tx = tests" convention) so the caller (`ensure_md_cache`) can resync its
    /// already-built `decorated` before storing it — see the `remote_images = false` branch below.
    pub(super) fn ensure_remote_md_fetch(&mut self, url: &str) -> bool {
        if !crate::preview::markdown::is_remote_image_url(url) {
            return false;
        }
        if !self.cfg.external.remote_images {
            // `[external] remote_images = false`: never call out to `curl`. Mark it failed right away
            // (instead of leaving it unrecorded) so the renderer degrades to the text placeholder
            // instead of showing `ImageSlot::Loading` forever. Marking it failed alone only affects a
            // *future* decoration build though — `ensure_md_cache` already built `decorated` (with a
            // Loading slot, since `md_remote_failed` didn't contain `url` yet) before calling this, and
            // unconditionally stores that `decorated` into `md_cache` afterwards regardless of what we
            // do to `self.md_cache` here. Since remote fetches never run, nothing else would ever
            // invalidate the cache to trigger a later rebuild (unlike a real download completing via
            // `apply_remote_fetch`), so the Loading placeholder would otherwise stick around forever.
            // Returning true (only the first time — `insert()` reports whether it was new) tells the
            // caller to rebuild `decorated` right now, in this same pass, using the now-failed URL.
            return self.md_remote_failed.insert(url.to_string());
        }
        // Already downloaded (cache file exists), already failed, or already downloading → nothing to do.
        if resolve_md_image_path(url, None).is_some()
            || self.md_remote_failed.contains(url)
            || self.md_remote_inflight.contains(url)
        {
            return false;
        }
        let (Some(tx), Some(dest)) = (self.md_remote_tx.clone(), md_remote_cache_path(url)) else {
            return false;
        };
        self.md_remote_inflight.insert(url.to_string());
        let u = url.to_string();
        std::thread::spawn(move || {
            // Don't leave the entry stuck in md_remote_inflight even if the download panics
            // (leaving it in would latch busy): always report a panic as a fetch failure (ok=false).
            let ok = crate::preview::markdown::catch_silent(|| fetch_remote_image(&u, &dest))
                .unwrap_or(false);
            let _ = tx.send(RemoteFetch { url: u, ok });
        });
        false
    }

    /// Ensure the inline image for `url` is decoding in the background and that the protocol for the
    /// currently-visible portion (whole image, or a cropped band when partially scrolled) is encoding on
    /// the worker thread. Called from the renderer for each visible inline image. Both decoding and
    /// encoding are off-thread (principle #4) so this never blocks the UI; the protocol appears a frame
    /// or two later. At most one encode is in flight per image, so scrolling never queues a backlog.
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
            p
        };
        // Kick off a one-time background decode.
        if !self.md_image_cache.contains_key(&path) {
            // A synthetic key (fence diagram / math expression) cannot be built here (the original
            // code/latex is needed). If a placement exists it should already be cached, so this is
            // a defensive guard for the case it never arrives (the next re-decoration has
            // ensure_mermaid_fence_render / ensure_math_render rebuild it). Never let `math://` be
            // decoded as a real file.
            if crate::preview::markdown::is_synthetic_md_url(url) {
                return;
            }
            self.md_image_cache
                .insert(path.clone(), MdImgEntry::default());
            if let Some(tx) = self.md_img_tx.clone() {
                let p = path.clone();
                let svg_max_px = self.cfg.ui.svg_max_px;
                std::thread::spawn(move || {
                    // Sniff the format from content (remote-cache files have no extension); rasterize SVG.
                    // Animated GIF: decode all frames so the inline image cycles the same way the
                    // full-screen preview does (App::advance_gif_if_due) — a smaller budget than the
                    // full-screen path bounds memory when a document embeds several GIFs at once.
                    // Anything that doesn't yield ≥2 frames (single-frame GIF, corrupt file, non-GIF)
                    // falls through unchanged to the normal still-image decode.
                    // Catch a panic (pathological image/SVG) too and always return a result
                    // (not returning would latch busy).
                    let (still, frames) = crate::preview::markdown::catch_silent(|| {
                        if App::looks_like_gif(&p) {
                            if let Some(frames) = crate::preview::image::decode_gif_inline(&p) {
                                let first = frames[0].0.clone();
                                return (Some(first), Some(frames));
                            }
                        }
                        (md_decode_image(&p, svg_max_px), None)
                    })
                    .unwrap_or((None, None));
                    let image = still.ok_or_else(|| "decode failed".to_string());
                    let _ = tx.send(MdImageResult {
                        path: p,
                        image,
                        svg: None,
                        reraster: false,
                        frames,
                    });
                });
            }
            return;
        }
        let Some(enc_tx) = self.md_enc_tx.clone() else {
            return;
        };
        let Some(entry) = self.md_image_cache.get_mut(&path) else {
            return;
        };
        // Wait if it failed, is still decoding, or already has an encode in flight (one at a time).
        if entry.failed || entry.enc_inflight {
            return;
        }
        let Some(img) = entry.decoded.clone() else {
            return;
        };
        // Fully visible: request an encode of the whole image at (cols, full_rows) unless already cached.
        if row_off == 0 && vis_rows >= full_rows {
            if entry.proto_size == Some((cols, full_rows)) {
                return;
            }
            entry.enc_inflight = true;
            let _ = enc_tx.send(MdEncodeRequest {
                path,
                key: MdEncodeKey::Full {
                    cols,
                    rows: full_rows,
                },
                image: img,
                crop: None,
                cols,
                rows: full_rows,
            });
            return;
        }
        // Partially scrolled: request an encode of just the visible pixel band at (cols, vis_rows), so the
        // image renders clipped to the viewport rather than being hidden.
        if entry.clip_key == Some((cols, full_rows, row_off, vis_rows)) {
            return;
        }
        let (dw, dh) = (img.width(), img.height());
        let (y0, h) = md_band_pixels(full_rows, row_off, vis_rows, dh);
        entry.enc_inflight = true;
        let _ = enc_tx.send(MdEncodeRequest {
            path,
            key: MdEncodeKey::Clip {
                cols,
                full_rows,
                row_off,
                vis_rows,
            },
            image: img,
            crop: Some((0, y0, dw, h)),
            cols,
            rows: vis_rows,
        });
    }

    /// The render protocol to draw for the visible portion of inline image `url`. Prefers the protocol
    /// that exactly matches the current position (full image when fully visible, or the band matching
    /// `(cols, full_rows, row_off, vis_rows)`); while a newly-scrolled band is still encoding it returns
    /// the last encoded protocol so the image stays on screen (and snaps to the exact band on arrival)
    /// rather than blinking out. None only until the very first protocol for this image is ready.
    pub fn md_image_proto(
        &self,
        url: &str,
        cols: u16,
        full_rows: u16,
        row_off: u16,
        vis_rows: u16,
    ) -> Option<&Protocol> {
        let path = if crate::preview::markdown::is_synthetic_md_url(url) {
            PathBuf::from(url)
        } else {
            let base = self
                .tab
                .preview_path
                .as_ref()
                .and_then(|p| p.parent())
                .map(|p| p.to_path_buf());
            resolve_md_image_path(url, base.as_deref())?
        };
        let entry = self.md_image_cache.get(&path)?;
        // Exact match for the current position.
        if row_off == 0 && vis_rows >= full_rows {
            if entry.proto_size == Some((cols, full_rows)) {
                return entry.protocol.as_ref();
            }
        } else if entry.clip_key == Some((cols, full_rows, row_off, vis_rows)) {
            // If the clip's encode had failed (only the key recorded, with no clip_protocol),
            // degrade to the full protocol (never leave it blank).
            return entry.clip_protocol.as_ref().or(entry.protocol.as_ref());
        }
        // Not yet encoded for this exact position: keep the last band (or the full image) visible.
        entry.clip_protocol.as_ref().or(entry.protocol.as_ref())
    }

    /// Drive every animated inline Markdown GIF (the per-entry analog of `App::advance_gif_if_due`
    /// for the full-screen GIF path). Each cache entry with ≥2 frames advances independently once
    /// its current frame's display time has elapsed. Advancing swaps `decoded` to the new frame and
    /// invalidates the encode keys (`proto_size`/`clip_key`/`zoom_key`) — but **not** the encoded
    /// protocols themselves, which stay visible until `ensure_md_image` (called from the renderer,
    /// only for placements actually drawn) requests and receives a fresh encode of the new frame.
    /// Advancing an off-screen entry therefore costs only an index bump, never a re-encode.
    /// Returns true if any entry advanced (the caller re-renders).
    pub fn advance_md_gifs_if_due(&mut self) -> bool {
        let now = std::time::Instant::now();
        let mut advanced = false;
        for entry in self.md_image_cache.values_mut() {
            if entry.frames.len() < 2 {
                continue;
            }
            let Some(shown_at) = entry.shown_at else {
                // First tick: just start the timer (the first frame is already shown in decoded).
                entry.shown_at = Some(now);
                continue;
            };
            let delay = entry.frames[entry.idx].1;
            if now.duration_since(shown_at) < delay {
                continue;
            }
            entry.idx = (entry.idx + 1) % entry.frames.len();
            entry.shown_at = Some(now);
            entry.decoded = Some(entry.frames[entry.idx].0.clone());
            // The next frame needs a re-encode: invalidate only the key. Keep the old protocol
            // displayed until the next encode arrives (clearing it would leave a momentary blank —
            // the same convention as the fence re-raster).
            entry.proto_size = None;
            entry.clip_key = None;
            entry.zoom_key = None;
            advanced = true;
        }
        advanced
    }

    /// Wait time until the soonest inline-GIF frame change, across every animating cache entry (for
    /// the run loop's poll timeout — mirrors `App::gif_poll_timeout` for the full-screen GIF path).
    /// None when no inline GIF is currently animating.
    pub fn md_gif_poll_timeout(&self) -> Option<std::time::Duration> {
        use std::time::Duration;
        let mut min: Option<Duration> = None;
        for entry in self.md_image_cache.values() {
            if entry.frames.len() < 2 {
                continue;
            }
            let remaining = match entry.shown_at {
                None => Duration::ZERO, // not timed yet: run the next tick right away to start timing
                Some(t) => entry.frames[entry.idx]
                    .1
                    .checked_sub(t.elapsed())
                    .unwrap_or(Duration::ZERO),
            };
            min = Some(min.map_or(remaining, |m| m.min(remaining)));
        }
        min.map(|d| d.clamp(Duration::from_millis(10), Duration::from_millis(100)))
    }
}
