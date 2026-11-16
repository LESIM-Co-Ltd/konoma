use super::navigation::Services;
use konoma_host_core::shared::SharedConfig;
use konoma_plugin::app::project::{
    prepare_projects, run_project_with, PreparedProjects, ProjectLoopCx,
};
use konoma_plugin::app::{EventSource, Next};
use konoma_plugin::TermCaps;
use konoma_test_support::TmpDir;
use ratatui::{backend::TestBackend, Terminal};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

#[path = "wake_fixture.rs"]
mod fixture;
use fixture::{Log, WatchProject};

fn setup() -> (
    TmpDir,
    PreparedProjects,
    mpsc::Receiver<()>,
    Arc<Mutex<Log>>,
) {
    let dir = konoma_test_support::unique_tmp("project_wake");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("project.marker"), "").unwrap();
    let dir = dir.into_canonical().unwrap();
    let log = Arc::new(Mutex::new(Log {
        paths: vec![dir.join("first/index")],
        ..Default::default()
    }));
    let mut prepared = prepare_projects(
        vec![Arc::new(WatchProject(log.clone()))],
        &dir,
        TermCaps::new(),
        (SharedConfig::parse("[ui]\nlang='en'\n").unwrap(), None),
        Arc::new(Services),
    )
    .unwrap_or_else(|e| panic!("{}", e.error));
    let (ready, replies) = mpsc::channel();
    for runtime in prepared.runtimes.values_mut() {
        let (tx, rx) = mpsc::sync_channel(1);
        let owner = std::mem::replace(&mut runtime.rx, rx);
        let ready = ready.clone();
        std::thread::spawn(move || {
            while let Ok(reply) = owner.recv() {
                if tx.send(reply).is_err() || ready.send(()).is_err() {
                    break;
                }
            }
        });
    }
    (dir, prepared, replies, log)
}

struct Settled {
    replies: mpsc::Receiver<()>,
    fences: usize,
}
impl EventSource for Settled {
    fn next(&mut self, wait: Option<Duration>) -> Next {
        if wait == Some(Duration::from_millis(16)) {
            self.replies
                .recv_timeout(Duration::from_secs(10))
                .expect("frame work must have an owner reply");
            Next::Timeout
        } else {
            assert!(
                wait.is_some(),
                "a live watcher needs a finite idle deadline"
            );
            self.fences += 1;
            Next::End
        }
    }
}

#[test]
fn stable_frame_fence_does_not_wait_for_the_live_watcher_to_end() {
    let (_dir, mut prepared, replies, log) = setup();
    assert!(prepared.detection.watching());
    assert!(!prepared.detection.pending());
    let worker = prepared.detection.clone();
    let mut events = Settled { replies, fences: 0 };
    let mut terminal = Terminal::new(Probe {
        backend: Arc::new(Mutex::new(TestBackend::new(54, 14))),
        draws: Arc::new(Mutex::new(0)),
    })
    .unwrap();
    run_project_with(
        &mut terminal,
        &mut prepared,
        &mut events,
        &ProjectLoopCx::default(),
        None,
    )
    .unwrap();
    assert_eq!(events.fences, 1, "the frame settles with the watcher alive");
    assert!(worker.watching());
    assert_eq!(log.lock().unwrap().views, ["items"]);
}

