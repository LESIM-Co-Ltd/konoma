//! B1 equivalence tests: the layered, context-keyed keymap against the old one-table-per-surface
//! keymap ([`super::legacy`]). B1 restructured how the default keys are stored and looked up but
//! changed none of them, so *every* lookup must agree: every key the terminal can send, in every
//! surface and state a key can arrive in, with both paging schemes, with and without a leader
//! pending, and after the same configuration is merged.
//!
//! B3 changes keys on purpose; these tests are then replaced by ones that pin the new defaults.

use super::legacy::LegacyKeyMap;
use super::*;

/// Every `Surface`. `surface_ordinal` is exhaustive, so a new variant fails to compile until it is
/// added there, and `the_surface_list_is_complete` fails until it is added here.
fn all_surfaces() -> Vec<Surface> {
    #[allow(unused_mut)]
    let mut v = vec![
        Surface::DialogInput,
        Surface::Filter,
        Surface::Search,
        Surface::Mark,
        Surface::DialogConfirmDelete,
        Surface::DialogConfirmDrop,
        Surface::DialogRenamePreview,
        Surface::DialogConfirmQuit,
        Surface::DialogConfirmBookmark,
        Surface::Help,
        Surface::Sort,
        Surface::Bookmarks,
        Surface::Tabs,
        Surface::Outline,
        Surface::Info,
        Surface::TableCell,
        Surface::Visual,
        Surface::Tree,
        Surface::PreviewText,
        Surface::PreviewTextVisual,
        Surface::PreviewImage,
        Surface::PreviewTable,
    ];
    #[cfg(feature = "git")]
    v.extend([
        Surface::BranchFilter,
        Surface::WorktreeFilter,
        Surface::GitDetail,
        Surface::GitLog,
        Surface::GitGraph,
        Surface::GitGraphPicker,
        Surface::GitBranches,
        Surface::GitChanges,
        Surface::GitWorktrees,
        Surface::PreviewGitDiff,
    ]);
    v
}

fn surface_ordinal(s: Surface) -> usize {
    match s {
        Surface::DialogInput => 0,
        Surface::Filter => 1,
        Surface::Search => 2,
        Surface::Mark => 3,
        Surface::DialogConfirmDelete => 4,
        Surface::DialogConfirmDrop => 5,
        Surface::DialogRenamePreview => 6,
        Surface::DialogConfirmQuit => 7,
        Surface::DialogConfirmBookmark => 8,
        Surface::Help => 9,
        Surface::Sort => 10,
        Surface::Bookmarks => 11,
        Surface::Tabs => 12,
        Surface::Outline => 13,
        Surface::Info => 14,
        Surface::TableCell => 15,
        Surface::Visual => 16,
        Surface::Tree => 17,
        Surface::PreviewText => 18,
        Surface::PreviewTextVisual => 19,
        Surface::PreviewImage => 20,
        Surface::PreviewTable => 21,
        #[cfg(feature = "git")]
        Surface::BranchFilter => 22,
        #[cfg(feature = "git")]
        Surface::WorktreeFilter => 23,
        #[cfg(feature = "git")]
        Surface::GitDetail => 24,
        #[cfg(feature = "git")]
        Surface::GitLog => 25,
        #[cfg(feature = "git")]
        Surface::GitGraph => 26,
        #[cfg(feature = "git")]
        Surface::GitGraphPicker => 27,
        #[cfg(feature = "git")]
        Surface::GitBranches => 28,
        #[cfg(feature = "git")]
        Surface::GitChanges => 29,
        #[cfg(feature = "git")]
        Surface::GitWorktrees => 30,
        #[cfg(feature = "git")]
        Surface::PreviewGitDiff => 31,
    }
}

#[test]
fn the_surface_list_is_complete() {
    let all = all_surfaces();
    let mut ords: Vec<usize> = all.iter().map(|&s| surface_ordinal(s)).collect();
    ords.sort_unstable();
    let expected: Vec<usize> = if cfg!(feature = "git") {
        (0..32).collect()
    } else {
        (0..22).collect()
    };
    assert_eq!(ords, expected, "all_surfaces() misses or repeats a Surface");
}

