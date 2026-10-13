//! What the host knows about a plugin from its declarations, looked up by id.
//!
//! The host chooses a plugin's screen from what the plugin **declares** (`Manifest::shows`,
//! `converts`, `file_text`, `unavailable`) and from the view it returns, never from its id: a
//! plugin installed under another id (a replacement for a standard one) is drawn the same way as
//! long as it declares and returns the same display types.
//!
//! The table is filled whenever a registry is built (`build_registry`) with every plugin it
//! registers, standard or not; the standard ones are in it from the first lookup. Ids are unique
//! across the plugins of a process (the registry refuses a duplicate), so one table serves all
//! registries.

use std::collections::BTreeMap;
use std::sync::{OnceLock, RwLock};

use konoma_plugin::{Manifest, Unavailable, ViewKind};

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
    /// Shows a converted form of the file (no file has the lines it shows).
    pub converts: bool,
    /// Its text view is the file itself.
    pub file_text: bool,
    /// The host believes the `trusted` claim of the SVGs it returns: a **standard** plugin that
    /// draws the SVG itself, from a diagram or a formula, with konoma's own code (mermaid and
    /// math). Decided by where the plugin comes from: the plugins a user adds never have it, and
    /// neither does a standard plugin whose SVG comes from a file or a document.
    pub own_svg: bool,
}

impl Profile {
    fn of(m: &Manifest, standard: bool) -> Profile {
        let has = |k: ViewKind| m.shows.contains(&k);
        Profile {
            image: has(ViewKind::Image),
            document: has(ViewKind::Document),
            table: has(ViewKind::Table),
            text: has(ViewKind::Text),
            converts: m.converts,
            file_text: m.file_text,
            own_svg: standard
                && matches!(
                    m.id.as_str(),
                    konoma_mermaid::MERMAID_ID | konoma_math::MATH_ID
                ),
        }
    }
}

type Entries = BTreeMap<String, (Profile, Unavailable)>;

fn table() -> &'static RwLock<Entries> {
    static T: OnceLock<RwLock<Entries>> = OnceLock::new();
    T.get_or_init(|| {
        let mut m = Entries::new();
        for manifest in super::standard_manifests() {
            m.insert(
                manifest.id.clone(),
                (Profile::of(&manifest, true), manifest.unavailable.clone()),
            );
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

/// Remember what `manifest` declares (called for every plugin a registry registers). Only the
/// plugins a test adds need it today; a registered user plugin (stage 3) calls it the same way.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn record(manifest: &Manifest) {
    let entry = (Profile::of(manifest, false), manifest.unavailable.clone());
    let mut t = table().write().unwrap_or_else(|e| e.into_inner());
    // The standard plugins are in the table from the start, and a plugin that is not one never
    // takes the place of an id they own (the registry refuses a duplicate id as well).
    if standard_ids().contains(&manifest.id) {
        return;
    }
    if t.get(&manifest.id) != Some(&entry) {
        t.insert(manifest.id.clone(), entry);
    }
}

/// The declared character of the plugin `id` (all false for an id nobody registered).
pub(crate) fn profile_of(id: &str) -> Profile {
    table()
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .get(id)
        .map(|(p, _)| *p)
        .unwrap_or_default()
}

/// What the host shows when the plugin `id` shows nothing.
pub(crate) fn unavailable_of(id: &str) -> Unavailable {
    table()
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .get(id)
        .map(|(_, u)| u.clone())
        .unwrap_or_default()
}