// The observer sees the complete frame and the number of terminal flushes at an idle fence.
struct Probe {
    backend: Arc<Mutex<TestBackend>>,
    draws: Arc<Mutex<usize>>,
}
impl ratatui::backend::Backend for Probe {
    type Error = std::convert::Infallible;
    fn draw<'a, I>(&mut self, cells: I) -> Result<(), Self::Error>
    where
        I: Iterator<Item = (u16, u16, &'a ratatui::buffer::Cell)>,
    {
        self.backend.lock().unwrap().draw(cells)
    }
    fn hide_cursor(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
    fn show_cursor(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
    fn get_cursor_position(&mut self) -> Result<ratatui::layout::Position, Self::Error> {
        Ok((0, 0).into())
    }
    fn set_cursor_position<P: Into<ratatui::layout::Position>>(
        &mut self,
        _: P,
    ) -> Result<(), Self::Error> {
        Ok(())
    }
    fn clear(&mut self) -> Result<(), Self::Error> {
        self.backend.lock().unwrap().clear()
    }
    fn clear_region(&mut self, region: ratatui::backend::ClearType) -> Result<(), Self::Error> {
        self.backend.lock().unwrap().clear_region(region)
    }
    fn size(&self) -> Result<ratatui::layout::Size, Self::Error> {
        self.backend.lock().unwrap().size()
    }
    fn window_size(&mut self) -> Result<ratatui::backend::WindowSize, Self::Error> {
        self.backend.lock().unwrap().window_size()
    }
    fn flush(&mut self) -> Result<(), Self::Error> {
        *self.draws.lock().unwrap() += 1;
        Ok(())
    }
}

#[cfg(feature = "test-hooks")]
struct IdleEvents<'a> {
    replies: mpsc::Receiver<()>,
    idle: Box<dyn FnMut() -> Next + 'a>,
}
#[cfg(feature = "test-hooks")]
impl EventSource for IdleEvents<'_> {
    fn next(&mut self, wait: Option<Duration>) -> Next {
        if wait == Some(Duration::from_millis(16)) {
            self.replies
                .recv_timeout(Duration::from_secs(10))
                .expect("frame work must have an owner reply");
            Next::Timeout
        } else {
            assert!(
                wait.is_some(),
                "a live watcher needs a finite idle deadline"
            );
            (self.idle)()
        }
    }
}

#[cfg(feature = "test-hooks")]
fn assert_cells(buffer: &ratatui::buffer::Buffer, stage: usize) {
    let expected: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../snapshots/project_wake.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let expected = &expected[stage];
    let rect = ratatui::layout::Rect::new(0, 1, 54, 12);
    assert_eq!(expected["rect"], serde_json::json!([0, 1, 54, 12]));
    assert_eq!(expected["count"], 648);
    let cells: Vec<_> = expected["runs"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|run| std::iter::repeat_n(&run[1], run[0].as_u64().unwrap() as usize))
        .collect();
    assert_eq!(cells.len(), 648);
    let mut compared = 0;
    let mut nonempty = 0;
    for y in rect.y..rect.bottom() {
        for x in rect.x..rect.right() {
            let cell = &buffer[(x, y)];
            assert_eq!(
                serde_json::json!([
                    cell.symbol(),
                    format!("{:?}", cell.fg),
                    format!("{:?}", cell.bg),
                    cell.modifier.bits()
                ]),
                *cells[compared],
                "cell ({x}, {y})"
            );
            nonempty += usize::from(!cell.symbol().trim().is_empty());
            compared += 1;
        }
    }
    assert_eq!(compared, 648);
    assert!(nonempty > 0);
}

#[cfg(feature = "test-hooks")]
#[test]
fn idle_project_watch_updates_the_frame_without_a_key() {
    let (dir, mut prepared, replies, log) = setup();
    let detection = prepared.detection.clone();
    let backend = Arc::new(Mutex::new(TestBackend::new(54, 14)));
    let draws = Arc::new(Mutex::new(0));
    let mut terminal = Terminal::new(Probe {
        backend: backend.clone(),
        draws: draws.clone(),
    })
    .unwrap();
    let mut stage = 0;
    let mut settled_draws = 0;
    let mut events = IdleEvents {
        replies,
        idle: Box::new(|| {
            match stage {
                0 => {
                    assert_cells(backend.lock().unwrap().buffer(), 0);
                    log.lock().unwrap().changed = true;
                    // Only after the settled frame, publish through the real bounded queue/fence.
                    detection.inject_event_with_pending_watch_update(Ok(notify::Event::new(
                        notify::EventKind::Any,
                    )
                    .add_path(dir.join("first/index"))));
                }
                1 => {
                    assert_eq!(
                        log.lock().unwrap().views,
                        ["items", "items"],
                        "the idle timeout must adopt the watch update"
                    );
                    assert_cells(backend.lock().unwrap().buffer(), 1);
                    settled_draws = *draws.lock().unwrap();
                }
                2 => {
                    assert_eq!(
                        log.lock().unwrap().views.len(),
                        2,
                        "an empty timeout must not force a re-view"
                    );
                    assert_eq!(
                        *draws.lock().unwrap(),
                        settled_draws,
                        "an empty timeout must not redraw"
                    );
                    return Next::End;
                }
                _ => unreachable!(),
            }
            stage += 1;
            Next::Timeout
        }),
    };
    run_project_with(
        &mut terminal,
        &mut prepared,
        &mut events,
        &ProjectLoopCx::default(),
        None,
    )
    .unwrap();
    drop(events);
    assert_eq!(stage, 2);
}

#[cfg(feature = "test-hooks")]
#[test]
fn watch_paths_replacement_and_failure_keep_the_current_screen() {
    use konoma_plugin::app::AppEvent;
    use konoma_plugin::{Key, KeyCode};
    let (dir, mut prepared, replies, log) = setup();
    let detection = prepared.detection.clone();
    let backend = Arc::new(Mutex::new(TestBackend::new(54, 14)));
    let draws = Arc::new(Mutex::new(0));
    let mut terminal = Terminal::new(Probe {
        backend: backend.clone(),
        draws: draws.clone(),
    })
    .unwrap();
    let mut stage = 0;
    let mut settled_draws = 0;
    let inject = |path| {
        detection
            .inject_event_with_pending_watch_update(Ok(
                notify::Event::new(notify::EventKind::Any).add_path(path)
            ))
    };
    let mut events = IdleEvents {
        replies,
        idle: Box::new(|| {
            let step = stage;
            stage += 1;
            match step {
                0 | 1 => {
                    return Next::Event(AppEvent::Key(Key::new(KeyCode::Char(
                        fixture::CACHE_KEYS[step],
                    ))))
                }
                2 => {
                    assert_eq!(log.lock().unwrap().views, ["items", "text"]);
                    assert_cells(backend.lock().unwrap().buffer(), 0);
                    let mut log = log.lock().unwrap();
                    log.paths = vec![dir.join("second/index")];
                    log.changed = true;
                    drop(log);
                    inject(dir.join("first/index"));
                }
                3 => {
                    assert_eq!(log.lock().unwrap().views, ["items", "text", "items"], "Watch delivery must install session paths and refresh only the current screen");
                    assert_eq!(log.lock().unwrap().watches, 2);
                    assert_cells(backend.lock().unwrap().buffer(), 1);
                    settled_draws = *draws.lock().unwrap();
                    for path in ["first/index", "declared/index", "unrelated"] {
                        inject(dir.join(path));
                    }
                }
                4 => {
                    assert_eq!(
                        log.lock().unwrap().views.len(),
                        3,
                        "old, declared and unrelated paths must not invalidate"
                    );
                    assert_eq!(log.lock().unwrap().watches, 2);
                    assert_eq!(*draws.lock().unwrap(), settled_draws);
                    inject(dir.join("second/index"));
                }
                5 => {
                    assert_eq!(
                        log.lock().unwrap().views,
                        ["items", "text", "items", "items"],
                        "the replacement path must invalidate exactly once"
                    );
                    assert_eq!(log.lock().unwrap().watches, 3);
                    detection.inject_event_with_pending_watch_update(Err(notify::Error::generic(
                        "injected watch failure",
                    )));
                }
                6 => {
                    assert_eq!(
                        log.lock().unwrap().views.len(),
                        4,
                        "watch failure must not force a re-view"
                    );
                    assert_cells(backend.lock().unwrap().buffer(), 1);
                    return Next::End;
                }
                _ => unreachable!(),
            }
            Next::Timeout
        }),
    };
    run_project_with(
        &mut terminal,
        &mut prepared,
        &mut events,
        &ProjectLoopCx::default(),
        None,
    )
    .unwrap();
    drop(events);
    assert_eq!(stage, 7);
    assert_eq!(
        prepared.detection.projects.len(),
        1,
        "a watch failure must retain candidates"
    );
    assert_eq!(
        prepared.detection.projects[0].session_watch_paths,
        Some(vec![dir.join("second/index")])
    );
    let nav = prepared.screen.as_ref().unwrap();
    assert_eq!(nav.stack.len(), 1);
    assert_eq!(nav.frame().unwrap().key.screen, "items");
    assert_eq!(nav.frame().unwrap().key.selection, None);
    assert_eq!(
        prepared.notice.as_deref(),
        Some("could not detect or watch projects")
    );
}

/// The project root is removed while the standalone app runs: detection answers "no projects",
/// not a failure, so the loop shows no "could not detect or watch projects" notice.
#[cfg(feature = "test-hooks")]
#[test]
fn deleted_project_root_is_not_a_detection_failure() {
    let (dir, mut prepared, replies, _log) = setup();
    let detection = prepared.detection.clone();
    let backend = Arc::new(Mutex::new(TestBackend::new(54, 14)));
    let mut terminal = Terminal::new(Probe {
        backend,
        draws: Arc::new(Mutex::new(0)),
    })
    .unwrap();
    let mut stage = 0;
    let mut events = IdleEvents {
        replies,
        idle: Box::new(|| {
            let step = stage;
            stage += 1;
            match step {
                0 => {
                    std::fs::remove_dir_all(&dir).unwrap();
                    detection.inject_event_with_pending_watch_update(Ok(notify::Event::new(
                        notify::EventKind::Remove(notify::event::RemoveKind::Any),
                    )
                    .add_path(dir.join("project.marker"))));
                    Next::Timeout
                }
                1 => Next::End,
                _ => unreachable!(),
            }
        }),
    };
    run_project_with(
        &mut terminal,
        &mut prepared,
        &mut events,
        &ProjectLoopCx::default(),
        None,
    )
    .unwrap();
    drop(events);
    assert_eq!(stage, 2);
    assert!(prepared.detection.projects.is_empty());
    assert_ne!(
        prepared.notice.as_deref(),
        Some("could not detect or watch projects")
    );
    std::fs::create_dir_all(&dir).unwrap();
}

#[cfg(unix)]
#[test]
fn project_entry_refuses_before_terminal_initialisation() {
    use konoma_plugin::app::{project::run_projects, AppError};
    use std::os::unix::process::CommandExt;
    const CHILD: &str = "KONOMA_PROJECT_TERMINAL_ORDER_CHILD";
    if let Some(case) = std::env::var_os(CHILD) {
        let dir = konoma_test_support::unique_tmp("project_entry_refusal");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("file");
        std::fs::write(&file, "").unwrap();
        let path = match case.to_str().unwrap() {
            "missing" => dir.join("missing"),
            "file" => file,
            "empty" => dir.to_path_buf(),
            _ => unreachable!(),
        };
        let error = run_projects(vec![], &path, Arc::new(Services))
            .expect_err("refuse before terminal access");
        assert!(
            match case.to_str().unwrap() {
                "missing" => matches!(error, AppError::NoDirectory(_)),
                "file" => matches!(error, AppError::NotADirectory(_)),
                "empty" => matches!(error, AppError::NoProject(_)),
                _ => unreachable!(),
            },
            "project refusal must precede terminal entry: {error}"
        );
        assert_eq!(error.exit_code(), 2);
        drop(dir);
        std::process::exit(error.exit_code().into());
    }
    let config = konoma_test_support::unique_tmp("project_entry_config");
    std::fs::create_dir_all(&config).unwrap();
    for case in ["missing", "file", "empty"] {
        let mut child = std::process::Command::new(std::env::current_exe().unwrap());
        child
            .args([
                "--exact",
                "project::wake::project_entry_refuses_before_terminal_initialisation",
                "--nocapture",
            ])
            .env(CHILD, case)
            .env("HOME", config.as_path())
            .env("XDG_CONFIG_HOME", config.as_path())
            .stdin(std::process::Stdio::null());
        // SAFETY: setsid does not allocate or take locks in the forked child.
        unsafe {
            child.pre_exec(|| {
                if libc::setsid() == -1 {
                    Err(std::io::Error::last_os_error())
                } else {
                    Ok(())
                }
            });
        }
        let output = child.output().unwrap();
        assert_eq!(
            output.status.code(),
            Some(2),
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            !output.stdout.contains(&b'\x1b') && !output.stderr.contains(&b'\x1b'),
            "refusal must not write terminal escapes"
        );
    }
}
