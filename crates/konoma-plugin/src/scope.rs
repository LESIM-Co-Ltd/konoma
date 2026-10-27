//! Where the host reads files on a plugin's behalf.
//!
//! When a plugin hands the host a path to read (a picture file, a text file), the host reads it
//! only if the file is inside the plugin's **read scope**. The scope is decided by the host and the
//! user (never by the plugin's manifest): by default the folder tree the user opened and the
//! folder of the file being previewed, widened by the user's `[plugins.<id>] read_dirs`. The host
//! gives the settled scope to the plugin ([`crate::ViewCx::read_scope`]) so a plugin can keep its
//! own reads inside it too. It does not stop a plugin's own code from reading anything: the scope
//! limits what the host reads for the plugin.

use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Why a path is not readable under a scope.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScopeError {
    /// The path could not be resolved to a real file (it does not exist, or a part of it cannot
    /// be entered). Not a scope matter: reading it would fail anyway.
    Unresolved,
    /// The real path (symbolic links resolved) is outside every folder of the scope.
    Outside,
}

/// The folders the host reads files from for one plugin: unrestricted, or a list of real
/// (symbolic links resolved) directories.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReadScope {
    /// `None` = no limit (a standard plugin, and the default of a context made by `new`).
    dirs: Option<Arc<[PathBuf]>>,
}

impl ReadScope {
    /// No limit.
    pub fn unrestricted() -> ReadScope {
        ReadScope { dirs: None }
    }

    /// Limited to `dirs`. Each is resolved to its real path; one that does not exist is left out
    /// (so a scope can end up empty, which allows nothing).
    pub fn within<I, P>(dirs: I) -> ReadScope
    where
        I: IntoIterator<Item = P>,
        P: AsRef<Path>,
    {
        let mut real: Vec<PathBuf> = Vec::new();
        for d in dirs {
            if let Ok(r) = std::fs::canonicalize(d.as_ref()) {
                if !real.contains(&r) {
                    real.push(r);
                }
            }
        }
        ReadScope {
            dirs: Some(real.into()),
        }
    }

    /// Whether the scope limits anything.
    pub fn is_restricted(&self) -> bool {
        self.dirs.is_some()
    }

    /// The real directories of a limited scope; `None` when there is no limit.
    pub fn dirs(&self) -> Option<&[PathBuf]> {
        self.dirs.as_deref()
    }

    /// A number that names the scope's identity: `0` for no limit, otherwise a hash of the real
    /// directories (order-independent). Two scopes that allow different folders differ, so a cache
    /// keyed with it never serves a file judged under another scope.
    pub fn fingerprint(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let Some(dirs) = &self.dirs else {
            return 0;
        };
        let mut sorted: Vec<&PathBuf> = dirs.iter().collect();
        sorted.sort();
        let mut h = std::collections::hash_map::DefaultHasher::new();
        sorted.hash(&mut h);
        // Never 0, which is reserved for "no limit".
        h.finish() | 1
    }

    /// Judge `path` and give the path to read. With no limit the path is returned as it is.
    /// Otherwise it is resolved to its real path (symbolic links and `..` resolved) and must be
    /// inside one of the directories, compared component by component (`/a/bc` is not inside
    /// `/a/b`). **Read the returned path**, not the one asked about: that is the file that was
    /// judged.
    pub fn resolve(&self, path: &Path) -> Result<PathBuf, ScopeError> {
        let Some(dirs) = &self.dirs else {
            return Ok(path.to_path_buf());
        };
        let real = std::fs::canonicalize(path).map_err(|_| ScopeError::Unresolved)?;
        if dirs.iter().any(|d| real.starts_with(d)) {
            Ok(real)
        } else {
            Err(ScopeError::Outside)
        }
    }

