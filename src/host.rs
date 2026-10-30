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

use anyhow::{bail, Result};
use konoma_plugin::Plugin;

use crate::plugins::{self, statics::StaticSet};

/// Entry point of the library: [`Host::builder`].
pub struct Host;

impl Host {
    /// Start assembling the plugins of a program.
    pub fn builder() -> HostBuilder {
        HostBuilder::default()
    }
}

/// The plugins a program links in. Nothing is checked until [`HostBuilder::run`], so the order of
/// the calls does not matter.
#[derive(Default)]
pub struct HostBuilder {
    standard: bool,
    added: Vec<Arc<dyn Plugin>>,
    replaced: Vec<(String, Arc<dyn Plugin>)>,
}

impl HostBuilder {
    /// Include the standard plugins (markdown, image, pdf, ...). Without this call only the
    /// plugins given to [`plugin`](Self::plugin) are linked in.
    pub fn standard_plugins(mut self) -> Self {
        self.standard = true;
        self
    }

    /// Add a plugin. Its id must not be one that is already taken (a standard plugin's, or one
    /// added before); `run` fails otherwise.
    pub fn plugin(mut self, plugin: impl Plugin) -> Self {
        self.added.push(Arc::new(plugin));
        self
    }

    /// Put `plugin` in the place of the standard plugin `id` (it keeps the place in the order that
    /// decides which plugin shows a file). `run` fails when there is no such plugin.
    pub fn replace(mut self, id: &str, plugin: impl Plugin) -> Self {
        self.replaced.push((id.to_string(), Arc::new(plugin)));
        self
    }

    /// Check the ids and fix what is linked in. Touches nothing outside this value, so a failure
    /// here means konoma never started.
    fn resolve(self) -> Result<StaticSet> {
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
        let mut replaced = std::collections::BTreeMap::new();
        for (target, plugin) in self.replaced {
            if replaced.contains_key(&target) {
                bail!("the plugin `{target}` is replaced more than once");
            }
            if !standard_ids.contains(&target) {
                bail!("cannot replace `{target}`: there is no standard plugin with that id");
            }
            // The replacement leaves the id it stands for; its own id must be free.
            taken.remove(&target);
            let id = plugin.manifest().id;
            if !taken.insert(id.clone()) {
                bail!("plugin id `{id}` is already taken");
            }
            replaced.insert(target, plugin);
        }
        for plugin in &self.added {
            let id = plugin.manifest().id;
            if !taken.insert(id.clone()) {
                bail!("plugin id `{id}` is already taken");
            }
        }
        Ok(StaticSet {
            standard: self.standard,
            added: self.added,
            replaced,
        })
    }

    /// Start konoma with these plugins, as the `konoma` command does with its arguments and the
    /// terminal. Returns when konoma exits; a bad plugin set is an error before anything else
    /// happens (no terminal is touched).
    pub fn run(self) -> Result<()> {
        let set = self.resolve()?;
        // Declared after the check above, which touches nothing, so a program (or a test) that
        // only learns its plugin set is wrong never becomes "the application". It is done here and
        // not in the caller's `main` so that every program that runs konoma -- the `konoma`
        // command and the ones `konoma plugin install` builds alike -- keeps the promise that the
        // crates' test-only seams are off in the application, and so that nobody can forget it.
        // It stays ahead of everything else that matters (the drawing child, arguments, the
        // configuration, the terminal).
        konoma_base::process::mark_app_main();
        if plugins::statics::install(set).is_err() {
            bail!("konoma was already started in this process");
        }
        crate::run_application()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use konoma_plugin::prelude::*;

    struct Fake(&'static str);

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
    fn standard_only() {
        let s = Host::builder().standard_plugins().resolve().unwrap();
        assert!(s.standard && s.added.is_empty() && s.replaced.is_empty());
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
