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
            // 2行目=実行中の警告(ダイアログ描画は `\n` 区切りの複数行に対応済み)。
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
            return Ok(()); // 確認ダイアログはここへ来ない
        };
        // コミットは名前と扱いが違う(末尾 / を剥がさない・独自の空チェック)ので先に分岐。
        if matches!(dialog.op, PendingOp::GitCommit) {
            let message = buffer.trim();
            if message.is_empty() {
                self.flash =
                    Some(crate::i18n::tr(self.lang, crate::i18n::Msg::MessageEmpty).into());
                return Ok(());
            }
            match crate::git::commit(&self.tab.root, message) {
                Ok(()) => {
                    // ステージ済み index でコミット成功 → git データ再取得＋ビュー更新。
                    self.refresh()?;
                    if self.is_git_view() {
                        self.git_view_reload();
                    }
                    self.flash =
                        Some(crate::i18n::tr(self.lang, crate::i18n::Msg::Committed).into());
                }
                Err(e) => {
                    // 失敗(stderr)を表示し、同じメッセージで入力ダイアログを再オープン(やり直せる)。
                    self.flash = Some(format!("{e}"));
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
        // 新規ブランチ作成(コミット同様、名前を素直に trim して扱う)。
        if matches!(dialog.op, PendingOp::GitCreateBranch) {
            let bname = buffer.trim();
            if bname.is_empty() {
                self.flash = Some(crate::i18n::tr(self.lang, crate::i18n::Msg::NameEmpty).into());
                return Ok(());
            }
            match crate::git::create_branch(&self.tab.root, bname) {
                Ok(()) => {
                    self.refresh()?;
                    self.ensure_git_status_now(); // ブランチ名/状態を即更新(描画前でも正)
                    self.close_git_branches(); // 作成＆切替済み → 一覧を閉じて Git ビューへ
                    self.flash = Some(format!(
                        "{}: {bname}",
                        crate::i18n::tr(self.lang, crate::i18n::Msg::CreatedBranch)
                    ));
                }
                Err(e) => {
                    self.flash = Some(format!("{e}"));
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
                            "{}: {e}",
                            crate::i18n::tr(self.lang, crate::i18n::Msg::Failed)
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
                        "{}: {e}",
                        crate::i18n::tr(self.lang, crate::i18n::Msg::Failed)
                    ))
                }
            },
            PendingOp::BatchRenameInput { targets } => {
                match build_rename_plan(&targets, name) {
                    Ok(plan) => {
                        // プレビュー(旧 → 新)へ遷移。適用は dialog_preview_apply。
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
                        // 入力をやり直せるよう、同じテンプレで入力ダイアログを再オープン。
                        self.flash = Some(format!(
                            "{}: {e}",
                            crate::i18n::tr(self.lang, crate::i18n::Msg::Failed)
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
            // 削除/Git 破棄は確認ダイアログ側 / 一括リネーム適用はプレビュー側。
            // GitCommit は上で早期 return 済みなので到達しない。
            PendingOp::Delete { .. }
            | PendingOp::BatchRenameApply { .. }
            | PendingOp::GitDiscard { .. }
            | PendingOp::GitCommit
            | PendingOp::GitCreateBranch
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
                        "{}: {e}",
                        crate::i18n::tr(self.lang, crate::i18n::Msg::Failed)
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
                // 削除(ゴミ箱)も他の一括ファイル操作と同じくバックグラウンドで実行する(原則#4)。
                let job = FileOpJob {
                    kind: FileOpKind::Trash,
                    targets,
                    dest: None,
                    root: PathBuf::new(), // start_file_op が dispatch 時の tab.root で埋める
                    err_self_paste: String::new(),
                    err_failed: crate::i18n::tr(self.lang, crate::i18n::Msg::OperationFailed)
                        .to_string(),
                };
                // 選択解除は**成功して初めて**(`apply_file_op`)。途中で失敗した削除で選択ごと
                // 失うと選び直しからやり直しになる(旧同期版も Ok(()) の中でだけ解除していた)。
                self.start_file_op(job);
            }
            // Git ビューの破棄: git::discard → 一覧/ツリーの git status を取り直す。
            // GitDiff プレビューからの破棄(came_from_git_view)なら Git ビューを開き直して戻す。
            PendingOp::GitDiscard { path } => match crate::git::discard(&self.tab.root, &path) {
                Ok(()) => {
                    let from_diff = self.is_git_diff_preview();
                    if from_diff {
                        // プレビューを畳んで Git ビューへ復帰。
                        self.tab.came_from_git_view = false;
                        self.back_to_tree();
                        self.open_git_view();
                    }
                    self.git_view_reload();
                    // 破棄自体は成功。ツリー再構築が失敗した時はその旨を通知し、
                    // 成功 flash で上書きしない(誤って「成功」と見せない)。
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
                        "{}: {e}",
                        crate::i18n::tr(self.lang, crate::i18n::Msg::Failed)
                    ))
                }
            },
            // ブランチ削除(安全): git branch -d。失敗(未マージ等)は git の stderr を flash。
            PendingOp::GitDeleteBranch { name } => self.git_delete_branch(&name, false),
            // ブックマーク上書きの確定: 実際に登録する(mark_input と同じ経路)。
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
        // ブランチの強制削除 (`-D`)。
        if let PendingOp::GitDeleteBranch { name } = &dialog.op {
            self.git_delete_branch(&name.clone(), true);
            return Ok(());
        }
        if let PendingOp::Delete { targets } = dialog.op {
            // 完全削除も他の一括ファイル操作と同じくバックグラウンドで実行する(原則#4)。
            let job = FileOpJob {
                kind: FileOpKind::DeletePermanent,
                targets,
                dest: None,
                root: PathBuf::new(), // start_file_op が dispatch 時の tab.root で埋める
                err_self_paste: String::new(),
                err_failed: crate::i18n::tr(self.lang, crate::i18n::Msg::OperationFailed)
                    .to_string(),
            };
            // 選択解除は**成功して初めて**(`apply_file_op`)。途中失敗で選択を失わせない。
            self.start_file_op(job);
        }
        Ok(())
    }
}
