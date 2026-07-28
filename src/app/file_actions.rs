//! File-manager actions: create / rename / delete (trash) and multi-select / visual range — methods on `App`.

use super::*;

impl App {
    // --- ファイル操作: 作成/リネーム/削除 (M7 Phase B・確認＋ゴミ箱) -------------
    /// Whether a confirm/input dialog is showing (while true, main intercepts keys).
    pub fn is_dialog(&self) -> bool {
        self.dialog.is_some()
    }
    /// Whether it is a confirm (y/n) dialog (false = text input dialog). For render/key branching.
    pub fn dialog_is_confirm(&self) -> bool {
        matches!(
            self.dialog.as_ref().map(|d| &d.kind),
            Some(DialogKind::Confirm { .. })
        )
    }
    /// Render view for the dialog: (is_confirm, heading/message, text being entered, cursor character position).
    /// Preview is rendered via a separate path (`dialog_preview_view`), so this returns None for it.
    pub fn dialog_view(&self) -> Option<(bool, &str, &str, usize)> {
        match &self.dialog.as_ref()?.kind {
            DialogKind::Confirm { message, .. } => Some((true, message.as_str(), "", 0)),
            DialogKind::Input {
                title,
                buffer,
                cursor,
            } => Some((false, title.as_str(), buffer.as_str(), *cursor)),
            DialogKind::Preview { .. } => None,
        }
    }
    /// Whether the batch-rename preview is showing (keys: y apply / Esc cancel / j k scroll).
    pub fn dialog_is_preview(&self) -> bool {
        matches!(
            self.dialog.as_ref().map(|d| &d.kind),
            Some(DialogKind::Preview { .. })
        )
    }
    /// For rendering the preview: (heading, the "old → new" list, first displayed row).
    pub fn dialog_preview_view(&self) -> Option<(&str, &[String], usize)> {
        match &self.dialog.as_ref()?.kind {
            DialogKind::Preview {
                title,
                lines,
                scroll,
            } => Some((title.as_str(), lines.as_slice(), *scroll)),
            _ => None,
        }
    }
    /// Scroll the preview up/down.
    pub fn dialog_preview_scroll(&mut self, delta: i32) {
        if let Some(Dialog {
            kind: DialogKind::Preview { lines, scroll, .. },
            ..
        }) = self.dialog.as_mut()
        {
            let max = lines.len().saturating_sub(1) as i32;
            *scroll = (*scroll as i32 + delta).clamp(0, max) as usize;
        }
    }
    /// Whether the confirm dialog offers `!`=permanent delete (unrecoverable) (true only for delete confirmation).
    pub fn dialog_allow_permanent(&self) -> bool {
        matches!(
            self.dialog.as_ref().map(|d| &d.kind),
            Some(DialogKind::Confirm {
                allow_permanent: true,
                ..
            })
        )
    }
    /// Whether the current confirm dialog is a branch deletion (so the y/!/n wording is for a branch, not a file deletion).
    pub fn confirm_is_branch_delete(&self) -> bool {
        matches!(
            self.dialog.as_ref().map(|d| &d.op),
            Some(PendingOp::GitDeleteBranch { .. })
        )
    }
    /// Whether the current confirm dialog is a drag-and-drop transfer (so the keys/wording become c=copy / m=move).
    pub fn confirm_is_drop(&self) -> bool {
        matches!(
            self.dialog.as_ref().map(|d| &d.op),
            Some(PendingOp::DropTransfer { .. })
        )
    }
    /// Whether the current confirm dialog is the app-quit confirmation (so `q`/`y`/Enter quit and the chip/footer say "quit").
    pub fn confirm_is_quit(&self) -> bool {
        matches!(self.dialog.as_ref().map(|d| &d.op), Some(PendingOp::Quit))
    }
    /// Whether the current confirm dialog is a bookmark-overwrite confirmation (own chip/footer wording).
    pub fn confirm_is_bookmark(&self) -> bool {
        matches!(
            self.dialog.as_ref().map(|d| &d.op),
            Some(PendingOp::BookmarkOverwrite { .. })
        )
    }

