//! Static project discovery and watching. All directory reads and watch setup run on a worker.

use std::collections::BTreeSet;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Condvar, Mutex};
use std::time::Duration;

use konoma_plugin::project::{MarkerKind, ProjectDecl};
use konoma_plugin::Plugin;
use notify::{RecursiveMode, Watcher};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MatchedProject {
    pub plugin: String,
    pub root: PathBuf,
    /// The actual entry name, not the declaration's glob.
    pub marker: String,
    pub declaration: ProjectDecl,
}

#[derive(Clone)]
struct Declaration {
    plugin: String,
    project: ProjectDecl,
}

/// Process plugins may advertise projects, but discovery is restricted to static plugins.
fn declarations(disabled: &[String]) -> Vec<Declaration> {
    let external: Vec<Arc<dyn Plugin>> = disabled
        .iter()
        .filter_map(|id| super::external::token_of(id))
        .flat_map(super::external::plugins_of)
        .map(|p| p as Arc<dyn Plugin>)
        .collect();
    super::with_registry(disabled, |r| static_declarations(r, &external))
}

fn static_declarations(
    registry: &konoma_plugin::registry::Registry,
    external: &[Arc<dyn Plugin>],
) -> Vec<Declaration> {
    registry
        .ids()
        .into_iter()
        .filter(|id| {
            registry.is_enabled(id)
                && registry
                    .plugin(id)
                    .is_some_and(|plugin| external.iter().all(|p| !Arc::ptr_eq(p, &plugin)))
        })
        .filter_map(|id| {
            Some(Declaration {
                plugin: id.into(),
                project: registry.manifest(id)?.project.clone()?,
            })
        })
        .collect()
}

fn detect(
    root: &Path,
    declarations: &[Declaration],
    cancelled: &AtomicBool,
) -> io::Result<Vec<MatchedProject>> {
    detect_directories(root.ancestors(), declarations, cancelled)
}

fn detect_directories<'a>(
    directories: impl IntoIterator<Item = &'a Path>,
    declarations: &[Declaration],
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
                        _ => false,
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

