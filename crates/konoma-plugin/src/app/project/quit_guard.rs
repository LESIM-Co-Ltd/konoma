//! Quitting while a project act that declares `writes` runs: the standalone asks first, as the
//! host does. Quitting would orphan the write (e.g. a `git` process holding `.git/index.lock`).
//! A running read never asks, and an idle quit stays immediate (the standalone has no
//! `confirm_quit` setting).
//!
//! Every way out of the program goes through [`ProjectLoopState::request_quit`], so one rule
//! covers `Q`, Ctrl-c, the help's quit key, the diff screen's quit key, `q` on a root screen and a
//! quit asked from a file preview.

use super::*;
use crate::host::project::draw::{
    present_confirmation, ConfirmationChoiceWords, ProjectDialogPresentation, ProjectDialogWords,
};

/// The line the host shows for a running git write (`Msg::QuitWhileGitOp`); a parity test in the
/// host crate keeps the two texts equal, because the SDK must not depend on the host's i18n.
pub fn quit_while_write_text(lang: &str) -> &'static str {
    if lang == "ja" {
        "git 操作が実行中です — 終了すると中断されます"
    } else {
        "a git operation is still running — quitting will interrupt it"
    }
}

impl ProjectLoopState {
    /// Whether the program may quit now. When an act that writes is running, this opens the
    /// confirmation instead (closing the help and menu so nothing covers it) and answers `false`.
    pub(super) fn request_quit(&mut self) -> bool {
        if !self.runtimes.values().any(|r| r.write_act_running()) {
            return true;
        }
        self.quit_confirm = true;
        self.help = None;
        self.menu = None;
        false
    }

    /// The key a quit confirmation takes: `y` quits; `n`, `N` and `Esc` cancel (the keys of
    /// every project confirmation); any other key is ignored while the question is open.
    pub(super) fn quit_confirm_key(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Char('y') if key.modifiers == KeyModifiers::NONE => return true,
            KeyCode::Char('n' | 'N') | KeyCode::Esc => self.quit_confirm = false,
            _ => {}
        }
        false
    }

    pub(super) fn quit_presentation(
        &self,
        prepared: &PreparedProjects,
    ) -> Option<ProjectDialogPresentation> {
        if !self.quit_confirm {
            return None;
        }
        let ja = prepared.lang == "ja";
        let text = |en, ja_text| if ja { ja_text } else { en };
        let message = format!(
            "{}\n{}",
            text("Quit?", "終了しますか？"),
            quit_while_write_text(prepared.lang)
        );
        Some(present_confirmation(
            &dialog_words(prepared.lang),
            &message,
            &[ConfirmationChoiceWords {
                key: 'y',
                label: text("quit", "終了"),
                status: text("quit", "終了"),
                destructive: true,
            }],
            true,
        ))
    }
}

/// The words every project dialog of the standalone is drawn with.
pub(super) fn dialog_words(lang: &str) -> ProjectDialogWords<'_> {
    let text = |en, ja| if lang == "ja" { ja } else { en };
    ProjectDialogWords {
        lang,
        choose: text("Choose a project", "プロジェクトを選択"),
        confirm_title: text(" Confirm ", " 確認 "),
        input_title: text(" Input ", " 入力 "),
        keys: text("Enter:choose  Esc:cancel", "Enter:選択  Esc:キャンセル"),
        confirm_cancel: text("n / Esc = cancel", "n / Esc = 取消"),
        confirm_cancel_status: text("n/Esc:cancel", "n/Esc:取消"),
    }
}
