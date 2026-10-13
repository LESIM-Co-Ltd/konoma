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

// ---- the rest of the plugin-table audit ---------------------------------------------------------

/// "A spreadsheet" is a table of a plugin that **converts** the file: a CSV or an archive listing
/// is a table too and a Word document is a converting plugin too, and neither is one.
#[test]
fn only_a_table_that_converts_is_a_spreadsheet() {
    let cfg = Config::default();
    let kind = |name: &str| cfg.resolve_preview(Path::new(&format!("/nonexistent/{name}")));
    assert!(kind("a.xlsx").is_spreadsheet());
    assert!(kind("a.ods").is_spreadsheet());
    // Tables that are read as they are.
    assert!(kind("a.csv").profile().table);
    assert!(!kind("a.csv").is_spreadsheet());
    assert!(kind("a.zip").profile().table);
    assert!(!kind("a.zip").is_spreadsheet());
    // Converts, but is not a table.
    assert!(kind("a.docx").profile().converts);
    assert!(!kind("a.docx").profile().table);
    assert!(!kind("a.docx").is_spreadsheet());
    // Neither.
    assert!(!kind("a.rs").is_spreadsheet());
}

/// `enabled = false` is a complaint only for the host-internal pieces: an ordinary plugin may be
/// switched off without a word, and a host-internal one is told it cannot be.
#[test]
fn switching_an_ordinary_plugin_off_is_not_a_problem() {
    let lang = crate::i18n::Lang::En;
    let tables = |id: &str| {
        let mut t = toml::Table::new();
        t.insert("enabled".into(), toml::Value::Boolean(false));
        let mut all = PluginTables::new();
        all.insert(id.to_string(), t);
        all
    };
    for id in ["csv", "archive", "code", "markdown", "pdf"] {
        let problems = table_problems(&tables(id), lang);
        assert!(problems.is_empty(), "{id}: {problems:?}");
    }
    for id in [TEXT_ID, COMMAND_ID] {
        let problems = table_problems(&tables(id), lang);
        assert_eq!(problems.len(), 1, "{id}: {problems:?}");
        assert!(problems[0].contains("cannot be disabled"), "{problems:?}");
    }
    // `enabled = true` on an internal piece is fine.
    let mut t = toml::Table::new();
    t.insert("enabled".into(), toml::Value::Boolean(true));
    let mut all = PluginTables::new();
    all.insert(TEXT_ID.to_string(), t);
    assert!(table_problems(&all, lang).is_empty());
}

// ---- the actions a plugin declares (`plugins/acts.rs`) -------------------------------------------

mod declared_acts {
    use super::super::acts::{keypress_of, PluginKeys, PluginNote};
    use crate::keymap::KeyPress;
    use crossterm::event::KeyCode;
    use konoma_plugin::keys::{KeyCode as PKeyCode, KeySpec};
    use konoma_plugin::prelude::*;
    use konoma_plugin::registry::{PluginOrigin, Registry};
    use konoma_plugin::Act;
    use std::collections::HashMap;
    use std::sync::Arc;

    /// A plugin with actions in each key context and one in none, and two it names `w` and `x`.
    struct Declares;

    impl Plugin for Declares {
        fn manifest(&self) -> Manifest {
            Manifest::new("declares", "1")
                .shows(ViewKind::Text)
                .action(Act::new("anywhere").key("w").label("a").help("a help"))
                .action(
                    Act::new("in-table")
                        .context("table")
                        .label("t")
                        .help("t help"),
                )
                .action(
                    Act::new("in-image")
                        .context("image")
                        .label("i")
                        .help("i help"),
                )
                .action(
                    Act::new("in-doc")
                        .context("document")
                        .label("d")
                        .help("d help"),
                )
                .action(
                    Act::new("in-text")
                        .context("text")
                        .label("x")
                        .help("x help"),
                )
                .action(Act::new("second").key("x").label("s").help("s help"))
        }
        fn open(&self, _: Input, _: &OpenCx) -> Result<Box<dyn Session>, Failure> {
            Err(Failure::unsupported("not opened"))
        }
    }

