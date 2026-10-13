//! Tests added after a mutation audit of the plugin host's configuration side: the old
//! `builtin = "csv"` rule, the settings a plugin is handed, and the registry the host shares.

use super::*;
use crate::config::{Config, Rule};
use crate::preview::PreviewKind;
use konoma_plugin::Value;
use std::path::Path;

fn rule(glob: &str, builtin: &str) -> Rule {
    Rule {
        glob: Some(glob.to_string()),
        builtin: Some(builtin.to_string()),
        ..Rule::default()
    }
}

fn hint_of(kind: PreviewKind) -> Option<String> {
    match kind {
        PreviewKind::Plugin { hint, id, .. } => {
            assert_eq!(id, "csv");
            hint
        }
        other => panic!("not the csv plugin: {other:?}"),
    }
}

/// An old `builtin = "csv"` rule says the comma, whatever the file is called, and `"tsv"` the tab:
/// the rule's word wins over the extension.
#[test]
fn a_csv_rule_keeps_its_comma_on_a_tsv_name_and_a_tsv_rule_its_tab_on_a_csv_name() {
    let csv_rule = [rule("*.tsv", "csv")];
    let kind = resolve::resolve_kind(
        &csv_rule,
        &[],
        true,
        Path::new("/nowhere/x.tsv"),
        Some(b"a,b\n1,2\n"),
    );
    assert_eq!(hint_of(kind).as_deref(), Some("csv"));
    let tsv_rule = [rule("*.csv", "tsv")];
    let kind = resolve::resolve_kind(
        &tsv_rule,
        &[],
        true,
        Path::new("/nowhere/x.csv"),
        Some(b"a\tb\n"),
    );
    assert_eq!(hint_of(kind).as_deref(), Some("tsv"));
    // With no rule the extension decides, as it always did.
    let kind = resolve::resolve_kind(&[], &[], true, Path::new("/nowhere/x.tsv"), Some(b"a\tb\n"));
    assert_eq!(hint_of(kind).as_deref(), Some("tsv"));
    let kind = resolve::resolve_kind(&[], &[], true, Path::new("/nowhere/x.csv"), Some(b"a,b\n"));
    assert_eq!(hint_of(kind).as_deref(), Some("csv"));
}

/// Every TOML type becomes the settings value of the same shape.
#[test]
fn every_toml_type_round_trips_into_a_settings_value() {
    let table: toml::Table = toml::from_str(
        r#"
        b = true
        i = 7
        f = 1.5
        s = "text"
        d = 1979-05-27T07:32:00Z
        l = [1, "two", false]
        [t]
        k = "v"
        n = [ { x = 1 } ]
        "#,
    )
    .unwrap();
    let s = settings_of(&table);
    assert_eq!(s.get("b"), Some(&Value::Bool(true)));
    assert_eq!(s.get("i"), Some(&Value::Int(7)));
    assert_eq!(s.get("f"), Some(&Value::Float(1.5)));
    assert_eq!(s.get("s"), Some(&Value::Str("text".into())));
    assert_eq!(
        s.get("d"),
        Some(&Value::Str("1979-05-27T07:32:00Z".into())),
        "a date is its text"
    );
    assert_eq!(
        s.get("l"),
        Some(&Value::List(vec![
            Value::Int(1),
            Value::Str("two".into()),
            Value::Bool(false)
        ]))
    );
    match s.get("t") {
        Some(Value::Table(t)) => {
            assert_eq!(t.get("k"), Some(&Value::Str("v".into())));
            assert_eq!(
                t.get("n"),
                Some(&Value::List(vec![Value::Table(
                    [("x".to_string(), Value::Int(1))].into_iter().collect()
                )]))
            );
        }
        other => panic!("{other:?}"),
    }
    // The host's own keys are not the plugin's.
    let own: toml::Table = toml::from_str("enabled = false\nread_only = true\nx = 1\n").unwrap();
    let s = settings_of(&own);
    assert!(s.get("enabled").is_none() && s.get("read_only").is_none());
    assert_eq!(s.get("x"), Some(&Value::Int(1)));
}

