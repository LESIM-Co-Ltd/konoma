// Config loading, and resolving format → preview method.
// Works with defaults even when the config is absent/broken (robustness first).

use std::path::Path;
use std::str::FromStr;

use ratatui::style::Color;
use serde::Deserialize;

use crate::preview::PreviewKind;

// The default code background and the badge-brightening helper live in `konoma-widgets` (the
// document renderer's tests use them); this path stays for the host.
pub use konoma_widgets::colors::{lighten, DEFAULT_CODE_BG};

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Config {
    pub ui: UiConfig,
    pub preview: PreviewConfig,
    pub keys: KeysConfig,
    pub editor: EditorConfig,
    pub git: GitConfig,
    pub jj: JjConfig,
    pub external: ExternalConfig,
    /// `[plugins.<id>]`: `enabled = false` switches a plugin off; the other keys are the plugin's
    /// own settings.
    pub plugins: crate::plugins::PluginTables,
    /// The old `[ui]` names the loaded file still uses, with what to write instead (not part of
    /// the file: filled in by `Config::load_reporting`, named once in the startup message).
    #[serde(skip)]
    pub renamed_settings: Vec<(String, String)>,
    /// The external plugins (programs of their own) this run loaded, as the token
    /// `plugins::external::install` returned. Not part of the file. It is part of the registry's key
    /// (`plugin_disabled`), so a configuration with other external plugins never shares a registry.
    #[serde(skip)]
    pub external_plugins: Option<String>,
}

/// Single on/off switch per external process konoma can launch (`[external]`). Everything defaults to
/// `true`, so an absent section changes nothing. Like `[[preview.rules]]` (user rules are tried before the standard
/// plugins' assignments, see `PreviewConfig::default`), these flags are independent toggles layered
/// on top of whatever preview rules are in effect: they gate the specific mechanism, not the rule
/// table. `[ui] lang` already has its own explicit/auto switch for OS-language lookup
/// (`Lang::resolve`), so there is deliberately no separate flag for it here.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ExternalConfig {
    /// git integration (status colors, gutter, the Git views, stage/commit/checkout — `src/git.rs`,
    /// via the `git` CLI and the embedded git2/libgit2). `false` behaves like the crate built with
    /// `--no-default-features` (no `git` feature): every read returns empty/None and every write
    /// returns an error, exactly as when the feature is compiled out, so nothing else needs to change.
    pub git: bool,
    /// The external git tool launched with `!` in the changes hub (`[git] tool`, default lazygit).
    pub git_tool: bool,
    /// Which version-control system answers for a directory: `"auto"` (default), `"git"` or `"jj"`.
    ///
    /// `auto` keeps git wherever git can answer, so a repository that already worked shows exactly
    /// what it showed before; jj answers only where there is no git repository to ask (one created
    /// with `jj git init --no-colocate`, or a `jj workspace`). git answers anyway when the `jj`
    /// binary is missing. Set it to `"jj"` to pin a colocated repository, where both systems can
    /// answer, to the one you actually work in.
    ///
    /// Parsed by [`crate::vcs::Preference`], which is where the precedence is decided; anything
    /// unrecognised means `auto`.
    pub vcs: String,
    /// The external PDF fallback tools — macOS's bundled `qlmanage`/`sips`, used only when the
    /// primary renderer (`hayro`, pure Rust — always active regardless of this flag) fails to render
    /// page 1 of a given PDF (encrypted/corrupt/unsupported). `false` never launches those tools;
    /// PDF preview itself keeps working via `hayro` (see `preview::pdf`). Off macOS this flag is
    /// effectively a no-op: there is no external chain there at all.
    pub pdf: bool,
    /// Video thumbnail extraction (ffmpegthumbnailer/ffmpeg).
    pub video: bool,
    /// Fetching `http(s)://` images referenced from Markdown (`curl`) — the only outbound network call konoma makes.
    pub remote_images: bool,
    /// Opening URLs/files with the OS handler (`open` on macOS, `xdg-open` elsewhere) — Markdown links, `P`, etc.
    pub open_links: bool,
    /// `e` on an Office document (Word/Excel/PowerPoint/ODF) opens it in a GUI app — Microsoft Office,
    /// then LibreOffice, then the OS default — instead of handing the zip to `$EDITOR`. `false` opens
    /// nothing and says so. An explicit `[editor.ext]` entry for the extension always wins.
    pub office_apps: bool,
    /// Running a `[[preview.rules]] command = "..."` delegation. `false` makes that rule shape behave
    /// like no rule matched (falls through to `[can not preview]`).
    pub preview_commands: bool,
}

impl Default for ExternalConfig {
    fn default() -> Self {
        Self {
            git: true,
            git_tool: true,
            vcs: "auto".to_string(),
            pdf: true,
            video: true,
            remote_images: true,
            open_links: true,
            office_apps: true,
            preview_commands: true,
        }
    }
}

/// Git integration settings (`[git]`). How external git tools are launched and how diffs are shown.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct GitConfig {
    /// External git tool launched with `!` inside the changes hub (command + args, whitespace-separated). Default "lazygit".
    pub tool: String,
    /// Initial diff layout. "unified" (vertical, default) | "split" (side by side) | "auto" (by width).
    /// (Aliases: vertical = unified / horizontal, side-by-side = split.) At runtime, `s` while viewing a diff
    /// cycles vertical -> horizontal -> Auto. Applies to both the GitDiff preview and commit/working-tree details.
    pub diff: String,
    /// Initial layout for the image/PDF/SVG side-by-side "diff" (`docs/FEATURE-MEDIA-DIFF.md`).
    /// "auto" (default) = whichever of side-by-side/stacked lets the picture render larger |
    /// "side" (aliases: split/horizontal/side-by-side) | "stack" (aliases: vertical/stacked/unified).
    /// A separate setting from `diff` (above) on purpose — sharing one would make "auto" mean two
    /// different defaults for two unrelated layouts. At runtime, `s` while viewing a media diff
    /// cycles auto → side → stack → auto.
    pub media_diff: String,
    /// [Unimplemented/reserved] For the base-branch-pinned graph (post-release; docs/GRAPH-BASE-SPEC.md).
    /// Currently referenced by nothing (parsed only). Wired up when implemented.
    pub main_branch: String,
    /// Where `n` (in the worktree list, `[keys.git_worktrees]`) places a newly created linked
    /// worktree — resolved against the **main** worktree's own path (not wherever the tab's root
    /// happens to be, so creating from inside another linked worktree doesn't change where new ones
    /// land), then joined with the branch name (its `/`, if any, replaced with `-`, since a slash
    /// would otherwise make `git worktree add` create a **nested** directory). Default `"../"` — a
    /// sibling of the main worktree, e.g. `~/work/proj` + branch `feat` → `~/work/feat`. An absolute
    /// path is used as-is. **Avoid a path inside the repository itself** (e.g. `".worktrees/"`):
    /// every linked worktree created there would show up as an untracked directory in the *main*
    /// worktree's own `git status` unless you add it to `.git/info/exclude` yourself — konoma never
    /// writes to that file for you.
    pub worktree_dir: String,
}

/// jj (Jujutsu) settings (`[jj]`). Only what has no git counterpart lives here — everything else
/// about a jj repository is decided by `[external] vcs` and by jj itself.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct JjConfig {
    /// External jj tool launched with `!` inside the hub (command + args, whitespace-separated).
    /// Default "lazyjj". Missing from the machine behaves exactly like a missing lazygit: a flash
    /// naming what could not be launched.
    pub tool: String,
}

impl Default for JjConfig {
    fn default() -> Self {
        Self {
            tool: "lazyjj".into(),
        }
    }
}

impl Default for GitConfig {
    fn default() -> Self {
        Self {
            tool: "lazygit".into(),
            diff: "unified".into(), // default is vertical. horizontal/Auto via config or runtime `s`.
            media_diff: "auto".into(),
            main_branch: "".into(),
            worktree_dir: "../".into(),
        }
    }
}

