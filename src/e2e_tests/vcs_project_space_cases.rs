// The host's shared operations in the project canvas's `Space` menu: each is listed iff its key
// acts there, with the words and the order the old hub listed them in, and the menu letter does
// what the key does.

use crate::keymap::{Action, ProjectMenuControl};
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

/// The canvas's own controls, with the plain key the menu lists each under where it has one.
const CONTROL_KEYS: [(Action, Option<char>); 4] = [
    (Action::ProjectControl(ProjectMenuControl::Search), Some('/')),
    (Action::ProjectControl(ProjectMenuControl::NextScreen), None),
    (Action::ProjectControl(ProjectMenuControl::PrevScreen), None),
    (Action::ProjectControl(ProjectMenuControl::Back), Some('h')),
];

fn is_control(a: Action) -> bool {
    CONTROL_KEYS.iter().any(|(c, _)| *c == a)
}

fn is_host(a: Action) -> bool {
    is_shared(a) || is_control(a)
}

/// The controls that act on this screen, written from the rule: search on a list, the screen
/// cycle off a pushed screen (`Tab` does nothing there), back always.
fn expected_controls(sim: &Sim) -> Vec<Action> {
    if sim.app.project_pushed() {
        vec![Action::ProjectControl(ProjectMenuControl::Back)]
    } else {
        CONTROL_KEYS.iter().map(|(a, _)| *a).collect()
    }
}

/// The host items as the old hub listed them (label and letter; `*` where the canvas hands a free
/// letter), frozen from the old hub before it was removed.
const FIXED_HOST: &str = include_str!("vcs_space_host_fixed.txt");

fn host_entry(item: &MenuItem) -> String {
    let keyless = CONTROL_KEYS.iter().any(|(c, l)| *c == item.action && l.is_none());
    format!("{}={}", item.label, if keyless { "*".to_string() } else { format!("{:?}", item.key.code) })
}