/// Every state a key can arrive in: (surface, text preview is a decorated document, git screens
/// are open for jj). The two flags only matter to the surfaces they refine; the rest are visited
/// once per flag value anyway, which also checks that the flags do not leak.
fn all_states() -> Vec<(Surface, bool, bool)> {
    let mut v = Vec::new();
    for s in all_surfaces() {
        for doc in [false, true] {
            for jj in [false, true] {
                // Visiting the unrefined surfaces four times would only repeat work.
                let refined_by_doc = s == Surface::PreviewText;
                let refined_by_jj = surface_has_backend(s);
                if (doc && !refined_by_doc) || (jj && !refined_by_jj) {
                    continue;
                }
                v.push((s, doc, jj));
            }
        }
    }
    v
}

fn surface_has_backend(s: Surface) -> bool {
    #[cfg(feature = "git")]
    {
        matches!(
            s,
            Surface::GitDetail
                | Surface::GitLog
                | Surface::GitGraph
                | Surface::GitGraphPicker
                | Surface::GitBranches
                | Surface::GitChanges
                | Surface::GitWorktrees
        )
    }
    #[cfg(not(feature = "git"))]
    {
        let _ = s;
        false
    }
}

/// Every key the terminal can send: printable ASCII and its Ctrl version, the named keys, F1-F12.
fn all_keys() -> Vec<KeyPress> {
    let mut v = Vec::new();
    for b in 0x20u8..=0x7e {
        v.push(KeyPress::ch(char::from(b)));
        v.push(KeyPress::ctrl_ch(char::from(b)));
    }
    for code in [
        KeyCode::Esc,
        KeyCode::Enter,
        KeyCode::Tab,
        KeyCode::BackTab,
        KeyCode::Up,
        KeyCode::Down,
        KeyCode::Left,
        KeyCode::Right,
        KeyCode::Home,
        KeyCode::End,
        KeyCode::PageUp,
        KeyCode::PageDown,
        KeyCode::Backspace,
        KeyCode::Delete,
    ] {
        v.push(KeyPress::key(code));
    }
    for n in 1..=12 {
        v.push(KeyPress::key(KeyCode::F(n)));
    }
    v
}

fn all_leaders() -> Vec<LeaderId> {
    #[allow(unused_mut)]
    let mut v = vec![LeaderId::Copy, LeaderId::File, LeaderId::TableCopy];
    #[cfg(feature = "git")]
    v.push(LeaderId::GitCopy);
    v
}

/// The new keymap's answer where the old one was asked about `sfc`.
fn new_resolve(
    new: &KeyMap,
    (sfc, doc, jj): (Surface, bool, bool),
    pending: Option<LeaderId>,
    kp: KeyPress,
) -> Resolution {
    match (Context::of_surface(sfc, doc, jj), pending) {
        (Some(c), p) => new.resolve(c, p, kp),
        // No context = a surface that does not use the keymap (text input, confirm modal).
        (None, Some(l)) => new.resolve_leader(l, kp),
        (None, None) => Resolution::Unbound,
    }
}

