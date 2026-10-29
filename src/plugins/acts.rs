//! The actions plugins declare (`Manifest::actions`), as the host's operations.
//!
//! A plugin's action becomes `Action::Plugin(PluginAct)`: it is found by its key in the plugin's
//! own layer (layer 4 of the key design: a plugin cannot use a key of layers 0 to 3, so there is
//! nothing to collide with), shown in the footer, the `?` help and the `Space` menu, and run
//! through `dispatch_action` like every other operation. This module only holds the data: who
//! declared what, with which key. Whether an action works *now* is the open session's word
//! (`Session::enabled`), kept by the plugin show (`app/plugin_acts.rs`).
//!
//! Keys: the declared one, replaced by `[keys.<plugin id>]` (`<key> = "<action id>"`, or
//! `<key> = "noop"` to take a key away). Every key is checked the way the registry checks a
//! manifest (`konoma_plugin::keys`); one that is reserved or taken is dropped, the action stays in
//! the `Space` menu, and the startup message says so.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Mutex, OnceLock};

use konoma_plugin::host::bind::{bind_actions, KeyNote};
use konoma_plugin::keys::KeyIssue;
use konoma_plugin::registry::{NoteKind, Registry};
use konoma_plugin::{ApiVersion, Tr};

/// Make a string live for the whole run, once per distinct value. Ids come from manifests, so the
/// set is as small as the set of declared plugins and actions.
fn intern(s: &str) -> &'static str {
    static POOL: OnceLock<Mutex<BTreeSet<&'static str>>> = OnceLock::new();
    let mut pool = POOL
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if let Some(found) = pool.get(s) {
        return found;
    }
    let leaked: &'static str = Box::leak(s.to_string().into_boxed_str());
    pool.insert(leaked);
    leaked
}

/// One declared action of one plugin. `Copy` so it can live inside `Action`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PluginAct {
    plugin: &'static str,
    act: &'static str,
}

impl PluginAct {
    pub fn new(plugin: &str, act: &str) -> PluginAct {
        PluginAct {
            plugin: intern(plugin),
            act: intern(act),
        }
    }

    /// The plugin id.
    pub fn plugin(self) -> &'static str {
        self.plugin
    }

    /// The action id, as the plugin declared it.
    pub fn act(self) -> &'static str {
        self.act
    }
}

/// An action as the host binds it (the shared binding, with konoma's `Copy` handle).
pub type ActBinding = konoma_plugin::host::acts::ActBinding<PluginAct>;

/// Something worth telling the user at startup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginNote {
    /// A key of a manifest was dropped or an action refused.
    Key { plugin: String, issue: KeyIssue },
    /// A plugin built for another API version is switched off.
    ApiMismatch {
        plugin: String,
        declared: ApiVersion,
        host: ApiVersion,
    },
    /// The plugin is given files but declares no `shows`: the host attaches no screen to them.
    ShowsNotDeclared { plugin: String },
    /// The plugin was refused: a name it declares is not usable or it declares too much.
    Refused { plugin: String, why: String },
    /// `[keys.<plugin>]` names an action the plugin does not declare.
    UnknownAction { plugin: String, action: String },
    /// `[keys.<plugin>]` has a key that is not usable (does not parse, reserved, taken).
    BadKey {
        plugin: String,
        key: String,
        why: String,
    },
}

/// Every declared action of every registered plugin, with its key.
#[derive(Debug, Clone, Default)]
pub struct PluginKeys {
    by_plugin: BTreeMap<String, PluginEntry>,
    notes: Vec<PluginNote>,
}

#[derive(Debug, Clone)]
struct PluginEntry {
    name: Tr,
    acts: Vec<ActBinding>,
}

#[cfg(test)]
pub use konoma_plugin::host::bind::keypress_of;

