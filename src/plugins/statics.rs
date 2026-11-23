//! Plugins linked into the program by the code that starts konoma (`konoma::Host`): the standard
//! ones (or not), the ones a program adds, and the standard ones it replaces.
//!
//! The set is decided once, before the first registry is built, and is process-wide (the registry
//! is shared per set of disabled ids: see `shared_registry`). Nothing was installed (a test
//! process, or `Host` was not used) means the plain standard set.

use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock};

use konoma_plugin::Plugin;

/// What `Host::run` hands to the registry builder.
pub(crate) struct StaticSet {
    /// Register the standard plugins (`HostBuilder::standard_plugins`).
    pub standard: bool,
    /// Plugins added next to the standard ones, in the order given.
    pub added: Vec<Arc<dyn Plugin>>,
    /// Standard plugins swapped for another plugin: standard id -> the plugin registered in its
    /// place (at the same position, so a file is decided as before).
    pub replaced: BTreeMap<String, Arc<dyn Plugin>>,
}

static SET: OnceLock<StaticSet> = OnceLock::new();

/// Fix the set for this process. Fails when one was already installed (`run` called twice).
pub(crate) fn install(set: StaticSet) -> Result<(), &'static str> {
    SET.set(set)
        .map_err(|_| "konoma was already started in this process")
}

/// The set `Host::run` installed (`None` = nothing: a test process, or no `Host`).
pub(crate) fn current() -> Option<&'static StaticSet> {
    SET.get()
}

/// Whether the standard plugin `id` was replaced.
pub(crate) fn is_replaced(id: &str) -> bool {
    SET.get().is_some_and(|s| s.replaced.contains_key(id))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use konoma_plugin::prelude::*;
    use konoma_plugin::registry::PluginOrigin;

    use super::{super::build_registry_with, StaticSet};

    struct Fake(&'static str);

    impl Plugin for Fake {
        fn manifest(&self) -> Manifest {
            Manifest::new(self.0, "0.0.1")
        }
        fn open(&self, _: Input, _: &OpenCx) -> Result<Box<dyn Session>, Failure> {
            Err(Failure::unsupported(t!(en: "none", ja: "なし")))
        }
    }

    fn set(standard: bool, added: &[&'static str], replaced: &[(&str, &'static str)]) -> StaticSet {
        StaticSet {
            standard,
            added: added
                .iter()
                .map(|id| Arc::new(Fake(id)) as Arc<dyn Plugin>)
                .collect(),
            replaced: replaced
                .iter()
                .map(|(t, id)| (t.to_string(), Arc::new(Fake(id)) as Arc<dyn Plugin>))
                .collect::<BTreeMap<_, _>>(),
        }
    }

    #[test]
    fn no_set_is_the_plain_standard_registry() {
        let r = build_registry_with(&[], None);
        assert_eq!(r.origin("markdown"), Some(PluginOrigin::Standard));
        assert!(r.manifest("text").is_some());
    }

    #[test]
    fn an_added_plugin_is_registered_as_a_user_plugin_after_the_standard_ones() {
        let s = set(true, &["mine"], &[]);
        let r = build_registry_with(&[], Some(&s));
        assert_eq!(r.origin("mine"), Some(PluginOrigin::User));
        assert_eq!(r.origin("markdown"), Some(PluginOrigin::Standard));
        let ids = r.ids();
        let pos = |id: &str| ids.iter().position(|i| *i == id).unwrap();
        assert!(pos("mine") > pos("pdf"));
    }

    #[test]
    fn a_replacement_stands_in_the_place_of_the_standard_plugin() {
        let plain = build_registry_with(&[], None);
        let s = set(true, &[], &[("markdown", "my-markdown")]);
        let r = build_registry_with(&[], Some(&s));
        assert!(r.manifest("markdown").is_none());
        assert_eq!(r.origin("my-markdown"), Some(PluginOrigin::User));
        let at = |r: &konoma_plugin::registry::Registry, id: &str| {
            r.ids().iter().position(|i| *i == id).unwrap()
        };
        assert_eq!(at(&plain, "markdown"), at(&r, "my-markdown"));
    }

    #[test]
    fn without_the_standard_plugins_only_the_internal_and_added_ones_remain() {
        let s = set(false, &["mine"], &[]);
        let r = build_registry_with(&[], Some(&s));
        assert!(r.manifest("markdown").is_none());
        assert!(r.manifest("image").is_none());
        assert!(r.manifest("mine").is_some());
        assert!(r.manifest("text").is_some() && r.manifest("can-not-preview").is_some());
    }

    #[test]
    fn config_can_still_switch_a_standard_plugin_off() {
        let s = set(true, &["mine"], &[]);
        let r = build_registry_with(&["image".to_string(), "mine".to_string()], Some(&s));
        assert!(!r.is_enabled("image"));
        assert!(r.manifest("image").is_some());
    }
}
