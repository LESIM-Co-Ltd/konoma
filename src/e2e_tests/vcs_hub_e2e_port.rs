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
    let row = w.root.join("a-modified");
    w.key('e');
    assert_eq!(w.sim.app.take_pending_edit(), Some((row.clone(), None)));
    w.key('o');
    assert_eq!(log.lock().unwrap().len(), 1);
    assert_eq!(
        log.lock().unwrap()[0].args,
        vec![row.to_string_lossy().into_owned()]
    );
    // Ctrl-t opens the row in a new tab and leaves the list in this one.
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
    w.key('e');
    assert!(w.sim.app.take_pending_edit().is_none());
    w.key('o');
    assert!(log.lock().unwrap().is_empty());
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

// ---- Open question: old e2e_jj_worktrees_key_flashes_instead_of_opening_the_git_list and
// old e2e_jj_sync_key_on_a_git_repo_flashes_and_opens_nothing ----
//
// What the user sees on the new path today. The old hub flashed `JjWorkspacesUnlisted` for `w` in
// a jj repository and `JjSyncNotJj` for a bound sync key in a git repository. On the new path the
// jj screens have no worktree screen and the git plugin has no jj sync operation, so both keys do
// nothing visible. Neither difference is in the decided list of the design document; this pins
// the current behaviour and is waiting for a decision.

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
/// removes `.git/BLOCK`; the order comes from that file, never from the clock.
fn blocked_commit(name: &str, message: &str) -> W {
    let mut w = W::git(name, true, "en", "relative", false);
    install_blocking_pre_commit_hook(&w.root);
    std::fs::write(w.root.join(".git/BLOCK"), b"").unwrap();
    w.key('c');
    w.type_text(message);
    // Direct press: `W::press` would wait for the parked job.
    w.sim.press(KeyCode::Enter, KeyModifiers::NONE);
    w
}

fn commit_subjects(root: &std::path::Path) -> Vec<String> {
    crate::git::log(root, 200)
        .into_iter()
        .map(|c| c.summary)
        .collect()
}

#[test]
fn vcs_hub_e2e_ui_keeps_responding_while_a_real_git_commit_is_blocked() {
    let mut w = blocked_commit("vcs_hub_e2e_blocked_commit", "BLOCKEDCOMMIT");
    let before = commit_subjects(&w.root);
    assert!(w.sim.app.project_write_running(), "the commit is running");
    assert!(w.sim.app.git_write_running());
    assert!(!commit_subjects(&w.root).contains(&"BLOCKEDCOMMIT".to_string()));

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
    let mut w = blocked_commit("vcs_hub_e2e_quit_commit", "WILLQUIT");
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
    // The warning only warns: `y` still leaves.
    w.sim.key('y');
    assert!(w.sim.quit);
    std::fs::remove_file(w.root.join(".git/BLOCK")).ok();
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
    let views_alloc = crate::mem_tests::allocated_by(|| {
        for _ in 0..3 {
            w.press(KeyCode::Tab); // log, graph, branches
            steps.push(w.sim.app.project_frame().unwrap().key.screen.clone());
        }
        for _ in 0..8 {
            if w.screen_is("changes") {
                break;
            }
            w.press(KeyCode::Tab);
        }
        steps.push(w.sim.app.project_frame().unwrap().key.screen.clone());
        w.select("big.txt");
        w.press(KeyCode::Enter);
    });
    // Every screen really was opened and drawn (a skipped screen would make the bound vacuous), and
    // the large diff is the full-screen preview of big.txt.
    assert_eq!(steps, ["log", "graph", "branches", "changes"]);
    assert_eq!(w.sim.app.tab.mode, Mode::Preview);
    w.sim.see("big.txt");
    w.sim.see("LINE changed 0");
    assert!(
        // measured ~36MB (five screens and a 1005-line diff); the old hub's bound, ~1.7x headroom
        views_alloc < 60_000_000,
        "the canvas screens and the large diff allocated too much on the UI thread: {views_alloc}"
    );
    assert!(
        // Not vacuous: the screens cost something, so a counter that stopped counting is caught.
        views_alloc > 10_000_000,
        "the allocation counter looks dead: {views_alloc}"
    );
}
