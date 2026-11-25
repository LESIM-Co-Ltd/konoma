use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use konoma_host_core::shared::SharedConfig;
use konoma_plugin::app::project::*;
use konoma_plugin::app::{AppEvent, EventSource, LaunchCx, Next};
use konoma_plugin::host::project::MatchedProject;
use konoma_plugin::prelude::*;
use konoma_plugin::registry::Registry;
use konoma_plugin::view::{ListRow, ListView};
use konoma_plugin::TermCaps;
use konoma_test_support::TmpDir;
use konoma_vcs_core::git::{ChangeEntry, DiffLine};
use ratatui::{backend::TestBackend, buffer::Buffer, layout::Rect, Terminal};

const WAIT: Duration = Duration::from_secs(10);

#[path = "root_switch_retirement.rs"]
mod retirement;

struct Gate {
    entered: mpsc::Sender<()>,
    release: mpsc::Receiver<()>,
}
impl Gate {
    fn wait(self) {
        self.entered.send(()).unwrap();
        self.release
            .recv_timeout(WAIT)
            .expect("release worker latch");
    }
}
fn latch() -> (Gate, mpsc::Receiver<()>, mpsc::Sender<()>) {
    let (entered, rx) = mpsc::channel();
    let (tx, release) = mpsc::channel();
    (Gate { entered, release }, rx, tx)
}

#[derive(Default)]
struct Log {
    opens: Vec<PathBuf>,
    drops: Vec<PathBuf>,
    target: PathBuf,
    open_gate: Option<Gate>,
    fail: Option<&'static str>,
    act_gate: Option<Gate>,
    view_gate: Option<Gate>,
    dropped: Option<mpsc::Sender<PathBuf>>,
}
impl Log {
    fn refuses(&self, root: &Path, stage: &str) -> bool {
        root == self.target && self.fail == Some(stage)
    }
}
struct Project(Arc<Mutex<Log>>);
struct Owner(PathBuf, Arc<Mutex<Log>>);
impl Plugin for Project {
    fn manifest(&self) -> Manifest {
        Manifest::new("root-project", "1")
            .project(
                ProjectDecl::new()
                    .marker(DetectionMarker::new("project.marker", MarkerKind::File))
                    .screen(
                        ProjectScreen::new("items", "Items")
                            .row_kind("file")
                            .marks(true),
                    ),
            )
            .action(
                Act::new("switch")
                    .key("Enter")
                    .project_context(ActContext::screen("items", ["file"])),
            )
    }
    fn open(&self, input: Input, _: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        let Input::Project { root, .. } = input else {
            unreachable!()
        };
        let gate = {
            let mut log = self.0.lock().unwrap();
            log.opens.push(root.clone());
            log.open_gate.take()
        };
        if let Some(gate) = gate {
            gate.wait();
        }
        if self.0.lock().unwrap().refuses(&root, "open") {
            return Err(Failure::external("open refused"));
        }
        Ok(Box::new(Owner(root, self.0.clone())))
    }
}
impl Drop for Owner {
    fn drop(&mut self) {
        let mut log = self.1.lock().unwrap();
        log.drops.push(self.0.clone());
        if let Some(tx) = &log.dropped {
            let _ = tx.send(self.0.clone());
        }
    }
}
impl Session for Owner {
    fn view(&mut self, _: &ViewReq, _: &ViewCx) -> Result<View, Failure> {
        let gate = self.1.lock().unwrap().view_gate.take();
        if let Some(gate) = gate {
            gate.wait();
        }
        if self.1.lock().unwrap().refuses(&self.0, "view") {
            return Err(Failure::external("view refused"));
        }
        Ok(View::List(ListView {
            rows: ["first", "second", "third"]
                .into_iter()
                .map(|name| {
                    ListRow::new("file", vec![Line::raw(name)])
                        .value(name)
                        .target(self.0.join(name))
                })
                .collect(),
            ..Default::default()
        }))
    }
    fn act(&mut self, _: &str, _: &ActCx) -> Result<Outcome, Failure> {
        let gate = self.1.lock().unwrap().act_gate.take();
        if let Some(gate) = gate {
            gate.wait();
            return Ok(Outcome::Flash("old mailbox".into()));
        }
        Ok(Outcome::SwitchRoot {
            root: self.1.lock().unwrap().target.clone(),
            message: Some("switched fixture".into()),
        })
    }
}

