use crate::host::project::md_diff::{
    select_diff_baseline, DiffBaseline, FollowBaseline, MdDiffKind,
};

#[test]
fn baseline_rule_uses_the_follow_snapshot_or_pinned_head_only_in_valid_scope() {
    use MdDiffKind::{Gutter, Rendered};
    // Literal expectations cover every rule case for both kinds, without synthetic inputs.
    for (kind, scope, full, valid, session, dirty, head, expected) in [
        (
            Gutter,
            false,
            false,
            true,
            true,
            Some(Some(&b"start"[..])),
            Some("abc"),
            DiffBaseline::Vcs,
        ),
        (
            Gutter,
            true,
            true,
            true,
            true,
            Some(Some(&b"start"[..])),
            Some("abc"),
            DiffBaseline::Vcs,
        ),
        (
            Gutter,
            true,
            false,
            false,
            true,
            Some(Some(&b"start"[..])),
            Some("abc"),
            DiffBaseline::Vcs,
        ),
        (
            Gutter,
            true,
            false,
            true,
            false,
            None,
            None,
            DiffBaseline::Vcs,
        ),
        (
            Gutter,
            true,
            false,
            true,
            true,
            Some(Some(&b"start"[..])),
            Some("abc"),
            DiffBaseline::Vcs,
        ),
        (
            Gutter,
            true,
            false,
            true,
            true,
            Some(None),
            Some("abc"),
            DiffBaseline::Vcs,
        ),
        (
            Gutter,
            true,
            false,
            true,
            true,
            None,
            Some("abc"),
            DiffBaseline::Vcs,
        ),
        (
            Gutter,
            true,
            false,
            true,
            true,
            None,
            None,
            DiffBaseline::Vcs,
        ),
        (
            Rendered,
            false,
            false,
            true,
            true,
            Some(Some(&b"start"[..])),
            Some("abc"),
            DiffBaseline::Vcs,
        ),
        (
            Rendered,
            true,
            true,
            true,
            true,
            Some(Some(&b"start"[..])),
            Some("abc"),
            DiffBaseline::Vcs,
        ),
        (
            Rendered,
            true,
            false,
            false,
            true,
            Some(Some(&b"start"[..])),
            Some("abc"),
            DiffBaseline::Vcs,
        ),
        (
            Rendered,
            true,
            false,
            true,
            false,
            None,
            None,
            DiffBaseline::Empty,
        ),
        (
            Rendered,
            true,
            false,
            true,
            true,
            Some(Some(&b"start"[..])),
            Some("abc"),
            DiffBaseline::FollowSnapshot(b"start".to_vec()),
        ),
        (
            Rendered,
            true,
            false,
            true,
            true,
            Some(None),
            Some("abc"),
            DiffBaseline::Empty,
        ),
        (
            Rendered,
            true,
            false,
            true,
            true,
            None,
            Some("abc"),
            DiffBaseline::FollowHead { sha: "abc".into() },
        ),
        (
            Rendered,
            true,
            false,
            true,
            true,
            None,
            None,
            DiffBaseline::Empty,
        ),
    ] {
        let got = select_diff_baseline(kind, scope, full, valid, || {
            session.then_some(FollowBaseline { dirty, head })
        });
        assert_eq!(
            got, expected,
            "{kind:?}/{scope}/{full}/{valid}/{session}/{dirty:?}/{head:?}"
        );
    }
}

use super::md_diff::{MdDiffOutcome, MdDiffRequest, MdDiffWorker};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

fn request(path: &str) -> MdDiffRequest {
    MdDiffRequest {
        gen: 0,
        path: path.into(),
        root: "fixture".into(),
        kind: MdDiffKind::Rendered,
        baseline: DiffBaseline::Empty,
        md_frontmatter: true,
        md_footnotes: true,
        md_inline_html: true,
    }
}

