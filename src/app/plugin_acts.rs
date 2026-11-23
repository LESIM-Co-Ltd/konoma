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
use konoma_plugin::host::acts;

impl App {
    /// The id of the plugin whose session the show holds (the destination after a `Reopen`), if the
    /// show is the current one: its declared actions are the ones offered and run.
    fn acting_plugin(&self) -> Option<&str> {
        let Some(PreviewKind::Plugin { id, path, .. }) = &self.tab.preview_kind else {
            return None;
        };
        self.plugin
            .as_ref()
            .filter(|p| p.id == *id && p.path == *path)
            .map(|p| p.acting())
    }

    /// The declared actions of the plugin on screen that belong to `ctx`, in declaration order.
    /// Empty off the plugin views.
    pub fn plugin_acts_in(&self, ctx: Context) -> Vec<&ActBinding> {
        let Some(id) = self.acting_plugin() else {
            return Vec::new();
        };
        acts::acts_in(self.plugin_keys.acts_of(id), &ctx.name())
    }

    /// The operation a key runs in the plugin's layer. Layers 0 to 3 are the host's and a plugin
    /// cannot take their keys, so this is asked after the fixed keys and before the keymap
    /// without any chance of shadowing either.
    pub fn plugin_action_for_key(&self, ctx: Context, kp: KeyPress) -> Option<Action> {
        acts::act_for_key(&self.plugin_acts_in(ctx), kp).map(|b| Action::Plugin(b.act))
    }

    /// The declared action `p`, if it exists and belongs to `ctx`.
    pub fn plugin_binding_in(&self, ctx: Context, p: PluginAct) -> Option<&ActBinding> {
        self.plugin_acts_in(ctx).into_iter().find(|b| b.act == p)
    }

    /// Whether the session says `p` works now. False before the first answer (while the file is
    /// loading) and when the plugin could not open the file. **While a job holds the session** the
    /// answer is the last the session gave, for a canvas (its presses wait in line, so the key does
    /// act: [[hint-shown-iff-key-acts]], and the hint does not flicker with a redraw timer's
    /// ticks); for any other display it is false, because the press would be ignored -- except while the file is read again (`ShowFlow::reopening`): then the press waits for the new session, so the hint stays.
    pub fn plugin_act_enabled(&self, p: PluginAct) -> bool {
        let canvas = self.is_canvas_preview();
        let facts = match self.plugin.as_ref() {
            Some(s) => acts::ShowFacts {
                same_plugin: s.acting() == p.plugin(),
                ready: matches!(s.phase, plugin_host::PluginPhase::Ready),
                session_present: s.session.is_some(),
                canvas,
                reopening: s.flow.reopening,
                says: s.enabled.get(p.act()).copied(),
            },
            None => acts::ShowFacts::default(),
        };
        acts::act_enabled(&facts)
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
    /// session of a canvas the press waits in line and goes when the session returns; for any
    /// other display it is ignored, and so is its hint (`plugin_act_enabled`), unless the file is
    /// being read again: then it waits for the new session.
    pub fn run_plugin_action(&mut self, p: PluginAct) {
        let _ = self.plugin_act(p.act());
    }
}
