//! The standard plugins' declarations, and the way a file picks the plugin that shows it.
//!
//! Stage 2 of the plugin architecture (`docs/FEATURE-PLUGIN-ARCHITECTURE.md` §3-2, §4-4, §7-2).
//! This step only settles **who** shows a file: the decision is made by `konoma_plugin::Registry`
//! (user rules first, then the standard plugins in registration order, then plain text, then
//! "can not preview"), and the result is mapped onto today's `PreviewKind`. The plugins that are
//! wired to the display (`csv`, `archive`, `spreadsheet`, `code`, `markdown`, the Office plugins and
//! the picture plugins `image`, `svg`, `pdf`, `video`) are opened by the host's Session mechanism
//! (`app/plugin_host.rs`) and show as `PreviewKind::Plugin`; the rest are still declarations only.

mod acts;
mod command;
pub(crate) mod external;
#[cfg(test)]
pub(crate) mod external_testkit;
mod listing;
mod profile;
#[cfg(test)]
mod program_parity_tests;
pub(crate) mod read_scope;
mod resolve;
#[cfg(test)]
mod settings_parity_tests;
#[cfg(test)]
mod survivor_tests;
#[cfg(test)]
pub(crate) mod test_plugins;
#[cfg(test)]
pub(crate) mod test_views;
#[cfg(test)]
mod tests;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock};

use konoma_plugin::registry::{PluginOrigin, Registry};
use konoma_plugin::t;
use konoma_plugin::{
    Failure, Input, LaunchMode, Launcher, Manifest, OpenCx, Plugin, Settings, Verb, VerbDecl,
};

#[cfg(test)]
pub(crate) use acts::PluginNote;
pub(crate) use acts::{ActBinding, PluginAct, PluginKeys};
pub(crate) use command::CommandPlugin;
pub(crate) use listing::plugins_table;
#[cfg(test)]
pub(crate) use profile::record as record_for_test;
pub(crate) use profile::{profile_of, register, shows_of, unavailable_of, Profile};
pub(crate) use resolve::{plugin_id_by_path, plugin_id_of, resolve_kind, user_rules};

/// Host-internal pseudo plugins. They own no file binding: a `[[preview.rules]]` entry that asks
/// for plain text, an external command, or an unknown renderer is aimed at one of them, so the
/// registry's "first user rule that matches wins" covers those rules too.
pub(crate) const TEXT_ID: &str = "text";
pub(crate) const COMMAND_ID: &str = "command";
pub(crate) const UNSUPPORTED_ID: &str = "can-not-preview";

/// Whether `id` is a host-internal pseudo plugin (it cannot be disabled).
pub(crate) fn is_internal(id: &str) -> bool {
    matches!(id, TEXT_ID | COMMAND_ID | UNSUPPORTED_ID)
}

/// A standard plugin that is declared but not wired to the display yet: `open` is never called by
/// the host in this step, so reaching it is a bug and says so.
struct Declared(Manifest);

impl Plugin for Declared {
    fn manifest(&self) -> Manifest {
        self.0.clone()
    }

    fn open(
        &self,
        _input: Input,
        _cx: &OpenCx,
    ) -> Result<Box<dyn konoma_plugin::Session>, Failure> {
        Err(Failure::unsupported(t!(
            en: "this plugin is declared but not opened through the plugin API yet",
            ja: "このプラグインは宣言のみで、まだプラグイン API では開かれません",
        )))
    }
}

fn standard(id: &str, name: konoma_plugin::Tr) -> Manifest {
    Manifest::new(id, env!("CARGO_PKG_VERSION")).name(name)
}

