//! Project discovery and watching. All directory reads and watch setup run on a worker.

use std::collections::BTreeSet;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Condvar, Mutex};
use std::time::Duration;

use notify::{RecursiveMode, Watcher};

use crate::project::{MarkerKind, ProjectDecl};

#[cfg(test)]
mod fence_tests;
#[cfg(test)]
mod tests;

/// A project found for a directory: the plugin that declared it, the directory that carries the
/// marker, and the declaration itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MatchedProject {
    pub plugin: String,
    pub root: PathBuf,
    /// The actual entry name, not the declaration's glob.
    pub marker: String,
    pub declaration: ProjectDecl,
    pub session_watch_paths: Option<Vec<PathBuf>>,
}

impl MatchedProject {
    fn watch_paths(&self) -> Vec<PathBuf> {
        self.session_watch_paths.clone().unwrap_or_else(|| {
            self.declaration
                .watch_paths
                .iter()
                .map(|p| self.root.join(p))
                .collect()
        })
    }
}

/// The project declaration of one plugin, as the host read it from its registry.
#[derive(Clone, Debug)]
pub struct ProjectDeclaration {
    /// The id of the plugin that declares the project.
    pub plugin: String,
    pub project: ProjectDecl,
}

/// What a host asks discovery to look for. Which plugins belong here (enabled, linked into the
/// host rather than run as a child process) is the host's decision.
pub type ProjectDeclarationSet = Vec<ProjectDeclaration>;

/// Project declarations of enabled registrations in registry order. The host decides which
/// instances are static; the SDK does not know about its process-plugin catalogue.
pub fn project_declarations(
    registry: &crate::registry::Registry,
    include: impl Fn(&Arc<dyn crate::Plugin>) -> bool,
) -> ProjectDeclarationSet {
    registry
        .ids()
        .into_iter()
        .filter(|id| registry.is_enabled(id) && registry.plugin(id).is_some_and(|p| include(&p)))
        .filter_map(|id| {
            Some(ProjectDeclaration {
                plugin: id.into(),
                project: registry.manifest(id)?.project.clone()?,
            })
        })
        .collect()
}

type WatchKey = (String, PathBuf, String);
struct SessionWatches {
    paths: Mutex<std::collections::BTreeMap<WatchKey, Vec<PathBuf>>>,
    dirty: AtomicBool,
    wake: mpsc::SyncSender<WatchMessage>,
}
impl SessionWatches {
    fn apply(&self, projects: &mut [MatchedProject]) {
        let paths = self.paths.lock().unwrap_or_else(|e| e.into_inner());
        for project in projects {
            project.session_watch_paths = paths
                .get(&(
                    project.plugin.clone(),
                    project.root.clone(),
                    project.marker.clone(),
                ))
                .cloned();
        }
    }
}

/// Search `root` and each directory above it for the declared markers.
pub fn detect(
    root: &Path,
    declarations: &[ProjectDeclaration],
    cancelled: &AtomicBool,
) -> io::Result<Vec<MatchedProject>> {
    detect_directories(root.ancestors(), declarations, cancelled)
}

