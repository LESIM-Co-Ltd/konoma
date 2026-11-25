//! Discovery of the vcs plugins' projects with konoma's own readers (the discovery and the event
//! fences themselves are tested in the SDK, `konoma_plugin::host::project`).

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
#[cfg(feature = "git")]
use std::sync::Arc;

use konoma_plugin::host::project::{affected, detect_directories, ProjectDeclaration};
#[cfg(feature = "git")]
use konoma_plugin::host::project::{detect, watch_targets};
use konoma_plugin::Plugin;

use crate::test_support::unique_tmp;

fn vcs_declarations() -> Vec<ProjectDeclaration> {
    [&konoma_vcs::Git as &dyn Plugin, &konoma_vcs::Jj]
        .into_iter()
        .map(|plugin| {
            let manifest = plugin.manifest();
            ProjectDeclaration {
                plugin: manifest.id,
                project: manifest.project.unwrap(),
            }
        })
        .collect()
}

#[test]
fn vcs_projects_are_registered_as_standard_plugins_when_the_backend_is_available() {
    let registry = crate::plugins::build_registry(&[]);
    for id in ["git", "jj"] {
        assert_eq!(
            registry.manifest(id).is_some(),
            cfg!(feature = "git"),
            "{id}"
        );
    }
}

#[test]
fn standard_vcs_registration_respects_disabled_replacements_and_backend_gates() {
    use crate::plugins::{build_registry_with, statics::StaticSet};
    use konoma_plugin::registry::PluginOrigin;
    use std::collections::BTreeMap;
    use std::sync::Arc;

    let manifest: toml::Table = include_str!("../../Cargo.toml").parse().unwrap();
    let normal = &manifest["dependencies"]["konoma-vcs"];
    assert_eq!(normal["default-features"].as_bool(), Some(false));
    assert!(
        normal.get("features").is_none_or(|features| !features
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f.as_str() == Some("app"))),
        "konoma's normal dependency must not enable app"
    );
    assert!(manifest["dev-dependencies"]["konoma-vcs"]["features"]
        .as_array()
        .unwrap()
        .iter()
        .any(|f| f.as_str() == Some("app")));
    let features = manifest["features"].as_table().unwrap();
    let mut pending = vec!["default"];
    let mut visited = std::collections::BTreeSet::new();
    while let Some(feature) = pending.pop() {
        assert!(
            !matches!(feature, "konoma-vcs/app" | "konoma-vcs/default"),
            "normal feature graph must not enable the standalone app"
        );
        if visited.insert(feature) {
            if let Some(children) = features.get(feature) {
                pending.extend(
                    children
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|v| v.as_str().unwrap()),
                );
            }
        }
    }

    let plain = build_registry_with(&[], None);
    let linked = StaticSet {
        standard: false,
        added: Vec::new(),
        replaced: BTreeMap::new(),
    };
    let without = build_registry_with(&[], Some(&linked));
    for id in ["git", "jj"] {
        assert!(
            without.manifest(id).is_none(),
            "standard=false must drop {id}"
        );
        assert_eq!(
            plain.origin(id),
            cfg!(feature = "git").then_some(PluginOrigin::Standard),
            "{id}"
        );
        assert_eq!(
            super::declarations(&[]).iter().any(|d| d.plugin == id),
            cfg!(feature = "git")
        );
        assert!(!crate::plugins::build_registry(&[id.into()]).is_enabled(id));
    }
    let replaced = StaticSet {
        standard: true,
        added: Vec::new(),
        replaced: BTreeMap::from([
            ("git".into(), Arc::new(konoma_vcs::Git) as Arc<dyn Plugin>),
            ("jj".into(), Arc::new(konoma_vcs::Jj) as Arc<dyn Plugin>),
        ]),
    };
    let registry = build_registry_with(&[], Some(&replaced));
    for id in ["git", "jj"] {
        assert_eq!(
            registry.origin(id),
            cfg!(feature = "git").then_some(PluginOrigin::User),
            "replacement {id}"
        );
        assert_eq!(
            registry.ids().iter().position(|p| *p == id),
            plain.ids().iter().position(|p| *p == id)
        );
        assert!(!build_registry_with(&[id.into()], Some(&replaced)).is_enabled(id));
    }
}

