use konoma_plugin::prelude::*;

#[cfg(not(feature = "git"))]
#[test]
fn vcs_no_git_feature_declares_no_disabled_writes_on_any_screen() {
    let dir = konoma_test_support::unique_tmp("vcs_no_git_writes");
    for plugin in [&konoma_vcs::Git as &dyn Plugin, &konoma_vcs::Jj] {
        let manifest = plugin.manifest();
        let session = plugin
            .open(
                Input::Project {
                    root: dir.to_path_buf(),
                    marker: String::new(),
                },
                &OpenCx::bare(),
            )
            .unwrap();
        for action in &manifest.actions {
            assert!(!action.writes, "{} {}", manifest.id, action.id);
        }
        for id in [
            "stage",
            "stage_all",
            "unstage",
            "unstage_all",
            "discard",
            "commit",
            "sync",
            "checkout",
            "create_branch",
            "delete_branch",
            "delete_current_branch",
        ] {
            assert!(!session.enabled(id));
            assert!(
                !manifest.actions.iter().any(|action| action.id == id),
                "{} {id}",
                manifest.id
            );
        }
    }
}

#[test]
fn branches_declaration_titles_order_and_read_only_bookmarks() {
    for (plugin, title, ja, writes) in [
        (&konoma_vcs::Git as &dyn Plugin, "Branches", "ブランチ", 4),
        (
            &konoma_vcs::Jj as &dyn Plugin,
            "Bookmarks",
            "ブックマーク",
            0,
        ),
    ] {
        let manifest = plugin.manifest();
        let screens = &manifest.project.as_ref().unwrap().screens;
        let writes = if cfg!(feature = "git") { writes } else { 0 };
        assert_eq!(
            screens.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
            if manifest.id == "git" {
                vec![
                    "changes",
                    "log",
                    "branches",
                    "worktrees",
                    "detail",
                    "worktree_diff",
                ]
            } else {
                vec!["changes", "log", "branches", "detail"]
            }
        );
        let screen = &screens[2];
        assert_eq!(screen.title.get("en"), title);
        assert_eq!(screen.title.get("ja"), ja);
        assert_eq!(screen.key, Some('b'));
        assert_eq!(screen.row_kinds, ["branch", "current", "empty"]);
        assert!(!screen.marks && !screen.checks && !screen.pushed);
        let acts: Vec<_> = manifest.actions.iter().filter(|a| matches!(
            &a.project_context, Some(ActContext::Screen { screen, .. }) if screen == "branches"
        )).collect();
        assert_eq!(acts.iter().filter(|a| a.writes).count(), writes);
        let copy = acts.iter().find(|a| a.copy).unwrap();
        assert_eq!(copy.key.as_deref(), Some("n"));
        assert_eq!(copy.label.get("en"), "branch name");
        assert_eq!(copy.label.get("ja"), "ブランチ名");
        if writes > 0 {
            for (id, priority) in [
                ("checkout", 0),
                ("create_branch", 1),
                ("delete_branch", 2),
                ("delete_current_branch", 2),
            ] {
                assert_eq!(
                    acts.iter().find(|a| a.id == id).unwrap().footer,
                    Some(priority)
                );
            }
        }
    }
}

#[cfg(feature = "git")]
mod reading {
    use super::*;
    use konoma_test_support::{unique_tmp, TmpDir};
    use konoma_vcs_core::git2;

    fn fixture() -> TmpDir {
        let dir = unique_tmp("vcs_branches");
        std::fs::create_dir_all(&dir).unwrap();
        let repo = git2::Repository::init(&dir).unwrap();
        let tree = repo
            .find_tree(repo.index().unwrap().write_tree().unwrap())
            .unwrap();
        let sig = git2::Signature::new(
            "Fixture",
            "fixture@example.invalid",
            &git2::Time::new(1_577_836_800, 0),
        )
        .unwrap();
        let id = repo
            .commit(Some("HEAD"), &sig, &sig, "seed", &tree, &[])
            .unwrap();
        let commit = repo.find_commit(id).unwrap();
        repo.branch("alpha", &commit, false).unwrap();
        repo.branch("日本語", &commit, false).unwrap();
        dir
    }

