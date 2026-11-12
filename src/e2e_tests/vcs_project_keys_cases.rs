// Key behaviour of the vcs project screens: `h` goes back, `d` is diff-all, `!` launches the tool.

fn frame_screen(sim: &Sim) -> String {
    sim.app.project_frame().unwrap().key.screen.clone()
}

/// The title is on screen. Whitespace is ignored because wide glyphs are padded in the dump.
#[track_caller]
fn see_title(sim: &Sim, lang: &str) {
    let squash = |s: &str| s.split_whitespace().collect::<String>();
    let want = if lang == "en" { "Uncommittedchanges" } else { "未コミットの変更" };
    let screen = sim.screen();
    // The title sits on the tab row, so look at the first two rows only.
    let top: String = screen.lines().take(2).collect::<Vec<_>>().join("\n");
    assert!(squash(&top).contains(want), "{lang}: title missing:\n{screen}");
}

#[test]
fn vcs_keys_git_h_goes_back_from_every_list_screen() {
    if isolated("vcs_keys_git_h_goes_back_from_every_list_screen") {
        return;
    }
    let repo = super::vcs_changes::git_repo("vcs_keys_git_h_back", true);
    crate::test_support::run_git(&repo, &["branch", "feature"]);
    for name in ["changes", "log", "branches", "worktrees"] {
        for code in [KeyCode::Char('h'), KeyCode::Left] {
            let mut sim = open(&repo, config("git", "en"), (120, 30));
            screen(&mut sim, name);
            assert_eq!(frame_screen(&sim), name);
            press(&mut sim, code);
            assert!(
                !sim.app.project_active(),
                "{code:?} on {name} leaves the project"
            );
        }
    }
}

#[test]
fn vcs_keys_git_detail_h_pans_and_q_pops() {
    if isolated("vcs_keys_git_detail_h_pans_and_q_pops") {
        return;
    }
    let repo = super::vcs_changes::git_repo("vcs_keys_git_h_pop", true);
    let mut sim = open(&repo, config("git", "en"), (120, 30));
    screen(&mut sim, "changes");
    press(&mut sim, KeyCode::Char('d'));
    assert_eq!(frame_screen(&sim), "detail");
    press(&mut sim, KeyCode::Char('h'));
    assert_eq!(frame_screen(&sim), "detail", "h pans the diff, it does not pop");
    press(&mut sim, KeyCode::Char('q'));
    assert!(sim.app.project_active(), "q pops only the pushed screen");
    assert_eq!(frame_screen(&sim), "changes");
    press(&mut sim, KeyCode::Char('h'));
    assert!(!sim.app.project_active(), "h on the root list leaves");
}

#[test]
fn vcs_keys_git_d_opens_the_uncommitted_diff_or_flashes_no_changes() {
    if isolated("vcs_keys_git_d_opens_the_uncommitted_diff_or_flashes_no_changes") {
        return;
    }
    for lang in ["en", "ja"] {
        let dirty = super::vcs_changes::git_repo("vcs_keys_git_d_dirty", true);
        let mut sim = open(&dirty, config("git", lang), (120, 30));
        screen(&mut sim, "changes");
        press(&mut sim, KeyCode::Char('d'));
        assert_eq!(frame_screen(&sim), "detail", "{lang}: d opens the detail");
        see_title(&sim, lang);

        let clean = super::vcs_changes::git_repo("vcs_keys_git_d_clean", false);
        let mut sim = open(&clean, config("git", lang), (120, 30));
        screen(&mut sim, "changes");
        press(&mut sim, KeyCode::Char('d'));
        assert_eq!(frame_screen(&sim), "changes", "{lang}: nothing to show");
        assert_eq!(
            sim.app.flash.as_deref(),
            Some(if lang == "en" { "no changes" } else { "変更なし" })
        );
    }
}

#[test]
fn vcs_keys_git_bang_requests_the_tool_and_respects_the_external_flag() {
    if isolated("vcs_keys_git_bang_requests_the_tool_and_respects_the_external_flag") {
        return;
    }
    let repo = super::vcs_changes::git_repo("vcs_keys_git_bang", true);
    let mut sim = open(&repo, config("git", "en"), (120, 30));
    screen(&mut sim, "changes");
    assert!(!sim.app.take_launch_git_tool());
    press(&mut sim, KeyCode::Char('!'));
    assert!(sim.app.take_launch_git_tool(), "! queues the tool launch");
    assert!(sim.app.project_active(), "the project stays open");

    let mut cfg = config("git", "en");
    cfg.external.git_tool = false;
    let mut sim = open(&repo, cfg, (120, 30));
    screen(&mut sim, "changes");
    press(&mut sim, KeyCode::Char('!'));
    assert!(!sim.app.take_launch_git_tool(), "git_tool = false queues nothing");
    assert_eq!(
        sim.app.flash.as_deref(),
        Some(crate::i18n::tr(
            sim.app.lang,
            crate::i18n::Msg::ExternalGitToolDisabled
        ))
    );
}

#[test]
fn vcs_keys_jj_h_back_and_d_diff_all() {
    if isolated("vcs_keys_jj_h_back_and_d_diff_all") || !crate::vcs::jj::available() {
        return;
    }
    let Some(repo) = crate::test_support::jj_scratch_seeded("vcs_keys_jj") else {
        return;
    };
    let mut sim = open(&repo, config("jj", "en"), (120, 30));
    screen(&mut sim, "log");
    press(&mut sim, KeyCode::Char('h'));
    assert!(!sim.app.project_active(), "h leaves the jj project");

    let mut sim = open(&repo, config("jj", "en"), (120, 30));
    screen(&mut sim, "changes");
    press(&mut sim, KeyCode::Char('d'));
    let now = frame_screen(&sim);
    assert!(
        now == "detail" || sim.app.flash.as_deref() == Some("no changes"),
        "d opens the diff or says there is nothing (screen {now})"
    );
    if now == "detail" {
        see_title(&sim, "en");
        press(&mut sim, KeyCode::Char('h'));
        assert_eq!(frame_screen(&sim), "changes");
    }
}

#[test]
fn vcs_keys_jj_d_in_a_non_colocated_repo_shows_the_working_copy_diff() {
    if isolated("vcs_keys_jj_d_in_a_non_colocated_repo_shows_the_working_copy_diff")
        || !crate::vcs::jj::available()
    {
        return;
    }
    // `jj_scratch_seeded` inits with --no-colocate (no .git) and edits a.txt after the seed commit.
    let Some(repo) = crate::test_support::jj_scratch_seeded("vcs_keys_jj_nocolo") else {
        return;
    };
    assert!(!repo.join(".git").exists(), "the repository is not colocated");
    // konoma never snapshots on its own; some jj command must have run since the edit.
    assert!(crate::test_support::run_jj(&repo, &["status"]));
    for lang in ["en", "ja"] {
        let mut sim = open(&repo, config("jj", lang), (120, 30));
        screen(&mut sim, "changes");
        press(&mut sim, KeyCode::Char('d'));
        assert_eq!(
            frame_screen(&sim),
            "detail",
            "{lang}: d opens the detail (flash {:?})",
            sim.app.flash
        );
        see_title(&sim, lang);
        sim.see("a.txt");
        sim.see("two");
    }
}