#[test]
fn vcs_project_detection_finds_both_colocated_roots_from_a_descendant() {
    let dir = unique_tmp("vcs_detection_colocated");
    std::fs::create_dir_all(dir.join(".git")).unwrap();
    std::fs::create_dir_all(dir.join(".jj")).unwrap();
    std::fs::create_dir_all(dir.join("nested/deep")).unwrap();
    let root = dir.canonicalize().unwrap();
    let found = detect_directories(
        [root.join("nested/deep"), root.join("nested"), root.clone()]
            .iter()
            .map(PathBuf::as_path),
        &vcs_declarations(),
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(
        found
            .iter()
            .map(|p| (p.plugin.as_str(), &p.root))
            .collect::<Vec<_>>(),
        [("git", &root), ("jj", &root)]
    );
    for (marker, expected) in [
        (".git/HEAD", true),
        (".git/index", true),
        (".git/info/exclude", true),
        (".jj/repo/op_heads/new-op", true),
        ("nested/file.txt", true),
    ] {
        assert_eq!(
            affected(&root, &vcs_declarations(), &found, &[root.join(marker)]),
            expected,
            "{marker}"
        );
    }
}

#[test]
fn vcs_project_detection_accepts_gitdir_files_and_rejects_a_jj_file() {
    let dir = unique_tmp("vcs_detection_files");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(".git"), "gitdir: metadata\n").unwrap();
    std::fs::write(dir.join(".jj"), "not a directory\n").unwrap();
    let found = detect_directories(
        [dir.as_path()],
        &vcs_declarations(),
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(
        found.iter().map(|p| p.plugin.as_str()).collect::<Vec<_>>(),
        ["git"]
    );
    assert_eq!(found[0].root, dir.as_path());
}

#[cfg(feature = "git")]
#[test]
fn vcs_project_watch_targets_follow_a_linked_worktrees_gitdir_file() {
    use crate::test_support::{init_git_repo, run_git};
    let main = unique_tmp("vcs_watch_main");
    std::fs::create_dir_all(&main).unwrap();
    init_git_repo(&main);
    std::fs::write(main.join("file.txt"), "base\n").unwrap();
    run_git(&main, &["add", "."]);
    run_git(&main, &["commit", "-qm", "seed"]);
    let linked = unique_tmp("vcs_watch_linked");
    run_git(
        &main,
        &[
            "worktree",
            "add",
            "-q",
            "--detach",
            linked.to_str().unwrap(),
        ],
    );
    let root = linked.canonicalize().unwrap();
    let found = detect(&root, &vcs_declarations(), &AtomicBool::new(false)).unwrap();
    let project = found
        .iter()
        .find(|p| p.plugin == "git" && p.root == root)
        .unwrap();
    assert_eq!(Some(&project.root), crate::git::workdir(&root).as_ref());
    let old = crate::app::App::new(root.clone(), crate::config::Config::default()).unwrap();
    let metadata = old
        .git_dir_watch()
        .expect("legacy host watches external gitdir");
    let marker = crate::plugins::test_plugins::register_replacement(
        "vcs_watch_linked",
        Arc::new(konoma_vcs::Git),
    );
    let mut host = crate::app::App::new(
        root.clone(),
        crate::plugins::test_plugins::marker_config(&marker),
    )
    .unwrap();
    host.tab.projects.wait_for_result();
    host.poll_projects();
    for _ in 0..32 {
        if !host.project_jobs_pending() {
            break;
        }
        host.wait_project_job();
    }
    let targets = watch_targets(&root, &host.tab.projects.projects);
    assert!(
        targets.iter().any(
            |(path, recursive)| path == &metadata || (*recursive && metadata.starts_with(path))
        ),
        "project watches must cover the legacy host's external gitdir"
    );
}

#[cfg(feature = "git")]
#[test]
fn vcs_project_detection_root_matches_the_existing_git_and_jj_readers() {
    use crate::test_support::{init_git_repo, jj_scratch_bare};
    let git = unique_tmp("vcs_detection_git_reader");
    std::fs::create_dir_all(git.join("nested/deep")).unwrap();
    init_git_repo(&git);
    let root = git.canonicalize().unwrap();
    let found = detect(
        &root.join("nested/deep"),
        &vcs_declarations(),
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!(found.iter().any(|p| p.plugin == "git"
        && Some(&p.root) == crate::git::workdir(&root.join("nested/deep")).as_ref()));
    if let Some(jj) = jj_scratch_bare("vcs_detection_jj_reader") {
        std::fs::create_dir_all(jj.join("nested/deep")).unwrap();
        let root = jj.canonicalize().unwrap();
        let found = detect(
            &root.join("nested/deep"),
            &vcs_declarations(),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert!(found.iter().any(|p| p.plugin == "jj"
            && Some(&p.root)
                == crate::vcs::jj::workspace_root(&root.join("nested/deep")).as_ref()));
    }
}
