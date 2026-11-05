#[cfg(feature = "proc")]
mod wire {
    use konoma_plugin::proc::{frame, wire::*};
    use konoma_plugin::view::{DiffHunk, DiffKind, DiffLine, DiffView};
    use konoma_plugin::*;

    #[test]
    fn project_detail_wire_pushed_round_trip_and_old_json() {
        let screen = ProjectScreen::new("detail", "Detail").pushed();
        let bytes = frame::encode(&project_screen_to_wire(&screen)).unwrap();
        assert_eq!(
            project_screen_from_wire(frame::decode(&bytes).unwrap()),
            screen
        );
        let old =
            r#"{"id":"detail","title":{"en":"Detail","others":[]},"key":null,"row_kinds":[]}"#;
        let old: WProjectScreen = serde_json::from_str(old).unwrap();
        assert!(!project_screen_from_wire(old).pushed);
    }

    #[test]
    fn project_detail_wire_diff_round_trip_and_old_json() {
        let dir = konoma_test_support::unique_tmp("detail_wire");
        std::fs::create_dir_all(&dir).unwrap();
        let side = Side::new(&dir, 'x');
        let expected = DiffView::new(vec![DiffHunk::new(vec![DiffLine {
            kind: DiffKind::Added,
            old_no: None,
            new_no: Some(1),
            text: "日本語".into(),
        }])
        .path("main.rs")])
        .header(vec![Line::from(vec![Span::styled(
            "commit abc",
            Style::new().fg(Color::Yellow),
        )])]);
        let mut view = View::Diff(expected.clone());
        let wire = view_to_wire(&mut view, &mut Enc::new(&side)).unwrap();
        let bytes = frame::encode(&wire).unwrap();
        let View::Diff(back) =
            view_from_wire(frame::decode(&bytes).unwrap(), &mut Dec::new(&side)).unwrap()
        else {
            panic!("diff required")
        };
        assert_eq!(back, expected);
        for json in [
            r#"{"Diff":[[{"kind":"Context","old_no":1,"new_no":1,"text":"old"}]]}"#,
            r#"{"Diff":{"hunks":[{"lines":[{"kind":"Context","old_no":1,"new_no":1,"text":"old"}]}]}}"#,
        ] {
            let wire: WView = serde_json::from_str(json).unwrap();
            let View::Diff(old) = view_from_wire(wire, &mut Dec::new(&side)).unwrap() else {
                panic!("diff required")
            };
            assert!(old.header.is_empty());
            assert_eq!(old.hunks[0].path, None);
            assert_eq!(old.hunks[0].lines[0].text, "old");
        }
    }
}