/// The declarations of the standard plugins, in registration order. The order is the order of the
/// default `[[preview.rules]]` konoma had before plugins (svg before `image/*`, the mime rules for
/// images and videos in their old places), so a file is decided exactly as it was.
pub(crate) fn standard_manifests() -> Vec<Manifest> {
    let md = konoma_markdown::markdown_manifest();
    let mermaid = konoma_mermaid::mermaid_manifest();
    let math = konoma_math::math_manifest();
    let svg = konoma_media::svg_manifest();
    let image = konoma_media::image_manifest();
    let csv = konoma_table::csv_manifest();
    let spreadsheet = konoma_office::spreadsheet_manifest();
    let document = konoma_office::document_manifest();
    let presentation = konoma_office::presentation_manifest();
    let archive = konoma_table::archive_manifest();
    let code = konoma_code::code_manifest();
    let video = konoma_media::video_manifest();
    let pdf = konoma_media::pdf_manifest();

    // Every one of them already carries its own `edit` / `open` chain.
    let mut out: Vec<Manifest> = vec![
        md,
        mermaid,
        svg,
        image,
        csv,
        spreadsheet,
        document,
        presentation,
        archive,
        code,
        video,
        pdf,
    ];
    // `math` only takes the formulas inside Markdown, so it has no file to edit or open.
    out.insert(2, math);
    out
}

/// The `open` verb of the plugin that shows `path`: the chain `o` walks. A file no plugin takes
/// (plain text, an unknown type) or a plugin that declares none opens with the OS default.
pub(crate) fn open_verb(
    rules: &[crate::config::Rule],
    disabled: &[String],
    path: &std::path::Path,
) -> VerbDecl {
    plugin_id_of(rules, disabled, path)
        .and_then(|id| {
            with_registry(disabled, |r| {
                r.manifest(&id)
                    .and_then(|m| m.verbs.iter().find(|v| v.verb == Verb::Open).cloned())
            })
        })
        .unwrap_or_else(|| {
            VerbDecl::new(Verb::Open)
                .launcher(Launcher::OsDefault)
                .mode(LaunchMode::Gui)
        })
}

/// The `verb` the plugin that takes `path` declares, decided from the path alone (nothing is
/// read from the file, so the footer can ask every frame). `None` = no plugin takes it, or it
/// declares no such verb.
pub(crate) fn declared_verb_by_path(
    rules: &[crate::config::Rule],
    disabled: &[String],
    path: &std::path::Path,
    verb: Verb,
) -> Option<VerbDecl> {
    let id = plugin_id_by_path(rules, disabled, path)?;
    with_registry(disabled, |r| {
        r.manifest(&id)
            .and_then(|m| m.verbs.iter().find(|v| v.verb == verb).cloned())
    })
}

/// The host-internal pseudo plugins (see [`TEXT_ID`]).
fn internal_manifests() -> Vec<Manifest> {
    vec![
        standard(TEXT_ID, t!(en: "Plain text", ja: "テキスト")),
        command::manifest(),
        standard(UNSUPPORTED_ID, t!(en: "Not supported", ja: "未対応")),
    ]
}

/// The implementation behind a standard manifest: the plugins wired to the display are real, the
/// others are declarations only.
fn plugin_of(m: Manifest) -> Arc<dyn Plugin> {
    match m.id.as_str() {
        konoma_table::CSV_ID => Arc::new(konoma_table::CsvPlugin),
        konoma_table::ARCHIVE_ID => Arc::new(konoma_table::ArchivePlugin),
        konoma_media::IMAGE_ID => Arc::new(konoma_media::ImagePlugin),
        konoma_media::SVG_ID => Arc::new(konoma_media::SvgPlugin),
        konoma_media::PDF_ID => Arc::new(konoma_media::PdfPlugin),
        konoma_media::VIDEO_ID => Arc::new(konoma_media::VideoPlugin),
        konoma_code::CODE_ID => Arc::new(konoma_code::CodePlugin),
        konoma_office::SPREADSHEET_ID => Arc::new(konoma_office::SpreadsheetPlugin),
        konoma_office::DOCUMENT_ID => Arc::new(konoma_office::DocumentPlugin),
        konoma_office::PRESENTATION_ID => Arc::new(konoma_office::PresentationPlugin),
        konoma_mermaid::MERMAID_ID => Arc::new(konoma_mermaid::MermaidPlugin),
        konoma_math::MATH_ID => Arc::new(konoma_math::MathPlugin),
        konoma_markdown::MARKDOWN_ID => Arc::new(konoma_markdown::MarkdownPlugin),
        COMMAND_ID => Arc::new(CommandPlugin::unbound()),
        _ => Arc::new(Declared(m)),
    }
}

