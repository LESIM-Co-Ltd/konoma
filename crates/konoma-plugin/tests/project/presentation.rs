use konoma_plugin::view::ListRow;
#[cfg(feature = "proc")]
use konoma_plugin::view::ListView;
use konoma_plugin::*;

#[derive(Clone)]
#[cfg(feature = "proc")]
struct Project(bool);
#[cfg(feature = "proc")]
impl Plugin for Project {
    fn manifest(&self) -> Manifest {
        Manifest::new("presentation", "1").project(
            ProjectDecl::new()
                .marker(DetectionMarker::new("project", MarkerKind::File))
                .watch("declared")
                .screen(
                    ProjectScreen::new("changes", "Changes")
                        .marks(true)
                        .checks(true),
                ),
        )
    }
    fn open(&self, _: Input, _: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        Ok(Box::new(self.clone()))
    }
}
#[cfg(feature = "proc")]
impl Session for Project {
    fn view(&mut self, req: &ViewReq, _: &ViewCx) -> Result<View, Failure> {
        assert_eq!(req.project, Some(context()));
        Ok(View::List(ListView {
            title: None,
            bottom: None,
            rows: vec![
                ListRow::new("empty", vec![Line::raw("empty")]).selectable(false),
                ListRow::new("commit", vec![Line::raw("subject")])
                    .right(Line::raw("author · date"))
                    .filter_text("short subject author date"),
            ],
        }))
    }
    fn watch_paths(&mut self, cx: &ViewCx) -> Result<Option<Vec<std::path::PathBuf>>, Failure> {
        assert_eq!(cx.lang, "ja");
        Ok(Some(vec![if self.0 {
            "/next/index"
        } else {
            "/outside/index"
        }
        .into()]))
    }
    fn act(&mut self, _: &str, _: &ActCx) -> Result<Outcome, Failure> {
        self.0 = true;
        Ok(Outcome::Redraw)
    }
}
#[cfg(feature = "proc")]
fn context() -> ProjectView {
    ProjectView::new("changes")
        .paths("/project/sub", "/launch")
        .home_dir(Some("/home".into()))
}

#[test]
fn project_presentation_defaults_are_explicit_and_empty_rows_are_not_selectable() {
    let screen = ProjectScreen::new("changes", "Changes");
    assert!(!screen.marks && !screen.checks);
    let row = ListRow::new("empty", vec![]).selectable(false);
    assert!(!row.selectable);
    assert!(ListRow::new("file", vec![]).selectable);
    struct DefaultSession;
    impl Session for DefaultSession {
        fn view(&mut self, _: &ViewReq, _: &ViewCx) -> Result<View, Failure> {
            unreachable!()
        }
    }
    assert_eq!(DefaultSession.watch_paths(&ViewCx::bare()).unwrap(), None);
}

#[test]
fn project_list_filter_override_and_display_fallback() {
    let row = ListRow::new("commit", vec![Line::raw("Subject")]).right(Line::raw("作者"));
    assert!(row.matches_filter("subject"));
    assert!(row.matches_filter("作者"));
    let row = row.filter_text("hash subject author date");
    assert!(row.matches_filter("SUBJECT AUTHOR"));
    assert!(!row.matches_filter("作者"));
    assert!(!row.matches_filter("·"));
    let row = row.filter_text("");
    assert!(row.matches_filter(""));
    assert!(!row.matches_filter("subject"));
}

#[cfg(feature = "proc")]
mod transport {
    use super::*;
    use crate::host_api::transport::{connect, finish};
    use konoma_plugin::proc::{frame, proto::*, wire::*};

    #[test]
    fn project_presentation_and_watch_paths_round_trip_through_client_and_server() {
        let (plugin, threads, _dir) = connect(|child| {
            konoma_plugin::proc::serve_io(
                Project(false),
                child.try_clone().unwrap(),
                child,
                &Default::default(),
            )
        });
        let declaration = plugin.manifest().project.unwrap();
        assert!(declaration.screens[0].marks && declaration.screens[0].checks);
        let mut session = plugin
            .open(
                Input::Project {
                    root: "/project".into(),
                    marker: "project".into(),
                },
                &OpenCx::bare(),
            )
            .unwrap();
        let cx = ViewCx::new(Settings::empty(), "ja");
        assert_eq!(
            session.watch_paths(&cx).unwrap(),
            Some(vec!["/outside/index".into()])
        );
        let View::List(list) = session
            .view(&ViewReq::new(80, 20).project(context()), &cx)
            .unwrap()
        else {
            panic!("list required")
        };
        assert!(!list.rows[0].selectable);
        assert_eq!(list.rows[0].right, None);
        assert_eq!(list.rows[0].filter_text, None);
        assert_eq!(list.rows[1].right, Some(Line::raw("author · date")));
        assert_eq!(
            list.rows[1].filter_text.as_deref(),
            Some("short subject author date")
        );
        session.act("switch", &ActCx::bare()).unwrap();
        assert_eq!(
            session.watch_paths(&cx).unwrap(),
            Some(vec!["/next/index".into()])
        );
        drop(session);
        finish(plugin, threads);
    }

