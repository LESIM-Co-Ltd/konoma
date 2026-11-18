use super::*;
use crate::project::{DetectionMarker, ProjectScreen};
use konoma_test_support::unique_tmp;
use std::sync::atomic::AtomicUsize;

pub(super) fn declaration(id: &str, marker: &str, kind: MarkerKind) -> ProjectDeclaration {
    ProjectDeclaration {
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
        session_watch_paths: None,
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
        session_watch_paths: None,
        plugin: decl.plugin,
        root: dir.to_path_buf(),
        marker: "project.marker".into(),
        declaration: decl
            .project
            .watch("metadata/index")
            .watch("future/nested/file"),
    };
    let targets = watch_targets(&dir, &[project]);
    // `future/nested/file` is covered by the root (its nearest existing ancestor, recursive), which
    // also covers `metadata`: nothing is nested in another watch.
    assert!(targets.contains(&(dir.to_path_buf(), true)));
    assert!(!targets.contains(&(dir.to_path_buf(), false)));
    assert!(!targets.contains(&(dir.join("metadata"), true)));
}

/// No desired watch is nested in another: a recursive watch covers the same directory watched
/// non-recursively and everything below it, recursive or not (the backend does not keep them
/// independent: notify 8.2.0 inotify.rs ~486-499).
#[test]
fn project_watch_targets_drop_every_entry_a_recursive_watch_covers() {
    let dir = unique_tmp("konoma_project_watch_targets_nested");
    let base = dir.canonicalize().unwrap_or(dir.to_path_buf());
    let root = base.join("root");
    let (other, sub) = (root.join("other"), root.join("other").join("sub"));
    std::fs::create_dir_all(&sub).unwrap();
    let decl = declaration("nested", "project.marker", MarkerKind::Any);
    let project_with = |paths: Vec<PathBuf>| MatchedProject {
        session_watch_paths: Some(paths),
        plugin: decl.plugin.clone(),
        root: root.clone(),
        marker: "project.marker".into(),
        declaration: decl.project.clone(),
    };
    // Recursive `other` and `other/sub`: only the outer one.
    let targets = watch_targets(&root, &[project_with(vec![other.clone(), sub.clone()])]);
    assert!(targets.contains(&(other.clone(), true)));
    assert!(!targets.iter().any(|(path, _)| path == &sub), "{targets:?}");
    // Only `other/sub`: it stays.
    let targets = watch_targets(&root, &[project_with(vec![sub.clone()])]);
    assert!(targets.contains(&(sub.clone(), true)));
    // The root's ancestors are non-recursive entries: those under a recursive watch are dropped.
    let targets = watch_targets(&root, &[project_with(vec![base.clone()])]);
    assert!(targets.contains(&(base.clone(), true)));
    for (path, recursive) in &targets {
        assert!(
            *recursive || !path.starts_with(&base),
            "a non-recursive watch under a recursive one: {path:?}"
        );
    }
    assert!(
        targets
            .iter()
            .any(|(path, recursive)| !recursive && base.starts_with(path) && path != &base),
        "the ancestors above the recursive watch stay"
    );
}

#[test]
fn project_retry_backoff_doubles_up_to_its_cap() {
    let (first, cap) = (Duration::from_millis(100), Duration::from_millis(3200));
    let mut delay = first;
    let mut seen = Vec::new();
    for _ in 0..8 {
        seen.push(delay.as_millis());
        delay = doubled(delay, cap);
    }
    assert_eq!(seen, [100, 200, 400, 800, 1600, 3200, 3200, 3200]);
}

#[test]
fn project_worker_results_keep_the_latest_answer_and_preserve_it_on_watch_failure() {
    let slot = Slot::default();
    let decl = declaration("latest", "project.marker", MarkerKind::Any);
    let project = MatchedProject {
        session_watch_paths: None,
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
    let events = EventSender::new(tx);
    enqueue_event(
        &events,
        Ok(notify::Event::new(notify::EventKind::Access(
            notify::event::AccessKind::Any,
        ))),
        || {},
    );
    assert!(rx.try_recv().is_err(), "reads do not trigger detection");
    for name in ["unrelated", "project.marker"] {
        enqueue_event(
            &events,
            Ok(notify::Event::new(notify::EventKind::Any).add_path(name.into())),
            || {},
        );
    }
    assert!(
        events.overflow.load(Ordering::Relaxed),
        "a lost path requires a full scan"
    );
    assert!(
        !events.lost.load(Ordering::Relaxed),
        "ordinary events are no watch-loss signal"
    );
    enqueue_event(
        &events,
        Err(notify::Error::generic("fixture watch failure")),
        || {},
    );
    assert!(
        events.failed.load(Ordering::Relaxed),
        "a full queue cannot hide watch failures"
    );
    let WatchMessage::Event(event) = rx.try_recv().unwrap() else {
        panic!("notification event");
    };
    assert_eq!(event.unwrap().paths, [PathBuf::from("unrelated")]);
    assert!(rx.try_recv().is_err());
}

#[test]
fn project_ensure_with_an_empty_declaration_set_is_ready_at_once_with_no_projects() {
    let dir = unique_tmp("project_ensure_empty");
    std::fs::create_dir_all(&dir).unwrap();
    let mut detection = ProjectDetection::default();
    assert!(!detection.pending());
    detection.ensure(&dir, Vec::new());
    assert!(!detection.pending(), "nothing to wait for");
    assert!(detection.projects.is_empty());
    assert!(detection.worker.is_none(), "no worker for no declarations");
    assert_eq!(detection.poll(), None);
    assert!(detection.is_for(&dir));
}

#[test]
fn project_ensure_keeps_the_worker_for_the_same_root_and_cancels_it_for_a_new_root() {
    let first = unique_tmp("project_ensure_first");
    let second = unique_tmp("project_ensure_second");
    for dir in [&first, &second] {
        std::fs::create_dir_all(dir).unwrap();
    }
    std::fs::write(second.join("project.marker"), "").unwrap();
    let decls = || vec![declaration("ensure", "project.marker", MarkerKind::File)];
    let mut detection = ProjectDetection::default();
    detection.ensure(&first, decls());
    assert!(detection.pending());
    let old = detection.clone();
    assert!(old.shares_worker(&detection));

    // The same root is a no-op, even with different declarations.
    detection.ensure(&first, Vec::new());
    assert!(detection.shares_worker(&old));
    assert!(detection.pending());
    assert!(!old
        .worker
        .as_ref()
        .unwrap()
        .cancelled
        .load(Ordering::Relaxed));

    // A new root restarts: the old worker is cancelled and the answer is for the new root.
    detection.ensure(&second, decls());
    assert!(!detection.shares_worker(&old));
    assert!(old
        .worker
        .as_ref()
        .unwrap()
        .cancelled
        .load(Ordering::Relaxed));
    assert!(detection.pending());
    assert!(detection.projects.is_empty());
    detection.wait_for_result();
    assert_eq!(detection.poll(), Some(false));
    assert_eq!(
        detection
            .projects
            .iter()
            .map(|p| (p.plugin.as_str(), p.marker.as_str()))
            .collect::<Vec<_>>(),
        [("ensure", "project.marker")]
    );
}

#[test]
fn project_set_watch_paths_records_paths_only_for_a_known_project() {
    let dir = unique_tmp("project_set_watch_paths");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("project.marker"), "").unwrap();
    let root = dir.canonicalize().unwrap();
    let mut detection = ProjectDetection::default();
    detection.ensure(
        &root,
        vec![declaration("paths", "project.marker", MarkerKind::File)],
    );
    detection.wait_for_result();
    assert_eq!(detection.poll(), Some(false));
    let watches = detection.worker.as_ref().unwrap().watches.clone();
    detection.set_watch_paths("unknown", &root, "project.marker", vec![root.join("x")]);
    // The worker may consume the dirty flag at any time, so the recorded paths are observed.
    assert!(watches.paths.lock().unwrap().is_empty(), "unknown project");
    detection.set_watch_paths("paths", &root, "project.marker", vec![root.join("x")]);
    assert_eq!(
        watches.paths.lock().unwrap().values().collect::<Vec<_>>(),
        [&vec![root.join("x")]]
    );
    assert_eq!(
        detection.projects[0].session_watch_paths,
        Some(vec![root.join("x")])
    );
}

