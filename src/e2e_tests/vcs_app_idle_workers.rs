// Worker publication is ordered by channels; the loop receives timeouts without input.
use konoma_plugin::app::project::{ProjectReplyObserver, ProjectServiceInfo, ProjectServices};
use std::sync::atomic::{AtomicBool, Ordering};

enum IdleReply {
    Held(&'static str, std::sync::mpsc::Sender<()>),
    Published(&'static str),
}

#[derive(Clone)]
struct IdleWorkers {
    observer: ProjectReplyObserver,
    replies: Arc<Mutex<std::sync::mpsc::Receiver<IdleReply>>>,
    deferred: Arc<Mutex<std::collections::VecDeque<IdleReply>>>,
    completed: Arc<Mutex<std::collections::BTreeMap<&'static str, usize>>>,
    initial_order: [&'static str; 2],
    busy_key_sent: Arc<AtomicBool>,
    queued: Arc<AtomicBool>,
    gif: Arc<AtomicBool>,
}
impl IdleWorkers {
    fn new(initial_order: [&'static str; 2]) -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        Self {
            observer: Arc::new(move |kind, published| {
                if published {
                    let _ = tx.send(IdleReply::Published(kind));
                } else {
                    let (release, wait) = std::sync::mpsc::channel();
                    if tx.send(IdleReply::Held(kind, release)).is_ok() {
                        wait.recv_timeout(ROOT_WAIT)
                            .expect("release a computed reply's publication fence");
                    }
                }
            }),
            replies: Arc::new(Mutex::new(rx)),
            deferred: Default::default(),
            completed: Default::default(),
            initial_order,
            busy_key_sent: Default::default(),
            queued: Default::default(),
            gif: Default::default(),
        }
    }
    fn release_one(&self) {
        let next = self
            .deferred
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| {
                self.replies
                    .lock()
                    .unwrap()
                    .recv_timeout(ROOT_WAIT)
                    .expect("computed worker reached publication fence")
            });
        let IdleReply::Held(kind, release) = next else {
            panic!("each publication must follow its release");
        };
        release.send(()).unwrap();
        for _ in 0..64 {
            let next = self
                .replies
                .lock()
                .unwrap()
                .recv_timeout(ROOT_WAIT)
                .expect("released reply was published");
            if matches!(next, IdleReply::Published(k) if k == kind) {
                *self.completed.lock().unwrap().entry(kind).or_default() += 1;
                return;
            }
            self.deferred.lock().unwrap().push_back(next);
        }
        panic!("worker publication did not reach its fence");
    }
    fn publications(&self, kind: &'static str) -> usize {
        self.completed
            .lock()
            .unwrap()
            .get(kind)
            .copied()
            .unwrap_or(0)
    }
    fn release_kind(&self, kind: &'static str) {
        let wanted = {
            let mut deferred = self.deferred.lock().unwrap();
            deferred
                .iter()
                .position(|reply| matches!(reply, IdleReply::Held(found, _) if *found == kind))
                .and_then(|index| deferred.remove(index))
        };
        let wanted = wanted.unwrap_or_else(|| {
            for _ in 0..64 {
                let reply = self
                    .replies
                    .lock()
                    .unwrap()
                    .recv_timeout(ROOT_WAIT)
                    .expect("requested worker reached publication fence");
                if matches!(reply, IdleReply::Held(found, _) if found == kind) {
                    return reply;
                }
                self.deferred.lock().unwrap().push_back(reply);
            }
            panic!("{kind} worker must publish before the pending-media frame");
        });
        self.deferred.lock().unwrap().push_front(wanted);
        self.release_one();
    }
    fn services(&self) -> Arc<dyn ProjectServices> {
        Arc::new(IdleServices(self.clone()))
    }
    fn forward<T: Send + 'static>(
        &self,
        kind: &'static str,
        rx: std::sync::mpsc::Receiver<T>,
    ) -> std::sync::mpsc::Receiver<T> {
        let (tx, out) = std::sync::mpsc::channel();
        let observe = self.observer.clone();
        std::thread::spawn(move || {
            while let Ok(reply) = rx.recv() {
                observe(kind, false);
                if tx.send(reply).is_err() {
                    break;
                }
                observe(kind, true);
            }
        });
        out
    }
    fn run<B: Backend>(
        &self,
        terminal: &mut Terminal<B>,
        app: &mut konoma_plugin::app::project::PreparedProjects,
        cx: &ProjectLoopCx,
        mut pictures: Option<konoma_plugin::app::Pictures>,
        mut observe: impl FnMut(
            konoma_plugin::app::project::ProjectIdleSnapshot,
            Buffer,
            ProjectStateSnapshot,
        ) -> Next,
    ) {
        use konoma_plugin::app::{DocPics, EventSource, InProcessSvg};
        if let Some(pictures) = pictures.as_mut() {
            let picker = ratatui_image::picker::Picker::halfblocks();
            let mut cache = konoma_widgets::inline_images::InlineImageCache::default();
            cache.attach_picker(&picker);
            let (tx, requests) = std::sync::mpsc::channel();
            cache.enc_tx = Some(tx);
            let (encoded, replies) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                konoma_widgets::inline_images::md_encode_worker(picker, requests, encoded)
            });
            *pictures.doc_pics() = DocPics::new(
                &ratatui_image::picker::Picker::halfblocks(),
                "git",
                &app.open_dir,
                1024,
                Arc::new(InProcessSvg),
            )
            .with_cache(cache, self.forward("encode", replies));
        }
        struct Events<F> {
            probe: konoma_plugin::app::project::ProjectLoopProbe,
            observe: F,
        }
        impl<
                F: FnMut(
                    konoma_plugin::app::project::ProjectIdleSnapshot,
                    Buffer,
                    ProjectStateSnapshot,
                ) -> Next,
            > EventSource for Events<F>
        {
            fn next(&mut self, wait: Option<std::time::Duration>) -> Next {
                let (idle, frame, state) = self.probe.snapshot();
                if !idle.is_idle() {
                    assert!(wait.is_some_and(|d| d <= std::time::Duration::from_millis(16)), "pending diff/Preview/encode must supply its actual idle deadline: {wait:?}, {idle:?}");
                } else if state
                    .media
                    .as_ref()
                    .is_some_and(|m| m.cache.iter().any(|c| c.frames == 2 && c.frame == 0))
                {
                    let due = wait.expect("GIF frame owes a deadline without input");
                    assert!(
                        due < std::time::Duration::from_millis(100),
                        "GIF owns its deadline, independently of the project watch timer"
                    );
                    let (_keep_input_open, input) = std::sync::mpsc::channel::<()>();
                    assert_eq!(
                        input.recv_timeout(due),
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout)
                    );
                    return Next::Timeout;
                }
                (self.observe)(idle, frame, state)
            }
        }
        let probe = app.observe_loop_for_test();
        konoma_plugin::app::project::run_project_with(
            terminal,
            app,
            &mut Events {
                probe,
                observe: &mut observe,
            },
            cx,
            pictures,
        )
        .unwrap();
    }
    fn assert_finished(&self, path: &str) {
        let counts = self.completed.lock().unwrap();
        assert!(counts.get("source").copied().unwrap_or(0) > 0);
        if path == "doc.md" {
            assert!(counts.get("rendered").copied().unwrap_or(0) > 0);
            assert!(counts.get("preview").copied().unwrap_or(0) > 0);
        } else {
            assert!(counts.get("media").copied().unwrap_or(0) >= 2);
            assert!(counts.get("encode").copied().unwrap_or(0) > 0);
            assert!(self.queued.load(Ordering::SeqCst));
            assert!(self.gif.load(Ordering::SeqCst));
        }
        eprintln!(
            "idle {path}: real replies {counts:?}; no-input timeouts and queue/deadline checked"
        );
    }
}

