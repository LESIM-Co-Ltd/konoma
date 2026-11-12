// Literal contracts collected from the remaining legacy entry points.

fn real_list(sim: &Sim) -> konoma_plugin::view::ListView {
    let content = sim.app.project_content().unwrap();
    let ProjectContent::List(list) = content.as_ref() else {
        panic!("list required")
    };
    list.clone()
}

#[test]
fn vcs_port_git_a_is_inert_and_jj_a_keeps_literal_notices() {
    if isolated("vcs_port_git_a_is_inert_and_jj_a_keeps_literal_notices") {
        return;
    }
    let git = super::vcs_log::repo(3);
    for lang in ["en", "ja"] {
        let mut sim = open(&git, config("git", lang), (180, 22));
        screen(&mut sim, "graph");
        let before = sim.project_frame();
        let list = real_list(&sim);
        let selected = sim.app.project_act_context().selected;
        let nav_before = super::vcs_app_parity::host_state(&sim);
        keys(&mut sim, "a");
        assert_eq!(sim.app.flash, None, "Git a must be silent");
        // Draw first: the host refreshes a view's stored rows while drawing, so the snapshot
        // taken after the draw also covers a change to the list itself.
        let after = sim.project_frame();
        assert_eq!(
            super::vcs_app_parity::host_state(&sim),
            nav_before,
            "Git a leaves the whole host navigation state (frame key, cursor, dialog, menu, chip, target, rows) unchanged"
        );
        assert_eq!(after, before, "Git a cells unchanged");
        assert_eq!(real_list(&sim), list, "Git a list state unchanged");
        assert_eq!(sim.app.project_act_context().selected, selected);
        assert!(crate::ops::footer::tokens(&sim.app, Context::Canvas)
            .iter()
            .all(|t| !t.split_once(':').unwrap().0.split('/').any(|k| k == "a")));
        assert!(crate::ops::help::sections(&sim.app)
            .iter()
            .flat_map(|s| &s.rows)
            .all(|(k, _)| k != "a"));
        let menu = crate::ops::menu::menu(
            &sim.app,
            Context::Canvas,
            LeaderId::Actions,
            crate::ops::menu::View::Open,
        )
        .unwrap();
        assert!(
            menu.items.iter().all(|i| !i.label.contains("revision")),
            "Git Space has no graph_all"
        );
        // Action membership, not wording: no item runs the toggle action, built-in or plugin.
        // (Menu letters are assigned by the menu, so the item key is not the action's key.)
        assert!(
            menu.items.iter().all(|i| match i.action {
                crate::keymap::Action::GitGraphToggleAll => false,
                crate::keymap::Action::Plugin(p) => p.act() != "graph_all",
                _ => true,
            }),
            "Git Space actions must not contain the graph toggle: {:?}",
            menu.items.iter().map(|i| i.action).collect::<Vec<_>>()
        );
    }
    // Only the jj half needs the jj binary; the Git half above has already run.
    if !crate::vcs::jj::available() {
        return;
    }
    let jj = super::vcs_log::graph_jj_fixture();
    for lang in ["en", "ja"] {
        let mut sim = open(&jj, config("jj", lang), (180, 22));
        screen(&mut sim, "graph");
        let original = shown_values(&sim);
        for (all, message) in [
            (true, Msg::GraphShowingAll),
            (false, Msg::GraphShowingDefault),
        ] {
            keys(&mut sim, "a");
            assert_eq!(
                sim.app.flash.as_deref(),
                Some(tr(sim.app.lang, message)),
                "jj literal range notice"
            );
            if all {
                assert!(shown_values(&sim).len() > original.len());
            } else {
                assert_eq!(shown_values(&sim), original);
            }
        }
        let before = real_list(&sim);
        for key in ['s', 'x'] {
            press(&mut sim, KeyCode::Char(key));
            assert_eq!(real_list(&sim), before, "jj has no base key {key}");
        }
    }
}

#[test]
fn vcs_port_file_head_baseline_right_remaps_and_diff_layout() {
    if isolated("vcs_port_file_head_baseline_right_remaps_and_diff_layout") {
        return;
    }
    let root = super::vcs_changes::git_repo("vcs_finish_head", true);
    assert!(crate::git::changed_files(&root)[0].staged);
    let expected = crate::git::file_diff(&root, &root.join("a-staged.txt"));
    for word in ["base", "worktree"] {
        assert!(expected.iter().any(|l| l.text.contains(word)));
    }
    assert!(!expected.iter().any(|l| l.text.contains("index")));
    for (code, remap) in [
        (KeyCode::Enter, false),
        (KeyCode::Char('l'), false),
        (KeyCode::Right, false),
        (KeyCode::Char('Z'), true),
    ] {
        let mut cfg = config("git", "en");
        if remap {
            cfg.keys.surfaces.insert(
                "text".into(),
                [
                    ("Z".into(), "navigate:right".into()),
                    ("l".into(), "noop".into()),
                ]
                .into(),
            );
        }
        let mut sim = open(&root, cfg, (140, 20));
        if remap {
            keys(&mut sim, "l");
            assert!(sim.app.project_active());
        }
        press(&mut sim, code);
        assert!(sim.app.is_git_diff_preview());
        assert!(!sim.app.project_active());
        assert_eq!(sim.app.tab.preview_path, Some(root.join("a-staged.txt")));
        assert_eq!(
            sim.app
                .git_diff_lines()
                .into_iter()
                .map(|l| (l.kind, l.text))
                .collect::<Vec<_>>(),
            expected
                .iter()
                .map(|l| (l.kind, l.text.clone()))
                .collect::<Vec<_>>(),
            "HEAD baseline {code:?}"
        );
    }
    let root = super::vcs_log::detail_repo();
    std::fs::write(root.join("main.rs"), "changed\n").unwrap();
    for table in ["git.detail", "text"] {
        let mut cfg = config("git", "en");
        cfg.keys.surfaces.insert(
            "diff".into(),
            [
                ("S".into(), "cycle_diff_layout".into()),
                ("s".into(), "noop".into()),
            ]
            .into(),
        );
        cfg.keys.surfaces.insert(
            table.into(),
            [
                ("S".into(), "noop".into()),
                ("s".into(), "cycle_diff_layout".into()),
            ]
            .into(),
        );
        let mut sim = open(&root, cfg, (80, 12));
        screen(&mut sim, "log");
        press(&mut sim, KeyCode::Enter);
        keys(&mut sim, "s");
        assert!(!sim.app.diff_is_split(78), "detail s must be noop {table}");
        keys(&mut sim, "S");
        assert!(sim.app.diff_is_split(78), "detail S splits {table}");
        assert!(sim.app.project_controls().unwrap().contains("S:"));
        assert!(crate::ops::help::sections(&sim.app)
            .iter()
            .flat_map(|s| &s.rows)
            .any(|(k, _)| k == "S"));
        keys(&mut sim, "q");
        screen(&mut sim, "changes");
        press(&mut sim, KeyCode::Enter);
        assert!(sim.app.is_git_diff_preview() && !sim.app.project_active());
        assert_eq!(sim.app.key_context(), Some(Context::Diff));
        let before = sim.app.diff_is_split(78);
        keys(&mut sim, "s");
        assert_eq!(sim.app.diff_is_split(78), before);
        keys(&mut sim, "S");
        assert_ne!(sim.app.diff_is_split(78), before);
        assert!(crate::ops::footer::tokens(&sim.app, Context::Diff)
            .iter()
            .any(|t| t.starts_with("S:")));
        assert!(crate::ops::help::sections(&sim.app)
            .iter()
            .flat_map(|s| &s.rows)
            .any(|(k, _)| k == "S"));
    }
}

#[test]
fn vcs_port_unborn_both_languages_fixed_tabs_and_empty_row() {
    if isolated("vcs_port_unborn_both_languages_fixed_tabs_and_empty_row") {
        return;
    }
    for lang in ["en", "ja"] {
        let root = super::vcs_log::repo(0);
        let mut sim = open(&root, config("git", lang), (80, 8));
        for name in [
            "changes",
            "log",
            "graph",
            "branches",
            "worktrees",
            "changes",
        ] {
            screen(&mut sim, name);
            assert_eq!(sim.app.project_frame().unwrap().key.screen, name);
            if name == "log" {
                assert!(sim.app.project_act_context().selected.is_none());
                let list = real_list(&sim);
                assert_eq!(list.rows.len(), 1);
                assert!(!list.rows[0].selectable);
                assert_eq!(
                    list.rows[0].cells[0].plain(),
                    if lang == "en" {
                        "  (no commits)"
                    } else {
                        "  (コミットなし)"
                    }
                );
                let frame = sim.project_frame();
                assert!(frame[(2, 2)].modifier.contains(Modifier::DIM));
                press(&mut sim, KeyCode::Enter);
                assert!(!sim.app.project_pushed());
            }
        }
    }
}

#[test]
fn vcs_port_tree_keys_refresh_and_meta_alignment_literals() {
    if isolated("vcs_port_tree_keys_refresh_and_meta_alignment_literals") {
        return;
    }
    let root = sandbox("vcs_finish_no_repo");
    seed_files(&root);
    let mut sim = Sim::with_config(&canon(&root), config("git", "en"));
    keys(&mut sim, "s");
    assert_eq!(sim.app.pending_leader, None);
    assert_eq!(sim.app.key_context(), Some(Context::Tree));
    let before = sim.app.path_style;
    keys(&mut sim, "o");
    assert!(!sim.app.project_active());
    keys(&mut sim, "p");
    assert_eq!(sim.app.path_style, before);
    assert!(
        !sim.app.project_active(),
        "p outside a repository does not open a project"
    );
    assert_eq!(sim.app.tab.mode, crate::app::Mode::Tree);
    let root = sandbox("vcs_finish_refresh");
    seed_repo_history(&root);
    let mut sim = open(&root, config("git", "en"), (180, 24));
    screen(&mut sim, "log");
    assert_eq!(
        crate::git::commit_meta(&root, &real_list(&sim).rows[0].value)
            .unwrap()
            .message
            .trim(),
        "gammacommit"
    );
    crate::test_support::run_git(
        &root,
        &["commit", "-q", "--allow-empty", "-m", "deltacommit"],
    );
    keys(&mut sim, "r");
    assert_eq!(
        crate::git::commit_meta(&root, &real_list(&sim).rows[0].value)
            .unwrap()
            .message
            .trim(),
        "deltacommit"
    );
    screen(&mut sim, "branches");
    assert!(!shown_values(&sim).contains(&"fresh-branch".into()));
    crate::test_support::run_git(&root, &["branch", "fresh-branch"]);
    keys(&mut sim, "r");
    assert!(shown_values(&sim).contains(&"fresh-branch".into()));
    crate::test_support::run_git(&root, &["config", "user.name", "AuthorZZZ"]);
    crate::test_support::run_git(&root, &["commit", "-q", "--allow-empty", "-m", "shortsubj"]);
    let mut columns = Vec::new();
    for align in ["right", "inline"] {
        let mut cfg = config("git", "en");
        cfg.plugins.entry("git".into()).or_default().insert(
            "commit_meta_align".into(),
            toml::Value::String(align.into()),
        );
        let mut sim = open(&root, cfg, (180, 24));
        screen(&mut sim, "log");
        columns.push(sim.find_text("AuthorZZZ").unwrap().1);
    }
    assert!(
        columns[0] > columns[1],
        "author right versus inline columns {columns:?}"
    );
}

