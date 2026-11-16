// Real keys start the held jobs. Only the incoming transport schedules the competing effect.
use konoma_plugin::app::project::{PreparedProjects, ProjectIdleSnapshot, StandaloneSurface};
use konoma_plugin::host::project::{ProjectEffect, ProjectOrigin, ProjectReply, ProjectRuntime};
use konoma_plugin::prelude::{
    ActCx, Failure, Input, Manifest, OpenCx, Outcome, Session, View, ViewCx, ViewReq,
};
use std::sync::mpsc;

const ROOT_WAIT: std::time::Duration = std::time::Duration::from_secs(30);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OldRootJob {
    View,
    Input,
    Effect,
}
#[derive(Clone, Copy)]
struct RootJobCase {
    job: OldRootJob,
    return_a: bool,
    fail_open: bool,
}
struct RootGate {
    entered: mpsc::Sender<()>,
    release: mpsc::Receiver<()>,
}
impl RootGate {
    fn wait(self) {
        self.entered.send(()).unwrap();
        self.release
            .recv_timeout(ROOT_WAIT)
            .expect("release root worker latch");
    }
}
struct RootLatch {
    entered: mpsc::Receiver<()>,
    release: Option<mpsc::Sender<()>>,
}
impl RootLatch {
    fn new() -> (RootGate, Self) {
        let (entered, seen) = mpsc::channel();
        let (release, released) = mpsc::channel();
        (
            RootGate {
                entered,
                release: released,
            },
            Self {
                entered: seen,
                release: Some(release),
            },
        )
    }
    #[track_caller]
    fn entered(&self) {
        self.entered
            .recv_timeout(ROOT_WAIT)
            .expect("worker entered the requested root fence");
    }
    fn release(&mut self) {
        self.release.take().unwrap().send(()).unwrap();
    }
}
impl Drop for RootLatch {
    fn drop(&mut self) {
        if let Some(release) = self.release.take() {
            let _ = release.send(());
        }
    }
}

