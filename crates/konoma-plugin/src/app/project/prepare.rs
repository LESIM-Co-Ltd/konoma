use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use konoma_host_core::project::ProjectSettings;
use konoma_host_core::shared::SharedConfig;

use crate::host::project::detection::project_declarations;
use crate::host::project::prompt::{choose_projects, ProjectPromptUpdate};
use crate::host::project::{
    ProjectDetection, ProjectEnv, ProjectKey, ProjectMessages, ProjectRuntime, ProjectScreenState,
    ScreenKey,
};
use crate::project::Chip;
use crate::{Plugin, ReadScope, TermCaps};

use super::{PreparedProjects, ProjectServices};
use crate::app::{keys_from_load, register_all, AppError, Display, PrepareFailure};

/// Prepare a directory's projects using only the static plugins supplied by the program.
/// Discovery, backend inspection and every owner open finish on the ordinary screen. Config is
/// supplied by the caller, so this entry never reads the user's config or takes the terminal.
pub fn prepare_projects(
    plugins: Vec<Arc<dyn Plugin>>,
    dir: &Path,
    term: TermCaps,
    loaded: (SharedConfig, Option<String>),
    services: Arc<dyn ProjectServices>,
) -> Result<PreparedProjects, PrepareFailure> {
    let (config, load_error) = loaded;
    prepare(
        plugins,
        dir,
        term,
        config,
        services,
        ProjectDetection::default(),
    )
    .map(|mut prepared| {
        prepared.load_error = load_error.clone();
        prepared
    })
    .map_err(|error| PrepareFailure { error, load_error })
}

fn prepare(
    plugins: Vec<Arc<dyn Plugin>>,
    dir: &Path,
    term: TermCaps,
    config: SharedConfig,
    services: Arc<dyn ProjectServices>,
    mut detection: ProjectDetection,
) -> Result<PreparedProjects, AppError> {
    let detection_error = |e: std::io::Error| AppError::ProjectDetection(e.to_string());
    let dir = std::path::absolute(dir).map_err(detection_error)?;
    let metadata = std::fs::metadata(&dir).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            AppError::NoDirectory(dir.clone())
        } else if e.kind() == std::io::ErrorKind::NotADirectory {
            AppError::NotADirectory(dir.clone())
        } else {
            detection_error(e)
        }
    })?;
    if !metadata.is_dir() {
        return Err(AppError::NotADirectory(dir));
    }
    let dir = dir.canonicalize().map_err(detection_error)?;
    let mut registry = register_all(plugins, Vec::new())?;
    for (id, table) in &config.plugins {
        if table.get("enabled").and_then(|v| v.as_bool()) == Some(false) {
            registry.set_enabled(id, false);
        }
    }
    detection.ensure(&dir, project_declarations(&registry, |_| true));
    let watch_failed = detection.wait_for_initial().map_err(detection_error)?;
    let projects: Vec<_> = detection
        .projects
        .iter()
        .filter(|p| p.declaration.screens.iter().any(|s| !s.pushed))
        .cloned()
        .collect();
    if projects.is_empty() {
        return Err(AppError::NoProject(dir));
    }
    let settings = ProjectSettings::from_config(&config);
    let display = Display::of(&config);
    let (keys, _) = keys_from_load((config.clone(), None));
    let lang = match keys.lang() {
        konoma_host_core::lang::Lang::Jp => "ja",
        konoma_host_core::lang::Lang::En => "en",
    };
    let mut runtimes = HashMap::new();
    let mut service_info = HashMap::new();
    for project in &projects {
        let info = services
            .inspect(project, &settings)
            .map_err(|e| AppError::ProjectOpen(e.message.get(lang).into()))?;
        let plugin = registry.plugin(&project.plugin).ok_or_else(|| {
            AppError::Plugin(format!("plugin `{}` is not registered", project.plugin))
        })?;
        let mut plugin_settings =
            crate::config::plugin_settings(&config.plugins, &config.ui.moved(), &project.plugin);
        if plugin_settings.get("path_style").is_none() {
            plugin_settings.set("path_style", crate::Value::Str(settings.path_style.clone()));
        }
        let theme = settings.theme.code_theme.clone();
        let chip = project
            .declaration
            .chip
            .as_ref()
            .map(|c| Chip::new(c.get(lang)));
        let actions = registry
            .manifest(&project.plugin)
            .map(|m| m.actions.iter().map(|a| a.id.clone()).collect())
            .unwrap_or_default();
        let mut runtime = ProjectRuntime::start(
            project.clone(),
            plugin,
            actions,
            chip,
            move || ProjectEnv {
                settings: plugin_settings,
                lang: lang.into(),
                theme,
                term,
                read_scope: ReadScope::unrestricted(),
                messages: messages(lang),
            },
            |effect: &mut crate::host::project::ProjectEffect<super::StandaloneSurface>,
             outcome| {
                effect.outcome = Some(outcome);
            },
        );
        let reply = runtime
            .rx
            .recv()
            .map_err(|e| AppError::ProjectOpen(e.to_string()))?;
        if let Some(error) = &reply.error {
            return Err(AppError::ProjectOpen(error.message.get(lang).into()));
        }
        // Let the shared reply adopter apply watch/chip/enabled state when the loop is connected.
        runtime.pending = Some(reply);
        let key = ProjectKey::from(project);
        service_info.insert(key.clone(), info);
        runtimes.insert(key, runtime);
    }
    let mut dialog = None;
    let screen = match choose_projects(&mut dialog, projects.iter().map(ProjectKey::from).collect())
    {
        ProjectPromptUpdate::ChosenProject(key) => {
            let screen = projects
                .iter()
                .find(|p| ProjectKey::from(*p) == key)
                .and_then(|p| p.declaration.screens.iter().find(|s| !s.pushed))
                .expect("every candidate has a root screen");
            Some(ProjectScreenState::new(key, ScreenKey::new(&screen.id, 0)))
        }
        _ => None,
    };
    Ok(PreparedProjects {
        registry: Arc::new(registry),
        open_dir: dir,
        detection,
        watch_failed,
        runtimes,
        service_info,
        services,
        screen,
        dialog,
        config,
        settings,
        keys,
        lang,
        term,
        display,
        load_error: None,
        notice: None,
        diff: None,
        diff_return: None,
    })
}

