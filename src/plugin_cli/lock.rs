//! One `konoma plugin` that changes things at a time.
//!
//! `install`, `update`, `remove` and `clean` write the program, its record, `Cargo.lock`,
//! `plugins.toml` and the work directory; two of them at once would overwrite each other's
//! generated project and lists. They hold an exclusive lock on a file in the data directory for as
//! long as they change anything. `list` only reads and takes none.
//!
//! A second command **waits** for the first (and says so), as Cargo does for its own locks. A
//! refusal would only make the user type the command again once the first is done, and a build
//! takes minutes; waiting does what was asked. Ctrl-C ends the wait: the interrupt handlers of
//! the build are installed after the lock is held, so until then the signal has its default effect.
//!
//! The lock belongs to the open file, so it is released when the process ends, however it ends,
//! and the file descriptor is not inherited by the processes it starts (Rust opens with
//! close-on-exec), so a cargo that outlives its parent cannot keep it.

use std::fs::{File, TryLockError};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// The lock file of a data directory.
pub fn lock_path(data_dir: &Path) -> PathBuf {
    data_dir.join("plugin.lock")
}

/// Holds the lock until dropped.
#[derive(Debug)]
pub struct PluginLock {
    _file: File,
}

/// Take the lock, waiting for another `konoma plugin` that holds it (said on `notice`).
pub fn acquire(data_dir: &Path, notice: &mut dyn Write) -> Result<PluginLock> {
    std::fs::create_dir_all(data_dir)
        .with_context(|| format!("cannot create {}", data_dir.display()))?;
    let path = lock_path(data_dir);
    let file = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .with_context(|| format!("cannot open {}", path.display()))?;
    match file.try_lock() {
        Ok(()) => {}
        Err(TryLockError::WouldBlock) => {
            let _ = writeln!(
                notice,
                "Another `konoma plugin` is running; waiting for it to finish (Ctrl-C to give up)..."
            );
            file.lock()
                .with_context(|| format!("cannot lock {}", path.display()))?;
        }
        Err(TryLockError::Error(e)) => {
            return Err(e).with_context(|| format!("cannot lock {}", path.display()));
        }
    }
    Ok(PluginLock { _file: file })
}

#[cfg(test)]
mod tests {
    use super::*;
    use konoma_test_support::unique_tmp;

    #[test]
    fn a_second_holder_waits_for_the_first_and_says_so() {
        let d = unique_tmp("plugin-lock");
        std::fs::create_dir_all(&d).unwrap();
        let first = acquire(&d, &mut Vec::new()).unwrap();
        let dir = d.to_path_buf();
        let (tx, rx) = std::sync::mpsc::channel();
        let (attempt_tx, attempt_rx) = std::sync::mpsc::sync_channel(0);
        struct Notice {
            text: Vec<u8>,
            attempted: Option<std::sync::mpsc::SyncSender<()>>,
        }
        impl Write for Notice {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.text.extend_from_slice(bytes);
                if self.text.contains(&b'\n') {
                    if let Some(tx) = self.attempted.take() {
                        tx.send(()).unwrap();
                    }
                }
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let waiter = std::thread::spawn(move || {
            let mut said = Notice {
                text: Vec::new(),
                attempted: Some(attempt_tx),
            };
            let got = acquire(&dir, &mut said).is_ok();
            tx.send(String::from_utf8(said.text).unwrap()).unwrap();
            got
        });
        // A waiting notice is only written after try_lock observed the first holder.
        attempt_rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .unwrap();
        drop(first);
        let said = rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
        assert!(said.contains("waiting"), "{said}");
        assert!(waiter.join().unwrap());
    }

    #[test]
    fn a_free_lock_is_taken_silently_and_again_after_release() {
        let d = unique_tmp("plugin-lock-free");
        let mut said = Vec::new();
        drop(acquire(&d, &mut said).unwrap());
        drop(acquire(&d, &mut said).unwrap());
        assert!(said.is_empty());
        assert!(lock_path(&d).is_file());
    }
}
