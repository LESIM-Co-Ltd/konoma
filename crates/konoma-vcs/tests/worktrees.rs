use konoma_plugin::prelude::*;

#[test]
fn worktrees_git_declaration_settings_and_actions_jj_has_no_screen_or_jump() {
    let manifest = konoma_vcs::Git.manifest();
    let screens = &manifest.project.as_ref().unwrap().screens;
    let position = screens.iter().position(|s| s.id == "worktrees").unwrap();
    assert_eq!(screens[position - 1].id, "log");
    let screen = &screens[position];
    assert_eq!(screen.key, Some('w'));
    assert_eq!(screen.title.get("en"), "Worktrees");
    assert_eq!(screen.title.get("ja"), "ワークツリー");
    assert_eq!(
        screen.row_kinds,
        ["worktree", "current", "bare", "unavailable", "empty"]
    );
    assert!(!screen.marks && !screen.checks && !screen.pushed);
    assert!(screens.iter().any(|s| s.id == "worktree_diff" && s.pushed));
    for (name, default) in [
        ("worktree_dir", Value::Str("../".into())),
        ("graph_base_branches", Value::List(vec![])),
    ] {
        let setting = manifest.settings.iter().find(|s| s.name == name).unwrap();
        assert_eq!(setting.default, default);
        assert_ne!(setting.help.get("en"), setting.help.get("ja"));
    }
    let acts: Vec<_> = manifest.actions.iter().filter(|a| matches!(&a.project_context, Some(ActContext::Screen { screen, .. }) if screen == "worktrees")).collect();
    assert_eq!(acts.len(), 3);
    for key in ["Enter", "c", "d"] {
        let action = acts.iter().find(|a| a.key.as_deref() == Some(key)).unwrap();
        assert_ne!(action.label.get("en"), action.label.get("ja"));
        assert_ne!(action.help.get("en"), action.help.get("ja"));
        assert_eq!(action.writes, key == "c");
        if key == "c" {
            let input = action.text_input.as_ref().unwrap();
            assert!(!input.keep_on_failure);
            assert!(input.initial.is_empty());
            assert_eq!(input.label.get("en"), "New worktree: branch name");
            assert_eq!(input.label.get("ja"), "新規ワークツリーのブランチ名");
        }
    }
    let jj = konoma_vcs::Jj.manifest();
    assert!(!jj
        .project
        .as_ref()
        .unwrap()
        .screens
        .iter()
        .any(|s| s.id == "worktrees" || s.key == Some('w')));
    assert!(!jj
        .settings
        .iter()
        .any(|s| s.name == "worktree_dir" || s.name == "graph_base_branches"));
}

#[cfg(feature = "git")]
mod reading {
    use super::*;
    use konoma_plugin::view::ListView;
    use konoma_vcs_core::{git, git2};
    use std::path::Path;

    fn command(root: &Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .current_dir(root)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn fixture() -> konoma_test_support::TmpDir {
        let dir = konoma_test_support::unique_tmp("vcs_worktrees");
        std::fs::create_dir_all(&dir).unwrap();
        let dir = dir.into_canonical().unwrap();
        let root = dir.join("main");
        let repo = git2::Repository::init(&root).unwrap();
        let signature = git2::Signature::new(
            "Fixture",
            "fixture@example.invalid",
            &git2::Time::new(1_577_836_800, 0),
        )
        .unwrap();
        let tree = repo
            .find_tree(repo.index().unwrap().write_tree().unwrap())
            .unwrap();
        repo.commit(Some("HEAD"), &signature, &signature, "seed", &tree, &[])
            .unwrap();
        for (name, branch) in [("linked", "日本語"), ("gone", "gone")] {
            command(
                &root,
                &[
                    "worktree",
                    "add",
                    "-qb",
                    branch,
                    dir.join(name).to_str().unwrap(),
                ],
            );
        }
        command(
            &root,
            &[
                "worktree",
                "add",
                "-q",
                "--detach",
                dir.join("detached").to_str().unwrap(),
            ],
        );
        command(
            &root,
            &[
                "worktree",
                "lock",
                "--reason",
                "fixture lock",
                dir.join("linked").to_str().unwrap(),
            ],
        );
        std::fs::remove_dir_all(dir.join("gone")).unwrap();
        dir
    }

    fn rows(root: &Path, lang: &str) -> ListView {
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
                &ViewReq::new(160, 20).project(
                    ProjectView::new("worktrees").home_dir(Some(root.parent().unwrap().into())),
                ),
                &ViewCx::new(Settings::empty(), lang),
            )
            .unwrap()
        else {
            panic!("list required")
        };
        list
    }

