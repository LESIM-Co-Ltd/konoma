//! Project behavior through the same key dispatch and drawing entry points as the host.

#[path = "project_scroll.rs"]
mod scroll;

use super::*;
use crate::plugins::test_plugins::{self, Latch};
use crate::plugins::test_projects::{ProjectLog, Screens, ViewHold};
use konoma_plugin::project::{ProjectAct, ProjectRow, ProjectView};
use konoma_plugin::Outcome;
use std::sync::{Arc, Mutex};

struct Fixture {
    sim: Sim,
    log: Arc<Mutex<ProjectLog>>,
    _dir: crate::test_support::TmpDir,
}

fn fixture(
    name: &str,
    declared_read_only: bool,
    markers: usize,
    outcome: Option<Outcome>,
    hold: Option<Arc<Latch>>,
    started: Option<Arc<Latch>>,
) -> Fixture {
    fixture_with_log(
        name,
        declared_read_only,
        markers,
        outcome,
        hold,
        started,
        ProjectLog::default(),
    )
}

fn fixture_with_log(
    name: &str,
    declared_read_only: bool,
    markers: usize,
    outcome: Option<Outcome>,
    hold: Option<Arc<Latch>>,
    started: Option<Arc<Latch>>,
    initial: ProjectLog,
) -> Fixture {
    let dir = sandbox(name);
    for index in 0..markers {
        std::fs::write(
            dir.join(format!("{index}.screen-project")),
            format!("chip-{index}"),
        )
        .unwrap();
    }
    std::fs::write(dir.join("file.txt"), "one\ntwo\nthree\nfour\n").unwrap();
    std::fs::create_dir_all(dir.join("destination")).unwrap();
    let file_only = initial.file_only;
    let log = Arc::new(Mutex::new(initial));
    let marker = test_plugins::register(
        name,
        Arc::new(Screens {
            log: log.clone(),
            read_only: declared_read_only,
            hold,
            started,
            outcome,
        }),
    );
    let mut cfg = test_plugins::marker_config(&marker);
    cfg.ui.lang = "en".into();
    cfg.external.git = false;
    if let Some(value) = log.lock().unwrap().prompt_value.clone() {
        cfg.plugins
            .entry("project-screens".into())
            .or_default()
            .insert("confirm_sync".into(), value);
    }
    let mut sim = Sim::with_config_sized(&canon(&dir), cfg, 160, 24);
    if !file_only {
        sim.app.tab.projects.wait_for_result();
        assert!(sim.app.tab.projects.uses_injected_events());
    }
    sim.app.poll_projects();
    drain(&mut sim);
    Fixture {
        sim,
        log,
        _dir: dir,
    }
}

#[test]
fn e2e_project_chip_initial_styled_and_worker_owned() {
    use konoma_plugin::{Chip, Color, Line, Span, Style};
    let mut f = fixture_with_log(
        "project_chip_initial",
        false,
        1,
        None,
        None,
        None,
        ProjectLog {
            chip: Some(Chip::new(Line::from(vec![
                Span::styled("計算", Style::new().fg(Color::Red)),
                Span::styled(" chip", Style::new().fg(Color::Green)),
            ]))),
            ..Default::default()
        },
    );
    let screen = chip_screen(&f.sim);
    let title = screen
        .lines()
        .find(|line| line.contains(" · "))
        .unwrap_or(&screen);
    assert!(title.contains(" · 計算 chip "), "{title}");
    f.sim.dont_see("project chip");
    assert_eq!(f.log.lock().unwrap().chips, 1);
    assert_eq!(f.log.lock().unwrap().watch_threads.len(), 1);
    assert!(f
        .log
        .lock()
        .unwrap()
        .threads
        .iter()
        .all(|id| *id != std::thread::current().id()));
    let buffer = f.sim.term.backend().buffer();
    let (x, y) = (0..buffer.area.height)
        .flat_map(|y| (0..buffer.area.width).map(move |x| (x, y)))
        .find(|pos| buffer[*pos].symbol() == "計")
        .unwrap();
    assert_eq!(buffer[(x, y)].fg, ratatui::style::Color::Red);
    assert_eq!(buffer[(x + 2, y)].symbol(), "算");
    for _ in 0..3 {
        f.sim.draw();
    }
    assert_eq!(f.log.lock().unwrap().chips, 1);
}

fn chip_screen(s: &Sim) -> String {
    use unicode_width::UnicodeWidthStr;
    let buffer = s.term.backend().buffer();
    let mut text = String::new();
    for y in 0..buffer.area.height {
        let mut x = 0;
        while x < buffer.area.width {
            let symbol = buffer[(x, y)].symbol();
            text.push_str(symbol);
            // A terminal advances over the continuation cell of a wide glyph.
            x += symbol.width().max(1) as u16;
        }
        text.push('\n');
    }
    text
}

#[test]
fn e2e_project_chip_fallback_empty_failure_and_panic_keep_session() {
    for mode in 0..4 {
        for declared in [true, false] {
            let mut f = fixture_with_log(
                &format!("project_chip_fallback_{mode}_{declared}"),
                false,
                1,
                None,
                None,
                None,
                ProjectLog {
                    chip: (mode == 1).then(konoma_plugin::Chip::default),
                    chip_failure: mode == 2,
                    chip_panic: mode == 3,
                    no_declared_chip: !declared,
                    ..Default::default()
                },
            );
            assert_eq!(
                f.sim.screen().contains(" · project chip "),
                declared && mode != 1
            );
            if mode >= 2 {
                assert_eq!(
                    f.sim.app.flash.as_deref(),
                    Some(if mode == 2 {
                        "chip fixture failure"
                    } else {
                        crate::i18n::tr(f.sim.app.lang, crate::i18n::Msg::ProjectWorkerFailed)
                    })
                );
            }
            open(&mut f);
            f.sim.key('x');
            drain(&mut f.sim);
            assert_eq!(last_act(&f).0, "capture");
            assert_eq!(f.log.lock().unwrap().opens, 1);
            // A later chip failure must replace a previous computed value, too.
            f.log.lock().unwrap().chip = Some(konoma_plugin::Chip::new("before failure"));
            f.log.lock().unwrap().chip_failure = false;
            f.log.lock().unwrap().chip_panic = false;
            press(&mut f, KeyCode::F(10));
            assert_eq!(f.sim.app.project_chips()[0].to_string(), "before failure");
            f.log.lock().unwrap().chip = (mode == 1).then(konoma_plugin::Chip::default);
            f.log.lock().unwrap().chip_failure = mode == 2;
            f.log.lock().unwrap().chip_panic = mode == 3;
            press(&mut f, KeyCode::F(10));
            if mode >= 2 {
                assert_eq!(
                    f.sim.app.flash.as_deref(),
                    Some(if mode == 2 {
                        "chip fixture failure"
                    } else {
                        crate::i18n::tr(f.sim.app.lang, crate::i18n::Msg::ProjectWorkerFailed)
                    })
                );
            }
            f.sim.key('x');
            drain(&mut f.sim);
            assert_eq!(f.log.lock().unwrap().acts.last().unwrap().0, "capture");
            f.sim.key('q');
            assert_eq!(
                f.sim.screen().contains(" · project chip "),
                declared && mode != 1
            );
            f.sim.dont_see("before failure");
            assert_eq!(f.log.lock().unwrap().opens, 1);
        }
    }
}

