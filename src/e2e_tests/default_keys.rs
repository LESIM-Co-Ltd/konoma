//! The default keys of the key redesign (plugin stage 2 / B3a), through the real key path: what
//! each changed key does now, and that the old key no longer does it.

use super::*;

/// `q` in the tree's range selection used to quit the app. It cancels the selection now (the same as
/// `Esc`); `Q` is the one key that quits.
#[test]
fn e2e_q_in_the_range_selection_cancels_it_and_does_not_quit() {
    let dir = sandbox("dk_visual_q");
    seed_files(&dir);
    let mut cfg = cfg_en();
    cfg.ui.confirm_quit = false;
    let mut s = Sim::with_config(&canon(&dir), cfg);
    s.key('v');
    s.key('j');
    assert!(s.app.is_visual());
    s.key('q');
    assert!(!s.quit, "q must not quit the app from the selection");
    assert!(!s.app.is_visual(), "q leaves the selection");
    assert!(!s.app.has_selection(), "and drops what was selected");
    assert_eq!(s.app.tab.mode, Mode::Tree);
    // `Esc` does the same, and `Q` still quits from the selection.
    s.key('v');
    s.key('j');
    s.esc();
    assert!(!s.app.is_visual() && !s.app.has_selection());
    s.key('v');
    s.key('Q');
    assert!(s.quit, "Q quits, from the selection too");
}

/// `Enter` toggles the focused checkbox and `<details>`; `Space` does not (it is no longer a
/// document key, and under `ui.keys = "less"` it is not a page key either).
#[test]
fn e2e_enter_toggles_the_focused_item_and_space_does_not() {
    let dir = sandbox("dk_enter_not_space");
    std::fs::write(
        dir.join("d.md"),
        "- [ ] one\n\n<details>\n<summary>more</summary>\n\nbody text\n\n</details>\n",
    )
    .unwrap();
    for keys in ["vim", "less"] {
        let mut cfg = cfg_en();
        cfg.ui.keys = keys.into();
        let mut s = Sim::with_config(&canon(&dir), cfg);
        s.select("d.md");
        s.enter();
        s.tab();
        s.key(' ');
        s.esc(); // Space opens the operation menu
        assert!(
            std::fs::read_to_string(dir.join("d.md"))
                .unwrap()
                .contains("- [ ] one"),
            "{keys}: Space must not toggle the checkbox"
        );
        s.enter();
        assert!(
            std::fs::read_to_string(dir.join("d.md"))
                .unwrap()
                .contains("- [x] one"),
            "{keys}: Enter toggles the checkbox"
        );
        // And the same for `<details>`: Space leaves it closed, Enter opens it.
        s.tab();
        s.key(' ');
        s.esc();
        s.dont_see("body text");
        s.enter();
        s.see("body text");
        std::fs::write(
            dir.join("d.md"),
            "- [ ] one\n\n<details>\n<summary>more</summary>\n\nbody text\n\n</details>\n",
        )
        .unwrap();
    }
}

/// `t` opens the new tab right of the current one; the operation that appends at the end keeps
/// its name and works when a key is bound to it.
#[test]
fn e2e_tab_new_still_appends_at_the_end_when_a_key_is_bound() {
    let dir = sandbox("dk_tab_new_bound");
    seed_files(&dir);
    let mut s = Sim::with_keys(&canon(&dir), &[("global", &[("W", "tab_new")])]);
    s.key('t');
    s.key('t');
    assert_eq!(s.app.tab_count(), 3);
    s.key('1');
    assert_eq!(s.app.active_tab_index(), 0);
    s.key('W');
    assert_eq!(s.app.tab_count(), 4);
    assert_eq!(s.app.active_tab_index(), 3, "tab_new appends at the end");
    s.key('1');
    s.key('t');
    assert_eq!(
        s.app.active_tab_index(),
        1,
        "t inserts right of the current tab"
    );
}