#[test]
fn vcs_port_full_commit_header_literal_lines_and_styles() {
    if isolated("vcs_port_full_commit_header_literal_lines_and_styles") {
        return;
    }
    let root = super::vcs_log::repo(0);
    let repo = konoma_vcs_core::git2::Repository::open(&root).unwrap();
    let tree = repo.treebuilder(None).unwrap().write().unwrap();
    let tree = repo.find_tree(tree).unwrap();
    let signature = konoma_vcs_core::git2::Signature::new(
        "Alice",
        "fixture@example.invalid",
        &konoma_vcs_core::git2::Time::new(1_782_691_200, 0),
    )
    .unwrap();
    let oid = repo
        .commit(
            Some("HEAD"),
            &signature,
            &signature,
            "subject line\n\nbody paragraph one\nbody paragraph two",
            &tree,
            &[],
        )
        .unwrap();
    let meta = crate::git::commit_meta(&root, &oid.to_string()).unwrap();
    assert!(meta.date.starts_with("2026-06-29"));
    let mut sim = open(&root, config("git", "en"), (80, 16));
    screen(&mut sim, "log");
    press(&mut sim, KeyCode::Enter);
    let content = sim.app.project_content().unwrap();
    let ProjectContent::Diff(diff) = content.as_ref() else {
        panic!("diff required")
    };
    let header = &diff.header;
    let plain = |line: &ratatui::text::Line<'_>| {
        line.spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect::<String>()
    };
    let texts = header.iter().map(plain).collect::<Vec<_>>();
    for value in [&meta.short, &"Alice".to_string(), &meta.date] {
        assert!(texts[0].contains(value));
    }
    for text in [
        "subject line",
        "body paragraph one",
        "body paragraph two",
        "",
    ] {
        assert!(texts.iter().any(|s| s == text));
    }
    let subject = header.iter().find(|l| plain(l) == "subject line").unwrap();
    assert!(subject
        .spans
        .iter()
        .any(|s| s.style.add_modifier.contains(Modifier::BOLD)));
    let frame = sim.project_frame();
    assert!(
        frame
            .content()
            .iter()
            .any(|c| c.symbol() == "─" && c.fg == Color::DarkGray),
        "dark gray header divider"
    );
}

fn assert_space_contract(sim: &Sim) {
    use crate::keymap::Action;
    let ctx = sim.app.key_context().unwrap();
    let declared = crate::ops::menu::declared_actions_menu(&sim.app, ctx);
    let mut want: Vec<Action> = Vec::new();
    let priority = if let Some((presentation, actions)) = sim.app.project_presentation() {
        presentation
            .footer_priority()
            .iter()
            .map(|op| {
                let konoma_plugin::host::project::presentation::PresentationOp::ProjectAction(i) =
                    op;
                (Action::Plugin(actions[*i]), String::new())
            })
            .collect()
    } else {
        crate::ops::footer::priority(&sim.app, ctx)
            .into_iter()
            .map(|(a, _)| (a, String::new()))
            .collect::<Vec<_>>()
    };
    for (action, _) in priority {
        if declared.iter().any(|i| i.action == action) && !want.contains(&action) {
            want.push(action);
        }
    }
    // The old minimum-three assertion on Log is pending as project-space-shared-operations.
    if sim
        .app
        .project_frame()
        .is_none_or(|f| f.key.screen != "log")
    {
        assert!(
            want.len() >= 3,
            "enough footer operations {ctx:?}: {want:?}; menu={declared:?}\n{}",
            sim.screen()
        );
    }
    assert_eq!(
        declared
            .iter()
            .take(want.len())
            .map(|i| i.action)
            .collect::<Vec<_>>(),
        want
    );
    let menu = crate::ops::menu::menu(
        &sim.app,
        ctx,
        LeaderId::Actions,
        crate::ops::menu::View::Open,
    )
    .unwrap();
    let open = menu.items.iter().map(|i| i.action).collect::<Vec<_>>();
    let open_want = want
        .iter()
        .copied()
        .filter(|a| open.contains(a))
        .collect::<Vec<_>>();
    assert_eq!(
        open.iter()
            .copied()
            .take(open_want.len())
            .collect::<Vec<_>>(),
        open_want
    );
    if let Some((presentation, actions)) = sim.app.project_presentation() {
        for token in presentation.footer_tokens() {
            let Some((_, label)) = token.text.split_once(':') else {
                continue;
            };
            for op in &token.ops {
                let konoma_plugin::host::project::presentation::PresentationOp::ProjectAction(i) =
                    op;
                let action = Action::Plugin(actions[*i]);
                if let Some(item) = declared.iter().find(|item| item.action == action) {
                    let expected = crate::ops::decl::menu_label(&sim.app, ctx, action)
                        .map(|m| tr(sim.app.lang, m))
                        .unwrap_or(label);
                    assert_eq!(item.label, expected, "project footer and Space label");
                }
            }
        }
    }
    for token in
        crate::ops::footer::render_tokens(&sim.app, ctx, &crate::ops::footer::plan(&sim.app, ctx))
    {
        let Some((_, label)) = token.text.split_once(':') else {
            continue;
        };
        for action in token.actions {
            if let Some(item) = declared.iter().find(|i| i.action == action) {
                let expected = crate::ops::decl::menu_label(&sim.app, ctx, action)
                    .map(|m| tr(sim.app.lang, m))
                    .unwrap_or(label);
                assert_eq!(item.label, expected);
            }
        }
    }
    for item in declared {
        if let Some(help) = crate::ops::decl::text(&sim.app, ctx, item.action).help {
            let long = tr(sim.app.lang, help);
            assert!(
                item.label == long
                    || item.label.chars().count() < long.chars().count()
                    || long.chars().count() <= 24
            );
        }
        assert!(item.label.chars().count() <= 26);
    }
}

#[test]
fn vcs_port_copy_contexts_and_space_footer_order() {
    if isolated("vcs_port_copy_contexts_and_space_footer_order") {
        return;
    }
    let root = sandbox("vcs_finish_menus");
    seed_repo(&root);
    std::fs::write(root.join("t.csv"), "a,b\n1,2\n").unwrap();
    let mut sim = Sim::with_config(&canon(&root), config("git", "en"));
    let mut seen = Vec::new();
    for (name, leader) in [
        ("tree", LeaderId::Copy),
        ("table", LeaderId::TableCopy),
        ("branches", LeaderId::Copy),
        ("log", LeaderId::Copy),
    ] {
        if name == "table" {
            sim.select("t.csv");
            sim.enter();
        }
        if name == "branches" {
            sim.key('q');
            sim.app.tab.projects.wait_for_result();
            sim.app.poll_projects();
            sim.key('p');
            drain(&mut sim);
            screen(&mut sim, "branches");
        }
        if name == "log" {
            screen(&mut sim, "log");
        }
        keys(&mut sim, "y");
        assert_eq!(sim.app.pending_leader, Some(leader));
        let menu = crate::ops::menu::menu(
            &sim.app,
            sim.app.key_context().unwrap(),
            leader,
            crate::ops::menu::View::Open,
        )
        .unwrap();
        seen.push(menu.items.into_iter().map(|i| i.key).collect::<Vec<_>>());
        press(&mut sim, KeyCode::Esc);
        assert_eq!(sim.app.pending_leader, None);
    }
    for (i, a) in seen.iter().enumerate() {
        for b in &seen[i + 1..] {
            assert_ne!(a, b);
        }
    }
    let mut checked = Vec::new();
    for name in ["changes", "log", "graph", "branches", "worktrees"] {
        screen(&mut sim, name);
        assert_space_contract(&sim);
        checked.push(name);
    }
    // Every screen the project declares is checked: the loop above covers the whole Tab cycle.
    assert_eq!(checked.len(), 5, "all five Git screens are in the Space check");
    let first = sim.app.project_frame().unwrap().key.screen.clone();
    for _ in 0..checked.len() {
        press(&mut sim, KeyCode::Tab);
    }
    assert_eq!(
        sim.app.project_frame().unwrap().key.screen,
        first,
        "five Tabs walk exactly the five checked screens"
    );
    screen(&mut sim, "changes");
    press(&mut sim, KeyCode::Enter);
    assert_eq!(sim.app.key_context(), Some(Context::Diff));
    assert_space_contract(&sim);
}

fn graph_help_literals(sim: &Sim, jj: bool) {
    let sections = crate::ops::help::sections(&sim.app);
    let title = tr(
        sim.app.lang,
        if jj {
            Msg::JjGraphLabel
        } else {
            Msg::GitGraphLabel
        },
    );
    let graph = sections
        .iter()
        .find(|s| s.title == title)
        .expect("graph help section");
    let mut messages = vec![Msg::TopBottom, Msg::GitDetail];
    if jj {
        messages.extend([Msg::JjGraphAllRow, Msg::JjBookmarksRow]);
    } else {
        messages.extend([
            Msg::GraphSetBaseHelp,
            Msg::GraphClearBaseHelp,
            Msg::GraphBranchesHelp,
        ]);
    }
    messages.extend([
        Msg::ListFilterHelp,
        Msg::Refresh,
        if jj {
            Msg::JjScreensHelp
        } else {
            Msg::GitScreensHelp
        },
        Msg::BackToChanges,
    ]);
    let mut expected = messages
        .into_iter()
        .map(|m| tr(sim.app.lang, m).to_owned())
        .collect::<Vec<_>>();
    // The move row sits in its own Movement section after the graph section.
    expected.push(tr(sim.app.lang, Msg::GitMoveCommit).to_owned());
    let mut actual = graph
        .rows
        .iter()
        .map(|(_, d)| d.clone())
        .chain(
            sections
                .iter()
                .filter(|s| s.title == tr(sim.app.lang, Msg::HelpMove))
                .flat_map(|s| s.rows.iter().map(|(_, d)| d.clone())),
        )
        .collect::<Vec<_>>();
    expected.sort();
    actual.sort();
    assert_eq!(actual, expected, "all literal graph help rows jj={jj}");
    // jj has no base pin or all-revisions toggle on its graph; Git has no "all revisions" row.
    let footer = crate::ui::status::hint_tokens(&sim.app).join("  ");
    let help_text = graph
        .rows
        .iter()
        .map(|(k, d)| format!("{k} {d}"))
        .collect::<Vec<_>>()
        .join("\n");
    if !jj {
        for needle in ["all revisions", "全リビジョン"] {
            assert!(
                !help_text.contains(needle) && !footer.contains(needle),
                "git graph has no {needle}"
            );
        }
    } else {
        assert!(!footer.contains("base"), "jj graph footer has no base token: {footer}");
        assert!(!help_text.contains("base"), "jj graph help has no base row");
    }
    // As in the v0.31.2 frame: the graph section, then Movement, then the leader sections.
    let mut titles = vec![title.to_owned(), tr(sim.app.lang, Msg::HelpMove).to_owned()];
    for (leader, key) in [(LeaderId::Copy, "y"), (LeaderId::Actions, "Space")] {
        if let Some(section) =
            crate::ui::help::leader_section(&sim.app, Context::Canvas, leader, key)
        {
            titles.push(section.title);
        }
    }
    assert_eq!(
        sections.iter().map(|s| s.title.clone()).collect::<Vec<_>>(),
        titles
    );
    let footer = crate::ops::footer::tokens(&sim.app, Context::Canvas);
    let expected = if jj {
        if sim.app.lang == crate::i18n::Lang::En {
            vec![
                "nav",
                "detail",
                "all revisions",
                "bookmarks",
                "search",
                "screens",
                "back",
            ]
        } else {
            vec![
                "移動",
                "詳細",
                "全リビジョン",
                "ブックマーク",
                "検索",
                "画面",
                "戻る",
            ]
        }
    } else if sim.app.lang == crate::i18n::Lang::En {
        vec![
            "nav", "detail", "base", "base off", "branches", "search", "screens", "back",
        ]
    } else {
        vec![
            "移動",
            "詳細",
            "基準",
            "基準解除",
            "ブランチ",
            "検索",
            "画面",
            "戻る",
        ]
    };
    assert_eq!(
        footer
            .iter()
            .map(|t| t.split_once(':').unwrap().1)
            .collect::<Vec<_>>(),
        expected
    );
    let has = |key| {
        footer
            .iter()
            .any(|t| t.split_once(':').unwrap().0.split('/').any(|k| k == key))
    };
    assert!(has("b"));
    if jj {
        assert!(!has("s") && !has("x"));
    }
}

