// Overlays in front of a project screen keep their own keys; the actions left on over a project
// screen act on the project, never on the screen under it. Every case goes through `handle_key`.

use crate::app::list_filter::ListId;

const LINES_TXT: &str = "one\ntwo\nthree\nfour\nfive\nsix\nseven\neight\n";

/// A repo of the git fixture plus the files the cases use.
fn overlay_repo(name: &str) -> Option<crate::test_support::TmpDir> {
    let repo = esc_repo("git", name)?;
    std::fs::write(repo.join("notes.md"), HEADINGS_MD).unwrap();
    std::fs::write(repo.join("other.md"), "# Other\n\nbody\n").unwrap();
    std::fs::write(repo.join("lines.txt"), LINES_TXT).unwrap();
    Some(repo)
}

/// A session showing the preview of `file`, then (when `project`) the project screen over it with
/// `p`. Bookmarks live in `cfg_base`, never in the real config directory.
fn preview_session(
    repo: &std::path::Path,
    cfg: Config,
    file: &str,
    project: bool,
    cfg_base: &std::path::Path,
) -> Sim {
    let mut sim = Sim::with_config_sized(repo, cfg, 140, 30);
    sim.app.bookmarks = crate::bookmarks::Bookmarks::with_base(cfg_base.to_path_buf(), repo);
    sim.app.tab.projects.wait_for_result();
    sim.app.poll_projects();
    drain(&mut sim);
    sim.select(file);
    sim.enter();
    drain(&mut sim);
    assert_eq!(sim.app.tab.mode, Mode::Preview, "{file}: preview first");
    if project {
        sim.key('p');
        drain(&mut sim);
        assert!(sim.app.project_active(), "p opens the project over the preview");
    }
    sim
}

fn bind(cfg: &mut Config, table: &str, key: &str, action: &str) {
    cfg.keys
        .surfaces
        .entry(table.to_string())
        .or_default()
        .insert(key.to_string(), action.to_string());
}

fn ctrl_a() -> crate::keymap::KeyPress {
    crate::keymap::KeyPress::norm(&crossterm::event::KeyEvent::new(
        KeyCode::Char('a'),
        KeyModifiers::CONTROL,
    ))
}

/// What a scenario leaves, to compare a run over a project screen with one without it.
#[derive(Debug, PartialEq)]
struct Outcome {
    bookmark_list: bool,
    tab_list: bool,
    help: bool,
    info: bool,
    list_filter: bool,
    tabs: usize,
    mode: Mode,
    path: Option<std::path::PathBuf>,
}

fn outcome(sim: &Sim) -> Outcome {
    Outcome {
        bookmark_list: sim.app.is_bookmark_list(),
        tab_list: sim.app.is_tab_list(),
        help: sim.app.show_help,
        info: sim.app.is_info(),
        list_filter: sim.app.list_filtering(ListId::Bookmarks) || sim.app.list_filtering(ListId::Tabs),
        tabs: sim.app.tab_count(),
        mode: sim.app.tab.mode,
        path: sim.app.tab.preview_path.clone(),
    }
}

/// The overlay's own keys, as a list of (label, outcome after the keys).
fn bookmark_scenario(repo: &std::path::Path, project: bool, base: &std::path::Path) -> Vec<(String, Outcome)> {
    let mut sim = preview_session(repo, config("git", "en"), "notes.md", project, base);
    sim.app.bookmarks.set('a', repo.join("other.md")).unwrap();
    let mut log = Vec::new();
    let mut step = |label: &str, sim: &Sim| log.push((label.to_string(), outcome(sim)));
    sim.key('\'');
    drain(&mut sim);
    step("open", &sim);
    sim.key('q');
    step("q", &sim);
    sim.key('\'');
    sim.press(KeyCode::Esc, KeyModifiers::NONE);
    step("esc", &sim);
    sim.key('\'');
    sim.key('/');
    step("slash", &sim);
    sim.press(KeyCode::Esc, KeyModifiers::NONE);
    step("esc closes the filter", &sim);
    sim.press(KeyCode::Esc, KeyModifiers::NONE);
    step("esc closes the list", &sim);
    sim.key('\'');
    sim.press(KeyCode::Enter, KeyModifiers::NONE);
    drain(&mut sim);
    step("enter jumps", &sim);
    log
}

#[test]
fn vcs_overlay_bookmark_list_keeps_its_keys_over_a_project_screen() {
    if isolated("vcs_overlay_bookmark_list_keeps_its_keys_over_a_project_screen") {
        return;
    }
    let Some(repo) = overlay_repo("vcs_overlay_bookmarks") else {
        return;
    };
    let cfg = crate::test_support::unique_tmp("vcs_overlay_bookmarks_cfg");
    let with = bookmark_scenario(&repo, true, &cfg);
    let without = bookmark_scenario(&repo, false, &cfg);
    // The same keys do the same things over a project screen as without one (the project is
    // left by a jump, so only that step differs in the screen shown).
    assert_eq!(with.len(), without.len());
    for (a, b) in with.iter().zip(&without) {
        assert_eq!(a, b, "step {:?} differs with a project screen behind", a.0);
    }
    let open = &with[0].1;
    assert!(open.bookmark_list, "{open:?}");
    assert!(!with[1].1.bookmark_list, "q closes the list: {:?}", with[1].1);
    assert!(!with[2].1.bookmark_list, "Esc closes the list: {:?}", with[2].1);
    assert!(with[3].1.list_filter, "/ starts the list filter: {:?}", with[3].1);
    assert!(
        with[4].1.bookmark_list && !with[4].1.list_filter,
        "the first Esc ends the filter: {:?}",
        with[4].1
    );
    assert!(!with[5].1.bookmark_list, "the second Esc closes: {:?}", with[5].1);
    let jumped = &with[6].1;
    assert!(!jumped.bookmark_list);
    assert_eq!(jumped.mode, Mode::Preview);
    assert_eq!(jumped.path.as_deref(), Some(repo.join("other.md").as_path()), "{jumped:?}");
}

#[test]
fn vcs_overlay_a_bookmark_jump_shows_its_target_not_the_project_screen() {
    if isolated("vcs_overlay_a_bookmark_jump_shows_its_target_not_the_project_screen") {
        return;
    }
    let Some(repo) = overlay_repo("vcs_overlay_bm_jump") else {
        return;
    };
    let cfg = crate::test_support::unique_tmp("vcs_overlay_bm_jump_cfg");
    std::fs::create_dir_all(repo.join("sub")).unwrap();
    // The four jumps: by letter or from the list, to a file or to a directory.
    for (letter_jump, dir) in [(false, false), (false, true), (true, false), (true, true)] {
        let case = format!("letter_jump={letter_jump} dir={dir}");
        let mut sim = preview_session(&repo, config("git", "en"), "notes.md", true, &cfg);
        sim.app.bookmarks.set('a', repo.join("other.md")).unwrap();
        sim.app.bookmarks.set('b', repo.join("sub")).unwrap();
        sim.key('\'');
        drain(&mut sim);
        assert!(sim.app.is_bookmark_list());
        if letter_jump {
            sim.key(if dir { 'b' } else { 'a' });
        } else {
            if dir {
                sim.key('j');
            }
            sim.press(KeyCode::Enter, KeyModifiers::NONE);
        }
        drain(&mut sim);
        assert!(!sim.app.project_active(), "{case}: the project is left");
        assert!(!sim.app.is_bookmark_list(), "{case}");
        if dir {
            assert_eq!(sim.app.tab.root, repo.join("sub"), "{case}: a directory becomes the root");
            assert_eq!(sim.app.tab.mode, Mode::Tree, "{case}");
        } else {
            assert_eq!(sim.app.tab.mode, Mode::Preview, "{case}");
            assert_eq!(
                sim.app.tab.preview_path.as_deref(),
                Some(repo.join("other.md").as_path()),
                "{case}"
            );
        }
    }
}

#[test]
fn vcs_overlay_tab_list_keeps_its_keys_over_a_project_screen() {
    if isolated("vcs_overlay_tab_list_keeps_its_keys_over_a_project_screen") {
        return;
    }
    let Some(repo) = overlay_repo("vcs_overlay_tabs") else {
        return;
    };
    let cfg = crate::test_support::unique_tmp("vcs_overlay_tabs_cfg");
    let scenario = |project: bool| -> Vec<(String, Outcome)> {
        let mut sim = preview_session(&repo, config("git", "en"), "notes.md", project, &cfg);
        // A second tab, then back to the first (the one with the project screen).
        sim.key('t');
        drain(&mut sim);
        sim.key('[');
        drain(&mut sim);
        assert_eq!(sim.app.project_active(), project, "the first tab again");
        let mut log = Vec::new();
        let mut step = |label: &str, sim: &Sim| log.push((label.to_string(), outcome(sim)));
        sim.key('T');
        step("open", &sim);
        sim.key('/');
        step("slash", &sim);
        sim.press(KeyCode::Esc, KeyModifiers::NONE);
        sim.press(KeyCode::Esc, KeyModifiers::NONE);
        step("esc esc", &sim);
        sim.key('T');
        sim.key('q');
        step("q", &sim);
        sim.key('T');
        sim.key('j');
        sim.key('d');
        step("d closes the other tab", &sim);
        sim.press(KeyCode::Enter, KeyModifiers::NONE);
        drain(&mut sim);
        step("enter activates", &sim);
        log
    };
    let with = scenario(true);
    let without = scenario(false);
    for (a, b) in with.iter().zip(&without) {
        assert_eq!(a, b, "step {:?} differs with a project screen behind", a.0);
    }
    assert!(with[0].1.tab_list);
    assert!(with[1].1.list_filter, "/ filters the tab list: {:?}", with[1].1);
    assert!(!with[2].1.tab_list, "{:?}", with[2].1);
    assert!(!with[3].1.tab_list, "q closes: {:?}", with[3].1);
    assert_eq!(with[4].1.tabs, 1, "d closes a tab: {:?}", with[4].1);
    assert!(!with[5].1.tab_list, "Enter activates and closes: {:?}", with[5].1);
}

#[test]
fn vcs_overlay_help_and_info_close_over_a_project_screen() {
    if isolated("vcs_overlay_help_and_info_close_over_a_project_screen") {
        return;
    }
    let Some(repo) = overlay_repo("vcs_overlay_help") else {
        return;
    };
    let cfg = crate::test_support::unique_tmp("vcs_overlay_help_cfg");
    for closer in [KeyCode::Esc, KeyCode::Char('q'), KeyCode::Char('?')] {
        let mut sim = preview_session(&repo, config("git", "en"), "notes.md", true, &cfg);
        sim.key('?');
        drain(&mut sim);
        assert!(sim.app.show_help, "? opens the help over the project");
        sim.press(closer, KeyModifiers::NONE);
        drain(&mut sim);
        assert!(!sim.app.show_help, "{closer:?} closes the help");
        assert!(sim.app.project_active(), "the project is still open");
    }
    for closer in [KeyCode::Esc, KeyCode::Char('q'), KeyCode::Char('i')] {
        let mut sim = preview_session(&repo, config("git", "en"), "notes.md", true, &cfg);
        sim.app.toggle_info();
        drain(&mut sim);
        assert!(sim.app.is_info(), "Info is open in front of the project");
        sim.press(closer, KeyModifiers::NONE);
        drain(&mut sim);
        assert!(!sim.app.is_info(), "{closer:?} closes Info");
        assert!(sim.app.project_active(), "the project is still open");
    }
}

#[test]
fn vcs_overlay_footer_and_help_rows_are_the_same_with_and_without_a_project_behind() {
    if isolated("vcs_overlay_footer_and_help_rows_are_the_same_with_and_without_a_project_behind") {
        return;
    }
    let Some(repo) = overlay_repo("vcs_overlay_rows") else {
        return;
    };
    let cfg = crate::test_support::unique_tmp("vcs_overlay_rows_cfg");
    let open_overlay = |project: bool, ctx: Context| -> Sim {
        let mut sim = preview_session(&repo, config("git", "en"), "notes.md", project, &cfg);
        sim.app.bookmarks.set('a', repo.join("other.md")).unwrap();
        match ctx {
            Context::Bookmarks => sim.key('\''),
            Context::Tabs => sim.key('T'),
            Context::Info => sim.app.toggle_info(),
            Context::Help => sim.key('?'),
            _ => unreachable!(),
        }
        drain(&mut sim);
        sim
    };
    for ctx in [Context::Bookmarks, Context::Tabs, Context::Info, Context::Help] {
        let with = open_overlay(true, ctx);
        let without = open_overlay(false, ctx);
        assert_eq!(with.app.key_context(), Some(ctx), "{ctx:?}: the overlay is in front");
        assert_eq!(without.app.key_context(), Some(ctx));
        let a = crate::ops::footer::tokens(&with.app, ctx);
        let b = crate::ops::footer::tokens(&without.app, ctx);
        assert_eq!(a, b, "{ctx:?}: footer hints differ with a project screen behind");
        // The `?` rows the overlay's own table gives: every row the overlay lists without a
        // project is listed with one (what differs is only what the project itself changes).
        for action in crate::ops::keys::fixed_bindings(ctx).iter().map(|f| f.action) {
            assert_eq!(
                crate::ops::gate(&with.app, ctx, action).is_on(),
                crate::ops::gate(&without.app, ctx, action).is_on(),
                "{ctx:?}: {action:?} acts differently with a project screen behind"
            );
        }
    }
}

