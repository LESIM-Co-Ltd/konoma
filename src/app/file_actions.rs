//! File-manager actions: create / rename / delete (trash) and multi-select / visual range — methods on `App`.

use super::*;

/// Translate an error that may have come from `fileops` into the language the UI is currently
/// showing. `fileops` doesn't know `Lang` (see `fileops::FileOpError`'s doc comment for why), so
/// this downcasts instead: anyhow keeps a `.context(FileOpError::…)`-wrapped variant reachable via
/// `downcast_ref` exactly like a directly-returned one (it checks both the context value and the
/// wrapped error), so this works everywhere `fileops` uses either style. Anything that isn't one of
/// ours (a plain io error, git's own `fatal:` text, …) falls through to its own `Display` unchanged
/// — that text was never Japanese-only to begin with, so there's nothing to translate.
///
/// A free function (not an `App` method) so `run_file_op` can call it from the background worker
/// thread, where there is no `&self` to read `lang` from — the caller (`start_file_op`) resolves
/// `self.lang` once, before dispatch, and threads it through (same idea as `err_self_paste`/
/// `err_failed` below, which are pre-translated for the same reason).
fn describe_file_op_error(lang: crate::i18n::Lang, e: &anyhow::Error) -> String {
    use crate::fileops::FileOpError;
    use crate::i18n::{tr, Msg};
    match e.downcast_ref::<FileOpError>() {
        Some(FileOpError::AlreadyExists(p)) => {
            format!("{}{}", tr(lang, Msg::AlreadyExists), p.display())
        }
        Some(FileOpError::TrashFailed) => tr(lang, Msg::TrashFailed).to_string(),
        Some(FileOpError::RenameTempExists(p)) => {
            format!("{}{}", tr(lang, Msg::RenameTempExists), p.display())
        }
        Some(FileOpError::RenameDestExists(p)) => {
            format!("{}{}", tr(lang, Msg::RenameDestExists), p.display())
        }
        Some(FileOpError::NameUnavailable(p)) => {
            format!("{}{}", tr(lang, Msg::NameUnavailable), p.display())
        }
        Some(FileOpError::RenameStageFailed(p)) => {
            format!("{}{}", tr(lang, Msg::RenameStageFailed), p.display())
        }
        Some(FileOpError::RenameCommitFailed(p)) => {
            format!("{}{}", tr(lang, Msg::RenameCommitFailed), p.display())
        }
        None => e.to_string(),
    }
}

impl App {
    /// User-facing text for an error that may have come from `fileops` (create/rename collisions,
    /// Trash failures, batch-rename issues), translated to whichever language the UI is currently
    /// showing (`self.lang`). Falls back to the error's own message when it isn't one of ours — see
    /// `describe_file_op_error` (this just supplies `self.lang` to it).
    pub fn describe_error(&self, e: &anyhow::Error) -> String {
        describe_file_op_error(self.lang, e)
    }

    // --- File operations: create/rename/delete (M7 Phase B, confirm + trash) -------------
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
    /// Whether it is a text-input dialog (the third kind, alongside confirm and the rename preview).
    pub fn dialog_is_input(&self) -> bool {
        matches!(
            self.dialog.as_ref().map(|d| &d.kind),
            Some(DialogKind::Input { .. })
        )
    }
    /// The pending operations that give an input dialog its own chip/footer wording (see `UiLayer`);
    /// any other operation behind an input dialog falls through to the rename wording.
    pub fn pending_is_create(&self) -> bool {
        matches!(
            self.dialog.as_ref().map(|d| &d.op),
            Some(PendingOp::Create { .. })
        )
    }
    pub fn pending_is_batch_rename_input(&self) -> bool {
        matches!(
            self.dialog.as_ref().map(|d| &d.op),
            Some(PendingOp::BatchRenameInput { .. })
        )
    }
    pub fn pending_is_git_commit(&self) -> bool {
        matches!(
            self.dialog.as_ref().map(|d| &d.op),
            Some(PendingOp::GitCommit)
        )
    }
    pub fn pending_is_git_create_branch(&self) -> bool {
        matches!(
            self.dialog.as_ref().map(|d| &d.op),
            Some(PendingOp::GitCreateBranch)
        )
    }
    pub fn pending_is_worktree_create(&self) -> bool {
        matches!(
            self.dialog.as_ref().map(|d| &d.op),
            Some(PendingOp::WorktreeCreate)
        )
    }

