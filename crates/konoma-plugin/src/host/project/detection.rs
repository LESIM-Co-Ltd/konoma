//! Project discovery and watching. All directory reads and watch setup run on a worker.

use std::collections::BTreeSet;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Condvar, Mutex, RwLock};
use std::time::{Duration, Instant};

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
/// read (a missing one included), or a cancellation, is an error. The worker classifies that error
/// itself: when reading the *root* failed with NotFound or NotADirectory it answers "no projects"
/// rather than a failure and waits for the root to return; any other error keeps the caller's
/// previous answer.
pub fn detect_directories<'a>(
    directories: impl IntoIterator<Item = &'a Path>,
    declarations: &[ProjectDeclaration],
    cancelled: &AtomicBool,
) -> io::Result<Vec<MatchedProject>> {
    scan_directories(directories, declarations, cancelled, |_| {}).map_err(|failure| failure.error)
}

/// A failed scan, with the directory whose read failed (`None` for a cancellation or a bad glob).
struct ScanFailure {
    error: io::Error,
    directory: Option<PathBuf>,
}

impl From<io::Error> for ScanFailure {
    fn from(error: io::Error) -> Self {
        Self {
            error,
            directory: None,
        }
    }
}

fn scan_directories<'a>(
    directories: impl IntoIterator<Item = &'a Path>,
    declarations: &[ProjectDeclaration],
    cancelled: &AtomicBool,
    mut after_read: impl FnMut(&Path),
) -> Result<Vec<MatchedProject>, ScanFailure> {
    let mut out = Vec::new();
    for directory in directories {
        if cancelled.load(Ordering::Relaxed) {
            return Err(io::Error::from(io::ErrorKind::Interrupted).into());
        }
        let mut entries = std::fs::read_dir(directory)
            .and_then(|entries| entries.collect::<io::Result<Vec<_>>>())
            .map_err(|error| ScanFailure {
                error,
                directory: Some(directory.to_path_buf()),
            })?;
        after_read(directory);
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
    // No desired watch may be nested in another: the backend does not keep them independent. On
    // inotify, unwatching a recursive path also removes the watches nested below it, and adding a
    // non-recursive watch below a recursive one overwrites that directory's recursive flag
    // (notify 8.2.0 inotify.rs, `remove_watch` ~486-499 and `add_single_watch` ~463). A recursive watch
    // already covers the directory itself and everything below it, so drop every entry it covers.
    let recursive: Vec<_> = out
        .iter()
        .filter(|(_, r)| *r)
        .map(|(p, _)| p.clone())
        .collect();
    out.retain(|(path, is_recursive)| {
        !recursive.iter().any(|covering| {
            if *is_recursive {
                path != covering && path.starts_with(covering)
            } else {
                path.starts_with(covering)
            }
        })
    });
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
    /// Runs once on the worker right after a scan failed, before the failure is classified.
    #[cfg(test)]
    after_failed_scan: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    /// Runs once on the worker right after the scan read the given directory successfully.
    #[cfg(test)]
    after_directory: Mutex<Option<TestHook<PathBuf>>>,
    /// Runs once on the worker after a successful scan, right before it installs new watches.
    #[cfg(test)]
    before_watches: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    /// Runs on the worker right before every single watch is added. It may change the file system
    /// and may return an error that stands in for the backend's answer to that watch.
    #[cfg(test)]
    before_each_watch: Mutex<Option<WatchHook>>,
    /// While set, every scan ends with the retry of an incomplete set due, as if the scan had taken
    /// longer than the backoff.
    #[cfg(test)]
    scan_makes_retry_due: AtomicBool,
    /// While set, every turn of the wait for a missing root that follows a rebuild starts with the
    /// retry of an incomplete set due, as if the existence check and the ancestor search had
    /// outlasted the backoff.
    #[cfg(test)]
    wait_makes_retry_due: AtomicBool,
    #[cfg(all(test, feature = "app"))]
    startup_failure: Option<StartupFailure>,
}

/// A test hook that receives the directory the scan just read.
#[cfg(test)]
type TestHook<T> = Box<dyn FnMut(&T) + Send>;

/// A test hook that runs before a watch is added and may replace the backend's answer.
#[cfg(test)]
type WatchHook = Box<dyn FnMut(&Path) -> Option<notify::Error> + Send>;

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

impl Slot {
    /// The worker's hook point before each watch it adds; a test may fail the watch from here.
    fn before_add(&self, path: &Path) -> Option<notify::Error> {
        #[cfg(test)]
        {
            self.run_before_watches();
            let mut hook = self
                .before_each_watch
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            hook.as_mut().and_then(|run| run(path))
        }
        #[cfg(not(test))]
        {
            let _ = path;
            None
        }
    }
}

#[cfg(test)]
impl Slot {
    fn run_after_directory(&self, directory: &Path) {
        let mut hook = self
            .after_directory
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(run) = hook.as_mut() {
            run(&directory.to_path_buf());
        }
    }

    fn run_before_watches(&self) {
        let hook = self
            .before_watches
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        if let Some(hook) = hook {
            hook();
        }
    }

    fn run_after_failed_scan(&self) {
        let hook = self
            .after_failed_scan
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        if let Some(hook) = hook {
            hook();
        }
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
    /// Test hook: the backoff of an incomplete set is over now. Acknowledged after the retry (and
    /// the rescan that follows it) was done, or at once when no retry is owed.
    #[cfg(test)]
    RetryDue(mpsc::SyncSender<()>),
}

/// What the watcher callback and the worker share.
///
/// The worker's own queue has capacity 1 and coalesces: a burst of events (saving one file makes
/// several) drops all but the first. Whether a dropped event was a watch-loss signal must therefore
/// be decided where nothing is dropped, in the callback ([`enqueue_event`]): it classifies every
/// event against `installed` and stores `lost` before it tries to queue the event, so a full queue
/// cannot lose the signal. A full queue by itself (`overflow`) only asks the worker for a rescan.
#[derive(Clone)]
struct EventSender {
    tx: mpsc::SyncSender<WatchMessage>,
    /// The queue was full or a path vanished: scan again (no rebuild).
    overflow: Arc<AtomicBool>,
    failed: Arc<AtomicBool>,
    /// A watch-loss signal arrived (see [`signals_watch_loss`]): the next sync rebuilds.
    lost: Arc<AtomicBool>,
    /// The paths the watchers hold, written by [`WatchSet::rebuild`]. While a rebuild replaces a
    /// watcher, the callbacks of both watchers read this list, so it holds the old and the new
    /// paths together until the old watcher is dropped.
    installed: Arc<RwLock<Vec<PathBuf>>>,
}

impl EventSender {
    fn new(tx: mpsc::SyncSender<WatchMessage>) -> Self {
        Self {
            tx,
            overflow: Arc::new(AtomicBool::new(false)),
            failed: Arc::new(AtomicBool::new(false)),
            lost: Arc::new(AtomicBool::new(false)),
            installed: Arc::new(RwLock::new(Vec::new())),
        }
    }

    fn publish_installed(&self, paths: Vec<PathBuf>) {
        *self.installed.write().unwrap_or_else(|e| e.into_inner()) = paths;
    }
}

/// Whether the event says a watch was lost: the watched object (or a directory above it) was
/// removed or renamed (`Remove(_)` or `Modify(Name(_))` naming a path that some installed path
/// starts with), or the backend's queue overflowed (`Flag::Rescan`). An event deeper inside a
/// recursive watch does not count: notify itself removes and re-adds the nested watches on
/// DELETE, MOVED_FROM, MOVED_TO and CREATE (inotify.rs ~219-262).
fn signals_watch_loss(installed: &[PathBuf], event: &notify::Event) -> bool {
    if event.flag() == Some(notify::event::Flag::Rescan) {
        return true;
    }
    matches!(
        event.kind,
        notify::EventKind::Remove(_)
            | notify::EventKind::Modify(notify::event::ModifyKind::Name(_))
    ) && event
        .paths
        .iter()
        .any(|named| installed.iter().any(|path| path.starts_with(named)))
}

/// The watcher callback: classify, then queue. A watch-loss signal is stored in `lost` BEFORE the
/// `try_send`; a full queue only sets `overflow` (rescan), because the dropped event was already
/// classified here.
fn enqueue_event(
    events: &EventSender,
    event: notify::Result<notify::Event>,
    #[cfg(test)] before_overflow: impl FnOnce(),
) {
    if matches!(&event, Ok(event) if matches!(&event.kind, notify::EventKind::Access(_))) {
        return;
    }
    match &event {
        // A watch error about a path that no longer exists is the race of a deletion, not a
        // failure: ask for a rescan (the flag survives a full queue, like any lost path).
        Err(error) if watch_error_says_path_gone(error) => {
            events.overflow.store(true, Ordering::Relaxed)
        }
        Err(_) => events.failed.store(true, Ordering::Relaxed),
        Ok(event) => {
            let installed = events.installed.read().unwrap_or_else(|e| e.into_inner());
            if signals_watch_loss(&installed, event) {
                events.lost.store(true, Ordering::Relaxed);
            }
        }
    }
    if matches!(
        events.tx.try_send(WatchMessage::Event(event)),
        Err(mpsc::TrySendError::Full(_))
    ) {
        #[cfg(test)]
        before_overflow();
        // A burst occupies one event slot. Losing a named path invalidates everything, so
        // coalescing cannot miss a relevant marker or declared watch change: rescan.
        events.overflow.store(true, Ordering::Relaxed);
    }
}

/// Every watch an injected-event worker added, as (watcher instance, path, recursive). The
/// instance number tells a test which fresh watcher an add belongs to, so a rebuild is visible.
/// Shared by all tests: filter by a unique directory.
#[cfg(test)]
static INJECTED_WATCH_LOG: std::sync::Mutex<Vec<(usize, PathBuf, bool)>> =
    std::sync::Mutex::new(Vec::new());

/// The same adds, with the drop of each watcher, in the order they happened: `(instance, Some(path))`
/// for an add, `(instance, None)` for a drop.
#[cfg(test)]
static INJECTED_TRACE: std::sync::Mutex<Vec<(usize, Option<PathBuf>)>> =
    std::sync::Mutex::new(Vec::new());

/// Test hook: runs on the worker when the injected watcher of the given instance is dropped.
#[cfg(test)]
type DropHook = (usize, Box<dyn FnOnce() + Send>);
#[cfg(test)]
static INJECTED_DROP_HOOKS: std::sync::Mutex<Vec<DropHook>> = std::sync::Mutex::new(Vec::new());

#[cfg(test)]
static INJECTED_INSTANCES: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

enum ProjectWatcher {
    Native(notify::RecommendedWatcher),
    #[cfg(any(test, feature = "test-hooks"))]
    Injected {
        #[cfg(test)]
        instance: usize,
    },
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
            return Ok(Self::Injected {
                #[cfg(test)]
                instance: INJECTED_INSTANCES.fetch_add(1, Ordering::Relaxed),
            });
        }
        notify::recommended_watcher(move |event| {
            enqueue_event(
                &events,
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
            Self::Injected { .. } => {
                // Like a native backend, watching a path that does not exist is an error.
                if !path.exists() {
                    return Err(notify::Error::path_not_found().add_path(path.to_path_buf()));
                }
                #[cfg(test)]
                if let Self::Injected { instance } = self {
                    let mut trace = INJECTED_TRACE.lock().unwrap();
                    INJECTED_WATCH_LOG.lock().unwrap().push((
                        *instance,
                        path.to_path_buf(),
                        mode == RecursiveMode::Recursive,
                    ));
                    trace.push((*instance, Some(path.to_path_buf())));
                }
                Ok(())
            }
        }
    }
}

#[cfg(test)]
impl Drop for ProjectWatcher {
    fn drop(&mut self) {
        if let Self::Injected { instance } = self {
            INJECTED_TRACE.lock().unwrap().push((*instance, None));
            let hook = {
                let mut hooks = INJECTED_DROP_HOOKS.lock().unwrap();
                let position = hooks.iter().position(|(owner, _)| owner == instance);
                position.map(|position| hooks.remove(position).1)
            };
            if let Some(hook) = hook {
                hook();
            }
        }
    }
}

/// Exercise the native backend and callback queue, separately from injected events.
#[cfg(any(test, feature = "test-hooks"))]
pub fn assert_native_watch_delivery(directory: &Path) {
    let target = directory.join("created-after-watch");
    assert!(!target.exists());
    let (tx, rx) = mpsc::sync_channel(64);
    let events = EventSender::new(tx);
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
            #[cfg(test)]
            WatchMessage::RetryDue(_) => panic!("native watch does not receive retry hooks"),
        }
    }
}

