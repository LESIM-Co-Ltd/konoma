//! The library face of konoma: assemble the plugins a program links in, then run the application.
//!
//! ```no_run
//! fn main() -> anyhow::Result<()> {
//!     konoma::Host::builder().standard_plugins().run()
//! }
//! ```
//!
//! The `konoma` command is exactly that `main`. This is the whole public surface of the crate
//! (the rest is `pub(crate)`), so it is what a version promises not to break.

use std::collections::BTreeSet;
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use konoma_plugin::Plugin;

use crate::i18n::{tr, tr_format, Lang, Msg};
use crate::plugins::{self, statics::StaticSet};

/// Entry point of the library: [`Host::builder`].
pub struct Host;

impl Host {
    /// Start assembling the plugins of a program.
    pub fn builder() -> HostBuilder {
        HostBuilder::default()
    }
}

/// The plugins a program links in. Declarations are checked by [`HostBuilder::run`] after all
/// builder calls, so their order does not matter. Create the builder before creating plugins.
pub struct HostBuilder {
    startup: Startup,
    standard: bool,
    added: Vec<Arc<dyn Plugin>>,
    replaced: Vec<(String, Arc<dyn Plugin>)>,
}

pub(crate) struct Startup {
    pub original_exe: Option<std::path::PathBuf>,
    pub switched_from: Option<std::ffi::OsString>,
}

impl Default for HostBuilder {
    fn default() -> Self {
        crate::dispatch_internal_child();
        // Capture and remove handover markers before plugin factories or manifests can run.
        Self {
            startup: Startup {
                original_exe: crate::plugin_cli::switch::take_original_exe(),
                switched_from: crate::plugin_cli::switch::take_switched_from(),
            },
            standard: false,
            added: Vec::new(),
            replaced: Vec::new(),
        }
    }
}

impl HostBuilder {
    fn checked_id(plugin: &Arc<dyn Plugin>, lang: Lang) -> Result<String> {
        // Registration is the source of truth for all manifest refusals, including invalid globs.
        // A fresh registry checks one declaration; resolve handles the assembled set's taken ids.
        let mut validation = konoma_plugin::registry::Registry::new();
        let id = plugin.manifest().id;
        let result = validation.register(
            Arc::clone(plugin),
            konoma_plugin::registry::PluginOrigin::User,
        );
        if lang == Lang::En {
            result.with_context(|| format!("{id:?}"))?;
        } else {
            // Registry validation remains the source of truth. Its public why string is English;
            // render the typed rejection and offending data without reparsing that prose.
            result.map_err(|error| {
                use konoma_plugin::registry::RegistryError;
                anyhow::anyhow!(match error {
                    RegistryError::InvalidManifest { id, reason, .. } => format!(
                        "{}: {}",
                        tr_format(lang, Msg::PluginHostInvalidManifest, &[("id", &id)]),
                        manifest_reason_text(&reason, lang)
                    ),
                    RegistryError::InvalidGlob { id, glob } => tr_format(
                        lang,
                        Msg::PluginHostInvalidGlob,
                        &[("id", &id), ("glob", &glob)]
                    ),
                    RegistryError::DuplicateId(id) =>
                        tr(lang, Msg::PluginHostDuplicate).replace("{id}", &id),
                    other => tr_format(
                        lang,
                        Msg::PluginRegistryUnknown,
                        &[("reason", &other.to_string())]
                    ),
                })
            })?;
        }
        Ok(validation.ids()[0].to_owned())
    }

    /// Include the standard plugins (markdown, image, pdf, ...). Without this call only the
    /// plugins given to [`plugin`](Self::plugin) are linked in.
    pub fn standard_plugins(mut self) -> Self {
        self.standard = true;
        self
    }

    /// Add a plugin. Its id must not be one that is already taken (a standard plugin's, or one
    /// added before); `run` fails otherwise.
    ///
    /// Create the builder before creating plugins: it dispatches internal child modes, then
    /// consumes the startup environment while the process is still single-threaded.
    pub fn plugin(mut self, plugin: impl Plugin) -> Self {
        self.added.push(Arc::new(plugin));
        self
    }

    /// Add the plugins a plugin crate offers: its library's `plugins()` function, which is what
    /// `konoma plugin install` writes into the program it builds. Each one follows the rules of
    /// [`plugin`](Self::plugin) (its id must be free; `run` fails otherwise).
    ///
    /// The generated `main` creates the builder before calling the crate's `plugins()`, so startup
    /// internal child dispatch and environment changes finish before that plugin code runs.
    pub fn plugins(mut self, plugins: Vec<Box<dyn Plugin>>) -> Self {
        self.added.extend(plugins.into_iter().map(Arc::from));
        self
    }

