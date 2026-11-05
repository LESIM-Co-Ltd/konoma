use konoma_plugin::prelude::*;

#[test]
fn log_declaration_follows_changes_without_shortcut_or_actions() {
    for plugin in [&konoma_vcs::Git as &dyn Plugin, &konoma_vcs::Jj] {
        let manifest = plugin.manifest();
        let screens = &manifest.project.as_ref().unwrap().screens;
        assert_eq!(
            screens.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
            ["changes", "log"]
        );
        let log = &screens[1];
        assert_eq!(log.title.get("en"), "Log");
        assert_eq!(log.title.get("ja"), "ログ");
        assert_eq!(log.key, None);
        assert_eq!(log.row_kinds, ["commit", "empty"]);
        assert!(!log.marks && !log.checks);
        assert!(
            !manifest.actions.iter().any(|a| matches!(&a.project_context,
            Some(ActContext::Screen { screen, .. }) if screen == "log"))
        );
        let setting = manifest
            .settings
            .iter()
            .find(|s| s.name == "commit_meta_align")
            .unwrap();
        assert_eq!(setting.default, Value::Str("right".into()));
        assert_ne!(setting.help.get("en"), setting.help.get("ja"));
    }
}

#[cfg(feature = "git")]
mod reading {
    use super::*;
    use konoma_plugin::view::ListView;
    use konoma_vcs_core::git2;

    fn fixture(count: usize) -> konoma_test_support::TmpDir {
        let dir = konoma_test_support::unique_tmp("vcs_log_rows");
        let repo = git2::Repository::init(&dir).unwrap();
        let tree_id = repo.index().unwrap().write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        for i in 0..count {
            let signature = git2::Signature::new(
                "作者 Author",
                "fixture@example.invalid",
                &git2::Time::new(1_577_836_800 + i as i64, 0),
            )
            .unwrap();
            let parent = repo.head().ok().map(|h| h.peel_to_commit().unwrap());
            repo.commit(
                Some("HEAD"),
                &signature,
                &signature,
                &format!("Subject 日本語 {i}"),
                &tree,
                &parent.iter().collect::<Vec<_>>(),
            )
            .unwrap();
        }
        dir
    }

    fn rows(root: &std::path::Path, settings: &Settings, lang: &str) -> ListView {
        let mut session = konoma_vcs::Git
            .open(
                Input::Project {
                    root: root.into(),
                    marker: ".git".into(),
                },
                &OpenCx::bare(),
            )
            .unwrap();
        let View::List(list) = session
            .view(
                &ViewReq::new(80, 20).project(ProjectView::new("log")),
                &ViewCx::new(settings, lang),
            )
            .unwrap()
        else {
            panic!("list required")
        };
        for action in ["diff", "stage", "commit", "copy", "detail"] {
            assert_eq!(
                session
                    .act(
                        action,
                        &ActCx::bare().with_project(
                            ProjectAct::new()
                                .screen("log")
                                .selected(ProjectRow::from(&list.rows[0]))
                        )
                    )
                    .unwrap(),
                Outcome::Unhandled
            );
        }
        list
    }