/// The keys that were taken away from the tree do nothing there now (they are `p` and `,` later).
#[test]
fn e2e_the_tree_s_and_o_and_p_changes() {
    let dir = sandbox("dk_tree_keys");
    seed_files(&dir);
    let mut s = Sim::with_config(&canon(&dir), cfg_en());
    s.key('s');
    assert!(!s.app.is_sort_menu(), "s no longer opens the sort menu");
    let before = s.app.path_style;
    s.key('o');
    assert!(!s.app.is_git_view(), "o no longer opens the Git hub");
    // `p` is the project's screen: in a directory that is not a repository it says so.
    s.key('p');
    assert_eq!(
        s.app.path_style, before,
        "p no longer cycles the path style"
    );
}

#[cfg(feature = "git")]
mod git {
    use super::*;

    /// In a git hub `s` stages; in a jj hub it syncs (asks first); `R` is neither. `l` opens the
    /// selected file's diff and `h` goes back, like `Enter` and `q`.
    #[test]
    fn e2e_s_stages_in_a_git_hub_and_syncs_in_a_jj_hub_and_h_l_open_and_close() {
        let dir = sandbox("dk_hub_keys");
        seed_repo(&dir);
        let mut s = Sim::new(&canon(&dir));
        s.key('p');
        assert!(s.app.is_git_view());
        assert_eq!(staged_of(&s.app, "a.rs"), Some(false));
        s.key('s');
        assert_eq!(
            staged_of(&s.app, "a.rs"),
            Some(true),
            "s stages in a git hub"
        );
        assert!(!s.app.is_dialog());
        s.key('R');
        assert!(
            !s.app.is_dialog() && s.app.flash.is_none(),
            "R is not bound"
        );

        // The same screen opened for jj: `s` is the sync (the repository here is git, so it says
        // so; the real jj round trip is `e2e_jj_sync_full_round_trip_confirms_and_reports_completion`).
        // It must not stage again.
        s.app.git_vcs = crate::vcs::VcsKind::Jj;
        assert_eq!(
            s.app.key_context(),
            Some(crate::keymap::Context::Screen(
                crate::keymap::Backend::Jj,
                crate::keymap::Screen::Changes
            ))
        );
        s.key('s');
        assert!(
            s.app.flash.as_deref().is_some_and(
                |f| f.contains(crate::i18n::tr(s.app.lang, crate::i18n::Msg::JjSyncNotJj))
            ),
            "s in a jj hub is the sync, got {:?}",
            s.app.flash
        );
        s.app.git_vcs = crate::vcs::VcsKind::Git;

        // l = open (Enter's operation), h = back (q's).
        s.key('l');
        assert!(
            s.app.is_git_diff_preview(),
            "l opens the selected file's diff"
        );
        s.key('q');
        assert!(s.app.is_git_view());
        s.key('h');
        assert!(!s.app.is_git_view(), "h goes back out of the hub");
        assert_eq!(s.app.tab.mode, Mode::Tree);
    }

    /// The log and the graph are not reached from the hub by `l` / `g` any more: `g` is the top of
    /// the list and `l` opens the diff.
    #[test]
    fn e2e_the_hub_g_and_l_are_the_list_keys_not_the_log_and_the_graph() {
        let dir = sandbox("dk_hub_g_l");
        seed_repo(&dir);
        let mut s = Sim::new(&canon(&dir));
        s.key('p');
        s.key('G');
        let last = s.app.git_view_selected();
        s.key('g');
        let first = s.app.git_view_selected();
        assert_ne!(first, last, "G and g move to the ends of the list");
        assert!(s.app.is_git_view(), "g stays in the hub");
        assert!(!s.app.is_git_graph() && !s.app.is_git_log());
    }

    /// In the diff, `Ctrl-n` / `Ctrl-p` switch the changed file; `n` / `N` do nothing for now.
    #[test]
    fn e2e_in_the_diff_ctrl_n_p_cycle_the_changed_files_and_n_is_free() {
        let dir = sandbox("dk_diff_ctrl_n");
        seed_repo(&dir);
        let mut s = Sim::new(&canon(&dir));
        s.select("a.rs");
        s.key('d');
        assert!(s.app.is_git_diff_preview());
        let first = s.screen();
        s.key('n');
        assert_eq!(first, s.screen(), "n changes nothing in the diff");
        s.key('N');
        assert_eq!(first, s.screen(), "N changes nothing in the diff");
        s.ctrl('n');
        let next = s.screen();
        assert_ne!(first, next, "Ctrl-n shows the next changed file");
        s.ctrl('p');
        assert_eq!(first, s.screen(), "Ctrl-p goes back to the first");
    }