#[test]
fn e2e_project_chip_clips_controls_text_spans_and_narrow_cjk_title() {
    use konoma_plugin::{Chip, Line, Span};
    let mut f = fixture_with_log(
        "project_chip_clipped",
        false,
        1,
        None,
        None,
        None,
        ProjectLog {
            chip: Some(Chip::new(Line::from(vec![
                Span::raw("a\0\n\t\r\x1b\x7f\u{85}\u{10eeee}b"),
                Span::raw("界".repeat(600)),
            ]))),
            ..Default::default()
        },
    );
    let kept = f.sim.app.project_chips()[0].to_string();
    assert_eq!(kept, format!("ab{}…", "界".repeat(509)));
    f.sim.resize(1200, 24);
    drain(&mut f.sim);
    assert!(chip_screen(&f.sim).contains(&kept));
    for width in [73, 74, 75, 76] {
        f.sim.resize(width, 24);
        drain(&mut f.sim);
        let screen = chip_screen(&f.sim);
        let title = screen.lines().find(|line| line.starts_with('┌')).unwrap();
        assert!(title.ends_with('┐'), "{title}");
        assert_eq!(unicode_width::UnicodeWidthStr::width(title), width as usize);
        assert_eq!(f.sim.app.project_chips()[0].to_string(), kept);
    }
    assert_eq!(f.log.lock().unwrap().chips, 1);
    f.log.lock().unwrap().chip = Some(Chip::new(Line::from(
        (0..65)
            .map(|i| Span::raw(if i == 64 { "overflow" } else { "x" }))
            .collect::<Vec<_>>(),
    )));
    open(&mut f);
    press(&mut f, KeyCode::F(10));
    assert_eq!(f.sim.app.project_chips()[0].to_string(), "x".repeat(64));
}

#[test]
fn e2e_project_chip_file_only_session_is_never_called() {
    let mut f = fixture_with_log(
        "project_chip_file_only",
        false,
        1,
        None,
        None,
        None,
        ProjectLog {
            file_only: true,
            chip_panic: true,
            ..Default::default()
        },
    );
    assert!(f.sim.app.tab.projects.projects.is_empty());
    std::fs::write(f.sim.app.tab.root.join("view.screen-file"), "file").unwrap();
    f.sim.app.rebuild_tree().unwrap();
    f.sim.select("view.screen-file");
    f.sim.enter();
    f.sim.see("file session");
    assert_eq!(f.log.lock().unwrap().opens, 1);
    assert_eq!(f.log.lock().unwrap().chips, 0);
}

#[test]
fn e2e_project_chip_tabs_roots_and_multiple_project_order() {
    let mut f = fixture_with_log(
        "project_chip_tabs",
        false,
        2,
        None,
        None,
        None,
        ProjectLog {
            chip_from_marker: true,
            ..Default::default()
        },
    );
    let first_root = f.sim.app.tab.root.clone();
    f.sim.see(" · chip-0  · chip-1 ");
    let other = sandbox("project_chip_other_root");
    std::fs::write(other.join("0.screen-project"), "other chip").unwrap();
    let empty = sandbox("project_chip_empty_root");
    f.log.lock().unwrap().outcome = Some(Outcome::OpenTab {
        path: canon(&other),
    });
    f.sim.key('p');
    f.sim.enter();
    drain(&mut f.sim);
    press(&mut f, KeyCode::F(11));
    f.sim.app.tab.projects.wait_for_result();
    drain(&mut f.sim);
    f.sim.see(" · other chip ");
    f.sim.dont_see("chip-0");
    f.sim.dont_see("chip-1");
    f.sim.key('[');
    drain(&mut f.sim);
    assert_eq!(f.sim.app.tab.root, first_root);
    f.sim.key('q');
    f.sim.see(" · chip-0  · chip-1 ");
    f.sim.dont_see("other chip");
    f.sim.key(']');
    drain(&mut f.sim);
    open(&mut f);
    f.log.lock().unwrap().outcome = Some(Outcome::SwitchRoot {
        root: canon(&empty),
    });
    press(&mut f, KeyCode::F(11));
    assert!(f.sim.app.project_chips().is_empty());
    f.sim.app.tab.projects.wait_for_result();
    drain(&mut f.sim);
    f.sim.dont_see("other chip");
    f.sim.dont_see("chip-0");
}

#[test]
fn e2e_project_chip_watch_during_held_reply_fetches_latest_on_tree() {
    let mut f = fixture("project_chip_watch_busy", false, 1, None, None, None);
    open(&mut f);
    let before = f.log.lock().unwrap().chips;
    let gate = ViewHold::new("chip");
    f.log.lock().unwrap().chip = Some(konoma_plugin::Chip::new("stale chip"));
    f.log.lock().unwrap().chip_hold = Some(gate.clone());
    f.sim.press(KeyCode::F(10), KeyModifiers::NONE);
    gate.entered.wait();
    assert!(gate.entered.is_open());
    f.log.lock().unwrap().chip = Some(konoma_plugin::Chip::new("latest chip"));
    let root = f.sim.app.tab.root.clone();
    std::fs::create_dir_all(root.join("metadata")).unwrap();
    std::fs::write(root.join("metadata/index"), "change").unwrap();
    f.sim.app.tab.projects.inject_event(Ok(
        notify::Event::new(notify::EventKind::Any).add_path(root.join("metadata/index"))
    ));
    assert!(f.sim.app.poll_projects());
    f.sim.key('q');
    gate.release.open();
    f.sim.app.wait_project_job();
    assert!(!f
        .sim
        .app
        .project_chips()
        .iter()
        .any(|chip| chip.to_string() == "stale chip"));
    drain(&mut f.sim);
    f.sim.see("latest chip");
    assert_eq!(f.log.lock().unwrap().chips, before + 2);
    f.sim.dont_see("stale chip");
}

fn drain(s: &mut Sim) {
    // Every iteration consumes an actual worker reply; a stuck worker has a bounded receive.
    for _ in 0..32 {
        s.app.poll_projects();
        s.draw();
        if !s.app.project_jobs_pending() {
            return;
        }
        s.app.wait_project_job();
    }
    panic!("project did not become idle");
}

fn hold_view(f: &mut Fixture, screen: &str, reverse: bool) -> Arc<ViewHold> {
    drain(&mut f.sim);
    let gate = ViewHold::new(screen);
    f.log.lock().unwrap().reverse_rows = reverse;
    f.log.lock().unwrap().view_hold = Some(gate.clone());
    gate
}

fn resize_while_held(f: &mut Fixture, gate: &ViewHold) {
    f.sim.resize(159, 24);
    f.sim.app.wait_project_view_request(&gate.screen);
    gate.entered.wait();
    assert!(gate.entered.is_open());
}

#[test]
fn e2e_project_loading_keys_wait_for_current_screen_and_cancel_when_leaving() {
    let mut f = fixture("project_loading_diff_input", false, 1, None, None, None);
    open(&mut f);
    let gate = hold_view(&mut f, "diff", false);
    f.sim.key('d');
    f.sim.app.wait_project_view_request("diff");
    gate.entered.wait();
    assert!(gate.entered.is_open());
    f.sim.key('x');
    assert!(f.log.lock().unwrap().acts.is_empty());
    gate.release.open();
    drain(&mut f.sim);
    assert_eq!(last_act(&f).0, "diff-act");
    assert_eq!(last_act(&f).1.screen.as_deref(), Some("diff"));
    f.sim.key('i');
    drain(&mut f.sim);
    let gate = hold_view(&mut f, "items", false);
    resize_while_held(&mut f, &gate);
    let before = f.log.lock().unwrap().acts.len();
    f.sim.key('x');
    f.sim.key('q');
    assert!(!f.sim.app.project_active());
    gate.release.open();
    drain(&mut f.sim);
    open(&mut f);
    assert_eq!(f.log.lock().unwrap().acts.len(), before);
}

