//! Standard registration through real key dispatch, discovery and configuration loading.
use super::*;
use crate::i18n::{tr, Lang, Msg};
use crate::keymap::{Action, Context, LeaderId};

fn settle(sim: &mut Sim) {
    if sim.app.tab.projects.pending() {
        sim.app.tab.projects.wait_for_result();
    }
    for _ in 0..32 {
        sim.app.poll_projects();
        if !sim.app.project_jobs_pending() {
            sim.draw();
            return;
        }
        sim.app.wait_project_job();
    }
    panic!("standard VCS project did not settle");
}

fn assert_not_offered(sim: &Sim) {
    for ctx in [
        Context::Tree,
        Context::Text,
        Context::Document,
        Context::Table,
        Context::Image,
        Context::Canvas,
        Context::Diff,
    ] {
        assert!(!crate::ops::action_enabled(
            &sim.app,
            ctx,
            Action::OpenGitView
        ));
        for id in [LeaderId::Options, LeaderId::Actions] {
            for view in [crate::ops::menu::View::Open, crate::ops::menu::View::Help] {
                assert!(!crate::ops::menu::menu(&sim.app, ctx, id, view)
                    .is_some_and(|m| m.items.iter().any(|i| i.action == Action::OpenGitView)));
            }
        }
        assert!(!crate::ops::footer::tokens(&sim.app, ctx)
            .iter()
            .any(|t| t.starts_with("p:")));
        assert!(!crate::ops::help::sections_for(&sim.app, ctx)
            .iter()
            .flat_map(|s| &s.rows)
            .any(|(keys, _)| keys == "p"));
    }
}

#[test]
fn e2e_standard_vcs_disabled_and_no_backend_never_open_the_legacy_hub() {
    let dir = unique_tmp("standard_vcs_disabled");
    std::fs::create_dir_all(dir.join(".git")).unwrap();
    std::fs::create_dir_all(dir.join(".jj")).unwrap();
    std::fs::write(dir.join("file.txt"), "text\n").unwrap();
    let cfg: Config = toml::from_str(
        "[ui]\nlang = 'en'\n[plugins.git]\nenabled = false\n[plugins.jj]\nenabled = false\n",
    )
    .unwrap();
    let mut sim = Sim::with_config(&dir, cfg);
    settle(&mut sim);
    assert!(sim.app.tab.projects.projects.is_empty());
    for preview in [false, true] {
        if preview {
            sim.select("file.txt");
            sim.enter();
        }
        assert_not_offered(&sim);
        for menu in [None, Some(','), Some(' ')] {
            if let Some(key) = menu {
                sim.key(key);
            }
            sim.key('p');
            settle(&mut sim);
            assert!(!sim.app.project_active());
            assert!(sim.app.project_dialog.is_none());
            if menu.is_none() {
                assert_eq!(
                    sim.app.flash.as_deref(),
                    Some(tr(Lang::En, Msg::NotAGitRepo))
                );
            }
            sim.esc();
            if preview && sim.app.tab.mode != Mode::Preview {
                sim.select("file.txt");
                sim.enter();
            }
        }
    }
    #[cfg(not(feature = "git"))]
    {
        let mut sim = Sim::new(&dir);
        sim.app.lang = Lang::En;
        settle(&mut sim);
        assert!(sim.app.tab.projects.projects.is_empty());
        assert_not_offered(&sim);
        sim.key('p');
        settle(&mut sim);
        assert!(!sim.app.project_active());
        assert_eq!(
            sim.app.flash.as_deref(),
            Some(tr(Lang::En, Msg::NotAGitRepo))
        );
    }
}

