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

/// A config with `code_bg = "none"` — the real setting that broke the first bg-based code
/// detection (inline code / code blocks then have no background, only `fg(White)` / the `▎` gutter).
/// Tests must exercise this so a bg-only skip can never regress unnoticed again.
#[cfg(test)]
fn cfg_code_bg_none() -> Config {
    let mut cfg = Config::default();
    cfg.ui.theme.code_bg = "none".into();
    cfg
}

/// Write `name.md` with `body` in a fresh sandbox, open it in a Sim under `cfg` (tree → preview),
/// and return the Sim (at the Markdown preview) plus the sandbox dir. Runs the full render pipeline,
/// so `md_link_targets()` and the drawn screen reflect autolink/emoji/alerts exactly as live.
#[cfg(test)]
fn md_preview(cfg: Config, name: &str, body: &str) -> (Sim, std::path::PathBuf) {
    let dir = sandbox(name);
    std::fs::write(dir.join(format!("{name}.md")), body).unwrap();
    let root = canon(&dir);
    let mut s = Sim::with_config(&root, cfg);
    s.select(&format!("{name}.md"));
    s.enter();
    (s, dir)
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
fn e2e_file_duplicate_in_place() {
    let dir = sandbox("file_duplicate");
    seed_files(&dir);
    let mut s = Sim::new(&canon(&dir));
    // notes.txt にカーソル → Space→D で複製 → notes copy.txt がその場にできる。
    s.select("notes.txt");
    s.key(' ');
    s.key('D');
    assert!(
        dir.join("notes copy.txt").exists(),
        "複製 notes copy.txt がディスクに存在"
    );
    assert_eq!(
        std::fs::read(dir.join("notes copy.txt")).unwrap(),
        std::fs::read(dir.join("notes.txt")).unwrap(),
        "複製の内容が元と同一"
    );
    s.see("notes copy.txt"); // ツリーに現れ(refresh+reveal)、選択される
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

/// 装飾 Markdown の検索を **キー入力経由**で通す。装飾表示のまま(raw に切替えず)一致行へ
/// スクロールし、n で次の一致へ動くこと。CJK 本文でも壊れないこと。
#[test]
fn e2e_decorated_markdown_search() {
    let dir = sandbox("md_search");
    let mut src = String::from("# Guide\n\n");
    for i in 0..30 {
        src.push_str(&format!("padding paragraph {i}\n\n"));
    }
    src.push_str("the **haystack** contains a needle here\n\n");
    for i in 30..60 {
        src.push_str(&format!("padding paragraph {i}\n\n"));
    }
    src.push_str("日本語の行にも needle があります\n");
    std::fs::write(dir.join("guide.md"), &src).unwrap();

    let mut s = Sim::new(&canon(&dir));
    s.select("guide.md");
    s.enter();
    s.key('/');
    s.keys("needle");
    s.enter();
    assert_eq!(
        s.app.search_status(),
        Some((1, 2)),
        "装飾 md で 2 件見つかる"
    );
    assert!(!s.app.is_raw_source(), "装飾表示のまま検索できる");
    s.see("needle");

    s.key('n');
    assert_eq!(s.app.search_status(), Some((2, 2)), "n で 2 件目へ");
    s.see("needle"); // CJK 行の一致も画面に出る

    s.key('q');
    assert_eq!(s.app.mode, Mode::Tree);
    std::fs::remove_dir_all(&dir).ok();
}

/// 表内検索を **キー入力経由**で通す(`/` → クエリ → Enter → n/N)。単体テストは App の
/// メソッドを直接叩くので、keymap の bind 漏れ・入力面の横取りはここでしか捕まらない。
#[test]
fn e2e_table_search_jumps_to_matching_cell() {
    let dir = sandbox("table_search");
    std::fs::write(
        dir.join("data.csv"),
        "name,city\nalice,kyoto\nbob,osaka\ncarol,kyoto\n",
    )
    .unwrap();
    let mut s = Sim::new(&canon(&dir));
    s.select("data.csv");
    s.enter();
    s.see("TABLE");
    assert_eq!(s.app.table_cursor(), (0, 0));

    s.key('/');
    s.keys("kyoto");
    s.enter();
    assert_eq!(s.app.table_cursor(), (0, 1), "最初の一致セルへ");
    assert_eq!(s.app.search_status(), Some((1, 2)), "2 件中 1 件目");

    s.key('n');
    assert_eq!(s.app.table_cursor(), (2, 1), "n で次の一致セルへ");
    s.key('n');
    assert_eq!(s.app.table_cursor(), (0, 1), "wrap して先頭へ");

    // Esc は「検索解除 → もう一度でツリーへ」(text プレビューと同じ流儀)。
    // 実機で Esc が無反応=強調が消せない不具合を見つけたのでここで固定する。
    s.esc();
    assert_eq!(s.app.preview_search_query(), None, "Esc で検索が解除される");
    assert!(!s.app.table_cell_is_hit(0, 1), "一致の強調も消える");
    assert_eq!(s.app.mode, Mode::Preview, "1回目の Esc では表に留まる");
    s.esc();
    assert_eq!(s.app.mode, Mode::Tree, "2回目の Esc でツリーへ戻る");
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

#[test]
fn e2e_ctrl_t_opens_selected_in_new_tab() {
    // ツリーで Ctrl-t=カーソル下を別タブで開く。ファイル→プレビュー / ディレクトリ→新タブの root。
    // 元タブは維持される。
    let dir = sandbox("newtab_open");
    std::fs::write(dir.join("note.txt"), "HELLO_NEWTAB body\n").unwrap();
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    std::fs::write(dir.join("sub/inside.txt"), "INSIDE_MARK\n").unwrap();
    let mut s = Sim::new(&canon(&dir));

    // ファイル: Ctrl-t で新規タブにプレビュー。
    s.select("note.txt");
    assert_eq!(s.app.tab_count(), 1);
    s.ctrl('t');
    assert_eq!(s.app.tab_count(), 2, "Ctrl-t で新規タブができる");
    assert_eq!(s.app.active_tab_index(), 1, "新規タブがアクティブ");
    s.see("HELLO_NEWTAB"); // 新タブでファイルがプレビューされている

    // 元タブは維持: [ で戻るとツリーに note.txt が居る(プレビュー内容は見えない)。
    s.key('[');
    assert_eq!(s.app.active_tab_index(), 0);
    s.see("note.txt");
    s.dont_see("HELLO_NEWTAB");

    // ディレクトリ: Ctrl-t で新規タブがそのフォルダを root に。
    s.select("sub");
    s.ctrl('t');
    assert_eq!(s.app.tab_count(), 3);
    s.see("inside.txt"); // 新タブは sub の中身を表示
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

#[test]
fn e2e_bookmark_overwrite_prompts_confirm_then_applies() {
    // 既定(confirm_bookmark_overwrite=true): 使用済みキーへ別パスを登録すると確認ダイアログ。
    // n=取消で元のまま・y=上書きで新パスへ。
    let root = sandbox("bm_overwrite");
    let proj = root.join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    seed_files(&proj);
    let proj = canon(&proj);
    let mut s = Sim::new(&proj);
    s.app.bookmarks = crate::bookmarks::Bookmarks::with_base(root.join("cfg"), &proj);

    // notes.txt を 'b' に登録(未使用キー=確認無し)。
    s.select("notes.txt");
    s.keys("mb");
    s.see("bookmarked");
    let notes = s.app.bookmarks.get('b').expect("b set");

    // data.csv を同じ 'b' に登録 → 確認ダイアログ。まだ上書きしない。
    s.select("data.csv");
    s.keys("mb");
    s.see("OVERWRITE?");
    s.see("Overwrite bookmark");
    assert_eq!(
        s.app.bookmarks.get('b'),
        Some(notes.clone()),
        "確認前は元のまま"
    );

    // n=取消。
    s.key('n');
    s.see("canceled");
    assert_eq!(
        s.app.bookmarks.get('b'),
        Some(notes.clone()),
        "取消で notes.txt のまま"
    );

    // 再度出して y=上書き。
    s.select("data.csv");
    s.keys("mb");
    s.see("OVERWRITE?");
    s.key('y');
    assert!(
        s.app
            .bookmarks
            .get('b')
            .is_some_and(|p| p.ends_with("data.csv")),
        "y で data.csv へ上書き"
    );
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn e2e_bookmark_overwrite_off_applies_silently() {
    // confirm_bookmark_overwrite=false: 別パスでも確認せず即上書き。
    let root = sandbox("bm_overwrite_off");
    let proj = root.join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    seed_files(&proj);
    let proj = canon(&proj);
    let mut cfg = Config::default();
    cfg.ui.confirm_bookmark_overwrite = false;
    let mut s = Sim::with_config(&proj, cfg);
    s.app.bookmarks = crate::bookmarks::Bookmarks::with_base(root.join("cfg"), &proj);

    s.select("notes.txt");
    s.keys("mb");
    s.select("data.csv");
    s.keys("mb");
    s.dont_see("OVERWRITE?");
    assert!(
        s.app
            .bookmarks
            .get('b')
            .is_some_and(|p| p.ends_with("data.csv")),
        "confirm オフは即上書き"
    );
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
fn e2e_md_html_details_collapse_and_toggle() {
    // <details>(open 無し)は既定で折りたたみ: summary は出るが本文は隠れる。Tab で summary に
    // フォーカス→Space/Enter で展開すると本文が出る。タグは剥がれる。
    let dir = sandbox("md_html_details");
    seed_files(&dir);
    std::fs::write(
        dir.join("html.md"),
        "before text\n\n<details>\n<summary>Summary text</summary>\n\nhidden body line\n\n</details>\n\nafter text\n",
    )
    .unwrap();
    let mut s = Sim::new(&canon(&dir));
    s.select("html.md");
    s.enter();
    s.see("Summary text"); // <summary> は常に出る
    s.dont_see("hidden body line"); // 既定は折りたたみ=本文は隠れる
    s.dont_see("<summary>"); // タグは剥がれている
    s.dont_see("<details>");
    // Tab で summary にフォーカス → Space で展開。
    s.tab();
    assert_eq!(s.app.focused_item(), Some(0), "summary が Tab アイテム");
    s.key(' ');
    s.see("hidden body line"); // 展開して本文が出る
                               // 再度トグルで折りたたみ。
    s.enter();
    s.dont_see("hidden body line");
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

// =============================================================================
// Markdown web パリティ Phase 1: 裸URL自動リンク / GitHub Alerts / 絵文字
// (全経路を code_bg="none" と既定の両方で通す = 合成 span でなく本物の描画)
// =============================================================================

#[test]
fn e2e_markdown_autolink_and_emoji_full_pipeline() {
    // The full source → decorated spans → postprocess_md path. Prose URLs/emails/www become
    // focusable links and shortcodes convert; anything inside inline code or a code fence stays
    // verbatim — under BOTH code_bg settings (this is the regression the first, synthetic-span
    // unit tests missed: they never ran under code_bg="none" nor through the real render).
    let body = concat!(
        "Prose url https://prose.example here.\n\n",
        "Prose mail me@prose.example here.\n\n",
        "Prose www www.prose.example here.\n\n",
        "Prose emoji :rocket: here.\n\n",
        "Inline `https://in-code.example` and `:sparkles:` stay literal.\n\n",
        "```sh\ncurl https://in-fence.example  # :tada:\n```\n",
    );
    for (label, cfg) in [
        ("code_bg_set", Config::default()),
        ("code_bg_none", cfg_code_bg_none()),
    ] {
        let (mut s, dir) = md_preview(cfg, "autolink_emoji", body);
        // Exactly the three prose links, in document order — never the in-code / in-fence URLs.
        assert_eq!(
            s.app.md_link_targets(),
            vec![
                "https://prose.example".to_string(),
                "mailto:me@prose.example".to_string(),
                "http://www.prose.example".to_string(),
            ],
            "[{label}] only prose url/email/www linked; code URLs excluded"
        );
        // They are actually Tab-navigable (the point of autolinking).
        s.tab();
        assert_eq!(
            s.app.focused_item(),
            Some(0),
            "[{label}] Tab reaches the first link"
        );
        // Prose emoji converted; every code shortcode stays literal.
        s.see("🚀");
        s.dont_see(":rocket:");
        s.see(":sparkles:");
        s.see(":tada:");
        // In-code / in-fence URLs remain visible as plain text (just not links, asserted above).
        s.see("https://in-code.example");
        s.see("https://in-fence.example");
        std::fs::remove_dir_all(&dir).ok();
    }
}

#[test]
fn e2e_markdown_alert_callout_and_nested_code_fence() {
    // Alerts render as a labeled callout (not the raw `[!NOTE]`). The body's prose URL links, but a
    // code fence INSIDE the alert keeps its URL/shortcode verbatim (Finding 1: alert body lines are
    // bar-prefixed, so the code gutter is the 2nd span — is_code_line must scan all spans).
    let body = concat!(
        "> [!NOTE]\n",
        "> Docs https://alert-prose.example here.\n",
        "> ```sh\n",
        "> curl https://alert-fence.example # :tada:\n",
        "> ```\n",
    );
    for (label, cfg) in [
        ("code_bg_set", Config::default()),
        ("code_bg_none", cfg_code_bg_none()),
    ] {
        let (s, dir) = md_preview(cfg, "alert_code", body);
        s.see("Note"); // callout label
        s.dont_see("[!NOTE]"); // raw marker gone
        assert_eq!(
            s.app.md_link_targets(),
            vec!["https://alert-prose.example".to_string()],
            "[{label}] only the alert prose URL links, not the fenced one"
        );
        s.see(":tada:"); // fenced shortcode literal
        s.see("https://alert-fence.example"); // visible as text, not a link
        std::fs::remove_dir_all(&dir).ok();
    }
}

#[test]
fn e2e_markdown_alert_all_five_types_and_aliases() {
    // All five GitHub types plus an alias render a callout label; none leak the raw marker.
    let body = concat!(
        "> [!NOTE]\n> n\n\n",
        "> [!TIP]\n> t\n\n",
        "> [!IMPORTANT]\n> i\n\n",
        "> [!WARNING]\n> w\n\n",
        "> [!CAUTION]\n> c\n\n",
        "> [!danger] Aliased\n> d\n",
    );
    let (s, dir) = md_preview(Config::default(), "alert_types", body);
    for label in ["Note", "Tip", "Important", "Warning", "Caution"] {
        s.see(label);
    }
    s.see("Caution — Aliased"); // danger alias → Caution, with its inline title
    s.dont_see("[!");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_markdown_autolink_emoji_alerts_toggle_off() {
    // With the three toggles off: bare URL stays plain, shortcode literal, alert stays a blockquote.
    let body = concat!(
        "> [!WARNING]\n> careful\n\n",
        "Prose https://x.example :rocket: here.\n",
    );
    let mut cfg = cfg_code_bg_none();
    cfg.ui.md_autolink = false;
    cfg.ui.md_emoji = false;
    cfg.ui.md_alerts = false;
    let (s, dir) = md_preview(cfg, "md_toggles_off", body);
    assert!(s.app.md_link_targets().is_empty(), "autolink off: no links");
    s.see(":rocket:"); // emoji off: literal
    s.see("https://x.example"); // not a link
    s.see("[!WARNING]"); // alerts off: raw marker kept
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_markdown_inline_html_renders() {
    let body = "Press <kbd>Ctrl</kbd>. H<sub>2</sub>O. <del>old</del> new.\n";
    let (s, dir) = md_preview(Config::default(), "inline_html", body);
    s.see("Ctrl"); // <kbd> content, rendered as an inline-code keycap
    s.see("H₂O"); // <sub> → Unicode subscript
    s.see("new"); // surrounding text intact
    s.dont_see("<kbd>"); // raw tags gone
    s.dont_see("<sub>");
    s.dont_see("<del>");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_markdown_footnotes_render_superscript_and_section() {
    let body = "A claim.[^src] More text.\n\n[^src]: The evidence.\n";
    let (s, dir) = md_preview(Config::default(), "footnotes", body);
    s.see("¹"); // superscript reference marker
    s.dont_see("[^src]"); // raw reference and definition are gone
    s.see("The evidence."); // definition text in the footnotes section
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_markdown_footnotes_off_stays_literal() {
    let body = "A claim.[^src]\n\n[^src]: The evidence.\n";
    let mut cfg = Config::default();
    cfg.ui.md_footnotes = false;
    let (s, dir) = md_preview(cfg, "footnotes_off", body);
    s.see("[^src]"); // literal, unprocessed
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_markdown_front_matter_renders_as_metadata() {
    // Leading `---`…`---` is shown as a metadata block; the body (and its heading anchors) render
    // normally after it. Anchor jump on a heading defined below the front matter still resolves.
    let body = concat!(
        "---\n",
        "title: My Doc\n",
        "author: Me\n",
        "---\n",
        "[to section](#real-heading)\n\n",
        "# Real Heading\n\n",
        "body text\n",
    );
    let (mut s, dir) = md_preview(Config::default(), "frontmatter", body);
    s.see("title"); // metadata key
    s.see("My Doc"); // metadata value
    s.see("author");
    s.see("Real Heading"); // body heading still renders
                           // The anchor (computed from the body heading, past the front matter offset) resolves.
    assert_eq!(
        s.app.md_link_targets(),
        vec!["#real-heading".to_string()],
        "the in-page anchor link is present"
    );
    s.tab();
    s.enter();
    assert!(
        s.app.flash.is_none(),
        "anchor resolves (no 'not found' flash)"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_markdown_front_matter_off_leaves_body_intact() {
    let body = "---\ntitle: My Doc\n---\n# H\n\nbody\n";
    let mut cfg = Config::default();
    cfg.ui.md_frontmatter = false;
    let (s, dir) = md_preview(cfg, "frontmatter_off", body);
    // Recognition off: the title line and body both still render (no crash, old behavior).
    s.see("title");
    s.see("body");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_outline_lists_headings_and_jumps() {
    // `o` opens a heading outline; Enter on a heading scrolls the preview there.
    let mut body = String::from("# Top Heading\n\nintro\n\n");
    for i in 0..40 {
        body.push_str(&format!("filler line {i}\n\n"));
    }
    body.push_str("## Deep Section\n\ndeep body text\n");
    let (mut s, dir) = md_preview(Config::default(), "outline_jump", &body);
    assert_eq!(s.app.preview_scroll, 0);
    s.key('o');
    assert!(s.app.is_outline(), "o opens the outline");
    s.see("Top Heading");
    s.see("Deep Section");
    // On open the selection is the current section (Top, at scroll 0); j → the deep heading.
    assert_eq!(s.app.outline_sel(), 0);
    s.key('j');
    assert_eq!(s.app.outline_sel(), 1);
    s.enter();
    assert!(!s.app.is_outline(), "Enter closes the outline");
    assert!(s.app.preview_scroll > 0, "jumped down to the deep heading");
    s.see("Deep Section");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_outline_toggle_and_esc_close() {
    let (mut s, dir) = md_preview(
        Config::default(),
        "outline_toggle",
        "# A\n\nx\n\n## B\n\ny\n",
    );
    s.key('o');
    assert!(s.app.is_outline());
    s.key('o'); // o again closes
    assert!(!s.app.is_outline());
    s.key('o');
    assert!(s.app.is_outline());
    s.esc(); // Esc also closes
    assert!(!s.app.is_outline());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_outline_closes_on_new_tab() {
    // The outline overlay is app-global; opening a new tab (`t`, inherited from global while the
    // overlay is up) must close it, or an empty overlay would linger over the fresh tab.
    let (mut s, dir) = md_preview(
        Config::default(),
        "outline_newtab",
        "# A\n\nx\n\n## B\n\ny\n",
    );
    s.key('o');
    assert!(s.app.is_outline());
    s.key('t'); // global new tab
    assert!(!s.app.is_outline(), "a new tab closes the outline overlay");
    assert_eq!(s.app.mode, Mode::Tree, "the new tab is a fresh tree");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_outline_empty_flashes_on_plain_text() {
    // A plain-text (windowed, non-Markdown) preview has no heading outline: `o` flashes, no overlay.
    let dir = sandbox("outline_empty");
    std::fs::write(dir.join("f.txt"), "# not a markdown heading\njust text\n").unwrap();
    let mut s = Sim::new(&canon(&dir));
    s.select("f.txt");
    s.enter();
    s.key('o');
    assert!(
        !s.app.is_outline(),
        "no overlay opens for a non-Markdown preview"
    );
    assert!(s.app.flash.is_some(), "a 'no headings' flash is shown");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_markdown_anchor_jump_scrolls_to_heading() {
    // An in-page anchor `[x](#slug)` scrolls the decorated preview to the matching heading (it used
    // to flash "not supported"). Filler pushes the target well below the fold.
    let mut body = String::from("[go to target](#the-target)\n\n");
    for i in 0..40 {
        body.push_str(&format!("line {i}\n\n"));
    }
    body.push_str("## The Target\n\ndestination text\n");
    let (mut s, dir) = md_preview(Config::default(), "anchor_jump", &body);
    assert_eq!(s.app.preview_scroll, 0, "starts at the top");
    s.tab(); // focus the anchor link
    assert_eq!(s.app.focused_item(), Some(0));
    s.enter(); // jump
    assert!(
        s.app.preview_scroll > 0,
        "scrolled down toward the heading (was {})",
        s.app.preview_scroll
    );
    s.see("The Target");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_markdown_anchor_jump_unknown_slug_flashes() {
    let body = "[bad](#nope)\n\n# Real Heading\n\nbody\n";
    let (mut s, dir) = md_preview(Config::default(), "anchor_bad", body);
    s.tab();
    s.enter();
    // Unknown anchor: a "no heading" flash, and no scroll.
    assert!(
        s.screen().contains("nope") || s.app.flash.is_some(),
        "unknown anchor flashes"
    );
    assert_eq!(s.app.preview_scroll, 0, "no scroll for a missing anchor");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_markdown_autolink_cjk_and_trailing_punctuation() {
    // Byte-boundary safety: a bare URL right after CJK, and trailing sentence punctuation trimmed.
    let body = "見るhttps://cjk.example、と (https://paren.example) を確認。\n";
    let (s, dir) = md_preview(cfg_code_bg_none(), "autolink_cjk", body);
    assert_eq!(
        s.app.md_link_targets(),
        vec![
            "https://cjk.example".to_string(),
            "https://paren.example".to_string(),
        ],
        "CJK boundary + paren-wrapped URL both link, punctuation trimmed"
    );
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
fn e2e_markdown_edit_opens_near_scroll_position() {
    // 装飾 Markdown をスクロールして `e` を押すと、エディタは現在の表示位置に対応する近似
    // ソース行で開く(reflow のため厳密でなく近似)。先頭では 1 行目・下へスクロールで前進。
    // Sim は毎キー後に UI を描画するので md_view_rows(折返し総行数)が実使用同様に設定される。
    let dir = sandbox("md_edit_line");
    let mut content = String::from("# Top\n\n");
    for i in 1..=25 {
        content.push_str(&format!(
            "Intro prose line {i:02} with several words to read on it here.\n"
        ));
    }
    content.push_str("\n## Section Two\n\n");
    for i in 26..=50 {
        content.push_str(&format!(
            "Section prose line {i:02} with several words to read on it here.\n"
        ));
    }
    std::fs::write(dir.join("doc.md"), content).unwrap();
    let mut s = Sim::new(&canon(&dir));
    s.select("doc.md");
    s.enter(); // 装飾 Markdown プレビュー
    assert!(!s.app.is_windowed(), "装飾 md は非 windowed");

    // 先頭では 1 行目で開く。
    s.key('e');
    let (_p, top) = s.app.take_pending_edit().expect("edit requested");
    assert_eq!(top, Some(1), "先頭スクロールは 1 行目");

    // 下へ十分スクロールしてから `e`: 先頭に張り付かず、後方のソース行を指す。
    for _ in 0..40 {
        s.key('j');
    }
    s.key('e');
    let (_p, deep) = s.app.take_pending_edit().expect("edit requested");
    let deep = deep.expect("装飾 md は描画後に近似行を渡す");
    assert!(
        deep > 10,
        "スクロール後は後方のソース行(>10)を指すべき: {deep}"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_markdown_edit_content_anchor_hits_exact_line() {
    // 折返す長いイントロの後に一意な見出しを置く。比例推定だけだと折返しでずれるが、content-anchor
    // が画面先頭のテキストをソース検索して見出しの「正確な」ソース行に着地することを確認する。
    let dir = sandbox("md_anchor");
    let mut content = String::from("# Title\n\n");
    for i in 1..=15 {
        content.push_str(&format!(
            "Intro paragraph {i:02} with a deliberately long sentence that wraps across several display rows in a narrow preview here.\n"
        ));
    }
    content.push_str("\n## ANCHORZZZ Section\n\n");
    // 見出しを画面先頭(row2)まで押し上げられるよう、後続に十分な行を積む。
    for i in 1..=60 {
        content.push_str(&format!("Trailing line {i:02} after the anchor.\n"));
    }
    // 見出しのソース行(1始まり)。
    let heading_line = content
        .lines()
        .position(|l| l.contains("ANCHORZZZ"))
        .expect("見出しがある")
        + 1;
    std::fs::write(dir.join("doc.md"), &content).unwrap();

    let mut s = Sim::new(&canon(&dir));
    s.select("doc.md");
    s.enter(); // 装飾 Markdown プレビュー
    assert!(!s.app.is_windowed(), "装飾 md は非 windowed");

    // 見出しが画面先頭(枠内1行目=buffer 3行目)に来るまで 1 行ずつスクロール。
    let mut scrolled = 0;
    while scrolled < 300 {
        let top = s.screen().lines().nth(2).unwrap_or("").to_string();
        if top.contains("ANCHORZZZ") {
            break;
        }
        s.key('j');
        scrolled += 1;
    }
    s.see("ANCHORZZZ"); // 画面に見出しが出ている

    s.key('e');
    let (_p, line) = s.app.take_pending_edit().expect("edit requested");
    assert_eq!(
        line,
        Some(heading_line),
        "content-anchor は見出しの正確なソース行 {heading_line} に着地すべき"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_markdown_edit_lands_on_tab_focused_item() {
    // Tab でアイテム(タスク/リンク)にフォーカスしていれば、`e` は画面先頭ではなくその
    // フォーカス行で開く。Tab を進めるとフォーカスが移り、開く行も追従する。
    let dir = sandbox("md_focus_edit");
    let content = "# Title\n\nIntro paragraph one.\n\n- [ ] UNIQUETASK finish the report\n\nMiddle paragraph.\n\nSee [UNIQUELINK](./target.txt) here.\n";
    std::fs::write(dir.join("doc.md"), content).unwrap();
    std::fs::write(dir.join("target.txt"), "x\n").unwrap();
    let task_line = content
        .lines()
        .position(|l| l.contains("UNIQUETASK"))
        .expect("task line")
        + 1;
    let link_line = content
        .lines()
        .position(|l| l.contains("UNIQUELINK"))
        .expect("link line")
        + 1;

    let mut s = Sim::new(&canon(&dir));
    s.select("doc.md");
    s.enter(); // 装飾 Markdown プレビュー
    assert!(!s.app.is_windowed(), "装飾 md は非 windowed");

    // Tab → 最初のアイテム(タスク)にフォーカス。e → タスクのソース行で開く。
    s.tab();
    s.key('e');
    let (_p, line) = s.app.take_pending_edit().expect("edit requested");
    assert_eq!(
        line,
        Some(task_line),
        "Tab フォーカスのタスク行 {task_line}"
    );

    // もう一度 Tab → リンクへ。e → リンクのソース行で開く。
    s.tab();
    s.key('e');
    let (_p, line) = s.app.take_pending_edit().expect("edit requested");
    assert_eq!(
        line,
        Some(link_line),
        "Tab フォーカスのリンク行 {link_line}"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_md_code_block_tab_focus_and_copy() {
    // Tab がリンク→コードブロック→タスクを文書順で巡回し、コードブロックにフォーカス中は
    // `y` でコピーメニューが開き、そこに現れる `c` でその生ソースをコピーできる(値は
    // clipboard 非依存の getter で照合)。Enter はコードブロックでは何もしない。
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
    // `y` は横取りせずコピーメニューを開く(which-key)。フォーカス中は `c:code block` が現れる。
    s.key('y');
    assert_eq!(
        s.app.pending_leader,
        Some(crate::keymap::LeaderId::Copy),
        "y はコピーリーダー(which-key)を開く"
    );
    s.see("code block"); // メニューにコードブロックコピーが出る
                         // `c` でコピー(clipboard は環境依存なので flash が立つことだけ確認)。
    s.key('c');
    assert!(s.app.pending_leader.is_none(), "c で確定=リーダー閉じる");
    assert!(s.app.flash.is_some(), "y c でコピー通知が出る");
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

// =============================================================================
// Paste-jump (P): クリップボードのパス/GitHub リンク → その位置へ移動
// =============================================================================

#[test]
fn e2e_paste_jump_local_path_with_line() {
    // ローカル相対パス + `:行` を渡すと、その位置へ移動してプレビューを開き、指定行まで
    // スクロールする(先頭マーカーは画面外へ)。不在パスは flash 通知でクラッシュしない。
    let dir = sandbox("paste_jump_local");
    seed_files(&dir);
    std::fs::create_dir_all(dir.join("src")).unwrap();
    // 素テキスト(windowed・ハイライト待ちが無い=同期ハーネスで決定的)。行ジャンプ経路は Code と同一。
    let mut body = String::new();
    for i in 1..=60 {
        if i == 1 {
            body.push_str("LINE_ONE_MARKER\n");
        } else if i == 30 {
            body.push_str("LINE_THIRTY_MARKER\n");
        } else {
            body.push_str(&format!("plain line {i}\n"));
        }
    }
    std::fs::write(dir.join("src/deep.txt"), body).unwrap();
    let dir = canon(&dir);

    let mut s = Sim::new(&dir);
    // ツリー(プレビュー未開)から貼り付けジャンプ。paste_jump_from はクリップボード非依存の pub 入口。
    s.app.paste_jump_from("src/deep.txt:30");
    s.draw();
    assert_eq!(s.app.mode, Mode::Preview, "ファイルはプレビューで開く");
    assert!(
        s.app
            .preview_path
            .as_deref()
            .map(|p| p.ends_with("src/deep.txt"))
            .unwrap_or(false),
        "deep.txt がプレビュー対象: {:?}",
        s.app.preview_path
    );
    s.see("LINE_THIRTY_MARKER"); // 30 行目までスクロールした
    s.dont_see("LINE_ONE_MARKER"); // 先頭は画面外＝実際にスクロールしている

    // 見つからないパスは flash で通知し、クラッシュしない(原則#3)。
    s.app.paste_jump_from("does/not/exist.rs");
    s.draw();
    assert!(
        s.app
            .flash
            .as_deref()
            .map(|f| f.contains("exist.rs"))
            .unwrap_or(false),
        "不在パスは flash 通知: {:?}",
        s.app.flash
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[cfg(feature = "git")]
#[test]
fn e2e_paste_jump_github_url_switches_root() {
    // repo のサブディレクトリで konoma を開いた状態で、その repo の別ディレクトリを指す GitHub blob
    // URL を貼ると: ①URL の owner/repo/blob/ref を落として手元の repo(workdir)基準でファイルを解決し、
    // ②対象が現在の root 外なので root を repo(workdir)へ切替えて reveal + preview する。
    let dir = sandbox("paste_jump_url");
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("docs")).unwrap();
    std::fs::write(dir.join("src/lib.rs"), "pub fn hi() {}\n").unwrap();
    std::fs::write(dir.join("docs/guide.md"), "# GUIDE_MARKER\n\nbody\n").unwrap();
    let run = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .current_dir(&dir)
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {out:?}");
    };
    run(&["init", "-q", "."]);
    run(&["config", "user.email", "t@t"]);
    run(&["config", "user.name", "t"]);
    run(&["add", "-A"]);
    run(&["commit", "-q", "-m", "init"]);

    let repo = canon(&dir);
    let subdir = repo.join("src");
    // konoma をサブディレクトリ(src)で起動。docs/guide.md はこの root の外。
    let mut s = Sim::new(&subdir);
    assert_eq!(s.app.root, subdir, "起動 root は src サブディレクトリ");

    // リポジトリ名(owner/name)が手元と違っても、末尾サフィックス docs/guide.md が実在すれば開ける。
    s.app
        .paste_jump_from("https://github.com/some-owner/some-name/blob/main/docs/guide.md");
    s.draw();

    assert_eq!(s.app.root, repo, "root が repo(workdir)へ切替わる");
    assert_eq!(s.app.mode, Mode::Preview, "guide.md をプレビューで開く");
    assert!(
        s.app
            .preview_path
            .as_deref()
            .map(|p| p.ends_with("docs/guide.md"))
            .unwrap_or(false),
        "guide.md がプレビュー対象: {:?}",
        s.app.preview_path
    );
    s.see("GUIDE_MARKER");
    std::fs::remove_dir_all(&dir).ok();
}

// =============================================================================
// Markdown リンクを別タブで開く (Ctrl-t)
// =============================================================================

#[test]
fn e2e_md_link_ctrl_t_opens_in_new_tab() {
    // Markdown のローカルリンクにフォーカスして Ctrl-t を押すと、リンク先を**別タブ**で開く。
    // 元のドキュメントのタブは残り、[ で戻れる。Enter(同タブ)は従来どおり。
    let dir = sandbox("md_link_newtab");
    seed_files(&dir);
    std::fs::write(dir.join("target.txt"), "TARGET_FILE_MARKER\nmore lines\n").unwrap();
    std::fs::write(
        dir.join("doc.md"),
        "# DOC_HEADING\n\nsee [go](./target.txt) here.\n",
    )
    .unwrap();
    let dir = canon(&dir);

    let mut s = Sim::new(&dir);
    s.select("doc.md");
    s.enter();
    s.see("DOC_HEADING");
    assert_eq!(s.app.tab_count(), 1, "最初は1タブ");

    s.tab(); // リンク "go" にフォーカス
    assert!(s.app.focused_item().is_some(), "リンクにフォーカス");

    s.ctrl('t'); // 別タブで開く
    assert_eq!(s.app.tab_count(), 2, "別タブが増える");
    assert_eq!(s.app.mode, Mode::Preview);
    assert!(
        s.app
            .preview_path
            .as_deref()
            .map(|p| p.ends_with("target.txt"))
            .unwrap_or(false),
        "新タブは target.txt を開く: {:?}",
        s.app.preview_path
    );
    s.see("TARGET_FILE_MARKER");

    // 元のドキュメントのタブは残っている: [ (tab_prev) で戻ると doc.md のまま。
    s.key('[');
    assert!(
        s.app
            .preview_path
            .as_deref()
            .map(|p| p.ends_with("doc.md"))
            .unwrap_or(false),
        "元タブは doc.md のまま: {:?}",
        s.app.preview_path
    );
    s.see("DOC_HEADING");
    std::fs::remove_dir_all(&dir).ok();
}

// =============================================================================
// タブ切替でアクティブになったタブをディスクから再読み込み
// =============================================================================

#[test]
fn e2e_tab_switch_reloads_tree_from_disk() {
    // ファイル監視はアクティブ root しか見ないので、裏に居たタブの外部変更はスナップショットのまま。
    // タブに切替えると refresh_fs で再読み込みされ、裏で作成されたファイルがツリーに現れる。
    let dir = sandbox("tab_switch_reload");
    seed_files(&dir);
    let dir = canon(&dir);

    let mut s = Sim::new(&dir);
    s.dont_see("ZZ_NEW_FILE.txt");

    // 新規タブ(同じ root)を作り、そちらをアクティブにする(タブ1 は裏へ)。
    s.key('t');
    assert_eq!(s.app.tab_count(), 2, "タブが2つ");

    // タブ1 が裏に居る間に、外部でファイルを作成する。
    std::fs::write(dir.join("ZZ_NEW_FILE.txt"), "hi\n").unwrap();

    // タブ1 へ戻る([ = tab_prev)。切替時の再読み込みで新規ファイルが現れるはず。
    s.key('[');
    assert_eq!(s.app.active_tab_index(), 0, "タブ1 に戻る");
    assert!(
        s.app
            .entries
            .iter()
            .any(|e| e.path.ends_with("ZZ_NEW_FILE.txt")),
        "タブ切替でツリーが再読み込みされ新規ファイルが現れる"
    );
    s.see("ZZ_NEW_FILE.txt");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_preview_file_paging_ctrl_n_p() {
    // Ctrl-n/Ctrl-p=プレビュー中のファイル送り: ツリー表示順・ディレクトリはスキップ・端で wrap・
    // 展開中サブフォルダ内のファイルも辿る・ツリーカーソルが追従する。
    let dir = std::env::temp_dir().join("konoma_e2e_file_paging");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("mid")).unwrap();
    std::fs::write(dir.join("a.txt"), b"alpha content\n").unwrap();
    std::fs::write(dir.join("z.txt"), b"zulu content\n").unwrap();
    std::fs::write(dir.join("mid/inner.txt"), b"inner content\n").unwrap();
    let mut s = Sim::new(&dir);

    // mid を展開(dirs_first 既定でツリー順は [mid, mid/inner.txt, a.txt, z.txt])。
    s.enter();
    s.key('j'); // inner.txt
    s.key('j'); // a.txt
    s.enter(); // a.txt をプレビュー
    assert!(matches!(s.app.mode, Mode::Preview));
    s.see("alpha content");

    // 次ファイル: a.txt → z.txt。
    s.ctrl('n');
    s.see("zulu content");
    assert!(s.app.preview_path.as_ref().unwrap().ends_with("z.txt"));

    // さらに次: 末尾から wrap し、先頭の mid(ディレクトリ)はスキップして inner.txt へ。
    s.ctrl('n');
    s.see("inner content");
    assert!(s.app.preview_path.as_ref().unwrap().ends_with("inner.txt"));

    // 前ファイル: inner.txt の前は mid(スキップ)→ wrap で z.txt。
    s.ctrl('p');
    s.see("zulu content");
    assert!(s.app.preview_path.as_ref().unwrap().ends_with("z.txt"));

    // ツリーカーソルが追従している(q で戻ると z.txt の上)。
    s.key('q');
    assert!(matches!(s.app.mode, Mode::Tree));
    assert!(s.app.entries[s.app.selected].path.ends_with("z.txt"));
    std::fs::remove_dir_all(&dir).ok();
}

/// `[ui] restore_tabs`: タブセッションが起動ディレクトリ毎に保存され、「再起動」で
/// タブ構成(枚数・アクティブタブ・カーソル・プレビュー面)がそのまま復元される。
#[test]
fn e2e_session_restore_reopens_previous_tabs() {
    let dir = sandbox("session_restore");
    std::fs::write(dir.join("a.txt"), "alpha body\n").unwrap();
    std::fs::write(dir.join("b.txt"), "beta body\n").unwrap();
    let base = std::env::temp_dir().join("konoma_e2e_session_restore_base");
    let _ = std::fs::remove_dir_all(&base);

    // セッション1: a.txt をプレビュー → t で新タブ(ツリー) → b.txt にカーソル。
    let mut s = Sim::new(&dir);
    s.app
        .attach_session_store(crate::session::SessionStore::with_base(base.clone(), &dir));
    s.select("a.txt");
    s.press(KeyCode::Enter, KeyModifiers::NONE);
    assert!(matches!(s.app.mode, Mode::Preview));
    s.key('t');
    s.select("b.txt");
    s.app.save_session(); // 終了時保存に相当(main が run 後に呼ぶ)
    drop(s);

    // 「再起動」: 同じ起動 dir の新しい Sim にストアを繋いで復元する(main の起動順と同じ)。
    let mut s2 = Sim::new(&dir);
    assert_eq!(s2.app.tab_count(), 1, "復元前は素の1タブ");
    s2.app
        .attach_session_store(crate::session::SessionStore::with_base(base.clone(), &dir));
    s2.app.restore_session();
    s2.draw();
    assert_eq!(s2.app.tab_count(), 2, "タブ数を復元");
    assert_eq!(s2.app.active_tab_index(), 1, "アクティブタブを復元");
    assert!(matches!(s2.app.mode, Mode::Tree));
    assert!(
        s2.app.entries[s2.app.selected].path.ends_with("b.txt"),
        "ツリーカーソルを復元"
    );
    s2.see("a.txt"); // タブバーにプレビュータブ(タブ1)のラベルが出る
                     // タブ1へ切替 = a.txt のプレビューが開き直っている(中身が画面に出る)。
    s2.key('[');
    assert!(matches!(s2.app.mode, Mode::Preview));
    s2.see("alpha body");

    std::fs::remove_dir_all(&dir).ok();
    std::fs::remove_dir_all(&base).ok();
}

// =============================================================================
// 追加 E2E バッチ (2026-07-18): カバレッジ薄/欠落領域を大量に補完。
// Sim ハーネス経由で source→実描画の全経路を回し、クリップボード非依存の pub アクセサで照合。
// =============================================================================

/// Write `file` with `body` in a fresh sandbox `dir_name`, open it (tree → preview) under `cfg`,
/// and return the Sim plus the sandbox dir. Like `md_preview` but for any single file.
fn text_preview(cfg: Config, dir_name: &str, file: &str, body: &str) -> (Sim, std::path::PathBuf) {
    let dir = sandbox(dir_name);
    std::fs::write(dir.join(file), body).unwrap();
    let root = canon(&dir);
    let mut s = Sim::with_config(&root, cfg);
    s.select(file);
    s.enter();
    (s, dir)
}

// ---- プレビュー内テキスト検索 (`/` 起動・n/N 移動・大小無視・一致なし・Esc・CJK) ----

#[test]
fn e2e_search_opens_input_shows_prompt_and_query() {
    let dir = sandbox("search_open_input");
    std::fs::write(dir.join("f.txt"), "needle at top\nplain line\n").unwrap();
    let mut s = Sim::new(&canon(&dir));
    s.select("f.txt");
    s.enter();
    assert!(s.app.is_windowed());
    s.key('/');
    assert!(s.app.is_searching());
    assert_eq!(s.app.search_input(), Some(""));
    s.keys("needle");
    assert_eq!(s.app.search_input(), Some("needle"));
    s.see("/needle");
    s.enter();
    assert!(!s.app.is_searching());
    assert_eq!(s.app.preview_search_query(), Some("needle"));
    assert_eq!(s.app.search_status(), Some((1, 1)));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_search_backspace_edits_query() {
    let dir = sandbox("search_backspace");
    std::fs::write(dir.join("f.txt"), "needle here\n").unwrap();
    let mut s = Sim::new(&canon(&dir));
    s.select("f.txt");
    s.enter();
    s.key('/');
    s.keys("need");
    assert_eq!(s.app.search_input(), Some("need"));
    s.press(KeyCode::Backspace, KeyModifiers::NONE);
    assert_eq!(s.app.search_input(), Some("nee"));
    s.keys("dle");
    assert_eq!(s.app.search_input(), Some("needle"));
    s.esc();
    assert!(!s.app.is_searching());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_search_esc_during_input_cancels_no_search() {
    let dir = sandbox("search_esc_input");
    std::fs::write(dir.join("f.txt"), "needle here\nmore\n").unwrap();
    let mut s = Sim::new(&canon(&dir));
    s.select("f.txt");
    s.enter();
    s.key('/');
    s.keys("needle");
    assert!(s.app.is_searching());
    s.esc();
    assert!(!s.app.is_searching());
    assert_eq!(s.app.preview_search_query(), None);
    assert_eq!(s.app.search_status(), None);
    assert_eq!(s.app.mode, Mode::Preview);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_search_commit_populates_status_and_jumps_to_first() {
    let dir = sandbox("search_commit");
    let body = "x MARK a\nfiller one\nfiller two\ny MARK b\nfiller three\nz MARK c\n";
    std::fs::write(dir.join("f.txt"), body).unwrap();
    let mut s = Sim::new(&canon(&dir));
    s.select("f.txt");
    s.enter();
    s.key('/');
    s.keys("MARK");
    s.enter();
    assert!(!s.app.is_searching());
    assert_eq!(s.app.preview_search_query(), Some("MARK"));
    assert_eq!(s.app.search_status(), Some((1, 3)));
    s.see("x MARK a");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_search_next_prev_cycle_with_wrap() {
    let dir = sandbox("search_cycle");
    let body = "a MARK a\nfiller\nfiller\nb MARK b\nfiller\nc MARK c\n";
    std::fs::write(dir.join("f.txt"), body).unwrap();
    let mut s = Sim::new(&canon(&dir));
    s.select("f.txt");
    s.enter();
    s.key('/');
    s.keys("MARK");
    s.enter();
    assert_eq!(s.app.search_status(), Some((1, 3)));
    s.key('n');
    assert_eq!(s.app.search_status(), Some((2, 3)));
    s.key('n');
    assert_eq!(s.app.search_status(), Some((3, 3)));
    s.key('n');
    assert_eq!(s.app.search_status(), Some((1, 3)));
    s.key('N');
    assert_eq!(s.app.search_status(), Some((3, 3)));
    s.key('N');
    assert_eq!(s.app.search_status(), Some((2, 3)));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_search_scroll_follows_current_match() {
    let dir = sandbox("search_follow");
    let mut body = String::new();
    for i in 0..300 {
        if i == 10 || i == 150 || i == 290 {
            body.push_str(&format!("MARK line {i}\n"));
        } else {
            body.push_str(&format!("filler {i}\n"));
        }
    }
    std::fs::write(dir.join("big.txt"), &body).unwrap();
    let mut s = Sim::new(&canon(&dir));
    s.select("big.txt");
    s.enter();
    s.key('/');
    s.keys("MARK");
    s.enter();
    s.see("MARK line 10");
    s.dont_see("MARK line 150");
    s.key('n');
    s.see("MARK line 150");
    s.dont_see("MARK line 10");
    s.key('n');
    s.see("MARK line 290");
    s.key('n');
    s.see("MARK line 10");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_search_case_insensitive_matches() {
    let dir = sandbox("search_ci");
    std::fs::write(
        dir.join("f.txt"),
        "hello line\nWORLD marker here\nbye line\n",
    )
    .unwrap();
    let mut s = Sim::new(&canon(&dir));
    s.select("f.txt");
    s.enter();
    s.key('/');
    s.keys("world");
    s.enter();
    assert_eq!(s.app.search_status(), Some((1, 1)));
    s.see("WORLD marker here");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_search_no_match_flash_and_empty_status() {
    let dir = sandbox("search_no_match");
    std::fs::write(dir.join("f.txt"), "alpha\nbeta\ngamma\n").unwrap();
    let mut s = Sim::new(&canon(&dir));
    s.select("f.txt");
    s.enter();
    s.key('/');
    s.keys("zzzznope");
    s.enter();
    assert_eq!(s.app.preview_search_query(), Some("zzzznope"));
    assert_eq!(s.app.search_status(), None);
    assert!(s
        .app
        .flash
        .as_deref()
        .is_some_and(|m| m.contains("no match")));
    s.key('n');
    assert_eq!(s.app.search_status(), None);
    s.key('N');
    assert_eq!(s.app.search_status(), None);
    s.esc();
    assert_eq!(s.app.preview_search_query(), None);
    assert_eq!(s.app.mode, Mode::Preview);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_search_esc_clears_then_second_esc_returns_to_tree() {
    let dir = sandbox("search_esc_two");
    std::fs::write(dir.join("f.txt"), "alpha\nbeta marker\ngamma\n").unwrap();
    let mut s = Sim::new(&canon(&dir));
    s.select("f.txt");
    s.enter();
    s.key('/');
    s.keys("marker");
    s.enter();
    assert_eq!(s.app.search_status(), Some((1, 1)));
    s.esc();
    assert_eq!(s.app.preview_search_query(), None);
    assert_eq!(s.app.mode, Mode::Preview);
    s.esc();
    assert_eq!(s.app.mode, Mode::Tree);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_search_counts_multiple_occurrences_per_line() {
    let dir = sandbox("search_multi_occ");
    std::fs::write(
        dir.join("f.txt"),
        "one TARGET two TARGET end\nfiller line\nTARGET solo\n",
    )
    .unwrap();
    let mut s = Sim::new(&canon(&dir));
    s.select("f.txt");
    s.enter();
    s.key('/');
    s.keys("TARGET");
    s.enter();
    assert_eq!(s.app.search_status(), Some((1, 3)));
    s.key('n');
    assert_eq!(s.app.search_status(), Some((2, 3)));
    s.see("one TARGET two TARGET end");
    s.key('n');
    assert_eq!(s.app.search_status(), Some((3, 3)));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_search_works_in_code_preview() {
    let dir = sandbox("search_code");
    std::fs::write(
        dir.join("code.rs"),
        "fn alpha() {}\nfn beta() {}\nlet x = 1;\n",
    )
    .unwrap();
    // ハイライトを切って実行順に依存させない: 文法が cold だとプレビューは "loading…" を出し、
    // この検索テストは「他のテストが .rs を温めていたか」で結果が変わってしまう(単独実行だと落ちた)。
    // 検索の検証にシンタックス着色は不要なので、確定的に本文が出る設定で回す。
    let mut cfg = Config::default();
    cfg.ui.syntax_highlight = false;
    let mut s = Sim::with_config(&canon(&dir), cfg);
    s.select("code.rs");
    s.enter();
    assert!(s.app.is_windowed());
    s.key('/');
    s.keys("fn");
    s.enter();
    assert_eq!(s.app.search_status(), Some((1, 2)));
    s.see("fn alpha()");
    std::fs::remove_dir_all(&dir).ok();
}

/// 装飾 Markdown でも `/` が使える(以前は「コード/テキストのみ」と拒否していた)。
/// 検索対象は装飾行なので、`R` の生ソースへ切替えずに一致を強調できる。
#[test]
fn e2e_search_works_on_decorated_markdown_preview() {
    let (mut s, dir) = md_preview(
        Config::default(),
        "doc",
        "# Title\n\nsome needle text here\n",
    );
    assert!(!s.app.is_windowed(), "装飾 md は窓読みではない");
    s.key('/');
    assert!(s.app.is_searching(), "装飾 md でも検索入力に入れる");
    s.keys("needle");
    s.enter();
    assert_eq!(s.app.preview_search_query(), Some("needle"));
    assert_eq!(s.app.search_status(), Some((1, 1)));
    assert!(!s.app.is_raw_source(), "raw ソースに切替わらない");
    std::fs::remove_dir_all(&dir).ok();
}

/// 検索モデルを持たない面(画像など)では従来どおり拒否して flash で知らせる。
#[test]
fn e2e_search_rejected_on_media_preview() {
    let dir = sandbox("search_media");
    std::fs::copy("samples/sample.svg", dir.join("pic.svg")).unwrap();
    let mut s = Sim::new(&canon(&dir));
    s.select("pic.svg");
    s.enter();
    s.key('/');
    assert!(!s.app.is_searching(), "画像プレビューでは検索に入らない");
    assert_eq!(s.app.preview_search_query(), None);
    assert!(s.app.flash.is_some(), "拒否理由を flash で知らせる");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_search_empty_query_enter_is_noop() {
    let dir = sandbox("search_empty");
    std::fs::write(dir.join("f.txt"), "alpha\nbeta\n").unwrap();
    let mut s = Sim::new(&canon(&dir));
    s.select("f.txt");
    s.enter();
    s.key('/');
    assert!(s.app.is_searching());
    s.enter();
    assert!(!s.app.is_searching());
    assert_eq!(s.app.preview_search_query(), None);
    assert_eq!(s.app.search_status(), None);
    assert_eq!(s.app.mode, Mode::Preview);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_search_cjk_query_matches_without_crash() {
    let dir = sandbox("search_cjk");
    std::fs::write(
        dir.join("jp.txt"),
        "リンゴ みかん\nぶどう みかん\nバナナ だけ\n",
    )
    .unwrap();
    let mut s = Sim::new(&canon(&dir));
    s.select("jp.txt");
    s.enter();
    assert!(s.app.is_windowed());
    s.key('/');
    s.keys("みかん");
    s.enter();
    // NOTE: the on-screen text isn't asserted here — the TestBackend renders each full-width CJK
    // glyph as a cell plus a blank continuation cell, so `screen()` inserts spaces between the
    // characters. The functional path (match found, `n` cycles) is what matters and is CJK-safe.
    assert_eq!(s.app.search_status(), Some((1, 2)));
    s.key('n');
    assert_eq!(s.app.search_status(), Some((2, 2)));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_search_second_query_replaces_first() {
    let dir = sandbox("search_replace");
    std::fs::write(
        dir.join("f.txt"),
        "MARK aaa\nfiller\nMARK bbb\nSPOT ccc\nfiller\n",
    )
    .unwrap();
    let mut s = Sim::new(&canon(&dir));
    s.select("f.txt");
    s.enter();
    s.key('/');
    s.keys("MARK");
    s.enter();
    assert_eq!(s.app.search_status(), Some((1, 2)));
    assert_eq!(s.app.preview_search_query(), Some("MARK"));
    s.key('/');
    assert!(s.app.is_searching());
    s.keys("SPOT");
    s.enter();
    assert_eq!(s.app.preview_search_query(), Some("SPOT"));
    assert_eq!(s.app.search_status(), Some((1, 1)));
    s.see("SPOT ccc");
    std::fs::remove_dir_all(&dir).ok();
}

// ---- preview 2D caret + visual selection (v/V) + Y reference ----
// caret line は selection_ref_string() (@path#L{n}) 経由、caret col は preview_selection() で照合。

#[test]
fn e2e_visual_block_caret_drawn_and_moves() {
    use ratatui::style::Modifier;
    let (mut s, dir) = text_preview(
        Config::default(),
        "visual_caret_cell",
        "caret.txt",
        "alpha\nbeta\ngamma\n",
    );
    assert!(s.app.is_windowed());
    s.see("alpha");
    let reversed = |s: &Sim| -> Vec<(usize, usize)> {
        let buf = s.term.backend().buffer();
        let w = buf.area.width as usize;
        buf.content()
            .iter()
            .enumerate()
            .filter(|(_, c)| c.modifier.contains(Modifier::REVERSED))
            .map(|(i, _)| (i % w, i / w))
            .collect()
    };
    let before = reversed(&s);
    assert_eq!(before.len(), 1, "one block-caret cell: {before:?}");
    s.key('j');
    let after = reversed(&s);
    assert_eq!(after.len(), 1);
    assert_eq!(after[0].0, before[0].0);
    assert_eq!(after[0].1, before[0].1 + 1);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_visual_caret_line_tracks_jk_and_gg() {
    let (mut s, dir) = text_preview(
        Config::default(),
        "visual_line_nav",
        "nav.txt",
        "alpha\nbeta\ngamma\n",
    );
    let refstr = |s: &Sim| s.app.selection_ref_string().unwrap();
    assert_eq!(refstr(&s), "@nav.txt#L1");
    s.key('j');
    assert_eq!(refstr(&s), "@nav.txt#L2");
    s.key('j');
    assert_eq!(refstr(&s), "@nav.txt#L3");
    s.key('j');
    assert_eq!(refstr(&s), "@nav.txt#L3");
    s.key('k');
    assert_eq!(refstr(&s), "@nav.txt#L2");
    s.key('G');
    assert_eq!(refstr(&s), "@nav.txt#L3");
    s.key('g');
    assert_eq!(refstr(&s), "@nav.txt#L1");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_visual_v_enters_charwise_state() {
    let (mut s, dir) = text_preview(
        Config::default(),
        "visual_v_char",
        "v.txt",
        "alpha\nbeta\ngamma\n",
    );
    assert!(!s.app.is_preview_visual());
    s.key('v');
    assert!(s.app.is_preview_visual());
    assert!(!s.app.preview_visual_linewise());
    assert_eq!(
        s.app.preview_selection(),
        crate::app::PreviewSelection::Char {
            start: (0, 0),
            end: (0, 0)
        }
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_visual_bigv_enters_linewise_state() {
    let (mut s, dir) = text_preview(
        Config::default(),
        "visual_V_line",
        "vv.txt",
        "alpha\nbeta\ngamma\n",
    );
    s.key('V');
    assert!(s.app.is_preview_visual());
    assert!(s.app.preview_visual_linewise());
    assert_eq!(
        s.app.preview_selection(),
        crate::app::PreviewSelection::Line { lo: 0, hi: 0 }
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_visual_charwise_range_grows_with_l() {
    let (mut s, dir) = text_preview(
        Config::default(),
        "visual_grow_l",
        "gl.txt",
        "alpha\nbeta\ngamma\n",
    );
    s.key('v');
    s.key('l');
    assert_eq!(
        s.app.preview_selection(),
        crate::app::PreviewSelection::Char {
            start: (0, 0),
            end: (0, 1)
        }
    );
    s.key('l');
    assert_eq!(
        s.app.preview_selection(),
        crate::app::PreviewSelection::Char {
            start: (0, 0),
            end: (0, 2)
        }
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_visual_charwise_range_grows_across_lines() {
    let (mut s, dir) = text_preview(
        Config::default(),
        "visual_grow_j",
        "gj.txt",
        "alpha\nbeta\ngamma\n",
    );
    s.key('v');
    s.key('j');
    assert_eq!(
        s.app.preview_selection(),
        crate::app::PreviewSelection::Char {
            start: (0, 0),
            end: (1, 0)
        }
    );
    s.key('j');
    assert_eq!(
        s.app.preview_selection(),
        crate::app::PreviewSelection::Char {
            start: (0, 0),
            end: (2, 0)
        }
    );
    assert_eq!(
        s.app.selection_ref_string().as_deref(),
        Some("@gj.txt#L1-3")
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_visual_linewise_range_grows_with_j() {
    let (mut s, dir) = text_preview(
        Config::default(),
        "visual_lw_grow",
        "lw.txt",
        "alpha\nbeta\ngamma\n",
    );
    s.key('V');
    s.key('j');
    assert_eq!(
        s.app.preview_selection(),
        crate::app::PreviewSelection::Line { lo: 0, hi: 1 }
    );
    assert_eq!(
        s.app.selection_ref_string().as_deref(),
        Some("@lw.txt#L1-2")
    );
    s.key('j');
    assert_eq!(
        s.app.preview_selection(),
        crate::app::PreviewSelection::Line { lo: 0, hi: 2 }
    );
    assert_eq!(
        s.app.selection_ref_string().as_deref(),
        Some("@lw.txt#L1-3")
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_visual_col_home_and_end_bound_selection() {
    let (mut s, dir) = text_preview(
        Config::default(),
        "visual_home_end",
        "he.txt",
        "alpha\nbeta\ngamma\n",
    );
    s.key('v');
    s.key('$');
    assert_eq!(
        s.app.preview_selection(),
        crate::app::PreviewSelection::Char {
            start: (0, 0),
            end: (0, 4)
        }
    );
    s.key('0');
    assert_eq!(
        s.app.preview_selection(),
        crate::app::PreviewSelection::Char {
            start: (0, 0),
            end: (0, 0)
        }
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_visual_gg_extend_linewise_to_ends() {
    let (mut s, dir) = text_preview(
        Config::default(),
        "visual_gg_line",
        "gg.txt",
        "alpha\nbeta\ngamma\n",
    );
    s.key('V');
    s.key('G');
    assert_eq!(
        s.app.preview_selection(),
        crate::app::PreviewSelection::Line { lo: 0, hi: 2 }
    );
    assert_eq!(
        s.app.selection_ref_string().as_deref(),
        Some("@gg.txt#L1-3")
    );
    s.key('g');
    assert_eq!(
        s.app.preview_selection(),
        crate::app::PreviewSelection::Line { lo: 0, hi: 0 }
    );
    assert_eq!(s.app.selection_ref_string().as_deref(), Some("@gg.txt#L1"));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_visual_cancel_with_esc() {
    let (mut s, dir) = text_preview(
        Config::default(),
        "visual_esc",
        "esc.txt",
        "alpha\nbeta\ngamma\n",
    );
    s.key('v');
    s.key('l');
    assert!(s.app.is_preview_visual());
    s.esc();
    assert!(!s.app.is_preview_visual());
    assert_eq!(s.app.mode, Mode::Preview);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_visual_cancel_with_v_bigv_q_toggles() {
    let (mut s, dir) = text_preview(
        Config::default(),
        "visual_toggles",
        "tg.txt",
        "alpha\nbeta\ngamma\n",
    );
    s.key('v');
    assert!(s.app.is_preview_visual());
    s.key('v');
    assert!(!s.app.is_preview_visual());
    assert_eq!(s.app.mode, Mode::Preview);
    s.key('v');
    assert!(s.app.is_preview_visual());
    s.key('V');
    assert!(!s.app.is_preview_visual());
    assert_eq!(s.app.mode, Mode::Preview);
    s.key('v');
    assert!(s.app.is_preview_visual());
    s.key('q');
    assert!(!s.app.is_preview_visual());
    assert_eq!(s.app.mode, Mode::Preview);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_visual_ref_caret_line_and_charwise_span() {
    let (mut s, dir) = text_preview(
        Config::default(),
        "visual_ref_char",
        "rc.txt",
        "alpha\nbeta\ngamma\n",
    );
    assert_eq!(s.app.selection_ref_string().as_deref(), Some("@rc.txt#L1"));
    s.key('j');
    assert_eq!(s.app.selection_ref_string().as_deref(), Some("@rc.txt#L2"));
    s.key('v');
    s.key('j');
    assert!(matches!(
        s.app.preview_selection(),
        crate::app::PreviewSelection::Char { .. }
    ));
    assert_eq!(
        s.app.selection_ref_string().as_deref(),
        Some("@rc.txt#L2-3")
    );
    s.key('Y');
    assert!(!s.app.is_preview_visual());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_visual_ref_linewise_span() {
    let (mut s, dir) = text_preview(
        Config::default(),
        "visual_ref_line",
        "rl.txt",
        "alpha\nbeta\ngamma\n",
    );
    s.key('V');
    s.key('j');
    s.key('j');
    assert_eq!(
        s.app.preview_selection(),
        crate::app::PreviewSelection::Line { lo: 0, hi: 2 }
    );
    assert_eq!(
        s.app.selection_ref_string().as_deref(),
        Some("@rl.txt#L1-3")
    );
    s.key('Y');
    assert!(!s.app.is_preview_visual());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_visual_selection_in_code_preview() {
    let mut cfg = Config::default();
    cfg.ui.syntax_highlight = false;
    let (mut s, dir) = text_preview(cfg, "visual_code", "code.rs", "fn one() {}\nfn two() {}\n");
    assert!(s.app.is_windowed());
    s.see("fn one");
    assert_eq!(s.app.selection_ref_string().as_deref(), Some("@code.rs#L1"));
    s.key('j');
    assert_eq!(s.app.selection_ref_string().as_deref(), Some("@code.rs#L2"));
    s.key('k');
    assert_eq!(s.app.selection_ref_string().as_deref(), Some("@code.rs#L1"));
    s.key('v');
    assert!(s.app.is_preview_visual() && !s.app.preview_visual_linewise());
    s.key('l');
    assert_eq!(
        s.app.preview_selection(),
        crate::app::PreviewSelection::Char {
            start: (0, 0),
            end: (0, 1)
        }
    );
    s.esc();
    assert!(!s.app.is_preview_visual());
    assert_eq!(s.app.mode, Mode::Preview);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_visual_selection_in_raw_markdown() {
    let (mut s, dir) = text_preview(
        Config::default(),
        "visual_rawmd",
        "doc.md",
        "# Heading\nalpha line\nbeta line\n",
    );
    assert!(!s.app.is_windowed());
    assert!(!s.app.is_md_raw());
    s.key('v');
    assert!(!s.app.is_preview_visual());
    s.key('R');
    assert!(s.app.is_windowed());
    assert!(s.app.is_md_raw() && s.app.is_raw_source());
    assert_eq!(s.app.selection_ref_string().as_deref(), Some("@doc.md#L1"));
    s.key('v');
    assert!(s.app.is_preview_visual());
    assert_eq!(
        s.app.preview_selection(),
        crate::app::PreviewSelection::Char {
            start: (0, 0),
            end: (0, 0)
        }
    );
    s.key('j');
    assert_eq!(
        s.app.preview_selection(),
        crate::app::PreviewSelection::Char {
            start: (0, 0),
            end: (1, 0)
        }
    );
    assert_eq!(
        s.app.selection_ref_string().as_deref(),
        Some("@doc.md#L1-2")
    );
    s.esc();
    assert!(!s.app.is_preview_visual());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_visual_cjk_charwise_no_panic() {
    let (mut s, dir) = text_preview(
        Config::default(),
        "visual_cjk",
        "cjk.txt",
        "あいう\nかきくけ\n",
    );
    assert!(s.app.is_windowed());
    assert_eq!(s.app.selection_ref_string().as_deref(), Some("@cjk.txt#L1"));
    s.key('v');
    s.key('l');
    assert_eq!(
        s.app.preview_selection(),
        crate::app::PreviewSelection::Char {
            start: (0, 0),
            end: (0, 1)
        }
    );
    s.key('l');
    assert_eq!(
        s.app.preview_selection(),
        crate::app::PreviewSelection::Char {
            start: (0, 0),
            end: (0, 2)
        }
    );
    s.key('$');
    assert_eq!(
        s.app.preview_selection(),
        crate::app::PreviewSelection::Char {
            start: (0, 0),
            end: (0, 2)
        }
    );
    s.key('y');
    assert!(!s.app.is_preview_visual());
    std::fs::remove_dir_all(&dir).ok();
}

// ---- git: commit / stage・unstage / diff layout / log・graph・branches / copy / changed n/N ----
// すべて #[cfg(feature = "git")]。リポジトリ生成は既存 seed_repo パターンを踏襲。

#[cfg(feature = "git")]
fn seed_repo_history(dir: &std::path::Path) {
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
    run(&["add", "-A"]);
    run(&["commit", "-q", "-m", "alphacommit"]);
    std::fs::write(dir.join("a.rs"), "fn a() { 1 }\n").unwrap();
    run(&["add", "-A"]);
    run(&["commit", "-q", "-m", "betacommit"]);
    std::fs::write(dir.join("a.rs"), "fn a() { 22 }\n").unwrap();
    run(&["add", "-A"]);
    run(&["commit", "-q", "-m", "gammacommit"]);
    run(&["branch", "feature-x"]);
}

#[cfg(feature = "git")]
fn staged_of(app: &App, name: &str) -> Option<bool> {
    app.git_view_entries()
        .iter()
        .find(|e| e.path.ends_with(name))
        .map(|e| e.staged)
}

#[cfg(feature = "git")]
#[test]
fn e2e_git_stage_then_commit_appears_in_log() {
    let dir = sandbox("git_commit_flow");
    seed_repo(&dir);
    let mut s = Sim::new(&canon(&dir));
    s.key('o');
    assert!(s.app.is_git_view());
    assert_eq!(staged_of(&s.app, "a.rs"), Some(false));
    s.key('s');
    assert_eq!(staged_of(&s.app, "a.rs"), Some(true));
    s.key('c');
    assert!(s.app.is_dialog());
    s.keys("ZZCOMMITSUBJECT");
    s.enter();
    let commits = crate::git::log(&s.app.root, 200);
    assert!(
        commits.iter().any(|c| c.summary == "ZZCOMMITSUBJECT"),
        "commit in log: {:?}",
        commits
            .iter()
            .map(|c| c.summary.clone())
            .collect::<Vec<_>>()
    );
    assert!(s.app.is_git_view());
    assert_eq!(staged_of(&s.app, "a.rs"), None);
    assert!(staged_of(&s.app, "new.txt").is_some());
    s.key('l');
    assert!(s.app.is_git_log());
    s.see("ZZCOMMITSUBJECT");
    s.see("init");
    std::fs::remove_dir_all(&dir).ok();
}

#[cfg(feature = "git")]
#[test]
fn e2e_git_stage_unstage_roundtrip() {
    let dir = sandbox("git_stage_unstage");
    seed_repo(&dir);
    let mut s = Sim::new(&canon(&dir));
    s.key('o');
    assert_eq!(staged_of(&s.app, "a.rs"), Some(false));
    s.key('s');
    assert_eq!(staged_of(&s.app, "a.rs"), Some(true));
    s.key('u');
    assert_eq!(staged_of(&s.app, "a.rs"), Some(false));
    std::fs::remove_dir_all(&dir).ok();
}

#[cfg(feature = "git")]
#[test]
fn e2e_git_stage_all_then_unstage_all() {
    let dir = sandbox("git_stage_all");
    seed_repo(&dir);
    let mut s = Sim::new(&canon(&dir));
    s.key('o');
    assert!(!s.app.git_view_entries().is_empty());
    assert!(s.app.git_view_entries().iter().any(|e| !e.staged));
    s.key('S');
    assert!(s.app.git_view_entries().iter().all(|e| e.staged));
    s.key('U');
    assert!(s.app.git_view_entries().iter().all(|e| !e.staged));
    std::fs::remove_dir_all(&dir).ok();
}

#[cfg(feature = "git")]
#[test]
fn e2e_git_diff_layout_toggle_cycles_in_preview() {
    let dir = sandbox("git_diff_layout");
    seed_repo(&dir);
    let mut s = Sim::new(&canon(&dir));
    s.select("a.rs");
    s.key('d');
    assert!(s.app.is_git_diff_preview());
    s.see("diff");
    assert!(!s.app.diff_is_split(200));
    s.key('s');
    s.see("side-by-side");
    assert!(s.app.diff_is_split(1) && s.app.diff_is_split(200));
    s.key('s');
    s.see("diff: auto");
    assert!(!s.app.diff_is_split(50) && s.app.diff_is_split(200));
    s.key('s');
    s.see("diff: unified");
    assert!(!s.app.diff_is_split(200));
    std::fs::remove_dir_all(&dir).ok();
}

#[cfg(feature = "git")]
#[test]
fn e2e_git_worktree_diff_toggle_layout_and_back() {
    let dir = sandbox("git_worktree_detail");
    seed_repo(&dir);
    let mut s = Sim::new(&canon(&dir));
    s.key('o');
    s.key('d');
    assert!(s.app.is_git_detail());
    s.see("Uncommitted changes");
    s.dont_see("⇆");
    s.key('s');
    s.see("side-by-side");
    s.see("⇆");
    s.key('q');
    assert!(!s.app.is_git_detail());
    assert!(s.app.is_git_view());
    std::fs::remove_dir_all(&dir).ok();
}

#[cfg(feature = "git")]
#[test]
fn e2e_git_log_navigate_open_detail_and_back() {
    let dir = sandbox("git_log_nav");
    seed_repo_history(&dir);
    let mut s = Sim::new(&canon(&dir));
    s.key('o');
    s.key('l');
    assert!(s.app.is_git_log());
    s.see("gammacommit");
    s.see("betacommit");
    s.see("alphacommit");
    let top = s.app.git_log_selected_id().expect("top id");
    s.key('j');
    let second = s.app.git_log_selected_id().expect("second id");
    assert_ne!(top, second);
    // Derive the selected commit's subject rather than assuming a specific ordering.
    let second_subj = crate::git::commit_meta(&s.app.root, &second)
        .and_then(|m| m.message.lines().next().map(str::to_string))
        .expect("second subject");
    s.enter();
    assert!(s.app.is_git_detail());
    s.see(&second_subj); // Enter opens the selected commit's detail
    s.key('q');
    assert!(s.app.is_git_log());
    assert_eq!(
        s.app.git_log_selected_id().as_deref(),
        Some(second.as_str())
    );
    s.key('l');
    assert!(s.app.is_git_detail());
    s.see(&second_subj); // `l` also opens the same commit's detail
    s.key('q');
    assert!(s.app.is_git_log());
    std::fs::remove_dir_all(&dir).ok();
}

#[cfg(feature = "git")]
#[test]
fn e2e_git_graph_navigates_commit_rows() {
    let dir = sandbox("git_graph_nav");
    seed_repo_history(&dir);
    let mut s = Sim::new(&canon(&dir));
    s.key('o');
    s.key('g');
    assert!(s.app.is_git_graph());
    s.see("●");
    s.key('g');
    let top = s
        .app
        .git_graph_selected_row()
        .and_then(|r| r.commit.clone())
        .expect("top commit");
    s.key('G');
    let bottom = s
        .app
        .git_graph_selected_row()
        .and_then(|r| r.commit.clone())
        .expect("bottom commit");
    assert_ne!(top, bottom);
    s.key('g');
    s.key('j');
    let after_j = s
        .app
        .git_graph_selected_row()
        .and_then(|r| r.commit.clone());
    assert!(
        after_j.is_some() && after_j.as_deref() != Some(top.as_str()),
        "j: {after_j:?}"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[cfg(feature = "git")]
#[test]
fn e2e_git_graph_set_base_keeps_selection_then_clear() {
    let dir = sandbox("git_graph_base2");
    seed_repo_history(&dir);
    let mut s = Sim::new(&canon(&dir));
    s.key('o');
    s.key('g');
    s.dont_see("⌖ base:");
    assert!(s.app.git_graph_base_label().is_none());
    s.key('g');
    let sel_before = s
        .app
        .git_graph_selected_row()
        .and_then(|r| r.commit.clone())
        .expect("commit");
    s.key('s');
    s.see("⌖ base:");
    assert!(s.app.git_graph_base_label().is_some());
    assert_eq!(
        s.app
            .git_graph_selected_row()
            .and_then(|r| r.commit.clone()),
        Some(sel_before)
    );
    s.key('x');
    s.dont_see("⌖ base:");
    assert!(s.app.git_graph_base_label().is_none());
    std::fs::remove_dir_all(&dir).ok();
}

#[cfg(feature = "git")]
#[test]
fn e2e_git_graph_picker_current_only_then_all() {
    let dir = sandbox("git_graph_picker2");
    seed_repo_history(&dir);
    let mut s = Sim::new(&canon(&dir));
    s.key('o');
    s.key('g');
    s.key('b');
    assert!(s.app.is_git_graph_picker());
    let feature_on = |app: &App| -> Option<bool> {
        app.git_graph_picker_items()
            .into_iter()
            .find(|(name, _, _)| name == "feature-x")
            .map(|(_, _, on)| on)
    };
    assert_eq!(feature_on(&s.app), Some(true));
    s.key('n');
    assert_eq!(feature_on(&s.app), Some(false));
    s.key('a');
    assert_eq!(feature_on(&s.app), Some(true));
    s.key('q');
    assert!(!s.app.is_git_graph_picker());
    assert!(s.app.is_git_graph());
    std::fs::remove_dir_all(&dir).ok();
}

#[cfg(feature = "git")]
#[test]
fn e2e_git_branches_filter_narrows_and_clears() {
    let dir = sandbox("git_branches_filter");
    seed_repo_history(&dir);
    let mut s = Sim::new(&canon(&dir));
    s.key('o');
    s.key('b');
    assert!(s.app.is_git_branches());
    assert_eq!(s.app.git_branch_view().len(), 2);
    s.key('/');
    assert!(s.app.git_branch_filtering());
    s.keys("feature");
    let view = s.app.git_branch_view();
    assert_eq!(view.len(), 1);
    assert_eq!(view[0].name, "feature-x");
    s.see("feature-x");
    s.esc();
    assert!(!s.app.git_branch_filtering());
    assert_eq!(s.app.git_branch_view().len(), 2);
    std::fs::remove_dir_all(&dir).ok();
}

#[cfg(feature = "git")]
#[test]
fn e2e_git_branch_create_switches_to_it() {
    let dir = sandbox("git_branch_create");
    seed_repo(&dir);
    let mut s = Sim::new(&canon(&dir));
    s.key('o');
    s.key('b');
    s.key('n');
    assert!(s.app.is_dialog());
    s.keys("createdbr");
    s.enter();
    let branches = crate::git::branches(&s.app.root);
    let created = branches.iter().find(|b| b.name == "createdbr");
    assert!(created.is_some());
    assert!(created.unwrap().is_current);
    assert!(s.app.is_git_view());
    std::fs::remove_dir_all(&dir).ok();
}

#[cfg(feature = "git")]
#[test]
fn e2e_git_branch_delete_removes_feature_branch() {
    let dir = sandbox("git_branch_delete");
    seed_repo(&dir);
    let mut s = Sim::new(&canon(&dir));
    s.key('o');
    s.key('b');
    s.key('g');
    assert_eq!(
        s.app.git_branch_view()[s.app.git_branch_sel()].name,
        "feature-x"
    );
    s.key('d');
    assert!(s.app.is_dialog());
    s.key('y');
    assert!(!crate::git::branches(&s.app.root)
        .iter()
        .any(|b| b.name == "feature-x"));
    assert!(!s
        .app
        .git_branch_view()
        .iter()
        .any(|b| b.name == "feature-x"));
    std::fs::remove_dir_all(&dir).ok();
}

#[cfg(feature = "git")]
#[test]
fn e2e_git_copy_leader_all_subkeys_consume_from_log() {
    let dir = sandbox("git_copy_subkeys");
    seed_repo(&dir);
    let mut s = Sim::new(&canon(&dir));
    s.key('o');
    s.key('l');
    assert_eq!(s.app.surface(), crate::keymap::Surface::GitLog);
    s.see("init");
    let id = s.app.git_log_selected_id().expect("selected id");
    let meta = crate::git::commit_meta(&s.app.root, &id).expect("commit_meta");
    assert_eq!(meta.message.lines().next(), Some("init"));
    assert_eq!(meta.author, "t");
    assert_eq!(meta.short.len(), 7);
    assert_eq!(meta.id.len(), 40);
    assert!(meta.id.starts_with(&meta.short));
    assert!(!meta.date.is_empty());
    for sub in ['s', 'h', 't', 'm', 'a', 'd'] {
        s.key('y');
        assert_eq!(
            s.app.pending_leader,
            Some(crate::keymap::LeaderId::GitCopy),
            "sub={sub}"
        );
        s.key(sub);
        assert_eq!(s.app.pending_leader, None, "y->{sub}");
    }
    std::fs::remove_dir_all(&dir).ok();
}

#[cfg(feature = "git")]
#[test]
fn e2e_git_copy_leader_opens_in_graph_and_detail() {
    let dir = sandbox("git_copy_surfaces");
    seed_repo_history(&dir);
    let mut s = Sim::new(&canon(&dir));
    s.key('o');
    s.key('g');
    s.key('g');
    assert_eq!(s.app.surface(), crate::keymap::Surface::GitGraph);
    s.key('y');
    assert_eq!(s.app.pending_leader, Some(crate::keymap::LeaderId::GitCopy));
    s.esc();
    assert_eq!(s.app.pending_leader, None);
    s.enter();
    assert_eq!(s.app.surface(), crate::keymap::Surface::GitDetail);
    s.key('y');
    assert_eq!(s.app.pending_leader, Some(crate::keymap::LeaderId::GitCopy));
    s.esc();
    assert_eq!(s.app.pending_leader, None);
    std::fs::remove_dir_all(&dir).ok();
}

#[cfg(feature = "git")]
#[test]
fn e2e_git_changed_filter_n_jumps_between_changed_files() {
    let dir = sandbox("git_changed_nN");
    seed_repo(&dir);
    let mut s = Sim::new(&canon(&dir));
    s.key('C');
    assert!(s.app.changed_filter());
    s.see("a.rs");
    s.see("new.txt");
    s.select("a.rs");
    assert!(s.app.entries[s.app.selected].path.ends_with("a.rs"));
    s.key('n');
    assert!(s.app.entries[s.app.selected].path.ends_with("new.txt"));
    s.key('n');
    assert!(s.app.entries[s.app.selected].path.ends_with("a.rs"));
    s.key('N');
    assert!(s.app.entries[s.app.selected].path.ends_with("new.txt"));
    s.key('C');
    assert!(!s.app.changed_filter());
    std::fs::remove_dir_all(&dir).ok();
}

// ---- ツリーソート (size/modified/ext) + 絞り込みの端 ----

#[test]
fn e2e_sort_by_size_orders_ascending() {
    let dir = sandbox("sort_size");
    std::fs::write(dir.join("aaa.txt"), vec![b'a'; 300]).unwrap();
    std::fs::write(dir.join("bbb.txt"), vec![b'b'; 30]).unwrap();
    std::fs::write(dir.join("ccc.txt"), vec![b'c'; 150]).unwrap();
    let mut s = Sim::new(&canon(&dir));
    let names: Vec<&str> = s
        .app
        .entries
        .iter()
        .map(|e| e.path.file_name().and_then(|n| n.to_str()).unwrap_or(""))
        .collect();
    assert_eq!(names, vec!["aaa.txt", "bbb.txt", "ccc.txt"]);
    s.key('s');
    s.key('s');
    assert!(!s.app.is_sort_menu());
    let names: Vec<&str> = s
        .app
        .entries
        .iter()
        .map(|e| e.path.file_name().and_then(|n| n.to_str()).unwrap_or(""))
        .collect();
    assert_eq!(names, vec!["bbb.txt", "ccc.txt", "aaa.txt"]);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_sort_by_modified_orders_ascending() {
    let dir = sandbox("sort_mod");
    fn set_mtime(p: &std::path::Path, secs: u64) {
        std::fs::write(p, b"x\n").unwrap();
        let f = std::fs::OpenOptions::new().write(true).open(p).unwrap();
        f.set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs))
            .unwrap();
    }
    set_mtime(&dir.join("m_a.txt"), 2_000_000_000);
    set_mtime(&dir.join("m_b.txt"), 1_000_000_000);
    set_mtime(&dir.join("m_c.txt"), 1_500_000_000);
    let mut s = Sim::new(&canon(&dir));
    s.key('s');
    s.key('m');
    assert!(!s.app.is_sort_menu());
    let names: Vec<&str> = s
        .app
        .entries
        .iter()
        .map(|e| e.path.file_name().and_then(|n| n.to_str()).unwrap_or(""))
        .collect();
    assert_eq!(names, vec!["m_b.txt", "m_c.txt", "m_a.txt"]);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_sort_by_ext_orders_ascending() {
    let dir = sandbox("sort_ext");
    std::fs::write(dir.join("zzz.aaa"), b"1\n").unwrap();
    std::fs::write(dir.join("aaa.zzz"), b"2\n").unwrap();
    std::fs::write(dir.join("mmm.mmm"), b"3\n").unwrap();
    let mut s = Sim::new(&canon(&dir));
    let names: Vec<&str> = s
        .app
        .entries
        .iter()
        .map(|e| e.path.file_name().and_then(|n| n.to_str()).unwrap_or(""))
        .collect();
    assert_eq!(names, vec!["aaa.zzz", "mmm.mmm", "zzz.aaa"]);
    s.key('s');
    s.key('e');
    assert!(!s.app.is_sort_menu());
    let names: Vec<&str> = s
        .app
        .entries
        .iter()
        .map(|e| e.path.file_name().and_then(|n| n.to_str()).unwrap_or(""))
        .collect();
    assert_eq!(names, vec!["zzz.aaa", "mmm.mmm", "aaa.zzz"]);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_tree_filter_no_matches_shows_empty_list() {
    let dir = sandbox("filter_nomatch");
    seed_files(&dir);
    let mut s = Sim::new(&canon(&dir));
    let all = s.app.entries.len();
    s.key('/');
    s.keys("qzqzqznope");
    assert!(s.app.is_filtering());
    assert!(s.app.entries.is_empty(), "{}", s.screen());
    s.dont_see("notes.txt");
    s.enter();
    assert!(s.app.entries.is_empty());
    s.esc();
    assert_eq!(s.app.entries.len(), all);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_tree_filter_is_case_insensitive() {
    let dir = sandbox("filter_case");
    seed_files(&dir);
    std::fs::write(dir.join("UPPER.TXT"), "X\n").unwrap();
    let mut s = Sim::new(&canon(&dir));
    s.key('/');
    s.keys("README");
    s.see("readme.md");
    assert!(s.app.entries.iter().any(|e| e.path.ends_with("readme.md")));
    assert!(!s.app.entries.iter().any(|e| e.path.ends_with("UPPER.TXT")));
    for _ in 0..8 {
        s.press(KeyCode::Backspace, KeyModifiers::NONE);
    }
    s.keys("upper");
    s.see("UPPER.TXT");
    assert!(s.app.entries.iter().any(|e| e.path.ends_with("UPPER.TXT")));
    std::fs::remove_dir_all(&dir).ok();
}

// ---- CSV/テーブルのコピー (y→c/r/C/f) ----

#[test]
fn e2e_table_copy_leader_sub_keys_consume_and_flash() {
    let dir = sandbox("table_copy_wiring");
    std::fs::write(dir.join("data.csv"), "name,qty\napple,3\nkiwi,7\n").unwrap();
    let mut s = Sim::new(&canon(&dir));
    s.select("data.csv");
    s.enter();
    s.see("TABLE");
    s.key('j');
    s.key('l');
    assert_eq!(s.app.table_cursor(), (1, 1));
    for sub in ['c', 'r', 'C'] {
        s.app.flash = None;
        s.key('y');
        assert_eq!(
            s.app.pending_leader,
            Some(crate::keymap::LeaderId::TableCopy)
        );
        s.key(sub);
        assert_eq!(s.app.pending_leader, None, "sub={sub}");
        assert!(s.app.flash.is_some(), "sub={sub}");
    }
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_table_copy_full_path_value() {
    let dir = sandbox("table_copy_path");
    std::fs::write(dir.join("data.csv"), "name,qty\napple,3\n").unwrap();
    let mut s = Sim::new(&canon(&dir));
    s.select("data.csv");
    s.enter();
    s.see("TABLE");
    let csv = s.app.preview_path.clone().expect("preview path");
    assert_eq!(
        s.app.copy_string_for(crate::app::CopyKind::Full),
        Some(csv.display().to_string())
    );
    s.key('y');
    assert_eq!(
        s.app.pending_leader,
        Some(crate::keymap::LeaderId::TableCopy)
    );
    s.key('f');
    assert_eq!(s.app.pending_leader, None);
    std::fs::remove_dir_all(&dir).ok();
}

// ---- プレビューからのブックマーク (m / ') ----

#[test]
fn e2e_bookmark_set_from_preview_targets_shown_file() {
    let root = sandbox("bm_preview_target");
    let proj = root.join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    seed_files(&proj);
    let proj = canon(&proj);
    let mut s = Sim::new(&proj);
    s.app.bookmarks = crate::bookmarks::Bookmarks::with_base(root.join("cfg"), &proj);
    s.select("notes.txt");
    s.keys("ma");
    s.see("bookmarked");
    s.select("readme.md");
    s.key('\'');
    s.see("Bookmarks");
    s.key('a');
    assert_eq!(s.app.mode, Mode::Preview);
    assert!(s
        .app
        .preview_path
        .as_deref()
        .is_some_and(|p| p.ends_with("notes.txt")));
    assert!(s.app.entries[s.app.selected].path.ends_with("readme.md"));
    s.keys("mx");
    assert!(
        s.app
            .bookmarks
            .get('x')
            .is_some_and(|p| p.ends_with("notes.txt")),
        "{:?}",
        s.app.bookmarks.get('x')
    );
    assert!(!s
        .app
        .bookmarks
        .get('x')
        .is_some_and(|p| p.ends_with("readme.md")));
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn e2e_bookmark_list_enter_jumps_to_file() {
    let root = sandbox("bm_list_enter");
    let proj = root.join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    std::fs::write(proj.join("a.txt"), "ALPHA_BODY\n").unwrap();
    std::fs::write(proj.join("b.txt"), "BETA_BODY\n").unwrap();
    let proj = canon(&proj);
    let mut s = Sim::new(&proj);
    s.app.bookmarks = crate::bookmarks::Bookmarks::with_base(root.join("cfg"), &proj);
    s.select("a.txt");
    s.keys("ma");
    s.select("b.txt");
    s.keys("mb");
    s.select("b.txt");
    s.enter();
    assert_eq!(s.app.mode, Mode::Preview);
    s.see("BETA_BODY");
    s.key('\'');
    s.see("Bookmarks");
    assert_eq!(s.app.bookmark_list_sel(), 0);
    s.enter();
    assert!(!s.app.is_bookmark_list());
    assert_eq!(s.app.mode, Mode::Preview);
    assert!(
        s.app
            .preview_path
            .as_deref()
            .is_some_and(|p| p.ends_with("a.txt")),
        "{:?}",
        s.app.preview_path
    );
    s.see("ALPHA_BODY");
    std::fs::remove_dir_all(&root).ok();
}

// ---- Paste-jump (P) の端 ----

#[test]
fn e2e_pastejump_hash_line_fragment_scrolls() {
    let dir = sandbox("pastejump_hash");
    let mut body = String::new();
    for i in 1..=60 {
        if i == 1 {
            body.push_str("LINE_ONE_ZZ\n");
        } else if i == 40 {
            body.push_str("LINE_FORTY_ZZ\n");
        } else {
            body.push_str(&format!("plain line {i}\n"));
        }
    }
    std::fs::write(dir.join("deep.txt"), body).unwrap();
    let dir = canon(&dir);
    let mut s = Sim::new(&dir);
    s.app.paste_jump_from("deep.txt#L40");
    s.draw();
    assert_eq!(s.app.mode, Mode::Preview);
    assert!(s
        .app
        .preview_path
        .as_deref()
        .is_some_and(|p| p.ends_with("deep.txt")));
    s.see("LINE_FORTY_ZZ");
    s.dont_see("LINE_ONE_ZZ");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_pastejump_at_ref_relative_opens() {
    let dir = sandbox("pastejump_atref");
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    std::fs::write(dir.join("sub/target.txt"), "AT_REF_BODY\nsecond line\n").unwrap();
    let dir = canon(&dir);
    let mut s = Sim::new(&dir);
    s.app.paste_jump_from("@sub/target.txt");
    s.draw();
    assert_eq!(s.app.mode, Mode::Preview);
    assert!(s
        .app
        .preview_path
        .as_deref()
        .is_some_and(|p| p.ends_with("sub/target.txt")));
    s.see("AT_REF_BODY");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_pastejump_no_line_opens_at_top() {
    let dir = sandbox("pastejump_top");
    let mut body = String::new();
    for i in 1..=60 {
        if i == 1 {
            body.push_str("TOP_MARKER_LINE\n");
        } else if i == 50 {
            body.push_str("DEEP_MARKER_LINE\n");
        } else {
            body.push_str(&format!("filler line {i}\n"));
        }
    }
    std::fs::write(dir.join("top.txt"), body).unwrap();
    let dir = canon(&dir);
    let mut s = Sim::new(&dir);
    s.app.paste_jump_from("top.txt");
    s.draw();
    assert_eq!(s.app.mode, Mode::Preview);
    assert!(s
        .app
        .preview_path
        .as_deref()
        .is_some_and(|p| p.ends_with("top.txt")));
    s.see("TOP_MARKER_LINE");
    s.dont_see("DEEP_MARKER_LINE");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_pastejump_non_github_url_unrecognized() {
    let dir = sandbox("pastejump_nongithub");
    seed_files(&dir);
    let dir = canon(&dir);
    let mut s = Sim::new(&dir);
    assert_eq!(s.app.mode, Mode::Tree);
    assert!(s.app.preview_path.is_none());
    s.app.paste_jump_from("https://example.com/some/file.rs");
    s.draw();
    assert_eq!(s.app.mode, Mode::Tree);
    assert!(s.app.preview_path.is_none());
    assert!(s.app.flash.is_some());
    std::fs::remove_dir_all(&dir).ok();
}

// ---- ファイル操作の端 ----

#[cfg(target_os = "macos")]
#[test]
fn e2e_fileop_delete_trash_removes_file() {
    let dir = sandbox("fileop_trash");
    seed_files(&dir);
    let mut s = Sim::new(&canon(&dir));
    s.select("notes.txt");
    s.key(' ');
    s.key('d');
    assert!(s.app.is_dialog());
    s.key('y');
    assert!(!dir.join("notes.txt").exists());
    assert!(!s.app.is_dialog());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_fileop_rename_to_existing_name_fails() {
    let dir = sandbox("fileop_rename_exist");
    seed_files(&dir);
    let mut s = Sim::new(&canon(&dir));
    s.select("notes.txt");
    s.key(' ');
    s.key('r');
    for _ in 0..40 {
        s.press(KeyCode::Backspace, KeyModifiers::NONE);
    }
    s.keys("data.csv");
    s.enter();
    assert!(dir.join("notes.txt").exists());
    assert!(dir.join("data.csv").exists());
    assert!(!s.app.is_dialog());
    assert!(s.app.flash.is_some());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_fileop_create_targets_cursor_file_parent() {
    let dir = sandbox("fileop_create_subdir");
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    std::fs::write(dir.join("sub/inner.txt"), "x\n").unwrap();
    std::fs::write(dir.join("root_file.txt"), "y\n").unwrap();
    let dir = canon(&dir);
    let mut s = Sim::new(&dir);
    s.select("sub");
    s.enter();
    s.select("sub/inner.txt");
    s.key(' ');
    s.key('n');
    s.keys("made.txt");
    s.enter();
    assert!(dir.join("sub/made.txt").exists());
    assert!(!dir.join("made.txt").exists());
    std::fs::remove_dir_all(&dir).ok();
}
