//! Git views: changes hub, branches, worktrees, commit graph, and commit detail — methods on `App`.

use super::*;

/// Outcome of checking whether the worktree list's selected row can be switched to / opened in a
/// new tab (`worktree_goto`/`worktree_goto_new_tab` share this).
#[cfg_attr(not(feature = "git"), allow(dead_code))]
#[derive(Debug, Clone, PartialEq, Eq)]
enum WorktreeTarget {
    /// A real, present, non-bare, non-current checkout — safe to switch/open into.
    Go(PathBuf),
    /// Already the worktree this tab is in; nothing to do.
    AlreadyCurrent,
    /// A bare repository standing in as the main worktree — no checkout exists to switch into.
    Bare,
    /// git considers it prunable, or its checkout directory is missing.
    Unavailable,
}

impl App {
    // --- Git view (changes hub, default key `o`, changeable via keymap) -----------------
    /// `o`: Open the Git view. Reads the change list for the current root and moves the cursor to the top.
    /// When the feature is disabled, `[external] git = false`, this is not a repo, or (in the `git`
    /// build only) no `git` executable exists on this machine, do nothing (`is_git_view` stays false =
    /// no-op). Each of those reasons flashes its own message — "not a repo" must not be blamed for a
    /// missing binary. The no-git build has no way to probe for a git executable (git support itself is
    /// compiled out, independent of what is actually installed), so it never claims "git is not
    /// installed" and keeps flashing "not a repo" as before.
    pub fn open_git_view(&mut self) {
        if !self.cfg.external.git {
            // Explicitly disabled by config (the feature itself is on, and it could still be a repo): notify distinctly from "not a repo".
            self.flash =
                Some(crate::i18n::tr(self.lang, crate::i18n::Msg::ExternalGitDisabled).into());
            return;
        }
        #[cfg(feature = "git")]
        {
            if !crate::git::git_binary_available() {
                // Config allows it but there's no git executable. We haven't determined whether this
                // is a repo (discover goes through in-process libgit2), so we don't say "not a repo".
                self.flash =
                    Some(crate::i18n::tr(self.lang, crate::i18n::Msg::GitNotInstalled).into());
                return;
            }
        }
        // The backend's own answer: None means "no repository I can show", which is an unborn git
        // repository as much as a directory with no `.jj` and no `.git`.
        if crate::vcs::branch(&self.tab.root).is_none() {
            // Not a repo (or the feature is disabled). Ignore safely and notify via flash.
            self.flash = Some(crate::i18n::tr(self.lang, crate::i18n::Msg::NotAGitRepo).into());
            return;
        }
        self.tab.git_view_entries = crate::vcs::changed_files(&self.tab.root);
        self.tab.git_view_sel = 0;
        self.tab.git_view = true;
    }
    /// Close the Git view (q/Esc).
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn close_git_view(&mut self) {
        self.tab.git_view = false;
    }
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
    /// Move the cursor by delta rows, clamped to [0, last].
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn git_view_move(&mut self, delta: i32) {
        self.tab.git_view_sel = clamp_cursor(
            self.tab.git_view_sel,
            delta,
            self.tab.git_view_entries.len(),
        );
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
            K::Stage => crate::git::stage(&root, &path),
            K::Unstage => crate::git::unstage(&root, &path),
            K::StageAll => crate::git::stage_all(&root),
            K::UnstageAll => crate::git::unstage_all(&root),
            K::Discard => crate::git::discard(&root, &path),
            K::Commit => crate::git::commit(&root, &text),
            K::Checkout => crate::git::checkout(&root, &text),
            K::CreateBranch => crate::git::create_branch(&root, &text),
            K::DeleteBranch => crate::git::delete_branch(&root, &text, flag),
            K::WorktreeAdd => crate::git::worktree_add(&root, &path, &text, flag),
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
            K::Stage | K::Unstage | K::StageAll | K::UnstageAll => match &failed {
                Some(e) => self.flash = Some(failed_flash(e)),
                None => {
                    if shows_same_repo {
                        self.git_view_reload();
                    }
                    self.flash = Some(match res.kind {
                        K::Stage => format!("{}: {}", tr(lang, Msg::Staged), res.label),
                        K::Unstage => format!("{}: {}", tr(lang, Msg::Unstaged), res.label),
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
                    if is_origin_tab && self.is_git_diff_preview() {
                        self.tab.came_from_git_view = false;
                        self.back_to_tree();
                        self.open_git_view();
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
            K::Checkout | K::CreateBranch => match &failed {
                // git's own message, unmodified (both arms flashed it raw before).
                Some(e) => {
                    self.flash = Some(e.clone());
                    // Same retry affordance, and the same "don't steal live input" guard, as commit.
                    if res.kind == K::CreateBranch && is_origin_tab && self.can_restore_dialog() {
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
                    // Navigation: only the tab that asked leaves the branch list for the Git view.
                    if is_origin_tab {
                        self.close_git_branches();
                    }
                    self.flash = Some(match post {
                        Err(e) => format!("{}: {}", tr(lang, Msg::Failed), self.describe_error(&e)),
                        Ok(()) if res.kind == K::Checkout => {
                            format!("{}: {}", tr(lang, Msg::SwitchedTo), res.label)
                        }
                        Ok(()) => format!("{}: {}", tr(lang, Msg::CreatedBranch), res.label),
                    });
                }
            },
            K::DeleteBranch => match &failed {
                Some(e) => self.flash = Some(e.clone()),
                None => {
                    if shows_same_repo {
                        self.git_branches_reload();
                    }
                    self.flash = Some(format!("{}: {}", tr(lang, Msg::DeletedBranch), res.label));
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

    /// `s` in the Git view = stage. Flashes success/failure and rebuilds the list.
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn git_view_stage(&mut self) {
        let Some(path) = self.git_view_selected() else {
            return;
        };
        let label = self.format_path(&path);
        self.start_git_op(crate::app::GitOpJob {
            kind: crate::app::GitOpKind::Stage,
            root: PathBuf::new(), // filled in by start_git_op with tab.root at dispatch time
            tab_id: 0,            // likewise filled in by start_git_op
            path,
            text: String::new(),
            flag: false,
            label,
            count: 0,
        });
    }
    /// `u` in the Git view = unstage.
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn git_view_unstage(&mut self) {
        let Some(path) = self.git_view_selected() else {
            return;
        };
        let label = self.format_path(&path);
        self.start_git_op(crate::app::GitOpJob {
            kind: crate::app::GitOpKind::Unstage,
            root: PathBuf::new(), // filled in by start_git_op with tab.root at dispatch time
            tab_id: 0,            // likewise filled in by start_git_op
            path,
            text: String::new(),
            flag: false,
            label,
            count: 0,
        });
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
    /// `x` in the Git view = discard. Opens a confirmation dialog (on confirm: git::discard then reload).
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn git_view_start_discard(&mut self) {
        // The footer hides this key when the backend cannot write, but the binding still exists —
        // say why rather than attempting a git operation on a repository git does not own.
        if !crate::vcs::caps(&self.tab.root).write {
            self.flash =
                Some(crate::i18n::tr(self.lang, crate::i18n::Msg::VcsReadOnly).to_string());
            return;
        }
        let Some(path) = self.git_view_selected() else {
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

    // --- Git view stubs (replaced by phase 4/5) -------------------------
    /// `Enter`/`l`: Open the selected file's diff in the GitDiff preview. Closes the Git view and
    /// remembers where it came from (came_from_git_view) so Esc/q can return to the Git view.
    pub fn open_git_diff(&mut self, path: &Path) {
        self.tab.preview_path = Some(path.to_path_buf());
        self.tab.preview_kind = Some(PreviewKind::GitDiff(path.to_path_buf()));
        // The diff preview draws itself (it doesn't use window/image/md). Reset the related state.
        self.tab.preview_scroll = 0;
        self.tab.preview_hscroll = 0;
        self.tab.preview_byte_top = 0;
        self.tab.preview_top_line = 0;
        self.preview_win = None;
        self.win_cache = None;
        self.preview_total_lines = None;
        self.md_cache = None;
        self.diff_cache = None; // opening another file's diff: invalidate the raw diff cache
        self.md_items.clear();
        self.tab.focused_item = None;
        self.hl_pending = false;
        self.hl_warming = false;
        self.tab.came_from_git_view = self.tab.git_view;
        self.tab.git_view = false;
        // Defaults to the full git change scope (only the follow-originated case has the caller override it to true).
        self.diff_follow_scope = false;
        self.tab.mode = Mode::Preview;
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
    /// Cycle the diff layout unified→split→Auto (`s`). Called from both the GitDiff preview and the detail view.
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn cycle_diff_layout(&mut self) {
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

    /// Close the GitDiff preview (q/Esc). Returns to the Git view if it came from there,
    /// otherwise returns to the tree.
    pub fn close_git_diff(&mut self) {
        let return_to_git = self.tab.came_from_git_view;
        self.back_to_tree();
        if return_to_git {
            self.tab.came_from_git_view = false;
            self.open_git_view();
        }
    }

    /// `x` in the GitDiff preview = discard all changes to the displayed file. Opens a confirmation dialog
    /// (on confirm: git::discard then return to the Git view). Uses the same flow as discard from the Git view.
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn git_diff_start_discard(&mut self) {
        if !crate::vcs::caps(&self.tab.root).write {
            self.flash =
                Some(crate::i18n::tr(self.lang, crate::i18n::Msg::VcsReadOnly).to_string());
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
        // Since we want to return to the Git view after discarding, set the return-origin flag ahead of time.
        self.tab.came_from_git_view = true;
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
    /// `L`: Open the git log (linear, newest first). Reads up to 200 entries and shows a full-screen list.
    /// Opened from the Git view, so the Git view closes when it opens (q/Esc returns to the Git view).
    /// Does nothing if there are no commits (unborn), this is not a repo, or the feature is disabled (no-op).
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn open_git_log(&mut self) {
        let commits = crate::vcs::log(&self.tab.root, 200);
        if commits.is_empty() {
            self.flash = Some(crate::i18n::tr(self.lang, crate::i18n::Msg::NoCommits).into());
            return;
        }
        self.tab.git_log = Some(commits);
        self.tab.git_log_sel = 0;
        // The log is a higher-level view than the Git view. Close the open Git view (the return target is restored on close).
        self.tab.git_view = false;
    }

    /// Whether the git log is showing (for render/key branching).
    pub fn is_git_log(&self) -> bool {
        self.tab.git_log.is_some()
    }

    /// Returns all commits in the git log (for rendering). Empty when hidden.
    pub fn git_log_entries(&self) -> &[crate::git::CommitInfo] {
        self.tab.git_log.as_deref().unwrap_or(&[])
    }

    /// Cursor position in the git log.
    pub fn git_log_sel(&self) -> usize {
        self.tab.git_log_sel
    }

    /// Move the git log cursor by delta (clamped to range).
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn git_log_move(&mut self, delta: i32) {
        self.tab.git_log_sel =
            clamp_cursor(self.tab.git_log_sel, delta, self.git_log_entries().len());
    }

    /// Returns the id of the selected commit.
    pub fn git_log_selected_id(&self) -> Option<String> {
        self.tab
            .git_log
            .as_ref()?
            .get(self.tab.git_log_sel)
            .map(|c| c.id.clone())
    }

    /// Close the git log (q/Esc). Returns to the Git view since it came from there.
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn close_git_log(&mut self) {
        self.tab.git_log = None;
        self.tab.git_log_sel = 0;
        // The log is assumed to open on top of the Git view, so closing it returns to the Git view.
        self.open_git_view();
    }

    // --- Branch operations (`b` list / Enter switch / n new / d delete / `/` filter) ----------
    /// `b`: Open the local branch list. Opened from the Git view, so the view closes. Cursor moves to the current branch. The filter is reset.
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn open_git_branches(&mut self) {
        let list = crate::vcs::refs(&self.tab.root);
        if list.is_empty() {
            self.flash = Some(crate::i18n::tr(self.lang, crate::i18n::Msg::NoBranches).into());
            return;
        }
        self.tab.git_branch_filter.clear();
        self.tab.git_branch_filtering = false;
        self.tab.git_branch_sel = list.iter().position(|b| b.is_current).unwrap_or(0);
        self.tab.git_branches = Some(list);
        self.tab.git_view = false;
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
    /// Cursor position in the display list.
    pub fn git_branch_sel(&self) -> usize {
        self.tab.git_branch_sel
    }
    /// Move the display-list cursor by delta (clamped to range).
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn git_branch_move(&mut self, delta: i32) {
        self.tab.git_branch_sel =
            clamp_cursor(self.tab.git_branch_sel, delta, self.git_branch_view().len());
    }
    /// The branch selected in the display list.
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub(super) fn git_branch_selected(&self) -> Option<crate::git::BranchInfo> {
        self.git_branch_view()
            .into_iter()
            .nth(self.tab.git_branch_sel)
    }
    /// Close the branch list (q/Esc). Returns to the Git view since it came from there. Also resets the filter.
    pub fn close_git_branches(&mut self) {
        self.tab.git_branches = None;
        self.tab.git_branch_sel = 0;
        self.tab.git_branch_filter.clear();
        self.tab.git_branch_filtering = false;
        self.open_git_view();
    }
    /// Refetch the list (e.g. after deletion). Clamps the cursor to range.
    fn git_branches_reload(&mut self) {
        self.tab.git_branches = Some(crate::vcs::refs(&self.tab.root));
        self.clamp_git_branch_sel();
    }
    /// `Enter`: Switch to the selected branch. If uncommitted changes conflict, git refuses (Err is flashed). On success, returns to the Git view.
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn checkout_selected_branch(&mut self) -> Result<()> {
        let Some(b) = self.git_branch_selected() else {
            return Ok(());
        };
        let name = b.name;
        // Runs in the background (design principle #4) — a checkout rewrites the working tree and
        // can stall on a slow filesystem. `refresh`/`ensure_git_status_now`/`close_git_branches`
        // move to `apply_git_op`.
        self.start_git_op(crate::app::GitOpJob {
            kind: crate::app::GitOpKind::Checkout,
            root: PathBuf::new(), // filled in by start_git_op with tab.root at dispatch time
            tab_id: 0,            // likewise filled in by start_git_op
            path: PathBuf::new(),
            text: name.clone(),
            flag: false,
            label: name,
            count: 0,
        });
        Ok(())
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
    /// `d`: Open the delete confirmation for the selected branch (`y`=safe -d / `!`=force -D). The current branch cannot be deleted.
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn start_delete_branch(&mut self) {
        let Some(b) = self.git_branch_selected() else {
            return;
        };
        if b.is_current {
            self.flash = Some(
                crate::i18n::tr(self.lang, crate::i18n::Msg::CannotDeleteCurrentBranch).into(),
            );
            return;
        }
        let name = b.name;
        self.dialog = Some(Dialog {
            op: PendingOp::GitDeleteBranch { name: name.clone() },
            kind: DialogKind::Confirm {
                message: format!(
                    "{}: {name}",
                    crate::i18n::tr(self.lang, crate::i18n::Msg::DeleteBranch)
                ),
                allow_permanent: true,
            },
        });
    }
    /// Confirm the deletion. `force`=false is safe (-d) / true is force (-D). On success, reloads the list.
    pub fn git_delete_branch(&mut self, name: &str, force: bool) {
        self.start_git_op(crate::app::GitOpJob {
            kind: crate::app::GitOpKind::DeleteBranch,
            root: PathBuf::new(), // filled in by start_git_op with tab.root at dispatch time
            tab_id: 0,            // likewise filled in by start_git_op
            path: PathBuf::new(),
            text: name.to_string(),
            flag: force,
            label: name.to_string(),
            count: 0,
        });
    }

    // --- Branch filtering (`/`) ----------------------------------------------
    /// Whether branch-filter input is active (while true, main captures keys as characters).
    pub fn git_branch_filtering(&self) -> bool {
        self.tab.git_branch_filtering
    }
    /// The current filter query (for the footer prompt).
    pub fn git_branch_query(&self) -> &str {
        &self.tab.git_branch_filter
    }
    /// `/`: Start filter input (query starts empty).
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn git_branch_start_filter(&mut self) {
        self.tab.git_branch_filtering = true;
        self.tab.git_branch_filter.clear();
        self.tab.git_branch_sel = 0;
    }
    pub fn git_branch_filter_push(&mut self, c: char) {
        self.tab.git_branch_filter.push(c);
        self.clamp_git_branch_sel();
    }
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn git_branch_filter_backspace(&mut self) {
        self.tab.git_branch_filter.pop();
        self.clamp_git_branch_sel();
    }
    /// Enter: Commit the input (keeps the query; navigate with j/k afterwards).
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn git_branch_filter_commit(&mut self) {
        self.tab.git_branch_filtering = false;
    }
    /// Esc: Clear the filter (return to all entries).
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn git_branch_filter_clear(&mut self) {
        self.tab.git_branch_filter.clear();
        self.tab.git_branch_filtering = false;
        self.clamp_git_branch_sel();
    }
    fn clamp_git_branch_sel(&mut self) {
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

    /// `w`: Open the linked-worktree list. Opened from the Git view, so the view closes. The cursor
    /// starts on the worktree this tab is presently inside. Flashes if the list is empty (not a
    /// repo, or the feature/config disables git — a real repository always has at least the main
    /// worktree, so this is the same "not really a repo" edge case as `open_git_branches`'s "no
    /// branches yet").
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn open_git_worktrees(&mut self) {
        // A jj workspace is not a git worktree, and in a colocated repository git's list describes
        // the repository the user is *not* working in — a detached checkout they never made, with a
        // key offering to create another. Say what konoma does not do yet instead of showing it.
        #[cfg(feature = "git")]
        if crate::vcs::detect(&self.tab.root) == crate::vcs::VcsKind::Jj {
            self.flash =
                Some(crate::i18n::tr(self.lang, crate::i18n::Msg::JjWorkspacesUnlisted).into());
            return;
        }
        let list = crate::git::worktrees(&self.tab.root);
        if list.is_empty() {
            self.flash = Some(crate::i18n::tr(self.lang, crate::i18n::Msg::NoWorktrees).into());
            return;
        }
        self.tab.git_worktree_filter.clear();
        self.tab.git_worktree_filtering = false;
        self.tab.git_worktree_sel = list.iter().position(|w| w.is_current).unwrap_or(0);
        self.tab.git_worktrees = Some(list);
        self.tab.git_view = false;
    }
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
    /// Cursor position in the display list.
    pub fn git_worktree_sel(&self) -> usize {
        self.tab.git_worktree_sel
    }
    /// Move the display-list cursor by delta (clamped to range).
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn git_worktree_move(&mut self, delta: i32) {
        self.tab.git_worktree_sel = clamp_cursor(
            self.tab.git_worktree_sel,
            delta,
            self.git_worktree_view().len(),
        );
    }
    /// The worktree selected in the display list.
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub(super) fn git_worktree_selected(&self) -> Option<crate::git::WorktreeInfo> {
        self.git_worktree_view()
            .into_iter()
            .nth(self.tab.git_worktree_sel)
    }
    /// Close the worktree list (q/Esc). Returns to the Git view since it came from there. Also
    /// resets the filter.
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn close_git_worktrees(&mut self) {
        self.reset_git_worktree_list_state();
        self.open_git_view();
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
        let main = crate::git::worktrees(&self.tab.root)
            .into_iter()
            .find(|w| w.is_main)
            .map(|w| w.path)
            .unwrap_or_else(|| self.tab.root.clone());
        let dir_name = branch.replace('/', "-");
        main.join(&self.cfg.git.worktree_dir).join(dir_name)
    }
    /// Classifies the selected row for `worktree_goto`/`worktree_goto_new_tab` (shared guard: a
    /// bare main worktree has no checkout, a prunable/missing one has nothing to switch into, and
    /// the currently-active worktree needs no switch).
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    fn worktree_switch_target(&self) -> Option<WorktreeTarget> {
        let w = self.git_worktree_selected()?;
        Some(if w.is_bare {
            WorktreeTarget::Bare
        } else if w.prunable || !w.path.is_dir() {
            WorktreeTarget::Unavailable
        } else if w.is_current {
            WorktreeTarget::AlreadyCurrent
        } else {
            WorktreeTarget::Go(w.path)
        })
    }
    /// `Enter` in the worktree list: switch this tab's root **and `open_dir`** to the selected
    /// worktree. `jump_to_dir` alone does not touch `open_dir` (it is meant for descending, where
    /// staying relative to the original launch location is correct); a worktree switch is a "you
    /// are now somewhere else entirely" move, so `open_dir` must follow too — otherwise the `y @`/`Y`
    /// AI-reference copy (relative to `open_dir`) would keep producing paths like
    /// `@../other-worktree/src/x.rs`, anchored to a location that no longer means anything once
    /// you've switched. Refuses (flash, list stays open so another row can be picked) a bare main
    /// worktree or a prunable/missing one; on the already-current worktree, flashes and just closes
    /// the list back to the changes hub (root untouched).
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn worktree_goto(&mut self) {
        match self.worktree_switch_target() {
            None => {}
            Some(WorktreeTarget::Bare) => {
                self.flash =
                    Some(crate::i18n::tr(self.lang, crate::i18n::Msg::WorktreeIsBare).into());
            }
            Some(WorktreeTarget::Unavailable) => {
                self.flash =
                    Some(crate::i18n::tr(self.lang, crate::i18n::Msg::WorktreeUnavailable).into());
            }
            Some(WorktreeTarget::AlreadyCurrent) => {
                self.flash = Some(
                    crate::i18n::tr(self.lang, crate::i18n::Msg::WorktreeAlreadyCurrent).into(),
                );
                self.close_git_worktrees();
            }
            Some(WorktreeTarget::Go(path)) => {
                self.reset_git_worktree_list_state();
                self.jump_to_dir(path.clone());
                self.tab.open_dir = path;
                self.flash = Some(format!(
                    "{}: {}",
                    crate::i18n::tr(self.lang, crate::i18n::Msg::SwitchedTo),
                    home_relative(&self.tab.root)
                ));
            }
        }
    }
    /// `Ctrl-t` in the worktree list: open the selected worktree in a new (foreground) tab, leaving
    /// this tab — and its own open worktree list — untouched (mirrors `tab_new_from_selection`'s
    /// directory branch). Same guards as `worktree_goto` (also sets `open_dir`, see there for why);
    /// on refusal or "already current" nothing else happens beyond the flash, since this action
    /// does not own the list to close.
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn worktree_goto_new_tab(&mut self) -> Result<()> {
        match self.worktree_switch_target() {
            None => {}
            Some(WorktreeTarget::Bare) => {
                self.flash =
                    Some(crate::i18n::tr(self.lang, crate::i18n::Msg::WorktreeIsBare).into());
            }
            Some(WorktreeTarget::Unavailable) => {
                self.flash =
                    Some(crate::i18n::tr(self.lang, crate::i18n::Msg::WorktreeUnavailable).into());
            }
            Some(WorktreeTarget::AlreadyCurrent) => {
                self.flash = Some(
                    crate::i18n::tr(self.lang, crate::i18n::Msg::WorktreeAlreadyCurrent).into(),
                );
            }
            Some(WorktreeTarget::Go(path)) => {
                self.tab_new()?; // fresh Tree tab at the current root, now active — this tab's
                                 // worktree list was already preserved by tab_new's save_active
                self.clear_for_root_change();
                self.tab.root = path.clone();
                self.tab.open_dir = path;
                self.tab.entries.clear();
                self.tab.selected = 0;
                self.rebuild_tree()?;
                self.save_session();
            }
        }
        Ok(())
    }

    /// Decides the base branch for `worktree_show_changes`'s "everything since base" diff for a
    /// given target worktree `w`. Reuses the graph's existing `[ui] graph_base_branches`
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
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
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
            let Some(t) = crate::git::merge_base_time(&w.path, name) else {
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

    /// `d` in the worktree list: show what the **selected** worktree has piled up relative to the
    /// base branch — the diff since `git merge-base <base> HEAD`, i.e. committed *and* uncommitted
    /// content together (see `git::diff_since`'s doc for why: an AI agent working inside a worktree
    /// very often commits along the way, so an uncommitted-only diff would go blank the moment it
    /// does and hide everything the agent actually produced).
    ///
    /// Opens as a detail overlay **on top of the worktree list**, same layering as
    /// `open_worktree_detail`/`open_git_graph_detail`: `is_git_detail()` is checked before
    /// `is_git_worktrees()` in both `surface()` and `internal_mode()`, so as long as this doesn't
    /// touch `self.tab.git_worktrees`, `q`/Esc closes only the detail and the list underneath
    /// reappears untouched, cursor and filter intact.
    ///
    /// Refuses (flash, list stays open) a bare or unavailable (prunable/missing) row — the same
    /// guard `worktree_goto` uses (`worktree_switch_target`) — but **not** `AlreadyCurrent`: viewing
    /// the diff of the worktree you're presently in is perfectly legitimate.
    ///
    /// The base is `worktree_diff_base(&w)`. When there's no resolvable base, or the since-base diff
    /// comes back empty (e.g. this worktree sits exactly at the base with nothing piled up yet),
    /// falls back to an uncommitted-only diff (`worktree_diff`) — the title makes clear which one is
    /// being shown, so an uncommitted-only view is never mistaken for a diff against the base.
    /// Flashes "no changes" if both come back empty.
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn worktree_show_changes(&mut self) {
        match self.worktree_switch_target() {
            Some(WorktreeTarget::Bare) => {
                self.flash =
                    Some(crate::i18n::tr(self.lang, crate::i18n::Msg::WorktreeIsBare).into());
                return;
            }
            Some(WorktreeTarget::Unavailable) => {
                self.flash =
                    Some(crate::i18n::tr(self.lang, crate::i18n::Msg::WorktreeUnavailable).into());
                return;
            }
            // AlreadyCurrent / Go: both have a perfectly viewable diff — proceed to fetch the row's
            // fields (branch/head/path) below rather than destructuring them out of the enum, since
            // AlreadyCurrent doesn't carry a path anyway.
            Some(WorktreeTarget::AlreadyCurrent) | Some(WorktreeTarget::Go(_)) => {}
            // Selection vanished (e.g. filter narrowed the list to zero rows) — nothing to show.
            None => return,
        }
        let Some(w) = self.git_worktree_selected() else {
            return;
        };
        let branch_label = w
            .branch
            .clone()
            .or_else(|| w.head.clone())
            .unwrap_or_else(|| "?".to_string());
        let path = home_relative(&w.path);

        let base = self.worktree_diff_base(&w);
        let since = base
            .as_deref()
            .map(|b| crate::git::diff_since(&w.path, b))
            .unwrap_or_default();

        let (lines, title) = if !since.is_empty() {
            let base_name = base.expect("non-empty `since` implies `base` resolved above");
            (since, format!("{branch_label} ← {base_name}  {path}"))
        } else {
            (
                crate::git::worktree_diff(&w.path),
                format!(
                    "{branch_label}  {path}  ({})",
                    crate::i18n::tr(self.lang, crate::i18n::Msg::UncommittedChanges)
                ),
            )
        };
        if lines.is_empty() {
            self.flash = Some(crate::i18n::tr(self.lang, crate::i18n::Msg::NoChanges).into());
            return;
        }
        self.tab.git_detail = Some(lines);
        self.tab.git_detail_meta = None; // no single commit message — this is a range/aggregate
        self.tab.git_detail_scroll = 0;
        self.tab.git_detail_hscroll = 0;
        self.tab.git_detail_title = Some(title);
    }

    // --- Worktree filtering (`/`) ----------------------------------------------
    /// Whether worktree-filter input is active (while true, main captures keys as characters).
    pub fn git_worktree_filtering(&self) -> bool {
        self.tab.git_worktree_filtering
    }
    /// The current filter query (for the footer prompt).
    pub fn git_worktree_query(&self) -> &str {
        &self.tab.git_worktree_filter
    }
    /// `/`: Start filter input (query starts empty).
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn git_worktree_start_filter(&mut self) {
        self.tab.git_worktree_filtering = true;
        self.tab.git_worktree_filter.clear();
        self.tab.git_worktree_sel = 0;
    }
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn git_worktree_filter_push(&mut self, c: char) {
        self.tab.git_worktree_filter.push(c);
        self.clamp_git_worktree_sel();
    }
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn git_worktree_filter_backspace(&mut self) {
        self.tab.git_worktree_filter.pop();
        self.clamp_git_worktree_sel();
    }
    /// Enter: Commit the input (keeps the query; navigate with j/k afterwards).
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn git_worktree_filter_commit(&mut self) {
        self.tab.git_worktree_filtering = false;
    }
    /// Esc: Clear the filter (return to all entries).
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn git_worktree_filter_clear(&mut self) {
        self.tab.git_worktree_filter.clear();
        self.tab.git_worktree_filtering = false;
        self.clamp_git_worktree_sel();
    }
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    fn clamp_git_worktree_sel(&mut self) {
        let len = self.git_worktree_view().len();
        if self.tab.git_worktree_sel >= len {
            self.tab.git_worktree_sel = len.saturating_sub(1);
        }
    }

    // --- Commit graph (`G`: SourceTree / Git Graph style) -----------------------
    /// `G`: Open the commit graph (colored output like `git log --all --graph`). Closes the Git view.
    /// Moves the cursor to the first commit row. Flashes if there are no commits.
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn open_git_graph(&mut self) {
        // Prepare the priority order (config→HEAD→recency). Reconcile it against existing branches (drop removed ones, append new ones at the end).
        self.ensure_graph_order();
        // On open (first time, or the visible set is empty), fill in the capped default selection. HEAD is always included even if the base is a different branch.
        if self.tab.git_graph_visible.is_empty() {
            self.tab.git_graph_visible = self.default_graph_visible();
        }
        // If the base is unset, use config's priority branch (the leftmost one that both exists and is currently visible) as the base.
        if self.tab.git_graph_base.is_none() && !self.cfg.ui.graph_base_branches.is_empty() {
            if let Some((oid, label)) = self.derive_base_from_order() {
                self.tab.git_graph_base = Some(oid);
                self.tab.git_graph_base_label = Some(label);
            }
        }
        let (rows, has_wt, legend, hidden) = self.build_git_graph_rows();
        if rows.iter().all(|r| r.commit.is_none()) && !has_wt {
            self.flash = Some(crate::i18n::tr(self.lang, crate::i18n::Msg::NoCommits).into());
            return;
        }
        // When there are changes, put the cursor on the worktree row (at the top); otherwise the first commit row.
        self.tab.git_graph_sel = if has_wt {
            0
        } else {
            rows.iter().position(|r| r.commit.is_some()).unwrap_or(0)
        };
        self.tab.git_graph = Some(rows);
        self.tab.git_graph_legend = legend;
        self.tab.git_graph_hidden = hidden;
        self.tab.git_view = false;
    }

    /// Reconcile `git_graph_order` (the priority order) with the current local branches.
    /// On first run, build it as **`[ui.graph_base_branches]` (those that exist, in order) → HEAD → recency order**.
    /// In later sessions, keep the reordering while dropping removed branches and appending new ones in recency order.
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    fn ensure_graph_order(&mut self) {
        let by_rec = crate::git::branches_by_recency(&self.tab.root); // (name, is_current, time), newest first
        let exists: std::collections::HashSet<&str> =
            by_rec.iter().map(|(n, _, _)| n.as_str()).collect();
        // Remove branches that vanished (keeping the reordering).
        self.tab
            .git_graph_order
            .retain(|n| exists.contains(n.as_str()));
        if self.tab.git_graph_order.is_empty() {
            // Initial construction: config priority → HEAD → recency order.
            let mut order: Vec<String> = Vec::new();
            let mut seen = std::collections::HashSet::new();
            for b in &self.cfg.ui.graph_base_branches {
                if exists.contains(b.as_str()) && seen.insert(b.clone()) {
                    order.push(b.clone());
                }
            }
            if let Some((h, _, _)) = by_rec.iter().find(|(_, c, _)| *c) {
                if seen.insert(h.clone()) {
                    order.push(h.clone());
                }
            }
            for (n, _, _) in &by_rec {
                if seen.insert(n.clone()) {
                    order.push(n.clone());
                }
            }
            self.tab.git_graph_order = order;
        } else {
            // Append new branches at the end in recency order.
            let known: std::collections::HashSet<String> =
                self.tab.git_graph_order.iter().cloned().collect();
            for (n, _, _) in &by_rec {
                if !known.contains(n) {
                    self.tab.git_graph_order.push(n.clone());
                }
            }
        }
    }

    /// Returns the tip of the first branch in priority order (`git_graph_order`) that "exists and is visible" as the base candidate.
    /// `(oid, label)`. None if there is none.
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    fn derive_base_from_order(&self) -> Option<(String, String)> {
        for name in &self.tab.git_graph_order {
            if self.tab.git_graph_visible.contains(name) {
                if let Some(oid) = crate::vcs::ref_tip(&self.tab.root, name) {
                    return Some((oid, name.clone()));
                }
            }
        }
        None
    }

    /// The default set of visible branches subject to the cap (`ui.graph_max_branches`): **from the head of priority order (config→HEAD→recency)** up to the cap.
    /// HEAD is always included. A cap of 0 means unlimited (all branches).
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    fn default_graph_visible(&self) -> std::collections::HashSet<String> {
        let cap = self.cfg.ui.graph_max_branches;
        let mut set = std::collections::HashSet::new();
        // HEAD is always included.
        for (name, is_cur, _) in crate::git::branches_by_recency(&self.tab.root) {
            if is_cur {
                set.insert(name);
            }
        }
        // From the head of the priority order up to the cap (0=unlimited).
        for name in &self.tab.git_graph_order {
            if cap != 0 && set.len() >= cap {
                break;
            }
            set.insert(name.clone());
        }
        set
    }

    /// Build the graph rows and legend reflecting the base (`git_graph_base`) and visible set (`git_graph_visible`).
    /// `(rows, whether a working-tree row is present, legend)`. If the visible set covers all branches, fetch with `--all`.
    /// The uncommitted working-tree row (`●`) is placed **at the head of HEAD's lane** inside `graph_with_base`.
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    fn build_git_graph_rows(
        &self,
    ) -> (
        Vec<crate::git::GraphRow>,
        bool,
        Vec<crate::git::LegendEntry>,
        usize,
    ) {
        let total = crate::git::branches_by_recency(&self.tab.root).len();
        let visible: Vec<String> = self.tab.git_graph_visible.iter().cloned().collect();
        // Empty or covers all branches → --all (None). Otherwise, only the specified branches.
        let use_all = visible.is_empty() || visible.len() >= total;
        let refs = if use_all {
            None
        } else {
            Some(visible.as_slice())
        };
        let mut rows = crate::vcs::graph(
            self.tab.git_graph_all,
            &self.tab.root,
            self.tab.git_graph_base.as_deref(),
            self.lang,
            refs,
        );
        // Also strip hidden branches from a row's decoration (refs) (keeping it consistent with the
        // lane's visibility). Prevents the label/legend from bloating with hidden branch names that
        // happen to sit on the same commit.
        if !use_all {
            let allowed = &self.tab.git_graph_visible;
            for r in &mut rows {
                if r.refs.is_empty() {
                    continue;
                }
                r.refs = r
                    .refs
                    .split(',')
                    .map(|t| t.trim())
                    .filter(|t| {
                        t.starts_with("HEAD -> ")
                            || *t == "HEAD"
                            || t.starts_with("tag:")
                            || allowed.contains(*t)
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
            }
        }
        let has_wt = rows.iter().any(|r| r.worktree);
        let mut legend = crate::git::legend_from_rows(
            &rows,
            &self.tab.root,
            self.tab.git_graph_base_label.as_deref(),
        );
        // Sort the legend by **priority order (`git_graph_order`)** (config priority→HEAD→recency). Anything out of order goes to the end.
        legend.sort_by_key(|e| {
            self.tab
                .git_graph_order
                .iter()
                .position(|n| n == &e.name)
                .unwrap_or(usize::MAX)
        });
        // hidden count = total local branches − currently visible branch count.
        let shown = if use_all { total } else { visible.len() };
        let hidden = total.saturating_sub(shown);
        (rows, has_wt, legend, hidden)
    }

    /// Phase 2: Set the selected commit as the **base branch** and pin its first-parent chain to lane0 on the left.
    /// Does nothing on non-commit rows (working tree / connector). Restores the cursor to the same commit after redraw.
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn git_graph_set_base(&mut self) {
        let Some(row) = self.git_graph_selected_row() else {
            return;
        };
        let Some(id) = row.commit.clone() else {
            self.flash =
                Some(crate::i18n::tr(self.lang, crate::i18n::Msg::GraphBaseNeedsCommit).into());
            return;
        };
        let label = base_label_from(&row.refs, &row.short);
        self.tab.git_graph_base = Some(id);
        self.tab.git_graph_base_label = Some(label.clone());
        self.rebuild_git_graph_keep_sel();
        self.flash = Some(format!(
            "{}{label}",
            crate::i18n::tr(self.lang, crate::i18n::Msg::GraphBaseSet)
        ));
    }

    /// `R`: ask jj to take a snapshot.
    ///
    /// konoma tracks changes itself and never writes, which leaves one thing it cannot see: a file
    /// whose contents changed while its modification time did not. This is the way out of that, and
    /// the only write konoma makes to a jj repository — so it says so first, unless
    /// `[ui] confirm_jj_sync` is off.
    #[cfg(feature = "git")]
    pub fn jj_start_sync(&mut self) {
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

    /// `a`: widen the graph to every revision, or narrow it back.
    ///
    /// Only jj keeps a narrower default — it rewrites commits as you work, and every predecessor
    /// stays reachable, so its own range hides what no longer exists to you. git's graph is already
    /// everything, so there is nothing to widen.
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn git_graph_toggle_all(&mut self) {
        if crate::vcs::detect(&self.tab.root) == crate::vcs::VcsKind::Git {
            self.flash = Some(crate::i18n::tr(self.lang, crate::i18n::Msg::GraphAlreadyAll).into());
            return;
        }
        self.tab.git_graph_all = !self.tab.git_graph_all;
        self.rebuild_git_graph_keep_sel();
        self.flash = Some(
            crate::i18n::tr(
                self.lang,
                if self.tab.git_graph_all {
                    crate::i18n::Msg::GraphShowingAll
                } else {
                    crate::i18n::Msg::GraphShowingDefault
                },
            )
            .into(),
        );
    }

    /// Phase 2: Clear the base pin and return to the default display.
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn git_graph_clear_base(&mut self) {
        if self.tab.git_graph_base.is_none() {
            return;
        }
        self.tab.git_graph_base = None;
        self.tab.git_graph_base_label = None;
        self.rebuild_git_graph_keep_sel();
        self.flash = Some(crate::i18n::tr(self.lang, crate::i18n::Msg::GraphBaseCleared).into());
    }

    /// Rebuild the graph after a base change and restore the cursor to the same commit row (or the first commit/worktree if absent).
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    fn rebuild_git_graph_keep_sel(&mut self) {
        if self.tab.git_graph.is_none() {
            return;
        }
        let cur = self.git_graph_selected_row().and_then(|r| r.commit.clone());
        let (rows, has_wt, legend, hidden) = self.build_git_graph_rows();
        self.tab.git_graph_sel = cur
            .as_deref()
            .and_then(|id| rows.iter().position(|r| r.commit.as_deref() == Some(id)))
            .or(if has_wt { Some(0) } else { None })
            .or_else(|| rows.iter().position(|r| r.commit.is_some()))
            .unwrap_or(0);
        self.tab.git_graph = Some(rows);
        self.tab.git_graph_legend = legend;
        self.tab.git_graph_hidden = hidden;
    }

    /// The base label (for the title `base: …`). None if unset.
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn git_graph_base_label(&self) -> Option<&str> {
        self.tab.git_graph_base_label.as_deref()
    }

    /// The graph legend (branch ⇄ lane color, for rendering). HEAD first, base next, then in order of appearance.
    pub fn git_graph_legend(&self) -> &[crate::git::LegendEntry] {
        &self.tab.git_graph_legend
    }
    /// The number of branches hidden by the cap/toggle (for the legend's `(+K hidden)`).
    pub fn git_graph_hidden_count(&self) -> usize {
        self.tab.git_graph_hidden
    }

    // --- Branch display panel (`b`: visibility toggle + priority-order reordering for many branches) ----------
    /// Open the panel. Initializes the tentative selection from the current visible set and moves the cursor to the top (= head of priority order).
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn git_graph_open_picker(&mut self) {
        if self.tab.git_graph.is_none() {
            return;
        }
        self.ensure_graph_order();
        // When the visible set is empty (= treated as "show all"), show all branches as already selected.
        if self.tab.git_graph_visible.is_empty() {
            self.tab.git_graph_visible = self.tab.git_graph_order.iter().cloned().collect();
        }
        self.git_graph_picker_set = self.tab.git_graph_visible.clone();
        self.git_graph_picker_sel = 0;
        self.git_graph_reordered = false;
        self.git_graph_picker = true;
    }
    /// Whether the panel is showing.
    pub fn is_git_graph_picker(&self) -> bool {
        self.git_graph_picker
    }
    /// Cursor position in the panel.
    pub fn git_graph_picker_sel(&self) -> usize {
        self.git_graph_picker_sel
    }
    /// The current branch (HEAD) name.
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    fn head_branch_name(&self) -> Option<String> {
        crate::git::branches_by_recency(&self.tab.root)
            .into_iter()
            .find(|(_, c, _)| *c)
            .map(|(n, _, _)| n)
    }
    /// Panel rows: `(branch, is_current, tentatively visible ON)`. Ordered by **priority order (`git_graph_order`)**.
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn git_graph_picker_items(&self) -> Vec<(String, bool, bool)> {
        let head = self.head_branch_name();
        self.tab
            .git_graph_order
            .iter()
            .map(|name| {
                let is_cur = head.as_deref() == Some(name.as_str());
                let on = self.git_graph_picker_set.contains(name);
                (name.clone(), is_cur, on)
            })
            .collect()
    }
    /// Move the cursor row's branch up/down one in priority order (`J`=down / `K`=up). For this session only.
    /// The cursor follows the move, and on Enter (apply) the base is re-derived to the new head branch.
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn git_graph_picker_reorder(&mut self, delta: i32) {
        let n = self.tab.git_graph_order.len();
        if n < 2 {
            return;
        }
        let i = self.git_graph_picker_sel;
        let j = i as i32 + delta;
        if j < 0 || j >= n as i32 {
            return;
        }
        let j = j as usize;
        self.tab.git_graph_order.swap(i, j);
        self.git_graph_picker_sel = j;
        self.git_graph_reordered = true;
    }
    /// Cursor movement within the panel (row = branch, same as the commit graph).
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn git_graph_picker_move(&mut self, delta: i32) {
        let n = self.git_graph_picker_items().len();
        if n == 0 {
            return;
        }
        let cur = self.git_graph_picker_sel as i32;
        self.git_graph_picker_sel = (cur + delta).clamp(0, n as i32 - 1) as usize;
    }
    /// Jump to the top/bottom of the panel.
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn git_graph_picker_jump(&mut self, top: bool) {
        let n = self.git_graph_picker_items().len();
        self.git_graph_picker_sel = if top { 0 } else { n.saturating_sub(1) };
    }
    /// Toggle the cursor row's branch visibility ON/OFF. **HEAD is always ON (cannot be toggled)**.
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn git_graph_picker_toggle(&mut self) {
        let items = self.git_graph_picker_items();
        let Some((name, is_cur, _)) = items.get(self.git_graph_picker_sel).cloned() else {
            return;
        };
        if is_cur {
            self.flash =
                Some(crate::i18n::tr(self.lang, crate::i18n::Msg::GraphPickerHeadLocked).into());
            return; // HEAD cannot be turned off.
        }
        if self.git_graph_picker_set.contains(&name) {
            self.git_graph_picker_set.remove(&name);
        } else {
            self.git_graph_picker_set.insert(name);
        }
    }
    /// Turn visibility ON for all branches.
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn git_graph_picker_all(&mut self) {
        self.git_graph_picker_set = self
            .git_graph_picker_items()
            .into_iter()
            .map(|(n, _, _)| n)
            .collect();
    }
    /// Show only the current branch (plus the base).
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn git_graph_picker_current_only(&mut self) {
        let mut set = std::collections::HashSet::new();
        for (name, is_cur, _) in self.git_graph_picker_items() {
            if is_cur {
                set.insert(name);
            }
        }
        if let Some(b) = &self.tab.git_graph_base_label {
            set.insert(b.clone());
        }
        self.git_graph_picker_set = set;
    }
    /// Commit the tentative selection, rebuild the graph, and close the panel.
    /// If reordered with `J`/`K`, **re-derive the base to the head (visible) branch in priority order** (point 3+5).
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn git_graph_picker_apply(&mut self) {
        self.tab.git_graph_visible = self.git_graph_picker_set.clone();
        if self.git_graph_reordered {
            match self.derive_base_from_order() {
                Some((oid, label)) => {
                    self.tab.git_graph_base = Some(oid);
                    self.tab.git_graph_base_label = Some(label);
                }
                None => {
                    self.tab.git_graph_base = None;
                    self.tab.git_graph_base_label = None;
                }
            }
        }
        self.git_graph_reordered = false;
        self.git_graph_picker = false;
        self.rebuild_git_graph_keep_sel();
    }
    /// Discard changes and close the panel (the visible set is left as is).
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn git_graph_picker_cancel(&mut self) {
        self.git_graph_picker = false;
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
    /// All rows of the graph (for rendering). Empty when hidden.
    pub fn git_graph_rows(&self) -> &[crate::git::GraphRow] {
        self.tab.git_graph.as_deref().unwrap_or(&[])
    }
    /// Cursor position in the graph (row index).
    pub fn git_graph_sel(&self) -> usize {
        self.tab.git_graph_sel
    }
    /// Move the cursor by delta over **commit rows only** (skipping connector rows).
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn git_graph_move(&mut self, delta: i32) {
        let Some(rows) = &self.tab.git_graph else {
            return;
        };
        let commits: Vec<usize> = rows
            .iter()
            .enumerate()
            .filter(|(_, r)| r.commit.is_some() || r.worktree)
            .map(|(i, _)| i)
            .collect();
        if commits.is_empty() {
            return;
        }
        let cur = commits
            .iter()
            .position(|&i| i == self.tab.git_graph_sel)
            .unwrap_or(0);
        let next = clamp_cursor(cur, delta, commits.len());
        self.tab.git_graph_sel = commits[next];
    }
    /// Close the graph (q/Esc). Returns to the Git view.
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn close_git_graph(&mut self) {
        self.tab.git_graph = None;
        self.tab.git_graph_sel = 0;
        self.open_git_view();
    }
    /// The graph's cursor row (for render/title).
    pub fn git_graph_selected_row(&self) -> Option<&crate::git::GraphRow> {
        self.tab
            .git_graph
            .as_ref()
            .and_then(|rows| rows.get(self.tab.git_graph_sel))
    }
    /// `Enter`: Open the detail of the selected row. Commit row → commit_diff / working-tree row → worktree_diff.
    /// The graph is kept behind (closing the detail returns to it).
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn open_git_graph_detail(&mut self) {
        let Some(row) = self.git_graph_selected_row().cloned() else {
            return;
        };
        let (lines, meta) = if row.worktree {
            (crate::git::worktree_diff(&self.tab.root), None)
        } else if let Some(id) = row.commit {
            (
                crate::vcs::commit_diff(&self.tab.root, &id),
                crate::vcs::commit_meta(&self.tab.root, &id),
            )
        } else {
            return;
        };
        self.tab.git_detail = Some(lines);
        self.tab.git_detail_meta = meta; // for a commit, show the full message at the top
        self.tab.git_detail_scroll = 0;
        self.tab.git_detail_hscroll = 0;
        self.tab.git_detail_title = None; // the title is derived from the graph's selected row (worktree/commit)
    }

    /// `Enter`: Read the selected commit's detail (commit_diff) and show it as a full-screen diff.
    /// Overlaid while the log is kept behind (Esc/q closes only the detail and returns to the log).
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn open_git_commit_detail(&mut self) {
        let Some(id) = self.git_log_selected_id() else {
            return;
        };
        let lines = crate::vcs::commit_diff(&self.tab.root, &id);
        self.tab.git_detail = Some(lines);
        self.tab.git_detail_meta = crate::vcs::commit_meta(&self.tab.root, &id);
        self.tab.git_detail_scroll = 0;
        self.tab.git_detail_hscroll = 0;
        self.tab.git_detail_title = None;
    }

    /// `D` in the Git view: Open a diff that **aggregates all working-tree changes** (same as the graph's Uncommitted).
    /// Does nothing if there are no changes. Overlaid as a detail view (git_detail); `s` toggles unified/split, q/Esc returns to the Git view.
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn open_worktree_detail(&mut self) {
        let lines = crate::git::worktree_diff(&self.tab.root);
        if lines.is_empty() {
            self.flash = Some(crate::i18n::tr(self.lang, crate::i18n::Msg::NoChanges).into());
            return;
        }
        self.tab.git_detail = Some(lines);
        self.tab.git_detail_meta = None; // no message since it's uncommitted
        self.tab.git_detail_scroll = 0;
        self.tab.git_detail_hscroll = 0;
        self.tab.git_detail_title =
            Some(crate::i18n::tr(self.lang, crate::i18n::Msg::UncommittedChanges).into());
    }

    /// Title override for the detail (git_detail) (e.g. when opening all working-tree changes from the git view).
    pub fn git_detail_title(&self) -> Option<&str> {
        self.tab.git_detail_title.as_deref()
    }

    /// Whether the commit detail is showing (for render/key branching).
    pub fn is_git_detail(&self) -> bool {
        self.tab.git_detail.is_some()
    }

    /// Meta info shown at the top of the commit detail (full message, etc.). None for worktree diffs, etc.
    pub fn git_detail_meta(&self) -> Option<&crate::git::CommitMeta> {
        self.tab.git_detail_meta.as_ref()
    }

    /// Returns the DiffLine sequence of the commit detail (for rendering). Empty when hidden.
    pub fn git_detail_lines(&self) -> &[crate::git::DiffLine] {
        self.tab.git_detail.as_deref().unwrap_or(&[])
    }

    /// Vertically scroll the commit detail. The bottom is clamped by the viewport the renderer updates.
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn git_detail_scroll_by(&mut self, delta: i32) {
        // Clamped by the total line count (commit message + diff) that the render side updates.
        // Using only the diff line count would leave no room to scroll past the header, hiding the tail.
        let total = self.tab.git_detail_total;
        let max = total.saturating_sub(self.tab.git_detail_viewport as usize) as i32;
        let next = (self.tab.git_detail_scroll as i32)
            .saturating_add(delta)
            .clamp(0, max.max(0));
        self.tab.git_detail_scroll = next as u16;
    }

    /// Move the commit detail to the top/bottom (g/G).
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn git_detail_scroll_to(&mut self, end: bool) {
        if end {
            self.git_detail_scroll_by(i32::MAX);
        } else {
            self.tab.git_detail_scroll = 0;
        }
    }

    /// The commit detail's current scroll amount (for rendering).
    pub fn git_detail_scroll(&self) -> u16 {
        self.tab.git_detail_scroll
    }

    /// Horizontally scroll the commit detail (h/l). The end is clamped by the renderer's longest line width.
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn git_detail_hscroll_by(&mut self, delta: i32) {
        let next = (self.tab.git_detail_hscroll as i32 + delta).max(0);
        self.tab.git_detail_hscroll = next as u16;
    }
    /// Move horizontal scroll to the line start (left edge) / line end (right edge) (0/$). The line end is clamped to the longest line width by the renderer.
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn git_detail_hscroll_home(&mut self) {
        self.tab.git_detail_hscroll = 0;
    }
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn git_detail_hscroll_end(&mut self) {
        self.tab.git_detail_hscroll = u16::MAX;
    }
    /// The commit detail's current horizontal scroll amount (for rendering).
    pub fn git_detail_hscroll(&self) -> u16 {
        self.tab.git_detail_hscroll
    }
    /// Clamp the horizontal scroll amount to the maximum `max` (the renderer calls this with the longest line width).
    pub fn clamp_git_detail_hscroll(&mut self, max: u16) {
        self.tab.git_detail_hscroll = self.tab.git_detail_hscroll.min(max);
    }

    /// The renderer records the number of displayable rows (for g/G and scroll clamping).
    pub fn set_git_detail_viewport(&mut self, h: u16) {
        self.tab.git_detail_viewport = h;
    }

    /// The renderer records the total line count (commit message + diff) (for clamping the scroll limit).
    pub fn set_git_detail_total(&mut self, n: usize) {
        self.tab.git_detail_total = n;
    }

    /// Close the commit detail (q/Esc). Returns to the log behind it.
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn close_git_detail(&mut self) {
        self.tab.git_detail = None;
        self.tab.git_detail_meta = None;
        self.tab.git_detail_title = None;
        self.tab.git_detail_scroll = 0;
        self.tab.git_detail_hscroll = 0;
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
    use crate::test_support::unique_tmp;

    #[cfg(feature = "git")]
    fn init_repo(dir: &std::path::Path) {
        let repo = git2::Repository::init(dir).unwrap();
        let mut cfg = repo.config().unwrap();
        cfg.set_str("user.name", "Test").unwrap();
        cfg.set_str("user.email", "test@example.com").unwrap();
        cfg.set_str("commit.gpgsign", "false").ok();
    }

    #[cfg(feature = "git")]
    fn sh(cwd: &std::path::Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .current_dir(cwd)
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {out:?}");
    }

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
    fn sandbox_with_worktree(name: &str) -> (PathBuf, String, PathBuf) {
        let dir = unique_tmp(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        init_repo(&dir);
        std::fs::write(dir.join("a.txt"), b"one\n").unwrap();
        sh(&dir, &["add", "-A"]);
        sh(&dir, &["commit", "-q", "-m", "init"]);
        let root = dir.canonicalize().unwrap();
        let base = crate::git::branch(&root).expect("sanity: has a branch after the commit");
        let linked = unique_tmp(&format!("{name}_linked"));
        let _ = std::fs::remove_dir_all(&linked);
        sh(
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
        let linked = linked.canonicalize().unwrap();
        (root, base, linked)
    }

    /// Fetches the linked worktree's row (added by `sandbox_with_worktree`, checked out on
    /// `branch`) from `tab.git_worktrees`, populated by a prior `open_git_worktrees()` call. All
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
            .expect("git_worktrees populated by open_git_worktrees()")
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
        let mut app = App::new(root.clone(), cfg).unwrap();
        app.open_git_worktrees(); // populates `tab.git_worktrees` (the fallback path's source)
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
        let mut app = App::new(root.clone(), cfg).unwrap();
        app.open_git_worktrees();
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
        let mut app = App::new(root.clone(), cfg).unwrap();
        app.open_git_worktrees();
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
        init_repo(&dir);
        std::fs::write(dir.join("main.txt"), b"main\n").unwrap();
        sh(&dir, &["add", "-A"]);
        commit_at(&dir, "on main", 1_700_000_000); // deterministic: 20 days apart from develop's commit
        sh(&dir, &["branch", "-m", "main"]); // name the initial branch deterministically

        sh(&dir, &["checkout", "-q", "-b", "develop"]);
        std::fs::write(dir.join("develop.txt"), b"develop\n").unwrap();
        sh(&dir, &["add", "-A"]);
        commit_at(&dir, "on develop", 1_720_000_000); // newer than main's commit — develop's tip
                                                      // is now the merge-base any worktree branched off it will have with `develop`.

        let root = dir.canonicalize().unwrap();
        let linked = unique_tmp("konoma_gitview_diffbase_recency_linked");
        let _ = std::fs::remove_dir_all(&linked);
        sh(
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
        sh(&linked, &["add", "-A"]);
        commit_at(&linked, "agent work", 1_730_000_000);

        let mut cfg = Config::default();
        cfg.ui.graph_base_branches = vec!["main".to_string(), "develop".to_string()];
        let mut app = App::new(root.clone(), cfg).unwrap();
        app.open_git_worktrees();
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
        let mut app = App::new(root.clone(), cfg).unwrap();
        app.open_git_worktrees();
        let main_row = app
            .git_worktree_selected()
            .expect("cursor defaults to the main worktree row (is_current)");
        assert!(main_row.is_main, "前提: カーソルはメインワークツリー行");
        assert_eq!(
            app.worktree_diff_base(&main_row),
            None,
            "唯一の候補＝自分自身のブランチは除外されるので None"
        );
        std::fs::remove_dir_all(&root).ok();
    }
}
