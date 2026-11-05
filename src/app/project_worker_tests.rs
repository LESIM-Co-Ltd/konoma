//! Project owner worker contracts, including independent watch and chip replies.

use super::*;
use konoma_plugin::proc::{
    client::{Config, ProcPlugin, Spawned},
    frame,
    proto::{Answer, FromChild, Hello, HelloReply, Reply, ToChild, WState},
    wire::{layout_to_wire, manifest_to_wire},
};
use konoma_plugin::{Manifest, ProjectDecl};
use std::os::unix::net::UnixStream;
use std::time::Duration;

#[test]
fn project_diff_window_cells_match_full_render_at_headers_and_inside_change_blocks() {
    use crate::git::{DiffLine, DiffLineKind};
    use crate::preview::gitdiff::{diff_lines, diff_lines_side_by_side};
    use ratatui::{
        backend::TestBackend,
        layout::Rect,
        style::{Color, Style},
        text::Line,
        widgets::Paragraph,
        Terminal,
    };
    let mut data = Vec::new();
    for path in ["main.rs", "data.json"] {
        data.push(DiffLine {
            kind: DiffLineKind::Context,
            old_no: None,
            new_no: None,
            text: path.into(),
        });
        for kind in [
            DiffLineKind::Context,
            DiffLineKind::Removed,
            DiffLineKind::Added,
            DiffLineKind::Context,
        ] {
            for i in 0..12 {
                data.push(DiffLine {
                    kind,
                    old_no: (!matches!(kind, DiffLineKind::Added)).then_some(i + 1),
                    new_no: (!matches!(kind, DiffLineKind::Removed)).then_some(i + 1),
                    text: format!("let value_{i} = {kind:?}; // 日本語 {}", "long ".repeat(25)),
                });
            }
        }
    }
    let header = vec![
        Line::styled(
            "commit header 日本語 ".repeat(40),
            Style::new().fg(Color::Yellow),
        ),
        Line::raw("subject"),
        Line::raw(""),
    ];
    let diff = crate::app::project_screens::ProjectDiff::new(
        data.clone(),
        header.clone(),
        "TwoDark".into(),
        "No changes".into(),
    );
    for width in [30, 80, 180] {
        for split in [false, true] {
            for offset in [0, 8, u16::MAX] {
                let (total, max_h) = diff.metrics(width, split);
                let offset = offset.min(max_h);
                let mut full = header.clone();
                full.push(Line::styled(
                    "─".repeat(usize::from(width)),
                    Style::new().fg(Color::DarkGray),
                ));
                full.extend(if split {
                    diff_lines_side_by_side(
                        &data,
                        "",
                        "TwoDark",
                        usize::from(width),
                        usize::from(offset),
                    )
                } else {
                    diff_lines(&data, "", "TwoDark", usize::from(width))
                });
                assert_eq!(total, full.len());
                if !split {
                    assert_eq!(
                        usize::from(max_h),
                        full.iter()
                            .map(Line::width)
                            .max()
                            .unwrap()
                            .saturating_sub(usize::from(width))
                    );
                }
                // Every start includes windows straddling a file header, and starting inside
                // either half of an unequal-in-view Removed/Added block.
                for start in 0..=total {
                    let window = diff.layout(width, split, offset, start, 9);
                    let render = |lines, scroll| {
                        let mut term = Terminal::new(TestBackend::new(width, 9)).unwrap();
                        term.draw(|f| {
                            f.render_widget(
                                Paragraph::new(lines)
                                    .scroll((scroll, if split { 0 } else { offset })),
                                Rect::new(0, 0, width, 9),
                            )
                        })
                        .unwrap()
                        .buffer
                        .clone()
                    };
                    assert_eq!(
                        render(window.lines.clone(), 0),
                        render(full.clone(), start as u16),
                        "width={width}, split={split}, offset={offset}, start={start}"
                    );
                }
            }
        }
    }
}

#[test]
fn project_diff_content_keeps_styled_header_and_one_marker_per_changed_path() {
    use konoma_plugin::view::{DiffHunk, DiffKind, DiffLine, DiffView};
    let hunks = ["main.rs", "main.rs", "data.json"]
        .into_iter()
        .map(|path| {
            DiffHunk::new(vec![DiffLine {
                kind: DiffKind::Added,
                old_no: None,
                new_no: Some(1),
                text: "body".into(),
            }])
            .path(path)
        })
        .collect();
    let header = konoma_plugin::Line::from(vec![konoma_plugin::Span::styled(
        "header",
        konoma_plugin::Style::new().bold(),
    )]);
    let view = content(
        View::Diff(DiffView::new(hunks).header(vec![header.clone()])),
        &ViewReq::new(80, 20),
        &ReadScope::unrestricted(),
        "base16-ocean.dark",
        "en",
    )
    .unwrap();
    let ProjectContent::Diff(diff) = view else {
        panic!("diff required")
    };
    assert_eq!(
        diff.header,
        vec![konoma_plugin::ratatui_draw::into_line(header)]
    );
    assert_eq!(
        diff.lines
            .iter()
            .map(|l| l.text.as_str())
            .collect::<Vec<_>>(),
        ["main.rs", "body", "body", "data.json", "body"]
    );
    for index in [0, 3] {
        assert_eq!(diff.lines[index].old_no, None);
        assert_eq!(diff.lines[index].new_no, None);
    }
}