fn compare_pending_media_key_frame(
    host: &mut Sim,
    state: &ProjectStateSnapshot,
    frame: &Buffer,
    root: &Path,
    app_root: &Path,
) {
    let (mut host_frame, paints) = host.project_frame_with_paints();
    let mut host_snapshot = host_state(host);
    host_snapshot.diff = host_diff_snapshot(host);
    host_snapshot.media = Some(host_media_snapshot(host, &host_snapshot, paints));
    let mut app_snapshot = state.clone();
    let mut app_frame = frame.clone();
    for (snapshot, repo) in [(&mut host_snapshot, root), (&mut app_snapshot, app_root)] {
        normalize_write_state(snapshot, repo);
        if let Some(want) = snapshot
            .media
            .as_mut()
            .and_then(|media| media.queued.as_mut())
        {
            want.0 = Path::new("<repo>").join(want.0.strip_prefix(repo).unwrap());
        }
    }
    normalize_write_frame(&mut host_frame, root);
    normalize_write_frame(&mut app_frame, app_root);
    let cells = compare_frames(&host_frame, &app_frame);
    let values = compare_states(&host_snapshot, &app_snapshot);
    if cells.is_err() || values.is_err() {
        dump_parity_frame("host pending media key frame", &host_frame);
        dump_parity_frame("app pending media key frame", &app_frame);
    }
    assert!(cells.is_ok(), "pending media key frame: {cells:?}");
    assert!(values.is_ok(), "pending media key state: {values:?}");
}