/// A drain (`poll`) that runs before the test hook takes the answer out of the slot; no second
/// answer comes, so a wait on the slot value alone could only end at its 120 s guard. The wait
/// must end once the detection is no longer pending. (Under load the worker answers before the
/// first drain of an e2e test, which is how this order shows up.)
#[test]
fn project_wait_for_result_returns_when_the_answer_was_already_polled() {
    let dir = unique_tmp("project_wait_after_poll");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("project.marker"), "").unwrap();
    let root = dir.canonicalize().unwrap();
    let mut detection = ProjectDetection::default();
    detection.ensure(
        &root,
        vec![declaration("polled", "project.marker", MarkerKind::File)],
    );
    detection.wait_for_result();
    assert_eq!(detection.poll(), Some(false));
    assert!(!detection.pending());
    // The answer is consumed: this must return at once, not after the guard.
    let started = std::time::Instant::now();
    detection.wait_for_result();
    assert!(
        started.elapsed() < Duration::from_secs(30),
        "wait_for_result waited for an answer that was already taken"
    );
    assert_eq!(detection.poll(), None);
}

/// A detection for a root that carries a `project.marker` file, with its first answer taken.
fn detected_root(root: &Path) -> ProjectDetection {
    std::fs::create_dir_all(root).unwrap();
    std::fs::write(root.join("project.marker"), "").unwrap();
    let mut detection = ProjectDetection::default();
    detection.ensure(
        root,
        vec![declaration("gone", "project.marker", MarkerKind::File)],
    );
    detection.wait_for_result();
    assert_eq!(detection.poll(), Some(false));
    assert_eq!(detection.projects.len(), 1);
    assert!(detection.uses_injected_events());
    detection
}

fn modified(path: PathBuf) -> notify::Result<notify::Event> {
    Ok(
        notify::Event::new(notify::EventKind::Modify(notify::event::ModifyKind::Any))
            .add_path(path),
    )
}

/// A root that was deleted is not a detection failure: there are no projects, and no failure
/// flag for the host to announce on top of the tree's own "out of date" notice.
#[test]
fn project_root_deleted_answers_no_projects_without_failure() {
    let dir = unique_tmp("project_root_deleted");
    let root = dir.canonicalize().unwrap_or(dir.to_path_buf()).join("root");
    let mut detection = detected_root(&root);
    std::fs::remove_dir_all(&root).unwrap();
    detection.inject_event(modified(root.join("project.marker")));
    assert_eq!(detection.poll(), Some(false), "not a failure");
    assert!(detection.projects.is_empty(), "the old projects are gone");
}

/// The worker keeps waiting for the root: it answers again once the root is created again.
#[test]
fn project_root_recreated_is_detected_again() {
    let dir = unique_tmp("project_root_recreated");
    let root = dir.canonicalize().unwrap_or(dir.to_path_buf()).join("root");
    let mut detection = detected_root(&root);
    std::fs::remove_dir_all(&root).unwrap();
    detection.inject_event(modified(root.join("project.marker")));
    assert_eq!(detection.poll(), Some(false));
    assert!(detection.projects.is_empty());
    // Back with the marker; the event arrives on the directory that still existed.
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("project.marker"), "").unwrap();
    detection.inject_event(modified(dir.to_path_buf()));
    assert_eq!(detection.poll(), Some(false));
    assert_eq!(detection.projects.len(), 1);
    assert_eq!(detection.projects[0].root, root);
    // The watch installation of the normal loop is back: a later marker change is noticed.
    std::fs::remove_file(root.join("project.marker")).unwrap();
    detection.inject_event(modified(root.join("project.marker")));
    assert_eq!(detection.poll(), Some(false));
    assert!(detection.projects.is_empty());
}

/// The directories the newest injected watcher that added anything under `base` holds there. A
/// worker never edits a watcher, it builds a fresh one, so the newest instance is what is installed.
fn installed_under(base: &Path) -> BTreeSet<PathBuf> {
    let log = logged_under(base);
    let newest = log.iter().map(|(instance, _, _)| *instance).max();
    log.into_iter()
        .filter(|(instance, _, _)| Some(*instance) == newest)
        .map(|(_, path, _)| path)
        .collect()
}

/// The root vanishes, then its parent: the worker's watch moves up to the next existing
/// ancestor (observed through the injected watcher's log), and the root is detected once back.
#[test]
fn project_root_and_parent_deleted_then_recreated_is_detected_again() {
    let dir = unique_tmp("project_root_parent_recreated");
    let base = dir.canonicalize().unwrap_or(dir.to_path_buf());
    let parent = base.join("parent");
    let root = parent.join("root");
    let mut detection = detected_root(&root);
    std::fs::remove_dir_all(&root).unwrap();
    detection.inject_event(modified(root.join("project.marker")));
    assert_eq!(detection.poll(), Some(false));
    assert!(detection.projects.is_empty());
    let held = installed_under(&base);
    assert_eq!(
        held,
        BTreeSet::from([parent.clone()]),
        "waiting on the root's parent only"
    );
    // The parent vanishes while waiting: the watch moves up to the base directory.
    std::fs::remove_dir_all(&parent).unwrap();
    detection.inject_event(modified(parent.clone()));
    assert_eq!(detection.poll(), None, "root still missing: no new answer");
    assert_eq!(
        installed_under(&base),
        BTreeSet::from([base.clone()]),
        "the vanished parent is unwatched, the next existing ancestor is watched"
    );
    // Both come back; the event arrives on the base directory.
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("project.marker"), "").unwrap();
    detection.inject_event(modified(base.clone()));
    assert_eq!(detection.poll(), Some(false));
    assert_eq!(detection.projects.len(), 1);
}

/// A root that exists but cannot be read is a real failure, unlike a missing root.
#[cfg(unix)]
#[test]
fn project_unreadable_root_still_answers_failed() {
    use std::os::unix::fs::PermissionsExt;
    let dir = unique_tmp("project_root_unreadable");
    let root = dir.canonicalize().unwrap_or(dir.to_path_buf()).join("root");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o000)).unwrap();
    // Restores the mode on every exit, a panic included, so the temp dir can be deleted.
    struct Restore(PathBuf);
    impl Drop for Restore {
        fn drop(&mut self) {
            let _ = std::fs::set_permissions(&self.0, std::fs::Permissions::from_mode(0o755));
        }
    }
    let _restore = Restore(root.clone());
    if std::fs::read_dir(&root).is_ok() {
        // Running as a user the mode does not bind (uid 0).
        return;
    }
    let mut detection = ProjectDetection::default();
    detection.ensure(
        &root,
        vec![declaration("gone", "project.marker", MarkerKind::File)],
    );
    detection.wait_for_result();
    assert_eq!(detection.poll(), Some(true));
}

/// A scan that fails because the root is gone is "missing" by the error it returned: a root that
/// is created again before the worker looks (here: inside the hook that runs right after the failed
/// scan) must still be waited for and detected, not end the worker.
#[test]
fn project_root_recreated_right_after_the_failed_scan_is_detected_again() {
    let dir = unique_tmp("project_root_recreated_after_scan");
    let root = dir.canonicalize().unwrap_or(dir.to_path_buf()).join("root");
    let mut detection = detected_root(&root);
    std::fs::remove_dir_all(&root).unwrap();
    let again = root.clone();
    *detection
        .worker
        .as_ref()
        .unwrap()
        .slot
        .after_failed_scan
        .lock()
        .unwrap() = Some(Box::new(move || {
        std::fs::create_dir_all(&again).unwrap();
        std::fs::write(again.join("project.marker"), "").unwrap();
    }));
    detection.inject_event(modified(root.join("project.marker")));
    assert_eq!(detection.poll(), Some(false), "not a failure");
    assert_eq!(detection.projects.len(), 1, "the returned root is detected");
    // The worker is alive: a later change is still noticed.
    std::fs::remove_file(root.join("project.marker")).unwrap();
    detection.inject_event(modified(root.join("project.marker")));
    assert_eq!(detection.poll(), Some(false));
    assert!(detection.projects.is_empty());
}

