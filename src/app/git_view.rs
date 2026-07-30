//! Git views: changes hub, branches, commit graph, and commit detail — methods on `App`.

use super::*;

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
        if crate::git::branch(&self.tab.root).is_none() {
            // Not a repo (or the feature is disabled). Ignore safely and notify via flash.
            self.flash = Some(crate::i18n::tr(self.lang, crate::i18n::Msg::NotAGitRepo).into());
            return;
        }
        self.tab.git_view_entries = crate::git::changed_files(&self.tab.root);
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
        self.tab.git_view_entries = crate::git::changed_files(&self.tab.root);
        if self.tab.git_view_sel >= self.tab.git_view_entries.len() {
            self.tab.git_view_sel = self.tab.git_view_entries.len().saturating_sub(1);
        }
        self.git_status_for = None; // also re-fetch the tree's git status next time
        self.git_status_dirty = true; // status changed due to a git operation = invalidate the workdir cache
    }
    /// `s` in the Git view = stage. Flashes success/failure and rebuilds the list.
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn git_view_stage(&mut self) {
        let Some(path) = self.git_view_selected() else {
            return;
        };
        match crate::git::stage(&self.tab.root, &path) {
            Ok(()) => {
                self.git_view_reload();
                self.flash = Some(format!(
                    "{}: {}",
                    crate::i18n::tr(self.lang, crate::i18n::Msg::Staged),
                    self.format_path(&path)
                ));
            }
            Err(e) => {
                self.flash = Some(format!(
                    "{}: {e}",
                    crate::i18n::tr(self.lang, crate::i18n::Msg::Failed)
                ))
            }
        }
    }
    /// `u` in the Git view = unstage.
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn git_view_unstage(&mut self) {
        let Some(path) = self.git_view_selected() else {
            return;
        };
        match crate::git::unstage(&self.tab.root, &path) {
            Ok(()) => {
                self.git_view_reload();
                self.flash = Some(format!(
                    "{}: {}",
                    crate::i18n::tr(self.lang, crate::i18n::Msg::Unstaged),
                    self.format_path(&path)
                ));
            }
            Err(e) => {
                self.flash = Some(format!(
                    "{}: {e}",
                    crate::i18n::tr(self.lang, crate::i18n::Msg::Failed)
                ))
            }
        }
    }
    /// `S` in the Git view = stage all (git add -A). Does nothing if there are no changes.
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn git_view_stage_all(&mut self) {
        if self.tab.git_view_entries.is_empty() {
            self.flash = Some(crate::i18n::tr(self.lang, crate::i18n::Msg::NoChanges).into());
            return;
        }
        match crate::git::stage_all(&self.tab.root) {
            Ok(()) => {
                let n = self.tab.git_view_entries.len();
                self.git_view_reload();
                self.flash = Some(format!(
                    "{} ({n})",
                    crate::i18n::tr(self.lang, crate::i18n::Msg::StagedAll)
                ));
            }
            Err(e) => {
                self.flash = Some(format!(
                    "{}: {e}",
                    crate::i18n::tr(self.lang, crate::i18n::Msg::Failed)
                ))
            }
        }
    }
    /// `U` in the Git view = unstage all (git reset HEAD). Does nothing if nothing is staged.
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn git_view_unstage_all(&mut self) {
        if !self.tab.git_view_entries.iter().any(|e| e.staged) {
            self.flash = Some(crate::i18n::tr(self.lang, crate::i18n::Msg::NothingStaged).into());
            return;
        }
        match crate::git::unstage_all(&self.tab.root) {
            Ok(()) => {
                self.git_view_reload();
                self.flash = Some(crate::i18n::tr(self.lang, crate::i18n::Msg::UnstagedAll).into());
            }
            Err(e) => {
                self.flash = Some(format!(
                    "{}: {e}",
                    crate::i18n::tr(self.lang, crate::i18n::Msg::Failed)
                ))
            }
        }
    }
    /// `x` in the Git view = discard. Opens a confirmation dialog (on confirm: git::discard then reload).
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn git_view_start_discard(&mut self) {
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
        let commits = crate::git::log(&self.tab.root, 200);
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
        let list = crate::git::branches(&self.tab.root);
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
        self.tab.git_branches = Some(crate::git::branches(&self.tab.root));
        self.clamp_git_branch_sel();
    }
    /// `Enter`: Switch to the selected branch. If uncommitted changes conflict, git refuses (Err is flashed). On success, returns to the Git view.
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn checkout_selected_branch(&mut self) -> Result<()> {
        let Some(b) = self.git_branch_selected() else {
            return Ok(());
        };
        let name = b.name;
        match crate::git::checkout(&self.tab.root, &name) {
            Ok(()) => {
                self.refresh()?;
                self.ensure_git_status_now(); // update branch name/status immediately (correct even before the next draw)
                self.close_git_branches();
                self.flash = Some(format!(
                    "{}: {name}",
                    crate::i18n::tr(self.lang, crate::i18n::Msg::SwitchedTo)
                ));
            }
            Err(e) => self.flash = Some(format!("{e}")),
        }
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
        match crate::git::delete_branch(&self.tab.root, name, force) {
            Ok(()) => {
                self.git_branches_reload();
                self.flash = Some(format!(
                    "{}: {name}",
                    crate::i18n::tr(self.lang, crate::i18n::Msg::DeletedBranch)
                ));
            }
            Err(e) => self.flash = Some(format!("{e}")),
        }
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
                if let Some(oid) = crate::git::branch_tip(&self.tab.root, name) {
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
        let mut rows = crate::git::graph_with_base(
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
    /// **Whether Git is the main mode** (one of the changes hub / log / graph / branches / detail / diff).
    /// Used to set the outer (main) mode chip to `GIT`. The git views overlay tree/preview.
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn in_git_view(&self) -> bool {
        self.is_git_view()
            || self.is_git_log()
            || self.is_git_graph()
            || self.is_git_branches()
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
                crate::git::commit_diff(&self.tab.root, &id),
                crate::git::commit_meta(&self.tab.root, &id),
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
        let lines = crate::git::commit_diff(&self.tab.root, &id);
        self.tab.git_detail = Some(lines);
        self.tab.git_detail_meta = crate::git::commit_meta(&self.tab.root, &id);
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