pub use konoma_host_core::editor::EditorConfig;
#[cfg(test)]
use konoma_host_core::editor::{build_argv, env_nonempty};
pub use konoma_host_core::keys::KeysConfig;

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct UiConfig {
    pub show_hidden: bool,
    /// How the tree filter (`/`) matches typed characters against file/directory names.
    /// `"fuzzy"` (default) does fzf-style fuzzy subsequence matching via `nucleo-matcher`, ranked
    /// best-match-first — non-contiguous matches work (e.g. `aprs` → `app_resolver.rs`), and
    /// space-separated words in the query are AND-ed (e.g. `app rs`). Matching is always
    /// case-insensitive (matching the legacy behavior below), so typing in any case still finds
    /// everything. `"substring"` restores the legacy behavior: a plain, case-insensitive substring
    /// match, kept in the tree's original order (no ranking, no fuzzy, no AND). Any other value
    /// falls back to `"fuzzy"` (see `UiConfig::filter_mode`).
    pub filter_mode: String,
    pub tabbar: String,     // "always" | "auto" | "hidden"
    pub icons: bool, // Nerd Font icon at the start of each tree row (needs Nerd Font; false for plain symbols if unavailable)
    pub wrap: bool, // wrapping in text previews. true = wrap and show the whole line / false = no wrap + horizontal scroll
    pub line_numbers: bool, // whether to show a line-number gutter in code/text previews (default false)
    /// Show an editor-style git change gutter (Zed-like: green added / blue modified / red deleted
    /// markers in the left margin) on code/text previews of files with working-tree changes. Default true.
    pub git_gutter: bool,
    /// Tab stop width (default 4). The number of columns to which a tab is expanded in code/text previews as
    /// "a visible marker (→) plus the spaces up to the next tab stop." Because terminals do not column-align tabs, this
    /// aligns indentation and makes tabs recognizable. 0 disables expansion (raw tabs kept).
    pub tab_width: usize,
    /// Whether to syntax-highlight code (default true). false = plain-text display, fastest, syntect not used.
    pub syntax_highlight: bool,
    /// How to present the wait when opening a heavy code preview (the first time for a cold language).
    /// "indicator" (default) = show a loading display in the center of the screen, then the content / "progressive" = show plain text immediately
    /// and swap in highlighting as soon as it is ready (no freeze). Warmed-up languages display instantly, so neither applies.
    pub preview_loading: String,
    pub path_style: String, // default path display in the title. "relative" | "home" | "full" (cycled with the p key)
    pub keys: String,       // preview paging key style. "vim" | "less" (default vim)
    pub lang: String, // display language. "auto" (default: follows the OS language) | "en" | "jp". Applied to help/hints/messages etc.
    /// Layout of the status chrome. "split" (default) = context (mode/path/zoom) on top, key hints at the bottom /
    /// "bottom" = everything on one bottom line / "top" = everything on one top line.
    pub statusbar: String,
    pub theme: ThemeConfig, // colors (currently only the code background color)
    /// Image display size scale (0.1 to 1.0). Shrinks the center-filled rectangle to this factor for display.
    /// Smaller values reduce the pixels transferred to kitty (= draw/zoom wait) but also make the display smaller.
    /// The font is shrunk at actual size, so placement (centered) and aspect are always correct.
    /// Default 1.0 = full display (largest, but transfer takes time for large images / when zoomed in).
    pub image_render_scale: f64,
    /// Maximum side (px) for SVG rasterization. Since vectors have unlimited resolution, they are drawn scaled up to this px
    /// (so small SVGs are crisp too). Larger is sharper but heavier to draw/transfer. Default 800.
    /// **SVG only**: raster images/GIFs are unaffected by this setting (their transfer size is `image_render_scale`).
    pub svg_max_px: u32,
    /// Default tree sort order (at startup). Changeable at runtime via the `s` menu.
    pub sort: SortConfig,
    /// Detailed list view: the columns of metadata laid out to the right of each row. Empty (default) = none (plain display).
    /// Available columns: `"size"` (size) / `"modified"` (modified time) / `"perm"` (permissions rwx) /
    /// `"type"` (file/dir/link) / `"items"` (directory item count; calls `read_dir` on each dir row).
    /// Laid out to the right in order. Example: `details = ["type", "size", "modified"]`.
    /// (symlink target / absolute path are variable-length and unsuited to columns, so they are exclusive to the `i` popup.)
    pub details: Vec<String>,
    /// The **cap on local branches drawn simultaneously** in the commit graph (`o`->`g`) (default 12).
    /// Since many branches overflow lanes/colors/legend, at startup it auto-selects up to the cap by HEAD + base + most-recent-commit order.
    /// Show/hide can be toggled freely via the `b` selection panel in the graph (HEAD is always shown). 0 = unlimited.
    pub graph_max_branches: usize,
    /// The commit graph's **base/priority branches** (ordered, default `[]`). Example `["main", "develop"]`.
    /// (1) Branches listed here are shown preferentially (placed at the front of the cap), (2) the first one that exists, from the left,
    /// becomes the **base** (pinned to lane0 as a straight left line), and (3) the order is also reflected in the legend/panel display order.
    /// In the graph's `b` panel, `J`/`K` reorder for the current session only (not written back to config).
    pub graph_base_branches: Vec<String>,
    /// In the commit rows of log (`o`->`l`) / graph (`o`->`g`), how the right-side metadata (author, date, and short in the graph) is aligned.
    /// `"right"` (default) = aligned to the panel's right edge as a **column** (dates/authors line up neatly vertically, so age is readable at a glance;
    /// SourceTree / VS Code Git Graph style. When narrow, the subject side is truncated, and when even narrower the metadata is dropped).
    /// `"inline"` = placed right after the subject (the legacy display; metadata sits immediately next to the subject).
    pub commit_meta_align: String,
    /// Ask for confirmation before quitting the whole app (`q` at the top level / `Q` from anywhere). Default true.
    /// When true, `q`/`Q` open a yes/no dialog (`q`/`y`/Enter = quit, `n`/Esc = cancel). false = quit immediately.
    pub confirm_quit: bool,
    /// Ask for confirmation before **overwriting an existing bookmark**. Default true. When true, setting
    /// a bookmark (`m` + a letter) whose letter already points to a **different** path opens a yes/no
    /// dialog (`y`/Enter = overwrite, `n`/Esc = cancel). false = overwrite silently. Setting a fresh
    /// letter, or re-setting the same path, never asks.
    pub confirm_bookmark_overwrite: bool,
    /// Ask before letting jj take a snapshot (`R` in the hub). konoma never writes to a jj
    /// repository on its own, so this is the one place it can, and it says so before doing it.
    pub confirm_jj_sync: bool,
    /// What follow mode (`F`) opens when it jumps to a changed file. `"diff"` (default) = the full-screen
    /// git diff of that file (hunk-level before/after — the way hunk/livediff/diffpane present agent
    /// edits; untracked files show as an all-added diff); `"file"` = the normal content preview scrolled
    /// to the first changed hunk. Files with no diff (unchanged / outside a repo) and media
    /// (image/SVG/video/PDF) always fall back to the content preview.
    pub follow_view: String,
    /// Which of the full-screen diff's three presentations (`docs/FEATURE-MD-RENDERED-DIFF.md`)
    /// it opens showing: `"rendered"` (default) draws a Markdown file's diff as decorated blocks —
    /// removed content in red/dim, added in green, changed in amber — so an AI edit's *deletions*
    /// stay visible instead of only its final result; `"source"` is the classic unified/split diff;
    /// `"preview"` is the file's ordinary preview with a change gutter. `R` cycles between whichever
    /// of the three apply to the file being diffed (a non-Markdown text file only ever has
    /// `"source"`/`"preview"` — `"rendered"` rounds down to `"source"` for it; a non-text file has
    /// none of the three and never cycles at all). An unrecognized value falls back to `"rendered"`.
    pub diff_view: String,
    /// Color each column of a CSV/TSV table preview with a rotating "rainbow" palette (default true), the way
    /// Rainbow CSV / csvlens do, so columns are easy to tell apart. false = monochrome (still aligned, still navigable).
    pub csv_rainbow: bool,
    /// Task-list checkbox states cycled by Space on a focused checkbox in a Markdown preview, in cycle order
    /// (default `[" ", "x"]` = the standard GFM unchecked/checked pair). Each entry must be exactly one
    /// character; e.g. `[" ", "/", "x"]` adds an Obsidian-style "in progress" state (shown as `[/]`).
    /// A state not in the list normalizes to the first entry on toggle. Invalid config falls back to the default.
    pub md_task_states: Vec<String>,
    /// Auto-link bare URLs and emails in Markdown previews (GFM autolink), the way GitHub does: a plain
    /// `https://…`, `www.…`, or `foo@bar.com` becomes a focusable link (Tab to it, Enter opens it). Default
    /// true. Links inside code spans / code fences are never auto-linked. false keeps them as plain text.
    pub md_autolink: bool,
    /// Render GitHub-style alerts (`> [!NOTE]` / `[!TIP]` / `[!IMPORTANT]` / `[!WARNING]` / `[!CAUTION]`, and
    /// their common aliases) as colored callout boxes with an icon and label, instead of a plain blockquote.
    /// Default true. false renders them as ordinary blockquotes (the `[!TYPE]` marker stays literal).
    pub md_alerts: bool,
    /// Convert `:shortcode:` emoji (e.g. `:rocket:` → 🚀) to real Unicode emoji in Markdown previews, the way
    /// GitHub does. Default true. Shortcodes without a Unicode equivalent (GitHub-custom like `:shipit:`) are
    /// left as text. false leaves every `:shortcode:` untouched (useful if emoji width upsets column alignment).
    pub md_emoji: bool,
    /// Recognize leading YAML front matter (`---` … `---` at the very start) and render it as a compact
    /// dim metadata block instead of a thematic break + raw YAML text. Default true. false renders the
    /// leading `---` and the YAML as ordinary Markdown.
    pub md_frontmatter: bool,
    /// Render GFM footnotes: `text[^1]` references become superscript numbers and the `[^1]: …`
    /// definitions are collected into a numbered footnotes section at the end. Default true. false
    /// leaves `[^1]` and its definition as literal text.
    pub md_footnotes: bool,
    /// Render common inline HTML that GitHub shows but konoma's own renderer strips by default: `<del>`/`<s>`/`<strike>`
    /// as strikethrough, `<kbd>` as an inline-code keycap, `<sup>`/`<sub>` as Unicode (when the text
    /// maps), and `<br>` as a hard line break. Default true. `<mark>`/`<ins>` have no faithful
    /// terminal form and keep only their text either way. false leaves all these tags to be stripped.
    pub md_inline_html: bool,
    /// How `<details>` blocks start out. `"auto"` (default) honors the `open` attribute like GitHub
    /// (`<details>` collapsed, `<details open>` expanded); `"open"` always starts expanded; `"closed"`
    /// always collapsed. Either way `Tab` focuses the `<summary>` and `Space`/`Enter` toggle it.
    pub md_details: String,
    /// Where a **table** sits inside a Markdown preview: `"left"` (default — the historical layout,
    /// the grid starts at the left edge), `"center"`, or `"right"`. Applies to the box as a whole,
    /// GFM pipe tables and HTML `<table>` alike (and inside a blockquote / alert / `<details>` body,
    /// within that body's own width). It does **not** touch alignment *within* a cell — a column's
    /// `:---:` and an HTML cell's `align=` keep deciding that, as they always did. A table at least
    /// as wide as the pane stays flush left whatever this says. An unrecognized value falls back to
    /// the default.
    pub md_table_align: String,
    /// Where a **block image** sits inside a Markdown preview: `"left"`, `"center"` (default — the
    /// historical layout), or `"right"`. Covers a standalone `![alt](url)`, a packed row of badge
    /// images, and a ```mermaid diagram (whose caption line and focus frame follow it). It does
    /// **not** cover an image inside a table cell (its cell's own alignment places that) nor
    /// display math, whose centering is a typesetting convention rather than an image-layout choice.
    /// An image at least as wide as the pane stays flush left whatever this says. An unrecognized
    /// value falls back to the default.
    pub md_image_align: String,
    /// How LaTeX math renders: `"image"` (default) rasterizes `$…$` / `$$…$$` in-process via RaTeX
    /// (pure Rust, KaTeX-quality) and shows each as an inline image — inline math is lifted onto its
    /// own line since a terminal cannot place an image mid-text. `"text"` leaves the raw LaTeX as
    /// plain text. Image mode degrades to the raw LaTeX automatically when the terminal has no image
    /// protocol or an expression fails to render (principle #3).
    pub math: String,
    /// Glyph color for image-mode math (default `"#d0d0d0"`, a light gray). RaTeX paints equations pure
    /// black, which is invisible on a dark terminal; konoma is dark-terminal-first, so equations are
    /// recolored to this over a transparent background (the terminal shows through, like mermaid). On a
    /// light terminal set a dark color (e.g. `"#202020"` or `"black"`). Accepts any color usvg parses
    /// (`#hex`, `rgb(…)`, or a CSS color name); an unrecognized or fully-transparent value falls back to
    /// the default, so a typo can't silently blank equations (see `UiConfig::math_color`).
    pub math_color: String,
    /// Show a small spinner + job label at the top-right while background work is in flight
    /// (git-ignored scan, media decode, highlight warm-up, inline image fetches). Default true.
    /// The indicator only animates while something is running — idle stays at zero redraws.
    pub busy_indicator: bool,
    /// How mermaid diagrams render: `"image"` (default) rasterizes them in-process (pure Rust,
    /// mermaid.js-quality) and shows them full-screen (standalone `.mmd`) or inline (```mermaid
    /// fences in Markdown) with zoom/pan; `"text"` keeps the legacy Unicode box-drawing rendering.
    /// Image mode degrades to text automatically when the terminal has no image protocol or a
    /// diagram fails to render (principle #3).
    pub mermaid: String,
    /// Color theme for image-mode mermaid diagrams: `"dark"` (default; matches dark terminals),
    /// `"light"`, `"classic"` (mermaid.js default), `"forest"`, `"neutral"`. The diagram background
    /// is always transparent so it blends with the terminal.
    pub mermaid_theme: String,
    /// Curve a flowchart's edges are drawn in — mermaid's `flowchart.curve`. All 13 of mermaid's
    /// own curve names are implemented (`Curve`'s own variant docs describe each); an
    /// unrecognised value falls back to `"basis"` rather than erroring, like every unrecognised
    /// value in this file. `"basis"` (default; mermaid's own default) is the spline every diagram
    /// drew before this setting existed. `"linear"` draws straight segments through every
    /// waypoint; `"step"` / `"stepBefore"` / `"stepAfter"` draw right-angle lines, differing in
    /// where the corner sits (midpoint / vertical-first / horizontal-first) — the shape mermaid's
    /// most-requested flowchart feature asks for (mermaid-js#2817, mermaid-js#2549). `"natural"` /
    /// `"cardinal"` / `"catmullRom"` are smooth splines that pass through every waypoint (unlike
    /// `"basis"`); `"monotoneX"` / `"monotoneY"` never overshoot a waypoint, along the named axis;
    /// `"bumpX"` / `"bumpY"` draw an S-curve offset on the named axis; `"rounded"` is straight
    /// segments with every corner cut to a small rounded curve — mermaid's own curve, not part of
    /// d3-shape. Three ways to set it, weakest first: this setting < the diagram's own
    /// `%%{init: {"flowchart": {"curve": "..."}}}%%` directive < a `linkStyle <n> interpolate
    /// <curve>` naming one specific edge — each, if present, overrides every weaker source
    /// (`render::spec_of`'s own doc has the exact priority chain). Only a flowchart's own edges
    /// read any of this — every other diagram kind (state, class, ER, sequence, C4, requirement,
    /// mindmap, git graph, architecture, block, and every chart) always draws `"basis"`.
    pub mermaid_curve: String,
    /// How a flowchart's edges are routed — the mode is named after Graphviz's `splines=ortho` /
    /// ELK's `edgeRouting: ORTHOGONAL`, but the value carries a `konoma-` prefix because it is
    /// konoma's own mode, not mermaid vocabulary the way `[ui] mermaid_curve`'s values are — the
    /// prefix is what keeps it from colliding if upstream mermaid ever gives the bare word
    /// `"orthogonal"` a different meaning of its own.
    /// `"splines"` (default) is every curve this crate has always drawn, `[ui] mermaid_curve`
    /// included, byte for byte. `"konoma-orthogonal"` is konoma's own right-angle wiring mode
    /// (`docs/FEATURE-MERMAID-RENDERER.md` §10): every edge is drawn as axis-parallel segments
    /// meeting each node's boundary perpendicularly, and a decision node (`A{text}`) is drawn as a
    /// chamfered rectangle rather than a diamond so a port has a flat run to land on.
    /// `[ui] mermaid_curve` only means anything under `"splines"` — an orthogonal line is a
    /// polyline of right angles, which has no curve to interpolate. An unrecognised value falls
    /// back to `"splines"`, like every unrecognised value in this file. Only a flowchart's own
    /// edges read this — every other diagram kind always routes the way it always has.
    pub mermaid_routing: String,
    /// Max height (terminal rows) of an inline mermaid diagram inside Markdown (default 24).
    /// Bigger = larger diagrams in the document flow (still width-capped and aspect-preserving;
    /// a diagram taller than the viewport scrolls in bands like any inline image). 0/invalid
    /// falls back to the default.
    pub mermaid_rows: u16,
    /// Restore the previous tab set on startup, **per start directory** (default true). konoma records
    /// each tab's root / cursor / previewed file into `~/.config/konoma/sessions/<start dir>.toml` on
    /// every tab open/close/switch and on quit; launching konoma in the same directory reopens those
    /// tabs. false = always start fresh (the session file is neither read nor written).
    pub restore_tabs: bool,
    /// When `restore_tabs` is on: also restore a session that has only ONE tab. Default true (matches
    /// the common `cd project && konoma` case). Set false to NOT persist single-tab sessions — quitting
    /// with a lone tab deletes that directory's session file and next launch starts fresh; sessions with
    /// two or more tabs still restore.
    pub restore_single_tab: bool,
    /// Where the cursor lands after moving to the parent directory (`h`). `"origin"` (default) puts
    /// it on the directory just left (the same spirit as `q` returning from a preview: you land back
    /// where you came from, not at the top). `"top"` restores the legacy behavior of always landing on
    /// the first row. Any other value is treated as `"origin"`. Descending into a directory (`l`)
    /// always starts at the top either way — there is no "came from" row to return to there.
    pub tree_cursor: String,
}

