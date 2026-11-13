//! The old per-screen key tables (`[keys."git.changes"]`, `[keys."jj.log"]` and the older
//! `[keys.git_changes]` that covers both backends) read as old names of the git and jj plugins'
//! operations.
//!
//! Before the version-control screens became plugin screens, a key on a screen was bound to a
//! host action (`git_stage`, `jj_sync`, ...) in these tables. On the project canvas the plugin's
//! own operations (`stage`, `sync`, `goto_branches`, ...) are bound in `[keys.<plugin id>]`, so
//! an old entry is **translated** into that table (the table `bind_actions` and `bind_jumps`
//! read: one path, no second binder) and noted once at startup through the keymap's
//! `renames` (what to write instead) or `warnings` (no effect, and why).
//!
//! [`ROWS`] is the one explicit table: old action name, then what each backend does with it.
//! Everything an old table can say that is not in [`ROWS`] is not a hub action (`refresh`,
//! `list_filter_start`, ...) and stays in the host table untouched.
//!
//! Rules, in the order they apply:
//! - Entries are read in the order of the screens, then the backends, then the sorted keys;
//!   a `[keys.git_<screen>]` entry is read for both backends and a `[keys."<id>.<screen>"]` entry
//!   of the same key wins over it for its backend (noted).
//! - A mapped entry goes into `[keys.<plugin id>]` as `<key> = "<operation id>"` when that
//!   operation belongs to the table's screen (a screen jump belongs to every screen). A key or
//!   an operation that `[keys.<plugin id>]` already has wins: the old entry is dropped (noted).
//!   Of two old entries that want the same key or operation, the first one read wins (noted).
//! - A translated entry leaves the host table, so the inert host action no longer shadows the
//!   canvas key. What the host does itself with a fixed key ([`Target::Host`]) stays.
//! - `<key> = "noop"` is translated to `[keys.<plugin id>] <key> = "noop"` when the key is the
//!   default of an operation of the plugin and every one of those operations is on the table's
//!   screen (or is a screen jump). The plugin table cannot say "only on this screen": when an
//!   operation of another screen has the same default key, the entry stays in the host table,
//!   which is exact for its own screen.

use std::collections::{BTreeMap, HashMap};

use konoma_plugin::host::bind::{bind_jumps, keypress_of};
use konoma_plugin::keys;
use konoma_plugin::project::ActContext;
use konoma_plugin::registry::Registry;
use konoma_plugin::Manifest;

use crate::keymap::{Backend, Screen};

/// What one backend does with an old action name.
pub(crate) enum Target {
    /// The plugin operations (ids of its `[[actions]]` or `goto_<screen>`) that do the same, one
    /// per screen they belong to; the one of the table's screen is taken.
    Op(&'static [&'static str]),
    /// The host does it itself with a fixed key; the entry stays in the host table.
    Host,
    /// No counterpart: the entry is dropped and the reason is noted.
    None(&'static str),
}

pub(crate) struct Row {
    pub old: &'static str,
    pub git: Target,
    pub jj: Target,
}

const READ_ONLY: Target = Target::None("konoma only reads a jj repository");
const DIFF_VIEW: Target =
    Target::None("a diff-view action; a project screen has no counterpart for it");
const COPY: Target =
    Target::None("the copy operations take their keys from the `y` menu, not from a table");
const NO_BASE: Target = Target::None("jj has no graph base to pin");
const NO_WORKTREES: Target = Target::None("jj has no worktrees screen");
const NO_JUMP: Target =
    Target::None("no screen jump exists for it (only Changes, Branches and Worktrees have one)");