    #[test]
    fn log_rows_use_full_identity_short_name_order_filter_and_three_alignments() {
        let dir = fixture(3);
        let expected = konoma_vcs_core::git::log(&dir, 200);
        for align in ["right", "inline", "unknown"] {
            let settings = Settings::new().with("commit_meta_align", Value::Str(align.into()));
            let list = rows(&dir, &settings, "en");
            assert_eq!(list.rows.len(), expected.len());
            for (row, commit) in list.rows.iter().zip(&expected) {
                assert_eq!(row.kind, "commit");
                assert_eq!(row.value, commit.id);
                assert_eq!(row.name.as_deref(), Some(commit.short.as_str()));
                assert!(row.selectable);
                assert_eq!(
                    row.filter_text.as_deref(),
                    Some(
                        format!(
                            "{} {} {} 2020-01-01 00:00",
                            commit.short, commit.summary, commit.author
                        )
                        .as_str()
                    )
                );
                assert_eq!(
                    row.cells[0].spans[0],
                    Span::styled(format!("{} ", commit.short), Style::new().dim())
                );
                if align == "inline" {
                    assert_eq!(row.right, None);
                    assert_eq!(
                        row.cells[0].plain(),
                        format!(
                            "{} {}  · {}  · 2020-01-01 00:00",
                            commit.short, commit.summary, commit.author
                        )
                    );
                    assert_eq!(row.cells[0].spans[2].style, Style::new().dim());
                } else {
                    let meta = row.right.as_ref().unwrap();
                    assert_eq!(meta.plain(), "作者 Author · 2020-01-01 00:00");
                    assert_eq!(meta.spans[0].style, Style::new().fg(Color::DarkGray));
                }
            }
        }
        assert!(rows(&dir, Settings::empty(), "en").rows[0].right.is_some());
    }

    #[test]
    fn log_empty_is_localized_dim_and_unselectable() {
        let dir = fixture(0);
        for (lang, text) in [("en", "  (no commits)"), ("ja", "  (コミットなし)")] {
            let list = rows(&dir, Settings::empty(), lang);
            assert_eq!(list.rows.len(), 1);
            let row = &list.rows[0];
            assert_eq!(row.kind, "empty");
            assert!(!row.selectable);
            assert_eq!(row.cells[0].plain(), text);
            assert_eq!(row.cells[0].spans[0].style, Style::new().dim());
        }
    }

    #[test]
    fn log_jj_rows_use_jj_commit_ids_and_change_names_in_a_colocated_repo() {
        if !konoma_vcs_core::vcs::jj::available() {
            return;
        }
        let dir = fixture(3);
        for args in [
            vec!["git", "init", "--colocate", "."],
            vec!["describe", "-m", "jj 専用"],
        ] {
            let output = std::process::Command::new("jj")
                .current_dir(&dir)
                .env("HOME", dir.as_path())
                .env("JJ_USER", "作者 Author")
                .env("JJ_EMAIL", "fixture@example.invalid")
                .args([
                    "--config",
                    "debug.commit-timestamp=\"2020-01-01T00:20:00Z\"",
                    "--config",
                    "debug.operation-timestamp=\"2020-01-01T00:20:00Z\"",
                ])
                .args(args)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        let mut session = konoma_vcs::Jj
            .open(
                Input::Project {
                    root: dir.to_path_buf(),
                    marker: ".jj".into(),
                },
                &OpenCx::bare(),
            )
            .unwrap();
        let View::List(list) = session
            .view(
                &ViewReq::new(80, 20).project(ProjectView::new("log")),
                &ViewCx::bare(),
            )
            .unwrap()
        else {
            panic!("list required")
        };
        let expected = konoma_vcs_core::vcs::jj::log(&dir, 200);
        assert_eq!(list.rows.len(), expected.len());
        for (row, commit) in list.rows.iter().zip(&expected) {
            assert_eq!(row.kind, "commit");
            assert_eq!(row.value, commit.id);
            assert_eq!(row.name.as_deref(), Some(commit.short.as_str()));
            assert!(row.filter_text.as_ref().unwrap().contains(&commit.summary));
        }
        assert_ne!(list.rows[0].value, konoma_vcs_core::git::log(&dir, 1)[0].id);
    }

    #[test]
    fn log_limit_is_exactly_200_newest_commits() {
        let dir = fixture(205);
        let list = rows(&dir, Settings::empty(), "en");
        assert_eq!(list.rows.len(), 200);
        assert!(list.rows[0].cells[0]
            .plain()
            .ends_with("Subject 日本語 204"));
        assert!(list.rows[199].cells[0]
            .plain()
            .ends_with("Subject 日本語 5"));
    }
}