impl UiConfig {
    /// `filter_mode` resolved permissively: only the literal `"substring"` selects the legacy
    /// path; anything else (including a typo) is `"fuzzy"`, the default — a typo can't silently
    /// disable the tree filter.
    pub fn filter_mode(&self) -> &str {
        if self.filter_mode == "substring" {
            "substring"
        } else {
            "fuzzy"
        }
    }

    /// `md_task_states` resolved to chars: every entry must be exactly 1 char and there must be at
    /// least 2 states (a cycle needs somewhere to go) — otherwise fall back to the default (` `/`x`).
    /// The reference for `DocOptions::from_settings` (the Markdown plugin's options, which the app
    /// draws with): a test checks the two agree. Not used at run time.
    #[cfg(test)]
    pub fn md_task_state_chars(&self) -> Vec<char> {
        let chars: Vec<char> = self
            .md_task_states
            .iter()
            .filter_map(|s| {
                let mut it = s.chars();
                let c = it.next()?;
                it.next().is_none().then_some(c)
            })
            .collect();
        if chars.len() >= 2 && chars.len() == self.md_task_states.len() {
            chars
        } else {
            crate::preview::markdown::DEFAULT_TASK_STATES.to_vec()
        }
    }

    /// `md_table_align`/`md_image_align` resolved into the one type the Markdown renderer aligns
    /// blocks with. Permissive: an unrecognized value falls back to that option's own default
    /// (`left` for tables, `center` for images), so a typo can never blank or mis-place a block —
    /// the same "unknown value = default" contract `md_details` / `follow_view` / `commit_meta_align`
    /// already have.
    #[cfg(test)]
    pub fn md_block_aligns(&self) -> crate::preview::markdown::BlockAligns {
        use crate::preview::markdown::{BlockAlign, BlockAligns};
        let d = BlockAligns::default();
        BlockAligns {
            table: BlockAlign::from_config(&self.md_table_align, d.table),
            image: BlockAlign::from_config(&self.md_image_align, d.image),
        }
    }

    /// The math glyph color, sanitized against the **same parser usvg uses** (`svgtypes::Color`), so a
    /// value that passes here is one usvg will render — never one it silently falls back to *black* for
    /// (which is invisible on a dark terminal = the very "blank equation" bug). A `#hex`, `rgb(…)`, or a
    /// real CSS color name (`black`, `white`, …) passes; a typo (`wihte`), `none`, `currentColor`, or a
    /// fully-transparent color falls back to the light-gray default. Returning the *original* string is
    /// safe because usvg re-parses it identically.
    pub fn math_color(&self) -> &str {
        visible_math_color(&self.math_color)
    }
}

/// [`UiConfig::math_color`]'s test, for any value (`[plugins.math] color` is held to it too).
pub(crate) fn visible_math_color(value: &str) -> &str {
    use std::str::FromStr;
    let c = value.trim();
    let visible = svgtypes::Color::from_str(c)
        .map(|col| col.alpha != 0)
        .unwrap_or(false);
    if visible {
        c
    } else {
        "#d0d0d0"
    }
}

