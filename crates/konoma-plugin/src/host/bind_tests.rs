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