struct RootProbeState {
    a: std::path::PathBuf,
    b: std::path::PathBuf,
    job: OldRootJob,
    opens_a: usize,
    opens_b: usize,
    old: Option<RootGate>,
    open: Option<RootGate>,
    first_view: Option<RootGate>,
    input: Option<String>,
    input_failed: bool,
    dropped: mpsc::Sender<()>,
}
struct RootSide {
    state: Arc<Mutex<RootProbeState>>,
    old: Option<RootLatch>,
    open: Option<RootLatch>,
    view: Option<RootLatch>,
    dropped: mpsc::Receiver<()>,
}
impl RootSide {
    fn new(a: &Path, job: OldRootJob) -> Self {
        let (dropped, seen) = mpsc::channel();
        Self {
            state: Arc::new(Mutex::new(RootProbeState {
                a: a.into(),
                b: a.parent().unwrap().join("linked"),
                job,
                opens_a: 0,
                opens_b: 0,
                old: None,
                open: None,
                first_view: None,
                input: None,
                input_failed: false,
                dropped,
            })),
            old: None,
            open: None,
            view: None,
            dropped: seen,
        }
    }
    fn arm(&mut self, candidate: bool) {
        let (gate, latch) = RootLatch::new();
        self.state.lock().unwrap().old = Some(gate);
        self.old = Some(latch);
        if candidate {
            let (gate, latch) = RootLatch::new();
            self.state.lock().unwrap().open = Some(gate);
            self.open = Some(latch);
            let (gate, latch) = RootLatch::new();
            self.state.lock().unwrap().first_view = Some(gate);
            self.view = Some(latch);
        }
    }
    fn wrap(&self, inner: Arc<dyn Plugin>) -> Arc<dyn Plugin> {
        Arc::new(RootProbe {
            inner,
            state: self.state.clone(),
        })
    }
}
struct RootProbe {
    inner: Arc<dyn Plugin>,
    state: Arc<Mutex<RootProbeState>>,
}
impl Plugin for RootProbe {
    fn manifest(&self) -> Manifest {
        self.inner.manifest()
    }
    fn open(&self, input: Input, cx: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        let Input::Project { root, .. } = &input else {
            unreachable!()
        };
        let (first_a, gate) = {
            let mut state = self.state.lock().unwrap();
            if root == &state.a {
                state.opens_a += 1;
                (state.opens_a == 1, None)
            } else if root == &state.b {
                state.opens_b += 1;
                (false, state.open.take())
            } else {
                (false, None)
            }
        };
        if let Some(gate) = gate {
            gate.wait();
        }
        if !root.is_dir() {
            return Err(Failure::external(konoma_plugin::Tr::new(format!(
                "not found: {}",
                root.display()
            ))));
        }
        let root = root.clone();
        Ok(Box::new(RootSession {
            inner: self.inner.open(input, cx)?,
            state: self.state.clone(),
            root,
            first_a,
        }))
    }
}
struct RootSession {
    inner: Box<dyn Session>,
    state: Arc<Mutex<RootProbeState>>,
    root: std::path::PathBuf,
    first_a: bool,
}
impl Drop for RootSession {
    fn drop(&mut self) {
        if self.first_a {
            let _ = self.state.lock().unwrap().dropped.send(());
        }
    }
}
impl Session for RootSession {
    fn view(&mut self, req: &ViewReq, cx: &ViewCx) -> Result<View, Failure> {
        let view = self.inner.view(req, cx);
        let gate = {
            let mut state = self.state.lock().unwrap();
            if self.first_a && state.job == OldRootJob::View {
                state.old.take()
            } else if self.root == state.b
                && req.project.as_ref().is_some_and(|p| p.screen == "changes")
            {
                state.first_view.take()
            } else {
                None
            }
        };
        if let Some(gate) = gate {
            gate.wait();
        }
        view
    }
    fn act(&mut self, action: &str, cx: &ActCx) -> Result<Outcome, Failure> {
        let result = self.inner.act(action, cx);
        let gate = {
            let mut state = self.state.lock().unwrap();
            if self.first_a
                && ((state.job == OldRootJob::Effect && action == "worktree_switch")
                    || (state.job == OldRootJob::Input && action == "create_branch"))
            {
                if state.job == OldRootJob::Input {
                    state.input = cx.project.as_ref().and_then(|p| p.text.clone());
                    state.input_failed = result.is_err();
                }
                state.old.take()
            } else {
                None
            }
        };
        if let Some(gate) = gate {
            gate.wait();
        }
        result
    }
    fn enabled(&self, action: &str) -> bool {
        self.inner.enabled(action)
    }
    fn chip(&mut self, cx: &ViewCx) -> Result<Option<konoma_plugin::Chip>, Failure> {
        self.inner.chip(cx)
    }
    fn watch_paths(&mut self, cx: &ViewCx) -> Result<Option<Vec<std::path::PathBuf>>, Failure> {
        self.inner.watch_paths(cx)
    }
    fn tab_closed(&mut self, tab: u64) {
        self.inner.tab_closed(tab);
    }
}

struct RootMail<H> {
    inbox: mpsc::SyncSender<ProjectReply<ProjectEffect<H>>>,
    delivered: mpsc::Receiver<bool>,
    sent: Arc<std::sync::atomic::AtomicUsize>,
    acknowledged: usize,
    origin: ProjectOrigin<H>,
}
impl<H: Send + Clone + 'static> RootMail<H> {
    fn attach<I>(
        runtime: &mut ProjectRuntime<ProjectEffect<H>, I>,
        origin: ProjectOrigin<H>,
        ready: mpsc::Sender<()>,
    ) -> Self {
        let (inbox, receiver) = mpsc::sync_channel(1);
        let owner = std::mem::replace(&mut runtime.rx, receiver);
        let (delivered, seen) = mpsc::channel();
        let forward = inbox.clone();
        let sent = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count = sent.clone();
        std::thread::spawn(move || {
            while let Ok(reply) = owner.recv() {
                count.fetch_add(1, std::sync::atomic::Ordering::Release);
                let accepted = forward.send(reply).is_ok();
                let _ = delivered.send(accepted);
                let _ = ready.send(());
                if !accepted {
                    break;
                }
            }
        });
        Self {
            inbox,
            delivered: seen,
            sent,
            acknowledged: 0,
            origin,
        }
    }
    fn acknowledge(&mut self) {
        let count = self.sent.load(std::sync::atomic::Ordering::Acquire);
        while self.acknowledged < count {
            assert!(self.delivered.recv_timeout(ROOT_WAIT).unwrap());
            self.acknowledged += 1;
        }
    }
    fn switch(&self, root: &Path) {
        self.publish(Some(root));
    }
    fn publish(&self, root: Option<&Path>) {
        let mut effect = ProjectEffect::new(self.origin.clone(), "switch");
        effect.outcome = root.map(|root| Outcome::SwitchRoot {
            root: root.into(),
            message: None,
        });
        let mut reply = ProjectReply {
            watch_paths: None,
            views: vec![],
            launch_targets: Default::default(),
            enabled: Default::default(),
            chip: None,
            chip_error: None,
            effect: root.map(|_| effect),
            error: None,
            act_failed: false,
            contexts: Default::default(),
        };
        // The mailbox holds one reply and only this thread's poll empties it, so a blocking
        // send would wait forever on a reply nobody adopts.
        let deadline = std::time::Instant::now() + ROOT_WAIT;
        loop {
            match self.inbox.try_send(reply) {
                Ok(()) => break,
                Err(mpsc::TrySendError::Full(back)) => {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "the mailbox stayed full: an owner reply was forwarded but never adopted by a poll"
                    );
                    reply = back;
                    std::thread::sleep(std::time::Duration::from_millis(2));
                }
                Err(mpsc::TrySendError::Disconnected(_)) => {
                    panic!("current root receives the scheduled switch effect")
                }
            }
        }
    }
    fn late(&mut self, accepted: bool, side: &str) {
        assert_eq!(self.delivered.recv_timeout(ROOT_WAIT).expect("held old owner published its reply"), accepted, "{side}: first A input/view/effect mailbox must disconnect after replacement, including A -> B -> A");
        self.acknowledged += 1;
    }
}

