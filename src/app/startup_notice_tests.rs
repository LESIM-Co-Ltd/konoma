//! The message on the footer at start (`App::startup_flash`): the old `[ui]` names that moved to
//! `[plugins.<id>]` (named once, and not when the new name is used), the keys a `[plugins.<id>]`
//! table may not carry, and the order and the fit of everything the footer says when konoma
//! starts (what is wrong first, the guidance last, the rest counted).

use super::*;
use crate::i18n::Lang;
use crate::plugins::{effective_settings, moved_settings, MOVED_UI_SETTINGS};
use crate::test_support::unique_tmp;

fn cfg_from(text: &str) -> Config {
    let mut cfg = toml::from_str::<Config>(text).unwrap();
    cfg.renamed_settings = Config::old_setting_names(text, &cfg);
    cfg
}

fn app_with(cfg: Config, lang: Lang) -> (crate::test_support::TmpDir, App) {
    let dir = unique_tmp("konoma_startup_notice");
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = App::new(dir.to_path_buf(), cfg).unwrap();
    app.lang = lang;
    (dir, app)
}

// ---- the old names ---------------------------------------------------------------------------

/// Every `[ui]` setting that moved is named with where it goes; the ones that stayed are not.
#[test]
fn the_old_ui_names_are_named_with_their_new_places() {
    let text = "[ui]\nmd_alerts = false\nmermaid = \"text\"\nsvg_max_px = 900\nwrap = false\nicons = true\n";
    let cfg = cfg_from(text);
    assert_eq!(
        cfg.renamed_settings,
        vec![
            (
                "[ui] md_alerts".to_string(),
                "[plugins.markdown] alerts".to_string()
            ),
            (
                "[ui] mermaid".to_string(),
                "[plugins.mermaid] mode".to_string()
            ),
            (
                "[ui] svg_max_px".to_string(),
                "[plugins.svg] max_px".to_string()
            ),
        ]
    );
}

/// The new name written, or no old name at all: nothing to say. (Both written: the new one wins
/// and the user has moved on.)
#[test]
fn the_new_names_and_the_defaults_say_nothing() {
    assert!(cfg_from("").renamed_settings.is_empty());
    assert!(cfg_from("[ui]\nwrap = false\n").renamed_settings.is_empty());
    assert!(
        cfg_from("[plugins.markdown]\nalerts = false\n[plugins.mermaid]\nmode = \"text\"\n")
            .renamed_settings
            .is_empty()
    );
    assert!(
        cfg_from("[ui]\nmd_alerts = false\n[plugins.markdown]\nalerts = true\n")
            .renamed_settings
            .is_empty(),
        "the new name is already used"
    );
    // One of two moved: only the one still on its old name.
    let one =
        cfg_from("[ui]\nmd_alerts = false\nmd_emoji = false\n[plugins.markdown]\nalerts = false\n");
    assert_eq!(one.renamed_settings.len(), 1);
    assert_eq!(one.renamed_settings[0].0, "[ui] md_emoji");
}

/// The list the message is made from is the list the plugins read: each old name has the new name
/// filled in by `effective_settings` (an old `[ui]` value still works), and the plugin declares
/// the new name (writing it never earns the unknown-key warning).
#[test]
fn every_moved_name_is_read_by_the_plugin_and_declared_by_it() {
    let cfg = Config::default();
    let registry = crate::plugins::shared_registry(&[]);
    for (old, id, new) in MOVED_UI_SETTINGS {
        assert!(
            effective_settings(&cfg, id).get(new).is_some(),
            "[ui] {old} must be read as [plugins.{id}] {new}"
        );
        let manifest = registry
            .manifest(id)
            .unwrap_or_else(|| panic!("no plugin {id}"));
        assert!(
            manifest.settings.iter().any(|d| d.name == *new),
            "[plugins.{id}] must declare {new}"
        );
    }
}

