//! Which process this is: the application itself, or something else (a test process).
//!
//! The `test-hooks` feature of the workspace crates keeps their `cfg(test)` seams compiled in when
//! another package's tests call into them. Cargo unifies features within one build, so the very same
//! `cargo test` run also builds the `konoma` binary (which the integration tests start) with those
//! seams compiled in. A seam that **changes behaviour** (the exit-time temp-dir cleanup, the fixed
//! date and locale of the ODP reader, the `LC_ALL=C` of git children) must therefore not act in the
//! application: it acts only when it is compiled in **and** `!is_app_main()`.
//!
//! The binary calls [`mark_app_main`] first thing in `main()` (before the drawing-child branch).
//! Seams that only add (call counters, test-only functions, overrides that do nothing until a test
//! sets them) need no such guard.

use std::sync::atomic::{AtomicBool, Ordering};

static APP_MAIN: AtomicBool = AtomicBool::new(false);

/// Declares this process to be the application's `main` (never called by a test).
pub fn mark_app_main() {
    APP_MAIN.store(true, Ordering::SeqCst);
}

/// Whether [`mark_app_main`] was called in this process.
pub fn is_app_main() -> bool {
    APP_MAIN.load(Ordering::SeqCst)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_test_process_is_not_the_app() {
        // Nothing in a unit-test process may call `mark_app_main`.
        assert!(!is_app_main());
    }
}
