//! Bookmarks: set/jump leader state and the bookmark-list overlay — methods on `App`.

use super::*;

impl App {
    // --- ブックマーク (M7 補助) ----------------------------------------------
    /// Whether we are waiting for an alphabetic key after `m`/`'` (while true, main intercepts keys).
    pub fn is_marking(&self) -> bool {
        self.mark_pending.is_some()
    }
    /// For the footer prompt: Some(true)=set (m) / Some(false)=jump (') / None=not marking.
    pub fn mark_is_set(&self) -> Option<bool> {
        self.mark_pending.map(|a| a == MarkAction::Set)
    }
    /// `m`=enter set mode (the next letter registers; scope = letter case).
    pub fn start_mark_set(&mut self) {
        self.mark_pending = Some(MarkAction::Set);
    }
    /// `'`=enter jump mode (the next letter jumps; `'` shows the list).
    pub fn start_mark_jump(&mut self) {
        self.mark_pending = Some(MarkAction::Jump);
    }
    /// Cancel the mark wait (Esc or a non-applicable key).
    pub fn cancel_mark(&mut self) {
        self.mark_pending = None;
    }
    /// The one key after `m`/`'`. Set = register the current location (root) / Jump = letter jumps, `'` shows the list.
    pub fn mark_input(&mut self, c: char) {
        let Some(action) = self.mark_pending.take() else {
            return;
        };
        match action {
            MarkAction::Set if c.is_ascii_alphabetic() => {
                // カーソル位置のファイル/ディレクトリを登録。選択が無ければ現在の root。
                let target = self
                    .entries
                    .get(self.selected)
                    .map(|e| e.path.clone())
                    .unwrap_or_else(|| self.root.clone());
                match self.bookmarks.set(c, target.clone()) {
                    Ok(_) => {
                        let scope = if c.is_ascii_uppercase() {
                            crate::i18n::tr(self.lang, crate::i18n::Msg::GlobalApp)
                        } else {
                            crate::i18n::tr(self.lang, crate::i18n::Msg::Local)
                        };
                        self.flash = Some(format!(
                            "{} {c} = {}  [{scope}]",
                            crate::i18n::tr(self.lang, crate::i18n::Msg::Bookmarked),
                            self.format_path(&target),
                        ));
                    }
                    // 保存失敗: メモリ上には登録済みだが再起動で消える旨を通知(握り潰さない)。
                    Err(e) => {
                        self.flash = Some(format!(
                            "{}{e}",
                            crate::i18n::tr(self.lang, crate::i18n::Msg::OperationFailed)
                        ));
                    }
                }
            }
            MarkAction::Jump if c == '\'' => self.open_bookmark_list(),
            MarkAction::Jump if c.is_ascii_alphabetic() => match self.bookmarks.get(c) {
                // ディレクトリ=そこへ移動 / ファイル=プレビューで開く(tree は変えず、終了で元の tree へ戻る)。
                Some(p) if p.is_dir() => self.jump_to_dir(p),
                Some(p) if p.is_file() => self.enter_preview(&p),
                Some(p) => {
                    self.flash = Some(format!(
                        "{}: {}",
                        crate::i18n::tr(self.lang, crate::i18n::Msg::BookmarkTargetMissing),
                        self.format_path(&p)
                    ))
                }
                None => {
                    self.flash = Some(format!(
                        "{} '{c}'",
                        crate::i18n::tr(self.lang, crate::i18n::Msg::NoBookmark)
                    ))
                }
            },
            _ => {
                self.flash =
                    Some(crate::i18n::tr(self.lang, crate::i18n::Msg::InvalidMarkKey).into())
            }
        }
    }
    /// Make the bookmark target (a directory) the new root and show the Tree. Does not change open_dir (the local key).
    pub(super) fn jump_to_dir(&mut self, dir: std::path::PathBuf) {
        // root を変えるので旧 root の選択/ビジュアル/絞り込み/検索を破棄する(持ち越すと
        // マーカー不可視のまま旧 root のファイルが誤操作対象になる footgun)。
        self.clear_for_root_change();
        self.root = dir;
        self.entries.clear();
        self.selected = 0;
        self.rebuild_tree_notify();
        self.mode = Mode::Tree;
        self.preview_path = None;
        self.preview_kind = None;
    }

