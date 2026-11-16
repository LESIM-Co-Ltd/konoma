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
fn vcs_keys_git_detail_s_marks_side_by_side_and_q_pops() {
    if isolated("vcs_keys_git_detail_s_marks_side_by_side_and_q_pops") {
        return;
    }
    for lang in ["en", "ja"] {
        let repo = super::vcs_changes::git_repo("vcs_keys_git_d_s", true);
        let mut sim = open(&repo, config("git", lang), (80, 30));
        screen(&mut sim, "changes");
        press(&mut sim, KeyCode::Char('d'));
        assert_eq!(frame_screen(&sim), "detail", "{lang}: d opens the detail");
        sim.dont_see("⇆");
        press(&mut sim, KeyCode::Char('s'));
        sim.see("⇆");
        assert_eq!(
            sim.app.flash.as_deref(),
            Some(if lang == "en" {
                "diff: side-by-side"
            } else {
                "diff: 横並び"
            }),
            "{lang}"
        );
        press(&mut sim, KeyCode::Char('q'));
        assert!(sim.app.project_active(), "{lang}: q pops the detail");
        assert_eq!(frame_screen(&sim), "changes", "{lang}");
    }
}

#[test]
fn vcs_keys_git_file_diff_r_cycles_to_preview_and_q_returns_to_changes() {
    if isolated("vcs_keys_git_file_diff_r_cycles_to_preview_and_q_returns_to_changes") {
        return;
    }
    let dir = sandbox("vcs_keys_git_r_q");
    seed_repo_markdown(&dir);
    let root = canon(&dir);
    let mut sim = open(&root, config("git", "en"), (140, 30));
    screen(&mut sim, "changes");
    press(&mut sim, KeyCode::Char('g'));
    let doc = root.join("doc.md").to_string_lossy().into_owned();
    let index = {
        let content = sim.app.project_content().unwrap();
        let ProjectContent::List(list) = content.as_ref() else {
            panic!("not a list")
        };
        list.rows
            .iter()
            .filter(|r| r.selectable)
            .position(|r| r.value == doc)
            .expect("doc.md row")
    };
    for _ in 0..index {
        press(&mut sim, KeyCode::Char('j'));
    }
    press(&mut sim, KeyCode::Enter);
    assert!(sim.app.is_git_diff_preview(), "Enter opens the file diff");
    assert!(!sim.app.project_active());
    press(&mut sim, KeyCode::Char('R'));
    assert!(
        sim.app.preview_from_diff_for_test(),
        "R cycles to the file's Preview representation"
    );
    press(&mut sim, KeyCode::Char('q'));
    assert!(sim.app.project_active(), "q restores the project");
    assert_eq!(frame_screen(&sim), "changes");
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

// ---- old diff_view_n_switches_changed_files_and_keeps_return_target +
// git_diff_opens_from_view_and_esc_returns_to_view (the cache/close half) ----
// Decided difference: the changed-file cycle moved from `n`/`N` to `Ctrl-n`/`Ctrl-p` (plain
// `n`/`N` move between diff hunks). `Ctrl-n` inside the file diff preview cycles the project's
// change set (wrapping both ways, the tree cursor following), the drawn diff is cached until an
// fs refresh drops the cache, and `Esc`/`q` both close back onto the canvas's changes screen —
// the return target survives cycling.
#[test]
fn vcs_keys_git_file_diff_n_cycles_changed_files_and_closes_to_changes() {
    if isolated("vcs_keys_git_file_diff_n_cycles_changed_files_and_closes_to_changes") {
        return;
    }
    let repo = super::vcs_changes::git_repo("vcs_keys_git_diff_n", true);
    let mut sim = open(&repo, config("git", "en"), (140, 30));
    let ctrl = |sim: &mut Sim, c: char| {
        sim.press(
            KeyCode::Char(c),
            crossterm::event::KeyModifiers::CONTROL,
        );
        drain(sim);
    };
    press(&mut sim, KeyCode::Enter);
    assert!(sim.app.is_git_diff_preview() && !sim.app.project_active());
    assert!(
        sim.app.tab.project_return.is_some(),
        "Enter on the changes row saves the canvas for return"
    );
    let first = sim.app.tab.preview_path.clone().unwrap();
    // `Ctrl-n` walks the changed-file list forward and wraps at the end; `Ctrl-p` walks it back.
    let mut seen = vec![first.clone()];
    for _ in 0..64 {
        ctrl(&mut sim, 'n');
        let now = sim.app.tab.preview_path.clone().unwrap();
        if now == first {
            break;
        }
        assert!(
            !seen.contains(&now),
            "Ctrl-n visited {now:?} twice before wrapping"
        );
        seen.push(now);
    }
    assert!(seen.len() >= 2, "the fixture needs two changed files");
    // The position in the change set is shown and follows the cycling.
    let (pos0, total) = sim.app.diff_change_position().unwrap();
    assert_eq!(total, seen.len(), "the (n/total) indicator counts the change set");
    // `Ctrl-p` walks the same cycle back, wraps at the top, and the tree cursor follows the shown
    // diff.
    ctrl(&mut sim, 'n');
    assert_eq!(
        sim.app.diff_change_position().unwrap().0,
        pos0 % total + 1,
        "the position follows the cycling"
    );
    assert_eq!(
        sim.app.tab.entries[sim.app.tab.selected].path,
        sim.app.tab.preview_path.clone().unwrap(),
        "the tree cursor follows the shown diff"
    );
    ctrl(&mut sim, 'p');
    assert_eq!(
        sim.app.tab.preview_path.as_deref(),
        Some(first.as_path()),
        "Ctrl-p steps back to the previous file"
    );
    for expected in seen.iter().skip(1).rev() {
        ctrl(&mut sim, 'p');
        assert_eq!(
            sim.app.tab.preview_path.as_deref(),
            Some(expected.as_path()),
            "Ctrl-p wraps and walks the cycle back"
        );
    }
    ctrl(&mut sim, 'p');
    assert_eq!(
        sim.app.tab.preview_path.as_deref(),
        Some(first.as_path()),
        "Ctrl-p wraps back to the start"
    );
    // The drawn diff is served through the per-path diff cache: a seeded line comes back
    // verbatim instead of a fresh `file_diff` read, and an fs refresh drops the cache so the
    // next read recomputes from the working tree (never a stale diff).
    sim.app.seed_source_diff_for_test(vec![crate::git::DiffLine {
        kind: crate::git::DiffLineKind::Added,
        old_no: None,
        new_no: Some(1),
        text: "SEEDED CACHE MARKER".into(),
    }]);
    assert!(
        sim.app
            .git_diff_lines()
            .iter()
            .any(|l| l.text == "SEEDED CACHE MARKER"),
        "the per-path diff cache serves the read"
    );
    let mut body = std::fs::read_to_string(&first).unwrap();
    body.push_str("AFTER REFRESH\n");
    std::fs::write(&first, body).unwrap();
    sim.app.refresh_fs_watched(false, &[]);
    let lines = sim.app.git_diff_lines();
    assert!(
        !lines.iter().any(|l| l.text == "SEEDED CACHE MARKER"),
        "refresh drops the diff cache"
    );
    assert!(
        lines.iter().any(|l| l.text.contains("AFTER REFRESH")),
        "the re-read diff shows the new content"
    );
    // `q` closes back to the canvas's changes screen (the saved `project_return`), keeping the
    // row's origin — the fact the old hub's `came_from_git_view` carried.
    press(&mut sim, KeyCode::Char('q'));
    assert!(sim.app.project_active() && frame_screen(&sim) == "changes");
    press(&mut sim, KeyCode::Enter);
    assert!(sim.app.is_git_diff_preview());
    let reopened = sim.app.tab.preview_path.clone().unwrap();
    ctrl(&mut sim, 'n');
    assert_ne!(
        sim.app.tab.preview_path.as_deref(),
        Some(reopened.as_path()),
        "cycling still works after reopening"
    );
    press(&mut sim, KeyCode::Char('q'));
    assert!(sim.app.project_active() && frame_screen(&sim) == "changes");
}

// ---- old diff_from_tree_and_worktree_detail_and_cycle (the `s` cycle half) ----
// `s` on the pushed detail cycles the shared layout unified → split → auto → unified.
#[test]
fn vcs_keys_git_detail_s_cycles_unified_split_auto_unified() {
    if isolated("vcs_keys_git_detail_s_cycles_unified_split_auto_unified") {
        return;
    }
    let repo = super::vcs_changes::git_repo("vcs_keys_git_detail_s3", true);
    let mut sim = open(&repo, config("git", "en"), (120, 30));
    press(&mut sim, KeyCode::Char('d'));
    assert_eq!(frame_screen(&sim), "detail");
    assert!(!sim.app.diff_is_split(200), "starts unified");
    press(&mut sim, KeyCode::Char('s'));
    assert!(sim.app.diff_is_split(200), "s -> split");
    press(&mut sim, KeyCode::Char('s'));
    assert!(
        sim.app.diff_is_split(200) && !sim.app.diff_is_split(40),
        "s -> auto (width decides)"
    );
    press(&mut sim, KeyCode::Char('s'));
    assert!(!sim.app.diff_is_split(200), "s -> unified again");
}

// ---- Esc on a preview opened from a project canvas returns where `q` returns ----
// The old hub's `Esc` closed a diff back to the view it came from. On the project canvas the
// return target is `tab.project_return`; `Esc` and `q` both close the preview through one
// function, so each lands on the same screen/cursor and leaves no stale return target behind.

/// What a close must agree on between `q` and `Esc`.
#[derive(Debug, PartialEq)]
struct Landing {
    active: bool,
    screen: Option<String>,
    cursor: Option<usize>,
    has_return: bool,
    tree: bool,
}

fn landing(sim: &Sim) -> Landing {
    Landing {
        active: sim.app.project_active(),
        screen: sim.app.project_frame().map(|f| f.key.screen.clone()),
        cursor: sim.app.project_frame().map(|f| f.cursor),
        has_return: sim.app.tab.project_return.is_some(),
        tree: sim.app.tab.mode == Mode::Tree,
    }
}

/// A repo of the given backend with changed files, or `None` when jj is not installed (the
/// caller prints NOT RUN).
fn esc_repo(id: &str, name: &str) -> Option<crate::test_support::TmpDir> {
    if id == "git" {
        return Some(super::vcs_changes::git_repo(name, true));
    }
    use crate::test_support::{jj_scratch_bare, run_jj};
    if !crate::vcs::jj::available() {
        eprintln!("NOT RUN: {name}: jj is not installed; 0 jj cases");
        return None;
    }
    let dir = jj_scratch_bare(name).expect("jj is available: fixture must initialize");
    for file in ["a-one.txt", "b-two.txt", "c-three.txt"] {
        std::fs::write(dir.join(file), "base\n").unwrap();
    }
    assert!(run_jj(&dir, &["commit", "-m", "seed"]));
    std::fs::write(dir.join("a-one.txt"), "worktree\n").unwrap();
    std::fs::write(dir.join("b-two.txt"), "changed\n").unwrap();
    std::fs::write(dir.join("d-new.txt"), "new\n").unwrap();
    Some(dir.into_canonical().unwrap())
}

/// The canvas's changes screen with the cursor on the row `down` rows below the first, then the
/// row's diff opened. Returns the session and the row's path.
fn open_changed_diff(repo: &std::path::Path, id: &str, down: usize) -> Sim {
    let mut sim = open(repo, config(id, "en"), (140, 30));
    screen(&mut sim, "changes");
    for _ in 0..down {
        press(&mut sim, KeyCode::Char('j'));
    }
    press(&mut sim, KeyCode::Enter);
    assert!(sim.app.is_git_diff_preview() && !sim.app.project_active(), "{id}");
    assert!(sim.app.tab.project_return.is_some(), "{id}");
    sim
}

#[test]
fn vcs_keys_esc_on_a_file_diff_returns_to_the_changes_row_like_q() {
    if isolated("vcs_keys_esc_on_a_file_diff_returns_to_the_changes_row_like_q") {
        return;
    }
    for id in ["git", "jj"] {
        let Some(repo) = esc_repo(id, "vcs_keys_esc_diff") else {
            continue;
        };
        for down in [0usize, 1, 2] {
            let mut by_q = open_changed_diff(&repo, id, down);
            let before = by_q
                .app
                .tab
                .project_return
                .as_ref()
                .and_then(|nav| nav.frame())
                .map(|f| f.cursor);
            press(&mut by_q, KeyCode::Char('q'));
            let q = landing(&by_q);

            let mut by_esc = open_changed_diff(&repo, id, down);
            press(&mut by_esc, KeyCode::Esc);
            let esc = landing(&by_esc);
            assert!(esc.active, "{id} row {down}: Esc returns to the project: {esc:?}");
            assert_eq!(esc.screen.as_deref(), Some("changes"), "{id} row {down}");
            assert_eq!(esc.cursor, before, "{id} row {down}: same cursor row as saved");
            assert_eq!(esc.cursor, Some(down), "{id} row {down}: the row we opened");
            assert!(!esc.has_return, "{id} row {down}: no stale project_return");
            assert!(!esc.tree, "{id} row {down}");
            assert_eq!(esc, q, "{id} row {down}: Esc and q land alike");
        }
    }
}

#[test]
fn vcs_keys_esc_after_cycling_changed_files_still_returns_to_the_project() {
    if isolated("vcs_keys_esc_after_cycling_changed_files_still_returns_to_the_project") {
        return;
    }
    for id in ["git", "jj"] {
        let Some(repo) = esc_repo(id, "vcs_keys_esc_cycle") else {
            continue;
        };
        let mut sim = open_changed_diff(&repo, id, 0);
        sim.press(KeyCode::Char('n'), KeyModifiers::CONTROL);
        drain(&mut sim);
        assert!(sim.app.is_git_diff_preview(), "{id}");
        press(&mut sim, KeyCode::Esc);
        let esc = landing(&sim);
        assert!(esc.active && !esc.has_return && !esc.tree, "{id}: {esc:?}");
        assert_eq!(esc.screen.as_deref(), Some("changes"), "{id}");
    }
}

/// One changed Markdown file (`doc.md`, the only row of the changes screen), whose diff can be
/// cycled with `R` into its ordinary `Preview` representation.
fn md_diff_repo(id: &str, name: &str) -> Option<crate::test_support::TmpDir> {
    let base = "# Title\n\nOriginal paragraph.\n";
    let changed = "# Title\n\nCHANGED paragraph.\n";
    if id == "git" {
        let dir = sandbox(name);
        crate::test_support::init_git_repo(&dir);
        std::fs::write(dir.join("doc.md"), base).unwrap();
        crate::test_support::run_git(&dir, &["add", "doc.md"]);
        crate::test_support::run_git(&dir, &["commit", "-qm", "seed"]);
        std::fs::write(dir.join("doc.md"), changed).unwrap();
        return Some(dir.into_canonical().unwrap());
    }
    use crate::test_support::{jj_scratch_bare, run_jj};
    if !crate::vcs::jj::available() {
        eprintln!("NOT RUN: {name}: jj is not installed; 0 jj cases");
        return None;
    }
    let dir = jj_scratch_bare(name).expect("jj is available: fixture must initialize");
    std::fs::write(dir.join("doc.md"), base).unwrap();
    assert!(run_jj(&dir, &["commit", "-m", "seed"]));
    std::fs::write(dir.join("doc.md"), changed).unwrap();
    Some(dir.into_canonical().unwrap())
}

/// The diff of `doc.md` cycled with `R` into its `Preview` representation.
fn open_diff_preview_representation(repo: &std::path::Path, id: &str) -> Sim {
    let mut sim = open_changed_diff(repo, id, 0);
    for _ in 0..4 {
        if sim.app.preview_from_diff_for_test() {
            break;
        }
        press(&mut sim, KeyCode::Char('R'));
    }
    assert!(
        sim.app.preview_from_diff_for_test() && !sim.app.is_git_diff_preview(),
        "{id}: R reaches the diff's own Preview representation"
    );
    assert!(sim.app.tab.project_return.is_some(), "{id}: the canvas is still saved");
    sim
}

#[test]
fn vcs_keys_esc_on_the_diff_preview_representation_returns_like_q() {
    if isolated("vcs_keys_esc_on_the_diff_preview_representation_returns_like_q") {
        return;
    }
    for id in ["git", "jj"] {
        let Some(repo) = md_diff_repo(id, "vcs_keys_esc_repr") else {
            continue;
        };
        let mut landings = Vec::new();
        for key in [KeyCode::Char('q'), KeyCode::Esc] {
            let mut sim = open_diff_preview_representation(&repo, id);
            press(&mut sim, key);
            let l = landing(&sim);
            assert!(l.active && !l.has_return && !l.tree, "{id} {key:?}: {l:?}");
            assert_eq!(l.screen.as_deref(), Some("changes"), "{id} {key:?}");
            assert_eq!(l.cursor, Some(0), "{id} {key:?}");
            landings.push(l);
        }
        assert_eq!(landings[0], landings[1], "{id}: q and Esc land alike");
    }
}

#[test]
fn vcs_keys_esc_on_a_diff_opened_from_the_tree_still_closes_to_the_tree() {
    if isolated("vcs_keys_esc_on_a_diff_opened_from_the_tree_still_closes_to_the_tree") {
        return;
    }
    let repo = super::vcs_changes::git_repo("vcs_keys_esc_tree", true);
    let mut sim = Sim::with_config_sized(&repo, config("git", "en"), 140, 30);
    sim.app.tab.projects.wait_for_result();
    sim.app.poll_projects();
    let file = repo.join("b-unstaged.txt");
    sim.app.open_git_diff(&file);
    assert!(sim.app.is_git_diff_preview());
    assert!(sim.app.tab.project_return.is_none() && !sim.app.project_active());
    press(&mut sim, KeyCode::Esc);
    let l = landing(&sim);
    assert!(l.tree && !l.active && !l.has_return, "{l:?}");
    assert!(!sim.app.is_git_diff_preview());
}

#[test]
fn vcs_keys_esc_clears_a_preview_search_first_then_returns_to_the_project() {
    if isolated("vcs_keys_esc_clears_a_preview_search_first_then_returns_to_the_project") {
        return;
    }
    for id in ["git", "jj"] {
        let Some(repo) = md_diff_repo(id, "vcs_keys_esc_search") else {
            continue;
        };
        let mut sim = open_diff_preview_representation(&repo, id);
        press(&mut sim, KeyCode::Char('/'));
        for c in "paragraph".chars() {
            press(&mut sim, KeyCode::Char(c));
        }
        press(&mut sim, KeyCode::Enter);
        assert_eq!(
            sim.app.preview_search_query(),
            Some("paragraph"),
            "{id}: the representation is searchable"
        );
        press(&mut sim, KeyCode::Esc);
        assert!(sim.app.preview_search_query().is_none(), "{id}: first Esc clears the search");
        assert!(!sim.app.project_active(), "{id}: first Esc stays in the preview");
        assert!(sim.app.tab.project_return.is_some(), "{id}: the return target is kept");
        press(&mut sim, KeyCode::Esc);
        let l = landing(&sim);
        assert!(l.active && !l.has_return && !l.tree, "{id}: {l:?}");
        assert_eq!(l.screen.as_deref(), Some("changes"), "{id}");
    }
}

// ---- a project screen in front hides the layers of the screen under it ----
// The status line's chip, the footer and the keys of the project screen come from one answer
// (`App::frontmost_layer`): a diff the screen was opened from, or the tree's changed-only filter,
// is under the screen and must not name the chip while the screen is in front.

/// A session on the tree of `repo` with the projects detected, and no project screen yet.
fn open_tree(repo: &std::path::Path, id: &str) -> Sim {
    let mut sim = Sim::with_config_sized(repo, config(id, "en"), 140, 30);
    sim.app.tab.projects.wait_for_result();
    sim.app.poll_projects();
    drain(&mut sim);
    assert!(!sim.app.project_active(), "{id}: tree first");
    sim
}

/// The project screen is the frontmost layer: no internal mode, the canvas keys, and the screen's
/// own chip is drawn (and none of the chips of the screen under it).
#[track_caller]
fn assert_project_screen_in_front(sim: &Sim, what: &str, not: &[&str]) {
    assert!(sim.app.project_active(), "{what}: project screen is up");
    assert_eq!(sim.app.internal_mode(), None, "{what}: internal_mode");
    assert_eq!(
        sim.app.surface(),
        crate::keymap::Surface::PreviewCanvas,
        "{what}: surface"
    );
    let screen = sim
        .app
        .project_vcs_chips()
        .and_then(|c| c.screen)
        .unwrap_or_else(|| panic!("{what}: the project has a screen chip"));
    let chips = mode_chips(sim);
    assert!(
        chips.iter().any(|c| *c == screen.text.trim()),
        "{what}: the screen's chip {:?} must be drawn: {chips:?}",
        screen.text
    );
    for word in not {
        assert!(
            chips.iter().all(|c| c != word),
            "{what}: the chip {word:?} of the screen under must not be drawn: {chips:?}"
        );
    }
}

#[test]
fn vcs_keys_returning_from_a_diff_shows_the_screens_chip_not_the_diffs() {
    if isolated("vcs_keys_returning_from_a_diff_shows_the_screens_chip_not_the_diffs") {
        return;
    }
    for id in ["git", "jj"] {
        let Some(repo) = esc_repo(id, "vcs_keys_return_chip") else {
            continue;
        };
        for (name, code) in [("q", KeyCode::Char('q')), ("Esc", KeyCode::Esc)] {
            let mut sim = open_changed_diff(&repo, id, 0);
            // The diff itself is named DIFF, with the diff's own layer in front.
            assert!(mode_chips(&sim).iter().any(|c| c == "DIFF"), "{id}");
            press(&mut sim, code);
            let what = format!("{id} {name}");
            assert_eq!(frame_screen(&sim), "changes", "{what}");
            // The diff preview is still the state under the screen (`return_to_project` restores
            // only the navigation); it must not answer for the screen.
            assert!(sim.app.is_git_diff_preview(), "{what}: the state under is kept");
            assert_project_screen_in_front(&sim, &what, &["DIFF"]);
            assert!(mode_chips(&sim).iter().any(|c| c == "CHANGES"), "{what}");
            // Leaving the screens: the tree, as it was opened from.
            press(&mut sim, KeyCode::Char('h'));
            assert!(!sim.app.project_active(), "{what}: left");
            assert_eq!(sim.app.tab.mode, Mode::Tree, "{what}: back on the tree");
            assert!(!sim.app.is_git_diff_preview(), "{what}");
            assert_eq!(sim.app.internal_mode(), None, "{what}");
            assert_eq!(sim.app.surface(), crate::keymap::Surface::Tree, "{what}");
        }
    }
}

#[test]
fn vcs_keys_a_screen_over_a_tree_diff_shows_the_screens_chip_and_leaving_shows_the_diff() {
    if isolated("vcs_keys_a_screen_over_a_tree_diff_shows_the_screens_chip_and_leaving_shows_the_diff") {
        return;
    }
    for (id, file) in [("git", "b-unstaged.txt"), ("jj", "b-two.txt")] {
        let Some(repo) = esc_repo(id, "vcs_keys_tree_diff_chip") else {
            continue;
        };
        let mut sim = open_tree(&repo, id);
        sim.select(file);
        sim.key('d');
        drain(&mut sim);
        assert!(sim.app.is_git_diff_preview(), "{id}: the tree's d opens the diff");
        assert_eq!(sim.app.internal_mode(), Some(crate::app::InternalMode::GitDiff), "{id}");
        assert!(mode_chips(&sim).iter().any(|c| c == "DIFF"), "{id}");
        sim.key('p');
        drain(&mut sim);
        assert_project_screen_in_front(&sim, &format!("{id} over a diff"), &["DIFF"]);
        // Leaving returns to the diff, and its chip is the diff's again.
        screen(&mut sim, "changes");
        press(&mut sim, KeyCode::Char('h'));
        assert!(!sim.app.project_active(), "{id}: left");
        assert!(sim.app.is_git_diff_preview(), "{id}: back on the diff");
        assert_eq!(sim.app.internal_mode(), Some(crate::app::InternalMode::GitDiff), "{id}");
        assert!(mode_chips(&sim).iter().any(|c| c == "DIFF"), "{id}");
    }
}

#[test]
fn vcs_keys_a_screen_over_the_changed_only_filter_shows_the_screens_chip() {
    if isolated("vcs_keys_a_screen_over_the_changed_only_filter_shows_the_screens_chip") {
        return;
    }
    for id in ["git", "jj"] {
        let Some(repo) = esc_repo(id, "vcs_keys_changed_filter_chip") else {
            continue;
        };
        let mut sim = open_tree(&repo, id);
        sim.app.toggle_changed_filter();
        sim.draw();
        assert!(sim.app.changed_filter(), "{id}: the filter is on");
        assert_eq!(
            sim.app.internal_mode(),
            Some(crate::app::InternalMode::ChangedFilter),
            "{id}"
        );
        assert!(mode_chips(&sim).iter().any(|c| c == "CHANGED"), "{id}");
        sim.key('p');
        drain(&mut sim);
        assert!(sim.app.changed_filter(), "{id}: the state under is kept");
        assert_project_screen_in_front(&sim, &format!("{id} over the filter"), &["CHANGED"]);
        screen(&mut sim, "changes");
        press(&mut sim, KeyCode::Char('h'));
        assert!(!sim.app.project_active(), "{id}: left");
        assert!(sim.app.changed_filter(), "{id}: the filter is still on");
        assert_eq!(
            sim.app.internal_mode(),
            Some(crate::app::InternalMode::ChangedFilter),
            "{id}"
        );
        assert!(mode_chips(&sim).iter().any(|c| c == "CHANGED"), "{id}");
    }
}

// ---- the canvas keys never act on the screen under the project screen ----
// A project screen opened from a file preview leaves that preview under it. Its layers are masked
// (`App::frontmost_layer`), so a key resolved on the canvas must not open or act on them either:
// the gate, the footer, the help and the draw answer as the walk does.

const HEADINGS_MD: &str = "# Title\n\nintro\n\n## Section\n\nbody\n";

/// A session whose preview shows a Markdown file with headings, then the project screen opened
/// over it with `p`.
fn project_over_md_preview(repo: &std::path::Path, id: &str) -> Sim {
    std::fs::write(repo.join("notes.md"), HEADINGS_MD).unwrap();
    let mut sim = open_tree(repo, id);
    sim.select("notes.md");
    sim.enter();
    drain(&mut sim);
    assert_eq!(sim.app.tab.mode, Mode::Preview, "{id}: preview first");
    assert!(!sim.app.project_active(), "{id}");
    sim.key('p');
    drain(&mut sim);
    assert!(sim.app.project_active(), "{id}: p opens the project over the preview");
    sim
}

/// The state the keys below must leave alone, read from the screen under the project screen.
fn under_state(sim: &Sim) -> (Option<std::path::PathBuf>, Mode, bool, bool, bool, bool, usize) {
    (
        sim.app.tab.preview_path.clone(),
        sim.app.tab.mode,
        sim.app.is_outline(),
        sim.app.is_marking(),
        sim.app.is_raw_source(),
        sim.app.is_visual(),
        sim.app.tab.selected,
    )
}

/// The actions (among the candidates of the preview under the screen) whose help description is
/// on a row of the real `?` help of the front screen: the same sections the overlay draws
/// (`help::sections`, which reaches `project_sections` over a project screen).
fn real_help_words(sim: &Sim) -> Vec<crate::keymap::Action> {
    use crate::keymap::Action as A;
    let rows: Vec<String> = crate::ops::help::sections(&sim.app)
        .into_iter()
        .flat_map(|s| s.rows.into_iter().map(|(_, desc)| desc))
        .collect();
    assert!(!rows.is_empty(), "the project screen's help has rows");
    [
        A::ToggleOutline,
        A::CycleRepresentation,
        A::PreviewCopySelectionRef,
        A::MarkSet,
        A::PreviewFileNext,
        A::PreviewFilePrev,
    ]
    .into_iter()
    .filter(|a| {
        crate::ops::decl::decl(*a)
            .help
            .is_some_and(|m| rows.iter().any(|d| d == crate::i18n::tr(sim.app.lang, m)))
    })
    .collect()
}

#[test]
fn vcs_keys_o_over_a_preview_opens_no_outline_and_is_not_advertised() {
    if isolated("vcs_keys_o_over_a_preview_opens_no_outline_and_is_not_advertised") {
        return;
    }
    for id in ["git", "jj"] {
        let Some(repo) = esc_repo(id, "vcs_keys_o_outline") else {
            continue;
        };
        let mut sim = project_over_md_preview(&repo, id);
        let hint = crate::i18n::tr(sim.app.lang, crate::i18n::Msg::HintOutline);
        let before = under_state(&sim);
        sim.app.flash = None;
        press(&mut sim, KeyCode::Char('O'));
        assert!(!sim.app.is_outline(), "{id}: O opened the outline under the screen");
        assert_eq!(under_state(&sim), before, "{id}");
        assert!(sim.app.flash.is_none(), "{id}: no flash from the screen under: {:?}", sim.app.flash);
        assert_project_screen_in_front(&sim, &format!("{id} after O"), &[]);
        // The gate, the footer and the help of the canvas do not offer it.
        assert!(
            !crate::ops::action_enabled(&sim.app, Context::Canvas, crate::keymap::Action::ToggleOutline),
            "{id}: gate"
        );
        let footer = crate::ops::footer::tokens(&sim.app, Context::Canvas).join(" ");
        assert!(!footer.contains(hint), "{id}: footer {footer:?} names {hint:?}");
        assert!(
            !real_help_words(&sim).contains(&crate::keymap::Action::ToggleOutline),
            "{id}: the `?` help of the project screen lists the outline"
        );
        // Leaving the screens returns to the preview, where O opens its outline as before.
        press(&mut sim, KeyCode::Char('h'));
        assert!(!sim.app.project_active(), "{id}: left");
        assert!(
            crate::ops::action_enabled(&sim.app, Context::Text, crate::keymap::Action::ToggleOutline)
                || crate::ops::action_enabled(&sim.app, Context::Document, crate::keymap::Action::ToggleOutline),
            "{id}: the preview offers the outline again"
        );
        press(&mut sim, KeyCode::Char('O'));
        assert!(sim.app.is_outline(), "{id}: O opens the outline on the preview");
    }
}

#[test]
fn vcs_keys_the_other_canvas_keys_do_not_act_on_the_preview_under() {
    if isolated("vcs_keys_the_other_canvas_keys_do_not_act_on_the_preview_under") {
        return;
    }
    // R cycles the representation, Y copies the selection's reference, m waits for a mark letter,
    // Ctrl-n / Ctrl-p turn to the next / previous file: all of the preview under the screen.
    let keys: [(&str, KeyCode, KeyModifiers, crate::keymap::Action); 5] = [
        ("R", KeyCode::Char('R'), KeyModifiers::NONE, crate::keymap::Action::CycleRepresentation),
        ("Y", KeyCode::Char('Y'), KeyModifiers::NONE, crate::keymap::Action::PreviewCopySelectionRef),
        ("m", KeyCode::Char('m'), KeyModifiers::NONE, crate::keymap::Action::MarkSet),
        ("C-n", KeyCode::Char('n'), KeyModifiers::CONTROL, crate::keymap::Action::PreviewFileNext),
        ("C-p", KeyCode::Char('p'), KeyModifiers::CONTROL, crate::keymap::Action::PreviewFilePrev),
    ];
    for id in ["git", "jj"] {
        let Some(repo) = esc_repo(id, "vcs_keys_other_under") else {
            continue;
        };
        for (name, code, mods, action) in keys {
            let mut sim = project_over_md_preview(&repo, id);
            let what = format!("{id} {name}");
            // The key resolves to the action on the canvas, and the gate refuses it there.
            let kp = crate::keymap::KeyPress::norm(&crossterm::event::KeyEvent::new(code, mods));
            assert_eq!(
                sim.app.keymaps.resolve_on(Context::Canvas, sim.app.screen_keys_for(Context::Canvas), kp),
                crate::keymap::Resolution::Action(action),
                "{what}: resolves on the canvas"
            );
            let before = under_state(&sim);
            crate::test_support::clear_test_clipboard();
            sim.app.flash = None;
            sim.press(code, mods);
            drain(&mut sim);
            assert_eq!(under_state(&sim), before, "{what}: the screen under changed");
            assert_eq!(crate::test_support::get_test_clipboard(), None, "{what}: clipboard");
            assert!(sim.app.flash.is_none(), "{what}: flash {:?}", sim.app.flash);
            assert_project_screen_in_front(&sim, &what, &[]);
            assert!(!crate::ops::action_enabled(&sim.app, Context::Canvas, action), "{what}: gate");
            assert!(!real_help_words(&sim).contains(&action), "{what}: the `?` help lists it");
        }
    }
}

/// Every name the config parser accepts for an action (`keymap::action_from_str`): the shared
/// table of the core actions and the host's own match arms, read from the parser's source, so an
/// action added to the parser is walked without touching this test.
fn every_assignable_action() -> Vec<(String, crate::keymap::Action)> {
    let src = include_str!("../keymap.rs");
    let start = src.find("pub fn action_from_str").expect("parser");
    let end = start + src[start..].find("pub fn action_name").expect("end of parser");
    let mut names: Vec<String> = konoma_host_core::keymap::CoreAction::all()
        .into_iter()
        .map(|c| c.name())
        .collect();
    names.extend(konoma_host_core::keymap::HostOp::ALL.into_iter().map(|h| h.name().to_string()));
    for line in src[start..end].lines() {
        let t = line.trim_start();
        let Some(rest) = t.strip_prefix('"') else {
            continue;
        };
        let Some(q) = rest.find('"') else { continue };
        if rest[q + 1..].trim_start().starts_with("=>")
            || rest[q + 1..].trim_start().starts_with("| \"")
        {
            names.push(rest[..q].to_string());
        }
    }
    names.sort();
    names.dedup();
    names
        .into_iter()
        .filter_map(|n| crate::keymap::action_from_str(&n).map(|a| (n, a)))
        .collect()
}

#[test]
fn vcs_keys_only_the_listed_actions_stay_on_over_a_project_screen() {
    if isolated("vcs_keys_only_the_listed_actions_stay_on_over_a_project_screen") {
        return;
    }
    let Some(repo) = esc_repo("git", "vcs_keys_allow_list") else {
        return;
    };
    let sim = project_over_md_preview(&repo, "git");
    let all = every_assignable_action();
    // A floor on the walk: the parser has well over a hundred names.
    assert!(all.len() > 100, "only {} action names were walked", all.len());
    // Over the project screen the gate allows an explicit list; every other action is off, by
    // name, whatever key the config binds it to. Some listed actions have a rule of their own
    // (a tab count, a launch target), so a listed action may still be off.
    let allowed: &[&str] = &[
        "noop",
        "tab_new",
        "tab_new_after",
        "tab_close",
        "close_tab_or_quit",
        "tab_prev",
        "tab_next",
        "open_in_new_tab",
        "toggle_tab_list",
        "toggle_help",
        "toggle_follow",
        "paste_jump",
        "quit",
        "refresh",
        "preview_back",
        "open_git_view",
        "request_edit",
        "open_external",
        "mark_jump",
        "git_launch_tool",
        // The old hub names, read for the open project when bound on a canvas.
        "git_open_log",
        "git_open_graph",
        "git_open_branches",
        "git_open_worktrees",
        "git_worktree_diff",
        "git_commit",
        "git_stage_all",
        "git_unstage_all",
        "jj_sync",
        "git_graph_toggle_all",
        "branch_create",
        "worktree_create",
        "worktree_close",
    ];
    let motion = |n: &str| {
        matches!(
            crate::keymap::action_from_str(n),
            Some(crate::keymap::Action::Navigate(_))
        )
    };
    let mut on_names = Vec::new();
    for (name, action) in &all {
        let on = crate::ops::gate(&sim.app, Context::Canvas, *action).is_on();
        if on {
            on_names.push(name.clone());
        }
        assert!(
            !on || allowed.contains(&name.as_str()) || motion(name),
            "{name} ({action:?}) is on over a project screen: it can act on the screen under"
        );
    }
    // The actions the config cannot name (`tab_goto` is fixed to the digit keys) are reached
    // through the default keymaps: only the tab switch may be on among them.
    let named: Vec<crate::keymap::Action> = all.iter().map(|(_, a)| *a).collect();
    let mut bound: Vec<crate::keymap::Action> = Vec::new();
    {
        let km = &sim.app.keymaps;
        let mut grab = |m: &std::collections::HashMap<crate::keymap::KeyPress, crate::keymap::Binding>| {
            for b in m.values() {
                if let crate::keymap::Binding::Run(a) = b {
                    bound.push(*a);
                }
            }
        };
        km.contexts.values().for_each(&mut grab);
        km.types.values().for_each(&mut grab);
        grab(&km.basic);
        grab(&km.global);
    }
    let mut unnamed_on = 0;
    for a in bound.into_iter().filter(|a| !named.contains(a)) {
        if crate::ops::gate(&sim.app, Context::Canvas, a).is_on() {
            assert!(
                matches!(a, crate::keymap::Action::TabGoto(_)),
                "{a:?} has no config name and is on over a project screen"
            );
            unnamed_on += 1;
        }
    }
    assert!(unnamed_on >= 9, "the digit keys stay on: {unnamed_on}");
    // Floor on the on side, so an allow list emptied by mistake cannot pass: these have no rule
    // of their own, so they are on.
    for name in [
        "tab_new",
        "tab_new_after",
        "toggle_tab_list",
        "toggle_help",
        "toggle_follow",
        "paste_jump",
        "quit",
        "refresh",
        "preview_back",
        "mark_jump",
    ] {
        assert!(on_names.iter().any(|n| n == name), "{name} must stay on: {on_names:?}");
    }
    // The names that the preview under owns are off.
    for name in [
        "preview_copy_selection",
        "preview_copy_selection_ref",
        "search_start",
        "toggle_table_cell",
        "toggle_outline",
        "page_next",
        "page_prev",
        "cycle_representation",
        "mark_set",
        "preview_next_file",
        "preview_prev_file",
        "preview_enter_visual",
        "toggle_wrap",
    ] {
        assert!(
            all.iter().any(|(n, _)| n == name),
            "{name} is missing from the parser walk"
        );
        assert!(!on_names.iter().any(|n| n == name), "{name} is on over a project screen");
    }
}

#[test]
fn vcs_keys_a_config_remap_to_a_preview_action_does_not_reach_the_preview_under() {
    if isolated("vcs_keys_a_config_remap_to_a_preview_action_does_not_reach_the_preview_under") {
        return;
    }
    let Some(repo) = esc_repo("git", "vcs_keys_remap") else {
        return;
    };
    std::fs::write(repo.join("notes.md"), HEADINGS_MD).unwrap();
    let mut cfg = config("git", "en");
    cfg.keys.surfaces.entry("canvas".to_string()).or_default().insert(
        "ctrl-a".to_string(),
        "preview_copy_selection".to_string(),
    );
    let mut sim = Sim::with_config_sized(&repo, cfg, 140, 30);
    sim.app.tab.projects.wait_for_result();
    sim.app.poll_projects();
    drain(&mut sim);
    sim.select("notes.md");
    sim.enter();
    drain(&mut sim);
    assert_eq!(sim.app.tab.mode, Mode::Preview);
    // The binding is live on the preview itself: Ctrl-a copies there.
    let kp = crate::keymap::KeyPress::norm(&crossterm::event::KeyEvent::new(
        KeyCode::Char('a'),
        KeyModifiers::CONTROL,
    ));
    assert_eq!(
        sim.app.keymaps.resolve_on(Context::Canvas, sim.app.screen_keys_for(Context::Canvas), kp),
        crate::keymap::Resolution::Action(crate::keymap::Action::PreviewCopySelection),
        "the remap is live on the canvas"
    );
    sim.key('p');
    drain(&mut sim);
    assert!(sim.app.project_active(), "p opens the project over the preview");
    crate::test_support::clear_test_clipboard();
    let before = under_state(&sim);
    sim.app.flash = None;
    sim.press(KeyCode::Char('a'), KeyModifiers::CONTROL);
    drain(&mut sim);
    assert_eq!(crate::test_support::get_test_clipboard(), None, "the hidden line was copied");
    assert_eq!(under_state(&sim), before);
    assert!(sim.app.flash.is_none(), "flash {:?}", sim.app.flash);
    assert_project_screen_in_front(&sim, "after the remapped key", &[]);
}

/// A session on `repo` with the preview of `file` up (media loaded), then the project screen over it.
fn project_over_file_preview(repo: &std::path::Path, file: &str) -> Sim {
    let mut sim = Sim::with_config_sized(repo, config("git", "en"), 140, 30);
    sim = sim.with_media();
    sim.app.tab.projects.wait_for_result();
    sim.app.poll_projects();
    drain(&mut sim);
    sim.select(file);
    sim.enter();
    assert_eq!(sim.app.tab.mode, Mode::Preview, "{file}: preview first");
    sim.key('p');
    drain(&mut sim);
    assert!(sim.app.project_active(), "{file}: p opens the project over the preview");
    sim
}

#[test]
fn vcs_keys_a_table_cell_popup_left_open_is_not_drawn_over_a_project_screen() {
    if isolated("vcs_keys_a_table_cell_popup_left_open_is_not_drawn_over_a_project_screen") {
        return;
    }
    let Some(repo) = esc_repo("git", "vcs_keys_cell") else {
        return;
    };
    std::fs::write(repo.join("t.csv"), "name,value\nalpha,1\nbeta,2\n").unwrap();
    let mut sim = project_over_file_preview(&repo, "t.csv");
    sim.app.toggle_table_cell_view();
    assert!(sim.app.is_table_cell_open(), "the popup state is left open under the screen");
    assert!(sim.app.table_cell_view().is_some(), "the table has a cell to show");
    sim.draw();
    let title = crate::i18n::tr(sim.app.lang, crate::i18n::Msg::TableCellTitle).trim();
    assert!(
        !sim.screen().contains(title),
        "the cell popup {title:?} is drawn over the project screen:\n{}",
        sim.screen()
    );
    assert_project_screen_in_front(&sim, "table cell state under", &[]);
}