#[test]
fn vcs_overlay_a_key_bound_to_a_preview_action_in_an_overlay_does_not_reach_the_preview_under() {
    if isolated("vcs_overlay_a_key_bound_to_a_preview_action_in_an_overlay_does_not_reach_the_preview_under") {
        return;
    }
    let Some(repo) = overlay_repo("vcs_overlay_bound") else {
        return;
    };
    let cfg_base = crate::test_support::unique_tmp("vcs_overlay_bound_cfg");
    for (table, ctx) in [
        ("help", Context::Help),
        ("bookmarks", Context::Bookmarks),
        ("tabs", Context::Tabs),
        ("info", Context::Info),
    ] {
        for action in ["preview_copy_selection", "toggle_outline", "mark_set", "toggle_wrap"] {
            let mut cfg = config("git", "en");
            bind(&mut cfg, table, "ctrl-a", action);
            let mut sim = preview_session(&repo, cfg, "notes.md", true, &cfg_base);
            match ctx {
                Context::Help => sim.key('?'),
                Context::Bookmarks => sim.key('\''),
                Context::Tabs => sim.key('T'),
                Context::Info => sim.app.toggle_info(),
                _ => unreachable!(),
            }
            drain(&mut sim);
            assert_eq!(sim.app.key_context(), Some(ctx));
            let bound = crate::keymap::action_from_str(action).unwrap();
            assert_eq!(
                sim.app.keymaps.resolve_on(ctx, None, ctrl_a()),
                crate::keymap::Resolution::Action(bound),
                "{table}: the binding is live"
            );
            assert!(
                !crate::ops::gate(&sim.app, ctx, bound).is_on(),
                "{table}/{action} is on in an overlay over a project screen"
            );
            crate::test_support::clear_test_clipboard();
            let before = (under_state(&sim), outcome(&sim));
            sim.app.flash = None;
            sim.press(KeyCode::Char('a'), KeyModifiers::CONTROL);
            drain(&mut sim);
            assert_eq!(crate::test_support::get_test_clipboard(), None, "{table}/{action}: copied");
            assert_eq!((under_state(&sim), outcome(&sim)), before, "{table}/{action}");
            assert!(sim.app.flash.is_none(), "{table}/{action}: flash {:?}", sim.app.flash);
        }
    }
    // The positive control: without a project screen the same actions are on in those contexts.
    let sim = preview_session(&repo, config("git", "en"), "notes.md", false, &cfg_base);
    assert!(crate::ops::gate(&sim.app, Context::Help, crate::keymap::Action::PreviewCopySelection).is_on());
}

#[test]
fn vcs_project_a_remapped_preview_back_leaves_the_project_like_its_own_back_key() {
    if isolated("vcs_project_a_remapped_preview_back_leaves_the_project_like_its_own_back_key") {
        return;
    }
    let Some(repo) = overlay_repo("vcs_project_back") else {
        return;
    };
    let cfg_base = crate::test_support::unique_tmp("vcs_project_back_cfg");
    let run = |key: (KeyCode, KeyModifiers)| {
        let mut cfg = config("git", "en");
        bind(&mut cfg, "canvas", "ctrl-a", "preview_back");
        let mut sim = preview_session(&repo, cfg, "notes.md", true, &cfg_base);
        sim.press(key.0, key.1);
        drain(&mut sim);
        (sim.app.project_active(), sim.app.tab.mode, sim.app.tab.preview_path.clone())
    };
    let own = run((KeyCode::Char('q'), KeyModifiers::NONE));
    let remapped = run((KeyCode::Char('a'), KeyModifiers::CONTROL));
    assert_eq!(remapped, own, "the remapped back key and the project's own back key differ");
    assert!(!own.0, "leaving the project");
    assert_eq!(own.1, Mode::Preview, "back on the preview the project overlaid");
    assert_eq!(own.2.as_deref(), Some(repo.join("notes.md").as_path()));
}

#[test]
fn vcs_project_paste_jump_leaves_the_project_and_shows_the_target() {
    if isolated("vcs_project_paste_jump_leaves_the_project_and_shows_the_target") {
        return;
    }
    let Some(repo) = overlay_repo("vcs_project_paste") else {
        return;
    };
    let cfg_base = crate::test_support::unique_tmp("vcs_project_paste_cfg");
    let outside = crate::test_support::unique_tmp("vcs_project_paste_outside");
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("far.txt"), "far\n").unwrap();
    let outside_file = outside.join("far.txt").canonicalize().unwrap();
    // Same root, then a path outside the root.
    for (target, same_root) in [(repo.join("other.md"), true), (outside_file, false)] {
        let mut sim = preview_session(&repo, config("git", "en"), "notes.md", true, &cfg_base);
        crate::test_support::set_test_clipboard(&target.to_string_lossy());
        sim.key('P');
        drain(&mut sim);
        assert!(!sim.app.project_active(), "same_root={same_root}: the project is left");
        assert_eq!(sim.app.tab.mode, Mode::Preview, "same_root={same_root}: the target is shown");
        assert_eq!(sim.app.tab.preview_path.as_deref(), Some(target.as_path()), "same_root={same_root}");
        if !same_root {
            assert_ne!(sim.app.tab.root, repo, "the root moved");
        }
    }
    // A clipboard that is not a path leaves the project where it is.
    let mut sim = preview_session(&repo, config("git", "en"), "notes.md", true, &cfg_base);
    crate::test_support::set_test_clipboard("/no/such/path/anywhere");
    sim.key('P');
    drain(&mut sim);
    assert!(sim.app.project_active(), "a failed jump does not leave the project");
}

#[test]
fn vcs_project_e_opens_the_project_row_at_no_line_of_the_preview_under() {
    if isolated("vcs_project_e_opens_the_project_row_at_no_line_of_the_preview_under") {
        return;
    }
    let Some(repo) = overlay_repo("vcs_project_edit") else {
        return;
    };
    let cfg_base = crate::test_support::unique_tmp("vcs_project_edit_cfg");
    let mut sim = Sim::with_config_sized(&repo, config("git", "en"), 140, 30);
    sim.app.bookmarks = crate::bookmarks::Bookmarks::with_base(cfg_base.to_path_buf(), &repo);
    sim.app.tab.projects.wait_for_result();
    sim.app.poll_projects();
    drain(&mut sim);
    sim.select("lines.txt");
    sim.enter();
    drain(&mut sim);
    for _ in 0..5 {
        sim.key('j');
    }
    let line = sim.app.preview_edit_line_for_test();
    assert!(line.is_some_and(|l| l > 1), "the caret is down the file: {line:?}");
    sim.key('p');
    drain(&mut sim);
    assert!(sim.app.project_active());
    let target = match sim.app.verb_target() {
        crate::app::open_app::VerbTarget::File(p) => p,
        other => panic!("the project row is not a file: {other:?}"),
    };
    assert_ne!(target, repo.join("lines.txt"), "the row is not the hidden preview's file");
    sim.key('e');
    drain(&mut sim);
    assert_eq!(
        sim.app.take_pending_edit(),
        Some((target, None)),
        "the editor gets the project row's file with no line of the preview under"
    );
    assert_eq!(sim.app.preview_edit_line_for_test(), None, "nothing of the hidden preview is read");
}

#[test]
fn vcs_project_a_refresh_from_an_overlay_is_the_projects_refresh() {
    if isolated("vcs_project_a_refresh_from_an_overlay_is_the_projects_refresh") {
        return;
    }
    let Some(repo) = overlay_repo("vcs_project_refresh") else {
        return;
    };
    let cfg_base = crate::test_support::unique_tmp("vcs_project_refresh_cfg");
    let mut cfg = config("git", "en");
    bind(&mut cfg, "help", "ctrl-a", "refresh");
    bind(&mut cfg, "bookmarks", "ctrl-a", "refresh");
    for open in ['r', '?', '\''] {
        let mut sim = preview_session(&repo, cfg.clone(), "notes.md", true, &cfg_base);
        // `r` is the control: the project's own refresh key, which the overlay keys must equal.
        let own = open == 'r';
        if !own {
            sim.key(open);
            drain(&mut sim);
            let ctx = sim.app.key_context().unwrap();
            assert!(matches!(ctx, Context::Help | Context::Bookmarks), "{ctx:?}");
            assert_eq!(
                sim.app.keymaps.resolve_on(ctx, None, ctrl_a()),
                crate::keymap::Resolution::Action(crate::keymap::Action::Refresh)
            );
        }
        let entries_before = sim.app.tab.entries.len();
        // A file that appears after the project read the repository: the tree would list it
        // after a tree refresh, the project's changes list after the project's own.
        std::fs::write(repo.join(format!("late-{}.txt", open as u32)), "late\n").unwrap();
        sim.app.flash = None;
        if own {
            sim.key('r');
        } else {
            sim.press(KeyCode::Char('a'), KeyModifiers::CONTROL);
        }
        drain(&mut sim);
        assert_eq!(
            sim.app.tab.entries.len(),
            entries_before,
            "{open}: the tree under the project screen was rebuilt"
        );
        assert!(sim.app.project_active(), "{open}");
        let late = format!("late-{}.txt", open as u32);
        let rows_of = |sim: &Sim| match sim.app.project_content().as_deref() {
            Some(ProjectContent::List(list)) => list.rows.iter().map(|r| r.value.clone()).collect::<Vec<_>>(),
            _ => vec![],
        };
        let mut rows = rows_of(&sim);
        for _ in 0..40 {
            if rows.iter().any(|v| v.ends_with(&late)) {
                break;
            }
            drain(&mut sim);
            rows = rows_of(&sim);
        }
        assert!(
            rows.iter().any(|v| v.ends_with(&late)),
            "{open}: the project did not refresh: {rows:?}"
        );
    }
}

/// The project in front reads modifiers exactly (Alt is part of its keys), so an Alt chord is a
/// key of no host table while it is in front: it neither goes back (`Alt-q`, `Alt-a` with the back
/// key remapped to `ctrl-a`) nor refreshes (`Alt-r`); the plain and remapped keys still do.
#[test]
fn vcs_project_an_alt_chord_is_a_key_of_no_host_table_while_the_project_is_in_front() {
    if isolated("vcs_project_an_alt_chord_is_a_key_of_no_host_table_while_the_project_is_in_front") {
        return;
    }
    let Some(repo) = overlay_repo("vcs_project_alt") else {
        return;
    };
    let cfg_base = crate::test_support::unique_tmp("vcs_project_alt_cfg");
    let mut cfg = config("git", "en");
    bind(&mut cfg, "canvas", "ctrl-a", "preview_back");
    let mut sim = preview_session(&repo, cfg.clone(), "notes.md", true, &cfg_base);
    let rows_of = |sim: &Sim| match sim.app.project_content().as_deref() {
        Some(ProjectContent::List(list)) => list.rows.iter().map(|r| r.value.clone()).collect::<Vec<_>>(),
        _ => vec![],
    };
    std::fs::write(repo.join("late-alt.txt"), "late\n").unwrap();
    for (code, name) in [('q', "Alt-q"), ('r', "Alt-r"), ('a', "Alt-a")] {
        sim.press(KeyCode::Char(code), KeyModifiers::ALT);
        for _ in 0..40 {
            drain(&mut sim);
        }
        assert!(sim.app.project_active(), "{name} left the project");
        assert!(
            !rows_of(&sim).iter().any(|v| v.ends_with("late-alt.txt")),
            "{name} refreshed the project"
        );
    }
    // Plain `r` still refreshes.
    sim.key('r');
    let mut rows = rows_of(&sim);
    for _ in 0..40 {
        if rows.iter().any(|v| v.ends_with("late-alt.txt")) {
            break;
        }
        drain(&mut sim);
        rows = rows_of(&sim);
    }
    assert!(rows.iter().any(|v| v.ends_with("late-alt.txt")), "plain r did not refresh: {rows:?}");
    // The remapped back key still leaves.
    sim.press(KeyCode::Char('a'), KeyModifiers::CONTROL);
    drain(&mut sim);
    assert!(!sim.app.project_active(), "Ctrl-a (preview_back) did not leave");
    // Without a project, Alt is dropped as before: Alt-q on the preview still goes back to the tree.
    let mut sim = preview_session(&repo, cfg, "notes.md", false, &cfg_base);
    sim.press(KeyCode::Char('q'), KeyModifiers::ALT);
    drain(&mut sim);
    assert_eq!(sim.app.tab.mode, Mode::Tree, "the host without a project reads Alt-q as q");
}

// --- The all-actions walk ---------------------------------------------------------------------

