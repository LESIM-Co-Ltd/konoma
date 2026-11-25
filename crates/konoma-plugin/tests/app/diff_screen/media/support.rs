use konoma_host_core::{project::DiffLayout, shared::SharedConfig};
use konoma_plugin::{
    app::{diff_screen::*, project::*},
    host::project::{MatchedProject, ProjectScreenState, ScreenKey},
    prelude::*,
};
use konoma_vcs_core::git::{ChangeEntry, DiffLine};
use std::{
    path::Path,
    sync::{mpsc, Arc},
};
struct Services;
impl ProjectServices for Services {
    fn inspect(
        &self,
        p: &MatchedProject,
        _: &ProjectSettings,
    ) -> Result<ProjectServiceInfo, Failure> {
        Ok(ProjectServiceInfo {
            workdir: p.root.clone(),
            backend: ProjectBackend::Git,
        })
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
    fn spawn_file_diff(self: Arc<Self>, r: FileDiffRequest) -> Result<FileDiffReply, Failure> {
        let (tx, rx) = mpsc::channel();
        tx.send(Ok(FileDiffData {
            lines: vec![],
            paths: vec![r.path],
        }))
        .unwrap();
        Ok(rx)
    }
}
struct Project;
impl Plugin for Project {
    fn manifest(&self) -> Manifest {
        Manifest::new("media-screen-fixture", "1").project(
            ProjectDecl::new()
                .marker(DetectionMarker::new("project.marker", MarkerKind::File))
                .screen(ProjectScreen::new("items", "Items")),
        )
    }
    fn open(&self, _: Input, _: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        struct Empty;
        impl Session for Empty {
            fn view(&mut self, _: &ViewReq, _: &ViewCx) -> Result<View, Failure> {
                Ok(View::List(Default::default()))
            }
        }
        Ok(Box::new(Empty))
    }
}
pub(super) fn prepared(root: &Path) -> PreparedProjects {
    std::fs::create_dir_all(root).unwrap();
    std::fs::write(root.join("project.marker"), "").unwrap();
    let cfg = SharedConfig::parse("[ui]\nlang='en'").unwrap();
    let mut p = prepare_projects(
        vec![Arc::new(Project)],
        root,
        konoma_plugin::TermCaps::new(),
        (cfg.clone(), None),
        Arc::new(Services),
    )
    .unwrap_or_else(|e| panic!("{}", e.error));
    let project = p.runtimes.keys().next().unwrap().clone();
    p.diff_return = Some(ProjectScreenState::new(
        project,
        ScreenKey {
            tab: 0,
            screen: "items".into(),
            selection: None,
        },
    ));
    p.diff = Some(
        DiffScreenState::open(
            FileDiffRequest {
                root: root.into(),
                path: root.join("side.svg"),
                info: ProjectServiceInfo {
                    workdir: root.into(),
                    backend: ProjectBackend::Git,
                },
            },
            Arc::new(Services),
            &cfg,
            DiffLayout::Unified,
            None,
        )
        .unwrap(),
    );
    p
}