/// Search the given directories (in order) for the declared markers. A directory that cannot be
/// read, or a cancellation, is an error: the caller keeps its previous answer.
pub fn detect_directories<'a>(
    directories: impl IntoIterator<Item = &'a Path>,
    declarations: &[ProjectDeclaration],
    cancelled: &AtomicBool,
) -> io::Result<Vec<MatchedProject>> {
    let mut out = Vec::new();
    for directory in directories {
        if cancelled.load(Ordering::Relaxed) {
            return Err(io::ErrorKind::Interrupted.into());
        }
        let mut entries = std::fs::read_dir(directory)?.collect::<io::Result<Vec<_>>>()?;
        entries.sort_by_key(|e| e.file_name());
        for decl in declarations {
            for marker in &decl.project.markers {
                let pattern = globset::Glob::new(&marker.name)
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?
                    .compile_matcher();
                for entry in &entries {
                    let name = entry.file_name();
                    let Some(name) = name.to_str() else {
                        continue;
                    };
                    if !pattern.is_match(Path::new(name)) {
                        continue;
                    }
                    let kind_matches = match marker.kind {
                        MarkerKind::Any => true,
                        MarkerKind::File => entry.path().is_file(),
                        MarkerKind::Directory => entry.path().is_dir(),
                    };
                    if !kind_matches {
                        continue;
                    }
                    if out.iter().any(|p: &MatchedProject| {
                        p.plugin == decl.plugin && p.root == directory && p.marker == name
                    }) {
                        continue;
                    }
                    out.push(MatchedProject {
                        session_watch_paths: None,
                        plugin: decl.plugin.clone(),
                        root: directory.into(),
                        marker: name.into(),
                        declaration: decl.project.clone(),
                    });
                }
            }
        }
    }
    Ok(out)
}

/// Whether a change to any of `paths` can alter the answer for `root` (an empty list means
/// "unknown", which always does).
pub fn affected(
    root: &Path,
    declarations: &[ProjectDeclaration],
    projects: &[MatchedProject],
    paths: &[PathBuf],
) -> bool {
    paths.is_empty()
        || paths.iter().any(|path| {
            projects.iter().any(|p| {
                p.watch_paths()
                    .iter()
                    .any(|watch| path.starts_with(watch) || watch.starts_with(path))
            }) || (path.parent().is_some_and(|parent| root.starts_with(parent))
                && declarations.iter().any(|d| {
                    d.project.markers.iter().any(|m| {
                        globset::Glob::new(&m.name).is_ok_and(|g| {
                            path.file_name()
                                .is_some_and(|name| g.compile_matcher().is_match(Path::new(name)))
                        })
                    })
                }))
        })
}

/// Watch an existing directory above the target, including for a watch path not created yet.
fn watch_directory(path: &Path) -> Option<PathBuf> {
    path.ancestors().find(|p| p.is_dir()).map(Path::to_path_buf)
}

/// The directories the worker watches for `root` and its projects, each with whether the watch is
/// recursive.
pub fn watch_targets(root: &Path, projects: &[MatchedProject]) -> BTreeSet<(PathBuf, bool)> {
    let mut out: BTreeSet<_> = root.ancestors().map(|p| (p.to_path_buf(), false)).collect();
    for project in projects {
        for path in project.watch_paths() {
            if let Some(dir) = watch_directory(&path) {
                // Declared directories may have nested changes. Missing paths are covered by
                // their nearest existing ancestor, which also catches their later creation.
                out.insert((dir, true));
            }
        }
    }
    // A recursive watch subsumes the non-recursive watch of the same directory.
    let recursive: Vec<_> = out
        .iter()
        .filter(|(_, r)| *r)
        .map(|(p, _)| p.clone())
        .collect();
    for path in recursive {
        out.remove(&(path, false));
    }
    out
}

#[derive(Default)]
struct Slot {
    value: Mutex<Option<Update>>,
    ready: Condvar,
    published: AtomicBool,
    #[cfg(any(test, feature = "test-hooks"))]
    injected: AtomicBool,
    #[cfg(test)]
    checkpoints: Mutex<Option<fence_tests::WorkerCheckpoints>>,
    #[cfg(all(test, feature = "app"))]
    startup_failure: Option<StartupFailure>,
}

#[cfg(all(test, feature = "app"))]
#[derive(Clone, Copy)]
pub(crate) enum StartupFailure {
    Panic,
    Exit,
    WatcherCreation,
}

struct Update {
    projects: Option<Vec<MatchedProject>>,
    failed: bool,
}