/// The variant of an action, by name. The `match` has no wildcard arm: a new variant of `Action`
/// does not compile until it gets an arm here (and a name in [`ACTION_VARIANT_NAMES`]), which is
/// where it is classified. Variants behind `feature = "git"` have arms behind it.
fn variant_of(a: crate::keymap::Action) -> String {
    use crate::keymap::Action;
    let name: &'static str = match a {
        Action::Noop => "Noop",
        Action::Navigate(..) => "Navigate",
        Action::TabNew => "TabNew",
        Action::TabNewAfter => "TabNewAfter",
        Action::TabClose => "TabClose",
        Action::TabPrev => "TabPrev",
        Action::TabNext => "TabNext",
        Action::TabGoto(..) => "TabGoto",
        Action::ToggleHelp => "ToggleHelp",
        Action::CopyPath(..) => "CopyPath",
        Action::CopyCodeBlock => "CopyCodeBlock",
        Action::PasteJump => "PasteJump",
        Action::Quit => "Quit",
        Action::CloseTabOrQuit => "CloseTabOrQuit",
        Action::FilterStart => "FilterStart",
        Action::TreeDescend => "TreeDescend",
        Action::TreeActivate => "TreeActivate",
        Action::TreeLeave => "TreeLeave",
        Action::ToggleHidden => "ToggleHidden",
        Action::ToggleWrap => "ToggleWrap",
        Action::ToggleInfo => "ToggleInfo",
        Action::RequestEdit => "RequestEdit",
        Action::OpenExternal => "OpenExternal",
        Action::OpenGitView => "OpenGitView",
        Action::Refresh => "Refresh",
        Action::CyclePathStyle => "CyclePathStyle",
        Action::OpenSortMenu => "OpenSortMenu",
        Action::MarkSet => "MarkSet",
        Action::MarkJump => "MarkJump",
        Action::SetAnchor => "SetAnchor",
        Action::ResetAnchor => "ResetAnchor",
        Action::OpenGitDiffCursor => "OpenGitDiffCursor",
        Action::EnterVisual => "EnterVisual",
        Action::ToggleSelect => "ToggleSelect",
        Action::ToggleChangedFilter => "ToggleChangedFilter",
        Action::JumpNextChange => "JumpNextChange",
        Action::JumpPrevChange => "JumpPrevChange",
        Action::ToggleFollow => "ToggleFollow",
        Action::FileCreate => "FileCreate",
        Action::FileRename => "FileRename",
        Action::FileDelete => "FileDelete",
        Action::FileCopy => "FileCopy",
        Action::FileCut => "FileCut",
        Action::FilePaste => "FilePaste",
        Action::FileDuplicate => "FileDuplicate",
        Action::VisualCommit => "VisualCommit",
        Action::VisualSelectSiblings => "VisualSelectSiblings",
        Action::VisualSelectAll => "VisualSelectAll",
        Action::PreviewBack => "PreviewBack",
        Action::SearchStart => "SearchStart",
        Action::SearchNext => "SearchNext",
        Action::SearchPrev => "SearchPrev",
        Action::PreviewEnterVisual => "PreviewEnterVisual",
        Action::PreviewEnterVisualLine => "PreviewEnterVisualLine",
        Action::PreviewCopySelection => "PreviewCopySelection",
        Action::PreviewCopySelectionRef => "PreviewCopySelectionRef",
        Action::PreviewExitVisual => "PreviewExitVisual",
        Action::CycleRepresentation => "CycleRepresentation",
        Action::ImageReturnToDiff => "ImageReturnToDiff",
        Action::LinkFocusNext => "LinkFocusNext",
        Action::LinkFocusPrev => "LinkFocusPrev",
        Action::LinkOpen => "LinkOpen",
        Action::OpenLinkNewTab => "OpenLinkNewTab",
        Action::OpenInNewTab => "OpenInNewTab",
        Action::ImageZoomIn => "ImageZoomIn",
        Action::ImageZoomOut => "ImageZoomOut",
        Action::ImageZoomReset => "ImageZoomReset",
        Action::PageNext => "PageNext",
        Action::PagePrev => "PagePrev",
        Action::PreviewFileNext => "PreviewFileNext",
        Action::PreviewFilePrev => "PreviewFilePrev",
        Action::TableCopy(..) => "TableCopy",
        #[cfg(feature = "git")]
        Action::GitDiffDiscard => "GitDiffDiscard",
        Action::CycleDiffLayout => "CycleDiffLayout",
        #[cfg(feature = "git")]
        Action::CycleDiffView => "CycleDiffView",
        #[cfg(feature = "git")]
        Action::ToggleFollowDiffScope => "ToggleFollowDiffScope",
        #[cfg(feature = "git")]
        Action::DiffNextChange => "DiffNextChange",
        #[cfg(feature = "git")]
        Action::DiffPrevChange => "DiffPrevChange",
        #[cfg(feature = "git")]
        Action::GitStage => "GitStage",
        #[cfg(feature = "git")]
        Action::GitUnstage => "GitUnstage",
        #[cfg(feature = "git")]
        Action::GitStageAll => "GitStageAll",
        #[cfg(feature = "git")]
        Action::GitUnstageAll => "GitUnstageAll",
        #[cfg(feature = "git")]
        Action::GitDiscard => "GitDiscard",
        #[cfg(feature = "git")]
        Action::GitCommit => "GitCommit",
        #[cfg(feature = "git")]
        Action::GitWorktreeDiff => "GitWorktreeDiff",
        #[cfg(feature = "git")]
        Action::GitOpenLog => "GitOpenLog",
        #[cfg(feature = "git")]
        Action::GitOpenGraph => "GitOpenGraph",
        #[cfg(feature = "git")]
        Action::GitOpenBranches => "GitOpenBranches",
        #[cfg(feature = "git")]
        Action::GitLaunchTool => "GitLaunchTool",
        #[cfg(feature = "git")]
        Action::GitOpenSelectedDiff => "GitOpenSelectedDiff",
        #[cfg(feature = "git")]
        Action::GitOpenDetail => "GitOpenDetail",
        #[cfg(feature = "git")]
        Action::GitGraphSetBase => "GitGraphSetBase",
        #[cfg(feature = "git")]
        Action::GitGraphClearBase => "GitGraphClearBase",
        #[cfg(feature = "git")]
        Action::GitGraphToggleAll => "GitGraphToggleAll",
        #[cfg(feature = "git")]
        Action::JjSync => "JjSync",
        #[cfg(feature = "git")]
        Action::GitGraphOpenPicker => "GitGraphOpenPicker",
        #[cfg(feature = "git")]
        Action::GitGraphPickerToggle => "GitGraphPickerToggle",
        #[cfg(feature = "git")]
        Action::GitGraphPickerAll => "GitGraphPickerAll",
        #[cfg(feature = "git")]
        Action::GitGraphPickerCurrentOnly => "GitGraphPickerCurrentOnly",
        #[cfg(feature = "git")]
        Action::GitGraphPickerMoveUp => "GitGraphPickerMoveUp",
        #[cfg(feature = "git")]
        Action::GitGraphPickerMoveDown => "GitGraphPickerMoveDown",
        #[cfg(feature = "git")]
        Action::GitScreenNext => "GitScreenNext",
        #[cfg(feature = "git")]
        Action::GitScreenPrev => "GitScreenPrev",
        Action::ListFilterStart => "ListFilterStart",
        #[cfg(feature = "git")]
        Action::BranchFilterStart => "BranchFilterStart",
        #[cfg(feature = "git")]
        Action::BranchCheckout => "BranchCheckout",
        #[cfg(feature = "git")]
        Action::BranchCreate => "BranchCreate",
        #[cfg(feature = "git")]
        Action::BranchDelete => "BranchDelete",
        #[cfg(feature = "git")]
        Action::GitOpenWorktrees => "GitOpenWorktrees",
        #[cfg(feature = "git")]
        Action::WorktreeFilterStart => "WorktreeFilterStart",
        #[cfg(feature = "git")]
        Action::WorktreeGoto => "WorktreeGoto",
        #[cfg(feature = "git")]
        Action::WorktreeGotoNewTab => "WorktreeGotoNewTab",
        #[cfg(feature = "git")]
        Action::WorktreeClose => "WorktreeClose",
        #[cfg(feature = "git")]
        Action::WorktreeShowChanges => "WorktreeShowChanges",
        #[cfg(feature = "git")]
        Action::WorktreeCreate => "WorktreeCreate",
        #[cfg(feature = "git")]
        Action::GitCopy(..) => "GitCopy",
        #[cfg(feature = "git")]
        Action::CopyBranchName => "CopyBranchName",
        #[cfg(feature = "git")]
        Action::GitClose => "GitClose",
        Action::SortSet(..) => "SortSet",
        Action::SortToggleReverse => "SortToggleReverse",
        Action::SortToggleDirsFirst => "SortToggleDirsFirst",
        Action::ToggleTabList => "ToggleTabList",
        Action::TabListClose => "TabListClose",
        Action::ToggleOutline => "ToggleOutline",
        Action::BookmarkJump => "BookmarkJump",
        Action::BookmarkEdit => "BookmarkEdit",
        Action::BookmarkDelete => "BookmarkDelete",
        Action::BookmarkClose => "BookmarkClose",
        Action::InfoClose => "InfoClose",
        Action::ToggleTableCell => "ToggleTableCell",
        Action::Cancel => "Cancel",
        Action::TabListActivate => "TabListActivate",
        Action::OutlineJump => "OutlineJump",
        #[cfg(feature = "git")]
        Action::GitGraphPickerApply => "GitGraphPickerApply",
        Action::ToggleFocusedItem => "ToggleFocusedItem",
        Action::Plugin(..) => "Plugin",
        Action::ProjectControl(..) => "ProjectControl",
    };
    name.to_string()
}

/// Every variant name of `Action`; the walk must cover exactly these.
const ACTION_VARIANT_NAMES: &[&str] = &[
    "Noop",
    "Navigate",
    "TabNew",
    "TabNewAfter",
    "TabClose",
    "TabPrev",
    "TabNext",
    "TabGoto",
    "ToggleHelp",
    "CopyPath",
    "CopyCodeBlock",
    "PasteJump",
    "Quit",
    "CloseTabOrQuit",
    "FilterStart",
    "TreeDescend",
    "TreeActivate",
    "TreeLeave",
    "ToggleHidden",
    "ToggleWrap",
    "ToggleInfo",
    "RequestEdit",
    "OpenExternal",
    "OpenGitView",
    "Refresh",
    "CyclePathStyle",
    "OpenSortMenu",
    "MarkSet",
    "MarkJump",
    "SetAnchor",
    "ResetAnchor",
    "OpenGitDiffCursor",
    "EnterVisual",
    "ToggleSelect",
    "ToggleChangedFilter",
    "JumpNextChange",
    "JumpPrevChange",
    "ToggleFollow",
    "FileCreate",
    "FileRename",
    "FileDelete",
    "FileCopy",
    "FileCut",
    "FilePaste",
    "FileDuplicate",
    "VisualCommit",
    "VisualSelectSiblings",
    "VisualSelectAll",
    "PreviewBack",
    "SearchStart",
    "SearchNext",
    "SearchPrev",
    "PreviewEnterVisual",
    "PreviewEnterVisualLine",
    "PreviewCopySelection",
    "PreviewCopySelectionRef",
    "PreviewExitVisual",
    "CycleRepresentation",
    "ImageReturnToDiff",
    "LinkFocusNext",
    "LinkFocusPrev",
    "LinkOpen",
    "OpenLinkNewTab",
    "OpenInNewTab",
    "ImageZoomIn",
    "ImageZoomOut",
    "ImageZoomReset",
    "PageNext",
    "PagePrev",
    "PreviewFileNext",
    "PreviewFilePrev",
    "TableCopy",
    #[cfg(feature = "git")]
    "GitDiffDiscard",
    "CycleDiffLayout",
    #[cfg(feature = "git")]
    "CycleDiffView",
    #[cfg(feature = "git")]
    "ToggleFollowDiffScope",
    #[cfg(feature = "git")]
    "DiffNextChange",
    #[cfg(feature = "git")]
    "DiffPrevChange",
    #[cfg(feature = "git")]
    "GitStage",
    #[cfg(feature = "git")]
    "GitUnstage",
    #[cfg(feature = "git")]
    "GitStageAll",
    #[cfg(feature = "git")]
    "GitUnstageAll",
    #[cfg(feature = "git")]
    "GitDiscard",
    #[cfg(feature = "git")]
    "GitCommit",
    #[cfg(feature = "git")]
    "GitWorktreeDiff",
    #[cfg(feature = "git")]
    "GitOpenLog",
    #[cfg(feature = "git")]
    "GitOpenGraph",
    #[cfg(feature = "git")]
    "GitOpenBranches",
    #[cfg(feature = "git")]
    "GitLaunchTool",
    #[cfg(feature = "git")]
    "GitOpenSelectedDiff",
    #[cfg(feature = "git")]
    "GitOpenDetail",
    #[cfg(feature = "git")]
    "GitGraphSetBase",
    #[cfg(feature = "git")]
    "GitGraphClearBase",
    #[cfg(feature = "git")]
    "GitGraphToggleAll",
    #[cfg(feature = "git")]
    "JjSync",
    #[cfg(feature = "git")]
    "GitGraphOpenPicker",
    #[cfg(feature = "git")]
    "GitGraphPickerToggle",
    #[cfg(feature = "git")]
    "GitGraphPickerAll",
    #[cfg(feature = "git")]
    "GitGraphPickerCurrentOnly",
    #[cfg(feature = "git")]
    "GitGraphPickerMoveUp",
    #[cfg(feature = "git")]
    "GitGraphPickerMoveDown",
    #[cfg(feature = "git")]
    "GitScreenNext",
    #[cfg(feature = "git")]
    "GitScreenPrev",
    "ListFilterStart",
    #[cfg(feature = "git")]
    "BranchFilterStart",
    #[cfg(feature = "git")]
    "BranchCheckout",
    #[cfg(feature = "git")]
    "BranchCreate",
    #[cfg(feature = "git")]
    "BranchDelete",
    #[cfg(feature = "git")]
    "GitOpenWorktrees",
    #[cfg(feature = "git")]
    "WorktreeFilterStart",
    #[cfg(feature = "git")]
    "WorktreeGoto",
    #[cfg(feature = "git")]
    "WorktreeGotoNewTab",
    #[cfg(feature = "git")]
    "WorktreeClose",
    #[cfg(feature = "git")]
    "WorktreeShowChanges",
    #[cfg(feature = "git")]
    "WorktreeCreate",
    #[cfg(feature = "git")]
    "GitCopy",
    #[cfg(feature = "git")]
    "CopyBranchName",
    #[cfg(feature = "git")]
    "GitClose",
    "SortSet",
    "SortToggleReverse",
    "SortToggleDirsFirst",
    "ToggleTabList",
    "TabListClose",
    "ToggleOutline",
    "BookmarkJump",
    "BookmarkEdit",
    "BookmarkDelete",
    "BookmarkClose",
    "InfoClose",
    "ToggleTableCell",
    "Cancel",
    "TabListActivate",
    "OutlineJump",
    #[cfg(feature = "git")]
    "GitGraphPickerApply",
    "ToggleFocusedItem",
    "Plugin",
    "ProjectControl",
];

fn action_variants_in_source() -> Vec<String> {
    ACTION_VARIANT_NAMES.iter().map(|n| n.to_string()).collect()
}

/// Every action value the program can hold: the names the config parser accepts, what the
/// default keymaps bind (the actions with no config name among them), the items of the project
/// canvas's `Space` menu, and every action the registered plugins declare.
fn every_action_value(app: &App) -> Vec<crate::keymap::Action> {
    use crate::keymap::{Action, Binding, ProjectMenuControl};
    let mut out: Vec<Action> = every_assignable_action().into_iter().map(|(_, a)| a).collect();
    let km = &app.keymaps;
    for m in km.contexts.values().chain(km.types.values()).chain([&km.basic, &km.global]) {
        for b in m.values() {
            if let Binding::Run(a) = b {
                out.push(*a);
            }
        }
    }
    if let Some(menu) = crate::ops::menu::menu(
        app,
        Context::Canvas,
        crate::keymap::LeaderId::Actions,
        crate::ops::menu::View::Help,
    ) {
        out.extend(menu.items.iter().map(|i| i.action));
    }
    for control in [
        ProjectMenuControl::Search,
        ProjectMenuControl::NextScreen,
        ProjectMenuControl::PrevScreen,
        ProjectMenuControl::Back,
    ] {
        out.push(Action::ProjectControl(control));
    }
    let registry = crate::plugins::shared_registry(&app.cfg.plugin_disabled());
    for id in registry.ids() {
        if let Some(manifest) = registry.manifest(id) {
            for act in &manifest.actions {
                out.push(Action::Plugin(crate::plugins::PluginAct::new(id, &act.id)));
            }
        }
    }
    let mut seen = Vec::new();
    out.retain(|a| {
        let fresh = !seen.contains(a);
        if fresh {
            seen.push(*a);
        }
        fresh
    });
    out
}