#[cfg(feature = "git")]
fn check_project_entries(root: &std::path::Path, id: &str) {
    use super::canvas::{Draw, Painter};
    use std::sync::Arc;
    std::fs::write(root.join("file.txt"), "changed\n").unwrap();
    std::fs::write(root.join("notes.md"), "# Title\n\ntext\n").unwrap();
    std::fs::write(root.join("data.csv"), "a,b\n1,2\n").unwrap();
    std::fs::write(root.join("canvas.pnt"), "canvas\n").unwrap();
    write_solid_png(&root.join("picture.png"), 8, 8, [1, 2, 3]);
    let marker = crate::plugins::test_plugins::register(
        &format!("standard_entry_canvas_{id}"),
        Arc::new(Painter::new(
            "standard-entry-canvas",
            Draw::Pattern { cols: 20, rows: 4 },
        )),
    );
    let cfg = crate::plugins::test_plugins::marker_config(&marker);
    for entry in [None, Some(','), Some(' ')] {
        for (name, context) in [
            ("", Context::Tree),
            ("file.txt", Context::Text),
            ("notes.md", Context::Document),
            ("data.csv", Context::Table),
            ("picture.png", Context::Image),
            ("canvas.pnt", Context::Canvas),
            ("file.txt:diff", Context::Diff),
        ] {
            let mut sim = Sim::with_config(root, cfg.clone()).with_picker();
            settle(&mut sim);
            if name == "file.txt:diff" {
                sim.app.open_git_diff(&root.join("file.txt"));
                sim.draw();
            } else if !name.is_empty() {
                sim.select(name);
                sim.enter();
            }
            assert_eq!(sim.app.key_context(), Some(context), "{id} {name}");
            let origin = sim.app.tab.mode;
            if let Some(key) = entry {
                let leader = sim
                    .app
                    .keymaps
                    .leader_keys_of(sim.app.key_context().unwrap())
                    .into_iter()
                    .find(|(press, _)| press.code == KeyCode::Char(key))
                    .unwrap()
                    .1;
                let menu = crate::ops::menu::menu(
                    &sim.app,
                    sim.app.key_context().unwrap(),
                    leader,
                    crate::ops::menu::View::Open,
                )
                .unwrap();
                let item = menu.items.iter().find(|i| i.action == Action::OpenGitView);
                if let Some(item) = item {
                    let keypress = item.key;
                    sim.key(key);
                    sim.press(
                        keypress.code,
                        if keypress.ctrl {
                            KeyModifiers::CONTROL
                        } else {
                            KeyModifiers::NONE
                        },
                    );
                } else {
                    continue;
                }
            } else {
                sim.key('p');
            }
            settle(&mut sim);
            assert!(
                sim.app.project_active(),
                "id={id} entry={entry:?} preview={name} context={:?} flash={:?} dialog={}",
                sim.app.key_context(),
                sim.app.flash,
                sim.app.project_dialog.is_some()
            );
            let frame = sim.app.project_frame().unwrap();
            assert_eq!(
                sim.app
                    .tab
                    .project_navigation
                    .as_ref()
                    .unwrap()
                    .project
                    .plugin,
                id
            );
            assert_eq!(frame.key.screen, "changes");
            sim.key('q');
            assert!(!sim.app.project_active());
            assert_eq!(sim.app.tab.mode, origin);
        }
    }
}

#[cfg(feature = "git")]
#[test]
fn e2e_standard_vcs_p_opens_real_git_and_jj_project_screens() {
    let dir = unique_tmp("standard_vcs_git");
    std::fs::create_dir_all(&dir).unwrap();
    init_git_repo(&dir);
    std::fs::write(dir.join("file.txt"), "base\n").unwrap();
    run_git(&dir, &["add", "."]);
    run_git(&dir, &["commit", "-qm", "seed"]);
    check_project_entries(&dir, "git");
    let Some(jj) = crate::test_support::jj_scratch_bare("standard_vcs_jj") else {
        eprintln!("not run: jj project entry (jj unavailable)");
        return;
    };
    std::fs::write(jj.join("file.txt"), "text\n").unwrap();
    assert!(crate::test_support::run_jj(&jj, &["commit", "-m", "seed"]));
    check_project_entries(&jj, "jj");
}