#[test]
fn e2e_project_loading_dialog_keys_are_delivered_after_view_reply() {
    for (key, label) in [
        (2, "Confirm project?"),
        (3, "Project name"),
        (4, "Choose target"),
        (5, "Proceed?"),
    ] {
        let mut f = fixture(
            &format!("project_loading_dialog_{key}"),
            false,
            1,
            None,
            None,
            None,
        );
        open(&mut f);
        let gate = hold_view(&mut f, "items", false);
        resize_while_held(&mut f, &gate);
        f.sim.press(KeyCode::F(key), KeyModifiers::NONE);
        if key == 3 {
            f.sim.keys("q?t");
        }
        assert!(f.log.lock().unwrap().acts.is_empty());
        gate.release.open();
        drain(&mut f.sim);
        f.sim.see(label);
        assert!(f.sim.app.project_dialog.is_some());
        if key == 3 {
            assert!(
                matches!(&f.sim.app.project_dialog, Some(crate::app::project_screens::ProjectDialog::Act { buffer, .. }) if buffer == "初期q?t")
            );
        }
        f.sim.esc();
        assert!(f.log.lock().unwrap().acts.is_empty());
    }
}

#[test]
fn e2e_project_loading_navigation_key_is_not_lost() {
    let mut f = fixture("project_loading_navigation", false, 1, None, None, None);
    f.log.lock().unwrap().outcome = Some(Outcome::OpenFile {
        path: f.sim.app.tab.root.join("file.txt"),
        line: Some(3),
    });
    open(&mut f);
    let gate = hold_view(&mut f, "items", false);
    resize_while_held(&mut f, &gate);
    f.sim.press(KeyCode::F(11), KeyModifiers::NONE);
    gate.release.open();
    drain(&mut f.sim);
    assert!(!f.sim.app.project_active());
    assert_eq!(f.sim.app.preview_caret_line(), 2);
    f.sim.key('q');
    drain(&mut f.sim);
    assert!(f.sim.app.project_active());
}

#[test]
fn e2e_project_refresh_keeps_selected_and_marked_row_identity() {
    let mut f = fixture("project_refresh_row_identity", false, 1, None, None, None);
    open(&mut f);
    f.sim.key('j');
    let gate = hold_view(&mut f, "items", true);
    resize_while_held(&mut f, &gate);
    f.sim.key('j');
    f.sim.key('x');
    gate.release.open();
    drain(&mut f.sim);
    let (_, context) = last_act(&f);
    assert_eq!(
        context.selected,
        Some(ProjectRow::new("item", "opaque-gamma"))
    );
    assert_eq!(
        context.marked,
        vec![ProjectRow::new("item", "opaque-alpha")]
    );
    let before = f.log.lock().unwrap().acts.len();
    f.log.lock().unwrap().omit_row = Some("opaque-gamma".into());
    let gate = hold_view(&mut f, "items", false);
    f.sim.resize(158, 24);
    gate.entered.wait();
    assert!(gate.entered.is_open());
    f.sim.key('x');
    gate.release.open();
    drain(&mut f.sim);
    assert_eq!(
        f.log.lock().unwrap().acts.len(),
        before,
        "an action must not move to the removed row's neighbour"
    );
}

#[test]
fn e2e_project_dialog_submit_waits_for_view_and_rechecks_guard_and_row() {
    for (read_only, removed, disabled) in [
        (false, false, false),
        (true, false, false),
        (false, true, false),
        (false, false, true),
    ] {
        let mut f = fixture(
            &format!("project_queued_dialog_{read_only}_{removed}_{disabled}"),
            false,
            1,
            None,
            None,
            None,
        );
        open(&mut f);
        f.sim.press(KeyCode::F(3), KeyModifiers::NONE);
        drain(&mut f.sim);
        if removed {
            f.log.lock().unwrap().omit_row = Some("opaque-alpha".into());
        }
        let gate = hold_view(&mut f, "items", removed);
        resize_while_held(&mut f, &gate);
        f.sim.enter();
        assert!(f.log.lock().unwrap().acts.is_empty());
        if disabled {
            f.log.lock().unwrap().disabled.push("text-input".into());
        }
        f.sim.app.cfg.plugins.insert(
            "project-screens".into(),
            toml::Table::from_iter([("read_only".into(), read_only.into())]),
        );
        gate.release.open();
        drain(&mut f.sim);
        if disabled {
            assert!(f.log.lock().unwrap().acts.is_empty());
        } else if read_only || removed {
            assert!(f.log.lock().unwrap().acts.is_empty());
            assert_eq!(
                f.sim.app.flash.as_deref(),
                Some(crate::i18n::tr(
                    f.sim.app.lang,
                    if read_only {
                        crate::i18n::Msg::PluginReadOnly
                    } else {
                        crate::i18n::Msg::ProjectSelectionChanged
                    }
                ))
            );
        } else {
            assert_eq!(last_act(&f).0, "text-input");
            assert_eq!(last_act(&f).1.text.as_deref(), Some("初期"));
        }
    }
}

fn open(f: &mut Fixture) {
    f.sim.key('p');
    drain(&mut f.sim);
    assert!(f.sim.app.project_active(), "{:?}", f.sim.app.flash);
    f.sim.see("alpha");
}

#[test]
fn e2e_project_session_watch_paths_replace_declaration_and_reread_after_invalidation() {
    let mut f = fixture("project_dynamic_watch_paths", false, 1, None, None, None);
    open(&mut f);
    let outside = sandbox("project_dynamic_watch_outside");
    let first = canon(&outside).join("first/index");
    let second = canon(&outside).join("second/index");
    f.log.lock().unwrap().watch_paths = Some(vec![first.clone()]);
    let inject = |f: &mut Fixture, path| {
        f.sim
            .app
            .tab
            .projects
            .inject_event(Ok(notify::Event::new(notify::EventKind::Modify(
                notify::event::ModifyKind::Any,
            ))
            .add_path(path)));
        f.sim.app.poll_projects();
        drain(&mut f.sim);
    };
    let root = f.sim.app.tab.root.clone();
    inject(&mut f, root.join("metadata/index"));
    let count = f.log.lock().unwrap().views.len();
    let watches = f.log.lock().unwrap().watch_threads.len();
    f.log.lock().unwrap().watch_paths = Some(vec![second.clone()]);
    inject(&mut f, first.clone());
    assert_eq!(f.log.lock().unwrap().views.len(), count + 1);
    assert_eq!(f.log.lock().unwrap().watch_threads.len(), watches + 1);
    let count = f.log.lock().unwrap().views.len();
    inject(&mut f, first);
    inject(&mut f, root.join("metadata/index"));
    assert_eq!(
        f.log.lock().unwrap().views.len(),
        count,
        "old and declared paths no longer invalidate the session"
    );
    inject(&mut f, second);
    assert_eq!(f.log.lock().unwrap().views.len(), count + 1);
    assert!(f
        .log
        .lock()
        .unwrap()
        .watch_threads
        .iter()
        .all(|thread| *thread != std::thread::current().id()));
}

#[test]
fn e2e_project_act_replaces_watch_paths_before_next_invalidation() {
    let mut f = fixture("project_act_watch_paths", false, 1, None, None, None);
    open(&mut f);
    let outside = sandbox("project_act_watch_outside");
    let next = canon(&outside).join("new-gitdir/index");
    let root = f.sim.app.tab.root.clone();
    let watches = f.log.lock().unwrap().watch_threads.len();
    f.log.lock().unwrap().watch_paths = Some(vec![next.clone()]);
    f.sim.key('x');
    drain(&mut f.sim);
    assert_eq!(last_act(&f).0, "capture");
    assert_eq!(f.log.lock().unwrap().watch_threads.len(), watches + 1);
    assert_eq!(
        f.sim.app.tab.projects.projects[0].session_watch_paths,
        Some(vec![next.clone()])
    );

    // Flush PathsChanged before injecting: overflowing the bounded queue invalidates all paths.
    // Access events are ignored, but inject_event still waits for its processing fence.
    f.sim
        .app
        .tab
        .projects
        .inject_event(Ok(notify::Event::new(notify::EventKind::Access(
            notify::event::AccessKind::Any,
        ))));
    assert_eq!(f.sim.app.tab.projects.poll(), None);
    f.log.lock().unwrap().views.clear();
    let chips = f.log.lock().unwrap().chips;
    f.sim.app.tab.projects.inject_event(Ok(
        notify::Event::new(notify::EventKind::Any).add_path(root.join("metadata/index"))
    ));
    assert_eq!(f.sim.app.tab.projects.poll(), None);
    drain(&mut f.sim);
    assert!(f.log.lock().unwrap().views.is_empty());
    assert_eq!(f.log.lock().unwrap().chips, chips);

    f.sim
        .app
        .tab
        .projects
        .inject_event(Ok(notify::Event::new(notify::EventKind::Any).add_path(next)));
    assert!(f.sim.app.poll_projects());
    drain(&mut f.sim);
    assert_eq!(f.log.lock().unwrap().views, [view_context(&f, "items")]);
    assert_eq!(f.log.lock().unwrap().chips, chips + 1);
    assert_eq!(f.log.lock().unwrap().watch_threads.len(), watches + 2);
}