    /// `:`=make the current location the "anchored root" (**no text input**). Re-anchors the current tree root,
    /// navigated to with `h`/`l`, to the display base (open_dir), so relative-path display, the `yr` path copy, and
    /// the title all become relative to the current location (resolving what used to show as `../` from launch). Leaves the tree structure and cursor unchanged.
    pub fn reanchor_root(&mut self) {
        if self.open_dir == self.root {
            self.flash = Some(crate::i18n::tr(self.lang, crate::i18n::Msg::AlreadyRoot).into());
            return;
        }
        self.open_dir = self.root.clone();
        self.flash = Some(format!(
            "{}: {}",
            crate::i18n::tr(self.lang, crate::i18n::Msg::Root),
            home_relative(&self.root)
        ));
    }

    /// `A`=reset the anchor (display base = open_dir) back to the **launch dir** (the counterpart of reanchor_root=`a`).
    /// Resets the anchor that `a` moved to the current location back to the startup position, restoring relative-path display.
    /// Leaves the tree structure / cursor / root unchanged (moves only open_dir, symmetric to reanchor_root).
    pub fn reset_anchor(&mut self) {
        if self.open_dir == self.launch_dir {
            self.flash =
                Some(crate::i18n::tr(self.lang, crate::i18n::Msg::AlreadyAtStartDir).into());
            return;
        }
        self.open_dir = self.launch_dir.clone();
        self.flash = Some(format!(
            "{}: {}",
            crate::i18n::tr(self.lang, crate::i18n::Msg::AnchorReset),
            home_relative(&self.launch_dir)
        ));
    }

    // --- ブックマーク一覧オーバーレイ ----------------------------------------
    pub fn is_bookmark_list(&self) -> bool {
        self.bookmark_list
    }
    pub fn open_bookmark_list(&mut self) {
        self.bookmark_list = true;
        self.bookmark_list_sel = 0;
    }
    pub fn close_bookmark_list(&mut self) {
        self.bookmark_list = false;
    }
    pub fn bookmark_list_sel(&self) -> usize {
        self.bookmark_list_sel
    }
    /// List items (is_local, key, path). Local first, then global.
    pub fn bookmark_list_items(&self) -> Vec<(bool, char, std::path::PathBuf)> {
        let (local, global) = self.bookmarks.list();
        let mut v = Vec::with_capacity(local.len() + global.len());
        for (k, p) in local {
            v.push((true, k, p));
        }
        for (k, p) in global {
            v.push((false, k, p));
        }
        v
    }
    pub fn bookmark_list_move(&mut self, delta: i32) {
        let n = self.bookmark_list_items().len();
        if n == 0 {
            return;
        }
        self.bookmark_list_sel =
            (self.bookmark_list_sel as i32 + delta).rem_euclid(n as i32) as usize;
    }
    /// Jump to the selected bookmark (closes the list).
    pub fn bookmark_list_jump(&mut self) {
        let items = self.bookmark_list_items();
        if let Some((_, _, p)) = items.get(self.bookmark_list_sel).cloned() {
            self.bookmark_list = false;
            if p.is_dir() {
                self.jump_to_dir(p);
            } else if p.is_file() {
                self.enter_preview(&p); // ファイルはプレビューで開く(tree は変えない)
            } else {
                self.flash = Some(format!(
                    "{}: {}",
                    crate::i18n::tr(self.lang, crate::i18n::Msg::BookmarkTargetMissing),
                    self.format_path(&p)
                ));
            }
        }
    }
    /// If the selected list item is a file, **open it directly in the editor** without going through the preview
    /// (closes the list and sets pending_edit; the run loop launches run_editor). Directories cannot be edited and flash.
    pub fn bookmark_list_edit(&mut self) {
        let items = self.bookmark_list_items();
        if let Some((_, _, p)) = items.get(self.bookmark_list_sel).cloned() {
            if p.is_file() {
                self.bookmark_list = false;
                self.pending_edit = Some(p);
            } else if p.is_dir() {
                self.flash =
                    Some(crate::i18n::tr(self.lang, crate::i18n::Msg::CannotEditDirectory).into());
            } else {
                self.flash = Some(format!(
                    "{}: {}",
                    crate::i18n::tr(self.lang, crate::i18n::Msg::BookmarkTargetMissing),
                    self.format_path(&p)
                ));
            }
        }
    }
    /// Delete the selected bookmark (the list stays open; the selection is clamped).
    pub fn bookmark_list_delete(&mut self) {
        let items = self.bookmark_list_items();
        if let Some((_, key, _)) = items.get(self.bookmark_list_sel) {
            let key = *key;
            if let Err(e) = self.bookmarks.remove(key) {
                self.flash = Some(format!(
                    "{}{e}",
                    crate::i18n::tr(self.lang, crate::i18n::Msg::OperationFailed)
                ));
            }
            let n = self.bookmark_list_items().len();
            if self.bookmark_list_sel >= n {
                self.bookmark_list_sel = n.saturating_sub(1);
            }
        }
    }

