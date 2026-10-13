//! The standard plugins' declarations, and the way a file picks the plugin that shows it.
//!
//! Stage 2 of the plugin architecture (`docs/FEATURE-PLUGIN-ARCHITECTURE.md` §3-2, §4-4, §7-2).
//! This step only settles **who** shows a file: the decision is made by `konoma_plugin::Registry`
//! (user rules first, then the standard plugins in registration order, then plain text, then
//! "can not preview"), and the result is mapped onto today's `PreviewKind`. The plugins that are
//! wired to the display (`csv`, `archive`, `spreadsheet`, `code`, and the picture plugins `image`, `svg`, `pdf`, `video`) are opened by the host's Session mechanism
//! (`app/plugin_host.rs`) and show as `PreviewKind::Plugin`; the rest are still declarations only.

mod listing;
mod resolve;
#[cfg(test)]
mod tests;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock};

use konoma_plugin::registry::{PluginOrigin, Registry};
use konoma_plugin::t;
use konoma_plugin::{
    Bind, Failure, Input, LaunchMode, Launcher, Manifest, OpenCx, Plugin, SettingDecl, Settings,
    Verb, VerbDecl,
};

pub(crate) use listing::plugins_table;
pub(crate) use resolve::{resolve_kind, user_rules};

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

fn s(v: &str) -> konoma_plugin::Value {
    konoma_plugin::Value::Str(v.to_string())
}

fn b(v: bool) -> konoma_plugin::Value {
    konoma_plugin::Value::Bool(v)
}

/// `edit` opens the user's editor in the terminal; `open` hands the file to the OS default
/// application as a GUI program.
fn default_verbs(m: Manifest) -> Manifest {
    m.verb(
        VerbDecl::new(Verb::Edit)
            .launcher(Launcher::Editor)
            .mode(LaunchMode::Terminal)
            .line_token(),
    )
    .verb(
        VerbDecl::new(Verb::Open)
            .launcher(Launcher::OsDefault)
            .mode(LaunchMode::Gui),
    )
}

/// The chain `e` follows today for Office documents: the Microsoft app, then LibreOffice, then the
/// OS default. Both verbs use it (`e` keeps its behavior; `o` is the same chain).
fn office_verbs(m: Manifest, kind: crate::app::office_open::OfficeKind) -> Manifest {
    let chain = |verb: Verb| {
        VerbDecl::new(verb)
            .launcher(Launcher::MacApp {
                bundle_id: crate::app::office_open::microsoft_bundle_id(kind).to_string(),
            })
            .launcher(Launcher::MacApp {
                bundle_id: crate::app::office_open::LIBREOFFICE_BUNDLE_ID.to_string(),
            })
            .launcher(Launcher::Command(vec!["libreoffice".to_string()]))
            .launcher(Launcher::Command(vec!["soffice".to_string()]))
            .launcher(Launcher::OsDefault)
            .mode(LaunchMode::Gui)
    };
    m.verb(chain(Verb::Edit)).verb(chain(Verb::Open))
}

fn standard(id: &str, name: konoma_plugin::Tr) -> Manifest {
    Manifest::new(id, env!("CARGO_PKG_VERSION")).name(name)
}

/// The declarations of the standard plugins, in registration order. The order is the order of the
/// default `[[preview.rules]]` konoma had before plugins (svg before `image/*`, the mime rules for
/// images and videos in their old places), so a file is decided exactly as it was.
pub(crate) fn standard_manifests() -> Vec<Manifest> {
    use crate::app::office_open::OfficeKind;
    let md = standard("markdown", t!(en: "Markdown", ja: "Markdown"))
        .bind(Bind::files(["*.md"]))
        .bind(Bind::content("markdown"))
        .setting(SettingDecl::new(
            "task_states",
            konoma_plugin::Value::List(vec![s(" "), s("x")]),
            t!(en: "Checkbox states Space cycles through", ja: "Space で巡回するチェックボックスの状態"),
        ))
        .setting(SettingDecl::new(
            "autolink",
            b(true),
            t!(en: "Turn bare URLs into links", ja: "裸の URL をリンクにする"),
        ))
        .setting(SettingDecl::new(
            "alerts",
            b(true),
            t!(en: "Draw GitHub alerts (> [!NOTE])", ja: "GitHub のアラート(> [!NOTE])を描く"),
        ))
        .setting(SettingDecl::new(
            "emoji",
            b(true),
            t!(en: "Replace :shortcode: emoji", ja: ":shortcode: の絵文字を置き換える"),
        ))
        .setting(SettingDecl::new(
            "frontmatter",
            b(true),
            t!(en: "Show front matter as a block", ja: "front matter をブロックとして表示する"),
        ))
        .setting(SettingDecl::new(
            "footnotes",
            b(true),
            t!(en: "Draw footnotes", ja: "脚注を描く"),
        ))
        .setting(SettingDecl::new(
            "inline_html",
            b(true),
            t!(en: "Render inline HTML", ja: "インライン HTML を描く"),
        ))
        .setting(SettingDecl::new(
            "details",
            s("auto"),
            t!(en: "<details> folding: auto, open or closed", ja: "<details> の折りたたみ: auto・open・closed"),
        ))
        .setting(SettingDecl::new(
            "table_align",
            s("left"),
            t!(en: "Table alignment: left, center or right", ja: "表の寄せ: left・center・right"),
        ))
        .setting(SettingDecl::new(
            "image_align",
            s("center"),
            t!(en: "Image alignment: left, center or right", ja: "画像の寄せ: left・center・right"),
        ));
    let mermaid = konoma_mermaid::mermaid_manifest();
    let math = konoma_math::math_manifest();
    let svg = konoma_media::svg_manifest();
    let image = konoma_media::image_manifest();
    let csv = konoma_table::csv_manifest();
    let spreadsheet = konoma_office::spreadsheet_manifest();
    let document = office(
        standard("document", t!(en: "Word document", ja: "Word 文書")),
        OfficeKind::Word,
        "*.{docx,docm,dotx,dotm,odt,ott}",
    );
    let presentation = office(
        standard(
            "presentation",
            t!(en: "Presentation", ja: "プレゼンテーション"),
        ),
        OfficeKind::PowerPoint,
        "*.{pptx,pptm,ppsx,ppsm,potx,potm,odp,otp}",
    );
    let archive = konoma_table::archive_manifest();
    let code = konoma_code::code_manifest();
    let video = konoma_media::video_manifest();
    let pdf = konoma_media::pdf_manifest();

    let mut out: Vec<Manifest> = [
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
    ]
    .into_iter()
    .map(|m| {
        // The Office plugins already carry their own chain.
        if m.verbs.is_empty() {
            default_verbs(m)
        } else {
            m
        }
    })
    .collect();
    // `math` only takes the formulas inside Markdown, so it has no file to edit or open.
    out.insert(2, math);
    out
}

