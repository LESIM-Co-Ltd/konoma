//! The Markdown block-diff computation offloaded to a background worker (`docs/STATUS.md` ★未修正
//! item 4, `docs/FEATURE-MD-RENDERED-DIFF.md` §2/§3) — the "旧版取得・前処理・diff_align" step that
//! used to run synchronously on the UI thread inside `App::ensure_md_cache` on every fresh open /
//! width change, costing ~64ms on a 20k-line document (jj's baseline fetch alone is a ~20-25ms
//! subprocess pair). Follows the exact shape `docs/STATUS.md`'s `async-git-status-scan` entry
//! documents for `StatusResult`/`IgnoredResult`: a `gen`-tagged request/result pair, a `_pending`
//! guard against duplicate dispatch, a synchronous fallback when no `Sender` is attached (tests),
//! and a `compute_or_fallback` panic net so a worker crash can never latch `md_diff_pending` forever.
//!
//! `App::compute_md_diff` is the **one** implementation of "resolve old/new text, preprocess,
//! `diff_align`" — both the worker thread and the synchronous fallback call it, and
//! `App::ensure_md_cache` (`md_render.rs`) no longer does any of that inline; it only re-parses the
//! already-diffed strings into `Doc`s (`Doc::parse`, ~1.7ms at 20k lines — cheap next to the I/O +
//! `block_ops` this offloads) when it actually renders.

use super::*;

impl App {
    /// Attach the Sender of the worker that computes Markdown block-diffs in the background
    /// (called by `main` at startup, mirroring `attach_status_loader`/`attach_git_loader`).
    pub fn attach_md_diff_loader(&mut self, tx: std::sync::mpsc::Sender<MdDiffResult>) {
        self.md_diff_tx = Some(tx);
    }

    /// Resolves `path`'s block-diff baseline into a [`DiffBaseline`] the worker can act on without
    /// ever touching `App` state itself. Mirrors `follow_baseline_contents`'s own branching
    /// (`docs/FEATURE-MD-RENDERED-DIFF.md` §2's baseline selection) but stops one step earlier: the
    /// "clean at follow-start → HEAD blob" branch resolves only the **sha** here (a plain field
    /// read, no I/O) and leaves the actual blob fetch (`git::blob_at`, a subprocess under jj) to the
    /// worker. `Gutter` never follows a session (the ordinary preview's own gutter always compares
    /// against the backend's committed baseline, matching the pre-worker `preview_diff_baseline`).
    #[cfg_attr(not(feature = "git"), allow(unused_variables))]
    fn diff_baseline(&self, path: &Path, kind: MdDiffKind) -> DiffBaseline {
        if kind == MdDiffKind::Gutter {
            return DiffBaseline::Vcs;
        }
        #[cfg(feature = "git")]
        {
            if self.diff_follow_scope && !self.follow_diff_full && self.follow_scope_valid() {
                let Some(base) = self.follow_baseline.as_ref() else {
                    return DiffBaseline::Empty;
                };
                let key = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
                return match base.dirty.get(&key) {
                    Some(Some(content)) => DiffBaseline::FollowSnapshot(content.clone()),
                    // Dirty at follow-start but too large to snapshot — no baseline (§5's "旧版が
                    // 無い" rule; `compute_md_diff` reads this as an empty old side for `Rendered`).
                    Some(None) => DiffBaseline::Empty,
                    None => match base.head.as_deref() {
                        Some(sha) => DiffBaseline::FollowHead {
                            sha: sha.to_string(),
                        },
                        None => DiffBaseline::Empty,
                    },
                };
            }
        }
        DiffBaseline::Vcs
    }

    /// Test-only direct access to `diff_baseline` — lets a test pin its `follow_scope_valid()` gate
    /// (M2, pre-merge review of PR #21) without a full worker round trip. `diff_baseline` itself
    /// stays private (`app/tests.rs`, a sibling module of `md_diff`, can't otherwise reach it). Every
    /// current caller is `#[cfg(feature = "git")]` (the follow-session branch it pins only exists on
    /// that build) — `allow(dead_code)`, not `cfg(feature = "git")`, on a no-`git` test build, same
    /// as `diff_view_for_test`'s own doc comment explains.
    #[cfg(test)]
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub(crate) fn diff_baseline_for_test(&self, path: &Path, kind: MdDiffKind) -> DiffBaseline {
        self.diff_baseline(path, kind)
    }

    /// The landed result for `(path, kind)`, if it matches the current generation — factored out of
    /// `poll_md_diff` so it can be checked both before *and* after a kick: the synchronous fallback
    /// (`spawn_or_sync_md_diff` with no `md_diff_tx` attached — every test that doesn't explicitly
    /// wire the channel) applies its result immediately, inside the kick itself, so a check only
    /// made *before* kicking would always see the pre-kick `None` and never observe it.
    fn md_diff_landed_for(&self, path: &Path, kind: MdDiffKind) -> Option<MdDiffOutcome> {
        match &self.md_diff_landed {
            Some((p, k, gen, outcome))
                if p.as_path() == path && *k == kind && *gen == self.md_diff_gen =>
            {
                Some(outcome.clone())
            }
            _ => None,
        }
    }

