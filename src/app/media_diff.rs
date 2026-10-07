//! The image/PDF/SVG "diff" computation offloaded to a background worker (`docs/FEATURE-MEDIA-
//! DIFF.md`) — fetching each side's bytes (git/jj/follow-snapshot for the old side, the filesystem
//! for the new one), classifying the picture kind, and decoding into pixels is I/O plus real CPU
//! work that must never run on the UI thread. Follows `src/app/md_diff.rs`'s shape for the Markdown
//! block-diff: a `gen`-tagged request/result pair, a `_pending` guard against duplicate dispatch, a
//! synchronous fallback when no `Sender` is attached (tests), and a `compute_or_fallback` panic net.
//!
//! The baseline (which bytes count as "old") is resolved by `App::diff_baseline`, the same method
//! the Markdown block-diff's `Rendered` presentation uses, rather than re-deriving the follow-session
//! branching a second time (`App::media_diff_baseline`, below).
//!
//! `App::compute_media_diff` is the pure computation (no `&self`), shared byte-identically by the
//! worker thread and the synchronous fallback. Its result still carries decoded pixel data
//! (`MediaDiffComputed`/`MediaDiffSideDecoded`) — `App::apply_media_diff` moves that into
//! `md_image_cache` (under a `media_diff_url` key, the same pre-insert-then-`apply_md_image` shape
//! `ensure_mermaid_fence_render` uses) and keeps only the lightweight, pixel-free `MediaDiffOutcome`
//! resident in `App::media_diff_landed`.
//!
use std::collections::HashSet;

use crate::preview::media_diff::MediaDiffSide as KeySide;

use super::*;

impl App {
    /// Attach the Sender of the worker that computes media diffs in the background (called by
    /// `main`, mirroring `attach_md_diff_loader`).
    pub fn attach_media_diff_loader(&mut self, tx: std::sync::mpsc::Sender<MediaDiffResult>) {
        self.media_diff_tx = Some(tx);
    }

    /// The baseline a media diff's old side should be read against. Identical to the Rendered
    /// Markdown diff's own resolution (`App::diff_baseline`) — reused rather than re-derived, since
    /// the follow-session branching (dirty-at-follow-start snapshot / pinned clean-at-follow-start
    /// HEAD / no session at all) doesn't depend on what kind of file is being diffed.
    fn media_diff_baseline(&self, path: &Path) -> DiffBaseline {
        self.diff_baseline(path, MdDiffKind::Rendered)
    }

    /// The landed outcome for `(path, page, raster_px)`, if it matches the current generation.
    fn media_diff_landed_for(
        &self,
        path: &Path,
        page: u32,
        raster_px: (u32, u32),
    ) -> Option<MediaDiffOutcome> {
        match &self.media_diff_landed {
            Some((p, pg, rp, gen, outcome))
                if p.as_path() == path
                    && *pg == page
                    && *rp == raster_px
                    && *gen == self.media_diff_gen =>
            {
                Some(outcome.clone())
            }
            _ => None,
        }
    }

    /// Poll the landed media diff for `(path, page, raster_px)`, kicking a fresh computation
    /// (`kick_media_diff`) if neither a matching landed result nor an in-flight request already
    /// covers this exact key. `None` means not ready yet — the caller shows a "computing…" state for
    /// this frame rather than blocking (mirrors `App::poll_md_diff`).
    ///
    /// `raster_px` is normalized first (`normalize_raster_px`) so a caller-supplied box the decode
    /// doesn't actually depend on (a raw raster image always decodes at its own native resolution)
    /// never causes a spurious re-decode — a terminal resize changes the pane's pixel box every frame
    /// it's still settling, and only SVG/PDF's rasterization genuinely needs to redo work for that.
    pub(crate) fn poll_media_diff(
        &mut self,
        path: &Path,
        page: u32,
        raster_px: (u32, u32),
    ) -> Option<MediaDiffOutcome> {
        let raster_px = self.normalize_raster_px(path, raster_px);
        if let Some(outcome) = self.media_diff_landed_for(path, page, raster_px) {
            return Some(outcome);
        }
        let wants = |slot: &Option<(PathBuf, u32, (u32, u32))>| {
            slot.as_ref()
                .is_some_and(|(p, pg, rp)| p.as_path() == path && *pg == page && *rp == raster_px)
        };
        // Already covered by either the in-flight request (`media_diff_pending`) or the one
        // request-slot coalesced behind it (`media_diff_queued`, `kick_media_diff`'s own doc
        // comment) — either way there is nothing new to ask for.
        if !wants(&self.media_diff_pending) && !wants(&self.media_diff_queued) {
            self.kick_media_diff(path.to_path_buf(), page, raster_px);
        }
        self.media_diff_landed_for(path, page, raster_px)
    }

    /// `raster_px`, normalized to a fixed canonical value `(0, 0)` for every kind whose decode
    /// doesn't actually depend on it — only [`MediaDiffKind::Svg`]/[`MediaDiffKind::Pdf`] rasterize
    /// to a caller-chosen pixel box; [`MediaDiffKind::Image`] (and anything not picture-capable,
    /// which degrades to [`MediaDiffComputed::Summary`] regardless of `raster_px`) always decodes at
    /// its own native resolution.
    ///
    /// Prefers the worker's own landed classification (`media_landed_outcome_for`) over the cheaper
    /// `diff_target_kind` when one is on hand, since a **deleted** file whose extension isn't
    /// glob-recognized can only be classified by the worker's byte-sniff — `diff_target_kind`'s own
    /// `Config::resolve_preview` fallback can't see that at all for a path that doesn't exist. Before
    /// that first landing, such a path normalizes to `(0, 0)` same as "not picture-capable" would; if
    /// it turns out to be `Svg`/`Pdf` after all, that first request rasterizes tiny
    /// (`decode_svg_side`/`decode_pdf_side` floor `raster_px` at 1px) and self-corrects the moment the
    /// *next* poll sees the now-landed real kind — a one-frame transient, not a lasting bug.
    fn normalize_raster_px(&self, path: &Path, raster_px: (u32, u32)) -> (u32, u32) {
        let kind = match self.media_landed_outcome_for(path) {
            Some(MediaDiffOutcome::Ready { kind, .. }) => Some(*kind),
            _ => classify_kind(&self.diff_target_kind(path)),
        };
        match kind {
            Some(MediaDiffKind::Svg) | Some(MediaDiffKind::Pdf) => raster_px,
            _ => (0, 0),
        }
    }

