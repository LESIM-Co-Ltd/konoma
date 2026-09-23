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
        // here too); `Rendered` reads the raw bytes under the same cap the follow baseline uses.
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
                    Ok(s) => s,
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
                    Ok(s) => s,
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
    /// `ui/preview.rs::render_decorated_body` reads this — alongside `App::md_diff_is_computing_
    /// placeholder` for the `Rendered` case — to defer consuming `App::take_diff_scroll_pending`
    /// (a follow jump's or a fresh diff open's "scroll to the first change" request) rather than
    /// burning it against a frame drawn before the gutter's own marks exist yet (`docs/STATUS.md`
    /// ★未修正 item 4): an ordinary preview's still-computing gutter builds a real, non-placeholder
    /// `MdCache` (full width, simply with no marks), so `is_diff_computing_placeholder` alone
    /// doesn't cover it.
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
}
