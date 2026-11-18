//! A running project WRITE act counts as a running git write everywhere the old hub's did: the
//! busy label, the indicator forced on past `ui.busy_indicator = false`, and the deferral of the
//! watcher bursts the write itself causes (flushed once, after the act's reply). A running READ
//! never counts. The test plugin's `w` declares `writes` and `x` (capture) is a read, standing in
//! for the vcs plugin's commit / checkout and refresh. Order comes from latches, never the clock.

use super::*;
use crate::app::{DeferredFsBurst, FsBurstKinds};
use crate::i18n::Msg;

/// A project open on the first screen with `key`'s act started and held inside the plugin.
fn held_act(name: &str, key: char) -> (Fixture, Arc<Latch>) {
    let hold = Arc::new(Latch::default());
    let started = Arc::new(Latch::default());
    let mut f = fixture(
        name,
        false,
        1,
        None,
        Some(hold.clone()),
        Some(started.clone()),
    );
    open(&mut f);
    f.sim.key(key);
    started.wait();
    assert!(started.is_open(), "the act must be inside the plugin");
    assert!(f.sim.app.project_jobs_pending(), "{key}: the act runs");
    (f, hold)
}

fn structural() -> FsBurstKinds {
    FsBurstKinds {
        ignore_rules_changed: false,
        structural: true,
    }
}

fn rules() -> FsBurstKinds {
    FsBurstKinds {
        ignore_rules_changed: true,
        structural: false,
    }
}

#[test]
fn e2e_busy_label_lists_a_held_project_write_and_drops_it_after() {
    let (mut f, hold) = held_act("write_busy_label", 'w');
    assert!(f.sim.app.project_write_running());
    assert!(f.sim.app.git_write_running());
    assert!(
        f.sim.app.busy_jobs().contains(&Msg::BusyGitOp),
        "{:?}",
        f.sim.app.busy_jobs()
    );
    // The write is the first label, ahead of any scan label.
    assert_eq!(f.sim.app.busy_jobs().first(), Some(&Msg::BusyGitOp));
    hold.open();
    drain(&mut f.sim);
    assert!(!f.sim.app.git_write_running());
    assert!(
        !f.sim.app.busy_jobs().contains(&Msg::BusyGitOp),
        "{:?}",
        f.sim.app.busy_jobs()
    );
}

#[test]
fn e2e_busy_indicator_is_forced_during_a_held_project_write_even_when_off() {
    let (mut f, hold) = held_act("write_busy_forced", 'w');
    f.sim.app.cfg.ui.busy_indicator = false;
    assert!(
        f.sim.app.busy_indicator_active(),
        "a commit stuck in a hook must show with the indicator off"
    );
    f.sim.app.cfg.ui.busy_indicator = true;
    assert!(f.sim.app.busy_indicator_active());
    hold.open();
    drain(&mut f.sim);
    f.sim.app.cfg.ui.busy_indicator = false;
    assert!(
        !f.sim.app.busy_indicator_active(),
        "idle again: nothing forces the indicator"
    );
}

#[test]
fn e2e_a_held_project_read_is_neither_listed_nor_forced_nor_deferred() {
    let (mut f, hold) = held_act("write_busy_read", 'x');
    assert!(!f.sim.app.project_write_running(), "capture is a read");
    assert!(!f.sim.app.git_write_running());
    assert!(
        !f.sim.app.busy_jobs().contains(&Msg::BusyGitOp),
        "{:?}",
        f.sim.app.busy_jobs()
    );
    f.sim.app.cfg.ui.busy_indicator = false;
    assert!(
        !f.sim.app.busy_indicator_active(),
        "a read never forces the indicator"
    );
    assert!(
        !f.sim.app.should_defer_fs_events(),
        "a read never holds watcher bursts back"
    );
    hold.open();
    drain(&mut f.sim);
}

#[test]
fn e2e_a_view_refresh_is_neither_listed_nor_deferred() {
    // `r` re-reads the screen: the owner is busy but no act runs.
    let mut f = fixture("write_busy_refresh", false, 1, None, None, None);
    open(&mut f);
    let gate = ViewHold::new("items");
    f.log.lock().unwrap().view_hold = Some(gate.clone());
    f.sim.key('r');
    gate.entered.wait();
    assert!(f.sim.app.project_jobs_pending(), "the refresh is running");
    assert!(!f.sim.app.git_write_running());
    assert!(!f.sim.app.busy_jobs().contains(&Msg::BusyGitOp));
    assert!(!f.sim.app.should_defer_fs_events());
    gate.release.open();
    drain(&mut f.sim);
}

#[test]
fn e2e_watcher_burst_is_held_during_a_project_write_and_applied_once_after_the_reply() {
    let (mut f, hold) = held_act("write_defer_flush", 'w');
    let mut deferred = DeferredFsBurst::default();

    // Run-loop iteration while the write runs: the burst is held, nothing is released.
    let defer = f.sim.app.should_defer_fs_events();
    assert!(defer, "a held write defers the watcher bursts");
    deferred.hold(structural());
    deferred.hold(rules());
    assert!(deferred.is_pending());
    assert_eq!(deferred.release(defer), None, "nothing leaves mid-write");
    deferred.hold(structural());
    assert_eq!(deferred.release(f.sim.app.should_defer_fs_events()), None);

    // The act's reply lifts the deferral; the next iteration flushes the merged burst once.
    hold.open();
    drain(&mut f.sim);
    let defer = f.sim.app.should_defer_fs_events();
    assert!(!defer, "the reply lifts the deferral");
    assert_eq!(
        deferred.release(defer),
        Some(FsBurstKinds {
            ignore_rules_changed: true,
            structural: true,
        }),
        "every held event's flags come out merged"
    );
    assert!(!deferred.is_pending());
    assert_eq!(
        deferred.release(defer),
        None,
        "the burst is applied once, not again"
    );
}

#[test]
fn e2e_watcher_burst_is_not_held_during_a_project_read() {
    let (mut f, hold) = held_act("write_defer_read", 'x');
    let mut deferred = DeferredFsBurst::default();
    let defer = f.sim.app.should_defer_fs_events();
    assert!(!defer);
    // A burst that is not deferred never reaches the holder: the loop applies it directly.
    assert_eq!(deferred.release(defer), None);
    assert!(!deferred.is_pending());
    hold.open();
    drain(&mut f.sim);
}

#[test]
fn deferred_burst_holds_until_released_merges_flags_and_releases_once() {
    let mut d = DeferredFsBurst::default();
    assert_eq!(d.release(false), None, "nothing held, nothing released");
    d.hold(structural());
    assert_eq!(d.release(true), None);
    assert!(d.is_pending());
    d.hold(rules());
    assert_eq!(
        d.release(false),
        Some(FsBurstKinds {
            ignore_rules_changed: true,
            structural: true,
        })
    );
    assert_eq!(d.release(false), None);
    // The next write starts from clean flags, not the previous burst's.
    d.hold(structural());
    assert_eq!(d.release(false), Some(structural()));
}
