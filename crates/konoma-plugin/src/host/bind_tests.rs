//! The screen-jump operations (`goto_<screen id>`): their default keys, the user's rebinding and
//! removal, and every refusal.

use std::collections::HashMap;

use super::*;
use crate::project::{ProjectDecl, ProjectScreen};
use crate::{Act, Manifest};

fn manifest() -> Manifest {
    Manifest::new("vcs", "1")
        .project(
            ProjectDecl::new()
                .screen(ProjectScreen::new("changes", "Changes"))
                .screen(ProjectScreen::new("branches", "Branches").key('b'))
                .screen(ProjectScreen::new("worktrees", "Worktrees").key('w'))
                // A pushed screen is not reached by a key, even if it names one.
                .screen(ProjectScreen::new("detail", "Detail").key('z').pushed()),
        )
        .action(Act::new("stage").key("x"))
}

fn table(entries: &[(&str, &str)]) -> HashMap<String, String> {
    entries
        .iter()
        .map(|(k, a)| (k.to_string(), a.to_string()))
        .collect()
}

fn keys_of(jumps: &[JumpBinding]) -> Vec<(String, Option<KeyPress>)> {
    jumps.iter().map(|j| (j.op.clone(), j.key)).collect()
}

#[test]
fn the_operation_id_is_goto_and_the_screen_id() {
    assert_eq!(jump_op_id("branches"), "goto_branches");
    assert_eq!(jump_op_id("worktrees"), "goto_worktrees");
}

#[test]
fn the_defaults_are_the_declared_keys_of_the_screens_that_have_one() {
    let (jumps, notes) = bind_jumps("vcs", &manifest(), None);
    assert!(notes.is_empty());
    assert_eq!(
        keys_of(&jumps),
        vec![
            ("goto_branches".to_string(), Some(KeyPress::ch('b'))),
            ("goto_worktrees".to_string(), Some(KeyPress::ch('w'))),
        ],
        "`changes` has no key and `detail` is pushed"
    );
    let (again, _) = bind_jumps("vcs", &manifest(), Some(&HashMap::new()));
    assert_eq!(jumps, again, "an empty table changes nothing");
}

#[test]
fn a_rebinding_moves_the_jump_and_leaves_the_old_key_free() {
    let user = table(&[("x", "goto_branches")]);
    let (jumps, notes) = bind_jumps("vcs", &manifest(), Some(&user));
    assert!(notes.is_empty(), "{notes:?}");
    assert_eq!(jump_key_of(&jumps, "branches"), Some(KeyPress::ch('x')));
    assert_eq!(jump_key_of(&jumps, "worktrees"), Some(KeyPress::ch('w')));
    assert_eq!(jump_target(&jumps, KeyPress::ch('x')), Some("branches"));
    assert_eq!(
        jump_target(&jumps, KeyPress::ch('b')),
        None,
        "old key inert"
    );
}

#[test]
fn noop_removes_the_jump_that_has_the_key() {
    let user = table(&[("b", "noop")]);
    let (jumps, notes) = bind_jumps("vcs", &manifest(), Some(&user));
    assert!(notes.is_empty());
    assert_eq!(jump_key_of(&jumps, "branches"), None);
    assert_eq!(jump_target(&jumps, KeyPress::ch('b')), None);
    assert_eq!(jump_key_of(&jumps, "worktrees"), Some(KeyPress::ch('w')));
    let user = table(&[("w", "disabled")]);
    let (jumps, _) = bind_jumps("vcs", &manifest(), Some(&user));
    assert_eq!(
        jump_key_of(&jumps, "worktrees"),
        None,
        "`disabled` is `noop`"
    );
    assert_eq!(jump_key_of(&jumps, "branches"), Some(KeyPress::ch('b')));
}

#[test]
fn noop_on_a_key_no_jump_has_changes_no_jump() {
    let user = table(&[("x", "noop"), ("q", "noop")]);
    let (jumps, notes) = bind_jumps("vcs", &manifest(), Some(&user));
    assert!(notes.is_empty());
    assert_eq!(jump_key_of(&jumps, "branches"), Some(KeyPress::ch('b')));
    assert_eq!(jump_key_of(&jumps, "worktrees"), Some(KeyPress::ch('w')));
}

