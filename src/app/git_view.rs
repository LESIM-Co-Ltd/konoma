//! Git views: changes hub, branches, worktrees, commit graph, and commit detail — methods on `App`.

use super::list_filter::ListId;
use super::*;

#[cfg(all(test, feature = "git"))]
#[path = "diff_source_test_adapter.rs"]
mod diff_source_test_adapter;

#[cfg(test)]
thread_local! {
    static LEGACY_HUB_OPENS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
impl App {
    pub(crate) fn count_legacy_hub_opens<T>(f: impl FnOnce() -> T) -> (T, usize) {
        let before = LEGACY_HUB_OPENS.with(|calls| calls.get());
        let result = f();
        (result, LEGACY_HUB_OPENS.with(|calls| calls.get() - before))
    }
}

/// How a diff is being opened — the input `App::open_git_diff_with` needs *before* it decides
/// `diff_follow_scope`/`came_from_git_view`/the presentation and validates that presentation
/// exactly once. `Default` is the ordinary "fresh open, from a tree/hub selection" shape that plain
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
    /// `Some(b)` preserves a `came_from_git_view` value the caller already has, instead of the
    /// ordinary "opened straight from wherever the tab's Git view currently is" rule
    /// (`self.tab.git_view`, which a fresh open always reads and then closes). `n`/`N` and the
    /// `Preview` return both want this — they are continuing a diff that was already open, not
    /// opening one from the Git view.
    pub came_from_git_view: Option<bool>,
    /// A diff already fetched by the caller (`follow_jump`'s own `compute_gitdiff_lines`), seeded
    /// into `diff_cache` so the first render of the `Source` presentation doesn't have to re-invoke
    /// git for something already in hand. `None` (the default) leaves the cache empty
    /// (`invalidate_diff_caches` already clears it) for the render to populate lazily.
    pub seeded_diff: Option<DiffCache>,
}

