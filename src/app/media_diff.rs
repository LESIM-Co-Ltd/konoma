//! The image/PDF/SVG "diff" computation offloaded to a background worker
//! (`docs/FEATURE-MEDIA-DIFF.md` §3) — fetching each side's bytes (git/jj/follow-snapshot for the
//! old side, the filesystem for the new one), classifying the picture kind, and decoding into pixels
//! is I/O plus real CPU work that must never run on the UI thread (design principle #4). Follows the
//! exact shape `src/app/md_diff.rs` already established for the Markdown block-diff (see that
//! module's own doc comment for the general pattern this mirrors): a `gen`-tagged request/result
//! pair, a `_pending` guard against duplicate dispatch, a synchronous fallback when no `Sender` is
//! attached (tests), and a `compute_or_fallback` panic net.
//!
//! The baseline (which bytes count as "old") is resolved by `App::diff_baseline` — the exact same
//! method the Markdown block-diff's `Rendered` presentation uses — rather than re-deriving the
//! follow-session branching a second time (`App::media_diff_baseline`, below).
//!
//! `App::compute_media_diff` is the pure computation (no `&self`), shared byte-identically by the
//! worker thread and the synchronous fallback. Its result still carries decoded pixel data
//! (`MediaDiffComputed`/`MediaDiffSideDecoded`) — `App::apply_media_diff` is where that data actually
//! lands in `md_image_cache` (under a `media_diff_url` key, mirroring how `ensure_mermaid_fence_render`/
//! `apply_md_image` already insert a mermaid/math render) and the lightweight, pixel-free
//! `MediaDiffOutcome` is what stays resident in `App::media_diff_landed`.
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
    /// `raster_px` is normalized first (`normalize_raster_px`) so a caller-supplied box that the
    /// decode doesn't actually depend on (a raw raster image — PNG/JPG/GIF — always decodes at its
    /// own native resolution) never causes a spurious re-decode: a terminal resize changes the pane's
    /// pixel box on every frame it's still settling, and only SVG/PDF's rasterization genuinely needs
    /// to redo any work for that (`docs/FEATURE-MEDIA-DIFF.md`'s own perf note).
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
    /// to a caller-chosen pixel box; [`MediaDiffKind::Image`] (and anything not classified as
    /// picture-capable at all, which degrades to [`MediaDiffComputed::Summary`] regardless of
    /// `raster_px`) always decodes at its own native resolution (`decode_image_side`'s own doc
    /// comment: "raster_px doesn't apply").
    ///
    /// Prefers the worker's own landed classification (`media_landed_outcome_for`) over the cheaper
    /// `diff_target_kind` when one is on hand, since a **deleted** file whose extension isn't
    /// glob-recognized can only be classified by the worker's byte-sniff (`docs/FEATURE-MEDIA-DIFF.md`
    /// §2 — `diff_target_kind`'s own `Config::resolve_preview` fallback can't see that at all for a
    /// path that doesn't exist). Before that first landing, such a path normalizes to `(0, 0)` same
    /// as "not picture-capable" would — if it then turns out to be a `Svg`/`Pdf` after all, that
    /// first request rasterizes tiny (`decode_svg_side`/`decode_pdf_side` floor `raster_px` at 1px)
    /// and self-corrects the moment the *next* poll sees the now-landed real kind and asks again with
    /// the real box — a one-frame transient, not a lasting bug, and no worse than the "computing…"
    /// placeholder every other not-yet-landed path already shows for a frame or two.
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
    /// in flight at a time** (`docs/FEATURE-MEDIA-DIFF.md`'s "latest wins" perf note): if no worker
    /// is currently running (`!media_diff_worker_busy`), this dispatches immediately
    /// (`dispatch_media_diff`); otherwise this instead **coalesces** into the single
    /// `media_diff_queued` slot — overwriting whatever want, if any, was already waiting there — and
    /// `apply_media_diff` dispatches it the moment the busy worker's result lands (accepted *or*
    /// stale — `media_diff_worker_busy`'s own doc comment on why staleness doesn't free the slot any
    /// earlier). An AI rewriting an image repeatedly (each retarget/invalidate calling this again
    /// before the previous decode finishes) therefore never piles up concurrent decodes; only the
    /// newest want survives the wait.
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
    /// happens next. The picture *kind* is then classified **once**, from the new file when it
    /// exists (`Config::resolve_preview`'s own rules, by path) or from the old bytes when it doesn't
    /// (`Config::resolve_preview_with`, sniffing the baseline content — `docs/FEATURE-MEDIA-DIFF.md`
    /// §2's "削除されたファイルでも設定駆動で") — both sides of a *modified* file always share one
    /// kind, they are never independently reclassified. A kind that isn't picture-capable, or either
    /// side over the byte cap, degrades to [`MediaDiffComputed::Summary`] (§5's binary summary line);
    /// otherwise each side is decoded independently (a side's own decode failure doesn't take the
    /// other side down with it).
    fn compute_media_diff(req: &MediaDiffRequest) -> MediaDiffComputed {
        Self::compute_media_diff_with_cap(req, crate::preview::pdf::PAGE_COUNT_MAX_BYTES)
    }

    /// `compute_media_diff`'s real body, parameterized on the per-side byte cap purely for
    /// testability — production always passes `PAGE_COUNT_MAX_BYTES`; tests pass an artificially
    /// small cap against an ordinary small fixture (mirrors `preview::pdf::page_count_impl`'s
    /// identical `max_bytes` seam, for the identical reason: exercising the cap without needing to
    /// actually construct/store a real 64 MiB fixture file).
    fn compute_media_diff_with_cap(req: &MediaDiffRequest, cap: u64) -> MediaDiffComputed {
        // The old side has no size-only API to check first — `resolve_old_bytes` (git/jj/follow-
        // snapshot, `Vcs::base_contents`) always hands back the full blob or nothing at all, so its
        // length is only ever known *after* reading it. The **new** side, by contrast, is a plain
        // file on disk: `fs::metadata` gets its size for free, so a file already over `cap` from that
        // alone is never read into memory at all — the read below is skipped outright, not merely
        // discarded afterward (`docs/FEATURE-MEDIA-DIFF.md`'s "1 側 64 MiB 超" perf note: reading a
        // multi-hundred-MB file just to immediately throw it away was real, avoidable I/O on this
        // worker thread).
        let (old_bytes, base) = resolve_old_bytes(&req.baseline, &req.root, &req.path);
        let old_len = old_bytes.as_ref().map(|b| b.len() as u64);
        let old_over_cap = old_len.is_some_and(|n| n > cap);

        let new_meta = std::fs::metadata(&req.path).ok();
        let new_meta_len = new_meta.as_ref().map(|m| m.len());
        // Existence, independent of whether the bytes end up read at all (over-cap skips the read
        // below but the file still exists) — used just below to decide which classification rule
        // this reaches for (`Config::resolve_preview`'s own path-based rules for an existing file,
        // vs. sniffing the *old* bytes for one that's gone, `docs/FEATURE-MEDIA-DIFF.md` §2). Using
        // `new_bytes.is_some()` for that instead (as this used to) would misclassify a still-existing
        // over-cap file as if it had been deleted, once its own read is skipped below.
        //
        // **Requires `is_file()`, not just "metadata answers at all"**: a directory now sitting where
        // the new side's path used to be a regular file (a rename/rewrite race, or an agent replacing
        // a file with a folder) still has metadata (a directory has a size and mtime too), but is
        // never something this side can meaningfully read/classify — treating it as "exists" would
        // route classification through `Config::resolve_preview`'s own path rules, whose mime match
        // needs to actually read the path's bytes (`infer::get_from_path`) and fails outright on a
        // directory, so the whole diff would degrade to the binary-summary line even when the *old*
        // side is a perfectly good, classifiable picture. Treating it as absent instead routes
        // classification through the old bytes' own sniff (same as a genuinely deleted new side), so
        // a still-valid old picture is shown alongside an `Absent` new side rather than neither.
        let new_exists = new_meta.is_some_and(|m| m.is_file());
        let new_over_cap = new_meta_len.is_some_and(|n| n > cap);
        let new_bytes = if new_over_cap {
            None // known too large from metadata alone — never read.
        } else {
            std::fs::read(&req.path).ok()
        };
        // `new_len`: the metadata-derived size when the file was never read (over cap — there is no
        // `new_bytes` to measure); otherwise the actual bytes' own length (an already-successful read
        // measured directly, no second `stat`). Both agree when the file didn't change size out from
        // under this call, which is the overwhelmingly common case; on the rare mid-write race where
        // it did, the bytes' own length (when read) is the more honest of the two, since it's what
        // was actually decoded (or not).
        let new_len = if new_over_cap {
            new_meta_len
        } else {
            new_bytes.as_ref().map(|b| b.len() as u64)
        };
        let over_cap = old_over_cap || new_over_cap;
        // Lengths differing is conclusive ("not the same") without a byte-for-byte compare; only two
        // sides that were both actually read (never true for an over-cap side, which never got a
        // `new_bytes`/full `old_bytes` populated for that side, and never true for an absent side
        // either) and whose lengths already match are compared further at all.
        let same_bytes = match (&old_bytes, &new_bytes) {
            (Some(o), Some(n)) => o.len() == n.len() && o == n,
            _ => false,
        };

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

    /// Apply a media-diff result from the worker thread (or the synchronous fallback).
    ///
    /// **Always** frees the one worker slot first (`media_diff_worker_busy = false`) and, if a want
    /// coalesced behind it while it ran (`media_diff_queued`), immediately dispatches that one next —
    /// this runs whether or not `res` itself turns out to be stale, since either way the worker slot
    /// really is free now and `docs/FEATURE-MEDIA-DIFF.md`'s "at most one worker in flight, latest
    /// wins" has to keep making forward progress on whatever was actually asked for most recently
    /// (`kick_media_diff`'s own doc comment).
    ///
    /// Discards a stale generation (mirrors `App::apply_md_diff`) — a newer request/invalidation
    /// superseded this one, so `res.computed` itself is simply dropped. Otherwise:
    ///
    /// 1. Moves every decoded side's pixel data into `md_image_cache` under its `media_diff_url` key
    ///    — the same pre-insert-then-`apply_md_image` shape `ensure_mermaid_fence_render` already
    ///    uses for a synthetic key, so the existing decode/encode/kitty-id machinery draws it exactly
    ///    like an inline Markdown image once phase B wires up the render side.
    /// 2. Drops every `media-diff://` key this landing does **not** reference — an earlier landing
    ///    for a different `(path, page, raster_px)` (a file switch, a page turn, a resize) would
    ///    otherwise leave its own now-unreachable rasters resident forever
    ///    (`docs/FEATURE-MEDIA-DIFF.md` §4: "対象を変えたら media-diff:// のキーを消す"). Never
    ///    touches a mermaid/math key (`is_media_diff_url` alone gates the predicate).
    /// 3. Stores the lightweight (pixel-free) outcome in `media_diff_landed`.
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
                // Pre-place the entry, exactly like `ensure_mermaid_fence_render` does before
                // handing its result to `apply_md_image` — that fn drops any result whose key isn't
                // already present (its own guard against reviving an evicted entry), so the insert
                // has to happen here, not inside it.
                self.md_image_cache.entry(p.cache_key.clone()).or_default();
                // No transparency handling here anymore: each side is drawn with exactly the same
                // pixels the ordinary preview would draw for that kind (`decode_side`/`preview::
                // pdf`/`preview::svg`) — a PDF's own renderer now composites its pages onto opaque
                // white before this ever sees them (`preview::pdf::pixmap_to_dynamic_image`'s own
                // doc comment has the "why"), and SVG stays exactly as transparent as the ordinary
                // preview renders it, on every protocol including kitty. There used to be a
                // diff-only `flatten_transparent_to_white` step here (skipped on kitty) working
                // around the PDF half of that; fixing it once at the render source (rather than
                // patching it up again here) means this diff never special-cases a picture kind
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
    /// have changed — mirrors `App::invalidate_md_diff`'s own doc comment on why `_pending` is also
    /// cleared here (otherwise `poll_media_diff`'s dedup check would keep declining to kick a fresh
    /// request for the identical `(path, page, raster_px)`, since that check is keyed on the request
    /// identity, not `gen` — a poll for the very same want right after a baseline change must still
    /// produce a *fresh* dispatch, not be mistaken for "already covered" by a now-stale one).
    ///
    /// Deliberately does **not** touch `media_diff_worker_busy`/`media_diff_queued`: a worker thread
    /// already dispatched before this runs is still physically executing against the *old* baseline
    /// and cannot be recalled, so the one worker slot stays occupied until it actually reports back
    /// (`App::apply_media_diff` frees it and dispatches whatever coalesced behind it in the
    /// meantime) — `media_diff_worker_busy`'s own doc comment has the full reasoning.
    pub(crate) fn invalidate_media_diff(&mut self) {
        self.media_diff_gen = self.media_diff_gen.wrapping_add(1);
        self.media_diff_landed = None;
        self.media_diff_pending = None;
    }

    /// Removes every `media-diff://` entry from `md_image_cache` unconditionally — never touches a
    /// mermaid/math key (`is_media_diff_url` alone gates the predicate, mirroring `App::apply_media_
    /// diff`'s own `retain`). Called wherever the diff surface stops being able to ever repopulate
    /// them itself, which `App::apply_media_diff`'s own landing-triggered prune (`live_cache_keys`)
    /// cannot cover because there is no further landing to piggyback on.
    ///
    /// **The actual rule, unconditionally**: `media-diff://` pictures are dropped the moment the
    /// *active tab's own view* is no longer a media diff — whether that's because the diff was
    /// closed, retargeted to a non-media file, **or the active tab was switched to a different
    /// one** (`App::invalidate_diff_caches` runs on every tab switch via `load_active` →
    /// `refresh_fs_after_tab_switch`, not only on a genuine retarget of the *same* tab's target).
    /// `media_diff_landed`/`media_diff_pending`/`md_image_cache` are all `App`-level, not per-tab,
    /// so this is true even for a tab that never itself changed what it's diffing — switching away
    /// from it to *any* other tab (even a plain Tree one) frees its still-open media diff's pixels
    /// exactly the same way `back_to_tree` does, and switching back re-kicks a fresh computation
    /// (`switching_to_another_tab_and_back_redraws_both_pictures` pins the re-kick half of this).
    /// This is deliberate memory-vs-recompute behavior, not a bug: at most one tab's media-diff
    /// pixels are ever resident at a time.
    ///
    /// Concretely, called from:
    ///
    /// - `App::back_to_tree` (q/Esc closing the diff back to the tree, or to the Git hub via
    ///   `App::close_git_diff`'s own call into it) — the diff surface is left altogether, so nothing
    ///   will ever poll/land a media diff again until some *unrelated* one is opened later, which
    ///   could be a long time (or never, for the rest of the session).
    /// - `App::invalidate_diff_caches`, whenever the tab's currently-active view isn't a
    ///   media-diff-capable `GitDiff` target — that covers both an in-place retarget (an image/PDF/
    ///   SVG diff switching to, say, a Markdown/code file) *and* a tab switch landing on any tab
    ///   whose own target isn't itself a media diff (see above) — either way that view will never
    ///   land a fresh `Ready` picture of its own, so `apply_media_diff`'s prune never runs again for
    ///   the previous target's now-orphaned keys either.
    ///
    /// Before this existed, either case left the last-viewed media diff's decoded rasters resident in
    /// `md_image_cache` forever (`docs/FEATURE-MEDIA-DIFF.md` §4's "対象を変えたら media-diff:// の
    /// キーを消す" — the design called for this on every retarget, not only the ones that happen to
    /// land a new picture).
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
    /// media` uses this to detect a landed `Picture` whose pixels were evicted from underneath it
    /// and re-kick a fresh computation instead of silently drawing nothing. Confirmed (by direct
    /// instrumentation, not just reasoning) that this can genuinely happen: `App::enter_preview`'s
    /// file-switch clear fires whenever the *previous* preview target's path differs from the one
    /// being entered — which an R→Preview→R round trip on the diff's **own** target does *not*
    /// trigger (`same_file` is true there, so the cache survives), but any path that lands on a
    /// *different* file's preview first (`Ctrl-n`/`Ctrl-p` file paging, a Markdown link, a
    /// bookmark jump, …) while the `media-diff://` keys are still resident does. This accessor,
    /// and the re-kick it feeds, defend against that broader case — see `evict_md_image_cache_key_
    /// for_test` for how the test suite reproduces it directly rather than via a specific keypress
    /// sequence.
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

    /// Test-only: every `media-diff://` key currently in `md_image_cache` — lets a test discover
    /// the *real* cache keys a production render actually landed (which depend on the render
    /// path's own raster target, not one a test would have to guess/duplicate) rather than
    /// re-deriving them by calling `poll_media_diff` a second time with a possibly-mismatched
    /// `raster_px` (a mismatch there would itself trigger a fresh, unrelated poll — silently
    /// defeating a test that means to isolate the re-kick path specifically).
    #[cfg(test)]
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub(crate) fn md_image_cache_media_diff_keys_for_test(&self) -> Vec<PathBuf> {
        self.md_image_cache
            .keys()
            .filter(|k| crate::preview::media_diff::is_media_diff_url(&k.to_string_lossy()))
            .cloned()
            .collect()
    }

    /// The terminal's font cell size in pixels, or `None` when there is no picker at all (a
    /// terminal with no graphics protocol, or images disabled) — `ui/preview.rs::render_gitdiff_
    /// media` degrades to the binary summary line in that case
    /// (`docs/FEATURE-MEDIA-DIFF.md` §4's "画像を描けない端末・設定").
    pub(crate) fn picker_cell_px(&self) -> Option<(u32, u32)> {
        self.picker.as_ref().map(|p| {
            let f = p.font_size();
            (f.width as u32, f.height as u32)
        })
    }

    /// Whether the diff's `Rendered` presentation is currently the image/PDF/SVG side-by-side view
    /// (as opposed to decorated Markdown blocks) — `ui/preview.rs::render_gitdiff` reads this to
    /// pick `render_gitdiff_media` over `render_diff_rendered`.
    ///
    /// `Rendered` is only ever *set* (`App::round_diff_view`/`App::apply_diff_view`) on a target
    /// whose representation list (`App::diff_representations`) actually contains it, and the only
    /// kinds that do are Markdown (decorated blocks) and the media-capable ones (Image/Svg/Pdf, or
    /// an ambiguous deleted file that might turn out to be one) — so "not literally classified as
    /// Markdown" is a safe, self-contained test here: it needs no knowledge of
    /// `diff_representations`' own worker-outcome fallback (for a deleted file) to be correct,
    /// since `resolve_preview` on a deleted **Markdown** path still correctly returns `Markdown`
    /// (glob-matched by filename, not content — unaffected by the file's existence), while every
    /// other case this fn needs to say "media" for either resolves to something else already, or
    /// (an ambiguous deleted binary) resolves to `CanNotPreview`, which also isn't `Markdown`.
    pub(crate) fn diff_media_active(&self) -> bool {
        if !self.diff_rendered_active() {
            return false;
        }
        let Some(PreviewKind::GitDiff(path)) = self.tab.preview_kind.as_ref() else {
            return false;
        };
        !matches!(self.diff_target_kind(path), PreviewKind::Markdown(_))
    }

    /// `Config::resolve_preview(path)`, memoized in `diff_target_kind_cache` — the single read side
    /// of that cache (`App::refresh_diff_target_kind_cache` is the single write side). A cache hit
    /// (`path` matches whatever the diff surface is currently targeting) is the hot, per-frame path
    /// and costs nothing beyond a `PathBuf` comparison and a `Clone` of an already-resolved
    /// `PreviewKind`; a miss (a test, or any other caller asking about a path that isn't the diff's
    /// own current target) falls back to a direct, uncached resolve, so this is always *correct*,
    /// just not always free — exactly like `App::media_diff_baseline`'s reuse of `App::diff_baseline`
    /// is correct regardless of what's cached elsewhere.
    pub(super) fn diff_target_kind(&self, path: &Path) -> PreviewKind {
        match &self.diff_target_kind_cache {
            Some((p, k)) if p.as_path() == path => k.clone(),
            _ => self.cfg.resolve_preview(path),
        }
    }

    /// Refreshes `diff_target_kind_cache` for whatever the diff surface is currently targeting
    /// (`self.tab.preview_kind`'s `GitDiff` path, if any) — called from `App::invalidate_diff_caches`,
    /// which runs both on a genuine retarget (`App::open_git_diff_with`, always called *after* it has
    /// already updated `tab.preview_kind` to the new path) and on an invalidation-only refresh (a
    /// working-tree/follow-session change against the still-current target) — one call site covers
    /// both, per `docs/FEATURE-MEDIA-DIFF.md`'s perf note ("resolve once when the diff is (re)targeted
    /// … and again only when invalidate_diff_caches runs"). Clears the cache outright when the diff
    /// surface isn't showing a `GitDiff` at all, so a stale entry can never outlive the target it was
    /// resolved for.
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
    /// two can never drift apart on which kinds get the side-by-side treatment
    /// (`docs/FEATURE-MEDIA-DIFF.md` §6 decision 2). Goes through the same memoized `diff_target_kind`
    /// as every other diff-surface predicate in this module.
    pub(crate) fn diff_media_capable(&self, path: &Path) -> bool {
        classify_kind(&self.diff_target_kind(path)).is_some()
    }

    /// Whether `path`'s Source-representation diff, if the raw line diff comes back **empty**,
    /// should be checked against this module's worker instead of being trusted at face value as
    /// "(no changes)" (`docs/FEATURE-MEDIA-DIFF.md` §5). True for exactly the kinds that have no
    /// text/decorated representation of their own to fall back on (video/archive/table/unsupported/
    /// an image-mode delegated command) — these relied on git's raw line diff alone, which comes
    /// back empty for *any* binary file whether or not it actually changed (`git.rs:902`'s own doc
    /// comment on why), so an empty diff there was never actually proof of "no changes". Markdown/
    /// Code/Text/Mermaid/a text-mode command are excluded: an empty diff there is always genuinely
    /// "no changes" (no worker round-trip needed to say so, and always was correct before this
    /// feature existed). Image/Svg/Pdf are not excluded, but in practice never reach `Source` at all
    /// (`App::round_diff_view` always substitutes `Rendered`) — if a test forces the state anyway,
    /// their own kind resolves to `Some(_)` under `classify_kind`, so they still land on the worker's
    /// real `Ready` outcome (never `Summary`), which the caller handles correctly either way.
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
    /// classification and page-count don't depend on either) — used by `App::diff_representations`
    /// to classify a **deleted** file `resolve_preview` can't (`docs/FEATURE-MEDIA-DIFF.md` §2), and
    /// by the page-turn/footer helpers below, which are called from contexts (footer/help) that
    /// don't have a raster target on hand at all.
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

    /// Whether the media diff currently on screen has a PDF side with ≥2 pages on either side —
    /// gates the `J`/`K` key and its footer/help hint (`docs/FEATURE-MEDIA-DIFF.md` §1/§6).
    pub(crate) fn media_diff_can_page(&self) -> bool {
        self.media_diff_max_page_count().is_some_and(|n| n > 1)
    }

    /// Whether the media diff's side-by-side view isn't just *targeted* (`diff_media_active` — the
    /// target's own kind, true the instant the diff opens, before anything has landed) but is
    /// actually **showing at least one picture side by side right now**: the landed outcome is
    /// `Ready` and at least one side is a `Picture`. Gates the `s` key and its footer/help hint
    /// (`docs/FEATURE-MEDIA-DIFF.md` §6's "並べて表示中" — [[hint-shown-iff-key-acts]]): while the
    /// worker is still computing ("計算中"), or the outcome degraded to the one-line binary summary
    /// (`Summary`/`Unavailable`), or both sides landed as `Absent`/`Failed`/`PageMissing` (nothing to
    /// arrange), there is no layout for `s` to cycle between. `media_diff_can_page` already derives
    /// its own narrower gate from the same `media_diff_ready_sides` — this is that fn's sibling for
    /// `s`, which (unlike paging) only needs *a* picture, not a multi-page one.
    pub(crate) fn media_diff_showing_pictures(&self) -> bool {
        self.diff_media_active()
            && self.media_diff_ready_sides().is_some_and(|(old, new)| {
                matches!(old, MediaDiffSide::Picture(_)) || matches!(new, MediaDiffSide::Picture(_))
            })
    }

    /// `J`/`K` (and `PageDown`/`PageUp` while the media diff's side-by-side view is active): turn
    /// both sides' PDF page together (`docs/FEATURE-MEDIA-DIFF.md` §1's "両側を同じページ番号でそろ
    /// えてめくる"), clamped to the larger of the two sides' own page counts. No-op for anything but
    /// a multi-page PDF diff — re-derived here rather than trusted from the caller, so a keypress
    /// queued from a frame where paging *was* available can't wrap past a since-shrunk page count
    /// (e.g. `n`/`N` landed on a single-page file in between).
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

    /// `s` while the media diff's side-by-side view is active: auto → side → stack → auto
    /// (`docs/FEATURE-MEDIA-DIFF.md` §1/§6). Flashes the layout it switched to, mirroring
    /// `App::cycle_diff_layout`'s own flash for the text diff's `s`.
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
    /// binary-summary hint set (`n/N`, `x`(write), `q/Esc` — plus `s`/`J`/`K`/`R` inserted
    /// dynamically only while the side-by-side view is actually active) instead of the classic
    /// scrollable-diff hint set (`j/k`/`h/l`/`s:unified/split/auto`). True for the media diff's own
    /// side-by-side view (`diff_media_active`) and for any binary-summary-eligible kind's `Source`
    /// representation (`diff_binary_summary_eligible`) — the latter's raw line diff is *always*
    /// empty (git/jj never line-diff a binary file, `docs/FEATURE-MEDIA-DIFF.md` §5's own
    /// "git.rs:902" note), so — unlike `diff_rendered_active`'s own gate — this needs no per-frame
    /// "is the diff actually empty right now" check (which would need `&mut self`, unavailable to
    /// the footer) to be correct.
    pub(crate) fn diff_footer_is_media_or_summary(&self) -> bool {
        if self.diff_media_active() {
            return true;
        }
        let Some(PreviewKind::GitDiff(path)) = self.tab.preview_kind.as_ref() else {
            return false;
        };
        self.diff_binary_summary_eligible(path)
    }

    /// The `Msg` naming a media diff's **old**-side base (`MediaBase`, from the landed outcome) —
    /// the caption label from `docs/FEATURE-MEDIA-DIFF.md` §1's own base-name table.
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub(crate) fn media_base_msg(base: MediaBase) -> crate::i18n::Msg {
        match base {
            MediaBase::Head => crate::i18n::Msg::MediaBaseHead,
            MediaBase::JjParent => crate::i18n::Msg::MediaBaseJjParent,
            MediaBase::FollowStart => crate::i18n::Msg::MediaBaseFollowStart,
        }
    }

    /// The `Msg` naming a media diff's **new** side — always the live working copy (never a follow
    /// snapshot, `docs/FEATURE-MEDIA-DIFF.md` §1's base-name table): git's "working tree" or jj's
    /// "working copy (@)", decided the same way `resolve_old_bytes` decides the *old* side's label.
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

/// Which base to read the old side from, and what it actually resolved to
/// (`docs/FEATURE-MEDIA-DIFF.md` §1's "基準の名前" table / §3's "旧版"). Whether git or jj answers is
/// decided the same way every other backend-facing question in the app is (`crate::vcs::detect`),
/// not carried on the request — it's a pure function of `root` and costs nothing extra to call here
/// (no I/O beyond what `base_contents` itself already does).
///
/// The one place this **doesn't** mirror `App::compute_md_diff`'s own baseline handling: a
/// [`DiffBaseline::Empty`] (dirty-but-too-large-to-snapshot at follow-start, or genuinely no
/// baseline) falls back to the committed baseline (`base_contents`) here, rather than becoming "no
/// old side" the way the Markdown block-diff treats it (`Rendered`'s own "旧版が無い = 全ブロック
/// Insert" contract). A media diff has no line/block-level fallback to defer to the way the
/// Markdown/text diff does — pairing against `HEAD`/`@-` is strictly more useful than showing "新規
/// ファイル" for a file that in fact has real history, and the caption reports whichever base was
/// actually used (`MediaBase`), so this is never silently misleading about what's being compared.
///
/// [`DiffBaseline::FollowHead`] gets the identical committed-baseline fallback when
/// `crate::git::blob_at` returns `None` — **not** `unwrap_or_default()` into an empty byte vector.
/// `App::compute_md_diff` can default to empty because for text an empty old side just means "all
/// added" (§5's own degenerate case), but for a picture an empty `Vec<u8>` is not "absent", it is a
/// zero-byte file that fails to decode as anything — the old side would render as
/// [`MediaDiffSideDecoded::Failed`] ("表示できません") instead of
/// [`MediaDiffSideDecoded::Absent`] ("新規ファイル"), and `old_len` would report `Some(0)` instead
/// of `None` in the binary-summary fallback. `blob_at` returning `None` here means one of two
/// things, and the committed-baseline fallback is truthful for both: the file didn't exist at the
/// pinned follow-start `sha` at all (created after following began — `base_contents` against the
/// same repo also finds nothing there, so this still resolves to `Absent`, just correctly labeled
/// `Head`/`JjParent` instead of a `FollowStart` that never actually had any bytes to offer), or
/// `blob_at` failed for an unrelated reason (`[external] git` disabled, a corrupt repo) — in which
/// case showing the committed baseline (correctly labeled as such) beats mislabeling a *found*
/// committed blob as "since follow-start" when it demonstrably isn't.
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
            // The GIF's own logical-screen size (`GifDecoder::dimensions`, read from the header
            // before any frame is decoded) — **not** `first.dimensions()`, the first decoded
            // frame's own pixel size, which the inline decode budget may have downscaled for
            // memory (`MediaDiffPictureDecoded::natural_px`'s own doc comment).
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
/// distinction `MdImgEntry::layout_px` already draws for a mermaid/math render
/// (`docs/FEATURE-MEDIA-DIFF.md` §1: "寸法は…SVG は viewBox"). Rasterized at `raster_px`'s larger
/// side (`preview::svg::rasterize_bytes` takes a single max-side target, same as every other SVG
/// call site in this codebase — mermaid/math fences included).
fn decode_svg_side(
    bytes: &[u8],
    path: &Path,
    raster_px: (u32, u32),
    hash: u64,
    page: u32,
    side: KeySide,
) -> MediaDiffSideDecoded {
    let Some(natural_px) = crate::preview::svg::intrinsic_size_bytes(bytes) else {
        return MediaDiffSideDecoded::Failed {
            reason: "invalid svg".to_string(),
        };
    };
    let max_px = raster_px.0.max(raster_px.1).max(1);
    let Some(img) = crate::preview::svg::rasterize_bytes(bytes, path, max_px) else {
        return MediaDiffSideDecoded::Failed {
            reason: "svg rasterize failed".to_string(),
        };
    };
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

/// A PDF side. `page` beyond this side's own page count is [`MediaDiffSideDecoded::PageMissing`]
/// (`docs/FEATURE-MEDIA-DIFF.md` §1's "このページはありません"), judged independently per side — the
/// two sides can have different page counts. Natural size is the page's own size in PDF points
/// (`page_dimensions_bytes`), not the raster's. `render_page_bytes` is `hayro`-only (no
/// `qlmanage`/`sips` fallback) — see that function's own doc comment for why.
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
mod tests {
    //! Mostly `App::compute_media_diff` in isolation (no run loop) — the same style
    //! `md_diff.rs`'s own tests use, plus a handful of `App::poll_media_diff`/`apply_media_diff`
    //! tests that need a real (if tiny) `App` for the cache-insertion/eviction behavior.

    use super::*;
    use crate::config::Config;
    use crate::test_support::unique_tmp;

    fn sample_path_or_skip(name: &str) -> Option<PathBuf> {
        let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("samples")
            .join(name);
        if p.exists() {
            Some(p)
        } else {
            eprintln!(
                "SKIP: samples/{name} not found (excluded from the published crate) — this test verifies nothing this run"
            );
            None
        }
    }

    fn req(path: PathBuf, root: PathBuf, baseline: DiffBaseline) -> MediaDiffRequest {
        MediaDiffRequest {
            gen: 1,
            path,
            root,
            baseline,
            page: 1,
            raster_px: (800, 600),
            preview_rules: Config::default().preview.rules,
            preview_commands: true,
        }
    }

    #[cfg(feature = "git")]
    fn init_git_repo(dir: &Path) {
        let repo = git2::Repository::init(dir).unwrap();
        let mut cfg = repo.config().unwrap();
        cfg.set_str("user.name", "Test").unwrap();
        cfg.set_str("user.email", "test@example.com").unwrap();
        cfg.set_str("commit.gpgsign", "false").ok();
    }

    #[cfg(feature = "git")]
    fn git(dir: &Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .current_dir(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// A throwaway jj workspace (no colocated `.git`) — `None` when this machine has no `jj`
    /// (konoma falls back to git there, so the suite must stay green without it; mirrors
    /// `app::tests::jj_scratch`, duplicated locally since that one is private to its own module).
    #[cfg(feature = "git")]
    fn jj_scratch(name: &str) -> Option<crate::test_support::TmpDir> {
        if !crate::vcs::jj::available() {
            return None;
        }
        let dir = unique_tmp(name);
        std::fs::create_dir_all(&dir).ok()?;
        let jj = |args: &[&str]| {
            std::process::Command::new("jj")
                .current_dir(&dir)
                .env("HOME", &dir)
                .env("JJ_USER", "konoma test")
                .env("JJ_EMAIL", "test@example.invalid")
                .args(args)
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false)
        };
        if !jj(&["git", "init", "--no-colocate", "."]) {
            return None;
        }
        Some(dir)
    }

    // ---- kind classification / summary vs. ready ----

    #[test]
    fn a_new_untracked_image_is_ready_with_old_absent() {
        let dir = unique_tmp("konoma_media_diff_new_untracked");
        std::fs::create_dir_all(&dir).unwrap();
        let png = dir.join("new.png");
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(4, 4, image::Rgb([1, 2, 3])))
            .save(&png)
            .unwrap();
        let r = req(png, dir.to_path_buf(), DiffBaseline::Empty);
        match App::compute_media_diff(&r) {
            MediaDiffComputed::Ready { kind, old, new, .. } => {
                assert_eq!(kind, MediaDiffKind::Image);
                assert!(
                    matches!(old, MediaDiffSideDecoded::Absent),
                    "旧版は無いはず"
                );
                assert!(
                    matches!(new, MediaDiffSideDecoded::Picture(_)),
                    "新版はデコードされるはず"
                );
            }
            other => panic!("Ready のはず: {other:?}"),
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_deleted_image_is_ready_with_new_absent_and_old_a_picture() {
        let dir = unique_tmp("konoma_media_diff_deleted");
        std::fs::create_dir_all(&dir).unwrap();
        let png = dir.join("was.png"); // never actually written — "deleted" = no file on disk
        let mut bytes = Vec::new();
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(5, 5, image::Rgb([9, 8, 7])))
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .unwrap();
        let r = req(png, dir.to_path_buf(), DiffBaseline::FollowSnapshot(bytes));
        match App::compute_media_diff(&r) {
            MediaDiffComputed::Ready { kind, old, new, .. } => {
                assert_eq!(
                    kind,
                    MediaDiffKind::Image,
                    "削除された PNG も旧バイト列から判定できるはず"
                );
                assert!(matches!(old, MediaDiffSideDecoded::Picture(_)));
                assert!(matches!(new, MediaDiffSideDecoded::Absent));
            }
            other => panic!("Ready のはず: {other:?}"),
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn identical_bytes_report_same_bytes_true() {
        let dir = unique_tmp("konoma_media_diff_identical");
        std::fs::create_dir_all(&dir).unwrap();
        let png = dir.join("same.png");
        let mut bytes = Vec::new();
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(3, 3, image::Rgb([1, 1, 1])))
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .unwrap();
        std::fs::write(&png, &bytes).unwrap();
        let r = req(png, dir.to_path_buf(), DiffBaseline::FollowSnapshot(bytes));
        match App::compute_media_diff(&r) {
            MediaDiffComputed::Ready { same_bytes, .. } => {
                assert!(same_bytes, "同一バイト列なら same_bytes のはず");
            }
            other => panic!("Ready のはず: {other:?}"),
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_corrupt_image_fails_that_side_without_taking_the_other_down() {
        let dir = unique_tmp("konoma_media_diff_corrupt");
        std::fs::create_dir_all(&dir).unwrap();
        let png = dir.join("corrupt.png");
        // Real PNG magic bytes (`infer` only checks the first 4 — `docs/FEATURE-MEDIA-DIFF.md` §2's
        // MIME-sniff classification will therefore still resolve this to `Image`), followed by
        // garbage that isn't a valid PNG stream at all — the decode itself must fail, not the kind
        // classification (a plain-text "not a png at all" would instead sniff as `Text` and the
        // whole diff would degrade to `Summary` before ever reaching a per-side decode, which is a
        // different code path than the one this test means to exercise).
        std::fs::write(&png, b"\x89PNG\r\n\x1a\ngarbage, not a real PNG stream").unwrap();
        let good_old = {
            let mut b = Vec::new();
            image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
                2,
                2,
                image::Rgb([0, 0, 0]),
            ))
            .write_to(&mut std::io::Cursor::new(&mut b), image::ImageFormat::Png)
            .unwrap();
            b
        };
        let r = req(
            png,
            dir.to_path_buf(),
            DiffBaseline::FollowSnapshot(good_old),
        );
        match App::compute_media_diff(&r) {
            MediaDiffComputed::Ready { old, new, .. } => {
                assert!(
                    matches!(old, MediaDiffSideDecoded::Picture(_)),
                    "旧版は正常"
                );
                assert!(
                    matches!(new, MediaDiffSideDecoded::Failed { .. }),
                    "新版は壊れているので Failed のはず: {new:?}"
                );
            }
            other => panic!(
                "Ready のはず(kind は new が .png 拡張子なので Image に解決される): {other:?}"
            ),
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_non_picture_kind_degrades_to_summary_with_sizes() {
        let dir = unique_tmp("konoma_media_diff_summary_kind");
        std::fs::create_dir_all(&dir).unwrap();
        let mp4 = dir.join("clip.mp4");
        std::fs::write(&mp4, b"0123456789").unwrap();
        let r = req(
            mp4,
            dir.to_path_buf(),
            DiffBaseline::FollowSnapshot(b"01234".to_vec()),
        );
        match App::compute_media_diff(&r) {
            MediaDiffComputed::Summary {
                old_len, new_len, ..
            } => {
                assert_eq!(old_len, Some(5));
                assert_eq!(new_len, Some(10));
            }
            other => panic!("動画は Summary のはず: {other:?}"),
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    // ---- decode_svg_side: rasterizes to the LARGER axis of raster_px ----

    /// `decode_svg_side`'s `max_px = raster_px.0.max(raster_px.1)` — the rasterized picture's own
    /// pixel size must actually reflect the **larger** of the two `raster_px` components, not the
    /// smaller one and not, say, the width component alone. A deliberately non-square, asymmetric
    /// `raster_px` (100×50) against a small (40×20, 2:1) SVG — small enough that any target here is
    /// an upscale, never `rasterize_bytes`'s own "shrink to fit `HARD_MAX_PX`" branch — makes the
    /// three candidate target values (100, 50, and "width alone" = 100 too, so also cross-checked
    /// against a second, width-larger fixture below) produce three genuinely different pixel sizes,
    /// discriminating a `.max` from a `.min` or an accidental "just use one axis" mutant.
    #[test]
    fn decode_svg_side_rasterizes_to_the_larger_axis_of_raster_px() {
        let dir = unique_tmp("konoma_media_diff_svg_raster_axis");
        std::fs::create_dir_all(&dir).unwrap();
        let svg = dir.join("icon.svg");
        // A 40x20 (2:1) viewBox — small enough that `rasterize_bytes` always upscales for any
        // `raster_px` used below (never shrinks below 1:1 scale under `HARD_MAX_PX`).
        std::fs::write(
            &svg,
            "<svg xmlns='http://www.w3.org/2000/svg' width='40' height='20' viewBox='0 0 40 20'></svg>",
        )
        .unwrap();
        let r = MediaDiffRequest {
            gen: 1,
            path: svg.clone(),
            root: dir.to_path_buf(),
            baseline: DiffBaseline::Empty,
            page: 1,
            raster_px: (100, 50), // height (50) is the *smaller* component — must not be used alone.
            preview_rules: Config::default().preview.rules,
            preview_commands: true,
        };
        match App::compute_media_diff(&r) {
            MediaDiffComputed::Ready { new, .. } => match new {
                MediaDiffSideDecoded::Picture(p) => {
                    // scale = max(100,50) / max(40,20) = 100/40 = 2.5 -> (40*2.5, 20*2.5) = (100, 50).
                    assert_eq!(
                        (p.image.width(), p.image.height()),
                        (100, 50),
                        "raster_px の大きい方(100)が長辺のターゲットになるはず"
                    );
                }
                other => panic!("SVG はデコードされるはず: {other:?}"),
            },
            other => panic!("Ready のはず: {other:?}"),
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The mirror of the test above with the **width** as the larger component instead of the
    /// height — together the two pin that it is genuinely `max(w, h)`, not "whichever axis happens
    /// to be listed first" or a hardcoded preference for one axis.
    #[test]
    fn decode_svg_side_rasterizes_to_the_larger_axis_of_raster_px_when_width_is_larger() {
        let dir = unique_tmp("konoma_media_diff_svg_raster_axis_w");
        std::fs::create_dir_all(&dir).unwrap();
        let svg = dir.join("icon.svg");
        std::fs::write(
            &svg,
            "<svg xmlns='http://www.w3.org/2000/svg' width='20' height='40' viewBox='0 0 20 40'></svg>",
        )
        .unwrap();
        let r = MediaDiffRequest {
            gen: 1,
            path: svg.clone(),
            root: dir.to_path_buf(),
            baseline: DiffBaseline::Empty,
            page: 1,
            raster_px: (50, 100), // width (50) is the *smaller* component this time.
            preview_rules: Config::default().preview.rules,
            preview_commands: true,
        };
        match App::compute_media_diff(&r) {
            MediaDiffComputed::Ready { new, .. } => match new {
                MediaDiffSideDecoded::Picture(p) => {
                    // scale = max(50,100) / max(20,40) = 100/40 = 2.5 -> (20*2.5, 40*2.5) = (50, 100).
                    assert_eq!(
                        (p.image.width(), p.image.height()),
                        (50, 100),
                        "raster_px の大きい方(100)が長辺のターゲットになるはず"
                    );
                }
                other => panic!("SVG はデコードされるはず: {other:?}"),
            },
            other => panic!("Ready のはず: {other:?}"),
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    // ---- compute_media_diff_with_cap: cap-before-reading (over cap, `||` not `&&`) ----

    /// The new side alone over cap (the old side is entirely absent — an untracked/new file) still
    /// degrades to `Summary`, proving `over_cap = old_over_cap || new_over_cap` — a mutation to `&&`
    /// would require *both* sides over cap, and this fixture's old side isn't even present to be
    /// "over" anything, so a `&&` mutant would wrongly reach the `Ready`/decode path instead.
    #[test]
    fn new_side_over_cap_alone_degrades_to_summary() {
        let dir = unique_tmp("konoma_media_diff_cap_new_over");
        std::fs::create_dir_all(&dir).unwrap();
        let png = dir.join("big.png");
        // Content is irrelevant — once metadata alone says it's over cap, the bytes are never read
        // at all (`compute_media_diff_with_cap`'s own doc comment on the new side).
        std::fs::write(&png, vec![0u8; 100]).unwrap();
        let r = req(png, dir.to_path_buf(), DiffBaseline::Empty);
        match App::compute_media_diff_with_cap(&r, 50) {
            MediaDiffComputed::Summary {
                old_len, new_len, ..
            } => {
                assert_eq!(old_len, None, "旧版は無い(新規/未追跡)");
                assert_eq!(
                    new_len,
                    Some(100),
                    "新版のサイズは metadata から取れているはず(読まずに)"
                );
            }
            other => panic!("cap 超過は Summary のはず: {other:?}"),
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The **old** side alone over cap, while the new side is tiny — the mirror of the test above,
    /// completing the `||` pin (a `&&` mutant would also fail to degrade here, since only one side
    /// is over cap).
    #[test]
    fn old_side_over_cap_alone_degrades_to_summary_even_when_new_is_tiny() {
        let dir = unique_tmp("konoma_media_diff_cap_old_over");
        std::fs::create_dir_all(&dir).unwrap();
        let png = dir.join("small.png");
        std::fs::write(&png, vec![1u8; 10]).unwrap();
        // No size-only API for the old side (`resolve_old_bytes`'s own doc comment) — a follow
        // snapshot is always fully in hand once resolved, so its length is only known this way.
        let r = req(
            png,
            dir.to_path_buf(),
            DiffBaseline::FollowSnapshot(vec![2u8; 100]),
        );
        match App::compute_media_diff_with_cap(&r, 50) {
            MediaDiffComputed::Summary {
                old_len, new_len, ..
            } => {
                assert_eq!(old_len, Some(100));
                assert_eq!(
                    new_len,
                    Some(10),
                    "新版は cap 未満なので実際に読まれているはず"
                );
            }
            other => {
                panic!("cap 超過(旧版のみ)は Summary のはず(|| であって && ではない): {other:?}")
            }
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    /// `n == cap` is **not** over cap (the comparison is `>`, not `>=`) — the new side decodes for
    /// real at exactly the cap's own byte count.
    #[test]
    fn exact_cap_size_boundary_is_not_over_cap() {
        let dir = unique_tmp("konoma_media_diff_cap_boundary");
        std::fs::create_dir_all(&dir).unwrap();
        let png = dir.join("exact.png");
        let mut bytes = Vec::new();
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(2, 2, image::Rgb([1, 2, 3])))
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .unwrap();
        std::fs::write(&png, &bytes).unwrap();
        let cap = bytes.len() as u64; // n == cap, exactly.
        let r = req(png, dir.to_path_buf(), DiffBaseline::Empty);
        match App::compute_media_diff_with_cap(&r, cap) {
            MediaDiffComputed::Ready { new, .. } => {
                assert!(
                    matches!(new, MediaDiffSideDecoded::Picture(_)),
                    "n == cap は over cap ではないので実際にデコードされるはず"
                );
            }
            other => panic!("境界(n==cap)で Summary に落ちてはいけない: {other:?}"),
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Unlike `new_side_over_cap_alone_degrades_to_summary` above (whose garbage-bytes fixture
    /// fails `classify_kind` regardless of the cap check — `rule_matches`' mime branch sniffs the
    /// *real* file at `path` via `infer::get_from_path` even when `sniff` is `None`, so invalid
    /// magic bytes alone already forces `Summary`, independent of `over_cap` — this uses a **real,
    /// decodable** PNG whose actual byte length exceeds `cap`: genuinely discriminating, since a
    /// mutant that skips the metadata-based skip-the-read step would classify it as `Image` and
    /// actually decode it (`Ready`), not degrade to `Summary`.
    #[test]
    fn new_side_real_decodable_png_over_cap_still_degrades_to_summary() {
        let dir = unique_tmp("konoma_media_diff_cap_new_over_real_png");
        std::fs::create_dir_all(&dir).unwrap();
        let png = dir.join("big.png");
        let mut bytes = Vec::new();
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(20, 20, image::Rgb([9, 9, 9])))
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .unwrap();
        std::fs::write(&png, &bytes).unwrap();
        let cap = (bytes.len() as u64) - 1; // strictly under the file's real size.
        let r = req(png, dir.to_path_buf(), DiffBaseline::Empty);
        match App::compute_media_diff_with_cap(&r, cap) {
            MediaDiffComputed::Summary { new_len, .. } => {
                assert_eq!(new_len, Some(bytes.len() as u64));
            }
            other => panic!(
                "実在する有効な PNG でも cap 超過なら Summary のはず(実際にデコードされてはいけない): {other:?}"
            ),
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Mutation-proving: if `classify_kind` degenerated into "always picture-capable", this would
    /// come back `Ready` instead — pins the actual branch fires.
    #[test]
    fn classify_kind_actually_gates_the_ready_vs_summary_branch() {
        assert_eq!(
            classify_kind(&PreviewKind::Video(PathBuf::from("x.mp4"))),
            None
        );
        assert_eq!(
            classify_kind(&PreviewKind::Image(PathBuf::from("x.png"))),
            Some(MediaDiffKind::Image)
        );
    }

    #[test]
    fn over_the_cap_degrades_to_summary_via_the_test_seam() {
        let dir = unique_tmp("konoma_media_diff_cap");
        std::fs::create_dir_all(&dir).unwrap();
        let png = dir.join("big.png");
        std::fs::write(&png, vec![0u8; 100]).unwrap(); // "big" only relative to the tiny test cap below
        let r = req(
            png,
            dir.to_path_buf(),
            DiffBaseline::FollowSnapshot(vec![0u8; 100]),
        );
        // Sanity: under a real-sized cap this would be a Ready (Image, by content sniff/extension).
        match App::compute_media_diff_with_cap(&r, 1_000_000) {
            MediaDiffComputed::Ready { .. } | MediaDiffComputed::Summary { .. } => {}
            MediaDiffComputed::Unavailable => panic!("前提が崩れている"),
        }
        // A cap smaller than either side's byte length forces the Summary (not Ready) branch.
        match App::compute_media_diff_with_cap(&r, 10) {
            MediaDiffComputed::Summary {
                old_len, new_len, ..
            } => {
                assert_eq!(old_len, Some(100));
                assert_eq!(new_len, Some(100));
            }
            other => panic!("上限超は Summary のはず: {other:?}"),
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn pdf_page_beyond_that_sides_own_count_is_page_missing() {
        let Some(p) = sample_path_or_skip("sample.pdf") else {
            return;
        };
        let bytes = std::fs::read(&p).unwrap();
        let dir = unique_tmp("konoma_media_diff_pdf_page_missing");
        std::fs::create_dir_all(&dir).unwrap();
        let pdf = dir.join("doc.pdf");
        std::fs::write(&pdf, &bytes).unwrap(); // sample.pdf is a known 3-page document
        let mut r = req(pdf, dir.to_path_buf(), DiffBaseline::FollowSnapshot(bytes));
        r.page = 999;
        match App::compute_media_diff(&r) {
            MediaDiffComputed::Ready { old, new, .. } => {
                assert!(matches!(old, MediaDiffSideDecoded::PageMissing));
                assert!(matches!(new, MediaDiffSideDecoded::PageMissing));
            }
            other => panic!("Ready のはず: {other:?}"),
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    // ---- baseline / base selection ----

    #[cfg(feature = "git")]
    #[test]
    fn git_baseline_reports_head_and_reads_the_committed_blob() {
        let dir = unique_tmp("konoma_media_diff_git_head");
        std::fs::create_dir_all(&dir).unwrap();
        init_git_repo(&dir);
        let png = dir.join("pic.png");
        let old_bytes = {
            let mut b = Vec::new();
            image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
                2,
                2,
                image::Rgb([1, 1, 1]),
            ))
            .write_to(&mut std::io::Cursor::new(&mut b), image::ImageFormat::Png)
            .unwrap();
            b
        };
        std::fs::write(&png, &old_bytes).unwrap();
        git(&dir, &["add", "-A"]);
        git(&dir, &["commit", "-q", "-m", "init"]);
        // Modify on disk without committing.
        let new_bytes = {
            let mut b = Vec::new();
            image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
                3,
                3,
                image::Rgb([2, 2, 2]),
            ))
            .write_to(&mut std::io::Cursor::new(&mut b), image::ImageFormat::Png)
            .unwrap();
            b
        };
        std::fs::write(&png, &new_bytes).unwrap();

        let r = req(png, dir.to_path_buf(), DiffBaseline::Vcs);
        match App::compute_media_diff(&r) {
            MediaDiffComputed::Ready { base, old, .. } => {
                assert_eq!(base, MediaBase::Head);
                match old {
                    MediaDiffSideDecoded::Picture(p) => assert_eq!(p.bytes, old_bytes.len() as u64),
                    other => panic!("旧版はコミット済み画像のはず: {other:?}"),
                }
            }
            other => panic!("Ready のはず: {other:?}"),
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(feature = "git")]
    #[test]
    fn follow_snapshot_baseline_reports_follow_start() {
        let dir = unique_tmp("konoma_media_diff_follow_snapshot");
        std::fs::create_dir_all(&dir).unwrap();
        let png = dir.join("pic.png");
        let bytes = {
            let mut b = Vec::new();
            image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
                2,
                2,
                image::Rgb([1, 1, 1]),
            ))
            .write_to(&mut std::io::Cursor::new(&mut b), image::ImageFormat::Png)
            .unwrap();
            b
        };
        std::fs::write(&png, &bytes).unwrap();
        let r = req(png, dir.to_path_buf(), DiffBaseline::FollowSnapshot(bytes));
        match App::compute_media_diff(&r) {
            MediaDiffComputed::Ready { base, .. } => assert_eq!(base, MediaBase::FollowStart),
            other => panic!("Ready のはず: {other:?}"),
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A follow session with **no** usable snapshot for this file (`DiffBaseline::Empty` — dirty at
    /// follow-start but too large to snapshot, or genuinely no baseline) falls back to the committed
    /// baseline (`docs/FEATURE-MEDIA-DIFF.md` §7), and the reported base names that fallback
    /// (`Head`), not `FollowStart` — unlike the Markdown block-diff, which treats this as "no old
    /// side" instead (`App::compute_md_diff`'s own doc comment).
    #[cfg(feature = "git")]
    #[test]
    fn follow_without_a_snapshot_falls_back_to_head_and_reports_head() {
        let dir = unique_tmp("konoma_media_diff_follow_no_snapshot");
        std::fs::create_dir_all(&dir).unwrap();
        init_git_repo(&dir);
        let png = dir.join("pic.png");
        let bytes = {
            let mut b = Vec::new();
            image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
                2,
                2,
                image::Rgb([1, 1, 1]),
            ))
            .write_to(&mut std::io::Cursor::new(&mut b), image::ImageFormat::Png)
            .unwrap();
            b
        };
        std::fs::write(&png, &bytes).unwrap();
        git(&dir, &["add", "-A"]);
        git(&dir, &["commit", "-q", "-m", "init"]);

        let r = req(png, dir.to_path_buf(), DiffBaseline::Empty);
        match App::compute_media_diff(&r) {
            MediaDiffComputed::Ready { base, old, .. } => {
                assert_eq!(
                    base,
                    MediaBase::Head,
                    "スナップショット無し = HEAD に落ちて基準名も HEAD のはず"
                );
                assert!(
                    matches!(old, MediaDiffSideDecoded::Picture(_)),
                    "HEAD には実体があるので Absent ではないはず: {old:?}"
                );
            }
            other => panic!("Ready のはず: {other:?}"),
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    /// `FollowHead` (clean at follow-start, pinned to the HEAD sha) for a file that did **not exist**
    /// at that sha — created after following began. `crate::git::blob_at` returns `None` for this
    /// (not `Some(empty vec)`), and `resolve_old_bytes` must fall back to the committed baseline
    /// exactly like `DiffBaseline::Empty` does, landing on `Absent`/`old_len == None`/`base == Head`
    /// — not `Failed` with a spurious 0-byte "旧版" and not mislabeled `FollowStart`. Regression: an
    /// earlier version used `blob_at(..).unwrap_or_default()`, decoding the empty vec as a corrupt
    /// image (`Failed`) instead of recognizing the file simply didn't exist yet.
    #[cfg(feature = "git")]
    #[test]
    fn follow_head_for_a_file_created_after_follow_start_falls_back_to_head_and_is_absent() {
        let dir = unique_tmp("konoma_media_diff_follow_head_created_after");
        std::fs::create_dir_all(&dir).unwrap();
        init_git_repo(&dir);
        // A first commit that does NOT include the image at all — this is the sha follow-start pins.
        std::fs::write(dir.join("placeholder.txt"), b"seed\n").unwrap();
        git(&dir, &["add", "-A"]);
        git(&dir, &["commit", "-q", "-m", "seed"]);
        let sha = crate::git::head_commit_id(&dir).expect("HEAD sha が取れるはず");

        // The image is created only *after* that commit (untracked at follow-start).
        let png = dir.join("new-since-follow.png");
        let bytes = {
            let mut b = Vec::new();
            image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
                3,
                3,
                image::Rgb([9, 9, 9]),
            ))
            .write_to(&mut std::io::Cursor::new(&mut b), image::ImageFormat::Png)
            .unwrap();
            b
        };
        std::fs::write(&png, &bytes).unwrap();

        // Direct, precise check on the function under review: `blob_at` returns `None` here (the
        // path isn't in that commit at all), so this must resolve to `(None, Head)` — never
        // `(Some(vec![]), FollowStart)`.
        let (old_bytes, base) =
            resolve_old_bytes(&DiffBaseline::FollowHead { sha: sha.clone() }, &dir, &png);
        assert_eq!(
            old_bytes, None,
            "blob_at が None ⟹ 旧版は None のはず(空 Vec ではない)"
        );
        assert_eq!(
            base,
            MediaBase::Head,
            "blob_at が None ⟹ HEAD への構成的フォールバック、FollowStart と偽らない"
        );

        // Integration-level check: the full computation must therefore treat the old side as
        // genuinely absent, not as a corrupt/undecodable 0-byte image.
        let r = req(png, dir.to_path_buf(), DiffBaseline::FollowHead { sha });
        match App::compute_media_diff(&r) {
            MediaDiffComputed::Ready { base, old, .. } => {
                assert_eq!(base, MediaBase::Head);
                assert!(
                    matches!(old, MediaDiffSideDecoded::Absent),
                    "follow 開始時点に存在しないファイルは Absent のはず(0 バイトの Failed ではない): {old:?}"
                );
            }
            other => panic!("Ready のはず: {other:?}"),
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    /// `FollowHead` for a file created *after* the pinned follow-start sha, same as the test above —
    /// but this time it has **since been committed** (unlike that test's still-untracked fixture).
    /// `blob_at(root, sha, path)` still returns `None` (the path genuinely isn't in the pinned
    /// commit), so this falls back to `base_contents` exactly as before — but `base_contents` reads
    /// against the **current** HEAD, not the pinned sha, and the file *is* there now: the old side
    /// must be the real committed bytes (`Head`, matching `Some`), not `Absent`. A version of
    /// `resolve_old_bytes` that conflated "not in the pinned commit" with "not in the repo at all"
    /// (e.g. by short-circuiting straight to `Absent` on a `blob_at` miss, instead of actually
    /// falling through to `base_contents`) would still pass the sibling test above — both fixtures
    /// hit the identical `blob_at → None` branch — but only this one has a real committed blob on
    /// the other side of that fallback to notice going missing.
    #[cfg(feature = "git")]
    #[test]
    fn follow_head_for_a_file_created_after_follow_start_and_since_committed_reads_head_not_absent()
    {
        let dir = unique_tmp("konoma_media_diff_follow_head_created_then_committed");
        std::fs::create_dir_all(&dir).unwrap();
        init_git_repo(&dir);
        std::fs::write(dir.join("placeholder.txt"), b"seed\n").unwrap();
        git(&dir, &["add", "-A"]);
        git(&dir, &["commit", "-q", "-m", "seed"]);
        let sha = crate::git::head_commit_id(&dir).expect("HEAD sha が取れるはず"); // follow-start pin

        // Created after follow-start (not in `sha` at all)...
        let png = dir.join("new-since-follow.png");
        let committed_bytes = {
            let mut b = Vec::new();
            image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
                5,
                5,
                image::Rgb([3, 3, 3]),
            ))
            .write_to(&mut std::io::Cursor::new(&mut b), image::ImageFormat::Png)
            .unwrap();
            b
        };
        std::fs::write(&png, &committed_bytes).unwrap();
        // ...but, unlike the sibling test, it IS committed since (advancing HEAD past `sha`).
        git(&dir, &["add", "-A"]);
        git(
            &dir,
            &["commit", "-q", "-m", "add the png after follow-start"],
        );

        let (old_bytes, base) =
            resolve_old_bytes(&DiffBaseline::FollowHead { sha: sha.clone() }, &dir, &png);
        assert_eq!(
            old_bytes,
            Some(committed_bytes.clone()),
            "follow 開始後に作成されても、以後コミットされていれば旧版は現在の HEAD の実バイト列のはず(Absent ではない)"
        );
        assert_eq!(
            base,
            MediaBase::Head,
            "blob_at は None(follow 開始時点には無い)だが、現在の HEAD にはあるので Head 扱い"
        );

        let r = req(png, dir.to_path_buf(), DiffBaseline::FollowHead { sha });
        match App::compute_media_diff(&r) {
            MediaDiffComputed::Ready { base, old, .. } => {
                assert_eq!(base, MediaBase::Head);
                assert!(
                    matches!(old, MediaDiffSideDecoded::Picture(_)),
                    "以後コミットされているので旧版はデコードされた絵のはず(Absent ではない): {old:?}"
                );
            }
            other => panic!("Ready のはず: {other:?}"),
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    /// `FollowHead` for a file that **did** exist at the pinned sha (clean at follow-start) and was
    /// then modified — `blob_at` finds real bytes, so the base stays `FollowStart` (unlike the
    /// "created after" case above, which falls back to `Head`).
    #[cfg(feature = "git")]
    #[test]
    fn follow_head_for_a_file_modified_since_follow_start_reads_the_committed_blob() {
        let dir = unique_tmp("konoma_media_diff_follow_head_modified");
        std::fs::create_dir_all(&dir).unwrap();
        init_git_repo(&dir);
        let png = dir.join("pic.png");
        let old_bytes = {
            let mut b = Vec::new();
            image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
                2,
                2,
                image::Rgb([1, 1, 1]),
            ))
            .write_to(&mut std::io::Cursor::new(&mut b), image::ImageFormat::Png)
            .unwrap();
            b
        };
        std::fs::write(&png, &old_bytes).unwrap();
        git(&dir, &["add", "-A"]);
        git(&dir, &["commit", "-q", "-m", "init"]);
        let sha = crate::git::head_commit_id(&dir).expect("HEAD sha が取れるはず");

        // Modified on disk after the pinned sha (still clean/committed at follow-start itself).
        let new_bytes = {
            let mut b = Vec::new();
            image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
                4,
                4,
                image::Rgb([2, 2, 2]),
            ))
            .write_to(&mut std::io::Cursor::new(&mut b), image::ImageFormat::Png)
            .unwrap();
            b
        };
        std::fs::write(&png, &new_bytes).unwrap();

        let r = req(png, dir.to_path_buf(), DiffBaseline::FollowHead { sha });
        match App::compute_media_diff(&r) {
            MediaDiffComputed::Ready { base, old, .. } => {
                assert_eq!(
                    base,
                    MediaBase::FollowStart,
                    "sha に実体があるので FollowStart のはず(HEAD へのフォールバックではない)"
                );
                match old {
                    MediaDiffSideDecoded::Picture(p) => {
                        assert_eq!(
                            p.bytes,
                            old_bytes.len() as u64,
                            "旧版はピン留めされた sha のバイト列のはず"
                        );
                    }
                    other => panic!("旧版はコミット済み画像のはず: {other:?}"),
                }
            }
            other => panic!("Ready のはず: {other:?}"),
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(feature = "git")]
    #[test]
    fn jj_baseline_reports_jj_parent() {
        let Some(dir) = jj_scratch("konoma_media_diff_jj_parent") else {
            return;
        };
        let png = dir.join("pic.png");
        let old_bytes = {
            let mut b = Vec::new();
            image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
                2,
                2,
                image::Rgb([3, 3, 3]),
            ))
            .write_to(&mut std::io::Cursor::new(&mut b), image::ImageFormat::Png)
            .unwrap();
            b
        };
        std::fs::write(&png, &old_bytes).unwrap();
        let ok = std::process::Command::new("jj")
            .current_dir(&dir)
            .env("HOME", &dir)
            .env("JJ_USER", "konoma test")
            .env("JJ_EMAIL", "test@example.invalid")
            .args(["commit", "-m", "seed"])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if !ok {
            eprintln!("SKIP: jj commit failed in the scratch workspace");
            std::fs::remove_dir_all(&dir).ok();
            return;
        }
        // Modify on disk without committing (the new working-copy state).
        let new_bytes = {
            let mut b = Vec::new();
            image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
                3,
                3,
                image::Rgb([4, 4, 4]),
            ))
            .write_to(&mut std::io::Cursor::new(&mut b), image::ImageFormat::Png)
            .unwrap();
            b
        };
        std::fs::write(&png, &new_bytes).unwrap();

        let r = req(png, dir.to_path_buf(), DiffBaseline::Vcs);
        match App::compute_media_diff(&r) {
            MediaDiffComputed::Ready { base, .. } => assert_eq!(base, MediaBase::JjParent),
            other => panic!("Ready のはず: {other:?}"),
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Mutation-proving: `is_jj` must actually consult the real backend, not hardcode `false` — a
    /// jj-only workspace (no `.git`) must answer `true`.
    #[cfg(feature = "git")]
    #[test]
    fn is_jj_actually_detects_a_jj_only_workspace() {
        let Some(dir) = jj_scratch("konoma_media_diff_is_jj") else {
            return;
        };
        assert!(is_jj(&dir), "jj のみのワークスペースは jj と判定されるはず");
        let git_dir = unique_tmp("konoma_media_diff_is_jj_git_control");
        std::fs::create_dir_all(&git_dir).unwrap();
        init_git_repo(&git_dir);
        assert!(!is_jj(&git_dir), "git リポジトリは jj と判定されないはず");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(&git_dir).ok();
    }

    // ---- App-level: poll/apply, cache insertion, staleness, eviction ----

    #[test]
    fn poll_media_diff_sync_fallback_returns_ready_on_first_call() {
        let dir = unique_tmp("konoma_media_diff_poll_sync");
        std::fs::create_dir_all(&dir).unwrap();
        let png = dir.join("pic.png");
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(4, 4, image::Rgb([5, 5, 5])))
            .save(&png)
            .unwrap();
        let mut app = App::new(dir.to_path_buf(), Config::default()).unwrap();
        let outcome = app.poll_media_diff(&png, 1, (400, 300));
        assert!(
            outcome.is_some(),
            "tx 未 attach = 同期フォールバックで初回から結果が返るはず"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn apply_media_diff_drops_a_stale_generation() {
        let dir = unique_tmp("konoma_media_diff_stale_gen");
        std::fs::create_dir_all(&dir).unwrap();
        let png = dir.join("pic.png");
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(2, 2, image::Rgb([1, 1, 1])))
            .save(&png)
            .unwrap();
        let mut app = App::new(dir.to_path_buf(), Config::default()).unwrap();
        // Kick once for real (bumps media_diff_gen to 1 and lands a result via the sync fallback).
        let first = app.poll_media_diff(&png, 1, (400, 300));
        assert!(first.is_some());
        let current_gen = app.media_diff_gen;
        // A result tagged with an older generation must be rejected.
        let stale = MediaDiffResult {
            gen: current_gen.wrapping_sub(1),
            path: png.clone(),
            page: 1,
            raster_px: (400, 300),
            computed: MediaDiffComputed::Unavailable,
        };
        assert!(!app.apply_media_diff(stale), "古い gen は false を返すはず");
        // The landed outcome is still the earlier (real) one, not clobbered by the stale apply.
        assert!(app.poll_media_diff(&png, 1, (400, 300)).is_some());
    }

    #[test]
    fn applied_pictures_land_in_md_image_cache_under_media_diff_keys() {
        let dir = unique_tmp("konoma_media_diff_cache_insert");
        std::fs::create_dir_all(&dir).unwrap();
        let png = dir.join("pic.png");
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(4, 4, image::Rgb([7, 7, 7])))
            .save(&png)
            .unwrap();
        let mut app = App::new(dir.to_path_buf(), Config::default()).unwrap();
        let outcome = app
            .poll_media_diff(&png, 1, (400, 300))
            .expect("同期で返るはず");
        match outcome {
            MediaDiffOutcome::Ready {
                kind,
                base,
                same_bytes,
                old,
                new,
            } => {
                assert_eq!(kind, MediaDiffKind::Image);
                assert!(
                    matches!(base, MediaBase::Head),
                    "リポジトリ外なので HEAD 扱い"
                );
                assert!(!same_bytes, "旧版が無いので同一ではない");
                assert!(matches!(old, MediaDiffSide::Absent), "旧版は無いはず");
                match new {
                    MediaDiffSide::Picture(p) => {
                        assert!(
                            crate::preview::media_diff::is_media_diff_url(
                                &p.cache_key.to_string_lossy()
                            ),
                            "cache_key は media-diff:// キーのはず: {:?}",
                            p.cache_key
                        );
                        assert!(
                            app.md_image_cache.contains_key(&p.cache_key),
                            "md_image_cache に入っているはず"
                        );
                        assert_eq!(p.natural_px, (4, 4), "自然寸法が保存されているはず");
                        assert!(p.bytes > 0, "バイト数が保存されているはず");
                        assert_eq!(p.page_count, None, "画像は PDF ではないので None のはず");
                    }
                    other => panic!("新版はデコードされるはず: {other:?}"),
                }
            }
            other => panic!("Ready のはず: {other:?}"),
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    /// `App::ensure_md_cache`'s own Markdown-doc prune (`md_render.rs`'s `md_image_cache.retain`,
    /// keyed off `is_mermaid_fence_url`/`is_math_url`) must leave a `media-diff://` key alone — it
    /// isn't a mermaid/math key, so the predicate's `!(mermaid || math)` arm already keeps it
    /// unconditionally, but this pins that behavior against the real production rebuild path (not
    /// just by inspection) rather than assuming the `||` never grows a third disjunct that would
    /// change that.
    #[test]
    fn markdown_prune_on_rebuild_leaves_a_media_diff_key_alone() {
        let dir = unique_tmp("konoma_media_diff_markdown_prune");
        std::fs::create_dir_all(&dir).unwrap();
        let md = dir.join("doc.md");
        // Deliberately no mermaid fence / math expression in this document — isolates the prune's
        // treatment of a media-diff key from its own, separately-tested mermaid/math eviction.
        std::fs::write(&md, "# Title\n\nJust an ordinary paragraph.\n").unwrap();
        let media_key = PathBuf::from(crate::preview::media_diff::media_diff_url(
            KeySide::New,
            crate::preview::media_diff::fnv1a64(b"whatever bytes"),
            1,
            None,
        ));
        let mut app = App::new(dir.to_path_buf(), Config::default()).unwrap();
        // Open the document first (`enter_preview` clears the *whole* `md_image_cache` on a file
        // switch — a different, already-expected mechanism from the rebuild-time prune this test
        // means to isolate), then insert the media-diff key and force a second `ensure_md_cache`
        // pass (a width/content rebuild of the *same, already-open* file) to exercise the prune.
        app.enter_preview(&md);
        app.ensure_md_cache(100);
        app.md_image_cache
            .insert(media_key.clone(), MdImgEntry::default());
        app.md_cache = None; // force ensure_md_cache to actually rebuild (and prune) again
        app.ensure_md_cache(100);
        assert!(
            app.md_image_cache.contains_key(&media_key),
            "media-diff:// キーは Markdown の mermaid/math prune に巻き込まれないはず"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The landed (App-level) `MediaDiffSide::Failed` — not just `compute_media_diff`'s own
    /// `MediaDiffSideDecoded::Failed` — carries a non-empty reason through `apply_media_diff`'s
    /// pixel-stripping step (`materialize_side`), so a caption can eventually say *why* a side
    /// couldn't be shown rather than just that it couldn't.
    #[test]
    fn a_failed_side_keeps_its_reason_through_to_the_landed_outcome() {
        let dir = unique_tmp("konoma_media_diff_landed_failed_reason");
        std::fs::create_dir_all(&dir).unwrap();
        let png = dir.join("corrupt.png");
        std::fs::write(&png, b"\x89PNG\r\n\x1a\ngarbage, not a real PNG stream").unwrap();
        let mut app = App::new(dir.to_path_buf(), Config::default()).unwrap();
        let outcome = app
            .poll_media_diff(&png, 1, (400, 300))
            .expect("同期で返るはず");
        match outcome {
            MediaDiffOutcome::Ready { new, .. } => match new {
                MediaDiffSide::Failed { reason } => {
                    assert!(!reason.is_empty(), "理由が空でないはず");
                }
                other => panic!("壊れた PNG は Failed のはず: {other:?}"),
            },
            other => panic!("Ready のはず: {other:?}"),
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The landed `MediaDiffOutcome::Summary` (not just `MediaDiffComputed::Summary`) carries its
    /// sizes/`same_bytes`/`base` all the way through `apply_media_diff` unchanged (that variant has
    /// no pixel data to strip, unlike `Ready`).
    #[test]
    fn a_non_picture_kind_lands_as_summary_with_its_fields_intact() {
        let dir = unique_tmp("konoma_media_diff_landed_summary");
        std::fs::create_dir_all(&dir).unwrap();
        let mp4 = dir.join("clip.mp4");
        std::fs::write(&mp4, b"0123456789").unwrap();
        let mut app = App::new(dir.to_path_buf(), Config::default()).unwrap();
        let outcome = app
            .poll_media_diff(&mp4, 1, (400, 300))
            .expect("同期で返るはず");
        match outcome {
            MediaDiffOutcome::Summary {
                base,
                same_bytes,
                old_len,
                new_len,
            } => {
                assert!(matches!(base, MediaBase::Head));
                assert!(!same_bytes, "旧版が無いので同一ではない");
                assert_eq!(old_len, None, "旧版は無いはず");
                assert_eq!(new_len, Some(10));
            }
            other => panic!("動画は Summary のはず: {other:?}"),
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn switching_target_drops_the_old_media_diff_keys() {
        let dir = unique_tmp("konoma_media_diff_switch_target");
        std::fs::create_dir_all(&dir).unwrap();
        let a = dir.join("a.png");
        let b = dir.join("b.png");
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(4, 4, image::Rgb([1, 1, 1])))
            .save(&a)
            .unwrap();
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(6, 6, image::Rgb([2, 2, 2])))
            .save(&b)
            .unwrap();
        let mut app = App::new(dir.to_path_buf(), Config::default()).unwrap();

        let outcome_a = app.poll_media_diff(&a, 1, (400, 300)).unwrap();
        let key_a = match outcome_a {
            MediaDiffOutcome::Ready {
                new: MediaDiffSide::Picture(p),
                ..
            } => p.cache_key,
            other => panic!("a: Ready のはず: {other:?}"),
        };
        assert!(app.md_image_cache.contains_key(&key_a));

        // Switching to a different target/page/raster is itself a new request — invalidate first
        // (mirrors what `App::invalidate_diff_caches` does on a real file/target switch), then poll.
        app.invalidate_media_diff();
        let outcome_b = app.poll_media_diff(&b, 1, (400, 300)).unwrap();
        let key_b = match outcome_b {
            MediaDiffOutcome::Ready {
                new: MediaDiffSide::Picture(p),
                ..
            } => p.cache_key,
            other => panic!("b: Ready のはず: {other:?}"),
        };
        assert_ne!(key_a, key_b, "別内容なのでキーも違うはず");
        assert!(
            !app.md_image_cache.contains_key(&key_a),
            "旧ターゲットのキーは prune されるはず"
        );
        assert!(app.md_image_cache.contains_key(&key_b));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Mutation-proving: if the eviction predicate in `apply_media_diff` degenerated into a no-op
    /// (never pruning), `switching_target_drops_the_old_media_diff_keys` above already fails — this
    /// test additionally pins that a **mermaid** key untouched by any media diff survives the same
    /// `retain` call (the predicate must gate on `is_media_diff_url`, not evict everything).
    #[test]
    fn eviction_never_touches_a_mermaid_key() {
        let dir = unique_tmp("konoma_media_diff_eviction_leaves_mermaid");
        std::fs::create_dir_all(&dir).unwrap();
        let a = dir.join("a.png");
        let b = dir.join("b.png");
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(4, 4, image::Rgb([1, 1, 1])))
            .save(&a)
            .unwrap();
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(6, 6, image::Rgb([2, 2, 2])))
            .save(&b)
            .unwrap();
        let mut app = App::new(dir.to_path_buf(), Config::default()).unwrap();
        let mermaid_key = PathBuf::from(crate::preview::markdown::mermaid_fence_url(
            "graph LR\nA-->B",
        ));
        app.md_image_cache
            .insert(mermaid_key.clone(), MdImgEntry::default());

        app.poll_media_diff(&a, 1, (400, 300));
        app.invalidate_media_diff();
        app.poll_media_diff(&b, 1, (400, 300));

        assert!(
            app.md_image_cache.contains_key(&mermaid_key),
            "mermaid キーは media-diff の prune に巻き込まれないはず"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    // ---- worker dispatch: dedup / coalescing / stale-gen (real channel, `attach_media_diff_loader`) ----
    //
    // Every test above either calls `App::compute_media_diff[_with_cap]` directly (no dispatch
    // machinery involved at all) or drives `poll_media_diff`/`apply_media_diff` through the
    // synchronous no-`Sender` fallback (`spawn_or_sync_media_diff`'s own doc comment) — which
    // bypasses `App::kick_media_diff`'s dedup/coalesce branch and `App::dispatch_media_diff`'s real
    // thread spawn entirely, since the sync path never leaves a request "in flight" for a second
    // call to observe. These tests attach a **real** `mpsc::channel` (`App::attach_media_diff_loader`,
    // the exact production wiring `main.rs` uses) so a request genuinely stays in flight between two
    // calls, and use `test_support::count_media_diff_dispatch_calls` to observe how many worker
    // threads were actually spawned — the only way to exercise `kick_media_diff`'s dedup-vs-coalesce
    // branch and `apply_media_diff`'s "dispatch the coalesced want" step at all.

    /// Two `poll_media_diff` calls for the identical `(path, page, raster_px)`, before anything has
    /// landed, dispatch only **one** worker — `media_diff_pending`'s own dedup check in
    /// `poll_media_diff`.
    #[test]
    fn poll_media_diff_dedups_two_identical_polls_into_one_dispatch() {
        let dir = unique_tmp("konoma_media_diff_dedup");
        std::fs::create_dir_all(&dir).unwrap();
        let png = dir.join("pic.png");
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(4, 4, image::Rgb([1, 2, 3])))
            .save(&png)
            .unwrap();
        let mut app = App::new(dir.to_path_buf(), Config::default()).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        app.attach_media_diff_loader(tx);

        let (_, dispatches) = crate::test_support::count_media_diff_dispatch_calls(|| {
            let _ = app.poll_media_diff(&png, 1, (400, 300));
            let _ = app.poll_media_diff(&png, 1, (400, 300));
        });
        assert_eq!(
            dispatches, 1,
            "同一 want を2回 poll しても dispatch は1回のはず"
        );

        let res = rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("worker が結果を返す");
        assert!(app.apply_media_diff(res), "現世代の結果は適用されるはず");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A raw raster image (PNG) doesn't depend on `raster_px` at all (`normalize_raster_px`'s own
    /// doc comment) — two polls for the same `(path, page)` but two different `raster_px` boxes
    /// (standing in for a terminal resize) still dispatch only **one** worker, and both requested
    /// boxes are answered by the single landed result once it arrives.
    #[test]
    fn two_polls_with_different_raster_px_for_a_png_dispatch_only_once() {
        let dir = unique_tmp("konoma_media_diff_raster_normalize");
        std::fs::create_dir_all(&dir).unwrap();
        let png = dir.join("pic.png");
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(4, 4, image::Rgb([1, 2, 3])))
            .save(&png)
            .unwrap();
        let mut app = App::new(dir.to_path_buf(), Config::default()).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        app.attach_media_diff_loader(tx);

        let (_, dispatches) = crate::test_support::count_media_diff_dispatch_calls(|| {
            let _ = app.poll_media_diff(&png, 1, (100, 100));
            let _ = app.poll_media_diff(&png, 1, (900, 700));
        });
        assert_eq!(
            dispatches, 1,
            "ラスタ画像は raster_px に依存しないので、異なる箱を求めても再 dispatch しないはず"
        );

        let res = rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("worker が結果を返す");
        assert!(app.apply_media_diff(res));
        assert!(
            app.poll_media_diff(&png, 1, (100, 100)).is_some(),
            "どちらの raster_px でも同じ(正規化された)着地を引けるはず"
        );
        assert!(app.poll_media_diff(&png, 1, (900, 700)).is_some());
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A burst of (invalidate + poll) cycles while a worker is already busy — an AI rewriting the
    /// same file repeatedly, or a rapid string of page turns — coalesces into the single
    /// `media_diff_queued` slot instead of piling up concurrent decodes: no extra dispatch happens
    /// until the busy worker's result lands, and then exactly **one** follow-up dispatch fires, for
    /// the *newest* want only (earlier coalesced wants are overwritten, never accumulated). The busy
    /// worker's own result, once it does land, is itself stale by then (three invalidations bumped
    /// `media_diff_gen` out from under it) — `apply_media_diff` returns `false` for it, proving the
    /// stale-gen discard still holds even while this coalescing machinery is what freed the slot.
    #[cfg(feature = "git")]
    #[test]
    fn coalesces_a_burst_of_invalidate_and_poll_into_one_follow_up_dispatch_for_the_newest_want() {
        let Some(pdf) = sample_path_or_skip("sample.pdf") else {
            return;
        };
        let bytes = std::fs::read(&pdf).unwrap();
        let dir = unique_tmp("konoma_media_diff_coalesce");
        std::fs::create_dir_all(&dir).unwrap();
        init_git_repo(&dir);
        let doc = dir.join("doc.pdf");
        std::fs::write(&doc, &bytes).unwrap();
        git(&dir, &["add", "-A"]);
        git(&dir, &["commit", "-q", "-m", "init"]);
        let mut app = App::new(dir.to_path_buf(), Config::default()).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        app.attach_media_diff_loader(tx);

        let (_, dispatches) = crate::test_support::count_media_diff_dispatch_calls(|| {
            // The first poll dispatches — the one worker slot is now busy.
            let _ = app.poll_media_diff(&doc, 1, (800, 600));
            // Three (invalidate + poll) bursts while that worker is still busy, each wanting a
            // different page — PDF pages, unlike a raster image, genuinely depend on the request
            // identity (`normalize_raster_px` leaves `Pdf`/`Svg` alone).
            for pg in [3u32, 4, 2] {
                app.invalidate_media_diff();
                let _ = app.poll_media_diff(&doc, pg, (800, 600));
            }
        });
        assert_eq!(
            dispatches, 1,
            "busy な間の invalidate+poll バーストは新規 dispatch を増やさないはず"
        );
        assert_eq!(
            app.media_diff_queued,
            Some((doc.clone(), 2, (800, 600))),
            "coalesce された want は最新のものだけ(上書き、蓄積ではない)のはず"
        );

        // The busy worker's own result lands — its generation was superseded by the three
        // invalidations above, so it's discarded...
        let stale_res = rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("busy だったワーカーが結果を返す");
        let (applied, dispatches) = crate::test_support::count_media_diff_dispatch_calls(|| {
            app.apply_media_diff(stale_res)
        });
        assert!(
            !applied,
            "3回 invalidate 済みなので gen が古く、この結果自体は捨てられるはず"
        );
        // ...but the slot is freed and the coalesced want (page 2) is dispatched immediately, in the
        // very same call.
        assert_eq!(
            dispatches, 1,
            "着地の瞬間に coalesce されていた want が1回だけ dispatch されるはず"
        );
        assert!(
            app.media_diff_queued.is_none(),
            "dispatch 後は queued スロットが空になるはず"
        );

        let res2 = rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("coalesce された want のワーカーが結果を返す");
        assert!(app.apply_media_diff(res2), "最新世代の結果は適用されるはず");
        let outcome = app.poll_media_diff(&doc, 2, (800, 600));
        assert!(
            matches!(outcome, Some(MediaDiffOutcome::Ready { .. })),
            "最新の want(page 2) の結果が着地しているはず: {outcome:?}"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// `media_diff_landed_for` must reject an old-generation landing: switching from A to B and
    /// back to A, with A having **changed on disk** while B was on screen, must re-kick a fresh
    /// computation for A rather than silently serving the stale (pre-change) landed result — the
    /// same `(path, page, raster_px)` key would otherwise still "match" if generation weren't part
    /// of the check.
    #[test]
    fn switching_a_to_b_to_a_with_a_changed_on_disk_re_kicks_rather_than_serving_stale_a() {
        let dir = unique_tmp("konoma_media_diff_a_b_a_stale");
        std::fs::create_dir_all(&dir).unwrap();
        let a = dir.join("a.png");
        let b = dir.join("b.png");
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(4, 4, image::Rgb([1, 1, 1])))
            .save(&a)
            .unwrap();
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(6, 6, image::Rgb([2, 2, 2])))
            .save(&b)
            .unwrap();
        let mut app = App::new(dir.to_path_buf(), Config::default()).unwrap();

        let outcome_a1 = app.poll_media_diff(&a, 1, (400, 300)).unwrap();
        let key_a1 = match outcome_a1 {
            MediaDiffOutcome::Ready {
                new: MediaDiffSide::Picture(p),
                ..
            } => p.cache_key,
            other => panic!("A(1回目): Ready のはず: {other:?}"),
        };

        app.invalidate_media_diff();
        let _ = app.poll_media_diff(&b, 1, (400, 300)).unwrap();

        // A changes on disk while B is on screen (e.g. an AI rewriting the file).
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(8, 8, image::Rgb([9, 9, 9])))
            .save(&a)
            .unwrap();

        app.invalidate_media_diff();
        let outcome_a2 = app.poll_media_diff(&a, 1, (400, 300)).unwrap();
        let (key_a2, natural_a2) = match outcome_a2 {
            MediaDiffOutcome::Ready {
                new: MediaDiffSide::Picture(p),
                ..
            } => (p.cache_key, p.natural_px),
            other => panic!("A(2回目): Ready のはず: {other:?}"),
        };
        assert_ne!(
            key_a1, key_a2,
            "内容が変わったのでキーも変わるはず(古い A の着地を使い回していない)"
        );
        assert_eq!(
            natural_a2,
            (8, 8),
            "変更後の A の実寸が反映されているはず(re-kick された証拠)"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A stale landing (its `gen` superseded) with **nothing** coalesced behind it
    /// (`media_diff_queued` empty at that moment) must still free the one worker slot —
    /// otherwise `media_diff_worker_busy` is stuck `true` forever (no queued want to dispatch, and
    /// no future worker is ever spawned to eventually call `apply_media_diff` again), and every
    /// later `poll_media_diff` for anything at all just silently coalesces into `media_diff_queued`
    /// without ever dispatching. `apply_media_diff`'s own doc comment says this is unconditional
    /// ("Always frees the one worker slot first") — this pins it against a mutant that only frees
    /// the slot inside the non-stale branch.
    #[test]
    fn a_stale_landing_with_nothing_queued_still_frees_the_worker_slot() {
        let dir = unique_tmp("konoma_media_diff_stale_no_queue_frees_slot");
        std::fs::create_dir_all(&dir).unwrap();
        let a = dir.join("a.png");
        let b = dir.join("b.png");
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(4, 4, image::Rgb([1, 1, 1])))
            .save(&a)
            .unwrap();
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(6, 6, image::Rgb([2, 2, 2])))
            .save(&b)
            .unwrap();
        let mut app = App::new(dir.to_path_buf(), Config::default()).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        app.attach_media_diff_loader(tx);

        // Dispatch for A — the one worker slot is now busy.
        let _ = app.poll_media_diff(&a, 1, (400, 300));
        // Invalidate (no further poll yet) — bumps gen, does NOT touch worker_busy/queued. The
        // in-flight A worker's eventual result is now stale, and nothing is queued behind it.
        app.invalidate_media_diff();
        assert!(
            app.media_diff_queued.is_none(),
            "前提: この時点で何も queue されていない"
        );

        // The busy (now-stale) worker's own result lands.
        let stale_res = rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("A のワーカーが結果を返す");
        assert!(
            !app.apply_media_diff(stale_res),
            "gen が古いので適用されないはず"
        );

        // A brand new want (B) must actually dispatch a fresh worker now — the slot must have been
        // freed by the stale landing above, even though nothing was queued to piggyback on.
        let (_, dispatches) = crate::test_support::count_media_diff_dispatch_calls(|| {
            let _ = app.poll_media_diff(&b, 1, (400, 300));
        });
        assert_eq!(
            dispatches, 1,
            "stale landing (キューなし) の後は worker slot が解放され、新規 want は即 dispatch されるはず \
             (解放されないと診断が永久に止まる)"
        );
        let res_b = rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("B のワーカーが結果を返す");
        assert!(
            app.apply_media_diff(res_b),
            "最新世代の B の結果は適用されるはず"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// PROBE (rereview, cross-tab): switching away from a tab showing a landed media diff to a
    /// non-media tab, and back, still shows both pictures — the picture cache is pruned on the
    /// switch-away (see `App::prune_media_diff_picture_cache`'s doc comment: any moment the active
    /// tab's view stops being a media diff frees the pixels), but the switch-back correctly
    /// re-kicks a fresh computation rather than leaving a stale placeholder.
    #[cfg(feature = "git")]
    #[test]
    fn switching_to_another_tab_and_back_redraws_both_pictures() {
        let dir = unique_tmp("konoma_media_diff_tab_switch_redraw");
        std::fs::create_dir_all(&dir).unwrap();
        let png = dir.join("pic.png");
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(4, 4, image::Rgb([1, 1, 1])))
            .save(&png)
            .unwrap();
        let mut app = App::new(dir.to_path_buf(), Config::default()).unwrap();

        // Tab 0: open a media diff and let it land.
        app.open_git_diff(&png);
        let outcome = app
            .poll_media_diff(&png, app.diff_media_page(), (400, 300))
            .unwrap();
        let key = match outcome {
            MediaDiffOutcome::Ready {
                new: MediaDiffSide::Picture(p),
                ..
            } => p.cache_key,
            other => panic!("Ready のはず: {other:?}"),
        };
        assert!(
            app.md_image_cache_contains(&key),
            "前提: 着地して cache に乗る"
        );

        // Tab 1: a fresh Tree tab, then switch away and back — a genuine load_active round trip
        // against a non-media target and then back to the still-open media diff.
        app.tab_new().unwrap();
        app.tab_goto(0);
        app.tab_goto(1);
        assert!(
            !app.md_image_cache_contains(&key),
            "非 media タブへ切り替えたら picture は破棄されるはず(メモリは解放される)"
        );

        app.tab_goto(0);
        let outcome2 = app
            .poll_media_diff(&png, app.diff_media_page(), (400, 300))
            .unwrap();
        assert!(
            matches!(
                outcome2,
                MediaDiffOutcome::Ready {
                    new: MediaDiffSide::Picture(_),
                    ..
                }
            ),
            "タブへ戻ったら re-kick されて絵が再着地するはず: {outcome2:?}"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A path that's become a **directory** (not a regular file) is treated the same as a deleted
    /// new side, not as an existing-but-unreadable file — `new_exists` means "a regular file is
    /// there", not merely "something answers `fs::metadata`". Classification for the new side falls
    /// back to sniffing the old bytes (as it would for a genuinely deleted path), and the new side
    /// itself lands `Absent`, not a spurious `Failed { "image decode failed" }`.
    #[test]
    fn a_directory_at_the_new_path_is_treated_as_absent_not_an_existing_unreadable_file() {
        let dir = unique_tmp("konoma_media_diff_new_side_is_directory");
        std::fs::create_dir_all(&dir).unwrap();
        let png = dir.join("was_a_file.png");
        // The old side has real committed bytes (a valid PNG); the new "file" is actually a
        // directory now — e.g. a rename/rewrite race, or an agent replacing a file with a folder.
        let mut old_bytes = Vec::new();
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(3, 3, image::Rgb([7, 7, 7])))
            .write_to(
                &mut std::io::Cursor::new(&mut old_bytes),
                image::ImageFormat::Png,
            )
            .unwrap();
        std::fs::create_dir_all(&png).unwrap(); // `png` is now a directory, not a file.

        let r = MediaDiffRequest {
            gen: 1,
            path: png,
            root: dir.to_path_buf(),
            baseline: DiffBaseline::FollowSnapshot(old_bytes.clone()),
            page: 1,
            raster_px: (400, 300),
            preview_rules: Config::default().preview.rules,
            preview_commands: true,
        };
        match App::compute_media_diff(&r) {
            MediaDiffComputed::Ready { old, new, .. } => {
                assert!(
                    matches!(old, MediaDiffSideDecoded::Picture(_)),
                    "旧版は実バイト列からデコードされるはず: {old:?}"
                );
                assert!(
                    matches!(new, MediaDiffSideDecoded::Absent),
                    "新版はディレクトリなので Absent のはず(Failed ではない): {new:?}"
                );
            }
            other => panic!("旧版が PNG として分類されるので Ready のはず: {other:?}"),
        }
        std::fs::remove_dir_all(&dir).ok();
    }
}