fn press(f: &mut Fixture, code: KeyCode) {
    f.sim.press(code, KeyModifiers::NONE);
    drain(&mut f.sim);
}

fn last_act(f: &Fixture) -> (String, ProjectAct) {
    f.log
        .lock()
        .unwrap()
        .acts
        .last()
        .cloned()
        .expect("act received")
}

fn view_context(f: &Fixture, screen: &str) -> ProjectView {
    ProjectView::new(screen)
        .paths(&f.sim.app.tab.root, &f.sim.app.tab.open_dir)
        .home_dir(crate::plugins::read_scope::home_dir())
}

#[test]
fn e2e_project_watch_paths_refresh_only_current_screen_once_and_ignore_unrelated_paths() {
    let mut f = fixture("project_injected_watch_paths", false, 1, None, None, None);
    open(&mut f);
    // Cache a second screen to distinguish invalidation from eagerly fetching every screen.
    f.sim.key('z');
    drain(&mut f.sim);
    f.sim.key('i');
    drain(&mut f.sim);
    let root = f.sim.app.tab.root.clone();
    std::fs::create_dir_all(root.join("metadata")).unwrap();
    std::fs::write(root.join("metadata/index"), "watched chip").unwrap();
    let chips = f.log.lock().unwrap().chips;
    let watches = f.log.lock().unwrap().watch_threads.len();
    f.log.lock().unwrap().chip_from_watch = true;
    f.log.lock().unwrap().views.clear();
    // Actual file writes have no effect until the test delivers their notification.
    drain(&mut f.sim);
    assert!(f.log.lock().unwrap().views.is_empty());
    assert_eq!(f.sim.app.tab.projects.poll(), None);
    f.sim.app.tab.projects.inject_event(Ok(
        notify::Event::new(notify::EventKind::Any).add_path(root.join("metadata/index"))
    ));
    assert!(f.sim.app.poll_projects());
    drain(&mut f.sim);
    assert_eq!(f.log.lock().unwrap().views, [view_context(&f, "items")]);
    assert_eq!(f.log.lock().unwrap().chips, chips + 1);
    assert_eq!(f.log.lock().unwrap().watch_threads.len(), watches + 1);
    f.sim.key('q');
    f.sim.see("watched chip");
    f.log.lock().unwrap().views.clear();
    std::fs::write(root.join("unrelated.txt"), "unrelated change").unwrap();
    f.sim.app.tab.projects.inject_event(Ok(
        notify::Event::new(notify::EventKind::Any).add_path(root.join("unrelated.txt"))
    ));
    assert_eq!(
        f.sim.app.tab.projects.poll(),
        None,
        "the event fence proves the unrelated path was processed without a scan reply"
    );
    drain(&mut f.sim);
    assert!(f.log.lock().unwrap().views.is_empty());
    assert_eq!(f.log.lock().unwrap().chips, chips + 1);
    // A second UI poll cannot consume a duplicate event or produce another view request.
    drain(&mut f.sim);
    assert!(f.log.lock().unwrap().views.is_empty());
    assert_eq!(f.log.lock().unwrap().chips, chips + 1);
}

#[test]
fn e2e_project_marker_events_redetect_created_and_removed_matches() {
    let mut f = fixture("project_injected_marker_events", false, 1, None, None, None);
    let root = f.sim.app.tab.root.clone();
    assert_eq!(f.sim.app.tab.projects.projects.len(), 1);
    let marker = root.join("1.screen-project");
    std::fs::write(&marker, "second").unwrap();
    f.sim.app.tab.projects.inject_event(Ok(
        notify::Event::new(notify::EventKind::Any).add_path(marker.clone())
    ));
    assert!(f.sim.app.poll_projects());
    assert_eq!(f.sim.app.tab.projects.projects.len(), 2);
    f.sim.key('p');
    f.sim.see("Choose a project");
    f.sim.press(KeyCode::Esc, KeyModifiers::NONE);
    std::fs::remove_file(&marker).unwrap();
    f.sim.app.tab.projects.inject_event(Ok(
        notify::Event::new(notify::EventKind::Any).add_path(marker)
    ));
    assert!(f.sim.app.poll_projects());
    assert_eq!(f.sim.app.tab.projects.projects.len(), 1);
    open(&mut f);
    f.sim.key('q');
    std::fs::remove_file(root.join("0.screen-project")).unwrap();
    f.sim.app.tab.projects.inject_event(Ok(
        notify::Event::new(notify::EventKind::Any).add_path(root.join("0.screen-project"))
    ));
    assert!(f.sim.app.poll_projects());
    assert!(f.sim.app.tab.projects.projects.is_empty());
}

#[test]
fn e2e_project_enter_cycle_jump_push_pop_and_view_fields() {
    let mut f = fixture("project_screen_navigation", false, 1, None, None, None);
    open(&mut f);
    assert_eq!(
        f.log.lock().unwrap().views.last(),
        Some(&view_context(&f, "items"))
    );
    press(&mut f, KeyCode::Tab);
    f.sim.see("text selection=none");
    assert_eq!(
        f.log.lock().unwrap().views.last(),
        Some(&view_context(&f, "text"))
    );
    f.sim.press(KeyCode::BackTab, KeyModifiers::NONE);
    f.sim.see("● [x] alpha styled");
    f.sim.see("alpha");
    f.sim.key('b');
    drain(&mut f.sim);
    f.sim.see("Project document");
    f.sim.key('i');
    drain(&mut f.sim);
    press(&mut f, KeyCode::Enter);
    f.sim.see("text selection=opaque-alpha");
    assert_eq!(
        f.log.lock().unwrap().views.last(),
        Some(&view_context(&f, "text").selection("opaque-alpha"))
    );
    assert_eq!(last_act(&f).0, "push");
    f.sim.key('q');
    drain(&mut f.sim);
    f.sim.see("alpha");
    assert_eq!(f.sim.app.project_frame().unwrap().key.screen, "items");
    f.sim.key('q');
    assert!(!f.sim.app.project_active());
    assert_eq!(f.sim.app.tab.mode, Mode::Tree);
    assert!(!f.sim.quit);
}

#[test]
fn e2e_project_act_receives_screen_selected_kind_value_and_marked_rows() {
    let mut f = fixture("project_act_real_state", false, 1, None, None, None);
    open(&mut f);
    f.sim.key('j');
    f.sim.key('V');
    f.sim.key('j');
    f.sim.key('x');
    drain(&mut f.sim);
    let (action, context) = last_act(&f);
    assert_eq!(action, "capture");
    assert_eq!(context.screen.as_deref(), Some("items"));
    assert_eq!(
        context.selected,
        Some(ProjectRow::new("item", "opaque-gamma"))
    );
    assert_eq!(
        context.marked,
        vec![
            ProjectRow::new("item", "opaque-alpha"),
            ProjectRow::new("other", "opaque-beta")
        ]
    );
    assert_eq!(context.confirmation, None);
    assert_eq!(context.text, None);
    assert_eq!(context.choice, None);
    assert_eq!(f.sim.app.flash.as_deref(), Some("did capture"));
    let log = f.log.lock().unwrap();
    assert!(log
        .threads
        .iter()
        .all(|id| *id != std::thread::current().id()));
    assert!(log.threads.iter().all(|id| *id == log.threads[0]));
}

