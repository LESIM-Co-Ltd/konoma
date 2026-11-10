//! Backend I/O for the standalone project app, independent of the host's global VCS preference.

use std::path::Path;
use std::sync::Arc;

use konoma_plugin::app::diff_screen::{FileDiffReply, FileDiffRequest};

use konoma_plugin::app::project::{
    ProjectBackend, ProjectServiceInfo, ProjectServices, ProjectSettings,
};
use konoma_plugin::host::project::MatchedProject;
use konoma_plugin::Failure;
use konoma_vcs_core::git::{ChangeEntry, DiffLine};
use konoma_vcs_core::vcs::{Git, Vcs};

/// The program supplies this service along with its static Git/Jj plugins. No terminal or
/// preview plugin is installed here. Raw retrieval is synchronous and must run on a worker.
pub struct VcsServices;

/// Run the project program with the standard file plugins available as Preview destinations.
pub fn main() -> std::process::ExitCode {
    konoma_plugin::proc::cli_main_projects(
        crate::program(),
        konoma_file_plugins::plugins(),
        Arc::new(VcsServices),
    )
}

impl ProjectServices for VcsServices {
    fn root_switch_enabled(&self) -> bool {
        true
    }

    fn spawn_file_diff(
        self: Arc<Self>,
        request: FileDiffRequest,
    ) -> Result<FileDiffReply, Failure> {
        request.spawn(self)
    }

    fn inspect(
        &self,
        project: &MatchedProject,
        settings: &ProjectSettings,
    ) -> Result<ProjectServiceInfo, Failure> {
        #[cfg(feature = "git")]
        let checked = match project.plugin.as_str() {
            "git" if settings.git_enabled && konoma_vcs_core::git::git_binary_available() => Git
                .workdir(&project.root)
                .map(|workdir| ProjectServiceInfo {
                    workdir,
                    backend: ProjectBackend::Git,
                }),
            "jj" if konoma_vcs_core::vcs::jj::available() => {
                // workspace_root alone only checks a marker. A successful read of @ proves
                // that this jj can actually open the workspace, without snapshotting it.
                konoma_vcs_core::vcs::jj::workspace_root(&project.root).and_then(|workdir| {
                    konoma_vcs_core::vcs::jj::meta(&workdir).map(|_| ProjectServiceInfo {
                        workdir,
                        backend: ProjectBackend::Jj,
                    })
                })
            }
            _ => None,
        };
        #[cfg(not(feature = "git"))]
        let checked = {
            let _ = settings;
            None
        };
        checked.ok_or_else(|| Failure::external(konoma_plugin::t!(
            en: format!("{} backend is unavailable or cannot open this repository", project.plugin),
            ja: format!("{} バックエンドが利用できないか、このリポジトリを開けません", project.plugin)
        )))
    }

    fn file_diff(&self, info: &ProjectServiceInfo, path: &Path) -> Vec<DiffLine> {
        backend(info.backend)
            .map(|b| b.file_diff(&info.workdir, path))
            .unwrap_or_default()
    }

    fn changed_files(&self, info: &ProjectServiceInfo) -> Vec<ChangeEntry> {
        backend(info.backend)
            .map(|b| b.changed_files(&info.workdir))
            .unwrap_or_default()
    }

    fn base_contents(&self, info: &ProjectServiceInfo, path: &Path) -> Option<Vec<u8>> {
        backend(info.backend)?.base_contents(&info.workdir, path)
    }
}

fn backend(kind: ProjectBackend) -> Option<&'static dyn Vcs> {
    match kind {
        ProjectBackend::Git => Some(&Git),
        #[cfg(feature = "git")]
        ProjectBackend::Jj => Some(&konoma_vcs_core::vcs::Jj),
        #[cfg(not(feature = "git"))]
        ProjectBackend::Jj => None,
    }
}