    /// Poll the landed block-diff for `(path, kind)`, kicking a fresh computation (`kick_md_diff`)
    /// if neither a matching landed result nor an in-flight request already covers this exact key.
    /// Returns `None` when not ready yet — the caller (`App::ensure_md_cache`) degrades for this
    /// frame (no gutter / a "computing…" placeholder) rather than blocking.
    pub(super) fn poll_md_diff(&mut self, path: &Path, kind: MdDiffKind) -> Option<MdDiffOutcome> {
        if let Some(outcome) = self.md_diff_landed_for(path, kind) {
            return Some(outcome);
        }
        let already_pending = self
            .md_diff_pending
            .as_ref()
            .is_some_and(|(p, k)| p.as_path() == path && *k == kind);
        if !already_pending {
            self.kick_md_diff(path.to_path_buf(), kind);
        }
        self.md_diff_landed_for(path, kind)
    }

    /// Dispatch a fresh block-diff request for `(path, kind)`, discarding any earlier one (a bumped
    /// `gen` makes a stale in-flight result fail `apply_md_diff`'s staleness check on arrival —
    /// exactly `kick_status_refresh`'s own shape).
    fn kick_md_diff(&mut self, path: PathBuf, kind: MdDiffKind) {
        self.md_diff_gen = self.md_diff_gen.wrapping_add(1);
        self.md_diff_pending = Some((path.clone(), kind));
        let gen = self.md_diff_gen;
        let baseline = self.diff_baseline(&path, kind);
        let req = MdDiffRequest {
            gen,
            path,
            root: self.tab.root.clone(),
            kind,
            baseline,
            md_frontmatter: self.cfg.ui.md_frontmatter,
            md_footnotes: self.cfg.ui.md_footnotes,
            md_inline_html: self.cfg.ui.md_inline_html,
        };
        self.spawn_or_sync_md_diff(req);
    }

    /// Compute the block-diff on a separate thread and return it via `md_diff_tx`. With no Sender
    /// attached (tests / no channel), fall back to **synchronous** computation and application —
    /// the same contract `spawn_or_sync_statuses`/`spawn_or_sync_ignored` already have, so unit
    /// tests that don't drive a run loop still observe the result immediately.
    fn spawn_or_sync_md_diff(&mut self, req: MdDiffRequest) {
        let Some(tx) = self.md_diff_tx.clone() else {
            let outcome = Self::compute_md_diff(&req);
            self.apply_md_diff(MdDiffResult {
                gen: req.gen,
                path: req.path,
                kind: req.kind,
                outcome,
            });
            return;
        };
        std::thread::spawn(move || {
            // `compute_or_fallback` (the same panic net `spawn_or_sync_statuses`/`_ignored` use):
            // without it, a panicking worker would send nothing, latch `md_diff_pending` forever,
            // and freeze the gutter/Rendered presentation on a spinner that never stops.
            let outcome = crate::preview::markdown::compute_or_fallback(
                || Self::compute_md_diff(&req),
                || MdDiffOutcome::Unavailable,
            );
            let _ = tx.send(MdDiffResult {
                gen: req.gen,
                path: req.path.clone(),
                kind: req.kind,
                outcome,
            });
        });
    }

