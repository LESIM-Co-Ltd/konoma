#[cfg(feature = "proc")]
#[test]
fn graph_sdk_wire_strip_title_tab_icons_and_old_json() {
    use konoma_plugin::prelude::*;
    use konoma_plugin::proc::wire::*;
    use konoma_plugin::view::{ListStrip, ListView};
    let dir = konoma_test_support::unique_tmp("graph_sdk_wire");
    let side = Side::new(dir.as_path(), 'h');
    let expected = ListView {
        title: Some(Line::raw("Graph (2) base: main")),
        bottom: Some(ListStrip::new(
            vec![Line::from(vec![Span::styled(
                "⎇ ● main",
                Style::new().fg(Color::Cyan).bold(),
            )])],
            9,
        )),
        rows: vec![],
    };
    let mut view = View::List(expected.clone());
    let wire = view_to_wire(&mut view, &mut Enc::new(&side)).unwrap();
    assert_eq!(view_protocol(&wire), 2);
    let wire: WView = serde_json::from_value(serde_json::to_value(wire).unwrap()).unwrap();
    let wire =
        konoma_plugin::proc::frame::decode(&konoma_plugin::proc::frame::encode(&wire).unwrap())
            .unwrap();
    let View::List(actual) = view_from_wire(wire, &mut Dec::new(&side)).unwrap() else {
        panic!()
    };
    assert_eq!(actual, expected);
    let legacy: WView = serde_json::from_str(r#"{"List":[]}"#).unwrap();
    let View::List(legacy) = view_from_wire(legacy, &mut Dec::new(&side)).unwrap() else {
        panic!()
    };
    assert_eq!(legacy, ListView::default());
    let mut legacy = View::List(legacy);
    assert_eq!(
        serde_json::to_string(&view_to_wire(&mut legacy, &mut Enc::new(&side)).unwrap()).unwrap(),
        r#"{"List":[]}"#
    );
    let request = ProjectView::new("graph").tab(17).icons(false);
    let bytes = konoma_plugin::proc::frame::encode(&project_view_to_wire(&request)).unwrap();
    assert_eq!(
        project_view_from_wire(konoma_plugin::proc::frame::decode(&bytes).unwrap()).unwrap(),
        request
    );
    let action = ProjectAct::new().screen("graph").tab(17);
    let bytes = konoma_plugin::proc::frame::encode(&project_act_to_wire(&action)).unwrap();
    assert_eq!(
        project_act_from_wire(konoma_plugin::proc::frame::decode(&bytes).unwrap()),
        action
    );
    let legacy: WProjectView =
        serde_json::from_str(r#"{"screen":"graph","selection":null}"#).unwrap();
    let legacy = project_view_from_wire(legacy).unwrap();
    assert_eq!(legacy.tab, 0);
    assert!(legacy.icons);
    let legacy: WProjectAct = serde_json::from_str(r#"{"screen":"graph","selected":null,"marked":[],"confirmation":null,"text":null,"choice":null}"#).unwrap();
    assert_eq!(project_act_from_wire(legacy).tab, 0);
}

#[cfg(feature = "proc")]
mod tab_close {
    use crate::host_api::transport::{connect, finish};
    use konoma_plugin::prelude::*;
    use konoma_plugin::proc::{frame, proto::*, serve_io, wire::*, ServeOptions};
    use std::collections::BTreeSet;

    #[derive(Default)]
    struct Tabs(BTreeSet<u64>);
    impl Plugin for Tabs {
        fn manifest(&self) -> Manifest {
            Manifest::new("tabs", "1")
                .project(ProjectDecl::new().screen(ProjectScreen::new("graph", "Graph")))
                .shows(ViewKind::Text)
        }
        fn open(&self, _: Input, _: &OpenCx) -> Result<Box<dyn Session>, Failure> {
            Ok(Box::new(Self::default()))
        }
    }
    impl Session for Tabs {
        fn view(&mut self, req: &ViewReq, _: &ViewCx) -> Result<View, Failure> {
            self.0.insert(req.project.as_ref().unwrap().tab);
            Ok(View::text(self.0.iter().map(u64::to_string)))
        }
        fn tab_closed(&mut self, tab: u64) {
            self.0.remove(&tab);
        }
    }

    #[test]
    fn graph_sdk_tab_closed_notice_reaches_shared_session_and_preserves_other_tabs() {
        let (plugin, threads, _dir) = connect(|child| {
            serve_io(
                Tabs::default(),
                child.try_clone().unwrap(),
                child,
                &ServeOptions::default(),
            )
        });
        let mut session = plugin
            .open(
                Input::Project {
                    root: "/project".into(),
                    marker: ".git".into(),
                },
                &OpenCx::bare(),
            )
            .unwrap();
        let read = |session: &mut dyn Session, tab| {
            let View::Text(text) = session
                .view(
                    &ViewReq::new(80, 20).project(ProjectView::new("graph").tab(tab)),
                    &ViewCx::bare(),
                )
                .unwrap()
            else {
                panic!()
            };
            let konoma_plugin::view::TextSource::Lines(lines) = text.source else {
                panic!()
            };
            lines.iter().map(Line::plain).collect::<Vec<_>>()
        };
        assert_eq!(read(session.as_mut(), 1), ["1"]);
        assert_eq!(read(session.as_mut(), 2), ["1", "2"]);
        session.tab_closed(2);
        assert_eq!(read(session.as_mut(), 1), ["1"]);
        drop(session);
        finish(plugin, threads);
    }

    #[test]
    fn graph_sdk_tab_closed_wire_defaults_and_old_child_receive_no_unknown_notice() {
        let manifest = manifest_to_wire(&Tabs::default().manifest()).unwrap();
        let mut json = serde_json::to_value(&manifest).unwrap();
        json.as_object_mut().unwrap().remove("session_tab_closed");
        let old: WManifest = serde_json::from_value(json).unwrap();
        assert!(!old.session_tab_closed);
        let notice = ToChild::TabClosed { sid: 7, tab: 2 };
        assert_eq!(
            frame::decode::<ToChild>(&frame::encode(&notice).unwrap()).unwrap(),
            notice
        );
        let old: ToChild = serde_json::from_str(r#"{"Close":{"sid":7}}"#).unwrap();
        assert_eq!(old, ToChild::Close { sid: 7 });
        let (plugin, threads, _dir) = connect(|mut child| {
            let _: Hello = frame::recv(&mut child).unwrap().unwrap();
            frame::send(
                &mut child,
                &HelloReply::Ack {
                    version: 2,
                    manifest: manifest_to_wire(&Tabs::default().manifest()).unwrap(),
                },
            )
            .unwrap();
            let ToChild::Open { rid, .. } = frame::recv(&mut child).unwrap().unwrap() else {
                panic!()
            };
            let state = WState {
                layout: layout_to_wire(&konoma_plugin::view::Layout::single()),
                enabled: vec![],
            };
            frame::send(
                &mut child,
                &FromChild {
                    rid,
                    answer: Answer::Ok(Reply::Opened {
                        state: state.clone(),
                    }),
                },
            )
            .unwrap();
            let ToChild::View { rid, .. } = frame::recv(&mut child).unwrap().unwrap() else {
                panic!("old child must receive View, never TabClosed")
            };
            frame::send(
                &mut child,
                &FromChild {
                    rid,
                    answer: Answer::Ok(Reply::Viewed {
                        view: WView::Message(vec![]),
                        state,
                    }),
                },
            )
            .unwrap();
            loop {
                match frame::recv::<_, ToChild>(&mut child).unwrap() {
                    Some(ToChild::Close { .. }) => {}
                    Some(ToChild::Shutdown) | None => break,
                    other => panic!("{other:?}"),
                }
            }
            Ok(())
        });
        let mut session = plugin
            .open(
                Input::Project {
                    root: "/project".into(),
                    marker: ".git".into(),
                },
                &OpenCx::bare(),
            )
            .unwrap();
        session.tab_closed(2);
        session
            .view(
                &ViewReq::new(80, 20).project(ProjectView::new("graph").tab(1)),
                &ViewCx::bare(),
            )
            .unwrap();
        drop(session);
        finish(plugin, threads);
    }
}

#[cfg(feature = "proc")]
#[test]
fn graph_sdk_wire_rejects_oversized_list_strips_in_both_directions() {
    use konoma_plugin::proc::{frame, wire::*};
    use konoma_plugin::view::{ListStrip, ListView, View};
    use konoma_plugin::Line;
    let dir = konoma_test_support::unique_tmp("graph_strip_limits");
    let side = Side::new(&dir, 'h');
    for (count, width, valid) in [
        (LIST_STRIP_MAX_CHIPS, LIST_STRIP_MAX_WIDTH, true),
        (LIST_STRIP_MAX_CHIPS + 1, 1, false),
        (1, LIST_STRIP_MAX_WIDTH + 1, false),
    ] {
        let mut view = View::List(ListView {
            bottom: Some(ListStrip::new(vec![Line::raw("chip"); count], width)),
            ..Default::default()
        });
        let encoded = view_to_wire(&mut view, &mut Enc::new(&side));
        assert_eq!(encoded.is_ok(), valid);
        if !valid {
            assert!(matches!(encoded, Err(WireError::TooLarge(_))));
        }
        let wire = WView::ListDecorated {
            rows: vec![],
            title: None,
            bottom: Some(WListStrip {
                chips: vec![line_to_wire(&Line::raw("chip")); count],
                estimated_width: width,
            }),
        };
        let wire = frame::decode(&frame::encode(&wire).unwrap()).unwrap();
        let decoded = view_from_wire(wire, &mut Dec::new(&side));
        assert_eq!(decoded.is_ok(), valid);
        if !valid {
            assert!(matches!(decoded, Err(WireError::TooLarge(_))));
        }
    }
}
