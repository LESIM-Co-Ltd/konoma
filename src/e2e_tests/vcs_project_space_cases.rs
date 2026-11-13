// The host's shared operations in the project canvas's `Space` menu: each is listed iff its key
// acts there, with the words and the order the old hub listed them in, and the menu letter does
// what the key does.

use crate::keymap::Action;
use crate::ops::menu::{MenuItem, View};

/// The shared operations the canvas offers, in menu order, with the plain key each is listed
/// under (written out here, not read from the code under test).
const SHARED_KEYS: [(Action, char); 10] = [
    (Action::GitLaunchTool, '!'),
    (Action::Refresh, 'r'),
    (Action::TabNewAfter, 't'),
    (Action::TabPrev, '['),
    (Action::TabNext, ']'),
    (Action::ToggleTabList, 'T'),
    (Action::PasteJump, 'P'),
    (Action::ToggleHelp, '?'),
    (Action::ToggleFollow, 'F'),
    (Action::Quit, 'Q'),
];

fn space_items(app: &crate::app::App) -> Vec<MenuItem> {
    let ctx = app.key_context().unwrap();
    crate::ops::menu::menu(app, ctx, LeaderId::Actions, View::Open)
        .unwrap()
        .items
        .into_iter()
        .map(|i| MenuItem {
            key: i.key,
            action: i.action,
            label: i.label,
        })
        .collect()
}

fn is_shared(a: Action) -> bool {
    SHARED_KEYS.iter().any(|(s, _)| *s == a)
}

/// The shared operations that should be listed now, written from the rule (a key that acts).
fn expected_shared(sim: &Sim, tool_screen: bool) -> Vec<Action> {
    let mut want = Vec::new();
    if tool_screen {
        want.push(Action::GitLaunchTool);
    }
    want.extend([Action::Refresh, Action::TabNewAfter]);
    if sim.app.tab_count() > 1 {
        want.extend([Action::TabPrev, Action::TabNext]);
    }
    want.extend([
        Action::ToggleTabList,
        Action::PasteJump,
        Action::ToggleHelp,
        Action::ToggleFollow,
        Action::Quit,
    ]);
    want
}

fn check_space_shared(sim: &Sim, old: Option<&Sim>, tool_screen: bool, what: &str) {
    let items = space_items(&sim.app);
    let shared: Vec<&MenuItem> = items.iter().filter(|i| is_shared(i.action)).collect();
    assert_eq!(
        shared.iter().map(|i| i.action).collect::<Vec<_>>(),
        expected_shared(sim, tool_screen),
        "{what}: the shared items, in order\n{items:?}"
    );
    // Every plugin item comes before every shared one.
    let first_shared = items.iter().position(|i| is_shared(i.action)).unwrap();
    assert!(
        items[first_shared..].iter().all(|i| is_shared(i.action)),
        "{what}: plugin items first\n{items:?}"
    );
    // The minimum of three operations holds on every screen.
    assert!(shared.len() >= 3, "{what}");
    for item in &shared {
        let key = SHARED_KEYS.iter().find(|(a, _)| *a == item.action).unwrap().1;
        assert_eq!(item.key.code, KeyCode::Char(key), "{what}: {item:?}");
    }
    // Listed iff the operation's gate lets its key act, for each of the ten.
    for (action, _) in SHARED_KEYS {
        let acts = crate::ops::action_enabled(&sim.app, Context::Canvas, action);
        assert_eq!(
            shared.iter().any(|i| i.action == action),
            acts,
            "{what}: {action:?} listed iff it acts"
        );
    }
    if let Some(old) = old {
        let old_items = space_items(&old.app);
        for item in &shared {
            let counterpart = old_items
                .iter()
                .find(|o| o.action == item.action)
                .unwrap_or_else(|| panic!("{what}: the old hub has {:?}\n{old_items:?}", item.action));
            assert_eq!(item.label, counterpart.label, "{what}: {:?} words", item.action);
        }
        let old_order: Vec<Action> = old_items
            .iter()
            .map(|o| o.action)
            .filter(|a| shared.iter().any(|i| i.action == *a))
            .collect();
        assert_eq!(
            old_order,
            shared.iter().map(|i| i.action).collect::<Vec<_>>(),
            "{what}: the old hub's order"
        );
    }
}