/// Whether an error from reading a root says the root is gone: it does not exist, or it (or one
/// of its parents) is not a directory. Any other failure (permissions, I/O errors) is real.
fn is_missing_kind(kind: io::ErrorKind) -> bool {
    matches!(kind, io::ErrorKind::NotFound | io::ErrorKind::NotADirectory)
}

/// Whether the scan failed because `root` is gone: reading the root, or any directory above it
/// (when an ancestor is gone, the root is gone with it), failed with a missing kind. The scan's
/// own error decides, not a second look at the file system: the root may have been created
/// again since.
fn scan_says_root_missing(failure: &ScanFailure, root: &Path) -> bool {
    failure
        .directory
        .as_deref()
        .is_some_and(|directory| root.starts_with(directory))
        && is_missing_kind(failure.error.kind())
}

/// Whether the error is of a "not found" kind, whatever path it names (or none).
fn watch_error_kind_is_missing(error: &notify::Error) -> bool {
    match &error.kind {
        notify::ErrorKind::PathNotFound => true,
        notify::ErrorKind::Io(e) => is_missing_kind(e.kind()),
        _ => false,
    }
}

/// Whether an asynchronous watch error says a path vanished: a "not found" kind AND the paths it
/// names all gone now. An error that names no path cannot be checked and stays a failure. A
/// "not found" error whose paths still exist as directories (a backend rounding its errors) is
/// not a vanished path. "Gone" is judged by [`directory_missing`], the way the watch itself sees
/// the path: symlinks are followed, so a dangling symlink and a regular file where a directory
/// was both count as gone. That only costs a rescan, where calling them failures would show the
/// watch warning for an ordinary dangling link. (An error from adding a watch is judged by its
/// kind alone: see [`WatchSet`].)
fn watch_error_says_path_gone(error: &notify::Error) -> bool {
    watch_error_kind_is_missing(error)
        && !error.paths.is_empty()
        && error.paths.iter().all(|path| directory_missing(path))
}

