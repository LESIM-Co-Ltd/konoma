//! `SharedConfig`: the slice of `config.toml` every host reads the same way.

use std::path::Path;

use serde::Deserialize;

use crate::editor::EditorConfig;
use crate::keys::KeysConfig;

/// Formats a `toml::from_str::<Config>` parse error into one line for `load_reporting`'s startup
/// flash notification.
///
/// `toml::de::Error`'s own `Display` is multi-line — a `"TOML parse error at line N, column M"`
/// header, an excerpt of the offending line, and the actual reason (e.g.
/// `"invalid value: integer \`100000\`, expected u16"`) — but the flash notification this feeds
/// (`main.rs`'s one-shot `app.flash`, cleared on the very next keypress) can only show one line.
/// The pre-fix code kept just the *first* line (the location), which is the same
/// `"TOML parse error at line N, column M"` for every single kind of error and never states what
/// was actually wrong — reported as effectively silent, since the flash disappears before the user
/// can even open the file to compare against a line number alone.
///
/// This keeps both: the location (still the first line of `Display`) and the reason
/// (`Error::message()`, which — unlike `Display` — is *only* the "what went wrong" part, with no
/// location decoration). `message()` can itself embed a newline for some error kinds (a bad table
/// header's `"invalid table header\nexpected \`.\`, \`]\`"`), so that is collapsed too, to guarantee
/// this whole function's result is always a single line.
///
/// Deliberately does *not* attempt to identify which struct **field** was the cause beyond what
/// `toml`/`serde`'s own error already names — see the accompanying report for why a real per-field
/// (or per-top-level-table) partial-recovery rewrite was intentionally not attempted here.
pub fn format_load_error(e: &toml::de::Error) -> String {
    let location = e
        .to_string()
        .lines()
        .next()
        .unwrap_or("parse error")
        .to_string();
    let reason = e.message().replace('\n', "; ");
    if reason.is_empty() {
        location
    } else {
        format!("{location}: {reason}")
    }
}

/// The `[ui]` settings that moved into `[plugins.<id>]` (`konoma_plugin::config::MOVED_UI_SETTINGS`),
/// typed. This is the one definition of their names, types and defaults: konoma's `UiConfig`
/// starts from [`MovedUi::default`] and the small host reads this struct from `[ui]`, so a config
/// file means the same in both, and a value of the wrong type is an error in both (the whole file
/// is then not read; see [`SharedConfig::load`]).
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct MovedUi {
    /// Whether code is syntax-highlighted (`[plugins.code] syntax_highlight`).
    pub syntax_highlight: bool,
    /// CSV columns in rainbow colors (`[plugins.csv] rainbow`).
    pub csv_rainbow: bool,
    /// Markdown checkbox states (`[plugins.markdown] task_states`).
    pub md_task_states: Vec<String>,
    pub md_autolink: bool,
    pub md_alerts: bool,
    pub md_emoji: bool,
    pub md_frontmatter: bool,
    pub md_footnotes: bool,
    pub md_inline_html: bool,
    pub md_details: String,
    pub md_table_align: String,
    pub md_image_align: String,
    pub mermaid: String,
    pub mermaid_theme: String,
    pub mermaid_curve: String,
    pub mermaid_routing: String,
    pub mermaid_rows: u16,
    pub math: String,
    /// As written; [`MovedUi::math_color`] is the one that is used.
    pub math_color: String,
    /// `[ui] svg_max_px` (`[plugins.svg] max_px`). Not read from the flattened `[ui]` here: the
    /// owner of the `[ui]` table has the key itself and sets this in `moved()`.
    #[serde(skip, default = "default_svg_max_px")]
    pub svg_max_px: u32,
    /// `[ui.theme] code_theme` (handed to the code plugin as `code_theme`). Set by the owner of
    /// the `[ui.theme]` table, like `svg_max_px`.
    #[serde(skip, default = "default_code_theme")]
    pub code_theme: String,
}

/// The longest side an SVG is drawn at, by default (pinned equal to
/// `konoma_plugin::canvas_pictures::DEFAULT_SVG_MAX_PX` by a test there).
pub const DEFAULT_SVG_MAX_PX: u32 = 800;
/// The code highlight theme, by default.
pub const DEFAULT_CODE_THEME: &str = "TwoDark";