    /// Receive a paste from the terminal (including drag-and-drop). While input is active (dialog input / filter /
    /// search / branch filter), **insert text**; in Tree mode, if the dropped content is an existing path,
    /// open a **copy/move dialog** (drop target = the cursor's base directory). Control characters are stripped.
    pub fn handle_paste(&mut self, text: String) {
        // 1) テキスト入力中: そのまま入力欄へ流す(制御文字は捨てる)。
        if self.is_text_input_active() {
            for ch in text.chars().filter(|c| !c.is_control()) {
                self.input_push_char(ch);
            }
            return;
        }
        // 2) Tree モードのみ「ドロップ」として扱う(プレビュー等では無視)。
        if !matches!(self.tab.mode, Mode::Tree) {
            return;
        }
        let sources = parse_dropped_paths(&text);
        if sources.is_empty() {
            return; // ドロップ対象(実在パス)が無ければ何もしない
        }
        let dir = self.op_base_dir();
        let message = format!(
            "{} {} → {}",
            sources.len(),
            crate::i18n::tr(self.lang, crate::i18n::Msg::DroppedItems),
            self.format_path(&dir)
        );
        self.dialog = Some(Dialog {
            op: PendingOp::DropTransfer { sources, dir },
            kind: DialogKind::Confirm {
                message,
                allow_permanent: false,
            },
        });
    }

    /// Whether text input is active (a state where a paste should be inserted as characters).
    fn is_text_input_active(&self) -> bool {
        self.dialog_is_text_input()
            || self.is_filtering()
            || self.is_searching()
            || self.git_branch_filtering()
    }
    /// Whether the currently open dialog is the Input (text input) kind.
    fn dialog_is_text_input(&self) -> bool {
        matches!(
            self.dialog.as_ref().map(|d| &d.kind),
            Some(DialogKind::Input { .. })
        )
    }
    /// Feed one character to the active input field (priority: dialog input > filter > search > branch filter).
    fn input_push_char(&mut self, ch: char) {
        if self.dialog_is_text_input() {
            self.dialog_input_push(ch);
        } else if self.is_filtering() {
            self.filter_input_push(ch);
        } else if self.is_searching() {
            self.search_input_push(ch);
        } else if self.git_branch_filtering() {
            self.git_branch_filter_push(ch);
        }
    }

    /// Execute the drop confirmation's `c`=copy / `m`=move. Transfers each source to the drop target and flashes the result.
    /// Runs in the background (design principle #4) — see `start_file_op`. Stops at the first
    /// error (unlike paste/duplicate), matching the original synchronous implementation.
    pub fn drop_apply(&mut self, move_it: bool) -> Result<()> {
        let Some(dialog) = self.dialog.take() else {
            return Ok(());
        };
        let PendingOp::DropTransfer { sources, dir } = dialog.op else {
            return Ok(());
        };
        let job = FileOpJob {
            kind: if move_it {
                FileOpKind::DropMove
            } else {
                FileOpKind::DropCopy
            },
            targets: sources,
            dest: Some(dir),
            root: PathBuf::new(), // start_file_op が dispatch 時の tab.root で埋める
            err_self_paste: String::new(), // Drop 側は自己貼付の専用メッセージを使わない(copy_dir_all の一般エラーに任せる=既存挙動)
            err_failed: crate::i18n::tr(self.lang, crate::i18n::Msg::OperationFailed).to_string(),
        };
        self.start_file_op(job);
        Ok(())
    }

    /// Attach the Sender of the worker that runs background filesystem operations (called by main at startup).
    pub fn attach_fileop_runner(&mut self, tx: std::sync::mpsc::Sender<crate::app::FileOpResult>) {
        self.fileop_tx = Some(tx);
    }