fn office(m: Manifest, kind: crate::app::office_open::OfficeKind, glob: &str) -> Manifest {
    office_verbs(m.bind(Bind::files([glob])), kind)
}

/// The host-internal pseudo plugins (see [`TEXT_ID`]).
fn internal_manifests() -> Vec<Manifest> {
    vec![
        standard(TEXT_ID, t!(en: "Plain text", ja: "テキスト")),
        standard(COMMAND_ID, t!(en: "External command", ja: "外部コマンド")),
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
        konoma_mermaid::MERMAID_ID => Arc::new(konoma_mermaid::MermaidPlugin),
        konoma_math::MATH_ID => Arc::new(konoma_math::MathPlugin),
        _ => Arc::new(Declared(m)),
    }
}

/// A registry of the standard plugins, with `disabled` ids switched off.
pub(crate) fn build_registry(disabled: &[String]) -> Registry {
    let mut r = Registry::new();
    for m in standard_manifests().into_iter().chain(internal_manifests()) {
        // The declarations are static; a refusal would be a bug that `tests` catches.
        let _ = r.register(plugin_of(m), PluginOrigin::Standard);
    }
    for id in disabled {
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

/// The plugin's own settings from its table (`enabled` is not one of them).
pub(crate) fn settings_of(table: &toml::Table) -> Settings {
    let mut out = Settings::new();
    for (k, v) in table {
        if k != "enabled" {
            out.set(k.clone(), value_of(v));
        }
    }
    out
}

fn value_of(v: &toml::Value) -> konoma_plugin::Value {
    use konoma_plugin::Value;
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

/// What a plugin receives as `OpenCx::settings`: its `[plugins.<id>]` table, with the old `[ui]`
/// items it replaces filled in under their new names (S2-12). The new name wins when both are set.
/// Each plugin that is wired to the display adds its old names here.
pub(crate) fn effective_settings(cfg: &crate::config::Config, id: &str) -> Settings {
    let mut settings = cfg.plugin_settings(id);
    let mut fill_missing = |key: &str, value: konoma_plugin::Value| {
        if settings.get(key).is_none() {
            settings.set(key, value);
        }
    };
    if id == konoma_table::CSV_ID {
        fill_missing("rainbow", b(cfg.ui.csv_rainbow));
    }
    if id == konoma_office::SPREADSHEET_ID {
        // A sheet's columns follow `[plugins.csv] rainbow` (and the old `[ui] csv_rainbow`) until
        // the sheet is given a value of its own.
        let csv = effective_settings(cfg, konoma_table::CSV_ID);
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
    match u16::try_from(raw) {
        Ok(0) | Err(_) => konoma_mermaid::DEFAULT_ROWS as u16,
        Ok(n) => n,
    }
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

/// Problems in `[plugins.<id>]` worth a startup message: an id nobody registered, `enabled` that
/// is not a boolean, an attempt to disable a host-internal piece.
pub(crate) fn table_problems(tables: &PluginTables, lang: crate::i18n::Lang) -> Vec<String> {
    use crate::i18n::Lang;
    let registry = shared_registry(&[]);
    let ids = registry.ids();
    let mut out = Vec::new();
    for (id, table) in tables {
        if !ids.contains(&id.as_str()) {
            out.push(match lang {
                Lang::Jp => format!("未知のプラグイン `{id}`(無視)"),
                Lang::En => format!("unknown plugin `{id}` (ignored)"),
            });
            continue;
        }
        match table.get("enabled") {
            None | Some(toml::Value::Boolean(_)) => {}
            Some(_) => out.push(match lang {
                Lang::Jp => format!("[plugins.{id}] の enabled は true か false"),
                Lang::En => format!("[plugins.{id}] enabled must be true or false"),
            }),
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
