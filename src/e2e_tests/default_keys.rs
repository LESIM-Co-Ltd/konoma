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

#[cfg(feature = "git")]
mod git {
    use super::*;

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
}