    /// Whether watcher-driven refreshes should be held back right now.
    ///
    /// A background copy/move/delete of a large directory makes konoma's own watcher fire tens of
    /// thousands of events; refreshing per burst starves the run loop so keystrokes are not processed
    /// until the storm ends. While the operation runs, the run loop accumulates the bursts and applies
    /// **one** refresh afterwards — nothing is missed, because `apply_file_op` refreshes on completion
    /// and the deferred burst is flushed right after it. Same idea as `is_content_event`/the `.git`
    /// lock filter: never react to filesystem noise konoma itself caused.
    pub fn should_defer_fs_events(&self) -> bool {
        self.fileop_pending.is_some()
    }

    /// Start a filesystem operation in the background, or run it synchronously when no runner is
    /// attached (unit tests / no run loop) so the result is observable immediately (same contract
    /// as `spawn_or_sync_statuses`/`spawn_or_sync_ignored`). Returns false and flashes when another
    /// operation is already in flight (only one runs at a time).
    pub(super) fn start_file_op(&mut self, mut job: FileOpJob) -> bool {
        if self.fileop_pending.is_some() {
            self.flash = Some(crate::i18n::tr(self.lang, crate::i18n::Msg::FileOpBusy).into());
            return false;
        }
        // 「どのタブから投げたか」を dispatch 時にここで焼き付ける(呼び出し側は空で渡す)。
        // 完了時にアクティブなタブが同じとは限らないため、apply_file_op がこれで判定する。
        job.root = self.tab.root.clone();
        self.fileop_gen = self.fileop_gen.wrapping_add(1);
        let gen = self.fileop_gen;
        self.fileop_pending = Some(job.kind);
        self.fileop_total = job.targets.len();
        let progress = Arc::new(crate::fileops::Progress::default());
        self.fileop_progress = Some(progress.clone());

        let Some(tx) = self.fileop_tx.clone() else {
            let res = Self::run_file_op(gen, job, &progress);
            self.apply_file_op(res);
            return true;
        };
        let kind = job.kind;
        let root = job.root.clone();
        // panic 時の代替エラーは self.lang をここ(UI スレッド)で先に翻訳しておく(ワーカースレッドには
        // &self が無く tr() を呼べない)。
        let panic_err = crate::i18n::tr(self.lang, crate::i18n::Msg::OperationFailed).to_string();
        std::thread::spawn(move || {
            // ワーカーが panic すると結果が返らず `fileop_pending` が永久に残り、スピナーが回り続ける。
            // 他のワーカーと同じ安全網で必ず結果を返す(原則#3)。
            let res =
                crate::preview::markdown::catch_silent(|| Self::run_file_op(gen, job, &progress))
                    .unwrap_or(FileOpResult {
                        gen,
                        kind,
                        root,
                        ok: 0,
                        last: None,
                        err: Some(panic_err),
                    });
            let _ = tx.send(res);
        });
        true
    }