#[test]
fn e2e_project_default_enter_changes_with_row_kind() {
    let mut f = fixture("project_row_default", false, 1, None, None, None);
    open(&mut f);
    f.sim.key('j');
    press(&mut f, KeyCode::Enter);
    assert_eq!(last_act(&f).0, "other");
    assert_eq!(
        last_act(&f).1.selected,
        Some(ProjectRow::new("other", "opaque-beta"))
    );
    assert_eq!(f.sim.app.project_frame().unwrap().key.screen, "items");
}

fn advertised(f: &Fixture, action: &str) -> bool {
    crate::ops::menu::declared_actions_menu(
        &f.sim.app,
        f.sim
            .app
            .key_context()
            .unwrap_or_else(|| f.sim.app.base_context()),
    )
    .iter()
    .any(|item| matches!(item.action, crate::keymap::Action::Plugin(p) if p.act() == action))
}

#[test]
fn e2e_project_hints_help_menu_match_screen_row_tree_and_diff_contexts() {
    let mut f = fixture("project_hint_contexts", false, 1, None, None, None);
    f.sim.select("file.txt");
    assert!(advertised(&f, "tree-act"));
    assert!(!advertised(&f, "capture"));
    assert!(!advertised(&f, "diff-act"));
    let before = f.log.lock().unwrap().acts.len();
    f.sim.key('x');
    assert_eq!(
        f.log.lock().unwrap().acts.len(),
        before,
        "tree actions have no single key"
    );
    f.sim.key(' ');
    let ctx = f.sim.app.key_context().unwrap();
    let menu = crate::ops::menu::menu(
        &f.sim.app,
        ctx,
        f.sim.app.pending_leader.unwrap(),
        crate::ops::menu::View::Open,
    )
    .unwrap();
    let key = menu
        .items
        .iter()
        .find(|i| matches!(i.action, crate::keymap::Action::Plugin(p) if p.act() == "tree-act"))
        .unwrap()
        .key;
    handle_key(
        &mut f.sim.app,
        KeyEvent::new(
            key.code,
            if key.ctrl {
                KeyModifiers::CONTROL
            } else {
                KeyModifiers::NONE
            },
        ),
    )
    .unwrap();
    drain(&mut f.sim);
    let (_, context) = last_act(&f);
    assert_eq!(context.screen, None);
    assert_eq!(
        context.selected,
        Some(ProjectRow::new(
            "file",
            f.sim.app.tab.root.join("file.txt").to_string_lossy()
        ))
    );
    open(&mut f);
    assert!(advertised(&f, "capture"));
    assert!(advertised(&f, "screen"));
    assert!(!advertised(&f, "tree-act"));
    assert!(crate::ui::status::hint_tokens(&f.sim.app)
        .iter()
        .any(|t| t.contains("capture")));
    f.sim.key('j');
    assert!(!advertised(&f, "capture"));
    assert!(advertised(&f, "screen"));
    assert!(!crate::ui::status::hint_tokens(&f.sim.app)
        .iter()
        .any(|t| t.contains("capture")));
    f.sim.key('?');
    assert!(
        !crate::ops::footer::tokens(&f.sim.app, crate::keymap::Context::Help)
            .iter()
            .any(|token| token.contains(":screen")),
        "help keys must not advertise actions of the covered project screen"
    );
    let help = f.sim.screen();
    assert!(!help.contains("capture"));
    assert!(help.contains("screen"));
    f.sim.key('q');
    f.sim.key('d');
    drain(&mut f.sim);
    assert!(advertised(&f, "diff-act"));
    assert!(!advertised(&f, "capture"));
    f.sim.key('x');
    drain(&mut f.sim);
    assert_eq!(last_act(&f).0, "diff-act");
    assert_eq!(last_act(&f).1.screen.as_deref(), Some("diff"));
    f.sim.key('z');
    drain(&mut f.sim);
    assert!(!advertised(&f, "diff-act"));
    assert!(!advertised(&f, "capture"));
    f.sim.key('b');
    drain(&mut f.sim);
    assert!(!advertised(&f, "diff-act"));
}

#[cfg(feature = "git")]
#[test]
fn e2e_project_host_diff_context_has_hints_help_menu_and_real_file_selection() {
    let mut f = navigate("project_host_diff_context", |root| Outcome::OpenDiff {
        path: root.join("file.txt"),
    });
    assert!(f.sim.app.is_git_diff_preview());
    f.sim.press(KeyCode::F(12), KeyModifiers::NONE); // Clear the navigation flash.
    assert!(advertised(&f, "diff-act"));
    assert!(
        crate::ops::footer::tokens(&f.sim.app, crate::keymap::Context::Diff)
            .iter()
            .any(|hint| hint.contains("x:diff-act"))
    );
    f.sim.see("x:diff-act");
    f.sim.key('?');
    f.sim.see("diff-act");
    f.sim.key('q');
    f.sim.key('x');
    drain(&mut f.sim);
    let (action, context) = last_act(&f);
    assert_eq!(action, "diff-act");
    assert_eq!(context.screen, None);
    assert_eq!(
        context.selected,
        Some(ProjectRow::new(
            "file",
            f.sim.app.tab.root.join("file.txt").to_string_lossy()
        ))
    );
    f.sim.key('q');
    drain(&mut f.sim); // Return to the owning project.
    f.sim.key('q');
    drain(&mut f.sim); // Return to the original tree.
    assert_eq!(f.sim.app.tab.mode, Mode::Tree);
    assert!(!advertised(&f, "diff-act"));
    assert!(!crate::ui::status::hint_tokens(&f.sim.app)
        .iter()
        .any(|hint| hint.contains("diff-act")));
    f.sim.key('?');
    f.sim.dont_see("diff-act");
    f.sim.key('q');
    f.sim.select("file.txt");
    f.sim.enter();
    assert!(!f.sim.app.is_git_diff_preview());
    assert!(!advertised(&f, "diff-act"));
    assert!(!crate::ui::status::hint_tokens(&f.sim.app)
        .iter()
        .any(|hint| hint.contains("diff-act")));
    f.sim.key('?');
    f.sim.dont_see("diff-act");
}

