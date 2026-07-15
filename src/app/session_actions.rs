//! Tab-session save/restore (`[ui] restore_tabs`): persist the open tab set per start directory
//! and reopen it on the next launch in that directory.
//!
//! What is saved per tab: the tree **root**, the **cursor** entry, and — when the tab was left in
//! Preview — the **previewed file** (scroll/zoom are not persisted; a restored preview starts at
//! the top). Saving happens on every tab open/close/switch and on normal quit; restoring happens
//! once at startup, before the first draw. Anything stale (missing root/cursor/preview) degrades
//! silently to the nearest valid state (design principle #3: never crash, never block startup).

use std::path::{Path, PathBuf};

use crate::app::{App, Mode};
use crate::session::{SavedSession, SavedTab};

impl App {
    /// Persist the current tab set. No-op when `restore_tabs` is off or no store is attached
    /// (tests). Write errors are deliberately swallowed: losing one session snapshot must never
    /// disturb the UI, and the next tab event or quit writes again anyway.
    pub(crate) fn save_session(&self) {
        if !self.cfg.ui.restore_tabs {
            return;
        }
        let Some(store) = &self.session_store else {
            return;
        };
        let _ = store.write(self.session_snapshot());
    }

    /// Distill the live tab set into its persistent form. The active tab reads the **live App
    /// fields** (its slot in `tabs` is stale while active — the same rule as `tab_label`).
    fn session_snapshot(&self) -> SavedSession {
        let mut tabs = Vec::with_capacity(self.tabs.len());
        for (i, slot) in self.tabs.iter().enumerate() {
            let (mode, root, cursor, preview) = if i == self.active_tab {
                (
                    self.mode,
                    &self.root,
                    self.entries.get(self.selected),
                    self.preview_path.as_deref(),
                )
            } else {
                (
                    slot.mode,
                    &slot.root,
                    slot.entries.get(slot.selected),
                    slot.preview_path.as_deref(),
                )
            };
            tabs.push(SavedTab {
                root: root.to_string_lossy().to_string(),
                cursor: cursor.map(|e| e.path.to_string_lossy().to_string()),
                // プレビュー面のまま置いたタブだけ preview を持つ(Tree に戻っていたら復元もツリー)。
                preview: if mode == Mode::Preview {
                    preview.map(|p| p.to_string_lossy().to_string())
                } else {
                    None
                },
            });
        }
        SavedSession {
            dir: String::new(), // SessionStore::write が埋める
            active: self.active_tab,
            tabs,
        }
    }

    /// Restore the saved tab set for this start dir (startup only; main calls this after the
    /// loaders/image backend are attached so reopened previews can spawn their media jobs).
    /// Tabs whose root no longer exists are dropped; a session with no usable tab leaves the
    /// fresh startup tab untouched.
    pub fn restore_session(&mut self) {
        if !self.cfg.ui.restore_tabs {
            return;
        }
        let Some(store) = &self.session_store else {
            return;
        };
        let Some(sess) = store.read() else {
            return;
        };
        let tabs: Vec<SavedTab> = sess
            .tabs
            .into_iter()
            .filter(|t| Path::new(&t.root).is_dir())
            .collect();
        if tabs.is_empty() {
            return;
        }
        for (i, t) in tabs.iter().enumerate() {
            // 1枚目は起動時のタブをそのまま作り替え、2枚目以降は tab_new(現 root の素の Tree)を
            // 足してから中身を適用する(tab_new_from_selection と同じ組み立て順)。
            if i > 0 && self.tab_new().is_err() {
                break;
            }
            self.apply_saved_tab(t);
        }
        let want = sess.active.min(self.tabs.len() - 1);
        if want != self.active_tab {
            self.tab_goto(want);
        }
        // 復元し終えた完全な集合で保存し直す(復元途中の tab_new が書いた中間状態を上書き)。
        self.save_session();
    }

