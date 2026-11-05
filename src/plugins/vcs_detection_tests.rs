use super::*;
use crate::test_support::unique_tmp;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum WorkerCheckpoint {
    AfterFence,
    BeforeReceive,
}

pub(super) struct WorkerCheckpoints {
    reached: mpsc::SyncSender<WorkerCheckpoint>,
    resume: mpsc::Receiver<()>,
}

impl Slot {
    pub(super) fn checkpoint(&self, checkpoint: WorkerCheckpoint) {
        let checkpoints = self.checkpoints.lock().unwrap();
        if let Some(checkpoints) = checkpoints.as_ref() {
            if checkpoints.reached.send(checkpoint).is_ok() {
                let _ = checkpoints.resume.recv();
            }
        }
    }
}

#[test]
fn project_event_fence_publishes_overflow_rescan_before_acknowledging() {
    assert_late_overflow_fence(false);
}

#[test]
fn project_event_fence_publishes_late_watch_failure_before_acknowledging() {
    assert_late_overflow_fence(true);
}

fn assert_late_overflow_fence(failed: bool) {
    let dir = unique_tmp("project_fence_late_overflow");
    std::fs::create_dir_all(&dir).unwrap();
    let root = dir.canonicalize().unwrap();
    let registration = crate::plugins::test_plugins::register(
        "project_fence_late_overflow",
        Arc::new(konoma_vcs::Git),
    );
    let mut detection = ProjectDetection::default();
    detection.ensure(&root, &[registration]);
    detection.wait_for_result();
    assert_eq!(detection.poll(), Some(false));
    assert!(detection.projects.is_empty());
    let worker = detection.worker.as_ref().unwrap().clone();
    let events = worker.events.as_ref().unwrap();

    // Hold the worker in a zero-capacity fence acknowledgement, then fill its only slot.
    // The second send completes only once the worker has consumed the hold fence.
    let (hold, release) = mpsc::sync_channel(0);
    events.tx.send(WatchMessage::Settled(hold)).unwrap();
    events.tx.send(WatchMessage::PathsChanged).unwrap();
    let (reached, checkpoints) = mpsc::sync_channel(0);
    let (resume, proceed) = mpsc::sync_channel(0);
    *worker.slot.checkpoints.lock().unwrap() = Some(WorkerCheckpoints {
        reached,
        resume: proceed,
    });
    let checkpoint = |expected| {
        assert_eq!(
            checkpoints.recv_timeout(Duration::from_secs(30)).unwrap(),
            expected,
        );
    };
    let marker = root.join(".git");
    std::fs::create_dir(&marker).unwrap();
    enqueue_event(
        &events.tx,
        &events.overflow,
        &events.failed,
        Ok(notify::Event::new(notify::EventKind::Any).add_path(marker)),
        || {
            // Full has already been observed, but overflow has not been stored. Let the
            // worker consume PathsChanged and check the still-false flag before receiving.
            drop(release);
            checkpoint(WorkerCheckpoint::AfterFence);
            resume.send(()).unwrap();
            checkpoint(WorkerCheckpoint::BeforeReceive);
            resume.send(()).unwrap();
            checkpoint(WorkerCheckpoint::BeforeReceive);
        },
    );
    assert!(events.overflow.load(Ordering::Relaxed));
    if failed {
        // A callback error can also arrive after the worker's last flag check.
        events.failed.store(true, Ordering::Relaxed);
    }
    let (settled, acknowledgement) = mpsc::sync_channel(1);
    events.tx.send(WatchMessage::Settled(settled)).unwrap();
    resume.send(()).unwrap();
    acknowledgement
        .recv_timeout(Duration::from_secs(30))
        .expect("the fence must acknowledge after publishing");
    // Stop before the next flag check: a scan after the acknowledgement must not
    // race with our observation and accidentally make an immediate ack look correct.
    checkpoint(WorkerCheckpoint::AfterFence);
    assert_eq!(detection.poll(), Some(failed));
    if failed {
        assert!(
            detection.projects.is_empty(),
            "watch failure retains the last scan"
        );
    } else {
        assert!(detection
            .projects
            .iter()
            .any(|project| project.plugin == "git" && project.root == root));
    }
}