/// The walk covers every variant of `Action`, exactly: a variant with no value here, or a value
/// of a variant the enum does not have, fails until it is classified.
fn assert_the_walk_covers_every_variant(values: &[crate::keymap::Action]) {
    let mut walked: Vec<String> = values.iter().map(|a| variant_of(*a)).collect();
    walked.sort();
    walked.dedup();
    let mut declared = action_variants_in_source();
    declared.sort();
    declared.dedup();
    let missing: Vec<_> = declared.iter().filter(|v| !walked.contains(v)).collect();
    let extra: Vec<_> = walked.iter().filter(|v| !declared.contains(v)).collect();
    assert!(
        missing.is_empty() && extra.is_empty(),
        "the walk and the enum differ: no value for {missing:?}, not in the enum {extra:?}"
    );
    assert_eq!(walked, declared);
}

/// What may be on while a project screen covers the screen under it, by variant.
const ON_OVER_THE_PROJECT: &[&str] = &[
    "Noop", "Navigate", "TabNew", "TabNewAfter", "TabClose", "CloseTabOrQuit", "TabPrev", "TabNext",
    "TabGoto", "OpenInNewTab", "ToggleTabList", "ToggleHelp", "ToggleFollow", "PasteJump", "Quit",
    "Refresh", "PreviewBack", "OpenGitView", "RequestEdit", "OpenExternal", "ProjectControl",
    "WorktreeClose", "MarkJump", "GitLaunchTool", "GitOpenLog", "GitOpenGraph", "GitOpenBranches",
    "GitOpenWorktrees", "GitWorktreeDiff", "GitCommit", "GitStageAll", "GitUnstageAll", "JjSync",
    "GitGraphToggleAll", "BranchCreate", "WorktreeCreate", "Plugin",
];

/// The application-level part: what an overlay in front of the project screen keeps besides its own.
const APP_LEVEL: &[&str] = &[
    "Noop", "Navigate", "TabNew", "TabNewAfter", "TabClose", "CloseTabOrQuit", "TabPrev", "TabNext",
    "TabGoto", "ToggleTabList", "ToggleHelp", "ToggleFollow", "PasteJump", "Quit", "Refresh",
];

#[test]
fn vcs_keys_the_walk_over_every_action_and_every_context_of_a_project_screen() {
    if isolated("vcs_keys_the_walk_over_every_action_and_every_context_of_a_project_screen") {
        return;
    }
    let Some(repo) = overlay_repo("vcs_keys_walk_all") else {
        return;
    };
    let cfg = crate::test_support::unique_tmp("vcs_keys_walk_all_cfg");
    let declared = action_variants_in_source();
    // The two hand lists name real variants (a typo or a removed variant fails here).
    for name in ON_OVER_THE_PROJECT.iter().chain(APP_LEVEL) {
        assert!(
            declared.iter().any(|d| d == name) || !cfg!(feature = "git"),
            "{name} is not a variant of Action"
        );
    }
    for name in APP_LEVEL {
        assert!(ON_OVER_THE_PROJECT.contains(name), "{name}: app-level but not allowed");
    }

    let project = preview_session(&repo, config("git", "en"), "notes.md", true, &cfg);
    let values = every_action_value(&project.app);
    assert_the_walk_covers_every_variant(&values);
    // Plugin and ProjectControl values really are in the walk (not only named in a list).
    assert!(values.iter().any(|a| matches!(a, crate::keymap::Action::ProjectControl(_))));
    let plugin_values: Vec<_> = values
        .iter()
        .filter(|a| matches!(a, crate::keymap::Action::Plugin(_)))
        .collect();
    assert!(!plugin_values.is_empty(), "no plugin action was walked");

    // The project screen's own context: on only where listed; a plugin action only where it
    // declares a project context.
    let mut on = 0;
    for a in &values {
        let is_on = crate::ops::gate(&project.app, Context::Canvas, *a).is_on();
        if is_on {
            on += 1;
            assert!(
                ON_OVER_THE_PROJECT.contains(&variant_of(*a).as_str()),
                "{a:?} is on over a project screen: it can act on the screen under"
            );
        }
        if let crate::keymap::Action::Plugin(p) = a {
            let has_context = project
                .app
                .plugin_keys
                .declaration(*p)
                .is_some_and(|d| d.project_context.is_some());
            assert!(!is_on || has_context, "{a:?} is on without a project context");
        }
    }
    assert!(on >= ON_OVER_THE_PROJECT.len() / 2, "an emptied allow list passes: {on} on");

    // Every overlay in front of the project screen: its own actions and the application-level
    // ones act as they do with no project behind; everything else is off.
    let open_overlay = |with_project: bool, ctx: Context| -> Sim {
        let mut sim = preview_session(&repo, config("git", "en"), "notes.md", with_project, &cfg);
        sim.app.bookmarks.set('a', repo.join("other.md")).unwrap();
        match ctx {
            Context::Bookmarks => sim.key('\''),
            Context::Tabs => sim.key('T'),
            Context::Info => sim.app.toggle_info(),
            Context::Help => sim.key('?'),
            _ => unreachable!(),
        }
        drain(&mut sim);
        assert_eq!(sim.app.key_context(), Some(ctx));
        sim
    };
    for ctx in [Context::Help, Context::Bookmarks, Context::Tabs, Context::Info] {
        let with = open_overlay(true, ctx);
        let without = open_overlay(false, ctx);
        let owned: Vec<crate::keymap::Action> = with
            .app
            .keymaps
            .default_ops
            .get(&ctx)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .chain(crate::ops::keys::fixed_bindings(ctx).iter().map(|f| f.action))
            .chain(crate::ops::menu::unkeyed(ctx))
            .collect();
        let mut kept = 0;
        for a in &values {
            let keeps = owned.contains(a) || APP_LEVEL.contains(&variant_of(*a).as_str());
            let expected = keeps && crate::ops::gate(&without.app, ctx, *a).is_on();
            let got = crate::ops::gate(&with.app, ctx, *a).is_on();
            assert_eq!(got, expected, "{ctx:?}: {a:?} over a project screen (kept by the overlay: {keeps})");
            kept += usize::from(got);
        }
        assert!(kept > 0, "{ctx:?}: the overlay keeps nothing over a project screen");
    }
}

/// The project layer reads a key as it reads it (`project_key_press`): a chord with Alt is a key
/// of none of its tables, so Alt-h / Alt-j / Alt-l / Alt-q do nothing on a project screen, while
/// the plain letters work.
#[test]
fn vcs_project_alt_motion_chords_do_nothing_and_plain_letters_work() {
    if isolated("vcs_project_alt_motion_chords_do_nothing_and_plain_letters_work") {
        return;
    }
    let root = super::vcs_changes::git_repo("vcs_alt_motion_chords", true);
    let mut sim = open(&root, config("git", "en"), (180, 24));
    let screen = sim.app.project_frame().unwrap().key.screen.clone();
    let cursor = sim.app.project_frame().unwrap().cursor;
    for c in ['h', 'j', 'k', 'l', 'q', 'r'] {
        sim.press(KeyCode::Char(c), KeyModifiers::ALT);
        drain(&mut sim);
        assert!(sim.app.project_active(), "Alt-{c} left the project");
        let frame = sim.app.project_frame().unwrap();
        assert_eq!(frame.key.screen, screen, "Alt-{c} changed the screen");
        assert_eq!(frame.cursor, cursor, "Alt-{c} moved the cursor");
    }
    keys(&mut sim, "j");
    assert_eq!(
        sim.app.project_frame().unwrap().cursor,
        cursor + 1,
        "plain j moves"
    );
    keys(&mut sim, "q");
    assert!(!sim.app.project_active(), "plain q leaves");
}

/// Refresh is the project's operation, not the key `r`: with `r` bound to `navigate:left` on the
/// changes screen, the refresh bound in the help (preview -> `p` -> `?` -> the key) refreshes the
/// project and does not leave it.
#[test]
fn vcs_project_refresh_is_an_operation_not_the_key_r_read_through_the_config() {
    if isolated("vcs_project_refresh_is_an_operation_not_the_key_r_read_through_the_config") {
        return;
    }
    let Some(repo) = overlay_repo("vcs_project_refresh_op") else {
        return;
    };
    let cfg_base = crate::test_support::unique_tmp("vcs_project_refresh_op_cfg");
    for table in ["git.changes", "git_changes"] {
        let mut cfg = config("git", "en");
        bind(&mut cfg, table, "r", "navigate:left");
        bind(&mut cfg, "help", "ctrl-a", "refresh");
        let mut sim = preview_session(&repo, cfg, "notes.md", true, &cfg_base);
        let screen = sim.app.project_frame().unwrap().key.screen.clone();
        let depth = sim.app.tab.project_navigation.as_ref().unwrap().stack.len();
        sim.key('?');
        drain(&mut sim);
        let late = format!("late-refresh-{}.txt", table.replace('.', "_"));
        std::fs::write(repo.join(&late), "late\n").unwrap();
        sim.press(KeyCode::Char('a'), KeyModifiers::CONTROL);
        drain(&mut sim);
        assert!(sim.app.project_active(), "{table}: the refresh left the project");
        let nav = sim.app.tab.project_navigation.as_ref().unwrap();
        assert_eq!(nav.stack.len(), depth, "{table}: the refresh moved back a screen");
        assert_eq!(sim.app.project_frame().unwrap().key.screen, screen, "{table}");
        let rows_of = |sim: &Sim| match sim.app.project_content().as_deref() {
            Some(ProjectContent::List(list)) => {
                list.rows.iter().map(|r| r.value.clone()).collect::<Vec<_>>()
            }
            _ => vec![],
        };
        let mut rows = rows_of(&sim);
        for _ in 0..40 {
            if rows.iter().any(|v| v.ends_with(&late)) {
                break;
            }
            drain(&mut sim);
            rows = rows_of(&sim);
        }
        assert!(
            rows.iter().any(|v| v.ends_with(&late)),
            "{table}: the project did not refresh: {rows:?}"
        );
    }
}

