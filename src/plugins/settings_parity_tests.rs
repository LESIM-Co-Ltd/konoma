//! The settings a plugin receives, pinned to what they were before they were decided in one shared
//! function (`konoma_plugin::config::plugin_settings`): konoma's `effective_settings`, the small
//! host's reading of the same text, and the frozen original (`legacy_effective_settings`) must
//! agree, for every plugin and for a corpus made from the cases of the rule itself.

use super::*;
use crate::config::Config;
use konoma_host_core::shared::SharedConfig;
use konoma_plugin::Value;

fn s(v: &str) -> Value {
    Value::Str(v.to_string())
}

fn b(v: bool) -> Value {
    Value::Bool(v)
}

/// The host's own keys as they were: read by the host, not settings of the plugin (frozen).
fn legacy_is_host_key(table: &toml::Table, key: &str) -> bool {
    ["enabled", "read_only", "read_dirs"].contains(&key)
        || (table.contains_key("command") && ["command", "args"].contains(&key))
}

/// The TOML to plugin value conversion as it was (frozen).
fn legacy_value_of(v: &toml::Value) -> Value {
    match v {
        toml::Value::Boolean(x) => Value::Bool(*x),
        toml::Value::Integer(x) => Value::Int(*x),
        toml::Value::Float(x) => Value::Float(*x),
        toml::Value::String(x) => Value::Str(x.clone()),
        toml::Value::Datetime(x) => Value::Str(x.to_string()),
        toml::Value::Array(xs) => Value::List(xs.iter().map(legacy_value_of).collect()),
        toml::Value::Table(t) => Value::Table(
            t.iter()
                .map(|(k, v)| (k.clone(), legacy_value_of(v)))
                .collect::<std::collections::BTreeMap<_, _>>(),
        ),
    }
}

/// A plugin's own settings from its table, as they were (frozen).
fn legacy_settings_of(table: &toml::Table) -> Settings {
    let mut out = Settings::new();
    for (k, v) in table {
        if !legacy_is_host_key(table, k) {
            out.set(k.clone(), legacy_value_of(v));
        }
    }
    out
}

/// The function as it was before the move: the oracle of this file, down to the conversion of the
/// TOML values and the host's keys (`legacy_settings_of`), none of which calls the shared code.
/// Do not edit it to follow the code; a difference is a change in what a plugin receives.
fn legacy_effective_settings(cfg: &Config, id: &str) -> Settings {
    let mut settings = cfg
        .plugins
        .get(id)
        .map(legacy_settings_of)
        .unwrap_or_default();
    let mut fill_missing = |key: &str, value: konoma_plugin::Value| {
        if settings.get(key).is_none() {
            settings.set(key, value);
        }
    };
    if id == konoma_markdown::MARKDOWN_ID {
        let ui = &cfg.ui;
        fill_missing(
            "task_states",
            konoma_plugin::Value::List(ui.md_task_states.iter().map(|x| s(x)).collect()),
        );
        fill_missing("autolink", b(ui.md_autolink));
        fill_missing("alerts", b(ui.md_alerts));
        fill_missing("emoji", b(ui.md_emoji));
        fill_missing("frontmatter", b(ui.md_frontmatter));
        fill_missing("footnotes", b(ui.md_footnotes));
        fill_missing("inline_html", b(ui.md_inline_html));
        fill_missing("details", s(&ui.md_details));
        fill_missing("table_align", s(&ui.md_table_align));
        fill_missing("image_align", s(&ui.md_image_align));
    }
    if id == konoma_table::CSV_ID {
        fill_missing("rainbow", b(cfg.ui.csv_rainbow));
    }
    if id == konoma_office::SPREADSHEET_ID {
        // A sheet's columns follow `[plugins.csv] rainbow` (and the old `[ui] csv_rainbow`) until
        // the sheet is given a value of its own.
        let csv = legacy_effective_settings(cfg, konoma_table::CSV_ID);
        fill_missing("rainbow", b(csv.bool("rainbow", true)));
    }
    if id == konoma_code::CODE_ID {
        fill_missing("syntax_highlight", b(cfg.ui.syntax_highlight));
        // The theme is shared with the Markdown code blocks, so it stays `[ui.theme] code_theme`;
        // the host hands it to the plugin as a value, not as a `[plugins.code]` setting.
        fill_missing("code_theme", s(&cfg.ui.theme.code_theme));
    }
    if id == konoma_media::SVG_ID {
        fill_missing(
            "max_px",
            konoma_plugin::Value::Int(i64::from(cfg.ui.svg_max_px)),
        );
    }
    if id == konoma_mermaid::MERMAID_ID {
        fill_missing("mode", s(&cfg.ui.mermaid));
        fill_missing("theme", s(&cfg.ui.mermaid_theme));
        fill_missing("curve", s(&cfg.ui.mermaid_curve));
        fill_missing("routing", s(&cfg.ui.mermaid_routing));
        fill_missing(
            "rows",
            konoma_plugin::Value::Int(i64::from(cfg.ui.mermaid_rows)),
        );
    }
    if id == konoma_math::MATH_ID {
        fill_missing("mode", s(&cfg.ui.math));
        fill_missing("color", s(cfg.ui.math_color()));
        // A color from `[plugins.math]` is held to the same test as the old `[ui] math_color`: one
        // the rasterizer would paint black (invisible on a dark terminal) falls back to the default.
        let color = crate::config::visible_math_color(settings.str("color", ""));
        settings.set("color", s(color));
    }
    settings
}