#[cfg(feature = "git")]
#[test]
fn e2e_standard_vcs_colocated_p_shows_the_project_chooser() {
    if !crate::vcs::jj::available() {
        eprintln!("not run: colocated project chooser (jj unavailable)");
        return;
    }
    let dir = unique_tmp("standard_vcs_chooser");
    std::fs::create_dir_all(&dir).unwrap();
    init_git_repo(&dir);
    assert!(crate::test_support::run_jj(
        &dir,
        &["git", "init", "--colocate", "."]
    ));
    let mut sim = Sim::new(&dir);
    settle(&mut sim);
    sim.key('p');
    assert!(!sim.app.project_active());
    let crate::app::project_screens::ProjectDialog::Projects { projects, .. } =
        sim.app.project_dialog.as_ref().unwrap()
    else {
        panic!("project chooser required")
    };
    assert_eq!(
        projects
            .iter()
            .map(|p| p.plugin.as_str())
            .collect::<Vec<_>>(),
        ["git", "jj"]
    );
    sim.key('j');
    sim.enter();
    settle(&mut sim);
    assert_eq!(
        sim.app
            .tab
            .project_navigation
            .as_ref()
            .unwrap()
            .project
            .plugin,
        "jj"
    );
    // The positive outcome: the chosen project is open on its first screen.
    assert!(sim.app.project_active(), "the chosen jj project opens");
    assert_eq!(sim.app.project_frame().unwrap().key.screen, "changes");
}

/// A colocated repository (git and jj at one root) with the chooser open on `p`.
#[cfg(feature = "git")]
fn colocated_chooser(name: &str) -> Option<(konoma_test_support::TmpDir, Sim)> {
    if !crate::vcs::jj::available() {
        eprintln!("not run: colocated project chooser (jj unavailable)");
        return None;
    }
    let dir = unique_tmp(name);
    std::fs::create_dir_all(&dir).unwrap();
    init_git_repo(&dir);
    assert!(crate::test_support::run_jj(
        &dir,
        &["git", "init", "--colocate", "."]
    ));
    let mut sim = Sim::new(&dir);
    settle(&mut sim);
    sim.key('p');
    assert!(sim.app.project_dialog.is_some(), "the chooser is open");
    Some((dir, sim))
}

/// Removes `marker` under `root` and lets detection answer again (the injected event is fenced).
#[cfg(feature = "git")]
fn lose_marker(sim: &mut Sim, root: &std::path::Path, marker: &str) {
    std::fs::remove_dir_all(root.join(marker)).unwrap();
    sim.app
        .tab
        .projects
        .inject_event(Ok(notify::Event::new(notify::EventKind::Remove(
            notify::event::RemoveKind::Any,
        ))
        .add_path(root.join(marker))));
    sim.app.poll_projects();
    settle(sim);
}

#[cfg(feature = "git")]
fn chooser_plugins(sim: &Sim) -> Vec<String> {
    match sim.app.project_dialog.as_ref() {
        Some(crate::app::project_screens::ProjectDialog::Projects { projects, .. }) => {
            projects.iter().map(|p| p.plugin.clone()).collect()
        }
        _ => Vec::new(),
    }
}

/// A candidate the detection answer no longer lists leaves the chooser; with none left the chooser
/// closes, the old hub name waiting for it is dropped and the flash is the no-project one.
#[cfg(feature = "git")]
#[test]
fn e2e_standard_vcs_chooser_drops_a_candidate_that_vanished() {
    let Some((dir, mut sim)) = colocated_chooser("standard_vcs_chooser_vanish") else {
        return;
    };
    let root = dir.to_path_buf();
    assert_eq!(chooser_plugins(&sim), ["git", "jj"]);
    lose_marker(&mut sim, &root, ".jj");
    assert_eq!(chooser_plugins(&sim), ["git"], "jj left the chooser");
    assert!(!sim.app.project_active(), "nothing opens by itself");
    sim.app.tab.project_enter_op = Some("hub".into());
    lose_marker(&mut sim, &root, ".git");
    assert!(sim.app.project_dialog.is_none(), "the empty chooser closes");
    assert!(sim.app.tab.project_enter_op.is_none());
    assert_eq!(
        sim.app.flash.as_deref(),
        Some(tr(Lang::En, Msg::NotAGitRepo))
    );
    assert!(!sim.app.project_active());
}

