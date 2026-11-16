use super::*;
use crate::project::{DetectionMarker, ProjectScreen};
use konoma_test_support::unique_tmp;

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
    assert!(targets.contains(&(dir.join("metadata"), true)));
    assert!(targets.contains(&(dir.to_path_buf(), true)));
    assert!(!targets.contains(&(dir.to_path_buf(), false)));
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
    let overflow = AtomicBool::new(false);
    let failed = AtomicBool::new(false);
    enqueue_event(
        &tx,
        &overflow,
        &failed,
        Ok(notify::Event::new(notify::EventKind::Access(
            notify::event::AccessKind::Any,
        ))),
        || {},
    );
    assert!(rx.try_recv().is_err(), "reads do not trigger detection");
    for name in ["unrelated", "project.marker"] {
        enqueue_event(
            &tx,
            &overflow,
            &failed,
            Ok(notify::Event::new(notify::EventKind::Any).add_path(name.into())),
            || {},
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
        || {},
    );
    assert!(
        failed.load(Ordering::Relaxed),
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

/// The root vanishes, then its parent: the worker's watch moves up to the next existing
/// ancestor (observed through the injected watcher's log), and the root is detected once back.
#[test]
fn project_root_and_parent_deleted_then_recreated_is_detected_again() {
    let dir = unique_tmp("project_root_parent_recreated");
    let base = dir.canonicalize().unwrap_or(dir.to_path_buf());
    let parent = base.join("parent");
    let root = parent.join("root");
    let mut detection = detected_root(&root);
    let watches_in_base = || -> Vec<(PathBuf, bool)> {
        INJECTED_WATCH_LOG
            .lock()
            .unwrap()
            .iter()
            .filter(|(path, _)| path.starts_with(&base))
            .cloned()
            .collect()
    };
    std::fs::remove_dir_all(&root).unwrap();
    detection.inject_event(modified(root.join("project.marker")));
    assert_eq!(detection.poll(), Some(false));
    assert!(detection.projects.is_empty());
    assert_eq!(
        watches_in_base().last(),
        Some(&(parent.clone(), true)),
        "waiting on the root's parent"
    );
    // The parent vanishes while waiting: the watch moves up to the base directory.
    std::fs::remove_dir_all(&parent).unwrap();
    detection.inject_event(modified(parent.clone()));
    assert_eq!(detection.poll(), None, "root still missing: no new answer");
    let log = watches_in_base();
    assert!(
        log.contains(&(parent.clone(), false)),
        "the vanished parent is unwatched: {log:?}"
    );
    assert_eq!(
        log.last(),
        Some(&(base.clone(), true)),
        "the watch moved to the next existing ancestor: {log:?}"
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
    if std::fs::read_dir(&root).is_ok() {
        // Running as a user the mode does not bind (uid 0).
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o755)).unwrap();
        return;
    }
    let mut detection = ProjectDetection::default();
    detection.ensure(
        &root,
        vec![declaration("gone", "project.marker", MarkerKind::File)],
    );
    detection.wait_for_result();
    let answer = detection.poll();
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(answer, Some(true));
}