struct Services(mpsc::Sender<()>, Arc<Mutex<Log>>);
impl ProjectServices for Services {
    fn root_switch_enabled(&self) -> bool {
        true
    }

    fn inspect(
        &self,
        p: &MatchedProject,
        _: &ProjectSettings,
    ) -> Result<ProjectServiceInfo, Failure> {
        if self.1.lock().unwrap().refuses(&p.root, "inspect") {
            return Err(Failure::external("inspect refused"));
        }
        Ok(ProjectServiceInfo {
            workdir: p.root.clone(),
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
    fn spawn_launch_target(
        self: Arc<Self>,
        path: PathBuf,
        line: Option<usize>,
        registry: Arc<Registry>,
    ) -> mpsc::Receiver<Option<ProjectLaunchTarget>> {
        let worker = ProjectLaunchTarget::spawn(path, line, self.clone(), registry);
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(worker.recv().unwrap());
            let _ = self.0.send(());
        });
        rx
    }
}
struct Events {
    keys: VecDeque<Key>,
    ready: mpsc::Receiver<()>,
    stop: Option<mpsc::Receiver<()>>,
}
impl EventSource for Events {
    fn next(&mut self, wait: Option<Duration>) -> Next {
        if let Some(stop) = self.stop.take() {
            stop.recv_timeout(WAIT).expect("candidate entered open");
            return Next::End;
        }
        if wait == Some(Duration::from_millis(16)) {
            self.ready
                .recv_timeout(WAIT)
                .expect("worker published before polling");
            Next::Timeout
        } else {
            self.keys
                .pop_front()
                .map(|k| Next::Event(AppEvent::Key(k)))
                .unwrap_or(Next::End)
        }
    }
}
fn key(s: &str) -> Key {
    Key::new(if s == "Enter" {
        KeyCode::Enter
    } else {
        KeyCode::Char(s.chars().next().unwrap())
    })
}
struct Fixture {
    _dir: TmpDir,
    a: PathBuf,
    b: PathBuf,
    prepared: PreparedProjects,
    log: Arc<Mutex<Log>>,
    dropped: mpsc::Receiver<PathBuf>,
    old_delivery: mpsc::Receiver<bool>,
    ready: mpsc::Sender<()>,
    events: Events,
    terminal: Terminal<TestBackend>,
    cx: ProjectLoopCx,
    launches: Arc<Mutex<Vec<PathBuf>>>,
}
impl Fixture {
    fn new() -> Self {
        let dir = konoma_test_support::unique_tmp("root_switch");
        for name in ["a", "b"] {
            std::fs::create_dir_all(dir.join(name)).unwrap();
            for file in ["project.marker", "first", "second", "third"] {
                std::fs::write(dir.join(name).join(file), "").unwrap();
            }
        }
        let dir = dir.into_canonical().unwrap();
        let a = dir.join("a");
        let b = dir.join("b");
        let log = Arc::new(Mutex::new(Log {
            target: b.clone(),
            ..Default::default()
        }));
        let (ready, rx) = mpsc::channel();
        let (drop_tx, dropped) = mpsc::channel();
        let (delivery, old_delivery) = mpsc::channel();
        log.lock().unwrap().dropped = Some(drop_tx);
        let mut prepared = prepare_projects(
            vec![Arc::new(Project(log.clone()))],
            &a,
            TermCaps::new(),
            (
                SharedConfig::parse("[ui]\nlang='en'\n[editor]\ncommand='fixture-editor'").unwrap(),
                None,
            ),
            Arc::new(Services(ready.clone(), log.clone())),
        )
        .unwrap_or_else(|e| panic!("{}", e.error));
        prepared.fence_root_switches(ready.clone());
        for runtime in prepared.runtimes.values_mut() {
            let (tx, rx) = mpsc::sync_channel(1);
            let owner = std::mem::replace(&mut runtime.rx, rx);
            let ready = ready.clone();
            let delivery = delivery.clone();
            std::thread::spawn(move || {
                while let Ok(reply) = owner.recv() {
                    let sent = tx.send(reply).is_ok();
                    let _ = delivery.send(sent);
                    if !sent {
                        break;
                    }
                    let _ = ready.send(());
                }
            });
        }
        let launches = Arc::new(Mutex::new(Vec::new()));
        let record = launches.clone();
        let launch = LaunchCx::new(
            prepared.config.editor.clone(),
            &prepared.config.external,
            vec![],
        )
        .with_hand_off(Arc::new(move |a, _, redraw| {
            record
                .lock()
                .unwrap()
                .push(PathBuf::from(a.args.last().unwrap()));
            redraw()?;
            Ok(Some(0))
        }));
        Self {
            _dir: dir,
            a,
            b,
            prepared,
            log,
            dropped,
            old_delivery,
            ready,
            events: Events {
                keys: VecDeque::new(),
                ready: rx,
                stop: None,
            },
            terminal: Terminal::new(TestBackend::new(60, 12)).unwrap(),
            cx: ProjectLoopCx {
                launch: Some(launch),
                ..Default::default()
            },
            launches,
        }
    }
    fn run(&mut self, keys: &[&str]) {
        self.events.keys = keys.iter().map(|s| key(s)).collect();
        run_project_with(
            &mut self.terminal,
            &mut self.prepared,
            &mut self.events,
            &self.cx,
            None,
        )
        .unwrap();
    }
    fn origin(&self) -> konoma_plugin::host::project::ProjectOrigin<StandaloneSurface> {
        let nav = self.prepared.screen.as_ref().unwrap();
        konoma_plugin::host::project::ProjectOrigin {
            context: 0,
            project: nav.project.clone(),
            root: self.prepared.open_dir.clone(),
            screen: Some(nav.frame().unwrap().key.clone()),
            surface: StandaloneSurface,
        }
    }
    fn inject(
        &mut self,
        origin: konoma_plugin::host::project::ProjectOrigin<StandaloneSurface>,
        outcome: Outcome,
    ) {
        use konoma_plugin::host::project::{ProjectEffect, ProjectReply};
        let nav = self.prepared.screen.as_ref().unwrap();
        let runtime = self.prepared.runtimes.get_mut(&nav.project).unwrap();
        let mut effect = ProjectEffect::new(origin, "injected effect");
        effect.outcome = Some(outcome);
        runtime.pending = Some(ProjectReply {
            watch_paths: None,
            views: vec![],
            launch_targets: Default::default(),
            enabled: Default::default(),
            chip: None,
            chip_error: None,
            effect: Some(effect),
            error: None,
            act_failed: false,
            contexts: Default::default(),
        });
    }
    fn hold_act(&mut self) -> mpsc::Sender<()> {
        let (gate, entered, release) = latch();
        self.log.lock().unwrap().act_gate = Some(gate);
        self.events.keys = [key("Enter")].into();
        struct Hold<'a> {
            events: &'a mut Events,
            sent: bool,
            entered: mpsc::Receiver<()>,
        }
        impl EventSource for Hold<'_> {
            fn next(&mut self, wait: Option<Duration>) -> Next {
                if !self.sent {
                    let next = self.events.next(wait);
                    self.sent = matches!(next, Next::Event(AppEvent::Key(_)));
                    next
                } else {
                    self.entered
                        .recv_timeout(WAIT)
                        .expect("old act held on owner");
                    Next::End
                }
            }
        }
        run_project_with(
            &mut self.terminal,
            &mut self.prepared,
            &mut Hold {
                events: &mut self.events,
                sent: false,
                entered,
            },
            &self.cx,
            None,
        )
        .unwrap();
        release
    }
    fn wait_for_old_drop(&self) {
        konoma_base::discard::flush_discards();
        for _ in 0..3 {
            if self
                .dropped
                .recv_timeout(WAIT)
                .expect("old channel lifetime releases held owner")
                == self.a
            {
                return;
            }
        }
        panic!("old A owner must be released");
    }
    fn hold_view(&mut self) -> mpsc::Sender<()> {
        let (gate, entered, release) = latch();
        self.log.lock().unwrap().view_gate = Some(gate);
        let held = Arc::new(AtomicBool::new(false));
        let published = held.clone();
        let wake = self.ready.clone();
        std::thread::spawn(move || {
            entered
                .recv_timeout(WAIT)
                .expect("old view entered its latch");
            published.store(true, Ordering::Release);
            let _ = wake.send(());
        });
        let nav = self.prepared.screen.as_ref().unwrap();
        self.prepared
            .runtimes
            .get_mut(&nav.project)
            .unwrap()
            .views
            .clear();
        struct HoldView<'a> {
            events: &'a mut Events,
            held: Arc<AtomicBool>,
        }
        impl EventSource for HoldView<'_> {
            fn next(&mut self, wait: Option<Duration>) -> Next {
                assert_eq!(
                    wait,
                    Some(Duration::from_millis(16)),
                    "view preparation stays pending until the latch"
                );
                self.events
                    .ready
                    .recv_timeout(WAIT)
                    .expect("owner reply or held-view fence");
                if self.held.load(Ordering::Acquire) {
                    Next::End
                } else {
                    Next::Timeout
                }
            }
        }
        run_project_with(
            &mut self.terminal,
            &mut self.prepared,
            &mut HoldView {
                events: &mut self.events,
                held,
            },
            &self.cx,
            None,
        )
        .unwrap();
        release
    }
}
fn same_cells(actual: &Buffer, expected: &Buffer) {
    let rect = Rect::new(0, 1, 60, 10);
    assert_eq!(actual.area, Rect::new(0, 0, 60, 12));
    assert_eq!(actual.area, expected.area);
    assert!(actual
        .area
        .contains((rect.right() - 1, rect.bottom() - 1).into()));
    let mut compared = 0;
    let mut nonempty = 0;
    for y in rect.y..rect.bottom() {
        for x in rect.x..rect.right() {
            let a = &actual[(x, y)];
            let b = &expected[(x, y)];
            assert_eq!(
                (a.symbol(), a.fg, a.bg, a.modifier),
                (b.symbol(), b.fg, b.bg, b.modifier),
                "cell ({x}, {y})"
            );
            compared += 1;
            nonempty += usize::from(!a.symbol().trim().is_empty());
        }
    }
    assert_eq!(compared, usize::from(rect.width) * usize::from(rect.height));
    assert!(nonempty > 0);
}