    fn open(root: &std::path::Path) -> Box<dyn Session> {
        konoma_vcs::Git
            .open(
                Input::Project {
                    root: root.into(),
                    marker: ".git".into(),
                },
                &OpenCx::bare(),
            )
            .unwrap()
    }

    fn rows(session: &mut dyn Session, lang: &str) -> konoma_plugin::view::ListView {
        let View::List(list) = session
            .view(
                &ViewReq::new(80, 20).project(ProjectView::new("branches")),
                &ViewCx::new(Settings::empty(), lang),
            )
            .unwrap()
        else {
            panic!("list required")
        };
        list
    }

    #[test]
    fn branches_rows_order_identity_style_and_name_only_filter() {
        let dir = fixture();
        let expected = konoma_vcs_core::vcs::refs(&dir);
        let list = rows(open(&dir).as_mut(), "en");
        let actual: Vec<_> = list.rows.iter().filter(|r| r.selectable).collect();
        assert_eq!(actual.len(), 3);
        for (row, branch) in actual.iter().zip(expected) {
            assert_eq!(row.value, branch.name);
            assert_eq!(row.name.as_deref(), Some(branch.name.as_str()));
            assert_eq!(row.filter_text.as_deref(), Some(branch.name.as_str()));
            assert!(!row.matches_filter("*"));
            assert!(!row.matches_filter("  "));
            assert!(row.matches_filter(&branch.name.to_uppercase()));
            assert_eq!(
                row.kind,
                if branch.is_current {
                    "current"
                } else {
                    "branch"
                }
            );
            assert_eq!(
                row.cells[0].plain(),
                format!(
                    "{}{}",
                    if branch.is_current { "* " } else { "  " },
                    branch.name
                )
            );
            assert_eq!(row.cells[0].spans[0].style, Style::new().fg(Color::Green));
            assert_eq!(
                row.cells[0].spans[1].style,
                if branch.is_current {
                    Style::new().fg(Color::Green).bold()
                } else {
                    Style::new()
                }
            );
        }
    }

    #[test]
    fn branches_empty_is_dim_localized_and_unselectable() {
        let dir = unique_tmp("vcs_branches_empty");
        std::fs::create_dir_all(&dir).unwrap();
        git2::Repository::init(&dir).unwrap();
        for (lang, text) in [("en", "  (no branches)"), ("ja", "  (ブランチなし)")] {
            let list = rows(open(&dir).as_mut(), lang);
            assert_eq!(list.rows.len(), 1);
            assert_eq!(list.rows[0].kind, "empty");
            assert!(!list.rows[0].selectable);
            assert_eq!(list.rows[0].cells[0].plain(), text);
            assert_eq!(list.rows[0].cells[0].spans[0].style, Style::new().dim());
        }
    }

    fn act(
        session: &mut dyn Session,
        action: &str,
        kind: &str,
        value: &str,
        text: Option<&str>,
        confirmation: Option<&str>,
    ) -> Result<Outcome, Failure> {
        let mut project = ProjectAct::new()
            .screen("branches")
            .selected(ProjectRow::new(kind, value));
        if let Some(text) = text {
            project = project.text(text);
        }
        if let Some(choice) = confirmation {
            project = project.confirmation(choice);
        }
        session.act(action, &ActCx::bare().with_project(project))
    }

