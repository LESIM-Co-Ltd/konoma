//! The file diff (open, layout, discard) and the background git writes — methods on `App`.

use super::*;

#[cfg(all(test, feature = "git"))]
#[path = "diff_source_test_adapter.rs"]
mod diff_source_test_adapter;

/// How a diff is being opened — the input `App::open_git_diff_with` needs *before* it decides
/// `diff_follow_scope`/the presentation and validates that presentation
/// exactly once. `Default` is the ordinary "fresh open, from a tree/project selection" shape that plain
/// `App::open_git_diff` uses; the other three call sites (`follow_jump`, `n`/`N`, the `Preview`
/// representation's own `R` back to `Source`) each override just the fields they need.
#[derive(Default)]
pub(super) struct DiffOpen {
    pub read_context: konoma_plugin::host::project::DiffReadContext,
    /// `true` for a follow-originated diff — `n`/`N` then cycles only the follow session's own
    /// changed-file list, and the `Rendered` presentation's baseline is the follow-session snapshot
    /// rather than the backend's committed blob (`App::diff_baseline`, `md_diff.rs`). Defaults to
    /// `false` (the full uncommitted change set, the backend's committed blob).
    pub follow_scope: bool,
    /// The presentation to open into — recorded as the tab's new `diff_view_choice` and then
    /// rounded to what the *target* path can actually show (`App::round_diff_view`) either way.
    /// `None` (the default) resolves fresh from `[ui] diff_view` — a brand new diff surface.
    /// `Some(v)` carries an already-chosen presentation over instead, so moving to another file
    /// (`n`/`N`) or returning from the `Preview` representation doesn't reset it.
    pub view: Option<DiffView>,
    /// A diff already fetched by the caller (`follow_jump`'s own `compute_gitdiff_lines`), seeded
    /// into `diff_cache` so the first render of the `Source` presentation doesn't have to re-invoke
    /// git for something already in hand. `None` (the default) leaves the cache empty
    /// (`invalidate_diff_caches` already clears it) for the render to populate lazily.
    pub seeded_diff: Option<DiffCache>,
    /// The same open going on (`R` back from the Preview representation): the preview load
    /// identity (`PerTab::load_id`) is kept. Every other call is a new open and renews it.
    pub same_open: bool,
}

impl App {
    // --- Background git writes (design principle #4) ---------------------------------
    // Every mutation below (stage/unstage/discard/commit/checkout/branch/worktree) shells out to
    // `git`, whose runtime is unbounded: a `pre-commit` hook, a network mount, or another process
    // holding `.git/index.lock` can each stall it indefinitely. Running that on the UI thread froze
    // konoma outright, while the read side (`git status`, `ignored`) had long since been moved off
    // it. These follow the shape `start_file_op`/`apply_file_op` established for filesystem
    // operations: dispatch → worker thread → result applied on the run loop's thread.

    /// Attach the Sender of the worker that runs background git writes (called by main at startup).
    pub fn attach_gitop_runner(&mut self, tx: std::sync::mpsc::Sender<crate::app::GitOpResult>) {
        self.gitop_tx = Some(tx);
    }

    /// Whether a git write is running in the background right now.
    pub fn git_op_running(&self) -> bool {
        self.gitop_pending.is_some()
    }