    /// Want a fresh media-diff request for `(path, page, raster_px)`. **At most one worker is ever
    /// in flight at a time**: if none is currently running (`!media_diff_worker_busy`), this
    /// dispatches immediately (`dispatch_media_diff`); otherwise it **coalesces** into the single
    /// `media_diff_queued` slot — overwriting whatever want was already waiting there — and
    /// `apply_media_diff` dispatches it the moment the busy worker's result lands (accepted *or*
    /// stale). An AI rewriting an image repeatedly therefore never piles up concurrent decodes; only
    /// the newest want survives the wait.
    fn kick_media_diff(&mut self, path: PathBuf, page: u32, raster_px: (u32, u32)) {
        let want = (path, page, raster_px);
        if self.media_diff_worker_busy {
            self.media_diff_queued = Some(want);
            return;
        }
        self.dispatch_media_diff(want);
    }

    /// Actually dispatches `want` onto the worker (or the synchronous fallback) — the single place
    /// that bumps `media_diff_gen` (discarding any earlier in-flight result on arrival, exactly
    /// `kick_md_diff`'s own shape) and marks the one worker slot occupied
    /// (`media_diff_worker_busy`/`media_diff_pending`).
    fn dispatch_media_diff(&mut self, want: (PathBuf, u32, (u32, u32))) {
        #[cfg(test)]
        crate::test_support::note_media_diff_dispatch_call();
        let (path, page, raster_px) = want;
        self.media_diff_gen = self.media_diff_gen.wrapping_add(1);
        self.media_diff_pending = Some((path.clone(), page, raster_px));
        self.media_diff_worker_busy = true;
        let gen = self.media_diff_gen;
        let baseline = self.media_diff_baseline(&path);
        let req = MediaDiffRequest {
            gen,
            path,
            root: self.tab.root.clone(),
            baseline,
            page,
            raster_px,
            preview_rules: self.cfg.preview.rules.clone(),
            preview_commands: self.cfg.external.preview_commands,
        };
        self.spawn_or_sync_media_diff(req);
    }

    /// Compute the media diff on a separate thread and return it via `media_diff_tx`. With no
    /// Sender attached (tests / no channel), fall back to **synchronous** computation and
    /// application — the same contract `spawn_or_sync_md_diff` already has.
    fn spawn_or_sync_media_diff(&mut self, req: MediaDiffRequest) {
        let Some(tx) = self.media_diff_tx.clone() else {
            let computed = Self::compute_media_diff(&req);
            self.apply_media_diff(MediaDiffResult {
                gen: req.gen,
                path: req.path,
                page: req.page,
                raster_px: req.raster_px,
                computed,
            });
            return;
        };
        std::thread::spawn(move || {
            // `compute_or_fallback`: a worker that panicked without a result would latch
            // `media_diff_pending` forever, freezing the diff on a "computing…" spinner that never
            // resolves (same reasoning `spawn_or_sync_md_diff` documents for its own panic net).
            let computed = crate::preview::markdown::compute_or_fallback(
                || Self::compute_media_diff(&req),
                || MediaDiffComputed::Unavailable,
            );
            let _ = tx.send(MediaDiffResult {
                gen: req.gen,
                path: req.path.clone(),
                page: req.page,
                raster_px: req.raster_px,
                computed,
            });
        });
    }

    /// The pure media-diff computation (no `&self`), shared byte-identically by the worker thread
    /// and the synchronous fallback.
    ///
    /// Resolution order: old bytes (via `req.baseline`) and new bytes (`fs::read`, `None` = deleted)
    /// are both fetched first, so `same_bytes`/each side's length are always known regardless of what
    /// happens next. The picture *kind* is then classified **once** — from the new file when it
    /// exists (`Config::resolve_preview`'s own rules, by path), or from the old bytes when it doesn't
    /// (sniffing the baseline content, so a deleted file is still config-driven) — both sides of a
    /// *modified* file always share one kind, never independently reclassified. A kind that isn't
    /// picture-capable, or either side over the byte cap, degrades to [`MediaDiffComputed::Summary`];
    /// otherwise each side is decoded independently (one side's decode failure doesn't take the other
    /// side down with it).
    fn compute_media_diff(req: &MediaDiffRequest) -> MediaDiffComputed {
        Self::compute_media_diff_with_cap(req, crate::preview::pdf::PAGE_COUNT_MAX_BYTES)
    }

    /// `compute_media_diff`'s real body, parameterized on the per-side byte cap purely for
    /// testability — production always passes `PAGE_COUNT_MAX_BYTES`; tests pass an artificially
    /// small cap against an ordinary small fixture (mirrors `preview::pdf::page_count_impl`'s
    /// identical `max_bytes` seam, for the identical reason: exercising the cap without needing to
    /// actually construct/store a real 64 MiB fixture file).
    fn compute_media_diff_with_cap(req: &MediaDiffRequest, cap: u64) -> MediaDiffComputed {
        let bytes = resolve_media_diff_bytes(req, cap);
        classify_and_decode_media_diff(req, bytes)
    }

    /// Apply a media-diff result from the worker thread (or the synchronous fallback).
    ///
    /// **Always** frees the one worker slot first (`media_diff_worker_busy = false`) and, if a want
    /// coalesced behind it (`media_diff_queued`), immediately dispatches that one next — this runs
    /// whether or not `res` itself turns out to be stale, since the worker slot is free either way and
    /// "at most one worker in flight, latest wins" must keep making progress on whatever was asked for
    /// most recently.
    ///
    /// Discards a stale generation (mirrors `App::apply_md_diff`): a newer request/invalidation
    /// superseded this one, so `res.computed` is simply dropped. Otherwise it moves every decoded
    /// side's pixel data into `md_image_cache` under its `media_diff_url` key (the same
    /// pre-insert-then-`apply_md_image` shape `ensure_mermaid_fence_render` uses), drops every
    /// `media-diff://` key this landing does **not** reference (so an earlier landing for a different
    /// `(path, page, raster_px)` doesn't leave its own now-unreachable rasters resident forever — never
    /// touches a mermaid/math key), and stores the lightweight, pixel-free outcome in
    /// `media_diff_landed`.
    pub fn apply_media_diff(&mut self, res: MediaDiffResult) -> bool {
        self.media_diff_worker_busy = false;
        self.media_diff_pending = None;
        let applied = if res.gen != self.media_diff_gen {
            false // stale: a newer request/invalidation superseded this one.
        } else {
            let outcome = self.materialize_media_diff(res.computed);
            let live = live_cache_keys(&outcome);
            self.md_image_cache.retain(|k, _| {
                let s = k.to_string_lossy();
                !crate::preview::media_diff::is_media_diff_url(&s) || live.contains(k)
            });
            self.media_diff_landed = Some((res.path, res.page, res.raster_px, res.gen, outcome));
            true
        };
        if let Some(next) = self.media_diff_queued.take() {
            self.dispatch_media_diff(next);
        }
        applied
    }

