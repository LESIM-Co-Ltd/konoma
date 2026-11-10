//! Control-plane closure must not wait for the background reclamation of a replaced root.
use super::*;
use konoma_plugin::host::project::ProjectReply;

struct BlockReaper(Option<Gate>);
impl Drop for BlockReaper {
    fn drop(&mut self) {
        self.0.take().unwrap().wait();
    }
}
struct ReleaseOnDrop(Option<mpsc::Sender<()>>);
impl Drop for ReleaseOnDrop {
    fn drop(&mut self) {
        if let Some(tx) = self.0.take() {
            let _ = tx.send(());
        }
    }
}

#[test]
fn replaced_root_disconnects_delivery_before_background_reclamation() {
    let mut f = Fixture::new();
    f.run(&[]);
    let origin = f.origin();
    let runtime = f.prepared.runtimes.get_mut(&origin.project).unwrap();
    let (delivery, receiver) = mpsc::sync_channel(1);
    // This sender observes receiver lifetime directly, without a forwarding-thread race.
    runtime.rx = receiver;
    let (gate, entered, release) = latch();
    let _release = ReleaseOnDrop(Some(release));
    konoma_base::discard::discard_in_background(BlockReaper(Some(gate)));
    entered.recv_timeout(WAIT).unwrap();
    f.inject(
        origin,
        Outcome::SwitchRoot {
            root: f.b.clone(),
            message: None,
        },
    );
    f.run(&[]);
    assert_eq!(f.prepared.open_dir, f.b);
    let reply = ProjectReply {
        watch_paths: None,
        views: vec![],
        enabled: Default::default(),
        chip: None,
        chip_error: None,
        effect: None,
        error: None,
        act_failed: false,
        contexts: Default::default(),
    };
    assert!(matches!(delivery.try_send(reply), Err(mpsc::TrySendError::Disconnected(_))),
        "root replacement must disconnect the old delivery receiver before it returns, even while the reaper is held");
}
