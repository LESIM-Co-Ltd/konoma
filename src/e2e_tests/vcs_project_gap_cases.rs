// Gap-closing literals (U9b1 fix-5b).
use super::vcs_log::graph::{fixture as graph_fixture, graph_sim, picker_fixture};

/// (lang, worktree-row `s`, copy s/h/t/m/a/d, base set, base cleared, picker HEAD toggle)
type FlashLiterals = (
    &'static str,
    &'static str,
    [&'static str; 6],
    &'static str,
    &'static str,
    &'static str,
);

#[test]
fn vcs_port_graph_flash_texts_are_literal_in_both_languages() {
    if isolated("vcs_port_graph_flash_texts_are_literal_in_both_languages") {
        return;
    }
    let literals: [FlashLiterals; 2] = [
        (
            "en",
            "select a commit to set as base",
            [
                "copied: 17c49bb",
                "copied: 17c49bb312cf67e02eb01ca8dae662f555196b89",
                "copied: Merge 日本語",
                "copied: Merge 日本語…",
                "copied: 作者 Author",
                "copied: 2020-01-01 00:03",
            ],
            "base: master",
            "base cleared",
            "current branch (HEAD) is always shown",
        ),
        (
            "ja",
            "基準にするコミットを選んでください",
            [
                "コピー: 17c49bb",
                "コピー: 17c49bb312cf67e02eb01ca8dae662f555196b89",
                "コピー: Merge 日本語",
                "コピー: Merge 日本語…",
                "コピー: 作者 Author",
                "コピー: 2020-01-01 00:03",
            ],
            "基準: master",
            "基準を解除しました",
            "現在ブランチ(HEAD)は常に表示されます",
        ),
    ];
    let dir = graph_fixture();
    for (lang, select, copies, set, cleared, _) in literals {
        let mut new = graph_sim(&dir, lang, "right", (160, 16), 12, false, true);
        new.key('s');
        drain(&mut new);
        assert_eq!(new.app.flash.as_deref(), Some(select), "{lang} new s");
        new.key('j');
        drain(&mut new);
        for (key, text) in ['s', 'h', 't', 'm', 'a', 'd'].into_iter().zip(copies) {
            clear_test_clipboard();
            new.key('y');
            new.key(key);
            drain(&mut new);
            assert_eq!(new.app.flash.as_deref(), Some(text), "{lang} new y{key}");
        }
        for (key, text) in [('s', Some(set)), ('x', Some(cleared)), ('x', None)] {
            new.key(key);
            drain(&mut new);
            assert_eq!(new.app.flash.as_deref(), text, "{lang} new {key}");
        }
        // Detail `s` toggles the diff layout and says so.
        new.press(KeyCode::Enter, KeyModifiers::NONE);
        drain(&mut new);
        new.key('s');
        drain(&mut new);
        let side = if lang == "en" {
            "diff: side-by-side"
        } else {
            "diff: 横並び"
        };
        assert_eq!(new.app.flash.as_deref(), Some(side), "{lang} new detail s");
    }
    let dir = picker_fixture();
    for (lang, _, _, _, _, head) in literals {
        let mut new = graph_sim(&dir, lang, "right", (160, 16), 12, true, false);
        // Only the HEAD toggle speaks; every other picker key is silent.
        for (key, speaks) in [
            ('b', false),
            ('x', false),
            ('j', false),
            ('x', true),
            ('a', false),
            ('c', false),
            ('J', false),
            ('K', false),
            ('G', false),
            ('k', false),
            ('g', false),
        ] {
            new.key(key);
            drain(&mut new);
            let want = speaks.then_some(head);
            assert_eq!(new.app.flash.as_deref(), want, "{lang} new picker {key}");
        }
    }
}

/// Opens the project through konoma's real config loader (`Config::load_path_reporting`) with
/// `text` as the config file, so the old `[ui]` names go through the same path as a user file.
fn open_with_config_text(
    root: &std::path::Path,
    text: &str,
    size: (u16, u16),
) -> (Sim, Vec<(String, String)>) {
    let home = crate::test_support::unique_tmp("vcs_gap_config");
    std::fs::create_dir_all(&home).unwrap();
    let path = home.join("config.toml");
    std::fs::write(&path, text).unwrap();
    let (mut cfg, error) = Config::load_path_reporting(&path);
    assert_eq!(error, None, "{text}");
    let renamed = cfg.renamed_settings.clone();
    cfg.ui.lang = "en".into();
    cfg.ui.tabbar = "hidden".into();
    cfg.ui.statusbar = "split".into();
    cfg.plugins
        .entry("jj".into())
        .or_default()
        .insert("enabled".into(), toml::Value::Boolean(false));
    (open(root, cfg, size), renamed)
}

#[test]
fn vcs_port_old_ui_names_reach_the_new_graph_and_log_through_the_real_config_path() {
    if isolated("vcs_port_old_ui_names_reach_the_new_graph_and_log_through_the_real_config_path")
    {
        return;
    }
    let root = sandbox("port_old_ui_names");
    seed_repo_history(&root);
    for name in ["b1", "b2", "b3"] {
        crate::test_support::run_git(&root, &["branch", name]);
    }
    crate::test_support::run_git(&root, &["config", "user.name", "AuthorZZZ"]);
    crate::test_support::run_git(&root, &["commit", "-q", "--allow-empty", "-m", "shortsubj"]);
    let base = crate::git::branch(&root).unwrap();
    // graph_max_branches: the old name caps the graph, 0 lifts the cap.
    for (old_text, new_text, hidden) in [
        ("[ui]\ngraph_max_branches = 2\n", "[plugins.git]\ngraph_max_branches = 2\n", true),
        ("[ui]\ngraph_max_branches = 0\n", "[plugins.git]\ngraph_max_branches = 0\n", false),
    ] {
        let (mut old, renamed) = open_with_config_text(&root, old_text, (180, 30));
        assert!(
            renamed.iter().any(|(from, _)| from.contains("graph_max_branches")),
            "the old name is reported as renamed: {renamed:?}"
        );
        let (mut new, _) = open_with_config_text(&root, new_text, (180, 30));
        for sim in [&mut old, &mut new] {
            screen(sim, "graph");
            if hidden {
                sim.see("hidden)");
            } else {
                sim.dont_see("hidden)");
            }
        }
        assert_eq!(old.project_frame(), new.project_frame(), "{old_text}");
    }
    // graph_base_branches: the old name pins the base, the default does not.
    let text = format!("[ui]\ngraph_base_branches = [\"{base}\"]\n");
    let (mut pinned, renamed) = open_with_config_text(&root, &text, (180, 30));
    assert!(renamed.iter().any(|(from, _)| from.contains("graph_base_branches")));
    screen(&mut pinned, "graph");
    pinned.see("⌖ base:");
    let (mut plain, _) = open_with_config_text(&root, "", (180, 30));
    screen(&mut plain, "graph");
    plain.dont_see("⌖ base:");
    // commit_meta_align: the old name moves the author column in the log.
    let mut columns = Vec::new();
    for align in ["right", "inline"] {
        let text = format!("[ui]\ncommit_meta_align = \"{align}\"\n");
        let (mut sim, renamed) = open_with_config_text(&root, &text, (180, 24));
        assert!(renamed.iter().any(|(from, _)| from.contains("commit_meta_align")));
        screen(&mut sim, "log");
        columns.push(sim.find_text("AuthorZZZ").unwrap().1);
    }
    assert!(columns[0] > columns[1], "old-name author columns {columns:?}");
}

/// Visible rows of the current list as drawn text (cells joined), connectors included.
fn shown_row_texts(sim: &Sim) -> Vec<String> {
    let content = sim.app.project_content().unwrap();
    let ProjectContent::List(list) = content.as_ref() else {
        panic!("list required")
    };
    sim.app
        .project_visible_rows()
        .into_iter()
        .map(|i| {
            list.rows[i]
                .cells
                .iter()
                .map(|c| c.plain())
                .collect::<Vec<_>>()
                .join("")
        })
        .collect()
}

#[test]
fn vcs_port_graph_filter_rows_per_query_and_short_height_scroll() {
    if isolated("vcs_port_graph_filter_rows_per_query_and_short_height_scroll") {
        return;
    }
    let dir = graph_fixture();
    for lang in ["en", "ja"] {
        for width in [80u16, 160] {
            for (align, cap, priority, icons) in [
                ("right", 12, false, true),
                ("inline", 12, false, true),
                ("right", 1, false, false),
                ("right", 0, true, false),
            ] {
                let mut cfg = config("git", lang);
                cfg.ui.icons = icons;
                let settings = cfg.plugins.entry("git".into()).or_default();
                settings.insert("commit_meta_align".into(), toml::Value::String(align.into()));
                settings.insert("graph_max_branches".into(), toml::Value::Integer(cap));
                if priority {
                    settings.insert(
                        "graph_base_branches".into(),
                        toml::Value::Array(vec![
                            toml::Value::String("missing-branch".into()),
                            toml::Value::String("priority-branch-with-a-long-name".into()),
                        ]),
                    );
                }
                let mut sim = open(&dir, cfg, (width, 16));
                screen(&mut sim, "graph");
                keys(&mut sim, "jj");
                for query in ["日本語", "Main", "nothing-matches", "│"] {
                    keys(&mut sim, "/");
                    keys(&mut sim, query);
                    press(&mut sim, KeyCode::Enter);
                    let rows = shown_row_texts(&sim);
                    let ctx = format!("{lang} {width} {align}/{cap}/{priority}/{icons} {query:?}: {rows:?}");
                    // The filter drops connector rows and matches the text a row shows.
                    assert!(rows.iter().all(|r| !r.trim_start().starts_with("├")), "{ctx}");
                    let selected = sim.app.project_act_context().selected;
                    match query {
                        "nothing-matches" | "│" => {
                            // A glyph that only the lane drawing carries matches no row text.
                            assert!(rows.is_empty(), "{ctx}");
                            assert!(selected.is_none(), "{ctx}");
                        }
                        "Main" => {
                            assert_eq!(rows.len(), 1, "{ctx}");
                            assert!(rows[0].contains("Main commit"), "{ctx}");
                            assert_eq!(
                                selected.as_ref().map(|s| s.value.as_str()),
                                shown_values(&sim).first().map(String::as_str),
                                "{ctx}"
                            );
                        }
                        _ => {
                            // 日本語: every shown row carries the query; the cursor sits on a shown row.
                            assert!(!rows.is_empty(), "{ctx}");
                            assert!(rows.iter().all(|r| r.contains(query)), "{ctx}");
                            assert!(
                                shown_values(&sim).contains(&selected.unwrap().value),
                                "{ctx}"
                            );
                        }
                    }
                    if cap == 12 && align == "right" {
                        // The default configuration shows the whole fixture: pin the exact rows.
                        match query {
                            "日本語" => {
                                assert_eq!(rows.len(), 3, "{ctx}");
                                assert!(rows[0].ends_with("Merge 日本語"), "{ctx}");
                                assert!(rows[1].ends_with("Side 日本語"), "{ctx}");
                                assert!(rows[2].contains("0 日本語の長い件名"), "{ctx}");
                                assert_eq!(
                                    shown_values(&sim)[0],
                                    "17c49bb312cf67e02eb01ca8dae662f555196b89",
                                    "{ctx}"
                                );
                            }
                            "Main" => assert_eq!(
                                shown_values(&sim),
                                ["3b7da10f3a1eb3a7ff483292b7f5498ea3c7b9f4"],
                                "{ctx}"
                            ),
                            _ => {}
                        }
                    }
                }
            }
        }
    }
    // Short heights: G lands on the last row and g on the first, and the selected row is drawn.
    for width in [80u16, 160] {
        for height in [5u16, 6, 7, 8] {
            let mut sim = open(&dir, config("git", "en"), (width, height));
            screen(&mut sim, "graph");
            let values = shown_values(&sim);
            let texts = shown_row_texts(&sim);
            keys(&mut sim, "G");
            let ctx = format!("{width}x{height}");
            let last = selected_value(&sim);
            assert_eq!(last, *values.last().unwrap(), "{ctx}: G lands on the last row");
            // The last row is drawn (its short id is in the frame) and the view scrolled past the top.
            sim.see(&last[..7]);
            sim.dont_see("Uncommitted changes");
            keys(&mut sim, "g");
            assert_eq!(selected_value(&sim), values[0], "{ctx}: g lands on the first row");
            sim.see("Uncommitted changes");
            sim.dont_see(&last[..7]);
            assert!(texts.len() > 4, "{ctx}");
        }
    }
}

#[test]
fn vcs_port_q_returns_to_origin_and_l_g_are_list_keys_on_log_and_graph() {
    if isolated("vcs_port_q_returns_to_origin_and_l_g_are_list_keys_on_log_and_graph") {
        return;
    }
    let root = sandbox("port_q_origin");
    seed_repo_history(&root);
    for name in ["log", "graph"] {
        let mut sim = open(&root, config("git", "en"), (160, 20));
        // The old hub reached the log with `l` and the graph with `g`; the project reaches them
        // with Tab, and `l` / `g` keep their list meaning there (open detail / go to the top).
        assert_eq!(sim.app.project_frame().unwrap().key.screen, "changes");
        screen(&mut sim, name);
        assert_eq!(sim.app.project_frame().unwrap().key.screen, name);
        keys(&mut sim, "G");
        keys(&mut sim, "g");
        assert_eq!(sim.app.project_frame().unwrap().key.screen, name, "g stays on {name}");
        assert_eq!(
            selected_value(&sim),
            shown_values(&sim)[0],
            "g is the top of the {name} list"
        );
        keys(&mut sim, "l");
        assert_eq!(sim.app.project_frame().unwrap().key.screen, "detail", "l opens detail");
        // q on a pushed screen goes back one level, to the list.
        keys(&mut sim, "q");
        assert_eq!(sim.app.project_frame().unwrap().key.screen, name);
        assert!(sim.app.project_active(), "q on a pushed screen stays in the project");
        // q at the root of the project returns to where `p` was pressed: the tree, which is
        // re-derived from disk on the way back (a file created meanwhile is listed).
        let late = format!("late_{name}.txt");
        std::fs::write(root.join(&late), "x\n").unwrap();
        assert!(
            !sim.app.tab.entries.iter().any(|e| e.path.ends_with(&late)),
            "the tree has not seen {late} before the return"
        );
        keys(&mut sim, "q");
        assert!(!sim.app.project_active(), "root q leaves the project from {name}");
        assert_eq!(sim.app.tab.mode, crate::app::Mode::Tree, "back at the origin");
        assert!(
            sim.app.tab.entries.iter().any(|e| e.path.ends_with(&late)),
            "the return re-derives the tree from disk ({name})"
        );
    }
}

#[test]
fn vcs_port_filter_input_line_opens_and_closes_and_dirty_bottom_row_base_pin() {
    if isolated("vcs_port_filter_input_line_opens_and_closes_and_dirty_bottom_row_base_pin") {
        return;
    }
    // The `/` input line opens and closes on every filterable screen. The old hub switched to
    // Surface::ListFilter while it was open; the same fact is held here by the single `Canvas`
    // key context with the input line open on the canvas's own screen: the typed keys go to the
    // line (a typed `q` filters and does not leave the project) and `Esc` closes it.
    let root = sandbox("port_filter_surface");
    seed_repo_history(&root);
    std::fs::write(root.join("new.txt"), "x\n").unwrap();
    let mut sim = open(&root, config("git", "en"), (160, 20));
    for name in ["changes", "log", "graph", "branches"] {
        screen(&mut sim, name);
        assert!(!sim.app.project_frame().unwrap().filtering, "{name} before /");
        keys(&mut sim, "/");
        assert!(sim.app.project_frame().unwrap().filtering, "{name}");
        // The key context stays the single `Canvas`; the screen is the canvas's own.
        assert_eq!(sim.app.key_context(), Some(crate::keymap::Context::Canvas), "{name}");
        assert_eq!(
            sim.app.canvas_screen(),
            Some(crate::keymap::Context::Screen(
                crate::keymap::Backend::Git,
                match name {
                    "changes" => crate::keymap::Screen::Changes,
                    "log" => crate::keymap::Screen::Log,
                    "graph" => crate::keymap::Screen::Graph,
                    _ => crate::keymap::Screen::Branches,
                }
            )),
            "{name}: the input line is open on the canvas's screen"
        );
        // The line takes the keys: `q` is text, not "leave", and the screen does not change.
        keys(&mut sim, "q");
        assert!(sim.app.project_active(), "{name}: a typed q does not leave the project");
        assert!(sim.app.project_frame().unwrap().filtering, "{name}: still typing");
        assert_eq!(sim.app.project_frame().unwrap().query, "q", "{name}: q went to the filter");
        assert_eq!(sim.app.project_frame().unwrap().key.screen, name, "{name}");
        press(&mut sim, KeyCode::Esc);
        assert!(!sim.app.project_frame().unwrap().filtering, "{name}");
        assert!(sim.app.project_active(), "{name}: Esc closes the line only");
    }
    // The base pin on the bottom row of a dirty repository (the working-tree row is first).
    let dirty = sandbox("port_base_dirty");
    seed_repo(&dirty);
    let mut sim = open(&dirty, config("git", "en"), (160, 20));
    screen(&mut sim, "graph");
    sim.dont_see("⌖ base:");
    assert_eq!(real_list(&sim).rows[0].kind, "worktree");
    for zero in [false, true] {
        keys(&mut sim, "G");
        let bottom = selected_value(&sim);
        assert_ne!(bottom, shown_values(&sim)[0], "the bottom row is not the working-tree row");
        keys(&mut sim, "s");
        sim.see("⌖ base:");
        assert_eq!(selected_value(&sim), bottom, "s keeps the selection");
        if zero {
            keys(&mut sim, "0");
            sim.see("⌖ base:");
        }
        keys(&mut sim, "x");
        sim.dont_see("⌖ base:");
        assert_eq!(selected_value(&sim), bottom, "x keeps the selection");
    }
}

#[test]
fn vcs_port_unborn_after_open_then_tab_shows_the_empty_log() {
    if isolated("vcs_port_unborn_after_open_then_tab_shows_the_empty_log") {
        return;
    }
    for lang in ["en", "ja"] {
        let root = super::vcs_log::repo(1);
        let mut sim = open(&root, config("git", lang), (80, 8));
        // Remove the only ref after the project is open: HEAD becomes unborn before the cycle.
        let repository = konoma_vcs_core::git2::Repository::open(&root).unwrap();
        let head = repository.head().unwrap().name().unwrap().to_string();
        repository.find_reference(&head).unwrap().delete().unwrap();
        press(&mut sim, KeyCode::Tab);
        assert_eq!(sim.app.project_frame().unwrap().key.screen, "log");
        assert!(sim.app.project_act_context().selected.is_none());
        let list = real_list(&sim);
        assert_eq!(list.rows.len(), 1);
        assert!(!list.rows[0].selectable);
        assert_eq!(
            list.rows[0].cells[0].plain(),
            if lang == "en" { "  (no commits)" } else { "  (コミットなし)" }
        );
        press(&mut sim, KeyCode::Enter);
        assert!(!sim.app.project_pushed());
    }
}

#[test]
fn vcs_port_branch_footer_enter_token_per_row() {
    if isolated("vcs_port_branch_footer_enter_token_per_row") {
        return;
    }
    let root = sandbox("port_branch_enter");
    seed_repo_history(&root);
    let mut sim = open(&root, config("git", "en"), (160, 20));
    screen(&mut sim, "branches");
    for other in [false, true] {
        if other {
            keys(&mut sim, "j");
        }
        let tokens = crate::ui::status::hint_tokens(&sim.app);
        assert_eq!(
            tokens,
            // The old hub's branches footer (`vcs_port_every_screen_footer_equals_old_hub`).
            [
                "j/k:nav",
                "l/Enter:checkout",
                "c:new",
                "d:delete",
                "/:search",
                "Tab/⇧Tab:screens",
                "h/q/Esc:back"
            ],
            "branches footer, other row = {other}"
        );
    }
}

// ---- old git_{view,log,graph,branch}_move_extremes_no_panic (one test per list) ----
// The old hub clamped its own selection index; the fact it protected — a move past either end of a
// list never panics and stays on the first/last row — holds for every list screen of the canvas.
#[test]
fn vcs_port_list_cursor_clamps_at_both_ends_of_every_list_screen() {
    if isolated("vcs_port_list_cursor_clamps_at_both_ends_of_every_list_screen") {
        return;
    }
    let root = sandbox("vcs_port_clamp_ends");
    seed_repo_history(&root);
    // Two uncommitted files give the changes screen two rows and the graph a working-tree row.
    std::fs::write(root.join("a.rs"), "fn a() { 33 }\n").unwrap();
    std::fs::write(root.join("new.txt"), "new\n").unwrap();
    let mut sim = open(&root, config("git", "en"), (160, 20));
    for name in ["changes", "log", "graph", "branches"] {
        screen(&mut sim, name);
        keys(&mut sim, "g");
        let first = selected_value(&sim);
        keys(&mut sim, "k");
        assert_eq!(
            selected_value(&sim),
            first,
            "{name}: k above the top stays on the first row"
        );
        keys(&mut sim, "G");
        let last = selected_value(&sim);
        assert_ne!(last, first, "{name}: G reaches a different row");
        keys(&mut sim, "j");
        assert_eq!(
            selected_value(&sim),
            last,
            "{name}: j below the bottom stays on the last row"
        );
        keys(&mut sim, "g");
        assert_eq!(selected_value(&sim), first, "{name}: g returns to the top");
    }
}

// ---- old stale_listing_chip_shows_in_the_git_view ----
// The chip comes from the shared `context_spans`, which the project canvas's status bar draws too.
#[test]
fn vcs_port_stale_listing_chip_shows_on_the_project_canvas() {
    if isolated("vcs_port_stale_listing_chip_shows_on_the_project_canvas") {
        return;
    }
    let root = sandbox("vcs_port_stale_chip");
    crate::test_support::init_git_repo(&root);
    let mut sim = open(&root, config("git", "en"), (120, 20));
    assert!(
        !sim.screen().contains("STALE"),
        "a healthy canvas shows no stale chip"
    );
    std::fs::remove_dir_all(&root).unwrap();
    sim.app.refresh_fs_watched(false, &[]);
    assert!(sim.app.tree_stale(), "sanity: the root removal marked it");
    sim.draw();
    assert!(
        sim.screen().contains("STALE"),
        "the stale chip is drawn on the project canvas"
    );
}

// ---- old git_view_branch_re_verifies_when_switching_to_a_different_repo ----
// Each tab's canvas title is re-verified against that tab's repository, not the previously
// active tab's: tab A stays "alpha-branch", tab B stays "beta-branch", across switches.
#[test]
fn vcs_port_each_tabs_project_title_reverifies_its_repo_branch() {
    if isolated("vcs_port_each_tabs_project_title_reverifies_its_repo_branch") {
        return;
    }
    let parent = crate::test_support::unique_tmp("vcs_port_branch_title_tabs");
    for (name, branch, file) in [
        ("a", "alpha-branch", "only-a.txt"),
        ("b", "beta-branch", "only-b.txt"),
    ] {
        let root = parent.join(name);
        std::fs::create_dir_all(&root).unwrap();
        crate::test_support::init_git_repo(&root);
        std::fs::write(root.join(file), "x\n").unwrap();
        crate::test_support::run_git(&root, &["add", "-A"]);
        crate::test_support::run_git(&root, &["commit", "-qm", "init"]);
        crate::test_support::run_git(&root, &["checkout", "-q", "-b", branch]);
        std::fs::write(root.join(file), "x\nchanged\n").unwrap();
    }
    let mut sim = Sim::with_config_sized(&parent, config("git", "en"), 160, 20);
    let enter_project = |sim: &mut Sim, dir: &str| {
        sim.select(dir);
        sim.key('l');
        sim.app.tab.projects.wait_for_result();
        sim.app.poll_projects();
        sim.key('p');
        drain(sim);
        assert!(sim.app.project_active(), "{dir}: project opened");
    };
    enter_project(&mut sim, "/a");
    assert!(sim.screen().contains("alpha-branch"), "{}", sim.screen());
    assert!(sim.screen().contains("only-a.txt"), "{}", sim.screen());
    // The second tab starts as a tree at the first repo's root; go up, into repo b.
    sim.app.tab_new().unwrap();
    sim.key('h');
    enter_project(&mut sim, "/b");
    assert!(sim.screen().contains("beta-branch"), "{}", sim.screen());
    assert!(sim.screen().contains("only-b.txt"), "{}", sim.screen());
    assert!(
        !sim.screen().contains("alpha-branch"),
        "{}",
        sim.screen()
    );
    for _ in 0..2 {
        sim.app.tab_goto(0);
        drain(&mut sim);
        assert!(
            sim.screen().contains("alpha-branch"),
            "tab A's own branch: {}",
            sim.screen()
        );
        assert!(
            !sim.screen().contains("beta-branch"),
            "{}",
            sim.screen()
        );
        sim.app.tab_goto(1);
        drain(&mut sim);
        assert!(
            sim.screen().contains("beta-branch"),
            "tab B's own branch: {}",
            sim.screen()
        );
    }
}


/// A project root that is deleted is not a detection failure: the host shows no "could not
/// detect or watch projects" flash, `p` refuses exactly like in a directory with no project, and
/// once the root exists again as a repository `p` opens the project screen.
#[test]
fn vcs_port_deleted_project_root_is_no_project_and_comes_back() {
    if isolated("vcs_port_deleted_project_root_is_no_project_and_comes_back") {
        return;
    }
    let failed = tr(crate::i18n::Lang::En, Msg::ProjectDetectionFailed);
    // What `p` says (and does) where there is no project at all.
    let plain = unique_tmp("vcs_port_deleted_root_plain");
    std::fs::create_dir_all(&plain).unwrap();
    std::fs::write(plain.join("a.txt"), "x\n").unwrap();
    let mut none = Sim::with_config_sized(&plain, config("git", "en"), 100, 20);
    none.settle_projects();
    none.key('p');
    drain(&mut none);
    assert!(!none.app.project_active());
    let refusal = none.app.flash.clone();
    assert!(refusal.is_some(), "p refuses without a project");

    let repo = super::vcs_changes::git_repo("vcs_port_deleted_root", false);
    let root = repo.to_path_buf();
    let mut sim = Sim::with_config_sized(&root, config("git", "en"), 100, 20);
    sim.settle_projects();
    assert_eq!(sim.app.tab.projects.projects.len(), 1);
    assert!(sim.app.tab.projects.uses_injected_events());

    std::fs::remove_dir_all(&root).unwrap();
    sim.app
        .tab
        .projects
        .inject_event(Ok(notify::Event::new(notify::EventKind::Remove(
            notify::event::RemoveKind::Any,
        ))
        .add_path(root.join(".git"))));
    sim.app.poll_projects();
    drain(&mut sim);
    assert_ne!(sim.app.flash.as_deref(), Some(failed), "no failure flash");
    assert!(sim.app.tab.projects.projects.is_empty());
    sim.key('p');
    drain(&mut sim);
    assert!(!sim.app.project_active());
    assert_eq!(sim.app.flash, refusal, "p refuses like with no project");
    assert_ne!(sim.app.flash.as_deref(), Some(failed));

    // The root is created again, as a repository.
    std::fs::create_dir_all(&root).unwrap();
    crate::test_support::init_git_repo(&root);
    sim.app
        .tab
        .projects
        .inject_event(Ok(notify::Event::new(notify::EventKind::Create(
            notify::event::CreateKind::Any,
        ))
        .add_path(root.join(".git"))));
    sim.app.poll_projects();
    drain(&mut sim);
    assert_eq!(sim.app.tab.projects.projects.len(), 1);
    sim.key('p');
    drain(&mut sim);
    assert!(sim.app.project_active(), "{:?}", sim.app.flash);
}

/// A project that is no longer detected while its screens are open: the screens close like leaving
/// them from the top screen (back to the tree), and the flash is what `p` says where there is no
/// project. Both ways a project can vanish: the whole root removed, and only `.git` removed.
#[test]
fn vcs_port_open_project_screens_close_when_the_project_vanishes() {
    if isolated("vcs_port_open_project_screens_close_when_the_project_vanishes") {
        return;
    }
    // What `p` says where there is no project at all.
    let plain = unique_tmp("vcs_port_vanish_plain");
    std::fs::create_dir_all(&plain).unwrap();
    std::fs::write(plain.join("a.txt"), "x\n").unwrap();
    let mut none = Sim::with_config_sized(&plain, config("git", "en"), 100, 20);
    none.settle_projects();
    none.key('p');
    drain(&mut none);
    let refusal = none.app.flash.clone();
    assert!(refusal.is_some(), "p refuses without a project");

    for whole_root in [true, false] {
        let name = if whole_root {
            "vcs_port_vanish_root"
        } else {
            "vcs_port_vanish_dot_git"
        };
        let repo = super::vcs_changes::git_repo(name, false);
        let root = repo.to_path_buf();
        let mut sim = Sim::with_config_sized(&root, config("git", "en"), 100, 20);
        sim.settle_projects();
        sim.key('p');
        drain(&mut sim);
        assert!(sim.app.project_active(), "{name}: p opens the screens");
        sim.app.flash = None;

        if whole_root {
            std::fs::remove_dir_all(&root).unwrap();
        } else {
            std::fs::remove_dir_all(root.join(".git")).unwrap();
        }
        sim.app
            .tab
            .projects
            .inject_event(Ok(notify::Event::new(notify::EventKind::Remove(
                notify::event::RemoveKind::Any,
            ))
            .add_path(root.join(".git"))));
        sim.app.poll_projects();
        drain(&mut sim);
        assert!(sim.app.tab.projects.projects.is_empty(), "{name}");
        assert!(
            !sim.app.project_active(),
            "{name}: the screens of a vanished project are closed"
        );
        assert_eq!(sim.app.tab.mode, Mode::Tree, "{name}: back at the tree");
        assert_eq!(sim.app.flash, refusal, "{name}: the no-project message");
    }
}
