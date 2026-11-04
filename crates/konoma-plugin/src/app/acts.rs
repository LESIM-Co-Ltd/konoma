//! The declared actions of the plugin the app shows, as the app offers them: the same rules as
//! konoma's (`crate::host::acts`, `crate::host::bind`), over the handle "the action's id".
//!
//! What lives here is only what the loop asks: which declared action a key runs in the view on
//! screen, the footer tokens and the help section the actions add, and the words of the
//! refusals. Whether an action works now is the session's answer (`Session::enabled`), read
//! through [`crate::host::acts::act_enabled`]; whether it may run is [`crate::host::acts::gate`].

use std::collections::HashMap;

use konoma_host_core::help::{help_key, write_keys, CoreSection};
use konoma_host_core::keymap::KeyPress;

use crate::host::acts::{self, ActBinding};
use crate::host::bind::{bind_actions, KeyNote};
use crate::manifest::Manifest;
use crate::text::Tr;

/// The declared actions of the plugin showing the file.
#[derive(Clone, Debug, Default)]
pub struct AppActs {
    /// In declaration order, with the keys the user's `[keys.<id>]` left them.
    pub bindings: Vec<ActBinding<String>>,
    /// The plugin's display name: the title of its section in the help.
    pub plugin_name: Option<Tr>,
    /// `[plugins.<id>] read_only = true`: the writing actions are refused.
    pub read_only: bool,
    /// What was wrong with the user's `[keys.<id>]` table: told in the footer at start, as
    /// konoma tells it ([`crate::host::bind::key_notes_line`]).
    pub notes: Vec<KeyNote>,
}

/// The key tables of the config that are the host's (a context, `global`, the old `sort`), not a
/// plugin's `[keys.<id>]`.
fn is_host_key_table(name: &str) -> bool {
    name == "global"
        || name == "sort"
        || konoma_host_core::keymap::CoreContext::from_name(name).is_some()
}

impl AppActs {
    /// The actions of plugin `id` from its `manifest`, with the user's `[keys.<id>]` table
    /// (`user`, `None` = the user wrote none) applied the way konoma applies it. The notes say
    /// what was wrong with the table.
    pub fn from_manifest(
        id: &str,
        manifest: &Manifest,
        user: Option<&HashMap<String, String>>,
        read_only: bool,
    ) -> (AppActs, Vec<KeyNote>) {
        let (bindings, notes) = bind_actions(id, manifest, user, |a| a.to_string());
        let acts = AppActs {
            bindings,
            plugin_name: Some(manifest.name.clone()),
            read_only,
            notes: notes.clone(),
        };
        (acts, notes)
    }

    /// [`AppActs::from_manifest`] from the loaded config: `[keys.<id>]` and
    /// `[plugins.<id>] read_only`.
    pub fn from_config(
        id: &str,
        manifest: &Manifest,
        keys: &konoma_host_core::keys::KeysConfig,
        plugins: &std::collections::BTreeMap<String, toml::Table>,
    ) -> (AppActs, Vec<KeyNote>) {
        // A plugin whose id is also the name of a host context keeps that table for the host.
        let user = if is_host_key_table(id) {
            None
        } else {
            keys.surfaces.get(id)
        };
        let read_only = matches!(
            plugins.get(id).and_then(|t| t.get("read_only")),
            Some(toml::Value::Boolean(true))
        );
        AppActs::from_manifest(id, manifest, user, read_only)
    }

    /// The actions of the key context named `ctx` (`canvas`, `table`, ...).
    fn in_context(&self, ctx: &str) -> Vec<&ActBinding<String>> {
        acts::acts_in(&self.bindings, ctx)
    }

    /// The action `kp` runs in `ctx`.
    pub fn for_key(&self, ctx: &str, kp: KeyPress) -> Option<&ActBinding<String>> {
        acts::act_for_key(&self.in_context(ctx), kp)
    }

    /// The footer tokens the actions add (`<keys>:<label>`), in the order of the priority they
    /// declared; an action that does not work now (`enabled`) adds none.
    pub fn footer_tokens(
        &self,
        ctx: &str,
        lang: &str,
        enabled: impl Fn(&str) -> bool,
    ) -> Vec<String> {
        acts::footer_hints(&self.in_context(ctx))
            .into_iter()
            .filter_map(|hint| {
                let keys = hint.keys(|b| enabled(&b.act));
                (!keys.is_empty())
                    .then(|| format!("{}:{}", write_keys(&keys, true), hint.label(lang)))
            })
            .collect()
    }

    /// The footer of a screen with the actions' tokens put right after its first entry (konoma
    /// puts them right after the first entry of the plan, the way back).
    pub fn with_footer(
        &self,
        ctx: &str,
        lang: &str,
        enabled: impl Fn(&str) -> bool,
        mut tokens: Vec<String>,
    ) -> Vec<String> {
        let at = 1.min(tokens.len());
        let own = self.footer_tokens(ctx, lang, enabled);
        tokens.splice(at..at, own);
        tokens
    }