#[derive(Clone, Copy, Debug)]
enum RootPhase {
    Navigate,
    OldEntered,
    Queued,
    Open,
    View,
    Commit,
    Reopen,
    Release,
    Late,
    FailureSettle,
    FailureKeys,
    Done,
}
struct RootJobs {
    case: RootJobCase,
    host: RootSide,
    app: RootSide,
    host_mail: Option<RootMail<crate::app::project_screens::ProjectSurface>>,
    app_mail: Option<RootMail<StandaloneSurface>>,
    ready: mpsc::Receiver<()>,
    ready_tx: mpsc::Sender<()>,
    phase: RootPhase,
    keys: std::collections::VecDeque<KeyEvent>,
    before: Option<ProjectStateSnapshot>,
    late_before: Option<ProjectStateSnapshot>,
    a: std::path::PathBuf,
    app_a: std::path::PathBuf,
    finished: bool,
    fences: usize,
    /// The phase and start of the current not-idle stretch.
    stall: Option<(String, std::time::Instant)>,
    expecting_switch: bool,
    failure_cursor: usize,
}

/// Row 0 of the host frame carries the busy segment ("{spinner} {label}  ", DIM) only while a
/// project write act runs here (`busy_indicator=false`). Assert that, then copy the app frame's
/// cells over exactly that segment.
fn blank_host_busy_segment(host_frame: &mut Buffer, app_frame: &Buffer, host: &crate::app::App) {
    let width = host_frame.area.width;
    let row: Vec<String> = (0..width)
        .map(|x| host_frame[(x, 0)].symbol().to_string())
        .collect();
    let has_spinner = row.iter().any(|s| {
        konoma_widgets::loading::SPINNER_FRAMES
            .iter()
            .any(|g| g == s)
    });
    if !host.git_write_running() {
        assert!(!has_spinner, "no busy segment without a running write");
        return;
    }
    let jobs = host.busy_jobs();
    let first = jobs.first().expect("a running write is a busy job");
    let mut text = format!(
        "{} {}",
        host.spinner_glyph(),
        crate::i18n::tr(host.lang, *first)
    );
    if jobs.len() > 1 {
        text.push_str(&format!(" +{}", jobs.len() - 1));
    }
    text.push_str("  ");
    let glyphs: Vec<String> = text.chars().map(|c| c.to_string()).collect();
    let start = (0..width as usize)
        .find(|&x| row[x..].starts_with(&glyphs))
        .unwrap_or_else(|| panic!("busy segment {text:?} missing from host row 0: {row:?}"));
    for (i, _) in glyphs.iter().enumerate() {
        let x = (start + i) as u16;
        assert!(
            host_frame[(x, 0)]
                .modifier
                .contains(ratatui::style::Modifier::DIM),
            "busy segment cell {x} must be DIM"
        );
        host_frame[(x, 0)] = app_frame[(x, 0)].clone();
    }
}