/// A registry of the standard plugins, with `disabled` ids switched off.
pub(crate) fn build_registry(disabled: &[String]) -> Registry {
    let mut r = Registry::new();
    for m in standard_manifests().into_iter().chain(internal_manifests()) {
        // The declarations are static; a refusal would be a bug that `tests` catches.
        let _ = register(&mut r, plugin_of(m), PluginOrigin::Standard);
    }
    for id in disabled {
        // Programs of their own (`[plugins.<id>] command`, `--plugin`): the marker stands for the
        // set `external::install` loaded; they are registered the way a user's plugin is.
        if let Some(token) = external::token_of(id) {
            for plugin in external::plugins_of(token) {
                // A refused id (taken, malformed) was already reported by `install`.
                let _ = register(&mut r, plugin, PluginOrigin::User);
            }
            continue;
        }
        // Tests add plugins of their own through a name in the `disabled` list (the registry is
        // shared per distinct list, so a test's plugin never reaches another test): see
        // `test_plugins`.
        #[cfg(test)]
        if let Some(plugin) = test_plugins::lookup(id) {
            let _ = register(&mut r, plugin, PluginOrigin::User);
            continue;
        }
        if !is_internal(id) {
            r.set_enabled(id, false);
        }
    }
    r
}

/// The registry with `disabled` ids switched off, built **once** per distinct set of disabled ids
/// for the process and shared (`Arc`): the deciding of a file, the opening of a plugin and the
/// media-diff worker all use the same one, and nothing builds a registry per call.
pub(crate) fn shared_registry(disabled: &[String]) -> Arc<Registry> {
    static CACHE: OnceLock<Mutex<BTreeMap<Vec<String>, Arc<Registry>>>> = OnceLock::new();
    // Host-internal pseudo plugins cannot be switched off: they do not make a different registry.
    let mut key: Vec<String> = disabled
        .iter()
        .filter(|id| !is_internal(id))
        .cloned()
        .collect();
    key.sort();
    key.dedup();
    let cache = CACHE.get_or_init(Mutex::default);
    // A poisoned lock only means another thread panicked while holding it; the map is still sound.
    let mut map = cache.lock().unwrap_or_else(|e| e.into_inner());
    Arc::clone(
        map.entry(key.clone())
            .or_insert_with(|| Arc::new(build_registry(&key))),
    )
}

/// Run `f` with the registry that has `disabled` ids switched off (see [`shared_registry`]).
pub(crate) fn with_registry<T>(disabled: &[String], f: impl FnOnce(&Registry) -> T) -> T {
    f(&shared_registry(disabled))
}

/// `[plugins.<id>]` as read from the config: the raw tables. `enabled` is read by the host; every
/// other key is the plugin's own setting.
pub(crate) type PluginTables = BTreeMap<String, toml::Table>;

/// The ids whose table says `enabled = false`.
pub(crate) fn disabled_ids(tables: &PluginTables) -> Vec<String> {
    tables
        .iter()
        .filter(|(_, t)| matches!(t.get("enabled"), Some(toml::Value::Boolean(false))))
        .map(|(id, _)| id.clone())
        .collect()
}

/// The plugin's own settings from its table (`enabled` is not one of them): the reading the
/// standalone app shares (`konoma_plugin::config`).
pub(crate) use konoma_plugin::config::settings_of;

fn value_of(v: &toml::Value) -> konoma_plugin::Value {
    konoma_plugin::config::value_of(v)
}

/// The `[ui]` settings that moved to `[plugins.<id>]` (S2-12 of the design): the one list both
/// hosts read (`konoma_plugin::config::MOVED_UI_SETTINGS`). [`effective_settings`] still reads the
/// old keys; this list is what the startup message names, so a user learns where to write them.
pub(crate) const MOVED_UI_SETTINGS: &[(&str, &str, &str)] =
    konoma_plugin::config::MOVED_UI_SETTINGS;