#[test]
fn vcs_port_graph_picker_and_backend_help_exact_rows() {
    if isolated("vcs_port_graph_picker_and_backend_help_exact_rows") {
        return;
    }
    let root = super::vcs_log::graph_picker_fixture();
    let run = |root: &std::path::Path, id: &str, lang: &str| {
        {
            let mut sim = open(root, config(id, lang), (180, 28));
            screen(&mut sim, "graph");
            graph_help_literals(&sim, id == "jj");
            {
                keys(&mut sim, "b");
                let sections = crate::ops::help::sections(&sim.app);
                let section = sections
                    .iter()
                    .find(|s| s.title == tr(sim.app.lang, Msg::GraphPickerTitle))
                    .unwrap();
                let messages = [
                    Msg::GitMove,
                    Msg::TopBottom,
                    Msg::HintToggle,
                    Msg::FtReorder,
                    Msg::FtAll,
                    Msg::FtCurrentOnly,
                    Msg::FtApply,
                    Msg::FtCancel,
                ];
                assert_eq!(
                    section
                        .rows
                        .iter()
                        .map(|(_, d)| d.as_str())
                        .collect::<Vec<_>>(),
                    messages
                        .iter()
                        .map(|m| tr(sim.app.lang, *m))
                        .collect::<Vec<_>>()
                );
                assert_eq!(
                    section
                        .rows
                        .iter()
                        .map(|(k, _)| k.split_whitespace().collect::<String>())
                        .collect::<Vec<_>>(),
                    ["j/k", "g/G", "x", "J/K", "a", "c", "l/Enter", "h/Esc·q"]
                );
                let footer = crate::ops::footer::tokens(&sim.app, Context::Canvas);
                let expected = if lang == "en" {
                    vec![
                        "x:toggle",
                        "J/K:reorder",
                        "a:all",
                        "c:current only",
                        "l/Enter:apply",
                        "h/q:cancel",
                    ]
                } else {
                    vec![
                        "x:トグル",
                        "J/K:並替",
                        "a:全部",
                        "c:現在のみ",
                        "l/Enter:適用",
                        "h/q:取消",
                    ]
                };
                assert_eq!(footer, expected);
            }
        }
    };
    for lang in ["en", "ja"] {
        run(&root, "git", lang);
    }
    // Only the jj halves need the jj binary; the Git half above has already run.
    if !crate::vcs::jj::available() {
        return;
    }
    let jjroot = super::vcs_log::graph_jj_fixture();
    let colocated = super::vcs_log::jj_repo(true);
    for lang in ["en", "ja"] {
        run(&jjroot, "jj", lang);
        run(&colocated, "jj", lang);
    }
}

#[test]
fn vcs_port_renderer_normal_rows_markers_and_base_selection() {
    if isolated("vcs_port_renderer_normal_rows_markers_and_base_selection") {
        return;
    }
    let root = sandbox("vcs_finish_renderer");
    init_git_repo(&root);
    std::fs::write(root.join("a.txt"), "hi\n").unwrap();
    crate::test_support::run_git(&root, &["add", "-A"]);
    crate::test_support::run_git(&root, &["commit", "-q", "-m", "hello world commit"]);
    crate::test_support::run_git(&root, &["branch", "-M", "trunk"]);
    crate::test_support::run_git(&root, &["branch", "feature-x"]);
    std::fs::write(root.join("foo.txt"), "hi\n").unwrap();
    let mut columns = Vec::new();
    for icons in [true, false] {
        let mut cfg = config("git", "en");
        cfg.ui.icons = icons;
        let mut sim = open(&root, cfg, (180, 24));
        assert_eq!(sim.app.project_frame().unwrap().key.screen, "changes");
        sim.see("U");
        sim.see("foo.txt");
        screen(&mut sim, "log");
        sim.see("hello world commit");
        screen(&mut sim, "branches");
        sim.see("feature-x");
        sim.see("*");
        screen(&mut sim, "graph");
        keys(&mut sim, "G");
        let id = selected_value(&sim);
        keys(&mut sim, "s");
        assert_eq!(selected_value(&sim), id);
        let title = real_list(&sim).title.unwrap().plain();
        assert!(title.contains("base: trunk"));
        assert_eq!(title.matches("base:").count(), 1);
        assert_eq!(title.matches('⌖').count(), usize::from(icons));
        // The old test counted over the whole drawn frame, not only the title: the base glyph
        // appears exactly once (icons on, none otherwise), "base:" exactly once, and the base
        // branch name is on screen.
        // The `s` notice ("base: trunk") is a transient status flash that the old hub's frame
        // never carried; clear it so the count covers the drawn graph screen itself.
        assert_eq!(sim.app.flash.as_deref(), Some("base: trunk"));
        sim.app.flash = None;
        sim.draw();
        let frame = sim.screen();
        assert_eq!(
            frame.matches('⌖').count(),
            usize::from(icons),
            "whole-frame base glyph count (icons={icons}): {frame}"
        );
        assert_eq!(
            frame.matches("base:").count(),
            1,
            "whole-frame base: count (icons={icons}): {frame}"
        );
        assert!(frame.contains("trunk"), "base branch name on screen");
        let list = real_list(&sim);
        let chips = &list.bottom.as_ref().unwrap().chips;
        assert!(chips.iter().any(|l| l.plain().contains(if icons {
            "⎇ ●"
        } else {
            "br: ●"
        })));
        let full = sim.screen();
        assert!(!full.contains("⎇●") && !full.contains("⌖●"));
        keys(&mut sim, "x");
        assert!(!real_list(&sim).title.unwrap().plain().contains("base:"));
        keys(&mut sim, "b");
        sim.see("trunk");
        sim.see("feature-x");
        let current = sim.find_text("trunk").unwrap().1;
        let other = sim.find_text("feature-x").unwrap().1;
        assert_eq!(current, other);
        columns.push(current);
        if icons {
            sim.see("⎇");
        } else {
            sim.dont_see("⎇");
            sim.see("* ");
        }
    }
    assert_eq!(columns[0], columns[1]);
    let roles = real_graph_style_repository();
    crate::test_support::run_git(&roles, &["branch", "-m", "base", "trunk"]);
    let repo = konoma_vcs_core::git2::Repository::open(&roles).unwrap();
    let base = repo
        .find_reference("refs/heads/trunk")
        .unwrap()
        .target()
        .unwrap()
        .to_string();
    for icons in [true, false] {
        let mut cfg = config("git", "en");
        cfg.ui.icons = icons;
        cfg.plugins
            .entry("git".into())
            .or_default()
            .insert("graph_max_branches".into(), toml::Value::Integer(0));
        let mut sim = open(&roles, cfg, (180, 24));
        screen(&mut sim, "graph");
        for _ in 0..16 {
            if selected_value(&sim) == base {
                break;
            }
            keys(&mut sim, "j");
        }
        assert_eq!(selected_value(&sim), base);
        keys(&mut sim, "sg");
        let list = real_list(&sim);
        let chips = &list.bottom.as_ref().unwrap().chips;
        assert!(chips.iter().any(|l| l.plain()
            == if icons {
                "⎇ ● feature"
            } else {
                "br: ● feature"
            }));
        assert!(chips.iter().any(|l| l.plain()
            == if icons {
                "⌖ ● trunk"
            } else {
                "base: ● trunk"
            }));
        let text = sim.screen();
        assert!(!text.contains("⎇●") && !text.contains("⌖●"));
        if icons {
            assert!(text.contains("⎇ ●") && text.contains("⌖ ●"));
        } else {
            assert!(!text.contains('⎇') && !text.contains('⌖'));
            assert!(text.contains("br: ●") && text.contains("base: ●"));
        }
    }
    // Only the jj bookmarks part needs the jj binary; the Git part above has already run.
    if !crate::vcs::jj::available() {
        return;
    }
    let jjroot = super::vcs_log::jj_repo(false);
    super::vcs_log::jj(&jjroot, &["bookmark", "create", "-r", "@", "main"]);
    let mut sim = open(&jjroot, config("jj", "en"), (180, 24));
    screen(&mut sim, "branches");
    sim.see("main");
    sim.dont_see("*");
}

#[test]
fn vcs_port_backend_contexts_and_bindings_real_repositories() {
    if isolated("vcs_port_backend_contexts_and_bindings_real_repositories") {
        return;
    }
    let root = super::vcs_changes::git_repo("vcs_finish_context", true);
    let contexts = |root: &std::path::Path, id: &str| {
        use crate::keymap::{Backend, Screen};
        let backend = match id {
            "git" => Backend::Git,
            "jj" => Backend::Jj,
            other => panic!("unexpected backend {other}"),
        };
        let mut sim = open(root, config(id, "en"), (180, 24));
        for name in if id == "git" {
            vec!["changes", "log", "graph", "branches", "worktrees"]
        } else {
            vec!["changes", "log", "graph", "branches"]
        } {
            screen(&mut sim, name);
            // Old: Context::Screen(backend, screen). The expected screen is spelled out here, not
            // derived through the production name lookup.
            let expected = match name {
                "changes" => Screen::Changes,
                "log" => Screen::Log,
                "graph" => Screen::Graph,
                "branches" => Screen::Branches,
                "worktrees" => Screen::Worktrees,
                other => panic!("unexpected screen {other}"),
            };
            assert_eq!(
                sim.app.canvas_screen(),
                Some(Context::Screen(backend, expected)),
                "{id}/{name}"
            );
            assert_eq!(sim.app.key_context(), Some(Context::Canvas), "{id}/{name}");
            assert_eq!(
                sim.app
                    .tab
                    .project_navigation
                    .as_ref()
                    .unwrap()
                    .project
                    .plugin,
                id
            );
        }
        screen(&mut sim, "changes");
        press(&mut sim, KeyCode::Enter);
        assert!(sim.app.is_git_diff_preview());
        assert_eq!(sim.app.key_context(), Some(Context::Diff));
        keys(&mut sim, "q");
        screen(&mut sim, "log");
        press(&mut sim, KeyCode::Enter);
        assert_eq!(sim.app.project_frame().unwrap().key.screen, "detail");
        // Old: Context::Screen(backend, Detail).
        assert_eq!(
            sim.app.canvas_screen(),
            Some(Context::Screen(backend, Screen::Detail)),
            "{id}/detail"
        );
        assert_eq!(sim.app.key_context(), Some(Context::Canvas), "{id}/detail");
    };
    let binding = |table: &str, root: &std::path::Path, id: &str, expected: bool| {
        let mut cfg = config(id, "en");
        cfg.keys
            .surfaces
            .insert(table.into(), [("Z".into(), "toggle_help".into())].into());
        let mut sim = open(root, cfg, (180, 24));
        keys(&mut sim, "Z");
        // Old: the binding fires (Help opens) exactly on the backend the table names; the old
        // name `git_changes` reaches both.
        assert_eq!(
            sim.app.key_context() == Some(Context::Help),
            expected,
            "[keys.{table:?}] on {id}"
        );
    };
    contexts(&root, "git");
    for (table, on_git) in [("jj.changes", false), ("git.changes", true), ("git_changes", true)] {
        binding(table, &root, "git", on_git);
    }
    // Only the jj halves need the jj binary; the Git halves above have already run.
    if !crate::vcs::jj::available() {
        return;
    }
    let jjroot = super::vcs_log::jj_repo(false);
    std::fs::write(jjroot.join("changed.txt"), "new\n").unwrap();
    contexts(&jjroot, "jj");
    for (table, on_jj) in [("jj.changes", true), ("git.changes", false), ("git_changes", true)] {
        binding(table, &jjroot, "jj", on_jj);
    }
}

