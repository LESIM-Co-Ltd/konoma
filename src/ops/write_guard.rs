//! The host's write policy. The key gate and the execution boundaries ask this same function.

use crate::app::App;
use crate::i18n::Msg;

use super::Gate;

/// Whether an operation may write on behalf of `plugin`. `repository` identifies the legacy
/// git executor: its backend must support git writes as well as the plugin allowing writes.
/// Read operations always pass, including those of a read-only plugin.
pub fn write_guard(app: &App, plugin: &str, writes: bool, repository: bool) -> Gate {
    if !writes {
        return Gate::On;
    }
    let declared = crate::plugins::with_registry(&app.cfg.plugin_disabled(), |registry| {
        registry
            .manifest(plugin)
            .is_some_and(|manifest| manifest.read_only)
    });
    if declared
        || matches!(
            app.cfg.plugins.get(plugin).and_then(|t| t.get("read_only")),
            Some(toml::Value::Boolean(true))
        )
    {
        return Gate::Off(Some(Msg::PluginReadOnly));
    }
    #[cfg(feature = "git")]
    if repository && !crate::vcs::caps(&app.tab.root).write {
        return Gate::Off(Some(Msg::VcsReadOnly));
    }
    #[cfg(not(feature = "git"))]
    let _ = repository;
    Gate::On
}

impl App {
    /// Recheck immediately before dispatch, including after a dialog or a queued action.
    /// Refusal never takes a session, starts a worker, or changes repository state.
    pub(crate) fn allow_write(&mut self, plugin: &str, writes: bool, repository: bool) -> bool {
        match write_guard(self, plugin, writes, repository) {
            Gate::On => true,
            Gate::Off(reason) => {
                if let Some(msg) = reason {
                    self.flash = Some(crate::i18n::tr(self.lang, msg).into());
                }
                false
            }
        }
    }
}
