//! E2E-style behavior simulations (NOT coverage-oriented unit tests).
//!
//! Each test replays a real user session: keystrokes go through `handle_key` (the same entry
//! point as the run loop) and every key is followed by a full `ui::render` draw on a
//! `TestBackend`, so state that is built at render time (markdown items, git rows, viewport
//! sizes, clamps) participates exactly as it does live. Assertions read both the App state and
//! the drawn screen text.
//!
//! Policy (user-directed, 2026-07-08): tests are written against the **documented expected
//! behavior** (help/KEYMAP/docs). Where reality disagrees, the test is kept, marked
//! `#[ignore = "FINDING-.."]`, and the mismatch is reported in the findings list instead of
//! being silently "fixed" to match the bug.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

use crate::app::{App, Mode};
use crate::config::Config;
use crate::{handle_key, ui};

/// One simulated konoma session: an App plus a terminal, with a draw after every key
/// (mirroring the run loop's key→redraw cycle).
struct Sim {
    app: App,
    term: Terminal<TestBackend>,
    /// Set when a key handler requested quit (like the run loop breaking).
    quit: bool,
}

impl Sim {
    fn new(root: &std::path::Path) -> Sim {
        Sim::with_config(root, Config::default())
    }

    fn with_config(root: &std::path::Path, cfg: Config) -> Sim {
        let app = App::new(root.to_path_buf(), cfg).expect("App::new");
        let term = Terminal::new(TestBackend::new(90, 26)).expect("terminal");
        let mut sim = Sim {
            app,
            term,
            quit: false,
        };
        sim.draw();
        sim
    }

    fn draw(&mut self) {
        let app = &mut self.app;
        self.term.draw(|f| ui::render(f, app)).expect("draw");
    }

    fn press(&mut self, code: KeyCode, mods: KeyModifiers) {
        let res = handle_key(&mut self.app, KeyEvent::new(code, mods));
        // run ループ同様、回復可能な Err は flash 化される(resolve_key_result 相当は簡略化)。
        match res {
            Ok(true) => self.quit = true,
            Ok(false) => {}
            Err(e) => self.app.flash = Some(format!("error: {e:#}")),
        }
        self.draw();
    }

    fn key(&mut self, c: char) {
        self.press(KeyCode::Char(c), KeyModifiers::NONE);
    }

    /// Type a sequence of plain character keys.
    fn keys(&mut self, s: &str) {
        for c in s.chars() {
            self.key(c);
        }
    }

    fn ctrl(&mut self, c: char) {
        self.press(KeyCode::Char(c), KeyModifiers::CONTROL);
    }

    fn enter(&mut self) {
        self.press(KeyCode::Enter, KeyModifiers::NONE);
    }

    fn esc(&mut self) {
        self.press(KeyCode::Esc, KeyModifiers::NONE);
    }

    fn tab(&mut self) {
        self.press(KeyCode::Tab, KeyModifiers::NONE);
    }

    /// The full screen as one string (rows joined by \n).
    fn screen(&self) -> String {
        let buf = self.term.backend().buffer();
        let w = buf.area.width as usize;
        let mut out = String::new();
        for (i, cell) in buf.content().iter().enumerate() {
            out.push_str(cell.symbol());
            if (i + 1) % w == 0 {
                out.push('\n');
            }
        }
        out
    }

    #[track_caller]
    fn see(&self, needle: &str) {
        assert!(
            self.screen().contains(needle),
            "画面に「{needle}」が見えるはず:\n{}",
            self.screen()
        );
    }

    #[track_caller]
    fn dont_see(&self, needle: &str) {
        assert!(
            !self.screen().contains(needle),
            "画面に「{needle}」は見えないはず:\n{}",
            self.screen()
        );
    }

    /// Move the tree cursor onto the entry whose path ends with `suffix`.
    #[track_caller]
    fn select(&mut self, suffix: &str) {
        let i = self
            .app
            .entries
            .iter()
            .position(|e| e.path.to_string_lossy().ends_with(suffix))
            .unwrap_or_else(|| panic!("entry not found: {suffix}"));
        self.app.selected = i;
        self.draw();
    }
}

