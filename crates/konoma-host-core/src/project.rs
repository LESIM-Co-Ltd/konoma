//! What the project screens (git / jj: changes, log, graph, branches, worktrees) read from the
//! config, shared by konoma and the standalone `konoma-vcs` app: the diff layout and the
//! settings of `[git]`, `[jj]`, `[external]` and `[ui]` that those screens use. No renderer and
//! no plugin API.

use crate::shared::{SharedConfig, SharedTheme};

/// Minimum inner width for side-by-side under Auto (below this, vertical). A guideline so
/// two-column code is not cramped.
pub const DIFF_AUTO_MIN_WIDTH: u16 = 90;

/// diff layout (config `git.diff`; cycle at runtime with `s`). Auto picks vertical/horizontal
/// automatically from the terminal width.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffLayout {
    /// Vertical (unified).
    Unified,
    /// Side-by-side.
    Split,
    /// Horizontal if wide, vertical if narrow.
    Auto,
}

impl DiffLayout {
    /// Parse a config value. "split"/"horizontal"/"side-by-side"=horizontal / "auto"=auto /
    /// otherwise=vertical.
    pub fn parse(s: &str) -> Self {
        let n: String = s
            .chars()
            .filter(|c| !matches!(c, ' ' | '-' | '_'))
            .flat_map(|c| c.to_lowercase())
            .collect();
        match n.as_str() {
            "split" | "horizontal" | "sidebyside" | "side" => Self::Split,
            "auto" => Self::Auto,
            _ => Self::Unified, // "unified" / "vertical" / unknown
        }
    }

    /// Resolve whether to lay out side-by-side at the actual display width `width`. Auto decides
    /// by a threshold.
    pub fn is_split(self, width: u16) -> bool {
        match self {
            DiffLayout::Unified => false,
            DiffLayout::Split => true,
            DiffLayout::Auto => width >= DIFF_AUTO_MIN_WIDTH,
        }
    }

    /// Cycle to the next layout (vertical, horizontal, Auto, vertical). For `s`.
    pub fn next(self) -> Self {
        match self {
            DiffLayout::Unified => DiffLayout::Split,
            DiffLayout::Split => DiffLayout::Auto,
            DiffLayout::Auto => DiffLayout::Unified,
        }
    }
}

/// The default external git tool (`[git] tool`).
pub const DEFAULT_GIT_TOOL: &str = "lazygit";
/// The default external jj tool (`[jj] tool`).
pub const DEFAULT_JJ_TOOL: &str = "lazyjj";

/// The effective settings the project screens use, read from a [`SharedConfig`]. The one place
/// their precedence is decided; konoma's own `Config` holds the same values (pinned by a parity
/// test in konoma).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectSettings {
    /// `[git] diff`, parsed.
    pub diff_layout: DiffLayout,
    /// `[git] worktree_dir`, as written (default `"../"`).
    pub worktree_dir: String,
    /// `[ui] path_style` as written: "relative" | "home" | "full".
    pub path_style: String,
    /// `[ui] icons`.
    pub icons: bool,
    /// `[ui.theme]`.
    pub theme: SharedTheme,
    /// `[external] git`: the legacy master switch of the git integration (default on).
    pub git_enabled: bool,
    /// `[external] git_tool`: whether `!` may launch the external tool (default on).
    pub tool_enabled: bool,
    git_tool: String,
    jj_tool: String,
}

impl ProjectSettings {
    /// The settings `cfg` gives.
    pub fn from_config(cfg: &SharedConfig) -> Self {
        Self {
            diff_layout: DiffLayout::parse(&cfg.git.diff),
            worktree_dir: cfg.git.worktree_dir.clone(),
            path_style: cfg.ui.path_style.clone(),
            icons: cfg.ui.icons,
            theme: cfg.ui.theme.clone(),
            git_enabled: cfg.external.git,
            tool_enabled: cfg.external.git_tool,
            git_tool: cfg.git.tool.clone(),
            jj_tool: cfg.jj.tool.clone(),
        }
    }

