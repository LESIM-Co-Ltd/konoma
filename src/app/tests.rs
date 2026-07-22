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

    // 相対: 起動ディレクトリ名を先頭に出す。
    app.path_style = PathStyle::Relative;
    assert_eq!(
        app.format_path(&file),
        "konoma_app_test_open/src/main.rs".to_string()
    );

    // HOME 相対: HOME 配下のパスは ~/... になる。
    if let Some(home) = std::env::var_os("HOME") {
        let p = PathBuf::from(home).join("work").join("konoma");
        app.path_style = PathStyle::Home;
        assert_eq!(app.format_path(&p), "~/work/konoma".to_string());
    }

    std::fs::remove_dir_all(&open).ok();
}

#[test]
fn format_path_relative_uses_dotdot_outside_open_dir() {
    // open_dir = .../work/A。配下は起動dir名を先頭に(A/x.rs)、外(兄弟/上位)は `..` 込みで表示。
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
    // カーソルは a.txt(ファイル)。作成先はその親=dir。
    assert_eq!(app.entries[app.selected].path, dir.join("a.txt"));

    // --- 作成(ファイル): 入力ダイアログ → 確定でファイルが出来て選択される。
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
        app.entries[app.selected].path,
        dir.join("new.txt"),
        "作成物が選択される"
    );

    // --- 作成(フォルダ): 末尾 / でディレクトリ。
    app.start_create();
    for c in "sub/".chars() {
        app.dialog_input_push(c);
    }
    app.dialog_submit().unwrap();
    assert!(dir.join("sub").is_dir(), "末尾 / でフォルダ作成");

    // --- リネーム: 現在名プリフィル → 変更して反映。
    let ai = app
        .entries
        .iter()
        .position(|e| e.path == dir.join("a.txt"))
        .unwrap();
    app.selected = ai;
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

    // --- 削除は確認ゲート: n でキャンセル → ファイルは残る(ゴミ箱を汚さない)。
    let ri = app
        .entries
        .iter()
        .position(|e| e.path == dir.join("renamed.txt"))
        .unwrap();
    app.selected = ri;
    app.start_delete();
    assert!(
        app.is_dialog() && app.dialog_is_confirm(),
        "削除は y/n 確認"
    );
    app.dialog_confirm(false).unwrap(); // n=取消
    assert!(!app.is_dialog(), "確認後ダイアログは閉じる");
    assert!(
        dir.join("renamed.txt").is_file(),
        "キャンセルでファイルは残る"
    );

    // --- 既存名への作成は失敗(上書きしない)。ダイアログは閉じ、失敗を通知。
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

    // "ac" → ← で中間へ → 'b' 挿入 → "abc"。
    app.start_create();
    app.dialog_input_push('a');
    app.dialog_input_push('c');
    app.dialog_cursor_left();
    app.dialog_input_push('b');
    assert_eq!(buf(&app), "abc", "カーソル位置に中間挿入");
    // Backspace は直前('b')を削除 → "ac"。
    app.dialog_input_backspace();
    assert_eq!(buf(&app), "ac");
    // Home → Delete はカーソル位置('a')を削除 → "c"。
    app.dialog_cursor_home();
    app.dialog_input_delete();
    assert_eq!(buf(&app), "c");
    app.dialog_cancel();

    // マルチバイト(日本語): "あ" の前に "x" を挿入してもバイト境界を壊さない。
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

    // V: カーソル(a)を1件トグルして下へ。a 選択・cursor=b。
    assert!(!app.has_selection());
    app.selected = 0;
    app.toggle_select();
    assert!(app.is_selected(&dir.join("a.txt")));
    assert_eq!(app.selected, 1, "V は下へ進む");
    // 歯抜け: b を飛ばして c を選択(jk 相当でカーソルを c へ)。
    app.selected = 2;
    app.toggle_select();
    assert_eq!(app.marked_count(), 2);
    assert!(app.is_selected(&dir.join("c.txt")) && !app.is_selected(&dir.join("b.txt")));
    // 同じ項目をもう一度トグルで解除。
    app.selected = 0;
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
    assert_eq!(app.entries.len(), 4);

    // ビジュアル: a で開始 → c まで下げると a..c が範囲(ライブ)。
    app.selected = 0;
    app.enter_visual();
    assert!(app.is_visual());
    app.tree_next(); // b
    app.tree_next(); // c
    assert!(app.is_in_visual_range(0) && app.is_in_visual_range(2));
    assert!(!app.is_in_visual_range(3), "d は範囲外");
    assert_eq!(app.marked_count(), 3, "範囲(a..c)で3件");
    // 確定(v) → 選択集合へ取り込みビジュアルを抜ける。
    app.exit_visual_commit();
    assert!(!app.is_visual());
    assert_eq!(app.marked_count(), 3);
    assert!(app.is_selected(&dir.join("a.txt")) && app.is_selected(&dir.join("c.txt")));

    // 選択3件を一括完全削除 → d だけ残る・選択クリア。
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
    // sub を展開して inner を表示(同階層判定の検証)。
    let sub_idx = app
        .entries
        .iter()
        .position(|e| e.path == dir.join("sub"))
        .unwrap();
    app.selected = sub_idx;
    app.tree_activate().unwrap(); // 展開
    app.rebuild_tree().unwrap();

    // トップ階層(a.txt)で a=同階層全選択 → a.txt/b.txt/sub は入るが inner は入らない。
    let a_idx = app
        .entries
        .iter()
        .position(|e| e.path == dir.join("a.txt"))
        .unwrap();
    app.selected = a_idx;
    app.enter_visual();
    app.visual_select_scope(false); // a=同階層
    assert!(app.is_selected(&dir.join("a.txt")));
    assert!(app.is_selected(&dir.join("b.txt")));
    assert!(app.is_selected(&dir.join("sub")));
    assert!(
        !app.is_selected(&dir.join("sub").join("inner.txt")),
        "入れ子(別階層)は a では入らない"
    );
    assert!(!app.is_visual(), "スコープ選択後はビジュアルを抜ける");

    // A=表示全部なら inner も入る。
    app.clear_selection();
    app.selected = a_idx;
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
    // name 昇順ソートで apple.md, mango.rs, zebra.txt の順になる。
    std::fs::write(dir.join("zebra.txt"), b"z").unwrap();
    std::fs::write(dir.join("apple.md"), b"a").unwrap();
    std::fs::write(dir.join("mango.rs"), b"m").unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.rebuild_tree().unwrap();
    app.visual_select_scope(true);
    assert_eq!(app.marked_count(), 3);

    // 一括リネーム: "img_{n}" → プレビューへ。
    app.start_batch_rename();
    assert!(app.is_dialog() && !app.dialog_is_confirm() && !app.dialog_is_preview());
    for c in "img_{n}".chars() {
        app.dialog_input_push(c);
    }
    app.dialog_submit().unwrap();
    assert!(app.dialog_is_preview(), "確定でプレビューへ遷移");
    // 連番は**ソート順**(apple,mango,zebra)、拡張子は自動保持。
    let (_, pairs, _) = app.dialog_preview_view().unwrap();
    assert_eq!(pairs[0], "apple.md  →  img_1.md");
    assert_eq!(pairs[1], "mango.rs  →  img_2.rs");
    assert_eq!(pairs[2], "zebra.txt  →  img_3.txt");

    // 適用。
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

    // {n} 無しのテンプレ → 2件とも "dup.txt" に衝突。
    app.start_batch_rename();
    for c in "dup".chars() {
        app.dialog_input_push(c);
    }
    app.dialog_submit().unwrap();
    // プレビューに進まず、入力ダイアログのまま(テンプレ保持)・失敗を通知。
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
    // ファイルは変わっていない。
    assert!(dir.join("a.txt").exists() && dir.join("b.txt").exists());

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn parse_dropped_paths_unescapes_splits_and_filters() {
    let tmp = std::env::temp_dir().join("konoma_parse_drop_test");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let a = tmp.join("a b.txt"); // 空白入りの名前
    std::fs::write(&a, b"x").unwrap();
    let c = tmp.join("c.txt");
    std::fs::write(&c, b"y").unwrap();
    // ターミナル流儀: 空白入り名は `\ ` でエスケープ・複数は未エスケープ空白区切り。
    let text = format!(
        "{} {}",
        a.to_string_lossy().replace(' ', "\\ "),
        c.to_string_lossy()
    );
    let got = parse_dropped_paths(&text);
    assert_eq!(got.len(), 2, "実在2パスを解く: {got:?}");
    assert!(got.contains(&a) && got.contains(&c));
    // 実在しないテキストは弾く(=ただのペーストを安全に無視)。
    assert!(parse_dropped_paths("just some pasted text, not a path").is_empty());
    std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn drop_paste_opens_dialog_then_copy_and_move() {
    let dir = std::env::temp_dir().join("konoma_drop_flow_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    // ドロップ元(別ディレクトリ)に空白入り名のファイルを2つ。
    let ext = std::env::temp_dir().join("konoma_drop_src");
    let _ = std::fs::remove_dir_all(&ext);
    std::fs::create_dir_all(&ext).unwrap();
    let src1 = ext.join("drop one.txt");
    let src2 = ext.join("drop two.txt");
    std::fs::write(&src1, b"a").unwrap();
    std::fs::write(&src2, b"b").unwrap();

    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.rebuild_tree().unwrap();
    // 唯一のエントリ "sub"(dir) にカーソル → 落とし先 = sub。
    assert!(app.entries[app.selected].is_dir, "カーソルが sub(dir)");

    // --- コピー: 1ファイルをドロップ → c で sub へコピー(元は残る) ---
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

    // --- 移動: もう1ファイルをドロップ → m で sub へ移動(元は消える) ---
    app.handle_paste(src2.to_string_lossy().replace(' ', "\\ "));
    assert!(app.confirm_is_drop());
    app.drop_apply(true).unwrap();
    assert!(dir.join("sub").join("drop two.txt").is_file(), "sub へ移動");
    assert!(!src2.exists(), "移動は元を消す");

    // 実在しないテキストのペーストはダイアログを開かない。
    app.handle_paste("not a real path".to_string());
    assert!(!app.is_dialog(), "テキストペーストでは開かない");

    std::fs::remove_dir_all(&dir).ok();
    std::fs::remove_dir_all(&ext).ok();
}

#[test]
fn paste_into_filter_inserts_text_not_drop() {
    // 絞り込み入力中のペーストは「ドロップ」でなく**文字挿入**にする(回帰防止)。
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
    // `:` は文字入力なし。h/l で移動した現在のツリールートを表示の基準(open_dir)へ再アンカーする。
    let base = std::env::temp_dir().join("konoma_reanchor_test");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("sub")).unwrap();
    std::fs::write(base.join("sub").join("f.txt"), b"x").unwrap();
    let base_c = base.canonicalize().unwrap();
    let mut app = App::new(base_c.clone(), Config::default()).unwrap();
    app.rebuild_tree().unwrap();
    // 起動直後は open_dir == root == base。
    assert_eq!(app.open_dir, base_c);
    // l で sub へ降りる(root だけ sub に・open_dir は base のまま)。
    app.tree_descend().unwrap();
    assert_eq!(app.root, base_c.join("sub"), "root が sub へ");
    assert_eq!(app.open_dir, base_c, "open_dir はまだ base(launch)");
    // sub 配下のファイルは base 基準なので "sub/..." と表示される。
    let f = base_c.join("sub").join("f.txt");
    assert!(
        app.format_path(&f).contains("sub"),
        "再アンカー前は sub/ 込み表示: {}",
        app.format_path(&f)
    );

    // `:` = 現在地(sub)を固定ルートに再アンカー。文字入力なし・ダイアログも出ない。
    app.reanchor_root();
    assert!(!app.is_dialog(), "ダイアログは出ない(入力なし)");
    assert_eq!(
        app.open_dir,
        base_c.join("sub"),
        "open_dir が sub へ再アンカー"
    );
    assert_eq!(app.root, base_c.join("sub"), "root は変わらない");
    // 再アンカー後は sub 基準なので f.txt は "sub/f.txt"(=sub をルートとした表示)。
    assert_eq!(app.format_path(&f), "sub/f.txt");

    // 既にルート上(open_dir==root)で再度 `:` → 何もしない(flash のみ)。
    app.reanchor_root();
    assert_eq!(app.open_dir, base_c.join("sub"), "変化なし");
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

    // is_ignored: ignored.log / node_modules(と配下) は true、tracked / .gitignore は false。
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

    // 描画: ignored.log 行は DIM セルを含む / tracked.txt 行は DIM 無し(選択行=node_modules で両者は非選択)。
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
    // 巡回: 縦→横→Auto→縦。
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

    // 機能1: ツリーで変更ファイル a.txt にカーソル → `=` で直接 diff プレビュー。
    let ai = app
        .entries
        .iter()
        .position(|e| e.path == canon.join("a.txt"))
        .expect("a.txt がツリーに無い");
    app.selected = ai;
    app.tree_open_git_diff();
    assert!(
        app.is_git_diff_preview(),
        "ツリーから diff プレビューが開く"
    );
    assert!(!app.git_diff_lines().is_empty(), "diff 行がある");
    app.close_git_diff();

    // 機能2: git ビューで `D` = 作業ツリー全変更の diff(詳細)。複数ファイル含む。
    app.open_git_view();
    app.open_worktree_detail();
    assert!(app.is_git_detail(), "全変更 diff(詳細)が開く");
    assert_eq!(
        app.git_detail_title(),
        Some("Uncommitted changes"),
        "タイトル上書き"
    );
    // ファイル境界ヘッダ = Context かつ 行番号が両方 None。
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

    // 機能3+4: `s` で 縦→横→Auto→縦 を巡回(詳細でも効く)。
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
    // とても長い1行に変更(行頭 START / 行末 END)。
    let long = format!("START{}END\n", "x".repeat(200));
    std::fs::write(dir.join("a.txt"), long.as_bytes()).unwrap();

    let canon = dir.canonicalize().unwrap();
    let mut app = App::new(canon.clone(), Config::default()).unwrap();
    app.rebuild_tree().unwrap();
    app.refresh_git_if_needed();
    let ai = app
        .entries
        .iter()
        .position(|e| e.path == canon.join("a.txt"))
        .unwrap();
    app.selected = ai;
    app.tree_open_git_diff();
    assert!(app.is_git_diff_preview());

    let dump = |app: &mut App| -> String {
        let mut term = Terminal::new(TestBackend::new(40, 10)).unwrap();
        term.draw(|f| crate::ui::render(f, app)).unwrap();
        let buf = term.backend().buffer();
        buf.content().iter().map(|c| c.symbol()).collect()
    };
    // 行頭(hscroll=0): START が見え、遠い END は画面外。
    let s0 = dump(&mut app);
    assert!(s0.contains("START"), "行頭に START");
    assert!(!s0.contains("END"), "行頭で END は画面外");
    // $ 相当で行末へ: END が見える。
    app.preview_hscroll_end();
    let se = dump(&mut app);
    assert!(se.contains("END"), "$ で行末(END)が見える");
    // 0 相当で行頭へ戻る。
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
        |app: &App, p: &std::path::Path| app.entries.iter().position(|e| e.path == p).unwrap();

    // a.txt をコピー → クリップボードに積まれ、選択はクリア。
    app.selected = idx(&app, &dir.join("a.txt"));
    app.toggle_select();
    app.copy_selection();
    assert!(app.clipboard_label().is_some(), "コピーで積まれる");
    assert!(!app.has_selection(), "コピーで選択クリア");
    // dst にカーソルを置いてペースト → dst/a.txt 出来て元も残る(複製)。
    app.selected = idx(&app, &dir.join("dst"));
    app.paste().unwrap();
    assert!(dir.join("dst").join("a.txt").is_file(), "コピー先に出来る");
    assert!(dir.join("a.txt").is_file(), "コピー元は残る");
    assert!(app.clipboard_label().is_some(), "コピーは貼っても消えない");

    // b.txt をカット → dst へペースト → 移動してクリップボード消費。
    app.selected = idx(&app, &dir.join("b.txt"));
    app.toggle_select();
    app.cut_selection();
    app.selected = idx(&app, &dir.join("dst"));
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
        .entries
        .iter()
        .position(|e| e.path == dir.join("gone.txt"))
        .unwrap();
    app.selected = i;

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
    // 既知の件数に差し替え (100件)。
    app.entries = (0..100)
        .map(|i| Entry {
            path: dir.join(format!("f{i}")),
            is_dir: false,
            depth: 0,
            expanded: false,
        })
        .collect();
    app.tree_viewport = 20;

    app.selected = 0;
    app.tree_page(1); // +19 (1行重ね)
    assert_eq!(app.selected, 19);
    app.tree_half_page(1); // +10
    assert_eq!(app.selected, 29);

    // 末尾でクランプ。
    app.tree_last();
    assert_eq!(app.selected, 99);
    app.tree_page(1);
    assert_eq!(app.selected, 99);

    // 先頭でクランプ。
    app.tree_first();
    app.tree_page(-1);
    assert_eq!(app.selected, 0);

    std::fs::remove_dir_all(&dir).ok();
}

// --- 整数オーバーフロー回帰 (#7/#8): カーソルが先頭以外でも `g`/`G`/Home/End が渡す
//     i32::MIN/MAX で素の `as i32 + delta` がパニックしていた。共有 `clamp_cursor` で一掃。

#[test]
fn clamp_cursor_no_overflow_at_extremes() {
    // カーソルを 0 以外に置いても i64 経由でオーバーフローしないこと。
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
    app.entries = (0..50)
        .map(|i| Entry {
            path: dir.join(format!("f{i}")),
            is_dir: false,
            depth: 0,
            expanded: false,
        })
        .collect();
    // 先頭以外に置いてから極値: 以前はここで i32 加算がパニックした。
    app.selected = 10;
    app.tree_move(i32::MAX);
    assert_eq!(app.selected, 49, "末尾へ");
    app.tree_move(i32::MIN);
    assert_eq!(app.selected, 0, "先頭へ");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn git_branch_move_extremes_no_panic() {
    let dir = std::env::temp_dir().join("konoma_branch_move_extremes_test");
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.git_branches = Some(
        (0..6)
            .map(|i| crate::git::BranchInfo {
                name: format!("br{i}"),
                is_current: i == 0,
            })
            .collect(),
    );
    app.git_branch_sel = 2; // 先頭以外
    app.git_branch_move(i32::MAX);
    assert_eq!(app.git_branch_sel, 5, "末尾へ");
    app.git_branch_move(i32::MIN);
    assert_eq!(app.git_branch_sel, 0, "先頭へ");
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
    // コミット行(0,2,4) とコネクタ行(1,3) が混在: カーソルはコミット行のみを対象。
    app.git_graph = Some(vec![
        commit("a"),
        connector(),
        commit("b"),
        connector(),
        commit("c"),
    ]);
    app.git_graph_sel = 2; // 真ん中のコミット行(commits index=1)
    app.git_graph_move(i32::MAX);
    assert_eq!(app.git_graph_sel, 4, "末尾コミット行へ");
    app.git_graph_move(i32::MIN);
    assert_eq!(app.git_graph_sel, 0, "先頭コミット行へ");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn git_log_move_extremes_no_panic() {
    let dir = std::env::temp_dir().join("konoma_log_move_extremes_test");
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.git_log = Some(
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
    app.git_log_sel = 1; // 先頭以外
    app.git_log_move(i32::MAX);
    assert_eq!(app.git_log_sel, 4, "末尾へ");
    app.git_log_move(i32::MIN);
    assert_eq!(app.git_log_sel, 0, "先頭へ");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn git_view_move_extremes_no_panic() {
    let dir = std::env::temp_dir().join("konoma_view_move_extremes_test");
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.git_view_entries = (0..4)
        .map(|i| crate::git::ChangeEntry {
            path: dir.join(format!("f{i}")),
            status: crate::git::FileStatus::Modified,
            staged: false,
        })
        .collect();
    app.git_view_sel = 1; // 先頭以外
                          // git_view は ±1 しか渡らないが、共通ヘルパへ寄せたので極値でも安全であること。
    app.git_view_move(i32::MAX);
    assert_eq!(app.git_view_sel, 3, "末尾へ");
    app.git_view_move(i32::MIN);
    assert_eq!(app.git_view_sel, 0, "先頭へ");
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
    // 小さい画像: 拡大せず native(40x15) のまま中央へ (allow_upscale=false)。
    let r = centered_rect((40, 15), inner, false);
    assert_eq!((r.width, r.height), (40, 15));
    assert_eq!((r.x, r.y), (20, 4));

    // 大きい画像: アスペクト比を保って縮小し中央へ (200x100 → scale 0.24)。
    let r = centered_rect((200, 100), inner, false);
    assert_eq!((r.width, r.height), (48, 24));
    assert_eq!((r.x, r.y), (16, 0));

    // ズーム時(allow_upscale=true): 小さい画像も領域いっぱいまで拡大し中央へ。
    let r = centered_rect((40, 15), inner, true);
    // scale = min(80/40, 24/15)=min(2.0,1.6)=1.6 → 64x24, x=(80-64)/2=8。
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
    app.preview_kind = Some(PreviewKind::Image(PathBuf::from("x.png")));
    app.picker = Some(ratatui_image::picker::Picker::halfblocks());
    // tx は drop されないようリーク(テスト終了まで生存)。
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
    // halfblocks picker(端末不要)に kitty を強制設定して、kitty 転送列の生成を検証する。
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
        return; // samples 除外環境ではスキップ
    }
    let mut app = app_with_kitty();
    let (tx, rx) = std::sync::mpsc::channel();
    app.attach_media_loader(tx);
    let kind = PreviewKind::Svg(p.to_path_buf());
    app.preview_kind = Some(kind.clone());

    app.start_media_load(&kind, p);
    // 開始直後: 読み込み中で、まだ画像は載っていない(UI を塞いでいない)。
    assert!(app.is_media_loading(), "開始直後は loading");
    assert!(!app.is_image_preview(), "まだ読み込めていない");

    // 別スレッドの結果を待って反映 → 画像が載り loading 解除。
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

    let mut app = app_with_kitty(); // picker あり
    let (tx, rx) = std::sync::mpsc::channel();
    Box::leak(Box::new(rx)); // 結果は drain しない → loading のまま
    app.attach_media_loader(tx);
    let path = PathBuf::from("/does-not-exist.svg");
    let kind = PreviewKind::Svg(path.clone());
    app.preview_kind = Some(kind.clone());
    app.preview_path = Some(path.clone());
    app.mode = Mode::Preview;
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
    // 既定 Config は en なので文言は "loading…"(jp なら "読み込み中…")。
    assert!(s.contains("loading"), "ローディング文言が出る: {s:?}");
    let spinners = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
    assert!(
        spinners.iter().any(|g| s.contains(g)),
        "共通スピナーのコマが出る: {s:?}"
    );
    // 生 XML へのフォールバックではないこと(loading 中は XML を出さない)。
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
    app.preview_kind = Some(PreviewKind::Pdf(PathBuf::from("/x/doc.pdf")));
    app.mode = Mode::Preview;

    // 総数不明: 移動不可・インジケータ無し。
    app.pdf_pages = None;
    app.pdf_page = 1;
    assert!(!app.pdf_can_navigate());
    app.pdf_next_page();
    assert_eq!(app.pdf_page, 1, "総数不明では動かない");
    assert_eq!(app.pdf_page_indicator(), None);

    // 1ページ: 移動不可。
    app.pdf_pages = Some(1);
    assert!(!app.pdf_can_navigate(), "単ページは移動不可");
    app.pdf_next_page();
    assert_eq!(app.pdf_page, 1);

    // 3ページ: 前後移動と端クランプ・インジケータ。
    app.pdf_pages = Some(3);
    app.pdf_page = 1;
    assert!(app.pdf_can_navigate());
    assert_eq!(app.pdf_page_indicator(), Some((1, 3)));
    app.pdf_next_page();
    assert_eq!(app.pdf_page, 2);
    app.pdf_next_page();
    assert_eq!(app.pdf_page, 3);
    app.pdf_next_page();
    assert_eq!(app.pdf_page, 3, "末尾でクランプ");
    assert_eq!(app.pdf_page_indicator(), Some((3, 3)));
    app.pdf_prev_page();
    assert_eq!(app.pdf_page, 2);
    app.pdf_prev_page();
    app.pdf_prev_page();
    assert_eq!(app.pdf_page, 1, "先頭でクランプ");

    // ページ移動でズームは fit(1.0)へ戻る。
    app.image_zoom = 4.0;
    app.pdf_next_page();
    assert_eq!(app.pdf_page, 2);
    assert_eq!(app.image_zoom, 1.0, "ページ移動で fit に戻る");
}

/// PDF multi-page end-to-end: navigating to page 2 kicks off an off-thread re-rasterization that lands a
/// new image. Skipped without the sample or without poppler (page_count None → single-page, no navigation).
#[test]
fn pdf_next_page_renders_off_thread() {
    let p = Path::new("samples/sample.pdf");
    if !p.exists() {
        return; // samples 除外環境ではスキップ
    }
    let Some(pages) = crate::preview::pdf::page_count(p) else {
        return; // poppler 無し: ページ数不明=単ページ扱い
    };
    if pages < 2 {
        return;
    }
    let mut app = app_with_kitty();
    let (tx, rx) = std::sync::mpsc::channel();
    app.attach_media_loader(tx);
    let kind = PreviewKind::Pdf(p.to_path_buf());
    app.preview_kind = Some(kind.clone());
    app.preview_path = Some(p.to_path_buf());
    app.mode = Mode::Preview;
    app.pdf_pages = Some(pages);
    app.pdf_page = 1;

    // 1ページ目を読み込んで反映。
    app.start_media_load(&kind, p);
    let r1 = rx
        .recv_timeout(std::time::Duration::from_secs(15))
        .expect("page1 worker should return");
    assert!(app.apply_media(r1));
    assert!(app.is_image_preview(), "page1 が image_src に載る");

    // 2ページ目へ: 別スレッドで再ラスタライズ開始 → 反映。
    app.pdf_next_page();
    assert_eq!(app.pdf_page, 2);
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
    app.preview_kind = Some(PreviewKind::Svg(PathBuf::from("x.svg")));
    // 現在の世代と一致しない結果を作る(古い世代)。
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
    app.preview_kind = Some(PreviewKind::Image(PathBuf::from("x.gif")));
    // 異なる2フレーム(赤/緑の単色)を直接セットし、decode を介さず描画経路だけを検証。
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

    // フレーム0を準備+描画 → kitty 転送列がセル symbol に出る。
    let t0 = app.prepare_gif(area).expect("frame0 layout");
    let proto0 = app.gif_protocol().expect("frame0 should be encoded");
    let mut buf0 = Buffer::empty(area);
    Image::new(proto0).render(t0, &mut buf0);
    let dump0: String = buf0.content.iter().map(|c| c.symbol()).collect();
    assert!(
        dump0.contains("_G"),
        "frame0 は kitty graphics 転送(_G)を吐くはず(空でない描画)"
    );

    // フレーム1へ進めて再準備+描画 → 別の画像データが出る(churn せず原子的に切替)。
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
    assert_eq!(app.image_zoom, 1.0);
    app.image_zoom_by(0.5); // 1.0 未満には下がらない
    assert_eq!(app.image_zoom, 1.0);
    for _ in 0..40 {
        app.image_zoom_by(1.25);
    }
    assert_eq!(app.image_zoom, 16.0);
    app.image_zoom_reset();
    assert_eq!(app.image_zoom, 1.0);
    assert_eq!(app.image_center, (0.5, 0.5));
}

#[test]
fn fit_is_natural_size_and_centered_at_z1() {
    // z=1: 小さい画像(400x300, font10x20=40x15セル)は viewport より小さいので
    // 拡大せず natural(40x15)のまま中央表示(画面いっぱいにしない)。
    let mut app = app_with_image();
    let r = app.prepare_image(inner(200, 40)).unwrap();
    assert_eq!((r.width, r.height), (40, 15), "z=1 は natural サイズ");
    // 中央寄せ(左右・上下の余白が均等)。
    assert_eq!(r.x, (200 - 40) / 2);
    assert_eq!(r.y, (40 - 15) / 2);
    // 見切れていない → パン不可。
    assert_eq!(app.image_vis_frac, (1.0, 1.0));
}

#[test]
fn zoom_grows_then_clips_and_enables_pan() {
    let mut app = app_with_image();
    let inner = inner(200, 40);
    // z=2: 80x30 セル、まだ viewport(200x40)内 → 全体表示・中央・パン不可。
    app.image_zoom = 2.0;
    let r = app.prepare_image(inner).unwrap();
    assert_eq!((r.width, r.height), (80, 30));
    assert_eq!(app.image_vis_frac, (1.0, 1.0));
    // z=4: 160x60 → 高さ60>40 で縦が見切れる。表示は高さが viewport に制限され、縦パン可。
    app.image_zoom = 4.0;
    let r = app.prepare_image(inner).unwrap();
    assert_eq!(r.height, 40, "高さは viewport に制限");
    assert_eq!(r.width, 160);
    assert!(app.image_vis_frac.1 < 1.0, "縦が見切れ=パン可");
    assert_eq!(app.image_vis_frac.0, 1.0, "横はまだ収まる");
    // 中央寄せは維持。
    assert_eq!(r.x, (200 - 160) / 2);
}

#[test]
fn pan_noop_when_not_clipped_and_moves_when_clipped() {
    let mut app = app_with_image();
    let inner = inner(200, 40);
    // z=1: 見切れ無し → パンしても prepare で中心が 0.5 に戻る。
    app.image_pan(1.0, 1.0);
    app.prepare_image(inner).unwrap();
    assert_eq!(app.image_center, (0.5, 0.5));
    // 十分拡大して両軸見切れ → パンで中心が動き、端でクランプされる。
    app.image_zoom = 12.0;
    app.prepare_image(inner).unwrap(); // 可視率を更新
    for _ in 0..50 {
        app.image_pan(1.0, 1.0);
        app.prepare_image(inner).unwrap();
    }
    let (fw, fh) = app.image_vis_frac;
    assert!(fw < 1.0 && fh < 1.0, "両軸見切れ");
    // 端 = 1 - frac/2 にクランプ。
    assert!((app.image_center.0 - (1.0 - fw / 2.0)).abs() < 1e-6);
    assert!((app.image_center.1 - (1.0 - fh / 2.0)).abs() < 1e-6);
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
    // 既定 tabbar=auto: 1枚のときはタブバーを出さない。
    assert!(!render(&mut app).contains("1:konoma_tabbar_test"));
    // 2枚にするとタブバーに 1:/2: が出る。
    app.tab_new().unwrap();
    let s = render(&mut app);
    assert!(s.contains("1:konoma_tabbar_test"), "タブ1が無い: {s:?}");
    assert!(s.contains("2:konoma_tabbar_test"), "タブ2が無い: {s:?}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn markdown_links_collected_and_local_link_opens_in_konoma() {
    // md プレビューでリンク(URL)を収集し、フォーカス→ローカルリンクは konoma 内で開く。
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
    app.selected = app
        .entries
        .iter()
        .position(|e| e.path.ends_with("doc.md"))
        .unwrap();
    app.tree_activate().unwrap();
    // 描画でリンクを収集 (decorate_links が走る)。
    let mut term = Terminal::new(TestBackend::new(72, 8)).unwrap();
    term.draw(|f| crate::ui::render(f, &mut app)).unwrap();
    assert_eq!(app.md_items.len(), 2, "リンク数");
    assert!(item_target(&app.md_items[0]).ends_with("target.md"));
    assert_eq!(item_target(&app.md_items[1]), "https://example.com/x");
    // 先頭リンク(ローカル)にフォーカスして開く → konoma が target.md をプレビュー。
    app.md_focus_move(1);
    assert_eq!(app.focused_item, Some(0));
    app.md_activate_focused().unwrap();
    assert!(
        app.preview_path
            .as_deref()
            .is_some_and(|p| p.ends_with("target.md")),
        "ローカルリンクで target.md に遷移していない: {:?}",
        app.preview_path
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn md_focus_follows_offscreen_items_when_wrapped() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    // 折返し(wrap=true 既定)で長い段落が画面高さを超えるとき、Tab のフォーカス移動が
    // **表示行**基準でスクロール追従する。論理行基準だと「まだ画面内」と誤判定して
    // 一切スクロールしない(2026-07-08 ユーザー報告の回帰)。
    let dir = std::env::temp_dir().join("konoma_md_focus_wrap_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // 論理行は数行だが、先頭段落が幅40で大きく折り返して 20 行超の表示になる文書。
    let long = "word ".repeat(200); // 1000桁 ≒ 幅40で25表示行
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
    app.selected = app.entries.iter().position(|e| !e.is_dir).unwrap();
    app.tree_activate().unwrap();
    let mut term = Terminal::new(TestBackend::new(40, 12)).unwrap();
    term.draw(|f| crate::ui::render(f, &mut app)).unwrap();
    assert_eq!(app.md_items.len(), 1);
    assert_eq!(app.preview_scroll, 0);
    // Tab でリンクへ: リンクの論理行(2)は viewport(10) 未満だが、表示行では 25 行目付近
    // → スクロールが表示行基準で進む(旧実装は 0 のまま=このアサーションで落ちる)。
    app.md_focus_move(1);
    assert!(
        app.preview_scroll > 5,
        "折返し文書でフォーカスに追従してスクロールする: scroll={}",
        app.preview_scroll
    );
    // 追従後の描画でフォーカス行が画面内にある(表示行の窓に収まる)。
    term.draw(|f| crate::ui::render(f, &mut app)).unwrap();
    let buf = term.backend().buffer().clone();
    let screen: String = buf.content().iter().map(|c| c.symbol()).collect();
    assert!(
        screen.contains("link"),
        "フォーカスしたリンクが画面内に描画される"
    );
    // 逆方向(Shift-Tab 相当)で戻ると巡回して同じアイテム=スクロール維持、
    // 先頭側のアイテムが無いので上方向追従は別文書で担保(上方向分岐は同式の対称)。
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn md_task_toggle_cycles_and_writes_file() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    // Tab でチェックボックスへフォーカス → トグルで状態文字1文字だけがファイルに書き戻る。
    // 既定サイクル ' '⇄'x'・大文字 X は x と同値・他行は不変(CJK 本文でもバイト破壊なし)。
    let dir = std::env::temp_dir().join("konoma_md_task_toggle_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("todo.md");
    std::fs::write(&f, "# t\n\n- [ ] 未着手のタスク\n- [X] 済み\n").unwrap();
    let mut app = App::new(dir.canonicalize().unwrap(), Config::default()).unwrap();
    app.selected = app
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
    // 再描画で状態を取り直してから逆方向: 'x'→' '。
    term.draw(|fr| crate::ui::render(fr, &mut app)).unwrap();
    app.md_toggle_focused_task();
    let s = std::fs::read_to_string(&f).unwrap();
    assert!(s.contains("- [ ] 未着手のタスク"), "{s}");
    // 2つ目(大文字 X)へ: X=x と同値 → 次は ' '。
    term.draw(|fr| crate::ui::render(fr, &mut app)).unwrap();
    app.md_focus_move(1);
    app.md_toggle_focused_task();
    let s = std::fs::read_to_string(&f).unwrap();
    assert!(s.contains("- [ ] 済み"), "{s}");
    std::fs::remove_dir_all(&dir).ok();
}

/// `*` と `+` の箇条書きのチェックボックスもトグルできる(source scanner が `-` しか認識せず、
/// 個数照合が外れて「file changed on disk」で全キャンセルされていた・ユーザー報告 2026-07-22)。
#[test]
fn md_task_toggle_star_and_plus_bullets() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    let dir = std::env::temp_dir().join("konoma_md_task_star_plus");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("todo.md");
    // `*`/`+` 箇条書き(GFM 準拠)。tui-markdown は3種ともチェックボックス描画する。
    std::fs::write(&f, "* [ ] star task\n+ [ ] plus task\n").unwrap();
    let mut app = App::new(dir.canonicalize().unwrap(), Config::default()).unwrap();
    app.selected = app
        .entries
        .iter()
        .position(|e| e.path.ends_with("todo.md"))
        .unwrap();
    app.tree_activate().unwrap();
    let mut term = Terminal::new(TestBackend::new(72, 12)).unwrap();
    term.draw(|fr| crate::ui::render(fr, &mut app)).unwrap();
    assert_eq!(app.md_items.len(), 2, "star/plus 両方がタスクとして認識");

    // 1つ目(star)をトグル → キャンセルされず書き戻る。flash に「changed on disk」は出ない。
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

    // 2つ目(plus)も。
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
    // ui.md_task_states = [" ", "/", "x"]: Space が配列順に巡回し、カスタム状態 [/] も
    // 再描画後にトグル対象として認識される。
    let dir = std::env::temp_dir().join("konoma_md_task_custom_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("todo.md");
    std::fs::write(&f, "- [ ] a\n").unwrap();
    let mut cfg = Config::default();
    cfg.ui.md_task_states = vec![" ".into(), "/".into(), "x".into()];
    let mut app = App::new(dir.canonicalize().unwrap(), cfg).unwrap();
    app.selected = app.entries.iter().position(|e| !e.is_dir).unwrap();
    app.tree_activate().unwrap();
    let mut term = Terminal::new(TestBackend::new(60, 8)).unwrap();
    let cycle = |app: &mut App, term: &mut Terminal<TestBackend>| {
        term.draw(|fr| crate::ui::render(fr, app)).unwrap();
        if app.focused_item.is_none() {
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
    // 表示とディスクの状態が食い違ったら書かない(flash+再読込)。外部エージェントの編集と競合しない。
    let dir = std::env::temp_dir().join("konoma_md_task_abort_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("todo.md");
    std::fs::write(&f, "- [ ] a\n").unwrap();
    let mut app = App::new(dir.canonicalize().unwrap(), Config::default()).unwrap();
    app.selected = app.entries.iter().position(|e| !e.is_dir).unwrap();
    app.tree_activate().unwrap();
    let mut term = Terminal::new(TestBackend::new(60, 8)).unwrap();
    term.draw(|fr| crate::ui::render(fr, &mut app)).unwrap();
    app.md_focus_move(1);
    // ①状態の食い違い: 画面は ' ' のままディスクは 'x' に。
    std::fs::write(&f, "- [x] a\n").unwrap();
    app.md_toggle_focused_task();
    assert_eq!(
        std::fs::read_to_string(&f).unwrap(),
        "- [x] a\n",
        "書かない"
    );
    assert!(app.flash.is_some(), "flash で通知");
    // ②個数の食い違い: タスクが増えた(描画側は1個のまま)。
    term.draw(|fr| crate::ui::render(fr, &mut app)).unwrap(); // 画面を x に追従させ…
    std::fs::write(&f, "- [x] a\n- [ ] b\n").unwrap(); // …ディスクだけ2個に
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
    app.selected = app.entries.iter().position(|e| !e.is_dir).unwrap();
    app.tree_activate().unwrap();
    let mut term = Terminal::new(TestBackend::new(60, 8)).unwrap();
    term.draw(|fr| crate::ui::render(fr, &mut app)).unwrap();
    app.md_focus_move(1);
    // raw ソース表示(R)中はトグルもフォーカス判定も無効(2D キャレット面)。
    app.toggle_md_raw();
    assert!(!app.md_focused_task());
    app.md_toggle_focused_task();
    assert_eq!(
        std::fs::read_to_string(&f).unwrap(),
        "- [ ] a\r\n\r\ntail\r\n",
        "raw 中は書かない"
    );
    // 装飾表示に戻せばトグルでき、CRLF・末尾バイトは保たれる(1文字だけの置換)。
    app.toggle_md_raw();
    app.md_toggle_focused_task();
    assert_eq!(
        std::fs::read_to_string(&f).unwrap(),
        "- [x] a\r\n\r\ntail\r\n"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// トグルの安全ガード: ①どのチェックボックスにもフォーカスしていなければ無操作(書かない)
/// ②表示中ファイルが読めなくなっていたら flash で通知して安全に戻る(クラッシュしない・原則#3)。
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
    app.selected = app.entries.iter().position(|e| !e.is_dir).unwrap();
    app.tree_activate().unwrap();
    let mut term = Terminal::new(TestBackend::new(60, 8)).unwrap();
    term.draw(|fr| crate::ui::render(fr, &mut app)).unwrap();
    assert!(app.md_has_tasks());
    // ①未フォーカス(Tab を押していない)ではトグルしても書かない。
    assert!(app.focused_item.is_none());
    app.md_toggle_focused_task();
    assert_eq!(
        std::fs::read_to_string(&f).unwrap(),
        "- [ ] a\n",
        "未フォーカスでは書かない"
    );
    // ②フォーカスした状態でファイルが消えたら flash + 無操作(パニックしない)。
    app.md_focus_move(1);
    assert!(app.md_focused_task());
    std::fs::remove_file(&f).unwrap();
    app.md_toggle_focused_task();
    assert!(app.flash.is_some(), "読取エラーは flash で通知");
    assert!(!f.exists(), "書き直さない");
    std::fs::remove_dir_all(&dir).ok();
}

/// 書込みが拒否されても(読取専用ファイル)クラッシュせず flash で通知し、内容を壊さない(原則#3)。
#[cfg(unix)]
#[test]
fn md_task_toggle_flashes_on_write_error() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    use std::os::unix::fs::PermissionsExt;
    let dir = std::env::temp_dir().join("konoma_md_task_wrerr_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("todo.md");
    std::fs::write(&f, "- [ ] a\n").unwrap();
    let mut app = App::new(dir.canonicalize().unwrap(), Config::default()).unwrap();
    app.selected = app.entries.iter().position(|e| !e.is_dir).unwrap();
    app.tree_activate().unwrap();
    let mut term = Terminal::new(TestBackend::new(60, 8)).unwrap();
    term.draw(|fr| crate::ui::render(fr, &mut app)).unwrap();
    app.md_focus_move(1);
    // 読取専用にする: 読み(照合)は通るが書込みが EACCES で失敗する。
    std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o444)).unwrap();
    app.md_toggle_focused_task();
    assert!(app.flash.is_some(), "書込みエラーは flash で通知");
    // 書込みが拒否されたので内容は元のまま。
    std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(
        std::fs::read_to_string(&f).unwrap(),
        "- [ ] a\n",
        "拒否されたら壊さない"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// ファイル情報ポップアップ(`i`)の描画: 通常ファイル/ディレクトリ(項目数)/読めない対象(エラー行)/
/// 対象なし(早期 return・クラッシュしない)の各分岐を実描画バッファで確認する。
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

    // 通常ファイル: Type=file。
    app.selected = app
        .entries
        .iter()
        .position(|e| e.path.ends_with("reg.txt"))
        .unwrap();
    let s = info_screen(&app);
    assert!(s.contains(tr(Lang::En, Msg::InfoFile)), "file 種別: {s}");

    // ディレクトリ: Type=directory ＋ 項目数。
    app.selected = app
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

    // 読めない対象(不在): エラー行(クラッシュしない)。
    app.mode = Mode::Preview;
    app.preview_path = Some(dir.join("gone.txt"));
    let s = info_screen(&app);
    assert!(s.contains(tr(Lang::En, Msg::Failed)), "エラー表示: {s}");

    // 対象なし: 早期 return(何も描かない)。
    app.preview_path = None;
    let s = info_screen(&app);
    assert!(
        !s.contains(tr(Lang::En, Msg::InfoFile)),
        "対象なしは描画しない"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// シンボリックリンクのファイル情報: Type=symlink ＋ リンク先(Target)行を描く。
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
    app.selected = app
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

/// paste-jump のナビゲーション分岐: ①絶対パス解決 ②GitHub URL の末尾が実在せず not found flash
/// ③装飾 Markdown に `#L` を渡すと raw ソースへ切替(行アドレス可能に) ④テーブル(非 windowed)は
/// 行ジャンプが no-op(クラッシュしない)。
#[test]
fn paste_jump_navigation_branches() {
    let dir = unique_tmp("konoma_paste_nav_test");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("abs_target.txt"), "x\ny\nz\n").unwrap();
    std::fs::write(dir.join("doc.md"), "# H1\n\nl2\nl3\nl4\nl5\nl6\n").unwrap();
    std::fs::write(dir.join("data.csv"), "a,b\n1,2\n3,4\n").unwrap();
    let dir = dir.canonicalize().unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();

    // ①絶対パス(resolve_local_path の absolute 枝) → プレビュー。
    let abs = dir.join("abs_target.txt");
    app.paste_jump_from(abs.to_str().unwrap());
    assert_eq!(app.mode, Mode::Preview);
    assert!(app
        .preview_path
        .as_deref()
        .map(|p| p.ends_with("abs_target.txt"))
        .unwrap_or(false));

    // ②GitHub URL だが末尾が実在しない → not found flash(resolve_url_components が None)。
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

    // ③装飾 Markdown + `#L` → raw ソースへ切替(preview_goto_line の decorated 枝)。
    app.paste_jump_from("doc.md#L5");
    assert!(app
        .preview_path
        .as_deref()
        .map(|p| p.ends_with("doc.md"))
        .unwrap_or(false));
    assert!(app.is_md_raw(), "装飾 md + 行 → raw ソースへ切替");

    // ④テーブル(非 windowed)に `#L` → 行ジャンプは no-op(テーブル面のまま)。
    app.paste_jump_from("data.csv#L2");
    assert!(
        app.is_table_preview(),
        "csv はテーブル面のまま(行ジャンプ no-op)"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// repo のサブディレクトリで開いた状態では、root 直下に無い相対パスも repo(workdir)基準で解決する
/// (resolve_local_path の workdir フォールバック枝)。
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
    assert_eq!(app.root, subdir, "起動 root は src サブディレクトリ");
    // root(src)直下に docs/guide.md は無いが、repo workdir 基準で解決できる。
    app.paste_jump_from("docs/guide.md");
    assert_eq!(app.mode, Mode::Preview);
    assert!(
        app.preview_path
            .as_deref()
            .map(|p| p.ends_with("docs/guide.md"))
            .unwrap_or(false),
        "workdir 基準で解決: {:?}",
        app.preview_path
    );
    std::fs::remove_dir_all(&repo).ok();
}

#[test]
fn md_items_mix_links_and_tasks_in_document_order() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    // リンクとチェックボックスが文書順で1本の Tab 巡回に載る。
    let dir = std::env::temp_dir().join("konoma_md_items_mix_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("doc.md"),
        "[l1](https://x/1)\n\n- [ ] t1\n\n[l2](https://x/2)\n",
    )
    .unwrap();
    let mut app = App::new(dir.canonicalize().unwrap(), Config::default()).unwrap();
    app.selected = app.entries.iter().position(|e| !e.is_dir).unwrap();
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
    assert_eq!(app.focused_item, Some(0), "巡回で先頭へ戻る");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn md_code_block_is_tab_focusable_and_copies_source() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    // リンク・コードブロック・タスクが文書順で1本の Tab 巡回に載り、コードブロックに
    // フォーカスすると Enter でその生ソースをコピーできる(focused_code_text で値照合)。
    let dir = std::env::temp_dir().join("konoma_md_code_focus_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("doc.md"),
        "[l1](https://x/1)\n\n```rust\nfn main() {}\nlet x = 1;\n```\n\n- [ ] t1\n",
    )
    .unwrap();
    let mut app = App::new(dir.canonicalize().unwrap(), Config::default()).unwrap();
    app.selected = app.entries.iter().position(|e| !e.is_dir).unwrap();
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

    // フォーカス無し/リンク上ではコードコピー不可。
    assert!(app.focused_code_text().is_none());
    app.md_focus_move(1); // リンク
    assert!(!app.md_focused_code());
    app.md_focus_move(1); // コードブロック
    assert!(app.md_focused_code(), "2番目=コードブロック");
    assert_eq!(
        app.focused_code_text().as_deref(),
        Some("fn main() {}\nlet x = 1;"),
        "生ソース(装飾/ハイライトを含まない)をコピー"
    );

    // フォーカス手がかり: 再描画するとコードブロックのヘッダ行(言語名 rust を含む)が
    // 全幅反転(REVERSED)される。描画バッファのセル修飾で確認(tmux に依らず決定的)。
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
        // ヘッダ行の可視セル(▎ を含む)が REVERSED を持つ。
        let reversed = (0..w)
            .filter(|&x| buf[(x, y)].symbol().trim() != "" || buf[(x, y)].symbol() == "▎")
            .any(|x| buf[(x, y)].modifier.contains(Modifier::REVERSED));
        assert!(
            reversed,
            "フォーカス中コードブロックのヘッダ行が反転していない"
        );
    }

    // コピー動作(`y` 経由で呼ばれる。クリップボードは環境依存なのでパニックしないことだけ確認)。
    app.md_copy_focused_code();
    // Enter はコードブロックでは何もしない(開く/トグル対象が無い)。
    app.md_activate_focused().unwrap();
    app.md_focus_move(1); // タスク
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
    // tui-markdown の「ラベル (URL)」相当の span 列。末尾に "." が続く。
    let lines = vec![Line::from(vec![
        Span::raw("See "),
        Span::raw("the docs"),
        Span::raw(" ("),
        Span::styled("https://x/y", blue),
        Span::raw(")."),
    ])];
    // icons OFF: ラベルのみ表示・URL は targets へ・末尾 "." は保持。
    let (off, targets) = collapse_links(lines.clone(), false);
    assert_eq!(targets, vec!["https://x/y".to_string()]);
    let joined: String = off[0].spans.iter().map(|s| s.content.as_ref()).collect();
    assert_eq!(joined, "See the docs.", "URL 併記が消えてラベルのみ");
    assert_eq!(
        off[0].spans.iter().filter(|s| is_link_span(s)).count(),
        1,
        "リンク span は1つ"
    );
    // icons ON: ラベル先頭にリンクアイコン。
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
    app.focused_item = Some(1);
    let out = app.decorate_md_items(lines);
    assert_eq!(app.md_items.len(), 2);
    // フォーカス中(1番目)のリンク span だけ REVERSED が付く。
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
    // .rs を開くと Code 種別へ解決され、decorated_lines が色付き(Rgb 前景)の行を返す。
    use ratatui::style::Color;
    let dir = std::env::temp_dir().join("konoma_code_decorate_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.rs"), b"fn main() { let x = 1; }\n").unwrap();
    let mut app = App::new(dir.canonicalize().unwrap(), Config::default()).unwrap();
    let i = app.entries.iter().position(|e| !e.is_dir).unwrap();
    app.selected = i;
    app.tree_activate().unwrap();
    assert!(
        matches!(app.preview_kind, Some(PreviewKind::Code(_))),
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
    // `/` 絞り込み: root 配下を再帰収集し、部分一致(大小無視)で entries を絞る。Esc で通常へ。
    let dir = std::env::temp_dir().join("konoma_tree_filter_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src/ui")).unwrap();
    std::fs::write(dir.join("src/ui/tree.rs"), b"x").unwrap();
    std::fs::write(dir.join("src/main.rs"), b"x").unwrap();
    std::fs::write(dir.join("README.md"), b"x").unwrap();
    let mut app = App::new(dir.canonicalize().unwrap(), Config::default()).unwrap();
    let normal_count = app.entries.len();

    app.start_filter();
    assert!(app.is_filtering());
    // `/` 直後(クエリ空)は全件展開せず何も出さない。
    assert!(
        app.entries.is_empty(),
        "クエリ空のとき結果は0件であるべき(全展開しない)"
    );
    for c in "TREE".chars() {
        app.filter_input_push(c); // 大文字でも一致(大小無視)
    }
    assert_eq!(app.filter_query(), Some("TREE"));
    // 2 階層下の tree.rs を再帰的に発見。
    assert_eq!(app.entries.len(), 1, "tree 一致は1件");
    assert!(app.entries[0].path.ends_with("tree.rs"));

    // 1文字削るとライブで広がる("TRE")。tree.rs は残る。
    app.filter_input_backspace();
    assert_eq!(app.filter_query(), Some("TRE"));
    assert!(app.entries.iter().any(|e| e.path.ends_with("tree.rs")));

    // 確定(Enter): 入力は抜けるが絞り込みは維持。
    app.filter_commit();
    assert!(!app.is_filtering());
    assert!(app.filter_query().is_some());

    // 解除(Esc): 通常ツリーへ戻る。
    app.filter_clear();
    assert!(app.filter_query().is_none());
    assert_eq!(app.entries.len(), normal_count, "通常ツリーに復帰");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn small_code_file_is_also_windowed() {
    // #1: サイズに関わらず Code は windowed(全行ハイライトしない)＝小ファイルでも即時。
    let dir = std::env::temp_dir().join("konoma_small_windowed_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.rs"), b"fn main() {}\n").unwrap();
    let mut app = App::new(dir.canonicalize().unwrap(), Config::default()).unwrap();
    let i = app.entries.iter().position(|e| !e.is_dir).unwrap();
    app.selected = i;
    app.tree_activate().unwrap();
    assert!(
        app.is_windowed(),
        "小さい Code ファイルも windowed であるべき"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn line_numbers_gutter_tracks_position() {
    // #3: ui.line_numbers=true で行番号ガターが出る。先頭=1、G で末尾の正しい行番号。
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
        .entries
        .iter()
        .position(|e| e.path.ends_with("a.rs"))
        .unwrap();
    app.selected = i;
    app.tree_activate().unwrap();
    app.preview_viewport = 5;

    // 行頭ガター span(最初の span)の数字部分。
    let gutter_num = |lines: &[Line<'static>], row: usize| -> String {
        lines[row].spans[0].content.trim().to_string()
    };
    let top = app.windowed_lines(5, 80);
    assert_eq!(gutter_num(&top, 0), "1", "先頭行番号が 1 でない");
    assert_eq!(gutter_num(&top, 1), "2");
    // G で末尾へ → 末尾 5 行は 26..30。先頭行番号 = 26。
    app.preview_to_bottom();
    let bot = app.windowed_lines(5, 80);
    assert_eq!(
        gutter_num(&bot, 0),
        "26",
        "G 後の先頭行番号が総行数基準でない"
    );
    assert_eq!(gutter_num(&bot, 4), "30", "末尾行番号が 30 でない");

    // OFF(既定)ではガターが付かない(先頭 span が行番号でない)。
    let mut app2 = App::new(dir.canonicalize().unwrap(), Config::default()).unwrap();
    app2.selected = app2
        .entries
        .iter()
        .position(|e| e.path.ends_with("a.rs"))
        .unwrap();
    app2.tree_activate().unwrap();
    app2.preview_viewport = 5;
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
    // 閾値超の Code は less 風ウィンドウ読み。全文走査せず先頭/末尾の窓だけ読み・着色する。
    use ratatui::style::Color;
    use std::io::Write;
    let dir = std::env::temp_dir().join("konoma_windowed_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("big.rs");
    {
        let mut f = std::fs::File::create(&path).unwrap();
        // ~9000 行 × ~40B ≈ 350KB > WINDOW_THRESHOLD(256KB)。
        for i in 1..=9000 {
            writeln!(f, "fn func_{i:05}() -> i32 {{ {i} }} // line {i}").unwrap();
        }
    }
    let mut app = App::new(dir.canonicalize().unwrap(), Config::default()).unwrap();
    let i = app
        .entries
        .iter()
        .position(|e| e.path.ends_with("big.rs"))
        .unwrap();
    app.selected = i;
    app.tree_activate().unwrap();
    assert!(app.is_windowed(), "大きいファイルが windowed にならない");
    app.preview_viewport = 5;

    let joined = |lines: &[Line<'static>]| -> String {
        lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .map(|s| s.content.as_ref())
            .collect()
    };
    // 先頭ウィンドウは line 1 を含む。
    let top = app.windowed_lines(5, 80);
    assert!(joined(&top).contains("func_00001"), "先頭が出ない");
    // G で末尾へ → 末尾ウィンドウは line 9000 を含み、先頭は見えない(全文走査せず)。
    app.preview_to_bottom();
    let bot = app.windowed_lines(5, 80);
    let bot_text = joined(&bot);
    assert!(bot_text.contains("func_09000"), "末尾が出ない: {bot_text}");
    assert!(!bot_text.contains("func_00001"), "末尾なのに先頭が見える");
    // ウィンドウも syntect で着色される。
    let colored = bot
        .iter()
        .flat_map(|l| l.spans.iter())
        .any(|s| matches!(s.style.fg, Some(Color::Rgb(_, _, _))));
    assert!(colored, "windowed の着色が無い");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn tab_label_reflects_tree_root_or_preview_file() {
    // タブ名: Tree 表示中はルートのディレクトリ名、Preview/画像中はファイル名。
    let dir = std::env::temp_dir().join("konoma_tab_label_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("doc.md"), b"# hi\n").unwrap();
    let mut app = App::new(dir.canonicalize().unwrap(), Config::default()).unwrap();
    // Tree 表示中はルートのディレクトリ名。
    assert_eq!(app.tab_label(0), "konoma_tab_label_test");
    // doc.md をプレビュー → タブ名がファイル名になる。
    let idx = app.entries.iter().position(|e| !e.is_dir).unwrap();
    app.selected = idx;
    app.tree_activate().unwrap();
    assert_eq!(app.mode, Mode::Preview);
    assert_eq!(app.tab_label(0), "doc.md", "Preview 中はファイル名");
    // 新規タブ(Tree)はルート名。非アクティブの元タブはスナップショットのファイル名のまま。
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
    // 各タブはプレビュー状態を保持する。Preview→新規タブ(Tree)→戻ると Preview が復元される
    // (Tree に落とさない)。
    let dir = std::env::temp_dir().join("konoma_tab_preview_restore_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("doc.md"), b"# Hello\n\nbody\n").unwrap();
    let mut app = App::new(dir.canonicalize().unwrap(), Config::default()).unwrap();

    // doc.md を選んでプレビューへ。
    let idx = app.entries.iter().position(|e| !e.is_dir).unwrap();
    app.selected = idx;
    app.tree_activate().unwrap();
    assert_eq!(app.mode, Mode::Preview);
    let previewed = app.preview_path.clone();
    assert!(previewed.is_some());

    // 新規タブは Tree から (プレビューを持ち込まない)。
    app.tab_new().unwrap();
    assert_eq!(app.mode, Mode::Tree, "新規タブは Tree");
    assert!(app.preview_path.is_none(), "新規タブはプレビュー無し");

    // 元のタブへ戻ると Preview と対象が復元される。
    app.tab_cycle(-1);
    assert_eq!(app.mode, Mode::Preview, "戻ると Preview が復元される");
    assert_eq!(app.preview_path, previewed, "プレビュー対象も復元される");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn tabs_create_switch_close_preserve_state() {
    // 2つの子ディレクトリを持つツリーで、タブごとに選択位置が保たれることを確認。
    let dir = std::env::temp_dir().join("konoma_tabs_test");
    std::fs::create_dir_all(dir.join("a")).unwrap();
    std::fs::create_dir_all(dir.join("b")).unwrap();
    std::fs::write(dir.join("f.txt"), b"x").unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    assert_eq!(app.tab_count(), 1);

    // タブ1で選択を1つ下へ。
    app.tree_next();
    let sel1 = app.selected;
    assert!(sel1 > 0);

    // 新規タブ → 2枚、アクティブは2枚目、選択は先頭。
    app.tab_new().unwrap();
    assert_eq!(app.tab_count(), 2);
    assert_eq!(app.active_tab_index(), 1);
    assert_eq!(app.selected, 0);
    app.tree_last();
    let sel2 = app.selected;

    // 前のタブへ戻ると選択位置が復元される。
    app.tab_cycle(-1);
    assert_eq!(app.active_tab_index(), 0);
    assert_eq!(app.selected, sel1, "タブ1の選択が保たれる");

    // 番号ジャンプでタブ2へ。
    app.tab_goto(1);
    assert_eq!(app.active_tab_index(), 1);
    assert_eq!(app.selected, sel2, "タブ2の選択が保たれる");

    // タブを閉じると1枚に戻り、残ったタブがアクティブ。
    app.tab_close();
    assert_eq!(app.tab_count(), 1);
    assert_eq!(app.active_tab_index(), 0);
    // 最後の1枚は閉じられない。
    app.tab_close();
    assert_eq!(app.tab_count(), 1);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn tab_selection_is_per_tab_root_change_clears_clipboard_is_global() {
    // #2/#6 の核: 選択(BTreeSet<PathBuf>)はタブ毎に保たれ・リークせず、
    // root 変更でクリアされ、クリップボード(Y/X/P)は app 全体共有のまま保持される。
    let dir = std::env::temp_dir().join("konoma_tab_selection_perq_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    for n in ["a.txt", "b.txt", "c.txt"] {
        std::fs::write(dir.join(n), b"x").unwrap();
    }
    std::fs::write(dir.join("sub").join("x.txt"), b"x").unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();

    let idx =
        |app: &App, p: std::path::PathBuf| app.entries.iter().position(|e| e.path == p).unwrap();

    // --- (a) タブAで複数選択 ---
    app.selected = idx(&app, dir.join("a.txt"));
    app.toggle_select(); // a.txt
    app.selected = idx(&app, dir.join("b.txt"));
    app.toggle_select(); // b.txt
    assert!(app.is_selected(&dir.join("a.txt")) && app.is_selected(&dir.join("b.txt")));
    assert_eq!(app.op_targets().len(), 2, "A の op_targets は選択2件");

    // --- (a) タブBへ → 選択は空(リークしない)・op_targets は現 root のカーソル1件のみ ---
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

    // --- (a) タブAへ戻ると選択復元 ---
    app.tab_cycle(-1);
    assert_eq!(app.active_tab_index(), 0);
    assert!(
        app.is_selected(&dir.join("a.txt")) && app.is_selected(&dir.join("b.txt")),
        "A へ戻ると選択が復元される"
    );
    assert_eq!(app.op_targets().len(), 2);

    // --- (b) root 変更(l=descend)で選択クリア・op_targets が別 root のパスを返さない ---
    app.selected = idx(&app, dir.join("sub"));
    app.tree_descend().unwrap();
    assert_eq!(app.root, dir.join("sub"), "sub が新 root");
    assert!(!app.has_selection(), "root 変更(l)で選択クリア");
    for t in app.op_targets() {
        assert!(
            t.starts_with(dir.join("sub")),
            "op_targets が別 root(旧 dir)のパスを返さない: {t:?}"
        );
    }

    // --- (b) root 変更(h=leave)でも選択クリア ---
    app.selected = idx(&app, dir.join("sub").join("x.txt"));
    app.toggle_select();
    assert!(app.has_selection(), "sub 内で選択を積む");
    app.tree_leave().unwrap();
    assert_eq!(app.root, dir, "親へ戻る");
    assert!(!app.has_selection(), "root 変更(h)で選択クリア");

    // --- (c) clipboard はタブ跨ぎで保持(global) ---
    app.selected = idx(&app, dir.join("a.txt"));
    app.toggle_select();
    app.copy_selection(); // Y: clipboard へ積む(選択はクリアされる)
    assert!(
        app.clipboard_label().is_some(),
        "コピーで clipboard に積まれる"
    );
    app.tab_new().unwrap();
    assert!(
        app.clipboard_label().is_some(),
        "clipboard はタブ跨ぎで保持(global=TabState に入れない)"
    );
    app.tab_cycle(-1);
    assert!(app.clipboard_label().is_some(), "戻っても clipboard 保持");

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn refresh_rereads_directory_listing() {
    // 自動更新の核: refresh() でディレクトリを読み直し、外部で増えたファイルが現れる。
    let dir = std::env::temp_dir().join("konoma_refresh_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.txt"), b"x").unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    let before = app.entries.len();
    std::fs::write(dir.join("b.txt"), b"y").unwrap(); // 外部でファイル追加
    app.refresh().unwrap();
    assert!(
        app.entries.len() > before,
        "新規ファイルが一覧に反映されない"
    );
    assert!(app.entries.iter().any(|e| e.path.ends_with("b.txt")));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn refresh_prunes_deleted_paths_from_selection() {
    // #12: 外部で消えたパスを refresh() が selection から剪定する
    // (1件でも消えると一括ゴミ箱送りが全失敗していた回帰を防ぐ)。
    let dir = std::env::temp_dir().join("konoma_refresh_prune_sel");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for n in ["a.txt", "b.txt", "c.txt"] {
        std::fs::write(dir.join(n), b"x").unwrap();
    }
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    // 3件すべて選択。
    for n in ["a.txt", "b.txt", "c.txt"] {
        app.selection.insert(dir.join(n));
    }
    assert_eq!(app.op_targets().len(), 3, "選択3件が一括対象");
    // 外部で b.txt を削除 → refresh で selection から b.txt だけ剪定される。
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
    // 既知バグ: Preview モード中の refresh() で現プレビューを再読込(外部編集を反映)する。
    // 観測点: decorated_lines が張る装飾キャッシュ(md_cache)が refresh で無効化されること。
    let dir = std::env::temp_dir().join("konoma_refresh_reload_preview");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("doc.md"), b"# v1\n").unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    let idx = app
        .entries
        .iter()
        .position(|e| e.path.ends_with("doc.md"))
        .unwrap();
    app.selected = idx;
    app.tree_activate().unwrap(); // doc.md をプレビューで開く
    assert!(matches!(app.mode, Mode::Preview), "Preview モードへ");
    let _ = app.decorated_lines(80); // 装飾キャッシュを張る
    assert!(app.md_cache.is_some(), "プレビューでキャッシュが張られる");
    std::fs::write(dir.join("doc.md"), b"# v2 changed\n").unwrap(); // 外部編集
    app.refresh().unwrap();
    assert!(
        app.md_cache.is_none(),
        "refresh で装飾キャッシュが無効化され再読込される"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn out_of_root_watch_dir_targets_files_outside_the_root() {
    // 真因の回帰: FSEvents 監視は app.root だけを再帰監視する。root 外に表示中のファイル
    // (グローバルブックマーク先 / repo 全体の git ビュー)は変更イベントが来ず、AI 編集を検知できない。
    // out_of_root_watch_dir が「root 外プレビューはその親ディレクトリを返す / root 内・ツリーは None」を検証。
    let base = std::env::temp_dir().join("konoma_out_of_root_watch");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("root")).unwrap();
    std::fs::create_dir_all(base.join("outside")).unwrap();
    let root = base.join("root").canonicalize().unwrap();
    let outside = base.join("outside").canonicalize().unwrap();
    std::fs::write(root.join("inside.md"), b"# inside\n").unwrap();
    std::fs::write(outside.join("note.md"), b"# outside\n").unwrap();
    let mut app = App::new(root.clone(), Config::default()).unwrap();

    // ツリー表示中は常に追加監視しない。
    assert_eq!(
        app.out_of_root_watch_dir(),
        None,
        "ツリー表示中は追加監視しない"
    );

    // root 内ファイルのプレビュー → 再帰監視でカバー済 → None。
    app.enter_preview(&root.join("inside.md"));
    assert_eq!(app.mode, Mode::Preview);
    assert_eq!(app.out_of_root_watch_dir(), None, "root 内は追加監視不要");

    // root 外ファイルのプレビュー(ブックマーク先を模擬) → その親ディレクトリを追加監視対象に返す。
    app.enter_preview(&outside.join("note.md"));
    assert_eq!(
        app.out_of_root_watch_dir().as_deref(),
        Some(outside.as_path()),
        "root 外プレビューはその親ディレクトリを監視対象に返す"
    );

    // ツリーへ戻ると None(監視は外れる)。
    app.mode = Mode::Tree;
    assert_eq!(app.out_of_root_watch_dir(), None);

    std::fs::remove_dir_all(&base).ok();
}

#[test]
fn refresh_fs_reloads_preview_even_when_tree_rebuild_fails() {
    // 副次修正: refresh_fs で rebuild_tree が失敗しても、プレビュー再読込はスキップしない。
    // (エージェントの一括操作中に展開中ディレクトリが一瞬読めなくなっても、プレビューは追従する)。
    // 観測点: rebuild_tree が Err を返す状況でも、reload_preview が走って md_cache が無効化されること。
    let base = std::env::temp_dir().join("konoma_refresh_tree_fail");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("root")).unwrap();
    let root = base.join("root").canonicalize().unwrap();
    std::fs::write(root.join("doc.md"), b"# v1\n").unwrap();
    let mut app = App::new(root.clone(), Config::default()).unwrap();
    let idx = app
        .entries
        .iter()
        .position(|e| e.path.ends_with("doc.md"))
        .unwrap();
    app.selected = idx;
    app.tree_activate().unwrap();
    assert!(matches!(app.mode, Mode::Preview));
    let _ = app.decorated_lines(80);
    assert!(app.md_cache.is_some(), "プレビューでキャッシュが張られる");

    // root を存在しないパスへ差し替え → rebuild_tree(read_dir) が失敗する状況を作る。
    app.root = base.join("does-not-exist");
    std::fs::write(root.join("doc.md"), b"# v2 changed\n").unwrap(); // 外部編集

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
    // 画像/メディアのプレビューは、対象ファイルが変わった時だけ再ロードする(mtime ガード)。
    // 観測点: clear_image が進める media_gen と、基準時刻 preview_media_mtime。
    // (実デコードは picker 必須=テストでは no-op。再ロード経路が走ったかを media_gen で判定する。)
    use image::RgbImage;
    let dir = std::env::temp_dir().join("konoma_media_reload_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let pic = dir.join("pic.png");
    RgbImage::new(2, 2).save(&pic).unwrap(); // 有効な PNG (拡張子/mime どちらでも Image に解決)
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    let idx = app
        .entries
        .iter()
        .position(|e| e.path.ends_with("pic.png"))
        .unwrap();
    app.selected = idx;
    app.tree_activate().unwrap(); // 画像プレビューで開く
    assert!(
        matches!(
            app.preview_kind,
            Some(crate::preview::PreviewKind::Image(_))
        ),
        "画像として開く"
    );
    assert!(
        app.preview_media_mtime.is_some(),
        "ロード時に基準 mtime を記録"
    );

    let gen0 = app.media_gen;
    // 未変更 → mtime 一致 → 再ロードしない(無駄な再デコード/外部ツール実行を避ける)。
    app.refresh().unwrap();
    assert_eq!(
        app.media_gen, gen0,
        "未変更なら再ロードしない(mtime ガード)"
    );

    // ファイル変更を模擬(基準 mtime をずらす) → 変更検知で再ロードされる。
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
    std::fs::create_dir_all(dir.join("zdir")).unwrap(); // ディレクトリ
    std::fs::write(dir.join("a.md"), vec![0u8; 30]).unwrap(); // 30B
    std::fs::write(dir.join("b.txt"), vec![0u8; 10]).unwrap(); // 10B (最小)
    std::fs::write(dir.join("c.log"), vec![0u8; 50]).unwrap(); // 50B (最大)
    let mut app = App::new(dir.clone(), Config::default()).unwrap();

    let names = |a: &App| {
        a.entries
            .iter()
            .map(|e| e.path.file_name().unwrap().to_string_lossy().to_string())
            .collect::<Vec<_>>()
    };

    // 既定: 名前昇順・ディレクトリ先頭。
    assert_eq!(names(&app), vec!["zdir", "a.md", "b.txt", "c.log"]);

    // サイズ昇順(ディレクトリ先頭固定): ファイルは 10<30<50 = b,a,c。
    app.open_sort_menu();
    app.sort_menu_key('s').unwrap();
    assert_eq!(names(&app), vec!["zdir", "b.txt", "a.md", "c.log"]);
    assert!(!app.is_sort_menu(), "基準選択(n/s/m/e)でメニューは閉じる");

    // 反転(サイズ降順): 大→小 = c,a,b。トグルはメニューを開いたまま。
    app.open_sort_menu();
    app.sort_menu_key('r').unwrap();
    assert!(app.is_sort_menu(), "トグル(r/.)はメニューを開いたまま");
    assert_eq!(names(&app), vec!["zdir", "c.log", "a.md", "b.txt"]);

    // ディレクトリ先頭を解除し、名前昇順へ戻す → dir も名前順に混ざる。
    app.sort_menu_key('.').unwrap(); // dirs_first off
    app.sort_menu_key('r').unwrap(); // reverse off (元に戻す)
    app.sort_menu_key('n').unwrap(); // name (選択で閉じる)
    assert_eq!(names(&app), vec!["a.md", "b.txt", "c.log", "zdir"]);

    // 拡張子昇順(dirs_first off): "" < log < md < txt = zdir, c.log, a.md, b.txt。
    app.open_sort_menu();
    app.sort_menu_key('e').unwrap();
    assert_eq!(names(&app), vec!["zdir", "c.log", "a.md", "b.txt"]);

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn sort_config_sets_initial_order() {
    // [ui.sort] が起動時の並びに反映される (key=size, dirs_first=false)。
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

    // 設定が App.sort に反映されている。
    assert_eq!(app.sort.key, SortKey::Size);
    assert!(!app.sort.dirs_first);

    // size 昇順: ファイルは 10<30<50 = b,a,c の相対順 (dir のサイズは環境依存なので位置は問わない)。
    let names: Vec<String> = app
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
    // 実 ~/.config を汚さないようテスト用ベースへ差し替え。
    app.bookmarks = crate::bookmarks::Bookmarks::with_base(base.clone(), &app.open_dir);

    let sub_idx = app
        .entries
        .iter()
        .position(|e| e.path.ends_with("sub"))
        .unwrap();
    let file_idx = app
        .entries
        .iter()
        .position(|e| e.path.ends_with("f.txt"))
        .unwrap();

    // カーソルを sub(ディレクトリ)に置いて a に登録 → カーソルの dir が入る。
    app.selected = sub_idx;
    app.start_mark_set();
    assert!(app.is_marking());
    app.mark_input('a');
    assert!(!app.is_marking(), "登録後はマーク待ちが解除される");
    assert_eq!(app.bookmarks.get('a'), Some(proj.join("sub")));

    // カーソルを f.txt(ファイル)に置いて b に登録 → カーソルの file が入る。
    app.selected = file_idx;
    app.start_mark_set();
    app.mark_input('b');
    assert_eq!(app.bookmarks.get('b'), Some(proj.join("f.txt")));

    // ' → 一覧が開き、英字 a: ディレクトリ → そこへ移動 (Tree)。
    app.open_bookmark_list();
    app.bookmark_jump_letter('a');
    assert!(!app.is_bookmark_list(), "ジャンプで一覧が閉じる");
    assert_eq!(
        app.root,
        proj.join("sub"),
        "ディレクトリのブックマークはそこへ移動"
    );
    assert_eq!(app.mode, Mode::Tree);

    // root を proj に戻し、' b: ファイル → プレビューで開く (tree は変えない)。
    app.root = proj.clone();
    let _ = app.rebuild_tree();
    let root_before = app.root.clone();
    app.open_bookmark_list();
    app.bookmark_jump_letter('b');
    assert_eq!(
        app.mode,
        Mode::Preview,
        "ファイルのブックマークはプレビューで開く"
    );
    let f = proj.join("f.txt");
    assert_eq!(app.preview_path.as_deref(), Some(f.as_path()));
    assert_eq!(
        app.root, root_before,
        "ファイルを開いても tree(root) は変わらない"
    );

    // プレビュー中に e を押すと、そのブックマークのファイルが外部エディタの対象になる。
    app.request_edit();
    assert_eq!(
        app.take_pending_edit().map(|(p, _)| p).as_deref(),
        Some(f.as_path()),
        "ブックマークのファイルが編集対象になる"
    );

    // プレビューを終了すると元の tree に戻る。
    app.back_to_tree();
    assert_eq!(app.mode, Mode::Tree);
    assert_eq!(app.root, root_before);

    // 未登録マークはジャンプせず flash(一覧は開いたまま)。
    app.open_bookmark_list();
    app.bookmark_jump_letter('z');
    assert_eq!(app.root, root_before, "未登録マークではジャンプしない");
    assert!(app.flash.is_some());
    assert!(app.is_bookmark_list(), "未登録では一覧を閉じない");
    app.close_bookmark_list();

    // 一覧で e: 選択中がファイルなら直接エディタ対象に(プレビューを挟まない)。
    app.open_bookmark_list();
    app.bookmark_list_move(1); // a(dir) → b(file) へ
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
    // "sub" ディレクトリのエントリを選択。
    let i = app
        .entries
        .iter()
        .position(|e| e.is_dir && e.path.ends_with("sub"))
        .expect("sub が無い");
    app.selected = i;
    // l 相当: 選択ディレクトリを新しいルートにする。
    app.tree_descend().unwrap();
    assert_eq!(app.root, dir.join("sub"), "root が選択ディレクトリになる");
    assert_eq!(app.mode, Mode::Tree);
    assert!(
        app.entries.iter().any(|e| e.path.ends_with("f.txt")),
        "sub の中身が表示される"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn jump_to_dir_clears_selection_on_root_change() {
    // bug #2: root を変えると旧 root の選択がパスで残り、マーカー不可視のまま誤操作対象になる。
    // jump_to_dir(ブックマークジャンプ)でも clear_for_root_change を呼んで持ち越さないこと。
    let base = std::env::temp_dir().join("konoma_jump_clear_test");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("subB")).unwrap();
    std::fs::write(base.join("a.txt"), b"x").unwrap();
    let mut app = App::new(base.clone(), Config::default()).unwrap();
    // base で a.txt を選択する。
    let i = app
        .entries
        .iter()
        .position(|e| !e.is_dir && e.path.ends_with("a.txt"))
        .expect("a.txt が無い");
    app.selected = i;
    app.toggle_select();
    assert!(app.has_selection(), "前提: 選択がある");

    // subB へジャンプ(root 変更)。
    app.jump_to_dir(base.join("subB"));
    assert_eq!(app.root, base.join("subB"), "root が subB になる");
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
    // 相対は起動ディレクトリ名を先頭に (基準 = open_dir の親)。
    assert_eq!(
        copy_text(&file, &open, CopyKind::Relative),
        "konoma/src/main.rs"
    );
    assert_eq!(
        copy_text(&file, &open, CopyKind::Parent),
        "/Users/me/work/konoma/src"
    );
    // 起動ディレクトリの外(兄弟/上位)は format_path の表示と同じく `..` 込みで相対化する。
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
    // 遠い絶対パスも (表示と同様) open_dir 基準の `..` 連結で相対化する。
    let outside = PathBuf::from("/etc/hosts");
    assert_eq!(
        copy_text(&outside, &open, CopyKind::Relative),
        "../../../../etc/hosts"
    );
}

#[test]
fn copy_relative_matches_title_display() {
    // `yr`/`cr` のコピー文字列は左上のタイトル表示 (format_path の Relative) と完全一致する。
    let work = std::env::temp_dir().join("konoma_copy_match_title");
    let _ = std::fs::remove_dir_all(&work);
    let a = work.join("A");
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(work.join("B")).unwrap();
    let mut app = App::new(a.clone(), Config::default()).unwrap();
    app.path_style = PathStyle::Relative;
    for p in [
        a.join("x.rs"),                // 配下
        work.join("B").join("aaa.md"), // 兄弟
        work.clone(),                  // 上位
    ] {
        assert_eq!(
            copy_text(&p, &app.open_dir, CopyKind::Relative),
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
    // 画像プレビュー中の ? は画像節のみ。Tree/Git/テキスト専用節は出さない (モード最適化)。
    let mut app = app_with_image();
    app.mode = Mode::Preview;
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
    app.mode = Mode::Preview;
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
    app.image_zoom = 1.0;
    let s = text(&app);
    assert!(s.contains("IMAGE"), "IMAGE チップ: {s}");
    assert!(s.contains("x1.00"), "フィットで x1.00: {s}");
    app.image_zoom = 2.5;
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
    app.selected = app
        .entries
        .iter()
        .position(|e| e.path.ends_with(name))
        .unwrap();
    app.tree_activate().unwrap();
    app.preview_viewport = 5;
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
    // 無効なら重い待ち状態にせず、本文は素テキスト(Rgb 着色なし)。
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
    // 先に rs 文法を温めておけば、Code プレビューは pending 無しで即着色。
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

    // ファイル選択 → そのパスが編集要求になる。
    app.selected = app
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

    // ディレクトリ選択 → 要求は立たず flash で拒否。
    app.selected = app.entries.iter().position(|e| e.is_dir).unwrap();
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
    app.selected = app
        .entries
        .iter()
        .position(|e| e.path.ends_with("b.rs"))
        .unwrap();
    app.tree_activate().unwrap(); // Preview へ
    assert!(matches!(app.mode, Mode::Preview));
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
    // コマを進めると別グリフになり、10 コマで一巡する(=回る)。
    let g0 = app.spinner_glyph(); // frame 0
    app.tick_spinner();
    assert_ne!(g0, app.spinner_glyph(), "ティックでコマが進む(回る)");
    for _ in 0..9 {
        app.tick_spinner(); // 合計 10 ティック → frame 10 ≡ 0
    }
    assert_eq!(app.spinner_glyph(), g0, "10 コマで一巡して先頭へ");
    // 絵文字でなく点字パターン(U+2800〜U+28FF)＝端末で単一幅・色なし。
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
    app.hl_pending = true; // progressive のウォーム待ち中を強制
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
        // 7 行ごと(0,7,14,21,28,35)に "needle"。計 40 行。
        for i in 0..40 {
            let tail = if i % 7 == 0 { "needle here" } else { "x" };
            writeln!(f, "line {i} {tail}").unwrap();
        }
    }
    let mut app = App::new(dir.canonicalize().unwrap(), Config::default()).unwrap();
    app.selected = app.entries.iter().position(|e| !e.is_dir).unwrap();
    app.tree_activate().unwrap();
    assert!(app.is_windowed(), "コードはウィンドウ読み対象");

    // `/` → 入力モード。文字を積んで確定。
    app.start_search();
    assert!(app.is_searching(), "/ で検索入力モード");
    for c in "needle".chars() {
        app.search_input_push(c);
    }
    assert_eq!(app.search_input(), Some("needle"));
    app.search_commit();
    assert!(!app.is_searching(), "確定で入力モード解除");

    // 6 件・1件目(line0)へジャンプ・ステータス 1/6。
    assert_eq!(app.search_matches.len(), 6, "7行ごとに6件");
    assert_eq!(app.search_status(), Some((1, 6)));
    assert_eq!(app.preview_top_line, 0, "最初の一致(行0)を先頭に");
    assert_eq!(app.preview_search_query(), Some("needle"));

    // n で次(line7), もう一度で line14。
    app.search_next(1);
    assert_eq!(app.search_status(), Some((2, 6)));
    assert_eq!(app.preview_top_line, 7);
    app.search_next(1);
    assert_eq!(app.preview_top_line, 14);
    // N で戻る(line7)。
    app.search_next(-1);
    assert_eq!(app.preview_top_line, 7);
    // 末尾(idx0)から N で巡回 → 最終一致(line35)。
    app.search_idx = 0;
    app.search_next(-1);
    assert_eq!(app.search_status(), Some((6, 6)));
    assert_eq!(app.preview_top_line, 35);

    // Esc 相当: 解除でクエリ・一致が消える。
    app.search_clear();
    assert!(app.preview_search_query().is_none());
    assert_eq!(app.search_status(), None);
    assert!(app.search_matches.is_empty());

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn highlight_query_in_line_marks_only_matches_yellow() {
    use ratatui::style::Color;
    let line = Line::from(vec![Span::from("foo "), Span::from("Needle bar needle")]);
    let out = highlight_query_in_line(line, "needle", None);
    // 黄色背景が付いた断片の連結 = 一致テキスト(大文字小文字無視で2箇所)。
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
    // 全文は不変。
    let full: String = out.spans.iter().map(|s| s.content.as_ref()).collect();
    assert_eq!(full, "foo Needle bar needle");
}

#[test]
fn highlight_query_in_line_handles_multibyte_lines() {
    use ratatui::style::Color;
    // 日本語の小文字化はバイト長を変えないので、ASCII クエリは正しく強調される
    // (バイト境界も保たれパニックしない)。全文は不変。
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
    let line = Line::from("let needle = 1;"); // "needle" は行内 0 番目の出現
                                              // 出現順位 Some(0) はオレンジ、None は黄色(色で現在の出現を区別)。
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
    // 同一行に2つの "todo"(todos の todo と Todo)。出現順位 Some(1)=2つ目だけオレンジ。
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
    // #14: タブ展開後の行(先頭の "→" + 空白でバイト列が原文とずれる)でも、
    // 列(バイト位置)ではなく出現順位で現在一致を同定するので橙が正しい出現に乗る。
    // 行頭タブが "ab" の2つの "needle" の前にあり、原文の col とは異なるバイト位置になる。
    let expanded = crate::preview::code::expand_tabs(vec![Line::from("\tneedle ab needle")], 4);
    let line = expanded.into_iter().next().unwrap();
    // 出現順位 Some(1) = 2つ目の needle だけオレンジ、1つ目は黄色。
    let out = highlight_query_in_line(line, "needle", Some(1));
    // 橙断片が「ちょうど1つの needle 分」=現在一致だけに乗っていること。
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

// --- Git ビュー(変更ハブ) -------------------------------------------------
#[cfg(feature = "git")]
fn init_git_repo(dir: &Path) {
    let repo = git2::Repository::init(dir).unwrap();
    let mut cfg = repo.config().unwrap();
    cfg.set_str("user.name", "Test").unwrap();
    cfg.set_str("user.email", "test@example.com").unwrap();
    cfg.set_str("commit.gpgsign", "false").ok();
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

    // ② 基準 = 配列で左から最初に存在する develop。
    assert_eq!(
        app.git_graph_base_label(),
        Some("develop"),
        "config 配列の最初の存在ブランチが基準"
    );
    // ④ 並び順: 配列順(develop→main)→HEAD(trunk)→最近順。
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

    // ⑤ パネルで並び替え → 基準が新しい先頭へ再導出。
    app.git_graph_open_picker();
    // パネル順 = [develop, main, trunk, feat]。main(index1)を1つ上へ。
    app.git_graph_picker_sel = 1;
    app.git_graph_picker_reorder(-1); // main を先頭へ
    app.git_graph_picker_apply();
    assert_eq!(
        app.git_graph_base_label(),
        Some("main"),
        "並び替えで先頭になった main が基準に再導出される"
    );

    std::fs::remove_dir_all(&dir).ok();
}

// Phase G: 重い ignored(無視セット)は repo(workdir)単位でキャッシュし、同一 repo 内の
// サブディレクトリへ `l` で潜っても再計算しない(別 repo へ移ると再計算する)。
// 番兵を仕込んで「再計算されたか(=番兵が消えるか)」を観測する。
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

    // 番兵: 同一 repo 内へ潜って再計算されなければ生き残る。
    let sentinel = dir.join("__sentinel_marker__");
    app.git_ignored.insert(sentinel.clone());

    // sub を選択して潜行(root 変更・同一 repo)。
    let i = app
        .entries
        .iter()
        .position(|e| e.is_dir && e.path.ends_with("sub"))
        .expect("sub エントリが無い");
    app.selected = i;
    app.tree_descend().unwrap();
    assert!(app.root.ends_with("sub"), "root が sub になった");
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
        Some(app.root.as_path()),
        "statuses は root 変更のたびに取り直す"
    );
    std::fs::remove_dir_all(&dir).ok();
}

// Fix A(2026-07-13): プレビュー中に外部エージェントが git コミットし、その fs イベントを取りこぼしても
// (FSEvents は本質的に取りこぼす)、ツリーへ戻れば git status を再検証して変更マーカーが陳腐化しない。
// back_to_tree() が git_status_for を無効化 → 次の描画相当 refresh_git_if_needed が statuses を取り直す。
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
    // 変更 → ツリーで M。
    std::fs::write(dir.join("note.md"), b"v2\n").unwrap();

    let mut app = App::new(dir.canonicalize().unwrap(), Config::default()).unwrap();
    app.refresh_git_if_needed(); // git_status_for=root, note.md=M
    let note = dir.canonicalize().unwrap().join("note.md");
    assert!(
        app.git_status_of(&note).is_some(),
        "変更ファイルは M として見える"
    );

    // プレビューへ入る。
    app.mode = Mode::Preview;
    app.preview_path = Some(note.clone());

    // 外部でコミット(clean になる)。konoma には refresh を通さない = fs イベント取りこぼしの再現。
    git(&["commit", "-qam", "external commit"]);
    assert!(
        app.git_status_of(&note).is_some(),
        "取りこぼし再現: refresh 前は git_status がまだ古い(M のまま)"
    );

    // ツリーへ戻る → git_status_for 無効化。
    app.back_to_tree();
    assert!(
        app.git_status_for.is_none(),
        "back_to_tree が git status を無効化(次の描画で再取得)"
    );
    // 次の描画相当。
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
    // 外側 repoA の中に、独立した repoB(inner)を入れ子にする。
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

    // inner(別 repo)へ潜行。
    let i = app
        .entries
        .iter()
        .position(|e| e.is_dir && e.path.ends_with("inner"))
        .expect("inner エントリが無い");
    app.selected = i;
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

// fix③: 重い ignored は別スレッド計算→`apply_ignored` で反映する。世代(gen)で陳腐化判定し、
// 計算中に別 repo へ移った(=世代が進んだ)結果は捨てる。git 不要の純状態ロジックなので両 feature で走る。
#[test]
fn busy_indicator_reflects_background_jobs() {
    // busy_jobs は既存の各ジョブ状態から**導出**する(begin/end の対応漏れでスピナーが
    // 固まる事故を構造的に防ぐ)。アイドル=空・config off=非表示・複数ジョブ=+n 表記。
    let dir = std::env::temp_dir().join("konoma_busy_indicator_test");
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.lang = crate::i18n::Lang::En;
    assert!(app.busy_jobs().is_empty(), "アイドルではジョブ無し");
    assert!(!app.busy_indicator_active());

    // メディア読込中 → 右上コンテキストにスピナー+ラベル。
    app.media_loading = true;
    assert!(app.busy_jobs().contains(&crate::i18n::Msg::BusyMedia));
    assert!(app.busy_indicator_active());
    let joined: String = crate::ui::status::context_spans(&app)
        .iter()
        .map(|s| s.content.as_ref())
        .collect();
    assert!(joined.contains("loading media"), "{joined}");

    // 複数ジョブは先頭ラベル + n 表記。
    app.md_remote_inflight
        .insert("https://example.com/a.png".into());
    let joined: String = crate::ui::status::context_spans(&app)
        .iter()
        .map(|s| s.content.as_ref())
        .collect();
    assert!(joined.contains("+1"), "複数ジョブは +n: {joined}");

    // config off なら表示・ティックとも無効(個別の中央表示は別系統のまま)。
    app.cfg.ui.busy_indicator = false;
    assert!(!app.busy_indicator_active());
    let joined: String = crate::ui::status::context_spans(&app)
        .iter()
        .map(|s| s.content.as_ref())
        .collect();
    assert!(!joined.contains("loading media"), "{joined}");

    // 全ジョブ終了で空へ(=ティックが止まりアイドル負荷ゼロに戻る)。
    app.cfg.ui.busy_indicator = true;
    app.media_loading = false;
    app.md_remote_inflight.clear();
    assert!(app.busy_jobs().is_empty());
    assert!(!app.busy_indicator_active());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn busy_indicator_tracks_ignored_scan() {
    // git ignored スキャン: pending が立っている間 GitScan・現世代の適用で消える。
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

    // 古い世代(4)の結果: 反映せず、pending も消さない。
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

    // 現世代(5)の結果: 反映し、git_ignored_for を設定し、pending を消す。
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
    git(&["branch", "-M", "trunk"]); // 既知の現在ブランチ名にする。
                                     // 5本の feature ブランチ(各々が自分のコミット=自レーン)。
    for n in ["f1", "f2", "f3", "f4", "f5"] {
        git(&["checkout", "-q", "trunk"]);
        git(&["checkout", "-q", "-b", n]);
        std::fs::write(dir.join(format!("{n}.txt")), b"x\n").unwrap();
        git(&["add", "-A"]);
        git(&["commit", "-q", "-m", &format!("{n} c")]);
    }
    git(&["checkout", "-q", "trunk"]); // HEAD=trunk。計6ブランチ。

    let mut cfg = Config::default();
    cfg.ui.graph_max_branches = 3; // 小さい上限。
    let mut app = App::new(dir.canonicalize().unwrap(), cfg).unwrap();
    app.open_git_graph();
    assert!(app.is_git_graph(), "グラフが開く");

    // 凡例: HEAD(trunk)が先頭・上限3まで・残りは hidden。
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

    // パネル: HEAD は先頭かつ常時 ON(外せない)。
    app.git_graph_open_picker();
    assert!(app.is_git_graph_picker(), "パネルが開く");
    let items = app.git_graph_picker_items();
    assert!(items[0].1, "パネル先頭は HEAD(is_current)");
    app.git_graph_picker_sel = 0;
    app.git_graph_picker_toggle(); // HEAD を外そうとしても無効。
    assert!(
        app.git_graph_picker_items()[0].2,
        "HEAD はトグルしても表示のまま"
    );

    // a=全部 → 適用 → 非表示0・凡例6本。
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
    // 追跡済みファイルを1つ作りコミット → その後変更(=Modified)。
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
    // 変更2件(modified + untracked)が一覧に出る。
    assert_eq!(app.git_view_entries().len(), 2, "変更2件");
    // 既定はどちらも未ステージ。
    assert!(
        app.git_view_entries().iter().all(|e| !e.staged),
        "初期は全て未ステージ"
    );

    // 先頭(tracked.txt がパス順で先)をステージ → reload で staged=true になる。
    app.git_view_sel = 0;
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
    // #4: Git ビュー表示中の refresh() で変更一覧(git_view_entries)を取り直す
    // (外部 git ツール復帰や FSEvents で増減した変更を反映)。
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
    // 外部で新規ファイル追加 → refresh で変更一覧が再取得され2件に増える。
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

    // S=全ステージ → 全て staged(新規も含む add -A)。
    app.git_view_stage_all();
    assert!(
        app.git_view_entries().iter().all(|e| e.staged),
        "全ステージ後は全 staged"
    );

    // U=全アンステージ → 全て未ステージ。
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
    // Git ビューから diff を開く: Preview モード・GitDiff・Git ビューは閉じる。
    app.open_git_diff(&target);
    assert!(app.is_git_diff_preview(), "GitDiff プレビューになる");
    assert_eq!(app.mode, Mode::Preview);
    assert!(!app.is_git_view(), "Git ビューは閉じる");
    // diff 行が取得できる(変更ありなので非空)。
    assert!(!app.git_diff_lines().is_empty(), "diff 行が空");
    // C2: 一度引いた diff はパス単位でキャッシュされる(毎フレーム git を叩かない)。
    assert!(app.diff_cache.is_some(), "diff キャッシュが張られる");
    // 作業ツリーが変わりうる refresh(FS イベント/手動更新)でキャッシュを落とす
    // (外部編集に追従し stale な diff を見せない)。
    app.refresh().unwrap();
    assert!(
        app.diff_cache.is_none(),
        "refresh で diff キャッシュが無効化される"
    );
    // Esc 相当: Git ビューへ復帰する。
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
    // x: 破棄確認ダイアログ。
    app.git_diff_start_discard();
    assert!(app.is_dialog() && app.dialog_is_confirm(), "破棄確認が出る");
    // y で確定: ファイルが v1 に戻り、Git ビューへ復帰してクリーンになる。
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
    // git リポジトリでないディレクトリでは開かない(no-op + flash)。
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
    git(&["branch", "feature"]); // 2本目のブランチ
    let mut app = App::new(dir.canonicalize().unwrap(), Config::default()).unwrap();

    // 一覧: feature を含み、カーソルは現在ブランチに乗る。
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

    // 絞り込み: "feat" で feature に絞れる。
    app.git_branch_start_filter();
    for c in "feat".chars() {
        app.git_branch_filter_push(c);
    }
    let filtered = app.git_branch_view();
    assert_eq!(filtered.len(), 1, "絞り込みで1件");
    assert_eq!(filtered[0].name, "feature");
    app.git_branch_filter_clear();
    assert_eq!(app.git_branch_view().len(), names.len(), "解除で全件へ戻る");

    // feature を選んで checkout → 現在ブランチが変わり、一覧は閉じる。
    let fi = app
        .git_branch_view()
        .iter()
        .position(|b| b.name == "feature")
        .unwrap();
    app.git_branch_move(fi as i32 - app.git_branch_sel() as i32);
    app.checkout_selected_branch().unwrap();
    assert!(!app.is_git_branches(), "checkout で一覧は閉じる");
    assert_eq!(app.git_branch(), Some("feature"), "現在ブランチ=feature");

    // 新規ブランチ作成フロー: 入力→submit で作成＆切替(now on "newbr")。
    app.open_git_branches();
    app.start_create_branch();
    for c in "newbr".chars() {
        app.dialog_input_push(c);
    }
    app.dialog_submit().unwrap();
    assert_eq!(app.git_branch(), Some("newbr"), "作成して切替");

    // 削除: 現在ブランチ(newbr)は削除不可・他ブランチ(feature)は削除できる。
    app.open_git_branches();
    // カーソルを現在ブランチ(newbr)に置いて d → ガードで開かない。
    app.start_delete_branch();
    assert!(!app.is_dialog(), "現在ブランチは削除確認を開かない");
    // feature へカーソル移動して削除確認 → y(安全)。
    let fi = app
        .git_branch_view()
        .iter()
        .position(|b| b.name == "feature")
        .unwrap();
    app.git_branch_move(fi as i32 - app.git_branch_sel() as i32);
    app.start_delete_branch();
    assert!(app.is_dialog() && app.dialog_is_confirm(), "削除確認が開く");
    app.dialog_confirm(true).unwrap(); // y=安全削除(-d)
    assert!(
        !app.git_branch_view().iter().any(|b| b.name == "feature"),
        "feature が削除された"
    );

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
    // 公開 API でステージ(commit はステージ済み index を使う)。
    crate::git::stage(&dir, &f).unwrap();

    let canon = dir.canonicalize().unwrap();
    let mut app = App::new(canon.clone(), Config::default()).unwrap();
    app.open_git_view();
    assert!(app.is_git_view());

    // c 相当: コミットメッセージ入力ダイアログを開く。
    app.start_git_commit();
    assert!(app.is_dialog(), "コミット入力ダイアログが出る");
    // internal_mode は Commit を報告する(他の Input と区別できる)。
    assert_eq!(app.internal_mode(), Some(InternalMode::Commit));

    // メッセージを1文字ずつ入力。
    for c in "feat: add a".chars() {
        app.dialog_input_push(c);
    }
    // Enter 相当: 確定 → コミット成功。
    app.dialog_submit().unwrap();
    assert!(!app.is_dialog(), "成功するとダイアログは閉じる");

    // HEAD に そのサマリのコミットが出来ている。
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
    // 空白だけ入力 → 確定しても拒否され、コミットは作られない。
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

// --- git log + コミット詳細 ---------------------------------------------
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
    // 2 コミット作る。
    std::fs::write(dir.join("a.txt"), b"one\n").unwrap();
    sh(&["add", "-A"]);
    sh(&["commit", "-m", "first commit"]);
    std::fs::write(dir.join("a.txt"), b"one\ntwo\n").unwrap();
    sh(&["add", "-A"]);
    sh(&["commit", "-m", "second commit"]);

    let canon = dir.canonicalize().unwrap();
    let mut app = App::new(canon.clone(), Config::default()).unwrap();
    // Git ビューを経由して log を開く(実フローと同じ)。
    app.open_git_view();
    app.open_git_log();
    assert!(app.is_git_log(), "git log が開く");
    assert!(!app.is_git_view(), "log を開くと Git ビューは閉じる");
    assert_eq!(app.git_log_entries().len(), 2, "コミット2件");
    // 新しい順: 先頭が second commit。
    assert_eq!(app.git_log_entries()[0].summary, "second commit");
    assert_eq!(app.internal_mode(), Some(InternalMode::GitLog));

    // 2件目(古い=first commit・root)を選んで詳細を開く → diff 行が非空。
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

    // q 相当: 詳細を閉じると log へ戻る。
    app.close_git_detail();
    assert!(!app.is_git_detail());
    assert!(app.is_git_log(), "詳細を閉じると log へ戻る");
    // log を閉じると Git ビューへ戻る。
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
    init_git_repo(&dir); // コミット無し(unborn)
    let canon = dir.canonicalize().unwrap();
    let mut app = App::new(canon, Config::default()).unwrap();
    app.open_git_log();
    assert!(!app.is_git_log(), "コミットが無ければ log は開かない");
    std::fs::remove_dir_all(&dir).ok();
}

// ============================================================================
// 追加カバレッジ: App の純状態・プレビュースクロール・画像/GIF・リンク・タブ など。
// 既存規約に合わせる(temp_dir + ユニーク名・最後に remove_dir_all で後始末)。
// ============================================================================

/// 小さな PNG をテスト用に生成して書き出す(decode 経路の確認用)。
fn write_png(path: &Path, w: u32, h: u32) {
    let img = image::RgbImage::from_pixel(w, h, image::Rgb([10, 20, 30]));
    image::DynamicImage::ImageRgb8(img).save(path).unwrap();
}

#[test]
fn path_style_next_cycles_and_cycle_path_style_advances() {
    // 純関数の巡回: rel→home→full→rel。
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
    let has_dot = |a: &App| a.entries.iter().any(|e| e.path.ends_with(".secret"));
    assert!(!app.show_hidden && !has_dot(&app), "既定では隠しは出ない");
    app.toggle_hidden().unwrap();
    assert!(
        app.show_hidden && has_dot(&app),
        "トグルで .secret が現れる"
    );
    app.toggle_hidden().unwrap();
    assert!(!app.show_hidden && !has_dot(&app), "もう一度で隠れる");
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
    // マジック GIF8x で判定(拡張子は無関係)。
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
    // 既定キーマップは衝突なし → None。
    assert!(app.keymap_report().is_none(), "既定では報告なし");

    // 衝突1件 + 無効設定1件を仕込むと、件数入りの1行を返す(英語)。
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

    // 日本語でも件数入りで出る。
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
    // ハイライト待ち + プレビュー対象がある状態を作る。
    app.preview_path = Some(dir.join("main.rs"));
    app.hl_pending = true;
    app.hl_warming = false;
    let job = app.take_warm_job().expect("起動対象を1度だけ返す");
    assert_eq!(job.0, "rs", "拡張子");
    assert_eq!(job.1, dir.join("main.rs"));
    // 2度目は warming 中なので None(二重起動を防ぐ)。
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
    app.launch_git_tool(); // プロセスは起動せず、フラグを立てるだけ
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
    app.preview_viewport = 20;
    // 非 windowed: preview_scroll を直接操作。
    app.preview_scroll(5);
    assert_eq!(app.preview_scroll, 5);
    app.preview_scroll(-100);
    assert_eq!(app.preview_scroll, 0, "下限 0 でクランプ");
    app.preview_page(1); // +19 (1行重ね)
    assert_eq!(app.preview_scroll, 19);
    app.preview_half_page(1); // +10
    assert_eq!(app.preview_scroll, 29);
    app.preview_page(-1); // -19
    assert_eq!(app.preview_scroll, 10);
    app.preview_to_top();
    assert_eq!(app.preview_scroll, 0, "to_top で先頭へ");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn preview_hscroll_moves_and_home_end() {
    let dir = std::env::temp_dir().join("konoma_preview_hscroll_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.preview_hscroll(8);
    assert_eq!(app.preview_hscroll, 8);
    app.preview_hscroll(-100);
    assert_eq!(app.preview_hscroll, 0, "下限 0");
    app.preview_hscroll_end();
    assert_eq!(app.preview_hscroll, u16::MAX, "$ は上限(描画側でクランプ)");
    app.preview_hscroll_home();
    assert_eq!(app.preview_hscroll, 0, "0 で行頭");
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
        .entries
        .iter()
        .position(|e| e.path.ends_with("big.txt"))
        .unwrap();
    app.selected = i;
    app.tree_activate().unwrap(); // ファイル → プレビュー(Text=窓読み)
    assert!(app.is_windowed(), "テキストは窓読みモード");
    app.preview_viewport = 10;

    let first_line = |a: &mut App| -> String {
        a.windowed_lines(10, 80)[0]
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect()
    };
    assert!(first_line(&mut app).contains("line 0"), "先頭は line 0");

    // 常時カーソルモデル: 窓内(vh=10)で行カーソルを動かしても、末端に達するまで窓は動かない。
    assert_eq!(app.preview_byte_top, 0);
    app.preview_scroll(5); // カーソル 0→5(まだ可視範囲内)
    assert_eq!(app.preview_cursor_line, 5, "カーソルは 5 行目");
    assert_eq!(app.preview_byte_top, 0, "窓内移動では窓は動かない");
    assert!(
        first_line(&mut app).contains("line 0"),
        "先頭は line 0 のまま"
    );

    // カーソルが下端を超えると窓が追従する(byte_top が前進)。
    app.preview_scroll(5); // カーソル 5→10(下端 vh=10 を超える)
    assert_eq!(app.preview_cursor_line, 10, "カーソルは 10 行目");
    assert!(
        app.preview_byte_top > 0,
        "下端超えで窓が追従(byte_top 前進)"
    );
    assert!(
        first_line(&mut app).contains("line 1"),
        "line 10 が見えるよう先頭は line 1"
    );

    // windowed の preview_to_top はカーソルと窓を先頭へ。
    app.preview_to_top();
    assert_eq!(app.preview_cursor_line, 0);
    assert_eq!(app.preview_byte_top, 0);
    assert_eq!(app.preview_top_line, 0);
    assert!(
        first_line(&mut app).contains("line 0"),
        "to_top で先頭へ戻る"
    );

    // 末尾へ: カーソルは最終行(199)、窓は最終ページ。
    app.preview_to_bottom();
    assert_eq!(app.preview_cursor_line, 199, "to_bottom で最終行へ");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn request_edit_opens_at_windowed_caret_line() {
    // `e` は windowed プレビュー(text/code)のキャレット行でエディタを開く(1始まり)。
    // ツリー編集・非 windowed(装飾 Markdown)は行なし(先頭で開く)。
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

    // ① ツリーからの編集: 行なし。
    let i = app
        .entries
        .iter()
        .position(|e| e.path.ends_with("big.txt"))
        .unwrap();
    app.selected = i;
    app.request_edit();
    let (p, line) = app.take_pending_edit().expect("edit requested");
    assert!(p.ends_with("big.txt"));
    assert_eq!(line, None, "ツリー編集は行を渡さない");

    // ② windowed プレビュー: キャレット行 + 1。
    app.tree_activate().unwrap();
    assert!(app.is_windowed());
    app.preview_viewport = 10;
    app.preview_scroll(12); // キャレット 0→12
    assert_eq!(app.preview_cursor_line, 12);
    app.request_edit();
    let (p, line) = app.take_pending_edit().expect("edit requested");
    assert!(p.ends_with("big.txt"));
    assert_eq!(
        line,
        Some(13),
        "キャレット 12 行目 → エディタは 13 行目(1始まり)"
    );

    // ③ 装飾 Markdown(非 windowed): 描画前(md_cache 未構築)は行なし=先頭で開く。
    app.back_to_tree();
    let j = app
        .entries
        .iter()
        .position(|e| e.path.ends_with("doc.md"))
        .unwrap();
    app.selected = j;
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
        .entries
        .iter()
        .position(|e| e.path.ends_with("code.txt"))
        .unwrap();
    app.selected = i;
    app.tree_activate().unwrap();
    assert!(app.is_windowed(), "テキストは窓読み(行カーソル有効)");
    app.preview_viewport = 10;

    // カーソルを 2 行目へ。まだ非選択。
    app.preview_scroll(2);
    assert_eq!(app.preview_cursor_line, 2);
    assert!(!app.is_preview_visual(), "まだ選択していない");
    assert_eq!(app.surface(), crate::keymap::Surface::PreviewText);

    // V で行選択開始 → 2 行下へ伸ばす(2..=4)。
    app.preview_enter_visual(true);
    assert!(app.is_preview_visual(), "選択モードに入る");
    assert!(app.preview_visual_linewise(), "V は行選択");
    assert_eq!(app.surface(), crate::keymap::Surface::PreviewTextVisual);
    assert_eq!(
        app.internal_mode(),
        Some(crate::app::InternalMode::PreviewVisual)
    );
    app.preview_scroll(2); // カーソル 2→4(anchor=2 固定)
    assert_eq!(
        app.preview_selection(),
        crate::app::PreviewSelection::Line { lo: 2, hi: 4 }
    );

    // 選択テキスト = 論理行 2..=4。
    assert_eq!(app.preview_selection_text(), "line 2\nline 3\nline 4");

    // コピー実行で選択解除(クリップボード可否に依存せず状態遷移を検証)。
    app.preview_copy_selection();
    assert!(!app.is_preview_visual(), "コピー後は選択解除");

    // 逆方向(上へ)選択でも範囲は昇順に正規化される。
    app.preview_to_top(); // カーソル 0
    app.preview_scroll(5); // カーソル 5
    app.preview_enter_visual(true);
    app.preview_scroll(-3); // カーソル 2、anchor=5 → 範囲 2..=5
    assert_eq!(
        app.preview_selection(),
        crate::app::PreviewSelection::Line { lo: 2, hi: 5 }
    );
    assert_eq!(
        app.preview_selection_text(),
        "line 2\nline 3\nline 4\nline 5"
    );

    // Esc 相当(exit)で解除。
    app.preview_exit_visual();
    assert!(!app.is_preview_visual());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn preview_charwise_selection_copies_character_range() {
    let dir = std::env::temp_dir().join("konoma_preview_charwise_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // 各行 "abcdefghij" の 6 行。
    let content = (0..6)
        .map(|_| "abcdefghij".to_string())
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(dir.join("code.txt"), content.as_bytes()).unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.rebuild_tree().unwrap();
    let i = app
        .entries
        .iter()
        .position(|e| e.path.ends_with("code.txt"))
        .unwrap();
    app.selected = i;
    app.tree_activate().unwrap();
    assert!(app.is_windowed());
    app.preview_viewport = 10;

    // 1 行目(line 0)の 2 列目(c)から開始 → l を 3 回で col 2→5(f)。単一行 charwise。
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
    // 文字範囲 [2..=5] = "cdef"(end-inclusive)。
    assert_eq!(app.preview_selection_text(), "cdef");

    // 複数行 charwise: (0,2) から 2 行下・col 3 へ → "cdefghij\nabcdefghij\nabcd"。
    app.preview_scroll(2); // カーソル行 0→2
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

    // 逆方向でも (line,col) 昇順に正規化される。
    app.preview_exit_visual();
    app.preview_to_top();
    app.preview_scroll(3); // 行 3
    app.preview_col_home();
    app.preview_col_move(1); // col 1
    app.preview_enter_visual(false); // anchor=(3,1)
    app.preview_scroll(-1); // 行 2
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
        .entries
        .iter()
        .position(|e| e.path.ends_with("doc.md"))
        .unwrap();
    app.selected = i;
    app.tree_activate().unwrap();

    // 既定は装飾表示: windowed でない＝2D 選択は効かない。R トグル対象ではある。
    assert!(app.is_decorated_kind(), "Markdown は装飾種別");
    assert!(!app.is_windowed(), "装飾表示は windowed でない");
    assert!(!app.is_raw_source());
    assert_eq!(app.surface(), crate::keymap::Surface::PreviewText);

    // R で raw ソース表示へ: windowed 化し、選択が乗る。
    app.toggle_md_raw();
    assert!(app.is_raw_source(), "raw ソース表示に切替");
    assert!(app.is_windowed(), "raw は windowed(選択可)");
    app.preview_viewport = 10;

    // 先頭行 "# Title" を charwise で全選択してコピー内容を確認(生ソースの行/桁一致)。
    app.preview_enter_visual(false); // anchor=(0,0)
    for _ in 0..6 {
        app.preview_col_move(1); // (0,0)→(0,6) = "# Title"(7文字, end-inclusive)
    }
    assert_eq!(app.preview_selection_text(), "# Title");

    // R で装飾表示へ戻す: windowed 解除・選択解除。
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
    // 末尾 → home → right(1つ右) → end(末尾) を確認。中間挿入で位置を観測する。
    app.dialog_cursor_home();
    app.dialog_cursor_right(); // 1
    app.dialog_input_push('X'); // "aXbc"
    assert_eq!(
        app.dialog_view().map(|v| v.2.to_string()),
        Some("aXbc".into())
    );
    app.dialog_cursor_right(); // 末尾を超えない範囲で右へ(クランプ確認のため複数回)
    for _ in 0..10 {
        app.dialog_cursor_right();
    }
    app.dialog_input_push('Z'); // 末尾に付く → "aXbcZ"
    assert_eq!(
        app.dialog_view().map(|v| v.2.to_string()),
        Some("aXbcZ".into())
    );
    // end は末尾へ。home→end→挿入も末尾。
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
    // 1フレームだけ: アニメ扱いにならず advance は false。
    app.set_gif_frames(vec![(
        frame([1, 2, 3]),
        std::time::Duration::from_millis(50),
    )]);
    assert!(!app.is_gif_active());
    assert!(!app.advance_gif_if_due(), "単一フレームは進めない");

    // 2フレーム: 初回 tick は計時開始のみ(進めない)。
    app.set_gif_frames(vec![
        (frame([255, 0, 0]), std::time::Duration::from_millis(50)),
        (frame([0, 255, 0]), std::time::Duration::from_millis(50)),
    ]);
    assert!(app.is_gif_active());
    assert!(!app.advance_gif_if_due(), "最初の tick は計時開始だけ");
    assert_eq!(app.gif_idx, 0);
    // 表示時間を過ぎたことにして次フレームへ進む。
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

#[test]
fn apply_image_resize_err_is_ignored() {
    // エンコード失敗(Err)はクラッシュさせず false を返す(状態も変えない)。
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

    // バックエンド無し: 何も載らない(描画側がテキストへフォールバック)。
    let mut bare = App::new(dir.clone(), Config::default()).unwrap();
    bare.load_image(&png);
    assert!(bare.image_src.is_none(), "picker/tx 無しでは load しない");

    // バックエンドあり: decode_static → image_src に載る(サイズ一致)。
    let mut app = app_with_image();
    app.image_src = None; // app_with_image のダミーを消してから
    app.load_image(&png);
    let img = app.image_src.as_ref().expect("PNG をデコードして載せる");
    assert_eq!((img.width(), img.height()), (20, 10));
    // 画像でないファイルは load しない(image_src は None のまま)。
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

    // md を開いている体にして、相対リンクの基準を a.md にする。
    app.preview_path = Some(base.join("a.md"));
    app.preview_kind = Some(PreviewKind::Markdown(base.join("a.md")));
    app.mode = Mode::Preview;

    // アンカー(#) は、該当見出しが無ければ「見つからない」旨を flash(未対応ではない)。
    // ここでは md_cache 未構築(描画していない)ので anchors は空 → NotFound 扱い。
    app.open_link_target("#section").unwrap();
    assert!(
        app.flash
            .as_deref()
            .unwrap()
            .contains(tr(app.lang, crate::i18n::Msg::AnchorNotFound)),
        "アンカー未一致は見出し無し通知: {:?}",
        app.flash
    );

    // 実在しないローカルパスは NotFound 通知。
    app.preview_path = Some(base.join("a.md"));
    app.open_link_target("does_not_exist.zz").unwrap();
    assert!(
        app.flash
            .as_deref()
            .unwrap()
            .contains(tr(app.lang, crate::i18n::Msg::NotFound)),
        "見つからない: {:?}",
        app.flash
    );

    // 既存ファイルへのリンク → プレビュー対象が b.txt に切り替わる。
    app.preview_path = Some(base.join("a.md"));
    app.open_link_target("b.txt").unwrap();
    assert_eq!(
        app.preview_path
            .as_ref()
            .map(|p| p.file_name().unwrap().to_owned()),
        Some(std::ffi::OsString::from("b.txt")),
        "ファイルリンクで b.txt をプレビュー"
    );

    // ディレクトリへのリンク → root が sub になり Tree へ。
    app.preview_path = Some(base.join("a.md"));
    app.mode = Mode::Preview;
    app.open_link_target("sub").unwrap();
    assert_eq!(app.root, base.join("sub"), "ディレクトリリンクで root=sub");
    assert_eq!(app.mode, Mode::Tree, "ディレクトリは Tree へ戻る");
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
        .entries
        .iter()
        .position(|e| e.path.ends_with("a.txt"))
        .unwrap();
    app.selected = i;
    // Tree: カーソルのエントリ。
    assert_eq!(app.copy_target(), Some(dir.join("a.txt")));
    // Preview: プレビュー対象。
    app.mode = Mode::Preview;
    app.preview_path = Some(dir.join("p.txt"));
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
    // Tree 表示中はルートのディレクトリ名。
    assert_eq!(app.tab_label(active), "proj");
    // 範囲外は空文字。
    assert_eq!(app.tab_label(999), "");
    // Preview 表示中はプレビュー対象のファイル名。
    app.mode = Mode::Preview;
    app.preview_path = Some(dir.join("file.md"));
    assert_eq!(app.tab_label(active), "file.md");
    std::fs::remove_dir_all(dir.parent().unwrap()).ok();
}

// --- git 限定: コミットメタ取得 / コピー / グラフ・詳細スクロール・ピッカー --------

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

    // Tree 面ではコミット無し → None。
    assert!(
        app.current_commit_meta().is_none(),
        "Tree ではコミットメタ無し"
    );

    // log 面: 選択コミット(先頭=second)のメタを返す。
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

    // detail 面: ロード済みの git_detail_meta を使う(古い=first commit を開く)。
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

    // log の選択コミットを Message でコピー: クリップボードの可否に依らず copied/failed のいずれかを flash。
    app.open_git_view();
    app.open_git_log();
    app.git_copy(GitCopyKind::Message);
    let f = app.flash.clone().expect("コピーで flash");
    assert!(
        f.starts_with(copied) || f.starts_with(failed),
        "copied/failed のどちらかを通知: {f:?}"
    );
    assert_ne!(f, no_target, "コミットがあるので no-target ではない");

    // branches 未ロードで git_copy_branch_name → no-target ガードを通る。
    app.close_git_log();
    app.close_git_view();
    app.git_branches = None;
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
    // クリップボード成功なら "copied: <branch>"、失敗でも "copy failed: ..."。いずれもガードは越えている。
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
    app.git_view_sel = 0;
    let target = app.git_view_selected().unwrap();
    app.git_view_stage();
    assert!(
        app.git_view_entries()
            .iter()
            .find(|e| e.path == target)
            .unwrap()
            .staged
    );
    // u = 単一ファイル unstage。
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
    // 確認ダイアログだけ開く(まだ破棄しない=ファイルは v2 のまま)。
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
    // "f" は feature/fixup の2件、"fi" は fixup の1件に絞り込む(部分一致)。
    app.git_branch_filter_push('f');
    assert_eq!(app.git_branch_view().len(), 2, "f で feature/fixup の2件");
    app.git_branch_filter_push('i');
    assert_eq!(app.git_branch_view().len(), 1, "fi で fixup の1件に絞る");
    // backspace で "f" に戻ると再び2件へ広がる(backspace の効果)。
    app.git_branch_filter_backspace();
    assert_eq!(app.git_branch_view().len(), 2, "backspace で2件へ広がる");
    // commit(Enter): 入力を抜けるがクエリは残る(j/k 移動できる)。
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

    // 基準セット: 選択コミット行を基準化 → ラベルが付く。
    assert!(app.git_graph_base_label().is_none(), "初期は基準なし");
    app.git_graph_set_base();
    assert!(
        app.git_graph_base_label().is_some(),
        "set_base で基準ラベルが付く"
    );
    // 解除で消える。
    app.git_graph_clear_base();
    assert!(app.git_graph_base_label().is_none(), "clear_base で消える");

    // コミット詳細を開いてスクロール系を確認。
    app.open_git_graph_detail();
    assert!(app.is_git_detail());
    app.set_git_detail_viewport(5);
    app.set_git_detail_total(100);
    assert_eq!(app.git_detail_scroll(), 0);
    app.git_detail_scroll_by(10);
    assert_eq!(app.git_detail_scroll(), 10, "下へ10");
    app.git_detail_scroll_by(-3);
    assert_eq!(app.git_detail_scroll(), 7, "上へ3");
    app.git_detail_scroll_to(true); // G: 末尾(total-viewport=95)
    assert_eq!(app.git_detail_scroll(), 95, "G で末尾(クランプ)");
    app.git_detail_scroll_to(false); // g: 先頭
    assert_eq!(app.git_detail_scroll(), 0);
    // 横スクロール。
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

    // 移動 + ジャンプ。
    app.git_graph_picker_move(2);
    assert_eq!(app.git_graph_picker_sel(), 2);
    app.git_graph_picker_jump(false);
    assert_eq!(app.git_graph_picker_sel(), n - 1, "末尾へジャンプ");
    app.git_graph_picker_jump(true);
    assert_eq!(app.git_graph_picker_sel(), 0, "先頭へジャンプ");

    // current_only → HEAD(trunk)のみ ON。
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

    // 非 HEAD 行をトグル ON。
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

    // cancel は表示集合を変えずに閉じる。
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

// --- bookmark_actions / file_actions のカバレッジ ---------------------------

#[test]
fn mark_set_state_and_cancel() {
    // `m` の登録待ちのみが Mark 面(`'` は即一覧なので待ち状態を持たない)。
    let dir = std::env::temp_dir().join("konoma_mark_state_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    assert!(!app.is_marking());
    app.start_mark_set();
    assert!(app.is_marking(), "m=登録待ち");
    app.cancel_mark();
    assert!(!app.is_marking(), "cancel_mark で待機解除");
    // 待機していない時の mark_input は無視(flash も出ない)。
    app.flash = None;
    app.mark_input('a');
    assert!(app.flash.is_none());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn bookmark_from_preview_registers_previewed_file() {
    // プレビュー中の m は「表示中のファイル」を登録する(ツリーカーソルではなく)。
    let root = std::env::temp_dir().join("konoma_bm_preview_test");
    let _ = std::fs::remove_dir_all(&root);
    let proj = root.join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    std::fs::write(proj.join("a.txt"), b"a").unwrap();
    std::fs::write(proj.join("b.txt"), b"b").unwrap();
    let proj = proj.canonicalize().unwrap();
    let mut app = App::new(proj.clone(), Config::default()).unwrap();
    app.bookmarks = crate::bookmarks::Bookmarks::with_base(root.join("cfgbase"), &proj);
    // b.txt をプレビューで開くが、ツリーカーソルは a.txt に置いたまま(フォロー/ジャンプ等で乖離する状況)。
    app.enter_preview(&proj.join("b.txt"));
    app.selected = app
        .entries
        .iter()
        .position(|e| e.path.ends_with("a.txt"))
        .unwrap();
    assert_eq!(app.mode, Mode::Preview);
    app.start_mark_set();
    app.mark_input('p');
    assert_eq!(
        app.bookmarks.get('p'),
        Some(proj.join("b.txt")),
        "カーソルの a.txt でなく表示中の b.txt が登録される"
    );
    assert_eq!(app.mode, Mode::Preview, "登録後もプレビューのまま");
    // ツリーに戻れば従来どおりカーソル位置を登録。
    app.back_to_tree();
    app.start_mark_set();
    app.mark_input('q');
    assert_eq!(app.bookmarks.get('q'), Some(proj.join("a.txt")));
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn global_bookmark_display_is_absolute() {
    // グローバルは絶対(~短縮)表示・ローカルは従来の文脈相対表示。
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
    // 既定(confirm_bookmark_overwrite=true): 別パスへの上書きは確認ダイアログを出し、
    // 即座には書き換えない。y(dialog_confirm(true))で上書き・n(false)で元のまま。
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

    // まず a.txt を 'x' に登録(確認は出ない=未使用キー)。
    app.enter_preview(&a);
    app.start_mark_set();
    app.mark_input('x');
    assert_eq!(app.bookmarks.get('x'), Some(a.clone()));
    assert!(app.dialog.is_none(), "未使用キーは確認ダイアログ無し");

    // b.txt を同じ 'x' に登録しようとすると確認ダイアログが開き、まだ上書きしない。
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

    // n=取消: 元のまま。
    app.dialog_confirm(false).unwrap();
    assert!(app.dialog.is_none());
    assert_eq!(app.bookmarks.get('x'), Some(a.clone()), "取消で元のまま");

    // 再度出して y=上書き: b.txt に切り替わる。
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
    // 確認を出さない2ケース: ①同じパスの再登録(上書きにならない) ②confirm_bookmark_overwrite=false。
    let root = std::env::temp_dir().join("konoma_bm_overwrite_skip");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("a.txt"), b"a").unwrap();
    std::fs::write(root.join("b.txt"), b"b").unwrap();
    let root = root.canonicalize().unwrap();
    let a = root.join("a.txt");
    let b = root.join("b.txt");

    // ① 確認 ON でも「同じパスの再登録」は確認を出さず即登録。
    let mut app = App::new(root.clone(), Config::default()).unwrap();
    app.bookmarks = crate::bookmarks::Bookmarks::with_base(root.join("cfgbase1"), &root);
    app.enter_preview(&a);
    app.start_mark_set();
    app.mark_input('x');
    app.start_mark_set();
    app.mark_input('x'); // 同じ a.txt を再登録
    assert!(app.dialog.is_none(), "同一パス再登録は確認不要");
    assert_eq!(app.bookmarks.get('x'), Some(a.clone()));

    // ② confirm_bookmark_overwrite=false なら別パスでも即上書き。
    let mut cfg = Config::default();
    cfg.ui.confirm_bookmark_overwrite = false;
    let mut app = App::new(root.clone(), cfg).unwrap();
    app.bookmarks = crate::bookmarks::Bookmarks::with_base(root.join("cfgbase2"), &root);
    app.enter_preview(&a);
    app.start_mark_set();
    app.mark_input('x');
    app.enter_preview(&b);
    app.start_mark_set();
    app.mark_input('x'); // 別パスだが確認オフ
    assert!(
        app.dialog.is_none(),
        "confirm オフは確認ダイアログを出さない"
    );
    assert_eq!(app.bookmarks.get('x'), Some(b.clone()), "即上書き");

    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn tab_list_switch_close_and_guards() {
    // タブ一覧: T で開くと選択=アクティブ・j/k 巡回・Enter で切替(一覧は閉じる)・
    // w は**選択タブ**を閉じ(アクティブ index を正しく調整)・最後の1枚は拒否 flash。
    let dir = std::env::temp_dir().join("konoma_tab_list_test");
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = App::new(dir.canonicalize().unwrap(), Config::default()).unwrap();
    app.tab_new().unwrap();
    app.tab_new().unwrap(); // 3枚・アクティブ=2(末尾)
    assert_eq!(app.tab_count(), 3);
    assert_eq!(app.active_tab_index(), 2);

    app.toggle_tab_list();
    assert!(app.is_tab_list());
    assert_eq!(app.tab_list_sel(), 2, "開いた時の選択はアクティブタブ");
    app.tab_list_move(1);
    assert_eq!(app.tab_list_sel(), 0, "末尾から wrap");
    app.tab_list_move(-1);
    assert_eq!(app.tab_list_sel(), 2);

    // Enter: 選択タブへ切替+一覧が閉じる。
    app.tab_list_move(1); // → 0
    app.tab_list_activate();
    assert!(!app.is_tab_list(), "切替で一覧が閉じる");
    assert_eq!(app.active_tab_index(), 0);

    // w(選択=非アクティブの末尾タブ)を閉じてもアクティブは維持。
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

    // アクティブより前のタブを閉じたら active index は詰まる。
    app.tab_goto(1);
    app.toggle_tab_list();
    app.tab_list_move(-1); // sel=0 (アクティブ=1)
    app.tab_list_close_selected();
    assert_eq!(app.tab_count(), 1);
    assert_eq!(app.active_tab_index(), 0, "前方を閉じたら index が詰まる");

    // 最後の1枚は閉じられない(flash)。
    app.flash = None;
    app.tab_list_close_selected();
    assert_eq!(app.tab_count(), 1);
    assert!(app.flash.is_some(), "最後の1枚は拒否して flash");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn bookmark_list_jump_delete_and_close() {
    // ブックマークの base をテスト専用にして実 ~/.config を汚さない。
    let root = std::env::temp_dir().join("konoma_bm_list_ops_test");
    let _ = std::fs::remove_dir_all(&root);
    let proj = root.join("proj");
    std::fs::create_dir_all(proj.join("sub")).unwrap();
    std::fs::write(proj.join("f.txt"), b"x").unwrap();
    let base = root.join("cfgbase");
    let proj = proj.canonicalize().unwrap();

    let mut app = App::new(proj.clone(), Config::default()).unwrap();
    app.bookmarks = crate::bookmarks::Bookmarks::with_base(base.clone(), &proj);
    // ローカル 'a'=ディレクトリ sub / 'b'=ファイル f.txt を登録。
    app.bookmarks.set('a', proj.join("sub")).unwrap();
    app.bookmarks.set('b', proj.join("f.txt")).unwrap();
    app.open_bookmark_list();
    assert!(app.is_bookmark_list());
    assert_eq!(app.bookmark_list_items().len(), 2);

    // 先頭(a=sub ディレクトリ)へジャンプ → root が sub になり一覧は閉じる。
    app.bookmark_list_sel = 0;
    app.bookmark_list_jump();
    assert!(!app.is_bookmark_list(), "ジャンプで一覧が閉じる");
    assert_eq!(
        app.root,
        proj.join("sub"),
        "ディレクトリへジャンプで root 変更"
    );

    // 再度開いて削除 → 件数が減り一覧は開いたまま(選択クランプ)。
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
    app.refresh_git_if_needed(); // git_status_for をセット
    assert!(!app.git_has_changes(), "コミット直後はクリーン");
    // 外部で変更してから安い更新のみ(refresh_fs(false) が refresh_git_status_only を呼ぶ)。
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
    // ディレクトリ選択 → その中。
    let si = app
        .entries
        .iter()
        .position(|e| e.path.ends_with("sub"))
        .unwrap();
    app.selected = si;
    assert_eq!(app.op_base_dir(), dir.join("sub"), "dir 選択はその中");
    // ファイル選択 → その親。
    let ai = app
        .entries
        .iter()
        .position(|e| e.path.ends_with("a.txt"))
        .unwrap();
    app.selected = ai;
    assert_eq!(app.op_base_dir(), dir, "file 選択は親ディレクトリ");
    // エントリ無し(選択範囲外) → root。
    app.entries.clear();
    assert_eq!(app.op_base_dir(), app.root, "エントリ無しは root");
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

    // ファイル note.md にカーソル → その場に note copy.md ができ、内容は同一。
    let idx = app
        .entries
        .iter()
        .position(|e| e.path.ends_with("note.md"))
        .unwrap();
    app.selected = idx;
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

    // 2回目 → note copy 2.md(既存 unique_name 準拠の連番)。
    let idx = app
        .entries
        .iter()
        .position(|e| e.path.ends_with("note.md"))
        .unwrap();
    app.selected = idx;
    app.duplicate_selection().unwrap();
    assert!(
        root.join("note copy 2.md").exists(),
        "2回目は note copy 2.md"
    );

    // ディレクトリ sub にカーソル → 兄弟 sub copy/ が再帰複製される(中身ごと)。
    let idx = app
        .entries
        .iter()
        .position(|e| e.path.ends_with("sub") && e.is_dir)
        .unwrap();
    app.selected = idx;
    app.duplicate_selection().unwrap();
    assert!(root.join("sub copy").is_dir(), "sub copy/ が兄弟にできる");
    assert!(
        root.join("sub copy").join("inner.txt").exists(),
        "ディレクトリの中身も再帰複製される"
    );

    std::fs::remove_dir_all(&base).ok();
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
    app.visual_select_scope(true); // 全選択
                                   // 一括リネームのプレビューに入る(複数行のプレビュー)。
    app.start_batch_rename();
    for c in "img_{n}".chars() {
        app.dialog_input_push(c);
    }
    app.dialog_submit().unwrap();
    assert!(app.dialog_is_preview(), "プレビューに遷移");
    let n_lines = app.dialog_preview_view().unwrap().1.len();
    assert!(n_lines >= 3);
    // 上限(行数-1)でクランプ。
    app.dialog_preview_scroll(1000);
    assert_eq!(
        app.dialog_preview_view().unwrap().2,
        n_lines - 1,
        "末尾でクランプ"
    );
    // 下限 0 でクランプ。
    app.dialog_preview_scroll(-1000);
    assert_eq!(app.dialog_preview_view().unwrap().2, 0, "先頭でクランプ");
    app.dialog_cancel();
    std::fs::remove_dir_all(&dir).ok();
}

// --- ui::preview::render のディスパッチ分岐(TestBackend ゴールデン) -------------

#[test]
fn ui_preview_renders_image_via_kitty_path() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    // GIF(2フレーム)は同期エンコードなので、ワーカー無しの TestBackend でも render_image の
    // 画像経路が実ピクセル(kitty 転送列 _G)を吐く。静止画は非同期(app.image が後から届く)。
    let mut app = app_with_kitty();
    app.preview_kind = Some(PreviewKind::Image(PathBuf::from("x.gif")));
    let frame = |c: [u8; 3]| {
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(40, 30, image::Rgb(c)))
    };
    app.gif_frames = vec![
        (frame([255, 0, 0]), std::time::Duration::from_millis(50)),
        (frame([0, 255, 0]), std::time::Duration::from_millis(50)),
    ];
    app.gif_idx = 0;
    app.mode = Mode::Preview;
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
    // kitty graphics 転送列(_G)= 実ピクセルが描かれた証拠。
    assert!(s.contains("_G"), "render_image が kitty 転送列を吐かない");
}

#[test]
fn ui_preview_image_falls_back_when_not_yet_encoded() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    // 静止画 Image だが app.image(エンコード済み protocol)が未着 → ImageUnsupported に降格。
    // render_image の静止画フォールバック分岐を通す。
    let mut app = app_with_kitty();
    app.preview_kind = Some(PreviewKind::Image(PathBuf::from("x.png")));
    app.image_src = Some(std::sync::Arc::new(image::DynamicImage::new_rgb8(20, 10)));
    app.mode = Mode::Preview;
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
    app.mode = Mode::Preview;
    app.preview_path = Some(dir.join("thing.bin"));

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

    // 未対応 → [can not preview: <ext>]。
    app.preview_kind = Some(PreviewKind::CanNotPreview { ext: "bin".into() });
    assert!(
        dump(&mut app).contains("can not preview"),
        "未対応フォールバックが出ない"
    );

    // 動画でサムネ無し(image_src 無し)→ ヒント文言にフォールバック。
    app.preview_kind = Some(PreviewKind::Video(dir.join("v.mp4")));
    let video = dump(&mut app);
    assert!(
        video.contains("video") || video.contains("動画"),
        "動画フォールバックが出ない: {video}"
    );

    // 外部コマンド委譲: テンプレ表示。
    app.preview_kind = Some(PreviewKind::Command {
        path: dir.join("c.xyz"),
        template: "mpv {path}".into(),
        render_as: None,
        detached: true,
    });
    assert!(
        dump(&mut app).contains("command"),
        "コマンド委譲表示が出ない"
    );
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

/// リモート画像ディスクキャッシュは上限を超えたら古いものから削除される(無上限成長の防止)。
/// `.part`(取得中の一時ファイル)は対象外。
#[test]
fn prune_remote_cache_keeps_newest_and_skips_part() {
    let dir = std::env::temp_dir().join("konoma_prune_remote_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // 5 個のキャッシュファイルを古い順の mtime で作る。
    for i in 0..5 {
        let p = dir.join(format!("img{i}"));
        std::fs::write(&p, [i as u8]).unwrap();
        let t = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1000 + i as u64);
        filetime_set(&p, t);
    }
    // 取得中の一時ファイルは残す。
    std::fs::write(dir.join("busy.part"), b"x").unwrap();

    prune_remote_cache(&dir, 2);

    // 新しい 2 個(img3/img4)が残り、古い 3 個は消える。.part は温存。
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
    // image preview and a text/Markdown preview ("画像だけではない").
    use image::RgbImage;
    let dir = std::env::temp_dir().join("konoma_preview_file_vanish_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    // --- Image preview ---
    let pic = dir.join("plot.png");
    RgbImage::new(4, 3).save(&pic).unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    let idx = app
        .entries
        .iter()
        .position(|e| e.path.ends_with("plot.png"))
        .unwrap();
    app.selected = idx;
    app.tree_activate().unwrap();
    assert!(matches!(
        app.preview_kind,
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
        .entries
        .iter()
        .position(|e| e.path.ends_with("doc.md"))
        .unwrap();
    app.selected = idx;
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
    app.preview_kind = Some(app.cfg.resolve_preview(&csv));
    app.preview_path = Some(csv.clone());
    app.mode = Mode::Preview;
    app.load_table();
    (app, csv)
}

#[test]
fn csv_resolves_to_table_preview() {
    let (app, _) = app_with_table();
    assert!(
        matches!(
            app.preview_kind,
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

#[test]
fn table_cursor_moves_and_clamps() {
    let (mut app, _) = app_with_table();
    assert_eq!(app.table_cursor(), (0, 0));
    // 範囲外へ大きく動かしても末尾(row 1, col 2)にクランプ。
    app.table_cursor_move(9, 9);
    assert_eq!(app.table_cursor(), (1, 2));
    // 負方向も先頭(0,0)でクランプ。
    app.table_cursor_move(-9, -9);
    assert_eq!(app.table_cursor(), (0, 0));
    // 行/列ジャンプ。
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
    // カーソルを (row 1, col 1) = "e" に。
    app.table_cursor_move(1, 1);
    assert_eq!(
        app.table_copy_text(TableCopyKind::Cell).as_deref(),
        Some("e")
    );
    // 行コピー = その行をカンマ結合。
    assert_eq!(
        app.table_copy_text(TableCopyKind::Row).as_deref(),
        Some("d,e,f")
    );
    // 列コピー = ヘッダ + 各行の該当列を改行結合。
    assert_eq!(
        app.table_copy_text(TableCopyKind::Column).as_deref(),
        Some("h2\nb\ne")
    );
}

/// テーブルのページ送り: `table_page`=ビューポート1画面ぶん・`table_half_page`=半画面ぶん
/// カーソルを縦移動し、末尾/先頭でクランプする(ページ幅の導出が `table_cursor_move` 委譲の
/// 手前で未テストだった)。
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
    app.preview_kind = Some(app.cfg.resolve_preview(&csv));
    app.preview_path = Some(csv.clone());
    app.mode = Mode::Preview;
    app.load_table();
    // 通常は描画時に決まるビューポート行数をテスト用に固定。
    app.table_viewport_rows = 6;

    assert_eq!(app.table_cursor(), (0, 0));
    app.table_page(1); // 1画面(6行)下へ。
    assert_eq!(app.table_cursor().0, 6, "1ページ = viewport 行ぶん下");
    app.table_half_page(1); // 半画面(3行)下へ。
    assert_eq!(app.table_cursor().0, 9, "半ページ = viewport/2 行ぶん下");
    app.table_page(-1); // 1画面上へ。
    assert_eq!(app.table_cursor().0, 3);
    // 末尾/先頭でクランプ(20 データ行 → 最終 index 19)。
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
    // タブ保存/復元でセルカーソルが保たれ、テーブルが再パースされる。
    let (mut app, _) = app_with_table();
    app.table_cursor_move(1, 2);
    app.tab_new().unwrap(); // 現タブを保存し、新タブへ
    app.tab_cycle(-1); // 元タブへ戻る(load_active でテーブル再パース+カーソル復元)
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

// --- Agent Watch: @参照コピー(③) / 変更フィルタ+ジャンプ(①) / フォローモード(②) ---

#[test]
fn at_ref_is_strictly_relative_to_open_dir() {
    let work = std::env::temp_dir().join("konoma_at_ref_test");
    let _ = std::fs::remove_dir_all(&work);
    let a = work.join("A");
    std::fs::create_dir_all(&a).unwrap();
    // 配下: 起動 dir 名を先頭に付けない(Claude Code の cwd 相対 @参照と一致させる)。
    assert_eq!(
        at_ref_text(&a, &a.join("src").join("main.rs")),
        "@src/main.rs"
    );
    // 外(兄弟): `..` 相対。
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

    // 非選択: キャレット行(1-based)。
    app.preview_cursor_line = 2;
    assert_eq!(
        app.preview_selection_ref_text().as_deref(),
        Some("@notes.txt#L3")
    );
    // linewise 選択 3..=5 行目。
    app.preview_enter_visual(true);
    app.preview_cursor_line = 4;
    assert_eq!(
        app.preview_selection_ref_text().as_deref(),
        Some("@notes.txt#L3-5")
    );
    app.preview_exit_visual();
    // charwise 選択は行スパンへ丸める(2..=4 行目)。
    app.preview_cursor_line = 1;
    app.preview_cursor_col = 1;
    app.preview_enter_visual(false);
    app.preview_cursor_line = 3;
    app.preview_cursor_col = 0;
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
    git_repo_with_commits(&dir); // a.txt をコミット済み
    let canon = dir.canonicalize().unwrap();
    // 変更を作る: a.txt を編集(M) + collapsed になる sub/ 配下に未追跡ファイル(U)。
    std::fs::write(canon.join("a.txt"), b"changed\n").unwrap();
    std::fs::create_dir_all(canon.join("sub")).unwrap();
    std::fs::write(canon.join("sub").join("b.txt"), b"new\n").unwrap();

    let mut app = App::new(canon.clone(), Config::default()).unwrap();
    assert!(!app.changed_filter());
    app.toggle_changed_filter();
    assert!(app.changed_filter(), "変更があるので ON になる");
    let paths: Vec<PathBuf> = app.entries.iter().map(|e| e.path.clone()).collect();
    assert_eq!(
        paths,
        vec![canon.join("a.txt"), canon.join("sub").join("b.txt")],
        "root 配下の変更ファイルのみ・パス順のフラット一覧"
    );
    assert!(app.entries.iter().all(|e| !e.is_dir), "ファイルのみ");

    // 一覧から Enter でプレビュー(絞り込みと同じ動き)。
    app.selected = 0;
    app.tree_activate().unwrap();
    assert!(matches!(app.mode, Mode::Preview));
    assert_eq!(
        app.preview_path.as_deref(),
        Some(canon.join("a.txt").as_path())
    );
    app.back_to_tree();

    // h で通常ツリーへ戻る(root は上げない)。
    app.tree_leave().unwrap();
    assert!(!app.changed_filter());
    assert_eq!(app.root, canon, "変更フィルタ解除であって親移動ではない");
    assert!(
        app.entries.iter().any(|e| e.is_dir),
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
    app.selected = 0; // 既定ソート(dirs_first)では sub が先頭 = 変更ファイル上ではない
    app.jump_changed(1);
    assert_eq!(
        app.entries[app.selected].path,
        canon.join("sub").join("b.txt"),
        "collapsed な sub/ を展開して選択(deep reveal)"
    );
    app.jump_changed(1);
    assert_eq!(
        app.entries[app.selected].path,
        canon.join("a.txt"),
        "末尾の次は先頭へ wrap"
    );
    app.jump_changed(-1);
    assert_eq!(
        app.entries[app.selected].path,
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
    // OFF の間は何もしない。
    app.follow_jump(&dir.join("a.txt"));
    assert!(matches!(app.mode, Mode::Tree));

    app.toggle_follow();
    assert!(app.follow_enabled());
    // root 外・隠しディレクトリ配下・ディレクトリはスキップ(状態不変)。
    app.follow_jump(&outside);
    app.follow_jump(&dir.join(".hidden").join("c.txt"));
    app.follow_jump(&dir.join("sub"));
    assert!(matches!(app.mode, Mode::Tree), "無効ターゲットでは動かない");

    // collapsed な sub/ 配下でも deep reveal してプレビューへ。
    app.follow_jump(&dir.join("sub").join("b.txt"));
    assert!(matches!(app.mode, Mode::Preview));
    assert_eq!(
        app.preview_path.as_deref(),
        Some(dir.join("sub").join("b.txt").as_path())
    );
    // Preview 面のまま別ファイルの変更が来ても追従する。
    app.follow_jump(&dir.join("a.txt"));
    assert_eq!(
        app.preview_path.as_deref(),
        Some(dir.join("a.txt").as_path())
    );

    // ユーザーがキーボードを取ったら解除(flash 付き)。
    app.follow_break();
    assert!(!app.follow_enabled());
    assert!(app.flash.is_some(), "解除は flash で見せる");

    std::fs::remove_dir_all(&dir).ok();
    std::fs::remove_file(&outside).ok();
}

#[cfg(feature = "git")]
#[test]
fn follow_jump_scrolls_to_first_changed_hunk() {
    // 大きいファイルの深部(100行目)だけ変更 → follow で開くと**変更行が窓に入る**(先頭表示だと
    // 変更が画面外で見えない=P1 の回帰防止)。上に3行の文脈・キャレットは変更行。
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
    lines[99] = "line 100 CHANGED".into(); // 1-based 100 行目を変更
    std::fs::write(&big, lines.join("\n")).unwrap();

    let canon = dir.canonicalize().unwrap();
    let mut app = App::new(canon.clone(), Config::default()).unwrap();
    app.cfg.ui.follow_view = "file".into(); // 本テストはファイル表示モードの挙動(既定は diff)
    app.toggle_follow();
    app.follow_jump(&canon.join("big.txt"));
    assert!(matches!(app.mode, Mode::Preview));
    assert_eq!(
        app.preview_top_line, 96,
        "変更行(0-based 99)の3行上が窓の先頭"
    );
    assert_eq!(app.preview_cursor_line, 99, "キャレットは変更行");
    assert!(app.preview_byte_top > 0, "窓はファイル先頭ではない");

    // 未変更ファイル(変更ハンク無し)は従来どおり先頭から。
    // (実運用では follow_jump の前に必ず refresh_fs が走り新規ファイルがツリーに載る=同じ順で呼ぶ)
    let plain = canon.join("plain.txt");
    std::fs::write(&plain, b"p1\np2\n").unwrap();
    app.refresh().unwrap();
    app.follow_jump(&plain);
    assert_eq!(app.preview_path.as_deref(), Some(plain.as_path()));
    assert_eq!(app.preview_top_line, 0);
    std::fs::remove_dir_all(&dir).ok();
}

#[cfg(feature = "git")]
#[test]
fn follow_jump_opens_diff_view_by_default_and_falls_back() {
    // 既定(ui.follow_view="diff")では追跡済み変更ファイルを**全画面 diff** で開く。
    // 未追跡(diff が出せない=全行新規)はファイルプレビューへフォールバック。
    let dir = std::env::temp_dir().join("konoma_follow_diff_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    git_repo_with_commits(&dir); // a.txt をコミット済み
    let canon = dir.canonicalize().unwrap();
    std::fs::write(canon.join("a.txt"), b"one\nCHANGED\n").unwrap();
    std::fs::write(canon.join("fresh.txt"), b"new file\n").unwrap();

    let mut app = App::new(canon.clone(), Config::default()).unwrap();
    assert_eq!(app.cfg.ui.follow_view, "diff", "既定は diff 表示");
    app.toggle_follow();

    // 追跡済み+変更あり → GitDiff プレビュー。
    app.follow_jump(&canon.join("a.txt"));
    assert!(app.is_git_diff_preview(), "変更ファイルは diff で開く");
    assert_eq!(
        app.preview_path.as_deref(),
        Some(canon.join("a.txt").as_path())
    );

    // diff ビュー表示中でも次のファイルへ追従が続く(PreviewGitDiff 面の許可・回帰防止)。
    // 未追跡も file_diff が全行追加の diff を合成できる → diff で開く(一貫)。
    app.follow_jump(&canon.join("fresh.txt"));
    assert!(
        app.is_git_diff_preview(),
        "未追跡も all-added の diff で開く"
    );
    assert_eq!(
        app.preview_path.as_deref(),
        Some(canon.join("fresh.txt").as_path()),
        "diff ビューから次の変更ファイルへ切替わる"
    );
    // q はツリーへ戻る(git ハブ由来ではない)。
    app.close_git_diff();
    assert!(matches!(app.mode, Mode::Tree));

    // 変更の無いコミット済みファイル(diff が空)はファイルプレビューへフォールバック。
    let sh = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .current_dir(&dir)
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?} 失敗");
    };
    std::fs::write(canon.join("clean.txt"), b"committed\n").unwrap();
    sh(&["add", "clean.txt"]);
    sh(&["commit", "-m", "clean file"]);
    app.refresh().unwrap();
    app.follow_jump(&canon.join("clean.txt"));
    assert!(
        !app.is_git_diff_preview(),
        "変更なしはファイル表示へフォールバック"
    );
    assert!(matches!(app.mode, Mode::Preview));
    assert!(app.is_windowed());
    assert_eq!(
        app.preview_path.as_deref(),
        Some(canon.join("clean.txt").as_path())
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[cfg(feature = "git")]
#[test]
fn diff_view_n_switches_changed_files_and_keeps_return_target() {
    // diff ビュー内の n/N: ビューを出ずに次/前の変更ファイルの diff へ(wrap・位置表示・
    // ツリーカーソル同期・q の戻り先も保存)。
    let dir = std::env::temp_dir().join("konoma_diff_nav_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    git_repo_with_commits(&dir); // a.txt をコミット済み
    let canon = dir.canonicalize().unwrap();
    std::fs::write(canon.join("a.txt"), b"CHANGED\n").unwrap();
    std::fs::write(canon.join("b.txt"), b"new b\n").unwrap();
    std::fs::write(canon.join("e.txt"), b"new e\n").unwrap();

    let mut app = App::new(canon.clone(), Config::default()).unwrap();
    app.refresh_git_if_needed(); // 実運用では初回描画が取得済み(位置表示は statuses 由来)
    app.open_git_diff(&canon.join("a.txt"));
    assert_eq!(app.diff_change_position(), Some((1, 3)));

    app.jump_changed(1); // a → b
    assert!(app.is_git_diff_preview(), "diff ビューのまま");
    assert_eq!(
        app.preview_path.as_deref(),
        Some(canon.join("b.txt").as_path())
    );
    assert_eq!(app.diff_change_position(), Some((2, 3)));
    assert_eq!(
        app.entries[app.selected].path,
        canon.join("b.txt"),
        "ツリーカーソルも同期"
    );

    app.jump_changed(1); // b → e
    app.jump_changed(1); // e → a (wrap)
    assert_eq!(
        app.preview_path.as_deref(),
        Some(canon.join("a.txt").as_path())
    );
    app.jump_changed(-1); // a → e (逆順 wrap)
    assert_eq!(
        app.preview_path.as_deref(),
        Some(canon.join("e.txt").as_path())
    );

    // ハブ経由で開いた印(came_from_git_view)は回遊しても保たれ、q でハブへ戻れる。
    app.came_from_git_view = true;
    app.jump_changed(1); // e → a
    assert!(app.came_from_git_view, "戻り先(ハブ)が回遊で失われない");
    app.close_git_diff();
    assert!(app.is_git_view(), "q でハブへ戻る");
    std::fs::remove_dir_all(&dir).ok();
}

#[cfg(feature = "git")]
#[test]
fn follow_diff_n_cycles_only_session_files_and_clears_flash() {
    // フォロー由来の diff の n/N は「追尾セッション中に変わったファイル」だけを回遊する
    // (作業ツリー全体の未コミット変更ではない)。位置表示もセッション基準。
    let dir = std::env::temp_dir().join("konoma_follow_session_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    git_repo_with_commits(&dir); // a.txt をコミット済み
    let canon = dir.canonicalize().unwrap();
    // 未コミット変更を5つ用意(既存の作業に相当) — うちセッションで変わるのは b と d だけ。
    std::fs::write(canon.join("a.txt"), b"CHANGED\n").unwrap();
    for name in ["b.txt", "c.txt", "d.txt", "e.txt"] {
        std::fs::write(canon.join(name), format!("new {name}\n")).unwrap();
    }

    let mut app = App::new(canon.clone(), Config::default()).unwrap();
    app.refresh_git_if_needed();
    app.toggle_follow();
    assert!(app.follow_session.is_empty(), "F ON でセッションは空から");

    // ドレイン相当: セッションへ記録(有効ターゲットのみ true)。
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

    // ジャンプで古い flash(follow: on 等)が消える=フッターが diff ヒントに切り替わる。
    app.flash = Some("stale flash".into());
    app.follow_jump(&canon.join("d.txt"));
    assert!(app.is_git_diff_preview());
    assert!(app.flash.is_none(), "ジャンプ成立で flash を消す");
    assert_eq!(
        app.diff_change_position(),
        Some((2, 2)),
        "位置表示はセッション(2件)基準=全 git 変更(5件)ではない"
    );

    // n はセッション内だけを回遊(wrap)。c/e.txt(セッション外の変更)へは行かない。
    app.jump_changed(1);
    assert_eq!(
        app.preview_path.as_deref(),
        Some(canon.join("b.txt").as_path())
    );
    assert_eq!(app.diff_change_position(), Some((1, 2)));
    app.jump_changed(1);
    assert_eq!(
        app.preview_path.as_deref(),
        Some(canon.join("d.txt").as_path())
    );

    // F を入れ直すとセッションはリセットされる。
    app.toggle_follow();
    app.toggle_follow();
    assert!(app.follow_session.is_empty(), "次の ON で新セッション");

    std::fs::remove_dir_all(&dir).ok();
    std::fs::remove_file(&outside).ok();
}

#[test]
fn collapse_links_folds_table_hidden_targets_in_document_order() {
    // 表セルの「ラベル＋隠しターゲット」ペアが、通常リンク(label (URL) 形式)と**出現順で混ざって**
    // targets に回収されること(decorate_links の zip 整合=順序ズレは全リンクの誤爆になる)。
    // 表リンクはアイコンを付けない(幅を変えない=桁揃え維持)・隠しスパンは表示行から消えること。
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
    let (collapsed, targets) = collapse_links(lines, true); // icons=true(既定)でも表は幅不変
    assert_eq!(
        targets,
        vec!["a.md".to_string(), "b.md".to_string(), "c.md".to_string()],
        "targets は出現順(段落→表→段落)"
    );
    // 表リンクのラベルは "table" のまま(アイコン無し)でリンク様式。
    assert!(
        collapsed.iter().any(|l| l
            .spans
            .iter()
            .any(|sp| sp.content.as_ref() == "table" && is_link_span(sp))),
        "表リンクのラベルがリンク様式で残る"
    );
    // 隠しターゲット span は表示行に残らない。
    assert!(
        collapsed.iter().all(|l| l
            .spans
            .iter()
            .all(|sp| !crate::preview::markdown::is_hidden_link_target(sp))),
        "隠しスパンは除去される"
    );
    // リンク span 総数 == targets 数(decorate_links が順に対応付ける前提)。
    let n_links: usize = collapsed
        .iter()
        .map(|l| l.spans.iter().filter(|sp| is_link_span(sp)).count())
        .sum();
    assert_eq!(n_links, targets.len());
}

#[test]
fn md_row_prefix_matches_full_document_reflow() {
    // 可視範囲スライス描画(md_layout/md_slice)の前提=「行ごとの line_count の総和 = 全文書の
    // line_count」(ratatui の Wrap は行を跨いで状態を持たない)。これが崩れるとスクロール
    // クランプとスライス開始位置が全文書描画とズレるので、多様な内容で固定する。
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
        .entries
        .iter()
        .position(|e| e.path.ends_with("doc.md"))
        .unwrap();
    app.selected = idx;
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
    // 描画等価性: 「可視スライス + 残余スクロール」(md_slice=新実装)の Paragraph 描画が、
    // 「全文書 + グローバルスクロール」(旧実装相当)と(グリフ・スタイルとも)セル単位で一致する。
    // フォーカス反転込み。ui::render への配線は既存 e2e(画面文字列)と REVERSED バッファテストが担保。
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
        .entries
        .iter()
        .position(|e| e.path.ends_with("doc.md"))
        .unwrap();
    app.selected = idx;
    app.tree_activate().unwrap();

    let (iw, ih) = (58u16, 18u16); // 枠の内側相当
    let area = Rect {
        x: 0,
        y: 0,
        width: iw,
        height: ih,
    };
    // キャッシュ確立 → Tab フォーカスを1つ進めて反転の等価性も比較対象に含める。
    let (total, _) = app.md_layout(iw);
    app.md_focus_move(1);
    // 参照: 旧実装相当=フル文書を decorate してそのままグローバルスクロールで描く。
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

// ---- mermaid 画像レンダリング (v0.15 feature) --------------------------------

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
    // media_tx 無し=同期フォールバックで即ラスタ到着。
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

    // ズームイン → 同期経路で即シャープ再ラスタ。ラスタは育つが論理サイズは不変
    // (image_layout が論理で計る=画面上の大きさ・切り出しは動かない)。
    app.image_zoom_by(2.0);
    let after = app.image_src.as_ref().unwrap().dimensions();
    assert!(
        after.0 > before.0 || after.1 > before.1,
        "ズームで密度が上がる: {before:?} -> {after:?}"
    );
    assert_eq!(app.image_logical.unwrap(), logical, "論理サイズは不変");
    assert!(app.image_zoom > 1.9, "ユーザー向けズーム値は保たれる");

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
    // 装飾キャッシュ構築(同期フォールバック=フェンスも即レンダリング)。初回は Loading で
    // 構築→到着で md_cache 無効化→再構築、の2パスを回す。
    app.ensure_md_cache(80);
    app.ensure_md_cache(80);
    let imgs = app.md_images();
    assert_eq!(imgs.len(), 1, "フェンスがインライン図 placement になる");
    assert!(crate::preview::markdown::is_mermaid_fence_url(&imgs[0].url));
    // Tab アイテムに図が載る。
    let has_fence_item = app
        .md_items
        .iter()
        .any(|it| matches!(it.kind, MdItemKind::MermaidFence { .. }));
    assert!(has_fence_item, "図が Tab 巡回に載る");

    // フェンスへフォーカス → Enter → 全画面図(ズーム面)。
    let idx = app
        .md_items
        .iter()
        .position(|it| matches!(it.kind, MdItemKind::MermaidFence { .. }))
        .unwrap();
    app.focused_item = Some(idx);
    app.preview_scroll = 3;
    app.md_activate_focused().unwrap();
    assert!(
        matches!(app.preview_kind, Some(PreviewKind::MermaidFence(0))),
        "全画面フェンスビューへ"
    );
    assert!(app.image_src.is_some(), "図がラスタ化されている");
    assert!(app.is_image_preview());

    // q(back_to_tree)= md へ戻り、スクロール/フォーカスが復元される。
    app.back_to_tree();
    assert!(
        matches!(app.preview_kind, Some(PreviewKind::Markdown(_))),
        "ツリーでなく md プレビューへ戻る"
    );
    assert!(matches!(app.mode, Mode::Preview));
    assert_eq!(app.preview_scroll, 3, "スクロール位置を復元");
    assert_eq!(app.focused_item, Some(idx), "フォーカスを復元");

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
    // テキスト経路(生ソースの安全表示)に本文が残る=内容が欠落しない(原則#3)。
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

/// フェンス図のフォーカスは「キャプション span だけ反転」: 中央寄せインデントの空白まで
/// 反転すると巨大な白バーになる(Ghostty 実機で捕まった回帰)。描画バッファで決定的に確認。
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
    // 2回描画: 1回目で同期レンダ→キャッシュ再構築、2回目で配置込みの描画。
    term.draw(|fr| crate::ui::render(fr, &mut app)).unwrap();
    term.draw(|fr| crate::ui::render(fr, &mut app)).unwrap();
    let idx = app
        .md_items
        .iter()
        .position(|it| matches!(it.kind, MdItemKind::MermaidFence { .. }))
        .expect("フェンスが Tab アイテムに載る");
    app.focused_item = Some(idx);
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
    // キャプション(チップ)は REVERSED、行の右側(枠線のみの領域)は REVERSED でない
    // =行全体が反転する「白バー」の回帰を防ぐ(チップにはパディング空白を含む)。
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

/// フェンス図へフォーカスすると**図ブロック全体**が可視域へスクロールし、フォーカス枠
/// (キャプションをタイトルにした囲み)が画像の外側に描かれる(ユーザー要望 2026-07-17)。
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
    assert_eq!(app.preview_scroll, 0, "前提: 先頭表示・フェンスは画面外");

    // Tab (唯一のアイテム=フェンス) → ブロック全体を見せる位置へスクロール。
    app.md_focus_move(1);
    let caption_line = app.md_items[app.focused_item.unwrap()].line;
    assert!(
        app.preview_scroll as usize >= caption_line.saturating_sub(2),
        "図ブロックの先頭(キャプション)近くまでスクロールする: scroll={} caption={caption_line}",
        app.preview_scroll
    );

    // 再描画でフォーカス枠(┌ 角とタイトル)が画像の外側に出る。
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

/// フェンス図のエンコードは Resize::Scale=予約グリッドまで**拡大も**する(ラスタが小さくても
/// 左上詰めの余白バンドを作らない)。写真は従来どおり Fit=自然サイズ超に拡大しない。
/// 回帰 2026-07-17: Fit(縮小専用)だったため mermaid_rows/幅由来のグリッドがベースラスタより
/// 大きいと図が左上に寄り右・下に空きバンドが出ていた(ユーザー報告「中央に表示されない」)。
#[test]
fn encode_worker_scales_fence_diagrams_up_to_grid() {
    let picker = test_picker(); // font 10x20 / Halfblocks
    let (req_tx, req_rx) = std::sync::mpsc::channel();
    let (res_tx, res_rx) = std::sync::mpsc::channel();
    let h = std::thread::spawn(move || md_encode_worker(picker, req_rx, res_tx));
    let img = std::sync::Arc::new(image::DynamicImage::new_rgba8(200, 100)); // 20x5 セル相当
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
    send("mermaid-fence://cafe"); // フェンス図 → グリッド(40x10)へ拡大
    send("/tmp/photo.png"); // 写真 → 自然サイズ(20x5)のまま
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

/// インライン図の初期サイズは**表示領域にフィット**: 目標行数= min(mermaid_rows,
/// ビューポート-2)。ビューポートが変わると図を含む文書だけ再レイアウトされ、
/// 図の無い文書はリビルドされない(ユーザー要望 2026-07-17「最初は表示領域にフィット」)。
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
    // ビューポート未計測(0)=cap どおり。
    assert_eq!(app.md_images()[0].rows, 24, "既定 cap=24");
    // 低いビューポート → キャプション+マージン込みで収まる高さへ縮む。
    app.preview_viewport = 10;
    app.ensure_md_cache(80);
    assert_eq!(app.md_images()[0].rows, 8, "vp10 → 8 行にフィット");
    // 広いビューポート → cap へ戻る。
    app.preview_viewport = 60;
    app.ensure_md_cache(80);
    assert_eq!(app.md_images()[0].rows, 24, "vp60 → cap 24");

    // 図の無い文書はビューポート変化でリビルドされない(lines のポインタ不変)。
    app.enter_preview(&plain);
    app.ensure_md_cache(80);
    let ptr0 = app.md_cache.as_ref().unwrap().lines.as_ptr();
    app.preview_viewport = 12;
    app.ensure_md_cache(80);
    assert_eq!(
        app.md_cache.as_ref().unwrap().lines.as_ptr(),
        ptr0,
        "図なし文書は vp 変化で再構築しない"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// wrap 有効時、フェンスより上に折返し行があってもオーバーレイ(画像/フォーカス枠)が
/// テキスト層とズレない。回帰 2026-07-17: 配置が論理行・スクロールが visual 行の混同で
/// 画像が折返し分だけ上に描かれ、反転キャプション行に placeholder 行が重なって
/// 「ID 色の一行バー」(Ghostty 実機・色は run 毎に変わる)として露出していた。
#[test]
fn fence_overlay_aligns_with_wrapped_text_layer() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    let dir = std::env::temp_dir().join("konoma_fence_wrap_align_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let md = dir.join("doc.md");
    // 先頭に 1 論理行で端末幅を大きく超える段落 → wrap で visual 行が論理行より数行増える。
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
    // 非フォーカス: テキスト層のキャプション行が見える=視覚座標の ground truth。
    // (旧コードは画像バンドが折返し分上へズレてこの行を覆い、ここで既に落ちる)
    let unfocused = caption_rows(&term);
    assert_eq!(
        unfocused.len(),
        1,
        "キャプション行が1行見える: {unfocused:?}"
    );
    let caption_row = unfocused[0];

    // フェンスへフォーカス → フォーカス枠タイトルが同じ行に乗る(キャプションを覆う)。
    let idx = app
        .md_items
        .iter()
        .position(|it| matches!(it.kind, MdItemKind::MermaidFence { .. }))
        .unwrap();
    app.focused_item = Some(idx);
    term.draw(|fr| crate::ui::render(fr, &mut app)).unwrap();
    let focused = caption_rows(&term);
    assert_eq!(
        focused,
        vec![caption_row],
        "フォーカス枠タイトルがテキスト層キャプションと同じ行(折返し分ズレない)"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// インライン mermaid 図のその場ズーム: +/-(image_zoom_by の二役)がフォーカス中の図に効き、
/// レイアウト(予約セル)は不変・ズーム中は hjkl がパンに化け・フォーカス移動でリセットされる。
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

    // フェンスへフォーカス → +(zoom in)。全画面画像が無いので image_zoom_by は図に作用する。
    let idx = app
        .md_items
        .iter()
        .position(|it| matches!(it.kind, MdItemKind::MermaidFence { .. }))
        .unwrap();
    app.focused_item = Some(idx);
    app.image_zoom_by(2.0);
    assert!(
        (app.fence_zoom_level() - 2.0).abs() < 1e-9,
        "図がズームされる"
    );

    // ズーム中は hjkl がパン(消費=true)・等倍では消費しない。
    assert!(
        app.fence_pan_motion(Motion::Right),
        "ズーム中はパンに化ける"
    );
    assert!(app.fence_center.0 > 0.5, "中心が右へ動く");
    // レイアウトは不変(予約セル数が変わらない=md 上の表示サイズ固定)。
    app.ensure_md_cache(80);
    let imgs = app.md_images();
    assert_eq!(
        (imgs[0].cols, imgs[0].rows),
        (cols0, rows0),
        "表示エリア不変"
    );

    // = でフィットへ・フォーカス移動でもリセット。
    app.image_zoom_reset();
    assert!((app.fence_zoom_level() - 1.0).abs() < 1e-9);
    assert!(
        !app.fence_pan_motion(Motion::Right),
        "等倍では通常スクロールへ"
    );
    app.image_zoom_by(3.0);
    app.md_focus_move(1); // リンクへ移動
    assert!(
        (app.fence_zoom_level() - 1.0).abs() < 1e-9,
        "フォーカス移動でズームはリセット"
    );

    // リンクにフォーカス中の + は no-op(図が非フォーカス)。
    app.image_zoom_by(2.0);
    assert!((app.fence_zoom_level() - 1.0).abs() < 1e-9);

    std::fs::remove_dir_all(&dir).ok();
}

/// fence_crop: 可視率とクランプの純関数検証(比率ベース=再ラスタ後も同じ窓に写像される)。
#[test]
fn fence_crop_clamps_and_scales() {
    // 2倍ズーム(f=0.5)・中央: 中央の半分窓。
    let ((x, y, w, h), c) = fence_crop((800, 400), 0.5, (0.5, 0.5));
    assert_eq!((w, h), (400, 200));
    assert_eq!((x, y), (200, 100));
    assert_eq!(c, (0.5, 0.5));
    // 端へパン: 中心がクランプされ窓は画像内に収まる。
    let ((x, _y, w, _h), c) = fence_crop((800, 400), 0.5, (1.0, 0.5));
    assert_eq!(x + w, 800, "右端で止まる");
    assert!((c.0 - 0.75).abs() < 1e-9, "中心は f/2 でクランプ");
    // 4倍(f=0.25)で密度2倍のラスタ: 同じ比率窓が2倍の px に写る。
    let ((x1, _, w1, _), _) = fence_crop((800, 400), 0.25, (0.5, 0.5));
    let ((x2, _, w2, _), _) = fence_crop((1600, 800), 0.25, (0.5, 0.5));
    assert_eq!((x2, w2), (x1 * 2, w1 * 2));
}

/// 数式のセル寸法(純関数): SVG の em 単位(40/em)→行, アスペクト→桁。display は inline より
/// 背が高い・幅超過はアスペクト維持でクランプ・極端に高い式は 24 行で上限。兄弟の
/// `mermaid_cells`/`fence_crop` はテスト済みだったがこの2関数は未カバーだった。
#[test]
fn math_cells_size_from_em_units() {
    // inline: 1em(uh=40)・アスペクト5:1 → 1行×10桁。
    assert_eq!(math_cells(200, 40, 8, 16, 100, false), (10, 1));
    // display は rows_per_em が大きい(1.5>1.3) → 同じ式でも背が高くなる(inline は上で1行)。
    let (dc, dr) = math_cells(200, 40, 8, 16, 100, true);
    assert!(dr > 1, "display は inline(1行)より背が高い: {dr}");
    assert_eq!((dc, dr), (20, 2));
    // 幅超過: avail=30 でアスペクト維持クランプ(桁が avail に張り付く)。
    let (cc, _cr) = math_cells(1600, 40, 8, 16, 30, false);
    assert_eq!(cc, 30, "幅は avail に張り付く");
    // 極端に高い式(行列など)は 24 行上限で頭打ち。
    let (_bc, br) = math_cells(1200, 1200, 8, 16, 1000, false);
    assert_eq!(br, 24, "行数は 24 で上限");
}

/// インライン画像のセル寸法(純関数): 自然サイズを基準に、幅が余白を超えたら縮小、行が上限を
/// 超えたら更に縮小。**mermaid と違い自然サイズを超えて拡大はしない**(写真をぼかさない)。
#[test]
fn md_image_cells_downscale_but_never_upscale() {
    // 収まる: 自然サイズそのまま(80x160px, 8x16 セル → 10x10)。
    assert_eq!(md_image_cells(80, 160, 8, 16, 100, 100), (10, 10));
    // 横長で幅超過: avail=50 に縮小(高さも比例)。
    let (wc, wr) = md_image_cells(1600, 160, 8, 16, 50, 100);
    assert_eq!(wc, 50, "幅は avail に縮小");
    assert!(wr < 10, "高さも比例縮小: {wr}");
    // 縦長で行超過: max_rows=20 で頭打ち(桁も比例)。
    assert_eq!(md_image_cells(80, 1600, 8, 16, 100, 20), (2, 20));
    // 余白が広くても自然サイズより拡大しない(40x40px は 5x3 のまま)。
    assert_eq!(md_image_cells(40, 40, 8, 16, 1000, 1000), (5, 3));
}

/// `0`=フィット(ズーム中のみ消費・等倍では従来の行頭のまま)＋ `[ui] mermaid_rows` で
/// インライン図の表示高さが変わる(0/不正は既定 24)。
#[test]
fn fence_zero_fits_and_mermaid_rows_config_sizes_diagram() {
    use crate::keymap::Motion;

    let dir = std::env::temp_dir().join("konoma_fence_rows_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let md = dir.join("doc.md");
    std::fs::write(&md, "```mermaid\ngraph TD\n  A --> B\n  B --> C\n```\n").unwrap();

    // rows=10 の設定でロード。
    let mut cfg = Config::default();
    cfg.ui.mermaid_rows = 10;
    let mut app = App::new(dir.clone(), cfg).unwrap();
    app.picker = Some(test_picker());
    app.enter_preview(&md);
    app.ensure_md_cache(80);
    app.ensure_md_cache(80);
    let small = app.md_images()[0].rows;
    assert!(small <= 10, "mermaid_rows=10 で高さが縮む: {small}");

    // 既定(24)では大きくなる(縦長の図なので rows 上限が効く)。
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

    // 0(不正)は既定へフォールバック。
    let mut cfg0 = Config::default();
    cfg0.ui.mermaid_rows = 0;
    let mut app3 = App::new(dir.clone(), cfg0).unwrap();
    app3.picker = Some(test_picker());
    app3.enter_preview(&md);
    app3.ensure_md_cache(80);
    app3.ensure_md_cache(80);
    assert_eq!(app3.md_images()[0].rows, default_rows, "0 は既定 24 扱い");

    // 0=フィット: ズーム中の LineHome は図のリセットとして消費・等倍では消費しない。
    let idx = app2
        .md_items
        .iter()
        .position(|it| matches!(it.kind, MdItemKind::MermaidFence { .. }))
        .unwrap();
    app2.focused_item = Some(idx);
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

/// mermaid_rows は**実表示サイズの目標**: ベースラスタの自然サイズを超えて拡大でき(ベクタ由来
/// なので密度は再ラスタで追従=layout 不変のままシャープ)、テーマは dark 既定+背景透過。
#[test]
fn mermaid_rows_upscales_and_dark_theme_is_transparent() {
    // 純関数: 目標行数へ拡大する(自然サイズ非依存)・幅でクランプ。
    let (c, r) = mermaid_cells(400, 800, 8, 16, 200, 40);
    assert_eq!(r, 40, "目標行数まで拡大");
    assert_eq!(
        c, 40,
        "アスペクト維持(400/800 → 40行×16px=640px 高, 幅320px=40セル)"
    );
    let (c2, r2) = mermaid_cells(1600, 400, 8, 16, 60, 40);
    assert_eq!(c2, 60, "幅上限でクランプ");
    assert!(r2 < 40, "幅クランプに合わせ行数も縮む: {r2}");

    // テーマ: dark 既定は背景 fill="none"(透過)・白背景を持たない。
    let svg = crate::preview::markdown::mermaid_to_svg("graph LR\nA-->B", "dark").unwrap();
    assert!(svg.contains("fill=\"none\""), "背景透過");
    assert!(
        !svg.contains("fill=\"#FFFFFF\""),
        "dark テーマに白背景が無い"
    );

    // 密度追従: mermaid_rows を大きくした表示サイズに合わせ、ensure_md_fence_density が
    // 保持 SVG から高密度に再ラスタする(同期フォールバック)。layout(予約セル)は不変。
    use image::GenericImageView;
    let dir = std::env::temp_dir().join("konoma_mermaid_density_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let md = dir.join("doc.md");
    std::fs::write(&md, "```mermaid\ngraph TD\n  A --> B\n```\n").unwrap();
    let mut cfg = Config::default();
    cfg.ui.mermaid_rows = 60;
    cfg.ui.svg_max_px = 300; // ベースラスタを小さく=密度不足を作る
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
    // 再装飾してもセル数が変わらない(レイアウト固定)。
    app.md_cache = None;
    app.ensure_md_cache(120);
    let p2 = &app.md_images()[0];
    assert_eq!((p2.cols, p2.rows), (p.cols, p.rows), "セル数不変");

    std::fs::remove_dir_all(&dir).ok();
}
/// mermaid_rows=100 のような大きい目標でも placement がその行数まで拡大する(幅が許す限り)。
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

/// kitty placeholder 残骸掃除: **実際に描いた**インライン図が動いたフレームだけ「フル再描画
/// 要求」が立つ(静止中は立たない・図が現れた/消えたフレームで一度立つ)。Ghostty の色付きバー
/// (旧 ID 行の取り残し)対策。回帰 2026-07-17: 署名が生スクロール値+全 placement を含んで
/// いたため、図が**画面外**でも画像入り md はスクロール毎キーでフル clear+二重描画だった。
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

    term.draw(|fr| crate::ui::render(fr, &mut app)).unwrap(); // 初回(Loading→同期レンダ)
    term.draw(|fr| crate::ui::render(fr, &mut app)).unwrap(); // 配置確定(図は下方=画面外)
    let _ = app.take_md_overlay_moved();
    term.draw(|fr| crate::ui::render(fr, &mut app)).unwrap(); // 静止フレーム
    assert!(!app.take_md_overlay_moved(), "静止中は要求しない");

    // 図が**画面外**にしか無い間のスクロールは要求しない(placeholder がグリッドに無い=
    // 掃除する残骸が無い。ここが毎キー clear になっていたのが回帰)。
    app.preview_scroll = app.preview_scroll.saturating_add(3);
    term.draw(|fr| crate::ui::render(fr, &mut app)).unwrap();
    assert!(
        !app.take_md_overlay_moved(),
        "画面外の図しか無い間のスクロールでは要求しない"
    );

    // 末尾まで送って図を画面内へ。出現フレームは要求しない(直前まで何も描いていない=
    // グリッドに掃除すべき旧 placeholder が無い。掃除は「描いていた状態からの変化」のみ)。
    app.preview_scroll = 500; // 描画側が末尾へクランプ
    term.draw(|fr| crate::ui::render(fr, &mut app)).unwrap();
    assert!(!app.take_md_overlay_moved(), "出現フレームは掃除不要");
    term.draw(|fr| crate::ui::render(fr, &mut app)).unwrap();
    assert!(!app.take_md_overlay_moved(), "その後の静止では立たない");

    // 図が見えている間のスクロール → 動いたフレームで立つ。
    app.preview_scroll = app.preview_scroll.saturating_sub(1);
    term.draw(|fr| crate::ui::render(fr, &mut app)).unwrap();
    assert!(
        app.take_md_overlay_moved(),
        "見えている図が動いたら一度フル再描画"
    );
    term.draw(|fr| crate::ui::render(fr, &mut app)).unwrap();
    assert!(!app.take_md_overlay_moved(), "その後の静止では立たない");

    // フォーカスの変化(スクロール無しでも) → 一度立つ(Ghostty の合成乱れをその場で修復)。
    let idx = app
        .md_items
        .iter()
        .position(|it| matches!(it.kind, MdItemKind::MermaidFence { .. }))
        .unwrap();
    app.focused_item = Some(idx);
    term.draw(|fr| crate::ui::render(fr, &mut app)).unwrap();
    assert!(
        app.take_md_overlay_moved(),
        "フォーカス変化でも一度フル再描画"
    );
    term.draw(|fr| crate::ui::render(fr, &mut app)).unwrap();
    assert!(!app.take_md_overlay_moved(), "フォーカス静止では立たない");

    // プレビューを離れる(図が消える) → 一度立つ。
    app.back_to_tree();
    term.draw(|fr| crate::ui::render(fr, &mut app)).unwrap();
    assert!(app.take_md_overlay_moved(), "図が画面から消えたら掃除");

    std::fs::remove_dir_all(&dir).ok();
}

/// タブ切替で .mmd(画像モード)が画像のまま復元される。回帰 2026-07-18: load_active の媒体
/// 復元分岐に Mermaid 系が無く、さらに clear_image が preview_media_mtime を残すため
/// reload_media_if_changed も「未変更」と誤判定して塞がれ、タブ復帰でテキスト罫線図に
/// git グラフの装飾状態(基準ピン・表示ブランチ・優先順・凡例)はタブ毎に保存/復元される。
/// 回帰 2026-07-18: git_graph(GraphRow)は保存されるのにこれら派生状態は非保存で、別タブで
/// 基準/表示を変えて戻ると凡例や `base:` タイトルが他タブのまま残っていた(load_active は
/// グラフを再構築しない)。新規タブへは持ち越さず、戻れば復元される。
#[test]
fn git_graph_decoration_state_is_per_tab() {
    let dir = std::env::temp_dir().join("konoma_graph_pertab_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.txt"), "x").unwrap();

    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    // タブ0のグラフ装飾状態を模擬(実グラフ構築なしで派生フィールドを直接セット)。
    app.git_graph_base = Some("release/1.0".into());
    app.git_graph_base_label = Some("release/1.0".into());
    app.git_graph_visible = ["main".to_string(), "dev".to_string()]
        .into_iter()
        .collect();
    app.git_graph_order = vec!["release/1.0".into(), "main".into(), "dev".into()];
    app.git_graph_hidden = 3;

    app.tab_new().unwrap(); // タブ1(素の Tree)
    assert!(app.git_graph_base.is_none(), "新規タブへ基準ピンは漏れない");
    assert!(
        app.git_graph_visible.is_empty(),
        "新規タブへ表示ブランチは漏れない"
    );
    assert!(app.git_graph_order.is_empty(), "新規タブへ優先順は漏れない");
    assert_eq!(app.git_graph_hidden, 0, "新規タブの hidden は 0");

    // タブ1で別の装飾状態にしてから戻る。
    app.git_graph_base = Some("hotfix".into());
    app.tab_goto(0);
    assert_eq!(
        app.git_graph_base.as_deref(),
        Some("release/1.0"),
        "タブ0の基準ピンが復元される"
    );
    assert_eq!(app.git_graph_hidden, 3, "タブ0の hidden が復元される");
    assert!(
        app.git_graph_visible.contains("main") && app.git_graph_visible.contains("dev"),
        "タブ0の表示ブランチが復元される"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// 無通知劣化していた(裏で別メディアを開いた時だけ偶然復活する非決定挙動)。
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

    app.tab_new().unwrap(); // タブ2(素のツリー)へ
    assert!(app.image_src.is_none(), "新規タブに画像は無い");
    app.tab_cycle(1); // タブ1へ復帰
    assert!(
        app.image_src.is_some(),
        "タブ復帰で .mmd が画像として再ロードされる"
    );
    assert!(app.is_image_preview(), "IMAGE 面のまま");

    std::fs::remove_dir_all(&dir).ok();
}

/// 全画面フェンス表示のままタブを離れて戻っても、図が画像のまま復元される
/// (旧コードは偽の「cannot render」案内に落ちていた)。
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
    app.focused_item = Some(idx);
    app.md_activate_focused().unwrap();
    assert!(
        matches!(app.preview_kind, Some(PreviewKind::MermaidFence(_))),
        "全画面フェンスへ遷移"
    );
    assert!(app.image_src.is_some(), "前提: 全画面の図がラスタ化される");

    app.tab_new().unwrap();
    app.tab_cycle(1);
    assert!(
        matches!(app.preview_kind, Some(PreviewKind::MermaidFence(_))),
        "タブ復帰でも全画面フェンスのまま"
    );
    assert!(app.image_src.is_some(), "図が画像として再ロードされる");

    std::fs::remove_dir_all(&dir).ok();
}

/// 上流に画像化できないフェンス(テキスト降格)があっても、Tab フォーカスの Enter は
/// **その図のソース**を開く。回帰 2026-07-18: 序数を「描画済み番兵の計数」で導出していた
/// ため、番兵を出さないフェンス(失敗/loading)の分だけソース再抽出とずれ、別の図
/// (たいてい失敗した方=偽エラー表示)が全画面に開いていた。
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

    // 描画されたフェンスは 1 つ(壊れた方はテキスト降格)で、その序数は**ソース順の 1**。
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
    app.focused_item = Some(idx);
    app.md_activate_focused().unwrap();
    assert!(
        matches!(app.preview_kind, Some(PreviewKind::MermaidFence(1))),
        "Enter は**正しい方**のフェンスを開く: {:?}",
        app.preview_kind
    );
    assert!(
        app.mermaid_fence_code(&md, 1)
            .is_some_and(|c| c.contains("graph LR")),
        "序数 1 の再抽出は有効な図のソース"
    );
    assert!(app.image_src.is_some(), "全画面の図がラスタ化される");

    std::fs::remove_dir_all(&dir).ok();
}

/// 編集で内容が変わったフェンスの旧キャッシュエントリは、次の再装飾で回収される。
/// 回帰 2026-07-18: キーは内容ハッシュで evict は enter_preview(ファイル切替)のみだった
/// ため、agent-watch の反復編集で旧ラスタ/protocol がファイル切替まで単調成長していた。
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

    // 外部編集(内容変更)→ 再読込・再装飾。
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

/// Tab フォーカスとインライン図のズーム/パンはタブ毎に複製される: 別タブへ漏れず、
/// 戻れば復元される(image_zoom が TabState 保存されるのと同格)。
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
    app.focused_item = Some(idx);
    app.image_zoom_by(2.0); // 全画面画像なし+フェンスフォーカス中 → fence_zoom に作用
    assert!(app.fence_zoom > 1.9, "前提: タブ1でその場ズーム中");

    app.tab_new().unwrap(); // タブ2
    assert_eq!(app.focused_item, None, "新規タブへフォーカスは漏れない");
    assert!(app.fence_zoom < 1.001, "新規タブへズームは漏れない");
    app.enter_preview(&b);
    app.ensure_md_cache(100);
    assert_eq!(app.focused_item, None, "タブ2の文書は未フォーカスのまま");

    app.tab_cycle(1); // タブ1へ復帰
    assert_eq!(app.focused_item, Some(idx), "タブ1のフォーカスが復元される");
    assert!(app.fence_zoom > 1.9, "タブ1のズームが復元される");

    std::fs::remove_dir_all(&dir).ok();
}

/// 全画面フェンスからの `q` 復帰はインライン画像キャッシュを温存する(同一ファイル再入)。
/// 回帰 2026-07-18: enter_preview が無条件で md_image_cache を捨てるため、復帰のたび全フェンス
/// を再レンダし、Loading の縮退レイアウト 1 描画で復元スクロール/フォーカスがクランプされ得た。
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
    app.focused_item = Some(idx);
    app.md_activate_focused().unwrap(); // Enter → 全画面
    app.back_to_tree(); // q → md へ復帰
    assert!(
        matches!(app.preview_kind, Some(PreviewKind::Markdown(_))),
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
    assert_eq!(app.focused_item, Some(idx), "フォーカスも復元される");

    std::fs::remove_dir_all(&dir).ok();
}

/// エンコード失敗(protocol=None)の結果でも enc_inflight が解除され、キーが記録されて同じ
/// 要求を毎フレーム再試行しない。一度も成功していない Full の失敗のみテキスト降格。
/// 回帰 2026-07-18: ワーカーが Err の結果を握り潰し、enc_inflight が永久 true=その画像の
/// 再エンコード停止+md_images_loading() 恒久 true(busy スピナー+16ms ポーリング常駐)だった。
#[test]
fn failed_encode_clears_inflight_and_degrades_safely() {
    let dir = std::env::temp_dir().join("konoma_encode_fail_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();

    // Full 失敗・protocol 未保持 → failed へ降格し busy は解ける。
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

    // Zoom 失敗は旧表示のまま(failed にしない)・キーだけ記録。
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

/// 数式インライン画像 `math://…` が予約行だけで**空白**になっていた(ユーザー報告 2026-07-20)。
/// 真因: ensure_md_image / md_image_proto が mermaid フェンスの合成キーしか特別扱いせず、`math://`
/// を実ファイルパスとして resolve_md_image_path に渡していた=常に None を返し、**エンコードを一度も
/// 要求せず** protocol も返らない=予約セルが空白のまま。is_synthetic_md_url でフェンスと数式の両方を
/// 合成キーとして扱い、URL をそのままキャッシュキーにする。ここでは「デコード済みの数式に対して
/// ensure_md_image がエンコード要求を積むこと」を検証する(旧実装は早期 return で何も積まない=空白)。
#[test]
fn math_inline_image_requests_encode_via_synthetic_key() {
    use crate::preview::markdown::{is_synthetic_md_url, math_url};
    // 合成キーの分類: フェンスと数式は合成、実ファイル/リモートは実ファイル。
    assert!(is_synthetic_md_url(&math_url("E=mc^2", false)));
    assert!(is_synthetic_md_url("mermaid-fence://feedfeed"));
    assert!(!is_synthetic_md_url("figure.png"));
    assert!(!is_synthetic_md_url("https://example.com/x.svg"));

    let dir = std::env::temp_dir().join("konoma_math_encode_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();

    // エンコードワーカーの受け口を張る(ensure_md_image はここへ MdEncodeRequest を積む)。
    let (tx, rx) = std::sync::mpsc::channel();
    app.attach_md_encoder(tx);

    // ensure_math_render→apply_md_image 後の状態を再現: decoded + layout_px を持つ合成キーのエントリ。
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

    // 予約行(cols=12, rows=2)全表示。数式はエンコードを要求しなければならない(旧実装は空白)。
    app.ensure_md_image(&url, 12, 2, 0, 2);
    let req = rx
        .try_recv()
        .expect("数式はエンコードを要求する(math:// を合成キーとして解決)");
    assert_eq!(req.path, key, "math:// を実ファイルでなく合成キーで解決");
    assert!(matches!(req.key, MdEncodeKey::Full { cols: 12, rows: 2 }));

    std::fs::remove_dir_all(&dir).ok();
}

/// 全画面ベクタのシャープ再ラスタは同時 1 本(inflight ガード)。回帰 2026-07-18: ガードが
/// 無く、`+` のキーリピートでジョブ完了前に同じ再ラスタを十数本 spawn し得た
/// (1 本 ~数百 ms・過渡 ~128MiB。インライン側 reraster_inflight との非対称)。
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

    // inflight 中はズームしても再ラスタを出さない(ラスタ寸法が変わらない)。
    app.vector_reraster_inflight = true;
    app.image_zoom_by(2.0);
    let held = app.image_src.as_ref().unwrap().dimensions();
    assert_eq!(held, before, "inflight 中は再ラスタしない");

    // 解除後の次のズーム操作で追いつく(同期経路=即適用)。
    app.vector_reraster_inflight = false;
    app.image_zoom_by(1.1);
    let after = app.image_src.as_ref().unwrap().dimensions();
    assert!(
        after.0 > before.0 || after.1 > before.1,
        "解除後は最新ズームへ収束: {before:?} -> {after:?}"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// 空の ```mermaid フェンス(書きかけ)は「loading」行に固まらずテキスト経路へ落ち、
/// 有効なフェンスのソース序数もずれない。回帰 2026-07-18: slot の空文字が「抽出 ON か」の
/// probe と区別できず、probe 応答の Loading を拾って永久に loading 表示だった。
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
    // 有効なフェンスは描画され、序数はソース順の 1(空フェンスが 0 を占有)。
    let placements = app.md_images();
    assert_eq!(placements.len(), 1);
    assert_eq!(placements[0].fence_ord, Some(1), "序数はソース順");

    std::fs::remove_dir_all(&dir).ok();
}

/// ズーム中でも図が**画面外**ならパンはキーを奪わない。回帰 2026-07-18: 消費条件が
/// 「ズーム中+フォーカス有り」だけだったため、Ctrl-f/G で図を画面外へ送ると hjkl/j/k が
/// 見えない図のパンに食われ、キーが死んだように見えた。
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
    app.focused_item = Some(idx);
    app.image_zoom_by(2.0);
    assert!(app.fence_pan_motion(M::Down), "可視+ズーム中はパンを消費");

    // 図を画面外へ(末尾へスクロール) → もう消費しない=文書スクロールに戻る。
    app.preview_scroll = 500;
    term.draw(|fr| crate::ui::render(fr, &mut app)).unwrap();
    assert!(
        !app.fence_pan_motion(M::Down),
        "画面外の図のパンにキーを食わせない"
    );
    assert!(
        !app.fence_pan_motion(M::LineHome),
        "0 も奪わない(通常の行頭へ)"
    );
    assert!(app.fence_zoom > 1.9, "ズーム状態自体は保持(戻れば再開)");

    std::fs::remove_dir_all(&dir).ok();
}

/// キャッシュに無いキーの decode 結果(ファイル切替/prune 後に遅れて届いた陳腐化結果)は
/// 捨てられる: エントリを復活させず、現文書の md_cache も無効化しない。
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

/// 外部でファイルが**縮小**されたとき、窓読みプレビューのスクロール位置が新しい EOF を越えたまま
/// 残らないこと。エージェントがログを truncate する / ファイルを書き直す、という Agent Watch の
/// 日常操作で起きる。`preview_byte_top` は旧ファイルの末尾ページを指したままになり得るので、
/// 再読込→描画の経路でクランプされる必要がある。
#[test]
fn windowed_preview_clamps_scroll_when_file_shrinks_externally() {
    let dir = unique_tmp("konoma_shrink_clamp");
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("log.txt");
    let long: String = (1..=200).map(|i| format!("line {i}\n")).collect();
    std::fs::write(&f, &long).unwrap();

    let mut app = App::new(dir.canonicalize().unwrap(), Config::default()).unwrap();
    app.selected = app
        .entries
        .iter()
        .position(|e| e.path.ends_with("log.txt"))
        .unwrap();
    app.tree_activate().unwrap();
    app.preview_viewport = 5;
    // 末尾へ = byte_top は 200 行ファイルの最終ページ(短縮後のファイル長より大きい)。
    app.preview_to_bottom();
    let _ = app.windowed_lines(5, 80);
    let deep_top = app.preview_byte_top;
    assert!(deep_top > 0, "末尾へスクロールできている");

    // 外部で 3 行に切り詰め(エージェントの truncate 相当)。
    std::fs::write(&f, "a\nb\nc\n").unwrap();
    let new_len = std::fs::metadata(&f).unwrap().len();
    assert!(deep_top > new_len, "旧 top は新しい EOF を越えている(前提)");

    // FS イベント相当の再読込 → 描画。
    app.reload_preview();
    let lines = app.windowed_lines(5, 80);

    assert!(
        app.preview_byte_top <= new_len,
        "縮小後もスクロール位置が EOF を越えている: top={} len={new_len}",
        app.preview_byte_top
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

/// FS イベントでのプレビュー再読込が、**変更パスに応じて**行われること。
///
/// 目的: エージェントが `src/` を書き換え続けている間、無関係な `docs/foo.md` の装飾を毎イベント
/// 作り直さない(Markdown 全文の再レンダは重い)。一方で、表示中ファイル自身の変更・`.git` のみの
/// 変更(= 空リストで届く)・変更パス不明は必ず追従する。
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
    app.preview_kind = Some(app.cfg.resolve_preview(&md));
    app.preview_path = Some(md.clone());
    app.mode = Mode::Preview;
    app.preview_viewport = 20;

    // 装飾キャッシュを作る。
    let _ = app.decorated_lines(80);
    assert!(app.md_cache.is_some(), "装飾キャッシュができている");

    // (1) 無関係なファイルの変更 → キャッシュは保持される(再レンダしない)。
    app.refresh_fs_changed(false, std::slice::from_ref(&other))
        .unwrap();
    assert!(
        app.md_cache.is_some(),
        "無関係な src/lib.rs の変更で装飾キャッシュを捨てている"
    );

    // (2) 表示中ファイル自身の変更 → 再読込する(外部エディタ/エージェントの編集に追従)。
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

    // (3) 空リスト(= `.git` のみの変更 / 不明) → 安全側で必ず再読込する。
    let _ = app.decorated_lines(80);
    assert!(app.md_cache.is_some());
    app.refresh_fs_changed(false, &[]).unwrap();
    assert!(
        app.md_cache.is_none(),
        "変更パス不明(.git のみ等)は安全側で再読込すべき"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// 表内検索: `/` で一致セルへカーソルが飛び、`n`/`N` が読み順で巡回(端は wrap)すること。
/// 大文字小文字は無視し、一致セルは描画側が引ける集合にも載る。
#[test]
fn table_search_moves_cursor_through_matching_cells() {
    let dir = unique_tmp("konoma_table_search");
    std::fs::create_dir_all(&dir).unwrap();
    let csv = dir.join("t.csv");
    // 3 列 × 4 行。"apple" は (0,0) / (1,2) / (3,1) の 3 セルに出る(大小混在)。
    std::fs::write(
        &csv,
        "h1,h2,h3\napple,x,y\nq,r,APPLE\nz,z,z\nw,Apple pie,v\n",
    )
    .unwrap();
    let mut app = App::new(dir.canonicalize().unwrap(), Config::default()).unwrap();
    let csv = dir.canonicalize().unwrap().join("t.csv");
    app.preview_kind = Some(app.cfg.resolve_preview(&csv));
    app.preview_path = Some(csv);
    app.mode = Mode::Preview;
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

    // Esc 相当: 解除で一致集合も消える(描画に取り残さない)。
    app.search_clear();
    assert!(!app.table_cell_is_hit(0, 0), "解除で一致ハイライトが消える");
    assert_eq!(app.search_status(), None);

    std::fs::remove_dir_all(&dir).ok();
}

/// 一致が無いときは flash を出し、カーソルを動かさない(表を見失わせない)。
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

/// 装飾 Markdown のプレビュー内検索。ソースではなく**画面に出ている装飾行**を検索し、
/// 一致行を可視域へスクロールする(`R` の生ソースに切替えない)。`n`/`N` は文書順で巡回。
#[test]
fn decorated_markdown_search_scrolls_to_matches() {
    let dir = unique_tmp("konoma_md_search");
    std::fs::create_dir_all(&dir).unwrap();
    let md = dir.join("doc.md");
    // 見出し + 詰め物 + 遠く離れた 2 箇所の "needle"。
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
    app.preview_kind = Some(app.cfg.resolve_preview(&md));
    app.preview_path = Some(md);
    app.mode = Mode::Preview;
    app.preview_viewport = 10;
    let _ = app.md_layout(80); // 装飾キャッシュを張る(実描画と同じ経路)

    // 装飾 md でも検索を開始できる(以前は「コード/テキストのみ」と拒否していた)。
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
    let first_scroll = app.preview_scroll;
    assert!(first_scroll > 0, "1 件目まで下へスクロールした");

    app.search_next(1);
    let second_scroll = app.preview_scroll;
    assert!(
        second_scroll > first_scroll,
        "n で後方の一致へさらにスクロール: {first_scroll} → {second_scroll}"
    );
    app.search_next(1);
    assert_eq!(app.preview_scroll, first_scroll, "wrap して 1 件目へ戻る");

    // 一致が可視域に入り、かつ強調されている(現在の一致=オレンジ背景)。
    let (lines, _) = app.md_slice(app.preview_scroll, 10);
    let hit = lines.iter().any(|l| {
        l.spans
            .iter()
            .any(|s| s.content.contains("needle") && s.style.bg.is_some())
    });
    assert!(hit, "一致箇所が背景色で強調されている");

    // 一致なしはカーソル位置を動かさず flash で知らせる。
    let before = app.preview_scroll;
    app.start_search();
    for c in "zzz-absent".chars() {
        app.search_input_push(c);
    }
    app.search_commit();
    assert_eq!(app.preview_scroll, before, "一致なしでスクロールしない");
    assert_eq!(app.search_status(), None);

    std::fs::remove_dir_all(&dir).ok();
}

/// kitty 端末では prepare_image が **端末リサイズ(fit ズームで crop 不変・領域だけ変化)**でも
/// KittyImage を作り直すこと。作り直さないと、リサイズ後も画像が旧サイズのまま残る(回帰)。
#[test]
fn kitty_image_rebuilds_on_terminal_resize_at_fit() {
    let dir = std::env::temp_dir().join("konoma_kitty_resize_test");
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = App::new(dir, Config::default()).unwrap();
    // viewport より**大きい**ソース(4000x3000px=400x150セル @ font10x20)。fit で viewport に
    // 縮小されるので、端末リサイズ=表示サイズ変化=穴が顕在化する(小画像は自然サイズで不変)。
    app.image_src = Some(std::sync::Arc::new(image::DynamicImage::new_rgb8(
        4000, 3000,
    )));
    app.preview_kind = Some(PreviewKind::Image(PathBuf::from("x.png")));
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    Box::leak(Box::new(rx));
    let mut picker = ratatui_image::picker::Picker::halfblocks();
    picker.set_protocol_type(ratatui_image::picker::ProtocolType::Kitty);
    app.attach_image_backend(picker, tx);
    assert!(app.uses_kitty_image(), "kitty 経路が有効");

    // 最初の描画準備: fit で viewport(200x40)に収まる KittyImage。
    let r1 = app.prepare_image(inner(200, 40)).unwrap();
    let size1 = app.kitty_image_ref().map(|k| k.cell_size());
    assert_eq!(size1, Some((r1.width, r1.height)), "表示セルサイズで構築");

    // 同じ領域で再度 prepare しても作り直さないこと(静止画=毎フレーム構築しない)を、
    // **観測可能な不変量**で判定する: render は transmitted を立て、以後 o=z を再送しない。
    // 作り直せば transmitted が false に戻り o=z が再送される。ポインタ比較はフィールドの
    // アドレスが不変なので作り直しを検出できない(レビュー指摘)。
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
    app.prepare_image(inner(200, 40)).unwrap(); // 領域不変
    assert!(
        !render_syms(&app, area1).contains("o=z"),
        "領域不変なら作り直さない=転送を再送しない"
    );

    // 端末が大きくなった(crop は fit のまま (0,0,sw,sh) 不変・領域だけ拡大)→ 作り直す。
    let r2 = app.prepare_image(inner(300, 60)).unwrap();
    let size2 = app.kitty_image_ref().map(|k| k.cell_size());
    assert_ne!(size1, size2, "リサイズで表示サイズが変わる");
    assert_eq!(
        size2,
        Some((r2.width, r2.height)),
        "新しい表示サイズで再構築"
    );
    // 作り直した=新しい KittyImage は未転送 → o=z を再送する。
    let area2 = ratatui::layout::Rect::new(0, 0, r2.width.min(50), r2.height);
    assert!(
        render_syms(&app, area2).contains("o=z"),
        "リサイズ後の新 KittyImage は転送を送る"
    );
}

/// 非 kitty 端末(halfblocks/sixel/iterm2)では kitty 経路を使わず、従来の image(ThreadProtocol)を使う。
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

/// タブ切替で表内検索の一致ハイライトが別タブに漏れないこと。`table_search_hits` は `search_matches`
/// から導出される描画用集合だが TabState に無く、load_active が復元しないと前タブの座標が居残り、
/// 表 renderer(`table_cell_is_hit` を無条件参照)が別タブのセルを誤って強調する(レビュー指摘の複製漏れ)。
#[test]
fn table_search_hits_do_not_leak_across_tabs() {
    let dir = unique_tmp("konoma_tab_table_search");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.csv"), "h1,h2\nfoo,bar\nfoo,baz\n").unwrap();
    std::fs::write(dir.join("b.csv"), "h1,h2\nq,w\ne,r\n").unwrap();
    let root = dir.canonicalize().unwrap();

    // タブ0: a.csv を開いて "foo" を検索(一致セルができる)。
    let mut app = App::new(root.clone(), Config::default()).unwrap();
    let a = root.join("a.csv");
    app.preview_kind = Some(app.cfg.resolve_preview(&a));
    app.preview_path = Some(a);
    app.mode = Mode::Preview;
    app.load_table();
    app.start_search();
    for c in "foo".chars() {
        app.search_input_push(c);
    }
    app.search_commit();
    assert!(app.table_cell_is_hit(0, 0), "タブ0 で foo が一致");

    // タブ1を新規作成(空タブ)し、そこで b.csv を検索なしで開く。
    app.tab_new().unwrap();
    let b = root.join("b.csv");
    app.preview_kind = Some(app.cfg.resolve_preview(&b));
    app.preview_path = Some(b);
    app.mode = Mode::Preview;
    app.load_table();
    assert!(
        !app.table_cell_is_hit(0, 0),
        "新タブ(検索なし)に前タブの一致が漏れていない"
    );

    // タブ0へ戻る → 一致が復元され、タブ1へ再度切替 → 漏れない。
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

/// kitty ビルドの非同期経路: 初回(開いた瞬間)は同期で即表示、以降のズームは worker へ逃がし、
/// 旧画像を見せたまま結果到着で差し替える。stale(古い世代)は破棄する。
#[test]
fn kitty_zoom_builds_async_and_latest_wins() {
    let dir = std::env::temp_dir().join("konoma_kitty_async_test");
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.image_src = Some(std::sync::Arc::new(image::DynamicImage::new_rgb8(
        4000, 3000,
    )));
    app.preview_kind = Some(PreviewKind::Image(PathBuf::from("x.png")));
    let (itx, irx) = tokio::sync::mpsc::unbounded_channel();
    Box::leak(Box::new(irx));
    let mut picker = ratatui_image::picker::Picker::halfblocks();
    picker.set_protocol_type(ratatui_image::picker::ProtocolType::Kitty);
    app.attach_image_backend(picker, itx);
    // 実チャネルを付ける(worker が結果を送る)。
    let (ktx, krx) = std::sync::mpsc::channel();
    app.attach_kitty_loader(ktx);

    // 初回は同期: prepare 直後に kitty_image ができ、pending でない。worker には何も送られない。
    let r1 = app.prepare_image(inner(200, 40)).unwrap();
    assert!(app.kitty_image_ref().is_some(), "初回は同期で即表示");
    assert!(!app.kitty_build_pending(), "初回同期後は in-flight でない");
    let size1 = app.kitty_image_ref().unwrap().cell_size();
    assert_eq!(size1, (r1.width, r1.height));
    assert!(krx.try_recv().is_err(), "初回は worker を使わない(同期)");

    // ズーム: 以降は非同期。kitty_image は旧サイズのまま、pending=true。
    app.image_zoom = 4.0;
    app.prepare_image(inner(200, 40)).unwrap();
    assert!(app.kitty_build_pending(), "ズーム後は build in-flight");
    assert_eq!(
        app.kitty_image_ref().unwrap().cell_size(),
        size1,
        "結果到着まで旧画像を見せ続ける(ちらつかせない)"
    );

    // worker の結果を受けて適用 → 新サイズへ差し替え、pending 解消。
    let res = krx.recv().expect("worker sends a result");
    assert!(app.apply_kitty(res), "最新世代の結果を適用");
    assert!(!app.kitty_build_pending(), "適用後は in-flight でない");
    let size2 = app.kitty_image_ref().unwrap().cell_size();
    assert_ne!(size1, size2, "ズームで表示サイズが変わった");

    // 連続ズーム(2 spawn)→ 最新世代のみ適用、古い世代は破棄(スレッドは並行=到着順は非決定的
    // なので「先着=古い」とは限らない。gen で判定するので順序に依らず最新の1つだけが適用される)。
    app.image_zoom = 6.0;
    app.prepare_image(inner(200, 40)).unwrap();
    app.image_zoom = 8.0;
    app.prepare_image(inner(200, 40)).unwrap();
    let a = krx.recv().unwrap();
    let b = krx.recv().unwrap();
    let applied = u32::from(app.apply_kitty(a)) + u32::from(app.apply_kitty(b));
    assert_eq!(applied, 1, "2つの結果のうち最新世代の1つだけが適用される");
    assert!(!app.kitty_build_pending());

    std::fs::remove_dir_all(&dir).ok();
}

/// PDF ページ送り/動画再サムネ回帰: `set_static_image` は同じ表示サイズのラスタをその場で差し替える。
/// kitty 経路は geometry(kitty_want)で再ビルドを判定するので、同寸法だと want が変わらず**前ページの
/// ピクセルが居残る**。差し替え時に kitty geometry を無効化して必ず再ビルドさせる必要がある。
#[test]
fn kitty_rebuilds_on_same_size_image_swap() {
    let dir = std::env::temp_dir().join("konoma_kitty_swap_test");
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.image_src = Some(std::sync::Arc::new(image::DynamicImage::new_rgb8(800, 600)));
    app.preview_kind = Some(PreviewKind::Image(PathBuf::from("doc.pdf")));
    let (itx, irx) = tokio::sync::mpsc::unbounded_channel();
    Box::leak(Box::new(irx));
    let mut picker = ratatui_image::picker::Picker::halfblocks();
    picker.set_protocol_type(ratatui_image::picker::ProtocolType::Kitty);
    app.attach_image_backend(picker, itx); // kitty_tx なし=同期ビルド

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

    // ページ2を同寸法(800x600)でその場差し替え(pdf_goto→apply_media→set_static_image と同じ)。
    app.set_static_image(image::DynamicImage::new_rgb8(800, 600));
    app.prepare_image(inner(200, 40)).unwrap();
    assert!(
        render_syms(&app).contains("o=z"),
        "同寸法の差し替えでも kitty 画像が再ビルドされる(前ページが居残らない)"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// 非同期 kitty ビルドが失敗(worker panic → None)しても `kitty_build_pending` が永続 true にならない
/// こと。永続すると run ループが 16ms ポーリングを続けアイドル CPU 0% が崩れる(レビュー LOW 指摘)。
#[test]
fn kitty_failed_build_clears_pending() {
    let dir = std::env::temp_dir().join("konoma_kitty_fail_test");
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.image_src = Some(std::sync::Arc::new(image::DynamicImage::new_rgb8(
        4000, 3000,
    )));
    app.preview_kind = Some(PreviewKind::Image(PathBuf::from("x.png")));
    let (itx, irx) = tokio::sync::mpsc::unbounded_channel();
    Box::leak(Box::new(irx));
    let mut picker = ratatui_image::picker::Picker::halfblocks();
    picker.set_protocol_type(ratatui_image::picker::ProtocolType::Kitty);
    app.attach_image_backend(picker, itx);
    let (ktx, krx) = std::sync::mpsc::channel();
    app.attach_kitty_loader(ktx);
    Box::leak(Box::new(krx)); // 実 worker の結果は使わない(失敗を注入する)

    // 初回同期 → ズームで非同期ビルド in-flight(pending=true)。
    app.prepare_image(inner(200, 40)).unwrap();
    app.image_zoom = 4.0;
    app.prepare_image(inner(200, 40)).unwrap();
    assert!(app.kitty_build_pending(), "ズーム後は in-flight");

    // 現世代のビルド失敗(None)を注入 → 適用は false だが pending は解消する。
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

/// 大 repo での `h`/`l` の重さの主因: `git status`(全 worktree スキャン)が同一リポジトリ内の
/// root 変更のたびに同期実行されていた。`git status` は workdir から回すので結果は同一 → workdir が
/// 同じで dirty でなければ再計算せず流用する(ignored の Phase G と同型)。dirty / 別 repo なら取り直す。
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
    std::fs::write(dir.join("a.txt"), b"v2\n").unwrap(); // 変更 = status 非空

    let root = dir.canonicalize().unwrap();
    let mut app = App::new(root.clone(), Config::default()).unwrap();
    app.refresh_git_if_needed(); // 初回: statuses 計算 + workdir キャッシュ確立
    assert!(
        app.git_status_of(&root.join("a.txt")).is_some(),
        "変更が見える"
    );

    // status を再計算したか観測するためのセンチネル(実在しない偽エントリ)を差し込む。
    // 再計算が走れば git_status が丸ごと置き換わりセンチネルは消える(白箱: 直接フィールド操作)。
    let sentinel = root.join("__sentinel_not_a_real_file__");
    app.git_status
        .insert(sentinel.clone(), crate::git::FileStatus::Modified);

    // 同一 repo のサブディレクトリへ潜る(root 変更・workdir 不変・dirty でない)→ 再計算しない。
    app.root = root.join("sub");
    app.refresh_git_if_needed();
    assert!(
        app.git_status_of(&sentinel).is_some(),
        "同一 workdir の潜行は status を再計算しない(センチネル残存=キャッシュ流用)"
    );

    // dirty(外部コミット等の再検証)→ 取り直す(センチネル消失)。
    app.git_status_dirty = true;
    app.refresh_git_if_needed();
    assert!(
        app.git_status_of(&sentinel).is_none(),
        "dirty 指定で status を再計算(センチネル消失)"
    );
    // 実データは正しく取得できている(サブディレクトリからでも workdir 全体の変更が見える)。
    assert!(
        app.git_status_of(&root.join("a.txt")).is_some(),
        "再計算後も a.txt の変更は見える"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// workdir 単位の status キャッシュが **別 repo へ移ったら取り直す**こと(親 repo の status を
/// 入れ子 repo に流用しない)。`descend_into_nested_different_repo_recomputes_ignored_set` の
/// statuses 版=キャッシュの正しさの核心。
#[cfg(feature = "git")]
#[test]
fn descend_into_nested_different_repo_recomputes_status() {
    let dir = std::env::temp_dir().join("konoma_status_reuse_diff_repo");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("inner")).unwrap();
    init_git_repo(&dir); // 外側 repoA
    init_git_repo(&dir.join("inner")); // 入れ子の別 repoB

    let root = dir.canonicalize().unwrap();
    let mut app = App::new(root.clone(), Config::default()).unwrap();
    app.refresh_git_if_needed();
    let wd_a = app.git_status_workdir.clone();
    assert!(wd_a.is_some(), "外側 repo の status workdir が確立");

    // 再計算を観測するためのセンチネル(実在しない偽エントリ)。取り直せば消える。
    let sentinel = root.join("__status_sentinel__");
    app.git_status
        .insert(sentinel.clone(), crate::git::FileStatus::Modified);

    // inner(別 repo)へ潜行。
    let i = app
        .entries
        .iter()
        .position(|e| e.is_dir && e.path.ends_with("inner"))
        .expect("inner エントリが無い");
    app.selected = i;
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

/// 画像Aのズームで非同期ビルド(gen=N)が飛んだ後に**別ファイルへ切替**えると、遅れて届いた gen=N の
/// 結果は破棄されること(clear_image が gen を bump)。破棄しないと A の画像が B に紛れ込む。
#[test]
fn kitty_stale_build_from_previous_file_is_discarded_on_switch() {
    let dir = std::env::temp_dir().join("konoma_kitty_switch_race");
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.image_src = Some(std::sync::Arc::new(image::DynamicImage::new_rgb8(
        4000, 3000,
    )));
    app.preview_kind = Some(PreviewKind::Image(PathBuf::from("a.png")));
    let (itx, irx) = tokio::sync::mpsc::unbounded_channel();
    Box::leak(Box::new(irx));
    let mut picker = ratatui_image::picker::Picker::halfblocks();
    picker.set_protocol_type(ratatui_image::picker::ProtocolType::Kitty);
    app.attach_image_backend(picker, itx);
    let (ktx, krx) = std::sync::mpsc::channel();
    app.attach_kitty_loader(ktx);

    // Aを開き(初回同期)、ズームで非同期ビルド gen=N を飛ばす。
    app.prepare_image(inner(200, 40)).unwrap();
    app.image_zoom = 4.0;
    app.prepare_image(inner(200, 40)).unwrap();
    let stale = krx.recv().expect("Aのズームの worker 結果"); // gen=N の結果

    // 別ファイルBへ切替(clear_image が gen を bump=陳腐化)。
    app.clear_image();

    // 遅れて届いた A の gen=N 結果は破棄され、B(まだ画像なし)に紛れ込まない。
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

/// `git_dir_watch`: root が repo のサブディレクトリのとき親 `.git` を監視対象に返し、repo root や
/// 非 repo では None(再帰監視に含まれる/監視不要)。サブディレクトリ root で外部 git 操作を拾う穴埋め。
#[cfg(feature = "git")]
#[test]
fn git_dir_watch_targets_dot_git_only_for_subdir_root() {
    let dir = std::env::temp_dir().join("konoma_git_dir_watch");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src")).unwrap();
    init_git_repo(&dir);
    let root = dir.canonicalize().unwrap();

    // repo root: `.git` は再帰監視下 → None。
    let app_root = App::new(root.clone(), Config::default()).unwrap();
    assert_eq!(app_root.git_dir_watch(), None, "repo root は追加監視不要");

    // サブディレクトリ root: 親 `.git` は非監視 → その `.git` を返す。
    let app_sub = App::new(root.join("src"), Config::default()).unwrap();
    assert_eq!(
        app_sub.git_dir_watch(),
        Some(root.join(".git")),
        "subdir root は親 .git を監視対象に返す"
    );

    // 非 repo: None。
    let plain = std::env::temp_dir().join("konoma_git_dir_watch_norepo");
    let _ = std::fs::remove_dir_all(&plain);
    std::fs::create_dir_all(&plain).unwrap();
    let app_plain = App::new(plain.canonicalize().unwrap(), Config::default()).unwrap();
    assert_eq!(app_plain.git_dir_watch(), None, "非 repo は監視不要");

    std::fs::remove_dir_all(&dir).ok();
    std::fs::remove_dir_all(&plain).ok();
}

/// タブ切替(load_active)は git status の再検証の節目: 背面タブが監視外で取りこぼした外部変更に
/// 追従するため dirty を立て、切替後の描画で status を取り直す(同一 repo の**別サブディレクトリ**
/// タブでも陳腐化しない)。両タブが別 subdir=切替先 root が git_status_for と異なるので、dirty が
/// 無いと workdir キャッシュを流用して陳腐化が居残る(この構成で dirty を分離検証する)。
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
            .entries
            .iter()
            .position(|e| e.is_dir && e.path.ends_with(name))
            .unwrap_or_else(|| panic!("{name} エントリが無い"));
        app.selected = i;
        app.tree_descend().unwrap();
    };

    // タブ0を subx に。status を確立(git_status_for=subx)。
    let mut app = App::new(root.clone(), Config::default()).unwrap();
    descend(&mut app, "subx");
    app.refresh_git_if_needed();

    // タブ1を作り、suby へ移動して refresh(git_status_for=suby へ更新・workdir 同一で流用)。
    app.tab_new().unwrap();
    app.tree_leave().unwrap(); // repo root へ
    descend(&mut app, "suby");
    app.refresh_git_if_needed();

    // 再計算を観測するセンチネルを今の git_status に仕込む。
    let sentinel = root.join("__tab_status_sentinel__");
    app.git_status
        .insert(sentinel.clone(), crate::git::FileStatus::Modified);

    // タブ0(subx)へ戻る。切替先 root(subx) != git_status_for(suby) なので、dirty が無いと
    // workdir キャッシュを流用してセンチネルが残る。dirty があれば取り直して消える。
    app.tab_cycle(-1);
    app.refresh_git_if_needed(); // 切替後の描画相当

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

    // 旧実装はこの時点で statuses を同期計算し workdir まで埋めていた(=UI が止まっていた)。
    assert!(
        app.git_status_pending.is_some(),
        "別スレッドへ計算を投げているはず"
    );
    assert!(
        app.git_status_workdir.is_none(),
        "UI スレッドでは status を計算しない(旧実装ならここで Some になり落ちる)"
    );

    // ワーカーの結果を受け取って適用すると、初めて反映される。
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
    // 初期状態を確定させる(first.txt だけが見えている)。
    app.refresh_git_if_needed();
    let res = rx
        .recv_timeout(std::time::Duration::from_secs(30))
        .expect("初回スキャン");
    assert!(app.apply_statuses(res));
    assert!(app.git_status_of(&dir.join("first.txt")).is_some());

    // 裏で別のファイルが増える(= 切替時に取り直すべき変化)。
    std::fs::write(dir.join("second.txt"), b"y").unwrap();

    // 同一 root の2枚目のタブを作って切り替える。
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

    // ワーカーの結果が届いて初めて新しいファイルが見える。
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

    // 別 root へ移って新しい世代のスキャンを投げる。
    app.git_status_dirty = true;
    app.root = dir.join("sub");
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

    // 結果を**まだ適用せず**に描画相当を 20 回繰り返す(=スキャン走行中の描画)。
    for _ in 0..20 {
        app.refresh_git_if_needed();
    }
    let res = rx
        .recv_timeout(std::time::Duration::from_secs(30))
        .expect("1本目の結果");
    // 投げた本数はチャネルに届く結果数で数える(STATUS_CALLS はプロセス共有で並列テストを拾う)。
    assert_eq!(
        rx.try_iter().count(),
        0,
        "走行中は描画のたびに git status を投げ直してはいけない"
    );
    assert!(app.apply_statuses(res));

    // 適用後にさらに描画しても再スキャンしない(workdir 単位キャッシュ=Phase G)。
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
    app.refresh_git_if_needed(); // 1本目を走らせる(未適用のまま=走行中)

    // 走行中に fs イベントが 10 連続で来る(エージェントの書き込みバースト相当)。
    for i in 0..10 {
        std::fs::write(dir.join(format!("burst_{i}.txt")), b"y").unwrap();
        app.refresh_fs(false).unwrap();
    }
    // 投げたスキャン本数は「チャネルに届く結果の数」で数える(プロセス共有の STATUS_CALLS は
    // 並列実行される他テストの git 呼び出しを拾ってしまうため使わない)。
    let first = rx
        .recv_timeout(std::time::Duration::from_secs(30))
        .expect("1本目");
    assert_eq!(
        rx.try_iter().count(),
        0,
        "走行中の再検証要求はスレッドを増やさない(合体させる)"
    );

    // 1本目の結果を適用 → 溜まっていた要求を**1回だけ**引き継いで再スキャンする。
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
    app.toggle_changed_filter(); // `C`: 同期契約なのでその場で一覧が立つ
    assert!(app.changed_filter, "変更ファイルのみ表示になる");
    assert!(app.entries.iter().any(|e| e.path.ends_with("first.txt")));

    // エージェントが新規ファイルを作る → fs イベント(非同期スキャンを投げるだけ)。
    std::fs::write(dir.join("agent_new.txt"), b"y").unwrap();
    app.refresh_fs(false).unwrap();
    // 結果が届いて初めて一覧に載る。
    let res = rx
        .recv_timeout(std::time::Duration::from_secs(30))
        .expect("スキャン結果");
    app.apply_statuses(res);
    assert!(
        app.entries
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
    app.attach_status_loader(tx); // 以後 status は非同期(=押した瞬間はまだ届いていない)
    app.selected = app
        .entries
        .iter()
        .position(|e| e.path.ends_with("tracked.txt"))
        .unwrap();

    app.tree_open_git_diff();
    assert!(
        matches!(app.preview_kind, Some(PreviewKind::GitDiff(_))),
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
    // 1度適用して workdir キャッシュを確立(以後 `l` は流用パス=世代が進まない)。
    app.refresh_git_if_needed();
    let res = rx
        .recv_timeout(std::time::Duration::from_secs(30))
        .expect("初回");
    assert!(app.apply_statuses(res));

    // 再検証を要求してスキャンを走らせ(未適用のまま)、その最中に `l` で sub へ潜る。
    app.git_status_dirty = true;
    app.refresh_git_if_needed(); // kick(この時点の root = repo root)
    let i = app
        .entries
        .iter()
        .position(|e| e.is_dir && e.path.ends_with("sub"))
        .expect("sub");
    app.selected = i;
    app.tree_descend().unwrap();
    app.refresh_git_if_needed(); // 同一 repo=流用パス(世代は進まない)
    assert!(app.root.ends_with("sub"));

    // 走行中だった結果が今ここで届く。
    let res = rx
        .recv_timeout(std::time::Duration::from_secs(30))
        .expect("走行中だった結果");
    app.apply_statuses(res);

    // 直後の fs イベントが再検証を投げられること(旧実装は early return で丸ごと落としていた)。
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
    // タブ1 = data.csv のプレビュー、タブ2 = ツリー。
    app.selected = app
        .entries
        .iter()
        .position(|e| e.path.ends_with("data.csv"))
        .unwrap();
    app.tree_activate().unwrap();
    assert!(matches!(app.preview_kind, Some(PreviewKind::Table { .. })));
    app.tab_new().unwrap();

    // タブ1(表)へ戻る = load_active が走る。
    crate::preview::table::PARSE_CALLS.with(|c| c.set(0));
    app.tab_cycle(1);
    assert!(
        matches!(app.preview_kind, Some(PreviewKind::Table { .. })),
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
    std::fs::write(&img, b"not-a-real-png").unwrap(); // 中身は使わない(デコード結果は下で直接入れる)
    std::fs::write(dir.join("note.txt"), b"x").unwrap();

    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    // 画像プレビュー相当の状態を作る(picker 無しでもデコード済み状態を再現できる)。
    app.selected = app
        .entries
        .iter()
        .position(|e| e.path.ends_with("pic.png"))
        .unwrap();
    app.mode = Mode::Preview;
    app.preview_path = Some(img.clone());
    app.preview_kind = Some(PreviewKind::Image(img.clone()));
    let decoded = std::sync::Arc::new(image::DynamicImage::ImageRgba8(image::RgbaImage::new(4, 4)));
    app.image_src = Some(decoded.clone());
    app.preview_media_mtime = crate::app::file_mtime(&img);

    // 別タブへ移り、戻る。
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

    // 外部でファイルが変わったら再利用しない(古い絵を出さない)。
    app.tab_cycle(1); // 退避
    std::thread::sleep(std::time::Duration::from_millis(10));
    std::fs::write(&img, b"changed-bytes").unwrap();
    app.tab_cycle(1); // 画像タブへ戻る
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
    app.mode = Mode::Preview;
    app.preview_path = Some(img.clone());
    app.preview_kind = Some(PreviewKind::Image(img.clone()));
    app.preview_media_mtime = crate::app::file_mtime(&img);
    // 17M 画素 = 画素数だけ見れば「32MP 上限」を通ってしまうが、16bit(8B/px)なので 136MB。
    // バッファは 1 行に寄せてメモリを実寸のまま扱う(幅だけ極端に大きい画像)。
    let huge = image::DynamicImage::ImageRgba16(image::ImageBuffer::new(17_000_000, 1));
    assert!(
        huge.as_bytes().len() > 128 * 1024 * 1024,
        "テスト前提: 128MiB を超えるバッファ"
    );
    app.image_src = Some(std::sync::Arc::new(huge));

    app.tab_new().unwrap(); // save_active 経由で退避が走る
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
    app.mode = Mode::Preview;
    app.preview_path = Some(doc.clone());
    app.preview_media_mtime = crate::app::file_mtime(&doc);
    // フェンス#1 を全画面表示している状態で、その図を退避させる。
    app.preview_kind = Some(PreviewKind::MermaidFence(1));
    let fence1 = std::sync::Arc::new(image::DynamicImage::ImageRgba8(image::RgbaImage::new(4, 4)));
    app.image_src = Some(fence1.clone());
    app.tab_new().unwrap(); // save_active → stash(fence_ord=1)

    // 別タブが**同じ文書のフェンス#0**を開いている状態に戻る。
    app.mode = Mode::Preview;
    app.preview_path = Some(doc.clone());
    app.preview_kind = Some(PreviewKind::MermaidFence(0));
    app.image_src = None;
    let restored = app.restore_media_cache(&doc, 1);
    assert!(
        !restored && app.image_src.is_none(),
        "別のフェンスの図を使い回さない(序数までキーに含める)"
    );
    // 同じフェンスなら再利用する。
    app.preview_kind = Some(PreviewKind::MermaidFence(1));
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
    app.mode = Mode::Preview;
    app.preview_path = Some(img.clone());
    app.preview_kind = Some(PreviewKind::Image(img.clone()));
    app.preview_media_mtime = crate::app::file_mtime(&img);
    app.image_src = Some(std::sync::Arc::new(image::DynamicImage::ImageRgba8(
        image::RgbaImage::new(4, 4),
    )));

    app.tab_new().unwrap(); // 退避される
    assert!(app.media_cache.is_some(), "前提: 退避されている");
    app.tab_cycle(1); // 画像タブへ戻る
    app.tab_close(); // そのタブを閉じる
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
    app.mode = Mode::Preview;
    app.preview_path = Some(img.clone());
    app.preview_kind = Some(PreviewKind::Image(img.clone()));
    app.preview_media_mtime = crate::app::file_mtime(&img);
    app.image_src = Some(std::sync::Arc::new(image::DynamicImage::ImageRgba8(
        image::RgbaImage::new(4, 4),
    )));
    app.tab_new().unwrap(); // 退避

    // 中身とサイズを変え、mtime だけ元に戻す(cp -p 相当)。
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
    app.toggle_changed_filter(); // `C` は同期契約なので即座に一覧が立つ
    assert!(app.changed_filter, "前提: 変更ファイルのみ表示");

    // 別 repo のタブへ移り、そこで status を確定させる。
    app.tab_new().unwrap();
    app.root = b.clone();
    app.open_dir = b.clone();
    app.rebuild_tree().unwrap();
    app.refresh_git_if_needed();
    while let Ok(res) = rx.try_recv() {
        app.apply_statuses(res);
    }

    // repo A のタブへ戻る(この時点で A の status は走行中)。
    app.tab_cycle(1);
    assert!(
        app.changed_filter,
        "スキャン走行中でもフィルタは維持される(旧実装は解除+偽 flash)"
    );
    assert_ne!(
        app.flash.as_deref(),
        Some("no changed files"),
        "嘘の「変更なし」を出さない"
    );

    // 結果が届けば一覧が作り直される。
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while app.git_status_pending.is_some() && std::time::Instant::now() < deadline {
        if let Ok(res) = rx.recv_timeout(std::time::Duration::from_secs(30)) {
            app.apply_statuses(res);
        }
    }
    assert!(app.changed_filter, "到着後もフィルタは生きている");
    assert!(
        app.entries.iter().any(|e| e.path.ends_with("changed.txt")),
        "到着した status で一覧が作り直される"
    );
    std::fs::remove_dir_all(&base).ok();
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

    // repo の外(git 管理下でないディレクトリ)へ移る。
    app.root = plain.clone();
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
