//! Ports of the old-hub e2e tests that had no new-path test: every value below is written out and
//! driven through the host's `p` entry (the vcs plugin on the project canvas). Nothing here opens
//! the old hub or compares against it. Fixtures and helpers are the ones of `vcs_write_port`.
use super::*;

fn jj_bookmark_fixture(name: &str, lang: &str, read_only: bool, colocated: bool) -> Option<W> {
    if !crate::vcs::jj::available() {
        return None;
    }
    let dir = sandbox(name).into_canonical().unwrap();
    let root = dir.join("repo");
    seed_jj_write_repository(&root, colocated);
    super::super::vcs_changes::add_jj_bookmarks(&root);
    Some(open_repo(
        dir, root, "jj", name, lang, "relative", read_only, true,
    ))
}

fn git_status_porcelain(root: &std::path::Path) -> String {
    let out = std::process::Command::new("git")
        .current_dir(root)
        .args(["status", "--porcelain"])
        .output()
        .unwrap();
    assert!(out.status.success(), "git status --porcelain: {out:?}");
    String::from_utf8(out.stdout).unwrap()
}

// ---- old newkeys e2e_e_o_and_ctrl_t_act_on_the_row_of_the_changes_list and
// e2e_a_deleted_row_offers_neither_e_nor_o ----
// The launcher is replaced by a recorder: no process starts.

type Launches = std::sync::Arc<std::sync::Mutex<Vec<crate::app::office_open::Attempt>>>;

fn record_launches(w: &mut W) -> Launches {
    use crate::app::office_open::Attempt;
    let log: Launches = Default::default();
    let l = log.clone();
    w.sim
        .app
        .set_office_runner(std::sync::Arc::new(move |a: &Attempt| {
            l.lock().unwrap().push(a.clone());
            Ok(Some(0))
        }));
    log
}

#[test]
fn vcs_hub_e2e_e_o_and_ctrl_t_act_on_the_row_of_the_changes_list() {
    let mut w = W::git("vcs_hub_e2e_e_o_ctrl_t", true, "en", "relative", false);
    let log = record_launches(&mut w);
    w.select("a-modified");
    let ctx = w.sim.app.key_context().unwrap();
    assert!(
        crate::ops::action_enabled(&w.sim.app, ctx, crate::keymap::Action::RequestEdit),
        "e is offered on the modified row"
    );
    let row = w.root.join("a-modified");
    w.key('e');
    assert_eq!(w.sim.app.take_pending_edit(), Some((row.clone(), None)));
    w.key('o');
    assert_eq!(log.lock().unwrap().len(), 1);
    assert_eq!(
        log.lock().unwrap()[0].args,
        vec![row.to_string_lossy().into_owned()]
    );
    // Ctrl-t is enabled and its hints are shown (the canvas's own controls and the `?` help; the
    // changes screen's drawn footer has no `C-t` token on any row), then it opens the row in a
    // new tab and leaves the list in this one.
    assert!(
        crate::ops::action_enabled(&w.sim.app, ctx, crate::keymap::Action::OpenInNewTab),
        "Ctrl-t is enabled on the modified row"
    );
    assert!(
        w.sim.app.project_controls().unwrap().contains("Ctrl-t"),
        "the canvas's controls offer Ctrl-t"
    );
    assert!(
        crate::ops::help::sections(&w.sim.app)
            .iter()
            .flat_map(|s| &s.rows)
            .any(|(keys, _)| keys.contains("Ctrl-t")),
        "the `?` help names Ctrl-t"
    );
    assert_eq!(w.sim.app.tab_count(), 1);
    w.sim.ctrl('t');
    assert_eq!(w.sim.app.tab_count(), 2);
    assert_eq!(w.sim.app.tab.mode, Mode::Preview);
    assert_eq!(w.sim.app.tab.preview_path.as_deref(), Some(row.as_path()));
    w.sim.key('[');
    assert!(
        w.sim.app.project_active(),
        "the first tab still shows the list"
    );
    assert!(w.screen_is("changes"));
}

#[test]
fn vcs_hub_e2e_a_deleted_row_offers_neither_e_nor_o() {
    let mut w = W::git("vcs_hub_e2e_deleted_row", true, "en", "relative", false);
    let log = record_launches(&mut w);
    w.select("c-deleted");
    let ctx = w.sim.app.key_context().unwrap();
    assert!(
        !crate::ops::action_enabled(&w.sim.app, ctx, crate::keymap::Action::RequestEdit),
        "a deleted row offers no edit"
    );
    assert!(
        !crate::ops::action_enabled(&w.sim.app, ctx, crate::keymap::Action::OpenExternal),
        "a deleted row offers no open"
    );
    // `Ctrl-t` follows the same owner decision `e`/`o` read (decided: like the old hub's
    // `OpenInNewTab` gate at newkeys.rs:1165): on the missing file's row it is not enabled, no
    // Ctrl-t hint is drawn or offered, and pressing it opens nothing.
    assert!(
        !crate::ops::action_enabled(&w.sim.app, ctx, crate::keymap::Action::OpenInNewTab),
        "a deleted row offers no Ctrl-t"
    );
    assert!(
        !w.sim.app.project_controls().unwrap().contains("Ctrl-t"),
        "the canvas's controls offer no Ctrl-t on a deleted row"
    );
    assert!(
        !crate::ops::help::sections(&w.sim.app)
            .iter()
            .flat_map(|s| &s.rows)
            .any(|(keys, _)| keys.contains("Ctrl-t")),
        "the `?` help has no Ctrl-t row on a deleted row"
    );
    w.sim.dont_see("C-t");
    w.key('e');
    assert!(w.sim.app.take_pending_edit().is_none());
    w.key('o');
    assert!(log.lock().unwrap().is_empty());
    w.sim.ctrl('t');
    assert_eq!(w.sim.app.tab_count(), 1, "Ctrl-t opens nothing");
    assert!(w.screen_is("changes"));
}

// ---- old newkeys e2e_p_and_bang_work_from_the_tree_and_every_preview ----
// `p` opens the project's changes screen from the tree and from every kind of preview and `q`
// comes back to where `p` was pressed; `!` queues the project's tool from the same places. The
// entry is the host's `p` alone: opening must not reach the old hub.

