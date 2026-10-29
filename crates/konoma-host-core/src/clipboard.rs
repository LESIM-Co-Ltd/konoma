//! The system clipboard as text: [`set_text`] (write) and [`get_text`] (read), shared by konoma and
//! the plugin SDK's small host.
//!
//! Test builds (this crate's own tests, and any package that enables the `test-hooks` feature)
//! never touch the real clipboard: both calls go through a thread-local in-memory sink
//! ([`set_test_clipboard`], [`get_test_clipboard`], [`clear_test_clipboard`]). Running the suite
//! used to overwrite the developer's real clipboard on every copy key; with the arboard body
//! compiled out of such builds that cannot happen by construction.
//!
//! The sink is thread-local: `cargo test` reuses a pool of worker threads but never runs two tests
//! at once on one thread, so tests cannot leak into each other concurrently. It can still carry a
//! previous test's value to a *later* test on the same thread, so a test that asserts an empty
//! clipboard must call [`clear_test_clipboard`] first.

use anyhow::Result;

/// Write `text` to the clipboard. Err where the clipboard is unavailable (the caller shows a flash).
#[cfg(not(any(test, feature = "test-hooks")))]
pub fn set_text(text: &str) -> Result<()> {
    #[cfg(target_os = "linux")]
    {
        // On Linux the clipboard content is owned by the process holding the X11/Wayland
        // selection: dropping the Clipboard releases it and the copied text is lost unless a
        // clipboard manager grabbed it. Probe on the calling thread first so the caller still gets
        // an Err on headless/unavailable environments, then hold the selection in a detached
        // thread via SetExtLinux::wait() so paste works. wait() returns (and the thread exits) as
        // soon as another app, or the next copy, takes ownership, so at most one holder thread is
        // alive at a time. Not unit-tested (needs an X11/Wayland display).
        use arboard::SetExtLinux;
        let _ = arboard::Clipboard::new()?;
        let owned = text.to_string();
        std::thread::spawn(move || {
            if let Ok(mut cb) = arboard::Clipboard::new() {
                let _ = cb.set().wait().text(owned);
            }
        });
        Ok(())
    }
    #[cfg(not(target_os = "linux"))]
    {
        let mut cb = arboard::Clipboard::new()?;
        cb.set_text(text.to_string())?;
        Ok(())
    }
}

/// Test build: write to the in-memory sink instead of the system clipboard.
#[cfg(any(test, feature = "test-hooks"))]
pub fn set_text(text: &str) -> Result<()> {
    set_test_clipboard(text);
    Ok(())
}

/// Read the system clipboard as text. Err where the clipboard is unavailable.
#[cfg(not(any(test, feature = "test-hooks")))]
pub fn get_text() -> Result<String> {
    let mut cb = arboard::Clipboard::new()?;
    Ok(cb.get_text()?)
}

/// Test build: read the in-memory sink; Err ("empty") when nothing was set.
#[cfg(any(test, feature = "test-hooks"))]
pub fn get_text() -> Result<String> {
    get_test_clipboard().ok_or_else(|| anyhow::anyhow!("test clipboard is empty"))
}

#[cfg(any(test, feature = "test-hooks"))]
thread_local! {
    static CLIPBOARD: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

/// Test-only: put `text` in the sink (what [`set_text`] does in a test build).
#[cfg(any(test, feature = "test-hooks"))]
pub fn set_test_clipboard(text: &str) {
    CLIPBOARD.with(|c| *c.borrow_mut() = Some(text.to_string()));
}

/// Test-only: the sink's current value.
#[cfg(any(test, feature = "test-hooks"))]
pub fn get_test_clipboard() -> Option<String> {
    CLIPBOARD.with(|c| c.borrow().clone())
}

/// Test-only: empty the sink (mirrors an environment where the clipboard is unavailable).
#[cfg(any(test, feature = "test-hooks"))]
pub fn clear_test_clipboard() {
    CLIPBOARD.with(|c| *c.borrow_mut() = None);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_written_text_reads_back() {
        clear_test_clipboard();
        set_text("hello クリップ").unwrap();
        assert_eq!(get_text().unwrap(), "hello クリップ");
        assert_eq!(get_test_clipboard().as_deref(), Some("hello クリップ"));
    }

    #[test]
    fn an_empty_sink_is_an_error_on_read() {
        clear_test_clipboard();
        assert!(get_text().is_err());
        assert_eq!(get_test_clipboard(), None);
    }

    #[test]
    fn the_next_write_replaces_the_previous_one() {
        set_text("one").unwrap();
        set_text("two").unwrap();
        assert_eq!(get_text().unwrap(), "two");
    }
}