/// Every moved `[ui]` key: `(old key, owner, new key, old value, new value)`, the two values
/// being of the right type and unlike each other (TOML text; a mode is passed on as written, so
/// `"auto"` stands for a second value besides the default `"image"`). The old value is unlike the default,
/// so a fill from `[ui]` shows; the new value is unlike it too, except for a boolean, which has
/// only two values (the corpus also swaps the values between the two names).
const CASES: &[(&str, &str, &str, &str, &str)] = &[
    (
        "md_task_states",
        "markdown",
        "task_states",
        "[\" \", \"/\", \"x\"]",
        "[\" \", \"-\"]",
    ),
    ("md_autolink", "markdown", "autolink", "false", "true"),
    ("md_alerts", "markdown", "alerts", "false", "true"),
    ("md_emoji", "markdown", "emoji", "false", "true"),
    ("md_frontmatter", "markdown", "frontmatter", "false", "true"),
    ("md_footnotes", "markdown", "footnotes", "false", "true"),
    ("md_inline_html", "markdown", "inline_html", "false", "true"),
    (
        "md_details",
        "markdown",
        "details",
        "\"open\"",
        "\"closed\"",
    ),
    (
        "md_table_align",
        "markdown",
        "table_align",
        "\"right\"",
        "\"center\"",
    ),
    (
        "md_image_align",
        "markdown",
        "image_align",
        "\"left\"",
        "\"right\"",
    ),
    ("mermaid", "mermaid", "mode", "\"text\"", "\"auto\""),
    (
        "mermaid_theme",
        "mermaid",
        "theme",
        "\"forest\"",
        "\"light\"",
    ),
    (
        "mermaid_curve",
        "mermaid",
        "curve",
        "\"linear\"",
        "\"step\"",
    ),
    (
        "mermaid_routing",
        "mermaid",
        "routing",
        "\"konoma-orthogonal\"",
        "\"ortho\"",
    ),
    ("mermaid_rows", "mermaid", "rows", "7", "9"),
    ("math", "math", "mode", "\"text\"", "\"auto\""),
    ("math_color", "math", "color", "\"#123456\"", "\"white\""),
    ("csv_rainbow", "csv", "rainbow", "false", "true"),
    (
        "syntax_highlight",
        "code",
        "syntax_highlight",
        "false",
        "true",
    ),
    ("svg_max_px", "svg", "max_px", "321", "654"),
];