    /// Start a git write in the background, or run it synchronously when no runner is attached
    /// (unit tests / no run loop) so the result is observable immediately — the same contract as
    /// `start_file_op`/`spawn_or_sync_statuses`, and the reason every existing test of these flows
    /// keeps passing unchanged. Returns false and flashes when another write is already in flight.
    pub(super) fn start_git_op(&mut self, mut job: crate::app::GitOpJob) -> bool {
        if !self.allow_write("git", true, true) {
            return false;
        }
        if self.gitop_pending.is_some() {
            // Only one at a time: two concurrent writes would contend on `.git/index.lock` and the
            // loser would just fail. Refuse cleanly instead, and leave every bit of state (the
            // typed commit message, the selection) untouched.
            self.flash = Some(crate::i18n::tr(self.lang, crate::i18n::Msg::GitOpBusy).into());
            return false;
        }
        // Bake in "which tab this was dispatched from" here at dispatch time (callers pass it
        // empty). The active tab when it completes may be a different one — `apply_git_op` uses
        // this to decide whether it may touch tab state.
        job.root = self.tab.root.clone();
        job.tab_id = self.tab.id;
        self.gitop_gen = self.gitop_gen.wrapping_add(1);
        let gen = self.gitop_gen;
        self.gitop_pending = Some(job.kind);
        let Some(tx) = self.gitop_tx.clone() else {
            let res = Self::run_git_op(gen, job);
            self.apply_git_op(res);
            return true;
        };
        // Pre-translate the panic fallback message here (the worker has no `&self` to call `tr()`).
        let panic_err = crate::i18n::tr(self.lang, crate::i18n::Msg::OperationFailed).to_string();
        let (kind, root, tab_id, label) =
            (job.kind, job.root.clone(), job.tab_id, job.label.clone());
        std::thread::spawn(move || {
            // If the worker panics, no result comes back and `gitop_pending` latches forever —
            // the spinner never stops and every later git key is refused as "busy". Same safety
            // net as the other workers: always produce a result (design principle #3).
            let res = crate::preview::markdown::compute_or_fallback(
                || Self::run_git_op(gen, job),
                || crate::app::GitOpResult {
                    gen,
                    kind,
                    root,
                    tab_id,
                    label,
                    err: Some(panic_err),
                },
            );
            let _ = tx.send(res);
        });
        true
    }

    /// The actual git call (pure: no `&self`), shared by the worker thread and the synchronous
    /// fallback. The error is flattened to a `String` here rather than in `apply_git_op` because
    /// `anyhow::Error` is not what crosses the channel; the text is identical to what the old
    /// synchronous code showed, since `describe_error` only rewrites `fileops::FileOpError`
    /// variants and a git error is never one of those (it falls through to `Display`).
    fn run_git_op(gen: u64, job: crate::app::GitOpJob) -> crate::app::GitOpResult {
        use crate::app::GitOpKind as K;
        let crate::app::GitOpJob {
            kind,
            root,
            tab_id,
            path,
            label,
        } = job;
        let outcome = match kind {
            K::Discard => crate::vcs::Git::discard(&root, &path),
        };
        crate::app::GitOpResult {
            gen,
            kind,
            root,
            tab_id,
            label,
            err: outcome.err().map(|e| e.to_string()),
        }
    }

    /// Apply a finished git write: update the views it affects and flash the same summary the
    /// synchronous implementation used to produce.
    ///
    /// The generation check is belt-and-braces only (`gitop_gen` advances solely in `start_git_op`,
    /// which refuses to start while one is pending, so it is frozen for the whole flight). The guard
    /// that matters is **which tab this is landing on**: the user can switch tabs, or open a new one,
    /// while a commit hook runs — that is the entire point of moving the write off the UI thread —
    /// so the tab active when the result lands may not be the one that dispatched it. Two separate
    /// questions get two separate guards, because they have different answers:
    ///
    /// - `shows_same_repo` (root equality) gates **repo-scoped refreshes** — rebuilding the change
    ///   list, the branch list, the tree. Any tab rooted at that repository is displaying state the
    ///   write just invalidated, so refreshing it is right regardless of who asked.
    /// - `is_origin_tab` (tab identity) gates **navigation and focus** — returning from the diff
    ///   preview, closing the branch list, moving the tab into a new worktree, restoring a dialog.
    ///   Only the tab that asked may be moved or typed into. Root equality is *not* good enough
    ///   here: `t` opens the new tab at the current root, so a `worktree add` completing after the
    ///   user pressed `t` would otherwise yank the new tab into the worktree and leave the tab that
    ///   asked for it behind.
    ///
    /// The flash and the status invalidation are gated by neither — the repository really did change.
    pub fn apply_git_op(&mut self, res: crate::app::GitOpResult) -> bool {
        use crate::app::GitOpKind as K;
        use crate::i18n::{tr, Msg};
        if res.gen != self.gitop_gen {
            return false;
        }
        self.gitop_pending = None;
        let lang = self.lang;
        let shows_same_repo = res.root == self.tab.root;
        let is_origin_tab = res.tab_id == self.tab.id;
        let failed = res.err.clone();
        if failed.is_none() {
            // The write changed the repository, so the cached status is stale no matter which tab
            // is on screen. `git_view_reload` below sets the same two flags; setting them here as
            // well is what keeps a completion that lands on a *different* tab from silently
            // skipping the re-validation.
            self.git_status_for = None;
            self.git_status_dirty = true;
        }
        // Shared "Failed: <git's message>" shape (Msg::Failed + git's own `fatal:` line). Identical
        // to what the synchronous arms produced: they ran the message through `describe_error`,
        // which only rewrites `fileops::FileOpError` variants and falls through to `Display` for
        // anything else — and a git error is never one of those.
        let failed_flash = |e: &str| format!("{}: {e}", tr(lang, Msg::Failed));
        match res.kind {
            K::Discard => match &failed {
                Some(e) => self.flash = Some(failed_flash(e)),
                None => {
                    let mut ok = true;
                    // Navigation: only the tab that asked gets pulled out of the diff preview.
                    // A diff opened from a project screen returns to it; one opened from the tree
                    // goes to the project's changes screen, as `p` from the tree opens it.
                    if is_origin_tab
                        && self.is_git_diff_preview()
                        && !self.return_to_project_after_discard()
                    {
                        self.back_to_tree();
                        self.enter_project();
                    }
                    if shows_same_repo {
                        // The discard itself succeeded. If the tree rebuild fails, report that
                        // instead of covering it with a success flash.
                        ok = self.rebuild_tree_notify();
                    }
                    if ok {
                        self.flash = Some(format!("{}: {}", tr(lang, Msg::Discarded), res.label));
                    }
                }
            },
        }
        true
    }

