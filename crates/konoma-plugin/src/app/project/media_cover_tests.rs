//! A project screen that covers the diff screen leaves it, as a host tab that is left: the diff
//! screen's media diff (the held result, the ask in flight, the want queued behind it) and its
//! Markdown block diff (the held result, the ask in flight) are dropped, nothing of them lands
//! behind the project screen, and the return asks them again.
use super::*;
use crate::host::project::media_diff::{
    MediaBase, MediaDiffComputed, MediaDiffKind, MediaDiffPictureDecoded, MediaDiffSideDecoded,
    MediaDiffWorker,
};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

const DEADLINE: Duration = Duration::from_secs(10);

/// The VCS program's services: the project diff has its own chrome, so `p` covers it.
struct ChromeServices;
impl crate::app::project::ProjectServices for ChromeServices {
    fn project_diff_chrome(&self) -> bool {
        true
    }
    fn inspect(
        &self,
        p: &MatchedProject,
        s: &crate::app::project::ProjectSettings,
    ) -> Result<crate::app::project::ProjectServiceInfo, Failure> {
        Services.inspect(p, s)
    }
    fn file_diff(&self, _: &crate::app::project::ProjectServiceInfo, _: &Path) -> Vec<DiffLine> {
        Vec::new()
    }
    fn changed_files(&self, _: &crate::app::project::ProjectServiceInfo) -> Vec<ChangeEntry> {
        Vec::new()
    }
    fn base_contents(
        &self,
        _: &crate::app::project::ProjectServiceInfo,
        _: &Path,
    ) -> Option<Vec<u8>> {
        None
    }
    fn spawn_file_diff(self: Arc<Self>, r: FileDiffRequest) -> Result<FileDiffReply, Failure> {
        let (tx, rx) = mpsc::channel();
        tx.send(Ok(FileDiffData {
            lines: Vec::new(),
            paths: vec![r.path],
        }))
        .unwrap();
        Ok(rx)
    }
}

fn computed() -> MediaDiffComputed {
    let side = |old: bool| {
        MediaDiffSideDecoded::Picture(Box::new(MediaDiffPictureDecoded {
            natural_px: (8, 8),
            bytes: 8,
            page_count: None,
            image: image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
                2,
                1,
                image::Rgba([10, 20, 30, 255]),
            )),
            frames: None,
            svg: None,
            svg_base: None,
            cache_key: format!("media-diff://{}/1/64x64", if old { "o" } else { "n" }).into(),
        }))
    };
    MediaDiffComputed::Ready {
        kind: MediaDiffKind::Image,
        base: MediaBase::Head,
        same_bytes: false,
        old: side(true),
        new: side(false),
    }
}

/// Each computation tells the test it started (`entered`) and waits for `release`; its result
/// reaches the diff screen's receiver and then tells the test it was published (`ready`).
struct Worker {
    entered: mpsc::Receiver<()>,
    release: mpsc::Sender<()>,
    ready: mpsc::Receiver<()>,
    calls: Arc<AtomicUsize>,
}

enum Step {
    Key(char),
    /// Wait until a computation started.
    Entered,
    /// Let the started computation finish and wait until its result is in the receiver.
    Land,
}

struct Steps<'a> {
    worker: &'a Worker,
    steps: VecDeque<Step>,
}
impl EventSource for Steps<'_> {
    fn next(&mut self, _: Option<Duration>) -> Next {
        loop {
            match self.steps.pop_front() {
                Some(Step::Key(c)) => return Next::Event(AppEvent::Key(Key::char(c))),
                Some(Step::Entered) => self
                    .worker
                    .entered
                    .recv_timeout(DEADLINE)
                    .expect("a media diff computation is asked"),
                Some(Step::Land) => {
                    self.worker.release.send(()).unwrap();
                    self.worker
                        .ready
                        .recv_timeout(DEADLINE)
                        .expect("the media diff result is published");
                    // One more turn of the loop, which polls whatever is in front.
                    return Next::Timeout;
                }
                None => return Next::End,
            }
        }
    }
}