// The rule lives with `expand_tabs`, its only consumer; the config tests pin it from here.
#[cfg(test)]
pub(crate) use konoma_code_core::clamp_tab_width;

/// Default tree sort (`[ui.sort]`). Changeable at runtime via the `s` sort menu.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct SortConfig {
    /// Sort key. `"name"` (default) | `"size"` | `"modified"` | `"ext"`.
    pub key: String,
    /// Whether to sort descending (default false = ascending).
    pub reverse: bool,
    /// Whether to group directories first (default true). false = mixed with files and sorted by the key.
    pub dirs_first: bool,
}

impl Default for SortConfig {
    fn default() -> Self {
        Self {
            key: "name".into(),
            reverse: false,
            dirs_first: true,
        }
    }
}

/// Color settings. Colors are specified as `"#rrggbb"` / a name (`"black"`, `"lightblue"`, etc.) / an index (`"8"`).
/// `"none"` (or empty) means "unspecified."
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ThemeConfig {
    /// Background color of the whole app. `"none"` (default) keeps the terminal's default background (transparency stays intact too).
    pub bg: String,
    /// Markdown code background color (shared by inline code and code blocks). Default is dark slate.
    /// `"none"` removes the background band.
    pub code_bg: String,
    /// Alignment of code block language labels. `"right"` (default) or `"left"`.
    pub code_label_align: String,
    /// Background color of the language label (badge). `"auto"` (default) = a slightly brightened `code_bg` /
    /// `"none"` = no background / any color.
    pub code_label_bg: String,
    /// Theme name for code syntax highlighting (bundled with two-face). Default `"TwoDark"` (= Zed's
    /// One Dark). Others include `"Dracula"`/`"Nord"`/`"gruvbox-dark"`/`"Catppuccin Mocha"`/
    /// `"OneHalfDark"`/`"Solarized (dark)"`. Separators/case are ignored. An unknown name falls back to TwoDark.
    pub code_theme: String,
}

impl Default for ThemeConfig {
    fn default() -> Self {
        Self {
            bg: "none".into(), // default is the terminal's default background (not painted)
            code_bg: "#2b303b".into(), // = DEFAULT_CODE_BG (rgb 43,48,59)
            code_label_align: "right".into(),
            code_label_bg: "auto".into(),
            code_theme: "TwoDark".into(), // = Zed's One Dark
        }
    }
}

impl ThemeConfig {
    /// Resolves the app's background color. `"none"`/empty/invalid -> None (keeps the terminal default).
    pub fn bg(&self) -> Option<Color> {
        parse_color_opt(&self.bg, None)
    }

    /// Resolves the code background color. `"none"`/empty -> no background (None). Parse failure -> default color.
    pub fn code_bg(&self) -> Option<Color> {
        parse_color_opt(&self.code_bg, Some(DEFAULT_CODE_BG))
    }

    /// Whether to right-align the language label. false only when `"left"` is specified; otherwise the default right alignment.
    pub fn code_label_right(&self) -> bool {
        !self.code_label_align.trim().eq_ignore_ascii_case("left")
    }

    /// Background color of the language label (badge). `"auto"` is a brightened `code_bg`; `"none"` is no background.
    pub fn code_label_bg(&self) -> Option<Color> {
        if self.code_label_bg.trim().eq_ignore_ascii_case("auto") {
            return self.code_bg().map(lighten);
        }
        parse_color_opt(&self.code_label_bg, None)
    }
}

/// Resolves a color string. `"none"`/empty -> None. Parse success -> that color. Parse failure -> `fallback`.
fn parse_color_opt(s: &str, fallback: Option<Color>) -> Option<Color> {
    let s = s.trim();
    if s.is_empty() || s.eq_ignore_ascii_case("none") {
        return None;
    }
    match Color::from_str(s) {
        Ok(c) => Some(c),
        Err(_) => fallback,
    }
}

/// `[[preview.rules]]`: the user's assignments, tried first. The default assignments (markdown,
/// mermaid, svg, image, csv, ...) are not listed here; they come from the standard plugins'
/// declarations (`crate::plugins`), so the default is no rules at all.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct PreviewConfig {
    pub rules: Vec<Rule>,
}

/// A single preview rule. Matches by either glob or mime.
/// Specify either builtin (a built-in renderer name) or command (an external command).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Rule {
    pub glob: Option<String>,
    pub mime: Option<String>,
    pub builtin: Option<String>, // "markdown" | "mermaid" | "image" | "svg" | "video" | "pdf" | "code" | "archive" | "spreadsheet" | "document" | "text"
    pub command: Option<String>, // template: {path} {out}
    pub render_as: Option<String>, // how to treat the command's output: "image" | "text"
    pub detached: bool, // opens in a separate process so it doesn't block the TUI (video, etc.)
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            show_hidden: false,
            filter_mode: "fuzzy".into(),
            tabbar: "auto".into(),
            icons: true,
            wrap: true,
            line_numbers: false,
            git_gutter: true,
            tab_width: 4,
            syntax_highlight: true,
            preview_loading: "indicator".into(),
            path_style: "relative".into(),
            keys: "vim".into(),
            lang: "auto".into(),
            statusbar: "split".into(),
            theme: ThemeConfig::default(),
            image_render_scale: 1.0,
            svg_max_px: 800,
            sort: SortConfig::default(),
            details: Vec::new(),
            graph_max_branches: 12,
            graph_base_branches: Vec::new(),
            commit_meta_align: "right".into(),
            confirm_quit: true,
            confirm_bookmark_overwrite: true,
            confirm_jj_sync: true,
            csv_rainbow: true,
            follow_view: "diff".into(),
            diff_view: "rendered".into(),
            md_task_states: vec![" ".into(), "x".into()],
            md_autolink: true,
            md_alerts: true,
            md_emoji: true,
            md_frontmatter: true,
            md_footnotes: true,
            md_inline_html: true,
            md_details: "auto".into(),
            md_table_align: "left".into(),
            md_image_align: "center".into(),
            busy_indicator: true,
            mermaid: "image".into(),
            math: "image".into(),
            math_color: "#d0d0d0".into(),
            mermaid_theme: "dark".into(),
            mermaid_curve: "basis".into(),
            mermaid_routing: "splines".into(),
            mermaid_rows: 24,
            restore_tabs: true,
            restore_single_tab: true,
            tree_cursor: "origin".into(),
        }
    }
}

impl Config {
    /// Loads the config and returns (config, load error). On parse failure it returns the defaults along with an error string.
    /// The TUI uses this to surface the error as a startup message (since silently falling back to defaults would go unnoticed).
    pub fn load_reporting() -> (Self, Option<String>) {
        let Some(path) = dirs_config_path() else {
            return (Config::default(), None);
        };
        // An absent config file is normal (it runs on defaults). Only report an error when it can
        // be read but is broken.
        let Ok(text) = std::fs::read_to_string(&path) else {
            return (Config::default(), None);
        };
        match toml::from_str::<Config>(&text) {
            Ok(mut cfg) => {
                cfg.renamed_settings = Config::old_setting_names(&text, &cfg);
                (cfg, None)
            }
            Err(e) => {
                let msg = format_config_load_error(&e);
                // The config is broken = even the language setting can't be read, so notify in the
                // default (English).
                (
                    Config::default(),
                    Some(format!("config error (using defaults): {msg}")),
                )
            }
        }
    }

    /// The old `[ui]` names `text` (the config file) still uses, with what to write instead
    /// (`plugins::MOVED_UI_SETTINGS`). Empty when the file does not parse as a table.
    pub(crate) fn old_setting_names(text: &str, cfg: &Config) -> Vec<(String, String)> {
        let Ok(table) = text.parse::<toml::Table>() else {
            return Vec::new();
        };
        match table.get("ui") {
            Some(toml::Value::Table(ui)) => crate::plugins::moved_settings(ui, &cfg.plugins),
            _ => Vec::new(),
        }
    }

    /// Determines the PreviewKind for the given path. The user's `[[preview.rules]]` are tried
    /// first, top to bottom; when none matches, the standard plugins take it in their
    /// registration order (`crate::plugins`, `konoma_plugin::Registry::resolve_file`). When no
    /// plugin matches, it judges text vs. binary and falls back to the built-in text display for
    /// text, or safely to CanNotPreview for binary (design principle 3 "unsupported is handled
    /// safely"; picks up extensionless README/LICENSE/Makefile, etc.).
    pub fn resolve_preview(&self, path: &Path) -> PreviewKind {
        resolve_preview_kind(
            &self.preview.rules,
            &self.plugin_disabled(),
            self.external.preview_commands,
            path,
            None,
        )
    }

    /// `resolve_preview`, but classifying a file that may not exist on disk from `sniff` (its
    /// content) instead of reading `path`. Used for a **deleted** file
    /// (`docs/FEATURE-MEDIA-DIFF.md` §2): the ordinary path-based MIME sniff
    /// (`infer::get_from_path`) fails outright once the file is gone, so a rule like
    /// `mime = "image/*"` could never match a deleted PNG even though its old bytes (from git/jj)
    /// are right there. Glob rules are unaffected (they only ever look at the file *name*, which
    /// `path` still has even when the file itself doesn't exist), and the final text/binary fallback
    /// judges `sniff` the same way `is_probably_text` judges a file's leading bytes. Same rule order,
    /// same `[external] preview_commands` gate — this shares `resolve_preview`'s one implementation
    /// (`resolve_preview_kind`) rather than a second copy that could drift out of sync with it.
    // Not called from production code, deliberately: a deleted file's classification is instead
    // read off the media-diff worker's own landed outcome (`App::diff_representations`, `docs/
    // FEATURE-MEDIA-DIFF.md` §2). That worker sniffs the old bytes on a background thread via the
    // free-function form this method itself wraps (`resolve_preview_kind`, called directly by
    // `App::compute_media_diff` — not through this method, since the worker only has plain
    // `rules`/`preview_commands` data, not a whole `&Config`), so calling this synchronous, UI-
    // thread method a second time from anywhere in the render path would just be redundant I/O.
    // Kept as a public, directly-testable entry point for exactly that sniff behavior — exercised
    // by `config::parity_tests`.
    #[allow(dead_code)]
    pub fn resolve_preview_with(&self, path: &Path, sniff: &[u8]) -> PreviewKind {
        resolve_preview_kind(
            &self.preview.rules,
            &self.plugin_disabled(),
            self.external.preview_commands,
            path,
            Some(sniff),
        )
    }

