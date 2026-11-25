//! Markdown diff computation and generation ownership, independent of a host application.

/// The ordinary preview's gutter or the diff's rendered Markdown presentation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MdDiffKind {
    Gutter,
    Rendered,
}

/// An instruction for resolving the old side on the computation thread.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiffBaseline {
    Vcs,
    FollowSnapshot(Vec<u8>),
    FollowHead { sha: String },
    Empty,
}

/// Already captured follow-start facts for one file. `Some(None)` is a skipped snapshot;
/// `None` means the file was clean at follow-start. Backend and live file status do not affect
/// this selection: the VCS resolves `Vcs` and `FollowHead` on the worker thread.
pub struct FollowBaseline<'a> {
    pub dirty: Option<Option<&'a [u8]>>,
    pub head: Option<&'a str>,
}

use std::path::{Path, PathBuf};
use std::sync::{mpsc::Sender, Arc};

pub const FOLLOW_BASELINE_FILE_CAP: usize = 5 * 1024 * 1024;

#[derive(Clone)]
pub struct MdDiffRequest {
    pub gen: u64,
    pub path: PathBuf,
    pub root: PathBuf,
    pub kind: MdDiffKind,
    pub baseline: DiffBaseline,
    pub md_frontmatter: bool,
    pub md_footnotes: bool,
    pub md_inline_html: bool,
}

/// Outcome of a Markdown block-diff computation (`docs/FEATURE-MD-RENDERED-DIFF.md` §5's
/// degenerate cases). `Doc`s are never sent across the channel (they borrow from `old_pre`/
/// `new_pre`, which would make this self-referential) — the UI thread re-parses them from these
/// owned strings when it actually needs to render (`App::ensure_md_cache`), a cheap `Doc::parse`
/// (~1.7ms at 20k lines) compared to the I/O + preprocessing + `block_ops` this offloads.
#[derive(Clone, Debug)]
pub enum MdDiffOutcome {
    Ready {
        old_pre: String,
        new_pre: String,
        ops: Vec<konoma_widgets::markdown::BlockOp>,
        /// Whether `ops` marks any change at all — `GutterAlign::is_active`'s own `ops_has_any_change`
        /// re-derives this from `ops` for production use (so it's dead code outside tests), but
        /// tests read it directly off a landed outcome without needing an `align` to hand.
        #[cfg_attr(not(test), allow(dead_code))]
        any_change: bool,
        /// The block-diff found no change (`!any_change`) even though the raw file bytes differ —
        /// the only difference is in front matter, which is stripped before either side ever
        /// reaches `Doc::parse`. Only meaningful for `MdDiffKind::Rendered` (`App::apply_md_diff`
        /// flashes `DiffRenderedFrontMatterOnly`); `Gutter` never reads it.
        front_matter_only: bool,
    },
    /// The current file is over the size cap, not valid UTF-8, or unreadable. `Rendered` falls
    /// back to `Source` with a flash (`App::apply_md_diff`); `Gutter` simply shows no gutter.
    Unavailable,
    /// No committed baseline exists at all (untracked file / outside a repo). Only ever returned
    /// for `MdDiffKind::Gutter` — an ordinary preview then has no gutter, matching the pre-worker
    /// `GutterAlign::None` contract. `MdDiffKind::Rendered` never returns this: a missing baseline
    /// there is §5's "旧版が無い…全ブロック Insert", which `App::compute_md_diff` already resolves
    /// into an ordinary `Ready` (comparing against an empty old side) rather than a separate case.
    NoBaseline,
}

/// Result of a Markdown block-diff computed on a separate thread, returned via `App::md_diff_tx`.
/// Staleness is judged by `gen` exactly like [`StatusResult`]/[`IgnoredResult`].
#[derive(Clone, Debug)]
pub struct MdDiffResult {
    pub gen: u64,
    pub path: PathBuf,
    pub kind: MdDiffKind,
    pub outcome: MdDiffOutcome,
}

/// Every new want starts a computation. Generations reject superseded results in either order.
#[derive(Default)]
pub struct MdDiffWorker {
    read_context: super::DiffReadContext,
    gen: u64,
    pending: Option<(PathBuf, MdDiffKind)>,
    landed: Option<(PathBuf, MdDiffKind, u64, MdDiffOutcome)>,
    tx: Option<Sender<MdDiffResult>>,
    compute: Option<Arc<MdCompute>>,
}

type MdCompute = dyn Fn(&MdDiffRequest) -> MdDiffOutcome + Send + Sync;