/// The old `[ui]` names a config still uses, as `(what is written, what to write instead)`. `ui`
/// is the config's raw `[ui]` table. A name is not reported once the new one is also set (the new
/// one wins; the user has moved on).
pub(crate) fn moved_settings(ui: &toml::Table, plugins: &PluginTables) -> Vec<(String, String)> {
    MOVED_UI_SETTINGS
        .iter()
        .filter(|(old, id, new)| {
            ui.contains_key(*old) && !plugins.get(*id).is_some_and(|t| t.contains_key(*new))
        })
        .map(|(old, id, new)| (format!("[ui] {old}"), format!("[plugins.{id}] {new}")))
        .collect()
}

/// What a plugin receives as `OpenCx::settings`: its `[plugins.<id>]` table, completed from the old
/// `[ui]` items it replaces (S2-12). Decided by `konoma_plugin::config::plugin_settings`, the one
/// function the standalone app calls too; this is only konoma's config handed to it.
pub(crate) fn effective_settings(cfg: &crate::config::Config, id: &str) -> Settings {
    konoma_plugin::config::plugin_settings(&cfg.plugins, &cfg.ui.moved(), id)
}

/// `[plugins.<id>] <key>` when it is a string, otherwise the old `[ui]` value `old`. The same rule
/// [`effective_settings`] applies, without building the settings: the draw path asks these on
/// every frame.
fn plugin_str<'a>(cfg: &'a crate::config::Config, id: &str, key: &str, old: &'a str) -> &'a str {
    match cfg.plugins.get(id).and_then(|t| t.get(key)) {
        Some(toml::Value::String(v)) => v,
        _ => old,
    }
}

/// Whether the mermaid diagrams are drawn as text (`[plugins.mermaid] mode`, or `[ui] mermaid`).
pub(crate) fn mermaid_mode_is_text(cfg: &crate::config::Config) -> bool {
    plugin_str(cfg, konoma_mermaid::MERMAID_ID, "mode", &cfg.ui.mermaid) == "text"
}

/// Whether formulas are kept as text (`[plugins.math] mode`, or `[ui] math`).
pub(crate) fn math_mode_is_text(cfg: &crate::config::Config) -> bool {
    plugin_str(cfg, konoma_math::MATH_ID, "mode", &cfg.ui.math) == "text"
}

/// The largest height of an inline diagram, in rows (`[plugins.mermaid] rows`, or
/// `[ui] mermaid_rows`). `0`, a negative number or one past `u16` is the default, 24.
pub(crate) fn mermaid_rows(cfg: &crate::config::Config) -> u16 {
    let raw = match cfg
        .plugins
        .get(konoma_mermaid::MERMAID_ID)
        .and_then(|t| t.get("rows"))
    {
        Some(toml::Value::Integer(n)) => *n,
        _ => i64::from(cfg.ui.mermaid_rows),
    };
    konoma_plugin::document_pictures::mermaid_rows_cap(raw)
}

/// Whether code and config files are colored: `[plugins.code] syntax_highlight`, or the old
/// `[ui] syntax_highlight` when the new name is not set (the same rule [`effective_settings`]
/// applies, without building the settings: the text window asks this on every frame).
pub(crate) fn syntax_highlight_on(cfg: &crate::config::Config) -> bool {
    match cfg
        .plugins
        .get(konoma_code::CODE_ID)
        .and_then(|t| t.get("syntax_highlight"))
    {
        Some(toml::Value::Boolean(on)) => *on,
        _ => cfg.ui.syntax_highlight,
    }
}

/// The type a setting's declared default has, named for the message that says a value of another
/// type is not used.
#[derive(Clone, Copy)]
pub(crate) enum ValueType {
    Bool,
    Int,
    Float,
    Str,
    List,
    Table,
}

impl ValueType {
    fn of(v: &konoma_plugin::Value) -> ValueType {
        use konoma_plugin::Value;
        match v {
            Value::Bool(_) => ValueType::Bool,
            Value::Int(_) => ValueType::Int,
            Value::Float(_) => ValueType::Float,
            Value::Str(_) => ValueType::Str,
            Value::List(_) => ValueType::List,
            Value::Table(_) => ValueType::Table,
        }
    }

