//! The one rule that says which declared actions are shown as hints.
//!
//! The footer, the help and the `Space` menu of the host, and [`crate::testing::check_hints`] of a
//! plugin's own tests, all ask [`hints`]. An action is a hint only while the session says it is
//! [`enabled`](crate::Session::enabled), so a key is never offered while it would do nothing. The
//! manifest is read **sanitized** (see [`crate::keys::sanitize`]): an action that lost its key is
//! still listed (the menu offers it) but has no key.

use crate::keys::{parse_key, sanitize, KeySpec};
use crate::manifest::Manifest;
use crate::text::Tr;

/// One shown action.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hint {
    /// The action id.
    pub act: String,
    /// The key it is bound to, if it has one.
    pub key: Option<KeySpec>,
    pub label: Tr,
    pub help: Tr,
    /// The footer priority (`None` keeps it out of the footer).
    pub footer: Option<u8>,
    /// The context of the key (`None` = the main view).
    pub context: Option<String>,
    pub writes: bool,
}

/// The actions of `manifest` that work now (`enabled` answers per action id), in declaration order.
pub fn hints(manifest: &Manifest, enabled: &dyn Fn(&str) -> bool) -> Vec<Hint> {
    let (clean, _) = sanitize(manifest);
    clean
        .actions
        .iter()
        .filter(|a| enabled(&a.id))
        .map(|a| Hint {
            act: a.id.clone(),
            key: a.key.as_deref().and_then(|k| parse_key(k).ok()),
            label: a.label.clone(),
            help: a.help.clone(),
            footer: a.footer,
            context: a.screen.clone(),
            writes: a.writes,
        })
        .collect()
}