#[test]
fn vcs_hub_e2e_p_and_bang_work_from_the_tree_and_every_preview() {
    let dir = sandbox("vcs_hub_e2e_p_bang").into_canonical().unwrap();
    seed_repo(&dir);
    std::fs::write(dir.join("notes.md"), "# T\n\ntext\n").unwrap();
    std::fs::write(dir.join("data.csv"), "a,b\n1,2\n").unwrap();
    write_solid_png(&dir.join("pic.png"), 8, 8, [1, 2, 3]);
    let cfg = super::super::vcs_changes::plugin_config("git", "vcs_hub_e2e_p_bang", "en");
    for (name, ctx) in [
        ("", Context::Tree),
        ("a.rs", Context::Text),
        ("notes.md", Context::Document),
        ("data.csv", Context::Table),
        ("pic.png", Context::Image),
        ("a.rs:diff", Context::Diff),
    ] {
        let mut s = Sim::with_config(&dir, cfg.clone()).with_picker();
        s.app.tab.projects.wait_for_result();
        s.app.poll_projects();
        s.app.ensure_git_status_now();
        s.draw();
        match name {
            "" => {}
            "a.rs:diff" => {
                s.select("a.rs");
                s.key('d');
            }
            f => {
                s.select(f);
                s.enter();
            }
        }
        assert_eq!(s.app.key_context(), Some(ctx), "{name}");
        assert!(
            crate::ops::action_enabled(&s.app, ctx, crate::keymap::Action::OpenGitView),
            "{name}: p is offered"
        );
        assert!(
            crate::ops::action_enabled(&s.app, ctx, crate::keymap::Action::GitLaunchTool),
            "{name}: ! is offered"
        );
        let mode = s.app.tab.mode;
        s.key('p');
        drain(&mut s);
        assert!(
            s.app.project_active(),
            "{name}: p opens the project's screen"
        );
        assert_eq!(
            s.app.project_frame().unwrap().key.screen,
            "changes",
            "{name}"
        );
        s.key('q');
        drain(&mut s);
        assert!(!s.app.project_active(), "{name}");
        assert_eq!(
            s.app.tab.mode, mode,
            "{name}: q comes back to where p was pressed"
        );
        assert_eq!(s.app.key_context(), Some(ctx), "{name}");
        s.key('!');
        assert!(s.app.take_launch_git_tool(), "{name}: ! queues the tool");
    }
}

// ---- old newkeys e2e_branch_and_worktree_filters_are_the_same_component ----
// The branch and worktree lists keep their filter through the same project list filter: `/`
// opens the input on each screen, the typed query narrows the view, Esc drops it and the screen
// stays.

#[test]
fn vcs_hub_e2e_branch_and_worktree_filters_are_the_same_component() {
    let mut w = branches("vcs_hub_e2e_filter_component", "en", false);
    assert!(w.screen_is("branches"));
    w.key('/');
    assert!(w.sim.app.project_frame().unwrap().filtering);
    w.type_text("alp");
    let frame = w.sim.app.project_frame().unwrap();
    assert!(frame.filtering);
    assert_eq!(frame.query, "alp");
    assert_eq!(
        branch_values(&w.sim),
        vec![("branch".to_string(), "alpha".to_string())]
    );
    w.press(KeyCode::Esc);
    let frame = w.sim.app.project_frame().unwrap();
    assert!(!frame.filtering);
    assert_eq!(frame.query, "");
    assert_eq!(branch_values(&w.sim).len(), 3);
    w.key('w');
    assert!(w.screen_is("worktrees"));
    w.key('/');
    assert!(w.sim.app.project_frame().unwrap().filtering);
    w.type_text("zz");
    assert_eq!(w.sim.app.project_frame().unwrap().query, "zz");
    w.press(KeyCode::Esc);
    let frame = w.sim.app.project_frame().unwrap();
    assert!(!frame.filtering);
    assert_eq!(frame.query, "");
    assert!(w.screen_is("worktrees"));
}

// ---- old default_keys e2e_s_stages_in_a_git_hub_and_syncs_in_a_jj_hub_and_h_l_open_and_close ----
// (`s` stages and the jj sync are held by the write ports; this is the l / q / h part)

#[test]
fn vcs_hub_e2e_l_opens_the_diff_q_returns_to_the_list_and_h_leaves() {
    let mut w = W::git("vcs_hub_e2e_l_q_h", true, "en", "relative", false);
    w.select("a-modified");
    w.key('l');
    assert_eq!(w.sim.app.tab.mode, Mode::Preview, "l opens the row's diff");
    assert!(!w.sim.app.project_active());
    w.sim.key('q');
    assert!(w.sim.app.project_active(), "q returns to the list");
    assert!(w.screen_is("changes"));
    w.key('h');
    assert!(
        !w.sim.app.project_active(),
        "h goes back out of the project"
    );
    assert_eq!(w.sim.app.tab.mode, Mode::Tree);
}

// ---- old default_keys e2e_branch_list_y_menu_copies_the_name_and_c_creates ----

#[test]
fn vcs_hub_e2e_branch_list_y_menu_copies_the_name_and_c_creates() {
    let mut w = branches("vcs_hub_e2e_branch_y", "en", false);
    crate::test_support::clear_test_clipboard();
    w.key('y');
    assert_eq!(
        w.sim.app.pending_leader,
        Some(crate::keymap::LeaderId::Copy),
        "y opens the copy menu instead of copying at once"
    );
    assert_eq!(crate::test_support::get_test_clipboard(), None);
    w.sim.see("n:branch name");
    w.key('n');
    assert_eq!(w.sim.app.pending_leader, None);
    assert_eq!(
        crate::test_support::get_test_clipboard().as_deref(),
        Some("main"),
        "the menu's item copied the branch name"
    );
    assert_eq!(w.flash().as_deref(), Some("copied: main"));
    // `n` opens nothing; `c` opens the new-branch input.
    w.key('n');
    w.no_dialog();
    w.key('c');
    assert!(w.sim.app.project_dialog.is_some(), "c asks for the name");
    w.press(KeyCode::Esc);
    w.no_dialog();
    // Decided difference: `h` is `q`, so on a root screen it returns to where the project was
    // opened from (the old hub returned to its changes list).
    w.key('h');
    assert!(!w.sim.app.project_active());
    assert_eq!(w.sim.app.tab.mode, Mode::Tree);
}

// ---- old default_keys e2e_worktree_list_c_creates_and_n_does_not ----