impl PluginKeys {
    /// Read the actions of every registered plugin. `tables` is the config's raw `[keys.*]`
    /// tables: the ones named after a plugin are taken out (they are not host contexts) and
    /// applied here.
    pub fn build(
        registry: &Registry,
        tables: &mut HashMap<String, HashMap<String, String>>,
    ) -> PluginKeys {
        let mut out = PluginKeys::default();
        for note in registry.notes() {
            out.notes.push(match note.kind {
                NoteKind::Key(issue) => PluginNote::Key {
                    plugin: note.plugin,
                    issue,
                },
                NoteKind::ApiMismatch { declared, host } => PluginNote::ApiMismatch {
                    plugin: note.plugin,
                    declared,
                    host,
                },
                NoteKind::ShowsNotDeclared => PluginNote::ShowsNotDeclared {
                    plugin: note.plugin,
                },
                NoteKind::Refused { why } => PluginNote::Refused {
                    plugin: note.plugin,
                    why,
                },
            });
        }
        for id in registry.ids() {
            let Some(manifest) = registry.manifest(id) else {
                continue;
            };
            // A plugin whose id is also the name of a host context (`text`) keeps that table for
            // the host.
            let user = if crate::keymap::is_host_key_table(id) {
                None
            } else {
                tables.remove(id)
            };
            let (acts, key_notes) =
                bind_actions(id, manifest, user.as_ref(), |a| PluginAct::new(id, a));
            out.notes.extend(key_notes.into_iter().map(|n| match n {
                KeyNote::UnknownAction { plugin, action } => {
                    PluginNote::UnknownAction { plugin, action }
                }
                KeyNote::BadKey { plugin, key, why } => PluginNote::BadKey { plugin, key, why },
            }));
            if manifest.actions.is_empty() {
                continue;
            }
            out.by_plugin.insert(
                id.to_string(),
                PluginEntry {
                    name: manifest.name.clone(),
                    acts,
                },
            );
        }
        out
    }

    /// The actions of plugin `id`, in declaration order.
    pub fn acts_of(&self, id: &str) -> &[ActBinding] {
        self.by_plugin.get(id).map_or(&[], |e| e.acts.as_slice())
    }

    /// The display name of a plugin.
    pub fn name_of(&self, id: &str) -> Option<&Tr> {
        self.by_plugin.get(id).map(|e| &e.name)
    }

    /// The startup message: what is wrong with the plugins' keys and versions, one piece per
    /// problem (the caller joins them and counts what does not fit).
    pub fn report(&self, lang: crate::i18n::Lang) -> Vec<String> {
        use crate::i18n::Lang;
        self.notes
            .iter()
            .map(|n| match (n, lang) {
                (PluginNote::Key { plugin, issue }, _) => format!("{plugin}: {issue}"),
                (
                    PluginNote::ApiMismatch {
                        plugin,
                        declared,
                        host,
                    },
                    Lang::Jp,
                ) => format!(
                    "{plugin}: プラグイン API {declared} 用です(この konoma は {host})。無効にしました"
                ),
                (
                    PluginNote::ApiMismatch {
                        plugin,
                        declared,
                        host,
                    },
                    Lang::En,
                ) => format!(
                    "{plugin}: built for plugin API {declared} (this konoma speaks {host}); switched off"
                ),
                (PluginNote::ShowsNotDeclared { plugin }, Lang::Jp) => format!(
                    "{plugin}: ファイルを割り当てていますが shows を宣言していないので画面が付きません(Manifest::shows)"
                ),
                (PluginNote::ShowsNotDeclared { plugin }, Lang::En) => format!(
                    "{plugin}: it is given files but declares no `shows`, so it gets no screen (Manifest::shows)"
                ),
                (PluginNote::Refused { plugin, why }, Lang::Jp) => {
                    format!("{plugin}: 宣言が不正なので登録しませんでした: {why}")
                }
                (PluginNote::Refused { plugin, why }, Lang::En) => {
                    format!("{plugin}: not registered, its declarations are not usable: {why}")
                }
                (PluginNote::UnknownAction { plugin, action }, lang) => KeyNote::UnknownAction {
                    plugin: plugin.clone(),
                    action: action.clone(),
                }
                .text(lang),
                (PluginNote::BadKey { plugin, key, why }, lang) => KeyNote::BadKey {
                    plugin: plugin.clone(),
                    key: key.clone(),
                    why: why.clone(),
                }
                .text(lang),
            })
            .collect()
    }