    fn en(self) -> &'static str {
        match self {
            ValueType::Bool => "true or false",
            ValueType::Int => "an integer",
            ValueType::Float => "a number",
            ValueType::Str => "a string",
            ValueType::List => "a list",
            ValueType::Table => "a table",
        }
    }

    fn ja(self) -> &'static str {
        match self {
            ValueType::Bool => "真偽値(true か false)",
            ValueType::Int => "整数",
            ValueType::Float => "数値",
            ValueType::Str => "文字列",
            ValueType::List => "リスト",
            ValueType::Table => "テーブル",
        }
    }
}

/// The type the declared default asks for when `given` is of another one (`None` = it fits). A
/// whole number is accepted where a number is declared, as `Settings::float` does.
pub(crate) fn value_type_mismatch(
    default: &konoma_plugin::Value,
    given: &konoma_plugin::Value,
) -> Option<ValueType> {
    use konoma_plugin::Value;
    let fits = matches!(
        (default, given),
        (Value::Bool(_), Value::Bool(_))
            | (Value::Int(_), Value::Int(_))
            | (Value::Float(_), Value::Float(_) | Value::Int(_))
            | (Value::Str(_), Value::Str(_))
            | (Value::List(_), Value::List(_))
            | (Value::Table(_), Value::Table(_))
    );
    (!fits).then(|| ValueType::of(default))
}

pub(crate) use konoma_plugin::config::HOST_PLUGIN_KEYS;
// Only the tests ask the host's-own-key rule directly now; the reading itself is shared.
#[cfg(test)]
pub(crate) use konoma_plugin::config::is_host_key;

/// Problems in `[plugins.<id>]` worth a startup message: an id nobody registered, `enabled` that
/// is not a boolean, an attempt to disable a host-internal piece.
pub(crate) fn table_problems(tables: &PluginTables, lang: crate::i18n::Lang) -> Vec<String> {
    use crate::i18n::Lang;
    let registry = shared_registry(&[]);
    let ids = registry.ids();
    let mut out = Vec::new();
    for (id, table) in tables {
        #[cfg(test)]
        if test_plugins::lookup(id).is_some() {
            continue;
        }
        // A table with a `command` names a program of its own: its id is known to the program
        // (its manifest), not to the registry built here, and its other keys are the program's
        // settings, which only the program can check.
        let (external_spec, external_problems) = external::spec_of_table(id, table);
        let is_external = table.contains_key("command");
        if is_external {
            for p in external_problems {
                out.push(external::table_problem_message(id, p, lang));
            }
            let _ = external_spec;
            for key in ["enabled", "read_only"] {
                match table.get(key) {
                    None | Some(toml::Value::Boolean(_)) => {}
                    Some(_) => out.push(match lang {
                        Lang::Jp => format!("[plugins.{id}] の {key} は true か false"),
                        Lang::En => format!("[plugins.{id}] {key} must be true or false"),
                    }),
                }
            }
            if let Some(v) = table.get("read_dirs") {
                out.extend(read_dirs_messages(id, v, lang));
            }
            continue;
        }
        if !ids.contains(&id.as_str()) {
            out.push(match lang {
                Lang::Jp => format!("未知のプラグイン `{id}`(無視)"),
                Lang::En => format!("unknown plugin `{id}` (ignored)"),
            });
            continue;
        }
        // A key the plugin did not declare is a typo or a name from another version: said once at
        // startup, and ignored (`enabled` is the host's own key).
        if let Some(manifest) = registry.manifest(id) {
            for (key, value) in table {
                // The host's own keys are checked below, not by a declaration.
                if HOST_PLUGIN_KEYS.contains(&key.as_str()) {
                    continue;
                }
                let Some(decl) = manifest.settings.iter().find(|d| d.name == *key) else {
                    out.push(match lang {
                        Lang::Jp => format!("[plugins.{id}] の未知の設定 `{key}`(無視)"),
                        Lang::En => format!("unknown setting `{key}` in [plugins.{id}] (ignored)"),
                    });
                    continue;
                };
                // A value of another type than the declared default is not read: the plugin
                // falls back to the default. Said once at startup, not left silent.
                if let Some(want) = value_type_mismatch(&decl.default, &value_of(value)) {
                    out.push(match lang {
                        Lang::Jp => {
                            format!("[plugins.{id}] {key} は{}が必要(既定を使う)", want.ja())
                        }
                        Lang::En => format!(
                            "[plugins.{id}] {key} must be {} (using the default)",
                            want.en()
                        ),
                    });
                }
            }
        }
        if let Some(v) = table.get("read_dirs") {
            out.extend(read_dirs_messages(id, v, lang));
        }
        for key in ["enabled", "read_only"] {
            match table.get(key) {
                None | Some(toml::Value::Boolean(_)) => {}
                Some(_) => out.push(match lang {
                    Lang::Jp => format!("[plugins.{id}] の {key} は true か false"),
                    Lang::En => format!("[plugins.{id}] {key} must be true or false"),
                }),
            }
        }
        if is_internal(id) && matches!(table.get("enabled"), Some(toml::Value::Boolean(false))) {
            out.push(match lang {
                Lang::Jp => format!("`{id}` は止められません"),
                Lang::En => format!("`{id}` cannot be disabled"),
            });
        }
    }
    out
}