#[test]
fn switch_root_opens_first_screen_then_replaces_current_root() {
    let mut f = Fixture::new();
    f.run(&["j"]);
    let before = f.terminal.backend().buffer().clone();
    let (gate, entered, release) = latch();
    f.log.lock().unwrap().open_gate = Some(gate);
    // End input immediately after the action reply has started candidate preparation.
    f.events.keys = [key("Enter")].into();
    struct StopAtOpen<'a> {
        events: &'a mut Events,
        entered: Option<mpsc::Receiver<()>>,
        keys_read: bool,
    }
    impl EventSource for StopAtOpen<'_> {
        fn next(&mut self, wait: Option<Duration>) -> Next {
            if self.keys_read && wait == Some(Duration::from_millis(16)) {
                // First wait publishes the action reply; the next wait is candidate preparation.
                if let Some(entered) = self.entered.take() {
                    self.events.ready.recv_timeout(WAIT).unwrap();
                    self.events.stop = Some(entered);
                    return Next::Timeout;
                }
            }
            let next = self.events.next(wait);
            if matches!(next, Next::Event(_)) {
                self.keys_read = true;
            }
            next
        }
    }
    run_project_with(
        &mut f.terminal,
        &mut f.prepared,
        &mut StopAtOpen {
            events: &mut f.events,
            entered: Some(entered),
            keys_read: false,
        },
        &f.cx,
        None,
    )
    .unwrap();
    assert_eq!(
        f.prepared.open_dir, f.a,
        "old root lives until candidate first screen succeeds"
    );
    assert!(
        f.prepared.screen.is_some(),
        "old frame survives candidate open"
    );
    assert_eq!(
        f.prepared.screen.as_ref().unwrap().frame().unwrap().cursor,
        1
    );
    same_cells(f.terminal.backend().buffer(), &before);
    assert!(
        f.log.lock().unwrap().drops.is_empty(),
        "old owner survives candidate open"
    );
    release.send(()).unwrap();
    f.run(&[]);
    assert_eq!(
        f.prepared.open_dir, f.b,
        "successful candidate replaces open_dir"
    );
    let nav = f.prepared.screen.as_ref().unwrap();
    assert_eq!(nav.project.root, f.b);
    let runtime = &f.prepared.runtimes[&nav.project];
    assert!(
        runtime.views.contains_key(&nav.frame().unwrap().key),
        "replacement already owns its first screen"
    );
    let req = &runtime.contexts[&nav.frame().unwrap().key];
    assert_eq!(req.tree_root, Some(f.b.clone()));
    assert_eq!(req.open_dir, Some(f.b.clone()));
    f.run(&["e"]);
    assert_eq!(*f.launches.lock().unwrap(), vec![f.b.join("first")]);
}

