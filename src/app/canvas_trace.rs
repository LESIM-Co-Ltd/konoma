//! TEMPORARY (feature `demo-canvas`): counters for `KONOMA_CANVAS_TRACE`, which tells a stuck
//! canvas on a real terminal from a canvas that is opened again and again. Removed with the
//! feature when stage 3 can load plugins.

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Mutex;

/// Plugin worker threads running now.
static WORKERS: AtomicUsize = AtomicUsize::new(0);
/// How many times a show was read again with the previous view kept on screen.
static REOPENS: AtomicU64 = AtomicU64::new(0);
/// The generation and the reason of the last plugin answer that was not applied.
static LAST_DISCARD: Mutex<Option<(u64, &'static str)>> = Mutex::new(None);

/// Counts a plugin worker thread as alive for as long as it is held.
pub(super) struct WorkerGuard;

impl WorkerGuard {
    pub(super) fn new() -> WorkerGuard {
        WORKERS.fetch_add(1, Ordering::SeqCst);
        WorkerGuard
    }
}

impl Drop for WorkerGuard {
    fn drop(&mut self) {
        WORKERS.fetch_sub(1, Ordering::SeqCst);
    }
}

pub(super) fn note_reopen() {
    REOPENS.fetch_add(1, Ordering::SeqCst);
}

pub(super) fn note_discard(gen: u64, reason: &'static str) {
    *LAST_DISCARD.lock().unwrap_or_else(|e| e.into_inner()) = Some((gen, reason));
}

pub(super) fn workers() -> usize {
    WORKERS.load(Ordering::SeqCst)
}

pub(super) fn reopens() -> u64 {
    REOPENS.load(Ordering::SeqCst)
}

pub(super) fn last_discard() -> Option<(u64, &'static str)> {
    *LAST_DISCARD.lock().unwrap_or_else(|e| e.into_inner())
}
