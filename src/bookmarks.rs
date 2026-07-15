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
    base: PathBuf,     // 設定ベース (<base>/bookmarks.toml と <base>/bookmarks/)
    open_dir: PathBuf, // ローカルの鍵 (起動dir)
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

/// Config base (`$HOME/.config/konoma`). Shared with the tab-session files (`crate::session`).
pub(crate) fn default_base() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    home.join(".config/konoma")
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

fn write_marks(path: &Path, mf: &MarksFile) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("ブックマーク保存先の作成: {}", parent.display()))?;
    }
    let text = toml::to_string(mf).context("ブックマークの TOML 整形")?;
    std::fs::write(path, text).with_context(|| format!("ブックマーク保存: {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_path_is_reversible_and_safe() {
        let enc = encode_path(Path::new("/Users/me/work/konoma"));
        assert_eq!(enc, "%2FUsers%2Fme%2Fwork%2Fkonoma");
        // ファイル名に使えない文字を含まない(英数と % . _ - のみ)。
        assert!(enc
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '%' | '.' | '_' | '-')));
    }

    #[test]
    fn set_get_scope_by_case_and_persist() {
        let base = std::env::temp_dir().join("konoma_bm_test_base");
        let _ = std::fs::remove_dir_all(&base);
        let proj = std::env::temp_dir().join("konoma_bm_test_proj");
        std::fs::create_dir_all(&proj).unwrap();

        let mut bm = Bookmarks::with_base(base.clone(), &proj);
        // 小文字=ローカル / 大文字=グローバル。
        assert!(bm.set('a', PathBuf::from("/tmp/local_a")).unwrap());
        assert!(bm.set('A', PathBuf::from("/tmp/global_A")).unwrap());
        assert!(
            !bm.set('1', PathBuf::from("/tmp/x")).unwrap(),
            "英字以外は拒否"
        );
        assert_eq!(bm.get('a'), Some(PathBuf::from("/tmp/local_a")));
        assert_eq!(bm.get('A'), Some(PathBuf::from("/tmp/global_A")));
        assert_eq!(bm.get('b'), None);

        // 別インスタンスで読み直してもスコープごとに復元される(永続化)。
        let bm2 = Bookmarks::with_base(base.clone(), &proj);
        assert_eq!(bm2.get('a'), Some(PathBuf::from("/tmp/local_a")));
        assert_eq!(bm2.get('A'), Some(PathBuf::from("/tmp/global_A")));

        // 別の起動dir では a(ローカル)は見えないが A(グローバル)は共有される。
        let proj2 = std::env::temp_dir().join("konoma_bm_test_proj2");
        std::fs::create_dir_all(&proj2).unwrap();
        let bm3 = Bookmarks::with_base(base.clone(), &proj2);
        assert_eq!(bm3.get('a'), None, "ローカルは起動dir 別");
        assert_eq!(
            bm3.get('A'),
            Some(PathBuf::from("/tmp/global_A")),
            "グローバルは共有"
        );

        // 削除も永続化される。
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
        // FNV-1a 32bit の既知値(空=オフセット基底, "a"=0xe40c292c)。
        assert_eq!(fnv1a(b""), 0x811c_9dc5, "空入力はオフセット基底");
        assert_eq!(fnv1a(b"a"), 0xe40c_292c, "\"a\" の既知ハッシュ");
        // 決定的(同入力は同値)。
        assert_eq!(fnv1a(b"konoma"), fnv1a(b"konoma"));
        // 異なる入力は(まず)異なる。
        assert_ne!(fnv1a(b"hello"), fnv1a(b"world"));
        assert_ne!(fnv1a(b"ab"), fnv1a(b"ba"), "順序も効く");
    }

    #[test]
    fn write_marks_and_read_marks_round_trip() {
        // write_marks(親作成込み)→ read_marks で同じ内容が復元される。
        let dir = std::env::temp_dir().join("konoma_write_marks_test");
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("nested").join("marks.toml"); // 親(nested)は未作成 = create_dir_all 経路
        let mut mf = MarksFile {
            dir: "/some/start/dir".into(),
            ..Default::default()
        };
        mf.marks.insert("a".into(), "/tmp/local_a".into());
        mf.marks.insert("Z".into(), "/tmp/global_z".into());
        // 不正キー(複数文字)/空値は read 側で弾かれる。
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

    #[test]
    fn list_returns_local_then_global_sorted() {
        let base = std::env::temp_dir().join("konoma_bm_list_test_base");
        let _ = std::fs::remove_dir_all(&base);
        let proj = std::env::temp_dir().join("konoma_bm_list_test_proj");
        std::fs::create_dir_all(&proj).unwrap();
        let mut bm = Bookmarks::with_base(base.clone(), &proj);
        bm.set('b', PathBuf::from("/tmp/b")).unwrap();
        bm.set('a', PathBuf::from("/tmp/a")).unwrap();
        bm.set('B', PathBuf::from("/tmp/B")).unwrap();
        bm.set('A', PathBuf::from("/tmp/A")).unwrap();
        let (local, global) = bm.list();
        // BTreeMap なので各スコープは昇順。
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