/// Every old hub action that an old screen table accepts (`keymap::action_from_str`'s git
/// arms), with what the git and the jj plugin do with it now. A test enumerates the arms from
/// the source, so an action added there without a row here fails it.
pub(crate) const ROWS: &[Row] = &[
    Row {
        old: "git_graph_picker_apply",
        git: Target::Op(&["picker_apply"]),
        jj: Target::Op(&["picker_apply"]),
    },
    Row {
        old: "git_diff_discard",
        git: DIFF_VIEW,
        jj: DIFF_VIEW,
    },
    Row {
        old: "cycle_diff_view",
        git: DIFF_VIEW,
        jj: DIFF_VIEW,
    },
    Row {
        old: "toggle_follow_diff_scope",
        git: DIFF_VIEW,
        jj: DIFF_VIEW,
    },
    Row {
        old: "diff_next_change",
        git: DIFF_VIEW,
        jj: DIFF_VIEW,
    },
    Row {
        old: "diff_prev_change",
        git: DIFF_VIEW,
        jj: DIFF_VIEW,
    },
    Row {
        old: "git_stage",
        git: Target::Op(&["stage"]),
        jj: READ_ONLY,
    },
    Row {
        old: "git_unstage",
        git: Target::Op(&["unstage"]),
        jj: READ_ONLY,
    },
    Row {
        old: "git_stage_all",
        git: Target::Op(&["stage_all"]),
        jj: READ_ONLY,
    },
    Row {
        old: "git_unstage_all",
        git: Target::Op(&["unstage_all"]),
        jj: READ_ONLY,
    },
    Row {
        old: "git_discard",
        git: Target::Op(&["discard"]),
        jj: READ_ONLY,
    },
    Row {
        old: "git_commit",
        git: Target::Op(&["commit"]),
        jj: READ_ONLY,
    },
    Row {
        old: "git_worktree_diff",
        git: Target::Op(&["diff_all"]),
        jj: Target::Op(&["diff_all"]),
    },
    Row {
        old: "git_open_log",
        git: NO_JUMP,
        jj: NO_JUMP,
    },
    Row {
        old: "git_open_graph",
        git: NO_JUMP,
        jj: NO_JUMP,
    },
    Row {
        old: "git_open_branches",
        git: Target::Op(&["goto_branches"]),
        jj: Target::Op(&["goto_branches"]),
    },
    Row {
        old: "git_launch_tool",
        git: Target::Host,
        jj: Target::Host,
    },
    Row {
        old: "git_open_selected_diff",
        git: Target::Op(&["diff"]),
        jj: Target::Op(&["diff"]),
    },
    Row {
        old: "git_open_detail",
        git: Target::Op(&["detail", "graph_detail"]),
        jj: Target::Op(&["detail", "graph_detail"]),
    },
    Row {
        old: "git_graph_set_base",
        git: Target::Op(&["graph_set_base"]),
        jj: NO_BASE,
    },
    Row {
        old: "git_graph_clear_base",
        git: Target::Op(&["graph_clear_base"]),
        jj: NO_BASE,
    },
    Row {
        old: "git_graph_toggle_all",
        git: Target::None("git's graph always shows every commit"),
        jj: Target::Op(&["graph_all"]),
    },
    Row {
        old: "jj_sync",
        git: Target::None("this is not a jj repository"),
        jj: Target::Op(&["sync"]),
    },
    Row {
        old: "git_graph_open_picker",
        git: Target::Op(&["graph_picker"]),
        jj: Target::Op(&["graph_picker"]),
    },
    Row {
        old: "git_graph_picker_toggle",
        git: Target::Op(&["picker_toggle"]),
        jj: Target::Op(&["picker_toggle"]),
    },
    Row {
        old: "git_graph_picker_all",
        git: Target::Op(&["picker_all"]),
        jj: Target::Op(&["picker_all"]),
    },
    Row {
        old: "git_graph_picker_current_only",
        git: Target::Op(&["picker_current"]),
        jj: Target::Op(&["picker_current"]),
    },
    Row {
        old: "git_graph_picker_move_up",
        git: Target::Op(&["picker_up"]),
        jj: Target::Op(&["picker_up"]),
    },
    Row {
        old: "git_graph_picker_move_down",
        git: Target::Op(&["picker_down"]),
        jj: Target::Op(&["picker_down"]),
    },
    Row {
        old: "git_screen_next",
        git: Target::None("it has no effect on a project screen"),
        jj: Target::None("it has no effect on a project screen"),
    },
    Row {
        old: "git_screen_prev",
        git: Target::None("it has no effect on a project screen"),
        jj: Target::None("it has no effect on a project screen"),
    },
    Row {
        old: "branch_filter_start",
        git: Target::Host,
        jj: Target::Host,
    },
    Row {
        old: "branch_checkout",
        git: Target::Op(&["checkout"]),
        jj: READ_ONLY,
    },
    Row {
        old: "branch_create",
        git: Target::Op(&["create_branch"]),
        jj: READ_ONLY,
    },
    Row {
        old: "branch_delete",
        git: Target::Op(&["delete_branch"]),
        jj: READ_ONLY,
    },
    Row {
        old: "git_open_worktrees",
        git: Target::Op(&["goto_worktrees"]),
        jj: NO_WORKTREES,
    },
    Row {
        old: "worktree_filter_start",
        git: Target::Host,
        jj: Target::Host,
    },
    Row {
        old: "worktree_goto",
        git: Target::Op(&["worktree_switch"]),
        jj: NO_WORKTREES,
    },
    Row {
        old: "worktree_goto_new_tab",
        git: Target::None("the git plugin has no operation that opens a worktree in a new tab"),
        jj: NO_WORKTREES,
    },
    Row {
        old: "worktree_close",
        git: Target::Host,
        jj: Target::Host,
    },
    Row {
        old: "worktree_show_changes",
        git: Target::Op(&["worktree_changes"]),
        jj: NO_WORKTREES,
    },
    Row {
        old: "worktree_create",
        git: Target::Op(&["worktree_create"]),
        jj: NO_WORKTREES,
    },
    Row {
        old: "git_copy_short_hash",
        git: COPY,
        jj: COPY,
    },
    Row {
        old: "git_copy_full_hash",
        git: COPY,
        jj: COPY,
    },
    Row {
        old: "git_copy_subject",
        git: COPY,
        jj: COPY,
    },
    Row {
        old: "git_copy_message",
        git: COPY,
        jj: COPY,
    },
    Row {
        old: "git_copy_author",
        git: COPY,
        jj: COPY,
    },
    Row {
        old: "git_copy_date",
        git: COPY,
        jj: COPY,
    },
    Row {
        old: "copy_branch_name",
        git: COPY,
        jj: COPY,
    },
    Row {
        old: "git_close",
        git: Target::Host,
        jj: Target::Host,
    },
];

