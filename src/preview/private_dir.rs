//! The per-process private temp directories (`konoma-<kind>-<pid>`) of the delegated-command,
//! PDF and video-thumbnail renderers, in one implementation.
//!
//! A world-writable temp dir (`/tmp` on Linux) plus a name another user can predict is an attack
//! surface: a directory or symlink planted at that name must never be adopted. So a directory is
//! only used when this process **created it** (`mkdir(2)` succeeded, mode `0700`), or when it is
//! one this process already chose and that still passes `dir_is_private`. An existing entry at
//! the predictable name is removed only if it is a real directory owned by us with no group/other
//! access (a stale leftover of our own); anything else makes us pick an unpredictable name
//! instead. The chosen paths are the only things exit cleanup ever removes.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Whether `meta` (from `symlink_metadata`, so a symlink is never a directory here) describes a
/// directory that only `euid` can use: a real directory, owned by `euid`, no group/other bits.
#[cfg(unix)]
pub(crate) fn dir_is_private(meta: &std::fs::Metadata, euid: u32) -> bool {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    meta.file_type().is_dir() && meta.uid() == euid && meta.permissions().mode() & 0o077 == 0
}

#[cfg(unix)]
fn current_euid() -> u32 {
    // SAFETY: `geteuid` has no preconditions and cannot fail.
    unsafe { libc::geteuid() }
}

/// An unpredictable suffix: `RandomState` is seeded from the OS per process, and the counter
/// keeps successive draws apart.
fn random_suffix() -> String {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let mut h = RandomState::new().build_hasher();
    h.write_u64(N.fetch_add(1, Ordering::Relaxed));
    h.write_u32(std::process::id());
    format!("{:016x}", h.finish())
}

#[cfg(unix)]
fn mkdir_private(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    // The mode is applied atomically by `mkdir(2)` itself: no create-then-chmod window, and no
    // chmod that could follow a symlink.
    std::fs::DirBuilder::new().mode(0o700).create(path)
}

#[cfg(not(unix))]
fn mkdir_private(path: &Path) -> io::Result<()> {
    std::fs::create_dir(path)
}

#[cfg(unix)]
fn is_ours_and_private(path: &Path) -> bool {
    std::fs::symlink_metadata(path)
        .map(|m| dir_is_private(&m, current_euid()))
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_ours_and_private(path: &Path) -> bool {
    std::fs::symlink_metadata(path)
        .map(|m| m.is_dir())
        .unwrap_or(false)
}

/// Creates `path` as a fresh private directory this process owns. Err when anything is already
/// there: an existing entry cannot be proven to be this process's own leftover (a same-named
/// directory may belong to another live konoma whose pid namespace differs but shares this
/// `/tmp`), so it is never adopted and never removed; the caller moves on to an unpredictable
/// name instead.
fn create_fresh(path: &Path) -> io::Result<()> {
    mkdir_private(path)
}

/// The private directories this process has chosen, by kind.
#[derive(Default)]
pub(crate) struct PrivateDirs {
    chosen: Vec<(String, PathBuf)>,
}

impl PrivateDirs {
    pub(crate) const fn new() -> Self {
        Self { chosen: Vec::new() }
    }

    /// The private directory of `kind` under `base`, created if it does not exist. The first
    /// call picks `konoma-<kind>-<pid>`, or an unpredictable `konoma-<kind>-<pid>-<random>` when
    /// that name is taken by something unsafe; later calls return the same path (recreating it if
    /// it was removed, and re-picking if it was replaced by something unsafe). Err only when no
    /// safe directory could be made.
    pub(crate) fn ensure(&mut self, base: &Path, kind: &str) -> io::Result<PathBuf> {
        let idx = self.chosen.iter().position(|(k, _)| k == kind);
        if let Some(i) = idx {
            let path = &self.chosen[i].1;
            if is_ours_and_private(path) {
                return Ok(path.clone());
            }
            // Removed (e.g. by exit cleanup in a long-lived test process): recreate in place
            // only if nothing is there; a replaced entry is never adopted.
            if std::fs::symlink_metadata(path).is_err() && mkdir_private(path).is_ok() {
                return Ok(path.clone());
            }
        }
        let pid = std::process::id();
        let mut candidates = vec![format!("konoma-{kind}-{pid}")];
        candidates.extend((0..16).map(|_| format!("konoma-{kind}-{pid}-{}", random_suffix())));
        for name in candidates {
            let path = base.join(name);
            if create_fresh(&path).is_ok() {
                match idx {
                    Some(i) => self.chosen[i].1 = path.clone(),
                    None => self.chosen.push((kind.to_string(), path.clone())),
                }
                return Ok(path);
            }
        }
        Err(io::Error::other(format!(
            "could not create a private {kind} temp directory under {}",
            base.display()
        )))
    }

    /// Removes the directories this registry chose, and only those.
    pub(crate) fn remove_all(&self) {
        for (_, path) in &self.chosen {
            remove_chosen(path);
        }
    }
}

/// Removes `path` only when it is still a real directory (never a symlink someone swapped in).
fn remove_chosen(path: &Path) {
    if std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_dir()) {
        let _ = std::fs::remove_dir_all(path);
    }
}

