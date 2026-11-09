//! Frozen standalone-body references collected through konoma's real host adapter.
use super::*;
use ratatui::layout::Rect;
use serde_json::{json, Value};

fn stage(f: &Fixture) -> Value {
    let area = Rect::new(0, 1, 54, 12);
    let buffer = f.sim.term.backend().buffer();
    let mut runs: Vec<Value> = Vec::new();
    let mut nonempty = 0;
    for y in area.y..area.bottom() {
        for x in area.x..area.right() {
            let cell = &buffer[(x, y)];
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
    assert!(nonempty > 0);
    let nav = f.sim.app.tab.project_navigation.as_ref().unwrap();
    let frame = nav.frame().unwrap();
    json!({"rect":[0,1,54,12], "count":648, "screen":frame.key.screen,
        "selection":frame.key.selection, "cursor":frame.cursor, "query":frame.query,
        "marked":frame.marked, "runs":runs})
}

#[test]
fn project_navigation_body_matches_frozen_host_output() {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("snapshots/project_navigation.json");
    let expected: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let keys = expected["keys"].as_array().unwrap();
    assert_eq!(keys, &vec!["Tab", "BackTab", "b", "i", "Enter", "q"]);
    let mut f = fixture("project_navigation_bridge", false, 1, None, None, None);
    f.sim.resize(54, 14);
    open(&mut f);
    let mut stages = vec![stage(&f)];
    for key in keys {
        let key = key.as_str().unwrap();
        let code = match key {
            "Tab" => KeyCode::Tab,
            "BackTab" => KeyCode::BackTab,
            "Enter" => KeyCode::Enter,
            _ => KeyCode::Char(key.chars().next().unwrap()),
        };
        press(&mut f, code);
        stages.push(stage(&f));
    }
    press(&mut f, KeyCode::Char('d'));
    let diff = stage(&f);
    press(&mut f, KeyCode::Char('i'));
    let nav = f.sim.app.tab.project_navigation.as_ref().unwrap();
    let project = nav.project.clone();
    let key = nav.frame().unwrap().key.clone();
    let runtime = f.sim.app.project_runtimes.get_mut(&project).unwrap();
    let konoma_plugin::host::project::ProjectContent::List(list) = runtime.views[&key].as_ref()
    else {
        panic!("list")
    };
    let mut list = list.clone();
    list.bottom = Some(konoma_plugin::view::ListStrip::new(
        vec![konoma_plugin::Line::raw("legend")],
        6,
    ));
    runtime.views.insert(
        key,
        Arc::new(konoma_plugin::host::project::ProjectContent::List(list)),
    );
    f.sim.draw();
    let chips = stage(&f);
    let actual = json!({"keys":keys,"stages":stages,"diff":diff,"chips":chips});
    assert_eq!(actual, expected);
}