#[test]
fn markdown_worker_generation_and_want_table() {
    for case in [
        "old_first",
        "old_last",
        "invalidate",
        "path",
        "kind",
        "cache",
    ] {
        let (tx, rx) = mpsc::channel();
        let (entered_tx, entered_rx) = mpsc::channel();
        let (a_tx, a_rx) = mpsc::channel();
        let (b_tx, b_rx) = mpsc::channel();
        let a_rx = Mutex::new(a_rx);
        let b_rx = Mutex::new(b_rx);
        let mut worker = MdDiffWorker::with_compute(Arc::new(move |req| {
            entered_tx.send(req.path.clone()).unwrap();
            let latch = if req.path.to_str() == Some("a") {
                &a_rx
            } else {
                &b_rx
            };
            latch.lock().unwrap().recv().unwrap();
            MdDiffOutcome::Ready {
                old_pre: String::new(),
                new_pre: req.path.display().to_string(),
                ops: vec![],
                any_change: true,
                front_matter_only: false,
            }
        }));
        worker.attach(tx);
        assert!(worker.kick(request("a")).is_none());
        assert_eq!(
            entered_rx.recv_timeout(Duration::from_secs(10)).unwrap(),
            std::path::PathBuf::from("a")
        );
        assert!(!worker.needs(std::path::Path::new("a"), MdDiffKind::Rendered));
        assert!(worker.needs(std::path::Path::new("wrong"), MdDiffKind::Rendered));
        assert!(worker.needs(std::path::Path::new("a"), MdDiffKind::Gutter));
        if case.starts_with("old_") {
            assert!(worker.kick(request("b")).is_none());
            assert_eq!(
                entered_rx.recv_timeout(Duration::from_secs(10)).unwrap(),
                std::path::PathBuf::from("b")
            );
            if case == "old_first" {
                a_tx.send(()).unwrap();
                assert!(!worker.apply(rx.recv_timeout(Duration::from_secs(10)).unwrap()));
                assert!(worker.pending_for(std::path::Path::new("b")));
                b_tx.send(()).unwrap();
                assert!(worker.apply(rx.recv_timeout(Duration::from_secs(10)).unwrap()));
            } else {
                b_tx.send(()).unwrap();
                assert!(worker.apply(rx.recv_timeout(Duration::from_secs(10)).unwrap()));
                a_tx.send(()).unwrap();
                assert!(!worker.apply(rx.recv_timeout(Duration::from_secs(10)).unwrap()));
            }
            assert!(
                matches!(worker.landed_for(std::path::Path::new("b"), MdDiffKind::Rendered), Some(MdDiffOutcome::Ready { new_pre, .. }) if new_pre == "b")
            );
        } else {
            if case == "invalidate" {
                worker.invalidate();
            }
            a_tx.send(()).unwrap();
            let res = rx.recv_timeout(Duration::from_secs(10)).unwrap();
            if case == "invalidate" {
                assert!(!worker.apply(res));
                assert!(worker
                    .landed_for(std::path::Path::new("a"), MdDiffKind::Rendered)
                    .is_none());
            } else {
                if case == "path" || case == "kind" {
                    let mut wrong = res.clone();
                    if case == "path" {
                        wrong.path = "wrong".into();
                    } else {
                        wrong.kind = MdDiffKind::Gutter;
                    }
                    assert!(
                        worker.apply(wrong),
                        "same-generation {case} result must land"
                    );
                    assert!(
                        worker
                            .landed_for(std::path::Path::new("a"), MdDiffKind::Rendered)
                            .is_none(),
                        "{case}"
                    );
                    assert!(
                        worker.needs(std::path::Path::new("a"), MdDiffKind::Rendered),
                        "{case}"
                    );
                }
                assert!(worker.apply(res));
                assert!(worker
                    .landed_for(std::path::Path::new("wrong"), MdDiffKind::Rendered)
                    .is_none());
                assert!(worker
                    .landed_for(std::path::Path::new("a"), MdDiffKind::Gutter)
                    .is_none());
                assert!(!worker.needs(std::path::Path::new("a"), MdDiffKind::Rendered));
                assert!(worker
                    .landed_for(std::path::Path::new("a"), MdDiffKind::Rendered)
                    .is_some());
                assert!(
                    entered_rx.try_recv().is_err(),
                    "cached want must not recompute"
                );
            }
        }
    }
}
