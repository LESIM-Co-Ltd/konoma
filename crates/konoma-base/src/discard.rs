//! Freeing a large value off the calling thread.

/// Frees `value` on a thread of its own (one, shared, so holding a key does not start a thread
/// per press). The UI thread must not spend tens of milliseconds freeing a large workbook; if the
/// thread cannot be had the value is freed here, as it would have been.
pub fn discard_in_background<T: Send + 'static>(value: T) {
    #[cfg(any(test, feature = "test-hooks"))]
    if cfg!(test) || !crate::process::is_app_main() {
        DISCARDED.with(|d| d.borrow_mut().push(std::any::type_name::<T>()));
    }
    if let Some(tx) = reaper() {
        // A send that fails hands the value back inside the error, which is freed right here.
        if let Ok(tx) = tx.lock() {
            let _ = tx.send(Box::new(value));
            return;
        }
    }
    drop(value);
}

type Junk = Box<dyn Send>;

/// The sender of the shared reaper thread, started on first use; `None` if it could not start.
fn reaper() -> Option<&'static std::sync::Mutex<std::sync::mpsc::Sender<Junk>>> {
    use std::sync::{mpsc, Mutex, OnceLock};
    static REAPER: OnceLock<Option<Mutex<mpsc::Sender<Junk>>>> = OnceLock::new();
    REAPER
        .get_or_init(|| {
            let (tx, rx) = mpsc::channel::<Junk>();
            std::thread::Builder::new()
                .name("konoma-reaper".into())
                .spawn(move || {
                    // Dropping each item as it arrives is the whole job.
                    for junk in rx {
                        drop(junk);
                    }
                })
                .ok()
                .map(|_| Mutex::new(tx))
        })
        .as_ref()
}

/// Test-only: returns once everything handed to [`discard_in_background`] before this call has
/// been dropped. The reaper's channel is FIFO, so a marker sent now is dropped only after every
/// earlier value. Tests that judge a child's state (running, live sessions, reaped) after a
/// session was closed must call this first: the session's `Drop` runs on the reaper thread.
/// Returns at once if there is no reaper thread (the value was freed in place).
#[cfg(any(test, feature = "test-hooks"))]
pub fn flush_discards() {
    struct Marker(std::sync::mpsc::Sender<()>);
    impl Drop for Marker {
        fn drop(&mut self) {
            let _ = self.0.send(());
        }
    }
    let (tx, rx) = std::sync::mpsc::channel();
    let Some(sender) = reaper() else { return };
    let Ok(sender) = sender.lock() else { return };
    if sender.send(Box::new(Marker(tx))).is_err() {
        return;
    }
    drop(sender);
    rx.recv_timeout(std::time::Duration::from_secs(10))
        .expect("the reaper thread dropped everything discarded before the marker");
}

#[cfg(any(test, feature = "test-hooks"))]
thread_local! {
    static DISCARDED: std::cell::RefCell<Vec<&'static str>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// Test-only: forget what this thread discarded so far (see [`discarded_types`]).
#[cfg(any(test, feature = "test-hooks"))]
pub fn clear_discarded() {
    DISCARDED.with(|d| d.borrow_mut().clear());
}

/// Test-only: the type of every value handed to [`discard_in_background`] on this thread since
/// [`clear_discarded`], so a test can say that a large value was not freed on the calling (UI)
/// thread.
#[cfg(any(test, feature = "test-hooks"))]
pub fn discarded_types() -> Vec<&'static str> {
    DISCARDED.with(|d| d.borrow().clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Records which thread it was dropped on.
    struct Probe(std::sync::mpsc::Sender<std::thread::ThreadId>);

    impl Drop for Probe {
        fn drop(&mut self) {
            let _ = self.0.send(std::thread::current().id());
        }
    }

    #[test]
    fn a_discarded_value_is_dropped_on_another_thread() {
        let (tx, rx) = std::sync::mpsc::channel();
        discard_in_background(Probe(tx));
        let on = rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("the value is dropped");
        assert_ne!(on, std::thread::current().id());
    }

    #[test]
    fn many_discards_are_all_freed_by_the_one_thread() {
        let (tx, rx) = std::sync::mpsc::channel();
        for _ in 0..200 {
            discard_in_background(Probe(tx.clone()));
        }
        drop(tx);
        let ids: std::collections::HashSet<_> = rx.iter().take(200).collect();
        assert_eq!(ids.len(), 1, "one reaper thread, not a thread per value");
        assert!(!ids.contains(&std::thread::current().id()));
    }

    #[test]
    fn the_types_discarded_on_this_thread_are_recorded() {
        clear_discarded();
        discard_in_background(vec![1u8]);
        assert!(discarded_types().iter().any(|t| t.contains("Vec<u8>")));
        clear_discarded();
        assert!(discarded_types().is_empty());
    }
}