    /// The command line `!` launches for git (`jj == false`) or jj: `[git] tool` / `[jj] tool`
    /// trimmed, or the well-known default when it is empty or only whitespace. Returns
    /// `(command line, default tool name)`.
    pub fn tool_command(&self, jj: bool) -> (String, &'static str) {
        let (tool, fallback) = if jj {
            (self.jj_tool.trim(), DEFAULT_JJ_TOOL)
        } else {
            (self.git_tool.trim(), DEFAULT_GIT_TOOL)
        };
        let resolved = if tool.is_empty() { fallback } else { tool };
        (resolved.to_string(), fallback)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_layout_parses_every_spelling_the_host_accepts() {
        for s in [
            "split",
            "horizontal",
            "side-by-side",
            "side_by_side",
            "side by side",
            "SIDE",
            "Split",
            "Side-By-Side",
        ] {
            assert_eq!(DiffLayout::parse(s), DiffLayout::Split, "{s:?}");
        }
        for s in ["auto", "AUTO", "Auto"] {
            assert_eq!(DiffLayout::parse(s), DiffLayout::Auto, "{s:?}");
        }
        for s in ["unified", "vertical", "", "bogus", "autos", "splitt"] {
            assert_eq!(DiffLayout::parse(s), DiffLayout::Unified, "{s:?}");
        }
    }

    #[test]
    fn diff_layout_next_cycles_unified_split_auto() {
        assert_eq!(DiffLayout::Unified.next(), DiffLayout::Split);
        assert_eq!(DiffLayout::Split.next(), DiffLayout::Auto);
        assert_eq!(DiffLayout::Auto.next(), DiffLayout::Unified);
    }

    #[test]
    fn diff_layout_is_split_decides_auto_at_the_threshold() {
        assert_eq!(DIFF_AUTO_MIN_WIDTH, 90);
        for w in [0, 1, DIFF_AUTO_MIN_WIDTH - 1, DIFF_AUTO_MIN_WIDTH, u16::MAX] {
            assert!(!DiffLayout::Unified.is_split(w));
            assert!(DiffLayout::Split.is_split(w));
        }
        assert!(!DiffLayout::Auto.is_split(DIFF_AUTO_MIN_WIDTH - 1));
        assert!(DiffLayout::Auto.is_split(DIFF_AUTO_MIN_WIDTH));
        assert!(DiffLayout::Auto.is_split(DIFF_AUTO_MIN_WIDTH + 1));
    }

    fn settings(text: &str) -> ProjectSettings {
        ProjectSettings::from_config(&SharedConfig::parse(text).unwrap())
    }

    #[test]
    fn project_settings_default_when_nothing_is_set() {
        let s = settings("");
        assert_eq!(s.diff_layout, DiffLayout::Unified);
        assert_eq!(s.worktree_dir, "../");
        assert_eq!(s.path_style, "relative");
        assert!(s.icons && s.git_enabled && s.tool_enabled);
        assert_eq!(s.theme, SharedTheme::default());
        assert_eq!(s.tool_command(false), ("lazygit".to_string(), "lazygit"));
        assert_eq!(s.tool_command(true), ("lazyjj".to_string(), "lazyjj"));
    }

    #[test]
    fn project_settings_read_every_section() {
        let s = settings(
            "[git]\ndiff = \"auto\"\ntool = \" gitui -x \"\nworktree_dir = \"/w\"\n[jj]\ntool = \"jjui\"\n[external]\ngit = false\ngit_tool = false\n[ui]\npath_style = \"home\"\nicons = false\n[ui.theme]\nbg = \"#102030\"\n",
        );
        assert_eq!(s.diff_layout, DiffLayout::Auto);
        assert_eq!(s.worktree_dir, "/w");
        assert_eq!(s.path_style, "home");
        assert!(!s.icons && !s.git_enabled && !s.tool_enabled);
        assert_eq!(s.theme.bg, "#102030");
        assert_eq!(s.tool_command(false).0, "gitui -x");
        assert_eq!(s.tool_command(true).0, "jjui");
    }

    #[test]
    fn an_empty_or_blank_tool_falls_back_to_the_default_for_its_own_backend() {
        let s = settings("[git]\ntool = \"   \\t \"\n[jj]\ntool = \"\"\n");
        assert_eq!(s.tool_command(false), ("lazygit".to_string(), "lazygit"));
        assert_eq!(s.tool_command(true), ("lazyjj".to_string(), "lazyjj"));
        // One backend's tool never answers for the other.
        let s = settings("[git]\ntool = \"gitui\"\n");
        assert_eq!(s.tool_command(true).0, "lazyjj");
        let s = settings("[jj]\ntool = \"jjui\"\n");
        assert_eq!(s.tool_command(false).0, "lazygit");
    }

    #[test]
    fn a_wrong_type_in_a_project_setting_is_an_error_as_in_konoma() {
        for bad in [
            "[git]\ndiff = 1\n",
            "[git]\ntool = true\n",
            "[git]\nworktree_dir = 3\n",
            "[jj]\ntool = 3\n",
            "[external]\ngit = \"no\"\n",
            "[external]\ngit_tool = 1\n",
        ] {
            assert!(SharedConfig::parse(bad).is_err(), "{bad:?}");
        }
    }
}