impl ProjectDetection {
    pub(crate) fn inject_event_with_pending_watch_update(
        &self,
        event: notify::Result<notify::Event>,
    ) {
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
}

fn vcs_declarations() -> Vec<Declaration> {
    [&konoma_vcs::Git as &dyn Plugin, &konoma_vcs::Jj]
        .into_iter()
        .map(|plugin| {
            let manifest = plugin.manifest();
            Declaration {
                plugin: manifest.id,
                project: manifest.project.unwrap(),
            }
        })
        .collect()
}

#[test]
fn vcs_projects_are_not_registered_as_standard_plugins_yet() {
    let registry = crate::plugins::build_registry(&[]);
    assert!(registry.manifest("git").is_none());
    assert!(registry.manifest("jj").is_none());
}

#[test]
fn vcs_project_detection_finds_both_colocated_roots_from_a_descendant() {
    let dir = unique_tmp("vcs_detection_colocated");
    std::fs::create_dir_all(dir.join(".git")).unwrap();
    std::fs::create_dir_all(dir.join(".jj")).unwrap();
    std::fs::create_dir_all(dir.join("nested/deep")).unwrap();
    let root = dir.canonicalize().unwrap();
    let found = detect_directories(
        [root.join("nested/deep"), root.join("nested"), root.clone()]
            .iter()
            .map(PathBuf::as_path),
        &vcs_declarations(),
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(
        found
            .iter()
            .map(|p| (p.plugin.as_str(), &p.root))
            .collect::<Vec<_>>(),
        [("git", &root), ("jj", &root)]
    );
    for (marker, expected) in [
        (".git/HEAD", true),
        (".git/index", true),
        (".git/info/exclude", true),
        (".jj/repo/op_heads/new-op", true),
        ("nested/file.txt", true),
    ] {
        assert_eq!(
            affected(&root, &vcs_declarations(), &found, &[root.join(marker)]),
            expected,
            "{marker}"
        );
    }
}

#[test]
fn vcs_project_detection_accepts_gitdir_files_and_rejects_a_jj_file() {
    let dir = unique_tmp("vcs_detection_files");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(".git"), "gitdir: metadata\n").unwrap();
    std::fs::write(dir.join(".jj"), "not a directory\n").unwrap();
    let found = detect_directories(
        [dir.as_path()],
        &vcs_declarations(),
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(
        found.iter().map(|p| p.plugin.as_str()).collect::<Vec<_>>(),
        ["git"]
    );
    assert_eq!(found[0].root, dir.as_path());
}

#[cfg(feature = "git")]
#[test]
fn vcs_project_watch_targets_follow_a_linked_worktrees_gitdir_file() {
    use crate::test_support::{init_git_repo, run_git};
    let main = unique_tmp("vcs_watch_main");
    std::fs::create_dir_all(&main).unwrap();
    init_git_repo(&main);
    std::fs::write(main.join("file.txt"), "base\n").unwrap();
    run_git(&main, &["add", "."]);
    run_git(&main, &["commit", "-qm", "seed"]);
    let linked = unique_tmp("vcs_watch_linked");
    run_git(
        &main,
        &[
            "worktree",
            "add",
            "-q",
            "--detach",
            linked.to_str().unwrap(),
        ],
    );
    let root = linked.canonicalize().unwrap();
    let found = detect(&root, &vcs_declarations(), &AtomicBool::new(false)).unwrap();
    let project = found
        .iter()
        .find(|p| p.plugin == "git" && p.root == root)
        .unwrap();
    assert_eq!(Some(&project.root), crate::git::workdir(&root).as_ref());
    let old = crate::app::App::new(root.clone(), crate::config::Config::default()).unwrap();
    let metadata = old
        .git_dir_watch()
        .expect("legacy host watches external gitdir");
    let marker =
        crate::plugins::test_plugins::register("vcs_watch_linked", Arc::new(konoma_vcs::Git));
    let mut host = crate::app::App::new(
        root.clone(),
        crate::plugins::test_plugins::marker_config(&marker),
    )
    .unwrap();
    host.tab.projects.wait_for_result();
    host.poll_projects();
    for _ in 0..32 {
        if !host.project_jobs_pending() {
            break;
        }
        host.wait_project_job();
    }
    let targets = watch_targets(&root, &host.tab.projects.projects);
    assert!(
        targets.iter().any(
            |(path, recursive)| path == &metadata || (*recursive && metadata.starts_with(path))
        ),
        "project watches must cover the legacy host's external gitdir"
    );
}

#[cfg(feature = "git")]
#[test]
fn vcs_project_detection_root_matches_the_existing_git_and_jj_readers() {
    use crate::test_support::{init_git_repo, jj_scratch_bare};
    let git = unique_tmp("vcs_detection_git_reader");
    std::fs::create_dir_all(git.join("nested/deep")).unwrap();
    init_git_repo(&git);
    let root = git.canonicalize().unwrap();
    let found = detect(
        &root.join("nested/deep"),
        &vcs_declarations(),
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!(found.iter().any(|p| p.plugin == "git"
        && Some(&p.root) == crate::git::workdir(&root.join("nested/deep")).as_ref()));
    if let Some(jj) = jj_scratch_bare("vcs_detection_jj_reader") {
        std::fs::create_dir_all(jj.join("nested/deep")).unwrap();
        let root = jj.canonicalize().unwrap();
        let found = detect(
            &root.join("nested/deep"),
            &vcs_declarations(),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert!(found.iter().any(|p| p.plugin == "jj"
            && Some(&p.root)
                == crate::vcs::jj::workspace_root(&root.join("nested/deep")).as_ref()));
    }
}
