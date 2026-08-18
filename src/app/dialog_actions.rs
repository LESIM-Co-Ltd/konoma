use super::*;

impl App {
    /// A mutable reference to the (buffer, cursor) being edited. None if not an input dialog.
    fn dialog_input_mut(&mut self) -> Option<(&mut String, &mut usize)> {
        match self.dialog.as_mut() {
            Some(Dialog {
                kind: DialogKind::Input { buffer, cursor, .. },
                ..
            }) => Some((buffer, cursor)),
            _ => None,
        }
    }

    /// Insert one character at the cursor position in the input dialog and advance the cursor by one.
    pub fn dialog_input_push(&mut self, c: char) {
        if let Some((buffer, cursor)) = self.dialog_input_mut() {
            let at = char_byte(buffer, *cursor);
            buffer.insert(at, c);
            *cursor += 1;
        }
    }
    /// Delete the character just before the cursor (Backspace).
    pub fn dialog_input_backspace(&mut self) {
        if let Some((buffer, cursor)) = self.dialog_input_mut() {
            if *cursor > 0 {
                let start = char_byte(buffer, *cursor - 1);
                let end = char_byte(buffer, *cursor);
                buffer.replace_range(start..end, "");
                *cursor -= 1;
            }
        }
    }
    /// Delete the character at the cursor position (Delete). The cursor does not move.
    pub fn dialog_input_delete(&mut self) {
        if let Some((buffer, cursor)) = self.dialog_input_mut() {
            let count = buffer.chars().count();
            if *cursor < count {
                let start = char_byte(buffer, *cursor);
                let end = char_byte(buffer, *cursor + 1);
                buffer.replace_range(start..end, "");
            }
        }
    }
    /// Move the cursor left (←).
    pub fn dialog_cursor_left(&mut self) {
        if let Some((_, cursor)) = self.dialog_input_mut() {
            *cursor = cursor.saturating_sub(1);
        }
    }
    /// Move the cursor right (→). Clamped at the end.
    pub fn dialog_cursor_right(&mut self) {
        if let Some((buffer, cursor)) = self.dialog_input_mut() {
            *cursor = (*cursor + 1).min(buffer.chars().count());
        }
    }
    /// Move the cursor to the start (Home).
    pub fn dialog_cursor_home(&mut self) {
        if let Some((_, cursor)) = self.dialog_input_mut() {
            *cursor = 0;
        }
    }
    /// Move the cursor to the end (End).
    pub fn dialog_cursor_end(&mut self) {
        if let Some((buffer, cursor)) = self.dialog_input_mut() {
            *cursor = buffer.chars().count();
        }
    }
    /// Cancel the dialog (Esc / confirm n).
    pub fn dialog_cancel(&mut self) {
        self.dialog = None;
    }

    /// Put an input dialog back up with `text` already in it and the cursor at its end.
    ///
    /// Used when a submit could not go through after `dialog_submit` had already taken the dialog:
    /// a git write is dispatched to a worker and can be **refused** (one runs at a time), which
    /// would otherwise discard a commit message the user just wrote. Also used by `apply_git_op`
    /// to hand a failed write's text back for a retry.
    pub(super) fn reopen_input(&mut self, op: PendingOp, title: crate::i18n::Msg, text: &str) {
        self.dialog = Some(Dialog {
            op,
            kind: DialogKind::Input {
                title: crate::i18n::tr(self.lang, title).into(),
                buffer: text.to_string(),
                cursor: text.chars().count(),
            },
        });
    }

    /// Handle a quit request (`q` at the top level / `Q` from anywhere). If `ui.confirm_quit` is on,
    /// open a yes/no confirmation dialog and return `true` (the caller must NOT quit yet). Otherwise
    /// return `false` (the caller quits immediately).
    ///
    /// **While a file operation is running the confirmation is shown regardless of the setting**:
    /// the worker thread is detached, so quitting mid-copy leaves a half-copied directory behind.
    /// The dialog only warns — `y` still quits (we never block the user from leaving).
    ///
    /// **A running git write warns the same way**, and for a sharper reason: quitting during a
    /// `commit`/`checkout`/`worktree add` orphans a `git` process that is still holding
    /// `.git/index.lock`, so the repository can be left mid-write.
    pub fn request_quit(&mut self) -> bool {
        let file_op_running = self.fileop_pending.is_some();
        let git_op_running = self.git_op_running();
        if !self.cfg.ui.confirm_quit && !file_op_running && !git_op_running {
            return false;
        }
        let mut message = crate::i18n::tr(self.lang, crate::i18n::Msg::QuitConfirm).to_string();
        if file_op_running {
            // Line 2 = the in-progress warning (the dialog renderer already supports `\n`-separated multi-line text).
            message.push('\n');
            message.push_str(crate::i18n::tr(
                self.lang,
                crate::i18n::Msg::QuitWhileFileOp,
            ));
        }
        if git_op_running {
            message.push('\n');
            message.push_str(crate::i18n::tr(self.lang, crate::i18n::Msg::QuitWhileGitOp));
        }
        self.dialog = Some(Dialog {
            op: PendingOp::Quit,
            kind: DialogKind::Confirm {
                message,
                allow_permanent: false,
            },
        });
        true
    }

