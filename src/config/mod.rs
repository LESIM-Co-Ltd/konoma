// Config loading, and resolving format → preview method.
// Works with defaults even when the config is absent/broken (robustness first).

use std::collections::HashMap;
use std::path::Path;
use std::str::FromStr;

use ratatui::style::Color;
use serde::Deserialize;

use crate::preview::PreviewKind;

/// Default code background color for Markdown (dark slate). Used when `ui.theme.code_bg` is unspecified.
pub const DEFAULT_CODE_BG: Color = Color::Rgb(43, 48, 59);

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Config {
    pub ui: UiConfig,
    pub preview: PreviewConfig,
    pub keys: KeysConfig,
    pub editor: EditorConfig,
    pub git: GitConfig,
    pub external: ExternalConfig,
}

/// Single on/off switch per external process konoma can launch (`[external]`). Everything defaults to
/// `true`, so an absent section changes nothing. Unlike `[[preview.rules]]` (where writing one user rule
/// **replaces** the whole builtin list — see `PreviewConfig::default`), these flags are independent
/// toggles layered on top of whatever preview rules are in effect: they gate the specific mechanism, not
/// the rule table. `[ui] lang` already has its own explicit/auto switch for OS-language lookup
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
    /// The external PDF fallback tools (pdftocairo/pdftoppm/qlmanage/sips), used only when the
    /// primary renderer (`hayro`, pure Rust — always active regardless of this flag) fails to render
    /// a given PDF (encrypted/corrupt/unsupported). `false` never launches those tools; PDF preview
    /// itself keeps working via `hayro` (see `preview::pdf`).
    pub pdf: bool,
    /// Video thumbnail extraction (ffmpegthumbnailer/ffmpeg).
    pub video: bool,
    /// Fetching `http(s)://` images referenced from Markdown (`curl`) — the only outbound network call konoma makes.
    pub remote_images: bool,
    /// Opening URLs/files with the OS handler (`open` on macOS, `xdg-open` elsewhere) — Markdown links, `P`, etc.
    pub open_links: bool,
    /// Running a `[[preview.rules]] command = "..."` delegation. `false` makes that rule shape behave
    /// like no rule matched (falls through to `[can not preview]`).
    pub preview_commands: bool,
}