    // --- The file diff -------------------------------------------------
    /// Open `path`'s diff in the GitDiff preview. A fresh, tree-originated open —
    /// `open_git_diff_with`'s own doc comment covers the other three call shapes (follow-originated, `n`/`N`, returning from the `Preview` representation).
    pub fn open_git_diff(&mut self, path: &Path) {
        self.open_git_diff_with(path, DiffOpen::default());
    }

    /// `open_git_diff`, generalized over *how* the diff is being opened — the single place that
    /// decides `diff_follow_scope` and the presentation (`apply_diff_view`)
    /// from already-final inputs and calls `apply_diff_view` exactly once, rather than a caller
    /// opening with the wrong scope and immediately re-deciding once it knows better.
    ///
    /// Before this existed, `follow_jump` and `diff_jump_changed` (`n`/`N`) each called
    /// `open_git_diff` — which always resets `diff_follow_scope` to `false` and validates the
    /// `Rendered` presentation against *that* scope's baseline — and only afterward set the scope
    /// they actually wanted and re-ran `apply_diff_view` to re-validate. For a follow-originated
    /// Markdown diff whose HEAD-committed version exceeded the size cap but whose follow-session
    /// snapshot did not, the first call would flash `DiffRenderedUnavailable` and round down to
    /// `Source`; the second call would then successfully switch to `Rendered` — but never cleared
    /// the stale flash, which sat in the footer (looking exactly like the "am I broken?" case that
    /// flash exists to head off) despite `Rendered` already being on screen. `return_to_diff_from_
    /// preview` had the same save/restore shape for `diff_follow_scope` (though
    /// not the double-validation bug, since it never re-entered `Rendered`). All three now build a
    /// `DiffOpen` up front and call this once.
    pub(super) fn open_git_diff_with(&mut self, path: &Path, open: DiffOpen) {
        self.tab.diff_read_context = open.read_context;
        if !open.same_open {
            self.begin_preview_load();
        }
        self.tab.preview_path = Some(path.to_path_buf());
        self.set_preview_kind(Some(PreviewKind::GitDiff(path.to_path_buf())));
        // The diff preview draws itself (it doesn't use window/image/md). Reset the related state.
        self.tab.preview_scroll = 0;
        self.tab.preview_hscroll = 0;
        self.tab.preview_byte_top = 0;
        self.tab.preview_top_line = 0;
        self.tab.preview_top_skip = 0;
        // A fresh open: drop any leftover "scroll to first change" reservation before deciding, at
        // the bottom of this fn, whether to arm a new one for `path` — mirrors `App::enter_preview`'s
        // own up-front clear (`diff_scroll_pending`'s own doc comment).
        self.tab.diff_scroll_pending = None;
        // The first answer of the target resolution (worker on) makes the reservation below.
        self.tab.diff_answer_awaited = self.preview_resolution_tx.is_some();
        // A fresh diff target: the media diff's page position belongs to *this* file, not whatever
        // page a previously-open PDF diff happened to be on.
        self.tab.diff_media_page = 1;
        self.preview_win = None;
        self.win_cache = None;
        self.preview_total_lines = None;
        self.md_cache = None;
        self.invalidate_diff_caches(); // opening another file's diff: invalidate both diff caches
        if let Some(seeded) = open.seeded_diff {
            // A diff the caller already fetched (`follow_jump`'s own `compute_gitdiff_lines`) —
            // seed it so the very first render (and `apply_diff_view`'s own `git_diff_lines` read,
            // below) doesn't re-invoke git for something already in hand.
            self.diff_cache = Some(seeded);
        }
        self.md_items.clear();
        self.tab.focused_item = None;
        self.hl_pending = false;
        self.hl_warming = false;
        self.diff_follow_scope = open.follow_scope;
        // The presentation: a fresh open (`open.view == None`) wants `[ui] diff_view` unrounded; a
        // carried-over one (`n`/`N`, the `Preview` return) wants whatever the caller already chose
        // (`PerTab::diff_view_choice`, not the target it's carried *from*'s rounded `diff_view` —
        // see that field's own doc comment for why the two must stay separate). Either way, that
        // wanted presentation *is* the choice — recorded here, before rounding, so a file that can't
        // show it never overwrites what the user actually asked for. It's then rounded to what
        // `path` can actually show (`round_diff_view`) instead of trusting the caller already did it
        // against the *new* target. `apply_diff_view` (not a direct assignment) is used, not because
        // it validates anything itself — for `Rendered` it only **kicks** the block-diff computation
        // (`App::poll_md_diff`) so it's already in flight by the time the first frame draws — but so
        // every `Rendered` open goes through the one place that does that kick. The actual
        // rounding-down to `Source` (if the result lands `Unavailable`) or the "nothing to mark but
        // front matter" flash happens later, once the worker's result actually lands
        // (`App::apply_md_diff`, run from the event loop, never from here or the render path) — by
        // which point every input `apply_diff_view` reads here (`diff_follow_scope` above, the seeded
        // cache above) is already stale history, not something this fn needs to wait on. That async
        // fallback only ever rewrites the *effective* `diff_view`, never `diff_view_choice`, so it
        // can't leak into whatever file is opened next either.
        let wanted = open
            .view
            .unwrap_or_else(|| DiffView::parse(&self.cfg.ui.diff_view));
        self.tab.diff_view_choice = wanted;
        let view = self.round_diff_view(wanted, path);
        self.apply_diff_view(view, path);
        self.tab.diff_scroll_pending =
            (self.tab.diff_view == DiffView::Rendered).then(|| path.to_path_buf());
        // Leaving whatever *other* preview this tab may have been showing (including the diff's own
        // `Preview` representation, if `R`/`n`/`N` reached here from it) — this is a fresh open of
        // the diff surface, not a continuation of that preview.
        self.tab.preview_from_diff = false;
        self.tab.mode = Mode::Preview;
        // A resolved `Preview` presentation means "leave `Surface::PreviewGitDiff` altogether for
        // `path`'s own ordinary preview" (`docs/FEATURE-MD-RENDERED-DIFF.md` §1) — the same
        // transition `R` (`App::cycle_diff_view`) makes when it cycles into `Preview` from an
        // already-open diff. Reused here (`App::enter_diff_preview_representation`) so it also
        // fires for the other two ways a diff open can resolve to `Preview`: a fresh
        // `[ui] diff_view = "preview"` open (previously left `tab.diff_view == Preview` stranded
        // on the `Source` draw path above, since nothing else ever recognized it), and
        // `App::follow_jump` carrying an already-chosen `Preview` presentation into the next
        // changed file.
        #[cfg(feature = "git")]
        if view == DiffView::Preview {
            self.enter_diff_preview_representation(path);
        }
    }

