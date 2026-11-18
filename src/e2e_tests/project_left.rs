//! The list-screen key contract: `h` and Left go back, detail screens pan four cells.

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

/// Opens the fixture project on `screen`; `[keys]` overrides come from `keys`.
fn left_sim(screen: &str, keys: &[(&str, &str)]) -> Sim {
    let dir = sandbox("project_left_base");
    std::fs::write(dir.join("fixture.left-project"), "fixture").unwrap();
    let token = test_plugins::register("project_left_base", Arc::new(LeftScreens));
    let mut cfg = test_plugins::marker_config(&token);
    cfg.ui.diff_view = "source".into();
    if !keys.is_empty() {
        cfg.keys.surfaces.insert(
            "text".into(),
            keys.iter()
                .map(|(key, action)| ((*key).to_string(), (*action).to_string()))
                .collect(),
        );
    }
    let mut sim = Sim::with_config_sized(&canon(&dir), cfg, 100, 16);
    sim.app.tab.projects.wait_for_result();
    sim.app.poll_projects();
    sim.key('p');
    drain(&mut sim);
    for candidate in ["changes", "log", "worktrees", "items", "detail"] {
        if candidate == screen {
            assert_eq!(sim.app.project_frame().unwrap().key.screen, screen);
            return sim;
        }
        sim.press(KeyCode::Tab, KeyModifiers::NONE);
        drain(&mut sim);
    }
    panic!("unknown screen {screen}");
}

/// A list screen: `Left` (default `h`, and the arrow) goes back out of the project like `q`.
fn list_left_goes_back(screen: &str) {
    for key in [KeyCode::Char('h'), KeyCode::Left] {
        let mut sim = left_sim(screen, &[]);
        assert!(
            sim.app
                .handle_project_key(KeyEvent::new(key, KeyModifiers::NONE)),
            "{screen} {key:?}"
        );
        drain(&mut sim);
        assert!(
            !sim.app.project_active(),
            "{screen} {key:?} leaves the project"
        );
    }
    // `q` is the reference: same end state.
    let mut sim = left_sim(screen, &[]);
    sim.key('q');
    drain(&mut sim);
    assert!(!sim.app.project_active(), "{screen} q leaves the project");
}

macro_rules! left_back {
    ($name:ident, $screen:literal) => {
        #[test]
        fn $name() {
            list_left_goes_back($screen);
        }
    };
}
left_back!(e2e_project_changes_left_goes_back, "changes");
left_back!(e2e_project_log_left_goes_back, "log");
left_back!(e2e_project_worktrees_left_goes_back, "worktrees");
left_back!(e2e_project_generic_list_left_goes_back, "items");

fn left_footer(sim: &Sim) -> String {
    crate::ui::status::footer_spans(&sim.app, 100)
        .iter()
        .map(|s| s.content.as_ref())
        .collect()
}

/// A generic (non-VCS) list screen names the keys that go back in its footer: `Left` goes back
/// like `q`, so `h/q:back` shows; a rebound `Left` moves the token, a cleared one drops it, and a
/// detail screen (where `h` pans) never shows `h` as a way back.
#[test]
fn e2e_project_generic_list_footer_names_the_left_key() {
    let sim = left_sim("items", &[]);
    assert!(
        left_footer(&sim).contains("h/q:back"),
        "{}",
        left_footer(&sim)
    );
    let sim = left_sim("items", &[("h", "noop"), ("Z", "navigate:left")]);
    let footer = left_footer(&sim);
    assert!(
        footer.contains("Z/q:back") && !footer.contains("h/q"),
        "{footer}"
    );
    let sim = left_sim("items", &[("h", "noop")]);
    let footer = left_footer(&sim);
    assert!(
        footer.contains("q:back") && !footer.contains("h/q"),
        "{footer}"
    );
    let sim = left_sim("detail", &[]);
    let footer = left_footer(&sim);
    assert!(!footer.contains("h/q"), "{footer}");
}

/// A rebound `Left` moves the key: the new key goes back, the old `h` no longer does. A cleared
/// `Left` (`h` and the arrow unbound) does nothing.
#[test]
fn e2e_project_left_follows_the_keymap() {
    for screen in ["changes", "log", "worktrees", "items"] {
        let mut sim = left_sim(screen, &[("h", "noop"), ("Z", "navigate:left")]);
        sim.key('h');
        drain(&mut sim);
        assert!(
            sim.app.project_active(),
            "{screen}: unbound h left the project"
        );
        sim.key('Z');
        drain(&mut sim);
        assert!(
            !sim.app.project_active(),
            "{screen}: rebound Left key stays"
        );
        let mut sim = left_sim(screen, &[("h", "noop")]);
        sim.press(KeyCode::Char('h'), KeyModifiers::NONE);
        drain(&mut sim);
        assert!(sim.app.project_active(), "{screen}: cleared h acts");
        assert_eq!(sim.app.project_frame().unwrap().key.screen, screen);
        // The arrow is a fixed key (never in the keymap), so it keeps going back.
        sim.press(KeyCode::Left, KeyModifiers::NONE);
        drain(&mut sim);
        assert!(
            !sim.app.project_active(),
            "{screen}: the Left arrow is fixed"
        );
    }
}

#[test]
fn e2e_project_detail_left_pans_four_cells() {
    for key in [KeyCode::Char('h'), KeyCode::Left] {
        let mut sim = left_sim("detail", &[]);
        sim.key('l');
        sim.key('l');
        drain(&mut sim);
        let before = sim.app.project_frame().unwrap().hscroll;
        assert!(before >= 4);
        sim.press(key, KeyModifiers::NONE);
        drain(&mut sim);
        // Detail's Left is a four-cell pan, without leaving the screen.
        assert_eq!(sim.app.project_frame().unwrap().hscroll, before - 4);
        assert_eq!(sim.app.project_frame().unwrap().key.screen, "detail");
    }
}

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