/// Fresh sandbox dir under the OS temp dir (recreated per test).
fn sandbox(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("konoma_e2e_{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Standard file set for browsing scenarios.
fn seed_files(dir: &std::path::Path) {
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("src/lib.rs"), "pub fn hi() {}\n").unwrap();
    std::fs::write(
        dir.join("readme.md"),
        "# Title\n\nhello [go](./notes.txt)\n\n- [ ] task one\n",
    )
    .unwrap();
    std::fs::write(dir.join("notes.txt"), "alpha\nbeta\ngamma\ndelta\n").unwrap();
    std::fs::write(dir.join("data.csv"), "a,b\n1,2\n3,4\n").unwrap();
    std::fs::write(dir.join(".hidden.txt"), "secret\n").unwrap();
}

fn canon(dir: &std::path::Path) -> std::path::PathBuf {
    dir.canonicalize().unwrap()
}

// =============================================================================
// ツリー: 移動・絞り込み・隠しファイル・ソート
// =============================================================================

#[test]
fn e2e_tree_navigation_and_ends() {
    let dir = sandbox("tree_nav");
    seed_files(&dir);
    let mut s = Sim::new(&canon(&dir));
    s.see("TREE");
    let first = s.app.selected;
    s.key('j');
    assert_eq!(s.app.selected, first + 1, "j で1つ下へ");
    s.key('k');
    assert_eq!(s.app.selected, first, "k で戻る");
    s.key('G');
    assert_eq!(s.app.selected, s.app.entries.len() - 1, "G で末尾");
    s.key('g');
    assert_eq!(s.app.selected, 0, "g で先頭");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_tree_filter_narrows_and_esc_clears() {
    let dir = sandbox("tree_filter");
    seed_files(&dir);
    let mut s = Sim::new(&canon(&dir));
    let all = s.app.entries.len();
    s.key('/');
    s.keys("csv");
    s.see("data.csv");
    s.dont_see("readme.md");
    assert!(s.app.entries.len() < all, "絞り込みで件数が減る");
    s.enter(); // 確定
    s.see("data.csv");
    s.esc(); // 解除
    assert_eq!(s.app.entries.len(), all, "Esc で全件に戻る");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_tree_hidden_toggle() {
    let dir = sandbox("tree_hidden");
    seed_files(&dir);
    let mut s = Sim::new(&canon(&dir));
    s.dont_see(".hidden.txt");
    s.key('.');
    s.see(".hidden.txt");
    s.key('.');
    s.dont_see(".hidden.txt");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_tree_descend_and_parent() {
    let dir = sandbox("tree_descend");
    seed_files(&dir);
    let root = canon(&dir);
    let mut s = Sim::new(&root);
    s.select("src");
    s.key('l');
    assert!(
        s.app.root.ends_with("src"),
        "l でディレクトリへ潜る(root 変更)"
    );
    s.see("lib.rs");
    s.key('h');
    assert_eq!(s.app.root, root, "h で親へ戻る");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_sort_menu_switches_order() {
    let dir = sandbox("sort");
    seed_files(&dir);
    let mut s = Sim::new(&canon(&dir));
    s.key('s');
    s.see("sort"); // ソートメニューのフッター/表示
    s.key('s'); // サイズ順
                // メニューが閉じ、並びが変わっている(ディレクトリ先頭は維持)。
    assert!(!s.app.is_sort_menu());
    std::fs::remove_dir_all(&dir).ok();
}

// =============================================================================
// ファイル操作: 作成・リネーム・削除(取消と完全削除)
// =============================================================================

#[test]
fn e2e_file_create_via_dialog() {
    let dir = sandbox("file_create");
    seed_files(&dir);
    let mut s = Sim::new(&canon(&dir));
    // 作成先は「カーソル位置のディレクトリ」(op_base_dir)。ルート直下に作るには
    // ルート直下のファイル上にカーソルを置く(その親=ルートが作成先になる)。
    s.select("notes.txt");
    s.key(' ');
    s.key('n'); // Space→n 作成
    s.keys("fresh.txt");
    s.enter();
    assert!(
        dir.join("fresh.txt").exists(),
        "作成したファイルがディスクに存在"
    );
    s.see("fresh.txt");

    // ディレクトリにカーソルを置くと、その中に作られる(op_base_dir の設計どおり)。
    s.select("src");
    s.key(' ');
    s.key('n');
    s.keys("inner.txt");
    s.enter();
    assert!(
        dir.join("src/inner.txt").exists(),
        "ディレクトリ選択時はその中に作成される"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_file_rename_via_dialog() {
    let dir = sandbox("file_rename");
    seed_files(&dir);
    let mut s = Sim::new(&canon(&dir));
    s.select("notes.txt");
    s.key(' ');
    s.key('r');
    // 既存名がプリフィルされる想定 → 全消しして新名を入力。
    for _ in 0..30 {
        s.press(KeyCode::Backspace, KeyModifiers::NONE);
    }
    s.keys("renamed.txt");
    s.enter();
    assert!(dir.join("renamed.txt").exists(), "新名で存在");
    assert!(!dir.join("notes.txt").exists(), "旧名は消える");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_file_delete_cancel_and_permanent() {
    let dir = sandbox("file_delete");
    seed_files(&dir);
    let mut s = Sim::new(&canon(&dir));
    s.select("notes.txt");
    s.key(' ');
    s.key('d');
    // 確認ダイアログが出る → n で取消(ファイルは残る)。
    s.key('n');
    assert!(dir.join("notes.txt").exists(), "取消で残る");
    // もう一度 → `!` で完全削除(ゴミ箱を汚さない)。
    s.key(' ');
    s.key('d');
    s.key('!');
    assert!(!dir.join("notes.txt").exists(), "完全削除で消える");
    std::fs::remove_dir_all(&dir).ok();
}

// =============================================================================
// プレビュー: テキスト/Markdown/リンク/チェックボックス/CSV/検索
// =============================================================================

#[test]
fn e2e_preview_text_roundtrip() {
    let dir = sandbox("preview_text");
    seed_files(&dir);
    let mut s = Sim::new(&canon(&dir));
    s.select("notes.txt");
    s.enter();
    assert_eq!(s.app.mode, Mode::Preview);
    s.see("PREVIEW");
    s.see("alpha");
    s.key('q');
    assert_eq!(s.app.mode, Mode::Tree, "q でツリーへ戻る");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_markdown_link_follow_and_back() {
    let dir = sandbox("md_link");
    seed_files(&dir);
    let mut s = Sim::new(&canon(&dir));
    s.select("readme.md");
    s.enter();
    s.see("Title");
    s.tab(); // link "go" へフォーカス
    s.enter(); // ./notes.txt を開く
    s.see("alpha");
    assert!(
        s.app
            .preview_path
            .as_deref()
            .is_some_and(|p| p.ends_with("notes.txt")),
        "リンク先へ遷移"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_markdown_checkbox_space_toggles_file() {
    let dir = sandbox("md_checkbox");
    seed_files(&dir);
    let mut s = Sim::new(&canon(&dir));
    s.select("readme.md");
    s.enter();
    s.tab(); // link
    s.tab(); // checkbox
    s.key(' ');
    let src = std::fs::read_to_string(dir.join("readme.md")).unwrap();
    assert!(
        src.contains("- [x] task one"),
        "Space でファイルに書き戻る: {src}"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_csv_table_cell_navigation() {
    let dir = sandbox("csv_nav");
    seed_files(&dir);
    let mut s = Sim::new(&canon(&dir));
    s.select("data.csv");
    s.enter();
    s.see("TABLE");
    let r0 = s.app.table_cursor();
    s.key('j');
    s.key('l');
    let r1 = s.app.table_cursor();
    assert_ne!(r0, r1, "hjkl でセルカーソルが動く");
    s.key('q');
    assert_eq!(s.app.mode, Mode::Tree);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_preview_search_moves_between_matches() {
    let dir = sandbox("preview_search");
    let big: String = (0..300)
        .map(|i| {
            if i % 100 == 0 {
                format!("needle line {i}\n")
            } else {
                format!("filler {i}\n")
            }
        })
        .collect();
    std::fs::write(dir.join("big.txt"), &big).unwrap();
    let mut s = Sim::new(&canon(&dir));
    s.select("big.txt");
    s.enter();
    s.key('/');
    s.keys("needle");
    s.enter();
    s.see("needle line 0"); // 最初の一致位置へ
    s.key('n'); // 次の一致へ → 画面が 100 行目の一致へ移る
    s.see("needle line 100");
    s.key('N'); // 前へ戻る
    s.see("needle line 0");
    std::fs::remove_dir_all(&dir).ok();
}

// =============================================================================
// タブ・タブ一覧
// =============================================================================

#[test]
fn e2e_tabs_full_lifecycle() {
    let dir = sandbox("tabs_life");
    seed_files(&dir);
    let mut s = Sim::new(&canon(&dir));
    s.keys("tt"); // 3枚
    assert_eq!(s.app.tab_count(), 3);
    s.see("3:");
    s.key('1');
    assert_eq!(s.app.active_tab_index(), 0, "数字で直行");
    s.key(']');
    assert_eq!(s.app.active_tab_index(), 1, "] で次");
    s.key('T');
    assert!(s.app.is_tab_list());
    s.see("Tabs");
    s.key('j');
    s.enter();
    assert!(!s.app.is_tab_list(), "Enter で切替+一覧が閉じる");
    assert_eq!(s.app.active_tab_index(), 2);
    s.key('T');
    s.key('d');
    assert_eq!(s.app.tab_count(), 2, "一覧の d で選択タブを閉じる");
    s.key('q'); // 一覧を閉じる
    assert!(!s.app.is_tab_list());
    // w は既定で何もしない。
    s.key('w');
    assert_eq!(s.app.tab_count(), 2, "w は未割当");
    // ツリーの q はタブを閉じる。
    s.key('q');
    assert_eq!(s.app.tab_count(), 1, "tree の q でタブが閉じる");
    std::fs::remove_dir_all(&dir).ok();
}

// =============================================================================
// ブックマーク
// =============================================================================

#[test]
fn e2e_bookmarks_set_list_jump() {
    let root = sandbox("bm_flow");
    let proj = root.join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    seed_files(&proj);
    let proj = canon(&proj);
    let mut s = Sim::new(&proj);
    s.app.bookmarks = crate::bookmarks::Bookmarks::with_base(root.join("cfg"), &proj);
    s.select("notes.txt");
    s.keys("mb"); // b に登録
    s.see("bookmarked");
    s.key('\'');
    s.see("Bookmarks");
    s.key('b'); // 英字で直ジャンプ → ファイルはプレビュー
    assert_eq!(s.app.mode, Mode::Preview);
    assert!(s
        .app
        .preview_path
        .as_deref()
        .is_some_and(|p| p.ends_with("notes.txt")));
    // プレビュー中の m は表示中ファイルを登録。
    s.keys("mz");
    assert!(
        s.app
            .bookmarks
            .get('z')
            .is_some_and(|p| p.ends_with("notes.txt")),
        "プレビュー中の登録対象は表示中ファイル"
    );
    // 一覧を開いて Ctrl-d で選択中の1件を削除(順序に依存せず1件減ることを確認)。
    s.key('\'');
    s.see("Bookmarks");
    let before =
        s.app.bookmarks.get('b').is_some() as u8 + s.app.bookmarks.get('z').is_some() as u8;
    assert_eq!(before, 2, "削除前は b/z の2件");
    s.ctrl('d');
    let after = s.app.bookmarks.get('b').is_some() as u8 + s.app.bookmarks.get('z').is_some() as u8;
    assert_eq!(after, 1, "Ctrl-d で選択中の1件が削除される");
    std::fs::remove_dir_all(&root).ok();
}

// =============================================================================
// git: ハブ・stage/unstage・diff・変更フィルタ・フォロー
// =============================================================================

#[cfg(feature = "git")]
fn seed_repo(dir: &std::path::Path) {
    let run = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .current_dir(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {out:?}");
    };
    run(&["init", "-q", "."]);
    run(&["config", "user.email", "t@t"]);
    run(&["config", "user.name", "t"]);
    std::fs::write(dir.join("a.rs"), "fn a() {}\n").unwrap();
    std::fs::write(dir.join("b.rs"), "fn b() {}\n").unwrap();
    run(&["add", "-A"]);
    // 件名 "init" は据え置き(既存テスト不変)＋本文2段落。複数 -m は空行区切りの段落になる。
    // コミット詳細が本文の改行を保持することを E2E で確認できる。
    run(&[
        "commit",
        "-q",
        "-m",
        "init",
        "-m",
        "First body paragraph.",
        "-m",
        "Second body paragraph.",
    ]);
    // 非現在ブランチを1本(branches 一覧/グラフのパネル検証用)。同一コミットを指す。
    run(&["branch", "feature-x"]);
    // 未コミット変更を仕込む。
    std::fs::write(dir.join("a.rs"), "fn a() { let _x = 1; }\n").unwrap();
    std::fs::write(dir.join("new.txt"), "untracked\n").unwrap();
}

#[cfg(feature = "git")]
#[test]
fn e2e_git_hub_stage_and_diff() {
    let dir = sandbox("git_hub");
    seed_repo(&dir);
    let mut s = Sim::new(&canon(&dir));
    s.key('o');
    s.see("a.rs");
    s.see("new.txt");
    // stage → 表示が変わる(ステージ済みマーク)。unstage で戻る。
    s.key('s');
    s.key('u');
    // Enter で diff・q でハブへ戻る。
    s.enter();
    s.see("diff");
    s.key('q');
    assert!(s.app.is_git_view(), "diff の q でハブへ戻る");
    s.key('q');
    assert_eq!(s.app.mode, Mode::Tree);
    std::fs::remove_dir_all(&dir).ok();
}

#[cfg(feature = "git")]
#[test]
fn e2e_git_diff_from_tree_and_cycle_files() {
    let dir = sandbox("git_diff_cycle");
    seed_repo(&dir);
    let mut s = Sim::new(&canon(&dir));
    s.select("a.rs");
    s.key('d');
    s.see("diff");
    s.see("(");
    // n で次の変更ファイルへ(wrap)。
    let before = s.screen();
    s.key('n');
    assert_ne!(before, s.screen(), "n で別ファイルの diff へ");
    s.key('q');
    assert_eq!(s.app.mode, Mode::Tree);
    std::fs::remove_dir_all(&dir).ok();
}

#[cfg(feature = "git")]
#[test]
fn e2e_changed_filter_and_jumps() {
    let dir = sandbox("changed_filter");
    seed_repo(&dir);
    let mut s = Sim::new(&canon(&dir));
    s.key('C');
    s.see("CHANGED");
    assert!(s.app.changed_filter());
    // 変更2件(a.rs, new.txt)が一覧に。
    s.see("a.rs");
    s.see("new.txt");
    s.key('C');
    assert!(!s.app.changed_filter(), "C 再押下で解除");
    std::fs::remove_dir_all(&dir).ok();
}

#[cfg(feature = "git")]
#[test]
fn e2e_follow_mode_chip_toggles() {
    let dir = sandbox("follow_chip");
    seed_repo(&dir);
    let mut s = Sim::new(&canon(&dir));
    s.key('F');
    assert!(s.app.follow_enabled());
    s.see("FOLLOW");
    s.key('j'); // 任意キーで解除
    assert!(!s.app.follow_enabled());
    s.dont_see("FOLLOW");
    std::fs::remove_dir_all(&dir).ok();
}

// =============================================================================
// ヘルプ・情報・終了
// =============================================================================

#[test]
fn e2e_help_open_close() {
    let dir = sandbox("help");
    seed_files(&dir);
    let mut s = Sim::new(&canon(&dir));
    s.key('?');
    assert!(s.app.show_help);
    s.key('q');
    assert!(!s.app.show_help, "q でヘルプを閉じる");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_quit_confirm_flow() {
    let dir = sandbox("quit");
    seed_files(&dir);
    let mut s = Sim::new(&canon(&dir));
    s.key('Q');
    assert!(!s.quit, "確認ダイアログで即終了しない");
    s.key('n');
    assert!(!s.quit, "n で取消");
    s.key('Q');
    s.key('y');
    assert!(s.quit, "y で終了");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_info_popup() {
    let dir = sandbox("info");
    seed_files(&dir);
    let mut s = Sim::new(&canon(&dir));
    s.select("notes.txt");
    s.key('i');
    assert!(s.app.is_info());
    s.see("notes.txt");
    s.key('q');
    assert!(!s.app.is_info());
    std::fs::remove_dir_all(&dir).ok();
}

// =============================================================================
// 最近変更した複雑な経路を集中的にシミュレート
// (カスタムタスク状態・フォーカス往復・最後のタブ終了・グローバル BM 絶対表示・
//  ビジュアル選択・コピーメニュー・ソート反転・アンカー)
// =============================================================================

#[test]
fn e2e_markdown_task_cycles_custom_states() {
    // ui.md_task_states を [" ", "!", "x"] にすると Space は3状態を巡回し、
    // 各段階で 1 文字だけファイルへ書き戻る(v0.6.0 の可変チェックボックス)。
    let dir = sandbox("md_task_cycle");
    seed_files(&dir);
    let mut cfg = Config::default();
    cfg.ui.md_task_states = vec![" ".into(), "!".into(), "x".into()];
    let mut s = Sim::with_config(&canon(&dir), cfg);
    s.select("readme.md");
    s.enter();
    s.tab(); // link
    s.tab(); // checkbox
    let read = || std::fs::read_to_string(dir.join("readme.md")).unwrap();
    s.key(' ');
    assert!(
        read().contains("- [!] task one"),
        "1回目: [ ]→[!]: {}",
        read()
    );
    s.key(' ');
    assert!(
        read().contains("- [x] task one"),
        "2回目: [!]→[x]: {}",
        read()
    );
    s.key(' ');
    assert!(
        read().contains("- [ ] task one"),
        "3回目: [x]→[ ] へ巡回: {}",
        read()
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_markdown_focus_forward_then_back() {
    // Tab で次アイテム、Shift-Tab で前アイテムへフォーカスが往復する。
    let dir = sandbox("md_focus_back");
    seed_files(&dir);
    let mut s = Sim::new(&canon(&dir));
    s.select("readme.md");
    s.enter();
    assert_eq!(s.app.focused_item(), None, "初期はフォーカス無し");
    s.tab();
    let first = s.app.focused_item();
    assert!(first.is_some(), "Tab で最初のアイテムへ");
    s.tab();
    let second = s.app.focused_item();
    assert!(second > first, "2回目の Tab で次のアイテムへ");
    s.press(KeyCode::BackTab, KeyModifiers::SHIFT);
    assert_eq!(
        s.app.focused_item(),
        first,
        "Shift-Tab で前のアイテムへ戻る"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_last_tab_q_quits_when_confirm_off() {
    // confirm_quit=false で単一タブのとき、ツリーの q はアプリを終了する(CloseTabOrQuit)。
    let dir = sandbox("last_tab_quit");
    seed_files(&dir);
    let mut cfg = Config::default();
    cfg.ui.confirm_quit = false;
    let mut s = Sim::with_config(&canon(&dir), cfg);
    assert_eq!(s.app.tab_count(), 1);
    s.key('q');
    assert!(s.quit, "最後のタブでの q は終了する");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_last_tab_q_confirms_when_confirm_on() {
    // confirm_quit=ON(既定)なら、最後のタブの q は確認ダイアログを出しまだ終了しない。
    let dir = sandbox("last_tab_confirm");
    seed_files(&dir);
    let mut s = Sim::new(&canon(&dir));
    s.key('q');
    assert!(!s.quit, "確認ダイアログでまだ終了しない");
    assert!(s.app.is_dialog(), "終了確認ダイアログが開く");
    s.key('q'); // qq で確定終了
    assert!(s.quit, "qq で終了");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_global_bookmark_shows_absolute_path() {
    // 大文字マーク=グローバル。一覧表示は絶対パス(~ 短縮)で出る(v0.7.0)。
    let root = sandbox("bm_global");
    let proj = canon(&root);
    seed_files(&proj);
    let mut s = Sim::new(&proj);
    s.app.bookmarks = crate::bookmarks::Bookmarks::with_base(root.join("cfg"), &proj);
    s.select("notes.txt");
    s.keys("mB"); // 大文字=グローバル登録
    s.key('\'');
    let items = s.app.bookmark_list_items();
    let entry = items
        .iter()
        .find(|(_, k, _)| *k == 'B')
        .expect("global bookmark B");
    assert!(!entry.0, "大文字はグローバル(is_local=false)");
    let disp = s.app.bookmark_display_path(entry.0, &entry.2);
    assert!(
        disp.starts_with('/') || disp.starts_with('~'),
        "グローバルは絶対パス表示: {disp}"
    );
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn e2e_preview_visual_selection_and_cancel() {
    // windowed プレビューで v=charwise / V=linewise の選択が入り、Esc で解除される。
    let dir = sandbox("visual_sel");
    seed_files(&dir);
    let mut s = Sim::new(&canon(&dir));
    s.select("notes.txt");
    s.enter();
    assert!(s.app.is_windowed(), "テキストは windowed プレビュー");
    s.key('v');
    assert!(s.app.is_preview_visual(), "v で選択開始");
    assert!(!s.app.preview_visual_linewise(), "v は charwise");
    s.key('l'); // キャレットを右へ → 選択が広がる
    assert!(matches!(
        s.app.preview_selection(),
        crate::app::PreviewSelection::Char { .. }
    ));
    s.esc();
    assert!(!s.app.is_preview_visual(), "Esc で選択解除");
    // V=linewise
    s.key('V');
    assert!(
        s.app.is_preview_visual() && s.app.preview_visual_linewise(),
        "V は linewise"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_copy_leader_opens_and_cancels() {
    // y はコピーの which-key リーダーを開く。Esc で閉じる(何もコピーしない)。
    let dir = sandbox("copy_leader");
    seed_files(&dir);
    let mut s = Sim::new(&canon(&dir));
    s.select("notes.txt");
    s.key('y');
    assert_eq!(
        s.app.pending_leader,
        Some(crate::keymap::LeaderId::Copy),
        "y でコピーリーダー待ち"
    );
    s.esc();
    assert_eq!(s.app.pending_leader, None, "Esc でリーダー解除");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_sort_reverse_flips_order() {
    // s→r で並び順が反転する(ディレクトリ先頭は維持されるが、ファイル群の順が逆になる)。
    let dir = sandbox("sort_rev");
    seed_files(&dir);
    let mut s = Sim::new(&canon(&dir));
    let before: Vec<_> = s.app.entries.iter().map(|e| e.path.clone()).collect();
    s.key('s');
    s.key('r'); // reverse トグル(トグル系は複数切替できるようメニューは開いたまま=設計どおり)
    assert!(
        s.app.is_sort_menu(),
        "r/. はトグルなのでメニューは開いたまま"
    );
    let after: Vec<_> = s.app.entries.iter().map(|e| e.path.clone()).collect();
    assert_ne!(before, after, "反転で並びが変わる");
    s.esc(); // メニューを閉じる
    assert!(!s.app.is_sort_menu(), "Esc でメニューが閉じる");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_anchor_reanchor_and_reset() {
    // サブディレクトリへ潜って root を変えた後、a=現在地を基準に再アンカー、A=起動位置へ戻す。
    // どちらも flash を出しクラッシュしない(相対パス表示の基準を切替)。
    let dir = sandbox("anchor");
    seed_files(&dir);
    let root = canon(&dir);
    let mut s = Sim::new(&root);
    s.select("src");
    s.key('l'); // src へ潜行(root 変更)
    assert!(s.app.root.ends_with("src"));
    s.key('a'); // 再アンカー
    assert!(s.app.flash.is_some(), "a で通知");
    s.key('A'); // 起動位置へ
    assert!(s.app.flash.is_some(), "A で通知");
    // アンカー操作でツリー構造/root は不変(表示基準だけが変わる)。
    assert!(s.app.root.ends_with("src"), "アンカーは root を動かさない");
    std::fs::remove_dir_all(&dir).ok();
}

// =============================================================================
// git: log / グラフ / branches / コミット詳細 / 基準固定
// =============================================================================

#[cfg(feature = "git")]
#[test]
fn e2e_git_hub_to_log_shows_commit_subject() {
    // 変更ハブ(o) → log(l)。seed_repo の件名 "init" が一覧に出る。
    let dir = sandbox("git_log_view");
    seed_repo(&dir);
    let mut s = Sim::new(&canon(&dir));
    s.key('o');
    assert!(s.app.is_git_view(), "o で変更ハブが開く");
    s.key('l'); // ハブ内 l = log
    assert!(s.app.is_git_log(), "l で log ビューへ");
    s.see("Git log"); // render_log のリテラルタイトル
    s.see("init"); // コミット件名(summary)
    s.key('q'); // log の q はハブへ戻る
    assert!(s.app.is_git_view(), "log の q でハブへ戻る");
    std::fs::remove_dir_all(&dir).ok();
}

#[cfg(feature = "git")]
#[test]
fn e2e_git_graph_shows_commit_nodes() {
    // 変更ハブ(o) → グラフ(g)。角ばった罫線レンダラのノード ● が出る。
    let dir = sandbox("git_graph_view");
    seed_repo(&dir);
    let mut s = Sim::new(&canon(&dir));
    s.key('o');
    s.key('g'); // ハブ内 g = graph
    assert!(s.app.is_git_graph(), "g でグラフへ");
    s.see("Git graph"); // render_graph のリテラルタイトル
    s.see("●"); // コミット/作業ツリーのノードグリフ
    s.key('q');
    assert!(s.app.is_git_view(), "グラフの q でハブへ戻る");
    std::fs::remove_dir_all(&dir).ok();
}

#[cfg(feature = "git")]
#[test]
fn e2e_git_graph_set_and_clear_base() {
    // グラフで s=基準固定 / x=解除。タイトルに `⌖ base:` が出る/消える。
    let dir = sandbox("git_graph_base");
    seed_repo(&dir);
    let mut s = Sim::new(&canon(&dir));
    s.key('o');
    s.key('g');
    s.dont_see("⌖ base:"); // 既定 config は graph_base_branches=[] → 初期は基準なし
    s.key('G'); // 最下段=最古コミット行へ(作業ツリー行は先頭なので s 不可を回避)
    s.key('s'); // 選択コミットを基準に固定
    s.see("⌖ base:"); // タイトルに base 併記
    s.key('x'); // 基準解除
    s.dont_see("⌖ base:");
    std::fs::remove_dir_all(&dir).ok();
}

#[cfg(feature = "git")]
#[test]
fn e2e_git_branches_view_lists_branches() {
    // 変更ハブ(o) → branches(b)。ブランチ名と現在マーカー * が出る。
    let dir = sandbox("git_branches_view");
    seed_repo(&dir);
    let mut s = Sim::new(&canon(&dir));
    s.key('o');
    s.key('b'); // ハブ内 b = branches
    assert!(s.app.is_git_branches(), "b でブランチ一覧へ");
    s.see("Git branches"); // render_branches のリテラルタイトル
    s.see("feature-x"); // seed_repo が作る非現在ブランチ
    s.see("*"); // 現在ブランチのマーカー
    s.key('q');
    assert!(s.app.is_git_view(), "branches の q でハブへ戻る");
    std::fs::remove_dir_all(&dir).ok();
}

#[cfg(feature = "git")]
#[test]
fn e2e_git_commit_detail_shows_full_message() {
    // log → Enter でコミット詳細。完全メッセージ(本文の改行を保持)がヘッダに出る。
    let dir = sandbox("git_commit_detail");
    seed_repo(&dir);
    let mut s = Sim::new(&canon(&dir));
    s.key('o');
    s.key('l');
    s.enter(); // 選択コミット(init)の詳細
    assert!(s.app.is_git_detail(), "Enter で詳細へ");
    s.see("First body paragraph."); // 本文段落1(改行保持)
    s.see("Second body paragraph."); // 本文段落2
    s.key('q'); // 詳細の q は背後の log へ戻る(log は閉じられていない)
    assert!(s.app.is_git_log(), "詳細の q で log へ戻る");
    std::fs::remove_dir_all(&dir).ok();
}

#[cfg(feature = "git")]
#[test]
fn e2e_git_graph_branch_picker_toggles() {
    // グラフ内 b でブランチ表示パネルが開き、ブランチ名が並ぶ。q で取消して閉じる。
    let dir = sandbox("git_graph_picker");
    seed_repo(&dir);
    let mut s = Sim::new(&canon(&dir));
    s.key('o');
    s.key('g');
    s.key('b'); // グラフ内 b = ブランチ表示パネル
    assert!(s.app.is_git_graph_picker(), "b でパネルが開く");
    s.see("feature-x"); // パネル行にブランチ名
    s.key('q'); // 取消(git_graph_picker_cancel)
    assert!(!s.app.is_git_graph_picker(), "q でパネルを閉じる");
    assert!(s.app.is_git_graph(), "パネルを閉じるとグラフへ戻る");
    std::fs::remove_dir_all(&dir).ok();
}

// =============================================================================
// コピーの「実値」照合(クリップボード非依存=計算関数/getter で照合)
// =============================================================================

#[test]
fn e2e_copy_path_strings_from_tree() {
    // y→ のコピー各種(n/r/f/p/@)が「開いた場所(open_dir)」基準の期待文字列になることを、
    // クリップボードに依存せず copy_string_for(=copy_text)で照合する。
    // - @ (AtRef) は open_dir に **厳密相対**(起動dir名を付けない)= Claude Code の @ 構文。
    // - r (Relative) は起動dir名を先頭に付ける(タイトル表示=format_path と同一基準)。
    let dir = sandbox("copy_strings");
    seed_files(&dir);
    let mut s = Sim::new(&canon(&dir));
    s.select("notes.txt");
    let path = s.app.entries[s.app.selected].path.clone();

    assert_eq!(
        s.app.copy_string_for(crate::app::CopyKind::Name).as_deref(),
        Some("notes.txt")
    );
    assert_eq!(
        s.app.copy_string_for(crate::app::CopyKind::Full),
        Some(path.display().to_string())
    );
    assert_eq!(
        s.app.copy_string_for(crate::app::CopyKind::Parent),
        Some(path.parent().unwrap().display().to_string())
    );
    // @ 参照は起動dir名を付けない厳密相対。
    assert_eq!(
        s.app
            .copy_string_for(crate::app::CopyKind::AtRef)
            .as_deref(),
        Some("@notes.txt")
    );
    // 相対はタイトル表示(format_path)と同一で、起動dir名を先頭に付ける(@ とは別物)。
    let rel = s
        .app
        .copy_string_for(crate::app::CopyKind::Relative)
        .unwrap();
    assert_eq!(
        rel,
        s.app.format_path(&path),
        "相対コピー=タイトル表示と同一基準"
    );
    assert!(
        rel.ends_with("/notes.txt") && rel != "notes.txt" && rel != "@notes.txt",
        "相対は起動dir名を先頭に付ける(@ の厳密相対とは異なる): {rel}"
    );

    // y→@ のキー配線: コピーリーダーが開いて @ で消費される(pending_leader が戻る)。
    s.key('y');
    assert_eq!(s.app.pending_leader, Some(crate::keymap::LeaderId::Copy));
    s.key('@');
    assert_eq!(s.app.pending_leader, None, "y→@ でリーダーが消費される");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_copy_atref_is_open_dir_relative_when_descended() {
    // @ 参照は「起動(open)ディレクトリ」基準。サブディレクトリへ潜って root が変わっても、
    // open_dir は起動位置のままなので @src/lib.rs のように起動位置からの厳密相対になる。
    let dir = sandbox("atref_descend");
    seed_files(&dir);
    let root = canon(&dir);
    let mut s = Sim::new(&root);
    s.select("src");
    s.key('l'); // src へ潜行(root=src・open_dir は起動位置のまま)
    assert!(s.app.root.ends_with("src"));
    s.select("lib.rs");
    assert_eq!(
        s.app
            .copy_string_for(crate::app::CopyKind::AtRef)
            .as_deref(),
        Some("@src/lib.rs"),
        "@ 参照は起動位置からの厳密相対(現在の root ではない)"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_preview_at_ref_line_reference() {
    // プレビューの Y=@path#L 参照。キャレット行=単一 #L、v/V 選択=範囲 #L..-.. 。
    // 値はクリップボード非依存の getter (selection_ref_string=preview_selection_ref_text) で照合。
    let dir = sandbox("preview_atref");
    seed_files(&dir);
    let mut s = Sim::new(&canon(&dir));
    s.select("notes.txt");
    s.enter();
    assert!(s.app.is_windowed(), "テキストは windowed(2D キャレット)");
    // キャレットは先頭行 → #L1。
    assert_eq!(
        s.app.selection_ref_string().as_deref(),
        Some("@notes.txt#L1")
    );
    s.key('j'); // 2行目へ(windowed=行キャレット移動)
    assert_eq!(
        s.app.selection_ref_string().as_deref(),
        Some("@notes.txt#L2")
    );
    // charwise 選択で 2〜3行目を跨ぐ → 範囲参照。
    s.key('v');
    s.key('j'); // キャレットを 3 行目へ(アンカーは 2 行目)
    assert!(matches!(
        s.app.preview_selection(),
        crate::app::PreviewSelection::Char { .. }
    ));
    assert_eq!(
        s.app.selection_ref_string().as_deref(),
        Some("@notes.txt#L2-3")
    );
    // Y は参照をコピーして選択を抜ける(値はクリップボード=環境依存なので抜けたことだけ確認)。
    s.key('Y');
    assert!(!s.app.is_preview_visual(), "Y で参照コピー後に選択解除");
    std::fs::remove_dir_all(&dir).ok();
}

#[cfg(feature = "git")]
#[test]
fn e2e_git_commit_copy_values() {
    // git log で y→ のコミット情報コピー(短/完全ハッシュ・件名・全文・著者・日付)。
    // git_copy(kind) は current_commit_meta を各フィールドへ写すだけ。GitLog では
    // current_commit_meta = commit_meta(root, git_log_selected_id()) なので、同じ meta を
    // pub 経路で再現して照合する(getter 不要)。
    let dir = sandbox("git_commit_copy");
    seed_repo(&dir);
    let mut s = Sim::new(&canon(&dir));
    s.key('o'); // 変更ハブ
    s.key('l'); // → ログ(同期ロード)
    s.see("init");
    assert_eq!(s.app.surface(), crate::keymap::Surface::GitLog);

    let id = s.app.git_log_selected_id().expect("選択コミットの id");
    let meta = crate::git::commit_meta(&s.app.root, &id).expect("commit_meta");
    assert_eq!(meta.message.lines().next(), Some("init"), "Subject=件名");
    assert_eq!(meta.author, "t", "Author=user.name");
    assert_eq!(meta.short.len(), 7, "ShortHash=7桁");
    assert_eq!(meta.id.len(), 40, "FullHash=40桁");
    assert!(
        meta.id.starts_with(&meta.short),
        "短ハッシュは完全ハッシュの接頭辞"
    );
    assert!(!meta.date.is_empty(), "Date は非空");

    // y→ の配線: GitCopy リーダーが開いて t(件名)で消費される。
    s.key('y');
    assert_eq!(
        s.app.pending_leader,
        Some(crate::keymap::LeaderId::GitCopy),
        "log の y は GitCopy リーダー(パスコピーの Copy ではない)"
    );
    s.key('t');
    assert_eq!(s.app.pending_leader, None, "y→t でリーダー消費");
    std::fs::remove_dir_all(&dir).ok();
}

// =============================================================================
// フォローモードの diff 表示と n/N 回遊
// 【注記】Sim は run ループ/notify を回さないので、run ループが FS イベントで行う
//   follow_note_change(&p) → follow_jump(&p) を **直接 pub 呼び出し**で代替する。
//   FS 監視→pending_follow→dwell(1s)→jump のパイプライン(debounce/latest-wins)は
//   ここでは検証しない=フォローの「ロジック」を検証(配線/dwell は別レイヤ)。
// =============================================================================

#[cfg(feature = "git")]
#[test]
fn e2e_follow_opens_full_screen_diff() {
    // フォロー ON 中に外部変更が来たら、その差分を全画面 git diff で開く(ui.follow_view 既定 "diff")。
    let dir = sandbox("follow_diff");
    seed_repo(&dir); // a.rs=変更・new.txt=未追跡(いずれも file_diff あり)
    let mut s = Sim::new(&canon(&dir));
    s.key('F');
    assert!(s.app.follow_enabled(), "F でフォロー ON");

    let a = s.app.root.join("a.rs");
    // run ループ相当: 変更イベントを記録してからジャンプ(どちらも pub)。
    assert!(s.app.follow_note_change(&a), "変更ファイルは有効な追尾対象");
    s.app.follow_jump(&a);
    s.draw();

    assert!(
        s.app.is_git_diff_preview(),
        "追尾先は全画面 git diff で開く"
    );
    assert_eq!(s.app.mode, Mode::Preview);
    s.see("diff");
    // フォロー由来 diff の回遊対象=「セッション中に変わったファイル」。今は 1 件。
    assert_eq!(
        s.app.diff_change_position(),
        Some((1, 1)),
        "セッションに 1 ファイル → (1/1)"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[cfg(feature = "git")]
#[test]
fn e2e_follow_diff_cycles_session_files() {
    // フォロー由来の diff 内 n/N が「追尾セッション中に変わったファイルのみ」を wrap 回遊し、
    // タイトルの位置表示(diff_change_position)が変わる。
    let dir = sandbox("follow_cycle");
    seed_repo(&dir);
    let mut s = Sim::new(&canon(&dir));
    s.key('F');

    let a = s.app.root.join("a.rs");
    let nt = s.app.root.join("new.txt");
    // 2 つの変更イベントがセッションに溜まる(記録順 = 回遊順)。
    assert!(s.app.follow_note_change(&a));
    assert!(s.app.follow_note_change(&nt));
    s.app.follow_jump(&a); // まず a.rs の diff を開く
    s.draw();
    assert!(s.app.is_git_diff_preview());
    assert_eq!(
        s.app.diff_change_position(),
        Some((1, 2)),
        "a.rs = 2 件中 1 番目"
    );

    // n=次の変更ファイルの diff へ(セッション内のみ)。handle_key 経由なので follow_break が
    // follow_mode を落とすが、回遊は diff_follow_scope + follow_session で継続する。
    s.key('n');
    assert!(
        !s.app.follow_enabled(),
        "n(F 以外)で follow は解除される(Zed 流)"
    );
    assert_eq!(
        s.app.diff_change_position(),
        Some((2, 2)),
        "n で 2 番目(new.txt)へ"
    );
    s.key('N'); // 前へ(wrap)
    assert_eq!(
        s.app.diff_change_position(),
        Some((1, 2)),
        "N で 1 番目へ戻る"
    );
    std::fs::remove_dir_all(&dir).ok();
}

// =============================================================================
// Markdown 記法の描画照合 + 折返しフォーカス追従
// (グリフのみ照合可能を厳守=色/反転は screen() から取れない)
// =============================================================================

/// Display column (== screen char index, since screen() emits exactly one char per cell:
/// a wide CJK char is stored in one cell and its trailing cell resets to " ") of every
/// `target` glyph in a row string. Lets us compare table-column alignment across rows that
/// contain CJK of differing widths.
fn glyph_cols(row: &str, target: char) -> Vec<usize> {
    row.chars()
        .enumerate()
        .filter(|(_, c)| *c == target)
        .map(|(i, _)| i)
        .collect()
}

#[test]
fn e2e_md_table_renders_box_and_aligns_cjk() {
    // GFM 表が罫線グリフ(┌┬┐ │ ├┼┤ └┴┘)で描かれ、CJK 混在列でも桁が揃う。
    let dir = sandbox("md_table_grid");
    seed_files(&dir);
    std::fs::write(
        dir.join("grid.md"),
        "| Tool | 名称 |\n|------|------|\n| alpharow | 設定ファイル |\n| betarow | 日本語 |\n",
    )
    .unwrap();
    let mut s = Sim::new(&canon(&dir));
    s.select("grid.md");
    s.enter();
    // 表専用の T 字/十字グリフ(プレビュー枠は ┌┐└┘─│ のみなので、これらは表の証拠)。
    s.see("┬");
    s.see("┼");
    s.see("┴");
    s.see("├");
    s.see("┤");
    // 桁揃え: ヘッダ行とデータ2行で縦罫線 │ の表示列が一致する(CJK 幅が正しく効く証拠)。
    let scr = s.screen();
    let rows: Vec<&str> = scr.lines().collect();
    let bars = |needle: &str| -> Vec<usize> {
        let row = rows
            .iter()
            .find(|l| l.contains(needle))
            .unwrap_or_else(|| panic!("row not found: {needle}\n{scr}"));
        glyph_cols(row, '│')
    };
    let head = bars("Tool");
    let d1 = bars("alpharow");
    let d2 = bars("betarow");
    assert!(head.len() >= 4, "外枠+列区切りの │ が並ぶ: {head:?}");
    assert_eq!(head, d1, "ヘッダとデータ行1で列が揃う(CJK 桁): {scr}");
    assert_eq!(head, d2, "ヘッダとデータ行2で列が揃う(CJK 桁): {scr}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_md_table_cell_link_shows_label_hides_url() {
    // 表セル内 [label](url) は label だけ表示(URL は隠しターゲットで画面から消える)。
    // Tab でフォーカスでき、Enter でリンク先(./notes.txt)が開ける(md_items にリンクが載る)。
    let dir = sandbox("md_table_link");
    seed_files(&dir); // notes.txt("alpha\n...") を含む
    std::fs::write(
        dir.join("tbl.md"),
        "| name | doc |\n|---|---|\n| konoma | [OpenDoc](./notes.txt) |\n",
    )
    .unwrap();
    let mut s = Sim::new(&canon(&dir));
    s.select("tbl.md");
    s.enter();
    s.see("OpenDoc"); // ラベルは表示される
    s.dont_see("[OpenDoc]"); // 生の Markdown 記法は出さない
    s.dont_see("./notes.txt"); // URL は collapse_links が除去して非表示
    s.tab(); // 表内リンクへフォーカス
    assert_eq!(
        s.app.focused_item(),
        Some(0),
        "表内リンクが md_items の先頭"
    );
    s.enter(); // リンク先を開く
    assert!(
        s.app
            .preview_path
            .as_deref()
            .is_some_and(|p| p.ends_with("notes.txt")),
        "表内リンクの Enter で ./notes.txt を開く"
    );
    s.see("alpha"); // notes.txt の中身
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_md_html_details_block_rescued() {
    // tui-markdown が捨てる <details> ブロックの中身テキストを、タグを剥いで表示する。
    let dir = sandbox("md_html_details");
    seed_files(&dir);
    std::fs::write(
        dir.join("html.md"),
        "before text\n\n<details>\n<summary>Summary text</summary>\nhidden body line\n</details>\n\nafter text\n",
    )
    .unwrap();
    let mut s = Sim::new(&canon(&dir));
    s.select("html.md");
    s.enter();
    s.see("Summary text"); // <summary> の中身が残る
    s.see("hidden body line"); // <details> 本文が残る
    s.dont_see("<summary>"); // タグは剥がれている
    s.dont_see("<details>");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_md_thematic_break_becomes_rule_and_fenced_dashes_kept() {
    // `---`(thematic break)は全幅の ─ 罫線に。コードフェンス内の --- は変換されず literal。
    let dir = sandbox("md_hr");
    seed_files(&dir);
    // 見出しを置かない(H1/H2 の下線ルールと ─ が混ざるのを避ける)。
    std::fs::write(
        dir.join("hr.md"),
        "above paragraph\n\n---\n\nbelow paragraph\n\n```text\nraw --- text\n```\n",
    )
    .unwrap();
    let mut s = Sim::new(&canon(&dir));
    s.select("hr.md");
    s.enter();
    s.see("above paragraph");
    s.see("below paragraph");
    // thematic break の行: プレビュー枠内(先頭 │)で残りが全て ─ の行が存在する。
    // (枠の上下ボーダは ┌/└ 始まりなので除外される。)
    let has_rule = s.screen().lines().any(|l| {
        l.starts_with('│') && l.ends_with('│') && l.chars().filter(|&c| c == '─').count() >= 60
    });
    assert!(has_rule, "--- が全幅 ─ 罫線になる:\n{}", s.screen());
    // フェンス内の --- は literal のまま残る(コードパスの後に extras が走るので誤爆しない)。
    s.see("raw --- text");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_md_task_markers_render_ascii_or_nf_by_icons() {
    // タスク `- [ ]` / `- [x]` は、ui.icons=false なら ASCII ブラケット、true なら NF グリフ。
    let dir = sandbox("md_tasks");
    seed_files(&dir);
    std::fs::write(dir.join("tasks.md"), "- [ ] todo item\n- [x] done item\n").unwrap();
    let root = canon(&dir);

    // icons=false: ASCII "[ ]" / "[x]"。
    let mut cfg = Config::default();
    cfg.ui.icons = false;
    let mut s = Sim::with_config(&root, cfg);
    s.select("tasks.md");
    s.enter();
    s.see("[ ] todo item");
    s.see("[x] done item");

    // icons=true(既定): Nerd Font グリフ(square_o / check_square_o)、ASCII ブラケットは出ない。
    let mut s2 = Sim::new(&root); // Config::default() = icons true
    s2.select("tasks.md");
    s2.enter();
    s2.see(&crate::ui::icons::task_icon(false).to_string()); // '\u{f096}'
    s2.see(&crate::ui::icons::task_icon(true).to_string()); //  '\u{f046}'
    s2.dont_see("[ ]");
    s2.dont_see("[x]");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_md_raw_source_toggle() {
    // Markdown を R で装飾表示 ⇄ 生ソース(windowed・タイトルに "· raw source")へ切替。
    let dir = sandbox("md_raw");
    seed_files(&dir);
    std::fs::write(
        dir.join("guide.md"),
        "# Heading\n\nsome **bold** text and a [link](./notes.txt)\n",
    )
    .unwrap();
    let mut s = Sim::new(&canon(&dir));
    s.select("guide.md");
    s.enter();
    // 注: フッターは装飾表示中も "R raw source" ヒントを常時出す(トグルの案内)。
    // タイトルの raw マーカーは "· raw source"(中黒つき)なので、モード判定はこちらで。
    assert!(!s.app.is_raw_source(), "初期は装飾表示");
    s.dont_see("· raw source"); // タイトルに raw マーカーは無い(ヒントの "raw source" とは別)
    s.key('R');
    assert!(s.app.is_raw_source(), "R で raw ソース表示");
    s.see("· raw source"); // タイトルの "· raw source"(相対パス表示なので短く切れない)
    s.key('R');
    assert!(!s.app.is_raw_source(), "R で装飾表示へ戻る");
    s.dont_see("· raw source");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_md_code_block_tab_focus_and_copy() {
    // Tab がリンク→コードブロック→タスクを文書順で巡回し、コードブロックにフォーカス中は
    // `y`(他のコピー操作と同じキー)でその生ソースをコピーできる(値は clipboard 非依存の
    // getter で照合)。Enter はコードブロックでは何もしない。
    let dir = sandbox("md_code_copy");
    seed_files(&dir);
    std::fs::write(
        dir.join("snip.md"),
        "see [doc](./notes.txt)\n\n```rust\nfn main() {}\nlet y = 2;\n```\n\n- [ ] todo\n",
    )
    .unwrap();
    let mut s = Sim::new(&canon(&dir));
    s.select("snip.md");
    s.enter();
    s.see("rust"); // コードブロックの言語ヘッダ
    s.tab(); // リンク
    assert!(!s.app.md_focused_code());
    s.tab(); // コードブロック
    assert!(s.app.md_focused_code(), "2番目=コードブロックにフォーカス");
    assert_eq!(
        s.app.focused_code_text().as_deref(),
        Some("fn main() {}\nlet y = 2;"),
        "生ソースをコピー対象に"
    );
    // Enter はコードブロックでは無操作(which-key も出ず flash も出ない)。
    s.enter();
    assert!(s.app.flash.is_none(), "Enter はコードブロックで何もしない");
    // `y` でコピー(clipboard は環境依存なので flash が立つことだけ確認)。
    s.key('y');
    assert!(
        s.app.pending_leader.is_none(),
        "y はコピーリーダーを開かず直コピー"
    );
    assert!(s.app.flash.is_some(), "y でコピー通知が出る");
    s.tab(); // タスク
    assert!(!s.app.md_focused_code());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_md_wrapped_focus_follows_offscreen_item() {
    // 折返し時: 画面高さを超える段落の後ろのリンクへ Tab で移ると preview_scroll が追従する。
    // 非折返し時: 段落は1行に潰れ全て画面内=スクロール不要(both で挙動差を確認)。
    let dir = sandbox("md_wrap_focus");
    seed_files(&dir);
    // 幅88で ~34 表示行に折り返す長い段落(1論理行)+ その後ろに離れたリンク/タスク。
    let para = "lorem ".repeat(500); // 3000桁 ≒ 幅88で ~34 表示行 > viewport 22
    std::fs::write(
        dir.join("torture.md"),
        format!("{para}\n\n[ZZLINK](./notes.txt)\n\n- [ ] ZZTASK\n"),
    )
    .unwrap();
    let root = canon(&dir);

    // --- wrap=true(既定): 追従してスクロールする ---
    let mut on = Sim::new(&root);
    on.select("torture.md");
    on.enter();
    assert!(on.app.cfg.ui.wrap, "前提: 既定は折返しON");
    assert_eq!(on.app.preview_scroll, 0, "初期スクロール 0");
    on.dont_see("ZZLINK"); // 段落が画面を占有=リンクは画面外
    on.tab(); // 段落の先(表示行 ~34)のリンクへフォーカス
    assert!(
        on.app.preview_scroll > 5,
        "折返しでフォーカスに追従してスクロール: scroll={}",
        on.app.preview_scroll
    );
    on.see("ZZLINK"); // 追従後は画面内

    // --- wrap=false: 段落は1行=全て画面内なのでスクロールしない ---
    let mut cfg = Config::default();
    cfg.ui.wrap = false;
    let mut off = Sim::with_config(&root, cfg);
    off.select("torture.md");
    off.enter();
    off.see("ZZLINK"); // 段落1行=リンクは最初から画面内
    off.tab();
    assert_eq!(
        off.app.preview_scroll, 0,
        "非折返しは追従スクロール不要(scroll 0 のまま)"
    );
    off.see("ZZLINK");
    std::fs::remove_dir_all(&dir).ok();
}

// =============================================================================
// ファイル操作: コピー/カット→ペースト・一括リネーム・ビジュアル選択
// (op_base_dir = カーソル位置のディレクトリ。ディレクトリ選択中はその中／ファイルなら親)
// =============================================================================

#[test]
fn e2e_file_copy_then_paste_duplicates() {
    // Space→c で cursor 上のファイルをコピー、コピー先ディレクトリへカーソルを移して
    // Space→p で複製ができる(元は残る)。貼付先 = op_base_dir(ディレクトリ選択中はその中)。
    let dir = sandbox("copy_paste");
    std::fs::write(dir.join("orig.txt"), "hello copy\n").unwrap();
    std::fs::create_dir_all(dir.join("dest")).unwrap();
    let dir = canon(&dir);
    let mut s = Sim::new(&dir);

    s.select("orig.txt"); // コピー対象にカーソル(op_targets = カーソルのファイル)
    s.key(' ');
    s.key('c'); // Space→c = FileCopy
    s.select("dest"); // 貼付先ディレクトリ(is_dir なので op_base_dir = dest 自身)
    s.key(' ');
    s.key('p'); // Space→p = FilePaste

    assert!(
        dir.join("dest/orig.txt").exists(),
        "コピーがディレクトリ内に複製される"
    );
    assert!(dir.join("orig.txt").exists(), "コピーは元を残す");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_file_cut_then_paste_moves() {
    // Space→x で cut、別ディレクトリで Space→p → 元が消え先に出来る(移動)。
    let dir = sandbox("cut_paste");
    std::fs::write(dir.join("orig.txt"), "hello move\n").unwrap();
    std::fs::create_dir_all(dir.join("dest")).unwrap();
    let dir = canon(&dir);
    let mut s = Sim::new(&dir);

    s.select("orig.txt");
    s.key(' ');
    s.key('x'); // Space→x = FileCut
    s.select("dest");
    s.key(' ');
    s.key('p'); // Space→p = FilePaste(cut は move)

    assert!(dir.join("dest/orig.txt").exists(), "移動先に出来る");
    assert!(!dir.join("orig.txt").exists(), "移動元は消える");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_batch_rename_numbers_selection_via_preview() {
    // 複数選択(V×2)→ Space→r で一括リネーム。テンプレ {n} で連番、プレビューダイアログを
    // 挟んで y で適用 → 連番リネームされる(拡張子は自動保持)。
    let dir = sandbox("batch_rename");
    std::fs::write(dir.join("alpha.log"), "a\n").unwrap();
    std::fs::write(dir.join("beta.log"), "b\n").unwrap();
    let dir = canon(&dir);
    let mut s = Sim::new(&dir);

    // V=ToggleSelect: カーソルを選択して1つ下へ。2回で先頭2ファイルを選択。
    s.key('V');
    s.key('V');
    assert!(s.app.has_selection(), "複数選択が入っている");

    s.key(' ');
    s.key('r'); // Space→r。選択ありなので start_batch_rename(テンプレ入力ダイアログ)。
    assert!(s.app.is_dialog(), "一括リネームの入力ダイアログが開く");
    s.keys("renamed-{n}"); // {n}=連番トークン
    s.enter(); // build_rename_plan → プレビュー(旧→新)へ遷移
    assert!(s.app.dialog_is_preview(), "リネームプレビューが挟まる");
    s.see("renamed-1");
    s.key('y'); // プレビューの y=適用(dialog_preview_apply)

    assert!(dir.join("renamed-1.log").exists(), "1件目が連番リネーム");
    assert!(dir.join("renamed-2.log").exists(), "2件目が連番リネーム");
    assert!(!dir.join("alpha.log").exists(), "旧名は消える");
    assert!(!dir.join("beta.log").exists(), "旧名は消える");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_tree_visual_selection_grows_then_commits() {
    // v=範囲選択開始 → j で範囲が広がり(marked_count 増)→ v=確定で selection に取り込む。
    let dir = sandbox("tree_visual");
    seed_files(&dir);
    let mut s = Sim::new(&canon(&dir));

    s.key('v'); // EnterVisual(Surface=Visual)
    assert!(s.app.is_visual(), "v で範囲選択開始");
    assert_eq!(s.app.marked_count(), 1, "開始直後はアンカー1行");
    s.key('j'); // Visual 面の Down → 範囲が [anchor, anchor+1] に広がる
    assert_eq!(s.app.marked_count(), 2, "j で範囲が2行に広がる");
    s.key('v'); // Visual 面の v=VisualCommit → 範囲を selection に確定して抜ける
    assert!(!s.app.is_visual(), "v で範囲確定して visual を抜ける");
    assert!(s.app.has_selection(), "確定した集合が残る");
    assert_eq!(s.app.marked_count(), 2, "確定後は選択2件");
    std::fs::remove_dir_all(&dir).ok();
}

// =============================================================================
// 設定反映: details 列 / path style 巡回 / csv_rainbow / busy インジケーター
// =============================================================================

#[test]
fn e2e_details_columns_show_size_and_modified() {
    // ui.details=["size","modified"] で各行の右端にサイズ/更新列が出る。
    // 期待値はレンダラと同じ quick_meta 経由で算出(時刻/サイズをハードコードしない)。
    let dir = sandbox("details");
    seed_files(&dir);
    let dir = canon(&dir);
    let notes = dir.join("notes.txt");
    let meta = crate::fileops::quick_meta(&notes).expect("quick_meta");
    let size_cell = crate::fileops::human_size(meta.size);
    let mod_cell = crate::fileops::format_epoch_short(meta.mtime.expect("mtime"));

    // 対照: details 無しでは日付列は出ない(偽陽性ガード)。
    let plain = Sim::new(&dir);
    plain.dont_see(&mod_cell);

    let mut cfg = Config::default();
    cfg.ui.details = vec!["size".into(), "modified".into()];
    let s = Sim::with_config(&dir, cfg);
    s.see(&size_cell); // 例: "23 B"(右寄せ列に現れる)
    s.see(&mod_cell); // 例: "2026-07-08 14:23"(16桁の更新時刻列)
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_path_style_cycles() {
    // p=CyclePathStyle で Relative→Home→Full→Relative と巡回。上部の path: 表示も追随。
    use crate::app::PathStyle;
    let dir = sandbox("path_style");
    seed_files(&dir);
    let mut s = Sim::new(&canon(&dir));

    assert_eq!(s.app.path_style, PathStyle::Relative, "既定は relative");
    s.see("path:rel"); // status の path ラベル(英語ロケール前提=既存テストと同一)
    s.key('p');
    assert_eq!(s.app.path_style, PathStyle::Home, "p で ~ 表示へ");
    s.see("path:~");
    s.key('p');
    assert_eq!(s.app.path_style, PathStyle::Full, "p で絶対へ");
    s.see("path:abs");
    s.key('p');
    assert_eq!(s.app.path_style, PathStyle::Relative, "巡回して先頭へ戻る");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_csv_table_renders_and_navigates_without_rainbow() {
    // ui.csv_rainbow=false でも整列テーブルとしてセル内容は出る(色は検証不可=グリフのみ)。
    // hjkl/0/$ でセルカーソルが動く(既存テストと別の観点=列端ジャンプまで見る)。
    let dir = sandbox("csv_norainbow");
    std::fs::write(
        dir.join("fruit.csv"),
        "fruit,color\napple,red\nkiwi,green\n",
    )
    .unwrap();
    let mut cfg = Config::default();
    cfg.ui.csv_rainbow = false;
    let mut s = Sim::with_config(&canon(&dir), cfg);

    s.select("fruit.csv");
    s.enter();
    s.see("TABLE");
    s.see("fruit"); // ヘッダ(モノクロでもグリフは出る)
    s.see("apple"); // データセル
    assert_eq!(s.app.table_cursor(), (0, 0), "初期カーソルは先頭セル");
    s.key('l'); // 右の列へ
    assert_eq!(s.app.table_cursor(), (0, 1), "l で1列右へ");
    s.key('l'); // 末列でクランプ(ncols=2)
    assert_eq!(s.app.table_cursor(), (0, 1), "末列でクランプ");
    s.key('0'); // 先頭列へ
    assert_eq!(s.app.table_cursor(), (0, 0), "0 で先頭列");
    s.key('$'); // 末列へ
    assert_eq!(s.app.table_cursor(), (0, 1), "$ で末列");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_tabbar_overflow_markers_keep_active_visible() {
    // タブを多数開いて狭い端末幅(90)で溢れさせると、あふれマーカー ‹n / n› が出て
    // アクティブタブは可視のまま(visible_range のアクティブ中心可視窓)。
    let dir = sandbox("tabbar_overflow");
    seed_files(&dir);
    let mut s = Sim::new(&canon(&dir));

    s.keys("tttttttt"); // t×8 → 計9タブ(各 t で新タブへ切替)
    assert_eq!(s.app.tab_count(), 9);
    s.key('5'); // タブ5(index4=中央)へ。両側に隠れタブができる。
    assert_eq!(s.app.active_tab_index(), 4);

    s.see("‹"); // 左あふれマーカー(s>0)
    s.see("›"); // 右あふれマーカー(e<n)
    let label = s.app.tab_label(4);
    s.see(&format!("5:{label}")); // アクティブタブのチップは可視のまま
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_busy_indicator_absent_when_idle() {
    // アイドル(裏で走るジョブが無い)なら busy 表示は出ない。config で無効化すれば常に非アクティブ。
    // 注: 同期ハーネスでは実際の非同期ジョブを起こせないため、idle 不変条件のみ検証(正直な範囲)。
    let dir = sandbox("busy_idle");
    seed_files(&dir);
    let dir = canon(&dir);

    let s = Sim::new(&dir);
    assert!(s.app.busy_jobs().is_empty(), "アイドルでジョブ無し");
    assert!(
        !s.app.busy_indicator_active(),
        "アイドルでインジケーター非アクティブ"
    );
    s.dont_see("loading media");
    s.dont_see("git scan");
    s.dont_see("loading images");

    let mut cfg = Config::default();
    cfg.ui.busy_indicator = false;
    let s2 = Sim::with_config(&dir, cfg);
    assert!(
        !s2.app.busy_indicator_active(),
        "busy_indicator=false は常に非アクティブ"
    );
    std::fs::remove_dir_all(&dir).ok();
}
