use super::*;
use konoma_plugin::registry::{ManifestCount, MAX_REFRESH_TARGETS};
use konoma_plugin::view::ListRow;
#[cfg(feature = "proc")]
use konoma_plugin::view::ListView;

#[cfg(feature = "proc")]
#[path = "host_transport.rs"]
mod transport;

fn answers() -> ProjectAct {
    ProjectAct::new()
        .screen("branches")
        .selected(ProjectRow::new("branch", "branch-2"))
        .marked([
            ProjectRow::new("branch", "branch-1"),
            ProjectRow::new("branch", "branch-2"),
        ])
        .confirmation("-D")
        .text("renamed")
        .choice("main")
}

fn targets() -> Vec<RefreshTarget> {
    vec![
        RefreshTarget::CurrentScreen,
        RefreshTarget::screen("branches"),
        RefreshTarget::AllScreens,
        RefreshTarget::TreeStatus,
        RefreshTarget::Chip,
    ]
}

#[test]
fn project_view_selection_and_row_values_are_explicit() {
    assert!(ViewReq::new(80, 24).project.is_none());
    let project = ProjectView::new("branches").selection("branch-2");
    assert_eq!(project.screen, "branches");
    assert_eq!(project.selection.as_deref(), Some("branch-2"));
    let req = ViewReq::new(10, 3).project(project.clone());
    assert_eq!(req.project, Some(project));
    let row = ListRow::new("branch", vec![])
        .value("branch-2")
        .marked(true)
        .checked(false);
    assert_eq!(row.value, "branch-2");
    let selected = ProjectRow::from(&row);
    assert_eq!(selected.kind, "branch");
    assert_eq!(selected.value, "branch-2");
    assert_eq!(
        ProjectRow::from(&row),
        ProjectRow::new("branch", "branch-2")
    );
    assert!(row.marked);
    assert_eq!(row.checked, Some(false));
    assert!(ListRow::new("legacy", vec![]).value.is_empty());
}

#[test]
fn project_act_answers_are_optional_and_distinct() {
    assert!(ActCx::bare().project.is_none());
    let cx = ActCx::bare().with_project(answers());
    let p = cx.project.unwrap();
    assert_eq!(p.screen.as_deref(), Some("branches"));
    assert_eq!(p.selected, Some(ProjectRow::new("branch", "branch-2")));
    assert_eq!(
        p.marked,
        [
            ProjectRow::new("branch", "branch-1"),
            ProjectRow::new("branch", "branch-2")
        ]
    );
    assert_eq!(p.confirmation.as_deref(), Some("-D"));
    assert_eq!(p.text.as_deref(), Some("renamed"));
    assert_eq!(p.choice.as_deref(), Some("main"));
    assert!(
        ProjectAct::new().screen.is_none(),
        "tree/diff actions need no screen"
    );
}

#[test]
fn plugin_read_only_is_static_and_opt_in() {
    assert!(!Manifest::new("p", "1").read_only);
    let m = Manifest::new("p", "1")
        .read_only()
        .action(Act::new("write").writes());
    assert!(m.read_only);
    let r = check(m).unwrap();
    assert!(r.manifest("p").unwrap().read_only);
}

#[test]
fn project_refresh_targets_validate_names_and_counts() {
    let mut act = Act::new("write");
    for t in targets() {
        act = act.refresh(t);
    }
    let r = check(manifest().action(act.clone())).unwrap();
    assert_eq!(r.manifest("p").unwrap().actions[0].refresh, targets());
    for m in [manifest(), Manifest::new("p", "1").screen("missing")] {
        assert_eq!(
            reason(m.action(Act::new("write").refresh(RefreshTarget::screen("missing")))),
            ManifestReason::UnknownRefreshScreen {
                action: "write".into(),
                screen: "missing".into()
            }
        );
    }
    act.refresh = vec![RefreshTarget::TreeStatus; MAX_REFRESH_TARGETS];
    check(manifest().action(act.clone())).unwrap();
    act.refresh.push(RefreshTarget::Chip);
    assert_eq!(
        reason(manifest().action(act)),
        ManifestReason::TooMany {
            kind: ManifestCount::RefreshTargets,
            count: MAX_REFRESH_TARGETS + 1,
            limit: MAX_REFRESH_TARGETS
        }
    );
}