    /// Start filtering (`/`). Recursively collects everything under root into a pool and enters input mode.
    pub fn start_filter(&mut self) {
        self.filter_pool = collect_all(&self.root, self.show_hidden);
        self.filter_input = Some(String::new());
        self.tree_filter = Some(String::new());
        self.selected = 0;
        self.reapply_filter();
    }

    /// Add one character to the filter input (live filtering).
    pub fn filter_input_push(&mut self, c: char) {
        if let Some(q) = self.filter_input.as_mut() {
            q.push(c);
        }
        self.tree_filter = self.filter_input.clone();
        self.reapply_filter();
    }

    /// Delete one character from the filter input.
    pub fn filter_input_backspace(&mut self) {
        if let Some(q) = self.filter_input.as_mut() {
            q.pop();
        }
        self.tree_filter = self.filter_input.clone();
        self.reapply_filter();
    }

    /// Commit input (Enter): leave editing but keep the filtered results (navigate normally afterwards).
    pub fn filter_commit(&mut self) {
        self.filter_input = None;
    }

    /// Clear the filter (Esc): return to the normal tree.
    pub fn filter_clear(&mut self) {
        self.clear_filter_state();
        self.selected = 0;
        self.rebuild_tree_notify();
    }

    /// Discard the filter state (input/query/pool and the changed-files filter) (does not rebuild the tree).
    pub(super) fn clear_filter_state(&mut self) {
        self.filter_input = None;
        self.tree_filter = None;
        self.filter_pool = Vec::new();
        self.changed_filter = false;
    }

    /// Whether input mode (key interception) is active.
    pub fn is_filtering(&self) -> bool {
        self.filter_input.is_some()
    }

    /// The query while a filter is applied (including after the input is committed). Used for title / relative-path display.
    pub fn filter_query(&self) -> Option<&str> {
        self.tree_filter.as_deref()
    }

    /// Filter the pool by the current query (substring match, case-insensitive) to build entries.
    /// When the query is empty (= right after pressing `/`, before typing any character), **show nothing**
    /// (avoiding a flat display of everything, which would look like "expand all").
    fn reapply_filter(&mut self) {
        let q = self.tree_filter.clone().unwrap_or_default().to_lowercase();
        self.entries = if q.is_empty() {
            Vec::new()
        } else {
            self.filter_pool
                .iter()
                .filter(|e| {
                    e.path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .map(|n| n.to_lowercase().contains(&q))
                        .unwrap_or(false)
                })
                .cloned()
                .collect()
        };
        if self.selected >= self.entries.len() {
            self.selected = self.entries.len().saturating_sub(1);
        }
        // 絞り込み結果は別 entries 集合なので visual_anchor(添字)は stale。無効化する。
        self.visual_anchor = None;
    }

    // --- 変更ファイルのみフィルタ (`C`) ＋ 変更間ジャンプ (`n`/`N`) — Agent Watch ① ----------
    /// Whether the changed-files-only tree filter is active (drives the flat relative-path rendering).
    pub fn changed_filter(&self) -> bool {
        self.changed_filter
    }

