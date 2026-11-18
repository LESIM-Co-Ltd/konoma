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

#[cfg(feature = "git")]
#[test]
fn standalone_selected_backend_reaches_diff_workers() {
    use konoma_plugin::app::diff_screen::{DiffScreenState, FileDiffRequest};
    use konoma_plugin::app::project::ProjectServiceInfo;
    use konoma_plugin::host::project::md_diff::{MdDiffOutcome, MdDiffWorker};
    use konoma_plugin::host::project::media_diff::{
        MediaBase, MediaDiffComputed, MediaDiffSideDecoded, MediaDiffWorker,
    };
    use konoma_vcs_core::git2;
    use std::sync::mpsc;
    use std::time::Duration;

    if !konoma_vcs_core::vcs::jj::available() {
        return;
    }
    const CHILD: &str = "KONOMA_STANDALONE_BACKEND_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let home = directory();
        let out = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "standalone_tests::standalone_selected_backend_reaches_diff_workers",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .env("HOME", home.as_path())
            .env("XDG_CONFIG_HOME", home.as_path())
            .env("JJ_CONFIG", "")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(String::from_utf8_lossy(&out.stdout).contains("1 passed"));
        return;
    }
    let dir = directory();
    let repo = git2::Repository::init(&dir).unwrap();
    let md = dir.join("a.md");
    let svg = dir.join("b.svg");
    const GIT_SVG: &str =
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"1\" height=\"1\"><!-- HEAD --></svg>";
    const JJ_SVG: &str =
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"1\" height=\"1\"><!-- parent --></svg>";
    let commit = |text: &str, picture: &str| {
        std::fs::write(&md, text).unwrap();
        std::fs::write(&svg, picture).unwrap();
        let mut index = repo.index().unwrap();
        for path in ["a.md", "b.svg"] {
            index.add_path(std::path::Path::new(path)).unwrap();
        }
        index.write().unwrap();
        let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
        let sig =
            git2::Signature::new("fixture", "fixture@example.invalid", &git2::Time::new(0, 0))
                .unwrap();
        let parent = repo.head().ok().and_then(|h| h.peel_to_commit().ok());
        repo.commit(
            Some("HEAD"),
            &sig,
            &sig,
            "fixture",
            &tree,
            &parent.iter().collect::<Vec<_>>(),
        )
        .unwrap();
    };
    commit("# jj parent\n", JJ_SVG);
    let output = std::process::Command::new("jj")
        .current_dir(&dir)
        .args(["git", "init", "--colocate", "."])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "jj init: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    commit("# git head\n", GIT_SVG);
    std::fs::write(&md, "# working\n").unwrap();
    std::fs::write(
        &svg,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"2\" height=\"2\"/>",
    )
    .unwrap();
    let config = Default::default();
    let layout = ProjectSettings::from_config(&config).diff_layout;
    let wait = Duration::from_secs(30);
    let mut checked = 0;
    for (backend, text, picture, base) in [
        (
            ProjectBackend::Git,
            "# git head\n",
            GIT_SVG,
            MediaBase::Head,
        ),
        (
            ProjectBackend::Jj,
            "# jj parent\n",
            JJ_SVG,
            MediaBase::JjParent,
        ),
    ] {
        let info = ProjectServiceInfo {
            workdir: dir.to_path_buf(),
            backend,
        };
        let raw = VcsServices.file_diff(&info, &md);
        assert!(
            raw.iter().any(|l| l.text == text.trim_end()),
            "selected standalone raw baseline: {raw:?}"
        );
        let request = |path| FileDiffRequest {
            root: dir.to_path_buf(),
            path,
            info: info.clone(),
        };
        let (tx, results) = mpsc::channel();
        let (_unused_tx, unused_rx) = mpsc::channel();
        let mut markdown = MdDiffWorker::default();
        markdown.attach(tx);
        let _screen = DiffScreenState::open_with_markdown_worker(
            request(md.clone()),
            Arc::new(VcsServices),
            &config,
            layout,
            None,
            markdown,
            unused_rx,
        )
        .unwrap();
        let result = results.recv_timeout(wait).unwrap();
        let MdDiffOutcome::Ready { old_pre, .. } = result.outcome else {
            panic!("expected standalone Markdown")
        };
        assert_eq!(old_pre, text, "selected standalone Rendered baseline");

        let (tx, results) = mpsc::channel();
        let (_unused_tx, unused_rx) = mpsc::channel();
        let mut media = MediaDiffWorker::default();
        media.attach(tx);
        let mut screen = DiffScreenState::open(
            request(svg.clone()),
            Arc::new(VcsServices),
            &config,
            layout,
            None,
        )
        .unwrap()
        .with_media_worker(media, unused_rx);
        screen.prepare_frame(Default::default(), None);
        let result = results.recv_timeout(wait).unwrap();
        let MediaDiffComputed::Ready {
            base: actual_base,
            old: MediaDiffSideDecoded::Picture(old),
            ..
        } = result.computed
        else {
            panic!("expected standalone media")
        };
        assert_eq!(actual_base, base);
        assert_eq!(
            old.svg.as_deref().unwrap().as_slice(),
            picture.as_bytes(),
            "selected standalone media bytes"
        );
        checked += 3;
    }
    assert!(checked >= 6, "both standalone backends must run");
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