fn run(
    prepared: &mut PreparedProjects,
    terminal: &mut Terminal<TestBackend>,
    w: &Worker,
    s: Vec<Step>,
) {
    run_project_with(
        terminal,
        prepared,
        &mut Steps {
            worker: w,
            steps: s.into(),
        },
        &ProjectLoopCx::default(),
        None,
    )
    .unwrap();
}

fn held(
    prepared: &PreparedProjects,
) -> (
    bool,
    bool,
    Option<crate::host::project::media_diff::MediaDiffWant>,
) {
    let media = prepared
        .diff
        .as_ref()
        .unwrap()
        .media_snapshot_for_test(None, false);
    (media.held.is_some(), media.pending, media.queued)
}

#[test]
fn covering_the_diff_screen_drops_its_media_diff_and_the_return_asks_it_again() {
    let dir = konoma_test_support::unique_tmp("project_media_cover");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("project.marker"), "").unwrap();
    let path = dir.join("picture.png");
    std::fs::write(&path, b"\x89PNG\r\n\x1a\n").unwrap();
    let mut prepared = crate::app::project::prepare::prepare_projects_with_previews(
        vec![Arc::new(Project)],
        Vec::new(),
        &dir,
        TermCaps::new(),
        (
            SharedConfig::parse("[ui]\nlang='en'\ndiff_view='rendered'\nwrap=false\n").unwrap(),
            None,
        ),
        Arc::new(ChromeServices),
    )
    .unwrap_or_else(|e| panic!("{}", e.error));
    let mut terminal = Terminal::new(TestBackend::new(54, 14)).unwrap();
    opening(&mut prepared, &mut terminal, "changes");

    let (entered_tx, entered) = mpsc::channel();
    let (release, waiting) = mpsc::channel::<()>();
    let waiting = Mutex::new(waiting);
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let mut media = MediaDiffWorker::with_compute(Arc::new(move |_| {
        count.fetch_add(1, Ordering::SeqCst);
        entered_tx.send(()).unwrap();
        waiting.lock().unwrap().recv().unwrap();
        computed()
    }));
    let (tx, rx) = mpsc::channel();
    media.attach(tx);
    let (forward, replies) = mpsc::channel();
    let (published, ready) = mpsc::channel();
    std::thread::spawn(move || {
        while let Ok(r) = rx.recv() {
            if forward.send(r).is_err() || published.send(()).is_err() {
                break;
            }
        }
    });
    let worker = Worker {
        entered,
        release,
        ready,
        calls,
    };

    // Open the picture's diff through the project's own navigation (the VCS program's road).
    let mut state = ProjectLoopState::new(&mut prepared, false);
    let info = state.info(&prepared, &ProjectLoopCx::default());
    let mut update = ProjectUpdate::handled();
    update.commands.push(ProjectCommand::Navigation {
        outcome: Outcome::OpenDiff { path: path.clone() },
        save_return: true,
    });
    state.apply(&mut prepared, &info, update);
    assert!(state.diff.as_ref().unwrap().project_owned);
    state.diff.as_mut().unwrap().settle_preview_for_test();
    prepared.diff = Some(state.diff.take().unwrap().with_media_worker(media, replies));
    prepared.screen = state.screen.take();
    prepared.runtimes = std::mem::take(&mut state.runtimes);

    // 1. The diff screen in front asks its media diff, and the result lands there.
    run(
        &mut prepared,
        &mut terminal,
        &worker,
        vec![Step::Entered, Step::Land],
    );
    assert!(prepared.diff_return.is_some() && !prepared.preview_project);
    assert_eq!(held(&prepared), (true, false, None), "landed in front");
    assert_eq!(worker.calls.load(Ordering::SeqCst), 1);

    // 2. `p` covers it: the held result goes, and nothing is asked behind the project screen.
    run(&mut prepared, &mut terminal, &worker, vec![Step::Key('p')]);
    assert!(
        prepared.preview_project,
        "the project screen covers the diff"
    );
    assert_eq!(
        held(&prepared),
        (false, false, None),
        "covering is leaving: the media diff is not held behind the project screen"
    );
    assert_eq!(worker.calls.load(Ordering::SeqCst), 1);

    // 3. `q` returns and asks again; `p` covers while that ask is out, and its result is
    // published while the project screen is in front.
    run(
        &mut prepared,
        &mut terminal,
        &worker,
        vec![Step::Key('q'), Step::Entered, Step::Key('p'), Step::Land],
    );
    assert!(prepared.preview_project);
    assert!(
        !held(&prepared).0,
        "a result published behind the project screen is not applied there"
    );
    // The host takes the answer in while covered (it is refused by generation) and its worker
    // is free again; the covered diff screen does the same, so nothing stays pending (and wakes
    // the loop) behind the project screen.
    assert_eq!(
        held(&prepared),
        (false, false, None),
        "the answer of the ask made before the cover left the worker pending behind the project screen"
    );
    assert_eq!(worker.calls.load(Ordering::SeqCst), 2);

    // 4. `q` returns: the answer of the ask made before the cover is refused (another
    // generation), and the return asks the media diff again; that answer lands in front.
    run(
        &mut prepared,
        &mut terminal,
        &worker,
        vec![Step::Key('q'), Step::Entered, Step::Land],
    );
    assert!(!prepared.preview_project && prepared.diff_return.is_some());
    assert_eq!(
        worker.calls.load(Ordering::SeqCst),
        3,
        "the return asks again"
    );
    assert_eq!(
        held(&prepared),
        (true, false, None),
        "the media diff comes back after the return"
    );
}

