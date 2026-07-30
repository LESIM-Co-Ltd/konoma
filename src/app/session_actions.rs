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
        // session_restoring: swallows the saves fired by tab_new/tab_goto during the restore loop
        // (never overwrite with a partial set; the full set is written exactly once at the end of
        // restore_session).
        if self.session_restoring || !self.cfg.ui.restore_tabs {
            return;
        }
        let Some(store) = &self.session_store else {
            return;
        };
        // While single-tab restore is disabled (`restore_single_tab=false`), a single-tab session
        // is deleted instead of saved = the next launch starts completely fresh (user's choice; 2+
        // tabs still save as before).
        if !self.cfg.ui.restore_single_tab && self.tabs.len() <= 1 {
            store.delete();
            return;
        }
        let _ = store.write(self.session_snapshot());
    }

    /// Distill the live tab set into its persistent form. The active tab reads the **live App
    /// fields** (its slot in `tabs` is stale while active — the same rule as `tab_label`).
    fn session_snapshot(&self) -> SavedSession {
        let mut tabs = Vec::with_capacity(self.tabs.len());
        for (i, slot) in self.tabs.iter().enumerate() {
            // The active tab is read from the live App fields (its `tabs` slot is stale while it's active — same rule as `tab_label`).
            let (mode, root, open_dir, show_hidden, cursor, preview, is_diff) =
                if i == self.active_tab {
                    (
                        self.tab.mode,
                        &self.tab.root,
                        &self.tab.open_dir,
                        self.tab.show_hidden,
                        self.tab.entries.get(self.tab.selected),
                        self.tab.preview_path.as_deref(),
                        self.is_git_diff_preview(),
                    )
                } else {
                    (
                        slot.mode,
                        &slot.root,
                        &slot.open_dir,
                        slot.show_hidden,
                        slot.entries.get(slot.selected),
                        slot.preview_path.as_deref(),
                        matches!(
                            slot.preview_kind,
                            Some(crate::preview::PreviewKind::GitDiff(_))
                        ),
                    )
                };
            let in_preview = mode == Mode::Preview;
            tabs.push(SavedTab {
                root: root.to_string_lossy().to_string(),
                cursor: cursor.map(|e| e.path.to_string_lossy().to_string()),
                // Only a tab left in the preview surface carries a preview (if it had returned to Tree, it restores as Tree too).
                preview: if in_preview {
                    preview.map(|p| p.to_string_lossy().to_string())
                } else {
                    None
                },
                // A git diff preview follows a different restore path from the normal viewer, so a flag distinguishes it.
                preview_diff: in_preview && is_diff,
                show_hidden,
                open_dir: Some(open_dir.to_string_lossy().to_string()),
            });
        }
        SavedSession {
            dir: String::new(), // filled in by SessionStore::write
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
        // While single-tab restore is disabled, an existing single-tab session file (a leftover from
        // switching the config to false later) is deleted instead of restored = this file won't be
        // pointlessly checked again on future launches.
        if !self.cfg.ui.restore_single_tab && sess.tabs.len() <= 1 {
            store.delete();
            return;
        }
        // Drop tabs whose root no longer exists. **Keep the original index** for remapping `active`.
        let kept: Vec<(usize, SavedTab)> = sess
            .tabs
            .into_iter()
            .enumerate()
            .filter(|(_, t)| Path::new(&t.root).is_dir())
            .collect();
        if kept.is_empty() {
            return;
        }
        // **Remap** `active` to its index after exclusion: the number of tabs that survived before
        // the original active becomes the new position. (If the original active itself was dropped,
        // it lands on whichever tab slid into that slot next. A plain clamp would shift focus back
        // by however many tabs were dropped ahead of it — the root cause of the restore-focus-drift bug.)
        let want = kept
            .iter()
            .take_while(|(orig, _)| *orig < sess.active)
            .count();
        // Suppress the saves fired by tab_new/tab_goto during the restore loop (don't let them overwrite with a partial set).
        self.session_restoring = true;
        for (i, (_, t)) in kept.iter().enumerate() {
            // The first tab reshapes the startup tab in place; the second and later ones are added
            // via tab_new (a plain Tree at the current root) before applying their contents (the
            // same assembly order as tab_new_from_selection).
            if i > 0 && self.tab_new().is_err() {
                break;
            }
            self.apply_saved_tab(t);
        }
        let want = want.min(self.tabs.len().saturating_sub(1));
        if want != self.active_tab {
            self.tab_goto(want);
        }
        // After lifting the suppression, save once with the **complete set** now that restore has finished.
        self.session_restoring = false;
        self.save_session();
    }

    /// Apply one saved tab onto the live (fresh-Tree) state: restore hidden-file visibility, move the
    /// root if it differs, restore the per-tab @-ref base, reveal the cursor, then reopen the preview
    /// (a git diff is reopened as a diff; mirrors `tab_new_from_selection`).
    fn apply_saved_tab(&mut self, t: &SavedTab) {
        // Apply show_hidden before rebuild (so the saved cursor can be revealed even if it's a dotfile).
        self.tab.show_hidden = t.show_hidden;
        let root = PathBuf::from(&t.root);
        let prev_root = self.tab.root.clone();
        if root != self.tab.root {
            // A root change follows the same path as `l` descend/Ctrl-t (clear_for_root_change→rebuild).
            self.clear_for_root_change();
            self.tab.root = root;
        }
        self.tab.entries.clear();
        self.tab.selected = 0;
        if self.rebuild_tree().is_err() {
            // is_dir() passes but read_dir fails (permissions/stale mount) = an unreadable root.
            // **Roll back to the last-known-good root** so a broken empty tree is neither displayed
            // nor persisted (startup never halts — principle #3).
            self.tab.root = prev_root;
            self.tab.entries.clear();
            self.tab.selected = 0;
            let _ = self.rebuild_tree();
            return;
        }
        // Precisely restore the per-tab @-reference base (open_dir) (normally the startup dir; a descended tab has a different root).
        if let Some(od) = &t.open_dir {
            self.tab.open_dir = PathBuf::from(od);
        }
        if let Some(cur) = &t.cursor {
            let p = Path::new(cur);
            if p.exists() {
                let _ = self.reveal_path_deep(p);
            }
        }
        if let Some(pv) = &t.preview {
            let p = Path::new(pv);
            // A file that's gone stays as a tree instead of opening a preview (principle #3). Reveal
            // runs first so that returning to the tree via `q` lands on that file (same as tab_new_from_selection).
            if p.is_file() {
                let _ = self.reveal_path_deep(p);
                if t.preview_diff {
                    // Reproduce the full-screen git diff that was being viewed. Falls back to the
                    // normal preview if there is no diff (since committed / a no-git build) —
                    // the same judgment as follow_jump.
                    let diff = crate::git::file_diff(&self.tab.root, p);
                    if !diff.is_empty() {
                        self.open_git_diff(p);
                        self.diff_cache = Some(super::DiffCache {
                            path: p.to_path_buf(),
                            lines: diff,
                        });
                    } else {
                        self.enter_preview(p);
                    }
                } else {
                    self.enter_preview(p);
                }
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
        // Session 1: tab 1 = a.txt left in preview / tab 2 (active) = tree with the cursor on b.txt.
        let mut app = App::new(dir.clone(), Config::default()).unwrap();
        app.attach_session_store(SessionStore::with_base(base.clone(), &dir));
        let a = dir.join("a.txt");
        let b = dir.join("b.txt");
        let _ = app.reveal_path_deep(&a);
        app.enter_preview(&a);
        app.tab_new().unwrap();
        let _ = app.reveal_path_deep(&b);
        app.save_session(); // corresponds to the on-quit save (main calls this after run)
        drop(app);

        // Session 2: restore in the same startup dir.
        let mut app2 = App::new(dir.clone(), Config::default()).unwrap();
        app2.attach_session_store(SessionStore::with_base(base.clone(), &dir));
        app2.restore_session();
        assert_eq!(app2.tab_count(), 2, "タブ数を復元");
        assert_eq!(app2.active_tab_index(), 1, "アクティブタブも復元");
        assert_eq!(app2.tab.mode, Mode::Tree);
        assert_eq!(
            app2.tab.entries[app2.tab.selected].path, b,
            "カーソル位置を復元"
        );
        // Switch to tab 1 = a.txt's preview reopens.
        app2.tab_goto(0);
        assert_eq!(app2.tab.mode, Mode::Preview, "プレビュー面ごと復元");
        assert_eq!(app2.tab.preview_path.as_deref(), Some(a.as_path()));

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
                active: 5, // an out-of-range index gets clamped
                tabs: vec![
                    SavedTab {
                        // A tab whose root is gone is discarded entirely.
                        root: dir.join("no_such_dir").to_string_lossy().into(),
                        cursor: None,
                        preview: None,
                        ..Default::default()
                    },
                    SavedTab {
                        root: dir.to_string_lossy().into(),
                        cursor: Some(dir.join("b.txt").to_string_lossy().into()),
                        // A preview of a file that's gone degrades to the tree.
                        preview: Some(dir.join("gone.txt").to_string_lossy().into()),
                        ..Default::default()
                    },
                ],
            })
            .unwrap();

        let mut app = App::new(dir.clone(), Config::default()).unwrap();
        app.attach_session_store(store);
        app.restore_session();
        assert_eq!(app.tab_count(), 1, "存在しない root のタブは捨てる");
        assert_eq!(
            app.tab.mode,
            Mode::Tree,
            "プレビュー先が消えていたらツリーで開く"
        );
        assert_eq!(app.tab.entries[app.tab.selected].path, dir.join("b.txt"));

        fs::remove_dir_all(&dir).ok();
        fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn restore_tabs_off_neither_saves_nor_restores() {
        let (dir, base) = setup("konoma_sess_off_test");
        let mut cfg = Config::default();
        cfg.ui.restore_tabs = false;

        // When OFF, it saves nothing (no file appears even after tab operations).
        let mut app = App::new(dir.clone(), cfg.clone()).unwrap();
        app.attach_session_store(SessionStore::with_base(base.clone(), &dir));
        app.tab_new().unwrap();
        app.save_session();
        assert!(!base.exists(), "restore_tabs=false は書き込みもしない");

        // Even if a file exists, it isn't read when OFF.
        SessionStore::with_base(base.clone(), &dir)
            .write(SavedSession {
                dir: String::new(),
                active: 0,
                tabs: vec![
                    SavedTab {
                        root: dir.to_string_lossy().into(),
                        cursor: None,
                        preview: None,
                        ..Default::default()
                    },
                    SavedTab {
                        root: dir.to_string_lossy().into(),
                        cursor: None,
                        preview: None,
                        ..Default::default()
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

    /// #1: dropping a stale-root tab that precedes the active one must remap the active index,
    /// not just clamp it (else focus lands on the wrong tab).
    #[test]
    fn session_restore_remaps_active_index_past_dropped_tabs() {
        let (dir, base) = setup("konoma_sess_remap_test");
        let store = SessionStore::with_base(base.clone(), &dir);
        // 3 tabs: [A(root=gone), B(=dir, active), C(root=dir/sub)]. Dropping A makes B the new index 0.
        store
            .write(SavedSession {
                dir: String::new(),
                active: 1, // B
                tabs: vec![
                    SavedTab {
                        root: dir.join("gone").to_string_lossy().into(),
                        ..Default::default()
                    },
                    SavedTab {
                        root: dir.to_string_lossy().into(),
                        cursor: Some(dir.join("b.txt").to_string_lossy().into()),
                        ..Default::default()
                    },
                    SavedTab {
                        root: dir.join("sub").to_string_lossy().into(),
                        ..Default::default()
                    },
                ],
            })
            .unwrap();

        let mut app = App::new(dir.clone(), Config::default()).unwrap();
        app.attach_session_store(store);
        app.restore_session();
        assert_eq!(app.tab_count(), 2, "生き残りは B と C");
        assert_eq!(
            app.active_tab_index(),
            0,
            "元 active(B)へ焦点=先頭の落ちたぶんずらす"
        );
        assert_eq!(
            app.tab.entries[app.tab.selected].path,
            dir.join("b.txt"),
            "B のカーソルが乗っている"
        );

        fs::remove_dir_all(&dir).ok();
        fs::remove_dir_all(&base).ok();
    }

    /// #3/#4: show_hidden and open_dir are persisted and restored — a dotfile cursor survives, and a
    /// non-first restored tab keeps its own @-ref base instead of inheriting tab_new's clobbered value.
    #[test]
    fn session_restore_reproduces_hidden_and_open_dir() {
        let (dir, base) = setup("konoma_sess_hidden_opendir_test");
        fs::write(dir.join(".secret"), "hidden\n").unwrap();
        fs::create_dir_all(dir.join("sub")).unwrap();

        // tab0: root=dir/sub (descended). tab1: root=dir, hidden on, cursor on .secret, open_dir=dir.
        let store = SessionStore::with_base(base.clone(), &dir);
        store
            .write(SavedSession {
                dir: String::new(),
                active: 1,
                tabs: vec![
                    SavedTab {
                        root: dir.join("sub").to_string_lossy().into(),
                        ..Default::default()
                    },
                    SavedTab {
                        root: dir.to_string_lossy().into(),
                        cursor: Some(dir.join(".secret").to_string_lossy().into()),
                        show_hidden: true,
                        open_dir: Some(dir.to_string_lossy().into()),
                        ..Default::default()
                    },
                ],
            })
            .unwrap();

        let mut app = App::new(dir.clone(), Config::default()).unwrap();
        app.attach_session_store(store);
        app.restore_session();
        assert_eq!(app.active_tab_index(), 1);
        assert!(app.tab.show_hidden, "#3: show_hidden を復元");
        assert_eq!(
            app.tab.entries[app.tab.selected].path,
            dir.join(".secret"),
            "#3: ドットファイルのカーソルが復元される(hidden off だと見つからず落ちていた)"
        );
        assert_eq!(
            app.tab.open_dir, dir,
            "#4: 起動 dir の @参照基準を復元(前タブ root=sub を継がない)"
        );

        fs::remove_dir_all(&dir).ok();
        fs::remove_dir_all(&base).ok();
    }

    /// #6: the per-op writes fired by tab_new during restore are suppressed (session_restoring guard),
    /// so a crash mid-restore can't overwrite the file with a partial set.
    #[test]
    fn save_session_suppressed_while_restoring() {
        let (dir, base) = setup("konoma_sess_guard_test");
        let mut app = App::new(dir.clone(), Config::default()).unwrap();
        app.attach_session_store(SessionStore::with_base(base.clone(), &dir));
        app.session_restoring = true;
        app.save_session();
        assert!(
            !base.join("sessions").exists(),
            "復元中(session_restoring)は保存を握り潰す"
        );
        app.session_restoring = false;
        app.save_session();
        assert!(
            SessionStore::with_base(base.clone(), &dir).read().is_some(),
            "抑止解除後は保存される"
        );

        fs::remove_dir_all(&dir).ok();
        fs::remove_dir_all(&base).ok();
    }

    /// #5: a saved root that passes is_dir() but is unreadable (read_dir fails) must roll back to the
    /// launch dir, not leave/persist a broken empty tree.
    #[cfg(unix)]
    #[test]
    fn session_restore_rolls_back_unreadable_root() {
        use std::os::unix::fs::PermissionsExt;
        let (dir, base) = setup("konoma_sess_unreadable_test");
        let bad = dir.join("locked");
        fs::create_dir_all(&bad).unwrap();
        let store = SessionStore::with_base(base.clone(), &dir);
        store
            .write(SavedSession {
                active: 0,
                tabs: vec![SavedTab {
                    root: bad.to_string_lossy().into(),
                    ..Default::default()
                }],
                ..Default::default()
            })
            .unwrap();
        // 000 = is_dir() (a stat via the parent) passes, but read_dir fails.
        fs::set_permissions(&bad, fs::Permissions::from_mode(0o000)).unwrap();
        let readable_as_root = fs::read_dir(&bad).is_ok(); // running as root can read it anyway → skip verification

        let mut app = App::new(dir.clone(), Config::default()).unwrap();
        app.attach_session_store(store);
        app.restore_session();
        if !readable_as_root {
            assert_eq!(app.tab_count(), 1);
            assert_eq!(
                app.tab.root, dir,
                "#5: 読めない root はロールバックして起動 dir に留まる"
            );
            assert!(
                app.tab.entries.iter().any(|e| e.path.ends_with("a.txt")),
                "起動 dir のツリーが生きている(壊れた空ツリーにしない)"
            );
        }

        let _ = fs::set_permissions(&bad, fs::Permissions::from_mode(0o755)); // restore so it can be cleaned up
        fs::remove_dir_all(&dir).ok();
        fs::remove_dir_all(&base).ok();
    }

    /// `restore_single_tab=false` + a lone tab: `save_session` must delete any existing file rather
    /// than write one, so the next launch starts fresh instead of reopening a single unremarkable tab.
    #[test]
    fn restore_single_tab_false_deletes_a_lone_tab_session() {
        let (dir, base) = setup("konoma_sess_single_off_delete_test");
        let mut cfg = Config::default();
        cfg.ui.restore_single_tab = false;
        let mut app = App::new(dir.clone(), cfg).unwrap();
        let store = SessionStore::with_base(base.clone(), &dir);
        app.attach_session_store(store);
        // Start from a state where a previous session exists (to confirm the leftover file is actually deleted).
        SessionStore::with_base(base.clone(), &dir)
            .write(SavedSession {
                dir: String::new(),
                active: 0,
                tabs: vec![SavedTab {
                    root: dir.to_string_lossy().into(),
                    ..Default::default()
                }],
            })
            .unwrap();
        assert!(app.tabs.len() <= 1, "前提: 起動直後はタブ1枚");
        app.save_session();
        assert!(
            SessionStore::with_base(base.clone(), &dir).read().is_none(),
            "restore_single_tab=false のタブ1枚は保存せず既存ファイルも削除する"
        );

        fs::remove_dir_all(&dir).ok();
        fs::remove_dir_all(&base).ok();
    }

    /// `restore_single_tab=false` + 2 tabs: multi-tab sessions still save/restore normally.
    #[test]
    fn restore_single_tab_false_still_saves_multi_tab() {
        let (dir, base) = setup("konoma_sess_single_off_multi_test");
        let mut cfg = Config::default();
        cfg.ui.restore_single_tab = false;
        let mut app = App::new(dir.clone(), cfg).unwrap();
        app.attach_session_store(SessionStore::with_base(base.clone(), &dir));
        app.tab_new().unwrap();
        assert_eq!(app.tabs.len(), 2, "前提: タブ2枚");
        app.save_session();
        let saved = SessionStore::with_base(base.clone(), &dir).read();
        assert!(
            saved.is_some(),
            "restore_single_tab=false でも2タブ以上は保存する"
        );
        assert_eq!(saved.unwrap().tabs.len(), 2);

        fs::remove_dir_all(&dir).ok();
        fs::remove_dir_all(&base).ok();
    }

    /// Default (`restore_single_tab=true`): a lone tab still saves — current behavior preserved.
    #[test]
    fn restore_single_tab_true_saves_a_lone_tab() {
        let (dir, base) = setup("konoma_sess_single_on_test");
        let cfg = Config::default();
        assert!(cfg.ui.restore_single_tab, "既定は true");
        let mut app = App::new(dir.clone(), cfg).unwrap();
        app.attach_session_store(SessionStore::with_base(base.clone(), &dir));
        assert!(app.tabs.len() <= 1, "前提: タブ1枚");
        app.save_session();
        let saved = SessionStore::with_base(base.clone(), &dir).read();
        assert!(
            saved.is_some(),
            "restore_single_tab=true(既定)はタブ1枚でも保存する"
        );

        fs::remove_dir_all(&dir).ok();
        fs::remove_dir_all(&base).ok();
    }

    /// Restore side: a stale single-tab session file present while `restore_single_tab=false` is
    /// not restored (fresh startup tab is left untouched) and the stale file is deleted.
    #[test]
    fn restore_single_tab_false_skips_and_deletes_stale_single_tab_file() {
        let (dir, base) = setup("konoma_sess_single_off_restore_test");
        let store = SessionStore::with_base(base.clone(), &dir);
        store
            .write(SavedSession {
                dir: String::new(),
                active: 0,
                tabs: vec![SavedTab {
                    root: dir.to_string_lossy().into(),
                    cursor: Some(dir.join("b.txt").to_string_lossy().into()),
                    ..Default::default()
                }],
            })
            .unwrap();

        let mut cfg = Config::default();
        cfg.ui.restore_single_tab = false;
        let mut app = App::new(dir.clone(), cfg).unwrap();
        app.attach_session_store(SessionStore::with_base(base.clone(), &dir));
        app.restore_session();
        assert_eq!(
            app.tab_count(),
            1,
            "1タブの保存済みセッションは復元せず起動直後のタブのまま"
        );
        assert!(
            app.tab.entries[app.tab.selected].path != dir.join("b.txt"),
            "保存済みカーソルへは動かない(復元しなかった)"
        );
        assert!(
            SessionStore::with_base(base.clone(), &dir).read().is_none(),
            "残骸の1タブ・セッションファイルは削除される"
        );

        fs::remove_dir_all(&dir).ok();
        fs::remove_dir_all(&base).ok();
    }

    /// #2: a tab left showing a git diff restores as a diff, not a plain content preview.
    #[cfg(feature = "git")]
    #[test]
    fn session_restore_reopens_git_diff_preview() {
        let (dir, base) = setup("konoma_sess_gitdiff_test");
        // init a repo, commit a.txt, then modify it so it has a diff.
        let sh = |args: &[&str]| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(&dir)
                .output()
                .unwrap();
        };
        sh(&["init", "-q"]);
        sh(&["config", "user.email", "t@t"]);
        sh(&["config", "user.name", "t"]);
        sh(&["add", "a.txt"]);
        sh(&["commit", "-qm", "init"]);
        fs::write(dir.join("a.txt"), "alpha\nCHANGED\n").unwrap();

        // session1: open a.txt as a git diff, save.
        let mut app = App::new(dir.clone(), Config::default()).unwrap();
        app.attach_session_store(SessionStore::with_base(base.clone(), &dir));
        let a = dir.join("a.txt");
        let _ = app.reveal_path_deep(&a);
        app.open_git_diff(&a);
        assert!(app.is_git_diff_preview(), "前提: diff プレビュー");
        app.save_session();
        drop(app);

        // session2: restore → still a git diff, not a plain preview.
        let mut app2 = App::new(dir.clone(), Config::default()).unwrap();
        app2.attach_session_store(SessionStore::with_base(base.clone(), &dir));
        app2.restore_session();
        assert!(matches!(app2.tab.mode, Mode::Preview));
        assert!(
            app2.is_git_diff_preview(),
            "#2: git diff タブが diff で復元される(素のプレビューに落ちない)"
        );

        fs::remove_dir_all(&dir).ok();
        fs::remove_dir_all(&base).ok();
    }
}