    /// Whether `path` is readable under the scope ([`ReadScope::resolve`] succeeds).
    pub fn allows(&self, path: &Path) -> bool {
        self.resolve(path).is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use konoma_test_support::unique_tmp;

    #[test]
    fn unrestricted_allows_anything_and_returns_the_path_as_given() {
        let s = ReadScope::unrestricted();
        assert!(!s.is_restricted());
        assert_eq!(s.dirs(), None);
        assert_eq!(
            s.resolve(Path::new("/no/such/file")),
            Ok(PathBuf::from("/no/such/file"))
        );
        assert_eq!(ReadScope::default(), s);
    }

    #[test]
    fn inside_ok_outside_refused_and_a_missing_file_is_unresolved() {
        let root = unique_tmp("scope-basic");
        let (a, b) = (root.join("in"), root.join("out"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        std::fs::write(a.join("x.png"), b"x").unwrap();
        std::fs::write(b.join("y.png"), b"y").unwrap();
        let s = ReadScope::within([&a]);
        assert!(s.is_restricted());
        assert!(s.allows(&a.join("x.png")));
        assert_eq!(s.resolve(&b.join("y.png")), Err(ScopeError::Outside));
        assert_eq!(s.resolve(&a.join("nope.png")), Err(ScopeError::Unresolved));
    }

    #[test]
    fn the_fingerprint_names_the_folders_and_not_their_order() {
        let root = unique_tmp("scope-fp");
        let (a, b) = (root.join("a"), root.join("b"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        assert_eq!(ReadScope::unrestricted().fingerprint(), 0);
        let (sa, sb, sab, sba) = (
            ReadScope::within([&a]),
            ReadScope::within([&b]),
            ReadScope::within([&a, &b]),
            ReadScope::within([&b, &a]),
        );
        assert_ne!(sa.fingerprint(), 0);
        assert_ne!(sa.fingerprint(), sb.fingerprint());
        assert_ne!(sa.fingerprint(), sab.fingerprint());
        assert_eq!(sab.fingerprint(), sba.fingerprint());
        // An empty scope is still a limit, not "no limit".
        assert_ne!(ReadScope::within(Vec::<PathBuf>::new()).fingerprint(), 0);
    }

    #[test]
    fn the_prefix_is_by_component_not_by_string() {
        let root = unique_tmp("scope-prefix");
        let (b, bc) = (root.join("b"), root.join("bc"));
        std::fs::create_dir_all(&b).unwrap();
        std::fs::create_dir_all(&bc).unwrap();
        std::fs::write(bc.join("f"), b"f").unwrap();
        assert_eq!(
            ReadScope::within([&b]).resolve(&bc.join("f")),
            Err(ScopeError::Outside)
        );
    }

    #[test]
    fn dot_dot_and_symlinks_are_judged_by_the_real_path() {
        let root = unique_tmp("scope-real");
        let (a, b) = (root.join("a"), root.join("b"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        std::fs::write(b.join("secret"), b"s").unwrap();
        std::fs::write(a.join("ok"), b"o").unwrap();
        let s = ReadScope::within([&a]);
        // `..` out of the scope.
        assert_eq!(s.resolve(&a.join("../b/secret")), Err(ScopeError::Outside));
        // A link inside pointing out is refused; a link outside pointing in is allowed, and the
        // path returned is the real one.
        std::os::unix::fs::symlink(b.join("secret"), a.join("link-out")).unwrap();
        std::os::unix::fs::symlink(a.join("ok"), b.join("link-in")).unwrap();
        assert_eq!(s.resolve(&a.join("link-out")), Err(ScopeError::Outside));
        assert_eq!(
            s.resolve(&b.join("link-in")),
            Ok(std::fs::canonicalize(a.join("ok")).unwrap())
        );
    }

    #[test]
    fn a_missing_directory_is_left_out_and_an_empty_scope_allows_nothing() {
        let root = unique_tmp("scope-empty");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("f"), b"f").unwrap();
        let s = ReadScope::within([root.join("missing")]);
        assert!(s.is_restricted());
        assert_eq!(s.dirs().map(<[PathBuf]>::len), Some(0));
        assert_eq!(s.resolve(&root.join("f")), Err(ScopeError::Outside));
    }
}