impl Default for ExternalConfig {
    fn default() -> Self {
        Self {
            git: true,
            git_tool: true,
            pdf: true,
            video: true,
            remote_images: true,
            open_links: true,
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

impl Default for GitConfig {
    fn default() -> Self {
        Self {
            tool: "lazygit".into(),
            diff: "unified".into(), // default is vertical. horizontal/Auto via config or runtime `s`.
            main_branch: "".into(),
            worktree_dir: "../".into(),
        }
    }
}

/// External editor settings (FR: delegate editing with `e`). **Configurable per extension.**
/// Priority: per-extension `[editor.ext]` -> `editor.command` (global default) -> `$VISUAL` -> `$EDITOR` -> `vim`.
/// Values are command + args (whitespace-separated). `{path}` is substituted with the file path (otherwise
/// appended at the end); `{line}` with the current preview line. Without a `{line}` token, common editors
/// still open at the line automatically (vim `+N`, VS Code `-g path:N`, Sublime/Helix `path:N`).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct EditorConfig {
    /// Default editor when there is no per-extension match. If empty, $VISUAL -> $EDITOR -> vim.
    pub command: String,
    /// Map of extension (no dot) -> editor command. Example: `rs = "nvim"`, `md = "code -w"`.
    pub ext: HashMap<String, String>,
}

impl EditorConfig {
    /// The configured editor command (the pure part, excluding env / the default vi). Priority: per-extension -> command.
    /// None if both are unset / whitespace-only. Extensions are matched in lowercase.
    fn configured(&self, ext: &str) -> Option<String> {
        let by_ext = self
            .ext
            .get(ext)
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());
        by_ext.or_else(|| {
            let d = self.command.trim();
            (!d.is_empty()).then(|| d.to_string())
        })
    }

    /// Resolves the argv of the editor for editing `path`, optionally jumping to `line` (1-based).
    /// Priority: per-extension -> command -> `$VISUAL` -> `$EDITOR` -> `vim`. `{path}` substitution or appended at the end.
    /// `line` matches the on-screen preview position; see `build_argv`/`apply_editor_line` for how it reaches the editor.
    pub fn resolve(&self, path: &Path, line: Option<usize>) -> Vec<String> {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();
        let tmpl = self
            .configured(&ext)
            .or_else(|| env_nonempty("VISUAL"))
            .or_else(|| env_nonempty("EDITOR"))
            .unwrap_or_else(|| "vim".to_string());
        build_argv(&tmpl, path, line)
    }
}

/// Reads an environment variable; None if whitespace-only / unset.
fn env_nonempty(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Splits a command template (whitespace-separated) into argv and inserts the file path (and,
/// optionally, a line number). A token containing `{path}` is substituted, otherwise the path is
/// appended at the end. `{line}` is substituted with `line` (or 1 when unknown). When `line` is
/// present but the template has no `{line}` token, `apply_editor_line` injects the right line flag
/// for common editors. An empty template is `vim <path>`.
fn build_argv(tmpl: &str, path: &Path, line: Option<usize>) -> Vec<String> {
    let p = path.to_string_lossy().to_string();
    let mut argv: Vec<String> = tmpl.split_whitespace().map(|s| s.to_string()).collect();
    if argv.is_empty() {
        argv.push("vim".to_string());
    }
    let mut path_sub = false;
    let mut line_sub = false;
    for a in argv.iter_mut() {
        if a.contains("{path}") {
            *a = a.replace("{path}", &p);
            path_sub = true;
        }
        if a.contains("{line}") {
            *a = a.replace("{line}", &line.unwrap_or(1).to_string());
            line_sub = true;
        }
    }
    if !path_sub {
        argv.push(p);
    }
    if let Some(l) = line {
        if !line_sub {
            apply_editor_line(&mut argv, l);
        }
    }
    argv
}

/// When the editor template has no explicit `{line}` token, inject a line-jump argument for common
/// editors so the file opens at `line` (1-based). The vi/vim family also gets `+normal! zt` to scroll
/// that line to the top of the window (matching konoma's top-of-view). Also recognizes terminal editors
/// that accept `+N`, VS Code (`-g <path>:<line>`), and Sublime/Helix/Zed (`<path>:<line>`). Unknown
/// editors are left unchanged (they simply open at the top — configure `{line}` for them explicitly).
fn apply_editor_line(argv: &mut Vec<String>, line: usize) {
    let Some(prog) = argv.first() else {
        return;
    };
    let name = Path::new(prog)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match name.as_str() {
        // vi/vim family: `+N` positions the cursor, and `+normal! zt` scrolls that line to the top of
        // the window so it matches konoma's top-of-view. Without `zt`, vim leaves the window at the file
        // top when N fits on the first screen (cursor buried mid-screen — 2026-07-09 user report).
        "vim" | "nvim" | "vi" | "view" | "gvim" | "mvim" | "vimx" | "neovim" => {
            argv.insert(1, format!("+{line}"));
            argv.insert(2, "+normal! zt".to_string());
        }
        // Other terminal editors that accept `+N` (no reliable scroll-to-top flag; open at the line).
        "nano" | "pico" | "emacs" | "emacsclient" | "gedit" | "kak" | "kakoune" | "joe" | "jed"
        | "ne" | "mcedit" | "micro" => {
            argv.insert(1, format!("+{line}"));
        }
        // VS Code family: `-g <path>:<line>`.
        "code" | "code-insiders" | "codium" | "vscodium" | "cursor" | "windsurf" => {
            if let Some(last) = argv.last_mut() {
                *last = format!("{last}:{line}");
            }
            argv.insert(1, "-g".to_string());
        }
        // Sublime / Helix / Zed: `<path>:<line>`.
        "subl" | "sublime_text" | "hx" | "helix" | "zed" => {
            if let Some(last) = argv.last_mut() {
                *last = format!("{last}:{line}");
            }
        }
        _ => {}
    }
}

/// Keybinding settings (`[keys]`).
///
/// Two systems coexist in the same `[keys]` table:
/// - **New form (Run2)**: subtables under `[keys.<surface>]` (surface name -> (chord string -> action string)).
///   Interpreted by the `crate::keymap` layer. serde flatten collects only the subtables directly under `[keys]` into `surfaces`.
/// - **Old form (backward-compat alias)**: the `copy_prefix`/`copy_*` scalars for path copy (FR-6). Kept for now.
///   The named fields consume them first, so they do not end up on the flatten side (`surfaces`).
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct KeysConfig {
    /// Copy prefix (default "c"; "y" for vim-style yank). [backward-compat alias]
    pub copy_prefix: String,
    /// Suffix key for each copy target. [backward-compat alias]
    pub copy_name: String, // file name
    pub copy_full: String,     // full path
    pub copy_relative: String, // relative path
    pub copy_parent: String,   // parent directory
    /// The raw new-form `[keys.<surface>]` table (surface name -> (chord string -> action string)).
    /// Example: `surfaces["tree"]["space d"] = "file_delete"`. Interpreted by `crate::keymap::KeyMap::from_config`.
    #[serde(flatten)]
    pub surfaces: HashMap<String, HashMap<String, String>>,
}

impl Default for KeysConfig {
    fn default() -> Self {
        Self {
            copy_prefix: "c".into(),
            copy_name: "n".into(),
            copy_full: "p".into(),
            copy_relative: "r".into(),
            copy_parent: "d".into(),
            surfaces: HashMap::new(),
        }
    }
}

impl KeysConfig {
    /// Builds the settings interpreted by the keymap layer (`crate::keymap`).
    /// Passes the new-form `[keys.<surface>]` (`surfaces`) through as-is, while mapping the old `copy_*` aliases onto the `y` leader.
    ///
    /// The old aliases add `"y <suffix>"` to the Copy leader **only for suffixes the user changed from the default**
    /// (when left at the default it respects the new defaults n/r/f/p and does not clobber them). Since leaders are surface-independent,
    /// they are placed in the `global` table for convenience (the surface name does not matter because the keymap routes prefix `y` -> Copy). Existing chords are not broken.
    /// The old `copy_prefix` is not mapped because the new scheme fixes it to `y` (open_risk).
    pub fn to_keymap_config(&self) -> crate::keymap::KeysFileConfig {
        let mut surfaces = self.surfaces.clone();
        let defaults = KeysConfig::default();
        let aliases: [(&str, &str, &str); 4] = [
            (&self.copy_name, &defaults.copy_name, "copy_name"),
            (
                &self.copy_relative,
                &defaults.copy_relative,
                "copy_relative",
            ),
            (&self.copy_full, &defaults.copy_full, "copy_full"),
            (&self.copy_parent, &defaults.copy_parent, "copy_parent"),
        ];
        for (cur, def, action) in aliases {
            let cur = cur.trim();
            if cur.is_empty() || cur == def {
                continue;
            }
            if let Some(c) = cur.chars().next() {
                surfaces
                    .entry("global".to_string())
                    .or_default()
                    .entry(format!("y {c}"))
                    .or_insert_with(|| action.to_string());
            }
        }
        crate::keymap::KeysFileConfig { surfaces }
    }
}

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
    /// What follow mode (`F`) opens when it jumps to a changed file. `"diff"` (default) = the full-screen
    /// git diff of that file (hunk-level before/after — the way hunk/livediff/diffpane present agent
    /// edits; untracked files show as an all-added diff); `"file"` = the normal content preview scrolled
    /// to the first changed hunk. Files with no diff (unchanged / outside a repo) and media
    /// (image/SVG/video/PDF) always fall back to the content preview.
    pub follow_view: String,
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
    /// Render common inline HTML that GitHub shows but tui-markdown strips: `<del>`/`<s>`/`<strike>`
    /// as strikethrough, `<kbd>` as an inline-code keycap, `<sup>`/`<sub>` as Unicode (when the text
    /// maps), and `<br>` as a hard line break. Default true. `<mark>`/`<ins>` have no faithful
    /// terminal form and keep only their text either way. false leaves all these tags to be stripped.
    pub md_inline_html: bool,
    /// How `<details>` blocks start out. `"auto"` (default) honors the `open` attribute like GitHub
    /// (`<details>` collapsed, `<details open>` expanded); `"open"` always starts expanded; `"closed"`
    /// always collapsed. Either way `Tab` focuses the `<summary>` and `Space`/`Enter` toggle it.
    pub md_details: String,
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

    /// The math glyph color, sanitized against the **same parser usvg uses** (`svgtypes::Color`), so a
    /// value that passes here is one usvg will render — never one it silently falls back to *black* for
    /// (which is invisible on a dark terminal = the very "blank equation" bug). A `#hex`, `rgb(…)`, or a
    /// real CSS color name (`black`, `white`, …) passes; a typo (`wihte`), `none`, `currentColor`, or a
    /// fully-transparent color falls back to the light-gray default. Returning the *original* string is
    /// safe because usvg re-parses it identically.
    pub fn math_color(&self) -> &str {
        use std::str::FromStr;
        let c = self.math_color.trim();
        let visible = svgtypes::Color::from_str(c)
            .map(|col| col.alpha != 0)
            .unwrap_or(false);
        if visible {
            c
        } else {
            "#d0d0d0"
        }
    }
}

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

/// Brightens a background color slightly (to distinguish the language badge from body code). Non-Rgb is left as-is.
pub fn lighten(c: Color) -> Color {
    match c {
        Color::Rgb(r, g, b) => Color::Rgb(
            r.saturating_add(27),
            g.saturating_add(30),
            b.saturating_add(40),
        ),
        other => other,
    }
}

#[derive(Debug, Clone, Deserialize)]
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
    pub builtin: Option<String>, // "markdown" | "mermaid" | "image" | "svg" | "video" | "pdf" | "code" | "archive" | "text"
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
            csv_rainbow: true,
            follow_view: "diff".into(),
            md_task_states: vec![" ".into(), "x".into()],
            md_autolink: true,
            md_alerts: true,
            md_emoji: true,
            md_frontmatter: true,
            md_footnotes: true,
            md_inline_html: true,
            md_details: "auto".into(),
            busy_indicator: true,
            mermaid: "image".into(),
            math: "image".into(),
            math_color: "#d0d0d0".into(),
            mermaid_theme: "dark".into(),
            mermaid_rows: 24,
            restore_tabs: true,
            restore_single_tab: true,
        }
    }
}

