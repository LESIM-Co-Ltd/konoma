//! The project chooser while detection changes under it: a candidate that is no longer detected is
//! dropped, an empty chooser ends the program as a start without a project does, and choosing a
//! candidate that went away is refused instead of leaving the loop with no screen.
use super::*;
use crate::host::project::prompt::ProjectDialog;

struct Marked(&'static str, &'static str);
impl Plugin for Marked {
    fn manifest(&self) -> Manifest {
        Manifest::new(self.0, "0").project(
            ProjectDecl::new()
                .marker(DetectionMarker::new(self.1, MarkerKind::File))
                .screen(ProjectScreen::new("changes", "Changes")),
        )
    }
    fn open(&self, _: Input, _: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        Ok(Box::new(ProjectSession))
    }
}

struct Chooser {
    _dir: konoma_test_support::TmpDir,
    root: PathBuf,
    prepared: PreparedProjects,
    state: ProjectLoopState,
}

fn chooser() -> Chooser {
    let dir = konoma_test_support::unique_tmp("project_chooser");
    std::fs::create_dir_all(&dir).unwrap();
    let dir = dir.into_canonical().unwrap();
    std::fs::write(dir.join("a.marker"), "").unwrap();
    std::fs::write(dir.join("b.marker"), "").unwrap();
    let mut prepared = crate::app::project::prepare::prepare_projects_with_previews(
        vec![
            Arc::new(Marked("alpha", "a.marker")),
            Arc::new(Marked("beta", "b.marker")),
        ],
        Vec::new(),
        &dir,
        TermCaps::new(),
        (SharedConfig::parse("[ui]\nlang='en'\n").unwrap(), None),
        Arc::new(Services),
    )
    .unwrap_or_else(|e| panic!("{}", e.error));
    let state = ProjectLoopState::new(&mut prepared, false);
    let root = dir.to_path_buf();
    Chooser {
        _dir: dir,
        root,
        prepared,
        state,
    }
}

impl Chooser {
    fn listed(&self) -> Vec<String> {
        match &self.state.dialog {
            Some(ProjectDialog::Projects { projects, .. }) => {
                projects.iter().map(|k| k.plugin.clone()).collect()
            }
            _ => Vec::new(),
        }
    }

    /// Removes the marker, lets the worker rescan (the injected event is fenced) and polls.
    fn lose(&mut self, marker: &str) {
        let path = self.root.join(marker);
        std::fs::remove_file(&path).unwrap();
        self.prepared
            .detection
            .inject_event(Ok(notify::Event::new(notify::EventKind::Remove(
                notify::event::RemoveKind::Any,
            ))
            .add_path(path)));
        let info = self.state.info(&self.prepared, &ProjectLoopCx::default());
        assert!(self.state.poll_detection(&mut self.prepared, &info));
    }

    /// The normal Enter on the chooser as it is.
    fn press_enter(&mut self) -> bool {
        let info = self.state.info(&self.prepared, &ProjectLoopCx::default());
        self.state.key(
            &mut self.prepared,
            &info,
            KeyEvent::new(crossterm::event::KeyCode::Enter, KeyModifiers::NONE),
        )
    }
}

#[test]
fn a_lost_candidate_leaves_the_chooser_with_the_remaining_one() {
    let mut c = chooser();
    assert_eq!(c.listed(), ["alpha", "beta"]);
    c.lose("a.marker");
    assert_eq!(c.listed(), ["beta"], "the gone candidate is dropped");
    assert!(!c.state.project_gone, "one candidate is left to choose");
    assert!(c.state.screen.is_none(), "nothing opens by itself");
}

#[test]
fn a_chooser_with_no_candidate_left_ends_as_a_start_without_a_project() {
    let mut c = chooser();
    c.lose("a.marker");
    c.lose("b.marker");
    assert!(c.state.dialog.is_none(), "the empty chooser is closed");
    assert!(c.state.project_gone);
}