    /// Move a computed outcome's pixel data into `md_image_cache`, returning the pixel-free stored
    /// form.
    fn materialize_media_diff(&mut self, computed: MediaDiffComputed) -> MediaDiffOutcome {
        match computed {
            MediaDiffComputed::Ready {
                kind,
                base,
                same_bytes,
                old,
                new,
            } => MediaDiffOutcome::Ready {
                kind,
                base,
                same_bytes,
                old: self.materialize_side(old),
                new: self.materialize_side(new),
            },
            MediaDiffComputed::Summary {
                base,
                same_bytes,
                old_len,
                new_len,
            } => MediaDiffOutcome::Summary {
                base,
                same_bytes,
                old_len,
                new_len,
            },
            MediaDiffComputed::Unavailable => MediaDiffOutcome::Unavailable,
        }
    }

    /// One side's move-into-cache — a no-op for every non-`Picture` variant.
    fn materialize_side(&mut self, side: MediaDiffSideDecoded) -> MediaDiffSide {
        match side {
            MediaDiffSideDecoded::Absent => MediaDiffSide::Absent,
            MediaDiffSideDecoded::Failed { reason } => MediaDiffSide::Failed { reason },
            MediaDiffSideDecoded::PageMissing => MediaDiffSide::PageMissing,
            MediaDiffSideDecoded::Picture(p) => {
                // Pre-place the entry, exactly like `ensure_mermaid_fence_render` does before handing
                // its result to `apply_md_image` — that fn drops any result whose key isn't already
                // present (its own guard against reviving an evicted entry), so the insert has to
                // happen here, not inside it.
                self.md_image_cache.entry(p.cache_key.clone()).or_default();
                // No transparency handling here: each side draws with exactly the same pixels the
                // ordinary preview would for that kind — a PDF's own renderer composites its pages
                // onto opaque white before this ever sees them (`preview::pdf::pixmap_to_dynamic_
                // image`), and SVG stays exactly as transparent as the ordinary preview renders it, on
                // every protocol including kitty. So this diff never special-cases a picture kind
                // differently from how the ordinary preview already draws it.
                self.apply_md_image(MdImageResult {
                    path: p.cache_key.clone(),
                    image: Ok(p.image),
                    svg: p.svg,
                    reraster: false,
                    frames: p.frames,
                });
                MediaDiffSide::Picture(MediaDiffPicture {
                    natural_px: p.natural_px,
                    bytes: p.bytes,
                    page_count: p.page_count,
                    cache_key: p.cache_key,
                })
            }
        }
    }

    /// Drop the landed media diff and bump the generation, so any result already in flight is
    /// discarded on arrival (`App::apply_media_diff`'s gen check). Called from
    /// `App::invalidate_diff_caches` wherever the working tree/baseline being compared against may
    /// have changed. `_pending` is also cleared (mirrors `App::invalidate_md_diff`): otherwise
    /// `poll_media_diff`'s dedup check, keyed on request identity rather than `gen`, would keep
    /// declining to kick a fresh request for the identical `(path, page, raster_px)` right after a
    /// baseline change.
    ///
    /// Deliberately does **not** touch `media_diff_worker_busy`/`media_diff_queued`: a worker already
    /// dispatched before this runs is still executing against the *old* baseline and cannot be
    /// recalled, so the one worker slot stays occupied until it reports back (`App::apply_media_diff`
    /// frees it and dispatches whatever coalesced behind it in the meantime).
    pub(crate) fn invalidate_media_diff(&mut self) {
        self.media_diff_gen = self.media_diff_gen.wrapping_add(1);
        self.media_diff_landed = None;
        self.media_diff_pending = None;
    }

    /// Removes every `media-diff://` entry from `md_image_cache` unconditionally — never touches a
    /// mermaid/math key (`is_media_diff_url` alone gates the predicate). Called wherever the diff
    /// surface stops being able to ever repopulate them itself, which `App::apply_media_diff`'s own
    /// landing-triggered prune (`live_cache_keys`) can't cover since there's no further landing to
    /// piggyback on: `App::back_to_tree` (closing the diff back to the tree/Git hub — nothing will
    /// poll/land a media diff again until some unrelated one is opened), and
    /// `App::invalidate_diff_caches` whenever the tab's active view isn't a media-diff-capable
    /// `GitDiff` target (an in-place retarget to a non-media file, or a tab switch landing on a tab
    /// whose own target isn't a media diff).
    ///
    /// **The rule is unconditional on tab identity**: `media_diff_landed`/`_pending`/`md_image_cache`
    /// are all `App`-level, not per-tab, so switching to *any* other tab (`App::invalidate_diff_
    /// caches` runs on every tab switch, not only a genuine retarget) frees a still-open media diff's
    /// pixels exactly like `back_to_tree` does, and switching back re-kicks a fresh computation
    /// (`switching_to_another_tab_and_back_redraws_both_pictures` pins the re-kick half). Deliberate
    /// memory-vs-recompute behavior, not a bug: at most one tab's media-diff pixels are ever resident
    /// at a time.
    pub(crate) fn prune_media_diff_picture_cache(&mut self) {
        self.md_image_cache
            .retain(|k, _| !crate::preview::media_diff::is_media_diff_url(&k.to_string_lossy()));
    }