static GLOBAL: Mutex<PrivateDirs> = Mutex::new(PrivateDirs::new());

/// This process's private directory of `kind` (`cmd`, `pdf`, `vthumb`) under the system temp dir,
/// created on demand. When no safe directory can be made the returned path does not exist, so
/// every write into it fails instead of landing somewhere hostile.
pub(crate) fn private_dir(kind: &str) -> PathBuf {
    super::command::register_test_exit_cleanup();
    let mut g = GLOBAL.lock().unwrap_or_else(|e| e.into_inner());
    g.ensure(&system_temp(), kind)
        .unwrap_or_else(|_| unavailable_path(kind))
}

fn system_temp() -> PathBuf {
    std::env::temp_dir()
}

/// A path that does not exist (and has no existing parent to hijack), so writes into it fail.
fn unavailable_path(kind: &str) -> PathBuf {
    system_temp().join(format!("konoma-{kind}-unavailable-{}", random_suffix()))
}

/// Removes every private directory this process chose (and nothing else). Never creates one.
pub(crate) fn remove_all_private_dirs() {
    let g = GLOBAL.lock().unwrap_or_else(|e| e.into_inner());
    g.remove_all();
}

/// Creates `path` exclusively with owner-only permissions and writes `data`. Refuses to follow a
/// symlink at `path` and to reuse an existing file (`O_NOFOLLOW | O_EXCL`).
#[cfg(unix)]
pub(crate) fn create_private_file(path: &Path, data: &[u8]) -> io::Result<()> {
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .custom_flags(libc::O_NOFOLLOW)
        .mode(0o600)
        .open(path)?;
    f.write_all(data)
}