/// A parent that turns into a file makes the root's metadata NotADirectory: still "missing". The
/// root is detected again once the parent is a directory again.
#[test]
fn project_root_under_a_parent_that_became_a_file_waits_and_is_detected_again() {
    let dir = unique_tmp("project_parent_became_file");
    let base = dir.canonicalize().unwrap_or(dir.to_path_buf());
    let parent = base.join("parent");
    let root = parent.join("root");
    let mut detection = detected_root(&root);
    std::fs::remove_dir_all(&parent).unwrap();
    std::fs::write(&parent, "now a file").unwrap();
    detection.inject_event(modified(root.join("project.marker")));
    assert_eq!(detection.poll(), Some(false), "not a failure");
    assert!(detection.projects.is_empty());
    std::fs::remove_file(&parent).unwrap();
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("project.marker"), "").unwrap();
    detection.inject_event(modified(base.clone()));
    assert_eq!(detection.poll(), Some(false));
    assert_eq!(detection.projects.len(), 1);
    assert_eq!(detection.projects[0].root, root);
}

/// A watch-path update that arrives while the worker waits for the root is consumed there: the
/// host's idle check must not stay false until the root returns, and the paths are applied by the
/// scan once it does.
#[test]
fn project_watch_paths_set_while_the_root_is_missing_are_consumed_and_kept() {
    let dir = unique_tmp("project_paths_while_missing");
    let root = dir.canonicalize().unwrap_or(dir.to_path_buf()).join("root");
    let mut detection = detected_root(&root);
    std::fs::remove_dir_all(&root).unwrap();
    detection.inject_event(modified(root.join("project.marker")));
    // The answer is not polled yet, so the host still knows the project and may report paths.
    let paths = vec![root.join("extra")];
    detection.set_watch_paths("gone", &root, "project.marker", paths.clone());
    detection.fence_watch_for_test();
    assert_eq!(detection.poll(), Some(false));
    assert!(detection.projects.is_empty());
    assert!(
        detection.watch_settled_for_test(),
        "the wait consumed the dirty flag"
    );
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("project.marker"), "").unwrap();
    detection.inject_event(modified(root.parent().unwrap().to_path_buf()));
    assert_eq!(detection.poll(), Some(false));
    assert_eq!(detection.projects.len(), 1);
    assert_eq!(
        detection.projects[0].session_watch_paths,
        Some(paths),
        "the session paths are applied after the root returned"
    );
}

/// The scan reads the root, then the parent vanishes before the scan reaches it: the failed read is
/// of an ancestor of the root, which means the root is gone too. Not a failure; the root is
/// waited for and detected once it is back.
#[test]
fn project_scan_failing_on_a_vanished_ancestor_is_a_missing_root() {
    let dir = unique_tmp("project_ancestor_vanished_in_scan");
    let base = dir.canonicalize().unwrap_or(dir.to_path_buf());
    let parent = base.join("parent");
    let root = parent.join("root");
    let mut detection = detected_root(&root);
    let (hook_root, hook_parent) = (root.clone(), parent.clone());
    let mut fired = false;
    *detection
        .worker
        .as_ref()
        .unwrap()
        .slot
        .after_directory
        .lock()
        .unwrap() = Some(Box::new(move |directory: &PathBuf| {
        if directory == &hook_root && !fired {
            fired = true;
            std::fs::remove_dir_all(&hook_parent).unwrap();
        }
    }));
    detection.inject_event(modified(root.join("project.marker")));
    assert_eq!(detection.poll(), Some(false), "not a failure");
    assert!(detection.projects.is_empty());
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("project.marker"), "").unwrap();
    detection.inject_event(modified(base.clone()));
    assert_eq!(detection.poll(), Some(false));
    assert_eq!(detection.projects.len(), 1, "detected again");
}

/// The root vanishes after a successful scan, while the watches are installed: the watch error is
/// "path not found", not a failure. The worker scans again, finds the root missing and waits.
#[test]
fn project_root_vanishing_while_watches_are_installed_is_a_missing_root() {
    let dir = unique_tmp("project_root_vanishes_at_watch");
    let base = dir.canonicalize().unwrap_or(dir.to_path_buf());
    let root = base.join("root");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("project.marker"), "").unwrap();
    let mut declared = declaration("gone", "project.marker", MarkerKind::File);
    declared.project = declared.project.watch("sub");
    let mut detection = ProjectDetection::default();
    detection.ensure(&root, vec![declared]);
    detection.wait_for_result();
    assert_eq!(detection.poll(), Some(false));
    assert_eq!(detection.projects.len(), 1);
    // The declared watch path appears: the next scan wants a new watch on it, and the root is
    // deleted right before that watch is installed.
    std::fs::create_dir_all(root.join("sub")).unwrap();
    let doomed = root.clone();
    *detection
        .worker
        .as_ref()
        .unwrap()
        .slot
        .before_watches
        .lock()
        .unwrap() = Some(Box::new(move || {
        std::fs::remove_dir_all(&doomed).unwrap();
    }));
    detection.inject_event(modified(root.join("sub")));
    assert_eq!(detection.poll(), Some(false), "not a failure");
    assert!(detection.projects.is_empty());
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("project.marker"), "").unwrap();
    detection.inject_event(modified(base.clone()));
    assert_eq!(detection.poll(), Some(false));
    assert_eq!(detection.projects.len(), 1, "detected again");
}

/// Same rule for a session watch-path update handled while the worker idles: the reported path
/// vanishes before its watch is installed; the worker scans again and keeps the project.
#[test]
fn project_watch_path_vanishing_before_a_session_watch_is_installed_scans_again() {
    let dir = unique_tmp("project_session_path_vanishes");
    let root = dir.canonicalize().unwrap_or(dir.to_path_buf()).join("root");
    let mut detection = detected_root(&root);
    let extra = root.join("extra");
    std::fs::create_dir_all(&extra).unwrap();
    let doomed = extra.clone();
    *detection
        .worker
        .as_ref()
        .unwrap()
        .slot
        .before_watches
        .lock()
        .unwrap() = Some(Box::new(move || {
        std::fs::remove_dir_all(&doomed).unwrap();
    }));
    detection.set_watch_paths("gone", &root, "project.marker", vec![extra]);
    detection.fence_watch_for_test();
    assert_eq!(detection.poll(), Some(false), "not a failure");
    assert_eq!(detection.projects.len(), 1, "the project is kept");
    // The worker is alive: a later change is still noticed.
    std::fs::remove_file(root.join("project.marker")).unwrap();
    detection.inject_event(modified(root.join("project.marker")));
    assert_eq!(detection.poll(), Some(false));
    assert!(detection.projects.is_empty());
}

/// The nearest existing ancestor chosen by the wait vanishes before its watch is installed: the
/// wait moves up to the next existing ancestor instead of ending the worker.
#[test]
fn project_wait_ancestor_vanishing_before_its_watch_moves_up() {
    let dir = unique_tmp("project_wait_ancestor_vanishes");
    let base = dir.canonicalize().unwrap_or(dir.to_path_buf());
    let parent = base.join("parent");
    let child = parent.join("child");
    let root = child.join("root");
    // The root does not exist from the start: the worker waits, watching `parent`.
    std::fs::create_dir_all(&parent).unwrap();
    let mut detection = ProjectDetection::default();
    detection.ensure(
        &root,
        vec![declaration("gone", "project.marker", MarkerKind::File)],
    );
    detection.wait_for_result();
    assert_eq!(detection.poll(), Some(false));
    assert!(detection.projects.is_empty());
    assert!(detection.uses_injected_events());
    // The answer is published before the wait installs its watch: the fence waits for that.
    detection.fence_watch_for_test();
    assert!(installed_under(&base).contains(&parent));
    // `child` appears, so the wait wants it; it is removed right before that watch is added.
    let doomed = child.clone();
    let mut fired = false;
    on_each_watch(&detection, move |path| {
        if path == doomed && !fired {
            fired = true;
            std::fs::remove_dir_all(&doomed).unwrap();
        }
        None
    });
    std::fs::create_dir_all(&child).unwrap();
    detection.inject_event(modified(child.clone()));
    assert_eq!(detection.poll(), None, "root still missing: no new answer");
    let held = installed_under(&base);
    assert!(held.contains(&parent), "the wait chose again: {held:?}");
    assert!(!held.contains(&child), "the vanished ancestor is not held");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("project.marker"), "").unwrap();
    detection.inject_event(modified(parent));
    assert_eq!(detection.poll(), Some(false));
    assert_eq!(detection.projects.len(), 1, "detected again");
}

