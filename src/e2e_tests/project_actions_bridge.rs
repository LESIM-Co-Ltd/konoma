//! Independent cell references captured through konoma's ordinary key and drawing adapters.
use super::*;
use ratatui::layout::Rect;
use serde_json::{json, Value};

fn key_names(value: &Value) -> Vec<&str> {
    value
        .as_array()
        .unwrap()
        .iter()
        .map(|key| key.as_str().unwrap())
        .collect()
}

fn raw_key(f: &mut Fixture, name: &str) {
    let (name, mods) = name
        .strip_prefix("ctrl-")
        .map(|name| (name, KeyModifiers::CONTROL))
        .unwrap_or((name, KeyModifiers::NONE));
    let code = match name {
        "Enter" => KeyCode::Enter,
        "Tab" => KeyCode::Tab,
        _ if name.starts_with('F') => KeyCode::F(name[1..].parse().unwrap()),
        _ => KeyCode::Char(name.chars().next().unwrap()),
    };
    f.sim.press(code, mods);
}

fn cells(f: &mut Fixture) -> Value {
    let area = Rect::new(0, 1, 54, 12);
    let finished = f.sim.term.backend().buffer().clone();
    let buffer = &finished;
    let mut runs: Vec<Value> = Vec::new();
    let mut compared = 0;
    let mut nonempty = 0;
    for y in area.y..area.bottom() {
        for x in area.x..area.right() {
            let cell = &buffer[(x, y)];
            compared += 1;
            nonempty += usize::from(!cell.symbol().trim().is_empty());
            let value = json!([
                cell.symbol(),
                format!("{:?}", cell.fg),
                format!("{:?}", cell.bg),
                cell.modifier.bits()
            ]);
            if let Some(last) = runs.last_mut().filter(|last| last[1] == value) {
                last[0] = json!(last[0].as_u64().unwrap() + 1);
            } else {
                runs.push(json!([1, value]));
            }
        }
    }
    assert_eq!(compared, 648);
    assert!(nonempty > 0);
    let frame = super::frozen_frame::frame_info(&mut f.sim, area, &finished);
    json!({"rect":[0,1,54,12], "count":compared, "runs":runs, "frame":frame})
}

#[test]
fn project_prompt_sequence_matches_frozen_host_output() {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("snapshots/project_actions.json");
    let expected: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(
        expected["keys"],
        json!(["F5", "y", "追", "加", "Enter", "j", "Enter"])
    );
    let mut f = fixture("project_actions_bridge", false, 1, None, None, None);
    f.sim.resize(54, 14);
    open(&mut f);
    let mut stages = Vec::new();
    for key in key_names(&expected["keys"]) {
        raw_key(&mut f, key);
        drain(&mut f.sim);
        stages.push(cells(&mut f));
    }
    let (action, context) = last_act(&f);
    assert_eq!(action, "answers");
    assert_eq!(context.confirmation.as_deref(), Some("accepted"));
    assert_eq!(context.text.as_deref(), Some("seed追加"));
    assert_eq!(context.choice.as_deref(), Some("second"));
    assert_eq!(
        context.selected,
        Some(ProjectRow::new("item", "opaque-alpha"))
    );
    let actual = json!({"keys":expected["keys"], "stages":stages});
    super::frozen_frame::frozen_eq(&actual, &expected, "project_actions");
}

fn extra_reference() -> Value {
    serde_json::from_str(
        &std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("snapshots/project_actions_extra.json"),
        )
        .unwrap(),
    )
    .unwrap()
}

