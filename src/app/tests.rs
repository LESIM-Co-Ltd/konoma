use super::bookmark_actions::fuzzy_filter_pool;
use super::*;
use crate::config::Config;
use crate::test_support::unique_tmp;

/// Test helper: the link target of an `MdItem` (panics if the item is a checkbox).
fn item_target(it: &MdItem) -> &str {
    match &it.kind {
        MdItemKind::Link { target } => target,
        MdItemKind::Task { .. } => panic!("expected a link item"),
        MdItemKind::CodeBlock { .. } => panic!("expected a link item"),
        MdItemKind::MermaidFence { .. } => panic!("expected a link item"),
        MdItemKind::Details { .. } => panic!("expected a link item"),
    }
}

/// Resolves a fixture bundled under the repo's `samples/` directory, anchored at
/// `CARGO_MANIFEST_DIR` (baked in at compile time) rather than a bare relative path — a plain
/// `Path::new("samples/…")` resolves against the test binary's **cwd**, which is only the crate
/// root by convention (`cargo test` run from elsewhere, e.g. `cd /tmp && cargo test
/// --manifest-path …`, is a real, supported invocation), so it silently missed the fixture and
/// silently skipped every assertion in every test that used it. Tolerant of the one case where the
/// fixture is legitimately absent — `samples/` is excluded from the published crate (`Cargo.toml`'s
/// `exclude`) — by returning `None` (same early-return as before) but saying so loudly
/// (`eprintln!`, visible with `--nocapture` or in the captured-output dump whenever the process
/// later exits non-zero for any reason) instead of silently passing zero assertions.
fn sample_path_or_skip(name: &str) -> Option<PathBuf> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("samples")
        .join(name);
    if p.exists() {
        Some(p)
    } else {
        eprintln!(
            "SKIP: samples/{name} not found (excluded from the published crate) — this test verifies nothing this run"
        );
        None
    }
}

#[test]
fn path_styles_format_as_expected() {
    let open = unique_tmp("konoma_app_test_open");
    std::fs::create_dir_all(&open).unwrap();
    let mut app = App::new(open.clone(), Config::default()).unwrap();
    let file = open.join("src").join("main.rs");

    app.path_style = PathStyle::Full;
    assert_eq!(app.format_path(&file), file.display().to_string());

    // Relative: prefixes the launch directory name (derived from `open`'s own basename, not a
    // literal — `open` is a `unique_tmp` fixture so its basename carries a pid/counter suffix).
    app.path_style = PathStyle::Relative;
    let expected_rel = format!(
        "{}/src/main.rs",
        open.file_name().unwrap().to_string_lossy()
    );
    assert_eq!(app.format_path(&file), expected_rel);

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
    let work = unique_tmp("konoma_relpath_test_work");
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
    let dir = unique_tmp("konoma_dialog_ops_test");
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
    let dir = unique_tmp("konoma_dialog_cursor_test");
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
    let dir = unique_tmp("konoma_single_toggle_test");
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
    let dir = unique_tmp("konoma_visual_test");
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
    let dir = unique_tmp("konoma_visual_scope_test");
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
    let dir = unique_tmp("konoma_batchrename_app_test");
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
    let dir = unique_tmp("konoma_batchrename_collision_test");
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

/// `build_rename_plan`'s pre-validation (`src/app.rs`) has a real gap: its duplicate-name check is a
/// `BTreeSet<PathBuf>` (byte-exact — `X.txt` and `x.txt` are "different"), and its existing-file check
/// runs *before* any rename has executed, when neither of two case-variant destinations exists on disk
/// yet. Both pass silently, and a plain `mv a.txt X.txt; mv b.txt x.txt` on a case-insensitive
/// filesystem (verified independently in a shell: the second `mv` silently overwrites the first,
/// leaving one file) is exactly the shape that gap lets through unvalidated.
///
/// **What this test found, going through the real entry point** (`App::start_batch_rename` →
/// `dialog_submit` → `dialog_preview_apply` → `fileops::batch_rename`), **is that konoma does not
/// reproduce that data loss** — not because the gap above isn't real, but because
/// `fileops::batch_rename`'s two-phase commit independently re-checks `dst.exists()` at *commit* time,
/// after every source has already been staged aside. By the time the second of two colliding
/// destinations is about to commit, the first has already landed, so this second, later check catches
/// the collision `build_rename_plan` missed and the whole batch rolls back — both original files come
/// back intact. Verified for both commit orders and for two sequential single-file renames
/// (`fileops::rename` has the identical before-rename `dst.exists()` guard). This is a genuine,
/// separate safety net, not a fix to the validation gap above — a future change to
/// `fileops::batch_rename` that drops or weakens its own `dst.exists()` check would reopen the data
/// loss with nothing left to catch it, which is what this test guards against.
///
/// The template mechanism (`{n}`/`{n:0W}`/`{name}`/`{ext}`, no case transform) can't turn one selected
/// file into an uppercase variant of another directly, so the case collision here is produced the way
/// a real template realistically would hit it: a literal template with no extension token, applied to
/// two files whose original extensions differ only in case — automatic extension-preservation then
/// proposes `dup.txt` and `dup.TXT`, which collide on a case-insensitive filesystem exactly as `X.txt`/
/// `x.txt` would.
#[test]
fn batch_rename_case_insensitive_destination_collision_does_not_lose_data() {
    let dir = unique_tmp("konoma_batchrename_case_collision_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    // Detect at runtime whether this filesystem folds case (never assume by `cfg(target_os)`):
    // write "CI_PROBE", then ask whether "ci_probe" resolves to the same entry.
    std::fs::write(dir.join("CI_PROBE"), b"probe").unwrap();
    let case_insensitive = dir.join("ci_probe").exists();
    std::fs::remove_file(dir.join("CI_PROBE")).ok();

    std::fs::write(dir.join("a.txt"), b"AAA").unwrap();
    std::fs::write(dir.join("b.TXT"), b"BBB").unwrap();

    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.rebuild_tree().unwrap();
    app.visual_select_scope(true);
    assert_eq!(app.marked_count(), 2, "両方選択されている前提");

    // A literal template (no {n}/{ext}) → extension auto-preservation proposes "dup.txt" and
    // "dup.TXT" — a case-insensitive collision `build_rename_plan` does not reject (this is the gap
    // described above: neither destination exists yet at validation time, and the byte-exact
    // duplicate-name `BTreeSet` treats them as distinct names).
    app.start_batch_rename();
    for c in "dup".chars() {
        app.dialog_input_push(c);
    }
    app.dialog_submit().unwrap();
    assert!(
        app.dialog_is_preview(),
        "検証を通り抜けてプレビューへ進む(=事前検証の穴そのもの): {:?}",
        app.dialog_view()
    );
    let (_, pairs, _) = app.dialog_preview_view().unwrap();
    assert_eq!(pairs, vec!["a.txt  →  dup.txt", "b.TXT  →  dup.TXT"]);

    // Apply through the real production entry point. `dialog_preview_apply`'s `Result` is not the
    // rename outcome (it stays `Ok(())` even when the underlying `fileops::batch_rename` fails and
    // only sets `self.flash`) — the actual outcome has to be read from the filesystem and the flash.
    app.dialog_preview_apply().unwrap();

    let names_after: std::collections::BTreeSet<String> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    let contents_after: std::collections::BTreeSet<Vec<u8>> = names_after
        .iter()
        .map(|n| std::fs::read(dir.join(n)).unwrap_or_default())
        .collect();

    if case_insensitive {
        // The collision IS let through by validation, but `fileops::batch_rename`'s own commit-time
        // check catches it and rolls the whole batch back. Both files remain, as two files, with
        // their original bytes intact — nothing is silently overwritten.
        assert_eq!(
            names_after.len(),
            2,
            "大小非区別fs: 2ファイルが2ファイルのまま残るはず — 消えた: {names_after:?}"
        );
        assert_eq!(
            contents_after,
            [b"AAA".to_vec(), b"BBB".to_vec()].into_iter().collect(),
            "大小非区別fs: 両方の内容が失われずに残っているはず(名前は元に戻る) — 実際: {names_after:?}"
        );
        assert!(
            app.flash.is_some(),
            "衝突は失敗として通知されるはず(黙って何も言わないのはそれ自体バグ)"
        );
    } else {
        // A case-sensitive filesystem never sees this as a collision at all — "dup.txt" and
        // "dup.TXT" are simply two different, unrelated names, and the rename succeeds normally.
        assert_eq!(
            names_after,
            ["dup.txt".to_string(), "dup.TXT".to_string()]
                .into_iter()
                .collect(),
            "大小区別fs(Linux): 通常どおり両方の新しい名前に変わって残るはず"
        );
        assert_eq!(
            contents_after,
            [b"AAA".to_vec(), b"BBB".to_vec()].into_iter().collect(),
            "大小区別fs: 内容も保持されているはず"
        );
    }

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn parse_dropped_paths_unescapes_splits_and_filters() {
    let tmp = unique_tmp("konoma_parse_drop_test");
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
    let dir = unique_tmp("konoma_drop_flow_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    // Two files with names containing spaces at the drop source (a different directory).
    let ext = unique_tmp("konoma_drop_src");
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
    let dir = unique_tmp("konoma_paste_filter_test");
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
    let base = unique_tmp("konoma_reanchor_test");
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
    let dir = unique_tmp("konoma_gitignore_test");
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

/// C13 (minor carryover): if an fs-event burst consists entirely of gitignored paths **whose
/// contents were merely rewritten** (build churn such as target/, etc.), it is fine to skip
/// `refresh_fs_changed`. Mixed/empty/ignore-rule-change/structural cases err on the safe side =
/// always refresh.
#[cfg(feature = "git")]
#[test]
fn fs_burst_build_churn_skips_only_all_ignored_paths() {
    let dir = unique_tmp("konoma_fs_burst_churn_test");
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

    // A burst of plain writes (nothing created/removed/renamed) — the case the guard exists for.
    let writes = crate::app::FsBurstKinds {
        ignore_rules_changed: false,
        structural: false,
    };
    // The same paths, but the burst also added/removed/renamed something.
    let structural = crate::app::FsBurstKinds {
        ignore_rules_changed: false,
        structural: true,
    };

    assert!(
        app.fs_burst_is_build_churn(std::slice::from_ref(&out_o), writes),
        "全パスが ignored で書き込みのみ → churn=true でスキップ可(ビルド churn の非退行)"
    );
    assert!(
        !app.fs_burst_is_build_churn(std::slice::from_ref(&src_rs), writes),
        "非 ignored パスのみ → churn=false でリフレッシュ"
    );
    assert!(
        !app.fs_burst_is_build_churn(&[out_o.clone(), src_rs.clone()], writes),
        "混在 → churn=false でリフレッシュ"
    );
    assert!(
        !app.fs_burst_is_build_churn(&[], writes),
        "空(不明/.git のみ) → churn=false でリフレッシュ"
    );
    assert!(
        !app.fs_burst_is_build_churn(
            std::slice::from_ref(&out_o),
            crate::app::FsBurstKinds {
                ignore_rules_changed: true,
                structural: false,
            }
        ),
        "無視ルール自体が変わった → churn=false で常にリフレッシュ"
    );
    // The fix: a create/remove/rename changes *which rows exist*, so it must never be skipped —
    // even when every path in the burst is ignored. Skipping it left deleted files on screen.
    assert!(
        !app.fs_burst_is_build_churn(std::slice::from_ref(&out_o), structural),
        "ignored でも作成/削除/リネームを含む → churn=false でリフレッシュ(行の有無が変わる)"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// The reported bug, end to end on the decision the run loop makes: a file deleted **inside a
/// gitignored directory** (`out/` in the `.gitignore`, browsed with root set to `out/`) used to stay
/// on screen forever. The build-churn guard saw "every changed path is ignored" and skipped the
/// refresh — a rule written for *writes* to `target/`, which silently swallowed removals too.
///
/// The event kinds are produced by the real classifier (`crate::is_structural_event`) from the kind
/// a deletion actually raises, so reverting either half — the classifier or the guard's use of it —
/// fails here.
#[cfg(feature = "git")]
#[test]
fn deleting_a_file_inside_an_ignored_directory_updates_the_tree() {
    use notify::event::RemoveKind;
    use notify::EventKind;

    let dir = unique_tmp("konoma_ignored_delete_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("out")).unwrap();
    git2::Repository::init(&dir).unwrap();
    std::fs::write(dir.join(".gitignore"), b"out/\n").unwrap();
    std::fs::write(dir.join("out").join("a_keep.jpg"), b"keep-me").unwrap();
    std::fs::write(dir.join("out").join("b_delete.png"), b"delete-me").unwrap();

    // Browse the ignored directory itself, as in the report.
    let root = dir.join("out").canonicalize().unwrap();
    let mut app = App::new(root.clone(), Config::default()).unwrap();
    app.rebuild_tree().unwrap();
    app.refresh_git_if_needed();

    let keep = root.join("a_keep.jpg");
    let doomed = root.join("b_delete.png");
    assert!(
        app.is_ignored(&doomed),
        "前提: out/ 配下は gitignore 対象(この条件でのみ再現する)"
    );
    assert!(
        app.tab.entries.iter().any(|e| e.path == doomed),
        "前提: 削除前はツリーに居る"
    );

    std::fs::remove_file(&doomed).unwrap();

    // What `main`'s watcher callback + run loop build for this event.
    let kinds = crate::app::FsBurstKinds {
        ignore_rules_changed: false,
        structural: crate::is_structural_event(&EventKind::Remove(RemoveKind::File)),
    };
    let changed = vec![doomed.clone()];
    assert!(
        !app.fs_burst_is_build_churn(&changed, kinds),
        "ignored 配下でも削除はスキップしてはいけない"
    );
    app.refresh_fs_watched(kinds.ignore_rules_changed, &changed);

    assert!(
        !app.tab.entries.iter().any(|e| e.path == doomed),
        "削除されたファイルがツリーに残っている(幽霊行)"
    );
    assert!(
        app.tab.entries.iter().any(|e| e.path == keep),
        "残っているファイルは巻き添えで消えない"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// A row whose metadata can't be read shows **blank** detail cells, never invented ones. The old
/// fallback (`RowMeta { size: 0, .. }`) rendered as a confident `0 B`, which is how a row that
/// outlived its file managed to look like a real empty file rather than like a mistake.
#[test]
fn detail_cells_are_blank_when_the_metadata_cannot_be_read() {
    let dir = unique_tmp("konoma_detail_cells_missing_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("real.txt"), b"0123456789").unwrap();
    let dir = dir.canonicalize().unwrap();

    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    let cols: Vec<String> = ["size", "modified", "type", "perm"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let real = dir.join("real.txt");
    let missing = dir.join("never_existed.txt");
    app.ensure_detail_cells(&[real.clone(), missing.clone()], &cols);

    let real_cells = app
        .detail_cells_get(&real)
        .expect("実在ファイルはセルを持つ");
    assert_eq!(
        real_cells[0], "10 B",
        "実在ファイルは実サイズを出す(非退行)"
    );
    assert!(!real_cells[1].is_empty(), "実在ファイルは更新日時を出す");

    let missing_cells = app
        .detail_cells_get(&missing)
        .expect("不在パスもキャッシュはされる(毎フレーム stat し直さない)");
    assert_eq!(
        missing_cells.len(),
        cols.len(),
        "列数は揃える(右寄せレイアウトが崩れない)"
    );
    assert!(
        missing_cells.iter().all(|c| c.is_empty()),
        "メタデータが取れない行は全列空欄(0 B や file と偽らない): {missing_cells:?}"
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
    let dir = unique_tmp("konoma_diff_entrypoints");
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

/// macOS-only end-to-end regression for the NFC/NFD status-key mismatch (see `git.rs`'s
/// `nfd_named_file_status_is_found_via_the_tree_path` for the lower-level `statuses()` version of
/// this). `git init` sets `core.precomposeunicode=true` on macOS by default, so `git status
/// --porcelain` reports a changed file's name **precomposed (NFC)** while the bytes macOS actually
/// wrote to disk for that name are **decomposed (NFD)**. Before `git::normalize_status_path`,
/// `git_status_of` — an exact `HashMap::get` keyed by the tree's `read_dir`-sourced (on-disk, NFD)
/// path — never matched: the file showed no `M` marker in the tree, and pressing `d` to open its
/// diff (`tree_open_git_diff`) was rejected with "no changes" even though it really was modified.
#[cfg(all(feature = "git", target_os = "macos"))]
#[test]
fn nfd_named_file_status_and_diff_are_reachable_from_the_tree_path() {
    let dir = unique_tmp("konoma_nfd_status_app_test");
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

    let canon = dir.canonicalize().unwrap();
    // NFD spelling: か (U+304B) + the combining voiced sound mark (U+3099) — decomposed が.
    let nfd_name = "\u{304B}\u{3099}_nfd.txt";
    let nfd_path = canon.join(nfd_name);
    std::fs::write(&nfd_path, b"original\n").unwrap();
    git(&["add", "-A"]);
    git(&["commit", "-qm", "init"]);
    // Modify it — porcelain now reports it as changed, spelled NFC.
    std::fs::write(&nfd_path, b"original\nchanged\n").unwrap();

    let mut app = App::new(canon.clone(), Config::default()).unwrap();
    app.rebuild_tree().unwrap();
    app.refresh_git_if_needed();

    assert!(
        app.git_status_of(&nfd_path).is_some(),
        "ツリー側の NFD パスで M が引ける必要がある(git_status_of)"
    );

    let idx = app
        .tab
        .entries
        .iter()
        .position(|e| e.path == nfd_path)
        .expect("NFD 名のファイルがツリーに無い");
    app.tab.selected = idx;
    app.tree_open_git_diff();
    assert!(
        app.is_git_diff_preview(),
        "d は「変更なし」で拒否されず diff が開く必要がある(tree_open_git_diff)"
    );
    assert!(!app.git_diff_lines().is_empty(), "diff 行がある");

    std::fs::remove_dir_all(&dir).ok();
}

#[cfg(feature = "git")]
#[test]
fn diff_horizontal_scroll_reveals_long_line() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    let dir = unique_tmp("konoma_diff_hscroll");
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
    let dir = unique_tmp("konoma_clipboard_test");
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
    let dir = unique_tmp("konoma_dialog_perm_test");
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
    let dir = unique_tmp("konoma_tree_page_test");
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
    let dir = unique_tmp("konoma_tree_move_extremes_test");
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
    let dir = unique_tmp("konoma_branch_move_extremes_test");
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
    let dir = unique_tmp("konoma_graph_move_extremes_test");
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    let commit = |id: &str| crate::git::GraphRow {
        graph: Vec::new(),
        node: Some(crate::git::NodeKind::Normal),
        node_col: Some(0),
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
        node: None,
        node_col: None,
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
    let dir = unique_tmp("konoma_log_move_extremes_test");
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
    let dir = unique_tmp("konoma_view_move_extremes_test");
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
    let dir = unique_tmp("konoma_img_state_test");
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
    let dir = unique_tmp("konoma_gif_kitty_test");
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
    let Some(p) = sample_path_or_skip("sample.svg") else {
        return;
    };
    let mut app = app_with_kitty();
    let (tx, rx) = std::sync::mpsc::channel();
    app.attach_media_loader(tx);
    let kind = PreviewKind::Svg(p.clone());
    app.tab.preview_kind = Some(kind.clone());

    app.start_media_load(&kind, &p);
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
    let dir = unique_tmp("konoma_pdf_nav_test");
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
/// resolves (nothing installed needed), so this only skips without the bundled sample. Page 2 in
/// particular has no fallback at all any more (the macOS chain is first-page-only), so this also
/// pins that the native renderer carries multi-page navigation on its own.
#[test]
fn pdf_next_page_renders_off_thread() {
    let Some(p) = sample_path_or_skip("sample.pdf") else {
        return;
    };
    let Some(pages) = crate::preview::pdf::page_count(&p) else {
        return; // safety net in case parsing fails unexpectedly (normally unreachable)
    };
    if pages < 2 {
        return;
    }
    let mut app = app_with_kitty();
    let (tx, rx) = std::sync::mpsc::channel();
    app.attach_media_loader(tx);
    let kind = PreviewKind::Pdf(p.clone());
    app.tab.preview_kind = Some(kind.clone());
    app.tab.preview_path = Some(p.clone());
    app.tab.mode = Mode::Preview;
    app.tab.pdf_pages = Some(pages);
    app.tab.pdf_page = 1;

    // Load page 1 and apply it.
    app.start_media_load(&kind, &p);
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
    let dir = unique_tmp("konoma_tabbar_test");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("f.txt"), b"x").unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    // Expected label is derived from `dir`'s own basename, not a literal — `dir` is a
    // `unique_tmp` fixture, so its basename carries a pid/counter suffix.
    let label = dir.file_name().unwrap().to_string_lossy().to_string();

    let render = |app: &mut App| -> String {
        // Wide enough that both tab chips (each carrying `label`'s pid/counter suffix) are shown
        // in full rather than dropped by the tab bar's overflow window — the chip text itself is
        // never truncated (`visible_range` hides whole tabs, not partial chip text), so extra
        // width just gives headroom against a long pid.
        let mut term = Terminal::new(TestBackend::new(120, 6)).unwrap();
        term.draw(|f| crate::ui::render(f, app)).unwrap();
        term.backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect()
    };
    // Default tabbar=auto: no tabbar is shown with a single tab.
    assert!(!render(&mut app).contains(&format!("1:{label}")));
    // With two tabs, 1:/2: appear on the tabbar.
    app.tab_new().unwrap();
    let s = render(&mut app);
    assert!(s.contains(&format!("1:{label}")), "タブ1が無い: {s:?}");
    assert!(s.contains(&format!("2:{label}")), "タブ2が無い: {s:?}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn markdown_links_collected_and_local_link_opens_in_konoma() {
    // Collect links (URLs) in the md preview, focus one, and open a local link inside konoma.
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    let dir = unique_tmp("konoma_links_open_test");
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
    let dir = unique_tmp("konoma_md_focus_wrap_test");
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
    let dir = unique_tmp("konoma_md_task_toggle_test");
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
/// recognize `-`, so the count check drifted and everything got cancelled — user report 2026-07-22).
#[test]
fn md_task_toggle_star_and_plus_bullets() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    let dir = unique_tmp("konoma_md_task_star_plus");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("todo.md");
    // `*`/`+` bullet lists (GFM-compliant). konoma's own renderer draws checkboxes for all three kinds.
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

    // Toggle the first one (star) → it isn't cancelled and writes back. No refusal flash.
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
            .map(|m| !m.contains("couldn't toggle checkbox"))
            .unwrap_or(true),
        "トグル拒否のフラッシュが出ていない: {:?}",
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
    let dir = unique_tmp("konoma_md_task_custom_test");
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
    // Don't write if the displayed state and disk state disagree **on the focused checkbox's own
    // line** (flash + reload) — position-based verification, not a document-wide count. Avoids
    // conflicting with an external agent's edits to that exact checkbox.
    let dir = unique_tmp("konoma_md_task_abort_test");
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
    // (1) State mismatch on the focused checkbox's own line: the screen stays at ' ' while the
    // disk becomes 'x'. The write-back verification is position-based (does *this* line, read
    // fresh from disk, still match what's on screen up to and including the state char) — it
    // catches this the same way a document-wide count once did, without needing one.
    std::fs::write(&f, "- [x] a\n").unwrap();
    app.md_toggle_focused_task();
    assert_eq!(
        std::fs::read_to_string(&f).unwrap(),
        "- [x] a\n",
        "書かない"
    );
    assert!(app.flash.is_some(), "flash で通知");
    // (2) A genuinely *unrelated* external edit — a second, brand-new task appended after the
    // focused one — no longer blocks the toggle (the migration's whole point: the old document-wide
    // count guard used to refuse this too, even though the focused checkbox's own line is untouched
    // and toggling it is perfectly safe). `md-block-walk`'s position-based verification only checks
    // the byte range of the checkbox actually being toggled, so this now succeeds.
    term.draw(|fr| crate::ui::render(fr, &mut app)).unwrap(); // let the screen catch up to x…
    std::fs::write(&f, "- [x] a\n- [ ] b\n").unwrap(); // …then append an unrelated new task on disk
    app.flash = None;
    app.md_toggle_focused_task();
    assert_eq!(
        std::fs::read_to_string(&f).unwrap(),
        "- [ ] a\n- [ ] b\n",
        "無関係な行が増えただけなら、フォーカス中のチェックボックス自身の行が一致していれば書く"
    );
    // (3) An edit to the *same* checkbox's own line, concurrent with the count change above, is
    // still caught (position-based verification does not degrade into "never refuses"): the disk
    // now shows "a" already checked by someone else, so the app's own toggle must refuse rather
    // than blindly flip it back to unchecked.
    term.draw(|fr| crate::ui::render(fr, &mut app)).unwrap(); // catch up to "- [ ] a\n- [ ] b\n"
    std::fs::write(&f, "- [x] a\n- [ ] b\n").unwrap(); // the focused line ("a") changes underneath it
    app.flash = None;
    app.md_toggle_focused_task();
    assert_eq!(
        std::fs::read_to_string(&f).unwrap(),
        "- [x] a\n- [ ] b\n",
        "フォーカス中の行自体が変わっていれば依然として書かない"
    );
    assert!(app.flash.is_some());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn md_task_toggle_noop_in_raw_source_and_preserves_crlf() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    let dir = unique_tmp("konoma_md_task_raw_test");
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
    // `toggle_md_raw` drops `md_cache`, and the toggle now resolves the checkbox through the text
    // that cache holds (see `md_toggle_focused_task`). Draw once first, exactly as the run loop does
    // between two keypresses — without a frame there is no rendered document to point at, and the
    // app never puts a checkbox on screen without having drawn it.
    term.draw(|fr| crate::ui::render(fr, &mut app)).unwrap();
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
    let dir = unique_tmp("konoma_md_task_guard_test");
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
    let probe = unique_tmp("konoma_perm_probe");
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
    let dir = unique_tmp("konoma_md_task_wrerr_test");
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
    let dir = unique_tmp("konoma_md_items_mix_test");
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
    let dir = unique_tmp("konoma_md_code_focus_test");
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
            MdItemKind::CodeBlock { .. } => "code",
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
    // The span sequence corresponding to konoma's own renderer's "label (URL)" output. Followed by a trailing ".".
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
    let dir = unique_tmp("konoma_autolink_item_test");
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    // A plain paragraph line with a bare URL (as konoma's own renderer emits: one raw span).
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
    let dir = unique_tmp("konoma_autolink_off_test");
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
    let dir = unique_tmp("konoma_emoji_off_test");
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
    let dir = unique_tmp("konoma_links_hl_test");
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
    let dir = unique_tmp("konoma_code_decorate_test");
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
    let dir = unique_tmp("konoma_tree_filter_test");
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

// GUARDS: fuzzy-filtering a large pool (the tree filter's realistic worst case — `collect_all`'s
// own cap is 50,000) must not visibly stall the UI thread on every keystroke. `Matcher` reuse (the
// thread-local in `bookmark_actions.rs`) is what keeps this fast — rebuilding one per call would
// add its own ~135KB allocation on top. Converted from a wall-clock bound to a **deterministic
// allocation-scaling** check (same reasoning as `speed_tests::highlight_lang_large_source_is_bounded`):
// doubling the pool size should roughly double the allocation, not quadruple it (measured: ~2.0x,
// stable regardless of measurement order).
#[test]
fn filter_fuzzy_large_pool_is_bounded() {
    // Warm-up: the first `fuzzy_filter_pool` call anywhere in the process builds the thread-local
    // `Matcher` (~135KB, one-time), which would otherwise land on whichever of the two measurements
    // below runs first and skew the ratio (measured: without this, the first measurement absorbs
    // the ~135KB and the ratio drifts to ~1.87 instead of ~2.0).
    let warm_pool: Vec<Entry> = (0..10).map(|i| named_entry(&format!("w{i}.rs"))).collect();
    let _ = fuzzy_filter_pool(&warm_pool, "w");

    let pool_small: Vec<Entry> = (0..15_000)
        .map(|i| named_entry(&format!("module_{i}_helper_resolver.rs")))
        .collect();
    // 2x entries
    let pool_large: Vec<Entry> = (0..30_000)
        .map(|i| named_entry(&format!("module_{i}_helper_resolver.rs")))
        .collect();
    let small_alloc = crate::mem_tests::allocated_by(|| {
        let hits = fuzzy_filter_pool(&pool_small, "mhr");
        assert!(!hits.is_empty(), "少なくとも一部は一致するはず");
    });
    let large_alloc = crate::mem_tests::allocated_by(|| {
        let hits = fuzzy_filter_pool(&pool_large, "mhr");
        assert!(!hits.is_empty(), "少なくとも一部は一致するはず");
    });
    assert!(
        large_alloc < small_alloc.saturating_mul(3),
        "2倍の件数で確保バイト数が3倍を超えた(回帰: O(n^2)?): small={small_alloc} large={large_alloc}"
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
    // the tightest way to reproduce it. It reports a rebuild failure itself rather than returning
    // one (see `App::note_refresh_failure`), so "it succeeded" is asserted as "it had nothing to
    // report" instead of the `.unwrap()` this used to be.
    app.refresh_fs_after_tab_switch();
    assert_eq!(app.flash, None, "refresh 自体は成功している");

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
    let dir = unique_tmp("konoma_small_windowed_test");
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
    let dir = unique_tmp("konoma_linenum_test");
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
    let dir = unique_tmp("konoma_windowed_test");
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
    let dir = unique_tmp("konoma_tab_label_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("doc.md"), b"# hi\n").unwrap();
    let mut app = App::new(dir.canonicalize().unwrap(), Config::default()).unwrap();
    // Expected label is derived from `dir`'s own basename, not a literal — `dir` is a
    // `unique_tmp` fixture, so its basename carries a pid/counter suffix.
    let label = dir.file_name().unwrap().to_string_lossy().to_string();
    // While showing Tree, it's the root's directory name.
    assert_eq!(app.tab_label(0), label);
    // Preview doc.md → the tab name becomes the file name.
    let idx = app.tab.entries.iter().position(|e| !e.is_dir).unwrap();
    app.tab.selected = idx;
    app.tree_activate().unwrap();
    assert_eq!(app.tab.mode, Mode::Preview);
    assert_eq!(app.tab_label(0), "doc.md", "Preview 中はファイル名");
    // A new tab (Tree) uses the root name. The now-inactive original tab keeps the snapshotted
    // file name.
    app.tab_new().unwrap();
    assert_eq!(app.tab_label(1), label, "新規タブはルート名");
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
    let dir = unique_tmp("konoma_tab_preview_restore_test");
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
    let dir = unique_tmp("konoma_tabs_test");
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
    let dir = unique_tmp("konoma_tab_selection_perq_test");
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
    let dir = unique_tmp("konoma_refresh_test");
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
    let dir = unique_tmp("konoma_refresh_prune_sel");
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
    let dir = unique_tmp("konoma_refresh_reload_preview");
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
    let base = unique_tmp("konoma_out_of_root_watch");
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
    let base = unique_tmp("konoma_refresh_tree_fail");
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

/// Regression: a tree rebuild that fails on the **fs-watch driven refresh** used to be dropped with
/// `let _ = …`, so the user was shown a stale listing with no hint that it had stopped updating.
/// `rebuild_tree` keeps the last good state instead of crashing, which makes silence the worst
/// possible outcome — the tree looks perfectly healthy while being out of date.
///
/// The notification is edge-triggered on purpose (see `App::note_refresh_failure`): the watcher
/// fires on every burst, so reporting each failure would overwrite the flash line continuously.
/// This pins both halves — it *is* announced once, and it does *not* repeat — plus the success
/// cases, so that neither "never notify" (the old bug) nor "always notify" can pass.
///
/// No permission juggling, so no root guard is needed: a removed directory makes `read_dir` fail
/// for every user including root, and it is the realistic trigger anyway (an agent deleting a
/// directory konoma happens to be showing).
#[test]
fn fs_watch_refresh_failure_is_announced_once_per_outage() {
    let base = unique_tmp("konoma_stale_listing_watch_test");
    let _ = std::fs::remove_dir_all(&base);
    let root = base.join("root");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("a.txt"), b"x").unwrap();
    let mut app = App::new(root.clone(), Config::default()).unwrap();
    app.rebuild_tree().unwrap();
    let stale = crate::i18n::tr(app.lang, crate::i18n::Msg::ListingStale);

    // A healthy refresh stays silent (paired with the failure assertions below so an
    // "always flash" implementation cannot pass either).
    app.refresh_fs_watched(false, &[]);
    assert_eq!(app.flash, None, "成功時に余計な flash を出さない");

    // The root disappears from under us.
    std::fs::remove_dir_all(&root).unwrap();
    app.refresh_fs_watched(false, &[]);
    let first = app.flash.clone().expect("失敗が伝わる");
    assert!(first.contains(stale), "一覧が古い旨の文言が出る: {first:?}");
    assert!(
        !app.tab.entries.is_empty(),
        "クラッシュせず最後に成功した一覧を保持する(=だからこそ黙ってはいけない)"
    );
    assert!(app.tree_stale(), "一覧が古いという事実が立つ");

    // Repeated failures must not keep re-flashing: once the user dismisses it (a keypress clears
    // flash), the same outage stays quiet instead of burying every later message.
    app.flash = None;
    for _ in 0..5 {
        app.refresh_fs_watched(false, &[]);
    }
    assert_eq!(app.flash, None, "同じ障害の間は繰り返し通知しない");

    // An unrelated flash set in the meantime is not clobbered either.
    app.flash = Some("unrelated".into());
    app.refresh_fs_watched(false, &[]);
    assert_eq!(
        app.flash.as_deref(),
        Some("unrelated"),
        "他の flash を上書きしない"
    );

    // Recovery re-arms the notification, so a *new* outage is announced again.
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("b.txt"), b"y").unwrap();
    app.flash = None;
    app.refresh_fs_watched(false, &[]);
    assert_eq!(app.flash, None, "復旧時も余計な通知は出さない");
    assert!(!app.tree_stale(), "復旧で「古い」が解除される");
    std::fs::remove_dir_all(&root).unwrap();
    app.refresh_fs_watched(false, &[]);
    let second = app.flash.clone().expect("再発は改めて伝わる");
    assert!(second.contains(stale), "再発時も同じ文言: {second:?}");

    let _ = std::fs::remove_dir_all(&base);
}

/// The other path that cannot propagate a rebuild failure: **switching to a tab** whose root became
/// unreadable while it sat in the background. `load_active` returns nothing, so the error used to
/// be dropped with `let _ = self.refresh_fs_after_tab_switch();` and the tab silently showed its
/// old snapshot. Also pins the negative: switching to a healthy tab flashes nothing.
#[test]
fn tab_switch_announces_a_failed_rebuild() {
    let base = unique_tmp("konoma_stale_listing_tab_test");
    let _ = std::fs::remove_dir_all(&base);
    let a = base.join("a");
    let b = base.join("b");
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    std::fs::write(a.join("in-a.txt"), b"x").unwrap();
    std::fs::write(b.join("in-b.txt"), b"y").unwrap();

    let mut app = App::new(a.clone(), Config::default()).unwrap();
    app.rebuild_tree().unwrap();
    // Tab 2 on a different root.
    app.tab_new().unwrap();
    app.tab.root = b.clone();
    app.tab.entries.clear();
    app.tab.selected = 0;
    app.rebuild_tree().unwrap();

    // Healthy round trip: no flash in either direction.
    app.tab_goto(0);
    assert_eq!(app.flash, None, "健全なタブ切替では通知しない");
    app.tab_goto(1);
    assert_eq!(app.flash, None, "健全なタブ切替では通知しない(往路も)");

    // Tab 1's root vanishes while tab 2 is in front, then we switch back to it.
    std::fs::remove_dir_all(&a).unwrap();
    app.tab_goto(0);
    let msg = app.flash.clone().expect("タブ切替でも失敗が伝わる");
    assert!(
        msg.contains(crate::i18n::tr(app.lang, crate::i18n::Msg::ListingStale)),
        "一覧が古い旨の文言が出る: {msg:?}"
    );

    // Sitting on the broken tab, the watcher keeps firing — that is the flood source, and it stays
    // quiet.
    app.flash = None;
    for _ in 0..5 {
        app.refresh_fs_watched(false, &[]);
    }
    assert_eq!(app.flash, None, "同じ障害の間は繰り返し通知しない");

    // Deliberate: a round trip through a *healthy* tab rebuilds successfully, which re-arms the
    // latch, so deliberately visiting the broken tab again says so again. That is one flash per
    // explicit user action (konoma's normal flash cadence), not the unattended repetition the edge
    // trigger exists to stop — and the user just asked to look at that listing, so telling them it
    // is out of date is the point.
    app.flash = None;
    app.tab_goto(1);
    assert_eq!(app.flash, None, "健全なタブ側では黙っている");
    app.tab_goto(0);
    assert!(
        app.flash.is_some(),
        "壊れたタブを開き直したら改めて伝える(ユーザー操作 1 回につき 1 回)"
    );

    let _ = std::fs::remove_dir_all(&base);
}

/// The staleness level is maintained by `rebuild_tree` itself (the one choke point), not by the
/// reporting helper, so a recovery that happens on a *different* path — one that propagates its
/// error with `?` and never goes near the reporting helper — clears it too, and the next outage is
/// announced again. Without that, a successful `refresh()` between two outages would leave the
/// flag stuck and the second outage silent (and the chip stuck on over a healthy listing).
#[test]
fn stale_listing_flag_is_cleared_by_any_successful_rebuild() {
    let base = unique_tmp("konoma_stale_listing_rearm_test");
    let _ = std::fs::remove_dir_all(&base);
    let root = base.join("root");
    std::fs::create_dir_all(&root).unwrap();
    let mut app = App::new(root.clone(), Config::default()).unwrap();

    std::fs::remove_dir_all(&root).unwrap();
    app.refresh_fs_watched(false, &[]);
    assert!(app.flash.is_some(), "1回目の障害は伝わる");

    // Recover, and let a `?`-style caller be the one that succeeds.
    std::fs::create_dir_all(&root).unwrap();
    app.refresh().expect("復旧後は成功する");
    assert!(
        !app.tree_stale(),
        "報告ヘルパーを経由しない成功でも「古い」が解除される"
    );

    std::fs::remove_dir_all(&root).unwrap();
    app.flash = None;
    app.refresh_fs_watched(false, &[]);
    assert!(app.flash.is_some(), "2回目の障害も改めて伝わる");

    let _ = std::fs::remove_dir_all(&base);
}

/// The persistent chip is wired for both languages and — paired with that — appears **only** while
/// the listing really is out of date. Without the negative half, "always show the chip" would pass
/// every other assertion here.
///
/// The chip is a plain word rather than a warning glyph on purpose: `⚠` and friends are
/// East-Asian-Ambiguous width, so a CJK fallback font draws them two cells wide and shifts the
/// whole right-aligned bar (the trap `☐`/`☑` hit, and why the worktree chip is `WT`).
#[test]
fn stale_listing_chip_is_localized_and_only_shown_when_stale() {
    let base = unique_tmp("konoma_stale_chip_i18n_test");
    let _ = std::fs::remove_dir_all(&base);
    let root = base.join("root");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("a.txt"), b"x").unwrap();
    let mut app = App::new(root.clone(), Config::default()).unwrap();
    let bar = |app: &App| -> String {
        crate::ui::status::context_spans(app)
            .iter()
            .map(|s| s.content.as_ref())
            .collect()
    };

    // Healthy: nothing, in either language.
    for lang in [crate::i18n::Lang::En, crate::i18n::Lang::Jp] {
        app.lang = lang;
        let text = bar(&app);
        assert!(
            !text.contains("STALE"),
            "健全な一覧でチップは出ない: {text}"
        );
        assert!(
            !text.contains("古い一覧"),
            "健全な一覧でチップは出ない: {text}"
        );
    }

    std::fs::remove_dir_all(&root).unwrap();
    app.refresh_fs_watched(false, &[]);
    assert!(app.tree_stale());

    app.lang = crate::i18n::Lang::En;
    let en = bar(&app);
    assert!(en.contains("STALE"), "en のチップ文言: {en}");
    app.lang = crate::i18n::Lang::Jp;
    let jp = bar(&app);
    assert!(jp.contains("古い一覧"), "jp のチップ文言: {jp}");
    assert!(!jp.contains("STALE"), "jp では英語のまま出ない: {jp}");

    // Recovery takes it down in both languages too.
    std::fs::create_dir_all(&root).unwrap();
    app.refresh_fs_watched(false, &[]);
    for lang in [crate::i18n::Lang::En, crate::i18n::Lang::Jp] {
        app.lang = lang;
        let text = bar(&app);
        assert!(!text.contains("STALE"), "復旧後は消える: {text}");
        assert!(!text.contains("古い一覧"), "復旧後は消える: {text}");
    }

    let _ = std::fs::remove_dir_all(&base);
}

/// The Git full-screen views bypass `ui/tree.rs` entirely, so an indicator that only reached the
/// tree would silently stop existing there. The chip is drawn by `context_spans`, which every mode
/// shares — pinned here through a real full-screen draw of the Git view, since "it is in the shared
/// helper" is exactly the kind of assumption that quietly stops being true.
#[cfg(feature = "git")]
#[test]
fn stale_listing_chip_shows_in_the_git_view() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    let base = unique_tmp("konoma_stale_chip_git_test");
    let _ = std::fs::remove_dir_all(&base);
    let root = base.join("repo");
    std::fs::create_dir_all(&root).unwrap();
    init_git_repo(&root);
    std::fs::write(root.join("tracked.txt"), b"one\n").unwrap();

    let mut app = App::new(root.clone(), Config::default()).unwrap();
    app.lang = crate::i18n::Lang::En;
    app.refresh_git_if_needed();
    app.open_git_view();
    assert!(app.in_git_view(), "Git ビューに入れている");

    let screen = |app: &mut App| -> String {
        let mut term = Terminal::new(TestBackend::new(80, 20)).unwrap();
        term.draw(|f| crate::ui::render(f, app)).unwrap();
        term.backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect()
    };
    assert!(
        !screen(&mut app).contains("STALE"),
        "健全なうちは Git ビューにも出ない"
    );

    std::fs::remove_dir_all(&root).unwrap();
    app.refresh_fs_watched(false, &[]);
    assert!(app.tree_stale());
    let text = screen(&mut app);
    assert!(
        text.contains("STALE"),
        "Git ビューでもチップが出る:\n{text}"
    );

    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn media_preview_reloads_only_when_file_changes() {
    // An image/media preview only reloads when the target file actually changes (an mtime
    // guard). What we observe: media_gen, which clear_image advances, and the reference
    // timestamp preview_media_mtime.
    // (Actual decoding requires a picker = a no-op in tests. We judge whether the reload path
    // ran by media_gen.)
    use image::RgbImage;
    let dir = unique_tmp("konoma_media_reload_test");
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
    let dir = unique_tmp("konoma_sort_test");
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
    let dir = unique_tmp("konoma_sort_config_test");
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
    let base = unique_tmp("konoma_bm_app_base");
    let _ = std::fs::remove_dir_all(&base);
    let proj = unique_tmp("konoma_bm_app_proj");
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
    let dir = unique_tmp("konoma_descend_test");
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

/// `ui.tree_cursor` default (`"origin"`): ascending (`h`) out of a directory that was not the
/// first row puts the cursor back on that directory, not on row 0. Regression guard for the old
/// unconditional `self.tab.selected = 0`, which this test would catch (it fails if `tree_leave`
/// stops restoring the cursor).
#[test]
fn ascend_puts_the_cursor_on_the_directory_just_left() {
    let dir = unique_tmp("konoma_ascend_origin_test");
    std::fs::create_dir_all(dir.join("alpha")).unwrap();
    std::fs::create_dir_all(dir.join("beta")).unwrap();
    std::fs::create_dir_all(dir.join("gamma")).unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    let i = app
        .tab
        .entries
        .iter()
        .position(|e| e.is_dir && e.path.ends_with("beta"))
        .expect("beta が無い");
    assert_ne!(
        i, 0,
        "土台: beta は一覧の先頭ではない(name昇順・dirs_firstでも alpha が先)"
    );
    app.tab.selected = i;
    app.tree_descend().unwrap();
    assert_eq!(app.tab.root, dir.join("beta"), "root が beta になる");

    app.tree_leave().unwrap();
    assert_eq!(app.tab.root, dir, "root は親に戻る");
    let expect_idx = app
        .tab
        .entries
        .iter()
        .position(|e| e.path.ends_with("beta"))
        .expect("戻った先の一覧に beta が無い");
    assert_eq!(
        app.tab.selected, expect_idx,
        "カーソルは出てきた beta の行に乗る"
    );
    assert_ne!(app.tab.selected, 0, "先頭(alpha)に飛んではいけない");
    std::fs::remove_dir_all(&dir).ok();
}

/// `ui.tree_cursor = "top"` restores the legacy behavior: `h` always lands on row 0 regardless of
/// which directory was left.
#[test]
fn ascend_with_tree_cursor_top_goes_back_to_the_first_row() {
    let dir = unique_tmp("konoma_ascend_top_test");
    std::fs::create_dir_all(dir.join("alpha")).unwrap();
    std::fs::create_dir_all(dir.join("beta")).unwrap();
    let mut cfg = Config::default();
    cfg.ui.tree_cursor = "top".into();
    let mut app = App::new(dir.clone(), cfg).unwrap();
    let i = app
        .tab
        .entries
        .iter()
        .position(|e| e.is_dir && e.path.ends_with("beta"))
        .expect("beta が無い");
    app.tab.selected = i;
    app.tree_descend().unwrap();
    app.tree_leave().unwrap();
    assert_eq!(app.tab.root, dir, "root は親に戻る");
    assert_eq!(app.tab.selected, 0, "tree_cursor=\"top\" は常に先頭に戻す");
    std::fs::remove_dir_all(&dir).ok();
}

/// When the directory just left is not present in the parent's rebuilt listing (here: it is
/// hidden and `show_hidden` was turned off in between), `h` falls back to row 0 instead of
/// pointing at a stale/nonexistent index.
#[test]
fn ascend_falls_back_to_the_top_when_the_directory_it_left_is_not_listed() {
    let dir = unique_tmp("konoma_ascend_hidden_fallback_test");
    std::fs::create_dir_all(dir.join(".secret")).unwrap();
    std::fs::create_dir_all(dir.join("visible")).unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    // Reveal the hidden directory just long enough to descend into it.
    app.tab.show_hidden = true;
    app.rebuild_tree().unwrap();
    let i = app
        .tab
        .entries
        .iter()
        .position(|e| e.is_dir && e.path.ends_with(".secret"))
        .expect(".secret が無い");
    app.tab.selected = i;
    app.tree_descend().unwrap();
    assert_eq!(app.tab.root, dir.join(".secret"));

    // Hide dotfiles again before ascending: the parent listing tree_leave rebuilds will no longer
    // contain `.secret`, so the by-path lookup must fail closed to row 0 rather than panic or
    // silently leave a stale index.
    app.tab.show_hidden = false;
    app.tree_leave().unwrap();
    assert_eq!(app.tab.root, dir, "root は親に戻る");
    assert_eq!(
        app.tab.selected, 0,
        "戻り先の一覧に無ければ先頭にフォールバック"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// Regression: descending (`l`) is untouched by `ui.tree_cursor` — it always starts at the top of
/// the newly-entered directory, since there is no "came from" row to return to there.
#[test]
fn descend_still_starts_at_the_top_of_the_new_directory() {
    let dir = unique_tmp("konoma_descend_top_test");
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    std::fs::write(dir.join("sub").join("a_first.txt"), b"x").unwrap();
    std::fs::write(dir.join("sub").join("z_last.txt"), b"x").unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    let i = app
        .tab
        .entries
        .iter()
        .position(|e| e.is_dir && e.path.ends_with("sub"))
        .expect("sub が無い");
    app.tab.selected = i;
    app.tree_descend().unwrap();
    assert_eq!(app.tab.root, dir.join("sub"));
    assert_eq!(app.tab.selected, 0, "l は常に先頭から始まる");
    std::fs::remove_dir_all(&dir).ok();
}

/// An unrecognized `tree_cursor` value (a typo) must not silently disable the new behavior —
/// permissive fallback treats it the same as `"origin"` (only the literal `"top"` opts out).
#[test]
fn ascend_treats_an_unknown_tree_cursor_value_as_origin() {
    let dir = unique_tmp("konoma_ascend_bogus_test");
    std::fs::create_dir_all(dir.join("alpha")).unwrap();
    std::fs::create_dir_all(dir.join("beta")).unwrap();
    let mut cfg = Config::default();
    cfg.ui.tree_cursor = "bogus".into();
    let mut app = App::new(dir.clone(), cfg).unwrap();
    let i = app
        .tab
        .entries
        .iter()
        .position(|e| e.is_dir && e.path.ends_with("beta"))
        .expect("beta が無い");
    app.tab.selected = i;
    app.tree_descend().unwrap();
    app.tree_leave().unwrap();
    let expect_idx = app
        .tab
        .entries
        .iter()
        .position(|e| e.path.ends_with("beta"))
        .expect("戻った先の一覧に beta が無い");
    assert_eq!(
        app.tab.selected, expect_idx,
        "未知の値は origin として扱われる"
    );
    assert_ne!(app.tab.selected, 0);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn jump_to_dir_clears_selection_on_root_change() {
    // bug #2: changing root used to leave the old root's selection lingering by path, invisibly
    // targetable for a mis-operation since the marker isn't visible. jump_to_dir (a bookmark
    // jump) must also call clear_for_root_change so it isn't carried over.
    let base = unique_tmp("konoma_jump_clear_test");
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
    let work = unique_tmp("konoma_copy_match_title");
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

/// The scroll position indicator (title marker + border-column thumb) must stay off the
/// image-family views — image/PDF/video/SVG/full-screen mermaid all render through `render_image`.
///
/// Two reasons, both concrete: those cells carry kitty graphics Unicode placeholders, and drawing
/// anything else into or beside them is what exposes an image-ID-colored bar (chased down three
/// separate times here); and they don't scroll at all — zoom (`x1.6`) and PDF paging (`2/3`) are
/// what report position for them.
#[test]
fn image_preview_draws_no_scroll_indicator() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    let mut app = app_with_image();
    app.tab.preview_path = Some(PathBuf::from("x.png"));
    app.tab.mode = Mode::Preview;
    assert!(app.is_image_preview(), "画像ビューに入っている");

    let mut term = Terminal::new(TestBackend::new(40, 12)).unwrap();
    term.draw(|f| crate::ui::preview::render(f, &mut app, f.area()))
        .unwrap();
    let buf = term.backend().buffer();

    // The right border column is where the thumb would land: every cell between the corners must
    // still be plain border.
    let x = buf.area.width - 1;
    for y in 1..(buf.area.height - 1) {
        assert_eq!(
            buf[(x, y)].symbol(),
            "│",
            "画像ビューの右端は枠のまま (row {y})"
        );
    }
    let s: String = buf.content().iter().map(|c| c.symbol()).collect();
    for marker in ["[Top]", "[Bot]", "[All]", "%]"] {
        assert!(
            !s.contains(marker),
            "画像ビューに位置ラベル {marker} は出さない: {s:?}"
        );
    }
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
    let dir = unique_tmp("konoma_hl_off_test");
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
    let dir = unique_tmp("konoma_hl_warm_test");
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
    let dir = unique_tmp("konoma_loading_mode_test");
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    assert!(app.loading_is_indicator(), "既定は indicator");
    app.cfg.ui.preview_loading = "progressive".into();
    assert!(!app.loading_is_indicator(), "progressive 指定で素表示");
}

#[test]
fn request_edit_targets_selected_file_or_warns_on_dir() {
    let dir = unique_tmp("konoma_edit_target_test");
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
    let dir = unique_tmp("konoma_edit_preview_test");
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
    let dir = unique_tmp("konoma_spinner_test");
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
    let dir = unique_tmp("konoma_prog_plain_test");
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
    let dir = unique_tmp("konoma_preview_search_test");
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
    let dir = unique_tmp("konoma_follow_baseline_core");
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
    let dir = unique_tmp("konoma_follow_baseline_clean");
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
    let dir = unique_tmp("konoma_follow_baseline_big");
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
    let dir = unique_tmp("konoma_follow_non_repo");
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
    let dir = unique_tmp("konoma_follow_recapture");
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
    let dir = unique_tmp("konoma_graph_config_base");
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
    let dir = unique_tmp("konoma_ignored_reuse_same");
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
    let dir = unique_tmp("konoma_back_to_tree_resync");
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
    let dir = unique_tmp("konoma_ignored_reuse_diff");
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
    let dir = unique_tmp("konoma_busy_indicator_test");
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
    let dir = unique_tmp("konoma_busy_gitscan_test");
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
    let dir = unique_tmp("konoma_apply_ignored_gen");
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

/// D2 (2026-08-05): `spawn_or_sync_ignored`'s worker now sends a fallback `IgnoredResult` (an empty
/// set, but the correct gen/workdir) if the scan panics, instead of the old "send nothing on
/// failure" shape that left `git_ignored_pending` latched forever (a spinner that never stops).
/// This fixes `apply_ignored` at the boundary that actually matters: feeding it that exact
/// fallback shape must still clear `pending` (the panic-catching itself is exercised for real by
/// `preview::markdown::tests::catch_silent_returns_none_on_panic_and_some_on_success`, since a real
/// git-scan panic can't be induced on demand from a test). The stale-generation case (a slow
/// panic-catch racing a newer request) must still be discarded exactly as before.
#[test]
fn apply_ignored_with_a_panic_shaped_result_still_clears_pending() {
    let dir = unique_tmp("konoma_apply_ignored_panic_shaped");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.git_ignored_gen = 3;
    app.git_ignored_pending = Some(dir.clone());
    // Something was already shown from a previous (successful) scan — the failure result
    // legitimately replaces it, since the panic means "we don't actually know the ignore set now."
    app.git_ignored.insert(dir.join("stale-entry"));

    // Current-generation fallback (mirrors what the worker sends on a caught panic): applies and
    // clears pending, same as a real successful scan would.
    let panic_fallback = IgnoredResult {
        gen: 3,
        workdir: dir.clone(),
        set: Default::default(),
    };
    assert!(app.apply_ignored(panic_fallback), "現世代なので適用される");
    assert!(
        app.git_ignored_pending.is_none(),
        "パニックのフォールバック結果でも pending が解ける(スピナーが固着しない)"
    );
    assert!(app.git_ignored.is_empty(), "失敗は安全側=空集合に倒す");

    // A stale-generation fallback (the panic was caught for a request that's since been superseded)
    // must still be discarded — the unconditional send doesn't bypass the generation guard.
    app.git_ignored_gen = 4;
    app.git_ignored_pending = Some(dir.clone());
    let stale_panic_fallback = IgnoredResult {
        gen: 3,
        workdir: dir.clone(),
        set: Default::default(),
    };
    assert!(
        !app.apply_ignored(stale_panic_fallback),
        "古い世代のフォールバックは捨てる"
    );
    assert!(
        app.git_ignored_pending.is_some(),
        "stale では pending を残す(現行計算待ちのまま)"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[cfg(feature = "git")]
#[test]
fn graph_legend_caps_branches_head_first_and_picker_toggles() {
    use std::process::Command;
    let dir = unique_tmp("konoma_graph_legend_cap_picker");
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
    let dir = unique_tmp("konoma_app_git_view");
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
    let dir = unique_tmp("konoma_refresh_git_view_refetch");
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
    let dir = unique_tmp("konoma_app_git_stage_all");
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
    let dir = unique_tmp("konoma_app_gitdiff_roundtrip");
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
    let dir = unique_tmp("konoma_app_gitdiff_discard");
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
    let dir = unique_tmp("konoma_app_git_view_norepo");
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
    let dir = unique_tmp("konoma_app_git_branch");
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
    let dir = unique_tmp("konoma_app_git_commit_flow");
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
    let dir = unique_tmp("konoma_app_git_commit_empty");
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
    let dir = unique_tmp("konoma_app_git_log_detail");
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
    let dir = unique_tmp("konoma_app_git_log_unborn");
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

    let dir = unique_tmp("konoma_cycle_pathstyle_test");
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
    let dir = unique_tmp("konoma_toggle_hidden_app_test");
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
    let dir = unique_tmp("konoma_sortmenu_close_test");
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
    let dir = unique_tmp("konoma_looks_like_gif_test");
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
    let dir = unique_tmp("konoma_keymap_report_test");
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
    let dir = unique_tmp("konoma_take_warm_job_test");
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
    let dir = unique_tmp("konoma_launch_git_tool_test");
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
    let dir = unique_tmp("konoma_preview_scroll_test");
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
    let dir = unique_tmp("konoma_preview_hscroll_test");
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
    let dir = unique_tmp("konoma_windowed_lines_test");
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
    let dir = unique_tmp("konoma_edit_line_test");
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
    let dir = unique_tmp("konoma_preview_visual_test");
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
    let dir = unique_tmp("konoma_preview_charwise_test");
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
    let dir = unique_tmp("konoma_md_raw_test");
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
    let dir = unique_tmp("konoma_dialog_cursor_re_test");
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
    // These exercise the advance/poll mechanism itself, which only runs while a preview is up:
    // `md_gif_poll_timeout`/`advance_md_gifs_if_due` return early outside `Mode::Preview` so that
    // leaving a preview stops the animation (and the wakeups it needs) instead of driving redraws
    // of a tree nothing is moving on. `App::new` starts in Tree, so put the app in the mode the
    // mechanism belongs to rather than asserting from a state the feature never runs in.
    app.tab.mode = Mode::Preview;

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
            protocol: Some(crate::app::InlineImage::Proto(proto)),
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
    // These exercise the advance/poll mechanism itself, which only runs while a preview is up:
    // `md_gif_poll_timeout`/`advance_md_gifs_if_due` return early outside `Mode::Preview` so that
    // leaving a preview stops the animation (and the wakeups it needs) instead of driving redraws
    // of a tree nothing is moving on. `App::new` starts in Tree, so put the app in the mode the
    // mechanism belongs to rather than asserting from a state the feature never runs in.
    app.tab.mode = Mode::Preview;

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

// ---- Inline images on a kitty terminal: fixed ids + compressed transmit -----------------------

/// A `Picker` forced to kitty graphics (no real terminal needed), used to drive the inline-image
/// path down the branch a Ghostty/kitty user actually takes.
fn kitty_picker() -> ratatui_image::picker::Picker {
    let mut p = ratatui_image::picker::Picker::halfblocks();
    p.set_protocol_type(ratatui_image::picker::ProtocolType::Kitty);
    p
}

/// Every kitty image id **transmitted** into `buf` (`i=<id>,a=T` is the header of a transmit
/// escape). This is the quantity the terminal accounts its image-storage budget against, so it is
/// the honest observable for "konoma stops filling the terminal up".
fn transmitted_kitty_ids(buf: &ratatui::buffer::Buffer) -> Vec<u32> {
    let dump: String = buf.content.iter().map(|c| c.symbol()).collect();
    dump.split("i=")
        .skip(1)
        .filter_map(|part| part.split_once(",a=T"))
        .filter_map(|(id, _)| id.parse::<u32>().ok())
        .collect()
}

/// An App on a kitty terminal with a real encode worker, plus a pre-decoded inline-GIF cache entry
/// of `colors.len()` frames (zero frame delays, so advancing is driven by calls, not by the clock).
/// Returns the app, the url used as the cache key, the encode-result receiver, and the sandbox dir.
fn inline_gif_app(
    name: &str,
    colors: &[[u8; 3]],
) -> (
    App,
    String,
    std::sync::mpsc::Receiver<MdEncodeResult>,
    PathBuf,
) {
    let dir = unique_tmp(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // `ensure_md_image` resolves a non-synthetic url on disk before encoding it, so the key has to
    // name a real file. Its contents are never read here: the entry below stands in for a decode
    // that already happened.
    let file = dir.join("anim.gif");
    std::fs::write(&file, b"GIF89a").unwrap();

    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    let (resize_tx, resize_rx) = tokio::sync::mpsc::unbounded_channel();
    Box::leak(Box::new(resize_rx));
    app.attach_image_backend(kitty_picker(), resize_tx);
    assert!(app.uses_kitty_image(), "前提: kitty 端末として扱われる");

    let (req_tx, req_rx) = std::sync::mpsc::channel();
    let (res_tx, res_rx) = std::sync::mpsc::channel();
    let picker = kitty_picker();
    std::thread::spawn(move || md_encode_worker(picker, req_rx, res_tx));
    app.attach_md_encoder(req_tx);

    let frames: Vec<_> = colors
        .iter()
        .map(|c| {
            let img = image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
                200,
                100,
                image::Rgb(*c),
            ));
            (std::sync::Arc::new(img), std::time::Duration::ZERO)
        })
        .collect();
    app.tab.mode = Mode::Preview;
    app.md_image_cache.insert(
        file.clone(),
        MdImgEntry {
            decoded: Some(frames[0].0.clone()),
            frames,
            ..Default::default()
        },
    );
    (app, file.to_string_lossy().to_string(), res_rx, dir)
}

/// **The bug**: an animated inline GIF re-encodes its picture on every frame, and each re-encode
/// used to go through `Picker::new_protocol`, which picks a kitty image id with `rand::random()`.
/// A fresh id per frame is a fresh *image* to the terminal — nothing is ever replaced and konoma
/// never deletes anything — so a Markdown document with a GIF in it pushed ~66 MB/min at Ghostty's
/// 320 MB image budget and, about five minutes in, got a still image evicted out from under
/// placeholder cells konoma would never re-transmit: the still simply disappeared.
///
/// The fix pins one id per cache-entry slot and reuses it, which makes each later transmit a
/// *replacement* (kitty: "when re-transmitting image data for a specific id, the existing image and
/// all its placements must be deleted"). So the observable is not a byte count or a timing — it is
/// that cycling the animation many times over never introduces a second id.
///
/// Deliberately driven by calls rather than by the clock (zero frame delays): this has to fail on
/// the old code for a structural reason, not because a slow machine got fewer frames in.
#[test]
fn inline_gif_animation_never_transmits_a_second_kitty_image_id() {
    let colors = [[255, 0, 0], [0, 255, 0], [0, 0, 255]];
    let (mut app, url, res_rx, dir) = inline_gif_app("konoma_md_gif_kitty_id_test", &colors);
    let (cols, rows) = (20u16, 5u16);
    let area = Rect::new(0, 0, cols, rows);

    let mut ids = std::collections::BTreeSet::new();
    let mut transmits = 0usize;
    let mut dumps = Vec::new();
    // The very first tick only starts the frame timer (frame 0 is already the shown one).
    assert!(!app.advance_md_gifs_if_due(), "最初の tick は計時開始だけ");
    // Three full laps of a three-frame animation.
    for _ in 0..(colors.len() * 3) {
        app.ensure_md_image(&url, cols, rows, 0, rows);
        let res = res_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("encode worker が結果を返す");
        assert!(app.apply_md_encode(res));

        let mut buf = ratatui::buffer::Buffer::empty(area);
        let img = app
            .md_image_proto(&url, cols, rows, 0, rows)
            .expect("エンコード済みの画像が描ける");
        assert!(img.is_kitty(), "kitty 端末では konoma 自前の転送を使う");
        img.render(area, &mut buf);

        let frame_ids = transmitted_kitty_ids(&buf);
        transmits += frame_ids.len();
        ids.extend(frame_ids);
        dumps.push(buf.content.iter().map(|c| c.symbol()).collect::<String>());

        assert!(app.advance_md_gifs_if_due(), "遅延0なので毎回コマが進む");
    }

    // Vacuity guard: the assertion below is only meaningful if every frame really did re-transmit.
    assert_eq!(transmits, colors.len() * 3, "毎コマ転送が起きている");
    assert_eq!(
        ids.len(),
        1,
        "コマを何周させても端末に載る画像 ID は1つのまま(= 端末が埋まらない): {ids:?}"
    );
    // ...and the animation still animates: consecutive frames carry different pixels.
    assert_ne!(dumps[0], dumps[1], "コマが変われば転送データも変わる");
    assert_eq!(
        dumps[0],
        dumps[colors.len()],
        "一周すると同じコマ=同じデータに戻る"
    );

    // Nothing accumulates in konoma either: one entry holds exactly one encoded full-image slot,
    // whose payload stays one frame's worth however long it has been playing.
    let entry = &app.md_image_cache[&PathBuf::from(&url)];
    let held = entry
        .protocol
        .as_ref()
        .expect("full スロットの画像")
        .transmit_len();
    assert!(entry.clip_protocol.is_none() && entry.zoom_protocol.is_none());
    assert!(
        held < 64 * 1024,
        "常駐はコマ1枚ぶんだけ(コマ数に比例しない): {held} bytes"
    );
    assert_eq!(entry.frames.len(), colors.len(), "デコード済みコマ数は不変");

    std::fs::remove_dir_all(&dir).ok();
}

/// The other half of the fix: on a kitty terminal the inline path now sends the pixels
/// **zlib-compressed** (`o=z`), which is what makes re-encoding a frame several times a second
/// affordable at all. Measured against the very thing it replaced — the same encode request with
/// the ratatui-image kitty protocol, whose payload is raw RGBA.
#[test]
fn inline_kitty_transmit_is_much_smaller_than_the_uncompressed_protocol() {
    let picker = kitty_picker();
    let (req_tx, req_rx) = std::sync::mpsc::channel();
    let (res_tx, res_rx) = std::sync::mpsc::channel();
    let h = std::thread::spawn(move || md_encode_worker(picker, req_rx, res_tx));
    // Screenshot/diagram-like content: large flat regions, which is what konoma is usually asked
    // to show next to an agent.
    let mut img = image::RgbaImage::new(600, 300);
    for (x, y, p) in img.enumerate_pixels_mut() {
        let on = (x / 40 + y / 40) % 2 == 0;
        *p = image::Rgba(if on {
            [32, 36, 44, 255]
        } else {
            [200, 205, 215, 255]
        });
    }
    let img = std::sync::Arc::new(image::DynamicImage::ImageRgba8(img));
    for kitty in [None, Some((4321u32, false))] {
        req_tx
            .send(MdEncodeRequest {
                path: std::path::PathBuf::from("/tmp/shot.png"),
                key: MdEncodeKey::Full { cols: 60, rows: 15 },
                image: img.clone(),
                crop: None,
                cols: 60,
                rows: 15,
                kitty,
            })
            .unwrap();
    }
    drop(req_tx);
    let old = res_rx.recv().unwrap().image.expect("旧経路のエンコード");
    let new = res_rx.recv().unwrap().image.expect("新経路のエンコード");
    h.join().unwrap();
    assert!(!old.is_kitty() && new.is_kitty(), "比較対象の取り違え防止");
    assert_eq!(old.cell_size(), new.cell_size(), "表示セル数は同じ");

    // Bytes that actually reach the terminal on the transmit frame.
    let bytes = |i: &crate::app::InlineImage| {
        let area = Rect::new(0, 0, 60, 15);
        let mut buf = ratatui::buffer::Buffer::empty(area);
        i.render(area, &mut buf);
        buf.content.iter().map(|c| c.symbol().len()).sum::<usize>()
    };
    let (old_bytes, new_bytes) = (bytes(&old), bytes(&new));
    assert!(
        new_bytes * 5 < old_bytes,
        "圧縮転送は非圧縮より桁違いに小さい: {new_bytes} vs {old_bytes} bytes"
    );
}

/// The ids are fixed **per slot**, and the three slots must not share one.
///
/// Pinning one id per *entry* would look like it works and then corrupt the display while
/// scrolling: the renderer keeps drawing the full-image encode until the freshly scrolled band's
/// encode lands, so the two are on screen across consecutive frames — and a transmit on a shared id
/// deletes whatever that id was showing. The band would silently take over the cells the full image
/// is still drawn from. Conversely each slot must *keep* its id across re-encodes, which is the
/// property that stops the terminal filling up.
#[test]
fn each_inline_image_slot_keeps_its_own_fixed_kitty_id() {
    let (mut app, url, res_rx, dir) =
        inline_gif_app("konoma_md_inline_slot_id_test", &[[10, 20, 30]]);
    let key = PathBuf::from(&url);
    let (cols, full_rows) = (20u16, 8u16);

    // Fully visible → the Full slot.
    app.ensure_md_image(&url, cols, full_rows, 0, full_rows);
    let full_req_id = app.md_image_cache[&key].kitty_ids[0].expect("full スロットに ID");
    assert!(app.apply_md_encode(res_rx.recv().unwrap()));

    // Scrolled so only the lower band shows → the Clip slot, which needs an id of its own.
    app.ensure_md_image(&url, cols, full_rows, 3, 5);
    let clip_id = app.md_image_cache[&key].kitty_ids[1].expect("clip スロットに ID");
    assert!(app.apply_md_encode(res_rx.recv().unwrap()));

    // The in-place zoom of a focused diagram → the Zoom slot.
    app.tab.fence_zoom = 2.0;
    app.ensure_md_fence_zoom(&url, cols, full_rows);
    let zoom_id = app.md_image_cache[&key].kitty_ids[2].expect("zoom スロットに ID");
    assert!(app.apply_md_encode(res_rx.recv().unwrap()));

    let ids = [full_req_id, clip_id, zoom_id];
    assert_eq!(
        std::collections::BTreeSet::from(ids).len(),
        3,
        "3スロットは別々の ID を持つ(同一だと転送が互いの表示を消す): {ids:?}"
    );

    // Re-encoding any slot reuses that slot's id rather than taking a new one.
    let entry = app.md_image_cache.get_mut(&key).unwrap();
    entry.proto_size = None; // what a GIF frame advance does
    entry.clip_key = None;
    app.ensure_md_image(&url, cols, full_rows, 0, full_rows);
    assert_eq!(
        app.md_image_cache[&key].kitty_ids,
        [Some(full_req_id), Some(clip_id), Some(zoom_id)],
        "再エンコードでは ID を取り直さない"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// Non-kitty terminals (sixel / iTerm2 / halfblocks) are untouched: they write the image as cell
/// content, so the terminal keeps nothing between frames and there is nothing to accumulate. The
/// request must not carry a kitty id, and the result must be the ratatui-image protocol.
#[test]
fn inline_images_on_a_non_kitty_terminal_keep_the_ratatui_image_path() {
    let dir = unique_tmp("konoma_md_inline_nonkitty_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("pic.png");
    std::fs::write(&file, b"not really a png").unwrap();

    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    let (resize_tx, resize_rx) = tokio::sync::mpsc::unbounded_channel();
    Box::leak(Box::new(resize_rx));
    app.attach_image_backend(test_picker(), resize_tx); // halfblocks
    assert!(!app.uses_kitty_image());

    let (req_tx, req_rx) = std::sync::mpsc::channel();
    app.attach_md_encoder(req_tx);
    app.md_image_cache.insert(
        file.clone(),
        MdImgEntry {
            decoded: Some(std::sync::Arc::new(image::DynamicImage::new_rgba8(
                200, 100,
            ))),
            ..Default::default()
        },
    );
    app.ensure_md_image(&file.to_string_lossy(), 20, 5, 0, 5);
    let req = req_rx.try_recv().expect("エンコードを要求する");
    assert!(req.kitty.is_none(), "非 kitty 端末では固定 ID を渡さない");
    // ...and no id was burned for a slot that will never use one.
    assert_eq!(app.md_image_cache[&file].kitty_ids, [None, None, None]);

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
    let dir = unique_tmp("konoma_attach_detach_img_test");
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
    let dir = unique_tmp("konoma_load_image_test");
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
    let base = unique_tmp("konoma_open_link_target_test");
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
    let dir = unique_tmp("konoma_copy_target_test");
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
    let dir = unique_tmp("konoma_tab_label_name_test").join("proj");
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
    let dir = unique_tmp("konoma_current_commit_meta_test");
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
    let dir = unique_tmp("konoma_git_copy_flash_test");
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
    let dir = unique_tmp("konoma_git_copy_branch_test");
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
    let dir = unique_tmp("konoma_git_view_unstage_test");
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
    let dir = unique_tmp("konoma_git_view_start_discard_test");
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
    let dir = unique_tmp("konoma_branch_filter_commit_test");
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
    let dir = unique_tmp("konoma_graph_base_detail_scroll_test");
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
    let dir = unique_tmp("konoma_graph_picker_ops_test");
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
    let dir = unique_tmp("konoma_close_git_graph_test");
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
    let dir = unique_tmp("konoma_mark_state_test");
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
    let root = unique_tmp("konoma_bm_preview_test");
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
    let dir = unique_tmp("konoma_bm_display_test");
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
    let root = unique_tmp("konoma_bm_overwrite_confirm");
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
    let root = unique_tmp("konoma_bm_overwrite_skip");
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
    let dir = unique_tmp("konoma_tab_list_test");
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
    let root = unique_tmp("konoma_bm_list_ops_test");
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
    let dir = unique_tmp("konoma_attach_git_loader_test");
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
    let dir = unique_tmp("konoma_refresh_status_only_test");
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
    let dir = unique_tmp("konoma_op_base_dir_test");
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
    let base = unique_tmp("konoma_duplicate_test");
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

/// Regression: when a batch rename failed **and could not undo itself**, the user was shown only
/// the inner reason. `describe_error` matched on `FileOpError` with a `downcast_ref`, and anyhow's
/// downcast reaches straight through the outer context to find it — so the "rollback also failed"
/// news attached on top was dropped without trace, and nothing said that `.konoma-rename-tmp-*`
/// files were still lying around waiting to be cleaned up by hand.
///
/// Both halves must survive, in both languages: the reason **and** the leftovers.
#[test]
fn describe_error_keeps_the_rollback_context_around_the_reason() {
    use crate::fileops::{FileOpError, RollbackIncomplete};
    use crate::i18n::{tr, Lang, Msg};

    let dir = unique_tmp("konoma_rollback_describe_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();

    let failed_at = dir.join("b.txt");
    let left_a = dir.join(".konoma-rename-tmp-0");
    let left_b = dir.join(".konoma-rename-tmp-1");
    // Exactly the shape `batch_rename` produces: the reason, with the rollback news layered on top.
    let err = anyhow::Error::new(FileOpError::RenameCommitFailed(failed_at.clone())).context(
        RollbackIncomplete {
            leftovers: vec![left_a.clone(), left_b.clone()],
        },
    );

    for lang in [Lang::En, Lang::Jp] {
        app.lang = lang;
        let msg = app.describe_error(&err);
        assert!(
            msg.contains(tr(lang, Msg::RenameCommitFailed)),
            "内側の理由が残る({lang:?}): {msg:?}"
        );
        assert!(
            msg.contains(&failed_at.display().to_string()),
            "理由が指すパスも残る({lang:?}): {msg:?}"
        );
        assert!(
            msg.contains(tr(lang, Msg::RollbackIncomplete)),
            "巻き戻し失敗が伝わる({lang:?}): {msg:?}"
        );
        assert!(
            msg.contains(&left_a.display().to_string())
                && msg.contains(&left_b.display().to_string()),
            "残ったファイルが全部わかる({lang:?}): {msg:?}"
        );
    }

    // Literal wording, so this can't pass on a catalog that is itself wrong.
    app.lang = Lang::En;
    assert_eq!(
        app.describe_error(&err),
        format!(
            "committing rename: {} / rollback also failed, clean up by hand: {}, {}",
            failed_at.display(),
            left_a.display(),
            left_b.display()
        )
    );
    app.lang = Lang::Jp;
    assert_eq!(
        app.describe_error(&err),
        format!(
            "一括リネーム(確定): {} / 巻き戻しにも失敗、手で片付けてください: {}, {}",
            failed_at.display(),
            left_a.display(),
            left_b.display()
        )
    );

    // The pair: a failure whose rollback *did* succeed says nothing about leftovers, so this can't
    // be passed by unconditionally appending the warning.
    let clean = anyhow::Error::new(FileOpError::RenameCommitFailed(failed_at.clone()));
    for lang in [Lang::En, Lang::Jp] {
        app.lang = lang;
        assert_eq!(
            app.describe_error(&clean),
            format!(
                "{}{}",
                tr(lang, Msg::RenameCommitFailed),
                failed_at.display()
            ),
            "巻き戻しが成功した場合は従来どおりの文言({lang:?})"
        );
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

/// The other side of the root guard above, pinned **as intended behaviour**: the reveal that was
/// skipped because another tab was in front is *not* replayed when the originating tab comes back.
/// There is no pending-reveal queue and there should not be one — the reveal is immediate feedback,
/// and a tab switch promises to put the cursor back where the user left it (see the comment in
/// `apply_file_op`). What must not be lost is the *content*: the copied entry is in the listing
/// when the tab returns, because the switch runs a full rebuild.
///
/// Written so that adding a deferred reveal later fails here loudly rather than silently changing
/// behaviour nobody remembered deciding on.
#[test]
fn file_op_reveal_is_not_replayed_on_returning_to_the_tab() {
    let base = unique_tmp("konoma_fileop_no_deferred_reveal_test");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let base = base.canonicalize().unwrap();
    let a = base.join("a");
    let b = base.join("b");
    let src = a.join("src.txt");
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    std::fs::write(&src, b"x").unwrap();
    // Something in tab 1 for the cursor to sit on (and, being a file, it makes the paste land in
    // the root itself, so the copy is visible without expanding anything).
    std::fs::write(a.join("aaa.txt"), b"x").unwrap();
    std::fs::write(b.join("in-b.txt"), b"x").unwrap();

    let mut app = App::new(a.clone(), Config::default()).unwrap();
    app.rebuild_tree().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    app.attach_fileop_runner(tx);

    let idx =
        |app: &App, p: &std::path::Path| app.tab.entries.iter().position(|e| e.path == p).unwrap();
    app.tab.selected = idx(&app, &src);
    app.toggle_select();
    app.copy_selection();
    // Park the cursor somewhere deliberate (also the paste destination = the root), then leave for
    // another tab while the copy runs. `aaa.txt` sorts first on purpose: a tab switch restores the
    // saved cursor *index*, so parking on an entry the new file cannot shift keeps this test about
    // the reveal rather than about index arithmetic.
    app.tab.selected = idx(&app, &a.join("aaa.txt"));
    let parked = app.tab.entries[app.tab.selected].path.clone();
    app.paste().unwrap();
    assert!(app.fileop_pending.is_some(), "実行中");
    app.tab_new().unwrap();
    app.tab.root = b.clone();
    app.tab.entries.clear();
    app.tab.selected = 0;
    app.rebuild_tree().unwrap();

    let res = rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("ワーカーが結果を返す");
    let copied = a.join("src copy.txt"); // same directory → collision-avoiding name
    assert!(app.apply_file_op(res), "現世代の結果は適用される");
    assert!(copied.is_file(), "コピー自体は完了している");
    // The completion is still reported, so nothing about the outcome is lost by not revealing.
    assert!(app.flash.is_some(), "完了は flash で伝わる");

    // Back to the originating tab.
    app.tab_goto(0);
    assert_eq!(app.tab.root, a, "タブ1に戻っている");
    assert_eq!(
        app.tab.entries.get(app.tab.selected).map(|e| &e.path),
        Some(&parked),
        "戻ってきてもカーソルは離れた場所のまま(遅れて動かさない)"
    );
    // …and the listing itself did catch up on its own.
    assert!(
        app.tab.entries.iter().any(|e| e.path == copied),
        "コピーされたものは一覧に出ている(内容は自力で追いつく)"
    );

    std::fs::remove_dir_all(&base).ok();
}

/// A delete/trash that finishes while a *different* tab is active must not clear that tab's
/// committed selection. `clear_selection` used to run unconditionally — unlike `reveal_and_select`
/// a few lines above it in the very same function, which already carries a root guard for exactly
/// this reason (the user can switch tabs while a delete is in flight) — so finishing a delete
/// dispatched from tab A could silently wipe a selection the user is still building in tab B. Same
/// root-comparison fix as `file_op_result_does_not_disturb_another_tab`, applied to the
/// clear-selection branch instead. Covers both delete kinds.
///
/// This only checks the **committed** `selection` set (a set of paths), not `visual_anchor` (an
/// index into `entries`). `visual_anchor` is a separate story: `apply_file_op` calls
/// `self.refresh()` unconditionally at the very top, *before* either guard is reached (by design —
/// see its own comment: the disk actually changed, regardless of which tab is being viewed), and
/// `refresh()` always ends up calling `rebuild_tree()` against whichever tab happens to be active.
/// `rebuild_tree()` in turn *always* resets `visual_anchor` to `None` ("entries index, always
/// stale after a rebuild" — see its doc comment), unconditionally, regardless of file-op kind,
/// root, or the guard added here. So an in-progress visual range on tab B is wiped by that
/// unconditional rebuild the moment *any* background refresh lands while B is active — this is
/// pre-existing behaviour this fix neither introduces nor can address without also scoping the
/// whole `refresh()` call by root, which would defeat its documented purpose. Confirmed directly:
/// `file_op_result_clears_selection_on_the_same_tab` (below) demonstrates the case where
/// `clear_selection` *does* legitimately clear `visual_anchor` too — the same-tab path, where this
/// unrelated invalidation and the guarded one are not in tension.
#[test]
fn file_op_result_does_not_clear_another_tabs_selection() {
    for kind in [FileOpKind::Trash, FileOpKind::DeletePermanent] {
        let base = unique_tmp("konoma_fileop_del_other_tab_sel_test");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let base = base.canonicalize().unwrap();
        let a = base.join("a");
        let b = base.join("b");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        std::fs::write(a.join("x.txt"), b"x").unwrap();
        std::fs::write(a.join("y.txt"), b"x").unwrap();
        std::fs::write(b.join("p.txt"), b"x").unwrap();
        std::fs::write(b.join("q.txt"), b"x").unwrap();

        let mut app = App::new(a.clone(), Config::default()).unwrap();
        app.rebuild_tree().unwrap();

        // Tab A (the dispatching tab): select both files, then remove them from disk to stand in
        // for the worker having already finished the delete — `apply_file_op` only performs the
        // book-keeping step here, same idea as `file_op_refresh_failure_is_reported_not_masked`.
        app.tab.selected = 0;
        app.toggle_select();
        app.toggle_select();
        assert_eq!(app.marked_count(), 2, "タブAで2件選択");
        std::fs::remove_file(a.join("x.txt")).unwrap();
        std::fs::remove_file(a.join("y.txt")).unwrap();
        app.fileop_gen = 11;
        app.fileop_pending = Some(kind);

        // Switch to tab B and build an unrelated committed selection there.
        app.tab_new().unwrap();
        app.tab.root = b.clone();
        app.tab.entries.clear();
        app.tab.selected = 0;
        app.rebuild_tree().unwrap();
        app.tab.selected = 0;
        app.toggle_select(); // selects p.txt
        assert_eq!(app.tab.selection.len(), 1, "タブBで1件選択(コミット済み)");
        let before_selection = app.tab.selection.clone();

        // The result belongs to tab A (root == a), but tab B is active when it arrives.
        assert!(app.apply_file_op(FileOpResult {
            gen: 11,
            kind,
            root: a.clone(),
            ok: 2,
            last: None,
            err: None,
        }));

        assert_eq!(
            app.tab.selection, before_selection,
            "{kind:?}: 別タブ(B)で組み立て中の選択が消えてはいけない"
        );

        std::fs::remove_dir_all(&base).ok();
    }
}

/// The exact same completed-elsewhere delete must still clear the selection (and, per
/// `clear_selection`'s own doc comment, the in-progress visual range too) when the user is still
/// sitting on the tab that dispatched it — no regression from the new root guard. Covers both
/// delete kinds. (`visual_anchor` is meaningfully checkable here, unlike in the cross-tab test
/// above, because on the *same* tab we actually want it gone either way — there is no tension
/// between the guarded `clear_selection` call and the unconditional `rebuild_tree` invalidation.)
#[test]
fn file_op_result_clears_selection_on_the_same_tab() {
    for kind in [FileOpKind::Trash, FileOpKind::DeletePermanent] {
        let dir = unique_tmp("konoma_fileop_del_same_tab_sel_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("x.txt"), b"x").unwrap();
        std::fs::write(dir.join("y.txt"), b"x").unwrap();
        let dir = dir.canonicalize().unwrap();

        let mut app = App::new(dir.clone(), Config::default()).unwrap();
        app.rebuild_tree().unwrap();
        app.tab.selected = 0;
        app.toggle_select();
        app.toggle_select();
        assert_eq!(app.marked_count(), 2, "2件選択した状態から始める");
        app.enter_visual();
        assert!(app.is_visual(), "ビジュアルモードにも入っている");
        std::fs::remove_file(dir.join("x.txt")).unwrap();
        std::fs::remove_file(dir.join("y.txt")).unwrap();
        app.fileop_gen = 12;
        app.fileop_pending = Some(kind);

        assert!(app.apply_file_op(FileOpResult {
            gen: 12,
            kind,
            root: dir.clone(),
            ok: 2,
            last: None,
            err: None,
        }));

        assert!(
            !app.has_selection(),
            "{kind:?}: 同じタブに留まっていれば従来どおり選択は消える"
        );
        assert!(
            app.tab.visual_anchor.is_none(),
            "{kind:?}: clear_selection はビジュアル範囲も消す(同じタブでは従来どおり)"
        );

        std::fs::remove_dir_all(&dir).ok();
    }
}

/// Self-healing check for the "leave the originating tab's stale selection alone" claim in
/// `apply_file_op`'s doc comment: switching back to the tab that dispatched a delete must prune
/// the now-vanished paths from its selection, via `refresh_fs_inner`'s
/// `self.tab.selection.retain(...)` — the same pruning `refresh_fs_after_tab_switch` performs on
/// every tab switch. This is exercised for real (not asserted from reading the source): a delete
/// finishes while tab B is active (so `apply_file_op` intentionally leaves tab A's selection
/// untouched, per the previous two tests), then switching back to tab A is expected to have swept
/// it clean.
#[test]
fn returning_to_the_originating_tab_prunes_its_stale_selection() {
    let base = unique_tmp("konoma_fileop_del_self_heal_test");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let base = base.canonicalize().unwrap();
    let a = base.join("a");
    let b = base.join("b");
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    std::fs::write(a.join("x.txt"), b"x").unwrap();
    std::fs::write(a.join("y.txt"), b"x").unwrap();
    std::fs::write(b.join("p.txt"), b"x").unwrap();

    let mut app = App::new(a.clone(), Config::default()).unwrap();
    app.rebuild_tree().unwrap();
    app.tab.selected = 0;
    app.toggle_select();
    app.toggle_select();
    assert_eq!(app.marked_count(), 2, "タブAで2件選択");
    std::fs::remove_file(a.join("x.txt")).unwrap();
    std::fs::remove_file(a.join("y.txt")).unwrap();
    app.fileop_gen = 13;
    app.fileop_pending = Some(FileOpKind::Trash);

    // Switch away before the result lands, exactly like the previous two tests.
    app.tab_new().unwrap();
    app.tab.root = b.clone();
    app.tab.entries.clear();
    app.tab.selected = 0;
    app.rebuild_tree().unwrap();

    assert!(app.apply_file_op(FileOpResult {
        gen: 13,
        kind: FileOpKind::Trash,
        root: a.clone(),
        ok: 2,
        last: None,
        err: None,
    }));
    // Tab A's snapshot still carries the stale selection at this point (the previous two tests
    // assert exactly that); this test instead follows through on what happens when the user
    // actually switches back to it. Tab A is index 0: `App::new` pushes the initial tab as
    // `tabs[0]`, and `tab_new()` only ever appends — it never touches index 0 afterwards.
    app.tab_goto(0);
    assert_eq!(app.tab.root, a, "タブAへ切替できている");
    assert!(
        app.tab.selection.is_empty(),
        "存在しないファイルへの選択は切替時に剪定される: {:?}",
        app.tab.selection
    );

    std::fs::remove_dir_all(&base).ok();
}

// --- Background git writes (design principle #4) ----------------------------------------------
// These mirror the file-operation tests above, one axis at a time. Git writes were the last thing
// still shelling out synchronously on the UI thread, even though the *read* side (`git status`,
// `ignored`) had long been moved off it: a `pre-commit` hook, a network mount, or another process
// holding `.git/index.lock` each froze konoma outright. Deliberately **not** solved with a
// timeout+kill — killing `git commit` leaves `.git/index.lock` behind and the repo unusable.

/// A repo with one commit plus one uncommitted modification, and an `App` already in its Git view
/// (so `git_view_entries` has the row the stage/discard tests act on). Returns the canonical root.
#[cfg(feature = "git")]
fn git_repo_with_one_change(prefix: &str) -> (std::path::PathBuf, App) {
    let dir = unique_tmp(prefix);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    init_git_repo(&dir);
    let f = dir.join("a.txt");
    std::fs::write(&f, b"one\n").unwrap();
    crate::git::stage(&dir, &f).unwrap();
    crate::git::commit(&dir, "init").unwrap();
    // Modify the tracked file so there is exactly one changed entry to act on.
    std::fs::write(&f, b"two\n").unwrap();

    let canon = dir.canonicalize().unwrap();
    let mut app = App::new(canon.clone(), Config::default()).unwrap();
    app.open_git_view();
    assert!(app.is_git_view(), "{prefix}: Git ビューが開く");
    assert_eq!(app.git_view_entries().len(), 1, "{prefix}: 変更1件");
    (canon, app)
}

/// A git-write runner is attached → the dispatching call must return **immediately**, with the
/// write still out on the worker (`gitop_pending` set, busy indicator reporting it, watcher bursts
/// deferred), and the result must apply only once the channel delivers it. This is the whole point
/// of D3: `git` has no bounded runtime, so it must never run on the UI thread.
#[cfg(feature = "git")]
#[test]
fn git_op_runs_in_background_when_runner_attached() {
    let (dir, mut app) = git_repo_with_one_change("konoma_gitop_bg");
    let (tx, rx) = std::sync::mpsc::channel();
    app.attach_gitop_runner(tx);

    app.git_view_stage();
    // The moment the key handler returns, the write is still in flight: only the main thread's
    // `apply_git_op` clears `gitop_pending`, so it is always set here even if the worker has
    // already finished running.
    assert!(
        app.gitop_pending.is_some(),
        "バックグラウンド実行中は gitop_pending が立つ"
    );
    assert!(
        app.busy_jobs().contains(&crate::i18n::Msg::BusyGitOp),
        "busy インジケーターに git 操作が出る: {:?}",
        app.busy_jobs()
    );
    // Forced on even with `ui.busy_indicator = false`: nothing on screen moves while a hook runs,
    // so suppressing the spinner would leave no evidence at all that the commit is still going.
    app.cfg.ui.busy_indicator = false;
    assert!(
        app.busy_indicator_active(),
        "busy_indicator=false でも git 書き込み中は表示する"
    );
    app.cfg.ui.busy_indicator = true;
    assert!(
        app.should_defer_fs_events(),
        "実行中は自分が生む watcher イベントを溜める"
    );
    // Not applied yet: the Git view still shows the file as unstaged.
    assert_eq!(
        app.git_view_entries().first().map(|e| e.staged),
        Some(false),
        "結果適用前に UI が先走らない"
    );

    let res = rx
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("ワーカーが結果を返す");
    assert!(app.apply_git_op(res), "現世代の結果は適用される");
    assert!(app.gitop_pending.is_none(), "適用で pending が解ける");
    assert!(
        !app.should_defer_fs_events(),
        "完了後は watcher イベントの保留も解ける"
    );
    assert_eq!(
        app.git_view_entries().first().map(|e| e.staged),
        Some(true),
        "適用後は一覧が staged になる"
    );
    assert!(
        crate::git::changed_files(&dir).iter().any(|e| e.staged),
        "index に実際にステージされている"
    );
    assert!(
        app.flash
            .as_deref()
            .unwrap_or_default()
            .contains(crate::i18n::tr(app.lang, crate::i18n::Msg::Staged)),
        "flash にステージ結果が出る: {:?}",
        app.flash
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// With **no** runner attached (unit tests / no run loop), the write must still complete inline and
/// be observable on return — the same contract as `start_file_op`/`spawn_or_sync_statuses`. This is
/// what lets every pre-existing test of these flows keep passing unchanged.
#[cfg(feature = "git")]
#[test]
fn git_op_falls_back_to_synchronous_without_a_runner() {
    let (dir, mut app) = git_repo_with_one_change("konoma_gitop_sync");
    app.git_view_stage();
    assert!(
        app.gitop_pending.is_none(),
        "ランナー未 attach ならその場で完了する(pending が残らない)"
    );
    assert_eq!(
        app.git_view_entries().first().map(|e| e.staged),
        Some(true),
        "戻った時点で一覧に反映されている"
    );
    assert!(
        crate::git::changed_files(&dir).iter().any(|e| e.staged),
        "index に実際にステージされている"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// While one git write is in flight, a second must be rejected (flash `GitOpBusy`, generation
/// unchanged, nothing written) rather than starting a second `git` that would only lose the race
/// for `.git/index.lock`.
#[cfg(feature = "git")]
#[test]
fn second_git_op_is_rejected_while_one_is_in_flight() {
    let (dir, mut app) = git_repo_with_one_change("konoma_gitop_busy");
    // Without launching an actual worker, build the in-flight state directly (private fields are
    // reachable from within the module).
    app.gitop_gen = app.gitop_gen.wrapping_add(1);
    app.gitop_pending = Some(crate::app::GitOpKind::Commit);
    let gen_before = app.gitop_gen;

    app.git_view_stage();
    assert_eq!(
        app.flash.as_deref(),
        Some(crate::i18n::tr(app.lang, crate::i18n::Msg::GitOpBusy)),
        "実行中は GitOpBusy フラッシュで拒否される"
    );
    assert_eq!(app.gitop_gen, gen_before, "拒否された2件目は世代を進めない");
    assert!(
        !crate::git::changed_files(&dir).iter().any(|e| e.staged),
        "拒否された操作は git を一度も呼ばない"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// `apply_git_op` must ignore a result whose generation doesn't match the current one (superseded)
/// and must not clear `gitop_pending`.
#[cfg(feature = "git")]
#[test]
fn stale_git_op_result_is_dropped() {
    let dir = unique_tmp("konoma_gitop_stale");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let dir = dir.canonicalize().unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();

    app.gitop_gen = 5;
    app.gitop_pending = Some(crate::app::GitOpKind::Stage);
    let stale = crate::app::GitOpResult {
        gen: 4,
        kind: crate::app::GitOpKind::Stage,
        root: dir.clone(),
        tab_id: app.tab.id,
        path: dir.join("a.txt"),
        text: String::new(),
        label: "a.txt".into(),
        count: 0,
        err: None,
    };
    assert!(
        !app.apply_git_op(stale),
        "陳腐化した世代の結果は適用されない"
    );
    assert!(
        app.gitop_pending.is_some(),
        "陳腐化した結果は pending を解かない"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// A finished git write must not disturb whatever tab happens to be active when the result arrives.
/// The user can switch tabs while a `pre-commit` hook runs — that is the entire point of moving the
/// write off the UI thread — so the tab active on arrival may not be the one that dispatched it.
/// `rebuild_tree_notify`/`git_view_reload` would collapse that other tab's `/` filter and replace
/// its change list. The generation check does **not** protect against this (the generation is frozen
/// while an operation is pending) — the root comparison in `apply_git_op` does.
#[cfg(feature = "git")]
#[test]
fn git_op_result_does_not_disturb_another_tab() {
    let (a, mut app) = git_repo_with_one_change("konoma_gitop_other_tab");
    // Tab 2's root: a different, plain directory.
    let b = unique_tmp("konoma_gitop_other_tab_b");
    let _ = std::fs::remove_dir_all(&b);
    std::fs::create_dir_all(&b).unwrap();
    for n in ["alpha.txt", "beta.txt", "gamma.txt"] {
        std::fs::write(b.join(n), b"x").unwrap();
    }
    let b = b.canonicalize().unwrap();

    let (tx, rx) = std::sync::mpsc::channel();
    app.attach_gitop_runner(tx);
    // Discard through the real key path (`x` → confirm `y`), which on the same tab rebuilds the
    // tree and reloads the change list.
    app.git_view_start_discard();
    assert!(app.is_dialog(), "破棄確認ダイアログが出る");
    app.dialog_confirm(true).unwrap();
    assert!(app.gitop_pending.is_some(), "実行中");

    // Switch to a tab with a different root during the write, and put it into a `/` filter there.
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

    let res = rx
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("ワーカーが結果を返す");
    assert!(app.apply_git_op(res), "現世代の結果は適用される");
    assert_eq!(
        std::fs::read_to_string(a.join("a.txt")).unwrap(),
        "one\n",
        "破棄自体は完了している(コミット時点の内容に戻る)"
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
    // The repo really did change, so the status cache must still be invalidated — that part is
    // deliberately *not* gated on the root, or the originating tab would keep showing a stale marker.
    assert!(
        app.git_status_dirty && app.git_status_for.is_none(),
        "別タブに着地しても git status の再検証は要求される"
    );

    std::fs::remove_dir_all(&a).ok();
    std::fs::remove_dir_all(&b).ok();
}

/// Moving the write off-thread must not degrade the failure message: `run_git`'s contract is that
/// git's own `fatal:` line comes **first**, so the reason is never pushed off the end of the
/// one-line flash by the command string (the v0.23.1 fix). Also checks the retry affordance —
/// the input dialog reopens carrying what was typed.
#[cfg(feature = "git")]
#[test]
fn failed_git_op_keeps_gits_fatal_line_first_and_reopens_the_dialog() {
    let (dir, mut app) = git_repo_with_one_change("konoma_gitop_fatal");
    let current = crate::git::branch(&dir).expect("現在のブランチ名が取れる");
    let (tx, rx) = std::sync::mpsc::channel();
    app.attach_gitop_runner(tx);

    // Creating a branch that already exists → `fatal: a branch named '<x>' already exists`.
    app.start_create_branch();
    for c in current.chars() {
        app.dialog_input_push(c);
    }
    app.dialog_submit().unwrap();
    assert!(app.gitop_pending.is_some(), "失敗する書き込みも裏で走る");
    assert!(
        !app.is_dialog(),
        "投げた時点で入力ダイアログは閉じている(結果待ち)"
    );

    let res = rx
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("ワーカーが結果を返す");
    assert!(app.apply_git_op(res));
    let flash = app.flash.clone().unwrap_or_default();
    assert!(
        flash.starts_with("fatal:"),
        "git の fatal: 行が先頭に来る(コマンド文字列で押し出されない): {flash}"
    );
    assert!(
        !flash.contains("switch -c"),
        "実行したコマンド文字列は含まない: {flash}"
    );
    // The retry affordance the synchronous version had: the dialog comes back with the same text.
    assert!(app.is_dialog(), "失敗すると入力ダイアログが戻る");
    assert_eq!(
        app.dialog_view().map(|(_, _, buf, _)| buf.to_string()),
        Some(current.clone()),
        "打った名前が保持されている(打ち直させない)"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// A failed write must never restore its own dialog **over** one the user opened in the meantime —
/// only possible now that the write is asynchronous. Losing the draft message is bad; silently
/// swapping a "new file name" prompt for a commit box mid-typing is worse.
#[cfg(feature = "git")]
#[test]
fn failed_git_op_does_not_clobber_a_dialog_opened_meanwhile() {
    let (dir, mut app) = git_repo_with_one_change("konoma_gitop_no_clobber");
    let current = crate::git::branch(&dir).expect("現在のブランチ名");
    let (tx, rx) = std::sync::mpsc::channel();
    app.attach_gitop_runner(tx);

    app.start_create_branch();
    for c in current.chars() {
        app.dialog_input_push(c);
    }
    app.dialog_submit().unwrap();

    // While the (doomed) branch creation is out on the worker, the user opens another prompt.
    app.start_create();
    assert!(app.is_dialog(), "別のダイアログが開いている");
    for c in "newfile.txt".chars() {
        app.dialog_input_push(c);
    }

    let res = rx
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("ワーカーが結果を返す");
    assert!(app.apply_git_op(res));
    assert_eq!(
        app.dialog_view().map(|(_, _, buf, _)| buf.to_string()),
        Some("newfile.txt".to_string()),
        "入力中のダイアログを git の失敗で置き換えない"
    );
    assert!(
        app.flash
            .as_deref()
            .unwrap_or_default()
            .starts_with("fatal:"),
        "失敗自体は flash で伝える: {:?}",
        app.flash
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// A failed **commit** must bring its own dialog back carrying the message, so it can be retried
/// without retyping — the affordance the synchronous version had. (`git commit` with nothing staged
/// fails, which is what `git_repo_with_one_change` leaves the repo in.)
#[cfg(feature = "git")]
#[test]
fn failed_commit_reopens_the_dialog_with_the_message() {
    let (dir, mut app) = git_repo_with_one_change("konoma_gitop_commit_retry");
    let (tx, rx) = std::sync::mpsc::channel();
    app.attach_gitop_runner(tx);

    app.start_git_commit();
    for c in "wip: try".chars() {
        app.dialog_input_push(c);
    }
    app.dialog_submit().unwrap();
    assert!(app.gitop_pending.is_some(), "コミットは裏で走る");
    assert!(!app.is_dialog(), "投げた時点では閉じている");

    let res = rx
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("ワーカーが結果を返す");
    assert!(app.apply_git_op(res));
    assert_eq!(
        app.internal_mode(),
        Some(InternalMode::Commit),
        "失敗するとコミット入力に戻る"
    );
    assert_eq!(
        app.dialog_view().map(|(_, _, buf, _)| buf.to_string()),
        Some("wip: try".to_string()),
        "打ったメッセージが保持されている(打ち直させない)"
    );
    assert!(
        app.flash.as_deref().is_some_and(|s| !s.is_empty()),
        "失敗理由が flash に出る: {:?}",
        app.flash
    );
    assert!(
        crate::git::log(&dir, 10).len() == 1,
        "コミットは作られていない"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// …but it must not restore that commit box **over** a dialog the user opened while the commit was
/// out on the worker. Losing the draft message is bad; silently swapping a "new file name" prompt
/// for a commit box mid-typing is worse. (The `CreateBranch` arm carries the same guard and is
/// covered by `failed_git_op_does_not_clobber_a_dialog_opened_meanwhile`.)
#[cfg(feature = "git")]
#[test]
fn failed_commit_does_not_clobber_a_dialog_opened_meanwhile() {
    let (dir, mut app) = git_repo_with_one_change("konoma_gitop_commit_no_clobber");
    let (tx, rx) = std::sync::mpsc::channel();
    app.attach_gitop_runner(tx);

    app.start_git_commit();
    for c in "wip".chars() {
        app.dialog_input_push(c);
    }
    app.dialog_submit().unwrap();

    // While the (doomed) commit is out on the worker, the user opens another prompt and types.
    app.start_create();
    assert!(app.is_dialog(), "別のダイアログが開いている");
    for c in "newfile.txt".chars() {
        app.dialog_input_push(c);
    }

    let res = rx
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("ワーカーが結果を返す");
    assert!(app.apply_git_op(res));
    assert_ne!(
        app.internal_mode(),
        Some(InternalMode::Commit),
        "入力中のダイアログをコミット入力で置き換えない"
    );
    assert_eq!(
        app.dialog_view().map(|(_, _, buf, _)| buf.to_string()),
        Some("newfile.txt".to_string()),
        "入力中の内容がそのまま残る"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// A completed write changed the repository, so the cached `git status` must be re-validated —
/// otherwise the tree keeps showing the marker the write just removed.
#[cfg(feature = "git")]
#[test]
fn git_status_is_revalidated_after_a_git_write() {
    let (dir, mut app) = git_repo_with_one_change("konoma_gitop_revalidate");
    // Pretend the status cache is fresh for this root.
    app.git_status_for = Some(dir.clone());
    app.git_status_dirty = false;

    app.git_view_stage(); // synchronous fallback: completes inline
    assert!(
        app.git_status_dirty,
        "書き込み後は status の再取得が要求される"
    );
    assert!(
        app.git_status_for.is_none(),
        "ツリーの status キャッシュも無効化される"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// A refused dispatch (one git write at a time) must not eat what the user typed. `dialog_submit`
/// takes the dialog before it knows whether the write can start, so the prompt has to come back
/// with the text intact — otherwise pressing `c`, writing a paragraph and hitting Enter while a
/// stage happens to still be running silently throws the message away.
#[cfg(feature = "git")]
#[test]
fn refused_git_op_puts_the_typed_text_back() {
    let (dir, mut app) = git_repo_with_one_change("konoma_gitop_refused_keeps_text");
    // Build the in-flight state directly, without launching a worker.
    app.gitop_gen = app.gitop_gen.wrapping_add(1);
    app.gitop_pending = Some(crate::app::GitOpKind::Stage);

    let message = "a carefully written commit message";
    app.start_git_commit();
    for c in message.chars() {
        app.dialog_input_push(c);
    }
    app.dialog_submit().unwrap();

    assert_eq!(
        app.internal_mode(),
        Some(InternalMode::Commit),
        "拒否されてもコミット入力は残る"
    );
    assert_eq!(
        app.dialog_view().map(|(_, _, buf, _)| buf.to_string()),
        Some(message.to_string()),
        "打ったメッセージが捨てられない"
    );
    assert_eq!(
        app.dialog_view().map(|(_, _, _, cur)| cur),
        Some(message.chars().count()),
        "カーソルは末尾(続けて打てる)"
    );
    assert_eq!(
        app.flash.as_deref(),
        Some(crate::i18n::tr(app.lang, crate::i18n::Msg::GitOpBusy)),
        "なぜ起きなかったかは flash が説明する"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// The retry dialog must not pop up over input the user has already started. `/` (tree filter),
/// search and the branch filter are text input too, but none of them is a `Dialog` — checking only
/// `dialog.is_none()` would restore a commit box on top of a half-typed filter and steal the next
/// keystrokes. Only reachable because the write is asynchronous.
#[cfg(feature = "git")]
#[test]
fn failed_git_op_does_not_restore_over_an_active_filter() {
    let (dir, mut app) = git_repo_with_one_change("konoma_gitop_no_filter_hijack");
    let (tx, rx) = std::sync::mpsc::channel();
    app.attach_gitop_runner(tx);

    app.start_git_commit(); // nothing is staged → the commit will fail
    for c in "wip".chars() {
        app.dialog_input_push(c);
    }
    app.dialog_submit().unwrap();
    assert!(app.gitop_pending.is_some());

    // While the commit is out on the worker, the user starts filtering the tree.
    app.back_to_tree();
    app.start_filter();
    for c in "a.t".chars() {
        app.filter_input_push(c);
    }
    assert!(app.is_filtering(), "絞り込み入力中");

    let res = rx
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("ワーカーが結果を返す");
    assert!(app.apply_git_op(res));
    assert!(
        !app.is_dialog(),
        "入力中の絞り込みの上にダイアログを出さない"
    );
    assert!(app.is_filtering(), "絞り込み入力はそのまま続けられる");
    assert_eq!(app.filter_query(), Some("a.t"), "打った内容も無傷");
    assert!(
        app.flash.as_deref().is_some_and(|s| !s.is_empty()),
        "失敗自体は flash で伝える"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// **Root equality is not tab identity.** `t` (`tab_new`) opens the new tab *at the current root*,
/// so two tabs sharing a root is the ordinary case — and `worktree add` relocates whichever tab it
/// decides is "the one that asked". Pressing `t` while the worktree is being written must not drag
/// that brand-new tab into it and strand the tab that actually asked.
#[cfg(feature = "git")]
#[test]
fn worktree_add_never_relocates_a_tab_that_did_not_ask() {
    let base = unique_tmp("konoma_gitop_worktree_wrong_tab");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let base = base.canonicalize().unwrap();
    let repo = base.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    let f = repo.join("a.txt");
    std::fs::write(&f, b"one\n").unwrap();
    crate::git::stage(&repo, &f).unwrap();
    crate::git::commit(&repo, "init").unwrap();

    let mut cfg = Config::default();
    cfg.git.worktree_dir = base.to_string_lossy().into_owned();
    let mut app = App::new(repo.clone(), cfg).unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    app.attach_gitop_runner(tx);

    // Tab A asks for a new worktree.
    app.open_git_worktrees();
    app.start_create_worktree();
    for c in "wt-branch".chars() {
        app.dialog_input_push(c);
    }
    app.dialog_submit().unwrap();
    assert!(app.gitop_pending.is_some(), "worktree add は裏で走る");
    let tab_a = app.tab.id;

    // …and the user opens a new tab while it runs. Same root, different tab.
    app.tab_new().unwrap();
    let tab_b = app.tab.id;
    assert_ne!(tab_a, tab_b, "新しいタブは別の識別子を持つ");
    assert_eq!(app.tab.root, repo, "新しいタブは同じ root で開く(t の既定)");

    let res = rx
        .recv_timeout(std::time::Duration::from_secs(20))
        .expect("ワーカーが結果を返す");
    assert!(app.apply_git_op(res));

    assert!(
        base.join("wt-branch").join("a.txt").is_file(),
        "worktree 自体は作られている"
    );
    assert_eq!(
        app.tab.root, repo,
        "頼んでいないタブ(B)を新しい worktree へ引きずり込まない"
    );
    assert_eq!(app.tab.open_dir, repo, "open_dir(@参照の基準)も動かさない");
    assert!(
        app.flash
            .as_deref()
            .unwrap_or_default()
            .contains(crate::i18n::tr(app.lang, crate::i18n::Msg::CreatedWorktree)),
        "作成できたことは flash で伝える: {:?}",
        app.flash
    );

    std::fs::remove_dir_all(&base).ok();
}

/// The counterpart to the guard above: a *repo-scoped refresh* must still happen on any tab rooted
/// at the repository that changed, even one that did not ask. The two questions ("is this the tab
/// that asked" / "is this tab showing the repo that changed") have deliberately different answers —
/// collapsing them into one would either strand stale change lists or relocate innocent tabs.
#[cfg(feature = "git")]
#[test]
fn same_root_tab_is_refreshed_even_though_it_is_not_the_origin() {
    let (dir, mut app) = git_repo_with_one_change("konoma_gitop_same_root_refresh");
    let (tx, rx) = std::sync::mpsc::channel();
    app.attach_gitop_runner(tx);

    app.git_view_stage();
    let tab_a = app.tab.id;

    // A second tab on the same root (what `t` gives you), which has never opened the Git view.
    app.tab_new().unwrap();
    assert_ne!(tab_a, app.tab.id);
    assert_eq!(app.tab.root, dir);
    assert!(
        app.git_view_entries().is_empty(),
        "新しいタブはまだ変更一覧を持たない"
    );

    let res = rx
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("ワーカーが結果を返す");
    assert!(app.apply_git_op(res));
    assert_eq!(
        app.git_view_entries().first().map(|e| e.staged),
        Some(true),
        "同じ repo を映すタブは、頼んでいなくても最新の変更一覧に追従する"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// Quitting mid-write orphans a `git` still holding `.git/index.lock`, so the confirmation is shown
/// **even with `confirm_quit = false`** — same rule as a running file operation. It only warns;
/// `y` still quits.
#[cfg(feature = "git")]
#[test]
fn quit_confirms_while_a_git_op_is_running() {
    let dir = unique_tmp("konoma_gitop_quit");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let dir = dir.canonicalize().unwrap();
    let mut cfg = Config::default();
    cfg.ui.confirm_quit = false;
    let mut app = App::new(dir.clone(), cfg).unwrap();

    assert!(
        !app.request_quit(),
        "confirm_quit=false かつアイドルなら即終了"
    );
    app.gitop_pending = Some(crate::app::GitOpKind::Commit);
    assert!(
        app.request_quit(),
        "git 書き込み中は設定に関わらず確認を出す"
    );
    let message = app
        .dialog_view()
        .map(|(_, head, _, _)| head.to_string())
        .unwrap_or_default();
    assert!(
        message.contains(crate::i18n::tr(app.lang, crate::i18n::Msg::QuitWhileGitOp)),
        "実行中である旨の警告行が入る: {message}"
    );
    std::fs::remove_dir_all(&dir).ok();
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

/// Regression: a permanent delete that fails partway through used to flash a flat "Failed" with
/// no success count at all — even though the earlier targets in the batch had already been
/// unrecoverably removed by the time the failing one was hit. `delete_permanently_with_progress`
/// loops per path and bumps `Progress` on each success before returning `Err` via `?` on the first
/// failure (see `delete_permanently_reports_progress_per_path`), so that count was always sitting
/// right there — `App::run_file_op`'s `DeletePermanent` arm just never read it back on the error
/// path, leaving `ok` at its initial 0. With 3 real targets and the *middle* one made
/// undeletable, this exercises the full dispatch path (`start_delete` → `dialog_delete_permanent`
/// → `start_file_op` → `run_file_op` → `apply_file_op`) and checks against the actual on-disk
/// outcome, not an assumption: exactly one target is gone for good, one failed and is still there,
/// and one was never attempted (the loop breaks at the first failure) — and the flash must say "1"
/// succeeded, not imply zero.
#[test]
#[cfg(unix)]
fn delete_permanent_partial_failure_reports_the_real_success_count() {
    use std::os::unix::fs::PermissionsExt;
    // This test depends on "permission bits deny removing an entry from a directory". Under root
    // (or any process able to bypass permission bits), the removal succeeds anyway and the test
    // alone would go red for environmental reasons even though the product is correct — same
    // caveat as `md_task_toggle_flashes_on_write_error`/`write_denied_by_permissions`. Skip rather
    // than report a false failure.
    if !write_denied_by_permissions() {
        eprintln!(
            "delete_permanent_partial_failure_reports_the_real_success_count: このプロセスはパーミッションで書込みを拒否されない(root 等)ためスキップ"
        );
        return;
    }
    let dir = unique_tmp("konoma_delete_partial_fail_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let dir = dir.canonicalize().unwrap();

    std::fs::write(dir.join("a.txt"), b"a").unwrap();
    let blocked = dir.join("blocked");
    std::fs::create_dir_all(&blocked).unwrap();
    std::fs::write(blocked.join("inner.txt"), b"x").unwrap();
    std::fs::write(dir.join("c.txt"), b"c").unwrap();
    // Deny write on `blocked` itself: removing `inner.txt` (required before the directory itself
    // can go) needs write+exec on its containing directory. Sorted order (`op_targets` collects a
    // `BTreeSet`) is a.txt, blocked, c.txt, so this fails exactly the *middle* target while its
    // siblings stay removable — a.txt gets deleted before the loop reaches `blocked`, and c.txt is
    // never attempted because the loop returns via `?` at `blocked`.
    std::fs::set_permissions(&blocked, std::fs::Permissions::from_mode(0o555)).unwrap();

    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.lang = crate::i18n::Lang::En;
    app.rebuild_tree().unwrap();
    for name in ["a.txt", "blocked", "c.txt"] {
        let idx = app
            .tab
            .entries
            .iter()
            .position(|e| e.path == dir.join(name))
            .unwrap();
        app.tab.selected = idx;
        app.toggle_select();
    }
    assert_eq!(app.marked_count(), 3, "3件選択");

    app.start_delete();
    assert!(app.dialog_is_confirm() && app.dialog_allow_permanent());
    app.dialog_delete_permanent().unwrap();

    // Restore permissions so cleanup below can actually remove `blocked`, regardless of outcome.
    let _ = std::fs::set_permissions(&blocked, std::fs::Permissions::from_mode(0o755));

    // Ground truth from the filesystem itself, independent of anything the flash claims.
    assert!(!dir.join("a.txt").exists(), "a.txt は実際に消えている");
    assert!(dir.join("blocked").exists(), "blocked は失敗して残っている");
    assert!(
        dir.join("c.txt").exists(),
        "blocked で ? 早期returnしたので c.txt は未着手のまま残っている"
    );

    let flash = app.flash.clone().unwrap_or_default();
    assert!(
        flash.contains("Deleted permanently 1"),
        "実際に消えた1件ぶんの成功数が flash に出る(0でも3でもない): {flash:?}"
    );
    assert!(
        flash.contains(crate::i18n::tr(app.lang, crate::i18n::Msg::Failed)),
        "失敗の事実も flash に残る: {flash:?}"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// Regression, the Trash side of the same bug: a batch send-to-trash where one target is a path
/// that no longer exists (e.g. removed by another process/agent between selecting it and
/// confirming) used to just say "Failed", with no way to tell whether the *other*, still-valid
/// target actually got trashed or not. `trash::delete_all` doesn't report partial success, so the
/// fix (`trash_partial_outcome`) observes the filesystem afterward instead of assuming zero. This
/// hits the real OS Trash (macOS's default `Finder`-AppleScript backend, gated accordingly, same
/// as the existing `move_to_trash_removes_from_original_then_cleanup` live test) and asserts
/// against whatever it actually did, not a guess: `ok` must equal the number of targets that are
/// actually gone (never a number that overstates it — this fix's whole point), and the error must
/// name a target that is still actually present whenever one exists.
#[test]
#[cfg(target_os = "macos")]
fn trash_partial_failure_reports_the_real_outcome_and_names_a_remaining_target() {
    let dir = unique_tmp("konoma_trash_missing_target_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let dir = dir.canonicalize().unwrap();
    // Unique names (not just "a.txt"/"b.txt") so a leftover in the real ~/.Trash from a previous
    // run — or from another test — can never look like this run's own artifact.
    let name_a = format!("konoma_trash_probe_a_{}.txt", std::process::id());
    let name_b = format!("konoma_trash_probe_b_{}.txt", std::process::id());
    let a = dir.join(&name_a);
    let b = dir.join(&name_b);
    std::fs::write(&a, b"a").unwrap();
    std::fs::write(&b, b"b").unwrap();

    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.lang = crate::i18n::Lang::En;
    app.rebuild_tree().unwrap();
    for name in [&name_a, &name_b] {
        let idx = app
            .tab
            .entries
            .iter()
            .position(|e| e.path == dir.join(name))
            .unwrap();
        app.tab.selected = idx;
        app.toggle_select();
    }
    assert_eq!(app.marked_count(), 2, "2件選択");

    // `b` disappears out from under the selection — the real-world repro (deleted externally
    // between select and confirm) — leaving `move_to_trash` a target that no longer exists mixed
    // in with one that does.
    std::fs::remove_file(&b).unwrap();

    app.start_delete();
    app.dialog_confirm(true).unwrap();

    // Ground truth, matching `trash_partial_outcome`'s own contract exactly (count of targets
    // that are simply gone right now, whether this specific call removed them or — as with `b`
    // here — they were already gone going in): never assume, always ask the filesystem for both
    // targets, not just the one the operation could plausibly have touched.
    let a_gone = !a.exists();
    let b_gone = !b.exists(); // always true here — `b` was removed above, before the call even ran
    assert!(b_gone, "b は事前に外部削除済み(このテストの前提)");
    let real_ok = usize::from(a_gone) + usize::from(b_gone);

    let flash = app.flash.clone().unwrap_or_default();
    assert!(
        flash.contains(&format!("Moved to Trash {real_ok}")),
        "実際に(今)存在しない対象の数と flash の成功数が一致する: {flash:?}"
    );
    assert!(
        flash.contains(crate::i18n::tr(app.lang, crate::i18n::Msg::Failed)),
        "失敗の事実が flash に出る: {flash:?}"
    );
    // Whichever of the two targets is still actually present gets named in the error. `b` never
    // qualifies (it was never present during this call), so if anything is named it must be `a`.
    if !a_gone {
        assert!(
            flash.contains(&name_a),
            "まだ残っている対象(a)の名前がエラーに出る: {flash:?}"
        );
    } else {
        // Both targets ended up gone (the real Trash backend actually managed `a` too, despite
        // reporting an error) — nothing is left standing, so there is nothing to name; the error
        // must fall back to the plain reason with no dangling ": <path>" suffix.
        assert!(
            !flash.trim_end().ends_with(".txt"),
            "残っている対象が無ければパスを付け足さない: {flash:?}"
        );
    }

    // Best-effort cleanup of the real trash, in case `a` did get moved there (same courtesy as
    // the existing `move_to_trash_removes_from_original_then_cleanup` live test).
    if let Some(home) = std::env::var_os("HOME") {
        let _ = std::fs::remove_file(std::path::PathBuf::from(home).join(".Trash").join(&name_a));
    }
    let _ = std::fs::remove_file(&a);
    std::fs::remove_dir_all(&dir).ok();
}

/// Non-regression for both delete kinds: a fully successful batch must keep showing "<verb> (N)"
/// with no `Failed` wording — the new partial-success branch (`Some(e) if res.ok > 0`) must not
/// somehow also swallow the plain-success `None` arm.
#[test]
fn file_op_full_success_flash_is_unchanged_for_trash_and_delete_permanent() {
    let dir = unique_tmp("konoma_fileop_full_success_flash_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let dir = dir.canonicalize().unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.lang = crate::i18n::Lang::En;
    app.rebuild_tree().unwrap();

    for (kind, verb) in [
        (FileOpKind::Trash, "Moved to Trash"),
        (FileOpKind::DeletePermanent, "Deleted permanently"),
    ] {
        app.fileop_gen = app.fileop_gen.wrapping_add(1);
        let gen = app.fileop_gen;
        app.fileop_pending = Some(kind);
        assert!(app.apply_file_op(FileOpResult {
            gen,
            kind,
            root: dir.clone(),
            ok: 2,
            last: None,
            err: None,
        }));
        let flash = app.flash.clone().unwrap_or_default();
        assert_eq!(
            flash,
            format!("{verb} (2)"),
            "{kind:?}: 全部成功したときの文言は従来どおり"
        );
    }

    std::fs::remove_dir_all(&dir).ok();
}

/// Non-regression for both delete kinds: when **nothing at all** succeeded (`ok == 0`), the flash
/// must stay the plain "Failed: <reason>" — no count, since showing "<verb> 0 / Failed: ..." would
/// be a technically-true but pointless (and newly-introduced-looking) "0 succeeded" message. Only
/// `res.ok > 0` should ever switch to the partial-success wording.
#[test]
fn file_op_zero_success_flash_stays_plain_failed_for_trash_and_delete_permanent() {
    let dir = unique_tmp("konoma_fileop_zero_success_flash_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let dir = dir.canonicalize().unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.lang = crate::i18n::Lang::En;
    app.rebuild_tree().unwrap();

    for kind in [FileOpKind::Trash, FileOpKind::DeletePermanent] {
        app.fileop_gen = app.fileop_gen.wrapping_add(1);
        let gen = app.fileop_gen;
        app.fileop_pending = Some(kind);
        assert!(app.apply_file_op(FileOpResult {
            gen,
            kind,
            root: dir.clone(),
            ok: 0,
            last: None,
            err: Some("boom".into()),
        }));
        let flash = app.flash.clone().unwrap_or_default();
        assert_eq!(
            flash,
            format!(
                "{}: boom",
                crate::i18n::tr(app.lang, crate::i18n::Msg::Failed)
            ),
            "{kind:?}: 0件成功時は数を出さず従来どおり Failed: <reason>"
        );
    }

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
    let dir = unique_tmp("konoma_dialog_preview_scroll_test");
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
    let dir = unique_tmp("konoma_ui_preview_fallback_test");
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
    let dir = unique_tmp("konoma_prune_remote_test");
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
    let dir = unique_tmp("konoma_md_dims_test");
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
    let dir = unique_tmp("konoma_md_decode_test");
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
    let dir = unique_tmp("konoma_apply_remote_test");
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

/// Regression: once a remote Markdown image's download failed, `md_remote_failed` remembered it for
/// the rest of the process — nothing in the codebase ever called `.remove()` / `.clear()` /
/// `.retain()` on it (grep confirms `insert()` is the only mutation) — so it was never retried even
/// after the network came back, short of restarting konoma. An explicit reload (`refresh()`, called
/// from the `r` key / after a paste / after an external git tool / after a confirm dialog — see
/// `bookmark_actions.rs::refresh`'s callers) is exactly the moment a user expects another chance, so
/// it now clears the negative cache too. The **FS-watch** path must NOT do this — see the sibling
/// `refresh_fs_watched_does_not_retry_a_previously_failed_remote_image` test below: an agent
/// rewriting nearby files fires an fs event per write, and retrying a doomed download on every single
/// one is exactly what `md_remote_failed` exists to prevent (see that field's own doc comment).
///
/// Exercises the real production round trip end to end (attach a channel, `ensure_remote_md_fetch`
/// spawns a background thread, the thread reports back, `apply_remote_fetch` re-records it) instead
/// of only asserting on the private field, so this also proves the retry is not short-circuited by
/// some other guard (`resolve_md_image_path` / `md_remote_inflight`). The URL is deliberately
/// malformed — `"http://"` has no host, so `ureq` rejects it (`InvalidUri(Empty)`) inside its own
/// request-construction step, before any DNS lookup or socket is opened — confirmed against this
/// crate's exact `ureq` version with a standalone probe (~90µs, vs. milliseconds-plus for even a
/// loopback connect attempt). This matches the "no test reaches the network" rule the sibling
/// `fetch_remote_image_malformed_url_fails_without_touching_the_network` test already documents; that
/// test can't reuse this exact shape because it calls the lower-level `fetch_remote_image` directly,
/// bypassing `is_remote_image_url`'s `http://`/`https://`-prefix gate that `ensure_remote_md_fetch`
/// requires to even attempt a fetch.
#[test]
fn refresh_retries_a_previously_failed_remote_image() {
    let dir = unique_tmp("konoma_remote_retry_refresh_test");
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();

    let url = "http://".to_string(); // malformed (no host) — see the doc comment above
    let (tx, rx) = std::sync::mpsc::channel();
    app.attach_remote_md_loader(tx);

    // Record a failure through the same completion path a real failed download takes.
    app.md_remote_inflight.insert(url.clone());
    app.apply_remote_fetch(RemoteFetch {
        url: url.clone(),
        ok: false,
    });
    assert!(app.md_remote_failed.contains(&url), "前提: 失敗を記録済み");

    // Still marked failed → short-circuits before ever spawning a fetch thread.
    assert!(
        !app.ensure_remote_md_fetch(&url),
        "失敗記録中は再試行しない"
    );
    assert!(
        rx.try_recv().is_err(),
        "再試行していないので結果はまだ来ないはず"
    );

    app.refresh().unwrap();
    assert!(
        !app.md_remote_failed.contains(&url),
        "refresh() は失敗記録をクリアし再試行対象に戻す必要がある"
    );

    // Now it actually retries: a real background thread runs `fetch_remote_image`, fails fast (no
    // network reached — see the URL's doc comment above) and reports back through the channel.
    app.ensure_remote_md_fetch(&url);
    let res = rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("refresh() の後は再試行が走り結果が返ってくるはず");
    assert_eq!(res.url, url);
    assert!(!res.ok, "ホストのない URL は失敗するはず");

    std::fs::remove_dir_all(&dir).ok();
}

/// The sibling non-regression: the **FS-watch** entry point (`refresh_fs_watched` — what `main`'s run
/// loop calls on every filesystem event, `main.rs:868`) must NOT clear `md_remote_failed`. Only
/// `refresh()` (the explicit-reload path, see the test above) does. Locks in the fix's scope so a
/// future edit that moves the `.clear()` into the shared `refresh_fs_inner` (the tempting place,
/// since both `refresh()` and `refresh_fs_watched` funnel through it) doesn't silently turn every fs
/// event into a retry storm against a permanently-broken URL.
#[test]
fn refresh_fs_watched_does_not_retry_a_previously_failed_remote_image() {
    let dir = unique_tmp("konoma_remote_no_retry_fswatch_test");
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();

    let url = "http://".to_string();
    app.md_remote_inflight.insert(url.clone());
    app.apply_remote_fetch(RemoteFetch {
        url: url.clone(),
        ok: false,
    });
    assert!(app.md_remote_failed.contains(&url), "前提: 失敗を記録済み");

    // The exact call `main`'s run loop makes on every filesystem event.
    app.refresh_fs_watched(false, &[]);

    assert!(
        app.md_remote_failed.contains(&url),
        "refresh_fs_watched（FS イベント経路）は失敗記録をクリアしてはいけない"
    );

    std::fs::remove_dir_all(&dir).ok();
}

// ---- inline Markdown images: remote fetch — real HTTP round trips over a loopback socket ----
//
// The tests above (`fetch_remote_image_malformed_url_fails_without_touching_the_network`,
// `refresh_retries_a_previously_failed_remote_image`) deliberately never let `ureq` send a byte: a
// malformed URL fails inside its own request-construction step, before any DNS lookup or socket is
// opened. So nothing in the suite had ever actually spoken HTTP — the response-parsing path (status
// codes, redirects, gzip, the body-size cap, and the "reject a 200 HTML error page" guard) was
// completely unverified. These tests close that gap with a real HTTP/1.1 server this test process
// itself binds to a loopback port, serves from, and tears down — never touching an outside host,
// which is what the "no test reaches the network" rule (see the malformed-URL test's doc comment
// above) is actually about; the existing e2e test
// `e2e_ui_remote_image_fetch_failure_without_network_degrades_to_placeholder` already treats
// 127.0.0.1 the same way (there it's a connection-refused reply from nothing bound there; here it's
// a real reply from something this test bound itself).

/// Minimal, single-purpose HTTP/1.1 test server bound to an OS-assigned loopback port. Spawns a
/// background thread that answers exactly `responses.len()` sequential TCP connections, each with
/// the raw bytes given verbatim (callers build a whole response themselves: status line + headers +
/// body), so a test using it exercises `fetch_remote_bytes_capped`/`fetch_remote_image_capped`'s
/// real HTTP/1.1 response parsing, not just their failure-before-any-byte-is-sent paths.
struct LoopbackServer {
    addr: std::net::SocketAddr,
}

impl LoopbackServer {
    fn start(responses: Vec<Vec<u8>>) -> Self {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let addr = listener.local_addr().expect("local_addr");
        std::thread::spawn(move || {
            for resp in responses {
                let Ok((mut stream, _)) = listener.accept() else {
                    break;
                };
                drain_request_head(&mut stream);
                use std::io::Write;
                let _ = stream.write_all(&resp);
                let _ = stream.flush();
            }
        });
        Self { addr }
    }

    fn url(&self, path: &str) -> String {
        format!("http://{}{}", self.addr, path)
    }
}

/// Read (and discard) a request up to its blank line (a GET has no body) before replying — so a
/// slow reader on the client side sees the full reply instead of a reset once the serving loop moves
/// on and drops the stream.
fn drain_request_head(stream: &mut std::net::TcpStream) {
    use std::io::Read;
    let mut seen = Vec::new();
    let mut buf = [0u8; 512];
    while !seen.windows(4).any(|w| w == b"\r\n\r\n") {
        match stream.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => seen.extend_from_slice(&buf[..n]),
        }
    }
}

/// Build a raw HTTP/1.1 response. Always closes the connection after the one reply (matches
/// `LoopbackServer`'s one-reply-per-connection design) and fills in `Content-Length` from `body`
/// unless the caller already supplied one (the redirect test's empty-body 302 doesn't need one;
/// everything else does, for deterministic framing).
fn http_response(status_line: &str, headers: &[(&str, String)], body: &[u8]) -> Vec<u8> {
    let mut out = format!("HTTP/1.1 {status_line}\r\n").into_bytes();
    let has_len = headers
        .iter()
        .any(|(k, _)| k.eq_ignore_ascii_case("content-length"));
    for (k, v) in headers {
        out.extend_from_slice(format!("{k}: {v}\r\n").as_bytes());
    }
    if !has_len {
        out.extend_from_slice(format!("Content-Length: {}\r\n", body.len()).as_bytes());
    }
    out.extend_from_slice(b"Connection: close\r\n\r\n");
    out.extend_from_slice(body);
    out
}

/// A tiny real PNG's bytes (2x2, opaque) — built through the same `image` crate encoder the app
/// itself decodes with (not a hand-rolled byte literal), via a scratch file (mirrors this file's
/// existing `image::RgbaImage::…save(&png)` convention elsewhere) read back into memory.
fn tiny_png_bytes() -> Vec<u8> {
    let dir = unique_tmp("konoma_tiny_png_src");
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("t.png");
    image::RgbaImage::from_pixel(2, 2, image::Rgba([10, 20, 30, 255]))
        .save(&p)
        .unwrap();
    let bytes = std::fs::read(&p).unwrap();
    std::fs::remove_dir_all(&dir).ok();
    bytes
}

fn gzip_bytes(data: &[u8]) -> Vec<u8> {
    use std::io::Write;
    let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    enc.write_all(data).unwrap();
    enc.finish().unwrap()
}

/// Layer 1: a plain 200 response is received byte-for-byte.
#[test]
fn fetch_remote_bytes_capped_receives_a_real_200_body() {
    let body = b"hello from the loopback test server";
    let srv = LoopbackServer::start(vec![http_response(
        "200 OK",
        &[("Content-Type", "text/plain".into())],
        body,
    )]);
    let got = fetch_remote_bytes_capped(&srv.url("/x"), MD_REMOTE_MAX_BYTES);
    assert_eq!(got.as_deref(), Some(body.as_slice()));
}

/// Layer 3: a non-2xx status (client or server error) is treated as failure — `None`, mirroring
/// `curl --fail` — even though the server sent a perfectly well-formed body along with it.
#[test]
fn fetch_remote_bytes_capped_rejects_non_2xx_status() {
    for status in [
        "404 Not Found",
        "500 Internal Server Error",
        "403 Forbidden",
    ] {
        let srv = LoopbackServer::start(vec![http_response(status, &[], b"nope")]);
        assert!(
            fetch_remote_bytes_capped(&srv.url("/x"), MD_REMOTE_MAX_BYTES).is_none(),
            "status {status} は失敗のはず"
        );
    }
}

/// Layer 4: a redirect (3xx + `Location`) is followed automatically, and the bytes returned are the
/// *final* response's — not the redirect's empty body. GitHub proxies images through camo exactly
/// this way, per `fetch_remote_bytes_capped`'s doc comment.
#[test]
fn fetch_remote_bytes_capped_follows_a_redirect_to_the_final_body() {
    let body = b"the real image lives here, not at the redirect";
    // Two sequential connections on one server: the redirect, then the target it points to. The
    // `Location` needs the real ephemeral port, so bind first and reuse `LoopbackServer`'s serving
    // loop shape (duplicated here rather than factored out, since `LoopbackServer::start` takes an
    // already-built `Vec<Vec<u8>>` and this response depends on the very address that start-up
    // produces).
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let addr = listener.local_addr().expect("local_addr");
    let redirect = http_response(
        "302 Found",
        &[("Location", format!("http://{addr}/target"))],
        b"",
    );
    let target = http_response("200 OK", &[], body);
    std::thread::spawn(move || {
        for resp in [redirect, target] {
            let Ok((mut stream, _)) = listener.accept() else {
                break;
            };
            drain_request_head(&mut stream);
            use std::io::Write;
            let _ = stream.write_all(&resp);
            let _ = stream.flush();
        }
    });
    let got = fetch_remote_bytes_capped(&format!("http://{addr}/start"), MD_REMOTE_MAX_BYTES);
    assert_eq!(
        got.as_deref(),
        Some(body.as_slice()),
        "リダイレクト先の本文を受信するはず"
    );
}

/// Layer 5: a `Content-Encoding: gzip` response is transparently decompressed — the crate is built
/// with ureq's `gzip` feature (`Cargo.toml`) specifically for this.
#[test]
fn fetch_remote_bytes_capped_decodes_a_gzip_response() {
    let plain = b"this text only exists compressed on the wire ".repeat(20);
    let compressed = gzip_bytes(&plain);
    let srv = LoopbackServer::start(vec![http_response(
        "200 OK",
        &[
            ("Content-Encoding", "gzip".into()),
            ("Content-Length", compressed.len().to_string()),
        ],
        &compressed,
    )]);
    let got = fetch_remote_bytes_capped(&srv.url("/x"), MD_REMOTE_MAX_BYTES);
    assert_eq!(
        got.as_deref(),
        Some(plain.as_slice()),
        "gzip 展開後の平文を受信するはず"
    );
}

/// Layer 2: `MD_REMOTE_MAX_BYTES`'s enforcement mechanism (`resp.body_mut()…limit(n)`) rejects a
/// body that exceeds the cap, and accepts the identical body under a cap that's big enough — proving
/// it's specifically the cap causing the rejection, not something else about this response.
///
/// This deliberately does NOT transfer the real 25MiB `MD_REMOTE_MAX_BYTES` over the wire just to
/// prove the number is wired through (slow, no extra coverage): `fetch_remote_bytes_capped` takes
/// the cap as a parameter precisely so a small cap can exercise the exact same `.limit()` call
/// production makes, just with a small enough `n` that a small (200-byte) body already exceeds it.
#[test]
fn fetch_remote_bytes_capped_rejects_a_body_over_the_cap() {
    let body = vec![b'x'; 200];
    let srv = LoopbackServer::start(vec![
        http_response("200 OK", &[], &body),
        http_response("200 OK", &[], &body),
    ]);
    assert!(
        fetch_remote_bytes_capped(&srv.url("/a"), 50).is_none(),
        "上限 50 バイトに対し 200 バイトの本文は失敗するはず"
    );
    assert_eq!(
        fetch_remote_bytes_capped(&srv.url("/b"), 5_000).as_deref(),
        Some(body.as_slice()),
        "十分大きい上限では同じ本文が成功するはず(失敗の原因が上限そのものであることの対照)"
    );
}

/// Layer 6: `fetch_remote_image`'s `md_image_dims` guard rejects a 200 response whose body isn't
/// actually an image — the classic "server returns an HTML error page with a 200 status" case a bare
/// status-code check would miss. No `.part` temp file is left behind either.
#[test]
fn fetch_remote_image_capped_rejects_a_200_html_error_page() {
    let dir = unique_tmp("konoma_remote_html_reject_test");
    std::fs::create_dir_all(&dir).unwrap();
    let dest = dir.join("out.png");
    let html = b"<html><body><h1>404 - not actually found</h1></body></html>";
    let srv = LoopbackServer::start(vec![http_response(
        "200 OK",
        &[("Content-Type", "text/html".into())],
        html,
    )]);
    assert!(!fetch_remote_image_capped(
        &srv.url("/x"),
        &dest,
        MD_REMOTE_MAX_BYTES
    ));
    assert!(!dest.exists(), "HTML は画像として保存されないはず");
    assert!(
        !dest.with_extension("part").exists(),
        ".part も残らないはず"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// The positive counterpart, full round trip: a real 200 PNG response is fetched, validated (accepted
/// this time, unlike the HTML-error-page test above), and committed to `dest` — the whole
/// `fetch_remote_image` pipeline a background thread runs every time a Markdown document embeds a
/// remote image (`ensure_remote_md_fetch`).
#[test]
fn fetch_remote_image_capped_accepts_a_real_200_png() {
    let dir = unique_tmp("konoma_remote_png_accept_test");
    std::fs::create_dir_all(&dir).unwrap();
    let dest = dir.join("out.png");
    let png = tiny_png_bytes();
    let srv = LoopbackServer::start(vec![http_response(
        "200 OK",
        &[("Content-Type", "image/png".into())],
        &png,
    )]);
    assert!(fetch_remote_image_capped(
        &srv.url("/x"),
        &dest,
        MD_REMOTE_MAX_BYTES
    ));
    assert!(dest.is_file(), "有効な画像は保存されるはず");
    assert_eq!(
        std::fs::read(&dest).unwrap(),
        png,
        "保存されたバイト列は取得したものと一致するはず"
    );
    assert!(
        !dest.with_extension("part").exists(),
        "コミット後は .part が残らないはず"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn preview_survives_target_file_overwrite_and_delete() {
    // A previewed file can be overwritten or deleted out from under konoma (a script re-plots or
    // removes it). This must never panic — the preview reloads / degrades gracefully. Covers both an
    // image preview and a text/Markdown preview (not just images).
    use image::RgbImage;
    let dir = unique_tmp("konoma_preview_file_vanish_test");
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
    let work = unique_tmp("konoma_at_ref_test");
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
    let dir = unique_tmp("konoma_sel_ref_test");
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
    let dir = unique_tmp("konoma_changed_filter_test");
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
    let dir = unique_tmp("konoma_jump_changed_test");
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
    let dir = unique_tmp("konoma_follow_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    std::fs::create_dir_all(dir.join(".hidden")).unwrap();
    std::fs::write(dir.join("a.txt"), b"a\n").unwrap();
    std::fs::write(dir.join("sub").join("b.txt"), b"b\n").unwrap();
    std::fs::write(dir.join(".hidden").join("c.txt"), b"c\n").unwrap();
    let outside = unique_tmp("konoma_follow_outside.txt");
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
    let dir = unique_tmp("konoma_follow_scroll_test");
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
    let dir = unique_tmp("konoma_follow_diff_test");
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
    let dir = unique_tmp("konoma_diff_nav_test");
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
    let dir = unique_tmp("konoma_follow_session_test");
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
    let outside = unique_tmp("konoma_follow_session_outside.txt");
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

// =============================================================================
// Follow scope leaking across a root change (`follow_root`, 2026-08)
//
// `follow_mode`/`follow_session`/`follow_baseline` are App-level (a session isn't per-tab — `F` is
// a global key), but they describe one specific repo. The root can change out from under a RUNNING
// follow session without ever calling `toggle_follow` (tab switch, `l`/`h`, worktree switch via
// `o`→`w`→`Enter`, paste-jump, a bookmark jump, ...). Before `follow_root`, this silently mixed
// paths from two repos into `n`/`N`'s population (and the title's `(i/n)` denominator), and — worse
// — let `follow_baseline_diff`'s `blob_at` call *successfully* resolve against a linked worktree's
// shared object database using the WRONG worktree's pinned HEAD, producing a diff that looked
// plausible but was wrong.
// =============================================================================

#[cfg(feature = "git")]
#[test]
fn follow_session_and_diff_denominator_do_not_leak_across_tabs_with_different_roots() {
    // Regression ①: switching to a tab rooted in an entirely different repo must not leave the
    // OLD repo's file in the follow session — `diff_change_position`'s denominator must reflect
    // only the NEW repo's changes.
    let dir_x = unique_tmp("konoma_follow_scope_leak_x");
    std::fs::create_dir_all(&dir_x).unwrap();
    git_repo_with_commits(&dir_x);
    let root_x = dir_x.canonicalize().unwrap();

    let dir_y = unique_tmp("konoma_follow_scope_leak_y");
    std::fs::create_dir_all(&dir_y).unwrap();
    git_repo_with_commits(&dir_y);
    let root_y = dir_y.canonicalize().unwrap();

    let mut app = App::new(root_x.clone(), Config::default()).unwrap();
    app.toggle_follow();
    assert!(app.follow_enabled());

    // A change in X, recorded and opened while the tab is still rooted at X.
    std::fs::write(root_x.join("a.txt"), b"one\ntwo\nX-CHANGE\n").unwrap();
    assert!(app.follow_note_change(&root_x.join("a.txt")));
    app.follow_jump(&root_x.join("a.txt"));
    assert!(app.is_git_diff_preview(), "X の変更は全画面 diff で開く");
    assert_eq!(app.diff_change_position(), Some((1, 1)), "X 単独で 1/1");

    // Switch tabs and re-root the new tab at an entirely different repo (Y) — one of several
    // root-changing paths that never call `toggle_follow` (tab switch + `jump_to_dir`, the same
    // primitive `worktree_goto`/a bookmark jump use).
    app.tab_new().expect("tab_new");
    app.jump_to_dir(root_y.clone());
    assert_eq!(app.tab.root, root_y, "新タブの root は Y");

    // The first follow_note_change after a root change recaptures the scope (clears the old
    // session, retakes the baseline, pins follow_root=Y) BEFORE recording anything — a.txt is
    // still clean at this instant, so this call itself folds it into the fresh baseline (the same
    // "pre-existing state becomes invisible" fold a plain F-on already does), not into a visible diff.
    assert!(app.follow_note_change(&root_y.join("a.txt")));
    assert_eq!(
        app.follow_root,
        Some(root_y.clone()),
        "スコープが Y に取り直される"
    );

    // The actual change since recapture — this is what `n`/`N`/diff_change_position should count.
    std::fs::write(root_y.join("a.txt"), b"one\ntwo\nY-CHANGE\n").unwrap();
    assert!(
        app.follow_note_change(&root_y.join("a.txt")),
        "Y での変更は追尾対象"
    );
    app.follow_jump(&root_y.join("a.txt"));
    assert!(app.is_git_diff_preview(), "Y の変更も全画面 diff で開く");
    assert_eq!(
        app.diff_change_position(),
        Some((1, 1)),
        "X の a.txt が母集合に混ざらず、Y 単独で 1/1(修正前は分母が 2 になる)"
    );

    std::fs::remove_dir_all(&dir_x).ok();
    std::fs::remove_dir_all(&dir_y).ok();
}

#[cfg(feature = "git")]
#[test]
fn follow_baseline_diff_is_none_after_switching_to_a_linked_worktree_in_the_same_tab() {
    // Regression ②: linked worktrees share ONE object database. Switching this tab's root to a
    // linked worktree — the same `jump_to_dir` primitive `worktree_goto`'s `Enter` uses, entirely
    // within one tab, never touching `toggle_follow` — must not let a stale `follow_baseline`
    // silently resolve against the OLD worktree's pinned HEAD (which `blob_at` would happily do,
    // since the object is reachable from either worktree).
    let dir_m = unique_tmp("konoma_follow_scope_wt_main");
    std::fs::create_dir_all(&dir_m).unwrap();
    git_repo_with_commits(&dir_m);
    let root_m = dir_m.canonicalize().unwrap();

    let dir_w = unique_tmp("konoma_follow_scope_wt_linked");
    let out = std::process::Command::new("git")
        .current_dir(&root_m)
        .args([
            "worktree",
            "add",
            "-q",
            "-b",
            "wt-follow-scope",
            dir_w.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(out.status.success(), "git worktree add: {out:?}");
    let root_w = dir_w.canonicalize().unwrap();

    let mut app = App::new(root_m.clone(), Config::default()).unwrap();
    app.toggle_follow();
    assert!(app.follow_enabled());
    assert_eq!(app.follow_root, Some(root_m.clone()));

    // A change in main opens correctly against main's baseline while still rooted there.
    std::fs::write(root_m.join("a.txt"), b"one\ntwo\nMAIN-CHANGE\n").unwrap();
    assert!(app.follow_note_change(&root_m.join("a.txt")));
    assert!(
        app.follow_baseline_diff(&root_m.join("a.txt")).is_some(),
        "同一 root ではベースライン diff が引ける"
    );

    // Switch this SAME tab's root to the linked worktree (mirroring worktree_goto's Enter),
    // WITHOUT any follow_note_change in between.
    app.jump_to_dir(root_w.clone());
    assert_eq!(app.tab.root, root_w);

    // A naive blob_at(root_w, <main's pinned HEAD>, a.txt) would SUCCEED here — the object database
    // is shared — and silently produce a diff against the wrong worktree's history. This must
    // refuse instead, purely from the cheap root comparison (no git call at all).
    assert!(
        app.follow_baseline_diff(&root_w.join("a.txt")).is_none(),
        "worktree 切替直後は別 root の基準を静かに使い回さない"
    );

    std::fs::remove_dir_all(&dir_w).ok();
    std::fs::remove_dir_all(&dir_m).ok();
}

#[cfg(feature = "git")]
#[test]
fn follow_note_change_recaptures_baseline_and_session_when_the_scope_root_changes() {
    // Regression ③ (re-acquisition): once the scope goes stale, the NEXT follow_note_change call —
    // the event-drain side, never the render path — rebuilds the session/baseline fresh against the
    // CURRENT root, the same recovery a fresh `F`-on gives, so the old root's path never lingers.
    let dir_x = unique_tmp("konoma_follow_scope_reacquire_x");
    std::fs::create_dir_all(&dir_x).unwrap();
    git_repo_with_commits(&dir_x);
    let root_x = dir_x.canonicalize().unwrap();

    let dir_y = unique_tmp("konoma_follow_scope_reacquire_y");
    std::fs::create_dir_all(&dir_y).unwrap();
    git_repo_with_commits(&dir_y);
    let root_y = dir_y.canonicalize().unwrap();

    let mut app = App::new(root_x.clone(), Config::default()).unwrap();
    app.toggle_follow();
    std::fs::write(root_x.join("a.txt"), b"one\ntwo\nX-CHANGE\n").unwrap();
    assert!(app.follow_note_change(&root_x.join("a.txt")));
    assert_eq!(app.follow_session.len(), 1, "X で 1 件記録");

    app.jump_to_dir(root_y.clone());
    assert!(
        !app.follow_scope_valid(),
        "root 変更直後はスコープ無効(まだ follow_note_change を呼んでいない)"
    );

    // The first event since the root change recaptures (a.txt is still clean here, so it's folded
    // into the fresh baseline rather than the visible diff — see the leak test above for why).
    assert!(app.follow_note_change(&root_y.join("a.txt")));
    assert!(app.follow_scope_valid(), "再取得後はスコープ有効");
    assert_eq!(
        app.follow_root,
        Some(root_y.clone()),
        "follow_root が新しい root に更新される"
    );
    assert_eq!(
        app.follow_session,
        vec![root_y.join("a.txt")],
        "旧 root(X)のパスは残らず新 root(Y)のパスだけになる"
    );

    // The recaptured baseline is actually usable for Y (not just an empty/stale placeholder).
    std::fs::write(root_y.join("a.txt"), b"one\ntwo\nY-CHANGE\n").unwrap();
    assert!(
        app.follow_baseline_diff(&root_y.join("a.txt")).is_some(),
        "再取得後は新 root の基準で diff が引ける"
    );

    std::fs::remove_dir_all(&dir_x).ok();
    std::fs::remove_dir_all(&dir_y).ok();
}

#[cfg(feature = "git")]
#[test]
fn render_path_never_recaptures_the_follow_baseline_after_a_root_change() {
    // Regression ④ (the invariant most likely to be broken by a future change): `follow_baseline_diff`
    // is read from the render path (`ui/preview.rs` calls `diff_change_position`, which shares the
    // same scope check, on every frame). It takes `&self` — the compiler already refuses a
    // `&mut self` recapture from inside it — but pin the *behavioral* guarantee too: repeatedly
    // calling it after a root change must never advance `follow_root`, which only
    // `follow_note_change` (the event-drain side) is allowed to update. If this regresses, a stale
    // scope would trigger a synchronous git-status scan + file reads on every single render.
    let dir_x = unique_tmp("konoma_follow_scope_render_x");
    std::fs::create_dir_all(&dir_x).unwrap();
    git_repo_with_commits(&dir_x);
    let root_x = dir_x.canonicalize().unwrap();

    let dir_y = unique_tmp("konoma_follow_scope_render_y");
    std::fs::create_dir_all(&dir_y).unwrap();
    git_repo_with_commits(&dir_y);
    let root_y = dir_y.canonicalize().unwrap();

    let mut app = App::new(root_x.clone(), Config::default()).unwrap();
    app.toggle_follow();
    assert_eq!(app.follow_root, Some(root_x.clone()));

    // Root changes WITHOUT any follow_note_change call (no fs event has arrived yet).
    app.jump_to_dir(root_y.clone());
    let stale_root = app.follow_root.clone();
    assert_eq!(
        stale_root,
        Some(root_x.clone()),
        "root 変更直後はまだ古い root を指す"
    );
    assert!(!app.follow_scope_valid());

    // Simulate several frames' worth of render-path reads (the same call `diff_change_position`
    // makes every draw) with no event-drain call in between.
    for _ in 0..5 {
        assert!(
            app.follow_baseline_diff(&root_y.join("a.txt")).is_none(),
            "スコープ無効な間は安い判定だけで None を返す"
        );
    }
    assert_eq!(
        app.follow_root, stale_root,
        "描画経路の呼び出しだけでは follow_root(=ベースラインの取得元)が動かない\
         (=同期 git status を毎フレーム走らせない)"
    );
    assert!(
        !app.follow_scope_valid(),
        "描画経路だけではスコープも有効化されない(再取得は event-drain 側の follow_note_change だけの責務)"
    );

    std::fs::remove_dir_all(&dir_x).ok();
    std::fs::remove_dir_all(&dir_y).ok();
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
    let dir = unique_tmp("konoma_md_prefix_test");
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
    let dir = unique_tmp("konoma_md_slice_equiv_test");
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
    let dir = unique_tmp("konoma_mermaid_img_test");
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
    let dir = unique_tmp("konoma_mermaid_txt_test");
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
    let dir = unique_tmp("konoma_mermaid_zoom_test");
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
    let dir = unique_tmp("konoma_mermaid_fence_test");
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
    let dir = unique_tmp("konoma_mermaid_broken_test");
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

    let dir = unique_tmp("konoma_mermaid_focus_test");
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

    let dir = unique_tmp("konoma_fence_scroll_test");
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
///
/// Runs the whole thing **twice**, once per graphics backend: the kitty arm does not call
/// `Picker::new_protocol` at all (it builds konoma's compressed transmit instead) and therefore
/// re-derives the target cell area itself, so the two must be measured against each other. A
/// silent few-percent stretch of every diagram and equation is exactly the kind of thing that
/// would not fail loudly anywhere else.
#[test]
fn encode_worker_scales_fence_diagrams_up_to_grid() {
    // (label, kitty request payload) — None = the ratatui-image path, Some = konoma's own transmit.
    for (label, kitty) in [("ratatui-image", None), ("kitty", Some((1234u32, false)))] {
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
                    kitty,
                })
                .unwrap();
        };
        send("mermaid-fence://cafe"); // a fence diagram → upscaled to the grid (40x10)
        send("/tmp/photo.png"); // a photo → stays at its natural size (20x5)
        drop(req_tx);
        let fence = res_rx.recv().unwrap();
        let photo = res_rx.recv().unwrap();
        h.join().unwrap();
        let fence = fence.image.expect("フェンスのエンコードは成功する");
        assert_eq!(
            fence.cell_size(),
            (40, 10),
            "{label}: フェンスはグリッドを満たす"
        );
        assert_eq!(fence.is_kitty(), kitty.is_some(), "{label}: 経路の選択");
        let photo = photo.image.expect("写真のエンコードは成功する");
        assert_eq!(photo.cell_size(), (20, 5), "{label}: 写真は拡大しない");
    }
}

/// An inline diagram's initial size **fits the display area**: target rows = min(mermaid_rows,
/// viewport-2). Only documents containing a diagram get re-laid-out when the viewport changes;
/// documents without one are not rebuilt (user request 2026-07-17: "fit the display area at first").
#[test]
fn mermaid_initial_size_fits_viewport_and_refits_on_change() {
    let dir = unique_tmp("konoma_mermaid_fit_test");
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

    let dir = unique_tmp("konoma_fence_wrap_align_test");
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

    let dir = unique_tmp("konoma_fence_zoom_test");
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

    let dir = unique_tmp("konoma_fence_rows_test");
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
    let dir = unique_tmp("konoma_mermaid_density_test");
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
    let dir = unique_tmp("konoma_probe_rows100");
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

    let dir = unique_tmp("konoma_overlay_move_test");
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
    let dir = unique_tmp("konoma_graph_pertab_test");
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
    let dir = unique_tmp("konoma_mermaid_tab_restore_test");
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
    let dir = unique_tmp("konoma_fence_tab_restore_test");
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
    let dir = unique_tmp("konoma_fence_ordinal_test");
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
    let dir = unique_tmp("konoma_fence_prune_test");
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
    let dir = unique_tmp("konoma_fence_per_tab_test");
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
    let dir = unique_tmp("konoma_fence_return_cache_test");
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
    let dir = unique_tmp("konoma_encode_fail_test");
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
        image: None,
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
        image: None,
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

    let dir = unique_tmp("konoma_math_encode_test");
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
    let dir = unique_tmp("konoma_vector_inflight_test");
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
    let dir = unique_tmp("konoma_empty_fence_test");
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

    let dir = unique_tmp("konoma_fence_pan_offscreen_test");
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
    let dir = unique_tmp("konoma_stale_md_result_test");
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

/// D2 (2026-08-05): `fence_sharpen_if_needed`'s worker thread called `rasterize_bytes` (resvg — the
/// exact same panic-prone call `ensure_mermaid_fence_render`/`ensure_math_render` already guard)
/// completely unguarded, so a panic there killed the thread before it ever sent anything, latching
/// `reraster_inflight` `true` forever (this fence could never sharpen again). It now catches the
/// panic and always sends a `reraster: true, image: Err(_)` fallback. This tests `apply_md_image`
/// at the boundary that actually matters: feeding it that exact fallback shape must take the
/// existing "a re-raster failure leaves the current raster in place" branch — clearing
/// `reraster_inflight` and changing nothing else (not the initial-failure `entry.failed = true`
/// branch, which is reserved for a diagram that has never rendered at all).
#[test]
fn apply_md_image_with_a_panic_shaped_reraster_failure_clears_inflight_without_degrading() {
    let dir = unique_tmp("konoma_apply_md_image_panic_shaped");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();

    let key = std::path::PathBuf::from("mermaid-fence://deadbeefdeadbeef");
    let existing = std::sync::Arc::new(image::DynamicImage::new_rgba8(120, 48));
    app.md_image_cache.insert(
        key.clone(),
        MdImgEntry {
            decoded: Some(existing.clone()),
            layout_px: Some((120, 48)),
            reraster_inflight: true,
            ..Default::default()
        },
    );

    // Mirrors what fence_sharpen_if_needed's worker now sends when rasterize_bytes panics.
    let panic_fallback = MdImageResult {
        path: key.clone(),
        image: Err("re-raster panicked".to_string()),
        svg: None,
        reraster: true,
        frames: None,
    };
    let redraw = app.apply_md_image(panic_fallback);
    assert!(redraw);
    let entry = app
        .md_image_cache
        .get(&key)
        .expect("エントリは残る(消えない)");
    assert!(
        !entry.reraster_inflight,
        "パニックのフォールバック結果でも inflight が解ける(再ズームが二度と効かなくならない)"
    );
    assert!(
        std::sync::Arc::ptr_eq(entry.decoded.as_ref().unwrap(), &existing),
        "再ラスタ失敗は現在表示中の画像をそのまま保持するはず(全画面が消えない)"
    );
    assert!(
        !entry.failed,
        "初回失敗用の text フォールバックへは降格しないはず(既に表示できていたので)"
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
    let dir = unique_tmp("konoma_kitty_resize_test");
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
    let dir = unique_tmp("konoma_kitty_async_test");
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
    let dir = unique_tmp("konoma_kitty_swap_test");
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
    let dir = unique_tmp("konoma_kitty_fail_test");
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
    let dir = unique_tmp("konoma_status_workdir_cache");
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
    let dir = unique_tmp("konoma_status_reuse_diff_repo");
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
    let dir = unique_tmp("konoma_kitty_switch_race");
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
    let dir = unique_tmp("konoma_git_dir_watch");
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
    let plain = unique_tmp("konoma_git_dir_watch_norepo");
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
    let dir = unique_tmp("konoma_tab_switch_status");
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
    // it re-fetches and the sentinel disappears. Render through the real `crate::ui::render`
    // (rather than calling `refresh_git_if_needed` by hand) so this test exercises the actual call
    // site the fix lives at, instead of resting on the unverified assumption that a render is
    // "equivalent" to it.
    app.tab_cycle(-1);
    {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;
        let mut term = Terminal::new(TestBackend::new(60, 12)).unwrap();
        term.draw(|f| crate::ui::render(f, &mut app)).unwrap();
    }

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
    let dir = unique_tmp("konoma_status_async_offload");
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
    let dir = unique_tmp("konoma_status_async_tabswitch");
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
    let dir = unique_tmp("konoma_status_async_stale");
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
        vcs: crate::vcs::VcsKind::Git,
    };
    assert!(!app.apply_statuses(stale), "古い世代の結果は捨てる");
    assert!(
        app.git_status_of(&sentinel).is_none(),
        "捨てた結果で status を汚染しない"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// D2 (2026-08-05): `spawn_or_sync_statuses`'s worker used to guard the scan with `catch_silent`
/// but only sent a result `if let Some(res) = ...` — so a caught panic sent *nothing*, and
/// `git_status_pending` still latched forever (the exact bug its own comment claimed was fixed).
/// It now always sends a result, falling back to an empty/known-nothing `StatusResult` (correct
/// gen/workdir) on a panic. This tests `apply_statuses` at the boundary that actually matters:
/// feeding it that fallback shape must clear `pending` just like a real scan would (a real git-scan
/// panic can't be induced on demand from a test). The stale-generation case must still discard.
/// Pure state logic (no real git call), so — like `apply_ignored_reflects_current_gen_and_discards_stale`
/// above — it runs under both features.
#[test]
fn apply_statuses_with_a_panic_shaped_result_still_clears_pending() {
    let dir = unique_tmp("konoma_apply_statuses_panic_shaped");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let dir = dir.canonicalize().unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.git_status_gen = 9;
    app.git_status_pending = Some(dir.clone());
    app.git_status
        .insert(dir.join("stale.txt"), crate::git::FileStatus::Modified);

    // Current-generation fallback (mirrors what the worker sends on a caught panic).
    let panic_fallback = crate::app::StatusResult {
        gen: 9,
        workdir: Some(dir.clone()),
        statuses: Default::default(),
        branch: None,
        worktree_origin: None,
        vcs: crate::vcs::VcsKind::Git,
    };
    assert!(app.apply_statuses(panic_fallback), "現世代なので適用される");
    assert!(
        app.git_status_pending.is_none(),
        "パニックのフォールバック結果でも pending が解ける(スピナーが固着せず git status も固まらない)"
    );
    assert!(
        app.git_status.is_empty(),
        "失敗は安全側=空の statuses に倒す"
    );
    assert!(app.git_branch.is_none());

    // A stale-generation fallback must still be discarded.
    app.git_status_gen = 10;
    app.git_status_pending = Some(dir.clone());
    let stale_panic_fallback = crate::app::StatusResult {
        gen: 9,
        workdir: Some(dir.clone()),
        statuses: Default::default(),
        branch: None,
        worktree_origin: None,
        vcs: crate::vcs::VcsKind::Git,
    };
    assert!(
        !app.apply_statuses(stale_panic_fallback),
        "古い世代のフォールバックは捨てる"
    );
    assert!(
        app.git_status_pending.is_some(),
        "stale では pending を残す(現行計算待ちのまま)"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// The render path must not re-dispatch a scan on every frame while one is in flight, and must not
/// re-scan at all once the result has landed (the per-workdir cache = Phase G).
#[cfg(feature = "git")]
#[test]
fn repeated_renders_dispatch_at_most_one_git_status_scan() {
    let dir = unique_tmp("konoma_status_async_norekick");
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
    let dir = unique_tmp("konoma_status_async_coalesce");
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
    let dir = unique_tmp("konoma_status_async_changedfilter");
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

/// **Core regression**: switching to a different repository's tab must re-verify `git_branch`
/// even when the destination view is the Git changes hub (not Tree). `git_branch`/`worktree_origin`
/// used to be re-verified only inside `tree::render`'s own call to `refresh_git_if_needed` — and the
/// changes hub (like log/graph/branches/worktrees/commit-detail and Preview) bypasses `tree::render`
/// entirely (see the content dispatch in `ui::render`), so switching straight from one repo's
/// changes hub to another's never re-verified the branch name. The hub's *file list*
/// (`git_view_entries`) is separate per-tab state rebuilt by `open_git_view`/`git_view_reload` and
/// was already correct across tab switches — only the branch-name title (and the `WT <origin>`
/// chip, covered separately below) lagged behind.
///
/// Renders through the real `crate::ui::render` (not a direct `refresh_git_if_needed()` call), so
/// this exercises the actual call site the fix moved to, rather than the primitive it calls.
#[cfg(feature = "git")]
#[test]
fn git_view_branch_re_verifies_when_switching_to_a_different_repo() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    let repo_a = unique_tmp("konoma_git_view_branch_switch_a");
    let repo_b = unique_tmp("konoma_git_view_branch_switch_b");
    for d in [&repo_a, &repo_b] {
        let _ = std::fs::remove_dir_all(d);
        std::fs::create_dir_all(d).unwrap();
    }
    init_git_repo(&repo_a);
    init_git_repo(&repo_b);
    let git = |cwd: &std::path::Path, args: &[&str]| {
        let out = std::process::Command::new("git")
            .current_dir(cwd)
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {out:?}");
    };
    // repoA: branch "alpha", with a modified tracked file (shows up in the changes hub).
    std::fs::write(repo_a.join("fileA.txt"), b"one\n").unwrap();
    git(&repo_a, &["add", "-A"]);
    git(&repo_a, &["commit", "-qm", "init"]);
    git(&repo_a, &["checkout", "-q", "-b", "alpha"]);
    std::fs::write(repo_a.join("fileA.txt"), b"one\nmodified\n").unwrap();
    // repoB: branch "beta", likewise with a modified tracked file.
    std::fs::write(repo_b.join("fileB.txt"), b"two\n").unwrap();
    git(&repo_b, &["add", "-A"]);
    git(&repo_b, &["commit", "-qm", "init"]);
    git(&repo_b, &["checkout", "-q", "-b", "beta"]);
    std::fs::write(repo_b.join("fileB.txt"), b"two\nmodified\n").unwrap();

    let root_a = repo_a.canonicalize().unwrap();
    let root_b = repo_b.canonicalize().unwrap();

    let render_text = |app: &mut App| -> String {
        let mut term = Terminal::new(TestBackend::new(80, 20)).unwrap();
        term.draw(|f| crate::ui::render(f, app)).unwrap();
        term.backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect()
    };

    // tab0 = repoA, changes hub open, rendered once (establishes git_branch = "alpha").
    let mut app = App::new(root_a.clone(), Config::default()).unwrap();
    app.open_git_view();
    assert!(app.is_git_view(), "テストの前提: Git changes hub にいる");
    let text = render_text(&mut app);
    assert!(
        text.contains("⎇ alpha"),
        "repoA の初回描画で alpha が出ない: {text}"
    );

    // tab1 = repoB, changes hub open, rendered once (establishes git_branch = "beta").
    app.tab_new().unwrap();
    app.tab.root = root_b.clone();
    app.tab.open_dir = root_b.clone();
    app.open_git_view();
    assert!(app.is_git_view());
    let text = render_text(&mut app);
    assert!(
        text.contains("⎇ beta"),
        "repoB の描画で beta が出ない: {text}"
    );

    // Switch back to tab0 (repoA) and render once more: the branch title must follow, not stay on
    // the previously-active tab's repo.
    app.tab_cycle(-1);
    assert!(
        app.is_git_view(),
        "タブ0は Git changes hub のままのはず(per-tab 状態)"
    );
    let text = render_text(&mut app);
    assert!(
        text.contains("⎇ alpha"),
        "別 repo のタブへ戻ったのにブランチ名が更新されない(スケール前の症状): {text}"
    );
    assert!(
        !text.contains("⎇ beta"),
        "前のタブ(repoB)のブランチ名が居座っている: {text}"
    );
    // The change list itself is per-tab state and was already correct before this fix — confirm it
    // stays correct alongside the now-fixed branch name (no regression introduced in that half).
    assert!(
        app.git_view_entries()
            .iter()
            .any(|e| e.path.ends_with("fileA.txt")),
        "変更ファイル一覧は repoA のものであるべき"
    );

    std::fs::remove_dir_all(&repo_a).ok();
    std::fs::remove_dir_all(&repo_b).ok();
}

/// The persistent `WT <origin>` chip (linked-worktree indicator) must likewise re-verify across a
/// tab switch to/from an unrelated repo — in **both directions**, and while the destination tab is
/// showing **Preview**, not just Tree/the Git hub (the chip is drawn by `context_spans`, shared by
/// every mode, so a stale `worktree_origin()` would leak into Preview too if re-verification only
/// happened from `tree::render`).
#[cfg(feature = "git")]
#[test]
fn worktree_chip_re_verifies_across_tab_switch_into_preview() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    // The origin-name assertions below check for the *exact* string, and a `unique_tmp` suffix
    // directly on these leaf names would push the directory's basename past the chip's 20-char
    // truncation budget (`truncate_display`) — same reasoning as
    // `worktree_chip_shows_only_inside_a_linked_worktree` above, which does the same for the same
    // reason. So the leaf names stay short and fixed (`main`/`other`/`linked`), and only the
    // *parent* segment is uniqued — that keeps the full path collision-free across concurrent
    // test runs (a fixed leaf name under a unique parent never collides on disk) without touching
    // the basename the chip actually renders.
    let base = unique_tmp("konoma_wtsw");
    let main_dir = base.join("main");
    let other_dir = base.join("other");
    let linked_dir = base.join("linked");
    for d in [&main_dir, &other_dir] {
        let _ = std::fs::remove_dir_all(d);
        std::fs::create_dir_all(d).unwrap();
    }
    let _ = std::fs::remove_dir_all(&linked_dir);
    init_git_repo(&main_dir);
    init_git_repo(&other_dir);
    let git = |cwd: &std::path::Path, args: &[&str]| {
        let out = std::process::Command::new("git")
            .current_dir(cwd)
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {out:?}");
    };
    std::fs::write(main_dir.join("a.txt"), b"one\n").unwrap();
    git(&main_dir, &["add", "-A"]);
    git(&main_dir, &["commit", "-qm", "init"]);
    std::fs::write(other_dir.join("b.txt"), b"two\n").unwrap();
    git(&other_dir, &["add", "-A"]);
    git(&other_dir, &["commit", "-qm", "init"]);
    let main_root = main_dir.canonicalize().unwrap();
    let other_root = other_dir.canonicalize().unwrap();
    let expected_origin = main_root
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap()
        .to_string();

    git(
        &main_root,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "konoma-wt-chip-switch",
            linked_dir.to_str().unwrap(),
        ],
    );
    let linked_root = linked_dir.canonicalize().unwrap();
    std::fs::write(linked_root.join("note.txt"), b"hello\n").unwrap();

    let render_text = |app: &mut App| -> String {
        let mut term = Terminal::new(TestBackend::new(80, 20)).unwrap();
        term.draw(|f| crate::ui::render(f, app)).unwrap();
        term.backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect()
    };

    // tab0 = other_root (a plain repo, no linked worktree): render once to establish "no chip".
    let mut app = App::new(other_root.clone(), Config::default()).unwrap();
    assert!(
        !render_text(&mut app).contains("WT"),
        "プレーンな repo では WT チップが出てはいけない"
    );

    // tab1 = the linked worktree, in **Preview** mode (not Tree, not the Git hub) — item 3: not
    // just the git full-screen views.
    app.tab_new().unwrap();
    app.tab.root = linked_root.clone();
    app.tab.open_dir = linked_root.clone();
    app.enter_preview(&linked_root.join("note.txt"));
    assert_eq!(
        app.tab.mode,
        Mode::Preview,
        "テストの前提: Preview 面にいる"
    );
    let text = render_text(&mut app);
    assert!(
        text.contains("WT"),
        "linked worktree の Preview では WT チップが出るはず: {text}"
    );
    assert!(
        text.contains(&expected_origin),
        "チップに元 repo 名 {expected_origin} が出ない: {text}"
    );

    // Switch back to tab0 (plain repo) and render: the chip must disappear (not leak the previous
    // tab's linked-worktree origin).
    app.tab_cycle(-1);
    let text = render_text(&mut app);
    assert!(
        !text.contains("WT"),
        "別 repo のタブへ戻ったら WT チップは消えるはず: {text}"
    );

    // Switch forward again to tab1 (the linked worktree) and render: the chip must reappear.
    app.tab_cycle(1);
    let text = render_text(&mut app);
    assert!(
        text.contains("WT") && text.contains(&expected_origin),
        "linked worktree のタブへ戻ったら WT チップが復活するはず: {text}"
    );

    std::fs::remove_dir_all(&base).ok();
}

/// Performance invariant for the new call site: `ui::render` now calls `refresh_git_if_needed`
/// unconditionally every frame (previously only `tree::render` did), so this guards that rendering
/// a **non-Tree** view (the changes hub) repeatedly still dispatches **at most one** git-status scan
/// while one is in flight, and none at all once it has landed and the root hasn't changed —
/// mirroring `repeated_renders_dispatch_at_most_one_git_status_scan`, but through the real
/// `crate::ui::render` call site instead of a direct `refresh_git_if_needed()` loop.
#[cfg(feature = "git")]
#[test]
fn repeated_ui_renders_of_the_changes_hub_do_not_rescan_git_status() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    let dir = unique_tmp("konoma_status_async_render_norekick");
    std::fs::create_dir_all(&dir).unwrap();
    init_git_repo(&dir);
    std::fs::write(dir.join("a.txt"), b"x").unwrap();
    let dir = dir.canonicalize().unwrap();

    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    app.attach_status_loader(tx);
    app.open_git_view();
    assert!(
        app.is_git_view(),
        "テストの前提: changes hub(非 Tree)にいる"
    );

    let render_once = |app: &mut App| {
        let mut term = Terminal::new(TestBackend::new(80, 20)).unwrap();
        term.draw(|f| crate::ui::render(f, app)).unwrap();
    };

    // 20 renders while the 1st scan is still in flight (nothing has been received/applied yet).
    for _ in 0..20 {
        render_once(&mut app);
    }
    let res = rx
        .recv_timeout(std::time::Duration::from_secs(30))
        .expect("1本目の結果");
    assert_eq!(
        rx.try_iter().count(),
        0,
        "非 Tree 面を毎フレーム描画しても走行中に git status を投げ直してはいけない"
    );
    assert!(app.apply_statuses(res));

    // 20 more renders after applying: the per-workdir cache must still prevent a rescan.
    for _ in 0..20 {
        render_once(&mut app);
    }
    assert_eq!(
        rx.try_iter().count(),
        0,
        "適用後も非 Tree 面の毎フレーム描画で再スキャンしてはいけない"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// Commands that answer *from* the status (`d` = open diff) must not report "no changes" merely because
/// a background scan is still running: that reads as a broken feature rather than a slow one.
#[cfg(feature = "git")]
#[test]
fn open_diff_command_waits_for_status_instead_of_reporting_no_changes() {
    let dir = unique_tmp("konoma_status_async_opendiff");
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
    let dir = unique_tmp("konoma_status_async_rootrewind");
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
    let dir = unique_tmp("konoma_tabswitch_single_parse");
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
/// (an image decode, an SVG/mermaid/PDF rasterization, or an `ffmpeg` run costing hundreds of
/// milliseconds). Reuse is keyed on `(path, mtime, page)`, so an externally edited file is still re-read.
#[test]
fn returning_to_a_media_tab_reuses_the_decoded_image() {
    let dir = unique_tmp("konoma_media_cache");
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
    let dir = unique_tmp("konoma_media_cache_cap");
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
    let dir = unique_tmp("konoma_media_cache_fence");
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
    let dir = unique_tmp("konoma_media_cache_close");
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
    let dir = unique_tmp("konoma_media_cache_mtime");
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
    let base = unique_tmp("konoma_changed_filter_two_repos");
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
///
/// The independent "expected position" oracle is `task_source_locs(src, ..)`, deliberately **not**
/// the model renderer's own record (`MdRenderExtras::tasks`) — even though the latter is what
/// `MdItemKind::Task::state_at` is actually read from in production (`app::md_render::build_decorated`).
/// Two reasons, both confirmed while fixing this test's crash on the `"plain blockquote"` corpus case:
///
/// 1. `task_source_locs`'s own contract is calibrated to the **legacy** renderer, not the model one —
///    it is only ever consulted in production when the model renderer falls back to the legacy path,
///    and a plain (non-alert) block quote is a **model-only** decoration `render_quote` draws that the
///    legacy renderer still does not. This is pinned directly, in `preview/markdown.rs`, by
///    `task_scan_parity_tests::heading_gap_is_fixed_and_plain_blockquote_gap_has_no_mismatch` (which
///    asserts `code_block_source_locs("> para\n>\n>     code\n", &[]).is_empty()` as *correct*, not a
///    gap to close) and `task_scan_parity_tests::scanner_counts_exactly_what_the_renderer_draws` (which
///    checks the whole `task_corpus` against the **legacy**-only `render_markdown_tasks`). Teaching
///    `task_source_locs`/`code_block_source_locs` to also decorate a plain quote (tried first) broke
///    both of those and, worse, would make the scanner **over-count** relative to what the legacy
///    renderer actually draws whenever a document both reaches that legacy fallback (a `Table`/`Html`
///    nested in a *different* quote) and separately contains an unrelated plain-quote checkbox/code
///    block — silently shifting every later checkbox's/code block's write-back ordinal by one in
///    production (`build_md_items`'s `task_marks.get(seen)`/`code_blocks.get(seen)` lookup), the exact
///    "silent wrong-byte write" class principle #3 exists to rule out.
/// 2. `MdRenderExtras::tasks` is not (yet) a safe stand-in either: probed directly, it currently
///    reports 2 entries for `"1. [ ] ordered task\n2. [x] done\n"` (an entry already in `code_corpus`)
///    even though the model renderer draws **no** interactive checkbox span for either line (GFM task
///    markers only decorate an unordered-list item — `task_prefix_state`'s own doc comment — and this
///    project's own `task_items`/`is_task_span` scan agrees: 0 on screen). `RenderOut::tasks`'s own doc
///    comment promises it only records a marker "this pass actually drew", so that is model-renderer
///    behavior still in flux (under active, concurrent development elsewhere in this same migration),
///    not a stable oracle to build a byte-position test on.
///
/// So: this test keeps `task_source_locs` as the oracle (its established, tested role), but no longer
/// assumes its count always matches what's on screen. For the (rare, currently exactly-one-entry)
/// documents where a construct the model renders but the legacy scanner does not skip decorates a real,
/// on-screen, toggleable checkbox — `task_source_locs`'s count undershoots `task_items`'s — the
/// byte-position cross-check is skipped for that document (the toggle itself is still driven and still
/// checked for the invariants that do not depend on the scanner: it must not be refused, must change
/// exactly one character, and must preserve CRLF/trailing-newline shape). Never the other direction:
/// `task_source_locs` overshooting `task_items` would mean it invented a checkbox that is not really on
/// screen, which is asserted against directly (that shape is *not* a known, accepted gap).
#[test]
fn md_task_toggle_is_byte_exact_across_the_corpus() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    let dir = unique_tmp("konoma_task_corpus_roundtrip");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let root = dir.canonicalize().unwrap();
    let f = root.join("doc.md");

    // Both corpora: `code_corpus` carries the container-context axis (a checkbox in a nested item
    // written `*   [ ]`, one with nothing after the `]`, one with four spaces), where the state
    // character no longer sits at a fixed offset from the bullet — exactly the case where a wrong
    // `state_off` would silently corrupt a byte of the user's file rather than just refuse.
    let corpus = crate::preview::markdown::task_corpus::cases()
        .into_iter()
        .chain(crate::preview::markdown::code_corpus::cases());
    for (name, src) in corpus {
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

        let locs = crate::preview::markdown::task_source_locs(src, &[' ', 'x'], &[]);
        assert!(
            locs.len() <= task_items.len(),
            "{name}: 書き戻しスキャナが画面より多くのチェックボックスを見つけた\
             (安全な方向=見落としのみのはずが逆転している)\n--- src ---\n{src}"
        );
        // Whether `task_source_locs` describes exactly what is on screen for this document — see
        // this test's own doc comment above for the (currently one) documented exception.
        let scanner_matches_screen = locs.len() == task_items.len();

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
            if scanner_matches_screen {
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
                let after_locs =
                    crate::preview::markdown::task_source_locs(&after, &[' ', 'x'], &[]);
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

/// The `y c` analog of `md_task_toggle_is_byte_exact_across_the_corpus`: drive the real preview
/// pipeline over the whole corpus and, for every code block Tab reaches, copy it and check the text
/// against an independent computation — `render_markdown_with_images`'s own record
/// (`MdRenderExtras::code_blocks`), over the exact preprocessed text the app's own cache would have
/// used (`md_snapshot_tests::pre_src_for`), matched by ordinal. This is the model renderer's own
/// output, **not** a second, hand-rolled scanner (`code_block_source_locs` disagrees with the model
/// renderer in many already-documented, intentional ways unrelated to any bug — see
/// `preview::markdown::app_faithful_parity_tests::code_scanner_matches_the_render_through_the_app_pipeline`'s
/// own doc comment) — so this test's own job is specifically the *plumbing* from that record through
/// to the clipboard (`ensure_md_cache` → `build_md_items` → `MdItemKind::CodeBlock::body` →
/// `focused_code_source`), not a second exercise of the renderer's own correctness (already covered
/// by `model::code_body_text_matches_parser_code_blocks_for_every_non_quote_code_block_in_the_corpus`
/// and the diff harness).
#[test]
fn md_code_block_copy_is_byte_exact_across_the_corpus() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    let dir = unique_tmp("konoma_code_corpus_copy");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let root = dir.canonicalize().unwrap();
    let f = root.join("doc.md");

    let corpus = crate::preview::markdown::task_corpus::cases()
        .into_iter()
        .chain(crate::preview::markdown::code_corpus::cases());
    for (name, src) in corpus {
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

        let code_items: Vec<usize> = app
            .md_items
            .iter()
            .enumerate()
            .filter(|(_, it)| matches!(it.kind, MdItemKind::CodeBlock { .. }))
            .map(|(i, _)| i)
            .collect();
        if code_items.is_empty() {
            continue;
        }

        // Independent oracle: the model renderer's own record, over the same preprocessed text the
        // app's own cache built from — `MermaidSlot::Text` matches what a picker-less test `App`
        // itself resolves to (`font.is_none()` forces `MermaidSlot::Text` regardless of config; see
        // `app::md_render::build_decorated`'s own `mermaid_slot` closure), so a mermaid-tagged fence
        // is drawn as ordinary code on both sides here, consistently.
        let cfg = Config::default();
        let pre_src = super::md_snapshot_tests::pre_src_for(&cfg, src);
        let (_, _, extras) = crate::preview::markdown::render_markdown_with_images(
            &pre_src,
            100,
            crate::preview::markdown::CodeStyle::default(),
            &cfg.ui.theme.code_theme,
            cfg.ui.icons,
            &cfg.ui.md_task_state_chars(),
            &|_: &str, _: Option<u16>| crate::preview::markdown::ImageSlot::Unavailable,
            &|_: &str| crate::preview::markdown::MermaidSlot::Text,
            "mermaid",
            cfg.ui.md_alerts,
            &|_: &str, _: bool| crate::preview::markdown::MathSlot::Raw,
            false,
        );

        assert_eq!(
            code_items.len(),
            extras.code_blocks.len(),
            "{name}: 画面のコードブロック項目数とモデルの記録数が食い違う\n--- src ---\n{src}"
        );

        for (nth, &item_idx) in code_items.iter().enumerate() {
            app.tab.focused_item = Some(item_idx);
            let copied = app.focused_code_text();
            assert_eq!(
                copied.as_deref(),
                Some(extras.code_blocks[nth].as_str()),
                "{name} #{nth}: `y c` がモデルの記録と違う内容をコピーする\n--- src ---\n{src}"
            );
        }
    }
    std::fs::remove_dir_all(&dir).ok();
}

/// The general form of the regression fixed live (2026-08) — an **ordered**-list task item
/// (`"1. [ ] x"`) was pushed into `RenderOut::tasks` unconditionally, the same as an unordered
/// one — even though `decorate_extras`'s own `replace_task_checkbox` never decorates one
/// (`task_prefix_state`'s own "first byte must be `-`/`*`/`+`" contract; a `push_item_marker`'s own
/// ordered-item marker line always starts `"N. "`, a digit, never a bullet — see either's own doc
/// comment). A document mixing an ordered task list *ahead of* an unordered one therefore recorded a
/// "ghost" entry for every invisible ordered checkbox, shifting every later, real, on-screen
/// checkbox's own ordinal down by one in `RenderOut::tasks` — `build_md_items`'s "Nth sentinel found
/// on screen == `task_marks[N]`" pairing (see that field's own doc comment) then handed the
/// **visible** checkbox's own Tab-focus/`Space`-toggle the **invisible** ordered item's own byte
/// offset instead: pressing `Space` on the one checkbox actually on screen silently edited a
/// *different* line of the file — the "toggle writes to the wrong line while the checkbox count
/// still agrees" class principle #3 exists to rule out, one level more subtle than a pure count
/// mismatch (which the existing safe-refusal path already catches): the counts looked fine
/// (`task_marks.len() == 1` even before this fix, `render_item` never having produced more than the
/// one real GFM task on this exact document — the mis-pairing shows up only once a *second*,
/// undrawn task exists earlier in the file to steal the first slot), so a naive "does the write
/// happen at all" check would not have caught it — the position itself has to be checked.
///
/// The exact, real-keypress reproduction (`Tab` then `Space` on the one visible checkbox in a
/// document byte-identical to the one reported live) lives in `e2e_tests.rs`
/// (`e2e_ordered_list_ghost_task_does_not_steal_the_visible_checkboxs_toggle`, which needs the
/// `Sim` keystroke harness this file does not have); this is the general form, verified the way a
/// "record vs. draw" bug actually has
/// to be verified — by rendering and counting, not by reasoning about the code: for **every** case
/// in the golden-snapshot corpus (`all_cases()` — the full `task_corpus`/`code_corpus`/
/// `code_span_corpus`/`preprocess_corpus`/`inline_corpus`/`list_corpus` union *plus* the repo's own
/// real `samples/*.md` files, the same broad net `markdown_render_snapshot_default` runs), the model
/// renderer's own "what did this pass actually draw a sentinel for" records —
/// `RenderOut::tasks`/`RenderOut::code_blocks`, and mermaid's own `ImagePlacement::fence_ord`
/// (`Some` exactly when `mermaid_placeholder_lines`'s own caption sentinel was drawn — see
/// `render_mermaid_slot`'s own doc comment) — must have **exactly** one entry per matching sentinel
/// span (`is_task_span`/`is_code_header_span`/`is_mermaid_header_span`) the render pass actually put
/// on screen for that same document. `Link`'s own "record" (`collapse_links`/`autolink_bare_urls`'s
/// `targets`) needs no equivalent check here: unlike these three, it is never a separately
/// accumulated list indexed by a later, independent "how many have I seen" count — every target is
/// pushed in the exact same loop iteration that produces its own link span, so the two can never
/// drift apart by construction (see `md_text.rs::collapse_links`'s own body).
///
/// `Details`'s own ordinal needs no equivalent count-parity check either — unlike the other three,
/// it has no separately accumulated list to have desynced from in the first place: `build_md_items`
/// computes it purely self-referentially (the count of `Details` items this exact same scan has
/// already found), so the one way it *could* still disagree with what actually gets toggled is
/// `render_details_from_model`'s own `next_details_open` sequence landing on a different ordinal
/// than the sentinel-scan one for the identical document — a distinct risk, already covered by its
/// own dedicated tests (`details_nested_inside_an_alert_does_not_consume_the_top_level_ordinal`,
/// here, and `e2e_tests.rs`'s own `e2e_two_sibling_details_blocks_toggle_independently`, which needs
/// the `Sim` keystroke harness this file does not have).
#[test]
fn model_render_records_exactly_the_sentinels_it_draws_across_the_corpus() {
    // Every non-empty fence becomes a diagram (a fixed, arbitrary placement size) — including the
    // one-time `fences_on` probe (`render_doc`'s own `mermaid_slot("")` call): that probe must
    // answer something other than `Text` to keep extraction on at all, and a *real* empty fence
    // body never reaches this closure a second time to ask again (`render_mermaid_slot`'s own
    // `code.trim().is_empty()` short-circuit renders it directly, bypassing the slot entirely — see
    // `render.rs`'s own `empty_mermaid_fence_uses_the_text_fallback_without_consulting_the_slot`
    // test), so there is no real input this closure ever needs to answer `Text` for at all. This
    // test cares only about whether a sentinel/record pair was produced together, not about real
    // diagram sizing, so no picker/font is needed to drive this meaningfully, unlike the app-level
    // tests above which stay `MermaidSlot::Text` throughout (a font-less test `App` always resolves
    // to `Text` — see `build_decorated`'s own `mermaid_slot` closure) and so never exercise this
    // record/sentinel pair at all.
    let mermaid_slot = |_: &str| crate::preview::markdown::MermaidSlot::Image { cols: 6, rows: 2 };
    let cfg = Config::default();
    let mut checked_a_mermaid_fence = false;
    for (name, src) in super::md_snapshot_tests::all_cases() {
        let pre_src = super::md_snapshot_tests::pre_src_for(&cfg, &src);
        let (lines, images, extras) = crate::preview::markdown::render_markdown_with_images(
            &pre_src,
            100,
            crate::preview::markdown::CodeStyle::default(),
            &cfg.ui.theme.code_theme,
            cfg.ui.icons,
            &cfg.ui.md_task_state_chars(),
            &|_: &str, _: Option<u16>| crate::preview::markdown::ImageSlot::Unavailable,
            &mermaid_slot,
            "mermaid",
            cfg.ui.md_alerts,
            &|_: &str, _: bool| crate::preview::markdown::MathSlot::Raw,
            false,
        );
        let all_spans = || lines.iter().flat_map(|l| l.spans.iter());
        let drawn_tasks = all_spans()
            .filter(|s| crate::preview::markdown::is_task_span(s))
            .count();
        assert_eq!(
            extras.tasks.len(),
            drawn_tasks,
            "{name}: RenderOut::tasks の件数({}) と実際に描かれたチェックボックス数({drawn_tasks}) が食い違う\n--- src ---\n{src}",
            extras.tasks.len()
        );
        let drawn_code = all_spans()
            .filter(|s| crate::preview::markdown::is_code_header_span(s))
            .count();
        assert_eq!(
            extras.code_blocks.len(),
            drawn_code,
            "{name}: RenderOut::code_blocks の件数({}) と実際に描かれたヘッダ数({drawn_code}) が食い違う\n--- src ---\n{src}",
            extras.code_blocks.len()
        );
        let drawn_mermaid = all_spans()
            .filter(|s| crate::preview::markdown::is_mermaid_header_span(s))
            .count();
        let recorded_mermaid = images.iter().filter(|p| p.fence_ord.is_some()).count();
        assert_eq!(
            recorded_mermaid,
            drawn_mermaid,
            "{name}: 描かれた mermaid キャプション数({drawn_mermaid}) と fence_ord を持つ記録数({recorded_mermaid}) が食い違う\n--- src ---\n{src}"
        );
        if drawn_mermaid > 0 {
            checked_a_mermaid_fence = true;
        }
    }
    assert!(
        checked_a_mermaid_fence,
        "corpus に mermaid フェンスを含む文書が一つも無かった(このチェック自体が空振りしている)"
    );
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
    let dir = unique_tmp("konoma_md_task_front_matter");
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
    let dir = unique_tmp("konoma_md_task_beyond_cap");
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
    let dir = unique_tmp("konoma_md_code_beyond_cap");
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
        .filter(|(_, it)| matches!(it.kind, MdItemKind::CodeBlock { .. }))
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
    let base = unique_tmp("konoma_ignored_leave_repo");
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
    let dir = unique_tmp("konoma_wrap_task_reveal");
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
    let dir = unique_tmp("konoma_wrap_code_reveal");
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

/// `[external] pdf = false` means "never spawn macOS's bundled qlmanage/sips" — it does **not**
/// disable PDF preview, because `hayro` (the primary renderer) and `page_count` (`hayro-syntax`) are
/// both pure Rust and never touch an external process. `tab.pdf_pages` must resolve identically
/// (`Some(3)`, the bundled sample's known page count) whether the flag is on or off — unlike before
/// `hayro` existed, when this flag (and `arbitrary_page_renderer_available`) gated whether page
/// navigation was exposed at all.
#[test]
fn pdf_page_count_and_native_render_work_even_with_external_pdf_disabled() {
    let Some(p) = sample_path_or_skip("sample.pdf") else {
        return;
    };
    let dir = unique_tmp("konoma_pdf_disabled_test");
    std::fs::create_dir_all(&dir).unwrap();

    let mut cfg = Config::default();
    cfg.external.pdf = false;
    let mut app = App::new(dir.clone(), cfg).unwrap();
    app.enter_preview(&p);
    assert_eq!(
        app.tab.pdf_pages,
        Some(3),
        "pdf=false: page_count (hayro-syntax) still runs — it never touched an external process"
    );

    // Same with the flag enabled (default) — external.pdf must not change what pdf_pages resolves to.
    let mut app2 = App::new(dir.clone(), Config::default()).unwrap();
    app2.enter_preview(&p);
    assert_eq!(
        app2.tab.pdf_pages,
        Some(3),
        "external.pdf doesn't change the resolved page count"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// `[external] video = false` must **not** disable video thumbnails as such — only the external
/// extractors. H.264 in mp4/m4v/mov and in mkv/webm is decoded in pure Rust, in-process, so it has
/// to keep working with the flag off, exactly like PDF keeps working via `hayro`. (Before the
/// built-in decoder existed this flag was the whole feature's switch, and this test asserted the
/// opposite.)
///
/// Both containers are checked through the **whole app path**, not just the extractor: preview-kind
/// resolution sniffs content, the media job runs, and `image_src` ends up set. That is a different
/// question from `preview::video`'s own tests — a container the extractor handles perfectly is still
/// invisible if nothing upstream classifies the file as a video in the first place.
///
/// Deliberately does **not** probe for ffmpeg: the point is that the picture appears on a machine
/// with no external tool at all, and with the flag denying their use even if there were one.
#[test]
fn video_thumbnail_is_native_even_when_external_video_disabled() {
    let dir = unique_tmp("konoma_video_native_flag_test");
    std::fs::create_dir_all(&dir).unwrap();

    for name in ["sample.mp4", "sample.mkv"] {
        let Some(p) = sample_path_or_skip(name) else {
            continue;
        };
        let mut cfg = Config::default();
        cfg.external.video = false;
        let mut app = App::new(dir.clone(), cfg).unwrap();
        app.picker = Some(test_picker());
        app.enter_preview(&p);
        assert!(
            matches!(app.tab.preview_kind, Some(PreviewKind::Video(_))),
            "{name}: 動画プレビューとして解決されていない(でなければ以下は何も検査していない)"
        );
        assert!(
            app.image_src.is_some(),
            "{name}: video=false でも H.264 は内蔵デコーダでサムネイルになる(フラグは外部ツール専用)"
        );
        assert!(
            !app.is_media_loading(),
            "{name}: no job left pending either"
        );
    }

    std::fs::remove_dir_all(&dir).ok();
}

/// The other half of the flag's meaning: for a video the **built-in decoder declines**, `[external]
/// video = false` really does keep the external tools from running, and `image_src` stays `None`
/// (the render side then shows the same "unavailable" hint a missing tool would).
///
/// The fixture is `samples/sample.mp4`'s bytes under a `.mkv` name. That is genuinely the shape this
/// branch is for: preview-kind resolution sniffs *content* (`infer`), so it still resolves to
/// `PreviewKind::Video`, while the built-in path declines it — the extension routes it to the
/// Matroska demuxer, which correctly refuses a file that is actually ISO-BMFF. (Before mkv support
/// the refusal came one layer earlier, from the extension gate itself; the outcome this test asserts
/// is the same either way.) Using a real VP9 or AV1 clip would exercise the same branch through the
/// codec check instead, but no such fixture is committed — and generating one would need the very
/// tool this test must not depend on.
#[test]
fn video_thumbnail_falls_back_to_external_tools_only_when_allowed() {
    let Some(src) = sample_path_or_skip("sample.mp4") else {
        return;
    };
    let dir = unique_tmp("konoma_video_external_gate_test");
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("clip.mkv");
    std::fs::copy(&src, &p).unwrap();

    let mut cfg = Config::default();
    cfg.external.video = false;
    let mut app = App::new(dir.clone(), cfg).unwrap();
    app.picker = Some(test_picker());
    app.enter_preview(&p);
    assert!(
        matches!(app.tab.preview_kind, Some(PreviewKind::Video(_))),
        "sanity: このファイルは動画プレビューとして解決される(でなければ以下は何も検査していない)"
    );
    assert!(
        app.image_src.is_none(),
        "video=false: 内蔵デコーダが扱えないファイルで外部ツールを起動してはいけない"
    );
    assert!(!app.is_media_loading(), "no job left pending either");

    // Sanity: with the flag enabled and ffmpeg installed, a thumbnail IS produced — this is what
    // proves the assertion above is testing the *flag*, not just "no tool happens to be installed
    // on this machine" (the two are indistinguishable from `image_src.is_none()` alone). Probe
    // `ffmpeg -version` directly rather than inferring availability from whether the thumbnail
    // happened to appear: without this independent probe, an `if image_src.is_none() { eprintln!(..) }`
    // with no `else` verifies nothing either way — a broken gate and a genuinely absent tool both
    // print the same "skip" line and let the test pass.
    let has_ffmpeg = std::process::Command::new("ffmpeg")
        .arg("-version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !has_ffmpeg {
        eprintln!("SKIP sanity: ffmpeg not on PATH");
    } else {
        let mut app2 = App::new(dir.clone(), Config::default()).unwrap();
        app2.picker = Some(test_picker());
        app2.enter_preview(&p);
        assert!(
            app2.image_src.is_some(),
            "sanity: with the flag enabled and ffmpeg available, a thumbnail IS produced"
        );
    }

    std::fs::remove_dir_all(&dir).ok();
}

/// The preprocessing corpus driven through the real preview pipeline, toggling **every** checkbox in
/// every document. Sibling of `md_task_toggle_is_byte_exact_across_the_corpus`, with two differences
/// that are the whole point of it.
///
/// First, **refusing is an allowed outcome here.** These documents deliberately include shapes where
/// a pre-pass invents a checkbox (a `<br>` splitting a line) or moves one out of the body (a checkbox
/// carried inside a footnote definition), and for those the honest answer is to write nothing.
///
/// Second, and decisively, **the ground truth is the state of the checkboxes on screen**, recovered
/// by re-rendering the file afterwards — not by re-scanning the raw source. Scanning the raw source
/// is exactly the assumption that failed: on the released build, toggling the first checkbox on
/// screen wrote `x` into a line that the screen was drawing as *code*, and every guard and every
/// raw-source assertion agreed with each other while doing it. Re-rendering asks the only question a
/// reader can check: "did the box I pressed, and only that box, change?"
///
/// Third — and this is the guard the test has to keep on *itself* — "refusing is an allowed outcome"
/// is exactly the structural hole that shipped as a real regression three times (v0.18.1, v0.23.5,
/// v0.23.6): a whole document, or every document, silently refusing to toggle *anything* still makes
/// every assertion in the loop below vacuously true (the loop over `task_items` just runs zero times,
/// or every iteration takes the early "refused, file unchanged" branch). `DOCS_WITH_TASKS` pins,
/// document by document, that a checkbox is actually drawn where the corpus means one to be — a
/// document dropping to zero rendered checkboxes is not silently skipped, it fails loudly. `wrote_ok`
/// pins the same thing at the level of the whole corpus: at least one toggle across all of it has to
/// actually write, or the test fails — it does not accept "every single toggle everywhere refused" as
/// a pass.
#[test]
fn md_task_toggle_never_writes_to_the_wrong_checkbox_across_the_preprocess_corpus() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    // Corpus document names known — by actually rendering the whole corpus once and counting what
    // lands in `md_items` — to draw at least one checkbox on screen. Declared explicitly rather than
    // just letting documents with zero rendered tasks fall through the loop below unnoticed: the
    // rest of the corpus (a plain footnote, a bare `<kbd>`, front matter alone, ...) legitimately
    // never produces a checkbox at all, and lumping "legitimately none" together with "regressed to
    // none" is what would let the next occurrence of this exact failure back in unnoticed.
    const DOCS_WITH_TASKS: &[&str] = &[
        "br making a list interrupt a paragraph",
        "br on a checkbox line",
        "kbd on a checkbox line",
        "checkbox carrying a ref",
        "checkbox carrying a ref and a continued def",
        "checkbox in an alert plus a continued def",
        "cjk checkbox with a ref",
        "br injected checkbox above a del-fence swallowed one",
        "footnote leftover plus br injected checkbox",
    ];

    let dir = unique_tmp("konoma_preprocess_corpus_roundtrip");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let root = dir.canonicalize().unwrap();
    let f = root.join("doc.md");

    /// Open `doc.md` fresh and draw one frame, returning the app at the Markdown preview.
    fn opened(root: &std::path::Path) -> App {
        let mut app = App::new(root.to_path_buf(), Config::default()).unwrap();
        app.tab.selected = app
            .tab
            .entries
            .iter()
            .position(|e| e.path.ends_with("doc.md"))
            .unwrap();
        app.tree_activate().unwrap();
        let mut term = Terminal::new(TestBackend::new(100, 40)).unwrap();
        term.draw(|fr| crate::ui::render(fr, &mut app)).unwrap();
        app
    }
    /// The state character of each checkbox **as drawn**, in document order.
    fn drawn_states(app: &App) -> Vec<char> {
        app.md_items
            .iter()
            .filter_map(|it| match it.kind {
                MdItemKind::Task { state, .. } => Some(state),
                _ => None,
            })
            .collect()
    }

    // Toggles that actually wrote, across the entire corpus (not per document — a single document
    // is allowed to refuse every one of its checkboxes, e.g. because they're all `<br>`-injected
    // phantoms; the corpus as a whole is not).
    let mut wrote_ok = 0usize;

    for (name, src) in crate::preview::markdown::preprocess_corpus::cases() {
        std::fs::write(&f, src).unwrap();
        let app = opened(&root);
        let before_states = drawn_states(&app);
        let task_items: Vec<usize> = app
            .md_items
            .iter()
            .enumerate()
            .filter(|(_, it)| matches!(it.kind, MdItemKind::Task { .. }))
            .map(|(i, _)| i)
            .collect();

        if DOCS_WITH_TASKS.contains(&name) {
            assert!(
                !task_items.is_empty(),
                "{name}: この文書は画面に少なくとも1個チェックボックスを描くはずが0個だった \
                 (v0.18.1/v0.23.5/v0.23.6 で3回出荷した「トグルが文書全体を拒否する」症状の兆候 — \
                 このアサート自体が守っているもの)"
            );
        }

        for (nth, &item_idx) in task_items.iter().enumerate() {
            std::fs::write(&f, src).unwrap();
            let mut app = opened(&root);
            app.tab.focused_item = Some(item_idx);
            app.flash = None;
            app.md_toggle_focused_task();
            let after = std::fs::read_to_string(&f).unwrap();

            if app.flash.is_some() {
                assert_eq!(
                    after, src,
                    "{name} #{nth}: 拒否したのにファイルが変わった\n--- after ---\n{after}"
                );
                continue;
            }
            wrote_ok += 1;
            // Wrote: exactly one character, and the file's shape is untouched.
            assert_eq!(
                src.chars().count(),
                after.chars().count(),
                "{name} #{nth}: 文字数が変わった(1文字置換のはず)\n--- after ---\n{after}"
            );
            let diff = src
                .char_indices()
                .zip(after.char_indices())
                .filter(|((_, a), (_, b))| a != b)
                .count();
            assert_eq!(
                diff, 1,
                "{name} #{nth}: 変更が1文字でない\n--- after ---\n{after}"
            );
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
            // The decisive check: re-render the written file and compare what a reader would see.
            // Exactly the pressed checkbox changed state; every other one is untouched.
            let reopened = opened(&root);
            let after_states = drawn_states(&reopened);
            assert_eq!(
                after_states.len(),
                before_states.len(),
                "{name} #{nth}: 書き戻しで画面のチェックボックス数が変わった\n--- after ---\n{after}"
            );
            for (i, (b, a)) in before_states.iter().zip(after_states.iter()).enumerate() {
                if i == nth {
                    assert_ne!(b, a, "{name} #{nth}: 押した箱の状態が変わっていない");
                } else {
                    assert_eq!(
                        b, a,
                        "{name} #{nth}: 押していない箱 #{i} が書き換わった\n--- after ---\n{after}"
                    );
                }
            }
        }
    }
    assert!(
        wrote_ok > 0,
        "コーパス全体で1回もトグルが書き込みに成功しなかった \
         (「拒否は正当」を口実に文書全体/コーパス全体を黙って拒否しても緑になっていた抜け穴 — \
         v0.18.1/v0.23.5/v0.23.6 で3回出荷した症状そのもの。このアサート自体が守っているもの)"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// The document that made the released build corrupt a file, kept as its own regression case so the
/// exact reported shape can never come back silently. On screen it draws two checkboxes (`REAL` and
/// an `INJECTED` one a `<br>` created); the raw source contains two others (`LEFTOVER`, hidden inside
/// what the screen draws as code, and `REAL`). Counts matched, states matched, both guards passed —
/// and pressing the *first* box on screen wrote `x` into the `LEFTOVER` line, with no flash.
#[test]
fn md_task_toggle_refuses_the_documented_corruption_shape() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    let dir = unique_tmp("konoma_task_corruption_regression");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let root = dir.canonicalize().unwrap();
    let f = root.join("doc.md");
    let src =
        "[^1]: def text\n    - [ ] LEFTOVER\n\n- [ ] REAL\n\nprose<br>- [ ] INJECTED\n\nSee[^1].\n";
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

    let first_task = app
        .md_items
        .iter()
        .position(|it| matches!(it.kind, MdItemKind::Task { .. }))
        .expect("画面にチェックボックスがある");
    app.tab.focused_item = Some(first_task);
    app.flash = None;
    app.md_toggle_focused_task();

    // The invariant is *not* "it refuses" — refusing and correctly toggling `REAL` are both fine
    // answers, and which one comes out depends on how much of the document the pre-passes still
    // disturb. The invariant is that the line the reader never touched is never written: the
    // released build turned `- [ ] LEFTOVER` into `- [x] LEFTOVER` while leaving `REAL` unchecked.
    let after = std::fs::read_to_string(&f).unwrap();
    assert!(
        after.contains("- [ ] LEFTOVER"),
        "押していない LEFTOVER 行を書き換えた(公開版 v0.23.5 の挙動)\n--- after ---\n{after}"
    );
    assert!(
        after == src || after.contains("- [x] REAL"),
        "書いたのなら画面で押した REAL でなければならない\n--- after ---\n{after}"
    );
    if after == src {
        assert!(app.flash.is_some(), "書かないなら黙らずに通知する");
    }
    std::fs::remove_dir_all(&dir).ok();
}

// =============================================================================================
// `collect_all` (the `/` filter's population scan) — syscall-count, behaviour and scaling guards.
//
// Why count syscalls rather than time: the walk's cost is dominated by *how many stat calls per
// entry* it makes, and that is a structural property the code either has or doesn't. A wall-clock
// bound on the same thing flakes on a shared CI runner (this repo removed all of its timing
// bounds for exactly that reason), while "zero stats for an ordinary entry" is exact and stable.
//
// The counter is thread-local (`test_support::count_stat_calls`), so these exact-count assertions
// stay valid when the suite runs in parallel.
//
// NOTE on what the counter can and cannot see: it counts the walk's *own* stat calls. On a
// filesystem that does not fill in readdir's `d_type` (so `DirEntry::file_type()` has to lstat
// internally), the kernel still does a syscall the counter cannot observe. Every filesystem
// konoma targets in practice (APFS, HFS+, ext4, btrfs, xfs) fills `d_type` in, which is what makes
// "0" the meaningful number here.

/// Fixture: `n` plain files in one directory. Returns the directory.
fn stat_fixture_flat(prefix: &str, n: usize) -> PathBuf {
    let dir = unique_tmp(prefix);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for i in 0..n {
        std::fs::write(dir.join(format!("f{i}.rs")), b"x").unwrap();
    }
    dir
}

#[test]
fn collect_all_costs_no_stat_syscall_for_ordinary_entries() {
    // The population scan must read the entry kind straight from readdir's d_type
    // (`DirEntry::file_type`), the same rule `child_meta` already follows on `build_dir`'s side.
    // Before this guard the scan stat'ed **twice per entry** (symlink_metadata + is_dir), which is
    // what made pressing `/` on a large tree stall the UI thread.
    let small = stat_fixture_flat("konoma_collect_stat_small", 20);
    let large = stat_fixture_flat("konoma_collect_stat_large", 200);

    let (v_small, stats_small) =
        crate::test_support::count_stat_calls(|| collect_all(&small, false));
    let (v_large, stats_large) =
        crate::test_support::count_stat_calls(|| collect_all(&large, false));

    assert_eq!(v_small.len(), 20, "全ファイルを集める");
    assert_eq!(v_large.len(), 200, "全ファイルを集める");
    assert_eq!(
        stats_small, 0,
        "symlink の無いツリーでは stat は 1 回も要らない"
    );
    // The point of the guard: the count must not grow with the tree.
    assert_eq!(
        stats_large, 0,
        "エントリ数が 10 倍でも stat は 0 回のまま(件数に比例して増えてはいけない)"
    );

    std::fs::remove_dir_all(&small).ok();
    std::fs::remove_dir_all(&large).ok();
}

#[cfg(unix)]
#[test]
fn collect_all_stats_only_symlinks_and_keeps_link_following_is_dir() {
    // A symlink is the one case that still needs a stat, because `is_dir` must follow the link
    // (a symlinked directory stays browsable) while descent must not (that's the loop guard).
    use std::os::unix::fs::symlink;
    let dir = unique_tmp("konoma_collect_stat_symlink");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("real_dir")).unwrap();
    std::fs::write(dir.join("real_dir").join("inner.txt"), b"x").unwrap();
    std::fs::write(dir.join("real_file.txt"), b"x").unwrap();
    symlink(dir.join("real_dir"), dir.join("link_to_dir")).unwrap();
    symlink(dir.join("real_file.txt"), dir.join("link_to_file")).unwrap();
    symlink(dir.join("nope"), dir.join("link_broken")).unwrap();

    let (v, stats) = crate::test_support::count_stat_calls(|| collect_all(&dir, false));

    // Exactly one stat per symlink — and none for the four ordinary entries.
    assert_eq!(
        stats, 3,
        "stat は symlink の本数(3)ぶんだけ: ordinary entry では 0 回"
    );

    let is_dir_of = |name: &str| {
        v.iter()
            .find(|e| e.path.file_name().unwrap() == name)
            .unwrap_or_else(|| panic!("{name} が見つからない"))
            .is_dir
    };
    // Behaviour must be identical to the old symlink_metadata + is_dir pair.
    assert!(is_dir_of("real_dir"), "実ディレクトリは is_dir");
    assert!(
        is_dir_of("link_to_dir"),
        "ディレクトリへの symlink はリンク先を追って is_dir=true(従来どおり)"
    );
    assert!(!is_dir_of("real_file.txt"), "ファイルは is_dir=false");
    assert!(
        !is_dir_of("link_to_file"),
        "ファイルへの symlink は is_dir=false"
    );
    assert!(
        !is_dir_of("link_broken"),
        "壊れた symlink は is_dir=false(stat 失敗→false)"
    );
    // Descent stops at the symlink: inner.txt appears once (via real_dir), never via link_to_dir.
    let inner: Vec<_> = v
        .iter()
        .filter(|e| e.path.file_name().unwrap() == "inner.txt")
        .collect();
    assert_eq!(
        inner.len(),
        1,
        "symlink されたディレクトリには潜らない(ループ防止): inner.txt は real_dir 経由の1件のみ"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[cfg(unix)]
#[test]
fn collect_all_does_not_loop_on_a_symlink_cycle() {
    // The classic reason the walk refuses to follow symlinked directories.
    use std::os::unix::fs::symlink;
    let dir = unique_tmp("konoma_collect_symlink_cycle");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("a").join("b")).unwrap();
    // a/b/loop -> a  (following it would recurse forever)
    symlink(dir.join("a"), dir.join("a").join("b").join("loop")).unwrap();

    let v = collect_all(&dir, false);

    assert!(
        v.len() < 10,
        "循環 symlink に入り込まず有限で終わる: got {}",
        v.len()
    );
    assert!(
        v.iter().any(|e| e.path.file_name().unwrap() == "loop"),
        "リンク自体は結果に現れる(辿らないだけ)"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn collect_all_honors_show_hidden_both_ways() {
    let dir = unique_tmp("konoma_collect_hidden");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join(".hidden_dir")).unwrap();
    std::fs::write(dir.join(".hidden_dir").join("deep.txt"), b"x").unwrap();
    std::fs::write(dir.join(".dotfile"), b"x").unwrap();
    std::fs::write(dir.join("visible.txt"), b"x").unwrap();

    let shown = collect_all(&dir, false);
    let names: Vec<_> = shown
        .iter()
        .map(|e| e.path.file_name().unwrap().to_string_lossy().to_string())
        .collect();
    assert_eq!(
        names,
        vec!["visible.txt"],
        "隠しは除外し、その中にも潜らない"
    );

    let all = collect_all(&dir, true);
    let names: Vec<String> = all
        .iter()
        .map(|e| e.path.file_name().unwrap().to_string_lossy().to_string())
        .collect();
    assert!(
        names.contains(&".dotfile".to_string()),
        "show_hidden で出る"
    );
    assert!(
        names.contains(&"deep.txt".to_string()),
        "show_hidden なら隠しディレクトリの中まで辿る"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn collect_all_truncates_at_the_cap() {
    // The production cap is 50,000; the behaviour is exercised through the injected-cap seam so
    // this doesn't need a 50,000-file fixture (see `collect_all_capped`).
    assert_eq!(COLLECT_CAP, 50_000, "本番の打ち切り件数");
    let dir = stat_fixture_flat("konoma_collect_cap", 25);

    let capped = collect_all_capped(&dir, false, 10);
    assert_eq!(capped.len(), 10, "cap ちょうどで打ち切る");
    let uncapped = collect_all_capped(&dir, false, 1_000);
    assert_eq!(uncapped.len(), 25, "cap 以下なら全部返る");

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn collect_all_returns_ascending_path_order() {
    let dir = unique_tmp("konoma_collect_order");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("b_dir")).unwrap();
    std::fs::write(dir.join("b_dir").join("z.txt"), b"x").unwrap();
    std::fs::write(dir.join("a.txt"), b"x").unwrap();
    std::fs::write(dir.join("c.txt"), b"x").unwrap();

    let v = collect_all(&dir, false);
    // Pin down the count first: `assert_eq!(paths, sorted, ..)` and `v.iter().all(..)` below both
    // hold vacuously (trivially pass) for an empty `v` — an early-return bug in `collect_all` (or
    // `collect_scan` underneath it) that silently walked nothing would still make this test go
    // green. 4 entries: the two top-level files, the one directory, and the file nested inside it
    // (`collect_all` walks recursively and returns directories as entries too — see its doc).
    assert_eq!(
        v.len(),
        4,
        "a.txt, b_dir, b_dir/z.txt, c.txt の4件が返るはず: {v:?}"
    );
    let paths: Vec<_> = v.iter().map(|e| e.path.clone()).collect();
    let mut sorted = paths.clone();
    sorted.sort();
    assert_eq!(paths, sorted, "パス昇順で返る(表示順の前提)");
    // Every entry is flat: depth 0, not expanded.
    assert!(
        v.iter().all(|e| e.depth == 0 && !e.expanded),
        "フラット(depth=0/expanded=false)で返る"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[cfg(unix)]
#[test]
fn collect_all_and_build_dir_agree_on_is_dir() {
    // Two independent walks decide `is_dir` for the same entries. They must not drift — this is
    // the guard against "fixed one, forgot the other" (both now share `child_meta`'s rule:
    // d_type when available, a link-following stat only for symlinks).
    use std::collections::HashSet;
    use std::os::unix::fs::symlink;
    let dir = unique_tmp("konoma_collect_vs_build_dir");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    std::fs::write(dir.join("plain.txt"), b"x").unwrap();
    symlink(dir.join("sub"), dir.join("link_dir")).unwrap();
    symlink(dir.join("plain.txt"), dir.join("link_file")).unwrap();
    symlink(dir.join("missing"), dir.join("link_dead")).unwrap();

    let mut built = Vec::new();
    build_dir(&dir, 0, &HashSet::new(), false, Sort::default(), &mut built).unwrap();

    let collected = collect_all(&dir, false);
    let pick = |v: &[Entry]| -> Vec<(String, bool)> {
        let mut out: Vec<(String, bool)> = v
            .iter()
            // build_dir only lists direct children; compare like for like.
            .filter(|e| e.path.parent() == Some(dir.as_path()))
            .map(|e| {
                (
                    e.path.file_name().unwrap().to_string_lossy().to_string(),
                    e.is_dir,
                )
            })
            .collect();
        out.sort();
        out
    };

    assert_eq!(
        pick(&collected),
        pick(&built),
        "collect_all と build_dir の is_dir 判定が一致すること(二重実装のドリフト防止)"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn collect_all_allocation_scales_linearly() {
    // Deterministic stand-in for "doesn't blow up": doubling the entry count should roughly double
    // the allocation, not quadruple it. Same technique as `filter_fuzzy_large_pool_is_bounded`.
    let small = stat_fixture_flat("konoma_collect_alloc_small", 1_000);
    let large = stat_fixture_flat("konoma_collect_alloc_large", 2_000);
    // Warm: first call in the process touches lazily-initialised bits we don't want in the ratio.
    let _ = collect_all(&small, false);

    let a_small = crate::mem_tests::allocated_by(|| {
        assert_eq!(collect_all(&small, false).len(), 1_000);
    });
    let a_large = crate::mem_tests::allocated_by(|| {
        assert_eq!(collect_all(&large, false).len(), 2_000);
    });
    assert!(
        a_large < a_small.saturating_mul(3),
        "2倍の件数で確保バイト数が3倍を超えた(回帰: O(n^2)?): small={a_small} large={a_large}"
    );

    std::fs::remove_dir_all(&small).ok();
    std::fs::remove_dir_all(&large).ok();
}

/// Extract one `fn NAME` item's source text from `src/app.rs` (up to the closing brace in column 0).
/// Panics if the function can't be found, so a rename fails the guard loudly instead of vacuously.
fn app_rs_fn_source(name: &str) -> String {
    let src = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src")
            .join("app.rs"),
    )
    .expect("src/app.rs を読めない");
    let start = src
        .find(&format!("\nfn {name}("))
        .unwrap_or_else(|| panic!("fn {name} が src/app.rs に見つからない(改名された?)"));
    let rest = &src[start + 1..];
    let end = rest
        .find("\n}\n")
        .unwrap_or_else(|| panic!("fn {name} の終端が見つからない"));
    rest[..end].to_string()
}

#[test]
fn directory_walks_route_every_stat_through_the_counted_choke_point() {
    // Companion to the count guards above. Those can only see stats that go through
    // `stat_follow`; a reintroduced *raw* `fs::metadata` / `path.is_dir()` would do a syscall the
    // counter never observes, so the count guards would pass while the regression was back. This
    // reads the two walk functions' own source and forbids the raw forms outright, which is what
    // makes "0 stat calls" a trustworthy statement rather than a bookkeeping artifact.
    //
    // `stat_follow` is the single sanctioned door (it bumps the counter, then calls fs::metadata).
    const FORBIDDEN: &[(&str, &str)] = &[
        ("fs::metadata(", "stat_follow を使う"),
        ("fs::symlink_metadata(", "DirEntry::file_type() を使う"),
        (
            "path.is_dir()",
            "内部で stat する: stat_follow か d_type を使う",
        ),
        ("path.is_file()", "内部で stat する"),
        ("path.exists()", "内部で stat する"),
        (".symlink_metadata()", "DirEntry::file_type() を使う"),
    ];
    // Safety valve: prove we actually read the real bodies before asserting absence.
    // `collect_scan` is where the filter-pool walk's body lives (`collect_all`/`collect_all_capped`
    // are test-only wrappers over it), so that is the one that has to stay stat-free in the shipped
    // binary.
    for name in ["collect_scan", "child_meta", "build_dir"] {
        let body = app_rs_fn_source(name);
        assert!(
            body.len() > 200,
            "{name} の本文が短すぎる(抽出が壊れている): {} bytes",
            body.len()
        );
        // Both walks must decide "is this a symlink" from the cheap FileType, not from a stat.
        // (`child_meta` takes the FileType from its caller `build_dir`, so only the two that read
        // a DirEntry call `file_type()` themselves.)
        assert!(
            body.contains("is_symlink") || body.contains("file_type()"),
            "{name} が FileType ベースの symlink 判定を持っていない(抽出が壊れている?)"
        );
        for (bad, why) in FORBIDDEN {
            assert!(
                !body.contains(bad),
                "{name} に生の stat 呼び出し `{bad}` がある({why})。\
                 walk の stat は必ず stat_follow 経由にすること\
                 ——さもないと stat 回数ガードが素通りする"
            );
        }
    }
}

// ═════════════════════════════════════════════════════════════════════════════════════════
// Characterization of the UI-stack priority cascade (`surface()` / `internal_mode()`).
//
// Both functions answer the same question — "which layer of the UI stack is in front?" — and
// used to be two hand-written cascades that had to be kept in the same order by hand. They
// drifted once already (the `?` help overlay reached `surface()` but not `internal_mode()`, so
// the chip and footer advertised keys that did nothing).
//
// These tests pin the exact observable behaviour of *every* arm of both functions, plus the
// relative order of every adjacent pair of layers. They are written against the two hand-written
// cascades and must keep passing, unchanged, after they are merged into one — that is what makes
// them evidence that the merge changed no behaviour. Never regenerate the expectations from a
// new implementation.
// ═════════════════════════════════════════════════════════════════════════════════════════

/// One characterized state of the UI stack: `setup` is applied to a freshly built App, and the
/// resulting `(surface(), internal_mode())` pair must equal the recorded one.
struct LayerCase {
    /// `<surface() arm> / <internal_mode() arm>` — reported when an assertion fails.
    name: &'static str,
    /// Applied to a fresh App (Tree mode, nothing open). Must not capture (coerced to `fn`).
    setup: LayerSetup,
    surface: crate::keymap::Surface,
    internal: Option<InternalMode>,
}

/// Puts a fresh App into the state a case describes.
type LayerSetup = fn(&mut App);

fn layer_fixture() -> PathBuf {
    let dir = unique_tmp("konoma_layer_cascade");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let text: String = (0..30).map(|i| format!("line {i}\n")).collect();
    std::fs::write(dir.join("a.txt"), text).unwrap();
    std::fs::write(dir.join("t.csv"), "h1,h2\n1,2\n").unwrap();
    dir
}

fn layer_app(dir: &Path) -> App {
    let mut app = App::new(dir.to_path_buf(), Config::default()).unwrap();
    app.rebuild_tree().unwrap();
    app
}

// ---- setup building blocks -------------------------------------------------------------
/// Windowed text preview (`preview_win` set → `is_windowed()`).
fn lc_open_text(app: &mut App) {
    let p = app.tab.root.join("a.txt");
    app.enter_preview(&p);
}
/// Swap the preview kind to an image *without* going through `enter_preview`, so that whatever
/// `preview_win` / visual anchor the case set up beforehand survives.
fn lc_make_image(app: &mut App) {
    app.image_src = Some(std::sync::Arc::new(image::DynamicImage::new_rgb8(4, 4)));
    app.tab.preview_kind = Some(PreviewKind::Image(PathBuf::from("x.png")));
    app.tab.mode = Mode::Preview;
}
/// Same idea for a parsed CSV table.
fn lc_make_table(app: &mut App) {
    let p = app.tab.root.join("t.csv");
    app.tab.preview_kind = Some(app.cfg.resolve_preview(&p));
    app.tab.preview_path = Some(p);
    app.tab.mode = Mode::Preview;
    app.load_table();
}
fn lc_confirm(app: &mut App, op: PendingOp) {
    app.dialog = Some(Dialog {
        op,
        kind: DialogKind::Confirm {
            message: "message".into(),
            allow_permanent: false,
        },
    });
}
fn lc_input(app: &mut App, op: PendingOp) {
    app.dialog = Some(Dialog {
        op,
        kind: DialogKind::Input {
            title: "title".into(),
            buffer: String::new(),
            cursor: 0,
        },
    });
}

fn layer_cases() -> Vec<LayerCase> {
    use crate::keymap::Surface as S;
    let mut v: Vec<LayerCase> = vec![
        // ── The base full screens (nothing overlaying them) ───────────────────────────
        LayerCase {
            name: "Tree / (none)",
            setup: |_| {},
            surface: S::Tree,
            internal: None,
        },
        LayerCase {
            name: "Tree / ChangedFilter",
            setup: |a| a.tab.changed_filter = true,
            surface: S::Tree,
            internal: Some(InternalMode::ChangedFilter),
        },
        LayerCase {
            name: "PreviewText / (none)",
            setup: lc_open_text,
            surface: S::PreviewText,
            internal: None,
        },
        LayerCase {
            // `changed_filter` only reaches internal_mode in Tree mode.
            name: "PreviewText / (none) — changed_filter is ignored outside Tree mode",
            setup: |a| {
                a.tab.changed_filter = true;
                lc_open_text(a);
            },
            surface: S::PreviewText,
            internal: None,
        },
        LayerCase {
            name: "PreviewTextVisual / PreviewVisual",
            setup: |a| {
                lc_open_text(a);
                a.preview_enter_visual(false);
            },
            surface: S::PreviewTextVisual,
            internal: Some(InternalMode::PreviewVisual),
        },
        LayerCase {
            name: "PreviewImage / (none)",
            setup: lc_make_image,
            surface: S::PreviewImage,
            internal: None,
        },
        LayerCase {
            name: "PreviewTable / (none)",
            setup: lc_make_table,
            surface: S::PreviewTable,
            internal: None,
        },
        // The next three are states the UI cannot actually produce (a visual anchor survives only
        // in a windowed preview). They are here because they pin the *exact* semantics of the two
        // cascades where they disagree: `surface()` resolves the preview kind first and only looks
        // at the visual anchor for text, while `internal_mode()` checks the anchor before it looks
        // at the mode at all. A merged cascade must keep both readings.
        LayerCase {
            name: "PreviewImage / PreviewVisual (artificial: anchor + image kind)",
            setup: |a| {
                lc_open_text(a);
                a.preview_enter_visual(false);
                lc_make_image(a);
            },
            surface: S::PreviewImage,
            internal: Some(InternalMode::PreviewVisual),
        },
        LayerCase {
            name: "PreviewTable / PreviewVisual (artificial: anchor + table kind)",
            setup: |a| {
                lc_open_text(a);
                a.preview_enter_visual(false);
                lc_make_table(a);
            },
            surface: S::PreviewTable,
            internal: Some(InternalMode::PreviewVisual),
        },
        LayerCase {
            name: "Tree / PreviewVisual (artificial: anchor while back in Tree mode)",
            setup: |a| {
                lc_open_text(a);
                a.preview_enter_visual(false);
                a.tab.mode = Mode::Tree;
            },
            surface: S::Tree,
            internal: Some(InternalMode::PreviewVisual),
        },
        // ── Overlays, back to front ───────────────────────────────────────────────────
        LayerCase {
            name: "Visual / Visual",
            setup: |a| a.tab.visual_anchor = Some(0),
            surface: S::Visual,
            internal: Some(InternalMode::Visual),
        },
        LayerCase {
            name: "TableCell / TableCell",
            setup: |a| a.table_cell_open = true,
            surface: S::TableCell,
            internal: Some(InternalMode::TableCell),
        },
        LayerCase {
            name: "Info / Info",
            setup: |a| a.show_info = true,
            surface: S::Info,
            internal: Some(InternalMode::Info),
        },
        LayerCase {
            name: "Outline / Outline",
            setup: |a| a.outline_open = true,
            surface: S::Outline,
            internal: Some(InternalMode::Outline),
        },
        LayerCase {
            name: "Tabs / Tabs",
            setup: |a| a.tab_list = true,
            surface: S::Tabs,
            internal: Some(InternalMode::Tabs),
        },
        LayerCase {
            name: "Bookmarks / Bookmarks",
            setup: |a| a.bookmark_list = true,
            surface: S::Bookmarks,
            internal: Some(InternalMode::Bookmarks),
        },
        LayerCase {
            name: "Mark / Mark",
            setup: |a| a.mark_set_pending = true,
            surface: S::Mark,
            internal: Some(InternalMode::Mark),
        },
        LayerCase {
            name: "Sort / Sort",
            setup: |a| a.sort_menu = true,
            surface: S::Sort,
            internal: Some(InternalMode::Sort),
        },
        LayerCase {
            name: "Search / Search",
            setup: |a| a.tab.search_input = Some(String::new()),
            surface: S::Search,
            internal: Some(InternalMode::Search),
        },
        LayerCase {
            name: "Filter / Filter",
            setup: |a| a.tab.filter_input = Some(String::new()),
            surface: S::Filter,
            internal: Some(InternalMode::Filter),
        },
        LayerCase {
            name: "Help / Help",
            setup: |a| a.show_help = true,
            surface: S::Help,
            internal: Some(InternalMode::Help),
        },
        // ── Dialogs (frontmost) ───────────────────────────────────────────────────────
        LayerCase {
            name: "DialogConfirmQuit / QuitConfirm",
            setup: |a| lc_confirm(a, PendingOp::Quit),
            surface: S::DialogConfirmQuit,
            internal: Some(InternalMode::QuitConfirm),
        },
        LayerCase {
            name: "DialogConfirmBookmark / BookmarkConfirm",
            setup: |a| {
                lc_confirm(
                    a,
                    PendingOp::BookmarkOverwrite {
                        key: 'b',
                        target: PathBuf::from("/tmp/x"),
                    },
                )
            },
            surface: S::DialogConfirmBookmark,
            internal: Some(InternalMode::BookmarkConfirm),
        },
        LayerCase {
            name: "DialogConfirmDrop / DropConfirm",
            setup: |a| {
                lc_confirm(
                    a,
                    PendingOp::DropTransfer {
                        sources: vec![PathBuf::from("/tmp/x")],
                        dir: PathBuf::from("/tmp"),
                    },
                )
            },
            surface: S::DialogConfirmDrop,
            internal: Some(InternalMode::DropConfirm),
        },
        LayerCase {
            name: "DialogConfirmDelete / DeleteConfirm (Delete op)",
            setup: |a| {
                lc_confirm(
                    a,
                    PendingOp::Delete {
                        targets: vec![PathBuf::from("/tmp/x")],
                    },
                )
            },
            surface: S::DialogConfirmDelete,
            internal: Some(InternalMode::DeleteConfirm),
        },
        LayerCase {
            // The confirm catch-all: any op that is not quit/bookmark/drop lands here.
            name: "DialogConfirmDelete / DeleteConfirm (GitDiscard op via the catch-all)",
            setup: |a| {
                lc_confirm(
                    a,
                    PendingOp::GitDiscard {
                        path: PathBuf::from("/tmp/x"),
                    },
                )
            },
            surface: S::DialogConfirmDelete,
            internal: Some(InternalMode::DeleteConfirm),
        },
        LayerCase {
            name: "DialogRenamePreview / RenamePreview",
            setup: |a| {
                a.dialog = Some(Dialog {
                    op: PendingOp::BatchRenameApply { plan: Vec::new() },
                    kind: DialogKind::Preview {
                        title: "t".into(),
                        lines: Vec::new(),
                        scroll: 0,
                    },
                })
            },
            surface: S::DialogRenamePreview,
            internal: Some(InternalMode::RenamePreview),
        },
        LayerCase {
            name: "DialogInput / Create",
            setup: |a| {
                lc_input(
                    a,
                    PendingOp::Create {
                        dir: PathBuf::from("/tmp"),
                    },
                )
            },
            surface: S::DialogInput,
            internal: Some(InternalMode::Create),
        },
        LayerCase {
            name: "DialogInput / BatchRename",
            setup: |a| {
                lc_input(
                    a,
                    PendingOp::BatchRenameInput {
                        targets: Vec::new(),
                    },
                )
            },
            surface: S::DialogInput,
            internal: Some(InternalMode::BatchRename),
        },
        LayerCase {
            name: "DialogInput / Commit",
            setup: |a| lc_input(a, PendingOp::GitCommit),
            surface: S::DialogInput,
            internal: Some(InternalMode::Commit),
        },
        LayerCase {
            name: "DialogInput / GitBranch",
            setup: |a| lc_input(a, PendingOp::GitCreateBranch),
            surface: S::DialogInput,
            internal: Some(InternalMode::GitBranch),
        },
        LayerCase {
            name: "DialogInput / GitWorktrees",
            setup: |a| lc_input(a, PendingOp::WorktreeCreate),
            surface: S::DialogInput,
            internal: Some(InternalMode::GitWorktrees),
        },
        LayerCase {
            // The input catch-all.
            name: "DialogInput / Rename",
            setup: |a| {
                lc_input(
                    a,
                    PendingOp::Rename {
                        target: PathBuf::from("/tmp/x"),
                    },
                )
            },
            surface: S::DialogInput,
            internal: Some(InternalMode::Rename),
        },
        // ── Adjacent-pair ordering (this is what makes a reordering fail) ─────────────
        LayerCase {
            name: "order: a dialog beats help",
            setup: |a| {
                a.show_help = true;
                lc_input(
                    a,
                    PendingOp::Rename {
                        target: PathBuf::from("/tmp/x"),
                    },
                );
            },
            surface: S::DialogInput,
            internal: Some(InternalMode::Rename),
        },
        LayerCase {
            name: "order: help beats the filter input",
            setup: |a| {
                a.show_help = true;
                a.tab.filter_input = Some(String::new());
            },
            surface: S::Help,
            internal: Some(InternalMode::Help),
        },
        LayerCase {
            name: "order: filter beats search",
            setup: |a| {
                a.tab.filter_input = Some(String::new());
                a.tab.search_input = Some(String::new());
            },
            surface: S::Filter,
            internal: Some(InternalMode::Filter),
        },
        LayerCase {
            name: "order: search beats the sort menu",
            setup: |a| {
                a.tab.search_input = Some(String::new());
                a.sort_menu = true;
            },
            surface: S::Search,
            internal: Some(InternalMode::Search),
        },
        LayerCase {
            name: "order: sort beats mark",
            setup: |a| {
                a.sort_menu = true;
                a.mark_set_pending = true;
            },
            surface: S::Sort,
            internal: Some(InternalMode::Sort),
        },
        LayerCase {
            name: "order: mark beats the bookmark list",
            setup: |a| {
                a.mark_set_pending = true;
                a.bookmark_list = true;
            },
            surface: S::Mark,
            internal: Some(InternalMode::Mark),
        },
        LayerCase {
            name: "order: bookmarks beats the tab list",
            setup: |a| {
                a.bookmark_list = true;
                a.tab_list = true;
            },
            surface: S::Bookmarks,
            internal: Some(InternalMode::Bookmarks),
        },
        LayerCase {
            name: "order: tabs beats the outline",
            setup: |a| {
                a.tab_list = true;
                a.outline_open = true;
            },
            surface: S::Tabs,
            internal: Some(InternalMode::Tabs),
        },
        LayerCase {
            name: "order: outline beats info",
            setup: |a| {
                a.outline_open = true;
                a.show_info = true;
            },
            surface: S::Outline,
            internal: Some(InternalMode::Outline),
        },
        LayerCase {
            name: "order: info beats the table-cell popup",
            setup: |a| {
                a.show_info = true;
                a.table_cell_open = true;
            },
            surface: S::Info,
            internal: Some(InternalMode::Info),
        },
        LayerCase {
            name: "order: the table-cell popup beats visual",
            setup: |a| {
                a.table_cell_open = true;
                a.tab.visual_anchor = Some(0);
            },
            surface: S::TableCell,
            internal: Some(InternalMode::TableCell),
        },
        LayerCase {
            name: "order: visual beats the base layer",
            setup: |a| {
                a.tab.visual_anchor = Some(0);
                a.tab.changed_filter = true;
            },
            surface: S::Visual,
            internal: Some(InternalMode::Visual),
        },
        LayerCase {
            name: "order: visual beats the preview visual selection",
            setup: |a| {
                lc_open_text(a);
                a.preview_enter_visual(false);
                a.tab.visual_anchor = Some(0);
            },
            surface: S::Visual,
            internal: Some(InternalMode::Visual),
        },
        LayerCase {
            name: "order: the preview visual selection beats the changed-files filter",
            setup: |a| {
                lc_open_text(a);
                a.preview_enter_visual(false);
                a.tab.mode = Mode::Tree;
                a.tab.changed_filter = true;
            },
            surface: S::Tree,
            internal: Some(InternalMode::PreviewVisual),
        },
    ];
    v.extend(layer_cases_git());
    v
}

/// The git layers. `surface()` gates its whole git block behind `#[cfg(feature = "git")]` while
/// `internal_mode()` does not, so on a no-git build the very same state reports a git *chip* while
/// the keys fall through to whatever is behind it. That asymmetry is characterized here too: it is
/// unreachable through the UI (every opener bails out because the no-git `git.rs` stubs return
/// empty/None), but a merged cascade must still reproduce it exactly.
#[cfg(feature = "git")]
fn layer_cases_git() -> Vec<LayerCase> {
    use crate::keymap::Surface as S;
    vec![
        LayerCase {
            name: "PreviewGitDiff / GitDiff",
            setup: |a| {
                a.tab.preview_kind = Some(PreviewKind::GitDiff(PathBuf::from("/tmp/x")));
                a.tab.mode = Mode::Preview;
            },
            surface: S::PreviewGitDiff,
            internal: Some(InternalMode::GitDiff),
        },
        LayerCase {
            name: "GitChanges / GitChanges",
            setup: |a| a.tab.git_view = true,
            surface: S::GitChanges,
            internal: Some(InternalMode::GitChanges),
        },
        LayerCase {
            name: "GitWorktrees / GitWorktrees",
            setup: |a| a.tab.git_worktrees = Some(Vec::new()),
            surface: S::GitWorktrees,
            internal: Some(InternalMode::GitWorktrees),
        },
        LayerCase {
            name: "WorktreeFilter / GitWorktrees",
            setup: |a| {
                a.tab.git_worktrees = Some(Vec::new());
                a.tab.git_worktree_filtering = true;
            },
            surface: S::WorktreeFilter,
            internal: Some(InternalMode::GitWorktrees),
        },
        LayerCase {
            name: "GitBranches / GitBranch",
            setup: |a| a.tab.git_branches = Some(Vec::new()),
            surface: S::GitBranches,
            internal: Some(InternalMode::GitBranch),
        },
        LayerCase {
            name: "BranchFilter / GitBranch",
            setup: |a| {
                a.tab.git_branches = Some(Vec::new());
                a.tab.git_branch_filtering = true;
            },
            surface: S::BranchFilter,
            internal: Some(InternalMode::GitBranch),
        },
        LayerCase {
            name: "GitGraph / GitGraph",
            setup: |a| a.tab.git_graph = Some(Vec::new()),
            surface: S::GitGraph,
            internal: Some(InternalMode::GitGraph),
        },
        LayerCase {
            name: "GitGraphPicker / GitGraphPicker (panel over the graph)",
            setup: |a| {
                a.tab.git_graph = Some(Vec::new());
                a.git_graph_picker = true;
            },
            surface: S::GitGraphPicker,
            internal: Some(InternalMode::GitGraphPicker),
        },
        LayerCase {
            name: "GitGraphPicker / GitGraphPicker (panel without a graph)",
            setup: |a| a.git_graph_picker = true,
            surface: S::GitGraphPicker,
            internal: Some(InternalMode::GitGraphPicker),
        },
        LayerCase {
            name: "GitLog / GitLog",
            setup: |a| a.tab.git_log = Some(Vec::new()),
            surface: S::GitLog,
            internal: Some(InternalMode::GitLog),
        },
        LayerCase {
            name: "GitDetail / GitDetail",
            setup: |a| a.tab.git_detail = Some(Vec::new()),
            surface: S::GitDetail,
            internal: Some(InternalMode::GitDetail),
        },
        // -- adjacent-pair ordering inside and around the git block --
        LayerCase {
            name: "order: help beats the git detail",
            setup: |a| {
                a.show_help = true;
                a.tab.git_detail = Some(Vec::new());
            },
            surface: S::Help,
            internal: Some(InternalMode::Help),
        },
        LayerCase {
            name: "order: the git detail beats the log",
            setup: |a| {
                a.tab.git_detail = Some(Vec::new());
                a.tab.git_log = Some(Vec::new());
            },
            surface: S::GitDetail,
            internal: Some(InternalMode::GitDetail),
        },
        LayerCase {
            name: "order: the log beats the graph picker",
            setup: |a| {
                a.tab.git_log = Some(Vec::new());
                a.git_graph_picker = true;
            },
            surface: S::GitLog,
            internal: Some(InternalMode::GitLog),
        },
        LayerCase {
            name: "order: the graph picker beats the graph",
            setup: |a| {
                a.git_graph_picker = true;
                a.tab.git_graph = Some(Vec::new());
            },
            surface: S::GitGraphPicker,
            internal: Some(InternalMode::GitGraphPicker),
        },
        LayerCase {
            name: "order: the graph beats the branch list",
            setup: |a| {
                a.tab.git_graph = Some(Vec::new());
                a.tab.git_branches = Some(Vec::new());
            },
            surface: S::GitGraph,
            internal: Some(InternalMode::GitGraph),
        },
        LayerCase {
            name: "order: the branch list beats the worktree list",
            setup: |a| {
                a.tab.git_branches = Some(Vec::new());
                a.tab.git_worktrees = Some(Vec::new());
            },
            surface: S::GitBranches,
            internal: Some(InternalMode::GitBranch),
        },
        LayerCase {
            name: "order: the worktree list beats the changes hub",
            setup: |a| {
                a.tab.git_worktrees = Some(Vec::new());
                a.tab.git_view = true;
            },
            surface: S::GitWorktrees,
            internal: Some(InternalMode::GitWorktrees),
        },
        LayerCase {
            name: "order: the changes hub beats the diff preview",
            setup: |a| {
                a.tab.git_view = true;
                a.tab.preview_kind = Some(PreviewKind::GitDiff(PathBuf::from("/tmp/x")));
                a.tab.mode = Mode::Preview;
            },
            surface: S::GitChanges,
            internal: Some(InternalMode::GitChanges),
        },
        LayerCase {
            name: "order: the diff preview beats the filter input",
            setup: |a| {
                a.tab.preview_kind = Some(PreviewKind::GitDiff(PathBuf::from("/tmp/x")));
                a.tab.mode = Mode::Preview;
                a.tab.filter_input = Some(String::new());
            },
            surface: S::PreviewGitDiff,
            internal: Some(InternalMode::GitDiff),
        },
    ]
}

/// On a no-git build `keymap::Surface` has no git variants, so `surface()` skips its whole git
/// block and the keys land on whatever is behind the git state, while `internal_mode()` still
/// reports the git chip. Characterized so the merge cannot quietly "fix" it.
#[cfg(not(feature = "git"))]
fn layer_cases_git() -> Vec<LayerCase> {
    use crate::keymap::Surface as S;
    vec![
        LayerCase {
            name: "Tree / GitChanges (no-git: the hub has no surface, keys fall through)",
            setup: |a| a.tab.git_view = true,
            surface: S::Tree,
            internal: Some(InternalMode::GitChanges),
        },
        LayerCase {
            name:
                "Filter / GitChanges (no-git: falls through to the layer behind, not to the base)",
            setup: |a| {
                a.tab.git_view = true;
                a.tab.filter_input = Some(String::new());
            },
            surface: S::Filter,
            internal: Some(InternalMode::GitChanges),
        },
        LayerCase {
            name: "Tree / GitDetail (no-git)",
            setup: |a| a.tab.git_detail = Some(Vec::new()),
            surface: S::Tree,
            internal: Some(InternalMode::GitDetail),
        },
        LayerCase {
            name:
                "PreviewText / GitDiff (no-git: a GitDiff kind is not windowed, so it stays text)",
            setup: |a| {
                a.tab.preview_kind = Some(PreviewKind::GitDiff(PathBuf::from("/tmp/x")));
                a.tab.mode = Mode::Preview;
            },
            surface: S::PreviewText,
            internal: Some(InternalMode::GitDiff),
        },
        LayerCase {
            name: "Tree / GitGraphPicker (no-git)",
            setup: |a| a.git_graph_picker = true,
            surface: S::Tree,
            internal: Some(InternalMode::GitGraphPicker),
        },
        LayerCase {
            name: "Tree / GitLog (no-git)",
            setup: |a| a.tab.git_log = Some(Vec::new()),
            surface: S::Tree,
            internal: Some(InternalMode::GitLog),
        },
        LayerCase {
            name: "Tree / GitGraph (no-git)",
            setup: |a| a.tab.git_graph = Some(Vec::new()),
            surface: S::Tree,
            internal: Some(InternalMode::GitGraph),
        },
        LayerCase {
            name: "Tree / GitBranch (no-git: the branch-list arm)",
            setup: |a| a.tab.git_branches = Some(Vec::new()),
            surface: S::Tree,
            internal: Some(InternalMode::GitBranch),
        },
        LayerCase {
            name: "Tree / GitWorktrees (no-git: the worktree-list arm)",
            setup: |a| a.tab.git_worktrees = Some(Vec::new()),
            surface: S::Tree,
            internal: Some(InternalMode::GitWorktrees),
        },
        LayerCase {
            name: "order: the git detail beats the log (no-git: still ordered for the chip)",
            setup: |a| {
                a.tab.git_detail = Some(Vec::new());
                a.tab.git_log = Some(Vec::new());
            },
            surface: S::Tree,
            internal: Some(InternalMode::GitDetail),
        },
    ]
}

/// **The** characterization: every arm of both cascades, and the order of every adjacent pair.
#[test]
fn ui_stack_priority_is_characterized() {
    let dir = layer_fixture();
    let mut failures: Vec<String> = Vec::new();
    for c in layer_cases() {
        let mut app = layer_app(&dir);
        (c.setup)(&mut app);
        let got = (app.surface(), app.internal_mode());
        if got != (c.surface, c.internal) {
            failures.push(format!(
                "{}: expected (surface={:?}, internal={:?}) but got (surface={:?}, internal={:?})",
                c.name, c.surface, c.internal, got.0, got.1
            ));
        }
    }
    std::fs::remove_dir_all(&dir).ok();
    assert!(
        failures.is_empty(),
        "the UI-stack priority changed:\n{}",
        failures.join("\n")
    );
}

/// Coverage proof for the sweep above: every `InternalMode` variant, and every `Surface` variant
/// the cascade can produce, is reached by at least one characterized case.
#[test]
fn ui_stack_characterization_covers_every_arm() {
    let dir = layer_fixture();
    let mut surfaces: Vec<crate::keymap::Surface> = Vec::new();
    let mut internals: Vec<Option<InternalMode>> = Vec::new();
    for c in layer_cases() {
        let mut app = layer_app(&dir);
        (c.setup)(&mut app);
        let s = app.surface();
        let i = app.internal_mode();
        if !surfaces.contains(&s) {
            surfaces.push(s);
        }
        if !internals.contains(&i) {
            internals.push(i);
        }
    }
    std::fs::remove_dir_all(&dir).ok();

    let missing_i: Vec<_> = all_internal_modes()
        .into_iter()
        .filter(|m| !internals.contains(&Some(*m)))
        .collect();
    assert!(
        missing_i.is_empty(),
        "internal_mode() arms never reached by the characterization: {missing_i:?}"
    );
    assert!(
        internals.contains(&None),
        "the 'no overlay' arm of internal_mode() is never reached"
    );
    let missing_s: Vec<_> = all_surfaces()
        .into_iter()
        .filter(|s| !surfaces.contains(s))
        .collect();
    assert!(
        missing_s.is_empty(),
        "surface() arms never reached by the characterization: {missing_s:?}"
    );
}

/// Every `InternalMode` variant. Adding a variant is a compile error in `internal_mode_name`
/// below, which is what forces this list to be extended (and therefore characterized).
fn all_internal_modes() -> Vec<InternalMode> {
    use InternalMode as M;
    let all = vec![
        M::Help,
        M::Visual,
        M::PreviewVisual,
        M::Filter,
        M::ChangedFilter,
        M::Search,
        M::Sort,
        M::Mark,
        M::Bookmarks,
        M::Tabs,
        M::Outline,
        M::Info,
        M::TableCell,
        M::Create,
        M::Rename,
        M::BatchRename,
        M::RenamePreview,
        M::DeleteConfirm,
        M::DropConfirm,
        M::QuitConfirm,
        M::BookmarkConfirm,
        M::GitChanges,
        M::GitDiff,
        M::Commit,
        M::GitLog,
        M::GitDetail,
        M::GitBranch,
        M::GitWorktrees,
        M::GitGraph,
        M::GitGraphPicker,
    ];
    // Exhaustive on purpose: a new variant does not compile until it is named here, and the
    // duplicate/count check below then forces it into the list above.
    let mut names: Vec<&'static str> = all.iter().map(|m| internal_mode_name(*m)).collect();
    names.sort_unstable();
    names.dedup();
    assert_eq!(
        names.len(),
        all.len(),
        "all_internal_modes() has duplicates or is missing a variant"
    );
    all
}

/// Exhaustive on purpose (see `all_internal_modes`).
fn internal_mode_name(m: InternalMode) -> &'static str {
    use InternalMode as M;
    match m {
        M::Help => "Help",
        M::Visual => "Visual",
        M::PreviewVisual => "PreviewVisual",
        M::Filter => "Filter",
        M::ChangedFilter => "ChangedFilter",
        M::Search => "Search",
        M::Sort => "Sort",
        M::Mark => "Mark",
        M::Bookmarks => "Bookmarks",
        M::Tabs => "Tabs",
        M::Outline => "Outline",
        M::Info => "Info",
        M::TableCell => "TableCell",
        M::Create => "Create",
        M::Rename => "Rename",
        M::BatchRename => "BatchRename",
        M::RenamePreview => "RenamePreview",
        M::DeleteConfirm => "DeleteConfirm",
        M::DropConfirm => "DropConfirm",
        M::QuitConfirm => "QuitConfirm",
        M::BookmarkConfirm => "BookmarkConfirm",
        M::GitChanges => "GitChanges",
        M::GitDiff => "GitDiff",
        M::Commit => "Commit",
        M::GitLog => "GitLog",
        M::GitDetail => "GitDetail",
        M::GitBranch => "GitBranch",
        M::GitWorktrees => "GitWorktrees",
        M::GitGraph => "GitGraph",
        M::GitGraphPicker => "GitGraphPicker",
    }
}

/// Every `Surface` variant that exists on this build.
fn all_surfaces() -> Vec<crate::keymap::Surface> {
    use crate::keymap::Surface as S;
    vec![
        S::DialogInput,
        S::Filter,
        S::Search,
        S::Mark,
        #[cfg(feature = "git")]
        S::BranchFilter,
        #[cfg(feature = "git")]
        S::WorktreeFilter,
        S::DialogConfirmDelete,
        S::DialogConfirmDrop,
        S::DialogRenamePreview,
        S::DialogConfirmQuit,
        S::DialogConfirmBookmark,
        S::Help,
        S::Sort,
        S::Bookmarks,
        S::Tabs,
        S::Outline,
        S::Info,
        S::TableCell,
        #[cfg(feature = "git")]
        S::GitDetail,
        #[cfg(feature = "git")]
        S::GitLog,
        #[cfg(feature = "git")]
        S::GitGraph,
        #[cfg(feature = "git")]
        S::GitGraphPicker,
        #[cfg(feature = "git")]
        S::GitBranches,
        #[cfg(feature = "git")]
        S::GitChanges,
        #[cfg(feature = "git")]
        S::GitWorktrees,
        S::Visual,
        S::Tree,
        S::PreviewText,
        S::PreviewTextVisual,
        S::PreviewImage,
        S::PreviewTable,
        #[cfg(feature = "git")]
        S::PreviewGitDiff,
    ]
}

/// The runtime half of the compile-time guard: every `Surface` names the layer it belongs to
/// (`Surface::layer`, an exhaustive match — a new variant does not compile until it is placed),
/// and that layer really does project back to it. Without this, a new surface could be "placed"
/// anywhere and nothing would notice.
#[test]
fn surface_round_trips_through_its_layer() {
    for s in all_surfaces() {
        assert_eq!(
            s.layer().surface(),
            Some(s),
            "{s:?} does not come back from the layer it claims to belong to"
        );
    }
}

/// The general form of the `?` help bug: whatever is in front decides **both** what the chip and
/// footer say and which keys work. Checked over every characterized state — the chip must be the
/// frontmost layer's mode, and the keymap must be that same layer's surface whenever it has one.
#[test]
fn the_chip_and_the_keys_come_from_the_same_layer() {
    let dir = layer_fixture();
    let mut failures: Vec<String> = Vec::new();
    for c in layer_cases() {
        let mut app = layer_app(&dir);
        (c.setup)(&mut app);
        let front = app.frontmost_layer(|_| true);
        if app.internal_mode() != front.internal_mode() {
            failures.push(format!("{}: the chip does not follow {front:?}", c.name));
        }
        // The one documented exception is a git layer on a no-git build, which owns no surface at
        // all; then — and only then — the keys legitimately belong to a layer further back.
        match front.surface() {
            Some(s) if app.surface() != s => {
                failures.push(format!("{}: the keys do not follow {front:?}", c.name))
            }
            _ => {}
        }
    }
    std::fs::remove_dir_all(&dir).ok();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// The `?` help overlay is a centered popup that leaves the chip and footer visible, so it must win
/// from *every* screen behind it — that is the shape the two cascades drifted on once before.
#[test]
fn help_wins_over_whatever_screen_is_behind_it() {
    let dir = layer_fixture();
    let behind: Vec<(&str, LayerSetup)> = vec![
        ("tree", |_| {}),
        ("text preview", lc_open_text),
        ("image preview", lc_make_image),
        ("table preview", lc_make_table),
        ("tree visual", |a| a.tab.visual_anchor = Some(0)),
        ("filter input", |a| a.tab.filter_input = Some(String::new())),
        ("tab list", |a| a.tab_list = true),
        #[cfg(feature = "git")]
        ("git changes hub", |a| a.tab.git_view = true),
        #[cfg(feature = "git")]
        ("git commit detail", |a| a.tab.git_detail = Some(Vec::new())),
        #[cfg(feature = "git")]
        ("git log", |a| a.tab.git_log = Some(Vec::new())),
        #[cfg(feature = "git")]
        ("git graph picker", |a| {
            a.tab.git_graph = Some(Vec::new());
            a.git_graph_picker = true;
        }),
    ];
    for (name, setup) in behind {
        let mut app = layer_app(&dir);
        setup(&mut app);
        app.show_help = true;
        assert_eq!(
            app.surface(),
            crate::keymap::Surface::Help,
            "help over {name}: the keys must be help's"
        );
        assert_eq!(
            app.internal_mode(),
            Some(InternalMode::Help),
            "help over {name}: the chip/footer must be help's"
        );
    }
    std::fs::remove_dir_all(&dir).ok();
}

/// The branch/worktree list `/` filter is deliberately **not** symmetric across the three consumers,
/// and this pins why (see `UiLayer::internal_mode`): the keymap needs its own surface because typing
/// must be captured instead of dispatched, the chip deliberately stays on the list (the list is
/// still what you are looking at, cursor and all), and the footer swaps to the `/query` prompt
/// through its own branch in `footer_spans`, not through the mode.
#[cfg(feature = "git")]
#[test]
fn a_list_filter_switches_the_keymap_and_the_footer_but_not_the_chip() {
    let dir = layer_fixture();
    for (name, list, filtering, surface, mode) in [
        (
            "branches",
            (|a: &mut App| a.tab.git_branches = Some(Vec::new())) as fn(&mut App),
            (|a: &mut App| a.tab.git_branch_filtering = true) as fn(&mut App),
            crate::keymap::Surface::BranchFilter,
            InternalMode::GitBranch,
        ),
        (
            "worktrees",
            (|a: &mut App| a.tab.git_worktrees = Some(Vec::new())) as fn(&mut App),
            (|a: &mut App| a.tab.git_worktree_filtering = true) as fn(&mut App),
            crate::keymap::Surface::WorktreeFilter,
            InternalMode::GitWorktrees,
        ),
    ] {
        let mut app = layer_app(&dir);
        list(&mut app);
        let footer_before: String = crate::ui::status::footer_spans(&app, 80)
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        assert_eq!(app.internal_mode(), Some(mode), "{name}: chip before");

        filtering(&mut app);
        assert_eq!(
            app.surface(),
            surface,
            "{name}: the keymap must switch to the text-input surface"
        );
        assert!(
            app.surface().is_text_input(),
            "{name}: characters must be captured, not dispatched"
        );
        assert_eq!(
            app.internal_mode(),
            Some(mode),
            "{name}: the chip must stay on the list — the filter is an input inside it, not a mode"
        );
        let footer_after: String = crate::ui::status::footer_spans(&app, 80)
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        assert!(
            footer_after.starts_with('/') && footer_after != footer_before,
            "{name}: the footer must show the /query prompt: {footer_after:?}"
        );
    }
    std::fs::remove_dir_all(&dir).ok();
}

/// The priority order is written down once — as the order of `UiLayer`'s variants, which
/// `App::frontmost_layer` walks. This checks the two really do agree, so the enum can be read as
/// the order: turn on everything that can be on at once, then repeatedly ask for the frontmost
/// layer while hiding the ones already seen, and require the sequence to climb the declaration
/// order (`Ord` follows it) without ever going backwards.
///
/// The dialog layers are left out on purpose: a dialog has exactly one kind and one pending
/// operation, so no two of them can be active together and their relative order is not observable
/// (`ui_stack_priority_is_characterized` pins each of them individually, and that a dialog beats
/// help).
#[test]
fn the_walk_visits_the_layers_in_their_declaration_order() {
    let dir = layer_fixture();
    let mut app = layer_app(&dir);
    app.show_help = true;
    #[cfg(feature = "git")]
    {
        app.tab.git_detail = Some(Vec::new());
        app.tab.git_log = Some(Vec::new());
        app.git_graph_picker = true;
        app.tab.git_graph = Some(Vec::new());
        app.tab.git_branches = Some(Vec::new());
        app.tab.git_branch_filtering = true;
        app.tab.git_worktrees = Some(Vec::new());
        app.tab.git_worktree_filtering = true;
        app.tab.git_view = true;
    }
    app.tab.filter_input = Some(String::new());
    app.tab.search_input = Some(String::new());
    app.sort_menu = true;
    app.mark_set_pending = true;
    app.bookmark_list = true;
    app.tab_list = true;
    app.outline_open = true;
    app.show_info = true;
    app.table_cell_open = true;
    app.tab.visual_anchor = Some(0);

    let mut seen: Vec<UiLayer> = Vec::new();
    loop {
        let next = app.frontmost_layer(|l| !seen.contains(&l));
        seen.push(next);
        if matches!(next, UiLayer::Base(_)) {
            break;
        }
        assert!(seen.len() < 64, "the walk does not terminate: {seen:?}");
    }
    for w in seen.windows(2) {
        assert!(
            w[0] < w[1],
            "the walk goes backwards through the declaration order: {:?} came before {:?}\nfull order: {seen:?}",
            w[0],
            w[1]
        );
    }
    // Sanity: it really did walk a stack, not just fall straight through to the base.
    assert!(
        seen.len() >= 12,
        "expected the walk to pass through many layers, got {seen:?}"
    );
    std::fs::remove_dir_all(&dir).ok();
}

// =============================================================================================
// The `/` filter's population scan: budgeted synchronous start + background hand-off.
//
// Why this exists at all: 50,000 entries cost ~45ms in `readdir` alone on this machine, so no
// implementation can walk a large tree inside the <60ms draw budget. The scan therefore starts
// synchronously with a deadline and hands the remainder to a worker. Two code paths follow from
// that, and **both have to be exercised deliberately** — a test fixture is far too small to ever
// exhaust the real budget, so without forcing it the hand-off would never run in the suite at all
// (the same shape of hole that let a broken image path ship once: "the test never went down that
// branch"). `set_filter_scan_budget` is that seam.
//
// Nothing here asserts on wall-clock time. This repo replaced every timing bound with
// count/allocation/structure guards because a shared CI runner's clock is not reliable; the
// properties that actually matter (does it finish inline? does the split lose entries? does one
// scan run at a time?) are all exactly measurable.

/// Fixture: a nested tree — several directories, each with files — so a walk has more than one
/// directory to pop and can therefore be split at a boundary.
fn nested_fixture(prefix: &str, dirs: usize, files_per_dir: usize) -> PathBuf {
    let dir = unique_tmp(prefix);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for d in 0..dirs {
        let sub = dir.join(format!("d{d:02}"));
        std::fs::create_dir_all(&sub).unwrap();
        for f in 0..files_per_dir {
            std::fs::write(sub.join(format!("f{f:02}.txt")), b"x").unwrap();
        }
    }
    dir
}

/// Set the scan budget for this thread and restore it when dropped, so one test forcing a budget
/// can never leak into another that runs on the same thread afterwards.
struct BudgetGuard;
impl BudgetGuard {
    fn always_async() -> Self {
        crate::app::set_filter_scan_budget(Some(Some(std::time::Duration::ZERO)));
        Self
    }
    fn always_sync() -> Self {
        crate::app::set_filter_scan_budget(Some(None));
        Self
    }
}
impl Drop for BudgetGuard {
    fn drop(&mut self) {
        crate::app::set_filter_scan_budget(None);
    }
}

#[test]
fn collect_scan_split_at_every_boundary_matches_one_uninterrupted_walk() {
    // The correctness question the whole design rests on: does splitting a walk lose anything?
    // An exhausted deadline stops the walk after exactly one directory (the "always make progress"
    // rule), so stepping with `Some(Instant::now())` walks the tree one directory per call and
    // splits it at *every* boundary in turn — a deterministic stand-in for "the budget ran out
    // here", with no clock sensitivity at all.
    let dir = nested_fixture("konoma_scan_split", 6, 4);
    let whole = collect_all(&dir, false);
    assert_eq!(whole.len(), 6 + 6 * 4, "土台: 全部で 30 エントリ");

    let mut pieces: Vec<Entry> = Vec::new();
    let mut stack = vec![dir.clone()];
    let mut calls = 0;
    while !stack.is_empty() {
        let (part, rest) = collect_scan(
            std::mem::take(&mut stack),
            false,
            COLLECT_CAP - pieces.len(),
            Some(std::time::Instant::now()), // already elapsed = stop after one directory
        );
        assert!(
            !part.is_empty() || calls > 0,
            "予算切れでも 1 ディレクトリは必ず読む(前進しない受け渡しは無限ループになる)"
        );
        pieces.extend(part);
        stack = rest;
        calls += 1;
        assert!(calls < 100, "分割が前進していない(無限ループ)");
    }
    assert!(
        calls >= 7,
        "土台: ディレクトリごとに分割されている(実際 {calls} 回)"
    );
    pieces.sort_by(|a, b| a.path.cmp(&b.path));
    let paths: Vec<_> = pieces.iter().map(|e| e.path.clone()).collect();
    let whole_paths: Vec<_> = whole.iter().map(|e| e.path.clone()).collect();
    assert_eq!(
        paths, whole_paths,
        "どこで分割しても、繋ぎ直した結果は一気に歩いた結果と完全に一致すること"
    );
    for (a, b) in pieces.iter().zip(whole.iter()) {
        assert_eq!(a.is_dir, b.is_dir, "is_dir も一致する: {:?}", a.path);
    }
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn collect_scan_without_a_deadline_finishes_in_one_call() {
    // The worker's own contract: handed a stack and no deadline, it must run the walk to the end
    // (an empty leftover stack) — otherwise the hand-off would drop entries on the floor.
    let dir = nested_fixture("konoma_scan_nodeadline", 4, 3);
    let (out, rest) = collect_scan(vec![dir.clone()], false, COLLECT_CAP, None);
    assert!(rest.is_empty(), "期限なしなら歩き残しは無い");
    assert_eq!(out.len(), 4 + 4 * 3);
    assert!(
        out.windows(2).all(|w| w[0].path <= w[1].path),
        "各回の返り値はソート済み(呼び出し側の sort_by がラン結合で済む前提)"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn collect_scan_treats_the_cap_as_complete_not_as_an_interruption() {
    // Truncation by the cost limit must come back with an **empty** leftover stack: it is a
    // deliberate stop, and reporting it as "unfinished" would have the worker walk the rest of a
    // huge tree only to have it thrown away past the cap.
    let dir = nested_fixture("konoma_scan_cap", 5, 5);
    let (out, rest) = collect_scan(vec![dir.clone()], false, 7, None);
    assert_eq!(out.len(), 7, "cap ちょうどで打ち切る");
    assert!(rest.is_empty(), "cap 到達は「完了」= 続きを渡さない");
    std::fs::remove_dir_all(&dir).ok();
}

/// Attach a filter-pool channel and return the receiving end (the run loop's role, in a test).
fn with_pool_channel(app: &mut App) -> std::sync::mpsc::Receiver<crate::app::FilterPoolResult> {
    let (tx, rx) = std::sync::mpsc::channel();
    app.attach_filter_pool_loader(tx);
    rx
}

#[test]
fn start_filter_within_budget_fills_the_pool_before_returning() {
    // Condition 1 of the design: on a tree small enough to finish inside the budget — that is,
    // nearly every directory anyone opens — pressing `/` must behave *exactly* as it did before
    // this was made interruptible. The pool is complete when `start_filter` returns, so the very
    // first frame after `/` already has everything; nothing arrives later.
    let _g = BudgetGuard::always_sync();
    let dir = nested_fixture("konoma_pool_sync", 4, 3);
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    let rx = with_pool_channel(&mut app);

    app.start_filter();

    assert_eq!(
        app.tab.filter_pool.len(),
        4 + 4 * 3,
        "`/` から戻った時点でプールは全件揃っている"
    );
    assert!(
        app.filter_pool_pending.is_none(),
        "同期で終わったので裏の走査は残らない"
    );
    assert!(
        rx.try_recv().is_err(),
        "ワーカーに投げていない(=一瞬でも空リストが出ることはない)"
    );
    assert!(
        !app.busy_jobs().contains(&crate::i18n::Msg::BusyFilterScan),
        "走査中インジケータも出ない"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn the_shipped_budget_is_a_real_one() {
    // The test above forces the budget, so on its own it would still pass if production shipped a
    // zero budget — i.e. if every `/` press, on every tree, went through the hand-off and showed a
    // momentarily empty list. Structural rather than timed: assert the shipped constant is a
    // meaningful slice of the 60ms draw budget, and that nothing overrides it by default.
    assert!(
        crate::app::COLLECT_BUDGET >= std::time::Duration::from_millis(5),
        "同期で歩く予算が実質ゼロ = 小さなツリーでも受け渡しになる(条件1の破壊)"
    );
    assert!(
        crate::app::COLLECT_BUDGET < std::time::Duration::from_millis(60),
        "予算が描画バジェット(60ms)以上 = 予算内に収まっても遅延の原因になる"
    );
    assert_eq!(
        crate::app::filter_scan_budget(),
        Some(crate::app::COLLECT_BUDGET),
        "上書きなしなら本番の予算がそのまま使われる"
    );
}

#[test]
fn collect_scan_carries_the_cap_across_a_split() {
    // The cap is a whole-scan limit, so a split walk has to hand over how much of it is left. If
    // the second half were given the full cap again, a large tree would collect past the cost
    // limit the cap exists to enforce.
    let dir = nested_fixture("konoma_scan_cap_split", 6, 4);
    const CAP: usize = 11;
    let mut got: Vec<Entry> = Vec::new();
    let mut stack = vec![dir.clone()];
    while !stack.is_empty() && got.len() < CAP {
        let (part, rest) = collect_scan(
            std::mem::take(&mut stack),
            false,
            CAP - got.len(),
            Some(std::time::Instant::now()),
        );
        got.extend(part);
        stack = rest;
    }
    assert_eq!(
        got.len(),
        CAP,
        "分割しても打ち切りは cap ちょうど(残量を引き継いでいる)"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn start_filter_over_budget_hands_off_and_the_pool_completes_on_arrival() {
    // Condition 2: when the budget runs out, `/` still returns (with a partial pool and a
    // background scan running), and the result that arrives later completes the pool to **exactly**
    // what an uninterrupted walk would have produced.
    let _g = BudgetGuard::always_async();
    let dir = nested_fixture("konoma_pool_async", 6, 4);
    let full = collect_all(&dir, false);
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    let rx = with_pool_channel(&mut app);

    app.start_filter();

    let partial = app.tab.filter_pool.len();
    assert!(
        partial < full.len(),
        "予算切れなので `/` の時点では部分的: {partial} / {}",
        full.len()
    );
    assert_eq!(
        app.filter_pool_pending.as_ref().map(|(_, p)| p.as_path()),
        Some(dir.as_path()),
        "残りは裏で走っている"
    );
    assert!(
        app.busy_jobs().contains(&crate::i18n::Msg::BusyFilterScan),
        "裏で走っていることがユーザーに見える"
    );

    // The user keeps typing while the scan runs — the partial pool must filter without breaking.
    app.filter_input_push('f');
    app.filter_input_push('0');
    let mid = app.tab.entries.len();
    assert!(
        app.tab.entries.iter().all(|e| e.path.starts_with(&dir)),
        "部分プールでも絞り込みは破綻しない"
    );

    let res = rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
    assert!(app.apply_filter_pool(res), "到着した結果は適用される");

    let got: Vec<_> = app.tab.filter_pool.iter().map(|e| &e.path).collect();
    let want: Vec<_> = full.iter().map(|e| &e.path).collect();
    assert_eq!(got, want, "最終的なプールは一気に歩いた結果と完全に一致");
    assert!(
        app.filter_pool_pending.is_none(),
        "完了したので走査中フラグは解ける"
    );
    assert!(!app.busy_jobs().contains(&crate::i18n::Msg::BusyFilterScan));
    // Re-applied with the query the user has typed *by now*, not the empty one `/` started with.
    assert!(
        app.tab.entries.len() > mid,
        "到着後は現在の入力(f0)で再適用される: {mid} → {}",
        app.tab.entries.len()
    );
    assert!(
        app.tab
            .entries
            .iter()
            .all(|e| e.path.to_string_lossy().contains("f0")),
        "再適用は現在の絞り込み条件で行われる(空クエリに戻したりしない)"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_scan_that_lands_after_the_root_changed_is_discarded() {
    // Staleness by root: the walk was collecting somewhere else, and dropping it into the current
    // pool would show another directory's files under this one's filter.
    //
    // Driven directly rather than through a key sequence, and deliberately so: every path that
    // moves the root today *also* bumps the generation (they all either clear the filter or kick a
    // fresh scan), so a scenario test would be satisfied by the generation check alone and prove
    // nothing about this guard — a test with no detection power, which is worse than no test. What
    // is asserted here is the guard's own contract: **generation matching is not sufficient; the
    // result must also belong to the root that is on screen.**
    let _g = BudgetGuard::always_async();
    let a = nested_fixture("konoma_pool_stale_a", 3, 3);
    let b = nested_fixture("konoma_pool_stale_b", 3, 3);
    let mut app = App::new(a.clone(), Config::default()).unwrap();
    let rx = with_pool_channel(&mut app);
    app.start_filter();
    let res = rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
    assert_eq!(res.gen, app.filter_pool_gen, "土台: 世代はまだ一致している");
    assert_eq!(res.root, a, "土台: この結果は a の走査");

    // The root moves under the running scan, with the filter still active.
    app.tab.root = b.clone();
    app.tab.filter_pool.clear();

    assert!(
        !app.apply_filter_pool(res),
        "root が変わった結果は適用しない"
    );
    assert!(
        app.tab.filter_pool.is_empty(),
        "前の root のパスがプールに流れ込まない"
    );

    // And the realistic sequence (move away, filter there) rejects it too — belt and braces.
    let mut app = App::new(a.clone(), Config::default()).unwrap();
    let rx = with_pool_channel(&mut app);
    app.start_filter();
    let old = rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
    app.jump_to_dir(b.clone());
    app.start_filter();
    let _ = rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
    let before: Vec<_> = app.tab.filter_pool.iter().map(|e| e.path.clone()).collect();
    assert!(!app.apply_filter_pool(old));
    let after: Vec<_> = app.tab.filter_pool.iter().map(|e| e.path.clone()).collect();
    assert_eq!(before, after, "プールは汚染されない");
    assert!(
        !after.iter().any(|p| p.starts_with(&a)),
        "前の root のパスが混ざらない"
    );
    std::fs::remove_dir_all(&a).ok();
    std::fs::remove_dir_all(&b).ok();
}

#[test]
fn a_scan_that_lands_after_leaving_the_filter_is_discarded() {
    // Staleness by mode: `Esc` cleared the filter, so there is no pool for the result to belong to.
    let _g = BudgetGuard::always_async();
    let dir = nested_fixture("konoma_pool_left", 3, 3);
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    let rx = with_pool_channel(&mut app);
    app.start_filter();
    let res = rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();

    app.filter_clear();
    assert!(
        !app.apply_filter_pool(res),
        "絞り込みを抜けた後の結果は適用しない"
    );
    assert!(
        app.tab.filter_pool.is_empty(),
        "捨てたプールが復活してはいけない"
    );
    assert!(app.tab.tree_filter.is_none());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_scan_that_lands_after_a_tab_switch_is_discarded() {
    // Staleness by tab: `filter_pool` is per-tab but the scan's bookkeeping is on `App`, so without
    // invalidating on switch one tab's walk lands in whichever tab happens to be active when it
    // finishes.
    //
    // Neither the generation nor the root guard covers this on its own, which is why the scenario
    // is built the way it is: both tabs filter the **same root** (so the root check passes) and the
    // switch hits the coalescing branch rather than dispatching a new scan (so the generation would
    // still match). What distinguishes them is `show_hidden` — tab 2 shows dotfiles, tab 1 does
    // not — so tab 1's result landing in tab 2 is directly visible as tab 2's dotfiles vanishing.
    let dir = nested_fixture("konoma_pool_tab", 3, 3);
    std::fs::write(dir.join(".dotfile.txt"), b"x").unwrap();
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    let rx = with_pool_channel(&mut app);
    {
        let _g = BudgetGuard::always_sync();
        app.start_filter(); // tab 1: filtering, hidden excluded
        app.tab_new().unwrap(); // tab 2 (same root; tab_new clears the filter)
        app.tab.show_hidden = true;
        app.start_filter(); // tab 2: filtering, dotfiles included
    }
    assert!(
        app.tab
            .filter_pool
            .iter()
            .any(|e| e.path.ends_with(".dotfile.txt")),
        "土台: タブ2のプールにはドットファイルが入っている"
    );

    // Back to tab 1: the switch re-collects (async, hidden excluded) for tab 1's root.
    app.tab_goto(0);
    let tab1_scan = rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
    assert!(
        !tab1_scan
            .entries
            .as_ref()
            .unwrap()
            .iter()
            .any(|e| e.path.ends_with(".dotfile.txt")),
        "土台: これはタブ1(隠しファイル非表示)の走査結果"
    );

    // ...and away again before it lands.
    app.tab_goto(1);
    assert!(
        !app.apply_filter_pool(tab1_scan),
        "タブを切り替えた後に届いた結果は適用しない"
    );
    assert!(
        app.tab
            .filter_pool
            .iter()
            .any(|e| e.path.ends_with(".dotfile.txt")),
        "タブ1の走査結果がタブ2のプールを上書きしていない"
    );
    while rx.try_recv().is_ok() {} // drain whatever the switches themselves kicked off
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn concurrent_fs_events_coalesce_into_a_single_rescan() {
    // The failure this prevents: an agent writing files fires an FS event per write, and each one
    // used to trigger a synchronous whole-tree walk. Going async removes the natural rate-limit
    // that a synchronous walk's own duration provided, so without coalescing the scans would pile
    // up — worse the larger the tree, which is precisely the case this change exists for.
    //
    // Counted by how many results reach the channel, which is exactly "how many scans ran".
    let _g = BudgetGuard::always_async();
    let dir = nested_fixture("konoma_pool_coalesce", 4, 3);
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    let rx = with_pool_channel(&mut app);
    app.start_filter();
    let first = rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
    assert!(app.apply_filter_pool(first));

    // Ten FS bursts arrive back to back while the first re-scan is still running.
    for _ in 0..10 {
        app.refresh_fs_changed(false, &[]).unwrap();
    }
    assert!(app.filter_pool_dirty, "走行中の要求は合体して保留される");

    let a = rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
    assert!(app.apply_filter_pool(a), "1本目が着地");
    // Exactly one more (the coalesced request), then nothing.
    let b = rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
    assert!(app.apply_filter_pool(b), "合体した要求が1回だけ実行される");
    assert!(
        rx.recv_timeout(std::time::Duration::from_millis(300))
            .is_err(),
        "10 個のイベントから走ったのは合計 2 本(実行中の1本 + 合体した1本)だけ"
    );
    assert!(!app.filter_pool_dirty, "保留は解消済み");
    assert!(app.filter_pool_pending.is_none());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn an_fs_refresh_keeps_showing_the_current_pool_until_the_new_one_lands() {
    // Nobody is waiting on the FS-driven re-scan, so it has no synchronous part at all — but the
    // list must not blink empty while it runs. `rebuild_tree` resets `entries` to the whole tree on
    // every refresh, so the filter is re-applied immediately from the pool already in hand.
    let _g = BudgetGuard::always_async();
    let dir = nested_fixture("konoma_pool_keep", 4, 3);
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    let rx = with_pool_channel(&mut app);
    app.start_filter();
    let res = rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
    assert!(app.apply_filter_pool(res));
    app.filter_input_push('f');
    let pool_before = app.tab.filter_pool.len();
    let shown_before = app.tab.entries.len();
    assert!(shown_before > 0, "土台: 何か表示されている");

    app.refresh_fs_changed(false, &[]).unwrap();

    assert_eq!(
        app.tab.filter_pool.len(),
        pool_before,
        "走査中も今のプールを持ち続ける"
    );
    assert_eq!(
        app.tab.entries.len(),
        shown_before,
        "fs 更新の瞬間にリストが空にならない(rebuild_tree の全件表示にも戻らない)"
    );
    let res = rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
    assert!(app.apply_filter_pool(res));
    assert_eq!(app.tab.filter_pool.len(), pool_before, "件数は変わらない");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn an_fs_refresh_picks_up_files_created_while_filtering() {
    // The point of re-scanning at all: konoma's headline use is watching an agent write files, so
    // a file that appears while `/` is open has to show up in the results.
    let _g = BudgetGuard::always_async();
    let dir = nested_fixture("konoma_pool_newfile", 3, 2);
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    let rx = with_pool_channel(&mut app);
    app.start_filter();
    let res = rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
    assert!(app.apply_filter_pool(res));
    app.filter_input_push('z');
    app.filter_input_push('z');
    assert!(app.tab.entries.is_empty(), "土台: まだ一致は無い");

    std::fs::write(dir.join("d00").join("zzz_agent.txt"), b"x").unwrap();
    app.refresh_fs_changed(false, &[]).unwrap();
    let res = rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
    assert!(app.apply_filter_pool(res));

    assert!(
        app.tab
            .entries
            .iter()
            .any(|e| e.path.ends_with("zzz_agent.txt")),
        "走査中に増えたファイルが、今の入力のまま結果に現れる"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn the_async_and_synchronous_paths_produce_the_same_pool() {
    // The single most important equivalence: whichever path a tree happens to take, the user must
    // end up with the same set of files. (Also the guard that would catch a cap accounted for twice
    // across the hand-off, or a leftover stack handed over wrong.)
    let dir = nested_fixture("konoma_pool_equal", 5, 4);

    let sync_pool = {
        let _g = BudgetGuard::always_sync();
        let mut app = App::new(dir.clone(), Config::default()).unwrap();
        app.start_filter();
        app.tab
            .filter_pool
            .iter()
            .map(|e| e.path.clone())
            .collect::<Vec<_>>()
    };
    let async_pool = {
        let _g = BudgetGuard::always_async();
        let mut app = App::new(dir.clone(), Config::default()).unwrap();
        let rx = with_pool_channel(&mut app);
        app.start_filter();
        let res = rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
        assert!(app.apply_filter_pool(res));
        app.tab
            .filter_pool
            .iter()
            .map(|e| e.path.clone())
            .collect::<Vec<_>>()
    };

    assert_eq!(
        sync_pool, async_pool,
        "同期経路と非同期経路の結果が一致する"
    );
    assert!(
        async_pool.windows(2).all(|w| w[0] <= w[1]),
        "連結後もソート順は保たれる(fuzzy_filter_pool は順序を前提にしている)"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn without_a_channel_every_scan_still_completes_synchronously() {
    // The compatibility contract that lets the whole existing suite stay untouched: with no Sender
    // attached, even a scan that blows the budget is finished on the spot, so a test that never
    // drives a run loop sees the same complete pool it always did.
    let _g = BudgetGuard::always_async();
    let dir = nested_fixture("konoma_pool_nochannel", 4, 3);
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    // deliberately no attach_filter_pool_loader

    app.start_filter();

    assert_eq!(
        app.tab.filter_pool.len(),
        4 + 4 * 3,
        "チャネル未接続なら同期で完走する"
    );
    assert!(app.filter_pool_pending.is_none());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_panicking_scan_releases_the_slot_without_wiping_the_pool() {
    // A worker that panics must not look like "the directory is empty": for the FS refresh the
    // result *replaces* the pool, so flattening a crash into an empty Vec would leave `/` matching
    // nothing at all. It must also not latch `filter_pool_pending`, which would spin the busy
    // indicator forever and swallow the coalesced re-scan request.
    let _g = BudgetGuard::always_sync();
    let dir = nested_fixture("konoma_pool_panic", 3, 3);
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.start_filter();
    app.filter_input_push('f');
    let pool = app.tab.filter_pool.len();
    let shown = app.tab.entries.len();
    assert!(pool > 0 && shown > 0, "土台: プールも表示も埋まっている");

    // What the worker sends when `catch_silent` caught a panic.
    app.filter_pool_pending = Some((app.filter_pool_gen, dir.clone()));
    let failed = crate::app::FilterPoolResult {
        gen: app.filter_pool_gen,
        root: dir.clone(),
        entries: None,
        append: false,
    };
    assert!(!app.apply_filter_pool(failed), "失敗結果は「変化なし」");

    assert_eq!(app.tab.filter_pool.len(), pool, "プールは維持される");
    assert_eq!(app.tab.entries.len(), shown, "表示も維持される");
    assert!(
        app.filter_pool_pending.is_none(),
        "走査中フラグは解ける(スピナーが回りっぱなしにならない)"
    );
    assert!(!app.busy_jobs().contains(&crate::i18n::Msg::BusyFilterScan));
    std::fs::remove_dir_all(&dir).ok();
}

// ---------------------------------------------------------------------------------------------
// A: the filter cursor must follow the *file*, not the index.
//
// `reapply_filter` rebuilds `entries` from the pool, and the default `filter_mode = "fuzzy"` orders
// by score — so anything that grows the pool (the budgeted hand-off from `/` landing, an FS-driven
// re-scan) reshuffles the list under a cursor that was only ever range-clamped. The index stayed in
// range and pointed at a different file, silently retargeting the next `Enter` / `y` / `Space→d`.
// ---------------------------------------------------------------------------------------------

/// Fixture built so the fuzzy ranking is *guaranteed* to reshuffle when the pool grows: `abcd.txt`
/// is a far better match for "abc" than `x_a_b_c_N.txt`, so once it arrives it sorts to the front
/// and pushes everything already on screen down by one.
fn reorder_fixture(prefix: &str) -> PathBuf {
    let dir = unique_tmp(prefix);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for i in 0..3 {
        std::fs::write(dir.join(format!("x_a_b_c_{i}.txt")), b"x").unwrap();
    }
    dir
}

/// The result the worker sends when the handed-off tail of a `/` walk finishes.
fn tail_result(app: &App, root: &Path, entries: Vec<Entry>) -> crate::app::FilterPoolResult {
    crate::app::FilterPoolResult {
        gen: app.filter_pool_gen,
        root: root.to_path_buf(),
        entries: Some(entries),
        append: true,
    }
}

#[test]
fn filter_cursor_follows_its_file_when_a_late_scan_reorders_the_results() {
    // The window is real: `/` on a big tree hands the tail to a worker, and the user is *expected*
    // to be picking a target out of the partial list while it finishes. If the arrival renumbers
    // the list under the cursor, the next key acts on a file the user never looked at.
    let _g = BudgetGuard::always_sync();
    let dir = reorder_fixture("konoma_filter_reorder");
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.start_filter();
    for c in "abc".chars() {
        app.filter_input_push(c);
    }
    // Park the cursor on a specific file (not the first — so a plain "reset to 0" would also fail).
    app.tab.selected = 1;
    let target = app.tab.entries[1].path.clone();
    assert!(target.ends_with("x_a_b_c_1.txt"), "土台: {target:?}");

    // The handed-off tail lands, carrying a much better match for the same query.
    std::fs::write(dir.join("abcd.txt"), b"x").unwrap();
    let late = collect_all(&dir, false)
        .into_iter()
        .filter(|e| e.path.ends_with("abcd.txt"))
        .collect::<Vec<_>>();
    assert_eq!(late.len(), 1, "土台: 到着するのは 1 件");
    let res = tail_result(&app, &dir, late);
    assert!(app.apply_filter_pool(res), "到着した結果は適用される");

    let moved = app
        .tab
        .entries
        .iter()
        .position(|e| e.path == target)
        .expect("対象は今も一致する");
    assert_ne!(
        moved, 1,
        "土台: 到着で順位が入れ替わっていないとこのテストは何も検出しない"
    );
    assert_eq!(
        app.tab.entries[app.tab.selected].path, target,
        "カーソルは同じファイルを指し続ける(index ではなくファイルを追う)"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn filter_cursor_falls_back_to_clamping_when_its_file_is_no_longer_a_match() {
    // The other half of the contract: "follow the file" must not become "refuse to move". When the
    // selected entry is gone (deleted, or no longer matching the query the user just extended),
    // there is nothing to follow and the old positional clamp is exactly right.
    let _g = BudgetGuard::always_sync();
    let dir = reorder_fixture("konoma_filter_gone");
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.start_filter();
    for c in "abc".chars() {
        app.filter_input_push(c);
    }
    app.tab.selected = app.tab.entries.len() - 1;
    let target = app.tab.entries[app.tab.selected].path.clone();

    // Typing one more character narrows the results so the selected file drops out entirely.
    app.filter_input_push('d');
    assert!(
        !app.tab.entries.iter().any(|e| e.path == target),
        "土台: 選択中のファイルは新しい結果に居ない"
    );
    assert!(
        app.tab.selected < app.tab.entries.len().max(1),
        "範囲内にクランプされる"
    );
    // And with nothing matching at all it must not go out of range either.
    for c in "zzz".chars() {
        app.filter_input_push(c);
    }
    assert!(app.tab.entries.is_empty(), "土台: 一致なし");
    assert_eq!(app.tab.selected, 0, "空でも範囲外にならない");
    std::fs::remove_dir_all(&dir).ok();
}

/// Fixture for the **FS-event / rebuild** path, where the trap is a different one: `rebuild_tree`
/// puts the *whole listing* back into `entries` before the re-filter runs, so anything reading
/// "the entry at `selected`" after that point is reading the unfiltered tree by a filtered index.
///
/// Laid out so those two orderings genuinely disagree **and** the file the wrong index lands on is
/// itself a match — otherwise the mistake is invisible (a non-matching anchor is simply not found,
/// and the fallback clamp quietly does the right thing):
///
/// ```text
/// whole listing      aaa_other_0  aaa_other_1  abc_new_0..4  x_a_b_c_0  x_a_b_c_1  x_a_b_c_2
/// filtered ("abc")                             abc_new_0..4  x_a_b_c_0  x_a_b_c_1  x_a_b_c_2
///                                                        ^                    ^
///                                             read at index 6            cursor at 6
/// ```
fn rebuild_reorder_fixture(prefix: &str) -> PathBuf {
    let dir = unique_tmp(prefix);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // Sort before "abc" and do NOT match it: present in the listing, absent from the results, so
    // the two indexings drift apart by exactly two.
    for i in 0..2 {
        std::fs::write(dir.join(format!("aaa_other_{i}.txt")), b"x").unwrap();
    }
    for i in 0..3 {
        std::fs::write(dir.join(format!("x_a_b_c_{i}.txt")), b"weak").unwrap();
    }
    dir
}

#[test]
fn an_fs_driven_rescan_keeps_the_cursor_on_the_same_file() {
    // Driven through **`refresh_fs_watched`** — the entry point `main`'s watcher actually calls —
    // because the bug lives in what that path does *before* re-filtering, and a test that calls the
    // re-filter directly cannot see it. An agent writing a burst of files is the live case.
    let _g = BudgetGuard::always_sync();
    let dir = rebuild_reorder_fixture("konoma_filter_fsreorder");
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.start_filter();
    for c in "abc".chars() {
        app.filter_input_push(c);
    }
    app.tab.selected = 1;
    let target = app.tab.entries[1].path.clone();
    assert!(target.ends_with("x_a_b_c_1.txt"), "土台: {target:?}");

    // First burst: five much better matches for the same query appear.
    for i in 0..5 {
        std::fs::write(dir.join(format!("abc_new_{i}.txt")), b"strong").unwrap();
    }
    app.refresh_fs_watched(false, &[]);
    assert_ne!(
        app.tab.entries.iter().position(|e| e.path == target),
        Some(1),
        "土台: 順位が入れ替わっていないとこのテストは何も検出しない"
    );
    assert_eq!(
        app.tab.entries[app.tab.selected].path, target,
        "1回目のイベントでカーソル下のファイルがすり替わらない"
    );

    // Second burst. By now the pool already holds the new matches, so an anchor mistakenly read
    // out of the rebuilt (unfiltered) listing is *found* — and the cursor is actively dragged onto
    // the wrong file rather than merely staying put.
    for i in 5..10 {
        std::fs::write(dir.join(format!("abc_new_{i}.txt")), b"strong").unwrap();
    }
    app.refresh_fs_watched(false, &[]);
    assert_eq!(
        app.tab.entries[app.tab.selected].path, target,
        "2回目のイベントでもカーソル下のファイルがすり替わらない"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn toggling_hidden_while_filtering_keeps_the_cursor_on_the_same_file() {
    // `.` reaches the re-filter through the same "rebuild first" shape as an FS event, so it has
    // the same trap — and here the pool is re-collected *before* the anchor is looked up, so a
    // wrong anchor is found on the very first press.
    let _g = BudgetGuard::always_sync();
    let dir = rebuild_reorder_fixture("konoma_filter_hiddenreorder");
    // Hidden, sort first, and match "abc" strongly: revealed by `.`, they take over the top of the
    // results and the head of the listing at the same time.
    for i in 0..5 {
        std::fs::write(dir.join(format!(".abc_hidden_{i}.txt")), b"strong").unwrap();
    }
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.start_filter();
    for c in "abc".chars() {
        app.filter_input_push(c);
    }
    app.tab.selected = 1;
    let target = app.tab.entries[1].path.clone();
    assert!(target.ends_with("x_a_b_c_1.txt"), "土台: {target:?}");

    app.toggle_hidden().unwrap();
    assert_ne!(
        app.tab.entries.iter().position(|e| e.path == target),
        Some(1),
        "土台: 順位が入れ替わっていないとこのテストは何も検出しない"
    );
    assert_eq!(
        app.tab.entries[app.tab.selected].path, target,
        "`.` でカーソル下のファイルがすり替わらない"
    );
    std::fs::remove_dir_all(&dir).ok();
}

// ---------------------------------------------------------------------------------------------
// B/C: the in-flight mark is what makes scans coalesce, so retiring a scan must not fake its
// disappearance. `invalidate_filter_pool_scan` used to clear `filter_pool_pending`, which meant
// `kick_filter_pool_refresh`'s guard could never hold on the tab-switch path — the one path that
// calls it on *every* switch.
// ---------------------------------------------------------------------------------------------

/// Two tabs on the same root, both filtering, with a filter-pool channel attached. Returns the
/// receiver so a test can count the walks that actually ran.
/// Each `/` is settled before the next step, so setup leaves **nothing** walking and the channel
/// empty: a walk left running here would land in the middle of the measurement that follows.
fn two_filtering_tabs(
    dir: &Path,
) -> (App, std::sync::mpsc::Receiver<crate::app::FilterPoolResult>) {
    let mut app = App::new(dir.to_path_buf(), Config::default()).unwrap();
    let rx = with_pool_channel(&mut app);
    app.start_filter();
    app.filter_input_push('f');
    settle_pool(&mut app, &rx);
    app.tab_new().unwrap();
    app.start_filter();
    app.filter_input_push('f');
    settle_pool(&mut app, &rx);
    (app, rx)
}

/// Wait until nothing is walking any more, applying every result on the way. Setup dispatches walks
/// of its own, and *draining* the channel is not enough to be rid of them — a walk that has not
/// finished yet has nothing in the channel to drain, and would otherwise land in the middle of the
/// measurement and be counted as part of it. The in-flight mark is the thing to wait on.
fn settle_pool(app: &mut App, rx: &std::sync::mpsc::Receiver<crate::app::FilterPoolResult>) {
    for _ in 0..64 {
        if app.filter_pool_pending.is_none() {
            return;
        }
        let res = rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("走査中なら必ず結果が届く");
        app.apply_filter_pool(res);
    }
    panic!("走査が収束しない");
}

#[test]
fn repeated_tab_switches_coalesce_into_a_single_filter_pool_scan() {
    // Holding `]` down is the failure: `main` drains the whole key burst in one iteration, so every
    // press ran `load_active` → `refresh_fs_after_tab_switch` → `kick_filter_pool_refresh`, and with
    // the guard structurally dead each one spawned its own whole-tree walk (~55ms of `readdir` on a
    // 50,000-entry tree). Counted by results reaching the channel = "how many walks ran".
    let _g = BudgetGuard::always_async();
    let dir = nested_fixture("konoma_pool_tabswitch", 4, 3);
    let (mut app, rx) = two_filtering_tabs(&dir);

    for _ in 0..8 {
        app.tab_cycle(1);
    }
    // Nothing is applied during the burst, so the mark stays taken throughout: exactly one switch
    // may dispatch, and the other seven must coalesce onto it into one further walk.
    let mut ran = 0;
    while let Ok(res) = rx.recv_timeout(std::time::Duration::from_millis(600)) {
        ran += 1;
        app.apply_filter_pool(res);
        assert!(ran <= 2, "タブ切替 8 回で走査が積み上がっている: {ran} 本");
    }
    assert_eq!(
        ran, 2,
        "8 回の切替から走ったのは 1 本 + 合体した 1 本だけ: {ran} 本"
    );
    assert!(
        !app.filter_pool_dirty,
        "合体した要求は取りこぼされず消化される"
    );
    assert!(
        app.filter_pool_pending.is_none(),
        "走査中フラグは latch しない(スピナーが回りっぱなしにならない)"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_tab_switch_to_a_different_root_still_dispatches_its_own_scan() {
    // Coalescing keys off the root, so it must not swallow the case it is *not* about: the newly
    // active tab is looking at a different directory, and the walk in flight cannot answer for it.
    let _g = BudgetGuard::always_async();
    let a = nested_fixture("konoma_pool_roota", 4, 3);
    let b = nested_fixture("konoma_pool_rootb", 4, 3);
    let mut app = App::new(a.clone(), Config::default()).unwrap();
    let rx = with_pool_channel(&mut app);
    app.start_filter();
    app.filter_input_push('f');
    settle_pool(&mut app, &rx);
    app.tab_new().unwrap();
    app.jump_to_dir(b.clone());
    app.start_filter();
    app.filter_input_push('f');
    settle_pool(&mut app, &rx);

    app.tab_cycle(1); // back to the tab rooted at `a`
    assert_eq!(
        app.filter_pool_pending.as_ref().map(|(_, p)| p.as_path()),
        Some(a.as_path()),
        "別 root へ移ったら合体せず、その root の走査を新たに飛ばす"
    );
    settle_pool(&mut app, &rx);
    assert!(
        app.tab.filter_pool.iter().all(|e| e.path.starts_with(&a)),
        "着地したのは移った先の root の内容"
    );
    std::fs::remove_dir_all(&a).ok();
    std::fs::remove_dir_all(&b).ok();
}

#[test]
fn a_superseded_scan_releases_the_slot_and_honours_the_coalesced_request() {
    // The hazard introduced by keeping `filter_pool_pending` across an invalidation: the walk's
    // result is now discarded by generation *before* anything cleared the slot it holds. If the
    // release is not done first, the mark latches — a spinner that never stops and, worse, a
    // coalescing guard that from then on rejects every future scan forever.
    let _g = BudgetGuard::always_async();
    let dir = nested_fixture("konoma_pool_supersede", 4, 3);
    let (mut app, rx) = two_filtering_tabs(&dir);

    // A walk is in flight for this root; a tab switch supersedes it (same root, so the switch's own
    // refresh coalesces onto it rather than dispatching).
    app.refresh_fs_changed(false, &[]).unwrap();
    let inflight = app.filter_pool_pending.clone().expect("走査中");
    app.tab_cycle(1);
    assert!(
        app.filter_pool_pending.is_some(),
        "走査は止まらない = 走査中の事実も消さない"
    );
    assert!(app.filter_pool_dirty, "切替の要求は合体して保留される");
    assert!(
        inflight.0 != app.filter_pool_gen,
        "土台: この結果はもう陳腐化している"
    );

    let stale = rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
    assert_eq!(stale.gen, inflight.0, "土台: 届いたのは陳腐化した結果");
    assert!(!app.apply_filter_pool(stale), "陳腐化した結果は適用しない");
    let after = rx
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("合体していた要求が着地後に実行される");
    assert!(app.apply_filter_pool(after));
    assert!(
        app.filter_pool_pending.is_none(),
        "陳腐化した結果でも走査中フラグは解ける(latch しない)"
    );
    assert!(!app.filter_pool_dirty);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_superseded_result_does_not_free_a_newer_scans_slot() {
    // The other direction of the same rule. If releasing the slot ignored the generation, a walk
    // superseded by a *newer dispatch* would clear the newer one's mark on arrival — and the next
    // FS event would then stack a second whole-tree walk on top of one still running.
    let _g = BudgetGuard::always_async();
    let dir = nested_fixture("konoma_pool_twowalks", 4, 3);
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    let rx = with_pool_channel(&mut app);
    app.start_filter();
    app.filter_input_push('f');
    settle_pool(&mut app, &rx);
    let old_gen = app.filter_pool_gen;

    // Move to a different root and back, so the next kick dispatches rather than coalescing.
    let other = nested_fixture("konoma_pool_twowalks_other", 2, 1);
    app.jump_to_dir(other.clone());
    app.start_filter();
    app.filter_input_push('f');
    let newer = app.filter_pool_pending.clone().expect("新しい走査が在る");
    assert_ne!(newer.0, old_gen, "土台: 世代が違う");

    // The first root's walk lands late.
    let stale = crate::app::FilterPoolResult {
        gen: old_gen,
        root: dir.clone(),
        entries: Some(Vec::new()),
        append: false,
    };
    assert!(!app.apply_filter_pool(stale));
    assert_eq!(
        app.filter_pool_pending,
        Some(newer),
        "まだ走っている新しい走査の枠を横取りしない"
    );
    while rx.try_recv().is_ok() {}
    std::fs::remove_dir_all(&dir).ok();
    std::fs::remove_dir_all(&other).ok();
}

#[test]
fn a_new_tab_does_not_inherit_the_previous_tabs_scan() {
    // `t` is the one way the active tab changes without going through `load_active`, so it never
    // retired the scan bookkeeping. The previous tab's walk stayed marked at the *current*
    // generation, which put "scanning files" on a brand-new tab that is not filtering at all, and
    // left the discard resting on `apply_filter_pool`'s "not filtering" check alone — no
    // generation, and no root either, since `t` opens at the same root.
    let _g = BudgetGuard::always_async();
    let dir = nested_fixture("konoma_pool_newtab", 4, 3);
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    let rx = with_pool_channel(&mut app);
    app.start_filter();
    app.filter_input_push('f');
    let inflight = app.filter_pool_pending.clone().expect("土台: 走査中");
    assert!(
        app.busy_jobs().contains(&crate::i18n::Msg::BusyFilterScan),
        "土台: 元のタブではスピナーが出ている"
    );

    app.tab_new().unwrap();

    assert!(
        !app.busy_jobs().contains(&crate::i18n::Msg::BusyFilterScan),
        "走査していない新しいタブにスピナーを出さない"
    );
    assert_ne!(
        inflight.0, app.filter_pool_gen,
        "前のタブの走査は世代で無効化される(絞り込みの有無に頼らない)"
    );
    let res = rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
    assert!(!app.apply_filter_pool(res), "前のタブの結果は捨てられる");
    assert!(app.tab.filter_pool.is_empty(), "新しいタブのプールは無傷");
    assert!(
        app.filter_pool_pending.is_none(),
        "走査中フラグは latch しない"
    );
    std::fs::remove_dir_all(&dir).ok();
}

// ---------------------------------------------------------------------------------------------
// D: `.` (toggle_hidden) while a filter is up.
// ---------------------------------------------------------------------------------------------

#[test]
fn toggling_hidden_while_filtering_keeps_the_filter_and_re_collects_the_pool() {
    // `toggle_hidden` only rebuilt the tree, and `rebuild_tree` puts the *whole* directory back
    // into `entries` — so the heading still said "filtering" while the list underneath was a plain
    // listing full of non-matching entries. And because the pool had been walked with the previous
    // `show_hidden`, revealing dotfiles in the tree still left `/` unable to find any of them.
    let _g = BudgetGuard::always_sync();
    let dir = unique_tmp("konoma_hidden_filter");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("visible_target.txt"), b"x").unwrap();
    std::fs::write(dir.join(".hidden_target.txt"), b"x").unwrap();
    std::fs::write(dir.join("unrelated.md"), b"x").unwrap();

    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.start_filter();
    for c in "target".chars() {
        app.filter_input_push(c);
    }
    let names = |app: &App| -> Vec<String> {
        app.tab
            .entries
            .iter()
            .map(|e| e.path.file_name().unwrap().to_string_lossy().into_owned())
            .collect()
    };
    assert_eq!(
        names(&app),
        vec!["visible_target.txt"],
        "土台: 隠しは出ない"
    );

    app.toggle_hidden().unwrap();
    assert_eq!(
        app.tab.tree_filter.as_deref(),
        Some("target"),
        "絞り込みは維持される"
    );
    let shown = names(&app);
    assert!(
        shown.iter().any(|n| n == ".hidden_target.txt"),
        "`.` で隠しファイルが絞り込み結果に入る: {shown:?}"
    );
    assert!(
        !shown.iter().any(|n| n == "unrelated.md"),
        "一致しないものが混ざらない(素の一覧に戻っていない): {shown:?}"
    );

    app.toggle_hidden().unwrap();
    let shown = names(&app);
    assert_eq!(shown, vec!["visible_target.txt"], "もう一度で元に戻る");
    std::fs::remove_dir_all(&dir).ok();
}

// ---------------------------------------------------------------------------------------------
// E: the walk's panic net has to cover the halves that run on the UI thread too, not just the
// worker — an unwind there takes the whole TUI down mid-keystroke rather than one background job.
// ---------------------------------------------------------------------------------------------

/// Arm the injected scan panic for this thread and disarm it on drop (so a failing assertion can
/// never leave it armed for the next test on this thread). See `set_filter_scan_panic`.
struct ScanPanicGuard;
impl ScanPanicGuard {
    fn armed() -> Self {
        crate::app::set_filter_scan_panic(true);
        Self
    }
}
impl Drop for ScanPanicGuard {
    fn drop(&mut self) {
        crate::app::set_filter_scan_panic(false);
    }
}

#[test]
fn a_panicking_walk_on_the_ui_thread_degrades_instead_of_taking_the_tui_down() {
    // Both UI-thread walks: `/`'s budgeted head start, and `spawn_or_sync_pool`'s no-channel
    // fallback (which every test and any run without a loader attached goes through).
    let _g = BudgetGuard::always_sync();
    let dir = nested_fixture("konoma_pool_syncpanic", 3, 3);
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    {
        let _p = ScanPanicGuard::armed();
        app.start_filter(); // must return, not unwind
    }
    assert!(
        app.tab.filter_pool.is_empty(),
        "失敗した走査は空プールになる"
    );
    assert!(
        app.filter_pool_pending.is_none(),
        "受け渡しもしないので走査中フラグは立たない"
    );
    assert!(!app.busy_jobs().contains(&crate::i18n::Msg::BusyFilterScan));
    for c in "f0".chars() {
        app.filter_input_push(c); // still usable: an empty pool just matches nothing
    }
    assert!(app.tab.entries.is_empty());

    // Recovering: with the injection disarmed the next scan repopulates normally.
    app.refresh_fs_changed(false, &[]).unwrap();
    assert!(!app.tab.filter_pool.is_empty(), "次の走査で復帰する");

    // The no-channel fallback (`spawn_or_sync_pool`), reached by forcing the hand-off with no
    // loader attached. Its degraded value is the same `None` the worker sends, which
    // `apply_filter_pool` already keeps the pool for.
    let pool = app.tab.filter_pool.len();
    {
        let _b = BudgetGuard::always_async();
        let _p = ScanPanicGuard::armed();
        app.refresh_fs_changed(false, &[]).unwrap(); // must return, not unwind
    }
    assert_eq!(app.tab.filter_pool.len(), pool, "失敗でプールは消されない");
    assert!(app.filter_pool_pending.is_none());
    std::fs::remove_dir_all(&dir).ok();
}

// ---------------------------------------------------------------------------------------------
// The `C` (changed-files-only) cursor must follow the *file*, not the index.
//
// `changed_paths` is sorted by path, so a change appearing anywhere above the cursor shifts every
// entry below it. The list was only ever range-clamped, so the index stayed in range and pointed at
// a different file — silently retargeting the next `Enter` / `d` / `y` / `Space→D`. This is the
// headline Agent Watch path: the list is rebuilt on every write the agent makes, i.e. exactly while
// the user is reading it.
// ---------------------------------------------------------------------------------------------

/// A repo shaped so that these tests **can** fail. Three things are deliberate:
///
/// - Everything committed stays unmodified, so it shows in the tree and never in `C`: the two lists
///   are genuinely different, and reading the cursor out of a rebuilt tree returns something
///   recognisably wrong rather than a plausible path by luck.
/// - The changed files live in a **collapsed subdirectory**, which is what an agent editing
///   `src/…` actually produces. The flat `C` list is then *longer than the tree*, so `rebuild_tree`
///   does not merely renumber the cursor, it clamps it — and a fixture without this passes against a
///   broken synchronous call site purely because the index happened to survive.
/// - The one the cursor parks on (`m_two.txt`) sorts **last**, so `aaa_new.txt` arriving in front of
///   it shifts it; without that shift the tests assert nothing.
#[cfg(feature = "git")]
fn changed_cursor_repo(prefix: &str) -> PathBuf {
    let dir = unique_tmp(prefix);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    init_git_repo(&dir);
    std::fs::write(dir.join("committed_a.txt"), b"c\n").unwrap();
    std::fs::write(dir.join("committed_z.txt"), b"c\n").unwrap();
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    std::fs::write(dir.join("sub").join("committed_s.txt"), b"c\n").unwrap();
    for args in [["add", "-A"], ["commit", "-m"]] {
        let mut cmd = std::process::Command::new("git");
        cmd.current_dir(&dir).arg(args[0]).arg(args[1]);
        if args[0] == "commit" {
            cmd.arg("base");
        }
        let out = cmd.output().unwrap();
        assert!(out.status.success(), "git {args:?}: {out:?}");
    }
    for n in ["m_one.txt", "m_two.txt", "m_three.txt"] {
        std::fs::write(dir.join("sub").join(n), b"x\n").unwrap();
    }
    dir.canonicalize().unwrap()
}

/// A new change that sorts to the **front** of the list, the way an agent adding a file does.
#[cfg(feature = "git")]
fn add_change_above(dir: &Path) -> PathBuf {
    let p = dir.join("sub").join("aaa_new.txt");
    std::fs::write(&p, b"y\n").unwrap();
    p
}

/// Park the cursor on the **last** changed file (index 2 — never 0, which a broken "keep the index"
/// would match by accident) and return its path. Also pins the shape the tests rely on: the flat
/// changed list is longer than the collapsed tree, so a rebuild clamps rather than renumbers.
#[cfg(feature = "git")]
fn park_on_last_change(app: &mut App) -> PathBuf {
    assert_eq!(app.tab.entries.len(), 3, "土台: 変更3件から始まる");
    app.tab.selected = 2;
    let target = app.tab.entries[2].path.clone();
    assert!(target.ends_with("m_two.txt"), "土台: {target:?}");
    target
}

/// The synchronous half: the FS event's own refresh rebuilds the list, and `rebuild_tree` has
/// already replaced `entries` with the whole tree by the time it does.
#[cfg(feature = "git")]
#[test]
fn changed_cursor_follows_its_file_when_a_new_change_sorts_above_it() {
    let dir = changed_cursor_repo("konoma_changed_cursor_sync");
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.toggle_changed_filter();
    let target = park_on_last_change(&mut app);

    // An agent creates a file whose name sorts to the **front** of the list.
    let newfile = add_change_above(&dir);
    app.refresh_fs_changed(false, &[newfile]).unwrap();

    let moved = app
        .tab
        .entries
        .iter()
        .position(|e| e.path == target)
        .expect("対象は今も変更ファイル");
    assert_eq!(
        moved, 3,
        "土台: 先頭に入って index がずれていないとこのテストは何も検出しない"
    );
    assert_eq!(
        app.tab.entries[app.tab.selected].path, target,
        "カーソルは同じファイルを指し続ける(index ではなくファイルを追う)"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// The asynchronous half — **the path the real app takes**. `refresh_fs` only *kicks* the scan, so
/// the rebuild is deferred to `apply_statuses`, by which time `entries` is the whole tree. Fixing
/// only the synchronous call site leaves this one broken while the test above goes green.
#[cfg(feature = "git")]
#[test]
fn changed_cursor_survives_a_rebuild_deferred_to_the_async_scan() {
    let dir = changed_cursor_repo("konoma_changed_cursor_async");
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    app.attach_status_loader(tx);
    app.toggle_changed_filter(); // `C` has a synchronous contract, so the list is up right here
    let target = park_on_last_change(&mut app);

    let newfile = add_change_above(&dir);
    app.refresh_fs_changed(false, &[newfile]).unwrap();

    // Ground truth for what makes this the harder half: the rebuild really was deferred, and what
    // is in `entries` right now is the *tree* — so the cursor's identity cannot be recovered from it.
    assert!(
        app.git_status_pending.is_some(),
        "土台: 走査中なので一覧の作り直しは apply_statuses に先送りされる"
    );
    assert!(
        app.tab
            .entries
            .iter()
            .any(|e| e.path.ends_with("committed_a.txt")),
        "土台: この時点の entries は変更一覧ではなく通常ツリー"
    );
    assert!(
        !app.tab.entries[app.tab.selected]
            .path
            .ends_with("m_two.txt"),
        "土台: ここで entries から読むと別のファイルになる(だから呼び出し元が持ち越す)"
    );

    let res = rx
        .recv_timeout(std::time::Duration::from_secs(30))
        .expect("スキャン結果");
    app.apply_statuses(res); // what main's run loop does with the channel

    assert!(
        app.tab
            .entries
            .iter()
            .any(|e| e.path.ends_with("aaa_new.txt")),
        "土台: 到着で一覧が作り直されている"
    );
    assert_eq!(
        app.tab.entries[app.tab.selected].path, target,
        "先送りされた作り直しでもカーソルは同じファイルに残る"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// The other half of the contract: "follow the file" must not turn into "refuse to move". When the
/// selected file leaves the list (committed, reverted, deleted) there is nothing to follow, and the
/// old positional clamp is exactly right.
#[cfg(feature = "git")]
#[test]
fn changed_cursor_falls_back_to_clamping_when_its_file_leaves_the_list() {
    let dir = changed_cursor_repo("konoma_changed_cursor_gone");
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.toggle_changed_filter();
    let target = park_on_last_change(&mut app);

    // The agent reverts that one file: it is no longer changed, so it drops out of the list.
    std::fs::remove_file(&target).unwrap();
    app.refresh_fs_changed(false, std::slice::from_ref(&target))
        .unwrap();

    assert!(
        !app.tab.entries.iter().any(|e| e.path == target),
        "土台: 対象は一覧から消えた"
    );
    assert_eq!(app.tab.entries.len(), 2, "土台: 残りは2件");
    assert!(
        app.tab.selected < app.tab.entries.len(),
        "追えない時は従来どおり範囲内にクランプされる(選択が範囲外で固まらない)"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// `.` (toggle_hidden) while the `C` view is up rebuilds the tree and then re-derives the list. The
/// changed set does not depend on `show_hidden`, so the list comes back identical — but it is
/// re-derived *through* a tree that is shorter than it, so the cursor is clamped on the way and has
/// to be put back by identity. The anchor for this one is taken by `toggle_hidden`, ahead of its
/// rebuild.
#[cfg(feature = "git")]
#[test]
fn changed_cursor_survives_toggling_hidden_files() {
    let dir = changed_cursor_repo("konoma_changed_cursor_hidden");
    // Two more changes in the same collapsed directory, so the flat list (5) is strictly longer than
    // the tree it is rebuilt through (`committed_a.txt`, `committed_z.txt`, `sub` = 3). Without that
    // the clamp has nothing to do and the cursor survives by luck rather than by design.
    for n in ["m_four.txt", "m_five.txt"] {
        std::fs::write(dir.join("sub").join(n), b"x\n").unwrap();
    }
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.toggle_changed_filter();
    assert_eq!(app.tab.entries.len(), 5, "土台: 変更5件");
    app.tab.selected = 4;
    let target = app.tab.entries[4].path.clone();
    assert!(target.ends_with("m_two.txt"), "土台: {target:?}");

    app.toggle_hidden().unwrap();

    assert_eq!(app.tab.entries.len(), 5, "変更一覧は `.` で変わらない");
    assert_eq!(
        app.tab.entries[app.tab.selected].path, target,
        "`.` を挟んでもカーソルは同じファイルに残る"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// A deferred anchor is a **path** carried by `PerTab` for a scan that has not landed yet, so it
/// must not be able to reach a *different* tab's list — on two tabs of the same repo the path exists
/// in both, so a leak would be found and acted on rather than harmlessly ignored.
///
/// Asserted on the published anchor itself rather than on the resulting cursor: an end-to-end
/// version of this is **vacuous**, because entering `C` on the second tab goes through
/// `ensure_git_status_now`, which bumps the generation and makes `apply_statuses` discard the first
/// tab's result before it can reach the list at all. The invariant that actually carries the weight
/// is the one below — a fresh tab's own `PerTab` starts at `None`, and switching to it restores
/// *its* value, never tab 1's.
///
/// This is the isolation half of the contract; the persistence half — that tab 1's *own* anchor
/// survives being saved and switched back to, rather than being discarded the instant it stops
/// being active — is `a_deferred_changed_rebuild_survives_a_tab_switch_and_back` below.
#[cfg(feature = "git")]
#[test]
fn a_deferred_changed_anchor_is_not_inherited_by_the_next_tab() {
    let dir = changed_cursor_repo("konoma_changed_cursor_tabs");
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    let (tx, _rx) = std::sync::mpsc::channel();
    app.attach_status_loader(tx);
    app.toggle_changed_filter();
    let parked = park_on_last_change(&mut app); // tab 1 sits on m_two.txt

    // Tab 1 defers a rebuild, publishing m_two.txt for whenever the scan lands.
    let newfile = add_change_above(&dir);
    app.refresh_fs_changed(false, &[newfile]).unwrap();
    assert!(app.git_status_pending.is_some(), "土台: 先送りされている");
    assert_eq!(
        app.tab.changed_anchor_pending.as_deref(),
        Some(parked.as_path()),
        "土台: 先送り分のアンカーが公開されている"
    );

    // `tab_new` builds a brand-new `PerTab` in place (reusing `self.tab`, not cloning tab 1's), and
    // `clear_filter_state` (called near its end) resets that fresh tab's own anchor to `None` — so
    // it never sees tab 1's still-pending value.
    app.tab_new().unwrap();
    assert!(!app.tab.changed_filter, "土台: 新しいタブは通常ツリー");
    assert!(
        app.tab.changed_anchor_pending.is_none(),
        "新しいタブに先送りアンカーが引き継がれない"
    );

    // And the ordinary switch: each tab's anchor is genuinely its own, so switching to tab 2 reads
    // *its* (still `None`) value, not tab 1's.
    app.tab_goto(0);
    assert!(app.tab.changed_filter, "土台: 1枚目は C 表示のまま");
    app.tab_cycle(1);
    assert!(
        app.tab.changed_anchor_pending.is_none(),
        "タブ切替でも先送りアンカーは引き継がれない"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// **Residual bug (2026-08-08), same shape as Bug 1-4 above**: `changed_anchor_pending` used to live
/// directly on `App` (not `PerTab`), and `save_active` explicitly reset it to `None` on *every* tab
/// switch (see the removed reasoning: "one tab's anchor must never be found and acted on by another
/// tab's rebuild"). That reset also discarded a tab's *own* still-pending anchor the moment the user
/// switched away from it, even though the rest of its state (`entries`, `changed_filter`, `selected`)
/// survives the switch just fine — so the snapshot and its anchor fell out of sync. When the deferred
/// scan eventually landed (`apply_statuses`, run against whichever tab happened to be active by
/// then), it fell back to reading the cursor out of the stale full-tree `entries` left behind by
/// `rebuild_tree`, landing on an arbitrary, unrelated file instead of the one the deferred rebuild
/// was anchored on.
#[cfg(feature = "git")]
#[test]
fn a_deferred_changed_rebuild_survives_a_tab_switch_and_back() {
    let dir = changed_cursor_repo("konoma_changed_defer_tabswitch");
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    app.attach_status_loader(tx);
    app.toggle_changed_filter();
    let target = park_on_last_change(&mut app); // sub/m_two.txt

    // Defer a rebuild to the (real, threaded) status scan.
    let newfile = add_change_above(&dir);
    app.refresh_fs_changed(false, &[newfile]).unwrap();
    assert!(
        app.git_status_pending.is_some(),
        "土台: 走査中で先送りされている"
    );

    // Switch to a new tab (this is what `save_active` runs through) and confirm the fresh tab does
    // not inherit tab 0's pending anchor.
    app.tab_new().unwrap();
    assert!(!app.tab.changed_filter, "土台: 新しいタブは通常ツリー");
    assert!(
        app.tab.changed_anchor_pending.is_none(),
        "新しいタブに先送りアンカーが引き継がれない"
    );

    // ...but tab 0's own anchor must have survived being saved: it sits in its snapshot slot,
    // untouched (the reset this test used to hit is gone — `save_active` no longer has anything
    // App-level to retire).
    assert_eq!(
        app.tabs[0].changed_anchor_pending.as_deref(),
        Some(target.as_path()),
        "タブ0のスナップショット自身は先送りアンカーを保持したまま(save_active で消されていない)"
    );

    // Switch back **before** the scan lands.
    app.tab_goto(0);
    assert!(app.tab.changed_filter, "戻ったタブは C 表示のまま");

    // Land the deferred scan now — the same thing `apply_statuses` does for whichever tab happens
    // to be active when the result arrives (tab 0 again, here).
    let res = rx
        .recv_timeout(std::time::Duration::from_secs(30))
        .expect("スキャン結果");
    app.apply_statuses(res);

    assert_eq!(
        app.tab.entries.len(),
        4,
        "着地後、変更一覧は新規分を含め4件"
    );
    assert!(
        app.tab
            .entries
            .iter()
            .all(|e| !e.path.ends_with("committed_a.txt") && !e.path.ends_with("committed_z.txt")),
        "着地後の一覧に無変更の通常ツリーのファイルが混ざっていない"
    );
    assert_eq!(
        app.tab.entries[app.tab.selected].path, target,
        "タブ切替を挟んでもカーソルは同じファイルに残る(先送りアンカーが引き継がれている)"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// Non-regression: following the file must not pin the cursor against the moves that are *supposed*
/// to move it. `n`/`N` walk the changed files, and `C` off/on resets to the top.
#[cfg(feature = "git")]
#[test]
fn changed_cursor_anchoring_does_not_block_deliberate_moves() {
    let dir = changed_cursor_repo("konoma_changed_cursor_moves");
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.toggle_changed_filter();
    let first = app.tab.entries[0].path.clone();
    app.jump_changed(1);
    assert_eq!(app.tab.selected, 1, "n は次の変更ファイルへ進む");
    app.jump_changed(-1);
    assert_eq!(
        app.tab.entries[app.tab.selected].path, first,
        "N は前へ戻る"
    );

    app.tab.selected = 2;
    app.toggle_changed_filter(); // off
    assert!(!app.tab.changed_filter);
    app.toggle_changed_filter(); // on again
    assert_eq!(
        app.tab.selected, 0,
        "入り直しは先頭から(カーソルは固定されない)"
    );
    std::fs::remove_dir_all(&dir).ok();
}

// ---------------------------------------------------------------------------------------------
// Bug 1/2/3/4 (2026-08-08): "every place that rebuilds `entries` or takes a `C` anchor must
// stay in sync with `changed_filter`" is one shape, not four unrelated bugs — see each test's
// doc comment for the specific hazard it pins down.
// ---------------------------------------------------------------------------------------------

/// **Bug 1**: `changed_anchor_pending` was overwritten *unconditionally* on every
/// `refresh_fs_changed` call, even while a previous refresh's anchor was still waiting for
/// `apply_statuses` to consume it. By the time a **second** FS event lands before the first status
/// scan finishes, `entries` is already the whole tree (the first call's `rebuild_tree` replaced
/// it), so re-reading `filter_anchor()` here reads the wrong structure and clobbers the correct
/// pending identity with whatever happens to sit at `selected` in the full tree — the very next
/// scan landing then drags the cursor onto that wrong file. This is the headline Agent Watch
/// scenario: an agent writing files back-to-back, faster than one `git status` scan.
#[cfg(feature = "git")]
#[test]
fn changed_cursor_survives_two_refreshes_deferred_to_the_same_scan() {
    let dir = changed_cursor_repo("konoma_changed_cursor_async_x2");
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    app.attach_status_loader(tx);
    app.toggle_changed_filter();
    let target = park_on_last_change(&mut app);

    // First FS event: the rebuild is deferred to `apply_statuses` (a scan is now in flight).
    let newfile = add_change_above(&dir);
    app.refresh_fs_changed(false, &[newfile]).unwrap();
    assert!(
        app.git_status_pending.is_some(),
        "土台: 1回目のスキャンがまだ走行中"
    );
    assert_eq!(
        app.tab.changed_anchor_pending.as_deref(),
        Some(target.as_path()),
        "土台: 1回目の先送りアンカーは正しい"
    );

    // A second FS event arrives **before that scan lands** (back-to-back agent writes). `entries`
    // is already the whole tree at this point (the first call's rebuild replaced it), so this
    // call's own `filter_anchor()` would read the wrong structure if it were consulted again.
    let newfile2 = dir.join("sub").join("bbb_second.txt");
    std::fs::write(&newfile2, b"z\n").unwrap();
    app.refresh_fs_changed(false, &[newfile2]).unwrap();
    assert!(
        app.git_status_pending.is_some(),
        "土台: 2回目の要求もコアレスされ、まだ同じスキャンが走行中"
    );
    assert_eq!(
        app.tab.changed_anchor_pending.as_deref(),
        Some(target.as_path()),
        "2回目の refresh で先送りアンカーが汚染されない(正しいアンカーが保持される)"
    );

    let res = rx
        .recv_timeout(std::time::Duration::from_secs(30))
        .expect("スキャン結果");
    app.apply_statuses(res);

    assert_eq!(
        app.tab.entries[app.tab.selected].path, target,
        "2回連続の先送りでもカーソルは同じファイルに残る"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// **Bug 2**: `toggle_hidden`'s reapply of `C` (via `refilter_after_visibility_change`) rebuilt the
/// changed list *unconditionally*, without the same "a scan is in flight" guard `refresh_fs_inner`
/// already has. Switching to a different repo's tab and back leaves `git_status` briefly empty
/// (the kick clears it because the workdir changed) while the real result is still on its way —
/// pressing `.` in that exact window used to rebuild against the empty status, judge "zero
/// changes", and silently turn `C` off with a false "no changed files".
#[cfg(feature = "git")]
#[test]
fn toggle_hidden_does_not_falsely_disable_changed_filter_during_a_pending_scan() {
    let dir_a = changed_cursor_repo("konoma_changed_hidden_race_a");
    let dir_b = changed_cursor_repo("konoma_changed_hidden_race_b");
    let mut app = App::new(dir_a.clone(), Config::default()).unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    app.attach_status_loader(tx);

    app.toggle_changed_filter();
    let target = park_on_last_change(&mut app);

    // Visit repo B's tab and actually land its status, so `git_status_workdir` genuinely becomes
    // B's (not just B's root sitting unrefreshed).
    app.tab_new().unwrap();
    app.jump_to_dir(dir_b.clone());
    app.refresh_git_if_needed();
    let res_b = rx
        .recv_timeout(std::time::Duration::from_secs(30))
        .expect("B のスキャン結果");
    app.apply_statuses(res_b);
    assert_eq!(
        app.git_status_workdir,
        crate::git::workdir(&dir_b),
        "土台: workdir は今 B"
    );

    // Switch back to tab A: `load_active` marks status dirty, and the resulting refresh's own
    // `kick_status_refresh` sees the workdir changed (B -> A) and clears `git_status` while the
    // real result for A is still on its way.
    app.tab_goto(0);
    assert!(app.tab.changed_filter, "土台: A は C 表示のまま復元される");
    assert!(
        app.git_status_pending.is_some(),
        "土台: 復帰で A の再検証スキャンが走行中"
    );
    assert!(
        app.git_status.is_empty(),
        "土台: workdir 切替の瞬間、status は空になっている"
    );

    // `.` pressed right in that window.
    app.toggle_hidden().unwrap();

    assert!(
        app.tab.changed_filter,
        "スキャン走行中の `.` で C が偽の「変更なし」判定を起こして OFF にならない"
    );

    // Drain until the deferred rebuild actually lands.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while app.git_status_pending.is_some() && std::time::Instant::now() < deadline {
        if let Ok(res) = rx.recv_timeout(std::time::Duration::from_secs(30)) {
            app.apply_statuses(res);
        } else {
            break;
        }
    }
    assert!(
        app.git_status_pending.is_none(),
        "土台: 最終的にはスキャンが着地する"
    );
    assert!(app.tab.changed_filter, "着地後も C は ON のまま");
    assert_eq!(
        app.tab.entries[app.tab.selected].path, target,
        "着地後カーソルは同じファイルに残る"
    );
    std::fs::remove_dir_all(&dir_a).ok();
    std::fs::remove_dir_all(&dir_b).ok();
}

/// **Bug 3**: `sort_menu_key` rebuilt the tree on `n`/`s`/`m`/`e`/`r`/`.` without ever reapplying
/// `C` (unlike `toggle_hidden`, which does via `refilter_after_visibility_change`) — so pressing a
/// sort key while the changed-files view was up silently replaced the flat changed list with the
/// ordinary tree, while the header/chip kept claiming CHANGED (`changed_filter` itself was never
/// touched, so nothing else noticed the mismatch).
#[cfg(feature = "git")]
#[test]
fn sort_menu_key_keeps_the_changed_filter_active() {
    let dir = changed_cursor_repo("konoma_changed_sort_test");
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.toggle_changed_filter();
    let changed_before: Vec<PathBuf> = app.tab.entries.iter().map(|e| e.path.clone()).collect();
    assert_eq!(changed_before.len(), 3, "土台: 変更3件");

    app.open_sort_menu();
    app.sort_menu_key('s').unwrap(); // switch sort key to Size

    assert!(
        app.changed_filter(),
        "s の後も CHANGED フィルタは解除されない(見出しと一覧の食い違いが起きない)"
    );
    let after: Vec<PathBuf> = app.tab.entries.iter().map(|e| e.path.clone()).collect();
    assert_eq!(
        after, changed_before,
        "s の後も一覧は変更ファイルのまま(通常ツリーに戻っていない・C の並びはソート設定を無視した固定の path 順)"
    );
    assert!(
        app.tab.entries.iter().all(|e| !e.is_dir),
        "一覧に通常ツリーのディレクトリ(sub 等)が混ざっていない"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// **Bug 4**: `start_filter` (`/`) didn't clear `changed_filter`, so pressing `/` while `C` was
/// active left *both* filters set. `refresh_fs_inner`'s `if self.tab.changed_filter { .. } else if
/// self.tab.tree_filter.is_some() { .. }` then always takes the changed branch, so `/`'s own
/// filter-pool refresh (`kick_filter_pool_refresh`) is never dispatched, and a later FS event
/// rebuilds the *changed* list over the `/` results the user is typing into instead.
#[cfg(feature = "git")]
#[test]
fn start_filter_turns_off_the_changed_filter() {
    let dir = changed_cursor_repo("konoma_changed_slash_test");
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.toggle_changed_filter();
    assert!(app.changed_filter(), "土台: C が ON");
    assert_eq!(app.tab.entries.len(), 3, "土台: 変更3件が見えている");

    app.start_filter();
    assert!(
        !app.changed_filter(),
        "`/` を開始すると C は解除される(相互排他・`toggle_changed_filter` の逆方向と対称)"
    );
    assert!(app.is_filtering(), "土台: `/` の入力モードに入っている");
    assert!(
        app.tab.entries.is_empty(),
        "`/` はクエリが空の間は何も出さない(C の一覧が残っていない)"
    );

    // Consequence check: without the fix, `changed_filter` stays on and wins the if/else-if
    // ordering in `refresh_fs_inner`, so a later FS event rebuilds the *changed* list (3 entries)
    // over the `/` session instead of refreshing its filter pool.
    let touched = dir.join("sub").join("m_one.txt");
    app.refresh_fs_changed(false, std::slice::from_ref(&touched))
        .unwrap();
    assert!(
        app.tab.entries.is_empty(),
        "FS イベントの後も `/` セッションのまま(件数0を維持・変更一覧に上書きされない)"
    );
    assert!(
        app.tab.tree_filter.is_some(),
        "`/` セッション自体は継続している"
    );

    std::fs::remove_dir_all(&dir).ok();
}

// ---------------------------------------------------------------------------------------------
// `reveal_and_select` (2026-08-08): every completed file operation (create/rename via
// `dialog_submit`, paste/duplicate/trash/delete via `apply_file_op`) ends by calling this to put
// the cursor on the thing that was just acted on. It called `rebuild_tree()` **on its own**,
// unconditionally replacing `entries` with the ordinary listing and never reapplying `C`/`/`
// afterward — so any of those operations, performed while a tree filter was active, silently
// dropped out of it: the header kept claiming CHANGED (or the `/` query), but the list underneath
// became the ordinary tree. This is the same shape as Bug 1-4 above, just at a different rebuild
// call site.
// ---------------------------------------------------------------------------------------------

/// The `dialog_submit` `Create` path: `self.refresh()?` (which correctly reapplies `C`) runs first,
/// so without the fix the drop only happens on `reveal_and_select`'s own second, unfiltered rebuild
/// right after it — easy to miss by only checking the state right after `refresh()`.
#[cfg(feature = "git")]
#[test]
fn creating_a_file_keeps_the_changed_filter_active() {
    let dir = changed_cursor_repo("konoma_reveal_create_changed");
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.toggle_changed_filter();
    park_on_last_change(&mut app); // cursor on sub/m_two.txt (a file, so op_base_dir = sub/)

    app.start_create();
    for c in "zz_new.txt".chars() {
        app.dialog_input_push(c);
    }
    app.dialog_submit().unwrap();

    assert!(app.changed_filter(), "作成後も C フィルタは解除されない");
    assert!(
        app.tab.entries.iter().all(|e| {
            !e.path.ends_with("committed_a.txt") && !e.path.ends_with("committed_z.txt")
        }),
        "一覧に無変更の通常ツリーのファイルが混ざっていない(通常ツリーへ戻っていない): {:?}",
        app.tab.entries.iter().map(|e| &e.path).collect::<Vec<_>>()
    );
    let created = dir.join("sub").join("zz_new.txt");
    assert!(
        app.tab.entries.iter().any(|e| e.path == created),
        "新規作成したファイルが変更一覧に現れる"
    );
    assert_eq!(
        app.tab.entries[app.tab.selected].path, created,
        "カーソルは新規作成したファイル上にある"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// The `dialog_submit` `Rename` path.
#[cfg(feature = "git")]
#[test]
fn renaming_a_file_keeps_the_changed_filter_active() {
    let dir = changed_cursor_repo("konoma_reveal_rename_changed");
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.toggle_changed_filter();
    let target = park_on_last_change(&mut app); // sub/m_two.txt

    app.start_rename(); // prefilled with "m_two.txt"
    for _ in 0.."m_two.txt".chars().count() {
        app.dialog_input_backspace();
    }
    for c in "renamed_two.txt".chars() {
        app.dialog_input_push(c);
    }
    app.dialog_submit().unwrap();

    assert!(
        app.changed_filter(),
        "リネーム後も C フィルタは解除されない"
    );
    assert!(
        app.tab.entries.iter().all(|e| {
            !e.path.ends_with("committed_a.txt") && !e.path.ends_with("committed_z.txt")
        }),
        "一覧に無変更の通常ツリーのファイルが混ざっていない: {:?}",
        app.tab.entries.iter().map(|e| &e.path).collect::<Vec<_>>()
    );
    let renamed = target.parent().unwrap().join("renamed_two.txt");
    assert!(
        app.tab.entries.iter().any(|e| e.path == renamed),
        "リネーム後のファイルが変更一覧に現れる"
    );
    assert_eq!(
        app.tab.entries[app.tab.selected].path, renamed,
        "カーソルはリネーム後のファイル上にある"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// The `apply_file_op` path (paste/duplicate/trash/delete all funnel through the same
/// `reveal_and_select` call). Simulated the same way the existing cross-tab `FileOpResult` tests
/// above do: the worker's work (here, a duplicate) is performed directly on disk, and
/// `apply_file_op` is called with the result it would have produced.
#[cfg(feature = "git")]
#[test]
fn duplicating_a_file_keeps_the_changed_filter_active() {
    let dir = changed_cursor_repo("konoma_reveal_dup_changed");
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.rebuild_tree().unwrap();
    app.toggle_changed_filter();
    let target = park_on_last_change(&mut app); // sub/m_two.txt

    let duplicated = target.parent().unwrap().join("m_two copy.txt");
    std::fs::copy(&target, &duplicated).unwrap();
    app.fileop_gen = 101;
    app.fileop_pending = Some(FileOpKind::Duplicate);
    assert!(app.apply_file_op(FileOpResult {
        gen: 101,
        kind: FileOpKind::Duplicate,
        root: dir.clone(),
        ok: 1,
        last: Some(duplicated.clone()),
        err: None,
    }));

    assert!(app.changed_filter(), "複製後も C フィルタは解除されない");
    assert!(
        app.tab.entries.iter().all(|e| {
            !e.path.ends_with("committed_a.txt") && !e.path.ends_with("committed_z.txt")
        }),
        "一覧に無変更の通常ツリーのファイルが混ざっていない: {:?}",
        app.tab.entries.iter().map(|e| &e.path).collect::<Vec<_>>()
    );
    assert!(
        app.tab.entries.iter().any(|e| e.path == duplicated),
        "複製後のファイルが変更一覧に現れる"
    );
    assert_eq!(
        app.tab.entries[app.tab.selected].path, duplicated,
        "カーソルは複製後のファイル上にある"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// The `/` (name filter) side of the same bug — `reveal_and_select` is not `C`-specific, so the
/// unfiltered rebuild dropped an active `/` session exactly the same way. Doesn't need the `git`
/// feature: `/` has nothing to do with git status.
#[test]
fn creating_a_file_keeps_the_name_filter_active() {
    let dir = unique_tmp("konoma_reveal_create_filter");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("alpha.rs"), b"x").unwrap();
    std::fs::write(dir.join("beta.rs"), b"x").unwrap();
    std::fs::write(dir.join("gamma.txt"), b"x").unwrap(); // does not match the "rs" query below
    let dir = dir.canonicalize().unwrap();

    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.rebuild_tree().unwrap();
    app.start_filter();
    for c in "rs".chars() {
        app.filter_input_push(c);
    }
    assert_eq!(
        app.tab.entries.len(),
        2,
        "土台: `rs` フィルタで2件(alpha.rs/beta.rs)"
    );
    app.tab.selected = 0;

    app.start_create();
    for c in "delta.rs".chars() {
        app.dialog_input_push(c);
    }
    app.dialog_submit().unwrap();

    assert_eq!(
        app.tab.tree_filter.as_deref(),
        Some("rs"),
        "作成後も `/` フィルタは解除されない"
    );
    assert!(
        !app.tab.entries.iter().any(|e| e.path.ends_with("gamma.txt")),
        "一覧にクエリへ一致しない通常ツリーのファイルが混ざっていない(通常ツリーへ戻っていない): {:?}",
        app.tab.entries.iter().map(|e| &e.path).collect::<Vec<_>>()
    );
    let created = dir.join("delta.rs");
    assert!(
        app.tab.entries.iter().any(|e| e.path == created),
        "新規作成したファイルがフィルタ結果に現れる"
    );
    assert_eq!(
        app.tab.entries[app.tab.selected].path, created,
        "カーソルは新規作成したファイル上にある"
    );
    std::fs::remove_dir_all(&dir).ok();
}

// -------------------------------------------------------------------------------------------
// FIFO / device-file preview: `tree_activate` (and every other `enter_preview` entry point)
// used to check only `entry.is_dir`, so a FIFO/character-device "file" got handed straight to
// `enter_preview`, which resolves a `PreviewKind` (opening the path to sniff it) and then opens
// it again to actually read it. `File::open` on a FIFO with nobody writing to the other end
// blocks the calling thread forever (POSIX `open(2)`) — and `enter_preview` runs on the
// key-processing thread, so this froze the whole UI (not even `q`/`Ctrl-C` got processed again).
// -------------------------------------------------------------------------------------------

/// Run `f` (expected to call `enter_preview`/`tree_activate`/similar on `app`, entirely on this
/// call) on a background thread and require it to finish within `timeout`. If the fix regresses
/// and the underlying `File::open` blocks on a FIFO with no writer, that call can never return by
/// itself (there is no way to cancel a blocked `open(2)` from another thread) — so rather than let
/// the whole test (and everything after it in the same `cargo test` process) hang forever, this
/// fails loudly via `recv_timeout` instead. The stuck worker thread is simply leaked when that
/// happens (harmless: it just sits blocked until process exit, and doesn't stop the test binary
/// from finishing the run).
#[cfg(unix)]
fn run_with_hang_guard<F>(mut app: App, timeout: std::time::Duration, f: F) -> App
where
    F: FnOnce(&mut App) + Send + 'static,
{
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        f(&mut app);
        // Send-back failing (the receiver having already given up after a timeout) is fine —
        // there's nobody left to report to.
        let _ = tx.send(app);
    });
    rx.recv_timeout(timeout).unwrap_or_else(|_| {
        panic!(
            "operation did not return within {timeout:?} — File::open reached a FIFO/device and \
             blocked on the key-processing path"
        )
    })
}

/// Entering preview on a FIFO must not hang, and must degrade to `CanNotPreview` (principle #3)
/// instead of resolving to `Text`/`Code`/any kind whose renderer would try to actually open it.
#[cfg(unix)]
#[test]
fn entering_preview_on_a_fifo_does_not_hang_and_degrades_safely() {
    let dir = unique_tmp("konoma_fifo_preview_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let fifo = dir.join("pipe");
    let status = std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .expect("mkfifo コマンドを起動できない");
    assert!(status.success(), "mkfifo に失敗");

    let app = App::new(dir.clone(), Config::default()).unwrap();
    let target = fifo.clone();
    let app = run_with_hang_guard(app, std::time::Duration::from_secs(5), move |a| {
        a.enter_preview(&target);
    });

    assert!(
        matches!(
            app.tab.preview_kind,
            Some(PreviewKind::CanNotPreview { .. })
        ),
        "FIFO は CanNotPreview へ安全降格するはず: {:?}",
        app.tab.preview_kind
    );
    assert_eq!(
        app.tab.mode,
        Mode::Preview,
        "モード遷移自体は既存の unsupported extension と同じく Preview のまま"
    );
    assert_eq!(app.tab.preview_path.as_deref(), Some(fifo.as_path()));

    std::fs::remove_dir_all(&dir).ok();
}

/// A FIFO reached via a *directory* path also degrades safely (no crash), even though production
/// code never calls `enter_preview` directly on a directory (`tree_activate`/`tree_descend` branch
/// on `entry.is_dir` first) — a direct robustness check on the guard itself.
#[cfg(unix)]
#[test]
fn entering_preview_on_a_directory_degrades_safely() {
    let dir = unique_tmp("konoma_dir_preview_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let sub = dir.join("sub");
    std::fs::create_dir_all(&sub).unwrap();

    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.enter_preview(&sub);
    assert!(
        matches!(
            app.tab.preview_kind,
            Some(PreviewKind::CanNotPreview { .. })
        ),
        "ディレクトリを直接渡しても CanNotPreview へ安全降格: {:?}",
        app.tab.preview_kind
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// Non-regression: an ordinary regular file previews exactly as before (guard doesn't reject it).
#[test]
fn entering_preview_on_a_regular_file_still_works() {
    let dir = unique_tmp("konoma_regular_preview_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("note.txt");
    std::fs::write(&file, b"hello world\n").unwrap();

    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.enter_preview(&file);
    assert!(
        matches!(app.tab.preview_kind, Some(PreviewKind::Text(_))),
        "通常ファイルは従来どおり Text として開ける: {:?}",
        app.tab.preview_kind
    );
    assert_eq!(app.tab.mode, Mode::Preview);

    std::fs::remove_dir_all(&dir).ok();
}

/// Non-regression: a symlink to a regular file still previews the link target's content, exactly
/// as before (the guard follows symlinks rather than rejecting them outright).
#[cfg(unix)]
#[test]
fn entering_preview_on_a_symlink_to_a_regular_file_still_works() {
    let dir = unique_tmp("konoma_symlink_preview_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let real = dir.join("real.txt");
    std::fs::write(&real, b"real content\n").unwrap();
    let link = dir.join("link.txt");
    std::os::unix::fs::symlink(&real, &link).unwrap();

    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.enter_preview(&link);
    assert!(
        matches!(app.tab.preview_kind, Some(PreviewKind::Text(_))),
        "通常ファイルへのシンボリックリンクは従来どおり Text として開ける: {:?}",
        app.tab.preview_kind
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// Non-regression: `tree_descend` (`l`) still descends into a directory rather than trying to
/// preview it — the guard lives in `enter_preview` and never runs for directory entries, since
/// `tree_activate`/`tree_descend` branch on `entry.is_dir` before ever calling it.
#[test]
fn tree_descend_still_descends_into_directories() {
    let dir = unique_tmp("konoma_dir_descend_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let sub = dir.join("sub");
    std::fs::create_dir_all(&sub).unwrap();
    std::fs::write(sub.join("inner.txt"), b"x").unwrap();

    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    app.tab.selected = app
        .tab
        .entries
        .iter()
        .position(|e| e.path.ends_with("sub"))
        .unwrap();
    app.tree_descend().unwrap();
    assert_eq!(
        app.tab.root.file_name().unwrap(),
        "sub",
        "ディレクトリはツリーの新しい root として潜行する(プレビューは開かない)"
    );
    assert_eq!(app.tab.mode, Mode::Tree, "モードは Tree のまま");

    std::fs::remove_dir_all(&dir).ok();
}

/// Builds a throwaway jj workspace with no colocated `.git`. None when this machine has no jj —
/// konoma falls back to git there, so the suite has to stay green without it.
#[cfg(feature = "git")]
fn jj_scratch(name: &str) -> Option<PathBuf> {
    if !crate::vcs::jj::available() {
        return None;
    }
    let dir = unique_tmp(name);
    std::fs::create_dir_all(&dir).ok()?;
    let jj = |args: &[&str]| {
        std::process::Command::new("jj")
            .current_dir(&dir)
            .env("HOME", &dir) // never touch the running machine's own jj config
            .env("JJ_USER", "konoma test")
            .env("JJ_EMAIL", "test@example.invalid")
            .args(args)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    };
    if !jj(&["git", "init", "--no-colocate", "."]) {
        return None;
    }
    std::fs::write(dir.join("a.txt"), b"one\n").ok()?;
    if !jj(&["commit", "-m", "seed"]) {
        return None;
    }
    std::fs::write(dir.join("a.txt"), b"two\n").ok()?;
    Some(dir)
}

/// `[ui] confirm_jj_sync` decides whether `R` asks first. konoma writes to a jj repository only
/// here, so the default has to be the one that asks.
#[cfg(feature = "git")]
#[test]
fn ui_confirm_jj_sync_gates_the_only_write() {
    let Some(dir) = jj_scratch("konoma_confirm_jj_sync") else {
        return;
    };

    let mut on = Config::default();
    assert!(
        on.ui.confirm_jj_sync,
        "the setting that guards konoma's only write must default to asking"
    );
    on.ui.confirm_jj_sync = true;
    let mut app = App::new(dir.clone(), on).unwrap();
    app.jj_start_sync();
    assert!(
        app.dialog.is_some(),
        "with the confirmation on, R must ask before letting jj snapshot"
    );

    let mut off = Config::default();
    off.ui.confirm_jj_sync = false;
    let mut app = App::new(dir.clone(), off).unwrap();
    app.jj_start_sync();
    assert!(
        app.dialog.is_none(),
        "with the confirmation off, R must act without asking"
    );
    assert!(
        app.flash.is_some(),
        "acting without asking still has to report what happened"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// The jj backend never shells out to `git` — `src/vcs/jj.rs` only ever runs the `jj` executable —
/// so a machine with no `git` executable at all must not block `o` on a jj workspace. Before the fix,
/// `open_git_view` probed `git_binary_available()` before it had even decided which backend would
/// answer, so this refused with `GitNotInstalled` on a directory the tree was already rendering jj's
/// chip and change markers for (see `open_git_view_reports_a_missing_git_binary_distinctly` for the
/// matching git-repo case, which this must not regress).
#[cfg(feature = "git")]
#[test]
fn jj_hub_opens_even_when_the_git_binary_is_missing() {
    let Some(dir) = jj_scratch("konoma_jj_hub_no_git_binary") else {
        return;
    };
    crate::git::set_git_binary_available_for_test(Some(false));
    let mut app = App::new(dir.clone(), Config::default()).unwrap(); // external.git = true (default)
    assert_eq!(
        crate::vcs::detect(&app.tab.root),
        crate::vcs::VcsKind::Jj,
        "sanity: the fixture must resolve to the jj backend, or this test proves nothing"
    );

    app.open_git_view();
    assert!(
        app.is_git_view(),
        "the jj hub must open even though this machine has no git executable"
    );
    assert_ne!(
        app.flash.as_deref(),
        Some(tr(app.lang, crate::i18n::Msg::GitNotInstalled)),
        "the jj backend never touches the git executable, so this reason must never fire for it"
    );

    crate::git::set_git_binary_available_for_test(None);
    std::fs::remove_dir_all(&dir).ok();
}

/// The matching case for `[external] git = false`: turning git integration off must not close the jj
/// hub either — jj never asks that setting. This is also where the tree/hub inconsistency the bug
/// report described gets pinned directly: the tree's markers (`crate::vcs::statuses`, driving the
/// `M a.txt`-style rows and the `@ ...` chip) already ignore `[external] git` for the jj backend, so
/// the hub refusing to open on the same directory was the two views disagreeing about the same repo.
#[cfg(feature = "git")]
#[test]
fn jj_hub_opens_even_when_the_git_integration_is_switched_off() {
    let Some(dir) = jj_scratch("konoma_jj_hub_git_disabled") else {
        return;
    };
    let mut cfg = Config::default();
    cfg.external.git = false;
    let mut app = App::new(dir.clone(), cfg).unwrap();
    assert_eq!(
        crate::vcs::detect(&app.tab.root),
        crate::vcs::VcsKind::Jj,
        "sanity: the fixture must resolve to the jj backend, or this test proves nothing"
    );

    // The tree side of the same inconsistency: jj's status markers don't consult `[external] git`,
    // so the tree already shows this workspace's uncommitted change regardless of the setting.
    assert!(
        !crate::vcs::statuses(&app.tab.root).is_empty(),
        "sanity: the tree's markers come from jj regardless of [external] git"
    );

    app.open_git_view();
    assert!(
        app.is_git_view(),
        "external.git=false must not block the jj hub — the tree and the hub must agree on this repo"
    );
    assert_ne!(
        app.flash.as_deref(),
        Some(tr(app.lang, crate::i18n::Msg::ExternalGitDisabled)),
        "jj never asks [external] git, so this reason must never fire for it"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// Moving to a different repository has to settle **which backend answers** right away, not when
/// the background scan lands. `git_vcs` is not just a label: `!` reads it to choose between lazygit
/// and lazyjj, and the hub reads it to decide whether it is listing branches or bookmarks. Leaving
/// the previous repository's answer in place hands whoever presses a key in that window the wrong
/// tool for the repository they are looking at.
#[cfg(feature = "git")]
#[test]
fn moving_to_another_repository_settles_the_backend_before_the_scan_lands() {
    let root = unique_tmp("konoma_git_vcs_switch");
    let _ = std::fs::remove_dir_all(&root);
    let from = root.join("from");
    let to = root.join("to");
    std::fs::create_dir_all(&from).unwrap();
    std::fs::create_dir_all(&to).unwrap();
    init_git_repo(&from);
    init_git_repo(&to);
    let from = from.canonicalize().unwrap();
    let to = to.canonicalize().unwrap();

    let mut app = App::new(from, Config::default()).unwrap();
    // Nothing drains the channel, so the scan's answer never arrives — the window it leaves open is
    // exactly what this test is about.
    let (tx, _rx) = std::sync::mpsc::channel();
    app.attach_status_loader(tx);
    app.refresh_git_if_needed();

    // Stand in for having arrived from a jj repository.
    app.git_vcs = crate::vcs::VcsKind::Jj;
    app.tab.root = to;
    app.refresh_git_if_needed();
    assert!(
        app.git_status_pending.is_some(),
        "the scan has to still be in flight, or this proves nothing"
    );
    assert_eq!(
        app.git_vcs,
        crate::vcs::VcsKind::Git,
        "a git repository must not keep answering as jj until the scan lands"
    );

    std::fs::remove_dir_all(&root).ok();
}

/// The same window, the other way round: arriving in a jj workspace has to read as jj immediately,
/// or `!` launches lazygit for a repository lazygit cannot see.
#[cfg(feature = "git")]
#[test]
fn arriving_in_a_jj_workspace_reads_as_jj_before_the_scan_lands() {
    let Some(jj_dir) = jj_scratch("konoma_git_vcs_to_jj") else {
        return;
    };
    let git_dir = unique_tmp("konoma_git_vcs_to_jj_git");
    let _ = std::fs::remove_dir_all(&git_dir);
    std::fs::create_dir_all(&git_dir).unwrap();
    init_git_repo(&git_dir);
    let git_dir = git_dir.canonicalize().unwrap();
    let jj_dir = jj_dir.canonicalize().unwrap();

    let mut app = App::new(git_dir.clone(), Config::default()).unwrap();
    let (tx, _rx) = std::sync::mpsc::channel();
    app.attach_status_loader(tx);
    app.refresh_git_if_needed();

    app.tab.root = jj_dir.clone();
    app.refresh_git_if_needed();
    assert!(
        app.git_status_pending.is_some(),
        "the scan has to still be in flight, or this proves nothing"
    );
    assert_eq!(
        app.git_vcs,
        crate::vcs::VcsKind::Jj,
        "a jj workspace must not keep answering as git until the scan lands"
    );

    std::fs::remove_dir_all(&jj_dir).ok();
    std::fs::remove_dir_all(&git_dir).ok();
}

// =============================================================================
// jj write-safety audit (2026-08): the read-only gate itself
// (`Action::writes_repository()` + `dispatch_action`'s check, `src/main.rs`), which had no test
// coverage of its own before this. The key-driven walk through every reachable write action lives
// in `src/e2e_tests.rs` (needs `Sim`); what stays here needs either private `PerTab` field access
// (only visible inside `app` and its submodules) or is a pure source/logic check unrelated to keys.
// =============================================================================

/// `WorktreeCreate` cannot be reached through real navigation in a jj repository: `w` from the
/// changes hub redirects to the `JjWorkspacesUnlisted` flash before `Surface::GitWorktrees` is ever
/// entered (`open_git_worktrees`, `src/app/git_view.rs`) — see
/// `e2e_jj_worktrees_key_flashes_instead_of_opening_the_git_list` (`src/e2e_tests.rs`). That
/// unreachability is exactly why this needs its own test: the gate in `dispatch_action` must hold
/// even if the surface is somehow reached anyway — a future regression in `open_git_worktrees`'s own
/// redirect, or stale per-tab state. Forcing `tab.git_worktrees` open directly (rather than pressing
/// `w`) is what lets this test exist despite the surface being unreachable through keys; it needs
/// `PerTab`'s private fields, which is why it lives here rather than in `e2e_tests.rs`.
#[cfg(feature = "git")]
#[test]
fn worktree_create_is_gated_even_if_the_normally_unreachable_surface_is_forced_open() {
    let Some(dir) = jj_scratch("konoma_worktree_create_gate") else {
        return;
    };
    let mut app = App::new(dir.clone(), Config::default()).unwrap();
    assert!(
        !crate::vcs::caps(&app.tab.root).write,
        "sanity: the fixture must resolve to the read-only jj backend, or this test proves nothing"
    );
    app.tab.git_worktrees = Some(Vec::new()); // force the surface open; `w` cannot reach it in jj
    app.tab.git_worktree_sel = 0;
    assert_eq!(
        app.surface(),
        crate::keymap::Surface::GitWorktrees,
        "sanity: the forced state must read as the worktree list"
    );

    let res = crate::handle_key(
        &mut app,
        crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char('n'),
            crossterm::event::KeyModifiers::NONE,
        ),
    );
    assert!(
        res.is_ok(),
        "the gate must return Ok(false), not error: {res:?}"
    );
    assert!(
        app.flash
            .as_deref()
            .is_some_and(|f| f.contains(crate::i18n::tr(app.lang, crate::i18n::Msg::VcsReadOnly))),
        "expected the VcsReadOnly flash, got {:?}",
        app.flash
    );
    assert!(
        !app.is_dialog(),
        "n must not have opened the new-worktree input dialog"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// The read-only gate in `dispatch_action` only protects a write if it is reached through an
/// `Action` — a git write that bypasses `run_git_op` (the one place every `crate::git::` write call
/// lives, `src/app/git_view.rs`) would bypass the gate entirely, no matter what
/// `Action::writes_repository()` says. This scans the actual source of `git_view.rs` at compile time
/// (`include_str!`) and confirms every one of the write API's call sites is still exactly the one
/// inside `run_git_op`.
///
/// What this catches: a write call added, or moved, to somewhere else in this file, outside
/// `run_git_op` — the concrete way today's single gate could stop covering a write that already
/// exists. What this does **not** catch: a brand new write-shaped `Action` that calls a brand new
/// `crate::git::` function correctly routed *through* `run_git_op`, but never added to
/// `writes_repository()` in the first place — see `writes_repository_set_is_pinned...` below for
/// that (still incomplete) half of the picture.
#[cfg(feature = "git")]
#[test]
fn git_write_api_calls_stay_confined_to_run_git_op() {
    let src = include_str!("git_view.rs");
    let start = src.find("fn run_git_op(").expect(
        "run_git_op not found — did the write dispatch move or get renamed? update this test's search string",
    );
    let after_start = &src[start..];
    // Every method in this file is `    fn `/`    pub fn ` at the same 4-space indent as
    // `run_git_op` itself (optionally preceded by its own doc comment) — the next such line marks
    // where this function's body ends.
    let next_fn = after_start[1..].find("\n    fn ");
    let next_pub_fn = after_start[1..].find("\n    pub fn ");
    let rel_end = match (next_fn, next_pub_fn) {
        (Some(a), Some(b)) => a.min(b),
        (Some(a), None) | (None, Some(a)) => a,
        (None, None) => panic!("could not find the end of run_git_op — did the next method move?"),
    };
    let body = &after_start[..rel_end + 1];

    // Exactly what `run_git_op`'s match calls today (mirrors `GitOpKind`, `src/app.rs`).
    let write_calls = [
        "crate::git::stage(",
        "crate::git::unstage(",
        "crate::git::stage_all(",
        "crate::git::unstage_all(",
        "crate::git::discard(",
        "crate::git::commit(",
        "crate::git::checkout(",
        "crate::git::create_branch(",
        "crate::git::delete_branch(",
        "crate::git::worktree_add(",
    ];
    for needle in write_calls {
        let total = src.matches(needle).count();
        assert_eq!(
            total, 1,
            "{needle} must appear exactly once in git_view.rs (inside run_git_op). More than once \
             means a write path was added outside run_git_op — route it through \
             start_git_op/run_git_op like every other write here, so dispatch_action's \
             writes_repository() gate (src/main.rs) still covers it. Zero means this needle itself \
             needs updating (the function was renamed, removed, or its call site changed shape)."
        );
        let inside = body.matches(needle).count();
        assert_eq!(
            inside, 1,
            "{needle} exists in git_view.rs but not inside run_git_op — it moved outside the gate's \
             one known entry point. Every git write must go through start_git_op/run_git_op."
        );
    }
}

/// `writes_repository()` (`src/keymap.rs`) is a hand-written list — the only thing that decides
/// which `Action`s `dispatch_action`'s read-only gate refuses in a jj repository. Pinning its answer
/// here means any change to the SET it returns true for breaks this test and forces a human to look.
///
/// If you just **added** a new write-shaped Action and correctly included it in
/// `writes_repository()`, this test fails with a diff — add the new variant to both `candidates` and
/// `want` below, AND add a real key press for it to
/// `e2e_jj_write_actions_are_gated_by_the_read_only_backend` (`src/e2e_tests.rs`), so the new action
/// is actually exercised against jj rather than merely declared correctly.
///
/// What this test **cannot** catch: a new write-shaped Action that was never added to
/// `writes_repository()` at all. `Action` has no enumerable list of its own variants (unlike
/// `i18n::Msg`'s `ALL_MSGS`, which `all_msg_variants_are_covered_by_all_msgs` checks against the
/// enum source directly) — there is no way, from outside `src/keymap.rs`, to ask "what are all the
/// Action variants", and therefore no way to mechanically prove none of them was left out of this
/// list. That is a real, standing gap that only a human reviewing a diff to `keymap.rs` can close;
/// the closest mechanical mitigation is `git_write_api_calls_stay_confined_to_run_git_op` (above),
/// which at least confirms no git write call exists outside the one path this gate is meant to guard.
#[cfg(feature = "git")]
#[test]
fn writes_repository_set_is_pinned_add_new_write_actions_to_e2e_tests_too() {
    use crate::keymap::{action_name, Action};
    let candidates = [
        Action::GitStage,
        Action::GitUnstage,
        Action::GitStageAll,
        Action::GitUnstageAll,
        Action::GitDiscard,
        Action::GitDiffDiscard,
        Action::GitCommit,
        Action::BranchCheckout,
        Action::BranchCreate,
        Action::BranchDelete,
        Action::WorktreeCreate,
    ];
    let mut got: Vec<String> = candidates
        .into_iter()
        .filter(|a| a.writes_repository())
        .map(action_name)
        .collect();
    got.sort();

    let mut want: Vec<String> = [
        "branch_checkout",
        "branch_create",
        "branch_delete",
        "git_commit",
        "git_diff_discard",
        "git_discard",
        "git_stage",
        "git_stage_all",
        "git_unstage",
        "git_unstage_all",
        "worktree_create",
    ]
    .into_iter()
    .map(String::from)
    .collect();
    want.sort();

    assert_eq!(
        got, want,
        "writes_repository()'s answer for the known write actions changed. If you removed one, the \
         jj gate just got weaker — put it back unless that was deliberate. If you added a brand new \
         write action, this candidate list (and the pinned `want` list) also needs the new variant, \
         AND it needs a real key press in e2e_jj_write_actions_are_gated_by_the_read_only_backend \
         (src/e2e_tests.rs) — this test alone cannot see Action variants it was never told about."
    );

    // Sanity: some read/navigation Actions must stay ungated, or the pinned lists above could
    // vacuously agree even if writes_repository() had degenerated into "gate everything".
    for (name, a) in [
        ("jj_sync", Action::JjSync),
        ("git_open_worktrees", Action::GitOpenWorktrees),
        ("git_open_branches", Action::GitOpenBranches),
        ("git_close", Action::GitClose),
        ("git_graph_toggle_all", Action::GitGraphToggleAll),
    ] {
        assert!(
            !a.writes_repository(),
            "{name} is a read/navigation action and must not be gated as a write"
        );
    }
}