#[test]
fn failed_root_switch_keeps_frame_selection_and_launch_target() {
    for failure in ["missing", "inspect", "open", "view"] {
        let mut f = Fixture::new();
        f.run(&["j", "V"]);
        let before = f.terminal.backend().buffer().clone();
        let frame = f.prepared.screen.as_ref().unwrap().frame().unwrap().clone();
        {
            let mut log = f.log.lock().unwrap();
            log.fail = Some(failure);
            if failure == "missing" {
                log.target = f.b.join("missing");
            }
        }
        f.run(&["Enter"]);
        assert_eq!(
            f.prepared.open_dir, f.a,
            "failure keeps original open_dir: {failure}"
        );
        let nav = f
            .prepared
            .screen
            .as_ref()
            .expect("failure preserves active frame");
        assert_eq!(nav.project.root, f.a, "failure keeps original owner");
        assert!(
            f.prepared.runtimes[&nav.project].error.is_none(),
            "candidate failure does not alter the old owner's operation state"
        );
        let after = nav.frame().unwrap();
        assert_eq!(
            (
                after.cursor,
                after.scroll,
                after.hscroll,
                &after.query,
                &after.marked,
                &after.key
            ),
            (
                frame.cursor,
                frame.scroll,
                frame.hscroll,
                &frame.query,
                &frame.marked,
                &frame.key
            ),
            "failure keeps selection and operation state"
        );
        same_cells(f.terminal.backend().buffer(), &before);
        assert!(f
            .prepared
            .notice
            .as_deref()
            .unwrap()
            .contains(if failure == "missing" {
                "not found: "
            } else {
                failure
            }));
        f.run(&["e"]);
        assert_eq!(
            *f.launches.lock().unwrap(),
            vec![f.a.join("second")],
            "failure keeps launch target"
        );
    }
}

