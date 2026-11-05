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