fn held_markdown(prepared: &PreparedProjects) -> (bool, bool) {
    prepared
        .diff
        .as_ref()
        .unwrap()
        .markdown_held_and_pending_for_test()
}

/// The Markdown block diff (the Rendered presentation of a Markdown diff) is the left screen's
/// too: covering drops the held result and the ask in flight, as the host's `leave_tab_view`
/// drops them (`App::invalidate_md_diff`), an answer published behind the project screen is not
/// applied there, and the return asks the block diff again.
#[test]
fn covering_the_diff_screen_drops_its_markdown_block_diff_and_the_return_asks_it_again() {
    use crate::host::project::md_diff::MdDiffWorker;
    let dir = konoma_test_support::unique_tmp("project_markdown_cover");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("project.marker"), "").unwrap();
    let path = dir.join("notes.md");
    std::fs::write(&path, "# Notes\n\nA paragraph.\n").unwrap();
    let mut prepared = crate::app::project::prepare::prepare_projects_with_previews(
        vec![Arc::new(Project)],
        Vec::new(),
        &dir,
        TermCaps::new(),
        (
            SharedConfig::parse("[ui]\nlang='en'\ndiff_view='rendered'\nwrap=false\n").unwrap(),
            None,
        ),
        Arc::new(ChromeServices),
    )
    .unwrap_or_else(|e| panic!("{}", e.error));
    let mut terminal = Terminal::new(TestBackend::new(54, 14)).unwrap();
    opening(&mut prepared, &mut terminal, "changes");

    let (entered_tx, entered) = mpsc::channel();
    let (release, waiting) = mpsc::channel::<()>();
    let waiting = Mutex::new(waiting);
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let mut markdown = MdDiffWorker::with_compute(Arc::new(move |req| {
        count.fetch_add(1, Ordering::SeqCst);
        entered_tx.send(()).unwrap();
        waiting.lock().unwrap().recv().unwrap();
        MdDiffWorker::compute_md_diff(req)
    }));
    let (tx, rx) = mpsc::channel();
    markdown.attach(tx);
    let (forward, replies) = mpsc::channel();
    let (published, ready) = mpsc::channel();
    std::thread::spawn(move || {
        while let Ok(r) = rx.recv() {
            if forward.send(r).is_err() || published.send(()).is_err() {
                break;
            }
        }
    });
    let worker = Worker {
        entered,
        release,
        ready,
        calls,
    };

    // Open the document's diff through the project's own navigation (the VCS program's road),
    // then hand it the controlled block-diff worker and read it again so that worker is asked.
    let mut state = ProjectLoopState::new(&mut prepared, false);
    let info = state.info(&prepared, &ProjectLoopCx::default());
    let mut update = ProjectUpdate::handled();
    update.commands.push(ProjectCommand::Navigation {
        outcome: Outcome::OpenDiff { path: path.clone() },
        save_return: true,
    });
    state.apply(&mut prepared, &info, update);
    assert!(state.diff.as_ref().unwrap().project_owned);
    state.diff.as_mut().unwrap().settle_preview_for_test();
    let mut diff = state
        .diff
        .take()
        .unwrap()
        .with_markdown_worker_for_test(markdown, replies);
    diff.reload().unwrap();
    diff.settle_preview_for_test();
    prepared.diff = Some(diff);
    prepared.screen = state.screen.take();
    prepared.runtimes = std::mem::take(&mut state.runtimes);

    // 1. The diff screen in front asks its block diff, and the result lands there.
    run(
        &mut prepared,
        &mut terminal,
        &worker,
        vec![Step::Entered, Step::Land],
    );
    assert!(prepared.diff_return.is_some() && !prepared.preview_project);
    assert_eq!(held_markdown(&prepared), (true, false), "landed in front");
    assert_eq!(worker.calls.load(Ordering::SeqCst), 1);

    // 2. `p` covers it: the held result goes, and nothing is asked behind the project screen.
    run(&mut prepared, &mut terminal, &worker, vec![Step::Key('p')]);
    assert!(
        prepared.preview_project,
        "the project screen covers the diff"
    );
    assert_eq!(
        held_markdown(&prepared),
        (false, false),
        "covering is leaving: the block diff is not held behind the project screen"
    );
    assert_eq!(worker.calls.load(Ordering::SeqCst), 1);

    // 3. `q` returns and asks again; `p` covers while that ask is out, and its result is
    // published while the project screen is in front.
    run(
        &mut prepared,
        &mut terminal,
        &worker,
        vec![Step::Key('q'), Step::Entered, Step::Key('p'), Step::Land],
    );
    assert!(prepared.preview_project);
    assert_eq!(worker.calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        held_markdown(&prepared),
        (false, false),
        "the ask made before the cover is still the covered screen's: held or waited for behind \
         the project screen"
    );

    // 4. `q` returns: the answer of the ask made before the cover is refused (another
    // generation), and the return asks the block diff again; that answer lands in front.
    run(
        &mut prepared,
        &mut terminal,
        &worker,
        vec![Step::Key('q'), Step::Entered, Step::Land],
    );
    assert!(!prepared.preview_project && prepared.diff_return.is_some());
    assert_eq!(
        worker.calls.load(Ordering::SeqCst),
        3,
        "the return asks again"
    );
    assert_eq!(
        held_markdown(&prepared),
        (true, false),
        "the block diff comes back after the return"
    );
}