    fn build(entries: &[(&str, &str)]) -> PluginKeys {
        let mut r = Registry::new();
        r.register(Arc::new(Declares), PluginOrigin::User).unwrap();
        let mut tables = HashMap::new();
        if !entries.is_empty() {
            tables.insert(
                "declares".to_string(),
                entries
                    .iter()
                    .map(|(k, a)| (k.to_string(), a.to_string()))
                    .collect::<HashMap<_, _>>(),
            );
        }
        PluginKeys::build(&r, &mut tables)
    }

    fn key_of(k: &PluginKeys, act: &str) -> Option<KeyPress> {
        k.acts_of("declares")
            .iter()
            .find(|b| b.act.act() == act)
            .and_then(|b| b.key)
    }

    /// An action keeps the key context it was declared in, and the rest of its declaration.
    #[test]
    fn an_action_keeps_the_context_it_was_declared_in() {
        let k = build(&[]);
        let ctx = |act: &str| {
            k.acts_of("declares")
                .iter()
                .find(|b| b.act.act() == act)
                .unwrap_or_else(|| panic!("{act}"))
                .context
                .clone()
        };
        assert_eq!(ctx("anywhere"), None);
        assert_eq!(ctx("in-table").as_deref(), Some("table"));
        assert_eq!(ctx("in-image").as_deref(), Some("image"));
        assert_eq!(ctx("in-doc").as_deref(), Some("document"));
        assert_eq!(ctx("in-text").as_deref(), Some("text"));
        assert_eq!(k.acts_of("declares").len(), 6, "in declaration order");
        assert_eq!(k.acts_of("declares")[0].act.act(), "anywhere");
        assert_eq!(k.acts_of("declares")[5].act.act(), "second");
    }

    /// `disabled` is another word for `noop`: it takes the key from the action that has it by
    /// default, and the action stays.
    #[test]
    fn disabled_takes_a_key_away_like_noop() {
        for word in ["noop", "disabled"] {
            let k = build(&[("w", word)]);
            assert_eq!(key_of(&k, "anywhere"), None, "{word}");
            assert_eq!(
                key_of(&k, "second"),
                Some(KeyPress::ch('x')),
                "{word}: only that key"
            );
            assert_eq!(k.acts_of("declares").len(), 6, "{word}");
            assert!(k.notes().is_empty(), "{word}: {:?}", k.notes());
        }
    }

    /// A key that is not a key is said so even where the entry only takes a key away.
    #[test]
    fn a_noop_on_something_that_is_not_a_key_is_told() {
        for word in ["noop", "disabled"] {
            let k = build(&[("ab", word)]);
            let told = k.notes().iter().any(|n| {
                matches!(n, PluginNote::BadKey { key, why, .. } if key == "ab" && why == "not a key")
            });
            assert!(told, "{word}: {:?}", k.notes());
            assert_eq!(
                key_of(&k, "anywhere"),
                Some(KeyPress::ch('w')),
                "{word}: nothing taken"
            );
        }
    }

    /// One key per action: the first key (in key order) is kept, the second is refused with its
    /// reason, and the action keeps the first.
    #[test]
    fn a_second_key_for_one_action_is_refused_and_the_first_is_kept() {
        let k = build(&[("W", "anywhere"), ("w", "anywhere")]);
        assert_eq!(key_of(&k, "anywhere"), Some(KeyPress::ch('W')));
        let refused: Vec<_> = k
            .notes()
            .iter()
            .filter_map(|n| match n {
                PluginNote::BadKey { key, why, .. } => Some((key.as_str(), why.as_str())),
                _ => None,
            })
            .collect();
        assert_eq!(refused.len(), 1, "{:?}", k.notes());
        assert_eq!(refused[0].0, "w");
        assert!(refused[0].1.contains("one key per action"), "{refused:?}");
    }