impl RootJobs {
    fn new(a: &Path, app_a: &Path, case: RootJobCase) -> Self {
        let (ready_tx, ready) = mpsc::channel();
        let keys = if case.job == OldRootJob::Input {
            vec![ch('b'), ch('c'), ch('m'), ch('a'), ch('i'), ch('n')]
        } else {
            let mut keys = vec![ch('w'), ch('/')];
            keys.extend(typed("日本語"));
            keys.extend([key(KeyCode::Enter), key(KeyCode::Esc)]);
            keys
        };
        Self {
            case,
            host: RootSide::new(a, case.job),
            app: RootSide::new(app_a, case.job),
            host_mail: None,
            app_mail: None,
            ready,
            ready_tx,
            phase: RootPhase::Navigate,
            keys: keys
                .into_iter()
                .map(|s| match s {
                    Step::Key(key) => key,
                    _ => unreachable!(),
                })
                .collect(),
            before: None,
            late_before: None,
            a: a.into(),
            app_a: app_a.into(),
            finished: false,
            fences: 0,
            stall: None,
            expecting_switch: false,
            failure_cursor: 0,
        }
    }
    fn attach(&mut self, host: &mut Sim, app: &mut PreparedProjects) {
        let nav = host.app.tab.project_navigation.as_ref().unwrap();
        let mut screen = nav.frame().unwrap().key.clone();
        screen.screen = if self.case.job == OldRootJob::Input {
            "branches"
        } else {
            "worktrees"
        }
        .into();
        let origin = ProjectOrigin {
            context: screen.tab,
            project: nav.project.clone(),
            root: self.a.clone(),
            screen: Some(screen),
            surface: crate::app::project_screens::ProjectSurface::default(),
        };
        self.host_mail = Some(RootMail::attach(
            host.app.project_runtimes.get_mut(&origin.project).unwrap(),
            origin.clone(),
            self.ready_tx.clone(),
        ));
        let nav = app.screen.as_ref().unwrap();
        let project = nav.project.clone();
        let mut screen = nav.frame().unwrap().key.clone();
        screen.screen = origin.screen.unwrap().screen;
        let origin = ProjectOrigin {
            context: screen.tab,
            project: project.clone(),
            root: self.app_a.clone(),
            screen: Some(screen),
            surface: StandaloneSurface,
        };
        self.app_mail = Some(RootMail::attach(
            app.runtimes.get_mut(&project).unwrap(),
            origin,
            self.ready_tx.clone(),
        ));
        app.fence_root_switches(self.ready_tx.clone());
    }
    /// Wait for what the snapshot lacks. Owner replies announce themselves on `ready`; the
    /// watch answer is adopted only by the app's own poll, which runs between observations,
    /// so blocking here for it would stall the poll that settles it.
    fn wait(&mut self, idle: &ProjectIdleSnapshot) -> Next {
        let phase = format!("{:?}", self.phase);
        let started = match &self.stall {
            Some((seen, at)) if *seen == phase => *at,
            _ => std::time::Instant::now(),
        };
        self.stall = Some((phase.clone(), started));
        assert!(
            started.elapsed() < ROOT_WAIT,
            "phase {phase} never settled: owners={} contexts={} replay={} detection={} watch={} navigation={}",
            idle.owners,
            idle.contexts,
            idle.replay,
            idle.detection,
            idle.watch,
            idle.navigation
        );
        if idle.owners && idle.contexts && idle.replay {
            std::thread::sleep(std::time::Duration::from_millis(2));
            return Next::Timeout;
        }
        self.ready
            .recv_timeout(ROOT_WAIT)
            .expect("owner/root publication precedes the next poll");
        Next::Timeout
    }
    fn press(&self, host: &mut Sim, key: KeyEvent, settle: bool) -> Next {
        host.press(key.code, key.modifiers);
        if settle {
            drain(host);
        }
        Next::Event(AppEvent::Key(key_from_crossterm(&key).unwrap()))
    }
    fn compare(
        &mut self,
        state: &ProjectStateSnapshot,
        frame: &Buffer,
        host: &mut Sim,
        fixture: &serde_json::Value,
        graph_fixture: &serde_json::Value,
    ) {
        // Pending frames must never use project_frame(), whose idle drain would wait on the latch.
        let host_frame = host
            .term
            .draw(|f| crate::ui::render(f, &mut host.app))
            .unwrap()
            .buffer
            .clone();
        let mut host_state = host_state(host);
        let mut app_state = state.clone();
        normalize_write_state(&mut host_state, self.a.parent().unwrap());
        normalize_write_state(&mut app_state, self.app_a.parent().unwrap());
        compare_states(&host_state, &app_state)
            .unwrap_or_else(|e| panic!("continuous {:?}/{:?}: {e}", self.case.job, self.phase));
        let mut different = app_state.clone();
        different.root = Some("wrong-root".into());
        assert!(
            compare_states(&app_state, &different).is_err(),
            "shared-state comparison must include the new root path"
        );
        let mut host_frame = host_frame;
        let mut app_frame = frame.clone();
        normalize_write_frame(&mut host_frame, &self.a);
        normalize_write_frame(&mut app_frame, &self.app_a);
        // The busy indicator is host-only chrome (the standalone app has none); a running write
        // forces it on even with `busy_indicator=false`. Assert it precisely, then blank exactly
        // its cells so the rest of the frame is still compared cell by cell.
        blank_host_busy_segment(&mut host_frame, &app_frame, &host.app);
        let mut chrome = graph_fixture.clone();
        let worktrees = read_snapshot_for_test("vcs_app_worktrees");
        let row = if app_state
            .selected
            .as_ref()
            .is_some_and(|r| r.kind == "worktree")
        {
            "target"
        } else {
            "plain"
        };
        chrome["en"]["worktrees"] = worktrees["en"]["write"][row].clone();
        if self.case.job == OldRootJob::Input
            && matches!(self.phase, RootPhase::OldEntered | RootPhase::Queued)
        {
            chrome["en"]["branches"] = worktrees["en"]["busy_branches"].clone();
        }
        if self.case.job == OldRootJob::Effect
            && matches!(self.phase, RootPhase::OldEntered | RootPhase::Queued)
        {
            chrome["en"]["worktrees"] = worktrees["en"]["busy_worktrees"].clone();
        }
        frames(
            &host_frame,
            &app_frame,
            (&self.a, &self.app_a),
            (&app_state, &host_state),
            "en",
            fixture,
            &chrome,
            false,
        );
    }
    fn host_switch(&self, host: &mut Sim) {
        self.host_mail
            .as_ref()
            .unwrap()
            .switch(&self.host.state.lock().unwrap().b);
        host.app.poll_projects();
        host.app.poll_projects();
        if host.app.tab.projects.pending() {
            host.app.tab.projects.wait_for_result();
            host.app.poll_projects();
        }
    }
    fn observe(
        &mut self,
        idle: ProjectIdleSnapshot,
        frame: Buffer,
        state: ProjectStateSnapshot,
        host: &mut Sim,
        fixture: &serde_json::Value,
        graph: &serde_json::Value,
    ) -> Next {
        self.fences += 1;
        if idle.is_idle() {
            self.stall = None;
        }
        match self.phase {
            RootPhase::Navigate => {
                if !idle.is_idle() {
                    return self.wait(&idle);
                }
                self.compare(&state, &frame, host, fixture, graph);
                if let Some(key) = self.keys.pop_front() {
                    return self.press(host, key, true);
                }
                self.host_mail.as_mut().unwrap().acknowledge();
                self.app_mail.as_mut().unwrap().acknowledge();
                self.host.arm(false);
                self.app.arm(true);
                self.phase = RootPhase::OldEntered;
                let trigger = if self.case.job == OldRootJob::View {
                    KeyCode::Char('r')
                } else {
                    KeyCode::Enter
                };
                let next = self.press(host, KeyEvent::new(trigger, KeyModifiers::NONE), false);
                self.host.old.as_ref().unwrap().entered();
                next
            }
            RootPhase::OldEntered => {
                self.app.old.as_ref().unwrap().entered();
                self.compare(&state, &frame, host, fixture, graph);
                if self.case.job == OldRootJob::Input {
                    for side in [&self.host, &self.app] {
                        let log = side.state.lock().unwrap();
                        assert_eq!(log.input.as_deref(), Some("main"));
                        assert!(
                            log.input_failed,
                            "the held input reply really is a failed submitted branch action"
                        );
                    }
                    assert!(host
                        .app
                        .project_runtimes
                        .values()
                        .any(|r| r.pending_retry.is_some()));
                }
                self.phase = RootPhase::Queued;
                self.press(
                    host,
                    KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE),
                    false,
                )
            }
            RootPhase::Queued => {
                self.compare(&state, &frame, host, fixture, graph);
                assert_eq!(
                    state.queued_inputs,
                    usize::from(self.case.job == OldRootJob::View),
                    "view queues the real c key; a running act refuses it on both sides"
                );
                self.before = Some(state);
                // Match the completion boundary before konoma's synchronous Tree switch.
                // Its navigation effect is delivered only once the candidate fences finish.
                self.host_mail.as_ref().unwrap().publish(None);
                host.app.poll_projects();
                self.app_mail
                    .as_ref()
                    .unwrap()
                    .switch(&self.app.state.lock().unwrap().b);
                self.phase = RootPhase::Open;
                Next::Timeout
            }
            RootPhase::Open | RootPhase::View => {
                if matches!(self.phase, RootPhase::Open) {
                    self.app.open.as_ref().unwrap().entered();
                } else {
                    self.app.view.as_ref().unwrap().entered();
                }
                let before = self.before.as_ref().unwrap();
                assert_eq!(
                    state.root, before.root,
                    "old root lives until candidate open and first view succeed"
                );
                assert_eq!(
                    state.open_dir, before.open_dir,
                    "old open_dir lives during candidate preparation"
                );
                assert_eq!(
                    state.queued_inputs, before.queued_inputs,
                    "old queued input waits for candidate success or failure"
                );
                assert_eq!(
                    state.prompt, before.prompt,
                    "old queued input must not execute during candidate preparation"
                );
                assert_eq!(state.selected, before.selected);
                assert_eq!(state.target, before.target);
                self.compare(&state, &frame, host, fixture, graph);
                if matches!(self.phase, RootPhase::Open) {
                    if self.case.fail_open {
                        for side in [&self.host, &self.app] {
                            let b = side.state.lock().unwrap().b.clone();
                            std::fs::rename(&b, b.with_extension("held")).unwrap();
                        }
                    }
                    self.app.open.as_mut().unwrap().release();
                    self.phase = if self.case.fail_open {
                        RootPhase::Commit
                    } else {
                        RootPhase::View
                    };
                } else {
                    self.app.view.as_mut().unwrap().release();
                    self.phase = RootPhase::Commit;
                }
                Next::Timeout
            }
            RootPhase::Commit => {
                if !idle.navigation {
                    return self.wait(&idle);
                }
                if self.case.fail_open {
                    self.host_switch(host);
                    assert_eq!(
                        state.root.as_deref(),
                        Some(self.app_a.as_path()),
                        "failed candidate keeps root A"
                    );
                    let before = self.before.as_ref().unwrap();
                    assert_eq!(state.selected, before.selected);
                    assert_eq!(state.target, before.target);
                    assert!(
                        state
                            .notice
                            .as_deref()
                            .is_some_and(|n| n.starts_with("not found: ")),
                        "candidate failure notice: {:?}",
                        state.notice
                    );
                    self.compare(&state, &frame, host, fixture, graph);
                    // Both opens have now failed. Restore the fixture before releasing the
                    // old owner so queued input is independent of filesystem watch ordering.
                    for side in [&self.host, &self.app] {
                        let b = side.state.lock().unwrap().b.clone();
                        std::fs::rename(b.with_extension("held"), &b).unwrap();
                    }
                    self.host.old.as_mut().unwrap().release();
                    self.app.old.as_mut().unwrap().release();
                    self.host_mail.as_mut().unwrap().late(true, "host failure");
                    self.app_mail.as_mut().unwrap().late(true, "app failure");
                    drain(host);
                    self.phase = RootPhase::FailureSettle;
                    return Next::Timeout;
                }
                if !idle.is_idle() {
                    return self.wait(&idle);
                }
                self.host_switch(host);
                drain(host);
                check_switched_tree(host, &state, &self.a, &self.app_a);
                assert_eq!(
                    state.queued_inputs, 0,
                    "replacement discards A's queued key"
                );
                assert!(
                    state.prompt.is_none(),
                    "the old key/input reply cannot open a B prompt"
                );
                self.compare(&state, &frame, host, fixture, graph);
                self.keys = if self.case.return_a {
                    vec![
                        KeyCode::Char('w'),
                        KeyCode::Char('g'),
                        KeyCode::Enter,
                        KeyCode::Char('w'),
                    ]
                } else {
                    vec![KeyCode::Char('w')]
                }
                .into_iter()
                .map(|code| KeyEvent::new(code, KeyModifiers::NONE))
                .collect();
                self.phase = RootPhase::Reopen;
                Next::Timeout
            }
            RootPhase::Reopen => {
                if !idle.is_idle() {
                    return self.wait(&idle);
                }
                if self.expecting_switch {
                    check_switched_tree(host, &state, &self.a, &self.app_a);
                    self.expecting_switch = false;
                }
                self.compare(&state, &frame, host, fixture, graph);
                if let Some(key) = self.keys.pop_front() {
                    self.expecting_switch = key.code == KeyCode::Enter;
                    return self.press(host, key, true);
                }
                if self.case.return_a {
                    assert_eq!(state.root.as_deref(), Some(self.app_a.as_path()));
                    assert_eq!(
                        self.app.state.lock().unwrap().opens_a,
                        2,
                        "returning to A opens a fresh owner"
                    );
                }
                self.late_before = Some(state);
                self.phase = RootPhase::Release;
                Next::Timeout
            }
            RootPhase::Release => {
                self.host.old.as_mut().unwrap().release();
                self.app.old.as_mut().unwrap().release();
                self.host_mail.as_mut().unwrap().late(false, "host");
                self.app_mail.as_mut().unwrap().late(false, "app");
                self.host
                    .dropped
                    .recv_timeout(ROOT_WAIT)
                    .expect("host retires first A after late publication");
                self.app
                    .dropped
                    .recv_timeout(ROOT_WAIT)
                    .expect("app retires first A after late publication");
                self.phase = RootPhase::Late;
                Next::Timeout
            }
            RootPhase::Late => {
                if !idle.is_idle() {
                    return self.wait(&idle);
                }
                drain(host);
                assert_eq!(
                    Some(&state),
                    self.late_before.as_ref(),
                    "late view/input reply/effect changes no field after replacement"
                );
                self.compare(&state, &frame, host, fixture, graph);
                self.phase = RootPhase::Done;
                self.finished = true;
                Next::End
            }
            RootPhase::FailureSettle => {
                if !idle.is_idle() {
                    return self.wait(&idle);
                }
                self.compare(&state, &frame, host, fixture, graph);
                assert!(
                    state.prompt.is_some(),
                    "after failed preparation the retained queued c still executes on A"
                );
                self.failure_cursor = state.cursor;
                self.phase = RootPhase::FailureKeys;
                self.press(host, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), true)
            }
            RootPhase::FailureKeys => {
                if !idle.is_idle() {
                    return self.wait(&idle);
                }
                self.compare(&state, &frame, host, fixture, graph);
                if state.cursor == self.failure_cursor {
                    return self.press(
                        host,
                        KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE),
                        true,
                    );
                }
                assert_eq!(state.root.as_deref(), Some(self.app_a.as_path()));
                assert!(state.prompt.is_none());
                self.finished = true;
                self.phase = RootPhase::Done;
                Next::End
            }
            RootPhase::Done => panic!("the root loop has exactly one final End"),
        }
    }
}