    /// Put `plugin` in the place of the standard plugin `id` (it keeps the place in the order that
    /// decides which plugin shows a file). `run` fails when there is no such plugin.
    /// All replaced ids are freed before new ids are checked, so another replacement may reuse
    /// one of them in either call order. Duplicate targets and conflicting new ids are errors.
    pub fn replace(mut self, id: &str, plugin: impl Plugin) -> Self {
        self.replaced.push((id.to_string(), Arc::new(plugin)));
        self
    }

    /// Check the linked declarations using the language supplied by the verifying parent,
    /// without reading configuration or starting the application.
    fn resolve(&self) -> Result<StaticSet> {
        self.resolve_lang(crate::plugin_cli::verification_language())
    }

    fn resolve_lang(&self, lang: Lang) -> Result<StaticSet> {
        // The ids a registry would hold before the program's own plugins: the standard ones (when
        // asked for) and the host-internal ones, which nobody can take or replace.
        let mut taken: BTreeSet<String> = [
            plugins::TEXT_ID,
            plugins::COMMAND_ID,
            plugins::UNSUPPORTED_ID,
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let mut standard_ids = BTreeSet::new();
        if self.standard {
            for m in plugins::standard_manifests() {
                standard_ids.insert(m.id.clone());
                taken.insert(m.id);
            }
        }
        let mut targets = BTreeSet::new();
        for (target, _) in &self.replaced {
            if !targets.insert(target.clone()) {
                bail!(tr(lang, Msg::PluginHostReplacedTwice).replace("{id}", target));
            }
            if !standard_ids.contains(target) {
                bail!(tr(lang, Msg::PluginHostMissingTarget).replace("{id}", target));
            }
            taken.remove(target);
        }
        // All old ids leave together, before any new id is checked. A replacement may take
        // another replaced slot's old id, regardless of the order the builder calls arrived.
        let mut replaced = std::collections::BTreeMap::new();
        for (target, plugin) in &self.replaced {
            let id = Self::checked_id(plugin, lang)?;
            if !taken.insert(id.clone()) {
                bail!(tr(lang, Msg::PluginHostTaken).replace("{id}", &id));
            }
            replaced.insert(target.clone(), Arc::clone(plugin));
        }
        for plugin in &self.added {
            let id = Self::checked_id(plugin, lang)?;
            if !taken.insert(id.clone()) {
                bail!(tr(lang, Msg::PluginHostTaken).replace("{id}", &id));
            }
        }
        Ok(StaticSet {
            standard: self.standard,
            added: self.added.clone(),
            replaced,
        })
    }

    /// Start konoma with these plugins, as the `konoma` command does with its arguments and the
    /// terminal. Returns when konoma exits; a bad plugin set is an error before anything else
    /// happens (no terminal is touched).
    pub fn run(self) -> Result<()> {
        let set = self.resolve()?;
        let startup = self.startup;
        // Declared after the check above, which touches nothing, so a program (or a test) that
        // only learns its plugin set is wrong never becomes "the application". It is done here and
        // not in the caller's `main` so that every program that runs konoma -- the `konoma`
        // command and the ones `konoma plugin install` builds alike -- keeps the promise that the
        // crates' test-only seams are off in the application, and so that nobody can forget it.
        // It stays ahead of everything else that matters (the drawing child, arguments, the
        // configuration, the terminal).
        konoma_base::process::mark_app_main();
        if plugins::statics::install(set).is_err() {
            bail!(tr(
                crate::plugin_cli::verification_language(),
                Msg::PluginHostStarted
            ));
        }
        crate::run_application(startup)
    }
}

fn manifest_reason_text(reason: &konoma_plugin::registry::ManifestReason, lang: Lang) -> String {
    use konoma_plugin::registry::{ManifestCount as C, ManifestName as N, ManifestReason as R};
    match reason {
        R::InvalidWatchPath { index } => tr_format(
            lang,
            Msg::PluginReasonWatchPath,
            &[("index", &index.to_string())],
        ),
        R::InvalidMarker { index, limit } => tr_format(
            lang,
            Msg::PluginReasonMarker,
            &[("index", &index.to_string()), ("max", &limit.to_string())],
        ),
        R::InvalidScreenKey { key } => tr_format(lang, Msg::PluginReasonScreenKey, &[("key", key)]),
        R::InvalidConfirmationKey { action, key } => tr_format(
            lang,
            Msg::PluginReasonConfirmKey,
            &[("action", action), ("key", key)],
        ),
        R::DuplicateConfirmationKey { action, key } => tr_format(
            lang,
            Msg::PluginReasonConfirmDuplicate,
            &[("action", action), ("key", key)],
        ),
        R::EmptyConfirmation { action } => {
            tr_format(lang, Msg::PluginReasonConfirmEmpty, &[("action", action)])
        }
        R::ChoiceValueTooLong { action, limit } => tr_format(
            lang,
            Msg::PluginReasonChoiceValue,
            &[("action", action), ("max", &limit.to_string())],
        ),
        R::ChoiceLabelTooLong { action, limit } => tr_format(
            lang,
            Msg::PluginReasonChoiceLabel,
            &[("action", action), ("max", &limit.to_string())],
        ),
        R::DestructiveWithoutConfirmation { action } => {
            tr_format(lang, Msg::PluginReasonDestructive, &[("action", action)])
        }
        R::DuplicateScreen { screen } => tr_format(
            lang,
            Msg::PluginReasonDuplicateScreen,
            &[("screen", screen)],
        ),
        R::DuplicateScreenKey { key } => {
            tr_format(lang, Msg::PluginReasonDuplicateScreenKey, &[("key", key)])
        }
        R::UnknownScreen { action, screen } => tr_format(
            lang,
            Msg::PluginReasonUnknownScreen,
            &[("action", action), ("screen", screen)],
        ),
        R::UnknownRowKind {
            action,
            screen,
            kind,
        } => tr_format(
            lang,
            Msg::PluginReasonUnknownRowKind,
            &[("action", action), ("screen", screen), ("kind", kind)],
        ),
        R::NoDetectionMarkers => tr(lang, Msg::PluginReasonNoMarkers).into(),
        R::InvalidName { kind, name, limit } => {
            let kind = tr(
                lang,
                match kind {
                    N::PluginId => Msg::PluginNameId,
                    N::ActionId => Msg::PluginNameAction,
                    N::ActionScreen => Msg::PluginNameActionScreen,
                    N::SettingName => Msg::PluginNameSetting,
                    N::ScreenName => Msg::PluginNameScreen,
                    N::WorkGroup => Msg::PluginNameWorkGroup,
                    N::RowKind => Msg::PluginNameRowKind,
                    _ => Msg::PluginNameUnknown,
                },
            );
            tr_format(
                lang,
                Msg::PluginReasonName,
                &[("kind", kind), ("name", name), ("max", &limit.to_string())],
            )
        }
        R::TooMany { kind, count, limit } => {
            let kind = tr(
                lang,
                match kind {
                    C::Actions => Msg::PluginCountActions,
                    C::Settings => Msg::PluginCountSettings,
                    C::Binds => Msg::PluginCountBinds,
                    C::Screens => Msg::PluginCountScreens,
                    C::Targets => Msg::PluginCountTargets,
                    C::Markers => Msg::PluginCountMarkers,
                    C::RowKinds => Msg::PluginCountRowKinds,
                    C::WatchPaths => Msg::PluginCountWatchPaths,
                    C::Choices => Msg::PluginCountChoices,
                    _ => Msg::PluginCountUnknown,
                },
            );
            tr_format(
                lang,
                Msg::PluginReasonCount,
                &[
                    ("kind", kind),
                    ("n", &count.to_string()),
                    ("max", &limit.to_string()),
                ],
            )
        }
        R::TargetTooLong { bind, bytes, limit } => tr_format(
            lang,
            Msg::PluginReasonTargetLong,
            &[
                ("bind", &bind.to_string()),
                ("bytes", &bytes.to_string()),
                ("max", &limit.to_string()),
            ],
        ),
        R::TargetUnclean { bind } => tr_format(
            lang,
            Msg::PluginReasonTargetUnclean,
            &[("bind", &bind.to_string())],
        ),
        R::DefaultTooMany { limit } => tr_format(
            lang,
            Msg::PluginReasonDefaultCount,
            &[("max", &limit.to_string())],
        ),
        R::DefaultKeyTooLong { limit } => tr_format(
            lang,
            Msg::PluginReasonDefaultKeyLong,
            &[("max", &limit.to_string())],
        ),
        R::DefaultKeyUnclean { limit } => tr_format(
            lang,
            Msg::PluginReasonDefaultKeyUnclean,
            &[("max", &limit.to_string())],
        ),
        R::DefaultTooDeep { limit } => tr_format(
            lang,
            Msg::PluginReasonDefaultDepth,
            &[("max", &limit.to_string())],
        ),
        other => tr_format(
            lang,
            Msg::PluginReasonUnknown,
            &[("reason", &other.to_string())],
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use konoma_plugin::prelude::*;

    #[test]
    fn verification_manifest_errors_follow_the_parent_language() {
        for lang in [Lang::Jp, Lang::En] {
            for builder in [
                Host::builder().plugin(Fake("Bad ID")),
                Host::builder().standard_plugins().plugin(Fake("markdown")),
                Host::builder()
                    .standard_plugins()
                    .replace("absent", Fake("valid")),
                Host::builder()
                    .standard_plugins()
                    .replace("markdown", Fake("first"))
                    .replace("markdown", Fake("second")),
            ] {
                let error = builder.resolve_lang(lang).err().unwrap();
                if lang == Lang::Jp {
                    crate::test_support::plugin_language::assert_japanese_plugin_prose(
                        "verified manifest",
                        &format!("{error:#}"),
                    );
                }
            }
        }
    }

    #[test]
    fn every_manifest_reason_keeps_the_sanitized_id_and_reason_in_both_languages() {
        use konoma_plugin::registry::{PluginOrigin, Registry, RegistryError};
        use konoma_plugin::settings::Value;
        struct Decl(Manifest);
        impl Plugin for Decl {
            fn manifest(&self) -> Manifest {
                self.0.clone()
            }
            fn open(&self, _: Input, _: &OpenCx) -> Result<Box<dyn Session>, Failure> {
                Err(Failure::unsupported("fixture"))
            }
        }
        let mut cases = vec![Manifest::new("Bad ID", "1")];
        for m in [
            Manifest::new("p", "1").action(Act::new("Bad ID")),
            Manifest::new("p", "1").action(Act::new("a").screen("Bad ID")),
            Manifest::new("p", "1").setting(SettingDecl::new(
                "Bad ID",
                Value::Bool(false),
                Tr::new(""),
            )),
            Manifest::new("p", "1").screen("Bad ID"),
            Manifest::new("p", "1").work_group("Bad ID"),
        ] {
            cases.push(m);
        }
        let mut m = Manifest::new("p", "1");
        m.actions = vec![Act::new("a"); 257];
        cases.push(m);
        let mut m = Manifest::new("p", "1");
        m.settings = vec![SettingDecl::new("k", Value::Bool(false), Tr::new("")); 257];
        cases.push(m);
        let mut m = Manifest::new("p", "1");
        m.binds = vec![Bind::files(["a"]); 1025];
        cases.push(m);
        let mut m = Manifest::new("p", "1");
        m.screens = vec!["s".into(); 65];
        cases.push(m);
        cases.push(Manifest::new("p", "1").bind(Bind::files(vec!["a"; 513])));
        cases.push(Manifest::new("p", "1").bind(Bind::files(["a".repeat(1025)])));
        cases.push(Manifest::new("p", "1").bind(Bind::files(["a\n"])));
        for value in [
            Value::List(vec![Value::Bool(false); 1024]),
            Value::Table(std::collections::BTreeMap::from([(
                "a".repeat(257),
                Value::Bool(false),
            )])),
            Value::Table(std::collections::BTreeMap::from([(
                "a\n".into(),
                Value::Bool(false),
            )])),
            (0..9).fold(Value::Bool(false), |v, _| Value::List(vec![v])),
        ] {
            cases.push(Manifest::new("p", "1").setting(SettingDecl::new("k", value, Tr::new(""))));
        }
        let mut expected_japanese = [
            "プラグインの id",
            "操作の id",
            "操作の画面名",
            "設定名",
            "画面名",
            "作業グループ名",
        ]
        .map(|kind| format!("{kind} `?ad???` は名前として使えません（`[a-z0-9._-]`、1〜64文字）"))
        .to_vec();
        expected_japanese.extend([
            "操作が 257 件あります（最大 256 件）",
            "設定が 257 件あります（最大 256 件）",
            "関連付けが 1025 件あります（最大 1024 件）",
            "画面が 65 件あります（最大 64 件）",
            "1つの関連付けの対象が 513 件あります（最大 512 件）",
            "関連付け 0 の対象が長すぎます（1025 バイト、最大 1024 バイト）",
            "関連付け 0 の対象に制御文字があります",
            "設定の既定値の値が 1024 個を超えています",
            "設定の既定値のキーが長すぎます（最大 256 バイト、制御文字は使用できません）",
            "設定の既定値のキーに制御文字があります（最大 256 バイト、制御文字は使用できません）",
            "設定の既定値の入れ子が 8 段を超えています",
        ].map(str::to_owned));
        assert_eq!(cases.len(), expected_japanese.len());
        for (manifest, expected_japanese) in cases.into_iter().zip(expected_japanese) {
            let plugin: Arc<dyn Plugin> = Arc::new(Decl(manifest));
            let RegistryError::InvalidManifest {
                id, why, reason, ..
            } = Registry::new()
                .register(plugin.clone(), PluginOrigin::User)
                .unwrap_err()
            else {
                panic!("expected invalid manifest")
            };
            assert_eq!(why, reason.to_string());
            assert_eq!(manifest_reason_text(&reason, Lang::En), why);
            let english = format!(
                "{:#}",
                HostBuilder::checked_id(&plugin, Lang::En).unwrap_err()
            );
            assert!(
                english.ends_with(&format!("plugin `{id}` is refused: {why}")),
                "{english}"
            );
            let japanese = format!(
                "{:#}",
                HostBuilder::checked_id(&plugin, Lang::Jp).unwrap_err()
            );
            assert!(
                japanese.contains(&format!("プラグイン `{id}`")),
                "{japanese}"
            );
            let cause = manifest_reason_text(&reason, Lang::Jp);
            assert_eq!(cause, expected_japanese);
            assert!(japanese.ends_with(&cause), "{japanese}");
            assert_ne!(cause, why);
            crate::test_support::plugin_language::assert_japanese_plugin_prose(
                "manifest reason",
                &japanese,
            );
        }
    }

    struct Fake(&'static str);

    #[test]
    fn project_manifest_reasons_are_translated_in_both_languages() {
        use konoma_plugin::registry::{PluginOrigin, Registry, RegistryError};
        struct Decl(Manifest);
        impl Plugin for Decl {
            fn manifest(&self) -> Manifest {
                self.0.clone()
            }
            fn open(&self, _: Input, _: &OpenCx) -> Result<Box<dyn Session>, Failure> {
                Err(Failure::unsupported("fixture"))
            }
        }
        let project = || {
            ProjectDecl::new()
                .marker(DetectionMarker::new(".git", MarkerKind::Any))
                .screen(ProjectScreen::new("log", "Log").key('z').row_kind("commit"))
        };
        let m = || Manifest::new("p", "1").project(project());
        let cases = [
            (
                m().action(Act::new("delete").destructive()),
                "操作 `delete` は破壊的ですが確認の選択肢がありません",
            ),
            (
                m().action(
                    Act::new("delete")
                        .destructive()
                        .confirmation(Confirmation::new("Delete?", vec![])),
                ),
                "操作 `delete` は破壊的ですが確認の選択肢がありません",
            ),
            (m().screen("log"), "画面 `log` が重複しています"),
            (
                Manifest::new("p", "1")
                    .project(project().screen(ProjectScreen::new("log", "Other log"))),
                "画面 `log` が重複しています",
            ),
            (
                Manifest::new("p", "1")
                    .project(project().screen(ProjectScreen::new("other", "Other").key('z'))),
                "画面のキー `z` が重複しています",
            ),
            (
                m().action(
                    Act::new("go").project_context(ActContext::screen("absent", ["commit"])),
                ),
                "操作 `go` が未宣言の画面 `absent` を指定しています",
            ),
            (
                Manifest::new("p", "1")
                    .action(Act::new("go").project_context(ActContext::screen("log", ["commit"]))),
                "操作 `go` が未宣言の画面 `log` を指定しています",
            ),
            (
                m().action(Act::new("go").project_context(ActContext::screen("log", ["file"]))),
                "操作 `go` が画面 `log` の未宣言の行の種類 `file` を指定しています",
            ),
            (
                Manifest::new("p", "1").project(ProjectDecl::new()),
                "プロジェクトには検出目印を1つ以上宣言してください",
            ),
            (
                Manifest::new("p", "1")
                    .project(project().screen(ProjectScreen::new("bad", "Bad").row_kind("Bad ID"))),
                "行の種類 `?ad???` は名前として使えません（`[a-z0-9._-]`、1〜64文字）",
            ),
            (
                Manifest::new("p", "1")
                    .project(project().screen(ProjectScreen::new("Bad ID", "Bad screen"))),
                "画面名 `?ad???` は名前として使えません（`[a-z0-9._-]`、1〜64文字）",
            ),
            (
                {
                    let mut p = project();
                    p.screens = (0..64)
                        .map(|i| ProjectScreen::new(format!("s{i}"), "Screen"))
                        .collect();
                    Manifest::new("p", "1").screen("legacy").project(p)
                },
                "画面が 65 件あります（最大 64 件）",
            ),
            (
                {
                    let mut p = project();
                    p.markers = vec![DetectionMarker::new(".git", MarkerKind::Any); 513];
                    Manifest::new("p", "1").project(p)
                },
                "プロジェクトの検出目印が 513 件あります（最大 512 件）",
            ),
            (
                {
                    let mut p = project();
                    p.screens[0].row_kinds = vec!["commit".into(); 513];
                    Manifest::new("p", "1").project(p)
                },
                "画面内の行の種類が 513 件あります（最大 512 件）",
            ),
        ];
        for (manifest, japanese) in cases {
            let plugin: Arc<dyn Plugin> = Arc::new(Decl(manifest));
            let RegistryError::InvalidManifest { reason, why, .. } = Registry::new()
                .register(plugin.clone(), PluginOrigin::User)
                .unwrap_err()
            else {
                panic!("expected refusal")
            };
            assert_eq!(manifest_reason_text(&reason, Lang::En), why);
            assert_eq!(manifest_reason_text(&reason, Lang::Jp), japanese);
            for (lang, expected) in [(Lang::En, why.as_str()), (Lang::Jp, japanese)] {
                let error = format!("{:#}", HostBuilder::checked_id(&plugin, lang).unwrap_err());
                assert!(error.ends_with(expected), "{error}");
            }
        }

        let confirm = |choices| {
            Manifest::new("p", "1")
                .action(Act::new("go").confirmation(Confirmation::new("Go?", choices)))
        };
        let pick = |choice| {
            Manifest::new("p", "1")
                .action(Act::new("go").choice_input(ChoiceInput::new("Pick", vec![choice])))
        };
        let extra = [
            (Manifest::new("p", "1").project(project().watch("../out")),
             "project watch path 0 must be nonempty and relative without parent components",
             "プロジェクトの監視パス 0 は空でなく、親要素を含まない相対パスにしてください"),
            (Manifest::new("p", "1").project(ProjectDecl::new().marker(DetectionMarker::new("../out", MarkerKind::Any))),
             "project detection marker 0 must be one nonempty file name other than . or .. (at most 1024 bytes)",
             "プロジェクトの検出目印 0 は . と .. 以外の空でない単一のファイル名にしてください（最大 1024 バイト）"),
            (Manifest::new("p", "1").project(project().screen(ProjectScreen::new("other", "Other").key('q'))),
             "screen key `q` is a control character or reserved by the host",
             "画面のキー `q` は制御文字かホストの予約キーです"),
            (confirm(vec![ConfirmChoice::new('\n', "v", "V")]),
             "confirmation key `\\n` of action `go` is a control character or reserved by the host",
             "操作 `go` の確認キー `\\n` は制御文字かホストの予約キーです"),
            (confirm(vec![ConfirmChoice::new('y', "a", "A"), ConfirmChoice::new('y', "b", "B")]),
             "confirmation key `y` of action `go` is declared twice",
             "操作 `go` の確認キー `y` が重複しています"),
            (confirm(vec![]),
             "confirmation of action `go` must declare at least one choice",
             "操作 `go` の確認には選択肢を1つ以上宣言してください"),
            (pick(Choice::new("x".repeat(1025), "V")),
             "a choice value of action `go` exceeds 1024 bytes",
             "操作 `go` の選択肢の値が 1024 バイトを超えています"),
            (pick(Choice::new("v", Tr::new("V").ja("界".repeat(513)))),
             "a choice label of action `go` exceeds 512 characters",
             "操作 `go` の選択肢のラベルが 512 文字を超えています"),
            ({
                let mut p = project(); p.watch_paths = vec![".git/HEAD".into(); 513];
                Manifest::new("p", "1").project(p)
             },
             "513 project watch paths (at most 512)",
             "プロジェクトの監視パスが 513 件あります（最大 512 件）"),
            (Manifest::new("p", "1").action(Act::new("go").choice_input(ChoiceInput::new("Pick", vec![Choice::new("v", "V"); 65]))),
             "65 choices in a prompt (at most 64)",
             "入力画面の選択肢が 65 件あります（最大 64 件）"),
        ];
        for (manifest, english, japanese) in extra {
            let plugin: Arc<dyn Plugin> = Arc::new(Decl(manifest));
            let RegistryError::InvalidManifest { reason, why, .. } = Registry::new()
                .register(plugin.clone(), PluginOrigin::User)
                .unwrap_err()
            else {
                panic!("expected refusal")
            };
            assert_eq!(why, english);
            assert_eq!(manifest_reason_text(&reason, Lang::En), english);
            assert_eq!(manifest_reason_text(&reason, Lang::Jp), japanese);
            for (lang, expected) in [(Lang::En, english), (Lang::Jp, japanese)] {
                let error = format!("{:#}", HostBuilder::checked_id(&plugin, lang).unwrap_err());
                assert!(error.ends_with(expected), "{error}");
            }
        }
    }

    impl Plugin for Fake {
        fn manifest(&self) -> Manifest {
            Manifest::new(self.0, "0.0.1")
        }
        fn open(&self, _: Input, _: &OpenCx) -> Result<Box<dyn Session>, Failure> {
            Err(Failure::unsupported(t!(en: "none", ja: "なし")))
        }
    }

    fn err(b: HostBuilder) -> String {
        match b.resolve() {
            Ok(_) => panic!("expected an error"),
            Err(e) => e.to_string(),
        }
    }

    #[test]
    fn malformed_added_plugin_is_refused_before_startup() {
        let error = err(Host::builder().plugin(Fake("Bad ID")));
        assert!(error.contains("Bad ID"), "{error}");
    }

    #[test]
    fn malformed_replacement_is_refused_before_startup() {
        let error = err(Host::builder()
            .standard_plugins()
            .replace("markdown", Fake("Bad ID")));
        assert!(error.contains("Bad ID"), "{error}");
    }

    #[test]
    fn linked_plugin_uses_the_registry_glob_validation_too() {
        struct BadGlob;
        impl Plugin for BadGlob {
            fn manifest(&self) -> Manifest {
                Manifest::new("bad-glob", "0.1.0").bind(Bind::files(["["]))
            }
            fn open(&self, _: Input, _: &OpenCx) -> Result<Box<dyn Session>, Failure> {
                Err(Failure::unsupported("fixture"))
            }
        }
        for lang in [Lang::En, Lang::Jp] {
            let error = Host::builder()
                .plugin(BadGlob)
                .resolve_lang(lang)
                .err()
                .unwrap()
                .to_string();
            assert!(error.contains("bad-glob"), "{error}");
            if lang == Lang::Jp {
                crate::test_support::plugin_language::assert_japanese_plugin_prose(
                    "manifest glob",
                    &error,
                );
            }
        }
    }

    #[test]
    fn startup_environment_is_consumed_before_any_plugin_code() {
        let source = include_str!("host.rs");
        let builder = &source[source.find("pub fn builder()").unwrap()..];
        let builder = &builder[..builder.find("\n    }").unwrap()];
        assert!(builder.contains("HostBuilder::default()"));
        let default = source
            .find("impl Default for HostBuilder")
            .expect("startup must be captured when the builder is created");
        let default =
            &source[default..source[default..].find("\nimpl HostBuilder").unwrap() + default];
        assert!(default.contains("take_switched_from()"));
        assert!(default.contains("take_original_exe()"));
        let run = &source[source.find("pub fn run(self)").unwrap()..];
        assert!(run.contains("run_application(startup)"));
    }

    #[test]
    fn standard_only() {
        let s = Host::builder().standard_plugins().resolve().unwrap();
        assert!(s.standard && s.added.is_empty() && s.replaced.is_empty());
    }

    #[test]
    fn replacement_chains_are_independent_of_call_order() {
        let mut results = Vec::new();
        for replacements in [
            [("markdown", "my-markdown"), ("csv", "markdown")],
            [("csv", "markdown"), ("markdown", "my-markdown")],
        ] {
            let mut builder = Host::builder().standard_plugins();
            for (target, id) in replacements {
                builder = builder.replace(target, Fake(id));
            }
            let set = builder.resolve().unwrap();
            results.push(
                set.replaced
                    .iter()
                    .map(|(target, plugin)| (target.clone(), plugin.manifest().id))
                    .collect::<Vec<_>>(),
            );
        }
        assert_eq!(results[0], results[1]);
        assert_eq!(
            results[0],
            [
                ("csv".into(), "markdown".into()),
                ("markdown".into(), "my-markdown".into())
            ]
        );
    }

    #[test]
    fn replacement_conflicts_are_rejected_in_both_orders() {
        for mut replacements in [
            [("markdown", "same"), ("csv", "same")],
            [("markdown", "image"), ("csv", "other")],
            [("markdown", "text"), ("csv", "other")],
            [("markdown", "first"), ("markdown", "second")],
            [("unknown", "first"), ("csv", "other")],
        ] {
            for _ in 0..2 {
                let mut builder = Host::builder().standard_plugins();
                for (target, id) in replacements {
                    builder = builder.replace(target, Fake(id));
                }
                assert!(builder.resolve().is_err(), "accepted {replacements:?}");
                replacements.reverse();
            }
        }
    }

    #[test]
    fn nothing_standard_without_the_call() {
        let s = Host::builder().plugin(Fake("mine")).resolve().unwrap();
        assert!(!s.standard);
        assert_eq!(s.added.len(), 1);
    }

    #[test]
    fn plugin_adds_next_to_the_standard_ones() {
        let s = Host::builder()
            .standard_plugins()
            .plugin(Fake("mine"))
            .plugin(Fake("other"))
            .resolve()
            .unwrap();
        assert_eq!(s.added.len(), 2);
    }

    #[test]
    fn plugins_adds_a_crates_whole_list() {
        let list: Vec<Box<dyn Plugin>> = vec![Box::new(Fake("a")), Box::new(Fake("b"))];
        let s = Host::builder()
            .standard_plugins()
            .plugin(Fake("mine"))
            .plugins(list)
            .resolve()
            .unwrap();
        let ids: Vec<String> = s.added.iter().map(|p| p.manifest().id).collect();
        assert_eq!(ids, ["mine", "a", "b"]);
        assert!(Host::builder().plugins(Vec::new()).resolve().is_ok());
    }

    #[test]
    fn plugins_go_through_the_same_id_check() {
        let std_id: Vec<Box<dyn Plugin>> = vec![Box::new(Fake("markdown"))];
        let e = err(Host::builder().standard_plugins().plugins(std_id));
        assert!(e.contains("`markdown`"), "{e}");
        let internal: Vec<Box<dyn Plugin>> = vec![Box::new(Fake("text"))];
        assert!(err(Host::builder().plugins(internal)).contains("`text`"));
        let dup: Vec<Box<dyn Plugin>> = vec![Box::new(Fake("x")), Box::new(Fake("x"))];
        assert!(err(Host::builder().plugins(dup)).contains("`x`"));
        let across: Vec<Box<dyn Plugin>> = vec![Box::new(Fake("x"))];
        assert!(err(Host::builder().plugin(Fake("x")).plugins(across)).contains("`x`"));
    }

    #[test]
    fn order_of_the_calls_does_not_matter() {
        let s = Host::builder()
            .replace("markdown", Fake("my-markdown"))
            .standard_plugins()
            .resolve()
            .unwrap();
        assert!(s.replaced.contains_key("markdown"));
    }

    #[test]
    fn replace_swaps_a_standard_plugin() {
        let s = Host::builder()
            .standard_plugins()
            .replace("markdown", Fake("my-markdown"))
            .resolve()
            .unwrap();
        assert_eq!(s.replaced["markdown"].manifest().id, "my-markdown");
    }

    #[test]
    fn replace_by_a_plugin_of_the_same_id_is_allowed() {
        let s = Host::builder()
            .standard_plugins()
            .replace("markdown", Fake("markdown"))
            .resolve()
            .unwrap();
        assert!(s.replaced.contains_key("markdown"));
    }

    #[test]
    fn a_replaced_id_can_be_taken_again_by_plugin() {
        // The standard `markdown` is gone, so its id is free for another plugin.
        assert!(Host::builder()
            .standard_plugins()
            .replace("markdown", Fake("my-markdown"))
            .plugin(Fake("markdown"))
            .resolve()
            .is_ok());
    }

    #[test]
    fn plugin_with_a_standard_id_is_refused() {
        let e = err(Host::builder().standard_plugins().plugin(Fake("markdown")));
        assert!(e.contains("`markdown`"), "{e}");
    }

    #[test]
    fn plugin_with_an_internal_id_is_refused() {
        for id in ["text", "command", "can-not-preview"] {
            let e = err(Host::builder().plugin(Fake(id)));
            assert!(e.contains(id), "{e}");
        }
    }

    #[test]
    fn the_same_id_twice_is_refused() {
        let e = err(Host::builder().plugin(Fake("mine")).plugin(Fake("mine")));
        assert!(e.contains("`mine`"), "{e}");
    }

    #[test]
    fn replace_of_an_unknown_id_is_refused() {
        let e = err(Host::builder()
            .standard_plugins()
            .replace("nope", Fake("x")));
        assert!(e.contains("`nope`"), "{e}");
    }

    #[test]
    fn replace_without_the_standard_plugins_is_refused() {
        let e = err(Host::builder().replace("markdown", Fake("x")));
        assert!(e.contains("`markdown`"), "{e}");
    }

    #[test]
    fn replace_of_an_internal_id_is_refused() {
        let e = err(Host::builder()
            .standard_plugins()
            .replace("text", Fake("x")));
        assert!(e.contains("`text`"), "{e}");
    }

    #[test]
    fn replacing_the_same_standard_plugin_twice_is_refused() {
        let e = err(Host::builder()
            .standard_plugins()
            .replace("markdown", Fake("a"))
            .replace("markdown", Fake("b")));
        assert!(e.contains("`markdown`"), "{e}");
    }

    #[test]
    fn a_replacement_may_not_take_another_standard_id() {
        let e = err(Host::builder()
            .standard_plugins()
            .replace("markdown", Fake("image")));
        assert!(e.contains("`image`"), "{e}");
    }

    #[test]
    fn plugin_may_not_reuse_a_replacements_id() {
        let e = err(Host::builder()
            .standard_plugins()
            .replace("markdown", Fake("mine"))
            .plugin(Fake("mine")));
        assert!(e.contains("`mine`"), "{e}");
    }

    #[test]
    fn run_returns_the_error_before_anything_starts() {
        // `run` must fail on a bad set without touching the terminal -- and without declaring this
        // process to be the application (which would switch the test seams off).
        let r = Host::builder()
            .standard_plugins()
            .plugin(Fake("markdown"))
            .run();
        assert!(r.unwrap_err().to_string().contains("`markdown`"));
        assert!(!konoma_base::process::is_app_main());
        let r = Host::builder().replace("nope", Fake("x")).run();
        assert!(r.is_err());
        assert!(!konoma_base::process::is_app_main());
    }
}
