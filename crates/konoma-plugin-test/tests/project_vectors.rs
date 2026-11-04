//! Language-neutral protocol 2 examples, sent through the actual MessagePack framing.

use konoma_plugin::proc::{frame, manifest_from_json, wire::*};
use konoma_plugin::*;

#[test]
fn project_v2_manifest_vector() {
    let m = manifest_from_json(include_str!("vectors/project-v2.json")).unwrap();
    let manifest = manifest_to_wire(&m).unwrap();
    let back =
        manifest_from_wire(frame::decode(&frame::encode(&manifest).unwrap()).unwrap()).unwrap();
    assert_eq!(manifest_to_wire(&back).unwrap(), manifest);
    assert_eq!(manifest_protocol(&manifest), 2);
    assert!(require_protocol(1, manifest_protocol(&manifest)).is_err());
    let project = back.project.as_ref().unwrap();
    assert_eq!(project.markers.len(), 3);
    assert_eq!(project.screens[0].title.get("ja"), "ブランチ");
    assert!(project.version_control);
    assert!(back.read_only);
    assert_eq!(
        back.actions[0].refresh,
        vec![
            RefreshTarget::CurrentScreen,
            RefreshTarget::screen("branches"),
            RefreshTarget::AllScreens,
            RefreshTarget::TreeStatus,
            RefreshTarget::Chip
        ]
    );
    assert_eq!(
        back.actions[0].confirmation.as_ref().unwrap().choices[1]
            .choice
            .value,
        "-D"
    );
    struct Decl(Manifest);
    impl Plugin for Decl {
        fn manifest(&self) -> Manifest {
            self.0.clone()
        }
        fn open(&self, _: Input, _: &OpenCx) -> Result<Box<dyn Session>, Failure> {
            Err(Failure::unsupported("fixture"))
        }
    }
    konoma_plugin::registry::Registry::new()
        .register(
            std::sync::Arc::new(Decl(back)),
            konoma_plugin::registry::PluginOrigin::User,
        )
        .unwrap();
}

#[test]
fn project_v2_input_outcomes_and_list_vectors() {
    use konoma_plugin::view::{ListRow, ListView};
    // These values have no side files: decoding must not access this path.
    let side = Side::new(std::path::Path::new("/unused-project-vector"), 'h');
    let input = input_to_wire(
        &Input::Project {
            root: "/p".into(),
            marker: ".git".into(),
        },
        &mut Enc::new(&side),
    )
    .unwrap();
    let back = input_from_wire(
        frame::decode(&frame::encode(&input).unwrap()).unwrap(),
        &mut Dec::new(&side),
    )
    .unwrap();
    assert!(
        matches!(&back, Input::Project { root, marker } if root == std::path::Path::new("/p") && marker == ".git")
    );
    assert_eq!(input_to_wire(&back, &mut Enc::new(&side)).unwrap(), input);
    for o in [
        Outcome::PushScreen {
            screen: "branches".into(),
            selection: Some("main".into()),
        },
        Outcome::OpenFile {
            path: "/p/f".into(),
            line: Some(3),
        },
        Outcome::OpenDiff {
            path: "/p/f".into(),
        },
        Outcome::SwitchRoot { root: "/p".into() },
        Outcome::OpenTab { path: "/p".into() },
    ] {
        let w = outcome_to_wire(&o);
        assert_eq!(outcome_protocol(&w), 2);
        let back =
            outcome_from_wire_checked(frame::decode(&frame::encode(&w).unwrap()).unwrap()).unwrap();
        assert_eq!(back, o);
    }
    let mut row = ListRow::new("branch", vec![]);
    row.marked = true;
    row.checked = Some(false);
    row.target = Some("/p".into());
    let expected = ListView { rows: vec![row] };
    let w = view_to_wire(&mut View::List(expected.clone()), &mut Enc::new(&side)).unwrap();
    assert_eq!(view_protocol(&w), 2);
    let back = view_from_wire(
        frame::decode(&frame::encode(&w).unwrap()).unwrap(),
        &mut Dec::new(&side),
    )
    .unwrap();
    assert!(matches!(back, View::List(l) if l == expected));
}

#[test]
fn project_v2_request_context_and_row_vectors() {
    let record = |name| {
        let hex = include_str!("vectors/project-calls-v2.msgpack.hex")
            .lines()
            .filter_map(|line| line.split_once(' '))
            .find(|(key, _)| *key == name)
            .unwrap()
            .1;
        assert_eq!(hex.len() % 2, 0);
        (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
            .collect::<Vec<_>>()
    };
    let req: WViewReq = frame::decode(&record("req")).unwrap();
    let cx: WCx = frame::decode(&record("cx")).unwrap();
    let view: WView = frame::decode(&record("view")).unwrap();
    assert_eq!(frame::encode(&req).unwrap(), record("req"));
    assert_eq!(frame::encode(&cx).unwrap(), record("cx"));
    assert_eq!(frame::encode(&view).unwrap(), record("view"));
    for required in [req_protocol(&req), cx_protocol(&cx), view_protocol(&view)] {
        assert_eq!(required, 2);
        assert!(require_protocol(1, required).is_err());
    }
    let req = req_from_wire(frame::decode(&frame::encode(&req).unwrap()).unwrap()).unwrap();
    assert_eq!(
        req.project,
        Some(ProjectView::new("branches").selection("branch-2"))
    );
    let cx = cx_from_wire(frame::decode(&frame::encode(&cx).unwrap()).unwrap()).unwrap();
    assert_eq!(
        cx.project,
        Some(
            ProjectAct::new()
                .screen("branches")
                .selected(ProjectRow::new("branch", "branch-2"))
                .marked([
                    ProjectRow::new("branch", "branch-1"),
                    ProjectRow::new("branch", "branch-2")
                ])
                .confirmation("-D")
                .text("renamed")
                .choice("main")
        )
    );
    let side = Side::new(std::path::Path::new("/unused-project-calls-vector"), 'h');
    let View::List(list) = view_from_wire(
        frame::decode(&frame::encode(&view).unwrap()).unwrap(),
        &mut Dec::new(&side),
    )
    .unwrap() else {
        panic!("list")
    };
    assert_eq!(
        ProjectRow::from(&list.rows[0]),
        ProjectRow::new("branch", "branch-2")
    );
}