/// Which directory a watch is on: device and inode, from `std::fs::metadata` (symlinks followed,
/// like the watch itself). A directory deleted and created again is another identity.
type Identity = (u64, u64);

fn identity_of(path: &Path) -> Option<Identity> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::metadata(path).ok()?;
    meta.is_dir().then(|| (meta.dev(), meta.ino()))
}

#[cfg(not(test))]
const RETRY_FIRST: Duration = Duration::from_millis(100);
#[cfg(not(test))]
const RETRY_CAP: Duration = Duration::from_millis(3200);
/// Tests retry only when a test makes the retry due (`WatchMessage::RetryDue`), never by time.
#[cfg(test)]
const RETRY_FIRST: Duration = Duration::from_secs(3600);
#[cfg(test)]
const RETRY_CAP: Duration = Duration::from_secs(3600);

/// The longest the worker blocks on its channel: it bounds how late a cancellation is noticed.
const RECEIVE_SLICE: Duration = Duration::from_millis(100);

fn doubled(delay: Duration, cap: Duration) -> Duration {
    (delay * 2).min(cap)
}

/// What [`WatchSet::sync`] did.
enum Synced {
    /// The installed watches stay as they were.
    Unchanged,
    /// A fresh watcher replaced the old one: the caller scans again and publishes. Changes made
    /// while no watch covered a place are found only by scanning (inotify and FSEvents with
    /// `SinceNow` do not replay them), so every rebuild is followed by a rescan.
    Rebuilt,
}