    /// The sorted list of files with a git status under the current root (the review work-list).
    /// Deleted files have a status but no longer exist, so they are excluded (nothing to preview/select).
    fn changed_paths(&self) -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = self
            .git_status
            .keys()
            .filter(|p| p.starts_with(&self.root) && p.is_file())
            .cloned()
            .collect();
        v.sort();
        v
    }

    /// `C`: toggle the changed-files-only view. ON shows the flat list of changed files (like the `/`
    /// filter's result list — review them top to bottom); OFF returns to the normal tree. When there is
    /// no change, stays on the tree with a flash (never trap the user in an empty list).
    pub fn toggle_changed_filter(&mut self) {
        if self.changed_filter {
            self.changed_filter = false;
            self.selected = 0;
            self.rebuild_tree_notify();
            return;
        }
        self.refresh_git_if_needed();
        if self.changed_paths().is_empty() {
            self.flash = Some(crate::i18n::tr(self.lang, crate::i18n::Msg::NoChangedFiles).into());
            return;
        }
        self.clear_filter_state(); // 名前絞り込み(`/`)とは排他
        self.changed_filter = true;
        self.selected = 0;
        self.reapply_changed_filter();
    }

    /// Rebuild entries as the flat changed-files list. Called on toggle and by `refresh_fs` while active
    /// (an agent committing/reverting updates the list live). Auto-exits to the tree when the last change
    /// disappears (e.g. everything was committed) instead of leaving an empty list.
    pub(super) fn reapply_changed_filter(&mut self) {
        let entries: Vec<super::Entry> = self
            .changed_paths()
            .into_iter()
            .map(|path| super::Entry {
                path,
                is_dir: false,
                depth: 0,
                expanded: false,
            })
            .collect();
        if entries.is_empty() {
            self.changed_filter = false;
            self.selected = 0;
            self.rebuild_tree_notify();
            self.flash = Some(crate::i18n::tr(self.lang, crate::i18n::Msg::NoChangedFiles).into());
            return;
        }
        self.entries = entries;
        if self.selected >= self.entries.len() {
            self.selected = self.entries.len().saturating_sub(1);
        }
        self.visual_anchor = None;
    }

    /// `n`/`N` on the tree: jump the cursor to the next/previous changed file in path order (wraps).
    /// In the normal tree the target is revealed (collapsed ancestors expanded); in the changed-only
    /// filter it moves within the flat list.
    pub fn jump_changed(&mut self, dir: i64) {
        // diff ビュー表示中は「次/前の変更ファイルの diff へ切替」= ビューを出ずに変更を回遊する
        // (hunk/lazygit 流のレビュー動線)。ツリーでは従来どおりカーソルジャンプ。
        if self.is_git_diff_preview() {
            self.diff_jump_changed(dir);
            return;
        }
        self.refresh_git_if_needed();
        let paths = self.changed_paths();
        if paths.is_empty() {
            self.flash = Some(crate::i18n::tr(self.lang, crate::i18n::Msg::NoChangedFiles).into());
            return;
        }
        let len = paths.len() as i64;
        let idx = match self.entries.get(self.selected).map(|e| e.path.clone()) {
            Some(cur) => match paths.binary_search(&cur) {
                Ok(i) => (i as i64 + dir).rem_euclid(len) as usize,
                // カーソルが変更ファイル上にない: 挿入位置の次(前)へ(wrap)。
                Err(ins) => {
                    if dir > 0 {
                        (ins as i64).rem_euclid(len) as usize
                    } else {
                        (ins as i64 - 1).rem_euclid(len) as usize
                    }
                }
            },
            None => 0,
        };
        let target = paths[idx].clone();
        if self.changed_filter {
            if let Some(i) = self.entries.iter().position(|e| e.path == target) {
                self.selected = i;
            }
            return;
        }
        match self.reveal_path_deep(&target) {
            Ok(true) => {}
            // 対象が隠しディレクトリ配下などで可視化できない(`.` で隠し表示に切替すれば届く)。
            Ok(false) => {
                self.flash =
                    Some(crate::i18n::tr(self.lang, crate::i18n::Msg::JumpTargetHidden).into());
            }
            Err(e) => {
                self.flash = Some(format!(
                    "{}{e}",
                    crate::i18n::tr(self.lang, crate::i18n::Msg::OperationFailed)
                ));
            }
        }
    }

    /// `n`/`N` inside the GitDiff preview: switch the diff target to the next/previous changed file
    /// (wraps) without leaving the view. **Scope depends on how the diff was opened**: a follow-opened
    /// diff cycles only the files changed during the current follow session ("what just changed"),
    /// while a tree-/hub-opened diff cycles the full uncommitted change set. The tree cursor follows
    /// (so `q` lands on the file just reviewed), and where `q` returns to (hub vs tree) is preserved.
    fn diff_jump_changed(&mut self, dir: i64) {
        let paths = if self.diff_follow_scope {
            self.follow_session_paths()
        } else {
            self.refresh_git_if_needed();
            self.changed_paths()
        };
        if paths.is_empty() {
            self.flash = Some(crate::i18n::tr(self.lang, crate::i18n::Msg::NoChangedFiles).into());
            return;
        }
        let Some(crate::preview::PreviewKind::GitDiff(cur)) = self.preview_kind.clone() else {
            return;
        };
        let len = paths.len() as i64;
        let idx = match paths.iter().position(|p| *p == cur) {
            Some(i) => (i as i64 + dir).rem_euclid(len) as usize,
            // 表示中ファイルが集合に無い(直前にコミットされた等): 先頭/末尾から。
            None => {
                if dir > 0 {
                    0
                } else {
                    (len - 1) as usize
                }
            }
        };
        let target = paths[idx].clone();
        if target == cur {
            return; // 対象が1つだけ
        }
        // ツリーのカーソルも同期(q で戻ったとき、いま見ていたファイルの上に居る)。
        if self.changed_filter {
            self.reapply_changed_filter();
            if let Some(i) = self.entries.iter().position(|e| e.path == target) {
                self.selected = i;
            }
        } else {
            let _ = self.reveal_path_deep(&target);
        }
        // open_git_diff は came_from_git_view / diff_follow_scope を初期化するため保存/復元する
        // (ハブ経由の diff から回遊しても q でハブへ戻れる・フォロースコープの回遊が続く)。
        let came = self.came_from_git_view;
        let scope = self.diff_follow_scope;
        self.open_git_diff(&target);
        self.came_from_git_view = came;
        self.diff_follow_scope = scope;
    }

    /// The follow session's reviewable files (recorded while `F` was ON), pruned to still-existing
    /// non-media files. First-change order (chronological — the review order of "what just happened").
    fn follow_session_paths(&self) -> Vec<PathBuf> {
        self.follow_session
            .iter()
            .filter(|p| p.is_file() && !self.follow_is_media(p))
            .cloned()
            .collect()
    }

    /// The current GitDiff target's position within its navigation set (1-based, total) — the follow
    /// session for follow-opened diffs, the full change set otherwise. For the title's `(2/5)` indicator.
    pub fn diff_change_position(&self) -> Option<(usize, usize)> {
        let crate::preview::PreviewKind::GitDiff(p) = self.preview_kind.as_ref()? else {
            return None;
        };
        let paths = if self.diff_follow_scope {
            self.follow_session_paths()
        } else {
            self.changed_paths()
        };
        let i = paths.iter().position(|q| q == p)?;
        Some((i + 1, paths.len()))
    }

    /// Refetch git status if root changed (called just before rendering). Does nothing if root is unchanged.
    /// As a result, expand/collapse does not refetch; updates happen only on directory moves and tab switches.
    ///
    /// The cheap `statuses`+`branch` (tens of ms, within the <60ms budget) is refetched synchronously, but the
    /// **heavy `ignored` (~800ms on large repos) is offloaded to a separate thread** (the "don't block the UI" principle). Rendering stays responsive, and
    /// only the dimming based on ignore rules appears after the result arrives (`apply_ignored`).
    pub fn refresh_git_if_needed(&mut self) {
        if self.git_status_for.as_deref() == Some(self.root.as_path()) {
            return;
        }
        // 安い statuses/branch は root が変わるたびに取り直す(表示外の変更も上に戻った時に追従)。
        self.git_status = crate::git::statuses(&self.root);
        self.git_branch = crate::git::branch(&self.root);
        self.git_status_for = Some(self.root.clone());

        // 重い ignored(無視セット)は **repo(workdir)が変わった時だけ** 作り直す。同一リポジトリ内の
        // サブディレクトリへ潜っても無視ルールは同一なので流用する(`l` 潜行時の再計算を回避)。
        let wd = crate::git::workdir(&self.root);
        let different_repo = self.git_ignored_for != wd;
        let need = self.git_ignored_dirty || different_repo;
        // 同一 workdir の計算が既に走行中なら待つ(無視ルール変更 dirty の時はそれより新しい結果が要る)。
        let inflight = self.git_ignored_pending == wd && !self.git_ignored_dirty;
        if !need || inflight {
            return;
        }
        match wd {
            None => {
                // repo でない: 即クリア(計算不要)。
                self.git_ignored.clear();
                self.git_ignored_for = None;
                self.git_ignored_pending = None;
                self.git_ignored_dirty = false;
            }
            Some(wd) => {
                // 別 repo へ移った時だけ旧セットを消す(暗転の混在を防ぐ)。無視ルール変更(dirty)で
                // 同一 repo を作り直す時は、新セット到着まで旧セットを見せ続ける(チラつき回避)。
                if different_repo {
                    self.git_ignored.clear();
                    self.git_ignored_for = None;
                }
                self.git_ignored_gen = self.git_ignored_gen.wrapping_add(1);
                self.git_ignored_pending = Some(wd.clone());
                self.git_ignored_dirty = false;
                let gen = self.git_ignored_gen;
                self.spawn_or_sync_ignored(self.root.clone(), wd, gen);
            }
        }
    }

    /// Attach the Sender of the worker that computes `ignored` in the background (called by main at startup).
    pub fn attach_git_loader(&mut self, tx: std::sync::mpsc::Sender<IgnoredResult>) {
        self.ignored_tx = Some(tx);
    }

    /// Compute the heavy `git::ignored(root)` on a separate thread and return the result via the Sender. If no Sender is attached (tests /
    /// no channel), fall back to **synchronous** immediate computation and application (keeping unit tests that don't assume rendering working).
    fn spawn_or_sync_ignored(&mut self, root: PathBuf, workdir: PathBuf, gen: u64) {
        let Some(tx) = self.ignored_tx.clone() else {
            // 同期フォールバック: その場で計算して反映(陳腐化なし)。
            let set = crate::git::ignored(&root);
            self.apply_ignored(IgnoredResult { gen, workdir, set });
            return;
        };
        std::thread::spawn(move || {
            let set = crate::git::ignored(&root);
            let _ = tx.send(IgnoredResult { gen, workdir, set });
        });
    }

    /// Apply the `ignored` computation result from the other thread. Discards results with an old generation (moved to a different repo during computation).
    /// Returns true if applying changed the state (the caller redraws).
    pub fn apply_ignored(&mut self, res: IgnoredResult) -> bool {
        if res.gen != self.git_ignored_gen {
            return false; // 陳腐化: 既に別 repo / 別世代へ移っている
        }
        self.git_ignored = res.set;
        self.git_ignored_for = Some(res.workdir);
        self.git_ignored_pending = None;
        true
    }

    /// Returns the git status of a given path.
    pub fn git_status_of(&self, path: &Path) -> Option<crate::git::FileStatus> {
        self.git_status.get(path).copied()
    }

    /// Whether `path` is excluded by gitignore (itself or an **ancestor directory** is in the ignored set).
    /// Even when expanding under `node_modules` (= one entry), an ancestor match lets the contents be dimmed too. Does not look above root.
    pub fn is_ignored(&self, path: &Path) -> bool {
        if self.git_ignored.is_empty() {
            return false;
        }
        let mut p = path;
        loop {
            if self.git_ignored.contains(p) {
                return true;
            }
            if p == self.root {
                return false;
            }
            match p.parent() {
                Some(parent) => p = parent,
                None => return false,
            }
        }
    }

    /// The current branch name (None if not a repo). Used for the tree's title display.
    pub fn git_branch(&self) -> Option<&str> {
        self.git_branch.as_deref()
    }

    /// Whether there is at least one change (= a git repo with uncommitted changes). Used to decide the status-gutter display.
    pub fn git_has_changes(&self) -> bool {
        !self.git_status.is_empty()
    }

    /// Reload: refetch the directory listing and git status (`r` key, FSEvents, returning from an external tool).
    /// Keeps derived state in sync **in one place** to prevent the display from diverging from reality:
    /// - **Prune selection to existing paths only** (prevents one externally-deleted item from making a batch trash operation fail entirely #12).
    ///   A broken symlink itself is kept as existing (judged via `symlink_metadata` without following → don't wrongly drop it from operation targets).
    /// - **Recompute only the active derived views** (don't touch inactive ones, to avoid over-recomputation;
    ///   same spirit as the root guard in `refresh_git_if_needed`): in the Git view, the change list; in Preview mode,
    ///   refetch the current preview (reflecting git_view_entries staleness on return from an external git tool #4 / preview staleness on external edit).
    pub fn refresh(&mut self) -> Result<()> {
        // 明示的な再読込(`r`・fileops・外部ツール復帰)は無視セットも作り直す(全再計算)。
        self.refresh_fs(true)
    }

    /// Reload from FSEvents. When `recompute_ignored=false`, **skip recomputing the heavy ignore set (`ignored`)**
    /// and refetch only the cheap `statuses`+`branch`. Ignore rules (`.gitignore` /
    /// `.git/info/exclude`) rarely change, so this is called with `true` only when they actually change, also rebuilding
    /// `ignored`. This avoids a few-hundred-ms freeze per event on large repositories.
    pub fn refresh_fs(&mut self, recompute_ignored: bool) -> Result<()> {
        if recompute_ignored {
            self.git_status_for = None; // 次の描画で statuses+branch を再計算
            self.git_ignored_dirty = true; // 重い ignored も作り直す(無視ルール変更/明示 refresh)。
                                           // 旧セットは反映まで維持(別スレッド計算・チラつき回避)。
        } else {
            self.refresh_git_status_only(); // statuses+branch のみ(ignored はキャッシュ保持)
        }
        self.diff_cache = None; // 作業ツリーが変わった可能性 → diff キャッシュを落とす(外部編集の追従)
        self.gutter_cache = None; // 同上: git 変更ガターも作業ツリー変更で作り直す
        self.rebuild_tree()?;
        // 変更ファイルのみフィルタ中は一覧を最新の statuses から作り直す(エージェントの編集に追従)。
        if self.changed_filter {
            self.refresh_git_if_needed();
            self.reapply_changed_filter();
        }
        // 消えたパスを選択集合から除く(retain で実在のみ残す。シンボリックリンクは辿らない)。
        self.selection.retain(|p| p.symlink_metadata().is_ok());
        // アクティブな派生ビューのみ追従させる。
        if self.is_git_view() {
            self.git_view_reload();
        }
        if matches!(self.mode, Mode::Preview) {
            self.reload_preview();
        }
        Ok(())
    }

    /// Cheap git update: refetch only `statuses` + `branch`, **leaving the heavy `ignored` (ignore set) untouched**
    /// (keeping the cache). Does nothing if the initial `ignored` computation hasn't happened yet (`git_status_for` unset)
    /// (the next render's `refresh_git_if_needed` computes everything).
    fn refresh_git_status_only(&mut self) {
        if self.git_status_for.as_deref() != Some(self.root.as_path()) {
            return;
        }
        self.git_status = crate::git::statuses(&self.root);
        self.git_branch = crate::git::branch(&self.root);
    }
}