#[test]
fn e2e_project_screen_key_collision_preserves_tree_key_and_menu_help_access() {
    let mut f = fixture("project_context_key_collision", false, 1, None, None, None);
    f.sim.select("file.txt");
    assert!(crate::ui::status::hint_tokens(&f.sim.app)
        .iter()
        .any(|hint| hint.contains("z:tree-shortcut")));
    f.sim.key('z');
    drain(&mut f.sim);
    assert_eq!(last_act(&f).0, "tree-shortcut");
    assert_eq!(last_act(&f).1.screen, None);
    f.sim.key('?');
    f.sim.see("tree-shortcut");
    f.sim.key('q');
    open(&mut f);
    assert!(
        f.sim
            .app
            .plugin_keys
            .acts_of("project-screens")
            .iter()
            .find(|binding| binding.act.act() == "screen-shortcut")
            .unwrap()
            .key
            .is_some(),
        "the declaration must keep its key"
    );
    assert!(!crate::ui::status::hint_tokens(&f.sim.app)
        .iter()
        .any(|hint| hint.contains("z:screen-shortcut")));
    let before = f.log.lock().unwrap().acts.len();
    f.sim.key('z');
    drain(&mut f.sim);
    assert_eq!(f.sim.app.project_frame().unwrap().key.screen, "text");
    assert_eq!(f.log.lock().unwrap().acts.len(), before);
    f.sim.key('i');
    drain(&mut f.sim);
    f.sim.key('?');
    assert!(crate::ops::help::sections(&f.sim.app)
        .iter()
        .flat_map(|section| &section.rows)
        .any(|(keys, desc)| keys.starts_with("Space") && desc == "screen-shortcut"));
    f.sim.key('q');
    f.sim.key(' ');
    let ctx = f.sim.app.key_context().unwrap();
    let menu = crate::ops::menu::menu(
        &f.sim.app,
        ctx,
        f.sim.app.pending_leader.unwrap(),
        crate::ops::menu::View::Open,
    )
    .unwrap();
    let key = menu.items.iter().find(|item| matches!(item.action, crate::keymap::Action::Plugin(p) if p.act() == "screen-shortcut")).unwrap().key;
    handle_key(
        &mut f.sim.app,
        KeyEvent::new(
            key.code,
            if key.ctrl {
                KeyModifiers::CONTROL
            } else {
                KeyModifiers::NONE
            },
        ),
    )
    .unwrap();
    drain(&mut f.sim);
    assert_eq!(last_act(&f).0, "screen-shortcut");
    assert_eq!(last_act(&f).1.screen.as_deref(), Some("items"));
    let gate = hold_view(&mut f, "items", false);
    resize_while_held(&mut f, &gate);
    let before = f.log.lock().unwrap().acts.len();
    f.sim.key('q');
    f.sim.key('z');
    assert_eq!(f.log.lock().unwrap().acts.len(), before);
    gate.release.open();
    drain(&mut f.sim);
    assert_eq!(last_act(&f).0, "tree-shortcut");
    assert_eq!(last_act(&f).1.screen, None);
    open(&mut f);
    let gate = hold_view(&mut f, "items", false);
    f.sim.resize(158, 24);
    gate.entered.wait();
    assert!(gate.entered.is_open());
    let before = f.log.lock().unwrap().acts.len();
    f.sim.key('q');
    f.sim.key('z');
    f.sim.key('t');
    gate.release.open();
    drain(&mut f.sim);
    f.sim.key('[');
    drain(&mut f.sim);
    assert_eq!(
        f.log.lock().unwrap().acts.len(),
        before,
        "a queued tree shortcut must not survive leaving its tab"
    );
}

#[test]
fn e2e_project_rendering_reuses_styled_list_text_diff_and_document_widgets() {
    use ratatui::style::{Color, Modifier};
    let mut f = fixture("project_render_widgets", false, 1, None, None, None);
    open(&mut f);
    f.sim.see("● [x] alpha styled");
    f.sim.see("[ ] beta");
    let cells = f.sim.term.backend().buffer().content();
    assert!(cells
        .iter()
        .any(|c| c.symbol() == "a" && c.fg == Color::Red && c.modifier.contains(Modifier::BOLD)));
    assert!(cells.iter().any(|c| c.fg == Color::Green));
    assert!(cells
        .iter()
        .any(|c| c.modifier.contains(Modifier::REVERSED)));
    f.sim.key('z');
    drain(&mut f.sim);
    f.sim.see("text selection=none");
    f.sim.key('d');
    drain(&mut f.sim);
    f.sim.see("project addition");
    f.sim.see("project removal");
    f.sim.key('b');
    drain(&mut f.sim);
    f.sim.see("strong body");
    f.sim.dont_see("**strong body**");
}

#[test]
fn e2e_project_shared_dialog_answers_reach_the_act_context() {
    let mut f = fixture("project_dialog_answers", false, 1, None, None, None);
    open(&mut f);
    f.sim.press(KeyCode::F(5), KeyModifiers::NONE);
    drain(&mut f.sim);
    f.sim.see("Proceed?");
    assert!(f.log.lock().unwrap().acts.is_empty());
    f.sim.key('y');
    f.sim.see("Name");
    f.sim.keys("追加");
    f.sim.enter();
    f.sim.see("Target");
    f.sim.key('j');
    f.sim.enter();
    drain(&mut f.sim);
    let (action, context) = last_act(&f);
    assert_eq!(action, "answers");
    assert_eq!(context.confirmation.as_deref(), Some("accepted"));
    assert_eq!(context.text.as_deref(), Some("seed追加"));
    assert_eq!(context.choice.as_deref(), Some("second"));
    assert_eq!(context.screen.as_deref(), Some("items"));
    assert_eq!(
        context.selected,
        Some(ProjectRow::new("item", "opaque-alpha"))
    );
    assert!(f.sim.app.project_dialog.is_none());
}

#[test]
fn e2e_project_confirmation_chooses_the_declared_value() {
    let mut f = fixture("project_confirmation_choice", false, 1, None, None, None);
    open(&mut f);
    f.sim.press(KeyCode::F(2), KeyModifiers::NONE);
    drain(&mut f.sim);
    f.sim.key('!');
    drain(&mut f.sim);
    assert_eq!(last_act(&f).1.confirmation.as_deref(), Some("force-value"));
}

#[test]
fn e2e_project_confirmation_row_name_uses_invocation_after_cursor_moves() {
    let mut f = fixture_with_log(
        "project_prompt_row_name",
        false,
        1,
        None,
        None,
        None,
        ProjectLog {
            row_name: Some("short.txt".into()),
            ..Default::default()
        },
    );
    open(&mut f);
    f.sim.press(KeyCode::F(2), KeyModifiers::CONTROL);
    f.sim.see("Discard short.txt? Again short.txt; {other}");
    f.sim
        .app
        .tab
        .project_navigation
        .as_mut()
        .unwrap()
        .stack
        .last_mut()
        .unwrap()
        .cursor = 2;
    f.sim.draw();
    f.sim.see("Discard short.txt? Again short.txt; {other}");
    f.sim.key('y');
    drain(&mut f.sim);
    assert_eq!(
        last_act(&f).1.selected,
        Some(ProjectRow::new("item", "opaque-alpha").name("short.txt"))
    );
}

#[test]
fn e2e_project_confirmation_row_value_and_empty_selection() {
    let mut f = fixture("project_prompt_row_value", false, 1, None, None, None);
    open(&mut f);
    f.sim.press(KeyCode::F(2), KeyModifiers::CONTROL);
    f.sim
        .see("Discard opaque-alpha? Again opaque-alpha; {other}");
    f.sim.esc();
    f.sim.key('/');
    f.sim.keys("no matching rows");
    f.sim.enter();
    f.sim.press(KeyCode::F(2), KeyModifiers::CONTROL);
    f.sim.see("Discard ? Again ; {other}");
}

#[test]
fn e2e_project_confirmation_row_localized_and_refreshed() {
    let mut f = fixture_with_log(
        "project_prompt_row_refresh",
        false,
        1,
        None,
        None,
        None,
        ProjectLog {
            row_name: Some("{row}.txt".into()),
            ..Default::default()
        },
    );
    open(&mut f);
    f.sim.app.lang = crate::i18n::Lang::Jp;
    f.sim.press(KeyCode::F(2), KeyModifiers::CONTROL);
    f.sim.see("JA {row}.txt / {row}.txt");
    f.log.lock().unwrap().row_name = Some("changed.txt".into());
    f.log.lock().unwrap().reverse_rows = true;
    let views = f.log.lock().unwrap().views.len();
    f.sim.resize(159, 24);
    drain(&mut f.sim);
    assert!(f.log.lock().unwrap().views.len() > views);
    f.sim.see("JA {row}.txt / {row}.txt");
}

fn conditional_confirmation(name: &str, value: Option<toml::Value>, default: bool, asks: bool) {
    let mut f = fixture_with_log(
        name,
        false,
        1,
        None,
        None,
        None,
        ProjectLog {
            prompt_value: value,
            prompt_default: Some(default),
            ..Default::default()
        },
    );
    open(&mut f);
    f.sim.press(KeyCode::F(3), KeyModifiers::CONTROL);
    drain(&mut f.sim);
    assert_eq!(f.sim.app.project_dialog.is_some(), asks);
    if asks {
        f.sim.see("Sync project?");
        assert!(f.log.lock().unwrap().acts.is_empty());
        f.sim.key('y');
        drain(&mut f.sim);
    }
    assert_eq!(last_act(&f).0, "conditional");
    assert_eq!(
        last_act(&f).1.confirmation.as_deref(),
        asks.then_some("yes")
    );
    assert_eq!(f.log.lock().unwrap().prompt_settings, vec![asks]);
}

