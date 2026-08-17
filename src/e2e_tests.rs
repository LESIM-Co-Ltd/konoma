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
use crate::test_support::unique_tmp;
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
    /// Receiver for background git writes, once `with_async_git_ops` opted in (the run loop owns
    /// this channel live). `None` = the synchronous fallback in `start_git_op` is used.
    /// Gated on the `git` feature: without it there is no git write to run in the background.
    #[cfg(feature = "git")]
    gitop_rx: Option<std::sync::mpsc::Receiver<crate::app::GitOpResult>>,
    /// Receiver for background inline-Markdown-image decodes (`with_media`). Drained by `drain_md_images`.
    md_img_rx: Option<std::sync::mpsc::Receiver<crate::app::MdImageResult>>,
    /// Receiver for the background inline-image encode worker (`with_media`). Drained by `drain_md_encodes`.
    md_enc_rx: Option<std::sync::mpsc::Receiver<crate::app::MdEncodeResult>>,
    /// Receiver for full-screen media loads — SVG/GIF/PDF/video/standalone-mermaid/fullscreen-fence
    /// (`with_media`). Drained by `drain_media`.
    media_rx: Option<std::sync::mpsc::Receiver<crate::app::MediaResult>>,
    /// Receiver for background remote (http/https) Markdown image fetches (`with_media`). Drained by `drain_remote`.
    md_remote_rx: Option<std::sync::mpsc::Receiver<crate::app::RemoteFetch>>,
}

impl Sim {
    fn new(root: &std::path::Path) -> Sim {
        Sim::with_config(root, Config::default())
    }

    fn with_config(root: &std::path::Path, cfg: Config) -> Sim {
        Sim::with_config_sized(root, cfg, 90, 26)
    }

