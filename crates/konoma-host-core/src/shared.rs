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

/// The `[ui]` settings a small host honours.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct SharedUi {
    /// Display language: "auto" (follows the OS) | "en" | "jp".
    pub lang: String,
    /// Preview paging key style: "vim" | "less".
    pub keys: String,
}

impl Default for SharedUi {
    fn default() -> Self {
        Self {
            lang: "auto".into(),
            keys: "vim".into(),
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
}

impl Default for SharedExternal {
    fn default() -> Self {
        Self {
            open_links: true,
            office_apps: true,
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
        assert!(c.external.open_links && c.external.office_apps);
        assert!(c.editor.command.is_empty() && c.editor.ext.is_empty());
        assert!(c.keys.surfaces.is_empty());
        assert_eq!(c.keys.copy_prefix, "c");
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