/// What S2-12 lists, by name, so a rename that is dropped from the list (or added to the config
/// without it) is noticed here.
#[test]
fn the_list_is_the_one_the_design_names() {
    let mut got: Vec<String> = MOVED_UI_SETTINGS
        .iter()
        .map(|(old, id, new)| format!("{old} -> {id}.{new}"))
        .collect();
    got.sort();
    let mut want: Vec<String> = [
        "md_task_states -> markdown.task_states",
        "md_autolink -> markdown.autolink",
        "md_alerts -> markdown.alerts",
        "md_emoji -> markdown.emoji",
        "md_frontmatter -> markdown.frontmatter",
        "md_footnotes -> markdown.footnotes",
        "md_inline_html -> markdown.inline_html",
        "md_details -> markdown.details",
        "md_table_align -> markdown.table_align",
        "md_image_align -> markdown.image_align",
        "mermaid -> mermaid.mode",
        "mermaid_theme -> mermaid.theme",
        "mermaid_curve -> mermaid.curve",
        "mermaid_routing -> mermaid.routing",
        "mermaid_rows -> mermaid.rows",
        "math -> math.mode",
        "math_color -> math.color",
        "csv_rainbow -> csv.rainbow",
        "syntax_highlight -> code.syntax_highlight",
        "svg_max_px -> svg.max_px",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    want.sort();
    assert_eq!(got, want);
    // The display settings stay in `[ui]`.
    for stays in [
        "wrap",
        "line_numbers",
        "git_gutter",
        "tab_width",
        "image_render_scale",
    ] {
        assert!(MOVED_UI_SETTINGS.iter().all(|(old, _, _)| *old != stays));
    }
    // A table that is not `[ui]` is not an old name.
    let plugins = std::collections::BTreeMap::new();
    let not_ui: toml::Table = toml::from_str("[other]\nmd_alerts = false\n").unwrap();
    assert!(moved_settings(&not_ui, &plugins).is_empty());
}

/// The guidance is in the startup message once, in the user's language, and absent for a config
/// that uses the new names.
#[test]
fn the_startup_message_names_the_old_names_once() {
    let text = "[ui]\nmd_alerts = false\nmd_emoji = false\n";
    let (_d, app) = app_with(cfg_from(text), Lang::En);
    let flash = app.startup_flash(None, 400).expect("a note");
    assert_eq!(
        flash,
        "config: [ui] md_alerts is an old name — write [plugins.markdown] alerts instead (+1 more)"
    );
    assert_eq!(flash.matches("old name").count(), 1);
    let (_d, app) = app_with(cfg_from(text), Lang::Jp);
    let flash = app.startup_flash(None, 400).unwrap();
    assert!(
        flash.starts_with("設定: [ui] md_alerts は古い名前です"),
        "{flash}"
    );
    assert!(flash.contains("[plugins.markdown] alerts"), "{flash}");
    assert!(flash.ends_with("(ほか 1 件)"), "{flash}");
    let (_d, app) = app_with(
        cfg_from("[plugins.markdown]\nalerts = false\nemoji = false\n"),
        Lang::En,
    );
    assert_eq!(
        app.startup_flash(None, 400),
        None,
        "the new names are quiet"
    );
}

// ---- the keys a plugin table may not carry ------------------------------------------------------

/// `[plugins.markdown] foo = 1`: said once at start, with the plugin and the key, and ignored.
#[test]
fn an_undeclared_plugin_setting_is_warned_about() {
    let cfg = cfg_from("[plugins.markdown]\nfoo = 1\nalerts = false\n");
    let en = cfg.plugin_report(Lang::En).expect("a warning");
    assert_eq!(
        en,
        "plugins: unknown setting `foo` in [plugins.markdown] (ignored)"
    );
    let ja = cfg.plugin_report(Lang::Jp).unwrap();
    assert_eq!(ja, "plugins: [plugins.markdown] の未知の設定 `foo`(無視)");
    // The warning is among the things the startup message leads with.
    let (_d, app) = app_with(cfg, Lang::En);
    let flash = app.startup_flash(None, 400).unwrap();
    assert!(flash.contains("unknown setting `foo`"), "{flash}");
}

/// Every key a plugin declares is accepted, `enabled` is the host's own, and a plugin that
/// declares nothing takes only `enabled`.
#[test]
fn declared_settings_and_enabled_are_not_warned_about() {
    let registry = crate::plugins::shared_registry(&[]);
    for id in registry.ids() {
        let manifest = registry.manifest(id).unwrap();
        let mut table = toml::Table::new();
        table.insert("enabled".into(), toml::Value::Boolean(true));
        for d in &manifest.settings {
            table.insert(d.name.clone(), toml::Value::Boolean(true));
        }
        let mut tables = crate::plugins::PluginTables::new();
        tables.insert(id.to_string(), table);
        let problems = crate::plugins::table_problems(&tables, Lang::En);
        assert!(problems.is_empty(), "[plugins.{id}]: {problems:?}");
    }
    // One undeclared key beside the declared ones: exactly one warning, naming it.
    let cfg = cfg_from("[plugins.csv]\nrainbow = false\nrainbo = true\n[plugins.pdf]\nzoom = 2\n");
    let report = cfg.plugin_report(Lang::En).unwrap();
    assert!(
        report.contains("unknown setting `rainbo` in [plugins.csv]"),
        "{report}"
    );
    assert!(
        report.contains("unknown setting `zoom` in [plugins.pdf]"),
        "{report}"
    );
    assert!(!report.contains("`rainbow`"), "{report}");
}

/// The keys the host fills in for a plugin are all keys that plugin declares: a setting that
/// `effective_settings` supplies but the manifest does not list would be warned about when a user
/// wrote it.
#[test]
fn what_the_host_supplies_to_a_plugin_is_declared_by_it() {
    let cfg = Config::default();
    let registry = crate::plugins::shared_registry(&[]);
    for id in registry.ids() {
        let manifest = registry.manifest(id).unwrap();
        for (key, _) in effective_settings(&cfg, id).iter() {
            // The theme of code blocks is handed over as a value, not as a setting to write.
            if id == "code" && key == "code_theme" {
                continue;
            }
            assert!(
                manifest.settings.iter().any(|d| d.name == *key),
                "{id} is handed `{key}` but does not declare it"
            );
        }
    }
}

// ---- the order and the fit of the whole message -------------------------------------------------

fn everything() -> (crate::test_support::TmpDir, App) {
    let text = "[ui]\nmd_alerts = false\n[plugins.markdown]\nfoo = 1\n";
    let mut cfg = cfg_from(text);
    cfg.keys.surfaces.insert(
        "tree".into(),
        [("t".to_string(), "refresh".to_string())].into(),
    );
    cfg.keys.surfaces.insert(
        "preview_text".into(),
        [("Z".to_string(), "refresh".to_string())].into(),
    );
    cfg.keys.surfaces.insert(
        "nonsense".into(),
        [("Z".to_string(), "refresh".to_string())].into(),
    );
    app_with(cfg, Lang::En)
}

/// Everything wrong comes before any guidance, whichever part of the config it came from; the
/// config that could not be read leads.
#[test]
fn what_is_wrong_comes_first_and_the_guidance_last() {
    let (_d, app) = everything();
    let flash = app
        .startup_flash(Some("config error (using defaults): demo".into()), 4000)
        .unwrap();
    let at = |needle: &str| {
        flash
            .find(needle)
            .unwrap_or_else(|| panic!("{needle} in {flash}"))
    };
    let (cfg_err, conflict, invalid, plugins, key_old, setting_old) = (
        at("config error"),
        at("key conflict"),
        at("invalid key setting"),
        at("unknown setting `foo`"),
        at("[keys.preview_text] is an old name"),
        at("[ui] md_alerts is an old name"),
    );
    assert!(cfg_err < conflict, "{flash}");
    assert!(conflict < invalid, "{flash}");
    assert!(invalid < plugins, "{flash}");
    assert!(
        plugins < key_old,
        "the warnings come before the guidance: {flash}"
    );
    assert!(key_old < setting_old, "{flash}");
    assert!(
        !flash.contains("more)"),
        "everything fits at this width: {flash}"
    );
    assert!(!flash.contains('\n'));
}

/// When not everything fits, the front stays and the rest is counted; the guidance is what goes
/// first, and what is cut is never counted wrongly.
#[test]
fn what_does_not_fit_is_counted_not_cut() {
    use unicode_width::UnicodeWidthStr;
    let (_d, app) = everything();
    let err = "config error (using defaults): demo";
    let total = 5; // the config error, the key problems, the plugin warning, two pieces of guidance
    let mut last_hidden = 0;
    for width in [4000, 400, 300, 200, 150, 100, 40] {
        let flash = app.startup_flash(Some(err.into()), width).unwrap();
        assert!(
            flash.starts_with(err),
            "the config error always leads: {flash}"
        );
        let hidden = flash
            .rsplit_once(" (+")
            .and_then(|(_, t)| t.strip_suffix(" more)"))
            .map(|n| n.parse::<usize>().expect("a count"))
            .unwrap_or(0);
        assert!(hidden < total, "{flash}");
        assert!(
            hidden >= last_hidden,
            "a narrower footer never shows more: {width} {flash}"
        );
        last_hidden = hidden;
        if hidden > 0 && total - hidden > 1 {
            assert!(flash.width() <= width, "{width}: {flash}");
        }
        // The guidance goes before any warning does.
        if flash.contains("unknown setting `foo`") {
            assert!(flash.contains("key conflict"), "{flash}");
        }
        if flash.contains("old name") {
            assert!(flash.contains("unknown setting `foo`"), "{flash}");
        }
    }
    assert!(last_hidden >= 1, "the narrowest footer counted something");
}

/// Japanese counts the same way.
#[test]
fn the_count_of_what_is_left_out_is_in_the_users_language() {
    let (_d, mut app) = everything();
    app.lang = Lang::Jp;
    let narrow = app.startup_flash(None, 60).unwrap();
    assert!(narrow.contains("(ほか "), "{narrow}");
}

/// One notice wider than the footer is still shown (the footer clips it); a message that fits is
/// whole; nothing to say is nothing.
#[test]
fn the_edges_of_fitting_the_message() {
    use crate::app::fit_notices;
    assert_eq!(fit_notices(Vec::new(), 80, Lang::En), None);
    assert_eq!(
        fit_notices(vec!["a".into(), "b".into()], 80, Lang::En).as_deref(),
        Some("a / b")
    );
    let wide = "x".repeat(200);
    let one = fit_notices(vec![wide.clone(), "b".into()], 40, Lang::En).unwrap();
    assert!(
        one.starts_with(&wide) && one.ends_with("(+1 more)"),
        "{one}"
    );
    // Exactly the width fits; one column less drops the last notice.
    assert_eq!(
        fit_notices(vec!["abc".into(), "def".into()], 9, Lang::En).as_deref(),
        Some("abc / def")
    );
    assert_eq!(
        fit_notices(vec!["abc".into(), "def".into()], 8, Lang::En).as_deref(),
        Some("abc (+1 more)")
    );
    // Wide characters count by columns, not chars.
    let cjk = fit_notices(vec!["日本語".into(), "文字".into()], 10, Lang::En).unwrap();
    assert_eq!(cjk, "日本語 (+1 more)");
}
