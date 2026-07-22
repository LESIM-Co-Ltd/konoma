//! Git views: changes hub, branches, commit graph, and commit detail — methods on `App`.

use super::*;

impl App {
    // --- Git ビュー(変更ハブ・既定キー `o`・keymap で変更可) -----------------
    /// `o`: Open the Git view. Reads the change list for the current root and moves the cursor to the top.
    /// When the feature is disabled or this is not a repo, do nothing (`is_git_view` stays false = no-op).
    pub fn open_git_view(&mut self) {
        if crate::git::branch(&self.root).is_none() {
            // repo でない(または feature 無効)。安全に無視し、flash で知らせる。
            self.flash = Some(crate::i18n::tr(self.lang, crate::i18n::Msg::NotAGitRepo).into());
            return;
        }
        self.git_view_entries = crate::git::changed_files(&self.root);
        self.git_view_sel = 0;
        self.git_view = true;
    }
    /// Close the Git view (q/Esc).
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn close_git_view(&mut self) {
        self.git_view = false;
    }
    /// Whether the Git view is showing. False when the feature is disabled or this is not a repo (open_git_view never sets it).
    pub fn is_git_view(&self) -> bool {
        self.git_view
    }
    /// The change list (for rendering).
    pub fn git_view_entries(&self) -> &[crate::git::ChangeEntry] {
        &self.git_view_entries
    }
    /// Cursor position in the Git view (used for render scrolling/inversion).
    pub fn git_view_sel(&self) -> usize {
        self.git_view_sel
    }
    /// Move the cursor by delta rows, clamped to [0, last].
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn git_view_move(&mut self, delta: i32) {
        self.git_view_sel = clamp_cursor(self.git_view_sel, delta, self.git_view_entries.len());
    }
    /// Absolute path of the changed file at the cursor.
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn git_view_selected(&self) -> Option<PathBuf> {
        self.git_view_entries
            .get(self.git_view_sel)
            .map(|e| e.path.clone())
    }
    /// Rebuild the list after a write operation and clamp the cursor. Also invalidates and refetches git status.
    pub fn git_view_reload(&mut self) {
        self.git_view_entries = crate::git::changed_files(&self.root);
        if self.git_view_sel >= self.git_view_entries.len() {
            self.git_view_sel = self.git_view_entries.len().saturating_sub(1);
        }
        self.git_status_for = None; // ツリーの git status も次回再取得
        self.git_status_dirty = true; // git 操作で status が変わった=workdir キャッシュを無効化
    }
    /// `s` in the Git view = stage. Flashes success/failure and rebuilds the list.
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn git_view_stage(&mut self) {
        let Some(path) = self.git_view_selected() else {
            return;
        };
        match crate::git::stage(&self.root, &path) {
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
        match crate::git::unstage(&self.root, &path) {
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
        if self.git_view_entries.is_empty() {
            self.flash = Some(crate::i18n::tr(self.lang, crate::i18n::Msg::NoChanges).into());
            return;
        }
        match crate::git::stage_all(&self.root) {
            Ok(()) => {
                let n = self.git_view_entries.len();
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
        if !self.git_view_entries.iter().any(|e| e.staged) {
            self.flash = Some(crate::i18n::tr(self.lang, crate::i18n::Msg::NothingStaged).into());
            return;
        }
        match crate::git::unstage_all(&self.root) {
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

    // --- Git ビューのスタブ (phase 4/5 が置き換える) -------------------------
    /// `Enter`/`l`: Open the selected file's diff in the GitDiff preview. Closes the Git view and
    /// remembers where it came from (came_from_git_view) so Esc/q can return to the Git view.
    pub fn open_git_diff(&mut self, path: &Path) {
        self.preview_path = Some(path.to_path_buf());
        self.preview_kind = Some(PreviewKind::GitDiff(path.to_path_buf()));
        // diff プレビューは独自描画(window/image/md を使わない)。関連状態をリセット。
        self.preview_scroll = 0;
        self.preview_hscroll = 0;
        self.preview_byte_top = 0;
        self.preview_top_line = 0;
        self.preview_win = None;
        self.win_cache = None;
        self.preview_total_lines = None;
        self.md_cache = None;
        self.diff_cache = None; // 別ファイルの diff を開く: 生 diff キャッシュを無効化
        self.md_items.clear();
        self.focused_item = None;
        self.hl_pending = false;
        self.hl_warming = false;
        self.came_from_git_view = self.git_view;
        self.git_view = false;
        // 既定はフルの git 変更スコープ(フォロー由来のときだけ呼び出し側が true に上書きする)。
        self.diff_follow_scope = false;
        self.mode = Mode::Preview;
    }

    /// Whether a GitDiff preview is currently showing (for render/key branching).
    pub fn is_git_diff_preview(&self) -> bool {
        matches!(self.preview_kind, Some(PreviewKind::GitDiff(_)))
    }

    /// Returns the diff lines of the GitDiff preview (for rendering). Empty if not applicable.
    /// Cached per path to avoid calling `file_diff` (a git invocation) every frame.
    /// When the working tree changes, `refresh()` drops the cache, so j/k and horizontal scroll while
    /// displayed don't re-run git; only external edits (FS events) refetch. The return value is cloned every frame for rendering.
    pub fn git_diff_lines(&mut self) -> Vec<crate::git::DiffLine> {
        let Some(PreviewKind::GitDiff(p)) = self.preview_kind.clone() else {
            return Vec::new();
        };
        let hit = matches!(&self.diff_cache, Some(c) if c.path == p);
        if !hit {
            let lines = crate::git::file_diff(&self.root, &p);
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
        self.refresh_git_if_needed(); // 念のため最新の git status に
        let Some(e) = self.entries.get(self.selected) else {
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
        self.open_git_diff(&path); // came_from_git_view=false(ツリー由来)→ q でツリーへ
    }

    /// Whether to lay the diff side by side at display width `width` (resolves Auto).
    pub fn diff_is_split(&self, width: u16) -> bool {
        self.diff_layout.is_split(width)
    }
    /// Cycle the diff layout unified→split→Auto (`s`). Called from both the GitDiff preview and the detail view.
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn cycle_diff_layout(&mut self) {
        self.diff_layout = self.diff_layout.next();
        self.preview_hscroll = 0;
        self.git_detail_hscroll = 0; // 並び替えで横位置はリセット(意味が変わるため)
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
        let return_to_git = self.came_from_git_view;
        self.back_to_tree();
        if return_to_git {
            self.came_from_git_view = false;
            self.open_git_view();
        }
    }

    /// `x` in the GitDiff preview = discard all changes to the displayed file. Opens a confirmation dialog
    /// (on confirm: git::discard then return to the Git view). Uses the same flow as discard from the Git view.
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn git_diff_start_discard(&mut self) {
        let Some(PreviewKind::GitDiff(path)) = self.preview_kind.clone() else {
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
        // 破棄後は Git ビューへ戻したいので、戻り元フラグを立てておく。
        self.came_from_git_view = true;
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
        let commits = crate::git::log(&self.root, 200);
        if commits.is_empty() {
            self.flash = Some(crate::i18n::tr(self.lang, crate::i18n::Msg::NoCommits).into());
            return;
        }
        self.git_log = Some(commits);
        self.git_log_sel = 0;
        // log は Git ビューの上位ビュー。開いている Git ビューは閉じておく(戻り先は close で復元)。
        self.git_view = false;
    }

    /// Whether the git log is showing (for render/key branching).
    pub fn is_git_log(&self) -> bool {
        self.git_log.is_some()
    }

    /// Returns all commits in the git log (for rendering). Empty when hidden.
    pub fn git_log_entries(&self) -> &[crate::git::CommitInfo] {
        self.git_log.as_deref().unwrap_or(&[])
    }

    /// Cursor position in the git log.
    pub fn git_log_sel(&self) -> usize {
        self.git_log_sel
    }

    /// Move the git log cursor by delta (clamped to range).
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn git_log_move(&mut self, delta: i32) {
        self.git_log_sel = clamp_cursor(self.git_log_sel, delta, self.git_log_entries().len());
    }

    /// Returns the id of the selected commit.
    pub fn git_log_selected_id(&self) -> Option<String> {
        self.git_log
            .as_ref()?
            .get(self.git_log_sel)
            .map(|c| c.id.clone())
    }

    /// Close the git log (q/Esc). Returns to the Git view since it came from there.
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn close_git_log(&mut self) {
        self.git_log = None;
        self.git_log_sel = 0;
        // log は Git ビューの上に開く想定なので、閉じたら Git ビューへ復帰させる。
        self.open_git_view();
    }

    // --- ブランチ操作 (`b`一覧 / Enter切替 / n新規 / d削除 / `/`絞り込み) ----------
    /// `b`: Open the local branch list. Opened from the Git view, so the view closes. Cursor moves to the current branch. The filter is reset.
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn open_git_branches(&mut self) {
        let list = crate::git::branches(&self.root);
        if list.is_empty() {
            self.flash = Some(crate::i18n::tr(self.lang, crate::i18n::Msg::NoBranches).into());
            return;
        }
        self.git_branch_filter.clear();
        self.git_branch_filtering = false;
        self.git_branch_sel = list.iter().position(|b| b.is_current).unwrap_or(0);
        self.git_branches = Some(list);
        self.git_view = false;
    }
    /// Whether the branch list is showing.
    pub fn is_git_branches(&self) -> bool {
        self.git_branches.is_some()
    }
    /// The filtered display list (names containing the query, case-insensitive). All entries if the query is empty. Used for rendering/operations.
    pub fn git_branch_view(&self) -> Vec<crate::git::BranchInfo> {
        let Some(all) = &self.git_branches else {
            return Vec::new();
        };
        let q = self.git_branch_filter.to_lowercase();
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
        self.git_branch_sel
    }
    /// Move the display-list cursor by delta (clamped to range).
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn git_branch_move(&mut self, delta: i32) {
        self.git_branch_sel =
            clamp_cursor(self.git_branch_sel, delta, self.git_branch_view().len());
    }
    /// The branch selected in the display list.
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub(super) fn git_branch_selected(&self) -> Option<crate::git::BranchInfo> {
        self.git_branch_view().into_iter().nth(self.git_branch_sel)
    }
    /// Close the branch list (q/Esc). Returns to the Git view since it came from there. Also resets the filter.
    pub fn close_git_branches(&mut self) {
        self.git_branches = None;
        self.git_branch_sel = 0;
        self.git_branch_filter.clear();
        self.git_branch_filtering = false;
        self.open_git_view();
    }
    /// Refetch the list (e.g. after deletion). Clamps the cursor to range.
    fn git_branches_reload(&mut self) {
        self.git_branches = Some(crate::git::branches(&self.root));
        self.clamp_git_branch_sel();
    }
    /// `Enter`: Switch to the selected branch. If uncommitted changes conflict, git refuses (Err is flashed). On success, returns to the Git view.
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn checkout_selected_branch(&mut self) -> Result<()> {
        let Some(b) = self.git_branch_selected() else {
            return Ok(());
        };
        let name = b.name;
        match crate::git::checkout(&self.root, &name) {
            Ok(()) => {
                self.refresh()?;
                self.refresh_git_if_needed(); // ブランチ名/状態のキャッシュを即更新(描画前でも正)
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
        match crate::git::delete_branch(&self.root, name, force) {
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

    // --- ブランチ絞り込み (`/`) ----------------------------------------------
    /// Whether branch-filter input is active (while true, main captures keys as characters).
    pub fn git_branch_filtering(&self) -> bool {
        self.git_branch_filtering
    }
    /// The current filter query (for the footer prompt).
    pub fn git_branch_query(&self) -> &str {
        &self.git_branch_filter
    }
    /// `/`: Start filter input (query starts empty).
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn git_branch_start_filter(&mut self) {
        self.git_branch_filtering = true;
        self.git_branch_filter.clear();
        self.git_branch_sel = 0;
    }
    pub fn git_branch_filter_push(&mut self, c: char) {
        self.git_branch_filter.push(c);
        self.clamp_git_branch_sel();
    }
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn git_branch_filter_backspace(&mut self) {
        self.git_branch_filter.pop();
        self.clamp_git_branch_sel();
    }
    /// Enter: Commit the input (keeps the query; navigate with j/k afterwards).
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn git_branch_filter_commit(&mut self) {
        self.git_branch_filtering = false;
    }
    /// Esc: Clear the filter (return to all entries).
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn git_branch_filter_clear(&mut self) {
        self.git_branch_filter.clear();
        self.git_branch_filtering = false;
        self.clamp_git_branch_sel();
    }
    fn clamp_git_branch_sel(&mut self) {
        let len = self.git_branch_view().len();
        if self.git_branch_sel >= len {
            self.git_branch_sel = len.saturating_sub(1);
        }
    }

    // --- コミットグラフ (`G`: SourceTree / Git Graph 風) -----------------------
    /// `G`: Open the commit graph (colored output like `git log --all --graph`). Closes the Git view.
    /// Moves the cursor to the first commit row. Flashes if there are no commits.
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn open_git_graph(&mut self) {
        // 優先順(config→HEAD→最近順)を用意。既存ブランチに合わせて整える(削除を除き新規を末尾へ)。
        self.ensure_graph_order();
        // 起動時(初回 or 表示集合が空)は上限つき既定選択を入れる。基準が別枝でも HEAD は必ず含む。
        if self.git_graph_visible.is_empty() {
            self.git_graph_visible = self.default_graph_visible();
        }
        // 基準が未設定なら config の優先ブランチ(左から最初に存在し表示中のもの)を基準にする。
        if self.git_graph_base.is_none() && !self.cfg.ui.graph_base_branches.is_empty() {
            if let Some((oid, label)) = self.derive_base_from_order() {
                self.git_graph_base = Some(oid);
                self.git_graph_base_label = Some(label);
            }
        }
        let (rows, has_wt, legend, hidden) = self.build_git_graph_rows();
        if rows.iter().all(|r| r.commit.is_none()) && !has_wt {
            self.flash = Some(crate::i18n::tr(self.lang, crate::i18n::Msg::NoCommits).into());
            return;
        }
        // 変更がある時は worktree 行(先頭)にカーソルを置き、無ければ最初のコミット行へ。
        self.git_graph_sel = if has_wt {
            0
        } else {
            rows.iter().position(|r| r.commit.is_some()).unwrap_or(0)
        };
        self.git_graph = Some(rows);
        self.git_graph_legend = legend;
        self.git_graph_hidden = hidden;
        self.git_view = false;
    }

    /// Reconcile `git_graph_order` (the priority order) with the current local branches.
    /// On first run, build it as **`[ui.graph_base_branches]` (those that exist, in order) → HEAD → recency order**.
    /// In later sessions, keep the reordering while dropping removed branches and appending new ones in recency order.
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    fn ensure_graph_order(&mut self) {
        let by_rec = crate::git::branches_by_recency(&self.root); // (name, is_current, time) 新しい順
        let exists: std::collections::HashSet<&str> =
            by_rec.iter().map(|(n, _, _)| n.as_str()).collect();
        // 消えたブランチを除去(並び替えは維持)。
        self.git_graph_order.retain(|n| exists.contains(n.as_str()));
        if self.git_graph_order.is_empty() {
            // 初期構築: config 優先 → HEAD → 最近順。
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
            self.git_graph_order = order;
        } else {
            // 新規ブランチを最近順で末尾に追加。
            let known: std::collections::HashSet<String> =
                self.git_graph_order.iter().cloned().collect();
            for (n, _, _) in &by_rec {
                if !known.contains(n) {
                    self.git_graph_order.push(n.clone());
                }
            }
        }
    }

    /// Returns the tip of the first branch in priority order (`git_graph_order`) that "exists and is visible" as the base candidate.
    /// `(oid, label)`. None if there is none.
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    fn derive_base_from_order(&self) -> Option<(String, String)> {
        for name in &self.git_graph_order {
            if self.git_graph_visible.contains(name) {
                if let Some(oid) = crate::git::branch_tip(&self.root, name) {
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
        // HEAD は常に。
        for (name, is_cur, _) in crate::git::branches_by_recency(&self.root) {
            if is_cur {
                set.insert(name);
            }
        }
        // 優先順の先頭から上限まで(0=無制限)。
        for name in &self.git_graph_order {
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
        let total = crate::git::branches_by_recency(&self.root).len();
        let visible: Vec<String> = self.git_graph_visible.iter().cloned().collect();
        // 空 or 全ブランチ網羅 → --all(None)。それ以外は指定ブランチのみ。
        let use_all = visible.is_empty() || visible.len() >= total;
        let refs = if use_all {
            None
        } else {
            Some(visible.as_slice())
        };
        let mut rows = crate::git::graph_with_base(
            &self.root,
            self.git_graph_base.as_deref(),
            self.lang,
            refs,
        );
        // 非表示ブランチを行の装飾(refs)からも除く(レーン非表示と一貫させる)。
        // 同じコミットに同居する非表示ブランチ名でラベル/凡例が膨らむのを防ぐ。
        if !use_all {
            let allowed = &self.git_graph_visible;
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
        let mut legend =
            crate::git::legend_from_rows(&rows, &self.root, self.git_graph_base_label.as_deref());
        // 凡例を**優先順(`git_graph_order`)**に並べ替える(config 優先→HEAD→最近)。順序外は末尾。
        legend.sort_by_key(|e| {
            self.git_graph_order
                .iter()
                .position(|n| n == &e.name)
                .unwrap_or(usize::MAX)
        });
        // 非表示数 = 全ローカルブランチ − 表示中ブランチ数。
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
        self.git_graph_base = Some(id);
        self.git_graph_base_label = Some(label.clone());
        self.rebuild_git_graph_keep_sel();
        self.flash = Some(format!(
            "{}{label}",
            crate::i18n::tr(self.lang, crate::i18n::Msg::GraphBaseSet)
        ));
    }

    /// Phase 2: Clear the base pin and return to the default display.
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn git_graph_clear_base(&mut self) {
        if self.git_graph_base.is_none() {
            return;
        }
        self.git_graph_base = None;
        self.git_graph_base_label = None;
        self.rebuild_git_graph_keep_sel();
        self.flash = Some(crate::i18n::tr(self.lang, crate::i18n::Msg::GraphBaseCleared).into());
    }

    /// Rebuild the graph after a base change and restore the cursor to the same commit row (or the first commit/worktree if absent).
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    fn rebuild_git_graph_keep_sel(&mut self) {
        if self.git_graph.is_none() {
            return;
        }
        let cur = self.git_graph_selected_row().and_then(|r| r.commit.clone());
        let (rows, has_wt, legend, hidden) = self.build_git_graph_rows();
        self.git_graph_sel = cur
            .as_deref()
            .and_then(|id| rows.iter().position(|r| r.commit.as_deref() == Some(id)))
            .or(if has_wt { Some(0) } else { None })
            .or_else(|| rows.iter().position(|r| r.commit.is_some()))
            .unwrap_or(0);
        self.git_graph = Some(rows);
        self.git_graph_legend = legend;
        self.git_graph_hidden = hidden;
    }

    /// The base label (for the title `base: …`). None if unset.
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn git_graph_base_label(&self) -> Option<&str> {
        self.git_graph_base_label.as_deref()
    }

    /// The graph legend (branch ⇄ lane color, for rendering). HEAD first, base next, then in order of appearance.
    pub fn git_graph_legend(&self) -> &[crate::git::LegendEntry] {
        &self.git_graph_legend
    }
    /// The number of branches hidden by the cap/toggle (for the legend's `(+K hidden)`).
    pub fn git_graph_hidden_count(&self) -> usize {
        self.git_graph_hidden
    }

    // --- ブランチ表示パネル(`b`: 多ブランチ時の表示トグル＋優先順の並び替え) ----------
    /// Open the panel. Initializes the tentative selection from the current visible set and moves the cursor to the top (= head of priority order).
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn git_graph_open_picker(&mut self) {
        if self.git_graph.is_none() {
            return;
        }
        self.ensure_graph_order();
        // 表示集合が空(=全表示扱い)のときは全ブランチを選択済みとして見せる。
        if self.git_graph_visible.is_empty() {
            self.git_graph_visible = self.git_graph_order.iter().cloned().collect();
        }
        self.git_graph_picker_set = self.git_graph_visible.clone();
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
        crate::git::branches_by_recency(&self.root)
            .into_iter()
            .find(|(_, c, _)| *c)
            .map(|(n, _, _)| n)
    }
    /// Panel rows: `(branch, is_current, tentatively visible ON)`. Ordered by **priority order (`git_graph_order`)**.
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn git_graph_picker_items(&self) -> Vec<(String, bool, bool)> {
        let head = self.head_branch_name();
        self.git_graph_order
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
        let n = self.git_graph_order.len();
        if n < 2 {
            return;
        }
        let i = self.git_graph_picker_sel;
        let j = i as i32 + delta;
        if j < 0 || j >= n as i32 {
            return;
        }
        let j = j as usize;
        self.git_graph_order.swap(i, j);
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
            return; // HEAD は外せない。
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
        if let Some(b) = &self.git_graph_base_label {
            set.insert(b.clone());
        }
        self.git_graph_picker_set = set;
    }
    /// Commit the tentative selection, rebuild the graph, and close the panel.
    /// If reordered with `J`/`K`, **re-derive the base to the head (visible) branch in priority order** (point 3+5).
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn git_graph_picker_apply(&mut self) {
        self.git_graph_visible = self.git_graph_picker_set.clone();
        if self.git_graph_reordered {
            match self.derive_base_from_order() {
                Some((oid, label)) => {
                    self.git_graph_base = Some(oid);
                    self.git_graph_base_label = Some(label);
                }
                None => {
                    self.git_graph_base = None;
                    self.git_graph_base_label = None;
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
        self.git_graph.is_some()
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
        self.git_graph.as_deref().unwrap_or(&[])
    }
    /// Cursor position in the graph (row index).
    pub fn git_graph_sel(&self) -> usize {
        self.git_graph_sel
    }
    /// Move the cursor by delta over **commit rows only** (skipping connector rows).
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn git_graph_move(&mut self, delta: i32) {
        let Some(rows) = &self.git_graph else {
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
            .position(|&i| i == self.git_graph_sel)
            .unwrap_or(0);
        let next = clamp_cursor(cur, delta, commits.len());
        self.git_graph_sel = commits[next];
    }
    /// Close the graph (q/Esc). Returns to the Git view.
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn close_git_graph(&mut self) {
        self.git_graph = None;
        self.git_graph_sel = 0;
        self.open_git_view();
    }
    /// The graph's cursor row (for render/title).
    pub fn git_graph_selected_row(&self) -> Option<&crate::git::GraphRow> {
        self.git_graph
            .as_ref()
            .and_then(|rows| rows.get(self.git_graph_sel))
    }
    /// `Enter`: Open the detail of the selected row. Commit row → commit_diff / working-tree row → worktree_diff.
    /// The graph is kept behind (closing the detail returns to it).
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn open_git_graph_detail(&mut self) {
        let Some(row) = self.git_graph_selected_row().cloned() else {
            return;
        };
        let (lines, meta) = if row.worktree {
            (crate::git::worktree_diff(&self.root), None)
        } else if let Some(id) = row.commit {
            (
                crate::git::commit_diff(&self.root, &id),
                crate::git::commit_meta(&self.root, &id),
            )
        } else {
            return;
        };
        self.git_detail = Some(lines);
        self.git_detail_meta = meta; // コミットなら全文メッセージを上部に出す
        self.git_detail_scroll = 0;
        self.git_detail_hscroll = 0;
        self.git_detail_title = None; // タイトルはグラフ選択行(worktree/commit)から出す
    }

    /// `Enter`: Read the selected commit's detail (commit_diff) and show it as a full-screen diff.
    /// Overlaid while the log is kept behind (Esc/q closes only the detail and returns to the log).
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn open_git_commit_detail(&mut self) {
        let Some(id) = self.git_log_selected_id() else {
            return;
        };
        let lines = crate::git::commit_diff(&self.root, &id);
        self.git_detail = Some(lines);
        self.git_detail_meta = crate::git::commit_meta(&self.root, &id);
        self.git_detail_scroll = 0;
        self.git_detail_hscroll = 0;
        self.git_detail_title = None;
    }

    /// `D` in the Git view: Open a diff that **aggregates all working-tree changes** (same as the graph's Uncommitted).
    /// Does nothing if there are no changes. Overlaid as a detail view (git_detail); `s` toggles unified/split, q/Esc returns to the Git view.
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn open_worktree_detail(&mut self) {
        let lines = crate::git::worktree_diff(&self.root);
        if lines.is_empty() {
            self.flash = Some(crate::i18n::tr(self.lang, crate::i18n::Msg::NoChanges).into());
            return;
        }
        self.git_detail = Some(lines);
        self.git_detail_meta = None; // 未コミットなのでメッセージ無し
        self.git_detail_scroll = 0;
        self.git_detail_hscroll = 0;
        self.git_detail_title =
            Some(crate::i18n::tr(self.lang, crate::i18n::Msg::UncommittedChanges).into());
    }

    /// Title override for the detail (git_detail) (e.g. when opening all working-tree changes from the git view).
    pub fn git_detail_title(&self) -> Option<&str> {
        self.git_detail_title.as_deref()
    }

    /// Whether the commit detail is showing (for render/key branching).
    pub fn is_git_detail(&self) -> bool {
        self.git_detail.is_some()
    }

    /// Meta info shown at the top of the commit detail (full message, etc.). None for worktree diffs, etc.
    pub fn git_detail_meta(&self) -> Option<&crate::git::CommitMeta> {
        self.git_detail_meta.as_ref()
    }

    /// Returns the DiffLine sequence of the commit detail (for rendering). Empty when hidden.
    pub fn git_detail_lines(&self) -> &[crate::git::DiffLine] {
        self.git_detail.as_deref().unwrap_or(&[])
    }

    /// Vertically scroll the commit detail. The bottom is clamped by the viewport the renderer updates.
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn git_detail_scroll_by(&mut self, delta: i32) {
        // 描画側が更新する総行数(コミットメッセージ＋diff)でクランプ。
        // diff 行数だけだとヘッダ分スクロールできず末尾が隠れてしまう。
        let total = self.git_detail_total;
        let max = total.saturating_sub(self.git_detail_viewport as usize) as i32;
        let next = (self.git_detail_scroll as i32)
            .saturating_add(delta)
            .clamp(0, max.max(0));
        self.git_detail_scroll = next as u16;
    }

    /// Move the commit detail to the top/bottom (g/G).
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn git_detail_scroll_to(&mut self, end: bool) {
        if end {
            self.git_detail_scroll_by(i32::MAX);
        } else {
            self.git_detail_scroll = 0;
        }
    }

    /// The commit detail's current scroll amount (for rendering).
    pub fn git_detail_scroll(&self) -> u16 {
        self.git_detail_scroll
    }

    /// Horizontally scroll the commit detail (h/l). The end is clamped by the renderer's longest line width.
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn git_detail_hscroll_by(&mut self, delta: i32) {
        let next = (self.git_detail_hscroll as i32 + delta).max(0);
        self.git_detail_hscroll = next as u16;
    }
    /// Move horizontal scroll to the line start (left edge) / line end (right edge) (0/$). The line end is clamped to the longest line width by the renderer.
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn git_detail_hscroll_home(&mut self) {
        self.git_detail_hscroll = 0;
    }
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn git_detail_hscroll_end(&mut self) {
        self.git_detail_hscroll = u16::MAX;
    }
    /// The commit detail's current horizontal scroll amount (for rendering).
    pub fn git_detail_hscroll(&self) -> u16 {
        self.git_detail_hscroll
    }
    /// Clamp the horizontal scroll amount to the maximum `max` (the renderer calls this with the longest line width).
    pub fn clamp_git_detail_hscroll(&mut self, max: u16) {
        self.git_detail_hscroll = self.git_detail_hscroll.min(max);
    }

    /// The renderer records the number of displayable rows (for g/G and scroll clamping).
    pub fn set_git_detail_viewport(&mut self, h: u16) {
        self.git_detail_viewport = h;
    }

    /// The renderer records the total line count (commit message + diff) (for clamping the scroll limit).
    pub fn set_git_detail_total(&mut self, n: usize) {
        self.git_detail_total = n;
    }

    /// Close the commit detail (q/Esc). Returns to the log behind it.
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn close_git_detail(&mut self) {
        self.git_detail = None;
        self.git_detail_meta = None;
        self.git_detail_title = None;
        self.git_detail_scroll = 0;
        self.git_detail_hscroll = 0;
    }
}
