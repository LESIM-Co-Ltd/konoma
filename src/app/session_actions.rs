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
        // session_restoring: 復元ループ中の tab_new/tab_goto が撃つ保存を握り潰す(部分集合で
        // 上書きしない・完全集合の書き込みは restore_session 末尾で1回だけ)。
        if self.session_restoring || !self.cfg.ui.restore_tabs {
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
            // アクティブタブはライブ App fields から読む(tabs スロットは在職中 stale=tab_label と同じ規則)。
            let (mode, root, open_dir, show_hidden, cursor, preview, is_diff) =
                if i == self.active_tab {
                    (
                        self.mode,
                        &self.root,
                        &self.open_dir,
                        self.show_hidden,
                        self.entries.get(self.selected),
                        self.preview_path.as_deref(),
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
                // プレビュー面のまま置いたタブだけ preview を持つ(Tree に戻っていたら復元もツリー)。
                preview: if in_preview {
                    preview.map(|p| p.to_string_lossy().to_string())
                } else {
                    None
                },
                // git diff プレビューは通常ビューアと復元経路が違うのでフラグで区別する。
                preview_diff: in_preview && is_diff,
                show_hidden,
                open_dir: Some(open_dir.to_string_lossy().to_string()),
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
        // 存在しない root のタブを落とす。**元 index を保持**して active の再マップに使う。
        let kept: Vec<(usize, SavedTab)> = sess
            .tabs
            .into_iter()
            .enumerate()
            .filter(|(_, t)| Path::new(&t.root).is_dir())
            .collect();
        if kept.is_empty() {
            return;
        }
        // active を除外後の index へ**再マップ**する: 元 active より前に生き残ったタブ数が新しい位置。
        // (元 active 自身が落ちていた場合はその位置へ滑り込んだ次のタブに乗る。単純クランプだと
        //  先頭側に落ちたタブがあるぶん焦点が後ろへずれる — restore 焦点ずれバグの真因。)
        let want = kept
            .iter()
            .take_while(|(orig, _)| *orig < sess.active)
            .count();
        // 復元ループ中の tab_new/tab_goto が撃つ保存を抑止(部分集合で上書きさせない)。
        self.session_restoring = true;
        for (i, (_, t)) in kept.iter().enumerate() {
            // 1枚目は起動時のタブをそのまま作り替え、2枚目以降は tab_new(現 root の素の Tree)を
            // 足してから中身を適用する(tab_new_from_selection と同じ組み立て順)。
            if i > 0 && self.tab_new().is_err() {
                break;
            }
            self.apply_saved_tab(t);
        }
        let want = want.min(self.tabs.len().saturating_sub(1));
        if want != self.active_tab {
            self.tab_goto(want);
        }
        // 抑止を解除してから、復元し終えた**完全な集合**で1回だけ保存する。
        self.session_restoring = false;
        self.save_session();
    }

    /// Apply one saved tab onto the live (fresh-Tree) state: restore hidden-file visibility, move the
    /// root if it differs, restore the per-tab @-ref base, reveal the cursor, then reopen the preview
    /// (a git diff is reopened as a diff; mirrors `tab_new_from_selection`).
    fn apply_saved_tab(&mut self, t: &SavedTab) {
        // show_hidden は rebuild の前に適用する(保存カーソルがドットファイルでも reveal できるように)。
        self.show_hidden = t.show_hidden;
        let root = PathBuf::from(&t.root);
        let prev_root = self.root.clone();
        if root != self.root {
            // root 変更は `l` 潜行/Ctrl-t と同じ経路(clear_for_root_change→rebuild)。
            self.clear_for_root_change();
            self.root = root;
        }
        self.entries.clear();
        self.selected = 0;
        if self.rebuild_tree().is_err() {
            // is_dir() は通るが read_dir 失敗(権限/stale mount)= 読めない root。**直前の良い root へ
            // ロールバック**して、壊れた空ツリーを表示も永続化もしない(起動を止めない・原則#3)。
            self.root = prev_root;
            self.entries.clear();
            self.selected = 0;
            let _ = self.rebuild_tree();
            return;
        }
        // per-tab の @参照基準(open_dir)を正確に復元する(通常は起動 dir・潜行タブは別 root)。
        if let Some(od) = &t.open_dir {
            self.open_dir = PathBuf::from(od);
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
                if t.preview_diff {
                    // 見ていた全画面 git diff を再現する。diff が無い(以後コミット済み/no-git ビルド)
                    // なら通常プレビューへフォールバック(follow_jump と同じ判定)。
                    let diff = crate::git::file_diff(&self.root, p);
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
                        ..Default::default()
                    },
                    SavedTab {
                        root: dir.to_string_lossy().into(),
                        cursor: Some(dir.join("b.txt").to_string_lossy().into()),
                        // 消えたファイルのプレビューはツリーに降格。
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
        // 3 tabs: [A(root=消), B(=dir, active), C(root=dir/sub)]. A を落とすと B が新 index 0。
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
            app.entries[app.selected].path,
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
        assert!(app.show_hidden, "#3: show_hidden を復元");
        assert_eq!(
            app.entries[app.selected].path,
            dir.join(".secret"),
            "#3: ドットファイルのカーソルが復元される(hidden off だと見つからず落ちていた)"
        );
        assert_eq!(
            app.open_dir, dir,
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
        // 000 = is_dir()(親経由の stat)は通るが read_dir は失敗する。
        fs::set_permissions(&bad, fs::Permissions::from_mode(0o000)).unwrap();
        let readable_as_root = fs::read_dir(&bad).is_ok(); // root 実行時は読めてしまう→検証を飛ばす

        let mut app = App::new(dir.clone(), Config::default()).unwrap();
        app.attach_session_store(store);
        app.restore_session();
        if !readable_as_root {
            assert_eq!(app.tab_count(), 1);
            assert_eq!(
                app.root, dir,
                "#5: 読めない root はロールバックして起動 dir に留まる"
            );
            assert!(
                app.entries.iter().any(|e| e.path.ends_with("a.txt")),
                "起動 dir のツリーが生きている(壊れた空ツリーにしない)"
            );
        }

        let _ = fs::set_permissions(&bad, fs::Permissions::from_mode(0o755)); // 掃除できるよう戻す
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
        assert!(matches!(app2.mode, Mode::Preview));
        assert!(
            app2.is_git_diff_preview(),
            "#2: git diff タブが diff で復元される(素のプレビューに落ちない)"
        );

        fs::remove_dir_all(&dir).ok();
        fs::remove_dir_all(&base).ok();
    }
}