/// Install a hook that runs before every single watch the worker adds.
fn on_each_watch(
    detection: &ProjectDetection,
    hook: impl FnMut(&Path) -> Option<notify::Error> + Send + 'static,
) {
    *detection
        .worker
        .as_ref()
        .unwrap()
        .slot
        .before_each_watch
        .lock()
        .unwrap() = Some(Box::new(hook));
}

/// A detection whose project declares the watch path `watch` (relative to the root).
fn detected_root_watching(root: &Path, watch: &str) -> ProjectDetection {
    detected_root_watching_all(root, &[watch])
}

fn detected_root_watching_all(root: &Path, watches: &[&str]) -> ProjectDetection {
    std::fs::create_dir_all(root).unwrap();
    std::fs::write(root.join("project.marker"), "").unwrap();
    let mut declared = declaration("gone", "project.marker", MarkerKind::File);
    for watch in watches {
        declared.project = declared.project.watch(*watch);
    }
    let mut detection = ProjectDetection::default();
    detection.ensure(root, vec![declared]);
    detection.wait_for_result();
    assert_eq!(detection.poll(), Some(false));
    assert_eq!(detection.projects.len(), 1);
    assert!(detection.uses_injected_events());
    detection
}

fn logged_under(base: &Path) -> Vec<(usize, PathBuf, bool)> {
    INJECTED_WATCH_LOG
        .lock()
        .unwrap()
        .iter()
        .filter(|(_, path, _)| path.starts_with(base))
        .cloned()
        .collect()
}

/// Every watcher instance that added something under `base`.
fn logged_instances_under(base: &Path) -> BTreeSet<usize> {
    logged_under(base)
        .into_iter()
        .map(|(instance, _, _)| instance)
        .collect()
}

/// The declared target `root/other/sub` is created, so the wanted set moves from `other` to `sub`;
/// `sub` is removed right before its watch is added. The add is "not found": nothing is failed,
/// the next scan wants `other` again, and the rebuild after it watches `other`. A later change under
/// `other` is still processed: `sub` comes back and is watched.
#[test]
fn project_target_vanishing_before_its_add_leaves_the_parent_watched() {
    let dir = unique_tmp("project_back_to_parent");
    let base = dir.canonicalize().unwrap_or(dir.to_path_buf());
    let root = base.join("root");
    let other = root.join("other");
    let sub = other.join("sub");
    std::fs::create_dir_all(&other).unwrap();
    let mut detection = detected_root_watching(&root, "other/sub");
    assert!(
        logged_under(&base)
            .iter()
            .any(|(_, path, recursive)| path == &other && *recursive),
        "`other` is watched while `sub` does not exist"
    );
    std::fs::create_dir_all(&sub).unwrap();
    let doomed = sub.clone();
    let mut fired = false;
    on_each_watch(&detection, move |path| {
        if path == doomed && !fired {
            fired = true;
            std::fs::remove_dir_all(&doomed).unwrap();
        }
        None
    });
    let mark = logged_under(&base).len();
    detection.inject_event(modified(sub.clone()));
    assert_eq!(detection.poll(), Some(false), "not a failure");
    assert_eq!(detection.projects.len(), 1);
    let later = logged_under(&base)[mark..].to_vec();
    assert!(
        !later.iter().any(|(_, path, _)| path == &sub),
        "the vanished `sub` was never watched: {later:?}"
    );
    assert_eq!(
        installed_under(&base)
            .iter()
            .find(|p| p.starts_with(&other)),
        Some(&other),
        "`other` is watched again: {later:?}"
    );
    // A later change under `other` is processed: `sub` comes back and is watched.
    std::fs::create_dir_all(&sub).unwrap();
    let mark = logged_under(&base).len();
    detection.inject_event(modified(sub.clone()));
    assert_eq!(detection.poll(), Some(false));
    assert!(logged_under(&base)[mark..]
        .iter()
        .any(|(_, path, recursive)| path == &sub && *recursive));
}

/// The target is removed before each of 20 adds: every add is "not found", never a failure, and one
/// rebuild is tried per due retry (the retry is made due by the test hook; part 7 retried on every
/// fence). Once the churn ends the target is watched.
#[test]
fn project_target_removed_again_and_again_never_fails_the_worker() {
    use std::sync::atomic::AtomicUsize;
    const DELETIONS: usize = 20;
    let dir = unique_tmp("project_repeated_deletion");
    let base = dir.canonicalize().unwrap_or(dir.to_path_buf());
    let root = base.join("root");
    let sub = root.join("sub");
    let mut detection = detected_root_watching(&root, "sub");
    let deletions = Arc::new(AtomicUsize::new(0));
    let (counted, doomed) = (deletions.clone(), sub.clone());
    on_each_watch(&detection, move |path| {
        if path == doomed && counted.load(Ordering::Relaxed) < DELETIONS {
            counted.fetch_add(1, Ordering::Relaxed);
            std::fs::remove_dir_all(&doomed).unwrap();
        }
        None
    });
    // The scan after each deletion finds `sub` again, as if a build kept re-creating it.
    let (counted, again, scanned_root) = (deletions.clone(), sub.clone(), root.clone());
    *detection
        .worker
        .as_ref()
        .unwrap()
        .slot
        .after_directory
        .lock()
        .unwrap() = Some(Box::new(move |directory: &PathBuf| {
        if directory == &scanned_root && counted.load(Ordering::Relaxed) < DELETIONS {
            std::fs::create_dir_all(&again).unwrap();
        }
    }));
    std::fs::create_dir_all(&sub).unwrap();
    detection.inject_event(modified(sub.clone()));
    assert_eq!(detection.poll(), Some(false), "not failed");
    let mut retries = 0;
    while deletions.load(Ordering::Relaxed) < DELETIONS {
        detection.make_retry_due_for_test();
        assert_eq!(detection.poll(), Some(false), "not failed");
        retries += 1;
        assert!(
            retries < 100,
            "one attempt per due retry: the count must rise"
        );
    }
    assert_eq!(deletions.load(Ordering::Relaxed), DELETIONS);
    // The churn ends: `sub` stays, and the next change under it is watched.
    std::fs::create_dir_all(&sub).unwrap();
    let mark = logged_under(&base).len();
    detection.inject_event(modified(sub.clone()));
    assert_eq!(detection.poll(), Some(false));
    assert_eq!(
        logged_under(&base)[mark..]
            .iter()
            .filter(|(_, path, _)| path == &sub)
            .count(),
        1,
        "watched once the churn stopped"
    );
    assert_eq!(detection.projects.len(), 1);
    // Still alive.
    std::fs::remove_file(root.join("project.marker")).unwrap();
    detection.inject_event(modified(root.join("project.marker")));
    assert_eq!(detection.poll(), Some(false));
    assert!(detection.projects.is_empty());
}

fn missing_root_error_is_not_a_failure(error: impl Fn(&Path) -> notify::Error) {
    let dir = unique_tmp("project_async_missing_error");
    let root = dir.canonicalize().unwrap_or(dir.to_path_buf()).join("root");
    let mut detection = detected_root(&root);
    std::fs::remove_dir_all(&root).unwrap();
    detection.inject_event(Err(error(&root)));
    assert_eq!(detection.poll(), Some(false), "not a failure");
    assert!(detection.projects.is_empty());
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("project.marker"), "").unwrap();
    detection.inject_event(modified(dir.to_path_buf()));
    assert_eq!(detection.poll(), Some(false));
    assert_eq!(detection.projects.len(), 1, "detected again");
}

#[test]
fn project_async_path_not_found_error_for_a_deleted_root_is_not_a_failure() {
    missing_root_error_is_not_a_failure(|root| {
        notify::Error::path_not_found().add_path(root.to_path_buf())
    });
}