impl Slot {
    fn publish(&self, projects: Option<Vec<MatchedProject>>, failed: bool) {
        let mut slot = self.value.lock().unwrap_or_else(|e| e.into_inner());
        let mut previous = slot.replace(Update { projects, failed });
        if let (Some(current), Some(previous)) = (slot.as_mut(), previous.as_mut()) {
            if current.projects.is_none() {
                current.projects = previous.projects.take();
            }
        }
        self.published.store(true, Ordering::Release);
        drop(slot);
        self.ready.notify_all();
        drop(previous);
    }
}

/// The handle keeps the slot alive even after its thread exits. Publish explicitly rather than
/// relying on channel disconnection; a panic after a consumed answer must also wake the host.
struct PublishOnExit<'a>(&'a Slot);

impl Drop for PublishOnExit<'_> {
    fn drop(&mut self) {
        if std::thread::panicking() || !self.0.published.load(Ordering::Acquire) {
            self.0.publish(None, true);
        }
    }
}

struct Worker {
    watches: Arc<SessionWatches>,
    slot: Arc<Slot>,
    cancelled: Arc<AtomicBool>,
    #[cfg(any(test, feature = "test-hooks"))]
    events: Option<EventSender>,
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
}

enum WatchMessage {
    PathsChanged,
    Event(notify::Result<notify::Event>),
    /// Acknowledge only after preceding events and any resulting scan have completed.
    #[cfg(any(test, feature = "test-hooks"))]
    Settled(mpsc::SyncSender<()>),
}

#[derive(Clone)]
struct EventSender {
    tx: mpsc::SyncSender<WatchMessage>,
    overflow: Arc<AtomicBool>,
    failed: Arc<AtomicBool>,
}

fn enqueue_event(
    tx: &mpsc::SyncSender<WatchMessage>,
    overflow: &AtomicBool,
    failed: &AtomicBool,
    event: notify::Result<notify::Event>,
    #[cfg(test)] before_overflow: impl FnOnce(),
) {
    if matches!(&event, Ok(event) if matches!(&event.kind, notify::EventKind::Access(_))) {
        return;
    }
    if event.is_err() {
        failed.store(true, Ordering::Relaxed);
    }
    if matches!(
        tx.try_send(WatchMessage::Event(event)),
        Err(mpsc::TrySendError::Full(_))
    ) {
        #[cfg(test)]
        before_overflow();
        // A burst occupies one event slot. Losing a named path invalidates everything,
        // so coalescing cannot miss a relevant marker or declared watch change.
        overflow.store(true, Ordering::Relaxed);
    }
}

enum ProjectWatcher {
    Native(notify::RecommendedWatcher),
    #[cfg(any(test, feature = "test-hooks"))]
    Injected,
}

impl ProjectWatcher {
    fn new(
        events: EventSender,
        #[cfg(any(test, feature = "test-hooks"))] native: bool,
        #[cfg(all(test, feature = "app"))] fail_creation: bool,
    ) -> notify::Result<Self> {
        #[cfg(all(test, feature = "app"))]
        if fail_creation {
            return Err(notify::Error::generic("injected watcher creation failure"));
        }
        #[cfg(any(test, feature = "test-hooks"))]
        if !native {
            return Ok(Self::Injected);
        }
        notify::recommended_watcher(move |event| {
            enqueue_event(
                &events.tx,
                &events.overflow,
                &events.failed,
                event,
                #[cfg(test)]
                || {},
            );
        })
        .map(Self::Native)
    }

    fn watch(&mut self, path: &Path, mode: RecursiveMode) -> notify::Result<()> {
        match self {
            Self::Native(watcher) => watcher.watch(path, mode),
            #[cfg(any(test, feature = "test-hooks"))]
            Self::Injected => Ok(()),
        }
    }

    fn unwatch(&mut self, path: &Path) -> notify::Result<()> {
        match self {
            Self::Native(watcher) => watcher.unwatch(path),
            #[cfg(any(test, feature = "test-hooks"))]
            Self::Injected => Ok(()),
        }
    }
}