/// What the diff screen asked its workers: raw file-diff reads and target-kind resolutions.
#[derive(Default)]
struct Asked {
    file_diffs: AtomicUsize,
    resolves: AtomicUsize,
}

/// Holds the target-kind answers (`resolve`) once armed: each waits for a permit before it is
/// put in the diff screen's receiver. A held answer reports `Delivered` when its forwarding is
/// over, whether the answer reached the receiver or the receiver was already dropped.
struct ResolveGate {
    armed: std::sync::atomic::AtomicBool,
    permits: Mutex<mpsc::Receiver<()>>,
    delivered: Mutex<mpsc::Sender<()>>,
}

/// Reports the end of one held answer's forwarding (the observer of that receiver is dropped
/// when its forwarding thread ends).
struct HeldAnswer {
    gate: Arc<ResolveGate>,
    held: std::sync::atomic::AtomicBool,
}
impl Drop for HeldAnswer {
    fn drop(&mut self) {
        if self.held.load(Ordering::SeqCst) {
            let _ = self.gate.delivered.lock().unwrap().send(());
        }
    }
}

/// The VCS program's services (own diff chrome) that count what is asked and gate the
/// target-kind answers.
struct GatedServices {
    asked: Arc<Asked>,
    gate: Arc<ResolveGate>,
}
impl crate::app::project::ProjectServices for GatedServices {
    fn diff_reply_observer_for_test(&self) -> Option<crate::app::project::ProjectReplyObserver> {
        let held = HeldAnswer {
            gate: self.gate.clone(),
            held: Default::default(),
        };
        let asked = self.asked.clone();
        Some(Arc::new(move |kind, published| {
            if kind != "resolve" || published {
                return;
            }
            asked.resolves.fetch_add(1, Ordering::SeqCst);
            if held.gate.armed.load(Ordering::SeqCst) {
                held.held.store(true, Ordering::SeqCst);
                held.gate
                    .permits
                    .lock()
                    .unwrap()
                    .recv_timeout(DEADLINE)
                    .expect("the test lets the held target-kind answer go");
            }
        }))
    }
    fn project_diff_chrome(&self) -> bool {
        true
    }
    fn inspect(
        &self,
        p: &MatchedProject,
        s: &crate::app::project::ProjectSettings,
    ) -> Result<crate::app::project::ProjectServiceInfo, Failure> {
        Services.inspect(p, s)
    }
    fn file_diff(&self, _: &crate::app::project::ProjectServiceInfo, _: &Path) -> Vec<DiffLine> {
        Vec::new()
    }
    fn changed_files(&self, _: &crate::app::project::ProjectServiceInfo) -> Vec<ChangeEntry> {
        Vec::new()
    }
    fn base_contents(
        &self,
        _: &crate::app::project::ProjectServiceInfo,
        _: &Path,
    ) -> Option<Vec<u8>> {
        None
    }
    fn spawn_file_diff(self: Arc<Self>, r: FileDiffRequest) -> Result<FileDiffReply, Failure> {
        self.asked.file_diffs.fetch_add(1, Ordering::SeqCst);
        crate::app::project::ProjectServices::spawn_file_diff(Arc::new(ChromeServices), r)
    }
}

