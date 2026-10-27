//! The signal thread of `term::install_exit_signals`, in a test binary of its own: it installs
//! process-wide handlers for SIGTERM / SIGHUP / SIGINT. Every hook here answers `None` (ignore),
//! so the process is never ended by what it raises.

#![cfg(unix)]

use std::sync::mpsc::channel;
use std::time::Duration;

use konoma_host_core::term::{install_exit_signals, ForegroundChildGuard};
use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM};

#[test]
fn the_hook_gets_each_signal_and_whether_a_foreground_child_owns_the_terminal() {
    let (tx, rx) = channel::<(i32, bool)>();
    install_exit_signals(move |sig, running| {
        let _ = tx.send((sig, running));
        None
    });
    for sig in [SIGTERM, SIGHUP, SIGINT] {
        signal_hook::low_level::raise(sig).unwrap();
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(10)).unwrap(),
            (sig, false)
        );
    }
    // While a child owns the terminal the hook is told so (it decides to ignore SIGINT).
    let _child = ForegroundChildGuard::new();
    signal_hook::low_level::raise(SIGINT).unwrap();
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(10)).unwrap(),
        (SIGINT, true)
    );
}