    /// Whether a GitDiff preview is currently showing (for render/key branching).
    pub fn is_git_diff_preview(&self) -> bool {
        matches!(self.tab.preview_kind, Some(PreviewKind::GitDiff(_)))
    }

    /// Returns the diff lines of the GitDiff preview (for rendering). Empty if not applicable.
    /// Cached per path to avoid calling `file_diff` (a git invocation) every frame.
    /// When the working tree changes, `refresh()` drops the cache, so j/k and horizontal scroll while
    /// displayed don't re-run git; only external edits (FS events) refetch. The return value is cloned every frame for rendering.
    pub fn git_diff_lines(&mut self) -> Vec<crate::git::DiffLine> {
        let Some(PreviewKind::GitDiff(p)) = self.tab.preview_kind.clone() else {
            return Vec::new();
        };
        #[cfg(feature = "git")]
        if self.diff_follow_scope && !self.follow_diff_full && self.jj_follow_workers() {
            self.request_follow_path(&p);
        }
        let hit = matches!(&self.diff_cache, Some(c) if c.path == p);
        if !hit {
            // If follow-originated (diff_follow_scope), the since-start baseline diff; otherwise the full diff.
            let lines = self.compute_gitdiff_lines(&p, self.diff_follow_scope);
            self.diff_cache = Some(DiffCache {
                path: p.clone(),
                lines,
            });
        }
        self.diff_cache
            .as_ref()
            .map(|c| c.lines.clone())
            .unwrap_or_default()
    }