    #[test]
    fn worktrees_rows_match_core_order_names_paths_suffixes_targets_and_filter_only_branch_or_path()
    {
        let dir = fixture();
        for root in [dir.join("main"), dir.join("linked")] {
            let expected = git::worktrees(&root);
            let list = rows(&root, "en");
            let actual: Vec<_> = list.rows.iter().filter(|r| r.kind != "empty").collect();
            assert_eq!(actual.len(), expected.len());
            for (row, worktree) in actual.iter().zip(expected) {
                let kind = if worktree.prunable {
                    "unavailable"
                } else if worktree.is_current {
                    "current"
                } else {
                    "worktree"
                };
                assert_eq!(row.kind, kind);
                assert_eq!(row.value, worktree.path.to_string_lossy());
                assert_eq!(
                    row.name.as_deref(),
                    Some(worktree.branch.as_deref().unwrap_or(&row.value))
                );
                assert_eq!(
                    row.target.as_ref(),
                    (kind == "worktree").then_some(&worktree.path)
                );
                let head = worktree.head.as_deref().unwrap();
                assert!(!row.matches_filter(head), "the head is not filter text");
                assert!(row.matches_filter(worktree.path.file_name().unwrap().to_str().unwrap()));
                if let Some(branch) = &worktree.branch {
                    assert!(row.matches_filter(&branch.to_uppercase()));
                }
                let name = worktree.branch.as_deref().unwrap_or("(detached)");
                let text = format!(
                    "{}{name}  {head}  ~/{}{}{}{}",
                    if worktree.is_current { "* " } else { "  " },
                    worktree.path.file_name().unwrap().to_str().unwrap(),
                    if worktree.is_main { "  main" } else { "" },
                    if worktree.locked.is_some() {
                        "  locked: fixture lock"
                    } else {
                        ""
                    },
                    if worktree.prunable { "  prunable" } else { "" }
                );
                assert_eq!(row.cells[0].plain(), text);
                assert_eq!(row.cells[0].spans[0].style.fg, Some(Color::Green));
                assert_eq!(
                    row.cells[0].spans[1].style,
                    if worktree.is_current {
                        Style::new().fg(Color::Green).bold()
                    } else {
                        Style::new()
                    }
                );
                for suffix in row.cells[0].spans.iter().skip(3) {
                    assert_eq!(suffix.style, Style::new().dim());
                }
            }
        }
    }

    #[test]
    fn worktrees_empty_and_bare_rows_are_localized_and_unselectable_or_without_target() {
        let dir = fixture();
        for (lang, text) in [("en", "  (no worktrees)"), ("ja", "  (ワークツリーなし)")] {
            let list = rows(&dir.join("not-a-repo"), lang);
            assert_eq!(list.rows.len(), 1);
            assert_eq!(list.rows[0].kind, "empty");
            assert!(!list.rows[0].selectable);
            assert_eq!(list.rows[0].cells[0].plain(), text);
            assert_eq!(list.rows[0].cells[0].spans[0].style, Style::new().dim());
        }
        command(&dir, &["clone", "-q", "--bare", "main", "bare"]);
        command(
            &dir.join("bare"),
            &[
                "worktree",
                "add",
                "-q",
                "--detach",
                dir.join("bare-linked").to_str().unwrap(),
            ],
        );
        let list = rows(&dir.join("bare-linked"), "en");
        let row = &list.rows[0];
        assert_eq!(row.kind, "bare");
        assert!(row.target.is_none());
        assert_eq!(row.name.as_deref(), Some(row.value.as_str()));
        assert_eq!(
            row.cells[0].plain(),
            "  (bare)  -------  ~/bare  main  bare"
        );
    }

