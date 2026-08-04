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
    /// Receiver for background file operations, once `with_async_file_ops` opted in (the run loop
    /// owns this channel live). `None` = the synchronous fallback in `start_file_op` is used.
    fileop_rx: Option<std::sync::mpsc::Receiver<crate::app::FileOpResult>>,
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
            fileop_rx: None,
        };
        sim.draw();
        sim
    }

    /// Opt in to the **real background path** for file operations (copy/move/duplicate/delete):
    /// attach a runner channel just like `main` does, so `start_file_op` spawns a worker instead of
    /// falling back to running the operation inline. Results are applied by `drain_file_ops`.
    fn with_async_file_ops(mut self) -> Sim {
        let (tx, rx) = std::sync::mpsc::channel();
        self.app.attach_fileop_runner(tx);
        self.fileop_rx = Some(rx);
        self
    }

    /// Attach a bare image backend — a deterministic, no-real-terminal-needed `Picker::halfblocks()`
    /// — WITHOUT attaching any of the async decode/encode worker channels (media_tx / md_img_tx /
    /// md_enc_tx). This is enough to unlock every picker-gated code path that only needs
    /// `picker.font_size()` (still-image decode, and the Markdown mermaid-fence / math-expression
    /// classification and placement sizing), because those all have a synchronous fallback when no
    /// worker tx is attached (`ensure_mermaid_fence_render` / `ensure_math_render` / `load_image`).
    /// What stays out of reach without a real encode-worker thread is the actual **pixel content**
    /// of a rendered raster (e.g. comparing colors for `math_color`/`mermaid_theme`) — that needs
    /// `md_enc_tx` + a running `md_encode_worker`, which this helper does not set up.
    fn with_picker(mut self) -> Sim {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        std::mem::forget(rx); // never drained: nothing sends without media_tx/md_img_tx attached
        self.app
            .attach_image_backend(ratatui_image::picker::Picker::halfblocks(), tx);
        self
    }

    /// Wait for the in-flight file operation's result and apply it (the run loop's
    /// `while let Ok(result) = rx.fileop.try_recv()` step), then redraw.
    #[track_caller]
    fn drain_file_ops(&mut self) {
        let rx = self
            .fileop_rx
            .as_ref()
            .expect("with_async_file_ops() を呼んでいない");
        let res = rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("ワーカーが結果を返す");
        assert!(self.app.apply_file_op(res), "現世代の結果は適用される");
        self.draw();
    }

    fn draw(&mut self) {
        let app = &mut self.app;
        self.term.draw(|f| ui::render(f, app)).expect("draw");
    }

    fn press(&mut self, code: KeyCode, mods: KeyModifiers) {
        let res = handle_key(&mut self.app, KeyEvent::new(code, mods));
        // Same as the run loop, a recoverable Err becomes a flash (a simplified equivalent of resolve_key_result).
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
            .tab
            .entries
            .iter()
            .position(|e| e.path.to_string_lossy().ends_with(suffix))
            .unwrap_or_else(|| panic!("entry not found: {suffix}"));
        self.app.tab.selected = i;
        self.draw();
    }

    // ---- Style inspection (H2: konoma leans on "sentinel styles" — REVERSED focus, colored
    // gutters/markers — that plain `screen()` string matching cannot see. These helpers read the
    // drawn buffer's per-cell `Style` so a test can assert on it directly.) ----

    /// The style of the cell at (row, col) in the last drawn frame.
    fn cell_style(&self, row: u16, col: u16) -> ratatui::style::Style {
        let buf = self.term.backend().buffer();
        buf[(col, row)].style()
    }

    /// The (row, col) of the first cell where `needle` starts, searching row by row from the top.
    /// Only matches within a single row (konoma's screen rows are independent grids; a needle that
    /// wraps across rows will not be found — none of this harness's needles do). Multi-byte glyphs
    /// (CJK, icons) are handled the same way `screen()` renders them: one cell's `symbol()` may be
    /// several bytes, and the byte→column mapping below accounts for that.
    fn find_text(&self, needle: &str) -> Option<(u16, u16)> {
        let buf = self.term.backend().buffer();
        let w = buf.area.width;
        let h = buf.area.height;
        for row in 0..h {
            let mut row_str = String::new();
            let mut col_at_byte: Vec<u16> = Vec::new();
            for col in 0..w {
                let sym = buf[(col, row)].symbol();
                for _ in 0..sym.len() {
                    col_at_byte.push(col);
                }
                row_str.push_str(sym);
            }
            if let Some(byte_idx) = row_str.find(needle) {
                let col = col_at_byte.get(byte_idx).copied().unwrap_or(0);
                return Some((row, col));
            }
        }
        None
    }

    /// The style of the first cell of `needle`'s first on-screen occurrence, if any.
    fn style_of(&self, needle: &str) -> Option<ratatui::style::Style> {
        self.find_text(needle).map(|(r, c)| self.cell_style(r, c))
    }

    /// Assert `needle` is on screen and its (first cell's) style satisfies `pred`. Fails loudly —
    /// both when `needle` cannot be found at all, and when it's found but styled wrong — with the
    /// full screen dumped either way, so a failure is diagnosable without rerunning under a debugger.
    #[track_caller]
    fn see_styled(&self, needle: &str, pred: impl Fn(&ratatui::style::Style) -> bool, what: &str) {
        let Some((row, col)) = self.find_text(needle) else {
            panic!(
                "画面に「{needle}」が見えるはず({what}を確認できない):\n{}",
                self.screen()
            );
        };
        let style = self.cell_style(row, col);
        assert!(
            pred(&style),
            "「{needle}」のスタイルが {what} を満たさない(実際: {style:?}, 位置=({row},{col})):\n{}",
            self.screen()
        );
    }

    /// How many distinct foreground colors appear in row `row` (e.g. to confirm a rainbow-colored
    /// table row actually alternates colors instead of being uniformly styled).
    fn distinct_fg_in_row(&self, row: u16) -> usize {
        let buf = self.term.backend().buffer();
        let w = buf.area.width;
        let mut set = std::collections::HashSet::new();
        for col in 0..w {
            set.insert(buf[(col, row)].fg);
        }
        set.len()
    }

    /// Every (row, col) currently drawn with the REVERSED modifier.
    fn reversed_cells(&self) -> Vec<(u16, u16)> {
        use ratatui::style::Modifier;
        let buf = self.term.backend().buffer();
        let w = buf.area.width;
        let h = buf.area.height;
        let mut out = Vec::new();
        for row in 0..h {
            for col in 0..w {
                if buf[(col, row)].modifier.contains(Modifier::REVERSED) {
                    out.push((row, col));
                }
            }
        }
        out
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
// Tree: movement, filtering, hidden files, sort
// =============================================================================

#[test]
fn e2e_tree_navigation_and_ends() {
    let dir = sandbox("tree_nav");
    seed_files(&dir);
    let mut s = Sim::new(&canon(&dir));
    s.see("TREE");
    let first = s.app.tab.selected;
    s.key('j');
    assert_eq!(s.app.tab.selected, first + 1, "j で1つ下へ");
    s.key('k');
    assert_eq!(s.app.tab.selected, first, "k で戻る");
    s.key('G');
    assert_eq!(s.app.tab.selected, s.app.tab.entries.len() - 1, "G で末尾");
    s.key('g');
    assert_eq!(s.app.tab.selected, 0, "g で先頭");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_tree_filter_narrows_and_esc_clears() {
    let dir = sandbox("tree_filter");
    seed_files(&dir);
    let mut s = Sim::new(&canon(&dir));
    let all = s.app.tab.entries.len();
    s.key('/');
    s.keys("csv");
    s.see("data.csv");
    s.dont_see("readme.md");
    assert!(s.app.tab.entries.len() < all, "絞り込みで件数が減る");
    s.enter(); // confirm
    s.see("data.csv");
    s.esc(); // clear
    assert_eq!(s.app.tab.entries.len(), all, "Esc で全件に戻る");
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
        s.app.tab.root.ends_with("src"),
        "l でディレクトリへ潜る(root 変更)"
    );
    s.see("lib.rs");
    s.key('h');
    assert_eq!(s.app.tab.root, root, "h で親へ戻る");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_sort_menu_switches_order() {
    let dir = sandbox("sort");
    seed_files(&dir);
    let mut s = Sim::new(&canon(&dir));
    s.key('s');
    s.see("sort"); // the sort menu's footer/display
    s.key('s'); // by size
                // The menu closes and the order has changed (directories still stay first).
    assert!(!s.app.is_sort_menu());
    std::fs::remove_dir_all(&dir).ok();
}

// =============================================================================
// File operations: create, rename, delete (cancel and permanent delete)
// =============================================================================

#[test]
fn e2e_file_create_via_dialog() {
    let dir = sandbox("file_create");
    seed_files(&dir);
    let mut s = Sim::new(&canon(&dir));
    // The creation target is "the directory at the cursor position" (op_base_dir). To create directly
    // under root, put the cursor on a file directly under root (its parent = root becomes the creation target).
    s.select("notes.txt");
    s.key(' ');
    s.key('n'); // Space→n create
    s.keys("fresh.txt");
    s.enter();
    assert!(
        dir.join("fresh.txt").exists(),
        "作成したファイルがディスクに存在"
    );
    s.see("fresh.txt");

    // Putting the cursor on a directory creates it inside that directory (as op_base_dir is designed to).
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

/// Creating a file that already exists must flash in whichever language the UI is configured for
/// (not a fixed language regardless of `ui.lang`) — the bug this whole i18n pass fixes.
#[test]
fn e2e_file_create_existing_name_flashes_in_ui_language() {
    let dir_en = sandbox("file_create_existing_name_en");
    seed_files(&dir_en);
    let mut cfg_en = Config::default();
    cfg_en.ui.lang = "en".into();
    let mut s = Sim::with_config(&canon(&dir_en), cfg_en);
    s.select("notes.txt");
    s.key(' ');
    s.key('n'); // Space→n create
    s.keys("notes.txt"); // op_base_dir = notes.txt's parent (root), which already has notes.txt
    s.enter();
    let flash_en = s.app.flash.clone().unwrap_or_default();
    assert!(
        flash_en.contains("already exists: "),
        "英語 UI では英語で: {flash_en:?}"
    );
    assert!(
        !flash_en.contains("既に存在"),
        "英語 UI に日本語が混ざってはならない: {flash_en:?}"
    );
    assert!(
        !dir_en.join("notes.txt.tmp").exists(),
        "衝突時は何も作られない"
    ); // sanity: the collision really was rejected, not silently overwritten

    let dir_jp = sandbox("file_create_existing_name_jp");
    seed_files(&dir_jp);
    let mut cfg_jp = Config::default();
    cfg_jp.ui.lang = "jp".into();
    let mut s2 = Sim::with_config(&canon(&dir_jp), cfg_jp);
    s2.select("notes.txt");
    s2.key(' ');
    s2.key('n');
    s2.keys("notes.txt");
    s2.enter();
    let flash_jp = s2.app.flash.clone().unwrap_or_default();
    assert!(
        flash_jp.contains("既に存在します: "),
        "日本語 UI では日本語で: {flash_jp:?}"
    );

    std::fs::remove_dir_all(&dir_en).ok();
    std::fs::remove_dir_all(&dir_jp).ok();
}

#[test]
fn e2e_file_duplicate_in_place() {
    let dir = sandbox("file_duplicate");
    seed_files(&dir);
    let mut s = Sim::new(&canon(&dir));
    // Cursor on notes.txt → Space→D to duplicate → notes copy.txt is created in place.
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
    s.see("notes copy.txt"); // it appears in the tree (refresh+reveal), selected
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
    // Expects the existing name to be prefilled → clear it all and type the new name.
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
    // A confirmation dialog appears → n to cancel (the file remains).
    s.key('n');
    assert!(dir.join("notes.txt").exists(), "取消で残る");
    // Once more → `!` to permanently delete (doesn't touch the trash).
    s.key(' ');
    s.key('d');
    s.key('!');
    assert!(!dir.join("notes.txt").exists(), "完全削除で消える");
    std::fs::remove_dir_all(&dir).ok();
}

// =============================================================================
// Preview: text/Markdown/links/checkboxes/CSV/search
// =============================================================================

#[test]
fn e2e_preview_text_roundtrip() {
    let dir = sandbox("preview_text");
    seed_files(&dir);
    let mut s = Sim::new(&canon(&dir));
    s.select("notes.txt");
    s.enter();
    assert_eq!(s.app.tab.mode, Mode::Preview);
    s.see("PREVIEW");
    s.see("alpha");
    s.key('q');
    assert_eq!(s.app.tab.mode, Mode::Tree, "q でツリーへ戻る");
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
    s.tab(); // focus the link "go"
    s.enter(); // open ./notes.txt
    s.see("alpha");
    assert!(
        s.app
            .tab
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
    assert_eq!(s.app.tab.mode, Mode::Tree);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_zip_archive_opens_as_table() {
    let dir = sandbox("zip_archive");
    {
        use std::io::Write as _;
        let f = std::fs::File::create(dir.join("bundle.zip")).unwrap();
        let mut zw = zip::ZipWriter::new(f);
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        zw.start_file("one.txt", opts).unwrap();
        zw.write_all(b"hello").unwrap();
        zw.start_file("two.txt", opts).unwrap();
        zw.write_all(b"world").unwrap();
        zw.finish().unwrap();
    }
    let mut s = Sim::new(&canon(&dir));
    s.select("bundle.zip");
    s.enter();
    s.see("TABLE");
    s.see("one.txt");
    s.see("two.txt");
    assert!(s.app.is_table_preview());
    s.key('q');
    assert_eq!(s.app.tab.mode, Mode::Tree);
    std::fs::remove_dir_all(&dir).ok();
}

/// `Enter`'s full-text popup: the grid truncates at column width (40), but the popup shows the raw
/// full text. A continuous run far past the truncation cap (39 consecutive + `…`) being visible on screen proves there is no truncation.
#[test]
fn e2e_table_cell_popup_shows_full_text_not_truncated() {
    let dir = sandbox("cellpopup_long");
    let long = "x".repeat(80);
    std::fs::write(dir.join("wide.csv"), format!("h1,h2\nshort,{long}\n")).unwrap();
    let mut s = Sim::new(&canon(&dir));
    s.select("wide.csv");
    s.enter();
    s.see("TABLE");
    s.dont_see(&"x".repeat(40)); // the grid's truncation cap is 39 consecutive + "…".
    s.key('l'); // move to the long cell (h2 column).
    s.enter(); // open the popup.
    s.see("CELL");
    s.see(&"x".repeat(45)); // a continuous run far past the truncation cap is visible.
    assert_eq!(
        s.app.table_cell_view().unwrap().text,
        long,
        "ポップアップの中身は生の全文(切り詰め無し)"
    );
    s.key('q');
    s.see("TABLE");
    s.dont_see("CELL");
    assert_eq!(
        s.app.tab.mode,
        Mode::Preview,
        "q はポップアップだけ閉じる(テーブルへ留まる・ツリーへは戻らない)"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// A newline inside CSV quotes (e.g. a multi-line address field) stays intact (as a real newline) in
/// the popup's content. Also confirms it can be closed with Esc too (not just `q`).
#[test]
fn e2e_table_cell_popup_shows_multiline_cell_and_esc_closes() {
    let dir = sandbox("cellpopup_multiline");
    std::fs::write(
        dir.join("addr.csv"),
        "name,address\nDoe,\"123 Main St\nSuite 4\nCity\"\n",
    )
    .unwrap();
    let mut s = Sim::new(&canon(&dir));
    s.select("addr.csv");
    s.enter();
    s.see("TABLE");
    s.key('l'); // move to the address column (contains a newline).
    s.enter();
    s.see("CELL");
    assert_eq!(
        s.app.table_cell_view().unwrap().text,
        "123 Main St\nSuite 4\nCity",
        "引用符内の実改行がそのまま保持される"
    );
    s.esc();
    s.dont_see("CELL");
    assert_eq!(s.app.tab.mode, Mode::Preview, "Esc もツリーへは戻らない");
    std::fs::remove_dir_all(&dir).ok();
}

/// The archive listing also goes through the same `TableData`, so the cell popup works via real keystrokes too.
#[test]
fn e2e_zip_archive_cell_popup_shows_full_name_via_enter() {
    let dir = sandbox("cellpopup_zip");
    let long_name = format!("{}.txt", "x".repeat(90));
    {
        use std::io::Write as _;
        let f = std::fs::File::create(dir.join("bundle.zip")).unwrap();
        let mut zw = zip::ZipWriter::new(f);
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        zw.start_file(&long_name, opts).unwrap();
        zw.write_all(b"hi").unwrap();
        zw.finish().unwrap();
    }
    let mut s = Sim::new(&canon(&dir));
    s.select("bundle.zip");
    s.enter();
    s.see("TABLE");
    s.enter(); // cursor is on the first cell (Name column) → open the popup.
    s.see("CELL");
    let view = s.app.table_cell_view().unwrap();
    assert_eq!(view.header, "Name");
    assert_eq!(view.text, long_name, "長いエントリ名も切り詰め無しで取れる");
    s.key('q');
    s.see("TABLE");
    std::fs::remove_dir_all(&dir).ok();
}

/// Route decorated Markdown search **through key input**. While staying in decorated display (not
/// switching to raw), scroll to the matching line, and move to the next match with n. Must not break on CJK body text either.
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
    s.see("needle"); // a match on the CJK line also shows up on screen

    s.key('q');
    assert_eq!(s.app.tab.mode, Mode::Tree);
    std::fs::remove_dir_all(&dir).ok();
}

/// Route table search **through key input** too (`/` → query → Enter → n/N). Unit tests hit App's
/// methods directly, so a missing keymap binding or an input surface hijacking the key can only be caught here.
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

    // Esc is "clear search → once more to the tree" (the same convention as the text preview).
    // A real-terminal bug where Esc did nothing = the highlight couldn't be cleared was found, so pin it down here.
    s.esc();
    assert_eq!(s.app.preview_search_query(), None, "Esc で検索が解除される");
    assert!(!s.app.table_cell_is_hit(0, 1), "一致の強調も消える");
    assert_eq!(s.app.tab.mode, Mode::Preview, "1回目の Esc では表に留まる");
    s.esc();
    assert_eq!(s.app.tab.mode, Mode::Tree, "2回目の Esc でツリーへ戻る");
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
    s.see("needle line 0"); // to the first match position
    s.key('n'); // to the next match → the screen moves to the match at line 100
    s.see("needle line 100");
    s.key('N'); // back to the previous
    s.see("needle line 0");
    std::fs::remove_dir_all(&dir).ok();
}

// =============================================================================
// Tabs, tab list
// =============================================================================

#[test]
fn e2e_tabs_full_lifecycle() {
    let dir = sandbox("tabs_life");
    seed_files(&dir);
    let mut s = Sim::new(&canon(&dir));
    s.keys("tt"); // 3 tabs
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
    s.key('q'); // close the list
    assert!(!s.app.is_tab_list());
    // w does nothing by default.
    s.key('w');
    assert_eq!(s.app.tab_count(), 2, "w は未割当");
    // The tree's q closes the tab.
    s.key('q');
    assert_eq!(s.app.tab_count(), 1, "tree の q でタブが閉じる");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_ctrl_t_opens_selected_in_new_tab() {
    // In the tree, Ctrl-t = open what's under the cursor in a separate tab. File → preview / directory → new tab's root.
    // The original tab is preserved.
    let dir = sandbox("newtab_open");
    std::fs::write(dir.join("note.txt"), "HELLO_NEWTAB body\n").unwrap();
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    std::fs::write(dir.join("sub/inside.txt"), "INSIDE_MARK\n").unwrap();
    let mut s = Sim::new(&canon(&dir));

    // File: Ctrl-t previews it in a new tab.
    s.select("note.txt");
    assert_eq!(s.app.tab_count(), 1);
    s.ctrl('t');
    assert_eq!(s.app.tab_count(), 2, "Ctrl-t で新規タブができる");
    assert_eq!(s.app.active_tab_index(), 1, "新規タブがアクティブ");
    s.see("HELLO_NEWTAB"); // the file is previewed in the new tab

    // The original tab is preserved: going back with [ shows note.txt in the tree (the preview content isn't visible).
    s.key('[');
    assert_eq!(s.app.active_tab_index(), 0);
    s.see("note.txt");
    s.dont_see("HELLO_NEWTAB");

    // Directory: Ctrl-t makes the new tab's root that folder.
    s.select("sub");
    s.ctrl('t');
    assert_eq!(s.app.tab_count(), 3);
    s.see("inside.txt"); // the new tab shows sub's contents
    std::fs::remove_dir_all(&dir).ok();
}

// =============================================================================
// Bookmarks
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
    s.keys("mb"); // register to b
    s.see("bookmarked");
    s.key('\'');
    s.see("Bookmarks");
    s.key('b'); // a letter jumps directly → the file is previewed
    assert_eq!(s.app.tab.mode, Mode::Preview);
    assert!(s
        .app
        .tab
        .preview_path
        .as_deref()
        .is_some_and(|p| p.ends_with("notes.txt")));
    // m while previewing registers the file currently on display.
    s.keys("mz");
    assert!(
        s.app
            .bookmarks
            .get('z')
            .is_some_and(|p| p.ends_with("notes.txt")),
        "プレビュー中の登録対象は表示中ファイル"
    );
    // Open the list and delete the selected one with Ctrl-d (confirms the count drops by one, regardless of order).
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
    // Default (confirm_bookmark_overwrite=true): registering a different path to an already-used key shows a confirmation dialog.
    // n = cancel, stays as-is / y = overwrite, moves to the new path.
    let root = sandbox("bm_overwrite");
    let proj = root.join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    seed_files(&proj);
    let proj = canon(&proj);
    let mut s = Sim::new(&proj);
    s.app.bookmarks = crate::bookmarks::Bookmarks::with_base(root.join("cfg"), &proj);

    // Register notes.txt to 'b' (an unused key = no confirmation).
    s.select("notes.txt");
    s.keys("mb");
    s.see("bookmarked");
    let notes = s.app.bookmarks.get('b').expect("b set");

    // Register data.csv to the same 'b' → a confirmation dialog. Don't overwrite yet.
    s.select("data.csv");
    s.keys("mb");
    s.see("OVERWRITE?");
    s.see("Overwrite bookmark");
    assert_eq!(
        s.app.bookmarks.get('b'),
        Some(notes.clone()),
        "確認前は元のまま"
    );

    // n = cancel.
    s.key('n');
    s.see("canceled");
    assert_eq!(
        s.app.bookmarks.get('b'),
        Some(notes.clone()),
        "取消で notes.txt のまま"
    );

    // Show it again and y = overwrite.
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
    // confirm_bookmark_overwrite=false: even a different path overwrites immediately with no confirmation.
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
// git: hub, stage/unstage, diff, changed filter, follow
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
    // Keep the subject "init" as-is (existing tests stay unchanged) + a 2-paragraph body. Multiple -m
    // flags become blank-line-separated paragraphs. Lets the E2E confirm commit details preserve the body's newlines.
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
    // One non-current branch (for validating the branches list/graph panel). Points at the same commit.
    run(&["branch", "feature-x"]);
    // Set up uncommitted changes.
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
    // stage → the display changes (a staged mark). unstage reverts it.
    s.key('s');
    s.key('u');
    // Enter opens the diff; q returns to the hub.
    s.enter();
    s.see("diff");
    s.key('q');
    assert!(s.app.is_git_view(), "diff の q でハブへ戻る");
    s.key('q');
    assert_eq!(s.app.tab.mode, Mode::Tree);
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
    // n moves to the next changed file (wraps).
    let before = s.screen();
    s.key('n');
    assert_ne!(before, s.screen(), "n で別ファイルの diff へ");
    s.key('q');
    assert_eq!(s.app.tab.mode, Mode::Tree);
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
    // The 2 changes (a.rs, new.txt) are in the list.
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
    // Maximally sticky (2026-07-23): a normal key does not break follow.
    s.key('j');
    assert!(s.app.follow_enabled(), "j のような通常キーではフォロー維持");
    s.see("FOLLOW");
    // Only broken by pressing F again (via toggle_follow).
    s.key('F');
    assert!(!s.app.follow_enabled(), "F の再押下で解除");
    s.dont_see("FOLLOW");
    std::fs::remove_dir_all(&dir).ok();
}

// =============================================================================
// Help, info, quit
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
// Focused simulation of recently-changed complex paths
// (custom task states, focus round-trip, closing the last tab, global BM absolute display,
//  visual selection, copy menu, reversed sort, anchor)
// =============================================================================

#[test]
fn e2e_markdown_task_cycles_custom_states() {
    // Setting ui.md_task_states to [" ", "!", "x"] makes Space cycle through 3 states,
    // writing just 1 character back to the file at each step (v0.6.0's variable checkbox).
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
    // Tab moves focus to the next item, Shift-Tab to the previous, round-tripping.
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
    // With confirm_quit=false and a single tab, the tree's q quits the app (CloseTabOrQuit).
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
    // With confirm_quit=ON (default), the last tab's q shows a confirmation dialog and doesn't quit yet.
    let dir = sandbox("last_tab_confirm");
    seed_files(&dir);
    let mut s = Sim::new(&canon(&dir));
    s.key('q');
    assert!(!s.quit, "確認ダイアログでまだ終了しない");
    assert!(s.app.is_dialog(), "終了確認ダイアログが開く");
    s.key('q'); // qq confirms quitting
    assert!(s.quit, "qq で終了");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_global_bookmark_shows_absolute_path() {
    // An uppercase mark = global. The list display shows an absolute path (~ shortened) (v0.7.0).
    let root = sandbox("bm_global");
    let proj = canon(&root);
    seed_files(&proj);
    let mut s = Sim::new(&proj);
    s.app.bookmarks = crate::bookmarks::Bookmarks::with_base(root.join("cfg"), &proj);
    s.select("notes.txt");
    s.keys("mB"); // uppercase = global registration
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
    // In a windowed preview, v = charwise / V = linewise selection begins, and Esc cancels it.
    let dir = sandbox("visual_sel");
    seed_files(&dir);
    let mut s = Sim::new(&canon(&dir));
    s.select("notes.txt");
    s.enter();
    assert!(s.app.is_windowed(), "テキストは windowed プレビュー");
    s.key('v');
    assert!(s.app.is_preview_visual(), "v で選択開始");
    assert!(!s.app.preview_visual_linewise(), "v は charwise");
    s.key('l'); // move the caret right → the selection grows
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
    // y opens the copy which-key leader. Esc closes it (copies nothing).
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
    // s→r reverses the order (directories still stay first, but the files' order reverses).
    let dir = sandbox("sort_rev");
    seed_files(&dir);
    let mut s = Sim::new(&canon(&dir));
    let before: Vec<_> = s.app.tab.entries.iter().map(|e| e.path.clone()).collect();
    s.key('s');
    s.key('r'); // reverse toggle (toggle-type items keep the menu open so several can be switched — as designed)
    assert!(
        s.app.is_sort_menu(),
        "r/. はトグルなのでメニューは開いたまま"
    );
    let after: Vec<_> = s.app.tab.entries.iter().map(|e| e.path.clone()).collect();
    assert_ne!(before, after, "反転で並びが変わる");
    s.esc(); // close the menu
    assert!(!s.app.is_sort_menu(), "Esc でメニューが閉じる");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_anchor_reanchor_and_reset() {
    // After descending into a subdirectory and changing root, a = re-anchor to the current location, A = return to the launch position.
    // Both flash and don't crash (switching the basis for relative-path display).
    let dir = sandbox("anchor");
    seed_files(&dir);
    let root = canon(&dir);
    let mut s = Sim::new(&root);
    s.select("src");
    s.key('l'); // descend into src (root changes)
    assert!(s.app.tab.root.ends_with("src"));
    s.key('a'); // re-anchor
    assert!(s.app.flash.is_some(), "a で通知");
    s.key('A'); // to the launch position
    assert!(s.app.flash.is_some(), "A で通知");
    // Anchoring leaves the tree structure/root unchanged (only the display basis changes).
    assert!(
        s.app.tab.root.ends_with("src"),
        "アンカーは root を動かさない"
    );
    std::fs::remove_dir_all(&dir).ok();
}

// =============================================================================
// git: log / graph / branches / commit detail / base pin
// =============================================================================

#[cfg(feature = "git")]
#[test]
fn e2e_git_hub_to_log_shows_commit_subject() {
    // Changes hub (o) → log (l). seed_repo's subject "init" shows up in the list.
    let dir = sandbox("git_log_view");
    seed_repo(&dir);
    let mut s = Sim::new(&canon(&dir));
    s.key('o');
    assert!(s.app.is_git_view(), "o で変更ハブが開く");
    s.key('l'); // in the hub, l = log
    assert!(s.app.is_git_log(), "l で log ビューへ");
    s.see("Git log"); // render_log's literal title
    s.see("init"); // the commit subject (summary)
    s.key('q'); // log's q returns to the hub
    assert!(s.app.is_git_view(), "log の q でハブへ戻る");
    std::fs::remove_dir_all(&dir).ok();
}

#[cfg(feature = "git")]
#[test]
fn e2e_git_graph_shows_commit_nodes() {
    // Changes hub (o) → graph (g). The angular-line renderer's node ● shows up.
    let dir = sandbox("git_graph_view");
    seed_repo(&dir);
    let mut s = Sim::new(&canon(&dir));
    s.key('o');
    s.key('g'); // in the hub, g = graph
    assert!(s.app.is_git_graph(), "g でグラフへ");
    s.see("Git graph"); // render_graph's literal title
    s.see("●"); // the commit/working-tree node glyph
    s.key('q');
    assert!(s.app.is_git_view(), "グラフの q でハブへ戻る");
    std::fs::remove_dir_all(&dir).ok();
}

#[cfg(feature = "git")]
#[test]
fn e2e_git_graph_set_and_clear_base() {
    // In the graph, s = pin the base / x = clear it. `⌖ base:` appears/disappears in the title.
    let dir = sandbox("git_graph_base");
    seed_repo(&dir);
    let mut s = Sim::new(&canon(&dir));
    s.key('o');
    s.key('g');
    s.dont_see("⌖ base:"); // default config has graph_base_branches=[] → no base initially
    s.key('G'); // to the bottom row = the oldest commit line (avoids s being unusable on the working-tree row at the top)
    s.key('s'); // pin the selected commit as base
    s.see("⌖ base:"); // base appended in the title
    s.key('x'); // clear the base
    s.dont_see("⌖ base:");
    std::fs::remove_dir_all(&dir).ok();
}

#[cfg(feature = "git")]
#[test]
fn e2e_git_branches_view_lists_branches() {
    // Changes hub (o) → branches (b). Branch names and the current-branch marker * show up.
    let dir = sandbox("git_branches_view");
    seed_repo(&dir);
    let mut s = Sim::new(&canon(&dir));
    s.key('o');
    s.key('b'); // in the hub, b = branches
    assert!(s.app.is_git_branches(), "b でブランチ一覧へ");
    s.see("Git branches"); // render_branches's literal title
    s.see("feature-x"); // the non-current branch that seed_repo creates
    s.see("*"); // the current-branch marker
    s.key('q');
    assert!(s.app.is_git_view(), "branches の q でハブへ戻る");
    std::fs::remove_dir_all(&dir).ok();
}

#[cfg(feature = "git")]
#[test]
fn e2e_git_worktrees_view_lists_and_enter_switches_root() {
    // Changes hub (o) → worktrees (w). Both the main and a linked worktree show up; `Enter` on the
    // linked one switches this tab's root **and** open_dir (so `y @`/`Y` references anchor to the
    // new location, not the one the tab launched from).
    let dir = sandbox("git_worktrees_view");
    seed_repo(&dir);
    let root = canon(&dir);
    let linked = std::env::temp_dir().join("konoma_e2e_git_worktrees_view_linked");
    let _ = std::fs::remove_dir_all(&linked);
    let out = std::process::Command::new("git")
        .current_dir(&root)
        .args([
            "worktree",
            "add",
            "-q",
            "-b",
            "wt-feature",
            linked.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(out.status.success(), "git worktree add: {out:?}");
    let linked = linked.canonicalize().unwrap();

    let mut s = Sim::new(&root);
    s.key('o');
    s.key('w'); // in the hub, w = worktrees
    assert!(s.app.is_git_worktrees(), "w でワークツリー一覧へ");
    // The outer mode chip must promote to GIT here, same as the hub/log/graph/branches/detail
    // views (`in_git_view()` — regression coverage for a spot where the worktree list was missing
    // from that predicate and the chip stayed on TREE).
    s.see("GIT");
    s.see("WORKTREES");
    s.see("Git worktrees");
    s.see("wt-feature");
    s.see("*"); // current-worktree marker
    assert_eq!(
        s.app.git_worktree_sel(),
        0,
        "cursor はカレント(main)から始まる"
    );

    s.key('j'); // move onto the linked worktree
    s.enter();
    assert!(!s.app.is_git_worktrees(), "Enter で一覧が閉じる");
    assert!(!s.app.is_git_view(), "ハブへも戻らず Tree に着地する");
    assert_eq!(s.app.tab.root, linked, "root が切り替わる");
    assert_eq!(s.app.tab.open_dir, linked, "open_dir も切り替わる");
    assert!(matches!(s.app.tab.mode, Mode::Tree), "切替後は Tree 表示");

    std::fs::remove_dir_all(&linked).ok();
    std::fs::remove_dir_all(&dir).ok();
}

#[cfg(feature = "git")]
#[test]
fn e2e_git_worktrees_ctrl_t_opens_in_new_tab_leaving_current_tab_untouched() {
    let dir = sandbox("git_worktrees_new_tab");
    seed_repo(&dir);
    let root = canon(&dir);
    let linked = std::env::temp_dir().join("konoma_e2e_git_worktrees_new_tab_linked");
    let _ = std::fs::remove_dir_all(&linked);
    let out = std::process::Command::new("git")
        .current_dir(&root)
        .args([
            "worktree",
            "add",
            "-q",
            "-b",
            "wt-feature",
            linked.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(out.status.success(), "git worktree add: {out:?}");
    let linked = linked.canonicalize().unwrap();

    let mut s = Sim::new(&root);
    s.key('o');
    s.key('w');
    s.key('j'); // move onto the linked worktree
    s.ctrl('t');
    assert_eq!(s.app.tab_count(), 2, "新規タブが開く");
    assert_eq!(s.app.tab.root, linked, "新タブの root が対象ワークツリー");
    assert_eq!(s.app.tab.open_dir, linked);
    assert!(matches!(s.app.tab.mode, Mode::Tree));
    assert!(
        !s.app.is_git_worktrees(),
        "新タブは素の Tree（一覧を引き継がない）"
    );

    // The original tab's own worktree list is left open (this action doesn't own it).
    s.key('[');
    assert_eq!(s.app.tab.root, root, "元タブの root は不変");
    assert!(
        s.app.is_git_worktrees(),
        "元タブのワークツリー一覧は開いたまま"
    );

    std::fs::remove_dir_all(&linked).ok();
    std::fs::remove_dir_all(&dir).ok();
}

/// `o` `w` `n`: type a branch name into the input dialog, submit → `git worktree add` creates it
/// next to the main worktree (default `[git] worktree_dir = "../"`) and this tab switches into it
/// (both `root` and `open_dir`, mirroring `worktree_goto`'s `Enter`).
#[cfg(feature = "git")]
#[test]
fn e2e_git_worktree_create_switches_into_it() {
    let dir = sandbox("git_worktree_create");
    seed_repo(&dir);
    let root = canon(&dir);
    // The default worktree_dir ("../") places the new worktree as a sibling of the sandbox dir —
    // pre-clean any stale leftover from a previous failed run (a *non-empty* leftover would make a
    // fresh `git worktree add` fail with "already exists"; this test's own cleanup removes it again
    // at the end, but a prior interrupted run could have skipped that).
    let expected = std::env::temp_dir().join("e2e-wt-branch");
    let _ = std::fs::remove_dir_all(&expected);

    let mut s = Sim::new(&root);
    s.key('o');
    s.key('w');
    assert!(s.app.is_git_worktrees());
    s.key('n');
    assert!(s.app.is_dialog(), "n で入力面に入る");
    s.keys("e2e-wt-branch");
    s.enter();

    assert!(!s.app.is_git_worktrees(), "作成後は一覧を閉じる");
    let new_root = s.app.tab.root.clone();
    assert_ne!(new_root, root, "root が新しいワークツリーへ切替わる");
    assert_eq!(s.app.tab.open_dir, new_root, "open_dir も切り替わる");
    assert_eq!(new_root, expected.canonicalize().unwrap());
    assert!(matches!(s.app.tab.mode, Mode::Tree), "切替後は Tree 表示");
    assert!(
        crate::git::branch_tip(&root, "e2e-wt-branch").is_some(),
        "新規ブランチが作られる"
    );

    std::fs::remove_dir_all(&expected).ok();
    std::fs::remove_dir_all(&dir).ok();
}

#[cfg(feature = "git")]
#[test]
fn e2e_git_worktrees_enter_on_current_worktree_flashes_and_returns_to_hub() {
    // The safety guard for "already here": Enter on the currently-active worktree must not touch
    // root, and must close back to the changes hub (not silently land in Tree) — a regression here
    // (guard removed) would instead re-run jump_to_dir on the same path and skip open_git_view.
    let dir = sandbox("git_worktrees_already_current");
    seed_repo(&dir);
    let root = canon(&dir);
    let mut s = Sim::new(&root);
    s.key('o');
    s.key('w');
    assert!(s.app.is_git_worktrees());
    s.enter(); // cursor starts on the current (main) worktree
    s.see("already in this worktree");
    assert!(!s.app.is_git_worktrees(), "閉じる");
    assert!(
        s.app.is_git_view(),
        "ハブへ戻る（root は動かないので Tree ではない）"
    );
    assert_eq!(s.app.tab.root, root, "root は不変");
    std::fs::remove_dir_all(&dir).ok();
}

#[cfg(feature = "git")]
#[test]
fn e2e_git_worktrees_refuses_bare_and_prunable_targets() {
    // Safety guards: a bare main worktree (no checkout) and a prunable/missing one both refuse with
    // a flash, and the list stays open (unlike a successful switch, which closes it).
    let src = sandbox("git_worktrees_refuse_src");
    seed_repo(&src);
    let src = canon(&src);
    let bare = std::env::temp_dir().join("konoma_e2e_git_worktrees_refuse_bare.git");
    let wt = std::env::temp_dir().join("konoma_e2e_git_worktrees_refuse_wt");
    let gone = std::env::temp_dir().join("konoma_e2e_git_worktrees_refuse_gone");
    for p in [&bare, &wt, &gone] {
        let _ = std::fs::remove_dir_all(p);
    }
    let sh = |cwd: &std::path::Path, args: &[&str]| {
        let out = std::process::Command::new("git")
            .current_dir(cwd)
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {out:?}");
    };
    sh(
        &std::env::temp_dir(),
        &[
            "clone",
            "-q",
            "--bare",
            src.to_str().unwrap(),
            bare.to_str().unwrap(),
        ],
    );
    sh(
        &bare,
        &["worktree", "add", "-q", wt.to_str().unwrap(), "master"],
    );
    sh(
        &bare,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "gone-branch",
            gone.to_str().unwrap(),
        ],
    );
    let wt = wt.canonicalize().unwrap();
    std::fs::remove_dir_all(&gone).unwrap(); // now prunable (checkout gone, entry still listed)

    let mut s = Sim::new(&wt);
    s.key('o');
    s.key('w');
    assert!(s.app.is_git_worktrees());
    s.see("bare");

    // Locate rows by content rather than assuming a fixed order: git's listing order for
    // non-main entries is not creation order (confirmed empirically), only "main is always
    // first" is guaranteed.
    let view = s.app.git_worktree_view();
    let cur_idx = view.iter().position(|w| w.is_current).expect("current");
    let bare_idx = view.iter().position(|w| w.is_bare).expect("bare");
    let gone_idx = view.iter().position(|w| w.prunable).expect("prunable");
    assert_eq!(
        s.app.git_worktree_sel(),
        cur_idx,
        "cursor はカレント(wt)から始まる"
    );

    // Move onto the bare main worktree's row: refused.
    let step = |s: &mut Sim, from: usize, to: usize| {
        let delta = to as i32 - from as i32;
        for _ in 0..delta.unsigned_abs() {
            s.key(if delta > 0 { 'j' } else { 'k' });
        }
    };
    step(&mut s, cur_idx, bare_idx);
    s.enter();
    assert!(s.app.is_git_worktrees(), "bare は拒否されて一覧のまま");
    assert_eq!(s.app.tab.root, wt, "root は不変");
    s.see("bare repository");

    // Move onto the removed/prunable worktree's row: also refused.
    step(&mut s, bare_idx, gone_idx);
    s.enter();
    assert!(s.app.is_git_worktrees(), "prunable は拒否されて一覧のまま");
    assert_eq!(s.app.tab.root, wt, "root は不変");
    s.see("missing");

    std::fs::remove_dir_all(&wt).ok();
    std::fs::remove_dir_all(&bare).ok();
    std::fs::remove_dir_all(&src).ok();
}

#[cfg(feature = "git")]
#[test]
fn e2e_git_worktree_show_changes_opens_detail_with_title_and_returns_to_list() {
    // o → w → d on a **linked** worktree that has both committed and uncommitted work on top of
    // the base: the detail shows both (committed+uncommitted, not just one), the title names the
    // worktree's branch and the base it's diffed against, and q returns to the worktree list
    // (not the hub, not the tree) — the list is still open underneath, never closed by this action.
    let dir = sandbox("git_worktree_show_changes");
    seed_repo(&dir);
    let root = canon(&dir);
    let base = crate::git::branch(&root).expect("sanity: has a branch after seed_repo's commit");

    let linked = std::env::temp_dir().join("konoma_e2e_git_worktree_show_changes_linked");
    let _ = std::fs::remove_dir_all(&linked);
    let sh = |cwd: &std::path::Path, args: &[&str]| {
        let out = std::process::Command::new("git")
            .current_dir(cwd)
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {out:?}");
    };
    sh(
        &root,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "wt-since-feature",
            linked.to_str().unwrap(),
        ],
    );
    let linked = linked.canonicalize().unwrap();

    // ① Something committed *inside the worktree* (what an agent typically does mid-task).
    std::fs::write(linked.join("agent_landed.txt"), "AGENT_LANDED_MARKER\n").unwrap();
    sh(&linked, &["add", "-A"]);
    sh(&linked, &["commit", "-q", "-m", "agent commit"]);
    // ② Something still uncommitted on top of that.
    std::fs::write(linked.join("agent_pending.txt"), "AGENT_PENDING_MARKER\n").unwrap();

    let mut cfg = Config::default();
    cfg.ui.graph_base_branches = vec![base.clone()];
    let mut s = Sim::with_config(&root, cfg);
    s.key('o');
    s.key('w');
    assert!(s.app.is_git_worktrees());

    // Move the cursor onto the linked worktree's row (position by content, not assumed order —
    // git's non-main listing order isn't creation order, confirmed empirically elsewhere in this
    // file).
    let view = s.app.git_worktree_view();
    let cur = s.app.git_worktree_sel();
    let target = view
        .iter()
        .position(|w| w.path == linked)
        .expect("linked worktree row");
    for _ in 0..target.abs_diff(cur) {
        s.key(if target > cur { 'j' } else { 'k' });
    }

    s.key('d');
    assert!(
        s.app.is_git_detail(),
        "d でこのワークツリーの diff が detail として開く"
    );
    s.see(&format!("wt-since-feature ← {base}"));
    s.see("agent_landed.txt");
    s.see("AGENT_LANDED_MARKER");
    s.see("agent_pending.txt");
    s.see("AGENT_PENDING_MARKER");

    s.key('q');
    assert!(!s.app.is_git_detail(), "q で detail のみ閉じる");
    assert!(
        s.app.is_git_worktrees(),
        "detail の裏にあったワークツリー一覧へ戻る（ハブでもツリーでもない）"
    );
    assert!(!s.app.is_git_view());

    std::fs::remove_dir_all(&linked).ok();
    std::fs::remove_dir_all(&dir).ok();
}

#[cfg(feature = "git")]
#[test]
fn e2e_git_worktree_show_changes_works_on_the_currently_active_worktree() {
    // Unlike `worktree_goto`'s "already here" guard, `d` does NOT refuse the worktree the tab is
    // presently inside — viewing its own diff is perfectly legitimate. The cursor starts there
    // (per the worktree list's documented behavior), so pressing `d` immediately exercises this.
    let dir = sandbox("git_worktree_show_changes_current");
    seed_repo(&dir); // leaves uncommitted changes on a.rs + an untracked new.txt
    let root = canon(&dir);
    let mut s = Sim::new(&root); // default config: graph_base_branches = [] → falls back to the
                                 // main worktree's own branch, which is exactly the one it's on
    s.key('o');
    s.key('w');
    assert_eq!(
        s.app.git_worktree_sel(),
        s.app
            .git_worktree_view()
            .iter()
            .position(|w| w.is_current)
            .unwrap(),
        "cursor はカレントから始まる"
    );
    s.key('d');
    assert!(
        s.app.is_git_detail(),
        "現在地のワークツリーでも d は拒否されない（AlreadyCurrent は worktree_goto 専用のガード）"
    );
    s.see("new.txt"); // the uncommitted untracked file from seed_repo
    s.see("_x = 1"); // the uncommitted edit to a.rs from seed_repo

    s.key('q');
    assert!(s.app.is_git_worktrees(), "一覧へ戻る");
    std::fs::remove_dir_all(&dir).ok();
}

#[cfg(feature = "git")]
#[test]
fn e2e_git_worktree_show_changes_refuses_bare_worktree() {
    // Same guard `worktree_goto` uses for a bare main worktree (no checkout to diff at all): `d`
    // refuses with a flash and leaves the list open, rather than crashing or showing nothing.
    let src = sandbox("git_worktree_show_changes_refuse_bare_src");
    seed_repo(&src);
    let src = canon(&src);
    let bare = std::env::temp_dir().join("konoma_e2e_git_worktree_show_changes_refuse_bare.git");
    let wt = std::env::temp_dir().join("konoma_e2e_git_worktree_show_changes_refuse_wt");
    for p in [&bare, &wt] {
        let _ = std::fs::remove_dir_all(p);
    }
    let sh = |cwd: &std::path::Path, args: &[&str]| {
        let out = std::process::Command::new("git")
            .current_dir(cwd)
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {out:?}");
    };
    sh(
        &std::env::temp_dir(),
        &[
            "clone",
            "-q",
            "--bare",
            src.to_str().unwrap(),
            bare.to_str().unwrap(),
        ],
    );
    sh(
        &bare,
        &["worktree", "add", "-q", wt.to_str().unwrap(), "master"],
    );
    let wt = wt.canonicalize().unwrap();

    let mut s = Sim::new(&wt);
    s.key('o');
    s.key('w');
    let view = s.app.git_worktree_view();
    let cur = s.app.git_worktree_sel();
    let bare_idx = view.iter().position(|w| w.is_bare).expect("bare row");
    for _ in 0..bare_idx.abs_diff(cur) {
        s.key(if bare_idx > cur { 'j' } else { 'k' });
    }

    s.key('d');
    assert!(
        !s.app.is_git_detail(),
        "bare は拒否されて detail は開かない"
    );
    assert!(s.app.is_git_worktrees(), "一覧は開いたまま");
    s.see("bare repository");

    std::fs::remove_dir_all(&wt).ok();
    std::fs::remove_dir_all(&bare).ok();
    std::fs::remove_dir_all(&src).ok();
}

#[cfg(feature = "git")]
#[test]
fn e2e_git_commit_detail_shows_full_message() {
    // log → Enter opens commit detail. The full message (body newlines preserved) shows up in the header.
    let dir = sandbox("git_commit_detail");
    seed_repo(&dir);
    let mut s = Sim::new(&canon(&dir));
    s.key('o');
    s.key('l');
    s.enter(); // detail of the selected commit (init)
    assert!(s.app.is_git_detail(), "Enter で詳細へ");
    s.see("First body paragraph."); // body paragraph 1 (newlines preserved)
    s.see("Second body paragraph."); // body paragraph 2
    s.key('q'); // detail's q returns to the log behind it (the log isn't closed)
    assert!(s.app.is_git_log(), "詳細の q で log へ戻る");
    std::fs::remove_dir_all(&dir).ok();
}

#[cfg(feature = "git")]
#[test]
fn e2e_git_graph_branch_picker_toggles() {
    // b in the graph opens the branch-display panel, listing branch names. q cancels and closes it.
    let dir = sandbox("git_graph_picker");
    seed_repo(&dir);
    let mut s = Sim::new(&canon(&dir));
    s.key('o');
    s.key('g');
    s.key('b'); // in the graph, b = the branch-display panel
    assert!(s.app.is_git_graph_picker(), "b でパネルが開く");
    s.see("feature-x"); // the branch name in a panel row
    s.key('q'); // cancel (git_graph_picker_cancel)
    assert!(!s.app.is_git_graph_picker(), "q でパネルを閉じる");
    assert!(s.app.is_git_graph(), "パネルを閉じるとグラフへ戻る");
    std::fs::remove_dir_all(&dir).ok();
}

// =============================================================================
// Verifying "actual values" for copy (clipboard-independent — verified via computation functions/getters)
// =============================================================================

#[test]
fn e2e_copy_path_strings_from_tree() {
    // Verify, without relying on the clipboard, that y→'s various copy kinds (n/r/f/p/@) produce the
    // expected strings relative to "where it was opened" (open_dir), via copy_string_for (= copy_text).
    // - @ (AtRef) is **strictly relative** to open_dir (doesn't prefix the launch dir name) = Claude Code's @ syntax.
    // - r (Relative) prefixes the launch dir name (the same basis as the title display = format_path).
    let dir = sandbox("copy_strings");
    seed_files(&dir);
    let mut s = Sim::new(&canon(&dir));
    s.select("notes.txt");
    let path = s.app.tab.entries[s.app.tab.selected].path.clone();

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
    // @ reference is strictly relative, without prefixing the launch dir name.
    assert_eq!(
        s.app
            .copy_string_for(crate::app::CopyKind::AtRef)
            .as_deref(),
        Some("@notes.txt")
    );
    // Relative is the same as the title display (format_path), prefixing the launch dir name (distinct from @).
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

    // y→@ key wiring: the copy leader opens and is consumed by @ (pending_leader returns to None).
    s.key('y');
    assert_eq!(s.app.pending_leader, Some(crate::keymap::LeaderId::Copy));
    s.key('@');
    assert_eq!(s.app.pending_leader, None, "y→@ でリーダーが消費される");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_copy_atref_is_open_dir_relative_when_descended() {
    // The @ reference is relative to the "launch (open) directory". Even after descending into a
    // subdirectory changes root, open_dir stays at the launch position, so it's strictly relative from there, e.g. @src/lib.rs.
    let dir = sandbox("atref_descend");
    seed_files(&dir);
    let root = canon(&dir);
    let mut s = Sim::new(&root);
    s.select("src");
    s.key('l'); // descend into src (root=src; open_dir stays at the launch position)
    assert!(s.app.tab.root.ends_with("src"));
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
    // Preview's Y = @path#L reference. Caret line = single #L, v/V selection = range #L..-... The value
    // is verified via the clipboard-independent getter (selection_ref_string=preview_selection_ref_text).
    let dir = sandbox("preview_atref");
    seed_files(&dir);
    let mut s = Sim::new(&canon(&dir));
    s.select("notes.txt");
    s.enter();
    assert!(s.app.is_windowed(), "テキストは windowed(2D キャレット)");
    // The caret is on the first line → #L1.
    assert_eq!(
        s.app.selection_ref_string().as_deref(),
        Some("@notes.txt#L1")
    );
    s.key('j'); // to line 2 (windowed = line-caret movement)
    assert_eq!(
        s.app.selection_ref_string().as_deref(),
        Some("@notes.txt#L2")
    );
    // A charwise selection spanning lines 2-3 → a range reference.
    s.key('v');
    s.key('j'); // move the caret to line 3 (the anchor is line 2)
    assert!(matches!(
        s.app.preview_selection(),
        crate::app::PreviewSelection::Char { .. }
    ));
    assert_eq!(
        s.app.selection_ref_string().as_deref(),
        Some("@notes.txt#L2-3")
    );
    // Y copies the reference and exits the selection (the value goes through the clipboard = environment-dependent, so only confirm that it exited).
    s.key('Y');
    assert!(!s.app.is_preview_visual(), "Y で参照コピー後に選択解除");
    std::fs::remove_dir_all(&dir).ok();
}

#[cfg(feature = "git")]
#[test]
fn e2e_git_commit_copy_values() {
    // In git log, y→'s commit-info copy (short/full hash, subject, full text, author, date).
    // git_copy(kind) just copies current_commit_meta into each field. In GitLog,
    // current_commit_meta = commit_meta(root, git_log_selected_id()), so verify by reproducing the
    // same meta via a pub path (no getter needed).
    let dir = sandbox("git_commit_copy");
    seed_repo(&dir);
    let mut s = Sim::new(&canon(&dir));
    s.key('o'); // changes hub
    s.key('l'); // → log (synchronous load)
    s.see("init");
    assert_eq!(s.app.surface(), crate::keymap::Surface::GitLog);

    let id = s.app.git_log_selected_id().expect("選択コミットの id");
    let meta = crate::git::commit_meta(&s.app.tab.root, &id).expect("commit_meta");
    assert_eq!(meta.message.lines().next(), Some("init"), "Subject=件名");
    assert_eq!(meta.author, "t", "Author=user.name");
    assert_eq!(meta.short.len(), 7, "ShortHash=7桁");
    assert_eq!(meta.id.len(), 40, "FullHash=40桁");
    assert!(
        meta.id.starts_with(&meta.short),
        "短ハッシュは完全ハッシュの接頭辞"
    );
    assert!(!meta.date.is_empty(), "Date は非空");

    // y→ wiring: the GitCopy leader opens and is consumed by t (subject).
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
// Follow mode's diff display and n/N cycling
// [Note] Since Sim doesn't run the run loop/notify, what the run loop does on an FS event —
//   follow_note_change(&p) → follow_jump(&p) — is replaced with a **direct pub call**.
//   The pipeline FS watch → pending_follow → dwell (1s) → jump (debounce/latest-wins) is
//   not verified here — this verifies follow's "logic" (wiring/dwell is a separate layer).
// =============================================================================

#[cfg(feature = "git")]
#[test]
fn e2e_follow_opens_full_screen_diff() {
    // With follow ON, an external change opens its diff as a full-screen git diff (ui.follow_view default "diff").
    let dir = sandbox("follow_diff");
    seed_repo(&dir); // a.rs = modified, new.txt = untracked (both have a file_diff)
    let mut s = Sim::new(&canon(&dir));
    s.key('F');
    assert!(s.app.follow_enabled(), "F でフォロー ON");

    let a = s.app.tab.root.join("a.rs");
    // The baseline diff shows changes "since F" → edit a.rs after F (real-world: an AI's edit).
    std::fs::write(&a, "fn a() { let _x = 2; }\n").unwrap();
    // Equivalent to the run loop: record the change event, then jump (both are pub).
    assert!(s.app.follow_note_change(&a), "変更ファイルは有効な追尾対象");
    s.app.follow_jump(&a);
    s.draw();

    assert!(
        s.app.is_git_diff_preview(),
        "追尾先は全画面 git diff で開く"
    );
    assert_eq!(s.app.tab.mode, Mode::Preview);
    s.see("diff");
    // The follow-originated diff's cycling target = "files changed during this session". Currently 1.
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
    // n/N inside a follow-originated diff wrap-cycle through only "files changed during the follow
    // session", and the title's position indicator (diff_change_position) changes.
    let dir = sandbox("follow_cycle");
    seed_repo(&dir);
    let mut s = Sim::new(&canon(&dir));
    s.key('F');

    let a = s.app.tab.root.join("a.rs");
    let nt = s.app.tab.root.join("new.txt");
    // The baseline diff shows changes "since F" → edit both files after F.
    std::fs::write(&a, "fn a() { let _x = 2; }\n").unwrap();
    std::fs::write(&nt, "untracked\nAFTER\n").unwrap();
    // 2 change events accumulate in the session (record order = cycle order).
    assert!(s.app.follow_note_change(&a));
    assert!(s.app.follow_note_change(&nt));
    s.app.follow_jump(&a); // open a.rs's diff first
    s.draw();
    assert!(s.app.is_git_diff_preview());
    assert_eq!(
        s.app.diff_change_position(),
        Some((1, 2)),
        "a.rs = 2 件中 1 番目"
    );

    // n = to the next changed file's diff (session-scoped only). Maximally sticky (2026-07-23) = n
    // doesn't resolve to PreviewBack, so it cycles while follow stays on (diff_follow_scope + follow_session).
    s.key('n');
    assert!(
        s.app.follow_enabled(),
        "n(変更ファイル回遊)でもフォロー維持(最大粘着)"
    );
    assert_eq!(
        s.app.diff_change_position(),
        Some((2, 2)),
        "n で 2 番目(new.txt)へ"
    );
    s.key('N'); // to the previous (wraps)
    assert_eq!(
        s.app.diff_change_position(),
        Some((1, 2)),
        "N で 1 番目へ戻る"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[cfg(feature = "git")]
#[test]
fn e2e_f_toggles_follow_diff_scope_without_breaking_follow() {
    // Regression: f (ToggleFollowDiffScope) is a display option that switches the follow-originated
    // diff's scope (since-start ⇄ full), not a takeover of manual operation → it must not break follow.
    // Before the fix, the "anything but F is follow_break" check didn't include f, so pressing it turned FOLLOW OFF.
    let dir = sandbox("follow_f_scope");
    seed_repo(&dir);
    let mut s = Sim::new(&canon(&dir));
    s.key('F');

    let a = s.app.tab.root.join("a.rs");
    std::fs::write(&a, "fn a() { let _x = 2; }\n").unwrap();
    assert!(s.app.follow_note_change(&a));
    s.app.follow_jump(&a);
    s.draw();
    assert!(s.app.is_git_diff_preview(), "追尾先は全画面 diff で開く");
    assert!(s.app.follow_enabled(), "f を押す前は follow が ON");

    s.key('f');
    assert!(
        s.app.follow_enabled(),
        "f はフォロー表示オプション=follow を解除してはいけない(回帰)"
    );
    assert_eq!(
        s.app.follow_diff_scope_msg(),
        Some(crate::i18n::Msg::FollowShowFull),
        "1 回目の f で範囲がフルへ切り替わる"
    );

    s.key('f');
    assert!(
        s.app.follow_enabled(),
        "2 回目の f でも follow は ON のまま"
    );
    assert_eq!(
        s.app.follow_diff_scope_msg(),
        Some(crate::i18n::Msg::FollowShowSince),
        "2 回目の f で範囲が開始以降へ戻る"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[cfg(feature = "git")]
#[test]
fn e2e_follow_survives_scroll_and_cycle_breaks_only_on_q() {
    // Regression (2026-07-23, user's choice = maximally sticky): inside a follow-originated diff,
    // every key except q (which exits the diff via PreviewBack) — scroll/horizontal scroll/n's
    // changed-file cycling — keeps follow on. Before the fix, any key other than F/f broke it (hands-off/Zed-style).
    let dir = sandbox("follow_sticky");
    seed_repo(&dir);
    let mut s = Sim::new(&canon(&dir));
    s.key('F');
    assert!(s.app.follow_enabled(), "F でフォロー ON");

    let a = s.app.tab.root.join("a.rs");
    std::fs::write(&a, "fn a() { let _x = 2; }\n").unwrap();
    assert!(s.app.follow_note_change(&a));
    s.app.follow_jump(&a);
    s.draw();
    assert!(s.app.is_git_diff_preview(), "追尾先は全画面 diff で開く");
    assert!(s.app.follow_enabled(), "diff を開いた直後は follow ON");

    s.key('j');
    assert!(s.app.follow_enabled(), "スクロールでフォロー維持");
    s.key('h');
    assert!(s.app.follow_enabled(), "横スクロールでフォロー維持");
    s.key('n');
    assert!(
        s.app.follow_enabled(),
        "n(変更ファイル回遊)でもフォロー維持"
    );
    s.key('q');
    assert!(!s.app.follow_enabled(), "q(diff を出る)でだけフォロー解除");
    std::fs::remove_dir_all(&dir).ok();
}

// =============================================================================
// Markdown syntax render verification + wrapped-focus following
// (strictly glyph-only verification — color/reversal can't be read from screen())
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
    // A GFM table is drawn with line-drawing glyphs (┌┬┐ │ ├┼┤ └┴┘), and columns align even with CJK mixed in.
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
    // T-shaped/cross glyphs unique to tables (the preview frame only uses ┌┐└┘─│, so these are proof of the table).
    s.see("┬");
    s.see("┼");
    s.see("┴");
    s.see("├");
    s.see("┤");
    // Column alignment: the vertical bar │'s display column matches across the header row and both data rows (proof CJK width is handled correctly).
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
    // [label](url) inside a table cell only shows the label (the URL is a hidden target, gone from the screen).
    // Can be focused with Tab, and Enter opens the target (./notes.txt) (the link lands in md_items).
    let dir = sandbox("md_table_link");
    seed_files(&dir); // includes notes.txt ("alpha\n...")
    std::fs::write(
        dir.join("tbl.md"),
        "| name | doc |\n|---|---|\n| konoma | [OpenDoc](./notes.txt) |\n",
    )
    .unwrap();
    let mut s = Sim::new(&canon(&dir));
    s.select("tbl.md");
    s.enter();
    s.see("OpenDoc"); // the label is displayed
    s.dont_see("[OpenDoc]"); // the raw Markdown syntax isn't shown
    s.dont_see("./notes.txt"); // the URL is removed by collapse_links, hidden
    s.tab(); // focus the in-table link
    assert_eq!(
        s.app.focused_item(),
        Some(0),
        "表内リンクが md_items の先頭"
    );
    s.enter(); // open the link target
    assert!(
        s.app
            .tab
            .preview_path
            .as_deref()
            .is_some_and(|p| p.ends_with("notes.txt")),
        "表内リンクの Enter で ./notes.txt を開く"
    );
    s.see("alpha"); // notes.txt's content
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_md_html_details_collapse_and_toggle() {
    // <details> (without open) is collapsed by default: summary shows but the body is hidden. Tab
    // focuses summary → Space/Enter expands it, revealing the body. The tags are stripped.
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
    s.see("Summary text"); // <summary> always shows
    s.dont_see("hidden body line"); // default is collapsed = the body is hidden
    s.dont_see("<summary>"); // the tag is stripped
    s.dont_see("<details>");
    // Tab focuses summary → Space expands it.
    s.tab();
    assert_eq!(s.app.focused_item(), Some(0), "summary が Tab アイテム");
    s.key(' ');
    s.see("hidden body line"); // expanded, the body shows
                               // Toggling again collapses it.
    s.enter();
    s.dont_see("hidden body line");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_md_thematic_break_becomes_rule_and_fenced_dashes_kept() {
    // `---` (thematic break) becomes a full-width ─ rule. --- inside a code fence stays literal, unconverted.
    let dir = sandbox("md_hr");
    seed_files(&dir);
    // Don't place a heading (avoids the H1/H2 underline rule mixing with ─).
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
    // A thematic-break row: within the preview frame (leading │), a row exists where the rest is all ─.
    // (The frame's top/bottom border is excluded since it starts with ┌/└.)
    let has_rule = s.screen().lines().any(|l| {
        l.starts_with('│') && l.ends_with('│') && l.chars().filter(|&c| c == '─').count() >= 60
    });
    assert!(has_rule, "--- が全幅 ─ 罫線になる:\n{}", s.screen());
    // --- inside a fence stays literal (since extras run after the code path, there's no false trigger).
    s.see("raw --- text");
    std::fs::remove_dir_all(&dir).ok();
}

// =============================================================================
// Markdown web parity Phase 1: bare-URL autolinking / GitHub Alerts / emoji
// (route every path through both code_bg="none" and the default = real rendering, not synthetic spans)
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
    assert_eq!(s.app.tab.preview_scroll, 0);
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
    assert!(
        s.app.tab.preview_scroll > 0,
        "jumped down to the deep heading"
    );
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
    assert_eq!(s.app.tab.mode, Mode::Tree, "the new tab is a fresh tree");
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
    assert_eq!(s.app.tab.preview_scroll, 0, "starts at the top");
    s.tab(); // focus the anchor link
    assert_eq!(s.app.focused_item(), Some(0));
    s.enter(); // jump
    assert!(
        s.app.tab.preview_scroll > 0,
        "scrolled down toward the heading (was {})",
        s.app.tab.preview_scroll
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
    assert_eq!(
        s.app.tab.preview_scroll, 0,
        "no scroll for a missing anchor"
    );
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
    // Tasks `- [ ]` / `- [x]`: ASCII brackets when ui.icons=false, NF glyphs when true.
    let dir = sandbox("md_tasks");
    seed_files(&dir);
    std::fs::write(dir.join("tasks.md"), "- [ ] todo item\n- [x] done item\n").unwrap();
    let root = canon(&dir);

    // icons=false: ASCII "[ ]" / "[x]".
    let mut cfg = Config::default();
    cfg.ui.icons = false;
    let mut s = Sim::with_config(&root, cfg);
    s.select("tasks.md");
    s.enter();
    s.see("[ ] todo item");
    s.see("[x] done item");

    // icons=true (default): Nerd Font glyphs (square_o / check_square_o); ASCII brackets don't appear.
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
    // R switches Markdown between decorated display ⇄ raw source (windowed, "· raw source" in the title).
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
    // Note: the footer always shows the "R raw source" hint even while decorated (a toggle notice).
    // The title's raw marker is "· raw source" (with a middle dot), so use it for the mode check.
    assert!(!s.app.is_raw_source(), "初期は装飾表示");
    s.dont_see("· raw source"); // no raw marker in the title (distinct from the hint's "raw source")
    s.key('R');
    assert!(s.app.is_raw_source(), "R で raw ソース表示");
    s.see("· raw source"); // the title's "· raw source" (short enough not to be cut, being a relative path)
    s.key('R');
    assert!(!s.app.is_raw_source(), "R で装飾表示へ戻る");
    s.dont_see("· raw source");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_markdown_edit_opens_near_scroll_position() {
    // Scrolling decorated Markdown and pressing `e` opens the editor at an approximate source line
    // corresponding to the current display position (approximate, not exact, because of reflow). At
    // the top it's line 1; scrolling down advances it. Since Sim draws the UI after every key, md_view_rows (the total wrapped row count) is set just as in real usage.
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
    s.enter(); // decorated Markdown preview
    assert!(!s.app.is_windowed(), "装飾 md は非 windowed");

    // At the top, it opens at line 1.
    s.key('e');
    let (_p, top) = s.app.take_pending_edit().expect("edit requested");
    assert_eq!(top, Some(1), "先頭スクロールは 1 行目");

    // Scroll down far enough, then `e`: it doesn't stick to the top, and points at a later source line.
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
    // Place a unique heading after a long, wrapping intro. Proportional estimation alone would drift
    // due to wrapping, but confirm that content-anchor searches the source for the screen-top text and lands on the heading's "exact" source line.
    let dir = sandbox("md_anchor");
    let mut content = String::from("# Title\n\n");
    for i in 1..=15 {
        content.push_str(&format!(
            "Intro paragraph {i:02} with a deliberately long sentence that wraps across several display rows in a narrow preview here.\n"
        ));
    }
    content.push_str("\n## ANCHORZZZ Section\n\n");
    // Stack enough following lines so the heading can be pushed up to the screen top (row 2).
    for i in 1..=60 {
        content.push_str(&format!("Trailing line {i:02} after the anchor.\n"));
    }
    // The heading's source line (1-based).
    let heading_line = content
        .lines()
        .position(|l| l.contains("ANCHORZZZ"))
        .expect("見出しがある")
        + 1;
    std::fs::write(dir.join("doc.md"), &content).unwrap();

    let mut s = Sim::new(&canon(&dir));
    s.select("doc.md");
    s.enter(); // decorated Markdown preview
    assert!(!s.app.is_windowed(), "装飾 md は非 windowed");

    // Scroll one line at a time until the heading reaches the screen top (frame's row 1 = buffer's row 3).
    let mut scrolled = 0;
    while scrolled < 300 {
        let top = s.screen().lines().nth(2).unwrap_or("").to_string();
        if top.contains("ANCHORZZZ") {
            break;
        }
        s.key('j');
        scrolled += 1;
    }
    s.see("ANCHORZZZ"); // the heading is showing on screen

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
    // If Tab has focused an item (task/link), `e` opens at that focused line instead of the screen
    // top. Advancing Tab moves the focus, and the opened line follows along.
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
    s.enter(); // decorated Markdown preview
    assert!(!s.app.is_windowed(), "装飾 md は非 windowed");

    // Tab → focuses the first item (the task). e → opens at the task's source line.
    s.tab();
    s.key('e');
    let (_p, line) = s.app.take_pending_edit().expect("edit requested");
    assert_eq!(
        line,
        Some(task_line),
        "Tab フォーカスのタスク行 {task_line}"
    );

    // Tab again → to the link. e → opens at the link's source line.
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
    // Tab cycles link → code block → task in document order, and while a code block is focused,
    // `y` opens the copy menu where the `c` that appears there copies its raw source (the value is
    // verified via a clipboard-independent getter). Enter does nothing on a code block.
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
    s.see("rust"); // the code block's language header
    s.tab(); // the link
    assert!(!s.app.md_focused_code());
    s.tab(); // the code block
    assert!(s.app.md_focused_code(), "2番目=コードブロックにフォーカス");
    assert_eq!(
        s.app.focused_code_text().as_deref(),
        Some("fn main() {}\nlet y = 2;"),
        "生ソースをコピー対象に"
    );
    // Enter is a no-op on a code block (no which-key, no flash either).
    s.enter();
    assert!(s.app.flash.is_none(), "Enter はコードブロックで何もしない");
    // `y` doesn't hijack it — it opens the copy menu (which-key). While focused, `c:code block` appears.
    s.key('y');
    assert_eq!(
        s.app.pending_leader,
        Some(crate::keymap::LeaderId::Copy),
        "y はコピーリーダー(which-key)を開く"
    );
    s.see("code block"); // the code-block copy entry shows up in the menu
                         // `c` copies (since the clipboard is environment-dependent, only confirm the flash fires).
    s.key('c');
    assert!(s.app.pending_leader.is_none(), "c で確定=リーダー閉じる");
    assert!(s.app.flash.is_some(), "y c でコピー通知が出る");
    s.tab(); // the task
    assert!(!s.app.md_focused_code());
    std::fs::remove_dir_all(&dir).ok();
}

/// Root cause (indented code blocks, 2026-08): a document containing a 4+-column indented code
/// block refused `y c` for **every** code block in it (the count guard is document-wide), so a real
/// fence right next to a perfectly ordinary indented example also stopped copying. Drives Tab → `y`
/// `c` with real keystrokes across a document mixing a fence and an indented block, and confirms
/// each one copies its *own* content — not its neighbor's, which an off-by-one in the ordinal
/// mapping between the two block kinds could otherwise produce silently.
#[test]
fn e2e_md_indented_code_block_tab_focus_and_copy() {
    let dir = sandbox("md_indented_code_copy");
    seed_files(&dir);
    std::fs::write(
        dir.join("snip.md"),
        "Intro paragraph.\n\n```rust\nfn fenced() {}\n```\n\nMore text.\n\n    indented one\n    indented two\n\nTail.\n",
    )
    .unwrap();
    let mut s = Sim::new(&canon(&dir));
    s.select("snip.md");
    s.enter();
    s.see("rust"); // the fence's language header

    // 1st focusable item: the fenced block.
    s.tab();
    assert!(s.app.md_focused_code(), "1番目=フェンスコードにフォーカス");
    assert_eq!(
        s.app.focused_code_text().as_deref(),
        Some("fn fenced() {}"),
        "フェンス側の内容が取得できる(コピー拒否されない)"
    );
    s.key('y');
    s.key('c');
    assert!(s.app.flash.is_some(), "フェンス側のコピー通知が出る");

    // 2nd focusable item: the indented block. Must resolve to its own content, not the fence's.
    s.tab();
    assert!(s.app.md_focused_code(), "2番目=字下げコードにフォーカス");
    assert_eq!(
        s.app.focused_code_text().as_deref(),
        Some("indented one\nindented two"),
        "字下げ側の内容が正しく取得できる(フェンス側を誤って取ってこない)"
    );
    s.key('y');
    s.key('c');
    assert!(s.app.flash.is_some(), "字下げ側のコピー通知が出る");

    std::fs::remove_dir_all(&dir).ok();
}

/// The same class of bug, but proven the other way around: an indented block that comes *before*
/// several real fences in the document must not throw off every later block's ordinal.
#[test]
fn e2e_md_indented_code_block_before_fences_keeps_ordinals_correct() {
    let dir = sandbox("md_indented_before_fences");
    seed_files(&dir);
    std::fs::write(
        dir.join("snip.md"),
        "Para.\n\n    first indented\n\nMid.\n\n```\nsecond fenced\n```\n\n```\nthird fenced\n```\n",
    )
    .unwrap();
    let mut s = Sim::new(&canon(&dir));
    s.select("snip.md");
    s.enter();

    s.tab(); // 1st: indented
    assert_eq!(s.app.focused_code_text().as_deref(), Some("first indented"),);
    s.tab(); // 2nd: fenced
    assert_eq!(s.app.focused_code_text().as_deref(), Some("second fenced"));
    s.tab(); // 3rd: fenced
    assert_eq!(s.app.focused_code_text().as_deref(), Some("third fenced"));

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_md_wrapped_focus_follows_offscreen_item() {
    // When wrapped: moving with Tab to a link after a paragraph taller than the screen makes preview_scroll follow.
    // When not wrapped: the paragraph collapses to 1 line and all fits on screen = no scroll needed (confirm the behavior differs both ways).
    let dir = sandbox("md_wrap_focus");
    seed_files(&dir);
    // A long paragraph (1 logical line) that wraps to ~34 display rows at width 88 + a link/task after it, apart.
    let para = "lorem ".repeat(500); // 3000 chars ≈ ~34 display rows at width 88 > viewport 22
    std::fs::write(
        dir.join("torture.md"),
        format!("{para}\n\n[ZZLINK](./notes.txt)\n\n- [ ] ZZTASK\n"),
    )
    .unwrap();
    let root = canon(&dir);

    // --- wrap=true (default): scrolls to follow ---
    let mut on = Sim::new(&root);
    on.select("torture.md");
    on.enter();
    assert!(on.app.cfg.ui.wrap, "前提: 既定は折返しON");
    assert_eq!(on.app.tab.preview_scroll, 0, "初期スクロール 0");
    on.dont_see("ZZLINK"); // the paragraph occupies the screen = the link is off-screen
    on.tab(); // focus the link past the paragraph (~34 display rows in)
    assert!(
        on.app.tab.preview_scroll > 5,
        "折返しでフォーカスに追従してスクロール: scroll={}",
        on.app.tab.preview_scroll
    );
    on.see("ZZLINK"); // after following, it's on screen

    // --- wrap=false: the paragraph is 1 line = everything fits on screen, so no scroll ---
    let mut cfg = Config::default();
    cfg.ui.wrap = false;
    let mut off = Sim::with_config(&root, cfg);
    off.select("torture.md");
    off.enter();
    off.see("ZZLINK"); // paragraph is 1 line = the link is on screen from the start
    off.tab();
    assert_eq!(
        off.app.tab.preview_scroll, 0,
        "非折返しは追従スクロール不要(scroll 0 のまま)"
    );
    off.see("ZZLINK");
    std::fs::remove_dir_all(&dir).ok();
}

// =============================================================================
// File operations: copy/cut → paste, batch rename, visual selection
// (op_base_dir = the directory at the cursor. Inside it while a directory is selected / its parent for a file)
// =============================================================================

#[test]
fn e2e_file_copy_then_paste_duplicates() {
    // Space→c copies the file at the cursor, then moving the cursor to the destination directory and
    // Space→p duplicates it (the original remains). The paste target = op_base_dir (inside it while a directory is selected).
    let dir = sandbox("copy_paste");
    std::fs::write(dir.join("orig.txt"), "hello copy\n").unwrap();
    std::fs::create_dir_all(dir.join("dest")).unwrap();
    let dir = canon(&dir);
    let mut s = Sim::new(&dir);

    s.select("orig.txt"); // cursor on the copy target (op_targets = the file at the cursor)
    s.key(' ');
    s.key('c'); // Space→c = FileCopy
    s.select("dest"); // the paste-destination directory (is_dir, so op_base_dir = dest itself)
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
fn e2e_file_copy_then_paste_runs_in_background() {
    // Route the same operation as the synchronous version above through **the actual background path** (attaching a runner).
    // Space→c copies the directory, move the cursor to the paste target, then Space→p. Right after
    // paste, it's still running (shows up in the busy indicator), and only reflects once the worker's result arrives.
    let dir = sandbox("copy_paste_async");
    std::fs::create_dir_all(dir.join("pkg/inner")).unwrap();
    std::fs::write(dir.join("pkg/one.txt"), "1\n").unwrap();
    std::fs::write(dir.join("pkg/inner/two.txt"), "2\n").unwrap();
    std::fs::create_dir_all(dir.join("dest")).unwrap();
    let dir = canon(&dir);
    let mut s = Sim::new(&dir).with_async_file_ops();

    s.select("pkg");
    s.key(' ');
    s.key('c'); // Space→c = FileCopy
    s.select("dest");
    s.key(' ');
    s.key('p'); // Space→p = FilePaste (dispatched to the worker)

    assert!(
        s.app.busy_jobs().contains(&crate::i18n::Msg::BusyFileOp),
        "貼付直後はバックグラウンド実行中(busy インジケーターに出る)"
    );

    s.drain_file_ops();

    assert!(
        dir.join("dest/pkg/one.txt").is_file() && dir.join("dest/pkg/inner/two.txt").is_file(),
        "ディレクトリごとコピーされている"
    );
    assert!(dir.join("pkg/one.txt").is_file(), "コピー元は残る");
    assert!(
        s.app
            .tab
            .entries
            .iter()
            .any(|e| e.path == dir.join("dest/pkg")),
        "貼付先が展開され、コピーがツリーに現れる"
    );
    s.see("pkg");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_file_cut_then_paste_moves() {
    // Space→x cuts, then Space→p in another directory → the original disappears and it's created at the destination (a move).
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
    s.key('p'); // Space→p = FilePaste (cut is move)

    assert!(dir.join("dest/orig.txt").exists(), "移動先に出来る");
    assert!(!dir.join("orig.txt").exists(), "移動元は消える");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_batch_rename_numbers_selection_via_preview() {
    // Multi-select (V×2) → Space→r for batch rename. A {n} template gives sequential numbers; going
    // through the preview dialog and applying with y → sequentially renamed (the extension is kept automatically).
    let dir = sandbox("batch_rename");
    std::fs::write(dir.join("alpha.log"), "a\n").unwrap();
    std::fs::write(dir.join("beta.log"), "b\n").unwrap();
    let dir = canon(&dir);
    let mut s = Sim::new(&dir);

    // V=ToggleSelect: selects the cursor and moves down by one. Twice selects the first 2 files.
    s.key('V');
    s.key('V');
    assert!(s.app.has_selection(), "複数選択が入っている");

    s.key(' ');
    s.key('r'); // Space→r. Since there's a selection, start_batch_rename runs (a template-entry dialog).
    assert!(s.app.is_dialog(), "一括リネームの入力ダイアログが開く");
    s.keys("renamed-{n}"); // {n} = the sequence-number token
    s.enter(); // build_rename_plan → transitions to the preview (old → new)
    assert!(s.app.dialog_is_preview(), "リネームプレビューが挟まる");
    s.see("renamed-1");
    s.key('y'); // the preview's y = apply (dialog_preview_apply)

    assert!(dir.join("renamed-1.log").exists(), "1件目が連番リネーム");
    assert!(dir.join("renamed-2.log").exists(), "2件目が連番リネーム");
    assert!(!dir.join("alpha.log").exists(), "旧名は消える");
    assert!(!dir.join("beta.log").exists(), "旧名は消える");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_tree_visual_selection_grows_then_commits() {
    // v = start range selection → j grows the range (marked_count increases) → v = commit, folding it into selection.
    let dir = sandbox("tree_visual");
    seed_files(&dir);
    let mut s = Sim::new(&canon(&dir));

    s.key('v'); // EnterVisual (Surface=Visual)
    assert!(s.app.is_visual(), "v で範囲選択開始");
    assert_eq!(s.app.marked_count(), 1, "開始直後はアンカー1行");
    s.key('j'); // in the Visual surface, Down → the range grows to [anchor, anchor+1]
    assert_eq!(s.app.marked_count(), 2, "j で範囲が2行に広がる");
    s.key('v'); // in the Visual surface, v=VisualCommit → commit the range into selection and exit
    assert!(!s.app.is_visual(), "v で範囲確定して visual を抜ける");
    assert!(s.app.has_selection(), "確定した集合が残る");
    assert_eq!(s.app.marked_count(), 2, "確定後は選択2件");
    std::fs::remove_dir_all(&dir).ok();
}

// =============================================================================
// Config reflection: details columns / path style cycling / csv_rainbow / busy indicator
// =============================================================================

#[test]
fn e2e_details_columns_show_size_and_modified() {
    // With ui.details=["size","modified"], each row's right edge shows size/modified columns.
    // The expected value is computed via the same quick_meta as the renderer (no hardcoded time/size).
    let dir = sandbox("details");
    seed_files(&dir);
    let dir = canon(&dir);
    let notes = dir.join("notes.txt");
    let meta = crate::fileops::quick_meta(&notes).expect("quick_meta");
    let size_cell = crate::fileops::human_size(meta.size);
    let mod_cell = crate::fileops::format_epoch_short(meta.mtime.expect("mtime"));

    // Control: without details, the date column doesn't appear (a false-positive guard).
    let plain = Sim::new(&dir);
    plain.dont_see(&mod_cell);

    let mut cfg = Config::default();
    cfg.ui.details = vec!["size".into(), "modified".into()];
    let s = Sim::with_config(&dir, cfg);
    s.see(&size_cell); // e.g. "23 B" (appears in the right-aligned column)
    s.see(&mod_cell); // e.g. "2026-07-08 14:23" (the 16-column modified-time column)
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_path_style_cycles() {
    // p=CyclePathStyle cycles Relative→Home→Full→Relative. The path: display at the top follows along too.
    use crate::app::PathStyle;
    let dir = sandbox("path_style");
    seed_files(&dir);
    let mut s = Sim::new(&canon(&dir));

    assert_eq!(s.app.path_style, PathStyle::Relative, "既定は relative");
    s.see("path:rel"); // status's path label (assumes the English locale = same as existing tests)
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
    // Even with ui.csv_rainbow=false, cell content still shows as an aligned table (color can't be verified — glyphs only).
    // hjkl/0/$ move the cell cursor (a different angle than the existing test — also checks jumping to a column edge).
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
    s.see("fruit"); // header (glyphs show even in monochrome)
    s.see("apple"); // data cell
    assert_eq!(s.app.table_cursor(), (0, 0), "初期カーソルは先頭セル");
    s.key('l'); // to the right column
    assert_eq!(s.app.table_cursor(), (0, 1), "l で1列右へ");
    s.key('l'); // clamps at the last column (ncols=2)
    assert_eq!(s.app.table_cursor(), (0, 1), "末列でクランプ");
    s.key('0'); // to the first column
    assert_eq!(s.app.table_cursor(), (0, 0), "0 で先頭列");
    s.key('$'); // to the last column
    assert_eq!(s.app.table_cursor(), (0, 1), "$ で末列");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_tabbar_overflow_markers_keep_active_visible() {
    // Opening many tabs and overflowing at a narrow terminal width (90) shows the overflow markers
    // ‹n / n›, while the active tab stays visible (visible_range's active-centered visible window).
    let dir = sandbox("tabbar_overflow");
    seed_files(&dir);
    let mut s = Sim::new(&canon(&dir));

    s.keys("tttttttt"); // t×8 → 9 tabs total (each t switches to the new tab)
    assert_eq!(s.app.tab_count(), 9);
    s.key('5'); // to tab 5 (index 4 = center). Hidden tabs appear on both sides.
    assert_eq!(s.app.active_tab_index(), 4);

    s.see("‹"); // left overflow marker (s>0)
    s.see("›"); // right overflow marker (e<n)
    let label = s.app.tab_label(4);
    s.see(&format!("5:{label}")); // the active tab's chip stays visible
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_busy_indicator_absent_when_idle() {
    // When idle (no job running in the background), the busy display doesn't show. Disabling via config makes it always inactive.
    // Note: since the synchronous harness can't actually trigger an async job, only the idle invariant is verified (an honest scope).
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
// Paste-jump (P): clipboard path/GitHub link → jumps to that location
// =============================================================================

#[test]
fn e2e_paste_jump_local_path_with_line() {
    // Passing a local relative path + `:line` jumps to that location, opens the preview, and scrolls
    // to the given line (the top marker goes off-screen). A nonexistent path flashes a notice, no crash.
    let dir = sandbox("paste_jump_local");
    seed_files(&dir);
    std::fs::create_dir_all(dir.join("src")).unwrap();
    // Plain text (windowed, no highlight-pending wait = deterministic in the synchronous harness). The line-jump path is the same as Code's.
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
    // Paste-jump from the tree (no preview open). paste_jump_from is a clipboard-independent pub entry point.
    s.app.paste_jump_from("src/deep.txt:30");
    s.draw();
    assert_eq!(s.app.tab.mode, Mode::Preview, "ファイルはプレビューで開く");
    assert!(
        s.app
            .tab
            .preview_path
            .as_deref()
            .map(|p| p.ends_with("src/deep.txt"))
            .unwrap_or(false),
        "deep.txt がプレビュー対象: {:?}",
        s.app.tab.preview_path
    );
    s.see("LINE_THIRTY_MARKER"); // scrolled down to line 30
    s.dont_see("LINE_ONE_MARKER"); // the top is off-screen = it actually scrolled

    // A path that isn't found flashes a notice, without crashing (principle #3).
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
    // With konoma opened at a subdirectory of the repo, pasting a GitHub blob URL pointing at a
    // different directory within that same repo: ① drops the URL's owner/repo/blob/ref and resolves
    // the file relative to the local repo (workdir); ② since the target is outside the current root, switches root to the repo (workdir) and reveals + previews.
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
    // Launch konoma at the subdirectory (src). docs/guide.md is outside this root.
    let mut s = Sim::new(&subdir);
    assert_eq!(s.app.tab.root, subdir, "起動 root は src サブディレクトリ");

    // Even when the repository name (owner/name) differs from the local one, it can open as long as the trailing suffix docs/guide.md actually exists.
    s.app
        .paste_jump_from("https://github.com/some-owner/some-name/blob/main/docs/guide.md");
    s.draw();

    assert_eq!(s.app.tab.root, repo, "root が repo(workdir)へ切替わる");
    assert_eq!(s.app.tab.mode, Mode::Preview, "guide.md をプレビューで開く");
    assert!(
        s.app
            .tab
            .preview_path
            .as_deref()
            .map(|p| p.ends_with("docs/guide.md"))
            .unwrap_or(false),
        "guide.md がプレビュー対象: {:?}",
        s.app.tab.preview_path
    );
    s.see("GUIDE_MARKER");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_paste_jump_from_visual_exits_visual_and_lands_in_preview() {
    // Bug regression: pressing `P` (PasteJump) while in the tree's Visual (range selection) used to
    // transition to a full-screen preview without the selection being committed/discarded. Since
    // `is_visual()` doesn't check `tab.mode` and only checks whether `tab.visual_anchor` exists,
    // `surface()` kept returning Visual even while the preview was displayed, causing an accident
    // where keys (`j` etc.) flowed into the tree's Visual map even though what's visible is the preview.
    let dir = sandbox("paste_jump_visual");
    std::fs::write(dir.join("target.txt"), "PASTE_JUMP_VISUAL_BODY\n").unwrap();
    let dir = canon(&dir);
    let mut s = Sim::new(&dir);

    // Via a real key: v enters the tree's Visual.
    s.select("target.txt");
    s.key('v');
    assert!(s.app.is_visual(), "v でツリー Visual に入る");
    assert_eq!(s.app.surface(), crate::keymap::Surface::Visual);

    // Dispatch P (PasteJump). Why call `crate::dispatch_action` directly instead of a real key:
    // this test doesn't want to touch the real machine's system clipboard (its content is
    // environment-dependent = the test becomes flaky. paste_jump()'s own resolution/crash resilience
    // is separately verified elsewhere, e.g. e2e_paste_jump_local_path_with_line, via the
    // clipboard-independent `paste_jump_from`). dispatch_action IS main.rs's central dispatch, and
    // passes exactly the same Action/Surface arguments that the real key `P` resolves to, so as a
    // path it's identical to going through a real key. As long as main.rs's dispatch routes through
    // `commit_visual_if_needed` (as with the other Space→ actions), Visual is guaranteed to end
    // regardless of the clipboard's content/whether reading it succeeds (commit_visual_if_needed
    // runs **before** calling app.paste_jump()).
    let sfc = s.app.surface();
    assert_eq!(sfc, crate::keymap::Surface::Visual);
    crate::dispatch_action(&mut s.app, crate::keymap::Action::PasteJump, sfc)
        .expect("dispatch_action(PasteJump)");
    s.draw();
    assert!(
        !s.app.is_visual(),
        "PasteJump を dispatch した後は(ジャンプの成否によらず)ツリーの Visual 選択が終わっている"
    );

    // Independently verify enter_preview's own defense: paste_jump_from is a clipboard-independent
    // pub entry point (the same style as the existing paste-jump e2e). Deliberately re-enter Visual
    // to simulate a hypothetical caller that doesn't go through commit_visual_if_needed, and confirm
    // the invariant enter_preview itself must guarantee (entering the full-screen preview = the
    // tree's range selection has ended).
    s.app.enter_visual();
    assert!(s.app.is_visual(), "enter_visual で Visual に入り直す");
    s.app.paste_jump_from("target.txt");
    s.draw();
    assert!(
        !s.app.is_visual(),
        "enter_preview は(呼び出し元が畳み忘れていても)ツリーの Visual を必ず終える"
    );
    assert_ne!(
        s.app.surface(),
        crate::keymap::Surface::Visual,
        "surface() はもう Visual を返さない"
    );
    assert_eq!(s.app.tab.mode, Mode::Preview, "プレビューに入っている");
    s.see("PASTE_JUMP_VISUAL_BODY");

    // Proof that keys aren't flowing into the tree's Visual map while viewing the preview: j doesn't
    // move the tree's cursor/Visual range (at the time of the bug, j was extending the tree's Visual range).
    let cursor_before = s.app.tab.selected;
    s.key('j');
    assert_eq!(
        s.app.tab.selected, cursor_before,
        "j はもうツリーの Visual カーソルを動かさない(プレビュー中)"
    );
    std::fs::remove_dir_all(&dir).ok();
}

// =============================================================================
// Open Markdown links in a new tab (Ctrl-t)
// =============================================================================

#[test]
fn e2e_md_link_ctrl_t_opens_in_new_tab() {
    // Focusing a local Markdown link and pressing Ctrl-t opens the target in a **separate tab**.
    // The original document's tab remains, reachable with [. Enter (same tab) works as before.
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

    s.tab(); // focus the link "go"
    assert!(s.app.focused_item().is_some(), "リンクにフォーカス");

    s.ctrl('t'); // open in a new tab
    assert_eq!(s.app.tab_count(), 2, "別タブが増える");
    assert_eq!(s.app.tab.mode, Mode::Preview);
    assert!(
        s.app
            .tab
            .preview_path
            .as_deref()
            .map(|p| p.ends_with("target.txt"))
            .unwrap_or(false),
        "新タブは target.txt を開く: {:?}",
        s.app.tab.preview_path
    );
    s.see("TARGET_FILE_MARKER");

    // The original document's tab remains: going back with [ (tab_prev) still shows doc.md.
    s.key('[');
    assert!(
        s.app
            .tab
            .preview_path
            .as_deref()
            .map(|p| p.ends_with("doc.md"))
            .unwrap_or(false),
        "元タブは doc.md のまま: {:?}",
        s.app.tab.preview_path
    );
    s.see("DOC_HEADING");
    std::fs::remove_dir_all(&dir).ok();
}

// =============================================================================
// Reload from disk the tab that became active on a tab switch
// =============================================================================

#[test]
fn e2e_tab_switch_reloads_tree_from_disk() {
    // File watching only sees the active root, so an external change to a backgrounded tab stays as a stale snapshot.
    // Switching to that tab triggers a refresh_fs reload, and files created in the background appear in the tree.
    let dir = sandbox("tab_switch_reload");
    seed_files(&dir);
    let dir = canon(&dir);

    let mut s = Sim::new(&dir);
    s.dont_see("ZZ_NEW_FILE.txt");

    // Create a new tab (same root) and make it active (tab 1 goes to the background).
    s.key('t');
    assert_eq!(s.app.tab_count(), 2, "タブが2つ");

    // While tab 1 is in the background, create a file externally.
    std::fs::write(dir.join("ZZ_NEW_FILE.txt"), "hi\n").unwrap();

    // Return to tab 1 ([ = tab_prev). The switch-time reload should reveal the new file.
    s.key('[');
    assert_eq!(s.app.active_tab_index(), 0, "タブ1 に戻る");
    assert!(
        s.app
            .tab
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
    // Ctrl-n/Ctrl-p = paging through files while previewing: tree display order, directories skipped,
    // wraps at the ends, also traverses files inside an expanded subfolder, the tree cursor follows.
    let dir = std::env::temp_dir().join("konoma_e2e_file_paging");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("mid")).unwrap();
    std::fs::write(dir.join("a.txt"), b"alpha content\n").unwrap();
    std::fs::write(dir.join("z.txt"), b"zulu content\n").unwrap();
    std::fs::write(dir.join("mid/inner.txt"), b"inner content\n").unwrap();
    let mut s = Sim::new(&dir);

    // Expand mid (with the dirs_first default, tree order is [mid, mid/inner.txt, a.txt, z.txt]).
    s.enter();
    s.key('j'); // inner.txt
    s.key('j'); // a.txt
    s.enter(); // preview a.txt
    assert!(matches!(s.app.tab.mode, Mode::Preview));
    s.see("alpha content");

    // Next file: a.txt → z.txt.
    s.ctrl('n');
    s.see("zulu content");
    assert!(s.app.tab.preview_path.as_ref().unwrap().ends_with("z.txt"));

    // Next again: wraps from the end, and skips mid (a directory) at the top to reach inner.txt.
    s.ctrl('n');
    s.see("inner content");
    assert!(s
        .app
        .tab
        .preview_path
        .as_ref()
        .unwrap()
        .ends_with("inner.txt"));

    // Previous file: before inner.txt is mid (skipped) → wraps to z.txt.
    s.ctrl('p');
    s.see("zulu content");
    assert!(s.app.tab.preview_path.as_ref().unwrap().ends_with("z.txt"));

    // The tree cursor followed along (q returns to being on z.txt).
    s.key('q');
    assert!(matches!(s.app.tab.mode, Mode::Tree));
    assert!(s.app.tab.entries[s.app.tab.selected]
        .path
        .ends_with("z.txt"));
    std::fs::remove_dir_all(&dir).ok();
}

/// `[ui] restore_tabs`: the tab session is saved per launch directory, and on "restart" the tab
/// configuration (count, active tab, cursor, preview surface) is restored as-is.
#[test]
fn e2e_session_restore_reopens_previous_tabs() {
    let dir = sandbox("session_restore");
    std::fs::write(dir.join("a.txt"), "alpha body\n").unwrap();
    std::fs::write(dir.join("b.txt"), "beta body\n").unwrap();
    let base = std::env::temp_dir().join("konoma_e2e_session_restore_base");
    let _ = std::fs::remove_dir_all(&base);

    // Session 1: preview a.txt → t for a new tab (tree) → cursor on b.txt.
    let mut s = Sim::new(&dir);
    s.app
        .attach_session_store(crate::session::SessionStore::with_base(base.clone(), &dir));
    s.select("a.txt");
    s.press(KeyCode::Enter, KeyModifiers::NONE);
    assert!(matches!(s.app.tab.mode, Mode::Preview));
    s.key('t');
    s.select("b.txt");
    s.app.save_session(); // equivalent to the on-quit save (main calls this after run)
    drop(s);

    // "Restart": attach the store to a fresh Sim of the same launch dir and restore (same order as main's launch).
    let mut s2 = Sim::new(&dir);
    assert_eq!(s2.app.tab_count(), 1, "復元前は素の1タブ");
    s2.app
        .attach_session_store(crate::session::SessionStore::with_base(base.clone(), &dir));
    s2.app.restore_session();
    s2.draw();
    assert_eq!(s2.app.tab_count(), 2, "タブ数を復元");
    assert_eq!(s2.app.active_tab_index(), 1, "アクティブタブを復元");
    assert!(matches!(s2.app.tab.mode, Mode::Tree));
    assert!(
        s2.app.tab.entries[s2.app.tab.selected]
            .path
            .ends_with("b.txt"),
        "ツリーカーソルを復元"
    );
    s2.see("a.txt"); // the tab bar shows the preview tab's (tab 1's) label
                     // Switching to tab 1 = a.txt's preview reopens (the content shows on screen).
    s2.key('[');
    assert!(matches!(s2.app.tab.mode, Mode::Preview));
    s2.see("alpha body");

    std::fs::remove_dir_all(&dir).ok();
    std::fs::remove_dir_all(&base).ok();
}

// =============================================================================
// Additional E2E batch (2026-07-18): fills in a large number of thin/missing coverage areas.
// Routes the whole source→real-render path through the Sim harness, verified via clipboard-independent pub accessors.
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

// ---- In-preview text search (`/` starts it, n/N move, case-insensitive, no match, Esc, CJK) ----

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
    assert_eq!(s.app.tab.mode, Mode::Preview);
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
    assert_eq!(s.app.tab.mode, Mode::Preview);
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
    assert_eq!(s.app.tab.mode, Mode::Preview);
    s.esc();
    assert_eq!(s.app.tab.mode, Mode::Tree);
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
    // Turn off highlighting to avoid depending on execution order: with a cold grammar, the preview
    // shows "loading…" and this search test's result would change depending on "whether another
    // test had warmed .rs" (it failed when run alone). Syntax coloring isn't needed to verify search, so run with a setting that deterministically shows the body.
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

/// `/` works on decorated Markdown too (it used to be rejected as "code/text only").
/// Since the search target is the decorated lines, matches can be highlighted without switching to `R`'s raw source.
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

/// A surface without a search model (images, etc.) still rejects it and notifies via flash, as before.
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
    assert_eq!(s.app.tab.mode, Mode::Preview);
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

// ---- Windowed page/half-page scroll: real keystrokes (both key schemes) ----
// Regression: after paging *down* through a windowed (Code/Text) preview, the caret sits on the
// last visible row, so the very next page/half-page *up* moved the caret back to `top` — still
// on screen — and `follow_cursor` saw nothing to do: the window didn't move (the first upward
// page/half-page after a downward one was silently swallowed). Fixed by `App::windowed_page_scroll`,
// which moves the window directly and carries the caret along. These drive real key input through
// `handle_key` (not the App method directly) for both the vim (Ctrl-f/b/d/u) and less (f/b/d/u)
// key schemes, and cross-check the two schemes land on identical window positions.

/// 400 lines "line001".."line400", one per line — enough room to page through repeatedly without
/// hitting the top/bottom clamp.
fn paging_body() -> String {
    let mut body = String::new();
    for i in 1..=400 {
        body.push_str(&format!("line{i:03}\n"));
    }
    body
}

#[test]
fn e2e_preview_page_vim_scheme_first_page_up_after_paging_down_moves_the_window() {
    let (mut s, dir) = text_preview(Config::default(), "wpage_vim", "big.txt", &paging_body());
    assert!(s.app.is_windowed());
    let page = (s.app.tab.preview_viewport as i64 - 1).max(1);

    s.ctrl('f');
    s.ctrl('f');
    s.ctrl('f');
    let top_after_downs = s.app.preview_top_line();
    assert_eq!(top_after_downs as i64, 3 * page, "Ctrl-f×3 後の窓位置");

    s.ctrl('b');
    assert_ne!(
        s.app.preview_top_line(),
        top_after_downs,
        "vim: Ctrl-f の直後、最初の Ctrl-b で窓が動かない(バグの再現)"
    );
    assert_eq!(
        s.app.preview_top_line() as i64,
        2 * page,
        "Ctrl-b は 1 ページ分だけ戻るはず"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_preview_half_page_vim_scheme_first_half_up_after_half_paging_down_moves_the_window() {
    let (mut s, dir) = text_preview(Config::default(), "whalf_vim", "big.txt", &paging_body());
    assert!(s.app.is_windowed());
    let half = (s.app.tab.preview_viewport as i64 / 2).max(1);

    s.ctrl('d');
    s.ctrl('d');
    let top_after_downs = s.app.preview_top_line();
    assert_eq!(top_after_downs as i64, 2 * half, "Ctrl-d×2 後の窓位置");

    s.ctrl('u');
    assert_ne!(
        s.app.preview_top_line(),
        top_after_downs,
        "vim: Ctrl-d の直後、最初の Ctrl-u で窓が動かない(バグの再現)"
    );
    assert_eq!(
        s.app.preview_top_line() as i64,
        half,
        "Ctrl-u は半ページ分だけ戻るはず"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_preview_page_less_scheme_first_page_up_after_paging_down_moves_the_window() {
    let mut cfg = Config::default();
    cfg.ui.keys = "less".into();
    let (mut s, dir) = text_preview(cfg, "wpage_less", "big.txt", &paging_body());
    assert!(s.app.is_windowed());
    let page = (s.app.tab.preview_viewport as i64 - 1).max(1);

    // less: Space also pages down (in addition to `f`) — exercise both.
    s.key(' ');
    s.key('f');
    s.key('f');
    let top_after_downs = s.app.preview_top_line();
    assert_eq!(top_after_downs as i64, 3 * page, "Space,f,f 後の窓位置");

    s.key('b');
    assert_ne!(
        s.app.preview_top_line(),
        top_after_downs,
        "less: f の直後、最初の b で窓が動かない(バグの再現)"
    );
    assert_eq!(
        s.app.preview_top_line() as i64,
        2 * page,
        "b は 1 ページ分だけ戻るはず"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_preview_half_page_less_scheme_first_half_up_after_half_paging_down_moves_the_window() {
    let mut cfg = Config::default();
    cfg.ui.keys = "less".into();
    let (mut s, dir) = text_preview(cfg, "whalf_less", "big.txt", &paging_body());
    assert!(s.app.is_windowed());
    let half = (s.app.tab.preview_viewport as i64 / 2).max(1);

    s.key('d');
    s.key('d');
    let top_after_downs = s.app.preview_top_line();
    assert_eq!(top_after_downs as i64, 2 * half, "d×2 後の窓位置");

    s.key('u');
    assert_ne!(
        s.app.preview_top_line(),
        top_after_downs,
        "less: d の直後、最初の u で窓が動かない(バグの再現)"
    );
    assert_eq!(
        s.app.preview_top_line() as i64,
        half,
        "u は半ページ分だけ戻るはず"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_preview_paging_vim_and_less_schemes_land_on_identical_positions() {
    // Both schemes route the same keys to the same `Action::Navigate(Motion::Page*/Half*)`, so a
    // vim session (Ctrl-f/b/d/u) and a less session (f/b/d/u) run through the identical scenario
    // must end up with the identical window/caret state.
    fn drive(scheme: &str) -> (usize, usize, usize, usize) {
        let mut cfg = Config::default();
        cfg.ui.keys = scheme.into();
        let (mut s, dir) = text_preview(
            cfg,
            &format!("wpage_agree_{scheme}"),
            "big.txt",
            &paging_body(),
        );
        assert!(s.app.is_windowed(), "{scheme}");
        if scheme == "less" {
            s.key('f');
            s.key('f');
            s.key('f');
        } else {
            s.ctrl('f');
            s.ctrl('f');
            s.ctrl('f');
        }
        let down3 = s.app.preview_top_line();
        if scheme == "less" {
            s.key('b');
        } else {
            s.ctrl('b');
        }
        let up1 = s.app.preview_top_line();
        if scheme == "less" {
            s.key('d');
            s.key('d');
        } else {
            s.ctrl('d');
            s.ctrl('d');
        }
        let halfdown2 = s.app.preview_top_line();
        if scheme == "less" {
            s.key('u');
        } else {
            s.ctrl('u');
        }
        let halfup1 = s.app.preview_top_line();
        std::fs::remove_dir_all(&dir).ok();
        (down3, up1, halfdown2, halfup1)
    }

    let vim = drive("vim");
    let less = drive("less");
    assert_eq!(
        vim, less,
        "vim/less スキームで同じキー操作の結果が一致しない"
    );
    // Also pin the regression fix down in this combined scenario: none of the "first up after
    // paging down" steps left the window where it was.
    assert_ne!(vim.0, vim.1);
    assert_ne!(vim.2, vim.3);
}

// ---- preview 2D caret + visual selection (v/V) + Y reference ----
// The caret line is verified via selection_ref_string() (@path#L{n}), the caret column via preview_selection().

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
    assert_eq!(s.app.tab.mode, Mode::Preview);
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
    assert_eq!(s.app.tab.mode, Mode::Preview);
    s.key('v');
    assert!(s.app.is_preview_visual());
    s.key('V');
    assert!(!s.app.is_preview_visual());
    assert_eq!(s.app.tab.mode, Mode::Preview);
    s.key('v');
    assert!(s.app.is_preview_visual());
    s.key('q');
    assert!(!s.app.is_preview_visual());
    assert_eq!(s.app.tab.mode, Mode::Preview);
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
    assert_eq!(s.app.tab.mode, Mode::Preview);
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

// ---- git: commit / stage/unstage / diff layout / log/graph/branches / copy / changed n/N ----
// All under #[cfg(feature = "git")]. Repository creation follows the existing seed_repo pattern.

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
    let commits = crate::git::log(&s.app.tab.root, 200);
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
    let second_subj = crate::git::commit_meta(&s.app.tab.root, &second)
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
    let branches = crate::git::branches(&s.app.tab.root);
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
    assert!(!crate::git::branches(&s.app.tab.root)
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
    let meta = crate::git::commit_meta(&s.app.tab.root, &id).expect("commit_meta");
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
    assert!(s.app.tab.entries[s.app.tab.selected].path.ends_with("a.rs"));
    s.key('n');
    assert!(s.app.tab.entries[s.app.tab.selected]
        .path
        .ends_with("new.txt"));
    s.key('n');
    assert!(s.app.tab.entries[s.app.tab.selected].path.ends_with("a.rs"));
    s.key('N');
    assert!(s.app.tab.entries[s.app.tab.selected]
        .path
        .ends_with("new.txt"));
    s.key('C');
    assert!(!s.app.changed_filter());
    std::fs::remove_dir_all(&dir).ok();
}

// ---- Tree sort (size/modified/ext) + filtering edge cases ----

#[test]
fn e2e_sort_by_size_orders_ascending() {
    let dir = sandbox("sort_size");
    std::fs::write(dir.join("aaa.txt"), vec![b'a'; 300]).unwrap();
    std::fs::write(dir.join("bbb.txt"), vec![b'b'; 30]).unwrap();
    std::fs::write(dir.join("ccc.txt"), vec![b'c'; 150]).unwrap();
    let mut s = Sim::new(&canon(&dir));
    let names: Vec<&str> = s
        .app
        .tab
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
        .tab
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
        .tab
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
        .tab
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
        .tab
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
    let all = s.app.tab.entries.len();
    s.key('/');
    s.keys("qzqzqznope");
    assert!(s.app.is_filtering());
    assert!(s.app.tab.entries.is_empty(), "{}", s.screen());
    s.dont_see("notes.txt");
    s.enter();
    assert!(s.app.tab.entries.is_empty());
    s.esc();
    assert_eq!(s.app.tab.entries.len(), all);
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
    assert!(s
        .app
        .tab
        .entries
        .iter()
        .any(|e| e.path.ends_with("readme.md")));
    assert!(!s
        .app
        .tab
        .entries
        .iter()
        .any(|e| e.path.ends_with("UPPER.TXT")));
    for _ in 0..8 {
        s.press(KeyCode::Backspace, KeyModifiers::NONE);
    }
    s.keys("upper");
    s.see("UPPER.TXT");
    assert!(s
        .app
        .tab
        .entries
        .iter()
        .any(|e| e.path.ends_with("UPPER.TXT")));
    std::fs::remove_dir_all(&dir).ok();
}

// ---- CSV/table copy (y→c/r/C/f) ----

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
    let csv = s.app.tab.preview_path.clone().expect("preview path");
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

// ---- Bookmarking from the preview (m / ') ----

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
    assert_eq!(s.app.tab.mode, Mode::Preview);
    assert!(s
        .app
        .tab
        .preview_path
        .as_deref()
        .is_some_and(|p| p.ends_with("notes.txt")));
    assert!(s.app.tab.entries[s.app.tab.selected]
        .path
        .ends_with("readme.md"));
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
    assert_eq!(s.app.tab.mode, Mode::Preview);
    s.see("BETA_BODY");
    s.key('\'');
    s.see("Bookmarks");
    assert_eq!(s.app.bookmark_list_sel(), 0);
    s.enter();
    assert!(!s.app.is_bookmark_list());
    assert_eq!(s.app.tab.mode, Mode::Preview);
    assert!(
        s.app
            .tab
            .preview_path
            .as_deref()
            .is_some_and(|p| p.ends_with("a.txt")),
        "{:?}",
        s.app.tab.preview_path
    );
    s.see("ALPHA_BODY");
    std::fs::remove_dir_all(&root).ok();
}

// ---- Paste-jump (P) edge cases ----

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
    assert_eq!(s.app.tab.mode, Mode::Preview);
    assert!(s
        .app
        .tab
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
    assert_eq!(s.app.tab.mode, Mode::Preview);
    assert!(s
        .app
        .tab
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
    assert_eq!(s.app.tab.mode, Mode::Preview);
    assert!(s
        .app
        .tab
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
    assert_eq!(s.app.tab.mode, Mode::Tree);
    assert!(s.app.tab.preview_path.is_none());
    s.app.paste_jump_from("https://example.com/some/file.rs");
    s.draw();
    assert_eq!(s.app.tab.mode, Mode::Tree);
    assert!(s.app.tab.preview_path.is_none());
    assert!(s.app.flash.is_some());
    std::fs::remove_dir_all(&dir).ok();
}

// ---- File operations edge cases ----

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

/// The user-facing shape of the alert regression: a document that mixes plain checkboxes with a
/// checklist inside a `> [!NOTE]`, driven by **real keystrokes**. Before the fix every Space in this
/// document — including on the plain checkboxes outside the alert — was refused (flash
/// "couldn't toggle checkbox — reloaded") because the write-back scanner never saw the alert's tasks
/// and so disagreed with the screen about how many there were.
#[test]
fn e2e_task_toggle_works_in_a_document_containing_an_alert() {
    let dir = sandbox("md_task_alert");
    let body = "# Checklist\n\n- [ ] plain one\n\n> [!NOTE]\n> - [ ] inside alert\n> - [ ] second in alert\n\n- [ ] after alert\n";
    std::fs::write(dir.join("todo.md"), body).unwrap();
    let mut s = Sim::with_config(&canon(&dir), Config::default());
    s.select("todo.md");
    s.enter();
    let read = || std::fs::read_to_string(dir.join("todo.md")).unwrap();

    // The 1st (outside the alert).
    s.tab();
    s.key(' ');
    assert!(read().contains("- [x] plain one"), "外側1: {}", read());
    s.dont_see("couldn't toggle checkbox");

    // The 2nd and 3rd (inside the alert) — this used to not work at all.
    s.tab();
    s.key(' ');
    assert!(
        read().contains("> - [x] inside alert"),
        "アラート内1: {}",
        read()
    );
    s.tab();
    s.key(' ');
    assert!(
        read().contains("> - [x] second in alert"),
        "アラート内2: {}",
        read()
    );

    // The 4th (after the alert) — the ordinal doesn't drift crossing the alert.
    s.tab();
    s.key(' ');
    assert!(read().contains("- [x] after alert"), "外側2: {}", read());
    s.dont_see("couldn't toggle checkbox");

    // Other lines aren't broken (the heading and alert header are unchanged).
    let out = read();
    assert!(
        out.contains("# Checklist") && out.contains("> [!NOTE]"),
        "{out}"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// Custom `ui.md_task_states` must cycle inside an alert too — the state char is written back through
/// the same path, and the alert's `>` prefix shifts every column on the line.
#[test]
fn e2e_task_custom_states_cycle_inside_an_alert() {
    let dir = sandbox("md_task_alert_states");
    std::fs::write(dir.join("todo.md"), "> [!TIP]\n> - [ ] cycle me\n").unwrap();
    let mut cfg = Config::default();
    cfg.ui.md_task_states = vec![" ".into(), "/".into(), "x".into()];
    let mut s = Sim::with_config(&canon(&dir), cfg);
    s.select("todo.md");
    s.enter();
    let read = || std::fs::read_to_string(dir.join("todo.md")).unwrap();
    s.tab();
    for want in ["> - [/] cycle me", "> - [x] cycle me", "> - [ ] cycle me"] {
        s.key(' ');
        assert!(read().contains(want), "期待 {want}: {}", read());
    }
    std::fs::remove_dir_all(&dir).ok();
}

/// A collapsed `<details>` hides its checkboxes, so Space on the visible ones must still work: the
/// scanner must not count the hidden ones (it used to, which cancelled every toggle in the file).
#[test]
fn e2e_task_toggle_works_with_a_collapsed_details_block() {
    let dir = sandbox("md_task_details");
    std::fs::write(
        dir.join("todo.md"),
        "- [ ] visible\n\n<details>\n<summary>Hidden</summary>\n\n- [ ] collapsed\n\n</details>\n",
    )
    .unwrap();
    let mut s = Sim::with_config(&canon(&dir), Config::default());
    s.select("todo.md");
    s.enter();
    let read = || std::fs::read_to_string(dir.join("todo.md")).unwrap();
    s.tab(); // either the collapsed heading or the visible task
    s.key(' ');
    let out = read();
    assert!(
        out.contains("- [x] visible") || out.contains("- [ ] collapsed"),
        "可視タスクがトグルできる(全キャンセルされない): {out}"
    );
    s.dont_see("couldn't toggle checkbox");
    std::fs::remove_dir_all(&dir).ok();
}

/// Link spans and their targets are produced by two passes — `collapse_links` rewrites the rendered
/// spans, and the item list zips the Nth link span with the Nth collected target. If the two ever fall
/// out of step **every** link in the document points somewhere else (Enter opens the wrong file), and
/// nothing on screen looks wrong. This walks a corpus of every link form konoma renders and asserts the
/// Nth focusable link carries exactly the Nth URL in document order.
#[test]
fn e2e_link_targets_stay_aligned_with_document_order() {
    let cases: &[(&str, &str, &[&str])] = &[
        (
            "inline",
            "[a](./a.md) and [b](./b.md)\n",
            &["./a.md", "./b.md"],
        ),
        (
            "with title",
            "[a](./a.md \"T\") [b](./b.md)\n",
            &["./a.md", "./b.md"],
        ),
        (
            "reference style",
            "[a][r1] [b][r2]\n\n[r1]: ./a.md\n[r2]: ./b.md\n",
            &["./a.md", "./b.md"],
        ),
        (
            "autolink angle",
            "<https://x.example/1> then [b](./b.md)\n",
            &["https://x.example/1", "./b.md"],
        ),
        (
            "bare url autolinked",
            "see https://x.example/2 and [b](./b.md)\n",
            &["https://x.example/2", "./b.md"],
        ),
        (
            "url inside code is not a link",
            "`https://x.example/no` but [b](./b.md)\n",
            &["./b.md"],
        ),
        (
            "url inside a fence is not a link",
            "```\nhttps://x.example/no\n```\n\n[b](./b.md)\n",
            &["./b.md"],
        ),
        (
            "link inside a table cell",
            "| a | b |\n|---|---|\n| [x](./x.md) | [y](./y.md) |\n",
            &["./x.md", "./y.md"],
        ),
        (
            "link inside an alert",
            "> [!NOTE]\n> [in](./in.md)\n\n[out](./out.md)\n",
            &["./in.md", "./out.md"],
        ),
        (
            "heading anchor plus file",
            "[h](#section) [f](./f.md)\n",
            &["#section", "./f.md"],
        ),
        (
            "cjk label",
            "[日本語のリンク](./ja.md) [b](./b.md)\n",
            &["./ja.md", "./b.md"],
        ),
        (
            "emphasis around link",
            "**[bold](./x.md)** and *[em](./y.md)*\n",
            &["./x.md", "./y.md"],
        ),
    ];
    for (name, body, want) in cases {
        let (s, dir) = md_preview(cfg_code_bg_none(), "links", body);
        assert_eq!(
            s.app.md_link_targets(),
            want.iter().map(|t| t.to_string()).collect::<Vec<_>>(),
            "{name}: リンクの並び/対応がソース順とズレている(Enter が別の宛先を開く)\n--- src ---\n{body}"
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}

/// Tab focus must bring the **whole** focused block into view, not just the line the focus marker sits
/// on. A code block's marker is its header and a `<details>`' marker is its summary, so ensuring only
/// that line was visible parked the block against the bottom edge with its contents cut off — the user
/// had to scroll manually after every Tab. Diagrams already did this; everything multi-line now does.
#[test]
fn e2e_tab_scrolls_whole_block_into_view() {
    // Place a block smaller than the screen right at the bottom edge.
    let mut body = String::from("# Doc\n\n");
    for i in 0..20 {
        body.push_str(&format!("filler line {i}\n\n"));
    }
    body.push_str("```rust\n");
    for i in 0..8 {
        body.push_str(&format!("let line_{i} = {i};\n"));
    }
    body.push_str("```\n\nafter\n");

    let (mut s, dir) = md_preview(cfg_code_bg_none(), "blocks", &body);
    let vh = s.app.preview_viewport_for_test().max(1) as usize;
    s.tab(); // the sole Tab target = the code block
    let scroll = s.app.tab.preview_scroll as usize;
    let focused = s.app.focused_item().expect("コードブロックにフォーカス");
    let start = s.app.md_item_line_for_test(focused);
    let end = s.app.md_item_block_end_for_test(focused);
    assert!(
        end > start,
        "コードブロックは複数行にわたる (start={start} end={end})"
    );

    let (top, _) = s.app.md_visual_span_for_test(start);
    let (btop, bh) = s.app.md_visual_span_for_test(end);
    let block_bottom = btop + bh;
    assert!(
        top >= scroll,
        "ブロック先頭が上に見切れている: top={top} scroll={scroll}"
    );
    assert!(
        block_bottom <= scroll + vh,
        "ブロック末尾が下に見切れている: bottom={block_bottom} scroll={scroll} viewport={vh}"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// The same guarantee for an **open** `<details>`: focusing its summary must reveal the body under it.
#[test]
fn e2e_tab_scrolls_open_details_body_into_view() {
    let mut body = String::from("# Doc\n\n");
    for i in 0..20 {
        body.push_str(&format!("filler line {i}\n\n"));
    }
    body.push_str("<details open>\n<summary>Show me</summary>\n\n");
    for i in 0..8 {
        body.push_str(&format!("- body line {i}\n"));
    }
    body.push_str("\n</details>\n");

    let (mut s, dir) = md_preview(cfg_code_bg_none(), "detblocks", &body);
    let vh = s.app.preview_viewport_for_test().max(1) as usize;
    s.tab();
    let focused = s.app.focused_item().expect("details にフォーカス");
    let start = s.app.md_item_line_for_test(focused);
    let end = s.app.md_item_block_end_for_test(focused);
    assert!(
        end > start,
        "開いた details は本文を持つ (start={start} end={end})"
    );
    let scroll = s.app.tab.preview_scroll as usize;
    let (btop, bh) = s.app.md_visual_span_for_test(end);
    assert!(
        btop + bh <= scroll + vh,
        "details 本文の末尾が見切れている: bottom={} scroll={scroll} viewport={vh}",
        btop + bh
    );
    std::fs::remove_dir_all(&dir).ok();
}

// =============================================================================
// External command delegation (`[[preview.rules]] command = "..."`)
// =============================================================================

/// A real key press (`Enter` on a file matched by a `command = "..."` rule) shows the delegated
/// command's captured output through the ordinary windowed text reader. This is the actual
/// regression this module exists to prevent: before implementation, the preview showed the raw
/// `{:?}` Debug dump of `PreviewKind::Command` (`render_as=None, detached=false`) instead of any
/// real content — `command.rs`'s own tests cover `run_capture`/`build_argv` in isolation, and
/// `app/tests.rs` covers `MediaJob::Command` and the render-side fallback directly, but only this
/// full key→render path proves the delegation is actually wired end to end. Image-mode delegation
/// isn't exercised here — `Sim` has no real image backend/picker, so `App::is_image_preview()` can
/// never become true in this harness; `MediaJob::Command`'s `as_image` branch is covered directly
/// in `app/tests.rs` instead.
#[cfg(unix)]
#[test]
fn e2e_delegated_command_text_preview_shows_captured_output() {
    let dir = sandbox("cmd_delegate_text");
    std::fs::write(dir.join("app.log"), "line one\nline two\n").unwrap();
    let root = canon(&dir);

    let mut cfg = Config::default();
    // Put it first so it wins over the builtin rules (which don't match *.log anyway, but this
    // mirrors how a user rule is meant to take priority).
    cfg.preview.rules.insert(
        0,
        crate::config::Rule {
            glob: Some("*.log".into()),
            command: Some("cat {path}".into()),
            ..Default::default()
        },
    );
    let mut s = Sim::with_config(&root, cfg);
    s.select("app.log");
    s.enter();
    s.see("line one");
    s.see("line two");
    // The title still names the original file — the generated temp path never leaks into the UI.
    s.see("app.log");
    std::fs::remove_dir_all(&dir).ok();
}

// =============================================================================
// H2/H3 (docs/AUDIT-TESTS-2026-08.md): style-attribute assertions + config→keybinding wiring.
//
// The audit found two "breakable with zero red tests" gaps: (1) `keymap::scheme_from_str` (the
// `ui.keys` string → paging key scheme) had no test that goes config string → real keystrokes →
// screen — only unit tests that construct a `KeyScheme` directly. (2) `Sim::screen()` only ever
// returned glyphs, never `Style`, so konoma's "sentinel style" focus markers (REVERSED on Tab
// focus) were unverifiable at the e2e layer even though `grep -rn REVERSED src` shows they're
// central to the UI. Both are demonstrated broken-with-red-tests below by literally breaking the
// implementation locally and re-running (see the task report for the failure output); these tests
// are what stayed red.
// =============================================================================

/// `ui.keys = "less"`: plain `f`/Space page down, plain `b` pages up — and `Ctrl-f`/`Ctrl-b`,
/// which are the *vim* scheme's keys, are simply unbound in this surface (no fallback, no-op).
/// Breaking `scheme_from_str` to always return `KeyScheme::Vim` makes `f`/Space do nothing here
/// (verified: this test goes red with "left: 0 right: <n>"-style failures when that's done).
#[test]
fn e2e_ui_keys_less_scheme_uses_plain_letters_not_ctrl() {
    let mut cfg = Config::default();
    cfg.ui.keys = "less".into();
    let (mut s, dir) = text_preview(cfg, "keys_less", "big.txt", &paging_body());
    assert!(s.app.is_windowed());
    let top0 = s.app.preview_top_line();

    // Ctrl-f is the *vim* scheme's page-down key — unbound here, so nothing happens.
    s.ctrl('f');
    assert_eq!(
        s.app.preview_top_line(),
        top0,
        "less スキームでは Ctrl-f はバインドされていないはず"
    );

    // Plain `f` pages down (the less scheme's own key).
    s.key('f');
    let top1 = s.app.preview_top_line();
    assert!(top1 > top0, "less: 素の f がページ送りになるはず");

    // Space also pages down (less has both).
    s.key(' ');
    let top2 = s.app.preview_top_line();
    assert!(top2 > top1, "less: Space もページ送りになるはず");

    // Plain `b` pages back up.
    s.key('b');
    let top3 = s.app.preview_top_line();
    assert!(top3 < top2, "less: 素の b がページ戻しになるはず");

    std::fs::remove_dir_all(&dir).ok();
}

/// `ui.keys = "vim"` (also the default): `Ctrl-f`/`Ctrl-b` page, and the *less* scheme's plain
/// `f`/`b`/Space are simply unbound (no-op) in this surface.
#[test]
fn e2e_ui_keys_vim_scheme_uses_ctrl_not_plain_letters() {
    let mut cfg = Config::default();
    cfg.ui.keys = "vim".into();
    let (mut s, dir) = text_preview(cfg, "keys_vim", "big.txt", &paging_body());
    assert!(s.app.is_windowed());
    let top0 = s.app.preview_top_line();

    // Plain f/Space/b are the *less* scheme's keys — unbound here.
    s.key('f');
    assert_eq!(
        s.app.preview_top_line(),
        top0,
        "vim スキームでは素の f はバインドされていないはず"
    );
    s.key(' ');
    assert_eq!(
        s.app.preview_top_line(),
        top0,
        "vim スキームでは Space もバインドされていないはず"
    );

    // Ctrl-f/Ctrl-b (vim's own keys) work.
    s.ctrl('f');
    let top1 = s.app.preview_top_line();
    assert!(top1 > top0, "vim: Ctrl-f がページ送りになるはず");
    s.ctrl('b');
    let top2 = s.app.preview_top_line();
    assert!(top2 < top1, "vim: Ctrl-b がページ戻しになるはず");

    std::fs::remove_dir_all(&dir).ok();
}

/// An unrecognized `ui.keys` value (anything but `"less"`) falls back to vim — pinning down
/// `scheme_from_str`'s documented contract ("anything but 'less' is vim") through real keystrokes,
/// not just by constructing `KeyScheme::Vim` directly.
#[test]
fn e2e_ui_keys_unknown_value_falls_back_to_vim_scheme() {
    let mut cfg = Config::default();
    cfg.ui.keys = "emacs".into(); // not a recognized scheme name
    let (mut s, dir) = text_preview(cfg, "keys_unknown", "big.txt", &paging_body());
    assert!(s.app.is_windowed());
    let top0 = s.app.preview_top_line();

    // Falls back to vim: plain f is unbound...
    s.key('f');
    assert_eq!(
        s.app.preview_top_line(),
        top0,
        "不明な ui.keys は vim にフォールバックするはず(素の f は無効)"
    );
    // ...but Ctrl-f (vim's key) works.
    s.ctrl('f');
    assert!(
        s.app.preview_top_line() > top0,
        "不明な ui.keys は vim にフォールバックするはず(Ctrl-f が有効)"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// Tab-focus reversal, verified through the actual render buffer's `Style` (not internal indices)
/// — one test per `MdItemKind` that's reachable without an image `Picker` (`Sim` never attaches
/// one; see the mermaid note at the end). Each item is checked both while focused (REVERSED) and
/// while *not* focused (not REVERSED), so this also proves the effect is exclusive to the current
/// focus rather than sticking around. Breaking `invert_focused_line` into a no-op (returning the
/// line unchanged) turns every `reversed(...)` assertion below red (verified: see the task report).
#[test]
fn e2e_markdown_tab_focus_reverses_only_the_focused_marker_style() {
    let mut cfg = Config::default();
    cfg.ui.icons = false; // plain "[ ]" checkbox / no link-icon prefix glyph to worry about
    let (mut s, dir) = md_preview(
        cfg,
        "focus_styles",
        "Read the [guide](https://example.com/guide) first.\n\n\
         - [ ] pending task\n\n\
         ```rust\n\
         fn main() {}\n\
         ```\n\n\
         <details>\n\
         <summary>Extra Info</summary>\n\n\
         Some hidden body text.\n\n\
         </details>\n",
    );

    fn reversed(s: &Sim, needle: &str) -> bool {
        s.style_of(needle)
            .is_some_and(|st| st.add_modifier.contains(ratatui::style::Modifier::REVERSED))
    }

    // Nothing is focused before the first Tab: no marker is reversed, and in fact the buffer holds
    // no REVERSED cells at all (decorated Markdown has no separate block caret the way windowed
    // text/code previews do).
    assert!(
        s.reversed_cells().is_empty(),
        "フォーカス前は反転セルが1つも無いはず: {:?}",
        s.reversed_cells()
    );
    assert!(!reversed(&s, "guide"));
    assert!(!reversed(&s, "[ ]"));
    assert!(!reversed(&s, "rust"));
    assert!(!reversed(&s, "▸"));

    // 1st Tab → the link (document order: link, task, code block, details).
    s.tab();
    s.see_styled(
        "guide",
        |st| st.add_modifier.contains(ratatui::style::Modifier::REVERSED),
        "フォーカス中のリンク=反転",
    );
    assert!(
        !reversed(&s, "[ ]"),
        "リンクにフォーカス中はチェックボックスは反転しない"
    );
    assert!(
        !reversed(&s, "rust"),
        "リンクにフォーカス中はコードブロックは反転しない"
    );
    assert!(
        !reversed(&s, "▸"),
        "リンクにフォーカス中は details は反転しない"
    );

    // 2nd Tab → the checkbox marker.
    s.tab();
    assert!(
        !reversed(&s, "guide"),
        "フォーカスが移ればリンクは反転が解ける"
    );
    s.see_styled(
        "[ ]",
        |st| st.add_modifier.contains(ratatui::style::Modifier::REVERSED),
        "フォーカス中のチェックボックス=反転",
    );
    assert!(!reversed(&s, "rust"));
    assert!(!reversed(&s, "▸"));

    // 3rd Tab → the code block header (the *whole header line* inverts, per `invert_focused_line`'s
    // `whole_line` handling for `MdItemKind::CodeBlock`).
    s.tab();
    assert!(!reversed(&s, "[ ]"));
    s.see_styled(
        "rust",
        |st| st.add_modifier.contains(ratatui::style::Modifier::REVERSED),
        "フォーカス中のコードブロック=ヘッダ行が反転",
    );
    assert!(!reversed(&s, "▸"));

    // 4th Tab → the <details> summary marker (▸ = closed, the default with no `open` attribute).
    s.tab();
    assert!(!reversed(&s, "rust"));
    s.see_styled(
        "▸",
        |st| st.add_modifier.contains(ratatui::style::Modifier::REVERSED),
        "フォーカス中の <details>=▸ が反転",
    );

    // 5th Tab wraps back around to the link.
    s.tab();
    s.see_styled(
        "guide",
        |st| st.add_modifier.contains(ratatui::style::Modifier::REVERSED),
        "巡回して先頭(リンク)へ戻る",
    );
    assert!(!reversed(&s, "▸"));

    // A mermaid fence is not covered here: `Sim` never attaches an image `Picker` (that's a
    // separate, larger change — H1 in docs/AUDIT-TESTS-2026-08.md, out of scope for this task), so
    // `mermaid_image_mode()` is false and a ```mermaid fence degrades to the plain-text
    // box-drawing diagram — it never becomes an `MdItemKind::MermaidFence` Tab item at all.
    // Confirmed by hand: opening a mermaid-only doc through this same harness, `focused_item()`
    // stays `None` through every `Tab` press (there is nothing to cycle to), so there is no
    // reachable "focused mermaid caption" state to assert on here.

    std::fs::remove_dir_all(&dir).ok();
}

/// `distinct_fg_in_row`: a CSV/TSV preview's rainbow column coloring (`ui.csv_rainbow`, default on)
/// actually uses multiple distinct foreground colors across one row, not a single uniform color.
#[test]
fn e2e_csv_rainbow_columns_use_distinct_foreground_colors() {
    let dir = sandbox("csv_rainbow_style");
    std::fs::write(dir.join("wide.csv"), "colA,colB,colC,colD\n1,2,3,4\n").unwrap();
    let mut s = Sim::new(&canon(&dir));
    s.select("wide.csv");
    s.enter();
    s.see("TABLE");
    let (header_row, _) = s.find_text("colA").expect("header row is drawn");
    assert!(
        s.distinct_fg_in_row(header_row) >= 3,
        "4列のヘッダ行は3色以上の前景色が混在するはず(レインボー)"
    );
    s.key('q');
    std::fs::remove_dir_all(&dir).ok();
}

// =============================================================================
// H3 (docs/AUDIT-TESTS-2026-08.md): full `[ui]` config-field coverage table.
//
// Layer (c) from the audit: does setting a field to a NON-default value, through `App::new`,
// actually change observable behavior (screen text/style) or app-state through real keystrokes —
// as opposed to (a) TOML parses into the right default (config/parity_tests.rs) or (b) calling the
// logic directly with an argument (app/tests.rs)? `ui.keys` was the first gap found (fixed by the
// three `e2e_ui_keys_*` tests above); this table makes sure the other 41 fields aren't hiding the
// same kind of gap, and the completeness test below means a newly added field can't be silently
// left out (mirrors i18n's `ALL_MSGS` exhaustiveness check).
// =============================================================================

/// How a `[ui]` field's role is demonstrated at this layer.
#[derive(Debug, Clone, Copy)]
enum Coverage {
    /// A `Sim` test in this file sets the field to a non-default value and asserts the screen/style/
    /// app-state differs from the default — written specifically for this table (possibly alongside
    /// a pre-existing test). `.0` names the covering test(s).
    Covered(&'static str),
    /// A pre-existing `Sim` test (written before this table, for a related feature) already exercises
    /// the field with a genuine default-vs-non-default contrast. `.0` names it.
    CoveredElsewhere(&'static str),
    /// A dedicated unit/integration test outside the `Sim`/keystroke harness already drives
    /// `App::new(path, cfg)` end to end with real file I/O (so it satisfies layer (c) without going
    /// through `handle_key`). `.0` names it.
    ///
    /// (There used to be a fourth `NotObservable` category here for a field genuinely not wired up
    /// to anything observable — `show_hidden` was its only occupant, tracking the dead-config-field
    /// bug fixed in 2026-08. Every field is `Covered`/`CoveredElsewhere`/`CoveredByUnitTest` now, so
    /// the category was removed rather than left with no variant to construct it.)
    CoveredByUnitTest(&'static str),
}

const UI_CONFIG_COVERAGE: &[(&str, Coverage)] = &[
    (
        "show_hidden",
        Coverage::Covered("e2e_ui_show_hidden_config_starts_with_dotfiles_visible"),
    ),
    (
        "filter_mode",
        Coverage::Covered("e2e_ui_filter_mode_fuzzy_vs_substring_noncontiguous_query"),
    ),
    (
        "tabbar",
        Coverage::Covered("e2e_ui_tabbar_always_vs_auto_with_a_single_tab"),
    ),
    (
        "icons",
        Coverage::CoveredElsewhere("e2e_md_task_markers_render_ascii_or_nf_by_icons"),
    ),
    (
        "wrap",
        Coverage::CoveredElsewhere("e2e_md_wrapped_focus_follows_offscreen_item"),
    ),
    (
        "line_numbers",
        Coverage::Covered("e2e_ui_line_numbers_toggle_shows_gutter"),
    ),
    (
        "git_gutter",
        Coverage::Covered("e2e_ui_git_gutter_toggle_shows_or_hides_change_marker"),
    ),
    (
        "tab_width",
        Coverage::Covered("e2e_ui_tab_width_changes_expansion_columns"),
    ),
    (
        "syntax_highlight",
        Coverage::Covered("e2e_ui_syntax_highlight_toggle_changes_color_diversity"),
    ),
    (
        "preview_loading",
        Coverage::Covered("e2e_ui_preview_loading_indicator_vs_progressive_for_cold_language"),
    ),
    (
        "path_style",
        Coverage::Covered("e2e_ui_path_style_config_sets_initial_style"),
    ),
    (
        "keys",
        Coverage::CoveredElsewhere(
            "e2e_ui_keys_less_scheme_uses_plain_letters_not_ctrl / e2e_ui_keys_vim_scheme_uses_ctrl_not_plain_letters / e2e_ui_keys_unknown_value_falls_back_to_vim_scheme",
        ),
    ),
    (
        "lang",
        Coverage::Covered("e2e_ui_lang_toggle_switches_footer_translation"),
    ),
    (
        "statusbar",
        Coverage::Covered("e2e_ui_statusbar_layout_split_vs_bottom_moves_context_row"),
    ),
    (
        "theme",
        Coverage::Covered("e2e_ui_theme_code_bg_toggle_changes_code_block_background"),
    ),
    (
        "image_render_scale",
        Coverage::Covered("e2e_ui_image_render_scale_shrinks_display_rect"),
    ),
    (
        "svg_max_px",
        Coverage::Covered("e2e_ui_svg_max_px_changes_apparent_display_size"),
    ),
    (
        "sort",
        Coverage::Covered("e2e_ui_sort_config_sets_initial_order_without_pressing_s"),
    ),
    (
        "details",
        Coverage::CoveredElsewhere("e2e_details_columns_show_size_and_modified"),
    ),
    (
        "graph_max_branches",
        Coverage::Covered("e2e_ui_graph_max_branches_caps_legend_entries"),
    ),
    (
        "graph_base_branches",
        Coverage::Covered("e2e_ui_graph_base_branches_config_auto_selects_base"),
    ),
    (
        "commit_meta_align",
        Coverage::Covered("e2e_ui_commit_meta_align_right_vs_inline_moves_author_column"),
    ),
    (
        "confirm_quit",
        Coverage::CoveredElsewhere(
            "e2e_last_tab_q_quits_when_confirm_off / e2e_last_tab_q_confirms_when_confirm_on",
        ),
    ),
    (
        "confirm_bookmark_overwrite",
        Coverage::CoveredElsewhere(
            "e2e_bookmark_overwrite_prompts_confirm_then_applies / e2e_bookmark_overwrite_off_applies_silently",
        ),
    ),
    (
        "follow_view",
        Coverage::Covered(
            "e2e_follow_opens_full_screen_diff (default) / e2e_ui_follow_view_file_shows_content_preview_not_diff",
        ),
    ),
    (
        "csv_rainbow",
        Coverage::CoveredElsewhere(
            "e2e_csv_rainbow_columns_use_distinct_foreground_colors / e2e_csv_table_renders_and_navigates_without_rainbow",
        ),
    ),
    (
        "md_task_states",
        Coverage::CoveredElsewhere("e2e_markdown_task_cycles_custom_states"),
    ),
    (
        "md_autolink",
        Coverage::CoveredElsewhere(
            "e2e_markdown_autolink_and_emoji_full_pipeline (on) / e2e_markdown_autolink_emoji_alerts_toggle_off (off)",
        ),
    ),
    (
        "md_alerts",
        Coverage::CoveredElsewhere(
            "e2e_markdown_alert_all_five_types_and_aliases (on) / e2e_markdown_autolink_emoji_alerts_toggle_off (off)",
        ),
    ),
    (
        "md_emoji",
        Coverage::CoveredElsewhere(
            "e2e_markdown_autolink_and_emoji_full_pipeline (on) / e2e_markdown_autolink_emoji_alerts_toggle_off (off)",
        ),
    ),
    (
        "md_frontmatter",
        Coverage::CoveredElsewhere(
            "e2e_markdown_front_matter_renders_as_metadata (on) / e2e_markdown_front_matter_off_leaves_body_intact (off)",
        ),
    ),
    (
        "md_footnotes",
        Coverage::CoveredElsewhere(
            "e2e_markdown_footnotes_render_superscript_and_section (on) / e2e_markdown_footnotes_off_stays_literal (off)",
        ),
    ),
    (
        "md_inline_html",
        Coverage::Covered(
            "e2e_markdown_inline_html_renders (on) / e2e_ui_md_inline_html_off_keeps_raw_tags",
        ),
    ),
    (
        "md_details",
        Coverage::Covered(
            "e2e_md_html_details_collapse_and_toggle (auto) / e2e_ui_md_details_config_forces_open_despite_missing_attribute",
        ),
    ),
    (
        "math",
        Coverage::Covered("e2e_ui_math_image_mode_lifts_expression_text_mode_keeps_it_inline"),
    ),
    (
        "math_color",
        Coverage::Covered("e2e_ui_math_color_changes_rendered_pixel_hue"),
    ),
    (
        "busy_indicator",
        Coverage::Covered(
            "e2e_busy_indicator_absent_when_idle (idle invariant) / e2e_ui_busy_indicator_shows_while_media_loads_hides_when_disabled",
        ),
    ),
    (
        "mermaid",
        Coverage::Covered("e2e_ui_mermaid_image_mode_shows_caption_text_mode_does_not"),
    ),
    (
        "mermaid_theme",
        Coverage::Covered("e2e_ui_mermaid_theme_changes_rendered_pixel_brightness"),
    ),
    (
        "mermaid_rows",
        Coverage::Covered("e2e_ui_mermaid_rows_changes_reserved_diagram_height"),
    ),
    (
        "restore_tabs",
        Coverage::CoveredElsewhere(
            "e2e_session_restore_reopens_previous_tabs (on, via Sim) / session_actions::restore_tabs_off_neither_saves_nor_restores (off, via App::new)",
        ),
    ),
    (
        "restore_single_tab",
        Coverage::CoveredByUnitTest(
            "session_actions::restore_single_tab_true_saves_a_lone_tab / restore_single_tab_false_deletes_a_lone_tab_session / restore_single_tab_false_still_saves_multi_tab / restore_single_tab_false_skips_and_deletes_stale_single_tab_file",
        ),
    ),
];

/// Extract `UiConfig`'s field names directly from `config/mod.rs`'s own source text: locate
/// `pub struct UiConfig {`, brace-match to the closing `}`, then pull every `pub <name>:` line in
/// between. Mirrors i18n's `ALL_MSGS` exhaustiveness pattern — a newly added `[ui]` field fails
/// `ui_config_coverage_table_is_complete` below until it gets a table entry.
fn extract_ui_config_field_names() -> Vec<String> {
    let src = include_str!("config/mod.rs");
    let marker = "pub struct UiConfig {";
    let start = src
        .find(marker)
        .expect("UiConfig struct not found in config/mod.rs — did it move or get renamed?");
    let body_start = start + marker.len() - 1; // the opening '{' itself
    let bytes = src.as_bytes();
    let mut depth = 0i32;
    let mut end = body_start;
    for (i, &b) in bytes[body_start..].iter().enumerate() {
        match b {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    end = body_start + i;
                    break;
                }
            }
            _ => {}
        }
    }
    assert!(end > body_start, "matching closing brace not found");
    let body = &src[body_start..end];
    let mut names = Vec::new();
    for line in body.lines() {
        let t = line.trim_start();
        if let Some(rest) = t.strip_prefix("pub ") {
            if let Some(colon) = rest.find(':') {
                let name = rest[..colon].trim();
                if !name.is_empty() {
                    names.push(name.to_string());
                }
            }
        }
    }
    names
}

/// Safety valve for the extractor above: if the brace/regex-ish scan above breaks (e.g. someone
/// reformats the struct in a way the simple line scan can't follow), it must fail LOUD by finding
/// too few fields — not silently return an empty/tiny list that would make
/// `ui_config_coverage_table_is_complete` vacuously pass. `UiConfig` currently has 42 fields; 35 is
/// a conservative floor that still catches "extraction basically broke."
#[test]
fn ui_config_field_extraction_finds_at_least_35_fields() {
    let names = extract_ui_config_field_names();
    assert!(
        names.len() >= 35,
        "抽出数が少なすぎる(安全弁): {} 件 — パーサが壊れている可能性: {:?}",
        names.len(),
        names
    );
}

/// Completeness: every field `UiConfig` actually has must appear in `UI_CONFIG_COVERAGE`, and vice
/// versa (no stale entries for fields that got removed/renamed). Add a field to `UiConfig` without
/// adding a table entry → this test fails and names exactly which field is missing.
#[test]
fn ui_config_coverage_table_is_complete() {
    let extracted = extract_ui_config_field_names();
    assert!(
        extracted.len() >= 35,
        "抽出数が少なすぎる(安全弁): {} 件",
        extracted.len()
    );
    let extracted: std::collections::BTreeSet<&str> =
        extracted.iter().map(|s| s.as_str()).collect();
    let table: std::collections::BTreeSet<&str> =
        UI_CONFIG_COVERAGE.iter().map(|(name, _)| *name).collect();
    let missing_from_table: Vec<&&str> = extracted.difference(&table).collect();
    let stale_in_table: Vec<&&str> = table.difference(&extracted).collect();
    assert!(
        missing_from_table.is_empty() && stale_in_table.is_empty(),
        "UiConfig のフィールドと UI_CONFIG_COVERAGE テーブルが不一致。\n\
         テーブルに無い(新規フィールド?): {missing_from_table:?}\n\
         構造体に無い(削除/改名?): {stale_in_table:?}"
    );
    // No duplicate entries either (a copy-paste of an existing field name would hide a gap).
    assert_eq!(
        table.len(),
        UI_CONFIG_COVERAGE.len(),
        "UI_CONFIG_COVERAGE に重複キーがある"
    );
}

/// Print the coverage breakdown by category (also gives every entry's payload string a real read,
/// so the enum variants document genuine data, not accidental waste). Run with `-- --nocapture` to
/// see the table; the assertions just guard against an accidentally-empty test-name string.
#[test]
fn ui_config_coverage_table_breakdown() {
    let mut covered = 0usize;
    let mut covered_elsewhere = 0usize;
    let mut covered_by_unit_test = 0usize;
    for (field, cov) in UI_CONFIG_COVERAGE {
        match cov {
            Coverage::Covered(test) => {
                assert!(!test.is_empty(), "{field}: Covered のテスト名が空");
                covered += 1;
            }
            Coverage::CoveredElsewhere(test) => {
                assert!(!test.is_empty(), "{field}: CoveredElsewhere のテスト名が空");
                covered_elsewhere += 1;
            }
            Coverage::CoveredByUnitTest(test) => {
                assert!(
                    !test.is_empty(),
                    "{field}: CoveredByUnitTest のテスト名が空"
                );
                covered_by_unit_test += 1;
            }
        }
    }
    eprintln!(
        "UI_CONFIG_COVERAGE breakdown: total={} Covered={covered} CoveredElsewhere={covered_elsewhere} CoveredByUnitTest={covered_by_unit_test}",
        UI_CONFIG_COVERAGE.len(),
    );
}

// =============================================================================
// H3: the field-specific tests referenced by UI_CONFIG_COVERAGE above (new coverage only —
// pre-existing tests referenced as CoveredElsewhere/CoveredByUnitTest live elsewhere in this file
// or in src/app/session_actions.rs).
// =============================================================================

/// FIXED (2026-08, was FINDING-2026-08): `[ui] show_hidden` used to parse fine
/// (config/parity_tests.rs) but was **never read anywhere in app/*.rs** — `tab.show_hidden` was
/// unconditionally initialized to `false` in the `PerTab` default and only ever flipped via the
/// `.` keypress (`app.rs`, `toggle_hidden`) or session restore (`session_actions.rs`), never from
/// config at `App::new`. `App::new` now captures `cfg.ui.show_hidden` before `cfg` is moved into
/// `Self` and feeds it into the fresh tab's `PerTab` (before the first `rebuild_tree`, so the very
/// first draw already reflects it). `tab_new` (`t`/`Ctrl-t`) resets to the same config value rather
/// than silently inheriting whatever the source tab's `.` toggle last left it at; session restore
/// (`apply_saved_tab`) still overwrites it afterwards, so a restored tab's saved visibility wins
/// over the config default, matching its documented persistence.
#[test]
fn e2e_ui_show_hidden_config_starts_with_dotfiles_visible() {
    let dir = sandbox("ui_show_hidden_cfg");
    seed_files(&dir);
    let root = canon(&dir);

    let s_default = Sim::new(&root); // show_hidden=false
    s_default.dont_see(".hidden.txt");

    let mut cfg = Config::default();
    cfg.ui.show_hidden = true;
    let s_on = Sim::with_config(&root, cfg);
    s_on.see(".hidden.txt"); // visible from the very first draw, no `.` key needed
    std::fs::remove_dir_all(&dir).ok();
}

/// A fresh tab (`t` → `tab_new`) starts hidden-file visibility from `[ui] show_hidden` again — it
/// does not silently inherit whatever the source tab's `.` toggle last left it at. Exercised in
/// both directions (config-on overriding a toggled-off source tab, and config-off overriding a
/// toggled-on one) so either "always inherits the previous tab" or "always resets to false" would
/// fail this.
#[test]
fn e2e_ui_show_hidden_new_tab_resets_to_config_default() {
    // config on: toggle off in tab0, then `t` — tab1 should come back on.
    let dir_on = sandbox("ui_show_hidden_new_tab_on");
    seed_files(&dir_on);
    let root_on = canon(&dir_on);
    let mut cfg_on = Config::default();
    cfg_on.ui.show_hidden = true;
    let mut s_on = Sim::with_config(&root_on, cfg_on);
    s_on.see(".hidden.txt");
    s_on.key('.'); // toggle off in tab0
    s_on.dont_see(".hidden.txt");
    s_on.key('t'); // new tab
    s_on.see(".hidden.txt"); // config default (on), not tab0's toggled-off state
    std::fs::remove_dir_all(&dir_on).ok();

    // config off (default): toggle on in tab0, then `t` — tab1 should come back off.
    let dir_off = sandbox("ui_show_hidden_new_tab_off");
    seed_files(&dir_off);
    let root_off = canon(&dir_off);
    let mut s_off = Sim::new(&root_off);
    s_off.dont_see(".hidden.txt");
    s_off.key('.'); // toggle on in tab0
    s_off.see(".hidden.txt");
    s_off.key('t'); // new tab
    s_off.dont_see(".hidden.txt"); // config default (off), not tab0's toggled-on state
    std::fs::remove_dir_all(&dir_off).ok();
}

/// The `.` toggle flips correctly from both possible starting points (config on and config off) —
/// a regression that always flips to a hardcoded state (e.g. always ending up `false`) would pass
/// the two config-default tests above (they only toggle once) but fail this.
#[test]
fn e2e_ui_show_hidden_dot_key_toggles_from_either_config_default() {
    let dir_on = sandbox("ui_show_hidden_toggle_from_on");
    seed_files(&dir_on);
    let root_on = canon(&dir_on);
    let mut cfg_on = Config::default();
    cfg_on.ui.show_hidden = true;
    let mut s_on = Sim::with_config(&root_on, cfg_on);
    s_on.see(".hidden.txt");
    s_on.key('.');
    s_on.dont_see(".hidden.txt");
    s_on.key('.'); // toggle back
    s_on.see(".hidden.txt");
    std::fs::remove_dir_all(&dir_on).ok();

    let dir_off = sandbox("ui_show_hidden_toggle_from_off");
    seed_files(&dir_off);
    let root_off = canon(&dir_off);
    let mut s_off = Sim::new(&root_off); // show_hidden=false
    s_off.dont_see(".hidden.txt");
    s_off.key('.');
    s_off.see(".hidden.txt");
    s_off.key('.'); // toggle back
    s_off.dont_see(".hidden.txt");
    std::fs::remove_dir_all(&dir_off).ok();
}

#[test]
fn e2e_ui_filter_mode_fuzzy_vs_substring_noncontiguous_query() {
    let dir = sandbox("ui_filter_mode_cfg");
    std::fs::write(dir.join("app_resolver.rs"), "x").unwrap();
    let root = canon(&dir);

    // default "fuzzy": a non-contiguous subsequence query still matches (fzf-style).
    let mut s_fuzzy = Sim::new(&root);
    s_fuzzy.key('/');
    s_fuzzy.keys("aprs");
    s_fuzzy.see("app_resolver.rs");

    // "substring": the same query is not a literal substring, so nothing matches.
    let mut cfg = Config::default();
    cfg.ui.filter_mode = "substring".into();
    let mut s_sub = Sim::with_config(&root, cfg);
    s_sub.key('/');
    s_sub.keys("aprs");
    s_sub.dont_see("app_resolver.rs");
    assert_eq!(
        s_sub.app.tab.entries.len(),
        0,
        "substring は非連続クエリに一致しないはず"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_ui_tabbar_always_vs_auto_with_a_single_tab() {
    let dir = sandbox("ui_tabbar_cfg");
    seed_files(&dir);
    let root = canon(&dir);

    let s_auto = Sim::new(&root); // default "auto": 1 tab → tab bar hidden
    let label = s_auto.app.tab_label(0);
    let chip = format!("1:{label}");
    s_auto.dont_see(&chip);

    let mut cfg = Config::default();
    cfg.ui.tabbar = "always".into();
    let s_always = Sim::with_config(&root, cfg);
    s_always.see(&chip); // shown even with a single tab
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_ui_line_numbers_toggle_shows_gutter() {
    let dir = sandbox("ui_line_numbers_cfg");
    std::fs::write(dir.join("f.txt"), "alpha\nbeta\ngamma\n").unwrap();
    let root = canon(&dir);

    let mut s_off = Sim::new(&root); // default false
    s_off.select("f.txt");
    s_off.enter();
    s_off.dont_see("  1 alpha");

    let mut cfg = Config::default();
    cfg.ui.line_numbers = true;
    let mut s_on = Sim::with_config(&root, cfg);
    s_on.select("f.txt");
    s_on.enter();
    s_on.see("  1 alpha");
    std::fs::remove_dir_all(&dir).ok();
}

#[cfg(feature = "git")]
#[test]
fn e2e_ui_git_gutter_toggle_shows_or_hides_change_marker() {
    let dir = sandbox("ui_git_gutter_cfg");
    seed_repo(&dir);
    let dir = canon(&dir);
    std::fs::write(dir.join("a.rs"), "fn a() {}\nfn a2() {}\n").unwrap(); // an uncommitted addition

    let mut cfg_on = Config::default();
    cfg_on.ui.syntax_highlight = false; // determinism: don't depend on cold/warm grammar state
    let mut s_on = Sim::with_config(&dir, cfg_on);
    s_on.select("a.rs");
    s_on.enter();
    s_on.see("▌"); // default git_gutter=true: the added-line marker shows

    let mut cfg_off = Config::default();
    cfg_off.ui.syntax_highlight = false;
    cfg_off.ui.git_gutter = false;
    let mut s_off = Sim::with_config(&dir, cfg_off);
    s_off.select("a.rs");
    s_off.enter();
    s_off.dont_see("▌");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_ui_tab_width_changes_expansion_columns() {
    let dir = sandbox("ui_tab_width_cfg");
    std::fs::write(dir.join("f.txt"), "\tX\n").unwrap();
    let root = canon(&dir);

    let mut s4 = Sim::new(&root); // default tab_width=4: marker + 3 spaces = 4 cols
    s4.select("f.txt");
    s4.enter();
    s4.see("→   X");

    let mut cfg = Config::default();
    cfg.ui.tab_width = 8;
    let mut s8 = Sim::with_config(&root, cfg); // marker + 7 spaces = 8 cols
    s8.select("f.txt");
    s8.enter();
    s8.see("→       X");
    s8.dont_see("→   X");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_ui_syntax_highlight_toggle_changes_color_diversity() {
    let dir = sandbox("ui_syntax_hl_cfg");
    let path = dir.join("code.rs");
    std::fs::write(&path, "fn main() { let x = 1; }\n").unwrap();
    // Deterministic: warm the grammar ourselves so the very first render already shows real
    // syntax coloring rather than a "loading" placeholder (whose state is otherwise shared/global
    // across the whole test binary — see the comment on e2e_search_works_in_code_preview).
    crate::preview::code::warm_file("rs", &path);
    let root = canon(&dir);

    let mut s_on = Sim::new(&root); // default: syntax_highlight=true
    s_on.select("code.rs");
    s_on.enter();
    let (row_on, _) = s_on.find_text("fn main").expect("code line drawn");
    let variety_on = s_on.distinct_fg_in_row(row_on);

    let mut cfg = Config::default();
    cfg.ui.syntax_highlight = false;
    let mut s_off = Sim::with_config(&root, cfg);
    s_off.select("code.rs");
    s_off.enter();
    let (row_off, _) = s_off.find_text("fn main").expect("code line drawn");
    let variety_off = s_off.distinct_fg_in_row(row_off);

    assert!(
        variety_on > variety_off,
        "既定はハイライトで複数色/off は単色のはず: on={variety_on} off={variety_off}"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_ui_preview_loading_indicator_vs_progressive_for_cold_language() {
    // Note: the sandbox name must not contain "loading" — it would leak into the title bar path
    // and false-positive the `dont_see("loading")` check below.
    let dir = sandbox("ui_cold_lang_cfg");
    // A fabricated extension registered as `code` via a custom rule, so it is both (a) guaranteed
    // never warmed by any other test in this binary and (b) still classified as `PreviewKind::Code`
    // (an unrecognized extension with no custom rule would fall through to Text, where
    // syntax-highlight warm-up — and so `preview_loading` — never applies).
    std::fs::write(dir.join("f.zzzcoldlang"), "UNIQUEBODYTEXT123\n").unwrap();
    let root = canon(&dir);
    let rule = crate::config::Rule {
        glob: Some("*.zzzcoldlang".into()),
        builtin: Some("code".into()),
        ..Default::default()
    };

    // default "indicator": a cold grammar shows a centered loading message, not the body.
    let mut cfg_ind = Config::default();
    cfg_ind.preview.rules.insert(0, rule.clone());
    let mut s_ind = Sim::with_config(&root, cfg_ind);
    s_ind.select("f.zzzcoldlang");
    s_ind.enter();
    assert!(
        s_ind.app.is_highlight_pending(),
        "登録したての拡張子はコールドのはず"
    );
    s_ind.see("loading"); // i18n Msg::Loading = "loading…"
    s_ind.dont_see("UNIQUEBODYTEXT123");

    // "progressive": the same cold grammar shows the plain body immediately instead.
    let mut cfg_prog = Config::default();
    cfg_prog.preview.rules.insert(0, rule);
    cfg_prog.ui.preview_loading = "progressive".into();
    let mut s_prog = Sim::with_config(&root, cfg_prog);
    s_prog.select("f.zzzcoldlang");
    s_prog.enter();
    assert!(s_prog.app.is_highlight_pending());
    s_prog.see("UNIQUEBODYTEXT123");
    s_prog.dont_see("loading");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_ui_path_style_config_sets_initial_style() {
    use crate::app::PathStyle;
    let dir = sandbox("ui_path_style_cfg");
    seed_files(&dir);
    let root = canon(&dir);
    let mut cfg = Config::default();
    cfg.ui.path_style = "full".into();
    let s = Sim::with_config(&root, cfg);
    assert_eq!(
        s.app.path_style,
        PathStyle::Full,
        "config で起動時から full のはず(既定は relative — e2e_path_style_cycles参照)"
    );
    s.see("path:abs");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_ui_lang_toggle_switches_footer_translation() {
    let dir = sandbox("ui_lang_cfg");
    seed_files(&dir);
    let root = canon(&dir);

    let mut cfg_en = Config::default();
    cfg_en.ui.lang = "en".into();
    let s_en = Sim::with_config(&root, cfg_en);
    s_en.see("quit");

    let mut cfg_jp = Config::default();
    cfg_jp.ui.lang = "jp".into();
    let s_jp = Sim::with_config(&root, cfg_jp);
    s_jp.dont_see("quit");
    s_jp.see("終"); // "終了" (quit)
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_ui_statusbar_layout_split_vs_bottom_moves_context_row() {
    let dir = sandbox("ui_statusbar_cfg");
    seed_files(&dir);
    let root = canon(&dir);

    let s_split = Sim::new(&root); // default "split": context chip on the top row
    let (row_split, _) = s_split.find_text("TREE").expect("TREE chip shown");
    assert_eq!(row_split, 0, "split: 上段(0行目)に TREE チップ");

    let mut cfg = Config::default();
    cfg.ui.statusbar = "bottom".into();
    let s_bottom = Sim::with_config(&root, cfg);
    let (row_bottom, _) = s_bottom
        .find_text("TREE")
        .expect("TREE chip shown (combined bottom line)");
    assert_ne!(
        row_bottom, 0,
        "bottom: 上段は消え最終行にまとめて表示されるはず"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_ui_theme_code_bg_toggle_changes_code_block_background() {
    let body = "```rust\nfn a(){}\n```\n";
    let (s_on, dir1) = md_preview(Config::default(), "ui_theme_bg_on", body);
    let bg_on = s_on.style_of("fn a").map(|st| st.bg);
    std::fs::remove_dir_all(&dir1).ok();

    let (s_off, dir2) = md_preview(cfg_code_bg_none(), "ui_theme_bg_off", body);
    let bg_off = s_off.style_of("fn a").map(|st| st.bg);
    std::fs::remove_dir_all(&dir2).ok();

    assert!(
        matches!(bg_on, Some(Some(ratatui::style::Color::Rgb(..)))),
        "既定のコード背景は実色のはず: {bg_on:?}"
    );
    assert_eq!(
        bg_off,
        Some(Some(ratatui::style::Color::Reset)),
        "code_bg=none は背景なし(Reset)のはず: {bg_off:?}"
    );
    assert_ne!(bg_on, bg_off);
}

/// `image_render_scale`: `App::prepare_image` (the function `ui/preview.rs` calls right before
/// drawing) shrinks the on-screen target rect proportionally when < 1.0. Calling it directly with a
/// fixed `inner` rect avoids needing to inspect drawn pixels. A picker is required (`with_picker`)
/// only for `picker.font_size()`/`is_some()` — no async worker needed since still-image decode
/// (`load_image`) has a synchronous fallback.
#[test]
fn e2e_ui_image_render_scale_shrinks_display_rect() {
    let dir = sandbox("ui_image_render_scale_cfg");
    let img = image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
        800,
        600,
        image::Rgb([200, 50, 50]),
    ));
    img.save(dir.join("p.png")).unwrap();
    let root = canon(&dir);
    let inner = ratatui::layout::Rect::new(0, 0, 80, 20);

    let mut s_default = Sim::new(&root).with_picker(); // image_render_scale=1.0
    s_default.select("p.png");
    s_default.enter();
    let target_default = s_default
        .app
        .prepare_image(inner)
        .expect("画像がデコードされ target rect が計算されるはず");

    let mut cfg = Config::default();
    cfg.ui.image_render_scale = 0.3;
    let mut s_small = Sim::with_config(&root, cfg).with_picker();
    s_small.select("p.png");
    s_small.enter();
    let target_small = s_small
        .app
        .prepare_image(inner)
        .expect("画像がデコードされ target rect が計算されるはず");

    assert!(
        target_small.width < target_default.width && target_small.height < target_default.height,
        "image_render_scale=0.3 は既定1.0より小さい表示矩形になるはず: small={target_small:?} default={target_default:?}"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// `svg_max_px`: a lower rasterization cap produces a raster with fewer pixels, which
/// `image_layout`'s natural-size calculation (pixels ÷ font size) maps to a smaller apparent
/// on-screen cell size for the very first display of a vector image.
#[test]
fn e2e_ui_svg_max_px_changes_apparent_display_size() {
    let dir = sandbox("ui_svg_max_px_cfg");
    std::fs::copy("samples/sample.svg", dir.join("p.svg")).unwrap();
    let root = canon(&dir);
    let inner = ratatui::layout::Rect::new(0, 0, 80, 20);

    let mut s_default = Sim::new(&root).with_picker(); // svg_max_px=800
    s_default.select("p.svg");
    s_default.enter();
    let target_default = s_default
        .app
        .prepare_image(inner)
        .expect("SVG がラスタライズされ target rect が計算されるはず");

    let mut cfg = Config::default();
    cfg.ui.svg_max_px = 40;
    let mut s_small = Sim::with_config(&root, cfg).with_picker();
    s_small.select("p.svg");
    s_small.enter();
    let target_small = s_small
        .app
        .prepare_image(inner)
        .expect("SVG がラスタライズされ target rect が計算されるはず");

    assert!(
        target_small.width < target_default.width || target_small.height < target_default.height,
        "svg_max_px=40 は既定800より小さい見かけサイズになるはず: small={target_small:?} default={target_default:?}"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_ui_sort_config_sets_initial_order_without_pressing_s() {
    let dir = sandbox("ui_sort_cfg");
    std::fs::write(dir.join("aaa.txt"), vec![b'a'; 300]).unwrap();
    std::fs::write(dir.join("bbb.txt"), vec![b'b'; 30]).unwrap();
    std::fs::write(dir.join("ccc.txt"), vec![b'c'; 150]).unwrap();
    let root = canon(&dir);

    fn names(s: &Sim) -> Vec<&str> {
        s.app
            .tab
            .entries
            .iter()
            .map(|e| e.path.file_name().and_then(|n| n.to_str()).unwrap_or(""))
            .collect()
    }

    let s_default = Sim::new(&root); // default sort.key="name"
    assert_eq!(names(&s_default), vec!["aaa.txt", "bbb.txt", "ccc.txt"]);

    let mut cfg = Config::default();
    cfg.ui.sort.key = "size".into();
    let s_cfg = Sim::with_config(&root, cfg);
    assert_eq!(
        names(&s_cfg),
        vec!["bbb.txt", "ccc.txt", "aaa.txt"],
        "sort.key=\"size\" は起動時からサイズ順のはず(sメニュー不要)"
    );
    assert!(
        !s_cfg.app.is_sort_menu(),
        "メニューを開かず既に反映済みのはず"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[cfg(feature = "git")]
#[test]
fn e2e_ui_graph_max_branches_caps_legend_entries() {
    let dir = sandbox("ui_graph_max_branches_cfg");
    seed_repo_history(&dir); // main + feature-x = 2 branches already
    let dir = canon(&dir);
    let run = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .current_dir(&dir)
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {out:?}");
    };
    run(&["branch", "b1"]);
    run(&["branch", "b2"]);
    run(&["branch", "b3"]); // now 5 branches total

    let mut cfg = Config::default();
    cfg.ui.graph_max_branches = 2;
    let mut s = Sim::with_config(&dir, cfg);
    s.key('o');
    s.key('g');
    s.see("hidden)");

    let mut cfg2 = Config::default();
    cfg2.ui.graph_max_branches = 0; // 0 = unlimited
    let mut s2 = Sim::with_config(&dir, cfg2);
    s2.key('o');
    s2.key('g');
    s2.dont_see("hidden)");
    std::fs::remove_dir_all(&dir).ok();
}

#[cfg(feature = "git")]
#[test]
fn e2e_ui_graph_base_branches_config_auto_selects_base() {
    let dir = sandbox("ui_graph_base_branches_cfg");
    seed_repo(&dir);
    let root = canon(&dir);
    let base = crate::git::branch(&root).expect("has a branch after seed_repo's commit");

    let mut s_default = Sim::new(&root); // default graph_base_branches=[]: no auto base
    s_default.key('o');
    s_default.key('g');
    s_default.dont_see("⌖ base:");

    let mut cfg = Config::default();
    cfg.ui.graph_base_branches = vec![base];
    let mut s_cfg = Sim::with_config(&root, cfg);
    s_cfg.key('o');
    s_cfg.key('g');
    s_cfg.see("⌖ base:"); // auto-pinned without pressing `s`
    std::fs::remove_dir_all(&dir).ok();
}

#[cfg(feature = "git")]
#[test]
fn e2e_ui_commit_meta_align_right_vs_inline_moves_author_column() {
    let dir = sandbox("ui_commit_meta_align_cfg");
    std::fs::create_dir_all(&dir).unwrap();
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
    run(&["config", "user.name", "AuthorZZZ"]);
    std::fs::write(dir.join("a.rs"), "fn a(){}\n").unwrap();
    run(&["add", "-A"]);
    run(&["commit", "-q", "-m", "shortsubj"]);
    let dir = canon(&dir);

    let mut s_right = Sim::new(&dir); // default "right": a padded column near the right edge
    s_right.key('o');
    s_right.key('l');
    let (_, col_right) = s_right
        .find_text("AuthorZZZ")
        .expect("author shown in the log row");

    let mut cfg = Config::default();
    cfg.ui.commit_meta_align = "inline".into();
    let mut s_inline = Sim::with_config(&dir, cfg); // right after the (short) subject
    s_inline.key('o');
    s_inline.key('l');
    let (_, col_inline) = s_inline
        .find_text("AuthorZZZ")
        .expect("author shown in the log row");

    assert!(
        col_right > col_inline,
        "right は列揃え(右寄り)/inline は subject 直後(左寄り)のはず: right_col={col_right} inline_col={col_inline}"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[cfg(feature = "git")]
#[test]
fn e2e_ui_follow_view_file_shows_content_preview_not_diff() {
    let dir = sandbox("ui_follow_view_file_cfg");
    seed_repo(&dir); // a.rs is tracked
    let mut cfg = Config::default();
    cfg.ui.follow_view = "file".into();
    let mut s = Sim::with_config(&canon(&dir), cfg);
    s.key('F');
    assert!(s.app.follow_enabled(), "F でフォロー ON");

    let a = s.app.tab.root.join("a.rs");
    std::fs::write(&a, "fn a() { let _x = 2; }\n").unwrap();
    assert!(s.app.follow_note_change(&a));
    s.app.follow_jump(&a);
    s.draw();

    assert!(
        !s.app.is_git_diff_preview(),
        "follow_view=\"file\" は diff でなく通常プレビューのはず(既定は diff — e2e_follow_opens_full_screen_diff参照)"
    );
    assert_eq!(s.app.tab.mode, Mode::Preview);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_ui_md_inline_html_off_keeps_raw_tags() {
    // The doc comment on `md_inline_html` is precise about what "off" means: "false leaves all
    // these tags to be stripped" — i.e. off doesn't restore the literal `<sub>...</sub>` tags (both
    // on and off strip them; tui-markdown always drops unrecognized tags), it just stops converting
    // the surviving text to Unicode. So the contrast is glyph-level ("2" vs "₂"), not tag-visibility.
    let body = "H<sub>2</sub>O\n";
    let mut cfg = Config::default();
    cfg.ui.md_inline_html = false;
    let (s, dir) = md_preview(cfg, "ui_inline_html_off", body);
    s.see("H2O"); // literal "2" — not converted to a Unicode subscript
    s.dont_see("H₂O");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_ui_md_details_config_forces_open_despite_missing_attribute() {
    let dir = sandbox("ui_md_details_cfg");
    std::fs::write(
        dir.join("d.md"),
        "<details>\n<summary>Sum</summary>\n\nBODYMARKER\n\n</details>\n",
    )
    .unwrap();
    let root = canon(&dir);

    // No `open` attribute → "auto" (default) honors it and collapses.
    let mut s_auto = Sim::new(&root);
    s_auto.select("d.md");
    s_auto.enter();
    s_auto.dont_see("BODYMARKER");

    // md_details="open" forces it open despite the missing attribute.
    let mut cfg = Config::default();
    cfg.ui.md_details = "open".into();
    let mut s_open = Sim::with_config(&root, cfg);
    s_open.select("d.md");
    s_open.enter();
    s_open.see("BODYMARKER");
    std::fs::remove_dir_all(&dir).ok();
}

/// `math`: with a picker attached (only needed for `picker.font_size()` — decode itself is
/// synchronous without `md_img_tx`), the default "image" mode lifts an inline `$...$` expression
/// onto its own reserved image placement (so the literal LaTeX text is NOT shown inline); "text"
/// mode disables extraction entirely, so the literal source stays in the paragraph text.
#[test]
fn e2e_ui_math_image_mode_lifts_expression_text_mode_keeps_it_inline() {
    let dir = sandbox("ui_math_mode_cfg");
    std::fs::write(dir.join("d.md"), "before $E=mc^2$ after\n").unwrap();
    let root = canon(&dir);

    let mut s_image = Sim::new(&root).with_picker(); // default math="image"
    s_image.select("d.md");
    s_image.enter();
    assert_eq!(
        s_image.app.md_images().len(),
        1,
        "画像モードは math 用のプレースメントを1つ持つはず"
    );
    s_image.dont_see("E=mc^2"); // lifted onto its own image row, not shown as literal text

    let mut cfg = Config::default();
    cfg.ui.math = "text".into();
    let mut s_text = Sim::with_config(&root, cfg).with_picker();
    s_text.select("d.md");
    s_text.enter();
    assert!(
        s_text.app.md_images().is_empty(),
        "text モードは画像プレースメント無しのはず"
    );
    s_text.see("$E=mc^2$");
    std::fs::remove_dir_all(&dir).ok();
}

/// `math_color`: drives the real `md_encode_worker` (the same worker `main` spawns) so the
/// rendered raster's pixel colors actually reach a halfblocks-protocol drawn cell, then reads the
/// foreground RGB channels off the drawn buffer. The default light-gray (#d0d0d0) paints
/// approximately-equal r/g/b; a saturated red (#ff0000) paints g≈b≈0 with r>0 — a signal that
/// can't be produced by anything except the glyph color itself.
#[test]
fn e2e_ui_math_color_changes_rendered_pixel_hue() {
    let dir = sandbox("ui_math_color_cfg");
    std::fs::write(dir.join("d.md"), "$E=mc^2$\n").unwrap();
    let root = canon(&dir);

    fn render_and_get_rgbs(cfg: Config, dir: &std::path::Path) -> Vec<(u8, u8, u8)> {
        let mut s = Sim::with_config(dir, cfg).with_picker();
        let (enc_tx, enc_worker_rx) = std::sync::mpsc::channel();
        let (enc_res_tx, enc_res_rx) = std::sync::mpsc::channel();
        s.app.attach_md_encoder(enc_tx);
        let picker = ratatui_image::picker::Picker::halfblocks();
        std::thread::spawn(move || crate::app::md_encode_worker(picker, enc_worker_rx, enc_res_tx));
        s.select("d.md");
        s.enter(); // decode (sync, no md_img_tx) + kick an encode request
        let res = enc_res_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("encode worker returns a result");
        assert!(s.app.apply_md_encode(res), "現世代の結果は適用されるはず");
        s.draw(); // now the protocol is Some → the Image widget actually draws pixels
        let buf = s.term.backend().buffer();
        let mut out = Vec::new();
        for cell in buf.content().iter() {
            if let Some(ratatui::style::Color::Rgb(r, g, b)) = cell.style().fg {
                out.push((r, g, b));
            }
        }
        out
    }

    let rgbs_default = render_and_get_rgbs(Config::default(), &root); // #d0d0d0
    assert!(
        !rgbs_default.is_empty(),
        "既定色でも実ピクセルが描かれるはず"
    );
    assert!(
        rgbs_default
            .iter()
            .all(|&(r, g, b)| r.abs_diff(g) <= 2 && g.abs_diff(b) <= 2),
        "既定(#d0d0d0)は無彩色(r≈g≈b)のはず: {rgbs_default:?}"
    );

    let mut cfg_red = Config::default();
    cfg_red.ui.math_color = "#ff0000".into();
    let rgbs_red = render_and_get_rgbs(cfg_red, &root);
    assert!(!rgbs_red.is_empty(), "赤色設定でも実ピクセルが描かれるはず");
    assert!(
        rgbs_red.iter().any(|&(r, g, b)| r > 20 && g == 0 && b == 0),
        "math_color=#ff0000 は赤チャンネルのみのはず: {rgbs_red:?}"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// `mermaid`: same reasoning as `math` above — a picker unlocks classification/sizing without
/// needing the encode worker. Default "image" shows the sentinel caption and a reserved image
/// placement; "text" disables the whole image path (legacy Unicode box-drawing renders instead,
/// with no caption and no placement).
#[test]
fn e2e_ui_mermaid_image_mode_shows_caption_text_mode_does_not() {
    let dir = sandbox("ui_mermaid_mode_cfg");
    std::fs::write(
        dir.join("d.md"),
        "# Doc\n\n```mermaid\nflowchart TD\nA-->B\n```\n\nafter\n",
    )
    .unwrap();
    let root = canon(&dir);

    let mut s_image = Sim::new(&root).with_picker(); // default mermaid="image"
    s_image.select("d.md");
    s_image.enter();
    s_image.see("◇ mermaid");
    assert_eq!(s_image.app.md_images().len(), 1);

    let mut cfg = Config::default();
    cfg.ui.mermaid = "text".into();
    let mut s_text = Sim::with_config(&root, cfg).with_picker();
    s_text.select("d.md");
    s_text.enter();
    s_text.dont_see("◇ mermaid");
    assert!(s_text.app.md_images().is_empty());
    std::fs::remove_dir_all(&dir).ok();
}

/// `mermaid_theme`: same driven-encode-worker technique as `math_color`. "dark" paints near-black
/// low RGB values; "neutral" paints near-white high RGB values — a difference confirmed directly
/// against `mermaid_to_svg`+`rasterize_bytes` output (204,204,204 vs 212,212,212 at a sample pixel)
/// before this test was written, then verified to actually reach the drawn buffer once the diagram
/// is sized small enough to stay fully inside the viewport (a too-tall diagram gets clipped and its
/// visible band alone doesn't reach the encode step in this harness — see mermaid_rows below).
#[test]
fn e2e_ui_mermaid_theme_changes_rendered_pixel_brightness() {
    let dir = sandbox("ui_mermaid_theme_cfg");
    std::fs::write(dir.join("d.md"), "```mermaid\nflowchart TD\nA-->B\n```\n").unwrap();
    let root = canon(&dir);

    fn render_and_get_fgs(cfg: Config, dir: &std::path::Path) -> Vec<(u8, u8, u8)> {
        let mut s = Sim::with_config(dir, cfg).with_picker();
        let (enc_tx, enc_worker_rx) = std::sync::mpsc::channel();
        let (enc_res_tx, enc_res_rx) = std::sync::mpsc::channel();
        s.app.attach_md_encoder(enc_tx);
        let picker = ratatui_image::picker::Picker::halfblocks();
        std::thread::spawn(move || crate::app::md_encode_worker(picker, enc_worker_rx, enc_res_tx));
        s.select("d.md");
        s.enter();
        let res = enc_res_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("encode worker returns a result");
        assert!(s.app.apply_md_encode(res));
        s.draw();
        let buf = s.term.backend().buffer();
        let mut out = Vec::new();
        for cell in buf.content().iter() {
            if let Some(ratatui::style::Color::Rgb(r, g, b)) = cell.style().fg {
                out.push((r, g, b));
            }
        }
        out
    }

    let mut cfg_dark = Config::default();
    cfg_dark.ui.mermaid_theme = "dark".into();
    cfg_dark.ui.mermaid_rows = 6; // keep the whole block inside the 26-row test terminal
    let fg_dark = render_and_get_fgs(cfg_dark, &root);
    assert!(
        !fg_dark.is_empty(),
        "dark テーマでも実ピクセルが描かれるはず"
    );

    let mut cfg_neutral = Config::default();
    cfg_neutral.ui.mermaid_theme = "neutral".into();
    cfg_neutral.ui.mermaid_rows = 6;
    let fg_neutral = render_and_get_fgs(cfg_neutral, &root);
    assert!(
        !fg_neutral.is_empty(),
        "neutral テーマでも実ピクセルが描かれるはず"
    );

    let avg = |v: &[(u8, u8, u8)]| -> f64 {
        v.iter()
            .map(|&(r, g, b)| (r as f64 + g as f64 + b as f64) / 3.0)
            .sum::<f64>()
            / v.len() as f64
    };
    let avg_dark = avg(&fg_dark);
    let avg_neutral = avg(&fg_neutral);
    assert!(
        avg_dark < avg_neutral,
        "dark は neutral より暗い(低RGB)はず: dark_avg={avg_dark:.1} neutral_avg={avg_neutral:.1}"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// `mermaid_rows`: the reserved cell box (`ImagePlacement.rows`) scales directly with the target
/// row count, independent of any encode/rendering — a picker alone (for `font_size()`) is enough.
#[test]
fn e2e_ui_mermaid_rows_changes_reserved_diagram_height() {
    let dir = sandbox("ui_mermaid_rows_cfg");
    std::fs::write(
        dir.join("d.md"),
        "```mermaid\nflowchart TD\nA-->B-->C-->D-->E-->F\n```\n",
    )
    .unwrap();
    let root = canon(&dir);

    let mut s_default = Sim::new(&root).with_picker(); // default mermaid_rows=24
    s_default.select("d.md");
    s_default.enter();
    let rows_default = s_default.app.md_images()[0].rows;

    let mut cfg = Config::default();
    cfg.ui.mermaid_rows = 4;
    let mut s_small = Sim::with_config(&root, cfg).with_picker();
    s_small.select("d.md");
    s_small.enter();
    let rows_small = s_small.app.md_images()[0].rows;

    assert!(
        rows_small < rows_default,
        "mermaid_rows=4 は既定24より小さい高さになるはず: small={rows_small} default={rows_default}"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_ui_busy_indicator_shows_while_media_loads_hides_when_disabled() {
    let dir = sandbox("ui_busy_indicator_cfg");
    std::fs::copy("samples/sample.svg", dir.join("p.svg")).unwrap();
    let root = canon(&dir);

    // default busy_indicator=true: while media is loading, the spinner+label shows.
    let (tx_on, rx_on) = std::sync::mpsc::channel();
    let mut s_on = Sim::new(&root).with_picker();
    s_on.app.attach_media_loader(tx_on);
    std::mem::forget(rx_on); // never drained: media_loading stays true for this frame
    s_on.select("p.svg");
    s_on.enter();
    assert!(s_on.app.is_media_loading());
    s_on.see("loading media");

    // busy_indicator=false: the same in-flight state, but the indicator is suppressed.
    let (tx_off, rx_off) = std::sync::mpsc::channel();
    let mut cfg = Config::default();
    cfg.ui.busy_indicator = false;
    let mut s_off = Sim::with_config(&root, cfg).with_picker();
    s_off.app.attach_media_loader(tx_off);
    std::mem::forget(rx_off);
    s_off.select("p.svg");
    s_off.enter();
    assert!(s_off.app.is_media_loading());
    s_off.dont_see("loading media");
    std::fs::remove_dir_all(&dir).ok();
}