/// Choosing a candidate the detection answer no longer lists is refused with the no-project flash
/// instead of opening screens from a runtime that is not this tab's.
#[cfg(feature = "git")]
#[test]
fn e2e_standard_vcs_choosing_a_vanished_candidate_is_refused() {
    let Some((dir, mut sim)) = colocated_chooser("standard_vcs_chooser_stale") else {
        return;
    };
    let root = dir.to_path_buf();
    let both = match sim.app.project_dialog.as_ref() {
        Some(crate::app::project_screens::ProjectDialog::Projects { projects, .. }) => {
            projects.clone()
        }
        _ => panic!("project chooser required"),
    };
    lose_marker(&mut sim, &root, ".jj");
    // The chooser as it was before the answer: it still lists jj.
    sim.app.project_dialog = Some(crate::app::project_screens::ProjectDialog::Projects {
        projects: both,
        selected: 1,
    });
    sim.app.flash = None;
    sim.enter();
    settle(&mut sim);
    assert!(!sim.app.project_active(), "no screens from a vanished jj");
    assert_eq!(
        sim.app.flash.as_deref(),
        Some(tr(Lang::En, Msg::NotAGitRepo))
    );
}

#[cfg(feature = "git")]
#[test]
fn e2e_standard_vcs_disabling_a_detected_project_blocks_keys_menus_and_a_stale_chooser() {
    let dir = unique_tmp("standard_vcs_disabled_cached");
    std::fs::create_dir_all(&dir).unwrap();
    init_git_repo(&dir);
    std::fs::write(dir.join("file.txt"), "base\n").unwrap();
    run_git(&dir, &["add", "."]);
    run_git(&dir, &["commit", "-qm", "seed"]);
    for preview in [false, true] {
        let mut sim = Sim::new(&dir);
        settle(&mut sim);
        assert_eq!(sim.app.tab.projects.projects.len(), 1);
        sim.app
            .cfg
            .plugins
            .entry("git".into())
            .or_default()
            .insert("enabled".into(), toml::Value::Boolean(false));
        if preview {
            sim.select("file.txt");
            sim.enter();
        }
        sim.key('p');
        settle(&mut sim);
        assert!(
            !sim.app.project_active(),
            "disabled cached project must not open on p"
        );
        assert_eq!(
            sim.app.flash.as_deref(),
            Some(tr(Lang::En, Msg::NotAGitRepo)),
            "disabled cached project: p says there is no repository"
        );
        assert_not_offered(&sim);
        // A chooser opened before disabling a plugin must recheck the choice at acceptance.
        let project =
            crate::app::project_screens::ProjectKey::from(&sim.app.tab.projects.projects[0]);
        sim.app.project_dialog = Some(crate::app::project_screens::ProjectDialog::Projects {
            projects: vec![project],
            selected: 0,
        });
        sim.enter();
        settle(&mut sim);
        assert!(
            !sim.app.project_active(),
            "disabled chooser selection must not open"
        );
        assert!(
            sim.app.project_dialog.is_none(),
            "the stale chooser is closed"
        );
    }
}

#[cfg(feature = "git")]
#[test]
fn e2e_standard_vcs_p_while_detection_is_pending_enters_the_project_or_reports_no_repository() {
    for repository in [false, true] {
        let dir = unique_tmp("standard_vcs_pending");
        std::fs::create_dir_all(&dir).unwrap();
        if repository {
            init_git_repo(&dir);
            std::fs::write(dir.join("file.txt"), "base\n").unwrap();
            run_git(&dir, &["add", "."]);
            run_git(&dir, &["commit", "-qm", "seed"]);
        }
        let mut sim = Sim::new(&dir);
        sim.app.lang = Lang::En;
        assert!(sim.app.tab.projects.pending());
        sim.key('p');
        settle(&mut sim);
        assert_eq!(sim.app.project_active(), repository);
        if repository {
            assert_eq!(sim.app.project_frame().unwrap().key.screen, "changes");
        }
        if !repository {
            assert_eq!(
                sim.app.flash.as_deref(),
                Some(tr(Lang::En, Msg::NotAGitRepo))
            );
        }
    }
}