    #[test]
    fn worktrees_enter_rechecks_current_missing_bare_and_only_switches_available_worktrees() {
        let dir = fixture();
        command(&dir, &["clone", "-q", "--bare", "main", "bare"]);
        command(
            &dir.join("bare"),
            &[
                "worktree",
                "add",
                "-q",
                "--detach",
                dir.join("bare-linked").to_str().unwrap(),
            ],
        );
        for root in [dir.join("main"), dir.join("bare-linked")] {
            let mut session = konoma_vcs::Git
                .open(
                    Input::Project {
                        root: root.clone(),
                        marker: ".git".into(),
                    },
                    &OpenCx::bare(),
                )
                .unwrap();
            let list = rows(&root, "en");
            for lang in ["en", "ja"] {
                for row in &list.rows {
                    let cx = ActCx::new(Settings::empty(), lang).with_project(
                        ProjectAct::new()
                            .screen("worktrees")
                            .selected(ProjectRow::from(row)),
                    );
                    let result = session.act("worktree_switch", &cx).unwrap();
                    let core_kind = git::worktrees(&root)
                        .into_iter()
                        .find(|w| w.path.to_string_lossy() == row.value)
                        .map(|w| {
                            if w.is_bare {
                                "bare"
                            } else if w.prunable || !w.path.is_dir() {
                                "unavailable"
                            } else if w.is_current {
                                "current"
                            } else {
                                "worktree"
                            }
                        })
                        .unwrap_or("empty");
                    let expected = match core_kind {
                        "worktree" => Outcome::SwitchRoot {
                            root: row.value.clone().into(),
                            message: None,
                        },
                        "current" => Outcome::Flash(
                            t!(en: "already in this worktree", ja: "既にこのワークツリーにいます"),
                        ),
                        "bare" => Outcome::Flash(
                            t!(en: "bare repository — it has no checkout", ja: "ベアリポジトリ（実体のチェックアウトがありません）"),
                        ),
                        "unavailable" => Outcome::Flash(
                            t!(en: "worktree is missing (run `git worktree prune`)", ja: "ワークツリーが見つかりません（`git worktree prune` を実行してください）"),
                        ),
                        _ => Outcome::Unhandled,
                    };
                    assert_eq!(result, expected);
                }
            }
        }
    }

    #[test]
    fn worktrees_create_trims_flattens_uses_main_and_setting_existing_branch_blank_and_raw_failure()
    {
        let dir = fixture();
        let main = dir.join("main");
        let linked = dir.join("linked");
        let create = |root: &Path, name: &str, settings: &Settings| {
            let mut session = konoma_vcs::Git
                .open(
                    Input::Project {
                        root: root.into(),
                        marker: ".git".into(),
                    },
                    &OpenCx::bare(),
                )
                .unwrap();
            session.act(
                "worktree_create",
                &ActCx::new(settings, "en")
                    .with_project(ProjectAct::new().screen("worktrees").text(name)),
            )
        };
        for name in ["", "   \t"] {
            assert_eq!(
                create(&main, name, Settings::empty()).unwrap(),
                Outcome::Flash(t!(en: "name is empty", ja: "名前が空です"))
            );
        }
        command(&main, &["branch", "existing"]);
        let settings = Settings::new().with("worktree_dir", Value::Str("../placed".into()));
        for (root, name, settings, target) in [
            (
                &linked,
                "  feat/nested  ",
                Settings::empty(),
                dir.join("feat-nested"),
            ),
            (&main, "existing", &settings, dir.join("placed/existing")),
        ] {
            let outcome = create(root, name, settings).unwrap();
            let path = target.canonicalize().unwrap();
            assert_eq!(
                outcome,
                Outcome::SwitchRoot {
                    root: path.clone(),
                    message: Some(
                        t!(en: format!("Created worktree: {}", path.display()), ja: format!("ワークツリー作成: {}", path.display()))
                    )
                }
            );
            assert!(path.join(".git").is_file());
            assert!(git::branch_tip(&main, name.trim()).is_some());
        }
        let name = git::worktrees(&main)[0].branch.clone().unwrap();
        let target = main.join("../").join(&name);
        let expected = git::worktree_add(&main, &target, &name, false)
            .unwrap_err()
            .to_string();
        let error = create(&main, &name, Settings::empty()).unwrap_err();
        assert_eq!(error.message.get("en"), expected);
        assert_eq!(error.message.get("ja"), expected);
    }