/// The test hook point before each watch is added (see [`Slot::before_add`]).
type BeforeAdd<'a> = &'a mut dyn FnMut(&Path) -> Option<notify::Error>;

/// The watches of one worker. It never edits a watcher: it replaces it.
///
/// Why a rebuild and no incremental bookkeeping: the backend changes its own watch state, so a
/// worker-side mirror of it cannot be kept true. inotify drops the watch of a deleted directory
/// (notify 8.2.0 inotify.rs ~303, ~315, ~379); unwatching a recursive path also removes the watches
/// nested below it (~486-499); a failed recursive add keeps the watches it already made and a
/// failed unwatch stops part way (~492); an add error may name no path (~461). The standard plugins
/// watch the whole work tree recursively, so on Linux these races happen during ordinary builds.
/// Instead, when the wanted set changes (or a watch may be lost) the worker creates a fresh
/// watcher, adds every wanted path to it and only then drops the old one, so the old watches keep
/// reporting during the walk. What the fresh watcher accepted IS the installed set, with the
/// identity (device, inode) of each path to notice a directory that was replaced.
///
/// What requests a rebuild (`rebuild_requested`), besides a changed wanted set:
/// - a watch-loss signal. inotify loses a watch when the watched object is deleted or moved
///   (IN_DELETE_SELF / IN_MOVE_SELF / IN_IGNORED, or the parent's DELETE / MOVED_FROM, which notify
///   turns into removing that watch: inotify.rs ~232 and ~379) and when its queue overflows
///   (Q_OVERFLOW, which notify reports as `Flag::Rescan`, ~195). notify reports these as
///   `Remove(_)` and `Modify(Name(_))` events naming the object, and as the rescan flag: see
///   [`signals_watch_loss`]. The callback classifies, because the worker's queue coalesces (see
///   [`EventSender`]); the worker's own queue overflow only rescans. A directory renamed away and
///   back keeps its inode, so the identity check alone cannot see that its watch is gone. The check
///   stays as well, after each successful scan.
/// - a retry that is due (below).
///
/// Every rebuild is followed by a rescan (the caller breaks to its outer loop, which scans and
/// publishes), including one in the idle loop: a place that no watch covered meanwhile is only
/// found by scanning.
///
/// A "not found" add error (named or not) is never a failure: the path is left out, the set is
/// `incomplete`, and a rebuild is retried only by time: once `retry_at` has passed (100 ms
/// doubling to 3.2 s, reset after a complete install). Events, `PathsChanged` and fences never
/// make a retry due. Any other error (permissions, I/O, watcher creation) fails.
///
/// A timed retry is taken only by the idle loop and by a wait-for-root turn that follows a receive,
/// never by the outer (scan) loop nor by the wait's turn right after a rebuild, and the backoff is
/// measured from when the worker becomes idle ([`WatchSet::enter_idle`]), not from the end of the
/// rebuild. Why: a scan slower than the backoff would otherwise find the retry due again right after
/// the rebuild, rebuild again and never reach the publish, so a quiet file system would turn the
/// retries into continuous work with no answer. Now a retry cycle (rebuild, scan, publish) is always
/// followed by at least the backoff of idle time, whatever the scan costs. The wait for a missing
/// root follows the same rule: its existence check and ancestor search may also outlast the backoff,
/// so it enters idle ([`WatchSet::enter_idle`]) right before each blocking receive.
///
/// It cannot spin: a rebuild is requested only by a file system event, a changed wanted set (itself
/// a file system change), or a due retry, a rebuild causes no event of its own, and a due retry is
/// bounded by the backoff.
struct WatchSet {
    events: EventSender,
    #[cfg(any(test, feature = "test-hooks"))]
    native: bool,
    #[cfg(all(test, feature = "app"))]
    fail_creation: bool,
    watcher: ProjectWatcher,
    /// The watcher has no watch yet, so the first rebuild uses it instead of creating another.
    fresh: bool,
    /// What the last rebuild was asked to install.
    wanted: BTreeSet<(PathBuf, bool)>,
    /// What that watcher accepted.
    installed: Vec<(PathBuf, Identity)>,
    /// Some wanted path was left out (not found); retried when `retry_at` has passed.
    incomplete: bool,
    /// A watch-loss signal arrived: the next sync rebuilds.
    rebuild_requested: bool,
    retry_at: Instant,
    delay: Duration,
    /// The backoff the last rebuild used while it left the set incomplete; [`WatchSet::enter_idle`]
    /// measures it again from the idle start (once).
    idle_backoff: Option<Duration>,
}

impl WatchSet {
    fn new(
        events: EventSender,
        #[cfg(any(test, feature = "test-hooks"))] native: bool,
        #[cfg(all(test, feature = "app"))] fail_creation: bool,
    ) -> notify::Result<Self> {
        let watcher = ProjectWatcher::new(
            events.clone(),
            #[cfg(any(test, feature = "test-hooks"))]
            native,
            #[cfg(all(test, feature = "app"))]
            fail_creation,
        )?;
        Ok(Self {
            events,
            #[cfg(any(test, feature = "test-hooks"))]
            native,
            #[cfg(all(test, feature = "app"))]
            fail_creation,
            watcher,
            fresh: true,
            wanted: BTreeSet::new(),
            installed: Vec::new(),
            incomplete: false,
            rebuild_requested: false,
            retry_at: Instant::now(),
            delay: RETRY_FIRST,
            idle_backoff: None,
        })
    }