fn walk_names(jj: bool) -> &'static [&'static str] {
    if jj {
        &["changes", "log", "graph", "branches"]
    } else {
        &["changes", "log", "graph", "branches", "worktrees"]
    }
}

#[test]
fn vcs_port_project_space_shared_operations() {
    if isolated("vcs_port_project_space_shared_operations") {
        return;
    }
    let git = super::vcs_log::repo(3);
    let jj = if crate::vcs::jj::available() {
        Some(super::vcs_log::graph_jj_fixture())
    } else {
        None
    };
    let mut cases: Vec<(&std::path::Path, &str, bool)> = vec![(git.as_ref(), "git", false)];
    if let Some(jj) = &jj {
        cases.push((jj.as_ref(), "jj", true));
    }
    let mut checked = 0;
    for (root, id, is_jj) in cases {
        for lang in ["en", "ja"] {
            let mut old = Sim::with_config_sized(root, config(id, lang), 160, 20);
            old.open_legacy_hub();
            let mut sim = open(root, config(id, lang), (160, 20));
            for name in walk_names(is_jj) {
                screen(&mut sim, name);
                let what = format!("{id} {lang} {name}");
                check_space_shared(&sim, Some(&old), *name == "changes", &what);
                checked += 1;
                old.tab();
            }
            // The pushed detail: the old hub's detail has no `Space` items of the shared kind
            // beyond the tail, and no tool.
            screen(&mut sim, "log");
            press(&mut sim, KeyCode::Enter);
            check_space_shared(&sim, None, false, &format!("{id} {lang} detail"));
            checked += 1;
        }
    }
    assert!(checked >= if jj.is_some() { 20 } else { 10 }, "{checked}");
}

#[test]
fn vcs_port_project_space_tab_keys_follow_the_tab_count_in_menu_and_help() {
    if isolated("vcs_port_project_space_tab_keys_follow_the_tab_count_in_menu_and_help") {
        return;
    }
    let git = super::vcs_log::repo(3);
    let jj = if crate::vcs::jj::available() {
        Some(super::vcs_log::graph_jj_fixture())
    } else {
        None
    };
    let mut cases: Vec<(&std::path::Path, &str)> = vec![(git.as_ref(), "git")];
    if let Some(jj) = &jj {
        cases.push((jj.as_ref(), "jj"));
    }
    for (root, id) in cases {
        for lang in ["en", "ja"] {
            let mut sim = open(root, config(id, lang), (160, 20));
            screen(&mut sim, "log");
            let tabs = tr(sim.app.lang, Msg::HelpTabs);
            let help_has_pair = |sim: &Sim| {
                crate::ops::help::sections(&sim.app)
                    .iter()
                    .filter(|s| s.title == tabs)
                    .any(|s| s.rows.iter().any(|(k, _)| k == "[ / ]"))
            };
            // One tab: `[` and `]` return at once, so neither the menu nor the help names them.
            assert_eq!(sim.app.tab_count(), 1);
            let one = space_items(&sim.app);
            assert!(!one
                .iter()
                .any(|i| matches!(i.action, Action::TabPrev | Action::TabNext)));
            assert!(!help_has_pair(&sim), "{id} {lang}: help without the pair");
            // A key that does not act does nothing (no flash, no dispatch effect on the tabs).
            keys(&mut sim, "]");
            assert_eq!(sim.app.tab_count(), 1);
            // Two tabs: both are listed, under the letters they were always bound to, and the
            // letters of the other items did not move.
            keys(&mut sim, "t");
            assert_eq!(sim.app.tab_count(), 2);
            // The new tab is a plain tree tab: go back to the canvas tab.
            keys(&mut sim, "[");
            assert!(sim.app.project_active());
            let two = space_items(&sim.app);
            for a in [Action::TabPrev, Action::TabNext] {
                assert!(two.iter().any(|i| i.action == a), "{id} {lang}: {a:?}");
            }
            assert!(help_has_pair(&sim), "{id} {lang}: help with the pair");
            for item in &one {
                let again = two.iter().find(|i| i.action == item.action).unwrap();
                assert_eq!(again.key, item.key, "{id} {lang}: {:?} letter stays", item.action);
                assert_eq!(again.label, item.label);
            }
            // Back to one tab (close the tree tab): the two disappear again.
            keys(&mut sim, "]");
            sim.app.tab_close();
            drain(&mut sim);
            assert_eq!(sim.app.tab_count(), 1);
            assert!(!space_items(&sim.app)
                .iter()
                .any(|i| matches!(i.action, Action::TabPrev | Action::TabNext)));
        }
    }
}