    /// The plugin's section of the `?` help: the keyed actions that work now, under the plugin's
    /// name. `None` when there is no row to list.
    pub fn help_section(
        &self,
        ctx: &str,
        lang: &str,
        enabled: impl Fn(&str) -> bool,
    ) -> Option<CoreSection> {
        let acts = self.in_context(ctx);
        let rows = acts::help_rows(&acts, lang, |b| enabled(&b.act));
        if rows.is_empty() {
            return None;
        }
        Some(CoreSection {
            title: self
                .plugin_name
                .as_ref()
                .map(|n| n.get(lang).to_string())
                .unwrap_or_default(),
            rows: rows
                .into_iter()
                .map(|(keys, desc, _)| {
                    (
                        keys.iter()
                            .map(|k| help_key(*k))
                            .collect::<Vec<_>>()
                            .join(" / "),
                        desc.to_string(),
                    )
                })
                .collect(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Act, HintGroup, ViewKind};

    fn grouped_actions() -> AppActs {
        let manifest = Manifest::new("hints", "1")
            .name("Hint actions")
            .shows(ViewKind::Canvas)
            .hint_group(HintGroup::new("stage", "stage(all)").help("stage / stage all"))
            .action(
                Act::new("stage")
                    .key("s")
                    .label("stage")
                    .footer(8)
                    .hint_group("stage"),
            )
            .action(
                Act::new("outside")
                    .key("x")
                    .label("outside")
                    .help("outside help")
                    .footer(4),
            )
            .action(
                Act::new("stage-all")
                    .key("S")
                    .label("stage all")
                    .footer(2)
                    .hint_group("stage"),
            );
        let (actions, notes) = AppActs::from_manifest("hints", &manifest, None, false);
        assert!(notes.is_empty(), "{notes:?}");
        actions
    }

    #[test]
    fn standalone_hint_group_footer_order_and_enabled_keys() {
        let actions = grouped_actions();
        assert_eq!(
            actions.footer_tokens("canvas", "en", |_| true),
            ["s/S:stage(all)", "x:outside"]
        );
        for (enabled, token) in [("stage", "s:stage(all)"), ("stage-all", "S:stage(all)")] {
            assert_eq!(
                actions.footer_tokens("canvas", "en", |id| id == enabled || id == "outside"),
                [token, "x:outside"]
            );
        }
        assert_eq!(
            actions.footer_tokens("canvas", "en", |id| id == "outside"),
            ["x:outside"]
        );
        assert!(actions.footer_tokens("canvas", "en", |_| false).is_empty());
    }

    #[test]
    fn standalone_hint_group_help_matches_host_and_keeps_ungrouped_row() {
        let mut actions = grouped_actions();
        let section = actions.help_section("canvas", "en", |_| true).unwrap();
        assert_eq!(section.title, "Hint actions");
        // These are the same keys and description as the host's project-screen test.
        assert_eq!(
            section.rows,
            [
                ("s / S".into(), "stage / stage all".into()),
                ("x".into(), "outside help".into()),
            ]
        );
        for (enabled, key) in [("stage", "s"), ("stage-all", "S")] {
            assert_eq!(
                actions
                    .help_section("canvas", "en", |id| id == enabled || id == "outside")
                    .unwrap()
                    .rows,
                [
                    (key.into(), "stage / stage all".into()),
                    ("x".into(), "outside help".into())
                ]
            );
        }
        assert_eq!(
            actions
                .help_section("canvas", "en", |id| id == "outside")
                .unwrap()
                .rows,
            [("x".into(), "outside help".into())]
        );
        assert!(actions.help_section("canvas", "en", |_| false).is_none());
        for binding in &mut actions.bindings {
            if let Some((_, group)) = &mut binding.hint_group {
                group.help = Tr::new("");
            }
        }
        assert_eq!(
            actions.help_section("canvas", "en", |_| true).unwrap().rows,
            [
                ("s / S".into(), "stage(all)".into()),
                ("x".into(), "outside help".into())
            ]
        );
    }

    #[test]
    fn standalone_hint_group_space_menu_keeps_individual_actions() {
        use crate::app::{space, LaunchFacts};
        use konoma_host_core::keymap::{CoreContext, CoreKeys, KeyScheme};
        use konoma_host_core::lang::Lang;

        let actions = grouped_actions();
        let keys = CoreKeys::defaults(KeyScheme::Vim);
        let items = space::items(
            &keys,
            &keys,
            CoreContext::Canvas,
            Lang::En,
            "en",
            &actions,
            "canvas",
            false,
            &LaunchFacts::default(),
            |_| true,
        );
        let mut declared: Vec<_> = items
            .iter()
            .filter_map(|item| {
                let binding = space::declared(&actions, "canvas", item.action)?;
                Some((binding.act.as_str(), item.key, item.label.as_str()))
            })
            .collect();
        declared.sort_by_key(|(id, _, _)| *id);
        assert_eq!(
            declared,
            [
                ("outside", KeyPress::ch('x'), "outside"),
                ("stage", KeyPress::ch('s'), "stage"),
                ("stage-all", KeyPress::ch('S'), "stage all"),
            ]
        );
    }
}