    #[cfg(any(test, feature = "test-hooks"))]
    fn is_injected(&self) -> bool {
        matches!(self.watcher, ProjectWatcher::Injected { .. })
    }

    /// Whether an incomplete set is due for a retry: only the clock decides.
    fn retry_due(&self) -> bool {
        self.incomplete && Instant::now() >= self.retry_at
    }

    /// Whether a rebuild is due although the wanted set did not change. A due timed retry counts
    /// only when `take_retry` (the idle loop and the wait for the root).
    fn rebuild_due(&mut self, take_retry: bool) -> bool {
        self.absorb_loss();
        self.rebuild_requested || (take_retry && self.retry_due())
    }

    /// The worker published an answer and becomes idle: an incomplete set is not retried before its
    /// backoff has passed counted from now (see the type's doc for why).
    fn enter_idle(&mut self) {
        if let Some(backoff) = self.idle_backoff.take() {
            if self.incomplete {
                self.retry_at = self.retry_at.max(Instant::now() + backoff);
            }
        }
    }

    /// How long the worker may block on its channel: no longer than the next due retry.
    fn receive_timeout(&self) -> Duration {
        if self.incomplete {
            RECEIVE_SLICE.min(self.retry_at.saturating_duration_since(Instant::now()))
        } else {
            RECEIVE_SLICE
        }
    }

    /// Take the watch-loss signal the callback stored: the next sync rebuilds. Returns whether
    /// there was one.
    fn absorb_loss(&mut self) -> bool {
        let lost = self.events.lost.swap(false, Ordering::Relaxed);
        if lost {
            self.rebuild_requested = true;
        }
        lost
    }

    /// Whether some installed path is gone or replaced since it was accepted.
    fn lost_a_watch(&self) -> bool {
        self.installed
            .iter()
            .any(|(path, identity)| identity_of(path) != Some(*identity))
    }

    /// Make the watcher hold exactly `desired`, rebuilding it when the wanted set changed, when a
    /// watch was lost (`check_lost`: the caller just scanned successfully, or a watch-loss signal
    /// arrived), or when a retry is due and `take_retry`. Only a non-"not found" error is returned.
    fn sync(
        &mut self,
        desired: &BTreeSet<(PathBuf, bool)>,
        check_lost: bool,
        take_retry: bool,
        before_add: BeforeAdd,
    ) -> notify::Result<Synced> {
        self.absorb_loss();
        if check_lost && self.lost_a_watch() {
            self.rebuild_requested = true;
        }
        if *desired == self.wanted && !self.rebuild_due(take_retry) {
            return Ok(Synced::Unchanged);
        }
        self.rebuild(desired, before_add)?;
        Ok(Synced::Rebuilt)
    }

    /// Fill a fresh watcher, then replace the old one with it (dropping the old one removes all of
    /// its watches, whatever state it was in).
    fn rebuild(
        &mut self,
        desired: &BTreeSet<(PathBuf, bool)>,
        before_add: BeforeAdd,
    ) -> notify::Result<()> {
        let mut replacement = if self.fresh {
            None
        } else {
            Some(ProjectWatcher::new(
                self.events.clone(),
                #[cfg(any(test, feature = "test-hooks"))]
                self.native,
                #[cfg(all(test, feature = "app"))]
                self.fail_creation,
            )?)
        };
        // The callbacks classify against the shared list while the fresh watcher is filled: it
        // holds the old installed paths and every wanted one (a signal about a path the fresh
        // watcher is about to hold must not be missed), and the union of the old and the new
        // installed paths until the old watcher is dropped.
        let old_paths: Vec<PathBuf> = self.installed.iter().map(|(p, _)| p.clone()).collect();
        let mut union = old_paths.clone();
        union.extend(desired.iter().map(|(path, _)| path.clone()));
        self.events.publish_installed(union);
        let watcher = replacement.as_mut().unwrap_or(&mut self.watcher);
        let mut installed = Vec::new();
        let mut missing = false;
        for (path, recursive) in desired {
            let mode = if *recursive {
                RecursiveMode::Recursive
            } else {
                RecursiveMode::NonRecursive
            };
            let before = identity_of(path);
            let added = match before_add(path) {
                Some(error) => Err(error),
                None => watcher.watch(path, mode),
            };
            match added {
                // The directory must be the same one before and after the add: otherwise it was
                // replaced meanwhile and the watch (if any) belongs to the old one.
                Ok(()) => match (before, identity_of(path)) {
                    (Some(a), Some(b)) if a == b => installed.push((path.clone(), a)),
                    _ => missing = true,
                },
                Err(error) if watch_error_kind_is_missing(&error) => missing = true,
                Err(error) => {
                    self.events.publish_installed(old_paths);
                    return Err(error);
                }
            }
        }
        let new_paths: Vec<PathBuf> = installed.iter().map(|(p, _)| p.clone()).collect();
        let mut union = old_paths;
        union.extend(new_paths.iter().cloned());
        self.events.publish_installed(union);
        if let Some(replacement) = replacement {
            // Dropping the old watcher is the end of the window: its callback may still run until
            // here, and it names the old paths.
            self.watcher = replacement;
        }
        self.events.publish_installed(new_paths);
        self.fresh = false;
        self.wanted = desired.clone();
        self.installed = installed;
        self.rebuild_requested = false;
        self.incomplete = missing;
        if missing {
            self.retry_at = Instant::now() + self.delay;
            self.idle_backoff = Some(self.delay);
            self.delay = doubled(self.delay, RETRY_CAP);
        } else {
            self.delay = RETRY_FIRST;
            self.idle_backoff = None;
        }
        Ok(())
    }
}