#[test]
fn project_initial_watch_and_chip_failures_are_independent_and_chip_job_skips_watch() {
    use crate::plugins::test_projects::{ProjectLog, Screens};
    use std::sync::Mutex;

    let dir = crate::test_support::unique_tmp("konoma_project_initial_watch_chip");
    std::fs::create_dir_all(&dir).unwrap();
    for invalid_watch in [false, true] {
        for chip_mode in 0..3 {
            let log = Arc::new(Mutex::new(ProjectLog {
                watch_paths: Some(if invalid_watch {
                    vec!["relative".into()]
                } else {
                    Vec::new()
                }),
                chip: Some(Chip::new("computed chip")),
                chip_failure: chip_mode == 1,
                chip_panic: chip_mode == 2,
                ..Default::default()
            }));
            let plugin = Arc::new(Screens {
                log: log.clone(),
                read_only: false,
                hold: None,
                started: None,
                outcome: None,
            });
            let runtime = ProjectRuntime::start(
                crate::plugins::projects::MatchedProject {
                    session_watch_paths: None,
                    plugin: "project-screens".into(),
                    root: dir.to_path_buf(),
                    marker: "project".into(),
                    declaration: plugin.manifest().project.unwrap(),
                },
                plugin,
                Vec::new(),
                Settings::empty().clone(),
                "ja".into(),
                String::new(),
                TermCaps::new(),
                None,
                false,
                None,
                "worker failed".into(),
            );
            let initial = runtime.rx.recv_timeout(Duration::from_secs(20)).unwrap();
            assert_eq!(initial.error.is_some(), invalid_watch);
            assert_eq!(initial.watch_paths, (!invalid_watch).then(Vec::new));
            assert_eq!(initial.chip_error.is_some(), chip_mode != 0);
            let expected = if chip_mode == 0 {
                "computed chip"
            } else {
                "宣言チップ"
            };
            assert_eq!(initial.chip.unwrap().unwrap().text.plain(), expected);
            assert_eq!(log.lock().unwrap().chips, 1);
            assert_eq!(log.lock().unwrap().watch_threads.len(), 1);

            runtime.tx.send(ProjectJob::Chip).unwrap();
            let reply = runtime.rx.recv_timeout(Duration::from_secs(20)).unwrap();
            assert!(reply.error.is_none());
            assert!(reply.watch_paths.is_none());
            assert_eq!(reply.chip_error.is_some(), chip_mode != 0);
            assert_eq!(reply.chip.unwrap().unwrap().text.plain(), expected);
            assert_eq!(log.lock().unwrap().chips, 2);
            assert_eq!(log.lock().unwrap().watch_threads.len(), 1);

            let screen = ScreenKey {
                screen: "text".into(),
                selection: None,
            };
            let context = ProjectView::new("text").paths(&dir, &dir);
            let act_paths = vec![dir.join("after-act/index")];
            log.lock().unwrap().watch_paths = Some(act_paths.clone());
            let effect = ProjectEffect {
                project: ProjectKey::from(&runtime.project),
                tab: 0,
                tree_root: dir.to_path_buf(),
                view: None,
                outcome: None,
                tree_status: false,
                label: "current".into(),
            };
            runtime
                .tx
                .send(ProjectJob::Act {
                    action: "current".into(),
                    context: konoma_plugin::ProjectAct::new().screen("items"),
                    refresh: vec![(
                        screen.clone(),
                        ViewReq::new(80, 24).project(context.clone()),
                    )],
                    chip: true,
                    watch: true,
                    effect: Box::new(effect.clone()),
                })
                .unwrap();
            let reply = runtime.rx.recv_timeout(Duration::from_secs(20)).unwrap();
            assert!(reply.error.is_none());
            assert_eq!(reply.watch_paths, Some(act_paths));
            assert_eq!(reply.chip_error.is_some(), chip_mode != 0);
            assert_eq!(reply.chip.unwrap().unwrap().text.plain(), expected);
            assert_eq!(reply.views.len(), 1);
            assert_eq!(reply.views[0].0, screen);
            assert_eq!(reply.contexts.get(&screen), Some(&context));
            assert!(matches!(
                reply.effect.unwrap().outcome,
                Some(konoma_plugin::Outcome::Flash(message)) if message.get("en") == "did current"
            ));
            assert_eq!(log.lock().unwrap().chips, 3);
            assert_eq!(log.lock().unwrap().watch_threads.len(), 2);

            // A chip-only action must not even ask for these invalid watch paths.
            log.lock().unwrap().watch_paths = Some(vec!["relative".into()]);
            runtime
                .tx
                .send(ProjectJob::Act {
                    action: "chip".into(),
                    context: konoma_plugin::ProjectAct::new().screen("items"),
                    refresh: Vec::new(),
                    chip: true,
                    watch: false,
                    effect: Box::new(effect),
                })
                .unwrap();
            let reply = runtime.rx.recv_timeout(Duration::from_secs(20)).unwrap();
            assert!(reply.error.is_none());
            assert!(reply.watch_paths.is_none());
            assert!(reply.views.is_empty());
            assert_eq!(reply.chip_error.is_some(), chip_mode != 0);
            assert_eq!(reply.chip.unwrap().unwrap().text.plain(), expected);
            assert!(matches!(
                reply.effect.unwrap().outcome,
                Some(konoma_plugin::Outcome::Flash(message)) if message.get("en") == "did chip"
            ));
            assert_eq!(log.lock().unwrap().chips, 4);
            assert_eq!(log.lock().unwrap().watch_threads.len(), 2);
        }
    }
}