fn default_svg_max_px() -> u32 {
    DEFAULT_SVG_MAX_PX
}

fn default_code_theme() -> String {
    DEFAULT_CODE_THEME.to_string()
}

/// The color formulas are painted in when none is set (or the one set would not show).
pub const DEFAULT_MATH_COLOR: &str = "#d0d0d0";

impl Default for MovedUi {
    fn default() -> Self {
        Self {
            syntax_highlight: true,
            csv_rainbow: true,
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
            mermaid: "image".into(),
            mermaid_theme: "dark".into(),
            mermaid_curve: "basis".into(),
            mermaid_routing: "splines".into(),
            mermaid_rows: 24,
            math: "image".into(),
            math_color: DEFAULT_MATH_COLOR.into(),
            svg_max_px: DEFAULT_SVG_MAX_PX,
            code_theme: DEFAULT_CODE_THEME.into(),
        }
    }
}

impl MovedUi {
    /// The formula color to use: the one set when it shows on a dark terminal, otherwise the
    /// default (see [`visible_math_color`]).
    pub fn math_color(&self) -> &str {
        visible_math_color(&self.math_color)
    }
}

/// `value` when usvg parses it as a color that is not fully transparent, otherwise
/// [`DEFAULT_MATH_COLOR`]. RaTeX paints equations pure black, which is invisible on a dark
/// terminal, and an unparsable color is painted black by usvg: a typo must not blank equations.
/// A `#hex`, `rgb(...)` or a CSS color name passes; `none`, `currentColor` and a transparent color
/// do not. The original string is returned because usvg re-parses it identically.
pub fn visible_math_color(value: &str) -> &str {
    use std::str::FromStr;
    let c = value.trim();
    let visible = svgtypes::Color::from_str(c)
        .map(|col| col.alpha != 0)
        .unwrap_or(false);
    if visible {
        c
    } else {
        DEFAULT_MATH_COLOR
    }
}

/// `[ui.theme]`: the colors a host paints with. The strings are as written; the widgets crate
/// (`konoma_widgets::colors`) resolves them to colors, for konoma and the small host alike.
/// konoma's `ThemeConfig` has the same fields and defaults (pinned by a test there).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct SharedTheme {
    /// Background of the whole app; `"none"` keeps the terminal's.
    pub bg: String,
    /// Markdown code background (inline code and blocks); `"none"` removes the band.
    pub code_bg: String,
    /// Alignment of a code block's language label: `"right"` or `"left"`.
    pub code_label_align: String,
    /// Background of the language label: `"auto"` (a lighter `code_bg`), `"none"` or a color.
    pub code_label_bg: String,
    /// Theme of the code highlight (shared by the code plugin and Markdown code blocks).
    pub code_theme: String,
}

/// The default `[ui.theme] code_bg` (`konoma_widgets::colors::DEFAULT_CODE_BG`, pinned equal by
/// `theme_defaults_are_the_shared_ones` in konoma's config tests).
pub const DEFAULT_CODE_BG_TEXT: &str = "#2b303b";

impl Default for SharedTheme {
    fn default() -> Self {
        Self {
            bg: "none".into(),
            code_bg: DEFAULT_CODE_BG_TEXT.into(),
            code_label_align: "right".into(),
            code_label_bg: "auto".into(),
            code_theme: DEFAULT_CODE_THEME.into(),
        }
    }
}

/// The `[ui]` settings a small host honours.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct SharedUi {
    /// Display language: "auto" (follows the OS) | "en" | "jp".
    pub lang: String,
    /// Preview paging key style: "vim" | "less".
    pub keys: String,
    /// The longest side, in pixels, an SVG picture is drawn at (konoma's `ui.svg_max_px`; its
    /// default is `konoma_plugin::canvas_pictures::DEFAULT_SVG_MAX_PX`, pinned by a test there).
    pub svg_max_px: u32,
    /// Wrapping in text previews and documents (konoma's `ui.wrap`): `false` scrolls sideways.
    pub wrap: bool,
    /// Icon glyphs in a document (konoma's `ui.icons`).
    pub icons: bool,
    /// The tab stop of code blocks in a document (konoma's `ui.tab_width`).
    pub tab_width: usize,
    pub theme: SharedTheme,
    /// The old plugin settings that lived in `[ui]` (`md_*`, `mermaid_*`, ...), typed. Use
    /// [`SharedUi::moved`], which also carries `svg_max_px` and `code_theme`.
    #[serde(flatten)]
    moved_keys: MovedUi,
}