/// The config texts: from the cases of the rule, not from the bugs found.
fn corpus() -> Vec<String> {
    let mut out = vec![
        String::new(),
        "[ui]\n".to_string(),
        "[plugins.csv]\n".to_string(),
    ];
    for (old, id, new, old_v, new_v) in CASES {
        // The old name alone, the new name alone, both (the new one wins).
        out.push(format!("[ui]\n{old} = {old_v}\n"));
        out.push(format!("[plugins.{id}]\n{new} = {new_v}\n"));
        out.push(format!(
            "[ui]\n{old} = {old_v}\n[plugins.{id}]\n{new} = {new_v}\n"
        ));
        // The new name set to the value of the old one, and the old one at another value.
        out.push(format!(
            "[ui]\n{old} = {new_v}\n[plugins.{id}]\n{new} = {old_v}\n"
        ));
        // Another plugin's table is not read for this one.
        out.push(format!(
            "[ui]\n{old} = {old_v}\n[plugins.archive]\n{new} = {new_v}\n"
        ));
    }
    // The code theme: `[ui.theme]`, and the plugin's own table over it.
    out.push("[ui.theme]\ncode_theme = \"Dracula\"\n".into());
    out.push(
        "[ui.theme]\ncode_theme = \"Dracula\"\n[plugins.code]\ncode_theme = \"Nord\"\n".into(),
    );
    out.push("[plugins.code]\ncode_theme = \"Nord\"\n".into());
    out.push("[ui.theme]\nbg = \"#000000\"\n".into());
    // The size of an SVG.
    out.push("[ui]\nsvg_max_px = 1600\n".into());
    out.push("[ui]\nsvg_max_px = 0\n".into());
    out.push("[plugins.svg]\nmax_px = 99\n[ui]\nsvg_max_px = 1600\n".into());
    // The rainbow of the sheet: csv only, spreadsheet only, both, and the old name.
    out.push("[plugins.csv]\nrainbow = false\n".into());
    out.push("[plugins.spreadsheet]\nrainbow = false\n".into());
    out.push("[plugins.csv]\nrainbow = false\n[plugins.spreadsheet]\nrainbow = true\n".into());
    out.push("[plugins.csv]\nrainbow = true\n[plugins.spreadsheet]\nrainbow = false\n".into());
    out.push("[ui]\ncsv_rainbow = false\n".into());
    out.push("[ui]\ncsv_rainbow = false\n[plugins.spreadsheet]\nrainbow = true\n".into());
    out.push("[ui]\ncsv_rainbow = false\n[plugins.csv]\nrainbow = true\n".into());
    // The formula color: a color that does not show is the default, in either place.
    for bad in ["wihte", "none", "transparent", "", "currentColor"] {
        out.push(format!("[ui]\nmath_color = \"{bad}\"\n"));
        out.push(format!("[plugins.math]\ncolor = \"{bad}\"\n"));
        out.push(format!(
            "[ui]\nmath_color = \"#abcdef\"\n[plugins.math]\ncolor = \"{bad}\"\n"
        ));
    }
    // A plugin's own key of the wrong type is passed on as it is.
    out.push("[plugins.markdown]\nalerts = \"yes\"\n".into());
    out.push("[plugins.csv]\nrainbow = 3\n[plugins.spreadsheet]\nrainbow = \"x\"\n".into());
    out.push("[plugins.math]\ncolor = 3\n".into());
    // The host's own keys and a program of its own.
    out.push(
        "[plugins.markdown]\nenabled = false\nread_only = true\nread_dirs = [\"/x\"]\n".into(),
    );
    out.push("[plugins.stand]\ncommand = \"x\"\nargs = [\"a\"]\nlevel = 2\n".into());
    out
}

/// Every plugin id of the registry, and some that are not there.
fn all_ids() -> Vec<String> {
    let mut ids: Vec<String> = shared_registry(&[])
        .ids()
        .into_iter()
        .map(str::to_string)
        .collect();
    ids.extend(["nothing", "stand", ""].map(str::to_string));
    ids
}

