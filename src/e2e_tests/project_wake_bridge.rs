//! Freeze complete project-body cells through the host's real detection and UI adapters.
use super::*;
use serde_json::{json, Value};

#[path = "../../crates/konoma-plugin/tests/app/project/wake_fixture.rs"]
mod plugin;

fn cells(sim: &Sim) -> Value {
    let rect = ratatui::layout::Rect::new(0, 1, 54, 12);
    let buffer = sim.term.backend().buffer();
    let mut runs: Vec<Value> = Vec::new();
    let mut count = 0;
    let mut nonempty = 0;
    for y in rect.y..rect.bottom() {
        for x in rect.x..rect.right() {
            let cell = &buffer[(x, y)];
            let cell = json!([
                cell.symbol(),
                format!("{:?}", cell.fg),
                format!("{:?}", cell.bg),
                cell.modifier.bits()
            ]);
            nonempty += usize::from(!cell[0].as_str().unwrap().trim().is_empty());
            if let Some(last) = runs.last_mut().filter(|last| last[1] == cell) {
                last[0] = json!(last[0].as_u64().unwrap() + 1);
            } else {
                runs.push(json!([1, cell]));
            }
            count += 1;
        }
    }
    assert_eq!(count, 648);
    assert!(nonempty > 0);
    json!({"rect": [0, 1, 54, 12], "count":count, "runs":runs})
}

#[test]
fn project_idle_watch_matches_frozen_host_cells() {
    let dir = sandbox("project_wake_bridge");
    std::fs::write(dir.join("project.marker"), "").unwrap();
    let root = canon(&dir);
    let log = Arc::new(Mutex::new(plugin::Log {
        paths: vec![root.join("first/index")],
        ..Default::default()
    }));
    let marker = test_plugins::register(
        "project_wake_bridge",
        Arc::new(plugin::WatchProject(log.clone())),
    );
    let mut cfg = test_plugins::marker_config(&marker);
    cfg.ui.lang = "en".into();
    cfg.ui.tabbar = "hidden".into();
    cfg.external.git = false;
    let mut sim = Sim::with_config_sized(&root, cfg, 54, 14);
    sim.app.tab.projects.wait_for_result();
    sim.app.poll_projects();
    drain(&mut sim);
    sim.key('p');
    drain(&mut sim);
    for key in plugin::CACHE_KEYS {
        sim.key(key);
        drain(&mut sim);
    }
    let before = cells(&sim);
    let views = log.lock().unwrap().views.len();
    log.lock().unwrap().changed = true;
    sim.app
        .tab
        .projects
        .inject_event_with_pending_watch_update(Ok(
            notify::Event::new(notify::EventKind::Any).add_path(root.join("first/index"))
        ));
    assert!(sim.app.poll_projects());
    drain(&mut sim);
    assert_eq!(log.lock().unwrap().views.len(), views + 1);
    assert_eq!(log.lock().unwrap().views.last().unwrap(), "items");
    let actual = json!([before, cells(&sim)]);
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("snapshots/project_wake.json");
    let expected: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    assert_eq!(actual, expected);
}