#[test]
fn old_input_reply_and_effect_do_not_cross_root_replacement() {
    let mut f = Fixture::new();
    f.run(&["j"]);
    let old_origin = f.origin();
    let release_old = f.hold_act();
    let (gate, entered, release) = latch();
    f.log.lock().unwrap().open_gate = Some(gate);
    f.prepared
        .screen
        .as_mut()
        .unwrap()
        .frame_mut()
        .unwrap()
        .pending
        .push_back(konoma_plugin::host::project::ProjectInput::Key(
            crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Char('j'),
                crossterm::event::KeyModifiers::NONE,
            ),
        ));
    f.inject(
        old_origin.clone(),
        Outcome::SwitchRoot {
            root: f.b.clone(),
            message: None,
        },
    );
    f.events.stop = Some(entered);
    f.run(&[]);
    assert_eq!(
        f.prepared
            .screen
            .as_ref()
            .unwrap()
            .frame()
            .unwrap()
            .pending
            .len(),
        1,
        "candidate preparation retains old queued input for a possible failure"
    );
    release.send(()).unwrap();
    f.run(&[]);
    assert_eq!(f.prepared.open_dir, f.b);
    assert_eq!(
        f.prepared.screen.as_ref().unwrap().frame().unwrap().cursor,
        0,
        "old pending input must be discarded on replacement"
    );
    let before = f.terminal.backend().buffer().clone();
    f.old_delivery.try_iter().for_each(drop);
    release_old.send(()).unwrap();
    f.wait_for_old_drop();
    assert!(
        !f.old_delivery.recv_timeout(WAIT).unwrap(),
        "old receiver is disconnected after replacement"
    );
    f.run(&[]);
    same_cells(f.terminal.backend().buffer(), &before);
    assert_ne!(
        f.prepared.notice.as_deref(),
        Some("old mailbox"),
        "old receiver must never be polled"
    );
    f.inject(old_origin, Outcome::Flash("stale extracted effect".into()));
    f.run(&[]);
    assert_ne!(
        f.prepared.notice.as_deref(),
        Some("stale extracted effect"),
        "extracted effects recheck origin against the current root"
    );
}