#[cfg(feature = "proc")]
mod wire {
    use super::*;
    use konoma_plugin::proc::{frame, wire::*};
    use std::path::Path;

    fn bytes<T: serde::Serialize + serde::de::DeserializeOwned>(t: &T) -> T {
        frame::decode(&frame::encode(t).unwrap()).unwrap()
    }

    #[test]
    fn project_view_request_round_trip() {
        for p in [
            ProjectView::new("branches"),
            ProjectView::new("branches").selection("branch-2"),
        ] {
            assert_eq!(project_view_from_wire(bytes(&project_view_to_wire(&p))), p);
            let req = ViewReq::new(101, 37)
                .repr("graph")
                .page(2)
                .cell_px(8, 16)
                .project(p);
            let w = req_to_wire(&req);
            assert_eq!(req_from_wire(bytes(&w)).unwrap(), req);
            assert_eq!(req_protocol(&w), 2);
            assert!(require_protocol(1, req_protocol(&w)).is_err());
        }
        assert_eq!(
            WProjectView::new("branches").selection("branch-2"),
            project_view_to_wire(&ProjectView::new("branches").selection("branch-2"))
        );
        assert_eq!(
            WViewReq::new(80, 24).project(WProjectView::new("branches")),
            req_to_wire(&ViewReq::new(80, 24).project(ProjectView::new("branches")))
        );
    }

    #[test]
    fn project_action_context_round_trip() {
        for optional in [
            ProjectAct::new(),
            ProjectAct::new().confirmation("").text("").choice(""),
        ] {
            assert_eq!(
                project_act_from_wire(bytes(&project_act_to_wire(&optional))),
                optional
            );
        }
        let p = answers();
        assert_eq!(project_act_from_wire(bytes(&project_act_to_wire(&p))), p);
        let row = ProjectRow::new("branch", "branch-2");
        assert_eq!(
            project_row_from_wire(bytes(&project_row_to_wire(&row))),
            row
        );
        assert_eq!(
            WProjectRow::new("branch", "branch-2"),
            project_row_to_wire(&row)
        );
        let expected = WProjectAct::new()
            .screen("branches")
            .selected(WProjectRow::new("branch", "branch-2"))
            .marked([
                WProjectRow::new("branch", "branch-1"),
                WProjectRow::new("branch", "branch-2"),
            ])
            .confirmation("-D")
            .text("renamed")
            .choice("main");
        assert_eq!(project_act_to_wire(&p), expected);
        let cx = ActCx::bare().with_project(p.clone());
        let w = act_cx_to_wire(&cx).unwrap();
        assert_eq!(cx_from_wire(bytes(&w)).unwrap().project, Some(p.clone()));
        assert_eq!(cx_protocol(&w), 2);
        assert!(require_protocol(1, cx_protocol(&w)).is_err());
        assert_eq!(WCx::new("en").project(expected), w);
        assert_eq!(
            CxData::new(Settings::new(), "en")
                .project(p.clone())
                .project,
            Some(p)
        );
    }

    #[test]
    fn project_row_values_round_trip_and_require_two_without_a_mark() {
        let side = Side::new(Path::new("/unused-project-row-value"), 'h');
        let row = ListRow::new("branch", vec![]).value("branch-2");
        let expected = ListView { rows: vec![row] };
        let w = view_to_wire(&mut View::List(expected.clone()), &mut Enc::new(&side)).unwrap();
        assert_eq!(view_protocol(&w), 2);
        assert!(require_protocol(1, view_protocol(&w)).is_err());
        assert!(
            matches!(view_from_wire(bytes(&w), &mut Dec::new(&side)).unwrap(), View::List(v) if v == expected)
        );
    }

