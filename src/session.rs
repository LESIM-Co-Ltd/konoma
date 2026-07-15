//! Per-project tab-session persistence for `[ui] restore_tabs` (restore the previous tab set).
//!
//! Storage (config base = `$HOME/.config/konoma/`): `<base>/sessions/<start dir percent-encoded>.toml`,
//! one file per start dir — the same layout as local bookmarks (the original path is recorded inside
//! as `dir = "..."`). Saved paths are absolute. A missing or unparsable file simply means "no saved
//! session" (design principle #3: a broken file must never crash or block startup).

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::bookmarks::{default_base, encode_path};

/// One saved tab: its tree root, the entry under the cursor, and — when the tab was left in
/// Preview — the file that was being previewed.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct SavedTab {
    pub root: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
}

/// The tab set that was open when konoma last exited in a start dir.
/// Declaration order matters for TOML: scalars (`dir`, `active`) first, the table array (`tabs`) last.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct SavedSession {
    /// Original start dir (human-readable record; the file name is the percent-encoded form).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub dir: String,
    /// Index of the tab that was active (clamped on restore).
    #[serde(default)]
    pub active: usize,
    #[serde(default)]
    pub tabs: Vec<SavedTab>,
}

/// Reads/writes the session file of one start dir.
pub struct SessionStore {
    path: PathBuf,
    dir: String,
}

impl SessionStore {
    /// Store for the default config base (`$HOME/.config/konoma`).
    pub fn load(start_dir: &Path) -> Self {
        Self::with_base(default_base(), start_dir)
    }

    /// Store with an explicit base directory (so tests don't pollute the real `~/.config`).
    pub fn with_base(base: PathBuf, start_dir: &Path) -> Self {
        Self {
            path: base
                .join("sessions")
                .join(format!("{}.toml", encode_path(start_dir))),
            dir: start_dir.to_string_lossy().to_string(),
        }
    }

    /// Read the saved session. `None` = no file **or** unparsable — both mean a fresh start.
    pub fn read(&self) -> Option<SavedSession> {
        let text = std::fs::read_to_string(&self.path).ok()?;
        toml::from_str(&text).ok()
    }

    /// Persist `sess` (the store fills in `dir`). Creates the parent directory as needed.
    pub fn write(&self, mut sess: SavedSession) -> Result<()> {
        sess.dir = self.dir.clone();
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("セッション保存先の作成: {}", parent.display()))?;
        }
        let text = toml::to_string(&sess).context("セッションの TOML 整形")?;
        std::fs::write(&self.path, text)
            .with_context(|| format!("セッション保存: {}", self.path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_read_round_trip_records_dir_and_tabs() {
        let base = std::env::temp_dir().join("konoma_session_store_test_base");
        let _ = std::fs::remove_dir_all(&base);
        let proj = std::env::temp_dir().join("konoma_session_store_test_proj");
        std::fs::create_dir_all(&proj).unwrap();

        let store = SessionStore::with_base(base.clone(), &proj);
        let sess = SavedSession {
            dir: String::new(), // write が埋める
            active: 1,
            tabs: vec![
                SavedTab {
                    root: "/tmp/a".into(),
                    cursor: Some("/tmp/a/x.txt".into()),
                    preview: None,
                },
                SavedTab {
                    root: "/tmp/b".into(),
                    cursor: None,
                    preview: Some("/tmp/b/y.md".into()),
                },
            ],
        };
        store.write(sess.clone()).unwrap();

        let got = SessionStore::with_base(base.clone(), &proj).read().unwrap();
        assert_eq!(got.dir, proj.to_string_lossy(), "起動 dir を記録する");
        assert_eq!(got.active, 1);
        assert_eq!(got.tabs, sess.tabs);

        // 別の起動 dir のストアからは見えない(ディレクトリ毎に1ファイル)。
        let proj2 = std::env::temp_dir().join("konoma_session_store_test_proj2");
        std::fs::create_dir_all(&proj2).unwrap();
        assert!(SessionStore::with_base(base.clone(), &proj2)
            .read()
            .is_none());

        std::fs::remove_dir_all(&base).ok();
        std::fs::remove_dir_all(&proj).ok();
        std::fs::remove_dir_all(&proj2).ok();
    }

    #[test]
    fn missing_or_corrupt_file_reads_none() {
        let base = std::env::temp_dir().join("konoma_session_corrupt_test_base");
        let _ = std::fs::remove_dir_all(&base);
        let proj = std::env::temp_dir().join("konoma_session_corrupt_test_proj");
        std::fs::create_dir_all(&proj).unwrap();

        let store = SessionStore::with_base(base.clone(), &proj);
        assert!(store.read().is_none(), "ファイル無し = セッション無し");

        // 壊れた TOML は None(= まっさら起動)に安全降格する。
        std::fs::create_dir_all(store.path.parent().unwrap()).unwrap();
        std::fs::write(&store.path, "this is [not toml").unwrap();
        assert!(store.read().is_none(), "壊れたファイルはクラッシュせず無視");

        std::fs::remove_dir_all(&base).ok();
        std::fs::remove_dir_all(&proj).ok();
    }
}
