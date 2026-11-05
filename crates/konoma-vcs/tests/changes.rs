use konoma_plugin::prelude::*;

#[cfg(feature = "git")]
#[test]
fn vcs_changes_enter_opens_only_a_displayed_file_in_the_changes_screen() {
    let dir = konoma_test_support::unique_tmp("vcs_changes_enter");
    std::fs::create_dir_all(&dir).unwrap();
    git(&dir, &["init", "-q"]);
    std::fs::write(dir.join("日本語.txt"), "new\n").unwrap();
    let root = dir.canonicalize().unwrap();
    let mut session = konoma_vcs::Git
        .open(
            Input::Project {
                root: root.clone(),
                marker: ".git".into(),
            },
            &OpenCx::bare(),
        )
        .unwrap();
    let View::List(list) = session
        .view(
            &ViewReq::new(80, 20).project(ProjectView::new("changes")),
            &ViewCx::bare(),
        )
        .unwrap()
    else {
        panic!("expected list")
    };
    let selected = ProjectRow::from(&list.rows[0]);
    let context = ProjectAct::new()
        .screen("changes")
        .selected(selected.clone());
    assert_eq!(
        session
            .act("diff", &ActCx::bare().with_project(context))
            .unwrap(),
        Outcome::OpenDiff {
            path: root.join("日本語.txt")
        }
    );
    for context in [
        ProjectAct::new().screen("other").selected(selected.clone()),
        ProjectAct::new()
            .screen("changes")
            .selected(ProjectRow::new("empty", &selected.value)),
        ProjectAct::new()
            .screen("changes")
            .selected(ProjectRow::new("file", "not a displayed row")),
        ProjectAct::new().screen("changes"),
    ] {
        assert_eq!(
            session
                .act("diff", &ActCx::bare().with_project(context))
                .unwrap(),
            Outcome::Unhandled
        );
    }
    assert_eq!(
        session.act("stage", &ActCx::bare()).unwrap(),
        Outcome::Unhandled
    );
}

#[test]
fn vcs_changes_watch_declarations_cover_worktree_and_repository_metadata() {
    for (plugin, expected) in [
        (
            &konoma_vcs::Git as &dyn Plugin,
            vec![
                ".",
                ".git",
                ".git/HEAD",
                ".git/index",
                ".git/info/exclude",
                ".git/refs",
                ".git/packed-refs",
                ".gitignore",
            ],
        ),
        (
            &konoma_vcs::Jj as &dyn Plugin,
            vec![
                ".",
                ".jj/repo/op_heads",
                ".jj/repo/op_store",
                ".jj/working_copy",
                ".gitignore",
            ],
        ),
    ] {
        let project = plugin.manifest().project.unwrap();
        for path in expected {
            assert!(
                project.watch_paths.contains(&path.into()),
                "{} watch missing: {path}",
                plugin.manifest().id
            );
        }
    }
}

#[cfg(feature = "git")]
fn git(root: &std::path::Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .current_dir(root)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[cfg(feature = "git")]
#[test]
fn vcs_changes_git_rows_keep_status_staging_order_and_cjk() {
    let dir = konoma_test_support::unique_tmp("vcs_changes_rows");
    std::fs::create_dir_all(&dir).unwrap();
    git(&dir, &["init", "-q"]);
    for name in ["a-staged", "b-unstaged", "c-deleted", "d-old", "日本語.txt"] {
        std::fs::write(dir.join(name), "base\n").unwrap();
    }
    git(&dir, &["add", "."]);
    git(
        &dir,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-qm",
            "seed",
        ],
    );
    std::fs::write(dir.join("a-staged"), "index\n").unwrap();
    git(&dir, &["add", "a-staged"]);
    std::fs::write(dir.join("a-staged"), "worktree\n").unwrap();
    std::fs::write(dir.join("b-unstaged"), "worktree\n").unwrap();
    std::fs::remove_file(dir.join("c-deleted")).unwrap();
    git(&dir, &["mv", "d-old", "d-renamed"]);
    std::fs::write(dir.join("e-untracked"), "new\n").unwrap();
    std::fs::write(dir.join("f-added"), "new\n").unwrap();
    git(&dir, &["add", "f-added"]);
    std::fs::write(dir.join("日本語.txt"), "変更\n").unwrap();
    let root = dir.canonicalize().unwrap();
    let mut session = konoma_vcs::Git
        .open(
            Input::Project {
                root: root.clone(),
                marker: ".git".into(),
            },
            &OpenCx::bare(),
        )
        .unwrap();
    let View::List(list) = session
        .view(
            &ViewReq::new(80, 20).project(ProjectView::new("changes")),
            &ViewCx::bare(),
        )
        .unwrap()
    else {
        panic!("expected list")
    };
    let expected = [
        ("● M a-staged", Color::Yellow),
        ("  M b-unstaged", Color::Yellow),
        ("  D c-deleted", Color::Red),
        ("● R d-renamed", Color::Cyan),
        ("  U e-untracked", Color::LightGreen),
        ("● A f-added", Color::Green),
        ("  M 日本語.txt", Color::Yellow),
    ];
    assert_eq!(list.rows.len(), expected.len());
    for (row, (text, color)) in list.rows.iter().zip(expected) {
        assert_eq!(row.kind, "file");
        assert_eq!(row.cells[0].plain(), text);
        assert_eq!(row.cells[0].spans[1].style.fg, Some(color));
        assert_eq!(row.target.as_ref().unwrap().parent(), Some(root.as_path()));
    }
}