    /// Apply one saved tab onto the live (fresh-Tree) state: move the root if it differs, reveal
    /// the cursor, then reopen the preview (mirrors `tab_new_from_selection`).
    fn apply_saved_tab(&mut self, t: &SavedTab) {
        let root = PathBuf::from(&t.root);
        if root != self.root {
            // root 変更は `l` 潜行/Ctrl-t と同じ経路(clear_for_root_change→rebuild)。open_dir は
            // 起動 dir のまま触らない(@参照の相対基準・tab_new_from_selection と同じ)。
            self.clear_for_root_change();
            self.root = root;
            self.entries.clear();
            self.selected = 0;
            if self.rebuild_tree().is_err() {
                return; // 読めない root は素の状態のまま置く(起動を止めない)
            }
        }
        if let Some(cur) = &t.cursor {
            let p = Path::new(cur);
            if p.exists() {
                let _ = self.reveal_path_deep(p);
            }
        }
        if let Some(pv) = &t.preview {
            let p = Path::new(pv);
            // 消えたファイルはプレビューを開かずツリーのまま(原則#3)。reveal を先にやるのは
            // q でツリーへ戻ったときそのファイルの上に居るため(tab_new_from_selection と同じ)。
            if p.is_file() {
                let _ = self.reveal_path_deep(p);
                self.enter_preview(p);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::session::SessionStore;
    use std::fs;

    /// Temp project (a.txt / b.txt / sub/c.txt) + temp session base.
    fn setup(name: &str) -> (PathBuf, PathBuf) {
        let dir = std::env::temp_dir().join(name);
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("sub")).unwrap();
        fs::write(dir.join("a.txt"), "alpha\n").unwrap();
        fs::write(dir.join("b.txt"), "beta\n").unwrap();
        fs::write(dir.join("sub/c.txt"), "gamma\n").unwrap();
        let base = std::env::temp_dir().join(format!("{name}_base"));
        let _ = fs::remove_dir_all(&base);
        (dir, base)
    }

    #[test]
    fn session_restore_rebuilds_tabs_cursor_and_preview() {
        let (dir, base) = setup("konoma_sess_restore_test");
        // セッション1: タブ1= a.txt をプレビューのまま / タブ2(アクティブ)= ツリーで b.txt にカーソル。
        let mut app = App::new(dir.clone(), Config::default()).unwrap();
        app.attach_session_store(SessionStore::with_base(base.clone(), &dir));
        let a = dir.join("a.txt");
        let b = dir.join("b.txt");
        let _ = app.reveal_path_deep(&a);
        app.enter_preview(&a);
        app.tab_new().unwrap();
        let _ = app.reveal_path_deep(&b);
        app.save_session(); // 終了時保存に相当(main が run 後に呼ぶ)
        drop(app);

        // セッション2: 同じ起動 dir で復元。
        let mut app2 = App::new(dir.clone(), Config::default()).unwrap();
        app2.attach_session_store(SessionStore::with_base(base.clone(), &dir));
        app2.restore_session();
        assert_eq!(app2.tab_count(), 2, "タブ数を復元");
        assert_eq!(app2.active_tab_index(), 1, "アクティブタブも復元");
        assert_eq!(app2.mode, Mode::Tree);
        assert_eq!(app2.entries[app2.selected].path, b, "カーソル位置を復元");
        // タブ1へ切替 = a.txt のプレビューが開き直っている。
        app2.tab_goto(0);
        assert_eq!(app2.mode, Mode::Preview, "プレビュー面ごと復元");
        assert_eq!(app2.preview_path.as_deref(), Some(a.as_path()));

        fs::remove_dir_all(&dir).ok();
        fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn session_restore_skips_stale_roots_and_previews() {
        let (dir, base) = setup("konoma_sess_stale_test");
        let store = SessionStore::with_base(base.clone(), &dir);
        store
            .write(SavedSession {
                dir: String::new(),
                active: 5, // 範囲外 index はクランプされる
                tabs: vec![
                    SavedTab {
                        // 消えた root のタブは丸ごと捨てる。
                        root: dir.join("no_such_dir").to_string_lossy().into(),
                        cursor: None,
                        preview: None,
                    },
                    SavedTab {
                        root: dir.to_string_lossy().into(),
                        cursor: Some(dir.join("b.txt").to_string_lossy().into()),
                        // 消えたファイルのプレビューはツリーに降格。
                        preview: Some(dir.join("gone.txt").to_string_lossy().into()),
                    },
                ],
            })
            .unwrap();

        let mut app = App::new(dir.clone(), Config::default()).unwrap();
        app.attach_session_store(store);
        app.restore_session();
        assert_eq!(app.tab_count(), 1, "存在しない root のタブは捨てる");
        assert_eq!(
            app.mode,
            Mode::Tree,
            "プレビュー先が消えていたらツリーで開く"
        );
        assert_eq!(app.entries[app.selected].path, dir.join("b.txt"));

        fs::remove_dir_all(&dir).ok();
        fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn restore_tabs_off_neither_saves_nor_restores() {
        let (dir, base) = setup("konoma_sess_off_test");
        let mut cfg = Config::default();
        cfg.ui.restore_tabs = false;

        // OFF なら保存しない(タブ操作でもファイルが生えない)。
        let mut app = App::new(dir.clone(), cfg.clone()).unwrap();
        app.attach_session_store(SessionStore::with_base(base.clone(), &dir));
        app.tab_new().unwrap();
        app.save_session();
        assert!(!base.exists(), "restore_tabs=false は書き込みもしない");

        // ファイルが在っても OFF なら読まない。
        SessionStore::with_base(base.clone(), &dir)
            .write(SavedSession {
                dir: String::new(),
                active: 0,
                tabs: vec![
                    SavedTab {
                        root: dir.to_string_lossy().into(),
                        cursor: None,
                        preview: None,
                    },
                    SavedTab {
                        root: dir.to_string_lossy().into(),
                        cursor: None,
                        preview: None,
                    },
                ],
            })
            .unwrap();
        let mut app2 = App::new(dir.clone(), cfg).unwrap();
        app2.attach_session_store(SessionStore::with_base(base.clone(), &dir));
        app2.restore_session();
        assert_eq!(app2.tab_count(), 1, "restore_tabs=false は復元しない");

        fs::remove_dir_all(&dir).ok();
        fs::remove_dir_all(&base).ok();
    }
}