/// A version-control project (its `changes` home screen launches the tool with `!`).
struct VcsProject;
impl Plugin for VcsProject {
    fn manifest(&self) -> Manifest {
        Manifest::new("preview-project", "0").project(
            ProjectDecl::new()
                .version_control()
                .marker(DetectionMarker::new("project.marker", MarkerKind::File))
                .screen(ProjectScreen::new("changes", "Changes"))
                .screen(ProjectScreen::new("log", "Log")),
        )
    }
    fn open(&self, _: Input, _: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        Ok(Box::new(ProjectSession))
    }
}

/// The file plugin that makes `*.md` a Markdown diff target (a document view), so the diff
/// screen's representation waits for the target-kind answer as in the VCS program.
struct MarkdownDocument;
impl Plugin for MarkdownDocument {
    fn manifest(&self) -> Manifest {
        Manifest::new("cover-markdown", "0")
            .bind(Bind::files(["*.md"]))
            .shows(ViewKind::Document)
    }
    fn open(&self, _: Input, _: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        unreachable!("the Rendered diff does not open the file preview")
    }
}

enum GatedStep {
    Key(char),
    /// Turn the loop until a block-diff computation started (the return asks the target kind
    /// first, and the loop adopts its answer before the block diff is asked).
    Entered,
    /// Let the started block-diff computation finish and wait until its result is published.
    Land,
    /// Let the held target-kind answer go and wait until its forwarding is over.
    Resolve,
    /// Turn the loop until the condition holds (a key sent while the project screen's view
    /// loads is replayed later, and a queued launch is handed over at the top of a turn).
    Until(Box<dyn Fn() -> bool>),
    /// Turn the loop until it no longer asks to be woken in 16 ms: the project screen's view
    /// has loaded and nothing owed is outstanding (keys sent while a screen loads are replayed
    /// later, not handled when sent).
    Settle,
}