#[test]
fn a_key_the_host_acts_on_is_refused_and_the_jump_keeps_its_default() {
    // `j` moves, `q` leaves, `/` filters, `e` edits, `Q` quits, `1` is a tab: all the host's.
    for key in ["j", "q", "/", "e", "Q", "1", "h", "l", "space"] {
        let user = table(&[(key, "goto_branches")]);
        let (jumps, notes) = bind_jumps("vcs", &manifest(), Some(&user));
        assert_eq!(
            jump_key_of(&jumps, "branches"),
            Some(KeyPress::ch('b')),
            "{key}"
        );
        assert_eq!(notes.len(), 1, "{key}: {notes:?}");
        assert!(
            matches!(&notes[0], KeyNote::BadKey { plugin, key: k, .. } if plugin == "vcs" && k == key),
            "{key}: {notes:?}"
        );
    }
}

#[test]
fn a_ctrl_chord_or_a_named_key_is_refused() {
    for key in ["ctrl-x", "enter", "tab", "f2", "up"] {
        let user = table(&[(key, "goto_worktrees")]);
        let (jumps, notes) = bind_jumps("vcs", &manifest(), Some(&user));
        assert_eq!(
            jump_key_of(&jumps, "worktrees"),
            Some(KeyPress::ch('w')),
            "{key}"
        );
        assert_eq!(notes.len(), 1, "{key}: {notes:?}");
        assert!(
            matches!(&notes[0], KeyNote::BadKey { why, .. } if why.contains("one plain character")),
            "{key}: {notes:?}"
        );
    }
}

#[test]
fn the_project_entry_key_p_may_be_a_jump_key() {
    let user = table(&[("p", "goto_branches")]);
    let (jumps, notes) = bind_jumps("vcs", &manifest(), Some(&user));
    assert!(notes.is_empty(), "{notes:?}");
    assert_eq!(jump_key_of(&jumps, "branches"), Some(KeyPress::ch('p')));
}

#[test]
fn an_upper_case_letter_is_a_jump_key() {
    let user = table(&[("B", "goto_branches"), ("shift-w", "goto_worktrees")]);
    let (jumps, notes) = bind_jumps("vcs", &manifest(), Some(&user));
    assert!(notes.is_empty(), "{notes:?}");
    assert_eq!(jump_key_of(&jumps, "branches"), Some(KeyPress::ch('B')));
    assert_eq!(jump_key_of(&jumps, "worktrees"), Some(KeyPress::ch('W')));
}

#[test]
fn a_key_another_jump_has_is_a_collision_and_the_jump_falls_back_to_its_default() {
    // `w` already jumps to worktrees: branches cannot take it and goes back to `b`.
    let user = table(&[("w", "goto_branches")]);
    let (jumps, notes) = bind_jumps("vcs", &manifest(), Some(&user));
    assert_eq!(jump_key_of(&jumps, "branches"), Some(KeyPress::ch('b')));
    assert_eq!(jump_key_of(&jumps, "worktrees"), Some(KeyPress::ch('w')));
    assert_eq!(notes.len(), 1, "{notes:?}");
    assert!(
        matches!(&notes[0], KeyNote::BadKey { key, why, .. }
            if key == "w" && why.contains("goto_worktrees")),
        "{notes:?}"
    );
}

#[test]
fn two_jumps_may_swap_their_keys() {
    let user = table(&[("w", "goto_branches"), ("b", "goto_worktrees")]);
    let (jumps, notes) = bind_jumps("vcs", &manifest(), Some(&user));
    assert!(notes.is_empty(), "{notes:?}");
    assert_eq!(jump_key_of(&jumps, "branches"), Some(KeyPress::ch('w')));
    assert_eq!(jump_key_of(&jumps, "worktrees"), Some(KeyPress::ch('b')));
}

