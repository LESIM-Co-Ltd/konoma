//! Catching panics without printing them: the safety net of every background worker.
//!
//! Moved unchanged from `src/preview/markdown.rs`. The suppression flag is one thread-local in
//! this crate, shared by every caller in the process (the host re-exports these functions rather
//! than copying them), which is what lets the host's panic hook ask `panic_is_caught_here`.

thread_local! {
    /// This thread is inside a `silence_panics` section (its expected panics are caught and
    /// must not print). Thread-local so other threads' panics keep their messages.
    static PANIC_SILENCED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Run `f`, catching any panic and suppressing its message on **this** thread — returns
/// `None` on panic. The shared safety net for background workers (media / image decode /
/// fence render / encode): a worker that panics without reporting back would latch its
/// in-flight flag on, keeping `md_images_loading()`/`media_loading` true forever (busy
/// spinner + 16ms polling — idle CPU 0% broken). Wrapping the job here guarantees a result
/// is always produced so the flag clears and the render side degrades (principle #3).
pub fn catch_silent<T>(f: impl FnOnce() -> T) -> Option<T> {
    silence_panics(|| std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).ok())
}

/// Whether this thread is inside a `catch_silent` section (a panic here is expected and caught).
pub fn panic_is_caught_here() -> bool {
    PANIC_SILENCED.with(|c| c.get())
}

/// `catch_silent(f).unwrap_or_else(fallback)` — run `f`, and on a caught panic run `fallback`
/// instead, so the caller always has *a* value to report back rather than nothing at all. Several
/// background workers (`App::spawn_or_sync_statuses`/`spawn_or_sync_ignored` in
/// `app/bookmark_actions.rs`, `App::fence_sharpen_if_needed` in `app/md_media.rs`) used to guard
/// their job with `catch_silent` but only send a result on the `Some` branch — so a caught panic
/// sent *nothing* on the channel, and the caller's "in flight" flag (`git_status_pending`/
/// `git_ignored_pending`/`reraster_inflight`) stayed latched forever (a spinner that never stops).
/// Naming this pattern (rather than leaving `catch_silent(..).unwrap_or_else(..)` duplicated at
/// each call site) also makes the "always send a result" contract impossible to accidentally
/// half-implement again — every caller's `tx.send(..)` sits right after this call with nothing
/// gating it. Extracted so the panic/fallback behavior is directly testable (a real panic — e.g. in
/// syntect, resvg, or a `git status` scan — can't be induced on demand from a test; see this
/// module's tests below).
pub fn compute_or_fallback<T>(f: impl FnOnce() -> T, fallback: impl FnOnce() -> T) -> T {
    catch_silent(f).unwrap_or_else(fallback)
}

/// Run `f` with panic **messages** suppressed on this thread (the caller still catches the
/// unwind itself). A composite hook is installed process-wide **once** and consults the
/// thread-local flag — the previous take_hook/set_hook swap around every call raced when
/// several diagram renders ran concurrently (fence workers / media thread / warm-up): an
/// interleaved restore could leave the silent hook installed forever, permanently losing
/// panic messages for the whole process (a diagnostics hazard, not a crash).
pub fn silence_panics<T>(f: impl FnOnce() -> T) -> T {
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(|| {
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if !PANIC_SILENCED.with(|c| c.get()) {
                prev(info);
            }
        }));
    });
    PANIC_SILENCED.with(|c| c.set(true));
    let r = f();
    PANIC_SILENCED.with(|c| c.set(false));
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catch_silent_returns_none_on_panic_and_some_on_success() {
        // The worker safety net: a panic yields None (the caller sends a failure result to clear the
        // inflight state); a normal value comes back as Some, unchanged. The suppression flag never lingers on either path.
        assert_eq!(catch_silent(|| 42), Some(42));
        let paniced: Option<i32> = catch_silent(|| panic!("worker blew up"));
        assert_eq!(paniced, None);
        PANIC_SILENCED.with(|c| assert!(!c.get(), "panic 経路でも抑制フラグが残らない"));
        // Confirm a subsequent normal panic message can still be emitted (the hook isn't stuck silencing).
        assert_eq!(catch_silent(|| "ok"), Some("ok"));
    }

    /// D2 (2026-08-05): `compute_or_fallback` is what `spawn_or_sync_statuses`/`spawn_or_sync_ignored`
    /// (`app/bookmark_actions.rs`) and `fence_sharpen_if_needed` (`app/md_media.rs`) now call instead
    /// of the old `if let Some(res) = catch_silent(..) { tx.send(res) }` shape, which sent *nothing*
    /// on a caught panic. This is the part of that fix that's directly testable with a real panic
    /// (the git-scan/resvg call each site wraps can't be made to panic on demand): on success it
    /// returns `f`'s value untouched; on a panic it returns `fallback()`'s value instead of
    /// propagating — so a caller placing an unconditional `tx.send(..)` right after this call, as all
    /// three sites now do, is guaranteed to always send *something*.
    #[test]
    fn compute_or_fallback_returns_fs_value_on_success_and_fallbacks_value_on_panic() {
        assert_eq!(
            compute_or_fallback(|| 42, || 0),
            42,
            "成功時は f の値そのまま"
        );
        let v = compute_or_fallback(|| -> i32 { panic!("simulated worker panic") }, || 7);
        assert_eq!(
            v, 7,
            "パニック時は fallback の値(=何も送らないよりは安全な既知の失敗状態)"
        );
        // The suppression flag doesn't linger (same guarantee catch_silent itself gives).
        PANIC_SILENCED.with(|c| assert!(!c.get(), "パニック経路でも抑制フラグが残らない"));
    }
}