struct GatedSteps<'a> {
    worker: &'a Worker,
    gate: &'a ResolveGate,
    permit: &'a mpsc::Sender<()>,
    delivered: &'a mpsc::Receiver<()>,
    steps: VecDeque<GatedStep>,
    settle: Option<std::time::Instant>,
}
impl EventSource for GatedSteps<'_> {
    fn next(&mut self, wait: Option<Duration>) -> Next {
        loop {
            match self.steps.pop_front() {
                Some(GatedStep::Settle) => {
                    let since = *self.settle.get_or_insert_with(std::time::Instant::now);
                    if wait == Some(Duration::from_millis(16)) {
                        assert!(
                            since.elapsed() < DEADLINE,
                            "the loop keeps asking to be woken in 16 ms"
                        );
                        self.steps.push_front(GatedStep::Settle);
                        std::thread::yield_now();
                        return Next::Timeout;
                    }
                    self.settle = None;
                }
                Some(GatedStep::Key(c)) => return Next::Event(AppEvent::Key(Key::char(c))),
                Some(GatedStep::Until(done)) => {
                    let since = *self.settle.get_or_insert_with(std::time::Instant::now);
                    if done() {
                        self.settle = None;
                        continue;
                    }
                    assert!(
                        since.elapsed() < DEADLINE,
                        "the awaited condition never held"
                    );
                    self.steps.push_front(GatedStep::Until(done));
                    std::thread::yield_now();
                    return Next::Timeout;
                }
                Some(GatedStep::Entered) => {
                    let since = *self.settle.get_or_insert_with(std::time::Instant::now);
                    if self.worker.entered.try_recv().is_ok() {
                        self.settle = None;
                        continue;
                    }
                    assert!(
                        since.elapsed() < DEADLINE,
                        "a block diff computation is asked"
                    );
                    self.steps.push_front(GatedStep::Entered);
                    std::thread::yield_now();
                    return Next::Timeout;
                }
                Some(GatedStep::Land) => {
                    self.worker.release.send(()).unwrap();
                    self.worker
                        .ready
                        .recv_timeout(DEADLINE)
                        .expect("the block diff result is published");
                    return Next::Timeout;
                }
                Some(GatedStep::Resolve) => {
                    assert!(self.gate.armed.load(Ordering::SeqCst));
                    self.permit.send(()).unwrap();
                    self.delivered
                        .recv_timeout(DEADLINE)
                        .expect("the held target-kind answer is forwarded");
                    // One more turn of the loop, which polls whatever it polls in this state.
                    return Next::Timeout;
                }
                None => return Next::End,
            }
        }
    }
}

struct GatedFixture {
    prepared: PreparedProjects,
    terminal: Terminal<TestBackend>,
    worker: Worker,
    asked: Arc<Asked>,
    gate: Arc<ResolveGate>,
    permit: mpsc::Sender<()>,
    delivered: mpsc::Receiver<()>,
}