#[test]
fn a_second_key_for_one_jump_keeps_the_first() {
    let user = table(&[("x", "goto_branches"), ("y", "goto_branches")]);
    let (jumps, _) = bind_jumps("vcs", &manifest(), Some(&user));
    assert_eq!(jump_key_of(&jumps, "branches"), Some(KeyPress::ch('x')));
    assert_eq!(jump_target(&jumps, KeyPress::ch('y')), None);
}

#[test]
fn a_screen_without_a_default_key_has_no_jump_to_rebind() {
    let user = table(&[("x", "goto_changes"), ("y", "goto_detail")]);
    let (jumps, _) = bind_jumps("vcs", &manifest(), Some(&user));
    assert_eq!(jump_target(&jumps, KeyPress::ch('x')), None);
    assert_eq!(jump_target(&jumps, KeyPress::ch('y')), None);
    assert_eq!(jumps.len(), 2);
}

#[test]
fn a_manifest_without_a_project_has_no_jumps() {
    let (jumps, notes) = bind_jumps(
        "vcs",
        &Manifest::new("vcs", "1"),
        Some(&table(&[("x", "goto_branches")])),
    );
    assert!(jumps.is_empty() && notes.is_empty());
}

#[test]
fn a_jump_entry_in_the_table_is_not_an_action_and_takes_no_action_key() {
    let user = table(&[("y", "goto_branches"), ("b", "noop")]);
    let (acts, notes) = bind_actions("vcs", &manifest(), Some(&user), |a| a.to_string());
    assert!(
        notes.is_empty(),
        "`goto_*` is not an unknown action: {notes:?}"
    );
    assert_eq!(acts.len(), 1, "no action is made for a jump");
    assert_eq!(acts[0].act, "stage");
    assert_eq!(
        acts[0].key,
        Some(KeyPress::ch('x')),
        "the jump's key did not move the action's"
    );
}

#[test]
fn an_unknown_operation_is_still_unknown() {
    let user = table(&[("x", "goto_nowhere")]);
    let (_, notes) = bind_actions("vcs", &manifest(), Some(&user), |a| a.to_string());
    assert_eq!(
        notes,
        vec![KeyNote::UnknownAction {
            plugin: "vcs".into(),
            action: "goto_nowhere".into()
        }]
    );
}

/// Three jumps with the default keys `a`, `b`, `c`.
fn three() -> Manifest {
    Manifest::new("vcs", "1").project(
        ProjectDecl::new()
            .screen(ProjectScreen::new("sa", "A").key('a'))
            .screen(ProjectScreen::new("sb", "B").key('b'))
            .screen(ProjectScreen::new("sc", "C").key('c')),
    )
}