fn check_space_shared(sim: &Sim, check_host: bool, tool_screen: bool, what: &str) {
    let items = space_items(&sim.app);
    let shared: Vec<&MenuItem> = items.iter().filter(|i| is_shared(i.action)).collect();
    assert_eq!(
        shared.iter().map(|i| i.action).collect::<Vec<_>>(),
        expected_shared(sim, tool_screen),
        "{what}: the shared items, in order\n{items:?}"
    );
    let controls: Vec<&MenuItem> = items.iter().filter(|i| is_control(i.action)).collect();
    assert_eq!(
        controls.iter().map(|i| i.action).collect::<Vec<_>>(),
        expected_controls(sim),
        "{what}: the canvas controls, in order\n{items:?}"
    );
    for item in &controls {
        let mut want = CONTROL_KEYS.iter().find(|(a, _)| *a == item.action).unwrap().1;
        // On the pushed detail (a text screen) `h` pans: the back keys are `q` and `Esc`, and the
        // menu does not list `q` as a letter, so the item takes a free one.
        if sim.app.project_pushed() && item.action == Action::ProjectControl(ProjectMenuControl::Back) {
            want = None;
        }
        if let Some(c) = want {
            assert_eq!(item.key.code, KeyCode::Char(c), "{what}: {item:?}");
        }
    }
    // Every plugin item comes before every host one.
    let first_host = items.iter().position(|i| is_host(i.action)).unwrap();
    assert!(
        items[first_host..].iter().all(|i| is_host(i.action)),
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
    if check_host {
        let host: Vec<&MenuItem> = items.iter().filter(|i| is_host(i.action)).collect();
        let new_line: String = host.iter().map(|i| host_entry(i)).collect::<Vec<_>>().join(";");
        let want = FIXED_HOST
            .lines()
            .find_map(|l| l.strip_prefix(what)?.strip_prefix('\t'))
            .unwrap_or_else(|| panic!("{what}: no fixed host items"));
        assert_eq!(new_line, want, "{what}: the host items as the old hub listed them");
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
    // Each repository is read clean (empty changes list) and then with a changed file.
    for (root, id, is_jj, dirty) in cases
        .iter()
        .flat_map(|(root, id, is_jj)| [(*root, *id, *is_jj, false), (*root, *id, *is_jj, true)])
    {
        if dirty {
            std::fs::write(root.join("dirty.txt"), "x\n").unwrap();
        }
        for lang in ["en", "ja"] {
            let mut sim = open(root, config(id, lang), (160, 20));
            for name in walk_names(is_jj) {
                screen(&mut sim, name);
                let what = format!("{id} {lang} {name} dirty={dirty}");
                check_space_shared(&sim, true, *name == "changes", &what);
                checked += 1;
            }
            // The pushed detail: the old hub's detail has no `Space` items of the shared kind
            // beyond the tail, and no tool.
            screen(&mut sim, "log");
            press(&mut sim, KeyCode::Enter);
            check_space_shared(&sim, false, false, &format!("{id} {lang} detail"));
            checked += 1;
        }
    }
    assert!(checked >= if jj.is_some() { 44 } else { 24 }, "{checked}");
}

/// (project active, screen name, pushed, text input open)
type ControlState = (bool, String, bool, bool);

/// What a canvas control changes, observed after it ran.
fn control_state(sim: &Sim) -> ControlState {
    (
        sim.app.project_active(),
        sim.app
            .project_frame()
            .map(|f| f.key.screen.clone())
            .unwrap_or_default(),
        sim.app.project_pushed(),
        sim.app.surface().is_text_input()
            || sim.app.project_frame().is_some_and(|f| f.filtering),
    )
}

/// Run `control` on a fresh canvas at `name` (`detail` = a pushed detail) by its key, and the
/// same control by `Space` and its menu letter; both end in the same state.
fn control_run(
    root: &std::path::Path,
    id: &str,
    lang: &str,
    name: &str,
    control: Action,
    key: KeyCode,
    by_menu: bool,
) -> (ControlState, ControlState, bool) {
    let mut sim = open(root, config(id, lang), (160, 20));
    if name == "detail" {
        screen(&mut sim, "log");
        press(&mut sim, KeyCode::Enter);
        assert!(sim.app.project_pushed());
    } else {
        screen(&mut sim, name);
    }
    let before = control_state(&sim);
    let listed = space_items(&sim.app).iter().any(|i| i.action == control);
    if by_menu {
        let letter = space_items(&sim.app)
            .into_iter()
            .find(|i| i.action == control)
            .map(|i| i.key);
        if let Some(letter) = letter {
            press(&mut sim, KeyCode::Char(' '));
            press(&mut sim, letter.code);
        }
    } else {
        press(&mut sim, key);
    }
    (before, control_state(&sim), listed)
}

#[test]
fn vcs_port_project_space_controls_do_what_their_keys_do() {
    if isolated("vcs_port_project_space_controls_do_what_their_keys_do") {
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
    use ProjectMenuControl as C;
    let controls: [(Action, KeyCode); 4] = [
        (Action::ProjectControl(C::Search), KeyCode::Char('/')),
        (Action::ProjectControl(C::NextScreen), KeyCode::Tab),
        (Action::ProjectControl(C::PrevScreen), KeyCode::BackTab),
        (Action::ProjectControl(C::Back), KeyCode::Char('h')),
    ];
    let mut compared = 0;
    for (root, id, is_jj) in cases {
        // A changed file, so the changes list has a row the filter can act on.
        std::fs::write(root.join("dirty.txt"), "x\n").unwrap();
        for lang in ["en", "ja"] {
            let mut names: Vec<&str> = walk_names(is_jj).to_vec();
            names.push("detail");
            for name in names {
                for (control, key) in controls {
                    let what = format!("{id} {lang} {name} {control:?}");
                    // The pushed detail is a text screen: `h` pans there, and `q` goes back.
                    let key = if name == "detail" && key == KeyCode::Char('h') {
                        let (before, by_h, _) =
                            control_run(root, id, lang, name, control, key, false);
                        assert_eq!(by_h, before, "{what}: h pans on the detail, it does not go back");
                        KeyCode::Char('q')
                    } else {
                        key
                    };
                    let (before, by_key, listed) =
                        control_run(root, id, lang, name, control, key, false);
                    let (_, by_menu, _) = control_run(root, id, lang, name, control, key, true);
                    // The key acts iff the menu lists it, and the menu letter ends where the key does.
                    assert_eq!(
                        by_key != before,
                        listed,
                        "{what}: listed iff the key acts (before {before:?}, by key {by_key:?})"
                    );
                    assert_eq!(by_menu, by_key, "{what}: the menu letter does what the key does");
                    // The effect itself, by control.
                    match control {
                        Action::ProjectControl(C::Search) if listed => {
                            assert!(by_key.3, "{what}: filter")
                        }
                        Action::ProjectControl(C::NextScreen | C::PrevScreen) if listed => {
                            assert_ne!(by_key.1, before.1, "{what}: another screen")
                        }
                        // A pushed screen pops; a root screen leaves the project.
                        Action::ProjectControl(C::Back) if before.2 => {
                            assert!(by_key.0 && !by_key.2, "{what}: popped")
                        }
                        Action::ProjectControl(C::Back) => assert!(!by_key.0, "{what}: left"),
                        _ => {}
                    }
                    compared += 1;
                }
            }
        }
    }
    assert!(compared >= if jj.is_some() { 80 } else { 48 }, "{compared}");
}

#[test]
fn vcs_port_project_space_refresh_runs_the_keys_path() {
    if isolated("vcs_port_project_space_refresh_runs_the_keys_path") {
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
    let mut n = 0;
    for (root, id) in cases {
        for by_menu in [false, true] {
            n += 1;
            let file = format!("fresh_{id}_{n}.txt");
            let mut sim = open(root, config(id, "en"), (160, 20));
            screen(&mut sim, "changes");
            assert!(!sim.screen().contains(&file));
            std::fs::write(root.join(&file), "x\n").unwrap();
            drain(&mut sim);
            assert!(
                !sim.screen().contains(&file),
                "{id}: the new file is not read before a refresh"
            );
            if by_menu {
                let letter = space_items(&sim.app)
                    .into_iter()
                    .find(|i| i.action == Action::Refresh)
                    .expect("refresh is listed")
                    .key;
                press(&mut sim, KeyCode::Char(' '));
                press(&mut sim, letter.code);
            } else {
                press(&mut sim, KeyCode::Char('r'));
            }
            drain(&mut sim);
            assert!(
                sim.screen().contains(&file),
                "{id} by_menu={by_menu}: the refresh reads the new file\n{}",
                sim.screen()
            );
        }
    }
}

/// The menu's refresh item takes the key `r`'s path, busy guard included: while the screen's job
/// holds the session both the key and `Space` + the item's letter queue behind it, and with a full
/// queue both answer with the busy notice. Idle, both refresh (see
/// `vcs_port_project_space_refresh_runs_the_keys_path`).
#[test]
fn vcs_port_project_space_refresh_has_the_keys_busy_guard() {
    if isolated("vcs_port_project_space_refresh_has_the_keys_busy_guard") {
        return;
    }
    let git = super::vcs_changes::git_repo("vcs_space_refresh_busy", true);
    let jj = if crate::vcs::jj::available() {
        let root = super::vcs_log::jj_repo(false);
        std::fs::write(root.join("changed.txt"), "new\n").unwrap();
        Some(root)
    } else {
        None
    };
    let mut cases: Vec<(&'static str, &str, &std::path::Path)> =
        vec![("git", "git.changes", git.as_ref())];
    if let Some(jj) = &jj {
        cases.push(("jj", "jj.changes", jj.as_ref()));
    }
    let queue_cap = 4096;
    let mut compared = 0;
    for (backend, table, root) in cases {
        for lang in ["en", "ja"] {
            for full in [false, true] {
                // Filling the queue is slow, so the full-queue case runs in en only.
                if full && lang == "ja" {
                    continue;
                }
                let mut seen = Vec::new();
                for by_menu in [false, true] {
                    let what = format!("{backend} {lang} full={full} by_menu={by_menu}");
                    let (mut cfg, h) = wt_flaky_cfg(backend, table);
                    cfg.ui.lang = lang.into();
                    let mut sim = open(root, cfg, (160, 20));
                    screen(&mut sim, "changes");
                    // `r` starts a read the plugin holds: the view is waiting.
                    h.hold.store(true, std::sync::atomic::Ordering::SeqCst);
                    sim.press(KeyCode::Char('r'), KeyModifiers::NONE);
                    assert!(sim.app.project_jobs_pending(), "{what}: r did not start a job");
                    if full {
                        // An unbound key waits in line too; fill the queue to its bound.
                        let have = sim.app.project_frame().unwrap().pending.len();
                        for _ in have..queue_cap {
                            sim.app.handle_project_key(crossterm::event::KeyEvent::new(
                                KeyCode::Char('W'),
                                KeyModifiers::NONE,
                            ));
                        }
                        assert_eq!(
                            sim.app.project_frame().unwrap().pending.len(),
                            queue_cap,
                            "{what}"
                        );
                    }
                    let letter = space_items(&sim.app)
                        .into_iter()
                        .find(|i| i.action == Action::Refresh)
                        .expect("refresh is listed")
                        .key;
                    let before = sim.app.project_frame().unwrap().pending.len();
                    sim.app.flash = None;
                    if by_menu {
                        // While a job holds the view every press, `Space` and a menu letter
                        // included, waits in line in the host before the menu sees it. What the
                        // menu does with its letter once it has it is `find` then
                        // `dispatch_action`; run exactly that.
                        let action = crate::ops::menu::find(
                            &sim.app,
                            Context::Canvas,
                            LeaderId::Actions,
                            letter,
                        )
                        .expect("the letter is in the menu");
                        assert_eq!(action, Action::Refresh, "{what}");
                        let surface = sim.app.surface();
                        crate::dispatch_action(&mut sim.app, action, surface).unwrap();
                    } else {
                        sim.press(KeyCode::Char('r'), KeyModifiers::NONE);
                    }
                    let after = (
                        sim.app.project_frame().unwrap().pending.len(),
                        sim.app.flash.clone(),
                        sim.app.project_jobs_pending(),
                    );
                    if full {
                        assert_eq!(after.0, before, "{what}: nothing more is queued");
                        assert_eq!(
                            after.1.as_deref(),
                            Some(tr(sim.app.lang, Msg::ProjectBusy)),
                            "{what}: the busy notice"
                        );
                    } else {
                        assert_eq!(after.0, before + 1, "{what}: queued behind the job");
                        assert_eq!(after.1, None, "{what}");
                    }
                    assert!(after.2, "{what}: still waiting on the one held job");
                    assert_eq!(sim.app.pending_leader, None, "{what}");
                    seen.push(after);
                    h.latch.open();
                    drain(&mut sim);
                    compared += 1;
                }
                assert_eq!(seen[0], seen[1], "{backend} {lang} full={full}: key vs menu");
            }
        }
    }
    assert!(compared >= if jj.is_some() { 12 } else { 6 }, "{compared}");
}

/// Letters are assigned over the whole candidate list and the gate then hides what does not act,
/// so an item keeps its letter when another stops acting: the same on a list screen and on the
/// pushed detail of the same project, also when `[keys]` takes a shared operation's key away.
#[test]
fn vcs_port_project_space_letters_do_not_move_between_screens() {
    if isolated("vcs_port_project_space_letters_do_not_move_between_screens") {
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
    // Operations whose key `[keys]` takes away one by one, then all together: each is then a
    // keyless item whose letter is picked from the free ones.
    let unbound: [&[&str]; 8] = [
        &[],
        &["r"],
        &["t"],
        &["T"],
        &["P"],
        &["F"],
        &["?"],
        &["r", "t", "T", "P", "F", "?"],
    ];
    use ProjectMenuControl as C;
    let mut compared = 0;
    for (root, id) in cases {
        for keys_off in unbound {
            let make = || {
                let mut cfg = config(id, "en");
                cfg.keys.surfaces.insert(
                    "canvas".into(),
                    keys_off.iter().map(|k| (k.to_string(), "noop".into())).collect(),
                );
                open(root, cfg, (160, 20))
            };
            let mut list = make();
            screen(&mut list, "log");
            let on_list = space_items(&list.app);
            let mut pushed = make();
            screen(&mut pushed, "log");
            press(&mut pushed, KeyCode::Enter);
            assert!(pushed.app.project_pushed(), "{id} {keys_off:?}");
            let on_pushed = space_items(&pushed.app);
            // The screen cycle does not act on the pushed screen, the search not on its text
            // detail: they are hidden there, and every item that stays has the letter it has
            // in the full candidate list (the same screen with nothing gated).
            for (sim, shown, screen_name) in [
                (&list, &on_list, "list"),
                (&pushed, &on_pushed, "pushed"),
            ] {
                let all = crate::ops::menu::declared_actions_menu(&sim.app, Context::Canvas);
                // Every control is a candidate on every screen (the letters are assigned over
                // all of them), also the ones the gate hides here.
                for control in [C::Search, C::NextScreen, C::PrevScreen, C::Back] {
                    assert!(
                        all.iter().any(|a| a.action == Action::ProjectControl(control)),
                        "{id} {keys_off:?} {screen_name}: {control:?} is not a candidate\nall={all:?}"
                    );
                }
                for item in shown {
                    let full = all
                        .iter()
                        .find(|a| a.action == item.action)
                        .unwrap_or_else(|| panic!("{id} {screen_name}: {item:?} not a candidate"));
                    assert_eq!(
                        item.key, full.key,
                        "{id} {keys_off:?} {screen_name}: {:?} letter moved when the others were hidden\nshown={shown:?}\nall={all:?}",
                        item.action
                    );
                    compared += 1;
                }
            }
            // The pushed screen hides exactly the items whose keys do not act there.
            assert!(
                on_list.iter().any(|i| i.action == Action::ProjectControl(C::NextScreen)),
                "{id} {keys_off:?}"
            );
            for hidden in [C::NextScreen, C::PrevScreen] {
                assert!(
                    !on_pushed.iter().any(|i| i.action == Action::ProjectControl(hidden)),
                    "{id} {keys_off:?}: {hidden:?} on the pushed screen"
                );
            }
        }
    }
    assert!(compared >= if jj.is_some() { 120 } else { 60 }, "{compared}");
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
    // (key, the action it runs, what must have changed from the start). `r` has its own test
    // (`vcs_port_project_space_refresh_runs_the_keys_path`): the key is the project host's, so it
    // never reaches `dispatch_action`.
    let ops: [(char, Action); 5] = [
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
                    assert_eq!(after_key.4, Some(action), "{what}: the key ran it");
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
    assert!(compared >= if jj.is_some() { 40 } else { 20 }, "{compared}");
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
fn vcs_port_project_space_an_item_whose_key_is_unbound_stays_in_the_menu_and_runs() {
    if isolated("vcs_port_project_space_an_item_whose_key_is_unbound_stays_in_the_menu_and_runs") {
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
