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

/// The id of the operation that jumps to the project screen `screen`: `goto_<screen id>`, written
/// like the other operation ids (`diff_all`, `worktree_diff`) and bound in the same
/// `[keys.<plugin id>]` table (`<key> = "goto_branches"` rebinds it, `<key> = "noop"` removes the
/// key it has).
pub fn jump_op_id(screen: &str) -> String {
    format!("goto_{screen}")
}

/// One screen-jump operation of a plugin with the key it has now (`None` = removed). Only a
/// screen that declares a default jump key has one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JumpBinding {
    /// The operation id ([`jump_op_id`]).
    pub op: String,
    /// The id of the screen it jumps to.
    pub screen: String,
    pub key: Option<KeyPress>,
}

/// The jump operations a manifest declares: its project's screens that are reached by a key
/// (a pushed screen has none), as `(operation id, screen id, default key)`.
fn jump_ops(manifest: &Manifest) -> Vec<(String, String, KeyPress)> {
    manifest
        .project
        .iter()
        .flat_map(|p| p.screens.iter())
        .filter(|s| !s.pushed)
        .filter_map(|s| Some((jump_op_id(&s.id), s.id.clone(), KeyPress::ch(s.key?))))
        .collect()
}

/// Why `key` cannot be a screen's jump key, or `None` when it can: a jump takes one plain
/// character, and not one the canvas or its lists already act on (those win over the jump, so the
/// hint would show a key that does nothing). The project entry key `p` may be reused here, as for
/// a declared jump key.
fn jump_key_refusal(key: &keys::KeySpec) -> Option<String> {
    let keys::KeyName::Char(c) = key.code else {
        return Some("a screen jump takes one plain character".to_string());
    };
    if key.ctrl {
        return Some("a screen jump takes one plain character".to_string());
    }
    let taken = ["canvas", "list"]
        .into_iter()
        .any(|ctx| keys::reserved(ctx).into_iter().any(|(k, _)| k == *key));
    (taken && c != 'p').then(|| format!("`{c}` is the host's key on a project screen"))
}

/// The jump operations of a plugin with their keys: the manifest's defaults, replaced by `user`
/// (the plugin's `[keys.<id>]` table). The same table [`bind_actions`] reads: `<key> =
/// "noop"` takes the key from the jump that has it, `<key> = "goto_<screen>"` gives the jump
/// that key. A rebinding that cannot be kept (a key the host acts on, a key another jump has)
/// leaves the jump on its default key and is noted; the notes of an unparseable key and a second
/// key for one operation are [`bind_actions`]'s, which reads the same entries.
pub fn bind_jumps(
    id: &str,
    manifest: &Manifest,
    user: Option<&HashMap<String, String>>,
) -> (Vec<JumpBinding>, Vec<KeyNote>) {
    let mut notes = Vec::new();
    let mut jumps: Vec<JumpBinding> = jump_ops(manifest)
        .into_iter()
        .map(|(op, screen, key)| JumpBinding {
            op,
            screen,
            key: Some(key),
        })
        .collect();
    let Some(user) = user else {
        return (jumps, notes);
    };
    let mut entries: Vec<(&String, &String)> = user.iter().collect();
    entries.sort();
    let mut chosen: Vec<(&str, KeyPress, &String)> = Vec::new();
    for (key, action) in entries {
        let Ok(spec) = keys::parse_key(key) else {
            continue;
        };
        if action == "noop" || action == "disabled" {
            for jump in jumps.iter_mut().filter(|j| j.key == Some(keypress_of(spec))) {
                jump.key = None;
            }
        } else if jumps.iter().any(|j| j.op == *action) {
            if chosen.iter().any(|(op, _, _)| op == action) {
                continue;
            }
            if let Some(why) = jump_key_refusal(&spec) {
                notes.push(KeyNote::BadKey {
                    plugin: id.to_string(),
                    key: key.clone(),
                    why,
                });
                continue;
            }
            chosen.push((action, keypress_of(spec), key));
        }
    }
    // The chosen jumps leave their defaults first, then each takes its key unless another jump
    // has it; a jump that cannot gets its default back when that is free.
    let defaults = jump_ops(manifest);
    for (op, _, _) in &chosen {
        if let Some(jump) = jumps.iter_mut().find(|j| j.op == *op) {
            jump.key = None;
        }
    }
    for (op, key, written) in &chosen {
        let Some(slot) = jumps.iter().position(|j| j.op == *op) else {
            continue;
        };
        if let Some(other) = jumps.iter().find(|j| j.key == Some(*key)) {
            notes.push(KeyNote::BadKey {
                plugin: id.to_string(),
                key: (*written).clone(),
                why: format!("`{}` already jumps on it", other.op),
            });
            let back = defaults.iter().find(|(d, _, _)| d == op).map(|(_, _, k)| *k);
            if let Some(back) = back.filter(|b| !jumps.iter().any(|j| j.key == Some(*b))) {
                jumps[slot].key = Some(back);
            }
            continue;
        }
        jumps[slot].key = Some(*key);
    }
    (jumps, notes)
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
        .filter(|a| !a.copy)
        .map(|a| (a.id.clone(), a.key.clone()))
        .collect();
    if let Some(user) = user {
        apply_user_table(id, &mut keys, user, manifest, &mut notes);
    }
    let acts = manifest
        .actions
        .iter()
        .map(|a| ActBinding {
            copy_key: a
                .copy
                .then(|| a.key.as_deref()?.chars().next().map(KeyPress::ch))
                .flatten(),
            hint_group: a
                .hint_group
                .as_ref()
                .and_then(|id| manifest.hint_groups.iter().find(|g| g.id == *id))
                .map(|group| {
                    let mut group = group.clone();
                    group.members = manifest
                        .actions
                        .iter()
                        .filter(|m| m.hint_group.as_deref() == Some(group.id.as_str()))
                        .count();
                    (id.to_string(), group)
                }),
            act: handle(&a.id),
            key: (!a.copy)
                .then(|| {
                    keys.iter()
                        .find(|(i, _)| *i == a.id)
                        .and_then(|(_, k)| k.as_deref())
                        .and_then(|k| keys::parse_key(k).ok())
                        .map(keypress_of)
                })
                .flatten(),
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
        // A screen's jump is an operation of the same table ([`bind_jumps`] binds it).
        let is_jump = jump_ops(manifest).iter().any(|(op, _, _)| op == action);
        if is_jump && keys::parse_key(key).is_ok() {
            if chosen.iter().any(|(a, _)| a == action) {
                notes.push(KeyNote::BadKey {
                    plugin: id.to_string(),
                    key: key.clone(),
                    why: format!("`{action}` already has a key here; one key per action"),
                });
            } else {
                chosen.push((action.clone(), key.clone()));
            }
            continue;
        }
        if !is_jump && !manifest.actions.iter().any(|a| a.id == *action) {
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
        for act in probe.actions.iter_mut().filter(|a| !a.copy) {
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