/// Exercise the native backend and callback queue, separately from injected events.
#[cfg(any(test, feature = "test-hooks"))]
pub fn assert_native_watch_delivery(directory: &Path) {
    let target = directory.join("created-after-watch");
    assert!(!target.exists());
    let (tx, rx) = mpsc::sync_channel(64);
    let events = EventSender {
        tx,
        overflow: Arc::new(AtomicBool::new(false)),
        failed: Arc::new(AtomicBool::new(false)),
    };
    let mut watcher = ProjectWatcher::new(
        events,
        true,
        #[cfg(all(test, feature = "app"))]
        false,
    )
    .expect("native watcher setup");
    watcher
        .watch(directory, RecursiveMode::NonRecursive)
        .expect("native watch registration");
    // Registration completes before this path first exists. Consume the event stream until
    // this exact path arrives; delayed directory events cannot satisfy the observation.
    // This performs one write, with no retry, sleep or event-count assertion.
    std::fs::write(&target, "notification").unwrap();
    loop {
        match rx
            .recv_timeout(Duration::from_secs(30))
            .expect("native watcher did not deliver the newly created path")
        {
            WatchMessage::PathsChanged => panic!("native watch has no project session"),
            WatchMessage::Event(event) => {
                if event
                    .expect("native notification error")
                    .paths
                    .contains(&target)
                {
                    return;
                }
            }
            WatchMessage::Settled(_) => panic!("native watch does not receive test fences"),
        }
    }
}