    fn commit_file(root: &Path, file: &str, text: &str, time: i64) {
        std::fs::write(root.join(file), text).unwrap();
        let repo = git2::Repository::open(root).unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new(file)).unwrap();
        index.write().unwrap();
        let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
        let parent = repo.head().unwrap().peel_to_commit().unwrap();
        let sig = git2::Signature::new(
            "Fixture",
            "fixture@example.invalid",
            &git2::Time::new(time, 0),
        )
        .unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, file, &tree, &[&parent])
            .unwrap();
    }

    #[test]
    fn worktrees_changes_base_uses_newest_merge_base_then_first_on_ties_excludes_own_and_falls_back(
    ) {
        let dir = fixture();
        let main = dir.join("main");
        command(&main, &["branch", "older"]);
        commit_file(&main, "base.txt", "base\n", 1_577_837_000);
        command(&main, &["branch", "newer"]);
        command(&main, &["branch", "tied"]);
        let feature = dir.join("feature");
        command(
            &main,
            &[
                "worktree",
                "add",
                "-qb",
                "feature",
                feature.to_str().unwrap(),
                "newer",
            ],
        );
        commit_file(&feature, "feature.txt", "feature\n", 1_577_837_100);
        let mut session = konoma_vcs::Git
            .open(
                Input::Project {
                    root: main.clone(),
                    marker: ".git".into(),
                },
                &OpenCx::bare(),
            )
            .unwrap();
        for (candidates, base) in [
            (vec!["older", "feature", "newer"], "newer"),
            (vec!["tied", "newer", "older"], "tied"),
        ] {
            let settings = Settings::new().with(
                "graph_base_branches",
                Value::List(candidates.iter().map(|s| Value::Str((*s).into())).collect()),
            );
            let context = ProjectAct::new()
                .screen("worktrees")
                .selected(ProjectRow::new("worktree", feature.to_string_lossy()));
            let outcome = session
                .act(
                    "worktree_changes",
                    &ActCx::new(&settings, "en").with_project(context),
                )
                .unwrap();
            assert_eq!(
                outcome,
                Outcome::PushScreen {
                    screen: "worktree_diff".into(),
                    selection: Some(feature.to_string_lossy().into())
                }
            );
            let View::Diff(diff) = session
                .view(
                    &ViewReq::new(80, 20).project(
                        ProjectView::new("worktree_diff").selection(feature.to_string_lossy()),
                    ),
                    &ViewCx::new(&settings, "en"),
                )
                .unwrap()
            else {
                panic!("diff required")
            };
            assert_eq!(
                diff.header[0].plain(),
                format!("feature ← {base}  {}", feature.display())
            );
            assert_eq!(diff.hunks.len(), 1);
            assert_eq!(diff.hunks[0].path.as_deref(), Some("feature.txt"));
            assert_eq!(diff.hunks[0].lines[0].text, "feature");
        }
        for (changed, lang) in [(false, "en"), (true, "ja")] {
            if changed {
                std::fs::write(main.join("base.txt"), "uncommitted\n").unwrap();
            }
            let context = ProjectAct::new()
                .screen("worktrees")
                .selected(ProjectRow::new("current", main.to_string_lossy()));
            let outcome = session
                .act(
                    "worktree_changes",
                    &ActCx::new(Settings::empty(), lang).with_project(context),
                )
                .unwrap();
            if !changed {
                assert_eq!(
                    outcome,
                    Outcome::Flash(t!(en: "no changes", ja: "変更なし"))
                );
            } else {
                assert!(matches!(outcome, Outcome::PushScreen { .. }));
                let View::Diff(diff) = session
                    .view(
                        &ViewReq::new(80, 20).project(
                            ProjectView::new("worktree_diff").selection(main.to_string_lossy()),
                        ),
                        &ViewCx::new(Settings::empty(), lang),
                    )
                    .unwrap()
                else {
                    panic!("diff required")
                };
                assert!(diff.header[0].plain().ends_with("(未コミットの変更)"));
                assert!(diff
                    .hunks
                    .iter()
                    .flat_map(|h| &h.lines)
                    .any(|l| l.text == "uncommitted"));
            }
        }
        for row in rows(&main, "en")
            .rows
            .iter()
            .filter(|r| r.kind == "unavailable")
        {
            assert_eq!(
                session
                    .act(
                        "worktree_changes",
                        &ActCx::bare().with_project(
                            ProjectAct::new()
                                .screen("worktrees")
                                .selected(ProjectRow::from(row))
                        )
                    )
                    .unwrap(),
                Outcome::Flash(
                    t!(en: "worktree is missing (run `git worktree prune`)", ja: "ワークツリーが見つかりません（`git worktree prune` を実行してください）")
                )
            );
        }
    }
}
