//! The parts of project hosting available without inventing missing SDK request fields.

use super::*;
use crate::i18n::Msg;
use crate::plugins::test_plugins::{self, ProjectFixture};
use konoma_plugin::project::{DetectionMarker, MarkerKind, ProjectDecl, ProjectScreen};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

fn config(name: &str, read_only: bool) -> (Config, Arc<AtomicUsize>) {
    navigation_config(name, read_only, None)
}

fn navigation_config(
    name: &str,
    read_only: bool,
    navigation: Option<konoma_plugin::Outcome>,
) -> (Config, Arc<AtomicUsize>) {
    let calls = Arc::new(AtomicUsize::new(0));
    let marker = test_plugins::register(
        name,
        Arc::new(ProjectFixture {
            id: "e2e-project",
            project: ProjectDecl::new()
                .marker(DetectionMarker::new("host-project.marker", MarkerKind::File))
                .screen(ProjectScreen::new("items", "Items").key('i').row_kind("item"))
                .screen(ProjectScreen::new("text", "Text").key('z'))
                .watch("metadata/index"),
            calls: calls.clone(),
            navigation,
        }),
    );
    let mut cfg = test_plugins::marker_config(&marker);
    cfg.ui.lang = "en".into();
    cfg.external.git = false;
    cfg.plugins.insert(
        "e2e-project".into(),
        toml::Table::from_iter([("read_only".into(), read_only.into())]),
    );
    (cfg, calls)
}

fn land_projects(s: &mut Sim) {
    s.app.tab.projects.wait_for_result();
    assert!(s.app.poll_projects());
    assert!(!s.app.tab.projects.projects.is_empty(), "{:?}", s.app.flash);
    s.draw();
}

#[test]
fn e2e_project_detection_follows_root_keys_and_stays_with_its_tab() {
    let dir = sandbox("e2e_project_detection_root_tabs");
    let nested = dir.join("nested");
    std::fs::create_dir_all(&nested).unwrap();
    std::fs::write(dir.join("host-project.marker"), "outer").unwrap();
    std::fs::write(nested.join("host-project.marker"), "inner").unwrap();
    let (cfg, _) = config("e2e_project_detection_root_tabs", false);
    let mut s = Sim::with_config(&canon(&dir), cfg);
    land_projects(&mut s);
    assert_eq!(s.app.tab.projects.projects.len(), 1);
    s.select("nested");
    s.key('l');
    land_projects(&mut s);
    assert_eq!(s.app.tab.projects.projects.len(), 2);
    assert_eq!(s.app.tab.projects.projects[0].root, canon(&nested));
    let source_projects = s.app.tab.projects.clone();
    s.key('t');
    assert!(!s.app.tab.projects.shares_worker(&source_projects));
    land_projects(&mut s);
    assert_eq!(s.app.tab.projects.projects.len(), 2);
    s.key('h');
    land_projects(&mut s);
    assert_eq!(s.app.tab.projects.projects.len(), 1);
    s.key('[');
    assert_eq!(s.app.tab.root, canon(&nested));
    assert_eq!(s.app.tab.projects.projects.len(), 2);
    s.key(']');
    assert_eq!(s.app.tab.root, canon(&dir));
    assert_eq!(s.app.tab.projects.projects.len(), 1);
}

#[test]
fn e2e_project_plugin_write_uses_the_guard_and_hides_its_hint() {
    for read_only in [true, false] {
        let name = if read_only {
            "e2e_project_read_only"
        } else {
            "e2e_project_writable"
        };
        let dir = sandbox(name);
        std::fs::write(dir.join("view.project-test"), "project").unwrap();
        let (cfg, calls) = config(name, read_only);
        let mut s = Sim::with_config(&canon(&dir), cfg);
        s.select("view.project-test");
        s.enter();
        if read_only {
            s.dont_see("w:save project");
        } else {
            s.see("w:save project");
        }
        s.key('w');
        assert_eq!(calls.load(Ordering::SeqCst), usize::from(!read_only));
        assert_eq!(
            s.app.flash.as_deref(),
            Some(if read_only {
                crate::i18n::tr(s.app.lang, Msg::PluginReadOnly)
            } else {
                "saved project"
            })
        );
    }
}

#[cfg(feature = "git")]
#[test]
fn e2e_git_read_only_stops_checkout_enter_before_starting_a_job() {
    let dir = sandbox("e2e_git_read_only_checkout_enter");
    seed_repo(&dir);
    let mut cfg = cfg_en();
    cfg.plugins.insert(
        "git".into(),
        toml::Table::from_iter([("read_only".into(), true.into())]),
    );
    let mut s = Sim::with_config(&canon(&dir), cfg).with_async_git_ops();
    s.key('p');
    s.key('b');
    assert!(s.app.is_git_branches());
    let want = s
        .app
        .git_branch_view()
        .iter()
        .position(|b| b.name == "feature-x")
        .unwrap();
    while s.app.git_branch_sel() != want {
        s.key(if s.app.git_branch_sel() > want { 'k' } else { 'j' });
    }
    s.dont_see("checkout");
    s.enter();
    assert_eq!(
        s.app.flash.as_deref(),
        Some(crate::i18n::tr(s.app.lang, Msg::PluginReadOnly))
    );
    assert!(!s.app.git_op_running());
    assert!(s.app.is_git_branches());
    assert!(s.app.busy_jobs().is_empty());
}