/// Compare every lookup; returns how many (state, key, leader) combinations were compared.
/// `with_leaders` also asks every key inside each open leader menu (the menus do not depend on the
/// state, so the config tests compare them once, directly, instead).
fn compare_all(old: &LegacyKeyMap, new: &KeyMap, what: &str, with_leaders: bool) -> usize {
    let keys = all_keys();
    let mut n = 0;
    let mut diffs: Vec<String> = Vec::new();
    for st in all_states() {
        let (sfc, _, _) = st;
        for &kp in &keys {
            let (a, b) = (old.resolve(sfc, None, kp), new_resolve(new, st, None, kp));
            n += 1;
            if a != b {
                diffs.push(format!(
                    "{what}: {st:?} {} : old {a:?} new {b:?}",
                    kp.label()
                ));
            }
            if !with_leaders {
                continue;
            }
            for lead in all_leaders() {
                let (a, b) = (
                    old.resolve(sfc, Some(lead), kp),
                    new_resolve(new, st, Some(lead), kp),
                );
                n += 1;
                if a != b {
                    diffs.push(format!(
                        "{what}: {st:?} leader {lead:?} {} : old {a:?} new {b:?}",
                        kp.label()
                    ));
                }
            }
        }
    }
    assert!(
        diffs.is_empty(),
        "{} lookups differ (first 20):\n{}",
        diffs.len(),
        diffs
            .iter()
            .take(20)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
    n
}

#[test]
fn every_default_lookup_matches_the_old_keymap() {
    let mut total = 0;
    for scheme in [KeyScheme::Vim, KeyScheme::Less] {
        let old = LegacyKeyMap::defaults(scheme);
        let new = KeyMap::defaults(scheme);
        total += compare_all(&old, &new, &format!("defaults {scheme:?}"), true);
    }
    // 2 schemes x states x 216 keys x (no leader + each leader). Printed so the number can be
    // reported; the exact count keeps the enumeration from silently shrinking.
    println!("equivalence: {total} (state, key, leader) lookups compared");
    let states = if cfg!(feature = "git") { 40 } else { 23 };
    assert_eq!(all_states().len(), states);
    assert_eq!(all_keys().len(), 216);
    assert_eq!(total, 2 * states * 216 * (1 + all_leaders().len()));
}

#[test]
fn the_default_leader_menus_match_the_old_ones() {
    for scheme in [KeyScheme::Vim, KeyScheme::Less] {
        let old = LegacyKeyMap::defaults(scheme);
        let new = KeyMap::defaults(scheme);
        assert_eq!(old.leaders.len(), new.leaders.len());
        for (id, menu) in &old.leaders {
            let n = new.leaders.get(id).expect("leader present");
            let a: Vec<_> = menu.items.iter().map(|i| (i.key, i.action)).collect();
            let b: Vec<_> = n.items.iter().map(|i| (i.key, i.action)).collect();
            assert_eq!(a, b, "{id:?}");
        }
    }
}

/// Every context is reached from some surface state, and every keymap-driven surface state
/// reaches one (so no context is dead and no surface falls through).
#[test]
fn the_contexts_and_the_surface_states_cover_each_other() {
    let mut reached: Vec<Context> = Vec::new();
    for (s, doc, jj) in all_states() {
        match Context::of_surface(s, doc, jj) {
            Some(c) => {
                assert!(
                    s.allows_tabs(),
                    "{s:?} has a context but is a fixed surface"
                );
                reached.push(c);
            }
            None => assert!(!s.allows_tabs(), "{s:?} uses the keymap but has no context"),
        }
    }
    reached.sort_by_key(|c| c.name());
    reached.dedup();
    let mut all = Context::all();
    all.sort_by_key(|c| c.name());
    assert_eq!(reached, all);
}

// ---------------------------------------------------------------------------------------------
// The same configuration, merged into the old and the new keymap, gives the same keymap.
// ---------------------------------------------------------------------------------------------

/// One-entry tables that probe the merge rules, each written under every old surface name.
const PROBES: &[(&str, &str)] = &[
    ("j", "noop"),    // unbind a key the layers give
    ("k", "refresh"), // override a layer-1 key
    ("g", "refresh"), // override a key that is layer 2 in some, own in others
    ("G", "noop"),
    ("h", "noop"),
    ("l", "tab_new"), // bind a global action
    ("0", "noop"),
    ("$", "refresh"),
    ("q", "noop"),
    ("Z", "refresh"), // a new key
    ("t", "refresh"), // steal a global key (conflict)
    ("T", "noop"),    // unbind a global key in a context
    ("Q", "refresh"),
    ("1", "refresh"), // steal a tab-goto digit
    ("?", "refresh"),
    ("y", "quit"), // steal a leader prefix (conflict where it is one)
    ("space", "quit"),
    ("y z", "copy_full"), // a leader suffix
    ("space z", "file_delete"),
    ("pagedown", "noop"),
    ("ctrl-f", "refresh"),
    ("f", "navigate:page_down"),
    ("ctrl-d", "noop"),
    ("enter", "quit"),         // fixed key (warning)
    ("zz", "refresh"),         // not a key (warning)
    ("x", "no_such_action"),   // not an action (warning)
    ("a", "tab_goto"),         // fixed action (warning)
    ("g s", "open_sort_menu"), // anonymous prefix (warning)
    ("d", "noop"),
    ("R", "noop"),
    ("s", "navigate:down"),
    ("n", "noop"),
    ("v", "refresh"),
];

/// Every `[keys.<name>]` name the old keymap understood.
fn old_names() -> Vec<&'static str> {
    #[allow(unused_mut)]
    let mut v = vec![
        "global",
        "tree",
        "tree_visual",
        "preview_text",
        "preview_text_visual",
        "preview_image",
        "preview_table",
        "sort",
        "bookmarks",
        "tabs",
        "outline",
        "info",
        "table_cell",
        "help",
        "nonsense",
    ];
    #[cfg(feature = "git")]
    v.extend([
        "preview_git_diff",
        "git_changes",
        "git_log",
        "git_graph",
        "git_graph_picker",
        "git_branches",
        "git_worktrees",
        "git_detail",
    ]);
    v
}