    /// The pure block-diff computation (no `&self`) shared, byte-identical, by the worker thread
    /// and the synchronous fallback — the "resolve old/new text, preprocess, `diff_align`" step
    /// this whole module offloads. Every input is already plain data on `req` (`App::diff_baseline`
    /// resolved the baseline on the UI thread ahead of time), so this never touches `App`.
    ///
    /// Baseline first: for `Gutter`, a wholly missing baseline (untracked file / outside a repo)
    /// short-circuits to [`MdDiffOutcome::NoBaseline`] without even reading the current file — an
    /// ordinary preview then simply has no gutter, matching the pre-worker `GutterAlign::None`
    /// contract. `Rendered` never takes this path: §5's "旧版が無い…全ブロック Insert" is handled
    /// below by treating a missing baseline as an empty old side, not as a separate outcome.
    fn compute_md_diff(req: &MdDiffRequest) -> MdDiffOutcome {
        let old_bytes: Option<Vec<u8>> = match &req.baseline {
            DiffBaseline::Vcs => crate::vcs::base_contents(&req.root, &req.path),
            DiffBaseline::FollowSnapshot(bytes) => Some(bytes.clone()),
            DiffBaseline::FollowHead { sha } => {
                #[cfg(feature = "git")]
                {
                    Some(crate::git::blob_at(&req.root, sha, &req.path).unwrap_or_default())
                }
                #[cfg(not(feature = "git"))]
                {
                    let _ = sha;
                    None
                }
            }
            DiffBaseline::Empty => None,
        };
        if old_bytes.is_none() && req.kind == MdDiffKind::Gutter {
            return MdDiffOutcome::NoBaseline;
        }

        // The current file: `Gutter` reads it exactly as an ordinary preview does
        // (`preview::text::load`, which truncates rather than failing on an oversized file — see
        // `decorated_file_text`'s own doc comment for why the truncation suffix must be appended
        // here too). `Rendered` used to read the raw bytes under only the much looser
        // `FOLLOW_BASELINE_FILE_CAP` (5 MiB) and no line cap at all — a real bug (not the duplicate-
        // rendering one it looked like from the outside): a document past `preview::text::MAX_LINES`
        // (5,000 lines) renders *fully* here while the ordinary preview of the identical file stops
        // at 5,000, so the two presentations' own line counts were never comparable in the first
        // place for a large file, and rendering/wrapping the whole thing synchronously on the UI
        // thread is exactly the kind of unbounded work `docs/PRD.md`'s "UI is never blocked" principle
        // exists to prevent. `cap_for_display` (below) now applies the identical
        // `MAX_BYTES`/`MAX_LINES` cap `Gutter`'s own `preview::text::load` already enforces, via the
        // same `decorated_file_text` suffix — the `FOLLOW_BASELINE_FILE_CAP` checks below stay as a
        // separate, coarser safety net (§5's own "5MB 超 → rendered を諦める" — reading a truly huge
        // file into memory at all, unrelated to how much of it is then *displayed*).
        let new_raw = match req.kind {
            MdDiffKind::Gutter => match crate::preview::text::load(&req.path) {
                Ok(content) => super::md_render::decorated_file_text(&content),
                Err(_) => return MdDiffOutcome::Unavailable,
            },
            MdDiffKind::Rendered => {
                let bytes = match std::fs::read(&req.path) {
                    Ok(b) => b,
                    Err(_) => return MdDiffOutcome::Unavailable,
                };
                if bytes.len() > FOLLOW_BASELINE_FILE_CAP {
                    return MdDiffOutcome::Unavailable;
                }
                match String::from_utf8(bytes) {
                    Ok(s) => cap_for_display(s),
                    Err(_) => return MdDiffOutcome::Unavailable,
                }
            }
        };

        let old_raw = match old_bytes {
            None => String::new(), // Rendered's "no baseline = all added" (§5)
            Some(b) => {
                if req.kind == MdDiffKind::Rendered && b.len() > FOLLOW_BASELINE_FILE_CAP {
                    return MdDiffOutcome::Unavailable;
                }
                match String::from_utf8(b) {
                    // The baseline gets the identical display cap the current-file side already
                    // gets, for **both** kinds — `Gutter`'s own old side used to be read straight
                    // through, uncapped, while its own new side (`preview::text::load`, above) was
                    // always capped at `MAX_LINES`. For any committed file past that line count, an
                    // *entirely unchanged* file therefore diffed the capped new content against the
                    // baseline's full, uncapped tail — a spurious "everything past line 5,000 was
                    // deleted" — which activated the gutter (and shifted `render_width` by 1) on a
                    // file with no real changes in its own displayed portion at all. Found while
                    // pinning `Rendered`'s own parity against this exact presentation
                    // (`app::tests::diff_rendered_matches_the_ordinary_preview_for_a_document_past_the_line_cap`).
                    Ok(s) => cap_for_display(s),
                    // Gutter: an undecodable baseline is just "no baseline to compare" (never
                    // attempted as Markdown, same as `preview_diff_baseline`'s old contract).
                    Err(_) if req.kind == MdDiffKind::Gutter => return MdDiffOutcome::NoBaseline,
                    Err(_) => return MdDiffOutcome::Unavailable,
                }
            }
        };

        let old_pre = preprocess_md_src_pure(
            &old_raw,
            req.md_frontmatter,
            req.md_footnotes,
            req.md_inline_html,
        );
        let new_pre = preprocess_md_src_pure(
            &new_raw,
            req.md_frontmatter,
            req.md_footnotes,
            req.md_inline_html,
        );
        let (_, _, ops) = crate::preview::markdown::diff_align(&old_pre, &new_pre);
        let any_change = crate::preview::markdown::ops_has_any_change(&ops);
        // The block-diff found nothing (front matter is stripped before either side reaches
        // `Doc::parse`), yet the raw bytes differ — §5's "front matter だけの変更" degeneration.
        // Computed straight from the two raw strings already in hand rather than depending on the
        // separately-cached unified line-diff (`git_diff_lines`) the pre-worker `apply_diff_view`
        // used for the identical question: this keeps the whole decision self-contained on the
        // worker, with no synchronous call back into `App` needed once the result lands.
        let front_matter_only = !any_change && old_raw != new_raw;

        MdDiffOutcome::Ready {
            old_pre,
            new_pre,
            ops,
            any_change,
            front_matter_only,
        }
    }