#[cfg(feature = "git")]
#[test]
fn e2e_standard_vcs_disabled_return_paths_do_not_reopen_project_or_legacy_hub() {
    let dir = unique_tmp("standard_vcs_disabled_return");
    std::fs::create_dir_all(&dir).unwrap();
    init_git_repo(&dir);
    std::fs::write(dir.join("file.txt"), "base\n").unwrap();
    run_git(&dir, &["add", "."]);
    run_git(&dir, &["commit", "-qm", "seed"]);
    std::fs::write(dir.join("file.txt"), "changed\n").unwrap();
    let mut sim = Sim::new(&dir);
    settle(&mut sim);
    sim.key('p');
    settle(&mut sim);
    sim.enter();
    settle(&mut sim);
    assert!(sim.app.is_git_diff_preview());
    sim.app
        .cfg
        .plugins
        .entry("git".into())
        .or_default()
        .insert("enabled".into(), toml::Value::Boolean(false));
    sim.key('q');
    settle(&mut sim);
    assert!(
        !sim.app.project_active(),
        "disabled project must not reopen from a diff return"
    );
    assert!(!sim.app.is_git_diff_preview(), "the diff was left");
    assert_eq!(
        sim.app.tab.mode,
        Mode::Tree,
        "q on the diff returns to the tree, not to a project"
    );
}

#[cfg(feature = "git")]
#[test]
fn e2e_standard_vcs_loaded_settings_reach_the_real_plugin_open() {
    use konoma_plugin::{Failure, Input, Manifest, OpenCx, Plugin, Session, Settings, Value};
    use std::sync::{Arc, Mutex};
    struct ObservedSettings {
        plugin: Arc<dyn Plugin>,
        opened: Arc<Mutex<Option<Settings>>>,
    }
    impl Plugin for ObservedSettings {
        fn manifest(&self) -> Manifest {
            self.plugin.manifest()
        }
        fn open(&self, input: Input, cx: &OpenCx) -> Result<Box<dyn Session>, Failure> {
            *self.opened.lock().unwrap() = Some(cx.settings.clone());
            self.plugin.open(input, cx)
        }
    }
    let dir = unique_tmp("standard_vcs_settings");
    std::fs::create_dir_all(&dir).unwrap();
    init_git_repo(&dir);
    for id in ["git", "jj"] {
        let jj;
        let root = if id == "jj" {
            let Some(repository) = crate::test_support::jj_scratch_bare("standard_vcs_settings_jj")
            else {
                eprintln!("not run: jj plugin settings (jj unavailable)");
                continue;
            };
            jj = repository;
            jj.as_path()
        } else {
            dir.as_path()
        };
        for (index, (old, new, expected)) in [
            ("commit_meta_align = 'inline'\n", "", "inline"),
            ("", "commit_meta_align = 'inline'\n", "inline"),
            (
                "commit_meta_align = 'right'\n",
                "commit_meta_align = 'inline'\n",
                "inline",
            ),
            (
                "commit_meta_align = 'inline'\n",
                "commit_meta_align = false\n",
                "right",
            ),
        ]
        .into_iter()
        .enumerate()
        {
            let path = dir.join("config.toml");
            std::fs::write(&path, format!("[ui]\n{old}[plugins.{id}]\n{new}")).unwrap();
            let (mut cfg, error) = Config::load_path_reporting(&path);
            assert!(error.is_none());
            let opened = Arc::new(Mutex::new(None));
            let plugin: Arc<dyn Plugin> = if id == "git" {
                Arc::new(konoma_vcs::Git)
            } else {
                Arc::new(konoma_vcs::Jj)
            };
            let marker = crate::plugins::test_plugins::register_replacement(
                &format!("standard_settings_{id}_{index}"),
                Arc::new(ObservedSettings {
                    plugin,
                    opened: opened.clone(),
                }),
            );
            cfg.plugins
                .extend(crate::plugins::test_plugins::marker_config(&marker).plugins);
            let mut sim = Sim::with_config(root, cfg);
            settle(&mut sim);
            sim.key('p');
            settle(&mut sim);
            assert!(sim.app.project_active());
            assert_eq!(
                opened
                    .lock()
                    .unwrap()
                    .as_ref()
                    .unwrap()
                    .get("commit_meta_align"),
                Some(&Value::Str(expected.into())),
                "{id} case {index}: loaded settings must reach Plugin::open"
            );
        }
    }
}