#[test]
fn project_chip_old_child_uses_localized_declaration_without_unknown_call() {
    let dir = crate::test_support::unique_tmp("konoma_project_chip_old_child");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::create_dir_all(dir.join("side")).unwrap();
    let declaration = ProjectDecl::new().chip(Tr::new("declared chip").ja("宣言チップ"));
    let manifest = Manifest::new("old-project", "1").project(declaration.clone());
    let (host, mut child) = UnixStream::pair().unwrap();
    let peer = std::thread::spawn(move || {
        let _: Hello = frame::recv(&mut child).unwrap().unwrap();
        let mut manifest = manifest_to_wire(&manifest).unwrap();
        manifest.session_chip = false;
        frame::send(
            &mut child,
            &HelloReply::Ack {
                version: 2,
                manifest,
            },
        )
        .unwrap();
        let ToChild::Open { rid, .. } = frame::recv(&mut child).unwrap().unwrap() else {
            panic!("expected project open");
        };
        frame::send(
            &mut child,
            &FromChild {
                rid,
                answer: Answer::Ok(Reply::Opened {
                    state: WState {
                        layout: layout_to_wire(&konoma_plugin::view::Layout::single()),
                        enabled: Vec::new(),
                    },
                }),
            },
        )
        .unwrap();
        loop {
            match frame::recv::<_, ToChild>(&mut child).unwrap() {
                Some(ToChild::Close { .. }) => {}
                Some(ToChild::Shutdown) | None => break,
                other => panic!("old child received unknown call: {other:?}"),
            }
        }
    });
    let connection = Arc::new(std::sync::Mutex::new(Some(host)));
    let plugin = Arc::new(
        ProcPlugin::start(
            move || {
                let host = connection.lock().unwrap().take().expect("one launch");
                let kill = host.try_clone()?;
                Ok(Spawned {
                    reader: Box::new(host.try_clone()?),
                    writer: Box::new(host),
                    stderr: None,
                    pid: None,
                    kill: Box::new(move || {
                        let _ = kill.shutdown(std::net::Shutdown::Both);
                    }),
                })
            },
            Config {
                scratch_root: dir.join("side"),
                ..Default::default()
            },
        )
        .unwrap(),
    );
    let runtime = ProjectRuntime::start(
        crate::plugins::projects::MatchedProject {
            session_watch_paths: None,
            plugin: "old-project".into(),
            root: dir.to_path_buf(),
            marker: "project".into(),
            declaration,
        },
        plugin.clone(),
        Vec::new(),
        Settings::empty().clone(),
        "ja".into(),
        String::new(),
        TermCaps::new(),
        None,
        false,
        None,
        "worker failed".into(),
    );
    let reply = runtime.rx.recv_timeout(Duration::from_secs(20)).unwrap();
    assert!(reply.error.is_none());
    assert!(reply.chip_error.is_none());
    assert_eq!(reply.chip.unwrap().unwrap().text.plain(), "宣言チップ");
    drop(runtime);
    plugin.stop();
    peer.join().unwrap();
}