    /// Apply a block-diff result from the worker thread (or the synchronous fallback). Discards a
    /// stale generation (the file/kind changed, or the working tree did, while it was computing) —
    /// returns `false` in that case so the caller doesn't redraw for nothing.
    ///
    /// For `Rendered`, this is also where `App::apply_diff_view`'s old synchronous validation now
    /// happens — deferred to the moment the result actually lands, not the moment the presentation
    /// was requested (`docs/FEATURE-MD-RENDERED-DIFF.md` §5's "描画中に状態を変えて表示に頼る設計を
    /// やめる": this runs from the run loop's message-draining step, never from the render path).
    /// Only touches `tab.diff_view`/`flash` when the result is still relevant to what's on screen —
    /// the user may have cycled `R`/navigated away while it was computing.
    pub fn apply_md_diff(&mut self, res: MdDiffResult) -> bool {
        if res.gen != self.md_diff_gen {
            return false; // stale: the file/kind/working-tree changed since this was kicked
        }
        self.md_diff_pending = None;
        let on_screen = self.tab.preview_path.as_deref() == Some(res.path.as_path());
        if res.kind == MdDiffKind::Rendered && on_screen && self.tab.diff_view == DiffView::Rendered
        {
            match &res.outcome {
                MdDiffOutcome::Unavailable => {
                    self.tab.diff_view = DiffView::Source;
                    // `Rendered` is no longer reachable for this path — the "scroll to first change"
                    // reservation `App::open_git_diff_with`/`App::cycle_diff_view` armed for it (if
                    // any) will never be consumed by `render_decorated_body` now that `Source` draws
                    // through a different renderer entirely. Drop it explicitly rather than leave it
                    // dangling on `res.path` until some later, unrelated event clears it.
                    if self.tab.diff_scroll_pending.as_deref() == Some(res.path.as_path()) {
                        self.tab.diff_scroll_pending = None;
                    }
                    self.flash =
                        Some(tr(self.lang, crate::i18n::Msg::DiffRenderedUnavailable).into());
                }
                MdDiffOutcome::Ready {
                    front_matter_only: true,
                    ..
                } => {
                    self.flash =
                        Some(tr(self.lang, crate::i18n::Msg::DiffRenderedFrontMatterOnly).into());
                }
                _ => {}
            }
        }
        self.md_diff_landed = Some((res.path.clone(), res.kind, res.gen, res.outcome));
        // The next `ensure_md_cache` rebuild picks up the now-ready result (the block-diff itself
        // is already computed; only `Doc::parse` — cheap — remains synchronous).
        if on_screen {
            self.md_cache = None;
        }
        true
    }

    /// Whether a block-diff for the **currently previewed path** is still in flight (either kind).
    /// `ui/preview.rs::render_decorated_body` reads this alone (pre-merge review of PR #21 found the
    /// guard it used to sit alongside — `App::md_diff_is_computing_placeholder`, checking
    /// `MdCache::is_diff_computing_placeholder` — strictly redundant here and removed it) to defer
    /// consuming `App::take_diff_scroll_pending_for` (a follow jump's or a fresh diff open's "scroll
    /// to the first change" request) rather than burning it against a frame drawn before the
    /// gutter's own marks exist yet (`docs/STATUS.md` ★未修正 item 4).
    ///
    /// Covers both shapes the "still computing" frame can take: the `Rendered` presentation's own
    /// placeholder body (`App::md_diff_computing_cache`) is built **only** when `poll_md_diff`
    /// returns `None` for the current `(path, Rendered)`, which is exactly when this is `true` for
    /// that path — so the old placeholder check could never observe anything this one didn't already
    /// cover. An ordinary preview's still-computing *gutter*, by contrast, has no placeholder body at
    /// all (`ensure_md_cache`'s `File` branch draws the real content immediately, simply without a
    /// gutter column yet) — this check is the *only* thing that defers scroll consumption for that
    /// case, which is why it can't be dropped in the other direction.
    pub(crate) fn md_diff_pending_for_current(&self) -> bool {
        let Some(path) = self.tab.preview_path.as_deref() else {
            return false;
        };
        matches!(&self.md_diff_pending, Some((p, _)) if p.as_path() == path)
    }

