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
fn vcs_changes_declarations_are_read_only_and_match_existing_markers() {
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
        assert_eq!(project.screens.len(), 1);
        assert_eq!(project.screens[0].id, "changes");
        assert_eq!(project.screens[0].key, Some('p'));
        assert!(project.screens[0].row_kinds.contains(&"file".into()));
        assert_eq!(manifest.actions.len(), 1);
        let action = &manifest.actions[0];
        assert_eq!(action.id, "diff");
        assert_eq!(
            action.project_context,
            Some(ActContext::screen("changes", ["file"]))
        );
        assert!(!action.writes);
        assert!(!action.destructive);
    }
}
