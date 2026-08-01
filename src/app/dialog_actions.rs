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

    /// Handle a quit request (`q` at the top level / `Q` from anywhere). If `ui.confirm_quit` is on,
    /// open a yes/no confirmation dialog and return `true` (the caller must NOT quit yet). Otherwise
    /// return `false` (the caller quits immediately).
    ///
    /// **While a file operation is running the confirmation is shown regardless of the setting**:
    /// the worker thread is detached, so quitting mid-copy leaves a half-copied directory behind.
    /// The dialog only warns — `y` still quits (we never block the user from leaving).
    pub fn request_quit(&mut self) -> bool {
        let file_op_running = self.fileop_pending.is_some();
        if !self.cfg.ui.confirm_quit && !file_op_running {
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
            match crate::git::commit(&self.tab.root, message) {
                Ok(()) => {
                    // Commit succeeded against the staged index → re-fetch git data and update the view.
                    self.refresh()?;
                    if self.is_git_view() {
                        self.git_view_reload();
                    }
                    self.flash =
                        Some(crate::i18n::tr(self.lang, crate::i18n::Msg::Committed).into());
                }
                Err(e) => {
                    // Show the failure (stderr) and reopen the input dialog with the same message (so it can be retried).
                    self.flash = Some(self.describe_error(&e));
                    let cursor = message.chars().count();
                    self.dialog = Some(Dialog {
                        op: PendingOp::GitCommit,
                        kind: DialogKind::Input {
                            title: crate::i18n::tr(self.lang, crate::i18n::Msg::CommitMessage)
                                .into(),
                            buffer: message.to_string(),
                            cursor,
                        },
                    });
                }
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
            match crate::git::create_branch(&self.tab.root, bname) {
                Ok(()) => {
                    self.refresh()?;
                    self.ensure_git_status_now(); // update branch name/status immediately (correct even before the next draw)
                    self.close_git_branches(); // created & switched → close the list and go to the Git view
                    self.flash = Some(format!(
                        "{}: {bname}",
                        crate::i18n::tr(self.lang, crate::i18n::Msg::CreatedBranch)
                    ));
                }
                Err(e) => {
                    self.flash = Some(self.describe_error(&e));
                    let cursor = bname.chars().count();
                    self.dialog = Some(Dialog {
                        op: PendingOp::GitCreateBranch,
                        kind: DialogKind::Input {
                            title: crate::i18n::tr(self.lang, crate::i18n::Msg::NewBranch).into(),
                            buffer: bname.to_string(),
                            cursor,
                        },
                    });
                }
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
            let create_new = crate::git::branch_tip(&self.tab.root, bname).is_none();
            match crate::git::worktree_add(&self.tab.root, &target, bname, create_new) {
                Ok(()) => {
                    // The target now exists on disk, so canonicalize it for a clean root/open_dir/
                    // flash (`worktree_dir` can carry a literal `..` component, e.g. the default "../").
                    let path = target.canonicalize().unwrap_or(target);
                    self.reset_git_worktree_list_state();
                    self.jump_to_dir(path.clone());
                    self.tab.open_dir = path.clone();
                    self.flash = Some(format!(
                        "{}: {}",
                        crate::i18n::tr(self.lang, crate::i18n::Msg::CreatedWorktree),
                        home_relative(&path)
                    ));
                }
                // git's own `fatal: ...` message, shown unmodified (same contract as branch
                // creation above). `self.tab.git_worktrees` was never touched here, so the list is
                // still showing once this (now-closed) dialog is gone.
                Err(e) => self.flash = Some(self.describe_error(&e)),
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
            // If discarded from the GitDiff preview (came_from_git_view), reopen the Git view to return to it.
            PendingOp::GitDiscard { path } => match crate::git::discard(&self.tab.root, &path) {
                Ok(()) => {
                    let from_diff = self.is_git_diff_preview();
                    if from_diff {
                        // Collapse the preview and go back to the Git view.
                        self.tab.came_from_git_view = false;
                        self.back_to_tree();
                        self.open_git_view();
                    }
                    self.git_view_reload();
                    // The discard itself succeeded. If the tree rebuild fails, notify about that
                    // and don't overwrite it with a success flash (never falsely show "success").
                    if self.rebuild_tree_notify() {
                        self.flash = Some(format!(
                            "{}: {}",
                            crate::i18n::tr(self.lang, crate::i18n::Msg::Discarded),
                            self.format_path(&path)
                        ));
                    }
                }
                Err(e) => {
                    self.flash = Some(format!(
                        "{}: {}",
                        crate::i18n::tr(self.lang, crate::i18n::Msg::Failed),
                        self.describe_error(&e)
                    ))
                }
            },
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
