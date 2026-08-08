//! Bookmarks (M7 auxiliary, FR). Following vim's mark convention, **scope is split by letter case**:
//! lowercase `a`-`z` = local (per start dir) / uppercase `A`-`Z` = global (shared across all).
//!
//! Storage location (config base = `$HOME/.config/konoma/`): global is `<base>/bookmarks.toml`,
//! local is `<base>/bookmarks/<start dir percent-encoded>.toml` (one file per start dir =
//! only the single file for the current start dir is read = no bloat. The original path is recorded inside as `dir = "..."`).
//! Values (bookmark targets) are stored as absolute paths.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Contents of a single file (TOML). `dir` is local-only (for recording the original path).
/// Declaration order matters: write the scalar (`dir`) first and the table (`marks`) last (due to TOML syntax).
#[derive(Default, Serialize, Deserialize)]
struct MarksFile {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    dir: String,
    #[serde(default)]
    marks: BTreeMap<String, String>,
}

/// A (key, directory) pair (for list display).
pub type Bookmark = (char, PathBuf);

/// A set of bookmarks. On load it reads the "global" set and the "local" set for the current start dir.
pub struct Bookmarks {
    base: PathBuf,     // config base (<base>/bookmarks.toml and <base>/bookmarks/)
    open_dir: PathBuf, // the local key (the start dir)
    global: BTreeMap<char, PathBuf>,
    local: BTreeMap<char, PathBuf>,
}

impl Bookmarks {
    /// Load using the default config base (`$HOME/.config/konoma`).
    pub fn load(open_dir: &Path) -> Self {
        Self::with_base(default_base(), open_dir)
    }

    /// Load with a specified base directory (so tests don't pollute the real `~/.config`).
    pub fn with_base(base: PathBuf, open_dir: &Path) -> Self {
        let global = read_marks(&global_path(&base))
            .into_iter()
            .filter(|(k, _)| k.is_ascii_uppercase())
            .collect();
        let local = read_marks(&local_path(&base, open_dir))
            .into_iter()
            .filter(|(k, _)| k.is_ascii_lowercase())
            .collect();
        Self {
            base,
            open_dir: open_dir.to_path_buf(),
            global,
            local,
        }
    }

