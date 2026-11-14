// Key behaviour of the vcs project screens: `h` goes back, `d` is diff-all, `!` launches the tool.

fn frame_screen(sim: &Sim) -> String {
    sim.app.project_frame().unwrap().key.screen.clone()
}

/// The title is on screen. Whitespace is ignored. The cell after a wide glyph is its continuation:
/// a terminal never shows it, but the test backend keeps whatever an earlier frame left there, so
/// those cells are skipped (reading them mixes the previous screen's text into the title).
#[track_caller]
fn see_title(sim: &Sim, lang: &str) {
    let squash = |s: &str| s.split_whitespace().collect::<String>();
    let want = if lang == "en" { "Uncommittedchanges" } else { "未コミットの変更" };
    let screen = sim.screen();
    // The title sits on the outer frame's top edge, so look at the first two rows only.
    let buf = sim.term.backend().buffer();
    let mut top = String::new();
    for y in 0..2 {
        top.push_str(&crate::test_support::top_edge::row_text(buf, y, 0..buf.area.width));
        top.push('\n');
    }
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

/// `config(id, lang)` with the plugin's `[keys.<id>]` table set to `entries`.
fn config_with_keys(id: &str, lang: &str, entries: &[(&str, &str)]) -> Config {
    let mut cfg = config(id, lang);
    cfg.keys.surfaces.insert(
        id.to_string(),
        entries
            .iter()
            .map(|(k, a)| (k.to_string(), a.to_string()))
            .collect(),
    );
    cfg
}

/// The `?` help rows of the current screen as (keys, description).
fn help_rows(sim: &Sim) -> Vec<(String, String)> {
    crate::ops::help::sections(&sim.app)
        .into_iter()
        .flat_map(|s| s.rows)
        .map(|(k, d)| (k.to_string(), d.to_string()))
        .collect()
}

fn footer_has_key(sim: &Sim, key: char) -> bool {
    let prefix = format!("{key}:");
    crate::ops::footer::tokens(&sim.app, Context::Canvas)
        .iter()
        .any(|t| t.starts_with(&prefix))
}

#[test]
fn vcs_jump_keys_rebinding_moves_the_jump_in_keys_help_and_footer() {
    if isolated("vcs_jump_keys_rebinding_moves_the_jump_in_keys_help_and_footer") {
        return;
    }
    let repo = super::vcs_changes::git_repo("vcs_jump_rebind", true);
    // `B` is free on every vcs screen (`x` is the changes screen's discard).
    for (lang, branches) in [("en", "branches"), ("ja", "ブランチ")] {
        let cfg = config_with_keys("git", lang, &[("B", "goto_branches")]);
        let mut sim = open(&repo, cfg, (160, 20));
        screen(&mut sim, "changes");
        press(&mut sim, KeyCode::Char('b'));
        assert_ne!(frame_screen(&sim), "branches", "{lang}: the old key is inert");
        assert_eq!(frame_screen(&sim), "changes", "{lang}");
        let rows = help_rows(&sim);
        assert!(
            rows.iter().any(|(k, d)| k == "B" && d == branches),
            "{lang}: help lists B for branches: {rows:?}"
        );
        assert!(
            !rows.iter().any(|(k, d)| k == "b" && d == branches),
            "{lang}: help no longer lists b for branches: {rows:?}"
        );
        assert!(footer_has_key(&sim, 'B'), "{lang}: the footer shows B");
        assert!(!footer_has_key(&sim, 'b'), "{lang}: the footer drops b");
        // The other jump keeps its key.
        assert!(footer_has_key(&sim, 'w'), "{lang}");
        press(&mut sim, KeyCode::Char('B'));
        assert_eq!(frame_screen(&sim), "branches", "{lang}: the new key jumps");
        // From the branches screen the new key is the way to the others too.
        press(&mut sim, KeyCode::Char('w'));
        assert_eq!(frame_screen(&sim), "worktrees", "{lang}");
        press(&mut sim, KeyCode::Char('B'));
        assert_eq!(frame_screen(&sim), "branches", "{lang}");
    }
}

#[test]
fn vcs_jump_keys_noop_removes_the_jump_everywhere() {
    if isolated("vcs_jump_keys_noop_removes_the_jump_everywhere") {
        return;
    }
    let repo = super::vcs_changes::git_repo("vcs_jump_noop", true);
    for (lang, branches) in [("en", "branches"), ("ja", "ブランチ")] {
        let cfg = config_with_keys("git", lang, &[("b", "noop")]);
        let mut sim = open(&repo, cfg, (160, 20));
        screen(&mut sim, "changes");
        // The default has the row, so the absence below is not vacuous.
        let mut base = open(&repo, config("git", lang), (160, 20));
        screen(&mut base, "changes");
        assert!(help_rows(&base).iter().any(|(k, d)| k == "b" && d == branches));
        assert!(footer_has_key(&base, 'b'));
        press(&mut sim, KeyCode::Char('b'));
        assert_eq!(frame_screen(&sim), "changes", "{lang}: b does not jump");
        assert!(
            !help_rows(&sim).iter().any(|(_, d)| d == branches),
            "{lang}: no help row for the removed jump"
        );
        assert!(!footer_has_key(&sim, 'b'), "{lang}: no footer hint");
        assert!(footer_has_key(&sim, 'w'), "{lang}: worktrees still jumps");
        press(&mut sim, KeyCode::Char('w'));
        assert_eq!(frame_screen(&sim), "worktrees", "{lang}");
        // Tab still reaches the screen: only the key is gone.
        screen(&mut sim, "branches");
        assert_eq!(frame_screen(&sim), "branches", "{lang}");
    }
}

/// The frame the settled state draws on a cleared terminal. The test backend keeps the cells a
/// wide glyph's continuation did not overwrite, so a frame drawn over earlier ones carries
/// leftovers that depend on how many intermediate frames (loading, busy) came before it; two
/// sims compared cell for cell must not depend on that history.
fn full_repaint(sim: &mut Sim) -> String {
    sim.term.clear().expect("clear");
    sim.draw();
    sim.screen()
}

#[test]
fn vcs_jump_keys_an_empty_or_unrelated_table_leaves_the_frames_byte_identical() {
    if isolated("vcs_jump_keys_an_empty_or_unrelated_table_leaves_the_frames_byte_identical") {
        return;
    }
    let repo = super::vcs_changes::git_repo("vcs_jump_default", true);
    for lang in ["en", "ja"] {
        for name in ["changes", "log", "branches", "worktrees", "graph"] {
            let mut base = open(&repo, config("git", lang), (160, 20));
            screen(&mut base, name);
            let want = full_repaint(&mut base);
            for cfg in [
                config_with_keys("git", lang, &[]),
                // No action or jump has `B`: removing it changes nothing.
                config_with_keys("git", lang, &[("B", "noop")]),
            ] {
                let mut sim = open(&repo, cfg, (160, 20));
                screen(&mut sim, name);
                assert_eq!(full_repaint(&mut sim), want, "{lang} {name}");
                assert_eq!(help_rows(&sim), help_rows(&base), "{lang} {name}");
            }
        }
        let mut sim = open(&repo, config("git", lang), (160, 20));
        screen(&mut sim, "changes");
        press(&mut sim, KeyCode::Char('b'));
        assert_eq!(frame_screen(&sim), "branches", "{lang}: b is the default");
        press(&mut sim, KeyCode::Char('w'));
        assert_eq!(frame_screen(&sim), "worktrees", "{lang}: w is the default");
    }
}

#[test]
fn vcs_jump_keys_a_refused_key_keeps_the_default_jump() {
    if isolated("vcs_jump_keys_a_refused_key_keeps_the_default_jump") {
        return;
    }
    let repo = super::vcs_changes::git_repo("vcs_jump_refused", true);
    // `j` moves the cursor, `q` leaves, `/` filters: the host's keys cannot be jump keys.
    for refused in ["j", "q", "/"] {
        let cfg = config_with_keys("git", "en", &[(refused, "goto_branches")]);
        let mut sim = open(&repo, cfg, (160, 20));
        screen(&mut sim, "changes");
        press(&mut sim, KeyCode::Char('b'));
        assert_eq!(frame_screen(&sim), "branches", "{refused}: default kept");
    }
    // Another jump's key is a collision: the jump keeps its own default.
    let cfg = config_with_keys("git", "en", &[("w", "goto_branches")]);
    let mut sim = open(&repo, cfg, (160, 20));
    screen(&mut sim, "changes");
    press(&mut sim, KeyCode::Char('w'));
    assert_eq!(frame_screen(&sim), "worktrees");
    press(&mut sim, KeyCode::Char('b'));
    assert_eq!(frame_screen(&sim), "branches");
}

#[test]
fn vcs_jump_keys_jj_bookmarks_follow_the_rebinding() {
    if isolated("vcs_jump_keys_jj_bookmarks_follow_the_rebinding") || !crate::vcs::jj::available() {
        return;
    }
    let Some(repo) = crate::test_support::jj_scratch_seeded("vcs_jump_jj") else {
        return;
    };
    for (lang, title) in [("en", "bookmarks"), ("ja", "ブックマーク")] {
        let cfg = config_with_keys("jj", lang, &[("B", "goto_branches")]);
        let mut sim = open(&repo, cfg, (160, 20));
        screen(&mut sim, "changes");
        press(&mut sim, KeyCode::Char('b'));
        assert_eq!(frame_screen(&sim), "changes", "{lang}: the old key is inert");
        assert!(
            help_rows(&sim).iter().any(|(k, d)| k == "B" && d == title),
            "{lang}: {:?}",
            help_rows(&sim)
        );
        press(&mut sim, KeyCode::Char('B'));
        assert_eq!(frame_screen(&sim), "branches", "{lang}");
        let cfg = config_with_keys("jj", lang, &[("b", "noop")]);
        let mut sim = open(&repo, cfg, (160, 20));
        screen(&mut sim, "changes");
        press(&mut sim, KeyCode::Char('b'));
        assert_eq!(frame_screen(&sim), "changes", "{lang}: noop");
        assert!(!help_rows(&sim).iter().any(|(_, d)| d == title), "{lang}");
    }
}

/// A jump key that a screen's own action also has: on that screen the action wins (it asks its
/// confirmation, the screen does not change, and help/footer show the action, not the jump); on
/// a screen where no action has the key, it jumps. The jump hints are the home (changes)
/// screen's only, so no other screen lists the jump row, as for the default `b`.
#[test]
fn vcs_jump_keys_a_screen_actions_key_wins_over_a_rebound_jump_there_only() {
    if isolated("vcs_jump_keys_a_screen_actions_key_wins_over_a_rebound_jump_there_only") {
        return;
    }
    let repo = super::vcs_changes::git_repo("vcs_jump_collision", true);
    // `x` is the changes screen's discard.
    for (lang, branches, discard) in [
        ("en", "branches", "discard file (confirm)"),
        ("ja", "ブランチ", "ファイル破棄 (確認)"),
    ] {
        let cfg = config_with_keys("git", lang, &[("x", "goto_branches")]);
        let mut sim = open(&repo, cfg, (160, 20));
        screen(&mut sim, "changes");
        let rows = help_rows(&sim);
        assert!(
            rows.iter().any(|(k, d)| k == "x" && d == discard),
            "{lang}: help has the discard row on x: {rows:?}"
        );
        assert!(
            !rows.iter().any(|(_, d)| d == branches),
            "{lang}: help has no branches jump row (x is taken, b is rebound away): {rows:?}"
        );
        assert!(footer_has_key(&sim, 'x'), "{lang}: the footer's x is the action");
        let footer = crate::ops::footer::tokens(&sim.app, Context::Canvas);
        assert!(
            !footer.iter().any(|t| t.starts_with("x:") && t.contains(branches)),
            "{lang}: {footer:?}"
        );
        assert!(!footer_has_key(&sim, 'b'), "{lang}: b no longer shows");
        // The action runs: its confirmation is open (not answered), the screen is the same.
        assert_eq!(press_x(&mut sim), XOutcome::Confirm, "{lang}");
        assert_eq!(frame_screen(&sim), "changes", "{lang}: x did not jump");
        drop(sim);
        // The old key is inert on that screen too.
        let cfg = config_with_keys("git", lang, &[("x", "goto_branches")]);
        let mut sim = open(&repo, cfg, (160, 20));
        screen(&mut sim, "changes");
        press(&mut sim, KeyCode::Char('b'));
        assert_eq!(frame_screen(&sim), "changes", "{lang}: b is inert");
        assert!(sim.app.project_dialog.is_none(), "{lang}");
        // On the screens whose actions do not have `x`, the same key jumps.
        for from in ["worktrees", "log"] {
            screen(&mut sim, from);
            assert_eq!(press_x(&mut sim), XOutcome::Nothing, "{lang} {from}");
            assert_eq!(frame_screen(&sim), "branches", "{lang}: x jumps from {from}");
            assert!(sim.app.project_dialog.is_none(), "{lang} {from}");
        }
        // The default `b` has no row off the home screen either, so the rebound key has none:
        // the rows are as in the unconfigured app.
        screen(&mut sim, "worktrees");
        let mut base = open(&repo, config("git", lang), (160, 20));
        screen(&mut base, "worktrees");
        assert_eq!(help_rows(&sim), help_rows(&base), "{lang}: worktrees help");
    }
}

/// The startup notice about a refused jump key reaches the footer notice, in both languages.
#[test]
fn vcs_jump_keys_a_refused_key_is_told_at_startup() {
    if isolated("vcs_jump_keys_a_refused_key_is_told_at_startup") {
        return;
    }
    let repo = super::vcs_changes::git_repo("vcs_jump_notice", true);
    for lang in ["en", "ja"] {
        let cfg = config_with_keys("git", lang, &[("j", "goto_branches")]);
        let mut sim = Sim::with_config_sized(&repo, cfg, 160, 20);
        sim.app.set_startup_flash(None, 400);
        let flash = sim.app.flash.clone().expect("a startup message");
        assert!(
            flash.contains("git") && flash.contains("`j`"),
            "{lang}: {flash}"
        );
        // A good rebinding says nothing.
        let cfg = config_with_keys("git", lang, &[("B", "goto_branches")]);
        let mut sim = Sim::with_config_sized(&repo, cfg, 160, 20);
        sim.app.set_startup_flash(None, 400);
        assert!(
            sim.app.flash.as_deref().is_none_or(|f| !f.contains("`B`")),
            "{lang}: {:?}",
            sim.app.flash
        );
    }
}