    /// Confirm the text-input dialog (Enter). Performs create/rename.
    pub fn dialog_submit(&mut self) -> Result<()> {
        let Some(dialog) = self.dialog.take() else {
            return Ok(());
        };
        let DialogKind::Input { buffer, .. } = dialog.kind else {
            return Ok(()); // a confirmation dialog never reaches here
        };
        // Commit is handled differently from a name (we don't strip a trailing / and use its own emptiness check), so branch on it first.
        if matches!(dialog.op, PendingOp::GitCommit) {
            let message = buffer.trim();
            if message.is_empty() {
                self.flash =
                    Some(crate::i18n::tr(self.lang, crate::i18n::Msg::MessageEmpty).into());
                return Ok(());
            }
            // Runs in the background (design principle #4): `git commit` runs the repository's
            // hooks, and a `pre-commit` that lints a large tree takes as long as it takes. The
            // success path (refresh + view reload) and the failure path (reopen this dialog with
            // the message so it can be retried) both move to `apply_git_op`.
            // `dialog` was taken at the top of this function, so a refused dispatch (another git
            // write already in flight) would silently swallow what the user typed. Put the prompt
            // back with the text intact — the flash `start_git_op` set explains why nothing happened.
            if !self.start_git_op(crate::app::GitOpJob {
                kind: crate::app::GitOpKind::Commit,
                root: PathBuf::new(), // filled in by start_git_op with tab.root at dispatch time
                tab_id: 0,            // likewise filled in by start_git_op
                path: PathBuf::new(),
                text: message.to_string(),
                flag: false,
                label: String::new(),
                count: 0,
            }) {
                self.reopen_input(
                    PendingOp::GitCommit,
                    crate::i18n::Msg::CommitMessage,
                    message,
                );
            }
            return Ok(());
        }
        // Creating a new branch (like commit, just trim the name and use it as-is).
        if matches!(dialog.op, PendingOp::GitCreateBranch) {
            let bname = buffer.trim();
            if bname.is_empty() {
                self.flash = Some(crate::i18n::tr(self.lang, crate::i18n::Msg::NameEmpty).into());
                return Ok(());
            }
            // Runs in the background, same as checkout (it is a `git switch -c`, so it rewrites the
            // working tree too). Success/failure handling moves to `apply_git_op`.
            if !self.start_git_op(crate::app::GitOpJob {
                kind: crate::app::GitOpKind::CreateBranch,
                root: PathBuf::new(), // filled in by start_git_op with tab.root at dispatch time
                tab_id: 0,            // likewise filled in by start_git_op
                path: PathBuf::new(),
                text: bname.to_string(),
                flag: false,
                label: bname.to_string(),
                count: 0,
            }) {
                // Same as commit above: a refused dispatch must not eat the typed name.
                self.reopen_input(
                    PendingOp::GitCreateBranch,
                    crate::i18n::Msg::NewBranch,
                    bname,
                );
            }
            return Ok(());
        }
        // Creating a linked worktree: the typed name is a branch — new-vs-existing is auto-detected
        // (no separate prompt for it), then placed under `[git] worktree_dir` next to the main
        // worktree (`worktree_create_target`) and switched into (same "root changed entirely, don't
        // return to the hub" treatment as `worktree_goto`'s `Go` arm, not `close_git_worktrees`).
        if matches!(dialog.op, PendingOp::WorktreeCreate) {
            let bname = buffer.trim();
            if bname.is_empty() {
                self.flash = Some(crate::i18n::tr(self.lang, crate::i18n::Msg::NameEmpty).into());
                return Ok(());
            }
            let target = self.worktree_create_target(bname);
            // New-vs-existing is probed here, on the UI thread, and the answer is handed to the
            // worker: it's a cheap libgit2 ref lookup (not a process), and it belongs with the rest
            // of the dispatch-time decisions.
            let create_new = crate::git::branch_tip(&self.tab.root, bname).is_none();
            // Runs in the background (design principle #4): `worktree add` writes out a whole new
            // checkout. Landing in the new worktree happens in `apply_git_op`.
            if !self.start_git_op(crate::app::GitOpJob {
                kind: crate::app::GitOpKind::WorktreeAdd,
                root: PathBuf::new(), // filled in by start_git_op with tab.root at dispatch time
                tab_id: 0,            // likewise filled in by start_git_op
                path: target,
                text: bname.to_string(),
                flag: create_new,
                label: bname.to_string(),
                count: 0,
            }) {
                // Same as commit/branch above: a refused dispatch must not eat the typed name.
                self.reopen_input(
                    PendingOp::WorktreeCreate,
                    crate::i18n::Msg::NewWorktree,
                    bname,
                );
            }
            return Ok(());
        }
        let name = buffer.trim().trim_end_matches('/').trim();
        let want_folder = buffer.trim_end().ends_with('/');
        if name.is_empty() {
            self.flash = Some(crate::i18n::tr(self.lang, crate::i18n::Msg::NameEmpty).into());
            return Ok(());
        }
        match dialog.op {
            PendingOp::Create { dir } => {
                let created = if want_folder {
                    crate::fileops::create_dir(&dir, name)
                } else {
                    crate::fileops::create_file(&dir, name)
                };
                match created {
                    Ok(path) => {
                        self.refresh()?;
                        self.reveal_and_select(&path)?;
                        self.flash = Some(format!(
                            "{}: {}",
                            crate::i18n::tr(self.lang, crate::i18n::Msg::Created),
                            self.format_path(&path)
                        ));
                    }
                    Err(e) => {
                        self.flash = Some(format!(
                            "{}: {}",
                            crate::i18n::tr(self.lang, crate::i18n::Msg::Failed),
                            self.describe_error(&e)
                        ))
                    }
                }
            }
            PendingOp::Rename { target } => match crate::fileops::rename(&target, name) {
                Ok(path) => {
                    self.refresh()?;
                    self.reveal_and_select(&path)?;
                    self.flash = Some(format!(
                        "{}: {}",
                        crate::i18n::tr(self.lang, crate::i18n::Msg::Renamed),
                        self.format_path(&path)
                    ));
                }
                Err(e) => {
                    self.flash = Some(format!(
                        "{}: {}",
                        crate::i18n::tr(self.lang, crate::i18n::Msg::Failed),
                        self.describe_error(&e)
                    ))
                }
            },
            PendingOp::BatchRenameInput { targets } => {
                match build_rename_plan(&targets, name, self.lang) {
                    Ok(plan) => {
                        // Move to the preview (old → new). Applying it happens in dialog_preview_apply.
                        let lines: Vec<String> = plan
                            .iter()
                            .map(|(s, d)| {
                                format!(
                                    "{}  →  {}",
                                    s.file_name().and_then(|n| n.to_str()).unwrap_or("?"),
                                    d.file_name().and_then(|n| n.to_str()).unwrap_or("?"),
                                )
                            })
                            .collect();
                        let title = format!(
                            "{} {} {}",
                            crate::i18n::tr(self.lang, crate::i18n::Msg::Rename),
                            plan.len(),
                            crate::i18n::tr(self.lang, crate::i18n::Msg::Items),
                        );
                        self.dialog = Some(Dialog {
                            op: PendingOp::BatchRenameApply { plan },
                            kind: DialogKind::Preview {
                                title,
                                lines,
                                scroll: 0,
                            },
                        });
                    }
                    Err(e) => {
                        // Reopen the input dialog with the same template so the input can be retried.
                        self.flash = Some(format!(
                            "{}: {}",
                            crate::i18n::tr(self.lang, crate::i18n::Msg::Failed),
                            self.describe_error(&e)
                        ));
                        let cursor = name.chars().count();
                        let title = self.batch_rename_title(targets.len());
                        self.dialog = Some(Dialog {
                            op: PendingOp::BatchRenameInput { targets },
                            kind: DialogKind::Input {
                                title,
                                buffer: name.to_string(),
                                cursor,
                            },
                        });
                    }
                }
            }
            // Delete/Git discard live on the confirmation-dialog side / batch rename apply lives on the preview side.
            // GitCommit/GitCreateBranch/WorktreeCreate already returned early above, so they never reach here.
            PendingOp::Delete { .. }
            | PendingOp::BatchRenameApply { .. }
            | PendingOp::GitDiscard { .. }
            | PendingOp::GitCommit
            | PendingOp::GitCreateBranch
            | PendingOp::WorktreeCreate
            | PendingOp::GitDeleteBranch { .. }
            | PendingOp::DropTransfer { .. }
            | PendingOp::BookmarkOverwrite { .. }
            | PendingOp::JjSync
            | PendingOp::Quit => {}
        }
        Ok(())
    }