    /// Receive a paste from the terminal (including drag-and-drop). While input is active (dialog input / filter /
    /// search / branch filter), **insert text**; in Tree mode, if the dropped content is an existing path,
    /// open a **copy/move dialog** (drop target = the cursor's base directory). Control characters are stripped.
    pub fn handle_paste(&mut self, text: String) {
        // 1) While text input is active: feed it straight into the input field (control
        // characters are dropped).
        if self.is_text_input_active() {
            for ch in text.chars().filter(|c| !c.is_control()) {
                self.input_push_char(ch);
            }
            return;
        }
        // 2) Only Tree mode treats this as a "drop" (ignored in Preview, etc.).
        if !matches!(self.tab.mode, Mode::Tree) {
            return;
        }
        let sources = parse_dropped_paths(&text);
        if sources.is_empty() {
            return; // do nothing if there's no drop target (an existing path)
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
    /// `pub(super)` so `apply_git_op` can ask the same question before restoring a dialog.
    pub(super) fn is_text_input_active(&self) -> bool {
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
            root: PathBuf::new(), // filled in by start_file_op with tab.root at dispatch time
            err_self_paste: String::new(), // Drop doesn't use a dedicated self-paste message (relies on copy_dir_all's generic error = existing behavior)
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
    ///
    /// A background **git write** is held back for the same reason: `checkout`/`discard`/`worktree
    /// add` rewrite the working tree, and a `pre-commit` hook can churn through it too. Nothing is
    /// missed — `apply_git_op` refreshes on completion, and the deferred burst is flushed right after.
    pub fn should_defer_fs_events(&self) -> bool {
        self.fileop_pending.is_some() || self.git_op_running()
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
        // Bake in "which tab this was dispatched from" here at dispatch time (the caller passes
        // it empty). The active tab when it completes may not be the same one, so apply_file_op
        // uses this to decide.
        job.root = self.tab.root.clone();
        self.fileop_gen = self.fileop_gen.wrapping_add(1);
        let gen = self.fileop_gen;
        self.fileop_pending = Some(job.kind);
        self.fileop_total = job.targets.len();
        let progress = Arc::new(crate::fileops::Progress::default());
        self.fileop_progress = Some(progress.clone());

        // Resolve `self.lang` here (on the UI thread) ahead of time: the worker thread has no
        // `&self`, so `run_file_op` can't call `tr()`/`describe_file_op_error` without it.
        let lang = self.lang;
        let Some(tx) = self.fileop_tx.clone() else {
            let res = Self::run_file_op(gen, job, &progress, lang);
            self.apply_file_op(res);
            return true;
        };
        let kind = job.kind;
        let root = job.root.clone();
        // Translate the fallback panic error using self.lang here (on the UI thread) ahead of
        // time (the worker thread has no &self, so it cannot call tr()).
        let panic_err = crate::i18n::tr(self.lang, crate::i18n::Msg::OperationFailed).to_string();
        std::thread::spawn(move || {
            // If the worker panics, no result comes back and `fileop_pending` stays set forever,
            // leaving the spinner spinning. Use the same safety net as the other workers to
            // always return a result (design principle #3).
            let res = crate::preview::markdown::catch_silent(|| {
                Self::run_file_op(gen, job, &progress, lang)
            })
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
    /// `DropCopy`/`DropMove` stop at the first error (existing `drop_apply` behaviour). `lang` is
    /// resolved by the caller (`start_file_op`) — see `describe_file_op_error`'s doc comment for why.
    fn run_file_op(
        gen: u64,
        job: FileOpJob,
        p: &crate::fileops::Progress,
        lang: crate::i18n::Lang,
    ) -> FileOpResult {
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
                    // Reject pasting into itself (or inside itself), since that would become an
                    // infinite copy.
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
                        Err(e) => err = Some(describe_file_op_error(lang, &e)),
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
                            err = Some(describe_file_op_error(lang, &e));
                            break; // a drop stops at the first failure (same as the existing drop_apply)
                        }
                    }
                }
            }
            FileOpKind::Duplicate => {
                for src in &targets {
                    // Parent directory = the duplication target (duplicate in place). If there's
                    // no parent (e.g. root), treat it as a failure (no crash).
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
                        Err(e) => err = Some(describe_file_op_error(lang, &e)),
                    }
                    p.bump_item();
                }
            }
            // Sending to trash is **a single call** to `trash::delete_all`, so no progress can be
            // read mid-flight. It only advances by the full count once done (progress stays at
            // 0/M until it jumps to M/M at the end).
            FileOpKind::Trash => match crate::fileops::move_to_trash(&targets) {
                Ok(()) => {
                    ok = targets.len();
                    for _ in 0..ok {
                        p.bump_item();
                    }
                }
                Err(e) => {
                    // `move_to_trash` doesn't say which (if any) targets it got to before the
                    // error — observe the filesystem instead of assuming zero succeeded (see
                    // `trash_partial_outcome`'s doc comment). Without this, a batch that actually
                    // removed some targets was reported as a flat "Failed", leaving the user
                    // thinking nothing happened when part of the batch is already gone.
                    let (found_ok, still_present) = crate::fileops::trash_partial_outcome(&targets);
                    ok = found_ok;
                    for _ in 0..ok {
                        p.bump_item();
                    }
                    let mut msg = describe_file_op_error(lang, &e);
                    if let Some(remaining) = still_present {
                        msg = format!("{msg}: {}", remaining.display());
                    }
                    err = Some(msg);
                }
            },
            // Permanent deletion loops per path, so progress can advance one item at a time
            // (`_with_progress`).
            FileOpKind::DeletePermanent => {
                match crate::fileops::delete_permanently_with_progress(&targets, p) {
                    Ok(()) => ok = targets.len(),
                    Err(e) => {
                        // Targets before the one that failed were already removed **for good**
                        // and bumped into `p` by `delete_permanently_with_progress`'s per-path
                        // loop (it returns via `?` on the first failure) — read that count back
                        // instead of leaving `ok` at its initial 0, or the flash claims an
                        // irreversible delete "failed" outright when part of the batch is already
                        // unrecoverably gone.
                        ok = p.items();
                        err = Some(describe_file_op_error(lang, &e));
                    }
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
        // Reload unconditionally (the disk has actually changed regardless of which tab is being
        // viewed).
        let mut post = self.refresh();
        // Reveal (= move the cursor) only **when we're back on the tab that dispatched it**.
        // `reveal_and_select` unconditionally calls rebuild_tree, so doing it on another tab
        // would collapse its filter (`/`) or changed view (`C`) and even move that tab's cursor.
        // Compare by root, not tab number (tabs can be closed or reordered mid-operation, so a
        // number is not grounds for identity).
        if post.is_ok() && res.root == self.tab.root {
            if let Some(p) = &res.last {
                post = self.reveal_and_select(p);
            }
        }
        // Clear the selection for delete-type operations **only on success** (same as the old
        // synchronous version). If a partial failure — such as a permission error on the 3rd of
        // 12 items — also wiped the selection, the user would have to start over from scratch.
        // Same root guard as `reveal_and_select` above, and for the same reason: `self.tab` is
        // whichever tab happens to be active when the result arrives, which may not be the one
        // that dispatched the delete. Without this guard, finishing a delete in tab A could wipe a
        // selection the user is still building in tab B. Leaving the originating tab's now-defunct
        // selection alone here is fine — `refresh_fs_inner`'s `self.tab.selection.retain(...)`
        // (called on every tab switch via `refresh_fs_after_tab_switch`) prunes paths that no
        // longer exist, so it self-heals the moment that tab becomes active again.
        if res.err.is_none()
            && matches!(res.kind, FileOpKind::Trash | FileOpKind::DeletePermanent)
            && res.root == self.tab.root
        {
            self.clear_selection();
        }
        let lang = self.lang;
        // If the operation itself succeeded but a later step (reload/reveal) fails, surface that
        // failure. The old synchronous version propagated `refresh()?` / `reveal_and_select(p)?`
        // up to handle_key, where `resolve_key_result` flashed it — so don't paper over it with a
        // success flash (principle: don't call something a success unverified).
        // If the operation itself failed, that error is the real story, so leave it to the branch
        // below.
        if res.err.is_none() {
            if let Err(e) = post {
                self.flash = Some(format!(
                    "{}: {}",
                    crate::i18n::tr(lang, crate::i18n::Msg::Failed),
                    self.describe_error(&e)
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
            // A failure with `res.ok > 0` uses the same "<verb> N / Failed: <e>" shape as
            // PasteCopy/Duplicate above — some targets are genuinely gone (trashed or, for
            // DeletePermanent, unrecoverably deleted), so saying only "Failed" would be a lie by
            // omission. `res.ok == 0` keeps the plain "Failed: <e>" (no count to show).
            FileOpKind::Trash => match &res.err {
                Some(e) if res.ok > 0 => format!(
                    "{} {} / {}: {e}",
                    crate::i18n::tr(lang, crate::i18n::Msg::MovedToTrash),
                    res.ok,
                    crate::i18n::tr(lang, crate::i18n::Msg::Failed),
                ),
                Some(e) => format!("{}: {e}", crate::i18n::tr(lang, crate::i18n::Msg::Failed)),
                None => format!(
                    "{} ({})",
                    crate::i18n::tr(lang, crate::i18n::Msg::MovedToTrash),
                    res.ok
                ),
            },
            FileOpKind::DeletePermanent => match &res.err {
                Some(e) if res.ok > 0 => format!(
                    "{} {} / {}: {e}",
                    crate::i18n::tr(lang, crate::i18n::Msg::DeletedPermanently),
                    res.ok,
                    crate::i18n::tr(lang, crate::i18n::Msg::Failed),
                ),
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
        // Shallow-to-deep, from directly under root to target's parent. Setting expanded and
        // rebuilding reveals the next level.
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

    // --- Multi-select + visual (range) selection (M7 Phase B) ------------------------
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
        // Commit the in-progress range.
        let mut paths: Vec<PathBuf> = self
            .visual_bounds()
            .map(|(lo, hi)| {
                (lo..=hi)
                    .filter_map(|i| self.tab.entries.get(i).map(|e| e.path.clone()))
                    .collect()
            })
            .unwrap_or_default();
        // Add the scope (everything displayed, or the same parent as the cursor).
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