    /// `PerTab::diff_media_page`, floored at `1` (a fresh tab/diff starts there; nothing should
    /// ever observe `0`, but this is the one place every reader goes through so that stays true
    /// regardless).
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub(crate) fn diff_media_page(&self) -> u32 {
        self.tab.diff_media_page.max(1)
    }

    /// The media diff's current layout (`[git] media_diff`, cycled by `s` —
    /// `App::cycle_media_diff_layout`) — read by `ui/preview.rs::render_gitdiff_media` to call
    /// `preview::media_diff::layout`.
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub(crate) fn media_diff_layout(&self) -> MediaDiffLayout {
        self.media_diff_layout
    }

    /// Whether `key` currently has an entry in `md_image_cache` — `ui/preview.rs::render_gitdiff_
    /// media` uses this to detect a landed `Picture` whose pixels were evicted from underneath it and
    /// re-kick a fresh computation instead of silently drawing nothing. This genuinely happens:
    /// `App::enter_preview`'s file-switch clear fires whenever the *previous* preview target's path
    /// differs from the one being entered — landing on a *different* file's preview first (file
    /// paging, a Markdown link, a bookmark jump) evicts the still-resident `media-diff://` keys.
    /// `evict_md_image_cache_key_for_test` reproduces that directly for tests.
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub(crate) fn md_image_cache_contains(&self, key: &Path) -> bool {
        self.md_image_cache.contains_key(key)
    }

    /// Test-only: remove `key` from `md_image_cache` directly, simulating the eviction
    /// `md_image_cache_contains`'s own doc comment describes (a different file's preview reusing
    /// the one shared cache) without needing to actually reproduce that specific keypress sequence.
    #[cfg(test)]
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub(crate) fn evict_md_image_cache_key_for_test(&mut self, key: &Path) {
        self.md_image_cache.remove(key);
    }

    /// Test-only: every `media-diff://` key currently in `md_image_cache` — lets a test discover the
    /// *real* cache keys a production render landed (depending on the render path's own raster
    /// target) rather than re-deriving them via a second `poll_media_diff` call, whose `raster_px`
    /// might mismatch and trigger an unrelated poll of its own.
    #[cfg(test)]
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub(crate) fn md_image_cache_media_diff_keys_for_test(&self) -> Vec<PathBuf> {
        self.md_image_cache
            .keys()
            .filter(|k| crate::preview::media_diff::is_media_diff_url(&k.to_string_lossy()))
            .cloned()
            .collect()
    }

    /// The terminal's font cell size in pixels, or `None` when there is no picker at all (a terminal
    /// with no graphics protocol, or images disabled) — `ui/preview.rs::render_gitdiff_media` degrades
    /// to the binary summary line in that case.
    pub(crate) fn picker_cell_px(&self) -> Option<(u32, u32)> {
        self.picker.as_ref().map(|p| {
            let f = p.font_size();
            (f.width as u32, f.height as u32)
        })
    }

    /// Whether the diff's `Rendered` presentation is currently the image/PDF/SVG side-by-side view
    /// (as opposed to decorated Markdown blocks) — `ui/preview.rs::render_gitdiff` reads this to pick
    /// `render_gitdiff_media` over `render_diff_rendered`.
    ///
    /// `Rendered` is reachable only for Markdown and the media kinds (pinned by
    /// `app::tests::diff_media_active_agrees_with_the_single_not_markdown_check_across_every_
    /// reachable_kind`), so "not Markdown" identifies the side-by-side view. `diff_media_capable` is
    /// deliberately *not* used here: a deleted, content-sniffed (no glob) image classifies as
    /// `CanNotPreview` until the worker lands, which `diff_media_capable` would wrongly read as "not
    /// media".
    pub(crate) fn diff_media_active(&self) -> bool {
        if !self.diff_rendered_active() {
            return false;
        }
        let Some(PreviewKind::GitDiff(path)) = self.tab.preview_kind.as_ref() else {
            return false;
        };
        !matches!(self.diff_target_kind(path), PreviewKind::Markdown(_))
    }

    /// `Config::resolve_preview(path)`, memoized in `diff_target_kind_cache` — the single read side of
    /// that cache (`App::refresh_diff_target_kind_cache` is the single write side). A cache hit
    /// (`path` matches whatever the diff surface is currently targeting) is the hot, per-frame path,
    /// costing nothing beyond a `PathBuf` comparison and a `Clone`; a miss falls back to a direct,
    /// uncached resolve, so this is always *correct*, just not always free.
    pub(super) fn diff_target_kind(&self, path: &Path) -> PreviewKind {
        match &self.diff_target_kind_cache {
            Some((p, k)) if p.as_path() == path => k.clone(),
            _ => self.cfg.resolve_preview(path),
        }
    }

    /// Refreshes `diff_target_kind_cache` for whatever the diff surface is currently targeting
    /// (`self.tab.preview_kind`'s `GitDiff` path, if any) — called from `App::invalidate_diff_caches`,
    /// which runs both on a genuine retarget and on an invalidation-only refresh, so one call site
    /// covers both. Clears the cache outright when the diff surface isn't showing a `GitDiff` at all,
    /// so a stale entry can never outlive the target it was resolved for.
    pub(super) fn refresh_diff_target_kind_cache(&mut self) {
        self.diff_target_kind_cache = match self.tab.preview_kind.clone() {
            Some(PreviewKind::GitDiff(path)) => {
                let kind = self.cfg.resolve_preview(&path);
                Some((path, kind))
            }
            _ => None,
        };
    }

    /// Whether `path` resolves to a media-diff-capable kind (Image/Svg/Pdf) — the one predicate
    /// `classify_kind` and `App::follow_jump`'s own `media_side_by_side` check both defer to, so the
    /// two can never drift apart on which kinds get the side-by-side treatment. Goes through the same
    /// memoized `diff_target_kind` as every other diff-surface predicate in this module.
    pub(crate) fn diff_media_capable(&self, path: &Path) -> bool {
        classify_kind(&self.diff_target_kind(path)).is_some()
    }