    /// The branch list: `y` opens a menu whose item copies the selected branch's name; `c` creates
    /// (`n` does not); `h` goes back.
    #[test]
    fn e2e_branch_list_y_menu_copies_the_name_and_c_creates() {
        let dir = sandbox("dk_branch_keys");
        seed_repo(&dir);
        let mut s = Sim::new(&canon(&dir));
        s.key('p');
        s.key('b');
        assert!(s.app.is_git_branches());
        let name = s.app.git_branch_view()[s.app.git_branch_sel()].name.clone();
        crate::test_support::clear_test_clipboard();
        s.key('y');
        assert_eq!(
            s.app.pending_leader,
            Some(crate::keymap::LeaderId::BranchCopy),
            "y opens the copy menu instead of copying at once"
        );
        assert_eq!(crate::test_support::get_test_clipboard(), None);
        s.see(crate::i18n::tr(s.app.lang, crate::i18n::Msg::WkBranchName));
        s.key('n');
        assert_eq!(s.app.pending_leader, None);
        assert_eq!(
            crate::test_support::get_test_clipboard().as_deref(),
            Some(name.as_str()),
            "the menu's item copied the branch name"
        );
        // `n` opens nothing (it is layer 1's next match); `c` opens the new-branch input.
        s.key('n');
        assert!(!s.app.is_dialog());
        s.key('c');
        assert!(s.app.is_dialog(), "c asks for the new branch's name");
        s.esc();
        s.key('h');
        assert!(s.app.is_git_view(), "h goes back to the hub");
    }

    /// The worktree list: `c` creates and `l` / `h` open and go back; `n` creates nothing.
    #[test]
    fn e2e_worktree_list_c_creates_and_n_does_not() {
        let dir = sandbox("dk_worktree_keys");
        seed_repo(&dir);
        let mut s = Sim::new(&canon(&dir));
        s.key('p');
        s.key('w');
        assert!(s.app.is_git_worktrees());
        s.key('n');
        assert!(!s.app.is_dialog(), "n no longer creates a worktree");
        s.key('c');
        assert!(s.app.is_dialog(), "c asks for the new worktree's branch");
        s.esc();
        s.key('h');
        assert!(s.app.is_git_view(), "h goes back to the hub");
    }

    /// The graph's base is cleared by `x` alone: `0` is the line start and clears nothing.
    #[test]
    fn e2e_graph_x_clears_the_base_and_zero_does_not() {
        let dir = sandbox("dk_graph_zero");
        seed_repo(&dir);
        let mut s = Sim::with_hub_screen_keys(&canon(&dir));
        s.key('p');
        s.key('g');
        s.key('G');
        s.key('s');
        s.see("⌖ base:");
        s.key('0');
        s.see("⌖ base:");
        s.key('x');
        s.dont_see("⌖ base:");
    }

    /// The branch panel of the graph: `c` shows the current branch only (it was `n`, the next
    /// match), and `l` applies like `Enter`.
    #[test]
    fn e2e_graph_panel_c_is_current_only_and_l_applies() {
        let dir = sandbox("dk_graph_panel");
        seed_repo_history(&dir);
        let mut s = Sim::with_hub_screen_keys(&canon(&dir));
        s.key('p');
        s.key('g');
        s.key('b');
        assert!(s.app.is_git_graph_picker());
        let feature_on = |app: &App| -> Option<bool> {
            app.git_graph_picker_items()
                .into_iter()
                .find(|(name, _, _)| name == "feature-x")
                .map(|(_, _, on)| on)
        };
        assert_eq!(feature_on(&s.app), Some(true));
        s.key('n');
        assert_eq!(feature_on(&s.app), Some(true), "n does nothing here");
        s.key('c');
        assert_eq!(feature_on(&s.app), Some(false));
        s.key('l');
        assert!(
            !s.app.is_git_graph_picker(),
            "l applies and closes the panel"
        );
        assert!(s.app.is_git_graph());
    }
}
