use konoma_plugin::prelude::*;

#[test]
fn graph_declarations_respect_layers_backend_and_copy_contexts() {
    for (plugin, jj) in [
        (&konoma_vcs::Git as &dyn Plugin, false),
        (&konoma_vcs::Jj as &dyn Plugin, true),
    ] {
        let manifest = plugin.manifest();
        let screens = &manifest.project.as_ref().unwrap().screens;
        let graph = screens.iter().position(|s| s.id == "graph").unwrap();
        assert_eq!(screens[graph - 1].id, "log");
        assert_eq!(screens[graph].title.get("en"), "Graph");
        assert_eq!(screens[graph].title.get("ja"), "グラフ");
        assert_eq!(screens[graph].key, None);
        assert_eq!(
            screens[graph].row_kinds,
            ["commit", "worktree", "connector", "empty"]
        );
        let (sanitized, issues) = konoma_plugin::keys::sanitize(&manifest);
        let acts: Vec<_> = sanitized.actions.iter().filter(|a| {
            matches!(&a.project_context, Some(ActContext::Screen { screen, .. }) if screen == "graph")
        }).collect();
        assert!(
            !issues.iter().any(|i| i.act.starts_with("graph")),
            "{issues:?}"
        );
        assert!(
            !issues.iter().any(|i| i.act.starts_with("picker_")),
            "{issues:?}"
        );
        for (key, expected) in [("s", !jj), ("x", !jj), ("a", jj), ("b", true)] {
            assert_eq!(
                acts.iter()
                    .any(|a| !a.copy && a.key.as_deref() == Some(key)),
                expected
            );
        }
        assert_eq!(acts.iter().filter(|a| a.copy).count(), 6);
        assert!(acts.iter().filter(|a| a.copy).all(|a| {
            matches!(&a.project_context, Some(ActContext::Screen { row_kinds, .. }) if row_kinds == &["commit"])
        }));
        for (key, default) in [
            ("graph_max_branches", Value::Int(12)),
            ("graph_base_branches", Value::List(vec![])),
        ] {
            assert_eq!(
                manifest
                    .settings
                    .iter()
                    .find(|s| s.name == key)
                    .unwrap()
                    .default,
                default
            );
        }
    }
}

#[cfg(feature = "git")]
mod reading {
    use super::*;
    use konoma_plugin::view::ListView;
    use konoma_vcs_core::{git, git2};

    fn fixture() -> konoma_test_support::TmpDir {
        let dir = konoma_test_support::unique_tmp("vcs_graph_rows");
        let repo = git2::Repository::init(&dir).unwrap();
        let tree = repo
            .find_tree(repo.index().unwrap().write_tree().unwrap())
            .unwrap();
        let sig = git2::Signature::new(
            "Author",
            "fixture@example.invalid",
            &git2::Time::new(1_577_836_800, 0),
        )
        .unwrap();
        let id = repo
            .commit(Some("HEAD"), &sig, &sig, "日本語 subject", &tree, &[])
            .unwrap();
        let commit = repo.find_commit(id).unwrap();
        repo.branch("hidden-one", &commit, false).unwrap();
        repo.branch("priority", &commit, false).unwrap();
        std::fs::write(dir.join("untracked"), "new\n").unwrap();
        dir
    }

    fn session(dir: &std::path::Path) -> Box<dyn Session> {
        konoma_vcs::Git
            .open(
                Input::Project {
                    root: dir.into(),
                    marker: ".git".into(),
                },
                &OpenCx::bare(),
            )
            .unwrap()
    }

    fn view(session: &mut dyn Session, tab: u64, settings: &Settings, lang: &str) -> ListView {
        let View::List(list) = session
            .view(
                &ViewReq::new(80, 20).project(ProjectView::new("graph").tab(tab).icons(false)),
                &ViewCx::new(settings, lang),
            )
            .unwrap()
        else {
            panic!("list required")
        };
        list
    }