impl App {
    /// Whether the Git view is showing. False when the feature is disabled or this is not a repo (open_git_view never sets it).
    pub fn is_git_view(&self) -> bool {
        self.tab.git_view
    }
    /// The change list (for rendering).
    pub fn git_view_entries(&self) -> &[crate::git::ChangeEntry] {
        &self.tab.git_view_entries
    }
    /// Cursor position in the Git view (used for render scrolling/inversion).
    pub fn git_view_sel(&self) -> usize {
        self.tab.git_view_sel
    }
    /// Absolute path of the changed file at the cursor.
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn git_view_selected(&self) -> Option<PathBuf> {
        self.tab
            .git_view_entries
            .get(self.tab.git_view_sel)
            .map(|e| e.path.clone())
    }
    /// Rebuild the list after a write operation and clamp the cursor. Also invalidates and refetches git status.
    pub fn git_view_reload(&mut self) {
        self.tab.git_view_entries = crate::vcs::changed_files(&self.tab.root);
        if self.tab.git_view_sel >= self.tab.git_view_entries.len() {
            self.tab.git_view_sel = self.tab.git_view_entries.len().saturating_sub(1);
        }
        self.git_status_for = None; // also re-fetch the tree's git status next time
        self.git_status_dirty = true; // status changed due to a git operation = invalidate the workdir cache
    }

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
                        self.tab.came_from_git_view = false;
                        self.back_to_tree();
                        self.enter_project();
                    }
                    if shows_same_repo {
                        self.git_view_reload();
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

    // --- Git view stubs (replaced by phase 4/5) -------------------------
    /// `Enter`/`l`: Open the selected file's diff in the GitDiff preview. Closes the Git view and
    /// remembers where it came from (came_from_git_view) so Esc/q can return to the Git view. A
    /// fresh, tree-/hub-originated open — `open_git_diff_with`'s own doc comment covers the other
    /// three call shapes (follow-originated, `n`/`N`, returning from the `Preview` representation).
    pub fn open_git_diff(&mut self, path: &Path) {
        self.open_git_diff_with(path, DiffOpen::default());
    }

    /// `open_git_diff`, generalized over *how* the diff is being opened — the single place that
    /// decides `diff_follow_scope`, `came_from_git_view` and the presentation (`apply_diff_view`)
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
    /// preview` had the same save/restore shape for `came_from_git_view`/`diff_follow_scope` (though
    /// not the double-validation bug, since it never re-entered `Rendered`). All three now build a
    /// `DiffOpen` up front and call this once.
    pub(super) fn open_git_diff_with(&mut self, path: &Path, open: DiffOpen) {
        self.tab.diff_read_context = open.read_context;
        self.tab.preview_path = Some(path.to_path_buf());
        self.set_preview_kind(Some(PreviewKind::GitDiff(path.to_path_buf())));
        // The diff preview draws itself (it doesn't use window/image/md). Reset the related state.
        self.tab.preview_scroll = 0;
        self.tab.preview_hscroll = 0;
        self.tab.preview_byte_top = 0;
        self.tab.preview_top_line = 0;
        // A fresh open: drop any leftover "scroll to first change" reservation before deciding, at
        // the bottom of this fn, whether to arm a new one for `path` — mirrors `App::enter_preview`'s
        // own up-front clear (`diff_scroll_pending`'s own doc comment).
        self.tab.diff_scroll_pending = None;
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
        self.tab.came_from_git_view = open.came_from_git_view.unwrap_or(self.tab.git_view);
        self.tab.git_view = false;
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
        self.open_git_diff(&path); // came_from_git_view=false (tree-originated) → `q` returns to the tree
    }

    /// Whether to lay the diff side by side at display width `width` (resolves Auto).
    pub fn diff_is_split(&self, width: u16) -> bool {
        self.diff_layout.is_split(width)
    }

    pub(crate) fn source_diff_layout(&self) -> konoma_widgets::diff_frame::SourceDiffLayout {
        konoma_plugin::host::project::diff_frame::source_diff_layout(self.diff_layout)
    }
    /// Cycle the diff layout unified→split→Auto (`s`). Called from both the GitDiff preview and the
    /// detail view. While the media diff's side-by-side view is actually **showing pictures**
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
        self.tab.git_detail_hscroll = 0; // reset the horizontal position on layout switch (its meaning changes)
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
        self.tab.came_from_git_view = false;
        self.back_to_tree();
    }

    /// `x` in the GitDiff preview = discard all changes to the displayed file. Opens a confirmation dialog
    /// (on confirm: git::discard then open the project's changes screen). Uses the same flow as discard from the Git view.
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
    /// Whether the git log is showing (for render/key branching).
    pub fn is_git_log(&self) -> bool {
        self.tab.git_log.is_some()
    }

    /// Returns the id of the selected commit.
    // Read only by the git-gated `current_commit_meta` (copy_actions); U9l removes the pair.
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn git_log_selected_id(&self) -> Option<String> {
        self.tab
            .git_log
            .as_ref()?
            .get(self.tab.git_log_sel)
            .map(|c| c.id.clone())
    }

    /// Whether the branch list is showing.
    pub fn is_git_branches(&self) -> bool {
        self.tab.git_branches.is_some()
    }
    /// The filtered display list (names containing the query, case-insensitive). All entries if the query is empty. Used for rendering/operations.
    pub fn git_branch_view(&self) -> Vec<crate::git::BranchInfo> {
        let Some(all) = &self.tab.git_branches else {
            return Vec::new();
        };
        let q = self.tab.git_branch_filter.to_lowercase();
        if q.is_empty() {
            all.clone()
        } else {
            all.iter()
                .filter(|b| b.name.to_lowercase().contains(&q))
                .cloned()
                .collect()
        }
    }
    /// The branch selected in the display list.
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub(super) fn git_branch_selected(&self) -> Option<crate::git::BranchInfo> {
        self.git_branch_view()
            .into_iter()
            .nth(self.tab.git_branch_sel)
    }
    // --- Branch filtering (`/`): the shared list filter (`list_filter.rs`) ---------------
    /// Whether branch-filter input is active (while true, main captures keys as characters).
    pub fn git_branch_filtering(&self) -> bool {
        self.list_filtering(ListId::Branches)
    }
    pub(super) fn clamp_git_branch_sel(&mut self) {
        let len = self.git_branch_view().len();
        if self.tab.git_branch_sel >= len {
            self.tab.git_branch_sel = len.saturating_sub(1);
        }
    }

    // --- Linked worktrees (`w` from the changes hub) --------------------------------
    // "Worktree" here means a **linked working tree** (`git worktree add`) — a different concept
    // from the rest of this codebase's "worktree", which always means *the uncommitted working
    // tree* (`GitWorktreeDiff`/`GraphRow.worktree`/config's `git_worktree_diff`). Do not rename
    // either to avoid the ambiguity; the doc comments carry the distinction instead.

    /// Whether the worktree list is showing.
    pub fn is_git_worktrees(&self) -> bool {
        self.tab.git_worktrees.is_some()
    }
    /// The filtered display list (branch name or path containing the query, case-insensitive). All
    /// entries if the query is empty. Used for rendering/operations.
    pub fn git_worktree_view(&self) -> Vec<crate::git::WorktreeInfo> {
        let Some(all) = &self.tab.git_worktrees else {
            return Vec::new();
        };
        let q = self.tab.git_worktree_filter.to_lowercase();
        if q.is_empty() {
            all.clone()
        } else {
            all.iter()
                .filter(|w| {
                    w.branch
                        .as_deref()
                        .is_some_and(|b| b.to_lowercase().contains(&q))
                        || w.path.to_string_lossy().to_lowercase().contains(&q)
                })
                .cloned()
                .collect()
        }
    }
    /// Clears the worktree list's transient UI state (list/cursor/filter/filtering). Shared by
    /// `close_git_worktrees` (which follows up with `open_git_view()` to return to the hub),
    /// `worktree_goto`'s `Go` arm (which follows up with `jump_to_dir` instead — switching root,
    /// not returning to the hub, so it can't just call `close_git_worktrees`), and `dialog_submit`'s
    /// `PendingOp::WorktreeCreate` success arm (same "switched root, don't return to the hub" case
    /// as `worktree_goto`, hence `pub(super)` rather than private — `dialog_submit` lives in the
    /// sibling `dialog_actions` module). Kept as one spot so a future field added to this state
    /// can't be reset in one caller and forgotten in the others.
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub(super) fn reset_git_worktree_list_state(&mut self) {
        self.tab.git_worktrees = None;
        self.tab.git_worktree_sel = 0;
        self.tab.git_worktree_filter.clear();
        self.tab.git_worktree_filtering = false;
    }
    /// The existing worktree root switch, also used by a project's SwitchRoot outcome.
    pub(super) fn switch_worktree_root(&mut self, path: PathBuf) {
        self.reset_git_worktree_list_state();
        self.jump_to_dir(path.clone());
        self.tab.open_dir = path;
        self.flash = Some(format!(
            "{}: {}",
            crate::i18n::tr(self.lang, crate::i18n::Msg::SwitchedTo),
            home_relative(&self.tab.root)
        ));
    }
    // --- Worktree filtering (`/`): the shared list filter (`list_filter.rs`) ------------
    /// Whether worktree-filter input is active (while true, main captures keys as characters).
    pub fn git_worktree_filtering(&self) -> bool {
        self.list_filtering(ListId::Worktrees)
    }
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub(super) fn clamp_git_worktree_sel(&mut self) {
        let len = self.git_worktree_view().len();
        if self.tab.git_worktree_sel >= len {
            self.tab.git_worktree_sel = len.saturating_sub(1);
        }
    }

    /// Whether the panel is showing.
    pub fn is_git_graph_picker(&self) -> bool {
        self.git_graph_picker
    }
    /// Whether the graph is showing.
    pub fn is_git_graph(&self) -> bool {
        self.tab.git_graph.is_some()
    }
    /// **Whether Git is the main mode** (one of the changes hub / log / graph / branches / worktrees /
    /// detail / diff). Used to set the outer (main) mode chip to `GIT`. The git views overlay tree/preview.
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn in_git_view(&self) -> bool {
        self.is_git_view()
            || self.is_git_log()
            || self.is_git_graph()
            || self.is_git_branches()
            || self.is_git_worktrees()
            || self.is_git_detail()
            || self.is_git_diff_preview()
    }
    /// The graph's cursor row (for render/title).
    // Read only by the git-gated `current_commit_meta` (copy_actions); U9l removes the pair.
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn git_graph_selected_row(&self) -> Option<&crate::git::GraphRow> {
        self.tab
            .git_graph
            .as_ref()
            .and_then(|rows| rows.get(self.tab.git_graph_sel))
    }
    /// Whether the commit detail is showing (for render/key branching).
    pub fn is_git_detail(&self) -> bool {
        self.tab.git_detail.is_some()
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
