// 設定の読み込みと、フォーマット→プレビュー方法の解決。
// 設定が無い・壊れている場合もデフォルトで動く（堅牢性優先）。

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
}

/// Git integration settings (`[git]`). How external git tools are launched and how diffs are shown.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct GitConfig {
    /// External git tool launched with `O` (command + args, whitespace-separated). Default "lazygit".
    pub tool: String,
    /// Initial diff layout. "unified" (vertical, default) | "split" (side by side) | "auto" (by width).
    /// (Aliases: vertical = unified / horizontal, side-by-side = split.) At runtime, `s` while viewing a diff
    /// cycles vertical -> horizontal -> Auto. Applies to both the GitDiff preview and commit/working-tree details.
    pub diff: String,
    /// [Unimplemented/reserved] For the base-branch-pinned graph (post-release; docs/GRAPH-BASE-SPEC.md).
    /// Currently referenced by nothing (parsed only). Wired up when implemented.
    pub main_branch: String,
}

impl Default for GitConfig {
    fn default() -> Self {
        Self {
            tool: "lazygit".into(),
            diff: "unified".into(), // 既定は縦。横/Auto は設定 or 実行時 `s`。
            main_branch: "".into(),
        }
    }
}

/// External editor settings (FR: delegate editing with `e`). **Configurable per extension.**
/// Priority: per-extension `[editor.ext]` -> `editor.command` (global default) -> `$VISUAL` -> `$EDITOR` -> `vim`.
/// Values are command + args (whitespace-separated). If `{path}` is present the file path is substituted there, otherwise appended at the end.
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

    /// Resolves the argv of the editor for editing `path`.
    /// Priority: per-extension -> command -> `$VISUAL` -> `$EDITOR` -> `vim`. `{path}` substitution or appended at the end.
    pub fn resolve(&self, path: &Path) -> Vec<String> {
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
        build_argv(&tmpl, path)
    }
}

/// Reads an environment variable; None if whitespace-only / unset.
fn env_nonempty(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Splits a command template (whitespace-separated) into argv and inserts the file path.
/// If a token contains `{path}` it is substituted, otherwise appended at the end. An empty template is `vim <path>`.
fn build_argv(tmpl: &str, path: &Path) -> Vec<String> {
    let p = path.to_string_lossy().to_string();
    let mut argv: Vec<String> = tmpl.split_whitespace().map(|s| s.to_string()).collect();
    if argv.is_empty() {
        return vec!["vim".to_string(), p];
    }
    let mut substituted = false;
    for a in argv.iter_mut() {
        if a.contains("{path}") {
            *a = a.replace("{path}", &p);
            substituted = true;
        }
    }
    if !substituted {
        argv.push(p);
    }
    argv
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
    pub copy_name: String, // ファイル名
    pub copy_full: String,     // フルパス
    pub copy_relative: String, // 相対パス
    pub copy_parent: String,   // 親ディレクトリ
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
    pub tabbar: String,     // "always" | "auto" | "hidden"
    pub icons: bool, // ツリー行頭の Nerd Font アイコン (要 Nerd Font。無ければ false でプレーン記号)
    pub wrap: bool, // テキストプレビューの折返し。true=折返して全文表示 / false=非折返し+横スクロール
    pub line_numbers: bool, // コード/テキストプレビューに行番号ガターを出すか (既定 false)
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
    pub path_style: String, // タイトルのパス表示既定。"relative" | "home" | "full" (p キーで巡回)
    pub keys: String,       // プレビューのページ送りキー流儀。"vim" | "less" (既定 vim)
    pub lang: String, // 表示言語。"auto"(既定:OS言語に追従) | "en" | "jp"。ヘルプ/ヒント/メッセージ等に適用。
    /// Layout of the status chrome. "split" (default) = context (mode/path/zoom) on top, key hints at the bottom /
    /// "bottom" = everything on one bottom line / "top" = everything on one top line.
    pub statusbar: String,
    pub theme: ThemeConfig, // 配色 (現状はコード背景色のみ)
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
            bg: "none".into(),         // 既定は端末既定背景 (塗らない)
            code_bg: "#2b303b".into(), // = DEFAULT_CODE_BG (rgb 43,48,59)
            code_label_align: "right".into(),
            code_label_bg: "auto".into(),
            code_theme: "TwoDark".into(), // = Zed の One Dark
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
    pub builtin: Option<String>, // "markdown" | "mermaid" | "image" | "svg" | "video" | "pdf" | "code" | "text"
    pub command: Option<String>, // テンプレ: {path} {out}
    pub render_as: Option<String>, // command 出力の扱い: "image" | "text"
    pub detached: bool,          // 別プロセスで開きTUIをブロックしない（動画等）
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            show_hidden: false,
            tabbar: "auto".into(),
            icons: true,
            wrap: true,
            line_numbers: false,
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
        }
    }
}