    /// If the file at the tree cursor has git changes, open its diff **directly** (without going through `o`).
    /// For a directory / no changes / outside a repo, only flashes. Closing (q/Esc) returns to the tree.
    pub fn tree_open_git_diff(&mut self) {
        // `d` judges "has changes / no changes" from status = if a scan is in flight, wait. Reading
        // while it's still async would let a mere press during the scan be rejected as "no changes", looking like a broken feature.
        self.ensure_git_status_now();
        let Some(e) = self.tab.entries.get(self.tab.selected) else {
            return;
        };
        if e.is_dir {
            self.flash = Some(crate::i18n::tr(self.lang, crate::i18n::Msg::NotAFile).into());
            return;
        }
        let path = e.path.clone();
        if self.git_status_of(&path).is_none() {
            self.flash = Some(crate::i18n::tr(self.lang, crate::i18n::Msg::NoChanges).into());
            return;
        }
        self.open_git_diff(&path); // tree-originated → `q` returns to the tree
    }

    /// Whether to lay the diff side by side at display width `width` (resolves Auto).
    pub fn diff_is_split(&self, width: u16) -> bool {
        self.diff_layout.is_split(width)
    }

    pub(crate) fn source_diff_layout(&self) -> konoma_widgets::diff_frame::SourceDiffLayout {
        konoma_plugin::host::project::diff_frame::source_diff_layout(self.diff_layout)
    }
    /// Cycle the diff layout unified→split→Auto (`s`). Called from the GitDiff preview. While the media diff's side-by-side view is actually **showing pictures**
    /// (`media_diff_showing_pictures` — not merely targeted; a still-computing or binary-summary body
    /// has nothing to lay out), delegates to `cycle_media_diff_layout` instead — `s` there cycles
    /// auto/side/stack, a wholly different (and separately configured) layout state than this one.
    /// Otherwise a no-op while the `Rendered` presentation is on screen at all (`diff_rendered_
    /// active`) — for Markdown that's decorated blocks, not the unified/split raw-line layout this
    /// key cycles, so there's nothing here to change (the same fallthrough also makes a
    /// still-computing/summary media diff a no-op, since its `diff_media_active` is already `true`).
    /// The footer already hides the `s:...` hint in exactly that case
    /// ([[hint-shown-iff-key-acts]]) — this is the key-handler half of the same gate.
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn cycle_diff_layout(&mut self) {
        if self.media_diff_showing_pictures() {
            self.cycle_media_diff_layout();
            return;
        }
        if self.diff_rendered_active() {
            return;
        }
        self.diff_layout = self.diff_layout.next();
        self.tab.preview_hscroll = 0;
        let label = match self.diff_layout {
            DiffLayout::Unified => crate::i18n::tr(self.lang, crate::i18n::Msg::DiffUnified),
            DiffLayout::Split => crate::i18n::tr(self.lang, crate::i18n::Msg::DiffSideBySide),
            DiffLayout::Auto => crate::i18n::tr(self.lang, crate::i18n::Msg::DiffAuto),
        };
        self.flash = Some(label.into());
    }

