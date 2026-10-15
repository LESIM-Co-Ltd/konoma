//! What the host knows about a plugin from its declarations, looked up by id.
//!
//! The host chooses a plugin's screen from what the plugin **declares** (`Manifest::shows`,
//! `converts`, `file_text`, `unavailable`) and from the view it returns, never from its id: a
//! plugin installed under another id (a replacement for a standard one) is drawn the same way as
//! long as it declares and returns the same display types.
//!
//! The declarations come from the registry: **every plugin is registered through [`register`]**
//! (the one place that calls `Registry::register`), which records what the registered manifest
//! declares, standard or not, so a plugin added by any road has its profile from the moment it is
//! in a registry. The standard ones are also in the table before any registry is built. Ids are
//! unique across the plugins of a process (the registry refuses a duplicate), so one table serves
//! all registries.

use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock, RwLock};

use konoma_plugin::registry::{PluginOrigin, RegistryError};
use konoma_plugin::{Manifest, Plugin, Unavailable, ViewKind};

/// The declared character of a plugin's main view.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Profile {
    /// Shows pictures (the host's image pipeline: decoding, zoom, pan, pages).
    pub image: bool,
    /// Shows a document (the host's decorated document widget).
    pub document: bool,
    /// Shows a table.
    pub table: bool,
    /// Shows text (the host's windowed reader).
    pub text: bool,
    /// Draws cells and pictures itself (a canvas).
    pub canvas: bool,
    /// One of the host's own standard plugins (not one a user added): the only ones whose `view`
    /// the host may call on the UI thread (the cheap document representations; see
    /// `App::plugin_view_now`).
    pub standard: bool,
    /// Shows a converted form of the file (no file has the lines it shows).
    pub converts: bool,
    /// Its text view is the file itself.
    pub file_text: bool,
    /// The host believes the `trusted` claim of the SVGs it returns: a **standard** plugin that
    /// draws the SVG itself, from a diagram or a formula, with konoma's own code (mermaid and
    /// math). Decided by where the plugin comes from: the plugins a user adds never have it, and
    /// neither does a standard plugin whose SVG comes from a file or a document.
    pub own_svg: bool,
    /// It is given files but declares no `shows`: the host attaches no screen to them (reported
    /// at startup; a view it returns is explained on screen).
    pub unscreened: bool,
}

impl Profile {
    fn of(m: &Manifest, standard: bool) -> Profile {
        let has = |k: ViewKind| m.shows.contains(&k);
        Profile {
            image: has(ViewKind::Image),
            document: has(ViewKind::Document),
            table: has(ViewKind::Table),
            text: has(ViewKind::Text),
            canvas: has(ViewKind::Canvas),
            standard,
            converts: m.converts,
            file_text: m.file_text,
            unscreened: m.assigns_files_without_shows(),
            own_svg: standard
                && matches!(
                    m.id.as_str(),
                    konoma_mermaid::MERMAID_ID | konoma_math::MATH_ID
                ),
        }
    }
}

/// What is kept for one plugin.
struct Entry {
    profile: Profile,
    unavailable: Unavailable,
    shows: Vec<ViewKind>,
}

impl Entry {
    fn of(m: &Manifest, standard: bool) -> Entry {
        Entry {
            profile: Profile::of(m, standard),
            unavailable: m.unavailable.clone(),
            shows: m.shows.clone(),
        }
    }
}

type Entries = BTreeMap<String, Entry>;

fn table() -> &'static RwLock<Entries> {
    static T: OnceLock<RwLock<Entries>> = OnceLock::new();
    T.get_or_init(|| {
        let mut m = Entries::new();
        for manifest in super::standard_manifests() {
            m.insert(manifest.id.clone(), Entry::of(&manifest, true));
        }
        RwLock::new(m)
    })
}

fn standard_ids() -> &'static Vec<String> {
    static IDS: OnceLock<Vec<String>> = OnceLock::new();
    IDS.get_or_init(|| {
        super::standard_manifests()
            .into_iter()
            .map(|m| m.id)
            .collect()
    })
}

/// Register `plugin` in `registry` and remember what it declares. **The only way the host adds a
/// plugin to a registry** (the standard ones, the stand-ins of the tests, and the plugins a user
/// installs at stage 3 alike), so the profile table cannot fall behind the registry.
pub(crate) fn register(
    registry: &mut konoma_plugin::registry::Registry,
    plugin: Arc<dyn Plugin>,
    origin: PluginOrigin,
) -> Result<(), RegistryError> {
    let id = plugin.manifest().id;
    registry.register(plugin, origin)?;
    // The standard plugins are in the table from the start. What is recorded for the others is
    // the manifest as registered (keys the registry dropped are gone).
    if origin != PluginOrigin::Standard {
        if let Some(manifest) = registry.manifest(&id) {
            record(manifest);
        }
    }
    Ok(())
}

/// Remember what `manifest` (of a plugin that is not a standard one) declares.
pub(crate) fn record(manifest: &Manifest) {
    let mut t = table().write().unwrap_or_else(|e| e.into_inner());
    // A plugin that is not a standard one never takes the place of an id they own.
    if standard_ids().contains(&manifest.id) {
        return;
    }
    let entry = Entry::of(manifest, false);
    if t.get(&manifest.id)
        .map(|e| (&e.profile, &e.unavailable, &e.shows))
        != Some((&entry.profile, &entry.unavailable, &entry.shows))
    {
        t.insert(manifest.id.clone(), entry);
    }
}

/// The declared character of the plugin `id` (all false for an id nobody registered).
pub(crate) fn profile_of(id: &str) -> Profile {
    table()
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .get(id)
        .map(|e| e.profile)
        .unwrap_or_default()
}

/// What the host shows when the plugin `id` shows nothing.
pub(crate) fn unavailable_of(id: &str) -> Unavailable {
    table()
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .get(id)
        .map(|e| e.unavailable.clone())
        .unwrap_or_default()
}

/// The display types the plugin `id` declared (`Manifest::shows`).
pub(crate) fn shows_of(id: &str) -> Vec<ViewKind> {
    table()
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .get(id)
        .map(|e| e.shows.clone())
        .unwrap_or_default()
}
