use super::*;

/// Cap on how many mermaid-fence/math renders may be in flight (spawned, result not yet applied) at
/// once — see `App::ensure_mermaid_fence_render` / `App::ensure_math_render`. Unlike a real inline
/// image, which is only requested for a placement the renderer actually draws (`ensure_md_image`,
/// called per *visible* placement), a fence/math render is requested for the *whole* document right
/// in `ensure_md_cache` (its rendered size has to be known just to lay the document out, even for
/// content that is off-screen). Without a cap, a document with hundreds of distinct expressions calls
/// `std::thread::spawn` hundreds of times synchronously in a single pass — on the UI thread, before
/// the first frame can even draw (measured: 600 expressions → 911ms, ~15x the 60ms draw budget) — and
/// past some count `std::thread::spawn` can itself fail (EAGAIN) and panic, which would take the
/// whole app down (nothing wraps this loop in `catch_unwind`).
const MAX_SYNTHETIC_RENDERS_IN_FLIGHT: usize = 16;

/// Whether we're running inside tmux, for deciding whether konoma's own full-screen kitty transfer
/// (`preview/kitty.rs`) must wrap its escapes in tmux's DCS passthrough (`kitty_is_tmux`).
///
/// OR's `$TMUX`'s mere *presence* together with `ratatui-image`'s own (private) detection rule —
/// `TERM` starting with `"tmux"`, or `TERM_PROGRAM == "tmux"` (`picker.rs`'s
/// `detect_tmux_and_outer_protocol_from_env`) — because the Markdown *inline*-image path asks the
/// picker to build its own protocol (which uses that private `is_tmux`), while the full-screen path
/// reads this crate's own `kitty_is_tmux`: two independent flags fed by two different rules, each
/// only consulted by one of the two image paths. Checking `$TMUX` alone left them disagreeing behind
/// an ssh hop out of a tmux pane, or under `sudo` inside one — both forward `TERM` but drop `$TMUX`
/// (`sudo`'s `env_reset` keeps `TERM` via `env_keep` while clearing everything not on that list) — so
/// the picker correctly detected tmux and wrapped its escapes while this flag stayed `false` and sent
/// *unwrapped* kitty escapes for the full-screen path only: an unwrapped-vs-wrapped mismatch between
/// the two paths, not present in either path taken alone. The other direction (this returns `true`
/// when the picker's own rule would say `false`, e.g. a pre-3.2 tmux session where `$TMUX` is set but
/// `TERM` is still `screen-*` and no known `TERM_PROGRAM` is set) is harmless: the picker then never
/// detects `Kitty` as the `protocol_type` in the first place, so `use_kitty` is `false` and this flag
/// is never read.
///
/// Takes the three env values as parameters instead of reading `std::env::var`/`var_os` directly, so
/// it is a pure function safe to unit-test — `std::env::set_var` mutates process-wide state shared by
/// every test thread (this crate's established convention, see `bookmarks::xdg_base_dir`).
fn is_tmux_from_env(tmux_var_set: bool, term: Option<&str>, term_program: Option<&str>) -> bool {
    tmux_var_set || term.is_some_and(|t| t.starts_with("tmux")) || term_program == Some("tmux")
}

/// The fixed kitty image id of slot `slot` in the family `key` belongs to, for the picture filed
/// under `path`, allocating it on first use. `None` when this terminal is not a kitty one — then
/// the encode goes back through ratatui-image, which writes the picture as cell content and leaves
/// the terminal holding nothing between frames, so there is no id to pin and none is burned.
///
/// `slot` is the index `reserve_proto_slot` just handed out, so an id is pinned to a slot and not
/// to the size that slot currently holds: recycling the slot re-points its id at the new size, and
/// the terminal replaces that one picture instead of gaining another. Ids are only ever appended,
/// never dropped — see `App::md_kitty_ids`.
///
/// Takes the map rather than `&mut App` so a caller can hold a `&mut` borrow of an `md_image_cache`
/// entry at the same time (two disjoint fields of `App`, which the borrow checker splits happily).
fn kitty_id_for(
    ids: &mut std::collections::HashMap<PathBuf, [Vec<u32>; 3]>,
    path: &Path,
    key: &MdEncodeKey,
    slot: usize,
    use_kitty: bool,
) -> Option<u32> {
    if !use_kitty {
        return None;
    }
    let family = &mut ids.entry(path.to_path_buf()).or_default()[key.slot()];
    while family.len() <= slot {
        family.push(crate::preview::kitty::next_id());
    }
    family.get(slot).copied()
}