/// What the old entries of the config come to: notes for the keymap's startup report.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Translation {
    /// `(what was written, what to write instead)`: one per translated entry.
    pub renames: Vec<(String, String)>,
    /// An old entry that has no effect (and why), or that lost to another entry.
    pub warnings: Vec<String>,
}

enum Outcome {
    /// Translated into `[keys.<plugin id>] <key> = "<operation>"` (`"noop"` for a removal).
    Moved(String),
    /// Stays in the host table (the host does it itself, or it is not a hub action).
    Keep,
    /// No effect, with the reason.
    NoEffect(String),
}

pub(crate) fn act_screen(act: &konoma_plugin::Act) -> Option<&str> {
    match act.project_context.as_ref()? {
        ActContext::Screen { screen, .. } => Some(screen),
        _ => None,
    }
}

/// Whether `op` is an operation of `manifest` usable on `screen`: `Ok` or the reason it is not.
pub(crate) fn op_on_screen(
    manifest: &Manifest,
    id: &str,
    op: &str,
    screen: Screen,
) -> Result<(), String> {
    let (jumps, _) = bind_jumps(id, manifest, None);
    if jumps.iter().any(|j| j.op == op) {
        return Ok(());
    }
    let Some(act) = manifest.actions.iter().find(|a| !a.copy && a.id == op) else {
        return Err(format!("the {id} plugin has no `{op}` operation"));
    };
    match act_screen(act) {
        Some(s) if s == screen.name() => Ok(()),
        Some(s) => Err(format!(
            "`{op}` is an operation of the {s} screen, not of {}",
            screen.name()
        )),
        None => Ok(()),
    }
}

fn classify(manifest: &Manifest, id: &str, screen: Screen, key: &str, action: &str) -> Outcome {
    let Ok(spec) = keys::parse_key(key) else {
        return Outcome::Keep;
    };
    if action == "noop" || action == "disabled" {
        let (jumps, _) = bind_jumps(id, manifest, None);
        let mut on_screen = false;
        let mut elsewhere = false;
        for act in manifest.actions.iter().filter(|a| !a.copy) {
            if act.key.as_deref().and_then(|k| keys::parse_key(k).ok()) == Some(spec) {
                if act_screen(act) == Some(screen.name()) {
                    on_screen = true;
                } else {
                    elsewhere = true;
                }
            }
        }
        let jump = jumps.iter().any(|j| j.key == Some(keypress_of(spec)));
        return if (on_screen || jump) && !elsewhere {
            Outcome::Moved("noop".to_string())
        } else {
            Outcome::Keep
        };
    }
    let Some(row) = ROWS.iter().find(|r| r.old == action) else {
        return Outcome::Keep;
    };
    let target = if id == "git" { &row.git } else { &row.jj };
    match target {
        Target::Host => Outcome::Keep,
        Target::None(why) => Outcome::NoEffect((*why).to_string()),
        Target::Op(candidates) => {
            let mut first_error = None;
            for op in *candidates {
                match op_on_screen(manifest, id, op, screen) {
                    Ok(()) => return Outcome::Moved((*op).to_string()),
                    Err(e) => {
                        first_error.get_or_insert(e);
                    }
                }
            }
            Outcome::NoEffect(first_error.unwrap_or_default())
        }
    }
}

