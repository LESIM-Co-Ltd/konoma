//! Standard preview classification never calls the project-only approximation.
use super::super::project::{ProjectBackend, ProjectSettings};
use super::*;
use crate::host::file_preview::{FilePreviewPlan, FilePreviewSource};
use crate::host::project::MatchedProject;
use crate::registry::{PluginOrigin, Registry};
use crate::{Bind, Input, Manifest, OpenCx, Plugin, Session, ViewKind};
use konoma_host_core::project::DiffTarget;
use konoma_vcs_core::git::ChangeEntry;
use std::path::Path;

#[test]
fn project_diff_preserves_statusbar_configuration() {
    for statusbar in ["split", "top", "bottom"] {
        let config = SharedConfig::parse(&format!("[ui]\nstatusbar='{statusbar}'\n")).unwrap();
        assert_eq!(config.ui.statusbar, statusbar);
    }
}

struct Document;
impl Plugin for Document {
    fn manifest(&self) -> Manifest {
        Manifest::new("another-document-id", "0")
            .bind(Bind::mime("image/*"))
            .shows(ViewKind::Document)
    }
    fn open(&self, _: Input, _: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        unreachable!()
    }
}

pub(super) struct Services(pub(super) std::thread::ThreadId);
impl ProjectServices for Services {
    fn inspect(
        &self,
        _: &MatchedProject,
        _: &ProjectSettings,
    ) -> Result<ProjectServiceInfo, Failure> {
        unreachable!()
    }
    fn file_diff(&self, _: &ProjectServiceInfo, _: &Path) -> Vec<DiffLine> {
        unreachable!()
    }
    fn changed_files(&self, _: &ProjectServiceInfo) -> Vec<ChangeEntry> {
        unreachable!()
    }
    fn base_contents(&self, _: &ProjectServiceInfo, _: &Path) -> Option<Vec<u8>> {
        assert_ne!(
            std::thread::current().id(),
            self.0,
            "baseline reading belongs on a worker"
        );
        Some(b"\x89PNG\r\n\x1a\n\0\0\0\0".to_vec())
    }
    fn diff_target(&self, _: &Path, _: Option<&[u8]>) -> DiffTarget {
        panic!("standard previews must not call the simplified service decision")
    }
    fn spawn_file_diff(self: Arc<Self>, _: FileDiffRequest) -> Result<FileDiffReply, Failure> {
        let (tx, rx) = mpsc::channel();
        tx.send(Ok(FileDiffData {
            lines: Vec::new(),
            paths: Vec::new(),
        }))
        .unwrap();
        Ok(rx)
    }
}

#[test]
fn project_preview_worker_keeps_the_plan_and_uses_baseline_facts() {
    let dir = konoma_test_support::unique_tmp("preview_plan_worker");
    std::fs::create_dir_all(&dir).unwrap();
    let mut registry = Registry::new();
    registry
        .register(Arc::new(Document), PluginOrigin::Standard)
        .unwrap();
    let source = FilePreviewSource::new(Arc::new(registry), Vec::new(), false);
    let mut state = DiffScreenState::open_for_project(
        FileDiffRequest {
            root: dir.to_path_buf(),
            path: dir.join("deleted"),
            info: ProjectServiceInfo {
                workdir: dir.to_path_buf(),
                backend: ProjectBackend::Git,
            },
        },
        Arc::new(Services(std::thread::current().id())),
        &SharedConfig::default(),
        DiffLayout::Unified,
        Some(DiffView::Rendered),
        Some(source),
    )
    .unwrap();
    let reply = state
        .preview_reply
        .take()
        .unwrap()
        .recv_timeout(Duration::from_secs(10))
        .unwrap();
    assert!(
        !reply.1,
        "the baseline must not turn a missing file into an existing one"
    );
    assert_eq!(
        reply.0.plan,
        FilePreviewPlan::Plugin {
            id: "another-document-id".into(),
            hint: None
        }
    );
    assert_eq!(
        reply.0.diff_target,
        DiffTarget::Markdown,
        "the declaration decides, regardless of id or name"
    );
    state.resolved_preview = Some(reply);
    assert_eq!(state.target(), DiffTarget::Markdown);
    assert_eq!(
        state.representations(),
        vec![DiffView::Source, DiffView::Rendered]
    );
}

#[test]
fn preview_representations_and_representation_bindings_require_the_executor() {
    struct TextServices;
    impl ProjectServices for TextServices {
        fn inspect(
            &self,
            _: &MatchedProject,
            _: &ProjectSettings,
        ) -> Result<ProjectServiceInfo, Failure> {
            unreachable!()
        }
        fn file_diff(&self, _: &ProjectServiceInfo, _: &Path) -> Vec<DiffLine> {
            unreachable!()
        }
        fn changed_files(&self, _: &ProjectServiceInfo) -> Vec<ChangeEntry> {
            unreachable!()
        }
        fn base_contents(&self, _: &ProjectServiceInfo, _: &Path) -> Option<Vec<u8>> {
            unreachable!()
        }
        fn spawn_file_diff(
            self: Arc<Self>,
            request: FileDiffRequest,
        ) -> Result<FileDiffReply, Failure> {
            Arc::new(Services(std::thread::current().id())).spawn_file_diff(request)
        }
    }
    let dir = konoma_test_support::unique_tmp("preview_executor_representations");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("file.txt");
    std::fs::write(&path, "plain text\n").unwrap();
    let config =
        SharedConfig::parse("[ui]\ndiff_view='source'\n[keys.diff]\nx='cycle_representation'\n")
            .unwrap();
    let mut state = DiffScreenState::open(
        FileDiffRequest {
            root: dir.to_path_buf(),
            path,
            info: ProjectServiceInfo {
                workdir: dir.to_path_buf(),
                backend: ProjectBackend::Git,
            },
        },
        Arc::new(TextServices),
        &config,
        DiffLayout::Unified,
        None,
    )
    .unwrap();
    for key in ['R', 'x'] {
        assert_eq!(state.representations(), vec![DiffView::Source]);
        state.handle_key(KeyPress::ch(key)).unwrap();
        assert!(state.take_command().is_none());
        assert_eq!(state.choice, DiffView::Source);
    }
    state = state.with_file_previews(FilePreviewSource::new(
        Arc::new(Registry::new()),
        Vec::new(),
        false,
    ));
    state.settle_preview_for_test();
    assert_eq!(
        state.representations(),
        vec![DiffView::Source, DiffView::Preview]
    );
    for key in ['R', 'x'] {
        state.handle_key(KeyPress::ch(key)).unwrap();
        assert_eq!(
            state.take_command(),
            Some(DiffScreenCommand::OpenPreview {
                path: state.request.path.clone()
            })
        );
    }
}
