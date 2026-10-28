//! Reading `[plugins.<id>]` from `config.toml` into the [`Settings`] a plugin receives.
//!
//! One reading for both hosts: konoma and the standalone app (the small host of a plugin program)
//! call the same functions, so a value means the same in either. What is here is the part that
//! needs no knowledge of konoma's own config structs: the TOML-to-[`Value`] conversion, the keys
//! that are the host's own, and the old `[ui]` names that moved into plugin tables.

use std::collections::BTreeMap;

use konoma_host_core::shared::{visible_math_color, MovedUi};

use crate::{Settings, Value};

/// The keys of `[plugins.<id>]` that are the host's own, not the plugin's settings: `enabled`,
/// `read_only` (both booleans) and `read_dirs` (a list of folders).
pub const HOST_PLUGIN_KEYS: &[&str] = &["enabled", "read_only", "read_dirs"];

/// The keys that are the host's own only in the table of a program of its own (one with a
/// `command`): the program to run and its arguments. In the table of a plugin that runs in the
/// host's process they are the plugin's own settings, if it declares them.
pub const EXTERNAL_PLUGIN_KEYS: &[&str] = &["command", "args"];

/// Whether `key` of `table` is read by the host rather than being a setting of the plugin.
pub fn is_host_key(table: &toml::Table, key: &str) -> bool {
    HOST_PLUGIN_KEYS.contains(&key)
        || (table.contains_key("command") && EXTERNAL_PLUGIN_KEYS.contains(&key))
}

/// A TOML value as a settings [`Value`] (a date-time becomes its text).
pub fn value_of(v: &toml::Value) -> Value {
    match v {
        toml::Value::Boolean(x) => Value::Bool(*x),
        toml::Value::Integer(x) => Value::Int(*x),
        toml::Value::Float(x) => Value::Float(*x),
        toml::Value::String(x) => Value::Str(x.clone()),
        toml::Value::Datetime(x) => Value::Str(x.to_string()),
        toml::Value::Array(xs) => Value::List(xs.iter().map(value_of).collect()),
        toml::Value::Table(t) => Value::Table(
            t.iter()
                .map(|(k, v)| (k.clone(), value_of(v)))
                .collect::<BTreeMap<_, _>>(),
        ),
    }
}

/// The plugin's own settings from its table (the host's keys are not among them).
pub fn settings_of(table: &toml::Table) -> Settings {
    let mut out = Settings::new();
    for (k, v) in table {
        if !is_host_key(table, k) {
            out.set(k.clone(), value_of(v));
        }
    }
    out
}

/// The `[ui]` settings that moved to `[plugins.<id>]`: the old `[ui]` key, the id of the plugin
/// that owns it now, and its new name. Both hosts read the old keys; konoma's startup message
/// names this list so a user learns where to write them. A key that was only renamed inside its
/// plugin's table does not appear: the display settings (`wrap`, `line_numbers`, `git_gutter`,
/// `tab_width`, `image_render_scale`) stay in `[ui]`.
pub const MOVED_UI_SETTINGS: &[(&str, &str, &str)] = &[
    ("md_task_states", "markdown", "task_states"),
    ("md_autolink", "markdown", "autolink"),
    ("md_alerts", "markdown", "alerts"),
    ("md_emoji", "markdown", "emoji"),
    ("md_frontmatter", "markdown", "frontmatter"),
    ("md_footnotes", "markdown", "footnotes"),
    ("md_inline_html", "markdown", "inline_html"),
    ("md_details", "markdown", "details"),
    ("md_table_align", "markdown", "table_align"),
    ("md_image_align", "markdown", "image_align"),
    ("mermaid", "mermaid", "mode"),
    ("mermaid_theme", "mermaid", "theme"),
    ("mermaid_curve", "mermaid", "curve"),
    ("mermaid_routing", "mermaid", "routing"),
    ("mermaid_rows", "mermaid", "rows"),
    ("math", "math", "mode"),
    ("math_color", "math", "color"),
    ("csv_rainbow", "csv", "rainbow"),
    ("syntax_highlight", "code", "syntax_highlight"),
    ("svg_max_px", "svg", "max_px"),
];