/// What the plugin reads: every setting it declares, its value when set and its declared default
/// when not.
fn resolved(id: &str, settings: &Settings) -> Vec<(String, Value)> {
    let registry = shared_registry(&[]);
    let Some(manifest) = registry.manifest(id) else {
        return Vec::new();
    };
    manifest
        .settings
        .iter()
        .map(|d| {
            (
                d.name.clone(),
                settings.get(&d.name).cloned().unwrap_or(d.default.clone()),
            )
        })
        .collect()
}

#[test]
fn konomas_settings_are_what_they_were_for_every_plugin_and_config() {
    let ids = all_ids();
    let corpus = corpus();
    assert!(ids.len() >= 14, "the registry's plugins are in: {ids:?}");
    assert!(corpus.len() >= 100, "{}", corpus.len());
    let mut compared = 0;
    for text in &corpus {
        let cfg: Config = toml::from_str(text).unwrap_or_else(|e| panic!("{text:?}: {e}"));
        for id in &ids {
            let was = legacy_effective_settings(&cfg, id);
            let now = effective_settings(&cfg, id);
            assert_eq!(now, was, "{id} from {text:?}");
            assert_eq!(resolved(id, &now), resolved(id, &was), "{id} from {text:?}");
            compared += 1;
        }
    }
    assert_eq!(compared, ids.len() * corpus.len());
}

#[test]
fn the_small_host_gets_what_konoma_gets_for_every_plugin_and_config() {
    let ids = all_ids();
    for text in corpus() {
        let cfg: Config = toml::from_str(&text).unwrap_or_else(|e| panic!("{text:?}: {e}"));
        let shared = SharedConfig::parse(&text).unwrap_or_else(|e| panic!("{text:?}: {e}"));
        for id in &ids {
            let konoma = effective_settings(&cfg, id);
            let small =
                konoma_plugin::config::plugin_settings(&shared.plugins, &shared.ui.moved(), id);
            assert_eq!(small, konoma, "{id} from {text:?}");
            assert_eq!(
                resolved(id, &small),
                resolved(id, &konoma),
                "{id} from {text:?}"
            );
        }
    }
}

/// The corpus reaches every moved key and every plugin that has one: a corpus that does not move
/// a value would agree with anything.
#[test]
fn the_corpus_moves_every_value_of_every_owner() {
    for (old, id, new, _, _) in CASES {
        let moved = corpus()
            .iter()
            .filter(|t| t.contains(&format!("{old} = ")))
            .map(|t| {
                let cfg: Config = toml::from_str(t).unwrap();
                effective_settings(&cfg, id)
            })
            .any(|got| got != effective_settings(&Config::default(), id) && got.get(new).is_some());
        assert!(moved, "{old} -> {id}.{new} changes nothing in the corpus");
    }
}

/// A value of another type for each moved key, as TOML text.
fn wrong_typed(old: &str, ok_value: &str) -> Vec<String> {
    let mut out = Vec::new();
    let is_bool = ok_value == "true" || ok_value == "false";
    let is_int = ok_value.parse::<i64>().is_ok();
    let is_list = ok_value.starts_with('[');
    if !is_bool {
        out.push("true".to_string());
    }
    if !is_int {
        out.push("3".to_string());
    }
    if is_bool || is_int || is_list {
        out.push("\"yes\"".to_string());
    }
    if !is_list {
        out.push("[\"a\"]".to_string());
    }
    out.push("1.5".to_string());
    out.push("{ k = 1 }".to_string());
    if old == "md_task_states" {
        out.push("[\" \", 3]".to_string());
    }
    if old == "mermaid_rows" {
        out.extend(["100000".to_string(), "-1".to_string()]);
    }
    if old == "svg_max_px" {
        out.extend(["-1".to_string(), "4294967296".to_string()]);
    }
    out
}

