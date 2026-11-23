//! The declared actions of a plugin as a host binds them: the key each one runs on, from the
//! manifest's default and the user's `[keys.<plugin id>]` table. One reading for konoma and the
//! standalone app, so a rebinding means the same in both.
//!
//! `[keys.<id>]`: `<key> = "<action id>"` gives the action that key (instead of its default),
//! `<key> = "noop"` takes the key from whichever action has it. A key that cannot be kept is
//! dropped (the action stays reachable from the `Space` menu) and a [`KeyNote`] says why.

use std::collections::HashMap;

use konoma_host_core::keymap::KeyPress;

use super::acts::ActBinding;
use crate::keys::{self, KeyName as PKeyName, KeySpec};
use crate::Manifest;

/// What is wrong with a `[keys.<plugin>]` table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyNote {
    /// The table names an action the plugin does not declare.
    UnknownAction { plugin: String, action: String },
    /// The table has a key that is not usable (does not parse, reserved, taken).
    BadKey {
        plugin: String,
        key: String,
        why: String,
    },
}

impl KeyNote {
    /// The words of the note (one piece of the startup message), the same in konoma and in the
    /// standalone app.
    pub fn text(&self, lang: konoma_host_core::lang::Lang) -> String {
        use konoma_host_core::help_msg::CoreMsg;
        match self {
            KeyNote::UnknownAction { plugin, action } => CoreMsg::PluginKeyUnknownAction
                .tr(lang)
                .replace("{plugin}", plugin)
                .replace("{action}", action),
            KeyNote::BadKey { plugin, key, why } => CoreMsg::PluginKeyBad
                .tr(lang)
                .replace("{plugin}", plugin)
                .replace("{key}", key)
                .replace("{why}", why),
        }
    }
}

/// The startup message for the notes: `plugins: <note> / <note>` (`None` with no note).
pub fn key_notes_line(notes: &[KeyNote], lang: konoma_host_core::lang::Lang) -> Option<String> {
    if notes.is_empty() {
        return None;
    }
    let texts: Vec<String> = notes.iter().map(|n| n.text(lang)).collect();
    Some(format!(
        "{}: {}",
        konoma_host_core::help_msg::CoreMsg::PluginsHead.tr(lang),
        texts.join(" / ")
    ))
}

/// The key a plugin's `KeySpec` is on the host side.
pub fn keypress_of(spec: KeySpec) -> KeyPress {
    use crossterm::event::KeyCode;
    let code = match spec.code {
        PKeyName::Char(c) => KeyCode::Char(c),
        PKeyName::Enter => KeyCode::Enter,
        PKeyName::Tab => KeyCode::Tab,
        PKeyName::BackTab => KeyCode::BackTab,
        PKeyName::Esc => KeyCode::Esc,
        PKeyName::Backspace => KeyCode::Backspace,
        PKeyName::Delete => KeyCode::Delete,
        PKeyName::Up => KeyCode::Up,
        PKeyName::Down => KeyCode::Down,
        PKeyName::Left => KeyCode::Left,
        PKeyName::Right => KeyCode::Right,
        PKeyName::Home => KeyCode::Home,
        PKeyName::End => KeyCode::End,
        PKeyName::PageUp => KeyCode::PageUp,
        PKeyName::PageDown => KeyCode::PageDown,
        PKeyName::F(n) => KeyCode::F(n),
    };
    KeyPress {
        code,
        ctrl: spec.ctrl,
    }
}

/// The declared actions of plugin `id`, in declaration order, with their keys: the manifest's,
/// replaced by `user` (the plugin's `[keys.<id>]` table, if the user wrote one). `handle` makes
/// the host's handle of an action from its id.
pub fn bind_actions<H>(
    id: &str,
    manifest: &Manifest,
    user: Option<&HashMap<String, String>>,
    handle: impl Fn(&str) -> H,
) -> (Vec<ActBinding<H>>, Vec<KeyNote>) {
    let mut notes = Vec::new();
    let mut keys: Vec<(String, Option<String>)> = manifest
        .actions
        .iter()
        .map(|a| (a.id.clone(), a.key.clone()))
        .collect();
    if let Some(user) = user {
        apply_user_table(id, &mut keys, user, manifest, &mut notes);
    }
    let acts = manifest
        .actions
        .iter()
        .map(|a| ActBinding {
            act: handle(&a.id),
            key: keys
                .iter()
                .find(|(i, _)| *i == a.id)
                .and_then(|(_, k)| k.as_deref())
                .and_then(|k| keys::parse_key(k).ok())
                .map(keypress_of),
            context: a.screen.clone(),
            label: a.label.clone(),
            help: a.help.clone(),
            footer: a.footer,
            writes: a.writes,
        })
        .collect();
    (acts, notes)
}

fn apply_user_table(
    id: &str,
    keys: &mut [(String, Option<String>)],
    user: &HashMap<String, String>,
    manifest: &Manifest,
    notes: &mut Vec<KeyNote>,
) {
    let mut entries: Vec<(&String, &String)> = user.iter().collect();
    entries.sort();
    // Action -> the key the user chose for it.
    let mut chosen: Vec<(String, String)> = Vec::new();
    for (key, action) in entries {
        if action == "noop" || action == "disabled" {
            // Taking a key away: whichever action has it by default loses it.
            if let Ok(spec) = keys::parse_key(key) {
                for (_, k) in keys.iter_mut() {
                    if k.as_deref().and_then(|k| keys::parse_key(k).ok()) == Some(spec) {
                        *k = None;
                    }
                }
            } else {
                notes.push(KeyNote::BadKey {
                    plugin: id.to_string(),
                    key: key.clone(),
                    why: "not a key".to_string(),
                });
            }
            continue;
        }
        if !manifest.actions.iter().any(|a| a.id == *action) {
            notes.push(KeyNote::UnknownAction {
                plugin: id.to_string(),
                action: action.clone(),
            });
            continue;
        }
        if let Err(e) = keys::parse_key(key) {
            notes.push(KeyNote::BadKey {
                plugin: id.to_string(),
                key: key.clone(),
                why: e.to_string(),
            });
            continue;
        }
        if chosen.iter().any(|(a, _)| a == action) {
            notes.push(KeyNote::BadKey {
                plugin: id.to_string(),
                key: key.clone(),
                why: format!("`{action}` already has a key here; one key per action"),
            });
            continue;
        }
        chosen.push((action.clone(), key.clone()));
    }
    for (action, key) in &chosen {
        if let Some((_, slot)) = keys.iter_mut().find(|(a, _)| a == action) {
            *slot = Some(key.clone());
        }
    }
    // Check the keys as they will be, with the rules the registry uses for a manifest. A key that
    // fails is dropped (the action stays in the `Space` menu).
    if !chosen.is_empty() {
        let mut probe = manifest.clone();
        for act in &mut probe.actions {
            act.key = keys
                .iter()
                .find(|(i, _)| *i == act.id)
                .and_then(|(_, k)| k.clone());
        }
        for issue in keys::validate(&probe) {
            if let Some((_, slot)) = keys.iter_mut().find(|(a, _)| *a == issue.act) {
                let bad = slot.take();
                notes.push(KeyNote::BadKey {
                    plugin: id.to_string(),
                    key: bad.unwrap_or_default(),
                    why: issue.to_string(),
                });
            }
        }
    }
}
