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

/// The variants of `Action`, read from the enum's own source (so a new variant is a new name
/// here). Variants behind `feature = "git"` count only on a build that has it.
fn action_variants_in_source() -> Vec<String> {
    let src = include_str!("../keymap.rs");
    let start = src.find("pub enum Action {").expect("enum Action");
    let body = &src[start..];
    let end = body.find("\n}\n").expect("end of enum Action");
    let mut out = Vec::new();
    let mut gated_by_git = false;
    for line in body[..end].lines().skip(1) {
        if line.starts_with("    #[cfg(feature = \"git\")]") {
            gated_by_git = true;
            continue;
        }
        assert!(
            !line.starts_with("    #[cfg("),
            "a cfg other than the git feature on a variant: classify it here: {line}"
        );
        let Some(first) = line.strip_prefix("    ").and_then(|r| r.chars().next()) else {
            continue;
        };
        if !first.is_ascii_uppercase() {
            continue;
        }
        let name: String = line.trim_start().chars().take_while(|c| c.is_alphanumeric()).collect();
        if !gated_by_git || cfg!(feature = "git") {
            out.push(name);
        }
        gated_by_git = false;
    }
    out
}

fn variant_of(a: crate::keymap::Action) -> String {
    format!("{a:?}").chars().take_while(|c| c.is_alphanumeric()).collect()
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
