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
