//! Fences for the uninterrupted real loop. None of this is built into production hosts.
use crate::app::{AppError, EventSource, Next, Pictures};
use ratatui::{backend::Backend, buffer::Buffer, Terminal};

use super::{run_project_with, PreparedProjects, ProjectLoopCx};

/// Each bit describes work owed by the current standalone surface, not worker lifetime.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProjectIdleSnapshot {
    pub owners: bool,
    pub replay: bool,
    pub contexts: bool,
    pub detection: bool,
    pub watch: bool,
    pub representations: bool,
    pub preview: bool,
    pub pictures: bool,
    pub navigation: bool,
    pub state_generation: u64,
    pub drawn_generation: u64,
}

impl ProjectIdleSnapshot {
    pub fn is_idle(&self) -> bool {
        self.owners
            && self.replay
            && self.contexts
            && self.detection
            && self.watch
            && self.representations
            && self.preview
            && self.pictures
            && self.navigation
            && self.state_generation == self.drawn_generation
    }
}

/// The observer owns the complete logical frame and supplies the next input at each fence.
/// Returning `End` ends the run once, after all steps, preserving local state between steps.
/// Resizes use `AppEvent::Resize`, with the backend size supplied by the test backend.
pub fn run_project_fenced_for_test<B: Backend>(
    terminal: &mut Terminal<B>,
    prepared: &mut PreparedProjects,
    cx: &ProjectLoopCx,
    pictures: Option<Pictures>,
    observe: impl FnMut(ProjectIdleSnapshot, Buffer) -> Next,
) -> Result<(), AppError> {
    if prepared.file_previews.is_none() {
        prepared.file_previews = Some(crate::host::file_preview::FilePreviewSource::new(
            prepared.registry.clone(),
            prepared.config.preview.rules.clone(),
            prepared.config.external.preview_commands,
        ));
    }
    struct Fence<F>(F);
    impl<F: FnMut(ProjectIdleSnapshot, Buffer) -> Next> EventSource for Fence<F> {
        fn next(&mut self, _: Option<std::time::Duration>) -> Next {
            panic!("idle fence reached an unfenced input wait")
        }
        fn project_fenced(&self) -> bool {
            true
        }
        fn project_idle(&mut self, snapshot: ProjectIdleSnapshot, frame: Buffer) -> Next {
            assert!(snapshot.is_idle(), "project idle fence: {snapshot:?}");
            (self.0)(snapshot, frame)
        }
    }
    run_project_with(terminal, prepared, &mut Fence(observe), cx, pictures)
}

/// Runtime fixture lookup shared by SDK and root tests. `serde_json` is already an app
/// dependency here; the test-support crate intentionally has no JSON dependency.
pub fn read_snapshot_for_test(name: &str) -> serde_json::Value {
    assert!(
        !name.contains(['/', '\\']),
        "snapshot name must be a file stem"
    );
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../snapshots")
        .join(format!("{name}.json"));
    let contents =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("snapshot {name}: {e}"));
    serde_json::from_str(&contents).unwrap_or_else(|e| panic!("snapshot {name}: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_idle_item_and_a_draw_after_state_change_are_required() {
        let idle = ProjectIdleSnapshot {
            owners: true,
            replay: true,
            contexts: true,
            detection: true,
            watch: true,
            representations: true,
            preview: true,
            pictures: true,
            navigation: true,
            state_generation: 7,
            drawn_generation: 7,
        };
        assert!(idle.is_idle());
        let unset: [fn(&mut ProjectIdleSnapshot); 10] = [
            |s| s.owners = false,
            |s| s.replay = false,
            |s| s.contexts = false,
            |s| s.detection = false,
            |s| s.watch = false,
            |s| s.representations = false,
            |s| s.preview = false,
            |s| s.pictures = false,
            |s| s.navigation = false,
            |s| s.drawn_generation -= 1,
        ];
        for (item, unset) in unset.into_iter().enumerate() {
            let mut pending = idle.clone();
            unset(&mut pending);
            assert!(!pending.is_idle(), "unsettled item {item}: {pending:?}");
        }
    }
}