    #[test]
    fn graph_rows_match_core_lanes_metadata_filter_hidden_refs_and_icons_in_both_languages() {
        let dir = fixture();
        for lang in ["en", "ja"] {
            for align in ["right", "inline", "unknown"] {
                let settings = Settings::new()
                    .with("graph_max_branches", Value::Int(1))
                    .with("commit_meta_align", Value::Str(align.into()));
                let mut session = session(&dir);
                let list = view(session.as_mut(), 1, &settings, lang);
                let commit = list.rows.iter().find(|r| r.kind == "commit").unwrap();
                let head = git::branches_by_recency(&dir)
                    .into_iter()
                    .find(|(_, head, _)| *head)
                    .unwrap()
                    .0;
                assert!(!commit.cells[0].plain().contains("hidden-one"));
                assert!(!commit.filter_text.as_ref().unwrap().contains("hidden-one"));
                for query in [
                    "subject",
                    "Author",
                    "2020-01-01",
                    &head,
                    commit.name.as_deref().unwrap(),
                ] {
                    assert!(commit.matches_filter(query), "{query}");
                }
                let core = konoma_vcs_core::vcs::graph(
                    false,
                    &dir,
                    None,
                    &git::WorktreeLabels::default(),
                    Some(std::slice::from_ref(&head)),
                );
                for (row, expected) in list.rows.iter().zip(&core) {
                    assert_eq!(
                        row.cells[0].spans[..expected.graph.len()]
                            .iter()
                            .map(|s| s.text.clone())
                            .collect::<Vec<_>>(),
                        expected
                            .graph
                            .iter()
                            .map(|(t, _)| t.clone())
                            .collect::<Vec<_>>()
                    );
                    if row.kind == "connector" {
                        assert!(!row.selectable);
                        assert_eq!(row.filter_text.as_deref(), Some(""));
                    }
                }
                let worktree = &list.rows[0];
                assert_eq!(worktree.kind, "worktree");
                assert!(worktree.selectable);
                assert_eq!(
                    worktree.cells[0].spans.last().unwrap().style,
                    if align == "inline" {
                        Style::new().dim()
                    } else {
                        Style::new().fg(Color::Yellow).bold()
                    }
                );
                assert!(worktree.cells[0].plain().contains(if lang == "en" {
                    "Uncommitted changes"
                } else {
                    "未コミットの変更"
                }));
                for row in [commit, worktree] {
                    assert_eq!(row.right.is_none(), align == "inline");
                    if align == "inline" {
                        assert!(row.cells[0].spans.last().unwrap().text.starts_with("  "));
                    }
                }
                let strip = list.bottom.unwrap();
                assert!(strip.chips[0].plain().starts_with("br: ● "));
                assert_eq!(
                    strip.chips.last().unwrap().plain(),
                    if lang == "en" {
                        "(+2 hidden)"
                    } else {
                        "(+2 非表示)"
                    }
                );
            }
        }
    }