    /// Same as `with_config`, but with an explicit terminal size instead of the standard 90x26.
    /// Needed by tests that must clear `mermaid_fit_rows`'s viewport clamp (`preview_viewport - 2`)
    /// to observe a **large** `ui.mermaid_rows` actually growing the diagram — at 90x26 the
    /// viewport (22 rows for a single-fence document, confirmed by direct measurement) already
    /// clamps the *default* 24 down to 20, so any `mermaid_rows` >= 22 would clamp to the exact same
    /// 20 and the "large value changes something" contrast would be unobservable, not merely small.
    fn with_config_sized(root: &std::path::Path, cfg: Config, w: u16, h: u16) -> Sim {
        let app = App::new(root.to_path_buf(), cfg).expect("App::new");
        let term = Terminal::new(TestBackend::new(w, h)).expect("terminal");
        let mut sim = Sim {
            app,
            term,
            quit: false,
            fileop_rx: None,
            #[cfg(feature = "git")]
            gitop_rx: None,
            md_img_rx: None,
            md_enc_rx: None,
            media_rx: None,
            md_remote_rx: None,
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

    /// Opt in to the **real background path** for git writes (stage/unstage/discard/commit/
    /// checkout/branch/worktree): attach a runner channel just like `main` does, so `start_git_op`
    /// spawns a worker instead of running `git` inline. Results are applied by `drain_git_ops`.
    #[cfg(feature = "git")]
    fn with_async_git_ops(mut self) -> Sim {
        let (tx, rx) = std::sync::mpsc::channel();
        self.app.attach_gitop_runner(tx);
        self.gitop_rx = Some(rx);
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

    /// Attach the **full async media pipeline** — the same background workers `main` spawns
    /// (`md_encode_worker` running on a cloned `Picker::halfblocks()`, plus the raw channels behind
    /// `attach_media_loader` / `attach_md_image_loader` / `attach_remote_md_loader`) — so a
    /// keystroke-driven test exercises the real decode(thread) → apply → re-render → encode(thread)
    /// → apply pipeline, instead of `with_picker()`'s synchronous no-loader-tx fallback. This is what
    /// reaches `ensure_md_image` / `ensure_mermaid_fence_render`'s and `ensure_math_render`'s
    /// `if let Some(tx) = ...` thread-spawning branches (previously uncovered — every existing test
    /// that touched inline images/mermaid/math went through the synchronous else-branch instead).
    /// The receivers are kept on `Sim` and drained one result at a time by `drain_md_images` /
    /// `drain_md_encodes` / `drain_media` / `drain_remote` — mirrors `with_async_file_ops` +
    /// `drain_file_ops`, and the run loop's per-tick `while let Ok(r) = rx.try_recv()` loops in `main.rs`.
    fn with_media(self) -> Sim {
        self.with_media_on(ratatui_image::picker::Picker::halfblocks())
    }

    /// `with_media` on a `Picker` forced to **kitty graphics**, so a test can drive the branch a
    /// Ghostty/kitty user actually takes: konoma's own compressed transmit on a fixed image id,
    /// rather than ratatui-image's protocol. Note that the drawn buffer then carries kitty escape
    /// sequences instead of halfblock glyphs — read it with `transmitted_kitty_ids`, not `see`.
    fn with_media_kitty(self) -> Sim {
        let mut picker = ratatui_image::picker::Picker::halfblocks();
        picker.set_protocol_type(ratatui_image::picker::ProtocolType::Kitty);
        self.with_media_on(picker)
    }

    fn with_media_on(mut self, picker: ratatui_image::picker::Picker) -> Sim {
        let (resize_tx, resize_rx) = tokio::sync::mpsc::unbounded_channel();
        std::mem::forget(resize_rx); // the kitty zoom/pan resize path isn't exercised here
        self.app.attach_image_backend(picker.clone(), resize_tx);

        let (media_tx, media_rx) = std::sync::mpsc::channel();
        self.app.attach_media_loader(media_tx);
        self.media_rx = Some(media_rx);

        let (md_img_tx, md_img_rx) = std::sync::mpsc::channel();
        self.app.attach_md_image_loader(md_img_tx);
        self.md_img_rx = Some(md_img_rx);

        let (md_remote_tx, md_remote_rx) = std::sync::mpsc::channel();
        self.app.attach_remote_md_loader(md_remote_tx);
        self.md_remote_rx = Some(md_remote_rx);

        // The inline-image encode worker is a real thread, exactly like `main` spawns it (a cloned
        // Picker, requests in, results out). It exits on its own once `md_enc_tx` is dropped (App
        // teardown at the end of the test).
        let (md_enc_tx, md_enc_worker_rx) = std::sync::mpsc::channel();
        let (md_enc_res_tx, md_enc_res_rx) = std::sync::mpsc::channel();
        self.app.attach_md_encoder(md_enc_tx);
        std::thread::spawn(move || {
            crate::app::md_encode_worker(picker, md_enc_worker_rx, md_enc_res_tx)
        });
        self.md_enc_rx = Some(md_enc_res_rx);

        self
    }

    /// Wait for the next inline-Markdown-image decode result and apply it (the run loop's
    /// `rx.md_img.try_recv()` step), then redraw — which is what feeds a decoded image back into
    /// `ensure_md_image`'s "request an encode" branch on the next frame.
    #[track_caller]
    fn drain_md_images(&mut self) {
        let rx = self
            .md_img_rx
            .as_ref()
            .expect("with_media() を呼んでいない");
        let res = rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("decode worker が結果を返す");
        assert!(self.app.apply_md_image(res), "現世代の結果は適用される");
        self.draw();
    }

    /// Wait for the next inline-image encode result and apply it (the run loop's
    /// `rx.md_enc.try_recv()` step), then redraw.
    #[track_caller]
    fn drain_md_encodes(&mut self) {
        let rx = self
            .md_enc_rx
            .as_ref()
            .expect("with_media() を呼んでいない");
        let res = rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("encode worker が結果を返す");
        assert!(self.app.apply_md_encode(res), "現世代の結果は適用される");
        self.draw();
    }

    /// Wait for the next full-screen media-load result (SVG/GIF/PDF/video/standalone-mermaid/
    /// fullscreen-fence) and apply it (the run loop's `rx.media.try_recv()` step), then redraw.
    #[track_caller]
    fn drain_media(&mut self) {
        let rx = self.media_rx.as_ref().expect("with_media() を呼んでいない");
        let res = rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("media loader が結果を返す");
        assert!(self.app.apply_media(res), "現世代の結果は適用される");
        self.draw();
    }

    /// Wait for the next remote (http/https) Markdown image fetch's completion and apply it (the
    /// run loop's `rx.md_remote.try_recv()` step), then redraw.
    #[track_caller]
    fn drain_remote(&mut self) {
        let rx = self
            .md_remote_rx
            .as_ref()
            .expect("with_media() を呼んでいない");
        let res = rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("remote fetch worker が結果を返す");
        assert!(self.app.apply_remote_fetch(res), "常に再描画を要求する");
        self.draw();
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

    /// Wait for the in-flight git write's result and apply it (the run loop's
    /// `while let Ok(result) = rx.gitop.try_recv()` step), then redraw.
    #[cfg(feature = "git")]
    #[track_caller]
    fn drain_git_ops(&mut self) {
        let rx = self
            .gitop_rx
            .as_ref()
            .expect("with_async_git_ops() を呼んでいない");
        let res = rx
            .recv_timeout(std::time::Duration::from_secs(30))
            .expect("ワーカーが結果を返す");
        assert!(self.app.apply_git_op(res), "現世代の結果は適用される");
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

/// Fresh, process-unique sandbox dir under the OS temp dir (recreated per call). The name is
/// suffixed with the PID + a process-global counter — the same pattern already used by
/// `app/tests.rs`'s `unique_tmp` and `git.rs`'s `unique_tmp` ("a fixed path would collide across
/// parallel test runs"). A bare `name`-keyed path let two *concurrently running processes* that
/// both execute this same test race on `remove_dir_all()` + `create_dir_all()` + file writes in
/// the identical shared directory (e.g. an overlapping/orphaned `cargo test` invocation, a rerun
/// while a previous run was still in flight, or parallel CI jobs sharing a temp mount): one
/// process's sandbox reset can delete a fixture file out from under another process's still-running
/// test. Root-caused via `e2e_ui_inline_gif_animates_through_real_decode_worker`'s intermittent
/// "encode worker が結果を返す: Timeout" — two processes both looping that single test reproduced it
/// reliably (~4% of runs): `resolve_md_image_path`'s `Path::is_file()` transiently observed
/// `anim.gif` as missing mid-recreation-by-the-other-process, so `ensure_md_image` silently
/// returned without ever queuing the second encode request, and `drain_md_encodes()` then waited
/// on a message nobody had sent.
fn sandbox(name: &str) -> std::path::PathBuf {
    let dir = crate::test_support::unique_tmp(&format!("konoma_e2e_{name}"));
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

/// Resolves a fixture bundled under the repo's `samples/` directory, anchored at
/// `CARGO_MANIFEST_DIR` (baked in at compile time) rather than a bare relative path. A bare
/// `"samples/…"` resolves against the test binary's **cwd**, which is only the crate root by
/// convention (`cargo test` run from elsewhere, e.g. `cd /tmp && cargo test --manifest-path …`, is
/// a real, supported invocation) — the tests that used to build one directly (`std::fs::copy`
/// straight off a bare relative path, with no existence check at all) would then hard-fail with an
/// IO error from a cwd that has nothing to do with whether the fixture is actually missing.
/// Tolerant of the one case where the fixture is legitimately absent — `samples/` is excluded from
/// the published crate (`Cargo.toml`'s `exclude`) — by returning `None` so the caller can skip,
/// same as the rest of this codebase's samples-gated tests, but saying so loudly (`eprintln!`,
/// visible with `--nocapture` or in the captured-output dump whenever the process later exits
/// non-zero for any reason) instead of silently passing zero assertions.
fn sample_path_or_skip(name: &str) -> Option<std::path::PathBuf> {
    let p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
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
fn e2e_filter_over_budget_stays_usable_and_fills_in_from_the_worker() {
    // The same journey as `e2e_tree_filter_narrows_and_esc_clears`, but through the **hand-off**
    // path: the scan's budget is forced to zero, so `/` returns with only part of the tree
    // collected and the rest arrives from a worker while the user is already typing.
    //
    // Driven entirely by real key input through `handle_key`, which is what makes it worth having
    // on top of the unit tests: it proves the budgeted start, the worker result and the redraw
    // compose end to end, not just that each piece behaves in isolation.
    crate::app::set_filter_scan_budget(Some(Some(std::time::Duration::ZERO)));
    let dir = sandbox("filter_budget");
    seed_files(&dir);
    // Enough directories that a one-directory-at-a-time walk really does get interrupted.
    for i in 0..6 {
        let sub = dir.join(format!("pkg{i}"));
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join(format!("deep{i}.csv")), "a,b\n").unwrap();
    }
    let root = canon(&dir);
    let mut s = Sim::new(&root);
    let (tx, rx) = std::sync::mpsc::channel();
    s.app.attach_filter_pool_loader(tx);

    s.key('/');
    s.keys("deep");
    // The key press returned and the UI is live even though the tree is not fully collected yet.
    s.see("/deep");

    // Drain the worker the way the run loop does, then redraw.
    let res = rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
    assert!(s.app.apply_filter_pool(res), "ワーカーの結果が適用される");
    assert!(
        rx.try_recv().is_err(),
        "受け渡しは1回で完結する(残りは期限なしで完走するので追加の往復は起きない)"
    );
    s.draw();

    for i in 0..6 {
        s.see(&format!("deep{i}.csv"));
    }
    s.dont_see("readme.md");
    s.esc();
    s.see("readme.md");
    crate::app::set_filter_scan_budget(None);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_filter_cursor_keeps_its_file_when_the_worker_result_reorders_the_list() {
    // The window this closes, driven by real keys: `/` on a big tree hands the tail to a worker and
    // the user is *expected* to be picking a target out of the partial list while it finishes.
    // `filter_mode = "fuzzy"` (the default) ranks by score, so the arrival renumbers the list — and
    // a cursor that was only range-clamped then pointed at a different file, silently retargeting
    // the very next `Enter` / `y` / `Space→d`. Every step below goes through `handle_key`.
    crate::app::set_filter_scan_budget(Some(Some(std::time::Duration::ZERO)));
    let dir = sandbox("filter_reorder");
    for i in 0..3 {
        std::fs::write(dir.join(format!("x_a_b_c_{i}.txt")), b"weak\n").unwrap();
    }
    // In a subdirectory, so the zero budget leaves it for the worker: a far better match for "abc"
    // than anything the synchronous half collected, which is what makes the arrival reorder.
    std::fs::create_dir_all(dir.join("pkg")).unwrap();
    std::fs::write(dir.join("pkg/abc.txt"), b"strong\n").unwrap();
    let root = canon(&dir);
    let mut s = Sim::new(&root);
    let (tx, rx) = std::sync::mpsc::channel();
    s.app.attach_filter_pool_loader(tx);

    s.key('/');
    s.keys("abc");
    s.enter(); // commit the query — from here the results are navigated normally
    s.key('j'); // move off the first row, so "reset to 0" would fail too
    let before = s.app.tab.selected;
    assert_eq!(before, 1, "土台: カーソルは 2 行目");
    let target = s.app.tab.entries[before].path.clone();
    let target_name = target.file_name().unwrap().to_string_lossy().into_owned();
    assert!(target_name.starts_with("x_a_b_c_"), "土台: {target_name}");

    // The run loop applying the worker's result, then redrawing.
    let res = rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
    assert!(s.app.apply_filter_pool(res), "ワーカーの結果が適用される");
    s.draw();

    s.see("abc.txt");
    let now = s
        .app
        .tab
        .entries
        .iter()
        .position(|e| e.path == target)
        .expect("対象は今も一致する");
    assert_ne!(
        now, before,
        "土台: 到着で順位が入れ替わっていないとこのテストは何も検出しない"
    );
    assert_eq!(
        s.app.tab.entries[s.app.tab.selected].path, target,
        "カーソルは同じファイルを指し続ける"
    );

    // 1) Enter previews the file the user was looking at, not whatever took its row.
    s.enter();
    assert_eq!(s.app.tab.mode, Mode::Preview);
    assert_eq!(
        s.app.tab.preview_path.as_deref(),
        Some(target.as_path()),
        "Enter が開くのはカーソル下のファイル"
    );
    s.see("weak");
    s.key('q');
    assert_eq!(
        s.app.tab.entries[s.app.tab.selected].path, target,
        "戻ってもカーソルは同じファイル"
    );

    // 2) `y` → `f` copies that file's path. Assert the *value* via the clipboard-independent
    // getter (copy_string_for = what copy_path would write) — headless CI has no clipboard, so
    // reading `flash` (which reports the round-trip's success/failure) is flaky there. The key
    // wiring itself (leader opens, then is consumed) is still exercised via real keys below.
    s.key('y');
    assert_eq!(
        s.app.pending_leader,
        Some(crate::keymap::LeaderId::Copy),
        "y でコピーリーダーが開く"
    );
    let copied = s
        .app
        .copy_string_for(crate::app::CopyKind::Full)
        .expect("コピー対象が存在する");
    s.key('f');
    assert_eq!(s.app.pending_leader, None, "y f でリーダーが消費される");
    assert_eq!(
        copied,
        target.display().to_string(),
        "y f がコピーするのはカーソル下のファイル"
    );
    assert!(
        !copied.contains("abc.txt"),
        "先頭を奪った別ファイルではない: {copied}"
    );

    // 3) `Space` → `d` → `!` deletes that file and no other. The decisive one: an off-by-one here
    //    is unrecoverable.
    s.key(' ');
    s.key('d');
    s.key('!');
    assert!(!target.exists(), "消えたのはカーソル下のファイル");
    assert!(
        dir.join("pkg/abc.txt").exists(),
        "順位を奪った別ファイルは無傷"
    );

    crate::app::set_filter_scan_budget(None);
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

// A file with (almost) no newlines — minified JS, a one-line JSON log — must behave like any other
// text preview: the draw returns, scrolling works, and `q` goes back. `read_lines` runs inside
// `terminal.draw`, so the per-line cap is what keeps that true; the allocation bound below is what
// gives this test teeth (uncapped, opening a 4 MiB one-line file allocated ~21 MB in the frame).
#[test]
fn e2e_preview_one_huge_line_file_stays_responsive() {
    let dir = sandbox("preview_huge_line");
    seed_files(&dir);
    let huge = "x".repeat(4 * 1024 * 1024);
    std::fs::write(
        dir.join("min.txt"),
        format!("{huge}\nsecond line\nlast line\n"),
    )
    .unwrap();
    let mut s = Sim::new(&canon(&dir));
    // Warm up on an ordinary file first, so one-time process-global init isn't attributed below.
    s.select("notes.txt");
    s.enter();
    s.key('q');

    s.select("min.txt");
    let open_alloc = crate::mem_tests::allocated_by(|| s.enter());
    assert_eq!(s.app.tab.mode, Mode::Preview, "巨大1行でもプレビューに入る");
    s.see("PREVIEW");
    s.see("xxxxxxxx");
    assert!(
        open_alloc < 2 * 1024 * 1024,
        "4MiB の1行ファイルを開く1フレームが {open_alloc} バイト確保した(行を丸ごと実体化?)"
    );
    // Scrolling (the window re-reads on every step, so an uncapped read would be paid again here).
    let scroll_alloc = crate::mem_tests::allocated_by(|| {
        s.key('j');
        s.key('k');
    });
    assert!(
        scroll_alloc < 2 * 1024 * 1024,
        "巨大1行のスクロール2回が {scroll_alloc} バイト確保した"
    );
    s.key('q');
    assert_eq!(s.app.tab.mode, Mode::Tree, "q でツリーへ戻る");

    // Again with `wrap = false` (the horizontal-scroll setting): now one logical line is one row,
    // so the lines after the huge one are reachable and `l` actually moves along it.
    let mut cfg = Config::default();
    cfg.ui.wrap = false;
    let mut s = Sim::with_config(&canon(&dir), cfg);
    s.select("min.txt");
    s.enter();
    s.see("xxxxxxxx");
    s.key('G');
    s.see("last line");
    s.key('g');
    s.see("xxxxxxxx");
    // `l` walks the 2D caret along the line and the unwrapped view follows it horizontally; `$`
    // goes to its last character. Read the column through the visual selection (the public view of
    // the caret): on a 4 MiB line it must land inside the readable prefix, not at column ~4,194,303.
    s.key('v'); // charwise visual, anchored at column 0
    s.keys("lll");
    s.press(KeyCode::Char('$'), KeyModifiers::NONE);
    let end_col = match s.app.preview_selection() {
        crate::app::PreviewSelection::Char { start, end } => {
            assert_eq!(start.1, 0, "アンカーは行頭");
            end.1
        }
        other => panic!("charwise 選択のはず: {other:?}"),
    };
    assert!(end_col > 2, "l/$ でキャレットが行に沿って進む: {end_col}");
    assert!(
        end_col < 100_000,
        "$ のキャレット列が行長に比例している(上限が効いていない?): {end_col}"
    );
    s.esc(); // leave visual
    s.see("xxxxxxxx");
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
    // Enter opens the diff; q returns to the hub. `s.see("diff")` alone has no detection power
    // here: the hub's own footer legend (`StGitHubKeys`) always contains the literal substring
    // "diff" (the "Enter:diff" hint), so it would still pass even if `Enter` were a complete
    // no-op that left the hub showing. Assert the actual state transition via `is_git_diff_preview`
    // instead — the same accessor used elsewhere in this file for the same purpose.
    s.enter();
    assert!(
        s.app.is_git_diff_preview(),
        "Enter で全画面 diff プレビューに入る"
    );
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
    let linked = unique_tmp("konoma_e2e_git_worktrees_view_linked");
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
    let linked = unique_tmp("konoma_e2e_git_worktrees_new_tab_linked");
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
    // The default worktree_dir ("../") places the new worktree as a **sibling of the sandbox
    // dir** (`worktree_create_target`: `main.join(worktree_dir).join(branch_name)`) — and
    // `sandbox()` puts `dir` directly under the shared OS temp dir, so that sibling also lands
    // directly under the shared temp dir with the *branch name* as its only unique component.
    // A literal branch name here would therefore collide with a concurrently-running instance of
    // this very test even though `dir` itself is `unique_tmp`-safe — so the branch name itself
    // must be the unique part; `expected` is then derived from the real placement formula
    // (`root`'s parent + that branch name), matching `worktree_create_target`.
    let branch_name = unique_tmp("e2e-wt-branch")
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let expected = root.parent().unwrap().join(&branch_name);
    let _ = std::fs::remove_dir_all(&expected);

    let mut s = Sim::new(&root);
    s.key('o');
    s.key('w');
    assert!(s.app.is_git_worktrees());
    s.key('n');
    assert!(s.app.is_dialog(), "n で入力面に入る");
    s.keys(&branch_name);
    s.enter();

    assert!(!s.app.is_git_worktrees(), "作成後は一覧を閉じる");
    let new_root = s.app.tab.root.clone();
    assert_ne!(new_root, root, "root が新しいワークツリーへ切替わる");
    assert_eq!(s.app.tab.open_dir, new_root, "open_dir も切り替わる");
    assert_eq!(new_root, expected.canonicalize().unwrap());
    assert!(matches!(s.app.tab.mode, Mode::Tree), "切替後は Tree 表示");
    assert!(
        crate::git::branch_tip(&root, &branch_name).is_some(),
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
    let bare = unique_tmp("konoma_e2e_git_worktrees_refuse_bare.git");
    let wt = unique_tmp("konoma_e2e_git_worktrees_refuse_wt");
    let gone = unique_tmp("konoma_e2e_git_worktrees_refuse_gone");
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

    let linked = unique_tmp("konoma_e2e_git_worktree_show_changes_linked");
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
    let bare = unique_tmp("konoma_e2e_git_worktree_show_changes_refuse_bare.git");
    let wt = unique_tmp("konoma_e2e_git_worktree_show_changes_refuse_wt");
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
    // (The frame's top/bottom border is excluded since it starts with ┌/└.) The *right* border is
    // either the frame's `│` or the scroll indicator's `█` — the indicator is drawn on top of that
    // column — so both close the row here; what this asserts is the rule's own width, not the frame.
    let has_rule = s.screen().lines().any(|l| {
        l.starts_with('│')
            && (l.ends_with('│') || l.ends_with('█'))
            && l.chars().filter(|&c| c == '─').count() >= 60
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

/// Documentation *of* a syntax must survive the preview. Both preprocessing passes run in the app
/// layer (`md_render.rs`), not inside `render_markdown_tasks`, so the unit tests over the passes
/// alone never touch this path — this scenario drives the real one, from a key press to the drawn
/// screen, under the user's `code_bg = "none"` theme (the setting that has hidden code-detection
/// bugs before).
///
/// Before the shared code-span primitive: `` `[^1]` `` became `` `¹` `` and `` `<kbd>Ctrl</kbd>` ``
/// became ``` ``Ctrl`` ``` — a *doubled* backtick pair that broke the span outright — so a document
/// explaining either syntax could not show it.
#[test]
fn e2e_markdown_code_span_examples_are_not_rewritten() {
    let body = concat!(
        "Write `<kbd>Ctrl</kbd>` to get a keycap, like <kbd>Ctrl</kbd>.\n\n",
        "Write `[^1]` for a reference, like this one.[^1]\n\n",
        "A line break tag is `<br>`.\n\n",
        "[^1]: The note.\n",
    );
    let (s, dir) = md_preview(cfg_code_bg_none(), "code_span_examples", body);
    // The examples survive verbatim…
    s.see("<kbd>Ctrl</kbd>");
    s.see("[^1]");
    s.see("<br>");
    // …and the doubled-backtick corruption never appears.
    s.dont_see("``");
    // …while the real markup on the same lines is still converted.
    s.see("¹"); // the reference outside the span became a superscript
    s.see("The note."); // and its definition moved into the footnotes section
    s.see("Ctrl"); // the <kbd> outside the span became a keycap
    std::fs::remove_dir_all(&dir).ok();
}

/// The same invariant as the test above, but for CommonMark's *other* kind of code block — a
/// **fenced** block and an **indented** (4-column) block, in the same document, one right after
/// the other — under the real app pipeline (key press → drawn screen), with the user's
/// `code_bg = "none"` theme (2026-08: `literal_code_mask` folds `code_block_mask` into `fence_mask`
/// so both kinds are protected the same way; this drives that union through `md_render.rs`, not
/// just the string-rewriting passes in isolation).
#[test]
fn e2e_markdown_indented_and_fenced_code_blocks_protect_the_same_notations() {
    let body = concat!(
        "Fenced example:\n\n",
        "```\n[^1] <kbd>K</kbd>\n```\n\n",
        "Indented example:\n\n",
        "    [^2] <kbd>J</kbd>\n\n",
        "Outside: real[^1] and <kbd>Ctrl</kbd>, and another[^2].\n\n",
        "[^1]: The note.\n[^2]: The other note.\n",
    );
    let (s, dir) = md_preview(cfg_code_bg_none(), "fence_and_indent_protect", body);
    // Both blocks' examples survive verbatim — neither the reference nor the tag was converted.
    s.see("[^1] <kbd>K</kbd>"); // fenced block
    s.see("[^2] <kbd>J</kbd>"); // indented block
                                // …while the identical notations outside either block still convert as usual.
    s.dont_see("<kbd>Ctrl</kbd>"); // converted to a keycap, tag gone
    s.see("¹"); // real[^1] outside → superscript 1
    s.see("²"); // another[^2] outside → superscript 2
    s.see("The note.");
    s.see("The other note.");
    std::fs::remove_dir_all(&dir).ok();
}

/// `<br>` inside either kind of code block used to inject a real newline *into* the block, splitting
/// it into two — the most visible way this class of bug breaks a document (a code block's header
/// draws around only the first half, and the second half loses its gutter/fence entirely). This
/// checks the on-screen row layout directly: the block's content stays on one row; the identical
/// tag outside a block still becomes a real hard line break, landing "before"/"after" on two rows.
#[test]
fn e2e_markdown_br_inside_a_code_block_does_not_split_it() {
    let body = concat!(
        "Fenced example:\n\n",
        "```\nleft<br>right\n```\n\n",
        "Indented example:\n\n",
        "    leftind<br>rightind\n\n",
        "Outside: before<br>after.\n",
    );
    let (s, dir) = md_preview(cfg_code_bg_none(), "br_code_block_integrity", body);
    // Inside either block, the tag and both halves of the line stay together — one screen row.
    s.see("left<br>right");
    s.see("leftind<br>rightind");
    // Outside, the same tag becomes a real break: "before" and "after" land on different rows.
    let (before_row, _) = s.find_text("before").expect("before is drawn");
    let (after_row, _) = s.find_text("after.").expect("after is drawn");
    assert!(
        after_row > before_row,
        "<br> outside a code block still injects a real line break: before_row={before_row}, \
         after_row={after_row}\n{}",
        s.screen()
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// Root cause (2026-08): rewriting `<br>` unconditionally into `"  \n"` injects a line that carries
/// none of the prefix its enclosing block requires. `split_alerts` captures a callout body by
/// consuming *consecutive* `>`-prefixed lines, so the box ended at the injected line, everything
/// after it escaped, and a fence written inside the alert leaked onto the screen as literal
/// `` > ```rust `` text instead of being drawn as a code block.
///
/// The unit tests for this measure the renderer's `Line`s. This drives the same document through the
/// app — key press → `md_render.rs` preprocessing → drawn screen — and asserts on the screen rows,
/// under the user's real `code_bg = "none"` theme. Both spellings are present because they take
/// different branches: a mid-line `<br>` injects a prefixed continuation, while a trailing one must
/// emit the `"  "` marker and **no** newline (a newline there would make an empty quote line, which
/// splits the callout's paragraph instead of breaking a line).
#[test]
fn e2e_markdown_br_in_an_alert_keeps_the_callout_and_its_fence_whole() {
    let body = concat!(
        "> [!NOTE]\n",
        "> line one<br>line two\n",
        "> tail line<br>\n",
        "> after the break\n",
        ">\n",
        "> ```rust\n",
        "> fn inside_the_note() {}\n",
        "> ```\n",
        "\nAfter the note.\n",
    );
    let (mut s, dir) = md_preview(cfg_code_bg_none(), "br_alert_callout", body);

    // The row holding each piece of the body also carries the callout's own bar — i.e. the text is
    // still *inside* the box, not text that escaped it.
    for needle in [
        "line one",
        "line two",
        "tail line",
        "after the break",
        "fn inside_the_note",
    ] {
        let (row, _) = s
            .find_text(needle)
            .unwrap_or_else(|| panic!("「{needle}」が画面にない:\n{}", s.screen()));
        let screen = s.screen();
        let text = screen.lines().nth(row as usize).unwrap_or_default();
        assert!(
            text.contains('▌'),
            "「{needle}」の行が callout の外に逃げている: {text:?}\n{screen}"
        );
    }
    // The fence inside the alert is drawn as a code block (gutter present), and its raw markers
    // never reach the screen — leaked markup is exactly what the bug produced.
    s.see("▎");
    s.dont_see("```");
    s.dont_see("> fn inside_the_note");
    // …and it is a real, copyable code block: the count guard passed, so `Tab` reaches it and the
    // copy resolves to the fence's own source with the `>` markers stripped, as drawn.
    s.tab();
    assert!(
        s.app.md_focused_code(),
        "alert 内の fence に Tab でフォーカスできない:\n{}",
        s.screen()
    );
    assert_eq!(
        s.app.focused_code_text().as_deref(),
        Some("fn inside_the_note() {}"),
        "alert 内 fence のコピー内容"
    );
    // A line break, not a paragraph break. The trailing `<br>` must emit only the `"  "` marker:
    // adding a newline as well would inject an empty `> ` line, which reads as a blank quote line
    // and splits the callout's paragraph — a stray `▌` row on screen. The document has exactly one
    // genuinely empty quote line of its own (the `>` between the prose and the fence), so the count
    // is the test, not the presence.
    let screen = s.screen();
    // `█` joins the trim set alongside the frame's `│`: the scroll indicator is drawn on top of the
    // right border column, so a row that is "nothing but the callout bar" can end in either glyph.
    let blank_callout_rows = screen
        .lines()
        .filter(|r| r.trim_matches(|c| c == '│' || c == '█' || c == ' ') == "▌")
        .count();
    assert_eq!(
        blank_callout_rows, 1,
        "callout 本文の空行数が想定外(行末 <br> が段落区切りになっている):\n{screen}"
    );
    // The document continues normally below the callout.
    s.see("After the note.");
    std::fs::remove_dir_all(&dir).ok();
}

/// `$…$` inside an indented code block must not be lifted out onto its own image row — the same
/// "is this line literal code" question `process_footnotes`/`process_inline_html` ask, but for
/// `split_math`, which has no string output to eyeball on screen the way the other two do. A picker
/// is required: without one, `math` never enters image mode at all (`md_images()` would stay empty
/// regardless of whether the indented block was protected), so this would pass vacuously.
#[test]
fn e2e_markdown_math_inside_an_indented_code_block_is_not_lifted_as_an_image() {
    let body = concat!(
        "Indented code with a dollar sign, not math:\n\n",
        "    literal $x$ stays text\n\n",
        "Outside this is real math: $y$ done.\n",
    );
    let dir = sandbox("math_indented_code");
    std::fs::write(dir.join("d.md"), body).unwrap();
    let root = canon(&dir);
    let mut s = Sim::with_config(&root, cfg_code_bg_none()).with_picker();
    s.select("d.md");
    s.enter();
    assert_eq!(
        s.app.md_images().len(),
        1,
        "the only image placement must be the outside $y$ — the indented block's $x$ must not be \
         lifted out as a second one"
    );
    s.see("literal $x$ stays text"); // the indented block's dollar sign stays literal, on screen
    std::fs::remove_dir_all(&dir).ok();
}

/// The one case in `literal_code_mask`'s own doc comment where `fence_mask`, not `code_block_mask`,
/// is the one doing the protecting: a fence with no blank line between it and a preceding
/// `<summary>…</summary>` line, inside a `<details>` block. `md_details = "open"` forces the block
/// visible (the default "auto" would collapse it, since there is no `open` attribute here — that
/// collapse behavior is covered elsewhere; this test is only about what's inside once it's open).
#[test]
fn e2e_markdown_details_summary_then_a_fence_with_no_blank_line_stays_literal() {
    let mut cfg = cfg_code_bg_none();
    cfg.ui.md_details = "open".into();
    let body = concat!(
        "<details>\n",
        "<summary>s</summary>\n",
        "```rust\n[^1] <kbd>K</kbd>\n```\n",
        "</details>\n\n",
        "Outside: real[^1] and <kbd>Ctrl</kbd>.\n\n",
        "[^1]: The note.\n",
    );
    let (s, dir) = md_preview(cfg, "details_fence_no_blank_e2e", body);
    s.see("[^1] <kbd>K</kbd>"); // the fence right after <summary>, no blank line, stays literal
    s.dont_see("<kbd>Ctrl</kbd>"); // the identical tag outside the details block still converts
    s.see("¹");
    s.see("The note.");
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

/// Shared by the on/off front-matter pair below so the two configs render **the same source** —
/// otherwise a needle that merely differs between two unrelated documents proves nothing about the
/// config flag itself.
const FRONT_MATTER_BODY: &str = concat!(
    "---\n",
    "title: My Doc\n",
    "author: Me\n",
    "---\n",
    "[to section](#real-heading)\n\n",
    "# Real Heading\n\n",
    "body text\n",
);

#[test]
fn e2e_markdown_front_matter_renders_as_metadata() {
    // Leading `---`…`---` is shown as a metadata block; the body (and its heading anchors) render
    // normally after it. Anchor jump on a heading defined below the front matter still resolves.
    let (mut s, dir) = md_preview(Config::default(), "frontmatter", FRONT_MATTER_BODY);
    s.see("title"); // metadata key
    s.see("My Doc"); // metadata value
    s.see("author");
    s.see("Real Heading"); // body heading still renders
                           // The needle above ("title" as plain text) is not enough by itself: with `md_frontmatter =
                           // false` the exact same source still shows the literal word "title" on screen too (tui-markdown
                           // renders the un-stripped `---`...`---` block itself — see the `off` test below), so the two
                           // configs are indistinguishable by text alone. What differs is *how* it's styled: konoma's own
                           // `render_front_matter` colors the key Cyan+DIM, closing the block with a rule. Dumped and
                           // confirmed by hand (`cargo test -- --nocapture`) before picking this needle.
    s.see_styled(
        "title",
        |st| {
            st.fg == Some(ratatui::style::Color::Cyan)
                && st.add_modifier.contains(ratatui::style::Modifier::DIM)
        },
        "konoma 自身のフロントマター key 色(Cyan+DIM) — off でも「title」というテキスト自体は\
         画面に出る(tui-markdown 自身が生の---ブロックを描く)ので、テキストだけでは on/off を\
         区別できない",
    );
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
    // Same source as the `on` test above — only the config flag differs, so any on/off difference
    // in what follows is attributable to `md_frontmatter` and nothing else.
    let mut cfg = Config::default();
    cfg.ui.md_frontmatter = false;
    let (s, dir) = md_preview(cfg, "frontmatter_off", FRONT_MATTER_BODY);
    // Recognition off: the title line and body both still render (no crash, old behavior). This much
    // is true in *both* configs (see the comment on the `on` test), so on its own it does not prove
    // recognition is off.
    s.see("title");
    s.see("body text");
    // What actually proves it's off: konoma's own pre-pass never ran, so the raw `---`...`---` block
    // reaches tui-markdown unstripped. tui-markdown's parser unconditionally recognizes YAML-style
    // metadata blocks (`ParseOptions::ENABLE_YAML_STYLE_METADATA_BLOCKS`, always on — see
    // `tui_markdown_parse_options` in preview/markdown.rs) and renders the *whole* block with its own
    // `metadata_block()` style — plain LightYellow, no separate key/value coloring. Confirmed by hand
    // (`cargo test -- --nocapture`, dumping `style_of("title")`) that this is exactly the opposite of
    // the `on` test's Cyan+DIM key style, so the two needles are mutually exclusive.
    s.see_styled(
        "title",
        |st| st.fg == Some(ratatui::style::Color::LightYellow),
        "tui-markdown 自身のネイティブ YAML front-matter 色(LightYellow) — konoma 側の\
         Cyan+DIM key 色にはならない(pre-pass が走っていない証拠)",
    );
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

/// Root cause (the document-level code mask, 2026-08 — reported by a user): an indented code block
/// whose **first line looked like an HTML tag** was not drawn as a code block at all.
/// `split_html_blocks` asked `fence_mask`, which has no notion of indentation, so the block was
/// rescued as an HTML block instead: its tags were stripped, its gutter was gone, and
/// `"para\n\n    <code>\n"` rendered as *nothing*. Because it never reached the renderer as code it
/// was also not Tab-focusable and not copyable — so the whole reason a reader indents a snippet (to
/// show markup literally, then copy it) failed silently.
///
/// The unit tests for this measure the renderer directly. This drives the shape a user actually hits
/// — key press → `md_render.rs` preprocessing → drawn screen → `Tab` → `y` `c` — under **both**
/// themes, because `code_bg = "none"` (the reporting user's real setting) removes the background
/// that some of this module's code detection has leaned on before, and a bg-only answer must not be
/// able to regress unnoticed again.
#[test]
fn e2e_md_indented_code_block_whose_content_is_a_tag_is_focusable_and_copyable() {
    for (label, cfg) in [
        ("default", Config::default()),
        ("code_bg=none", cfg_code_bg_none()),
    ] {
        let body = concat!(
            "Intro paragraph.\n\n",
            "    <code>\n",
            "    still code\n\n",
            "Tail with a real <kbd>Ctrl</kbd> outside.\n",
        );
        let (mut s, dir) = md_preview(cfg, "indented_tag_code", body);
        // Drawn as a code block: the content is on screen verbatim, behind the `▎` gutter. Under
        // the bug the tag was stripped and this row was empty.
        //
        // Both content lines land on the *same* row (`▎ <code>still code`) — tui-markdown runs an
        // indented block's adjacent lines together, unlike a fenced one. That is upstream behavior
        // rather than anything this fix does, and it is pinned directly in
        // `markdown.rs::tui_markdown_runs_an_indented_blocks_adjacent_lines_together`; asserted here
        // only as "both are present", so this test fails for the reason it is about.
        s.see("▎ <code>");
        s.see("still code");
        // …and the pass that would have rewritten it is demonstrably still running elsewhere, so
        // this is not "conversion stopped everywhere".
        s.dont_see("<kbd>Ctrl</kbd>");

        // Tab reaches it, and `y` `c` copies its exact raw source (not the screen's decorated form).
        s.tab();
        assert!(
            s.app.md_focused_code(),
            "{label}: 字下げコードブロックに Tab でフォーカスできない"
        );
        assert_eq!(
            s.app.focused_code_text().as_deref(),
            Some("<code>\nstill code"),
            "{label}: コピー対象が生ソースと一致しない"
        );
        s.key('y');
        assert_eq!(
            s.app.pending_leader,
            Some(crate::keymap::LeaderId::Copy),
            "{label}: y はコピーリーダーを開く"
        );
        s.see("code block");
        s.key('c');
        assert!(
            s.app.pending_leader.is_none(),
            "{label}: c で確定=リーダーが閉じる"
        );
        assert!(s.app.flash.is_some(), "{label}: y c でコピー通知が出る");
        std::fs::remove_dir_all(&dir).ok();
    }
}

/// The other half of the same pair, through the app pipeline: a centered `<div align="center">`
/// badge banner — the shape at the top of a great many crate READMEs — must draw **no** code gutter.
///
/// This is what makes the fix above hard rather than obvious. `split_block_parts` lifts the `<img>`
/// lines out before the block splitters run, and what is left (`    </a>` / `    <a href="…">`) is
/// byte-for-byte an indented code block when read on its own. The naive fix — point
/// `split_html_blocks` at the indentation-aware mask — turns every banner like this into a `▎ …`
/// block full of raw markup, which is exactly what three registry READMEs did while it was tried.
/// So this test and the one above have to pass **together**; either alone is satisfiable by a wrong
/// answer.
#[test]
fn e2e_md_centered_banner_draws_no_code_gutter() {
    let body = concat!(
        "<p align=\"center\">\n",
        "    <a href=\"https://crates.io/crates/demo\">\n",
        "      <img src=\"https://img.example/v.svg\" alt=\"crates.io badge\">\n",
        "    </a>\n",
        "    <br>\n",
        "    <a href=\"LICENSE-MIT\">\n",
        "      <img src=\"https://img.example/mit.svg\" alt=\"MIT badge\">\n",
        "    </a>\n",
        "</p>\n",
        "\nOrdinary prose after the banner.\n",
    );
    let (s, dir) = md_preview(cfg_code_bg_none(), "banner_no_gutter", body);
    // No code block anywhere on screen, and no raw markup leaked into one.
    s.dont_see("▎");
    s.dont_see("</a>");
    // The badges survived as images (so the `<img>` extraction that creates the ambiguity really
    // happened) and the prose below the banner was not swallowed.
    s.see("crates.io badge");
    s.see("MIT badge");
    s.see("Ordinary prose after the banner.");
    std::fs::remove_dir_all(&dir).ok();
}

/// The same banner **one container deep**, through the app pipeline.
///
/// Root cause (2026-08): `structure_mask` reported only the outermost construct, so inside
/// `<details>` (or a blockquote) every line of the banner came back as the container rather than as
/// HTML-block body. The lone `    <br>` therefore took the hard-break rewrite, `"    " + "  "` is a
/// blank line to CommonMark, the inner `<p align="center">` ended there, and the 4-column-indented
/// lines below it became a real indented code block — so the markup drew as `▎ …` **and the second
/// badge stopped being extracted at all**.
///
/// The top-level form above is exercised by three tests; this is the level none of them reached, and
/// the measurement that caught it needed both containers: `<details>` shows the `▎` gutter, while
/// the quoted form loses its badge while drawing no konoma code gutter whatsoever.
///
/// The `<details>` case keeps the real README shape (`<img>` on its own line, so `split_block_parts`
/// leaves `    </a>` fragments behind); the quoted case puts the `<img>` inside the `<a>` line so
/// nothing is left over, because a *quoted* leftover fragment still draws as markup for an unrelated,
/// pre-existing reason — see `banner_quoted_multiline_still_leaks_its_leftover_fragments`. Both
/// shapes lost a badge before the fix, which is what this asserts.
#[test]
fn e2e_md_banner_nested_in_a_container_draws_no_code_gutter() {
    for (label, body) in [
        (
            "details",
            concat!(
                "<details open>\n",
                "<summary>Badges</summary>\n",
                "\n",
                "<p align=\"center\">\n",
                "    <a href=\"https://crates.io/crates/demo\">\n",
                "      <img src=\"https://img.example/v.svg\" alt=\"crates.io badge\">\n",
                "    </a>\n",
                "    <br>\n",
                "    <a href=\"LICENSE-MIT\">\n",
                "      <img src=\"https://img.example/mit.svg\" alt=\"MIT badge\">\n",
                "    </a>\n",
                "</p>\n",
                "\n",
                "</details>\n",
                "\nOrdinary prose after the banner.\n",
            ),
        ),
        (
            "blockquote",
            concat!(
                "> <p align=\"center\">\n",
                ">     <a href=\"https://crates.io/crates/demo\"><img src=\"https://img.example/v.svg\" alt=\"crates.io badge\"></a>\n",
                ">     <br>\n",
                ">     <a href=\"LICENSE-MIT\"><img src=\"https://img.example/mit.svg\" alt=\"MIT badge\"></a>\n",
                "> </p>\n",
                "\nOrdinary prose after the banner.\n",
            ),
        ),
    ] {
        let (s, dir) = md_preview(cfg_code_bg_none(), "banner_nested_no_gutter", body);
        s.dont_see("▎");
        s.dont_see("</a>");
        // Both badges survived the nesting — the second one is the one that used to vanish.
        s.see("crates.io badge");
        s.see("MIT badge");
        s.see("Ordinary prose after the banner.");
        assert!(
            !s.screen().contains("```"),
            "{label}: 生のフェンスが漏れている"
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}

/// Root cause (container-relative indentation, 2026-08 — reported by a user on the released build):
/// CommonMark measures a block's indentation from the container it sits in, so a fence indented 4
/// columns under `1. ` is a real fence, while a paragraph indented 6 columns under `   4. ` is not
/// code at all. The write-back scanner measured absolute columns and got both backwards, tripping
/// the document-wide count guard — `y c` answered "cannot copy code block" for *every* block in the
/// file. Both shapes are ordinary prose (a numbered install guide; the Apache license text), and 26
/// of 2,182 real `.md` files in the crate registry hit one of them.
///
/// Drives real keystrokes through a document containing both, plus an unrelated top-level fence that
/// used to be collaterally refused, and checks each block copies **its own** content.
#[test]
fn e2e_md_code_block_in_a_list_item_is_copyable() {
    let dir = sandbox("md_list_indented_fence");
    seed_files(&dir);
    std::fs::write(
        dir.join("snip.md"),
        // 1. a fence indented 4 columns inside an ordered item (a real fence)
        // 2. a 6-column continuation paragraph under `   4. ` (not code)
        // 3. an unrelated top-level fence
        "## Install\n\n1. Fork it:\n\n    ```sh\n    git clone x\n    ```\n\n\
         2. Build it.\n\n\
            4. Redistribution. You may reproduce and distribute copies of the\n\n\
         \x20     (b) You must cause any modified files to carry prominent notices\n\n\
         ```rust\nfn top_level() {}\n```\n",
    )
    .unwrap();
    let mut s = Sim::new(&canon(&dir));
    s.select("snip.md");
    s.enter();

    // 1st focusable code block: the one indented inside the ordered item. Its content comes back
    // flush-left (the item's content column is stripped, exactly as the screen shows it).
    s.tab();
    assert!(
        s.app.md_focused_code(),
        "1番目=リスト項目内のフェンスにフォーカス"
    );
    assert_eq!(
        s.app.focused_code_text().as_deref(),
        Some("git clone x"),
        "リスト項目内のフェンスがコピーできる(以前は文書全体が拒否された)"
    );
    s.key('y');
    s.key('c');
    assert!(s.app.flash.is_some(), "コピー通知が出る");

    // 2nd: the unrelated top-level fence — the continuation paragraph in between is not a code
    // block, so it must not consume an ordinal or shift this one.
    s.tab();
    assert!(s.app.md_focused_code(), "2番目=トップレベルのフェンス");
    assert_eq!(
        s.app.focused_code_text().as_deref(),
        Some("fn top_level() {}"),
        "ライセンス文体の字下げ段落をコードと数えず、序数がずれない"
    );

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

/// Root cause (heading/thematic-break/setext gate, 2026-08): an indented code block right after a
/// heading, with no blank line between them, used to go unrecognized by the write-back scanner (the
/// renderer draws it fine — only a paragraph gates a following indented block, and a heading isn't
/// one). The mismatch cancelled `y c` for the *whole document*, so an unrelated real fence right
/// after it (`npm test`, below) was collaterally refused too. Drives real keys through both blocks.
#[test]
fn e2e_md_indented_code_after_heading_no_blank_keeps_fence_copyable() {
    let dir = sandbox("md_heading_then_indented");
    seed_files(&dir);
    std::fs::write(
        dir.join("snip.md"),
        "## Usage\n    npm install foo\n\n```bash\nnpm test\n```\n",
    )
    .unwrap();
    let mut s = Sim::new(&canon(&dir));
    s.select("snip.md");
    s.enter();
    s.see("Usage");

    // 1st focusable item: the indented block right under the heading (no blank line).
    s.tab();
    assert!(
        s.app.md_focused_code(),
        "1番目=見出し直後(空行なし)の字下げコードにフォーカス"
    );
    assert_eq!(
        s.app.focused_code_text().as_deref(),
        Some("npm install foo"),
        "見出し直後の字下げコードの内容が取得できる(以前は文書全体のコピーが拒否されていた)"
    );
    s.key('y');
    s.key('c');
    assert!(s.app.flash.is_some(), "字下げ側のコピー通知が出る");

    // 2nd focusable item: the real fence right after it — must not be collaterally refused.
    s.tab();
    assert!(s.app.md_focused_code(), "2番目=フェンスコードにフォーカス");
    assert_eq!(
        s.app.focused_code_text().as_deref(),
        Some("npm test"),
        "無関係な実フェンスまで巻き添えで拒否されない"
    );
    s.key('y');
    s.key('c');
    assert!(s.app.flash.is_some(), "フェンス側のコピー通知も出る");

    std::fs::remove_dir_all(&dir).ok();
}

/// Root cause (container-closed fences, 2026-08): a fence is closed by a delimiter line **or** by the
/// end of the container it was opened in. The mask gating the block splitters knew only the first
/// rule, so a fence in a list item whose "closing" line carries trailing text — `` ``` ([#2642](…)) ``,
/// nix's CHANGELOG.md line 110 — was read as running to the end of the *document*, and every
/// construct below it was skipped as "inside code". Drives real keys, and asserts on the drawn
/// screen: the GFM table below renders with borders instead of one line of raw pipes, and the fence
/// itself is still a copyable code block (its own extent unchanged).
#[test]
fn e2e_md_fence_closed_by_its_list_item_still_renders_the_table_below() {
    let dir = sandbox("md_container_closed_fence");
    seed_files(&dir);
    std::fs::write(
        dir.join("snip.md"),
        "- Removed `Eq`, for example:\n\n  ```\n  let h = 1;\n  ``` ([#2642](https://example.com/pull/2642))\n\n\
         Implemented I/O safety:\n\n| Original Type | New Type |\n| ------------- | -------- |\n| AsRawFd | AsFd |\n\n\
         ```sh\nafter\n```\n",
    )
    .unwrap();
    let mut s = Sim::new(&canon(&dir));
    s.select("snip.md");
    s.enter();

    // The table below the fence is drawn by konoma's own table renderer (borders), not collapsed by
    // tui-markdown into `| Original Type | New Type | | ---…` on one line.
    // `┬` only ever comes from konoma's table renderer (the preview frame itself draws `┌`/`│`).
    s.see("┬");
    s.see("│ Original Type │ New Type │");
    s.see("│ AsRawFd");
    s.dont_see("| Original Type | New Type | |");

    // The fence's own extent is unchanged: it is still the 1st focusable code block, and the real
    // fence after the table is still the 2nd (the count guard covers the whole document, so a
    // disagreement here would refuse `y c` everywhere in the file).
    s.tab();
    assert!(
        s.app.md_focused_code(),
        "1番目=リスト項目内のコードブロック"
    );
    assert_eq!(
        s.app.focused_code_text().as_deref(),
        Some("let h = 1;\n``` ([#2642](https://example.com/pull/2642))\n"),
        "末尾テキスト付きの行は閉じないので、その行もブロックの中身のまま"
    );
    s.tab();
    assert_eq!(
        s.app.focused_code_text().as_deref(),
        Some("after"),
        "表の後ろの実フェンスが2番目として拾える"
    );
    s.key('y');
    s.key('c');
    assert!(s.app.flash.is_some(), "コピーが拒否されない");

    std::fs::remove_dir_all(&dir).ok();
}

/// The other axis the same gate was blind to: an **indented** code block (4+ columns), whose contents
/// are a literal transcript. A GFM table or a `> [!NOTE]` written inside one used to be carved out and
/// drawn as a real table / callout, tearing the code block apart. Runs under the user's real
/// `code_bg = "none"`, where a code block is only identifiable by its `▎` gutter.
#[test]
fn e2e_md_indented_code_block_keeps_table_and_alert_literal() {
    let (s, dir) = md_preview(
        cfg_code_bg_none(),
        "md_indented_literal",
        "Example table markup:\n\n    | a | b |\n    |---|---|\n    | 1 | 2 |\n\nExample alert markup:\n\n    > [!NOTE]\n    > body\n",
    );
    // Literal, verbatim — not carved into a bordered table or a colored callout box. `┬` is unique
    // to the table renderer and `▌` to the alert box (the preview frame draws neither); `▎` is the
    // code-block gutter, the only mark of a code block when `code_bg = "none"`.
    s.see("| a | b |");
    s.see("> [!NOTE]");
    s.dont_see("┬");
    s.dont_see("▌");
    s.see("▎");
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
fn e2e_returning_from_a_preview_drops_a_row_whose_file_vanished() {
    // The reported bug, at the seam where it became visible: while one file is previewed another
    // file in the same directory disappears (an agent, or an external `rm`), and `q` brings back a
    // listing that still carries its row — with a freshly computed, entirely fictional `0 B` in the
    // size column, because returning to the tree re-derived the detail cells but not the rows.
    //
    // Deliberately watcher-free (and git-free): this covers the *self-repair* half of the fix, so
    // it holds no matter which fs events were delivered or missed. The watcher half — never
    // skipping a create/remove/rename — is covered in `main.rs` and `app::tests`.
    let dir = sandbox("stale_row_on_return");
    std::fs::write(dir.join("kept.txt"), "kept-kept-kept\n").unwrap();
    std::fs::write(dir.join("gone.txt"), "gone\n").unwrap();
    let dir = canon(&dir);

    let mut cfg = Config::default();
    cfg.ui.details = vec!["size".into()];
    let mut s = Sim::with_config(&dir, cfg);
    s.see("gone.txt");
    s.see("5 B"); // gone.txt's real size, before it disappears
    s.dont_see("0 B");

    s.select("kept.txt");
    s.enter(); // full-screen preview
    assert!(s.app.tab.preview_path.is_some(), "プレビューに入っている");

    // Disappears while the preview is up — exactly what an agent deleting a build artifact does.
    std::fs::remove_file(dir.join("gone.txt")).unwrap();

    s.key('q'); // back to the tree
    assert!(
        !s.app
            .tab
            .entries
            .iter()
            .any(|e| e.path.ends_with("gone.txt")),
        "消えたファイルの行が entries に残っている(ツリーを再検証していない)"
    );
    s.dont_see("gone.txt");
    // The specific lie: a row for a file that no longer exists, sized 0 B.
    s.dont_see("0 B");
    s.see("kept.txt"); // …without throwing away the rows that are still real
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn e2e_returning_from_a_preview_keeps_the_active_filter() {
    // Guards the risk the seam re-verification introduces: `rebuild_tree` resets `entries` to the
    // whole tree, so re-deriving the listing on the way back from a preview must go through the
    // filter-preserving path. Getting this wrong shows the query in the title while listing
    // everything under it — the same inconsistency a previous fix (tab switching) had to repair.
    let dir = sandbox("filter_survives_preview");
    seed_files(&dir);
    let dir = canon(&dir);
    let mut s = Sim::new(&dir);

    s.key('/');
    s.keys("csv");
    s.enter(); // confirm the filter (the list stays filtered)
    let filtered = s.app.tab.entries.len();
    assert!(filtered >= 1, "csv に一致する行がある");
    s.see("data.csv");
    s.dont_see("readme.md");

    s.select("data.csv");
    s.enter(); // preview
    assert!(s.app.tab.preview_path.is_some(), "プレビューに入っている");
    s.key('q'); // back to the tree — re-derives the listing

    assert_eq!(
        s.app.tab.entries.len(),
        filtered,
        "プレビューから戻っても絞り込みが効いたまま(全件に戻らない)"
    );
    s.see("data.csv");
    s.dont_see("readme.md");
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
// Stale-listing indicator (the context bar's STALE chip)
// =============================================================================

/// English wording for the chip regardless of the machine's locale (`lang = "auto"` follows the OS,
/// and these tests match on "STALE").
fn cfg_en() -> Config {
    let mut cfg = Config::default();
    cfg.ui.lang = "en".into();
    cfg
}

/// A failed tree rebuild deliberately keeps the last good listing, so a stale tree looks exactly
/// like a healthy one — files that no longer exist sit there with nothing to doubt them by. The
/// flash that announces the moment it happens is wiped by the very next keypress, which is the
/// whole reason the *level* needs a chip: it has to still be there ten keys later.
///
/// Pins all three halves — it appears, it survives keypresses (while the flash does not), and it
/// disappears once a rebuild succeeds — plus the negative (nothing at all while healthy), so an
/// "always show the chip" implementation cannot pass either.
#[test]
fn e2e_stale_listing_chip_stays_up_until_the_listing_recovers() {
    let dir = sandbox("stale_chip_level");
    seed_files(&dir);
    let root = canon(&dir);
    let mut s = Sim::with_config(&root, cfg_en());

    // Healthy: no chip at all.
    s.see("notes.txt");
    s.dont_see("STALE");
    assert!(!s.app.tree_stale());

    // The root disappears out from under us (an agent removing a directory konoma happens to be
    // showing). This is the fs-watch path the run loop drives, which cannot propagate a failure.
    std::fs::remove_dir_all(&root).unwrap();
    s.app.refresh_fs_watched(false, &[]);
    s.draw();
    s.see("STALE");
    s.see_styled(
        "STALE",
        |st| st.bg == Some(ratatui::style::Color::Yellow),
        "警告色(黄)の背景",
    );
    // The listing itself is still there — which is exactly why the chip has to be.
    s.see("notes.txt");
    // …and the one-shot announcement fired for this frame.
    let flash = s.app.flash.clone().expect("立ち上がりで一度は伝える");
    assert!(flash.contains("out of date"), "{flash}");

    // Keypresses clear the flash. The chip must outlive every one of them — that is the difference
    // this test exists to pin.
    for k in ['j', 'k', 'g', 'G', 'j', 'j', 'k'] {
        s.key(k);
        assert_eq!(s.app.flash, None, "flash はキー入力で消える");
        s.see("STALE");
    }

    // Recovery takes it down again (nothing keeps announcing "stale" over a healthy listing).
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("back.txt"), b"y").unwrap();
    s.app.refresh_fs_watched(false, &[]);
    s.draw();
    assert!(!s.app.tree_stale());
    s.dont_see("STALE");
    s.see("back.txt");
    std::fs::remove_dir_all(&root).ok();
}

/// The chip states something about **the listing you are looking at**, so a broken tab sitting in
/// the background must not put it on screen while a healthy tab is in front. Every tab activation
/// ends in a rebuild (`load_active` → `refresh_fs_after_tab_switch`), which is what keeps the
/// app-wide flag describing the visible tab; this pins that, in both directions and repeatedly.
#[test]
fn e2e_stale_listing_chip_follows_the_tab_you_are_looking_at() {
    let dir = sandbox("stale_chip_tabs");
    let base = canon(&dir);
    for sub in ["alpha", "beta"] {
        std::fs::create_dir_all(base.join(sub)).unwrap();
    }
    std::fs::write(base.join("alpha/in-alpha.txt"), b"a\n").unwrap();
    std::fs::write(base.join("beta/in-beta.txt"), b"b\n").unwrap();
    let mut s = Sim::with_config(&base, cfg_en());

    // tab2 = alpha, tab3 = beta (Ctrl-t on a directory roots a new tab there).
    s.select("alpha");
    s.ctrl('t');
    s.key('['); // back to tab1 to pick the other directory
    s.select("beta");
    s.ctrl('t');
    assert_eq!(s.app.tab_count(), 3);
    s.see("in-beta.txt");
    s.dont_see("STALE");

    // tab2's root vanishes while tab3 is in front. Nothing is on screen about it yet — correct,
    // that tab is not the one being shown.
    std::fs::remove_dir_all(base.join("alpha")).unwrap();
    s.dont_see("STALE");

    // Switching to it says so, and keeps saying so.
    s.key('[');
    assert_eq!(s.app.active_tab_index(), 1);
    s.see("STALE");
    s.see("in-alpha.txt"); // the stale listing it is warning about

    // Switching to the healthy tab takes it back down…
    s.key(']');
    assert_eq!(s.app.active_tab_index(), 2);
    s.dont_see("STALE");
    // …and coming back brings it up again (not a one-shot that got used up).
    s.key('[');
    s.see("STALE");
    std::fs::remove_dir_all(&base).ok();
}

/// The context bar is drawn in every mode, so the chip must be too: a stale listing does not stop
/// being stale because you are looking at a file. (The Git view is covered by
/// `stale_listing_chip_shows_in_the_git_view` in `app/tests.rs`, which needs a real repository.)
#[test]
fn e2e_stale_listing_chip_shows_in_tree_and_preview() {
    let dir = sandbox("stale_chip_modes");
    seed_files(&dir);
    let root = canon(&dir);
    let mut s = Sim::with_config(&root, cfg_en());

    // Open a preview first, so the root can then be pulled out from under an open file.
    s.select("notes.txt");
    s.enter();
    assert!(matches!(s.app.tab.mode, Mode::Preview));
    s.dont_see("STALE");

    std::fs::remove_dir_all(&root).unwrap();
    s.app.refresh_fs_watched(false, &[]);
    s.draw();
    assert!(matches!(s.app.tab.mode, Mode::Preview), "プレビューのまま");
    s.see("STALE");

    // Back to the tree: still stale, still shown (the same flag, a shared context bar).
    s.key('q');
    assert!(matches!(s.app.tab.mode, Mode::Tree));
    s.see("STALE");
    std::fs::remove_dir_all(&root).ok();
}

/// The tab bar shares its row with the context bar and is sized by subtracting
/// `status::context_width` (v0.9.0 shipped a bug where it planned across the full width and got
/// painted under the right-hand side). One more chip widens that subtraction, so this pins that the
/// tab bar still behaves with the chip up: the active tab and the overflow markers stay visible,
/// and the chip sits entirely inside the reserved right-hand region rather than on top of a tab.
#[test]
fn e2e_stale_listing_chip_does_not_break_the_tab_bar() {
    let dir = sandbox("stale_chip_tabbar");
    seed_files(&dir);
    let root = canon(&dir);
    let mut s = Sim::with_config(&root, cfg_en());

    s.keys("tttttttt"); // 9 tabs → the bar overflows at width 90
    assert_eq!(s.app.tab_count(), 9);
    s.key('5');
    assert_eq!(s.app.active_tab_index(), 4);

    std::fs::remove_dir_all(&root).unwrap();
    s.app.refresh_fs_watched(false, &[]);
    s.draw();

    s.see("STALE");
    // The overflow window still works: markers on both sides, active tab still visible.
    s.see("‹");
    s.see("›");
    let label = s.app.tab_label(4);
    s.see(&format!("5:{label}"));

    // Both live on the top row, and the chip is inside the region the tab bar was told to keep
    // clear — if the tab bar had planned across the full width, a tab chip would be sitting where
    // the context bar then painted over it.
    let width = s.term.backend().buffer().area.width as usize;
    let ctx = crate::ui::status::context_width(&s.app) as usize;
    let (chip_row, chip_col) = s.find_text("STALE").expect("チップが描かれている");
    assert_eq!(chip_row, 0, "チップは上段(タブバーと同じ行)");
    assert!(
        chip_col as usize >= width - ctx,
        "チップがタブバー側にはみ出している (col={chip_col}, 予約境界={})",
        width - ctx
    );
    let (tab_row, tab_col) = s.find_text(&format!("5:{label}")).expect("アクティブタブ");
    assert_eq!(tab_row, 0);
    assert!(
        (tab_col as usize) < width - ctx,
        "タブがコンテキスト側に食い込んでいる (col={tab_col})"
    );
    std::fs::remove_dir_all(&root).ok();
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
    let dir = unique_tmp("konoma_e2e_file_paging");
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
    let base = unique_tmp("konoma_e2e_session_restore_base");
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
    let Some(fixture) = sample_path_or_skip("sample.svg") else {
        return;
    };
    let dir = sandbox("search_media");
    std::fs::copy(&fixture, dir.join("pic.svg")).unwrap();
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

// --- Background git writes through real key input (design principle #4) -----------------------

/// Install a `pre-commit` hook that parks until the test removes `$GIT_DIR/BLOCK`, so a commit's
/// duration is controlled by the test rather than by a timer.
///
/// The `max` bound is a safety net, not a timing assumption: it exists so that a regression which
/// puts the write back on the UI thread **fails** (the commit eventually lands, and the "no commit
/// yet" assertion below breaks) instead of hanging the suite forever.
#[cfg(feature = "git")]
fn install_blocking_pre_commit_hook(dir: &std::path::Path) {
    let hooks = dir.join(".git").join("hooks");
    std::fs::create_dir_all(&hooks).unwrap();
    let hook = hooks.join("pre-commit");
    std::fs::write(
        &hook,
        "#!/bin/sh\nn=0\nwhile [ -f \"$(git rev-parse --git-dir)/BLOCK\" ] && [ $n -lt 1000 ]; do\n  sleep 0.02\n  n=$((n+1))\ndone\nexit 0\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

/// Stage and commit through the real key path with the background runner attached: `o` `s` `c`
/// <message> Enter, then the run loop's drain step. The keys are identical to the synchronous
/// version (`e2e_git_stage_then_commit_appears_in_log`) — moving the write off-thread must not
/// change what the user types or sees afterwards.
#[cfg(feature = "git")]
#[test]
fn e2e_git_stage_and_commit_through_the_background_runner() {
    let dir = sandbox("git_async_commit");
    seed_repo(&dir);
    let mut s = Sim::new(&canon(&dir)).with_async_git_ops();
    s.key('o');
    assert!(s.app.is_git_view());
    assert_eq!(staged_of(&s.app, "a.rs"), Some(false));

    s.key('s');
    assert!(s.app.git_op_running(), "ステージは裏で走る");
    s.drain_git_ops();
    assert_eq!(
        staged_of(&s.app, "a.rs"),
        Some(true),
        "適用後に staged になる"
    );

    s.key('c');
    assert!(s.app.is_dialog());
    s.keys("ASYNCCOMMITSUBJECT");
    s.enter();
    assert!(s.app.git_op_running(), "コミットも裏で走る");
    s.drain_git_ops();

    let commits = crate::git::log(&s.app.tab.root, 200);
    assert!(
        commits.iter().any(|c| c.summary == "ASYNCCOMMITSUBJECT"),
        "コミットが log に載る: {:?}",
        commits
            .iter()
            .map(|c| c.summary.clone())
            .collect::<Vec<_>>()
    );
    assert!(s.app.is_git_view(), "完了後も Git ビューに居る");
    assert_eq!(
        staged_of(&s.app, "a.rs"),
        None,
        "コミット済みは一覧から消える"
    );
    s.key('l');
    assert!(s.app.is_git_log());
    s.see("ASYNCCOMMITSUBJECT");
    std::fs::remove_dir_all(&dir).ok();
}

/// **The point of D3, verified structurally rather than by a clock.** While `git commit` is parked
/// inside a `pre-commit` hook, konoma must keep handling keys and drawing.
///
/// No wall-clock assertion is involved: the hook holds until this test deletes `BLOCK`, so
/// "`git log` still has only the seed commits" is a fact the test controls, not a race. The
/// combination of "the commit has demonstrably not finished" and "these keys were processed and
/// these frames rendered" is what proves the write is not on the UI thread. Before the fix,
/// `s.enter()` never returned until the hook gave up, so the first assertion below would fail.
#[cfg(feature = "git")]
#[test]
fn e2e_ui_keeps_responding_while_a_git_commit_is_blocked() {
    let dir = sandbox("git_blocked_commit");
    seed_repo(&dir);
    install_blocking_pre_commit_hook(&dir);
    let root = canon(&dir);
    let git_dir = root.join(".git");
    std::fs::write(git_dir.join("BLOCK"), b"").unwrap();

    // Stage outside the harness, so the commit is the *only* git op this test dispatches and the
    // "still running" assertion below is the first thing a regression trips over.
    crate::git::stage_all(&root).unwrap();
    let before = crate::git::log(&root, 200).len();
    let mut s = Sim::new(&root).with_async_git_ops();
    s.key('o');
    s.key('c');
    s.keys("BLOCKEDCOMMIT");
    s.enter(); // dispatches the commit; the hook is parked

    assert!(
        s.app.git_op_running(),
        "コミットは実行中(まだ完了していない)"
    );
    assert_eq!(
        crate::git::log(&root, 200).len(),
        before,
        "フックが握っている間はコミットが作られていない"
    );

    // Keys keep working, and every one of them redraws (Sim::press draws after each key, exactly
    // like the run loop) — so this also proves rendering isn't stuck behind the write.
    s.key('q'); // leave the Git view
    assert!(!s.app.is_git_view(), "git 実行中でも Git ビューを出られる");
    let start = s.app.tab.selected;
    for _ in 0..3 {
        s.key('j');
    }
    assert!(
        s.app.tab.selected > start,
        "git 実行中でもツリーのカーソルが動く"
    );
    s.key('k');
    s.see("a.rs"); // a frame really was produced
                   // Still blocked, and still nothing committed: the keys above did not secretly wait for git.
    assert!(s.app.git_op_running(), "ここまでキーを叩いてもまだ実行中");
    assert_eq!(
        crate::git::log(&root, 200).len(),
        before,
        "キー処理の間もコミットは作られていない"
    );

    // Release the hook and let the result land.
    std::fs::remove_file(git_dir.join("BLOCK")).unwrap();
    s.drain_git_ops();
    assert!(!s.app.git_op_running(), "適用で実行中フラグが解ける");
    let commits = crate::git::log(&root, 200);
    assert!(
        commits.iter().any(|c| c.summary == "BLOCKEDCOMMIT"),
        "解放後にコミットが完了する: {:?}",
        commits
            .iter()
            .map(|c| c.summary.clone())
            .collect::<Vec<_>>()
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// `Q` while a git write is in flight must warn before quitting, even with `confirm_quit = false`:
/// leaving mid-`commit` orphans a `git` that is still holding `.git/index.lock`.
#[cfg(feature = "git")]
#[test]
fn e2e_quit_during_a_git_write_asks_for_confirmation() {
    let dir = sandbox("git_quit_during_write");
    seed_repo(&dir);
    install_blocking_pre_commit_hook(&dir);
    let root = canon(&dir);
    std::fs::write(root.join(".git").join("BLOCK"), b"").unwrap();

    let mut cfg = Config::default();
    cfg.ui.confirm_quit = false;
    crate::git::stage_all(&root).unwrap();
    let mut s = Sim::with_config(&root, cfg).with_async_git_ops();
    s.key('o');
    s.key('c');
    s.keys("WILLQUIT");
    s.enter();
    assert!(s.app.git_op_running(), "コミットは裏で実行中");

    s.key('Q');
    assert!(!s.quit, "確認を出すのでこの時点では終了しない");
    assert!(s.app.is_dialog(), "確認ダイアログが出る");
    s.see(crate::i18n::tr(
        s.app.lang,
        crate::i18n::Msg::QuitWhileGitOp,
    ));

    // The warning only warns: `y` still leaves.
    s.key('y');
    assert!(s.quit, "y なら終了できる(引き止めない)");

    std::fs::remove_file(root.join(".git").join("BLOCK")).ok();
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

// =============================================================================
// The windowed-diff perf fix: `render_gitdiff` must highlight only what's on screen.
//
// `windowed_range_only_highlights_visible_rows_{unified,side_by_side}` in gitdiff.rs prove
// `diff_lines_range`/`diff_lines_side_by_side_range` themselves are windowed — called directly, not
// through `render_gitdiff`/`ui::render`. That leaves the wiring unverified: nothing stops
// `render_gitdiff` from going back to passing `(0, diff.len())` (the whole document) while the
// windowed functions it calls stay correct in isolation — and the 16 tests that existed before this
// one all stayed green when that exact regression was tried (see this file's own history). These
// tests drive a real keystroke → `handle_key` → `ui::render` pass (the same path the run loop takes)
// against a diff of 2,400+ rows and assert the highlight counter — which `diff_line_to_line`/
// `render_half` bump on every actual `highlight_line_by_ext` call — stays in the viewport's order of
// magnitude, not the diff's.
// =============================================================================

/// A repo with one file whose working-tree diff is far larger than any screen: every one of
/// `n` lines is rewritten, so nothing folds into "unchanged" context — the diff is ~2n rows
/// (n Removed + n Added), all real output from an actual `git diff` invocation (not a synthetic
/// `Vec<DiffLine>` built in-process), so `App::git_diff_lines()` → `file_diff` → `render_gitdiff`'s
/// real wiring runs end to end.
#[cfg(feature = "git")]
fn seed_huge_diff_repo(dir: &std::path::Path) {
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
    let n = 1200;
    let original: String = (0..n).map(|i| format!("line {i}\n")).collect();
    std::fs::write(dir.join("big.txt"), &original).unwrap();
    run(&["add", "-A"]);
    run(&["commit", "-q", "-m", "init"]);
    // Rewrite every single line: nothing is left unchanged, so the working-tree diff has ~2n rows
    // (n Removed immediately followed by n Added), far larger than any terminal's screen.
    let changed: String = (0..n).map(|i| format!("line {i} changed\n")).collect();
    std::fs::write(dir.join("big.txt"), &changed).unwrap();
}

#[cfg(feature = "git")]
#[test]
fn e2e_gitdiff_unified_render_highlights_only_the_visible_rows() {
    let dir = sandbox("gitdiff_unified_windowed");
    seed_huge_diff_repo(&dir);
    let mut s = Sim::new(&canon(&dir));
    s.select("big.txt");
    s.key('d');
    assert!(s.app.is_git_diff_preview(), "d で big.txt の diff を開く");
    assert!(
        !s.app.diff_is_split(200),
        "既定は unified(diff.layout=\"unified\")"
    );

    // The diff itself must actually be far larger than the screen, or a small highlight count
    // would prove nothing (it could just be a small diff).
    let total_rows = s.app.git_diff_lines().len();
    assert!(
        total_rows > 2000,
        "全1200行を書き換えたので diff は2000行超のはず(実際 {total_rows})"
    );

    // `s.select`/`s.key('d')` above already triggered draws while the highlight counter was
    // running (it's already been counting since Sim::new's very first draw) — reset it and take
    // exactly one more render pass to measure, mirroring the unit tests in gitdiff.rs.
    crate::preview::gitdiff::reset_highlight_calls_for_test();
    s.draw();
    let calls = crate::preview::gitdiff::highlight_calls_for_test();
    let vh = s.app.tab.preview_viewport as usize;
    assert!(vh > 0, "diff プレビューの高さが描画で設定されているはず");
    assert!(
        calls > 0 && calls <= vh * 3,
        "可視範囲(高さ{vh})を大きく超えてハイライトしている(全{total_rows}行中{calls}回): \
         render_gitdiff が diff_lines_range に全行を渡す退行が起きていないか"
    );

    // Scrolling must not break the windowing: after `G` (to the very end), the count must stay in
    // the same order of magnitude, not jump toward `total_rows`.
    s.key('G');
    crate::preview::gitdiff::reset_highlight_calls_for_test();
    s.draw();
    let calls_at_bottom = crate::preview::gitdiff::highlight_calls_for_test();
    assert!(
        calls_at_bottom > 0 && calls_at_bottom <= vh * 3,
        "末尾(G)へスクロールしても画面オーダーのはず(全{total_rows}行中{calls_at_bottom}回)"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[cfg(feature = "git")]
#[test]
fn e2e_gitdiff_side_by_side_render_highlights_only_the_visible_rows() {
    let dir = sandbox("gitdiff_split_windowed");
    seed_huge_diff_repo(&dir);
    let mut s = Sim::new(&canon(&dir));
    s.select("big.txt");
    s.key('d');
    s.key('s'); // unified → side-by-side (unconditional at any width, unlike Auto)
    assert!(
        s.app.diff_is_split(1) && s.app.diff_is_split(200),
        "s 一回で side-by-side に固定されるはず"
    );
    s.see("side-by-side");

    let diff = s.app.git_diff_lines();
    let ext = s.app.current_preview_ext().to_string();
    let total_rows = crate::preview::gitdiff::side_by_side_row_count(&diff, &ext);
    assert!(
        total_rows > 1000,
        "side-by-side の行数も画面よりずっと大きいはず(実際 {total_rows})"
    );

    // Each side-by-side row can highlight up to two halves (old + new) — same 2x headroom the
    // unit tests in gitdiff.rs use.
    crate::preview::gitdiff::reset_highlight_calls_for_test();
    s.draw();
    let calls = crate::preview::gitdiff::highlight_calls_for_test();
    let vh = s.app.tab.preview_viewport as usize;
    assert!(vh > 0);
    assert!(
        calls > 0 && calls <= vh * 6,
        "side-by-side でも可視範囲(高さ{vh})を大きく超えてハイライトしている(全{total_rows}行中{calls}回)"
    );

    // Scrolling in side-by-side layout must not break the windowing either.
    s.key('G');
    crate::preview::gitdiff::reset_highlight_calls_for_test();
    s.draw();
    let calls_at_bottom = crate::preview::gitdiff::highlight_calls_for_test();
    assert!(
        calls_at_bottom > 0 && calls_at_bottom <= vh * 6,
        "side-by-side で末尾(G)へスクロールしても画面オーダーのはず(全{total_rows}行中{calls_at_bottom}回)"
    );

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
    // Not just "some flash appeared" — specifically the *unrecognized* classification
    // (`Msg::PasteJumpUnrecognized`), so this can't silently regress into reporting "not found" or
    // "no clipboard" instead (see `parse_paste_target` / `paste_jump_from` in `paste_jump.rs`).
    assert_eq!(
        s.app.flash.as_deref(),
        Some(crate::i18n::tr(
            s.app.lang,
            crate::i18n::Msg::PasteJumpUnrecognized
        )),
        "非 GitHub URL は unrecognized として分類されるはず: {:?}",
        s.app.flash
    );
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
    // Not just "some flash appeared" — specifically *why* it was refused: `AlreadyExists` (the
    // typed `fileops::FileOpError` translated by `App::describe_error`, added in v0.23.1). Without
    // this, the rename could start silently succeeding, or start failing for an unrelated reason
    // (e.g. a permissions error), and this test would still pass.
    assert!(
        s.app.flash.as_deref().is_some_and(
            |f| f.contains(crate::i18n::tr(s.app.lang, crate::i18n::Msg::AlreadyExists))
        ),
        "既に存在するので AlreadyExists の flash が出るはず: {:?}",
        s.app.flash
    );
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

    // Advance focus onto the visible checkbox. Tab cycles links/tasks/details in document order;
    // a closed `<details>` renders only its summary as a focusable item (the hidden checkbox inside
    // never becomes one), so with this document the only two Tab stops are the visible task and the
    // details heading. Bounded to a couple of full cycles (2 items ⇒ 2 tabs is enough) so that if a
    // future change ever drops the task item entirely, this fails loudly with a clear message
    // instead of degrading into the old "either" assertion that passed even when nothing moved.
    let mut on_task = false;
    for _ in 0..4 {
        s.tab();
        if s.app.md_focused_task() {
            on_task = true;
            break;
        }
    }
    assert!(
        on_task,
        "Tab で可視タスクへフォーカスできるはず(focused_item={:?}, md_focused_details={:?})",
        s.app.focused_item(),
        s.app.md_focused_details()
    );

    s.key(' ');
    let out = read();
    // Both assertions matter: the visible task must actually flip (not "either this or that stayed
    // the same" — that OR let a no-op write-back pass), and the hidden one must be left alone (it's
    // not even visible, so Space must never have reached it).
    assert!(
        out.contains("- [x] visible"),
        "可視タスクが実際にトグルされる(全キャンセルされない): {out}"
    );
    assert!(
        out.contains("- [ ] collapsed"),
        "隠れたタスクは変化しない: {out}"
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
// H2/H3 (docs/archive/AUDIT-TESTS-2026-08.md): style-attribute assertions + config→keybinding wiring.
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
    // separate, larger change — H1 in docs/archive/AUDIT-TESTS-2026-08.md, out of scope for this task), so
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
// H3 (docs/archive/AUDIT-TESTS-2026-08.md): full `[ui]` config-field coverage table.
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
        Coverage::Covered(
            "e2e_ui_tab_width_changes_expansion_columns (plain text) / \
             e2e_ui_tab_width_changes_expansion_in_markdown_code_block (Markdown fence + \
             extreme values 0/usize::MAX)",
        ),
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
        // `ThemeConfig` has 5 sub-fields; `code_bg` was the only one with a real e2e contrast
        // before the 2026-08-17 pre-release sweep added one test per remaining sub-field.
        Coverage::Covered(
            "e2e_ui_theme_code_bg_toggle_changes_code_block_background (code_bg) / \
             e2e_ui_theme_code_theme_changes_markdown_code_block_highlight_colors (code_theme) / \
             e2e_ui_theme_code_label_align_left_vs_right_moves_badge_in_markdown (code_label_align) / \
             e2e_ui_theme_code_label_bg_configurable_in_markdown (code_label_bg) / \
             e2e_ui_theme_bg_paints_the_whole_frame_background (bg)",
        ),
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

/// `tab_width`, inside a MARKDOWN FENCED CODE BLOCK specifically — the test just above only ever
/// exercises a plain `.txt` file; `CodeStyle.tab_width` is threaded through
/// `render_markdown_with_images` as its own argument, separate from the standalone-code preview
/// path, so this is a genuinely different call site worth its own keystroke-level check. Also covers
/// the "極端値" (extreme value) axis this config's own field explicitly promises: `0` disables
/// expansion (raw tab char kept, no `→` marker at all) and an absurd out-of-range value safely
/// clamps back to the default (4) rather than allocating a huge buffer or hanging
/// (`config::clamp_tab_width`).
#[test]
fn e2e_ui_tab_width_changes_expansion_in_markdown_code_block() {
    let dir = sandbox("ui_tab_width_md_cfg");
    std::fs::write(dir.join("d.md"), "```rust\n\tX\n```\n").unwrap();
    let root = canon(&dir);

    let mut s4 = Sim::new(&root); // default tab_width=4: marker + 3 spaces = 4 cols
    s4.select("d.md");
    s4.enter();
    s4.see("→   X");

    let mut cfg8 = Config::default();
    cfg8.ui.tab_width = 8;
    let mut s8 = Sim::with_config(&root, cfg8); // marker + 7 spaces = 8 cols
    s8.select("d.md");
    s8.enter();
    s8.see("→       X");
    s8.dont_see("→   X");

    // 0 = expansion disabled: the raw tab character survives, no marker drawn at all.
    let mut cfg0 = Config::default();
    cfg0.ui.tab_width = 0;
    let mut s0 = Sim::with_config(&root, cfg0);
    s0.select("d.md");
    s0.enter();
    s0.dont_see("→");

    // An absurd out-of-range value must not crash/hang/allocate huge — it clamps back to the
    // default (4), the same as `tab_width` unset.
    let mut cfg_huge = Config::default();
    cfg_huge.ui.tab_width = 9_223_372_036_854_775_807; // usize::MAX on a 64-bit target
    let mut s_huge = Sim::with_config(&root, cfg_huge);
    s_huge.select("d.md");
    s_huge.enter();
    s_huge.see("→   X"); // clamped to the default, not a huge allocation
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
    let Some(fixture) = sample_path_or_skip("sample.svg") else {
        return;
    };
    let dir = sandbox("ui_svg_max_px_cfg");
    std::fs::copy(&fixture, dir.join("p.svg")).unwrap();
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
    let Some(fixture) = sample_path_or_skip("sample.svg") else {
        return;
    };
    let dir = sandbox("ui_busy_indicator_cfg");
    std::fs::copy(&fixture, dir.join("p.svg")).unwrap();
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

// =============================================================================
// H1: real-keystroke-driven async media pipeline (`Sim::with_media`).
//
// Every test above that touches an inline image / mermaid fence / math expression uses
// `with_picker()`, which attaches a `Picker` but no loader channels — so `ensure_md_image` /
// `ensure_mermaid_fence_render` / `ensure_math_render` all take their **synchronous, no-tx**
// fallback branch (decode happens inline, on the test's own thread, before the first draw even
// finishes). That's fine for testing classification/sizing/config, but it never reaches the
// `if let Some(tx) = ...` branches that spawn a **real background thread** and hand the result
// back through a channel — the exact code path `main` uses, and the one where two real bugs (a
// synthetic-URL key going unrecognized, and RaTeX's invisible-black glyphs) shipped in v0.16.0
// without a single test — unit or e2e — ever exercising it.
//
// `with_media()` attaches the real channels (`attach_media_loader` / `attach_md_image_loader` /
// `attach_remote_md_loader` / `attach_md_encoder` + a real `md_encode_worker` thread), so the
// tests below drive that pipeline from actual keystrokes (`Sim::key`/`enter`/`tab`) end to end:
// decode on a worker thread → `drain_md_images` applies it and redraws → the redraw's renderer
// asks for an encode → `drain_md_encodes` applies that and redraws → real pixels reach the drawn
// terminal buffer, read back exactly like `e2e_ui_math_color_changes_rendered_pixel_hue` already
// does for the encode half alone.
// =============================================================================

/// Every `Color::Rgb` currently painted into the drawn buffer's cell foregrounds. Real
/// image/diagram/equation rasters (halfblocks protocol) paint actual pixel colors this way; plain
/// text/UI chrome uses named `Color` variants, never `Rgb`. Used to tell "a real raster reached the
/// screen" apart from "still a placeholder/blank" without needing to inspect `App`'s private cache.
fn drawn_rgb_fgs(term: &Terminal<TestBackend>) -> Vec<(u8, u8, u8)> {
    let buf = term.backend().buffer();
    let mut out = Vec::new();
    for cell in buf.content().iter() {
        if let Some(ratatui::style::Color::Rgb(r, g, b)) = cell.style().fg {
            out.push((r, g, b));
        }
    }
    out
}

/// A **local** inline Markdown image, decoded and encoded through the real background threads.
/// The placement (cols/rows) is available immediately after `enter()` (it's sized straight off the
/// file's on-disk dimensions, synchronously — no worker needed for that part), but the raster only
/// exists once `ensure_md_image`'s real decode thread reports back through `md_img_rx`, and a
/// drawable protocol only exists once the real `md_encode_worker` thread reports back through
/// `md_enc_rx`. Confirms both by reading real pixel colors off the drawn buffer, not just "a
/// `Protocol` object exists" (which a bug in the color pipeline could satisfy while still being
/// visually wrong).
#[test]
fn e2e_ui_inline_local_image_decodes_and_encodes_through_real_worker_threads() {
    let dir = sandbox("inline_local_image_real_workers");
    // A solid, distinctive color: any pixel that reaches the screen unblended is unambiguous proof
    // this specific raster (not some other placeholder/UI chrome) was drawn.
    image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
        300,
        120,
        image::Rgb([10, 210, 10]),
    ))
    .save(dir.join("pic.png"))
    .unwrap();
    std::fs::write(
        dir.join("d.md"),
        "before\n\n![a green square](pic.png)\n\nafter\n",
    )
    .unwrap();
    let root = canon(&dir);

    let mut s = Sim::new(&root).with_media();
    s.select("d.md");
    s.enter();
    let placements = s.app.md_images();
    assert_eq!(
        placements.len(),
        1,
        "サイズはファイルの寸法から即座に(デコード前でも)確定するはず"
    );
    let (url, cols, rows) = (
        placements[0].url.clone(),
        placements[0].cols,
        placements[0].rows,
    );
    assert!(cols > 0 && rows > 0);
    assert!(
        s.app.md_image_proto(&url, cols, rows, 0, rows).is_none(),
        "デコードが終わる前はまだ protocol が無いはず"
    );

    s.drain_md_images(); // real decode thread applies the raster + redraws (which requests an encode)
    s.drain_md_encodes(); // real encode-worker thread turns it into a drawable protocol

    assert!(
        s.app.md_image_proto(&url, cols, rows, 0, rows).is_some(),
        "エンコードが終われば protocol が立つはず"
    );
    let greens: Vec<_> = drawn_rgb_fgs(&s.term)
        .into_iter()
        .filter(|&(r, g, b)| g > 150 && r < 60 && b < 60)
        .collect();
    assert!(
        !greens.is_empty(),
        "実ピクセル(緑)が描画バッファに届いているはず: {:?}",
        drawn_rgb_fgs(&s.term)
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// Every kitty image id **transmitted** into the drawn terminal buffer (`i=<id>,a=T` heads a
/// transmit escape). That is the quantity a kitty terminal charges its image-storage budget
/// against, so it is the honest way to ask "is konoma filling the terminal up?".
fn transmitted_kitty_ids(term: &Terminal<TestBackend>) -> Vec<u32> {
    let dump: String = term
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|c| c.symbol())
        .collect();
    dump.split("i=")
        .skip(1)
        .filter_map(|part| part.split_once(",a=T"))
        .filter_map(|(id, _)| id.parse::<u32>().ok())
        .collect()
}

/// Write a `frames`-frame animated GIF, each frame a distinct solid color.
fn write_animated_gif(path: &std::path::Path, colors: &[[u8; 3]]) {
    use image::codecs::gif::GifEncoder;
    use image::{Delay, Frame, Rgba, RgbaImage};
    let file = std::fs::File::create(path).unwrap();
    let mut enc = GifEncoder::new(file);
    enc.set_repeat(image::codecs::gif::Repeat::Infinite)
        .unwrap();
    for c in colors {
        let img = RgbaImage::from_pixel(120, 60, Rgba([c[0], c[1], c[2], 255]));
        enc.encode_frame(Frame::from_parts(
            img,
            0,
            0,
            Delay::from_numer_denom_ms(10, 1),
        ))
        .unwrap();
    }
}

/// **The regression the whole fix exists for**, driven through the real renderer on a kitty
/// terminal: open a Markdown document with an animated GIF in it and let the animation run several
/// laps, exactly as the run loop does (`advance_md_gifs_if_due` → redraw → the redraw asks for the
/// new frame's encode → apply → redraw).
///
/// Before the fix every frame was transmitted under a fresh `rand::random()` id, so the terminal
/// accumulated one whole image per frame (~66 MB/min measured) until it hit its storage budget and
/// evicted a still image that had scrolled out of view — which konoma would never re-transmit, so
/// the still came back blank after a few minutes. The observable is therefore not bytes or timing
/// but the *number of distinct ids the terminal is handed*, read straight off the drawn buffer.
///
/// The unit-level sibling (`inline_gif_animation_never_transmits_a_second_kitty_image_id`) pins the
/// same invariant one layer down; this one additionally covers `overlay_inline_images`, i.e. that
/// the real draw path is what emits those transmits.
#[test]
fn e2e_ui_animated_inline_gif_reuses_one_kitty_image_id_across_laps() {
    let dir = sandbox("inline_gif_kitty_id");
    let colors = [[220, 20, 20], [20, 220, 20], [20, 20, 220]];
    write_animated_gif(&dir.join("anim.gif"), &colors);
    std::fs::write(dir.join("d.md"), "before\n\n![anim](anim.gif)\n\nafter\n").unwrap();
    let root = canon(&dir);

    let mut s = Sim::new(&root).with_media_kitty();
    s.select("d.md");
    s.enter();
    s.drain_md_images(); // the decode thread hands back all frames
    s.drain_md_encodes(); // the encode worker turns frame 0 into a drawable image

    let placements = s.app.md_images();
    assert_eq!(placements.len(), 1, "GIF が1枚配置される");
    let (url, cols, rows) = (
        placements[0].url.clone(),
        placements[0].cols,
        placements[0].rows,
    );
    assert!(
        s.app
            .md_image_proto(&url, cols, rows, 0, rows)
            .is_some_and(|i| i.is_kitty()),
        "kitty 端末では konoma 自前の転送経路を通る"
    );

    let mut ids: std::collections::BTreeSet<u32> =
        transmitted_kitty_ids(&s.term).into_iter().collect();
    assert_eq!(ids.len(), 1, "最初の描画で1枚だけ転送される");
    let mut transmits = ids.len();

    // Three laps of a three-frame animation, each step the run loop's own sequence.
    for _ in 0..(colors.len() * 3) {
        while !s.app.advance_md_gifs_if_due() {
            // The very first tick only starts the frame timer; the 10ms delays then elapse on
            // their own. Looping (rather than sleeping a fixed amount) keeps this independent of
            // how fast the machine is.
            std::hint::spin_loop();
        }
        s.draw(); // the redraw notices the new frame and requests its encode
        s.drain_md_encodes(); // ...which the worker returns, and drain redraws with it
        let frame_ids = transmitted_kitty_ids(&s.term);
        transmits += frame_ids.len();
        ids.extend(frame_ids);
    }

    // Vacuity guard: without real re-transmits the assertion below would hold trivially.
    assert!(
        transmits >= colors.len() * 3,
        "コマごとに転送が起きている: {transmits}"
    );
    assert_eq!(
        ids.len(),
        1,
        "何周させても端末に載る画像 ID は1つのまま(= 画像ストレージが埋まらない): {ids:?}"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// Two ```mermaid fences, both rendered on real background threads: Tab-focuses the caption line
/// (only reachable once the fence has actually decoded to an `Image` slot — while `Loading`, a
/// fence produces no `MdItemKind::MermaidFence` Tab item at all, so this also closes the
/// previously-noted "mermaid focus is unreachable without a picker" gap), opens it full-screen with
/// Enter, and confirms the **correct** fence opened by its source ordinal — not by draw order or
/// completion order (the two real decode threads can finish in either order; `fence_ord` is
/// assigned during the synchronous, single-pass source parse, so it's stable regardless). `q`
/// restores the exact scroll/focus that were active before opening the diagram.
#[test]
fn e2e_ui_mermaid_fence_tab_enter_opens_correct_ordinal_fullscreen_q_returns() {
    let dir = sandbox("mermaid_fence_real_async");
    std::fs::write(
        dir.join("d.md"),
        "# Doc\n\n```mermaid\nflowchart TD\nA-->B\n```\n\nmiddle text\n\n```mermaid\nflowchart TD\nC-->D\n```\n\ntail\n",
    )
    .unwrap();
    let root = canon(&dir);

    let mut s = Sim::new(&root).with_media();
    s.select("d.md");
    s.enter(); // both fences kick off a real decode thread (Loading — no image placement yet)
    assert!(
        s.app.md_images().is_empty(),
        "デコード前はまだ画像プレースメント無し"
    );
    assert_eq!(
        s.app.focused_item(),
        None,
        "Loading中はまだ Tab で辿れるアイテムが無いはず"
    );

    // Both fences finish decoding (the two threads' completion order is not guaranteed, but two
    // drains always land both entries in md_image_cache, and fence_ord is source-order regardless).
    s.drain_md_images();
    s.drain_md_images();
    let placements = s.app.md_images();
    assert_eq!(
        placements.len(),
        2,
        "両方の図が画像プレースメントを持つはず"
    );

    // Tab to the first fence (ordinal 0) and open it.
    s.tab();
    assert_eq!(s.app.focused_item(), Some(0));
    let scroll_before = s.app.tab.preview_scroll;
    let focus_before = s.app.focused_item();
    s.enter();
    assert!(
        matches!(
            s.app.tab.preview_kind,
            Some(crate::preview::PreviewKind::MermaidFence(0))
        ),
        "1つ目のTabアイテムは序数0の図を開くはず: {:?}",
        s.app.tab.preview_kind
    );
    s.drain_media(); // the fullscreen re-render (MediaJob::MermaidSrc) is a real thread too
    assert!(
        s.app
            .prepare_image(ratatui::layout::Rect::new(0, 0, 80, 20))
            .is_some(),
        "フルスクリーン描画用のラスタが実際にデコードされているはず"
    );

    s.key('q');
    assert!(
        matches!(
            s.app.tab.preview_kind,
            Some(crate::preview::PreviewKind::Markdown(_))
        ),
        "q でMarkdownへ戻るはず: {:?}",
        s.app.tab.preview_kind
    );
    assert_eq!(
        s.app.tab.preview_scroll, scroll_before,
        "スクロール位置が復元されるはず"
    );
    assert_eq!(
        s.app.focused_item(),
        focus_before,
        "フォーカスが復元されるはず"
    );

    // Tab to the second fence (ordinal 1) and open it — the actual "correct ordinal" regression
    // check: a draw-order/completion-order-based ordinal would also happen to say "1" here by
    // coincidence with exactly two fences, but this exercises the same `open_mermaid_fence` path
    // that re-extracts the fence by its **source** ordinal. `q` above restored focus to item 0, so
    // a single Tab now reaches item 1.
    s.tab();
    assert_eq!(s.app.focused_item(), Some(1));
    s.enter();
    assert!(
        matches!(
            s.app.tab.preview_kind,
            Some(crate::preview::PreviewKind::MermaidFence(1))
        ),
        "2つ目のTabアイテムは序数1の図を開くはず: {:?}",
        s.app.tab.preview_kind
    );
    s.drain_media();
    assert!(s
        .app
        .prepare_image(ratatui::layout::Rect::new(0, 0, 80, 20))
        .is_some());
    std::fs::remove_dir_all(&dir).ok();
}

/// A math expression, decoded and encoded through the real background threads — exactly the path
/// where v0.16.0 shipped two real bugs invisible to every test that existed at the time: (1) the
/// synthetic `math://` key wasn't recognized by the "should this URL be resolved on disk" check, so
/// an encode was never even requested (the reserved row stayed permanently blank); (2) RaTeX paints
/// every glyph pure black, invisible on a dark terminal. Both regressions are pinned here by
/// actually going through `ensure_math_render`'s real-thread branch (previously unreached) and
/// reading real pixel colors — a `Some(url)` synthetic key alone (bug 1) or a `Some(protocol)`
/// alone (bug 2, since black paints a technically-valid protocol) would not have caught either.
#[test]
fn e2e_ui_math_expression_decodes_through_real_worker_and_reserves_image_row() {
    let dir = sandbox("math_real_worker");
    std::fs::write(dir.join("d.md"), "before $E=mc^2$ after\n").unwrap();
    let root = canon(&dir);

    let mut s = Sim::new(&root).with_media();
    s.select("d.md");
    s.enter();
    // While loading: no placement yet, and the literal LaTeX is not shown as plain text (it's on
    // its own reserved "loading" row, not inline with "before"/"after").
    assert!(s.app.md_images().is_empty());
    s.dont_see("E=mc^2");
    s.see("loading");

    s.drain_md_images(); // real background render (ensure_math_render's Some(tx) branch)
    let placements = s.app.md_images();
    assert_eq!(
        placements.len(),
        1,
        "デコード完了後に画像プレースメントが立つはず"
    );
    let (url, cols, rows) = (
        placements[0].url.clone(),
        placements[0].cols,
        placements[0].rows,
    );
    assert!(
        crate::preview::markdown::is_math_url(&url),
        "合成キー math:// が使われているはず: {url}"
    );

    // Bug 1's regression: with the synthetic key unrecognized, `ensure_md_image` would try to
    // resolve `math://...` as a real file path, fail, and never even ask for an encode — this
    // `drain_md_encodes` would then hang (5s timeout → test failure) instead of completing fast.
    s.drain_md_encodes();

    assert!(
        s.app.md_image_proto(&url, cols, rows, 0, rows).is_some(),
        "エンコード完了後は protocol が立つはず"
    );
    // Bug 2's regression: RaTeX's default glyph color is pure black (0,0,0) — invisible on
    // konoma's dark terminal. `recolor` repaints it (default `math_color` #d0d0d0), so real,
    // non-black ink must reach the drawn buffer.
    let visible_ink: Vec<_> = drawn_rgb_fgs(&s.term)
        .into_iter()
        .filter(|&(r, g, b)| r > 20 || g > 20 || b > 20)
        .collect();
    assert!(
        !visible_ink.is_empty(),
        "純黒(不可視)でない実インクが描かれるはず: {:?}",
        drawn_rgb_fgs(&s.term)
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// Failure path (design principle #3): when the real background render fails — a mermaid fence
/// mermaid-rs-renderer can't lay out, and a math expression RaTeX can't parse — the async result
/// still arrives (never silently drops) and must degrade safely to the raw source text: never stuck
/// on "loading…" forever, and never a crash. Both failures go through the very same
/// `md_img_tx`/`apply_md_image` real-thread pipeline as the success-path tests above; only the
/// renderer's `Result` differs.
#[test]
fn e2e_ui_mermaid_and_math_render_failure_degrades_safely_to_raw_source() {
    let dir = sandbox("mermaid_math_failure_real_worker");
    std::fs::write(
        dir.join("d.md"),
        "```mermaid\ndefinitely not a diagram !!!\n```\n\n$\\frac{$\n",
    )
    .unwrap();
    let root = canon(&dir);

    let mut s = Sim::new(&root).with_media();
    s.select("d.md");
    s.enter();
    s.see("loading"); // both start out as loading placeholders

    // Both async renders finish (with failures). Each `drain_md_images` also redraws, which is
    // what turns a `failed` cache entry back into the text fallback (`apply_md_image` invalidates
    // `md_cache` on both success *and* failure, so the very next render re-decorates).
    s.drain_md_images();
    s.drain_md_images();

    assert!(
        s.app.md_images().is_empty(),
        "失敗した図/式は画像プレースメントを持たないはず"
    );
    s.dont_see("loading…");
    // mermaid: mermaid-rs-renderer fails on garbage, and so does the legacy text renderer,
    // degrading all the way to the raw fence source (`fallback_raw`).
    s.see("definitely not a diagram");
    // math: RaTeX fails on unbalanced LaTeX, degrading to the raw `$...$` text.
    s.see("\\frac{");
    std::fs::remove_dir_all(&dir).ok();
}

/// An animated GIF embedded inline in Markdown, decoded through the real background decode thread
/// (`decode_gif_inline`, keeping every frame — the inline-specific budget, separate from the
/// full-screen GIF path) and cycled via `advance_md_gifs_if_due`/`md_gif_poll_timeout` (the inline
/// analog of the full-screen `App::advance_gif_if_due`/`gif_poll_timeout`, previously never
/// exercised through a real decode). Two solid, distinctly-colored frames make the actual frame
/// swap unambiguous from the drawn buffer's pixel colors, not just an internal frame-index bump.
#[test]
fn e2e_ui_inline_gif_animates_through_real_decode_worker() {
    let dir = sandbox("inline_gif_real_worker");
    {
        use image::codecs::gif::GifEncoder;
        use image::{Delay, Frame, Rgba, RgbaImage};
        let out = std::fs::File::create(dir.join("anim.gif")).unwrap();
        let mut enc = GifEncoder::new(out);
        let frames = [Rgba([220, 20, 20, 255]), Rgba([20, 20, 220, 255])] // red, then blue
            .into_iter()
            .map(|color| {
                Frame::from_parts(
                    RgbaImage::from_pixel(120, 80, color),
                    0,
                    0,
                    Delay::from_numer_denom_ms(100, 1),
                )
            });
        enc.encode_frames(frames).unwrap();
    }
    std::fs::write(dir.join("d.md"), "![anim](anim.gif)\n").unwrap();
    let root = canon(&dir);

    let mut s = Sim::new(&root).with_media();
    s.select("d.md");
    s.enter();
    let placements = s.app.md_images();
    assert_eq!(placements.len(), 1);
    let (url, cols, rows) = (
        placements[0].url.clone(),
        placements[0].cols,
        placements[0].rows,
    );

    s.drain_md_images(); // real decode thread: decode_gif_inline finds 2 frames
    assert!(
        s.app.md_gif_poll_timeout().is_some(),
        "アニメGIFは巡回のポーリング待ちを要求するはず"
    );
    s.drain_md_encodes(); // frame 0 (red)'s protocol

    assert!(s.app.md_image_proto(&url, cols, rows, 0, rows).is_some());
    let reds: Vec<_> = drawn_rgb_fgs(&s.term)
        .into_iter()
        .filter(|&(r, g, b)| r > 150 && g < 60 && b < 60)
        .collect();
    assert!(
        !reds.is_empty(),
        "1コマ目(赤)の実ピクセルが描かれるはず: {:?}",
        drawn_rgb_fgs(&s.term)
    );

    assert!(
        !s.app.advance_md_gifs_if_due(),
        "最初の呼び出しはタイマーを起動するだけで、まだ進めないはず"
    );
    std::thread::sleep(std::time::Duration::from_millis(150)); // > the 100ms per-frame delay
    assert!(
        s.app.advance_md_gifs_if_due(),
        "遅延経過後は次フレームへ進むはず"
    );
    s.draw(); // the frame swap invalidated proto_size → the renderer requests a fresh encode
    s.drain_md_encodes(); // frame 1 (blue)'s protocol

    let blues: Vec<_> = drawn_rgb_fgs(&s.term)
        .into_iter()
        .filter(|&(r, g, b)| b > 150 && r < 60 && g < 60)
        .collect();
    assert!(
        !blues.is_empty(),
        "2コマ目(青)へ切り替わった実ピクセルが描かれるはず: {:?}",
        drawn_rgb_fgs(&s.term)
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// RAII guard: pins `NO_PROXY` to `*` for its lifetime and restores whatever the ambient
/// environment had (or clears it) when dropped — including on an early return via a failed
/// `assert!`/panic, so a test failure never leaves the process-global var permanently mutated for
/// whatever else runs in the same test binary afterward. See `NoProxyGuard::new`'s doc for *why*.
struct NoProxyGuard(Option<std::ffi::OsString>);

impl NoProxyGuard {
    /// Isolates the real fetch (`fetch_remote_bytes_capped` in app.rs) from whatever proxy the
    /// *ambient* environment happens to configure. `ureq`'s `Config::default()` — which
    /// `fetch_remote_bytes_capped` goes through unconditionally, it never calls `.proxy(..)` itself
    /// — bakes in `Proxy::try_from_env()` at agent-construction time: if
    /// `ALL_PROXY`/`HTTPS_PROXY`/`HTTP_PROXY` (any casing) is set, a request meant to stay on
    /// loopback is actually routed through that proxy instead of connecting directly, and
    /// connection-refused-in-~1ms turns into "however long the real proxy takes to answer or time
    /// out" — up to `fetch_remote_bytes_capped`'s 20s
    /// `timeout_global`, so a proxied environment would flake or hang the calling test, and worse,
    /// a *reachable* proxy could make a "without ever reaching a real network" test actually reach
    /// one. `NO_PROXY=*` (checked by `ureq` before any of the proxy vars are consulted — see
    /// `Proxy::is_no_proxy`) makes every host bypass the proxy unconditionally, restoring the
    /// loopback-only guarantee regardless of what the environment running the suite has configured.
    fn new() -> Self {
        let saved = std::env::var_os("NO_PROXY");
        std::env::set_var("NO_PROXY", "*");
        Self(saved)
    }
}

impl Drop for NoProxyGuard {
    fn drop(&mut self) {
        match self.0.take() {
            Some(v) => std::env::set_var("NO_PROXY", v),
            None => std::env::remove_var("NO_PROXY"),
        }
    }
}

/// A remote (http/https) inline Markdown image, fetched by the real background thread
/// (`attach_remote_md_loader`) — **without ever reaching a real network**: `http://127.0.0.1:1/...`
/// fails via connection-refused near-instantly (confirmed separately: ~1ms, well under the
/// `recv_timeout`), since nothing can bind privileged port 1, and the request never leaves the
/// loopback interface. Confirms loading → failure → text-placeholder degrade (principle #3)
/// through actual keystrokes and the real fetch thread, not the config-level synchronous-failure
/// path (`[external] remote_images = false`) unit tests already cover. `NoProxyGuard` (above)
/// makes that "never reaching a real network" claim hold regardless of the ambient proxy env.
#[test]
fn e2e_ui_remote_image_fetch_failure_without_network_degrades_to_placeholder() {
    let _no_proxy = NoProxyGuard::new();

    let dir = sandbox("remote_image_no_network");
    std::fs::write(
        dir.join("d.md"),
        "![unreachable](http://127.0.0.1:1/nope.png)\n",
    )
    .unwrap();
    let root = canon(&dir);

    let mut s = Sim::new(&root).with_media();
    s.select("d.md");
    s.enter();
    s.see("loading");
    assert!(s.app.md_images().is_empty());

    s.drain_remote(); // real fetch thread: connection refused instantly, ok=false

    s.dont_see("loading");
    s.see("http://127.0.0.1:1/nope.png"); // degraded to the text-fallback line (image_text_fallback)
    assert!(
        s.app.md_images().is_empty(),
        "失敗した画像はプレースメントを持たないはず"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// The reported bug, end to end through real key input: a document shaped like `rand_chacha`'s
/// README — a footnote definition wrapped onto a second line — plus an ordinary fenced code block.
/// On the released build `y c` was refused for the *whole* document, because the phantom code block
/// the unclosed footnote left behind made the screen and the source scanner disagree about how many
/// code blocks there were. The payload is the one the reporting user was actually trying to copy.
#[test]
fn e2e_yc_copies_code_in_a_document_with_a_multiline_footnote() {
    let payload = "/groundwork <仕様書> <NotionURL> <資料dir>";
    let doc = format!(
        "Intro[^1] text.\n\n```sh\n{payload}\n```\n\n[^1]: [label](\n      https://example.com/x)\n"
    );
    let (mut s, dir) = md_preview(cfg_code_bg_none(), "fnote", &doc);

    // Tab to the code block (the only focusable code item in the document).
    let mut found = false;
    for _ in 0..8 {
        s.tab();
        if s.app.md_focused_code() {
            found = true;
            break;
        }
    }
    assert!(found, "コードブロックにフォーカスできる\n{}", s.screen());
    assert_eq!(
        s.app.focused_code_text().as_deref(),
        Some(payload),
        "`y c` が拒否されず、ファイルどおりの中身を返す"
    );

    // And through the copy leader exactly as a user types it: `y` then `c`.
    s.key('y');
    s.see("code block");
    s.key('c');
    assert!(
        !s.app
            .flash
            .as_deref()
            .unwrap_or_default()
            .contains("couldn't"),
        "コピーが拒否されていない(flash={:?})",
        s.app.flash
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// The other half of the same bug: the phantom code block itself. A wrapped footnote definition used
/// to leave its continuation line behind in the body, where the indentation turned it into an
/// indented code block on screen, and the footnote section showed a truncated, unclosed link. Both
/// the stray block and the broken link must be gone — the continuation belongs to the footnote.
#[test]
fn e2e_multiline_footnote_leaves_no_phantom_code_block() {
    let doc = "Intro[^1] text.\n\n[^1]: [label](\n      https://example.com/x)\n";
    let (s, dir) = md_preview(cfg_code_bg_none(), "phantom", doc);
    let screen = s.screen();
    assert!(
        !screen.contains('▎'),
        "本文に幽霊コードブロックが出ていない\n{screen}"
    );
    // The definition is in the footnotes section as a *working* link. Links render label-only (the
    // URL is collapsed out and recovered into the item), so the proof is the recovered target: on
    // the released build the wrapped URL was stranded in the body and the link was left unclosed,
    // so no target like this existed at all.
    assert!(
        screen.contains("label"),
        "脚注の節に定義が入っている\n{screen}"
    );
    assert_eq!(
        s.app.md_link_targets(),
        vec!["https://example.com/x".to_string()],
        "折り返された URL まで含めて1本のリンクとして復元される\n{screen}"
    );
    // Nothing focusable is a code block any more (there is no code in this document at all).
    let mut s = s;
    for _ in 0..6 {
        s.tab();
        assert!(
            !s.app.md_focused_code(),
            "コードブロックとして巡回対象になっていない\n{}",
            s.screen()
        );
    }
    std::fs::remove_dir_all(&dir).ok();
}

/// `y c` must keep working on a document the pre-passes give a code block the *file* does not have.
/// `<del></del>` alone on a line rewrites to `~~~~`, a tilde fence opener — so the screen shows a
/// code block that scanning the raw file cannot find. This is the same class as the reported
/// multi-line-footnote bug but survives that fix, which is why the copy scanner reads the text the
/// renderer parsed instead of re-reading the file.
#[test]
fn e2e_yc_copies_when_preprocessing_creates_the_code_block() {
    let doc = "before\n\n<del></del>\n\nafter\n";
    let (mut s, dir) = md_preview(cfg_code_bg_none(), "delfence", doc);
    let mut found = false;
    for _ in 0..6 {
        s.tab();
        if s.app.md_focused_code() {
            found = true;
            break;
        }
    }
    assert!(
        found,
        "前処理が作ったコードブロックにフォーカスできる\n{}",
        s.screen()
    );
    assert!(
        s.app.focused_code_text().is_some(),
        "`y c` が拒否されない(生ファイルを読むと 0 個に見えて全拒否になる)"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// Report 1: an inline animated GIF keeps ticking forever after leaving Preview. `md_image_cache` is
/// a flat, path-keyed cache that `enter_preview` only clears when switching to a genuinely
/// *different* file (`if !same_file`, `src/app.rs`) — nothing clears it just because the mode moved
/// back to Tree via `q` (`back_to_tree`, also in `src/app.rs`, never touches it). Since
/// `md_gif_poll_timeout`/`advance_md_gifs_if_due` (both in `src/app/md_media.rs`) scan every cache
/// entry unconditionally, the run loop keeps waking up (≤100ms) and redrawing the *tree* forever even
/// though nothing on screen is animating. Drives the exact same real-thread GIF pipeline as
/// `e2e_ui_inline_gif_animates_through_real_decode_worker`, then leaves Preview with the real `q`
/// keystroke (the same entry point the run loop uses).
#[test]
fn e2e_ui_inline_gif_stops_polling_after_leaving_preview() {
    let dir = sandbox("inline_gif_stops_on_tree");
    {
        use image::codecs::gif::GifEncoder;
        use image::{Delay, Frame, Rgba, RgbaImage};
        let out = std::fs::File::create(dir.join("anim.gif")).unwrap();
        let mut enc = GifEncoder::new(out);
        let frames = [Rgba([220, 20, 20, 255]), Rgba([20, 20, 220, 255])]
            .into_iter()
            .map(|color| {
                Frame::from_parts(
                    RgbaImage::from_pixel(120, 80, color),
                    0,
                    0,
                    Delay::from_numer_denom_ms(100, 1),
                )
            });
        enc.encode_frames(frames).unwrap();
    }
    std::fs::write(dir.join("d.md"), "![anim](anim.gif)\n").unwrap();
    let root = canon(&dir);

    let mut s = Sim::new(&root).with_media();
    s.select("d.md");
    s.enter();
    s.drain_md_images(); // real decode thread: decode_gif_inline finds 2 frames
    s.drain_md_encodes(); // frame 0's protocol
    assert!(
        s.app.md_gif_poll_timeout().is_some(),
        "プレビュー中はアニメGIFのポーリング待ちが立つはず"
    );

    s.key('q'); // Action::PreviewBack -> back_to_tree, the same key the run loop dispatches
    assert_eq!(s.app.tab.mode, Mode::Tree, "q でツリーへ戻るはず");
    assert!(
        s.app.md_gif_poll_timeout().is_none(),
        "ツリーへ戻った後は GIF ポーリングが止まるはず(報告1): {:?}",
        s.app.md_gif_poll_timeout()
    );
    assert!(
        !s.app.advance_md_gifs_if_due(),
        "ツリー表示中はコマ送りも起きないはず(報告1)"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// Report 1, tab-switch companion: switching to a *different* file within the same tab already goes
/// through `enter_preview`'s `if !same_file { self.md_image_cache.clear() }`, which is the one place
/// that genuinely empties the cache. Pins that this path keeps working once `back_to_tree`/mode
/// gating is added elsewhere — a change scoped to `md_gif_poll_timeout`/`advance_md_gifs_if_due`
/// alone must not weaken this existing behavior.
#[test]
fn e2e_ui_inline_gif_stops_polling_after_opening_a_different_file() {
    let dir = sandbox("inline_gif_stops_on_switch");
    {
        use image::codecs::gif::GifEncoder;
        use image::{Delay, Frame, Rgba, RgbaImage};
        let out = std::fs::File::create(dir.join("anim.gif")).unwrap();
        let mut enc = GifEncoder::new(out);
        let frames = [Rgba([220, 20, 20, 255]), Rgba([20, 20, 220, 255])]
            .into_iter()
            .map(|color| {
                Frame::from_parts(
                    RgbaImage::from_pixel(120, 80, color),
                    0,
                    0,
                    Delay::from_numer_denom_ms(100, 1),
                )
            });
        enc.encode_frames(frames).unwrap();
    }
    std::fs::write(dir.join("d.md"), "![anim](anim.gif)\n").unwrap();
    std::fs::write(dir.join("other.txt"), "plain text\n").unwrap();
    let root = canon(&dir);

    let mut s = Sim::new(&root).with_media();
    s.select("d.md");
    s.enter();
    s.drain_md_images();
    s.drain_md_encodes();
    assert!(s.app.md_gif_poll_timeout().is_some());

    s.key('q');
    s.select("other.txt");
    s.enter();
    assert!(
        s.app.md_gif_poll_timeout().is_none(),
        "別ファイルを開けば GIF ポーリングは止まるはず(既存挙動の非退行)"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// Report 2: every math expression in a document spawns its own OS thread the instant it's first
/// scanned (`ensure_math_render`, `src/app/md_media.rs`), with no cap on how many run at once. A
/// document with many distinct `$...$` expressions therefore calls `std::thread::spawn` that many
/// times synchronously inside the very first `ensure_md_cache` pass (`src/app/md_render.rs`), which
/// runs on the UI thread before the first frame can draw. Confirmed by a *state* count — how many
/// entries land in the cache in this one pass, i.e. how many renders were actually kicked off — not a
/// wall-clock timing (`md_image_cache` is private to `app`; the count is read through the
/// `#[cfg(test)]` accessor added alongside this test in `src/app/md_media.rs`).
#[test]
fn e2e_ui_math_render_requests_are_bounded_not_all_at_once() {
    let dir = sandbox("math_render_bounded");
    let exprs: String = (0..80)
        .map(|i| format!("$a{i}$"))
        .collect::<Vec<_>>()
        .join("\n\n");
    std::fs::write(dir.join("d.md"), exprs).unwrap();
    let root = canon(&dir);

    let mut s = Sim::new(&root).with_media();
    s.select("d.md");
    s.enter(); // the very first ensure_md_cache pass — exactly where an unbounded implementation
               // calls std::thread::spawn 80 times in a row before this line returns
    let inserted = s.app.md_image_cache_len();
    assert!(
        inserted < 80,
        "80個の式が一度に全部スレッド起動されてはいけない(報告2): 挿入数={inserted}"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// Report 2, convergence companion: capping concurrency must not silently strand the expressions
/// beyond the cap in `Loading` forever. Drains only the decode channel (placement existence depends
/// on decode+layout, not on the separate protocol-encode step, which — like a real inline image — is
/// only requested for currently *visible* placements and would never arrive for most of these 40
/// off-screen ones, hanging the test) in rounds, mirroring the run loop's per-tick drain, until every
/// expression has a placement. Bounds the number of rounds so a regression that stops re-kicking
/// skipped entries (e.g. capping without ever retrying) fails loudly instead of hanging.
#[test]
fn e2e_ui_math_render_cap_eventually_renders_every_expression() {
    let dir = sandbox("math_render_cap_converges");
    let n = 40;
    let exprs: String = (0..n)
        .map(|i| format!("$a{i}$"))
        .collect::<Vec<_>>()
        .join("\n\n");
    std::fs::write(dir.join("d.md"), exprs).unwrap();
    let root = canon(&dir);

    let mut s = Sim::new(&root).with_media();
    s.select("d.md");
    s.enter();

    // Drain decode results in rounds until every expression is placed, or give up after a generous
    // number of rounds (well beyond ceil(n / cap) for any sane cap).
    let mut rounds = 0;
    while s.app.md_images().len() < n && rounds < n * 4 {
        s.drain_md_images();
        rounds += 1;
    }
    assert_eq!(
        s.app.md_images().len(),
        n,
        "上限を設けても、いずれ全ての式がレンダリングされ切るはず(報告2の修正が新しいスタックを生まない)"
    );
    std::fs::remove_dir_all(&dir).ok();
}

// =============================================================================
// Config-axis x real-keystroke sweep (final pre-release audit): fills the specific gaps found by
// cross-referencing `UI_CONFIG_COVERAGE` (H3, above) against every sub-field of the `theme` entry
// (only `code_bg` had a real e2e contrast; `code_theme`/`code_label_align`/`code_label_bg`/`bg` did
// not) plus a handful of concrete config x keystroke combinations `docs/archive/AUDIT-TESTS-2026-08.md`'s
// own H1/H3 gaps didn't happen to cover: `mermaid_rows` at a value LARGER than the default (only
// smaller was tested), `md_details = "closed"`, `icons` on the alert-header glyph, `y c` code-block
// copy from inside a plain (non-alert) blockquote and an open `<details>`, checkbox toggling in a
// loose list and inside an *open* `<details>` (only *collapsed* was tested), manual scroll keys
// (`j`/`g`/`G`) under both `wrap` values, Markdown-specific state surviving a tab switch, and one
// stress test with every Markdown toggle off at once.
// =============================================================================

/// `theme.code_theme`: the bundled two-face syntax theme used inside a Markdown fenced code block.
/// Only `code_bg` (not `code_theme`) had a real config x keystroke contrast before this — the
/// `UI_CONFIG_COVERAGE` table's `theme` entry names one test for the whole 5-field `ThemeConfig`.
/// Changing the theme name must repaint the keyword's own foreground color (confirmed against real
/// two-face palettes: TwoDark = Zed One Dark's purple keyword vs Dracula's pink).
#[test]
fn e2e_ui_theme_code_theme_changes_markdown_code_block_highlight_colors() {
    let dir = sandbox("ui_code_theme_md");
    std::fs::write(
        dir.join("d.md"),
        "```rust\nfn add(x: i32) -> i32 { x + 1 }\n```\n",
    )
    .unwrap();
    let root = canon(&dir);

    let mut s_default = Sim::new(&root); // theme.code_theme = "TwoDark" (default)
    s_default.select("d.md");
    s_default.enter();
    let fg_default = s_default
        .style_of("fn")
        .and_then(|st| st.fg)
        .expect("既定テーマでも `fn` キーワードは色付けされるはず");

    let mut cfg = Config::default();
    cfg.ui.theme.code_theme = "Dracula".into();
    let mut s_dracula = Sim::with_config(&root, cfg);
    s_dracula.select("d.md");
    s_dracula.enter();
    let fg_dracula = s_dracula
        .style_of("fn")
        .and_then(|st| st.fg)
        .expect("Dracula テーマでも `fn` キーワードは色付けされるはず");

    assert_ne!(
        fg_default, fg_dracula,
        "code_theme を変えると Markdown コードブロック内のキーワード色が変わるはず: \
         TwoDark={fg_default:?} Dracula={fg_dracula:?}"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// `theme.code_label_align`: the language badge (`" rust "`) at the top of a fenced code block is
/// right-aligned by default and left-aligned (right after the gutter) when set to `"left"`.
#[test]
fn e2e_ui_theme_code_label_align_left_vs_right_moves_badge_in_markdown() {
    let dir = sandbox("ui_code_label_align_md");
    std::fs::write(dir.join("d.md"), "```rust\nfn f() {}\n```\n").unwrap();
    let root = canon(&dir);

    let mut s_right = Sim::new(&root); // theme.code_label_align = "right" (default)
    s_right.select("d.md");
    s_right.enter();
    let (_, col_right) = s_right
        .find_text(" rust ")
        .expect("既定=右寄せの言語バッジが見えるはず");

    let mut cfg = Config::default();
    cfg.ui.theme.code_label_align = "left".into();
    let mut s_left = Sim::with_config(&root, cfg);
    s_left.select("d.md");
    s_left.enter();
    let (_, col_left) = s_left
        .find_text(" rust ")
        .expect("left=左寄せの言語バッジが見えるはず");

    assert!(
        col_left < 10,
        "left はガター直後(左端寄り)に描かれるはず: col={col_left}"
    );
    assert!(
        col_right > col_left + 20,
        "right は left より十分右に描かれるはず: right={col_right} left={col_left}"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// `theme.code_label_bg`: `"auto"` (default) gives the language badge a brightened background; an
/// explicit color overrides it; `"none"` (combined with `code_bg="none"` — see the in-test comment)
/// removes it entirely (dim italic text on no background instead).
#[test]
fn e2e_ui_theme_code_label_bg_configurable_in_markdown() {
    let dir = sandbox("ui_code_label_bg_md");
    std::fs::write(dir.join("d.md"), "```rust\nfn f() {}\n```\n").unwrap();
    let root = canon(&dir);

    let mut s_auto = Sim::new(&root); // theme.code_label_bg = "auto" (default)
    s_auto.select("d.md");
    s_auto.enter();
    let bg_auto = s_auto
        .style_of(" rust ")
        .and_then(|st| st.bg)
        .expect("既定(auto)はバッジに背景色があるはず");

    // An explicit color overrides "auto"'s derived brightened shade.
    let mut cfg_red = Config::default();
    cfg_red.ui.theme.code_label_bg = "#ff0000".into();
    let mut s_red = Sim::with_config(&root, cfg_red);
    s_red.select("d.md");
    s_red.enter();
    let bg_red = s_red.style_of(" rust ").and_then(|st| st.bg);
    assert_eq!(
        bg_red,
        Some(ratatui::style::Color::Rgb(0xff, 0, 0)),
        "明示色を設定するとバッジの背景がそれになるはず: auto={bg_auto:?} red={bg_red:?}"
    );
    assert_ne!(Some(bg_auto), bg_red, "auto と明示色は異なる色のはず");

    // `code_label_bg="none"` alone is NOT enough to observe "no background at all": `code_header`'s
    // own final step (`markdown.rs`) applies `line.style(Style::new().bg(bg))` for the WHOLE header
    // line whenever `code_bg` (the surrounding code block's own background, a SEPARATE setting) is
    // still `Some` — a badge span with no `.bg()` of its own then still shows the code block's base
    // color through that line-level style. Isolating "truly no background" requires BOTH set to
    // "none" together.
    let mut cfg_none = Config::default();
    cfg_none.ui.theme.code_label_bg = "none".into();
    cfg_none.ui.theme.code_bg = "none".into();
    let mut s_none = Sim::with_config(&root, cfg_none);
    s_none.select("d.md");
    s_none.enter();
    // Same `Color::Reset`-sentinel caveat as `e2e_ui_theme_bg_paints_the_whole_frame_background`:
    // ratatui's own `Cell::default()` bg is `Some(Color::Reset)`, not a literal `None` — so "no
    // explicit background was painted" is the absence of an `Rgb` variant, not `Option::is_none()`.
    let bg_none = s_none.style_of(" rust ").and_then(|st| st.bg);
    assert!(
        !matches!(bg_none, Some(ratatui::style::Color::Rgb(..))),
        "code_label_bg=none かつ code_bg=none はバッジに明示的な Rgb 背景が無いはず: {bg_none:?}"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// `theme.bg`: paints the WHOLE frame's background (`ui::render`'s own base coat, applied before any
/// widget draws — see that call site's own doc comment), so it reaches a blank cell in the tree view
/// too, not just Markdown — but it is exercised here specifically because this whole section audits
/// every `ThemeConfig` sub-field, and this is the only one with zero prior real e2e contrast.
#[test]
fn e2e_ui_theme_bg_paints_the_whole_frame_background() {
    let dir = sandbox("ui_theme_bg_cfg");
    std::fs::write(dir.join("d.md"), "hello\n").unwrap();
    let root = canon(&dir);

    // Default theme.bg = "none": a blank tree-view cell carries no EXPLICIT (Rgb) background paint
    // — `ratatui::buffer::Cell::default()`'s own bg is `Some(Color::Reset)` (a sentinel meaning "no
    // color set," not a literal `None`), so the real signal that `theme.bg` did NOT run its base
    // coat (`ui::render`'s own `frame.buffer_mut().set_style(area, Style::new().bg(bg))`) is the
    // ABSENCE of an `Rgb` variant, not `Option::is_none()`.
    let s_default = Sim::new(&root);
    let bg_default = s_default.cell_style(3, 85).bg;
    assert!(
        !matches!(bg_default, Some(ratatui::style::Color::Rgb(..))),
        "既定(none)は明示的な Rgb 背景を塗らないはず: {bg_default:?}"
    );

    let mut cfg = Config::default();
    cfg.ui.theme.bg = "#123456".into();
    let s_painted = Sim::with_config(&root, cfg);
    let bg_painted = s_painted.cell_style(3, 85).bg;
    assert_eq!(
        bg_painted,
        Some(ratatui::style::Color::Rgb(0x12, 0x34, 0x56)),
        "theme.bg は画面全体の背景として塗られるはず: {bg_painted:?}"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// `mermaid_rows`: the pre-existing `e2e_ui_mermaid_rows_changes_reserved_diagram_height` only ever
/// compares a SMALLER value (4) against the default (24) — both well under the standard 90x26
/// terminal's own viewport clamp (measured directly: `mermaid_fit_rows` caps at
/// `preview_viewport - 2` = 20 rows there, so anything >= 22 clamps to the exact same 20 as the
/// default and a "larger value" contrast would be unobservable). This uses a taller terminal
/// (`with_config_sized`) so the cap never engages, and confirms a LARGER explicit value actually
/// grows the diagram past the default — the other half of "小さい値/大きい値" this config's own doc
/// comment promises.
#[test]
fn e2e_ui_mermaid_rows_large_value_grows_diagram_beyond_default() {
    let dir = sandbox("ui_mermaid_rows_large_cfg");
    std::fs::write(
        dir.join("d.md"),
        "```mermaid\nflowchart TD\nA-->B-->C-->D-->E-->F-->G-->H\n```\n",
    )
    .unwrap();
    let root = canon(&dir);

    let mut s_default = Sim::with_config_sized(&root, Config::default(), 90, 60).with_picker();
    s_default.select("d.md");
    s_default.enter();
    assert!(
        s_default.app.preview_viewport_for_test() > 26,
        "前提: 広い viewport で mermaid_rows=24(既定) がクランプされないはず: viewport={}",
        s_default.app.preview_viewport_for_test()
    );
    let rows_default = s_default.app.md_images()[0].rows;

    let mut cfg_large = Config::default();
    cfg_large.ui.mermaid_rows = 48;
    let mut s_large = Sim::with_config_sized(&root, cfg_large, 90, 60).with_picker();
    s_large.select("d.md");
    s_large.enter();
    let rows_large = s_large.app.md_images()[0].rows;

    assert!(
        rows_large > rows_default,
        "mermaid_rows=48 は既定24より大きい高さになるはず(十分な viewport下): \
         large={rows_large} default={rows_default}"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// `md_details = "closed"`: forces a `<details>` block collapsed even when its `open` attribute
/// says otherwise (the mirror image of the pre-existing `"open"` override test) — the "auto"/"open"
/// axis was covered, "closed" was not.
#[test]
fn e2e_ui_md_details_closed_config_forces_collapse_despite_open_attribute() {
    let dir = sandbox("ui_md_details_closed_cfg");
    std::fs::write(
        dir.join("d.md"),
        "<details open>\n<summary>Sum</summary>\n\nBODYMARKER\n\n</details>\n",
    )
    .unwrap();
    let root = canon(&dir);

    // "auto" (default) honors the `open` attribute → expanded.
    let mut s_auto = Sim::new(&root);
    s_auto.select("d.md");
    s_auto.enter();
    s_auto.see("BODYMARKER");

    // md_details="closed" forces it collapsed even though the attribute says open.
    let mut cfg = Config::default();
    cfg.ui.md_details = "closed".into();
    let mut s_closed = Sim::with_config(&root, cfg);
    s_closed.select("d.md");
    s_closed.enter();
    s_closed.dont_see("BODYMARKER");
    std::fs::remove_dir_all(&dir).ok();
}

/// `icons`: a GitHub alert's header carries a Nerd Font icon before the label when `icons=true`
/// (`alert_header_line`), shifting the label's own screen column right by the icon-glyph + space
/// width. `icons` was already covered elsewhere via the task-marker glyph, but never against the
/// alert header specifically.
#[test]
fn e2e_ui_icons_alert_header_glyph_shifts_label_column() {
    let dir = sandbox("ui_icons_alert_cfg");
    std::fs::write(dir.join("d.md"), "> [!NOTE]\n> body\n").unwrap();
    let root = canon(&dir);

    let mut cfg_off = Config::default();
    cfg_off.ui.icons = false;
    let mut s_off = Sim::with_config(&root, cfg_off);
    s_off.select("d.md");
    s_off.enter();
    let (_, col_off) = s_off
        .find_text("Note")
        .expect("icons=false でもラベルテキストは見えるはず");

    let mut s_on = Sim::new(&root); // default icons=true
    s_on.select("d.md");
    s_on.enter();
    let (_, col_on) = s_on
        .find_text("Note")
        .expect("icons=true でもラベルテキストは見えるはず");

    assert!(
        col_on > col_off,
        "icons=true は Nerd Font アイコン分だけラベルが右にずれるはず: on={col_on} off={col_off}"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// `y c` inside a PLAIN blockquote (no `[!TYPE]` header — a genuine `Quote`, not an alert). Every
/// existing `y c` coverage is at top level or inside a list item; a quote-nested code block is a
/// distinct code path (`render_code_block_dispatch` reached through `render_quote` → `render_block`,
/// documented as "an intentional superset of what `parser_code_blocks` reports" in
/// `model::BlockKind::CodeBlock`'s own doc comment) that had no keystroke-level coverage at all.
#[test]
fn e2e_md_code_block_inside_plain_blockquote_is_focusable_and_copyable() {
    let dir = sandbox("ui_md_quote_code_cfg");
    std::fs::write(
        dir.join("d.md"),
        "> Intro line.\n>\n> ```rust\n> fn quoted() {}\n> ```\n",
    )
    .unwrap();
    let mut s = Sim::new(&canon(&dir));
    s.select("d.md");
    s.enter();
    s.see("rust"); // the fence's own bar-prefixed language header
    s.tab(); // the only focusable item: the fenced code block
    assert!(
        s.app.md_focused_code(),
        "引用内のコードブロックにフォーカスできるはず"
    );
    assert_eq!(
        s.app.focused_code_text().as_deref(),
        Some("fn quoted() {}"),
        "引用の `>` を含まない生ソースが取得できるはず"
    );
    s.key('y');
    s.key('c');
    assert!(s.app.flash.is_some(), "y c でコピー通知が出るはず");
    std::fs::remove_dir_all(&dir).ok();
}

/// `y c` inside an OPEN `<details>` block — Tab must land on the summary first, then the nested code
/// block, and the copied text must be the bar-free raw source (`render_bar_prefixed_body`'s own
/// isolation, documented at length on that function).
#[test]
fn e2e_md_code_block_inside_open_details_is_focusable_and_copyable() {
    let dir = sandbox("ui_md_details_code_cfg");
    std::fs::write(
        dir.join("d.md"),
        "<details open>\n<summary>Sum</summary>\n\n```python\ndef f():\n    pass\n```\n\n</details>\n",
    )
    .unwrap();
    let mut s = Sim::new(&canon(&dir));
    s.select("d.md");
    s.enter();
    s.see("python"); // the fence's language header, inside the open details body

    // Document order: summary, then the code block.
    s.tab();
    assert!(
        s.app.md_focused_details().is_some(),
        "1番目=summary(details) のはず"
    );
    s.tab();
    assert!(
        s.app.md_focused_code(),
        "2番目=details 内のコードブロックのはず"
    );
    assert_eq!(
        s.app.focused_code_text().as_deref(),
        Some("def f():\n    pass"),
        "details のバー(▏)を含まない生ソースが取得できるはず"
    );
    s.key('y');
    s.key('c');
    assert!(s.app.flash.is_some(), "y c でコピー通知が出るはず");
    std::fs::remove_dir_all(&dir).ok();
}

/// A LOOSE list (a blank line between items) of task checkboxes — the shape that used to panic
/// tui-markdown (`loose_list_task_item_does_not_panic`, v0.8.0), and that the block-model renderer
/// drew with the list marker and the item's own text on two separate rows, leaving the checkbox as
/// literal `[ ]`: no icon, no focus, no toggle. `render_item` now keeps a task item's checkbox on
/// the marker's own row whether the list is tight or loose, so every checkbox behaves the same way
/// regardless of the blank lines around it.
#[test]
fn e2e_task_toggle_works_in_a_loose_list() {
    let dir = sandbox("ui_md_loose_list_should_work_cfg");
    let body = "- [ ] one\n\n- [ ] two\n\n- [ ] three\n";
    std::fs::write(dir.join("d.md"), body).unwrap();
    let mut s = Sim::new(&canon(&dir));
    s.select("d.md");
    s.enter();
    let read = || std::fs::read_to_string(dir.join("d.md")).unwrap();

    s.tab();
    s.key(' ');
    assert!(read().contains("- [x] one"), "1番目: {}", read());
    s.dont_see("couldn't toggle checkbox");

    s.tab();
    s.key(' ');
    assert!(read().contains("- [x] two"), "2番目: {}", read());
    assert!(
        read().contains("- [x] one"),
        "1番目は変化しないまま: {}",
        read()
    );

    s.tab();
    s.key(' ');
    assert!(read().contains("- [x] three"), "3番目: {}", read());
    s.dont_see("couldn't toggle checkbox");
    std::fs::remove_dir_all(&dir).ok();
}

/// Checkbox toggling on a checkbox that's actually visible inside an OPEN `<details>` block — the
/// complement of the pre-existing `e2e_task_toggle_works_with_a_collapsed_details_block` (which only
/// ever proves a *hidden* checkbox is correctly excluded from the scanner's count; this proves a
/// *visible* one inside an open block is correctly included and toggled).
#[test]
fn e2e_task_toggle_works_on_checkbox_inside_an_open_details_block() {
    let dir = sandbox("ui_md_details_open_task_cfg");
    std::fs::write(
        dir.join("d.md"),
        "<details open>\n<summary>Sum</summary>\n\n- [ ] inside open details\n\n</details>\n",
    )
    .unwrap();
    let mut s = Sim::with_config(&canon(&dir), Config::default());
    s.select("d.md");
    s.enter();
    let read = || std::fs::read_to_string(dir.join("d.md")).unwrap();

    // 1st focusable: the summary. 2nd: the visible checkbox inside the open body.
    s.tab();
    assert!(s.app.md_focused_details().is_some(), "1番目=summary のはず");
    s.tab();
    assert!(
        s.app.md_focused_task(),
        "2番目=開いた details 内の可視タスクのはず"
    );
    s.key(' ');
    assert!(
        read().contains("- [x] inside open details"),
        "開いた details 内のタスクがトグルされるはず: {}",
        read()
    );
    s.dont_see("couldn't toggle checkbox");
    std::fs::remove_dir_all(&dir).ok();
}

/// A checkbox inside a plain block quote (`> - [ ] x`, no `[!TYPE]` header). It used to be drawn as
/// literal text and skipped by `Tab` entirely, because `render_quote` rendered its children into the
/// surrounding `Writer` with the `>` prefix already installed, so the top-level decoration pass never
/// saw a line whose own text began with a bullet. `render_quote` now isolates, decorates, then
/// prefixes - the same order an alert's and a `<details>`'s body already used - so a quoted checkbox
/// behaves exactly like one anywhere else.
#[test]
fn e2e_task_toggle_works_inside_a_plain_blockquote() {
    let dir = sandbox("ui_md_quote_task_should_work_cfg");
    std::fs::write(dir.join("d.md"), "> - [ ] quoted task\n").unwrap();
    let mut s = Sim::with_config(&canon(&dir), Config::default());
    s.select("d.md");
    s.enter();
    let read = || std::fs::read_to_string(dir.join("d.md")).unwrap();
    s.tab();
    assert!(
        s.app.md_focused_task(),
        "引用内のタスクにフォーカスできるはず"
    );
    s.key(' ');
    assert!(
        read().contains("> - [x] quoted task"),
        "引用内のタスクがトグルされるはず: {}",
        read()
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// Regression: an **ordered**-list task item (`"1. [ ] x"`) used to be pushed into
/// `RenderOut::tasks` unconditionally, the same as an unordered one — even though
/// `decorate_extras`'s own `replace_task_checkbox` never decorates one (`task_prefix_state`'s own
/// "first byte must be `-`/`*`/`+`" contract; a `push_item_marker`'s own ordered-item marker line
/// always starts `"N. "`, a digit, never a bullet — see either's own doc comment). A document
/// mixing an ordered task list *ahead of* an unordered one therefore recorded a "ghost" entry for
/// every invisible ordered checkbox, shifting every later, real, on-screen checkbox's own ordinal
/// down by one in `RenderOut::tasks` — `build_md_items`'s "Nth sentinel found on screen ==
/// `task_marks[N]`" pairing (see that field's own doc comment) then handed the **visible**
/// checkbox's own Tab-focus/`Space`-toggle the **invisible** ordered item's own byte offset
/// instead: pressing `Space` on the one checkbox actually on screen silently edited a *different*
/// line of the file. Found live on exactly this shape (2026-08), reproduced here byte-for-byte:
/// `Space` on the sole focusable checkbox must flip line 4 (`"- [ ] visible bullet"`), never line
/// 1 or 2 (the two ordered, undrawn ones) — this is the "toggle writes to the wrong line while the
/// checkbox count still agrees" class principle #3 exists to rule out, one level more subtle than a
/// pure count mismatch (which the existing safe-refusal path already catches): here the counts
/// looked fine (`task_marks.len() == 1` in production before this fix too, `render_item` never
/// having produced more than the one real GFM task on this exact document — the mis-pairing shows
/// up only once a *second*, undrawn task exists earlier in the file to steal the first slot), so a
/// naive "does the write happen at all" check would not have caught it — the position itself has to
/// be checked. The general, corpus-wide form of this check (every case in the golden-snapshot
/// corpus, not just this one document) lives in `app/tests.rs`'s own
/// `model_render_records_exactly_the_sentinels_it_draws_across_the_corpus`.
#[test]
fn e2e_ordered_list_ghost_task_does_not_steal_the_visible_checkboxs_toggle() {
    let dir = sandbox("md_ordered_ghost_task");
    let body = "1. [ ] ordered invisible\n2. [ ] ordered invisible two\n\n- [ ] visible bullet\n";
    std::fs::write(dir.join("mix.md"), body).unwrap();
    let mut s = Sim::new(&canon(&dir));
    s.select("mix.md");
    s.enter();

    // Exactly one checkbox is ever drawn/focusable — GFM never decorates an ordered item's own
    // checkbox, so both "1. [ ]"/"2. [ ]" lines stay literal, undrawn text (see this test's own
    // doc comment) — proven directly against `md_items`, not inferred from Tab behavior alone.
    assert_eq!(
        s.app.md_task_item_count_for_test(),
        1,
        "画面上のチェックボックスは可視の1個だけのはず"
    );

    s.tab(); // the only Tab stop for a checkbox: the visible bullet's own
    assert!(
        s.app.md_focused_task(),
        "Tab は唯一の可視チェックボックス(visible bullet)にフォーカスするはず"
    );
    s.key(' ');

    let src = std::fs::read_to_string(dir.join("mix.md")).unwrap();
    assert!(
        src.contains("- [x] visible bullet"),
        "Space はフォーカスした可視チェックボックス(4行目)をトグルするはず: {src}"
    );
    assert!(
        src.contains("1. [ ] ordered invisible\n"),
        "1行目(不可視の順序付きタスク)は変化しないはず: {src}"
    );
    assert!(
        src.contains("2. [ ] ordered invisible two\n"),
        "2行目(不可視の順序付きタスク)は変化しないはず: {src}"
    );
    s.dont_see("couldn't toggle checkbox");
    std::fs::remove_dir_all(&dir).ok();
}

/// The `Details` analog of the ordered-list-task regression above: two independent, sibling
/// top-level `<details>` blocks (both closed by default), toggled one at a time through the real
/// Tab/Space path, confirming the **first** one's own toggle only ever opens the **first** one's own
/// body — never the second's — and vice versa. Unlike a task/code-block/mermaid record,
/// `Details`'s own Tab-focus ordinal has no separate accumulator to have desynced from (it is
/// computed purely self-referentially — see `app/tests.rs`'s own
/// `model_render_records_exactly_the_sentinels_it_draws_across_the_corpus` doc comment) — the risk
/// here is the *other* sequence, `render_details_from_model`'s own `next_details_open` counter,
/// landing on the wrong slot of `details_states` for one of these two blocks; this proves it does
/// not, end to end, via the exact same `Tab`/`Space` keys a reader would press.
#[test]
fn e2e_two_sibling_details_blocks_toggle_independently() {
    let dir = sandbox("md_two_sibling_details");
    let body = "<details>\n<summary>One</summary>\n\nBODY-ONE\n\n</details>\n\n\
                <details>\n<summary>Two</summary>\n\nBODY-TWO\n\n</details>\n";
    std::fs::write(dir.join("d.md"), body).unwrap();
    let mut s = Sim::new(&canon(&dir));
    s.select("d.md");
    s.enter();
    s.dont_see("BODY-ONE");
    s.dont_see("BODY-TWO");

    s.tab(); // 1st Tab stop: "One"'s own summary (ordinal 0)
    assert!(
        s.app.md_focused_details().is_some(),
        "1つ目は details のはず"
    );
    s.key(' '); // open ordinal 0
    s.see("BODY-ONE");
    s.dont_see("BODY-TWO");

    s.tab(); // 2nd Tab stop: "Two"'s own summary (ordinal 1) — count of summaries is unchanged
             // by One's own body becoming visible, so this is still the very next item.
    assert!(
        s.app.md_focused_details().is_some(),
        "2つ目も details のはず"
    );
    s.key(' '); // open ordinal 1
    s.see("BODY-ONE"); // unaffected by opening the second block
    s.see("BODY-TWO");
    std::fs::remove_dir_all(&dir).ok();
}

/// Manual scroll keys (`j`/`k`/`g`/`G` — `Motion::Down/Up/Top/Bottom` on `Surface::PreviewText`,
/// which moves `preview_scroll` directly for a non-windowed decorated Markdown document) under both
/// `wrap` values. Existing coverage only ever drove scrolling indirectly via Tab-follow
/// (`e2e_md_wrapped_focus_follows_offscreen_item`); this drives it directly, the way a reader
/// actually pages through a long document without touching any link/task/code-block focus target.
#[test]
fn e2e_md_manual_scroll_moves_preview_scroll_wrap_on_and_off() {
    let mut src = String::from("# Doc\n\n");
    for i in 0..80 {
        src.push_str(&format!("paragraph line {i}\n\n"));
    }
    for (label, wrap) in [("wrap_on", true), ("wrap_off", false)] {
        let dir = sandbox(&format!("ui_md_manual_scroll_{label}"));
        std::fs::write(dir.join("d.md"), &src).unwrap();
        let mut cfg = Config::default();
        cfg.ui.wrap = wrap;
        let mut s = Sim::with_config(&canon(&dir), cfg);
        s.select("d.md");
        s.enter();
        assert_eq!(s.app.tab.preview_scroll, 0, "[{label}] 初期スクロール0");

        for _ in 0..10 {
            s.key('j');
        }
        let after_j = s.app.tab.preview_scroll;
        assert!(
            after_j > 0,
            "[{label}] j 連打でスクロールが進むはず: scroll={after_j}"
        );

        s.key('G');
        let bottom = s.app.tab.preview_scroll;
        assert!(
            bottom > after_j,
            "[{label}] G で j 連打後よりさらに末尾までスクロールするはず: \
             after_j={after_j} bottom={bottom}"
        );

        s.key('g');
        assert_eq!(s.app.tab.preview_scroll, 0, "[{label}] g で先頭へ戻るはず");
        std::fs::remove_dir_all(&dir).ok();
    }
}

/// Markdown-specific preview state (decorated-view scroll position, raw-source toggle) must survive
/// opening a new tab and switching back — `t`/`[` are declared `global` (keymap.rs), so they must
/// work while a Markdown preview (not the tree) is showing, and per-tab state must round-trip
/// through `TabState`/`PerTab`'s own snapshot/load. Distinct from
/// `e2e_tab_switch_reloads_tree_from_disk` (which only exercises the tree) and from session-restore
/// (which is a fresh-process concept, not a same-process tab switch).
///
/// The scroll-position and raw-toggle checks are two SEPARATE round trips (not one continuous
/// sequence): confirmed directly (a scratch probe) that pressing `R` itself resets
/// `tab.preview_scroll` to 0 as part of switching into raw/windowed mode — raw mode tracks its own
/// view position through a different mechanism (`FileWindow`), not `preview_scroll` — so comparing a
/// decorated-mode scroll value against what `preview_scroll` reads AFTER a raw-mode toggle would
/// conflate "the mode switch reset it" with "the tab switch lost it." Testing them independently
/// isolates what a tab switch actually does to each one.
#[test]
fn e2e_md_state_preserved_across_tab_switch() {
    let dir = sandbox("ui_md_tab_switch_state");
    let mut src = String::from("# Doc\n\n");
    for i in 0..60 {
        src.push_str(&format!("paragraph line {i}\n\n"));
    }
    std::fs::write(dir.join("d.md"), &src).unwrap();
    let mut s = Sim::new(&canon(&dir));
    s.select("d.md");
    s.enter();

    // 1) Decorated-view scroll position survives a tab switch.
    for _ in 0..10 {
        s.key('j');
    }
    let scroll_before = s.app.tab.preview_scroll;
    assert!(scroll_before > 0, "前提: スクロールが進んでいる");
    assert!(!s.app.is_raw_source(), "前提: まだ装飾表示のまま");

    s.key('t'); // open a fresh (blank) tab
    assert_eq!(s.app.tab_count(), 2, "t でタブが増える");
    s.key('['); // switch back to the original tab
    assert_eq!(s.app.active_tab_index(), 0, "[ で元のタブへ戻る");
    assert_eq!(
        s.app.tab.preview_scroll, scroll_before,
        "装飾 Markdown のスクロール位置がタブ切替後も保持されるはず: \
         before={scroll_before} after={}",
        s.app.tab.preview_scroll
    );

    // 2) The raw/decorated MODE flag itself survives a further tab switch (checked separately from
    // scroll — see this test's own doc comment for why the two can't share one round trip).
    s.key('R');
    assert!(s.app.is_raw_source(), "前提: R で raw に切替わる");
    s.key(']'); // to the tab opened above
    assert_eq!(s.app.active_tab_index(), 1, "] で新規タブへ");
    s.key('['); // back to the original (raw) tab
    assert_eq!(s.app.active_tab_index(), 0, "[ で元のタブへ戻る");
    assert!(
        s.app.is_raw_source(),
        "raw 表示モードがタブ切替後も保持されるはず"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// Every Markdown decoration toggle off at once, on one document that exercises all of them —
/// front-matter deliberately excluded (`md_frontmatter=false` does NOT restore the literal `---`
/// text: `ParseOptions::ENABLE_YAML_STYLE_METADATA_BLOCKS` is unconditionally on in tui-markdown
/// itself, see `e2e_markdown_front_matter_off_leaves_body_intact`'s own comment, so it would need a
/// style-level assertion, not a text one, and is already covered on its own). A robustness sweep:
/// disabling every toggle together must never crash and must leave every marker exactly as literal
/// text as disabling each one individually already proves — no toggle should start firing only when
/// combined with the others (e.g. an interaction between `md_alerts=false` and `md_inline_html=false`
/// changing how the alert's `[!NOTE]` marker or its body is masked).
#[test]
fn e2e_md_all_toggles_off_renders_without_crashing() {
    let dir = sandbox("ui_md_all_off_cfg");
    let body = concat!(
        "> [!NOTE]\n> alert body\n\n",
        "text[^1] with :rocket: and https://x.example and H<sub>2</sub>O\n\n",
        "[^1]: a footnote\n\n",
        "- [ ] task one\n\n",
        "```mermaid\nflowchart TD\nA-->B\n```\n\n",
        "$E=mc^2$\n",
    );
    std::fs::write(dir.join("d.md"), body).unwrap();
    let mut cfg = Config::default();
    cfg.ui.md_alerts = false;
    cfg.ui.md_autolink = false;
    cfg.ui.md_emoji = false;
    cfg.ui.md_footnotes = false;
    cfg.ui.md_inline_html = false;
    cfg.ui.mermaid = "text".into();
    cfg.ui.math = "text".into();
    cfg.ui.icons = false;
    cfg.ui.wrap = false;
    let mut s = Sim::with_config(&canon(&dir), cfg);
    s.select("d.md");
    s.enter();

    s.see("[!NOTE]"); // alert marker left literal (no callout box)
    assert!(
        s.app.md_link_targets().is_empty(),
        "autolink off: リンク無し"
    );
    s.see(":rocket:"); // emoji shortcode left literal
    s.see("https://x.example"); // bare URL not linked
    s.see("[^1]"); // footnote marker left literal
    s.see("H2O"); // <sub> tags stripped, "2" stays literal (not superscript)
    s.see("[ ] task one"); // icons=false: ASCII checkbox
    s.see("$E=mc^2$"); // math left as raw LaTeX text
    s.dont_see("can not preview"); // no crash-fallback banner anywhere
    std::fs::remove_dir_all(&dir).ok();
}