impl GatedFixture {
    /// A Markdown diff opened through the project's own navigation, its target kind answered
    /// and a controlled block-diff worker attached; nothing has run the loop yet.
    fn new(name: &str, plugin: Arc<dyn Plugin>, config: &str) -> Self {
        use crate::host::project::md_diff::MdDiffWorker;
        let dir = konoma_test_support::unique_tmp(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("project.marker"), "").unwrap();
        let path = dir.join("notes.md");
        std::fs::write(&path, "# Notes\n\nA paragraph.\n").unwrap();
        let asked = Arc::new(Asked::default());
        let (permit, permits) = mpsc::channel();
        let (delivered_tx, delivered) = mpsc::channel();
        let gate = Arc::new(ResolveGate {
            armed: Default::default(),
            permits: Mutex::new(permits),
            delivered: Mutex::new(delivered_tx),
        });
        let mut prepared = crate::app::project::prepare::prepare_projects_with_previews(
            vec![plugin],
            vec![Arc::new(MarkdownDocument)],
            &dir,
            TermCaps::new(),
            (SharedConfig::parse(config).unwrap(), None),
            Arc::new(GatedServices {
                asked: asked.clone(),
                gate: gate.clone(),
            }),
        )
        .unwrap_or_else(|e| panic!("{}", e.error));
        let mut terminal = Terminal::new(TestBackend::new(54, 14)).unwrap();
        opening(&mut prepared, &mut terminal, "changes");

        let (entered_tx, entered) = mpsc::channel();
        let (release, waiting) = mpsc::channel::<()>();
        let waiting = Mutex::new(waiting);
        let calls = Arc::new(AtomicUsize::new(0));
        let count = calls.clone();
        let mut markdown = MdDiffWorker::with_compute(Arc::new(move |req| {
            count.fetch_add(1, Ordering::SeqCst);
            entered_tx.send(()).unwrap();
            waiting.lock().unwrap().recv().unwrap();
            MdDiffWorker::compute_md_diff(req)
        }));
        let (tx, rx) = mpsc::channel();
        markdown.attach(tx);
        let (forward, replies) = mpsc::channel();
        let (published, ready) = mpsc::channel();
        std::thread::spawn(move || {
            while let Ok(r) = rx.recv() {
                if forward.send(r).is_err() || published.send(()).is_err() {
                    break;
                }
            }
        });
        let worker = Worker {
            entered,
            release,
            ready,
            calls,
        };

        let mut state = ProjectLoopState::new(&mut prepared, false);
        let info = state.info(&prepared, &ProjectLoopCx::default());
        let mut update = ProjectUpdate::handled();
        update.commands.push(ProjectCommand::Navigation {
            outcome: Outcome::OpenDiff { path },
            save_return: true,
        });
        state.apply(&mut prepared, &info, update);
        assert!(state.diff.as_ref().unwrap().project_owned);
        state.diff.as_mut().unwrap().settle_preview_for_test();
        let mut diff = state
            .diff
            .take()
            .unwrap()
            .with_markdown_worker_for_test(markdown, replies);
        diff.reload().unwrap();
        diff.settle_preview_for_test();
        prepared.diff = Some(diff);
        prepared.screen = state.screen.take();
        prepared.runtimes = std::mem::take(&mut state.runtimes);
        Self {
            prepared,
            terminal,
            worker,
            asked,
            gate,
            permit,
            delivered,
        }
    }

    fn run(&mut self, cx: &ProjectLoopCx, steps: Vec<GatedStep>) {
        run_project_with(
            &mut self.terminal,
            &mut self.prepared,
            &mut GatedSteps {
                worker: &self.worker,
                gate: &self.gate,
                permit: &self.permit,
                delivered: &self.delivered,
                steps: steps.into(),
                settle: None,
            },
            cx,
            None,
        )
        .unwrap();
    }

    fn asked(&self) -> (usize, usize, usize) {
        (
            self.asked.file_diffs.load(Ordering::SeqCst),
            self.asked.resolves.load(Ordering::SeqCst),
            self.worker.calls.load(Ordering::SeqCst),
        )
    }

    fn diff(&self) -> &crate::app::diff_screen::DiffScreenState {
        self.prepared.diff.as_ref().unwrap()
    }
}

/// The target-kind answer of the read made for a return is the diff screen's view too: a cover
/// that comes before it drops it with the rest of the left screen. Answering behind the project
/// screen starts no block diff (nothing pending, no 16 ms wake-up for it), and the next return
/// asks the kind, the raw diff and the block diff once each.
#[test]
fn a_target_kind_answer_landing_behind_the_project_screen_starts_no_block_diff() {
    let mut f = GatedFixture::new(
        "project_markdown_cover_kind",
        Arc::new(Project),
        "[ui]\nlang='en'\ndiff_view='rendered'\nwrap=false\n",
    );
    let cx = ProjectLoopCx::default();

    // 1. The diff screen in front asks its block diff, and the result lands there; `p` covers.
    f.run(
        &cx,
        vec![
            GatedStep::Entered,
            GatedStep::Land,
            GatedStep::Key('p'),
            GatedStep::Settle,
        ],
    );
    assert!(f.prepared.preview_project);
    assert_eq!(held_markdown(&f.prepared), (false, false));
    assert_eq!(f.worker.calls.load(Ordering::SeqCst), 1);

    // 2. `q` returns (a raw read and a target-kind resolution are asked; the kind answer is held),
    // `p` covers again before the kind answers, and the answer then arrives behind the project
    // screen.
    f.gate.armed.store(true, Ordering::SeqCst);
    let before = f.asked();
    f.run(
        &cx,
        vec![GatedStep::Key('q'), GatedStep::Key('p'), GatedStep::Resolve],
    );
    assert!(
        f.prepared.preview_project,
        "the project screen covers the diff"
    );
    assert_eq!(
        (f.asked().0 - before.0, f.asked().1 - before.1),
        (1, 1),
        "the return asked the raw diff and the target kind once"
    );
    assert_eq!(
        held_markdown(&f.prepared),
        (false, false),
        "a target-kind answer behind the project screen started a block diff there"
    );
    assert_eq!(
        f.diff().wake_in(),
        None,
        "the covered diff screen keeps waking the loop for answers nobody reads behind the \
         project screen"
    );

    // 3. `q` returns: the kind, the raw diff and the block diff are asked once each, and the
    // block diff lands in front.
    let before = f.asked();
    f.run(
        &cx,
        vec![
            GatedStep::Settle,
            GatedStep::Key('q'),
            GatedStep::Resolve,
            GatedStep::Entered,
            GatedStep::Land,
        ],
    );
    assert!(!f.prepared.preview_project && f.prepared.diff_return.is_some());
    assert_eq!(
        (
            f.asked().0 - before.0,
            f.asked().1 - before.1,
            f.asked().2 - before.2
        ),
        (1, 1, 1),
        "the return asks the raw diff, the target kind and the block diff once each"
    );
    assert_eq!(
        held_markdown(&f.prepared),
        (true, false),
        "the block diff comes back after the return"
    );
}