impl Default for PreviewConfig {
    fn default() -> Self {
        // Default delegation rules. External dependencies are always optional.
        Self {
            rules: vec![
                Rule {
                    glob: Some("*.md".into()),
                    builtin: Some("markdown".into()),
                    ..Rule::empty()
                },
                Rule {
                    glob: Some("*.{mmd,mermaid}".into()),
                    builtin: Some("mermaid".into()), // a standalone mermaid file is rendered via mermaid-text
                    ..Rule::empty()
                },
                Rule {
                    // SVG is picked up before raster images (infer also classifies svg as image/*, so order matters).
                    glob: Some("*.svg".into()),
                    builtin: Some("svg".into()),
                    ..Rule::empty()
                },
                Rule {
                    mime: Some("image/*".into()),
                    builtin: Some("image".into()),
                    ..Rule::empty()
                },
                Rule {
                    // CSV/TSV are shown as an aligned table (column rainbow + cell cursor).
                    glob: Some("*.csv".into()),
                    builtin: Some("csv".into()),
                    ..Rule::empty()
                },
                Rule {
                    glob: Some("*.tsv".into()),
                    builtin: Some("tsv".into()),
                    ..Rule::empty()
                },
                Rule {
                    // Archive contents listing (name/size/modified date). Doesn't extract (doesn't read the contents — principle #3).
                    glob: Some("*.{zip,tar,tgz}".into()),
                    builtin: Some("archive".into()),
                    ..Rule::empty()
                },
                Rule {
                    // globset's `*.{...}` alternation can't catch a compound extension joined by
                    // plain dots (.tar.gz), so it's a separate rule (registering 2 rules side by
                    // side, as with csv/tsv, mirrors the existing svg/image shape).
                    glob: Some("*.tar.gz".into()),
                    builtin: Some("archive".into()),
                    ..Rule::empty()
                },
                Rule {
                    glob: Some("*.{rs,ts,tsx,js,py,go,toml,json,sh,yaml,yml,c,cpp,h}".into()),
                    builtin: Some("code".into()),
                    ..Rule::empty()
                },
                Rule {
                    // Video shows a built-in thumbnail (one representative frame). It doesn't play
                    // inside the terminal.
                    // If playback is wanted, the user swaps in a rule like command="mpv {path}" (delegation).
                    mime: Some("video/*".into()),
                    builtin: Some("video".into()),
                    ..Rule::empty()
                },
                Rule {
                    // PDF is shown built-in by rasterizing the page in pure Rust (hayro) — no
                    // external tool needed. Only when hayro can't render it (encrypted/corrupted,
                    // etc.) does it degrade to pdftocairo/pdftoppm/qlmanage/sips, and if those are
                    // absent too it safely falls back to a hint display (principle #3).
                    glob: Some("*.pdf".into()),
                    builtin: Some("pdf".into()),
                    ..Rule::empty()
                },
            ],
        }
    }
}