fn cfg_from(tables: &[(&str, &[(&str, &str)])]) -> KeysFileConfig {
    let mut cfg = KeysFileConfig::default();
    for (name, entries) in tables {
        let t: HashMap<String, String> = entries
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        cfg.surfaces.insert((*name).to_string(), t);
    }
    cfg
}

/// The conflicts as a comparable, order-free list.
fn conflict_ids(cs: &[KeyConflict]) -> Vec<String> {
    let mut v: Vec<String> = cs
        .iter()
        .map(|c| format!("{:?} {} {:?}", c.surface, c.key.label(), c.reason))
        .collect();
    v.sort();
    v
}

fn compare_config(cfg: &KeysFileConfig, scheme: KeyScheme, what: &str) -> usize {
    let old = LegacyKeyMap::from_config(scheme, cfg);
    let new = KeyMap::from_config(scheme, cfg);
    let n = compare_all(&old, &new, what, false);
    assert_eq!(
        conflict_ids(&old.conflicts),
        conflict_ids(&new.conflicts),
        "{what}: conflicts"
    );
    let (mut a, mut b) = (old.warnings.clone(), new.warnings.clone());
    a.sort();
    b.sort();
    assert_eq!(a, b, "{what}: warnings");
    for (id, menu) in &old.leaders {
        let m2 = &new.leaders[id];
        let x: Vec<_> = menu.items.iter().map(|i| (i.key, i.action)).collect();
        let y: Vec<_> = m2.items.iter().map(|i| (i.key, i.action)).collect();
        assert_eq!(x, y, "{what}: leader {id:?}");
    }
    n
}

#[test]
fn a_merged_config_matches_the_old_keymap_for_every_old_name_and_probe() {
    let mut total = 0;
    let mut cases = 0;
    for scheme in [KeyScheme::Vim, KeyScheme::Less] {
        for name in old_names() {
            for &(k, a) in PROBES {
                let cfg = cfg_from(&[(name, &[(k, a)])]);
                total += compare_config(&cfg, scheme, &format!("[{name}] {k}={a} {scheme:?}"));
                cases += 1;
            }
            // All the probes at once (they interact: an unbind then an override, ...).
            let cfg = cfg_from(&[(name, PROBES)]);
            total += compare_config(&cfg, scheme, &format!("[{name}] all probes {scheme:?}"));
            cases += 1;
        }
    }
    println!("equivalence: {cases} configs, {total} lookups compared after merging");
    assert!(cases > 1000);
}

#[test]
fn several_tables_at_once_match_the_old_keymap() {
    for scheme in [KeyScheme::Vim, KeyScheme::Less] {
        let cfg = cfg_from(&[
            ("global", &[("W", "tab_close"), ("t", "refresh")]),
            ("tree", &[("j", "noop"), ("space z", "file_delete")]),
            ("preview_text", &[("q", "noop"), ("o", "refresh")]),
            ("preview_image", &[("0", "noop"), ("g", "navigate:top")]),
            ("preview_table", &[("J", "noop"), ("y", "quit")]),
            ("sort", &[("n", "noop")]),
            ("bookmarks", &[("j", "noop"), ("e", "refresh")]),
            ("nonsense", &[("a", "refresh")]),
        ]);
        compare_config(&cfg, scheme, &format!("several tables {scheme:?}"));
    }
}

#[cfg(feature = "git")]
#[test]
fn several_git_tables_at_once_match_the_old_keymap() {
    for scheme in [KeyScheme::Vim, KeyScheme::Less] {
        let cfg = cfg_from(&[
            (
                "git_changes",
                &[("g", "noop"), ("G", "navigate:bottom"), ("R", "noop")],
            ),
            ("git_log", &[("l", "noop"), ("g", "refresh")]),
            ("git_graph", &[("0", "navigate:line_home"), ("x", "noop")]),
            ("git_graph_picker", &[("g", "noop"), ("n", "noop")]),
            ("git_branches", &[("n", "noop"), ("G", "noop")]),
            ("git_worktrees", &[("n", "refresh")]),
            ("git_detail", &[("s", "noop"), ("ctrl-f", "noop")]),
            (
                "preview_git_diff",
                &[("f", "noop"), ("s", "noop"), ("J", "noop")],
            ),
        ]);
        compare_config(&cfg, scheme, &format!("several git tables {scheme:?}"));
    }
}