    /// The actual work (pure: no `&self`), shared by the worker thread and the synchronous fallback.
    /// `PasteCopy`/`PasteMove`/`Duplicate` continue past a failing target (existing behaviour);
    /// `DropCopy`/`DropMove` stop at the first error (existing `drop_apply` behaviour).
    fn run_file_op(gen: u64, job: FileOpJob, p: &crate::fileops::Progress) -> FileOpResult {
        let FileOpJob {
            kind,
            targets,
            dest,
            root,
            err_self_paste,
            err_failed,
        } = job;
        let (mut ok, mut last, mut err) = (0usize, None, None);
        match kind {
            FileOpKind::PasteCopy | FileOpKind::PasteMove => {
                let Some(dest) = dest else {
                    return FileOpResult {
                        gen,
                        kind,
                        root,
                        ok,
                        last,
                        err: Some(err_failed),
                    };
                };
                for src in &targets {
                    // 自分自身(やその中)への貼り付けは無限コピーになるので弾く。
                    if dest.starts_with(src) {
                        err = Some(err_self_paste.clone());
                        p.bump_item();
                        continue;
                    }
                    let res = if kind == FileOpKind::PasteCopy {
                        crate::fileops::copy_into_with_progress(&dest, src, p)
                    } else {
                        crate::fileops::move_into_with_progress(&dest, src, p)
                    };
                    match res {
                        Ok(path) => {
                            ok += 1;
                            last = Some(path);
                        }
                        Err(e) => err = Some(e.to_string()),
                    }
                    p.bump_item();
                }
            }
            FileOpKind::DropCopy | FileOpKind::DropMove => {
                let Some(dest) = dest else {
                    return FileOpResult {
                        gen,
                        kind,
                        root,
                        ok,
                        last,
                        err: Some(err_failed),
                    };
                };
                for src in &targets {
                    let res = if kind == FileOpKind::DropCopy {
                        crate::fileops::copy_into_with_progress(&dest, src, p)
                    } else {
                        crate::fileops::move_into_with_progress(&dest, src, p)
                    };
                    match res {
                        Ok(path) => {
                            ok += 1;
                            last = Some(path);
                            p.bump_item();
                        }
                        Err(e) => {
                            err = Some(e.to_string());
                            break; // ドロップは最初の失敗で打ち切る(既存 drop_apply と同じ)
                        }
                    }
                }
            }
            FileOpKind::Duplicate => {
                for src in &targets {
                    // 親ディレクトリ = 複製先(その場に複製)。ルート等で親が無ければ失敗扱い(クラッシュしない)。
                    let Some(parent) = src.parent() else {
                        err = Some(err_failed.clone());
                        p.bump_item();
                        continue;
                    };
                    match crate::fileops::copy_into_with_progress(parent, src, p) {
                        Ok(path) => {
                            ok += 1;
                            last = Some(path);
                        }
                        Err(e) => err = Some(e.to_string()),
                    }
                    p.bump_item();
                }
            }
            // ゴミ箱送りは `trash::delete_all` の**1回の呼び出し**なので途中経過が取れない。
            // 完了して初めて全件ぶん進む(進捗は 0/M のまま最後に M/M になる)。
            FileOpKind::Trash => match crate::fileops::move_to_trash(&targets) {
                Ok(()) => {
                    ok = targets.len();
                    for _ in 0..ok {
                        p.bump_item();
                    }
                }
                Err(e) => err = Some(e.to_string()),
            },
            // 完全削除はパスごとに回るので、1件ずつ進捗を進められる(`_with_progress`)。
            FileOpKind::DeletePermanent => {
                match crate::fileops::delete_permanently_with_progress(&targets, p) {
                    Ok(()) => ok = targets.len(),
                    Err(e) => err = Some(e.to_string()),
                }
            }
        }
        FileOpResult {
            gen,
            kind,
            root,
            ok,
            last,
            err,
        }
    }

