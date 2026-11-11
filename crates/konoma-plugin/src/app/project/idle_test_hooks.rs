//! Fences for the uninterrupted real loop. None of this is built into production hosts.
use crate::app::{AppError, EventSource, Next, Pictures};
use ratatui::{backend::Backend, buffer::Buffer, Terminal};

use super::{run_project_with, PreparedProjects, ProjectLoopCx};
use crate::host::project::{
    ProjectContent, ProjectDialog, ProjectPrompt, ProjectScreenState, PromptInvocation,
};
use crate::project::ProjectRow;
use std::sync::{Arc, Mutex};

/// Owned, read-only facts at a fence. No host surface or action types cross this seam.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProjectStateSnapshot {
    pub diff_actions: Vec<(String, String)>,
    pub media: Option<super::media_test_hooks::MediaStateSnapshot>,
    pub return_to_preview: bool,
    pub diff: Option<ProjectDiffSnapshot>,
    pub root: Option<std::path::PathBuf>,
    pub open_dir: Option<std::path::PathBuf>,
    pub target: Option<std::path::PathBuf>,
    pub queued_inputs: usize,
    pub chip: Option<crate::project::Chip>,
    pub graph: Option<super::ProjectGraphSnapshot>,
    pub backend: Option<String>,
    pub stack: Vec<String>,
    pub stack_selection: Vec<Option<String>>,
    pub selected: Option<ProjectRow>,
    pub cursor: usize,
    pub scroll: u16,
    pub hscroll: u16,
    pub query: String,
    pub query_cursor: usize,
    pub filtering: bool,
    pub marked: Vec<ProjectRow>,
    pub rows: Vec<(ProjectRow, bool)>,
    pub visible: Vec<ProjectRow>,
    pub prompt: Option<ProjectPromptSnapshot>,
    pub menu: Option<String>,
    pub notice: Option<String>,
}