    /// The ids of the plugins `[plugins.<id>] enabled = false` switches off.
    ///
    /// This list is also the key the registry is shared under: when external plugins were loaded
    /// it ends with their marker, so the registry holds them too (`plugins::build_registry`).
    pub fn plugin_disabled(&self) -> Vec<String> {
        let mut ids = crate::plugins::disabled_ids(&self.plugins);
        if let Some(token) = &self.external_plugins {
            ids.push(crate::plugins::external::marker(token));
        }
        ids
    }

    /// The plugin's own settings from `[plugins.<id>]` (everything but `enabled`).
    // Read by the plugins as they are wired to the display (later steps of stage 2).
    #[allow(dead_code)]
    pub fn plugin_settings(&self, id: &str) -> konoma_plugin::Settings {
        self.plugins
            .get(id)
            .map(crate::plugins::settings_of)
            .unwrap_or_default()
    }

    /// A one-line startup message about `[plugins.*]` mistakes (an unknown id, ...), if any.
    pub fn plugin_report(&self, lang: crate::i18n::Lang) -> Option<String> {
        let mut problems = crate::plugins::table_problems(&self.plugins, lang);
        if let Some(token) = &self.external_plugins {
            problems.extend(crate::plugins::external::problems(token, lang));
        }
        if problems.is_empty() {
            None
        } else {
            Some(format!("plugins: {}", problems.join(" / ")))
        }
    }
}

/// The one implementation `Config::resolve_preview`/`resolve_preview_with` both call — see the
/// latter's doc comment for why a deleted file needs this to be parameterized on an optional content
/// sniff rather than always reading `path` itself. `rules`/`disabled`/`preview_commands` are passed
/// as plain data (not `&Config`) so the media-diff worker (`app/media_diff.rs`) can call this from
/// a thread with only a cloned snapshot of the config fields it actually needs, mirroring how
/// `MdDiffRequest` carries plain `md_frontmatter`/`md_footnotes`/`md_inline_html` booleans instead
/// of a whole `&Config` (`app/md_diff.rs`'s module doc comment).
pub(crate) fn resolve_preview_kind(
    rules: &[Rule],
    disabled: &[String],
    preview_commands: bool,
    path: &Path,
    sniff: Option<&[u8]>,
) -> PreviewKind {
    #[cfg(test)]
    crate::test_support::note_resolve_preview_call();
    crate::plugins::resolve_kind(rules, disabled, preview_commands, path, sniff)
}

/// Test stand-in for the matcher that used to live here: whether `rule` takes `path`, judged by the
/// plugin registry's own matching (`plugins::rule_hits`). The content sniff is the file's own, or
/// the given bytes.
#[cfg(test)]
fn rule_matches(rule: &Rule, path: &Path, sniff: Option<&[u8]>) -> bool {
    let kind = match sniff {
        Some(bytes) => infer::get(bytes),
        None => infer::get_from_path(path).ok().flatten(),
    };
    crate::plugins::rule_hits(rule, path, kind.map(|k| k.mime_type()))
}

/// Test stand-in: does a rule with `mime = pattern` take content that reads as `mime`?
#[cfg(test)]
fn mime_glob_match(pattern: &str, mime: &str) -> bool {
    let rule = Rule {
        mime: Some(pattern.to_string()),
        ..Rule::default()
    };
    crate::plugins::rule_hits(&rule, Path::new("/x/file"), Some(mime))
}

pub(crate) use konoma_base::xdg::dirs_config_path;
#[cfg(test)]
use konoma_base::xdg::dirs_config_path_from;

use konoma_host_core::shared::format_load_error as format_config_load_error;