fn preview_text(sim: &mut Sim) -> String {
    sim.app
        .windowed_lines(20, 100)
        .iter()
        .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

/// The real return of the editor / a terminal launch / the git tool goes through
/// `App::external_returned`. While a project screen covers the preview under it, the screen in
/// front is the project's and nothing of the preview is drawn; leaving the project shows the
/// preview read again from the file.
#[test]
fn vcs_project_external_return_leaves_the_covered_preview_alone_until_it_is_in_front() {
    if isolated("vcs_project_external_return_leaves_the_covered_preview_alone_until_it_is_in_front") {
        return;
    }
    let Some(repo) = overlay_repo("vcs_project_external_return_preview") else {
        return;
    };
    let cfg_base = crate::test_support::unique_tmp("vcs_project_external_return_preview_cfg");
    for what in [crate::app::ExternalReturn::File, crate::app::ExternalReturn::Tree] {
        std::fs::write(repo.join("lines.txt"), LINES_TXT).unwrap();
        let mut sim = preview_session(&repo, config("git", "en"), "lines.txt", true, &cfg_base);
        let _ = sim.app.project_frame().unwrap();
        std::fs::write(repo.join("lines.txt"), "edited-one\nedited-two\n").unwrap();
        // Nothing of the covered view is read again behind the project screen (the windowed
        // reader, the plugin show, the media load, the preview reload).
        let ((), behind) = crate::test_support::count_tab_view_loads(|| {
            sim.app.external_returned(what);
            drain(&mut sim);
            sim.draw();
        });
        assert_eq!(behind, 0, "{what:?}: the covered view was read again behind the screen");
        assert!(sim.app.project_active(), "{what:?}: the return left the project");
        assert_project_screen_in_front(&sim, &format!("{what:?}: after the return"), &[]);
        assert!(
            !sim.screen().contains("edited-one"),
            "{what:?}: the preview is drawn over the project screen:\n{}",
            sim.screen()
        );
        // The counter sees the read of the return (a lower bound, so the zero above is a count).
        let ((), on_return) = crate::test_support::count_tab_view_loads(|| {
            sim.app.leave_project_screens();
            drain(&mut sim);
        });
        assert!(on_return >= 1, "{what:?}: the return read nothing again");
        assert!(!sim.app.project_active());
        sim.draw();
        assert!(
            sim.screen().contains("edited-one"),
            "{what:?}: stale preview after leaving:\n{}",
            sim.screen()
        );
        let shown = preview_text(&mut sim);
        assert!(shown.contains("edited-one"), "{what:?}: stale preview after leaving: {shown}");
    }
}

/// The same for the tree under a project screen: the tab's tree is not rebuilt behind the project
/// screen in front; a file the git tool made shows in the tree after leaving.
#[test]
fn vcs_project_external_return_leaves_the_covered_tree_alone_until_it_is_in_front() {
    if isolated("vcs_project_external_return_leaves_the_covered_tree_alone_until_it_is_in_front") {
        return;
    }
    let Some(repo) = overlay_repo("vcs_project_external_return_tree") else {
        return;
    };
    let cfg_base = crate::test_support::unique_tmp("vcs_project_external_return_tree_cfg");
    let mut sim = Sim::with_config_sized(&repo, config("git", "en"), 140, 30);
    sim.app.bookmarks = crate::bookmarks::Bookmarks::with_base(cfg_base.to_path_buf(), &repo);
    sim.app.tab.projects.wait_for_result();
    sim.app.poll_projects();
    drain(&mut sim);
    sim.key('p');
    drain(&mut sim);
    assert!(sim.app.project_active());
    let before = sim.app.tab.entries.len();
    std::fs::write(repo.join("made-by-the-tool.txt"), "x\n").unwrap();
    sim.app.external_returned(crate::app::ExternalReturn::Tree);
    drain(&mut sim);
    assert!(sim.app.project_active());
    sim.draw();
    assert_project_screen_in_front(&sim, "after the return", &[]);
    assert_eq!(sim.app.tab.entries.len(), before, "the covered tree was rebuilt");
    sim.app.leave_project_screens();
    drain(&mut sim);
    assert!(
        sim.app
            .tab
            .entries
            .iter()
            .any(|e| e.path.ends_with("made-by-the-tool.txt")),
        "the tree did not show the new file after leaving"
    );
    sim.draw();
    assert!(
        sim.screen().contains("made-by-the-tool.txt"),
        "the drawn tree does not show the new file after leaving:\n{}",
        sim.screen()
    );
}

/// `F` reads the status for its baseline. While a project screen covers the changed-files list,
/// the screen in front is the project's (the list is not drawn) and the `C` filter stays the
/// tab's; the list is correct once it is in front again.
#[test]
fn vcs_project_follow_baseline_does_not_rebuild_the_covered_changed_files_list() {
    if isolated("vcs_project_follow_baseline_does_not_rebuild_the_covered_changed_files_list") {
        return;
    }
    let Some(repo) = overlay_repo("vcs_project_follow_baseline_list") else {
        return;
    };
    let cfg_base = crate::test_support::unique_tmp("vcs_project_follow_baseline_list_cfg");
    std::fs::write(repo.join("dirty-one.txt"), "1\n").unwrap();
    let mut sim = Sim::with_config_sized(&repo, config("git", "en"), 140, 30);
    sim.app.bookmarks = crate::bookmarks::Bookmarks::with_base(cfg_base.to_path_buf(), &repo);
    sim.app.tab.projects.wait_for_result();
    sim.app.poll_projects();
    drain(&mut sim);
    sim.key('C');
    drain(&mut sim);
    assert!(sim.app.changed_filter(), "C turns the changed-files list on");
    let paths = |sim: &Sim| {
        sim.app
            .tab
            .entries
            .iter()
            .map(|e| e.path.clone())
            .collect::<Vec<_>>()
    };
    let before = paths(&sim);
    assert!(before.iter().any(|p| p.ends_with("dirty-one.txt")), "{before:?}");
    sim.key('p');
    drain(&mut sim);
    assert!(sim.app.project_active());
    std::fs::write(repo.join("dirty-two.txt"), "2\n").unwrap();
    sim.app.mark_git_status_dirty();
    sim.app.toggle_follow();
    drain(&mut sim);
    assert!(sim.app.project_active());
    assert!(sim.app.changed_filter(), "the covered list's filter was dropped");
    sim.draw();
    assert_project_screen_in_front(&sim, "after F", &[]);
    sim.app.leave_project_screens();
    drain(&mut sim);
    assert!(sim.app.changed_filter());
    let after = paths(&sim);
    assert!(
        after.iter().any(|p| p.ends_with("dirty-two.txt")),
        "the list is stale after leaving: {after:?}"
    );
    sim.draw();
    assert!(
        sim.screen().contains("dirty-two.txt") && sim.screen().contains("dirty-one.txt"),
        "the drawn list is stale after leaving:\n{}",
        sim.screen()
    );
}

/// A watched change (the watcher seam `refresh_fs_watched`, which is what an external program's
/// edits reach) while a project screen covers the preview: the screen in front is the project's
/// and nothing of the preview is drawn; leaving the project shows the preview read again.
#[test]
fn vcs_project_watched_change_leaves_the_covered_preview_alone_until_it_is_in_front() {
    if isolated("vcs_project_watched_change_leaves_the_covered_preview_alone_until_it_is_in_front")
    {
        return;
    }
    let Some(repo) = overlay_repo("vcs_project_watched_preview") else {
        return;
    };
    let cfg_base = crate::test_support::unique_tmp("vcs_project_watched_preview_cfg");
    let mut sim = preview_session(&repo, config("git", "en"), "lines.txt", true, &cfg_base);
    let _ = sim.app.project_frame().unwrap();
    let old = preview_text(&mut sim);
    std::fs::write(repo.join("lines.txt"), "edited-one\nedited-two\n").unwrap();
    assert!(old.contains("one"), "{old}");
    // Nothing of the covered view is read again behind the project screen.
    let ((), behind) = crate::test_support::count_tab_view_loads(|| {
        sim.app.refresh_fs_watched(false, &[repo.join("lines.txt")]);
        drain(&mut sim);
        sim.draw();
    });
    assert_eq!(behind, 0, "the covered view was read again behind the screen");
    assert!(sim.app.project_active(), "the watched change left the project");
    assert_project_screen_in_front(&sim, "after the watched change", &[]);
    assert!(
        !sim.screen().contains("edited-one"),
        "the preview is drawn over the project screen:\n{}",
        sim.screen()
    );
    let ((), on_return) = crate::test_support::count_tab_view_loads(|| {
        sim.app.leave_project_screens();
        drain(&mut sim);
    });
    assert!(on_return >= 1, "the return read nothing again");
    sim.draw();
    assert!(
        sim.screen().contains("edited-one"),
        "stale preview drawn after leaving:\n{}",
        sim.screen()
    );
    let shown = preview_text(&mut sim);
    assert!(shown.contains("edited-one"), "stale preview after leaving: {shown}");
}

/// The same for the tree under a project screen: the tab's tree is not rebuilt behind the
/// project screen in front (the view is read again on the return).
#[test]
fn vcs_project_watched_change_leaves_the_covered_tree_alone_until_it_is_in_front() {
    if isolated("vcs_project_watched_change_leaves_the_covered_tree_alone_until_it_is_in_front") {
        return;
    }
    let Some(repo) = overlay_repo("vcs_project_watched_tree") else {
        return;
    };
    let cfg_base = crate::test_support::unique_tmp("vcs_project_watched_tree_cfg");
    let mut sim = Sim::with_config_sized(&repo, config("git", "en"), 140, 30);
    sim.app.bookmarks = crate::bookmarks::Bookmarks::with_base(cfg_base.to_path_buf(), &repo);
    sim.app.tab.projects.wait_for_result();
    sim.app.poll_projects();
    drain(&mut sim);
    sim.key('p');
    drain(&mut sim);
    assert!(sim.app.project_active());
    let before = sim.app.tab.entries.len();
    let made = repo.join("made-by-the-tool.txt");
    std::fs::write(&made, "x\n").unwrap();
    sim.app.refresh_fs_watched(false, &[made]);
    drain(&mut sim);
    assert!(sim.app.project_active());
    sim.draw();
    assert_project_screen_in_front(&sim, "after the watched change", &[]);
    assert_eq!(sim.app.tab.entries.len(), before, "the covered tree was rebuilt");
    sim.app.leave_project_screens();
    drain(&mut sim);
    assert!(
        sim.app.tab.entries.iter().any(|e| e.path.ends_with("made-by-the-tool.txt")),
        "the tree did not show the new file after leaving"
    );
    sim.draw();
    assert!(
        sim.screen().contains("made-by-the-tool.txt"),
        "the drawn tree does not show the new file after leaving:\n{}",
        sim.screen()
    );
}

/// A status that lands while a project screen covers tab 0's changed-files list stays tab 0's: a
/// new tab shows its own tree (no `C` list), switching back shows tab 0's project screen in front,
/// and leaving it draws tab 0's list with the change of that status.
#[test]
fn vcs_project_status_landed_while_covered_stays_with_its_tab() {
    if isolated("vcs_project_status_landed_while_covered_stays_with_its_tab") {
        return;
    }
    let Some(repo) = overlay_repo("vcs_project_covered_flag_tab") else {
        return;
    };
    let cfg_base = crate::test_support::unique_tmp("vcs_project_covered_flag_tab_cfg");
    std::fs::write(repo.join("dirty-one.txt"), "1\n").unwrap();
    let mut sim = Sim::with_config_sized(&repo, config("git", "en"), 140, 30);
    sim.app.bookmarks = crate::bookmarks::Bookmarks::with_base(cfg_base.to_path_buf(), &repo);
    sim.app.tab.projects.wait_for_result();
    sim.app.poll_projects();
    drain(&mut sim);
    sim.key('C');
    drain(&mut sim);
    sim.key('p');
    drain(&mut sim);
    assert!(sim.app.project_active());
    let (tx, rx) = std::sync::mpsc::channel();
    sim.app.attach_status_loader(tx);
    std::fs::write(repo.join("dirty-two.txt"), "2\n").unwrap();
    sim.app.mark_git_status_dirty();
    sim.app.refresh_git_if_needed();
    let res = rx.recv_timeout(std::time::Duration::from_secs(30)).expect("status result");
    let applied = sim.app.apply_statuses(res);
    assert!(applied, "result discarded");
    sim.app.tab_new().unwrap();
    drain(&mut sim);
    assert!(!sim.app.project_active(), "the new tab inherited the project screen");
    assert!(!sim.app.changed_filter(), "the new tab inherited the C list");
    sim.app.tab_goto(0);
    drain(&mut sim);
    sim.draw();
    assert_project_screen_in_front(&sim, "tab 0 again", &[]);
    assert!(sim.app.changed_filter(), "tab 0 lost its C list");
    sim.app.leave_project_screens();
    drain(&mut sim);
    sim.draw();
    // Leaving is a return to the tab: the tree saved at the cover comes back, the list it had,
    // and its refresh asks for a scan. Until that scan lands the list it had is kept (as on a tab
    // switch); it is not the whole tree, which would show dirty-two.txt as a plain file.
    assert!(
        sim.app.git_status_in_flight(),
        "leaving asked for no scan:\n{}",
        sim.screen()
    );
    let names = overlay_tree_names(&sim);
    assert!(
        names.iter().any(|n| n == "dirty-one.txt") && !names.iter().any(|n| n == "dirty-two.txt"),
        "tab 0's list while the return's scan is out is not the list it had: {names:?}\n{}",
        sim.screen()
    );
    for _ in 0..4 {
        if !sim.app.git_status_in_flight() {
            break;
        }
        let res = rx
            .recv_timeout(std::time::Duration::from_secs(30))
            .expect("the return's status result");
        sim.app.apply_statuses(res);
    }
    assert!(!sim.app.git_status_in_flight(), "the return's scan did not land");
    sim.draw();
    assert!(sim.app.changed_filter(), "tab 0 lost its C list after the scan");
    assert!(
        sim.screen().contains("dirty-two.txt"),
        "tab 0's list does not show the status that landed while covered:\n{}",
        sim.screen()
    );
}

/// Leaving the project while that tab's git status is still being fetched does not rebuild the
/// changed-files list from the old status: the rebuild is left to the status that lands.
#[test]
fn vcs_project_covered_rebuild_waits_for_the_status() {
    if isolated("vcs_project_covered_rebuild_waits_for_the_status") {
        return;
    }
    let Some(repo) = overlay_repo("vcs_project_covered_rebuild_waits") else {
        return;
    };
    let cfg_base = crate::test_support::unique_tmp("vcs_project_covered_rebuild_waits_cfg");
    std::fs::write(repo.join("dirty-one.txt"), "1\n").unwrap();
    let mut sim = Sim::with_config_sized(&repo, config("git", "en"), 140, 30);
    sim.app.bookmarks = crate::bookmarks::Bookmarks::with_base(cfg_base.to_path_buf(), &repo);
    sim.app.tab.projects.wait_for_result();
    sim.app.poll_projects();
    drain(&mut sim);
    sim.key('C');
    drain(&mut sim);
    sim.key('p');
    drain(&mut sim);
    let (tx, rx) = std::sync::mpsc::channel();
    sim.app.attach_status_loader(tx);
    std::fs::write(repo.join("dirty-two.txt"), "2\n").unwrap();
    sim.app.mark_git_status_dirty();
    sim.app.refresh_git_if_needed();
    let res = rx.recv_timeout(std::time::Duration::from_secs(30)).expect("status result");
    sim.app.apply_statuses(res);
    assert!(sim.app.project_active());
    // A newer status is requested and still in flight when the project is left.
    std::fs::write(repo.join("dirty-three.txt"), "3\n").unwrap();
    sim.app.mark_git_status_dirty();
    sim.app.refresh_git_if_needed();
    assert!(sim.app.git_status_in_flight(), "the scan is in flight");
    sim.app.leave_project_screens();
    assert!(sim.app.changed_filter(), "leaving dropped the changed-files list");
    assert!(
        sim.app.changed_anchor_left_to_status(),
        "the rebuild did not wait for the status"
    );
    let res = rx.recv_timeout(std::time::Duration::from_secs(30)).expect("status result");
    sim.app.apply_statuses(res);
    assert!(sim.app.changed_filter());
    let paths: Vec<_> = sim.app.tab.entries.iter().map(|e| e.path.clone()).collect();
    assert!(
        paths.iter().any(|p| p.ends_with("dirty-three.txt")),
        "the list is wrong after the status landed: {paths:?}"
    );
    sim.draw();
    assert!(
        sim.screen().contains("dirty-three.txt"),
        "the drawn list is wrong after the status landed:\n{}",
        sim.screen()
    );
}

// --- A screen covered by a project screen is a tab that was left (e4-review items 1-4): what the
// App keeps fresh for every tab goes on while covered, and what was asked for the screen that was
// left does not land behind the screen in front.

/// The file the tree cursor is on, by name.
fn overlay_cursor_file(sim: &Sim) -> Option<String> {
    sim.app
        .tab
        .entries
        .get(sim.app.tab.selected)
        .map(|e| e.path.file_name().unwrap().to_string_lossy().into_owned())
}

fn overlay_tree_names(sim: &Sim) -> Vec<String> {
    sim.app
        .tab
        .entries
        .iter()
        .map(|e| e.path.file_name().unwrap().to_string_lossy().into_owned())
        .collect()
}

/// Whether the drawn row of `name` is the reversed cursor row.
fn overlay_drawn_reversed(sim: &Sim, name: &str) -> bool {
    sim.style_of(name)
        .is_some_and(|st| st.add_modifier.contains(ratatui::style::Modifier::REVERSED))
}

/// e4-review item 1: two repositories, each tab with its `C` list under a project screen. Tab 0
/// (repo A) is switched back to from tab 1 (repo B): the switch to a repository of its own drops
/// the other repository's statuses and asks for a scan, and `q` leaves the project before that
/// scan lands. The `C` list stays on and, once the scan lands, it is drawn with the cursor on the
/// file it was on when `p` covered it.
#[test]
fn vcs_project_two_repos_quit_before_the_status_keeps_the_changed_list_and_its_cursor() {
    if isolated("vcs_project_two_repos_quit_before_the_status_keeps_the_changed_list_and_its_cursor")
    {
        return;
    }
    let parent = sandbox("vcs_project_two_repos_quit");
    let make = |name: &str| {
        let dir = parent.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        // A clean tracked file: the `C` list is told from the whole tree by its absence.
        std::fs::write(dir.join("k-clean.txt"), "clean\n").unwrap();
        super::vcs_changes::git_repo_at(&dir, true);
        dir.canonicalize().unwrap()
    };
    let a = make("a-repo");
    let _b = make("b-repo");
    let cfg_base = crate::test_support::unique_tmp("vcs_project_two_repos_quit_cfg");
    let mut sim = Sim::with_config_sized(&a, config("git", "en"), 140, 30);
    sim.app.bookmarks = crate::bookmarks::Bookmarks::with_base(cfg_base.to_path_buf(), &a);
    sim.app.tab.projects.wait_for_result();
    sim.app.poll_projects();
    drain(&mut sim);
    sim.key('C');
    drain(&mut sim);
    assert!(sim.app.changed_filter(), "C turns the changed-files list on");
    assert!(
        !overlay_tree_names(&sim).iter().any(|n| n == "k-clean.txt"),
        "not the changed list: {:?}",
        overlay_tree_names(&sim)
    );
    press(&mut sim, KeyCode::Char('j'));
    press(&mut sim, KeyCode::Char('j'));
    let on = overlay_cursor_file(&sim).expect("the cursor is on a file");
    assert_ne!(sim.app.tab.selected, 0, "the cursor is off the first row");
    sim.key('p');
    drain(&mut sim);
    assert!(sim.app.project_active(), "p covers tab 0's list");
    // Tab 1 on repository B, its `C` list covered the same way.
    sim.app.tab_new().unwrap();
    drain(&mut sim);
    assert!(!sim.app.project_active(), "the new tab shows its own tree");
    sim.key('h');
    drain(&mut sim);
    sim.select("b-repo");
    sim.key('l');
    sim.app.tab.projects.wait_for_result();
    sim.app.poll_projects();
    drain(&mut sim);
    assert!(sim.app.tab.root.ends_with("b-repo"), "{:?}", sim.app.tab.root);
    sim.key('C');
    drain(&mut sim);
    assert!(sim.app.changed_filter(), "tab 1's C list");
    sim.key('p');
    drain(&mut sim);
    assert!(sim.app.project_active(), "p covers tab 1's list");
    // The scans are asked of a loader from here on, so tab 0's scan can be held back.
    let (tx, rx) = std::sync::mpsc::channel();
    sim.app.attach_status_loader(tx);
    sim.app.tab_goto(0);
    drain(&mut sim);
    assert!(sim.app.tab.root.ends_with("a-repo"));
    assert!(
        sim.app.git_status_in_flight(),
        "the switch to repository A asked for its scan"
    );
    sim.draw();
    assert_project_screen_in_front(&sim, "tab 0 again", &[]);
    // `q` before the scan lands: the `C` list is drawn at once, with the cursor on its file (no
    // moment of the plain tree, no cursor on the first row).
    press(&mut sim, KeyCode::Char('q'));
    assert!(!sim.app.project_active(), "q left the project screen");
    assert!(
        sim.app.git_status_in_flight(),
        "the scan landed before the check"
    );
    assert_two_repos_changed_list_drawn(&mut sim, &on, "after q, before the scan lands");
    let mut landed = 0;
    while let Ok(res) = rx.recv_timeout(std::time::Duration::from_secs(3)) {
        sim.app.apply_statuses(res);
        landed += 1;
    }
    assert!(landed >= 1, "no scan landed");
    assert_two_repos_changed_list_drawn(&mut sim, &on, "after the scan landed");
    // Covered again, tab 0 is switched to tab 1 and back while covered: each switch drops the
    // other repository's statuses and asks for a scan. `q` before that scan lands shows the same.
    sim.key('p');
    drain(&mut sim);
    assert!(sim.app.project_active(), "p covers tab 0's list again");
    sim.app.tab_goto(1);
    drain(&mut sim);
    assert!(sim.app.tab.root.ends_with("b-repo"), "{:?}", sim.app.tab.root);
    assert!(sim.app.project_active(), "tab 1 comes back covered");
    sim.app.tab_goto(0);
    drain(&mut sim);
    assert!(sim.app.tab.root.ends_with("a-repo"));
    assert!(
        sim.app.git_status_in_flight(),
        "the switch back to repository A asked for its scan"
    );
    sim.draw();
    assert_project_screen_in_front(&sim, "tab 0 after the round trip", &[]);
    press(&mut sim, KeyCode::Char('q'));
    assert!(!sim.app.project_active(), "q left the project screen");
    assert!(
        sim.app.git_status_in_flight(),
        "the scan landed before the check"
    );
    assert_two_repos_changed_list_drawn(
        &mut sim,
        &on,
        "after the round trip and q, before the scan lands",
    );
    let mut landed = 0;
    while let Ok(res) = rx.recv_timeout(std::time::Duration::from_secs(3)) {
        sim.app.apply_statuses(res);
        landed += 1;
    }
    assert!(landed >= 1, "no scan landed after the round trip");
    assert_two_repos_changed_list_drawn(&mut sim, &on, "after the round trip's scan landed");
}

/// e4-review item 1 on a plain tab switch (no project screen): the same rule. Tab 0's `C` list on
/// repository A, tab 1 on repository B; switching back to tab 0 drops B's statuses and asks for
/// A's scan. Until it lands the list tab 0 had is drawn, cursor and reversed row on its file, not
/// the whole tree under the "CHANGED (n)" title; once it lands the list is the same.
#[test]
fn vcs_two_repos_tab_switch_before_the_status_keeps_the_changed_list_and_its_cursor() {
    if isolated("vcs_two_repos_tab_switch_before_the_status_keeps_the_changed_list_and_its_cursor")
    {
        return;
    }
    let parent = sandbox("vcs_two_repos_tab_switch");
    let make = |name: &str| {
        let dir = parent.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("k-clean.txt"), "clean\n").unwrap();
        super::vcs_changes::git_repo_at(&dir, true);
        dir.canonicalize().unwrap()
    };
    let a = make("a-repo");
    let _b = make("b-repo");
    let cfg_base = crate::test_support::unique_tmp("vcs_two_repos_tab_switch_cfg");
    let mut sim = Sim::with_config_sized(&a, config("git", "en"), 140, 30);
    sim.app.bookmarks = crate::bookmarks::Bookmarks::with_base(cfg_base.to_path_buf(), &a);
    sim.app.tab.projects.wait_for_result();
    sim.app.poll_projects();
    drain(&mut sim);
    sim.key('C');
    drain(&mut sim);
    assert!(sim.app.changed_filter(), "C turns the changed-files list on");
    press(&mut sim, KeyCode::Char('j'));
    press(&mut sim, KeyCode::Char('j'));
    let on = overlay_cursor_file(&sim).expect("the cursor is on a file");
    assert_ne!(sim.app.tab.selected, 0, "the cursor is off the first row");
    assert_two_repos_changed_list_drawn(&mut sim, &on, "before the switch");
    sim.app.tab_new().unwrap();
    drain(&mut sim);
    sim.key('h');
    drain(&mut sim);
    sim.select("b-repo");
    sim.key('l');
    sim.app.tab.projects.wait_for_result();
    sim.app.poll_projects();
    drain(&mut sim);
    assert!(sim.app.tab.root.ends_with("b-repo"), "{:?}", sim.app.tab.root);
    let (tx, rx) = std::sync::mpsc::channel();
    sim.app.attach_status_loader(tx);
    sim.app.tab_goto(0);
    drain(&mut sim);
    assert!(sim.app.tab.root.ends_with("a-repo"));
    assert!(
        sim.app.git_status_in_flight(),
        "the switch to repository A asked for its scan"
    );
    assert_two_repos_changed_list_drawn(&mut sim, &on, "tab switch, before the scan lands");
    let mut landed = 0;
    while let Ok(res) = rx.recv_timeout(std::time::Duration::from_secs(3)) {
        sim.app.apply_statuses(res);
        landed += 1;
    }
    assert!(landed >= 1, "no scan landed");
    assert_two_repos_changed_list_drawn(&mut sim, &on, "tab switch, after the scan landed");
}