/// Read-only facts of one diff owner. Preview updates its live pose at its own frame fence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectDiffSnapshot {
    pub help: Option<ProjectHelpSnapshot>,
    pub path: std::path::PathBuf,
    pub baseline: String,
    pub choice: konoma_host_core::project::DiffView,
    pub effective: konoma_host_core::project::DiffView,
    pub next_representation: Option<konoma_host_core::project::DiffView>,
    pub layout: String,
    pub resolved_split: bool,
    pub split_drawn: Option<bool>,
    pub wrap: bool,
    pub scroll: usize,
    pub hscroll: u16,
    pub first_change: Option<usize>,
    pub editor_line: Option<usize>,
    pub return_to: Vec<String>,
    pub source_lines: Vec<(String, Option<u32>, Option<u32>, String)>,
    pub baseline_text: Option<String>,
    pub preview_context: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectHelpSnapshot {
    pub sections: Vec<(String, Vec<(String, String)>)>,
    pub scroll: usize,
    pub title: String,
}
impl ProjectDiffSnapshot {
    /// Old-side bytes of the displayed hunks; short baseline fixtures fit entirely in them.
    pub fn old_bytes(&self) -> Vec<u8> {
        self.source_lines
            .iter()
            .filter(|(_, old, _, _)| old.is_some())
            .flat_map(|(_, _, _, text)| text.bytes().chain(std::iter::once(b'\n')))
            .collect()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectPromptSnapshot {
    pub kind: String,
    pub action: String,
    pub stage: usize,
    pub buffer: String,
    pub cursor: usize,
    pub selected: usize,
    pub prompts: Vec<String>,
}

/// Read the same model from either host, without changing selection or normalizing identities.
pub fn observe_project_state_for_test<I: PromptInvocation>(
    nav: Option<&ProjectScreenState<I>>,
    content: Option<&ProjectContent>,
    dialog: Option<&ProjectDialog<I>>,
    menu: Option<&str>,
    notice: Option<String>,
) -> ProjectStateSnapshot {
    let mut state = ProjectStateSnapshot {
        menu: menu.map(str::to_owned),
        notice,
        ..Default::default()
    };
    if let Some(nav) = nav {
        state.backend = Some(nav.project.plugin.clone());
        state.stack = nav.stack.iter().map(|f| f.key.screen.clone()).collect();
        state.stack_selection = nav.stack.iter().map(|f| f.key.selection.clone()).collect();
        let context = nav.act_context(content);
        state.selected = context.selected;
        state.marked = context.marked;
        if let Some(frame) = nav.frame() {
            state.cursor = frame.cursor;
            state.scroll = frame.scroll;
            state.hscroll = frame.hscroll;
            state.query = frame.query.clone();
            // Project filters currently append/pop characters; their insertion point is the end.
            // Both hosts derive this from the query; the cursor is not observed independently.
            state.query_cursor = frame.query.chars().count();
            state.filtering = frame.filtering;
            state.queued_inputs = frame.pending.len();
        }
    }
    if let Some(ProjectContent::List(list)) = content {
        state.rows = list
            .rows
            .iter()
            .map(|r| (ProjectRow::from(r), r.selectable))
            .collect();
        state.visible = list
            .visible_rows(&state.query)
            .iter()
            .map(|i| ProjectRow::from(&list.rows[*i]))
            .collect();
    }
    state.prompt = dialog.map(|dialog| match dialog {
        ProjectDialog::Projects { projects, selected } => ProjectPromptSnapshot {
            kind: "projects".into(),
            action: "projects".into(),
            stage: 0,
            buffer: String::new(),
            cursor: 0,
            selected: *selected,
            prompts: projects
                .iter()
                .map(|p| format!("{}:{}", p.plugin, p.root.display()))
                .collect(),
        },
        ProjectDialog::Act {
            invocation,
            prompts,
            stage,
            buffer,
            cursor,
            selected,
        } => ProjectPromptSnapshot {
            kind: match &prompts[*stage] {
                ProjectPrompt::Confirmation(_) => "confirm",
                ProjectPrompt::Text(_) => "input",
                ProjectPrompt::Choice(_) => "choice",
            }
            .into(),
            action: invocation.prompt_invocation().action.action.clone(),
            stage: *stage,
            buffer: buffer.clone(),
            cursor: *cursor,
            selected: *selected,
            prompts: prompts
                .iter()
                .map(|p| match p {
                    ProjectPrompt::Confirmation(p) => format!("{p:?}"),
                    ProjectPrompt::Text(p) => format!("{p:?}"),
                    ProjectPrompt::Choice(p) => format!("{p:?}"),
                })
                .collect(),
        },
    });
    state
}

#[derive(Default)]
struct ClipboardState {
    fail: bool,
    writes: Vec<String>,
}

/// A sink and failure switch owned by exactly one PreparedProjects, safe beside parallel tests.
#[derive(Clone, Default)]
pub struct TestProjectClipboard(Arc<Mutex<ClipboardState>>);
impl TestProjectClipboard {
    pub fn fail(&self, fail: bool) {
        self.0.lock().unwrap().fail = fail;
    }
    pub fn writes(&self) -> Vec<String> {
        self.0.lock().unwrap().writes.clone()
    }
    pub(super) fn write(&self, text: &str) -> Result<(), String> {
        let mut sink = self.0.lock().unwrap();
        if sink.fail {
            return Err("test clipboard failure".into());
        }
        sink.writes.push(text.into());
        Ok(())
    }
}
impl PreparedProjects {
    pub fn observe_loop_for_test(&mut self) -> ProjectLoopProbe {
        if self.file_previews.is_none() {
            self.file_previews = Some(crate::host::file_preview::FilePreviewSource::new(
                self.registry.clone(),
                self.config.preview.rules.clone(),
                self.config.external.preview_commands,
            ));
        }
        let probe = ProjectLoopProbe::default();
        self.test_observed = Some(Default::default());
        self.test_probe = Some(probe.clone());
        probe
    }
    pub fn observe_media_for_test(&mut self) {
        self.test_observe_media = true;
    }
    pub fn clipboard_for_test(&mut self) -> TestProjectClipboard {
        self.test_clipboard
            .get_or_insert_with(Default::default)
            .clone()
    }
}

pub type ProjectReplyObserver = Arc<dyn Fn(&'static str, bool) + Send + Sync>;

pub(in crate::app) fn observe_replies<T: Send + 'static>(
    services: &dyn super::ProjectServices,
    kind: &'static str,
    rx: std::sync::mpsc::Receiver<T>,
) -> std::sync::mpsc::Receiver<T> {
    let Some(observe) = services.diff_reply_observer_for_test() else {
        return rx;
    };
    let (tx, output) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        while let Ok(reply) = rx.recv() {
            observe(kind, false);
            if tx.send(reply).is_err() {
                break;
            }
            observe(kind, true);
        }
    });
    output
}

