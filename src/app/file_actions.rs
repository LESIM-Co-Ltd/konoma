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
        if !matches!(self.mode, Mode::Tree) {
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
    pub fn drop_apply(&mut self, move_it: bool) -> Result<()> {
        let Some(dialog) = self.dialog.take() else {
            return Ok(());
        };
        let PendingOp::DropTransfer { sources, dir } = dialog.op else {
            return Ok(());
        };
        let (mut ok, mut last) = (0usize, None);
        let mut err: Option<String> = None;
        for src in &sources {
            let r = if move_it {
                crate::fileops::move_into(&dir, src)
            } else {
                crate::fileops::copy_into(&dir, src)
            };
            match r {
                Ok(p) => {
                    ok += 1;
                    last = Some(p);
                }
                Err(e) => {
                    err = Some(e.to_string());
                    break;
                }
            }
        }
        self.refresh()?;
        if let Some(p) = &last {
            let _ = self.reveal_and_select(p);
        }
        let verb = if move_it {
            crate::i18n::tr(self.lang, crate::i18n::Msg::Moved)
        } else {
            crate::i18n::tr(self.lang, crate::i18n::Msg::Copied)
        };
        self.flash = Some(match err {
            Some(e) => format!(
                "{}: {e}",
                crate::i18n::tr(self.lang, crate::i18n::Msg::Failed)
            ),
            None => format!("{verb} ({ok})"),
        });
        Ok(())
    }

    /// The operation target directory relative to the cursor (selection is a directory = inside it / a file = its parent / none = root).
    pub(super) fn op_base_dir(&self) -> PathBuf {
        match self.entries.get(self.selected) {
            Some(e) if e.is_dir => e.path.clone(),
            Some(e) => e
                .path
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| self.root.clone()),
            None => self.root.clone(),
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
            if a == self.root || !a.starts_with(&self.root) {
                break;
            }
            ancestors.push(a.to_path_buf());
            p = a.parent();
        }
        ancestors.reverse();
        for anc in ancestors {
            if let Some(e) = self.entries.iter_mut().find(|e| e.path == anc) {
                if e.is_dir && !e.expanded {
                    e.expanded = true;
                    self.rebuild_tree()?;
                }
            }
        }
        if let Some(i) = self.entries.iter().position(|e| e.path == target) {
            self.selected = i;
            return Ok(true);
        }
        Ok(false)
    }

    /// After an operation, reveal and select `target`. If the parent is a collapsed dir, expand it before rebuilding.
    pub(super) fn reveal_and_select(&mut self, target: &Path) -> Result<()> {
        if let Some(parent) = target.parent() {
            if let Some(e) = self.entries.iter_mut().find(|e| e.path == parent) {
                if e.is_dir && !e.expanded {
                    e.expanded = true;
                }
            }
        }
        self.rebuild_tree()?;
        if let Some(i) = self.entries.iter().position(|e| e.path == target) {
            self.selected = i;
        }
        Ok(())
    }

    // --- 複数選択 + ビジュアル(範囲)選択 (M7 Phase B) ------------------------
    /// Whether there are any selected items (the committed set).
    pub fn has_selection(&self) -> bool {
        !self.selection.is_empty()
    }
    /// Whether `path` is in the committed selection (for the render's marker check).
    pub fn is_selected(&self, path: &Path) -> bool {
        self.selection.contains(path)
    }
    /// Clear the entire selection (Esc / after a batch operation). If in visual mode, also clears the range.
    pub fn clear_selection(&mut self) {
        self.selection.clear();
        self.visual_anchor = None;
    }
    /// `V`=toggle the selection of the single item at the cursor and move down one (for picking scattered items / consecutive selection).
    pub fn toggle_select(&mut self) {
        if let Some(e) = self.entries.get(self.selected) {
            let p = e.path.clone();
            if !self.selection.remove(&p) {
                self.selection.insert(p);
            }
            self.tree_next();
        }
    }

    /// Whether visual (range) selection mode is active.
    pub fn is_visual(&self) -> bool {
        self.visual_anchor.is_some()
    }
    /// `v`=start visual mode. Places the anchor at the current cursor (does nothing on an empty tree).
    pub fn enter_visual(&mut self) {
        if !self.entries.is_empty() {
            self.visual_anchor = Some(self.selected);
        }
    }
    /// The visual range [lo, hi] (anchor to cursor, ascending). None if not in visual mode.
    fn visual_bounds(&self) -> Option<(usize, usize)> {
        self.visual_anchor
            .map(|a| (a.min(self.selected), a.max(self.selected)))
    }
    /// Whether row `idx` is within the visual range (for the render's live preview).
    pub fn is_in_visual_range(&self, idx: usize) -> bool {
        matches!(self.visual_bounds(), Some((lo, hi)) if idx >= lo && idx <= hi)
    }
    /// Commit the range with `v`/Esc etc.: add the paths in range to the selection set and leave visual mode.
    pub fn exit_visual_commit(&mut self) {
        if let Some((lo, hi)) = self.visual_bounds() {
            let paths: Vec<PathBuf> = (lo..=hi)
                .filter_map(|i| self.entries.get(i).map(|e| e.path.clone()))
                .collect();
            for p in paths {
                self.selection.insert(p);
            }
        }
        self.visual_anchor = None;
    }
    /// Esc=leave visual mode without taking in the range (keeps the committed selection).
    pub fn exit_visual_cancel(&mut self) {
        self.visual_anchor = None;
    }
    /// `a`/`A` during visual mode = scope bulk selection. `all_displayed`=everything displayed / otherwise=the same parent level as the cursor.
    /// Also takes in the in-progress range, adds it to the selection, and leaves visual mode.
    pub fn visual_select_scope(&mut self, all_displayed: bool) {
        // 進行中の範囲を確定。
        let mut paths: Vec<PathBuf> = self
            .visual_bounds()
            .map(|(lo, hi)| {
                (lo..=hi)
                    .filter_map(|i| self.entries.get(i).map(|e| e.path.clone()))
                    .collect()
            })
            .unwrap_or_default();
        // スコープ(全表示 or カーソルと同じ親)を追加。
        let parent = self
            .entries
            .get(self.selected)
            .and_then(|e| e.path.parent().map(|p| p.to_path_buf()));
        for e in &self.entries {
            let same_parent = e.path.parent().map(|p| p.to_path_buf()) == parent;
            if all_displayed || same_parent {
                paths.push(e.path.clone());
            }
        }
        for p in paths {
            self.selection.insert(p);
        }
        self.visual_anchor = None;
    }
    /// Whether to render the marker column (leftmost 2 cells): there is a committed selection, or visual mode is active.
    pub fn show_selection_gutter(&self) -> bool {
        !self.selection.is_empty() || self.is_visual()
    }
    /// The count currently marked (committed ∪ visual range). For the context's `sel: N` display.
    pub fn marked_count(&self) -> usize {
        match self.visual_bounds() {
            Some((lo, hi)) => {
                let mut n = self.selection.len();
                for i in lo..=hi {
                    if let Some(e) = self.entries.get(i) {
                        if !self.selection.contains(&e.path) {
                            n += 1;
                        }
                    }
                }
                n
            }
            None => self.selection.len(),
        }
    }
    /// Target paths for a batch operation: if there is a selection, **all selected items** (path ascending); otherwise, the single item at the cursor.
    pub(super) fn op_targets(&self) -> Vec<PathBuf> {
        if self.selection.is_empty() {
            self.entries
                .get(self.selected)
                .map(|e| vec![e.path.clone()])
                .unwrap_or_default()
        } else {
            self.selection.iter().cloned().collect()
        }
    }
}
