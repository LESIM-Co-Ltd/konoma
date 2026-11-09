//! Project preparation, terminal entry and the act, prompt and navigation loop.

mod looping;
mod prepare;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub use konoma_host_core::project::ProjectSettings;
use konoma_host_core::shared::SharedConfig;
use konoma_vcs_core::git::{ChangeEntry, DiffLine};

use crate::host::project::{
    MatchedProject, ProjectDetection, ProjectDialog, ProjectKey, ProjectRuntime, ProjectScreenState,
};
use crate::registry::Registry;
use crate::{Failure, TermCaps};

use super::{AppKeys, Display};

pub use looping::{run_project_with, ProjectLoopCx, StandaloneInvocation, StandaloneSurface};
pub use prepare::prepare_projects;

fn watch_warning(lang: &str) -> &'static str {
    if lang == "ja" {
        "プロジェクトの検出または監視に失敗しました"
    } else {
        "could not detect or watch projects"
    }
}

/// Explicit backend selection, independent of process-wide VCS preference and feature flags.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProjectBackend {
    Git,
    Jj,
}

/// A backend checked before terminal entry. Empty diffs and changed-file sets are valid answers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectServiceInfo {
    pub workdir: PathBuf,
    pub backend: ProjectBackend,
}

/// Backend I/O for the small project host. `inspect` runs during preparation; retrieval methods
/// belong on workers. These return raw data only: shared Markdown/media workers choose baselines
/// and prepare their own content. Classification and verbs remain registry/host decisions.
pub trait ProjectServices: Send + Sync {
    fn inspect(
        &self,
        project: &MatchedProject,
        settings: &ProjectSettings,
    ) -> Result<ProjectServiceInfo, Failure>;
    fn file_diff(&self, info: &ProjectServiceInfo, path: &Path) -> Vec<DiffLine>;
    fn changed_files(&self, info: &ProjectServiceInfo) -> Vec<ChangeEntry>;
    fn base_contents(&self, info: &ProjectServiceInfo, path: &Path) -> Option<Vec<u8>>;
}

/// Everything settled before terminal entry, retaining the shared detection and serial owners.
/// Preparation does not take the terminal or start the input loop.
pub struct PreparedProjects {
    pub registry: Arc<Registry>,
    pub open_dir: PathBuf,
    pub detection: ProjectDetection,
    /// Detection succeeded, but watching failed. The entry can warn and continue as konoma does.
    pub watch_failed: bool,
    pub runtimes: HashMap<
        ProjectKey,
        ProjectRuntime<
            crate::host::project::ProjectEffect<StandaloneSurface>,
            StandaloneInvocation,
        >,
    >,
    pub service_info: HashMap<ProjectKey, ProjectServiceInfo>,
    pub services: Arc<dyn ProjectServices>,
    pub screen: Option<ProjectScreenState<StandaloneInvocation>>,
    pub dialog: Option<ProjectDialog<StandaloneInvocation>>,
    pub config: SharedConfig,
    pub settings: ProjectSettings,
    pub keys: AppKeys,
    pub lang: &'static str,
    pub term: TermCaps,
    pub display: Display,
    /// Still unreported, as in `Prepared`.
    pub load_error: Option<String>,
    /// Latest delivered status, shown until the next key.
    pub notice: Option<String>,
}

/// Prepare and run statically registered projects, refusing input before taking the terminal.
pub fn run_projects(
    plugins: Vec<Arc<dyn crate::Plugin>>,
    dir: &Path,
    services: Arc<dyn ProjectServices>,
) -> Result<(), super::AppError> {
    use super::{term, AppError, PrepareFailure};
    use std::io::IsTerminal;

    let colors = super::colors_of(
        &std::env::var("COLORTERM").unwrap_or_default(),
        &std::env::var("TERM").unwrap_or_default(),
    );
    let mut prepared = match prepare_projects(
        plugins,
        dir,
        TermCaps::new().colors(colors),
        SharedConfig::load_default(),
        services,
    ) {
        Ok(prepared) => prepared,
        Err(failure) => return Err(failure.report(&mut std::io::stderr())),
    };
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        return Err(PrepareFailure {
            error: AppError::NoTerminal,
            load_error: prepared.load_error,
        }
        .report(&mut std::io::stderr()));
    }
    let cx = ProjectLoopCx {
        home: std::env::var_os("HOME").map(PathBuf::from),
    };
    term::install_exit_signals();
    let mut guard = term::TerminalGuard::enter()?;
    let konoma_widgets::term_query::Startup {
        typed, late_reply, ..
    } = konoma_widgets::term_query::picker_from_terminal();
    // Project Document content is currently prepared as Lines by the shared owner.
    let result = run_project_with(
        guard.terminal(),
        &mut prepared,
        &mut term::CrosstermEvents::after_query(&typed, late_reply),
        &cx,
        None,
    );
    guard.leave();
    let _ = super::write_load_error(&mut std::io::stderr(), prepared.load_error.as_deref());
    result
}