impl MdDiffWorker {
    /// Set the choice copied into each dispatched computation, including synchronous fallback.
    pub fn with_read_context(mut self, context: super::DiffReadContext) -> Self {
        self.read_context = context;
        self
    }

    pub fn compute_md_diff(req: &MdDiffRequest) -> MdDiffOutcome {
        Self::compute_with_context(req, super::DiffReadContext::default())
    }

    fn compute_with_context(req: &MdDiffRequest, context: super::DiffReadContext) -> MdDiffOutcome {
        let old_bytes: Option<Vec<u8>> = match &req.baseline {
            DiffBaseline::Vcs => context.base_contents(&req.root, &req.path),
            DiffBaseline::FollowSnapshot(bytes) => Some(bytes.clone()),
            DiffBaseline::FollowHead { sha } => Some(
                konoma_vcs_core::vcs::contents_at(&req.root, sha, &req.path).unwrap_or_default(),
            ),
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
            MdDiffKind::Gutter => match konoma_widgets::text::load(&req.path) {
                Ok(content) => decorated_file_text(&content),
                Err(_) => return MdDiffOutcome::Unavailable,
            },
            MdDiffKind::Rendered => {
                // Check the size before reading: never slurp a device / FIFO / huge file whole.
                match std::fs::metadata(&req.path) {
                    Ok(m) if m.is_file() && m.len() <= FOLLOW_BASELINE_FILE_CAP as u64 => {}
                    _ => return MdDiffOutcome::Unavailable,
                }
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
        let (_, _, ops) = konoma_widgets::markdown::diff_align(&old_pre, &new_pre);
        let any_change = konoma_widgets::markdown::ops_has_any_change(&ops);
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

    /// A controlled computation seam; production uses `compute_md_diff`.
    pub fn with_compute(compute: Arc<MdCompute>) -> Self {
        Self {
            compute: Some(compute),
            ..Self::default()
        }
    }

    pub fn attach(&mut self, tx: Sender<MdDiffResult>) {
        self.tx = Some(tx);
    }

    pub fn landed_for(&self, path: &Path, kind: MdDiffKind) -> Option<MdDiffOutcome> {
        self.landed
            .as_ref()
            .filter(|(p, k, gen, _)| p == path && *k == kind && *gen == self.gen)
            .map(|(_, _, _, o)| o.clone())
    }

    pub fn needs(&self, path: &Path, kind: MdDiffKind) -> bool {
        !self
            .landed
            .as_ref()
            .is_some_and(|(p, k, gen, _)| p == path && *k == kind && *gen == self.gen)
            && !self
                .pending
                .as_ref()
                .is_some_and(|(p, k)| p == path && *k == kind)
    }

    pub fn pending_for(&self, path: &Path) -> bool {
        self.pending.as_ref().is_some_and(|(p, _)| p == path)
    }

    pub fn kick(&mut self, mut req: MdDiffRequest) -> Option<MdDiffResult> {
        self.gen = self.gen.wrapping_add(1);
        req.gen = self.gen;
        self.pending = Some((req.path.clone(), req.kind));
        self.spawn_or_sync(req)
    }

    fn spawn_or_sync(&self, req: MdDiffRequest) -> Option<MdDiffResult> {
        let compute = self.compute.clone();
        let context = self.read_context;
        let run = move || {
            let outcome = match compute {
                Some(compute) => compute(&req),
                None => Self::compute_with_context(&req, context),
            };
            MdDiffResult {
                gen: req.gen,
                path: req.path,
                kind: req.kind,
                outcome,
            }
        };
        let Some(tx) = self.tx.clone() else {
            return Some(run());
        };
        let config = konoma_vcs_core::vcs::WorkerConfig::capture();
        // Preserve the synchronous fallback's direct call and the spawned worker's panic net.
        let fallback = MdDiffResult {
            gen: self.gen,
            path: self.pending.as_ref().unwrap().0.clone(),
            kind: self.pending.as_ref().unwrap().1,
            outcome: MdDiffOutcome::Unavailable,
        };
        std::thread::spawn(move || {
            config.install();
            let result = konoma_widgets::markdown::compute_or_fallback(run, || fallback);
            let _ = tx.send(result);
        });
        None
    }

    pub fn apply(&mut self, res: MdDiffResult) -> bool {
        if res.gen != self.gen {
            return false;
        }
        self.pending = None;
        self.landed = Some((res.path, res.kind, res.gen, res.outcome));
        true
    }

    pub fn invalidate(&mut self) {
        self.gen = self.gen.wrapping_add(1);
        self.landed = None;
        self.pending = None;
    }

    #[cfg(any(test, feature = "test-hooks"))]
    pub fn generation_for_test(&mut self) -> &mut u64 {
        &mut self.gen
    }
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn pending_for_test(&mut self) -> &mut Option<(PathBuf, MdDiffKind)> {
        &mut self.pending
    }
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn landed_for_test(&mut self) -> &mut Option<(PathBuf, MdDiffKind, u64, MdDiffOutcome)> {
        &mut self.landed
    }
}

pub fn select_diff_baseline<'a>(
    kind: MdDiffKind,
    follow_scope: bool,
    full: bool,
    valid: bool,
    baseline: impl FnOnce() -> Option<FollowBaseline<'a>>,
) -> DiffBaseline {
    if kind == MdDiffKind::Gutter || !follow_scope || full || !valid {
        return DiffBaseline::Vcs;
    }
    let Some(baseline) = baseline() else {
        return DiffBaseline::Empty;
    };
    match baseline.dirty {
        Some(Some(bytes)) => DiffBaseline::FollowSnapshot(bytes.to_vec()),
        Some(None) => DiffBaseline::Empty,
        None => match baseline.head {
            Some(sha) => DiffBaseline::FollowHead { sha: sha.into() },
            None => DiffBaseline::Empty,
        },
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
    let (lines, truncated) = konoma_widgets::text::cap_lines(s.as_bytes());
    if !truncated {
        return s;
    }
    decorated_file_text(&konoma_widgets::text::TextContent { lines, truncated })
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
        konoma_widgets::markdown::strip_front_matter(src).1
    } else {
        src.to_string()
    };
    let origin = konoma_widgets::markdown::identity_origin(&src);
    let (src, origin) = if footnotes {
        konoma_widgets::markdown::process_footnotes_traced(&src, &origin)
    } else {
        (src, origin)
    };
    let (src, _origin) = if inline_html {
        konoma_widgets::markdown::process_inline_html_traced(&src, &origin)
    } else {
        (src, origin)
    };
    src
}

/// The exact capped-file text used by both the ordinary preview and its diff.
pub fn decorated_file_text(content: &konoma_widgets::text::TextContent) -> String {
    let mut text = content.lines.join("\n");
    if content.truncated {
        text.push_str(konoma_widgets::text::TRUNCATED_NOTE);
    }
    text
}

#[cfg(test)]
mod tests {
    //! `MdDiffWorker::compute_md_diff` in isolation — no App/tree/render machinery, just the pure
    //! computation. `DiffBaseline::FollowSnapshot`/`Empty` need no git repository at all (only the
    //! `Vcs` variant does), so these run on both feature builds.

    use super::*;
    use konoma_test_support::unique_tmp;

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
        match MdDiffWorker::compute_md_diff(&r) {
            MdDiffOutcome::Ready {
                any_change,
                front_matter_only,
                ops,
                ..
            } => {
                assert!(any_change, "本文が変わっているので any_change のはず");
                assert!(!front_matter_only, "front matter だけの変更ではない");
                assert!(
                    konoma_widgets::markdown::ops_has_any_change(&ops),
                    "any_change と ops の実際の中身が一致するはず"
                );
            }
            other => panic!("Ready のはず: {other:?}"),
        }
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
        match MdDiffWorker::compute_md_diff(&r) {
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
        match MdDiffWorker::compute_md_diff(&r) {
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
    }

    #[test]
    fn gutter_kind_with_no_baseline_is_no_baseline_not_ready() {
        let dir = unique_tmp("konoma_compute_md_diff_gutter_no_baseline");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("doc.md");
        std::fs::write(&path, "# Title\n").unwrap();
        let r = req(path, MdDiffKind::Gutter, DiffBaseline::Empty);
        assert!(
            matches!(MdDiffWorker::compute_md_diff(&r), MdDiffOutcome::NoBaseline),
            "Gutter はベースライン無しでは NoBaseline のはず(全追加にはしない)"
        );
    }

    #[test]
    fn rendered_kind_with_no_baseline_is_ready_all_added() {
        let dir = unique_tmp("konoma_compute_md_diff_rendered_no_baseline");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("doc.md");
        std::fs::write(&path, "# Title\n\nAll new.\n").unwrap();
        let r = req(path, MdDiffKind::Rendered, DiffBaseline::Empty);
        match MdDiffWorker::compute_md_diff(&r) {
            MdDiffOutcome::Ready {
                any_change, ops, ..
            } => {
                assert!(any_change, "旧版が空なので全ブロック Insert のはず");
                assert!(
                    ops.iter()
                        .all(|op| matches!(op, konoma_widgets::markdown::BlockOp::Insert { .. })),
                    "§5『旧版が無い→全ブロック Insert』: {ops:?}"
                );
            }
            other => panic!("Ready のはず: {other:?}"),
        }
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
            matches!(
                MdDiffWorker::compute_md_diff(&r),
                MdDiffOutcome::Unavailable
            ),
            "上限超の新版は Unavailable のはず"
        );
    }

    /// M11 (pre-merge review of PR #21): the *baseline's own* `FOLLOW_BASELINE_FILE_CAP` check
    /// (`b.len() > FOLLOW_BASELINE_FILE_CAP`, right before the `String::from_utf8` in
    /// `MdDiffWorker::compute_md_diff`) is gated `req.kind == MdDiffKind::Rendered` only — `Gutter`'s old
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
                MdDiffWorker::compute_md_diff(&r_rendered),
                MdDiffOutcome::Unavailable
            ),
            "Rendered は旧版が上限超なら Unavailable のはず"
        );

