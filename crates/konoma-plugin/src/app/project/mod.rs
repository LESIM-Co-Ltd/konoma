//! Project preparation, terminal entry and the act, prompt and navigation loop.

#[cfg(any(test, feature = "test-hooks"))]
mod graph_test_hooks;
#[cfg(any(test, feature = "test-hooks"))]
pub(in crate::app) mod idle_test_hooks;
#[cfg(any(test, feature = "test-hooks"))]
#[doc(hidden)]
pub mod media_test_hooks;
#[cfg(any(test, feature = "test-hooks"))]
#[doc(hidden)]
pub use graph_test_hooks::{
    probe_graph_plugin_for_test, record_graph_for_test, ProjectGraphProbe, ProjectGraphSnapshot,
};

// The dependency selects the expansion: a normal build never evaluates or types the payload.
#[cfg(any(test, feature = "test-hooks"))]
#[doc(hidden)]
#[macro_export]
macro_rules! project_graph_probe {
    ($snapshot:expr) => {
        $crate::app::project::record_graph_for_test($snapshot)
    };
}
#[cfg(not(any(test, feature = "test-hooks")))]
#[doc(hidden)]
#[macro_export]
macro_rules! project_graph_probe {
    ($snapshot:expr) => {};
}
mod launch;
mod looping;
mod prepare;
pub(in crate::app) mod preview;
#[cfg(any(test, feature = "test-hooks"))]
mod representation_test_hooks;
#[cfg(any(test, feature = "test-hooks"))]
#[doc(hidden)]
pub use idle_test_hooks::{
    observe_project_state_for_test, read_snapshot_for_test, run_project_fenced_for_test,
    run_project_fenced_state_for_test, run_project_observed_for_test, ProjectDiffSnapshot,
    ProjectHelpSnapshot, ProjectIdleSnapshot, ProjectLoopProbe, ProjectPromptSnapshot,
    ProjectReplyObserver, ProjectStateSnapshot, TestProjectClipboard,
};
#[cfg(any(test, feature = "test-hooks"))]
#[doc(hidden)]
pub use representation_test_hooks::drive_diff_representation_for_test;

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

pub use launch::ProjectLaunchTarget;
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

impl ProjectBackend {
    pub(in crate::app) fn diff_read_context(self) -> crate::host::project::DiffReadContext {
        use konoma_vcs_core::vcs::Preference;
        crate::host::project::DiffReadContext::new(match self {
            Self::Git => Preference::Git,
            Self::Jj => Preference::Jj,
        })
    }
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
pub trait ProjectServices: Send + Sync + 'static {
    #[cfg(any(test, feature = "test-hooks"))]
    fn diff_reply_observer_for_test(&self) -> Option<ProjectReplyObserver> {
        None
    }
    /// Opt in to the VCS program's project-diff chrome. Other embedders retain
    /// the ordinary file runner's layout and controls.
    fn project_diff_chrome(&self) -> bool {
        false
    }
    /// A service capability, queried without I/O. Execution belongs to a worker.
    fn discard_enabled(&self) -> bool {
        false
    }
    fn discard_file(&self, _: &ProjectServiceInfo, _: &Path) -> Result<(), Failure> {
        Err(Failure::unsupported("discard is unavailable"))
    }
    /// Read the linked worktree's origin during preparation, before entering the UI loop.
    fn worktree_origin(&self, _: &Path) -> Option<String> {
        None
    }
    /// Opt in to preparing and replacing a project root. Notice-only hosts keep their
    /// existing navigation contract, just as services without a file-diff executor do.
    fn root_switch_enabled(&self) -> bool {
        false
    }

    /// Resolve the selected file on a worker. A host can wrap reply publication with a fence.
    fn spawn_launch_target(
        self: Arc<Self>,
        path: PathBuf,
        line: Option<usize>,
        registry: Arc<Registry>,
    ) -> std::sync::mpsc::Receiver<Option<ProjectLaunchTarget>> {
        ProjectLaunchTarget::spawn(path, line, self, registry)
    }

    /// Full file classification and verb lookup, called only by the launch worker.
    fn launch_verbs(&self, path: &Path, registry: &Registry) -> Vec<crate::VerbDecl> {
        crate::host::file_preview::selected_plugin_of_path(registry, &[], path)
            .and_then(|id| registry.manifest(&id).map(|m| m.verbs.clone()))
            .unwrap_or_default()
    }
    fn inspect(
        &self,
        project: &MatchedProject,
        settings: &ProjectSettings,
    ) -> Result<ProjectServiceInfo, Failure>;
    fn file_diff(&self, info: &ProjectServiceInfo, path: &Path) -> Vec<DiffLine>;
    fn changed_files(&self, info: &ProjectServiceInfo) -> Vec<ChangeEntry>;
    fn base_contents(&self, info: &ProjectServiceInfo, path: &Path) -> Option<Vec<u8>>;

