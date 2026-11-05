use konoma_plugin::prelude::*;

#[test]
fn detail_is_pushed_and_log_enter_pushes_full_commit_id_only() {
    for plugin in [&konoma_vcs::Git as &dyn Plugin, &konoma_vcs::Jj] {
        let manifest = plugin.manifest();
        let screen = manifest
            .project
            .as_ref()
            .unwrap()
            .screens
            .iter()
            .find(|s| s.id == "detail")
            .unwrap();
        assert!(screen.pushed);
        assert_eq!(screen.key, None);
        assert_eq!(screen.title.get("en"), "Detail");
        assert_eq!(screen.title.get("ja"), "詳細");
        let mut session = plugin
            .open(
                Input::Project {
                    root: "/missing-fixture".into(),
                    marker: ".git".into(),
                },
                &OpenCx::bare(),
            )
            .unwrap();
        assert!(!manifest
            .actions
            .iter()
            .any(|a| a.key.as_deref() == Some("l")));
        {
            let key = "Enter";
            let action = manifest.actions.iter().find(|a| a.key.as_deref() == Some(key) && matches!(&a.project_context, Some(ActContext::Screen { screen, row_kinds }) if screen == "log" && row_kinds == &["commit"])).unwrap();
            for kind in ["commit", "empty"] {
                let outcome = session
                    .act(
                        &action.id,
                        &ActCx::bare().with_project(
                            ProjectAct::new()
                                .screen("log")
                                .selected(ProjectRow::new(kind, "full-id")),
                        ),
                    )
                    .unwrap();
                assert_eq!(
                    outcome,
                    if kind == "commit" {
                        Outcome::PushScreen {
                            screen: "detail".into(),
                            selection: Some("full-id".into()),
                        }
                    } else {
                        Outcome::Unhandled
                    }
                );
            }
        }
        for selection in [None, Some("unknown")] {
            let mut project = ProjectView::new("detail");
            if let Some(id) = selection {
                project = project.selection(id);
            }
            let View::Diff(diff) = session
                .view(&ViewReq::new(80, 20).project(project), &ViewCx::bare())
                .unwrap()
            else {
                panic!("diff required")
            };
            assert!(diff.header.is_empty() && diff.hunks.is_empty());
        }
    }
}
