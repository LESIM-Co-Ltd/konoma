//! A frame fence must publish its reply before allowing the loop to consume it.
use super::*;

#[test]
fn a_reply_cannot_be_consumed_before_its_frame_fence() {
    let (_dir, mut prepared, events, _log) = setup("[ui]\nlang='en'\n");
    let runtime = prepared.runtimes.values_mut().next().unwrap();
    assert!(runtime
        .tx
        .try_send(konoma_plugin::host::project::ProjectJob::Chip)
        .is_ok());
    let publish = events
        .replies
        .recv_timeout(Duration::from_secs(10))
        .expect("the owner produced its chip reply");
    assert!(
        matches!(runtime.rx.try_recv(), Err(mpsc::TryRecvError::Empty)),
        "the loop cannot consume a reply before its frame fence"
    );
    publish();
    assert!(
        runtime.rx.try_recv().is_ok(),
        "the frame fence publishes its own unconsumed reply"
    );
    assert!(
        matches!(events.replies.try_recv(), Err(mpsc::TryRecvError::Empty)),
        "consuming a fenced reply leaves no stale notification"
    );
}
