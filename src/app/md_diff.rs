//! Host value capture and UI effects for the shared Markdown diff worker.
use super::*;
use konoma_plugin::host::project::md_diff as worker;

impl App {
    pub fn attach_md_diff_loader(&mut self, tx: std::sync::mpsc::Sender<MdDiffResult>) {
        self.md_diff.attach(tx);
    }

    #[cfg_attr(not(feature = "git"), allow(unused_variables))]
    pub(super) fn diff_baseline(&self, path: &Path, kind: MdDiffKind) -> DiffBaseline {
        #[cfg(feature = "git")]
        {
            worker::select_diff_baseline(
                kind,
                self.diff_follow_scope,
                self.follow_diff_full,
                self.follow_scope_valid(),
                || {
                    self.follow_baseline.as_ref().map(|base| {
                        let key = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
                        worker::FollowBaseline {
                            dirty: base.dirty.get(&key).map(|bytes| bytes.as_deref()),
                            head: base.head.as_deref(),
                        }
                    })
                },
            )
        }
        #[cfg(not(feature = "git"))]
        worker::select_diff_baseline(kind, false, false, false, || None)
    }

    #[cfg(test)]
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub(crate) fn diff_baseline_for_test(&self, path: &Path, kind: MdDiffKind) -> DiffBaseline {
        self.diff_baseline(path, kind)
    }

    fn md_diff_landed_for(&self, path: &Path, kind: MdDiffKind) -> Option<MdDiffOutcome> {
        self.md_diff.landed_for(path, kind)
    }

    pub(super) fn poll_md_diff(&mut self, path: &Path, kind: MdDiffKind) -> Option<MdDiffOutcome> {
        if self.md_diff.needs(path, kind) {
            self.kick_md_diff(path.to_path_buf(), kind);
        }
        self.md_diff_landed_for(path, kind)
    }

    fn kick_md_diff(&mut self, path: PathBuf, kind: MdDiffKind) {
        let baseline = self.diff_baseline(&path, kind);
        let opts = self.doc_options();
        self.spawn_or_sync_md_diff(MdDiffRequest {
            gen: 0,
            path,
            root: self.tab.root.clone(),
            kind,
            baseline,
            md_frontmatter: opts.front_matter,
            md_footnotes: opts.footnotes,
            md_inline_html: opts.inline_html,
        });
    }

    fn spawn_or_sync_md_diff(&mut self, req: MdDiffRequest) {
        self.md_diff =
            std::mem::take(&mut self.md_diff).with_read_context(self.tab.diff_read_context);
        if let Some(result) = self.md_diff.kick(req) {
            self.apply_md_diff(result);
        }
    }

    pub fn apply_md_diff(&mut self, res: MdDiffResult) -> bool {
        let path = res.path.clone();
        let kind = res.kind;
        let unavailable = matches!(&res.outcome, MdDiffOutcome::Unavailable);
        let front_matter_only = matches!(
            &res.outcome,
            MdDiffOutcome::Ready {
                front_matter_only: true,
                ..
            }
        );
        if !self.md_diff.apply(res) {
            return false;
        }
        let on_screen = self.tab.preview_path.as_deref() == Some(path.as_path());
        if kind == MdDiffKind::Rendered && on_screen && self.tab.diff_view == DiffView::Rendered {
            match (unavailable, front_matter_only) {
                (true, _) => {
                    self.tab.diff_view = DiffView::Source;
                    // `Rendered` is no longer reachable for this path — the "scroll to first change"
                    // reservation `App::open_git_diff_with`/`App::cycle_diff_view` armed for it (if
                    // any) will never be consumed by `render_decorated_body` now that `Source` draws
                    // through a different renderer entirely. Drop it explicitly rather than leave it
                    // dangling on `res.path` until some later, unrelated event clears it.
                    if self.tab.diff_scroll_pending.as_deref() == Some(path.as_path()) {
                        self.tab.diff_scroll_pending = None;
                    }
                    self.flash =
                        Some(tr(self.lang, crate::i18n::Msg::DiffRenderedUnavailable).into());
                }
                (_, true) => {
                    self.flash =
                        Some(tr(self.lang, crate::i18n::Msg::DiffRenderedFrontMatterOnly).into());
                }
                _ => {}
            }
        }
        // The next `ensure_md_cache` rebuild picks up the now-ready result (the block-diff itself
        // is already computed; only `Doc::parse` — cheap — remains synchronous).
        if on_screen {
            self.md_cache = None;
        }
        true
    }

    pub(crate) fn md_diff_pending_for_current(&self) -> bool {
        self.tab
            .preview_path
            .as_deref()
            .is_some_and(|path| self.md_diff.pending_for(path))
    }

    pub(super) fn invalidate_md_diff(&mut self) {
        self.md_diff.invalidate();
    }
}
