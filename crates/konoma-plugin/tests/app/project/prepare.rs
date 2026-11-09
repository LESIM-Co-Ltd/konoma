use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use konoma_host_core::project::ProjectSettings;
use konoma_host_core::shared::SharedConfig;
use konoma_plugin::app::project::{
    prepare_projects, ProjectBackend, ProjectServiceInfo, ProjectServices,
};
use konoma_plugin::app::AppError;
use konoma_plugin::host::project::MatchedProject;
use konoma_plugin::prelude::*;
use konoma_plugin::TermCaps;
use konoma_test_support::{unique_tmp, TmpDir};
use konoma_vcs_core::git::{ChangeEntry, DiffLine};

type OpenLog = Arc<Mutex<Vec<(String, Input, Settings, String, TermCaps)>>>;

struct ProjectPlugin {
    id: &'static str,
    seen: OpenLog,
    fail_open: bool,
    remove_on_manifest: Option<PathBuf>,
}

impl Plugin for ProjectPlugin {
    fn manifest(&self) -> Manifest {
        if let Some(path) = &self.remove_on_manifest {
            let _ = std::fs::remove_dir(path);
        }
        Manifest::new(self.id, "0.1.0").project(
            ProjectDecl::new()
                .marker(DetectionMarker::new("project.marker", MarkerKind::File))
                .screen(ProjectScreen::new("detail", "Detail").pushed())
                .screen(ProjectScreen::new("items", "Items").row_kind("item")),
        )
    }

    fn open(&self, input: Input, cx: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        self.seen.lock().unwrap().push((
            self.id.into(),
            input,
            cx.settings.clone(),
            cx.lang.into(),
            cx.term,
        ));
        if self.fail_open {
            return Err(Failure::external("owner refused"));
        }
        Ok(Box::new(ProjectSession))
    }
}

struct ProjectSession;
impl Session for ProjectSession {
    fn view(&mut self, _: &ViewReq, _: &ViewCx) -> Result<View, Failure> {
        Ok(View::Message(konoma_plugin::view::MessageView {
            lines: vec![],
        }))
    }
}

struct Services {
    fail: bool,
    checked: Mutex<Vec<String>>,
}

impl ProjectServices for Services {
    fn inspect(
        &self,
        project: &MatchedProject,
        _: &ProjectSettings,
    ) -> Result<ProjectServiceInfo, Failure> {
        self.checked.lock().unwrap().push(project.plugin.clone());
        if self.fail {
            return Err(Failure::external("backend unavailable"));
        }
        Ok(ProjectServiceInfo {
            workdir: project.root.clone(),
            backend: ProjectBackend::Git,
        })
    }

    fn file_diff(&self, _: &ProjectServiceInfo, _: &Path) -> Vec<DiffLine> {
        vec![]
    }
    fn changed_files(&self, _: &ProjectServiceInfo) -> Vec<ChangeEntry> {
        vec![]
    }
    fn base_contents(&self, _: &ProjectServiceInfo, _: &Path) -> Option<Vec<u8>> {
        None
    }
}

fn directory() -> TmpDir {
    let dir = unique_tmp("app_project_prepare");
    std::fs::create_dir_all(&dir).unwrap();
    dir.into_canonical().unwrap()
}

fn plugin(id: &'static str, seen: &OpenLog, fail_open: bool) -> Arc<dyn Plugin> {
    Arc::new(ProjectPlugin {
        id,
        seen: seen.clone(),
        fail_open,
        remove_on_manifest: None,
    })
}

fn services(fail: bool) -> Arc<Services> {
    Arc::new(Services {
        fail,
        checked: Mutex::new(vec![]),
    })
}

