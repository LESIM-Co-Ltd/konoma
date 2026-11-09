//! Launch references from konoma's real dispatcher; no editor, opener or tool is started.
use super::*;
use crossterm::event::KeyCode;
use konoma_plugin::prelude::*;
use konoma_plugin::{Bind, LaunchMode, Launcher, Verb, VerbDecl};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

struct LaunchProject;
impl Plugin for LaunchProject {
    fn manifest(&self) -> Manifest {
        Manifest::new("launch-project-bridge", "1")
            .bind(Bind::files(["*.launch"]))
            .verb(
                VerbDecl::new(Verb::Edit)
                    .launcher(Launcher::Command(vec![
                        "row-editor".into(),
                        "--line={line}".into(),
                    ]))
                    .mode(LaunchMode::Terminal),
            )
            .verb(
                VerbDecl::new(Verb::Open)
                    .launcher(Launcher::Command(vec!["row-opener".into()]))
                    .mode(LaunchMode::Gui),
            )
            .project(
                ProjectDecl::new()
                    .marker(DetectionMarker::new("project.marker", MarkerKind::File))
                    .screen(ProjectScreen::new("items", "Items").row_kind("file")),
            )
            .action(
                Act::new("diff")
                    .key("Enter")
                    .project_context(ActContext::screen("items", ["file"])),
            )
    }
    fn open(&self, input: Input, _: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        let Input::Project { root, .. } = input else {
            unreachable!()
        };
        Ok(Box::new(Rows(root)))
    }
}
struct Rows(std::path::PathBuf);
impl Session for Rows {
    fn view(&mut self, _: &ViewReq, _: &ViewCx) -> Result<View, Failure> {
        Ok(View::List(konoma_plugin::view::ListView {
            rows: ["first.launch", "nested/second.launch"]
                .into_iter()
                .map(|name| {
                    konoma_plugin::view::ListRow::new("file", vec![konoma_plugin::Line::raw(name)])
                        .value(name)
                        .target(self.0.join(name))
                })
                .collect(),
            ..Default::default()
        }))
    }
    fn act(&mut self, _: &str, cx: &ActCx) -> Result<Outcome, Failure> {
        Ok(Outcome::OpenDiff {
            path: self.0.join(
                &cx.project
                    .as_ref()
                    .unwrap()
                    .selected
                    .as_ref()
                    .unwrap()
                    .value,
            ),
        })
    }
}
fn raw_diff() -> Vec<crate::git::DiffLine> {
    use crate::git::{DiffLine, DiffLineKind};
    (1..=36)
        .flat_map(|n| {
            let changed = [1, 8, 19, 32].contains(&n);
            let line = |kind, old_no, new_no| DiffLine {
                kind,
                old_no,
                new_no,
                text: format!("line {n}"),
            };
            changed
                .then(|| line(DiffLineKind::Removed, Some(n), None))
                .into_iter()
                .chain([line(
                    if changed {
                        DiffLineKind::Added
                    } else {
                        DiffLineKind::Context
                    },
                    (!changed).then_some(n),
                    Some(n),
                )])
        })
        .collect()
}
fn drain(sim: &mut Sim) {
    for _ in 0..32 {
        sim.app.poll_projects();
        sim.draw();
        if !sim.app.project_jobs_pending() {
            return;
        }
        sim.app.wait_project_job();
    }
    panic!("project did not become idle");
}

#[test]
fn project_launch_targets_lines_and_cwds_match_the_host_path() {
    let expected: Value = serde_json::from_str(
        &std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("snapshots/project_launch.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let dir = sandbox("project_launch_bridge");
    std::fs::create_dir_all(dir.join("nested")).unwrap();
    std::fs::write(dir.join("project.marker"), "").unwrap();
    for name in ["first.launch", "nested/second.launch"] {
        std::fs::write(dir.join(name), "body\n").unwrap();
    }
    let root = canon(&dir);
    assert!(std::process::Command::new("git")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_COUNT", "0")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .args(["-c", "init.defaultBranch=main", "init", "-q"])
        .current_dir(&root)
        .status()
        .unwrap()
        .success());
    let marker =
        crate::plugins::test_plugins::register("project-launch-bridge", Arc::new(LaunchProject));
    let mut cfg = crate::plugins::test_plugins::marker_config(&marker);
    cfg.ui.lang = "en".into();
    cfg.ui.diff_view = "source".into();
    cfg.git.diff = "unified".into();
    cfg.git.tool = "git-tool --git".into();
    let mut sim = Sim::with_config_sized(&root, cfg, 100, 14);
    sim.app.ensure_git_status_now();
    sim.app.tab.projects.wait_for_result();
    sim.app.poll_projects();
    drain(&mut sim);
    sim.key('p');
    drain(&mut sim);
    assert!(sim.app.project_active());
    let ran = Arc::new(Mutex::new(vec![]));
    let gui = ran.clone();
    let gui_root = root.clone();
    sim.app.set_office_runner(Arc::new(move |a| {
        gui.lock().unwrap().push(record(a, None, &gui_root));
        Ok(Some(0))
    }));
    for key in expected["keys"].as_array().unwrap() {
        if key == "Enter" {
            sim.press(KeyCode::Enter, KeyModifiers::NONE);
        } else {
            sim.key(key.as_str().unwrap().chars().next().unwrap());
        }
        drain(&mut sim);
        if sim.app.is_git_diff_preview() {
            sim.app.seed_source_diff_for_test(raw_diff());
            sim.draw();
        }
        if let Some((attempts, cwd)) = sim.app.take_pending_terminal() {
            assert_eq!(attempts.len(), 1);
            ran.lock()
                .unwrap()
                .push(record(&attempts[0], Some(&cwd), &root));
        }
        assert!(
            sim.app.take_pending_edit().is_none(),
            "the selected file's declared verb wins"
        );
        if sim.app.take_launch_git_tool() {
            let plan = crate::git_tool_plan(&sim.app).unwrap();
            ran.lock()
                .unwrap()
                .push(record(&plan.attempt, Some(&plan.cwd), &root));
        }
    }
    assert_eq!(
        *ran.lock().unwrap(),
        expected["launches"].as_array().unwrap().clone()
    );
}
fn record(
    a: &konoma_plugin::launch::Attempt,
    cwd: Option<&std::path::Path>,
    root: &std::path::Path,
) -> Value {
    let root = root.to_string_lossy();
    json!({"program":a.prog,"args":a.args.iter().map(|s|s.replace(root.as_ref(), "$ROOT")).collect::<Vec<_>>(),
        "cwd":cwd.map(|p|p.to_string_lossy().replace(root.as_ref(), "$ROOT"))})
}
