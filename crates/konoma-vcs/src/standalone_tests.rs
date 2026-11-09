use std::sync::Arc;

#[cfg(feature = "git")]
use konoma_plugin::app::project::ProjectBackend;
use konoma_plugin::app::project::{prepare_projects, ProjectServices, ProjectSettings};
use konoma_plugin::host::project::{detect, MatchedProject};
use konoma_plugin::{Plugin, TermCaps};
use konoma_test_support::{unique_tmp, TmpDir};

use super::standalone::VcsServices;

fn directory() -> TmpDir {
    let dir = unique_tmp("vcs_standalone");
    std::fs::create_dir_all(&dir).unwrap();
    dir.into_canonical().unwrap()
}

fn project(dir: &std::path::Path, plugin: &dyn Plugin) -> MatchedProject {
    let declaration = plugin.manifest().project.unwrap();
    MatchedProject {
        plugin: plugin.manifest().id,
        root: dir.to_path_buf(),
        marker: declaration.markers[0].name.clone(),
        declaration,
        session_watch_paths: None,
    }
}

// Follow the existing VCS tests: git init is local and does not read a real global config.
#[cfg(feature = "git")]
fn init(dir: &std::path::Path) {
    let out = std::process::Command::new("git")
        .current_dir(dir)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .args(["init", "-q"])
        .output()
        .unwrap();
    assert!(out.status.success(), "git init failed");
}

#[test]
fn project_service_distinguishes_empty_repo_from_unavailable_backend() {
    let settings = ProjectSettings::from_config(&Default::default());
    #[cfg(feature = "git")]
    {
        let dir = directory();
        init(&dir);
        let matched = project(&dir, &crate::Git);
        let info = VcsServices
            .inspect(&matched, &settings)
            .expect("an unborn repo is usable");
        assert_eq!(info.workdir, dir.as_path());
        assert_eq!(info.backend, ProjectBackend::Git);
        assert!(VcsServices.changed_files(&info).is_empty());
        assert!(VcsServices
            .file_diff(&info, &dir.join("missing"))
            .is_empty());
        assert!(VcsServices
            .base_contents(&info, &dir.join("missing"))
            .is_none());
        let prepared = prepare_projects(
            vec![Arc::new(crate::Git)],
            &dir,
            TermCaps::new(),
            (Default::default(), None),
            Arc::new(VcsServices),
        )
        .unwrap_or_else(|f| panic!("{}", f.error));
        assert_eq!(prepared.runtimes.len(), 1);
        assert_eq!(prepared.screen.as_ref().unwrap().project.plugin, "git");
        let mut disabled = settings.clone();
        disabled.git_enabled = false;
        assert!(VcsServices.inspect(&matched, &disabled).is_err());

        std::fs::write(dir.join("new.txt"), "new\n").unwrap();
        let changed = VcsServices.changed_files(&info);
        assert_eq!(changed.len(), 1);
        assert_eq!(changed[0].path, dir.join("new.txt"));
        assert!(!VcsServices
            .file_diff(&info, &dir.join("new.txt"))
            .is_empty());
    }
    let invalid = directory();
    {
        std::fs::create_dir(invalid.join(".git")).unwrap();
        let matched = project(&invalid, &crate::Git);
        assert!(
            VcsServices.inspect(&matched, &settings).is_err(),
            "a marker is not a repository"
        );
        let mut registry = konoma_plugin::registry::Registry::new();
        let owned: Arc<dyn Plugin> = Arc::new(crate::Git);
        registry
            .register(
                owned.clone(),
                konoma_plugin::registry::PluginOrigin::Standard,
            )
            .unwrap();
        let detected = detect(
            &invalid,
            &konoma_plugin::host::project::detection::project_declarations(&registry, |_| true),
            &std::sync::atomic::AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(
            detected.len(),
            1,
            "the invalid marker still matches discovery"
        );
        let failure = prepare_projects(
            vec![owned],
            &invalid,
            TermCaps::new(),
            (Default::default(), None),
            Arc::new(VcsServices),
        )
        .err()
        .expect("invalid marker refused before the terminal");
        assert!(matches!(
            failure.error,
            konoma_plugin::app::AppError::ProjectOpen(_)
        ));
        assert_eq!(failure.error.exit_code(), 1);
    }
}

#[cfg(all(unix, feature = "git"))]
#[test]
fn jj_service_requires_a_successful_read_only_query_and_an_available_backend() {
    use std::os::unix::fs::PermissionsExt;
    const CHILD: &str = "KONOMA_VCS_SERVICE_CHILD";
    if let Ok(mode) = std::env::var(CHILD) {
        let dir = directory();
        std::fs::create_dir(dir.join(".jj")).unwrap();
        let matched = project(&dir, &crate::Jj);
        let settings = ProjectSettings::from_config(&Default::default());
        assert!(
            VcsServices.inspect(&matched, &settings).is_err(),
            "invalid marker"
        );
        std::fs::write(dir.join(".jj/valid"), "").unwrap();
        if mode == "missing" {
            assert!(VcsServices.inspect(&matched, &settings).is_err());
        } else {
            let info = VcsServices
                .inspect(&matched, &settings)
                .expect("the query succeeded");
            assert_eq!(info.backend, ProjectBackend::Jj);
            assert_eq!(info.workdir, dir.as_path());
            let log =
                std::fs::read_to_string(std::env::var("KONOMA_VCS_QUERY_LOG").unwrap()).unwrap();
            let queries: Vec<_> = log.lines().filter(|s| *s != "--version").collect();
            assert_eq!(
                queries.len(),
                2,
                "both marker and valid workspace were queried"
            );
            assert!(queries
                .iter()
                .all(|q| q.starts_with("log -r @ --no-graph -T ")
                    && q.ends_with(" --ignore-working-copy")));
        }
        return;
    }
    let dir = directory();
    let bin = dir.join("bin");
    let empty = dir.join("empty");
    std::fs::create_dir(&bin).unwrap();
    std::fs::create_dir(&empty).unwrap();
    let jj = bin.join("jj");
    std::fs::write(
        &jj,
        r#"#!/bin/sh
printf '%s\n' "$*" >> "$KONOMA_VCS_QUERY_LOG"
[ "$1" = '--version' ] && exit 0
[ -f .jj/valid ] || exit 1
case "$*" in
  'log -r @ --no-graph -T '*' --ignore-working-copy') printf 'change\t0\tparent\tsummary\n';;
  *) exit 1;;
esac
"#,
    )
    .unwrap();
    std::fs::set_permissions(&jj, std::fs::Permissions::from_mode(0o755)).unwrap();
    for (mode, path) in [("available", &bin), ("missing", &empty)] {
        let out = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "standalone_tests::jj_service_requires_a_successful_read_only_query_and_an_available_backend", "--nocapture"])
            .env(CHILD, mode)
            .env("PATH", path)
            .env("HOME", dir.as_path())
            .env("XDG_CONFIG_HOME", dir.as_path())
            .env("KONOMA_VCS_QUERY_LOG", dir.join("queries"))
            .output().unwrap();
        assert!(
            out.status.success(),
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(String::from_utf8_lossy(&out.stdout).contains("1 passed"));
    }
}