/// Whether `path` is gone right now: it does not exist, or exists but is not a directory.
fn directory_missing(path: &Path) -> bool {
    match std::fs::metadata(path) {
        Ok(meta) => !meta.is_dir(),
        Err(e) => is_missing_kind(e.kind()),
    }
}

/// The nearest existing directory above `root`.
fn nearest_existing_ancestor(root: &Path) -> Option<PathBuf> {
    root.ancestors()
        .skip(1)
        .find(|p| p.is_dir())
        .map(Path::to_path_buf)
}

/// What ended the wait for a missing root.
enum RootWait {
    /// The root exists again: the caller scans and installs its watches as usual.
    Back,
    /// The worker must end (cancellation, a lost channel, or a watch failure published here).
    End,
}

/// Wait for a missing `root` to exist again, watching its nearest existing ancestor
/// non-recursively; when that ancestor vanishes too, the next existing one is chosen. The watch goes
/// through the same [`WatchSet::sync`] as the main loop.
#[allow(clippy::too_many_arguments)]
fn wait_for_root(
    root: &Path,
    slot: &Slot,
    cancelled: &AtomicBool,
    watches: &SessionWatches,
    rx: &mpsc::Receiver<WatchMessage>,
    set: &mut WatchSet,
    overflow: &AtomicBool,
    watch_failed: &AtomicBool,
    #[cfg(any(test, feature = "test-hooks"))] pending_fence: &mut Option<mpsc::SyncSender<()>>,
    #[cfg(any(test, feature = "test-hooks"))] acknowledge: &dyn Fn(mpsc::SyncSender<()>),
) -> RootWait {
    // Publish a failure (like the main loop) and acknowledge a fence that is still owed.
    macro_rules! fail {
        () => {{
            slot.publish(None, true);
            #[cfg(any(test, feature = "test-hooks"))]
            if let Some(tx) = pending_fence.take() {
                acknowledge(tx);
            }
            return RootWait::End;
        }};
    }
    // Whether the turn follows a rebuild: only a turn that follows a receive (the wait was idle) may
    // take a due timed retry, see the doc of `WatchSet`.
    let mut just_rebuilt = true;
    loop {
        // Session watch paths are kept in `watches` and applied by the scan after `Back`; the
        // dirty flag is consumed here so the host's idle check does not wait for a root that
        // may never return.
        watches.dirty.store(false, Ordering::Release);
        if cancelled.load(Ordering::Relaxed) {
            return RootWait::End;
        }
        if !directory_missing(root) {
            return RootWait::Back;
        }
        #[cfg(test)]
        if just_rebuilt && slot.wait_makes_retry_due.load(Ordering::Relaxed) {
            set.retry_at = Instant::now();
        }
        let Some(ancestor) = nearest_existing_ancestor(root) else {
            fail!();
        };
        let desired = BTreeSet::from([(ancestor, false)]);
        // A rebuild may find the ancestor gone (it is then chosen again) or the root back.
        let take_retry = !std::mem::take(&mut just_rebuilt);
        match set.sync(&desired, true, take_retry, &mut |path| {
            slot.before_add(path)
        }) {
            Err(_) => fail!(),
            Ok(Synced::Rebuilt) => {
                just_rebuilt = true;
                continue;
            }
            Ok(Synced::Unchanged) => {}
        }
        if watch_failed.swap(false, Ordering::Relaxed) {
            fail!();
        }
        // A full queue asks for a rescan; waiting for the root rescans nothing but the root's own
        // existence, which the loop checks anyway. A watch-loss signal was classified by the
        // callback and is taken by the next sync.
        overflow.store(false, Ordering::Relaxed);
        #[cfg(any(test, feature = "test-hooks"))]
        if let Some(tx) = pending_fence.take() {
            acknowledge(tx);
        }
        // About to block: the backoff of an incomplete set counts from here (once per rebuild).
        set.enter_idle();
        match rx.recv_timeout(set.receive_timeout()) {
            // Any change at the watched ancestor may be the root's return: the loop checks. An
            // event that says the watch was lost was classified by the callback (`lost`).
            Ok(WatchMessage::Event(Ok(_))) => {}
            Ok(WatchMessage::PathsChanged) => {}
            Ok(WatchMessage::Event(Err(_))) => {
                // A real error was flagged when it arrived; a vanished path only asks to look again.
                if watch_failed.swap(false, Ordering::Relaxed) {
                    fail!();
                }
            }
            #[cfg(any(test, feature = "test-hooks"))]
            Ok(WatchMessage::Settled(tx)) => {
                if watch_failed.swap(false, Ordering::Relaxed) {
                    slot.publish(None, true);
                    acknowledge(tx);
                    return RootWait::End;
                }
                // The fence is owed after everything this wait still has to do for the events
                // before it: a rebuilt watch, or the scan a returned root needs. The loop
                // acknowledges it just before it blocks again.
                // A pending watch-loss signal asks for a rebuild, like one the loop saw itself. The
                // fence starts no retry.
                set.absorb_loss();
                overflow.store(false, Ordering::Relaxed);
                *pending_fence = Some(tx);
                continue;
            }
            #[cfg(test)]
            Ok(WatchMessage::RetryDue(tx)) => {
                set.retry_at = Instant::now();
                *pending_fence = Some(tx);
                continue;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return RootWait::End,
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
    let mut set = match WatchSet::new(
        events,
        #[cfg(any(test, feature = "test-hooks"))]
        native,
        #[cfg(all(test, feature = "app"))]
        matches!(startup_failure, Some(StartupFailure::WatcherCreation)),
    ) {
        Ok(set) => set,
        Err(_) => {
            slot.publish(detect(&root, &declarations, &cancelled).ok(), true);
            return;
        }
    };
    #[cfg(any(test, feature = "test-hooks"))]
    slot.injected.store(set.is_injected(), Ordering::Relaxed);
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
        let scan = scan_directories(root.ancestors(), &declarations, &cancelled, |directory| {
            #[cfg(test)]
            slot.run_after_directory(directory);
            #[cfg(not(test))]
            let _ = directory;
        });
        #[cfg(test)]
        if scan.is_err() {
            slot.run_after_failed_scan();
        }
        #[cfg(test)]
        if slot.scan_makes_retry_due.load(Ordering::Relaxed) {
            set.retry_at = Instant::now();
        }
        let mut projects = match scan {
            Ok(found) => found,
            Err(failure) if scan_says_root_missing(&failure, &root) => {
                // A root that is gone is not a failure: there are no projects. Wait for it to
                // come back instead of ending, so a later re-creation is detected again.
                slot.publish(Some(Vec::new()), false);
                // A fence owed for this scan is acknowledged by the wait, once its watch on the
                // nearest existing ancestor is installed.
                let back = wait_for_root(
                    &root,
                    &slot,
                    &cancelled,
                    &watches,
                    &rx,
                    &mut set,
                    &overflow,
                    &watch_failed,
                    #[cfg(any(test, feature = "test-hooks"))]
                    &mut pending_fence,
                    #[cfg(any(test, feature = "test-hooks"))]
                    &acknowledge,
                );
                if matches!(back, RootWait::End) {
                    return;
                }
                continue;
            }
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
        // The scan succeeded, so a watch that was lost since it was accepted (a directory
        // deleted and created again) is noticed here, not through events.
        // A due timed retry is not taken here (`take_retry` false): see the type's doc.
        match set.sync(&desired, true, false, &mut |path| slot.before_add(path)) {
            Ok(Synced::Unchanged) => {}
            // Scan again after the watches were rebuilt: a marker could have changed between the
            // scan and the watches. The second scan closes that gap without a timer or UI-thread
            // I/O. A watch left out as "not found" is retried by the idle loop when its backoff is
            // over, not here.
            Ok(Synced::Rebuilt) => continue,
            Err(_) => {
                slot.publish(Some(projects), true);
                #[cfg(any(test, feature = "test-hooks"))]
                if let Some(tx) = pending_fence.take() {
                    acknowledge(tx);
                }
                return;
            }
        }
        slot.publish(Some(projects.clone()), false);
        #[cfg(any(test, feature = "test-hooks"))]
        if let Some(tx) = pending_fence.take() {
            acknowledge(tx);
        }
        set.enter_idle();
        loop {
            let dirty = watches.dirty.swap(false, Ordering::AcqRel);
            if dirty {
                watches.apply(&mut projects);
            }
            if dirty || set.rebuild_due(true) {
                let desired = watch_targets(&root, &projects);
                match set.sync(&desired, false, true, &mut |path| slot.before_add(path)) {
                    Ok(Synced::Unchanged) => {}
                    // Scan again after a rebuild, here too: a change made while no watch covered
                    // its place is found only by scanning.
                    Ok(Synced::Rebuilt) => break,
                    Err(_) => {
                        slot.publish(Some(projects), true);
                        return;
                    }
                }
            }
            if cancelled.load(Ordering::Relaxed) {
                return;
            }
            if watch_failed.swap(false, Ordering::Relaxed) {
                slot.publish(Some(projects), true);
                return;
            }
            if overflow.swap(false, Ordering::Relaxed) {
                // The queue overflowed: events were dropped, so scan again. No rebuild: a dropped
                // watch-loss signal was classified by the callback and is in `lost` (taken by
                // the sync above or the outer loop's).
                break;
            }
            #[cfg(test)]
            slot.checkpoint(fence_tests::WorkerCheckpoint::BeforeReceive);
            match rx.recv_timeout(set.receive_timeout()) {
                Ok(WatchMessage::PathsChanged) => {}
                Ok(WatchMessage::Event(Ok(event))) => {
                    if !matches!(event.kind, notify::EventKind::Access(_))
                        && affected(&root, &declarations, &projects, &event.paths)
                    {
                        break;
                    }
                }
                Ok(WatchMessage::Event(Err(_))) => {
                    // A real error was flagged when it arrived; a vanished path asks for a rescan.
                    if watch_failed.swap(false, Ordering::Relaxed) {
                        slot.publish(Some(projects), true);
                        return;
                    }
                    overflow.store(false, Ordering::Relaxed);
                    break;
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
                    // The fence starts no retry and publishes nothing by itself: it is owed only
                    // for a pending overflow (a rescan) or watch loss (a rebuild and a rescan),
                    // which acknowledges it.
                    let lost = set.absorb_loss();
                    if overflow.swap(false, Ordering::Relaxed) || lost {
                        pending_fence = Some(tx);
                        break;
                    }
                    acknowledge(tx);
                }
                #[cfg(test)]
                Ok(WatchMessage::RetryDue(tx)) => {
                    set.retry_at = Instant::now();
                    if set.retry_due() {
                        // Owed after the retry and the rescan it causes.
                        pending_fence = Some(tx);
                    } else {
                        acknowledge(tx);
                    }
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
        let events = EventSender::new(tx);
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
    /// projects are kept when the scan itself failed). A root that does not exist (or is not a
    /// directory) is an answer of no projects, not a failure: the worker keeps waiting for the
    /// root to be created again.
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

    /// Whether a resident detection/watch worker exists, including after its first answer.
    pub fn watching(&self) -> bool {
        self.worker.is_some()
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
        // Like the worker: an error about a vanished path is a rescan request, not a failure.
        let failed = event
            .as_ref()
            .is_err_and(|error| !watch_error_says_path_gone(error));
        let events = self
            .worker
            .as_ref()
            .and_then(|worker| worker.events.as_ref())
            .expect("project tests must use injected events");
        // The queue holds one message. A path-change wake still waiting there would make this
        // event overflow into a full rescan, so first wait until the worker has consumed it.
        // A worker that already exited (a watch failure) has nothing left to fence.
        {
            let (tx, rx) = mpsc::sync_channel(1);
            if events.tx.send(WatchMessage::Settled(tx)).is_ok() {
                match rx.recv_timeout(Duration::from_secs(30)) {
                    Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => {}
                    Err(error) => panic!("project event worker is stuck before the event: {error}"),
                }
            }
        }
        enqueue_event(
            events,
            event,
            #[cfg(test)]
            || {},
        );
        let (tx, rx) = mpsc::sync_channel(1);
        let sent = events.tx.send(WatchMessage::Settled(tx));
        if failed {
            // A watch failure publishes and exits, possibly before receiving the fence.
            // Channel disconnection is then the exit fence; publication must already exist.
            if sent.is_ok() {
                match rx.recv_timeout(Duration::from_secs(30)) {
                    Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => {}
                    Err(error) => panic!("project failure processed: {error}"),
                }
            }
            self.wait_for_result();
        } else {
            sent.expect("project event worker");
            rx.recv_timeout(Duration::from_secs(30))
                .expect("project event processed");
        }
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

    /// Test hook: make the retry of an incomplete watch set due now and wait until the worker did
    /// it (and published the rescan it causes). Nothing is retried when the set is complete.
    #[cfg(test)]
    pub(crate) fn make_retry_due_for_test(&self) {
        let worker = self.worker.as_ref().expect("a worker");
        let (tx, rx) = mpsc::sync_channel(1);
        worker
            .watches
            .wake
            .send(WatchMessage::RetryDue(tx))
            .expect("project worker");
        match rx.recv_timeout(Duration::from_secs(30)) {
            Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => {}
            Err(e) => panic!("retry hook: {e}"),
        }
    }

    /// Test hook: put an answer into the worker's slot as if the worker had published it (for
    /// example one whose watch failed after a scan that no longer lists a project).
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn publish_answer_for_test(&self, projects: Option<Vec<MatchedProject>>, failed: bool) {
        let worker = self.worker.as_ref().expect("a worker");
        worker.slot.publish(projects, failed);
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
    /// Returns once an answer sits in the slot, or once the detection is no longer `pending()`
    /// (the answer was already taken by `poll`, e.g. by a drain that ran before this call, and
    /// nothing else is coming). Waiting for a slot value alone would never end in that case.
    /// The 120 s bound only guards against a worker that never answers.
    ///
    /// This is for the first answer only. A rescan (a later request after the first answer was
    /// taken) is not waited for here: once `pending()` is false this returns at once, so wait for
    /// a rescan with the `inject_event` fence instead.
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
            .wait_timeout_while(guard, Duration::from_secs(120), |value| {
                value.is_none() && self.pending()
            })
            .unwrap_or_else(|e| e.into_inner());
        assert!(
            !timeout.timed_out() || guard.is_some(),
            "project worker did not answer"
        );
    }

    /// Fence queued watch/path messages, without waiting for the resident watcher to exit.
    #[cfg(any(test, feature = "test-hooks"))]
    pub(crate) fn fence_watch_for_test(&self) {
        if let Some(worker) = &self.worker {
            let (tx, rx) = mpsc::sync_channel(1);
            if worker.watches.wake.send(WatchMessage::Settled(tx)).is_ok() {
                match rx.recv_timeout(Duration::from_secs(30)) {
                    Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => {}
                    Err(e) => panic!("project idle fence: watch publication: {e}"),
                }
            }
        }
    }

    #[cfg(any(test, feature = "test-hooks"))]
    pub(crate) fn watch_settled_for_test(&self) -> bool {
        self.worker.as_ref().is_none_or(|w| {
            w.slot
                .value
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .is_none()
                && !w.watches.dirty.load(Ordering::Acquire)
        })
    }
}