#[test]
fn vcs_changes_empty_uses_the_hub_message_in_both_languages() {
    let dir = konoma_test_support::unique_tmp("vcs_changes_empty");
    std::fs::create_dir_all(&dir).unwrap();
    for plugin in [&konoma_vcs::Git as &dyn Plugin, &konoma_vcs::Jj] {
        let mut session = plugin
            .open(
                Input::Project {
                    root: dir.to_path_buf(),
                    marker: ".git".into(),
                },
                &OpenCx::bare(),
            )
            .unwrap();
        for (lang, expected) in [("en", "  (no changes)"), ("ja", "  (変更なし)")] {
            let settings = Settings::new();
            let view = session
                .view(
                    &ViewReq::new(80, 20).project(ProjectView::new("changes")),
                    &ViewCx::new(&settings, lang),
                )
                .unwrap();
            let View::List(list) = view else {
                panic!("expected list")
            };
            assert_eq!(list.rows.len(), 1);
            assert_eq!(list.rows[0].kind, "empty");
            assert_eq!(list.rows[0].cells[0].plain(), expected);
            assert!(list.rows[0].cells[0].style.add.contains(Modifier::DIM));
        }
    }
}

#[test]
fn vcs_changes_declarations_match_existing_markers_and_separate_backend_writes() {
    for (plugin, id, marker, kind) in [
        (
            &konoma_vcs::Git as &dyn Plugin,
            "git",
            ".git",
            MarkerKind::Any,
        ),
        (
            &konoma_vcs::Jj as &dyn Plugin,
            "jj",
            ".jj",
            MarkerKind::Directory,
        ),
    ] {
        let manifest = plugin.manifest();
        assert_eq!(manifest.id, id);
        let mut registry = konoma_plugin::registry::Registry::new();
        let owned: std::sync::Arc<dyn Plugin> = if id == "git" {
            std::sync::Arc::new(konoma_vcs::Git)
        } else {
            std::sync::Arc::new(konoma_vcs::Jj)
        };
        registry
            .register(owned, konoma_plugin::registry::PluginOrigin::Standard)
            .unwrap();
        assert!(konoma_plugin::testing::check_manifest(plugin).is_empty());
        let project = manifest.project.expect("VCS project declaration");
        assert_eq!(project.markers, [DetectionMarker::new(marker, kind)]);
        assert!(project.version_control);
        assert_eq!(project.screens.len(), 4);
        assert_eq!(project.screens[0].id, "changes");
        assert_eq!(project.screens[0].key, Some('p'));
        assert!(project.screens[0].row_kinds.contains(&"file".into()));
        assert_eq!(manifest.actions.len(), if id == "git" { 29 } else { 23 });
        let action = manifest.actions.iter().find(|a| a.id == "diff").unwrap();
        assert_eq!(action.id, "diff");
        assert_eq!(
            action.project_context,
            Some(ActContext::screen("changes", ["file"]))
        );
        assert!(!action.writes);
        assert!(!action.destructive);
    }
}