fn navigation_session(
    name: &str,
    asynchronous: bool,
    outcome: impl FnOnce(&std::path::Path) -> konoma_plugin::Outcome,
) -> (Sim, crate::test_support::TmpDir, std::path::PathBuf) {
    let name = format!("{name}-{}", usize::from(asynchronous));
    let dir = sandbox(&name);
    std::fs::create_dir_all(dir.join("destination")).unwrap();
    std::fs::write(dir.join("view.project-test"), "source").unwrap();
    std::fs::write(dir.join("destination/file.txt"), "one\ntwo\nthree\nfour\n").unwrap();
    std::fs::write(dir.join("host-project.marker"), "project").unwrap();
    let target = canon(&dir).join("destination/file.txt");
    let (cfg, _) = navigation_config(&name, false, Some(outcome(&target)));
    let mut s = Sim::with_config(&canon(&dir), cfg);
    if asynchronous {
        s = s.with_async_plugins();
    }
    s.select("view.project-test");
    s.enter();
    if asynchronous && s.app.is_media_loading() {
        assert!(s.drain_plugin());
    }
    let source = s.app.tab.preview_path.clone();
    s.key('x');
    // An action answer can land inside its wait or arrive on the channel. Wait only while the
    // source is still shown, so the test does not depend on which side of that race won.
    if asynchronous && s.app.tab.preview_path == source {
        assert!(s.drain_plugin());
    }
    (s, dir, target)
}

#[test]
fn e2e_project_outcome_open_file_uses_link_preview_and_its_line() {
    for asynchronous in [false, true] {
        let (s, _dir, target) = navigation_session("project_outcome_file", asynchronous, |target| {
            konoma_plugin::Outcome::OpenFile {
                path: target.into(),
                line: Some(3),
            }
        });
        assert_eq!(s.app.tab.preview_path.as_ref(), Some(&target));
        assert_eq!(s.app.tab.mode, Mode::Preview);
        assert_eq!(s.app.preview_caret_line(), 2);
        s.dont_see("save project");
    }
}

#[test]
fn e2e_project_outcome_open_diff_uses_the_host_full_screen_diff() {
    for asynchronous in [false, true] {
        let (s, _dir, target) = navigation_session("project_outcome_diff", asynchronous, |target| {
            konoma_plugin::Outcome::OpenDiff {
                path: target.into(),
            }
        });
        assert!(
            matches!(&s.app.tab.preview_kind, Some(crate::preview::PreviewKind::GitDiff(path)) if *path == target)
        );
        assert_eq!(s.app.tab.mode, Mode::Preview);
    }
}

#[test]
fn e2e_project_outcome_switch_root_uses_the_worktree_root_switch() {
    for asynchronous in [false, true] {
        let (s, _dir, target) = navigation_session("project_outcome_root", asynchronous, |target| {
            konoma_plugin::Outcome::SwitchRoot {
                root: target.parent().unwrap().into(),
            }
        });
        assert_eq!(s.app.tab.root, target.parent().unwrap());
        assert_eq!(s.app.tab.open_dir, s.app.tab.root);
        assert_eq!(s.app.tab.mode, Mode::Tree);
        assert!(s.app.tab.preview_path.is_none());
        s.see("file.txt");
    }
}

#[test]
fn e2e_project_outcome_open_tab_uses_ctrl_t_for_files_and_directories() {
    for asynchronous in [false, true] {
        for directory in [false, true] {
            let name = format!("project_outcome_tab-{}", usize::from(directory));
            let (mut s, dir, target) = navigation_session(&name, asynchronous, |target| {
                konoma_plugin::Outcome::OpenTab {
                    path: if directory {
                        target.parent().unwrap().into()
                    } else {
                        target.into()
                    },
                }
            });
            assert_eq!(s.app.tab_count(), 2);
            if directory {
                assert_eq!(s.app.tab.mode, Mode::Tree);
                assert_eq!(s.app.tab.root, target.parent().unwrap());
            } else {
                assert_eq!(s.app.tab.mode, Mode::Preview);
                assert_eq!(s.app.tab.preview_path.as_ref(), Some(&target));
            }
            s.key('[');
            assert_eq!(
                s.app.tab.preview_path,
                Some(canon(&dir).join("view.project-test"))
            );
        }
    }
}