    /// Apply a finished filesystem operation: refresh the tree, reveal the newest item and flash the
    /// same summary the synchronous implementation used to produce.
    ///
    /// The generation check is belt-and-braces only: `fileop_gen` advances solely in
    /// `start_file_op`, which refuses to start while `fileop_pending` is set, so the generation is
    /// frozen for as long as an operation is in flight and a stale result cannot actually occur
    /// today. The guard that matters is the **root comparison**: the user can switch tabs during a
    /// long copy, so the tab active when the result arrives may not be the one that dispatched it.
    /// The tree cursor is only touched when the roots match.
    pub fn apply_file_op(&mut self, res: FileOpResult) -> bool {
        if res.gen != self.fileop_gen {
            return false;
        }
        self.fileop_pending = None;
        self.fileop_progress = None;
        self.fileop_total = 0;
        // 再読込は無条件(どのタブを見ていてもディスクは実際に変わっている)。
        let mut post = self.refresh();
        // リビール(=カーソル移動)は**投げたタブに戻っている時だけ**。`reveal_and_select` は
        // 無条件に rebuild_tree するので、別タブでやると絞り込み(`/`)や変更ビュー(`C`)を畳み、
        // そのタブのカーソルまで動かしてしまう。比較はタブ番号でなく root で行う
        // (タブは操作中に閉じたり並べ替えたりできる=番号は同一性の根拠にならない)。
        if post.is_ok() && res.root == self.tab.root {
            if let Some(p) = &res.last {
                post = self.reveal_and_select(p);
            }
        }
        // 削除系の選択解除は**成功した時だけ**(旧同期版と同じ)。12件中3件目で権限エラー、
        // のような部分失敗で選択ごと失うと、ユーザーは選び直しからやり直しになる。
        if res.err.is_none() && matches!(res.kind, FileOpKind::Trash | FileOpKind::DeletePermanent)
        {
            self.clear_selection();
        }
        let lang = self.lang;
        // 操作自体は成功したのに後段(再読込/リビール)が失敗したら、その失敗を出す。
        // 旧同期版は `refresh()?` / `reveal_and_select(p)?` で handle_key へ伝播し
        // `resolve_key_result` が flash していた=成功 flash で塗り潰さない(原則: 未確認を成功と言わない)。
        // 操作自体が失敗している場合は、そちらのエラーの方が本題なので下の分岐に任せる。
        if res.err.is_none() {
            if let Err(e) = post {
                self.flash = Some(format!(
                    "{}: {e}",
                    crate::i18n::tr(lang, crate::i18n::Msg::Failed)
                ));
                return true;
            }
        }
        self.flash = Some(match res.kind {
            FileOpKind::PasteCopy | FileOpKind::PasteMove => match &res.err {
                Some(e) => format!(
                    "{} {} / {}: {e}",
                    crate::i18n::tr(lang, crate::i18n::Msg::Pasted),
                    res.ok,
                    crate::i18n::tr(lang, crate::i18n::Msg::Failed),
                ),
                None => format!(
                    "{} ({})",
                    crate::i18n::tr(lang, crate::i18n::Msg::Pasted),
                    res.ok
                ),
            },
            FileOpKind::Duplicate => match &res.err {
                Some(e) => format!(
                    "{} {} / {}: {e}",
                    crate::i18n::tr(lang, crate::i18n::Msg::Duplicated),
                    res.ok,
                    crate::i18n::tr(lang, crate::i18n::Msg::Failed)
                ),
                None => format!(
                    "{} ({})",
                    crate::i18n::tr(lang, crate::i18n::Msg::Duplicated),
                    res.ok
                ),
            },
            FileOpKind::DropCopy | FileOpKind::DropMove => {
                let verb = if res.kind == FileOpKind::DropMove {
                    crate::i18n::tr(lang, crate::i18n::Msg::Moved)
                } else {
                    crate::i18n::tr(lang, crate::i18n::Msg::Copied)
                };
                match &res.err {
                    Some(e) => format!("{}: {e}", crate::i18n::tr(lang, crate::i18n::Msg::Failed)),
                    None => format!("{verb} ({})", res.ok),
                }
            }
            FileOpKind::Trash => match &res.err {
                Some(e) => format!("{}: {e}", crate::i18n::tr(lang, crate::i18n::Msg::Failed)),
                None => format!(
                    "{} ({})",
                    crate::i18n::tr(lang, crate::i18n::Msg::MovedToTrash),
                    res.ok
                ),
            },
            FileOpKind::DeletePermanent => match &res.err {
                Some(e) => format!("{}: {e}", crate::i18n::tr(lang, crate::i18n::Msg::Failed)),
                None => format!(
                    "{} ({})",
                    crate::i18n::tr(lang, crate::i18n::Msg::DeletedPermanently),
                    res.ok
                ),
            },
        });
        true
    }

