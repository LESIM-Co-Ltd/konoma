//! The routing contract is pinned to 7f02d00, before graph-specific list motion.

use super::*;
use crossterm::event::KeyCode;
use konoma_plugin::prelude::*;
use konoma_plugin::view::{DiffHunk, DiffKind, DiffLine, DiffView, ListRow, ListView};

struct LeftScreens;
impl Plugin for LeftScreens {
    fn manifest(&self) -> Manifest {
        let project = ["changes", "log", "worktrees", "items", "detail"]
            .into_iter()
            .fold(
                ProjectDecl::new().marker(DetectionMarker::new("*.left-project", MarkerKind::File)),
                |project, screen| project.screen(ProjectScreen::new(screen, screen)),
            );
        Manifest::new("project-left", "1")
            .project(project)
            .shows(ViewKind::List)
            .shows(ViewKind::Diff)
    }
    fn open(&self, _: Input, _: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        Ok(Box::new(Self))
    }
}
impl Session for LeftScreens {
    fn view(&mut self, req: &ViewReq, _: &ViewCx) -> Result<View, Failure> {
        Ok(if req.project.as_ref().unwrap().screen == "detail" {
            View::Diff(DiffView::new(vec![DiffHunk::new(vec![DiffLine {
                kind: DiffKind::Context,
                old_no: Some(1),
                new_no: Some(1),
                text: "wide ".repeat(100),
            }])]))
        } else {
            View::List(ListView {
                rows: vec![ListRow::new("item", vec![Line::raw("row")])],
                ..Default::default()
            })
        })
    }
}

fn non_graph_left(screen: &str) {
    let dir = sandbox("project_left_base");
    std::fs::write(dir.join("fixture.left-project"), "fixture").unwrap();
    let token = test_plugins::register("project_left_base", Arc::new(LeftScreens));
    let mut cfg = test_plugins::marker_config(&token);
    cfg.ui.diff_view = "source".into();
    let mut sim = Sim::with_config_sized(&canon(&dir), cfg, 100, 16);
    sim.app.tab.projects.wait_for_result();
    sim.app.poll_projects();
    sim.key('p');
    drain(&mut sim);
    for candidate in ["changes", "log", "worktrees", "items", "detail"] {
        if candidate != screen {
            sim.press(KeyCode::Tab, KeyModifiers::NONE);
            drain(&mut sim);
            continue;
        }
        assert_eq!(sim.app.project_frame().unwrap().key.screen, screen);
        for key in [KeyCode::Char('h'), KeyCode::Left] {
            if screen == "detail" {
                sim.key('l');
                sim.key('l');
                drain(&mut sim);
                let before = sim.app.project_frame().unwrap().hscroll;
                assert!(before >= 4);
                sim.press(key, KeyModifiers::NONE);
                drain(&mut sim);
                // At 7f02d00 detail's Left is a four-cell pan, without leaving the screen.
                assert_eq!(sim.app.project_frame().unwrap().hscroll, before - 4);
            } else {
                // At 7f02d00 the project handler falls through; Canvas's default is a no-op.
                assert!(
                    !sim.app
                        .handle_project_key(KeyEvent::new(key, KeyModifiers::NONE)),
                    "{screen} {key:?}"
                );
                sim.press(key, KeyModifiers::NONE);
                drain(&mut sim);
                assert_eq!(sim.app.project_frame().unwrap().cursor, 0);
            }
            assert_eq!(sim.app.project_frame().unwrap().key.screen, screen);
        }
        return;
    }
    panic!("unknown screen {screen}");
}

macro_rules! left_baseline {
    ($name:ident, $screen:literal) => {
        #[test]
        fn $name() {
            non_graph_left($screen);
        }
    };
}
left_baseline!(e2e_project_changes_left_preserves_7f02d00, "changes");
left_baseline!(e2e_project_log_left_preserves_7f02d00, "log");
left_baseline!(e2e_project_worktrees_left_preserves_7f02d00, "worktrees");
left_baseline!(e2e_project_generic_list_left_preserves_7f02d00, "items");
left_baseline!(e2e_project_detail_left_preserves_7f02d00, "detail");

#[test]
fn e2e_project_all_projects_chooser_left_preserves_7f02d00() {
    let mut f = fixture("project_chooser_left", false, 2, None, None, None);
    f.sim.key('p');
    for key in [KeyCode::Char('h'), KeyCode::Left] {
        f.sim.press(key, KeyModifiers::NONE);
        assert!(matches!(
            f.sim.app.project_dialog,
            Some(crate::app::project_screens::ProjectDialog::Projects { selected: 0, .. })
        ));
        assert!(!f.sim.app.project_active());
    }
}
