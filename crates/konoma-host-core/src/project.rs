//! What the project screens (git / jj: changes, log, graph, branches, worktrees) read from the
//! config, shared by konoma and the standalone `konoma-vcs` app: the diff layout and the
//! settings of `[git]`, `[jj]`, `[external]` and `[ui]` that those screens use. No renderer and
//! no plugin API.

use crate::shared::{SharedConfig, SharedTheme};

#[cfg(test)]
#[path = "project_tests.rs"]
mod decision_tests;

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

/// Which of the full-screen diff's three presentations is shown. The host keeps this per tab
/// so cycling a second tab's presentation does not change the first tab's choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffView {
    /// The classic unified/split source diff.
    Source,
    /// Changed Markdown blocks, decorated, or pictures paired side by side.
    Rendered,
    /// The file's ordinary preview, with a change gutter on the current content.
    Preview,
}

impl DiffView {
    /// `[ui] diff_view`, resolved permissively: an unrecognized value falls back to `Rendered`
    /// (the config default), so a typo cannot silently change behavior in a surprising direction.
    pub fn parse(s: &str) -> Self {
        match s.trim() {
            "source" => Self::Source,
            "preview" => Self::Preview,
            _ => Self::Rendered,
        }
    }
}

/// The three picture-capable kinds a media diff can pair up side by side. Everything else
/// (video/archive/table/unsupported) degrades to a summary instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaDiffKind {
    Image,
    Svg,
    Pdf,
}

/// The host's resolved preview kind, reduced to what its full-screen diff can offer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffTarget {
    Markdown,
    Text,
    Picture,
    Svg,
    Other,
}

/// The media worker's classification of a deleted file, once its result has landed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LandedMedia {
    /// A decoded raster image or PDF.
    Picture,
    Svg,
    /// A summary or an unavailable result.
    NotPicture,
}

/// The ordered list of presentations a diff can actually show: `[Source, Rendered, Preview]`
/// for Markdown, `[Source, Preview]` for a windowed-capable text kind (code, plain text, `.mmd`,
/// a text-mode delegated command), `[Rendered, Preview]` for Image/GIF/PDF (no text `Source` to
/// speak of: `Rendered` is the side-by-side view), `[Source, Rendered, Preview]` for SVG (both a
/// picture and real text), and `[Source]` for anything else (video/archive/table/unsupported/an
/// image-mode delegated command: the binary-summary-line-only case).
///
/// A deleted file never has `Preview`, pruned unconditionally at the end. Its kind is still
/// judged by the host's preview resolver first (a glob rule matches by filename alone, so
/// Markdown/Code/Mermaid/SVG/PDF classify correctly even when deleted). Only an `Other` target
/// defers to the media worker's byte-sniffed classification: a landed picture names a real kind,
/// a summary/unavailable result means "not a picture" (`[Source]`), and `None` (not landed yet)
/// treats a missing path as `[Rendered]` with the computing body in the meantime. A landed
/// result is ignored for an existing file and for any target other than `Other`.
pub fn diff_representations(
    target: DiffTarget,
    exists: bool,
    landed: Option<LandedMedia>,
) -> Vec<DiffView> {
    use DiffView::{Preview, Rendered, Source};
    let mut reps = match target {
        DiffTarget::Markdown => vec![Source, Rendered, Preview],
        DiffTarget::Text => vec![Source, Preview],
        DiffTarget::Picture => vec![Rendered, Preview],
        DiffTarget::Svg => vec![Source, Rendered, Preview],
        DiffTarget::Other if !exists => match landed {
            Some(LandedMedia::Picture) => vec![Rendered],
            Some(LandedMedia::Svg) => vec![Source, Rendered],
            Some(LandedMedia::NotPicture) => vec![Source],
            None => vec![Rendered],
        },
        DiffTarget::Other => vec![Source],
    };
    if !exists {
        reps.retain(|v| *v != Preview);
    }
    reps
}

/// `view` rounded to what the diff can actually show (`diff_representations`): the requested
/// presentation is used as-is if it is in the list; otherwise `Rendered`/`Source` substitute for
/// each other (whichever of the pair is in the list). Failing that (a `Preview` request with no
/// substitute, or a substitute that also is not in the list), the list's own first entry is used.
/// An empty list falls back to `Source`.
pub fn round_diff_view(view: DiffView, reps: &[DiffView]) -> DiffView {
    if reps.contains(&view) {
        return view;
    }
    let substitute = match view {
        DiffView::Rendered => Some(DiffView::Source),
        DiffView::Source => Some(DiffView::Rendered),
        DiffView::Preview => None,
    };
    if let Some(sub) = substitute {
        if reps.contains(&sub) {
            return sub;
        }
    }
    reps.first().copied().unwrap_or(DiffView::Source)
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