fn same_key(a: &str, b: &str) -> bool {
    match (keys::parse_key(a), keys::parse_key(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => a == b,
    }
}

/// Read the old per-screen tables of `tables` (the config's raw `[keys.*]` tables, before the
/// plugins' own tables are taken out): the entries that are translated or dropped leave their
/// host table, and the translated ones are added to the plugin's table (`tables[<plugin id>]`).
pub(crate) fn translate(
    registry: &Registry,
    tables: &mut HashMap<String, HashMap<String, String>>,
) -> Translation {
    let mut out = Translation::default();
    // The plugin tables as the user wrote them: they win over every old entry.
    let written: HashMap<String, HashMap<String, String>> = Backend::ALL
        .iter()
        .filter_map(|b| Some((b.id().to_string(), tables.get(b.id())?.clone())))
        .collect();
    // What the old entries put into each plugin table so far: key -> operation.
    let mut added: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
    let mut remove: Vec<(String, String)> = Vec::new();

    for screen in Screen::ALL {
        let shared_name = format!("git_{}", screen.name());
        let shared: BTreeMap<String, String> = tables
            .get(&shared_name)
            .map(|t| t.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
            .unwrap_or_default();
        // For each shared key: the outcome per backend (the entry leaves the table only when no
        // backend keeps it), and the notes held back until both backends were read.
        let mut shared_outcomes: BTreeMap<String, Vec<(&'static str, Outcome)>> = BTreeMap::new();
        for backend in Backend::ALL {
            let id = backend.id();
            let Some(manifest) = registry.manifest(id) else {
                for key in shared.keys() {
                    shared_outcomes
                        .entry(key.clone())
                        .or_default()
                        .push((id, Outcome::Keep));
                }
                continue;
            };
            let specific_name = format!("{id}.{}", screen.name());
            let specific: BTreeMap<String, String> = tables
                .get(&specific_name)
                .map(|t| t.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
                .unwrap_or_default();
            // Specific entries first (their notes come out in key order), then the shared ones
            // that no specific entry of the same key covers.
            for (key, action) in &specific {
                let label = format!("[keys.\"{specific_name}\"] {key} = \"{action}\"");
                let outcome = classify(manifest, id, screen, key, action);
                let kept = settle(&mut out, &mut added, &written, id, key, &label, outcome);
                if !kept {
                    remove.push((specific_name.clone(), key.clone()));
                }
            }
            for (key, action) in &shared {
                if specific.keys().any(|k| same_key(k, key)) {
                    out.warnings.push(format!(
                        "[keys.{shared_name}] {key} = \"{action}\" is ignored for {id}: \
                         [keys.\"{specific_name}\"] has {key}"
                    ));
                    shared_outcomes
                        .entry(key.clone())
                        .or_default()
                        .push((id, Outcome::NoEffect(String::new())));
                    continue;
                }
                let outcome = classify(manifest, id, screen, key, action);
                shared_outcomes
                    .entry(key.clone())
                    .or_default()
                    .push((id, outcome));
            }
        }
        for (key, outcomes) in shared_outcomes {
            let action = &shared[&key];
            let label = format!("[keys.{shared_name}] {key} = \"{action}\"");
            let mut any_kept = false;
            let mut reasons: Vec<(&str, String)> = Vec::new();
            let mut any_moved = false;
            for (id, outcome) in outcomes {
                match outcome {
                    Outcome::NoEffect(why) if why.is_empty() => {}
                    Outcome::NoEffect(why) => reasons.push((id, why)),
                    outcome => {
                        let moved = matches!(outcome, Outcome::Moved(_));
                        let kept =
                            settle(&mut out, &mut added, &written, id, &key, &label, outcome);
                        any_kept |= kept;
                        any_moved |= moved && !kept;
                    }
                }
            }
            if !any_moved && !any_kept && !reasons.is_empty() {
                let distinct: Vec<&String> = {
                    let mut v: Vec<&String> = reasons.iter().map(|(_, w)| w).collect();
                    v.dedup();
                    v
                };
                let why = if distinct.len() == 1 {
                    distinct[0].clone()
                } else {
                    reasons
                        .iter()
                        .map(|(id, w)| format!("{id}: {w}"))
                        .collect::<Vec<_>>()
                        .join("; ")
                };
                out.warnings.push(format!("{label}: no effect — {why}"));
            }
            if !any_kept {
                remove.push((shared_name.clone(), key.clone()));
            }
        }
    }

    for (id, entries) in added {
        let table = tables.entry(id.to_string()).or_default();
        for (key, op) in entries {
            table.insert(key, op);
        }
    }
    for (name, key) in remove {
        if let Some(table) = tables.get_mut(&name) {
            table.remove(&key);
            if table.is_empty() {
                tables.remove(&name);
            }
        }
    }
    out
}

/// Note one entry's outcome and, for a move, add it to the plugin table. Returns whether the
/// entry stays in the host table.
fn settle(
    out: &mut Translation,
    added: &mut BTreeMap<String, BTreeMap<String, String>>,
    written: &HashMap<String, HashMap<String, String>>,
    id: &str,
    key: &str,
    label: &str,
    outcome: Outcome,
) -> bool {
    match outcome {
        Outcome::Keep => true,
        Outcome::NoEffect(why) => {
            if !why.is_empty() {
                out.warnings.push(format!("{label}: no effect — {why}"));
            }
            false
        }
        Outcome::Moved(op) => {
            let user = written.get(id);
            let mine = added.entry(id.to_string()).or_default();
            let taken_by_user = user.is_some_and(|t| t.keys().any(|k| same_key(k, key)));
            let op_by_user = op != "noop" && user.is_some_and(|t| t.values().any(|v| *v == op));
            let taken_by_old = mine.keys().any(|k| same_key(k, key));
            let op_by_old = op != "noop" && mine.values().any(|v| *v == op);
            if taken_by_user || op_by_user {
                out.warnings.push(format!(
                    "{label}: ignored — [keys.{id}] already sets {}",
                    if taken_by_user {
                        key.to_string()
                    } else {
                        format!("a key for `{op}`")
                    }
                ));
            } else if taken_by_old || op_by_old {
                out.warnings.push(format!(
                    "{label}: ignored — another old entry already gave {} to {id}",
                    if taken_by_old {
                        key.to_string()
                    } else {
                        format!("`{op}` a key")
                    }
                ));
            } else {
                mine.insert(key.to_string(), op.clone());
                out.renames
                    .push((label.to_string(), format!("[keys.{id}] {key} = \"{op}\"")));
            }
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type Tables = HashMap<String, HashMap<String, String>>;

    fn tables(entries: &[(&str, &[(&str, &str)])]) -> Tables {
        entries
            .iter()
            .map(|(name, rows)| {
                (
                    name.to_string(),
                    rows.iter()
                        .map(|(k, v)| (k.to_string(), v.to_string()))
                        .collect(),
                )
            })
            .collect()
    }

    fn run(entries: &[(&str, &[(&str, &str)])]) -> (Tables, Translation) {
        let mut t = tables(entries);
        let out = translate(&crate::plugins::shared_registry(&[]), &mut t);
        (t, out)
    }

    fn entry<'a>(t: &'a Tables, table: &str, key: &str) -> Option<&'a str> {
        t.get(table)?.get(key).map(String::as_str)
    }

    /// The names of the git arms of `keymap::action_from_str`, read from its source.
    fn accepted_hub_names() -> Vec<String> {
        let src = include_str!("../keymap.rs");
        let start = src.find("pub fn action_from_str").expect("action_from_str");
        let body = &src[start..];
        let body = &body[..body.find("_ => return None").expect("end of the match")];
        let mut names = Vec::new();
        let mut lines = body.lines().map(str::trim);
        while let Some(line) = lines.next() {
            if line != "#[cfg(feature = \"git\")]" {
                continue;
            }
            let Some(next) = lines.next() else { break };
            let Some(rest) = next.strip_prefix('"') else {
                continue;
            };
            if let Some(end) = rest.find('"') {
                names.push(rest[..end].to_string());
            }
        }
        names
    }

    /// Git-gated arms that are not old hub actions of a screen table (they are host actions of
    /// the tree and the diff view, bound elsewhere).
    const NOT_SCREEN_ACTIONS: &[&str] = &[];

    #[test]
    fn old_names_every_hub_action_accepted_has_a_row() {
        let names = accepted_hub_names();
        assert!(
            names.len() >= 40,
            "the enumeration must see the hub actions, saw {}",
            names.len()
        );
        for name in &names {
            if NOT_SCREEN_ACTIONS.contains(&name.as_str()) {
                continue;
            }
            assert!(
                ROWS.iter().any(|r| r.old == name),
                "`{name}` is accepted under an old screen table but has no row in ROWS"
            );
        }
        for row in ROWS {
            assert!(
                names.iter().any(|n| n == row.old),
                "ROWS names `{}`, which no git arm of action_from_str accepts",
                row.old
            );
            assert!(
                crate::keymap::action_from_str(row.old).is_some(),
                "`{}` does not parse",
                row.old
            );
            assert_eq!(
                ROWS.iter().filter(|r| r.old == row.old).count(),
                1,
                "`{}` has two rows",
                row.old
            );
        }
    }

    #[test]
    fn old_names_every_target_is_an_operation_of_its_plugin() {
        let registry = crate::plugins::shared_registry(&[]);
        for row in ROWS {
            for (id, target) in [("git", &row.git), ("jj", &row.jj)] {
                let manifest = registry
                    .manifest(id)
                    .expect("the vcs plugins are registered");
                let Target::Op(candidates) = target else {
                    continue;
                };
                for op in *candidates {
                    let known = Screen::ALL
                        .iter()
                        .any(|s| op_on_screen(manifest, id, op, *s).is_ok());
                    assert!(
                        known,
                        "`{}` maps {id} to `{op}`, which is no operation of {id} on any screen",
                        row.old
                    );
                }
            }
        }
    }

    #[test]
    fn old_names_a_mapped_entry_moves_into_the_plugin_table_with_one_note() {
        let (t, out) = run(&[("git.changes", &[("x", "git_stage")])]);
        assert_eq!(entry(&t, "git", "x"), Some("stage"));
        assert!(!t.contains_key("git.changes"), "the host table is emptied");
        assert_eq!(
            out.renames,
            vec![(
                "[keys.\"git.changes\"] x = \"git_stage\"".to_string(),
                "[keys.git] x = \"stage\"".to_string()
            )]
        );
        assert!(out.warnings.is_empty());
    }

    #[test]
    fn old_names_a_jump_and_a_screen_op_of_another_screen() {
        let (t, out) = run(&[
            ("git.changes", &[("y", "git_open_branches")]),
            ("jj.graph", &[("y", "git_graph_toggle_all")]),
            ("git.log", &[("x", "git_stage")]),
        ]);
        assert_eq!(entry(&t, "git", "y"), Some("goto_branches"));
        assert_eq!(entry(&t, "jj", "y"), Some("graph_all"));
        assert_eq!(entry(&t, "git", "x"), None);
        assert!(
            out.warnings
                .iter()
                .any(|w| w.contains("`stage` is an operation of the changes screen, not of log")),
            "{:?}",
            out.warnings
        );
    }

    #[test]
    fn old_names_an_unmapped_name_notes_no_effect_and_why() {
        let (t, out) = run(&[("jj.changes", &[("x", "git_stage"), ("c", "git_commit")])]);
        assert!(!t.contains_key("jj.changes"));
        assert!(!t.contains_key("jj"));
        assert_eq!(out.renames, vec![]);
        assert_eq!(out.warnings.len(), 2);
        for w in &out.warnings {
            assert!(
                w.contains("no effect — konoma only reads a jj repository"),
                "{w}"
            );
        }
    }

    #[test]
    fn old_names_a_shared_table_maps_each_backend_and_notes_once() {
        let (t, out) = run(&[(
            "git_changes",
            &[("x", "git_stage"), ("z", "jj_sync"), ("w", "git_open_log")],
        )]);
        assert_eq!(entry(&t, "git", "x"), Some("stage"));
        assert_eq!(entry(&t, "jj", "x"), None);
        assert_eq!(entry(&t, "jj", "z"), Some("sync"));
        assert_eq!(entry(&t, "git", "z"), None);
        assert!(!t.contains_key("git_changes"));
        assert_eq!(out.renames.len(), 2, "{:?}", out.renames);
        assert_eq!(out.warnings.len(), 1, "{:?}", out.warnings);
        assert!(out.warnings[0].contains("git_open_log"));
        assert!(out.warnings[0].contains("no screen jump exists"));
    }

    #[test]
    fn old_names_the_new_table_wins_and_the_loser_is_noted() {
        let (t, out) = run(&[
            ("git", &[("x", "stage_all")]),
            ("git.changes", &[("x", "git_stage"), ("s", "git_stage_all")]),
        ]);
        // `x` is set in [keys.git]; `stage_all` already has a key there.
        assert_eq!(entry(&t, "git", "x"), Some("stage_all"));
        assert_eq!(entry(&t, "git", "s"), None);
        assert_eq!(out.renames, vec![]);
        assert_eq!(out.warnings.len(), 2, "{:?}", out.warnings);
        assert!(out.warnings.iter().any(|w| w.contains("already sets x")));
        assert!(out
            .warnings
            .iter()
            .any(|w| w.contains("already sets a key for `stage_all`")));
    }

    #[test]
    fn old_names_a_specific_table_beats_the_shared_one_for_its_backend() {
        let (t, out) = run(&[
            ("git_changes", &[("x", "git_stage")]),
            ("git.changes", &[("x", "git_discard")]),
        ]);
        assert_eq!(entry(&t, "git", "x"), Some("discard"));
        assert!(out
            .warnings
            .iter()
            .any(|w| w.contains("[keys.git_changes] x = \"git_stage\" is ignored for git")));
    }

    #[test]
    fn old_names_two_old_entries_for_one_key_the_first_read_wins() {
        let (t, out) = run(&[
            ("git.changes", &[("x", "git_discard")]),
            ("git.graph", &[("x", "git_graph_clear_base")]),
        ]);
        // Screens are read in order: changes before graph.
        assert_eq!(entry(&t, "git", "x"), Some("discard"));
        assert!(out
            .warnings
            .iter()
            .any(|w| w.contains("another old entry already gave x to git")));
    }

    #[test]
    fn old_names_noop_moves_only_when_the_key_is_one_screens_operation() {
        // `u` is `unstage` on Changes only: removed in the plugin table.
        let (t, out) = run(&[("git.changes", &[("u", "noop")])]);
        assert_eq!(entry(&t, "git", "u"), Some("noop"));
        assert!(!t.contains_key("git.changes"));
        assert_eq!(out.renames.len(), 1);
        // `s` is `stage` on Changes and `graph_set_base` on Graph: the plugin table cannot say
        // "only here", so the entry stays in the host table (exact for its own screen).
        let (t, out) = run(&[("git.changes", &[("s", "noop")])]);
        assert_eq!(entry(&t, "git", "s"), None);
        assert_eq!(entry(&t, "git.changes", "s"), Some("noop"));
        assert_eq!(out.renames, vec![]);
        // `j` is a host key: nothing of the plugin has it.
        let (t, _) = run(&[("git.changes", &[("j", "noop")])]);
        assert_eq!(entry(&t, "git.changes", "j"), Some("noop"));
        // `p` is the jump to Changes.
        let (t, _) = run(&[("git.log", &[("p", "noop")])]);
        assert_eq!(entry(&t, "git", "p"), Some("noop"));
    }

    #[test]
    fn old_names_host_actions_and_other_actions_stay_in_the_host_table() {
        let (t, out) = run(&[(
            "git.changes",
            &[
                ("q", "git_close"),
                ("t", "git_launch_tool"),
                ("r", "refresh"),
            ],
        )]);
        for key in ["q", "t", "r"] {
            assert!(entry(&t, "git.changes", key).is_some(), "{key}");
        }
        assert_eq!(out, Translation::default());
    }

    #[test]
    fn old_names_the_result_is_the_same_every_time() {
        let input: &[(&str, &[(&str, &str)])] = &[
            (
                "git_changes",
                &[
                    ("x", "git_stage"),
                    ("y", "git_open_branches"),
                    ("n", "git_open_log"),
                ],
            ),
            ("git.changes", &[("a", "git_discard"), ("b", "noop")]),
            ("jj.log", &[("c", "git_open_branches")]),
            ("git.graph", &[("d", "git_graph_set_base")]),
        ];
        let (t1, o1) = run(input);
        for _ in 0..20 {
            let (t2, o2) = run(input);
            assert_eq!(t1, t2);
            assert_eq!(o1, o2);
        }
    }
}