#[test]
fn project_async_io_not_found_error_for_a_deleted_root_is_not_a_failure() {
    missing_root_error_is_not_a_failure(|root| {
        notify::Error::io(io::Error::from(io::ErrorKind::NotFound)).add_path(root.to_path_buf())
    });
}

/// Errors about paths that exist (or name no path) stay failures, whatever their kind.
#[test]
fn project_async_errors_for_existing_paths_are_failures() {
    type ErrorOf = fn(&Path) -> notify::Error;
    let cases: [(&str, ErrorOf); 4] = [
        ("permission denied", |root| {
            notify::Error::io(io::Error::from(io::ErrorKind::PermissionDenied))
                .add_path(root.to_path_buf())
        }),
        ("generic", |_| notify::Error::generic("fixture failure")),
        ("not found but the path exists", |root| {
            notify::Error::path_not_found().add_path(root.to_path_buf())
        }),
        ("not found without a path", |_| {
            notify::Error::path_not_found()
        }),
    ];
    for (name, error) in cases {
        let dir = unique_tmp("project_async_real_error");
        let root = dir.canonicalize().unwrap_or(dir.to_path_buf()).join("root");
        let mut detection = detected_root(&root);
        detection.inject_event(Err(error(&root)));
        assert_eq!(detection.poll(), Some(true), "{name} is a failure");
    }
}

/// What an asynchronous watch error "says a path vanished" means: a missing kind and every named
/// path gone as a directory, judged with symlinks followed.
#[test]
fn project_watch_error_says_path_gone_judges_the_named_paths() {
    let dir = unique_tmp("project_watch_error_gone");
    std::fs::create_dir_all(&dir).unwrap();
    let base = dir.canonicalize().unwrap_or(dir.to_path_buf());
    let removed = base.join("removed");
    std::fs::create_dir(&removed).unwrap();
    std::fs::remove_dir(&removed).unwrap();
    let dangling = base.join("dangling");
    std::os::unix::fs::symlink(base.join("no-such-target"), &dangling).unwrap();
    let file = base.join("now-a-file");
    std::fs::write(&file, "").unwrap();
    let existing = base.join("existing");
    std::fs::create_dir(&existing).unwrap();
    let missing = || notify::Error::path_not_found();
    assert!(watch_error_says_path_gone(
        &missing().add_path(removed.clone())
    ));
    assert!(watch_error_says_path_gone(&missing().add_path(dangling)));
    assert!(watch_error_says_path_gone(&missing().add_path(file)));
    assert!(!watch_error_says_path_gone(&missing().add_path(existing)));
    assert!(!watch_error_says_path_gone(&missing()));
    let denied = notify::Error::io(io::Error::from(io::ErrorKind::PermissionDenied));
    assert!(!watch_error_says_path_gone(&denied.add_path(removed)));
}

type ErrorOf = fn(&Path) -> notify::Error;