    /// `N/M` (targets finished) plus the running leaf-file count once it exceeds the target count,
    /// e.g. `2/3` or `1/1 · 3120`. `None` when no operation is running.
    pub fn fileop_progress_text(&self) -> Option<String> {
        let p = self.fileop_progress.as_ref()?;
        let items = p.items();
        let files = p.files();
        let total = self.fileop_total;
        let mut s = format!("{items}/{total}");
        if files > total {
            s.push_str(&format!(" · {files}"));
        }
        Some(s)
    }

    /// The operation target directory relative to the cursor (selection is a directory = inside it / a file = its parent / none = root).
    pub(super) fn op_base_dir(&self) -> PathBuf {
        match self.tab.entries.get(self.tab.selected) {
            Some(e) if e.is_dir => e.path.clone(),
            Some(e) => e
                .path
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| self.tab.root.clone()),
            None => self.tab.root.clone(),
        }
    }

    /// Deep reveal: expand **every** collapsed ancestor of `target` under root (rebuilding as each level
    /// appears), then select it. Unlike `reveal_and_select` (one level), this reaches into collapsed
    /// subtrees — used by the changed-file jump (`n`/`N`) and follow mode. Returns whether the target
    /// became visible and selected (false = e.g. hidden by the dotfile filter).
    pub(super) fn reveal_path_deep(&mut self, target: &Path) -> Result<bool> {
        // root 直下から target の親まで浅い順に。expanded を立てて rebuild すると次の階層が現れる。
        let mut ancestors: Vec<PathBuf> = Vec::new();
        let mut p = target.parent();
        while let Some(a) = p {
            if a == self.tab.root || !a.starts_with(&self.tab.root) {
                break;
            }
            ancestors.push(a.to_path_buf());
            p = a.parent();
        }
        ancestors.reverse();
        for anc in ancestors {
            if let Some(e) = self.tab.entries.iter_mut().find(|e| e.path == anc) {
                if e.is_dir && !e.expanded {
                    e.expanded = true;
                    self.rebuild_tree()?;
                }
            }
        }
        if let Some(i) = self.tab.entries.iter().position(|e| e.path == target) {
            self.tab.selected = i;
            return Ok(true);
        }
        Ok(false)
    }

    /// After an operation, reveal and select `target`. If the parent is a collapsed dir, expand it before rebuilding.
    pub(super) fn reveal_and_select(&mut self, target: &Path) -> Result<()> {
        if let Some(parent) = target.parent() {
            if let Some(e) = self.tab.entries.iter_mut().find(|e| e.path == parent) {
                if e.is_dir && !e.expanded {
                    e.expanded = true;
                }
            }
        }
        self.rebuild_tree()?;
        if let Some(i) = self.tab.entries.iter().position(|e| e.path == target) {
            self.tab.selected = i;
        }
        Ok(())
    }

    // --- 複数選択 + ビジュアル(範囲)選択 (M7 Phase B) ------------------------
    /// Whether there are any selected items (the committed set).
    pub fn has_selection(&self) -> bool {
        !self.tab.selection.is_empty()
    }
    /// Whether `path` is in the committed selection (for the render's marker check).
    pub fn is_selected(&self, path: &Path) -> bool {
        self.tab.selection.contains(path)
    }
    /// Clear the entire selection (Esc / after a batch operation). If in visual mode, also clears the range.
    pub fn clear_selection(&mut self) {
        self.tab.selection.clear();
        self.tab.visual_anchor = None;
    }
    /// `V`=toggle the selection of the single item at the cursor and move down one (for picking scattered items / consecutive selection).
    pub fn toggle_select(&mut self) {
        if let Some(e) = self.tab.entries.get(self.tab.selected) {
            let p = e.path.clone();
            if !self.tab.selection.remove(&p) {
                self.tab.selection.insert(p);
            }
            self.tree_next();
        }
    }

    /// Whether visual (range) selection mode is active.
    pub fn is_visual(&self) -> bool {
        self.tab.visual_anchor.is_some()
    }
    /// `v`=start visual mode. Places the anchor at the current cursor (does nothing on an empty tree).
    pub fn enter_visual(&mut self) {
        if !self.tab.entries.is_empty() {
            self.tab.visual_anchor = Some(self.tab.selected);
        }
    }
    /// The visual range [lo, hi] (anchor to cursor, ascending). None if not in visual mode.
    fn visual_bounds(&self) -> Option<(usize, usize)> {
        self.tab
            .visual_anchor
            .map(|a| (a.min(self.tab.selected), a.max(self.tab.selected)))
    }
    /// Whether row `idx` is within the visual range (for the render's live preview).
    pub fn is_in_visual_range(&self, idx: usize) -> bool {
        matches!(self.visual_bounds(), Some((lo, hi)) if idx >= lo && idx <= hi)
    }
    /// Commit the range with `v`/Esc etc.: add the paths in range to the selection set and leave visual mode.
    pub fn exit_visual_commit(&mut self) {
        if let Some((lo, hi)) = self.visual_bounds() {
            let paths: Vec<PathBuf> = (lo..=hi)
                .filter_map(|i| self.tab.entries.get(i).map(|e| e.path.clone()))
                .collect();
            for p in paths {
                self.tab.selection.insert(p);
            }
        }
        self.tab.visual_anchor = None;
    }
    /// Esc=leave visual mode without taking in the range (keeps the committed selection).
    pub fn exit_visual_cancel(&mut self) {
        self.tab.visual_anchor = None;
    }
    /// `a`/`A` during visual mode = scope bulk selection. `all_displayed`=everything displayed / otherwise=the same parent level as the cursor.
    /// Also takes in the in-progress range, adds it to the selection, and leaves visual mode.
    pub fn visual_select_scope(&mut self, all_displayed: bool) {
        // 進行中の範囲を確定。
        let mut paths: Vec<PathBuf> = self
            .visual_bounds()
            .map(|(lo, hi)| {
                (lo..=hi)
                    .filter_map(|i| self.tab.entries.get(i).map(|e| e.path.clone()))
                    .collect()
            })
            .unwrap_or_default();
        // スコープ(全表示 or カーソルと同じ親)を追加。
        let parent = self
            .tab
            .entries
            .get(self.tab.selected)
            .and_then(|e| e.path.parent().map(|p| p.to_path_buf()));
        for e in &self.tab.entries {
            let same_parent = e.path.parent().map(|p| p.to_path_buf()) == parent;
            if all_displayed || same_parent {
                paths.push(e.path.clone());
            }
        }
        for p in paths {
            self.tab.selection.insert(p);
        }
        self.tab.visual_anchor = None;
    }
    /// Whether to render the marker column (leftmost 2 cells): there is a committed selection, or visual mode is active.
    pub fn show_selection_gutter(&self) -> bool {
        !self.tab.selection.is_empty() || self.is_visual()
    }
    /// The count currently marked (committed ∪ visual range). For the context's `sel: N` display.
    pub fn marked_count(&self) -> usize {
        match self.visual_bounds() {
            Some((lo, hi)) => {
                let mut n = self.tab.selection.len();
                for i in lo..=hi {
                    if let Some(e) = self.tab.entries.get(i) {
                        if !self.tab.selection.contains(&e.path) {
                            n += 1;
                        }
                    }
                }
                n
            }
            None => self.tab.selection.len(),
        }
    }
    /// Target paths for a batch operation: if there is a selection, **all selected items** (path ascending); otherwise, the single item at the cursor.
    pub(super) fn op_targets(&self) -> Vec<PathBuf> {
        if self.tab.selection.is_empty() {
            self.tab
                .entries
                .get(self.tab.selected)
                .map(|e| vec![e.path.clone()])
                .unwrap_or_default()
        } else {
            self.tab.selection.iter().cloned().collect()
        }
    }
}