#[test]
fn e2e_project_confirmation_setting_true_asks() {
    conditional_confirmation(
        "project_prompt_true",
        Some(toml::Value::Boolean(true)),
        false,
        true,
    );
}

#[test]
fn e2e_project_confirmation_setting_false_runs_act() {
    conditional_confirmation(
        "project_prompt_false",
        Some(toml::Value::Boolean(false)),
        true,
        false,
    );
}

#[test]
fn e2e_project_confirmation_setting_missing_uses_default() {
    conditional_confirmation("project_prompt_missing_true", None, true, true);
    conditional_confirmation("project_prompt_missing_false", None, false, false);
}

#[test]
fn e2e_project_confirmation_setting_non_boolean_asks() {
    conditional_confirmation(
        "project_prompt_non_boolean",
        Some(toml::Value::String("false".into())),
        false,
        true,
    );
}

#[test]
fn e2e_project_confirmation_setting_false_keeps_later_inputs() {
    let mut f = fixture_with_log(
        "project_prompt_later_inputs",
        false,
        1,
        None,
        None,
        None,
        ProjectLog {
            prompt_value: Some(toml::Value::Boolean(false)),
            ..Default::default()
        },
    );
    open(&mut f);
    f.sim.press(KeyCode::F(4), KeyModifiers::CONTROL);
    f.sim.see("Sync message");
    f.sim.dont_see("Sync project?");
    assert!(f.log.lock().unwrap().acts.is_empty());
    f.sim.keys(" edit");
    f.sim.enter();
    f.sim.see("Sync target");
    f.sim.enter();
    drain(&mut f.sim);
    let (action, context) = last_act(&f);
    assert_eq!(action, "conditional-input");
    assert_eq!(context.confirmation, None);
    assert_eq!(context.text.as_deref(), Some("seed edit"));
    assert_eq!(context.choice.as_deref(), Some("all"));
    assert_eq!(f.log.lock().unwrap().prompt_settings, vec![false]);
}

#[test]
fn e2e_project_text_input_edits_unicode_initial_value() {
    let mut f = fixture("project_text_input", false, 1, None, None, None);
    open(&mut f);
    f.sim.press(KeyCode::F(3), KeyModifiers::NONE);
    drain(&mut f.sim);
    f.sim.see("Project name");
    assert!(
        matches!(&f.sim.app.project_dialog, Some(crate::app::project_screens::ProjectDialog::Act { buffer, .. }) if buffer == "初期")
    );
    f.sim.press(KeyCode::Left, KeyModifiers::NONE);
    f.sim.press(KeyCode::Backspace, KeyModifiers::NONE);
    f.sim.key('新');
    f.sim.enter();
    drain(&mut f.sim);
    assert_eq!(last_act(&f).1.text.as_deref(), Some("新期"));
}

#[test]
fn e2e_project_choice_input_returns_value_not_label() {
    let mut f = fixture("project_choice_input", false, 1, None, None, None);
    open(&mut f);
    f.sim.press(KeyCode::F(4), KeyModifiers::NONE);
    drain(&mut f.sim);
    f.sim.key('j');
    f.sim.enter();
    drain(&mut f.sim);
    assert_eq!(last_act(&f).1.choice.as_deref(), Some("two-value"));
}

macro_rules! cancel_dialog {
    ($name:ident, $key:expr) => {
        #[test]
        fn $name() {
            let mut f = fixture(stringify!($name), false, 1, None, None, None);
            open(&mut f);
            f.sim.press(KeyCode::F($key), KeyModifiers::NONE);
            drain(&mut f.sim);
            assert!(f.sim.app.project_dialog.is_some());
            f.sim.esc();
            drain(&mut f.sim);
            assert!(f.sim.app.project_dialog.is_none());
            assert!(f.log.lock().unwrap().acts.is_empty());
            f.sim.key('x');
            drain(&mut f.sim);
            assert_eq!(last_act(&f).0, "capture");
        }
    };
}
cancel_dialog!(e2e_project_confirmation_esc_runs_nothing, 2);
cancel_dialog!(e2e_project_text_input_esc_runs_nothing, 3);
cancel_dialog!(e2e_project_choice_input_esc_runs_nothing, 4);

macro_rules! refresh {
    ($name:ident, $key:expr, $screens:expr, $tree:expr, $chip:expr, $watch:expr) => {
        #[test]
        fn $name() {
            let mut f = fixture(stringify!($name), false, 1, None, None, None);
            open(&mut f);
            f.log.lock().unwrap().views.clear();
            let manifests = f.log.lock().unwrap().manifests;
            let chips = f.log.lock().unwrap().chips;
            let watches = f.log.lock().unwrap().watch_threads.len();
            f.log.lock().unwrap().chip = Some(konoma_plugin::Chip::new("updated chip"));
            f.sim.app.clear_project_tree_dirty();
            handle_key(
                &mut f.sim.app,
                KeyEvent::new(KeyCode::F($key), KeyModifiers::NONE),
            )
            .unwrap();
            while f.sim.app.project_jobs_pending() {
                f.sim.app.wait_project_job();
            }
            assert_eq!(f.sim.app.project_tree_dirty(), $tree);
            f.sim.key('q');
            f.sim.see(if $chip == 1 {
                "updated chip"
            } else {
                "project chip"
            });
            let log = f.log.lock().unwrap();
            let mut screens: Vec<_> = log.views.iter().map(|view| view.screen.as_str()).collect();
            screens.sort();
            assert_eq!(screens, $screens);
            assert_eq!(log.manifests - manifests, 0);
            assert_eq!(log.chips - chips, $chip);
            assert_eq!(log.watch_threads.len(), watches + $watch);
        }
    };
}
refresh!(
    e2e_project_refresh_current_screen_exactly,
    6,
    vec!["items"],
    false,
    0,
    1
);
refresh!(
    e2e_project_refresh_named_screen_exactly,
    7,
    vec!["text"],
    false,
    0,
    1
);
refresh!(
    e2e_project_refresh_all_screens_exactly,
    8,
    vec!["diff", "doc", "items", "text"],
    false,
    0,
    1
);
refresh!(
    e2e_project_refresh_tree_status_exactly,
    9,
    Vec::<&str>::new(),
    true,
    0,
    1
);
refresh!(
    e2e_project_refresh_chip_exactly,
    10,
    Vec::<&str>::new(),
    false,
    1,
    0
);
refresh!(
    e2e_project_refresh_none_rereads_watch,
    11,
    Vec::<&str>::new(),
    false,
    0,
    1
);
refresh!(
    e2e_project_refresh_current_screen_and_chip_rereads_watch,
    12,
    vec!["items"],
    false,
    1,
    1
);
refresh!(
    e2e_project_refresh_tree_status_and_chip_rereads_watch,
    1,
    Vec::<&str>::new(),
    true,
    1,
    1
);

#[test]
fn e2e_project_no_implicit_refresh_and_one_owner_per_project_across_tabs() {
    let hold = Arc::new(Latch::default());
    let started = Arc::new(Latch::default());
    let mut f = fixture(
        "project_serial_owner",
        false,
        1,
        None,
        Some(hold.clone()),
        Some(started.clone()),
    );
    open(&mut f);
    f.log.lock().unwrap().views.clear();
    f.sim.key('x');
    assert!(
        f.sim.app.project_jobs_pending(),
        "the act must have been dispatched before waiting for its worker"
    );
    f.sim.app.wait_project_view();
    started.wait();
    assert!(started.is_open());
    assert!(f.sim.app.project_jobs_pending());
    assert!(!crate::ui::status::hint_tokens(&f.sim.app)
        .iter()
        .any(|t| t.contains("capture")));
    f.sim.key('x');
    f.sim.key('t');
    assert!(!f.sim.app.project_active());
    f.sim.app.tab.projects.wait_for_result();
    f.sim.app.poll_projects();
    f.sim.key('p');
    f.sim.key('x');
    assert_eq!(f.log.lock().unwrap().acts.len(), 1);
    hold.open();
    drain(&mut f.sim);
    let log = f.log.lock().unwrap();
    assert_eq!(log.opens, 1);
    assert!(log.views.is_empty(), "Flash must not refresh a view");
}