/// The return from the external tool launched on the covering project screen refreshes that
/// project screen only: the diff screen under it is read again on its own return (once), not
/// behind the project screen.
#[test]
fn the_tool_return_behind_the_project_screen_reads_the_diff_screen_on_its_return_only() {
    let mut f = GatedFixture::new(
        "project_markdown_cover_tool",
        Arc::new(VcsProject),
        "[ui]\nlang='en'\ndiff_view='rendered'\nwrap=false\n[git]\ntool='fixture-git-tool'\n",
    );
    let launches = Arc::new(AtomicUsize::new(0));
    let counted = launches.clone();
    let cx = ProjectLoopCx {
        home: None,
        launch: Some(
            crate::app::LaunchCx::new(
                f.prepared.config.editor.clone(),
                &f.prepared.config.external,
                vec![],
            )
            .with_hand_off(Arc::new(move |_, _, redraw| {
                counted.fetch_add(1, Ordering::SeqCst);
                redraw()?;
                Ok(Some(0))
            })),
        ),
    };

    // 1. The block diff lands in front and `p` covers; then `!` hands the tool over and the
    // loop comes back from it.
    f.run(
        &cx,
        vec![
            GatedStep::Entered,
            GatedStep::Land,
            GatedStep::Key('p'),
            GatedStep::Settle,
        ],
    );
    assert!(f.prepared.preview_project);
    let covered = f.asked();
    let handed = launches.clone();
    f.run(
        &cx,
        vec![
            GatedStep::Key('!'),
            GatedStep::Until(Box::new(move || handed.load(Ordering::SeqCst) == 1)),
        ],
    );
    assert_eq!(
        launches.load(Ordering::SeqCst),
        1,
        "the tool was handed over"
    );
    assert!(
        f.prepared.preview_project,
        "the project screen covers the diff"
    );
    let after_tool = f.asked();
    assert_eq!(
        (after_tool.0, after_tool.1),
        (covered.0, covered.1),
        "the tool return read the covered diff screen again behind the project screen"
    );
    assert_eq!(held_markdown(&f.prepared), (false, false));
    assert_eq!(f.diff().wake_in(), None);

    // 2. `q` returns: the diff screen is read again once (one raw read, one target kind, one
    // block diff), and the block diff lands in front.
    f.run(
        &cx,
        vec![GatedStep::Key('q'), GatedStep::Entered, GatedStep::Land],
    );
    assert!(!f.prepared.preview_project && f.prepared.diff_return.is_some());
    let back = f.asked();
    assert_eq!(
        (
            back.0 - after_tool.0,
            back.1 - after_tool.1,
            back.2 - after_tool.2
        ),
        (1, 1, 1),
        "the return reads the diff screen again once"
    );
    assert_eq!(held_markdown(&f.prepared), (true, false));
}