#[cfg(feature = "git")]
fn act_dialog_open(sim: &Sim) -> bool {
    matches!(
        sim.app.project_dialog,
        Some(crate::app::project_screens::ProjectDialog::Act { .. })
    )
}

/// A confirmation or a text input of a project that stops being detected is closed with the
/// screens, so the keys reach the tree again instead of a dialog that can no longer act.
#[cfg(feature = "git")]
#[test]
fn e2e_standard_vcs_vanished_project_closes_its_open_dialog() {
    for (key, what) in [('c', "text input"), ('x', "confirmation")] {
        let repo = super::vcs_changes::git_repo("standard_vcs_vanish_dialog", true);
        let root = repo.to_path_buf();
        let mut sim = Sim::new(&root);
        settle(&mut sim);
        sim.key('p');
        settle(&mut sim);
        assert!(sim.app.project_active(), "{what}: the screens open");
        sim.key(key);
        assert!(act_dialog_open(&sim), "{what}: the dialog is open");
        lose_marker(&mut sim, &root, ".git");
        assert!(!act_dialog_open(&sim), "{what}: the dialog is closed");
        assert!(sim.app.project_dialog.is_none(), "{what}");
        assert!(!sim.app.project_active(), "{what}: the screens are closed");
        assert_eq!(
            sim.app.flash.as_deref(),
            Some(tr(Lang::En, Msg::NotAGitRepo)),
            "{what}"
        );
        // The keys reach the tree now: `j` moves the selection.
        let before = sim.app.tab.selected;
        sim.key('j');
        assert_ne!(sim.app.tab.selected, before, "{what}: keys reach the tree");
    }
}

/// An answer whose watch failed after a scan that no longer lists the project still retires it: the
/// screens close, the project is dropped, and the failure notice is shown.
#[cfg(feature = "git")]
#[test]
fn e2e_standard_vcs_a_failed_answer_without_the_project_retires_it() {
    let repo = super::vcs_changes::git_repo("standard_vcs_failed_answer_retires", true);
    let root = repo.to_path_buf();
    let mut sim = Sim::new(&root);
    settle(&mut sim);
    sim.key('p');
    settle(&mut sim);
    assert!(sim.app.project_active(), "the screens open");
    sim.app
        .tab
        .projects
        .publish_answer_for_test(Some(Vec::new()), true);
    assert!(sim.app.poll_projects());
    assert!(sim.app.tab.projects.projects.is_empty());
    assert!(
        !sim.app.project_active(),
        "the screens of the gone project are closed"
    );
    assert_eq!(
        sim.app.flash.as_deref(),
        Some(tr(Lang::En, Msg::ProjectDetectionFailed))
    );
}

/// Losing only the other project of a colocated root leaves the dialog of the project that stays.
#[cfg(feature = "git")]
#[test]
fn e2e_standard_vcs_losing_the_other_project_keeps_a_dialog() {
    let Some((dir, mut sim)) = colocated_chooser("standard_vcs_vanish_other_dialog") else {
        return;
    };
    let root = dir.to_path_buf();
    // The first candidate is git.
    sim.enter();
    settle(&mut sim);
    assert!(sim.app.project_active(), "the git project opens");
    std::fs::write(root.join("new.txt"), "x\n").unwrap();
    sim.key('c');
    assert!(act_dialog_open(&sim), "the git dialog is open");
    lose_marker(&mut sim, &root, ".jj");
    assert!(act_dialog_open(&sim), "the git dialog stays");
    assert!(sim.app.project_active(), "the git screens stay");
}