    #[test]
    fn plugin_read_only_round_trip_and_gating() {
        for read_only in [false, true] {
            let mut m = Manifest::new("p", "1");
            m.read_only = read_only;
            let w = manifest_to_wire(&m).unwrap();
            assert_eq!(manifest_from_wire(bytes(&w)).unwrap().read_only, read_only);
            let json = konoma_plugin::proc::manifest_to_json(&m).unwrap();
            assert_eq!(
                konoma_plugin::proc::manifest_from_json(&json)
                    .unwrap()
                    .read_only,
                read_only
            );
            assert_eq!(manifest_protocol(&w), if read_only { 2 } else { 1 });
            assert_eq!(
                require_protocol(1, manifest_protocol(&w)).is_err(),
                read_only
            );
        }
    }

    #[test]
    fn project_refresh_targets_round_trip_and_gating() {
        for t in targets() {
            assert_eq!(
                refresh_target_from_wire(bytes(&refresh_target_to_wire(&t))),
                t
            );
        }
        assert_eq!(
            WRefreshTarget::screen("branches"),
            refresh_target_to_wire(&RefreshTarget::screen("branches"))
        );
        let mut act = Act::new("write");
        act.refresh = targets();
        let m = manifest().action(act);
        let back = manifest_from_wire(bytes(&manifest_to_wire(&m).unwrap())).unwrap();
        assert_eq!(back.actions[0].refresh, targets());
        let w = manifest_to_wire(
            &Manifest::new("p", "1").action(Act::new("write").refresh(RefreshTarget::TreeStatus)),
        )
        .unwrap();
        assert_eq!(manifest_protocol(&w), 2);
        assert!(require_protocol(1, manifest_protocol(&w)).is_err());
    }

    #[test]
    fn project_call_fields_keep_legacy_bytes_and_defaults() {
        #[derive(serde::Serialize)]
        struct OldReq {
            repr: Option<String>,
            page: Option<u64>,
            cols: u16,
            rows: u16,
            cell_px: Option<(u16, u16)>,
        }
        #[derive(serde::Serialize)]
        struct OldCx {
            settings: Vec<(String, WValue)>,
            lang: String,
            pixels: bool,
            colors: u8,
            external: bool,
            read_dirs: Option<Vec<WPath>>,
        }
        let old_req = frame::encode(&OldReq {
            repr: None,
            page: None,
            cols: 80,
            rows: 24,
            cell_px: None,
        })
        .unwrap();
        assert_eq!(
            frame::encode(&req_to_wire(&ViewReq::new(80, 24))).unwrap(),
            old_req
        );
        assert!(req_from_wire(frame::decode(&old_req).unwrap())
            .unwrap()
            .project
            .is_none());
        let old_cx = frame::encode(&OldCx {
            settings: vec![],
            lang: "en".into(),
            pixels: false,
            colors: 0,
            external: false,
            read_dirs: None,
        })
        .unwrap();
        assert_eq!(
            frame::encode(&act_cx_to_wire(&ActCx::bare()).unwrap()).unwrap(),
            old_cx
        );
        assert!(cx_from_wire(frame::decode(&old_cx).unwrap())
            .unwrap()
            .project
            .is_none());
        assert_eq!(req_protocol(&WViewReq::new(80, 24)), 1);
        assert_eq!(cx_protocol(&WCx::new("en")), 1);
        let legacy = manifest_to_wire(&Manifest::new("p", "1").action(Act::new("old"))).unwrap();
        let json = serde_json::to_value(legacy).unwrap();
        assert!(json.get("read_only").is_none());
        assert!(json["actions"][0].get("refresh").is_none());
        let row: WListRow =
            serde_json::from_str(r#"{"cells":[],"kind":"file","target":null,"checked":null}"#)
                .unwrap();
        assert!(row.value.is_empty());
        assert!(serde_json::to_value(row).unwrap().get("value").is_none());
    }
}