    /// Whether `path`'s Source-representation diff, if the raw line diff comes back **empty**,
    /// should be checked against this module's worker instead of being trusted at face value as
    /// "(no changes)". True for exactly the kinds with no text/decorated representation of their own
    /// to fall back on (video/archive/table/unsupported/an image-mode delegated command): these rely
    /// on git's raw line diff alone, which comes back empty for *any* binary file whether or not it
    /// actually changed, so an empty diff there is never proof of "no changes". Markdown/Code/Text/
    /// Mermaid/a text-mode command are excluded — an empty diff there is always genuinely "no
    /// changes". Image/Svg/Pdf aren't excluded, but in practice never reach `Source` at all
    /// (`App::round_diff_view` always substitutes `Rendered`) — if a test forces the state anyway,
    /// their kind resolves to `Some(_)` under `classify_kind`, landing on the worker's real `Ready`
    /// outcome rather than `Summary`, which the caller handles correctly either way.
    pub(crate) fn diff_binary_summary_eligible(&self, path: &Path) -> bool {
        let resolved = self.diff_target_kind(path);
        let windowed_text = matches!(
            resolved,
            PreviewKind::Markdown(_)
                | PreviewKind::Code(_)
                | PreviewKind::Text(_)
                | PreviewKind::Mermaid(_)
        ) || matches!(&resolved, PreviewKind::Command { render_as, .. } if render_as.as_deref() != Some("image"));
        !windowed_text
    }

    /// The landed media-diff outcome for `path`, ignoring the page/raster it was computed at (kind
    /// classification and page-count don't depend on either) — used by `App::diff_representations` to
    /// classify a **deleted** file `resolve_preview` can't, and by the page-turn/footer helpers below,
    /// called from contexts that don't have a raster target on hand at all.
    pub(super) fn media_landed_outcome_for(&self, path: &Path) -> Option<&MediaDiffOutcome> {
        self.media_diff_landed
            .as_ref()
            .filter(|(p, ..)| p.as_path() == path)
            .map(|(_, _, _, _, outcome)| outcome)
    }

    /// The current GitDiff target's landed `Ready` sides, if any (ignoring page/raster — see
    /// `media_landed_outcome_for`'s own doc comment).
    fn media_diff_ready_sides(&self) -> Option<(&MediaDiffSide, &MediaDiffSide)> {
        let PreviewKind::GitDiff(path) = self.tab.preview_kind.as_ref()? else {
            return None;
        };
        match self.media_landed_outcome_for(path)? {
            MediaDiffOutcome::Ready { old, new, .. } => Some((old, new)),
            _ => None,
        }
    }

    /// The larger of the two sides' own PDF page counts (`None` for anything not a landed `Ready`
    /// PDF pair — a non-PDF picture kind reports `page_count: None` per side, which floors to `1`
    /// here exactly like a single-page PDF would).
    fn media_diff_max_page_count(&self) -> Option<u32> {
        let (old, new) = self.media_diff_ready_sides()?;
        let pc = |s: &MediaDiffSide| match s {
            MediaDiffSide::Picture(p) => p.page_count,
            _ => None,
        };
        Some(pc(old).unwrap_or(1).max(pc(new).unwrap_or(1)))
    }

    /// Whether the media diff currently on screen has a PDF side with ≥2 pages on either side — gates
    /// the `J`/`K` key and its footer/help hint.
    pub(crate) fn media_diff_can_page(&self) -> bool {
        self.media_diff_max_page_count().is_some_and(|n| n > 1)
    }

    /// Whether the media diff's side-by-side view isn't just *targeted* (`diff_media_active` — true
    /// the instant the diff opens, before anything has landed) but is actually **showing at least one
    /// picture side by side right now**: the landed outcome is `Ready` and at least one side is a
    /// `Picture`. Gates the `s` key and its footer/help hint ([[hint-shown-iff-key-acts]]): while the
    /// worker is still computing, or the outcome degraded to the binary summary, or both sides landed
    /// as `Absent`/`Failed`/`PageMissing`, there is no layout for `s` to cycle between.
    /// `media_diff_can_page` derives its own narrower gate from the same `media_diff_ready_sides` —
    /// this is that fn's sibling for `s`, which only needs *a* picture, not a multi-page one.
    pub(crate) fn media_diff_showing_pictures(&self) -> bool {
        self.diff_media_active()
            && self.media_diff_ready_sides().is_some_and(|(old, new)| {
                matches!(old, MediaDiffSide::Picture(_)) || matches!(new, MediaDiffSide::Picture(_))
            })
    }

    /// `J`/`K` (and `PageDown`/`PageUp` while the media diff's side-by-side view is active): turn both
    /// sides' PDF page together, clamped to the larger of the two sides' own page counts. No-op for
    /// anything but a multi-page PDF diff — re-derived here rather than trusted from the caller, so a
    /// keypress queued from a frame where paging *was* available can't wrap past a since-shrunk page
    /// count (e.g. `n`/`N` landed on a single-page file in between).
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub(crate) fn media_diff_page_turn(&mut self, dir: i32) {
        let Some(max_pages) = self.media_diff_max_page_count() else {
            return;
        };
        if max_pages <= 1 {
            return;
        }
        let cur = self.tab.diff_media_page.max(1);
        let next = if dir >= 0 {
            (cur + 1).min(max_pages)
        } else {
            cur.saturating_sub(1).max(1)
        };
        if next != cur {
            self.tab.diff_media_page = next;
        }
    }

    /// `s` while the media diff's side-by-side view is active: auto → side → stack → auto. Flashes
    /// the layout it switched to, mirroring `App::cycle_diff_layout`'s own flash for the text diff's
    /// `s`.
    pub(crate) fn cycle_media_diff_layout(&mut self) {
        self.media_diff_layout = self.media_diff_layout.next();
        let label = crate::i18n::tr(self.lang, media_layout_msg(self.media_diff_layout));
        self.flash = Some(label.into());
    }

    /// The `Msg` naming what `s` would switch the media diff's layout **to** — the footer's dynamic
    /// `s:<next>` fragment, mirroring `R`'s own `diff_view_cycle_hint`.
    pub(crate) fn media_diff_layout_next_msg(&self) -> crate::i18n::Msg {
        media_layout_msg(self.media_diff_layout.next())
    }