#[cfg(not(unix))]
pub(crate) fn create_private_file(path: &Path, data: &[u8]) -> io::Result<()> {
    std::fs::write(path, data)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::test_support::unique_tmp;
    use std::os::unix::fs::{symlink, PermissionsExt};

    fn mode(p: &Path) -> u32 {
        std::fs::symlink_metadata(p).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn fresh_directory_is_private_and_stable_across_calls() {
        let base = unique_tmp("pd_fresh");
        std::fs::create_dir_all(&base).unwrap();
        let mut d = PrivateDirs::new();
        let a = d.ensure(&base, "cmd").unwrap();
        assert_eq!(mode(&a), 0o700);
        assert_eq!(a, base.join(format!("konoma-cmd-{}", std::process::id())));
        assert_eq!(d.ensure(&base, "cmd").unwrap(), a);
        // Another kind gets its own directory.
        let b = d.ensure(&base, "pdf").unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn planted_symlink_to_a_directory_is_not_adopted() {
        let base = unique_tmp("pd_symlink");
        let victim = base.join("victim");
        std::fs::create_dir_all(&victim).unwrap();
        let pid = std::process::id();
        symlink(&victim, base.join(format!("konoma-cmd-{pid}"))).unwrap();
        let mut d = PrivateDirs::new();
        let got = d.ensure(&base, "cmd").unwrap();
        assert_ne!(got, base.join(format!("konoma-cmd-{pid}")));
        assert!(got.starts_with(&base));
        assert_eq!(mode(&got), 0o700);
        // The symlink and its target are untouched, and nothing was created in the victim.
        assert!(
            std::fs::symlink_metadata(base.join(format!("konoma-cmd-{pid}")))
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(std::fs::read_dir(&victim).unwrap().count(), 0);
    }

    #[test]
    fn planted_wide_directory_is_not_used_nor_chmodded() {
        let base = unique_tmp("pd_wide");
        std::fs::create_dir_all(&base).unwrap();
        let wide = base.join(format!("konoma-cmd-{}", std::process::id()));
        std::fs::create_dir(&wide).unwrap();
        std::fs::set_permissions(&wide, std::fs::Permissions::from_mode(0o777)).unwrap();
        std::fs::write(wide.join("keep"), b"x").unwrap();
        let mut d = PrivateDirs::new();
        let got = d.ensure(&base, "cmd").unwrap();
        assert_ne!(got, wide);
        assert_eq!(mode(&got), 0o700);
        assert_eq!(mode(&wide), 0o777, "the planted directory is left alone");
        assert!(wide.join("keep").exists());
    }

    #[test]
    fn live_private_directory_of_another_process_is_never_removed() {
        // Same name, same owner, 0700, with content: indistinguishable from another konoma's live
        // directory (a container sharing /tmp with its own pid namespace).
        let base = unique_tmp("pd_stale");
        std::fs::create_dir_all(&base).unwrap();
        let other = base.join(format!("konoma-cmd-{}", std::process::id()));
        std::fs::create_dir(&other).unwrap();
        std::fs::set_permissions(&other, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::write(other.join("out-0"), b"theirs").unwrap();
        let mut d = PrivateDirs::new();
        let got = d.ensure(&base, "cmd").unwrap();
        assert_ne!(got, other);
        assert!(got.starts_with(&base));
        assert_eq!(mode(&got), 0o700);
        assert_eq!(std::fs::read(other.join("out-0")).unwrap(), b"theirs");
        assert_eq!(std::fs::read_dir(&got).unwrap().count(), 0);
    }

    #[test]
    fn a_chosen_directory_swapped_for_a_symlink_is_never_adopted() {
        let base = unique_tmp("pd_swap");
        let victim = base.join("victim");
        std::fs::create_dir_all(&victim).unwrap();
        let mut d = PrivateDirs::new();
        let first = d.ensure(&base, "cmd").unwrap();
        std::fs::remove_dir_all(&first).unwrap();
        symlink(&victim, &first).unwrap();
        let second = d.ensure(&base, "cmd").unwrap();
        assert_ne!(second, first);
        assert_eq!(mode(&second), 0o700);
        // Cleanup removes the new choice only; the planted symlink and its target survive.
        d.remove_all();
        assert!(!second.exists());
        assert!(victim.exists());
        assert!(std::fs::symlink_metadata(&first).is_ok());
    }

    #[test]
    fn removed_directory_is_recreated_in_place() {
        let base = unique_tmp("pd_recreate");
        std::fs::create_dir_all(&base).unwrap();
        let mut d = PrivateDirs::new();
        let a = d.ensure(&base, "cmd").unwrap();
        d.remove_all();
        assert!(!a.exists());
        assert_eq!(d.ensure(&base, "cmd").unwrap(), a);
        assert_eq!(mode(&a), 0o700);
    }

    #[test]
    fn remove_all_only_removes_chosen_directories_and_not_through_links() {
        let base = unique_tmp("pd_remove");
        std::fs::create_dir_all(&base).unwrap();
        let other = base.join("konoma-cmd-other");
        std::fs::create_dir(&other).unwrap();
        let mut d = PrivateDirs::new();
        let a = d.ensure(&base, "cmd").unwrap();
        d.remove_all();
        assert!(!a.exists());
        assert!(
            other.exists(),
            "a directory that was not chosen is left alone"
        );

        // `remove_chosen` on a symlink removes nothing it points to.
        let victim = base.join("victim");
        std::fs::create_dir_all(&victim).unwrap();
        std::fs::write(victim.join("f"), b"x").unwrap();
        let link = base.join("link");
        symlink(&victim, &link).unwrap();
        remove_chosen(&link);
        assert!(victim.join("f").exists());
    }

    #[test]
    fn dir_is_private_checks_owner_type_and_mode() {
        let base = unique_tmp("pd_pred");
        std::fs::create_dir_all(&base).unwrap();
        let d = base.join("d");
        std::fs::create_dir(&d).unwrap();
        let euid = current_euid();
        let set = |m| std::fs::set_permissions(&d, std::fs::Permissions::from_mode(m)).unwrap();
        let meta = || std::fs::symlink_metadata(&d).unwrap();
        set(0o700);
        assert!(dir_is_private(&meta(), euid));
        assert!(
            !dir_is_private(&meta(), euid.wrapping_add(1)),
            "foreign owner"
        );
        for bad in [0o770, 0o707, 0o750, 0o705, 0o755, 0o777] {
            set(bad);
            assert!(!dir_is_private(&meta(), euid), "{bad:o}");
        }
        // A file and a symlink are not directories.
        let f = base.join("f");
        std::fs::write(&f, b"x").unwrap();
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(!dir_is_private(
            &std::fs::symlink_metadata(&f).unwrap(),
            euid
        ));
        set(0o700);
        let l = base.join("l");
        symlink(&d, &l).unwrap();
        assert!(!dir_is_private(
            &std::fs::symlink_metadata(&l).unwrap(),
            euid
        ));
    }

    #[test]
    fn private_file_refuses_symlinks_and_existing_files() {
        let base = unique_tmp("pd_file");
        std::fs::create_dir_all(&base).unwrap();
        let victim = base.join("victim.txt");
        std::fs::write(&victim, b"precious").unwrap();
        let link = base.join("out-0");
        symlink(&victim, &link).unwrap();
        assert!(create_private_file(&link, b"overwrite").is_err());
        assert_eq!(std::fs::read(&victim).unwrap(), b"precious");
        // A dangling link is refused too (O_CREAT would otherwise create the target).
        let gone = base.join("gone.txt");
        let dangling = base.join("out-1");
        symlink(&gone, &dangling).unwrap();
        assert!(create_private_file(&dangling, b"x").is_err());
        assert!(!gone.exists());
        // An existing regular file is not truncated and reused.
        assert!(create_private_file(&victim, b"x").is_err());
        assert_eq!(std::fs::read(&victim).unwrap(), b"precious");
        // A fresh path works and is 0600.
        let ok = base.join("out-2");
        create_private_file(&ok, b"data").unwrap();
        assert_eq!(std::fs::read(&ok).unwrap(), b"data");
        assert_eq!(mode(&ok), 0o600);
    }
}