fn run(
    root: PathBuf,
    declarations: Vec<ProjectDeclaration>,
    slot: Arc<Slot>,
    cancelled: Arc<AtomicBool>,
    events: EventSender,
    channel: (mpsc::Receiver<WatchMessage>, Arc<SessionWatches>),
    #[cfg(any(test, feature = "test-hooks"))] native: bool,
) {
    let _publication = PublishOnExit(&slot);
    #[cfg(all(test, feature = "app"))]
    let startup_failure = slot.startup_failure;
    #[cfg(all(test, feature = "app"))]
    match startup_failure {
        Some(StartupFailure::Panic) => panic!("injected discovery worker panic before publication"),
        Some(StartupFailure::Exit) => return,
        _ => {}
    }
    let (rx, watches) = channel;
    let overflow = events.overflow.clone();
    let watch_failed = events.failed.clone();
    let mut watcher = match ProjectWatcher::new(
        events,
        #[cfg(any(test, feature = "test-hooks"))]
        native,
        #[cfg(all(test, feature = "app"))]
        matches!(startup_failure, Some(StartupFailure::WatcherCreation)),
    ) {
        Ok(watcher) => watcher,
        Err(_) => {
            slot.publish(detect(&root, &declarations, &cancelled).ok(), true);
            return;
        }
    };
    #[cfg(any(test, feature = "test-hooks"))]
    slot.injected.store(
        matches!(watcher, ProjectWatcher::Injected),
        Ordering::Relaxed,
    );
    let mut watched = BTreeSet::new();
    #[cfg(any(test, feature = "test-hooks"))]
    let mut pending_fence = None;
    #[cfg(any(test, feature = "test-hooks"))]
    let acknowledge = |tx: mpsc::SyncSender<()>| {
        let _ = tx.send(());
        #[cfg(test)]
        slot.checkpoint(fence_tests::WorkerCheckpoint::AfterFence);
    };
    loop {
        if cancelled.load(Ordering::Relaxed) {
            return;
        }
        let mut projects = match detect(&root, &declarations, &cancelled) {
            Ok(found) => found,
            Err(_) => {
                slot.publish(None, true);
                #[cfg(any(test, feature = "test-hooks"))]
                if let Some(tx) = pending_fence.take() {
                    acknowledge(tx);
                }
                return;
            }
        };
        watches.apply(&mut projects);
        let desired = watch_targets(&root, &projects);
        let added_watches = desired.difference(&watched).next().is_some();
        for (path, _) in watched.difference(&desired) {
            let _ = watcher.unwatch(path);
        }
        for (path, recursive) in desired.difference(&watched) {
            let mode = if *recursive {
                RecursiveMode::Recursive
            } else {
                RecursiveMode::NonRecursive
            };
            if watcher.watch(path, mode).is_err() {
                slot.publish(Some(projects), true);
                #[cfg(any(test, feature = "test-hooks"))]
                if let Some(tx) = pending_fence.take() {
                    acknowledge(tx);
                }
                return;
            }
        }
        watched = desired;
        // Scan again after installing watches: a marker could have changed between the first
        // scan and watch setup. The second scan closes that gap without a timer or UI-thread I/O.
        if added_watches {
            continue;
        }
        slot.publish(Some(projects.clone()), false);
        #[cfg(any(test, feature = "test-hooks"))]
        if let Some(tx) = pending_fence.take() {
            acknowledge(tx);
        }
        loop {
            if watches.dirty.swap(false, Ordering::AcqRel) {
                watches.apply(&mut projects);
                let desired = watch_targets(&root, &projects);
                for (path, _) in watched.difference(&desired) {
                    let _ = watcher.unwatch(path);
                }
                for (path, recursive) in desired.difference(&watched) {
                    if watcher
                        .watch(
                            path,
                            if *recursive {
                                RecursiveMode::Recursive
                            } else {
                                RecursiveMode::NonRecursive
                            },
                        )
                        .is_err()
                    {
                        slot.publish(Some(projects), true);
                        return;
                    }
                }
                watched = desired;
            }
            if cancelled.load(Ordering::Relaxed) {
                return;
            }
            if watch_failed.swap(false, Ordering::Relaxed) {
                slot.publish(Some(projects), true);
                return;
            }
            if overflow.swap(false, Ordering::Relaxed) {
                break;
            }
            #[cfg(test)]
            slot.checkpoint(fence_tests::WorkerCheckpoint::BeforeReceive);
            match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(WatchMessage::PathsChanged) => {}
                Ok(WatchMessage::Event(Ok(event))) => {
                    if !matches!(event.kind, notify::EventKind::Access(_))
                        && affected(&root, &declarations, &projects, &event.paths)
                    {
                        break;
                    }
                }
                Ok(WatchMessage::Event(Err(_))) => {
                    slot.publish(Some(projects), true);
                    return;
                }
                #[cfg(any(test, feature = "test-hooks"))]
                Ok(WatchMessage::Settled(tx)) => {
                    // The sender stores these flags before sending the fence. They may
                    // have changed after our pre-receive checks, so check again here.
                    if watch_failed.swap(false, Ordering::Relaxed) {
                        slot.publish(Some(projects), true);
                        acknowledge(tx);
                        return;
                    }
                    if overflow.swap(false, Ordering::Relaxed) {
                        pending_fence = Some(tx);
                        break;
                    }
                    acknowledge(tx);
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
            }
        }
    }
}

/// Carried with the tab snapshot. A worker's latest answer occupies one slot even when the tab
/// is inactive; leaving a tab does not lose its cache or grow an unbounded result queue.
///
/// Cloning shares the worker; the worker stops when the last clone is dropped or the root changes.
#[derive(Clone, Default)]
pub struct ProjectDetection {
    root: Option<PathBuf>,
    pub projects: Vec<MatchedProject>,
    worker: Option<Arc<Worker>>,
    ready: bool,
    // Unit/e2e tests inject events by default, including after root and tab changes. Native
    // notification delivery is checked separately; no process-global test switch is used.
    // A test build compiled into the application's own binary (feature unification) must still
    // watch natively, so `ensure` ignores the injection when `is_app_main()`.
    #[cfg(any(test, feature = "test-hooks"))]
    native_watch: bool,
    #[cfg(all(test, feature = "app"))]
    startup_failure: Option<StartupFailure>,
}

impl ProjectDetection {
    #[cfg(all(test, feature = "app"))]
    pub(crate) fn with_startup_failure(failure: StartupFailure) -> Self {
        Self {
            startup_failure: Some(failure),
            ..Self::default()
        }
    }