    /// Whether the GitDiff footer (`ui/status.rs::mode_footer`) should show the reduced media/
    /// binary-summary hint set (`n/N`, `x`(write), `q/Esc` — plus `s`/`J`/`K`/`R` inserted dynamically
    /// only while the side-by-side view is active) instead of the classic scrollable-diff hint set.
    /// True for the media diff's own side-by-side view (`diff_media_active`) and for any
    /// binary-summary-eligible kind's `Source` representation (`diff_binary_summary_eligible`) — the
    /// latter's raw line diff is *always* empty (git/jj never line-diff a binary file), so — unlike
    /// `diff_rendered_active`'s own gate — this needs no per-frame "is the diff actually empty right
    /// now" check (which would need `&mut self`, unavailable to the footer) to be correct.
    pub(crate) fn diff_footer_is_media_or_summary(&self) -> bool {
        if self.diff_media_active() {
            return true;
        }
        let Some(PreviewKind::GitDiff(path)) = self.tab.preview_kind.as_ref() else {
            return false;
        };
        self.diff_binary_summary_eligible(path)
    }

    /// The `Msg` naming a media diff's **old**-side base (`MediaBase`, from the landed outcome) — the
    /// caption label.
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub(crate) fn media_base_msg(base: MediaBase) -> crate::i18n::Msg {
        match base {
            MediaBase::Head => crate::i18n::Msg::MediaBaseHead,
            MediaBase::JjParent => crate::i18n::Msg::MediaBaseJjParent,
            MediaBase::FollowStart => crate::i18n::Msg::MediaBaseFollowStart,
        }
    }

    /// The `Msg` naming a media diff's **new** side — always the live working copy (never a follow
    /// snapshot): git's "working tree" or jj's "working copy (@)", decided the same way
    /// `resolve_old_bytes` decides the *old* side's label.
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub(crate) fn media_new_side_msg(&self) -> crate::i18n::Msg {
        if is_jj(&self.tab.root) {
            crate::i18n::Msg::MediaBaseWorkCopyJj
        } else {
            crate::i18n::Msg::MediaBaseWorkTreeGit
        }
    }
}

/// Shared by `App::cycle_media_diff_layout`'s flash and `App::media_diff_layout_next_msg`'s footer
/// hint, so the two can never name the layout differently.
fn media_layout_msg(l: MediaDiffLayout) -> crate::i18n::Msg {
    match l {
        MediaDiffLayout::Auto => crate::i18n::Msg::MediaLayoutAuto,
        MediaDiffLayout::Side => crate::i18n::Msg::MediaLayoutSide,
        MediaDiffLayout::Stack => crate::i18n::Msg::MediaLayoutStack,
    }
}

/// Every `media-diff://` `md_image_cache` key an outcome's own pictures still reference — used by
/// `App::apply_media_diff` to prune everything else.
fn live_cache_keys(outcome: &MediaDiffOutcome) -> HashSet<PathBuf> {
    let mut set = HashSet::new();
    if let MediaDiffOutcome::Ready { old, new, .. } = outcome {
        if let MediaDiffSide::Picture(p) = old {
            set.insert(p.cache_key.clone());
        }
        if let MediaDiffSide::Picture(p) = new {
            set.insert(p.cache_key.clone());
        }
    }
    set
}

/// The [`MediaDiffKind`] a resolved [`PreviewKind`] maps to, or `None` for anything that isn't
/// picture-capable (video/archive/table/text/code/markdown/unsupported/…) — those degrade to
/// [`MediaDiffComputed::Summary`] instead of being paired up.
fn classify_kind(kind: &PreviewKind) -> Option<MediaDiffKind> {
    match kind {
        PreviewKind::Image(_) => Some(MediaDiffKind::Image),
        PreviewKind::Svg(_) => Some(MediaDiffKind::Svg),
        PreviewKind::Pdf(_) => Some(MediaDiffKind::Pdf),
        _ => None,
    }
}

/// Which base to read the old side from, and what it actually resolved to. Whether git or jj answers
/// is decided the same way every other backend-facing question in the app is (`crate::vcs::detect`),
/// not carried on the request — a pure function of `root`, costing nothing extra to call here.
///
/// Unlike `App::compute_md_diff`'s baseline handling, a [`DiffBaseline::Empty`] falls back to the
/// committed baseline (`base_contents`) here rather than becoming "no old side": a media diff has no
/// line/block-level fallback to defer to the way the Markdown/text diff does, so pairing against
/// `HEAD`/`@-` is strictly more useful than showing "new file" for one that in fact has real history
/// — the caption always reports whichever base was actually used (`MediaBase`).
///
/// [`DiffBaseline::FollowHead`] gets the identical fallback when `crate::git::blob_at` returns
/// `None` — **not** `unwrap_or_default()` into an empty byte vector. Text can default to empty
/// because an empty old side just means "all added", but for a picture an empty `Vec<u8>` is a
/// zero-byte file that fails to decode, not "absent": the old side would render as
/// [`MediaDiffSideDecoded::Failed`] instead of [`MediaDiffSideDecoded::Absent`], and `old_len` would
/// report `Some(0)` instead of `None`. `blob_at` returning `None` means either the file genuinely
/// didn't exist at the pinned follow-start `sha` (still resolves to `Absent`, correctly relabeled) or
/// the lookup itself failed for an unrelated reason — the committed-baseline fallback is truthful
/// either way.
fn resolve_old_bytes(
    baseline: &DiffBaseline,
    root: &Path,
    path: &Path,
) -> (Option<Vec<u8>>, MediaBase) {
    let vcs_base = if is_jj(root) {
        MediaBase::JjParent
    } else {
        MediaBase::Head
    };
    match baseline {
        DiffBaseline::Vcs => (crate::vcs::base_contents(root, path), vcs_base),
        DiffBaseline::FollowSnapshot(bytes) => (Some(bytes.clone()), MediaBase::FollowStart),
        DiffBaseline::FollowHead { sha } => {
            #[cfg(feature = "git")]
            {
                match crate::git::blob_at(root, sha, path) {
                    Some(bytes) => (Some(bytes), MediaBase::FollowStart),
                    None => (crate::vcs::base_contents(root, path), vcs_base),
                }
            }
            #[cfg(not(feature = "git"))]
            {
                let _ = sha;
                (None, vcs_base)
            }
        }
        DiffBaseline::Empty => (crate::vcs::base_contents(root, path), vcs_base),
    }
}