struct IdleServices(IdleWorkers);
impl ProjectServices for IdleServices {
    fn diff_reply_observer_for_test(&self) -> Option<ProjectReplyObserver> {
        Some(self.0.observer.clone())
    }
    fn project_diff_chrome(&self) -> bool {
        true
    }
    fn inspect(
        &self,
        p: &konoma_plugin::host::project::MatchedProject,
        settings: &konoma_host_core::project::ProjectSettings,
    ) -> Result<ProjectServiceInfo, konoma_plugin::Failure> {
        konoma_vcs::standalone::VcsServices.inspect(p, settings)
    }
    fn file_diff(
        &self,
        info: &ProjectServiceInfo,
        path: &Path,
    ) -> Vec<konoma_vcs_core::git::DiffLine> {
        konoma_vcs::standalone::VcsServices.file_diff(info, path)
    }
    fn changed_files(&self, info: &ProjectServiceInfo) -> Vec<konoma_vcs_core::git::ChangeEntry> {
        konoma_vcs::standalone::VcsServices.changed_files(info)
    }
    fn base_contents(&self, info: &ProjectServiceInfo, path: &Path) -> Option<Vec<u8>> {
        konoma_vcs::standalone::VcsServices.base_contents(info, path)
    }
    fn spawn_file_diff(
        self: Arc<Self>,
        request: konoma_plugin::app::diff_screen::FileDiffRequest,
    ) -> Result<konoma_plugin::app::diff_screen::FileDiffReply, konoma_plugin::Failure> {
        Ok(self.0.forward(
            "source",
            request.spawn(Arc::new(konoma_vcs::standalone::VcsServices))?,
        ))
    }
}
#[test]
fn diff_and_preview_workers_finish_while_input_is_idle() {
    if isolated("diff_and_preview_workers_finish_while_input_is_idle") {
        return;
    }
    for (path, initial_order) in [
        ("doc.md", ["source", "resolve"]),
        ("idle.gif", ["source", "resolve"]),
        ("idle.gif", ["resolve", "source"]),
    ] {
        let roots = diff_repositories(|| preview_shape_repo(path));
        let workers = IdleWorkers::new(initial_order);
        let text = config(
            "git",
            "en",
            "vim",
            "relative",
            "right",
            "[plugins.git]\nread_only=true\n",
        )
        .replace("diff_view='source'", "diff_view='rendered'");
        let mut steps = vec![Step::Resize(48, 14), ch('/')];
        steps.extend(typed(path));
        steps.extend([key(KeyCode::Enter), key(KeyCode::Enter)]);
        if path == "idle.gif" {
            steps.push(Step::BusyKey(KeyEvent::new(
                KeyCode::Char('r'),
                KeyModifiers::NONE,
            )));
        }
        if path == "doc.md" {
            steps.extend([ch('R'), ch('R')]);
        }
        steps.push(ch('q'));
        run_roots_with(
            (&roots.host, &roots.app),
            "git",
            &text,
            steps,
            false,
            RunOptions {
                idle_workers: Some(workers.clone()),
                pictures: true,
                media: true,
                launches: Some(Arc::new(Mutex::new(Vec::new()))),
                ..Default::default()
            },
            |_, state, _, _| {
                if path == "idle.gif" {
                    if let Some(media) = state.media.as_ref().filter(|m| !m.cache.is_empty()) {
                        assert!(
                            media.cache.iter().all(|c| c.frames == 2 && c.frame == 1),
                            "GIF's next frame was adopted without a key"
                        );
                    }
                }
            },
        );
        workers.assert_finished(path);
    }
}