#[test]
fn vcs_port_worktree_footer_shared_priority_literal() {
    if isolated("vcs_port_worktree_footer_shared_priority_literal") {
        return;
    }
    let fixture = sandbox("vcs_finish_worktree_footer");
    let (root, linked, _) = super::vcs_log::seed_worktrees(&canon(&fixture));
    for lang in ["en", "ja"] {
        let mut sim = open(&root, config("git", lang), (160, 20));
        screen(&mut sim, "worktrees");
        for _ in 0..16 {
            if sim.app.project_row_target().as_ref() == Some(&linked) {
                break;
            }
            keys(&mut sim, "j");
        }
        assert_eq!(sim.app.project_row_target(), Some(linked.clone()));
        let tokens = crate::ui::status::hint_tokens(&sim.app);
        let ops = tokens
            .iter()
            .filter_map(|t| {
                let (k, v) = t.split_once(':')?;
                if k == "c" || k == "d" || k == "/" {
                    Some(t.clone())
                } else if k == "Ctrl-t" || k == "C-t" {
                    Some(format!("Ctrl-t:{v}"))
                } else if k.contains("Enter") || k.contains('↵') {
                    Some(format!("Enter:{v}"))
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        let expected = if lang == "en" {
            vec![
                "Enter:switch",
                "c:new",
                "Ctrl-t:new tab",
                "d:diff",
                "/:search",
            ]
        } else {
            vec!["Enter:切替", "c:新規", "Ctrl-t:別タブ", "d:差分", "/:検索"]
        };
        assert_eq!(ops, expected, "shared worktree footer priority");
    }
}

#[test]
fn vcs_port_backend_display_words_copy_labels_and_project_origin() {
    if isolated("vcs_port_backend_display_words_copy_labels_and_project_origin") {
        return;
    }
    let git = super::vcs_changes::git_repo("vcs_finish_display", true);
    let display = |root: &std::path::Path, id: &str| {
        for lang in ["en", "ja"] {
            let mut sim = open(root, config(id, lang), (200, 40));
            assert_eq!(
                sim.app
                    .tab
                    .project_navigation
                    .as_ref()
                    .unwrap()
                    .project
                    .plugin,
                id
            );
            let footer = crate::ui::status::hint_tokens(&sim.app).join("  ");
            let help = crate::ui::help::help_lines(&sim.app)
                .iter()
                .flat_map(|l| &l.spans)
                .map(|s| s.content.as_ref())
                .collect::<String>();
            if id == "jj" {
                for word in ["stage", "unstage", "commit", "worktree"] {
                    assert!(!footer.contains(word));
                    assert!(!help.contains(word));
                }
                assert!(help.contains(tr(sim.app.lang, Msg::JjSyncRow)));
            } else if lang == "en" {
                for word in ["stage", "unstage", "commit"] {
                    assert!(footer.contains(word));
                }
                for word in ["sync", "bookmarks"] {
                    assert!(!footer.contains(word));
                }
            }
            if id == "git" {
                // Paired Git help: the write keys are documented (the "worktree" word is the
                // PENDING_DECISION:project-help-wording row) and no jj section or heading
                // appears, in either language.
                if lang == "en" {
                    for word in ["stage", "unstage", "commit"] {
                        assert!(help.contains(word), "Git help lacks {word:?}: {help}");
                    }
                    // "stage" is a substring of "unstage" and "staged", so the word check alone
                    // cannot tell which row is missing: each row's own description is required.
                    let rows: Vec<(String, String)> = crate::ops::help::sections(&sim.app)
                        .iter()
                        .flat_map(|s| s.rows.clone())
                        .collect();
                    for (key, desc) in [
                        ("s", "stage / stage all"),
                        ("u", "unstage / unstage all"),
                        ("c", "commit (staged)"),
                    ] {
                        assert!(
                            rows.iter().any(|(k, d)| k.split(" / ").any(|x| x == key) && d == desc),
                            "Git help lacks the {key:?} row {desc:?}: {rows:?}"
                        );
                    }
                }
                for m in [Msg::JjChangesLabel, Msg::JjSyncRow, Msg::JjCloseView] {
                    assert!(
                        !help.contains(tr(sim.app.lang, m)),
                        "Git help shows a jj row {:?}: {help}",
                        tr(sim.app.lang, m)
                    );
                }
                assert!(
                    !help.to_lowercase().contains("jj"),
                    "Git help must not mention jj: {help}"
                );
            }
            screen(&mut sim, "graph");
            graph_help_literals(&sim, id == "jj");
            screen(&mut sim, "branches");
            if id == "jj" {
                sim.see("main");
                sim.dont_see("*");
                let footer = crate::ui::status::hint_tokens(&sim.app).join(" ");
                for word in ["checkout", "new", "delete"] {
                    assert!(!footer.contains(word));
                }
            }
            screen(&mut sim, "log");
            keys(&mut sim, "y");
            let menu = crate::ops::menu::menu(
                &sim.app,
                Context::Canvas,
                LeaderId::Copy,
                crate::ops::menu::View::Open,
            )
            .unwrap();
            let want = if id == "jj" {
                tr(sim.app.lang, Msg::WkChangeId)
            } else {
                tr(sim.app.lang, Msg::WkShortHash)
            };
            assert_eq!(menu.items[0].label, want);
            let full = if id == "jj" {
                tr(sim.app.lang, Msg::WkCommitId)
            } else {
                tr(sim.app.lang, Msg::WkFullHash)
            };
            assert_eq!(menu.items[1].label, full);
            let footer = crate::ui::status::footer_spans(&sim.app, 200)
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>();
            assert!(footer.contains(want));
            if id == "jj" {
                assert!(!footer.contains(tr(sim.app.lang, Msg::WkShortHash)));
            } else {
                assert!(!footer.contains(tr(sim.app.lang, Msg::WkChangeId)));
            }
        }
    };
    display(&git, "git");
    // Only the jj half needs the jj binary; the Git half above has already run.
    if !crate::vcs::jj::available() {
        return;
    }
    let jj = super::vcs_log::jj_repo(false);
    super::vcs_log::jj(&jj, &["bookmark", "create", "-r", "@", "main"]);
    std::fs::write(jj.join("changed.txt"), "new\n").unwrap();
    display(&jj, "jj");
}

/// The status bar's mode chips as drawn: the display chip (GIT / JJ) and the internal chip (the
/// screen's word), each with surrounding blanks trimmed. Released v0.31.2: `display_chip` and
/// `internal_chip` in src/ui/status.rs.
fn mode_chips(sim: &Sim) -> Vec<String> {
    crate::ui::status::context_spans(&sim.app)
        .iter()
        .map(|s| s.content.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Returns how many screens it checked, so the caller can prove every case ran.
fn assert_mode_chips(sim: &mut Sim, id: &str, lang: &str, ctx: &str) -> usize {
    let mut checked = 0;
    let l = sim.app.lang;
    let display = tr(l, if id == "jj" { Msg::StJj } else { Msg::StGit });
    let other = tr(l, if id == "jj" { Msg::StGit } else { Msg::StJj });
    let list = if id == "jj" {
        tr(l, Msg::StBookmark)
    } else {
        tr(l, Msg::StBranch)
    };
    let mut screens: Vec<(&str, String)> = vec![
        ("changes", tr(l, Msg::StChanges).to_string()),
        ("log", tr(l, Msg::StLog).to_string()),
        ("graph", tr(l, Msg::StGraph).to_string()),
        ("branches", list.to_string()),
    ];
    if id == "git" {
        screens.push(("worktrees", tr(l, Msg::StWorktrees).to_string()));
    }
    for (name, word) in screens {
        screen(sim, name);
        let chips = mode_chips(sim);
        let here = format!("{ctx}/{id}/{lang}/{name}: {chips:?}");
        assert!(chips.iter().any(|c| c == display), "{here}: display chip");
        assert!(chips.iter().all(|c| c != other), "{here}: no other backend's chip");
        assert!(chips.contains(&word), "{here}: internal chip {word:?}");
        assert!(
            chips.iter().all(|c| c != tr(l, Msg::ProjectTitle)),
            "{here}: no generic project chip on a git/jj canvas"
        );
        checked += 1;
    }
    // Detail (opened from the log) and the graph's branch panel.
    screen(sim, "log");
    keys(sim, "l");
    let chips = mode_chips(sim);
    assert!(chips.iter().any(|c| *c == tr(l, Msg::StCommitDiff)), "{ctx}/{id}/{lang}: {chips:?}");
    assert!(chips.iter().any(|c| c == display), "{ctx}/{id}/{lang}: detail display chip");
    checked += 1;
    keys(sim, "q");
    screen(sim, "graph");
    keys(sim, "b");
    let picker = format!("{} - {}", tr(l, Msg::StGraph), tr(l, Msg::StBranch));
    let chips = mode_chips(sim);
    assert!(chips.contains(&picker),"{ctx}/{id}/{lang}: picker {chips:?}");
    checked += 1;
    keys(sim, "q");
    checked
}

#[test]
fn vcs_port_mode_chips_follow_the_backend_and_the_screen() {
    if isolated("vcs_port_mode_chips_follow_the_backend_and_the_screen") {
        return;
    }
    let git = super::vcs_log::repo(3);
    let mut git_cases = 0;
    for lang in ["en", "ja"] {
        let mut sim = open(&git, config("git", lang), (160, 20));
        git_cases += assert_mode_chips(&mut sim, "git", lang, "chips");
    }
    // Git: changes, log, graph, branches, worktrees, detail and the picker, in two languages.
    assert_eq!(git_cases, 14, "every git case ran");
    // Only the jj half needs the jj binary; the Git half above has already run.
    if !crate::vcs::jj::available() {
        return;
    }
    let jj = super::vcs_log::jj_repo(false);
    super::vcs_log::jj(&jj, &["bookmark", "create", "-r", "@", "main"]);
    std::fs::write(jj.join("changed.txt"), "new\n").unwrap();
    let mut jj_cases = 0;
    for lang in ["en", "ja"] {
        let mut sim = open(&jj, config("jj", lang), (160, 20));
        jj_cases += assert_mode_chips(&mut sim, "jj", lang, "chips");
    }
    // jj: no worktrees screen, so one fewer per language.
    assert_eq!(jj_cases, 12, "every jj case ran");
}

/// A git project in one tab and a jj project in another: each tab's chips come from its own
/// canvas, and switching tabs back and forth never carries the other tab's backend or screen.
#[test]
fn vcs_port_mode_chips_do_not_leak_across_tabs() {
    if isolated("vcs_port_mode_chips_do_not_leak_across_tabs") {
        return;
    }
    if !crate::vcs::jj::available() {
        return;
    }
    let parent = crate::test_support::unique_tmp("vcs_chips_tabs");
    let (git, jj) = (parent.join("g"), parent.join("j"));
    std::fs::create_dir_all(&git).unwrap();
    std::fs::create_dir_all(&jj).unwrap();
    crate::test_support::init_git_repo(&git);
    super::vcs_log::jj(&jj, &["git", "init", "--no-colocate", "."]);
    let mut cfg = config("git", "en");
    cfg.plugins.remove("jj");
    let mut sim = Sim::with_config_sized(&parent, cfg, 160, 20);
    let enter_project = |sim: &mut Sim, dir: &str| {
        sim.select(dir);
        sim.key('l');
        sim.app.tab.projects.wait_for_result();
        sim.app.poll_projects();
        sim.key('p');
        drain(sim);
        assert!(sim.app.project_active(), "{dir}: project opened");
    };
    enter_project(&mut sim, "/g");
    let git_chips = mode_chips(&sim);
    assert!(git_chips.iter().any(|c| c == "GIT"), "{git_chips:?}");
    assert!(git_chips.iter().all(|c| c != "JJ"), "{git_chips:?}");
    // The second tab starts as a tree at the git root; go up, into the jj repository.
    sim.app.tab_new().unwrap();
    sim.key('h');
    enter_project(&mut sim, "/j");
    screen(&mut sim, "branches");
    let jj_chips = mode_chips(&sim);
    assert!(jj_chips.iter().any(|c| c == "JJ"), "{jj_chips:?}");
    assert!(jj_chips.iter().any(|c| c == "BOOKMARK"), "{jj_chips:?}");
    assert!(jj_chips.iter().all(|c| c != "GIT"), "{jj_chips:?}");
    for _ in 0..2 {
        sim.app.tab_goto(0);
        drain(&mut sim);
        assert_eq!(mode_chips(&sim), git_chips, "back on the git tab");
        sim.app.tab_goto(1);
        drain(&mut sim);
        assert_eq!(mode_chips(&sim), jj_chips, "back on the jj tab");
    }
}

/// The footer tokens and the help text the app shows on its canvas now. (The project canvas's
/// Space menu is built from the plugin's actions only, so screen config keys never reach it.)
fn shown_hints(sim: &Sim) -> (String, String) {
    let footer = crate::ui::status::hint_tokens(&sim.app).join("  ");
    let help = crate::ui::help::help_lines(&sim.app)
        .iter()
        .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n");
    (footer, help)
}

#[test]
fn vcs_port_screen_keys_decide_the_keys_and_the_footer_hint_together() {
    if isolated("vcs_port_screen_keys_decide_the_keys_and_the_footer_hint_together") {
        return;
    }
    let root = super::vcs_changes::git_repo("vcs_finish_screen_keys", true);
    // `noop` is the config's unbind word. `[keys."git.changes"] j = "noop"`: j does nothing on
    // git changes and its hint goes with it; git log keeps j.
    for table in ["git.changes", "git_changes"] {
        let mut cfg = config("git", "en");
        cfg.keys
            .surfaces
            .insert(table.into(), [("j".into(), "noop".into())].into());
        let mut sim = open(&root, cfg, (180, 24));
        let before = sim.app.project_act_context().selected;
        keys(&mut sim, "j");
        assert_eq!(sim.app.project_act_context().selected, before, "{table}: j acts");
        keys(&mut sim, "k");
        let (footer, help) = shown_hints(&sim);
        // The old hub's first screen shows no nav hint: nothing of j/k is in its footer.
        assert!(!footer.contains("k:nav") && !footer.contains("j/k"), "{table}: {footer}");
        // The help body's scroll row reads the same keys (the list screen's row is the arrows
        // plus what is still bound).
        assert!(help.contains("k / ↑/↓") && !help.contains("j/k / ↑/↓"), "{table}: {help}");
        screen(&mut sim, "log");
        let (footer, _) = shown_hints(&sim);
        assert!(footer.contains("j/k:nav"), "{table} on log: {footer}");
    }
    // The nav hint is shown from the second screen on: a rebinding of that screen's table moves
    // it, and the other screens keep theirs.
    for table in ["git.log", "git_log"] {
        let mut cfg = config("git", "en");
        cfg.keys
            .surfaces
            .insert(table.into(), [("j".into(), "noop".into())].into());
        let mut sim = open(&root, cfg, (180, 24));
        screen(&mut sim, "log");
        let (footer, _) = shown_hints(&sim);
        assert!(footer.contains("k:nav") && !footer.contains("j/k:nav"), "{table}: {footer}");
        screen(&mut sim, "branches");
        let (footer, _) = shown_hints(&sim);
        assert!(footer.contains("j/k:nav"), "{table} on branches: {footer}");
    }
    // The other backend's table does not reach git.
    let mut cfg = config("git", "en");
    cfg.keys
        .surfaces
        .insert("jj.changes".into(), [("j".into(), "noop".into())].into());
    let mut sim = open(&root, cfg, (180, 24));
    screen(&mut sim, "log");
    assert!(shown_hints(&sim).0.contains("j/k:nav"));
    // A key written for the screen acts there (and only there).
    for (table, acts) in [("git.changes", true), ("git_changes", true), ("jj.changes", false)] {
        let mut cfg = config("git", "en");
        cfg.keys
            .surfaces
            .insert(table.into(), [("Z".into(), "toggle_help".into())].into());
        let mut sim = open(&root, cfg, (180, 24));
        keys(&mut sim, "Z");
        assert_eq!(sim.app.key_context() == Some(Context::Help), acts, "{table}");
    }
    // Only the jj half needs the jj binary; the Git half above has already run.
    if !crate::vcs::jj::available() {
        return;
    }
    let jjroot = super::vcs_log::jj_repo(false);
    std::fs::write(jjroot.join("changed.txt"), "new\n").unwrap();
    // A second row, so that a j that acts would move the selection.
    std::fs::write(jjroot.join("changed2.txt"), "new\n").unwrap();
    // `[keys."jj.changes"] j = "noop"` removes j's action and its footer/help hint on the jj
    // changes screen; the git changes screen (above) keeps both.
    let mut sim = open(&jjroot, config("jj", "en"), (180, 24));
    let (footer, help) = shown_hints(&sim);
    assert!(!footer.contains("j/k"), "jj first screen shows no nav hint: {footer}");
    assert!(help.contains("j/k / ↑/↓"), "jj default help: {help}");
    screen(&mut sim, "log");
    assert!(shown_hints(&sim).0.contains("j/k:nav"), "jj log default footer");
    let mut cfg = config("jj", "en");
    cfg.keys
        .surfaces
        .insert("jj.changes".into(), [("j".into(), "noop".into())].into());
    let mut sim = open(&jjroot, cfg, (180, 24));
    let before = sim.app.project_act_context().selected;
    keys(&mut sim, "j");
    assert_eq!(sim.app.project_act_context().selected, before, "jj.changes: j acts");
    keys(&mut sim, "k");
    let (footer, help) = shown_hints(&sim);
    assert!(!footer.contains("j/k"), "jj: {footer}");
    assert!(help.contains("k / ↑/↓") && !help.contains("j/k / ↑/↓"), "jj: {help}");
    // On the second screen the nav hint is shown; its own table moves it.
    let mut cfg = config("jj", "en");
    cfg.keys
        .surfaces
        .insert("jj.log".into(), [("j".into(), "noop".into())].into());
    let mut sim = open(&jjroot, cfg, (180, 24));
    screen(&mut sim, "log");
    let footer = shown_hints(&sim).0;
    assert!(footer.contains("k:nav") && !footer.contains("j/k:nav"), "jj log: {footer}");
}

#[test]
fn vcs_port_screen_keys_back_filter_help_follow_the_effective_keys() {
    if isolated("vcs_port_screen_keys_back_filter_help_follow_the_effective_keys") {
        return;
    }
    let root = super::vcs_changes::git_repo("vcs_finish_screen_keys_fixed", true);
    // Defaults: the three controls and the help rows are shown.
    {
        let sim = open(&root, config("git", "en"), (180, 24));
        let (footer, help) = shown_hints(&sim);
        // The old hub's tail on the first screen: search, screens, close. It never showed `?:help`.
        for word in ["/:search", "h/q:close"] {
            assert!(footer.contains(word), "default footer {word}: {footer}");
        }
        assert!(!footer.contains("?:help"), "default footer: {footer}");
        assert!(help.contains("j/k / ↑/↓"), "default help: {help}");
    }
    // `q` and `/` unbound on git.changes (both spellings of the table): the keys do nothing and
    // their controls and help rows go; git log keeps them.
    for table in ["git.changes", "git_changes"] {
        let mut cfg = config("git", "en");
        cfg.keys.surfaces.insert(
            table.into(),
            [("q".into(), "noop".into()), ("/".into(), "noop".into())].into(),
        );
        let mut sim = open(&root, cfg, (180, 24));
        keys(&mut sim, "q");
        assert!(sim.app.project_active(), "{table}: the unbound q left the project");
        keys(&mut sim, "/");
        assert!(
            !sim.app.project_frame().is_some_and(|f| f.filtering),
            "{table}: the unbound / started the filter"
        );
        let (footer, help) = shown_hints(&sim);
        // `Left` (h) still closes: only `q` left the token.
        assert!(!footer.contains("q:close") && !footer.contains("h/q"), "{table}: {footer}");
        assert!(footer.contains("h:close"), "{table}: {footer}");
        assert!(!footer.contains("/:search"), "{table}: {footer}");
        assert!(!help.contains("q / Esc"), "{table}: {help}");
        // The global `?` is not touched by the screen's table.
        keys(&mut sim, "?");
        assert_eq!(sim.app.key_context(), Some(Context::Help), "{table}: ? opens help");
        keys(&mut sim, "?");
        screen(&mut sim, "log");
        let (footer, _) = shown_hints(&sim);
        assert!(footer.contains("h/q/Esc:back"), "{table} on log: {footer}");
        assert!(footer.contains("/:search"), "{table} on log: {footer}");
        keys(&mut sim, "/");
        assert!(sim.app.project_frame().is_some_and(|f| f.filtering));
    }
    // jj's table does not reach git.
    let mut cfg = config("git", "en");
    cfg.keys.surfaces.insert(
        "jj.changes".into(),
        [("q".into(), "noop".into()), ("/".into(), "noop".into())].into(),
    );
    let mut sim = open(&root, cfg, (180, 24));
    let footer = shown_hints(&sim).0;
    assert!(footer.contains("h/q:close") && footer.contains("/:search"), "{footer}");
    keys(&mut sim, "/");
    assert!(sim.app.project_frame().is_some_and(|f| f.filtering));
    // A help key written for the screen acts; the global `?` stays (a context-level noop does not
    // hide a global key). The old hub's footer never showed a help token, so none appears.
    for table in ["git.changes", "git_changes"] {
        let mut cfg = config("git", "en");
        cfg.keys
            .surfaces
            .insert(table.into(), [("Z".into(), "toggle_help".into())].into());
        let mut sim = open(&root, cfg, (180, 24));
        let (footer, _) = shown_hints(&sim);
        assert!(!footer.contains(":help"), "{table}: {footer}");
        keys(&mut sim, "Z");
        assert_eq!(sim.app.key_context(), Some(Context::Help), "{table}");
        keys(&mut sim, "?");
        screen(&mut sim, "log");
        let (footer, _) = shown_hints(&sim);
        assert!(!footer.contains(":help"), "{table} on log: {footer}");
        // The key was written for git.changes only: it does not act on the log.
        keys(&mut sim, "Z");
        assert_ne!(sim.app.key_context(), Some(Context::Help), "{table} on log: Z acts");
    }
}

/// The released hub bound `q` and `/` in the screen's table, so `[keys.git_changes] q = ...`
/// rebound them. On the canvas the fixed key acts as its fixed operation only while the
/// effective keys do not bind it to something else; the footer and the help follow.
#[test]
fn vcs_port_rebinding_a_canvas_fixed_key_replaces_its_fixed_action() {
    if isolated("vcs_port_rebinding_a_canvas_fixed_key_replaces_its_fixed_action") {
        return;
    }
    let root = super::vcs_changes::git_repo("vcs_finish_fixed_rebind", true);
    for table in ["git.changes", "git_changes"] {
        // `q` rebound to the help: it opens the help instead of leaving, the footer and the
        // help show it as the help key and no longer as Back.
        let mut cfg = config("git", "en");
        cfg.keys
            .surfaces
            .insert(table.into(), [("q".into(), "toggle_help".into())].into());
        let mut sim = open(&root, cfg, (180, 24));
        let (footer, help) = shown_hints(&sim);
        assert!(!footer.contains("q:close") && !footer.contains("h/q"), "{table}: {footer}");
        assert!(footer.contains("h:close"), "{table}: {footer}");
        // The old hub's footer never showed a help token, so the help key has none either.
        assert!(!footer.contains(":help"), "{table}: {footer}");
        assert!(!help.contains("q / Esc"), "{table}: {help}");
        keys(&mut sim, "q");
        assert!(sim.app.project_active(), "{table}: q left the project although it is the help key");
        assert_eq!(sim.app.key_context(), Some(Context::Help), "{table}: q opens help");
        keys(&mut sim, "?");
        // Git log has no entry for `q`: it leaves there, as before.
        screen(&mut sim, "log");
        let (footer, _) = shown_hints(&sim);
        assert!(footer.contains("h/q/Esc:back"), "{table} on log: {footer}");
        // `/` rebound to the help: no filter, no `/:search`.
        let mut cfg = config("git", "en");
        cfg.keys
            .surfaces
            .insert(table.into(), [("/".into(), "toggle_help".into())].into());
        let mut sim = open(&root, cfg, (180, 24));
        let (footer, _) = shown_hints(&sim);
        assert!(!footer.contains("/:search"), "{table}: {footer}");
        assert!(footer.contains("h/q:close"), "{table}: {footer}");
        keys(&mut sim, "/");
        assert!(!sim.app.project_frame().is_some_and(|f| f.filtering), "{table}: / filters");
        assert_eq!(sim.app.key_context(), Some(Context::Help), "{table}: / opens help");
        keys(&mut sim, "?");
        screen(&mut sim, "log");
        keys(&mut sim, "/");
        assert!(sim.app.project_frame().is_some_and(|f| f.filtering), "{table} on log");
        // An entry that names the fixed operation itself changes nothing.
        let mut cfg = config("git", "en");
        cfg.keys.surfaces.insert(
            table.into(),
            [("q".into(), "git_close".into()), ("/".into(), "list_filter_start".into())].into(),
        );
        let mut sim = open(&root, cfg, (180, 24));
        let (footer, _) = shown_hints(&sim);
        assert!(footer.contains("h/q:close") && footer.contains("/:search"), "{table}: {footer}");
        keys(&mut sim, "/");
        assert!(sim.app.project_frame().is_some_and(|f| f.filtering), "{table}: own /");
    }
    // jj's table does not reach git.
    let mut cfg = config("git", "en");
    cfg.keys
        .surfaces
        .insert("jj.changes".into(), [("q".into(), "toggle_help".into())].into());
    let sim = open(&root, cfg, (180, 24));
    let (footer, _) = shown_hints(&sim);
    assert!(footer.contains("h/q:close"), "{footer}");
}

fn screen_cfg(backend: &str, table: &str, entries: &[(&str, &str)]) -> Config {
    let mut cfg = config(backend, "en");
    cfg.keys.surfaces.insert(
        table.into(),
        entries.iter().map(|(k, a)| (k.to_string(), a.to_string())).collect(),
    );
    cfg
}

/// The released hub's screen tables held `q` = git_close (worktree_close on worktrees) and `/` =
/// branch_filter_start / worktree_filter_start, so a key bound to one of those operations did the
/// operation wherever the config put it. The canvas does the same: the key handling, the footer
/// and the help read one answer, whichever key the effective keys bind to the screen's own back
/// or filter operation.
#[test]
fn vcs_port_moved_back_and_filter_keys_follow_the_operations() {
    if isolated("vcs_port_moved_back_and_filter_keys_follow_the_operations") {
        return;
    }
    let root = super::vcs_changes::git_repo("vcs_finish_moved_ops", true);
    for table in ["git.changes", "git_changes"] {
        // `q` unbound and the back operation on `x`: `x` leaves, the footer and the help name `x`.
        let sim = open(
            &root,
            screen_cfg("git", table, &[("q", "noop"), ("Z","git_close")]),
            (180, 24),
        );
        let (footer, help) = shown_hints(&sim);
        assert!(footer.contains("h/Z:close") && !footer.contains("q:close"), "{table}: {footer}");
        assert!(help.contains("Z / Esc") && !help.contains("q / Esc"), "{table}: {help}");
        let mut sim = sim;
        keys(&mut sim, "q");
        assert!(sim.app.project_active(), "{table}: the unbound q left the project");
        keys(&mut sim, "Z");
        assert!(!sim.app.project_active(), "{table}: x does not leave");
        // Git log has no entry for these keys: `q` still leaves there.
        let mut sim = open(
            &root,
            screen_cfg("git", table, &[("q", "noop"), ("Z","git_close")]),
            (180, 24),
        );
        screen(&mut sim, "log");
        let (footer, _) = shown_hints(&sim);
        assert!(footer.contains("h/q/Esc:back") && !footer.contains("Z"), "{table} on log: {footer}");
        keys(&mut sim, "Z");
        assert!(sim.app.project_active(), "{table} on log: x left the project");
        keys(&mut sim, "q");
        assert!(!sim.app.project_active(), "{table} on log: q does not leave");
        // The fixed `q` stays beside a second key for the same operation.
        let mut sim = open(&root, screen_cfg("git", table, &[("Z","git_close")]), (180, 24));
        let footer = shown_hints(&sim).0;
        assert!(footer.contains("h/q/Z:close"), "{table}: {footer}");
        keys(&mut sim, "Z");
        assert!(!sim.app.project_active(), "{table}: x beside q does not leave");
        // Another screen's close does nothing here (the old table ran it as a no-op): `q` bound
        // to worktree_close no longer leaves and is not shown as Back; `x` bound to it is dead.
        let mut sim = open(
            &root,
            screen_cfg("git", table, &[("q", "worktree_close")]),
            (180, 24),
        );
        let footer = shown_hints(&sim).0;
        assert!(!footer.contains("q:close") && !footer.contains("h/q"), "{table}: {footer}");
        keys(&mut sim, "q");
        assert!(sim.app.project_active(), "{table}: q = worktree_close left");
        let mut sim = open(
            &root,
            screen_cfg("git", table, &[("Z","worktree_close")]),
            (180, 24),
        );
        let footer = shown_hints(&sim).0;
        assert!(footer.contains("h/q:close") && !footer.contains("Z"), "{table}: {footer}");
        keys(&mut sim, "Z");
        assert!(sim.app.project_active(), "{table}: x = worktree_close left");
        keys(&mut sim, "q");
        assert!(!sim.app.project_active(), "{table}: q does not leave");
        // The filter: `/` unbound, the filter operation on `x`, on the list screens.
        let mut sim = open(
            &root,
            screen_cfg("git", table, &[("/", "noop"), ("Z","list_filter_start")]),
            (180, 24),
        );
        let (footer, _) = shown_hints(&sim);
        assert!(footer.contains("Z:search") && !footer.contains("/:search"), "{table}: {footer}");
        keys(&mut sim, "/");
        assert!(!sim.app.project_frame().is_some_and(|f| f.filtering), "{table}: unbound /");
        keys(&mut sim, "Z");
        assert!(sim.app.project_frame().is_some_and(|f| f.filtering), "{table}: x filters");
        // A branch filter operation on the changes screen starts no filter (the old table ran it
        // invisibly).
        let mut sim = open(
            &root,
            screen_cfg("git", table, &[("Z","branch_filter_start")]),
            (180, 24),
        );
        let footer = shown_hints(&sim).0;
        assert!(footer.contains("/:search") && !footer.contains("Z:search"), "{table}: {footer}");
        keys(&mut sim, "Z");
        assert!(!sim.app.project_frame().is_some_and(|f| f.filtering), "{table}: x filters");
    }
    // Branches and worktrees: the screen's own filter operation on another key.
    // (The worktree list's screen declares the word "search" for its filter.)
    for (table, name, word, own, other) in [
        ("git.branches", "branches", "search", "branch_filter_start", "worktree_filter_start"),
        ("git_branches", "branches", "search", "branch_filter_start", "worktree_filter_start"),
        ("git.worktrees", "worktrees", "search", "worktree_filter_start", "branch_filter_start"),
        ("git_worktrees", "worktrees", "search", "worktree_filter_start", "branch_filter_start"),
    ] {
        let mut sim = open(
            &root,
            screen_cfg("git", table, &[("/", "noop"), ("Z", own)]),
            (180, 24),
        );
        screen(&mut sim, name);
        let (footer, _) = shown_hints(&sim);
        assert!(
            footer.contains(&format!("Z:{word}")) && !footer.contains(&format!("/:{word}")),
            "{table}: {footer}"
        );
        keys(&mut sim, "/");
        assert!(!sim.app.project_frame().is_some_and(|f| f.filtering), "{table}: unbound /");
        keys(&mut sim, "Z");
        assert!(sim.app.project_frame().is_some_and(|f| f.filtering), "{table}: x filters");
        // The other list's filter operation is not this screen's: `x` does nothing.
        let mut sim = open(
            &root,
            screen_cfg("git", table, &[("/", "noop"), ("Z",other)]),
            (180, 24),
        );
        screen(&mut sim, name);
        let footer = shown_hints(&sim).0;
        assert!(
            !footer.contains(&format!("Z:{word}")) && !footer.contains(&format!("/:{word}")),
            "{table}: {footer}"
        );
        keys(&mut sim, "Z");
        assert!(!sim.app.project_frame().is_some_and(|f| f.filtering), "{table}: x filters");
    }
    // Worktrees: the screen's own close is worktree_close; git_close did nothing there.
    for table in ["git.worktrees", "git_worktrees"] {
        let mut sim = open(
            &root,
            screen_cfg("git", table, &[("q", "noop"), ("Z","worktree_close")]),
            (180, 24),
        );
        screen(&mut sim, "worktrees");
        let footer = shown_hints(&sim).0;
        assert!(footer.contains("h/Z/Esc:back") && !footer.contains("h/q"), "{table}: {footer}");
        keys(&mut sim, "Z");
        assert!(!sim.app.project_active(), "{table}: x does not leave");
        let mut sim = open(
            &root,
            screen_cfg("git", table, &[("Z","git_close")]),
            (180, 24),
        );
        screen(&mut sim, "worktrees");
        let footer = shown_hints(&sim).0;
        assert!(footer.contains("h/q/Esc:back") && !footer.contains("Z"), "{table}: {footer}");
        keys(&mut sim, "Z");
        assert!(sim.app.project_active(), "{table}: x = git_close left");
    }
    // jj's table is the jj screen's; git is unaffected.
    if !crate::vcs::jj::available() {
        return;
    }
    let jjroot = super::vcs_log::jj_repo(false);
    std::fs::write(jjroot.join("changed.txt"), "new\n").unwrap();
    let entries = [("q", "noop"), ("Z","git_close")];
    let mut sim = open(&jjroot, screen_cfg("jj", "jj.changes", &entries), (180, 24));
    let footer = shown_hints(&sim).0;
    assert!(footer.contains("h/Z:close") && !footer.contains("q:close"), "jj: {footer}");
    keys(&mut sim, "q");
    assert!(sim.app.project_active(), "jj: the unbound q left the project");
    keys(&mut sim, "Z");
    assert!(!sim.app.project_active(), "jj: x does not leave");
    let mut sim = open(&root, screen_cfg("git", "jj.changes", &entries), (180, 24));
    let footer = shown_hints(&sim).0;
    assert!(footer.contains("h/q:close") && !footer.contains("Z"), "git: {footer}");
    keys(&mut sim, "Z");
    assert!(sim.app.project_active(), "git: the jj table's x left the project");
}

/// What `x` did on the changes screen (the plugin's discard has `x` by default): nothing / the
/// confirmation dialog / left the project.
#[derive(Debug, PartialEq)]
enum XOutcome {
    Left,
    Confirm,
    Nothing,
}

fn press_x(sim: &mut Sim) -> XOutcome {
    keys(sim, "x");
    if !sim.app.project_active() {
        XOutcome::Left
    } else if sim.app.project_dialog.is_some() {
        XOutcome::Confirm
    } else {
        XOutcome::Nothing
    }
}

/// A plugin action's key stays the action's only while the screen's effective keys do not give
/// that key to another operation (or unbind it): the key handling and the footer / help read one
/// answer. In the released hub the screen table's `x = "git_close"` replaced GitDiscard.
#[test]
fn vcs_port_moved_key_taken_from_a_plugin_action_follows_the_screen_table() {
    if isolated("vcs_port_moved_key_taken_from_a_plugin_action_follows_the_screen_table") {
        return;
    }
    let root = super::vcs_changes::git_repo("vcs_finish_taken_key", true);
    // Baseline: with no config `x:discard` is shown and `x` opens its confirmation.
    let mut sim = open(&root, config("git", "en"), (180, 24));
    let (footer, help) = shown_hints(&sim);
    assert!(footer.contains("x:discard"), "baseline footer: {footer}");
    assert!(help.contains("discard file"), "baseline help: {help}");
    assert_eq!(press_x(&mut sim), XOutcome::Confirm, "baseline: x");
    for table in ["git.changes", "git_changes"] {
        for (entries, outcome, note) in [
            (&[("x", "git_close")][..], XOutcome::Left, "x = git_close"),
            (&[("x", "noop")][..], XOutcome::Nothing, "x = noop"),
            (&[("x", "toggle_help")][..], XOutcome::Nothing, "x = toggle_help"),
        ] {
            let mut sim = open(&root, screen_cfg("git", table, entries), (180, 24));
            let (footer, help) = shown_hints(&sim);
            assert!(!footer.contains("x:discard"), "{table} {note}: {footer}");
            assert!(!help.contains("discard file"), "{table} {note}: {help}");
            let discard = sim
                .app
                .plugin_context_bindings(Context::Canvas)
                .into_iter()
                .find(|b| b.act.act() == "discard")
                .expect("the discard action is declared on the changes screen");
            assert_eq!(discard.key, None, "{table} {note}: discard keeps a key");
            assert_eq!(press_x(&mut sim), outcome, "{table} {note}");
        }
        // An entry for another key leaves the discard alone.
        let mut sim = open(&root, screen_cfg("git", table, &[("Z", "git_close")]), (180, 24));
        let footer = shown_hints(&sim).0;
        assert!(footer.contains("x:discard"), "{table}: {footer}");
        assert_eq!(press_x(&mut sim), XOutcome::Confirm, "{table}: other key");
    }
    // Another screen's table does not take the key.
    let mut sim = open(&root, screen_cfg("git", "git.log", &[("x", "git_close")]), (180, 24));
    assert!(shown_hints(&sim).0.contains("x:discard"));
    assert_eq!(press_x(&mut sim), XOutcome::Confirm, "git.log's table");
}

/// In the released hub `worktree_close` on a key of another screen reset the worktree list's
/// state and re-opened the hub underneath whatever list was in front (`close_git_worktrees`; the
/// frontmost layer kept the log / branch list on screen, `App::frontmost_layer` in v0.31.2), so
/// nothing changed on screen there; on the change list it read the entries again and put the
/// cursor on the first row. The canvas has no hub behind it: on the change list it refreshes
/// as `r` does and starts at row 0, elsewhere it does nothing, and it leaves no legacy hub state
/// behind (leaving the project must show the tree, not the hub).
#[test]
fn vcs_port_moved_another_screens_worktree_close_leaves_no_hub_behind() {
    if isolated("vcs_port_moved_another_screens_worktree_close_leaves_no_hub_behind") {
        return;
    }
    let root = super::vcs_changes::git_repo("vcs_finish_wt_close", true);
    for table in ["git", "git_"] {
        let sep = if table == "git" { "." } else { "" };
        let mut cfg = config("git", "en");
        for name in ["changes", "log", "branches", "worktrees"] {
            cfg.keys.surfaces.insert(
                format!("{table}{sep}{name}"),
                [("Z".into(), "worktree_close".into())].into(),
            );
        }
        // On the change list the key reads the changes again (as `r`) and starts at row 0.
        let added = format!("00-added-{table}.txt");
        let mut sim = open(&root, cfg.clone(), (180, 24));
        screen(&mut sim, "changes");
        keys(&mut sim, "jj");
        assert_eq!(sim.app.project_frame().unwrap().cursor, 2, "{table}: moved");
        std::fs::write(root.join(&added), "new\n").unwrap();
        sim.dont_see(&added);
        keys(&mut sim, "Z");
        assert!(sim.app.project_active(), "{table} changes: Z left");
        sim.see(&added);
        let frame = sim.app.project_frame().unwrap();
        assert_eq!(
            (frame.key.screen.as_str(), frame.cursor),
            ("changes", 0),
            "{table}: the cursor does not start over"
        );
        assert!(!sim.app.is_git_view(), "{table} changes: the hub opened behind the canvas");
        keys(&mut sim, "q");
        assert!(!sim.app.project_active(), "{table} changes: q");
        assert!(!sim.app.is_git_view(), "{table} changes: the hub shows after the canvas");
        std::fs::remove_file(root.join(&added)).unwrap();
        for name in ["log", "branches"] {
            let mut sim = open(&root, cfg.clone(), (180, 24));
            screen(&mut sim, name);
            keys(&mut sim, "j");
            let cursor = sim.app.project_frame().unwrap().cursor;
            keys(&mut sim, "Z");
            assert!(sim.app.project_active(), "{table} {name}: Z left");
            let frame = sim.app.project_frame().unwrap();
            assert_eq!((frame.key.screen.as_str(), frame.cursor), (name, cursor), "{table} {name}");
            assert!(!sim.app.is_git_view(), "{table} {name}: the hub opened behind the canvas");
            keys(&mut sim, "q");
            assert!(!sim.app.project_active(), "{table} {name}: q");
            assert!(!sim.app.is_git_view(), "{table} {name}: the hub shows after the canvas");
            assert_eq!(sim.app.surface(), crate::keymap::Surface::Tree, "{table} {name}");
        }
        // The worktree list's own close is that operation: it leaves.
        let mut sim = open(&root, cfg.clone(), (180, 24));
        screen(&mut sim, "worktrees");
        keys(&mut sim, "Z");
        assert!(!sim.app.project_active(), "{table}: Z on worktrees");
        assert!(!sim.app.is_git_view(), "{table}: Z on worktrees opened the hub");
    }
}

/// The VCS plugin with two switches on its changes view: `fail` makes the next one return an
/// error, `hold` makes the next one wait on the latch after it has read (the session stays busy
/// until the test opens it). Both are ordered by the test, not by the clock.
struct WtFlaky {
    fail: std::sync::Arc<std::sync::atomic::AtomicBool>,
    hold: std::sync::Arc<std::sync::atomic::AtomicBool>,
    latch: std::sync::Arc<crate::plugins::test_plugins::Latch>,
    backend: &'static str,
}

struct WtFlakySession {
    inner: Box<dyn konoma_plugin::Session>,
    fail: std::sync::Arc<std::sync::atomic::AtomicBool>,
    hold: std::sync::Arc<std::sync::atomic::AtomicBool>,
    latch: std::sync::Arc<crate::plugins::test_plugins::Latch>,
}

impl konoma_plugin::Plugin for WtFlaky {
    fn manifest(&self) -> konoma_plugin::Manifest {
        if self.backend == "jj" {
            konoma_vcs::Jj.manifest()
        } else {
            konoma_vcs::Git.manifest()
        }
    }
    fn open(
        &self,
        input: konoma_plugin::Input,
        cx: &konoma_plugin::OpenCx,
    ) -> Result<Box<dyn konoma_plugin::Session>, konoma_plugin::Failure> {
        let inner = if self.backend == "jj" {
            konoma_vcs::Jj.open(input, cx)?
        } else {
            konoma_vcs::Git.open(input, cx)?
        };
        Ok(Box::new(WtFlakySession {
            inner,
            fail: self.fail.clone(),
            hold: self.hold.clone(),
            latch: self.latch.clone(),
        }))
    }
}

impl konoma_plugin::Session for WtFlakySession {
    fn view(
        &mut self,
        req: &konoma_plugin::ViewReq,
        cx: &konoma_plugin::ViewCx,
    ) -> Result<konoma_plugin::View, konoma_plugin::Failure> {
        use std::sync::atomic::Ordering;
        let on_changes = req.project.as_ref().is_some_and(|p| p.screen == "changes");
        if on_changes && self.fail.swap(false, Ordering::SeqCst) {
            return Err(konoma_plugin::Failure::external(konoma_plugin::Tr::new(
                "the repository is locked",
            )));
        }
        let view = self.inner.view(req, cx);
        if on_changes && self.hold.swap(false, Ordering::SeqCst) {
            self.latch.wait();
        }
        view
    }
    fn act(
        &mut self,
        action: &str,
        cx: &konoma_plugin::ActCx,
    ) -> Result<konoma_plugin::Outcome, konoma_plugin::Failure> {
        self.inner.act(action, cx)
    }
    fn enabled(&self, action: &str) -> bool {
        self.inner.enabled(action)
    }
    fn chip(
        &mut self,
        cx: &konoma_plugin::ViewCx,
    ) -> Result<Option<konoma_plugin::Chip>, konoma_plugin::Failure> {
        self.inner.chip(cx)
    }
    fn watch_paths(
        &mut self,
        cx: &konoma_plugin::ViewCx,
    ) -> Result<Option<Vec<std::path::PathBuf>>, konoma_plugin::Failure> {
        self.inner.watch_paths(cx)
    }
    fn tab_closed(&mut self, tab: u64) {
        self.inner.tab_closed(tab);
    }
}

struct WtFlakyHandles {
    fail: std::sync::Arc<std::sync::atomic::AtomicBool>,
    hold: std::sync::Arc<std::sync::atomic::AtomicBool>,
    latch: std::sync::Arc<crate::plugins::test_plugins::Latch>,
}

/// A config where `Z` is `worktree_close` on `table`, over [`WtFlaky`].
fn wt_flaky_cfg(backend: &'static str, table: &str) -> (Config, WtFlakyHandles) {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Arc;
    static ID: AtomicUsize = AtomicUsize::new(0);
    let handles = WtFlakyHandles {
        fail: Arc::new(AtomicBool::new(false)),
        hold: Arc::new(AtomicBool::new(false)),
        latch: Arc::new(crate::plugins::test_plugins::Latch::default()),
    };
    let marker = crate::plugins::test_plugins::register_replacement(
        &format!("vcs_port_wt_flaky_{}", ID.fetch_add(1, Ordering::Relaxed)),
        Arc::new(WtFlaky {
            fail: handles.fail.clone(),
            hold: handles.hold.clone(),
            latch: handles.latch.clone(),
            backend,
        }),
    );
    let mut cfg = screen_cfg(backend, table, &[("Z", "worktree_close")]);
    cfg.plugins
        .extend(crate::plugins::test_plugins::marker_config(&marker).plugins);
    (cfg, handles)
}

/// Whether the project's session holds a standing error (the last view failed).
fn project_view_error(sim: &Sim) -> bool {
    sim.app
        .project_runtimes
        .values()
        .any(|runtime| runtime.error.is_some())
}

/// The start-over a `worktree_close` asks for belongs to that one reload. When the reload fails
/// the request is over: a later reload (here `r`, with a motion waiting in line) follows the
/// selected row and keeps the key waiting in line instead of jumping to row 0 and dropping it.
#[test]
fn vcs_port_moved_worktree_close_start_over_does_not_outlive_a_failed_reload() {
    use std::sync::atomic::Ordering;
    if isolated("vcs_port_moved_worktree_close_start_over_does_not_outlive_a_failed_reload") {
        return;
    }
    let root = super::vcs_changes::git_repo("vcs_finish_wt_close_failed", true);
    for table in ["git.changes", "git_changes"] {
        let (cfg, h) = wt_flaky_cfg("git", table);
        let mut sim = open(&root, cfg, (180, 24));
        screen(&mut sim, "changes");
        keys(&mut sim, "jj");
        let rows = real_list(&sim).rows.len();
        assert!(rows >= 4, "{table}: the fixture needs rows below the cursor");
        assert_eq!(sim.app.project_frame().unwrap().cursor, 2, "{table}: moved");
        let third = real_list(&sim).rows[2].value.clone();
        // Z while the next read fails: no rows arrive, the cursor stays where it was.
        h.fail.store(true, Ordering::SeqCst);
        keys(&mut sim, "Z");
        assert!(project_view_error(&sim), "{table}: the reload did not fail");
        assert_eq!(sim.app.project_frame().unwrap().cursor, 2, "{table}: failed Z moved");
        // A plain reload now. While it reads, a motion waits in line; the rows then arrive.
        h.hold.store(true, Ordering::SeqCst);
        sim.press(KeyCode::Char('r'), KeyModifiers::NONE);
        assert!(sim.app.project_jobs_pending(), "{table}: r did not start a job");
        let before = sim.app.project_frame().unwrap().pending.len();
        // `x` (discard) asks for a confirmation when it runs.
        sim.press(KeyCode::Char('x'), KeyModifiers::NONE);
        assert_eq!(
            sim.app.project_frame().unwrap().pending.len(),
            before + 1,
            "{table}: the key did not wait in line"
        );
        assert!(sim.app.project_dialog.is_none(), "{table}: x ran before the rows arrived");
        h.latch.open();
        drain(&mut sim);
        let frame = sim.app.project_frame().unwrap();
        assert!(frame.pending.is_empty(), "{table}: the waiting key is left over");
        assert!(sim.app.project_dialog.is_some(), "{table}: the waiting key was dropped");
        assert_eq!(frame.cursor, 2, "{table}: the reload after a failed Z followed the selection");
        assert_eq!(
            real_list(&sim).rows[frame.cursor].value,
            third,
            "{table}: the cursor is on the row it was on"
        );
    }
}

/// `worktree_close` while the session is reading: the start-over waits for the read the dirty
/// screen needs next (the one in flight is discarded as stale) and applies to that one.
#[test]
fn vcs_port_moved_worktree_close_start_over_waits_for_a_busy_session() {
    use std::sync::atomic::Ordering;
    if isolated("vcs_port_moved_worktree_close_start_over_waits_for_a_busy_session") {
        return;
    }
    let root = super::vcs_changes::git_repo("vcs_finish_wt_close_busy", true);
    for table in ["git.changes", "git_changes"] {
        let (cfg, h) = wt_flaky_cfg("git", table);
        let mut sim = open(&root, cfg, (180, 24));
        screen(&mut sim, "changes");
        keys(&mut sim, "jj");
        assert_eq!(sim.app.project_frame().unwrap().cursor, 2, "{table}: moved");
        h.hold.store(true, Ordering::SeqCst);
        sim.press(KeyCode::Char('r'), KeyModifiers::NONE);
        assert!(sim.app.project_jobs_pending(), "{table}: r did not start a job");
        sim.press(KeyCode::Char('Z'), KeyModifiers::NONE);
        assert_eq!(sim.app.project_frame().unwrap().cursor, 2, "{table}: nothing read yet");
        h.latch.open();
        drain(&mut sim);
        assert_eq!(
            sim.app.project_frame().unwrap().cursor,
            0,
            "{table}: the reload Z asked for did not start over"
        );
    }
}

/// jj's change list answers `worktree_close` on another key the same way as git's: it reads
/// again and starts at the first row.
#[test]
fn vcs_port_moved_jj_worktree_close_reloads_the_changes_from_the_top() {
    if !crate::vcs::jj::available() {
        return;
    }
    if isolated("vcs_port_moved_jj_worktree_close_reloads_the_changes_from_the_top") {
        return;
    }
    let jjroot = super::vcs_log::jj_repo(false);
    for name in ["b.txt", "c.txt", "d.txt", "e.txt"] {
        std::fs::write(jjroot.join(name), "new\n").unwrap();
    }
    let added = "00-added.txt";
    let mut cfg = config("jj", "en");
    cfg.keys.surfaces.insert(
        "jj.changes".into(),
        [("Z".to_string(), "worktree_close".to_string())].into(),
    );
    let mut sim = open(&jjroot, cfg, (180, 24));
    screen(&mut sim, "changes");
    keys(&mut sim, "jj");
    assert_eq!(sim.app.project_frame().unwrap().cursor, 2, "jj: moved");
    std::fs::write(jjroot.join(added), "new\n").unwrap();
    sim.dont_see(added);
    keys(&mut sim, "Z");
    assert!(sim.app.project_active(), "jj: Z left");
    sim.see(added);
    let frame = sim.app.project_frame().unwrap();
    assert_eq!(
        (frame.key.screen.as_str(), frame.cursor),
        ("changes", 0),
        "jj: the cursor does not start over"
    );
    assert!(!sim.app.is_git_view(), "jj: the hub opened behind the canvas");
}

fn press_ctrl(sim: &mut Sim, c: char) {
    sim.press(KeyCode::Char(c), KeyModifiers::CONTROL);
    drain(sim);
}

/// In the released hub a screen table's `ctrl-x = "git_close"` resolved to GitClose and closed.
/// The canvas matches the press with its Ctrl state: the Ctrl chord leaves (and filters), the
/// footer and help name it, and the same letter plain or with Alt is not the key.
#[test]
fn vcs_port_moved_ctrl_keys_follow_the_screen_operations() {
    if isolated("vcs_port_moved_ctrl_keys_follow_the_screen_operations") {
        return;
    }
    let root = super::vcs_changes::git_repo("vcs_finish_ctrl_keys", true);
    for table in ["git.changes", "git_changes"] {
        let entries = [("q", "noop"), ("ctrl-x", "git_close")];
        let mut sim = open(&root, screen_cfg("git", table, &entries), (180, 24));
        let (footer, help) = shown_hints(&sim);
        assert!(footer.contains("h/C-x:close") && !footer.contains("q:close"), "{table}: {footer}");
        assert!(help.contains("Ctrl-x / Esc") && !help.contains("q / Esc"), "{table}: {help}");
        keys(&mut sim, "q");
        assert!(sim.app.project_active(), "{table}: the unbound q left");
        // The plain letter is not the chord (it is the discard's key here).
        assert_eq!(press_x(&mut sim), XOutcome::Confirm, "{table}: plain x");
        let mut sim = open(&root, screen_cfg("git", table, &entries), (180, 24));
        press_ctrl(&mut sim, 'x');
        assert!(!sim.app.project_active(), "{table}: Ctrl-x does not leave");
        // Another Ctrl chord is not it.
        let mut sim = open(&root, screen_cfg("git", table, &[("ctrl-x", "git_close")]), (180, 24));
        press_ctrl(&mut sim, 'y');
        assert!(sim.app.project_active(), "{table}: Ctrl-y left");
        let footer = shown_hints(&sim).0;
        assert!(footer.contains("h/q/C-x:close"), "{table}: {footer}");
        press_ctrl(&mut sim, 'x');
        assert!(!sim.app.project_active(), "{table}: Ctrl-x beside q does not leave");
    }
    // The list screens' filter on a Ctrl chord, the fixed `/` unbound.
    for (table, name, word, own) in [
        ("git.branches", "branches", "search", "branch_filter_start"),
        ("git_branches", "branches", "search", "branch_filter_start"),
        ("git.worktrees", "worktrees", "search", "worktree_filter_start"),
        ("git.log", "log", "search", "list_filter_start"),
    ] {
        let mut sim = open(
            &root,
            screen_cfg("git", table, &[("/", "noop"), ("ctrl-x", own)]),
            (180, 24),
        );
        screen(&mut sim, name);
        let footer = shown_hints(&sim).0;
        assert!(
            footer.contains(&format!("C-x:{word}")) && !footer.contains(&format!("/:{word}")),
            "{table}: {footer}"
        );
        keys(&mut sim, "/");
        assert!(!sim.app.project_frame().is_some_and(|f| f.filtering), "{table}: unbound /");
        sim.press(KeyCode::Char('x'), KeyModifiers::ALT);
        drain(&mut sim);
        assert!(!sim.app.project_frame().is_some_and(|f| f.filtering), "{table}: Alt-x filters");
        press_ctrl(&mut sim, 'x');
        assert!(sim.app.project_frame().is_some_and(|f| f.filtering), "{table}: Ctrl-x");
    }
    // jj when available.
    if !crate::vcs::jj::available() {
        return;
    }
    let jjroot = super::vcs_log::jj_repo(false);
    std::fs::write(jjroot.join("changed.txt"), "new\n").unwrap();
    let entries = [("q", "noop"), ("ctrl-x", "git_close")];
    let mut sim = open(&jjroot, screen_cfg("jj", "jj.changes", &entries), (180, 24));
    let footer = shown_hints(&sim).0;
    assert!(footer.contains("h/C-x:close") && !footer.contains("q:close"), "jj: {footer}");
    press_ctrl(&mut sim, 'x');
    assert!(!sim.app.project_active(), "jj: Ctrl-x does not leave");
    // jj declares one action key on the changes screen (`s` = sync) and no `x` (discard is git's),
    // so the screen table's `x = git_close` is the back key there.
    let mut sim = open(&jjroot, screen_cfg("jj", "jj.changes", &[("x", "git_close")]), (180, 24));
    keys(&mut sim, "x");
    assert!(!sim.app.project_active(), "jj: x = git_close leaves the project");
}

/// While a job holds the session the canvas queues presses, except the ones that move or navigate
/// (`navigation` and `local_movement` in the project host). Which keys those are follows the
/// screen's operations: a moved back or filter key acts at once, and the fixed `q` / `/` that no
/// longer mean back / filter wait in line like any other key.
#[test]
fn vcs_port_moved_back_and_filter_keys_act_at_once_while_busy() {
    use konoma_plugin::{Failure, Input, Manifest, OpenCx, Plugin, Session, View, ViewCx, ViewReq};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    if isolated("vcs_port_moved_back_and_filter_keys_act_at_once_while_busy") {
        return;
    }
    struct Held {
        armed: Arc<AtomicBool>,
        latch: Arc<crate::plugins::test_plugins::Latch>,
    }
    impl Plugin for Held {
        fn manifest(&self) -> Manifest {
            konoma_vcs::Git.manifest()
        }
        fn open(&self, input: Input, cx: &OpenCx) -> Result<Box<dyn Session>, Failure> {
            Ok(Box::new(HeldSession {
                inner: konoma_vcs::Git.open(input, cx)?,
                armed: self.armed.clone(),
                latch: self.latch.clone(),
            }))
        }
    }
    struct HeldSession {
        inner: Box<dyn Session>,
        armed: Arc<AtomicBool>,
        latch: Arc<crate::plugins::test_plugins::Latch>,
    }
    impl Session for HeldSession {
        fn view(&mut self, req: &ViewReq, cx: &ViewCx) -> Result<View, Failure> {
            let view = self.inner.view(req, cx);
            if req.project.as_ref().is_some_and(|p| p.screen == "changes")
                && self.armed.swap(false, Ordering::SeqCst)
            {
                self.latch.wait();
            }
            view
        }
        fn act(
            &mut self,
            action: &str,
            cx: &konoma_plugin::ActCx,
        ) -> Result<konoma_plugin::Outcome, Failure> {
            self.inner.act(action, cx)
        }
        fn enabled(&self, action: &str) -> bool {
            self.inner.enabled(action)
        }
        fn chip(&mut self, cx: &ViewCx) -> Result<Option<konoma_plugin::Chip>, Failure> {
            self.inner.chip(cx)
        }
        fn watch_paths(&mut self, cx: &ViewCx) -> Result<Option<Vec<std::path::PathBuf>>, Failure> {
            self.inner.watch_paths(cx)
        }
        fn tab_closed(&mut self, tab: u64) {
            self.inner.tab_closed(tab);
        }
    }
    let root = super::vcs_changes::git_repo("vcs_finish_busy_keys", true);
    static ID: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    for table in ["git.changes", "git_changes"] {
        for (moved, own, fixed, want_leave) in [
            ("Z", "git_close", "q", true),
            ("Z", "list_filter_start", "/", false),
        ] {
            let armed = Arc::new(AtomicBool::new(false));
            let latch = Arc::new(crate::plugins::test_plugins::Latch::default());
            let marker = crate::plugins::test_plugins::register_replacement(
                &format!(
                    "vcs_port_busy_keys_{}",
                    ID.fetch_add(1, Ordering::Relaxed)
                ),
                Arc::new(Held {
                    armed: armed.clone(),
                    latch: latch.clone(),
                }),
            );
            let mut cfg = screen_cfg("git", table, &[(fixed, "noop"), (moved, own)]);
            cfg.plugins
                .extend(crate::plugins::test_plugins::marker_config(&marker).plugins);
            let mut sim = open(&root, cfg, (180, 24));
            let queued = sim.app.project_frame().unwrap().pending.len();
            armed.store(true, Ordering::SeqCst);
            sim.press(KeyCode::Char('r'), KeyModifiers::NONE);
            assert!(
                sim.app.project_jobs_pending(),
                "{table} {own}: r did not start a job"
            );
            // The fixed key no longer means the operation: it waits in line behind the job.
            let before = sim.app.project_frame().unwrap().pending.len();
            sim.press(KeyCode::Char(fixed.chars().next().unwrap()), KeyModifiers::NONE);
            let frame = sim.app.project_frame().unwrap();
            assert_eq!(
                frame.pending.len(),
                before + 1,
                "{table} {own}: {fixed} did not wait in line (queued before r: {queued})"
            );
            assert!(sim.app.project_active(), "{table} {own}: {fixed} left");
            assert!(!frame.filtering, "{table} {own}: {fixed} filtered");
            // The moved key acts at once, not behind the job.
            let before = sim.app.project_frame().unwrap().pending.len();
            sim.press(KeyCode::Char('Z'), KeyModifiers::NONE);
            if want_leave {
                assert!(!sim.app.project_active(), "{table} {own}: Z waited in line");
            } else {
                let frame = sim.app.project_frame().unwrap();
                assert!(frame.filtering, "{table} {own}: Z waited in line");
                assert_eq!(frame.pending.len(), before, "{table} {own}: Z was queued");
            }
            latch.open();
            drain(&mut sim);
        }
    }
}

/// An Alt chord is never the back or filter key: `Alt-q` and `Alt-/` do not leave or filter.
#[test]
fn vcs_port_alt_chords_are_not_the_back_or_filter_key() {
    if isolated("vcs_port_alt_chords_are_not_the_back_or_filter_key") {
        return;
    }
    let root = super::vcs_changes::git_repo("vcs_finish_alt_chords", true);
    let mut sim = open(&root, config("git", "en"), (180, 24));
    sim.press(KeyCode::Char('q'), KeyModifiers::ALT);
    drain(&mut sim);
    assert!(sim.app.project_active(), "Alt-q left the project");
    sim.press(KeyCode::Char('/'), KeyModifiers::ALT);
    drain(&mut sim);
    assert!(
        !sim.app.project_frame().is_some_and(|f| f.filtering),
        "Alt-/ started the filter"
    );
    press_ctrl(&mut sim, 'q');
    assert!(sim.app.project_active(), "Ctrl-q (not bound) left the project");
    keys(&mut sim, "q");
    assert!(!sim.app.project_active(), "plain q does not leave");
}
