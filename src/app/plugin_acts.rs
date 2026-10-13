//! The actions of the open plugin as the host's operations.
//!
//! What a plugin declares in `Manifest::actions` (a key, a label, a help text, a footer priority,
//! whether it writes) is read once at startup (`plugins::PluginKeys`); here is what depends on the
//! screen: which of them belong to the context on screen, whether each works now, and running one.
//!
//! **Whether an action works** has one answer, the session's (`Session::enabled`), asked on the
//! worker each time the session comes back from a job and kept in `PluginShow::enabled`. The key
//! handler (`ops::gate`), the footer, the help and the `Space` menu all read that answer, so a hint
//! is shown exactly while its key acts, and the UI thread never waits for a session that a job
//! holds (the same shape holds when the session lives in a child process).

use super::*;
use crate::keymap::{Action, Context, KeyPress};
use crate::plugins::{ActBinding, PluginAct};

/// Whether a plugin's keys can live in `ctx`: the contexts that show a plugin's view (the plugin's
/// layer sits on top of the display type's).
fn is_view_context(ctx: Context) -> bool {
    matches!(
        ctx,
        Context::Text | Context::Document | Context::Table | Context::Image
    )
}

impl App {
    /// The id of the plugin whose file is on screen, if its show is the current one.
    fn acting_plugin(&self) -> Option<&str> {
        let Some(PreviewKind::Plugin { id, path, .. }) = &self.tab.preview_kind else {
            return None;
        };
        self.plugin
            .as_ref()
            .filter(|p| p.id == *id && p.path == *path)
            .map(|p| p.id.as_str())
    }

    /// The declared actions of the plugin on screen that belong to `ctx`, in declaration order.
    /// Empty off the plugin views.
    pub fn plugin_acts_in(&self, ctx: Context) -> Vec<&ActBinding> {
        if !is_view_context(ctx) {
            return Vec::new();
        }
        let Some(id) = self.acting_plugin() else {
            return Vec::new();
        };
        let name = ctx.name();
        self.plugin_keys
            .acts_of(id)
            .iter()
            .filter(|b| b.context.as_deref().is_none_or(|c| c == name))
            .collect()
    }

    /// The operation a key runs in the plugin's layer. Layers 0 to 3 are the host's and a plugin
    /// cannot take their keys, so this is asked after the fixed keys and before the keymap
    /// without any chance of shadowing either.
    pub fn plugin_action_for_key(&self, ctx: Context, kp: KeyPress) -> Option<Action> {
        self.plugin_acts_in(ctx)
            .into_iter()
            .find(|b| b.key == Some(kp))
            .map(|b| Action::Plugin(b.act))
    }

    /// The declared action `p`, if it exists and belongs to `ctx`.
    pub fn plugin_binding_in(&self, ctx: Context, p: PluginAct) -> Option<&ActBinding> {
        self.plugin_acts_in(ctx).into_iter().find(|b| b.act == p)
    }

    /// Whether the session says `p` works now. False before the first answer (while the file is
    /// loading) and when the plugin could not open the file.
    pub fn plugin_act_enabled(&self, p: PluginAct) -> bool {
        self.plugin
            .as_ref()
            .filter(|s| s.id == p.plugin() && matches!(s.phase, plugin_host::PluginPhase::Ready))
            .and_then(|s| s.enabled.get(p.act()))
            .copied()
            .unwrap_or(false)
    }

    /// Whether `[plugins.<id>] read_only = true` forbids the plugin's writing actions.
    pub fn plugin_is_read_only(&self, plugin: &str) -> bool {
        matches!(
            self.cfg
                .plugins
                .get(plugin)
                .and_then(|t| t.get("read_only")),
            Some(toml::Value::Boolean(true))
        )
    }

    /// The key of `p` in `ctx`, if it has one.
    pub fn plugin_key_of(&self, ctx: Context, p: PluginAct) -> Option<KeyPress> {
        self.plugin_binding_in(ctx, p).and_then(|b| b.key)
    }

    /// Run a declared action of the open plugin on the worker. While a job still holds the
    /// session (the previous key is being answered) the press is ignored; the footer keeps the
    /// last answer of `enabled` meanwhile.
    pub fn run_plugin_action(&mut self, p: PluginAct) {
        let _ = self.plugin_act(p.act());
    }
}
