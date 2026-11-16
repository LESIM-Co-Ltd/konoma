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
        let (kind, root, tab_id, path, text, label, count) = (
            job.kind,
            job.root.clone(),
            job.tab_id,
            job.path.clone(),
            job.text.clone(),
            job.label.clone(),
            job.count,
        );
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
                    path,
                    text,
                    label,
                    count,
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
            text,
            flag,
            label,
            count,
        } = job;
        let outcome = match kind {
            K::StageAll => crate::vcs::Git::stage_all(&root),
            K::UnstageAll => crate::vcs::Git::unstage_all(&root),
            K::Discard => crate::vcs::Git::discard(&root, &path),
            K::Commit => crate::vcs::Git::commit(&root, &text),
            K::CreateBranch => crate::vcs::Git::create_branch(&root, &text),
            K::WorktreeAdd => crate::vcs::Git::worktree_add(&root, &path, &text, flag),
        };
        crate::app::GitOpResult {
            gen,
            kind,
            root,
            tab_id,
            path,
            text,
            label,
            count,
            err: outcome.err().map(|e| e.to_string()),
        }
    }

    /// Whether it is safe for a completing git write to pop its own dialog back up.
    ///
    /// It is not, if the user has started typing anything in the meantime — another dialog (input
    /// *or* confirmation, including the quit confirmation), a `/` tree filter, an in-preview search,
    /// or a branch-list filter. All of those would have their next keystrokes stolen. This can only
    /// happen because the write is asynchronous: the synchronous version returned before the user
    /// could touch anything.
    fn can_restore_dialog(&self) -> bool {
        self.dialog.is_none() && !self.is_text_input_active()
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
            K::StageAll | K::UnstageAll => match &failed {
                Some(e) => self.flash = Some(failed_flash(e)),
                None => {
                    if shows_same_repo {
                        self.git_view_reload();
                    }
                    self.flash = Some(match res.kind {
                        K::StageAll => {
                            format!("{} ({})", tr(lang, Msg::StagedAll), res.count)
                        }
                        _ => tr(lang, Msg::UnstagedAll).to_string(),
                    });
                }
            },
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
            K::Commit => match &failed {
                // Show the failure (git's stderr) and reopen the input dialog with the same message
                // so it can be retried — but only when we're still on the tab that dispatched it and
                // the user is not in the middle of typing something else. Popping a commit box over
                // a half-typed `/` filter (or a quit confirmation) steals the next keystrokes, which
                // is worse than losing the draft. Only possible now that the write is asynchronous.
                Some(e) => {
                    self.flash = Some(e.clone());
                    if is_origin_tab && self.can_restore_dialog() {
                        self.reopen_input(PendingOp::GitCommit, Msg::CommitMessage, &res.text);
                    }
                }
                None => {
                    let post = self.refresh();
                    if shows_same_repo && self.is_git_view() {
                        self.git_view_reload();
                    }
                    self.flash = Some(match post {
                        // The commit succeeded but the reload didn't — say so rather than
                        // reporting an unverified success (same rule as `apply_file_op`).
                        Err(e) => format!("{}: {}", tr(lang, Msg::Failed), self.describe_error(&e)),
                        Ok(()) => tr(lang, Msg::Committed).to_string(),
                    });
                }
            },
            K::CreateBranch => match &failed {
                // git's own message, unmodified (both arms flashed it raw before).
                Some(e) => {
                    self.flash = Some(e.clone());
                    // Same retry affordance, and the same "don't steal live input" guard, as commit.
                    if is_origin_tab && self.can_restore_dialog() {
                        self.reopen_input(PendingOp::GitCreateBranch, Msg::NewBranch, &res.text);
                    }
                }
                None => {
                    let post = self.refresh();
                    if shows_same_repo {
                        // The branch label has to be right *now*: after this we land in the Git
                        // view, which never goes through `ui::tree::render` — the only production
                        // caller of the non-blocking `refresh_git_if_needed` — so a deferred
                        // refresh would leave the chip showing the old branch until the user
                        // happens to return to the tree. See `ensure_git_status_now`'s contract.
                        self.ensure_git_status_now();
                    }
                    self.flash = Some(match post {
                        Err(e) => format!("{}: {}", tr(lang, Msg::Failed), self.describe_error(&e)),
                        Ok(()) => format!("{}: {}", tr(lang, Msg::CreatedBranch), res.label),
                    });
                }
            },
            K::WorktreeAdd => match &failed {
                // git's own `fatal: ...`, shown unmodified. `git_worktrees` was never touched, so
                // the list is still showing behind the (now closed) dialog.
                Some(e) => self.flash = Some(e.clone()),
                None => {
                    // The target now exists on disk, so canonicalize it for a clean root/open_dir/
                    // flash (`worktree_dir` can carry a literal `..`, e.g. the default "../").
                    let path = res.path.canonicalize().unwrap_or(res.path);
                    // Moving a tab into the new worktree is the sharpest case for `is_origin_tab`:
                    // with root equality, pressing `t` while `worktree add` runs would drag the
                    // brand-new tab in and strand the one that asked.
                    if is_origin_tab {
                        self.reset_git_worktree_list_state();
                        self.jump_to_dir(path.clone());
                        self.tab.open_dir = path.clone();
                    }
                    self.flash = Some(format!(
                        "{}: {}",
                        tr(lang, Msg::CreatedWorktree),
                        home_relative(&path)
                    ));
                }
            },
        }
        true
    }

    /// `S` in the Git view = stage all (git add -A). Does nothing if there are no changes.
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn git_view_stage_all(&mut self) {
        if self.tab.git_view_entries.is_empty() {
            self.flash = Some(crate::i18n::tr(self.lang, crate::i18n::Msg::NoChanges).into());
            return;
        }
        // The count in the completion flash is how many entries were listed **when the key was
        // pressed** — same as the synchronous version, which read it before reloading the list.
        let count = self.tab.git_view_entries.len();
        self.start_git_op(crate::app::GitOpJob {
            kind: crate::app::GitOpKind::StageAll,
            root: PathBuf::new(), // filled in by start_git_op with tab.root at dispatch time
            tab_id: 0,            // likewise filled in by start_git_op
            path: PathBuf::new(),
            text: String::new(),
            flag: false,
            label: String::new(),
            count,
        });
    }
    /// `U` in the Git view = unstage all (git reset HEAD). Does nothing if nothing is staged.
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn git_view_unstage_all(&mut self) {
        if !self.tab.git_view_entries.iter().any(|e| e.staged) {
            self.flash = Some(crate::i18n::tr(self.lang, crate::i18n::Msg::NothingStaged).into());
            return;
        }
        self.start_git_op(crate::app::GitOpJob {
            kind: crate::app::GitOpKind::UnstageAll,
            root: PathBuf::new(), // filled in by start_git_op with tab.root at dispatch time
            tab_id: 0,            // likewise filled in by start_git_op
            path: PathBuf::new(),
            text: String::new(),
            flag: false,
            label: String::new(),
            count: 0,
        });
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
    /// `c`: Open the commit message input dialog. On confirm, git::commit (uses the already-staged index).
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn start_git_commit(&mut self) {
        self.dialog = Some(Dialog {
            op: PendingOp::GitCommit,
            kind: DialogKind::Input {
                title: crate::i18n::tr(self.lang, crate::i18n::Msg::CommitMessage).into(),
                buffer: String::new(),
                cursor: 0,
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
    /// `n`: Open the input dialog for a new branch name (on confirm, create and switch).
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn start_create_branch(&mut self) {
        self.dialog = Some(Dialog {
            op: PendingOp::GitCreateBranch,
            kind: DialogKind::Input {
                title: crate::i18n::tr(self.lang, crate::i18n::Msg::NewBranch).into(),
                buffer: String::new(),
                cursor: 0,
            },
        });
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
    /// `n`: Open the input dialog for a new linked worktree's branch name. The actual creation
    /// (new-vs-existing detection + `git::worktree_add` + success/failure handling) lives in
    /// `dialog_submit`'s `PendingOp::WorktreeCreate` arm — same split as `start_create_branch`.
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn start_create_worktree(&mut self) {
        self.dialog = Some(Dialog {
            op: PendingOp::WorktreeCreate,
            kind: DialogKind::Input {
                title: crate::i18n::tr(self.lang, crate::i18n::Msg::NewWorktree).into(),
                buffer: String::new(),
                cursor: 0,
            },
        });
    }
    /// Where a newly created worktree for `branch` should live: `[git] worktree_dir` resolved
    /// against the **main** worktree's own path (not `self.tab.root` — creating from inside a
    /// linked worktree must not make the placement drift with it; note the main entry's `path` is
    /// the right base even when it's a **bare** main worktree, since `git worktree list` always
    /// reports the bare repository's own directory there, the same "no checkout to switch into but
    /// still a real directory to build from" fact `worktree_switch_target`'s `Bare` case relies on),
    /// then joined with `branch`'s directory name — `/` replaced with `-`, since a slash in the leaf
    /// component would otherwise make `git worktree add` create a **nested** directory (`feat/`
    /// containing `nested/`) rather than one flat sibling; a worktree's directory name has no reason
    /// to mirror the branch's slash structure anyway. Falls back to `self.tab.root` if `worktrees()`
    /// somehow can't find a main entry (defensive only: a real repository always has one, and `n`
    /// only appears once the list — built from that same call — is already showing).
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub(super) fn worktree_create_target(&self, branch: &str) -> PathBuf {
        let main = crate::vcs::Git::worktrees(&self.tab.root)
            .into_iter()
            .find(|w| w.is_main)
            .map(|w| w.path)
            .unwrap_or_else(|| self.tab.root.clone());
        let dir_name = branch.replace('/', "-");
        main.join(&self.cfg.git.worktree_dir).join(dir_name)
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
    /// Decides the base branch for a linked worktree's "everything since base" diff for a
    /// given target worktree `w` (the VCS plugin has the same rule in `konoma-vcs`'s
    /// `worktrees::base`; this copy stays only for its tests, which cover the empty-config and
    /// all-missing fallbacks the plugin's tests do not). Reuses the graph's existing `[ui] graph_base_branches`
    /// (deliberately — no new setting to learn just for this one feature) plus the **main**
    /// worktree's checked-out branch (the first row `git worktree list` always reports, see
    /// `worktrees()`'s doc) as the candidate pool, then picks whichever candidate's merge-base with
    /// `w`'s own HEAD is **newest** — i.e. the branch `w` actually diverged from most recently —
    /// rather than picking by config-list position. `w`'s own branch is excluded from the pool
    /// (its merge-base with its own HEAD is HEAD itself, never a useful base).
    ///
    /// This ranking, not list order, matters in practice: on a repo where the main worktree sat on
    /// `develop` (well ahead of `main`) and a linked worktree branched off `develop`'s tip, diffing
    /// against `main` (first in `graph_base_branches = ["main", "develop"]`) showed +24 lines, 22 of
    /// which `develop` had already piled up *before* the worktree ever branched off it — `main`'s
    /// merge-base with the worktree was three weeks older than `develop`'s. Ranking by merge-base
    /// recency instead picks `develop` and correctly shows +2 lines.
    ///
    /// Ties (equal merge-base time — normally shouldn't happen, but handled defensively) are broken
    /// by candidate order: `graph_base_branches` entries in their configured order, then the main
    /// worktree's branch last — a strictly-greater comparison means the first-encountered candidate
    /// keeps its lead on a tie.
    ///
    /// `None` when no candidate resolves to a usable merge-base (doesn't exist in this repo, no
    /// merge-base with `w`'s HEAD, or the only candidate collapses to `w`'s own branch) — same
    /// "quietly return `None`, caller falls back to uncommitted-only diff" contract as before.
    #[cfg(all(test, feature = "git"))]
    fn worktree_diff_base(&self, w: &crate::git::WorktreeInfo) -> Option<String> {
        let mut candidates: Vec<&str> = Vec::new();
        for name in &self.cfg.ui.graph_base_branches {
            let name = name.as_str();
            if !candidates.contains(&name) && crate::vcs::ref_tip(&self.tab.root, name).is_some() {
                candidates.push(name);
            }
        }
        if let Some(main_branch) = self
            .tab
            .git_worktrees
            .as_ref()
            .and_then(|list| list.iter().find(|x| x.is_main))
            .and_then(|x| x.branch.as_deref())
        {
            if !candidates.contains(&main_branch) {
                candidates.push(main_branch);
            }
        }
        let own_branch = w.branch.as_deref();
        let mut best: Option<(&str, i64)> = None;
        for name in candidates {
            if Some(name) == own_branch {
                continue;
            }
            let Some(t) = crate::vcs::Git::merge_base_time(&w.path, name) else {
                continue;
            };
            let better = match best {
                None => true,
                Some((_, best_t)) => t > best_t,
            };
            if better {
                best = Some((name, t));
            }
        }
        best.map(|(name, _)| name.to_string())
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

    /// `R`: ask jj to take a snapshot.
    ///
    /// konoma tracks changes itself and never writes, which leaves one thing it cannot see: a file
    /// whose contents changed while its modification time did not. This is the way out of that, and
    /// the only write konoma makes to a jj repository — so it says so first, unless
    /// `[ui] confirm_jj_sync` is off.
    #[cfg(feature = "git")]
    pub fn jj_start_sync(&mut self) {
        if !self.allow_write("jj", true, false) {
            return;
        }
        if crate::vcs::detect(&self.tab.root) != crate::vcs::VcsKind::Jj {
            self.flash = Some(crate::i18n::tr(self.lang, crate::i18n::Msg::JjSyncNotJj).into());
            return;
        }
        if !self.cfg.ui.confirm_jj_sync {
            self.jj_run_sync();
            return;
        }
        self.dialog = Some(crate::app::Dialog {
            op: crate::app::PendingOp::JjSync,
            kind: crate::app::DialogKind::Confirm {
                message: crate::i18n::tr(self.lang, crate::i18n::Msg::JjSyncConfirm).to_string(),
                allow_permanent: false,
            },
        });
    }

    /// Takes the snapshot and re-reads, so the tree shows what jj now knows.
    #[cfg(feature = "git")]
    pub(super) fn jj_run_sync(&mut self) {
        if !self.allow_write("jj", true, false) {
            return;
        }
        let ok = crate::vcs::jj::snapshot(&self.tab.root);
        self.git_status_for = None;
        self.git_status_dirty = true;
        self.flash = Some(
            crate::i18n::tr(
                self.lang,
                if ok {
                    crate::i18n::Msg::JjSyncDone
                } else {
                    crate::i18n::Msg::JjSyncFailed
                },
            )
            .into(),
        );
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

// `worktree_diff_base` is private, so its tests live here (a descendant module of `git_view`) —
// same idiom as `paste_jump.rs`/`session_actions.rs`'s own co-located `mod tests`, rather than
// `src/app/tests.rs` (which cannot see it).
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

    /// `git commit` with an explicit, well-separated author/committer date (`GIT_AUTHOR_DATE` /
    /// `GIT_COMMITTER_DATE`, accepted as `@<unix-epoch> +0000`) so tests that assert "the newer
    /// commit wins" aren't flaky from two real `git commit` calls landing in the same wall-clock
    /// second.
    #[cfg(feature = "git")]
    fn commit_at(cwd: &std::path::Path, msg: &str, epoch: i64) {
        let date = format!("@{epoch} +0000");
        let out = std::process::Command::new("git")
            .current_dir(cwd)
            .args(["commit", "-q", "-m", msg])
            .env("GIT_AUTHOR_DATE", &date)
            .env("GIT_COMMITTER_DATE", &date)
            .output()
            .unwrap();
        assert!(out.status.success(), "git commit -m {msg:?}: {out:?}");
    }

    /// A main repo (one commit) with a linked worktree branched off it. Returns
    /// `(main_root, base_branch_name, linked_worktree_path)`.
    #[cfg(feature = "git")]
    fn sandbox_with_worktree(
        name: &str,
    ) -> (
        crate::test_support::TmpDir,
        String,
        crate::test_support::TmpDir,
    ) {
        let dir = unique_tmp(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        init_git_repo(&dir);
        std::fs::write(dir.join("a.txt"), b"one\n").unwrap();
        run_git(&dir, &["add", "-A"]);
        run_git(&dir, &["commit", "-q", "-m", "init"]);
        // `.canonicalize()` here transfers cleanup responsibility onto the returned guard (see its
        // own doc comment) rather than leaving it on `dir`/`linked`, which would otherwise drop
        // (and rm -rf the fixture) the moment this function returns.
        let root = dir.into_canonical().unwrap();
        let base = crate::git::branch(&root).expect("sanity: has a branch after the commit");
        let linked = unique_tmp(&format!("{name}_linked"));
        let _ = std::fs::remove_dir_all(&linked);
        run_git(
            &root,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "feature-x",
                linked.to_str().unwrap(),
            ],
        );
        let linked = linked.into_canonical().unwrap();
        (root, base, linked)
    }

    /// Fetches the linked worktree's row (added by `sandbox_with_worktree`, checked out on
    /// `branch`) from `tab.git_worktrees`, populated by the test before the call. All
    /// three "existing branch resolves" tests below target this row rather than the main
    /// worktree's: `worktree_diff_base` now excludes the *target's own* branch from its candidate
    /// pool, and in each of these configs the main worktree's own branch is the only (or first)
    /// candidate — targeting the main worktree row would make it resolve to `None`, not the branch
    /// under test. The linked worktree branched off that same branch and made no commits of its
    /// own, so its HEAD *is* the candidate's tip: `merge_base(candidate, HEAD)` trivially equals
    /// that tip, giving `merge_base_time` something to compare.
    #[cfg(feature = "git")]
    fn linked_worktree_row(app: &App, branch: &str) -> crate::git::WorktreeInfo {
        app.tab
            .git_worktrees
            .as_ref()
            .expect("git_worktrees populated by the test")
            .iter()
            .find(|w| w.branch.as_deref() == Some(branch))
            .cloned()
            .unwrap_or_else(|| panic!("no worktree row for branch {branch:?}"))
    }

    /// `graph_base_branches` mixing branches that don't exist with one that does: the existing one
    /// resolves as the diff base for the linked worktree (branched off it, so it's also the only
    /// candidate with a merge-base against the linked worktree's HEAD).
    #[cfg(feature = "git")]
    #[test]
    fn worktree_diff_base_picks_first_existing_configured_branch() {
        let (root, base, _linked) = sandbox_with_worktree("konoma_gitview_diffbase_prefer");
        let mut cfg = Config::default();
        cfg.ui.graph_base_branches = vec![
            "konoma-does-not-exist".to_string(),
            base.clone(),
            "konoma-also-missing".to_string(),
        ];
        let mut app = App::new(root.to_path_buf(), cfg).unwrap();
        app.tab.git_worktrees = Some(crate::vcs::Git::worktrees(&app.tab.root)); // the fallback path's source
        let target = linked_worktree_row(&app, "feature-x");
        assert_eq!(
            app.worktree_diff_base(&target),
            Some(base),
            "設定済みの中で実在するものを選ぶ"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// An empty `graph_base_branches` (the default): falls back to the **main** worktree's
    /// checked-out branch as the sole candidate for the linked worktree.
    #[cfg(feature = "git")]
    #[test]
    fn worktree_diff_base_falls_back_to_main_worktree_branch_when_config_is_empty() {
        let (root, base, _linked) = sandbox_with_worktree("konoma_gitview_diffbase_fallback");
        let cfg = Config::default(); // graph_base_branches defaults to []
        assert!(cfg.ui.graph_base_branches.is_empty(), "前提: 既定は []");
        let mut app = App::new(root.to_path_buf(), cfg).unwrap();
        app.tab.git_worktrees = Some(crate::vcs::Git::worktrees(&app.tab.root));
        let target = linked_worktree_row(&app, "feature-x");
        assert_eq!(
            app.worktree_diff_base(&target),
            Some(base),
            "config が空ならメインワークツリーのブランチへ"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// A non-empty `graph_base_branches` whose entries **all** fail to resolve also falls back to
    /// the main worktree's branch (not just the empty-config case).
    #[cfg(feature = "git")]
    #[test]
    fn worktree_diff_base_falls_back_when_every_configured_branch_is_missing() {
        let (root, base, _linked) = sandbox_with_worktree("konoma_gitview_diffbase_all_missing");
        let mut cfg = Config::default();
        cfg.ui.graph_base_branches =
            vec!["konoma-nope".to_string(), "konoma-also-nope".to_string()];
        let mut app = App::new(root.to_path_buf(), cfg).unwrap();
        app.tab.git_worktrees = Some(crate::vcs::Git::worktrees(&app.tab.root));
        let target = linked_worktree_row(&app, "feature-x");
        assert_eq!(
            app.worktree_diff_base(&target),
            Some(base),
            "設定が全滅ならメインワークツリーのブランチへ"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// The motivating regression, reproduced from a measurement on a real repo: `main` is listed
    /// **first** in `graph_base_branches`, but the worktree actually branched off `develop`'s tip
    /// (which had already advanced past `main`). The old "first configured branch that exists"
    /// rule picked `main`, dragging in everything `develop` had already piled up *before* the
    /// worktree ever branched off it — a diff that was mostly noise. Ranking candidates by
    /// merge-base recency instead picks `develop`, the branch actually diverged from, regardless
    /// of its position in the config list.
    #[cfg(feature = "git")]
    #[test]
    fn worktree_diff_base_prefers_newest_merge_base_over_config_order() {
        let dir = unique_tmp("konoma_gitview_diffbase_recency");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        init_git_repo(&dir);
        std::fs::write(dir.join("main.txt"), b"main\n").unwrap();
        run_git(&dir, &["add", "-A"]);
        commit_at(&dir, "on main", 1_700_000_000); // deterministic: 20 days apart from develop's commit
        run_git(&dir, &["branch", "-m", "main"]); // name the initial branch deterministically

        run_git(&dir, &["checkout", "-q", "-b", "develop"]);
        std::fs::write(dir.join("develop.txt"), b"develop\n").unwrap();
        run_git(&dir, &["add", "-A"]);
        commit_at(&dir, "on develop", 1_720_000_000); // newer than main's commit — develop's tip
                                                      // is now the merge-base any worktree branched off it will have with `develop`.

        let root = dir.canonicalize().unwrap();
        let linked = unique_tmp("konoma_gitview_diffbase_recency_linked");
        let _ = std::fs::remove_dir_all(&linked);
        run_git(
            &root, // root is currently checked out on `develop`, so the worktree branches off its tip
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "agent-branch",
                linked.to_str().unwrap(),
            ],
        );
        let linked = linked.canonicalize().unwrap();
        std::fs::write(linked.join("agent.txt"), b"agent\n").unwrap();
        run_git(&linked, &["add", "-A"]);
        commit_at(&linked, "agent work", 1_730_000_000);

        let mut cfg = Config::default();
        cfg.ui.graph_base_branches = vec!["main".to_string(), "develop".to_string()];
        let mut app = App::new(root.clone(), cfg).unwrap();
        app.tab.git_worktrees = Some(crate::vcs::Git::worktrees(&app.tab.root));
        let target = linked_worktree_row(&app, "agent-branch");
        assert_eq!(
            app.worktree_diff_base(&target),
            Some("develop".to_string()),
            "develop の方が merge-base が新しい＝実際に分岐した枝を選ぶ（config では main が先でも）"
        );
        std::fs::remove_dir_all(&root).ok();
        std::fs::remove_dir_all(&linked).ok();
    }

    /// When the only candidate collapses to the diffed worktree's own branch (empty config, so the
    /// only fallback candidate is the main worktree's branch, and the target *is* the main
    /// worktree), it's excluded from the pool and nothing is left to compare against — `None`, not
    /// a self-diff.
    #[cfg(feature = "git")]
    #[test]
    fn worktree_diff_base_is_none_when_only_candidate_is_the_targets_own_branch() {
        let (root, _base, _linked) = sandbox_with_worktree("konoma_gitview_diffbase_self_excluded");
        let cfg = Config::default(); // graph_base_branches defaults to []
        let mut app = App::new(root.to_path_buf(), cfg).unwrap();
        app.tab.git_worktrees = Some(crate::vcs::Git::worktrees(&app.tab.root));
        let main_row = app
            .git_worktree_view()
            .into_iter()
            .find(|w| w.is_current)
            .expect("the main worktree row is the current one");
        assert!(main_row.is_main, "前提: カーソルはメインワークツリー行");
        assert_eq!(
            app.worktree_diff_base(&main_row),
            None,
            "唯一の候補＝自分自身のブランチは除外されるので None"
        );
        std::fs::remove_dir_all(&root).ok();
    }

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