/// The `C` list of `vcs_project_two_repos_quit_before_the_status_keeps_the_changed_list_and_its_cursor`
/// is on, drawn without the clean file, with the cursor on `on` and its drawn row reversed.
fn assert_two_repos_changed_list_drawn(sim: &mut Sim, on: &str, when: &str) {
    sim.draw();
    assert!(sim.app.changed_filter(), "{when}: the C list was turned off");
    let names = overlay_tree_names(sim);
    assert!(
        !names.iter().any(|n| n == "k-clean.txt"),
        "{when}: the list is not the changed list: {names:?}\n{}",
        sim.screen()
    );
    assert!(
        !sim.screen().contains("k-clean.txt"),
        "{when}: the drawn list is the plain tree\n{}",
        sim.screen()
    );
    assert_eq!(
        overlay_cursor_file(sim).as_deref(),
        Some(on),
        "{when}: the cursor left its file\n{}",
        sim.screen()
    );
    assert!(
        overlay_drawn_reversed(sim, on),
        "{when}: the drawn cursor is not on {on}\n{}",
        sim.screen()
    );
}

/// e4-review item 2: a Markdown document's embedded picture is overwritten while a project screen
/// covers the document, and the watcher reports it (`refresh_fs_watched`, the run loop's call).
/// The picture cache is the App's and is kept for every screen, so the entry of the changed path
/// goes while covered; leaving the project draws the new pixels.
#[test]
fn vcs_project_picture_rewritten_while_covered_is_drawn_new_after_leaving() {
    if isolated("vcs_project_picture_rewritten_while_covered_is_drawn_new_after_leaving") {
        return;
    }
    let Some(repo) = overlay_repo("vcs_project_picture_rewritten") else {
        return;
    };
    let cfg_base = crate::test_support::unique_tmp("vcs_project_picture_rewritten_cfg");
    let png = repo.join("pic.png");
    write_solid_png(&png, 40, 20, [10, 210, 10]);
    std::fs::write(repo.join("pic.md"), "before\n\n![a](pic.png)\n\nafter\n").unwrap();
    let mut sim = Sim::with_config_sized(&repo, config("git", "en"), 140, 30).with_media();
    sim.app.bookmarks = crate::bookmarks::Bookmarks::with_base(cfg_base.to_path_buf(), &repo);
    sim.app.tab.projects.wait_for_result();
    sim.app.poll_projects();
    drain(&mut sim);
    sim.select("pic.md");
    sim.enter();
    sim.drain_md_images();
    sim.drain_md_encodes();
    assert!(drawn_has_green(&sim.term), "the first picture (green) is drawn");
    let placement = sim.app.md_images();
    assert_eq!(placement.len(), 1, "{placement:?}");
    let (url, cols, rows) = (placement[0].url.clone(), placement[0].cols, placement[0].rows);
    sim.key('p');
    drain(&mut sim);
    assert!(sim.app.project_active(), "p covers the document");
    write_solid_png(&png, 40, 20, [210, 10, 10]);
    sim.app.refresh_fs_watched(false, std::slice::from_ref(&png));
    drain(&mut sim);
    assert_project_screen_in_front(&sim, "after the watched change", &[]);
    press(&mut sim, KeyCode::Char('q'));
    assert!(!sim.app.project_active(), "q left the project screen");
    assert_eq!(sim.app.tab.mode, Mode::Preview);
    sim.draw();
    assert!(
        sim.app.md_image_proto(&url, cols, rows, 0, rows).is_none(),
        "the cache entry of the picture rewritten while covered was kept"
    );
    sim.drain_md_images();
    sim.drain_md_encodes();
    assert!(
        drawn_has_red(&sim.term),
        "the new pixels (red) are not drawn after leaving"
    );
    assert!(!drawn_has_green(&sim.term), "the old pixels (green) are drawn");
}

/// A tab whose screen a project screen covers is switched away from and back (`]`, then `[`). The
/// picture is rewritten while covered, so the return cannot reuse the stashed pixels: it puts the
/// tab back from its saved copy and starts a real load of the new file. The screen in front is
/// the project's: the tab view stays left as the cover left it (`put_tab_back` ends with
/// `leave_tab_view` while a project screen covers it). So no picture is shown behind the project
/// screen and none of the results of that load is applied (its generation is gone); at least one
/// result is seen, so the check is not vacuous. Leaving the project screen (`q`) then shows the
/// new picture.
#[test]
fn vcs_project_tab_switch_back_to_a_covered_tab_runs_no_picture_behind_the_screen() {
    if isolated("vcs_project_tab_switch_back_to_a_covered_tab_runs_no_picture_behind_the_screen") {
        return;
    }
    let Some(repo) = overlay_repo("vcs_project_covered_switch_picture") else {
        return;
    };
    let cfg_base = crate::test_support::unique_tmp("vcs_project_covered_switch_picture_cfg");
    crate::test_support::write_solid_png(&repo.join("pic.png"), 40, 20, [10, 210, 10]);
    let mut sim = Sim::with_config_sized(&repo, config("git", "en"), 140, 30).with_media();
    sim.app.bookmarks = crate::bookmarks::Bookmarks::with_base(cfg_base.to_path_buf(), &repo);
    sim.app.tab.projects.wait_for_result();
    sim.app.poll_projects();
    drain(&mut sim);
    sim.select("pic.png");
    sim.enter();
    sim.drain_media();
    assert!(sim.app.is_image_preview(), "premise: the picture is shown");
    // A full-screen picture reaches the drawn cells once its resizes are answered (the run loop's
    // resize worker).
    sim.draw();
    sim.answer_image_resizes();
    assert!(drawn_has_green(&sim.term), "premise: the first picture (green) is drawn");
    sim.key('p');
    drain(&mut sim);
    assert!(sim.app.project_active(), "p covers the picture");
    assert!(
        !sim.app.is_image_preview(),
        "premise: the cover leaves the picture"
    );
    // Another size and colour: the stash of the picture no longer matches the file (its length
    // and time differ), so the return loads it again.
    crate::test_support::write_solid_png(&repo.join("pic.png"), 44, 22, [210, 10, 10]);

    sim.app.tab_new().unwrap();
    drain(&mut sim);
    assert_eq!(sim.app.active_tab_index(), 1, "premise: on the new tab");
    sim.app.tab_cycle(-1);
    drain(&mut sim);
    assert_eq!(sim.app.active_tab_index(), 0, "premise: back on the covered tab");
    assert_project_screen_in_front(&sim, "back on the covered tab", &[]);

    // Whatever the return's reload sent is from a generation the cover has left.
    let rx = sim.media_rx.as_ref().expect("with_media");
    let mut seen = 0;
    while let Ok(res) = rx.recv_timeout(std::time::Duration::from_secs(5)) {
        seen += 1;
        assert!(
            !sim.app.apply_media(res),
            "a picture load of the covered tab was applied behind the project screen"
        );
    }
    assert!(seen >= 1, "the return started no load of the rewritten picture");
    sim.draw();
    assert!(
        !sim.app.is_image_preview(),
        "the covered tab's picture is held behind the project screen"
    );

    press(&mut sim, KeyCode::Char('q'));
    assert!(!sim.app.project_active(), "q left the project screen");
    assert_eq!(sim.app.tab.mode, Mode::Preview);
    sim.await_media("leaving the project screen");
    assert!(sim.app.is_image_preview(), "the picture is shown again after leaving");
    sim.draw();
    sim.answer_image_resizes();
    assert!(drawn_has_red(&sim.term), "the rewritten picture (red) is not drawn after leaving");
    assert!(!drawn_has_green(&sim.term), "the old picture (green) is drawn after leaving");
}