        let r_gutter = req(
            path,
            MdDiffKind::Gutter,
            DiffBaseline::FollowSnapshot(big_old.into_bytes()),
        );
        match MdDiffWorker::compute_md_diff(&r_gutter) {
            MdDiffOutcome::Ready { .. } => {} // Gutter has no file-size gate on the old side.
            other => panic!("Gutter は旧版の上限を見ないので Ready のはず: {other:?}"),
        }
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
            matches!(
                MdDiffWorker::compute_md_diff(&r),
                MdDiffOutcome::Unavailable
            ),
            "新版が読めない場合は Unavailable のはず"
        );
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
        let new_pre = match MdDiffWorker::compute_md_diff(&r) {
            MdDiffOutcome::Ready { new_pre, .. } => new_pre,
            other => panic!("Ready のはず: {other:?}"),
        };
        let (capped_lines, truncated) = konoma_widgets::text::cap_lines(big.as_bytes());
        assert!(truncated, "6000 行は MAX_LINES を超えるはず(テストの前提)");
        let expected = decorated_file_text(&konoma_widgets::text::TextContent {
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
        let old_pre = match MdDiffWorker::compute_md_diff(&r) {
            MdDiffOutcome::Ready { old_pre, .. } => old_pre,
            other => panic!("Ready のはず: {other:?}"),
        };
        let (capped_lines, truncated) = konoma_widgets::text::cap_lines(big.as_bytes());
        let expected = decorated_file_text(&konoma_widgets::text::TextContent {
            lines: capped_lines,
            truncated,
        });
        assert_eq!(
            old_pre, expected,
            "Rendered の old_pre(baseline) も同じ上限で切り詰められるはず"
        );
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
        let new_pre = match MdDiffWorker::compute_md_diff(&r) {
            MdDiffOutcome::Ready { new_pre, .. } => new_pre,
            other => panic!("Ready のはず: {other:?}"),
        };
        assert_eq!(
            new_pre, body,
            "上限未満では切り詰めもサフィックスも入らないはず"
        );
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
        match MdDiffWorker::compute_md_diff(&r) {
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
    }

    #[cfg(feature = "pictures")]
    #[test]
    fn ordinary_preview_and_diff_use_the_same_truncation_note() {
        let dir = unique_tmp("konoma_shared_truncation_note");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("doc.md");
        let source = "body\n".repeat(5001);
        std::fs::write(&path, &source).unwrap();
        let content = konoma_widgets::text::load(&path).unwrap();
        assert!(content.truncated);
        let ordinary = decorated_file_text(&content);
        // The standalone ordinary preview appends document_render's exported note.
        assert!(ordinary.ends_with(crate::document_render::TRUNCATED_NOTE));
        assert!(ordinary.ends_with(konoma_widgets::text::TRUNCATED_NOTE));
        for kind in [MdDiffKind::Gutter, MdDiffKind::Rendered] {
            let mut request = req(
                path.clone(),
                kind,
                DiffBaseline::FollowSnapshot(source.as_bytes().to_vec()),
            );
            // Compare capped text before Markdown normalization adds its trailing newline.
            request.md_frontmatter = false;
            request.md_footnotes = false;
            request.md_inline_html = false;
            let MdDiffOutcome::Ready {
                old_pre, new_pre, ..
            } = MdDiffWorker::compute_md_diff(&request)
            else {
                panic!("capped Markdown must remain renderable");
            };
            assert_eq!(ordinary, new_pre, "{kind:?}");
            assert_eq!(ordinary, old_pre, "{kind:?} baseline");
        }
    }
}