type LoopObservation = (ProjectIdleSnapshot, Buffer, ProjectStateSnapshot);
#[derive(Clone, Default)]
pub struct ProjectLoopProbe(Arc<Mutex<Option<LoopObservation>>>);
impl ProjectLoopProbe {
    pub fn snapshot(&self) -> LoopObservation {
        self.0
            .lock()
            .unwrap()
            .clone()
            .expect("the normal loop published its frame before waiting")
    }
    pub(in crate::app) fn capture(
        &self,
        idle: ProjectIdleSnapshot,
        frame: Buffer,
        state: ProjectStateSnapshot,
    ) {
        *self.0.lock().unwrap() = Some((idle, frame, state));
    }
}

/// U8a's uninterrupted driver with additional read-only state; its original observer is unchanged.
pub fn run_project_fenced_state_for_test<B: Backend>(
    terminal: &mut Terminal<B>,
    prepared: &mut PreparedProjects,
    cx: &ProjectLoopCx,
    pictures: Option<Pictures>,
    mut observe: impl FnMut(ProjectIdleSnapshot, Buffer, ProjectStateSnapshot) -> Next,
) -> Result<(), AppError> {
    let slot = Arc::new(Mutex::new(None));
    prepared.test_observed = Some(slot.clone());
    let result = run_project_fenced_for_test(terminal, prepared, cx, pictures, |idle, frame| {
        // A Preview can emit several file fences while its project owner remains suspended.
        let state = slot
            .lock()
            .unwrap()
            .clone()
            .expect("state published at the idle fence");
        observe(idle, frame, state)
    });
    prepared.test_observed = None;
    result
}

/// Observe pending candidate/owner fences as well as idle frames, without ending the loop.
/// The observer supplies ordinary terminal events; it never borrows or mutates loop state.
pub fn run_project_observed_for_test<B: Backend>(
    terminal: &mut Terminal<B>,
    prepared: &mut PreparedProjects,
    cx: &ProjectLoopCx,
    observe: impl FnMut(ProjectIdleSnapshot, Buffer, ProjectStateSnapshot) -> Next,
) -> Result<(), AppError> {
    prepared.test_observe_pending = true;
    let result = run_project_fenced_state_for_test(terminal, prepared, cx, None, observe);
    prepared.test_observe_pending = false;
    result
}

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
    struct Fence<F>(F, bool);
    impl<F: FnMut(ProjectIdleSnapshot, Buffer) -> Next> EventSource for Fence<F> {
        fn next(&mut self, _: Option<std::time::Duration>) -> Next {
            panic!("idle fence reached an unfenced input wait")
        }
        fn project_fenced(&self) -> bool {
            true
        }
        fn project_idle(&mut self, snapshot: ProjectIdleSnapshot, frame: Buffer) -> Next {
            assert!(
                self.1 || snapshot.is_idle(),
                "project idle fence: {snapshot:?}"
            );
            (self.0)(snapshot, frame)
        }
    }
    let pending = prepared.test_observe_pending;
    run_project_with(
        terminal,
        prepared,
        &mut Fence(observe, pending),
        cx,
        pictures,
    )
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