impl Rule {
    fn empty() -> Self {
        Self {
            glob: None,
            mime: None,
            builtin: None,
            command: None,
            render_as: None,
            detached: false,
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
            Ok(cfg) => (cfg, None),
            Err(e) => {
                // A toml error can span multiple lines, so compress it to one line.
                let msg = e
                    .to_string()
                    .lines()
                    .next()
                    .unwrap_or("parse error")
                    .to_string();
                // The config is broken = even the language setting can't be read, so notify in the
                // default (English).
                (
                    Config::default(),
                    Some(format!("config error (using defaults): {msg}")),
                )
            }
        }
    }

    /// Determines the PreviewKind for the given path from the first matching rule.
    /// When no rule matches, it judges text vs. binary and falls back to
    /// the built-in text display for text, or safely to CanNotPreview for binary
    /// (design principle 3 "unsupported is handled safely"; picks up extensionless README/LICENSE/Makefile, etc.).
    pub fn resolve_preview(&self, path: &Path) -> PreviewKind {
        for rule in &self.preview.rules {
            if rule_matches(rule, path) {
                let kind = PreviewKind::from_rule(rule, path);
                // `[external] preview_commands = false`: a matching `command = "..."` rule behaves as
                // if it hadn't matched at all (falls through to the safe CanNotPreview below), rather
                // than launching the external tool. Builtin renderers are unaffected.
                if matches!(kind, PreviewKind::Command { .. }) && !self.external.preview_commands {
                    return PreviewKind::can_not_preview(path);
                }
                return kind;
            }
        }
        if crate::preview::text::is_probably_text(path) {
            PreviewKind::Text(path.to_path_buf())
        } else {
            PreviewKind::can_not_preview(path)
        }
    }
}

