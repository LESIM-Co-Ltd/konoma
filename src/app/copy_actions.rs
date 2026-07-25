use super::*;

impl App {
    /// The path to copy. In Preview it is the preview target; in Tree it is the entry selected in the tree.
    pub(super) fn copy_target(&self) -> Option<PathBuf> {
        // Git 変更ハブ表示中は、ツリーの選択ではなく**変更ファイル**のパスをコピー対象にする。
        #[cfg(feature = "git")]
        if self.surface() == crate::keymap::Surface::GitChanges {
            return self.git_view_selected();
        }
        match self.tab.mode {
            Mode::Preview => self.tab.preview_path.clone(),
            Mode::Tree => self
                .tab
                .entries
                .get(self.tab.selected)
                .map(|e| e.path.clone()),
        }
    }

    /// Copy the selected path to the clipboard according to the kind, and show the result in flash (FR-6).
    pub fn copy_path(&mut self, kind: CopyKind) {
        let Some(path) = self.copy_target() else {
            self.flash = Some(tr(self.lang, crate::i18n::Msg::NoCopyTarget).into());
            return;
        };
        let text = copy_text(&path, &self.tab.open_dir, kind);
        match set_clipboard(&text) {
            Ok(()) => {
                self.flash = Some(format!(
                    "{}{text}",
                    tr(self.lang, crate::i18n::Msg::CopiedPrefix)
                ))
            }
            Err(e) => {
                self.flash = Some(format!(
                    "{}{e}",
                    tr(self.lang, crate::i18n::Msg::CopyFailed)
                ))
            }
        }
    }

    /// Test-only: the exact string `copy_path(kind)` would place on the clipboard for the current
    /// copy target (tree selection / preview path / git-changes selection). `None` when there is no
    /// target — the same gate as `copy_path`. Lets E2E assert copy values without the (headless-flaky)
    /// clipboard round-trip.
    #[cfg(test)]
    pub fn copy_string_for(&self, kind: CopyKind) -> Option<String> {
        let path = self.copy_target()?;
        Some(copy_text(&path, &self.tab.open_dir, kind))
    }

    /// Test-only: the exact `@path#L..` string that `preview_copy_selection_ref` (`Y`) would copy.
    #[cfg(test)]
    pub fn selection_ref_string(&self) -> Option<String> {
        self.preview_selection_ref_text()
    }

    /// Get metadata for the selected commit in log/graph/detail. detail uses the already-loaded data, while log/graph
    /// fetch `commit_meta` from the selected commit id. None for non-commits (uncommitted rows, etc.) or out-of-scope surfaces.
    #[cfg(feature = "git")]
    pub(super) fn current_commit_meta(&self) -> Option<crate::git::CommitMeta> {
        use crate::keymap::Surface;
        match self.surface() {
            Surface::GitDetail => self.tab.git_detail_meta.clone(),
            Surface::GitLog => {
                let id = self.git_log_selected_id()?;
                crate::git::commit_meta(&self.tab.root, &id)
            }
            Surface::GitGraph => {
                let id = self
                    .git_graph_selected_row()
                    .and_then(|r| r.commit.clone())?;
                crate::git::commit_meta(&self.tab.root, &id)
            }
            _ => None,
        }
    }

    /// git log/graph/detail: copy the selected commit's info (short/full hash, subject, full message, author, date)
    /// to the clipboard. If there is no commit, notify via flash.
    #[cfg(feature = "git")]
    pub fn git_copy(&mut self, kind: GitCopyKind) {
        let Some(meta) = self.current_commit_meta() else {
            self.flash = Some(tr(self.lang, crate::i18n::Msg::NoCommitToCopy).into());
            return;
        };
        let text = match kind {
            GitCopyKind::ShortHash => meta.short,
            GitCopyKind::FullHash => meta.id,
            GitCopyKind::Subject => meta.message.lines().next().unwrap_or("").to_string(),
            GitCopyKind::Message => meta.message,
            GitCopyKind::Author => meta.author,
            GitCopyKind::Date => meta.date,
        };
        self.set_clipboard_flash(&text);
    }

    /// branches view: copy the selected branch name to the clipboard.
    #[cfg(feature = "git")]
    pub fn git_copy_branch_name(&mut self) {
        let Some(b) = self.git_branch_selected() else {
            self.flash = Some(tr(self.lang, crate::i18n::Msg::NoCopyTarget).into());
            return;
        };
        let name = b.name;
        self.set_clipboard_flash(&name);
    }

    /// Write to the clipboard and flash success/failure (common processing for copy operations). On success, shows a one-line preview
    /// (multi-line content such as a full message is rounded to the first line + `…` so the footer does not overflow).
    pub(super) fn set_clipboard_flash(&mut self, text: &str) {
        match set_clipboard(text) {
            Ok(()) => {
                let first = text.lines().next().unwrap_or("");
                let mut preview: String = first.chars().take(60).collect();
                if first.chars().count() > 60 || text.lines().nth(1).is_some() {
                    preview.push('…');
                }
                self.flash = Some(format!(
                    "{}{preview}",
                    tr(self.lang, crate::i18n::Msg::CopiedPrefix)
                ));
            }
            Err(e) => {
                self.flash = Some(format!(
                    "{}{e}",
                    tr(self.lang, crate::i18n::Msg::CopyFailed)
                ))
            }
        }
    }
}