#[cfg(test)]
mod parity_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::unique_tmp;
    use std::collections::HashMap;
    use std::io::Write;

    fn tmp(name: &str, bytes: &[u8]) -> crate::test_support::TmpDir {
        // Preserve `name`'s extension (many of these tests exercise extension-based preview-rule
        // matching, including uppercase/mixed-case extensions) — putting the uniqueness suffix
        // straight after the whole `name` would swallow it (`"pic.svg"` → `"pic.svg_1234_5"`, no
        // longer ending in `.svg`). Put the suffix on the stem instead, then re-append the
        // extension via `with_extension` (a no-op when `name` has none).
        let stem = std::path::Path::new(name)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| name.to_string());
        let ext = std::path::Path::new(name)
            .extension()
            .map(|e| e.to_string_lossy().into_owned());
        let mut p = unique_tmp(&format!("konoma_cfg_test_{stem}"));
        if let Some(ext) = ext {
            p = p.with_extension(ext);
        }
        let mut f = std::fs::File::create(&p).unwrap();
        f.write_all(bytes).unwrap();
        p
    }

    #[test]
    fn md_task_state_chars_validates_and_falls_back() {
        let mut ui = UiConfig::default();
        assert_eq!(ui.md_task_state_chars(), vec![' ', 'x'], "既定");
        ui.md_task_states = vec![" ".into(), "/".into(), "x".into()];
        assert_eq!(
            ui.md_task_state_chars(),
            vec![' ', '/', 'x'],
            "カスタム3状態"
        );
        // Invalid: a 2-character entry → falls back to the default.
        ui.md_task_states = vec![" ".into(), "xx".into()];
        assert_eq!(ui.md_task_state_chars(), vec![' ', 'x']);
        // Invalid: only 1 entry (nowhere to cycle to) → falls back to the default. Same for empty.
        ui.md_task_states = vec!["x".into()];
        assert_eq!(ui.md_task_state_chars(), vec![' ', 'x']);
        ui.md_task_states = Vec::new();
        assert_eq!(ui.md_task_state_chars(), vec![' ', 'x']);
    }

    /// Regression test for the unbounded `[ui] tab_width` bug: `clamp_tab_width` (which
    /// `preview::code::expand_tabs` applies to whatever raw value it is handed) must clamp
    /// anything outside a realistic tab-stop range to the default, while preserving `0`'s
    /// documented special meaning ("expansion disabled"), which is *not* an error value and must
    /// not be folded into the fallback. Every case here is a pure comparison with no allocation,
    /// so it is safe to test at the actual reported magnitude (`9223372036854775807`) and even
    /// `usize::MAX` directly — unlike `expand_tabs` itself (covered separately, with a safe
    /// magnitude, in `preview::code::tests`), calling this function can never abort.
    #[test]
    fn clamp_tab_width_bounds_to_a_realistic_range_but_keeps_zero_as_disable() {
        // 0 keeps its documented special meaning (disable expansion) — it is not "invalid" and
        // must not be rounded up to the default.
        assert_eq!(clamp_tab_width(0), 0, "0=無効化は既定へ丸めない");
        // Realistic values pass through unchanged.
        assert_eq!(clamp_tab_width(1), 1);
        assert_eq!(clamp_tab_width(4), 4, "既定と同じ値もそのまま");
        assert_eq!(clamp_tab_width(16), 16, "上限16は許容");
        // Out of range falls back to the default (4): a plausible typo, the value from the bug
        // report, and the type's true maximum.
        assert_eq!(clamp_tab_width(17), 4, "上限超は既定へ");
        assert_eq!(clamp_tab_width(1_000_000_000), 4, "誤記(10億)も既定へ");
        assert_eq!(
            clamp_tab_width(9_223_372_036_854_775_807),
            4,
            "報告された極端値(9223372036854775807)も既定へ"
        );
        assert_eq!(clamp_tab_width(usize::MAX), 4, "usize::MAX も既定へ");
    }

    #[test]
    fn markdown_rule_still_wins() {
        let p = tmp("README.md", b"# title\n");
        let kind = Config::default().resolve_preview(&p);
        assert!(kind.is_markdown(), "got {kind:?}");
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn glob_matches_uppercase_extension() {
        // Uppercase extensions (README.MD / equivalent to *.RS) also hit the rule case-insensitively.
        let p = tmp("README.MD", b"# title\n");
        let kind = Config::default().resolve_preview(&p);
        assert!(kind.is_markdown(), "got {kind:?}");
        std::fs::remove_file(&p).ok();

        let p = tmp("MAIN.RS", b"fn main() {}\n");
        let kind = Config::default().resolve_preview(&p);
        assert!(kind.is_code_plugin(), "got {kind:?}");
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn editor_build_argv_substitutes_or_appends_path() {
        let path = Path::new("/tmp/a.rs");
        // When {path} is present, substitute it in place (no line specified).
        assert_eq!(
            build_argv("code -g {path}:1", path, None),
            vec!["code", "-g", "/tmp/a.rs:1"]
        );
        // Otherwise append it at the end.
        assert_eq!(build_argv("nvim", path, None), vec!["nvim", "/tmp/a.rs"]);
        assert_eq!(
            build_argv("code -w", path, None),
            vec!["code", "-w", "/tmp/a.rs"]
        );
        // An empty template falls back to vim.
        assert_eq!(build_argv("   ", path, None), vec!["vim", "/tmp/a.rs"]);
    }

    #[test]
    fn editor_build_argv_opens_at_line() {
        let path = Path::new("/tmp/a.rs");
        // The {line} token inserts the line number in place (an explicit template takes priority = no injection).
        assert_eq!(
            build_argv("code -g {path}:{line}", path, Some(42)),
            vec!["code", "-g", "/tmp/a.rs:42"]
        );
        assert_eq!(
            build_argv("nvim +{line} {path}", path, Some(42)),
            vec!["nvim", "+42", "/tmp/a.rs"]
        );
        // No {line} token + a line is given = auto-inject per known editor.
        // The vim family gets +N (cursor) + +normal! zt (scroll that line to the top of the window).
        assert_eq!(
            build_argv("vim", path, Some(42)),
            vec!["vim", "+42", "+normal! zt", "/tmp/a.rs"]
        );
        assert_eq!(
            build_argv("nvim", path, Some(7)),
            vec!["nvim", "+7", "+normal! zt", "/tmp/a.rs"]
        );
        // Even an empty template (= vim fallback) still gets the line + zt.
        assert_eq!(
            build_argv("   ", path, Some(9)),
            vec!["vim", "+9", "+normal! zt", "/tmp/a.rs"]
        );
        // nano/emacs etc. get only +N (zt is a vim-only command, so it's not added).
        assert_eq!(
            build_argv("nano", path, Some(5)),
            vec!["nano", "+5", "/tmp/a.rs"]
        );
        assert_eq!(
            build_argv("emacs", path, Some(8)),
            vec!["emacs", "+8", "/tmp/a.rs"]
        );
        // VS Code is -g path:line.
        assert_eq!(
            build_argv("code -w", path, Some(15)),
            vec!["code", "-g", "-w", "/tmp/a.rs:15"]
        );
        // Sublime/Helix is path:line.
        assert_eq!(build_argv("hx", path, Some(3)), vec!["hx", "/tmp/a.rs:3"]);
        // An unknown editor gets no injection (opens at the top).
        assert_eq!(
            build_argv("weirded", path, Some(3)),
            vec!["weirded", "/tmp/a.rs"]
        );
        // With no line (editing from the tree, etc.), no injection, as before.
        assert_eq!(build_argv("vim", path, None), vec!["vim", "/tmp/a.rs"]);
    }

    #[test]
    fn editor_configured_prefers_ext_then_command() {
        let mut e = EditorConfig {
            command: "nvim".into(),
            ext: HashMap::new(),
        };
        e.ext.insert("md".into(), "code -w".into());
        e.ext.insert("blank".into(), "   ".into()); // whitespace-only is invalid
                                                    // per-extension takes highest priority.
        assert_eq!(e.configured("md").as_deref(), Some("code -w"));
        // Falls back to command (the overall default) when there's no per-extension entry.
        assert_eq!(e.configured("rs").as_deref(), Some("nvim"));
        // A whitespace-only per-extension entry is invalid → falls back to command.
        assert_eq!(e.configured("blank").as_deref(), Some("nvim"));
        // If command is also empty, None (= the caller falls through to env → vim).
        let empty = EditorConfig::default();
        assert_eq!(empty.configured("rs"), None);
    }

    #[test]
    fn editor_resolve_uses_ext_mapping() {
        let mut e = EditorConfig {
            command: "nvim".into(),
            ext: HashMap::new(),
        };
        e.ext.insert("md".into(), "code -w".into());
        // Hits even an uppercase extension via lowercase matching, appends the path at the end.
        assert_eq!(
            e.resolve(Path::new("/x/NOTE.MD"), None),
            vec!["code", "-w", "/x/NOTE.MD"]
        );
        // An unregistered extension uses command.
        assert_eq!(
            e.resolve(Path::new("/x/a.rs"), None),
            vec!["nvim", "/x/a.rs"]
        );
    }

    #[test]
    fn unknown_text_falls_back_to_text() {
        // No extension, matches no rule, plain text (equivalent to LICENSE/Makefile).
        let p = tmp("LICENSE", b"MIT License\n\nPermission...\n");
        let kind = Config::default().resolve_preview(&p);
        assert!(matches!(kind, PreviewKind::Text(_)), "got {kind:?}");
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn theme_code_bg_parses_hex_none_and_invalid() {
        // The default (empty config) is the default color.
        assert_eq!(ThemeConfig::default().code_bg(), Some(DEFAULT_CODE_BG));
        // hex specification.
        let t = ThemeConfig {
            code_bg: "#101820".into(),
            ..Default::default()
        };
        assert_eq!(t.code_bg(), Some(Color::Rgb(16, 24, 32)));
        // "none" / whitespace / mixed case → no background.
        for v in ["none", "  NONE  ", ""] {
            let t = ThemeConfig {
                code_bg: v.into(),
                ..Default::default()
            };
            assert_eq!(t.code_bg(), None, "{v:?} は None になるべき");
        }
        // An invalid value doesn't crash — it falls back to the default color.
        let t = ThemeConfig {
            code_bg: "definitely-not-a-color".into(),
            ..Default::default()
        };
        assert_eq!(t.code_bg(), Some(DEFAULT_CODE_BG));
    }

    #[test]
    fn theme_bg_parses_and_defaults_to_none() {
        // Overall background: the default is none (stays the terminal default).
        assert_eq!(ThemeConfig::default().bg(), None);
        // A color specification is applied.
        let t = ThemeConfig {
            bg: "#102030".into(),
            ..Default::default()
        };
        assert_eq!(t.bg(), Some(Color::Rgb(16, 32, 48)));
        // An invalid value is None (falls back to the terminal default = not painted).
        let t = ThemeConfig {
            bg: "nope-xyz".into(),
            ..Default::default()
        };
        assert_eq!(t.bg(), None);
    }

    #[test]
    fn image_render_scale_defaults_and_parses() {
        assert_eq!(Config::default().ui.image_render_scale, 1.0);
        let p = tmp("imgscale.toml", b"[ui]\nimage_render_scale = 0.3\n");
        let text = std::fs::read_to_string(&p).unwrap();
        let cfg: Config = toml::from_str(&text).unwrap();
        assert_eq!(cfg.ui.image_render_scale, 0.3);
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn code_label_align_and_bg_resolve() {
        // Default: right-aligned · auto (= code_bg brightened).
        let d = ThemeConfig::default();
        assert!(d.code_label_right());
        assert_eq!(d.code_label_bg(), Some(lighten(DEFAULT_CODE_BG)));
        // Left-aligned specification.
        let t = ThemeConfig {
            code_label_align: "left".into(),
            ..Default::default()
        };
        assert!(!t.code_label_right());
        // "left" case-insensitively.
        let t = ThemeConfig {
            code_label_align: "LEFT".into(),
            ..Default::default()
        };
        assert!(!t.code_label_right());
        // Badge background: any color / none.
        let t = ThemeConfig {
            code_label_bg: "#ff0000".into(),
            ..Default::default()
        };
        assert_eq!(t.code_label_bg(), Some(Color::Rgb(255, 0, 0)));
        let t = ThemeConfig {
            code_label_bg: "none".into(),
            ..Default::default()
        };
        assert_eq!(t.code_label_bg(), None);
        // code_bg=none + auto → the badge also has no background.
        let t = ThemeConfig {
            code_bg: "none".into(),
            code_label_bg: "auto".into(),
            ..Default::default()
        };
        assert_eq!(t.code_label_bg(), None);
    }

    #[test]
    fn broken_config_reports_error_and_falls_back() {
        // Broken TOML fails to parse (Err) and falls back to the default, along with an error string.
        let broken = "[keys]\ncopy_prefix = \n"; // no value = syntax error
        let parsed = toml::from_str::<Config>(broken);
        assert!(parsed.is_err(), "壊れた TOML は Err になる");
        // Mimics load_reporting's formatting (compressing to one line): Err → Some(msg).
        let msg = parsed.err().map(|e| e.to_string());
        assert!(msg.is_some());
    }

    #[test]
    fn copy_keys_default_and_custom() {
        // The old [keys] copy_* scalars (backward-compat aliases) parse correctly from TOML.
        let d = KeysConfig::default();
        assert_eq!(d.copy_prefix, "c");
        assert_eq!(d.copy_name, "n");
        assert_eq!(d.copy_full, "p");
        // vim-style: set the prefix to y.
        let p = tmp(
            "keys.toml",
            b"[keys]\ncopy_prefix = \"y\"\ncopy_full = \"P\"\n",
        );
        let text = std::fs::read_to_string(&p).unwrap();
        let cfg: Config = toml::from_str(&text).unwrap();
        assert_eq!(cfg.keys.copy_prefix, "y");
        assert_eq!(cfg.keys.copy_full, "P");
        assert_eq!(cfg.keys.copy_name, "n", "未指定は既定のまま");
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn keys_surfaces_parse_from_toml() {
        // The new-form [keys.<context>] subtables are picked up into surfaces and can coexist with
        // the old copy_* scalars.
        let p = tmp(
            "keys_surfaces.toml",
            br#"
[keys]
copy_prefix = "y"

[keys.tree]
d = "refresh"
"space d" = "file_delete"

[keys.preview_text]
"ctrl-f" = "navigate:page_down"
"#,
        );
        let text = std::fs::read_to_string(&p).unwrap();
        let cfg: Config = toml::from_str(&text).unwrap();
        assert_eq!(cfg.keys.copy_prefix, "y");
        assert_eq!(
            cfg.keys.surfaces.get("tree").unwrap().get("d").unwrap(),
            "refresh"
        );
        assert_eq!(
            cfg.keys
                .surfaces
                .get("tree")
                .unwrap()
                .get("space d")
                .unwrap(),
            "file_delete"
        );
        assert_eq!(
            cfg.keys
                .surfaces
                .get("preview_text")
                .unwrap()
                .get("ctrl-f")
                .unwrap(),
            "navigate:page_down"
        );
        std::fs::remove_file(&p).ok();
    }

    /// A context name with a dot (`git.changes`) must be quoted in TOML: `[keys."git.changes"]` is
    /// one table called `git.changes`, whereas `[keys.git.changes]` is a table `git` holding a table
    /// `changes`, which is not a key table. The quoted form reaches the keymap; the unquoted one is
    /// refused with the position in the file (the whole config falls back to defaults and says so,
    /// like any other malformed `[keys]`).
    #[test]
    fn keys_context_names_with_a_dot_need_quotes() {
        use crate::keymap::{Action, Context, KeyMap, KeyPress, KeyScheme, Resolution};
        let ok: Config = toml::from_str(
            "[keys.\"git.changes\"]\nZ = \"refresh\"\n\n[keys.\"jj.log\"]\nZ = \"tab_new\"\n",
        )
        .unwrap();
        assert_eq!(ok.keys.surfaces["git.changes"]["Z"], "refresh");
        assert_eq!(ok.keys.surfaces["jj.log"]["Z"], "tab_new");
        let km = KeyMap::from_config(KeyScheme::Vim, &ok.keys.to_keymap_config());
        #[cfg(feature = "git")]
        {
            use crate::keymap::{Backend, Screen};
            assert!(km.warnings.is_empty(), "{:?}", km.warnings);
            assert_eq!(
                km.resolve(
                    Context::Screen(Backend::Git, Screen::Changes),
                    None,
                    KeyPress::ch('Z')
                ),
                Resolution::Action(Action::Refresh)
            );
            assert_eq!(
                km.resolve(
                    Context::Screen(Backend::Jj, Screen::Log),
                    None,
                    KeyPress::ch('Z')
                ),
                Resolution::Action(Action::TabNew)
            );
        }
        #[cfg(not(feature = "git"))]
        {
            let _ = (
                Context::Tree,
                KeyPress::ch('Z'),
                Action::Quit,
                Resolution::Unbound,
            );
            assert!(!km.warnings.is_empty(), "no git: the names are unknown");
        }

        let bad = toml::from_str::<Config>("[keys.git.changes]\nZ = \"refresh\"\n")
            .expect_err("a nested table is not a key table");
        let msg = format_config_load_error(&bad);
        assert!(msg.contains("line 1"), "{msg}");
    }

    #[test]
    fn to_keymap_config_maps_changed_copy_alias_only() {
        use crate::app::CopyKind;
        use crate::keymap::{Action, Context, KeyMap, KeyPress, KeyScheme, LeaderId, Resolution};
        // A copy_* left at its default doesn't clobber the new defaults (n/r/f/p).
        let def = KeysConfig::default();
        let km = KeyMap::from_config(KeyScheme::Vim, &def.to_keymap_config());
        // f=Full / p=Parent (the new defaults) are preserved.
        assert_eq!(
            km.resolve(Context::Tree, Some(LeaderId::Copy), KeyPress::ch('f')),
            Resolution::Action(Action::CopyPath(CopyKind::Full))
        );
        assert_eq!(
            km.resolve(Context::Tree, Some(LeaderId::Copy), KeyPress::ch('p')),
            Resolution::Action(Action::CopyPath(CopyKind::Parent))
        );

        // The user changed copy_full to P → `y P` is added as Copy(Full) (the new default remains).
        let custom = KeysConfig {
            copy_full: "P".into(),
            ..KeysConfig::default()
        };
        let km2 = KeyMap::from_config(KeyScheme::Vim, &custom.to_keymap_config());
        assert_eq!(
            km2.resolve(Context::Tree, Some(LeaderId::Copy), KeyPress::ch('P')),
            Resolution::Action(Action::CopyPath(CopyKind::Full))
        );
        // The new default f=Full also remains (additive).
        assert_eq!(
            km2.resolve(Context::Tree, Some(LeaderId::Copy), KeyPress::ch('f')),
            Resolution::Action(Action::CopyPath(CopyKind::Full))
        );
    }

    #[test]
    fn theme_parses_from_toml() {
        let p = tmp(
            "theme.toml",
            b"[ui.theme]\nbg = \"#000010\"\ncode_bg = \"none\"\n",
        );
        let text = std::fs::read_to_string(&p).unwrap();
        let cfg: Config = toml::from_str(&text).unwrap();
        assert_eq!(cfg.ui.theme.code_bg(), None);
        assert_eq!(cfg.ui.theme.bg(), Some(Color::Rgb(0, 0, 16)));
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn unknown_binary_falls_back_to_can_not_preview() {
        let p = tmp("mystery", &[0x00, 0x01, 0x02, 0x03]);
        let kind = Config::default().resolve_preview(&p);
        assert!(
            matches!(kind, PreviewKind::CanNotPreview { .. }),
            "got {kind:?}"
        );
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn env_nonempty_trims_and_filters_blank() {
        // Use a unique key name to avoid interference from parallel tests. Covers set/unset/whitespace.
        let key = "KONOMA_TEST_ENV_NONEMPTY_PROBE";
        std::env::remove_var(key);
        assert_eq!(env_nonempty(key), None, "未設定は None");
        std::env::set_var(key, "  spaced  ");
        assert_eq!(
            env_nonempty(key).as_deref(),
            Some("spaced"),
            "前後空白は trim"
        );
        std::env::set_var(key, "   ");
        assert_eq!(env_nonempty(key), None, "空白のみは None");
        std::env::remove_var(key);
    }

    #[test]
    fn mime_glob_match_prefix_and_exact() {
        assert!(
            mime_glob_match("image/*", "image/png"),
            "プレフィックス一致"
        );
        assert!(mime_glob_match("image/*", "image/"), "境界(空サブタイプ)");
        assert!(
            !mime_glob_match("image/*", "video/mp4"),
            "別カテゴリは不一致"
        );
        assert!(mime_glob_match("text/plain", "text/plain"), "完全一致");
        assert!(
            !mime_glob_match("text/plain", "text/html"),
            "完全指定は厳密"
        );
    }

    #[test]
    fn dirs_config_path_is_under_home_config() {
        // HOME isn't changed in this test (won't break even run in parallel with other tests).
        if std::env::var_os("HOME").is_some() {
            let p = dirs_config_path().expect("HOME があれば Some");
            assert!(
                p.ends_with(".config/konoma/config.toml"),
                "設定パスの末尾: {}",
                p.display()
            );
        }
    }

    /// Serializes this module's env-mutating test (below) against itself running concurrently with
    /// a copy of itself on another thread — `cargo test`'s default runner is multi-threaded, and
    /// `VISUAL`/`EDITOR` are process-global — and, via the `Drop`-based guard the test builds
    /// around it, guarantees they are restored even if one of the assertions in between panics.
    ///
    /// Same shape as `preview::video::PATH_MUTATING_TESTS` (that module's identical fix for `PATH`/
    /// this process's fd 0): without the `Drop` guard, a failing assertion partway through step 3
    /// below used to leave `VISUAL`/`EDITOR` mutated to `"myvisual"`/`"myeditor"` for the rest of
    /// this test binary's process — this project's actual editor-open flow
    /// (`main.rs::run_editor` → this same `EditorConfig::resolve`) reads those exact two vars for
    /// its own no-config-override fallback, so a later e2e test exercising `e` with the default
    /// config could then silently try to spawn `myvisual`/`myeditor` as a real command instead of
    /// the environment's actual editor — a confusing, hard-to-reproduce failure in an unrelated
    /// test, not in this one.
    static EDITOR_ENV_MUTATING_TESTS: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn editor_config_resolve_priority_ext_command_env_default() {
        let _guard = EDITOR_ENV_MUTATING_TESTS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        use std::path::Path;
        // 1) Per-extension takes highest priority.
        let mut ec = EditorConfig {
            command: "code -w".into(),
            ext: HashMap::new(),
        };
        ec.ext.insert("rs".into(), "nvim {path}".into());
        let argv = ec.resolve(Path::new("/x/main.rs"), None);
        assert_eq!(
            argv,
            vec!["nvim".to_string(), "/x/main.rs".to_string()],
            "{{path}} 置換"
        );
        // 2) When the extension isn't specified, command (appended at the end).
        let argv = ec.resolve(Path::new("/x/readme.md"), None);
        assert_eq!(
            argv,
            vec![
                "code".to_string(),
                "-w".to_string(),
                "/x/readme.md".to_string()
            ]
        );

        // 3) When both command and ext are empty: $VISUAL → $EDITOR → vim, in that order.
        //    Only this resolve reads VISUAL/EDITOR. Restoration is guaranteed by `RestoreEnv`'s
        //    `Drop` below (fires on normal return *and* on an assertion panic — this project
        //    disallows `panic = "abort"`, so unwinding + `Drop` cleanup is reliable here), not by
        //    reaching the end of the function.
        let empty = EditorConfig {
            command: String::new(),
            ext: HashMap::new(),
        };
        let save_v = std::env::var_os("VISUAL");
        let save_e = std::env::var_os("EDITOR");

        struct RestoreEnv {
            visual: Option<std::ffi::OsString>,
            editor: Option<std::ffi::OsString>,
        }
        impl Drop for RestoreEnv {
            fn drop(&mut self) {
                match &self.visual {
                    Some(v) => std::env::set_var("VISUAL", v),
                    None => std::env::remove_var("VISUAL"),
                }
                match &self.editor {
                    Some(v) => std::env::set_var("EDITOR", v),
                    None => std::env::remove_var("EDITOR"),
                }
            }
        }
        let _restore = RestoreEnv {
            visual: save_v,
            editor: save_e,
        };

        std::env::set_var("VISUAL", "myvisual");
        std::env::set_var("EDITOR", "myeditor");
        assert_eq!(
            empty.resolve(Path::new("/x/f"), None),
            vec!["myvisual".to_string(), "/x/f".to_string()],
            "VISUAL 優先"
        );
        std::env::remove_var("VISUAL");
        assert_eq!(
            empty.resolve(Path::new("/x/f"), None),
            vec!["myeditor".to_string(), "/x/f".to_string()],
            "次に EDITOR"
        );
        std::env::remove_var("EDITOR");
        assert_eq!(
            empty.resolve(Path::new("/x/f"), None),
            vec!["vim".to_string(), "/x/f".to_string()],
            "最後は vim"
        );
        // `_restore` drops here (function exit), restoring VISUAL/EDITOR regardless of whether any
        // assertion above panicked.
    }

    #[test]
    fn load_reporting_returns_config_and_well_formed_error() {
        // This depends on the real environment (whether ~/.config/konoma/config.toml exists and
        // its contents), so only invariants are verified: on a parse error the string has a fixed
        // prefix. It doesn't crash.
        let (_cfg, err) = Config::load_reporting();
        if let Some(e) = err {
            assert!(
                e.starts_with("config error (using defaults): "),
                "エラー文言の接頭辞が規約どおり: {e}"
            );
        }
    }

    /// Regression test for the reported bug: `dirs_config_path` used to read only `$HOME` and
    /// silently ignore `$XDG_CONFIG_HOME`, unlike `app::cache_root` (which already honored
    /// `XDG_CACHE_HOME`). This exercises the *real*, env-reading `dirs_config_path()` against
    /// whatever environment the process was actually started with — a no-op when `XDG_CONFIG_HOME`
    /// isn't set (most normal `cargo test` runs), so it never races other tests over process-wide
    /// env state. To make it actually exercise the bug/fix, run it standalone in a dedicated
    /// subprocess with a custom environment, e.g.:
    ///   `env XDG_CONFIG_HOME=/custom/xdg HOME=/custom/home <test binary> dirs_config_path_honors_xdg_config_home_when_set --test-threads=1`
    /// Before this fix: FAILED (the real path stayed under `/custom/home/.config` and ignored
    /// `/custom/xdg` entirely). After this fix: passes.
    #[test]
    fn dirs_config_path_honors_xdg_config_home_when_set() {
        if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME") {
            if !xdg.is_empty() {
                let p = dirs_config_path();
                assert_eq!(
                    p,
                    Some(std::path::PathBuf::from(&xdg).join("konoma/config.toml")),
                    "XDG_CONFIG_HOME={xdg:?} が設定されていれば dirs_config_path はその配下を指すべき: {p:?}"
                );
            }
        }
    }

    /// Pure core of `dirs_config_path`. `None` in means `None` out; otherwise it's always
    /// `<base>/konoma/config.toml`, matching `crate::bookmarks::default_base_from`'s `<base>/konoma`
    /// exactly (see the next test for the cross-function parity check).
    #[test]
    fn dirs_config_path_from_joins_konoma_config_toml_onto_the_resolved_base() {
        assert_eq!(
            dirs_config_path_from(Some(std::path::PathBuf::from("/x/.config"))),
            Some(std::path::PathBuf::from("/x/.config/konoma/config.toml"))
        );
        assert_eq!(dirs_config_path_from(None), None);
    }

    /// `dirs_config_path()` must agree with `crate::bookmarks::xdg_config_home()` — the same shared
    /// rule that `crate::bookmarks::default_base()` also routes through (see task 1 in the
    /// accompanying report: three independent implementations used to disagree). Deliberately
    /// compared against `xdg_config_home()` directly rather than `default_base()`: unlike
    /// bookmarks/session storage, `dirs_config_path` is read-only and correctly stays `None` (no
    /// config file to read, use defaults) when neither `XDG_CONFIG_HOME` nor `HOME` is set, instead
    /// of following `default_base`'s system-temp-dir fallback (which exists only because
    /// bookmarks/session storage *writes* and must never fall back to a relative path). Exercised
    /// against the real environment (read-only, no env mutation), so it holds for whatever
    /// `HOME`/`XDG_CONFIG_HOME` this test happens to run under.
    #[test]
    fn dirs_config_path_agrees_with_xdg_config_home() {
        assert_eq!(
            dirs_config_path(),
            crate::bookmarks::xdg_config_home().map(|b| b.join("konoma").join("config.toml")),
            "config ファイルパスは bookmarks/session と同じ共有ルール(xdg_config_home)で解決されるべき"
        );
    }

    /// Task 3 (reported): a single bad field (`[ui] mermaid_rows = 100000`, out of `u16` range)
    /// makes `toml::from_str::<Config>` fail for the **whole** document — confirmed here: even
    /// though `show_hidden = true` right next to it is perfectly valid, `Err` is returned and
    /// `load_reporting`'s `Err` arm replaces the *entire* config with `Config::default()`, silently
    /// discarding `show_hidden` too (not just `mermaid_rows`). This is inherent to how
    /// `#[derive(Deserialize)]` on a struct works (the whole struct either builds or it doesn't) —
    /// per the report, a real per-field partial-recovery rewrite is out of scope for this fix; see
    /// the report for why and what a coarser (per-top-level-table) alternative would look like.
    #[test]
    fn one_bad_field_currently_fails_the_whole_document_not_just_that_field() {
        let toml = r#"
[ui]
show_hidden = true
mermaid_rows = 100000
"#;
        assert!(
            toml::from_str::<Config>(toml).is_err(),
            "u16 を超える mermaid_rows は文書全体の parse を失敗させる(実測)"
        );
    }

    /// Regression test for the reported "実質サイレント" 1-line message: before this fix,
    /// `format_config_load_error` kept only the first line of `toml::de::Error`'s `Display`, which
    /// for every parse error is just `"TOML parse error at line N, column M"` — the location, never
    /// the actual reason (that's on a *different* line of `Display`, or in `Error::message()`).
    /// Confirmed FAILING against that old formula for all three scenarios below (none of the
    /// messages contained the word describing what was actually wrong).
    #[test]
    fn format_config_load_error_names_the_actual_reason_not_just_the_location() {
        // A field with the wrong type (`[ui] mermaid_rows = 100000`, out of u16 range).
        let bad_type = "\n[ui]\nshow_hidden = true\nmermaid_rows = 100000\n";
        let e1 = toml::from_str::<Config>(bad_type).unwrap_err();
        let msg1 = format_config_load_error(&e1);
        assert!(
            msg1.contains("u16"),
            "型エラーの理由(u16 期待)がメッセージに含まれるべき: {msg1:?}"
        );
        assert!(
            msg1.contains("line 4"),
            "位置情報も残っているべき: {msg1:?}"
        );

        // A typo under `[keys]` (`copy_prefx` instead of `copy_prefix`): the extra key is captured
        // by `KeysConfig`'s `#[serde(flatten)] surfaces: HashMap<String, HashMap<String, String>>`,
        // so the scalar `"y"` fails to deserialize as a sub-table.
        let bad_key = "\n[ui]\nshow_hidden = true\n\n[keys]\ncopy_prefx = \"y\"\n";
        let e2 = toml::from_str::<Config>(bad_key).unwrap_err();
        let msg2 = format_config_load_error(&e2);
        assert!(
            msg2.contains("expected a map"),
            "型エラーの理由(map 期待)がメッセージに含まれるべき: {msg2:?}"
        );

        // A plain syntax error (unclosed table header). Under toml 0.8, `Error::message()` for
        // this shape spanned two lines ("invalid table header\nexpected `.`, `]`"), which is what
        // motivated the `\n` -> `; ` collapse in `format_config_load_error`. As of toml >=0.9's
        // rewritten parser, `message()` no longer embeds newlines for any malformed-TOML shape
        // probed here (confirmed across ~10 distinct syntax errors) -- the reason text itself
        // changed too ("unclosed table, expected `]`", arguably more accurate for this input than
        // the old wording). The `!contains('\n')` assertion below is kept as a still-true defensive
        // invariant, even though no known toml::de::Error currently exercises the collapse it guards.
        let syntax_broken = "[ui\nshow_hidden = true\n";
        let e3 = toml::from_str::<Config>(syntax_broken).unwrap_err();
        let msg3 = format_config_load_error(&e3);
        assert!(
            !msg3.contains('\n'),
            "flash は1行なので、複数行に渡る reason は改行を潰して1行化すべき: {msg3:?}"
        );
        assert!(
            msg3.contains("unclosed table"),
            "構文エラーの理由も含まれるべき: {msg3:?}"
        );
    }
    /// The shared slice (`konoma_host_core::shared::SharedConfig`) must read the same values as
    /// konoma's own `Config` from the same text: both example files, plus a file that sets every
    /// shared field to a non-default value (the examples mostly restate defaults).
    #[test]
    fn shared_config_agrees_with_config_on_the_common_fields() {
        let rich = "[ui]\nlang = \"jp\"\nkeys = \"less\"\n[editor]\ncommand = \"nvim\"\n[editor.ext]\nmd = \"code -w\"\n[keys]\ncopy_name = \"m\"\n[keys.tree]\n\"space d\" = \"file_delete\"\n[external]\nopen_links = false\noffice_apps = false\n";
        for (name, text) in [
            ("en", include_str!("../../config.example.toml")),
            ("ja", include_str!("../../config.example.ja.toml")),
            ("rich", rich),
            ("empty", ""),
        ] {
            let c: Config = toml::from_str(text).unwrap_or_else(|e| panic!("{name}: {e}"));
            let s = konoma_host_core::shared::SharedConfig::parse(text)
                .unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(c.ui.lang, s.ui.lang, "{name}: ui.lang");
            assert_eq!(c.ui.keys, s.ui.keys, "{name}: ui.keys");
            assert_eq!(c.external.open_links, s.external.open_links, "{name}");
            assert_eq!(c.external.office_apps, s.external.office_apps, "{name}");
            assert_eq!(c.editor.command, s.editor.command, "{name}: editor.command");
            assert_eq!(c.editor.ext, s.editor.ext, "{name}: editor.ext");
            assert_eq!(c.keys.copy_name, s.keys.copy_name, "{name}");
            assert_eq!(c.keys.surfaces, s.keys.surfaces, "{name}: key tables");
        }
    }
}