/// The syntect theme of the Markdown code blocks (`[ui.theme] code_theme`) is what the `code`
/// plugin is handed, unless its own table names one; a theme other than the default reaches it.
#[test]
fn a_ui_theme_other_than_the_default_reaches_the_code_plugin() {
    let mut cfg = Config::default();
    assert_eq!(
        effective_settings(&cfg, konoma_code::CODE_ID).str("code_theme", "?"),
        cfg.ui.theme.code_theme
    );
    cfg.ui.theme.code_theme = "Dracula".into();
    assert_eq!(
        effective_settings(&cfg, konoma_code::CODE_ID).str("code_theme", "?"),
        "Dracula"
    );
    // The plugin's own table wins.
    let mut t = toml::Table::new();
    t.insert("code_theme".into(), toml::Value::String("Nord".into()));
    cfg.plugins.insert(konoma_code::CODE_ID.into(), t);
    assert_eq!(
        effective_settings(&cfg, konoma_code::CODE_ID).str("code_theme", "?"),
        "Nord"
    );
    // Other plugins are not handed it.
    assert!(effective_settings(&Config::default(), konoma_table::CSV_ID)
        .get("code_theme")
        .is_none());
}

/// The registry is shared per set of switched-off ids: the order of the ids, a repeated id and a
/// host-internal id do not make another registry.
#[test]
fn the_shared_registry_is_one_per_set_of_disabled_ids() {
    let ids = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let ab = shared_registry(&ids(&["csv", "code"]));
    let ba = shared_registry(&ids(&["code", "csv"]));
    assert!(Arc::ptr_eq(&ab, &ba), "order does not matter");
    let dup = shared_registry(&ids(&["csv", "code", "csv"]));
    assert!(Arc::ptr_eq(&ab, &dup), "a repeat does not matter");
    let with_internal =
        shared_registry(&ids(&["code", "text", "csv", "command", "can-not-preview"]));
    assert!(
        Arc::ptr_eq(&ab, &with_internal),
        "internal ids are not a difference"
    );
    let none = shared_registry(&[]);
    let only_internal = shared_registry(&ids(&["text", "command"]));
    assert!(Arc::ptr_eq(&none, &only_internal));
    assert!(!Arc::ptr_eq(&none, &ab));
    // And what is switched off is off, in the shared one.
    assert!(!ab.is_enabled("csv") && !ab.is_enabled("code"));
    assert!(none.is_enabled("csv") && none.is_enabled("code"));
}

/// The host-internal pseudo plugins cannot be switched off, whatever the list says.
#[test]
fn the_internal_plugins_stay_on_when_the_list_names_them() {
    let ids = ["text", "command", "can-not-preview", "csv"].map(String::from);
    let r = build_registry(&ids);
    for id in [TEXT_ID, COMMAND_ID, UNSUPPORTED_ID] {
        assert!(r.is_enabled(id), "{id}");
    }
    assert!(!r.is_enabled("csv"), "an ordinary plugin is switched off");
}

/// What a plugin declares as the default of a setting is what the host hands it when the user has
/// written nothing: the declaration, the old `[ui]` default and the plugin's own fallback are one
/// value (a setting the host does not fill is the plugin's own fallback, checked in its crate).
#[test]
fn the_declared_default_of_every_setting_is_what_the_host_hands_over() {
    let cfg = Config::default();
    let mut checked = 0;
    for m in standard_manifests() {
        let handed = effective_settings(&cfg, &m.id);
        for decl in &m.settings {
            if let Some(v) = handed.get(&decl.name) {
                assert_eq!(
                    v, &decl.default,
                    "{}: `{}` is declared {:?} but handed {:?}",
                    m.id, decl.name, decl.default, v
                );
                checked += 1;
            }
        }
    }
    assert!(checked >= 20, "checked {checked} settings");
}