fn continuous_root_replacement_cases() {
    for job in [OldRootJob::View, OldRootJob::Input, OldRootJob::Effect] {
        for return_a in [false, true] {
            let roots = worktree_repositories();
            let text = config("git", "en", "vim", "relative", "right", "")
                .replace("tabbar='hidden'", "tabbar='hidden'\nbusy_indicator=false");
            run_roots_with(
                (&roots.host.root, &roots.app.root),
                "git",
                &text,
                vec![],
                false,
                RunOptions {
                    worktrees: true,
                    root_jobs: Some(RootJobCase {
                        job,
                        return_a,
                        fail_open: false,
                    }),
                    ..Default::default()
                },
                |_, _, _, _| {},
            );
        }
    }
    let roots = worktree_repositories();
    let text = config("git", "en", "vim", "relative", "right", "")
        .replace("tabbar='hidden'", "tabbar='hidden'\nbusy_indicator=false");
    run_roots_with(
        (&roots.host.root, &roots.app.root),
        "git",
        &text,
        vec![],
        false,
        RunOptions {
            worktrees: true,
            root_jobs: Some(RootJobCase {
                job: OldRootJob::View,
                return_a: false,
                fail_open: true,
            }),
            ..Default::default()
        },
        |_, _, _, _| {},
    );
}