/// e4-review item 3: a clean tracked file is edited while a project screen covers the tree, the
/// watcher reports it, and `F` is pressed over the project screen. The status is the App's: the
/// watch event marked it stale while covered, so `F` reads it again and the edit made before `F`
/// is in the follow baseline. After leaving, an edit made after `F` is followed: its diff shows
/// that edit only, not the one made before `F`.
#[test]
fn vcs_project_edit_watched_while_covered_is_in_the_follow_baseline() {
    if isolated("vcs_project_edit_watched_while_covered_is_in_the_follow_baseline") {
        return;
    }
    let Some(repo) = overlay_repo("vcs_project_follow_watched") else {
        return;
    };
    let cfg_base = crate::test_support::unique_tmp("vcs_project_follow_watched_cfg");
    let file = repo.join("tracked.txt");
    std::fs::write(&file, "line1\nline2\nline3\n").unwrap();
    crate::test_support::run_git(&repo, &["add", "tracked.txt"]);
    crate::test_support::run_git(&repo, &["commit", "-qm", "tracked", "--", "tracked.txt"]);
    let mut sim = Sim::with_config_sized(&repo, config("git", "en"), 140, 30);
    sim.app.bookmarks = crate::bookmarks::Bookmarks::with_base(cfg_base.to_path_buf(), &repo);
    sim.app.tab.projects.wait_for_result();
    sim.app.poll_projects();
    drain(&mut sim);
    assert!(
        sim.app.git_status_of(&file).is_none(),
        "the status read before the cover has the file clean"
    );
    sim.key('p');
    drain(&mut sim);
    assert!(sim.app.project_active());
    std::fs::write(&file, "line1\nPRE-FOLLOW\nline3\n").unwrap();
    sim.app.refresh_fs_watched(false, std::slice::from_ref(&file));
    drain(&mut sim);
    assert_project_screen_in_front(&sim, "after the watched change", &[]);
    press(&mut sim, KeyCode::Char('F'));
    assert!(sim.app.follow_enabled(), "F over the project screen turns follow on");
    press(&mut sim, KeyCode::Char('q'));
    assert!(!sim.app.project_active(), "q left the project screen");
    assert!(sim.app.follow_enabled(), "leaving the project turned follow off");
    // An edit after `F`, followed as the run loop follows it.
    std::fs::write(&file, "line1\nPRE-FOLLOW\nPOST-FOLLOW\n").unwrap();
    sim.app.refresh_fs_watched(false, std::slice::from_ref(&file));
    sim.app.follow_jump(&file);
    drain(&mut sim);
    assert!(sim.app.is_git_diff_preview(), "the followed edit opens its diff");
    let added: Vec<String> = sim
        .app
        .git_diff_lines()
        .iter()
        .filter(|l| l.kind == crate::git::DiffLineKind::Added)
        .map(|l| l.text.clone())
        .collect();
    assert!(
        added.iter().any(|t| t == "POST-FOLLOW"),
        "the edit after F is not in the diff: {added:?}"
    );
    assert!(
        !added.iter().any(|t| t == "PRE-FOLLOW"),
        "the edit made before F (watched while covered) is not in the baseline: {added:?}"
    );
}

/// e4-review item 4: a `/` filter whose pool scan is still on the worker when `p` covers the
/// tree. The scan was asked for the screen that was left, so its result (applied as the run loop
/// applies it) does not change that screen: right after leaving, the list and its cursor are the
/// ones that were covered, as after a tab switch away and back. The scan the return asks for then
/// fills the list in with the cursor on its file.
#[test]
fn vcs_project_filter_pool_scan_landing_while_covered_leaves_the_covered_list_alone() {
    if isolated("vcs_project_filter_pool_scan_landing_while_covered_leaves_the_covered_list_alone")
    {
        return;
    }
    let Some(repo) = overlay_repo("vcs_project_filter_pool_covered") else {
        return;
    };
    let cfg_base = crate::test_support::unique_tmp("vcs_project_filter_pool_covered_cfg");
    std::fs::write(repo.join("deep_root1.txt"), "r\n").unwrap();
    std::fs::write(repo.join("deep_root2.txt"), "r\n").unwrap();
    for i in 0..6 {
        let sub = repo.join(format!("pkg{i}"));
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join(format!("deep{i}.csv")), "a,b\n").unwrap();
    }
    crate::app::set_filter_scan_budget(Some(Some(std::time::Duration::ZERO)));
    let mut sim = Sim::with_config_sized(&repo, config("git", "en"), 140, 30);
    sim.app.bookmarks = crate::bookmarks::Bookmarks::with_base(cfg_base.to_path_buf(), &repo);
    sim.app.tab.projects.wait_for_result();
    sim.app.poll_projects();
    drain(&mut sim);
    let (tx, rx) = std::sync::mpsc::channel();
    sim.app.attach_filter_pool_loader(tx);
    sim.key('/');
    sim.keys("deep");
    sim.enter();
    drain(&mut sim);
    assert!(sim.app.filter_query().is_some(), "the filter is on");
    let covered = overlay_tree_names(&sim);
    assert!(
        !covered.iter().any(|n| n.ends_with(".csv")),
        "the scan is not on the worker: {covered:?}"
    );
    sim.select("deep_root2.txt");
    let covered_cursor = overlay_cursor_file(&sim);
    sim.key('p');
    drain(&mut sim);
    assert!(sim.app.project_active(), "p covers the filtered list");
    let res = rx
        .recv_timeout(std::time::Duration::from_secs(30))
        .expect("the scan asked before the cover lands");
    let applied = sim.app.apply_filter_pool(res);
    drain(&mut sim);
    assert_project_screen_in_front(&sim, "after the scan landed", &[]);
    press(&mut sim, KeyCode::Char('q'));
    assert!(!sim.app.project_active(), "q left the project screen");
    sim.draw();
    assert_eq!(
        overlay_tree_names(&sim),
        covered,
        "the scan asked for the covered screen changed it (applied={applied})\n{}",
        sim.screen()
    );
    assert_eq!(overlay_cursor_file(&sim), covered_cursor, "the cursor left its file");
    assert!(
        overlay_drawn_reversed(&sim, "deep_root2.txt"),
        "the drawn cursor is not on deep_root2.txt\n{}",
        sim.screen()
    );
    // The scan of the return fills the list in, the cursor on its file.
    let res = rx
        .recv_timeout(std::time::Duration::from_secs(30))
        .expect("the return asks for a scan of its own");
    assert!(sim.app.apply_filter_pool(res), "the return's scan is applied");
    sim.draw();
    let names = overlay_tree_names(&sim);
    for i in 0..6 {
        assert!(
            names.iter().any(|n| *n == format!("deep{i}.csv")),
            "the return's scan did not fill the list in: {names:?}"
        );
    }
    assert_eq!(overlay_cursor_file(&sim), covered_cursor, "the cursor left its file");
    crate::app::set_filter_scan_budget(None);
}

/// A tracked file of `repo`, committed as `old` and then rewritten as `new` (a working-tree change).
fn overlay_tracked_change(repo: &std::path::Path, name: &str, old: &[u8], new: &[u8]) -> std::path::PathBuf {
    let path = repo.join(name);
    std::fs::write(&path, old).unwrap();
    crate::test_support::run_git(repo, &["add", name]);
    crate::test_support::run_git(repo, &["commit", "-qm", name, "--", name]);
    std::fs::write(&path, new).unwrap();
    path
}

/// fix-e6a A: a Markdown diff shown Rendered asks the block diff of its worker; the answer lands
/// after `p` covered the diff. The screen it was asked for is left (`leave_tab_view` drops the
/// block diff of the screen that is left, as a tab switch does), so the answer is not applied
/// behind the project screen. Leaving the project (`q`) shows the diff Rendered again with the
/// answer to its own new request.
#[test]
fn vcs_project_md_diff_landing_after_the_cover_is_not_applied_behind_the_screen() {
    if isolated("vcs_project_md_diff_landing_after_the_cover_is_not_applied_behind_the_screen") {
        return;
    }
    let Some(repo) = overlay_repo("vcs_project_md_diff_covered") else {
        return;
    };
    let cfg_base = crate::test_support::unique_tmp("vcs_project_md_diff_covered_cfg");
    let doc = overlay_tracked_change(
        &repo,
        "doc.md",
        b"# Title\n\nold paragraph\n",
        b"# Title\n\nnew paragraph\n\nadded block\n",
    );
    let mut sim = Sim::with_config_sized(&repo, config("git", "en"), 140, 30).with_async_md_diff();
    sim.app.bookmarks = crate::bookmarks::Bookmarks::with_base(cfg_base.to_path_buf(), &repo);
    sim.app.tab.projects.wait_for_result();
    sim.app.poll_projects();
    drain(&mut sim);
    sim.app.open_git_diff(&doc);
    assert_eq!(sim.app.diff_view_for_test(), crate::app::DiffView::Rendered);
    assert!(sim.app.md_diff_pending_for_current(), "premise: the block diff is out");
    sim.key('p');
    assert!(sim.app.project_active(), "p covers the diff");
    let rx = sim.md_diff_rx.as_ref().expect("with_async_md_diff");
    let res = rx
        .recv_timeout(std::time::Duration::from_secs(30))
        .expect("the block diff asked before the cover lands");
    assert!(
        !sim.app.apply_md_diff(res),
        "the block diff of the covered diff was applied behind the project screen"
    );
    drain(&mut sim);
    assert_project_screen_in_front(&sim, "after the block diff landed", &[]);
    press(&mut sim, KeyCode::Char('q'));
    assert!(!sim.app.project_active(), "q left the project screen");
    assert_eq!(sim.app.diff_view_for_test(), crate::app::DiffView::Rendered);
    sim.drain_md_diff();
    let marks = sim
        .app
        .diff_rendered_marks_for_test()
        .expect("the Rendered diff is built after leaving");
    assert!(!marks.is_empty(), "the Rendered diff has no change marks: {marks:?}");
}

/// A session showing `doc.md`'s working-tree diff Rendered, its block diff landed and drawn.
fn rendered_md_diff_session(repo: &std::path::Path, cfg_base: &std::path::Path) -> Sim {
    let doc = overlay_tracked_change(
        repo,
        "doc.md",
        b"# Title\n\nold paragraph\n",
        b"# Title\n\nnew paragraph\n\nadded block\n",
    );
    let mut sim = Sim::with_config_sized(repo, config("git", "en"), 140, 30).with_async_md_diff();
    sim.app.bookmarks = crate::bookmarks::Bookmarks::with_base(cfg_base.to_path_buf(), repo);
    sim.app.tab.projects.wait_for_result();
    sim.app.poll_projects();
    drain(&mut sim);
    sim.app.open_git_diff(&doc);
    assert_eq!(sim.app.diff_view_for_test(), crate::app::DiffView::Rendered);
    sim.drain_md_diff();
    assert!(
        sim.app
            .diff_rendered_marks_for_test()
            .is_some_and(|marks| !marks.is_empty()),
        "premise: the Rendered diff is built with its change marks"
    );
    sim
}

/// The return to a Rendered Markdown diff asks its block diff once, after the App-level freshness
/// of the return (which drops the diff caches and the block diff asked before it): `steps` is
/// the return (a tab switch back, or `q` off a project screen) and its first draw. Exactly one
/// ask goes out, and its answer is the current one (applied, with the change marks).
fn assert_return_asks_the_block_diff_once(sim: &mut Sim, road: &str, steps: impl FnOnce(&mut Sim)) {
    let ((), asks) = crate::test_support::count_md_diff_asks(|| steps(sim));
    assert_eq!(sim.app.diff_view_for_test(), crate::app::DiffView::Rendered, "{road}");
    assert_eq!(
        asks, 1,
        "{road}: the return asked the block diff {asks} times (an ask made before the return's \
         freshness is dropped by it and asked again by the draw)"
    );
    sim.drain_md_diff();
    let marks = sim
        .app
        .diff_rendered_marks_for_test()
        .unwrap_or_else(|| panic!("{road}: the Rendered diff is built after the return"));
    assert!(!marks.is_empty(), "{road}: the Rendered diff has no change marks: {marks:?}");
    let ((), later) = crate::test_support::count_md_diff_asks(|| sim.draw());
    assert_eq!(later, 0, "{road}: the landed block diff is asked for again");
}

/// handoff-e6-6 item 1, tab road: a tab switch away from a Rendered Markdown diff and back asks
/// the block diff exactly once, and that answer is applied.
#[test]
fn vcs_project_tab_switch_back_asks_the_block_diff_once() {
    if isolated("vcs_project_tab_switch_back_asks_the_block_diff_once") {
        return;
    }
    let Some(repo) = overlay_repo("vcs_project_md_diff_tab_back") else {
        return;
    };
    let cfg_base = crate::test_support::unique_tmp("vcs_project_md_diff_tab_back_cfg");
    let mut sim = rendered_md_diff_session(&repo, &cfg_base);
    sim.app.tab_new().unwrap();
    drain(&mut sim);
    assert_eq!(sim.app.active_tab_index(), 1, "premise: on the new tab");
    assert_return_asks_the_block_diff_once(&mut sim, "tab switch back", |sim| {
        sim.app.tab_cycle(-1);
        sim.draw();
        assert_eq!(sim.app.active_tab_index(), 0, "premise: back on the diff's tab");
    });
}

