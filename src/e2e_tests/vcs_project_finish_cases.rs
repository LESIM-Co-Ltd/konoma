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
    let mut messages = vec![Msg::GitMoveCommit, Msg::TopBottom, Msg::GitDetail];
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
    let mut actual = graph
        .rows
        .iter()
        .map(|(_, d)| d.clone())
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
    let mut titles = vec![title.to_owned()];
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
                "/:search",
                "Enter:switch",
                "Ctrl-t:new tab",
                "c:new",
                "d:diff",
            ]
        } else {
            vec!["/:検索", "Enter:切替", "Ctrl-t:別タブ", "c:新規", "d:差分"]
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
        assert!(footer.contains("k:move") && !footer.contains("j/k:move"), "{table}: {footer}");
        // The help body's scroll row reads the same keys (the list screen's row is the arrows
        // plus what is still bound).
        assert!(help.contains("k / ↑/↓") && !help.contains("j/k / ↑/↓"), "{table}: {help}");
        screen(&mut sim, "log");
        let (footer, _) = shown_hints(&sim);
        assert!(footer.contains("j/k:move"), "{table} on log: {footer}");
    }
    // The other backend's table does not reach git.
    let mut cfg = config("git", "en");
    cfg.keys
        .surfaces
        .insert("jj.changes".into(), [("j".into(), "noop".into())].into());
    let sim = open(&root, cfg, (180, 24));
    assert!(shown_hints(&sim).0.contains("j/k:move"));    // A key written for the screen acts there (and only there).
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
    let sim = open(&jjroot, config("jj", "en"), (180, 24));
    let (footer, help) = shown_hints(&sim);
    assert!(footer.contains("j/k:move"), "jj default footer: {footer}");
    assert!(help.contains("j/k / ↑/↓"), "jj default help: {help}");
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
    assert!(footer.contains("k:move") && !footer.contains("j/k:move"), "jj: {footer}");
    assert!(help.contains("k / ↑/↓") && !help.contains("j/k / ↑/↓"), "jj: {help}");
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
        for word in ["q:back", "/:filter", "?:help"] {
            assert!(footer.contains(word), "default footer {word}: {footer}");
        }
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
        assert!(sim.app.project_active(), "{table}: q still leaves");
        keys(&mut sim, "/");
        assert!(
            !sim.app.project_frame().is_some_and(|f| f.filtering),
            "{table}: / still filters"
        );
        let (footer, help) = shown_hints(&sim);
        assert!(!footer.contains("q:back"), "{table}: {footer}");
        assert!(!footer.contains("/:filter"), "{table}: {footer}");
        assert!(footer.contains("?:help"), "{table}: {footer}");
        assert!(!help.contains("q / Esc"), "{table}: {help}");
        screen(&mut sim, "log");
        let (footer, _) = shown_hints(&sim);
        assert!(footer.contains("q:back"), "{table} on log: {footer}");
        assert!(footer.contains("/:filter"), "{table} on log: {footer}");
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
    assert!(footer.contains("q:back") && footer.contains("/:filter"), "{footer}");
    keys(&mut sim, "/");
    assert!(sim.app.project_frame().is_some_and(|f| f.filtering));
    // A help key written for the screen acts, is shown in the footer, and is in the help; the
    // global `?` stays (a context-level noop does not hide a global key).
    for table in ["git.changes", "git_changes"] {
        let mut cfg = config("git", "en");
        cfg.keys
            .surfaces
            .insert(table.into(), [("Z".into(), "toggle_help".into())].into());
        let mut sim = open(&root, cfg, (180, 24));
        let (footer, _) = shown_hints(&sim);
        assert!(footer.contains("Z:help") || footer.contains("?/Z:help"), "{table}: {footer}");
        keys(&mut sim, "Z");
        assert_eq!(sim.app.key_context(), Some(Context::Help), "{table}");
        keys(&mut sim, "?");
        screen(&mut sim, "log");
        let (footer, _) = shown_hints(&sim);
        assert!(!footer.contains("Z:help") && !footer.contains("/Z:help"), "{table} on log: {footer}");
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
        assert!(!footer.contains("q:back"), "{table}: {footer}");
        assert!(footer.contains("?/q:help"), "{table}: {footer}");
        assert!(!help.contains("q / Esc"), "{table}: {help}");
        keys(&mut sim, "q");
        assert!(sim.app.project_active(), "{table}: q still leaves");
        assert_eq!(sim.app.key_context(), Some(Context::Help), "{table}: q opens help");
        keys(&mut sim, "?");
        // Git log has no entry for `q`: it leaves there, as before.
        screen(&mut sim, "log");
        let (footer, _) = shown_hints(&sim);
        assert!(footer.contains("q:back"), "{table} on log: {footer}");
        // `/` rebound to the help: no filter, no `/:filter`.
        let mut cfg = config("git", "en");
        cfg.keys
            .surfaces
            .insert(table.into(), [("/".into(), "toggle_help".into())].into());
        let mut sim = open(&root, cfg, (180, 24));
        let (footer, _) = shown_hints(&sim);
        assert!(!footer.contains("/:filter"), "{table}: {footer}");
        assert!(footer.contains("q:back"), "{table}: {footer}");
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
        assert!(footer.contains("q:back") && footer.contains("/:filter"), "{table}: {footer}");
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
    assert!(footer.contains("q:back"), "{footer}");
}