/// The "not found" answers an add may give for a target that exists: named, unnamed, and naming a
/// subdirectory that vanished during the walk.
fn not_found_cases() -> [(&'static str, ErrorOf); 4] {
    [
        ("named", |target| {
            notify::Error::path_not_found().add_path(target.to_path_buf())
        }),
        ("unnamed", |_| notify::Error::path_not_found()),
        ("unnamed io", |_| {
            notify::Error::io(io::Error::from(io::ErrorKind::NotFound))
        }),
        ("named subdirectory", |target| {
            notify::Error::path_not_found().add_path(target.join("vanished-inner"))
        }),
    ]
}

/// Fail the adds of `target` with `error` for the first `failures` attempts and count every attempt.
/// An attempt beyond 50 answers permission denied, so a worker that spins ends as failed instead of
/// hanging the test.
fn fail_adds_of(
    detection: &ProjectDetection,
    target: PathBuf,
    error: ErrorOf,
    failures: usize,
) -> Arc<AtomicUsize> {
    let attempts = Arc::new(AtomicUsize::new(0));
    let counted = attempts.clone();
    on_each_watch(detection, move |path| {
        if path != target {
            return None;
        }
        let attempt = counted.fetch_add(1, Ordering::Relaxed) + 1;
        if attempt > 50 {
            return Some(notify::Error::io(io::Error::from(
                io::ErrorKind::PermissionDenied,
            )));
        }
        (attempt <= failures).then(|| error(&target))
    });
    attempts
}

/// A "not found" add (named or not) for a target that exists is not a failure: the projects are
/// published, and only a due retry rebuilds again. (Part 7 retried on the next fence: a fence, an
/// event and a path update start no retry now.)
#[test]
fn project_not_found_add_for_an_existing_target_is_retried_only_when_due() {
    for (name, error) in not_found_cases() {
        let dir = unique_tmp("project_not_found_retry");
        let base = dir.canonicalize().unwrap_or(dir.to_path_buf());
        let root = base.join("root");
        let sub = root.join("sub");
        let mut detection = detected_root_watching(&root, "sub");
        std::fs::create_dir_all(&sub).unwrap();
        // The event's rebuild fails; the one at the due retry succeeds.
        let attempts = fail_adds_of(&detection, sub.clone(), error, 1);
        let mark = logged_under(&base).len();
        detection.inject_event(modified(sub.clone()));
        assert_eq!(detection.poll(), Some(false), "{name}: not a failure");
        assert_eq!(detection.projects.len(), 1, "{name}: projects published");
        assert_eq!(
            attempts.load(Ordering::Relaxed),
            1,
            "{name}: one attempt for the event"
        );
        detection.fence_watch_for_test();
        assert_eq!(
            attempts.load(Ordering::Relaxed),
            1,
            "{name}: a fence retries nothing"
        );
        assert!(
            !logged_under(&base)[mark..]
                .iter()
                .any(|(_, path, _)| path == &sub),
            "{name}: not watched yet"
        );
        detection.make_retry_due_for_test();
        assert_eq!(attempts.load(Ordering::Relaxed), 2, "{name}: the due retry");
        assert_eq!(detection.poll(), Some(false), "{name}: its rescan answers");
        assert_eq!(
            logged_under(&base)[mark..]
                .iter()
                .filter(|(_, path, recursive)| path == &sub && *recursive)
                .count(),
            1,
            "{name}: the rebuild added it"
        );
        // Complete now: nothing is left to retry, even when a retry is made due.
        detection.make_retry_due_for_test();
        assert_eq!(
            attempts.load(Ordering::Relaxed),
            2,
            "{name}: nothing to retry"
        );
        assert_eq!(detection.poll(), None, "{name}: and no answer");
    }
}

/// A set that stays incomplete is retried only when its retry is due: no event, path update or fence
/// makes a rebuild attempt, a due retry makes exactly one (and never a failure).
#[test]
fn project_incomplete_set_retries_only_when_the_retry_is_due() {
    for (name, error) in not_found_cases() {
        let dir = unique_tmp("project_not_found_always");
        let base = dir.canonicalize().unwrap_or(dir.to_path_buf());
        let root = base.join("root");
        let sub = root.join("sub");
        let mut detection = detected_root_watching(&root, "sub");
        std::fs::create_dir_all(&sub).unwrap();
        let attempts = fail_adds_of(&detection, sub.clone(), error, usize::MAX);
        detection.inject_event(modified(sub.clone()));
        assert_eq!(detection.poll(), Some(false), "{name}");
        assert_eq!(attempts.load(Ordering::Relaxed), 1, "{name}: the event");
        let instances = logged_instances_under(&base);
        // Events, a path update and fences: no attempt, whatever their number.
        for round in 0..4 {
            detection.inject_event(modified(sub.clone()));
            detection.inject_event(modified(root.join("project.marker")));
            detection.fence_watch_for_test();
            if round == 0 {
                detection.set_watch_paths("gone", &root, "project.marker", vec![sub.clone()]);
                detection.fence_watch_for_test();
            }
            assert_eq!(
                attempts.load(Ordering::Relaxed),
                1,
                "{name}: round {round}: nothing but a due retry retries"
            );
            assert_eq!(logged_instances_under(&base), instances, "{name}");
            while detection.poll().is_some() {}
        }
        for due in 1..=4 {
            detection.make_retry_due_for_test();
            assert_eq!(detection.poll(), Some(false), "{name}: due {due}");
            assert_eq!(
                attempts.load(Ordering::Relaxed),
                1 + due,
                "{name}: exactly one attempt per due retry"
            );
            assert_eq!(detection.projects.len(), 1, "{name}: projects published");
        }
        // Alive: a marker change is still noticed.
        std::fs::remove_file(root.join("project.marker")).unwrap();
        detection.inject_event(modified(root.join("project.marker")));
        assert_eq!(detection.poll(), Some(false), "{name}");
        assert!(detection.projects.is_empty(), "{name}");
    }
}

/// A scan slower than the backoff must not turn the retries into continuous work: the outer
/// (scan) loop never takes a due retry, so every rebuild is followed by a publish. Here every scan
/// ends with the retry due, as if it had outlasted the backoff.
#[test]
fn project_slow_scan_never_keeps_a_retry_from_publishing() {
    for (name, error) in not_found_cases() {
        let dir = unique_tmp("project_slow_scan_retry");
        let base = dir.canonicalize().unwrap_or(dir.to_path_buf());
        let root = base.join("root");
        let sub = root.join("sub");
        let mut detection = detected_root_watching(&root, "sub");
        std::fs::create_dir_all(&sub).unwrap();
        let attempts = fail_adds_of(&detection, sub.clone(), error, usize::MAX);
        detection
            .worker
            .as_ref()
            .unwrap()
            .slot
            .scan_makes_retry_due
            .store(true, Ordering::Relaxed);
        // The event changes the wanted set: one rebuild (the add of `sub` fails), then the rescan
        // ends with the retry due, and the answer is still published without another rebuild.
        detection.inject_event(modified(sub.clone()));
        assert_eq!(detection.poll(), Some(false), "{name}: answered");
        assert_eq!(detection.projects.len(), 1, "{name}: projects published");
        assert_eq!(
            attempts.load(Ordering::Relaxed),
            1,
            "{name}: a due retry is not taken by the scan loop"
        );
        // The worker is idle now (the fence is acknowledged after everything before it): the
        // retry made due by the slow scan waits for a whole backoff counted from the idle start.
        detection.fence_watch_for_test();
        assert_eq!(
            attempts.load(Ordering::Relaxed),
            1,
            "{name}: the idle loop does not retry at once either"
        );
        // The retry made due while idle: exactly one more rebuild, its rescan answers, and the
        // fence it owed is acknowledged (the call returns).
        detection.make_retry_due_for_test();
        assert_eq!(detection.poll(), Some(false), "{name}: the retry answers");
        assert_eq!(
            attempts.load(Ordering::Relaxed),
            2,
            "{name}: exactly one rebuild for the retry"
        );
        detection.fence_watch_for_test();
        assert_eq!(attempts.load(Ordering::Relaxed), 2, "{name}: and no more");
    }
}

/// The wait for a missing root follows the same rule as the idle loop: a due timed retry is taken
/// only by a turn that follows a receive, and the backoff counts from entering the blocking wait.
/// Here every turn of the wait starts with the retry due (as if the existence check and the ancestor
/// search had outlasted the backoff) and the ancestor's add never succeeds: a rebuild must still
/// be followed by a receive, so a fence is acknowledged and a due retry makes exactly one attempt.
#[test]
fn project_wait_for_a_missing_root_does_not_retry_continuously() {
    for (name, error) in not_found_cases() {
        let dir = unique_tmp("project_wait_slow_retry");
        let base = dir.canonicalize().unwrap_or(dir.to_path_buf());
        let root = base.join("root");
        let mut detection = detected_root(&root);
        let attempts = fail_adds_of(&detection, base.clone(), error, usize::MAX);
        detection
            .worker
            .as_ref()
            .unwrap()
            .slot
            .wait_makes_retry_due
            .store(true, Ordering::Relaxed);
        std::fs::remove_dir_all(&root).unwrap();
        detection.inject_event(modified(root.join("project.marker")));
        assert_eq!(detection.poll(), Some(false), "{name}: no failure");
        assert!(detection.projects.is_empty(), "{name}");
        // The worker reached its receive: the fence is acknowledged, after exactly one attempt.
        detection.fence_watch_for_test();
        assert_eq!(
            attempts.load(Ordering::Relaxed),
            1,
            "{name}: the wait rebuilt once and then blocked"
        );
        // A due retry: exactly one more attempt, then the worker blocks again.
        detection.make_retry_due_for_test();
        assert_eq!(
            attempts.load(Ordering::Relaxed),
            2,
            "{name}: exactly one attempt for the due retry"
        );
        detection.fence_watch_for_test();
        assert_eq!(
            attempts.load(Ordering::Relaxed),
            2,
            "{name}: a plain fence retries nothing"
        );
    }
}

/// The backoff is measured from when the worker becomes idle: a rebuild whose scan took longer than
/// the backoff does not leave the retry due.
#[test]
fn project_retry_backoff_counts_from_the_idle_start() {
    let dir = unique_tmp("project_backoff_idle");
    let target = dir.canonicalize().unwrap_or(dir.to_path_buf());
    let (tx, _rx) = mpsc::sync_channel(1);
    let mut set = WatchSet::new(
        EventSender::new(tx),
        #[cfg(any(test, feature = "test-hooks"))]
        false,
        #[cfg(all(test, feature = "app"))]
        false,
    )
    .expect("watch set");
    assert!(set.is_injected());
    let desired = BTreeSet::from([(target, true)]);
    set.rebuild(&desired, &mut |_| Some(notify::Error::path_not_found()))
        .expect("not found is no failure");
    assert!(set.incomplete);
    // The scan after the rebuild outlasted the backoff.
    set.retry_at = Instant::now() - Duration::from_secs(1);
    assert!(set.retry_due(), "the premise: the retry is due");
    set.enter_idle();
    assert!(
        !set.retry_due(),
        "idle time of a whole backoff follows the cycle"
    );
    assert!(
        set.retry_at >= Instant::now() + RETRY_FIRST - Duration::from_secs(60),
        "the backoff counts from the idle start"
    );
    // Once only: a later idle entry (after an event) does not push the retry again.
    set.retry_at = Instant::now() - Duration::from_secs(1);
    set.enter_idle();
    assert!(set.retry_due(), "no new backoff without a rebuild");
}

/// The fenced loop of the apps settles while a set stays incomplete: a fence is acknowledged with no
/// answer published after the first settled one (a fence that started a retry would publish again
/// and again, and the loop would never be idle).
#[test]
fn project_fences_settle_while_the_set_stays_incomplete() {
    for (name, error) in not_found_cases() {
        let dir = unique_tmp("project_fences_settle");
        let base = dir.canonicalize().unwrap_or(dir.to_path_buf());
        let root = base.join("root");
        let sub = root.join("sub");
        let mut detection = detected_root_watching(&root, "sub");
        std::fs::create_dir_all(&sub).unwrap();
        fail_adds_of(&detection, sub.clone(), error, usize::MAX);
        detection.inject_event(modified(sub.clone()));
        assert_eq!(detection.poll(), Some(false), "{name}: the first answer");
        for fence in 1..=5 {
            detection.fence_watch_for_test();
            assert_eq!(
                detection.poll(),
                None,
                "{name}: fence {fence} is acknowledged without an answer"
            );
            assert!(detection.watch_settled_for_test(), "{name}: fence {fence}");
        }
    }
}

/// Any other add error (permission, I/O) still fails the worker.
#[test]
fn project_permission_denied_add_error_is_a_failure() {
    let dir = unique_tmp("project_add_permission_denied");
    let base = dir.canonicalize().unwrap_or(dir.to_path_buf());
    let root = base.join("root");
    let sub = root.join("sub");
    let mut detection = detected_root_watching(&root, "sub");
    std::fs::create_dir_all(&sub).unwrap();
    fail_adds_of(
        &detection,
        sub.clone(),
        |target| {
            notify::Error::io(io::Error::from(io::ErrorKind::PermissionDenied))
                .add_path(target.to_path_buf())
        },
        usize::MAX,
    );
    // A failing worker ends before it can answer a fence: take the answer instead.
    detection.set_watch_paths("gone", &root, "project.marker", vec![sub]);
    detection.fence_watch_for_test();
    detection.wait_for_result();
    assert_eq!(detection.poll(), Some(true), "a failure");
}

/// The root is deleted and created again (a fresh directory, the old one kept so its inode cannot
/// be reused) before the worker scans: the scan succeeds and the wanted set is unchanged, but the
/// directory watched is not the root any more. The identity check rebuilds the watcher, so the
/// fresh watcher watches the new root; the later marker change is detected.
#[test]
fn project_root_replaced_before_the_scan_gets_a_fresh_watcher() {
    let dir = unique_tmp("project_root_replaced");
    let base = dir.canonicalize().unwrap_or(dir.to_path_buf());
    let root = base.join("root");
    let mut detection = detected_root(&root);
    let instances_of = |path: &Path| -> BTreeSet<usize> {
        logged_under(&base)
            .into_iter()
            .filter(|(_, watched, _)| watched == path)
            .map(|(instance, _, _)| instance)
            .collect()
    };
    let before = instances_of(&root);
    assert_eq!(before.len(), 1);
    std::fs::rename(&root, base.join("old-root")).unwrap();
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("project.marker"), "").unwrap();
    detection.inject_event(modified(root.join("project.marker")));
    assert_eq!(detection.poll(), Some(false), "not a failure");
    assert_eq!(detection.projects.len(), 1);
    let after = instances_of(&root);
    assert!(
        after.len() == 2 && after.is_superset(&before),
        "a fresh watcher added the replaced root: {before:?} then {after:?}"
    );
    std::fs::remove_file(root.join("project.marker")).unwrap();
    detection.inject_event(modified(root.join("project.marker")));
    assert_eq!(detection.poll(), Some(false));
    assert!(detection.projects.is_empty());
}