/// `compute_media_diff_with_cap`'s byte/cap/base half: both sides' bytes (or why each is missing),
/// their lengths, whether the new side exists as a regular file, whether either side is over `cap`,
/// whether the two sides are byte-identical, and which base the old side was actually read against.
/// No classification, no decoding — see `classify_and_decode_media_diff` for the other half.
struct MediaDiffByteResolution {
    old_bytes: Option<Vec<u8>>,
    new_bytes: Option<Vec<u8>>,
    new_exists: bool,
    over_cap: bool,
    same_bytes: bool,
    old_len: Option<u64>,
    new_len: Option<u64>,
    base: MediaBase,
}

/// Resolves everything `compute_media_diff_with_cap` needs to know before it can even ask what kind
/// of picture this is: both sides' bytes, sizes, existence, and the cap check. Pure extraction of
/// that half of the original function — same order of operations, same I/O, nothing moved earlier or
/// later.
fn resolve_media_diff_bytes(req: &MediaDiffRequest, cap: u64) -> MediaDiffByteResolution {
    // The old side has no size-only API to check first — `resolve_old_bytes` always hands back
    // the full blob or nothing, so its length is only known *after* reading it. The new side is a
    // plain file on disk, so `fs::metadata` gets its size for free: a file already over `cap` is
    // never read into memory at all (skipping a multi-hundred-MB read that would be thrown away
    // immediately).
    let (old_bytes, base) = resolve_old_bytes(&req.baseline, &req.root, &req.path);
    let old_len = old_bytes.as_ref().map(|b| b.len() as u64);
    let old_over_cap = old_len.is_some_and(|n| n > cap);

    let new_meta = std::fs::metadata(&req.path).ok();
    let new_meta_len = new_meta.as_ref().map(|m| m.len());
    // Existence, independent of whether the bytes get read (an over-cap file still exists, its
    // read is just skipped below) — decides which classification rule applies just below
    // (`Config::resolve_preview`'s path rules for an existing file, vs. sniffing the *old* bytes
    // for one that's gone). Requires `is_file()`, not just "metadata answers at all": a directory
    // now sitting where a regular file used to be (a rename/rewrite race) still has metadata but
    // can't be read/classified as a picture — treating it as "exists" would route through
    // `resolve_preview`'s path rules, whose mime sniff fails outright on a directory and degrades
    // the whole diff to the binary summary even when the *old* side is a perfectly good picture.
    // Treating it as absent instead sniffs the old bytes, same as a genuinely deleted file.
    let new_exists = new_meta.is_some_and(|m| m.is_file());
    let new_over_cap = new_meta_len.is_some_and(|n| n > cap);
    let new_bytes = if new_over_cap {
        None // known too large from metadata alone — never read.
    } else {
        std::fs::read(&req.path).ok()
    };
    // The metadata-derived size when the file was never read (over cap); otherwise the actual
    // bytes' own length. Both agree except on a rare mid-write race, where the bytes' length is
    // the more honest of the two since it's what was actually decoded.
    let new_len = if new_over_cap {
        new_meta_len
    } else {
        new_bytes.as_ref().map(|b| b.len() as u64)
    };
    let over_cap = old_over_cap || new_over_cap;
    // Differing lengths are conclusive without a byte-for-byte compare; only two sides that were
    // both actually read (never true for an over-cap or absent side) and already length-match are
    // compared further.
    let same_bytes = match (&old_bytes, &new_bytes) {
        (Some(o), Some(n)) => o.len() == n.len() && o == n,
        _ => false,
    };

    MediaDiffByteResolution {
        old_bytes,
        new_bytes,
        new_exists,
        over_cap,
        same_bytes,
        old_len,
        new_len,
        base,
    }
}

/// `compute_media_diff_with_cap`'s classify+decode half: resolves the picture kind (from the new
/// file when it exists, or by sniffing the old bytes when it doesn't), degrades to
/// [`MediaDiffComputed::Summary`] for a non-picture kind or an over-cap side, and otherwise decodes
/// both sides independently. Pure extraction of that half of the original function — same order of
/// operations, same I/O, nothing moved earlier or later.
fn classify_and_decode_media_diff(
    req: &MediaDiffRequest,
    bytes: MediaDiffByteResolution,
) -> MediaDiffComputed {
    let MediaDiffByteResolution {
        old_bytes,
        new_bytes,
        new_exists,
        over_cap,
        same_bytes,
        old_len,
        new_len,
        base,
    } = bytes;

    let preview_kind = if new_exists {
        crate::config::resolve_preview_kind(
            &req.preview_rules,
            req.preview_commands,
            &req.path,
            None,
        )
    } else {
        crate::config::resolve_preview_kind(
            &req.preview_rules,
            req.preview_commands,
            &req.path,
            old_bytes.as_deref(),
        )
    };

    let Some(kind) = classify_kind(&preview_kind) else {
        return MediaDiffComputed::Summary {
            base,
            same_bytes,
            old_len,
            new_len,
        };
    };
    if over_cap {
        return MediaDiffComputed::Summary {
            base,
            same_bytes,
            old_len,
            new_len,
        };
    }

    let old = decode_side(
        kind,
        old_bytes.as_deref(),
        &req.path,
        req.page,
        req.raster_px,
        KeySide::Old,
    );
    let new = decode_side(
        kind,
        new_bytes.as_deref(),
        &req.path,
        req.page,
        req.raster_px,
        KeySide::New,
    );
    MediaDiffComputed::Ready {
        kind,
        base,
        same_bytes,
        old,
        new,
    }
}

/// Whether `root` is answered by the jj backend — `crate::vcs::VcsKind::Jj` only exists behind
/// `feature = "git"`, so this stays a plain `false` (git is the only backend) on a no-git build,
/// exactly like every other jj-only branch in this module.
fn is_jj(root: &Path) -> bool {
    #[cfg(feature = "git")]
    {
        matches!(crate::vcs::detect(root), crate::vcs::VcsKind::Jj)
    }
    #[cfg(not(feature = "git"))]
    {
        let _ = root;
        false
    }
}