#[test]
fn prepare_projects_rejects_directory_and_project_errors_before_terminal() {
    let dir = directory();
    let seen = OpenLog::default();
    let service = services(false);
    let config_error = "config error (using defaults): broken";
    let file = dir.join("file");
    std::fs::write(&file, "plain file").unwrap();
    for (path, kind) in [
        (dir.join("missing"), 0),
        (file.join("child"), 1),
        (file, 1),
        (dir.to_path_buf(), 2),
    ] {
        let failure = prepare_projects(
            vec![plugin("one", &seen, false)],
            &path,
            TermCaps::new(),
            (SharedConfig::default(), Some(config_error.into())),
            service.clone(),
        )
        .err()
        .expect("refused before any terminal is available");
        let mut report = Vec::new();
        let error = failure.report(&mut report);
        assert_eq!(report, format!("{config_error}\n").as_bytes());
        assert_eq!(error.exit_code(), 2);
        assert!(
            match kind {
                0 => matches!(error, AppError::NoDirectory(_)),
                1 => matches!(error, AppError::NotADirectory(_)),
                _ => matches!(error, AppError::NoProject(_)),
            },
            "{error}"
        );
    }
    assert!(seen.lock().unwrap().is_empty());
    assert!(service.checked.lock().unwrap().is_empty());

    std::fs::write(dir.join("project.marker"), "").unwrap();
    for (fail_service, fail_open) in [(true, false), (false, true)] {
        let failure = prepare_projects(
            vec![plugin("one", &seen, fail_open)],
            &dir,
            TermCaps::new(),
            (SharedConfig::default(), None),
            services(fail_service),
        )
        .err()
        .expect("backend and owner failures precede the terminal");
        assert_eq!(failure.error.exit_code(), 1);
        assert!(matches!(failure.error, AppError::ProjectOpen(_)));
        let mut report = Vec::new();
        failure.report(&mut report);
        assert!(report.is_empty());
    }
    assert_eq!(
        seen.lock().unwrap().len(),
        1,
        "availability gates owner open"
    );

    // Remove the directory during registration, after metadata and before discovery. This is
    // ordered on the calling thread, so no permission assumptions or timing races are needed.
    let removed = directory();
    let failure = prepare_projects(
        vec![Arc::new(ProjectPlugin {
            id: "removed",
            seen,
            fail_open: false,
            remove_on_manifest: Some(removed.to_path_buf()),
        })],
        &removed,
        TermCaps::new(),
        (SharedConfig::default(), None),
        service,
    )
    .err()
    .expect("discovery cannot read the removed directory");
    assert!(matches!(failure.error, AppError::ProjectDetection(_)));
    assert_eq!(failure.error.exit_code(), 1);
}

#[test]
fn prepare_projects_preserves_settings_disabled_and_multiple_matches() {
    use konoma_plugin::host::project::{ProjectDialog, ProjectKey};
    use konoma_plugin::ColorDepth;

    let dir = directory();
    std::fs::write(dir.join("project.marker"), "").unwrap();
    let seen = OpenLog::default();
    let service = services(false);
    let config = SharedConfig::parse(
        "[ui]\nlang = 'jp'\npath_style = 'home'\nmd_alerts = false\n\
         [git]\ndiff = 'split'\ntool = 'gitui'\n\
         [plugins.disabled]\nenabled = false\n\
         [plugins.markdown]\nemoji = false\nread_only = true\n\
         [plugins.second]\npath_style = 'full'\ncustom = 42\n",
    )
    .unwrap();
    let term = TermCaps::new().colors(ColorDepth::TrueColor);
    let prepared = prepare_projects(
        vec![
            plugin("disabled", &seen, true),
            plugin("markdown", &seen, false),
            plugin("second", &seen, false),
        ],
        &dir,
        term,
        (
            config,
            Some("config error (using defaults): fixture".into()),
        ),
        service.clone(),
    )
    .unwrap_or_else(|f| panic!("{}", f.error));
    assert_eq!(prepared.term, term);
    assert_eq!(prepared.open_dir, dir.as_path());
    assert_eq!(prepared.lang, "ja");
    assert_eq!(prepared.config.ui.path_style, "home");
    assert_eq!(
        prepared.display,
        konoma_plugin::app::Display::of(&prepared.config)
    );
    assert_eq!(
        prepared.settings,
        ProjectSettings::from_config(&prepared.config)
    );
    assert_eq!(prepared.settings.tool_command(false).0, "gitui");
    assert_eq!(
        prepared.load_error.as_deref(),
        Some("config error (using defaults): fixture")
    );
    assert!(!prepared.registry.is_enabled("disabled"));
    assert!(
        prepared.screen.is_none(),
        "multiple matches must remain a chooser"
    );
    let Some(ProjectDialog::Projects { projects, selected }) = &prepared.dialog else {
        panic!("all candidates must reach the shared chooser");
    };
    assert_eq!(*selected, 0);
    assert_eq!(
        projects
            .iter()
            .map(|p| p.plugin.as_str())
            .collect::<Vec<_>>(),
        ["markdown", "second"]
    );
    assert_eq!(prepared.detection.projects.len(), 2);
    assert_eq!(prepared.runtimes.len(), 2);
    assert_eq!(prepared.service_info.len(), 2);
    assert_eq!(*service.checked.lock().unwrap(), ["markdown", "second"]);
    let opened = seen.lock().unwrap();
    assert_eq!(opened.len(), 2, "both owners opened before returning");
    for (id, input, settings, lang, got_term) in opened.iter() {
        assert_eq!(lang, "ja");
        assert_eq!(*got_term, term);
        let Input::Project { root, marker } = input else {
            panic!("project input required")
        };
        assert_eq!(root, dir.as_path());
        assert_eq!(marker, "project.marker");
        assert!(settings.get("enabled").is_none());
        assert!(settings.get("read_only").is_none());
        if id == "markdown" {
            assert!(!settings.bool("emoji", true));
            assert!(!settings.bool("alerts", true));
            assert!(settings.get("footnotes").is_some());
            assert_eq!(settings.str("path_style", ""), "home");
        } else {
            assert_eq!(settings.get("custom"), Some(&Value::Int(42)));
            assert_eq!(settings.str("path_style", ""), "full");
        }
    }
    drop(opened);
    for project in &prepared.detection.projects {
        let key = ProjectKey::from(project);
        assert_eq!(prepared.service_info[&key].workdir, dir.as_path());
        assert!(
            prepared.runtimes[&key].pending.is_some(),
            "the initial owner reply is retained for shared adoption"
        );
    }
    let one = prepare_projects(
        vec![plugin("one", &seen, false)],
        &dir,
        term,
        (SharedConfig::default(), None),
        services(false),
    )
    .unwrap_or_else(|f| panic!("{}", f.error));
    assert!(one.dialog.is_none());
    let screen = one
        .screen
        .as_ref()
        .expect("one project opens its first ordinary screen");
    assert_eq!(screen.project.plugin, "one");
    assert_eq!(
        screen.frame().unwrap().key.screen,
        "items",
        "skip pushed declarations"
    );
    assert!(one.load_error.is_none());
}