/// handoff-e6-6 item 1, project road: covering a Rendered Markdown diff with `p` and leaving with
/// `q` asks the block diff exactly once (as the tab switch back does), and that answer is applied.
#[test]
fn vcs_project_uncover_asks_the_block_diff_once() {
    if isolated("vcs_project_uncover_asks_the_block_diff_once") {
        return;
    }
    let Some(repo) = overlay_repo("vcs_project_md_diff_uncover") else {
        return;
    };
    let cfg_base = crate::test_support::unique_tmp("vcs_project_md_diff_uncover_cfg");
    let mut sim = rendered_md_diff_session(&repo, &cfg_base);
    let ((), covered_asks) = crate::test_support::count_md_diff_asks(|| {
        sim.key('p');
        drain(&mut sim);
    });
    assert!(sim.app.project_active(), "premise: p covers the diff");
    assert_eq!(covered_asks, 0, "a block diff was asked behind the project screen");
    assert_return_asks_the_block_diff_once(&mut sim, "uncover", |sim| {
        press(sim, KeyCode::Char('q'));
        assert!(!sim.app.project_active(), "premise: q left the project screen");
    });
}

/// fix-e6a A: an image diff asks its pictures of the media-diff worker; the answer lands after
/// `p` covered the diff. It is not applied behind the project screen (`leave_tab_view` drops the
/// media diff of the screen that is left); leaving the project asks again and that answer lands.
#[test]
fn vcs_project_media_diff_landing_after_the_cover_is_not_applied_behind_the_screen() {
    if isolated("vcs_project_media_diff_landing_after_the_cover_is_not_applied_behind_the_screen") {
        return;
    }
    let Some(repo) = overlay_repo("vcs_project_media_diff_covered") else {
        return;
    };
    let cfg_base = crate::test_support::unique_tmp("vcs_project_media_diff_covered_cfg");
    let png = repo.join("pic.png");
    crate::test_support::write_solid_png(&png, 40, 20, [10, 210, 10]);
    crate::test_support::run_git(&repo, &["add", "pic.png"]);
    crate::test_support::run_git(&repo, &["commit", "-qm", "pic", "--", "pic.png"]);
    crate::test_support::write_solid_png(&png, 40, 20, [210, 10, 10]);
    let mut sim = Sim::with_config_sized(&repo, config("git", "en"), 140, 30);
    sim.app.bookmarks = crate::bookmarks::Bookmarks::with_base(cfg_base.to_path_buf(), &repo);
    sim.app.tab.projects.wait_for_result();
    sim.app.poll_projects();
    drain(&mut sim);
    let (tx, rx) = std::sync::mpsc::channel();
    sim.app.attach_media_diff_loader(tx);
    let ((), asked) = crate::test_support::count_media_diff_dispatch_calls(|| {
        sim.app.open_git_diff(&png);
        sim.draw();
    });
    assert_eq!(asked, 1, "premise: the image diff asked its worker once");
    let view = sim.app.diff_view_for_test();
    sim.key('p');
    assert!(sim.app.project_active(), "p covers the image diff");
    let res = rx
        .recv_timeout(std::time::Duration::from_secs(30))
        .expect("the media diff asked before the cover lands");
    assert!(
        !sim.app.apply_media_diff(res),
        "the media diff of the covered diff was applied behind the project screen"
    );
    drain(&mut sim);
    assert_project_screen_in_front(&sim, "after the media diff landed", &[]);
    let ((), again) = crate::test_support::count_media_diff_dispatch_calls(|| {
        press(&mut sim, KeyCode::Char('q'));
        sim.draw();
    });
    assert!(!sim.app.project_active(), "q left the project screen");
    assert_eq!(sim.app.diff_view_for_test(), view, "the diff's view came back");
    assert!(again >= 1, "leaving did not ask the media diff again");
    let res = rx
        .recv_timeout(std::time::Duration::from_secs(30))
        .expect("the media diff asked after leaving lands");
    assert!(sim.app.apply_media_diff(res), "the answer asked after leaving is applied");
}

/// fix-e6a A: an SVG diff whose worker is busy when the terminal is resized holds the new size as
/// its queued want, sent as soon as the busy answer lands. `p` covers the diff: the queued want is
/// the left screen's and goes with it, so the busy answer landing behind the project screen sends
/// nothing (and is not applied).
#[test]
fn vcs_project_queued_media_diff_is_not_sent_behind_the_screen() {
    if isolated("vcs_project_queued_media_diff_is_not_sent_behind_the_screen") {
        return;
    }
    let Some(repo) = overlay_repo("vcs_project_media_diff_queued") else {
        return;
    };
    let cfg_base = crate::test_support::unique_tmp("vcs_project_media_diff_queued_cfg");
    let svg = |fill: &str| {
        format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"80\" height=\"40\">\
             <rect width=\"80\" height=\"40\" fill=\"{fill}\"/></svg>\n"
        )
    };
    let path = overlay_tracked_change(
        &repo,
        "pic.svg",
        svg("#10d210").as_bytes(),
        svg("#d21010").as_bytes(),
    );
    // With a picker: the raster box of an SVG diff is the drawn area in pixels
    // (`media_diff_raster_px`). Without one the box is the fixed placeholder (64, 64), so no
    // resize would ever be another want.
    let mut sim = Sim::with_config_sized(&repo, config("git", "en"), 140, 30).with_picker();
    sim.app.bookmarks = crate::bookmarks::Bookmarks::with_base(cfg_base.to_path_buf(), &repo);
    sim.app.tab.projects.wait_for_result();
    sim.app.poll_projects();
    drain(&mut sim);
    let (tx, rx) = std::sync::mpsc::channel();
    sim.app.attach_media_diff_loader(tx);
    sim.app.open_git_diff(&path);
    sim.draw();
    assert_eq!(
        sim.app.media_diff_busy_and_queued_for_test(),
        (true, false),
        "premise: the SVG diff is on the worker"
    );
    // Another size: an SVG is drawn to the size of its area, so this is another want, and the
    // busy worker keeps it queued.
    sim.resize(120, 26);
    assert_eq!(
        sim.app.media_diff_busy_and_queued_for_test(),
        (true, true),
        "premise: the new size waits for the busy worker"
    );
    sim.key('p');
    assert!(sim.app.project_active(), "p covers the SVG diff");
    let res = rx
        .recv_timeout(std::time::Duration::from_secs(30))
        .expect("the busy media diff lands");
    let (applied, sent) =
        crate::test_support::count_media_diff_dispatch_calls(|| sim.app.apply_media_diff(res));
    assert!(!applied, "the busy media diff was applied behind the project screen");
    assert_eq!(sent, 0, "the queued want of the covered diff was sent behind the project screen");
    drain(&mut sim);
    assert_project_screen_in_front(&sim, "after the busy media diff landed", &[]);
}

/// fix-e6a A: a Markdown document embedding an animated GIF plays it on the run loop's timer
/// (`animation_timers_due`). While a project screen covers the document the animation does not
/// advance and asks no wake-up (`md_gif_poll_timeout`); leaving the project plays it again.
#[test]
fn vcs_project_embedded_gif_stops_while_covered_and_plays_after_leaving() {
    if isolated("vcs_project_embedded_gif_stops_while_covered_and_plays_after_leaving") {
        return;
    }
    let Some(repo) = overlay_repo("vcs_project_embedded_gif_covered") else {
        return;
    };
    let cfg_base = crate::test_support::unique_tmp("vcs_project_embedded_gif_covered_cfg");
    write_animated_gif(&repo.join("anim.gif"), &[[220, 20, 20], [20, 220, 20], [20, 20, 220]]);
    std::fs::write(repo.join("anim.md"), "before\n\n![anim](anim.gif)\n\nafter\n").unwrap();
    let mut sim = Sim::with_config_sized(&repo, config("git", "en"), 140, 30).with_media();
    sim.app.bookmarks = crate::bookmarks::Bookmarks::with_base(cfg_base.to_path_buf(), &repo);
    sim.app.tab.projects.wait_for_result();
    sim.app.poll_projects();
    drain(&mut sim);
    sim.select("anim.md");
    sim.enter();
    sim.drain_md_images();
    sim.drain_md_encodes();
    // The run loop's sequence: the first tick starts the frame timer, a due frame advances.
    let plays = |sim: &mut Sim| {
        let _ = crate::animation_timers_due(&mut sim.app, std::time::Instant::now());
        sim.app.age_md_gif_frames_for_test();
        crate::animation_timers_due(&mut sim.app, std::time::Instant::now())
    };
    assert!(sim.app.md_gif_poll_timeout().is_some(), "premise: the GIF asks for wake-ups");
    assert!(plays(&mut sim), "premise: the embedded GIF plays");
    sim.key('p');
    drain(&mut sim);
    assert!(sim.app.project_active(), "p covers the document");
    assert_eq!(
        sim.app.md_gif_poll_timeout(),
        None,
        "the covered GIF still wakes the run loop"
    );
    assert!(!plays(&mut sim), "the covered GIF advanced behind the project screen");
    assert!(!plays(&mut sim), "the covered GIF advanced behind the project screen");
    press(&mut sim, KeyCode::Char('q'));
    assert!(!sim.app.project_active(), "q left the project screen");
    sim.drain_md_images();
    sim.drain_md_encodes();
    assert!(sim.app.md_gif_poll_timeout().is_some(), "the GIF asks no wake-up after leaving");
    assert!(plays(&mut sim), "the GIF does not play again after leaving");
}

/// fix-e6a A: a `/` filter confirmed, `p` over it, then a tab switch away and back. The return to
/// the covered tab is a tab switch (`put_tab_back`, then `refresh_fs_after_tab_switch`), and the
/// tab's screen is covered: the refresh at its end is routed as a watch event while covered, so no
/// filter-pool scan is asked for behind the project screen and the list stays as it was covered.
/// Leaving the project asks the scan of the return, which fills the list in.
#[test]
fn vcs_project_tab_switch_back_to_a_covered_filter_asks_no_scan_behind_the_screen() {
    if isolated("vcs_project_tab_switch_back_to_a_covered_filter_asks_no_scan_behind_the_screen") {
        return;
    }
    let Some(repo) = overlay_repo("vcs_project_filter_pool_tab_switch") else {
        return;
    };
    let cfg_base = crate::test_support::unique_tmp("vcs_project_filter_pool_tab_switch_cfg");
    std::fs::write(repo.join("deep_root1.txt"), "r\n").unwrap();
    std::fs::write(repo.join("deep_root2.txt"), "r\n").unwrap();
    for i in 0..6 {
        let sub = repo.join(format!("pkg{i}"));
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join(format!("deep{i}.csv")), "a,b\n").unwrap();
    }
    crate::app::set_filter_scan_budget(Some(Some(std::time::Duration::ZERO)));
    let mut sim = Sim::with_config_sized(&repo, config("git", "en"), 140, 30);
    sim.app.bookmarks = crate::bookmarks::Bookmarks::with_base(cfg_base.to_path_buf(), &repo);
    sim.app.tab.projects.wait_for_result();
    sim.app.poll_projects();
    drain(&mut sim);
    let (tx, rx) = std::sync::mpsc::channel();
    sim.app.attach_filter_pool_loader(tx);
    let ((), asked) = crate::test_support::count_filter_pool_asks(|| {
        sim.key('/');
        sim.keys("deep");
        sim.enter();
        drain(&mut sim);
    });
    assert!(asked >= 1, "premise: the filter asked its scan");
    assert!(sim.app.filter_query().is_some(), "the filter is on");
    sim.select("deep_root2.txt");
    let covered = overlay_tree_names(&sim);
    let covered_cursor = overlay_cursor_file(&sim);
    assert!(
        !covered.iter().any(|n| n.ends_with(".csv")),
        "premise: the scan is not on the worker: {covered:?}"
    );
    sim.key('p');
    drain(&mut sim);
    assert!(sim.app.project_active(), "p covers the filtered list");
    sim.app.tab_new().unwrap();
    drain(&mut sim);
    assert_eq!(sim.app.active_tab_index(), 1, "premise: on the new tab");
    let ((), behind) = crate::test_support::count_filter_pool_asks(|| {
        sim.app.tab_cycle(-1);
        drain(&mut sim);
    });
    assert_eq!(sim.app.active_tab_index(), 0, "premise: back on the covered tab");
    assert_project_screen_in_front(&sim, "back on the covered tab", &[]);
    assert_eq!(behind, 0, "a filter-pool scan was asked for behind the project screen");
    assert_eq!(overlay_tree_names(&sim), covered, "the covered list changed behind the screen");
    // The scan asked before the cover lands while covered: it changes nothing.
    let res = rx
        .recv_timeout(std::time::Duration::from_secs(30))
        .expect("the scan asked before the cover lands");
    let ((applied, asked_on_landing), _) = (
        crate::test_support::count_filter_pool_asks(|| sim.app.apply_filter_pool(res)),
        (),
    );
    assert_eq!(asked_on_landing, 0, "the landing asked a scan behind the project screen");
    drain(&mut sim);
    assert_eq!(
        overlay_tree_names(&sim),
        covered,
        "the scan asked for the covered list changed it behind the screen (applied={applied})"
    );
    let ((), on_return) = crate::test_support::count_filter_pool_asks(|| {
        press(&mut sim, KeyCode::Char('q'));
    });
    assert!(!sim.app.project_active(), "q left the project screen");
    assert!(on_return >= 1, "the return asked no scan of its own");
    sim.draw();
    assert_eq!(overlay_cursor_file(&sim), covered_cursor, "the cursor left its file");
    let res = rx
        .recv_timeout(std::time::Duration::from_secs(30))
        .expect("the return's scan lands");
    assert!(sim.app.apply_filter_pool(res), "the return's scan is applied");
    sim.draw();
    let names = overlay_tree_names(&sim);
    for i in 0..6 {
        assert!(
            names.iter().any(|n| *n == format!("deep{i}.csv")),
            "the return's scan did not fill the list in: {names:?}"
        );
    }
    assert_eq!(overlay_cursor_file(&sim), covered_cursor, "the cursor left its file");
    crate::app::set_filter_scan_budget(None);
}
