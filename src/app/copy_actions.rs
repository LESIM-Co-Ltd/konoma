use super::*;

#[cfg(test)]
thread_local! {
    static COPY_FAILURE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn write_copy_clipboard(text: &str) -> Result<()> {
    #[cfg(test)]
    if COPY_FAILURE.with(|fail| fail.get()) {
        anyhow::bail!("test clipboard failure");
    }
    set_clipboard(text)
}

impl App {
    #[cfg(all(test, feature = "git"))]
    pub(crate) fn with_clipboard_failure_for_test(f: impl FnOnce()) {
        struct Reset(bool);
        impl Drop for Reset {
            fn drop(&mut self) {
                COPY_FAILURE.with(|fail| fail.set(self.0));
            }
        }
        let _reset = Reset(COPY_FAILURE.with(|fail| fail.replace(true)));
        f();
    }
    /// The path to copy. In Preview it is the preview target; in Tree it is the entry selected in the tree.
    pub(super) fn copy_target(&self) -> Option<PathBuf> {
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
            self.flash = Some(konoma_host_core::copy::no_target_notice(self.lang).into());
            return;
        };
        let text = copy_text(&path, &self.tab.open_dir, kind);
        match set_clipboard(&text) {
            Ok(()) => self.flash = Some(konoma_host_core::copy::copied_notice(self.lang, &text)),
            Err(e) => self.flash = Some(konoma_host_core::copy::copy_failed_notice(self.lang, &e)),
        }
    }

    /// Test-only: the exact string `copy_path(kind)` would place on the clipboard for the current
    /// copy target (tree selection / preview path). `None` when there is no
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

    /// An old hub name that copies the selected commit's info. No screen of this app has a commit
    /// row under the cursor (the project's screens run their own copy operations), so it answers
    /// that there is nothing to copy.
    #[cfg(feature = "git")]
    pub fn git_copy(&mut self, _kind: GitCopyKind) {
        self.flash = Some(tr(self.lang, crate::i18n::Msg::NoCommitToCopy).into());
    }

    /// An old hub name that copies the selected branch's name; there is no branch row to copy.
    #[cfg(feature = "git")]
    pub fn git_copy_branch_name(&mut self) {
        self.flash = Some(tr(self.lang, crate::i18n::Msg::NoCopyTarget).into());
    }

    /// Write to the clipboard and flash success/failure (common processing for copy operations). On success, shows a one-line preview
    /// (multi-line content such as a full message is rounded to the first line + `…` so the footer does not overflow).
    pub(super) fn set_clipboard_flash(&mut self, text: &str) {
        match write_copy_clipboard(text) {
            Ok(()) => {
                self.flash = Some(konoma_host_core::copy::copied_preview_notice(
                    self.lang, text,
                ))
            }
            Err(e) => self.flash = Some(konoma_host_core::copy::copy_failed_notice(self.lang, &e)),
        }
    }
}