#[test]
fn project_service_distinguishes_empty_repo_from_unavailable_backend() {
    // Consumers can implement the service without depending on the host package directly.
    use konoma_plugin::app::project::ProjectSettings;
    let dir = directory();
    std::fs::write(dir.join("project.marker"), "").unwrap();
    let seen = OpenLog::default();
    let service = services(false);
    let prepared = prepare_projects(
        vec![plugin("empty", &seen, false)],
        &dir,
        TermCaps::new(),
        (SharedConfig::default(), None),
        service.clone(),
    )
    .unwrap_or_else(|f| panic!("an empty changed-file set is normal: {}", f.error));
    let info = prepared.service_info.values().next().unwrap();
    assert!(service.changed_files(info).is_empty());
    assert!(service.file_diff(info, &dir.join("absent")).is_empty());
    assert!(service.base_contents(info, &dir.join("absent")).is_none());
    let settings = ProjectSettings::from_config(&prepared.config);
    assert!(services(true)
        .inspect(&prepared.detection.projects[0], &settings)
        .is_err());
    let failure = prepare_projects(
        vec![plugin("empty", &seen, false)],
        &dir,
        TermCaps::new(),
        (SharedConfig::default(), None),
        services(true),
    )
    .err()
    .expect("unavailable backend is refused");
    assert!(matches!(failure.error, AppError::ProjectOpen(_)));
    assert_eq!(failure.error.exit_code(), 1);
    assert_eq!(
        seen.lock().unwrap().len(),
        1,
        "unavailable backend never opens an owner"
    );
}

#[cfg(unix)]
#[test]
fn file_entry_refuses_before_terminal_initialisation() {
    use std::os::unix::process::CommandExt;
    const CHILD: &str = "KONOMA_APP_TERMINAL_ORDER_CHILD";
    if std::env::var_os(CHILD).is_some() {
        let dir = directory();
        let error = konoma_plugin::app::run_bundled(vec![], vec![], &dir.join("missing"))
            .expect_err("the file must be refused");
        assert!(
            matches!(error, AppError::NoFile(_)),
            "file refusal must precede terminal entry: {error}"
        );
        assert_eq!(error.exit_code(), 2);
        return;
    }
    let config = directory();
    let mut child = std::process::Command::new(std::env::current_exe().unwrap());
    child
        .args([
            "--exact",
            "project::prepare::file_entry_refuses_before_terminal_initialisation",
            "--nocapture",
        ])
        .env(CHILD, "1")
        .env("HOME", config.as_path())
        .env("XDG_CONFIG_HOME", config.as_path())
        .stdin(std::process::Stdio::null());
    // SAFETY: setsid uses no allocation or locks after fork. Detaching the child makes an
    // attempted raw-mode entry fail independently of the caller's controlling terminal.
    unsafe {
        child.pre_exec(|| {
            if libc::setsid() == -1 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(())
            }
        });
    }
    let out = child.output().unwrap();
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("1 passed"));
    assert!(
        !out.stdout.contains(&b'\x1b'),
        "the entry must not write terminal escapes on refusal"
    );
}