fn bad_key_notes(notes: &[KeyNote]) -> Vec<String> {
    notes
        .iter()
        .filter_map(|n| match n {
            KeyNote::BadKey { key, .. } => Some(key.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn a_refused_key_does_not_make_the_next_key_for_the_jump_a_false_note() {
    // `j` is the host's (refused); `x` then is the jump's key, and nothing else is noted about it.
    let user = table(&[("j", "goto_branches"), ("x", "goto_branches")]);
    let (jumps, notes) = bind_jumps("vcs", &manifest(), Some(&user));
    assert_eq!(jump_key_of(&jumps, "branches"), Some(KeyPress::ch('x')));
    assert_eq!(bad_key_notes(&notes), vec!["j".to_string()], "{notes:?}");
    let (_, act_notes) = bind_actions("vcs", &manifest(), Some(&user), |a| a.to_string());
    assert!(act_notes.is_empty(), "{act_notes:?}");
}

#[test]
fn of_several_keys_for_one_jump_the_first_acceptable_wins_and_the_others_are_noted() {
    let user = table(&[
        ("z", "goto_branches"),
        ("x", "goto_branches"),
        ("y", "goto_branches"),
    ]);
    let (jumps, notes) = bind_jumps("vcs", &manifest(), Some(&user));
    assert_eq!(jump_key_of(&jumps, "branches"), Some(KeyPress::ch('x')));
    assert_eq!(
        bad_key_notes(&notes),
        vec!["y".to_string(), "z".to_string()],
        "{notes:?}"
    );
}

#[test]
fn an_unparseable_jump_key_is_noted_once_by_the_whole_binding() {
    let user = table(&[("nonsense-key", "goto_branches")]);
    let (jumps, jump_notes) = bind_jumps("vcs", &manifest(), Some(&user));
    let (_, act_notes) = bind_actions("vcs", &manifest(), Some(&user), |a| a.to_string());
    assert_eq!(jump_key_of(&jumps, "branches"), Some(KeyPress::ch('b')));
    assert_eq!(
        jump_notes.len() + act_notes.len(),
        1,
        "{jump_notes:?} {act_notes:?}"
    );
}

#[test]
fn a_jump_whose_rebinding_is_refused_keeps_its_default_even_when_another_jump_moved_onto_it() {
    // `a = goto_c` moves C onto A's default key; `b = goto_a` is refused (B has `b`). A must still
    // have `a`, so the move of C onto `a` is the one refused.
    let user = table(&[("a", "goto_sc"), ("b", "goto_sa")]);
    let (jumps, notes) = bind_jumps("vcs", &three(), Some(&user));
    assert_eq!(
        jump_key_of(&jumps, "sa"),
        Some(KeyPress::ch('a')),
        "{jumps:?}"
    );
    assert_eq!(jump_key_of(&jumps, "sb"), Some(KeyPress::ch('b')));
    assert_eq!(jump_key_of(&jumps, "sc"), Some(KeyPress::ch('c')));
    assert_eq!(notes.len(), 2, "{notes:?}");
}

#[test]
fn every_table_of_up_to_two_entries_keeps_the_jumps_keys_and_notes_each_refusal_once() {
    let m = three();
    let defaults = [("goto_sa", 'a'), ("goto_sb", 'b'), ("goto_sc", 'c')];
    let keys = ["a", "b", "c", "x", "j"];
    let actions = ["goto_sa", "goto_sb", "goto_sc", "noop"];
    let mut entries: Vec<(&str, &str)> = Vec::new();
    for k in keys {
        for a in actions {
            entries.push((k, a));
        }
    }
    let mut tables: Vec<Vec<(&str, &str)>> = vec![vec![]];
    for (i, e) in entries.iter().enumerate() {
        tables.push(vec![*e]);
        for f in &entries[i + 1..] {
            if f.0 != e.0 {
                tables.push(vec![*e, *f]);
            }
        }
    }
    assert_eq!(tables.len(), 181, "the enumeration shrank or grew");
    for t in tables {
        let user = table(&t);
        let (jumps, notes) = bind_jumps("vcs", &m, Some(&user));
        let case = format!("{t:?} -> {jumps:?} {notes:?}");
        let mut seen = Vec::new();
        for (op, d) in defaults {
            let jump = jumps.iter().find(|j| j.op == op).expect("jump");
            let user_keys: Vec<char> = t
                .iter()
                .filter(|(_, a)| *a == op)
                .map(|(k, _)| k.chars().next().unwrap())
                .collect();
            let removed = t.iter().any(|(k, a)| *a == "noop" && *k == d.to_string());
            match jump.key {
                Some(k) => {
                    let ok =
                        k == KeyPress::ch(d) || user_keys.iter().any(|u| KeyPress::ch(*u) == k);
                    assert!(ok, "{case}");
                    assert!(!seen.contains(&k), "shared key: {case}");
                    seen.push(k);
                }
                None => assert!(removed, "keyless without noop: {case}"),
            }
        }
        // Each entry that is not what its jump ended with is noted exactly once.
        let refused = t
            .iter()
            .filter(|(k, a)| {
                defaults.iter().any(|(op, _)| op == a)
                    && jumps.iter().find(|j| j.op == *a).and_then(|j| j.key)
                        != Some(KeyPress::ch(k.chars().next().unwrap()))
            })
            .count();
        assert_eq!(notes.len(), refused, "{case}");
    }
}