impl App {
    /// Attach the image backend (terminal Picker and the offload tx) at startup.
    pub fn attach_image_backend(&mut self, picker: Picker, tx: UnboundedSender<ResizeRequest>) {
        // Use konoma's own compressed transmit only on a kitty-graphics terminal; other protocols
        // (sixel/iterm2/halfblocks) keep the ratatui-image path.
        self.use_kitty = matches!(
            picker.protocol_type(),
            ratatui_image::picker::ProtocolType::Kitty
        );
        self.kitty_is_tmux = is_tmux_from_env(
            std::env::var_os("TMUX").is_some(),
            std::env::var("TERM").ok().as_deref(),
            std::env::var("TERM_PROGRAM").ok().as_deref(),
        );
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

    /// Begin one inline-image overlay pass — see `App::md_frame`. Called by the renderer before it
    /// walks the placements, so that every encode request the pass makes is stamped with the same
    /// number and the slots it is drawing from are protected from being recycled underneath it.
    pub fn begin_md_image_frame(&mut self) {
        self.md_frame = self.md_frame.saturating_add(1);
    }

    /// Apply a completed background decode of an inline Markdown image. Returns whether to redraw.
    pub fn apply_md_image(&mut self, res: MdImageResult) -> bool {
        // The entry is always pre-placed at kick time (ensure_mermaid_fence_render /
        // ensure_md_image). Missing = a stale result (already evicted from the cache on a file
        // switch / by `drop_changed_md_images` / already pruned), so drop it instead of reviving
        // it (with or_default, the old diagram's raster would be re-inserted and linger until the
        // next enter_preview, and on top of that the md_cache invalidation below would needlessly
        // do a full rebuild once for **an unrelated current document**).
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
        let is_mermaid = crate::preview::markdown::is_mermaid_fence_url(&url_str);
        if !res.reraster && (is_mermaid || is_math) {
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
                    // For math and mermaid fences, load the SVG's **intrinsic size (in px user
                    // units, "1 unit = 1px")** into layout_px, rather than the raster's pixel
                    // dimensions. The raster is scaled by an unrelated setting (`svg_max_px`), so
                    // sizing off of it would drag that setting into the text-size math; the
                    // intrinsic size is the same px domain the renderer laid glyphs out in
                    // (`text_metrics::FONT_SIZE` for mermaid, RaTeX's 40-units/em for math), which
                    // `mermaid_cells`/`math_cells` need to size the diagram/equation against the
                    // terminal's own body text. Fall back to the raster dimensions only when the
                    // intrinsic size can't be obtained. Regular images work as before (raster px).
                    entry.layout_px = if is_math || is_mermaid {
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
                    // Trigger a re-encode with the high-density raster, keeping the old protocol
                    // displayed until the new encode arrives (clearing it would leave a momentary
                    // blank).
                    entry.mark_stale();
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
        // Read before borrowing an entry out of the cache (the borrow checker will not let both live).
        let (use_kitty, is_tmux) = (self.use_kitty, self.kitty_is_tmux);
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
        let frame = self.md_frame;
        let ids = &mut self.md_kitty_ids;
        let Some(entry) = self.md_image_cache.get_mut(&key_path) else {
            return;
        };
        let enc_key = MdEncodeKey::Zoom { cols, rows, crop };
        let settled = entry.touch(&enc_key, frame);
        if settled || entry.enc_inflight {
            return;
        }
        let Some(tx) = enc_tx else { return };
        let slot = reserve_proto_slot(&mut entry.zoom, enc_key, frame, MD_ZOOM_SLOTS);
        let kitty = kitty_id_for(ids, &key_path, &enc_key, slot, use_kitty).map(|id| (id, is_tmux));
        entry.enc_inflight = true;
        let _ = tx.send(MdEncodeRequest {
            path: key_path,
            key: enc_key,
            image: img,
            crop: Some(crop),
            cols,
            rows,
            kitty,
        });
    }

    /// The image for the focused inline diagram's current zoom, if encoded for this (cols, rows).
    /// While a fresh crop is still encoding, the previous zoom crop (or the unzoomed full image)
    /// stays visible instead of blinking out.
    pub fn md_fence_zoom_proto(&self, url: &str, cols: u16, rows: u16) -> Option<&InlineImage> {
        let entry = self.md_image_cache.get(&PathBuf::from(url))?;
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
        // A second copy for the panic-fallback result below: `job` (built next) moves its own copy
        // of `kp` into the `MdImageResult` it builds on success, so the failure path needs its own.
        let kp_on_panic = kp.clone();
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
                // Same `compute_or_fallback` safety net as `ensure_mermaid_fence_render`/
                // `ensure_math_render` (principle #3): `rasterize_bytes` (resvg) is the exact same
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

    /// How many mermaid-fence/math renders currently have a placeholder in `md_image_cache` but no
    /// result yet (i.e. a background thread is running for them right now). Both kinds share the
    /// same one-shot-thread-per-request shape and compete for the same `MAX_SYNTHETIC_RENDERS_IN_FLIGHT`
    /// cap.
    fn synthetic_renders_in_flight(&self) -> usize {
        self.md_image_cache
            .iter()
            .filter(|(k, e)| {
                if e.decoded.is_some() || e.failed {
                    return false; // done (either way) — no longer occupying a slot
                }
                let s = k.to_string_lossy();
                crate::preview::markdown::is_mermaid_fence_url(&s)
                    || crate::preview::markdown::is_math_url(&s)
            })
            .count()
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
            // Bounded concurrency (MAX_SYNTHETIC_RENDERS_IN_FLIGHT): at capacity, skip this pass
            // *without* inserting a placeholder. Nothing is lost — `apply_md_image` invalidates
            // `md_cache` whenever any synthetic entry's first result lands, so the very next
            // `ensure_md_cache` rebuild re-scans the document's mermaid_fences and retries whatever
            // is still absent from `md_image_cache`, now that a slot has freed up (the classifier
            // that turns an absent key into a "loading" placement doesn't care whether a thread is
            // actually running for it yet). This self-driving wave always converges without a
            // dedicated persistent worker thread + channel, which is not an option here: that would
            // need its sender stored as a new field on `App` (`src/app.rs` is off limits).
            if self.synthetic_renders_in_flight() >= MAX_SYNTHETIC_RENDERS_IN_FLIGHT {
                return false;
            }
            self.md_image_cache
                .insert(key.clone(), MdImgEntry::default());
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
            self.md_image_cache
                .insert(key.clone(), MdImgEntry::default());
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
    /// `ensure_mermaid_fence_render` (including the `MAX_SYNTHETIC_RENDERS_IN_FLIGHT` cap and its
    /// self-driving retry); the SVG is kept so `apply_md_image` can record its intrinsic (em) size for
    /// layout. Returns true on a synchronous completion (no loader tx = tests).
    pub(super) fn ensure_math_render(&mut self, latex: String, display: bool) -> bool {
        let key = PathBuf::from(crate::preview::markdown::math_url(&latex, display));
        if self.md_image_cache.contains_key(&key) {
            return false;
        }
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
            // See the identical guard in ensure_mermaid_fence_render for why skipping here (without
            // inserting a placeholder) is safe and self-corrects on the next rebuild.
            if self.synthetic_renders_in_flight() >= MAX_SYNTHETIC_RENDERS_IN_FLIGHT {
                return false;
            }
            self.md_image_cache
                .insert(key.clone(), MdImgEntry::default());
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
            self.md_image_cache
                .insert(key.clone(), MdImgEntry::default());
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
        let Some(entry) = self.md_image_cache.get_mut(&res.path) else {
            return false;
        };
        entry.enc_inflight = false;
        let key = res.key;
        let mut stored = false;
        if let Some(slot) = entry.slots_mut(&key).iter_mut().find(|s| s.key == key) {
            slot.stale = false;
            if let Some(p) = res.image {
                slot.image = Some(p);
                stored = true;
            }
        }
        // Nothing drawable has ever come back for this picture at any size → text fallback.
        let degrade = !stored
            && matches!(key, MdEncodeKey::Full { .. })
            && !entry.failed
            && entry.full.iter().all(|s| s.image.is_none());
        if degrade {
            entry.failed = true;
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
        // Read before borrowing an entry out of the cache (the borrow checker will not let both live).
        let (use_kitty, is_tmux) = (self.use_kitty, self.kitty_is_tmux);
        let frame = self.md_frame;
        let ids = &mut self.md_kitty_ids;
        let Some(entry) = self.md_image_cache.get_mut(&path) else {
            return;
        };
        // Fully visible → the whole image at (cols, full_rows). Partially scrolled → just the
        // visible pixel band at (cols, vis_rows), so the image renders clipped to the viewport
        // rather than being hidden.
        let full_vis = row_off == 0 && vis_rows >= full_rows;
        let enc_key = if full_vis {
            MdEncodeKey::Full {
                cols,
                rows: full_rows,
            }
        } else {
            MdEncodeKey::Clip {
                cols,
                full_rows,
                row_off,
                vis_rows,
            }
        };
        // Claim this position for the frame being drawn **before** any early return: a placement
        // that has to wait for the single in-flight ticket still needs its slot kept out of the
        // recycling pool, otherwise two placements of one picture at two sizes take turns evicting
        // each other and the draw→request→apply→draw loop never stops.
        let settled = entry.touch(&enc_key, frame);
        // Wait if it failed, is still decoding, or already has an encode in flight (one at a time).
        if settled || entry.failed || entry.enc_inflight {
            return;
        }
        let Some(img) = entry.decoded.clone() else {
            return;
        };
        let (crop, rows) = if full_vis {
            (None, full_rows)
        } else {
            let (dw, dh) = (img.width(), img.height());
            let (y0, h) = md_band_pixels(full_rows, row_off, vis_rows, dh);
            (Some((0, y0, dw, h)), vis_rows)
        };
        let slot = reserve_proto_slot(entry.slots_mut(&enc_key), enc_key, frame, MD_PROTO_SLOTS);
        let kitty = kitty_id_for(ids, &path, &enc_key, slot, use_kitty).map(|id| (id, is_tmux));
        entry.enc_inflight = true;
        let _ = enc_tx.send(MdEncodeRequest {
            path,
            key: enc_key,
            image: img,
            crop,
            cols,
            rows,
            kitty,
        });
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
            resolve_md_image_path(url, base.as_deref())?
        };
        let entry = self.md_image_cache.get(&path)?;
        // Exact match for the current position, at this placement's own cell size.
        let key = if row_off == 0 && vis_rows >= full_rows {
            MdEncodeKey::Full {
                cols,
                rows: full_rows,
            }
        } else {
            MdEncodeKey::Clip {
                cols,
                full_rows,
                row_off,
                vis_rows,
            }
        };
        // Not yet encoded for this exact position (or that encode failed): keep the freshest band —
        // or full image — visible rather than leaving the reserved rows blank.
        entry.proto(&key).or_else(|| entry.newest_proto())
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
        self.md_image_cache
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
            // The next frame needs a re-encode: mark every slot stale rather than dropping it. The
            // old protocol stays displayed until the new encode arrives (clearing it would leave a
            // momentary blank — the same convention as the fence re-raster), and, because the slot
            // survives, the re-encode lands on the **same** kitty id and replaces the terminal's
            // picture instead of adding one per animation frame.
            entry.mark_stale();
            advanced = true;
        }
        advanced
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
            .md_image_cache
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
            self.md_image_cache.remove(p);
        }
    }

    /// Test-only: how many entries currently sit in `md_image_cache` (private to `app`, so an e2e
    /// test outside the module needs an accessor to observe it). Used to confirm — by state, not by
    /// wall-clock timing — that a document with many distinct fence/math expressions does not have
    /// them all land in the cache (= all spawned) in a single pass.
    #[cfg(test)]
    pub fn md_image_cache_len(&self) -> usize {
        self.md_image_cache.len()
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