/// Declared `other` and `other/sub` (both recursive): only `other` is added. When the session's
/// paths change to `other/sub` only, a rebuild adds `other/sub`.
#[test]
fn project_nested_watches_install_only_the_outer_and_a_change_rebuilds() {
    let dir = unique_tmp("project_nested_watches");
    let base = dir.canonicalize().unwrap_or(dir.to_path_buf());
    let root = base.join("root");
    let other = root.join("other");
    let sub = other.join("sub");
    std::fs::create_dir_all(&sub).unwrap();
    let mut detection = detected_root_watching_all(&root, &["other", "other/sub"]);
    let log = logged_under(&base);
    assert!(
        log.iter()
            .any(|(_, path, recursive)| path == &other && *recursive),
        "{log:?}"
    );
    assert!(
        !log.iter().any(|(_, path, _)| path == &sub),
        "`other/sub` is covered by `other`: {log:?}"
    );
    let first_instance = log.iter().map(|(instance, _, _)| *instance).max().unwrap();
    detection.set_watch_paths("gone", &root, "project.marker", vec![sub.clone()]);
    detection.fence_watch_for_test();
    let log = logged_under(&base);
    let rebuilt: Vec<_> = log
        .iter()
        .filter(|(instance, _, _)| *instance > first_instance)
        .collect();
    assert!(
        rebuilt
            .iter()
            .any(|(_, path, recursive)| path == &sub && *recursive),
        "a rebuild adds `other/sub`: {log:?}"
    );
    assert!(
        !rebuilt.iter().any(|(_, path, _)| path == &other),
        "`other` is no longer wanted: {log:?}"
    );
}

/// An asynchronous error naming a subdirectory that is gone (the watched target exists) is a
/// rescan request, not a failure.
#[test]
fn project_async_error_naming_a_vanished_subdirectory_is_not_a_failure() {
    let dir = unique_tmp("project_async_sub_vanished");
    let root = dir.canonicalize().unwrap_or(dir.to_path_buf()).join("root");
    let mut detection = detected_root(&root);
    let sub = root.join("sub");
    std::fs::create_dir_all(&sub).unwrap();
    std::fs::remove_dir_all(&sub).unwrap();
    detection.inject_event(Err(notify::Error::path_not_found().add_path(sub)));
    assert_eq!(detection.poll(), Some(false), "not a failure");
    assert_eq!(detection.projects.len(), 1, "the project is kept");
}

/// An event of a lost-watch kind, naming `path`.
fn lost(kind: notify::EventKind, path: &Path) -> notify::Result<notify::Event> {
    Ok(notify::Event::new(kind).add_path(path.to_path_buf()))
}

fn renamed_away(path: &Path) -> notify::Result<notify::Event> {
    lost(
        notify::EventKind::Modify(notify::event::ModifyKind::Name(
            notify::event::RenameMode::From,
        )),
        path,
    )
}

fn removed_folder(path: &Path) -> notify::Result<notify::Event> {
    lost(
        notify::EventKind::Remove(notify::event::RemoveKind::Folder),
        path,
    )
}

/// A directory renamed away and back keeps its dev+inode, but inotify drops its watch: the signal
/// (a `Remove` or a rename naming an installed path or a directory above it, or the rescan flag) must
/// rebuild the watcher and scan again, with no change of identity to show for it. The marker is
/// removed without any event, so only a rescan can see it gone.
#[test]
fn project_watch_loss_signals_rebuild_and_rescan_with_the_identity_unchanged() {
    type Signal = fn(&Path, &Path) -> notify::Result<notify::Event>;
    let cases: [(&str, Signal); 6] = [
        ("root renamed away", |root, _| renamed_away(root)),
        ("root renamed to", |root, _| {
            lost(
                notify::EventKind::Modify(notify::event::ModifyKind::Name(
                    notify::event::RenameMode::To,
                )),
                root,
            )
        }),
        ("root removed", |root, _| removed_folder(root)),
        ("a directory above the root removed", |_, base| {
            removed_folder(base)
        }),
        ("a directory above the root renamed", |_, base| {
            renamed_away(base)
        }),
        ("queue overflow", |_, _| {
            Ok(notify::Event::new(notify::EventKind::Other).set_flag(notify::event::Flag::Rescan))
        }),
    ];
    for (name, signal) in cases {
        let dir = unique_tmp("project_watch_loss");
        let base = dir.canonicalize().unwrap_or(dir.to_path_buf());
        let root = base.join("root");
        let mut detection = detected_root(&root);
        let identity = identity_of(&root);
        let before = logged_instances_under(&base);
        std::fs::remove_file(root.join("project.marker")).unwrap();
        detection.inject_event(signal(&root, &base));
        assert_eq!(
            identity_of(&root),
            identity,
            "{name}: the directory is the same"
        );
        assert!(
            logged_instances_under(&base).len() > before.len(),
            "{name}: a fresh watcher was built"
        );
        assert_eq!(detection.poll(), Some(false), "{name}: it scanned again");
        assert!(
            detection.projects.is_empty(),
            "{name}: and saw the marker gone"
        );
    }
}

/// An event of the same kinds naming a path inside a recursive watch (or an unrelated path) is not a
/// signal: notify keeps nested watches itself, so the watcher is not rebuilt.
#[test]
fn project_events_deeper_inside_a_recursive_watch_do_not_rebuild() {
    let dir = unique_tmp("project_watch_loss_inner");
    let base = dir.canonicalize().unwrap_or(dir.to_path_buf());
    let root = base.join("root");
    let sub = root.join("sub");
    let inner = sub.join("inner");
    std::fs::create_dir_all(&inner).unwrap();
    let mut detection = detected_root_watching(&root, "sub");
    let before = logged_instances_under(&base);
    assert_eq!(before.len(), 1);
    for (name, event) in [
        ("inner removed", removed_folder(&inner)),
        ("inner renamed", renamed_away(&inner)),
        (
            "file removed",
            lost(
                notify::EventKind::Remove(notify::event::RemoveKind::File),
                &sub.join("a.txt"),
            ),
        ),
        ("marker renamed", renamed_away(&root.join("project.marker"))),
        ("unrelated removed", removed_folder(&base.join("elsewhere"))),
    ] {
        detection.inject_event(event);
        while detection.poll().is_some() {}
        assert_eq!(logged_instances_under(&base), before, "{name}: no rebuild");
    }
}

