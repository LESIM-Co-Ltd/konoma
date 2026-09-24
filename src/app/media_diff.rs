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
//! `#![allow(dead_code)]`: phase A of `docs/FEATURE-MEDIA-DIFF.md` — `attach_media_diff_loader`/
//! `apply_media_diff` are already wired into `main.rs`'s startup/run loop, but nothing in production
//! calls `App::poll_media_diff` (the entry point that actually kicks a request) yet; that's phase
//! B's job (wiring it into `render_gitdiff`). Every item here is exercised directly by this module's
//! own tests in the meantime, mirroring the precedent in `vcs::jj::base_contents`'s doc comment.
#![allow(dead_code)]

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
    pub(crate) fn poll_media_diff(
        &mut self,
        path: &Path,
        page: u32,
        raster_px: (u32, u32),
    ) -> Option<MediaDiffOutcome> {
        if let Some(outcome) = self.media_diff_landed_for(path, page, raster_px) {
            return Some(outcome);
        }
        let already_pending = self
            .media_diff_pending
            .as_ref()
            .is_some_and(|(p, pg, rp)| p.as_path() == path && *pg == page && *rp == raster_px);
        if !already_pending {
            self.kick_media_diff(path.to_path_buf(), page, raster_px);
        }
        self.media_diff_landed_for(path, page, raster_px)
    }

    /// Dispatch a fresh media-diff request for `(path, page, raster_px)`, discarding any earlier one
    /// (a bumped `gen` makes a stale in-flight result fail `apply_media_diff`'s staleness check on
    /// arrival — exactly `kick_md_diff`'s own shape).
    fn kick_media_diff(&mut self, path: PathBuf, page: u32, raster_px: (u32, u32)) {
        self.media_diff_gen = self.media_diff_gen.wrapping_add(1);
        self.media_diff_pending = Some((path.clone(), page, raster_px));
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
        let (old_bytes, base) = resolve_old_bytes(&req.baseline, &req.root, &req.path);
        let new_bytes = std::fs::read(&req.path).ok();
        let old_len = old_bytes.as_ref().map(|b| b.len() as u64);
        let new_len = new_bytes.as_ref().map(|b| b.len() as u64);
        let same_bytes = matches!((&old_bytes, &new_bytes), (Some(o), Some(n)) if o == n);

        let over_cap = old_len.is_some_and(|n| n > cap) || new_len.is_some_and(|n| n > cap);

        let preview_kind = if new_bytes.is_some() {
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

    /// Apply a media-diff result from the worker thread (or the synchronous fallback). Discards a
    /// stale generation (mirrors `App::apply_md_diff`). Otherwise:
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
        if res.gen != self.media_diff_gen {
            return false; // stale: a newer request/invalidation superseded this one.
        }
        self.media_diff_pending = None;
        let outcome = self.materialize_media_diff(res.computed);
        let live = live_cache_keys(&outcome);
        self.md_image_cache.retain(|k, _| {
            let s = k.to_string_lossy();
            !crate::preview::media_diff::is_media_diff_url(&s) || live.contains(k)
        });
        self.media_diff_landed = Some((res.path, res.page, res.raster_px, res.gen, outcome));
        true
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
    /// cleared here (otherwise `poll_media_diff`'s "already in flight" guard would keep declining to
    /// kick a fresh request, since that guard is keyed on the request identity, not `gen`).
    pub(super) fn invalidate_media_diff(&mut self) {
        self.media_diff_gen = self.media_diff_gen.wrapping_add(1);
        self.media_diff_landed = None;
        self.media_diff_pending = None;
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
    if let Some(frames) = crate::preview::image::decode_gif_bytes_inline(bytes) {
        let Some((first, _)) = frames.first() else {
            return MediaDiffSideDecoded::Failed {
                reason: "empty gif".to_string(),
            };
        };
        let (w, h) = first.dimensions();
        let first_frame = first.clone();
        let key = crate::preview::media_diff::media_diff_url(side, hash, page, None);
        return MediaDiffSideDecoded::Picture(Box::new(MediaDiffPictureDecoded {
            natural_px: (w, h),
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
    fn jj_scratch(name: &str) -> Option<PathBuf> {
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
        let r = req(png, dir.clone(), DiffBaseline::Empty);
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
        let r = req(png, dir.clone(), DiffBaseline::FollowSnapshot(bytes));
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
        let r = req(png, dir.clone(), DiffBaseline::FollowSnapshot(bytes));
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
        let r = req(png, dir.clone(), DiffBaseline::FollowSnapshot(good_old));
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
            dir.clone(),
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
            dir.clone(),
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
        let mut r = req(pdf, dir.clone(), DiffBaseline::FollowSnapshot(bytes));
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

        let r = req(png, dir.clone(), DiffBaseline::Vcs);
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
        let r = req(png, dir.clone(), DiffBaseline::FollowSnapshot(bytes));
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

        let r = req(png, dir.clone(), DiffBaseline::Empty);
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
        let r = req(png, dir.clone(), DiffBaseline::FollowHead { sha });
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

        let r = req(png, dir.clone(), DiffBaseline::FollowHead { sha });
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

        let r = req(png, dir.clone(), DiffBaseline::Vcs);
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
        let mut app = App::new(dir.clone(), Config::default()).unwrap();
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
        let mut app = App::new(dir.clone(), Config::default()).unwrap();
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
        let mut app = App::new(dir.clone(), Config::default()).unwrap();
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
        let mut app = App::new(dir.clone(), Config::default()).unwrap();
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
        let mut app = App::new(dir.clone(), Config::default()).unwrap();
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
        let mut app = App::new(dir.clone(), Config::default()).unwrap();
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
        let mut app = App::new(dir.clone(), Config::default()).unwrap();

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
        let mut app = App::new(dir.clone(), Config::default()).unwrap();
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
}
