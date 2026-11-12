//! The status-bar chips of a version-control project canvas, derived once from the canvas's
//! backend and screen. The host's status bar and the standalone program's chrome both draw them,
//! so the words, colours and screen mapping live only here.

use ratatui::style::Color;

use super::state::ProjectScreenState;

/// One chip: its text, background and whether the background is dark (white text).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VcsChip {
    pub text: String,
    pub bg: Color,
    pub dark: bool,
}

/// The outer (backend) chip and the inner (screen) chip of a version-control canvas.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VcsChips {
    pub backend: VcsChip,
    /// `None` for a screen that has no name of its own.
    pub screen: Option<VcsChip>,
}

impl<I> ProjectScreenState<I> {
    /// The chips of this canvas; `None` when the project is not a git or jj project.
    pub fn vcs_chips(&self, ja: bool) -> Option<VcsChips> {
        let jj = self.vcs_jj()?;
        let t = |en: &str, ja_text: &str| (if ja { ja_text } else { en }).to_string();
        let backend = if jj {
            VcsChip {
                text: t("JJ", "jj"),
                bg: Color::Magenta,
                dark: false,
            }
        } else {
            VcsChip {
                text: t("GIT", "Git"),
                bg: Color::Yellow,
                dark: false,
            }
        };
        let yellow = |text: String| VcsChip {
            text,
            bg: Color::Yellow,
            dark: false,
        };
        let screen = match self.frame()?.key.screen.as_str() {
            "changes" => Some(yellow(t("CHANGES", "変更"))),
            "log" => Some(yellow(t("LOG", "ログ"))),
            "graph" => Some(yellow(t("GRAPH", "グラフ"))),
            "graph_picker" => Some(yellow(format!(
                "{} - {}",
                t("GRAPH", "グラフ"),
                t("BRANCH", "ブランチ")
            ))),
            // jj's list holds bookmarks; the word is not a synonym for branch there.
            "branches" if jj => Some(yellow(t("BOOKMARK", "ブックマーク"))),
            "branches" => Some(yellow(t("BRANCH", "ブランチ"))),
            "worktrees" => Some(yellow(t("WORKTREES", "ワークツリー"))),
            // The released hub showed a worktree's changes in the commit-detail view.
            "detail" | "worktree_diff" => Some(VcsChip {
                text: t("COMMIT DIFF", "コミット差分"),
                bg: Color::Blue,
                dark: true,
            }),
            _ => None,
        };
        Some(VcsChips { backend, screen })
    }
}