#[test]
fn e2e_project_plugin_declared_read_only_stops_a_write_with_reason() {
    let mut f = fixture("project_declared_read_only", true, 1, None, None, None);
    open(&mut f);
    assert!(!crate::ui::status::hint_tokens(&f.sim.app)
        .iter()
        .any(|t| t.contains("write")));
    f.sim.key('w');
    assert_eq!(
        f.sim.app.flash.as_deref(),
        Some(crate::i18n::tr(
            f.sim.app.lang,
            crate::i18n::Msg::PluginReadOnly
        ))
    );
    assert!(f.log.lock().unwrap().acts.is_empty());
    f.sim.key('x');
    drain(&mut f.sim);
    assert_eq!(last_act(&f).0, "capture");
}

#[test]
fn e2e_project_multiple_matches_open_choice_menu_and_esc_cancels() {
    let mut f = fixture("project_choice_matches", false, 2, None, None, None);
    f.sim.key('p');
    assert!(!f.sim.app.project_active());
    f.sim.see("Choose a project");
    f.sim.esc();
    assert!(f.sim.app.project_dialog.is_none());
    assert!(!f.sim.app.project_active());
    f.sim.key('p');
    f.sim.key('j');
    f.sim.enter();
    drain(&mut f.sim);
    assert_eq!(
        f.sim
            .app
            .tab
            .project_navigation
            .as_ref()
            .unwrap()
            .project
            .marker,
        "1.screen-project"
    );
}

#[test]
fn e2e_project_enter_from_preview_returns_to_the_same_file() {
    let mut f = fixture("project_preview_origin", false, 1, None, None, None);
    f.sim.select("file.txt");
    f.sim.enter();
    open(&mut f);
    f.sim.key('q');
    assert_eq!(f.sim.app.tab.mode, Mode::Preview);
    assert!(f
        .sim
        .app
        .tab
        .preview_path
        .as_ref()
        .unwrap()
        .ends_with("file.txt"));
    f.sim.see("three");
}

#[test]
fn e2e_project_tree_selection_push_carries_selection_and_returns_to_tree() {
    let mut f = fixture(
        "project_tree_push",
        false,
        1,
        Some(Outcome::PushScreen {
            screen: "text".into(),
            selection: Some("from-tree".into()),
        }),
        None,
        None,
    );
    f.sim.select("file.txt");
    f.sim.key(' ');
    let ctx = f.sim.app.key_context().unwrap();
    let menu = crate::ops::menu::menu(
        &f.sim.app,
        ctx,
        f.sim.app.pending_leader.unwrap(),
        crate::ops::menu::View::Open,
    )
    .unwrap();
    let key = menu
        .items
        .iter()
        .find(
            |item| matches!(item.action, crate::keymap::Action::Plugin(p) if p.act() == "tree-act"),
        )
        .unwrap()
        .key;
    handle_key(
        &mut f.sim.app,
        KeyEvent::new(
            key.code,
            if key.ctrl {
                KeyModifiers::CONTROL
            } else {
                KeyModifiers::NONE
            },
        ),
    )
    .unwrap();
    drain(&mut f.sim);
    assert!(f.sim.app.project_active());
    f.sim.see("text selection=from-tree");
    assert_eq!(
        f.log.lock().unwrap().views.last(),
        Some(&view_context(&f, "text").selection("from-tree"))
    );
    let (_, context) = last_act(&f);
    assert_eq!(context.screen, None);
    assert_eq!(
        context.selected,
        Some(ProjectRow::new(
            "file",
            f.sim.app.tab.root.join("file.txt").to_string_lossy()
        ))
    );
    f.sim.key('q');
    assert!(!f.sim.app.project_active());
    assert_eq!(f.sim.app.tab.mode, Mode::Tree);
}

fn navigate(name: &str, outcome: impl FnOnce(&std::path::Path) -> Outcome) -> Fixture {
    let mut f = fixture(name, false, 1, None, None, None);
    // Register a fresh configuration before opening the fixture's session.
    let root = f.sim.app.tab.root.clone();
    let outcome = outcome(&root);
    let diff = matches!(outcome, Outcome::OpenDiff { .. });
    #[cfg(feature = "git")]
    if diff {
        seed_repo(&f._dir);
        std::fs::write(root.join("file.txt"), "changed project file\n").unwrap();
    }
    let marker = test_plugins::register(
        &format!("{name}-navigation"),
        Arc::new(Screens {
            log: f.log.clone(),
            read_only: false,
            hold: None,
            started: None,
            outcome: Some(outcome),
        }),
    );
    let mut cfg = test_plugins::marker_config(&marker);
    cfg.ui.lang = "en".into();
    cfg.external.git = diff;
    f.sim = Sim::with_config_sized(&root, cfg, 160, 24);
    f.sim.app.tab.projects.wait_for_result();
    assert!(f.sim.app.tab.projects.uses_injected_events());
    f.sim.app.poll_projects();
    drain(&mut f.sim);
    open(&mut f);
    press(&mut f, KeyCode::F(11));
    f
}

#[test]
fn e2e_project_outcome_open_file_uses_line_and_q_returns_to_project() {
    let mut f = navigate("project_outcome_file", |root| Outcome::OpenFile {
        path: root.join("file.txt"),
        line: Some(3),
    });
    assert!(!f.sim.app.project_active());
    assert_eq!(f.sim.app.tab.mode, Mode::Preview);
    assert_eq!(f.sim.app.preview_caret_line(), 2);
    f.sim.see("three");
    assert!(f
        .sim
        .app
        .tab
        .preview_path
        .as_ref()
        .unwrap()
        .ends_with("file.txt"));
    f.sim.key('q');
    drain(&mut f.sim);
    assert!(f.sim.app.project_active());
    f.sim.see("alpha");
}

#[test]
fn e2e_project_outcome_open_diff_uses_host_diff() {
    let f = navigate("project_outcome_diff", |root| Outcome::OpenDiff {
        path: root.join("file.txt"),
    });
    assert!(!f.sim.app.project_active());
    #[cfg(feature = "git")]
    {
        assert!(f.sim.app.is_git_diff_preview());
        assert_eq!(
            f.sim.app.tab.preview_path.as_ref(),
            Some(&f.sim.app.tab.root.join("file.txt"))
        );
        f.sim.see("changed project file");
    }
    #[cfg(not(feature = "git"))]
    assert!(f.sim.app.flash.is_some());
}

#[test]
fn e2e_project_outcome_switch_root_uses_worktree_root_path() {
    let f = navigate("project_outcome_root", |root| Outcome::SwitchRoot {
        root: root.join("destination"),
    });
    assert_eq!(f.sim.app.tab.root, canon(&f._dir.join("destination")));
    assert!(!f.sim.app.project_active());
    assert_eq!(f.sim.app.tab.mode, Mode::Tree);
}

#[test]
fn e2e_project_outcome_open_tab_uses_host_tab_lifecycle() {
    let mut f = navigate("project_outcome_tab", |root| Outcome::OpenTab {
        path: root.join("destination"),
    });
    assert_eq!(f.sim.app.tab_count(), 2);
    assert_eq!(f.sim.app.tab.root, canon(&f._dir.join("destination")));
    assert!(!f.sim.app.project_active());
    f.sim.key('[');
    drain(&mut f.sim);
    assert!(f.sim.app.project_active());
    f.sim.see("alpha");
}