    /// Drop the landed block-diff and bump the generation, so any result already in flight is
    /// discarded on arrival (`App::apply_md_diff`'s gen check) — called wherever the working
    /// tree/baseline this compares against may have changed: `App::invalidate_diff_caches` (FS
    /// refresh, follow session (re)start, the `f` scope toggle, opening another file's diff) and
    /// `App::reload_preview` (an ordinary preview's own gutter baseline, reached by the same
    /// FS-refresh path but not routed through `invalidate_diff_caches` — see that fn's own doc
    /// comment on why it deliberately leaves an ordinary `md_cache` alone).
    ///
    /// Also clears `md_diff_pending`, even though a computation may genuinely still be running for
    /// the old generation: without this, `App::poll_md_diff`'s "already in flight for this
    /// `(path, kind)`" guard would keep declining to kick a fresh one — since that guard is keyed
    /// on `(path, kind)`, not `gen` — and the view would stay stuck on stale content until the
    /// *old* worker happens to land, gets discarded, and nothing re-kicks after it. The old
    /// worker's own result becoming irrelevant the moment it lands is an acceptable, bounded cost
    /// (one extra in-flight computation, briefly) — unlike `git_status_dirty`'s coalescing, which
    /// exists because *every* FS event during a write burst re-validates the same single root;
    /// `invalidate_md_diff` fires only when the specific file/baseline on screen actually needs a
    /// new comparison, not on a hot loop.
    pub(super) fn invalidate_md_diff(&mut self) {
        self.md_diff_gen = self.md_diff_gen.wrapping_add(1);
        self.md_diff_landed = None;
        self.md_diff_pending = None;
    }
}

/// Bounds an already-decoded Markdown string to the same display cap every other Markdown preview
/// in konoma already enforces (`preview::text::MAX_BYTES`/`MAX_LINES`, applied via `cap_lines` —
/// the identical function `preview::text::load` itself calls), and appends the identical
/// "— (省略: 表示上限に達しました) —" notice `decorated_file_text` already appends for the ordinary
/// preview/`Gutter`'s own current-file side when it fires. Two call sites used to skip this, each its
/// own bug found while investigating the same symptom (a `Rendered` diff rendering ~4x the line count
/// of an "equivalent" ordinary preview):
///
/// * `new_raw` for `MdDiffKind::Rendered` used to read/render the whole file under nothing but the
///   much looser `FOLLOW_BASELINE_FILE_CAP` (5 MiB, meant to bound the *follow baseline snapshot*
///   feature's memory use, not how much of a file a preview ever displays) — a document past
///   `MAX_LINES` rendered fully in `Rendered` while the ordinary preview of the identical file
///   stopped at `MAX_LINES`, leaving `Rendered`'s own decoration/wrap pass (run synchronously on the
///   UI thread) with unbounded work.
/// * `old_raw` (the baseline) for **both** kinds used to be read straight through, uncapped, even
///   though the corresponding current-file side (`new_raw`) was always capped (`Gutter`, via
///   `preview::text::load`) or is capped now (`Rendered`, this function). For any committed file
///   past `MAX_LINES`, an *entirely unchanged* file therefore diffed the capped new content against
///   the baseline's full, uncapped tail — a spurious "everything past the cap was deleted" — which
///   activated `Gutter`'s own column (and shifted the render width by 1 cell) for a file with no real
///   change in its own displayed portion at all.
///
/// Called on `new_raw` for `MdDiffKind::Rendered` and on `old_raw` for both kinds.
fn cap_for_display(s: String) -> String {
    let (lines, truncated) = crate::preview::text::cap_lines(s.as_bytes());
    if !truncated {
        return s;
    }
    super::md_render::decorated_file_text(&crate::preview::text::TextContent { lines, truncated })
}

/// The exact pre-pass chain `build_decorated`/`build_decorated_file` apply to a Markdown file's raw
/// bytes before handing the result to the renderer — front matter strip, then footnotes, then
/// inline HTML, each gated by its own `[ui] md_*` setting. A free function (not an `&App` method)
/// so `App::compute_md_diff` can call it from the worker thread with a plain snapshot of the three
/// booleans (`MdDiffRequest::md_frontmatter`/`md_footnotes`/`md_inline_html`) instead of `&App`.
fn preprocess_md_src_pure(
    src: &str,
    frontmatter: bool,
    footnotes: bool,
    inline_html: bool,
) -> String {
    let src = if frontmatter {
        crate::preview::markdown::strip_front_matter(src).1
    } else {
        src.to_string()
    };
    let origin = crate::preview::markdown::identity_origin(&src);
    let (src, origin) = if footnotes {
        crate::preview::markdown::process_footnotes_traced(&src, &origin)
    } else {
        (src, origin)
    };
    let (src, _origin) = if inline_html {
        crate::preview::markdown::process_inline_html_traced(&src, &origin)
    } else {
        (src, origin)
    };
    src
}

#[cfg(test)]
mod tests {
    //! `App::compute_md_diff` in isolation — no App/tree/render machinery, just the pure
    //! computation. `DiffBaseline::FollowSnapshot`/`Empty` need no git repository at all (only the
    //! `Vcs` variant does), so these run on both feature builds.

    use super::*;
    use crate::test_support::unique_tmp;

