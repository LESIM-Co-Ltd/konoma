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
            let (_, legacy_opens) = App::count_legacy_hub_opens(|| sim.key('p'));
            settle(&mut sim);
            assert_eq!(legacy_opens, 0, "p must never call the legacy hub");
            assert!(
                !sim.app.is_git_view(),
                "disabled p must not open the legacy hub"
            );
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
        let (_, legacy_opens) = App::count_legacy_hub_opens(|| sim.key('p'));
        settle(&mut sim);
        assert_eq!(legacy_opens, 0, "no-git p must never call the legacy hub");
        assert!(!sim.app.is_git_view());
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
            assert!(!sim.app.is_git_view());
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
    assert!(!sim.app.is_git_view());
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
    assert!(!sim.app.is_git_view());
    // The positive outcome: the chosen project is open on its first screen.
    assert!(sim.app.project_active(), "the chosen jj project opens");
    assert_eq!(sim.app.project_frame().unwrap().key.screen, "changes");
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
        assert!(!sim.app.is_git_view());
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
        assert!(!sim.app.is_git_view());
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
        let (_, legacy_opens) = App::count_legacy_hub_opens(|| {
            sim.key('p');
            settle(&mut sim);
        });
        assert_eq!(
            legacy_opens, 0,
            "pending entry must not fall back to the legacy hub"
        );
        assert_eq!(sim.app.project_active(), repository);
        assert!(!sim.app.is_git_view());
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
    assert!(!sim.app.is_git_view());
    assert!(!sim.app.is_git_diff_preview(), "the diff was left");
    assert_eq!(
        sim.app.tab.mode,
        Mode::Tree,
        "q on the diff returns to the tree, not to a project"
    );
    sim.app.open_git_view();
    assert!(
        !sim.app.is_git_view(),
        "a legacy return must not bypass disabled"
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