    /// Whether detection already runs (or ran) for `root`. A host asks this before it builds the
    /// declarations, so an unchanged root costs it nothing.
    pub fn is_for(&self, root: &Path) -> bool {
        self.root.as_deref() == Some(root)
    }

    /// Start discovery for `root` unless it already runs for it. A new root cancels the old worker
    /// and clears the projects; an empty declaration set is answered at once with no projects.
    pub fn ensure(&mut self, root: &Path, declarations: ProjectDeclarationSet) {
        if self.is_for(root) {
            return;
        }
        self.root = Some(root.into());
        self.ready = false;
        self.projects.clear();
        if let Some(worker) = &self.worker {
            worker.cancelled.store(true, Ordering::Relaxed);
        }
        self.worker = None;
        if declarations.is_empty() {
            self.ready = true;
            return;
        }
        let slot = Arc::new(Slot {
            #[cfg(all(test, feature = "app"))]
            startup_failure: self.startup_failure.take(),
            ..Default::default()
        });
        let cancelled = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::sync_channel(1);
        let events = EventSender {
            tx,
            overflow: Arc::new(AtomicBool::new(false)),
            failed: Arc::new(AtomicBool::new(false)),
        };
        let watches = Arc::new(SessionWatches {
            paths: Mutex::default(),
            dirty: AtomicBool::new(false),
            wake: events.tx.clone(),
        });
        #[cfg(any(test, feature = "test-hooks"))]
        let native = self.native_watch || konoma_base::process::is_app_main();
        self.worker = Some(Arc::new(Worker {
            watches: watches.clone(),
            slot: slot.clone(),
            cancelled: cancelled.clone(),
            #[cfg(any(test, feature = "test-hooks"))]
            events: (!native).then(|| events.clone()),
        }));
        let root = root.to_path_buf();
        std::thread::spawn(move || {
            run(
                root,
                declarations,
                slot,
                cancelled,
                events,
                (rx, watches),
                #[cfg(any(test, feature = "test-hooks"))]
                native,
            )
        });
    }

    /// Nonblocking: neither discovery nor watch setup holds this lock. `None` means no new answer;
    /// `Some(false)` an answer; `Some(true)` an answer whose scan or watch failed (the previous
    /// projects are kept when the scan itself failed).
    pub fn poll(&mut self) -> Option<bool> {
        let answer = self
            .worker
            .as_ref()?
            .slot
            .value
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()?;
        Some(self.apply_answer(answer))
    }

    fn apply_answer(&mut self, answer: Update) -> bool {
        if let Some(projects) = answer.projects {
            self.projects = projects;
        }
        self.ready = true;
        answer.failed
    }

    /// Whether a worker runs that has not answered yet.
    pub fn pending(&self) -> bool {
        self.worker.is_some() && !self.ready
    }

    /// Preparation only: wait for the first scan before taking the terminal. `Ok(true)` keeps
    /// the detected projects with a watch warning, like the host's nonblocking `poll`; `Err`
    /// means there is no scan answer. UI loops must use `poll` instead.
    #[cfg(feature = "app")]
    pub(crate) fn wait_for_initial(&mut self) -> io::Result<bool> {
        let Some(worker) = self.worker.as_ref() else {
            return Ok(false);
        };
        let mut guard = worker.slot.value.lock().unwrap_or_else(|e| e.into_inner());
        if self.pending() {
            guard = worker
                .slot
                .ready
                .wait_while(guard, |value| value.is_none())
                .unwrap_or_else(|e| e.into_inner());
        }
        let answer = guard.take();
        drop(guard);
        let Some(answer) = answer else {
            return Ok(false);
        };
        let scanned = answer.projects.is_some();
        let failed = self.apply_answer(answer);
        if scanned {
            Ok(failed)
        } else {
            Err(io::Error::other("project detection worker failed"))
        }
    }

