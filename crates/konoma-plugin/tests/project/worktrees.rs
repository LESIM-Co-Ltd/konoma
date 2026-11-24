use konoma_plugin::view::{ListRow, ListView};
use konoma_plugin::Line;

#[test]
fn project_worktree_list_fallback_only_without_matching_rows() {
    let list = ListView {
        rows: vec![
            ListRow::new("item", vec![Line::raw("branch path")]),
            ListRow::new("empty", vec![Line::raw("no worktrees")]).filter_fallback(true),
        ],
    };
    assert_eq!(list.visible_rows(""), [0]);
    assert_eq!(list.visible_rows("PATH"), [0]);
    assert_eq!(list.visible_rows("no match"), [1]);
    assert!(!list.rows[1].selectable);
}

#[cfg(feature = "proc")]
mod wire {
    use konoma_plugin::proc::wire::*;
    use konoma_plugin::view::{ListRow, ListView};
    use konoma_plugin::*;

    #[test]
    fn project_worktree_filter_label_round_trip_and_old_json_default() {
        let screen =
            ProjectScreen::new("worktrees", "Worktrees").filter_label(t!(en: "search", ja: "検索"));
        let bytes = konoma_plugin::proc::frame::encode(&project_screen_to_wire(&screen)).unwrap();
        assert_eq!(
            project_screen_from_wire(konoma_plugin::proc::frame::decode(&bytes).unwrap()),
            screen
        );
        let old = r#"{"id":"log","title":{"en":"Log","others":[]},"key":null,"row_kinds":[]}"#;
        let old = project_screen_from_wire(serde_json::from_str(old).unwrap());
        assert_eq!(old.filter_label, None);
        assert!(!serde_json::to_string(&project_screen_to_wire(&old))
            .unwrap()
            .contains("filter_label"));
    }

    #[test]
    fn project_worktree_switch_root_message_round_trip_and_old_json_defaults() {
        for message in [
            None,
            Some(t!(en: "Created worktree: /project", ja: "ワークツリー作成: /project")),
        ] {
            let expected = Outcome::SwitchRoot {
                root: "/project".into(),
                message,
            };
            let bytes = konoma_plugin::proc::frame::encode(&outcome_to_wire(&expected)).unwrap();
            assert_eq!(
                outcome_from_wire_checked(konoma_plugin::proc::frame::decode(&bytes).unwrap())
                    .unwrap(),
                expected
            );
        }
        let old = serde_json::json!({"SwitchRoot": {"root": path_to_wire(std::path::Path::new("/project"))}});
        let old: WOutcome = serde_json::from_value(old.clone()).unwrap();
        assert_eq!(
            outcome_from_wire_checked(old).unwrap(),
            Outcome::SwitchRoot {
                root: "/project".into(),
                message: None
            }
        );
        assert_eq!(
            serde_json::to_value(outcome_to_wire(&Outcome::SwitchRoot {
                root: "/project".into(),
                message: None
            }))
            .unwrap(),
            old_json()
        );
    }

    fn old_json() -> serde_json::Value {
        serde_json::json!({"SwitchRoot": {"root": path_to_wire(std::path::Path::new("/project"))}})
    }

    #[test]
    fn project_worktree_list_initial_selection_and_filter_fallback_round_trip_old_json_defaults() {
        let dir = konoma_test_support::unique_tmp("worktrees_wire");
        std::fs::create_dir_all(&dir).unwrap();
        let side = Side::new(&dir, 'x');
        let mut view = View::List(ListView {
            rows: vec![
                ListRow::new("current", vec![Line::raw("current")]).initial_selection(true),
                ListRow::new("empty", vec![Line::raw("empty")])
                    .selectable(false)
                    .filter_fallback(true),
            ],
        });
        let wire = view_to_wire(&mut view, &mut Enc::new(&side)).unwrap();
        let bytes = konoma_plugin::proc::frame::encode(&wire).unwrap();
        let View::List(back) = view_from_wire(
            konoma_plugin::proc::frame::decode(&bytes).unwrap(),
            &mut Dec::new(&side),
        )
        .unwrap() else {
            panic!("list required")
        };
        assert!(back.rows[0].initial_selection);
        assert!(back.rows[1].filter_fallback);
        let old = r#"{"List":[{"cells":[],"kind":"row","target":null,"checked":null}]}"#;
        let View::List(back) =
            view_from_wire(serde_json::from_str(old).unwrap(), &mut Dec::new(&side)).unwrap()
        else {
            panic!("list required")
        };
        assert!(!back.rows[0].initial_selection && !back.rows[0].filter_fallback);
        let mut view = View::List(back);
        let json =
            serde_json::to_string(&view_to_wire(&mut view, &mut Enc::new(&side)).unwrap()).unwrap();
        assert!(!json.contains("initial_selection") && !json.contains("filter_fallback"));
    }
}