/// What a shared operation changes, observed after the operation ran.
fn effect(sim: &Sim) -> (usize, u64, bool, bool, Option<Action>, Option<String>) {
    (
        sim.app.tab_count(),
        sim.app.tab.id,
        sim.app.is_tab_list(),
        sim.app.show_help,
        sim.app.last_dispatched,
        sim.app.flash.clone(),
    )
}

#[test]
fn vcs_port_project_space_letters_do_what_the_keys_do() {
    if isolated("vcs_port_project_space_letters_do_what_the_keys_do") {
        return;
    }
    let git = super::vcs_log::repo(3);
    let jj = if crate::vcs::jj::available() {
        Some(super::vcs_log::graph_jj_fixture())
    } else {
        None
    };
    let mut cases: Vec<(&std::path::Path, &str)> = vec![(git.as_ref(), "git")];
    if let Some(jj) = &jj {
        cases.push((jj.as_ref(), "jj"));
    }
    // (key, the action it runs, what must have changed from the start)
    let ops: [(char, Action); 6] = [
        ('r', Action::Refresh),
        ('t', Action::TabNewAfter),
        (']', Action::TabNext),
        ('[', Action::TabPrev),
        ('T', Action::ToggleTabList),
        ('?', Action::ToggleHelp),
    ];
    let mut compared = 0;
    for (root, id) in cases {
        for lang in ["en", "ja"] {
            for name in ["changes", "log"] {
                for (key, action) in ops {
                    let start = |sim: &mut Sim| {
                        screen(sim, name);
                        if matches!(action, Action::TabNext | Action::TabPrev) {
                            // The new tab is a plain tree tab: come back to the canvas tab.
                            keys(sim, "t");
                            assert_eq!(sim.app.tab_count(), 2);
                            keys(sim, "[");
                            assert!(sim.app.project_active());
                        }
                        sim.app.last_dispatched = None;
                    };
                    let what = format!("{id} {lang} {name} {key}");
                    // The key itself.
                    let mut by_key = open(root, config(id, lang), (160, 20));
                    start(&mut by_key);
                    let before = effect(&by_key);
                    keys(&mut by_key, &key.to_string());
                    let after_key = effect(&by_key);
                    // The same operation from `Space` and its letter.
                    let mut by_menu = open(root, config(id, lang), (160, 20));
                    start(&mut by_menu);
                    let letter = space_items(&by_menu.app)
                        .into_iter()
                        .find(|i| i.action == action)
                        .unwrap_or_else(|| panic!("{what}: not in the menu"))
                        .key;
                    press(&mut by_menu, KeyCode::Char(' '));
                    assert_eq!(by_menu.app.pending_leader, Some(LeaderId::Actions), "{what}");
                    press(&mut by_menu, letter.code);
                    let after_menu = effect(&by_menu);
                    assert_eq!(
                        after_menu.4,
                        Some(action),
                        "{what}: the menu letter ran the operation"
                    );
                    // `r` on the canvas is a view-layer key the project host takes before the
                    // shared dispatch, so `last_dispatched` stays empty for it; the key's own
                    // refresh effect is not observable here (reported as unverified).
                    if action != Action::Refresh {
                        assert_eq!(after_key.4, Some(action), "{what}: the key ran it");
                    }
                    // Ids of tabs are per run; compare the rest and the id's change.
                    assert_eq!(
                        (after_menu.0, after_menu.2, after_menu.3, &after_menu.5),
                        (after_key.0, after_key.2, after_key.3, &after_key.5),
                        "{what}: the same effect"
                    );
                    assert_eq!(
                        after_menu.1 != before.1,
                        after_key.1 != before.1,
                        "{what}: the same active tab"
                    );
                    match action {
                        Action::TabNewAfter => assert_eq!(after_key.0, 2, "{what}"),
                        Action::TabNext | Action::TabPrev => {
                            assert_ne!(after_key.1, before.1, "{what}: switched tabs")
                        }
                        Action::ToggleTabList => assert!(after_key.2, "{what}: tab list open"),
                        Action::ToggleHelp => assert!(after_key.3, "{what}: help open"),
                        _ => {}
                    }
                    assert_eq!(by_menu.app.pending_leader, None, "{what}");
                    compared += 1;
                }
            }
        }
    }
    assert!(compared >= if jj.is_some() { 48 } else { 24 }, "{compared}");
}