    /// Replace the watch paths one project's session reported, and wake the worker to watch them.
    pub fn set_watch_paths(
        &mut self,
        plugin: &str,
        root: &Path,
        marker: &str,
        paths: Vec<PathBuf>,
    ) {
        let Some(worker) = &self.worker else {
            return;
        };
        if !self
            .projects
            .iter()
            .any(|p| p.plugin == plugin && p.root == root && p.marker == marker)
        {
            return;
        }
        let key = (plugin.to_string(), root.to_path_buf(), marker.to_string());
        let mut current = worker
            .watches
            .paths
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if current.get(&key) == Some(&paths) {
            return;
        }
        current.insert(key, paths.clone());
        drop(current);
        for project in &mut self.projects {
            if project.plugin == plugin && project.root == root && project.marker == marker {
                project.session_watch_paths = Some(paths.clone());
            }
        }
        worker.watches.dirty.store(true, Ordering::Release);
        let _ = worker.watches.wake.try_send(WatchMessage::PathsChanged);
    }

    /// Test hook: whether the worker takes injected events instead of native notifications.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn uses_injected_events(&self) -> bool {
        self.worker.as_ref().is_some_and(|worker| {
            worker.events.is_some() && worker.slot.injected.load(Ordering::Relaxed)
        })
    }

    /// Test hook: feed the real callback queue and wait for its processing, including a
    /// resulting scan. A fence also proves that an unrelated event was consumed without
    /// publishing an update.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn inject_event(&self, event: notify::Result<notify::Event>) {
        let events = self
            .worker
            .as_ref()
            .and_then(|worker| worker.events.as_ref())
            .expect("project tests must use injected events");
        enqueue_event(
            &events.tx,
            &events.overflow,
            &events.failed,
            event,
            #[cfg(test)]
            || {},
        );
        let (tx, rx) = mpsc::sync_channel(1);
        events
            .tx
            .send(WatchMessage::Settled(tx))
            .expect("project event worker");
        rx.recv_timeout(Duration::from_secs(30))
            .expect("project event processed");
    }

    /// Test hook: like [`inject_event`](Self::inject_event), after a watch-path update that is
    /// still waiting in the event slot has been consumed (a full queue would otherwise request a
    /// conservative rescan).
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn inject_event_with_pending_watch_update(&self, event: notify::Result<notify::Event>) {
        let events = self
            .worker
            .as_ref()
            .and_then(|worker| worker.events.as_ref())
            .expect("injected project events");
        // A zero-capacity acknowledgement holds the worker while a path-update wake fills
        // its one event slot. Sending the wake proves the worker consumed the hold message.
        let (hold, release) = mpsc::sync_channel(0);
        events.tx.send(WatchMessage::Settled(hold)).unwrap();
        events.tx.send(WatchMessage::PathsChanged).unwrap();
        drop(release);
        // An ignored Access event fences the pending update without occupying the event slot.
        // Named events must wait for it: a full queue correctly requests a conservative rescan.
        self.inject_event(Ok(notify::Event::new(notify::EventKind::Access(
            notify::event::AccessKind::Any,
        ))));
        self.inject_event(event);
    }

    /// Test hook: whether both handles share one worker.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn shares_worker(&self, other: &Self) -> bool {
        match (&self.worker, &other.worker) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            _ => false,
        }
    }

    /// Test hook: wait for a worker answer without sleeping or depending on its scheduling.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn wait_for_result(&self) {
        let worker = self
            .worker
            .as_ref()
            .expect("the fixture declares a static project");
        let guard = worker.slot.value.lock().unwrap_or_else(|e| e.into_inner());
        let (guard, timeout) = worker
            .slot
            .ready
            .wait_timeout_while(guard, Duration::from_secs(120), |value| value.is_none())
            .unwrap_or_else(|e| e.into_inner());
        assert!(
            !timeout.timed_out() || guard.is_some(),
            "project worker did not answer"
        );
    }
}