    /// Close the GitDiff preview (q/Esc): returns to the tree. Also the return path from the diff's `Preview` representation
    /// (`main.rs`'s `Action::PreviewBack` routes here whenever `tab.preview_from_diff` is set, not
    /// only for an actual `Surface::PreviewGitDiff`) — `preview_from_diff` is cleared unconditionally
    /// here so either origin ends up in the identical "no longer inside a diff" state.
    pub fn close_git_diff(&mut self) {
        self.tab.preview_from_diff = false;
        self.back_to_tree();
    }

    /// `x` in the GitDiff preview = discard all changes to the displayed file. Opens a confirmation dialog
    /// (on confirm: git::discard then open the project's changes screen).
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn git_diff_start_discard(&mut self) {
        if !self.allow_write("git", true, true) {
            return;
        }
        let Some(PreviewKind::GitDiff(path)) = self.tab.preview_kind.clone() else {
            return;
        };
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("?")
            .to_string();
        let message = format!(
            "{} {name} ?",
            crate::i18n::tr(self.lang, crate::i18n::Msg::DiscardChangesTo)
        );
        self.dialog = Some(Dialog {
            op: PendingOp::GitDiscard { path },
            kind: DialogKind::Confirm {
                message,
                allow_permanent: false,
            },
        });
    }
    /// The existing worktree root switch, also used by a project's SwitchRoot outcome.
    pub(super) fn switch_worktree_root(&mut self, path: PathBuf) {
        self.jump_to_dir(path.clone());
        self.tab.open_dir = path;
        self.flash = Some(format!(
            "{}: {}",
            crate::i18n::tr(self.lang, crate::i18n::Msg::SwitchedTo),
            home_relative(&self.tab.root)
        ));
    }
}

#[cfg(test)]
mod tests {
    // Every test below needs a real repo (git2), so the imports are only actually used under the
    // `git` feature — gate them too, rather than leaving them to warn as unused in a no-git build.
    #[cfg(feature = "git")]
    use super::*;
    #[cfg(feature = "git")]
    use crate::config::Config;
    #[cfg(feature = "git")]
    use crate::test_support::{init_git_repo, run_git, unique_tmp};

    /// `s` (`cycle_diff_layout`) is a no-op while the diff's `Rendered` presentation is on screen —
    /// the same gate the footer already uses to hide the `s:...` hint there
    /// (`ui/status.rs::mode_footer`'s own `diff_rendered_active` check, [[hint-shown-iff-key-acts]]).
    /// `Source`/`Preview` are unaffected (still cycle normally), and the commit-detail view (where
    /// `s` also works — `diff_layout_and_word_diff...` tests that) is unaffected too, since
    /// `diff_rendered_active` is gated on `is_git_diff_preview()`.
    #[cfg(feature = "git")]
    #[test]
    fn cycle_diff_layout_is_a_no_op_while_rendered_is_active() {
        let dir = unique_tmp("konoma_cycle_diff_layout_rendered_noop_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        init_git_repo(&dir);
        std::fs::write(dir.join("a.md"), "# t\n\nbody\n").unwrap();
        run_git(&dir, &["add", "-A"]);
        run_git(&dir, &["commit", "-q", "-m", "init"]);
        std::fs::write(dir.join("a.md"), "# t\n\nCHANGED\n").unwrap();

        let canon = dir.canonicalize().unwrap();
        let mut app = App::new(canon.clone(), Config::default()).unwrap();
        app.open_git_diff(&canon.join("a.md"));
        assert_eq!(
            app.diff_view_for_test(),
            crate::app::DiffView::Rendered,
            "前提: 既定は Rendered"
        );
        assert!(app.diff_rendered_active());

        let layout_before = app.diff_layout;
        app.cycle_diff_layout();
        assert_eq!(
            app.diff_layout, layout_before,
            "Rendered 中は s で layout が変わってはいけない"
        );
        assert!(
            app.flash.is_none(),
            "Rendered 中の s はレイアウトラベルを flash してはいけない: {:?}",
            app.flash
        );

        // Leaving Rendered (→ Source, set directly — `cycle_diff_view` from `Rendered` goes to the
        // `Preview` representation instead, a different surface entirely) makes `s` work again.
        app.tab.diff_view = crate::app::DiffView::Source;
        assert!(!app.diff_rendered_active());
        app.cycle_diff_layout();
        assert_ne!(
            app.diff_layout, layout_before,
            "Source では s が通常どおり効く"
        );
    }
}