#[test]
fn vcs_hub_e2e_worktree_list_c_creates_and_n_does_not() {
    let mut w = W::git("vcs_hub_e2e_worktree_keys", true, "en", "relative", false);
    w.key('w');
    assert!(w.screen_is("worktrees"));
    w.key('n');
    w.no_dialog();
    w.key('c');
    assert!(
        w.sim.app.project_dialog.is_some(),
        "c asks for the new worktree's branch"
    );
    w.press(KeyCode::Esc);
    w.no_dialog();
    // Decided difference: `h` is `q`; a root screen returns to where the project was opened from.
    w.key('h');
    assert!(!w.sim.app.project_active());
    assert_eq!(w.sim.app.tab.mode, Mode::Tree);
}

// ---- old e2e_git_branches_view_lists_branches ----

#[test]
fn vcs_hub_e2e_branches_screen_lists_branches_marks_the_current_one_and_q_leaves() {
    let mut w = branches("vcs_hub_e2e_branches_view", "en", false);
    // Screen title, every branch name and the current-branch marker are drawn.
    w.sim.see("Git branches");
    w.sim.see("alpha");
    w.sim.see("main");
    w.sim.see("* main");
    assert_eq!(
        branch_values(&w.sim),
        vec![
            ("branch".to_string(), "alpha".to_string()),
            ("current".to_string(), "main".to_string()),
            ("branch".to_string(), "日本語".to_string()),
        ]
    );
    // `q` leaves to where the project was opened from (the tree), not to a changes hub.
    w.key('q');
    assert!(!w.sim.app.project_active());
    assert_eq!(w.sim.app.tab.mode, Mode::Tree);
}

// ---- old e2e_git_branches_filter_narrows_and_clears ----

#[test]
fn vcs_hub_e2e_branches_filter_narrows_to_the_query_and_clears_on_escape() {
    let mut w = branches("vcs_hub_e2e_branches_filter", "en", false);
    assert_eq!(branch_values(&w.sim).len(), 3);
    w.key('/');
    assert!(w.sim.app.project_frame().unwrap().filtering);
    w.type_text("alp");
    let frame = w.sim.app.project_frame().unwrap();
    assert!(frame.filtering);
    assert_eq!(frame.query, "alp");
    assert_eq!(
        branch_values(&w.sim),
        vec![("branch".to_string(), "alpha".to_string())]
    );
    w.sim.see("alpha");
    w.press(KeyCode::Esc);
    let frame = w.sim.app.project_frame().unwrap();
    assert!(!frame.filtering);
    assert_eq!(frame.query, "");
    assert_eq!(branch_values(&w.sim).len(), 3);
    // A query that matches nothing narrows to the single empty-state row.
    w.key('/');
    w.type_text("zzz");
    assert_eq!(
        branch_values(&w.sim),
        vec![("empty".to_string(), String::new())]
    );
    w.press(KeyCode::Esc);
    assert_eq!(branch_values(&w.sim).len(), 3);
    // ---- old git_branch_filter_commit_and_backspace ----
    // "a" narrows to alpha+main, backspacing off the "l" widens it back, and Enter commits the
    // query (leaves input mode) while keeping the list narrowed.
    let narrowed = |sim: &Sim| {
        let mut v: Vec<String> = branch_values(sim).iter().map(|(_, v)| v.clone()).collect();
        v.sort();
        v
    };
    w.key('/');
    w.type_text("a");
    assert_eq!(narrowed(&w.sim), ["alpha", "main"]);
    w.type_text("l");
    assert_eq!(
        branch_values(&w.sim),
        vec![("branch".to_string(), "alpha".to_string())]
    );
    w.press(KeyCode::Backspace);
    assert_eq!(
        narrowed(&w.sim),
        ["alpha", "main"],
        "backspace widens the query"
    );
    w.press(KeyCode::Enter);
    let frame = w.sim.app.project_frame().unwrap();
    assert!(!frame.filtering, "Enter leaves input mode");
    assert_eq!(frame.query, "a", "Enter keeps the query");
    assert_eq!(branch_values(&w.sim).len(), 2);
}

// ---- old e2e_enter_on_a_git_branch_list_still_checks_out (footer and Enter) ----

#[test]
fn vcs_hub_e2e_git_branches_footer_offers_checkout_and_enter_checks_out() {
    let mut w = branches("vcs_hub_e2e_git_enter", "en", false);
    w.select_branch("alpha");
    w.sim.see("l/Enter:checkout");
    w.press(KeyCode::Enter);
    assert_eq!(current_branch(&w.root).as_deref(), Some("alpha"));
    assert!(w.screen_is("branches"));
}

// ---- old e2e_enter_on_a_jj_bookmark_list_is_refused_like_every_other_write ----
//
// Decided difference (docs/FEATURE-PLUGIN-ARCHITECTURE.md, "jj のブックマーク画面は読むだけ"): the
// jj plugin declares no checkout, create or delete, so the old read-only flash is not shown; the
// keys do nothing and no job starts.

#[test]
fn vcs_hub_e2e_jj_bookmarks_enter_and_write_keys_dispatch_nothing() {
    let Some(mut w) = jj_bookmark_fixture("vcs_hub_e2e_jj_bookmarks", "en", false, false) else {
        return;
    };
    w.key('b');
    assert!(w.screen_is("branches"));
    let rows = branch_values(&w.sim);
    assert!(rows.iter().any(|(_, v)| v == "alpha"), "{rows:?}");
    // The footer of the same list does not offer a checkout.
    w.sim.see("/:search");
    w.sim.dont_see("checkout");
    let state = write_jj_state(&w.root);
    for code in [
        KeyCode::Enter,
        KeyCode::Char('l'),
        KeyCode::Char('c'),
        KeyCode::Char('d'),
    ] {
        w.press(code);
        assert_eq!(w.flash(), None, "{code:?}");
        w.no_dialog();
        assert_eq!(w.acts(), Vec::<String>::new(), "{code:?}");
        assert!(!w.sim.app.project_write_running(), "{code:?}");
        assert!(w.screen_is("branches"), "{code:?}");
        assert_eq!(branch_values(&w.sim), rows, "{code:?}");
        assert_eq!(write_jj_state(&w.root), state, "{code:?}");
    }
    w.key('q');
    assert!(!w.sim.app.project_active());
    assert_eq!(w.sim.app.tab.mode, Mode::Tree);
}

// ---- old e2e_jj_write_actions_are_gated_by_the_read_only_backend (changes hub part) ----
//
// Decided difference (the jj changes screen declares no stage family): `u S U x c` are not keys of
// the screen, so none of them flashes, opens a dialog or runs an act.

