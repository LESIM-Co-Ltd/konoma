//! Navigation returned by a plugin uses the same paths as links, worktrees and Ctrl-t.

use super::*;
use crate::i18n::Msg;
use konoma_plugin::Outcome;

impl App {
    pub(super) fn apply_plugin_navigation(&mut self, outcome: Outcome) -> Result<()> {
        match outcome {
            Outcome::OpenFile { path, line } => {
                if path.is_file() {
                    self.open_file_at(&path, line);
                } else {
                    self.flash = Some(format!(
                        "{}{}",
                        tr(self.lang, Msg::NotFound),
                        path.display()
                    ));
                }
            }
            Outcome::OpenDiff { path } => self.open_git_diff(&path),
            Outcome::SwitchRoot { root, message } => {
                if root.is_dir() {
                    self.switch_worktree_root(root);
                    if let Some(message) = message {
                        self.flash = Some(message.get(self.plugin_lang()).into());
                    }
                } else {
                    self.flash = Some(format!(
                        "{}{}",
                        tr(self.lang, Msg::NotFound),
                        root.display()
                    ));
                }
            }
            Outcome::OpenTab { path } => {
                if path.exists() {
                    let is_dir = path.is_dir();
                    self.tab_new_from_path(path, is_dir)?;
                } else {
                    self.flash = Some(format!(
                        "{}{}",
                        tr(self.lang, Msg::NotFound),
                        path.display()
                    ));
                }
            }
            _ => {}
        }
        Ok(())
    }
}