#[test]
fn switch_a_b_a_does_not_adopt_the_first_a_mailbox() {
    let mut f = Fixture::new();
    f.run(&["j"]);
    let old_origin = f.origin();
    let release_old = f.hold_view();
    f.log.lock().unwrap().target = f.a.clone();
    f.inject(
        old_origin,
        Outcome::SwitchRoot {
            root: f.b.clone(),
            message: None,
        },
    );
    // The B action and the second replacement run without ending the standalone loop.
    f.run(&["Enter"]);
    assert_eq!(f.prepared.open_dir, f.a);
    assert_eq!(
        f.log.lock().unwrap().opens,
        vec![f.a.clone(), f.b.clone(), f.a.clone()],
        "returning to A opens a new owner instead of caching by root string"
    );
    assert_eq!(
        f.prepared.screen.as_ref().unwrap().frame().unwrap().cursor,
        0,
        "returning to A starts a fresh frame"
    );
    let before = f.terminal.backend().buffer().clone();
    konoma_base::discard::flush_discards();
    f.old_delivery.try_iter().for_each(drop);
    release_old.send(()).unwrap();
    f.wait_for_old_drop();
    assert!(
        !f.old_delivery.recv_timeout(WAIT).unwrap(),
        "the first A mailbox is disconnected even after returning to A"
    );
    f.run(&[]);
    same_cells(f.terminal.backend().buffer(), &before);
    assert_eq!(
        f.prepared.screen.as_ref().unwrap().frame().unwrap().cursor,
        0
    );
    let nav = f.prepared.screen.as_ref().unwrap();
    assert!(
        f.prepared.runtimes[&nav.project]
            .views
            .contains_key(&nav.frame().unwrap().key),
        "the second A keeps its own prepared view after the first A replies"
    );
}