/// What plugin `id` receives from a config file: its `[plugins.<id>]` table, completed from the
/// old `[ui]` items that moved into it ([`MOVED_UI_SETTINGS`]; `ui` is [`MovedUi`], whose
/// defaults stand in for an item the file does not set). The new name wins when both are set; a
/// value of the wrong type in the plugin's own table is passed on as it is (the plugin falls back
/// to its declared default, as with any setting).
///
/// The one place this is decided: konoma (`plugins::effective_settings`) and the standalone app
/// both call it, so a plugin receives the same values from the same config file in either.
/// A plugin that is wired to the display adds its old names here.
pub fn plugin_settings(
    plugins: &BTreeMap<String, toml::Table>,
    ui: &MovedUi,
    id: &str,
) -> Settings {
    let mut settings = plugins.get(id).map(settings_of).unwrap_or_default();
    let mut fill_missing = |key: &str, value: Value| {
        if settings.get(key).is_none() {
            settings.set(key, value);
        }
    };
    let s = |v: &str| Value::Str(v.to_string());
    match id {
        "markdown" => {
            fill_missing(
                "task_states",
                Value::List(ui.md_task_states.iter().map(|x| s(x)).collect()),
            );
            fill_missing("autolink", Value::Bool(ui.md_autolink));
            fill_missing("alerts", Value::Bool(ui.md_alerts));
            fill_missing("emoji", Value::Bool(ui.md_emoji));
            fill_missing("frontmatter", Value::Bool(ui.md_frontmatter));
            fill_missing("footnotes", Value::Bool(ui.md_footnotes));
            fill_missing("inline_html", Value::Bool(ui.md_inline_html));
            fill_missing("details", s(&ui.md_details));
            fill_missing("table_align", s(&ui.md_table_align));
            fill_missing("image_align", s(&ui.md_image_align));
        }
        "csv" => fill_missing("rainbow", Value::Bool(ui.csv_rainbow)),
        "spreadsheet" => {
            // A sheet's columns follow `[plugins.csv] rainbow` (and the old `[ui] csv_rainbow`)
            // until the sheet is given a value of its own.
            let csv = plugin_settings(plugins, ui, "csv");
            fill_missing("rainbow", Value::Bool(csv.bool("rainbow", true)));
        }
        "code" => {
            fill_missing("syntax_highlight", Value::Bool(ui.syntax_highlight));
            // The theme is shared with the Markdown code blocks, so it stays `[ui.theme]
            // code_theme`; the host hands it to the plugin as a value, not as a
            // `[plugins.code]` setting.
            fill_missing("code_theme", s(&ui.code_theme));
        }
        "svg" => fill_missing("max_px", Value::Int(i64::from(ui.svg_max_px))),
        "mermaid" => {
            fill_missing("mode", s(&ui.mermaid));
            fill_missing("theme", s(&ui.mermaid_theme));
            fill_missing("curve", s(&ui.mermaid_curve));
            fill_missing("routing", s(&ui.mermaid_routing));
            fill_missing("rows", Value::Int(i64::from(ui.mermaid_rows)));
        }
        "math" => {
            fill_missing("mode", s(&ui.math));
            fill_missing("color", s(ui.math_color()));
            // A color from `[plugins.math]` is held to the same test as the old `[ui]
            // math_color`: one the rasterizer would paint black (invisible on a dark terminal)
            // falls back to the default.
            let color = visible_math_color(settings.str("color", "")).to_string();
            settings.set("color", Value::Str(color));
        }
        _ => {}
    }
    settings
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(s: &str) -> toml::Table {
        toml::from_str(s).unwrap()
    }

    fn moved(s: &str) -> MovedUi {
        toml::from_str(s).expect("the [ui] text is typed right")
    }

    fn plugins(s: &str) -> BTreeMap<String, toml::Table> {
        let t = table(s);
        match t.get("plugins") {
            Some(toml::Value::Table(p)) => p
                .iter()
                .filter_map(|(k, v)| v.as_table().map(|t| (k.clone(), t.clone())))
                .collect(),
            _ => BTreeMap::new(),
        }
    }

    #[test]
    fn host_keys_are_not_settings() {
        let t = table("enabled = false\nread_only = true\nread_dirs = [\"/x\"]\nmode = \"text\"");
        let s = settings_of(&t);
        assert_eq!(s.get("mode"), Some(&Value::Str("text".into())));
        for k in HOST_PLUGIN_KEYS {
            assert!(s.get(k).is_none(), "{k}");
        }
    }

    #[test]
    fn command_and_args_are_host_keys_only_with_a_command() {
        let with = settings_of(&table("command = \"x\"\nargs = [\"a\"]\nk = 1"));
        assert!(with.get("command").is_none() && with.get("args").is_none());
        assert_eq!(with.get("k"), Some(&Value::Int(1)));
        let without = settings_of(&table("args = [\"a\"]"));
        assert!(without.get("args").is_some());
    }

    #[test]
    fn every_toml_type_converts() {
        let t = table(
            "b = true\ni = 3\nf = 1.5\ns = \"x\"\nl = [1, 2]\nd = 1979-05-27\n[t]\nk = false",
        );
        let s = settings_of(&t);
        assert_eq!(s.get("b"), Some(&Value::Bool(true)));
        assert_eq!(s.get("i"), Some(&Value::Int(3)));
        assert_eq!(s.get("f"), Some(&Value::Float(1.5)));
        assert_eq!(s.get("s"), Some(&Value::Str("x".into())));
        assert_eq!(
            s.get("l"),
            Some(&Value::List(vec![Value::Int(1), Value::Int(2)]))
        );
        assert_eq!(s.get("d"), Some(&Value::Str("1979-05-27".into())));
        assert!(
            matches!(s.get("t"), Some(Value::Table(m)) if m.get("k") == Some(&Value::Bool(false)))
        );
    }

    #[test]
    fn an_old_ui_name_fills_in_and_the_new_name_wins() {
        let p = plugins("[plugins.markdown]\nalerts = true\n");
        let ui = moved("md_alerts = false\nmd_emoji = false\nunrelated = 1");
        let s = plugin_settings(&p, &ui, "markdown");
        assert_eq!(s.get("alerts"), Some(&Value::Bool(true)), "new name wins");
        assert_eq!(
            s.get("emoji"),
            Some(&Value::Bool(false)),
            "old name fills in"
        );
        assert!(s.get("unrelated").is_none());
        let other = plugin_settings(&p, &ui, "mermaid");
        assert!(other.get("alerts").is_none() && other.get("emoji").is_none());
    }

    /// A sample of each old `[ui]` key, of the type it has, different from its default.
    const MOVED_SAMPLES: &[(&str, &str)] = &[
        ("md_task_states", "[\"-\", \"x\"]"),
        ("md_autolink", "false"),
        ("md_alerts", "false"),
        ("md_emoji", "false"),
        ("md_frontmatter", "false"),
        ("md_footnotes", "false"),
        ("md_inline_html", "false"),
        ("md_details", "\"open\""),
        ("md_table_align", "\"right\""),
        ("md_image_align", "\"left\""),
        ("mermaid", "\"text\""),
        ("mermaid_theme", "\"forest\""),
        ("mermaid_curve", "\"linear\""),
        ("mermaid_routing", "\"konoma-orthogonal\""),
        ("mermaid_rows", "7"),
        ("math", "\"text\""),
        ("math_color", "\"#123456\""),
        ("csv_rainbow", "false"),
        ("syntax_highlight", "false"),
        ("svg_max_px", "123"),
    ];

    #[test]
    fn every_moved_name_reaches_its_owner() {
        assert_eq!(
            MOVED_SAMPLES.len(),
            MOVED_UI_SETTINGS.len(),
            "a moved key without a sample"
        );
        for (old, id, new) in MOVED_UI_SETTINGS {
            let (_, text) = MOVED_SAMPLES
                .iter()
                .find(|(k, _)| k == old)
                .unwrap_or_else(|| panic!("no sample for {old}"));
            // `svg_max_px` is the host's own key (the owner of `[ui]` sets it in `moved()`).
            let mut ui = moved(&format!("{old} = {text}"));
            if *old == "svg_max_px" {
                ui.svg_max_px = 123;
            }
            let s = plugin_settings(&BTreeMap::new(), &ui, id);
            let got = s.get(new).unwrap_or_else(|| panic!("{old} -> {id}.{new}"));
            let default = plugin_settings(&BTreeMap::new(), &MovedUi::default(), id);
            assert_ne!(
                Some(got),
                default.get(new),
                "{old} -> {id}.{new} is not the default"
            );
        }
    }

    #[test]
    fn a_wrong_typed_value_is_passed_on_and_reads_as_the_default() {
        let p = plugins("[plugins.markdown]\nalerts = \"yes\"\n");
        let s = plugin_settings(&p, &MovedUi::default(), "markdown");
        assert_eq!(s.get("alerts"), Some(&Value::Str("yes".into())));
        assert!(
            s.bool("alerts", true),
            "the plugin's read falls back to the default"
        );
    }

    #[test]
    fn a_sheet_follows_the_csv_rainbow_until_it_has_its_own() {
        let p = plugins("[plugins.csv]\nrainbow = false\n");
        let s = plugin_settings(&p, &MovedUi::default(), "spreadsheet");
        assert_eq!(s.get("rainbow"), Some(&Value::Bool(false)));
        let own =
            plugins("[plugins.csv]\nrainbow = false\n[plugins.spreadsheet]\nrainbow = true\n");
        let s = plugin_settings(&own, &MovedUi::default(), "spreadsheet");
        assert_eq!(s.get("rainbow"), Some(&Value::Bool(true)));
        let old = plugin_settings(
            &BTreeMap::new(),
            &moved("csv_rainbow = false"),
            "spreadsheet",
        );
        assert_eq!(
            old.get("rainbow"),
            Some(&Value::Bool(false)),
            "old [ui] name flows through csv"
        );
    }
}