    /// Filename/available-byte classification only; implementations must not perform I/O here.
    /// Workers may pass bytes for content sniffing. Preview execution is a separate capability.
    fn diff_target(
        &self,
        path: &Path,
        bytes: Option<&[u8]>,
    ) -> konoma_host_core::project::DiffTarget {
        use konoma_host_core::project::{DiffTarget, MediaDiffKind};
        if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("md"))
        {
            return DiffTarget::Markdown;
        }
        match crate::host::project::media_diff::media_format(path, Some(bytes.unwrap_or_default()))
        {
            Some(MediaDiffKind::Svg) => DiffTarget::Svg,
            Some(_) => DiffTarget::Picture,
            None => DiffTarget::Text,
        }
    }

    /// Opt in to file-diff execution. Existing project-only services retain their notice;
    /// executors use `FileDiffRequest::spawn`, optionally adding a reply-publication fence.
    fn spawn_file_diff(
        self: Arc<Self>,
        _: super::diff_screen::FileDiffRequest,
    ) -> Result<super::diff_screen::FileDiffReply, Failure> {
        Err(Failure::unsupported(crate::t!(
            en: "this action is not supported in the standalone app",
            ja: "この操作は単体アプリでは使えません",
        )))
    }
}

/// Everything settled before terminal entry, retaining the shared detection and serial owners.
/// Preparation does not take the terminal or start the input loop.
pub struct PreparedProjects {
    worktree_origin: Option<String>,
    #[cfg(any(test, feature = "test-hooks"))]
    test_observed: Option<Arc<std::sync::Mutex<Option<ProjectStateSnapshot>>>>,
    #[cfg(any(test, feature = "test-hooks"))]
    test_probe: Option<ProjectLoopProbe>,
    #[cfg(any(test, feature = "test-hooks"))]
    test_observe_pending: bool,
    #[cfg(any(test, feature = "test-hooks"))]
    test_observe_media: bool,
    #[cfg(any(test, feature = "test-hooks"))]
    test_clipboard: Option<TestProjectClipboard>,
    file_preview: Option<preview::FilePreviewState>,
    preview_open: Option<preview::PendingPreview>,
    #[cfg(test)]
    preview_open_fence: Option<std::sync::mpsc::Sender<()>>,
    pub(in crate::app) file_previews: Option<crate::host::file_preview::FilePreviewSource>,
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
    /// File diff ownership survives a caller's injected event source ending.
    pub diff: Option<super::diff_screen::DiffScreenState>,
    pub diff_return: Option<ProjectScreenState<StandaloneInvocation>>,
    preview_project: bool,
    root_switch: Option<looping::root_switch::PendingRootSwitch>,
    #[cfg(feature = "test-hooks")]
    root_switch_fence: Option<std::sync::mpsc::Sender<()>>,
}

impl PreparedProjects {
    pub(super) fn adopt_display(&mut self, display: super::Display) {
        self.config.ui.wrap = display.wrap;
        self.config.ui.path_style = match display.path_style {
            konoma_host_core::view_options::PathStyle::Relative => "relative",
            konoma_host_core::view_options::PathStyle::Home => "home",
            konoma_host_core::view_options::PathStyle::Full => "full",
        }
        .into();
        self.keys = self.keys.clone().with_wrap(display.wrap);
        self.display = display;
    }
}

#[cfg(feature = "test-hooks")]
impl PreparedProjects {
    /// Test fence: notification follows candidate publication and subsequent owner replies.
    pub fn fence_root_switches(&mut self, published: std::sync::mpsc::Sender<()>) {
        self.root_switch_fence = Some(published);
    }
}

/// Prepare and run statically registered projects, refusing input before taking the terminal.
pub fn run_projects(
    plugins: Vec<Arc<dyn crate::Plugin>>,
    dir: &Path,
    services: Arc<dyn ProjectServices>,
) -> Result<(), super::AppError> {
    run_projects_with_previews(plugins, Vec::new(), dir, services)
}

pub(crate) fn run_projects_with_previews(
    plugins: Vec<Arc<dyn crate::Plugin>>,
    previews: Vec<Arc<dyn crate::Plugin>>,
    dir: &Path,
    services: Arc<dyn ProjectServices>,
) -> Result<(), super::AppError> {
    use super::{term, AppError, PrepareFailure};
    use std::io::IsTerminal;

    let colors = super::colors_of(
        &std::env::var("COLORTERM").unwrap_or_default(),
        &std::env::var("TERM").unwrap_or_default(),
    );
    let mut prepared = match prepare::prepare_projects_with_previews(
        plugins,
        previews,
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
        launch: Some(
            super::LaunchCx::new(
                prepared.config.editor.clone(),
                &prepared.config.external,
                Vec::new(),
            )
            .live(),
        ),
    };
    term::install_exit_signals();
    let mut guard = term::TerminalGuard::enter()?;
    let konoma_widgets::term_query::Startup {
        picker,
        typed,
        late_reply,
    } = konoma_widgets::term_query::picker_from_terminal();
    let pictures = picker.map(|picker| {
        let config = prepared.config.clone();
        let env = super::DocEnv::from_registry(
            prepared.registry.clone(),
            prepared.lang,
            Box::new(move |id| {
                crate::config::plugin_settings(&config.plugins, &config.ui.moved(), id)
            }),
            prepared.config.external.remote_images,
        );
        super::Pictures::new(
            picker,
            "markdown",
            &prepared.open_dir,
            prepared.config.ui.svg_max_px,
        )
        .with_documents(env)
    });
    // Project Document content is currently prepared as Lines by the shared owner.
    let result = run_project_with(
        guard.terminal(),
        &mut prepared,
        &mut term::CrosstermEvents::after_query(&typed, late_reply),
        &cx,
        pictures,
    );
    guard.leave();
    let _ = super::write_load_error(&mut std::io::stderr(), prepared.load_error.as_deref());
    result
}
