//! The declared actions of the plugin the app shows, as the app offers them: the same rules as
//! konoma's (`crate::host::acts`, `crate::host::bind`), over the handle "the action's id".
//!
//! What lives here is only what the loop asks: which declared action a key runs in the view on
//! screen, the footer tokens and the help section the actions add, and the words of the
//! refusals. Whether an action works now is the session's answer (`Session::enabled`), read
//! through [`crate::host::acts::act_enabled`]; whether it may run is [`crate::host::acts::gate`].

use std::collections::HashMap;

use konoma_host_core::help::{footer_key, help_key, CoreSection};
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
        acts::footer_order(&self.in_context(ctx))
            .into_iter()
            .filter_map(|b| {
                acts::footer_token(b, enabled(&b.act), lang, |k| footer_key(k, true)).map(|t| t.0)
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
                .map(|(k, desc, _)| (help_key(k), desc.to_string()))
                .collect(),
        })
    }
}