#[test]
fn vcs_hub_e2e_jj_changes_stage_family_keys_do_nothing_and_leave_the_repository_alone() {
    for colocated in [false, true] {
        let Some(mut w) = W::jj("vcs_hub_e2e_jj_changes", "en", true, false, colocated) else {
            return;
        };
        // Sanity (the old `caps().write == false` check): the jj screen itself advertises no stage,
        // unstage, discard or commit key in its help (its `s` is the sync, which asks first and is
        // not one of the keys pressed below), so those keys have nothing to dispatch.
        assert_eq!(
            write_help(&w.sim.app),
            pairs(&[
                ("d", "diff of everything against @-"),
                ("s", "let jj snapshot the working copy (confirm)")
            ]),
            "{colocated}: the jj changes help offers no stage, unstage, discard or commit row"
        );
        let state = write_jj_state(&w.root);
        let git_before = colocated.then(|| git_status_porcelain(&w.root));
        if let Some(before) = &git_before {
            assert!(
                !before.is_empty(),
                "the colocated fixture must hold real uncommitted git state"
            );
        }
        assert!(w.screen_is("changes"));
        for key in ['u', 'S', 'U', 'x', 'c'] {
            w.key(key);
            assert_eq!(w.flash(), None, "{colocated}/{key}");
            w.no_dialog();
            assert_eq!(w.acts(), Vec::<String>::new(), "{colocated}/{key}");
            assert!(w.screen_is("changes"), "{colocated}/{key}");
            assert!(w.sim.app.project_active(), "{colocated}/{key}");
        }
        assert_eq!(write_jj_state(&w.root), state, "{colocated}");
        if let Some(before) = git_before {
            // Byte for byte: the real `.git` next to `.jj` is untouched.
            assert_eq!(git_status_porcelain(&w.root), before);
        }
        // Both `q` return to the tree.
        w.key('q');
        assert_eq!(w.sim.app.tab.mode, Mode::Tree);
    }
}

// ---- old e2e_jj_write_gate_holds_in_a_colocated_repository_pinned_to_jj ----
//
// The old test pinned `[external] vcs = "jj"` and proved no git write could reach the real `.git`.
// On the project path a colocated repository asks (git or jj) and `[external] vcs` does not narrow
// the menu; the jj choice is the one the old pin selected, and it must leave `.git` untouched.

#[test]
fn vcs_hub_e2e_colocated_repository_pinned_to_jj_offers_both_and_the_jj_choice_never_writes() {
    if !crate::vcs::jj::available() {
        return;
    }
    let dir = sandbox("vcs_hub_e2e_colocated_pinned")
        .into_canonical()
        .unwrap();
    let root = dir.join("repo");
    seed_jj_write_repository(&root, true);
    super::super::vcs_changes::add_jj_bookmarks(&root);
    let before = git_status_porcelain(&root);
    assert!(
        !before.is_empty(),
        "the fixture must leave real uncommitted git state, or a broken gate has nothing to prove itself against"
    );
    let mut cfg = Config::default();
    cfg.external.vcs = "jj".into();
    let mut sim = Sim::with_config_sized(&root, cfg, 200, 24);
    sim.app.tab.projects.wait_for_result();
    sim.app.poll_projects();
    sim.key('p');
    drain(&mut sim);
    let crate::app::project_screens::ProjectDialog::Projects { projects, .. } = sim
        .app
        .project_dialog
        .as_ref()
        .expect("the project chooser")
    else {
        panic!("project chooser required")
    };
    assert_eq!(
        projects
            .iter()
            .map(|p| p.plugin.as_str())
            .collect::<Vec<_>>(),
        ["git", "jj"]
    );
    sim.key('j');
    sim.enter();
    drain(&mut sim);
    assert!(sim.app.project_active());
    assert_eq!(
        sim.app
            .tab
            .project_navigation
            .as_ref()
            .unwrap()
            .project
            .plugin,
        "jj"
    );
    for key in ['u', 'S', 'U', 'x', 'c'] {
        sim.key(key);
        drain(&mut sim);
        assert_eq!(sim.app.flash, None, "{key}");
        assert!(!sim.app.is_dialog(), "{key}");
        assert!(sim.app.project_dialog.is_none(), "{key}");
        assert!(!sim.app.project_write_running(), "{key}");
    }
    // The colocated repository's auto-imported bookmark opens the list, and its keys do nothing.
    sim.key('b');
    drain(&mut sim);
    assert_eq!(sim.app.project_frame().unwrap().key.screen, "branches");
    for code in [
        KeyCode::Enter,
        KeyCode::Char('l'),
        KeyCode::Char('c'),
        KeyCode::Char('d'),
    ] {
        sim.press(code, KeyModifiers::NONE);
        drain(&mut sim);
        assert_eq!(sim.app.flash, None, "{code:?}");
        assert!(!sim.app.is_dialog(), "{code:?}");
        assert!(sim.app.project_dialog.is_none(), "{code:?}");
        assert!(!sim.app.project_write_running(), "{code:?}");
        assert_eq!(
            sim.app.project_frame().unwrap().key.screen,
            "branches",
            "{code:?}"
        );
    }
    assert_eq!(
        git_status_porcelain(&root),
        before,
        "the real .git is untouched, byte for byte"
    );
}

// ---- old e2e_jj_write_actions_are_gated_by_the_read_only_backend (tree diff part) ----
// The full-screen diff opened from the tree is not part of the old hub; the discard key is still
// refused for a jj repository there.

#[test]
fn vcs_hub_e2e_jj_tree_diff_discard_is_refused_and_stays_on_the_diff() {
    let Some(dir) = jj_seed_repo("vcs_hub_e2e_jj_tree_diff") else {
        return;
    };
    let root = canon(&dir);
    let mut s = Sim::new(&root);
    s.select("a.txt");
    s.key('d');
    assert!(s.app.is_git_diff_preview(), "d opens the read-only diff");
    s.key('x');
    assert!(
        s.app
            .flash
            .as_deref()
            .is_some_and(|f| f.contains(crate::i18n::tr(s.app.lang, crate::i18n::Msg::VcsReadOnly))),
        "x must be refused with the read-only flash, got {:?}",
        s.app.flash
    );
    assert!(
        s.app.is_git_diff_preview(),
        "a refused discard keeps the diff"
    );
}

// ---- A discard confirmed on the diff the tree's `d` opened ----
// The old hub was the place that discard returned to; the project's changes screen replaced it.
// After a successful discard the tab shows the changes screen as `p` from the tree opens it, and
// no state of the old hub is raised.