    /// Every key the plugin API has is the host key of the same name.
    #[test]
    fn every_plugin_key_code_is_its_host_key() {
        let all = [
            (PKeyCode::Char('q'), KeyCode::Char('q')),
            (PKeyCode::Enter, KeyCode::Enter),
            (PKeyCode::Tab, KeyCode::Tab),
            (PKeyCode::BackTab, KeyCode::BackTab),
            (PKeyCode::Esc, KeyCode::Esc),
            (PKeyCode::Backspace, KeyCode::Backspace),
            (PKeyCode::Delete, KeyCode::Delete),
            (PKeyCode::Up, KeyCode::Up),
            (PKeyCode::Down, KeyCode::Down),
            (PKeyCode::Left, KeyCode::Left),
            (PKeyCode::Right, KeyCode::Right),
            (PKeyCode::Home, KeyCode::Home),
            (PKeyCode::End, KeyCode::End),
            (PKeyCode::PageUp, KeyCode::PageUp),
            (PKeyCode::PageDown, KeyCode::PageDown),
            (PKeyCode::F(3), KeyCode::F(3)),
        ];
        for (p, h) in all {
            assert_eq!(keypress_of(KeySpec::key(p)), KeyPress::key(h), "{p:?}");
        }
        assert_eq!(keypress_of(KeySpec::ctrl('r')), KeyPress::ctrl_ch('r'));
    }
}

// ---- `konoma --plugins` -------------------------------------------------------------------------

mod listing_table {
    use super::*;
    use crate::plugins::plugins_table;
    use unicode_width::UnicodeWidthStr;

    fn cfg_of(text: &str) -> Config {
        toml::from_str::<Config>(text).unwrap()
    }

    fn rule_row(builtin: &str) -> Rule {
        Rule {
            glob: Some("*.zzz".into()),
            builtin: Some(builtin.into()),
            ..Rule::default()
        }
    }

    /// The columns of the first row whose assignment is `glob *.zzz` (a user rule's).
    fn user_row(out: &str) -> Vec<String> {
        let line = out
            .lines()
            .find(|l| l.starts_with("glob *.zzz"))
            .unwrap_or_else(|| panic!("no row:\n{out}"));
        // `glob *.zzz` has a space inside: the rest is single words but the last column.
        let rest = line.strip_prefix("glob *.zzz").unwrap();
        let mut cols: Vec<String> = vec!["glob *.zzz".into()];
        cols.extend(rest.split_whitespace().map(String::from));
        cols
    }

    /// Whether a user rule names a plugin the host has: the STANDARD column. A host piece
    /// (`text`, `command`) and a standard plugin say yes, a name nobody registered says no.
    #[test]
    fn a_rules_row_says_whether_its_plugin_is_one_the_host_has() {
        for (builtin, plugin, standard) in [
            ("markdown", "markdown", "yes"),
            ("text", "text", "yes"),
            ("no-such-plugin", "can-not-preview", "yes"),
        ] {
            let mut cfg = Config::default();
            cfg.preview.rules = vec![rule_row(builtin)];
            let cols = user_row(&plugins_table(&cfg));
            assert_eq!(cols[1], plugin, "{builtin}");
            assert_eq!(cols[2], standard, "{builtin}: STANDARD");
        }
        // A command rule is the host's `command` piece.
        let mut cfg = Config::default();
        cfg.preview.rules = vec![Rule {
            glob: Some("*.zzz".into()),
            command: Some("cat {path}".into()),
            ..Rule::default()
        }];
        let cols = user_row(&plugins_table(&cfg));
        assert_eq!((cols[1].as_str(), cols[2].as_str()), ("command", "yes"));
    }

    /// A rule for `document` covers Word and presentations: it is enabled while either is, and the
    /// row says so.
    #[test]
    fn the_document_rule_is_enabled_while_either_of_its_plugins_is() {
        for (off, enabled) in [
            ("", "yes"),
            ("[plugins.document]\nenabled = false\n", "yes"),
            ("[plugins.presentation]\nenabled = false\n", "yes"),
            (
                "[plugins.document]\nenabled = false\n[plugins.presentation]\nenabled = false\n",
                "no",
            ),
        ] {
            let mut cfg = cfg_of(off);
            cfg.preview.rules = vec![rule_row("document")];
            let cols = user_row(&plugins_table(&cfg));
            assert_eq!(cols[1], "document/presentation", "{off:?}");
            assert_eq!(cols[2], "yes", "{off:?}: STANDARD");
            assert_eq!(cols[3], enabled, "{off:?}: ENABLED");
        }
    }