fn messages(lang: &str) -> ProjectMessages {
    let text = |en: &str, ja: &str| if lang == "ja" { ja.into() } else { en.into() };
    ProjectMessages {
        worker_failed: text("project worker failed", "プロジェクトの処理に失敗しました"),
        no_changes: text("(no changes)", "(変更なし)"),
        mermaid_caption: text("mermaid", "mermaid"),
        view_unsupported: text(
            "this project view cannot be shown",
            "このプロジェクト表示には対応していません",
        ),
        outside_read_dirs: text("outside read directories", "読み取り範囲外です"),
        invalid_watch_paths: crate::t!(en: "Invalid project watch paths", ja: "プロジェクトの監視パスが不正です"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::project::{ProjectBackend, ProjectServiceInfo};
    use crate::host::project::detection::{MatchedProject, StartupFailure};
    use crate::prelude::*;
    use konoma_vcs_core::git::{ChangeEntry, DiffLine};
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Fixture(Arc<AtomicUsize>);
    struct FixtureSession;

    impl Plugin for Fixture {
        fn manifest(&self) -> Manifest {
            Manifest::new("fixture", "0.1.0").project(
                ProjectDecl::new()
                    .marker(DetectionMarker::new("project.marker", MarkerKind::File))
                    .screen(ProjectScreen::new("items", "Items")),
            )
        }

        fn open(&self, _: Input, _: &OpenCx) -> Result<Box<dyn Session>, Failure> {
            self.0.fetch_add(1, Ordering::Relaxed);
            Ok(Box::new(FixtureSession))
        }
    }

    impl Session for FixtureSession {
        fn view(&mut self, _: &ViewReq, _: &ViewCx) -> Result<View, Failure> {
            panic!("preparation does not draw a view")
        }
    }

    struct Services;
    impl ProjectServices for Services {
        fn inspect(
            &self,
            project: &MatchedProject,
            _: &ProjectSettings,
        ) -> Result<ProjectServiceInfo, Failure> {
            Ok(ProjectServiceInfo {
                workdir: project.root.clone(),
                backend: ProjectBackend::Git,
            })
        }
        fn file_diff(&self, _: &ProjectServiceInfo, _: &Path) -> Vec<DiffLine> {
            panic!("raw retrieval belongs on a worker after preparation")
        }
        fn changed_files(&self, _: &ProjectServiceInfo) -> Vec<ChangeEntry> {
            panic!("an empty changed-file set is not an availability check")
        }
        fn base_contents(&self, _: &ProjectServiceInfo, _: &Path) -> Option<Vec<u8>> {
            panic!("raw retrieval belongs on a worker after preparation")
        }
    }

    #[test]
    fn prepare_projects_keeps_detected_projects_when_watcher_creation_fails() {
        let dir = konoma_test_support::unique_tmp("app_project_failed_watcher");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("project.marker"), "").unwrap();
        let opened = Arc::new(AtomicUsize::new(0));
        let plugin: Arc<dyn Plugin> = Arc::new(Fixture(opened.clone()));
        let prepare_with_failed_watcher = || {
            prepare(
                vec![plugin.clone()],
                &dir,
                TermCaps::new(),
                SharedConfig::default(),
                Arc::new(Services),
                ProjectDetection::with_startup_failure(StartupFailure::WatcherCreation),
            )
        };
        let prepared = prepare_with_failed_watcher()
            .unwrap_or_else(|e| panic!("watcher failure must keep detected projects: {e}"));
        assert_eq!(prepared.detection.projects.len(), 1);
        assert!(
            prepared.watch_failed,
            "retain the warning for the future UI entry"
        );
        assert_eq!(prepared.runtimes.len(), 1);
        assert_eq!(opened.load(Ordering::Relaxed), 1);
        assert_eq!(prepared.screen.unwrap().project.plugin, "fixture");

        std::fs::remove_file(dir.join("project.marker")).unwrap();
        let failure = prepare_with_failed_watcher()
            .err()
            .expect("no matching project");
        assert!(
            matches!(failure, AppError::NoProject(_)),
            "a successful empty scan is not a scan failure"
        );
        assert_eq!(failure.exit_code(), 2);
    }
}