/// While the root is missing the worker waits on the nearest existing ancestor: a signal about that
/// ancestor rebuilds the wait's watcher too.
#[test]
fn project_wait_rebuilds_on_a_watch_loss_signal() {
    let dir = unique_tmp("project_wait_watch_loss");
    let base = dir.canonicalize().unwrap_or(dir.to_path_buf());
    let root = base.join("root");
    let mut detection = detected_root(&root);
    std::fs::remove_dir_all(&root).unwrap();
    detection.inject_event(modified(root.join("project.marker")));
    assert_eq!(detection.poll(), Some(false));
    assert!(detection.projects.is_empty());
    let before = logged_instances_under(&base);
    for (round, event) in [
        removed_folder(&base),
        renamed_away(&base),
        Ok(notify::Event::new(notify::EventKind::Other).set_flag(notify::event::Flag::Rescan)),
    ]
    .into_iter()
    .enumerate()
    {
        detection.inject_event(event);
        assert_eq!(
            logged_instances_under(&base).len(),
            before.len() + round + 1,
            "round {round}: a fresh watcher"
        );
    }
    // Still waiting, and the return of the root is still seen.
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("project.marker"), "").unwrap();
    detection.inject_event(modified(base));
    assert_eq!(detection.poll(), Some(false));
    assert_eq!(detection.projects.len(), 1);
}

/// A rebuild in the idle loop (here: the session's watch paths changed) is followed by a rescan: a
/// marker that changes while the rebuild adds its watches (every add succeeds, and no event ever
/// arrives) is found by the scan after it.
#[test]
fn project_idle_rebuild_is_followed_by_a_rescan() {
    let dir = unique_tmp("project_idle_rebuild_rescan");
    let root = dir.canonicalize().unwrap_or(dir.to_path_buf()).join("root");
    let mut detection = detected_root(&root);
    let extra = root.join("extra");
    std::fs::create_dir_all(&extra).unwrap();
    let (marker, watched) = (root.join("project.marker"), extra.clone());
    let mut fired = false;
    on_each_watch(&detection, move |path| {
        if path == watched && !fired {
            fired = true;
            std::fs::remove_file(&marker).unwrap();
        }
        None
    });
    detection.set_watch_paths("gone", &root, "project.marker", vec![extra]);
    detection.fence_watch_for_test();
    assert_eq!(
        detection.poll(),
        Some(false),
        "the rebuild's rescan answers"
    );
    assert!(detection.projects.is_empty(), "and finds the marker gone");
}

/// The rebuild fills the new watcher before the old one is dropped, so the old watches keep
/// reporting during the walk.
#[test]
fn project_rebuild_fills_the_new_watcher_before_dropping_the_old_one() {
    let dir = unique_tmp("project_fill_then_drop");
    let base = dir.canonicalize().unwrap_or(dir.to_path_buf());
    let root = base.join("root");
    let (other, third) = (root.join("other"), root.join("third"));
    std::fs::create_dir_all(&other).unwrap();
    std::fs::create_dir_all(&third).unwrap();
    let mut detection = detected_root(&root);
    let old = *logged_instances_under(&base).iter().max().unwrap();
    detection.set_watch_paths("gone", &root, "project.marker", vec![other, third]);
    detection.fence_watch_for_test();
    let new = *logged_instances_under(&base).iter().max().unwrap();
    assert!(new > old, "the paths changed, so a fresh watcher was built");
    let trace = INJECTED_TRACE.lock().unwrap().clone();
    let last_add = trace
        .iter()
        .rposition(|(instance, path)| *instance == new && path.is_some())
        .expect("the new watcher added its paths");
    let old_drop = trace
        .iter()
        .position(|(instance, path)| *instance == old && path.is_none())
        .expect("the old watcher was dropped");
    assert!(
        old_drop > last_add,
        "the old watcher is dropped after the new one is filled: {trace:?}"
    );
}

/// Fill the worker's one-slot queue (the worker is held in a fence acknowledgement while a wake
/// occupies the slot), deliver `event` through the callback path so that it is the one dropped,
/// then release the worker and fence it: the answer is whatever the worker did for the burst.
fn deliver_into_a_full_queue(detection: &ProjectDetection, event: notify::Result<notify::Event>) {
    let events = detection
        .worker
        .as_ref()
        .and_then(|worker| worker.events.clone())
        .expect("injected project events");
    let (hold, release) = mpsc::sync_channel(0);
    events.tx.send(WatchMessage::Settled(hold)).unwrap();
    events.tx.send(WatchMessage::PathsChanged).unwrap();
    let mut dropped = false;
    enqueue_event(&events, event, || dropped = true);
    assert!(dropped, "the queue was full: the event was dropped");
    drop(release);
    // An ignored Access event only fences: the worker has dealt with the burst when it returns.
    detection.inject_event(Ok(notify::Event::new(notify::EventKind::Access(
        notify::event::AccessKind::Any,
    ))));
}

/// A burst that overflows the worker's queue with only ordinary events rescans and does not rebuild
/// (every save makes a burst; a rebuild re-walks a recursive work tree).
#[test]
fn project_overflow_of_ordinary_events_rescans_without_a_rebuild() {
    let dir = unique_tmp("project_overflow_ordinary");
    let base = dir.canonicalize().unwrap_or(dir.to_path_buf());
    let root = base.join("root");
    let mut detection = detected_root(&root);
    let before = logged_instances_under(&base);
    std::fs::remove_file(root.join("project.marker")).unwrap();
    deliver_into_a_full_queue(&detection, modified(root.join("somewhere.txt")));
    assert_eq!(detection.poll(), Some(false), "it scanned again");
    assert!(detection.projects.is_empty(), "and saw the marker gone");
    assert_eq!(logged_instances_under(&base), before, "but did not rebuild");
}

/// A watch-loss signal that is the dropped event of a full queue still rebuilds: the callback
/// classified it before it tried to queue it.
#[test]
fn project_watch_loss_dropped_by_a_full_queue_still_rebuilds_and_rescans() {
    let dir = unique_tmp("project_loss_dropped");
    let base = dir.canonicalize().unwrap_or(dir.to_path_buf());
    let root = base.join("root");
    let mut detection = detected_root(&root);
    let before = logged_instances_under(&base);
    std::fs::remove_file(root.join("project.marker")).unwrap();
    deliver_into_a_full_queue(&detection, renamed_away(&root));
    assert_eq!(detection.poll(), Some(false), "it scanned again");
    assert!(detection.projects.is_empty(), "and saw the marker gone");
    assert_eq!(
        logged_instances_under(&base).len(),
        before.len() + 1,
        "and built a fresh watcher"
    );
}

/// The window of a rebuild: the old watcher is dropped after the new list is published, and its
/// callback may deliver until then. A signal naming a path only the OLD watcher holds, arriving at
/// that moment (the hook runs when the old watcher is dropped), still counts: the list holds the old
/// and the new paths until the drop.
#[test]
fn project_signal_from_the_old_watcher_during_the_rebuild_window_still_counts() {
    let dir = unique_tmp("project_old_watcher_window");
    let base = dir.canonicalize().unwrap_or(dir.to_path_buf());
    let root = base.join("root");
    let (other, third) = (root.join("other"), root.join("third"));
    std::fs::create_dir_all(&other).unwrap();
    std::fs::create_dir_all(&third).unwrap();
    let mut detection = detected_root_watching(&root, "other");
    let old = *logged_instances_under(&base).iter().max().unwrap();
    assert!(logged_under(&base)
        .iter()
        .any(|(instance, path, _)| *instance == old && path == &other));
    let events = detection
        .worker
        .as_ref()
        .and_then(|worker| worker.events.clone())
        .unwrap();
    let named = other.clone();
    INJECTED_DROP_HOOKS.lock().unwrap().push((
        old,
        Box::new(move || enqueue_event(&events, removed_folder(&named), || {})),
    ));
    let before = logged_instances_under(&base);
    detection.set_watch_paths("gone", &root, "project.marker", vec![third]);
    detection.fence_watch_for_test();
    // The rebuild itself, then the one the late signal asked for.
    let mut after = logged_instances_under(&base).len();
    while detection.poll().is_some() {}
    after = after.max(logged_instances_under(&base).len());
    assert_eq!(
        after,
        before.len() + 2,
        "the late signal of the old watcher rebuilt again"
    );
}