impl SharedUi {
    /// Everything a plugin's settings are completed from, the same as konoma's
    /// `UiConfig::moved`.
    pub fn moved(&self) -> MovedUi {
        MovedUi {
            svg_max_px: self.svg_max_px,
            code_theme: self.theme.code_theme.clone(),
            ..self.moved_keys.clone()
        }
    }
}

impl Default for SharedUi {
    fn default() -> Self {
        Self {
            lang: "auto".into(),
            keys: "vim".into(),
            svg_max_px: DEFAULT_SVG_MAX_PX,
            wrap: true,
            icons: true,
            tab_width: 4,
            theme: SharedTheme::default(),
            moved_keys: MovedUi::default(),
        }
    }
}

/// The `[external]` switches a small host honours (same names and defaults as konoma's).
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct SharedExternal {
    /// Opening URLs/files with the OS handler.
    pub open_links: bool,
    /// `e` on an Office document opens it in a GUI app instead of handing the zip to the editor.
    pub office_apps: bool,
    /// Fetching `http(s)://` images a document names (`curl`): the only network call. Off marks
    /// every remote image failed without asking.
    pub remote_images: bool,
}

impl Default for SharedExternal {
    fn default() -> Self {
        Self {
            open_links: true,
            office_apps: true,
            remote_images: true,
        }
    }
}

/// The shared part of konoma's `config.toml`. Unknown tables and fields are ignored, so a file
/// written for konoma loads here unchanged.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct SharedConfig {
    pub ui: SharedUi,
    pub keys: KeysConfig,
    pub editor: EditorConfig,
    pub external: SharedExternal,
    /// The `[plugins.<id>]` tables as written (the host reads `enabled`; the rest is the plugin's
    /// own settings, converted by `konoma_plugin::config`).
    pub plugins: std::collections::BTreeMap<String, toml::Table>,
}

impl SharedConfig {
    /// Parses `text`. Err carries the one-line reason (`format_load_error`).
    pub fn parse(text: &str) -> Result<Self, String> {
        toml::from_str::<Self>(text).map_err(|e| format_load_error(&e))
    }

    /// Loads the file at `path`: (config, load error). An absent or unreadable file is normal and
    /// gives the defaults without an error; a file that is read but broken gives the defaults and
    /// an error message.
    pub fn load(path: &Path) -> (Self, Option<String>) {
        let Ok(text) = std::fs::read_to_string(path) else {
            return (Self::default(), None);
        };
        match Self::parse(&text) {
            Ok(cfg) => (cfg, None),
            Err(msg) => (
                Self::default(),
                Some(format!("config error (using defaults): {msg}")),
            ),
        }
    }