fn rule_matches(rule: &Rule, path: &Path) -> bool {
    if let Some(glob) = &rule.glob {
        // Match case-insensitively so uppercase extensions (README.MD / *.RS etc.) aren't missed.
        // A lowercase pattern already matches lowercase names, so existing behavior is unchanged.
        if let Ok(set) = globset::GlobBuilder::new(glob)
            .case_insensitive(true)
            .build()
            .map(|g| g.compile_matcher())
        {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if set.is_match(name) {
                return true;
            }
        }
    }
    if let Some(mime_pat) = &rule.mime {
        if let Some(kind) = infer::get_from_path(path).ok().flatten() {
            let mime = kind.mime_type();
            if mime_glob_match(mime_pat, mime) {
                return true;
            }
        }
    }
    false
}

/// Simple mime glob matching like "image/*".
fn mime_glob_match(pattern: &str, mime: &str) -> bool {
    if let Some(prefix) = pattern.strip_suffix("/*") {
        mime.starts_with(prefix)
    } else {
        pattern == mime
    }
}

fn dirs_config_path() -> Option<std::path::PathBuf> {
    let home = std::env::var_os("HOME")?;
    let mut p = std::path::PathBuf::from(home);
    p.push(".config/konoma/config.toml");
    Some(p)
}

#[cfg(test)]
mod parity_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn tmp(name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("konoma_cfg_test_{name}"));
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

    #[test]
    fn markdown_rule_still_wins() {
        let p = tmp("README.md", b"# title\n");
        let kind = Config::default().resolve_preview(&p);
        assert!(matches!(kind, PreviewKind::Markdown(_)), "got {kind:?}");
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn glob_matches_uppercase_extension() {
        // Uppercase extensions (README.MD / equivalent to *.RS) also hit the rule case-insensitively.
        let p = tmp("README.MD", b"# title\n");
        let kind = Config::default().resolve_preview(&p);
        assert!(matches!(kind, PreviewKind::Markdown(_)), "got {kind:?}");
        std::fs::remove_file(&p).ok();

        let p = tmp("MAIN.RS", b"fn main() {}\n");
        let kind = Config::default().resolve_preview(&p);
        assert!(matches!(kind, PreviewKind::Code(_)), "got {kind:?}");
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
        // The new-form [keys.<surface>] subtables are picked up into surfaces and can coexist with
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

    #[test]
    fn to_keymap_config_maps_changed_copy_alias_only() {
        use crate::app::CopyKind;
        use crate::keymap::{Action, KeyMap, KeyPress, KeyScheme, LeaderId, Resolution, Surface};
        // A copy_* left at its default doesn't clobber the new defaults (n/r/f/p).
        let def = KeysConfig::default();
        let km = KeyMap::from_config(KeyScheme::Vim, &def.to_keymap_config());
        // f=Full / p=Parent (the new defaults) are preserved.
        assert_eq!(
            km.resolve(Surface::Tree, Some(LeaderId::Copy), KeyPress::ch('f')),
            Resolution::Action(Action::CopyPath(CopyKind::Full))
        );
        assert_eq!(
            km.resolve(Surface::Tree, Some(LeaderId::Copy), KeyPress::ch('p')),
            Resolution::Action(Action::CopyPath(CopyKind::Parent))
        );

        // The user changed copy_full to P → `y P` is added as Copy(Full) (the new default remains).
        let custom = KeysConfig {
            copy_full: "P".into(),
            ..KeysConfig::default()
        };
        let km2 = KeyMap::from_config(KeyScheme::Vim, &custom.to_keymap_config());
        assert_eq!(
            km2.resolve(Surface::Tree, Some(LeaderId::Copy), KeyPress::ch('P')),
            Resolution::Action(Action::CopyPath(CopyKind::Full))
        );
        // The new default f=Full also remains (additive).
        assert_eq!(
            km2.resolve(Surface::Tree, Some(LeaderId::Copy), KeyPress::ch('f')),
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

    #[test]
    fn editor_config_resolve_priority_ext_command_env_default() {
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
        //    Only this resolve reads VISUAL/EDITOR. Keep it self-contained within one test and
        //    always restore them.
        let empty = EditorConfig {
            command: String::new(),
            ext: HashMap::new(),
        };
        let save_v = std::env::var_os("VISUAL");
        let save_e = std::env::var_os("EDITOR");
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
        // Restore.
        match save_v {
            Some(v) => std::env::set_var("VISUAL", v),
            None => std::env::remove_var("VISUAL"),
        }
        match save_e {
            Some(v) => std::env::set_var("EDITOR", v),
            None => std::env::remove_var("EDITOR"),
        }
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
}