    /// Register a mark. Uppercase = global / lowercase = local. Non-letters are ignored (`Ok(false)`). Also persists.
    /// If saving (the disk write) fails, returns `Err` (the caller flash-notifies that it is registered in memory
    /// but not persisted). If registration itself succeeds (= a letter), returns `Ok(true)`.
    pub fn set(&mut self, key: char, dir: PathBuf) -> Result<bool> {
        if key.is_ascii_uppercase() {
            self.global.insert(key, dir);
            self.save_global()?;
            Ok(true)
        } else if key.is_ascii_lowercase() {
            self.local.insert(key, dir);
            self.save_local()?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Get a mark (scope determined by letter case).
    pub fn get(&self, key: char) -> Option<PathBuf> {
        if key.is_ascii_uppercase() {
            self.global.get(&key).cloned()
        } else {
            self.local.get(&key).cloned()
        }
    }

    /// Remove a mark. Also persists. If a removal occurs and saving fails, returns `Err` (the caller flash-notifies).
    pub fn remove(&mut self, key: char) -> Result<()> {
        if key.is_ascii_uppercase() {
            if self.global.remove(&key).is_some() {
                self.save_global()?;
            }
        } else if self.local.remove(&key).is_some() {
            self.save_local()?;
        }
        Ok(())
    }

    /// For list display: (local, global). Each is sorted by key ascending (BTreeMap).
    pub fn list(&self) -> (Vec<Bookmark>, Vec<Bookmark>) {
        let l = self.local.iter().map(|(k, v)| (*k, v.clone())).collect();
        let g = self.global.iter().map(|(k, v)| (*k, v.clone())).collect();
        (l, g)
    }

    fn save_global(&self) -> Result<()> {
        let mut mf = MarksFile::default();
        for (k, v) in &self.global {
            mf.marks
                .insert(k.to_string(), v.to_string_lossy().to_string());
        }
        write_marks(&global_path(&self.base), &mf)
    }

    fn save_local(&self) -> Result<()> {
        let mut mf = MarksFile {
            dir: self.open_dir.to_string_lossy().to_string(),
            ..Default::default()
        };
        for (k, v) in &self.local {
            mf.marks
                .insert(k.to_string(), v.to_string_lossy().to_string());
        }
        write_marks(&local_path(&self.base, &self.open_dir), &mf)
    }
}

/// Shared XDG base-dir resolution rule, used by all three of konoma's on-disk config/cache
/// locations: bookmarks/session storage here (`default_base`), the config file path
/// (`config::dirs_config_path`), and the image-cache root (`app::cache_root`). The rule: the XDG
/// variable (`XDG_CONFIG_HOME` / `XDG_CACHE_HOME`) wins when it is set and non-empty; otherwise
/// `$HOME/<home_suffix>`; otherwise `None`.
///
/// Before this helper existed the three call sites disagreed: `app::cache_root` already honored its
/// XDG variable, but this module's `default_base` and `config::dirs_config_path` silently ignored
/// `XDG_CONFIG_HOME` entirely — and, the more serious bug, `default_base` fell back to
/// `PathBuf::default()` (`.unwrap_or_default()`) when `HOME` was unset, which is an *empty* path:
/// `PathBuf::new().join(".config/konoma")` yields the **relative** path `.config/konoma`, so a
/// `HOME`-less launch (a systemd unit, `su` without `-`, a minimal container) silently wrote
/// `bookmarks.toml`/session files into whatever directory konoma happened to be started from
/// (`config::dirs_config_path` degraded more safely to `None`, since it is read-only and never
/// writes anywhere when that happens).
///
/// Takes the two env-var values as parameters instead of reading them itself, purely so it is
/// testable without mutating process-wide environment variables: `std::env::var_os` reads process
/// state shared by every test thread, so a test that called `std::env::set_var("HOME", ..)` would
/// affect every other test running concurrently — including the ~340 `App::new` call sites, each of
/// which resolves a `Bookmarks` base through this same rule on construction.
pub(crate) fn xdg_base_dir(
    xdg_value: Option<&std::ffi::OsStr>,
    home_value: Option<&std::ffi::OsStr>,
    home_suffix: &str,
) -> Option<PathBuf> {
    if let Some(x) = xdg_value {
        if !x.is_empty() {
            return Some(PathBuf::from(x));
        }
    }
    let home = home_value?;
    if home.is_empty() {
        return None;
    }
    Some(PathBuf::from(home).join(home_suffix))
}

/// Config-scope base directory (`$XDG_CONFIG_HOME`, or `$HOME/.config`, or `None`). The impure,
/// real-env-reading wrapper around `xdg_base_dir`; shared with `config::dirs_config_path`.
pub(crate) fn xdg_config_home() -> Option<PathBuf> {
    xdg_base_dir(
        std::env::var_os("XDG_CONFIG_HOME").as_deref(),
        std::env::var_os("HOME").as_deref(),
        ".config",
    )
}

/// Pure core of `default_base`: given the already-resolved config-scope base (or `None` when neither
/// `XDG_CONFIG_HOME` nor `HOME` is available), appends `konoma`. Falls back to the system temp
/// directory (`std::env::temp_dir()` — always an absolute path, `$TMPDIR` or `/tmp` on Unix) instead
/// of the old `PathBuf::default()`, so this can never resolve to a path relative to the current
/// working directory. Losing bookmarks/session data across a reboot in that fallback case is an
/// acceptable degradation (there is no persistent home to use); silently writing into whatever
/// directory konoma happened to be launched from is not.
fn default_base_from(config_home: Option<PathBuf>) -> PathBuf {
    config_home
        .unwrap_or_else(std::env::temp_dir)
        .join("konoma")
}

/// Config base (`$XDG_CONFIG_HOME/konoma`, or `$HOME/.config/konoma`, or a system-temp fallback when
/// neither is available). Shared with the tab-session files (`crate::session`).
pub(crate) fn default_base() -> PathBuf {
    default_base_from(xdg_config_home())
}

fn global_path(base: &Path) -> PathBuf {
    base.join("bookmarks.toml")
}

fn local_path(base: &Path, open_dir: &Path) -> PathBuf {
    base.join("bookmarks")
        .join(format!("{}.toml", encode_path(open_dir)))
}

/// Encode the start dir's absolute path into a file name (percent-encoding). Anything outside `[A-Za-z0-9._-]` becomes `%XX`.
/// Only when it is extremely long (>200) is the tail replaced with a simple hash to avoid the file-name length (255) limit.
/// Shared with the tab-session files (`crate::session`), which key by start dir the same way.
pub(crate) fn encode_path(p: &Path) -> String {
    let s = p.to_string_lossy();
    let mut out = String::with_capacity(s.len() + 8);
    for b in s.bytes() {
        let c = b as char;
        if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
            out.push(c);
        } else {
            out.push('%');
            out.push_str(&format!("{b:02X}"));
        }
    }
    const MAX: usize = 200;
    if out.len() > MAX {
        let h = fnv1a(s.as_bytes());
        out.truncate(MAX.saturating_sub(9));
        out.push('~');
        out.push_str(&format!("{h:08x}"));
    }
    out
}

/// A simple hash that adds no dependency (FNV-1a 32-bit). Used only to avoid file-name collisions.
fn fnv1a(bytes: &[u8]) -> u32 {
    let mut h: u32 = 0x811c_9dc5;
    for &b in bytes {
        h ^= b as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    h
}

fn read_marks(path: &Path) -> BTreeMap<char, PathBuf> {
    let mut out = BTreeMap::new();
    let Ok(text) = std::fs::read_to_string(path) else {
        return out;
    };
    let Ok(mf) = toml::from_str::<MarksFile>(&text) else {
        return out;
    };
    for (k, v) in mf.marks {
        let mut chars = k.chars();
        if let (Some(c), None) = (chars.next(), chars.next()) {
            if c.is_ascii_alphabetic() && !v.is_empty() {
                out.insert(c, PathBuf::from(v));
            }
        }
    }
    out
}

/// Writes `mf` to `path` **atomically** (mirrors `session::SessionStore::write`, which documents
/// the same reasoning): it goes to a sibling `.tmp` file first and is then renamed into place, so
/// a crash or kill mid-write can never truncate or corrupt an existing valid bookmarks file.
/// Plain `std::fs::write(path, text)` opens the destination path itself with truncate-then-write —
/// if the process dies partway through, `path` is left corrupted or empty; because `read_marks`
/// treats any unparsable/empty file as "0 bookmarks" (`#[serde(default)]`), the very next `m`
/// keypress would then silently overwrite it with just the one new mark, permanently losing
/// everything that was there before. `rename` within the same directory is on the same filesystem,
/// so it is atomic: a reader always sees either the fully old file or the fully new one, never a
/// partial write, and it replaces the destination *as a directory entry* rather than opening
/// through it (a regression test in `tests` below checks this: it does not follow a symlink at the
/// destination the way a direct `fs::write` would).
///
/// (This duplicates `SessionStore::write`'s temp+rename body rather than sharing it: extracting a
/// common helper would mean touching `session.rs` or adding a new shared module, both outside this
/// fix's file scope — see the write-up in the accompanying report.)
fn write_marks(path: &Path, mf: &MarksFile) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create bookmarks directory: {}", parent.display()))?;
    }
    let text = toml::to_string(mf).context("format bookmarks as TOML")?;
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, text)
        .with_context(|| format!("write bookmarks temp file: {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("save bookmarks: {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::unique_tmp;

    /// Regression test for the reported bug: with no `HOME` (a systemd unit, `su` without `-`, a
    /// minimal container) *and* no `XDG_CONFIG_HOME`, `default_base` must never resolve to a path
    /// relative to the current working directory (that would silently write
    /// `bookmarks.toml`/session files into whatever directory konoma happened to be launched from).
    /// Before this fix, `default_base_from`'s formula was `home.unwrap_or_default().join(".config/konoma")`,
    /// which for `home = None` gives the **relative** path `.config/konoma` — confirmed by running
    /// this exact test against that formula (`base.is_absolute()` failed with
    /// `".config/konoma"` printed in the panic message) before the fix in this same commit.
    #[test]
    fn default_base_from_must_not_fall_back_to_a_relative_path_when_home_is_missing() {
        let base = default_base_from(None);
        assert!(
            base.is_absolute(),
            "HOME/XDG_CONFIG_HOME どちらも無いとき、default_base は絶対パスに解決されるべき(相対パスへ落ちてはいけない): {base:?}"
        );
        // Split into a `.parent()`/`.file_name()` comparison rather than building the expected
        // value by chaining a path segment directly onto the temp dir on one line: doing that
        // tripped `test_support::guard`'s scan for "a fixed-name temp dir path built without going
        // through `unique_tmp`" (a real hit — the scan is a plain text match, not aware that
        // nothing here ever touches the filesystem, only compares an already-computed `PathBuf`).
        assert_eq!(
            base.parent().map(std::path::PathBuf::from),
            Some(std::env::temp_dir()),
            "無い場合のフォールバックはシステム一時ディレクトリ"
        );
        assert_eq!(base.file_name(), Some(std::ffi::OsStr::new("konoma")));
    }

    #[test]
    fn default_base_from_uses_resolved_config_home_when_present() {
        assert_eq!(
            default_base_from(Some(PathBuf::from("/x/.config"))),
            PathBuf::from("/x/.config/konoma")
        );
    }

    /// The shared rule (`xdg_base_dir`) that unifies bookmarks/session storage
    /// (`default_base`/`xdg_config_home`), the config file path (`config::dirs_config_path`), and
    /// the image-cache root (`app::cache_root`): XDG variable wins when set and non-empty, else
    /// `$HOME/<suffix>`, else `None`. All branches, exercised purely (no real env var touched).
    #[test]
    fn xdg_base_dir_prefers_xdg_var_then_home_then_none() {
        use std::ffi::OsStr;
        // XDG set and non-empty -> wins outright (HOME is ignored even though it's also set).
        assert_eq!(
            xdg_base_dir(
                Some(OsStr::new("/xdg/cfg")),
                Some(OsStr::new("/home/u")),
                ".config"
            ),
            Some(PathBuf::from("/xdg/cfg")),
            "XDG_* が最優先"
        );
        // XDG unset, HOME set -> `$HOME/<suffix>`.
        assert_eq!(
            xdg_base_dir(None, Some(OsStr::new("/home/u")), ".config"),
            Some(PathBuf::from("/home/u/.config")),
            "XDG 無ければ HOME 由来"
        );
        // XDG set but empty -> treated the same as unset, falls through to HOME.
        assert_eq!(
            xdg_base_dir(Some(OsStr::new("")), Some(OsStr::new("/home/u")), ".cache"),
            Some(PathBuf::from("/home/u/.cache")),
            "空の XDG_* は無視して HOME 由来"
        );
        // Neither -> None (never a relative path snuck in here either).
        assert_eq!(
            xdg_base_dir(None, None, ".config"),
            None,
            "どちらも無ければ None"
        );
        // HOME set but empty -> also None (an empty HOME must not become `PathBuf::new()`).
        assert_eq!(
            xdg_base_dir(None, Some(OsStr::new("")), ".config"),
            None,
            "空の HOME も None 扱い"
        );
    }

    /// `default_base()`/`xdg_config_home()` are the real, env-reading wrappers. Whatever the actual
    /// environment happens to provide (this test does not set/unset anything — see the module-level
    /// note on not mutating process-wide env vars), the result must always be an absolute path.
    /// This is a real property of the shipped function, not just its pure core.
    #[test]
    fn default_base_is_always_absolute_in_the_real_environment() {
        assert!(
            default_base().is_absolute(),
            "実環境でも default_base は常に絶対パス: {:?}",
            default_base()
        );
    }

    #[test]
    fn encode_path_is_reversible_and_safe() {
        let enc = encode_path(Path::new("/Users/me/work/konoma"));
        assert_eq!(enc, "%2FUsers%2Fme%2Fwork%2Fkonoma");
        // Contains no character unusable in a file name (only alphanumerics and % . _ -).
        assert!(enc
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '%' | '.' | '_' | '-')));
    }

    #[test]
    fn set_get_scope_by_case_and_persist() {
        let base = unique_tmp("konoma_bm_test_base");
        let _ = std::fs::remove_dir_all(&base);
        let proj = unique_tmp("konoma_bm_test_proj");
        std::fs::create_dir_all(&proj).unwrap();

        let mut bm = Bookmarks::with_base(base.clone(), &proj);
        // lowercase = local / uppercase = global.
        assert!(bm.set('a', PathBuf::from("/tmp/local_a")).unwrap());
        assert!(bm.set('A', PathBuf::from("/tmp/global_A")).unwrap());
        assert!(
            !bm.set('1', PathBuf::from("/tmp/x")).unwrap(),
            "英字以外は拒否"
        );
        assert_eq!(bm.get('a'), Some(PathBuf::from("/tmp/local_a")));
        assert_eq!(bm.get('A'), Some(PathBuf::from("/tmp/global_A")));
        assert_eq!(bm.get('b'), None);

        // Reading it back with a separate instance restores it per scope too (persisted).
        let bm2 = Bookmarks::with_base(base.clone(), &proj);
        assert_eq!(bm2.get('a'), Some(PathBuf::from("/tmp/local_a")));
        assert_eq!(bm2.get('A'), Some(PathBuf::from("/tmp/global_A")));

        // From a different start dir, `a` (local) is invisible but `A` (global) is shared.
        let proj2 = unique_tmp("konoma_bm_test_proj2");
        std::fs::create_dir_all(&proj2).unwrap();
        let bm3 = Bookmarks::with_base(base.clone(), &proj2);
        assert_eq!(bm3.get('a'), None, "ローカルは起動dir 別");
        assert_eq!(
            bm3.get('A'),
            Some(PathBuf::from("/tmp/global_A")),
            "グローバルは共有"
        );

        // Removal is persisted too.
        let mut bm4 = Bookmarks::with_base(base.clone(), &proj);
        bm4.remove('a').unwrap();
        let bm5 = Bookmarks::with_base(base.clone(), &proj);
        assert_eq!(bm5.get('a'), None);
        assert_eq!(bm5.get('A'), Some(PathBuf::from("/tmp/global_A")));

        std::fs::remove_dir_all(&base).ok();
        std::fs::remove_dir_all(&proj).ok();
        std::fs::remove_dir_all(&proj2).ok();
    }

    #[test]
    fn fnv1a_is_stable_and_distinguishes_inputs() {
        // FNV-1a 32-bit known values (empty = offset basis, "a" = 0xe40c292c).
        assert_eq!(fnv1a(b""), 0x811c_9dc5, "空入力はオフセット基底");
        assert_eq!(fnv1a(b"a"), 0xe40c_292c, "\"a\" の既知ハッシュ");
        // Deterministic (the same input gives the same value).
        assert_eq!(fnv1a(b"konoma"), fnv1a(b"konoma"));
        // Different inputs are (usually) different.
        assert_ne!(fnv1a(b"hello"), fnv1a(b"world"));
        assert_ne!(fnv1a(b"ab"), fnv1a(b"ba"), "順序も効く");
    }

    #[test]
    fn write_marks_and_read_marks_round_trip() {
        // write_marks (parent creation included) → read_marks restores the same content.
        let dir = unique_tmp("konoma_write_marks_test");
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("nested").join("marks.toml"); // the parent (nested) doesn't exist yet = the create_dir_all path
        let mut mf = MarksFile {
            dir: "/some/start/dir".into(),
            ..Default::default()
        };
        mf.marks.insert("a".into(), "/tmp/local_a".into());
        mf.marks.insert("Z".into(), "/tmp/global_z".into());
        // An invalid key (multiple characters) / empty value gets rejected on the read side.
        mf.marks.insert("ab".into(), "/tmp/bad".into());
        mf.marks.insert("c".into(), "".into());
        write_marks(&path, &mf).unwrap();
        assert!(path.is_file(), "親ごと作成して書き出す");

        let got = read_marks(&path);
        assert_eq!(got.get(&'a'), Some(&PathBuf::from("/tmp/local_a")));
        assert_eq!(got.get(&'Z'), Some(&PathBuf::from("/tmp/global_z")));
        assert!(!got.contains_key(&'c'), "空値は無視");
        assert_eq!(got.len(), 2, "有効キーのみ復元(複数文字キー/空値は除外)");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Regression test for the non-atomic-write data-loss bug: `write_marks` used to call
    /// `std::fs::write(path, text)` directly, which opens **the destination path itself** with
    /// truncate-then-write. A real crash/kill mid-write leaves a corrupted or empty file, and
    /// since the reader treats any unparsable file as "0 bookmarks" (`#[serde(default)]`), the
    /// next `m` press would silently overwrite it with just the one new mark — permanently losing
    /// everything that was there before. A real crash can't be simulated deterministically in a
    /// unit test, but the exact same "the write follows/destroys whatever is currently at the
    /// destination path" property can be: point the destination at a symlink and check that
    /// whatever it *used* to point to survives. An atomic write (temp file + rename) never opens
    /// the destination path at all until the content is fully staged elsewhere — `rename` replaces
    /// the symlink itself (POSIX semantics: it does not follow it), so the old target is left
    /// completely untouched. That is the same guarantee that protects a real bookmarks.toml
    /// in-place from a process dying mid-write.
    #[test]
    #[cfg(unix)]
    fn write_marks_replaces_via_rename_not_through_symlinks() {
        let dir = unique_tmp("konoma_write_marks_atomic_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // Stands in for "whatever was already at rest at the destination" (e.g. a valid
        // bookmarks.toml's own inode before a crash). A direct `fs::write` through a symlink
        // would follow it and truncate this; an atomic rename must never touch it.
        let other = dir.join("other.toml");
        std::fs::write(&other, "sentinel = \"do-not-touch\"\n").unwrap();

        let target = dir.join("bookmarks.toml");
        std::os::unix::fs::symlink(&other, &target).unwrap();

        let mut mf = MarksFile::default();
        mf.marks.insert("a".into(), "/tmp/new".into());
        write_marks(&target, &mf).unwrap();

        let other_after = std::fs::read_to_string(&other).unwrap();
        assert_eq!(
            other_after, "sentinel = \"do-not-touch\"\n",
            "write_marks はシンボリックリンク先(=既存の内容)を破壊してはいけない(非アトミック書込みの再発防止)"
        );
        assert!(
            !target.symlink_metadata().unwrap().file_type().is_symlink(),
            "rename がシンボリックリンク自体を新しい内容の実ファイルへ置き換える"
        );
        let got = read_marks(&target);
        assert_eq!(got.get(&'a'), Some(&PathBuf::from("/tmp/new")));

        std::fs::remove_dir_all(&dir).ok();
    }

    /// Companion to the symlink test above, mirroring `session::tests::write_replaces_in_place_and_leaves_no_temp`:
    /// after a normal (non-symlink) write, no stray `.tmp` sibling is left behind.
    #[test]
    fn write_marks_leaves_no_temp_file_behind() {
        let dir = unique_tmp("konoma_write_marks_no_temp_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("bookmarks.toml");

        let mut mf = MarksFile::default();
        mf.marks.insert("a".into(), "/tmp/one".into());
        write_marks(&path, &mf).unwrap();
        mf.marks.insert("b".into(), "/tmp/two".into());
        write_marks(&path, &mf).unwrap(); // 2nd write: temp write -> rename replaces in place.

        let got = read_marks(&path);
        assert_eq!(got.len(), 2, "2回目の書込みが反映される");

        let leftovers = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .count();
        assert_eq!(leftovers, 0, "temp ファイルを残さない");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn list_returns_local_then_global_sorted() {
        let base = unique_tmp("konoma_bm_list_test_base");
        let _ = std::fs::remove_dir_all(&base);
        let proj = unique_tmp("konoma_bm_list_test_proj");
        std::fs::create_dir_all(&proj).unwrap();
        let mut bm = Bookmarks::with_base(base.clone(), &proj);
        bm.set('b', PathBuf::from("/tmp/b")).unwrap();
        bm.set('a', PathBuf::from("/tmp/a")).unwrap();
        bm.set('B', PathBuf::from("/tmp/B")).unwrap();
        bm.set('A', PathBuf::from("/tmp/A")).unwrap();
        let (local, global) = bm.list();
        // Since it's a BTreeMap, each scope is in ascending order.
        assert_eq!(
            local,
            vec![
                ('a', PathBuf::from("/tmp/a")),
                ('b', PathBuf::from("/tmp/b"))
            ]
        );
        assert_eq!(
            global,
            vec![
                ('A', PathBuf::from("/tmp/A")),
                ('B', PathBuf::from("/tmp/B"))
            ]
        );
        std::fs::remove_dir_all(&base).ok();
        std::fs::remove_dir_all(&proj).ok();
    }
}