/// A git repository session whose git writes run on the real background path.
fn tree_diff_session(name: &str) -> (Sim, std::path::PathBuf, crate::test_support::TmpDir) {
    let dir = super::super::vcs_changes::git_repo(name, true);
    let root = dir.to_path_buf();
    let mut sim = Sim::new(&root).with_async_git_ops();
    sim.app.tab.projects.wait_for_result();
    sim.app.poll_projects();
    (sim, root, dir)
}

#[test]
fn tree_diff_discard_opens_the_project_changes_screen_like_p_and_q_returns_to_the_tree() {
    let (mut sim, root, _dir) = tree_diff_session("vcs_hub_e2e_tree_diff_discard");
    sim.select("b-unstaged.txt");
    sim.key('d');
    assert!(sim.app.is_git_diff_preview(), "d opens the file diff");
    sim.key('x');
    assert!(sim.app.is_dialog(), "x asks for confirmation");
    sim.key('y');
    sim.drain_git_ops();
    drain(&mut sim);
    assert_eq!(
        std::fs::read(root.join("b-unstaged.txt")).unwrap(),
        b"base\n",
        "the file is reverted on disk"
    );
    assert!(
        sim.app.project_active(),
        "the project changes screen is shown, flash {:?}",
        sim.app.flash
    );
    assert_eq!(sim.app.project_frame().unwrap().key.screen, "changes");
    assert_eq!(
        sim.app
            .tab
            .project_navigation
            .as_ref()
            .unwrap()
            .project
            .plugin,
        "git",
        "the repository's own project"
    );
    assert!(
        sim.app
            .flash
            .as_deref()
            .is_some_and(|f| f.starts_with("Discarded: ") && f.ends_with("b-unstaged.txt")),
        "the Discarded flash is kept: {:?}",
        sim.app.flash
    );
    // The same entry as `p` from the tree: the list shows the remaining changes only.
    let rows = |sim: &Sim| match sim.app.project_content().unwrap().as_ref() {
        ProjectContent::List(list) => list
            .rows
            .iter()
            .map(|r| r.cells[0].plain())
            .collect::<Vec<_>>()
            .join("\n"),
        _ => panic!("the changes list is required"),
    };
    let listed = rows(&sim);
    assert!(listed.contains("a-staged.txt"), "{listed}");
    assert!(!listed.contains("b-unstaged.txt"), "{listed}");
    // `q` behaves as after `p` from the tree: back to the tree, nothing of the hub left.
    sim.key('q');
    drain(&mut sim);
    assert!(!sim.app.project_active(), "q leaves the project");
    assert_eq!(sim.app.surface(), crate::keymap::Surface::Tree);
    assert!(!sim.app.is_git_diff_preview());
    sim.app.ensure_git_status_now();
    assert!(
        sim.app
            .git_status_of(&root.join("b-unstaged.txt"))
            .is_none(),
        "the tree row has no change status any more"
    );
    assert!(
        sim.app.git_status_of(&root.join("a-staged.txt")).is_some(),
        "the other changes stay marked"
    );
}

#[test]
fn tree_diff_discard_failure_keeps_the_diff_and_shows_the_failure() {
    let (mut sim, root, _dir) = tree_diff_session("vcs_hub_e2e_tree_diff_discard_fail");
    sim.select("e-untracked.txt");
    sim.key('d');
    assert!(sim.app.is_git_diff_preview(), "d opens the untracked diff");
    sim.key('x');
    sim.key('y');
    sim.drain_git_ops();
    drain(&mut sim);
    assert!(
        sim.app
            .flash
            .as_deref()
            .is_some_and(|f| f.starts_with("failed: ")),
        "{:?}",
        sim.app.flash
    );
    assert!(sim.app.is_git_diff_preview(), "a failed discard stays");
    assert!(!sim.app.project_active());
    assert_eq!(
        std::fs::read(root.join("e-untracked.txt")).unwrap(),
        b"new\n"
    );
    // `q` still returns to the tree (the diff was opened from it).
    sim.key('q');
    assert_eq!(sim.app.surface(), crate::keymap::Surface::Tree);
}

#[test]
fn tree_diff_cancelled_discard_then_q_returns_to_the_tree_not_a_hub() {
    let (mut sim, _root, _dir) = tree_diff_session("vcs_hub_e2e_tree_diff_discard_cancel");
    sim.select("b-unstaged.txt");
    sim.key('d');
    sim.key('x');
    assert!(sim.app.is_dialog());
    sim.key('n');
    assert!(sim.app.is_git_diff_preview(), "cancel keeps the diff");
    sim.key('q');
    assert_eq!(sim.app.surface(), crate::keymap::Surface::Tree);
    assert!(!sim.app.project_active());
}

// ---- Decided differences (the user decisions: on a jj repository `w` and on a git repository
// the jj sync key `R` show nothing): old e2e_jj_worktrees_key_flashes_instead_of_opening_the_git_list
// and old e2e_jj_sync_key_on_a_git_repo_flashes_and_opens_nothing ----
//
// The old hub flashed `JjWorkspacesUnlisted` for `w` in a jj repository and `JjSyncNotJj` for a
// bound sync key in a git repository. On the new path the jj screens have no worktree screen and
// the git plugin has no jj sync operation, so both keys do nothing visible.

#[test]
fn vcs_hub_e2e_jj_worktrees_key_does_nothing_and_opens_no_worktree_screen() {
    let Some(mut w) = W::jj("vcs_hub_e2e_jj_w", "en", true, false, false) else {
        return;
    };
    w.key('w');
    assert_eq!(w.flash(), None);
    w.no_dialog();
    assert!(w.screen_is("changes"));
    assert_eq!(w.acts(), Vec::<String>::new());
}

#[test]
fn vcs_hub_e2e_git_repository_has_no_sync_key_that_flashes_or_opens_a_dialog() {
    let mut w = W::git("vcs_hub_e2e_git_r", true, "en", "relative", false);
    let before = w.status();
    w.key('R');
    assert_eq!(w.flash(), None);
    w.no_dialog();
    assert_eq!(w.acts(), Vec::<String>::new());
    assert_eq!(w.status(), before);
    assert!(w.screen_is("changes"));
}

// ---- old e2e_ui_keeps_responding_while_a_git_commit_is_blocked and
// old e2e_quit_during_a_git_write_asks_for_confirmation ----