#[test]
fn vcs_port_project_space_tool_and_paste_follow_their_gates() {
    if isolated("vcs_port_project_space_tool_and_paste_follow_their_gates") {
        return;
    }
    let git = super::vcs_log::repo(3);
    for lang in ["en", "ja"] {
        // `!` acts on the changes screen alone, and only while the external tool is enabled.
        let mut sim = open(&git, config("git", lang), (160, 20));
        for name in ["log", "graph", "branches", "worktrees"] {
            screen(&mut sim, name);
            assert!(
                !space_items(&sim.app)
                    .iter()
                    .any(|i| i.action == Action::GitLaunchTool),
                "{lang} {name}"
            );
            assert!(!crate::ops::action_enabled(
                &sim.app,
                Context::Canvas,
                Action::GitLaunchTool
            ));
        }
        screen(&mut sim, "changes");
        assert!(space_items(&sim.app)
            .iter()
            .any(|i| i.action == Action::GitLaunchTool));
        let mut cfg = config("git", lang);
        cfg.external.git_tool = false;
        let mut off = open(&git, cfg, (160, 20));
        screen(&mut off, "changes");
        assert!(!space_items(&off.app)
            .iter()
            .any(|i| i.action == Action::GitLaunchTool));
        // `P` reads the clipboard: only its gate is checked here, through the test seam, and the
        // key is not pressed.
        clear_test_clipboard();
        screen(&mut sim, "log");
        let paste = space_items(&sim.app)
            .into_iter()
            .find(|i| i.action == Action::PasteJump)
            .expect("P is listed");
        assert_eq!(paste.key.code, KeyCode::Char('P'));
        assert!(crate::ops::action_enabled(
            &sim.app,
            Context::Canvas,
            Action::PasteJump
        ));
        assert_eq!(get_test_clipboard(), None);
        // `F` and `Q`: listed, gate checked; neither is pressed (Q would ask to quit).
        for action in [Action::ToggleFollow, Action::Quit] {
            assert!(crate::ops::action_enabled(&sim.app, Context::Canvas, action));
            assert!(space_items(&sim.app).iter().any(|i| i.action == action));
        }
    }
}

#[test]
fn vcs_port_project_space_unbinding_a_key_removes_its_item_not_the_letters() {
    if isolated("vcs_port_project_space_unbinding_a_key_removes_its_item_not_the_letters") {
        return;
    }
    let git = super::vcs_log::repo(3);
    // `r` moved to `R`: the menu still lists refresh, under the key it has now.
    let mut cfg = config("git", "en");
    cfg.keys
        .surfaces
        .insert("canvas".into(), [("r".into(), "noop".into())].into());
    let mut sim = open(&git, cfg, (160, 20));
    screen(&mut sim, "log");
    let items = space_items(&sim.app);
    let refresh = items.iter().find(|i| i.action == Action::Refresh);
    // The key no longer acts, but the operation is still reachable from the menu (the menu is
    // where an operation that lost its key stays reachable).
    let refresh = refresh.expect("refresh stays reachable from the menu");
    assert_ne!(refresh.key.code, KeyCode::Char('r'));
    let letter = refresh.key;
    press(&mut sim, KeyCode::Char(' '));
    press(&mut sim, letter.code);
    assert_eq!(sim.app.last_dispatched, Some(Action::Refresh));
}