    #[test]
    fn graph_base_actions_are_per_tab_clear_once_and_do_not_pin_noncommits() {
        let dir = fixture();
        let mut session = session(&dir);
        for lang in ["en", "ja"] {
            let settings = Settings::new().with("graph_max_branches", Value::Int(1));
            let original = view(session.as_mut(), 1, &settings, lang);
            let worktree = ProjectRow::from(&original.rows[0]);
            let commit =
                ProjectRow::from(original.rows.iter().find(|r| r.kind == "commit").unwrap());
            let act = |row, tab| {
                ActCx::new(&settings, lang)
                    .with_project(ProjectAct::new().screen("graph").tab(tab).selected(row))
            };
            assert_eq!(
                session.act("graph_set_base", &act(worktree, 1)).unwrap(),
                Outcome::Flash(
                    t!(en: "select a commit to set as base", ja: "基準にするコミットを選んでください")
                )
            );
            assert_eq!(view(session.as_mut(), 1, &settings, lang), original);
            let Outcome::Flash(flash) = session
                .act("graph_set_base", &act(commit.clone(), 1))
                .unwrap()
            else {
                panic!("base flash required")
            };
            let pinned = view(session.as_mut(), 1, &settings, lang);
            let label = git::branches_by_recency(&dir)
                .into_iter()
                .find(|(_, head, _)| *head)
                .unwrap()
                .0;
            assert_eq!(
                flash.get(lang),
                format!("{}{label}", if lang == "en" { "base: " } else { "基準: " })
            );
            assert!(pinned
                .title
                .unwrap()
                .plain()
                .contains(&format!("base: {label}")));
            let second = view(
                session.as_mut(),
                2,
                &Settings::new().with("graph_max_branches", Value::Int(0)),
                lang,
            );
            assert!(!second.title.unwrap().plain().contains("base:"));
            assert!(second
                .bottom
                .unwrap()
                .chips
                .iter()
                .any(|c| c.plain().contains("hidden-one")));
            assert_eq!(
                session
                    .act("graph_clear_base", &act(commit.clone(), 1))
                    .unwrap(),
                Outcome::Flash(t!(en: "base cleared", ja: "基準を解除しました"))
            );
            assert_eq!(view(session.as_mut(), 1, &settings, lang), original);
            assert_eq!(
                session
                    .act("graph_clear_base", &act(commit.clone(), 1))
                    .unwrap(),
                Outcome::Unhandled
            );
            assert_eq!(
                session.act("graph_all", &act(commit, 1)).unwrap(),
                Outcome::Unhandled
            );
        }
    }

    #[test]
    fn graph_enter_pushes_shared_commit_detail_and_worktree_diff_and_copy_ignores_worktree() {
        let dir = fixture();
        let mut session = session(&dir);
        let list = view(session.as_mut(), 1, Settings::empty(), "en");
        for row in list.rows.iter().filter(|row| row.selectable) {
            let cx = ActCx::bare().with_project(
                ProjectAct::new()
                    .screen("graph")
                    .tab(1)
                    .selected(ProjectRow::from(row)),
            );
            assert_eq!(
                session.act("graph_detail", &cx).unwrap(),
                Outcome::PushScreen {
                    screen: "detail".into(),
                    selection: Some(row.value.clone())
                }
            );
            let View::Diff(diff) = session
                .view(
                    &ViewReq::new(80, 20).project(ProjectView::new("detail").selection(&row.value)),
                    &ViewCx::bare(),
                )
                .unwrap()
            else {
                panic!()
            };
            if row.kind == "worktree" {
                assert!(diff.header.is_empty());
                assert!(!diff.hunks.is_empty());
                assert_eq!(
                    session.act("graph_copy_subject", &cx).unwrap(),
                    Outcome::Unhandled
                );
            } else {
                assert!(diff
                    .header
                    .iter()
                    .any(|line| line.plain() == "日本語 subject"));
                assert_eq!(
                    session.act("graph_copy_subject", &cx).unwrap(),
                    Outcome::Copy {
                        text: "日本語 subject".into()
                    }
                );
            }
        }
    }

    #[test]
    fn graph_empty_screen_is_localized_dim_and_nonselectable() {
        let dir = konoma_test_support::unique_tmp("vcs_graph_empty");
        git2::Repository::init(&dir).unwrap();
        for lang in ["en", "ja"] {
            let list = view(session(&dir).as_mut(), 1, Settings::empty(), lang);
            assert_eq!(list.rows.len(), 1);
            assert_eq!(list.rows[0].kind, "empty");
            assert!(!list.rows[0].selectable);
            assert_eq!(
                list.rows[0].cells[0],
                Line::from(Span::styled(
                    if lang == "en" {
                        "  (no commits)"
                    } else {
                        "  (コミットなし)"
                    },
                    Style::new().dim()
                ))
            );
        }
    }
}
