use super::bookmark_actions::fuzzy_filter_pool;
use super::*;
use crate::config::Config;

/// A unique temp directory per call (pid + a process-global counter). Tests that share a setup
/// helper (e.g. `app_with_table`) must not reuse one fixed path, or parallel runs collide and
/// flake — especially on a busy CI runner.
fn unique_tmp(prefix: &str) -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("{prefix}_{}_{n}", std::process::id()))
}

/// Test helper: the link target of an `MdItem` (panics if the item is a checkbox).
fn item_target(it: &MdItem) -> &str {
    match &it.kind {
        MdItemKind::Link { target } => target,
        MdItemKind::Task { .. } => panic!("expected a link item"),
        MdItemKind::CodeBlock => panic!("expected a link item"),
        MdItemKind::MermaidFence { .. } => panic!("expected a link item"),
        MdItemKind::Details { .. } => panic!("expected a link item"),
    }
}

#[test]
fn path_styles_format_as_expected() {
    let open = std::env::temp_dir().join("konoma_app_test_open");
    std::fs::create_dir_all(&open).unwrap();
    let mut app = App::new(open.clone(), Config::default()).unwrap();
    let file = open.join("src").join("main.rs");

    app.path_style = PathStyle::Full;
    assert_eq!(app.format_path(&file), file.display().to_string());

    // Relative: prefixes the launch directory name.
    app.path_style = PathStyle::Relative;
    assert_eq!(
        app.format_path(&file),
        "konoma_app_test_open/src/main.rs".to_string()
    );

    // Home-relative: a path under HOME becomes ~/....
    if let Some(home) = std::env::var_os("HOME") {
        let p = PathBuf::from(home).join("work").join("konoma");
        app.path_style = PathStyle::Home;
        assert_eq!(app.format_path(&p), "~/work/konoma".to_string());
    }

    std::fs::remove_dir_all(&open).ok();
}

#[test]
fn format_path_relative_uses_dotdot_outside_open_dir() {
    // open_dir = .../work/A. Paths under it get the launch dir name prefixed (A/x.rs);
    // paths outside it (siblings/ancestors) are shown with a leading `..`.
    let work = std::env::temp_dir().join("konoma_relpath_test_work");
    let _ = std::fs::remove_dir_all(&work);
    let a = work.join("A");
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(work.join("B")).unwrap();
    let mut app = App::new(a.clone(), Config::default()).unwrap();
    app.path_style = PathStyle::Relative;

    assert_eq!(
        app.format_path(&a.join("x.rs")),
        "A/x.rs",
        "配下は起動dir名を先頭に"
    );
    assert_eq!(
        app.format_path(&work.join("B").join("aaa.md")),
        "../B/aaa.md",
        "兄弟ディレクトリは ../ を付ける"
    );
    assert_eq!(app.format_path(&work), "..", "上位は ..");

    std::fs::remove_dir_all(&work).ok();
}

#[test]
fn dialog_create_rename_and_delete_is_gated() {
    let dir = std::env::temp_dir().join("konoma_dialog_ops_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.txt"), b"x").unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.rebuild_tree().unwrap();
    // Cursor is on a.txt (a file). The creation target is its parent = dir.
    assert_eq!(app.tab.entries[app.tab.selected].path, dir.join("a.txt"));

    // --- Create (file): input dialog → confirming creates the file and selects it.
    app.start_create();
    assert!(
        app.is_dialog() && !app.dialog_is_confirm(),
        "入力ダイアログ"
    );
    for c in "new.txt".chars() {
        app.dialog_input_push(c);
    }
    app.dialog_submit().unwrap();
    assert!(!app.is_dialog(), "確定でダイアログは閉じる");
    assert!(dir.join("new.txt").is_file(), "ファイルが作成される");
    assert_eq!(
        app.tab.entries[app.tab.selected].path,
        dir.join("new.txt"),
        "作成物が選択される"
    );

    // --- Create (folder): a trailing / makes it a directory.
    app.start_create();
    for c in "sub/".chars() {
        app.dialog_input_push(c);
    }
    app.dialog_submit().unwrap();
    assert!(dir.join("sub").is_dir(), "末尾 / でフォルダ作成");

    // --- Rename: current name is prefilled → change it and it takes effect.
    let ai = app
        .tab
        .entries
        .iter()
        .position(|e| e.path == dir.join("a.txt"))
        .unwrap();
    app.tab.selected = ai;
    app.start_rename();
    assert_eq!(
        app.dialog_view().map(|v| v.2.to_string()),
        Some("a.txt".to_string()),
        "現在名がプリフィル"
    );
    for _ in 0.."a.txt".len() {
        app.dialog_input_backspace();
    }
    for c in "renamed.txt".chars() {
        app.dialog_input_push(c);
    }
    app.dialog_submit().unwrap();
    assert!(
        dir.join("renamed.txt").is_file() && !dir.join("a.txt").exists(),
        "リネームが反映される"
    );

    // --- Delete is gated by confirmation: n cancels → the file remains (trash stays clean).
    let ri = app
        .tab
        .entries
        .iter()
        .position(|e| e.path == dir.join("renamed.txt"))
        .unwrap();
    app.tab.selected = ri;
    app.start_delete();
    assert!(
        app.is_dialog() && app.dialog_is_confirm(),
        "削除は y/n 確認"
    );
    app.dialog_confirm(false).unwrap(); // n=cancel
    assert!(!app.is_dialog(), "確認後ダイアログは閉じる");
    assert!(
        dir.join("renamed.txt").is_file(),
        "キャンセルでファイルは残る"
    );

    // --- Creating with an existing name fails (no overwrite). Dialog closes, failure is notified.
    app.start_create();
    for c in "renamed.txt".chars() {
        app.dialog_input_push(c);
    }
    app.dialog_submit().unwrap();
    assert!(!app.is_dialog());
    assert!(app.flash.is_some(), "衝突は失敗として通知される");

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn dialog_input_cursor_moves_and_edits_midstring() {
    let dir = std::env::temp_dir().join("konoma_dialog_cursor_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.txt"), b"x").unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.rebuild_tree().unwrap();
    let buf = |app: &App| app.dialog_view().map(|v| v.2.to_string()).unwrap();

    // "ac" → ← to the middle → insert 'b' → "abc".
    app.start_create();
    app.dialog_input_push('a');
    app.dialog_input_push('c');
    app.dialog_cursor_left();
    app.dialog_input_push('b');
    assert_eq!(buf(&app), "abc", "カーソル位置に中間挿入");
    // Backspace deletes the char right before ('b') → "ac".
    app.dialog_input_backspace();
    assert_eq!(buf(&app), "ac");
    // Home → Delete deletes the char at the cursor ('a') → "c".
    app.dialog_cursor_home();
    app.dialog_input_delete();
    assert_eq!(buf(&app), "c");
    app.dialog_cancel();

    // Multibyte (Japanese): inserting "x" before "あ" must not break byte boundaries.
    app.start_create();
    app.dialog_input_push('あ');
    app.dialog_cursor_left();
    app.dialog_input_push('x');
    assert_eq!(buf(&app), "xあ", "マルチバイトでも中間挿入できる");
    app.dialog_cancel();

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn single_toggle_picks_scattered_items() {
    let dir = std::env::temp_dir().join("konoma_single_toggle_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for n in ["a.txt", "b.txt", "c.txt"] {
        std::fs::write(dir.join(n), b"x").unwrap();
    }
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.rebuild_tree().unwrap();

    // V: toggle one item (a) and move down. a is selected, cursor=b.
    assert!(!app.has_selection());
    app.tab.selected = 0;
    app.toggle_select();
    assert!(app.is_selected(&dir.join("a.txt")));
    assert_eq!(app.tab.selected, 1, "V は下へ進む");
    // Gap: skip b and select c (move the cursor to c, as if via j/k).
    app.tab.selected = 2;
    app.toggle_select();
    assert_eq!(app.marked_count(), 2);
    assert!(app.is_selected(&dir.join("c.txt")) && !app.is_selected(&dir.join("b.txt")));
    // Toggling the same item again deselects it.
    app.tab.selected = 0;
    app.toggle_select();
    assert!(!app.is_selected(&dir.join("a.txt")));
    assert_eq!(app.marked_count(), 1);

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn visual_range_selects_and_batch_deletes() {
    let dir = std::env::temp_dir().join("konoma_visual_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for n in ["a.txt", "b.txt", "c.txt", "d.txt"] {
        std::fs::write(dir.join(n), b"x").unwrap();
    }
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.rebuild_tree().unwrap();
    assert_eq!(app.tab.entries.len(), 4);

    // Visual: start at a → moving down to c makes a..c the range (live).
    app.tab.selected = 0;
    app.enter_visual();
    assert!(app.is_visual());
    app.tree_next(); // b
    app.tree_next(); // c
    assert!(app.is_in_visual_range(0) && app.is_in_visual_range(2));
    assert!(!app.is_in_visual_range(3), "d は範囲外");
    assert_eq!(app.marked_count(), 3, "範囲(a..c)で3件");
    // Commit (v) → folds the range into the selection set and exits visual.
    app.exit_visual_commit();
    assert!(!app.is_visual());
    assert_eq!(app.marked_count(), 3);
    assert!(app.is_selected(&dir.join("a.txt")) && app.is_selected(&dir.join("c.txt")));

    // Permanently delete all 3 selected items in one batch → only d remains, selection clears.
    app.start_delete();
    assert!(app.dialog_is_confirm());
    app.dialog_delete_permanent().unwrap();
    assert!(dir.join("d.txt").exists() && !dir.join("a.txt").exists());
    assert!(!app.has_selection(), "一括操作後は選択クリア");

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn visual_scope_a_selects_same_dir_level_only() {
    let dir = std::env::temp_dir().join("konoma_visual_scope_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    std::fs::write(dir.join("a.txt"), b"x").unwrap();
    std::fs::write(dir.join("b.txt"), b"x").unwrap();
    std::fs::write(dir.join("sub").join("inner.txt"), b"x").unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.rebuild_tree().unwrap();
    // Expand sub to reveal inner (to verify same-level detection).
    let sub_idx = app
        .tab
        .entries
        .iter()
        .position(|e| e.path == dir.join("sub"))
        .unwrap();
    app.tab.selected = sub_idx;
    app.tree_activate().unwrap(); // expand
    app.rebuild_tree().unwrap();

    // At the top level (a.txt), a=select-all-at-same-level → a.txt/b.txt/sub are included but
    // inner is not.
    let a_idx = app
        .tab
        .entries
        .iter()
        .position(|e| e.path == dir.join("a.txt"))
        .unwrap();
    app.tab.selected = a_idx;
    app.enter_visual();
    app.visual_select_scope(false); // a=same level
    assert!(app.is_selected(&dir.join("a.txt")));
    assert!(app.is_selected(&dir.join("b.txt")));
    assert!(app.is_selected(&dir.join("sub")));
    assert!(
        !app.is_selected(&dir.join("sub").join("inner.txt")),
        "入れ子(別階層)は a では入らない"
    );
    assert!(!app.is_visual(), "スコープ選択後はビジュアルを抜ける");

    // A=everything visible, so inner is included too.
    app.clear_selection();
    app.tab.selected = a_idx;
    app.enter_visual();
    app.visual_select_scope(true);
    assert!(
        app.is_selected(&dir.join("sub").join("inner.txt")),
        "A は入れ子も"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn batch_rename_numbers_in_sort_order_and_keeps_ext() {
    let dir = std::env::temp_dir().join("konoma_batchrename_app_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // Sorting by name ascending gives the order apple.md, mango.rs, zebra.txt.
    std::fs::write(dir.join("zebra.txt"), b"z").unwrap();
    std::fs::write(dir.join("apple.md"), b"a").unwrap();
    std::fs::write(dir.join("mango.rs"), b"m").unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.rebuild_tree().unwrap();
    app.visual_select_scope(true);
    assert_eq!(app.marked_count(), 3);

    // Batch rename: "img_{n}" → goes to preview.
    app.start_batch_rename();
    assert!(app.is_dialog() && !app.dialog_is_confirm() && !app.dialog_is_preview());
    for c in "img_{n}".chars() {
        app.dialog_input_push(c);
    }
    app.dialog_submit().unwrap();
    assert!(app.dialog_is_preview(), "確定でプレビューへ遷移");
    // The sequence numbers follow **sort order** (apple, mango, zebra); extensions are kept automatically.
    let (_, pairs, _) = app.dialog_preview_view().unwrap();
    assert_eq!(pairs[0], "apple.md  →  img_1.md");
    assert_eq!(pairs[1], "mango.rs  →  img_2.rs");
    assert_eq!(pairs[2], "zebra.txt  →  img_3.txt");

    // Apply.
    app.dialog_preview_apply().unwrap();
    assert!(!app.is_dialog());
    assert!(dir.join("img_1.md").is_file());
    assert!(dir.join("img_2.rs").is_file());
    assert!(dir.join("img_3.txt").is_file());
    assert!(!dir.join("apple.md").exists());
    assert!(!app.has_selection(), "適用後は選択クリア");

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn batch_rename_collision_reopens_input() {
    let dir = std::env::temp_dir().join("konoma_batchrename_collision_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.txt"), b"a").unwrap();
    std::fs::write(dir.join("b.txt"), b"b").unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.rebuild_tree().unwrap();
    app.visual_select_scope(true);

    // A template without {n} → both entries collide on "dup.txt".
    app.start_batch_rename();
    for c in "dup".chars() {
        app.dialog_input_push(c);
    }
    app.dialog_submit().unwrap();
    // Does not advance to preview; stays on the input dialog (template kept) and reports failure.
    assert!(!app.dialog_is_preview(), "衝突時はプレビューへ行かない");
    assert!(
        app.is_dialog() && !app.dialog_is_confirm(),
        "入力ダイアログのまま"
    );
    assert_eq!(
        app.dialog_view().map(|v| v.2.to_string()),
        Some("dup".to_string()),
        "テンプレは保持される"
    );
    assert!(app.flash.is_some(), "衝突は失敗として通知");
    // The files are unchanged.
    assert!(dir.join("a.txt").exists() && dir.join("b.txt").exists());

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn parse_dropped_paths_unescapes_splits_and_filters() {
    let tmp = std::env::temp_dir().join("konoma_parse_drop_test");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let a = tmp.join("a b.txt"); // a name with a space
    std::fs::write(&a, b"x").unwrap();
    let c = tmp.join("c.txt");
    std::fs::write(&c, b"y").unwrap();
    // Terminal convention: names with spaces are escaped as `\ `; multiple paths are separated
    // by unescaped spaces.
    let text = format!(
        "{} {}",
        a.to_string_lossy().replace(' ', "\\ "),
        c.to_string_lossy()
    );
    let got = parse_dropped_paths(&text);
    assert_eq!(got.len(), 2, "実在2パスを解く: {got:?}");
    assert!(got.contains(&a) && got.contains(&c));
    // Rejects text that isn't a real path (= safely ignores a plain paste).
    assert!(parse_dropped_paths("just some pasted text, not a path").is_empty());
    std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn drop_paste_opens_dialog_then_copy_and_move() {
    let dir = std::env::temp_dir().join("konoma_drop_flow_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    // Two files with names containing spaces at the drop source (a different directory).
    let ext = std::env::temp_dir().join("konoma_drop_src");
    let _ = std::fs::remove_dir_all(&ext);
    std::fs::create_dir_all(&ext).unwrap();
    let src1 = ext.join("drop one.txt");
    let src2 = ext.join("drop two.txt");
    std::fs::write(&src1, b"a").unwrap();
    std::fs::write(&src2, b"b").unwrap();

    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.rebuild_tree().unwrap();
    // The only entry, "sub" (a dir), has the cursor → the drop destination = sub.
    assert!(
        app.tab.entries[app.tab.selected].is_dir,
        "カーソルが sub(dir)"
    );

    // --- Copy: drop one file → c copies it into sub (the source remains) ---
    app.handle_paste(src1.to_string_lossy().replace(' ', "\\ "));
    assert!(
        app.is_dialog() && app.confirm_is_drop(),
        "ドロップ確認が開く"
    );
    app.drop_apply(false).unwrap();
    assert!(!app.is_dialog(), "実行でダイアログが閉じる");
    assert!(
        dir.join("sub").join("drop one.txt").is_file(),
        "sub へコピー"
    );
    assert!(src1.is_file(), "コピーは元を残す");

    // --- Move: drop the other file → m moves it into sub (the source disappears) ---
    app.handle_paste(src2.to_string_lossy().replace(' ', "\\ "));
    assert!(app.confirm_is_drop());
    app.drop_apply(true).unwrap();
    assert!(dir.join("sub").join("drop two.txt").is_file(), "sub へ移動");
    assert!(!src2.exists(), "移動は元を消す");

    // Pasting text that isn't a real path does not open the dialog.
    app.handle_paste("not a real path".to_string());
    assert!(!app.is_dialog(), "テキストペーストでは開かない");

    std::fs::remove_dir_all(&dir).ok();
    std::fs::remove_dir_all(&ext).ok();
}

#[test]
fn paste_into_filter_inserts_text_not_drop() {
    // Pasting while filter input is active must be **text insertion**, not a "drop" (regression guard).
    let dir = std::env::temp_dir().join("konoma_paste_filter_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.txt"), b"x").unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.rebuild_tree().unwrap();
    app.start_filter();
    assert!(app.is_filtering());
    app.handle_paste("abc".to_string());
    assert_eq!(
        app.filter_query(),
        Some("abc"),
        "ペーストが絞り込みへ挿入される"
    );
    assert!(!app.is_dialog(), "ドロップダイアログは開かない");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn reanchor_root_sets_current_position_as_anchor() {
    // `:` takes no text input. It re-anchors the display base (open_dir) to the current tree
    // root reached via h/l.
    let base = std::env::temp_dir().join("konoma_reanchor_test");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("sub")).unwrap();
    std::fs::write(base.join("sub").join("f.txt"), b"x").unwrap();
    let base_c = base.canonicalize().unwrap();
    let mut app = App::new(base_c.clone(), Config::default()).unwrap();
    app.rebuild_tree().unwrap();
    // Right after launch, open_dir == root == base.
    assert_eq!(app.tab.open_dir, base_c);
    // l descends into sub (only root becomes sub; open_dir stays base).
    app.tree_descend().unwrap();
    assert_eq!(app.tab.root, base_c.join("sub"), "root が sub へ");
    assert_eq!(app.tab.open_dir, base_c, "open_dir はまだ base(launch)");
    // Files under sub are shown relative to base, so they display as "sub/...".
    let f = base_c.join("sub").join("f.txt");
    assert!(
        app.format_path(&f).contains("sub"),
        "再アンカー前は sub/ 込み表示: {}",
        app.format_path(&f)
    );

    // `:` = re-anchor the current location (sub) as the fixed root. No text input, no dialog.
    app.reanchor_root();
    assert!(!app.is_dialog(), "ダイアログは出ない(入力なし)");
    assert_eq!(
        app.tab.open_dir,
        base_c.join("sub"),
        "open_dir が sub へ再アンカー"
    );
    assert_eq!(app.tab.root, base_c.join("sub"), "root は変わらない");
    // After re-anchoring, paths are relative to sub, so f.txt is "sub/f.txt" (= displayed with sub as root).
    assert_eq!(app.format_path(&f), "sub/f.txt");

    // Already at the root (open_dir==root); `:` again → does nothing (just a flash).
    app.reanchor_root();
    assert_eq!(app.tab.open_dir, base_c.join("sub"), "変化なし");
    std::fs::remove_dir_all(&base).ok();
}

#[cfg(feature = "git")]
#[test]
fn gitignored_entries_detected_and_dimmed() {
    use ratatui::backend::TestBackend;
    use ratatui::style::Modifier;
    use ratatui::Terminal;
    let dir = std::env::temp_dir().join("konoma_gitignore_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    git2::Repository::init(&dir).unwrap();
    std::fs::write(dir.join(".gitignore"), b"ignored.log\nnode_modules/\n").unwrap();
    std::fs::write(dir.join("tracked.txt"), b"x").unwrap();
    std::fs::write(dir.join("ignored.log"), b"y").unwrap();
    std::fs::create_dir_all(dir.join("node_modules").join("pkg")).unwrap();
    std::fs::write(dir.join("node_modules").join("pkg").join("i.js"), b"z").unwrap();

    let canon = dir.canonicalize().unwrap();
    let mut app = App::new(canon.clone(), Config::default()).unwrap();
    app.rebuild_tree().unwrap();
    app.refresh_git_if_needed();

    // is_ignored: true for ignored.log / node_modules (and its contents), false for tracked / .gitignore.
    assert!(app.is_ignored(&canon.join("ignored.log")), "ignored.log");
    assert!(app.is_ignored(&canon.join("node_modules")), "node_modules");
    assert!(
        app.is_ignored(&canon.join("node_modules").join("pkg").join("i.js")),
        "node_modules 配下も祖先一致で ignored"
    );
    assert!(!app.is_ignored(&canon.join("tracked.txt")), "tracked");
    assert!(
        !app.is_ignored(&canon.join(".gitignore")),
        ".gitignore 自体"
    );

    // Rendering: the ignored.log row contains a DIM cell / the tracked.txt row has no DIM
    // (the selected row is node_modules, so neither is selected).
    let dim_in_row = |app: &mut App, needle: &str| -> Option<bool> {
        let mut term = Terminal::new(TestBackend::new(50, 12)).unwrap();
        term.draw(|f| crate::ui::render(f, app)).unwrap();
        let buf = term.backend().buffer();
        for y in 0..12u16 {
            let row: String = (0..50)
                .map(|x| buf.cell((x, y)).unwrap().symbol().to_string())
                .collect();
            if row.contains(needle) {
                let any_dim =
                    (0..50).any(|x| buf.cell((x, y)).unwrap().modifier.contains(Modifier::DIM));
                return Some(any_dim);
            }
        }
        None
    };
    assert_eq!(
        dim_in_row(&mut app, "ignored.log"),
        Some(true),
        "ignored は暗い"
    );
    assert_eq!(
        dim_in_row(&mut app, "tracked.txt"),
        Some(false),
        "tracked は暗くない"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// C13 (minor carryover): if an fs-event burst consists entirely of gitignored paths (build churn
/// such as target/, etc.), it is fine to skip `refresh_fs_changed`. Mixed/empty/ignore-rule-change
/// cases err on the safe side = always refresh.
#[cfg(feature = "git")]
#[test]
fn fs_burst_build_churn_skips_only_all_ignored_paths() {
    let dir = std::env::temp_dir().join("konoma_fs_burst_churn_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    git2::Repository::init(&dir).unwrap();
    std::fs::write(dir.join(".gitignore"), b"build/\n").unwrap();
    std::fs::create_dir_all(dir.join("build")).unwrap();
    std::fs::write(dir.join("build").join("out.o"), b"x").unwrap();
    std::fs::write(dir.join("src.rs"), b"y").unwrap();

    let canon = dir.canonicalize().unwrap();
    let mut app = App::new(canon.clone(), Config::default()).unwrap();
    app.rebuild_tree().unwrap();
    app.refresh_git_if_needed();

    let out_o = canon.join("build").join("out.o");
    let src_rs = canon.join("src.rs");
    assert!(
        app.is_ignored(&out_o),
        "build/out.o は ignored のはず(前提)"
    );
    assert!(!app.is_ignored(&src_rs), "src.rs は tracked のはず(前提)");

    assert!(
        app.fs_burst_is_build_churn(std::slice::from_ref(&out_o), false),
        "全パスが ignored → churn=true でスキップ可"
    );
    assert!(
        !app.fs_burst_is_build_churn(std::slice::from_ref(&src_rs), false),
        "非 ignored パスのみ → churn=false でリフレッシュ"
    );
    assert!(
        !app.fs_burst_is_build_churn(&[out_o.clone(), src_rs.clone()], false),
        "混在 → churn=false でリフレッシュ"
    );
    assert!(
        !app.fs_burst_is_build_churn(&[], false),
        "空(不明/.git のみ) → churn=false でリフレッシュ"
    );
    assert!(
        !app.fs_burst_is_build_churn(&[out_o], true),
        "無視ルール自体が変わった → churn=false で常にリフレッシュ"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn diff_layout_parse_and_resolve() {
    assert_eq!(DiffLayout::parse("unified"), DiffLayout::Unified);
    assert_eq!(DiffLayout::parse("vertical"), DiffLayout::Unified);
    assert_eq!(DiffLayout::parse("split"), DiffLayout::Split);
    assert_eq!(DiffLayout::parse("side-by-side"), DiffLayout::Split);
    assert_eq!(DiffLayout::parse("Horizontal"), DiffLayout::Split);
    assert_eq!(DiffLayout::parse("auto"), DiffLayout::Auto);
    assert_eq!(DiffLayout::parse("nonsense"), DiffLayout::Unified);
    assert!(!DiffLayout::Unified.is_split(999), "縦は常に false");
    assert!(DiffLayout::Split.is_split(10), "横は常に true");
    assert!(DiffLayout::Auto.is_split(90), "Auto: 広いと横");
    assert!(!DiffLayout::Auto.is_split(89), "Auto: 狭いと縦");
    // Cycle: vertical→horizontal→Auto→vertical.
    assert_eq!(DiffLayout::Unified.next(), DiffLayout::Split);
    assert_eq!(DiffLayout::Split.next(), DiffLayout::Auto);
    assert_eq!(DiffLayout::Auto.next(), DiffLayout::Unified);
}

#[cfg(feature = "git")]
#[test]
fn diff_from_tree_and_worktree_detail_and_cycle() {
    let dir = std::env::temp_dir().join("konoma_diff_entrypoints");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    init_git_repo(&dir);
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .current_dir(&dir)
            .args(args)
            .output()
            .unwrap();
    };
    std::fs::write(dir.join("a.txt"), b"v1\n").unwrap();
    git(&["add", "-A"]);
    git(&["commit", "-m", "init"]);
    std::fs::write(dir.join("a.txt"), b"v2\n").unwrap(); // modified
    std::fs::write(dir.join("b.txt"), b"new\n").unwrap(); // untracked

    let canon = dir.canonicalize().unwrap();
    let mut app = App::new(canon.clone(), Config::default()).unwrap();
    app.rebuild_tree().unwrap();
    app.refresh_git_if_needed();

    // Feature 1: cursor on changed file a.txt in the tree → `=` opens a diff preview directly.
    let ai = app
        .tab
        .entries
        .iter()
        .position(|e| e.path == canon.join("a.txt"))
        .expect("a.txt がツリーに無い");
    app.tab.selected = ai;
    app.tree_open_git_diff();
    assert!(
        app.is_git_diff_preview(),
        "ツリーから diff プレビューが開く"
    );
    assert!(!app.git_diff_lines().is_empty(), "diff 行がある");
    app.close_git_diff();

    // Feature 2: in the git view, `D` = diff (detail) of all working-tree changes. Includes multiple files.
    app.open_git_view();
    app.open_worktree_detail();
    assert!(app.is_git_detail(), "全変更 diff(詳細)が開く");
    assert_eq!(
        app.git_detail_title(),
        Some("Uncommitted changes"),
        "タイトル上書き"
    );
    // A file-boundary header = Context, with both line numbers None.
    let headers = app
        .git_detail_lines()
        .iter()
        .filter(|l| {
            matches!(l.kind, crate::git::DiffLineKind::Context)
                && l.old_no.is_none()
                && l.new_no.is_none()
        })
        .count();
    assert!(
        headers >= 2,
        "全変更 diff が複数ファイル: headers={headers}"
    );

    // Features 3+4: `s` cycles vertical→horizontal→Auto→vertical (works in detail too).
    assert!(!app.diff_is_split(200), "初期は unified(既定設定)");
    app.cycle_diff_layout();
    assert!(app.diff_is_split(200), "→ split");
    app.cycle_diff_layout();
    assert!(
        app.diff_is_split(200) && !app.diff_is_split(40),
        "→ auto(幅で縦横)"
    );
    app.cycle_diff_layout();
    assert!(!app.diff_is_split(200), "→ unified に戻る");

    std::fs::remove_dir_all(&dir).ok();
}

#[cfg(feature = "git")]
#[test]
fn diff_horizontal_scroll_reveals_long_line() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    let dir = std::env::temp_dir().join("konoma_diff_hscroll");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    init_git_repo(&dir);
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .current_dir(&dir)
            .args(args)
            .output()
            .unwrap();
    };
    std::fs::write(dir.join("a.txt"), b"short\n").unwrap();
    git(&["add", "-A"]);
    git(&["commit", "-m", "init"]);
    // Change it to one very long line (START at the start, END at the end).
    let long = format!("START{}END\n", "x".repeat(200));
    std::fs::write(dir.join("a.txt"), long.as_bytes()).unwrap();

    let canon = dir.canonicalize().unwrap();
    let mut app = App::new(canon.clone(), Config::default()).unwrap();
    app.rebuild_tree().unwrap();
    app.refresh_git_if_needed();
    let ai = app
        .tab
        .entries
        .iter()
        .position(|e| e.path == canon.join("a.txt"))
        .unwrap();
    app.tab.selected = ai;
    app.tree_open_git_diff();
    assert!(app.is_git_diff_preview());

    let dump = |app: &mut App| -> String {
        let mut term = Terminal::new(TestBackend::new(40, 10)).unwrap();
        term.draw(|f| crate::ui::render(f, app)).unwrap();
        let buf = term.backend().buffer();
        buf.content().iter().map(|c| c.symbol()).collect()
    };
    // At the line start (hscroll=0): START is visible, the far-off END is off-screen.
    let s0 = dump(&mut app);
    assert!(s0.contains("START"), "行頭に START");
    assert!(!s0.contains("END"), "行頭で END は画面外");
    // The $ equivalent, to the line end: END is visible.
    app.preview_hscroll_end();
    let se = dump(&mut app);
    assert!(se.contains("END"), "$ で行末(END)が見える");
    // The 0 equivalent, back to the line start.
    app.preview_hscroll_home();
    let sh = dump(&mut app);
    assert!(sh.contains("START") && !sh.contains("END"), "0 で行頭へ");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn copy_cut_paste_flow() {
    let dir = std::env::temp_dir().join("konoma_clipboard_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("dst")).unwrap();
    std::fs::write(dir.join("a.txt"), b"A").unwrap();
    std::fs::write(dir.join("b.txt"), b"B").unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.rebuild_tree().unwrap();
    let idx =
        |app: &App, p: &std::path::Path| app.tab.entries.iter().position(|e| e.path == p).unwrap();

    // Copy a.txt → it's loaded into the clipboard, and the selection clears.
    app.tab.selected = idx(&app, &dir.join("a.txt"));
    app.toggle_select();
    app.copy_selection();
    assert!(app.clipboard_label().is_some(), "コピーで積まれる");
    assert!(!app.has_selection(), "コピーで選択クリア");
    // Place the cursor on dst and paste → dst/a.txt is created and the original remains (duplicate).
    app.tab.selected = idx(&app, &dir.join("dst"));
    app.paste().unwrap();
    assert!(dir.join("dst").join("a.txt").is_file(), "コピー先に出来る");
    assert!(dir.join("a.txt").is_file(), "コピー元は残る");
    assert!(app.clipboard_label().is_some(), "コピーは貼っても消えない");

    // Cut b.txt → paste into dst → it moves and the clipboard is consumed.
    app.tab.selected = idx(&app, &dir.join("b.txt"));
    app.toggle_select();
    app.cut_selection();
    app.tab.selected = idx(&app, &dir.join("dst"));
    app.paste().unwrap();
    assert!(dir.join("dst").join("b.txt").is_file(), "カット先に出来る");
    assert!(!dir.join("b.txt").exists(), "カット元は消える(移動)");
    assert!(app.clipboard_label().is_none(), "カットは貼ると消費される");

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn dialog_delete_permanent_removes_immediately() {
    let dir = std::env::temp_dir().join("konoma_dialog_perm_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("gone.txt"), b"x").unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.rebuild_tree().unwrap();
    let i = app
        .tab
        .entries
        .iter()
        .position(|e| e.path == dir.join("gone.txt"))
        .unwrap();
    app.tab.selected = i;

    app.start_delete();
    assert!(
        app.dialog_is_confirm() && app.dialog_allow_permanent(),
        "削除確認は完全削除(!)を許可する"
    );
    app.dialog_delete_permanent().unwrap();
    assert!(!app.is_dialog(), "完全削除でダイアログは閉じる");
    assert!(
        !dir.join("gone.txt").exists(),
        "完全削除でファイルが消える(ゴミ箱を経由しない)"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn tree_page_clamps_within_bounds() {
    let dir = std::env::temp_dir().join("konoma_tree_page_test");
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    // Replace with a known count (100 entries).
    app.tab.entries = (0..100)
        .map(|i| Entry {
            path: dir.join(format!("f{i}")),
            is_dir: false,
            depth: 0,
            expanded: false,
        })
        .collect();
    app.tab.tree_viewport = 20;

    app.tab.selected = 0;
    app.tree_page(1); // +19 (overlaps by 1 line)
    assert_eq!(app.tab.selected, 19);
    app.tree_half_page(1); // +10
    assert_eq!(app.tab.selected, 29);

    // Clamps at the end.
    app.tree_last();
    assert_eq!(app.tab.selected, 99);
    app.tree_page(1);
    assert_eq!(app.tab.selected, 99);

    // Clamps at the start.
    app.tree_first();
    app.tree_page(-1);
    assert_eq!(app.tab.selected, 0);

    std::fs::remove_dir_all(&dir).ok();
}

// --- Integer overflow regression (#7/#8): even when the cursor is not at the start, `g`/`G`/Home/End
//     used to panic on a plain `as i32 + delta` with the i32::MIN/MAX they pass in. Cleaned up
//     entirely with the shared `clamp_cursor`.

#[test]
fn clamp_cursor_no_overflow_at_extremes() {
    // Even with the cursor placed somewhere other than 0, going through i64 must not overflow.
    assert_eq!(clamp_cursor(3, i32::MAX, 10), 9, "末尾へクランプ");
    assert_eq!(clamp_cursor(3, i32::MIN, 10), 0, "先頭へクランプ");
    assert_eq!(clamp_cursor(0, i32::MAX, 1), 0, "1件は 0 のまま");
    assert_eq!(clamp_cursor(5, -1, 10), 4, "通常の1行戻り");
    assert_eq!(clamp_cursor(5, 1, 10), 6, "通常の1行進み");
    assert_eq!(clamp_cursor(7, i32::MAX, 0), 0, "len==0 は常に 0");
    assert_eq!(clamp_cursor(7, i32::MIN, 0), 0, "len==0 は常に 0");
}

#[test]
fn tree_move_extremes_no_panic() {
    let dir = std::env::temp_dir().join("konoma_tree_move_extremes_test");
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.tab.entries = (0..50)
        .map(|i| Entry {
            path: dir.join(format!("f{i}")),
            is_dir: false,
            depth: 0,
            expanded: false,
        })
        .collect();
    // Place it somewhere other than the start, then use an extreme value: this used to panic on
    // the i32 addition here.
    app.tab.selected = 10;
    app.tree_move(i32::MAX);
    assert_eq!(app.tab.selected, 49, "末尾へ");
    app.tree_move(i32::MIN);
    assert_eq!(app.tab.selected, 0, "先頭へ");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn git_branch_move_extremes_no_panic() {
    let dir = std::env::temp_dir().join("konoma_branch_move_extremes_test");
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.tab.git_branches = Some(
        (0..6)
            .map(|i| crate::git::BranchInfo {
                name: format!("br{i}"),
                is_current: i == 0,
            })
            .collect(),
    );
    app.tab.git_branch_sel = 2; // not the start
    app.git_branch_move(i32::MAX);
    assert_eq!(app.tab.git_branch_sel, 5, "末尾へ");
    app.git_branch_move(i32::MIN);
    assert_eq!(app.tab.git_branch_sel, 0, "先頭へ");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn git_graph_move_extremes_no_panic() {
    let dir = std::env::temp_dir().join("konoma_graph_move_extremes_test");
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    let commit = |id: &str| crate::git::GraphRow {
        graph: Vec::new(),
        commit: Some(id.to_string()),
        short: String::new(),
        subject: String::new(),
        author: String::new(),
        date: String::new(),
        refs: String::new(),
        worktree: false,
    };
    let connector = || crate::git::GraphRow {
        graph: Vec::new(),
        commit: None,
        short: String::new(),
        subject: String::new(),
        author: String::new(),
        date: String::new(),
        refs: String::new(),
        worktree: false,
    };
    // Commit rows (0,2,4) and connector rows (1,3) are mixed in: the cursor only targets commit rows.
    app.tab.git_graph = Some(vec![
        commit("a"),
        connector(),
        commit("b"),
        connector(),
        commit("c"),
    ]);
    app.tab.git_graph_sel = 2; // the middle commit row (commits index=1)
    app.git_graph_move(i32::MAX);
    assert_eq!(app.tab.git_graph_sel, 4, "末尾コミット行へ");
    app.git_graph_move(i32::MIN);
    assert_eq!(app.tab.git_graph_sel, 0, "先頭コミット行へ");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn git_log_move_extremes_no_panic() {
    let dir = std::env::temp_dir().join("konoma_log_move_extremes_test");
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.tab.git_log = Some(
        (0..5)
            .map(|i| crate::git::CommitInfo {
                id: format!("id{i}"),
                short: format!("s{i}"),
                summary: String::new(),
                author: String::new(),
                time_epoch: 0,
            })
            .collect(),
    );
    app.tab.git_log_sel = 1; // not the start
    app.git_log_move(i32::MAX);
    assert_eq!(app.tab.git_log_sel, 4, "末尾へ");
    app.git_log_move(i32::MIN);
    assert_eq!(app.tab.git_log_sel, 0, "先頭へ");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn git_view_move_extremes_no_panic() {
    let dir = std::env::temp_dir().join("konoma_view_move_extremes_test");
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.tab.git_view_entries = (0..4)
        .map(|i| crate::git::ChangeEntry {
            path: dir.join(format!("f{i}")),
            status: crate::git::FileStatus::Modified,
            staged: false,
        })
        .collect();
    app.tab.git_view_sel = 1; // not the start
                              // git_view only ever passes ±1, but since it now goes through the shared
                              // helper, it must also be safe at extreme values.
    app.git_view_move(i32::MAX);
    assert_eq!(app.tab.git_view_sel, 3, "末尾へ");
    app.git_view_move(i32::MIN);
    assert_eq!(app.tab.git_view_sel, 0, "先頭へ");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn centered_fit_centers_and_downscales() {
    let inner = Rect {
        x: 0,
        y: 0,
        width: 80,
        height: 24,
    };
    // A small image: centered at native (40x15) without upscaling (allow_upscale=false).
    let r = centered_rect((40, 15), inner, false);
    assert_eq!((r.width, r.height), (40, 15));
    assert_eq!((r.x, r.y), (20, 4));

    // A large image: downscaled preserving aspect ratio and centered (200x100 → scale 0.24).
    let r = centered_rect((200, 100), inner, false);
    assert_eq!((r.width, r.height), (48, 24));
    assert_eq!((r.x, r.y), (16, 0));

    // When zoomed (allow_upscale=true): even a small image is upscaled to fill the area and centered.
    let r = centered_rect((40, 15), inner, true);
    // scale = min(80/40, 24/15)=min(2.0,1.6)=1.6 → 64x24, x=(80-64)/2=8.
    assert_eq!((r.width, r.height), (64, 24));
    assert_eq!((r.x, r.y), (8, 0));
}

/// A test App with image state. Provides a source image (400x300=4:3), the image kind, a halfblocks picker
/// (font 10x20, no terminal needed), and a dummy tx so that prepare_image can run.
fn app_with_image() -> App {
    let dir = std::env::temp_dir().join("konoma_img_state_test");
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = App::new(dir, Config::default()).unwrap();
    app.image_src = Some(std::sync::Arc::new(image::DynamicImage::new_rgb8(400, 300)));
    app.tab.preview_kind = Some(PreviewKind::Image(PathBuf::from("x.png")));
    app.picker = Some(ratatui_image::picker::Picker::halfblocks());
    // Leak tx so it isn't dropped (keeps it alive until the test ends).
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    Box::leak(Box::new(rx));
    app.img_tx = Some(tx);
    app
}

fn inner(w: u16, h: u16) -> Rect {
    Rect {
        x: 0,
        y: 0,
        width: w,
        height: h,
    }
}

/// An App with a Picker forced to kitty graphics (no terminal needed). Used to verify, via the TestBackend buffer,
/// that the GIF synchronous-encode render path actually emits image-transfer sequences.
fn app_with_kitty() -> App {
    let dir = std::env::temp_dir().join("konoma_gif_kitty_test");
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = App::new(dir, Config::default()).unwrap();
    // Force kitty onto the halfblocks picker (no terminal needed) to verify generation of the
    // kitty transfer sequence.
    let mut picker = ratatui_image::picker::Picker::halfblocks();
    picker.set_protocol_type(ratatui_image::picker::ProtocolType::Kitty);
    app.picker = Some(picker);
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    Box::leak(Box::new(rx));
    app.img_tx = Some(tx);
    app
}

/// Off-thread media loading: on start loading=true, nothing is read synchronously, and the result is applied when it arrives.
#[test]
fn media_load_is_async_then_applied() {
    let p = Path::new("samples/sample.svg");
    if !p.exists() {
        return; // skip in environments where samples are excluded
    }
    let mut app = app_with_kitty();
    let (tx, rx) = std::sync::mpsc::channel();
    app.attach_media_loader(tx);
    let kind = PreviewKind::Svg(p.to_path_buf());
    app.tab.preview_kind = Some(kind.clone());

    app.start_media_load(&kind, p);
    // Right after starting: it's loading, and the image isn't in place yet (the UI isn't blocked).
    assert!(app.is_media_loading(), "開始直後は loading");
    assert!(!app.is_image_preview(), "まだ読み込めていない");

    // Wait for the other thread's result and apply it → the image is in place and loading clears.
    let result = rx
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("worker should return");
    assert!(app.apply_media(result), "結果を反映");
    assert!(!app.is_media_loading());
    assert!(app.is_image_preview(), "SVG ラスタが image_src に載る");
}

/// While loading, the preview shows the (shared) braille spinner plus "loading…" (no raw XML / blank).
#[test]
fn media_loading_renders_shared_spinner() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    let mut app = app_with_kitty(); // has a picker
    let (tx, rx) = std::sync::mpsc::channel();
    Box::leak(Box::new(rx)); // don't drain the result → stays loading
    app.attach_media_loader(tx);
    let path = PathBuf::from("/does-not-exist.svg");
    let kind = PreviewKind::Svg(path.clone());
    app.tab.preview_kind = Some(kind.clone());
    app.tab.preview_path = Some(path.clone());
    app.tab.mode = Mode::Preview;
    app.start_media_load(&kind, &path);
    assert!(app.is_media_loading(), "読み込み中になる");

    let mut term = Terminal::new(TestBackend::new(60, 12)).unwrap();
    term.draw(|f| crate::ui::preview::render(f, &mut app, f.area()))
        .unwrap();
    let s: String = term
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|c| c.symbol())
        .collect();
    // The default Config is en, so the text is "loading…" (it would be "読み込み中…" for jp).
    assert!(s.contains("loading"), "ローディング文言が出る: {s:?}");
    let spinners = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
    assert!(
        spinners.iter().any(|g| s.contains(g)),
        "共通スピナーのコマが出る: {s:?}"
    );
    // Must not fall back to raw XML (no XML is shown while loading).
    assert!(!s.contains("<svg"), "読み込み中に生 XML を出さない");
}

/// PDF page navigation state machine: J/K move within [1, total] and clamp at the ends, the indicator
/// reports (current, total), navigation is disabled for single-page/unknown-count PDFs, and a page move
/// resets zoom to fit. Pure logic (no backend), so spawn_or_sync_media is a no-op here.
#[test]
fn pdf_page_navigation_clamps_and_indicates() {
    let dir = std::env::temp_dir().join("konoma_pdf_nav_test");
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = App::new(dir, Config::default()).unwrap();
    app.tab.preview_kind = Some(PreviewKind::Pdf(PathBuf::from("/x/doc.pdf")));
    app.tab.mode = Mode::Preview;

    // Unknown total: cannot navigate, no indicator.
    app.tab.pdf_pages = None;
    app.tab.pdf_page = 1;
    assert!(!app.pdf_can_navigate());
    app.pdf_next_page();
    assert_eq!(app.tab.pdf_page, 1, "総数不明では動かない");
    assert_eq!(app.pdf_page_indicator(), None);

    // Single page: cannot navigate.
    app.tab.pdf_pages = Some(1);
    assert!(!app.pdf_can_navigate(), "単ページは移動不可");
    app.pdf_next_page();
    assert_eq!(app.tab.pdf_page, 1);

    // Three pages: forward/backward navigation, edge clamping, and the indicator.
    app.tab.pdf_pages = Some(3);
    app.tab.pdf_page = 1;
    assert!(app.pdf_can_navigate());
    assert_eq!(app.pdf_page_indicator(), Some((1, 3)));
    app.pdf_next_page();
    assert_eq!(app.tab.pdf_page, 2);
    app.pdf_next_page();
    assert_eq!(app.tab.pdf_page, 3);
    app.pdf_next_page();
    assert_eq!(app.tab.pdf_page, 3, "末尾でクランプ");
    assert_eq!(app.pdf_page_indicator(), Some((3, 3)));
    app.pdf_prev_page();
    assert_eq!(app.tab.pdf_page, 2);
    app.pdf_prev_page();
    app.pdf_prev_page();
    assert_eq!(app.tab.pdf_page, 1, "先頭でクランプ");

    // A page move resets zoom back to fit (1.0).
    app.tab.image_zoom = 4.0;
    app.pdf_next_page();
    assert_eq!(app.tab.pdf_page, 2);
    assert_eq!(app.tab.image_zoom, 1.0, "ページ移動で fit に戻る");
}

/// PDF multi-page end-to-end: navigating to page 2 kicks off an off-thread re-rasterization (via
/// `hayro`, no external tool needed) that lands a new image. `page_count` is pure Rust and always
/// resolves (no poppler needed), so this only skips without the bundled sample.
#[test]
fn pdf_next_page_renders_off_thread() {
    let p = Path::new("samples/sample.pdf");
    if !p.exists() {
        return; // skip in environments where samples are excluded
    }
    let Some(pages) = crate::preview::pdf::page_count(p) else {
        return; // safety net in case parsing fails unexpectedly (normally unreachable)
    };
    if pages < 2 {
        return;
    }
    let mut app = app_with_kitty();
    let (tx, rx) = std::sync::mpsc::channel();
    app.attach_media_loader(tx);
    let kind = PreviewKind::Pdf(p.to_path_buf());
    app.tab.preview_kind = Some(kind.clone());
    app.tab.preview_path = Some(p.to_path_buf());
    app.tab.mode = Mode::Preview;
    app.tab.pdf_pages = Some(pages);
    app.tab.pdf_page = 1;

    // Load page 1 and apply it.
    app.start_media_load(&kind, p);
    let r1 = rx
        .recv_timeout(std::time::Duration::from_secs(15))
        .expect("page1 worker should return");
    assert!(app.apply_media(r1));
    assert!(app.is_image_preview(), "page1 が image_src に載る");

    // To page 2: kicks off re-rasterization on another thread → apply it.
    app.pdf_next_page();
    assert_eq!(app.tab.pdf_page, 2);
    assert!(app.is_media_loading(), "ページ送りで再読み込みが始まる");
    let r2 = rx
        .recv_timeout(std::time::Duration::from_secs(15))
        .expect("page2 worker should return");
    assert!(app.apply_media(r2));
    assert!(app.is_image_preview(), "page2 が image_src に載る");
}

/// A stale media result (the previous file's result arriving after moving to a different file) is discarded.
#[test]
fn stale_media_result_is_ignored() {
    let mut app = app_with_kitty();
    app.tab.preview_kind = Some(PreviewKind::Svg(PathBuf::from("x.svg")));
    // Build a result that doesn't match the current generation (a stale generation).
    let img =
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(8, 8, image::Rgb([1, 2, 3])));
    let stale = MediaResult {
        gen: app.media_gen.wrapping_sub(1),
        payload: Some(MediaPayload::Static(img)),
    };
    assert!(!app.apply_media(stale), "古い世代は反映しない");
    assert!(app.image_src.is_none(), "状態は変わらない");
}

/// Regression guard (user report: not a single GIF frame displayed). Verifies that when each GIF frame is
/// synchronously encoded and rendered, kitty's image-transfer sequence (`_G`) actually appears in the buffer = pixels are drawn.
/// Also checks that different frames produce different data (switching atomically).
#[test]
fn gif_emits_kitty_image_data_per_frame() {
    use ratatui::buffer::Buffer;
    use ratatui::widgets::Widget;
    use ratatui_image::Image;

    let mut app = app_with_kitty();
    app.tab.preview_kind = Some(PreviewKind::Image(PathBuf::from("x.gif")));
    // Set two different frames (solid red/green) directly, verifying only the render path
    // without going through decode.
    let red = image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
        40,
        30,
        image::Rgb([255, 0, 0]),
    ));
    let green = image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
        40,
        30,
        image::Rgb([0, 255, 0]),
    ));
    app.gif_frames = vec![
        (red, std::time::Duration::from_millis(50)),
        (green, std::time::Duration::from_millis(50)),
    ];
    app.gif_idx = 0;
    assert!(app.is_gif_active(), "2 フレームならアニメ扱い");
    assert!(
        app.is_image_preview(),
        "GIF も画像プレビュー(ズーム/パン有効)"
    );

    let area = inner(40, 20);

    // Prepare + render frame 0 → the kitty transfer sequence shows up in the cell symbol.
    let t0 = app.prepare_gif(area).expect("frame0 layout");
    let proto0 = app.gif_protocol().expect("frame0 should be encoded");
    let mut buf0 = Buffer::empty(area);
    Image::new(proto0).render(t0, &mut buf0);
    let dump0: String = buf0.content.iter().map(|c| c.symbol()).collect();
    assert!(
        dump0.contains("_G"),
        "frame0 は kitty graphics 転送(_G)を吐くはず(空でない描画)"
    );

    // Advance to frame 1 and re-prepare + render → different image data appears (an atomic
    // switch, no churn).
    app.gif_idx = 1;
    let t1 = app.prepare_gif(area).expect("frame1 layout");
    let proto1 = app.gif_protocol().expect("frame1 should be encoded");
    let mut buf1 = Buffer::empty(area);
    Image::new(proto1).render(t1, &mut buf1);
    let dump1: String = buf1.content.iter().map(|c| c.symbol()).collect();
    assert!(
        dump1.contains("_G"),
        "frame1 も kitty graphics 転送を吐くはず"
    );
    assert_ne!(dump0, dump1, "別フレームは別の画像データにエンコードされる");
}

#[test]
fn zoom_clamps_between_1_and_16() {
    let mut app = app_with_image();
    assert_eq!(app.tab.image_zoom, 1.0);
    app.image_zoom_by(0.5); // never goes below 1.0
    assert_eq!(app.tab.image_zoom, 1.0);
    for _ in 0..40 {
        app.image_zoom_by(1.25);
    }
    assert_eq!(app.tab.image_zoom, 16.0);
    app.image_zoom_reset();
    assert_eq!(app.tab.image_zoom, 1.0);
    assert_eq!(app.tab.image_center, (0.5, 0.5));
}

#[test]
fn fit_is_natural_size_and_centered_at_z1() {
    // z=1: a small image (400x300, font10x20=40x15 cells) is smaller than the viewport,
    // so it's shown centered at its natural size (40x15) without upscaling (doesn't fill the screen).
    let mut app = app_with_image();
    let r = app.prepare_image(inner(200, 40)).unwrap();
    assert_eq!((r.width, r.height), (40, 15), "z=1 は natural サイズ");
    // Centered (equal margins left/right and top/bottom).
    assert_eq!(r.x, (200 - 40) / 2);
    assert_eq!(r.y, (40 - 15) / 2);
    // Not clipped → panning is disabled.
    assert_eq!(app.image_vis_frac, (1.0, 1.0));
}

#[test]
fn zoom_grows_then_clips_and_enables_pan() {
    let mut app = app_with_image();
    let inner = inner(200, 40);
    // z=2: 80x30 cells, still within the viewport (200x40) → shown in full, centered, no panning.
    app.tab.image_zoom = 2.0;
    let r = app.prepare_image(inner).unwrap();
    assert_eq!((r.width, r.height), (80, 30));
    assert_eq!(app.image_vis_frac, (1.0, 1.0));
    // z=4: 160x60 → height 60>40 clips vertically. Display height is constrained to the
    // viewport, and vertical panning is enabled.
    app.tab.image_zoom = 4.0;
    let r = app.prepare_image(inner).unwrap();
    assert_eq!(r.height, 40, "高さは viewport に制限");
    assert_eq!(r.width, 160);
    assert!(app.image_vis_frac.1 < 1.0, "縦が見切れ=パン可");
    assert_eq!(app.image_vis_frac.0, 1.0, "横はまだ収まる");
    // Still centered.
    assert_eq!(r.x, (200 - 160) / 2);
}

#[test]
fn pan_noop_when_not_clipped_and_moves_when_clipped() {
    let mut app = app_with_image();
    let inner = inner(200, 40);
    // z=1: no clipping → even after panning, prepare resets the center back to 0.5.
    app.image_pan(1.0, 1.0);
    app.prepare_image(inner).unwrap();
    assert_eq!(app.tab.image_center, (0.5, 0.5));
    // Zoom in far enough to clip on both axes → panning moves the center, clamped at the edges.
    app.tab.image_zoom = 12.0;
    app.prepare_image(inner).unwrap(); // update the visible fraction
    for _ in 0..50 {
        app.image_pan(1.0, 1.0);
        app.prepare_image(inner).unwrap();
    }
    let (fw, fh) = app.image_vis_frac;
    assert!(fw < 1.0 && fh < 1.0, "両軸見切れ");
    // Clamped at the edge = 1 - frac/2.
    assert!((app.tab.image_center.0 - (1.0 - fw / 2.0)).abs() < 1e-6);
    assert!((app.tab.image_center.1 - (1.0 - fh / 2.0)).abs() < 1e-6);
}

#[test]
fn tabbar_appears_with_multiple_tabs() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    let dir = std::env::temp_dir().join("konoma_tabbar_test");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("f.txt"), b"x").unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();

    let render = |app: &mut App| -> String {
        let mut term = Terminal::new(TestBackend::new(80, 6)).unwrap();
        term.draw(|f| crate::ui::render(f, app)).unwrap();
        term.backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect()
    };
    // Default tabbar=auto: no tabbar is shown with a single tab.
    assert!(!render(&mut app).contains("1:konoma_tabbar_test"));
    // With two tabs, 1:/2: appear on the tabbar.
    app.tab_new().unwrap();
    let s = render(&mut app);
    assert!(s.contains("1:konoma_tabbar_test"), "タブ1が無い: {s:?}");
    assert!(s.contains("2:konoma_tabbar_test"), "タブ2が無い: {s:?}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn markdown_links_collected_and_local_link_opens_in_konoma() {
    // Collect links (URLs) in the md preview, focus one, and open a local link inside konoma.
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    let dir = std::env::temp_dir().join("konoma_links_open_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("target.md"), b"# Target\n\nhi\n").unwrap();
    std::fs::write(
        dir.join("doc.md"),
        b"See [go](./target.md) and [web](https://example.com/x).\n",
    )
    .unwrap();
    let mut app = App::new(dir.canonicalize().unwrap(), Config::default()).unwrap();
    app.tab.selected = app
        .tab
        .entries
        .iter()
        .position(|e| e.path.ends_with("doc.md"))
        .unwrap();
    app.tree_activate().unwrap();
    // Rendering collects the links (decorate_links runs).
    let mut term = Terminal::new(TestBackend::new(72, 8)).unwrap();
    term.draw(|f| crate::ui::render(f, &mut app)).unwrap();
    assert_eq!(app.md_items.len(), 2, "リンク数");
    assert!(item_target(&app.md_items[0]).ends_with("target.md"));
    assert_eq!(item_target(&app.md_items[1]), "https://example.com/x");
    // Focus the first link (local) and open it → konoma previews target.md.
    app.md_focus_move(1);
    assert_eq!(app.tab.focused_item, Some(0));
    app.md_activate_focused().unwrap();
    assert!(
        app.tab
            .preview_path
            .as_deref()
            .is_some_and(|p| p.ends_with("target.md")),
        "ローカルリンクで target.md に遷移していない: {:?}",
        app.tab.preview_path
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn md_focus_follows_offscreen_items_when_wrapped() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    // With wrapping (wrap=true, the default), when a long paragraph exceeds the screen height,
    // Tab's focus movement follows scroll based on **display rows**. Using logical rows instead
    // misjudges it as "still on screen" and never scrolls at all (regression reported by the
    // user on 2026-07-08).
    let dir = std::env::temp_dir().join("konoma_md_focus_wrap_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // A document with only a few logical lines, but whose first paragraph wraps heavily at
    // width 40 into more than 20 display rows.
    let long = "word ".repeat(200); // 1000 columns ≈ 25 display rows at width 40
    std::fs::write(
        dir.join("doc.md"),
        format!(
            "{long}

[link](https://example.com/x)
"
        ),
    )
    .unwrap();
    let mut app = App::new(dir.canonicalize().unwrap(), Config::default()).unwrap();
    assert!(app.cfg.ui.wrap, "前提: 既定は折返しON");
    app.tab.selected = app.tab.entries.iter().position(|e| !e.is_dir).unwrap();
    app.tree_activate().unwrap();
    let mut term = Terminal::new(TestBackend::new(40, 12)).unwrap();
    term.draw(|f| crate::ui::render(f, &mut app)).unwrap();
    assert_eq!(app.md_items.len(), 1);
    assert_eq!(app.tab.preview_scroll, 0);
    // Tab to the link: the link's logical row (2) is under the viewport (10), but in display
    // rows it's around row 25 → scrolling advances on a display-row basis (the old
    // implementation stayed at 0, which fails this assertion).
    app.md_focus_move(1);
    assert!(
        app.tab.preview_scroll > 5,
        "折返し文書でフォーカスに追従してスクロールする: scroll={}",
        app.tab.preview_scroll
    );
    // After following, the render has the focused row on screen (it fits within the display-row window).
    term.draw(|f| crate::ui::render(f, &mut app)).unwrap();
    let buf = term.backend().buffer().clone();
    let screen: String = buf.content().iter().map(|c| c.symbol()).collect();
    assert!(
        screen.contains("link"),
        "フォーカスしたリンクが画面内に描画される"
    );
    // Going back the other way (equivalent to Shift-Tab) cycles to the same item = scroll is
    // kept; since there's no earlier item here, upward-follow is covered by a separate document
    // (the upward branch is symmetric with the same formula).
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn md_task_toggle_cycles_and_writes_file() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    // Tab focuses a checkbox → toggling writes back only the single state character to the file.
    // Default cycle ' '⇄'x'; uppercase X is equivalent to x; other lines are unchanged (no byte
    // corruption even in CJK body text).
    let dir = std::env::temp_dir().join("konoma_md_task_toggle_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("todo.md");
    std::fs::write(&f, "# t\n\n- [ ] 未着手のタスク\n- [X] 済み\n").unwrap();
    let mut app = App::new(dir.canonicalize().unwrap(), Config::default()).unwrap();
    app.tab.selected = app
        .tab
        .entries
        .iter()
        .position(|e| e.path.ends_with("todo.md"))
        .unwrap();
    app.tree_activate().unwrap();
    let mut term = Terminal::new(TestBackend::new(72, 12)).unwrap();
    term.draw(|fr| crate::ui::render(fr, &mut app)).unwrap();
    assert_eq!(app.md_items.len(), 2);
    assert!(app.md_has_tasks());
    app.md_focus_move(1);
    assert!(app.md_focused_task());
    app.md_toggle_focused_task();
    let s = std::fs::read_to_string(&f).unwrap();
    assert!(s.contains("- [x] 未着手のタスク"), "{s}");
    assert!(s.contains("- [X] 済み"), "他行は不変: {s}");
    // Re-render to pick up the new state, then toggle the other way: 'x'→' '.
    term.draw(|fr| crate::ui::render(fr, &mut app)).unwrap();
    app.md_toggle_focused_task();
    let s = std::fs::read_to_string(&f).unwrap();
    assert!(s.contains("- [ ] 未着手のタスク"), "{s}");
    // To the second one (uppercase X): X is equivalent to x → next is ' '.
    term.draw(|fr| crate::ui::render(fr, &mut app)).unwrap();
    app.md_focus_move(1);
    app.md_toggle_focused_task();
    let s = std::fs::read_to_string(&f).unwrap();
    assert!(s.contains("- [ ] 済み"), "{s}");
    std::fs::remove_dir_all(&dir).ok();
}

/// Checkboxes in `*` and `+` bullet lists can be toggled too (the source scanner used to only
/// recognize `-`, so the count check drifted and everything got cancelled with "file changed on
/// disk" — user report 2026-07-22).
#[test]
fn md_task_toggle_star_and_plus_bullets() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    let dir = std::env::temp_dir().join("konoma_md_task_star_plus");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("todo.md");
    // `*`/`+` bullet lists (GFM-compliant). tui-markdown renders checkboxes for all three kinds.
    std::fs::write(&f, "* [ ] star task\n+ [ ] plus task\n").unwrap();
    let mut app = App::new(dir.canonicalize().unwrap(), Config::default()).unwrap();
    app.tab.selected = app
        .tab
        .entries
        .iter()
        .position(|e| e.path.ends_with("todo.md"))
        .unwrap();
    app.tree_activate().unwrap();
    let mut term = Terminal::new(TestBackend::new(72, 12)).unwrap();
    term.draw(|fr| crate::ui::render(fr, &mut app)).unwrap();
    assert_eq!(app.md_items.len(), 2, "star/plus 両方がタスクとして認識");

    // Toggle the first one (star) → it isn't cancelled and writes back. No "changed on disk" flash.
    app.md_focus_move(1);
    app.md_toggle_focused_task();
    let s = std::fs::read_to_string(&f).unwrap();
    assert!(
        s.contains("* [x] star task"),
        "star タスクがトグルできる: {s}"
    );
    assert!(s.contains("+ [ ] plus task"), "他行は不変: {s}");
    assert!(
        app.flash
            .as_deref()
            .map(|m| !m.contains("changed on disk"))
            .unwrap_or(true),
        "「file changed on disk」でキャンセルされない: {:?}",
        app.flash
    );

    // Same for the second one (plus).
    term.draw(|fr| crate::ui::render(fr, &mut app)).unwrap();
    app.md_focus_move(1);
    app.md_toggle_focused_task();
    let s = std::fs::read_to_string(&f).unwrap();
    assert!(
        s.contains("+ [x] plus task"),
        "plus タスクもトグルできる: {s}"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn md_task_toggle_custom_states_cycle() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    // ui.md_task_states = [" ", "/", "x"]: Space cycles through the array in order, and the
    // custom state [/] is also recognized as toggleable after re-rendering.
    let dir = std::env::temp_dir().join("konoma_md_task_custom_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("todo.md");
    std::fs::write(&f, "- [ ] a\n").unwrap();
    let mut cfg = Config::default();
    cfg.ui.md_task_states = vec![" ".into(), "/".into(), "x".into()];
    let mut app = App::new(dir.canonicalize().unwrap(), cfg).unwrap();
    app.tab.selected = app.tab.entries.iter().position(|e| !e.is_dir).unwrap();
    app.tree_activate().unwrap();
    let mut term = Terminal::new(TestBackend::new(60, 8)).unwrap();
    let cycle = |app: &mut App, term: &mut Terminal<TestBackend>| {
        term.draw(|fr| crate::ui::render(fr, app)).unwrap();
        if app.tab.focused_item.is_none() {
            app.md_focus_move(1);
        }
        app.md_toggle_focused_task();
        std::fs::read_to_string(&f).unwrap()
    };
    assert_eq!(cycle(&mut app, &mut term), "- [/] a\n");
    assert_eq!(cycle(&mut app, &mut term), "- [x] a\n");
    assert_eq!(cycle(&mut app, &mut term), "- [ ] a\n");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn md_task_toggle_aborts_when_file_changed_externally() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    // Don't write if the displayed state and disk state disagree (flash + reload). Avoids
    // conflicting with an external agent's edits.
    let dir = std::env::temp_dir().join("konoma_md_task_abort_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("todo.md");
    std::fs::write(&f, "- [ ] a\n").unwrap();
    let mut app = App::new(dir.canonicalize().unwrap(), Config::default()).unwrap();
    app.tab.selected = app.tab.entries.iter().position(|e| !e.is_dir).unwrap();
    app.tree_activate().unwrap();
    let mut term = Terminal::new(TestBackend::new(60, 8)).unwrap();
    term.draw(|fr| crate::ui::render(fr, &mut app)).unwrap();
    app.md_focus_move(1);
    // (1) State mismatch: the screen stays at ' ' while the disk becomes 'x'.
    std::fs::write(&f, "- [x] a\n").unwrap();
    app.md_toggle_focused_task();
    assert_eq!(
        std::fs::read_to_string(&f).unwrap(),
        "- [x] a\n",
        "書かない"
    );
    assert!(app.flash.is_some(), "flash で通知");
    // (2) Count mismatch: the number of tasks grew (the render side still sees 1).
    term.draw(|fr| crate::ui::render(fr, &mut app)).unwrap(); // let the screen catch up to x…
    std::fs::write(&f, "- [x] a\n- [ ] b\n").unwrap(); // …then make the disk have 2, only on disk
    app.flash = None;
    app.md_toggle_focused_task();
    assert_eq!(
        std::fs::read_to_string(&f).unwrap(),
        "- [x] a\n- [ ] b\n",
        "個数不一致でも書かない"
    );
    assert!(app.flash.is_some());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn md_task_toggle_noop_in_raw_source_and_preserves_crlf() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    let dir = std::env::temp_dir().join("konoma_md_task_raw_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("todo.md");
    std::fs::write(&f, "- [ ] a\r\n\r\ntail\r\n").unwrap();
    let mut app = App::new(dir.canonicalize().unwrap(), Config::default()).unwrap();
    app.tab.selected = app.tab.entries.iter().position(|e| !e.is_dir).unwrap();
    app.tree_activate().unwrap();
    let mut term = Terminal::new(TestBackend::new(60, 8)).unwrap();
    term.draw(|fr| crate::ui::render(fr, &mut app)).unwrap();
    app.md_focus_move(1);
    // While showing the raw source (R), both toggling and focus detection are disabled
    // (the 2D-caret surface).
    app.toggle_md_raw();
    assert!(!app.md_focused_task());
    app.md_toggle_focused_task();
    assert_eq!(
        std::fs::read_to_string(&f).unwrap(),
        "- [ ] a\r\n\r\ntail\r\n",
        "raw 中は書かない"
    );
    // Switching back to the decorated view allows toggling, and CRLF / trailing bytes are
    // preserved (only a single character is replaced).
    app.toggle_md_raw();
    app.md_toggle_focused_task();
    assert_eq!(
        std::fs::read_to_string(&f).unwrap(),
        "- [x] a\r\n\r\ntail\r\n"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// Safety guards for toggling: (1) if no checkbox is focused, it's a no-op (nothing is written);
/// (2) if the displayed file can no longer be read, notify via flash and back off safely
/// (no crash, principle #3).
#[test]
fn md_task_toggle_noop_without_focus_and_flashes_on_read_error() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    let dir = std::env::temp_dir().join("konoma_md_task_guard_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("todo.md");
    std::fs::write(&f, "- [ ] a\n").unwrap();
    let mut app = App::new(dir.canonicalize().unwrap(), Config::default()).unwrap();
    app.tab.selected = app.tab.entries.iter().position(|e| !e.is_dir).unwrap();
    app.tree_activate().unwrap();
    let mut term = Terminal::new(TestBackend::new(60, 8)).unwrap();
    term.draw(|fr| crate::ui::render(fr, &mut app)).unwrap();
    assert!(app.md_has_tasks());
    // (1) Without focus (Tab hasn't been pressed), toggling doesn't write anything.
    assert!(app.tab.focused_item.is_none());
    app.md_toggle_focused_task();
    assert_eq!(
        std::fs::read_to_string(&f).unwrap(),
        "- [ ] a\n",
        "未フォーカスでは書かない"
    );
    // (2) If the file disappears while focused, flash + no-op (no panic).
    app.md_focus_move(1);
    assert!(app.md_focused_task());
    std::fs::remove_file(&f).unwrap();
    app.md_toggle_focused_task();
    assert!(app.flash.is_some(), "読取エラーは flash で通知");
    assert!(!f.exists(), "書き直さない");
    std::fs::remove_dir_all(&dir).ok();
}

/// Probes whether this process is actually denied write access to a 0o444 file.
/// Under root (or any process able to bypass permission bits), permissions have no
/// effect and the write succeeds anyway (libc-independent: does not read uid directly,
/// just checks the fs behavior itself).
#[cfg(unix)]
fn write_denied_by_permissions() -> bool {
    use std::os::unix::fs::PermissionsExt;
    let probe = std::env::temp_dir().join(format!(
        "konoma_perm_probe_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::write(&probe, "x").unwrap();
    std::fs::set_permissions(&probe, std::fs::Permissions::from_mode(0o444)).unwrap();
    let denied = std::fs::OpenOptions::new()
        .write(true)
        .open(&probe)
        .is_err();
    let _ = std::fs::set_permissions(&probe, std::fs::Permissions::from_mode(0o644));
    let _ = std::fs::remove_file(&probe);
    denied
}

/// Even when a write is denied (a read-only file), it doesn't crash: notifies via flash and
/// leaves the content intact (principle #3).
#[cfg(unix)]
#[test]
fn md_task_toggle_flashes_on_write_error() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    use std::os::unix::fs::PermissionsExt;
    // This test depends on "permission bits denying the write". Run as root (or any process
    // able to bypass permission bits), the write succeeds even at 0o444, and the test alone
    // goes red for environmental reasons even though the product behaves correctly.
    // (Hit this while verifying behavior as root in a Linux container; the product code is
    // left unchanged.)
    if !write_denied_by_permissions() {
        eprintln!(
            "md_task_toggle_flashes_on_write_error: このプロセスはパーミッションで書込みを拒否されない(root 等)ためスキップ"
        );
        return;
    }
    let dir = std::env::temp_dir().join("konoma_md_task_wrerr_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("todo.md");
    std::fs::write(&f, "- [ ] a\n").unwrap();
    let mut app = App::new(dir.canonicalize().unwrap(), Config::default()).unwrap();
    app.tab.selected = app.tab.entries.iter().position(|e| !e.is_dir).unwrap();
    app.tree_activate().unwrap();
    let mut term = Terminal::new(TestBackend::new(60, 8)).unwrap();
    term.draw(|fr| crate::ui::render(fr, &mut app)).unwrap();
    app.md_focus_move(1);
    // Make it read-only: the read (verification) passes, but the write fails with EACCES.
    std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o444)).unwrap();
    app.md_toggle_focused_task();
    assert!(app.flash.is_some(), "書込みエラーは flash で通知");
    // The write was denied, so the content stays as it was.
    std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(
        std::fs::read_to_string(&f).unwrap(),
        "- [ ] a\n",
        "拒否されたら壊さない"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// Rendering the file-info popup (`i`): checks, against a real render buffer, the branches for a
/// regular file / a directory (item count) / an unreadable target (error line) / no target
/// (early return, no crash).
#[test]
fn ui_info_popup_renders_variants() {
    use crate::i18n::{tr, Lang, Msg};
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    fn info_screen(app: &App) -> String {
        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        term.draw(|f| crate::ui::info::render(f, app, f.area()))
            .unwrap();
        let buf = term.backend().buffer();
        let w = buf.area.width as usize;
        let mut out = String::new();
        for (i, c) in buf.content().iter().enumerate() {
            out.push_str(c.symbol());
            if (i + 1) % w == 0 {
                out.push('\n');
            }
        }
        out
    }
    let dir = unique_tmp("konoma_ui_info_test");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("reg.txt"), "hello").unwrap();
    let sub = dir.join("subdir");
    std::fs::create_dir_all(&sub).unwrap();
    std::fs::write(sub.join("a.txt"), "a").unwrap();
    std::fs::write(sub.join("b.txt"), "b").unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.lang = Lang::En;

    // A regular file: Type=file.
    app.tab.selected = app
        .tab
        .entries
        .iter()
        .position(|e| e.path.ends_with("reg.txt"))
        .unwrap();
    let s = info_screen(&app);
    assert!(s.contains(tr(Lang::En, Msg::InfoFile)), "file 種別: {s}");

    // A directory: Type=directory + item count.
    app.tab.selected = app
        .tab
        .entries
        .iter()
        .position(|e| e.path.ends_with("subdir"))
        .unwrap();
    let s = info_screen(&app);
    assert!(
        s.contains(tr(Lang::En, Msg::InfoDirectory)),
        "directory 種別"
    );
    assert!(s.contains(tr(Lang::En, Msg::InfoItems)), "項目数を表示");

    // An unreadable target (missing): an error line (no crash).
    app.tab.mode = Mode::Preview;
    app.tab.preview_path = Some(dir.join("gone.txt"));
    let s = info_screen(&app);
    assert!(s.contains(tr(Lang::En, Msg::Failed)), "エラー表示: {s}");

    // No target: early return (nothing is drawn).
    app.tab.preview_path = None;
    let s = info_screen(&app);
    assert!(
        !s.contains(tr(Lang::En, Msg::InfoFile)),
        "対象なしは描画しない"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// File info for a symlink: draws Type=symlink plus a Target line pointing to the link target.
#[cfg(unix)]
#[test]
fn ui_info_popup_renders_symlink_target() {
    use crate::i18n::{tr, Lang, Msg};
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    let dir = unique_tmp("konoma_ui_info_symlink_test");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("real.txt"), "x").unwrap();
    std::os::unix::fs::symlink(dir.join("real.txt"), dir.join("link")).unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.lang = Lang::En;
    app.tab.selected = app
        .tab
        .entries
        .iter()
        .position(|e| e.path.ends_with("link"))
        .unwrap();
    let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
    term.draw(|f| crate::ui::info::render(f, &app, f.area()))
        .unwrap();
    let buf = term.backend().buffer();
    let w = buf.area.width as usize;
    let mut s = String::new();
    for (i, c) in buf.content().iter().enumerate() {
        s.push_str(c.symbol());
        if (i + 1) % w == 0 {
            s.push('\n');
        }
    }
    assert!(s.contains(tr(Lang::En, Msg::Symlink)), "symlink 種別: {s}");
    assert!(s.contains(tr(Lang::En, Msg::InfoTarget)), "リンク先を表示");
    std::fs::remove_dir_all(&dir).ok();
}

/// paste-jump's navigation branches: (1) absolute path resolution; (2) a GitHub URL whose tail
/// doesn't actually exist → a not-found flash; (3) passing `#L` to decorated Markdown switches to
/// the raw source (making line addressing possible); (4) for a table (non-windowed), a line jump
/// is a no-op (no crash).
#[test]
fn paste_jump_navigation_branches() {
    let dir = unique_tmp("konoma_paste_nav_test");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("abs_target.txt"), "x\ny\nz\n").unwrap();
    std::fs::write(dir.join("doc.md"), "# H1\n\nl2\nl3\nl4\nl5\nl6\n").unwrap();
    std::fs::write(dir.join("data.csv"), "a,b\n1,2\n3,4\n").unwrap();
    let dir = dir.canonicalize().unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();

    // (1) An absolute path (resolve_local_path's absolute branch) → preview.
    let abs = dir.join("abs_target.txt");
    app.paste_jump_from(abs.to_str().unwrap());
    assert_eq!(app.tab.mode, Mode::Preview);
    assert!(app
        .tab
        .preview_path
        .as_deref()
        .map(|p| p.ends_with("abs_target.txt"))
        .unwrap_or(false));

    // (2) A GitHub URL whose tail doesn't actually exist → a not-found flash
    // (resolve_url_components returns None).
    app.flash = None;
    app.paste_jump_from("https://github.com/o/r/blob/main/nope/missing.rs");
    assert!(
        app.flash
            .as_deref()
            .map(|f| f.contains("missing.rs"))
            .unwrap_or(false),
        "not found flash: {:?}",
        app.flash
    );

    // (3) Decorated Markdown + `#L` → switches to the raw source (preview_goto_line's decorated branch).
    app.paste_jump_from("doc.md#L5");
    assert!(app
        .tab
        .preview_path
        .as_deref()
        .map(|p| p.ends_with("doc.md"))
        .unwrap_or(false));
    assert!(app.is_md_raw(), "装飾 md + 行 → raw ソースへ切替");

    // (4) `#L` against a table (non-windowed) → the line jump is a no-op (stays on the table surface).
    app.paste_jump_from("data.csv#L2");
    assert!(
        app.is_table_preview(),
        "csv はテーブル面のまま(行ジャンプ no-op)"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// When opened at a subdirectory of a repo, a relative path that doesn't exist directly under
/// root is still resolved relative to the repo (workdir) (resolve_local_path's workdir
/// fallback branch).
#[cfg(feature = "git")]
#[test]
fn paste_jump_relative_resolves_against_repo_workdir() {
    let dir = unique_tmp("konoma_paste_workdir_test");
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("docs")).unwrap();
    std::fs::write(dir.join("docs/guide.md"), "# G\n").unwrap();
    for args in [
        vec!["init", "-q", "."],
        vec!["config", "user.email", "t@t"],
        vec!["config", "user.name", "t"],
    ] {
        let ok = std::process::Command::new("git")
            .current_dir(&dir)
            .args(&args)
            .output()
            .unwrap()
            .status
            .success();
        assert!(ok, "git {args:?}");
    }
    let repo = dir.canonicalize().unwrap();
    let subdir = repo.join("src");
    let mut app = App::new(subdir.clone(), Config::default()).unwrap();
    assert_eq!(app.tab.root, subdir, "起動 root は src サブディレクトリ");
    // docs/guide.md doesn't exist directly under root (src), but it resolves relative to the
    // repo workdir.
    app.paste_jump_from("docs/guide.md");
    assert_eq!(app.tab.mode, Mode::Preview);
    assert!(
        app.tab
            .preview_path
            .as_deref()
            .map(|p| p.ends_with("docs/guide.md"))
            .unwrap_or(false),
        "workdir 基準で解決: {:?}",
        app.tab.preview_path
    );
    std::fs::remove_dir_all(&repo).ok();
}

#[test]
fn md_items_mix_links_and_tasks_in_document_order() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    // Links and checkboxes are loaded into a single Tab cycle in document order.
    let dir = std::env::temp_dir().join("konoma_md_items_mix_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("doc.md"),
        "[l1](https://x/1)\n\n- [ ] t1\n\n[l2](https://x/2)\n",
    )
    .unwrap();
    let mut app = App::new(dir.canonicalize().unwrap(), Config::default()).unwrap();
    app.tab.selected = app.tab.entries.iter().position(|e| !e.is_dir).unwrap();
    app.tree_activate().unwrap();
    let mut term = Terminal::new(TestBackend::new(60, 10)).unwrap();
    term.draw(|fr| crate::ui::render(fr, &mut app)).unwrap();
    let kinds: Vec<bool> = app
        .md_items
        .iter()
        .map(|it| matches!(it.kind, MdItemKind::Task { .. }))
        .collect();
    assert_eq!(kinds, vec![false, true, false], "Link→Task→Link の順");
    app.md_focus_move(1);
    assert!(!app.md_focused_task());
    app.md_focus_move(1);
    assert!(app.md_focused_task(), "2番目=チェックボックス");
    app.md_focus_move(1);
    assert!(!app.md_focused_task());
    app.md_focus_move(1);
    assert_eq!(app.tab.focused_item, Some(0), "巡回で先頭へ戻る");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn md_code_block_is_tab_focusable_and_copies_source() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    // Links, code blocks, and tasks are loaded into a single Tab cycle in document order, and
    // focusing a code block lets Enter copy its raw source (verified against focused_code_text).
    let dir = std::env::temp_dir().join("konoma_md_code_focus_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("doc.md"),
        "[l1](https://x/1)\n\n```rust\nfn main() {}\nlet x = 1;\n```\n\n- [ ] t1\n",
    )
    .unwrap();
    let mut app = App::new(dir.canonicalize().unwrap(), Config::default()).unwrap();
    app.tab.selected = app.tab.entries.iter().position(|e| !e.is_dir).unwrap();
    app.tree_activate().unwrap();
    let mut term = Terminal::new(TestBackend::new(60, 12)).unwrap();
    term.draw(|fr| crate::ui::render(fr, &mut app)).unwrap();

    let kinds: Vec<&str> = app
        .md_items
        .iter()
        .map(|it| match it.kind {
            MdItemKind::Link { .. } => "link",
            MdItemKind::Task { .. } => "task",
            MdItemKind::CodeBlock => "code",
            MdItemKind::MermaidFence { .. } => "mermaid",
            MdItemKind::Details { .. } => "details",
        })
        .collect();
    assert_eq!(kinds, vec!["link", "code", "task"], "Link→Code→Task の順");

    // Code copy isn't available without focus / while focused on a link.
    assert!(app.focused_code_text().is_none());
    app.md_focus_move(1); // the link
    assert!(!app.md_focused_code());
    app.md_focus_move(1); // the code block
    assert!(app.md_focused_code(), "2番目=コードブロック");
    assert_eq!(
        app.focused_code_text().as_deref(),
        Some("fn main() {}\nlet x = 1;"),
        "生ソース(装飾/ハイライトを含まない)をコピー"
    );

    // Focus cue: after re-rendering, the code block's header row (containing the language name
    // rust) is reversed (REVERSED) across its full width. Verified via the render buffer's cell
    // modifiers (deterministic, independent of tmux).
    term.draw(|fr| crate::ui::render(fr, &mut app)).unwrap();
    {
        use ratatui::style::Modifier;
        let buf = term.backend().buffer();
        let w = buf.area.width;
        let header_row = (0..buf.area.height).find(|&y| {
            (0..w)
                .map(|x| buf[(x, y)].symbol())
                .collect::<String>()
                .contains("rust")
        });
        let y = header_row.expect("rust ヘッダ行が描画されている");
        // The header row's visible cells (including ▎) carry REVERSED.
        let reversed = (0..w)
            .filter(|&x| buf[(x, y)].symbol().trim() != "" || buf[(x, y)].symbol() == "▎")
            .any(|x| buf[(x, y)].modifier.contains(Modifier::REVERSED));
        assert!(
            reversed,
            "フォーカス中コードブロックのヘッダ行が反転していない"
        );
    }

    // Copy action (invoked via `y`; since the clipboard is environment-dependent, we only check
    // that it doesn't panic).
    app.md_copy_focused_code();
    // Enter does nothing on a code block (there's nothing to open/toggle).
    app.md_activate_focused().unwrap();
    app.md_focus_move(1); // the task
    assert!(!app.md_focused_code());
    assert!(app.focused_code_text().is_none());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn links_collapse_to_label_only_with_optional_icon() {
    use ratatui::style::{Color, Modifier, Style};
    let blue = Style::new()
        .fg(Color::Blue)
        .add_modifier(Modifier::UNDERLINED);
    // The span sequence corresponding to tui-markdown's "label (URL)". Followed by a trailing ".".
    let lines = vec![Line::from(vec![
        Span::raw("See "),
        Span::raw("the docs"),
        Span::raw(" ("),
        Span::styled("https://x/y", blue),
        Span::raw(")."),
    ])];
    // icons OFF: only the label is shown, the URL goes into targets, the trailing "." is kept.
    let (off, targets) = collapse_links(lines.clone(), false);
    assert_eq!(targets, vec!["https://x/y".to_string()]);
    let joined: String = off[0].spans.iter().map(|s| s.content.as_ref()).collect();
    assert_eq!(joined, "See the docs.", "URL 併記が消えてラベルのみ");
    assert_eq!(
        off[0].spans.iter().filter(|s| is_link_span(s)).count(),
        1,
        "リンク span は1つ"
    );
    // icons ON: a link icon is prefixed to the label.
    let (on, _) = collapse_links(lines, true);
    let link = on[0].spans.iter().find(|s| is_link_span(s)).unwrap();
    assert!(
        link.content.starts_with(crate::ui::icons::link_icon()),
        "リンクアイコンが前置されていない: {:?}",
        link.content
    );
}

#[test]
fn find_bare_links_covers_urls_www_email_and_trims() {
    let links_of = |s: &str| -> Vec<(String, String)> {
        find_bare_links(s)
            .into_iter()
            .map(|(a, b, t)| (s[a..b].to_string(), t))
            .collect()
    };
    // https: the whole URL, target == text.
    assert_eq!(
        links_of("see https://example.com/path here"),
        vec![(
            "https://example.com/path".into(),
            "https://example.com/path".into()
        )]
    );
    // Trailing sentence punctuation is trimmed off the link.
    assert_eq!(
        links_of("go to https://example.com."),
        vec![("https://example.com".into(), "https://example.com".into())]
    );
    // An unbalanced closing paren is trimmed…
    assert_eq!(
        links_of("(see https://example.com)"),
        vec![("https://example.com".into(), "https://example.com".into())]
    );
    // …but balanced parens inside the URL are kept (Wikipedia-style).
    assert_eq!(
        links_of("https://en.wikipedia.org/wiki/Rust_(programming_language)"),
        vec![(
            "https://en.wikipedia.org/wiki/Rust_(programming_language)".into(),
            "https://en.wikipedia.org/wiki/Rust_(programming_language)".into()
        )]
    );
    // www. keeps its text but the target gains an http:// prefix.
    assert_eq!(
        links_of("visit www.example.com today"),
        vec![("www.example.com".into(), "http://www.example.com".into())]
    );
    // Email → mailto: target.
    assert_eq!(
        links_of("mail me@example.com please"),
        vec![("me@example.com".into(), "mailto:me@example.com".into())]
    );
    // Must not start mid-word (preceded by an alphanumeric byte).
    assert!(links_of("xhttps://example.com").is_empty());
    // CJK immediately before is a valid boundary.
    assert_eq!(links_of("見るhttps://example.com").len(), 1);
    // CJK punctuation right after a URL ends the run (no ASCII space needed) — konoma's CJK audience.
    assert_eq!(
        links_of("見るhttps://example.com、と"),
        vec![("https://example.com".into(), "https://example.com".into())]
    );
    assert_eq!(
        links_of("（https://example.com）"),
        vec![("https://example.com".into(), "https://example.com".into())]
    );
    // A bare @ without a dotted domain is not an email.
    assert!(links_of("a@b").is_empty());
    // Two links, in document order.
    let two = links_of("http://a.com and me@b.org");
    assert_eq!(two.len(), 2);
    assert_eq!(two[0].0, "http://a.com");
    assert_eq!(two[1].1, "mailto:me@b.org");
}

#[test]
fn autolink_bare_urls_links_plain_text_not_code_and_keeps_order() {
    use ratatui::style::{Color, Modifier, Style};
    let blue = Style::new()
        .fg(Color::Blue)
        .add_modifier(Modifier::UNDERLINED);
    let code = Style::new().bg(Color::Rgb(40, 40, 40)); // inline code carries a background
    let lines = vec![
        // Existing collapsed link (target "u0") followed by a bare URL on the same line.
        Line::from(vec![
            Span::styled("docs", blue),
            Span::raw(" and https://bare.example"),
        ]),
        // A URL inside a code span (bg set) must never be auto-linked.
        Line::from(vec![
            Span::raw("code "),
            Span::styled("https://nope.example", code),
        ]),
    ];
    let (out, targets) = autolink_bare_urls(lines, vec!["u0".to_string()]);
    // Targets stay in document order: existing link first, then the bare URL.
    assert_eq!(
        targets,
        vec!["u0".to_string(), "https://bare.example".to_string()]
    );
    assert_eq!(
        out[0].spans.iter().filter(|s| is_link_span(s)).count(),
        2,
        "existing link + newly auto-linked URL"
    );
    assert_eq!(
        out[1].spans.iter().filter(|s| is_link_span(s)).count(),
        0,
        "a URL inside a code span must not be auto-linked"
    );
}

#[test]
fn bare_url_becomes_a_focusable_md_item() {
    let dir = std::env::temp_dir().join("konoma_autolink_item_test");
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    // A plain paragraph line with a bare URL (as tui-markdown emits: one raw span).
    let lines = vec![Line::from(Span::raw(
        "visit https://konoma.example for docs",
    ))];
    let _ = app.decorate_md_items(lines);
    assert_eq!(app.md_items.len(), 1, "the bare URL is a Tab item");
    match &app.md_items[0].kind {
        MdItemKind::Link { target } => assert_eq!(target, "https://konoma.example"),
        _ => panic!("expected a link item"),
    }
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn md_autolink_false_leaves_bare_urls_plain() {
    let dir = std::env::temp_dir().join("konoma_autolink_off_test");
    std::fs::create_dir_all(&dir).unwrap();
    let mut cfg = Config::default();
    cfg.ui.md_autolink = false;
    let mut app = App::new(dir.clone(), cfg).unwrap();
    let lines = vec![Line::from(Span::raw("visit https://konoma.example"))];
    let _ = app.decorate_md_items(lines);
    assert!(app.md_items.is_empty(), "autolink off = no link item");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn github_slug_matches_common_headings() {
    assert_eq!(github_slug("Web parity demo"), "web-parity-demo");
    // Punctuation dropped; the surrounding spaces still become hyphens (GitHub keeps consecutive).
    assert_eq!(github_slug("Foo & Bar"), "foo--bar");
    // `-` and `_` are kept; letters lowercased.
    assert_eq!(github_slug("Under_score-Dash"), "under_score-dash");
    assert_eq!(github_slug("  Trim Me  "), "trim-me");
    // CJK letters are kept (headings in Japanese anchor too).
    assert_eq!(github_slug("コード block"), "コード-block");
}

#[test]
fn compute_md_anchors_from_headings_with_dedup() {
    use crate::preview::markdown::{render_markdown, CodeStyle};
    let md = "# Intro\n\n## Setup\n\ntext\n\n## Setup\n\nmore\n";
    let lines = render_markdown(md, 60, CodeStyle::default(), "TwoDark", false);
    let slugs: Vec<String> = compute_md_anchors(&lines)
        .into_iter()
        .map(|(s, _)| s)
        .collect();
    // GitHub disambiguates duplicate slugs with -1, -2, …
    assert_eq!(
        slugs,
        vec![
            "intro".to_string(),
            "setup".to_string(),
            "setup-1".to_string()
        ]
    );
}

#[test]
fn replace_emoji_shortcodes_converts_known_keeps_unknown() {
    let rocket = emojis::get_by_shortcode("rocket").unwrap().as_str();
    let tada = emojis::get_by_shortcode("tada").unwrap().as_str();
    assert_eq!(
        replace_emoji_shortcodes("ship it :rocket:"),
        Some(format!("ship it {rocket}"))
    );
    // Multiple shortcodes with surrounding text.
    assert_eq!(
        replace_emoji_shortcodes("a :rocket: b :tada:"),
        Some(format!("a {rocket} b {tada}"))
    );
    // GitHub-custom shortcode with no Unicode equivalent stays literal.
    assert_eq!(replace_emoji_shortcodes(":shipit:"), None);
    // Non-shortcode colons (a clock time) are untouched.
    assert_eq!(replace_emoji_shortcodes("meet at 10:30"), None);
    assert_eq!(replace_emoji_shortcodes("plain text"), None);
}

#[test]
fn substitute_emoji_skips_code_spans() {
    use ratatui::style::{Color, Style};
    let code = Style::new().bg(Color::Rgb(40, 40, 40)); // inline code carries a background
    let lines = vec![Line::from(vec![
        Span::raw("say :rocket: "),
        Span::styled(":rocket:", code),
    ])];
    let out = substitute_emoji(lines);
    assert!(
        !out[0].spans[0].content.contains(":rocket:"),
        "plain-text shortcode is converted"
    );
    assert_eq!(
        out[0].spans[1].content.as_ref(),
        ":rocket:",
        "shortcode inside a code span stays literal"
    );
}

#[test]
fn md_emoji_false_leaves_shortcodes() {
    let dir = std::env::temp_dir().join("konoma_emoji_off_test");
    std::fs::create_dir_all(&dir).unwrap();
    let mut cfg = Config::default();
    cfg.ui.md_emoji = false;
    let app = App::new(dir.clone(), cfg).unwrap();
    let (lines, _) = app.postprocess_md(vec![Line::from(Span::raw("hi :rocket:"))]);
    let joined: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
    assert!(joined.contains(":rocket:"), "emoji off keeps the shortcode");
    std::fs::remove_dir_all(&dir).ok();
}

// Regression: with `code_bg = "none"` (a supported theme setting) inline code has NO background —
// only `fg(White)` — and code-block lines have no fill either, only the `▎` gutter. Autolink/emoji
// must still leave code content literal, keying off the style/gutter rather than a background.
#[test]
fn autolink_and_emoji_skip_code_without_background() {
    use ratatui::style::{Color, Style};
    let white = Style::new().fg(Color::White); // inline code when code_bg = "none"
    let gutter = Style::new().fg(Color::Cyan); // code-block gutter (CODE_GUTTER_FG = Cyan)

    // Inline code (fg White, no bg): a URL and a shortcode inside it stay literal.
    let inline = vec![Line::from(vec![
        Span::raw("see "),
        Span::styled("https://x.example :rocket:", white),
    ])];
    let (out, targets) = autolink_bare_urls(inline.clone(), vec![]);
    assert!(targets.is_empty(), "no link created inside inline code");
    assert_eq!(out[0].spans.iter().filter(|s| is_link_span(s)).count(), 0);
    let em = substitute_emoji(inline);
    assert!(
        em[0].spans[1].content.contains(":rocket:"),
        "shortcode stays literal in inline code"
    );

    // Code-block line (first span is the `▎` gutter): the whole line is skipped.
    let block = vec![Line::from(vec![
        Span::styled("▎ ", gutter),
        Span::raw("curl https://y.example # :tada:"),
    ])];
    let (_, btargets) = autolink_bare_urls(block.clone(), vec![]);
    assert!(btargets.is_empty(), "no link created inside a code block");
    let bem = substitute_emoji(block);
    let joined: String = bem[0].spans.iter().map(|s| s.content.as_ref()).collect();
    assert!(
        joined.contains(":tada:"),
        "shortcode stays literal in code block"
    );
}

#[test]
fn decorate_links_highlights_focused() {
    use ratatui::style::{Color, Modifier, Style};
    let dir = std::env::temp_dir().join("konoma_links_hl_test");
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    let link = |t: &str| {
        Span::styled(
            t.to_string(),
            Style::new()
                .fg(Color::Blue)
                .add_modifier(Modifier::UNDERLINED),
        )
    };
    let lines = vec![
        Line::from(vec![Span::raw("a "), link("u0")]),
        Line::from(vec![Span::raw("b "), link("u1")]),
    ];
    app.tab.focused_item = Some(1);
    let out = app.decorate_md_items(lines);
    assert_eq!(app.md_items.len(), 2);
    // Only the focused (1st) link span carries REVERSED.
    let rev = |line: &Line<'static>| -> bool {
        line.spans
            .iter()
            .any(|s| is_link_span(s) && s.style.add_modifier.contains(Modifier::REVERSED))
    };
    assert!(!rev(&out[0]), "未フォーカスのリンクは反転しない");
    assert!(rev(&out[1]), "フォーカス中リンクが反転していない");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn code_file_decorates_with_colored_syntax() {
    // Opening a .rs file resolves to the Code kind, and decorated_lines returns colored
    // (Rgb foreground) lines.
    use ratatui::style::Color;
    let dir = std::env::temp_dir().join("konoma_code_decorate_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.rs"), b"fn main() { let x = 1; }\n").unwrap();
    let mut app = App::new(dir.canonicalize().unwrap(), Config::default()).unwrap();
    let i = app.tab.entries.iter().position(|e| !e.is_dir).unwrap();
    app.tab.selected = i;
    app.tree_activate().unwrap();
    assert!(
        matches!(app.tab.preview_kind, Some(PreviewKind::Code(_))),
        "Code 種別に解決されない"
    );
    let lines = app.decorated_lines(80);
    assert!(!lines.is_empty(), "装飾行が空");
    let colored = lines
        .iter()
        .flat_map(|l| l.spans.iter())
        .any(|s| matches!(s.style.fg, Some(Color::Rgb(_, _, _))));
    assert!(colored, "シンタックスハイライトの前景色が付いていない");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn tree_filter_finds_recursively_then_clears() {
    // `/` filter: recursively collects everything under root and narrows entries by a
    // case-insensitive substring match. Esc returns to normal.
    let dir = std::env::temp_dir().join("konoma_tree_filter_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src/ui")).unwrap();
    std::fs::write(dir.join("src/ui/tree.rs"), b"x").unwrap();
    std::fs::write(dir.join("src/main.rs"), b"x").unwrap();
    std::fs::write(dir.join("README.md"), b"x").unwrap();
    let mut app = App::new(dir.canonicalize().unwrap(), Config::default()).unwrap();
    let normal_count = app.tab.entries.len();

    app.start_filter();
    assert!(app.is_filtering());
    // Right after `/` (empty query), it does not expand everything and shows nothing.
    assert!(
        app.tab.entries.is_empty(),
        "クエリ空のとき結果は0件であるべき(全展開しない)"
    );
    for c in "TREE".chars() {
        app.filter_input_push(c); // matches even in uppercase (case-insensitive)
    }
    assert_eq!(app.filter_query(), Some("TREE"));
    // Recursively finds tree.rs, 2 levels down.
    assert_eq!(app.tab.entries.len(), 1, "tree 一致は1件");
    assert!(app.tab.entries[0].path.ends_with("tree.rs"));

    // Deleting one character widens the results live ("TRE"). tree.rs still remains.
    app.filter_input_backspace();
    assert_eq!(app.filter_query(), Some("TRE"));
    assert!(app.tab.entries.iter().any(|e| e.path.ends_with("tree.rs")));

    // Commit (Enter): exits input mode but keeps the filter applied.
    app.filter_commit();
    assert!(!app.is_filtering());
    assert!(app.filter_query().is_some());

    // Clear (Esc): returns to the normal tree.
    app.filter_clear();
    assert!(app.filter_query().is_none());
    assert_eq!(app.tab.entries.len(), normal_count, "通常ツリーに復帰");
    std::fs::remove_dir_all(&dir).ok();
}

/// Helper: an `Entry` wrapping a bare file name (as if it were the sole path component), for
/// exercising `fuzzy_filter_pool` without touching the filesystem.
fn named_entry(name: &str) -> Entry {
    Entry {
        path: std::path::PathBuf::from(name),
        is_dir: false,
        depth: 0,
        expanded: false,
    }
}

#[test]
fn filter_fuzzy_finds_non_contiguous_subsequence() {
    // The core of `[ui] filter_mode`'s default ("fuzzy"): a scattered match (matches even without
    // a contiguous substring, as long as the characters appear in order). "aprs" matches because
    // "app_resolver.rs" contains a-p-r-s in order.
    let pool = vec![
        named_entry("app_resolver.rs"),
        named_entry("main.rs"),
        named_entry("readme.md"),
    ];
    let hits = fuzzy_filter_pool(&pool, "aprs");
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert!(hits[0].path.ends_with("app_resolver.rs"));
}

#[test]
fn filter_fuzzy_ranks_better_match_first() {
    // Even for the same query, a tighter match (contiguous / closer to the start) ranks higher.
    let pool = vec![
        // "app"'s a-p-p is scattered far apart (a weak match): a(pretty)...p(rogram)...p(the
        // second one in rogram).
        named_entry("a_pretty_neat_program.rs"),
        // "app" appears contiguously right at the start (a strong match).
        named_entry("app.rs"),
    ];
    let hits = fuzzy_filter_pool(&pool, "app");
    assert_eq!(hits.len(), 2, "{hits:?}");
    assert!(
        hits[0].path.ends_with("app.rs"),
        "引き締まった一致が先頭に来るべき: {hits:?}"
    );
}

#[test]
fn filter_fuzzy_multi_word_query_is_and() {
    // Space-separated multiple words are AND'd (fzf-style). "app rs" matches as long as both
    // are present (even if not adjacent).
    let pool = vec![named_entry("app_resolver.rs"), named_entry("readme.md")];
    let hits = fuzzy_filter_pool(&pool, "app rs");
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert!(hits[0].path.ends_with("app_resolver.rs"));
}

#[test]
fn filter_fuzzy_no_matches_returns_empty() {
    let pool = vec![named_entry("readme.md"), named_entry("notes.txt")];
    assert!(fuzzy_filter_pool(&pool, "qzqzqznope").is_empty());
}

#[test]
fn filter_fuzzy_handles_cjk_filenames_without_panic() {
    let pool = vec![
        named_entry("日本語.txt"),
        named_entry("readme.md"),
        named_entry("メモ.md"),
    ];
    // A CJK query matches a CJK filename.
    let hits = fuzzy_filter_pool(&pool, "日本語");
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert!(hits[0].path.ends_with("日本語.txt"));
    // Even a non-matching ASCII query does not panic (empty result).
    assert!(fuzzy_filter_pool(&pool, "zzzz").is_empty());
    // A partially-matching CJK query is also handled safely.
    let hits2 = fuzzy_filter_pool(&pool, "モ");
    assert_eq!(hits2.len(), 1, "{hits2:?}");
    assert!(hits2[0].path.ends_with("メモ.md"));
}

#[test]
fn filter_fuzzy_is_case_insensitive_both_directions() {
    // Always case-insensitive (CaseMatching::Ignore): keeps the old substring mode's
    // "always case-insensitive" behavior in fuzzy mode too (fzf/ripgrep-style smart-case,
    // where an all-uppercase query becomes case-sensitive, is not adopted).
    let pool = vec![named_entry("TREE.txt"), named_entry("tree.rs")];
    assert_eq!(
        fuzzy_filter_pool(&pool, "tree").len(),
        2,
        "小文字クエリは両方に一致"
    );
    assert_eq!(
        fuzzy_filter_pool(&pool, "TREE").len(),
        2,
        "大文字クエリでも大小無視で両方に一致(legacy substring と同じ挙動)"
    );
}

/// Speed guard (loose bound, debug build): fuzzy-filtering a 30,000-entry pool (the tree filter's
/// realistic worst case — `collect_all`'s own cap is 50,000) must not visibly stall the UI thread
/// on every keystroke. `Matcher` reuse (the thread-local in `bookmark_actions.rs`) is what keeps
/// this fast — rebuilding one per call would add its own ~135KB allocation on top.
#[test]
fn filter_fuzzy_large_pool_is_bounded() {
    let pool: Vec<Entry> = (0..30_000)
        .map(|i| named_entry(&format!("module_{i}_helper_resolver.rs")))
        .collect();
    let t = std::time::Instant::now();
    let hits = fuzzy_filter_pool(&pool, "mhr");
    let dt = t.elapsed();
    assert!(!hits.is_empty(), "少なくとも一部は一致するはず");
    assert!(
        dt < std::time::Duration::from_millis(500),
        "30,000 件のファジー絞り込みが遅すぎる(回帰?): {dt:?}"
    );
}

#[test]
fn filter_substring_mode_excludes_scattered_query_but_keeps_legacy_behavior() {
    // `[ui] filter_mode = "substring"`: scattered matching is disabled (contiguous substring
    // only), while case-insensitivity is kept.
    let dir = unique_tmp("konoma_filter_substring_mode_test");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("app_resolver.rs"), b"x").unwrap();
    std::fs::write(dir.join("main.rs"), b"x").unwrap();
    std::fs::write(dir.join("README.md"), b"x").unwrap();

    let mut cfg = Config::default();
    cfg.ui.filter_mode = "substring".into();
    let mut app = App::new(dir.canonicalize().unwrap(), cfg).unwrap();

    app.start_filter();
    for c in "aprs".chars() {
        app.filter_input_push(c);
    }
    assert!(
        app.tab.entries.is_empty(),
        "substring モードでは飛び石一致(aprs→app_resolver.rs)はしない: {:?}",
        app.tab.entries
    );

    // Meanwhile, the normal (case-insensitive) contiguous substring match still works.
    app.filter_clear();
    app.start_filter();
    for c in "README".chars() {
        app.filter_input_push(c);
    }
    assert_eq!(app.tab.entries.len(), 1, "{:?}", app.tab.entries);
    assert!(app.tab.entries[0].path.ends_with("README.md"));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn text_filter_survives_fs_refresh() {
    // Regression: when refresh_fs_inner (the very path a tab switch goes through) ran while a
    // `/` filter was active, the full entries list built by rebuild_tree would overwrite the
    // filtered result — the query ("txt") stayed set, but the list reverted to all entries
    // including .rs.
    let dir = unique_tmp("konoma_text_filter_refresh_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("alpha.txt"), b"x").unwrap();
    std::fs::write(dir.join("beta.txt"), b"x").unwrap();
    std::fs::write(dir.join("gamma.rs"), b"x").unwrap();
    std::fs::write(dir.join("delta.rs"), b"x").unwrap();
    let mut app = App::new(dir.canonicalize().unwrap(), Config::default()).unwrap();

    app.start_filter();
    for c in "txt".chars() {
        app.filter_input_push(c);
    }
    app.filter_commit();
    assert_eq!(app.filter_query(), Some("txt"));
    assert_eq!(app.tab.entries.len(), 2, "txt 一致は2件のはず");
    assert!(
        app.tab
            .entries
            .iter()
            .all(|e| e.path.extension().and_then(|s| s.to_str()) == Some("txt")),
        ".rs が混入している"
    );

    // Directly hit the path a tab switch actually goes through (refresh_fs_after_tab_switch) =
    // the tightest way to reproduce it.
    app.refresh_fs_after_tab_switch().unwrap();

    assert_eq!(
        app.filter_query(),
        Some("txt"),
        "query は refresh を跨いで維持されるべき"
    );
    assert_eq!(
        app.tab.entries.len(),
        2,
        "refresh 後も txt 絞り込みが維持されるべき(バグは rebuild_tree で全4件に戻る)"
    );
    assert!(
        app.tab
            .entries
            .iter()
            .all(|e| e.path.extension().and_then(|s| s.to_str()) == Some("txt")),
        "refresh 後に .rs が混入してはいけない"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn small_code_file_is_also_windowed() {
    // #1: Code is windowed regardless of size (doesn't highlight every line) = instant even for
    // small files.
    let dir = std::env::temp_dir().join("konoma_small_windowed_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.rs"), b"fn main() {}\n").unwrap();
    let mut app = App::new(dir.canonicalize().unwrap(), Config::default()).unwrap();
    let i = app.tab.entries.iter().position(|e| !e.is_dir).unwrap();
    app.tab.selected = i;
    app.tree_activate().unwrap();
    assert!(
        app.is_windowed(),
        "小さい Code ファイルも windowed であるべき"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn line_numbers_gutter_tracks_position() {
    // #3: ui.line_numbers=true shows the line-number gutter. Start=1, and G gives the correct
    // line number at the end.
    use std::io::Write;
    let dir = std::env::temp_dir().join("konoma_linenum_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("a.rs");
    {
        let mut f = std::fs::File::create(&path).unwrap();
        for i in 1..=30 {
            writeln!(f, "let v{i} = {i};").unwrap();
        }
    }
    let mut cfg = Config::default();
    cfg.ui.line_numbers = true;
    let mut app = App::new(dir.canonicalize().unwrap(), cfg).unwrap();
    let i = app
        .tab
        .entries
        .iter()
        .position(|e| e.path.ends_with("a.rs"))
        .unwrap();
    app.tab.selected = i;
    app.tree_activate().unwrap();
    app.tab.preview_viewport = 5;

    // The numeric part of the leading gutter span (the first span).
    let gutter_num = |lines: &[Line<'static>], row: usize| -> String {
        lines[row].spans[0].content.trim().to_string()
    };
    let top = app.windowed_lines(5, 80);
    assert_eq!(gutter_num(&top, 0), "1", "先頭行番号が 1 でない");
    assert_eq!(gutter_num(&top, 1), "2");
    // G to the end → the last 5 lines are 26..30. The first line number = 26.
    app.preview_to_bottom();
    let bot = app.windowed_lines(5, 80);
    assert_eq!(
        gutter_num(&bot, 0),
        "26",
        "G 後の先頭行番号が総行数基準でない"
    );
    assert_eq!(gutter_num(&bot, 4), "30", "末尾行番号が 30 でない");

    // With OFF (the default), no gutter is shown (the first span is not a line number).
    let mut app2 = App::new(dir.canonicalize().unwrap(), Config::default()).unwrap();
    app2.tab.selected = app2
        .tab
        .entries
        .iter()
        .position(|e| e.path.ends_with("a.rs"))
        .unwrap();
    app2.tree_activate().unwrap();
    app2.tab.preview_viewport = 5;
    let off = app2.windowed_lines(5, 80);
    let first: String = off[0].spans[0].content.to_string();
    assert!(
        !first.trim().starts_with('1'),
        "OFF なのに行番号ガター: {first:?}"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn large_code_file_uses_windowed_reading() {
    // Code above the threshold uses a less-style windowed read: it reads and colors only the
    // start/end windows without scanning the whole file.
    use ratatui::style::Color;
    use std::io::Write;
    let dir = std::env::temp_dir().join("konoma_windowed_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("big.rs");
    {
        let mut f = std::fs::File::create(&path).unwrap();
        // ~9000 lines × ~40B ≈ 350KB > WINDOW_THRESHOLD (256KB).
        for i in 1..=9000 {
            writeln!(f, "fn func_{i:05}() -> i32 {{ {i} }} // line {i}").unwrap();
        }
    }
    let mut app = App::new(dir.canonicalize().unwrap(), Config::default()).unwrap();
    let i = app
        .tab
        .entries
        .iter()
        .position(|e| e.path.ends_with("big.rs"))
        .unwrap();
    app.tab.selected = i;
    app.tree_activate().unwrap();
    assert!(app.is_windowed(), "大きいファイルが windowed にならない");
    app.tab.preview_viewport = 5;

    let joined = |lines: &[Line<'static>]| -> String {
        lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .map(|s| s.content.as_ref())
            .collect()
    };
    // The start window includes line 1.
    let top = app.windowed_lines(5, 80);
    assert!(joined(&top).contains("func_00001"), "先頭が出ない");
    // G to the end → the end window includes line 9000, and the start is not visible (no
    // full-file scan).
    app.preview_to_bottom();
    let bot = app.windowed_lines(5, 80);
    let bot_text = joined(&bot);
    assert!(bot_text.contains("func_09000"), "末尾が出ない: {bot_text}");
    assert!(!bot_text.contains("func_00001"), "末尾なのに先頭が見える");
    // The window is colored with syntect too.
    let colored = bot
        .iter()
        .flat_map(|l| l.spans.iter())
        .any(|s| matches!(s.style.fg, Some(Color::Rgb(_, _, _))));
    assert!(colored, "windowed の着色が無い");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn tab_label_reflects_tree_root_or_preview_file() {
    // Tab name: the root's directory name while showing Tree, the file name while in Preview/an image.
    let dir = std::env::temp_dir().join("konoma_tab_label_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("doc.md"), b"# hi\n").unwrap();
    let mut app = App::new(dir.canonicalize().unwrap(), Config::default()).unwrap();
    // While showing Tree, it's the root's directory name.
    assert_eq!(app.tab_label(0), "konoma_tab_label_test");
    // Preview doc.md → the tab name becomes the file name.
    let idx = app.tab.entries.iter().position(|e| !e.is_dir).unwrap();
    app.tab.selected = idx;
    app.tree_activate().unwrap();
    assert_eq!(app.tab.mode, Mode::Preview);
    assert_eq!(app.tab_label(0), "doc.md", "Preview 中はファイル名");
    // A new tab (Tree) uses the root name. The now-inactive original tab keeps the snapshotted
    // file name.
    app.tab_new().unwrap();
    assert_eq!(
        app.tab_label(1),
        "konoma_tab_label_test",
        "新規タブはルート名"
    );
    assert_eq!(
        app.tab_label(0),
        "doc.md",
        "非アクティブの Preview タブはファイル名"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn tab_switch_preserves_and_restores_preview() {
    // Each tab keeps its own preview state. Preview→new tab (Tree)→going back restores Preview
    // (it doesn't fall back to Tree).
    let dir = std::env::temp_dir().join("konoma_tab_preview_restore_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("doc.md"), b"# Hello\n\nbody\n").unwrap();
    let mut app = App::new(dir.canonicalize().unwrap(), Config::default()).unwrap();

    // Select doc.md and go to preview.
    let idx = app.tab.entries.iter().position(|e| !e.is_dir).unwrap();
    app.tab.selected = idx;
    app.tree_activate().unwrap();
    assert_eq!(app.tab.mode, Mode::Preview);
    let previewed = app.tab.preview_path.clone();
    assert!(previewed.is_some());

    // A new tab starts from Tree (it doesn't carry the preview over).
    app.tab_new().unwrap();
    assert_eq!(app.tab.mode, Mode::Tree, "新規タブは Tree");
    assert!(app.tab.preview_path.is_none(), "新規タブはプレビュー無し");

    // Going back to the original tab restores Preview and its target.
    app.tab_cycle(-1);
    assert_eq!(app.tab.mode, Mode::Preview, "戻ると Preview が復元される");
    assert_eq!(
        app.tab.preview_path, previewed,
        "プレビュー対象も復元される"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn tabs_create_switch_close_preserve_state() {
    // Verify that, in a tree with two child directories, each tab keeps its own selection position.
    let dir = std::env::temp_dir().join("konoma_tabs_test");
    std::fs::create_dir_all(dir.join("a")).unwrap();
    std::fs::create_dir_all(dir.join("b")).unwrap();
    std::fs::write(dir.join("f.txt"), b"x").unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    assert_eq!(app.tab_count(), 1);

    // In tab 1, move the selection down by one.
    app.tree_next();
    let sel1 = app.tab.selected;
    assert!(sel1 > 0);

    // New tab → 2 tabs, tab 2 is active, and the selection starts at the top.
    app.tab_new().unwrap();
    assert_eq!(app.tab_count(), 2);
    assert_eq!(app.active_tab_index(), 1);
    assert_eq!(app.tab.selected, 0);
    app.tree_last();
    let sel2 = app.tab.selected;

    // Going back to the previous tab restores its selection position.
    app.tab_cycle(-1);
    assert_eq!(app.active_tab_index(), 0);
    assert_eq!(app.tab.selected, sel1, "タブ1の選択が保たれる");

    // Jump to tab 2 by number.
    app.tab_goto(1);
    assert_eq!(app.active_tab_index(), 1);
    assert_eq!(app.tab.selected, sel2, "タブ2の選択が保たれる");

    // Closing a tab returns to 1 tab, and the remaining tab becomes active.
    app.tab_close();
    assert_eq!(app.tab_count(), 1);
    assert_eq!(app.active_tab_index(), 0);
    // The last remaining tab cannot be closed.
    app.tab_close();
    assert_eq!(app.tab_count(), 1);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn tab_selection_is_per_tab_root_change_clears_clipboard_is_global() {
    // Core of #2/#6: the selection (BTreeSet<PathBuf>) is kept per tab without leaking, is
    // cleared on a root change, and the clipboard (Y/X/P) stays shared across the whole app.
    let dir = std::env::temp_dir().join("konoma_tab_selection_perq_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    for n in ["a.txt", "b.txt", "c.txt"] {
        std::fs::write(dir.join(n), b"x").unwrap();
    }
    std::fs::write(dir.join("sub").join("x.txt"), b"x").unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();

    let idx = |app: &App, p: std::path::PathBuf| {
        app.tab.entries.iter().position(|e| e.path == p).unwrap()
    };

    // --- (a) select multiple items in tab A ---
    app.tab.selected = idx(&app, dir.join("a.txt"));
    app.toggle_select(); // a.txt
    app.tab.selected = idx(&app, dir.join("b.txt"));
    app.toggle_select(); // b.txt
    assert!(app.is_selected(&dir.join("a.txt")) && app.is_selected(&dir.join("b.txt")));
    assert_eq!(app.op_targets().len(), 2, "A の op_targets は選択2件");

    // --- (a) to tab B → selection is empty (no leak); op_targets is only the current
    //     root's single cursor item ---
    app.tab_new().unwrap();
    assert!(
        !app.has_selection(),
        "新規タブBは選択空(旧Aの選択をリークしない)"
    );
    let bt = app.op_targets();
    assert_eq!(bt.len(), 1, "B はカーソル1件");
    assert!(
        bt[0].starts_with(&dir) && !bt.iter().any(|p| *p == dir.join("a.txt")),
        "B の op_targets は旧Aの選択(a.txt)を返さない"
    );

    // --- (a) going back to tab A restores the selection ---
    app.tab_cycle(-1);
    assert_eq!(app.active_tab_index(), 0);
    assert!(
        app.is_selected(&dir.join("a.txt")) && app.is_selected(&dir.join("b.txt")),
        "A へ戻ると選択が復元される"
    );
    assert_eq!(app.op_targets().len(), 2);

    // --- (b) a root change (l=descend) clears the selection; op_targets doesn't return
    //     paths from a different root ---
    app.tab.selected = idx(&app, dir.join("sub"));
    app.tree_descend().unwrap();
    assert_eq!(app.tab.root, dir.join("sub"), "sub が新 root");
    assert!(!app.has_selection(), "root 変更(l)で選択クリア");
    for t in app.op_targets() {
        assert!(
            t.starts_with(dir.join("sub")),
            "op_targets が別 root(旧 dir)のパスを返さない: {t:?}"
        );
    }

    // --- (b) a root change (h=leave) also clears the selection ---
    app.tab.selected = idx(&app, dir.join("sub").join("x.txt"));
    app.toggle_select();
    assert!(app.has_selection(), "sub 内で選択を積む");
    app.tree_leave().unwrap();
    assert_eq!(app.tab.root, dir, "親へ戻る");
    assert!(!app.has_selection(), "root 変更(h)で選択クリア");

    // --- (c) the clipboard is kept across tabs (global) ---
    app.tab.selected = idx(&app, dir.join("a.txt"));
    app.toggle_select();
    app.copy_selection(); // Y: loads it into the clipboard (the selection is cleared)
    assert!(
        app.clipboard_label().is_some(),
        "コピーで clipboard に積まれる"
    );
    app.tab_new().unwrap();
    assert!(
        app.clipboard_label().is_some(),
        "clipboard はタブ跨ぎで保持(global=PerTab に入れない)"
    );
    app.tab_cycle(-1);
    assert!(app.clipboard_label().is_some(), "戻っても clipboard 保持");

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn refresh_rereads_directory_listing() {
    // Core of auto-refresh: refresh() re-reads the directory, and files added externally show up.
    let dir = std::env::temp_dir().join("konoma_refresh_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.txt"), b"x").unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    let before = app.tab.entries.len();
    std::fs::write(dir.join("b.txt"), b"y").unwrap(); // add a file externally
    app.refresh().unwrap();
    assert!(
        app.tab.entries.len() > before,
        "新規ファイルが一覧に反映されない"
    );
    assert!(app.tab.entries.iter().any(|e| e.path.ends_with("b.txt")));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn refresh_prunes_deleted_paths_from_selection() {
    // #12: refresh() prunes externally-deleted paths from the selection
    // (guards against a regression where a batch trash operation failed entirely if even one
    // item was gone).
    let dir = std::env::temp_dir().join("konoma_refresh_prune_sel");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for n in ["a.txt", "b.txt", "c.txt"] {
        std::fs::write(dir.join(n), b"x").unwrap();
    }
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    // Select all 3 items.
    for n in ["a.txt", "b.txt", "c.txt"] {
        app.tab.selection.insert(dir.join(n));
    }
    assert_eq!(app.op_targets().len(), 3, "選択3件が一括対象");
    // Delete b.txt externally → refresh prunes only b.txt from the selection.
    std::fs::remove_file(dir.join("b.txt")).unwrap();
    app.refresh().unwrap();
    assert!(app.is_selected(&dir.join("a.txt")), "残存 a は選択維持");
    assert!(app.is_selected(&dir.join("c.txt")), "残存 c は選択維持");
    assert!(
        !app.is_selected(&dir.join("b.txt")),
        "消えた b は剪定される"
    );
    assert_eq!(app.op_targets().len(), 2, "一括対象は実在2件のみ");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn refresh_reloads_active_preview() {
    // Known bug: refresh() during Preview mode reloads the current preview (reflecting external
    // edits). What we observe: the decoration cache (md_cache) that decorated_lines builds gets
    // invalidated by refresh.
    let dir = std::env::temp_dir().join("konoma_refresh_reload_preview");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("doc.md"), b"# v1\n").unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    let idx = app
        .tab
        .entries
        .iter()
        .position(|e| e.path.ends_with("doc.md"))
        .unwrap();
    app.tab.selected = idx;
    app.tree_activate().unwrap(); // opens doc.md in preview
    assert!(matches!(app.tab.mode, Mode::Preview), "Preview モードへ");
    let _ = app.decorated_lines(80); // builds the decoration cache
    assert!(app.md_cache.is_some(), "プレビューでキャッシュが張られる");
    std::fs::write(dir.join("doc.md"), b"# v2 changed\n").unwrap(); // an external edit
    app.refresh().unwrap();
    assert!(
        app.md_cache.is_none(),
        "refresh で装飾キャッシュが無効化され再読込される"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn out_of_root_watch_dir_targets_files_outside_the_root() {
    // Root-cause regression: FSEvents watching recursively covers only app.tab.root. A file
    // displayed outside root (a global bookmark target / the git view for the whole repo)
    // never gets change events, so AI edits can't be detected. This verifies
    // out_of_root_watch_dir's contract: "an out-of-root preview returns its parent directory;
    // inside root / on the tree it's None".
    let base = std::env::temp_dir().join("konoma_out_of_root_watch");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("root")).unwrap();
    std::fs::create_dir_all(base.join("outside")).unwrap();
    let root = base.join("root").canonicalize().unwrap();
    let outside = base.join("outside").canonicalize().unwrap();
    std::fs::write(root.join("inside.md"), b"# inside\n").unwrap();
    std::fs::write(outside.join("note.md"), b"# outside\n").unwrap();
    let mut app = App::new(root.clone(), Config::default()).unwrap();

    // Never add extra watching while showing the tree.
    assert_eq!(
        app.out_of_root_watch_dir(),
        None,
        "ツリー表示中は追加監視しない"
    );

    // Previewing a file inside root → already covered by recursive watching → None.
    app.enter_preview(&root.join("inside.md"));
    assert_eq!(app.tab.mode, Mode::Preview);
    assert_eq!(app.out_of_root_watch_dir(), None, "root 内は追加監視不要");

    // Previewing a file outside root (simulating a bookmark target) → returns its parent
    // directory as the extra watch target.
    app.enter_preview(&outside.join("note.md"));
    assert_eq!(
        app.out_of_root_watch_dir().as_deref(),
        Some(outside.as_path()),
        "root 外プレビューはその親ディレクトリを監視対象に返す"
    );

    // Going back to the tree gives None (watching is dropped).
    app.tab.mode = Mode::Tree;
    assert_eq!(app.out_of_root_watch_dir(), None);

    std::fs::remove_dir_all(&base).ok();
}

#[test]
fn refresh_fs_reloads_preview_even_when_tree_rebuild_fails() {
    // Side fix: even if rebuild_tree fails inside refresh_fs, the preview reload is not skipped.
    // (Even if an expanded directory briefly becomes unreadable during an agent's batch
    // operation, the preview keeps following along.) What we observe: even when rebuild_tree
    // returns Err, reload_preview still runs and md_cache still gets invalidated.
    let base = std::env::temp_dir().join("konoma_refresh_tree_fail");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("root")).unwrap();
    let root = base.join("root").canonicalize().unwrap();
    std::fs::write(root.join("doc.md"), b"# v1\n").unwrap();
    let mut app = App::new(root.clone(), Config::default()).unwrap();
    let idx = app
        .tab
        .entries
        .iter()
        .position(|e| e.path.ends_with("doc.md"))
        .unwrap();
    app.tab.selected = idx;
    app.tree_activate().unwrap();
    assert!(matches!(app.tab.mode, Mode::Preview));
    let _ = app.decorated_lines(80);
    assert!(app.md_cache.is_some(), "プレビューでキャッシュが張られる");

    // Swap root for a path that doesn't exist → creates a situation where rebuild_tree
    // (read_dir) fails.
    app.tab.root = base.join("does-not-exist");
    std::fs::write(root.join("doc.md"), b"# v2 changed\n").unwrap(); // an external edit

    let result = app.refresh_fs(false);
    assert!(
        result.is_err(),
        "ツリー再構築失敗は Err で伝播する(握り潰さない)"
    );
    assert!(
        app.md_cache.is_none(),
        "rebuild_tree 失敗でもプレビュー再読込は走る(md_cache 無効化)"
    );
    std::fs::remove_dir_all(&base).ok();
}

#[test]
fn media_preview_reloads_only_when_file_changes() {
    // An image/media preview only reloads when the target file actually changes (an mtime
    // guard). What we observe: media_gen, which clear_image advances, and the reference
    // timestamp preview_media_mtime.
    // (Actual decoding requires a picker = a no-op in tests. We judge whether the reload path
    // ran by media_gen.)
    use image::RgbImage;
    let dir = std::env::temp_dir().join("konoma_media_reload_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let pic = dir.join("pic.png");
    RgbImage::new(2, 2).save(&pic).unwrap(); // a valid PNG (resolves to Image by either extension or mime)
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    let idx = app
        .tab
        .entries
        .iter()
        .position(|e| e.path.ends_with("pic.png"))
        .unwrap();
    app.tab.selected = idx;
    app.tree_activate().unwrap(); // opens in image preview
    assert!(
        matches!(
            app.tab.preview_kind,
            Some(crate::preview::PreviewKind::Image(_))
        ),
        "画像として開く"
    );
    assert!(
        app.preview_media_mtime.is_some(),
        "ロード時に基準 mtime を記録"
    );

    let gen0 = app.media_gen;
    // Unchanged → mtime matches → doesn't reload (avoids a wasted re-decode/external tool run).
    app.refresh().unwrap();
    assert_eq!(
        app.media_gen, gen0,
        "未変更なら再ロードしない(mtime ガード)"
    );

    // Simulate a file change (shift the reference mtime) → change detection triggers a reload.
    app.preview_media_mtime = None;
    app.refresh().unwrap();
    assert_ne!(
        app.media_gen, gen0,
        "変更検知で再ロード(clear_image が media_gen を進める)"
    );
    assert!(
        app.preview_media_mtime.is_some(),
        "再ロードで基準 mtime を更新"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn sort_menu_changes_order_by_key_reverse_and_dirs_first() {
    let dir = std::env::temp_dir().join("konoma_sort_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::create_dir_all(dir.join("zdir")).unwrap(); // a directory
    std::fs::write(dir.join("a.md"), vec![0u8; 30]).unwrap(); // 30B
    std::fs::write(dir.join("b.txt"), vec![0u8; 10]).unwrap(); // 10B (smallest)
    std::fs::write(dir.join("c.log"), vec![0u8; 50]).unwrap(); // 50B (largest)
    let mut app = App::new(dir.clone(), Config::default()).unwrap();

    let names = |a: &App| {
        a.tab
            .entries
            .iter()
            .map(|e| e.path.file_name().unwrap().to_string_lossy().to_string())
            .collect::<Vec<_>>()
    };

    // Default: name ascending, directories first.
    assert_eq!(names(&app), vec!["zdir", "a.md", "b.txt", "c.log"]);

    // Size ascending (directories still pinned first): files are 10<30<50 = b,a,c.
    app.open_sort_menu();
    app.sort_menu_key('s').unwrap();
    assert_eq!(names(&app), vec!["zdir", "b.txt", "a.md", "c.log"]);
    assert!(!app.is_sort_menu(), "基準選択(n/s/m/e)でメニューは閉じる");

    // Reversed (size descending): large→small = c,a,b. Toggling keeps the menu open.
    app.open_sort_menu();
    app.sort_menu_key('r').unwrap();
    assert!(app.is_sort_menu(), "トグル(r/.)はメニューを開いたまま");
    assert_eq!(names(&app), vec!["zdir", "c.log", "a.md", "b.txt"]);

    // Turn off directories-first and go back to name ascending → dirs get mixed in by name too.
    app.sort_menu_key('.').unwrap(); // dirs_first off
    app.sort_menu_key('r').unwrap(); // reverse off (back to normal)
    app.sort_menu_key('n').unwrap(); // name (closes the menu on selection)
    assert_eq!(names(&app), vec!["a.md", "b.txt", "c.log", "zdir"]);

    // Extension ascending (dirs_first off): "" < log < md < txt = zdir, c.log, a.md, b.txt.
    app.open_sort_menu();
    app.sort_menu_key('e').unwrap();
    assert_eq!(names(&app), vec!["zdir", "c.log", "a.md", "b.txt"]);

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn sort_config_sets_initial_order() {
    // [ui.sort] is reflected in the order at startup (key=size, dirs_first=false).
    let dir = std::env::temp_dir().join("konoma_sort_config_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::create_dir_all(dir.join("zdir")).unwrap();
    std::fs::write(dir.join("a.md"), vec![0u8; 30]).unwrap();
    std::fs::write(dir.join("b.txt"), vec![0u8; 10]).unwrap();
    std::fs::write(dir.join("c.log"), vec![0u8; 50]).unwrap();

    let mut cfg = Config::default();
    cfg.ui.sort.key = "size".into();
    cfg.ui.sort.dirs_first = false;
    let app = App::new(dir.clone(), cfg).unwrap();

    // The config is reflected in App.sort.
    assert_eq!(app.sort.key, SortKey::Size);
    assert!(!app.sort.dirs_first);

    // Size ascending: files follow the relative order 10<30<50 = b,a,c (a dir's size is
    // environment-dependent, so its position isn't checked).
    let names: Vec<String> = app
        .tab
        .entries
        .iter()
        .map(|e| e.path.file_name().unwrap().to_string_lossy().to_string())
        .collect();
    let pos = |n: &str| names.iter().position(|x| x == n).unwrap();
    assert!(pos("b.txt") < pos("a.md"), "size 昇順: b(10) < a(30)");
    assert!(pos("a.md") < pos("c.log"), "size 昇順: a(30) < c(50)");

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn bookmark_set_and_jump_via_marks() {
    let base = std::env::temp_dir().join("konoma_bm_app_base");
    let _ = std::fs::remove_dir_all(&base);
    let proj = std::env::temp_dir().join("konoma_bm_app_proj");
    let _ = std::fs::remove_dir_all(&proj);
    std::fs::create_dir_all(proj.join("sub")).unwrap();
    std::fs::write(proj.join("f.txt"), b"hello").unwrap();
    let mut app = App::new(proj.clone(), Config::default()).unwrap();
    // Swap in a test base so the real ~/.config isn't polluted.
    app.bookmarks = crate::bookmarks::Bookmarks::with_base(base.clone(), &app.tab.open_dir);

    let sub_idx = app
        .tab
        .entries
        .iter()
        .position(|e| e.path.ends_with("sub"))
        .unwrap();
    let file_idx = app
        .tab
        .entries
        .iter()
        .position(|e| e.path.ends_with("f.txt"))
        .unwrap();

    // Put the cursor on sub (a directory) and register at a → the cursor's dir is stored.
    app.tab.selected = sub_idx;
    app.start_mark_set();
    assert!(app.is_marking());
    app.mark_input('a');
    assert!(!app.is_marking(), "登録後はマーク待ちが解除される");
    assert_eq!(app.bookmarks.get('a'), Some(proj.join("sub")));

    // Put the cursor on f.txt (a file) and register at b → the cursor's file is stored.
    app.tab.selected = file_idx;
    app.start_mark_set();
    app.mark_input('b');
    assert_eq!(app.bookmarks.get('b'), Some(proj.join("f.txt")));

    // ' → the list opens, and the letter a: a directory → jumps there (Tree).
    app.open_bookmark_list();
    app.bookmark_jump_letter('a');
    assert!(!app.is_bookmark_list(), "ジャンプで一覧が閉じる");
    assert_eq!(
        app.tab.root,
        proj.join("sub"),
        "ディレクトリのブックマークはそこへ移動"
    );
    assert_eq!(app.tab.mode, Mode::Tree);

    // Set root back to proj, then ' b: a file → opens in preview (tree stays unchanged).
    app.tab.root = proj.clone();
    let _ = app.rebuild_tree();
    let root_before = app.tab.root.clone();
    app.open_bookmark_list();
    app.bookmark_jump_letter('b');
    assert_eq!(
        app.tab.mode,
        Mode::Preview,
        "ファイルのブックマークはプレビューで開く"
    );
    let f = proj.join("f.txt");
    assert_eq!(app.tab.preview_path.as_deref(), Some(f.as_path()));
    assert_eq!(
        app.tab.root, root_before,
        "ファイルを開いても tree(root) は変わらない"
    );

    // Pressing e while previewing makes that bookmarked file the target for the external editor.
    app.request_edit();
    assert_eq!(
        app.take_pending_edit().map(|(p, _)| p).as_deref(),
        Some(f.as_path()),
        "ブックマークのファイルが編集対象になる"
    );

    // Exiting preview returns to the original tree.
    app.back_to_tree();
    assert_eq!(app.tab.mode, Mode::Tree);
    assert_eq!(app.tab.root, root_before);

    // An unregistered mark doesn't jump and just flashes (the list stays open).
    app.open_bookmark_list();
    app.bookmark_jump_letter('z');
    assert_eq!(app.tab.root, root_before, "未登録マークではジャンプしない");
    assert!(app.flash.is_some());
    assert!(app.is_bookmark_list(), "未登録では一覧を閉じない");
    app.close_bookmark_list();

    // In the list, e: if the current selection is a file, it becomes the editor target directly
    // (without going through preview).
    app.open_bookmark_list();
    app.bookmark_list_move(1); // to a(dir) → b(file)
    app.bookmark_list_edit();
    assert!(!app.is_bookmark_list(), "編集で一覧を閉じる");
    assert_eq!(
        app.take_pending_edit().map(|(p, _)| p).as_deref(),
        Some(f.as_path()),
        "一覧の e でファイルが編集対象になる"
    );

    std::fs::remove_dir_all(&base).ok();
    std::fs::remove_dir_all(&proj).ok();
}

#[test]
fn tree_descend_sets_root_to_selected_dir() {
    let dir = std::env::temp_dir().join("konoma_descend_test");
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    std::fs::write(dir.join("sub").join("f.txt"), b"x").unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    // Select the "sub" directory's entry.
    let i = app
        .tab
        .entries
        .iter()
        .position(|e| e.is_dir && e.path.ends_with("sub"))
        .expect("sub が無い");
    app.tab.selected = i;
    // Equivalent to l: makes the selected directory the new root.
    app.tree_descend().unwrap();
    assert_eq!(
        app.tab.root,
        dir.join("sub"),
        "root が選択ディレクトリになる"
    );
    assert_eq!(app.tab.mode, Mode::Tree);
    assert!(
        app.tab.entries.iter().any(|e| e.path.ends_with("f.txt")),
        "sub の中身が表示される"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn jump_to_dir_clears_selection_on_root_change() {
    // bug #2: changing root used to leave the old root's selection lingering by path, invisibly
    // targetable for a mis-operation since the marker isn't visible. jump_to_dir (a bookmark
    // jump) must also call clear_for_root_change so it isn't carried over.
    let base = std::env::temp_dir().join("konoma_jump_clear_test");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("subB")).unwrap();
    std::fs::write(base.join("a.txt"), b"x").unwrap();
    let mut app = App::new(base.clone(), Config::default()).unwrap();
    // Select a.txt in base.
    let i = app
        .tab
        .entries
        .iter()
        .position(|e| !e.is_dir && e.path.ends_with("a.txt"))
        .expect("a.txt が無い");
    app.tab.selected = i;
    app.toggle_select();
    assert!(app.has_selection(), "前提: 選択がある");

    // Jump to subB (a root change).
    app.jump_to_dir(base.join("subB"));
    assert_eq!(app.tab.root, base.join("subB"), "root が subB になる");
    assert!(!app.has_selection(), "旧 root の選択は持ち越さない");
    assert!(
        !app.op_targets().iter().any(|p| p.ends_with("a.txt")),
        "op_targets が旧 root の a.txt を返さない"
    );
    std::fs::remove_dir_all(&base).ok();
}

#[test]
fn copy_text_builds_each_kind() {
    let open = PathBuf::from("/Users/me/work/konoma");
    let file = open.join("src").join("main.rs");
    assert_eq!(copy_text(&file, &open, CopyKind::Name), "main.rs");
    assert_eq!(
        copy_text(&file, &open, CopyKind::Full),
        "/Users/me/work/konoma/src/main.rs"
    );
    // Relative prefixes the launch directory name (base = open_dir's parent).
    assert_eq!(
        copy_text(&file, &open, CopyKind::Relative),
        "konoma/src/main.rs"
    );
    assert_eq!(
        copy_text(&file, &open, CopyKind::Parent),
        "/Users/me/work/konoma/src"
    );
    // Outside the launch directory (siblings/ancestors), it relativizes with a leading `..`,
    // just like format_path's display.
    let sibling = PathBuf::from("/Users/me/work/other/x.md");
    assert_eq!(
        copy_text(&sibling, &open, CopyKind::Relative),
        "../other/x.md",
        "兄弟ディレクトリは ../ を付ける"
    );
    let parent = PathBuf::from("/Users/me/work");
    assert_eq!(
        copy_text(&parent, &open, CopyKind::Relative),
        "..",
        "上位は .."
    );
    // A far-off absolute path also relativizes (as with the display) by chaining `..` from open_dir.
    let outside = PathBuf::from("/etc/hosts");
    assert_eq!(
        copy_text(&outside, &open, CopyKind::Relative),
        "../../../../etc/hosts"
    );
}

#[test]
fn copy_relative_matches_title_display() {
    // The copy string for `yr`/`cr` matches exactly the top-left title display
    // (format_path's Relative).
    let work = std::env::temp_dir().join("konoma_copy_match_title");
    let _ = std::fs::remove_dir_all(&work);
    let a = work.join("A");
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(work.join("B")).unwrap();
    let mut app = App::new(a.clone(), Config::default()).unwrap();
    app.path_style = PathStyle::Relative;
    for p in [
        a.join("x.rs"),                // under it
        work.join("B").join("aaa.md"), // a sibling
        work.clone(),                  // an ancestor
    ] {
        assert_eq!(
            copy_text(&p, &app.tab.open_dir, CopyKind::Relative),
            app.format_path(&p),
            "コピーとタイトル表示がずれている: {}",
            p.display()
        );
    }
    std::fs::remove_dir_all(&work).ok();
}

#[test]
fn is_image_preview_requires_src_and_kind() {
    let app = app_with_image();
    assert!(app.is_image_preview());
    let mut app2 = app_with_image();
    app2.image_src = None;
    assert!(!app2.is_image_preview());
}

#[test]
fn help_in_image_mode_shows_only_image_section() {
    // ? during an image preview shows only the image section. Tree/Git/text-only sections
    // are not shown (per-mode optimization).
    let mut app = app_with_image();
    app.tab.mode = Mode::Preview;
    let s: String = crate::ui::help::help_lines(&app)
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|sp| sp.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(s.contains("Preview: image") && s.contains("zoom"), "画像節");
    assert!(
        !s.contains("Git status (row markers)") && !s.contains("horizontal scroll"),
        "Tree/テキスト専用節は出さない"
    );
    assert!(s.contains("Tabs"), "画像中も共通タブ節");
}

#[test]
fn status_shows_image_zoom_factor() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    let mut app = app_with_image();
    app.tab.mode = Mode::Preview;
    let text = |app: &App| -> String {
        let mut term = Terminal::new(TestBackend::new(80, 1)).unwrap();
        term.draw(|f| crate::ui::status::render_combined(f, app, f.area()))
            .unwrap();
        term.backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect()
    };
    app.tab.image_zoom = 1.0;
    let s = text(&app);
    assert!(s.contains("IMAGE"), "IMAGE チップ: {s}");
    assert!(s.contains("x1.00"), "フィットで x1.00: {s}");
    app.tab.image_zoom = 2.5;
    assert!(text(&app).contains("x2.50"), "拡大時に倍率");
}

/// Whether windowed_lines contains an Rgb foreground (syntect coloring).
fn has_rgb_fg(lines: &[Line<'static>]) -> bool {
    use ratatui::style::Color;
    lines
        .iter()
        .flat_map(|l| l.spans.iter())
        .any(|s| matches!(s.style.fg, Some(Color::Rgb(_, _, _))))
}

fn open_code_file(dir: &std::path::Path, name: &str, cfg: Config) -> App {
    std::fs::write(dir.join(name), b"fn main() {\n    let x = 1;\n}\n").unwrap();
    let mut app = App::new(dir.canonicalize().unwrap(), cfg).unwrap();
    app.tab.selected = app
        .tab
        .entries
        .iter()
        .position(|e| e.path.ends_with(name))
        .unwrap();
    app.tree_activate().unwrap();
    app.tab.preview_viewport = 5;
    app
}

#[test]
fn syntax_highlight_off_is_plain_and_not_pending() {
    let dir = std::env::temp_dir().join("konoma_hl_off_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut cfg = Config::default();
    cfg.ui.syntax_highlight = false;
    let mut app = open_code_file(&dir, "a.rs", cfg);
    // When disabled, it never enters a heavy pending state, and the body is plain text (no Rgb coloring).
    assert!(
        !app.is_highlight_pending(),
        "off はローディング待ちにしない"
    );
    let lines = app.windowed_lines(5, 80);
    assert!(!has_rgb_fg(&lines), "off は素テキスト(着色なし)");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn warm_grammar_opens_without_pending_and_colored() {
    // Warming the rs grammar beforehand means the Code preview colors immediately with no pending.
    let _ =
        crate::preview::code::highlight("fn x(){}\n", std::path::Path::new("warm.rs"), "TwoDark");
    let dir = std::env::temp_dir().join("konoma_hl_warm_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = open_code_file(&dir, "b.rs", Config::default());
    assert!(
        !app.is_highlight_pending(),
        "温まった言語は pending 無し(即時着色)"
    );
    let lines = app.windowed_lines(5, 80);
    assert!(has_rgb_fg(&lines), "温まった Code は着色される");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn loading_mode_follows_config() {
    let dir = std::env::temp_dir().join("konoma_loading_mode_test");
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    assert!(app.loading_is_indicator(), "既定は indicator");
    app.cfg.ui.preview_loading = "progressive".into();
    assert!(!app.loading_is_indicator(), "progressive 指定で素表示");
}

#[test]
fn request_edit_targets_selected_file_or_warns_on_dir() {
    let dir = std::env::temp_dir().join("konoma_edit_target_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    std::fs::write(dir.join("a.rs"), b"x\n").unwrap();
    let mut app = App::new(dir.canonicalize().unwrap(), Config::default()).unwrap();

    // Select a file → its path becomes the edit request.
    app.tab.selected = app
        .tab
        .entries
        .iter()
        .position(|e| e.path.ends_with("a.rs"))
        .unwrap();
    app.request_edit();
    let got = app.take_pending_edit();
    assert!(
        got.as_ref()
            .map(|(p, _)| p.ends_with("a.rs"))
            .unwrap_or(false),
        "選択ファイルが編集対象: {got:?}"
    );
    assert!(
        app.take_pending_edit().is_none(),
        "取り出したらクリアされる"
    );

    // Select a directory → no request is raised, and it's rejected via flash.
    app.tab.selected = app.tab.entries.iter().position(|e| e.is_dir).unwrap();
    app.flash = None;
    app.request_edit();
    assert!(
        app.take_pending_edit().is_none(),
        "ディレクトリは編集対象外"
    );
    assert!(app.flash.is_some(), "拒否メッセージを出す");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn request_edit_targets_preview_file() {
    let dir = std::env::temp_dir().join("konoma_edit_preview_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("b.rs"), b"fn main() {}\n").unwrap();
    let mut app = App::new(dir.canonicalize().unwrap(), Config::default()).unwrap();
    app.tab.selected = app
        .tab
        .entries
        .iter()
        .position(|e| e.path.ends_with("b.rs"))
        .unwrap();
    app.tree_activate().unwrap(); // to Preview
    assert!(matches!(app.tab.mode, Mode::Preview));
    app.request_edit();
    let got = app.take_pending_edit();
    assert!(
        got.as_ref()
            .map(|(p, _)| p.ends_with("b.rs"))
            .unwrap_or(false),
        "プレビュー中ファイルが編集対象: {got:?}"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn spinner_cycles_and_has_no_emoji() {
    let dir = std::env::temp_dir().join("konoma_spinner_test");
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    // Advancing the frame gives a different glyph, and it cycles back after 10 frames (= spins).
    let g0 = app.spinner_glyph(); // frame 0
    app.tick_spinner();
    assert_ne!(g0, app.spinner_glyph(), "ティックでコマが進む(回る)");
    for _ in 0..9 {
        app.tick_spinner(); // 10 ticks total → frame 10 ≡ 0
    }
    assert_eq!(app.spinner_glyph(), g0, "10 コマで一巡して先頭へ");
    // Braille patterns (U+2800-U+28FF), not emoji = single-width and colorless in the terminal.
    for _ in 0..10 {
        let ch = app.spinner_glyph().chars().next().unwrap();
        assert!(
            ('\u{2800}'..='\u{28FF}').contains(&ch),
            "スピナーは点字(非絵文字): {ch:?}"
        );
        app.tick_spinner();
    }
}

#[test]
fn progressive_pending_renders_plain_text() {
    let dir = std::env::temp_dir().join("konoma_prog_plain_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut cfg = Config::default();
    cfg.ui.preview_loading = "progressive".into();
    let mut app = open_code_file(&dir, "c.rs", cfg);
    app.hl_pending = true; // force the progressive warm-up-pending state
    let lines = app.windowed_lines(5, 80);
    assert!(
        !has_rgb_fg(&lines),
        "progressive 待ち中は素テキスト(着色は完了後に差し替え)"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn preview_search_finds_highlights_and_navigates() {
    use std::io::Write;
    let dir = std::env::temp_dir().join("konoma_preview_search_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("a.rs");
    {
        let mut f = std::fs::File::create(&p).unwrap();
        // "needle" every 7 lines (0,7,14,21,28,35). 40 lines total.
        for i in 0..40 {
            let tail = if i % 7 == 0 { "needle here" } else { "x" };
            writeln!(f, "line {i} {tail}").unwrap();
        }
    }
    let mut app = App::new(dir.canonicalize().unwrap(), Config::default()).unwrap();
    app.tab.selected = app.tab.entries.iter().position(|e| !e.is_dir).unwrap();
    app.tree_activate().unwrap();
    assert!(app.is_windowed(), "コードはウィンドウ読み対象");

    // `/` → input mode. Push characters and commit.
    app.start_search();
    assert!(app.is_searching(), "/ で検索入力モード");
    for c in "needle".chars() {
        app.search_input_push(c);
    }
    assert_eq!(app.search_input(), Some("needle"));
    app.search_commit();
    assert!(!app.is_searching(), "確定で入力モード解除");

    // 6 hits; jumps to the first one (line0); status 1/6.
    assert_eq!(app.tab.search_matches.len(), 6, "7行ごとに6件");
    assert_eq!(app.search_status(), Some((1, 6)));
    assert_eq!(app.tab.preview_top_line, 0, "最初の一致(行0)を先頭に");
    assert_eq!(app.preview_search_query(), Some("needle"));

    // n to the next (line7), again to line14.
    app.search_next(1);
    assert_eq!(app.search_status(), Some((2, 6)));
    assert_eq!(app.tab.preview_top_line, 7);
    app.search_next(1);
    assert_eq!(app.tab.preview_top_line, 14);
    // N to go back (line7).
    app.search_next(-1);
    assert_eq!(app.tab.preview_top_line, 7);
    // Cycling with N from the end (idx0) → the last match (line35).
    app.tab.search_idx = 0;
    app.search_next(-1);
    assert_eq!(app.search_status(), Some((6, 6)));
    assert_eq!(app.tab.preview_top_line, 35);

    // Equivalent to Esc: clearing removes the query and matches.
    app.search_clear();
    assert!(app.preview_search_query().is_none());
    assert_eq!(app.search_status(), None);
    assert!(app.tab.search_matches.is_empty());

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn highlight_query_in_line_marks_only_matches_yellow() {
    use ratatui::style::Color;
    let line = Line::from(vec![Span::from("foo "), Span::from("Needle bar needle")]);
    let out = highlight_query_in_line(line, "needle", None);
    // Concatenating the fragments with a yellow background = the matched text (case-insensitive,
    // 2 occurrences).
    let hit: String = out
        .spans
        .iter()
        .filter(|s| s.style.bg == Some(Color::Yellow))
        .map(|s| s.content.as_ref())
        .collect();
    assert_eq!(
        hit, "Needleneedle",
        "一致部分だけ黄色強調(原文の大小は保持)"
    );
    // The full text is unchanged.
    let full: String = out.spans.iter().map(|s| s.content.as_ref()).collect();
    assert_eq!(full, "foo Needle bar needle");
}

#[test]
fn highlight_query_in_line_handles_multibyte_lines() {
    use ratatui::style::Color;
    // Lowercasing Japanese text doesn't change its byte length, so an ASCII query still
    // highlights correctly (byte boundaries stay intact and it doesn't panic). The full text
    // is unchanged.
    let line = Line::from("日本語 needle テスト");
    let out = highlight_query_in_line(line, "needle", None);
    let hit: String = out
        .spans
        .iter()
        .filter(|s| s.style.bg == Some(Color::Yellow))
        .map(|s| s.content.as_ref())
        .collect();
    assert_eq!(hit, "needle", "マルチバイト行内の ASCII 一致を黄色強調");
    let full: String = out.spans.iter().map(|s| s.content.as_ref()).collect();
    assert_eq!(full, "日本語 needle テスト", "全文は不変");
}

#[test]
fn highlight_query_in_line_current_match_is_orange() {
    use ratatui::style::Color;
    let orange = Color::Rgb(0xff, 0x8c, 0x00);
    let line = Line::from("let needle = 1;"); // "needle" is the 0th occurrence in the line
                                              // Occurrence index Some(0) is orange, None is yellow (color distinguishes the current occurrence).
    let cur = highlight_query_in_line(line.clone(), "needle", Some(0));
    let other = highlight_query_in_line(line, "needle", None);
    assert!(
        cur.spans.iter().any(|s| s.style.bg == Some(orange)),
        "現在の出現はオレンジ背景"
    );
    assert!(
        !cur.spans.iter().any(|s| s.style.bg == Some(Color::Yellow)),
        "現在の出現に黄色は使わない"
    );
    assert!(
        other
            .spans
            .iter()
            .any(|s| s.style.bg == Some(Color::Yellow))
            && !other.spans.iter().any(|s| s.style.bg == Some(orange)),
        "非選択の出現は黄色のみ"
    );
}

#[test]
fn highlight_query_only_current_occurrence_on_same_line_is_orange() {
    use ratatui::style::Color;
    let orange = Color::Rgb(0xff, 0x8c, 0x00);
    // Two "todo"s on the same line (the todo in todos, and Todo). Occurrence index Some(1) =
    // only the second one is orange.
    let line = Line::from("const todos: Todo[] = [");
    let out = highlight_query_in_line(line, "todo", Some(1));
    let orange_txt: String = out
        .spans
        .iter()
        .filter(|s| s.style.bg == Some(orange))
        .map(|s| s.content.as_ref())
        .collect();
    let yellow_txt: String = out
        .spans
        .iter()
        .filter(|s| s.style.bg == Some(Color::Yellow))
        .map(|s| s.content.as_ref())
        .collect();
    assert_eq!(orange_txt, "Todo", "現在の出現(2つ目)だけオレンジ");
    assert_eq!(yellow_txt, "todo", "同一行の他の出現(1つ目)は黄色");
}

#[test]
fn highlight_query_current_match_survives_tab_expansion() {
    use ratatui::style::Color;
    let orange = Color::Rgb(0xff, 0x8c, 0x00);
    // #14: even for a line after tab expansion (where the leading "→" plus spaces shift the
    // byte sequence away from the original text), the current match is identified by
    // occurrence index rather than column (byte position), so the orange lands on the correct
    // occurrence. A leading tab sits before "ab"'s two "needle"s, putting them at a different
    // byte position than the original col.
    let expanded = crate::preview::code::expand_tabs(vec![Line::from("\tneedle ab needle")], 4);
    let line = expanded.into_iter().next().unwrap();
    // Occurrence index Some(1) = only the second "needle" is orange, the first is yellow.
    let out = highlight_query_in_line(line, "needle", Some(1));
    // The orange fragment is "exactly one needle's worth" = it lands only on the current match.
    let orange_txt: String = out
        .spans
        .iter()
        .filter(|s| s.style.bg == Some(orange))
        .map(|s| s.content.as_ref())
        .collect();
    let yellow_txt: String = out
        .spans
        .iter()
        .filter(|s| s.style.bg == Some(Color::Yellow))
        .map(|s| s.content.as_ref())
        .collect();
    assert_eq!(orange_txt, "needle", "タブ展開後も現在一致(2つ目)だけ橙");
    assert_eq!(yellow_txt, "needle", "他の出現(1つ目)は黄色");
}

// --- Git view (the changes hub) -------------------------------------------------
#[cfg(feature = "git")]
fn init_git_repo(dir: &Path) {
    let repo = git2::Repository::init(dir).unwrap();
    let mut cfg = repo.config().unwrap();
    cfg.set_str("user.name", "Test").unwrap();
    cfg.set_str("user.email", "test@example.com").unwrap();
    cfg.set_str("commit.gpgsign", "false").ok();
}

// --- Follow baseline diff (anchored at the moment F is pressed; shows only changes since) ------------------
#[cfg(feature = "git")]
fn added_texts(lines: &[crate::git::DiffLine]) -> Vec<String> {
    lines
        .iter()
        .filter(|l| l.kind == crate::git::DiffLineKind::Added)
        .map(|l| l.text.clone())
        .collect()
}

/// Core regression: even when there are uncommitted changes from before F, the follow diff shows
/// only "changes since F". Pre-F changes are folded into the baseline and stay invisible, but
/// reappear once toggled to full diff via `f`.
#[cfg(feature = "git")]
#[test]
fn follow_baseline_hides_pre_follow_changes_and_toggles_to_full() {
    use std::process::Command;
    let dir = std::env::temp_dir().join("konoma_follow_baseline_core");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    init_git_repo(&dir);
    let git = |args: &[&str]| {
        Command::new("git")
            .current_dir(&dir)
            .args(args)
            .output()
            .unwrap();
    };
    let file = dir.join("app.txt");
    std::fs::write(&file, "line1\nline2\nline3\n").unwrap();
    git(&["add", "-A"]);
    git(&["commit", "-q", "-m", "init"]);
    // An uncommitted change that existed before F (rewrites line2).
    std::fs::write(&file, "line1\nPRE-FOLLOW\nline3\n").unwrap();

    let mut app = App::new(dir.canonicalize().unwrap(), Config::default()).unwrap();
    app.toggle_follow(); // F on → pins the content at this instant (including PRE-FOLLOW) as the baseline
    assert!(app.follow_enabled(), "F で追尾 ON");

    // An edit after F (rewrites line3).
    let fc = file.canonicalize().unwrap();
    std::fs::write(&fc, "line1\nPRE-FOLLOW\nPOST-FOLLOW\n").unwrap();
    app.follow_jump(&fc);
    assert!(app.is_git_diff_preview(), "追跡変更 → 全画面 diff");

    let base = app.git_diff_lines();
    let added = added_texts(&base);
    assert!(
        added.iter().any(|t| t == "POST-FOLLOW"),
        "F 後の変更は出る: {added:?}"
    );
    assert!(
        !added.iter().any(|t| t == "PRE-FOLLOW"),
        "F 前の変更はベースラインに畳まれて出ない: {added:?}"
    );
    assert_eq!(
        app.follow_diff_scope_msg(),
        Some(crate::i18n::Msg::FollowShowSince),
        "既定はフォロー開始以降"
    );

    // `f` toggle → full diff (against HEAD). Pre-F changes appear too, and the label switches.
    app.toggle_follow_diff_scope();
    assert_eq!(
        app.follow_diff_scope_msg(),
        Some(crate::i18n::Msg::FollowShowFull),
        "トグルでフル差分ラベル"
    );
    let full = added_texts(&app.git_diff_lines());
    assert!(
        full.iter().any(|t| t == "PRE-FOLLOW") && full.iter().any(|t| t == "POST-FOLLOW"),
        "フル diff は F 前後どちらの変更も出す: {full:?}"
    );
}

/// For a file that was clean at F time, a later edit shows "since F" using the HEAD blob as the baseline.
#[cfg(feature = "git")]
#[test]
fn follow_baseline_clean_file_diffs_against_head() {
    use std::process::Command;
    let dir = std::env::temp_dir().join("konoma_follow_baseline_clean");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    init_git_repo(&dir);
    let git = |args: &[&str]| {
        Command::new("git")
            .current_dir(&dir)
            .args(args)
            .output()
            .unwrap();
    };
    let file = dir.join("clean.txt");
    std::fs::write(&file, "one\ntwo\nthree\n").unwrap();
    git(&["add", "-A"]);
    git(&["commit", "-q", "-m", "init"]);

    let mut app = App::new(dir.canonicalize().unwrap(), Config::default()).unwrap();
    app.toggle_follow(); // clean.txt is clean → doesn't get into the dirty map (uses the HEAD blob)
    let fc = file.canonicalize().unwrap();
    std::fs::write(&fc, "one\nTWO-EDITED\nthree\n").unwrap();
    app.follow_jump(&fc);
    let added = added_texts(&app.git_diff_lines());
    assert!(
        added.iter().any(|t| t == "TWO-EDITED"),
        "clean だったファイルの F 後編集が出る: {added:?}"
    );
}

/// A dirty file over the size cap isn't snapshotted (None), and the follow baseline diff
/// returns None = the caller falls back to a full diff (principle #3, avoiding a memory blowup).
#[cfg(feature = "git")]
#[test]
fn follow_baseline_oversized_dirty_file_falls_back_to_full() {
    use std::process::Command;
    let dir = std::env::temp_dir().join("konoma_follow_baseline_big");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    init_git_repo(&dir);
    let git = |args: &[&str]| {
        Command::new("git")
            .current_dir(&dir)
            .args(args)
            .output()
            .unwrap();
    };
    let file = dir.join("big.txt");
    std::fs::write(&file, "seed\n").unwrap();
    git(&["add", "-A"]);
    git(&["commit", "-q", "-m", "init"]);
    // Bloated to >5MB before F (= dirty and over the cap).
    let big = "x".repeat(6 * 1024 * 1024);
    std::fs::write(&file, format!("seed\n{big}\n")).unwrap();

    let mut app = App::new(dir.canonicalize().unwrap(), Config::default()).unwrap();
    app.toggle_follow(); // big.txt is over the cap → recorded as None in the dirty map
    let fc = file.canonicalize().unwrap();
    // The follow baseline diff is None (= falls back to full diff).
    assert!(
        app.follow_baseline_diff(&fc).is_none(),
        "上限超の dirty ファイルはベースライン差分を持たない(フル diff へ降格)"
    );
}

/// Following in a non-git directory → an edited file falls back to a file preview instead of
/// a full-screen diff (there's no HEAD for the baseline = don't show an all-added wall, per
/// follow_jump's contract). The old implementation used an empty baseline → all lines Added,
/// mistakenly opening a diff (regression guard).
#[cfg(feature = "git")]
#[test]
fn follow_in_non_repo_falls_back_to_file_preview() {
    let dir = std::env::temp_dir().join("konoma_follow_non_repo");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap(); // don't create .git = a non-repo
    let file = dir.join("note.txt");
    std::fs::write(&file, "alpha\nbeta\ngamma\n").unwrap();

    let mut app = App::new(dir.canonicalize().unwrap(), Config::default()).unwrap();
    app.toggle_follow(); // head=None, an empty-dirty baseline
    let fc = file.canonicalize().unwrap();
    std::fs::write(&fc, "alpha\nBETA-EDIT\ngamma\n").unwrap();
    app.follow_jump(&fc);
    assert!(
        !app.is_git_diff_preview(),
        "非 repo は全画面 diff でなくファイルプレビューへ降格"
    );
    assert!(matches!(app.tab.mode, Mode::Preview));
}

/// Re-pressing `F` (OFF→ON) recaptures the baseline and drops the stale cache of the follow diff
/// being shown (drops the old diff that would otherwise linger by path match — symmetric with
/// `toggle_follow_diff_scope`).
#[cfg(feature = "git")]
#[test]
fn re_following_recaptures_baseline_and_drops_stale_diff() {
    use std::process::Command;
    let dir = std::env::temp_dir().join("konoma_follow_recapture");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    init_git_repo(&dir);
    let git = |args: &[&str]| {
        Command::new("git")
            .current_dir(&dir)
            .args(args)
            .output()
            .unwrap();
    };
    let file = dir.join("x.txt");
    std::fs::write(&file, "a\nb\nc\n").unwrap();
    git(&["add", "-A"]);
    git(&["commit", "-q", "-m", "init"]);

    let mut app = App::new(dir.canonicalize().unwrap(), Config::default()).unwrap();
    app.toggle_follow(); // F1
    let fc = file.canonicalize().unwrap();
    std::fs::write(&fc, "a\nEDIT1\nc\n").unwrap();
    app.refresh().unwrap(); // equivalent to an FS event: refreshes git_status after the edit (shows x.txt as dirty)
    app.follow_jump(&fc);
    assert!(app.is_git_diff_preview());
    assert!(
        added_texts(&app.git_diff_lines())
            .iter()
            .any(|t| t == "EDIT1"),
        "F1 以降の EDIT1 が出る"
    );

    // Re-press F (OFF→ON). The current content (including EDIT1) becomes the new baseline →
    // EDIT1 is no longer a "change since".
    app.toggle_follow(); // OFF
    app.toggle_follow(); // ON (re-baselines, drops diff_cache)
    let lines = app.git_diff_lines();
    assert!(
        lines.is_empty(),
        "再 F でベースライン取り直し → 旧 diff(EDIT1)が残らない: {:?}",
        added_texts(&lines)
    );
}

#[cfg(feature = "git")]
#[test]
fn graph_config_base_branches_drive_base_order_and_picker_reorder() {
    use std::process::Command;
    let dir = std::env::temp_dir().join("konoma_graph_config_base");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    init_git_repo(&dir);
    let git = |args: &[&str]| {
        Command::new("git")
            .current_dir(&dir)
            .args(args)
            .output()
            .unwrap();
    };
    std::fs::write(dir.join("a.txt"), b"a\n").unwrap();
    git(&["add", "-A"]);
    git(&["commit", "-q", "-m", "init"]);
    git(&["branch", "-M", "trunk"]); // HEAD=trunk
    for n in ["main", "develop", "feat"] {
        git(&["checkout", "-q", "trunk"]);
        git(&["checkout", "-q", "-b", n]);
        std::fs::write(dir.join(format!("{n}.txt")), b"x\n").unwrap();
        git(&["add", "-A"]);
        git(&["commit", "-q", "-m", &format!("{n} c")]);
    }
    git(&["checkout", "-q", "trunk"]); // HEAD=trunk

    let mut cfg = Config::default();
    cfg.ui.graph_base_branches = vec!["develop".into(), "main".into()];
    let mut app = App::new(dir.canonicalize().unwrap(), cfg).unwrap();
    app.open_git_graph();

    // (2) Base = develop, the first one in the array (left to right) that exists.
    assert_eq!(
        app.git_graph_base_label(),
        Some("develop"),
        "config 配列の最初の存在ブランチが基準"
    );
    // (4) Ordering: array order (develop→main) → HEAD (trunk) → most-recent order.
    let names: Vec<String> = app
        .git_graph_legend()
        .iter()
        .map(|e| e.name.clone())
        .collect();
    let pos = |n: &str| names.iter().position(|x| x == n);
    assert_eq!(names.first().map(|s| s.as_str()), Some("develop"));
    assert!(
        pos("develop") < pos("main"),
        "配列順 develop→main: {names:?}"
    );
    assert!(pos("main") < pos("trunk"), "配列が HEAD より先: {names:?}");

    // (5) Reordering in the panel → the base is re-derived from the new first entry.
    app.git_graph_open_picker();
    // Panel order = [develop, main, trunk, feat]. Move main (index 1) up by one.
    app.git_graph_picker_sel = 1;
    app.git_graph_picker_reorder(-1); // main to the front
    app.git_graph_picker_apply();
    assert_eq!(
        app.git_graph_base_label(),
        Some("main"),
        "並び替えで先頭になった main が基準に再導出される"
    );

    std::fs::remove_dir_all(&dir).ok();
}

// Phase G: the heavy ignored (ignore set) is cached per repo (workdir), and descending with `l`
// into a subdirectory of the same repo doesn't recompute it (moving to a different repo does
// recompute). We plant a sentinel and observe whether it was recomputed (= whether the
// sentinel disappears).
#[cfg(feature = "git")]
#[test]
fn descend_into_same_repo_subdir_reuses_ignored_set() {
    let dir = std::env::temp_dir().join("konoma_ignored_reuse_same");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    init_git_repo(&dir);
    std::fs::write(dir.join(".gitignore"), b"ignored.txt\n").unwrap();
    std::fs::write(dir.join("ignored.txt"), b"x").unwrap();

    let mut app = App::new(dir.canonicalize().unwrap(), Config::default()).unwrap();
    app.refresh_git_if_needed();
    let wd = app.git_ignored_for.clone();
    assert!(
        wd.is_some(),
        "ignored は repo workdir をキーに計算済みのはず"
    );

    // Sentinel: survives if descending within the same repo doesn't trigger a recompute.
    let sentinel = dir.join("__sentinel_marker__");
    app.git_ignored.insert(sentinel.clone());

    // Select sub and descend (root change, same repo).
    let i = app
        .tab
        .entries
        .iter()
        .position(|e| e.is_dir && e.path.ends_with("sub"))
        .expect("sub エントリが無い");
    app.tab.selected = i;
    app.tree_descend().unwrap();
    assert!(app.tab.root.ends_with("sub"), "root が sub になった");
    app.refresh_git_if_needed();

    assert_eq!(
        app.git_ignored_for, wd,
        "同一 repo: ignored のキャッシュキー(workdir)は不変"
    );
    assert!(
        app.git_ignored.contains(&sentinel),
        "同一 repo 内では ignored を再計算せず流用する(番兵が生存)"
    );
    assert_eq!(
        app.git_status_for.as_deref(),
        Some(app.tab.root.as_path()),
        "statuses は root 変更のたびに取り直す"
    );
    std::fs::remove_dir_all(&dir).ok();
}

// Fix A (2026-07-13): even if an external agent commits via git during a preview and its fs
// event is missed (FSEvents inherently misses events), going back to the tree re-verifies git
// status so the change marker doesn't go stale. back_to_tree() invalidates git_status_for →
// the next render-equivalent refresh_git_if_needed re-fetches the statuses.
#[cfg(feature = "git")]
#[test]
fn returning_to_tree_re_syncs_stale_git_status() {
    use std::process::Command;
    let dir = std::env::temp_dir().join("konoma_back_to_tree_resync");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    init_git_repo(&dir);
    let git = |args: &[&str]| {
        Command::new("git")
            .current_dir(&dir)
            .args(args)
            .output()
            .unwrap();
    };
    std::fs::write(dir.join("note.md"), b"v1\n").unwrap();
    git(&["add", "-A"]);
    git(&["commit", "-q", "-m", "init"]);
    // Change it → shows M in the tree.
    std::fs::write(dir.join("note.md"), b"v2\n").unwrap();

    let mut app = App::new(dir.canonicalize().unwrap(), Config::default()).unwrap();
    app.refresh_git_if_needed(); // git_status_for=root, note.md=M
    let note = dir.canonicalize().unwrap().join("note.md");
    assert!(
        app.git_status_of(&note).is_some(),
        "変更ファイルは M として見える"
    );

    // Enter preview.
    app.tab.mode = Mode::Preview;
    app.tab.preview_path = Some(note.clone());

    // Commit externally (becomes clean). We don't run konoma's refresh = reproducing a missed fs event.
    git(&["commit", "-qam", "external commit"]);
    assert!(
        app.git_status_of(&note).is_some(),
        "取りこぼし再現: refresh 前は git_status がまだ古い(M のまま)"
    );

    // Go back to the tree → git_status_for is invalidated.
    app.back_to_tree();
    assert!(
        app.git_status_for.is_none(),
        "back_to_tree が git status を無効化(次の描画で再取得)"
    );
    // The render-equivalent step.
    app.refresh_git_if_needed();
    assert!(
        app.git_status_of(&note).is_none(),
        "ツリー復帰で再検証 → コミット済みなので M が消える(陳腐化解消)"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[cfg(feature = "git")]
#[test]
fn descend_into_nested_different_repo_recomputes_ignored_set() {
    let dir = std::env::temp_dir().join("konoma_ignored_reuse_diff");
    let _ = std::fs::remove_dir_all(&dir);
    // Nest an independent repoB (inner) inside outer repoA.
    std::fs::create_dir_all(dir.join("inner")).unwrap();
    init_git_repo(&dir);
    init_git_repo(&dir.join("inner"));
    std::fs::write(dir.join(".gitignore"), b"ignored.txt\n").unwrap();

    let mut app = App::new(dir.canonicalize().unwrap(), Config::default()).unwrap();
    app.refresh_git_if_needed();
    let wd_a = app.git_ignored_for.clone();
    assert!(wd_a.is_some());

    let sentinel = dir.join("__sentinel_marker__");
    app.git_ignored.insert(sentinel.clone());

    // Descend into inner (a different repo).
    let i = app
        .tab
        .entries
        .iter()
        .position(|e| e.is_dir && e.path.ends_with("inner"))
        .expect("inner エントリが無い");
    app.tab.selected = i;
    app.tree_descend().unwrap();
    app.refresh_git_if_needed();

    assert_ne!(
        app.git_ignored_for, wd_a,
        "別 repo: workdir が変わったのでキャッシュキーも変わる"
    );
    assert!(
        !app.git_ignored.contains(&sentinel),
        "別 repo へ移ると ignored を作り直す(番兵が消える)"
    );
    std::fs::remove_dir_all(&dir).ok();
}

// fix③: the heavy ignored computation runs on another thread → applied via `apply_ignored`.
// Staleness is judged by generation (gen), and a result whose generation has advanced (moved to
// a different repo mid-computation) is discarded. Pure state logic that needs no git, so it
// runs under both features.
#[test]
fn busy_indicator_reflects_background_jobs() {
    // busy_jobs is **derived** from each job's existing state (structurally prevents the
    // accident of a spinner getting stuck from a missed begin/end pairing). Idle = empty,
    // config off = hidden, multiple jobs = "+n" notation.
    let dir = std::env::temp_dir().join("konoma_busy_indicator_test");
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.lang = crate::i18n::Lang::En;
    assert!(app.busy_jobs().is_empty(), "アイドルではジョブ無し");
    assert!(!app.busy_indicator_active());

    // Media loading in progress → a spinner + label in the top-right context.
    app.media_loading = true;
    assert!(app.busy_jobs().contains(&crate::i18n::Msg::BusyMedia));
    assert!(app.busy_indicator_active());
    let joined: String = crate::ui::status::context_spans(&app)
        .iter()
        .map(|s| s.content.as_ref())
        .collect();
    assert!(joined.contains("loading media"), "{joined}");

    // Multiple jobs show the first label plus an "+n" notation.
    app.md_remote_inflight
        .insert("https://example.com/a.png".into());
    let joined: String = crate::ui::status::context_spans(&app)
        .iter()
        .map(|s| s.content.as_ref())
        .collect();
    assert!(joined.contains("+1"), "複数ジョブは +n: {joined}");

    // With config off, both the display and the tick are disabled (individual center-screen
    // displays remain a separate system).
    app.cfg.ui.busy_indicator = false;
    assert!(!app.busy_indicator_active());
    let joined: String = crate::ui::status::context_spans(&app)
        .iter()
        .map(|s| s.content.as_ref())
        .collect();
    assert!(!joined.contains("loading media"), "{joined}");

    // Once every job ends, it goes back to empty (= the tick stops and idle load returns to zero).
    app.cfg.ui.busy_indicator = true;
    app.media_loading = false;
    app.md_remote_inflight.clear();
    assert!(app.busy_jobs().is_empty());
    assert!(!app.busy_indicator_active());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn busy_indicator_tracks_ignored_scan() {
    // git ignored scan: shows GitScan while pending is set, and clears once the current
    // generation is applied.
    let dir = std::env::temp_dir().join("konoma_busy_gitscan_test");
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.git_ignored_gen = 7;
    app.git_ignored_pending = Some(dir.clone());
    assert!(app.busy_jobs().contains(&crate::i18n::Msg::BusyGitScan));
    let cur = IgnoredResult {
        gen: 7,
        workdir: dir.clone(),
        set: Default::default(),
    };
    assert!(app.apply_ignored(cur));
    assert!(
        !app.busy_jobs().contains(&crate::i18n::Msg::BusyGitScan),
        "適用でスキャン中表示が消える"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn apply_ignored_reflects_current_gen_and_discards_stale() {
    let dir = std::env::temp_dir().join("konoma_apply_ignored_gen");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.git_ignored_gen = 5;
    app.git_ignored_pending = Some(dir.clone());

    // A result from an old generation (4): not applied, and pending is not cleared either.
    let stale = IgnoredResult {
        gen: 4,
        workdir: dir.clone(),
        set: [dir.join("x")].into_iter().collect(),
    };
    assert!(!app.apply_ignored(stale), "stale 世代は反映しない");
    assert!(app.git_ignored.is_empty(), "stale は無視セットを変えない");
    assert!(
        app.git_ignored_pending.is_some(),
        "stale では pending を残す(現行計算待ちのまま)"
    );

    // A result from the current generation (5): applies it, sets git_ignored_for, and clears pending.
    let cur = IgnoredResult {
        gen: 5,
        workdir: dir.clone(),
        set: [dir.join("y")].into_iter().collect(),
    };
    assert!(app.apply_ignored(cur), "現世代は反映する");
    assert!(app.git_ignored.contains(&dir.join("y")));
    assert_eq!(
        app.git_ignored_for.as_deref(),
        Some(dir.as_path()),
        "反映で workdir をキャッシュキーに記録"
    );
    assert!(
        app.git_ignored_pending.is_none(),
        "現世代の反映で pending を消す"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[cfg(feature = "git")]
#[test]
fn graph_legend_caps_branches_head_first_and_picker_toggles() {
    use std::process::Command;
    let dir = std::env::temp_dir().join("konoma_graph_legend_cap_picker");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    init_git_repo(&dir);
    let git = |args: &[&str]| {
        Command::new("git")
            .current_dir(&dir)
            .args(args)
            .output()
            .unwrap();
    };
    std::fs::write(dir.join("a.txt"), b"a\n").unwrap();
    git(&["add", "-A"]);
    git(&["commit", "-q", "-m", "init"]);
    git(&["branch", "-M", "trunk"]); // give it a known current branch name.
                                     // 5 feature branches (each with its own commit = its own lane).
    for n in ["f1", "f2", "f3", "f4", "f5"] {
        git(&["checkout", "-q", "trunk"]);
        git(&["checkout", "-q", "-b", n]);
        std::fs::write(dir.join(format!("{n}.txt")), b"x\n").unwrap();
        git(&["add", "-A"]);
        git(&["commit", "-q", "-m", &format!("{n} c")]);
    }
    git(&["checkout", "-q", "trunk"]); // HEAD=trunk. 6 branches total.

    let mut cfg = Config::default();
    cfg.ui.graph_max_branches = 3; // a small cap.
    let mut app = App::new(dir.canonicalize().unwrap(), cfg).unwrap();
    app.open_git_graph();
    assert!(app.is_git_graph(), "グラフが開く");

    // Legend: HEAD (trunk) comes first, up to the cap of 3, and the rest are hidden.
    let legend = app.git_graph_legend();
    assert!(!legend.is_empty(), "凡例が空でない");
    assert!(
        legend[0].is_head && legend[0].name == "trunk",
        "凡例の先頭は HEAD(trunk): {:?}",
        legend
            .iter()
            .map(|e| (&e.name, e.is_head))
            .collect::<Vec<_>>()
    );
    assert!(
        legend.len() <= 3,
        "表示ブランチは上限3まで: {}",
        legend.len()
    );
    assert!(app.git_graph_hidden_count() >= 1, "一部は非表示");

    // Panel: HEAD is always first and always ON (can't be turned off).
    app.git_graph_open_picker();
    assert!(app.is_git_graph_picker(), "パネルが開く");
    let items = app.git_graph_picker_items();
    assert!(items[0].1, "パネル先頭は HEAD(is_current)");
    app.git_graph_picker_sel = 0;
    app.git_graph_picker_toggle(); // trying to toggle off HEAD has no effect.
    assert!(
        app.git_graph_picker_items()[0].2,
        "HEAD はトグルしても表示のまま"
    );

    // a=everything → apply → 0 hidden, 6 in the legend.
    app.git_graph_picker_all();
    app.git_graph_picker_apply();
    assert!(!app.is_git_graph_picker(), "適用でパネルが閉じる");
    assert_eq!(app.git_graph_hidden_count(), 0, "全表示で非表示0");
    assert_eq!(app.git_graph_legend().len(), 6, "全6ブランチが凡例に出る");

    std::fs::remove_dir_all(&dir).ok();
}

#[cfg(feature = "git")]
#[test]
fn open_git_view_lists_changes_and_stage_reloads() {
    let dir = std::env::temp_dir().join("konoma_app_git_view");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    init_git_repo(&dir);
    // Create and commit one tracked file → then change it (=Modified).
    std::fs::write(dir.join("tracked.txt"), b"v1\n").unwrap();
    let out = std::process::Command::new("git")
        .current_dir(&dir)
        .args(["add", "-A"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let out = std::process::Command::new("git")
        .current_dir(&dir)
        .args(["commit", "-m", "init"])
        .output()
        .unwrap();
    assert!(out.status.success());
    std::fs::write(dir.join("tracked.txt"), b"v2\n").unwrap(); // modified
    std::fs::write(dir.join("untracked.txt"), b"new\n").unwrap(); // untracked

    let mut app = App::new(dir.canonicalize().unwrap(), Config::default()).unwrap();
    app.open_git_view();
    assert!(app.is_git_view(), "Git ビューが開く");
    // The 2 changes (modified + untracked) show up in the list.
    assert_eq!(app.git_view_entries().len(), 2, "変更2件");
    // Both are unstaged by default.
    assert!(
        app.git_view_entries().iter().all(|e| !e.staged),
        "初期は全て未ステージ"
    );

    // Stage the first entry (tracked.txt comes first in path order) → reload sets staged=true.
    app.tab.git_view_sel = 0;
    let target = app.git_view_selected().unwrap();
    app.git_view_stage();
    let staged_now = app
        .git_view_entries()
        .iter()
        .find(|e| e.path == target)
        .map(|e| e.staged)
        .unwrap_or(false);
    assert!(staged_now, "ステージ後は staged=true (reload 反映)");

    app.close_git_view();
    assert!(!app.is_git_view(), "閉じる");
    std::fs::remove_dir_all(&dir).ok();
}

#[cfg(feature = "git")]
#[test]
fn refresh_in_git_view_refetches_entries() {
    // #4: refresh() while showing the Git view re-fetches the change list (git_view_entries)
    // (reflects changes that grew/shrank via returning from an external git tool or via FSEvents).
    let dir = std::env::temp_dir().join("konoma_refresh_git_view_refetch");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    init_git_repo(&dir);
    std::fs::write(dir.join("tracked.txt"), b"v1\n").unwrap();
    let git = |args: &[&str]| {
        assert!(std::process::Command::new("git")
            .current_dir(&dir)
            .args(args)
            .output()
            .unwrap()
            .status
            .success());
    };
    git(&["add", "-A"]);
    git(&["commit", "-m", "init"]);
    std::fs::write(dir.join("tracked.txt"), b"v2\n").unwrap(); // modified

    let mut app = App::new(dir.canonicalize().unwrap(), Config::default()).unwrap();
    app.open_git_view();
    assert!(app.is_git_view());
    assert_eq!(app.git_view_entries().len(), 1, "変更1件(modified)");
    // Add a new file externally → refresh re-fetches the change list and it grows to 2 entries.
    std::fs::write(dir.join("untracked.txt"), b"new\n").unwrap();
    app.refresh().unwrap();
    assert_eq!(
        app.git_view_entries().len(),
        2,
        "refresh で git_view_entries が再取得され untracked が増える"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[cfg(feature = "git")]
#[test]
fn git_view_stage_all_then_unstage_all() {
    let dir = std::env::temp_dir().join("konoma_app_git_stage_all");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    init_git_repo(&dir);
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .current_dir(&dir)
            .args(args)
            .output()
            .unwrap();
    };
    std::fs::write(dir.join("a.txt"), b"v1\n").unwrap();
    git(&["add", "-A"]);
    git(&["commit", "-m", "init"]);
    std::fs::write(dir.join("a.txt"), b"v2\n").unwrap(); // modified
    std::fs::write(dir.join("b.txt"), b"new\n").unwrap(); // untracked

    let mut app = App::new(dir.canonicalize().unwrap(), Config::default()).unwrap();
    app.open_git_view();
    assert_eq!(app.git_view_entries().len(), 2, "変更2件");
    assert!(
        app.git_view_entries().iter().all(|e| !e.staged),
        "初期は全未ステージ"
    );

    // S=stage all → everything becomes staged (add -A, including new files).
    app.git_view_stage_all();
    assert!(
        app.git_view_entries().iter().all(|e| e.staged),
        "全ステージ後は全 staged"
    );

    // U=unstage all → everything becomes unstaged.
    app.git_view_unstage_all();
    assert!(
        app.git_view_entries().iter().all(|e| !e.staged),
        "全アンステージ後は全未ステージ"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[cfg(feature = "git")]
#[test]
fn git_diff_opens_from_view_and_esc_returns_to_view() {
    let dir = std::env::temp_dir().join("konoma_app_gitdiff_roundtrip");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    init_git_repo(&dir);
    std::fs::write(dir.join("a.txt"), b"v1\n").unwrap();
    let sh = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .current_dir(&dir)
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success());
    };
    sh(&["add", "-A"]);
    sh(&["commit", "-m", "init"]);
    std::fs::write(dir.join("a.txt"), b"v2\n").unwrap(); // modified

    let canon = dir.canonicalize().unwrap();
    let mut app = App::new(canon.clone(), Config::default()).unwrap();
    app.open_git_view();
    assert!(app.is_git_view());
    let target = app.git_view_selected().unwrap();
    // Open a diff from the Git view: enters Preview mode, GitDiff, and the Git view closes.
    app.open_git_diff(&target);
    assert!(app.is_git_diff_preview(), "GitDiff プレビューになる");
    assert_eq!(app.tab.mode, Mode::Preview);
    assert!(!app.is_git_view(), "Git ビューは閉じる");
    // diff lines can be fetched (non-empty since there's a change).
    assert!(!app.git_diff_lines().is_empty(), "diff 行が空");
    // C2: a diff, once fetched, is cached per path (doesn't hit git on every frame).
    assert!(app.diff_cache.is_some(), "diff キャッシュが張られる");
    // A refresh that could change the working tree (an FS event / a manual refresh) drops the
    // cache (follows external edits and never shows a stale diff).
    app.refresh().unwrap();
    assert!(
        app.diff_cache.is_none(),
        "refresh で diff キャッシュが無効化される"
    );
    // Equivalent to Esc: returns to the Git view.
    app.close_git_diff();
    assert!(app.is_git_view(), "Esc で Git ビューへ戻る");
    assert!(!app.is_git_diff_preview());
    std::fs::remove_dir_all(&dir).ok();
}

#[cfg(feature = "git")]
#[test]
fn git_diff_discard_confirm_reverts_and_returns_to_view() {
    let dir = std::env::temp_dir().join("konoma_app_gitdiff_discard");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    init_git_repo(&dir);
    let f = dir.join("a.txt");
    std::fs::write(&f, b"v1\n").unwrap();
    let sh = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .current_dir(&dir)
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success());
    };
    sh(&["add", "-A"]);
    sh(&["commit", "-m", "init"]);
    std::fs::write(&f, b"v2\n").unwrap(); // modified

    let canon = dir.canonicalize().unwrap();
    let mut app = App::new(canon.clone(), Config::default()).unwrap();
    app.open_git_view();
    let target = app.git_view_selected().unwrap();
    app.open_git_diff(&target);
    // x: the discard-confirmation dialog.
    app.git_diff_start_discard();
    assert!(app.is_dialog() && app.dialog_is_confirm(), "破棄確認が出る");
    // Confirm with y: the file reverts to v1, and it returns to the Git view now clean.
    app.dialog_confirm(true).unwrap();
    assert_eq!(std::fs::read_to_string(&f).unwrap(), "v1\n", "破棄で復元");
    assert!(app.is_git_view(), "破棄後は Git ビューへ戻る");
    assert!(!app.is_git_diff_preview());
    assert!(
        app.git_view_entries().is_empty(),
        "破棄後はクリーン(変更0件)"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[cfg(feature = "git")]
#[test]
fn open_git_view_noop_when_not_a_repo() {
    // Doesn't open in a directory that's not a git repository (no-op + flash).
    let dir = std::env::temp_dir().join("konoma_app_git_view_norepo");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = App::new(dir.canonicalize().unwrap(), Config::default()).unwrap();
    app.open_git_view();
    assert!(!app.is_git_view(), "repo でなければ開かない");
    assert!(app.flash.is_some(), "flash で知らせる");
    std::fs::remove_dir_all(&dir).ok();
}

#[cfg(feature = "git")]
#[test]
fn git_branches_list_checkout_and_create() {
    use std::process::Command;
    let dir = std::env::temp_dir().join("konoma_app_git_branch");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let git = |args: &[&str]| {
        Command::new("git")
            .current_dir(&dir)
            .args(args)
            .output()
            .unwrap();
    };
    git(&["init", "-q"]);
    git(&["config", "user.email", "t@t.t"]);
    git(&["config", "user.name", "t"]);
    std::fs::write(dir.join("a.txt"), b"x").unwrap();
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "init"]);
    git(&["branch", "feature"]); // a second branch
    let mut app = App::new(dir.canonicalize().unwrap(), Config::default()).unwrap();

    // The list: includes feature, and the cursor sits on the current branch.
    app.open_git_branches();
    assert!(app.is_git_branches());
    let view = app.git_branch_view();
    let names: Vec<String> = view.iter().map(|b| b.name.clone()).collect();
    assert!(
        names.iter().any(|n| n == "feature"),
        "feature が一覧に: {names:?}"
    );
    assert!(
        view[app.git_branch_sel()].is_current,
        "カーソルは現在ブランチ"
    );

    // Filter: "feat" narrows it down to feature.
    app.git_branch_start_filter();
    for c in "feat".chars() {
        app.git_branch_filter_push(c);
    }
    let filtered = app.git_branch_view();
    assert_eq!(filtered.len(), 1, "絞り込みで1件");
    assert_eq!(filtered[0].name, "feature");
    app.git_branch_filter_clear();
    assert_eq!(app.git_branch_view().len(), names.len(), "解除で全件へ戻る");

    // Select feature and check it out → the current branch changes and the list closes.
    let fi = app
        .git_branch_view()
        .iter()
        .position(|b| b.name == "feature")
        .unwrap();
    app.git_branch_move(fi as i32 - app.git_branch_sel() as i32);
    app.checkout_selected_branch().unwrap();
    assert!(!app.is_git_branches(), "checkout で一覧は閉じる");
    assert_eq!(app.git_branch(), Some("feature"), "現在ブランチ=feature");

    // New branch creation flow: enter a name → submit creates it and switches (now on "newbr").
    app.open_git_branches();
    app.start_create_branch();
    for c in "newbr".chars() {
        app.dialog_input_push(c);
    }
    app.dialog_submit().unwrap();
    assert_eq!(app.git_branch(), Some("newbr"), "作成して切替");

    // Delete: the current branch (newbr) cannot be deleted; another branch (feature) can be.
    app.open_git_branches();
    // Put the cursor on the current branch (newbr) and press d → the guard prevents it opening.
    app.start_delete_branch();
    assert!(!app.is_dialog(), "現在ブランチは削除確認を開かない");
    // Move the cursor to feature and confirm deletion → y (a safe delete).
    let fi = app
        .git_branch_view()
        .iter()
        .position(|b| b.name == "feature")
        .unwrap();
    app.git_branch_move(fi as i32 - app.git_branch_sel() as i32);
    app.start_delete_branch();
    assert!(app.is_dialog() && app.dialog_is_confirm(), "削除確認が開く");
    app.dialog_confirm(true).unwrap(); // y=safe delete (-d)
    assert!(
        !app.git_branch_view().iter().any(|b| b.name == "feature"),
        "feature が削除された"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// `worktree_create_target`'s directory-name rule: every `/` in the branch name becomes `-` (git
/// would otherwise create a **nested** directory for a branch like `feat/nested/deep`, i.e.
/// `feat/nested/deep/` as three levels, not one flat sibling directory).
#[cfg(feature = "git")]
#[test]
fn worktree_create_target_replaces_slash_with_dash_for_the_directory_name() {
    use std::process::Command;
    let dir = unique_tmp("konoma_wt_target_slash");
    std::fs::create_dir_all(&dir).unwrap();
    init_git_repo(&dir);
    std::fs::write(dir.join("a.txt"), b"x").unwrap();
    Command::new("git")
        .current_dir(&dir)
        .args(["add", "."])
        .output()
        .unwrap();
    Command::new("git")
        .current_dir(&dir)
        .args(["commit", "-q", "-m", "init"])
        .output()
        .unwrap();
    let root = dir.canonicalize().unwrap();
    let app = App::new(root.clone(), Config::default()).unwrap();

    let target = app.worktree_create_target("feat/nested/deep");
    assert_eq!(
        target.file_name().and_then(|n| n.to_str()),
        Some("feat-nested-deep"),
        "全ての / が - に置換され、1階層のディレクトリ名になる: {target:?}"
    );
    // Not a nested structure: the parent of `target` must not itself be named after a slash-split prefix.
    let parent_name = target
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str());
    assert_ne!(
        parent_name,
        Some("feat"),
        "入れ子ディレクトリが作られてはいけない: {target:?}"
    );
    assert_ne!(
        parent_name,
        Some("feat-nested"),
        "入れ子ディレクトリが作られてはいけない: {target:?}"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// `worktree_create_target` resolves against the **main** worktree's own path, not `self.tab.root`,
/// and respects `[git] worktree_dir` — the default `"../"` places it as a sibling of the main
/// worktree, and a custom (here: absolute) value changes the placement entirely.
#[cfg(feature = "git")]
#[test]
fn worktree_create_target_respects_worktree_dir_config() {
    use std::process::Command;
    let dir = unique_tmp("konoma_wt_target_dircfg");
    std::fs::create_dir_all(&dir).unwrap();
    init_git_repo(&dir);
    std::fs::write(dir.join("a.txt"), b"x").unwrap();
    Command::new("git")
        .current_dir(&dir)
        .args(["add", "."])
        .output()
        .unwrap();
    Command::new("git")
        .current_dir(&dir)
        .args(["commit", "-q", "-m", "init"])
        .output()
        .unwrap();
    let root = dir.canonicalize().unwrap();

    // Default ("../"): a sibling of the main worktree.
    let app_default = App::new(root.clone(), Config::default()).unwrap();
    let default_target = app_default.worktree_create_target("brA");
    assert_eq!(
        default_target.parent().unwrap().canonicalize().unwrap(),
        root.parent().unwrap().canonicalize().unwrap(),
        "既定 \"../\" はメインワークツリーの隣に置く: {default_target:?}"
    );

    // Custom (absolute) worktree_dir: placement follows it instead.
    let custom_base = unique_tmp("konoma_wt_target_dircfg_custom");
    std::fs::create_dir_all(&custom_base).unwrap();
    let mut cfg = Config::default();
    cfg.git.worktree_dir = custom_base.to_string_lossy().into_owned();
    let app_custom = App::new(root.clone(), cfg).unwrap();
    let custom_target = app_custom.worktree_create_target("brB");
    assert_eq!(
        custom_target.parent().unwrap().canonicalize().unwrap(),
        custom_base.canonicalize().unwrap(),
        "worktree_dir を変えると置き場所が変わる: {custom_target:?}"
    );

    std::fs::remove_dir_all(&dir).ok();
    std::fs::remove_dir_all(&custom_base).ok();
}

/// Full `n` flow: type a brand-new branch name, submit → `git worktree add -b` creates the branch
/// and the directory, the list closes, and this tab switches its root (and `open_dir`) into it.
#[cfg(feature = "git")]
#[test]
fn worktree_create_flow_creates_a_new_branch_and_switches_into_it() {
    use std::process::Command;
    let dir = unique_tmp("konoma_wt_flow_new");
    std::fs::create_dir_all(&dir).unwrap();
    init_git_repo(&dir);
    std::fs::write(dir.join("a.txt"), b"x").unwrap();
    Command::new("git")
        .current_dir(&dir)
        .args(["add", "."])
        .output()
        .unwrap();
    Command::new("git")
        .current_dir(&dir)
        .args(["commit", "-q", "-m", "init"])
        .output()
        .unwrap();
    let root = dir.canonicalize().unwrap();
    // A private worktree_dir keeps this test's placement from ever colliding with another
    // test/parallel run sharing the OS temp dir (unlike a branch name, this base is unique per test).
    let base = unique_tmp("konoma_wt_flow_new_base");
    std::fs::create_dir_all(&base).unwrap();
    let mut cfg = Config::default();
    cfg.git.worktree_dir = base.to_string_lossy().into_owned();
    let mut app = App::new(root.clone(), cfg).unwrap();

    app.open_git_worktrees();
    assert!(app.is_git_worktrees());
    app.start_create_worktree();
    assert!(app.is_dialog());
    for c in "feat-fresh".chars() {
        app.dialog_input_push(c);
    }
    app.dialog_submit().unwrap();

    assert!(!app.is_git_worktrees(), "作成後は一覧を閉じる");
    let new_root = app.tab.root.clone();
    assert_ne!(new_root, root, "root が新しいワークツリーへ切替わる");
    assert_eq!(app.tab.open_dir, new_root, "open_dir も追従する");
    assert!(new_root.is_dir(), "ディレクトリが実際に作られる");

    let list = crate::git::worktrees(&root);
    let created = list
        .iter()
        .find(|w| w.path == new_root)
        .expect("git worktree list に出る");
    assert_eq!(created.branch.as_deref(), Some("feat-fresh"));
    assert!(
        crate::git::branch_tip(&root, "feat-fresh").is_some(),
        "新規ブランチが作られる"
    );

    std::fs::remove_dir_all(&base).ok();
    std::fs::remove_dir_all(&dir).ok();
}

/// Full `n` flow on an **existing** branch (not checked out anywhere): no `-b`, checked out as-is,
/// and the branch is not duplicated.
#[cfg(feature = "git")]
#[test]
fn worktree_create_flow_checks_out_an_existing_branch_without_creating_a_duplicate() {
    use std::process::Command;
    let dir = unique_tmp("konoma_wt_flow_existing");
    std::fs::create_dir_all(&dir).unwrap();
    init_git_repo(&dir);
    std::fs::write(dir.join("a.txt"), b"x").unwrap();
    Command::new("git")
        .current_dir(&dir)
        .args(["add", "."])
        .output()
        .unwrap();
    Command::new("git")
        .current_dir(&dir)
        .args(["commit", "-q", "-m", "init"])
        .output()
        .unwrap();
    // A branch that already exists but isn't checked out anywhere yet.
    Command::new("git")
        .current_dir(&dir)
        .args(["branch", "already-there"])
        .output()
        .unwrap();
    let root = dir.canonicalize().unwrap();
    let base = unique_tmp("konoma_wt_flow_existing_base");
    std::fs::create_dir_all(&base).unwrap();
    let mut cfg = Config::default();
    cfg.git.worktree_dir = base.to_string_lossy().into_owned();
    let mut app = App::new(root.clone(), cfg).unwrap();

    app.open_git_worktrees();
    app.start_create_worktree();
    for c in "already-there".chars() {
        app.dialog_input_push(c);
    }
    app.dialog_submit().unwrap();

    let new_root = app.tab.root.clone();
    assert_ne!(new_root, root);
    assert!(new_root.is_dir());
    let branches = crate::git::branches(&root);
    assert_eq!(
        branches
            .iter()
            .filter(|b| b.name == "already-there")
            .count(),
        1,
        "既存ブランチが二重作成されない: {branches:?}"
    );
    let list = crate::git::worktrees(&root);
    let created = list
        .iter()
        .find(|w| w.path == new_root)
        .expect("git worktree list に出る");
    assert_eq!(created.branch.as_deref(), Some("already-there"));

    std::fs::remove_dir_all(&base).ok();
    std::fs::remove_dir_all(&dir).ok();
}

/// A branch already checked out by another worktree: `git worktree add` refuses, `dialog_submit`
/// flashes git's own `fatal: ...` line (leading, not buried behind the command that was run — the
/// flash footer is a single line clipped at the terminal width, so anything ahead of the reason
/// pushes it off-screen), the root stays put, and the list — never touched on this failure path —
/// is still open once the (now-closed) dialog is gone.
#[cfg(feature = "git")]
#[test]
fn worktree_create_flow_fails_with_gits_message_when_branch_is_already_checked_out_elsewhere() {
    use std::process::Command;
    let dir = unique_tmp("konoma_wt_flow_conflict");
    std::fs::create_dir_all(&dir).unwrap();
    init_git_repo(&dir);
    std::fs::write(dir.join("a.txt"), b"x").unwrap();
    Command::new("git")
        .current_dir(&dir)
        .args(["add", "."])
        .output()
        .unwrap();
    Command::new("git")
        .current_dir(&dir)
        .args(["commit", "-q", "-m", "init"])
        .output()
        .unwrap();
    let root = dir.canonicalize().unwrap();
    let base = unique_tmp("konoma_wt_flow_conflict_base");
    std::fs::create_dir_all(&base).unwrap();

    // A first worktree already has branch "taken" checked out.
    let first = base.join("first");
    let out = Command::new("git")
        .current_dir(&root)
        .args([
            "worktree",
            "add",
            "-q",
            "-b",
            "taken",
            first.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(out.status.success(), "setup: git worktree add: {out:?}");

    let mut cfg = Config::default();
    cfg.git.worktree_dir = base.to_string_lossy().into_owned();
    let mut app = App::new(root.clone(), cfg).unwrap();

    app.open_git_worktrees();
    app.start_create_worktree();
    for c in "taken".chars() {
        app.dialog_input_push(c);
    }
    app.dialog_submit().unwrap();

    assert_eq!(app.tab.root, root, "失敗時は root が変わらない");
    assert!(app.is_git_worktrees(), "失敗時は一覧が開いたまま");
    let flash = app.flash.clone().unwrap_or_default();
    assert!(
        flash.starts_with("fatal:"),
        "git の fatal: メッセージが先頭に来る(コマンド文字列で押し出されない): {flash}"
    );
    assert!(
        !flash.contains("worktree add"),
        "実行したコマンド文字列は含まない: {flash}"
    );

    std::fs::remove_dir_all(&base).ok();
    std::fs::remove_dir_all(&dir).ok();
}

#[cfg(feature = "git")]
#[test]
fn git_commit_flow_creates_commit_with_message() {
    let dir = std::env::temp_dir().join("konoma_app_git_commit_flow");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    init_git_repo(&dir);
    let f = dir.join("a.txt");
    std::fs::write(&f, b"hello\n").unwrap();
    // Stage via the public API (commit uses the staged index).
    crate::git::stage(&dir, &f).unwrap();

    let canon = dir.canonicalize().unwrap();
    let mut app = App::new(canon.clone(), Config::default()).unwrap();
    app.open_git_view();
    assert!(app.is_git_view());

    // Equivalent to c: opens the commit message input dialog.
    app.start_git_commit();
    assert!(app.is_dialog(), "コミット入力ダイアログが出る");
    // internal_mode reports Commit (distinguishable from other Input states).
    assert_eq!(app.internal_mode(), Some(InternalMode::Commit));

    // Type the message one character at a time.
    for c in "feat: add a".chars() {
        app.dialog_input_push(c);
    }
    // Equivalent to Enter: commit → commit succeeds.
    app.dialog_submit().unwrap();
    assert!(!app.is_dialog(), "成功するとダイアログは閉じる");

    // HEAD now has a commit with that summary.
    let entries = crate::git::log(&canon, 10);
    assert_eq!(entries.len(), 1, "コミット1件のはず");
    assert_eq!(entries[0].summary, "feat: add a");
    std::fs::remove_dir_all(&dir).ok();
}

#[cfg(feature = "git")]
#[test]
fn git_commit_empty_message_is_rejected() {
    let dir = std::env::temp_dir().join("konoma_app_git_commit_empty");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    init_git_repo(&dir);
    let f = dir.join("a.txt");
    std::fs::write(&f, b"hello\n").unwrap();
    crate::git::stage(&dir, &f).unwrap();

    let canon = dir.canonicalize().unwrap();
    let mut app = App::new(canon.clone(), Config::default()).unwrap();
    app.start_git_commit();
    // Enter only whitespace → committing it is rejected, and no commit is created.
    app.dialog_input_push(' ');
    app.dialog_submit().unwrap();
    assert!(
        app.flash
            .as_deref()
            .is_some_and(|s| s.contains("empty") || s.contains("空")),
        "空メッセージは flash で拒否: {:?}",
        app.flash
    );
    assert!(
        crate::git::log(&canon, 10).is_empty(),
        "空メッセージではコミットされない"
    );
    std::fs::remove_dir_all(&dir).ok();
}

// --- git log + commit detail ---------------------------------------------
#[cfg(feature = "git")]
#[test]
fn git_log_lists_commits_and_detail_has_diff_lines() {
    let dir = std::env::temp_dir().join("konoma_app_git_log_detail");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    init_git_repo(&dir);
    let sh = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .current_dir(&dir)
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?} 失敗");
    };
    // Create 2 commits.
    std::fs::write(dir.join("a.txt"), b"one\n").unwrap();
    sh(&["add", "-A"]);
    sh(&["commit", "-m", "first commit"]);
    std::fs::write(dir.join("a.txt"), b"one\ntwo\n").unwrap();
    sh(&["add", "-A"]);
    sh(&["commit", "-m", "second commit"]);

    let canon = dir.canonicalize().unwrap();
    let mut app = App::new(canon.clone(), Config::default()).unwrap();
    // Open log via the Git view (matches the real flow).
    app.open_git_view();
    app.open_git_log();
    assert!(app.is_git_log(), "git log が開く");
    assert!(!app.is_git_view(), "log を開くと Git ビューは閉じる");
    assert_eq!(app.git_log_entries().len(), 2, "コミット2件");
    // Newest first: second commit is at the front.
    assert_eq!(app.git_log_entries()[0].summary, "second commit");
    assert_eq!(app.internal_mode(), Some(InternalMode::GitLog));

    // Select the 2nd entry (the older one = first commit, the root) and open its detail →
    // the diff lines are non-empty.
    app.git_log_move(1);
    assert_eq!(app.git_log_sel(), 1);
    app.open_git_commit_detail();
    assert!(app.is_git_detail(), "コミット詳細が開く");
    assert_eq!(app.internal_mode(), Some(InternalMode::GitDetail));
    let lines = app.git_detail_lines();
    assert!(!lines.is_empty(), "コミット詳細の diff 行が空");
    assert!(
        lines
            .iter()
            .any(|l| l.kind == crate::git::DiffLineKind::Added),
        "Added 行が無い: {lines:?}"
    );

    // Equivalent to q: closing the detail returns to log.
    app.close_git_detail();
    assert!(!app.is_git_detail());
    assert!(app.is_git_log(), "詳細を閉じると log へ戻る");
    // Closing log returns to the Git view.
    app.close_git_log();
    assert!(!app.is_git_log());
    assert!(app.is_git_view(), "log を閉じると Git ビューへ戻る");
    std::fs::remove_dir_all(&dir).ok();
}

#[cfg(feature = "git")]
#[test]
fn open_git_log_noop_when_no_commits() {
    let dir = std::env::temp_dir().join("konoma_app_git_log_unborn");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    init_git_repo(&dir); // no commits (unborn)
    let canon = dir.canonicalize().unwrap();
    let mut app = App::new(canon, Config::default()).unwrap();
    app.open_git_log();
    assert!(!app.is_git_log(), "コミットが無ければ log は開かない");
    std::fs::remove_dir_all(&dir).ok();
}

// ============================================================================
// Additional coverage: App's pure state, preview scrolling, images/GIF, links, tabs, etc.
// Follows the existing convention (temp_dir + a unique name, cleaned up at the end with
// remove_dir_all).
// ============================================================================

/// Generates and writes out a small PNG for testing (to check the decode path).
fn write_png(path: &Path, w: u32, h: u32) {
    let img = image::RgbImage::from_pixel(w, h, image::Rgb([10, 20, 30]));
    image::DynamicImage::ImageRgb8(img).save(path).unwrap();
}

#[test]
fn path_style_next_cycles_and_cycle_path_style_advances() {
    // The pure function's cycle: rel→home→full→rel.
    assert_eq!(PathStyle::Relative.next(), PathStyle::Home);
    assert_eq!(PathStyle::Home.next(), PathStyle::Full);
    assert_eq!(PathStyle::Full.next(), PathStyle::Relative);

    let dir = std::env::temp_dir().join("konoma_cycle_pathstyle_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    let s0 = app.path_style;
    app.cycle_path_style();
    assert_eq!(app.path_style, s0.next(), "cycle は next() を適用する");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn toggle_hidden_reveals_and_hides_dotfiles() {
    let dir = std::env::temp_dir().join("konoma_toggle_hidden_app_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("visible.txt"), b"x").unwrap();
    std::fs::write(dir.join(".secret"), b"x").unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.rebuild_tree().unwrap();
    let has_dot = |a: &App| a.tab.entries.iter().any(|e| e.path.ends_with(".secret"));
    assert!(
        !app.tab.show_hidden && !has_dot(&app),
        "既定では隠しは出ない"
    );
    app.toggle_hidden().unwrap();
    assert!(
        app.tab.show_hidden && has_dot(&app),
        "トグルで .secret が現れる"
    );
    app.toggle_hidden().unwrap();
    assert!(!app.tab.show_hidden && !has_dot(&app), "もう一度で隠れる");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn sort_menu_open_and_close_toggles_flag() {
    let dir = std::env::temp_dir().join("konoma_sortmenu_close_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    assert!(!app.is_sort_menu());
    app.open_sort_menu();
    assert!(app.is_sort_menu());
    app.close_sort_menu();
    assert!(!app.is_sort_menu(), "close_sort_menu でフラグが下りる");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn looks_like_gif_uses_magic_not_extension() {
    let dir = std::env::temp_dir().join("konoma_looks_like_gif_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.gif"), b"GIF89a\x00\x00").unwrap();
    std::fs::write(dir.join("old.gif"), b"GIF87a\x00\x00").unwrap();
    std::fs::write(dir.join("fake.gif"), b"not a gif at all").unwrap();
    std::fs::write(dir.join("real.bin"), b"GIF89a binary").unwrap();
    // Judged by the GIF8x magic bytes (the extension is irrelevant).
    assert!(App::looks_like_gif(&dir.join("a.gif")));
    assert!(App::looks_like_gif(&dir.join("old.gif")));
    assert!(
        !App::looks_like_gif(&dir.join("fake.gif")),
        "中身が GIF でなければ false"
    );
    assert!(
        App::looks_like_gif(&dir.join("real.bin")),
        ".gif でなくてもマジックがあれば true"
    );
    assert!(
        !App::looks_like_gif(&dir.join("nope")),
        "存在しないファイルは false"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn keymap_report_none_default_and_formats_conflicts_and_warnings() {
    use crate::keymap::{ConflictKind, KeyConflict, KeyPress, Surface};
    let dir = std::env::temp_dir().join("konoma_keymap_report_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    // The default keymap has no conflicts → None.
    assert!(app.keymap_report().is_none(), "既定では報告なし");

    // Planting 1 conflict + 1 invalid setting yields a single line with the counts (English).
    app.keymaps.conflicts.push(KeyConflict {
        surface: Surface::Tree,
        key: KeyPress::ch('x'),
        kept: "kept".into(),
        dropped: "dropped".into(),
        reason: ConflictKind::GlobalShadow,
    });
    app.keymaps.warnings.push("bad setting".into());
    app.lang = crate::i18n::Lang::En;
    let en = app.keymap_report().expect("衝突/警告があれば Some");
    assert!(en.contains("1 key conflict"), "英語の衝突件数: {en}");
    assert!(
        en.contains("1 invalid key setting"),
        "英語の無効設定件数: {en}"
    );

    // Also shown with counts in Japanese.
    app.lang = crate::i18n::Lang::Jp;
    let jp = app.keymap_report().expect("Some");
    assert!(jp.contains("キー衝突1件"), "日本語の衝突件数: {jp}");
    assert!(jp.contains("無効な設定1件"), "日本語の無効設定件数: {jp}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn take_warm_job_returns_once_then_none() {
    let dir = std::env::temp_dir().join("konoma_take_warm_job_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    // Build a state that's highlight-pending with a preview target.
    app.tab.preview_path = Some(dir.join("main.rs"));
    app.hl_pending = true;
    app.hl_warming = false;
    let job = app.take_warm_job().expect("起動対象を1度だけ返す");
    assert_eq!(job.0, "rs", "拡張子");
    assert_eq!(job.1, dir.join("main.rs"));
    // The second time it's warming, so None (prevents a double launch).
    assert!(app.take_warm_job().is_none(), "二重起動しない");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn launch_git_tool_sets_pending_flag_and_take_clears_it() {
    let dir = std::env::temp_dir().join("konoma_launch_git_tool_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    assert!(!app.take_launch_git_tool(), "初期は要求なし");
    app.launch_git_tool(); // doesn't launch a process, just sets the flag
    assert!(app.take_launch_git_tool(), "要求が立つ");
    assert!(!app.take_launch_git_tool(), "take で消費される");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn preview_scroll_paging_and_to_top_non_windowed() {
    let dir = std::env::temp_dir().join("konoma_preview_scroll_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.tab.preview_viewport = 20;
    // Non-windowed: operates preview_scroll directly.
    app.preview_scroll(5);
    assert_eq!(app.tab.preview_scroll, 5);
    app.preview_scroll(-100);
    assert_eq!(app.tab.preview_scroll, 0, "下限 0 でクランプ");
    app.preview_page(1); // +19 (overlaps by 1 line)
    assert_eq!(app.tab.preview_scroll, 19);
    app.preview_half_page(1); // +10
    assert_eq!(app.tab.preview_scroll, 29);
    app.preview_page(-1); // -19
    assert_eq!(app.tab.preview_scroll, 10);
    app.preview_to_top();
    assert_eq!(app.tab.preview_scroll, 0, "to_top で先頭へ");
    std::fs::remove_dir_all(&dir).ok();
}

/// Opens a fresh windowed (Code/Text) text preview: creates `name` with `n` lines
/// ("line001".."lineNNN"), activates it from the tree, and sets `preview_viewport`. Used by the
/// `windowed_page_*` regression tests below (see `App::windowed_page_scroll`'s doc comment for the
/// bug this whole group pins down).
fn windowed_text_app(dir: &std::path::Path, name: &str, n: usize, viewport: u16) -> App {
    std::fs::create_dir_all(dir).unwrap();
    let mut body = String::new();
    for i in 1..=n {
        body.push_str(&format!("line{i:03}\n"));
    }
    std::fs::write(dir.join(name), &body).unwrap();
    let mut app = App::new(dir.canonicalize().unwrap(), Config::default()).unwrap();
    let idx = app
        .tab
        .entries
        .iter()
        .position(|e| e.path.ends_with(name))
        .unwrap();
    app.tab.selected = idx;
    app.tree_activate().unwrap();
    assert!(app.is_windowed(), "テキストファイルは windowed のはず");
    app.tab.preview_viewport = viewport;
    app
}

#[test]
fn windowed_page_down_then_up_returns_to_the_same_position() {
    // #1 symmetry: PageDown then PageUp (same size) lands back exactly where it started,
    // including the caret's on-screen row, when neither step is clamped by an edge.
    let dir = unique_tmp("konoma_wpage_symmetry");
    let mut app = windowed_text_app(&dir, "big.txt", 400, 20);
    // Start away from the top row (row_in_view=5) so the round-trip actually exercises the
    // caret-carrying math, not just the trivial top==0 case.
    app.preview_scroll(5); // j×5: cursor 0→5, window doesn't move yet (still on screen)
    let top0 = app.tab.preview_top_line;
    let cur0 = app.tab.preview_cursor_line;
    assert_eq!((top0, cur0), (0, 5));

    app.preview_page(1);
    assert_ne!(app.tab.preview_top_line, top0, "ページ送りで窓が動くはず");
    app.preview_page(-1);
    assert_eq!(
        app.tab.preview_top_line, top0,
        "1往復で窓が元の位置に戻らない"
    );
    assert_eq!(
        app.tab.preview_cursor_line, cur0,
        "キャレットも元の行に戻らない"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn windowed_first_page_up_after_paging_down_moves_the_window() {
    // #2 the regression itself: after paging down repeatedly, the caret sits on the last visible
    // row. The *old* code moved the caret back up by one page via `preview_cursor_move`, which
    // landed it exactly on `top` — still "on screen" — so `follow_cursor` saw nothing to do and
    // the window never moved: the first PageUp after a run of PageDowns was silently swallowed.
    let dir = unique_tmp("konoma_wpage_regression");
    let mut app = windowed_text_app(&dir, "big.txt", 400, 20);
    // page = viewport.saturating_sub(1) = 19.
    app.preview_page(1);
    app.preview_page(1);
    app.preview_page(1);
    assert_eq!(app.tab.preview_top_line, 57, "3 回の下ページ送り後の窓位置");
    assert_eq!(app.tab.preview_cursor_line, 57);

    let top_before_up = app.tab.preview_top_line;
    app.preview_page(-1);
    assert_ne!(
        app.tab.preview_top_line, top_before_up,
        "下ページ送りの直後、最初の上ページ送りで窓が動かない(バグの再現)"
    );
    assert_eq!(app.tab.preview_top_line, 38, "1 ページ分だけ戻るはず");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn windowed_first_half_page_up_after_half_paging_down_moves_the_window() {
    // #3: the same regression for the half-page motions (vim Ctrl-d/Ctrl-u, less d/u).
    let dir = unique_tmp("konoma_whalf_regression");
    let mut app = windowed_text_app(&dir, "big.txt", 400, 20);
    // half = viewport/2 = 10.
    app.preview_half_page(1);
    app.preview_half_page(1);
    assert_eq!(
        app.tab.preview_top_line, 20,
        "2 回の下半ページ送り後の窓位置"
    );

    let top_before_up = app.tab.preview_top_line;
    app.preview_half_page(-1);
    assert_ne!(
        app.tab.preview_top_line, top_before_up,
        "下半ページ送りの直後、最初の上半ページ送りで窓が動かない(バグの再現)"
    );
    assert_eq!(app.tab.preview_top_line, 10, "半ページ分だけ戻るはず");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn windowed_page_scroll_preserves_the_caret_on_screen_row() {
    // #4: the caret's row relative to the top of the window (`cursor_line - top_line`) is carried
    // across a page/half-page move, not reset to 0 and not left behind.
    let dir = unique_tmp("konoma_wpage_caret_row");
    let mut app = windowed_text_app(&dir, "big.txt", 400, 20);
    app.preview_scroll(5); // caret at on-screen row 5 (top=0, cursor=5)
    let row = |a: &App| a.tab.preview_cursor_line - a.tab.preview_top_line;
    assert_eq!(row(&app), 5);

    app.preview_page(1);
    assert_eq!(
        row(&app),
        5,
        "ページ送り後もキャレットの画面上の行位置が保たれない"
    );
    app.preview_page(1);
    assert_eq!(row(&app), 5, "2 回目のページ送りでも保たれない");
    app.preview_half_page(-1);
    assert_eq!(row(&app), 5, "半ページ送りでも保たれない");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn windowed_page_up_at_top_is_a_noop_and_does_not_panic() {
    // #5 clamp (top edge): PageUp/HalfPageUp already at the top don't move and don't panic.
    let dir = unique_tmp("konoma_wpage_top_clamp");
    let mut app = windowed_text_app(&dir, "big.txt", 400, 20);
    assert_eq!(
        (app.tab.preview_top_line, app.tab.preview_cursor_line),
        (0, 0)
    );
    app.preview_page(-1);
    assert_eq!(
        (app.tab.preview_top_line, app.tab.preview_cursor_line),
        (0, 0)
    );
    app.preview_half_page(-1);
    assert_eq!(
        (app.tab.preview_top_line, app.tab.preview_cursor_line),
        (0, 0)
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn windowed_page_down_at_bottom_does_not_pass_the_last_page() {
    // #5 clamp (bottom edge): PageDown/HalfPageDown at the end don't scroll past the last page.
    let dir = unique_tmp("konoma_wpage_bottom_clamp");
    let mut app = windowed_text_app(&dir, "big.txt", 400, 20);
    app.preview_to_bottom();
    let top_at_bottom = app.tab.preview_top_line;
    assert_eq!(
        app.tab.preview_cursor_line, 399,
        "to_bottom で最終行(0基準399)へ"
    );

    app.preview_page(1);
    assert_eq!(
        app.tab.preview_top_line, top_at_bottom,
        "最終ページより先へ動かない"
    );
    assert_eq!(
        app.tab.preview_cursor_line, 399,
        "キャレットは最終行にクランプされる"
    );
    app.preview_half_page(1);
    assert_eq!(app.tab.preview_top_line, top_at_bottom);
    assert_eq!(app.tab.preview_cursor_line, 399);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn windowed_single_step_scroll_is_unaffected_by_the_page_scroll_fix() {
    // #6 non-regression: j/k (single-step, via `preview_scroll`) still just move the caret inside
    // the current window and only scroll once the caret would leave the screen — unchanged by the
    // page/half-page fix above (a separate code path, `preview_cursor_move`/`follow_cursor`).
    let dir = unique_tmp("konoma_wpage_jk_nonregression");
    let mut app = windowed_text_app(&dir, "big.txt", 400, 20);
    app.preview_scroll(5); // cursor 0→5, still on screen
    assert_eq!(app.tab.preview_cursor_line, 5);
    assert_eq!(app.tab.preview_top_line, 0, "窓内移動では窓は動かない");
    for _ in 0..14 {
        app.preview_scroll(1); // cursor 5→19 (still the last visible row, vh=20)
    }
    assert_eq!(app.tab.preview_cursor_line, 19);
    assert_eq!(app.tab.preview_top_line, 0, "まだ窓は動かない");
    app.preview_scroll(1); // cursor 19→20: now past the bottom edge
    assert_eq!(app.tab.preview_cursor_line, 20);
    assert_eq!(
        app.tab.preview_top_line, 1,
        "下端超えでようやく窓が1行だけ追従"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn windowed_page_scroll_extends_an_active_visual_selection() {
    // #7: paging while a visual (V) selection is active must extend the selection by the same
    // amount the caret moves (the anchor stays fixed; `preview_selection` reads the live caret).
    //
    // Note on why this test also pins `preview_top_line`: the selection extent tracks
    // `preview_cursor_line`, and that value happens to be a plain cumulative sum of the applied
    // deltas in *both* the pre-fix and the fixed implementation (the pre-fix bug only ever left
    // the *window* behind — the caret itself always advanced correctly). So asserting only on
    // `preview_selection()` here can never fail against the pre-fix code; it would be a
    // non-discriminating test. Asserting the window position too makes this a real regression
    // guard for visual mode specifically (paging must still move the window while selecting, not
    // just move the caret/selection edge).
    let dir = unique_tmp("konoma_wpage_visual_extend");
    let mut app = windowed_text_app(&dir, "big.txt", 400, 20);
    app.preview_enter_visual(true); // V: linewise, anchor at (0, 0)

    app.preview_page(1);
    app.preview_page(1);
    app.preview_page(1);
    let (hi_after_downs, top_after_downs) = match app.preview_selection() {
        PreviewSelection::Line { lo, hi } => {
            assert_eq!(lo, 0, "アンカーは固定");
            assert_eq!(hi, app.tab.preview_cursor_line);
            assert_eq!(hi, 57, "3 回のページ送りで選択が伸びていない");
            (hi, app.tab.preview_top_line)
        }
        other => panic!("linewise 選択のはず: {other:?}"),
    };
    assert_eq!(top_after_downs, 57, "選択中でも窓は 3 ページ分動くはず");

    app.preview_page(-1);
    match app.preview_selection() {
        PreviewSelection::Line { lo, hi } => {
            assert_eq!(lo, 0, "アンカーは固定のまま");
            assert_ne!(hi, hi_after_downs, "上ページ送りで選択端が縮まない");
            assert_eq!(hi, 38, "1 ページ分だけ選択が縮むはず");
        }
        other => panic!("linewise 選択のはず: {other:?}"),
    }
    assert_ne!(
        app.tab.preview_top_line, top_after_downs,
        "選択中でも、下ページ送り直後の最初の上ページ送りで窓が動かない(バグの再現)"
    );
    assert_eq!(app.tab.preview_top_line, 38, "窓も 1 ページ分だけ戻るはず");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn preview_page_non_windowed_decorated_markdown_is_unaffected() {
    // #9 non-regression: decorated Markdown (not windowed) still pages via the pre-existing
    // `preview_scroll` path — the windowed fix must not touch it.
    let dir = unique_tmp("konoma_wpage_md_nonregression");
    std::fs::create_dir_all(&dir).unwrap();
    let mut src = String::from("# Title\n\n");
    for i in 0..200 {
        src.push_str(&format!("filler line {i}\n\n"));
    }
    std::fs::write(dir.join("doc.md"), &src).unwrap();

    let mut app = App::new(dir.canonicalize().unwrap(), Config::default()).unwrap();
    let md = dir.canonicalize().unwrap().join("doc.md");
    app.tab.preview_kind = Some(app.cfg.resolve_preview(&md));
    app.tab.preview_path = Some(md);
    app.tab.mode = Mode::Preview;
    app.tab.preview_viewport = 10;
    let _ = app.md_layout(80); // populate the decoration cache (same path as the real render)
    assert!(!app.is_windowed(), "装飾 Markdown は windowed ではない");

    app.preview_page(1); // +9 (overlaps by 1 line)
    assert_eq!(app.tab.preview_scroll, 9);
    app.preview_half_page(1); // +5
    assert_eq!(app.tab.preview_scroll, 14);
    app.preview_page(-1); // -9
    assert_eq!(app.tab.preview_scroll, 5);
    app.preview_half_page(-1); // -5
    assert_eq!(
        app.tab.preview_scroll, 0,
        "非 windowed は preview_scroll に委譲されたまま"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn preview_hscroll_moves_and_home_end() {
    let dir = std::env::temp_dir().join("konoma_preview_hscroll_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.preview_hscroll(8);
    assert_eq!(app.tab.preview_hscroll, 8);
    app.preview_hscroll(-100);
    assert_eq!(app.tab.preview_hscroll, 0, "下限 0");
    app.preview_hscroll_end();
    assert_eq!(
        app.tab.preview_hscroll,
        u16::MAX,
        "$ は上限(描画側でクランプ)"
    );
    app.preview_hscroll_home();
    assert_eq!(app.tab.preview_hscroll, 0, "0 で行頭");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn windowed_text_preview_reads_window_and_scrolls_lines() {
    let dir = std::env::temp_dir().join("konoma_windowed_lines_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut content = String::new();
    for i in 0..200 {
        content.push_str(&format!("line {i}\n"));
    }
    std::fs::write(dir.join("big.txt"), content.as_bytes()).unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.rebuild_tree().unwrap();
    let i = app
        .tab
        .entries
        .iter()
        .position(|e| e.path.ends_with("big.txt"))
        .unwrap();
    app.tab.selected = i;
    app.tree_activate().unwrap(); // file → preview (Text = windowed read)
    assert!(app.is_windowed(), "テキストは窓読みモード");
    app.tab.preview_viewport = 10;

    let first_line = |a: &mut App| -> String {
        a.windowed_lines(10, 80)[0]
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect()
    };
    assert!(first_line(&mut app).contains("line 0"), "先頭は line 0");

    // The always-a-cursor model: moving the line cursor within the window (vh=10) doesn't move
    // the window until it reaches the edge.
    assert_eq!(app.tab.preview_byte_top, 0);
    app.preview_scroll(5); // cursor 0→5 (still within the visible range)
    assert_eq!(app.tab.preview_cursor_line, 5, "カーソルは 5 行目");
    assert_eq!(app.tab.preview_byte_top, 0, "窓内移動では窓は動かない");
    assert!(
        first_line(&mut app).contains("line 0"),
        "先頭は line 0 のまま"
    );

    // Once the cursor passes the bottom edge, the window follows (byte_top advances).
    app.preview_scroll(5); // cursor 5→10 (passes the bottom edge vh=10)
    assert_eq!(app.tab.preview_cursor_line, 10, "カーソルは 10 行目");
    assert!(
        app.tab.preview_byte_top > 0,
        "下端超えで窓が追従(byte_top 前進)"
    );
    assert!(
        first_line(&mut app).contains("line 1"),
        "line 10 が見えるよう先頭は line 1"
    );

    // windowed's preview_to_top sends the cursor and the window to the top.
    app.preview_to_top();
    assert_eq!(app.tab.preview_cursor_line, 0);
    assert_eq!(app.tab.preview_byte_top, 0);
    assert_eq!(app.tab.preview_top_line, 0);
    assert!(
        first_line(&mut app).contains("line 0"),
        "to_top で先頭へ戻る"
    );

    // To the end: the cursor goes to the last line (199), and the window shows the last page.
    app.preview_to_bottom();
    assert_eq!(app.tab.preview_cursor_line, 199, "to_bottom で最終行へ");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn request_edit_opens_at_windowed_caret_line() {
    // `e` opens the editor at the caret line for a windowed preview (text/code) (1-indexed).
    // Tree edits and non-windowed (decorated Markdown) get no line (opens at the top).
    let dir = std::env::temp_dir().join("konoma_edit_line_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut content = String::new();
    for i in 0..200 {
        content.push_str(&format!("line {i}\n"));
    }
    std::fs::write(dir.join("big.txt"), content.as_bytes()).unwrap();
    std::fs::write(dir.join("doc.md"), "# Title\n\nsome text\n").unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.rebuild_tree().unwrap();

    // (1) Editing from the tree: no line.
    let i = app
        .tab
        .entries
        .iter()
        .position(|e| e.path.ends_with("big.txt"))
        .unwrap();
    app.tab.selected = i;
    app.request_edit();
    let (p, line) = app.take_pending_edit().expect("edit requested");
    assert!(p.ends_with("big.txt"));
    assert_eq!(line, None, "ツリー編集は行を渡さない");

    // (2) A windowed preview: caret line + 1.
    app.tree_activate().unwrap();
    assert!(app.is_windowed());
    app.tab.preview_viewport = 10;
    app.preview_scroll(12); // caret 0→12
    assert_eq!(app.tab.preview_cursor_line, 12);
    app.request_edit();
    let (p, line) = app.take_pending_edit().expect("edit requested");
    assert!(p.ends_with("big.txt"));
    assert_eq!(
        line,
        Some(13),
        "キャレット 12 行目 → エディタは 13 行目(1始まり)"
    );

    // (3) Decorated Markdown (non-windowed): before rendering (md_cache not yet built), no
    // line = opens at the top.
    app.back_to_tree();
    let j = app
        .tab
        .entries
        .iter()
        .position(|e| e.path.ends_with("doc.md"))
        .unwrap();
    app.tab.selected = j;
    app.tree_activate().unwrap();
    assert!(!app.is_windowed(), "装飾 Markdown は非 windowed");
    app.request_edit();
    let (p, line) = app.take_pending_edit().expect("edit requested");
    assert!(p.ends_with("doc.md"));
    assert_eq!(line, None, "描画前(cache 未構築)は行なし=先頭");

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn preview_visual_selection_copies_logical_lines() {
    let dir = std::env::temp_dir().join("konoma_preview_visual_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let content = (0..20)
        .map(|i| format!("line {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(dir.join("code.txt"), content.as_bytes()).unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.rebuild_tree().unwrap();
    let i = app
        .tab
        .entries
        .iter()
        .position(|e| e.path.ends_with("code.txt"))
        .unwrap();
    app.tab.selected = i;
    app.tree_activate().unwrap();
    assert!(app.is_windowed(), "テキストは窓読み(行カーソル有効)");
    app.tab.preview_viewport = 10;

    // Move the cursor to line 2. Not selected yet.
    app.preview_scroll(2);
    assert_eq!(app.tab.preview_cursor_line, 2);
    assert!(!app.is_preview_visual(), "まだ選択していない");
    assert_eq!(app.surface(), crate::keymap::Surface::PreviewText);

    // V starts line selection → extend it 2 lines down (2..=4).
    app.preview_enter_visual(true);
    assert!(app.is_preview_visual(), "選択モードに入る");
    assert!(app.preview_visual_linewise(), "V は行選択");
    assert_eq!(app.surface(), crate::keymap::Surface::PreviewTextVisual);
    assert_eq!(
        app.internal_mode(),
        Some(crate::app::InternalMode::PreviewVisual)
    );
    app.preview_scroll(2); // cursor 2→4 (anchor=2 fixed)
    assert_eq!(
        app.preview_selection(),
        crate::app::PreviewSelection::Line { lo: 2, hi: 4 }
    );

    // Selected text = logical lines 2..=4.
    assert_eq!(app.preview_selection_text(), "line 2\nline 3\nline 4");

    // Executing copy clears the selection (verifies the state transition, independent of
    // clipboard availability).
    app.preview_copy_selection();
    assert!(!app.is_preview_visual(), "コピー後は選択解除");

    // Selecting the other direction (upward) also normalizes the range in ascending order.
    app.preview_to_top(); // cursor 0
    app.preview_scroll(5); // cursor 5
    app.preview_enter_visual(true);
    app.preview_scroll(-3); // cursor 2, anchor=5 → range 2..=5
    assert_eq!(
        app.preview_selection(),
        crate::app::PreviewSelection::Line { lo: 2, hi: 5 }
    );
    assert_eq!(
        app.preview_selection_text(),
        "line 2\nline 3\nline 4\nline 5"
    );

    // Equivalent to Esc (exit) clears it.
    app.preview_exit_visual();
    assert!(!app.is_preview_visual());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn preview_charwise_selection_copies_character_range() {
    let dir = std::env::temp_dir().join("konoma_preview_charwise_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // 6 lines, each "abcdefghij".
    let content = (0..6)
        .map(|_| "abcdefghij".to_string())
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(dir.join("code.txt"), content.as_bytes()).unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.rebuild_tree().unwrap();
    let i = app
        .tab
        .entries
        .iter()
        .position(|e| e.path.ends_with("code.txt"))
        .unwrap();
    app.tab.selected = i;
    app.tree_activate().unwrap();
    assert!(app.is_windowed());
    app.tab.preview_viewport = 10;

    // Start at column 2 (c) on line 1 (line 0) → 3 presses of l move col 2→5 (f). Single-line charwise.
    app.preview_col_move(1); // col 1
    app.preview_col_move(1); // col 2
    app.preview_enter_visual(false); // charwise, anchor=(0,2)
    assert!(app.is_preview_visual());
    assert!(!app.preview_visual_linewise(), "v は文字選択");
    app.preview_col_move(1); // 3
    app.preview_col_move(1); // 4
    app.preview_col_move(1); // 5
    assert_eq!(
        app.preview_selection(),
        crate::app::PreviewSelection::Char {
            start: (0, 2),
            end: (0, 5)
        }
    );
    // Character range [2..=5] = "cdef" (end-inclusive).
    assert_eq!(app.preview_selection_text(), "cdef");

    // Multi-line charwise: from (0,2), 2 lines down and col 3 → "cdefghij\nabcdefghij\nabcd".
    app.preview_scroll(2); // cursor line 0→2
    app.preview_col_home(); // col 0
    app.preview_col_move(1); // 1
    app.preview_col_move(1); // 2
    app.preview_col_move(1); // 3
    assert_eq!(
        app.preview_selection(),
        crate::app::PreviewSelection::Char {
            start: (0, 2),
            end: (2, 3)
        }
    );
    assert_eq!(app.preview_selection_text(), "cdefghij\nabcdefghij\nabcd");

    // The other direction also normalizes to (line,col) ascending order.
    app.preview_exit_visual();
    app.preview_to_top();
    app.preview_scroll(3); // line 3
    app.preview_col_home();
    app.preview_col_move(1); // col 1
    app.preview_enter_visual(false); // anchor=(3,1)
    app.preview_scroll(-1); // line 2
    app.preview_col_move(1); // col 2
    assert_eq!(
        app.preview_selection(),
        crate::app::PreviewSelection::Char {
            start: (2, 2),
            end: (3, 1)
        }
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn markdown_raw_toggle_enables_windowed_selection() {
    let dir = std::env::temp_dir().join("konoma_md_raw_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("doc.md"),
        b"# Title\n\nSome **bold** text.\n\n- item one\n- item two\n",
    )
    .unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.rebuild_tree().unwrap();
    let i = app
        .tab
        .entries
        .iter()
        .position(|e| e.path.ends_with("doc.md"))
        .unwrap();
    app.tab.selected = i;
    app.tree_activate().unwrap();

    // Default is the decorated view: not windowed = 2D selection has no effect. It is, however,
    // a target for the R toggle.
    assert!(app.is_decorated_kind(), "Markdown は装飾種別");
    assert!(!app.is_windowed(), "装飾表示は windowed でない");
    assert!(!app.is_raw_source());
    assert_eq!(app.surface(), crate::keymap::Surface::PreviewText);

    // R switches to the raw source view: becomes windowed and selection can be applied.
    app.toggle_md_raw();
    assert!(app.is_raw_source(), "raw ソース表示に切替");
    assert!(app.is_windowed(), "raw は windowed(選択可)");
    app.tab.preview_viewport = 10;

    // Select the whole first line "# Title" charwise and verify the copied content (matches the
    // raw source's line/column).
    app.preview_enter_visual(false); // anchor=(0,0)
    for _ in 0..6 {
        app.preview_col_move(1); // (0,0)→(0,6) = "# Title" (7 chars, end-inclusive)
    }
    assert_eq!(app.preview_selection_text(), "# Title");

    // R switches back to the decorated view: unwindowed and selection cleared.
    app.preview_exit_visual();
    app.toggle_md_raw();
    assert!(!app.is_raw_source(), "装飾表示へ戻る");
    assert!(!app.is_windowed());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn dialog_cursor_right_and_end_clamp() {
    let dir = std::env::temp_dir().join("konoma_dialog_cursor_re_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.txt"), b"x").unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.rebuild_tree().unwrap();
    app.start_create();
    for c in "abc".chars() {
        app.dialog_input_push(c);
    }
    // Verify end → home → right (one to the right) → end (end). Observe the position via a
    // mid-string insert.
    app.dialog_cursor_home();
    app.dialog_cursor_right(); // 1
    app.dialog_input_push('X'); // "aXbc"
    assert_eq!(
        app.dialog_view().map(|v| v.2.to_string()),
        Some("aXbc".into())
    );
    app.dialog_cursor_right(); // right, staying within the end (repeated to check clamping)
    for _ in 0..10 {
        app.dialog_cursor_right();
    }
    app.dialog_input_push('Z'); // appends at the end → "aXbcZ"
    assert_eq!(
        app.dialog_view().map(|v| v.2.to_string()),
        Some("aXbcZ".into())
    );
    // end goes to the tail. home→end→insert also lands at the end.
    app.dialog_cursor_home();
    app.dialog_cursor_end();
    app.dialog_input_push('!'); // "aXbcZ!"
    assert_eq!(
        app.dialog_view().map(|v| v.2.to_string()),
        Some("aXbcZ!".into())
    );
    app.dialog_cancel();
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn set_gif_frames_and_advance_when_due() {
    let mut app = app_with_image();
    let frame = |c: [u8; 3]| {
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(8, 8, image::Rgb(c)))
    };
    // Only 1 frame: not treated as an animation, and advance is false.
    app.set_gif_frames(vec![(
        frame([1, 2, 3]),
        std::time::Duration::from_millis(50),
    )]);
    assert!(!app.is_gif_active());
    assert!(!app.advance_gif_if_due(), "単一フレームは進めない");

    // 2 frames: the first tick only starts the timer (doesn't advance).
    app.set_gif_frames(vec![
        (frame([255, 0, 0]), std::time::Duration::from_millis(50)),
        (frame([0, 255, 0]), std::time::Duration::from_millis(50)),
    ]);
    assert!(app.is_gif_active());
    assert!(!app.advance_gif_if_due(), "最初の tick は計時開始だけ");
    assert_eq!(app.gif_idx, 0);
    // Pretend the display duration has elapsed and advance to the next frame.
    app.gif_shown_at = Some(std::time::Instant::now() - std::time::Duration::from_secs(1));
    assert!(app.advance_gif_if_due(), "期限超過で次フレームへ");
    assert_eq!(app.gif_idx, 1);
}

#[test]
fn gif_poll_timeout_none_without_anim_some_when_playing() {
    use std::time::Duration;
    let mut app = app_with_image();
    assert!(app.gif_poll_timeout().is_none(), "アニメでなければ None");
    let frame =
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(4, 4, image::Rgb([0; 3])));
    app.gif_frames = vec![
        (frame.clone(), Duration::from_millis(50)),
        (frame, Duration::from_millis(50)),
    ];
    let t = app.gif_poll_timeout().expect("再生中は Some");
    assert!(
        t >= Duration::from_millis(10) && t <= Duration::from_millis(100),
        "10〜100ms にクランプ: {t:?}"
    );
}

// ---- Inline Markdown GIF animation --------------------------------------------

/// An entry with 1 frame (a still image / single-frame GIF) doesn't advance and its state doesn't change either.
#[test]
fn advance_md_gifs_single_frame_entry_does_not_advance() {
    let dir = unique_tmp("konoma_md_gif_single_test");
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();

    let key = PathBuf::from("/tmp/one.gif");
    let frame = Arc::new(image::DynamicImage::new_rgba8(4, 4));
    app.md_image_cache.insert(
        key.clone(),
        MdImgEntry {
            decoded: Some(frame.clone()),
            frames: vec![(frame, std::time::Duration::from_millis(50))],
            ..Default::default()
        },
    );

    assert!(
        !app.advance_md_gifs_if_due(),
        "1フレームしか無いエントリは進めない"
    );
    let e = &app.md_image_cache[&key];
    assert_eq!(e.idx, 0);
    assert!(e.shown_at.is_none(), "アニメ対象外なので計時も始めない");

    std::fs::remove_dir_all(&dir).ok();
}

/// A multi-frame entry: doesn't advance before the deadline (the first tick only starts the
/// timer). Past the deadline it advances to the next frame, decoded is swapped in, and
/// proto_size/clip_key/zoom_key are invalidated, but **the protocol itself is not cleared**
/// (kept for display until the next encode arrives — clearing it would flash a moment of blank
/// space). Once it cycles all the way around, idx returns to 0.
#[test]
fn advance_md_gifs_advances_on_deadline_and_keeps_protocol_while_invalidating_keys() {
    let dir = unique_tmp("konoma_md_gif_advance_test");
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();

    let key = PathBuf::from("/tmp/anim.gif");
    let f0 = Arc::new(image::DynamicImage::new_rgba8(4, 4));
    let f1 = Arc::new(image::DynamicImage::new_rgba8(4, 4));

    // Build a real Protocol (halfblocks = no terminal needed) and verify it "survives an advance".
    let picker = test_picker();
    let proto = picker
        .new_protocol(
            (*f0).clone(),
            ratatui::layout::Size::new(4, 2),
            ratatui_image::Resize::Fit(None),
        )
        .expect("halfblocks エンコードは端末無しでも成功する");

    app.md_image_cache.insert(
        key.clone(),
        MdImgEntry {
            decoded: Some(f0.clone()),
            frames: vec![
                (f0.clone(), std::time::Duration::from_millis(50)),
                (f1.clone(), std::time::Duration::from_millis(50)),
            ],
            protocol: Some(proto),
            proto_size: Some((4, 2)),
            clip_key: Some((4, 2, 0, 2)),
            zoom_key: Some((4, 2, (0, 0, 4, 4))),
            ..Default::default()
        },
    );

    // Before the deadline (the first tick): only starts the timer, doesn't advance.
    assert!(!app.advance_md_gifs_if_due(), "最初の tick は計時開始だけ");
    {
        let e = &app.md_image_cache[&key];
        assert_eq!(e.idx, 0);
        assert!(e.shown_at.is_some(), "計時は始まる");
        assert!(
            e.proto_size.is_some(),
            "計時開始だけではエンコードキーを崩さない"
        );
    }

    // Shift the deadline into the past to advance to the next frame.
    {
        let e = app.md_image_cache.get_mut(&key).unwrap();
        e.shown_at = Some(std::time::Instant::now() - std::time::Duration::from_secs(1));
    }
    assert!(app.advance_md_gifs_if_due(), "期限超過で次フレームへ進む");
    {
        let e = &app.md_image_cache[&key];
        assert_eq!(e.idx, 1, "次フレームへ進む");
        assert!(
            Arc::ptr_eq(e.decoded.as_ref().unwrap(), &f1),
            "decoded が新フレームへ差し替わる"
        );
        assert!(e.proto_size.is_none(), "proto_size は無効化される");
        assert!(e.clip_key.is_none(), "clip_key も無効化される");
        assert!(e.zoom_key.is_none(), "zoom_key も無効化される");
        assert!(
            e.protocol.is_some(),
            "旧 protocol 本体は消さない(次のエンコードが届くまでの表示用)"
        );
    }

    // Full cycle: shifting the deadline into the past once more returns idx to 0.
    {
        let e = app.md_image_cache.get_mut(&key).unwrap();
        e.shown_at = Some(std::time::Instant::now() - std::time::Duration::from_secs(1));
    }
    assert!(app.advance_md_gifs_if_due());
    assert_eq!(
        app.md_image_cache[&key].idx, 0,
        "2フレームを一周して先頭へ戻る"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// None with no animation target; Some, bounded by the frames' delay (clamped to 10-100ms), otherwise.
#[test]
fn md_gif_poll_timeout_none_without_anim_some_when_playing() {
    use std::time::Duration;
    let dir = unique_tmp("konoma_md_gif_poll_test");
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();

    assert!(
        app.md_gif_poll_timeout().is_none(),
        "アニメ中のインライン GIF が無ければ None"
    );

    let key = PathBuf::from("/tmp/anim2.gif");
    let frame = Arc::new(image::DynamicImage::new_rgba8(4, 4));
    app.md_image_cache.insert(
        key,
        MdImgEntry {
            decoded: Some(frame.clone()),
            frames: vec![
                (frame.clone(), Duration::from_millis(50)),
                (frame, Duration::from_millis(50)),
            ],
            ..Default::default()
        },
    );
    let t = app.md_gif_poll_timeout().expect("再生中は Some");
    assert!(
        t >= Duration::from_millis(10) && t <= Duration::from_millis(100),
        "10〜100ms にクランプ: {t:?}"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn apply_image_resize_err_is_ignored() {
    // An encode failure (Err) doesn't crash and returns false (state is unchanged too).
    let mut app = app_with_image();
    let r = app.apply_image_resize(Err(ratatui_image::errors::Errors::NoCap));
    assert!(!r, "Err は反映しない");
}

#[test]
fn attach_and_detach_image_backend_set_and_clear_state() {
    let dir = std::env::temp_dir().join("konoma_attach_detach_img_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    assert!(app.picker.is_none() && app.img_tx.is_none());
    let picker = ratatui_image::picker::Picker::halfblocks();
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<ratatui_image::thread::ResizeRequest>();
    Box::leak(Box::new(rx));
    app.attach_image_backend(picker, tx);
    assert!(
        app.picker.is_some() && app.img_tx.is_some(),
        "attach で両方載る"
    );
    app.image_src = Some(std::sync::Arc::new(image::DynamicImage::new_rgb8(4, 4)));
    app.detach_image_backend();
    assert!(app.img_tx.is_none(), "detach で tx を落とす(ワーカー終了)");
    assert!(app.image_src.is_none(), "detach で画像状態も解放");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn load_image_decodes_with_backend_and_noops_without() {
    let dir = std::env::temp_dir().join("konoma_load_image_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let png = dir.join("tiny.png");
    write_png(&png, 20, 10);

    // No backend: nothing is loaded (the render side falls back to text).
    let mut bare = App::new(dir.clone(), Config::default()).unwrap();
    bare.load_image(&png);
    assert!(bare.image_src.is_none(), "picker/tx 無しでは load しない");

    // With a backend: decode_static → loaded into image_src (size matches).
    let mut app = app_with_image();
    app.image_src = None; // first clear app_with_image's dummy
    app.load_image(&png);
    let img = app.image_src.as_ref().expect("PNG をデコードして載せる");
    assert_eq!((img.width(), img.height()), (20, 10));
    // A non-image file isn't loaded (image_src stays None).
    app.image_src = None;
    std::fs::write(dir.join("notimg.png"), b"this is not a png").unwrap();
    app.load_image(&dir.join("notimg.png"));
    assert!(app.image_src.is_none(), "デコード失敗時は載せない");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn open_link_target_handles_anchor_missing_file_and_dir() {
    let base = std::env::temp_dir().join("konoma_open_link_target_test");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("sub")).unwrap();
    std::fs::write(base.join("a.md"), b"# doc").unwrap();
    std::fs::write(base.join("b.txt"), b"hello").unwrap();
    let base = base.canonicalize().unwrap();
    let mut app = App::new(base.clone(), Config::default()).unwrap();
    app.rebuild_tree().unwrap();

    // Pretend an md is open, with a.md as the relative link base.
    app.tab.preview_path = Some(base.join("a.md"));
    app.tab.preview_kind = Some(PreviewKind::Markdown(base.join("a.md")));
    app.tab.mode = Mode::Preview;

    // For an anchor (#), if there's no matching heading, flash a "not found" notice (this isn't
    // "unsupported"). Here md_cache hasn't been built (nothing has rendered), so anchors is
    // empty → treated as NotFound.
    app.open_link_target("#section").unwrap();
    assert!(
        app.flash
            .as_deref()
            .unwrap()
            .contains(tr(app.lang, crate::i18n::Msg::AnchorNotFound)),
        "アンカー未一致は見出し無し通知: {:?}",
        app.flash
    );

    // A local path that doesn't exist gets a NotFound notice.
    app.tab.preview_path = Some(base.join("a.md"));
    app.open_link_target("does_not_exist.zz").unwrap();
    assert!(
        app.flash
            .as_deref()
            .unwrap()
            .contains(tr(app.lang, crate::i18n::Msg::NotFound)),
        "見つからない: {:?}",
        app.flash
    );

    // A link to an existing file → the preview target switches to b.txt.
    app.tab.preview_path = Some(base.join("a.md"));
    app.open_link_target("b.txt").unwrap();
    assert_eq!(
        app.tab
            .preview_path
            .as_ref()
            .map(|p| p.file_name().unwrap().to_owned()),
        Some(std::ffi::OsString::from("b.txt")),
        "ファイルリンクで b.txt をプレビュー"
    );

    // A link to a directory → root becomes sub and it goes to Tree.
    app.tab.preview_path = Some(base.join("a.md"));
    app.tab.mode = Mode::Preview;
    app.open_link_target("sub").unwrap();
    assert_eq!(
        app.tab.root,
        base.join("sub"),
        "ディレクトリリンクで root=sub"
    );
    assert_eq!(app.tab.mode, Mode::Tree, "ディレクトリは Tree へ戻る");
    std::fs::remove_dir_all(&base).ok();
}

#[test]
fn copy_target_follows_mode() {
    let dir = std::env::temp_dir().join("konoma_copy_target_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.txt"), b"x").unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.rebuild_tree().unwrap();
    let i = app
        .tab
        .entries
        .iter()
        .position(|e| e.path.ends_with("a.txt"))
        .unwrap();
    app.tab.selected = i;
    // Tree: the cursor's entry.
    assert_eq!(app.copy_target(), Some(dir.join("a.txt")));
    // Preview: the preview target.
    app.tab.mode = Mode::Preview;
    app.tab.preview_path = Some(dir.join("p.txt"));
    assert_eq!(app.copy_target(), Some(dir.join("p.txt")));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn tab_label_shows_root_dir_or_preview_file_name() {
    let dir = std::env::temp_dir()
        .join("konoma_tab_label_name_test")
        .join("proj");
    let _ = std::fs::remove_dir_all(dir.parent().unwrap());
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    let active = app.active_tab_index();
    // While showing Tree, it's the root's directory name.
    assert_eq!(app.tab_label(active), "proj");
    // Out of range is an empty string.
    assert_eq!(app.tab_label(999), "");
    // While in Preview, it's the file name of the preview target.
    app.tab.mode = Mode::Preview;
    app.tab.preview_path = Some(dir.join("file.md"));
    assert_eq!(app.tab_label(active), "file.md");
    std::fs::remove_dir_all(dir.parent().unwrap()).ok();
}

// --- git-only: fetching commit meta / copy / graph and detail scroll / the picker --------

#[cfg(feature = "git")]
fn git_repo_with_commits(dir: &Path) {
    init_git_repo(dir);
    let sh = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .current_dir(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?} 失敗");
    };
    std::fs::write(dir.join("a.txt"), b"one\n").unwrap();
    sh(&["add", "-A"]);
    sh(&["commit", "-m", "first commit\n\nbody line"]);
    std::fs::write(dir.join("a.txt"), b"one\ntwo\n").unwrap();
    sh(&["add", "-A"]);
    sh(&["commit", "-m", "second commit"]);
}

#[cfg(feature = "git")]
#[test]
fn current_commit_meta_resolves_per_surface_and_none_in_tree() {
    let dir = std::env::temp_dir().join("konoma_current_commit_meta_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    git_repo_with_commits(&dir);
    let canon = dir.canonicalize().unwrap();
    let mut app = App::new(canon.clone(), Config::default()).unwrap();

    // No commit on the Tree surface → None.
    assert!(
        app.current_commit_meta().is_none(),
        "Tree ではコミットメタ無し"
    );

    // The log surface: returns the meta of the selected commit (the first one = second).
    app.open_git_view();
    app.open_git_log();
    let meta = app.current_commit_meta().expect("log の選択コミットメタ");
    assert!(
        meta.message.starts_with("second commit"),
        "件名: {:?}",
        meta.message
    );
    assert_eq!(
        meta.short.len(),
        meta.id.chars().take(meta.short.len()).count()
    );
    assert!(
        !meta.id.is_empty() && meta.id.starts_with(&meta.short),
        "完全ハッシュは短縮から始まる"
    );

    // The detail surface: uses the already-loaded git_detail_meta (open the older one = first commit).
    app.git_log_move(1);
    app.open_git_commit_detail();
    let dmeta = app.current_commit_meta().expect("detail のメタ");
    assert!(
        dmeta.message.starts_with("first commit"),
        "詳細は first: {:?}",
        dmeta.message
    );
    assert!(
        dmeta.message.contains("body line"),
        "完全メッセージ(本文)も含む"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[cfg(feature = "git")]
#[test]
fn git_copy_message_and_branch_name_set_flash() {
    let dir = std::env::temp_dir().join("konoma_git_copy_flash_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    git_repo_with_commits(&dir);
    let canon = dir.canonicalize().unwrap();
    let mut app = App::new(canon, Config::default()).unwrap();
    let copied = tr(app.lang, crate::i18n::Msg::CopiedPrefix);
    let failed = tr(app.lang, crate::i18n::Msg::CopyFailed);
    let no_target = tr(app.lang, crate::i18n::Msg::NoCopyTarget);

    // Copy the log's selected commit via Message: flashes either copied/failed regardless of
    // clipboard availability.
    app.open_git_view();
    app.open_git_log();
    app.git_copy(GitCopyKind::Message);
    let f = app.flash.clone().expect("コピーで flash");
    assert!(
        f.starts_with(copied) || f.starts_with(failed),
        "copied/failed のどちらかを通知: {f:?}"
    );
    assert_ne!(f, no_target, "コミットがあるので no-target ではない");

    // git_copy_branch_name with branches unloaded → hits the no-target guard.
    app.close_git_log();
    app.close_git_view();
    app.tab.git_branches = None;
    app.git_copy_branch_name();
    assert_eq!(
        app.flash.as_deref(),
        Some(no_target),
        "ブランチ未選択は no-target"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[cfg(feature = "git")]
#[test]
fn git_copy_branch_name_copies_current_branch() {
    let dir = std::env::temp_dir().join("konoma_git_copy_branch_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    git_repo_with_commits(&dir);
    let canon = dir.canonicalize().unwrap();
    let mut app = App::new(canon, Config::default()).unwrap();
    app.open_git_branches();
    let branch = app.git_branch_view()[app.git_branch_sel()].name.clone();
    let no_target = tr(app.lang, crate::i18n::Msg::NoCopyTarget);
    app.git_copy_branch_name();
    let f = app.flash.clone().expect("flash");
    // If the clipboard succeeds, "copied: <branch>"; even on failure, "copy failed: ...". Either
    // way it's already past the guard.
    assert_ne!(
        f, no_target,
        "選択ブランチがあるので no-target ではない: {f:?}"
    );
    assert!(!branch.is_empty(), "現在ブランチ名が取れる");
    std::fs::remove_dir_all(&dir).ok();
}

#[cfg(feature = "git")]
#[test]
fn git_view_unstage_single_file_reloads() {
    let dir = std::env::temp_dir().join("konoma_git_view_unstage_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    init_git_repo(&dir);
    let sh = |args: &[&str]| {
        std::process::Command::new("git")
            .current_dir(&dir)
            .args(args)
            .output()
            .unwrap();
    };
    std::fs::write(dir.join("a.txt"), b"v1\n").unwrap();
    sh(&["add", "-A"]);
    sh(&["commit", "-m", "init"]);
    std::fs::write(dir.join("a.txt"), b"v2\n").unwrap();
    let canon = dir.canonicalize().unwrap();
    let mut app = App::new(canon, Config::default()).unwrap();
    app.open_git_view();
    app.tab.git_view_sel = 0;
    let target = app.git_view_selected().unwrap();
    app.git_view_stage();
    assert!(
        app.git_view_entries()
            .iter()
            .find(|e| e.path == target)
            .unwrap()
            .staged
    );
    // u = unstage a single file.
    app.git_view_unstage();
    assert!(
        !app.git_view_entries()
            .iter()
            .find(|e| e.path == target)
            .unwrap()
            .staged,
        "unstage で staged=false に戻る"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[cfg(feature = "git")]
#[test]
fn git_view_start_discard_opens_confirm_without_destroying() {
    let dir = std::env::temp_dir().join("konoma_git_view_start_discard_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    init_git_repo(&dir);
    let sh = |args: &[&str]| {
        std::process::Command::new("git")
            .current_dir(&dir)
            .args(args)
            .output()
            .unwrap();
    };
    std::fs::write(dir.join("a.txt"), b"v1\n").unwrap();
    sh(&["add", "-A"]);
    sh(&["commit", "-m", "init"]);
    std::fs::write(dir.join("a.txt"), b"v2\n").unwrap();
    let canon = dir.canonicalize().unwrap();
    let mut app = App::new(canon, Config::default()).unwrap();
    app.open_git_view();
    app.git_view_start_discard();
    // Only the confirmation dialog opens (nothing is discarded yet = the file stays at v2).
    assert!(
        app.is_dialog() && app.dialog_is_confirm(),
        "破棄は確認ダイアログ"
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("a.txt")).unwrap(),
        "v2\n",
        "確認段階では破棄しない"
    );
    app.dialog_cancel();
    std::fs::remove_dir_all(&dir).ok();
}

#[cfg(feature = "git")]
#[test]
fn git_branch_filter_commit_and_backspace() {
    use std::process::Command;
    let dir = std::env::temp_dir().join("konoma_branch_filter_commit_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let git = |args: &[&str]| {
        Command::new("git")
            .current_dir(&dir)
            .args(args)
            .output()
            .unwrap();
    };
    git(&["init", "-q"]);
    git(&["config", "user.email", "t@t.t"]);
    git(&["config", "user.name", "t"]);
    std::fs::write(dir.join("a.txt"), b"x").unwrap();
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "init"]);
    git(&["branch", "feature"]);
    git(&["branch", "fixup"]);
    let mut app = App::new(dir.canonicalize().unwrap(), Config::default()).unwrap();
    app.open_git_branches();
    app.git_branch_start_filter();
    // "f" narrows it to the 2 entries feature/fixup, "fi" narrows it to the 1 entry fixup
    // (substring match).
    app.git_branch_filter_push('f');
    assert_eq!(app.git_branch_view().len(), 2, "f で feature/fixup の2件");
    app.git_branch_filter_push('i');
    assert_eq!(app.git_branch_view().len(), 1, "fi で fixup の1件に絞る");
    // Backspacing back to "f" widens it to 2 entries again (backspace's effect).
    app.git_branch_filter_backspace();
    assert_eq!(app.git_branch_view().len(), 2, "backspace で2件へ広がる");
    // commit (Enter): exits input mode but the query remains (j/k can navigate).
    assert!(app.git_branch_filtering());
    app.git_branch_filter_commit();
    assert!(!app.git_branch_filtering(), "Enter で入力モードを抜ける");
    assert_eq!(app.git_branch_query(), "f", "クエリは保持される");
    std::fs::remove_dir_all(&dir).ok();
}

#[cfg(feature = "git")]
#[test]
fn graph_base_set_clear_and_detail_scroll_hscroll() {
    let dir = std::env::temp_dir().join("konoma_graph_base_detail_scroll_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    git_repo_with_commits(&dir);
    let canon = dir.canonicalize().unwrap();
    let mut app = App::new(canon, Config::default()).unwrap();
    app.open_git_graph();
    assert!(app.is_git_graph());

    // Set the base: pins the selected commit row as the base → gets a label.
    assert!(app.git_graph_base_label().is_none(), "初期は基準なし");
    app.git_graph_set_base();
    assert!(
        app.git_graph_base_label().is_some(),
        "set_base で基準ラベルが付く"
    );
    // Clearing removes it.
    app.git_graph_clear_base();
    assert!(app.git_graph_base_label().is_none(), "clear_base で消える");

    // Open the commit detail and verify the scroll behavior.
    app.open_git_graph_detail();
    assert!(app.is_git_detail());
    app.set_git_detail_viewport(5);
    app.set_git_detail_total(100);
    assert_eq!(app.git_detail_scroll(), 0);
    app.git_detail_scroll_by(10);
    assert_eq!(app.git_detail_scroll(), 10, "下へ10");
    app.git_detail_scroll_by(-3);
    assert_eq!(app.git_detail_scroll(), 7, "上へ3");
    app.git_detail_scroll_to(true); // G: to the end (total-viewport=95)
    assert_eq!(app.git_detail_scroll(), 95, "G で末尾(クランプ)");
    app.git_detail_scroll_to(false); // g: to the top
    assert_eq!(app.git_detail_scroll(), 0);
    // Horizontal scroll.
    app.git_detail_hscroll_by(4);
    assert_eq!(app.git_detail_hscroll(), 4);
    app.git_detail_hscroll_home();
    assert_eq!(app.git_detail_hscroll(), 0);
    app.git_detail_hscroll_end();
    assert_eq!(app.git_detail_hscroll(), u16::MAX);
    app.clamp_git_detail_hscroll(12);
    assert_eq!(app.git_detail_hscroll(), 12, "描画側の最大幅でクランプ");
    std::fs::remove_dir_all(&dir).ok();
}

#[cfg(feature = "git")]
#[test]
fn graph_picker_move_jump_toggle_current_only_and_cancel() {
    use std::process::Command;
    let dir = std::env::temp_dir().join("konoma_graph_picker_ops_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let git = |args: &[&str]| {
        Command::new("git")
            .current_dir(&dir)
            .args(args)
            .output()
            .unwrap();
    };
    init_git_repo(&dir);
    std::fs::write(dir.join("a.txt"), b"a\n").unwrap();
    git(&["add", "-A"]);
    git(&["commit", "-q", "-m", "init"]);
    git(&["branch", "-M", "trunk"]);
    for n in ["f1", "f2", "f3"] {
        git(&["checkout", "-q", "trunk"]);
        git(&["checkout", "-q", "-b", n]);
        std::fs::write(dir.join(format!("{n}.txt")), b"x\n").unwrap();
        git(&["add", "-A"]);
        git(&["commit", "-q", "-m", &format!("{n} c")]);
    }
    git(&["checkout", "-q", "trunk"]);
    let mut cfg = Config::default();
    cfg.ui.graph_max_branches = 10;
    let mut app = App::new(dir.canonicalize().unwrap(), cfg).unwrap();
    app.open_git_graph();
    app.git_graph_open_picker();
    assert!(app.is_git_graph_picker());
    let n = app.git_graph_picker_items().len();
    assert!(n >= 4, "4ブランチ以上");

    // Move + jump.
    app.git_graph_picker_move(2);
    assert_eq!(app.git_graph_picker_sel(), 2);
    app.git_graph_picker_jump(false);
    assert_eq!(app.git_graph_picker_sel(), n - 1, "末尾へジャンプ");
    app.git_graph_picker_jump(true);
    assert_eq!(app.git_graph_picker_sel(), 0, "先頭へジャンプ");

    // current_only → only HEAD (trunk) is ON.
    app.git_graph_picker_current_only();
    let on: Vec<String> = app
        .git_graph_picker_items()
        .into_iter()
        .filter(|(_, _, on)| *on)
        .map(|(n, _, _)| n)
        .collect();
    assert_eq!(
        on,
        vec!["trunk".to_string()],
        "current only は HEAD だけ: {on:?}"
    );

    // Toggle a non-HEAD row ON.
    let non_head = app
        .git_graph_picker_items()
        .iter()
        .position(|(_, is_cur, _)| !*is_cur)
        .unwrap();
    app.git_graph_picker_move(non_head as i32 - app.git_graph_picker_sel() as i32);
    app.git_graph_picker_toggle();
    assert!(
        app.git_graph_picker_items()[non_head].2,
        "非 HEAD はトグルで ON"
    );

    // cancel closes it without changing the visible set.
    app.git_graph_picker_cancel();
    assert!(!app.is_git_graph_picker(), "cancel でパネルが閉じる");
    std::fs::remove_dir_all(&dir).ok();
}

#[cfg(feature = "git")]
#[test]
fn close_git_graph_returns_to_git_view() {
    let dir = std::env::temp_dir().join("konoma_close_git_graph_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    git_repo_with_commits(&dir);
    let canon = dir.canonicalize().unwrap();
    let mut app = App::new(canon, Config::default()).unwrap();
    app.open_git_graph();
    assert!(app.is_git_graph());
    app.close_git_graph();
    assert!(!app.is_git_graph(), "グラフは閉じる");
    assert!(app.is_git_view(), "閉じると変更ハブへ戻る");
    std::fs::remove_dir_all(&dir).ok();
}

// --- bookmark_actions / file_actions coverage ---------------------------

#[test]
fn mark_set_state_and_cancel() {
    // Only `m`'s registration-pending state is the Mark surface (`'` opens the list immediately
    // and has no pending state).
    let dir = std::env::temp_dir().join("konoma_mark_state_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    assert!(!app.is_marking());
    app.start_mark_set();
    assert!(app.is_marking(), "m=登録待ち");
    app.cancel_mark();
    assert!(!app.is_marking(), "cancel_mark で待機解除");
    // mark_input while not waiting is ignored (no flash either).
    app.flash = None;
    app.mark_input('a');
    assert!(app.flash.is_none());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn bookmark_from_preview_registers_previewed_file() {
    // m while previewing registers "the file being displayed" (not the tree cursor).
    let root = std::env::temp_dir().join("konoma_bm_preview_test");
    let _ = std::fs::remove_dir_all(&root);
    let proj = root.join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    std::fs::write(proj.join("a.txt"), b"a").unwrap();
    std::fs::write(proj.join("b.txt"), b"b").unwrap();
    let proj = proj.canonicalize().unwrap();
    let mut app = App::new(proj.clone(), Config::default()).unwrap();
    app.bookmarks = crate::bookmarks::Bookmarks::with_base(root.join("cfgbase"), &proj);
    // Open b.txt in preview, but leave the tree cursor on a.txt (a situation where they diverge,
    // e.g. via follow/jump).
    app.enter_preview(&proj.join("b.txt"));
    app.tab.selected = app
        .tab
        .entries
        .iter()
        .position(|e| e.path.ends_with("a.txt"))
        .unwrap();
    assert_eq!(app.tab.mode, Mode::Preview);
    app.start_mark_set();
    app.mark_input('p');
    assert_eq!(
        app.bookmarks.get('p'),
        Some(proj.join("b.txt")),
        "カーソルの a.txt でなく表示中の b.txt が登録される"
    );
    assert_eq!(app.tab.mode, Mode::Preview, "登録後もプレビューのまま");
    // Back in the tree, it registers the cursor position as before.
    app.back_to_tree();
    app.start_mark_set();
    app.mark_input('q');
    assert_eq!(app.bookmarks.get('q'), Some(proj.join("a.txt")));
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn global_bookmark_display_is_absolute() {
    // Global is displayed absolute (~ shortened), local uses the existing context-relative display.
    let dir = std::env::temp_dir().join("konoma_bm_display_test");
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = App::new(dir.canonicalize().unwrap(), Config::default()).unwrap();
    app.path_style = PathStyle::Relative;
    if let Some(home) = std::env::var_os("HOME") {
        let vimrc = PathBuf::from(home).join(".vimrc");
        assert_eq!(
            app.bookmark_display_path(false, &vimrc),
            "~/.vimrc",
            "グローバル=HOME 配下は ~ 短縮の絶対表示(相対 ../../.. にしない)"
        );
        assert!(
            app.bookmark_display_path(true, &vimrc).contains(".vimrc"),
            "ローカルは format_path(文脈相対)"
        );
    }
    let outside = PathBuf::from("/opt/other/place");
    assert_eq!(
        app.bookmark_display_path(false, &outside),
        "/opt/other/place",
        "HOME 外はフル絶対のまま"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn bookmark_overwrite_confirm_opens_dialog_then_applies_or_cancels() {
    // Default (confirm_bookmark_overwrite=true): overwriting with a different path opens a
    // confirmation dialog and doesn't rewrite it immediately. y (dialog_confirm(true)) overwrites,
    // n (false) leaves it as-is.
    let root = std::env::temp_dir().join("konoma_bm_overwrite_confirm");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("a.txt"), b"a").unwrap();
    std::fs::write(root.join("b.txt"), b"b").unwrap();
    let root = root.canonicalize().unwrap();
    let mut app = App::new(root.clone(), Config::default()).unwrap();
    app.bookmarks = crate::bookmarks::Bookmarks::with_base(root.join("cfgbase"), &root);
    let a = root.join("a.txt");
    let b = root.join("b.txt");

    // First register a.txt at 'x' (no confirmation = an unused key).
    app.enter_preview(&a);
    app.start_mark_set();
    app.mark_input('x');
    assert_eq!(app.bookmarks.get('x'), Some(a.clone()));
    assert!(app.dialog.is_none(), "未使用キーは確認ダイアログ無し");

    // Trying to register b.txt at the same 'x' opens a confirmation dialog and doesn't overwrite yet.
    app.enter_preview(&b);
    app.start_mark_set();
    app.mark_input('x');
    assert!(
        app.dialog.is_some(),
        "別パスへの上書きは確認ダイアログを出す"
    );
    assert!(
        app.confirm_is_bookmark(),
        "ブックマーク確認として判定される"
    );
    assert_eq!(app.bookmarks.get('x'), Some(a.clone()), "確認前は元のまま");

    // n=cancel: unchanged.
    app.dialog_confirm(false).unwrap();
    assert!(app.dialog.is_none());
    assert_eq!(app.bookmarks.get('x'), Some(a.clone()), "取消で元のまま");

    // Trigger it again and y=overwrite: switches to b.txt.
    app.start_mark_set();
    app.mark_input('x');
    assert!(app.dialog.is_some());
    app.dialog_confirm(true).unwrap();
    assert!(app.dialog.is_none());
    assert_eq!(app.bookmarks.get('x'), Some(b.clone()), "y で上書き成立");

    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn bookmark_overwrite_same_path_and_confirm_off_skip_dialog() {
    // Two cases where no confirmation appears: (1) re-registering the same path (not an
    // overwrite); (2) confirm_bookmark_overwrite=false.
    let root = std::env::temp_dir().join("konoma_bm_overwrite_skip");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("a.txt"), b"a").unwrap();
    std::fs::write(root.join("b.txt"), b"b").unwrap();
    let root = root.canonicalize().unwrap();
    let a = root.join("a.txt");
    let b = root.join("b.txt");

    // (1) Even with confirmation ON, "re-registering the same path" registers immediately without confirmation.
    let mut app = App::new(root.clone(), Config::default()).unwrap();
    app.bookmarks = crate::bookmarks::Bookmarks::with_base(root.join("cfgbase1"), &root);
    app.enter_preview(&a);
    app.start_mark_set();
    app.mark_input('x');
    app.start_mark_set();
    app.mark_input('x'); // re-registers the same a.txt
    assert!(app.dialog.is_none(), "同一パス再登録は確認不要");
    assert_eq!(app.bookmarks.get('x'), Some(a.clone()));

    // (2) With confirm_bookmark_overwrite=false, even a different path overwrites immediately.
    let mut cfg = Config::default();
    cfg.ui.confirm_bookmark_overwrite = false;
    let mut app = App::new(root.clone(), cfg).unwrap();
    app.bookmarks = crate::bookmarks::Bookmarks::with_base(root.join("cfgbase2"), &root);
    app.enter_preview(&a);
    app.start_mark_set();
    app.mark_input('x');
    app.enter_preview(&b);
    app.start_mark_set();
    app.mark_input('x'); // a different path, but confirmation is off
    assert!(
        app.dialog.is_none(),
        "confirm オフは確認ダイアログを出さない"
    );
    assert_eq!(app.bookmarks.get('x'), Some(b.clone()), "即上書き");

    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn tab_list_switch_close_and_guards() {
    // Tab list: opening with T sets the selection=active, j/k cycle, Enter switches (the list
    // closes); w closes the **selected tab** (correctly adjusting the active index); the last
    // remaining tab is rejected with a flash.
    let dir = std::env::temp_dir().join("konoma_tab_list_test");
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = App::new(dir.canonicalize().unwrap(), Config::default()).unwrap();
    app.tab_new().unwrap();
    app.tab_new().unwrap(); // 3 tabs, active=2 (the last one)
    assert_eq!(app.tab_count(), 3);
    assert_eq!(app.active_tab_index(), 2);

    app.toggle_tab_list();
    assert!(app.is_tab_list());
    assert_eq!(app.tab_list_sel(), 2, "開いた時の選択はアクティブタブ");
    app.tab_list_move(1);
    assert_eq!(app.tab_list_sel(), 0, "末尾から wrap");
    app.tab_list_move(-1);
    assert_eq!(app.tab_list_sel(), 2);

    // Enter: switches to the selected tab + the list closes.
    app.tab_list_move(1); // → 0
    app.tab_list_activate();
    assert!(!app.is_tab_list(), "切替で一覧が閉じる");
    assert_eq!(app.active_tab_index(), 0);

    // Closing with w (selection = the trailing, inactive tab) keeps the active tab unchanged.
    app.toggle_tab_list();
    app.tab_list_move(-1); // sel=2
    app.tab_list_close_selected();
    assert!(app.is_tab_list(), "w では一覧は開いたまま");
    assert_eq!(app.tab_count(), 2);
    assert_eq!(
        app.active_tab_index(),
        0,
        "非アクティブを閉じてもアクティブ不変"
    );
    assert!(app.tab_list_sel() < app.tab_count(), "選択はクランプ");

    // Closing a tab before the active one shifts the active index down.
    app.tab_goto(1);
    app.toggle_tab_list();
    app.tab_list_move(-1); // sel=0 (active=1)
    app.tab_list_close_selected();
    assert_eq!(app.tab_count(), 1);
    assert_eq!(app.active_tab_index(), 0, "前方を閉じたら index が詰まる");

    // The last remaining tab cannot be closed (flash).
    app.flash = None;
    app.tab_list_close_selected();
    assert_eq!(app.tab_count(), 1);
    assert!(app.flash.is_some(), "最後の1枚は拒否して flash");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn bookmark_list_jump_delete_and_close() {
    // Use a test-only base for bookmarks so the real ~/.config isn't polluted.
    let root = std::env::temp_dir().join("konoma_bm_list_ops_test");
    let _ = std::fs::remove_dir_all(&root);
    let proj = root.join("proj");
    std::fs::create_dir_all(proj.join("sub")).unwrap();
    std::fs::write(proj.join("f.txt"), b"x").unwrap();
    let base = root.join("cfgbase");
    let proj = proj.canonicalize().unwrap();

    let mut app = App::new(proj.clone(), Config::default()).unwrap();
    app.bookmarks = crate::bookmarks::Bookmarks::with_base(base.clone(), &proj);
    // Register local 'a'=directory sub / 'b'=file f.txt.
    app.bookmarks.set('a', proj.join("sub")).unwrap();
    app.bookmarks.set('b', proj.join("f.txt")).unwrap();
    app.open_bookmark_list();
    assert!(app.is_bookmark_list());
    assert_eq!(app.bookmark_list_items().len(), 2);

    // Jump to the first one (a=the sub directory) → root becomes sub and the list closes.
    app.bookmark_list_sel = 0;
    app.bookmark_list_jump();
    assert!(!app.is_bookmark_list(), "ジャンプで一覧が閉じる");
    assert_eq!(
        app.tab.root,
        proj.join("sub"),
        "ディレクトリへジャンプで root 変更"
    );

    // Open it again and delete → the count decreases and the list stays open (selection clamped).
    app.open_bookmark_list();
    let before = app.bookmark_list_items().len();
    app.bookmark_list_sel = before - 1;
    app.bookmark_list_delete();
    assert_eq!(app.bookmark_list_items().len(), before - 1, "削除で1件減る");
    assert!(app.is_bookmark_list(), "削除後も一覧は開いたまま");
    assert!(app.bookmark_list_sel < app.bookmark_list_items().len().max(1));
    app.close_bookmark_list();
    assert!(!app.is_bookmark_list(), "close で閉じる");
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn attach_git_loader_stores_channel() {
    let dir = std::env::temp_dir().join("konoma_attach_git_loader_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    assert!(app.ignored_tx.is_none(), "初期はチャネル未装着");
    let (tx, rx) = std::sync::mpsc::channel::<IgnoredResult>();
    Box::leak(Box::new(rx));
    app.attach_git_loader(tx);
    assert!(
        app.ignored_tx.is_some(),
        "attach_git_loader で Sender を保持"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[cfg(feature = "git")]
#[test]
fn refresh_git_status_only_updates_statuses_without_recompute() {
    let dir = std::env::temp_dir().join("konoma_refresh_status_only_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    init_git_repo(&dir);
    let sh = |args: &[&str]| {
        std::process::Command::new("git")
            .current_dir(&dir)
            .args(args)
            .output()
            .unwrap();
    };
    std::fs::write(dir.join("a.txt"), b"v1\n").unwrap();
    sh(&["add", "-A"]);
    sh(&["commit", "-m", "init"]);
    let canon = dir.canonicalize().unwrap();
    let mut app = App::new(canon.clone(), Config::default()).unwrap();
    app.refresh_git_if_needed(); // sets git_status_for
    assert!(!app.git_has_changes(), "コミット直後はクリーン");
    // Change it externally, then only the cheap refresh (refresh_fs(false) calls refresh_git_status_only).
    std::fs::write(dir.join("a.txt"), b"v2\n").unwrap();
    app.refresh_fs(false).unwrap();
    assert!(app.git_has_changes(), "statuses のみ更新で変更を検知");
    assert!(
        app.git_status_of(&canon.join("a.txt")).is_some(),
        "a.txt に status が付く"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn op_base_dir_for_file_dir_and_empty() {
    let dir = std::env::temp_dir().join("konoma_op_base_dir_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    std::fs::write(dir.join("a.txt"), b"x").unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.rebuild_tree().unwrap();
    // Select a directory → inside it.
    let si = app
        .tab
        .entries
        .iter()
        .position(|e| e.path.ends_with("sub"))
        .unwrap();
    app.tab.selected = si;
    assert_eq!(app.op_base_dir(), dir.join("sub"), "dir 選択はその中");
    // Select a file → its parent.
    let ai = app
        .tab
        .entries
        .iter()
        .position(|e| e.path.ends_with("a.txt"))
        .unwrap();
    app.tab.selected = ai;
    assert_eq!(app.op_base_dir(), dir, "file 選択は親ディレクトリ");
    // No entry (out of selection range) → root.
    app.tab.entries.clear();
    assert_eq!(app.op_base_dir(), app.tab.root, "エントリ無しは root");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn duplicate_selection_copies_file_and_dir_in_place() {
    let base = std::env::temp_dir().join("konoma_duplicate_test");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("sub")).unwrap();
    let root = base.canonicalize().unwrap();
    std::fs::write(root.join("note.md"), b"hello").unwrap();
    std::fs::write(root.join("sub").join("inner.txt"), b"x").unwrap();
    let mut app = App::new(root.clone(), Config::default()).unwrap();

    // Cursor on file note.md → note copy.md is created in place, with identical content.
    let idx = app
        .tab
        .entries
        .iter()
        .position(|e| e.path.ends_with("note.md"))
        .unwrap();
    app.tab.selected = idx;
    app.duplicate_selection().unwrap();
    assert!(
        root.join("note copy.md").exists(),
        "note copy.md が同ディレクトリにできる"
    );
    assert_eq!(
        std::fs::read(root.join("note copy.md")).unwrap(),
        b"hello",
        "複製の内容が元と同一"
    );

    // Second time → note copy 2.md (a sequence number, per the existing unique_name convention).
    let idx = app
        .tab
        .entries
        .iter()
        .position(|e| e.path.ends_with("note.md"))
        .unwrap();
    app.tab.selected = idx;
    app.duplicate_selection().unwrap();
    assert!(
        root.join("note copy 2.md").exists(),
        "2回目は note copy 2.md"
    );

    // Cursor on directory sub → sibling sub copy/ is duplicated recursively (with its contents).
    let idx = app
        .tab
        .entries
        .iter()
        .position(|e| e.path.ends_with("sub") && e.is_dir)
        .unwrap();
    app.tab.selected = idx;
    app.duplicate_selection().unwrap();
    assert!(root.join("sub copy").is_dir(), "sub copy/ が兄弟にできる");
    assert!(
        root.join("sub copy").join("inner.txt").exists(),
        "ディレクトリの中身も再帰複製される"
    );

    std::fs::remove_dir_all(&base).ok();
}

/// A background file-op runner is attached → `paste()` must return immediately (before the copy
/// completes) with `fileop_pending` set and the busy indicator reporting it, then the result must
/// apply once the worker's channel delivers it. Regression guard for "pasting a big directory
/// freezes input/rendering" (design principle #4).
#[test]
fn file_op_runs_in_background_when_runner_attached() {
    let base = unique_tmp("konoma_fileop_bg_test");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    // Canonicalize first, then create the contents (temp_dir can be reached through a symlink,
    // so canonicalizing afterward would mismatch entries' paths and make position() return None).
    let dir = base.canonicalize().unwrap();
    let src = dir.join("src");
    std::fs::create_dir_all(&src).unwrap();
    for n in ["a.txt", "b.txt", "c.txt"] {
        std::fs::write(src.join(n), b"x").unwrap();
    }
    std::fs::create_dir_all(dir.join("dst")).unwrap();

    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.rebuild_tree().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    app.attach_fileop_runner(tx);

    let idx =
        |app: &App, p: &std::path::Path| app.tab.entries.iter().position(|e| e.path == p).unwrap();
    app.tab.selected = idx(&app, &src);
    app.toggle_select();
    app.copy_selection();
    app.tab.selected = idx(&app, &dir.join("dst"));

    app.paste().unwrap();
    // The moment paste() returns, pending is set = the result arrives **via a channel** (it
    // hasn't been applied on the spot). The worker may already have finished running, but since
    // only the main thread's `apply_file_op` clears `fileop_pending`, it is always set here.
    assert!(
        app.fileop_pending.is_some(),
        "バックグラウンド実行中は fileop_pending が立つ"
    );
    assert!(
        app.busy_jobs().contains(&crate::i18n::Msg::BusyFileOp),
        "busy インジケーターにファイル操作が出る"
    );
    // The progress counter is dropped by apply, so grab the Arc beforehand (to verify
    // items/files along the copy path).
    let progress = app.fileop_progress.clone().expect("進捗カウンタが在る");

    let res = rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("ワーカーが結果を返す");
    assert_eq!(
        progress.items(),
        1,
        "上位ターゲット(src/ 1件)ぶん items が進む"
    );
    assert_eq!(progress.files(), 3, "末端ファイル3枚ぶん files が進む");
    assert!(app.apply_file_op(res), "現世代の結果は適用される");
    assert!(app.fileop_pending.is_none(), "適用で pending が解ける");
    assert!(
        dir.join("dst").join("src").join("a.txt").is_file(),
        "コピーがディスクに反映されている"
    );
    assert!(
        app.flash
            .as_deref()
            .unwrap_or_default()
            .contains(crate::i18n::tr(app.lang, crate::i18n::Msg::Pasted)),
        "flash にペースト結果が出る: {:?}",
        app.flash
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// While one file operation is in flight, a second must be rejected (flash `FileOpBusy`, generation
/// unchanged) rather than starting a second worker thread.
#[test]
fn second_file_op_is_rejected_while_one_is_in_flight() {
    let dir = unique_tmp("konoma_fileop_busy_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.txt"), b"x").unwrap();
    let dir = dir.canonicalize().unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.rebuild_tree().unwrap();

    // Without launching the actual worker, build the in-flight state directly (within the same
    // module, private fields can be touched).
    let gen_before = app.fileop_gen;
    app.fileop_gen = app.fileop_gen.wrapping_add(1);
    app.fileop_pending = Some(FileOpKind::Trash);
    app.fileop_total = 1;

    app.cut_selection(); // loads the clipboard so op_targets() has something to satisfy
    app.paste().unwrap();
    assert_eq!(
        app.flash.as_deref(),
        Some(crate::i18n::tr(app.lang, crate::i18n::Msg::FileOpBusy)),
        "実行中は FileOpBusy フラッシュで拒否される"
    );
    assert_eq!(
        app.fileop_gen,
        gen_before.wrapping_add(1),
        "拒否された2件目は世代を進めない"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// `apply_file_op` must ignore a result whose generation doesn't match the current one (superseded)
/// and must not touch `fileop_pending`.
#[test]
fn stale_file_op_result_is_dropped() {
    let dir = unique_tmp("konoma_fileop_stale_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let dir = dir.canonicalize().unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();

    app.fileop_gen = 5;
    app.fileop_pending = Some(FileOpKind::Trash);
    let stale = FileOpResult {
        gen: 4,
        kind: FileOpKind::Trash,
        root: dir.clone(),
        ok: 1,
        last: None,
        err: None,
    };
    assert!(
        !app.apply_file_op(stale),
        "陳腐化した世代の結果は適用されない"
    );
    assert!(
        app.fileop_pending.is_some(),
        "陳腐化した結果は pending を解かない"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// `App::describe_error` translates every `fileops::FileOpError` variant to whichever language the
/// UI is currently showing (`app.lang`) — cross-checked against the `i18n` catalog itself, so a
/// match arm accidentally reaching for the wrong `Msg` (e.g. `RenameTempExists` using
/// `RenameDestExists`'s wording) fails this, since the two texts differ.
#[test]
fn describe_error_translates_file_op_errors_to_the_ui_language() {
    use crate::fileops::FileOpError;
    use crate::i18n::{tr, Lang, Msg};

    let dir = unique_tmp("konoma_describe_error_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();

    let p = dir.join("some name.txt");
    // (variant, its Msg, whether the wording carries the path)
    let cases: Vec<(anyhow::Error, Msg, bool)> = vec![
        (
            FileOpError::AlreadyExists(p.clone()).into(),
            Msg::AlreadyExists,
            true,
        ),
        (
            anyhow::Error::new(FileOpError::TrashFailed),
            Msg::TrashFailed,
            false,
        ),
        (
            FileOpError::RenameTempExists(p.clone()).into(),
            Msg::RenameTempExists,
            true,
        ),
        (
            FileOpError::RenameDestExists(p.clone()).into(),
            Msg::RenameDestExists,
            true,
        ),
        (
            FileOpError::NameUnavailable(p.clone()).into(),
            Msg::NameUnavailable,
            true,
        ),
        (
            FileOpError::RenameStageFailed(p.clone()).into(),
            Msg::RenameStageFailed,
            true,
        ),
        (
            FileOpError::RenameCommitFailed(p.clone()).into(),
            Msg::RenameCommitFailed,
            true,
        ),
    ];
    for (err, msg, has_path) in &cases {
        for lang in [Lang::En, Lang::Jp] {
            app.lang = lang;
            let want = if *has_path {
                format!("{}{}", tr(lang, *msg), p.display())
            } else {
                tr(lang, *msg).to_string()
            };
            assert_eq!(app.describe_error(err), want, "{msg:?} / {lang:?}");
        }
    }

    // Lock in the literal wording for the case the bug report was actually about (an existing
    // filename), in both languages — not just "matches the catalog" (which would also pass if the
    // catalog itself were wrong).
    let already = FileOpError::AlreadyExists(p.clone()).into();
    app.lang = Lang::En;
    assert_eq!(
        app.describe_error(&already),
        format!("already exists: {}", p.display())
    );
    app.lang = Lang::Jp;
    assert_eq!(
        app.describe_error(&already),
        format!("既に存在します: {}", p.display())
    );

    // A non-`fileops` error (e.g. a plain `anyhow!` from elsewhere, or git's own `fatal:` text) is
    // not one of ours — describe_error can't invent a translation for text it doesn't recognize, so
    // it passes the message through unmodified regardless of `app.lang`.
    let other = anyhow::anyhow!("boom, not a fileops error");
    for lang in [Lang::En, Lang::Jp] {
        app.lang = lang;
        assert_eq!(app.describe_error(&other), "boom, not a fileops error");
    }

    std::fs::remove_dir_all(&dir).ok();
}

// The regression in `copy_into_with_progress`'s leaf-file counter is verified on the
// `src/fileops.rs` side by `file_op_progress_counts_leaf_files` (fileops can be hit directly
// from there without going through `App`).

/// A finished operation must not disturb whatever tab happens to be active when the result arrives.
/// The user can switch tabs during a long copy; `reveal_and_select` rebuilds the tree
/// unconditionally, which would collapse that other tab's `/` filter and move its cursor. The
/// generation check does **not** protect against this (the generation is frozen while an operation
/// is pending) — the root comparison in `apply_file_op` does.
#[test]
fn file_op_result_does_not_disturb_another_tab() {
    let base = unique_tmp("konoma_fileop_other_tab_test");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let base = base.canonicalize().unwrap();
    // Tab 1's root (copy source/destination), and tab 2's different root.
    let a = base.join("a");
    let b = base.join("b");
    let src = a.join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("f.txt"), b"x").unwrap();
    std::fs::create_dir_all(a.join("dst")).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    for n in ["alpha.txt", "beta.txt", "gamma.txt"] {
        std::fs::write(b.join(n), b"x").unwrap();
    }

    let mut app = App::new(a.clone(), Config::default()).unwrap();
    app.rebuild_tree().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    app.attach_fileop_runner(tx);

    let idx =
        |app: &App, p: &std::path::Path| app.tab.entries.iter().position(|e| e.path == p).unwrap();
    app.tab.selected = idx(&app, &src);
    app.toggle_select();
    app.copy_selection();
    app.tab.selected = idx(&app, &a.join("dst"));
    app.paste().unwrap();
    assert!(app.fileop_pending.is_some(), "実行中");

    // Switch to a tab with a different root during the copy, and put it into a `/` filter there.
    app.tab_new().unwrap();
    app.tab.root = b.clone();
    app.tab.entries.clear();
    app.tab.selected = 0;
    app.rebuild_tree().unwrap();
    app.start_filter();
    for c in "beta".chars() {
        app.filter_input_push(c);
    }
    app.filter_commit();
    let before_len = app.tab.entries.len();
    let before_sel = app.tab.selected;
    let before_path = app.tab.entries.get(before_sel).map(|e| e.path.clone());
    assert_eq!(before_len, 1, "絞り込みで beta.txt だけが残っている");

    // The result is "the one launched from tab 1". Since tab 2 is active now, it must not touch
    // the cursor/list.
    let res = rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("ワーカーが結果を返す");
    assert!(app.apply_file_op(res), "現世代の結果は適用される");
    assert!(
        a.join("dst").join("src").join("f.txt").is_file(),
        "コピー自体は完了している"
    );
    assert_eq!(
        app.tab.entries.len(),
        before_len,
        "別タブの絞り込み結果を全表示に戻さない"
    );
    assert_eq!(app.tab.selected, before_sel, "別タブのカーソルを動かさない");
    assert_eq!(
        app.tab
            .entries
            .get(app.tab.selected)
            .map(|e| e.path.clone()),
        before_path,
        "カーソルが指すエントリも変わらない"
    );

    std::fs::remove_dir_all(&base).ok();
}

/// Probes whether this process is actually denied reading a 0o000 directory.
/// Under root (or a process able to bypass permission bits), this has no effect and read_dir
/// succeeds anyway, so permission-dependent tests skip in that case (same spirit as
/// `write_denied_by_permissions`).
#[cfg(unix)]
fn read_dir_denied_by_permissions() -> bool {
    use std::os::unix::fs::PermissionsExt;
    let probe = unique_tmp("konoma_readperm_probe");
    let _ = std::fs::remove_dir_all(&probe);
    std::fs::create_dir_all(&probe).unwrap();
    std::fs::set_permissions(&probe, std::fs::Permissions::from_mode(0o000)).unwrap();
    let denied = std::fs::read_dir(&probe).is_err();
    let _ = std::fs::set_permissions(&probe, std::fs::Permissions::from_mode(0o755));
    let _ = std::fs::remove_dir_all(&probe);
    denied
}

/// The operation succeeded but the follow-up refresh failed → report the failure, do **not** paper
/// over it with the success flash. (The old synchronous code used `refresh()?`, so the error
/// propagated to `handle_key` and `resolve_key_result` flashed it.)
#[cfg(unix)]
#[test]
fn file_op_refresh_failure_is_reported_not_masked() {
    use std::os::unix::fs::PermissionsExt;
    if !read_dir_denied_by_permissions() {
        eprintln!(
            "file_op_refresh_failure_is_reported_not_masked: このプロセスはパーミッションで読み取りを拒否されない(root 等)ためスキップ"
        );
        return;
    }
    let dir = unique_tmp("konoma_fileop_refresh_err_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.txt"), b"x").unwrap();
    let dir = dir.canonicalize().unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.rebuild_tree().unwrap();

    // Without launching the actual worker, build the in-flight state directly and construct a
    // successful result by hand.
    app.fileop_gen = 7;
    app.fileop_pending = Some(FileOpKind::PasteCopy);
    let ok_result = FileOpResult {
        gen: 7,
        kind: FileOpKind::PasteCopy,
        root: dir.clone(),
        ok: 1,
        last: None,
        err: None,
    };
    // Make root unreadable → the refresh() inside apply fails.
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o000)).unwrap();
    let applied = app.apply_file_op(ok_result);
    // Clean up (restore it before the assert, so it can be removed even on failure).
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();

    assert!(applied, "現世代の結果は適用される");
    let flash = app.flash.clone().unwrap_or_default();
    assert!(
        flash.contains(crate::i18n::tr(app.lang, crate::i18n::Msg::Failed)),
        "再読込の失敗を通知する: {flash:?}"
    );
    assert!(
        !flash.contains(crate::i18n::tr(app.lang, crate::i18n::Msg::Pasted)),
        "成功 flash で失敗を覆い隠さない: {flash:?}"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// A delete that fails partway must keep the selection (the user can retry); only a fully
/// successful delete clears it. The old synchronous code cleared it inside the `Ok(())` arm.
#[test]
fn failed_delete_keeps_the_selection() {
    let dir = unique_tmp("konoma_fileop_del_sel_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.txt"), b"x").unwrap();
    std::fs::write(dir.join("b.txt"), b"x").unwrap();
    let dir = dir.canonicalize().unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.rebuild_tree().unwrap();

    app.tab.selected = 0;
    app.toggle_select();
    app.toggle_select();
    assert_eq!(app.marked_count(), 2, "2件選択した状態から始める");

    // A failed delete: the selection remains.
    app.fileop_gen = 3;
    app.fileop_pending = Some(FileOpKind::Trash);
    assert!(app.apply_file_op(FileOpResult {
        gen: 3,
        kind: FileOpKind::Trash,
        root: dir.clone(),
        ok: 0,
        last: None,
        err: Some("boom".into()),
    }));
    assert!(
        app.has_selection(),
        "途中で失敗した削除は選択を保つ(選び直させない)"
    );

    // A successful delete: clears the selection.
    app.fileop_pending = Some(FileOpKind::Trash);
    assert!(app.apply_file_op(FileOpResult {
        gen: 3,
        kind: FileOpKind::Trash,
        root: dir.clone(),
        ok: 2,
        last: None,
        err: None,
    }));
    assert!(!app.has_selection(), "成功した削除は選択を解除する");

    // Same thing via the real dispatch path (`Space→d` → `!`). With no runner attached =
    // synchronous execution, so start_file_op carries it through to apply on the spot. Deleting
    // the first item externally beforehand makes the permanent delete fail partway, and the
    // remaining selection (b.txt) is kept = with the old implementation's clear-on-dispatch, it
    // would end up empty here.
    std::fs::write(dir.join("a.txt"), b"x").unwrap();
    app.refresh().unwrap();
    app.tab.selected = 0;
    app.toggle_select();
    app.toggle_select();
    assert_eq!(app.marked_count(), 2, "a.txt と b.txt を選択");
    app.start_delete();
    assert!(app.dialog_is_confirm() && app.dialog_allow_permanent());
    std::fs::remove_file(dir.join("a.txt")).unwrap(); // delete the first item externally → the permanent delete fails
    app.dialog_delete_permanent().unwrap();
    assert!(
        app.flash
            .as_deref()
            .unwrap_or_default()
            .contains(crate::i18n::tr(app.lang, crate::i18n::Msg::Failed)),
        "失敗が通知される: {:?}",
        app.flash
    );
    assert!(
        app.has_selection(),
        "ディスパッチ経路でも、失敗した削除は選択を保つ"
    );
    assert!(dir.join("b.txt").exists(), "失敗で打ち切られ b.txt は残る");

    std::fs::remove_dir_all(&dir).ok();
}

/// `Q` while a file operation is running must always confirm — even with `ui.confirm_quit = false`,
/// because the worker is detached and quitting would truncate a half-finished copy. Quitting is not
/// blocked (the dialog's `y` still quits); the message just gains the warning line.
#[test]
fn quit_while_file_op_always_confirms() {
    let dir = unique_tmp("konoma_quit_fileop_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let dir = dir.canonicalize().unwrap();
    let mut cfg = Config::default();
    cfg.ui.confirm_quit = false;
    let mut app = App::new(dir.clone(), cfg).unwrap();

    // No operation = quits immediately as before (no confirmation).
    assert!(!app.request_quit(), "confirm_quit=false + 操作なしは即終了");
    assert!(!app.is_dialog(), "ダイアログは開かない");

    // Mid-operation = confirms regardless of the setting.
    app.fileop_pending = Some(FileOpKind::PasteCopy);
    assert!(app.request_quit(), "実行中は確認ダイアログを出す");
    assert!(app.dialog_is_confirm() && app.confirm_is_quit(), "終了確認");
    let (_, head, _, _) = app.dialog_view().unwrap();
    assert!(
        head.contains(crate::i18n::tr(app.lang, crate::i18n::Msg::QuitWhileFileOp)),
        "実行中である旨の警告が本文に出る: {head:?}"
    );
    assert!(
        head.contains(crate::i18n::tr(app.lang, crate::i18n::Msg::QuitConfirm)),
        "通常の終了確認文も残る: {head:?}"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// The busy indicator must show a running file operation even when `ui.busy_indicator = false`:
/// it is the thing the user is waiting on, not background bookkeeping. Idle stays silent.
#[test]
fn busy_indicator_always_shows_file_operations() {
    let dir = unique_tmp("konoma_busy_fileop_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let dir = dir.canonicalize().unwrap();
    let mut cfg = Config::default();
    cfg.ui.busy_indicator = false;
    let mut app = App::new(dir.clone(), cfg).unwrap();

    assert!(
        !app.busy_indicator_active(),
        "アイドル時は(設定に関わらず)出さない"
    );
    app.fileop_pending = Some(FileOpKind::PasteCopy);
    assert!(
        app.busy_indicator_active(),
        "busy_indicator=false でもファイル操作中は出す"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// The "rejected while busy" branches: a second operation must leave the state it would otherwise
/// consume intact — the cut clipboard stays (so the user can retry the move) and the selection
/// stays (so a duplicate can be retried).
#[test]
fn rejected_file_op_keeps_clipboard_and_selection() {
    let dir = unique_tmp("konoma_fileop_reject_state_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.txt"), b"x").unwrap();
    let dir = dir.canonicalize().unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.rebuild_tree().unwrap();

    app.tab.selected = 0;
    app.toggle_select();
    app.cut_selection(); // into the clipboard (cut_selection itself clears the selection)
    assert!(app.clipboard_label().is_some());

    // Build the in-progress state directly (without launching the worker).
    app.fileop_pending = Some(FileOpKind::Trash);

    app.paste().unwrap();
    assert!(
        app.clipboard_label().is_some(),
        "拒否されたカット貼付はクリップボードを消費しない(やり直せる)"
    );

    // Same for duplicate: if rejected, keeps the selection.
    app.tab.selected = 0;
    app.toggle_select();
    assert!(app.has_selection());
    app.duplicate_selection().unwrap();
    assert!(
        app.has_selection(),
        "拒否された複製は選択を解除しない(やり直せる)"
    );
    assert_eq!(
        app.flash.as_deref(),
        Some(crate::i18n::tr(app.lang, crate::i18n::Msg::FileOpBusy)),
        "拒否は FileOpBusy で通知される"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// A cut clipboard must be consumed the moment `paste()` kicks the job off — **before** the result
/// is applied — since the paths are already being moved by the (possibly still-running) worker.
#[test]
fn paste_cut_consumes_the_clipboard_at_kick() {
    let dir = unique_tmp("konoma_fileop_cut_kick_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("dst")).unwrap();
    std::fs::write(dir.join("a.txt"), b"x").unwrap();
    let dir = dir.canonicalize().unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.rebuild_tree().unwrap();

    let idx =
        |app: &App, p: &std::path::Path| app.tab.entries.iter().position(|e| e.path == p).unwrap();
    app.tab.selected = idx(&app, &dir.join("a.txt"));
    app.toggle_select();
    app.cut_selection();
    assert!(app.clipboard_label().is_some());

    let (tx, rx) = std::sync::mpsc::channel();
    app.attach_fileop_runner(tx);
    app.tab.selected = idx(&app, &dir.join("dst"));
    app.paste().unwrap();
    assert!(
        app.clipboard_label().is_none(),
        "カットのクリップボードは投げた瞬間に消費される(結果適用を待たない)"
    );

    // Receive the worker's result and clean up (recv it here to catch resource leaks).
    if let Ok(res) = rx.recv_timeout(std::time::Duration::from_secs(5)) {
        app.apply_file_op(res);
    }
    std::fs::remove_dir_all(&dir).ok();
}

/// Background permanent delete: attach a runner, drive `dialog_delete_permanent`, receive and apply
/// the result, and confirm the targets are actually gone from disk. (Uses permanent delete, not
/// trash — the real OS trash is environment-dependent and gated to macOS elsewhere.)
#[test]
fn async_permanent_delete_removes_targets() {
    let dir = unique_tmp("konoma_fileop_async_delete_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("gone.txt"), b"x").unwrap();
    let dir = dir.canonicalize().unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.rebuild_tree().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    app.attach_fileop_runner(tx);

    let i = app
        .tab
        .entries
        .iter()
        .position(|e| e.path == dir.join("gone.txt"))
        .unwrap();
    app.tab.selected = i;
    app.start_delete();
    assert!(app.dialog_is_confirm() && app.dialog_allow_permanent());
    app.dialog_delete_permanent().unwrap();
    assert!(!app.is_dialog(), "完全削除でダイアログは閉じる");
    assert!(
        app.fileop_pending.is_some(),
        "バックグラウンド実行中は fileop_pending が立つ"
    );

    let res = rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("ワーカーが結果を返す");
    assert!(app.apply_file_op(res));
    assert!(app.fileop_pending.is_none());
    assert!(
        !dir.join("gone.txt").exists(),
        "完全削除でファイルが消える(ゴミ箱を経由しない)"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// `should_defer_fs_events()` must track whether a background file operation is in flight, so the
/// run loop can hold back watcher-driven refreshes while a large copy/move/delete is running
/// (regression guard for "pasting a big directory starves keyboard input" — the same watcher-noise
/// class as `is_content_event`/the `.git` lock filter, but for konoma's own bulk writes).
#[test]
fn should_defer_fs_events_tracks_file_op_in_flight() {
    let base = unique_tmp("konoma_defer_fs_events_test");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let dir = base.canonicalize().unwrap();
    let src = dir.join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("a.txt"), b"x").unwrap();
    std::fs::create_dir_all(dir.join("dst")).unwrap();

    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.rebuild_tree().unwrap();
    assert!(!app.should_defer_fs_events(), "操作前は溜め込まない");

    let (tx, rx) = std::sync::mpsc::channel();
    app.attach_fileop_runner(tx);

    let idx =
        |app: &App, p: &std::path::Path| app.tab.entries.iter().position(|e| e.path == p).unwrap();
    app.tab.selected = idx(&app, &src);
    app.toggle_select();
    app.copy_selection();
    app.tab.selected = idx(&app, &dir.join("dst"));

    app.paste().unwrap();
    assert!(
        app.should_defer_fs_events(),
        "バックグラウンド実行中は fs イベントを溜め込む"
    );

    let res = rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("ワーカーが結果を返す");
    assert!(app.apply_file_op(res), "現世代の結果は適用される");
    assert!(
        !app.should_defer_fs_events(),
        "適用後は溜め込みを解く(以後のバーストは通常どおり即時反映)"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn dialog_preview_scroll_clamps_within_lines() {
    let dir = std::env::temp_dir().join("konoma_dialog_preview_scroll_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for n in ["a.txt", "b.txt", "c.txt"] {
        std::fs::write(dir.join(n), b"x").unwrap();
    }
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.rebuild_tree().unwrap();
    app.visual_select_scope(true); // select all
                                   // Enter the batch-rename preview (a multi-line preview).
    app.start_batch_rename();
    for c in "img_{n}".chars() {
        app.dialog_input_push(c);
    }
    app.dialog_submit().unwrap();
    assert!(app.dialog_is_preview(), "プレビューに遷移");
    let n_lines = app.dialog_preview_view().unwrap().1.len();
    assert!(n_lines >= 3);
    // Clamped at the upper bound (line count - 1).
    app.dialog_preview_scroll(1000);
    assert_eq!(
        app.dialog_preview_view().unwrap().2,
        n_lines - 1,
        "末尾でクランプ"
    );
    // Clamped at the lower bound 0.
    app.dialog_preview_scroll(-1000);
    assert_eq!(app.dialog_preview_view().unwrap().2, 0, "先頭でクランプ");
    app.dialog_cancel();
    std::fs::remove_dir_all(&dir).ok();
}

// --- ui::preview::render's dispatch branches (TestBackend golden) -------------

#[test]
fn ui_preview_renders_image_via_kitty_path() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    // A GIF (2 frames) encodes synchronously, so render_image's image path emits real pixels
    // (the kitty transfer sequence _G) even with a workerless TestBackend. A still image is
    // asynchronous (app.image arrives later).
    let mut app = app_with_kitty();
    app.tab.preview_kind = Some(PreviewKind::Image(PathBuf::from("x.gif")));
    let frame = |c: [u8; 3]| {
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(40, 30, image::Rgb(c)))
    };
    app.gif_frames = vec![
        (frame([255, 0, 0]), std::time::Duration::from_millis(50)),
        (frame([0, 255, 0]), std::time::Duration::from_millis(50)),
    ];
    app.gif_idx = 0;
    app.tab.mode = Mode::Preview;
    assert!(app.is_gif_active() && app.is_image_preview());
    let mut term = Terminal::new(TestBackend::new(48, 20)).unwrap();
    term.draw(|f| crate::ui::preview::render(f, &mut app, f.area()))
        .unwrap();
    let s: String = term
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|c| c.symbol())
        .collect();
    // The kitty graphics transfer sequence (_G) = proof that real pixels were drawn.
    assert!(s.contains("_G"), "render_image が kitty 転送列を吐かない");
}

#[test]
fn ui_preview_image_falls_back_when_not_yet_encoded() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    // A still image, but app.image (the already-encoded protocol) hasn't arrived → falls back
    // to ImageUnsupported. Exercises render_image's still-image fallback branch.
    let mut app = app_with_kitty();
    app.tab.preview_kind = Some(PreviewKind::Image(PathBuf::from("x.png")));
    app.image_src = Some(std::sync::Arc::new(image::DynamicImage::new_rgb8(20, 10)));
    app.tab.mode = Mode::Preview;
    let mut term = Terminal::new(TestBackend::new(60, 10)).unwrap();
    term.draw(|f| crate::ui::preview::render(f, &mut app, f.area()))
        .unwrap();
    let s: String = term
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|c| c.symbol())
        .collect();
    assert!(s.contains("image"), "静止画フォールバック文言が出ない: {s}");
}

#[test]
fn ui_preview_renders_text_fallbacks_for_unsupported_kinds() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    let dir = std::env::temp_dir().join("konoma_ui_preview_fallback_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.tab.mode = Mode::Preview;
    app.tab.preview_path = Some(dir.join("thing.bin"));

    let dump = |app: &mut App| -> String {
        let mut term = Terminal::new(TestBackend::new(70, 10)).unwrap();
        term.draw(|f| crate::ui::preview::render(f, app, f.area()))
            .unwrap();
        term.backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect()
    };

    // Unsupported → [can not preview: <ext>].
    app.tab.preview_kind = Some(PreviewKind::CanNotPreview { ext: "bin".into() });
    assert!(
        dump(&mut app).contains("can not preview"),
        "未対応フォールバックが出ない"
    );

    // A video with no thumbnail (no image_src) → falls back to the hint text.
    app.tab.preview_kind = Some(PreviewKind::Video(dir.join("v.mp4")));
    let video = dump(&mut app);
    assert!(
        video.contains("video") || video.contains("動画"),
        "動画フォールバックが出ない: {video}"
    );

    // External command delegation (detached): a one-line "opened externally" summary naming the
    // template — not the raw `{:?}` dump this used to be (that bug is what motivated this rewrite:
    // `render_as=None, detached=true` leaking straight onto the screen).
    app.tab.preview_kind = Some(PreviewKind::Command {
        path: dir.join("c.xyz"),
        template: "mpv {path}".into(),
        render_as: None,
        detached: true,
    });
    let detached_body = dump(&mut app);
    assert!(
        detached_body.contains("mpv"),
        "detached コマンドの案内(テンプレート)が出ない: {detached_body}"
    );
    assert!(
        !detached_body.contains("render_as") && !detached_body.contains("detached="),
        "Debug 書式が画面に漏れている: {detached_body}"
    );

    // External command delegation (failed, non-detached): degrades to [can not preview: <ext>]
    // with the reason attached — never a Debug dump. `command_err` is set directly here (this test
    // targets the render fallback, not process execution — see preview::command's own tests for
    // run_capture/run_detached and app/media_load's MediaJob::Command coverage for the worker path).
    app.tab.preview_kind = Some(PreviewKind::Command {
        path: dir.join("c.xyz"),
        template: "does-not-exist {path}".into(),
        render_as: None,
        detached: false,
    });
    app.command_err = Some("failed to launch `does-not-exist`".into());
    let failed_body = dump(&mut app);
    assert!(
        failed_body.contains("can not preview") && failed_body.contains("xyz"),
        "失敗時に can not preview へ降格しない: {failed_body}"
    );
    assert!(
        failed_body.contains("failed to launch"),
        "失敗理由が表示されない: {failed_body}"
    );
    assert!(
        !failed_body.contains("render_as") && !failed_body.contains("detached="),
        "Debug 書式が画面に漏れている: {failed_body}"
    );
    std::fs::remove_dir_all(&dir).ok();
}

// ---- External command delegation (`preview::command` wired into `MediaJob`/`App`) ----

/// `MediaJob::Command` run directly (no App/thread involved): text mode (`as_image=false`) returns
/// `MediaPayload::CommandText` pointing at the file the captured output was written to.
#[cfg(unix)]
#[test]
fn media_job_command_text_mode_returns_command_text_payload() {
    let dir = unique_tmp("cmd_job_text");
    std::fs::create_dir_all(&dir).unwrap();
    let src = dir.join("note.txt");
    std::fs::write(&src, "hello").unwrap();
    let out = dir.join("out.txt");
    let argv = crate::preview::command::build_argv("cat {path}", &src, &out);
    let job = MediaJob::Command {
        argv,
        out: out.clone(),
        uses_out: false,
        as_image: false,
    };
    match job.run() {
        Some(MediaPayload::CommandText(p)) => {
            assert_eq!(p, out);
            assert_eq!(std::fs::read_to_string(&p).unwrap(), "hello");
        }
        Some(_) => panic!("CommandText を期待したが別の payload だった"),
        None => panic!("CommandText を期待したが None (失敗) だった"),
    }
    std::fs::remove_dir_all(&dir).ok();
}

/// Image mode (`as_image=true`) decodes the produced artifact into `MediaPayload::Static` and
/// deletes the artifact right after (like `preview::video::thumbnail`'s temp PNG — the image path
/// only needs the decoded pixels from here on).
#[cfg(unix)]
#[test]
fn media_job_command_image_mode_decodes_and_cleans_up_artifact() {
    let dir = unique_tmp("cmd_job_image");
    std::fs::create_dir_all(&dir).unwrap();
    // A real (tiny) PNG this "command" just copies via `cp`, so the test doesn't depend on any
    // image-conversion tool being installed.
    let seed = dir.join("seed.png");
    let img = image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
        4,
        4,
        image::Rgb([200, 10, 10]),
    ));
    img.save(&seed).unwrap();
    let out = dir.join("out.png");
    let argv = vec![
        "cp".to_string(),
        seed.to_string_lossy().to_string(),
        out.to_string_lossy().to_string(),
    ];
    let job = MediaJob::Command {
        argv,
        out: out.clone(),
        uses_out: true,
        as_image: true,
    };
    match job.run() {
        Some(MediaPayload::Static(im)) => {
            assert_eq!((im.width(), im.height()), (4, 4));
        }
        Some(_) => panic!("Static を期待したが別の payload だった"),
        None => panic!("Static を期待したが None (失敗) だった"),
    }
    assert!(
        !out.exists(),
        "画像モード成功後に成果物ファイルが残っている: {}",
        out.display()
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// Same as above, but the template writes to a **suffixed** artifact (`<out>.png`) rather than the
/// literal `{out}` path — the documented `merman -i {path} -o {out}.png` idiom (`command.rs`'s
/// `find_suffixed_out`). Cleanup must target the path actually used (the suffixed one), not the
/// unused literal `out` — deleting the wrong (nonexistent) path would silently leak the real artifact.
#[cfg(unix)]
#[test]
fn media_job_command_image_mode_cleans_up_a_suffixed_artifact() {
    let dir = unique_tmp("cmd_job_image_suffix");
    std::fs::create_dir_all(&dir).unwrap();
    let seed = dir.join("seed.png");
    let img = image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
        4,
        4,
        image::Rgb([10, 200, 10]),
    ));
    img.save(&seed).unwrap();
    // A bare {out} path (as `temp_out_path` produces — no extension); the "command" appends its
    // own extension, so the real artifact lands at `out`.png, never at the literal `out` path.
    let out = dir.join("konoma-cmd-suffix-test");
    let suffixed = out.with_extension("png");
    let argv = vec![
        "cp".to_string(),
        seed.to_string_lossy().to_string(),
        suffixed.to_string_lossy().to_string(),
    ];
    let job = MediaJob::Command {
        argv,
        out: out.clone(),
        uses_out: true,
        as_image: true,
    };
    match job.run() {
        Some(MediaPayload::Static(im)) => {
            assert_eq!((im.width(), im.height()), (4, 4));
        }
        Some(_) => panic!("Static を期待したが別の payload だった"),
        None => panic!("Static を期待したが None (失敗) だった"),
    }
    assert!(
        !suffixed.exists(),
        "画像モード成功後に接尾辞つき成果物ファイルが残っている: {}",
        suffixed.display()
    );
    assert!(!out.exists(), "リテラル out は元々書かれていないはず");
    std::fs::remove_dir_all(&dir).ok();
}

/// A failing command (missing binary) returns `MediaPayload::CommandFailed` with a non-empty
/// reason, not a bare `None` — this is what lets the render side show *why* it failed instead of a
/// generic hint (`app.command_error()`, exercised by `ui_preview_renders_text_fallbacks_for_unsupported_kinds`).
#[cfg(unix)]
#[test]
fn media_job_command_failure_returns_command_failed_payload() {
    let out = unique_tmp("cmd_job_fail_out");
    let argv = vec!["konoma-definitely-nonexistent-command-xyz123".to_string()];
    let job = MediaJob::Command {
        argv,
        out,
        uses_out: false,
        as_image: false,
    };
    match job.run() {
        Some(MediaPayload::CommandFailed(msg)) => assert!(!msg.is_empty()),
        Some(_) => panic!("CommandFailed を期待したが別の payload だった"),
        None => panic!("CommandFailed を期待したが None だった"),
    }
}

/// `detached=true` is launched exactly once (from `enter_preview`) and never re-launched by
/// redrawing the same preview repeatedly or by switching tabs away and back — the whole point of
/// excluding it from `kind_loads_media` (re-triggering it there would relaunch e.g. mpv every time
/// you flip back to that tab). Counted via `preview::command::calls_for_test` (a thread-local
/// counter of `temp_out_path` calls — `start_media_load` calls it exactly once per
/// `PreviewKind::Command` attempt, detached or not — rather than parsing the process-global,
/// cross-test-polluted counter baked into the generated filename).
#[cfg(unix)]
#[test]
fn detached_command_spawns_once_per_preview_entry_not_per_redraw_or_tab_switch() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    let dir = unique_tmp("cmd_detached_once");
    std::fs::create_dir_all(&dir).unwrap();
    let src = dir.join("note.detach");
    std::fs::write(&src, "x").unwrap();

    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.cfg.preview.rules = vec![crate::config::Rule {
        glob: Some("*.detach".into()),
        command: Some("true".into()), // a real, instantly-exiting POSIX no-op
        detached: true,
        ..Default::default()
    }];

    let before = crate::preview::command::calls_for_test();

    app.enter_preview(&src); // 1 attempt

    // Redrawing the same preview repeatedly must not re-attempt.
    for _ in 0..5 {
        let mut term = Terminal::new(TestBackend::new(60, 20)).unwrap();
        term.draw(|f| crate::ui::preview::render(f, &mut app, f.area()))
            .unwrap();
    }

    // Nor must creating a new tab and switching back to this one.
    app.tab_new().unwrap();
    app.tab_cycle(-1);

    let after = crate::preview::command::calls_for_test();
    assert_eq!(
        after - before,
        1,
        "detached コマンドが enter_preview 以外のタイミングでも起動された"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// A text-mode delegated command becomes windowed (2D caret / search / `v`/`V` selection all just
/// work), the title still shows the original file (not the temp path), and moving to a different
/// preview deletes the generated temp output.
#[cfg(unix)]
#[test]
fn text_command_becomes_windowed_with_original_title_and_cleans_up_on_leaving() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    let dir = unique_tmp("cmd_text_windowed");
    std::fs::create_dir_all(&dir).unwrap();
    let src = dir.join("report.custom");
    std::fs::write(&src, "irrelevant").unwrap();
    let other = dir.join("plain.txt");
    std::fs::write(&other, "just text").unwrap();

    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.cfg.preview.rules = vec![crate::config::Rule {
        glob: Some("*.custom".into()),
        command: Some("echo hello-from-command".into()),
        ..Default::default()
    }];

    app.enter_preview(&src);
    // Synchronous fallback (no media_tx attached in a bare App::new) ⇒ apply_payload already ran.
    assert!(
        app.is_windowed(),
        "テキストモードのコマンド出力が windowed にならない"
    );
    let out_path = app
        .tab
        .command_out
        .clone()
        .expect("command_out が設定されていない");
    assert!(out_path.exists());
    assert!(std::fs::read_to_string(&out_path)
        .unwrap()
        .trim_start()
        .starts_with("hello-from-command"));

    // The title still shows the original file, never the temp path.
    let mut term = Terminal::new(TestBackend::new(70, 10)).unwrap();
    term.draw(|f| crate::ui::preview::render(f, &mut app, f.area()))
        .unwrap();
    let s: String = term
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|c| c.symbol())
        .collect();
    assert!(
        s.contains("report.custom"),
        "タイトルに元ファイル名が出ない: {s}"
    );
    assert!(
        !s.contains(&out_path.to_string_lossy().to_string()),
        "タイトルに一時ファイルパスが漏れている: {s}"
    );

    // Moving to a different preview deletes the temp output.
    app.enter_preview(&other);
    assert!(
        !out_path.exists(),
        "別プレビューに移っても一時ファイルが残っている"
    );
    assert!(app.tab.command_out.is_none());

    std::fs::remove_dir_all(&dir).ok();
}

// ---- inline Markdown images: remote fetch + raster/SVG helpers (non-network) ----

#[test]
fn resolve_md_image_path_remote_uncached_and_data_are_none() {
    // A remote URL that has never been fetched has no cache file → None (renderer shows "loading").
    assert!(
        resolve_md_image_path("https://konoma.example/never-fetched-\u{2603}.png", None).is_none(),
        "未取得のリモートは None"
    );
    // data: URLs are never inlined.
    assert!(resolve_md_image_path("data:image/png;base64,AAAA", None).is_none());
}

#[test]
fn md_remote_cache_path_is_stable_and_url_specific() {
    let a1 = md_remote_cache_path("https://example.com/a.png");
    let a2 = md_remote_cache_path("https://example.com/a.png");
    let b = md_remote_cache_path("https://example.com/b.png");
    // Requires a cache root (HOME / XDG_CACHE_HOME); present in normal environments.
    if let (Some(a1), Some(a2), Some(b)) = (a1, a2, b) {
        assert_eq!(a1, a2, "同一 URL は同一パス");
        assert_ne!(a1, b, "異なる URL は異なるパス");
        assert!(
            a1.to_string_lossy().contains("remote-images"),
            "remote-images 配下: {a1:?}"
        );
    }
}

/// A malformed URL fails inside `ureq`'s own request construction (bad scheme / no host) — this
/// returns before any DNS lookup or socket is ever opened, so it exercises the failure path without
/// making a real network call (per the "no tests that reach the network" rule: a `.invalid`/loopback
/// URL would still trigger a DNS/connect attempt, which is why this uses a syntactically-invalid URL
/// instead). No `.part` file is left behind.
#[test]
fn fetch_remote_image_malformed_url_fails_without_touching_the_network() {
    let dir = unique_tmp("konoma_fetch_remote_malformed_test");
    std::fs::create_dir_all(&dir).unwrap();
    let dest = dir.join("out.png");
    assert!(!fetch_remote_image("not a url at all", &dest));
    assert!(!dest.exists());
    assert!(!dest.with_extension("part").exists());
    std::fs::remove_dir_all(&dir).ok();
}

/// Once the remote-image disk cache exceeds its cap, the oldest entries are deleted first
/// (prevents unbounded growth). `.part` (an in-progress temp file) is excluded.
#[test]
fn prune_remote_cache_keeps_newest_and_skips_part() {
    let dir = std::env::temp_dir().join("konoma_prune_remote_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // Create 5 cache files with mtimes in ascending (oldest-first) order.
    for i in 0..5 {
        let p = dir.join(format!("img{i}"));
        std::fs::write(&p, [i as u8]).unwrap();
        let t = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1000 + i as u64);
        filetime_set(&p, t);
    }
    // Leave the in-progress temp file alone.
    std::fs::write(dir.join("busy.part"), b"x").unwrap();

    prune_remote_cache(&dir, 2);

    // The 2 newest (img3/img4) remain, and the 3 oldest are removed. .part is kept.
    assert!(
        !dir.join("img0").exists() && !dir.join("img2").exists(),
        "古いものは削除"
    );
    assert!(
        dir.join("img3").exists() && dir.join("img4").exists(),
        "新しい 2 個は残る"
    );
    assert!(dir.join("busy.part").exists(), ".part は対象外で残る");

    std::fs::remove_dir_all(&dir).ok();
}

/// Set a file's mtime deterministically (test helper). `File::set_modified` is stable since 1.75
/// (MSRV 1.92), so no external `filetime` crate is needed.
fn filetime_set(path: &std::path::Path, t: std::time::SystemTime) {
    let f = std::fs::OpenOptions::new().write(true).open(path).unwrap();
    f.set_modified(t).unwrap();
}

#[test]
fn md_image_dims_reads_raster_and_svg() {
    let dir = std::env::temp_dir().join("konoma_md_dims_test");
    let _ = std::fs::create_dir_all(&dir);
    // Raster PNG (extension present).
    let png = dir.join("p.png");
    image::RgbaImage::from_pixel(4, 2, image::Rgba([1, 2, 3, 255]))
        .save(&png)
        .unwrap();
    assert_eq!(md_image_dims(&png), Some((4, 2)), "PNG の寸法");
    // SVG stored WITHOUT an extension (mimics a remote-cache file): must sniff as SVG.
    let svg = dir.join("noext");
    std::fs::write(
        &svg,
        br#"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10"></svg>"#,
    )
    .unwrap();
    assert_eq!(md_image_dims(&svg), Some((20, 10)), "拡張子なし SVG の寸法");
    // Junk is neither.
    let bad = dir.join("bad");
    std::fs::write(&bad, b"not an image").unwrap();
    assert!(md_image_dims(&bad).is_none(), "非画像は None");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn md_decode_image_decodes_raster_and_rasterizes_svg() {
    let dir = std::env::temp_dir().join("konoma_md_decode_test");
    let _ = std::fs::create_dir_all(&dir);
    let png = dir.join("p.png");
    image::RgbaImage::from_pixel(3, 3, image::Rgba([9, 9, 9, 255]))
        .save(&png)
        .unwrap();
    assert!(md_decode_image(&png, 800).is_some(), "PNG をデコード");
    // Extension-less SVG → rasterized via resvg (max side 800).
    let svg = dir.join("noext");
    std::fs::write(
        &svg,
        br##"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10"><rect width="20" height="10" fill="#0f0"/></svg>"##,
    )
    .unwrap();
    let img = md_decode_image(&svg, 800).expect("SVG をラスタライズ");
    assert_eq!((img.width(), img.height()), (800, 400), "最大辺 800 に拡大");
    assert!(md_decode_image(&dir.join("missing"), 800).is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn apply_remote_fetch_marks_failed_and_invalidates_cache() {
    let dir = std::env::temp_dir().join("konoma_apply_remote_test");
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    let url = "https://example.com/x.png".to_string();
    app.md_remote_inflight.insert(url.clone());
    // Failure: remembered as failed, removed from in-flight, decoration cache dropped.
    assert!(app.apply_remote_fetch(RemoteFetch {
        url: url.clone(),
        ok: false,
    }));
    assert!(!app.md_remote_inflight.contains(&url), "in-flight から除去");
    assert!(app.md_remote_failed.contains(&url), "失敗を記録");
    assert!(app.md_cache.is_none(), "装飾キャッシュを無効化");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn preview_survives_target_file_overwrite_and_delete() {
    // A previewed file can be overwritten or deleted out from under konoma (a script re-plots or
    // removes it). This must never panic — the preview reloads / degrades gracefully. Covers both an
    // image preview and a text/Markdown preview (not just images).
    use image::RgbImage;
    let dir = std::env::temp_dir().join("konoma_preview_file_vanish_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    // --- Image preview ---
    let pic = dir.join("plot.png");
    RgbImage::new(4, 3).save(&pic).unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    let idx = app
        .tab
        .entries
        .iter()
        .position(|e| e.path.ends_with("plot.png"))
        .unwrap();
    app.tab.selected = idx;
    app.tree_activate().unwrap();
    assert!(matches!(
        app.tab.preview_kind,
        Some(crate::preview::PreviewKind::Image(_))
    ));
    // Overwrite with different valid bytes, force the reload path — no panic.
    RgbImage::new(8, 6).save(&pic).unwrap();
    app.preview_media_mtime = None;
    let _ = app.refresh();
    // Delete the file while it is the active preview, force reload — no panic (errors are non-fatal).
    std::fs::remove_file(&pic).unwrap();
    app.preview_media_mtime = None;
    let _ = app.refresh();

    // --- Markdown/text preview ---
    let doc = dir.join("doc.md");
    std::fs::write(&doc, b"# hello\n\nbody\n").unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    let idx = app
        .tab
        .entries
        .iter()
        .position(|e| e.path.ends_with("doc.md"))
        .unwrap();
    app.tab.selected = idx;
    app.tree_activate().unwrap();
    let _ = app.decorated_lines(80);
    std::fs::remove_file(&doc).unwrap();
    let _ = app.refresh();
    // Rendering the now-missing file must not panic (degrades to a safe "can not preview" line).
    let _ = app.decorated_lines(80);

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn md_band_pixels_stays_within_image_bounds() {
    // The inline-image band crop must never exceed the image height (else crop_imm panics). Exercise
    // the top, middle, bottom, single-row, and whole-image bands over a range of sizes.
    for &dh in &[1u32, 3, 100, 1000] {
        for full_rows in [1u16, 2, 24, 60] {
            for row_off in 0..full_rows {
                for vis_rows in 1..=full_rows {
                    let (y0, h) = md_band_pixels(full_rows, row_off, vis_rows, dh);
                    assert!(h >= 1, "帯の高さは 1 以上");
                    assert!(
                        y0 + h <= dh,
                        "帯が画像外: dh={dh} full={full_rows} off={row_off} vis={vis_rows} → y0={y0} h={h}"
                    );
                }
            }
        }
    }
}

// ---- git change gutter (editor-style Zed-like markers on code/text previews) ----

#[test]
fn gutter_marks_classifies_add_modify_delete() {
    use crate::git::{DiffLine, DiffLineKind};
    let ctx = |old, new| DiffLine {
        kind: DiffLineKind::Context,
        old_no: Some(old),
        new_no: Some(new),
        text: String::new(),
    };
    let add = |new| DiffLine {
        kind: DiffLineKind::Added,
        old_no: None,
        new_no: Some(new),
        text: String::new(),
    };
    let rem = |old| DiffLine {
        kind: DiffLineKind::Removed,
        old_no: Some(old),
        new_no: None,
        text: String::new(),
    };

    // Pure insertion → Added (green).
    let m = gutter_marks(&[ctx(1, 1), add(2)]);
    assert_eq!(m.get(&2), Some(&GutterMark::Added));

    // Removed+Added in one block → the new line is Modified (blue).
    let m = gutter_marks(&[ctx(1, 1), rem(2), add(2), ctx(3, 3)]);
    assert_eq!(m.get(&2), Some(&GutterMark::Modified));

    // Pure deletion in the middle → the following line is marked Deleted (red).
    let m = gutter_marks(&[ctx(3, 3), rem(4), ctx(5, 4)]);
    assert_eq!(m.get(&4), Some(&GutterMark::Deleted));

    // Deletion at EOF (no following line) → anchor to the preceding line's number.
    let m = gutter_marks(&[ctx(3, 3), rem(4), rem(5)]);
    assert_eq!(m.get(&3), Some(&GutterMark::Deleted));

    // Unchanged → no marks (so no gutter column is shown).
    assert!(gutter_marks(&[ctx(1, 1), ctx(2, 2)]).is_empty());
}

#[test]
fn git_gutter_prepends_marker_only_when_changed() {
    let base = vec![Line::from("aaa"), Line::from("bbb")];
    // No marks → lines untouched (unchanged files / non-repos keep their exact layout).
    let out = with_git_gutter(base.clone(), 0, &std::collections::HashMap::new());
    assert_eq!(out[0].to_string(), "aaa");
    // A mark on new line 2 (top_line 0 → displayed row 1). Row 1 gets a blank cell, row 2 the bar.
    let mut marks = std::collections::HashMap::new();
    marks.insert(2u32, GutterMark::Added);
    let out = with_git_gutter(base, 0, &marks);
    assert!(out[0].to_string().starts_with(' '), "変更なし行は空セル");
    assert!(out[1].to_string().starts_with('▌'), "追加行は変更バー ▌");

    // A deletion is anchored to the line below it and drawn as a top-edge bar (▔),
    // so it reads as "removed between these rows" rather than "this row was removed".
    let base = vec![Line::from("aaa"), Line::from("bbb")];
    let mut marks = std::collections::HashMap::new();
    marks.insert(2u32, GutterMark::Deleted);
    let out = with_git_gutter(base, 0, &marks);
    assert!(
        out[1].to_string().starts_with('▔'),
        "削除位置は上端バー ▔（行間の目印）"
    );
}

/// Open a CSV as a table preview and set the cell cursor. 3 cols × 2 data rows.
fn app_with_table() -> (App, std::path::PathBuf) {
    let dir = unique_tmp("konoma_table_app_test");
    std::fs::create_dir_all(&dir).unwrap();
    let csv = dir.join("t.csv");
    std::fs::write(&csv, "h1,h2,h3\na,b,c\nd,e,f\n").unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.tab.preview_kind = Some(app.cfg.resolve_preview(&csv));
    app.tab.preview_path = Some(csv.clone());
    app.tab.mode = Mode::Preview;
    app.load_table();
    (app, csv)
}

#[test]
fn csv_resolves_to_table_preview() {
    let (app, _) = app_with_table();
    assert!(
        matches!(
            app.tab.preview_kind,
            Some(PreviewKind::Table {
                delimiter: b',',
                ..
            })
        ),
        "*.csv は Table(カンマ区切り)に解決される"
    );
    assert!(app.is_table_preview(), "パース成功でテーブル面が有効");
    assert_eq!(app.surface(), crate::keymap::Surface::PreviewTable);
    assert_eq!(app.display_mode(), crate::app::DisplayMode::Table);
}

/// Open a zip archive as a table preview (mirrors `app_with_table` but for `PreviewKind::Archive`).
fn app_with_archive_zip() -> (App, std::path::PathBuf) {
    use std::io::Write as _;
    let dir = unique_tmp("konoma_archive_app_test");
    std::fs::create_dir_all(&dir).unwrap();
    let zpath = dir.join("t.zip");
    let f = std::fs::File::create(&zpath).unwrap();
    let mut zw = zip::ZipWriter::new(f);
    let opts =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    zw.start_file("one.txt", opts).unwrap();
    zw.write_all(b"hello").unwrap();
    zw.start_file("two.txt", opts).unwrap();
    zw.write_all(b"world!!").unwrap();
    zw.finish().unwrap();

    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.tab.preview_kind = Some(app.cfg.resolve_preview(&zpath));
    app.tab.preview_path = Some(zpath.clone());
    app.tab.mode = Mode::Preview;
    app.load_table();
    (app, zpath)
}

#[test]
fn zip_resolves_to_archive_table_preview() {
    let (app, _) = app_with_archive_zip();
    assert!(
        matches!(
            app.tab.preview_kind,
            Some(PreviewKind::Archive {
                kind: crate::preview::archive::ArchiveKind::Zip,
                ..
            })
        ),
        "*.zip は Archive(Zip) に解決される"
    );
    assert!(app.is_table_preview(), "一覧化成功でテーブル面が有効");
    assert_eq!(app.surface(), crate::keymap::Surface::PreviewTable);
    assert_eq!(app.display_mode(), crate::app::DisplayMode::Table);
    let t = app.table_data().unwrap();
    assert_eq!(t.headers, vec!["Name", "Size", "Modified"]);
    assert_eq!(t.rows.len(), 2);
}

#[test]
fn corrupt_zip_degrades_to_no_table_data() {
    let dir = unique_tmp("konoma_archive_corrupt_test");
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("bad.zip");
    std::fs::write(&p, b"not a zip file at all").unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.tab.preview_kind = Some(app.cfg.resolve_preview(&p));
    app.tab.preview_path = Some(p.clone());
    app.tab.mode = Mode::Preview;
    app.load_table();
    assert!(app.table_data().is_none(), "壊れた zip はパース失敗→ None");
    assert!(
        !app.is_table_preview(),
        "テーブル面は無効(→ can not preview へ降格)"
    );
}

#[test]
fn table_cursor_moves_and_clamps() {
    let (mut app, _) = app_with_table();
    assert_eq!(app.table_cursor(), (0, 0));
    // Moving far out of range still clamps at the end (row 1, col 2).
    app.table_cursor_move(9, 9);
    assert_eq!(app.table_cursor(), (1, 2));
    // The negative direction also clamps at the start (0,0).
    app.table_cursor_move(-9, -9);
    assert_eq!(app.table_cursor(), (0, 0));
    // Row/column jump.
    app.table_row_to(true);
    app.table_col_to(true);
    assert_eq!(app.table_cursor(), (1, 2));
    app.table_row_to(false);
    app.table_col_to(false);
    assert_eq!(app.table_cursor(), (0, 0));
}

#[test]
fn table_copy_text_cell_row_column() {
    let (mut app, _) = app_with_table();
    // Put the cursor at (row 1, col 1) = "e".
    app.table_cursor_move(1, 1);
    assert_eq!(
        app.table_copy_text(TableCopyKind::Cell).as_deref(),
        Some("e")
    );
    // Row copy = that row joined by commas.
    assert_eq!(
        app.table_copy_text(TableCopyKind::Row).as_deref(),
        Some("d,e,f")
    );
    // Column copy = the header + each row's corresponding column joined by newlines.
    assert_eq!(
        app.table_copy_text(TableCopyKind::Column).as_deref(),
        Some("h2\nb\ne")
    );
}

/// Table paging: `table_page`=moves the cursor vertically by a full viewport, `table_half_page`=by
/// half a viewport, clamping at the end/start (the page-width derivation, right before it
/// delegates to `table_cursor_move`, was previously untested).
#[test]
fn table_page_and_half_page_move_by_viewport() {
    let dir = unique_tmp("konoma_table_page_test");
    std::fs::create_dir_all(&dir).unwrap();
    let csv = dir.join("tall.csv");
    let mut body = String::from("c0,c1,c2\n");
    for i in 0..20 {
        body.push_str(&format!("r{i}a,r{i}b,r{i}c\n"));
    }
    std::fs::write(&csv, body).unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.tab.preview_kind = Some(app.cfg.resolve_preview(&csv));
    app.tab.preview_path = Some(csv.clone());
    app.tab.mode = Mode::Preview;
    app.load_table();
    // Pin the viewport row count for the test (normally decided at render time).
    app.table_viewport_rows = 6;

    assert_eq!(app.table_cursor(), (0, 0));
    app.table_page(1); // down by 1 viewport (6 rows).
    assert_eq!(app.table_cursor().0, 6, "1ページ = viewport 行ぶん下");
    app.table_half_page(1); // down by half a viewport (3 rows).
    assert_eq!(app.table_cursor().0, 9, "半ページ = viewport/2 行ぶん下");
    app.table_page(-1); // up by 1 viewport.
    assert_eq!(app.table_cursor().0, 3);
    // Clamped at the end/start (20 data rows → last index 19).
    app.table_page(1);
    app.table_page(1);
    app.table_page(1);
    assert_eq!(app.table_cursor().0, 19, "末尾でクランプ");
    for _ in 0..10 {
        app.table_page(-1);
    }
    assert_eq!(app.table_cursor().0, 0, "先頭でクランプ");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn table_cursor_survives_tab_roundtrip() {
    // The cell cursor is kept and the table is re-parsed across tab save/restore.
    let (mut app, _) = app_with_table();
    app.table_cursor_move(1, 2);
    app.tab_new().unwrap(); // saves the current tab and moves to a new one
    app.tab_cycle(-1); // back to the original tab (load_active re-parses the table + restores the cursor)
    assert!(
        app.is_table_preview(),
        "復元後もテーブルとして再パースされる"
    );
    assert_eq!(
        app.table_cursor(),
        (1, 2),
        "セルカーソルがタブ跨ぎで保たれる"
    );
}

// --- Table cell full-text popup (`Enter`): a read-only view filling the gap left by the
//     truncated grid ---

/// The grid truncates cells exceeding the column width (`MAX_COL_W`=40) with `…`, but
/// `table_cell_view()` (like `y→c` copy) always returns the raw cell value = the full text can
/// be obtained even on screen. Doesn't break with CJK mixed in either.
#[test]
fn table_cell_view_returns_untruncated_long_and_cjk_text() {
    let dir = unique_tmp("konoma_cellview_long_test");
    std::fs::create_dir_all(&dir).unwrap();
    let long = "x".repeat(200); // a length far beyond the grid's MAX_COL_W (40).
    let cjk = "あいうえおかきくけこ".repeat(5); // 50 full-width chars = width 100 (also over 40).
    let csv = dir.join("t.csv");
    std::fs::write(&csv, format!("h1,h2\n{long},{cjk}\n")).unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.tab.preview_kind = Some(app.cfg.resolve_preview(&csv));
    app.tab.preview_path = Some(csv.clone());
    app.tab.mode = Mode::Preview;
    app.load_table();

    let view = app.table_cell_view().expect("table_cell_view");
    assert_eq!(view.header, "h1");
    assert_eq!(view.row, 1);
    assert_eq!(view.col, 1);
    assert_eq!(view.nrows, 1);
    assert_eq!(view.ncols, 2);
    assert_eq!(view.text, long, "長いセルが切り詰めなしで取れる");

    app.table_cursor_move(0, 1);
    let view = app.table_cell_view().expect("table_cell_view");
    assert_eq!(view.header, "h2");
    assert_eq!(view.col, 2);
    assert_eq!(view.text, cjk, "CJK セルも全文がそのまま取れる");
    std::fs::remove_dir_all(&dir).ok();
}

/// CSV can hold real newlines inside quotes (e.g. a multi-line address field). The popup's body
/// returns the raw cell value (keeping the `\n`s) and does not go through the grid's `flatten`
/// (which smooths them out to spaces).
#[test]
fn table_cell_view_preserves_embedded_newlines() {
    let dir = unique_tmp("konoma_cellview_newline_test");
    std::fs::create_dir_all(&dir).unwrap();
    let csv = dir.join("t.csv");
    std::fs::write(&csv, "name,address\nDoe,\"123 Main St\nSuite 4\nCity\"\n").unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.tab.preview_kind = Some(app.cfg.resolve_preview(&csv));
    app.tab.preview_path = Some(csv.clone());
    app.tab.mode = Mode::Preview;
    app.load_table();
    app.table_cursor_move(0, 1);

    let view = app.table_cell_view().expect("table_cell_view");
    assert_eq!(view.text, "123 Main St\nSuite 4\nCity");
    assert_eq!(
        view.text.lines().count(),
        3,
        "実改行3行がそのまま数えられる"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// Non-UTF-8 byte sequences are already replaced with U+FFFD by the csv crate's lossy decoding
/// (preview/table.rs). The popup side just displays that as-is; verify it doesn't crash.
#[test]
fn table_cell_view_handles_replacement_chars_without_panic() {
    let dir = unique_tmp("konoma_cellview_badbytes_test");
    std::fs::create_dir_all(&dir).unwrap();
    let csv = dir.join("t.csv");
    let mut bytes = b"h1,h2\n".to_vec();
    bytes.extend_from_slice(b"a\xff\xfeb,c\n"); // one row containing invalid bytes.
    std::fs::write(&csv, bytes).unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.tab.preview_kind = Some(app.cfg.resolve_preview(&csv));
    app.tab.preview_path = Some(csv.clone());
    app.tab.mode = Mode::Preview;
    app.load_table();

    let view = app.table_cell_view().expect("table_cell_view");
    assert!(
        view.text.contains('\u{FFFD}'),
        "不正バイトは置換文字になっている: {:?}",
        view.text
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// `Enter` (toggle): opening resets the scroll to the top, `is_table_cell_open` reflects it,
/// and pressing it again closes it.
#[test]
fn toggle_table_cell_view_opens_resets_scroll_and_closes() {
    let (mut app, _) = app_with_table();
    assert!(!app.is_table_cell_open());
    assert_eq!(app.internal_mode(), None);

    app.table_cell_scroll_by(5); // simulates leftovers from before (meaningless while closed, but the value sticks around).
    app.toggle_table_cell_view();
    assert!(app.is_table_cell_open());
    assert_eq!(app.table_cell_scroll(), 0, "開いた時点でスクロールは先頭へ");
    assert_eq!(app.surface(), crate::keymap::Surface::TableCell);
    assert_eq!(app.internal_mode(), Some(InternalMode::TableCell));

    app.toggle_table_cell_view();
    assert!(!app.is_table_cell_open());
    assert_eq!(app.surface(), crate::keymap::Surface::PreviewTable);
    assert_eq!(app.internal_mode(), None);
}

/// A file with no columns (empty) has no cell to display: flash and don't open.
#[test]
fn toggle_table_cell_view_flashes_on_empty_table() {
    let dir = unique_tmp("konoma_cellview_empty_test");
    std::fs::create_dir_all(&dir).unwrap();
    let csv = dir.join("empty.csv");
    std::fs::write(&csv, "").unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.tab.preview_kind = Some(app.cfg.resolve_preview(&csv));
    app.tab.preview_path = Some(csv.clone());
    app.tab.mode = Mode::Preview;
    app.load_table();
    assert!(app.table_cell_view().is_none());

    app.toggle_table_cell_view();
    assert!(!app.is_table_cell_open(), "表示するセルが無いので開かない");
    assert!(app.flash.is_some(), "no-cell flash が出る");
    std::fs::remove_dir_all(&dir).ok();
}

/// Pure scroll movement/clamping: clamps to 0 at the top, `scroll_to` goes to the start/end
/// (u16::MAX, per the convention that it's clamped at render time), and `table_cell_page` treats
/// the viewport width last written back by the renderer as one page.
#[test]
fn table_cell_scroll_by_to_and_page_move_and_clamp_floor() {
    let (mut app, _) = app_with_table();
    app.toggle_table_cell_view();
    assert_eq!(app.table_cell_scroll(), 0);

    app.table_cell_scroll_by(-3);
    assert_eq!(
        app.table_cell_scroll(),
        0,
        "上端で0にクランプ(負にならない)"
    );

    app.table_cell_scroll_by(7);
    assert_eq!(app.table_cell_scroll(), 7);

    app.table_cell_scroll_to(true);
    assert_eq!(
        app.table_cell_scroll(),
        u16::MAX,
        "末尾は描画時にクランプされる規約(preview_scroll と同型)"
    );
    app.table_cell_scroll_to(false);
    assert_eq!(app.table_cell_scroll(), 0);

    // Page using the viewport width (= 1 page's height) that the renderer wrote back.
    app.set_table_cell_view(0, 11); // viewport=11 rows → 1 page=10 rows (same -1 overlap as preview_page).
    app.table_cell_page(1);
    assert_eq!(app.table_cell_scroll(), 10);
    app.table_cell_page(-1);
    assert_eq!(app.table_cell_scroll(), 0);
}

/// The archive listing goes through the same `TableData`, so the cell popup works automatically (verify).
#[test]
fn table_cell_view_works_for_archive_listing() {
    let (mut app, _) = app_with_archive_zip();
    let view = app.table_cell_view().expect("table_cell_view");
    assert_eq!(view.header, "Name");
    assert_eq!(view.text, "one.txt");
    app.table_cursor_move(1, 0);
    let view = app.table_cell_view().expect("table_cell_view");
    assert_eq!(view.text, "two.txt");

    app.toggle_table_cell_view();
    assert!(
        app.is_table_cell_open(),
        "アーカイブ一覧でもポップアップが開く"
    );
}

/// The popup doesn't carry over across a tab switch/new tab (same App-global overlay convention as Outline).
#[test]
fn table_cell_popup_does_not_survive_tab_switch_or_new_tab() {
    let (mut app, _) = app_with_table();
    app.toggle_table_cell_view();
    assert!(app.is_table_cell_open());

    app.tab_new().unwrap(); // a new tab always starts with no popup (tab_new explicitly resets it).
    assert!(
        !app.is_table_cell_open(),
        "新規タブは常にポップアップ無しから始まる"
    );

    // A separate session: leaving it open, switching away, and coming back must not leave it open.
    let (mut app2, _) = app_with_table();
    app2.toggle_table_cell_view();
    assert!(app2.is_table_cell_open());
    app2.tab_new().unwrap();
    app2.tab_cycle(-1); // back to the original tab (load_active)
    assert!(
        !app2.is_table_cell_open(),
        "タブ切替(load_active)でもポップアップは閉じる"
    );
}

// --- Agent Watch: @ref copy (3) / changed filter + jump (1) / follow mode (2) ---

#[test]
fn at_ref_is_strictly_relative_to_open_dir() {
    let work = std::env::temp_dir().join("konoma_at_ref_test");
    let _ = std::fs::remove_dir_all(&work);
    let a = work.join("A");
    std::fs::create_dir_all(&a).unwrap();
    // Under it: doesn't prefix the launch dir name (matches Claude Code's cwd-relative @ref).
    assert_eq!(
        at_ref_text(&a, &a.join("src").join("main.rs")),
        "@src/main.rs"
    );
    // Outside it (a sibling): relative with `..`.
    assert_eq!(at_ref_text(&a, &work.join("B").join("x.md")), "@../B/x.md");
    std::fs::remove_dir_all(&work).ok();
}

#[test]
fn preview_selection_ref_formats_caret_and_ranges() {
    let dir = std::env::temp_dir().join("konoma_sel_ref_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("notes.txt");
    std::fs::write(&file, b"l1\nl2\nl3\nl4\nl5\n").unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.enter_preview(&file);
    assert!(app.is_windowed(), "テキストは windowed");

    // No selection: the caret line (1-based).
    app.tab.preview_cursor_line = 2;
    assert_eq!(
        app.preview_selection_ref_text().as_deref(),
        Some("@notes.txt#L3")
    );
    // A linewise selection, lines 3..=5.
    app.preview_enter_visual(true);
    app.tab.preview_cursor_line = 4;
    assert_eq!(
        app.preview_selection_ref_text().as_deref(),
        Some("@notes.txt#L3-5")
    );
    app.preview_exit_visual();
    // A charwise selection rounds to a line span (lines 2..=4).
    app.tab.preview_cursor_line = 1;
    app.tab.preview_cursor_col = 1;
    app.preview_enter_visual(false);
    app.tab.preview_cursor_line = 3;
    app.tab.preview_cursor_col = 0;
    assert_eq!(
        app.preview_selection_ref_text().as_deref(),
        Some("@notes.txt#L2-4")
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[cfg(feature = "git")]
#[test]
fn changed_filter_lists_changed_files_flat_and_toggles_back() {
    let dir = std::env::temp_dir().join("konoma_changed_filter_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    git_repo_with_commits(&dir); // a.txt is already committed
    let canon = dir.canonicalize().unwrap();
    // Create changes: edit a.txt (M) + an untracked file (U) under sub/, which will be collapsed.
    std::fs::write(canon.join("a.txt"), b"changed\n").unwrap();
    std::fs::create_dir_all(canon.join("sub")).unwrap();
    std::fs::write(canon.join("sub").join("b.txt"), b"new\n").unwrap();

    let mut app = App::new(canon.clone(), Config::default()).unwrap();
    assert!(!app.changed_filter());
    app.toggle_changed_filter();
    assert!(app.changed_filter(), "変更があるので ON になる");
    let paths: Vec<PathBuf> = app.tab.entries.iter().map(|e| e.path.clone()).collect();
    assert_eq!(
        paths,
        vec![canon.join("a.txt"), canon.join("sub").join("b.txt")],
        "root 配下の変更ファイルのみ・パス順のフラット一覧"
    );
    assert!(app.tab.entries.iter().all(|e| !e.is_dir), "ファイルのみ");

    // Enter from the list opens preview (the same behavior as the filter).
    app.tab.selected = 0;
    app.tree_activate().unwrap();
    assert!(matches!(app.tab.mode, Mode::Preview));
    assert_eq!(
        app.tab.preview_path.as_deref(),
        Some(canon.join("a.txt").as_path())
    );
    app.back_to_tree();

    // h returns to the normal tree (doesn't move root up).
    app.tree_leave().unwrap();
    assert!(!app.changed_filter());
    assert_eq!(
        app.tab.root, canon,
        "変更フィルタ解除であって親移動ではない"
    );
    assert!(
        app.tab.entries.iter().any(|e| e.is_dir),
        "通常ツリーに戻る(ディレクトリ行が復活)"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[cfg(feature = "git")]
#[test]
fn jump_changed_reveals_collapsed_targets_and_wraps() {
    let dir = std::env::temp_dir().join("konoma_jump_changed_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    git_repo_with_commits(&dir);
    let canon = dir.canonicalize().unwrap();
    std::fs::write(canon.join("a.txt"), b"changed\n").unwrap();
    std::fs::create_dir_all(canon.join("sub")).unwrap();
    std::fs::write(canon.join("sub").join("b.txt"), b"new\n").unwrap();

    let mut app = App::new(canon.clone(), Config::default()).unwrap();
    app.tab.selected = 0; // with the default sort (dirs_first), sub is first = not on a changed file
    app.jump_changed(1);
    assert_eq!(
        app.tab.entries[app.tab.selected].path,
        canon.join("sub").join("b.txt"),
        "collapsed な sub/ を展開して選択(deep reveal)"
    );
    app.jump_changed(1);
    assert_eq!(
        app.tab.entries[app.tab.selected].path,
        canon.join("a.txt"),
        "末尾の次は先頭へ wrap"
    );
    app.jump_changed(-1);
    assert_eq!(
        app.tab.entries[app.tab.selected].path,
        canon.join("sub").join("b.txt"),
        "N は逆順"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn follow_jump_reveals_and_previews_only_valid_targets() {
    let dir = std::env::temp_dir().join("konoma_follow_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    std::fs::create_dir_all(dir.join(".hidden")).unwrap();
    std::fs::write(dir.join("a.txt"), b"a\n").unwrap();
    std::fs::write(dir.join("sub").join("b.txt"), b"b\n").unwrap();
    std::fs::write(dir.join(".hidden").join("c.txt"), b"c\n").unwrap();
    let outside = std::env::temp_dir().join("konoma_follow_outside.txt");
    std::fs::write(&outside, b"x\n").unwrap();

    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    // Does nothing while OFF.
    app.follow_jump(&dir.join("a.txt"));
    assert!(matches!(app.tab.mode, Mode::Tree));

    app.toggle_follow();
    assert!(app.follow_enabled());
    // Skipped: outside root, under a hidden directory, or a directory (state unchanged).
    app.follow_jump(&outside);
    app.follow_jump(&dir.join(".hidden").join("c.txt"));
    app.follow_jump(&dir.join("sub"));
    assert!(
        matches!(app.tab.mode, Mode::Tree),
        "無効ターゲットでは動かない"
    );

    // Even under a collapsed sub/, deep-reveal it into preview.
    app.follow_jump(&dir.join("sub").join("b.txt"));
    assert!(matches!(app.tab.mode, Mode::Preview));
    assert_eq!(
        app.tab.preview_path.as_deref(),
        Some(dir.join("sub").join("b.txt").as_path())
    );
    // Stays on the Preview surface and follows even when another file's change arrives.
    app.follow_jump(&dir.join("a.txt"));
    assert_eq!(
        app.tab.preview_path.as_deref(),
        Some(dir.join("a.txt").as_path())
    );

    // Turned off once the user takes the keyboard back (with a flash).
    app.follow_break();
    assert!(!app.follow_enabled());
    assert!(app.flash.is_some(), "解除は flash で見せる");

    std::fs::remove_dir_all(&dir).ok();
    std::fs::remove_file(&outside).ok();
}

#[cfg(feature = "git")]
#[test]
fn follow_jump_scrolls_to_first_changed_hunk() {
    // Only deep inside a large file (line 100) is changed → opening it via follow makes **the
    // changed line land within the window** (showing from the top would leave the change
    // off-screen = a regression guard for P1). 3 lines of context above; the caret is the
    // changed line.
    let dir = std::env::temp_dir().join("konoma_follow_scroll_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    init_git_repo(&dir);
    let big = dir.join("big.txt");
    let mut lines: Vec<String> = (1..=200).map(|i| format!("line {i:03}")).collect();
    std::fs::write(&big, lines.join("\n")).unwrap();
    let sh = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .current_dir(&dir)
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?} 失敗");
    };
    sh(&["add", "-A"]);
    sh(&["commit", "-m", "big"]);
    lines[99] = "line 100 CHANGED".into(); // change the 1-based 100th line
    std::fs::write(&big, lines.join("\n")).unwrap();

    let canon = dir.canonicalize().unwrap();
    let mut app = App::new(canon.clone(), Config::default()).unwrap();
    app.cfg.ui.follow_view = "file".into(); // this test targets the file-view mode's behavior (the default is diff)
    app.toggle_follow();
    app.follow_jump(&canon.join("big.txt"));
    assert!(matches!(app.tab.mode, Mode::Preview));
    assert_eq!(
        app.tab.preview_top_line, 96,
        "変更行(0-based 99)の3行上が窓の先頭"
    );
    assert_eq!(app.tab.preview_cursor_line, 99, "キャレットは変更行");
    assert!(app.tab.preview_byte_top > 0, "窓はファイル先頭ではない");

    // An unchanged file (no change hunks) still starts from the top, as before.
    // (In real usage, refresh_fs always runs before follow_jump so the new file lands in the
    // tree = we call it in the same order here.)
    let plain = canon.join("plain.txt");
    std::fs::write(&plain, b"p1\np2\n").unwrap();
    app.refresh().unwrap();
    app.follow_jump(&plain);
    assert_eq!(app.tab.preview_path.as_deref(), Some(plain.as_path()));
    assert_eq!(app.tab.preview_top_line, 0);
    std::fs::remove_dir_all(&dir).ok();
}

#[cfg(feature = "git")]
#[test]
fn follow_jump_opens_diff_view_by_default_and_falls_back() {
    // By default (ui.follow_view="diff"), a tracked changed file opens in **full-screen diff**.
    // Untracked (can't produce a diff = all lines are new) falls back to a file preview.
    // The baseline diff shows changes "since F" (previously: dirty vs HEAD). So to get a diff,
    // the file must change after F (= real usage: press F, then the AI edits it).
    let dir = std::env::temp_dir().join("konoma_follow_diff_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    git_repo_with_commits(&dir); // a.txt is already committed
    let canon = dir.canonicalize().unwrap();
    let sh = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .current_dir(&dir)
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?} 失敗");
    };
    // A file already committed before F and never touched afterward (for verifying the fallback).
    std::fs::write(canon.join("keep.txt"), b"committed\n").unwrap();
    sh(&["add", "keep.txt"]);
    sh(&["commit", "-m", "keep"]);

    let mut app = App::new(canon.clone(), Config::default()).unwrap();
    assert_eq!(app.cfg.ui.follow_view, "diff", "既定は diff 表示");
    app.toggle_follow(); // F: pins this instant as the baseline (a.txt/keep.txt are clean)

    // Edit a.txt after F → tracked + has changes since F → a GitDiff preview.
    std::fs::write(canon.join("a.txt"), b"one\nCHANGED\n").unwrap();
    app.follow_jump(&canon.join("a.txt"));
    assert!(
        app.is_git_diff_preview(),
        "F 以降に変わったファイルは diff で開く"
    );
    assert_eq!(
        app.tab.preview_path.as_deref(),
        Some(canon.join("a.txt").as_path())
    );

    // A newly-created untracked file after F also opens with an all-added diff (following
    // continues from the diff view). In real usage, an FS event refreshes (rebuilds the tree)
    // before jumping → interleave a refresh here too.
    std::fs::write(canon.join("fresh.txt"), b"new file\n").unwrap();
    app.refresh().unwrap();
    app.follow_jump(&canon.join("fresh.txt"));
    assert!(
        app.is_git_diff_preview(),
        "F 後の新規ファイルも diff で開く"
    );
    assert_eq!(
        app.tab.preview_path.as_deref(),
        Some(canon.join("fresh.txt").as_path()),
        "diff ビューから次の変更ファイルへ切替わる"
    );
    // q returns to the tree (it did not come from the git hub).
    app.close_git_diff();
    assert!(matches!(app.tab.mode, Mode::Tree));

    // A file unchanged since F (an empty baseline diff) falls back to a file preview.
    app.follow_jump(&canon.join("keep.txt"));
    assert!(
        !app.is_git_diff_preview(),
        "F 以降に変更なし → ファイル表示へフォールバック"
    );
    assert!(matches!(app.tab.mode, Mode::Preview));
    assert!(app.is_windowed());
    assert_eq!(
        app.tab.preview_path.as_deref(),
        Some(canon.join("keep.txt").as_path())
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[cfg(feature = "git")]
#[test]
fn diff_view_n_switches_changed_files_and_keeps_return_target() {
    // n/N inside the diff view: switches to the next/previous changed file's diff without
    // leaving the view (wraps, shows position, syncs the tree cursor, and also saves q's
    // return target).
    let dir = std::env::temp_dir().join("konoma_diff_nav_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    git_repo_with_commits(&dir); // a.txt is already committed
    let canon = dir.canonicalize().unwrap();
    std::fs::write(canon.join("a.txt"), b"CHANGED\n").unwrap();
    std::fs::write(canon.join("b.txt"), b"new b\n").unwrap();
    std::fs::write(canon.join("e.txt"), b"new e\n").unwrap();

    let mut app = App::new(canon.clone(), Config::default()).unwrap();
    app.refresh_git_if_needed(); // in real usage the first render already fetched this (the position display comes from statuses)
    app.open_git_diff(&canon.join("a.txt"));
    assert_eq!(app.diff_change_position(), Some((1, 3)));

    app.jump_changed(1); // a → b
    assert!(app.is_git_diff_preview(), "diff ビューのまま");
    assert_eq!(
        app.tab.preview_path.as_deref(),
        Some(canon.join("b.txt").as_path())
    );
    assert_eq!(app.diff_change_position(), Some((2, 3)));
    assert_eq!(
        app.tab.entries[app.tab.selected].path,
        canon.join("b.txt"),
        "ツリーカーソルも同期"
    );

    app.jump_changed(1); // b → e
    app.jump_changed(1); // e → a (wrap)
    assert_eq!(
        app.tab.preview_path.as_deref(),
        Some(canon.join("a.txt").as_path())
    );
    app.jump_changed(-1); // a → e (reverse-direction wrap)
    assert_eq!(
        app.tab.preview_path.as_deref(),
        Some(canon.join("e.txt").as_path())
    );

    // The marker for having opened via the hub (came_from_git_view) survives cycling around,
    // and q returns to the hub.
    app.tab.came_from_git_view = true;
    app.jump_changed(1); // e → a
    assert!(app.tab.came_from_git_view, "戻り先(ハブ)が回遊で失われない");
    app.close_git_diff();
    assert!(app.is_git_view(), "q でハブへ戻る");
    std::fs::remove_dir_all(&dir).ok();
}

#[cfg(feature = "git")]
#[test]
fn follow_diff_n_cycles_only_session_files_and_clears_flash() {
    // n/N on a follow-originated diff cycles only through "files that changed during the
    // follow session" (not all uncommitted changes in the working tree). The position display
    // is also session-relative.
    let dir = std::env::temp_dir().join("konoma_follow_session_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    git_repo_with_commits(&dir); // a.txt is already committed
    let canon = dir.canonicalize().unwrap();
    // Set up 5 uncommitted changes (representing pre-existing work) — only b and d change
    // during the session.
    std::fs::write(canon.join("a.txt"), b"CHANGED\n").unwrap();
    for name in ["b.txt", "c.txt", "d.txt", "e.txt"] {
        std::fs::write(canon.join(name), format!("new {name}\n")).unwrap();
    }

    let mut app = App::new(canon.clone(), Config::default()).unwrap();
    app.refresh_git_if_needed();
    app.toggle_follow();
    assert!(app.follow_session.is_empty(), "F ON でセッションは空から");

    // b and d change after F (the baseline diff is non-empty → can be opened as a diff).
    std::fs::write(canon.join("b.txt"), b"new b.txt\nAFTER-FOLLOW\n").unwrap();
    std::fs::write(canon.join("d.txt"), b"new d.txt\nAFTER-FOLLOW\n").unwrap();

    // Equivalent to draining: records into the session (only true for valid targets).
    let outside = std::env::temp_dir().join("konoma_follow_session_outside.txt");
    std::fs::write(&outside, b"x\n").unwrap();
    assert!(!app.follow_note_change(&outside), "root 外は記録しない");
    assert!(app.follow_note_change(&canon.join("b.txt")));
    assert!(app.follow_note_change(&canon.join("d.txt")));
    assert!(
        app.follow_note_change(&canon.join("d.txt")),
        "重複は許容(再変更)"
    );
    assert_eq!(app.follow_session.len(), 2, "重複は積まない");

    // Jumping clears the old flash (e.g. "follow: on") = the footer switches to the diff hint.
    app.flash = Some("stale flash".into());
    app.follow_jump(&canon.join("d.txt"));
    assert!(app.is_git_diff_preview());
    assert!(app.flash.is_none(), "ジャンプ成立で flash を消す");
    assert_eq!(
        app.diff_change_position(),
        Some((2, 2)),
        "位置表示はセッション(2件)基準=全 git 変更(5件)ではない"
    );

    // n cycles only within the session (wrap). It never goes to c/e.txt (changes outside the session).
    app.jump_changed(1);
    assert_eq!(
        app.tab.preview_path.as_deref(),
        Some(canon.join("b.txt").as_path())
    );
    assert_eq!(app.diff_change_position(), Some((1, 2)));
    app.jump_changed(1);
    assert_eq!(
        app.tab.preview_path.as_deref(),
        Some(canon.join("d.txt").as_path())
    );

    // Re-pressing F resets the session.
    app.toggle_follow();
    app.toggle_follow();
    assert!(app.follow_session.is_empty(), "次の ON で新セッション");

    std::fs::remove_dir_all(&dir).ok();
    std::fs::remove_file(&outside).ok();
}

#[test]
fn collapse_links_folds_table_hidden_targets_in_document_order() {
    // A table cell's "label + hidden target" pair gets collected into targets **interleaved by
    // appearance order** with normal links (the label (URL) form) (this is decorate_links's zip
    // alignment = a misalignment would misfire every link). A table link doesn't get an icon
    // (doesn't change width = keeps column alignment), and the hidden span disappears from the
    // display row.
    let md =
        "See [first](a.md) here.\n\n| doc |\n|---|\n| [table](b.md) |\n\nAlso [last](c.md) end.\n";
    let style = crate::preview::markdown::CodeStyle {
        bg: None,
        label_bg: None,
        label_right: true,
        tab_width: 4,
        wrap: true,
    };
    let lines = crate::preview::markdown::render_markdown(md, 60, style, "TwoDark", false);
    let (collapsed, targets) = collapse_links(lines, true); // even with icons=true (the default), the table's width is unchanged
    assert_eq!(
        targets,
        vec!["a.md".to_string(), "b.md".to_string(), "c.md".to_string()],
        "targets は出現順(段落→表→段落)"
    );
    // The table link's label stays "table" (no icon), styled as a link.
    assert!(
        collapsed.iter().any(|l| l
            .spans
            .iter()
            .any(|sp| sp.content.as_ref() == "table" && is_link_span(sp))),
        "表リンクのラベルがリンク様式で残る"
    );
    // The hidden target span doesn't remain in the display row.
    assert!(
        collapsed.iter().all(|l| l
            .spans
            .iter()
            .all(|sp| !crate::preview::markdown::is_hidden_link_target(sp))),
        "隠しスパンは除去される"
    );
    // Total link-span count == targets count (the assumption decorate_links pairs them in order).
    let n_links: usize = collapsed
        .iter()
        .map(|l| l.spans.iter().filter(|sp| is_link_span(sp)).count())
        .sum();
    assert_eq!(n_links, targets.len());
}

#[test]
fn md_row_prefix_matches_full_document_reflow() {
    // The premise of visible-range slice rendering (md_layout/md_slice) is: "the per-line sum of
    // line_count = the full document's line_count" (ratatui's Wrap carries no state across
    // lines). If that breaks, scroll clamping and the slice start position drift from what a
    // full-document render would produce, so we pin it down with varied content.
    use ratatui::text::Text;
    use ratatui::widgets::{Paragraph, Wrap};
    let dir = std::env::temp_dir().join("konoma_md_prefix_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut md = String::from("# Title\n\n");
    for i in 0..40 {
        md.push_str(&format!(
            "Paragraph {i} with a [link{i}](https://example.com/{i}) and 日本語の長い文を混ぜて\
             折返しを起こす。This keeps going long enough to wrap at narrow widths.\n\n"
        ));
    }
    md.push_str("| a | b |\n|---|---|\n| 1 | 2 |\n\n- [ ] task\n\n```rust\nfn f() {}\n```\n");
    std::fs::write(dir.join("doc.md"), md.as_bytes()).unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    assert!(app.cfg.ui.wrap, "既定は wrap=on の前提");
    let idx = app
        .tab
        .entries
        .iter()
        .position(|e| e.path.ends_with("doc.md"))
        .unwrap();
    app.tab.selected = idx;
    app.tree_activate().unwrap();

    let width = 46u16;
    let lines = app.decorated_lines(width);
    let full = Paragraph::new(Text::from(lines))
        .wrap(Wrap { trim: false })
        .line_count(width);
    let (total, _) = app.md_layout(width);
    assert_eq!(total, full, "prefix 総和が全文書 reflow と一致しない");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn md_slice_render_matches_full_document_render() {
    // Rendering equivalence: a Paragraph rendered from "the visible slice + the remaining
    // scroll" (md_slice = the new implementation) matches, cell-for-cell (both glyphs and
    // styles), one rendered from "the full document + global scroll" (equivalent to the old
    // implementation). Including focus inversion. Wiring into ui::render is covered by the
    // existing e2e (screen strings) and the REVERSED buffer test.
    use ratatui::backend::TestBackend;
    use ratatui::layout::Rect;
    use ratatui::text::Text;
    use ratatui::widgets::{Paragraph, Wrap};
    use ratatui::Terminal;
    let dir = std::env::temp_dir().join("konoma_md_slice_equiv_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut md = String::from("# Doc\n\n");
    for i in 0..30 {
        md.push_str(&format!(
            "Line {i} [go{i}](./t{i}.txt) 長い日本語テキストで折返しを発生させる行 {i}。\
             wrap wrap wrap wrap wrap wrap.\n\n- [ ] task {i}\n\n"
        ));
    }
    std::fs::write(dir.join("doc.md"), md.as_bytes()).unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    let idx = app
        .tab
        .entries
        .iter()
        .position(|e| e.path.ends_with("doc.md"))
        .unwrap();
    app.tab.selected = idx;
    app.tree_activate().unwrap();

    let (iw, ih) = (58u16, 18u16); // equivalent to the frame's interior
    let area = Rect {
        x: 0,
        y: 0,
        width: iw,
        height: ih,
    };
    // Establish the cache → advance Tab focus by one so inversion equivalence is covered too.
    let (total, _) = app.md_layout(iw);
    app.md_focus_move(1);
    // Reference: equivalent to the old implementation = decorate the full document and draw it
    // with a plain global scroll.
    let full = app.decorated_lines(iw);
    let full = app.decorate_md_items(full);
    let max_v = total.saturating_sub(ih as usize) as u16;
    assert!(max_v > 17, "折返しでスクロール余地が出る前提");
    for scroll in [0u16, 3, 17, max_v] {
        let (slice, local) = app.md_slice(scroll, ih);
        let mut act_term = Terminal::new(TestBackend::new(iw, ih)).unwrap();
        act_term
            .draw(|f| {
                let para = Paragraph::new(Text::from(slice.clone()))
                    .wrap(Wrap { trim: false })
                    .scroll((local, 0));
                f.render_widget(para, area);
            })
            .unwrap();
        let actual = act_term.backend().buffer().clone();

        let mut ref_term = Terminal::new(TestBackend::new(iw, ih)).unwrap();
        ref_term
            .draw(|f| {
                let para = Paragraph::new(Text::from(full.clone()))
                    .wrap(Wrap { trim: false })
                    .scroll((scroll, 0));
                f.render_widget(para, area);
            })
            .unwrap();
        let reference = ref_term.backend().buffer().clone();
        for y in 0..ih {
            for x in 0..iw {
                let a = &actual[(x, y)];
                let r = &reference[(x, y)];
                assert_eq!(
                    (a.symbol(), a.fg, a.bg, a.modifier),
                    (r.symbol(), r.fg, r.bg, r.modifier),
                    "scroll={scroll} cell({x},{y}) がフル描画と不一致"
                );
            }
        }
    }
    std::fs::remove_dir_all(&dir).ok();
}

// ---- mermaid image rendering (v0.15 feature) --------------------------------

/// Test picker: no terminal query (protocol Halfblocks, fixed 10x20 font — enough for layout).
fn test_picker() -> ratatui_image::picker::Picker {
    ratatui_image::picker::Picker::halfblocks()
}

#[test]
fn standalone_mermaid_renders_as_image_with_vector_source() {
    let dir = std::env::temp_dir().join("konoma_mermaid_img_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("d.mmd"), "graph LR\n  A[start] --> B[end]\n").unwrap();

    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.picker = Some(test_picker());
    let _ = app.reveal_path_deep(&dir.join("d.mmd"));
    app.enter_preview(&dir.join("d.mmd"));
    // No media_tx = synchronous fallback, so the raster arrives immediately.
    assert!(
        app.image_src.is_some(),
        "画像モードで .mmd がラスタ化される"
    );
    assert!(
        app.vector_svg.is_some(),
        "SVG ソースを保持(シャープズーム用)"
    );
    assert!(app.image_logical.is_some(), "論理サイズ=初回ラスタ寸法");
    assert!(app.is_image_preview(), "IMAGE 面(ズーム/パンのキーが効く)");

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn mermaid_text_mode_keeps_legacy_rendering() {
    let dir = std::env::temp_dir().join("konoma_mermaid_txt_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("d.mmd"), "graph LR\n  A --> B\n").unwrap();

    let mut cfg = Config::default();
    cfg.ui.mermaid = "text".into();
    let mut app = App::new(dir.clone(), cfg).unwrap();
    app.picker = Some(test_picker());
    app.enter_preview(&dir.join("d.mmd"));
    assert!(app.image_src.is_none(), "text モードはラスタ化しない");
    assert!(!app.is_image_preview(), "従来どおり装飾テキスト面");

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn vector_zoom_rerasters_sharper_without_moving_geometry() {
    use image::GenericImageView;
    let dir = std::env::temp_dir().join("konoma_mermaid_zoom_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("d.mmd"), "graph TD\n  A --> B\n  B --> C\n").unwrap();

    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.picker = Some(test_picker());
    app.enter_preview(&dir.join("d.mmd"));
    let before = app.image_src.as_ref().unwrap().dimensions();
    let logical = app.image_logical.unwrap();
    assert_eq!(before, logical, "初回はラスタ寸法=論理寸法");

    // Zoom in → immediate sharp re-raster via the synchronous path. The raster grows but the
    // logical size stays fixed (image_layout measures in logical units = the on-screen size and
    // crop don't move).
    app.image_zoom_by(2.0);
    let after = app.image_src.as_ref().unwrap().dimensions();
    assert!(
        after.0 > before.0 || after.1 > before.1,
        "ズームで密度が上がる: {before:?} -> {after:?}"
    );
    assert_eq!(app.image_logical.unwrap(), logical, "論理サイズは不変");
    assert!(app.tab.image_zoom > 1.9, "ユーザー向けズーム値は保たれる");

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn md_fence_becomes_inline_diagram_and_opens_full_screen() {
    let dir = std::env::temp_dir().join("konoma_mermaid_fence_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let md = dir.join("doc.md");
    std::fs::write(
        &md,
        "# title\n\n```mermaid\ngraph LR\n  A --> B\n```\n\ntail\n",
    )
    .unwrap();

    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.picker = Some(test_picker());
    app.enter_preview(&md);
    // Build the decoration cache (synchronous fallback = the fence renders immediately too).
    // The first pass builds it while Loading → it arrives, invalidating md_cache → run 2 passes to rebuild.
    app.ensure_md_cache(80);
    app.ensure_md_cache(80);
    let imgs = app.md_images();
    assert_eq!(imgs.len(), 1, "フェンスがインライン図 placement になる");
    assert!(crate::preview::markdown::is_mermaid_fence_url(&imgs[0].url));
    // The diagram lands as a Tab item.
    let has_fence_item = app
        .md_items
        .iter()
        .any(|it| matches!(it.kind, MdItemKind::MermaidFence { .. }));
    assert!(has_fence_item, "図が Tab 巡回に載る");

    // Focus the fence → Enter → the full-screen diagram (a zoom surface).
    let idx = app
        .md_items
        .iter()
        .position(|it| matches!(it.kind, MdItemKind::MermaidFence { .. }))
        .unwrap();
    app.tab.focused_item = Some(idx);
    app.tab.preview_scroll = 3;
    app.md_activate_focused().unwrap();
    assert!(
        matches!(app.tab.preview_kind, Some(PreviewKind::MermaidFence(0))),
        "全画面フェンスビューへ"
    );
    assert!(app.image_src.is_some(), "図がラスタ化されている");
    assert!(app.is_image_preview());

    // q (back_to_tree) = returns to the md, restoring scroll/focus.
    app.back_to_tree();
    assert!(
        matches!(app.tab.preview_kind, Some(PreviewKind::Markdown(_))),
        "ツリーでなく md プレビューへ戻る"
    );
    assert!(matches!(app.tab.mode, Mode::Preview));
    assert_eq!(app.tab.preview_scroll, 3, "スクロール位置を復元");
    assert_eq!(app.tab.focused_item, Some(idx), "フォーカスを復元");

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn broken_fence_degrades_to_text_diagram() {
    let dir = std::env::temp_dir().join("konoma_mermaid_broken_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let md = dir.join("doc.md");
    std::fs::write(&md, "```mermaid\nthis is not a diagram at all\n```\n").unwrap();

    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.picker = Some(test_picker());
    app.enter_preview(&md);
    app.ensure_md_cache(80);
    app.ensure_md_cache(80);
    assert!(
        app.md_images().is_empty(),
        "壊れた図は placement にならない"
    );
    // The body remains via the text path (a safe display of the raw source) = content isn't
    // lost (principle #3).
    let joined: String = app
        .md_cache
        .as_ref()
        .unwrap()
        .lines
        .iter()
        .map(|l| l.to_string())
        .collect();
    assert!(
        joined.contains("not a diagram"),
        "テキスト降格で内容が残る: {joined}"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// A fence diagram's focus "inverts only the caption span": inverting even the centering
/// indentation's whitespace produces a giant white bar (a regression caught on a real Ghostty
/// terminal). Verified deterministically against the render buffer.
#[test]
fn fence_focus_inverts_caption_span_only() {
    use ratatui::backend::TestBackend;
    use ratatui::style::Modifier;
    use ratatui::Terminal;

    let dir = std::env::temp_dir().join("konoma_mermaid_focus_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let md = dir.join("doc.md");
    std::fs::write(&md, "# t\n\n```mermaid\ngraph LR\n  A --> B\n```\n").unwrap();

    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.picker = Some(test_picker());
    app.enter_preview(&md);
    let mut term = Terminal::new(TestBackend::new(120, 40)).unwrap();
    // Render twice: the 1st pass does the synchronous render → rebuilds the cache, and the 2nd
    // pass renders with the placement included.
    term.draw(|fr| crate::ui::render(fr, &mut app)).unwrap();
    term.draw(|fr| crate::ui::render(fr, &mut app)).unwrap();
    let idx = app
        .md_items
        .iter()
        .position(|it| matches!(it.kind, MdItemKind::MermaidFence { .. }))
        .expect("フェンスが Tab アイテムに載る");
    app.tab.focused_item = Some(idx);
    term.draw(|fr| crate::ui::render(fr, &mut app)).unwrap();

    let buf = term.backend().buffer();
    let w = buf.area.width;
    let y = (0..buf.area.height)
        .find(|&y| {
            (0..w)
                .map(|x| buf[(x, y)].symbol())
                .collect::<String>()
                .contains('◇')
        })
        .expect("キャプション行が描画されている");
    let row: String = (0..w).map(|x| buf[(x, y)].symbol()).collect();
    let cap_start = match row.find('◇') {
        Some(i) => row[..i].chars().count() as u16,
        None => panic!("キャプション開始が無い; matched row {y}: {row:?}"),
    };
    // The caption (the chip) is REVERSED, while the right side of the row (the border-only
    // area) is not REVERSED = guards against the "white bar" regression where the whole row
    // inverts (the chip includes padding whitespace).
    assert!(
        buf[(cap_start + 2, y)]
            .modifier
            .contains(Modifier::REVERSED),
        "キャプションが反転していない"
    );
    assert!(
        !buf[(w - 3, y)].modifier.contains(Modifier::REVERSED),
        "行全体が反転している(白バー回帰)"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// Focusing a fence diagram scrolls **the entire diagram block** into the visible area, and a
/// focus border (a box with the caption as its title) is drawn around the outside of the image
/// (user request 2026-07-17).
#[test]
fn fence_focus_scrolls_block_and_draws_border() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    let dir = std::env::temp_dir().join("konoma_fence_scroll_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let md = dir.join("doc.md");
    let mut src = String::new();
    for i in 0..30 {
        src.push_str(&format!("filler line {i}\n\n"));
    }
    src.push_str("```mermaid\ngraph LR\n  A --> B\n```\n");
    std::fs::write(&md, src).unwrap();

    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.picker = Some(test_picker());
    app.enter_preview(&md);
    let mut term = Terminal::new(TestBackend::new(100, 20)).unwrap();
    term.draw(|fr| crate::ui::render(fr, &mut app)).unwrap();
    term.draw(|fr| crate::ui::render(fr, &mut app)).unwrap();
    assert_eq!(
        app.tab.preview_scroll, 0,
        "前提: 先頭表示・フェンスは画面外"
    );

    // Tab (the only item = the fence) → scrolls to a position that shows the entire block.
    app.md_focus_move(1);
    let caption_line = app.md_items[app.tab.focused_item.unwrap()].line;
    assert!(
        app.tab.preview_scroll as usize >= caption_line.saturating_sub(2),
        "図ブロックの先頭(キャプション)近くまでスクロールする: scroll={} caption={caption_line}",
        app.tab.preview_scroll
    );

    // Re-rendering shows the focus border (the ┌ corner and title) outside the image.
    term.draw(|fr| crate::ui::render(fr, &mut app)).unwrap();
    let buf = term.backend().buffer();
    let w = buf.area.width;
    let all: String = (0..buf.area.height)
        .map(|y| (0..w).map(|x| buf[(x, y)].symbol()).collect::<String>() + "\n")
        .collect();
    assert!(all.contains('┌'), "フォーカス枠の角が描かれる");
    assert!(
        all.contains("mermaid — Enter"),
        "枠タイトルにキャプションが載る"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// A fence diagram's encoding uses Resize::Scale = it **upscales too**, up to the reserved grid
/// (even a small raster doesn't leave a top-left-packed empty band). A photo still uses Fit as
/// before = it doesn't upscale past its natural size.
/// Regression 2026-07-17: since it used Fit (downscale-only), when the mermaid_rows/width-derived
/// grid was larger than the base raster, the diagram would sit top-left with empty bands on the
/// right/bottom (user report: "not displayed centered").
#[test]
fn encode_worker_scales_fence_diagrams_up_to_grid() {
    let picker = test_picker(); // font 10x20 / Halfblocks
    let (req_tx, req_rx) = std::sync::mpsc::channel();
    let (res_tx, res_rx) = std::sync::mpsc::channel();
    let h = std::thread::spawn(move || md_encode_worker(picker, req_rx, res_tx));
    let img = std::sync::Arc::new(image::DynamicImage::new_rgba8(200, 100)); // equivalent to 20x5 cells
    let send = |path: &str| {
        req_tx
            .send(MdEncodeRequest {
                path: std::path::PathBuf::from(path),
                key: MdEncodeKey::Full { cols: 40, rows: 10 },
                image: img.clone(),
                crop: None,
                cols: 40,
                rows: 10,
            })
            .unwrap();
    };
    send("mermaid-fence://cafe"); // a fence diagram → upscaled to the grid (40x10)
    send("/tmp/photo.png"); // a photo → stays at its natural size (20x5)
    drop(req_tx);
    let fence = res_rx.recv().unwrap();
    let photo = res_rx.recv().unwrap();
    h.join().unwrap();
    let fs = fence
        .protocol
        .expect("フェンスのエンコードは成功する")
        .size();
    assert_eq!(
        (fs.width, fs.height),
        (40, 10),
        "フェンスはグリッドを満たす"
    );
    let ps = photo.protocol.expect("写真のエンコードは成功する").size();
    assert_eq!((ps.width, ps.height), (20, 5), "写真は拡大しない");
}

/// An inline diagram's initial size **fits the display area**: target rows = min(mermaid_rows,
/// viewport-2). Only documents containing a diagram get re-laid-out when the viewport changes;
/// documents without one are not rebuilt (user request 2026-07-17: "fit the display area at first").
#[test]
fn mermaid_initial_size_fits_viewport_and_refits_on_change() {
    let dir = std::env::temp_dir().join("konoma_mermaid_fit_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let md = dir.join("doc.md");
    std::fs::write(&md, "```mermaid\ngraph TD\n  A --> B\n  B --> C\n```\n").unwrap();
    let plain = dir.join("plain.md");
    std::fs::write(&plain, "# title\n\njust text\n").unwrap();

    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.picker = Some(test_picker());
    app.enter_preview(&md);
    app.ensure_md_cache(80);
    app.ensure_md_cache(80);
    // Viewport not measured yet (0) = follows the cap.
    assert_eq!(app.md_images()[0].rows, 24, "既定 cap=24");
    // A short viewport → shrinks to a height that fits including the caption + margin.
    app.tab.preview_viewport = 10;
    app.ensure_md_cache(80);
    assert_eq!(app.md_images()[0].rows, 8, "vp10 → 8 行にフィット");
    // A wide viewport → back to the cap.
    app.tab.preview_viewport = 60;
    app.ensure_md_cache(80);
    assert_eq!(app.md_images()[0].rows, 24, "vp60 → cap 24");

    // A document with no diagram isn't rebuilt on a viewport change (lines' pointer is unchanged).
    app.enter_preview(&plain);
    app.ensure_md_cache(80);
    let ptr0 = app.md_cache.as_ref().unwrap().lines.as_ptr();
    app.tab.preview_viewport = 12;
    app.ensure_md_cache(80);
    assert_eq!(
        app.md_cache.as_ref().unwrap().lines.as_ptr(),
        ptr0,
        "図なし文書は vp 変化で再構築しない"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// With wrap enabled, the overlay (image/focus border) doesn't drift from the text layer even
/// when there are wrapped rows above the fence. Regression 2026-07-17: confusing logical rows
/// for placement with visual rows for scroll caused the image to be drawn too high by the
/// wrapped amount, overlapping the placeholder row with the inverted caption row and surfacing
/// as "a one-row bar in an ID color" (on a real Ghostty terminal; the color varied per run).
#[test]
fn fence_overlay_aligns_with_wrapped_text_layer() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    let dir = std::env::temp_dir().join("konoma_fence_wrap_align_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let md = dir.join("doc.md");
    // A single logical-line paragraph at the top that far exceeds the terminal width → wrap adds
    // several visual rows beyond the logical row count.
    let long = "wrap ".repeat(60);
    std::fs::write(
        &md,
        format!("{long}\n\n```mermaid\ngraph LR\n  A --> B\n```\n\ntail\n"),
    )
    .unwrap();

    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    assert!(app.cfg.ui.wrap, "前提: 既定 wrap=on");
    app.picker = Some(test_picker());
    app.enter_preview(&md);
    let mut term = Terminal::new(TestBackend::new(100, 45)).unwrap();
    for _ in 0..3 {
        term.draw(|fr| crate::ui::render(fr, &mut app)).unwrap();
    }

    let caption_rows = |term: &Terminal<TestBackend>| -> Vec<u16> {
        let buf = term.backend().buffer();
        (0..buf.area.height)
            .filter(|&y| {
                let row: String = (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect();
                row.contains("mermaid — Enter")
            })
            .collect()
    };
    // Unfocused: the text layer's caption row is visible = the ground truth for visual coordinates.
    // (The old code shifted the image band up by the wrapped amount, covering this row — this
    // already fails right here.)
    let unfocused = caption_rows(&term);
    assert_eq!(
        unfocused.len(),
        1,
        "キャプション行が1行見える: {unfocused:?}"
    );
    let caption_row = unfocused[0];

    // Focus the fence → the focus border's title lands on the same row (covering the caption).
    let idx = app
        .md_items
        .iter()
        .position(|it| matches!(it.kind, MdItemKind::MermaidFence { .. }))
        .unwrap();
    app.tab.focused_item = Some(idx);
    term.draw(|fr| crate::ui::render(fr, &mut app)).unwrap();
    let focused = caption_rows(&term);
    assert_eq!(
        focused,
        vec![caption_row],
        "フォーカス枠タイトルがテキスト層キャプションと同じ行(折返し分ズレない)"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// In-place zoom for an inline mermaid diagram: +/- (image_zoom_by's dual role) affects the
/// focused diagram; the layout (reserved cells) stays fixed; while zoomed, hjkl turns into
/// panning; and it resets when focus moves.
#[test]
fn fence_inplace_zoom_pans_and_keeps_layout() {
    use crate::keymap::Motion;

    let dir = std::env::temp_dir().join("konoma_fence_zoom_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let md = dir.join("doc.md");
    std::fs::write(
        &md,
        "[a link](https://example.com)\n\n```mermaid\ngraph LR\n  A --> B\n```\n",
    )
    .unwrap();

    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.picker = Some(test_picker());
    app.enter_preview(&md);
    app.ensure_md_cache(80);
    app.ensure_md_cache(80);
    let imgs = app.md_images();
    assert_eq!(imgs.len(), 1);
    let (cols0, rows0) = (imgs[0].cols, imgs[0].rows);

    // Focus the fence → + (zoom in). With no full-screen image, image_zoom_by acts on the diagram.
    let idx = app
        .md_items
        .iter()
        .position(|it| matches!(it.kind, MdItemKind::MermaidFence { .. }))
        .unwrap();
    app.tab.focused_item = Some(idx);
    app.image_zoom_by(2.0);
    assert!(
        (app.fence_zoom_level() - 2.0).abs() < 1e-9,
        "図がズームされる"
    );

    // While zoomed, hjkl pans (consumed=true); at 1x it is not consumed.
    assert!(
        app.fence_pan_motion(Motion::Right),
        "ズーム中はパンに化ける"
    );
    assert!(app.tab.fence_center.0 > 0.5, "中心が右へ動く");
    // The layout is unchanged (the reserved cell count doesn't change = the display size on the md stays fixed).
    app.ensure_md_cache(80);
    let imgs = app.md_images();
    assert_eq!(
        (imgs[0].cols, imgs[0].rows),
        (cols0, rows0),
        "表示エリア不変"
    );

    // = fits it, and it also resets when focus moves.
    app.image_zoom_reset();
    assert!((app.fence_zoom_level() - 1.0).abs() < 1e-9);
    assert!(
        !app.fence_pan_motion(Motion::Right),
        "等倍では通常スクロールへ"
    );
    app.image_zoom_by(3.0);
    app.md_focus_move(1); // move to the link
    assert!(
        (app.fence_zoom_level() - 1.0).abs() < 1e-9,
        "フォーカス移動でズームはリセット"
    );

    // + while focused on the link is a no-op (the diagram isn't focused).
    app.image_zoom_by(2.0);
    assert!((app.fence_zoom_level() - 1.0).abs() < 1e-9);

    std::fs::remove_dir_all(&dir).ok();
}

/// fence_crop: pure-function verification of the visible fraction and clamping (ratio-based =
/// maps to the same window even after re-rasterizing).
#[test]
fn fence_crop_clamps_and_scales() {
    // 2x zoom (f=0.5), centered: a half-size window at the center.
    let ((x, y, w, h), c) = fence_crop((800, 400), 0.5, (0.5, 0.5));
    assert_eq!((w, h), (400, 200));
    assert_eq!((x, y), (200, 100));
    assert_eq!(c, (0.5, 0.5));
    // Pan to the edge: the center is clamped and the window stays within the image.
    let ((x, _y, w, _h), c) = fence_crop((800, 400), 0.5, (1.0, 0.5));
    assert_eq!(x + w, 800, "右端で止まる");
    assert!((c.0 - 0.75).abs() < 1e-9, "中心は f/2 でクランプ");
    // A raster at 4x density (f=0.25): the same ratio window maps to 2x the pixels.
    let ((x1, _, w1, _), _) = fence_crop((800, 400), 0.25, (0.5, 0.5));
    let ((x2, _, w2, _), _) = fence_crop((1600, 800), 0.25, (0.5, 0.5));
    assert_eq!((x2, w2), (x1 * 2, w1 * 2));
}

/// A math expression's cell dimensions (pure function): the SVG's em unit (40/em) → rows,
/// aspect ratio → columns. display is taller than inline; exceeding the width clamps while
/// preserving aspect ratio; an extremely tall expression is capped at 24 rows. The sibling
/// functions `mermaid_cells`/`fence_crop` were already tested, but these two were uncovered.
#[test]
fn math_cells_size_from_em_units() {
    // inline: 1em (uh=40), aspect ratio 5:1 → 1 row × 10 columns.
    assert_eq!(math_cells(200, 40, 8, 16, 100, false), (10, 1));
    // display has a larger rows_per_em (1.5>1.3) → even the same expression comes out taller
    // (inline above was 1 row).
    let (dc, dr) = math_cells(200, 40, 8, 16, 100, true);
    assert!(dr > 1, "display は inline(1行)より背が高い: {dr}");
    assert_eq!((dc, dr), (20, 2));
    // Width exceeded: clamped preserving aspect ratio at avail=30 (columns stick to avail).
    let (cc, _cr) = math_cells(1600, 40, 8, 16, 30, false);
    assert_eq!(cc, 30, "幅は avail に張り付く");
    // An extremely tall expression (like a matrix) tops out at 24 rows.
    let (_bc, br) = math_cells(1200, 1200, 8, 16, 1000, false);
    assert_eq!(br, 24, "行数は 24 で上限");
}

/// An inline image's cell dimensions (pure function): based on its natural size, shrinks if the
/// width exceeds the available space, and shrinks further if the rows exceed the cap.
/// **Unlike mermaid, it never upscales past its natural size** (doesn't blur a photo).
#[test]
fn md_image_cells_downscale_but_never_upscale() {
    // Fits: stays at its natural size (80x160px, 8x16 cells → 10x10).
    assert_eq!(md_image_cells(80, 160, 8, 16, 100, 100), (10, 10));
    // A wide image exceeding the width: shrunk to avail=50 (height scales proportionally too).
    let (wc, wr) = md_image_cells(1600, 160, 8, 16, 50, 100);
    assert_eq!(wc, 50, "幅は avail に縮小");
    assert!(wr < 10, "高さも比例縮小: {wr}");
    // A tall image exceeding rows: capped at max_rows=20 (columns scale proportionally too).
    assert_eq!(md_image_cells(80, 1600, 8, 16, 100, 20), (2, 20));
    // Even with plenty of available space, it doesn't upscale past its natural size (40x40px stays 5x3).
    assert_eq!(md_image_cells(40, 40, 8, 16, 1000, 1000), (5, 3));
}

/// `0`=fit (consumed only while zoomed; at 1x it stays at the line start, as before) + `[ui]
/// mermaid_rows` changes an inline diagram's display height (0/invalid defaults to 24).
#[test]
fn fence_zero_fits_and_mermaid_rows_config_sizes_diagram() {
    use crate::keymap::Motion;

    let dir = std::env::temp_dir().join("konoma_fence_rows_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let md = dir.join("doc.md");
    std::fs::write(&md, "```mermaid\ngraph TD\n  A --> B\n  B --> C\n```\n").unwrap();

    // Load with rows=10 configured.
    let mut cfg = Config::default();
    cfg.ui.mermaid_rows = 10;
    let mut app = App::new(dir.clone(), cfg).unwrap();
    app.picker = Some(test_picker());
    app.enter_preview(&md);
    app.ensure_md_cache(80);
    app.ensure_md_cache(80);
    let small = app.md_images()[0].rows;
    assert!(small <= 10, "mermaid_rows=10 で高さが縮む: {small}");

    // With the default (24) it's bigger (the diagram is tall, so the rows cap takes effect).
    let mut app2 = App::new(dir.clone(), Config::default()).unwrap();
    app2.picker = Some(test_picker());
    app2.enter_preview(&md);
    app2.ensure_md_cache(80);
    app2.ensure_md_cache(80);
    let default_rows = app2.md_images()[0].rows;
    assert!(
        default_rows > small,
        "既定 24 の方が大きい: {default_rows} > {small}"
    );

    // 0 (invalid) falls back to the default.
    let mut cfg0 = Config::default();
    cfg0.ui.mermaid_rows = 0;
    let mut app3 = App::new(dir.clone(), cfg0).unwrap();
    app3.picker = Some(test_picker());
    app3.enter_preview(&md);
    app3.ensure_md_cache(80);
    app3.ensure_md_cache(80);
    assert_eq!(app3.md_images()[0].rows, default_rows, "0 は既定 24 扱い");

    // 0=fit: LineHome while zoomed is consumed as a diagram reset; at 1x it's not consumed.
    let idx = app2
        .md_items
        .iter()
        .position(|it| matches!(it.kind, MdItemKind::MermaidFence { .. }))
        .unwrap();
    app2.tab.focused_item = Some(idx);
    app2.image_zoom_by(2.0);
    assert!(
        app2.fence_pan_motion(Motion::LineHome),
        "ズーム中の 0=フィット"
    );
    assert!((app2.fence_zoom_level() - 1.0).abs() < 1e-9);
    assert!(
        !app2.fence_pan_motion(Motion::LineHome),
        "等倍の 0 は消費しない(行頭のまま)"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// mermaid_rows is **a target for the actual display size**: it can upscale past the base
/// raster's natural size (being vector-derived, density follows via re-rasterizing = it stays
/// sharp while the layout is unchanged), and the theme is dark by default with a transparent
/// background.
#[test]
fn mermaid_rows_upscales_and_dark_theme_is_transparent() {
    // Pure function: upscales to the target row count (independent of natural size), clamped by width.
    let (c, r) = mermaid_cells(400, 800, 8, 16, 200, 40);
    assert_eq!(r, 40, "目標行数まで拡大");
    assert_eq!(
        c, 40,
        "アスペクト維持(400/800 → 40行×16px=640px 高, 幅320px=40セル)"
    );
    let (c2, r2) = mermaid_cells(1600, 400, 8, 16, 60, 40);
    assert_eq!(c2, 60, "幅上限でクランプ");
    assert!(r2 < 40, "幅クランプに合わせ行数も縮む: {r2}");

    // Theme: the dark default has a background fill="none" (transparent) and no white background.
    let svg = crate::preview::markdown::mermaid_to_svg("graph LR\nA-->B", "dark").unwrap();
    assert!(svg.contains("fill=\"none\""), "背景透過");
    assert!(
        !svg.contains("fill=\"#FFFFFF\""),
        "dark テーマに白背景が無い"
    );

    // Density follow-up: to match a display size enlarged by increasing mermaid_rows,
    // ensure_md_fence_density re-rasterizes at higher density from the retained SVG
    // (synchronous fallback). The layout (reserved cells) is unchanged.
    use image::GenericImageView;
    let dir = std::env::temp_dir().join("konoma_mermaid_density_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let md = dir.join("doc.md");
    std::fs::write(&md, "```mermaid\ngraph TD\n  A --> B\n```\n").unwrap();
    let mut cfg = Config::default();
    cfg.ui.mermaid_rows = 60;
    cfg.ui.svg_max_px = 300; // shrinks the base raster = creates a density shortfall
    let mut app = App::new(dir.clone(), cfg).unwrap();
    app.picker = Some(test_picker());
    app.enter_preview(&md);
    app.ensure_md_cache(120);
    app.ensure_md_cache(120);
    let p = app.md_images()[0].clone();
    let key = std::path::PathBuf::from(&p.url);
    let before = app.md_image_cache[&key]
        .decoded
        .as_ref()
        .unwrap()
        .dimensions();
    app.ensure_md_fence_density(&p.url, p.cols, p.rows);
    let e = &app.md_image_cache[&key];
    let after = e.decoded.as_ref().unwrap().dimensions();
    assert!(
        after.0.max(after.1) > before.0.max(before.1),
        "表示サイズに合わせ高密度化: {before:?} -> {after:?}"
    );
    assert_eq!(
        e.layout_px,
        Some(before),
        "layout_px は初回のまま(予約セル不変)"
    );
    // The cell count doesn't change even after re-decorating (the layout is fixed).
    app.md_cache = None;
    app.ensure_md_cache(120);
    let p2 = &app.md_images()[0];
    assert_eq!((p2.cols, p2.rows), (p.cols, p.rows), "セル数不変");

    std::fs::remove_dir_all(&dir).ok();
}
/// Even with a large target like mermaid_rows=100, the placement upscales to that row count (as
/// far as the width allows).
#[test]
fn mermaid_rows_large_target_reaches_placement() {
    let dir = std::env::temp_dir().join("konoma_probe_rows100");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let md = dir.join("doc.md");
    std::fs::write(
        &md,
        "```mermaid\ngraph TD\n  A[a] --> B{b}\n  B --> C[c]\n```\n",
    )
    .unwrap();
    let mut cfg = Config::default();
    cfg.ui.mermaid_rows = 100;
    let mut app = App::new(dir.clone(), cfg).unwrap();
    app.picker = Some(test_picker());
    app.enter_preview(&md);
    app.ensure_md_cache(148);
    app.ensure_md_cache(148);
    let p = &app.md_images()[0];
    assert!(p.rows >= 90, "rows=100 target: got {}", p.rows);
    std::fs::remove_dir_all(&dir).ok();
}

/// Cleaning up kitty placeholder leftovers: a "full redraw requested" flag is raised only on
/// frames where an inline diagram **actually drawn** moved (it isn't raised while static; it's
/// raised once on the frame where a diagram appears/disappears). A countermeasure for Ghostty's
/// colored bar (a leftover old ID row). Regression 2026-07-17: because the signature included
/// the raw scroll value + all placements, an md with images did a full clear + double-render on
/// every scroll key even when the diagram was **off-screen**.
#[test]
fn md_overlay_move_detection_requests_full_redraw() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    let dir = std::env::temp_dir().join("konoma_overlay_move_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let md = dir.join("doc.md");
    let mut src = String::new();
    for i in 0..20 {
        src.push_str(&format!("line {i}\n\n"));
    }
    src.push_str("```mermaid\ngraph LR\n  A --> B\n```\n");
    std::fs::write(&md, src).unwrap();

    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.picker = Some(test_picker());
    app.enter_preview(&md);
    let mut term = Terminal::new(TestBackend::new(100, 24)).unwrap();

    term.draw(|fr| crate::ui::render(fr, &mut app)).unwrap(); // first pass (Loading → synchronous render)
    term.draw(|fr| crate::ui::render(fr, &mut app)).unwrap(); // placement is settled (the diagram is below = off-screen)
    let _ = app.take_md_overlay_moved();
    term.draw(|fr| crate::ui::render(fr, &mut app)).unwrap(); // a static frame
    assert!(!app.take_md_overlay_moved(), "静止中は要求しない");

    // Scrolling while the diagram exists only **off-screen** doesn't request a redraw (there's
    // no placeholder in the grid = nothing to clean up. This is exactly where it became a
    // clear-on-every-key regression).
    app.tab.preview_scroll = app.tab.preview_scroll.saturating_add(3);
    term.draw(|fr| crate::ui::render(fr, &mut app)).unwrap();
    assert!(
        !app.take_md_overlay_moved(),
        "画面外の図しか無い間のスクロールでは要求しない"
    );

    // Scroll to the end to bring the diagram on-screen. The appearance frame doesn't request a
    // redraw either (nothing was drawn just before = there's no old placeholder in the grid to
    // clean up. Cleanup only happens on "a change from a previously-drawn state").
    app.tab.preview_scroll = 500; // the render side clamps it to the end
    term.draw(|fr| crate::ui::render(fr, &mut app)).unwrap();
    assert!(!app.take_md_overlay_moved(), "出現フレームは掃除不要");
    term.draw(|fr| crate::ui::render(fr, &mut app)).unwrap();
    assert!(!app.take_md_overlay_moved(), "その後の静止では立たない");

    // Scrolling while the diagram is visible → is raised on the frame it moved.
    app.tab.preview_scroll = app.tab.preview_scroll.saturating_sub(1);
    term.draw(|fr| crate::ui::render(fr, &mut app)).unwrap();
    assert!(
        app.take_md_overlay_moved(),
        "見えている図が動いたら一度フル再描画"
    );
    term.draw(|fr| crate::ui::render(fr, &mut app)).unwrap();
    assert!(!app.take_md_overlay_moved(), "その後の静止では立たない");

    // A focus change (even without scrolling) → raised once (fixes Ghostty's compositing glitch on the spot).
    let idx = app
        .md_items
        .iter()
        .position(|it| matches!(it.kind, MdItemKind::MermaidFence { .. }))
        .unwrap();
    app.tab.focused_item = Some(idx);
    term.draw(|fr| crate::ui::render(fr, &mut app)).unwrap();
    assert!(
        app.take_md_overlay_moved(),
        "フォーカス変化でも一度フル再描画"
    );
    term.draw(|fr| crate::ui::render(fr, &mut app)).unwrap();
    assert!(!app.take_md_overlay_moved(), "フォーカス静止では立たない");

    // Leaving preview (the diagram disappears) → raised once.
    app.back_to_tree();
    term.draw(|fr| crate::ui::render(fr, &mut app)).unwrap();
    assert!(app.take_md_overlay_moved(), "図が画面から消えたら掃除");

    std::fs::remove_dir_all(&dir).ok();
}

/// When switching tabs, a .mmd (image mode) is restored still as an image. Regression 2026-07-18:
/// load_active's media-restoration branch had no case for the Mermaid family, and clear_image
/// also left preview_media_mtime in place, so reload_media_if_changed also misjudged it as
/// "unchanged" and was blocked, so returning to the tab fell back to the text ruled-line diagram —
/// A git graph's decoration state (the pinned base, visible branches, priority order, legend) is
/// saved/restored per tab. Regression 2026-07-18: git_graph (GraphRow) is saved, but these
/// derived states were not, so changing the base/visibility in another tab and coming back left
/// the legend or the `base:` title showing the other tab's values (load_active doesn't rebuild
/// the graph). It is not carried over to a new tab, and is restored on returning.
#[test]
fn git_graph_decoration_state_is_per_tab() {
    let dir = std::env::temp_dir().join("konoma_graph_pertab_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.txt"), "x").unwrap();

    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    // Simulate tab 0's graph decoration state (set the derived fields directly, without building
    // a real graph).
    app.tab.git_graph_base = Some("release/1.0".into());
    app.tab.git_graph_base_label = Some("release/1.0".into());
    app.tab.git_graph_visible = ["main".to_string(), "dev".to_string()]
        .into_iter()
        .collect();
    app.tab.git_graph_order = vec!["release/1.0".into(), "main".into(), "dev".into()];
    app.tab.git_graph_hidden = 3;

    app.tab_new().unwrap(); // tab 1 (a plain Tree)
    assert!(
        app.tab.git_graph_base.is_none(),
        "新規タブへ基準ピンは漏れない"
    );
    assert!(
        app.tab.git_graph_visible.is_empty(),
        "新規タブへ表示ブランチは漏れない"
    );
    assert!(
        app.tab.git_graph_order.is_empty(),
        "新規タブへ優先順は漏れない"
    );
    assert_eq!(app.tab.git_graph_hidden, 0, "新規タブの hidden は 0");

    // Put tab 1 into a different decoration state, then go back.
    app.tab.git_graph_base = Some("hotfix".into());
    app.tab_goto(0);
    assert_eq!(
        app.tab.git_graph_base.as_deref(),
        Some("release/1.0"),
        "タブ0の基準ピンが復元される"
    );
    assert_eq!(app.tab.git_graph_hidden, 3, "タブ0の hidden が復元される");
    assert!(
        app.tab.git_graph_visible.contains("main") && app.tab.git_graph_visible.contains("dev"),
        "タブ0の表示ブランチが復元される"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// It had been silently degrading (a non-deterministic behavior that happened to recover only
/// when a different media was opened in the background).
#[test]
fn tab_switch_restores_mermaid_image_preview() {
    let dir = std::env::temp_dir().join("konoma_mermaid_tab_restore_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("d.mmd"), "graph LR\n  A --> B\n").unwrap();

    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.picker = Some(test_picker());
    app.enter_preview(&dir.join("d.mmd"));
    assert!(app.image_src.is_some(), "前提: 画像モードでラスタ化される");

    app.tab_new().unwrap(); // to tab 2 (a plain tree)
    assert!(app.image_src.is_none(), "新規タブに画像は無い");
    app.tab_cycle(1); // back to tab 1
    assert!(
        app.image_src.is_some(),
        "タブ復帰で .mmd が画像として再ロードされる"
    );
    assert!(app.is_image_preview(), "IMAGE 面のまま");

    std::fs::remove_dir_all(&dir).ok();
}

/// Even leaving and returning to a tab while a full-screen fence view is showing, the diagram is
/// restored still as an image (the old code fell back to a false "cannot render" notice).
#[test]
fn tab_switch_restores_fullscreen_fence_view() {
    let dir = std::env::temp_dir().join("konoma_fence_tab_restore_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let md = dir.join("doc.md");
    std::fs::write(&md, "intro\n\n```mermaid\ngraph LR\n  A --> B\n```\n").unwrap();

    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.picker = Some(test_picker());
    app.enter_preview(&md);
    app.ensure_md_cache(100);
    let idx = app
        .md_items
        .iter()
        .position(|it| matches!(it.kind, MdItemKind::MermaidFence { .. }))
        .expect("フェンスが Tab アイテムに載る");
    app.tab.focused_item = Some(idx);
    app.md_activate_focused().unwrap();
    assert!(
        matches!(app.tab.preview_kind, Some(PreviewKind::MermaidFence(_))),
        "全画面フェンスへ遷移"
    );
    assert!(app.image_src.is_some(), "前提: 全画面の図がラスタ化される");

    app.tab_new().unwrap();
    app.tab_cycle(1);
    assert!(
        matches!(app.tab.preview_kind, Some(PreviewKind::MermaidFence(_))),
        "タブ復帰でも全画面フェンスのまま"
    );
    assert!(app.image_src.is_some(), "図が画像として再ロードされる");

    std::fs::remove_dir_all(&dir).ok();
}

/// Even if there's an earlier fence that can't be turned into an image (degraded to text),
/// pressing Enter on a Tab-focused fence opens **that diagram's source**. Regression 2026-07-18:
/// the ordinal was derived by "counting rendered sentinels", so it drifted from source
/// re-extraction by however many fences (failed/loading) didn't emit a sentinel, and a different
/// diagram (usually the one that failed = a false error display) would open full-screen.
#[test]
fn fence_ordinal_survives_failed_upstream_fence() {
    let dir = std::env::temp_dir().join("konoma_fence_ordinal_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let md = dir.join("doc.md");
    std::fs::write(
        &md,
        "```mermaid\ntotally ]] broken [[ not a diagram\n```\n\nmid\n\n```mermaid\ngraph LR\n  A --> B\n```\n",
    )
    .unwrap();

    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.picker = Some(test_picker());
    app.enter_preview(&md);
    app.ensure_md_cache(100);

    // Only 1 fence gets rendered (the broken one degrades to text), and its ordinal is **1, in source order**.
    let fences: Vec<usize> = app
        .md_items
        .iter()
        .filter_map(|it| match it.kind {
            MdItemKind::MermaidFence { ordinal } => Some(ordinal),
            _ => None,
        })
        .collect();
    assert_eq!(fences, vec![1], "番兵の序数はソース順(失敗フェンス込み)");

    let idx = app
        .md_items
        .iter()
        .position(|it| matches!(it.kind, MdItemKind::MermaidFence { .. }))
        .unwrap();
    app.tab.focused_item = Some(idx);
    app.md_activate_focused().unwrap();
    assert!(
        matches!(app.tab.preview_kind, Some(PreviewKind::MermaidFence(1))),
        "Enter は**正しい方**のフェンスを開く: {:?}",
        app.tab.preview_kind
    );
    assert!(
        app.mermaid_fence_code(&md, 1)
            .is_some_and(|c| c.contains("graph LR")),
        "序数 1 の再抽出は有効な図のソース"
    );
    assert!(app.image_src.is_some(), "全画面の図がラスタ化される");

    std::fs::remove_dir_all(&dir).ok();
}

/// The old cache entry for a fence whose content changed via an edit is reclaimed on the next
/// re-decoration. Regression 2026-07-18: the key is a content hash, and eviction only happened on
/// enter_preview (a file switch), so under agent-watch's repeated edits, old rasters/protocols kept
/// growing monotonically until the file was switched.
#[test]
fn stale_fence_cache_entries_are_pruned_on_rebuild() {
    let dir = std::env::temp_dir().join("konoma_fence_prune_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let md = dir.join("doc.md");
    let v1 = "```mermaid\ngraph LR\n  A --> B\n```\n";
    std::fs::write(&md, v1).unwrap();

    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.picker = Some(test_picker());
    app.enter_preview(&md);
    app.ensure_md_cache(100);
    let code1 = crate::preview::markdown::collect_mermaid_fences(v1).remove(0);
    let key1 = std::path::PathBuf::from(crate::preview::markdown::mermaid_fence_url(&code1));
    assert!(
        app.md_image_cache.contains_key(&key1),
        "前提: v1 がキャッシュ済み"
    );

    // External edit (content change) -> reload, re-decorate.
    let v2 = "```mermaid\ngraph TD\n  X --> Y\n```\n";
    std::fs::write(&md, v2).unwrap();
    app.reload_preview();
    app.ensure_md_cache(100);
    let code2 = crate::preview::markdown::collect_mermaid_fences(v2).remove(0);
    let key2 = std::path::PathBuf::from(crate::preview::markdown::mermaid_fence_url(&code2));
    assert!(
        app.md_image_cache.contains_key(&key2),
        "v2 はキャッシュ済み"
    );
    assert!(
        !app.md_image_cache.contains_key(&key1),
        "旧内容のエントリは prune される(単調成長しない)"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// Tab focus and the inline diagram's zoom/pan are duplicated per tab: they don't leak into other
/// tabs, and are restored on return (on par with image_zoom being saved in PerTab).
#[test]
fn fence_focus_and_zoom_are_per_tab() {
    let dir = std::env::temp_dir().join("konoma_fence_per_tab_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let a = dir.join("a.md");
    let b = dir.join("b.md");
    std::fs::write(&a, "```mermaid\ngraph LR\n  A --> B\n```\n").unwrap();
    std::fs::write(&b, "```mermaid\ngraph TD\n  C --> D\n```\n").unwrap();

    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.picker = Some(test_picker());
    app.enter_preview(&a);
    app.ensure_md_cache(100);
    let idx = app
        .md_items
        .iter()
        .position(|it| matches!(it.kind, MdItemKind::MermaidFence { .. }))
        .unwrap();
    app.tab.focused_item = Some(idx);
    app.image_zoom_by(2.0); // no full-screen image + fence focused -> acts on fence_zoom
    assert!(app.tab.fence_zoom > 1.9, "前提: タブ1でその場ズーム中");

    app.tab_new().unwrap(); // tab 2
    assert_eq!(app.tab.focused_item, None, "新規タブへフォーカスは漏れない");
    assert!(app.tab.fence_zoom < 1.001, "新規タブへズームは漏れない");
    app.enter_preview(&b);
    app.ensure_md_cache(100);
    assert_eq!(
        app.tab.focused_item, None,
        "タブ2の文書は未フォーカスのまま"
    );

    app.tab_cycle(1); // back to tab 1
    assert_eq!(
        app.tab.focused_item,
        Some(idx),
        "タブ1のフォーカスが復元される"
    );
    assert!(app.tab.fence_zoom > 1.9, "タブ1のズームが復元される");

    std::fs::remove_dir_all(&dir).ok();
}

/// Returning with `q` from a full-screen fence keeps the inline image cache (re-entering the same
/// file). Regression 2026-07-18: enter_preview unconditionally dropped md_image_cache, so every
/// return re-rendered all fences, and a single render of the Loading degraded layout could clamp
/// the restored scroll/focus.
#[test]
fn fence_fullscreen_return_keeps_diagram_cache() {
    let dir = std::env::temp_dir().join("konoma_fence_return_cache_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let md = dir.join("doc.md");
    let src = "intro\n\n```mermaid\ngraph LR\n  A --> B\n```\n";
    std::fs::write(&md, src).unwrap();

    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.picker = Some(test_picker());
    app.enter_preview(&md);
    app.ensure_md_cache(100);
    let code = crate::preview::markdown::collect_mermaid_fences(src).remove(0);
    let key = std::path::PathBuf::from(crate::preview::markdown::mermaid_fence_url(&code));
    let arc0 = app
        .md_image_cache
        .get(&key)
        .and_then(|e| e.decoded.clone())
        .expect("前提: フェンスがレンダ済み");

    let idx = app
        .md_items
        .iter()
        .position(|it| matches!(it.kind, MdItemKind::MermaidFence { .. }))
        .unwrap();
    app.tab.focused_item = Some(idx);
    app.md_activate_focused().unwrap(); // Enter -> full screen
    app.back_to_tree(); // q -> return to md
    assert!(
        matches!(app.tab.preview_kind, Some(PreviewKind::Markdown(_))),
        "md ビューへ戻る"
    );
    let arc1 = app
        .md_image_cache
        .get(&key)
        .and_then(|e| e.decoded.clone())
        .expect("復帰後もキャッシュが残る");
    assert!(
        std::sync::Arc::ptr_eq(&arc0, &arc1),
        "同じラスタを温存(再レンダしていない)"
    );
    assert_eq!(app.tab.focused_item, Some(idx), "フォーカスも復元される");

    std::fs::remove_dir_all(&dir).ok();
}

/// Even for an encode failure result (protocol=None), enc_inflight is cleared and the key is
/// recorded so the same request is not retried every frame. Only a Full failure that has never
/// succeeded even once degrades to text. Regression 2026-07-18: the worker swallowed Err results,
/// leaving enc_inflight permanently true = that image's re-encoding stopped + md_images_loading()
/// permanently true (busy spinner + 16ms polling stuck on).
#[test]
fn failed_encode_clears_inflight_and_degrades_safely() {
    let dir = std::env::temp_dir().join("konoma_encode_fail_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();

    // Full failure, no protocol held -> degrades to failed and busy clears.
    let key = std::path::PathBuf::from("mermaid-fence://feedfeedfeedfeed");
    app.md_image_cache.insert(
        key.clone(),
        MdImgEntry {
            enc_inflight: true,
            ..Default::default()
        },
    );
    assert!(app.md_images_loading(), "前提: inflight 中は busy");
    let redraw = app.apply_md_encode(MdEncodeResult {
        path: key.clone(),
        key: MdEncodeKey::Full { cols: 10, rows: 5 },
        protocol: None,
    });
    assert!(redraw);
    {
        let e = app.md_image_cache.get(&key).unwrap();
        assert!(!e.enc_inflight, "inflight が解除される");
        assert!(e.failed, "表示できる protocol が無ければテキスト降格");
        assert_eq!(e.proto_size, Some((10, 5)), "キー記録=リトライループ防止");
    }
    assert!(!app.md_images_loading(), "busy が恒久化しない");

    // A Zoom failure keeps the old display (does not mark failed) -- only the key is recorded.
    let key2 = std::path::PathBuf::from("mermaid-fence://cafecafecafecafe");
    app.md_image_cache.insert(
        key2.clone(),
        MdImgEntry {
            enc_inflight: true,
            decoded: Some(std::sync::Arc::new(image::DynamicImage::new_rgba8(8, 8))),
            ..Default::default()
        },
    );
    let crop = (0u32, 0u32, 4u32, 4u32);
    app.apply_md_encode(MdEncodeResult {
        path: key2.clone(),
        key: MdEncodeKey::Zoom {
            cols: 4,
            rows: 2,
            crop,
        },
        protocol: None,
    });
    let e2 = app.md_image_cache.get(&key2).unwrap();
    assert!(!e2.enc_inflight);
    assert!(!e2.failed, "ズーム失敗で図全体を殺さない");
    assert_eq!(e2.zoom_key, Some((4, 2, crop)));

    std::fs::remove_dir_all(&dir).ok();
}

/// The math inline image `math://...` used to render as **blank**, only the reserved row (user
/// report 2026-07-20). Root cause: ensure_md_image / md_image_proto only special-cased the mermaid
/// fence's synthetic key, and passed `math://` to resolve_md_image_path as a real file path = it
/// always returned None, so **encoding was never requested** and no protocol came back either =
/// the reserved cell stayed blank. is_synthetic_md_url now treats both fences and math as synthetic
/// keys, using the URL itself as the cache key. This verifies that "for an already-decoded math
/// expression, ensure_md_image queues an encode request" (the old implementation queued nothing via
/// an early return = blank).
#[test]
fn math_inline_image_requests_encode_via_synthetic_key() {
    use crate::preview::markdown::{is_synthetic_md_url, math_url};
    // Classification of synthetic keys: fences and math are synthetic; real files/remote are real files.
    assert!(is_synthetic_md_url(&math_url("E=mc^2", false)));
    assert!(is_synthetic_md_url("mermaid-fence://feedfeed"));
    assert!(!is_synthetic_md_url("figure.png"));
    assert!(!is_synthetic_md_url("https://example.com/x.svg"));

    let dir = std::env::temp_dir().join("konoma_math_encode_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();

    // Wire up the encode worker's receiving end (ensure_md_image queues an MdEncodeRequest here).
    let (tx, rx) = std::sync::mpsc::channel();
    app.attach_md_encoder(tx);

    // Reproduce the state after ensure_math_render -> apply_md_image: a synthetic-key entry with decoded + layout_px.
    let url = math_url("E=mc^2", false);
    let key = std::path::PathBuf::from(&url);
    app.md_image_cache.insert(
        key.clone(),
        MdImgEntry {
            decoded: Some(std::sync::Arc::new(image::DynamicImage::new_rgba8(120, 48))),
            layout_px: Some((120, 48)),
            ..Default::default()
        },
    );

    // Full display of the reserved row (cols=12, rows=2). Math must request an encode (blank in the old implementation).
    app.ensure_md_image(&url, 12, 2, 0, 2);
    let req = rx
        .try_recv()
        .expect("数式はエンコードを要求する(math:// を合成キーとして解決)");
    assert_eq!(req.path, key, "math:// を実ファイルでなく合成キーで解決");
    assert!(matches!(req.key, MdEncodeKey::Full { cols: 12, rows: 2 }));

    std::fs::remove_dir_all(&dir).ok();
}

/// Sharp re-rasterization of a full-screen vector is limited to 1 at a time (inflight guard).
/// Regression 2026-07-18: without the guard, key-repeating `+` could spawn a dozen-odd identical
/// re-rasters before a job finished (each ~a few hundred ms, ~128MiB transient; an asymmetry with
/// the inline side's reraster_inflight).
#[test]
fn vector_reraster_inflight_guard_blocks_duplicate_jobs() {
    use image::GenericImageView;
    let dir = std::env::temp_dir().join("konoma_vector_inflight_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("d.mmd"), "graph TD\n  A --> B\n").unwrap();

    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.picker = Some(test_picker());
    app.enter_preview(&dir.join("d.mmd"));
    let before = app.image_src.as_ref().unwrap().dimensions();

    // While inflight, zooming does not trigger a re-raster (the raster dimensions don't change).
    app.vector_reraster_inflight = true;
    app.image_zoom_by(2.0);
    let held = app.image_src.as_ref().unwrap().dimensions();
    assert_eq!(held, before, "inflight 中は再ラスタしない");

    // Catches up on the next zoom operation after being cleared (synchronous path = applied immediately).
    app.vector_reraster_inflight = false;
    app.image_zoom_by(1.1);
    let after = app.image_src.as_ref().unwrap().dimensions();
    assert!(
        after.0 > before.0 || after.1 > before.1,
        "解除後は最新ズームへ収束: {before:?} -> {after:?}"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// An empty ```mermaid fence (mid-typing) falls back to the text path instead of sticking on a
/// "loading" line, and the source ordinal of a valid fence doesn't shift either. Regression
/// 2026-07-18: an empty string in the slot was indistinguishable from the "is extraction on?"
/// probe, so it picked up the probe response's Loading and showed "loading" forever.
#[test]
fn empty_mermaid_fence_does_not_stick_on_loading() {
    let dir = std::env::temp_dir().join("konoma_empty_fence_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let md = dir.join("doc.md");
    std::fs::write(
        &md,
        "```mermaid\n```\n\nmid\n\n```mermaid\ngraph LR\n  A --> B\n```\n",
    )
    .unwrap();

    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.picker = Some(test_picker());
    app.enter_preview(&md);
    app.ensure_md_cache(100);

    let joined: String = app
        .md_cache
        .as_ref()
        .unwrap()
        .lines
        .iter()
        .map(|l| {
            l.spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>()
                + "\n"
        })
        .collect();
    assert!(
        !joined.contains("loading"),
        "空フェンスが loading 行に固まらない: {joined}"
    );
    // The valid fence is drawn, and its ordinal is 1 in source order (the empty fence occupies 0).
    let placements = app.md_images();
    assert_eq!(placements.len(), 1);
    assert_eq!(placements[0].fence_ord, Some(1), "序数はソース順");

    std::fs::remove_dir_all(&dir).ok();
}

/// Even while zoomed, pan must not steal keys if the diagram is **offscreen**. Regression
/// 2026-07-18: the consumption condition was only "zoomed + has focus", so sending the diagram
/// offscreen with Ctrl-f/G caused hjkl/j/k to get eaten by panning an invisible diagram, making the
/// keys appear dead.
#[test]
fn zoomed_fence_offscreen_does_not_eat_motion_keys() {
    use crate::keymap::Motion as M;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    let dir = std::env::temp_dir().join("konoma_fence_pan_offscreen_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let md = dir.join("doc.md");
    let mut src = String::from("```mermaid\ngraph LR\n  A --> B\n```\n\n");
    for i in 0..60 {
        src.push_str(&format!("line {i}\n\n"));
    }
    std::fs::write(&md, src).unwrap();

    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.picker = Some(test_picker());
    app.enter_preview(&md);
    let mut term = Terminal::new(TestBackend::new(100, 24)).unwrap();
    term.draw(|fr| crate::ui::render(fr, &mut app)).unwrap();
    term.draw(|fr| crate::ui::render(fr, &mut app)).unwrap();

    let idx = app
        .md_items
        .iter()
        .position(|it| matches!(it.kind, MdItemKind::MermaidFence { .. }))
        .unwrap();
    app.tab.focused_item = Some(idx);
    app.image_zoom_by(2.0);
    assert!(app.fence_pan_motion(M::Down), "可視+ズーム中はパンを消費");

    // Send the diagram offscreen (scroll to the end) -> no longer consumed = falls back to document scroll.
    app.tab.preview_scroll = 500;
    term.draw(|fr| crate::ui::render(fr, &mut app)).unwrap();
    assert!(
        !app.fence_pan_motion(M::Down),
        "画面外の図のパンにキーを食わせない"
    );
    assert!(
        !app.fence_pan_motion(M::LineHome),
        "0 も奪わない(通常の行頭へ)"
    );
    assert!(app.tab.fence_zoom > 1.9, "ズーム状態自体は保持(戻れば再開)");

    std::fs::remove_dir_all(&dir).ok();
}

/// A decode result for a key not in the cache (a stale result arriving late after a file switch /
/// prune) is dropped: it doesn't resurrect the entry and doesn't invalidate the current document's
/// md_cache either.
#[test]
fn stale_md_image_result_is_dropped() {
    let dir = std::env::temp_dir().join("konoma_stale_md_result_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let md = dir.join("doc.md");
    std::fs::write(&md, "# title\n\nplain text\n").unwrap();

    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.picker = Some(test_picker());
    app.enter_preview(&md);
    app.ensure_md_cache(100);
    assert!(app.md_cache.is_some());

    let stale = std::path::PathBuf::from("mermaid-fence://deadbeefdeadbeef");
    let redraw = app.apply_md_image(MdImageResult {
        path: stale.clone(),
        image: Ok(image::DynamicImage::new_rgba8(4, 4)),
        svg: None,
        reraster: false,
        frames: None,
    });
    assert!(!redraw, "陳腐化結果は再描画も要求しない");
    assert!(
        !app.md_image_cache.contains_key(&stale),
        "エントリを復活させない"
    );
    assert!(
        app.md_cache.is_some(),
        "無関係な現文書の装飾キャッシュを壊さない"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// When a file is externally **shrunk**, the windowed preview's scroll position must not stay past
/// the new EOF. This happens during everyday Agent Watch operations, e.g. an agent truncating a
/// log / rewriting a file. `preview_byte_top` can be left pointing at the old file's last page, so
/// it must be clamped on the reload -> render path.
#[test]
fn windowed_preview_clamps_scroll_when_file_shrinks_externally() {
    let dir = unique_tmp("konoma_shrink_clamp");
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("log.txt");
    let long: String = (1..=200).map(|i| format!("line {i}\n")).collect();
    std::fs::write(&f, &long).unwrap();

    let mut app = App::new(dir.canonicalize().unwrap(), Config::default()).unwrap();
    app.tab.selected = app
        .tab
        .entries
        .iter()
        .position(|e| e.path.ends_with("log.txt"))
        .unwrap();
    app.tree_activate().unwrap();
    app.tab.preview_viewport = 5;
    // To the end = byte_top is the last page of the 200-line file (larger than the length after shrinking).
    app.preview_to_bottom();
    let _ = app.windowed_lines(5, 80);
    let deep_top = app.tab.preview_byte_top;
    assert!(deep_top > 0, "末尾へスクロールできている");

    // Externally truncated to 3 lines (equivalent to an agent's truncate).
    std::fs::write(&f, "a\nb\nc\n").unwrap();
    let new_len = std::fs::metadata(&f).unwrap().len();
    assert!(deep_top > new_len, "旧 top は新しい EOF を越えている(前提)");

    // Reload equivalent to a FS event -> render.
    app.reload_preview();
    let lines = app.windowed_lines(5, 80);

    assert!(
        app.tab.preview_byte_top <= new_len,
        "縮小後もスクロール位置が EOF を越えている: top={} len={new_len}",
        app.tab.preview_byte_top
    );
    let text: String = lines
        .iter()
        .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
        .collect();
    assert!(
        text.contains('a') && text.contains('c'),
        "縮小後の中身が描かれていない: {text:?}"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// A preview reload on a FS event must happen **according to the changed paths**.
///
/// Purpose: while an agent keeps rewriting `src/`, don't rebuild the decoration of an unrelated
/// `docs/foo.md` on every event (re-rendering a whole Markdown document is expensive). On the
/// other hand, always follow a change to the file currently shown, a `.git`-only change (= arrives
/// as an empty list), or unknown changed paths.
#[test]
fn preview_reloads_only_for_relevant_fs_changes() {
    let dir = unique_tmp("konoma_refresh_scope");
    std::fs::create_dir_all(dir.join("docs")).unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    let md = dir.join("docs/note.md");
    std::fs::write(&md, "# Title\n\nbody one\n").unwrap();
    std::fs::write(dir.join("src/lib.rs"), "fn a() {}\n").unwrap();

    let root = dir.canonicalize().unwrap();
    let md = root.join("docs/note.md");
    let other = root.join("src/lib.rs");
    let mut app = App::new(root.clone(), Config::default()).unwrap();
    app.tab.preview_kind = Some(app.cfg.resolve_preview(&md));
    app.tab.preview_path = Some(md.clone());
    app.tab.mode = Mode::Preview;
    app.tab.preview_viewport = 20;

    // Build the decoration cache.
    let _ = app.decorated_lines(80);
    assert!(app.md_cache.is_some(), "装飾キャッシュができている");

    // (1) A change to an unrelated file -> the cache is kept (no re-render).
    app.refresh_fs_changed(false, std::slice::from_ref(&other))
        .unwrap();
    assert!(
        app.md_cache.is_some(),
        "無関係な src/lib.rs の変更で装飾キャッシュを捨てている"
    );

    // (2) A change to the file currently shown -> reload it (follow edits by an external editor/agent).
    std::fs::write(&md, "# Title\n\nbody two\n").unwrap();
    app.refresh_fs_changed(false, std::slice::from_ref(&md))
        .unwrap();
    assert!(
        app.md_cache.is_none(),
        "表示中ファイルが変わったのに再読込していない"
    );
    let text: String = app
        .decorated_lines(80)
        .iter()
        .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
        .collect();
    assert!(
        text.contains("body two"),
        "新しい内容が反映されない: {text:?}"
    );

    // (3) An empty list (= a `.git`-only change / unknown) -> always reload, erring safe.
    let _ = app.decorated_lines(80);
    assert!(app.md_cache.is_some());
    app.refresh_fs_changed(false, &[]).unwrap();
    assert!(
        app.md_cache.is_none(),
        "変更パス不明(.git のみ等)は安全側で再読込すべき"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// In-table search: `/` jumps the cursor to a matching cell, and `n`/`N` cycle in reading order
/// (wrapping at the ends). Case-insensitive, and matching cells are also added to the set the
/// renderer can look up.
#[test]
fn table_search_moves_cursor_through_matching_cells() {
    let dir = unique_tmp("konoma_table_search");
    std::fs::create_dir_all(&dir).unwrap();
    let csv = dir.join("t.csv");
    // 3 columns x 4 rows. "apple" appears in 3 cells: (0,0) / (1,2) / (3,1) (mixed case).
    std::fs::write(
        &csv,
        "h1,h2,h3\napple,x,y\nq,r,APPLE\nz,z,z\nw,Apple pie,v\n",
    )
    .unwrap();
    let mut app = App::new(dir.canonicalize().unwrap(), Config::default()).unwrap();
    let csv = dir.canonicalize().unwrap().join("t.csv");
    app.tab.preview_kind = Some(app.cfg.resolve_preview(&csv));
    app.tab.preview_path = Some(csv);
    app.tab.mode = Mode::Preview;
    app.load_table();
    assert_eq!(app.table_data().map(|t| t.nrows()), Some(4));

    app.start_search();
    for c in "apple".chars() {
        app.search_input_push(c);
    }
    app.search_commit();

    assert_eq!(
        app.search_status(),
        Some((1, 3)),
        "大小無視で 3 セル一致し、1件目に居る"
    );
    assert_eq!(app.table_cursor(), (0, 0), "最初の一致セルへ移動");
    assert!(
        app.table_cell_is_hit(0, 0) && app.table_cell_is_hit(1, 2) && app.table_cell_is_hit(3, 1)
    );
    assert!(!app.table_cell_is_hit(2, 0), "非一致セルは含まない");

    app.search_next(1);
    assert_eq!(app.table_cursor(), (1, 2), "読み順で次の一致へ");
    app.search_next(1);
    assert_eq!(app.table_cursor(), (3, 1));
    app.search_next(1);
    assert_eq!(app.table_cursor(), (0, 0), "末尾から先頭へ wrap");
    app.search_next(-1);
    assert_eq!(app.table_cursor(), (3, 1), "N は逆順(先頭から末尾へ wrap)");

    // Equivalent to Esc: clearing also clears the match set (nothing left behind in the render).
    app.search_clear();
    assert!(!app.table_cell_is_hit(0, 0), "解除で一致ハイライトが消える");
    assert_eq!(app.search_status(), None);

    std::fs::remove_dir_all(&dir).ok();
}

/// When there's no match, show a flash and don't move the cursor (don't lose the user's place in the table).
#[test]
fn table_search_without_match_keeps_cursor() {
    let (mut app, _csv) = app_with_table();
    app.table_cursor_move(1, 1);
    let before = app.table_cursor();
    app.start_search();
    for c in "zzz-not-here".chars() {
        app.search_input_push(c);
    }
    app.search_commit();
    assert_eq!(app.table_cursor(), before, "一致なしでカーソルは不動");
    assert!(app.flash.is_some(), "一致なしを flash で知らせる");
    assert_eq!(app.search_status(), None);
}

/// In-preview search on decorated Markdown. Searches **the decorated lines shown on screen**, not
/// the source, and scrolls the matching line into view (does not switch to `R`'s raw source).
/// `n`/`N` cycle in document order.
#[test]
fn decorated_markdown_search_scrolls_to_matches() {
    let dir = unique_tmp("konoma_md_search");
    std::fs::create_dir_all(&dir).unwrap();
    let md = dir.join("doc.md");
    // A heading + filler + 2 widely separated occurrences of "needle".
    let mut src = String::from("# Title\n\n");
    for i in 0..40 {
        src.push_str(&format!("filler line {i}\n\n"));
    }
    src.push_str("first needle here\n\n");
    for i in 40..80 {
        src.push_str(&format!("filler line {i}\n\n"));
    }
    src.push_str("second needle there\n");
    std::fs::write(&md, &src).unwrap();

    let mut app = App::new(dir.canonicalize().unwrap(), Config::default()).unwrap();
    let md = dir.canonicalize().unwrap().join("doc.md");
    app.tab.preview_kind = Some(app.cfg.resolve_preview(&md));
    app.tab.preview_path = Some(md);
    app.tab.mode = Mode::Preview;
    app.tab.preview_viewport = 10;
    let _ = app.md_layout(80); // populate the decoration cache (same path as the real render)

    // Search can start even on decorated md (it used to be refused with "code/text only").
    app.start_search();
    assert!(app.search_input().is_some(), "装飾 md で検索入力に入れる");
    for c in "needle".chars() {
        app.search_input_push(c);
    }
    app.search_commit();

    assert_eq!(
        app.search_status(),
        Some((1, 2)),
        "2 件見つかり 1 件目に居る"
    );
    assert!(!app.is_raw_source(), "検索のために raw ソースへ切替えない");
    let first_scroll = app.tab.preview_scroll;
    assert!(first_scroll > 0, "1 件目まで下へスクロールした");

    app.search_next(1);
    let second_scroll = app.tab.preview_scroll;
    assert!(
        second_scroll > first_scroll,
        "n で後方の一致へさらにスクロール: {first_scroll} → {second_scroll}"
    );
    app.search_next(1);
    assert_eq!(
        app.tab.preview_scroll, first_scroll,
        "wrap して 1 件目へ戻る"
    );

    // The match is inside the viewport, and it's highlighted (current match = orange background).
    let (lines, _) = app.md_slice(app.tab.preview_scroll, 10);
    let hit = lines.iter().any(|l| {
        l.spans
            .iter()
            .any(|s| s.content.contains("needle") && s.style.bg.is_some())
    });
    assert!(hit, "一致箇所が背景色で強調されている");

    // No match: don't move the cursor position, notify with a flash.
    let before = app.tab.preview_scroll;
    app.start_search();
    for c in "zzz-absent".chars() {
        app.search_input_push(c);
    }
    app.search_commit();
    assert_eq!(app.tab.preview_scroll, before, "一致なしでスクロールしない");
    assert_eq!(app.search_status(), None);

    std::fs::remove_dir_all(&dir).ok();
}

/// On a kitty terminal, prepare_image must rebuild the KittyImage even for **a terminal resize
/// (at fit zoom crop is unchanged, only the area changes)**. Without rebuilding, the image stays
/// at the old size even after the resize (regression).
#[test]
fn kitty_image_rebuilds_on_terminal_resize_at_fit() {
    let dir = std::env::temp_dir().join("konoma_kitty_resize_test");
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = App::new(dir, Config::default()).unwrap();
    // A source **larger** than the viewport (4000x3000px = 400x150 cells @ font 10x20). It gets
    // scaled down to fit the viewport, so a terminal resize = a display-size change = the hole
    // surfaces (a small image is unchanged at its natural size).
    app.image_src = Some(std::sync::Arc::new(image::DynamicImage::new_rgb8(
        4000, 3000,
    )));
    app.tab.preview_kind = Some(PreviewKind::Image(PathBuf::from("x.png")));
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    Box::leak(Box::new(rx));
    let mut picker = ratatui_image::picker::Picker::halfblocks();
    picker.set_protocol_type(ratatui_image::picker::ProtocolType::Kitty);
    app.attach_image_backend(picker, tx);
    assert!(app.uses_kitty_image(), "kitty 経路が有効");

    // Prepare the first render: a KittyImage that fits the viewport (200x40) via fit.
    let r1 = app.prepare_image(inner(200, 40)).unwrap();
    let size1 = app.kitty_image_ref().map(|k| k.cell_size());
    assert_eq!(size1, Some((r1.width, r1.height)), "表示セルサイズで構築");

    // Judge, via an **observable invariant**, that re-`prepare`ing over the same area does not
    // rebuild (a still image = not built every frame): render sets transmitted, and o=z is not
    // resent after that. If it were rebuilt, transmitted would revert to false and o=z would be
    // resent. A pointer comparison can't detect a rebuild because the field's address doesn't
    // change (review finding).
    let area1 = ratatui::layout::Rect::new(0, 0, r1.width.min(50), r1.height);
    let render_syms = |app: &App, area| {
        let mut b = ratatui::buffer::Buffer::empty(area);
        app.kitty_image_ref().unwrap().render(area, &mut b);
        b.content.iter().map(|c| c.symbol()).collect::<String>()
    };
    assert!(
        render_syms(&app, area1).contains("o=z"),
        "初回 render で転送"
    );
    app.prepare_image(inner(200, 40)).unwrap(); // area unchanged
    assert!(
        !render_syms(&app, area1).contains("o=z"),
        "領域不変なら作り直さない=転送を再送しない"
    );

    // The terminal grew (crop stays as fit's (0,0,sw,sh), unchanged; only the area grows) -> rebuild.
    let r2 = app.prepare_image(inner(300, 60)).unwrap();
    let size2 = app.kitty_image_ref().map(|k| k.cell_size());
    assert_ne!(size1, size2, "リサイズで表示サイズが変わる");
    assert_eq!(
        size2,
        Some((r2.width, r2.height)),
        "新しい表示サイズで再構築"
    );
    // Rebuilt = the new KittyImage is untransmitted -> o=z is resent.
    let area2 = ratatui::layout::Rect::new(0, 0, r2.width.min(50), r2.height);
    assert!(
        render_syms(&app, area2).contains("o=z"),
        "リサイズ後の新 KittyImage は転送を送る"
    );
}

/// On a non-kitty terminal (halfblocks/sixel/iterm2), the kitty path isn't used; the conventional
/// image (ThreadProtocol) path is used instead.
#[test]
fn non_kitty_terminal_keeps_ratatui_image_path() {
    let mut app = app_with_image(); // halfblocks
    app.attach_image_backend(
        ratatui_image::picker::Picker::halfblocks(),
        app.img_tx.clone().unwrap(),
    );
    assert!(
        !app.uses_kitty_image(),
        "halfblocks は kitty 経路を使わない"
    );
    app.prepare_image(inner(200, 40)).unwrap();
    assert!(app.kitty_image_ref().is_none(), "KittyImage は作らない");
    assert!(app.image.is_some(), "従来の ThreadProtocol 経路を使う");
}

/// A tab switch must not leak table search match highlighting into another tab. `table_search_hits`
/// is a rendering-side set derived from `search_matches`, but it's not in PerTab, so unless
/// load_active restores it, the previous tab's coordinates linger and the table renderer (which
/// references `table_cell_is_hit` unconditionally) wrongly highlights another tab's cells (a
/// missing-duplication finding from review).
#[test]
fn table_search_hits_do_not_leak_across_tabs() {
    let dir = unique_tmp("konoma_tab_table_search");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.csv"), "h1,h2\nfoo,bar\nfoo,baz\n").unwrap();
    std::fs::write(dir.join("b.csv"), "h1,h2\nq,w\ne,r\n").unwrap();
    let root = dir.canonicalize().unwrap();

    // Tab 0: open a.csv and search for "foo" (produces a matching cell).
    let mut app = App::new(root.clone(), Config::default()).unwrap();
    let a = root.join("a.csv");
    app.tab.preview_kind = Some(app.cfg.resolve_preview(&a));
    app.tab.preview_path = Some(a);
    app.tab.mode = Mode::Preview;
    app.load_table();
    app.start_search();
    for c in "foo".chars() {
        app.search_input_push(c);
    }
    app.search_commit();
    assert!(app.table_cell_is_hit(0, 0), "タブ0 で foo が一致");

    // Create a new tab 1 (empty tab), and open b.csv there without searching.
    app.tab_new().unwrap();
    let b = root.join("b.csv");
    app.tab.preview_kind = Some(app.cfg.resolve_preview(&b));
    app.tab.preview_path = Some(b);
    app.tab.mode = Mode::Preview;
    app.load_table();
    assert!(
        !app.table_cell_is_hit(0, 0),
        "新タブ(検索なし)に前タブの一致が漏れていない"
    );

    // Switch back to tab 0 -> the match is restored, switch to tab 1 again -> no leak.
    app.tab_cycle(-1);
    assert!(
        app.table_cell_is_hit(0, 0),
        "タブ0 に戻ると一致が復元される"
    );
    app.tab_cycle(1);
    assert!(
        !app.table_cell_is_hit(0, 0),
        "タブ1 へ切替で前タブの一致が漏れない(load_active が再構築)"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// The async path for a kitty build: the first time (the moment it's opened) it's synchronous and
/// shows immediately, subsequent zooms are offloaded to a worker, showing the old image until the
/// result arrives and swaps it in. Stale (an older generation) is discarded.
#[test]
fn kitty_zoom_builds_async_and_latest_wins() {
    let dir = std::env::temp_dir().join("konoma_kitty_async_test");
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.image_src = Some(std::sync::Arc::new(image::DynamicImage::new_rgb8(
        4000, 3000,
    )));
    app.tab.preview_kind = Some(PreviewKind::Image(PathBuf::from("x.png")));
    let (itx, irx) = tokio::sync::mpsc::unbounded_channel();
    Box::leak(Box::new(irx));
    let mut picker = ratatui_image::picker::Picker::halfblocks();
    picker.set_protocol_type(ratatui_image::picker::ProtocolType::Kitty);
    app.attach_image_backend(picker, itx);
    // Attach a real channel (the worker sends results on it).
    let (ktx, krx) = std::sync::mpsc::channel();
    app.attach_kitty_loader(ktx);

    // The first time is synchronous: kitty_image is ready right after prepare, and it's not
    // pending. Nothing is sent to the worker.
    let r1 = app.prepare_image(inner(200, 40)).unwrap();
    assert!(app.kitty_image_ref().is_some(), "初回は同期で即表示");
    assert!(!app.kitty_build_pending(), "初回同期後は in-flight でない");
    let size1 = app.kitty_image_ref().unwrap().cell_size();
    assert_eq!(size1, (r1.width, r1.height));
    assert!(krx.try_recv().is_err(), "初回は worker を使わない(同期)");

    // Zoom: async from here on. kitty_image stays at the old size, pending=true.
    app.tab.image_zoom = 4.0;
    app.prepare_image(inner(200, 40)).unwrap();
    assert!(app.kitty_build_pending(), "ズーム後は build in-flight");
    assert_eq!(
        app.kitty_image_ref().unwrap().cell_size(),
        size1,
        "結果到着まで旧画像を見せ続ける(ちらつかせない)"
    );

    // Receive the worker's result and apply it -> swap to the new size, pending clears.
    let res = krx.recv().expect("worker sends a result");
    assert!(app.apply_kitty(res), "最新世代の結果を適用");
    assert!(!app.kitty_build_pending(), "適用後は in-flight でない");
    let size2 = app.kitty_image_ref().unwrap().cell_size();
    assert_ne!(size1, size2, "ズームで表示サイズが変わった");

    // Consecutive zooms (2 spawns) -> only the latest generation is applied, the older one is
    // discarded (threads run concurrently = arrival order is non-deterministic, so "arrives
    // first" doesn't necessarily mean "older". Judged by gen, so regardless of order exactly one,
    // the latest, gets applied).
    app.tab.image_zoom = 6.0;
    app.prepare_image(inner(200, 40)).unwrap();
    app.tab.image_zoom = 8.0;
    app.prepare_image(inner(200, 40)).unwrap();
    let a = krx.recv().unwrap();
    let b = krx.recv().unwrap();
    let applied = u32::from(app.apply_kitty(a)) + u32::from(app.apply_kitty(b));
    assert_eq!(applied, 1, "2つの結果のうち最新世代の1つだけが適用される");
    assert!(!app.kitty_build_pending());

    std::fs::remove_dir_all(&dir).ok();
}

/// PDF page navigation / video re-thumbnail regression: `set_static_image` swaps in a raster of the
/// same display size in place. The kitty path decides whether to rebuild based on geometry
/// (kitty_want), so with the same size want doesn't change and **the previous page's pixels
/// linger**. The kitty geometry must be invalidated on swap so a rebuild always happens.
#[test]
fn kitty_rebuilds_on_same_size_image_swap() {
    let dir = std::env::temp_dir().join("konoma_kitty_swap_test");
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.image_src = Some(std::sync::Arc::new(image::DynamicImage::new_rgb8(800, 600)));
    app.tab.preview_kind = Some(PreviewKind::Image(PathBuf::from("doc.pdf")));
    let (itx, irx) = tokio::sync::mpsc::unbounded_channel();
    Box::leak(Box::new(irx));
    let mut picker = ratatui_image::picker::Picker::halfblocks();
    picker.set_protocol_type(ratatui_image::picker::ProtocolType::Kitty);
    app.attach_image_backend(picker, itx); // no kitty_tx = synchronous build

    let r = app.prepare_image(inner(200, 40)).unwrap();
    let area = ratatui::layout::Rect::new(0, 0, r.width.min(40), r.height);
    let render_syms = |app: &App| {
        let mut b = ratatui::buffer::Buffer::empty(area);
        app.kitty_image_ref().unwrap().render(area, &mut b);
        b.content.iter().map(|c| c.symbol()).collect::<String>()
    };
    assert!(render_syms(&app).contains("o=z"), "初回 render で転送");
    assert!(
        !render_syms(&app).contains("o=z"),
        "同一画像の再 render は転送しない"
    );

    // Swap in page 2 at the same size (800x600) in place (same as pdf_goto -> apply_media -> set_static_image).
    app.set_static_image(image::DynamicImage::new_rgb8(800, 600));
    app.prepare_image(inner(200, 40)).unwrap();
    assert!(
        render_syms(&app).contains("o=z"),
        "同寸法の差し替えでも kitty 画像が再ビルドされる(前ページが居残らない)"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// Even when an async kitty build fails (worker panic -> None), `kitty_build_pending` must not stay
/// permanently true. If it did, the run loop would keep 16ms polling and the idle 0% CPU target
/// would break (review LOW finding).
#[test]
fn kitty_failed_build_clears_pending() {
    let dir = std::env::temp_dir().join("konoma_kitty_fail_test");
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.image_src = Some(std::sync::Arc::new(image::DynamicImage::new_rgb8(
        4000, 3000,
    )));
    app.tab.preview_kind = Some(PreviewKind::Image(PathBuf::from("x.png")));
    let (itx, irx) = tokio::sync::mpsc::unbounded_channel();
    Box::leak(Box::new(irx));
    let mut picker = ratatui_image::picker::Picker::halfblocks();
    picker.set_protocol_type(ratatui_image::picker::ProtocolType::Kitty);
    app.attach_image_backend(picker, itx);
    let (ktx, krx) = std::sync::mpsc::channel();
    app.attach_kitty_loader(ktx);
    Box::leak(Box::new(krx)); // don't use the real worker's result (a failure is injected instead)

    // Initial sync -> async build in-flight via zoom (pending=true).
    app.prepare_image(inner(200, 40)).unwrap();
    app.tab.image_zoom = 4.0;
    app.prepare_image(inner(200, 40)).unwrap();
    assert!(app.kitty_build_pending(), "ズーム後は in-flight");

    // Inject a build failure (None) for the current generation -> apply returns false but pending clears.
    let gen = app.kitty_gen;
    assert!(
        !app.apply_kitty(KittyResult { gen, image: None }),
        "失敗は適用しない"
    );
    assert!(
        !app.kitty_build_pending(),
        "失敗しても pending は解消(16ms ビジーポーリング防止)"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// The main cause of `h`/`l` being heavy in a large repo: `git status` (a whole-worktree scan) was
/// run synchronously on every root change within the same repository. `git status` runs from the
/// workdir so the result is identical -> if the workdir is the same and not dirty, reuse it without
/// recomputing (same pattern as ignored's Phase G). Re-fetch if dirty / a different repo.
#[cfg(feature = "git")]
#[test]
fn same_repo_navigation_reuses_status_without_recompute() {
    use std::process::Command;
    let dir = std::env::temp_dir().join("konoma_status_workdir_cache");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    init_git_repo(&dir);
    let git = |args: &[&str]| {
        Command::new("git")
            .current_dir(&dir)
            .args(args)
            .output()
            .unwrap();
    };
    std::fs::write(dir.join("a.txt"), b"v1\n").unwrap();
    git(&["add", "-A"]);
    git(&["commit", "-q", "-m", "init"]);
    std::fs::write(dir.join("a.txt"), b"v2\n").unwrap(); // change = status non-empty

    let root = dir.canonicalize().unwrap();
    let mut app = App::new(root.clone(), Config::default()).unwrap();
    app.refresh_git_if_needed(); // 1st time: compute statuses + establish the workdir cache
    assert!(
        app.git_status_of(&root.join("a.txt")).is_some(),
        "変更が見える"
    );

    // Plant a sentinel (a fake entry that doesn't exist) to observe whether status was
    // recomputed. If a recompute runs, git_status is replaced wholesale and the sentinel
    // disappears (white-box: direct field manipulation).
    let sentinel = root.join("__sentinel_not_a_real_file__");
    app.git_status
        .insert(sentinel.clone(), crate::git::FileStatus::Modified);

    // Descend into a subdirectory of the same repo (root changes, workdir unchanged, not dirty)
    // -> must not recompute.
    app.tab.root = root.join("sub");
    app.refresh_git_if_needed();
    assert!(
        app.git_status_of(&sentinel).is_some(),
        "同一 workdir の潜行は status を再計算しない(センチネル残存=キャッシュ流用)"
    );

    // dirty (re-validation for e.g. an external commit) -> re-fetch (sentinel disappears).
    app.git_status_dirty = true;
    app.refresh_git_if_needed();
    assert!(
        app.git_status_of(&sentinel).is_none(),
        "dirty 指定で status を再計算(センチネル消失)"
    );
    // The real data is correctly fetched (changes across the whole workdir are visible even from a subdirectory).
    assert!(
        app.git_status_of(&root.join("a.txt")).is_some(),
        "再計算後も a.txt の変更は見える"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// The per-workdir status cache must **re-fetch when moving to a different repo** (must not reuse
/// the parent repo's status for a nested repo). The statuses counterpart of
/// `descend_into_nested_different_repo_recomputes_ignored_set` = the core of the cache's correctness.
#[cfg(feature = "git")]
#[test]
fn descend_into_nested_different_repo_recomputes_status() {
    let dir = std::env::temp_dir().join("konoma_status_reuse_diff_repo");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("inner")).unwrap();
    init_git_repo(&dir); // outer repoA
    init_git_repo(&dir.join("inner")); // nested, different repoB

    let root = dir.canonicalize().unwrap();
    let mut app = App::new(root.clone(), Config::default()).unwrap();
    app.refresh_git_if_needed();
    let wd_a = app.git_status_workdir.clone();
    assert!(wd_a.is_some(), "外側 repo の status workdir が確立");

    // Sentinel (a fake entry that doesn't exist) to observe recomputation. Disappears on refetch.
    let sentinel = root.join("__status_sentinel__");
    app.git_status
        .insert(sentinel.clone(), crate::git::FileStatus::Modified);

    // Descend into inner (a different repo).
    let i = app
        .tab
        .entries
        .iter()
        .position(|e| e.is_dir && e.path.ends_with("inner"))
        .expect("inner エントリが無い");
    app.tab.selected = i;
    app.tree_descend().unwrap();
    app.refresh_git_if_needed();

    assert_ne!(
        app.git_status_workdir, wd_a,
        "別 repo: workdir が変わったので status キャッシュキーも変わる"
    );
    assert!(
        app.git_status_of(&sentinel).is_none(),
        "別 repo へ移ると status を作り直す(親 repo の status を流用しない)"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// If an async build (gen=N) is kicked off by zooming image A, and it is then **switched to a
/// different file**, the gen=N result that arrives late must be discarded (clear_image bumps
/// gen). Without discarding it, A's image would leak into B.
#[test]
fn kitty_stale_build_from_previous_file_is_discarded_on_switch() {
    let dir = std::env::temp_dir().join("konoma_kitty_switch_race");
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.image_src = Some(std::sync::Arc::new(image::DynamicImage::new_rgb8(
        4000, 3000,
    )));
    app.tab.preview_kind = Some(PreviewKind::Image(PathBuf::from("a.png")));
    let (itx, irx) = tokio::sync::mpsc::unbounded_channel();
    Box::leak(Box::new(irx));
    let mut picker = ratatui_image::picker::Picker::halfblocks();
    picker.set_protocol_type(ratatui_image::picker::ProtocolType::Kitty);
    app.attach_image_backend(picker, itx);
    let (ktx, krx) = std::sync::mpsc::channel();
    app.attach_kitty_loader(ktx);

    // Open A (initial synchronous), then kick off async build gen=N via zoom.
    app.prepare_image(inner(200, 40)).unwrap();
    app.tab.image_zoom = 4.0;
    app.prepare_image(inner(200, 40)).unwrap();
    let stale = krx.recv().expect("Aのズームの worker 結果"); // the gen=N result

    // Switch to a different file B (clear_image bumps gen = staleness).
    app.clear_image();

    // The late-arriving gen=N result for A is discarded and does not leak into B (which has no image yet).
    assert!(
        !app.apply_kitty(stale),
        "切替後に届いた旧ファイルのビルド結果は破棄される"
    );
    assert!(
        app.kitty_image_ref().is_none(),
        "破棄されたので画像は入らない(clear_image で None のまま)"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// `git_dir_watch`: when root is a subdirectory of a repo, returns the parent `.git` as the watch
/// target; for a repo root or a non-repo it returns None (already covered by recursive watching /
/// no watch needed). Plugs the gap so external git operations are picked up at a subdirectory root.
#[cfg(feature = "git")]
#[test]
fn git_dir_watch_targets_dot_git_only_for_subdir_root() {
    let dir = std::env::temp_dir().join("konoma_git_dir_watch");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src")).unwrap();
    init_git_repo(&dir);
    let root = dir.canonicalize().unwrap();

    // repo root: `.git` is already under recursive watching -> None.
    let app_root = App::new(root.clone(), Config::default()).unwrap();
    assert_eq!(app_root.git_dir_watch(), None, "repo root は追加監視不要");

    // Subdirectory root: the parent `.git` is not watched -> return that `.git`.
    let app_sub = App::new(root.join("src"), Config::default()).unwrap();
    assert_eq!(
        app_sub.git_dir_watch(),
        Some(root.join(".git")),
        "subdir root は親 .git を監視対象に返す"
    );

    // Non-repo: None.
    let plain = std::env::temp_dir().join("konoma_git_dir_watch_norepo");
    let _ = std::fs::remove_dir_all(&plain);
    std::fs::create_dir_all(&plain).unwrap();
    let app_plain = App::new(plain.canonicalize().unwrap(), Config::default()).unwrap();
    assert_eq!(app_plain.git_dir_watch(), None, "非 repo は監視不要");

    std::fs::remove_dir_all(&dir).ok();
    std::fs::remove_dir_all(&plain).ok();
}

/// `git_dir_watch` for a **linked worktree** (`git worktree add`): that worktree's own `HEAD`/`index`
/// live under the main repo's `.git/worktrees/<name>/`, entirely outside the worktree's own checkout,
/// so no event under the worktree root — however deep — ever used to reach the recursive watch.
/// Before the fix this returned `None` for two independent reasons (root == `workdir()`'s result, and
/// the worktree's own `.git` is a *file*, not a directory), so an external `git commit`/`checkout` on
/// a linked worktree root was never picked up and stale `M`/`U` markers lingered forever.
#[cfg(feature = "git")]
#[test]
fn git_dir_watch_targets_worktrees_git_dir_for_linked_worktree_root() {
    let main_dir = unique_tmp("konoma_git_dir_watch_wt_main");
    std::fs::create_dir_all(&main_dir).unwrap();
    init_git_repo(&main_dir);
    let main_root = main_dir.canonicalize().unwrap();

    // `git worktree add` creates the target directory itself, so pick a path that doesn't exist yet.
    let wt_dir = unique_tmp("konoma_git_dir_watch_wt_linked");
    let out = std::process::Command::new("git")
        .arg("worktree")
        .arg("add")
        .arg("-q")
        .arg("-b")
        .arg("konoma-wt-test-branch")
        .arg(&wt_dir)
        .current_dir(&main_root)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git worktree add failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let wt_root = wt_dir.canonicalize().unwrap();
    std::fs::create_dir_all(wt_root.join("sub")).unwrap();

    // Ask git itself where this worktree's git dir lives, instead of hard-coding git's
    // worktree-naming rules in the test.
    let gd_out = std::process::Command::new("git")
        .args(["rev-parse", "--git-dir"])
        .current_dir(&wt_root)
        .output()
        .unwrap();
    assert!(gd_out.status.success());
    let raw = String::from_utf8(gd_out.stdout).unwrap().trim().to_string();
    let expected_git_dir = std::path::PathBuf::from(&raw);
    let expected_git_dir = if expected_git_dir.is_absolute() {
        expected_git_dir
    } else {
        wt_root.join(expected_git_dir)
    };
    let expected_git_dir = expected_git_dir.canonicalize().unwrap();
    assert!(
        expected_git_dir.starts_with(main_root.join(".git").join("worktrees")),
        "sanity: a linked worktree's git dir must live under the main repo's .git/worktrees/, got {expected_git_dir:?}"
    );

    // Linked worktree root: its own HEAD/index live entirely outside its checkout -> must be watched.
    let app_wt = App::new(wt_root.clone(), Config::default()).unwrap();
    assert_eq!(
        app_wt.git_dir_watch(),
        Some(expected_git_dir.clone()),
        "linked worktree root は自分の .git/worktrees/<name> を監視対象に返す"
    );

    // A subdirectory of the linked worktree: same target.
    let app_wt_sub = App::new(wt_root.join("sub"), Config::default()).unwrap();
    assert_eq!(
        app_wt_sub.git_dir_watch(),
        Some(expected_git_dir),
        "linked worktree のサブディレクトリでも同じ監視対象を返す"
    );

    std::process::Command::new("git")
        .arg("worktree")
        .arg("remove")
        .arg("--force")
        .arg(&wt_root)
        .current_dir(&main_root)
        .output()
        .ok();
    std::fs::remove_dir_all(&wt_dir).ok();
    std::fs::remove_dir_all(&main_dir).ok();
}

/// A tab switch (load_active) is a checkpoint for re-validating git status: it sets dirty so that
/// external changes missed by a background tab's watch are picked up, and status is re-fetched on
/// the render after the switch (does not go stale even across **different subdirectory** tabs of
/// the same repo). With both tabs on different subdirs, the destination root differs from
/// git_status_for, so without dirty the workdir cache would be reused and the staleness would
/// linger (this setup isolates and verifies dirty).
#[cfg(feature = "git")]
#[test]
fn tab_switch_re_verifies_git_status() {
    let dir = std::env::temp_dir().join("konoma_tab_switch_status");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("subx")).unwrap();
    std::fs::create_dir_all(dir.join("suby")).unwrap();
    init_git_repo(&dir);
    let root = dir.canonicalize().unwrap();

    let descend = |app: &mut App, name: &str| {
        let i = app
            .tab
            .entries
            .iter()
            .position(|e| e.is_dir && e.path.ends_with(name))
            .unwrap_or_else(|| panic!("{name} エントリが無い"));
        app.tab.selected = i;
        app.tree_descend().unwrap();
    };

    // Put tab 0 at subx. Establish status (git_status_for=subx).
    let mut app = App::new(root.clone(), Config::default()).unwrap();
    descend(&mut app, "subx");
    app.refresh_git_if_needed();

    // Create tab 1, move to suby, and refresh (git_status_for updates to suby; same workdir so reused).
    app.tab_new().unwrap();
    app.tree_leave().unwrap(); // back to the repo root
    descend(&mut app, "suby");
    app.refresh_git_if_needed();

    // Plant a sentinel in the current git_status to observe whether it gets recomputed.
    let sentinel = root.join("__tab_status_sentinel__");
    app.git_status
        .insert(sentinel.clone(), crate::git::FileStatus::Modified);

    // Switch back to tab 0 (subx). The destination root (subx) != git_status_for (suby), so
    // without dirty the workdir cache would be reused and the sentinel would remain. With dirty
    // it re-fetches and the sentinel disappears.
    app.tab_cycle(-1);
    app.refresh_git_if_needed(); // equivalent to the render after the switch

    assert!(
        app.git_status_of(&sentinel).is_none(),
        "タブ切替で status を再検証(センチネル消失=キャッシュ流用しない)"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// The whole-worktree `git status` scan must run on a **worker thread**, never on the UI thread.
/// Regression guard for "switching tabs in a git repo freezes for the duration of `git status`":
/// `refresh_git_if_needed` used to call `crate::git::statuses()` inline, so every root/tab change paid
/// the full scan synchronously (~5ms even on a 6-file repo, hundreds of ms on a large one).
#[cfg(feature = "git")]
#[test]
fn git_status_scan_is_offloaded_to_a_worker_thread() {
    let dir = std::env::temp_dir().join("konoma_status_async_offload");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    init_git_repo(&dir);
    std::fs::write(dir.join("changed.txt"), b"x").unwrap();
    let dir = dir.canonicalize().unwrap();

    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    app.attach_status_loader(tx);

    app.refresh_git_if_needed();

    // The old implementation synchronously computed statuses at this point and even filled in
    // workdir (= the UI would stall).
    assert!(
        app.git_status_pending.is_some(),
        "別スレッドへ計算を投げているはず"
    );
    assert!(
        app.git_status_workdir.is_none(),
        "UI スレッドでは status を計算しない(旧実装ならここで Some になり落ちる)"
    );

    // It's only reflected once the worker's result is received and applied.
    let res = rx
        .recv_timeout(std::time::Duration::from_secs(30))
        .expect("ワーカーが結果を返す");
    assert!(app.apply_statuses(res), "現世代の結果は適用される");
    assert!(
        app.git_status_workdir.is_some(),
        "適用後は workdir が埋まる"
    );
    assert!(app.git_status_pending.is_none(), "適用で pending が解ける");
    assert!(
        app.git_status_of(&dir.join("changed.txt")).is_some(),
        "未追跡ファイルの変更マーカーが反映される"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// Switching tabs must still **re-validate** git status (background tabs miss FS events), but must not
/// block the keypress to do it. Guards both halves: a scan is requested, yet the previously known
/// statuses are still in place right after the switch (proving nothing was computed inline).
#[cfg(feature = "git")]
#[test]
fn tab_switch_requests_git_status_without_blocking() {
    let dir = std::env::temp_dir().join("konoma_status_async_tabswitch");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    init_git_repo(&dir);
    std::fs::write(dir.join("first.txt"), b"x").unwrap();
    let dir = dir.canonicalize().unwrap();

    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    app.attach_status_loader(tx);
    // Settle the initial state (only first.txt is visible).
    app.refresh_git_if_needed();
    let res = rx
        .recv_timeout(std::time::Duration::from_secs(30))
        .expect("初回スキャン");
    assert!(app.apply_statuses(res));
    assert!(app.git_status_of(&dir.join("first.txt")).is_some());

    // Another file appears in the background (= a change that should be re-fetched on switch).
    std::fs::write(dir.join("second.txt"), b"y").unwrap();

    // Create and switch to a second tab on the same root.
    app.tab_new().unwrap();
    app.tab_cycle(1);

    assert!(
        app.git_status_pending.is_some(),
        "タブ切替は再検証を要求する(背面タブは fs イベントを取りこぼすため)"
    );
    assert!(
        app.git_status_of(&dir.join("second.txt")).is_none(),
        "切替の時点ではまだ同期計算していない(旧実装なら既に second.txt が見えて落ちる)"
    );

    // The new file only becomes visible once the worker's result arrives.
    let mut applied = false;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while std::time::Instant::now() < deadline {
        if let Ok(res) = rx.recv_timeout(std::time::Duration::from_secs(30)) {
            applied |= app.apply_statuses(res);
            if app.git_status_pending.is_none() {
                break;
            }
        }
    }
    assert!(applied, "切替で投げたスキャンが適用される");
    assert!(
        app.git_status_of(&dir.join("second.txt")).is_some(),
        "再検証の結果、裏で増えたファイルが反映される"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// A scan that finishes after the user already moved on must be discarded (generation guard), so a slow
/// scan of the previous root can never overwrite the status of the root now on screen.
#[cfg(feature = "git")]
#[test]
fn stale_git_status_result_is_discarded() {
    let dir = std::env::temp_dir().join("konoma_status_async_stale");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    init_git_repo(&dir);
    let dir = dir.canonicalize().unwrap();

    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    let (tx, _rx) = std::sync::mpsc::channel();
    app.attach_status_loader(tx);
    app.refresh_git_if_needed();
    let stale_gen = app.git_status_gen;

    // Move to a different root and kick a scan of the new generation.
    app.git_status_dirty = true;
    app.tab.root = dir.join("sub");
    app.refresh_git_if_needed();
    assert_ne!(stale_gen, app.git_status_gen, "世代が進む");

    let sentinel = dir.join("__sentinel__");
    let stale = crate::app::StatusResult {
        gen: stale_gen,
        workdir: Some(dir.clone()),
        statuses: std::collections::HashMap::from([(
            sentinel.clone(),
            crate::git::FileStatus::Modified,
        )]),
        branch: Some("stale".into()),
        worktree_origin: None,
    };
    assert!(!app.apply_statuses(stale), "古い世代の結果は捨てる");
    assert!(
        app.git_status_of(&sentinel).is_none(),
        "捨てた結果で status を汚染しない"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// The render path must not re-dispatch a scan on every frame while one is in flight, and must not
/// re-scan at all once the result has landed (the per-workdir cache = Phase G).
#[cfg(feature = "git")]
#[test]
fn repeated_renders_dispatch_at_most_one_git_status_scan() {
    let dir = std::env::temp_dir().join("konoma_status_async_norekick");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    init_git_repo(&dir);
    std::fs::write(dir.join("a.txt"), b"x").unwrap();
    let dir = dir.canonicalize().unwrap();

    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    app.attach_status_loader(tx);

    // Repeat the render-equivalent 20 times **without applying** the result yet (= rendering while
    // a scan is in flight).
    for _ in 0..20 {
        app.refresh_git_if_needed();
    }
    let res = rx
        .recv_timeout(std::time::Duration::from_secs(30))
        .expect("1本目の結果");
    // Count the number launched by the number of results that arrive on the channel (STATUS_CALLS
    // is process-shared and picks up parallel tests).
    assert_eq!(
        rx.try_iter().count(),
        0,
        "走行中は描画のたびに git status を投げ直してはいけない"
    );
    assert!(app.apply_statuses(res));

    // Further rendering after applying still must not rescan (per-workdir cache = Phase G).
    for _ in 0..20 {
        app.refresh_git_if_needed();
    }
    assert_eq!(
        rx.try_iter().count(),
        0,
        "適用後も描画のたびに再スキャンしてはいけない"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// Re-validation requests that arrive while a scan is running must be **coalesced** into a single
/// follow-up scan, not spawn one scan per event. The synchronous implementation was self-throttling
/// (the next event waited for the scan); the async one is not, so without coalescing an agent writing
/// files would stack up one whole-worktree scan per FS event — worst exactly on the large repos this
/// change is meant to help.
#[cfg(feature = "git")]
#[test]
fn concurrent_refresh_requests_are_coalesced_into_one_rescan() {
    let dir = std::env::temp_dir().join("konoma_status_async_coalesce");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    init_git_repo(&dir);
    std::fs::write(dir.join("a.txt"), b"x").unwrap();
    let dir = dir.canonicalize().unwrap();

    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    app.attach_status_loader(tx);
    app.refresh_git_if_needed(); // start the 1st (left unapplied = in flight)

    // 10 consecutive fs events arrive while it's in flight (equivalent to an agent's write burst).
    for i in 0..10 {
        std::fs::write(dir.join(format!("burst_{i}.txt")), b"y").unwrap();
        app.refresh_fs(false).unwrap();
    }
    // Count the number of scans launched by "the number of results that arrive on the channel"
    // (don't use the process-shared STATUS_CALLS, since it also picks up git calls from other
    // tests running in parallel).
    let first = rx
        .recv_timeout(std::time::Duration::from_secs(30))
        .expect("1本目");
    assert_eq!(
        rx.try_iter().count(),
        0,
        "走行中の再検証要求はスレッドを増やさない(合体させる)"
    );

    // Apply the 1st result -> the queued-up requests are carried over into **exactly one** rescan.
    app.apply_statuses(first);
    let second = rx
        .recv_timeout(std::time::Duration::from_secs(30))
        .expect("合体した再スキャンが1本走る");
    app.apply_statuses(second);
    assert_eq!(
        rx.try_iter().count(),
        0,
        "10 イベントぶんの要求は 1 回の再スキャンに合体される(何本も走らない)"
    );
    assert!(
        app.git_status_of(&dir.join("burst_9.txt")).is_some(),
        "合体後の再スキャンで最新の変更まで反映される(バーストの最後を取りこぼさない)"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// `C` (changed-only filter) derives its entry list from the statuses, so the list must be rebuilt when
/// an async scan lands — a repaint alone cannot fix it. Guards the Agent Watch live-update path.
#[cfg(feature = "git")]
#[test]
fn changed_filter_list_follows_async_status_results() {
    let dir = std::env::temp_dir().join("konoma_status_async_changedfilter");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    init_git_repo(&dir);
    std::fs::write(dir.join("first.txt"), b"x").unwrap();
    let dir = dir.canonicalize().unwrap();

    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    app.attach_status_loader(tx);
    app.toggle_changed_filter(); // `C`: synchronous contract, so the list is up right there
    assert!(app.tab.changed_filter, "変更ファイルのみ表示になる");
    assert!(app
        .tab
        .entries
        .iter()
        .any(|e| e.path.ends_with("first.txt")));

    // An agent creates a new file -> fs event (only kicks the async scan).
    std::fs::write(dir.join("agent_new.txt"), b"y").unwrap();
    app.refresh_fs(false).unwrap();
    // It only lands in the list once the result arrives.
    let res = rx
        .recv_timeout(std::time::Duration::from_secs(30))
        .expect("スキャン結果");
    app.apply_statuses(res);
    assert!(
        app.tab
            .entries
            .iter()
            .any(|e| e.path.ends_with("agent_new.txt")),
        "非同期スキャンの到着で C の一覧が作り直される(再描画だけでは直らない)"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// Commands that answer *from* the status (`d` = open diff) must not report "no changes" merely because
/// a background scan is still running: that reads as a broken feature rather than a slow one.
#[cfg(feature = "git")]
#[test]
fn open_diff_command_waits_for_status_instead_of_reporting_no_changes() {
    let dir = std::env::temp_dir().join("konoma_status_async_opendiff");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    init_git_repo(&dir);
    std::fs::write(dir.join("tracked.txt"), b"one\n").unwrap();
    let dir = dir.canonicalize().unwrap();
    std::process::Command::new("git")
        .args(["add", "-A"])
        .current_dir(&dir)
        .output()
        .unwrap();
    std::process::Command::new("git")
        .args(["commit", "-m", "init"])
        .current_dir(&dir)
        .output()
        .unwrap();
    std::fs::write(dir.join("tracked.txt"), b"one\ntwo\n").unwrap();

    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    let (tx, _rx) = std::sync::mpsc::channel();
    app.attach_status_loader(tx); // status is async from here on (= not yet arrived at the moment of the keypress)
    app.tab.selected = app
        .tab
        .entries
        .iter()
        .position(|e| e.path.ends_with("tracked.txt"))
        .unwrap();

    app.tree_open_git_diff();
    assert!(
        matches!(app.tab.preview_kind, Some(PreviewKind::GitDiff(_))),
        "スキャン走行中でも `d` は diff を開く(flash={:?})",
        app.flash
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// A landing scan result must not rewind `git_status_for` to the root the scan *started* at. Within one
/// repo `l`/`h` take the cache-reuse path (no new generation), so the result can land after the root has
/// moved; writing the old root back made the next FS event's `refresh_git_status_only` early-return and
/// **drop a whole re-validation** — the last write of an agent's burst could then stay invisible until
/// the next tab switch.
#[cfg(feature = "git")]
#[test]
fn landing_status_result_keeps_tracking_the_current_root() {
    use std::sync::atomic::Ordering;
    let dir = std::env::temp_dir().join("konoma_status_async_rootrewind");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    init_git_repo(&dir);
    std::fs::write(dir.join("sub").join("a.txt"), b"x").unwrap();
    let dir = dir.canonicalize().unwrap();

    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    app.attach_status_loader(tx);
    // Apply once to establish the workdir cache (from here on `l` takes the reuse path = the
    // generation does not advance).
    app.refresh_git_if_needed();
    let res = rx
        .recv_timeout(std::time::Duration::from_secs(30))
        .expect("初回");
    assert!(app.apply_statuses(res));

    // Request re-validation to start a scan (left unapplied), and while it's in flight, descend
    // into sub with `l`.
    app.git_status_dirty = true;
    app.refresh_git_if_needed(); // kick (root at this point = repo root)
    let i = app
        .tab
        .entries
        .iter()
        .position(|e| e.is_dir && e.path.ends_with("sub"))
        .expect("sub");
    app.tab.selected = i;
    app.tree_descend().unwrap();
    app.refresh_git_if_needed(); // same repo = reuse path (generation does not advance)
    assert!(app.tab.root.ends_with("sub"));

    // The result that was in flight arrives right here.
    let res = rx
        .recv_timeout(std::time::Duration::from_secs(30))
        .expect("走行中だった結果");
    app.apply_statuses(res);

    // A fs event right after this must still be able to kick a re-validation (the old
    // implementation dropped it entirely via an early return).
    crate::git::STATUS_CALLS.store(0, Ordering::SeqCst);
    std::fs::write(dir.join("sub").join("burst.txt"), b"y").unwrap();
    app.refresh_fs(false).unwrap();
    assert!(
        app.git_status_pending.is_some(),
        "結果適用の直後でも fs イベントの再検証は投げられる(取りこぼさない)"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// Switching to a tab must build its preview **once**. `load_active` rebuilds the preview from disk
/// itself, and then used to hand off to the generic FS-refresh path, whose `reload_preview()` repeated
/// the same work — a table tab re-parsed the whole CSV twice on every switch.
#[test]
fn tab_switch_parses_a_table_preview_only_once() {
    let dir = std::env::temp_dir().join(unique_tmp("konoma_tabswitch_single_parse"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut csv = String::from("a,b,c\n");
    for i in 0..200 {
        csv.push_str(&format!("{i},x,y\n"));
    }
    std::fs::write(dir.join("data.csv"), &csv).unwrap();
    std::fs::write(dir.join("other.txt"), b"plain\n").unwrap();

    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    // Tab 1 = preview of data.csv, tab 2 = tree.
    app.tab.selected = app
        .tab
        .entries
        .iter()
        .position(|e| e.path.ends_with("data.csv"))
        .unwrap();
    app.tree_activate().unwrap();
    assert!(matches!(
        app.tab.preview_kind,
        Some(PreviewKind::Table { .. })
    ));
    app.tab_new().unwrap();

    // Switch back to tab 1 (the table) = load_active runs.
    crate::preview::table::PARSE_CALLS.with(|c| c.set(0));
    app.tab_cycle(1);
    assert!(
        matches!(app.tab.preview_kind, Some(PreviewKind::Table { .. })),
        "表タブへ戻っている"
    );
    assert_eq!(
        crate::preview::table::PARSE_CALLS.with(|c| c.get()),
        1,
        "タブ切替での CSV 全行パースは1回だけ(旧実装は load_active と reload_preview で2回)"
    );
    assert!(app.table_data.is_some(), "表の中身は復元されている");
    std::fs::remove_dir_all(&dir).ok();
}

/// Returning to a media tab must reuse the decoded image instead of redoing the work that produced it
/// (an image decode, an SVG/mermaid rasterization, or a `pdftocairo`/`ffmpeg` run costing hundreds of
/// milliseconds). Reuse is keyed on `(path, mtime, page)`, so an externally edited file is still re-read.
#[test]
fn returning_to_a_media_tab_reuses_the_decoded_image() {
    let dir = std::env::temp_dir().join(unique_tmp("konoma_media_cache"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let img = dir.join("pic.png");
    std::fs::write(&img, b"not-a-real-png").unwrap(); // content is unused (the decoded result is set directly below)
    std::fs::write(dir.join("note.txt"), b"x").unwrap();

    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    // Build a state equivalent to an image preview (a decoded state can be reproduced even without a picker).
    app.tab.selected = app
        .tab
        .entries
        .iter()
        .position(|e| e.path.ends_with("pic.png"))
        .unwrap();
    app.tab.mode = Mode::Preview;
    app.tab.preview_path = Some(img.clone());
    app.tab.preview_kind = Some(PreviewKind::Image(img.clone()));
    let decoded = std::sync::Arc::new(image::DynamicImage::ImageRgba8(image::RgbaImage::new(4, 4)));
    app.image_src = Some(decoded.clone());
    app.preview_media_mtime = crate::app::file_mtime(&img);

    // Switch to another tab, then come back.
    app.tab_new().unwrap();
    app.tab_cycle(1);

    assert!(
        app.image_src.is_some(),
        "戻ったタブの画像が復元されている(旧実装は毎回デコードし直していた)"
    );
    assert!(
        std::sync::Arc::ptr_eq(app.image_src.as_ref().unwrap(), &decoded),
        "同一のデコード済み画像を再利用している(作り直していない)"
    );

    // Don't reuse it if the file changed externally (don't show a stale picture).
    app.tab_cycle(1); // stash
    std::thread::sleep(std::time::Duration::from_millis(10));
    std::fs::write(&img, b"changed-bytes").unwrap();
    app.tab_cycle(1); // back to the image tab
    assert!(
        app.image_src.is_none()
            || !std::sync::Arc::ptr_eq(app.image_src.as_ref().unwrap(), &decoded),
        "mtime が変わったら退避分を使い回さない(外部編集に追従)"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// The one-slot media cache must not turn into a hidden hundreds-of-megabytes resident buffer. The cap
/// is measured in **bytes, not pixels**: a 16-bit image is 8 bytes per pixel (HDR/EXR is 12), so a
/// pixel-count cap lets exactly the heaviest formats through.
#[test]
fn oversized_media_is_not_cached() {
    let dir = std::env::temp_dir().join(unique_tmp("konoma_media_cache_cap"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let img = dir.join("huge.png");
    std::fs::write(&img, b"x").unwrap();
    std::fs::write(dir.join("note.txt"), b"x").unwrap();

    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.tab.mode = Mode::Preview;
    app.tab.preview_path = Some(img.clone());
    app.tab.preview_kind = Some(PreviewKind::Image(img.clone()));
    app.preview_media_mtime = crate::app::file_mtime(&img);
    // 17M pixels = looking only at the pixel count it would slip under a "32MP cap", but at
    // 16-bit (8B/px) that's 136MB. Squeeze the buffer into 1 row to keep the memory at its true
    // size (an image that's only extremely wide).
    let huge = image::DynamicImage::ImageRgba16(image::ImageBuffer::new(17_000_000, 1));
    assert!(
        huge.as_bytes().len() > 128 * 1024 * 1024,
        "テスト前提: 128MiB を超えるバッファ"
    );
    app.image_src = Some(std::sync::Arc::new(huge));

    app.tab_new().unwrap(); // stashing runs via save_active
    assert!(
        app.media_cache.is_none(),
        "バイト上限超えの画像は退避しない(画素数だけの判定では素通りしていた)"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// A full-screen mermaid fence keeps `preview_path` pointing at the Markdown file, so the media cache
/// must also key on which fence it is. Otherwise two tabs showing different diagrams of the *same*
/// document swap pictures on switch.
#[test]
fn media_cache_distinguishes_mermaid_fences_of_one_document() {
    let dir = std::env::temp_dir().join(unique_tmp("konoma_media_cache_fence"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let doc = dir.join("doc.md");
    std::fs::write(
        &doc,
        "# d\n\n```mermaid\nflowchart TD\nA-->B\n```\n\n```mermaid\nflowchart TD\nC-->D\n```\n",
    )
    .unwrap();

    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.tab.mode = Mode::Preview;
    app.tab.preview_path = Some(doc.clone());
    app.preview_media_mtime = crate::app::file_mtime(&doc);
    // With fence #1 shown full-screen, stash that diagram.
    app.tab.preview_kind = Some(PreviewKind::MermaidFence(1));
    let fence1 = std::sync::Arc::new(image::DynamicImage::ImageRgba8(image::RgbaImage::new(4, 4)));
    app.image_src = Some(fence1.clone());
    app.tab_new().unwrap(); // save_active → stash(fence_ord=1)

    // Return to a state where another tab has **fence #0 of the same document** open.
    app.tab.mode = Mode::Preview;
    app.tab.preview_path = Some(doc.clone());
    app.tab.preview_kind = Some(PreviewKind::MermaidFence(0));
    app.image_src = None;
    let restored = app.restore_media_cache(&doc, 1);
    assert!(
        !restored && app.image_src.is_none(),
        "別のフェンスの図を使い回さない(序数までキーに含める)"
    );
    // Reuse it when it's the same fence.
    app.tab.preview_kind = Some(PreviewKind::MermaidFence(1));
    assert!(app.restore_media_cache(&doc, 1), "同一フェンスは再利用");
    assert!(std::sync::Arc::ptr_eq(
        app.image_src.as_ref().unwrap(),
        &fence1
    ));
    std::fs::remove_dir_all(&dir).ok();
}

/// Closing a tab must release its cached image — otherwise the one slot keeps an image that no tab can
/// ever reuse (a silent resident buffer).
#[test]
fn closing_a_media_tab_releases_its_cached_image() {
    let dir = std::env::temp_dir().join(unique_tmp("konoma_media_cache_close"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let img = dir.join("pic.png");
    std::fs::write(&img, b"x").unwrap();

    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.tab.mode = Mode::Preview;
    app.tab.preview_path = Some(img.clone());
    app.tab.preview_kind = Some(PreviewKind::Image(img.clone()));
    app.preview_media_mtime = crate::app::file_mtime(&img);
    app.image_src = Some(std::sync::Arc::new(image::DynamicImage::ImageRgba8(
        image::RgbaImage::new(4, 4),
    )));

    app.tab_new().unwrap(); // gets stashed
    assert!(app.media_cache.is_some(), "前提: 退避されている");
    app.tab_cycle(1); // back to the image tab
    app.tab_close(); // close that tab
    assert!(
        app.media_cache.is_none(),
        "閉じたタブの画像は手放す(誰も再利用できない常駐を残さない)"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// `cp -p` / `rsync -a` / archive extraction **preserve the mtime**, so the cache must also compare the
/// file size; otherwise switching back keeps showing the previous picture.
#[test]
fn media_cache_misses_when_size_changes_under_the_same_mtime() {
    let dir = std::env::temp_dir().join(unique_tmp("konoma_media_cache_mtime"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let img = dir.join("pic.png");
    std::fs::write(&img, b"original").unwrap();

    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.tab.mode = Mode::Preview;
    app.tab.preview_path = Some(img.clone());
    app.tab.preview_kind = Some(PreviewKind::Image(img.clone()));
    app.preview_media_mtime = crate::app::file_mtime(&img);
    app.image_src = Some(std::sync::Arc::new(image::DynamicImage::ImageRgba8(
        image::RgbaImage::new(4, 4),
    )));
    app.tab_new().unwrap(); // evacuate

    // Change the content and size, but restore only the mtime (like `cp -p`).
    let stamp = std::fs::metadata(&img).unwrap().modified().unwrap();
    std::fs::write(&img, b"replaced-with-different-size").unwrap();
    let f = std::fs::OpenOptions::new().write(true).open(&img).unwrap();
    f.set_modified(stamp).unwrap();
    drop(f);
    assert_eq!(
        crate::app::file_mtime(&img),
        Some(stamp),
        "mtime は同一に戻った"
    );

    app.image_src = None;
    assert!(
        !app.restore_media_cache(&img, 1),
        "mtime が同じでもサイズが違えば使い回さない"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// Switching back to a tab that has the changed-only filter (`C`) on, from a tab in a **different**
/// repository, must not silently drop the filter. The status scan for the returning repo is still in
/// flight at that moment, so rebuilding the list from the (momentarily empty) statuses concluded "no
/// changes", turned the filter off and flashed a false "no changed files".
#[cfg(feature = "git")]
#[test]
fn changed_filter_survives_returning_from_another_repo() {
    let base = std::env::temp_dir().join(unique_tmp("konoma_changed_filter_two_repos"));
    let _ = std::fs::remove_dir_all(&base);
    let (a, b) = (base.join("repo_a"), base.join("repo_b"));
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    init_git_repo(&a);
    init_git_repo(&b);
    std::fs::write(a.join("changed.txt"), b"x").unwrap();
    std::fs::write(b.join("other.txt"), b"y").unwrap();
    let (a, b) = (a.canonicalize().unwrap(), b.canonicalize().unwrap());

    let mut app = App::new(a.clone(), Config::default()).unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    app.attach_status_loader(tx);
    app.toggle_changed_filter(); // `C` has a synchronous contract, so the list is up immediately
    assert!(app.tab.changed_filter, "前提: 変更ファイルのみ表示");

    // Switch to a tab in another repo and let its status settle there.
    app.tab_new().unwrap();
    app.tab.root = b.clone();
    app.tab.open_dir = b.clone();
    app.rebuild_tree().unwrap();
    app.refresh_git_if_needed();
    while let Ok(res) = rx.try_recv() {
        app.apply_statuses(res);
    }

    // Switch back to repo A's tab (A's status scan is still running at this point).
    app.tab_cycle(1);
    assert!(
        app.tab.changed_filter,
        "スキャン走行中でもフィルタは維持される(旧実装は解除+偽 flash)"
    );
    assert_ne!(
        app.flash.as_deref(),
        Some("no changed files"),
        "嘘の「変更なし」を出さない"
    );

    // Once the result arrives, the list is rebuilt.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while app.git_status_pending.is_some() && std::time::Instant::now() < deadline {
        if let Ok(res) = rx.recv_timeout(std::time::Duration::from_secs(30)) {
            app.apply_statuses(res);
        }
    }
    assert!(app.tab.changed_filter, "到着後もフィルタは生きている");
    assert!(
        app.tab
            .entries
            .iter()
            .any(|e| e.path.ends_with("changed.txt")),
        "到着した status で一覧が作り直される"
    );
    std::fs::remove_dir_all(&base).ok();
}

/// Drive the real preview pipeline over the whole task corpus and toggle **every** checkbox in every
/// document, asserting the write is byte-exact: one state character flips and nothing else in the file
/// moves. Counting parity (in `markdown::task_scan_parity_tests`) only proves the renderer and the
/// scanner agree on *how many* boxes there are; this proves the Nth box on screen edits the Nth box in
/// the source — the property that actually matters when a document mixes alerts, details and tables.
#[test]
fn md_task_toggle_is_byte_exact_across_the_corpus() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    let dir = std::env::temp_dir().join(unique_tmp("konoma_task_corpus_roundtrip"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let root = dir.canonicalize().unwrap();
    let f = root.join("doc.md");

    for (name, src) in crate::preview::markdown::task_corpus::cases() {
        // Count how many checkboxes appear on screen by opening the document once.
        std::fs::write(&f, src).unwrap();
        let mut app = App::new(root.clone(), Config::default()).unwrap();
        app.tab.selected = app
            .tab
            .entries
            .iter()
            .position(|e| e.path.ends_with("doc.md"))
            .unwrap();
        app.tree_activate().unwrap();
        let mut term = Terminal::new(TestBackend::new(100, 40)).unwrap();
        term.draw(|fr| crate::ui::render(fr, &mut app)).unwrap();
        let task_items: Vec<usize> = app
            .md_items
            .iter()
            .enumerate()
            .filter(|(_, it)| matches!(it.kind, MdItemKind::Task { .. }))
            .map(|(i, _)| i)
            .collect();

        for (nth, &item_idx) in task_items.iter().enumerate() {
            // Reopen from the original content each time (don't carry over the effect of a
            // previous toggle).
            std::fs::write(&f, src).unwrap();
            let mut app = App::new(root.clone(), Config::default()).unwrap();
            app.tab.selected = app
                .tab
                .entries
                .iter()
                .position(|e| e.path.ends_with("doc.md"))
                .unwrap();
            app.tree_activate().unwrap();
            let mut term = Terminal::new(TestBackend::new(100, 40)).unwrap();
            term.draw(|fr| crate::ui::render(fr, &mut app)).unwrap();
            app.tab.focused_item = Some(item_idx);
            app.flash = None;
            app.md_toggle_focused_task();

            let after = std::fs::read_to_string(&f).unwrap();
            assert!(
                app.flash.is_none(),
                "{name} #{nth}: トグルが拒否された(flash={:?})\n--- src ---\n{src}",
                app.flash
            );
            // Only one character changed, and its position is "the state character of the nth task".
            let diff: Vec<usize> = src
                .char_indices()
                .zip(after.char_indices())
                .filter(|((_, a), (_, b))| a != b)
                .map(|((i, _), _)| i)
                .collect();
            assert_eq!(
                src.chars().count(),
                after.chars().count(),
                "{name} #{nth}: 文字数が変わった(1文字置換のはず)\n--- after ---\n{after}"
            );
            assert_eq!(
                diff.len(),
                1,
                "{name} #{nth}: 変更が1文字でない(位置={diff:?})\n--- after ---\n{after}"
            );
            let locs = crate::preview::markdown::task_source_locs(src, &[' ', 'x'], &[]);
            let loc = &locs[nth];
            // Derive the expected position using **the same line-splitting as the product**
            // (`split('\n')`). `str::lines()` drops the CRLF `\r`, so counting with it is off
            // by one byte on CRLF documents (a pitfall on the test side).
            let line_start: usize = src.split('\n').take(loc.line).map(|l| l.len() + 1).sum();
            assert_eq!(
                diff[0],
                line_start + loc.state_off,
                "{name} #{nth}: 別の位置を書き換えた\n--- after ---\n{after}"
            );
            // A check that doesn't depend on the offset calculation: after writing back and
            // re-reading, **only that one** checkbox's state has changed and the rest are unchanged.
            let after_locs = crate::preview::markdown::task_source_locs(&after, &[' ', 'x'], &[]);
            assert_eq!(
                after_locs.len(),
                locs.len(),
                "{name} #{nth}: 書き戻しでチェックボックスの個数が変わった"
            );
            for (i, (b, a)) in locs.iter().zip(after_locs.iter()).enumerate() {
                if i == nth {
                    assert_ne!(
                        b.state, a.state,
                        "{name} #{nth}: 対象の状態が変わっていない"
                    );
                } else {
                    assert_eq!(
                        b.state, a.state,
                        "{name} #{nth}: 別のタスク #{i} を書き換えた"
                    );
                }
            }
            // Line structure (newline kind, line count) is unchanged.
            assert_eq!(
                src.matches("\r\n").count(),
                after.matches("\r\n").count(),
                "{name} #{nth}: CRLF が壊れた"
            );
            assert_eq!(
                src.ends_with('\n'),
                after.ends_with('\n'),
                "{name} #{nth}: 末尾改行が変わった"
            );
        }
    }
    std::fs::remove_dir_all(&dir).ok();
}

/// Root cause (b), the front-matter case: the renderer (`build_decorated`) strips the leading YAML front
/// matter before it ever reaches task detection, so a line inside it that merely *looks* like a task
/// (`  - [ ] draft`) is never drawn as a checkbox. The write-back scanner must agree — scan the same
/// (front-matter-stripped) body — or it counts one more "task" than is on screen and the safety check
/// cancels the toggle of the one real, visible checkbox. Kept out of `task_corpus` (see the comment
/// there): that shared corpus is also driven through a parallel, front-matter-**unaware** reference
/// computation in `md_task_toggle_is_byte_exact_across_the_corpus`, which this case would defeat.
#[test]
fn md_task_toggle_skips_pseudo_tasks_inside_front_matter() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    let dir = std::env::temp_dir().join(unique_tmp("konoma_md_task_front_matter"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let root = dir.canonicalize().unwrap();
    let f = root.join("doc.md");
    let src = "---\ntitle: t\ntags:\n  - [ ] draft\n---\n\n- [ ] real\n";
    std::fs::write(&f, src).unwrap();

    let mut app = App::new(root.clone(), Config::default()).unwrap();
    app.tab.selected = app
        .tab
        .entries
        .iter()
        .position(|e| e.path.ends_with("doc.md"))
        .unwrap();
    app.tree_activate().unwrap();
    let mut term = Terminal::new(TestBackend::new(100, 40)).unwrap();
    term.draw(|fr| crate::ui::render(fr, &mut app)).unwrap();

    let task_items: Vec<usize> = app
        .md_items
        .iter()
        .enumerate()
        .filter(|(_, it)| matches!(it.kind, MdItemKind::Task { .. }))
        .map(|(i, _)| i)
        .collect();
    assert_eq!(
        task_items.len(),
        1,
        "front matter 内の疑似タスクは描画されないはず(本文の1個だけ)"
    );

    app.tab.focused_item = Some(task_items[0]);
    app.flash = None;
    app.md_toggle_focused_task();
    assert!(
        app.flash.is_none(),
        "本文の唯一のチェックボックスのトグルが拒否された(flash={:?})",
        app.flash
    );

    let after = std::fs::read_to_string(&f).unwrap();
    assert_eq!(
        after, "---\ntitle: t\ntags:\n  - [ ] draft\n---\n\n- [x] real\n",
        "front matter 内の疑似タスクは無傷のまま、本文のチェックボックスだけが変わるはず"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// Root cause (b), the line-count-cap case: `preview::text::load` shows only the first `MAX_LINES` (5000)
/// lines of a Markdown file, so a document with real checkboxes both inside and beyond that cutoff
/// disagrees between the (unbounded) raw scanner and the (capped) render — and the safety check then
/// cancels **every** toggle in the document, including the very first checkbox, which is fully on
/// screen. This is the real-world failure the report cites (a 6,530-line `THIRD-PARTY-LICENSES.md`
/// showed 136 checkboxes but the naive scanner found 185).
#[test]
fn md_task_toggle_works_beyond_the_preview_line_cap() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    let dir = std::env::temp_dir().join(unique_tmp("konoma_md_task_beyond_cap"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let root = dir.canonicalize().unwrap();
    let f = root.join("big.md");

    let mut src = String::from("- [ ] first\n");
    for i in 0..6000 {
        src.push_str(&format!("filler line {i}\n"));
    }
    src.push_str("- [ ] last\n"); // beyond MAX_LINES=5000: culled from the render
    assert!(src.lines().count() > 5000, "前提: 5000行を超えていること");
    std::fs::write(&f, &src).unwrap();

    let mut app = App::new(root.clone(), Config::default()).unwrap();
    app.tab.selected = app
        .tab
        .entries
        .iter()
        .position(|e| e.path.ends_with("big.md"))
        .unwrap();
    app.tree_activate().unwrap();
    let mut term = Terminal::new(TestBackend::new(100, 40)).unwrap();
    term.draw(|fr| crate::ui::render(fr, &mut app)).unwrap();

    let task_items: Vec<usize> = app
        .md_items
        .iter()
        .enumerate()
        .filter(|(_, it)| matches!(it.kind, MdItemKind::Task { .. }))
        .map(|(i, _)| i)
        .collect();
    assert_eq!(
        task_items.len(),
        1,
        "5000行の上限で切り詰められ、可視のチェックボックスは先頭の1個だけのはず"
    );

    app.tab.focused_item = Some(task_items[0]);
    app.flash = None;
    app.md_toggle_focused_task();
    assert!(
        app.flash.is_none(),
        "画面に見えている先頭のチェックボックスのトグルが拒否された(flash={:?})",
        app.flash
    );

    let after = std::fs::read_to_string(&f).unwrap();
    assert!(
        after.starts_with("- [x] first\n"),
        "先頭のチェックボックスだけが状態変更されるはず: {:?}",
        &after[..after.len().min(20)]
    );
    assert!(
        after.ends_with("- [ ] last\n"),
        "切り詰め範囲より後ろの本物のチェックボックスは無傷のまま保存されるはず"
    );
    assert_eq!(
        src.len(),
        after.len(),
        "書込みでファイル長が変わってはいけない(切り詰め接頭辞でなく全文を保存している証拠)"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// The `y c` analog of the previous test: `focused_code_source` must scan the same capped prefix
/// the renderer used, or a document with fenced code blocks both inside and beyond the line cap
/// refuses to copy even the block that is fully on screen.
#[test]
fn md_code_block_copy_works_beyond_the_preview_line_cap() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    let dir = std::env::temp_dir().join(unique_tmp("konoma_md_code_beyond_cap"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let root = dir.canonicalize().unwrap();
    let f = root.join("big.md");

    let mut src = String::from("```\nvisible code\n```\n");
    for i in 0..6000 {
        src.push_str(&format!("filler line {i}\n"));
    }
    src.push_str("```\nhidden code\n```\n"); // beyond MAX_LINES=5000: culled from the render
    assert!(src.lines().count() > 5000, "前提: 5000行を超えていること");
    std::fs::write(&f, &src).unwrap();

    let mut app = App::new(root.clone(), Config::default()).unwrap();
    app.tab.selected = app
        .tab
        .entries
        .iter()
        .position(|e| e.path.ends_with("big.md"))
        .unwrap();
    app.tree_activate().unwrap();
    let mut term = Terminal::new(TestBackend::new(100, 40)).unwrap();
    term.draw(|fr| crate::ui::render(fr, &mut app)).unwrap();

    let code_items: Vec<usize> = app
        .md_items
        .iter()
        .enumerate()
        .filter(|(_, it)| matches!(it.kind, MdItemKind::CodeBlock))
        .map(|(i, _)| i)
        .collect();
    assert_eq!(
        code_items.len(),
        1,
        "5000行の上限で切り詰められ、可視のコードブロックは先頭の1個だけのはず"
    );

    app.tab.focused_item = Some(code_items[0]);
    let copied = app.focused_code_text();
    assert_eq!(
        copied.as_deref(),
        Some("visible code"),
        "画面に見えている先頭のコードブロックがコピーできるはず(copied={copied:?})"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// Moving the root out of a repository must drop that repository's ignore set. The "already computing"
/// guard compared `git_ignored_pending == wd`, and for a non-repo root **both are None**, so the guard
/// always fired and the clearing branch below it was never reached — the previous repo's ignored paths
/// stayed applied to an unrelated tree.
#[cfg(feature = "git")]
#[test]
fn leaving_a_repo_for_a_plain_directory_drops_the_ignore_set() {
    let base = std::env::temp_dir().join(unique_tmp("konoma_ignored_leave_repo"));
    let _ = std::fs::remove_dir_all(&base);
    let repo = base.join("repo");
    let plain = base.join("plain");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(&plain).unwrap();
    init_git_repo(&repo);
    std::fs::write(repo.join(".gitignore"), b"ignored.txt\n").unwrap();
    std::fs::write(repo.join("ignored.txt"), b"x").unwrap();
    std::fs::write(plain.join("a.txt"), b"y").unwrap();
    let repo = repo.canonicalize().unwrap();
    let plain = plain.canonicalize().unwrap();

    let mut app = App::new(repo.clone(), Config::default()).unwrap();
    app.refresh_git_if_needed();
    assert!(
        app.is_ignored(&repo.join("ignored.txt")),
        "前提: repo の無視セットが効いている"
    );

    // Move out of the repo (to a directory not under git control).
    app.tab.root = plain.clone();
    app.git_status_dirty = true;
    app.refresh_git_if_needed();

    assert!(
        app.git_ignored_for.is_none(),
        "repo を出たら無視セットのキャッシュキーも外れる"
    );
    assert!(
        !app.is_ignored(&repo.join("ignored.txt")),
        "前の repo の無視セットが残っていない"
    );
    std::fs::remove_dir_all(&base).ok();
}

/// Build an App previewing `name.md` (body) with wrap on, rendered once into a `w`x`h` TestBackend so
/// the wrap-aware `row_prefix` is built exactly as in the run loop. Returns the app at the preview.
#[cfg(test)]
fn wrapped_md_preview(dir: &Path, name: &str, body: &str, w: u16, h: u16) -> App {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    std::fs::write(dir.join(name), body).unwrap();
    let mut cfg = Config::default();
    cfg.ui.wrap = true;
    let mut app = App::new(dir.to_path_buf(), cfg).unwrap();
    app.tab.selected = app
        .tab
        .entries
        .iter()
        .position(|e| e.path.ends_with(name))
        .unwrap();
    app.tree_activate().unwrap();
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    term.draw(|fr| crate::ui::render(fr, &mut app)).unwrap();
    // Note: `term` is dropped but `app` is returned so the caller can keep drawing/accessing it.
    app
}

/// Assert the focused item's whole visual extent is inside the viewport after `md_focus_move`.
#[cfg(test)]
fn assert_focused_block_fully_visible(app: &App, msg: &str) {
    let f = app.tab.focused_item.expect("フォーカスがある");
    let line = app.md_item_line_for_test(f);
    let end = app.md_item_block_end_for_test(f);
    let (top, hh) = app.md_visual_span_for_test(line);
    let (bt, bh) = app.md_visual_span_for_test(end);
    let bottom = (bt + bh).max(top + hh);
    let vh = app.preview_viewport_for_test().max(1) as usize;
    let scroll = app.tab.preview_scroll as usize;
    // A block larger than the viewport is aligned to its top (the tail is unavoidably cut off);
    // otherwise it must be fully visible.
    if bottom - top >= vh {
        assert_eq!(scroll, top, "{msg}: 画面より大きいブロックは先頭合わせ");
    } else {
        assert!(
            top >= scroll,
            "{msg}: 先頭が上に見切れ (top={top} scroll={scroll})"
        );
        assert!(
            bottom <= scroll + vh,
            "{msg}: 末尾が下に見切れ (bottom={bottom} scroll={scroll} vh={vh})"
        );
    }
}

/// A long checkbox that **soft-wraps** must be fully scrolled into view when focused — the wrapped
/// tail included. This has worked via `md_visual_span` returning the wrap height, but nothing pinned
/// it; the block-scroll feature's own tests used short, non-wrapping lines.
#[test]
fn md_focus_reveals_a_wrapping_checkbox_in_full() {
    let dir = std::env::temp_dir().join(unique_tmp("konoma_wrap_task_reveal"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let dir = dir.canonicalize().unwrap();
    let mut body = String::from("# T\n\n");
    for i in 0..16 {
        body.push_str(&format!("filler {i}\n\n"));
    }
    // A long task that wraps across several lines at width 40, at the end.
    let long: String = (0..30).map(|i| format!("word{i} ")).collect();
    body.push_str(&format!("- [ ] {long}\n"));

    let mut app = wrapped_md_preview(&dir, "t.md", &body, 40, 18);
    // Move to the trailing checkbox.
    app.md_focus_move(-1);
    assert!(app.md_focused_task(), "チェックボックスにフォーカス");
    let f = app.tab.focused_item.unwrap();
    let (_, height) = app.md_visual_span_for_test(app.md_item_line_for_test(f));
    assert!(height > 1, "この幅ではタスクが折り返す (height={height})");
    assert_focused_block_fully_visible(&app, "折り返しチェックボックス");
    std::fs::remove_dir_all(&dir).ok();
}

/// A code block whose lines are long enough to **wrap** must also be fully revealed — the block-scroll
/// end is a logical line, and `md_visual_span` on it has to account for that line's own wrap.
#[test]
fn md_focus_reveals_a_wrapping_code_block_in_full() {
    let dir = std::env::temp_dir().join(unique_tmp("konoma_wrap_code_reveal"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let dir = dir.canonicalize().unwrap();
    let mut body = String::from("# T\n\n");
    for i in 0..10 {
        body.push_str(&format!("filler {i}\n\n"));
    }
    body.push_str("```text\n");
    // Make only the last line extremely long so it wraps (probes whether the block_end line's
    // wrap is accounted for).
    body.push_str("short first line\n");
    let long: String = (0..30).map(|i| format!("tok{i} ")).collect();
    body.push_str(&format!("{long}\n"));
    body.push_str("```\n");

    let mut app = wrapped_md_preview(&dir, "c.md", &body, 40, 20);
    app.md_focus_move(1); // only target = the code block
    let f = app.tab.focused_item.unwrap();
    let start = app.md_item_line_for_test(f);
    let end = app.md_item_block_end_for_test(f);
    // One long source line gets **pre-split** by the renderer into several `▎` logical lines
    // (line-splitting, not soft-wrapping — the 2026-07-07 gutter-break fix). So the 2 source body
    // lines should have expanded into more logical lines than that; the point is that all of them
    // get made visible.
    assert!(
        end - start + 1 > 3,
        "長い行が複数の視覚行に分割されている (start={start} end={end})"
    );
    assert_focused_block_fully_visible(&app, "折返しで膨らむコードブロック");
    std::fs::remove_dir_all(&dir).ok();
}

// =========================================================================================
// [external] — one on/off switch per external process konoma can launch
// =========================================================================================

/// `[external] git = false` in a real repo: `o` (Git view) refuses with a message distinct from
/// "not a repo" (`ExternalGitDisabled`, not `NotAGitRepo` — this really is a repo), and the tree
/// still renders without panicking (git status/branch/ignored all degrade to empty/None, same as a
/// `--no-default-features` build).
#[cfg(feature = "git")]
#[test]
fn external_git_disabled_flashes_distinct_message_and_tree_renders() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    let dir = unique_tmp("konoma_external_git_view_test");
    std::fs::create_dir_all(&dir).unwrap();
    init_git_repo(&dir);
    let sh = |args: &[&str]| {
        std::process::Command::new("git")
            .current_dir(&dir)
            .args(args)
            .output()
            .unwrap();
    };
    std::fs::write(dir.join("a.txt"), b"hi\n").unwrap();
    sh(&["add", "-A"]);
    sh(&["commit", "-qm", "init"]);
    std::fs::write(dir.join("a.txt"), b"changed\n").unwrap(); // would normally show an `M` marker

    let mut cfg = Config::default();
    cfg.external.git = false;
    let mut app = App::new(dir.clone(), cfg).unwrap();

    app.open_git_view();
    assert!(!app.is_git_view(), "git=false: the Git view does not open");
    assert_eq!(
        app.flash.as_deref(),
        Some(tr(app.lang, crate::i18n::Msg::ExternalGitDisabled)),
        "distinct from NotAGitRepo"
    );

    // The tree still renders (no color markers since status/branch are gated to empty/None, but no panic).
    let mut term = Terminal::new(TestBackend::new(60, 12)).unwrap();
    term.draw(|f| crate::ui::render(f, &mut app)).unwrap();

    // Sanity: with git enabled (default), `o` really does open on the same repo.
    let mut app2 = App::new(dir.clone(), Config::default()).unwrap();
    app2.open_git_view();
    assert!(
        app2.is_git_view(),
        "sanity: default config opens the Git view on a real repo"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// A machine without a `git` executable, with `[external] git = true` (the default): every read
/// degrades to exactly what a `--no-default-features` build returns, **even though the directory is a
/// real repo with real history**. Repo discovery goes through the embedded libgit2 and would still
/// succeed, so without the probe this state is half-broken (views open, every CLI-backed operation
/// silently does nothing).
#[cfg(feature = "git")]
#[test]
fn git_integration_is_off_when_the_binary_is_missing() {
    let dir = unique_tmp("konoma_git_binary_missing_reads");
    std::fs::create_dir_all(&dir).unwrap();
    init_git_repo(&dir);
    let sh = |args: &[&str]| {
        std::process::Command::new("git")
            .current_dir(&dir)
            .args(args)
            .output()
            .unwrap();
    };
    std::fs::write(dir.join("a.txt"), b"one\n").unwrap();
    sh(&["add", "-A"]);
    sh(&["commit", "-qm", "init"]);
    std::fs::write(dir.join("a.txt"), b"two\n").unwrap();

    // Baseline while the real probe runs: this really is a repo with a change to report.
    assert!(
        crate::git::git_binary_available(),
        "sanity: git is installed"
    );
    assert!(!crate::git::statuses(&dir).is_empty(), "sanity: statuses");
    assert!(crate::git::branch(&dir).is_some(), "sanity: branch");
    assert!(crate::git::workdir(&dir).is_some(), "sanity: workdir");

    // Simulate a machine without git: the config still says `true`, but the gate must be off.
    crate::git::set_git_binary_available_for_test(Some(false));
    assert!(
        !crate::git::external_git_enabled(),
        "config allows git, but no binary = the gate is off"
    );
    assert!(crate::git::statuses(&dir).is_empty(), "statuses");
    assert!(crate::git::branch(&dir).is_none(), "branch");
    assert!(crate::git::workdir(&dir).is_none(), "workdir");

    crate::git::set_git_binary_available_for_test(None);
    std::fs::remove_dir_all(&dir).ok();
}

/// A machine without a `git` executable: `o` refuses with `GitNotInstalled` — not `NotAGitRepo`
/// (this is a repo; blaming the directory would be wrong) and not `ExternalGitDisabled` (the config
/// allows git). The `[external] git = false` case is covered by the neighbouring test.
#[cfg(feature = "git")]
#[test]
fn open_git_view_reports_a_missing_git_binary_distinctly() {
    let dir = unique_tmp("konoma_git_binary_missing_view");
    std::fs::create_dir_all(&dir).unwrap();
    init_git_repo(&dir);
    let sh = |args: &[&str]| {
        std::process::Command::new("git")
            .current_dir(&dir)
            .args(args)
            .output()
            .unwrap();
    };
    std::fs::write(dir.join("a.txt"), b"hi\n").unwrap();
    sh(&["add", "-A"]);
    sh(&["commit", "-qm", "init"]);
    std::fs::write(dir.join("a.txt"), b"changed\n").unwrap();

    crate::git::set_git_binary_available_for_test(Some(false));
    let mut app = App::new(dir.clone(), Config::default()).unwrap(); // external.git = true (default)
    app.open_git_view();
    assert!(
        !app.is_git_view(),
        "no git binary: the Git view does not open"
    );
    assert_eq!(
        app.flash.as_deref(),
        Some(tr(app.lang, crate::i18n::Msg::GitNotInstalled)),
        "distinct from NotAGitRepo / ExternalGitDisabled"
    );

    // Unchanged when the binary IS available: the same repo opens the view as before.
    crate::git::set_git_binary_available_for_test(None);
    let mut app2 = App::new(dir.clone(), Config::default()).unwrap();
    app2.open_git_view();
    assert!(
        app2.is_git_view(),
        "sanity: with git installed the Git view opens on a real repo"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// A machine without a `git` executable still renders: the tree draws (no markers, no branch name)
/// and nothing panics — principle #3, degrade safely rather than show a half-broken screen.
#[cfg(feature = "git")]
#[test]
fn tree_renders_without_a_git_binary() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    let dir = unique_tmp("konoma_git_binary_missing_render");
    std::fs::create_dir_all(&dir).unwrap();
    init_git_repo(&dir);
    std::fs::write(dir.join("a.txt"), b"hi\n").unwrap(); // would normally show a `U` marker

    crate::git::set_git_binary_available_for_test(Some(false));
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    let mut term = Terminal::new(TestBackend::new(60, 12)).unwrap();
    term.draw(|f| crate::ui::render(f, &mut app)).unwrap();
    assert!(app.git_branch().is_none(), "no branch name without git");

    crate::git::set_git_binary_available_for_test(None);
    std::fs::remove_dir_all(&dir).ok();
}

/// `[external] git_tool = false`: `O` never queues a launch (the run loop's `take_launch_git_tool`
/// would otherwise suspend the TUI and exec the configured tool, e.g. lazygit).
#[test]
fn launch_git_tool_respects_external_flag() {
    let dir = unique_tmp("konoma_git_tool_disabled_test");
    std::fs::create_dir_all(&dir).unwrap();
    let mut cfg = Config::default();
    cfg.external.git_tool = false;
    let mut app = App::new(dir.clone(), cfg).unwrap();

    app.launch_git_tool();
    assert!(
        !app.take_launch_git_tool(),
        "git_tool=false: no launch request is queued"
    );
    assert_eq!(
        app.flash.as_deref(),
        Some(tr(app.lang, crate::i18n::Msg::ExternalGitToolDisabled))
    );

    // Sanity: enabled (default) does queue the request.
    let mut app2 = App::new(dir.clone(), Config::default()).unwrap();
    app2.launch_git_tool();
    assert!(
        app2.take_launch_git_tool(),
        "sanity: default queues the launch"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// `[external] open_links = false`: an external link is refused via flash *before* ever spawning
/// `open`/`xdg-open` — the test never actually launches a browser either way.
#[test]
fn open_link_target_respects_external_open_links_flag() {
    let dir = unique_tmp("konoma_open_links_disabled_test");
    std::fs::create_dir_all(&dir).unwrap();
    let mut cfg = Config::default();
    cfg.external.open_links = false;
    let mut app = App::new(dir.clone(), cfg).unwrap();

    app.open_link_target("https://example.com/page").unwrap();
    assert_eq!(
        app.flash.as_deref(),
        Some(tr(app.lang, crate::i18n::Msg::ExternalOpenLinksDisabled)),
        "opening is refused instead of spawning `open`/`xdg-open`"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// `[external] remote_images = false`: the Markdown-inline `curl` fetch is never kicked off — the
/// URL is marked failed immediately so the renderer degrades straight to the text placeholder
/// instead of showing `ImageSlot::Loading` forever.
#[test]
fn remote_image_fetch_skipped_and_marked_failed_when_disabled() {
    let dir = unique_tmp("konoma_remote_images_disabled_test");
    std::fs::create_dir_all(&dir).unwrap();
    let mut cfg = Config::default();
    cfg.external.remote_images = false;
    let mut app = App::new(dir.clone(), cfg).unwrap();

    let url = "https://example.invalid/does-not-matter.png";
    app.ensure_remote_md_fetch(url);
    assert!(
        app.md_remote_inflight.is_empty(),
        "no background curl download is kicked off"
    );
    assert!(
        app.md_remote_failed.contains(url),
        "marked failed immediately so the renderer shows the text placeholder, not a stuck Loading"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// Regression: `[external] remote_images = false` used to leave a Markdown-inline remote image
/// stuck showing "🖼 remote — loading…" forever. `ensure_remote_md_fetch`'s synchronous-failure
/// branch recorded the URL in `md_remote_failed`, but — unlike a real download completing via
/// `apply_remote_fetch` — nothing else would ever invalidate `md_cache` to redecorate the
/// already-built `ImageSlot::Loading` line, since disabling remote fetches means no download ever
/// runs to trigger that invalidation. Fixed by having `ensure_remote_md_fetch` report the *first*
/// time it records a failure (`HashSet::insert`'s return value) so `ensure_md_cache` resyncs its
/// already-built `decorated` in the very same pass — the same synchronous-completion convention
/// `ensure_mermaid_fence_render`/`ensure_math_render` already use.
#[test]
fn remote_image_disabled_does_not_stick_on_loading_forever() {
    let dir = unique_tmp("konoma_remote_images_disabled_no_hang_test");
    std::fs::create_dir_all(&dir).unwrap();
    let md = dir.join("doc.md");
    std::fs::write(
        &md,
        "# title\n\n![remote](https://example.invalid/pic.png)\n\ntail\n",
    )
    .unwrap();
    let mut cfg = Config::default();
    cfg.external.remote_images = false;
    let mut app = App::new(dir.clone(), cfg).unwrap();
    app.picker = Some(test_picker());
    app.enter_preview(&md);

    let joined = |app: &App| -> String {
        app.md_cache
            .as_ref()
            .unwrap()
            .lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect()
    };

    // First decoration pass: `decorated` is initially built with an `ImageSlot::Loading` line
    // (before `md_remote_failed` contains the URL), but `ensure_remote_md_fetch`'s synchronous
    // failure signal resyncs within this *same* pass, so the stored cache already reflects the
    // degrade — it must never observably show "loading…", even for a single frame.
    app.ensure_md_cache(80);
    let text = joined(&app);
    assert!(
        !text.contains("loading…"),
        "must degrade to the text placeholder within the first decoration pass, not hang: {text:?}"
    );
    assert!(
        text.contains("https://example.invalid/pic.png"),
        "degrades to the text fallback showing the URL: {text:?}"
    );

    // Second pass (next frame): still not stuck, and idempotent (insert() only resyncs once).
    app.ensure_md_cache(80);
    let text = joined(&app);
    assert!(
        !text.contains("loading…"),
        "still not stuck after a second frame: {text:?}"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// `[external] pdf = false` means "never spawn pdftocairo/pdftoppm/qlmanage/sips" — it does **not**
/// disable PDF preview, because `hayro` (the primary renderer) and `page_count` (`hayro-syntax`) are
/// both pure Rust and never touch an external process. `tab.pdf_pages` must resolve identically
/// (`Some(3)`, the bundled sample's known page count) whether the flag is on or off — unlike before
/// `hayro` existed, when this flag (and `arbitrary_page_renderer_available`) gated whether page
/// navigation was exposed at all.
#[test]
fn pdf_page_count_and_native_render_work_even_with_external_pdf_disabled() {
    let p = Path::new("samples/sample.pdf");
    if !p.exists() {
        return; // samples excluded from the published crate
    }
    let dir = unique_tmp("konoma_pdf_disabled_test");
    std::fs::create_dir_all(&dir).unwrap();

    let mut cfg = Config::default();
    cfg.external.pdf = false;
    let mut app = App::new(dir.clone(), cfg).unwrap();
    app.enter_preview(p);
    assert_eq!(
        app.tab.pdf_pages,
        Some(3),
        "pdf=false: page_count (hayro-syntax) still runs — it never touched an external process"
    );

    // Same with the flag enabled (default) — external.pdf must not change what pdf_pages resolves to.
    let mut app2 = App::new(dir.clone(), Config::default()).unwrap();
    app2.enter_preview(p);
    assert_eq!(
        app2.tab.pdf_pages,
        Some(3),
        "external.pdf doesn't change the resolved page count"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// `[external] video = false`: the thumbnail extraction job is never spawned, even for a real video
/// with ffmpeg installed — `image_src` stays `None` (falls back to the "unavailable" hint like a
/// missing tool would), proving the gate itself short-circuits rather than a missing tool doing it.
#[test]
fn video_thumbnail_skipped_when_external_video_disabled() {
    let p = Path::new("samples/sample.mp4");
    if !p.exists() {
        return; // samples excluded from the published crate
    }
    let dir = unique_tmp("konoma_video_disabled_test");
    std::fs::create_dir_all(&dir).unwrap();

    let mut cfg = Config::default();
    cfg.external.video = false;
    let mut app = App::new(dir.clone(), cfg).unwrap();
    app.picker = Some(test_picker());
    app.enter_preview(p);
    assert!(
        app.image_src.is_none(),
        "video=false: the thumbnail job must not run"
    );
    assert!(!app.is_media_loading(), "no job left pending either");

    // Sanity: with video enabled (default) and ffmpeg/ffmpegthumbnailer installed, a thumbnail IS produced.
    let mut app2 = App::new(dir.clone(), Config::default()).unwrap();
    app2.picker = Some(test_picker());
    app2.enter_preview(p);
    if app2.image_src.is_none() {
        eprintln!("skip sanity: no video-thumbnail tool available (ffmpeg/ffmpegthumbnailer)");
    }

    std::fs::remove_dir_all(&dir).ok();
}
