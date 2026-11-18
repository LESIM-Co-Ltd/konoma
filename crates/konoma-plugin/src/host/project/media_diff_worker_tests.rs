use crate::host::project::md_diff::DiffBaseline;
use crate::host::project::media_diff::*;
use konoma_host_core::project::DiffTarget;
use std::path::PathBuf;
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

fn req(want: MediaDiffWant) -> MediaDiffRequest {
    MediaDiffRequest {
        gen: 0,
        path: want.0,
        page: want.1,
        raster_px: want.2,
        root: "fixture".into(),
        baseline: DiffBaseline::Empty,
        cancel: Arc::default(),
        classify: Arc::new(|_, _| DiffTarget::Picture),
        rasterize_svg: |_, _, _, _| unreachable!("controlled compute does not decode"),
    }
}

fn want(path: &str, page: u32, px: (u32, u32)) -> MediaDiffWant {
    (path.into(), page, px)
}
fn kick(worker: &mut MediaDiffWorker, want: MediaDiffWant) {
    if let Some(want) = worker.kick(want) {
        assert!(worker.dispatch(req(want)).is_none());
    }
}
fn apply(worker: &mut MediaDiffWorker, result: MediaDiffResult) -> bool {
    worker.apply(result, |computed| match computed {
        MediaDiffComputed::Summary {
            base,
            same_bytes,
            old_len,
            new_len,
        } => MediaDiffOutcome::Summary {
            base,
            same_bytes,
            old_len,
            new_len,
        },
        MediaDiffComputed::Unavailable => MediaDiffOutcome::Unavailable,
        MediaDiffComputed::Ready { .. } => unreachable!("controlled compute does not decode"),
    })
}

#[test]
fn media_worker_generation_want_and_coalescing_table() {
    for case in [
        "old_first",
        "old_last",
        "invalidate",
        "path",
        "page",
        "raster",
        "cache",
        "coalesce",
    ] {
        let (tx, rx) = mpsc::channel();
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let release_rx = Mutex::new(release_rx);
        let mut worker = MediaDiffWorker::with_compute(Arc::new(move |req| {
            entered_tx
                .send((req.path.clone(), req.page, req.raster_px))
                .unwrap();
            release_rx.lock().unwrap().recv().unwrap();
            MediaDiffComputed::Summary {
                base: MediaBase::Head,
                same_bytes: false,
                old_len: Some(req.gen),
                new_len: Some(req.page.into()),
            }
        }));
        worker.attach(tx);
        let a = want("a", 1, (80, 60));
        kick(&mut worker, a.clone());
        assert_eq!(entered_rx.recv_timeout(Duration::from_secs(10)).unwrap(), a);
        assert!(!worker.needs(&a.0, a.1, a.2));
        assert!(worker.needs(std::path::Path::new("wrong"), a.1, a.2));
        assert!(worker.needs(&a.0, a.1 + 1, a.2));
        assert!(worker.needs(&a.0, a.1, (81, 60)));
        if case == "invalidate" {
            worker.invalidate();
        }
        if case.starts_with("old_") || case == "coalesce" {
            kick(&mut worker, want("discarded", 2, (90, 70)));
            kick(&mut worker, want("b", 3, (100, 80)));
            assert!(
                entered_rx.try_recv().is_err(),
                "only one computation may run"
            );
            assert_eq!(worker.queued(), &Some(want("b", 3, (100, 80))));
        }
        release_tx.send(()).unwrap();
        let result = rx.recv_timeout(Duration::from_secs(10)).unwrap();
        if case == "invalidate" {
            assert!(!apply(&mut worker, result));
            assert!(worker.landed_for(&a.0, a.1, a.2).is_none());
        } else if case.starts_with("old_") || case == "coalesce" {
            let stale = MediaDiffResult {
                gen: result.gen,
                path: result.path.clone(),
                page: result.page,
                raster_px: result.raster_px,
                computed: MediaDiffComputed::Unavailable,
            };
            assert!(
                !apply(&mut worker, result),
                "superseded decode is cancelled"
            );
            let next = worker.take_queued().unwrap();
            assert_eq!(next, want("b", 3, (100, 80)));
            assert!(worker.dispatch(req(next.clone())).is_none());
            assert_eq!(
                entered_rx.recv_timeout(Duration::from_secs(10)).unwrap(),
                next
            );
            if case == "old_first" {
                assert!(!apply(&mut worker, stale));
                assert!(
                    !worker.busy(),
                    "even a stale result frees the slot before validation"
                );
                assert!(worker.needs(&next.0, next.1, next.2));
            }
            release_tx.send(()).unwrap();
            assert!(apply(
                &mut worker,
                rx.recv_timeout(Duration::from_secs(10)).unwrap()
            ));
            if case != "old_first" {
                assert!(!apply(&mut worker, stale_result(&a)));
            }
            assert!(worker.take_queued().is_none());
            assert!(worker.landed_for(&next.0, next.1, next.2).is_some());
            assert!(entered_rx.try_recv().is_err());
        } else {
            if matches!(case, "path" | "page" | "raster") {
                let mut wrong = MediaDiffResult {
                    gen: result.gen,
                    path: result.path.clone(),
                    page: result.page,
                    raster_px: result.raster_px,
                    computed: MediaDiffComputed::Unavailable,
                };
                match case {
                    "path" => wrong.path = PathBuf::from("wrong"),
                    "page" => wrong.page += 1,
                    "raster" => wrong.raster_px.0 += 1,
                    _ => unreachable!(),
                }
                assert!(
                    apply(&mut worker, wrong),
                    "same-generation {case} result must land"
                );
                assert!(worker.landed_for(&a.0, a.1, a.2).is_none(), "{case}");
                assert!(worker.needs(&a.0, a.1, a.2), "{case}");
            }
            assert!(apply(&mut worker, result));
            assert!(worker
                .landed_for(std::path::Path::new("wrong"), a.1, a.2)
                .is_none());
            assert!(worker.landed_for(&a.0, a.1 + 1, a.2).is_none());
            assert!(worker.landed_for(&a.0, a.1, (81, 60)).is_none());
            assert!(!worker.needs(&a.0, a.1, a.2));
            assert!(worker.landed_for(&a.0, a.1, a.2).is_some());
            assert!(entered_rx.try_recv().is_err());
        }
    }
}