#[test]
fn project_held_view_and_retry_match_frozen_host_output() {
    let expected = extra_reference();
    let before_submit = key_names(&expected["keys"]["held_prompt"]);
    assert_eq!(before_submit, ["F5", "y", "追", "加", "Enter", "j"]);
    assert_eq!(
        expected["keys"]["held_raw"],
        json!(["F5", "y", "追", "加", "Enter", "j", "Enter", "j"])
    );
    assert_eq!(
        expected["keys"]["retry_confirmed"],
        json!(["ctrl-F6", "y", "追", "加", "Enter"])
    );
    assert_eq!(
        expected["keys"]["retry_refresh"],
        json!(["ctrl-F7", "追", "加", "Enter"])
    );
    let mut held_cells = Vec::new();
    for change in 0..4 {
        let mut f = fixture(
            &format!("project_held_bridge_{change}"),
            false,
            1,
            None,
            None,
            None,
        );
        f.sim.resize(54, 14);
        open(&mut f);
        if matches!(change, 1 | 2) {
            for name in &before_submit {
                raw_key(&mut f, name);
            }
        }
        let hold = ViewHold::new("items");
        f.log.lock().unwrap().view_hold = Some(hold.clone());
        if change == 2 {
            f.log.lock().unwrap().omit_row = Some("opaque-alpha".into());
        }
        f.sim.resize(55, 14);
        f.sim.app.wait_project_view_request("items");
        hold.entered.wait();
        if matches!(change, 0 | 3) {
            if change == 3 {
                raw_key(&mut f, "F5");
                f.sim.keys(&format!("y{}", "a".repeat(90)));
                f.sim.enter();
                f.sim.key('j');
            } else {
                for name in &before_submit {
                    raw_key(&mut f, name);
                }
            }
        }
        f.sim.enter();
        if matches!(change, 0 | 3) {
            f.sim.key('j');
        }
        assert!(f.log.lock().unwrap().acts.is_empty());
        if change == 1 {
            f.log.lock().unwrap().disabled.push("answers".into());
        }
        hold.release.open();
        drain(&mut f.sim);
        f.sim.resize(54, 14);
        drain(&mut f.sim);
        assert_eq!(
            f.log.lock().unwrap().acts.len(),
            usize::from(matches!(change, 0 | 3))
        );
        if change == 3 {
            assert_eq!(
                cells(&mut f),
                held_cells[0],
                "a replay longer than one slice preserves the same final frame"
            );
        } else {
            held_cells.push(cells(&mut f));
        }
    }
    let mut retry_cells = Vec::new();
    for mode in 0..4 {
        let hold = Arc::new(Latch::default());
        let started = Arc::new(Latch::default());
        let mut f = fixture_with_log(
            &format!("project_retry_bridge_{mode}"),
            false,
            1,
            None,
            (mode == 3).then(|| hold.clone()),
            (mode == 3).then(|| started.clone()),
            ProjectLog {
                act_failures: usize::from(mode == 0 || mode == 3),
                view_failure_after_act: mode == 1,
                watch_failure_after_act: mode == 2,
                ..Default::default()
            },
        );
        f.sim.resize(54, 14);
        open(&mut f);
        let sequence = if mode == 0 || mode == 3 {
            "retry_confirmed"
        } else {
            "retry_refresh"
        };
        for name in key_names(&expected["keys"][sequence]) {
            raw_key(&mut f, name);
        }
        if mode == 3 {
            started.wait();
            f.sim.press(KeyCode::Tab, KeyModifiers::NONE);
            hold.open();
        }
        drain(&mut f.sim);
        assert_eq!(f.log.lock().unwrap().acts.len(), 1);
        assert_eq!(f.sim.app.project_dialog.is_some(), mode == 0);
        retry_cells.push(cells(&mut f));
    }
    let actual = json!({"keys":expected["keys"], "held":held_cells, "retry":retry_cells});
    super::frozen_frame::frozen_eq(&actual, &expected, "project_actions_extra");
}

#[test]
fn project_outcome_refresh_matches_frozen_host_output() {
    use konoma_plugin::{view::ListRow, Color, Line, Span, Style};
    // OpenFile/OpenTab/OpenDiff/SwitchRoot have host-specific executors. Their standalone
    // notice policy preserves this same refreshed project frame, captured before any detour.
    let mut f = fixture(
        "project_outcome_refresh_bridge",
        false,
        1,
        Some(Outcome::Unhandled),
        None,
        None,
    );
    f.sim.resize(54, 14);
    open(&mut f);
    f.log.lock().unwrap().list_rows = Some(vec![
        ListRow::new(
            "item",
            vec![Line::from(vec![
                Span::styled("changed", Style::new().fg(Color::Red).bold()),
                Span::styled(" styled", Style::new().fg(Color::Green)),
            ])],
        )
        .value("opaque-alpha")
        .checked(true)
        .marked(true),
        ListRow::new("other", vec![Line::raw("beta")])
            .value("opaque-beta")
            .checked(false),
        ListRow::new("item", vec![Line::raw("gamma")]).value("opaque-gamma"),
    ]);
    let expected: Value = serde_json::from_str(
        &std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("snapshots/project_outcome_refresh.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(expected["keys"], json!(["ctrl-F7", "Enter"]));
    assert_eq!(
        expected["host_only"],
        json!(["OpenFile", "OpenTab", "OpenDiff", "SwitchRoot"])
    );
    for name in key_names(&expected["keys"]) {
        raw_key(&mut f, name);
    }
    drain(&mut f.sim);
    assert_eq!(last_act(&f).0, "retry-refresh");
    let mut actual = cells(&mut f);
    actual["keys"] = expected["keys"].clone();
    actual["host_only"] = expected["host_only"].clone();
    super::frozen_frame::frozen_eq(&actual, &expected, "project_outcome_refresh");
}