    /// `load` from konoma's config file (`$XDG_CONFIG_HOME/konoma/config.toml`).
    pub fn load_default() -> (Self, Option<String>) {
        match konoma_base::xdg::dirs_config_path() {
            Some(p) => Self::load(&p),
            None => (Self::default(), None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use konoma_test_support::unique_tmp;

    const EXAMPLE_EN: &str = include_str!("../../../config.example.toml");
    const EXAMPLE_JA: &str = include_str!("../../../config.example.ja.toml");

    #[test]
    fn empty_text_is_all_defaults() {
        let c = SharedConfig::parse("").expect("empty parses");
        assert_eq!(c.ui.lang, "auto");
        assert_eq!(c.ui.keys, "vim");
        assert_eq!(c.ui.svg_max_px, 800);
        assert!(c.external.open_links && c.external.office_apps);
        assert!(c.editor.command.is_empty() && c.editor.ext.is_empty());
        assert!(c.keys.surfaces.is_empty());
        assert_eq!(c.keys.copy_prefix, "c");
    }

    #[test]
    fn remote_images_is_read_and_defaults_to_on() {
        assert!(SharedConfig::parse("").unwrap().external.remote_images);
        let off = SharedConfig::parse("[external]\nremote_images = false\n").unwrap();
        assert!(!off.external.remote_images);
        assert!(off.external.open_links, "the other switches stay default");
        assert!(SharedConfig::parse("[external]\nremote_images = 1\n").is_err());
    }

    #[test]
    fn the_display_settings_default_and_are_read() {
        let d = SharedConfig::parse("").unwrap().ui;
        assert!(d.wrap && d.icons);
        assert_eq!(d.tab_width, 4);
        assert_eq!(d.theme, SharedTheme::default());
        let c = SharedConfig::parse(
            "[ui]\nwrap = false\nicons = false\ntab_width = 8\n[ui.theme]\nbg = \"#102030\"\ncode_bg = \"none\"\ncode_label_align = \"left\"\ncode_label_bg = \"none\"\ncode_theme = \"x\"\n",
        )
        .unwrap()
        .ui;
        assert!(!c.wrap && !c.icons);
        assert_eq!(c.tab_width, 8);
        assert_eq!(
            (c.theme.bg.as_str(), c.theme.code_bg.as_str()),
            ("#102030", "none")
        );
        assert_eq!(
            (
                c.theme.code_label_align.as_str(),
                c.theme.code_label_bg.as_str()
            ),
            ("left", "none")
        );
        for bad in [
            "[ui]\nwrap = \"no\"\n",
            "[ui]\ntab_width = \"x\"\n",
            "[ui]\nicons = 1\n",
            "[ui.theme]\nbg = 5\n",
            "[ui.theme]\ncode_bg = true\n",
            "[ui.theme]\ncode_label_align = 1\n",
            "[ui.theme]\ncode_label_bg = 1\n",
        ] {
            assert!(SharedConfig::parse(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn unknown_tables_and_fields_are_ignored() {
        let c = SharedConfig::parse(
            "[ui]\nlang = \"jp\"\nbrand_new = 1\n[git]\ntool = \"x\"\n[plugins.foo]\nenabled = false\n[external]\npdf = false\nopen_links = false\n",
        )
        .expect("unknown parts are ignored");
        assert_eq!(c.ui.lang, "jp");
        assert!(!c.external.open_links);
        assert!(c.external.office_apps, "unset stays default");
    }

    #[test]
    fn plugin_tables_are_kept_as_written_and_the_old_ui_keys_are_typed() {
        let c = SharedConfig::parse(
            "[ui]\nlang = \"jp\"\nmd_alerts = false\nwrap = true\n[plugins.markdown]\nemoji = false\n[plugins.csv]\nrainbow = true\n",
        )
        .unwrap();
        assert_eq!(c.ui.lang, "jp", "known keys are still read");
        assert!(
            !c.ui.moved().md_alerts,
            "an old plugin setting is read, typed"
        );
        assert_eq!(c.plugins.len(), 2);
        assert_eq!(c.plugins["markdown"]["emoji"], toml::Value::Boolean(false));
        assert!(SharedConfig::parse("[plugins]\nmarkdown = 3\n").is_err());
    }

    #[test]
    fn broken_toml_reports_and_load_falls_back_to_defaults() {
        assert!(SharedConfig::parse("[ui\nlang = ").is_err());
        let dir = unique_tmp("host_core_broken");
        std::fs::create_dir_all(dir.as_path()).unwrap();
        let p = dir.as_path().join("config.toml");
        std::fs::write(&p, "[ui\nlang = ").unwrap();
        let (c, err) = SharedConfig::load(&p);
        assert_eq!(c.ui.lang, "auto");
        let err = err.expect("a broken file is reported");
        assert!(err.starts_with("config error (using defaults): "), "{err}");
    }

    #[test]
    fn wrong_type_is_reported_with_its_reason() {
        let e = SharedConfig::parse("[external]\nopen_links = \"yes\"\n").unwrap_err();
        assert!(e.contains("line 2"), "{e}");
    }

    #[test]
    fn missing_and_empty_files_are_not_errors() {
        let dir = unique_tmp("host_core_missing");
        std::fs::create_dir_all(dir.as_path()).unwrap();
        let (c, err) = SharedConfig::load(&dir.as_path().join("nope.toml"));
        assert!(err.is_none());
        assert_eq!(c.ui.keys, "vim");
        let p = dir.as_path().join("empty.toml");
        std::fs::write(&p, "").unwrap();
        let (_, err) = SharedConfig::load(&p);
        assert!(err.is_none());
    }

    #[test]
    fn the_shared_fields_are_read() {
        let c = SharedConfig::parse(
            "[ui]\nkeys = \"less\"\n[editor]\ncommand = \"nvim\"\n[editor.ext]\nmd = \"code -w\"\n[keys]\ncopy_name = \"m\"\n[keys.tree]\n\"space d\" = \"file_delete\"\n",
        )
        .unwrap();
        assert_eq!(c.ui.keys, "less");
        assert_eq!(c.editor.command, "nvim");
        assert_eq!(c.editor.ext.get("md").map(String::as_str), Some("code -w"));
        assert_eq!(c.keys.copy_name, "m");
        assert_eq!(
            c.keys.surfaces["tree"]["space d"], "file_delete",
            "subtables land in surfaces"
        );
        let k = c.keys.to_keymap_config();
        assert_eq!(k.surfaces["global"]["y m"], "copy_name");
    }

    #[test]
    fn an_old_plugin_key_of_the_wrong_type_is_an_error_not_an_unknown_key() {
        for text in [
            "[ui]\nmd_alerts = \"yes\"\n",
            "[ui]\nmermaid_rows = 100000\n",
            "[ui]\nmd_task_states = [\" \", 3]\n",
            "[ui]\nsvg_max_px = -1\n",
            "[ui.theme]\ncode_theme = 3\n",
        ] {
            let e = SharedConfig::parse(text).expect_err(text);
            assert!(!e.contains('\n'), "{e:?}");
        }
        let dir = unique_tmp("host_core_moved_type");
        std::fs::create_dir_all(dir.as_path()).unwrap();
        let p = dir.as_path().join("config.toml");
        std::fs::write(&p, "[ui]\nlang = \"jp\"\ncsv_rainbow = 1\n").unwrap();
        let (c, err) = SharedConfig::load(&p);
        assert_eq!(c.ui.lang, "auto", "the defaults, not half a file");
        assert!(err.unwrap().starts_with("config error (using defaults): "));
    }

    #[test]
    fn moved_keys_carry_the_owners_own_values_and_the_defaults() {
        let c = SharedConfig::parse(
            "[ui]\nsvg_max_px = 1600\nmermaid_rows = 9\n[ui.theme]\ncode_theme = \"Nord\"\n",
        )
        .unwrap();
        let m = c.ui.moved();
        assert_eq!(
            (m.svg_max_px, m.mermaid_rows, m.code_theme.as_str()),
            (1600, 9, "Nord")
        );
        let d = SharedConfig::parse("").unwrap().ui.moved();
        assert_eq!(d.svg_max_px, DEFAULT_SVG_MAX_PX);
        assert_eq!(d.code_theme, DEFAULT_CODE_THEME);
        assert_eq!(d.math_color(), DEFAULT_MATH_COLOR);
        assert_eq!(visible_math_color("wihte"), DEFAULT_MATH_COLOR);
        assert_eq!(visible_math_color(" #123456 "), "#123456");
    }

    #[test]
    fn both_example_files_parse_with_the_documented_values() {
        for (name, text) in [("en", EXAMPLE_EN), ("ja", EXAMPLE_JA)] {
            let c = SharedConfig::parse(text).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(c.ui.lang, "auto", "{name}");
            assert_eq!(c.ui.keys, "vim", "{name}");
            assert!(c.external.open_links && c.external.office_apps, "{name}");
            assert!(c.editor.command.is_empty(), "{name}");
        }
    }
}