#[test]
fn vcs_changes_write_contracts_and_enabled_follow_backend_and_available_rows() {
    let dir = konoma_test_support::unique_tmp("vcs_changes_enabled");
    std::fs::create_dir_all(&dir).unwrap();
    for (plugin, git) in [
        (&konoma_vcs::Git as &dyn Plugin, true),
        (&konoma_vcs::Jj, false),
    ] {
        let manifest = plugin.manifest();
        let mut session = plugin
            .open(
                Input::Project {
                    root: dir.to_path_buf(),
                    marker: String::new(),
                },
                &OpenCx::bare(),
            )
            .unwrap();
        session
            .view(
                &ViewReq::new(80, 20).project(ProjectView::new("changes")),
                &ViewCx::bare(),
            )
            .unwrap();
        assert!(!session.enabled("unknown"));
        assert!(!session.enabled("diff"));
        for id in ["stage", "unstage", "discard"] {
            assert!(!session.enabled(id));
        }
        for id in ["stage_all", "unstage_all", "commit"] {
            assert_eq!(session.enabled(id), git && cfg!(feature = "git"));
        }
        assert_eq!(session.enabled("sync"), !git && cfg!(feature = "git"));
        for a in manifest
            .actions
            .iter()
            .filter(|a| !["diff", "detail"].contains(&a.id.as_str()))
        {
            if a.copy {
                assert!(
                    a.id.starts_with("log_copy_")
                        || a.id.starts_with("detail_copy_")
                        || a.id.starts_with("graph_copy_")
                );
                assert!(!a.writes && !a.destructive);
                assert!(a.refresh.is_empty());
                assert_eq!(session.enabled(&a.id), cfg!(feature = "git"));
            } else if a.id.starts_with("graph_") {
                assert!(!a.writes && !a.destructive);
                assert_eq!(
                    session.enabled(&a.id),
                    cfg!(feature = "git")
                        && match a.id.as_str() {
                            "graph_set_base" | "graph_clear_base" => git,
                            "graph_all" => !git,
                            _ => true,
                        }
                );
            } else {
                assert!(a.writes);
                assert!(a.refresh.contains(&RefreshTarget::CurrentScreen));
                assert!(a.refresh.contains(&RefreshTarget::TreeStatus));
                assert_eq!(a.destructive, a.id == "discard");
                if a.id == "commit" || a.id == "sync" {
                    assert!(a.refresh.contains(&RefreshTarget::Chip));
                }
                if ["stage", "stage_all", "unstage", "unstage_all"].contains(&a.id.as_str()) {
                    assert!(a.hint_group.is_some());
                }
            }
        }
        if git {
            let commit = manifest.actions.iter().find(|a| a.id == "commit").unwrap();
            assert!(commit.text_input.as_ref().unwrap().keep_on_failure);
            let discard = manifest.actions.iter().find(|a| a.id == "discard").unwrap();
            let confirm = discard.confirmation.as_ref().unwrap();
            assert_eq!(confirm.choices[0].key, 'y');
            assert_eq!(confirm.text.get("en"), "Discard changes to {row} ?");
            assert_eq!(confirm.text.get("ja"), "変更を破棄しますか: {row} ?");
        } else {
            let sync = manifest.actions.iter().find(|a| a.id == "sync").unwrap();
            assert_eq!(
                sync.confirmation.as_ref().unwrap().setting.as_deref(),
                Some("confirm_sync")
            );
            let setting = manifest
                .settings
                .iter()
                .find(|s| s.name == "confirm_sync")
                .unwrap();
            assert_eq!(setting.default, Value::Bool(true));
        }
    }
}

#[cfg(feature = "git")]
#[test]
fn vcs_changes_jj_failed_sync_uses_legacy_messages() {
    let dir = konoma_test_support::unique_tmp("vcs_sync_failure");
    std::fs::create_dir_all(&dir).unwrap();
    let mut session = konoma_vcs::Jj
        .open(
            Input::Project {
                root: dir.to_path_buf(),
                marker: ".jj".into(),
            },
            &OpenCx::bare(),
        )
        .unwrap();
    for lang in ["en", "ja"] {
        let settings = Settings::new();
        let cx = ActCx::new(&settings, lang).with_project(ProjectAct::new().screen("changes"));
        let Outcome::Flash(message) = session.act("sync", &cx).unwrap() else {
            panic!("missing failure flash")
        };
        assert_eq!(
            message.get(lang),
            if lang == "en" {
                "jj could not snapshot the working copy"
            } else {
                "jj が作業コピーを取り込めませんでした"
            }
        );
    }
}