fn affected(
    root: &Path,
    declarations: &[Declaration],
    projects: &[MatchedProject],
    paths: &[PathBuf],
) -> bool {
    paths.is_empty()
        || paths.iter().any(|path| {
            projects.iter().any(|p| {
                p.declaration.watch_paths.iter().any(|watch| {
                    let watch = p.root.join(watch);
                    path.starts_with(&watch) || watch.starts_with(path)
                })
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

fn watch_targets(root: &Path, projects: &[MatchedProject]) -> BTreeSet<(PathBuf, bool)> {
    let mut out: BTreeSet<_> = root.ancestors().map(|p| (p.to_path_buf(), false)).collect();
    for project in projects {
        for path in &project.declaration.watch_paths {
            if let Some(dir) = watch_directory(&project.root.join(path)) {
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
        drop(slot);
        self.ready.notify_all();
        drop(previous);
    }
}

struct Worker {
    slot: Arc<Slot>,
    cancelled: Arc<AtomicBool>,
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
}

fn enqueue_event(
    tx: &mpsc::SyncSender<notify::Result<notify::Event>>,
    overflow: &AtomicBool,
    failed: &AtomicBool,
    event: notify::Result<notify::Event>,
) {
    if matches!(&event, Ok(event) if matches!(&event.kind, notify::EventKind::Access(_))) {
        return;
    }
    if event.is_err() {
        failed.store(true, Ordering::Relaxed);
    }
    if matches!(tx.try_send(event), Err(mpsc::TrySendError::Full(_))) {
        // A burst occupies one event slot. Losing a named path invalidates everything,
        // so coalescing cannot miss a relevant marker or declared watch change.
        overflow.store(true, Ordering::Relaxed);
    }
}

fn run(root: PathBuf, declarations: Vec<Declaration>, slot: Arc<Slot>, cancelled: Arc<AtomicBool>) {
    let (tx, rx) = mpsc::sync_channel(1);
    let overflow = Arc::new(AtomicBool::new(false));
    let watch_failed = Arc::new(AtomicBool::new(false));
    let callback_overflow = overflow.clone();
    let callback_failed = watch_failed.clone();
    let mut watcher = match notify::recommended_watcher(move |event| {
        enqueue_event(&tx, &callback_overflow, &callback_failed, event);
    }) {
        Ok(watcher) => watcher,
        Err(_) => {
            slot.publish(detect(&root, &declarations, &cancelled).ok(), true);
            return;
        }
    };
    let mut watched = BTreeSet::new();
    loop {
        if cancelled.load(Ordering::Relaxed) {
            return;
        }
        let projects = match detect(&root, &declarations, &cancelled) {
            Ok(found) => found,
            Err(_) => {
                slot.publish(None, true);
                return;
            }
        };
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
        loop {
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
            match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(Ok(event)) => {
                    if !matches!(event.kind, notify::EventKind::Access(_))
                        && affected(&root, &declarations, &projects, &event.paths)
                    {
                        break;
                    }
                }
                Ok(Err(_)) => {
                    slot.publish(Some(projects), true);
                    return;
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
            }
        }
    }
}

/// Carried with the tab snapshot. A worker's latest answer occupies one slot even when the tab
/// is inactive; leaving a tab does not lose its cache or grow an unbounded result queue.
#[derive(Clone, Default)]
pub(crate) struct ProjectDetection {
    root: Option<PathBuf>,
    pub projects: Vec<MatchedProject>,
    worker: Option<Arc<Worker>>,
}

impl ProjectDetection {
    pub fn ensure(&mut self, root: &Path, disabled: &[String]) {
        if self.root.as_deref() == Some(root) {
            return;
        }
        self.root = Some(root.into());
        self.projects.clear();
        if let Some(worker) = &self.worker {
            worker.cancelled.store(true, Ordering::Relaxed);
        }
        self.worker = None;
        let declarations = declarations(disabled);
        if declarations.is_empty() {
            return;
        }
        let slot = Arc::new(Slot::default());
        let cancelled = Arc::new(AtomicBool::new(false));
        self.worker = Some(Arc::new(Worker {
            slot: slot.clone(),
            cancelled: cancelled.clone(),
        }));
        let root = root.to_path_buf();
        std::thread::spawn(move || run(root, declarations, slot, cancelled));
    }

    /// Nonblocking: neither discovery nor watch setup holds this lock.
    pub fn poll(&mut self) -> Option<bool> {
        let answer = self
            .worker
            .as_ref()?
            .slot
            .value
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()?;
        if let Some(projects) = answer.projects {
            self.projects = projects;
        }
        Some(answer.failed)
    }

    #[cfg(test)]
    pub(crate) fn shares_worker(&self, other: &Self) -> bool {
        match (&self.worker, &other.worker) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            _ => false,
        }
    }

    /// Wait for a worker answer in tests without sleeping or depending on its scheduling.
    #[cfg(test)]
    pub(crate) fn wait_for_result(&self) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::test_plugins::{self, ProjectFixture};
    use crate::test_support::unique_tmp;
    use konoma_plugin::project::{DetectionMarker, ProjectScreen};

    fn declaration(id: &str, marker: &str, kind: MarkerKind) -> Declaration {
        Declaration {
            plugin: id.into(),
            project: ProjectDecl::new()
                .marker(DetectionMarker::new(marker, kind))
                .screen(
                    ProjectScreen::new("items", "Items")
                        .key('i')
                        .row_kind("item"),
                ),
        }
    }

    #[test]
    fn project_detection_searches_upward_and_keeps_all_plugins_and_actual_glob_matches() {
        let dir = unique_tmp("konoma_project_detection");
        let nested = dir.join("nested");
        let root = nested.join("deep");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir(dir.join("A.project")).unwrap();
        std::fs::create_dir(dir.join("B.project")).unwrap();
        std::fs::write(dir.join("C.project"), "file").unwrap();
        std::fs::write(nested.join("project.marker"), "nested").unwrap();
        let decls = [
            declaration("directories", "*.project", MarkerKind::Directory),
            declaration("files", "*.project", MarkerKind::File),
            declaration("nested", "project.marker", MarkerKind::Any),
            declaration("case-sensitive", "*.PROJECT", MarkerKind::Any),
        ];
        let found = detect_directories(
            root.ancestors().take_while(|p| p.starts_with(&dir)),
            &decls,
            &AtomicBool::new(false),
        )
        .unwrap();
        let actual: Vec<_> = found
            .iter()
            .map(|p| (p.plugin.as_str(), p.root.as_path(), p.marker.as_str()))
            .collect();
        assert_eq!(
            actual,
            [
                ("nested", nested.as_path(), "project.marker"),
                ("directories", dir.as_path(), "A.project"),
                ("directories", dir.as_path(), "B.project"),
                ("files", dir.as_path(), "C.project"),
            ]
        );
    }

    #[test]
    fn project_detection_deduplicates_overlapping_markers_and_obeys_cancellation() {
        let dir = unique_tmp("konoma_project_marker_overlap");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("one.project"), "").unwrap();
        let mut decl = declaration("overlap", "*.project", MarkerKind::Any);
        decl.project
            .markers
            .push(DetectionMarker::new("one.project", MarkerKind::File));
        assert_eq!(
            detect_directories(
                [dir.as_path()],
                std::slice::from_ref(&decl),
                &AtomicBool::new(false),
            )
            .unwrap()
            .len(),
            1
        );
        assert_eq!(
            detect_directories([dir.as_path()], &[decl], &AtomicBool::new(true))
                .unwrap_err()
                .kind(),
            io::ErrorKind::Interrupted
        );
    }

    #[test]
    fn project_declared_watch_paths_and_marker_changes_invalidate_but_unrelated_files_do_not() {
        let root = Path::new("/fixture/tree/subdir");
        let decl = declaration("watched", "*.project", MarkerKind::Any);
        let project = MatchedProject {
            plugin: decl.plugin.clone(),
            root: PathBuf::from("/fixture/tree"),
            marker: "one.project".into(),
            declaration: decl
                .project
                .clone()
                .watch("metadata/index")
                .watch("settings"),
        };
        for path in [
            "/fixture/tree/metadata/index",
            "/fixture/tree/metadata",
            "/fixture/tree/settings/nested/file",
            "/fixture/new.project",
            "/fixture/tree/one.project",
        ] {
            assert!(
                affected(
                    root,
                    std::slice::from_ref(&decl),
                    std::slice::from_ref(&project),
                    &[path.into()],
                ),
                "{path}"
            );
        }
        for path in [
            "/fixture/tree/metadata/index.lock",
            "/fixture/tree/src/file",
            "/elsewhere/one.project",
            "/fixture/tree/subdir/deeper/two.project",
        ] {
            assert!(
                !affected(
                    root,
                    std::slice::from_ref(&decl),
                    std::slice::from_ref(&project),
                    &[path.into()],
                ),
                "{path}"
            );
        }
        assert!(affected(root, &[decl], &[project], &[]));
    }

    #[test]
    fn project_watch_targets_cover_nested_and_not_yet_created_paths() {
        let dir = unique_tmp("konoma_project_watch_targets");
        std::fs::create_dir_all(dir.join("metadata")).unwrap();
        let decl = declaration("watched", "project.marker", MarkerKind::Any);
        let project = MatchedProject {
            plugin: decl.plugin,
            root: dir.to_path_buf(),
            marker: "project.marker".into(),
            declaration: decl
                .project
                .watch("metadata/index")
                .watch("future/nested/file"),
        };
        let targets = watch_targets(&dir, &[project]);
        assert!(targets.contains(&(dir.join("metadata"), true)));
        assert!(targets.contains(&(dir.to_path_buf(), true)));
        assert!(!targets.contains(&(dir.to_path_buf(), false)));
    }

    #[test]
    fn project_detection_uses_static_registration_and_respects_disabled_plugins() {
        let decl = declaration("static-project", "project.marker", MarkerKind::Any);
        let marker = test_plugins::register(
            "project_detection_static",
            Arc::new(ProjectFixture {
                id: "static-project",
                project: decl.project.clone(),
                calls: Arc::default(),
                navigation: None,
            }),
        );
        let enabled = declarations(std::slice::from_ref(&marker));
        assert_eq!(enabled.len(), 1);
        assert_eq!(enabled[0].plugin, "static-project");
        assert!(declarations(&[marker, "static-project".into()]).is_empty());
    }

    #[test]
    fn project_static_discovery_excludes_the_process_instance_not_a_colliding_id() {
        let plugin = || -> Arc<dyn Plugin> {
            Arc::new(ProjectFixture {
                id: "project-identity",
                project: declaration("project-identity", "project.marker", MarkerKind::Any).project,
                calls: Arc::default(),
                navigation: None,
            })
        };
        let linked = plugin();
        let refused = plugin();
        let mut registry = konoma_plugin::registry::Registry::new();
        registry
            .register(linked.clone(), konoma_plugin::registry::PluginOrigin::User)
            .unwrap();
        assert_eq!(static_declarations(&registry, &[refused]).len(), 1);
        assert!(static_declarations(&registry, &[linked]).is_empty());
    }

    #[test]
    fn project_worker_results_keep_the_latest_answer_and_preserve_it_on_watch_failure() {
        let slot = Slot::default();
        let decl = declaration("latest", "project.marker", MarkerKind::Any);
        let project = MatchedProject {
            plugin: decl.plugin,
            root: "/fixture".into(),
            marker: "project.marker".into(),
            declaration: decl.project,
        };
        slot.publish(Some(vec![project.clone()]), false);
        slot.publish(None, true);
        let result = slot.value.lock().unwrap().take().unwrap();
        assert!(result.failed);
        assert_eq!(result.projects, Some(vec![project.clone()]));
        slot.publish(Some(vec![project]), false);
        slot.publish(Some(Vec::new()), false);
        assert_eq!(
            slot.value.lock().unwrap().take().unwrap().projects,
            Some(Vec::new())
        );
    }

    #[test]
    fn project_watch_event_queue_is_bounded_and_preserves_overflow_and_errors() {
        let (tx, rx) = mpsc::sync_channel(1);
        let overflow = AtomicBool::new(false);
        let failed = AtomicBool::new(false);
        enqueue_event(
            &tx,
            &overflow,
            &failed,
            Ok(notify::Event::new(notify::EventKind::Access(
                notify::event::AccessKind::Any,
            ))),
        );
        assert!(rx.try_recv().is_err(), "reads do not trigger detection");
        for name in ["unrelated", "project.marker"] {
            enqueue_event(
                &tx,
                &overflow,
                &failed,
                Ok(notify::Event::new(notify::EventKind::Any).add_path(name.into())),
            );
        }
        assert!(
            overflow.load(Ordering::Relaxed),
            "a lost path requires a full scan"
        );
        enqueue_event(
            &tx,
            &overflow,
            &failed,
            Err(notify::Error::generic("fixture watch failure")),
        );
        assert!(
            failed.load(Ordering::Relaxed),
            "a full queue cannot hide watch failures"
        );
        assert_eq!(
            rx.try_recv().unwrap().unwrap().paths,
            [PathBuf::from("unrelated")]
        );
        assert!(rx.try_recv().is_err());
    }
}