impl Default for PreviewConfig {
    fn default() -> Self {
        // デフォルトの委譲ルール。外部依存はあくまで任意。
        Self {
            rules: vec![
                Rule {
                    glob: Some("*.md".into()),
                    builtin: Some("markdown".into()),
                    ..Rule::empty()
                },
                Rule {
                    glob: Some("*.{mmd,mermaid}".into()),
                    builtin: Some("mermaid".into()), // 単体 mermaid ファイルは mermaid-text で描画
                    ..Rule::empty()
                },
                Rule {
                    // SVG はラスタ画像より先に拾う(infer は svg も image/* と判定するため順序が重要)。
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
                    glob: Some("*.{rs,ts,tsx,js,py,go,toml,json,sh,yaml,yml,c,cpp,h}".into()),
                    builtin: Some("code".into()),
                    ..Rule::empty()
                },
                Rule {
                    // 動画はサムネイル(代表フレーム1枚)を内蔵表示する。端末内再生はしない。
                    // 再生したい場合はユーザが command="mpv {path}" 等のルールに差し替える(委譲)。
                    mime: Some("video/*".into()),
                    builtin: Some("video".into()),
                    ..Rule::empty()
                },
                Rule {
                    // PDF は1ページ目をラスタライズして内蔵表示する(pdftocairo/pdftoppm/qlmanage/sips)。
                    // ツールが無ければ安全にヒント表示へフォールバックする(原則#3)。
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
        // 設定ファイルが無いのは正常(既定で動く)。読めるが壊れている場合のみエラーを報告。
        let Ok(text) = std::fs::read_to_string(&path) else {
            return (Config::default(), None);
        };
        match toml::from_str::<Config>(&text) {
            Ok(cfg) => (cfg, None),
            Err(e) => {
                // toml のエラーは複数行になりうるので1行に圧縮。
                let msg = e
                    .to_string()
                    .lines()
                    .next()
                    .unwrap_or("parse error")
                    .to_string();
                // 設定が壊れている=言語も読めないので、既定の英語で通知する。
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
                return PreviewKind::from_rule(rule, path);
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
        // 大文字拡張子(README.MD / *.RS 等)も取りこぼさないよう case-insensitive で照合。
        // 小文字パターンは元々小文字名にマッチするので既存挙動は不変。
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
    fn markdown_rule_still_wins() {
        let p = tmp("README.md", b"# title\n");
        let kind = Config::default().resolve_preview(&p);
        assert!(matches!(kind, PreviewKind::Markdown(_)), "got {kind:?}");
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn glob_matches_uppercase_extension() {
        // 大文字拡張子(README.MD / *.RS 相当)も case-insensitive で規則にヒットする。
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
        // {path} があればその位置へ置換。
        assert_eq!(
            build_argv("code -g {path}:1", path),
            vec!["code", "-g", "/tmp/a.rs:1"]
        );
        // 無ければ末尾に追加。
        assert_eq!(build_argv("nvim", path), vec!["nvim", "/tmp/a.rs"]);
        assert_eq!(build_argv("code -w", path), vec!["code", "-w", "/tmp/a.rs"]);
        // 空テンプレは vim フォールバック。
        assert_eq!(build_argv("   ", path), vec!["vim", "/tmp/a.rs"]);
    }

    #[test]
    fn editor_configured_prefers_ext_then_command() {
        let mut e = EditorConfig {
            command: "nvim".into(),
            ext: HashMap::new(),
        };
        e.ext.insert("md".into(), "code -w".into());
        e.ext.insert("blank".into(), "   ".into()); // 空白のみは無効
                                                    // 拡張子別が最優先。
        assert_eq!(e.configured("md").as_deref(), Some("code -w"));
        // 拡張子別が無ければ command(全体既定)。
        assert_eq!(e.configured("rs").as_deref(), Some("nvim"));
        // 空白のみの拡張子別は無効 → command にフォールバック。
        assert_eq!(e.configured("blank").as_deref(), Some("nvim"));
        // command も空なら None(=呼び出し側で env→vim)。
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
        // 大文字拡張子でも小文字照合でヒット、末尾にパス追加。
        assert_eq!(
            e.resolve(Path::new("/x/NOTE.MD")),
            vec!["code", "-w", "/x/NOTE.MD"]
        );
        // 未登録拡張子は command。
        assert_eq!(e.resolve(Path::new("/x/a.rs")), vec!["nvim", "/x/a.rs"]);
    }

    #[test]
    fn unknown_text_falls_back_to_text() {
        // 拡張子なし・どのルールにも当たらないテキスト (LICENSE/Makefile 相当)。
        let p = tmp("LICENSE", b"MIT License\n\nPermission...\n");
        let kind = Config::default().resolve_preview(&p);
        assert!(matches!(kind, PreviewKind::Text(_)), "got {kind:?}");
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn theme_code_bg_parses_hex_none_and_invalid() {
        // 既定 (空設定) は既定色。
        assert_eq!(ThemeConfig::default().code_bg(), Some(DEFAULT_CODE_BG));
        // hex 指定。
        let t = ThemeConfig {
            code_bg: "#101820".into(),
            ..Default::default()
        };
        assert_eq!(t.code_bg(), Some(Color::Rgb(16, 24, 32)));
        // "none"/空白/大文字混じり → 背景なし。
        for v in ["none", "  NONE  ", ""] {
            let t = ThemeConfig {
                code_bg: v.into(),
                ..Default::default()
            };
            assert_eq!(t.code_bg(), None, "{v:?} は None になるべき");
        }
        // 不正値はクラッシュせず既定色にフォールバック。
        let t = ThemeConfig {
            code_bg: "definitely-not-a-color".into(),
            ..Default::default()
        };
        assert_eq!(t.code_bg(), Some(DEFAULT_CODE_BG));
    }

    #[test]
    fn theme_bg_parses_and_defaults_to_none() {
        // 全体背景: 既定は none (端末既定のまま)。
        assert_eq!(ThemeConfig::default().bg(), None);
        // 色指定は反映。
        let t = ThemeConfig {
            bg: "#102030".into(),
            ..Default::default()
        };
        assert_eq!(t.bg(), Some(Color::Rgb(16, 32, 48)));
        // 不正値は None (端末既定にフォールバック=塗らない)。
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
        // 既定: 右寄せ・auto(=code_bg を明るく)。
        let d = ThemeConfig::default();
        assert!(d.code_label_right());
        assert_eq!(d.code_label_bg(), Some(lighten(DEFAULT_CODE_BG)));
        // 左寄せ指定。
        let t = ThemeConfig {
            code_label_align: "left".into(),
            ..Default::default()
        };
        assert!(!t.code_label_right());
        // 大小無視で left。
        let t = ThemeConfig {
            code_label_align: "LEFT".into(),
            ..Default::default()
        };
        assert!(!t.code_label_right());
        // バッジ背景: 任意色 / none。
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
        // code_bg=none + auto → バッジも背景なし。
        let t = ThemeConfig {
            code_bg: "none".into(),
            code_label_bg: "auto".into(),
            ..Default::default()
        };
        assert_eq!(t.code_label_bg(), None);
    }

    #[test]
    fn broken_config_reports_error_and_falls_back() {
        // 壊れた TOML はパースで Err になり、エラー文字列を伴って既定へフォールバックする。
        let broken = "[keys]\ncopy_prefix = \n"; // 値が無い=構文エラー
        let parsed = toml::from_str::<Config>(broken);
        assert!(parsed.is_err(), "壊れた TOML は Err になる");
        // load_reporting の整形(1行化)を模す: Err なら Some(msg)。
        let msg = parsed.err().map(|e| e.to_string());
        assert!(msg.is_some());
    }

    #[test]
    fn copy_keys_default_and_custom() {
        // 旧 [keys] copy_* スカラ (後方互換 alias) が TOML から正しく読める。
        let d = KeysConfig::default();
        assert_eq!(d.copy_prefix, "c");
        assert_eq!(d.copy_name, "n");
        assert_eq!(d.copy_full, "p");
        // vim 風: prefix を y に。
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
        // 新形式 [keys.<surface>] のサブテーブルを surfaces に拾い、旧 copy_* スカラと同居できる。
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
        // 既定のままの copy_* は新既定 (n/r/f/p) を clobber しない。
        let def = KeysConfig::default();
        let km = KeyMap::from_config(KeyScheme::Vim, &def.to_keymap_config());
        // f=Full / p=Parent (新既定) が保たれる。
        assert_eq!(
            km.resolve(Surface::Tree, Some(LeaderId::Copy), KeyPress::ch('f')),
            Resolution::Action(Action::CopyPath(CopyKind::Full))
        );
        assert_eq!(
            km.resolve(Surface::Tree, Some(LeaderId::Copy), KeyPress::ch('p')),
            Resolution::Action(Action::CopyPath(CopyKind::Parent))
        );

        // ユーザが copy_full を P に変えた → `y P` が Copy(Full) として追加される (新既定は残る)。
        let custom = KeysConfig {
            copy_full: "P".into(),
            ..KeysConfig::default()
        };
        let km2 = KeyMap::from_config(KeyScheme::Vim, &custom.to_keymap_config());
        assert_eq!(
            km2.resolve(Surface::Tree, Some(LeaderId::Copy), KeyPress::ch('P')),
            Resolution::Action(Action::CopyPath(CopyKind::Full))
        );
        // 新既定の f=Full も残る (additive)。
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
        // ユニークなキー名で並列テストの干渉を避ける。set/unset/空白を網羅。
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
        // HOME はこのテストでは変更しない(他テストと並列でも壊さない)。
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
        // 1) 拡張子別が最優先。
        let mut ec = EditorConfig {
            command: "code -w".into(),
            ext: HashMap::new(),
        };
        ec.ext.insert("rs".into(), "nvim {path}".into());
        let argv = ec.resolve(Path::new("/x/main.rs"));
        assert_eq!(
            argv,
            vec!["nvim".to_string(), "/x/main.rs".to_string()],
            "{{path}} 置換"
        );
        // 2) 拡張子未指定なら command(末尾追記)。
        let argv = ec.resolve(Path::new("/x/readme.md"));
        assert_eq!(
            argv,
            vec![
                "code".to_string(),
                "-w".to_string(),
                "/x/readme.md".to_string()
            ]
        );

        // 3) command も ext も空なら $VISUAL → $EDITOR → vim の順。
        //    VISUAL/EDITOR を読むのはこの resolve のみ。1テスト内で完結させ、必ず復元する。
        let empty = EditorConfig {
            command: String::new(),
            ext: HashMap::new(),
        };
        let save_v = std::env::var_os("VISUAL");
        let save_e = std::env::var_os("EDITOR");
        std::env::set_var("VISUAL", "myvisual");
        std::env::set_var("EDITOR", "myeditor");
        assert_eq!(
            empty.resolve(Path::new("/x/f")),
            vec!["myvisual".to_string(), "/x/f".to_string()],
            "VISUAL 優先"
        );
        std::env::remove_var("VISUAL");
        assert_eq!(
            empty.resolve(Path::new("/x/f")),
            vec!["myeditor".to_string(), "/x/f".to_string()],
            "次に EDITOR"
        );
        std::env::remove_var("EDITOR");
        assert_eq!(
            empty.resolve(Path::new("/x/f")),
            vec!["vim".to_string(), "/x/f".to_string()],
            "最後は vim"
        );
        // 復元。
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
        // 実環境(~/.config/konoma/config.toml の有無/内容)に依存するため、不変条件のみ検証する:
        // パースエラー時の文字列は決まった接頭辞を持つ。クラッシュしないこと。
        let (_cfg, err) = Config::load_reporting();
        if let Some(e) = err {
            assert!(
                e.starts_with("config error (using defaults): "),
                "エラー文言の接頭辞が規約どおり: {e}"
            );
        }
    }
}