    /// A rule for one plugin is enabled as that plugin is.
    #[test]
    fn another_rules_row_is_enabled_as_its_plugin_is() {
        let mut cfg = cfg_of("[plugins.pdf]\nenabled = false\n");
        cfg.preview.rules = vec![rule_row("pdf")];
        assert_eq!(user_row(&plugins_table(&cfg))[3], "no");
        let mut cfg = Config::default();
        cfg.preview.rules = vec![rule_row("pdf")];
        assert_eq!(user_row(&plugins_table(&cfg))[3], "yes");
    }

    /// A plugin a user added is not a standard one: its rows say `no` in STANDARD and the standard
    /// plugins say `yes`.
    #[test]
    fn a_user_plugin_is_not_marked_standard() {
        use super::super::test_plugins::{marker_config, register, Hexdump, Variant};
        use std::sync::Arc;
        let marker = register("listing_user_plugin", Arc::new(Hexdump(Variant::default())));
        let out = plugins_table(&marker_config(&marker));
        let row = |glob: &str| -> Vec<String> {
            out.lines()
                .find(|l| l.starts_with(glob))
                .unwrap_or_else(|| panic!("no {glob}:\n{out}"))
                .split_whitespace()
                .map(String::from)
                .collect()
        };
        let user = row("glob *.bin");
        assert_eq!(&user[2..], ["hexdump", "no", "yes", "built-in"], "{user:?}");
        let standard = row("glob *.md");
        assert_eq!(
            &standard[2..],
            ["markdown", "yes", "yes", "built-in"],
            "{standard:?}"
        );
    }

    /// The columns are as wide as the widest cell **on the screen** (a wide character takes two
    /// columns, three bytes), a narrower cell is padded to that, and the last column is not padded.
    #[test]
    fn columns_are_measured_and_padded_by_display_width() {
        // 20 wide characters: 40 columns, 60 bytes. 50 ASCII letters: 50 columns, 50 bytes. By
        // columns the ASCII one is the widest; by bytes the other would be.
        let cjk = "資".repeat(20);
        let ascii = "x".repeat(50);
        let mut cfg = Config::default();
        cfg.preview.rules = ["short", cjk.as_str(), ascii.as_str()]
            .into_iter()
            .map(|g| Rule {
                glob: Some(g.into()),
                command: Some("cat {path}".into()),
                ..Rule::default()
            })
            .collect();
        let out = plugins_table(&cfg);
        let lines: Vec<&str> = out.lines().collect();
        let head = lines
            .iter()
            .position(|l| l.starts_with("ASSIGNMENT"))
            .unwrap();
        let start = |l: &str, word: &str| UnicodeWidthStr::width(&l[..l.find(word).unwrap()]);
        // Every row's second column starts where the header's does, and that is two spaces
        // after the widest first cell, measured on the screen.
        let second = start(lines[head], "PLUGIN");
        let mut widest = 0;
        for l in &lines[head..] {
            if l.is_empty() {
                break;
            }
            let end = l
                .char_indices()
                .map(|(i, _)| i)
                .find(|&i| UnicodeWidthStr::width(&l[..i]) == second);
            let cell = l[..end.unwrap_or(l.len())].trim_end();
            widest = widest.max(UnicodeWidthStr::width(cell));
        }
        assert_eq!(
            second,
            widest + 2,
            "no wider than the widest cell on the screen"
        );
        for row in 1..=3 {
            let l = lines[head + row];
            assert_eq!(start(l, "command"), second, "{l:?}");
        }
        assert!(lines[head + 2].contains(&cjk));
        // The last column is not padded: no line ends in a space.
        for l in &lines[head..head + 4] {
            assert_eq!(l.trim_end(), *l, "{l:?}");
        }
    }
}