    fn req(path: PathBuf, kind: MdDiffKind, baseline: DiffBaseline) -> MdDiffRequest {
        MdDiffRequest {
            gen: 1,
            root: path.parent().unwrap().to_path_buf(),
            path,
            kind,
            baseline,
            md_frontmatter: true,
            md_footnotes: true,
            md_inline_html: true,
        }
    }

    #[test]
    fn ready_marks_a_real_change_and_is_not_front_matter_only() {
        let dir = unique_tmp("konoma_compute_md_diff_real_change");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("doc.md");
        std::fs::write(&path, "# Title\n\nCHANGED.\n").unwrap();
        let r = req(
            path,
            MdDiffKind::Rendered,
            DiffBaseline::FollowSnapshot(b"# Title\n\nOriginal.\n".to_vec()),
        );
        match App::compute_md_diff(&r) {
            MdDiffOutcome::Ready {
                any_change,
                front_matter_only,
                ops,
                ..
            } => {
                assert!(any_change, "本文が変わっているので any_change のはず");
                assert!(!front_matter_only, "front matter だけの変更ではない");
                assert!(
                    crate::preview::markdown::ops_has_any_change(&ops),
                    "any_change と ops の実際の中身が一致するはず"
                );
            }
            other => panic!("Ready のはず: {other:?}"),
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn ready_is_inactive_for_byte_identical_content() {
        let dir = unique_tmp("konoma_compute_md_diff_identical");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("doc.md");
        let body = "# Title\n\nSame.\n";
        std::fs::write(&path, body).unwrap();
        let r = req(
            path,
            MdDiffKind::Rendered,
            DiffBaseline::FollowSnapshot(body.as_bytes().to_vec()),
        );
        match App::compute_md_diff(&r) {
            MdDiffOutcome::Ready {
                any_change,
                front_matter_only,
                ..
            } => {
                assert!(!any_change, "同一内容では変更なしのはず");
                assert!(
                    !front_matter_only,
                    "raw バイトも同一なので front_matter_only でもない\
                     (単なる無変更)"
                );
            }
            other => panic!("Ready のはず: {other:?}"),
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn ready_flags_front_matter_only_when_raw_bytes_differ_but_body_does_not() {
        let dir = unique_tmp("konoma_compute_md_diff_front_matter_only");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("doc.md");
        let new_raw = "---\ntitle: New\n---\n\n# Title\n\nBody.\n";
        std::fs::write(&path, new_raw).unwrap();
        let old_raw = "---\ntitle: Old\n---\n\n# Title\n\nBody.\n";
        let r = req(
            path,
            MdDiffKind::Rendered,
            DiffBaseline::FollowSnapshot(old_raw.as_bytes().to_vec()),
        );
        match App::compute_md_diff(&r) {
            MdDiffOutcome::Ready {
                any_change,
                front_matter_only,
                ..
            } => {
                assert!(
                    !any_change,
                    "front matter はプリプロセスで除去されるので本文は無変更のはず"
                );
                assert!(
                    front_matter_only,
                    "raw バイトは違うので front_matter_only のはず"
                );
            }
            other => panic!("Ready のはず: {other:?}"),
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn gutter_kind_with_no_baseline_is_no_baseline_not_ready() {
        let dir = unique_tmp("konoma_compute_md_diff_gutter_no_baseline");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("doc.md");
        std::fs::write(&path, "# Title\n").unwrap();
        let r = req(path, MdDiffKind::Gutter, DiffBaseline::Empty);
        assert!(
            matches!(App::compute_md_diff(&r), MdDiffOutcome::NoBaseline),
            "Gutter はベースライン無しでは NoBaseline のはず(全追加にはしない)"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn rendered_kind_with_no_baseline_is_ready_all_added() {
        let dir = unique_tmp("konoma_compute_md_diff_rendered_no_baseline");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("doc.md");
        std::fs::write(&path, "# Title\n\nAll new.\n").unwrap();
        let r = req(path, MdDiffKind::Rendered, DiffBaseline::Empty);
        match App::compute_md_diff(&r) {
            MdDiffOutcome::Ready {
                any_change, ops, ..
            } => {
                assert!(any_change, "旧版が空なので全ブロック Insert のはず");
                assert!(
                    ops.iter()
                        .all(|op| matches!(op, crate::preview::markdown::BlockOp::Insert { .. })),
                    "§5『旧版が無い→全ブロック Insert』: {ops:?}"
                );
            }
            other => panic!("Ready のはず: {other:?}"),
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn rendered_kind_over_the_size_cap_is_unavailable() {
        let dir = unique_tmp("konoma_compute_md_diff_over_cap");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("doc.md");
        // FOLLOW_BASELINE_FILE_CAP is 5MiB; write just over it.
        let big = "x".repeat(FOLLOW_BASELINE_FILE_CAP + 1);
        std::fs::write(&path, &big).unwrap();
        let r = req(path, MdDiffKind::Rendered, DiffBaseline::Empty);
        assert!(
            matches!(App::compute_md_diff(&r), MdDiffOutcome::Unavailable),
            "上限超の新版は Unavailable のはず"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// M11 (pre-merge review of PR #21): the *baseline's own* `FOLLOW_BASELINE_FILE_CAP` check
    /// (`b.len() > FOLLOW_BASELINE_FILE_CAP`, right before the `String::from_utf8` in
    /// `App::compute_md_diff`) is gated `req.kind == MdDiffKind::Rendered` only — `Gutter`'s old
    /// side has **no such gate at all**, only the post-decode line cap (`cap_for_display`) applies
    /// once it's already a valid `String`. Pins that this asymmetry is the current, deliberate
    /// contract (not an accidental omission that should also reject a huge `Gutter` baseline
    /// outright): a baseline well over the cap is `Unavailable` for `Rendered` but still `Ready` for
    /// `Gutter`, given the identical bytes.
    #[test]
    fn baseline_file_cap_gates_rendered_only_not_gutter() {
        let dir = unique_tmp("konoma_baseline_file_cap_kind_asymmetry");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("doc.md");
        std::fs::write(&path, "line\n".repeat(10)).unwrap();
        // Comfortably over FOLLOW_BASELINE_FILE_CAP (5 MiB), valid UTF-8.
        let big_old = "x".repeat(FOLLOW_BASELINE_FILE_CAP + 1);

        let r_rendered = req(
            path.clone(),
            MdDiffKind::Rendered,
            DiffBaseline::FollowSnapshot(big_old.clone().into_bytes()),
        );
        assert!(
            matches!(
                App::compute_md_diff(&r_rendered),
                MdDiffOutcome::Unavailable
            ),
            "Rendered は旧版が上限超なら Unavailable のはず"
        );

        let r_gutter = req(
            path,
            MdDiffKind::Gutter,
            DiffBaseline::FollowSnapshot(big_old.into_bytes()),
        );
        match App::compute_md_diff(&r_gutter) {
            MdDiffOutcome::Ready { .. } => {} // Gutter has no file-size gate on the old side.
            other => panic!("Gutter は旧版の上限を見ないので Ready のはず: {other:?}"),
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn gutter_kind_reads_the_current_file_through_text_load_not_raw_bytes() {
        // Gutter must go through `preview::text::load` + `decorated_file_text` (truncation-aware),
        // not a raw `fs::read` — a nonexistent file therefore degrades to `Unavailable` the same
        // way `text::load`'s own error path does, not a panic or an empty-content false match.
        let dir = unique_tmp("konoma_compute_md_diff_gutter_missing_file");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("does-not-exist.md");
        let r = req(
            path,
            MdDiffKind::Gutter,
            DiffBaseline::FollowSnapshot(b"# Title\n".to_vec()),
        );
        assert!(
            matches!(App::compute_md_diff(&r), MdDiffOutcome::Unavailable),
            "新版が読めない場合は Unavailable のはず"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The regression this whole group pins: `Rendered` used to read/render a document's *entire*
    /// content under nothing but the much looser `FOLLOW_BASELINE_FILE_CAP` (5 MiB, no line cap at
    /// all), while `Gutter`/the ordinary preview always stop at `preview::text::MAX_LINES` (5,000) —
    /// so a document past that line count rendered ~4x more content in `Rendered` than the "ordinary
    /// preview" of the identical file ever showed, which looked like duplicate rendering from the
    /// outside but was actually two presentations of the same file simply never agreeing on how much
    /// of it to display. `cap_for_display` closes that gap: both `old_pre`/`new_pre` must
    /// come out capped to the exact same content `preview::text::cap_lines` (the function `Gutter`'s
    /// own `preview::text::load` already calls) would keep, with the identical truncation notice
    /// `decorated_file_text` appends.
    #[test]
    fn rendered_kind_caps_new_content_the_same_way_the_ordinary_preview_does() {
        let dir = unique_tmp("konoma_compute_md_diff_rendered_line_cap_new");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("doc.md");
        // Comfortably past `preview::text::MAX_LINES` (5,000) — each line unique so a byte-for-byte
        // comparison against `cap_lines`'s own output is unambiguous.
        let mut big = String::new();
        for i in 0..6000 {
            big.push_str(&format!("line {i}\n"));
        }
        std::fs::write(&path, &big).unwrap();
        // Preprocessing (front matter/footnotes/inline HTML) off — the fixture has none of those
        // constructs, so this isolates the truncation behavior under test from the separate
        // pre-pass chain `preprocess_md_src_pure` still applies afterward in production.
        let mut r = req(path, MdDiffKind::Rendered, DiffBaseline::Empty);
        r.md_frontmatter = false;
        r.md_footnotes = false;
        r.md_inline_html = false;
        let new_pre = match App::compute_md_diff(&r) {
            MdDiffOutcome::Ready { new_pre, .. } => new_pre,
            other => panic!("Ready のはず: {other:?}"),
        };
        let (capped_lines, truncated) = crate::preview::text::cap_lines(big.as_bytes());
        assert!(truncated, "6000 行は MAX_LINES を超えるはず(テストの前提)");
        let expected = super::md_render::decorated_file_text(&crate::preview::text::TextContent {
            lines: capped_lines,
            truncated,
        });
        assert_eq!(
            new_pre, expected,
            "Rendered の new_pre は Gutter/通常プレビューと同じ上限・同じ省略通知で切り詰められるはず"
        );
        assert!(
            !new_pre.contains("line 5999"),
            "上限を超えた末尾はもう描かれないはず: {}",
            &new_pre[new_pre.len().saturating_sub(200)..]
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The mirror of the above for the **old** (baseline) side: a baseline past `MAX_LINES` is
    /// capped identically, not just the current file. Without this, a change confined to the current
    /// file's own capped prefix could still classify as `Replace`d against an *uncapped* old block
    /// that no longer has any capped counterpart on the new side, producing spurious `Delete`s for
    /// every old block past the cap instead of simply not comparing that tail at all (the same
    /// "nothing past the cap is part of this presentation" contract the ordinary preview already
    /// has).
    #[test]
    fn rendered_kind_caps_baseline_content_the_same_way() {
        let dir = unique_tmp("konoma_compute_md_diff_rendered_line_cap_old");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("doc.md");
        let mut big = String::new();
        for i in 0..6000 {
            big.push_str(&format!("line {i}\n"));
        }
        std::fs::write(&path, &big).unwrap();
        let mut r = req(
            path,
            MdDiffKind::Rendered,
            DiffBaseline::FollowSnapshot(big.as_bytes().to_vec()),
        );
        r.md_frontmatter = false;
        r.md_footnotes = false;
        r.md_inline_html = false;
        let old_pre = match App::compute_md_diff(&r) {
            MdDiffOutcome::Ready { old_pre, .. } => old_pre,
            other => panic!("Ready のはず: {other:?}"),
        };
        let (capped_lines, truncated) = crate::preview::text::cap_lines(big.as_bytes());
        let expected = super::md_render::decorated_file_text(&crate::preview::text::TextContent {
            lines: capped_lines,
            truncated,
        });
        assert_eq!(
            old_pre, expected,
            "Rendered の old_pre(baseline) も同じ上限で切り詰められるはず"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A document *under* both caps is unaffected byte for byte — `cap_for_display` must be
    /// a true no-op below the threshold, matching `cap_lines_is_a_noop_under_both_caps`
    /// (`preview/text.rs`) at this call site too.
    #[test]
    fn rendered_kind_leaves_a_small_document_untouched() {
        let dir = unique_tmp("konoma_compute_md_diff_rendered_small_untouched");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("doc.md");
        let body = "# Title\n\nA small, ordinary document.\n";
        std::fs::write(&path, body).unwrap();
        let r = req(path, MdDiffKind::Rendered, DiffBaseline::Empty);
        let new_pre = match App::compute_md_diff(&r) {
            MdDiffOutcome::Ready { new_pre, .. } => new_pre,
            other => panic!("Ready のはず: {other:?}"),
        };
        assert_eq!(
            new_pre, body,
            "上限未満では切り詰めもサフィックスも入らないはず"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A second, adjacent bug found while pinning the above: `Gutter`'s own baseline (`old_raw`) used
    /// to be read straight through, uncapped, even though its own current-file side (`new_raw`) was
    /// already capped via `preview::text::load`. For a **byte-identical** baseline/current pair past
    /// `MAX_LINES`, that asymmetry alone used to make `block_ops` see a spurious trailing deletion
    /// (capped new vs. uncapped old) — `ops_has_any_change` came back `true` for a file with no real
    /// change in its own displayed portion at all, which is exactly what this test pins the negative
    /// of. Mirrors `ready_is_inactive_for_byte_identical_content`, just past the line cap.
    #[test]
    fn gutter_kind_is_inactive_for_a_byte_identical_baseline_past_the_line_cap() {
        let dir = unique_tmp("konoma_compute_md_diff_gutter_identical_past_cap");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("doc.md");
        let mut big = String::new();
        for i in 0..6000 {
            big.push_str(&format!("line {i}\n"));
        }
        std::fs::write(&path, &big).unwrap();
        let r = req(
            path,
            MdDiffKind::Gutter,
            DiffBaseline::FollowSnapshot(big.as_bytes().to_vec()),
        );
        match App::compute_md_diff(&r) {
            MdDiffOutcome::Ready {
                any_change, ops, ..
            } => {
                assert!(
                    !any_change,
                    "バイト同一のベースラインなので無変更のはず(旧: 切り詰め非対称で偽の削除): \
                     {ops:?}"
                );
            }
            other => panic!("Ready のはず: {other:?}"),
        }
        std::fs::remove_dir_all(&dir).ok();
    }
}