/// The startup messages for one plugin's `read_dirs`.
fn read_dirs_messages(id: &str, value: &toml::Value, lang: crate::i18n::Lang) -> Vec<String> {
    use crate::i18n::Lang;
    use read_scope::ReadDirsProblem as P;
    let home = read_scope::home_dir();
    read_scope::parse_read_dirs(value, home.as_deref())
        .problems
        .into_iter()
        .map(|p| match (p, lang) {
            (P::NotAList, Lang::Jp) => format!("[plugins.{id}] の read_dirs はフォルダの一覧"),
            (P::NotAList, Lang::En) => {
                format!("[plugins.{id}] read_dirs must be a list of folders")
            }
            (P::NotAString, Lang::Jp) => {
                format!("[plugins.{id}] の read_dirs に文字列でない要素(無視)")
            }
            (P::NotAString, Lang::En) => {
                format!("[plugins.{id}] read_dirs has an entry that is not a string (ignored)")
            }
            (P::Relative(e), Lang::Jp) => {
                format!("[plugins.{id}] の read_dirs `{e}` は絶対パスか ~ で始める(無視)")
            }
            (P::Relative(e), Lang::En) => {
                format!("[plugins.{id}] read_dirs `{e}` must be absolute or start with ~ (ignored)")
            }
            (P::Missing(e), Lang::Jp) => {
                format!("[plugins.{id}] の read_dirs `{e}` が見つかりません(無視)")
            }
            (P::Missing(e), Lang::En) => {
                format!("[plugins.{id}] read_dirs `{e}` does not exist (ignored)")
            }
        })
        .collect()
}

/// Test probe: does the registry's matching take `rule` for `path` when the content reads as
/// `mime`? (A stand-in plugin with no bindings of its own receives the rule, so only the rule's
/// own glob and mime decide.)
#[cfg(test)]
pub(crate) fn rule_hits(
    rule: &crate::config::Rule,
    path: &std::path::Path,
    mime: Option<&str>,
) -> bool {
    use konoma_plugin::registry::{FileFacts, Resolution};
    struct Probe;
    impl Plugin for Probe {
        fn manifest(&self) -> Manifest {
            Manifest::new("probe", "0")
        }
        fn open(
            &self,
            _input: Input,
            _cx: &OpenCx,
        ) -> Result<Box<dyn konoma_plugin::Session>, Failure> {
            Err(Failure::unsupported(t!(en: "probe")))
        }
    }
    let mut r = Registry::new();
    let _ = r.register(Arc::new(Probe), PluginOrigin::Standard);
    let rules: Vec<_> = [
        rule.glob.clone().map(konoma_plugin::Target::Glob),
        rule.mime.clone().map(konoma_plugin::Target::Mime),
    ]
    .into_iter()
    .flatten()
    .map(|target| konoma_plugin::registry::UserRule {
        target,
        plugin: "probe".to_string(),
        hint: None,
    })
    .collect();
    let sniff = || mime.map(str::to_string);
    let text = || false;
    let facts = FileFacts {
        path,
        sniff_mime: &sniff,
        is_text: &text,
    };
    matches!(r.resolve_file(&rules, &facts), Resolution::Plugin { id, .. } if id == "probe")
}