/// A moved `[ui]` key of the wrong type makes the whole file unreadable in konoma, and the small
/// host takes the same text the same way (an error, then the defaults), for every moved key.
#[test]
fn a_wrong_typed_moved_key_is_an_error_in_both_hosts() {
    let mut checked = 0;
    for (old, _, _, ok, _) in CASES {
        // The right type is accepted by both.
        let good = format!("[ui]\n{old} = {ok}\n");
        assert!(toml::from_str::<Config>(&good).is_ok(), "{good:?}");
        assert!(SharedConfig::parse(&good).is_ok(), "{good:?}");
        for bad in wrong_typed(old, ok) {
            let text = format!("[ui]\nshow_hidden = true\n{old} = {bad}\n");
            let konoma = toml::from_str::<Config>(&text);
            let small = SharedConfig::parse(&text);
            assert_eq!(konoma.is_err(), small.is_err(), "{text:?}");
            checked += 1;
        }
    }
    // The ones that cannot be fitted to a type by this rule are errors in konoma, so also here.
    for text in [
        "[ui]\nmermaid_rows = 100000\n",
        "[ui]\nmermaid_rows = -1\n",
        "[ui]\nsvg_max_px = -1\n",
        "[ui]\nmd_task_states = [\" \", 3]\n",
        "[ui]\nmd_alerts = \"yes\"\n",
        "[ui.theme]\ncode_theme = 3\n",
        "[ui]\nmath_color = 3\n",
        "[ui]\ncsv_rainbow = 1\n",
    ] {
        assert!(toml::from_str::<Config>(text).is_err(), "{text:?}");
        let e = SharedConfig::parse(text).expect_err(text);
        assert!(!e.contains('\n') && !e.is_empty(), "one line: {e:?}");
    }
    // The edges that are accepted: the largest u16, zero.
    for text in [
        "[ui]\nmermaid_rows = 65535\n",
        "[ui]\nmermaid_rows = 0\n",
        "[ui]\nsvg_max_px = 4294967295\n",
        "[ui]\nmd_task_states = []\n",
    ] {
        assert!(toml::from_str::<Config>(text).is_ok(), "{text:?}");
        assert!(SharedConfig::parse(text).is_ok(), "{text:?}");
    }
    assert!(checked >= 60, "{checked}");
}

/// The file is read as a whole in both hosts: one wrong-typed moved key and the other settings of
/// the file (a plugin table, a language) are not used either, and the plugins get the defaults.
#[test]
fn a_wrong_typed_moved_key_gives_the_defaults_and_one_warning_in_the_small_host() {
    let dir = crate::test_support::unique_tmp("settings_parity_load");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("config.toml");
    let text = "[ui]\nlang = \"jp\"\nmd_alerts = \"yes\"\n[plugins.csv]\nrainbow = false\n";
    std::fs::write(&path, text).unwrap();
    assert!(toml::from_str::<Config>(text).is_err());
    let (shared, error) = SharedConfig::load(&path);
    let error = error.expect("a warning");
    assert!(
        error.starts_with("config error (using defaults): "),
        "{error:?}"
    );
    assert!(!error.contains('\n'));
    assert_eq!(shared.ui.lang, "auto", "the whole file is dropped");
    assert!(shared.plugins.is_empty());
    for id in all_ids() {
        assert_eq!(
            konoma_plugin::config::plugin_settings(&shared.plugins, &shared.ui.moved(), &id),
            effective_settings(&Config::default(), &id),
            "{id}"
        );
    }
}

/// What `CASES` says of its values holds: the two differ from each other, the old one differs from
/// the default, and so does the new one unless it is a boolean.
#[test]
fn the_values_of_every_case_differ_from_each_other_and_from_the_default() {
    let value = |text: &str| -> Value {
        let t: toml::Table = toml::from_str(&format!("v = {text}")).unwrap();
        legacy_value_of(&t["v"])
    };
    for (old, id, new, old_v, new_v) in CASES {
        let (a, c) = (value(old_v), value(new_v));
        let default = legacy_effective_settings(&Config::default(), id);
        let default = default
            .get(new)
            .unwrap_or_else(|| panic!("{id}.{new} has a default"));
        assert_ne!(a, c, "{old}: the two values are alike");
        assert_ne!(&a, default, "{old}: the old value is the default");
        if !matches!(c, Value::Bool(_)) {
            assert_ne!(&c, default, "{old}: the new value is the default");
        }
    }
}