    /// In the preview, `y`=apply the batch rename. Two-phase rename → reload → clear selection.
    pub fn dialog_preview_apply(&mut self) -> Result<()> {
        let Some(dialog) = self.dialog.take() else {
            return Ok(());
        };
        if let PendingOp::BatchRenameApply { plan } = dialog.op {
            match crate::fileops::batch_rename(&plan) {
                Ok(()) => {
                    self.refresh()?;
                    self.clear_selection();
                    self.flash = Some(format!(
                        "{} ({})",
                        crate::i18n::tr(self.lang, crate::i18n::Msg::Renamed),
                        plan.len()
                    ));
                }
                Err(e) => {
                    self.flash = Some(format!(
                        "{}: {}",
                        crate::i18n::tr(self.lang, crate::i18n::Msg::Failed),
                        self.describe_error(&e)
                    ))
                }
            }
        }
        Ok(())
    }

    /// Response to a confirmation dialog. yes=execute (delete → trash, recoverable) / no=cancel.
    pub fn dialog_confirm(&mut self, yes: bool) -> Result<()> {
        let Some(dialog) = self.dialog.take() else {
            return Ok(());
        };
        if !yes {
            self.flash = Some(crate::i18n::tr(self.lang, crate::i18n::Msg::Canceled).into());
            return Ok(());
        }
        match dialog.op {
            PendingOp::Delete { targets } => {
                // Delete (to trash) runs in the background too, same as other bulk file operations (principle #4).
                let job = FileOpJob {
                    kind: FileOpKind::Trash,
                    targets,
                    dest: None,
                    root: PathBuf::new(), // start_file_op fills this with tab.root at dispatch time
                    err_self_paste: String::new(),
                    err_failed: crate::i18n::tr(self.lang, crate::i18n::Msg::OperationFailed)
                        .to_string(),
                };
                // Selection is cleared **only on success** (`apply_file_op`). If a partially-failed
                // delete also lost the selection, you'd have to redo the selection from scratch
                // (the old synchronous version also cleared it only inside the Ok(()) arm).
                self.start_file_op(job);
            }
            // Git-view discard: git::discard → re-fetch the git status for the list/tree.
            // Runs in the background too (design principle #4) — restoring a large file from the
            // index is a working-tree write like any other. Returning from the GitDiff preview to
            // the Git view (came_from_git_view) happens in `apply_git_op`.
            #[cfg(feature = "git")]
            PendingOp::JjSync => self.jj_run_sync(),
            #[cfg(not(feature = "git"))]
            PendingOp::JjSync => {}
            PendingOp::GitDiscard { path } => {
                let label = self.format_path(&path);
                self.start_git_op(crate::app::GitOpJob {
                    kind: crate::app::GitOpKind::Discard,
                    root: PathBuf::new(), // start_git_op fills this with tab.root at dispatch time
                    tab_id: 0,            // likewise filled in by start_git_op
                    path,
                    text: String::new(),
                    flag: false,
                    label,
                    count: 0,
                });
            }
            // Delete branch (safe): git branch -d. On failure (unmerged etc.) flash git's stderr.
            PendingOp::GitDeleteBranch { name } => self.git_delete_branch(&name, false),
            // Confirming a bookmark overwrite: actually register it (same path as mark_input).
            PendingOp::BookmarkOverwrite { key, target } => self.perform_mark_set(key, target),
            _ => {}
        }
        Ok(())
    }

    /// In a confirmation dialog, `!`=**permanent delete / force** (delete=without going through the trash / branch=`-D` force).
    pub fn dialog_delete_permanent(&mut self) -> Result<()> {
        let Some(dialog) = self.dialog.take() else {
            return Ok(());
        };
        // Force-delete the branch (`-D`).
        if let PendingOp::GitDeleteBranch { name } = &dialog.op {
            self.git_delete_branch(&name.clone(), true);
            return Ok(());
        }
        if let PendingOp::Delete { targets } = dialog.op {
            // Permanent delete also runs in the background, same as other bulk file operations (principle #4).
            let job = FileOpJob {
                kind: FileOpKind::DeletePermanent,
                targets,
                dest: None,
                root: PathBuf::new(), // start_file_op fills this with tab.root at dispatch time
                err_self_paste: String::new(),
                err_failed: crate::i18n::tr(self.lang, crate::i18n::Msg::OperationFailed)
                    .to_string(),
            };
            // Selection is cleared **only on success** (`apply_file_op`). A partial failure never loses the selection.
            self.start_file_op(job);
        }
        Ok(())
    }
}
