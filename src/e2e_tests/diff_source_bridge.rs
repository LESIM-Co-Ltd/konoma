//! Literal full-screen references captured using konoma's real project and file-diff adapters.
use super::*;
use serde_json::{json, Value};

#[path = "../../crates/konoma-plugin/tests/app/diff_screen/source_fixture.rs"]
mod fixture;

fn cells(sim: &Sim) -> Value {
    let rect = ratatui::layout::Rect::new(0, 0, 54, 14);
    let buffer = sim.term.backend().buffer();
    assert_eq!(buffer.area, rect);
    let mut runs: Vec<Value> = Vec::new();
    let mut count = 0;
    let mut nonempty = 0;
    for c in &buffer.content {
        let value = json!([
            c.symbol(),
            format!("{:?}", c.fg),
            format!("{:?}", c.bg),
            c.modifier.bits()
        ]);
        if let Some(last) = runs.last_mut().filter(|r| r[1] == value) {
            last[0] = json!(last[0].as_u64().unwrap() + 1);
        } else {
            runs.push(json!([1, value]));
        }
        count += 1;
        nonempty += usize::from(!c.symbol().trim().is_empty());
    }
    assert_eq!(count, 756);
    assert!(nonempty > 0);
    json!({"rect":[0,0,54,14], "count":count, "runs":runs})
}

#[test]
fn source_diff_keys_match_frozen_host_output() {
    use crate::plugins::test_plugins;
    let dir = sandbox("source_diff_bridge");
    let root = dir.join("reference");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("project.marker"), "").unwrap();
    for name in ["a.txt", "b.txt", "c.txt"] {
        std::fs::write(root.join(name), "body\n").unwrap();
    }
    seed_repo(&root);
    std::fs::write(root.join("a.rs"), "fn a() {}\n").unwrap();
    std::fs::remove_file(root.join("new.txt")).unwrap();
    for name in ["a.txt", "b.txt", "c.txt"] {
        std::fs::write(root.join(name), "changed\n").unwrap();
    }
    let marker = test_plugins::register("source_diff_bridge", Arc::new(fixture::SourceProject));
    let mut cfg = test_plugins::marker_config(&marker);
    cfg.ui.lang = "en".into();
    cfg.ui.diff_view = "source".into();
    cfg.ui.tabbar = "hidden".into();
    cfg.git.diff = "unified".into();
    // Capability differences are explicit operation IDs, never excluded screen cells.
    let unavailable = [
        "cycle_diff_view",
        "git_diff_discard",
        "request_edit",
        "open_external",
        "open_git_view",
        "git_launch_tool",
        "toggle_follow",
    ];
    for (key, action) in [
        ("R", "cycle_diff_view"),
        ("x", "git_diff_discard"),
        ("e", "request_edit"),
        ("o", "open_external"),
        ("g", "open_git_view"),
        ("!", "git_launch_tool"),
        ("F", "toggle_follow"),
    ] {
        assert!(unavailable.contains(&action));
        cfg.keys
            .surfaces
            .entry("preview_git_diff".into())
            .or_default()
            .insert(key.into(), "noop".into());
    }
    let mut actual = Vec::new();
    for keys in [
        fixture::SOURCE_KEYS,
        fixture::FILE_KEYS,
        fixture::REFRESH_KEYS,
        fixture::MOTION_KEYS,
    ] {
        let mut sim = Sim::with_config_sized(&canon(&root), cfg.clone(), 54, 14);
        sim.app.tab.projects.wait_for_result();
        sim.app.poll_projects();
        drain(&mut sim);
        sim.key('p');
        drain(&mut sim);
        for _ in 0..12 {
            sim.key('j');
        }
        let mut stages = vec![cells(&sim)];
        for key in keys {
            let (name, mods) = key
                .strip_prefix("ctrl-")
                .map(|s| (s, KeyModifiers::CONTROL))
                .unwrap_or((key, KeyModifiers::NONE));
            let code = match name {
                "Enter" => KeyCode::Enter,
                "Down" => KeyCode::Down,
                "PageDown" => KeyCode::PageDown,
                "PageUp" => KeyCode::PageUp,
                "End" => KeyCode::End,
                "Home" => KeyCode::Home,
                "Right" => KeyCode::Right,
                "Left" => KeyCode::Left,
                "Esc" => KeyCode::Esc,
                _ => KeyCode::Char(name.chars().next().unwrap()),
            };
            sim.press(code, mods);
            drain(&mut sim);
            if sim.app.is_git_diff_preview() {
                sim.app.seed_source_diff_for_test(fixture::raw_diff());
                sim.draw();
            }
            stages.push(cells(&sim));
        }
        if keys == fixture::REFRESH_KEYS {
            assert!(sim.app.is_git_diff_preview());
            actual.push(json!({"keys":keys,"stages":stages}));
            continue;
        }
        assert!(sim.app.project_active(), "q restores the project");
        let frame = sim
            .app
            .tab
            .project_navigation
            .as_ref()
            .unwrap()
            .frame()
            .unwrap();
        assert_eq!(frame.cursor, 12);
        actual.push(json!({"keys":keys,"stages":stages}));
    }
    let actual = json!({"host_only":unavailable,"sequences":actual});
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("snapshots/diff_source_screen.json");
    let expected: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    assert_eq!(actual, expected);
}