/// A git project whose `commit` is parked inside the repository's pre-commit hook until the test
/// removes `.git/BLOCK`; the order comes from that file, never from the clock. The second half of
/// the pair is the commit subjects read BEFORE Enter starts the write, so the caller can prove the
/// parked job landed nothing while it ran.
fn blocked_commit(name: &str, message: &str) -> (W, Vec<String>) {
    let mut w = W::git(name, true, "en", "relative", false);
    install_blocking_pre_commit_hook(&w.root);
    std::fs::write(w.root.join(".git/BLOCK"), b"").unwrap();
    w.key('c');
    w.type_text(message);
    let before = commit_subjects(&w.root);
    // Direct press: `W::press` would wait for the parked job.
    w.sim.press(KeyCode::Enter, KeyModifiers::NONE);
    // git rewrites `.git/index` before it runs the hook; the hook's marker file orders every
    // later snapshot after that write.
    let marker = w.root.join(".git/HOOK_ENTERED");
    let start = std::time::Instant::now();
    while !marker.exists() {
        assert!(
            start.elapsed() < std::time::Duration::from_secs(20),
            "the pre-commit hook never started"
        );
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    (w, before)
}

fn commit_subjects(root: &std::path::Path) -> Vec<String> {
    crate::git::log(root, 200)
        .into_iter()
        .map(|c| c.summary)
        .collect()
}

#[test]
fn vcs_hub_e2e_ui_keeps_responding_while_a_real_git_commit_is_blocked() {
    let (mut w, before) = blocked_commit("vcs_hub_e2e_blocked_commit", "BLOCKEDCOMMIT");
    assert!(w.sim.app.project_write_running(), "the commit is running");
    assert!(w.sim.app.git_write_running());
    assert!(!before.contains(&"BLOCKEDCOMMIT".to_string()));
    assert_eq!(
        commit_subjects(&w.root),
        before,
        "the parked commit has not reached the log"
    );

    // Keys keep working and every one redraws.
    w.sim.key('q');
    assert!(!w.sim.app.project_active(), "the project can be left");
    let start = w.sim.app.tab.selected;
    for _ in 0..3 {
        w.sim.key('j');
    }
    assert!(w.sim.app.tab.selected > start, "the tree cursor moves");
    w.sim.key('k');
    w.sim.see("a-modified");
    assert!(w.sim.app.project_write_running(), "still running");
    assert_eq!(commit_subjects(&w.root), before, "nothing committed yet");

    // Release the hook and let the result land.
    std::fs::remove_file(w.root.join(".git/BLOCK")).unwrap();
    drain(&mut w.sim);
    assert!(!w.sim.app.project_write_running());
    assert!(commit_subjects(&w.root).contains(&"BLOCKEDCOMMIT".to_string()));
}

#[test]
fn vcs_hub_e2e_quit_during_a_real_git_commit_asks_even_with_confirm_quit_off() {
    let (mut w, _) = blocked_commit("vcs_hub_e2e_quit_commit", "WILLQUIT");
    w.sim.app.cfg.ui.confirm_quit = false;
    assert!(w.sim.app.project_write_running());
    w.sim.key('Q');
    assert!(!w.sim.quit, "a confirmation is asked, so no quit yet");
    let (_, message, _, _) = w.sim.app.dialog_view().expect("a confirmation dialog");
    assert!(
        message.contains(crate::i18n::tr(
            w.sim.app.lang,
            crate::i18n::Msg::QuitWhileGitOp
        )),
        "{message}"
    );
    // The old test also saw the warning on the drawn dialog, not only in its message.
    w.sim.see(crate::i18n::tr(
        w.sim.app.lang,
        crate::i18n::Msg::QuitWhileGitOp,
    ));
    // The warning only warns: `y` still leaves.
    w.sim.key('y');
    assert!(w.sim.quit);
    std::fs::remove_file(w.root.join(".git/BLOCK")).ok();
}

// ---- old lib.rs help_opens_in_git_view_and_shows_git_keys ----

#[test]
fn vcs_hub_e2e_question_mark_opens_help_on_a_git_project_screen_and_closes_again() {
    let mut w = W::git("vcs_hub_e2e_help", true, "en", "relative", false);
    assert!(!w.sim.app.show_help);
    w.key('?');
    assert!(
        w.sim.app.show_help,
        "? opens the help on the project screen"
    );
    // The help is the screen's own: the stage family and the commit key are listed.
    w.sim.see("stage / stage all");
    w.sim.see("commit (staged)");
    let titles: Vec<String> = crate::ops::help::sections_for(&w.sim.app, Context::Canvas)
        .iter()
        .map(|s| s.title.to_string())
        .collect();
    assert_eq!(
        titles[0], "Git changes (p)",
        "the git section leads the help"
    );
    w.key('?');
    assert!(!w.sim.app.show_help, "? closes the help again");
    assert!(w.screen_is("changes"));
    assert!(w.sim.app.project_active());
}

// ---- old lib.rs tab_keys_per_tab_git_mode_and_literal_in_filter (the git-view part) ----

#[test]
fn vcs_hub_e2e_t_on_a_git_project_screen_opens_a_plain_tab_and_the_first_tab_keeps_its_screen() {
    let mut w = W::git("vcs_hub_e2e_tab", true, "en", "relative", false);
    let tabs = w.sim.app.tab_count();
    w.key('t');
    assert_eq!(w.sim.app.tab_count(), tabs + 1, "t opens a new tab");
    assert!(
        !w.sim.app.project_active(),
        "the new tab starts without a project screen"
    );
    // Back on the first tab the project screen is still there.
    w.key('1');
    assert!(w.sim.app.project_active(), "returning restores the screen");
    assert!(w.screen_is("changes"));
}

// ---- old lib.rs tab_keys_per_tab_git_mode_and_literal_in_filter (the filter part) ----

#[test]
fn vcs_hub_e2e_filter_input_types_t_and_capital_i_literally_instead_of_opening_a_tab() {
    let dir = sandbox("vcs_hub_e2e_filter_literal");
    std::fs::write(dir.join("a.txt"), b"x").unwrap();
    let mut s = Sim::new(&canon(&dir));
    s.key('/');
    let tabs = s.app.tab_count();
    s.key('t');
    assert_eq!(s.app.tab_count(), tabs, "t is typed, not a new tab");
    assert_eq!(s.app.filter_query(), Some("t"));
    s.key('I');
    assert_eq!(s.app.tab_count(), tabs, "I is typed, not a new tab");
    assert_eq!(s.app.filter_query(), Some("tI"));
    s.esc();
    assert!(s.app.filter_query().is_none());
}

// ---- old speed_tests::git_views_and_large_diff_render_is_bounded, part (2) ----
//
// Same data as the old test (3 commits, a small committed big.txt rewritten to 1000 lines), the
// same screens (changes, log, graph, branches, the large diff) and the same ceiling. The bytes are
// counted on the UI thread: that is the host's cost of laying out and drawing each canvas screen.

#[test]
fn vcs_hub_e2e_project_views_and_large_diff_render_is_bounded() {
    use crate::test_support::run_git;
    let dir = super::super::vcs_changes::git_repo("vcs_hub_e2e_speed", false);
    let root = dir.to_path_buf();
    for c in 0..3 {
        std::fs::write(root.join(format!("f{c}.txt")), format!("v{c}\n")).unwrap();
        run_git(&root, &["add", "-A"]);
        run_git(&root, &["commit", "-qm", &format!("commit {c}")]);
    }
    std::fs::write(
        root.join("big.txt"),
        "line 0\nline 1\nline 2\nline 3\nline 4\n",
    )
    .unwrap();
    run_git(&root, &["add", "big.txt"]);
    run_git(&root, &["commit", "-qm", "add big"]);
    let mut body = String::new();
    for i in 0..1000 {
        body.push_str(&format!("LINE changed {i}\n"));
    }
    std::fs::write(root.join("big.txt"), body).unwrap();

    let mut w = open_repo(
        dir,
        root,
        "git",
        "vcs_hub_e2e_speed",
        "en",
        "relative",
        false,
        true,
    );
    let mut steps = Vec::new();
    let p1 = crate::mem_tests::allocated_by(|| {
        for _ in 0..3 {
            w.press_settled(KeyCode::Tab); // log, graph, branches
            steps.push(w.sim.app.project_frame().unwrap().key.screen.clone());
        }
    });
    let p2 = crate::mem_tests::allocated_by(|| {
        for _ in 0..8 {
            if w.screen_is("changes") {
                break;
            }
            w.press_settled(KeyCode::Tab);
        }
        steps.push(w.sim.app.project_frame().unwrap().key.screen.clone());
        w.select_settled("big.txt");
    });
    let p3 = crate::mem_tests::allocated_by(|| {
        w.press_settled(KeyCode::Enter);
    });
    let views_alloc = p1 + p2 + p3;
    // Every screen really was opened and drawn (a skipped screen would make the bound vacuous), and
    // the large diff is the full-screen preview of big.txt.
    assert_eq!(steps, ["log", "graph", "branches", "changes"]);
    assert_eq!(w.sim.app.tab.mode, Mode::Preview);
    w.sim.see("big.txt");
    w.sim.see("LINE changed 0");
    // The count is deterministic (same thread, same data; measured: three Tab presses 10.1MB, the
    // lap back to changes plus the cursor move 16.6MB, the diff open 7.4MB, 34.2MB in all). It is
    // deterministic because every press uses `press_settled`: the worker result lands before the
    // first frame after the key is drawn. With a plain `drain` the stale screen is drawn too when
    // the worker loses the race to the first poll (one extra frame, about 3MB for the diff open,
    // which tripped p3's ceiling about 1 run in 80). The old hub's bound was 60MB (1.7x); each part
    // is held to about 1.3x of what it costs, so doing the same work twice on any part (every frame
    // drawn twice included) is over its ceiling.
    assert!(
        p1 < 14_000_000,
        "three screen switches allocated too much on the UI thread: {p1}"
    );
    assert!(
        p2 < 21_500_000,
        "the lap back to the changes screen allocated too much on the UI thread: {p2}"
    );
    assert!(
        p3 < 9_500_000,
        "opening the large diff allocated too much on the UI thread: {p3}"
    );
    assert!(
        views_alloc < 45_000_000,
        "the canvas screens and the large diff allocated too much on the UI thread: {views_alloc}"
    );
    for (name, part, floor) in [
        ("p1", p1, 5_000_000),
        ("p2", p2, 8_000_000),
        ("p3", p3, 3_000_000),
    ] {
        assert!(
            part > floor,
            "the allocation counter looks dead for {name}: {part}"
        );
    }
    assert!(
        // Not vacuous: the screens cost something, so a counter that stopped counting is caught.
        views_alloc > 10_000_000,
        "the allocation counter looks dead: {views_alloc}"
    );
}

// ---- decided difference: old second_git_op_is_rejected_while_one_is_in_flight ----
// The old hub refused a second write with the `GitOpBusy` flash. On the new path a running write
// removes the other acts' hints and their keys do nothing (no flash, no dialog, no git call).

fn has_write_hints(w: &W) -> bool {
    let hints = write_tokens(&w.sim.app);
    hints.contains(&"c:commit".to_string()) && hints.iter().any(|t| t.starts_with("s/S:"))
}

#[test]
fn vcs_hub_e2e_while_a_write_runs_other_act_keys_show_no_hint_and_do_nothing() {
    let (mut w, before) = blocked_commit("vcs_hub_e2e_running_hides_acts", "RUNNINGCOMMIT");
    assert!(w.sim.app.project_write_running());
    let (status, index) = (w.status(), w.index());
    w.sim.app.flash = None;
    w.sim.draw();
    // No stage or commit hint in the footer or the help, and none drawn on the screen.
    assert_eq!(write_tokens(&w.sim.app), Vec::<String>::new());
    assert_eq!(write_help(&w.sim.app), Vec::<(String, String)>::new());
    w.sim.dont_see("stage");
    w.sim.dont_see("commit");
    for key in ['s', 'S', 'u', 'U', 'x', 'c'] {
        w.sim.key(key);
        assert_eq!(w.flash(), None, "{key}: no flash");
        w.no_dialog();
        assert!(
            w.sim.app.project_write_running(),
            "{key}: still the one write"
        );
    }
    assert_eq!(w.status(), status);
    assert_eq!(w.index(), index);
    assert_eq!(commit_subjects(&w.root), before);
    // The write finishes and the hints come back.
    std::fs::remove_file(w.root.join(".git/BLOCK")).unwrap();
    drain(&mut w.sim);
    assert!(!w.sim.app.project_write_running());
    assert!(commit_subjects(&w.root).contains(&"RUNNINGCOMMIT".to_string()));
    assert!(has_write_hints(&w), "{:?}", write_tokens(&w.sim.app));
    // The "Committed" flash replaces the footer until it is cleared.
    w.sim.app.flash = None;
    w.sim.draw();
    w.sim.see("commit");
    w.sim.see("stage");
}

// ---- old repeated_ui_renders_of_the_changes_hub_do_not_rescan_git_status ----
// The canvas's drawn content is the runtime's last answered view; a draw alone never asks the
// session again, so repainting the changes screen cannot re-run `git status`.
#[test]
fn vcs_hub_e2e_repeated_draws_of_the_changes_screen_do_not_recall_the_view() {
    let mut w = W::git("vcs_hub_e2e_repeated_draws", true, "en", "relative", false);
    assert!(w.screen_is("changes"));
    let (views, _) = w.counts();
    for _ in 0..40 {
        w.sim.draw();
    }
    assert_eq!(
        w.counts().0,
        views,
        "40 draws issued no new view request (git status is not re-run per frame)"
    );
}

// ---- old jj_hub_opens_even_when_the_git_integration_is_switched_off +
// jj_hub_opens_even_when_the_git_binary_is_missing ----
// `[external] git` and the git-binary probe gate the git plugin only; a jj repo's project opens
// through `p` either way — jj never asks either.
#[test]
fn vcs_hub_e2e_jj_opens_despite_git_switched_off_and_missing_binary() {
    if !crate::vcs::jj::available() {
        return;
    }
    let dir = sandbox("vcs_hub_e2e_jj_no_git").into_canonical().unwrap();
    let root = dir.join("repo");
    seed_jj_write_repository(&root, false);
    let opens_jj = |sim: &mut Sim, why: &str| {
        sim.app.tab.projects.wait_for_result();
        sim.app.poll_projects();
        sim.key('p');
        drain(sim);
        assert!(
            sim.app.project_active(),
            "{why}: the jj project opens: {:?}",
            sim.app.flash
        );
        assert_eq!(sim.app.project_frame().unwrap().key.screen, "changes");
        let content = sim.app.project_content().unwrap();
        let ProjectContent::List(list) = content.as_ref() else {
            panic!("{why}: the changes list is required")
        };
        assert!(
            !list.rows.is_empty(),
            "{why}: the seeded jj change is listed"
        );
    };
    // `[external] git = false` (both plugins registered, the git one just disabled).
    // The tree's status markers come from jj and never consult that setting either.
    let mut cfg = Config::default();
    cfg.external.git = false;
    cfg.ui.tabbar = "hidden".into();
    cfg.ui.statusbar = "split".into();
    let mut sim = Sim::with_config_sized(&root, cfg, 200, 24);
    assert!(
        !crate::vcs::statuses(&root).is_empty(),
        "the tree's markers come from jj regardless of [external] git"
    );
    opens_jj(&mut sim, "external.git=false");
    assert_ne!(
        sim.app.flash.as_deref(),
        Some(crate::i18n::tr(
            sim.app.lang,
            crate::i18n::Msg::ExternalGitDisabled
        )),
        "jj never asks [external] git, so this reason must never fire for it"
    );
    // A missing git binary with the default config.
    crate::git::set_git_binary_available_for_test(Some(false));
    let mut sim = Sim::with_config_sized(&root, Config::default(), 200, 24);
    opens_jj(&mut sim, "no git binary");
    assert_ne!(
        sim.app.flash.as_deref(),
        Some(crate::i18n::tr(
            sim.app.lang,
            crate::i18n::Msg::GitNotInstalled
        )),
        "the jj backend never touches the git executable"
    );
    crate::git::set_git_binary_available_for_test(None);
}

// ---- old external_git_disabled_flashes_distinct_message_and_tree_renders +
// open_git_view_reports_a_missing_git_binary_distinctly ----
// On a real git repository `p` is gated by `ops::gate` (`gate_open_git_view`): with
// `[external] git = false`, or without a git executable, no project opens and no legacy hub
// opens; the flash names the real reason (not "not a repository") and the tree still draws.
#[test]
fn vcs_hub_e2e_p_on_a_git_repo_flashes_the_distinct_reason_when_git_is_off_or_missing() {
    use crate::i18n::{tr, Msg};
    let dir = sandbox("vcs_hub_e2e_git_off").into_canonical().unwrap();
    let root = dir.join("repo");
    seed_write_repository(&root, true);
    let refused = |sim: &mut Sim, msg: Msg, why: &str| {
        sim.app.flash = None;
        sim.key('p');
        drain(sim);
        assert!(!sim.app.project_active(), "{why}: no project opens");
        assert!(sim.app.project_frame().is_none(), "{why}: no project frame");
        assert_eq!(
            sim.app.flash.as_deref(),
            Some(tr(sim.app.lang, msg)),
            "{why}: the distinct reason"
        );
        for other in [
            Msg::NotAGitRepo,
            Msg::ExternalGitDisabled,
            Msg::GitNotInstalled,
        ] {
            if std::mem::discriminant(&other) != std::mem::discriminant(&msg) {
                assert_ne!(
                    sim.app.flash.as_deref(),
                    Some(tr(sim.app.lang, other)),
                    "{why}: not the other reasons"
                );
            }
        }
        // The tree still renders after the refusal.
        sim.draw();
        sim.see("a-modified");
    };
    // `[external] git = false`: statuses, branch and workdir degrade to empty/None.
    let mut cfg = Config::default();
    cfg.external.git = false;
    cfg.ui.tabbar = "hidden".into();
    cfg.ui.statusbar = "split".into();
    let mut sim = Sim::with_config_sized(&root, cfg, 200, 24);
    assert!(crate::git::statuses(&root).is_empty(), "statuses");
    assert!(crate::git::branch(&root).is_none(), "branch");
    assert!(crate::git::workdir(&root).is_none(), "workdir");
    refused(&mut sim, Msg::ExternalGitDisabled, "external.git=false");
    // No git executable with the default config.
    crate::git::set_git_binary_available_for_test(Some(false));
    let mut sim = Sim::with_config_sized(&root, Config::default(), 200, 24);
    refused(&mut sim, Msg::GitNotInstalled, "no git binary");
    // Sanity (old tests' second half): with git available and enabled, `p` opens the project.
    crate::git::set_git_binary_available_for_test(None);
    let mut sim = Sim::with_config_sized(&root, Config::default(), 200, 24);
    sim.app.tab.projects.wait_for_result();
    sim.app.poll_projects();
    sim.key('p');
    drain(&mut sim);
    assert!(
        sim.app.project_active(),
        "sanity: the same repo opens: {:?}",
        sim.app.flash
    );
}