    #[test]
    fn project_presentation_old_wire_defaults_and_absolute_path_validation() {
        let screen = ProjectScreen::new("changes", "Changes");
        let json = serde_json::to_value(project_screen_to_wire(&screen)).unwrap();
        assert!(json.get("marks").is_none() && json.get("checks").is_none());
        assert_eq!(
            project_screen_from_wire(serde_json::from_value(json).unwrap()),
            screen
        );
        let old: WProjectView =
            serde_json::from_str(r#"{"screen":"changes","selection":null}"#).unwrap();
        assert_eq!(
            project_view_from_wire(old).unwrap(),
            ProjectView::new("changes")
        );
        let mut bad = project_view_to_wire(&context());
        bad.tree_root = Some(path_to_wire(std::path::Path::new("relative")));
        assert!(project_view_from_wire(bad).is_err());
        let call = ToChild::WatchPaths {
            sid: 1,
            rid: 2,
            cx: WCx::new("ja"),
        };
        assert_eq!(
            frame::decode::<ToChild>(&frame::encode(&call).unwrap()).unwrap(),
            call
        );
        let reply = Reply::WatchPaths {
            paths: Some(vec![path_to_wire(std::path::Path::new("/outside/index"))]),
            state: WState {
                layout: layout_to_wire(&konoma_plugin::view::Layout::single()),
                enabled: vec![],
            },
        };
        assert_eq!(
            frame::decode::<Reply>(&frame::encode(&reply).unwrap()).unwrap(),
            reply
        );
    }

    #[test]
    fn project_list_right_and_filter_text_round_trip_with_legacy_defaults() {
        let dir = konoma_test_support::unique_tmp("project_list_meta");
        let side = Side::new(dir.as_path(), 'h');
        for row in [
            ListRow::new("commit", vec![Line::raw("subject")])
                .right(Line::from(vec![Span::styled(
                    "author · date",
                    Style::new().fg(Color::DarkGray),
                )]))
                .filter_text("short subject author date"),
            ListRow::new("file", vec![Line::raw("legacy")]),
        ] {
            let mut view = View::List(ListView {
                title: None,
                bottom: None,
                rows: vec![row.clone()],
            });
            let wire = view_to_wire(&mut view, &mut Enc::new(&side)).unwrap();
            let json = serde_json::to_value(wire).unwrap();
            if row.right.is_none() {
                assert!(json["List"][0].get("right").is_none());
                assert!(json["List"][0].get("filter_text").is_none());
            }
            let wire = serde_json::from_value(json).unwrap();
            let View::List(list) = view_from_wire(wire, &mut Dec::new(&side)).unwrap() else {
                panic!("list required")
            };
            assert_eq!(list.rows, [row]);
        }
    }

    #[test]
    fn project_watch_paths_old_child_uses_declaration_without_an_unknown_call() {
        let (plugin, threads, _dir) = connect(|mut child| {
            let _: Hello = frame::recv(&mut child).unwrap().unwrap();
            let manifest = manifest_to_wire(&Project(false).manifest()).unwrap();
            assert!(!manifest.session_watch);
            frame::send(
                &mut child,
                &HelloReply::Ack {
                    version: 2,
                    manifest,
                },
            )
            .unwrap();
            let ToChild::Open { rid, .. } = frame::recv(&mut child).unwrap().unwrap() else {
                panic!("open required")
            };
            frame::send(
                &mut child,
                &FromChild {
                    rid,
                    answer: Answer::Ok(Reply::Opened {
                        state: WState {
                            layout: layout_to_wire(&konoma_plugin::view::Layout::single()),
                            enabled: vec![],
                        },
                    }),
                },
            )
            .unwrap();
            loop {
                match frame::recv::<_, ToChild>(&mut child).unwrap() {
                    Some(ToChild::Close { .. }) => {}
                    Some(ToChild::Shutdown) | None => break,
                    other => panic!("old child received unknown request: {other:?}"),
                }
            }
            Ok(())
        });
        let mut session = plugin
            .open(
                Input::Project {
                    root: "/project".into(),
                    marker: "project".into(),
                },
                &OpenCx::bare(),
            )
            .unwrap();
        assert_eq!(session.watch_paths(&ViewCx::bare()).unwrap(), None);
        assert_eq!(session.watch_paths(&ViewCx::bare()).unwrap(), None);
        drop(session);
        finish(plugin, threads);
    }
}