#[test]
fn choosing_a_candidate_that_is_no_longer_detected_is_refused() {
    let mut c = chooser();
    // The worker's answer is adopted by the detection alone, as when it lands between two keys:
    // alpha is no longer detected, but the chooser still lists it as the selected row and the
    // loop still holds alpha's runtime.
    let path = c.root.join("a.marker");
    std::fs::remove_file(&path).unwrap();
    c.prepared
        .detection
        .inject_event(Ok(notify::Event::new(notify::EventKind::Remove(
            notify::event::RemoveKind::Any,
        ))
        .add_path(path)));
    assert_eq!(c.prepared.detection.poll(), Some(false));
    let detected: Vec<_> = c
        .prepared
        .detection
        .projects
        .iter()
        .map(|p| p.plugin.as_str())
        .collect();
    assert_eq!(detected, ["beta"], "alpha is no longer detected");
    assert_eq!(c.listed(), ["alpha", "beta"], "the chooser is stale");
    assert!(
        c.state.runtimes.keys().any(|k| k.plugin == "alpha"),
        "alpha's runtime is still held"
    );
    assert!(!c.press_enter());
    assert!(c.state.screen.is_none(), "no screen from a gone project");
    assert_eq!(c.listed(), ["beta"], "asked again among the detected ones");
    assert_eq!(c.prepared.notice.as_deref(), Some("no project here"));
    assert!(!c.state.project_gone);
}

#[test]
fn choosing_when_nothing_is_detected_ends_the_loop() {
    let mut c = chooser();
    // The worker's answer is adopted by the detection alone, as when it lands between two keys:
    // the loop still holds the chooser, the runtimes and `project_gone = false` from before.
    let paths = ["a.marker", "b.marker"].map(|m| c.root.join(m));
    for path in &paths {
        std::fs::remove_file(path).unwrap();
    }
    let mut event = notify::Event::new(notify::EventKind::Remove(notify::event::RemoveKind::Any));
    for path in paths {
        event = event.add_path(path);
    }
    c.prepared.detection.inject_event(Ok(event));
    assert_eq!(c.prepared.detection.poll(), Some(false));
    assert!(c.prepared.detection.projects.is_empty());
    assert_eq!(c.listed(), ["alpha", "beta"], "the chooser is stale");
    assert!(!c.state.project_gone);
    // Enter on the stale chooser goes through the normal key handling.
    assert!(!c.press_enter());
    assert!(c.state.screen.is_none(), "no screen from a gone project");
    assert!(c.state.dialog.is_none());
    assert!(c.state.project_gone, "nothing is left, the loop ends");
    assert_eq!(c.prepared.notice.as_deref(), Some("no project here"));
}

impl Chooser {
    /// Adopt an answer whose watch failed after a scan that lists only the projects `keep` accepts.
    fn adopt_failed_answer_keeping(&mut self, keep: impl Fn(&str) -> bool) {
        let kept: Vec<_> = self
            .prepared
            .detection
            .projects
            .iter()
            .filter(|p| keep(&p.plugin))
            .cloned()
            .collect();
        self.prepared
            .detection
            .publish_answer_for_test(Some(kept), true);
        let info = self.state.info(&self.prepared, &ProjectLoopCx::default());
        assert!(self.state.poll_detection(&mut self.prepared, &info));
    }
}

/// A scan that succeeded and then failed to watch publishes its new list with `failed = true`: the
/// projects it no longer lists are retired as with any answer, and the watch warning stays.
#[test]
fn a_failed_answer_that_drops_a_project_retires_it() {
    let mut c = chooser();
    c.adopt_failed_answer_keeping(|plugin| plugin == "beta");
    assert_eq!(c.listed(), ["beta"], "alpha is dropped from the chooser");
    assert!(!c.state.project_gone, "beta is left to choose");
    assert!(
        !c.state.runtimes.keys().any(|k| k.plugin == "alpha"),
        "alpha's runtime is retired"
    );
    assert_eq!(
        c.prepared.notice.as_deref(),
        Some(crate::app::project::watch_warning(c.prepared.lang)),
        "the failure notice stays"
    );
    c.adopt_failed_answer_keeping(|_| false);
    assert!(c.state.dialog.is_none(), "the empty chooser is closed");
    assert!(
        c.state.project_gone,
        "nothing is left: as a start without a project"
    );
}
