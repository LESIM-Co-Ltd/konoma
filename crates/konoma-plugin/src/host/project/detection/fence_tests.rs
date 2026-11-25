//! The event fence of the injected-event seam: an acknowledgement is sent only after the worker
//! has published whatever the preceding events (or a late overflow / watch failure) required.

use super::tests::declaration;
use super::*;
use konoma_test_support::unique_tmp;

#[cfg(feature = "app")]
#[test]
fn project_initial_wait_reports_worker_panic_or_exit_before_publication() {
    let dir = unique_tmp("project_worker_exit_before_publication");
    std::fs::create_dir_all(&dir).unwrap();
    for failure in [StartupFailure::Panic, StartupFailure::Exit] {
        let mut detection = ProjectDetection {
            startup_failure: Some(failure),
            ..Default::default()
        };
        detection.ensure(
            &dir,
            vec![declaration("fixture", ".marker", MarkerKind::Any)],
        );
        let slot = detection.worker.as_ref().unwrap().slot.clone();
        let (tx, rx) = mpsc::sync_channel(1);
        let waiter = std::thread::spawn(move || {
            let answer = detection.wait_for_initial();
            tx.send((answer, detection.pending(), detection.projects))
                .unwrap();
        });
        let answer = rx.recv_timeout(Duration::from_secs(30));
        if answer.is_err() {
            // Wake and join even with the publication guard removed by a mutation.
            slot.publish(None, true);
        }
        waiter.join().unwrap();
        let (answer, pending, projects) =
            answer.expect("initial wait must finish after worker exit");
        assert!(
            answer.is_err(),
            "a worker without a scan answer must report failure"
        );
        assert!(!pending);
        assert!(projects.is_empty());
    }
}

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
    let mut detection = ProjectDetection::default();
    let mut fixture = declaration("fence-fixture", ".fence-marker", MarkerKind::Directory);
    // Finding the marker adds watches, so the fence must span the following rescan too.
    fixture.project = fixture.project.watch(".").watch(".fence-marker");
    detection.ensure(&root, vec![fixture]);
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
    let marker = root.join(".fence-marker");
    std::fs::create_dir(&marker).unwrap();
    enqueue_event(
        events,
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
            .any(|project| project.plugin == "fence-fixture" && project.root == root));
    }
}