    #[test]
    fn branches_checkout_create_delete_and_copy_use_real_git() {
        let dir = fixture();
        let mut session = open(&dir);
        rows(session.as_mut(), "en");
        assert_eq!(
            act(session.as_mut(), "checkout", "branch", "alpha", None, None).unwrap(),
            Outcome::Flash(t!(en: "Switched to: alpha", ja: "切替: alpha"))
        );
        assert!(konoma_vcs_core::vcs::refs(&dir)
            .iter()
            .any(|r| r.name == "alpha" && r.is_current));
        assert_eq!(
            act(session.as_mut(), "checkout", "current", "alpha", None, None).unwrap(),
            Outcome::Flash(t!(en: "Switched to: alpha", ja: "切替: alpha"))
        );
        assert_eq!(
            act(
                session.as_mut(),
                "branch_copy_name",
                "current",
                "alpha",
                None,
                None
            )
            .unwrap(),
            Outcome::Copy {
                text: "alpha".into()
            }
        );
        assert_eq!(
            act(
                session.as_mut(),
                "create_branch",
                "empty",
                "",
                Some("  new-branch  "),
                None
            )
            .unwrap(),
            Outcome::Flash(t!(en: "Created branch: new-branch", ja: "ブランチ作成: new-branch"))
        );
        assert!(konoma_vcs_core::vcs::refs(&dir)
            .iter()
            .any(|r| r.name == "new-branch" && r.is_current));
        assert_eq!(
            act(
                session.as_mut(),
                "delete_branch",
                "branch",
                "alpha",
                None,
                None
            )
            .unwrap(),
            Outcome::Unhandled
        );
        assert_eq!(
            act(
                session.as_mut(),
                "delete_branch",
                "branch",
                "alpha",
                None,
                Some("safe")
            )
            .unwrap(),
            Outcome::Flash(t!(en: "Deleted branch: alpha", ja: "ブランチ削除: alpha"))
        );
        assert!(!konoma_vcs_core::vcs::refs(&dir)
            .iter()
            .any(|r| r.name == "alpha"));
    }

    #[test]
    fn branches_blank_name_current_delete_and_git_errors() {
        let dir = fixture();
        let mut session = open(&dir);
        for name in ["", "   "] {
            assert_eq!(
                act(
                    session.as_mut(),
                    "create_branch",
                    "empty",
                    "",
                    Some(name),
                    None
                )
                .unwrap(),
                Outcome::Flash(t!(en: "name is empty", ja: "名前が空です"))
            );
        }
        assert_eq!(
            act(
                session.as_mut(),
                "delete_current_branch",
                "current",
                "main",
                None,
                None
            )
            .unwrap(),
            Outcome::Flash(
                t!(en: "cannot delete the current branch", ja: "現在のブランチは削除できません")
            )
        );
        let expected = konoma_vcs_core::git::create_branch(&dir, "alpha")
            .unwrap_err()
            .to_string();
        let failure = act(
            session.as_mut(),
            "create_branch",
            "branch",
            "alpha",
            Some("alpha"),
            None,
        )
        .unwrap_err();
        assert_eq!(failure.message.get("en"), expected);
    }

    #[test]
    fn branches_safe_delete_refuses_unmerged_and_force_deletes_it() {
        let dir = fixture();
        let repo = git2::Repository::open(&dir).unwrap();
        let parent = repo.head().unwrap().peel_to_commit().unwrap();
        let sig = git2::Signature::new(
            "Fixture",
            "fixture@example.invalid",
            &git2::Time::new(1_577_836_860, 0),
        )
        .unwrap();
        let id = repo
            .commit(
                None,
                &sig,
                &sig,
                "unmerged",
                &parent.tree().unwrap(),
                &[&parent],
            )
            .unwrap();
        repo.branch("unmerged", &repo.find_commit(id).unwrap(), false)
            .unwrap();
        let mut session = open(&dir);
        let failure = act(
            session.as_mut(),
            "delete_branch",
            "branch",
            "unmerged",
            None,
            Some("safe"),
        )
        .unwrap_err();
        assert!(failure.message.get("en").contains("not fully merged"));
        assert!(konoma_vcs_core::vcs::refs(&dir)
            .iter()
            .any(|r| r.name == "unmerged"));
        assert!(matches!(
            act(
                session.as_mut(),
                "delete_branch",
                "branch",
                "unmerged",
                None,
                Some("force")
            )
            .unwrap(),
            Outcome::Flash(_)
        ));
        assert!(!konoma_vcs_core::vcs::refs(&dir)
            .iter()
            .any(|r| r.name == "unmerged"));
    }
}