fn stale_result(want: &MediaDiffWant) -> MediaDiffResult {
    MediaDiffResult {
        gen: 1,
        path: want.0.clone(),
        page: want.1,
        raster_px: want.2,
        computed: MediaDiffComputed::Unavailable,
    }
}

struct HeldMedia {
    worker: MediaDiffWorker,
    results: mpsc::Receiver<MediaDiffResult>,
    release: mpsc::Sender<()>,
}

fn held_media() -> HeldMedia {
    let (tx, results) = mpsc::channel();
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release, release_rx) = mpsc::channel();
    let release_rx = Mutex::new(release_rx);
    let mut worker = MediaDiffWorker::with_compute(Arc::new(move |req| {
        entered_tx.send(()).unwrap();
        release_rx.lock().unwrap().recv().unwrap();
        MediaDiffComputed::Summary {
            base: MediaBase::Head,
            same_bytes: false,
            old_len: Some(req.gen),
            new_len: None,
        }
    }));
    worker.attach(tx);
    kick(&mut worker, want("held", 1, (80, 60)));
    entered_rx.recv_timeout(Duration::from_secs(10)).unwrap();
    HeldMedia {
        worker,
        results,
        release,
    }
}

#[test]
fn media_invalidate_rejects_old_generation_without_cancellation() {
    let mut held = held_media();
    held.worker.invalidate();
    // Model a computation that finishes without cancellation having been observed.
    held.worker
        .cancel_for_test()
        .store(false, std::sync::atomic::Ordering::SeqCst);
    held.release.send(()).unwrap();
    let result = held.results.recv_timeout(Duration::from_secs(10)).unwrap();
    assert!(!apply(&mut held.worker, result));
    assert!(!held.worker.busy());
    assert!(held
        .worker
        .landed_for(std::path::Path::new("held"), 1, (80, 60))
        .is_none());
    assert!(held.worker.needs(std::path::Path::new("held"), 1, (80, 60)));
}

#[test]
fn media_invalidate_rejects_cancelled_result_at_current_generation() {
    let mut held = held_media();
    held.worker.invalidate();
    held.release.send(()).unwrap();
    let mut result = held.results.recv_timeout(Duration::from_secs(10)).unwrap();
    // Isolate cancellation: generation alone cannot reject this result.
    result.gen = *held.worker.generation_for_test();
    assert!(!apply(&mut held.worker, result));
    assert!(!held.worker.busy());
    assert!(held
        .worker
        .landed_for(std::path::Path::new("held"), 1, (80, 60))
        .is_none());
    assert!(held.worker.needs(std::path::Path::new("held"), 1, (80, 60)));
}