/// Decode one side of a picture-capable diff. `None` bytes (the side is absent — a new/untracked
/// file's old side, or a deleted file's new side) is the only case common to every kind.
fn decode_side(
    kind: MediaDiffKind,
    bytes: Option<&[u8]>,
    path: &Path,
    page: u32,
    raster_px: (u32, u32),
    side: KeySide,
) -> MediaDiffSideDecoded {
    let Some(bytes) = bytes else {
        return MediaDiffSideDecoded::Absent;
    };
    let hash = crate::preview::media_diff::fnv1a64(bytes);
    match kind {
        MediaDiffKind::Image => decode_image_side(bytes, hash, page, side),
        MediaDiffKind::Svg => decode_svg_side(bytes, path, raster_px, hash, page, side),
        MediaDiffKind::Pdf => decode_pdf_side(bytes, page, raster_px, hash, side),
    }
}

/// A raw raster image (PNG/JPG/…) or an animated GIF — GIF is tried first (mirrors
/// `App::ensure_md_image`'s own "GIF, else still image" order for an inline Markdown image), since a
/// multi-frame GIF also successfully decodes as a still (its first frame) and would otherwise never
/// be detected as animated. `raster_px` doesn't apply — the decode is at the image's own native
/// resolution, exactly like every other raster-image preview path in konoma.
fn decode_image_side(bytes: &[u8], hash: u64, page: u32, side: KeySide) -> MediaDiffSideDecoded {
    use image::GenericImageView;
    if let Some((frames, canvas_px)) = crate::preview::image::decode_gif_bytes_inline(bytes) {
        let Some((first, _)) = frames.first() else {
            return MediaDiffSideDecoded::Failed {
                reason: "empty gif".to_string(),
            };
        };
        let first_frame = first.clone();
        let key = crate::preview::media_diff::media_diff_url(side, hash, page, None);
        return MediaDiffSideDecoded::Picture(Box::new(MediaDiffPictureDecoded {
            // The GIF's own logical-screen size (read from the header) — not `first.dimensions()`,
            // which the inline decode budget may have downscaled for memory.
            natural_px: canvas_px,
            bytes: bytes.len() as u64,
            page_count: None,
            image: first_frame,
            frames: Some(frames),
            svg: None,
            cache_key: PathBuf::from(key),
        }));
    }
    match crate::preview::image::decode_static_bytes(bytes) {
        Some(img) => {
            let (w, h) = img.dimensions();
            let key = crate::preview::media_diff::media_diff_url(side, hash, page, None);
            MediaDiffSideDecoded::Picture(Box::new(MediaDiffPictureDecoded {
                natural_px: (w, h),
                bytes: bytes.len() as u64,
                page_count: None,
                image: img,
                frames: None,
                svg: None,
                cache_key: PathBuf::from(key),
            }))
        }
        None => MediaDiffSideDecoded::Failed {
            reason: "image decode failed".to_string(),
        },
    }
}

/// An SVG side: natural size is the intrinsic (viewBox) size, never the raster's — the same
/// distinction `MdImgEntry::layout_px` already draws for a mermaid/math render. Rasterized at
/// `raster_px`'s larger side (`preview::svg::rasterize_trusted` takes a single max-side target, same as
/// every other SVG call site in this codebase).
fn decode_svg_side(
    bytes: &[u8],
    path: &Path,
    raster_px: (u32, u32),
    hash: u64,
    page: u32,
    side: KeySide,
) -> MediaDiffSideDecoded {
    let max_px = raster_px.0.max(raster_px.1).max(1);
    // Git's old version of a file: drawn by a supervised child process, like any SVG from a file.
    let img = match crate::preview::svg::rasterize_untrusted(bytes, path, max_px, &|| false) {
        Ok(img) => img,
        Err(why) => {
            return MediaDiffSideDecoded::Failed {
                reason: why.reason().to_string(),
            }
        }
    };
    // A size that only drawing reveals (no width/height/viewBox) falls back to the raster's.
    let natural_px = crate::preview::svg::intrinsic_size_bytes(bytes).unwrap_or_else(|| {
        use image::GenericImageView;
        img.dimensions()
    });
    let key = crate::preview::media_diff::media_diff_url(side, hash, page, Some(raster_px));
    MediaDiffSideDecoded::Picture(Box::new(MediaDiffPictureDecoded {
        natural_px,
        bytes: bytes.len() as u64,
        page_count: None,
        image: img,
        frames: None,
        svg: Some(Arc::new(bytes.to_vec())),
        cache_key: PathBuf::from(key),
    }))
}

/// A PDF side. `page` beyond this side's own page count is [`MediaDiffSideDecoded::PageMissing`],
/// judged independently per side — the two sides can have different page counts. Natural size is the
/// page's own size in PDF points (`page_dimensions_bytes`), not the raster's. `render_page_bytes` is
/// `hayro`-only (no `qlmanage`/`sips` fallback) — see that function's own doc comment for why.
fn decode_pdf_side(
    bytes: &[u8],
    page: u32,
    raster_px: (u32, u32),
    hash: u64,
    side: KeySide,
) -> MediaDiffSideDecoded {
    let Some(page_count) = crate::preview::pdf::page_count_bytes(bytes) else {
        return MediaDiffSideDecoded::Failed {
            reason: "invalid pdf".to_string(),
        };
    };
    if page < 1 || page > page_count {
        return MediaDiffSideDecoded::PageMissing;
    }
    let Some((pw, ph)) = crate::preview::pdf::page_dimensions_bytes(bytes, page) else {
        return MediaDiffSideDecoded::Failed {
            reason: "pdf page dimensions unavailable".to_string(),
        };
    };
    let Some(img) = crate::preview::pdf::render_page_bytes(bytes, page) else {
        return MediaDiffSideDecoded::Failed {
            reason: "pdf page render failed".to_string(),
        };
    };
    let key = crate::preview::media_diff::media_diff_url(side, hash, page, Some(raster_px));
    MediaDiffSideDecoded::Picture(Box::new(MediaDiffPictureDecoded {
        natural_px: (pw.ceil().max(1.0) as u32, ph.ceil().max(1.0) as u32),
        bytes: bytes.len() as u64,
        page_count: Some(page_count),
        image: img,
        frames: None,
        svg: None,
        cache_key: PathBuf::from(key),
    }))
}

#[cfg(test)]
#[path = "media_diff_tests.rs"]
mod tests;