    /// The notes, for tests.
    #[cfg(test)]
    pub fn notes(&self) -> &[PluginNote] {
        &self.notes
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::keymap::KeyPress;
    use crossterm::event::KeyCode;
    use konoma_plugin::keys::{KeyName as PKeyName, KeySpec};
    use konoma_plugin::registry::PluginOrigin;

    use super::*;
    use crate::plugins::test_plugins::{Hexdump, Variant};

    fn registry(extra: bool) -> Registry {
        let mut r = Registry::new();
        r.register(
            Arc::new(Hexdump(Variant {
                extra_acts: extra,
                ..Variant::default()
            })),
            PluginOrigin::User,
        )
        .unwrap();
        r
    }

    fn tables(entries: &[(&str, &str)]) -> HashMap<String, HashMap<String, String>> {
        let mut t = HashMap::new();
        t.insert(
            "hexdump".to_string(),
            entries
                .iter()
                .map(|(k, a)| (k.to_string(), a.to_string()))
                .collect(),
        );
        t
    }

    fn key_of(k: &PluginKeys, act: &str) -> Option<KeyPress> {
        k.acts_of("hexdump")
            .iter()
            .find(|b| b.act.act() == act)
            .and_then(|b| b.key)
    }

    #[test]
    fn the_declared_key_is_used_and_the_plugin_table_is_taken_out_of_the_host_config() {
        let mut t = tables(&[]);
        t.insert("text".into(), HashMap::new());
        let k = PluginKeys::build(&registry(false), &mut t);
        assert_eq!(key_of(&k, "width"), Some(KeyPress::ch('w')));
        assert!(!t.contains_key("hexdump"));
        assert!(t.contains_key("text"), "a host context is left alone");
        assert!(k.notes().is_empty());
    }

    #[test]
    fn a_user_key_replaces_the_declared_one() {
        let mut t = tables(&[("W", "width")]);
        let k = PluginKeys::build(&registry(false), &mut t);
        assert_eq!(key_of(&k, "width"), Some(KeyPress::ch('W')));
        assert!(k.notes().is_empty());
    }

    #[test]
    fn function_keys_and_ctrl_keys_are_accepted() {
        let mut t = tables(&[("F5", "width")]);
        let k = PluginKeys::build(&registry(false), &mut t);
        assert_eq!(
            key_of(&k, "width"),
            Some(KeyPress::key(KeyCode::F(5))),
            "{:?}",
            k.notes()
        );
        let mut t = tables(&[("Ctrl-w", "width")]);
        let k = PluginKeys::build(&registry(false), &mut t);
        assert_eq!(key_of(&k, "width"), Some(KeyPress::ctrl_ch('w')));
    }

    #[test]
    fn noop_takes_a_key_away_and_the_action_stays() {
        let mut t = tables(&[("w", "noop")]);
        let k = PluginKeys::build(&registry(false), &mut t);
        assert_eq!(key_of(&k, "width"), None);
        assert_eq!(k.acts_of("hexdump").len(), 1);
    }

    #[test]
    fn bad_entries_are_noted_and_ignored() {
        // An unknown action, a reserved key, an unparseable key, a second key for one action.
        let mut t = tables(&[
            ("a", "nope"),
            ("j", "width"),
            ("ab", "width"),
            ("x", "noop"),
        ]);
        let k = PluginKeys::build(&registry(false), &mut t);
        let notes = k.notes();
        assert!(
            notes.iter().any(|n| matches!(
                n,
                PluginNote::UnknownAction { action, .. } if action == "nope"
            )),
            "{notes:?}"
        );
        assert_eq!(
            key_of(&k, "width"),
            None,
            "no usable key was given: {notes:?}"
        );
        let said = k.report(crate::i18n::Lang::En).join(" / ");
        assert!(
            said.contains("[keys.hexdump] unknown action `nope`"),
            "{said}"
        );
        assert!(
            said.contains("key `j`") && said.contains("reserved"),
            "{said}"
        );
        let ja = k.report(crate::i18n::Lang::Jp).join(" / ");
        assert!(ja.contains("未知の操作"), "{ja}");
    }

    #[test]
    fn two_actions_cannot_share_a_user_key() {
        let mut t = tables(&[("y", "dump"), ("y2", "dump")]);
        let k = PluginKeys::build(&registry(true), &mut t);
        assert!(
            k.notes()
                .iter()
                .any(|n| matches!(n, PluginNote::BadKey { .. })),
            "{:?}",
            k.notes()
        );
        let mut t = tables(&[("t", "dump"), ("t", "jump")]);
        let _ = PluginKeys::build(&registry(true), &mut t);
    }

    #[test]
    fn a_reserved_default_key_is_told_with_the_registry_note() {
        let mut t = tables(&[]);
        let k = PluginKeys::build(&registry(true), &mut t);
        assert_eq!(key_of(&k, "jump"), None);
        assert_eq!(key_of(&k, "dump"), Some(KeyPress::ch('x')));
        let said = k.report(crate::i18n::Lang::En);
        assert_eq!(said.len(), 1, "{said:?}");
        assert!(
            said[0].starts_with("hexdump: action `jump`: key `j`"),
            "{said:?}"
        );
    }

    #[test]
    fn a_plugin_built_for_another_api_is_told() {
        struct Old;
        impl konoma_plugin::Plugin for Old {
            fn manifest(&self) -> konoma_plugin::Manifest {
                let mut m = konoma_plugin::Manifest::new("old", "1");
                m.api = ApiVersion {
                    major: 0,
                    minor: 99,
                };
                m
            }
            fn open(
                &self,
                _: konoma_plugin::Input,
                _: &konoma_plugin::OpenCx,
            ) -> Result<Box<dyn konoma_plugin::Session>, konoma_plugin::Failure> {
                Err(konoma_plugin::Failure::unsupported("old"))
            }
        }
        let mut r = Registry::new();
        r.register(Arc::new(Old), PluginOrigin::User).unwrap();
        let k = PluginKeys::build(&r, &mut HashMap::new());
        let said = k.report(crate::i18n::Lang::En);
        assert_eq!(said.len(), 1);
        assert!(
            said[0].contains("switched off") && said[0].contains("0.99"),
            "{said:?}"
        );
        assert!(k.report(crate::i18n::Lang::Jp)[0].contains("無効"));
    }

    #[test]
    fn the_interner_gives_equal_handles_for_equal_ids() {
        let a = PluginAct::new("p", "go");
        let b = PluginAct::new(&String::from("p"), &String::from("go"));
        assert_eq!(a, b);
        assert!(std::ptr::eq(a.plugin(), b.plugin()));
        assert_ne!(a, PluginAct::new("p", "stop"));
    }

    #[test]
    fn keys_cover_every_plugin_key_code() {
        for (spec, want) in [
            (KeySpec::ch('a'), KeyPress::ch('a')),
            (KeySpec::ctrl('a'), KeyPress::ctrl_ch('a')),
            (KeySpec::key(PKeyName::Enter), KeyPress::key(KeyCode::Enter)),
            (
                KeySpec::key(PKeyName::BackTab),
                KeyPress::key(KeyCode::BackTab),
            ),
            (
                KeySpec::key(PKeyName::PageDown),
                KeyPress::key(KeyCode::PageDown),
            ),
            (KeySpec::key(PKeyName::F(12)), KeyPress::key(KeyCode::F(12))),
        ] {
            assert_eq!(keypress_of(spec), want);
        }
    }

    /// A plugin whose manifest the registry refuses (a name with an escape character) is named in
    /// the startup message under a safe id, in both languages, and is not registered.
    #[test]
    fn a_refused_manifest_is_named_in_the_startup_message_with_a_safe_id() {
        struct Evil;
        impl konoma_plugin::Plugin for Evil {
            fn manifest(&self) -> konoma_plugin::Manifest {
                konoma_plugin::Manifest::new("evil\u{1b}[2J", "1")
            }
            fn open(
                &self,
                _: konoma_plugin::Input,
                _: &konoma_plugin::OpenCx,
            ) -> Result<Box<dyn konoma_plugin::Session>, konoma_plugin::Failure> {
                Err(konoma_plugin::Failure::unsupported(Tr::new("no")))
            }
        }
        let mut r = Registry::new();
        assert!(r.register(Arc::new(Evil), PluginOrigin::User).is_err());
        let k = PluginKeys::build(&r, &mut HashMap::new());
        for lang in [crate::i18n::Lang::En, crate::i18n::Lang::Jp] {
            let report = k.report(lang);
            assert_eq!(report.len(), 1, "{report:?}");
            assert!(report[0].starts_with("evil??2?: "), "{report:?}");
            assert!(!report[0].contains('\u{1b}'));
        }
        assert!(r.ids().is_empty());
    }
}
