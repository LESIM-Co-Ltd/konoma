// The entry point for ratatui rendering. Draws full-screen according to the current mode.
// Following the "mode transition" design principle, there is no split display between Tree and
// Preview. (The bottom status line is overall chrome, not a content split — the body area stays
// full-screen per mode.)

pub mod bookmarks;
pub mod canvas;
pub mod dialog;
pub mod help;
pub mod icons;
pub mod info;
mod list;
pub mod outline;
pub mod preview;
pub(crate) mod project;
pub mod status;
pub mod tab_list;
pub mod tabbar;
pub mod table;
pub mod tree;

use ratatui::layout::{Constraint, Layout};
use ratatui::style::Style;
use ratatui::Frame;

use crate::app::{App, Mode, StatusbarLayout};

/// Convert the core's small color vocabulary at the host's drawing boundary.
pub(crate) fn vcs_color(color: konoma_vcs_core::style::VcsColor) -> ratatui::style::Color {
    use konoma_vcs_core::style::VcsColor;
    use ratatui::style::Color;
    match color {
        VcsColor::Reset => Color::Reset,
        VcsColor::Red => Color::Red,
        VcsColor::Green => Color::Green,
        VcsColor::Yellow => Color::Yellow,
        VcsColor::Blue => Color::Blue,
        VcsColor::Magenta => Color::Magenta,
        VcsColor::Cyan => Color::Cyan,
        VcsColor::White => Color::White,
        VcsColor::LightRed => Color::LightRed,
        VcsColor::LightGreen => Color::LightGreen,
        VcsColor::LightYellow => Color::LightYellow,
    }
}

/// Space for a notice in a full-width status row, shared by startup and rendering.
pub(crate) fn notice_footer_width(app: &App, columns: usize) -> usize {
    if StatusbarLayout::parse(&app.cfg.ui.statusbar) == StatusbarLayout::Split {
        columns
    } else {
        status::combined_footer_width(app, columns)
    }
}

/// Full-screen rendering per mode + status chrome (placement via `ui.statusbar`).
pub fn render(frame: &mut Frame, app: &mut App) {
    app.begin_frame();
    // Re-validate git status if the root changed or a re-validation was requested (FR-7). Never
    // blocks (async kick + reuse of the previous frame's data): see `App::refresh_git_if_needed`.
    //
    // This must run **once per frame, unconditionally, before anything reads git_branch/
    // worktree_origin/git_status** — not only when `tree::render` happens to run. The Git
    // full-screen views (changes hub / log / graph / branches / worktrees / detail) and Preview
    // mode bypass `tree::render` entirely (see the content dispatch below), so a call placed only
    // inside `tree::render` never re-verifies while those views are showing. That let a stale
    // `git_branch`/`worktree_origin` from a previously active tab linger on-screen after switching
    // to a different repo's tab (the changes hub's file *list* is per-tab state and did update
    // correctly, but the branch-name chip in its title, and the persistent `WT <origin>` chip,
    // are process-wide caches that only this call refreshes).
    app.refresh_git_if_needed();
    // Move detection for the inline image overlay (across frames): reset the reading at the start, compare at the end.
    app.begin_md_overlay_frame();
    // If config `ui.theme.bg` is set, paint the whole background first. A text span's bg=None does
    // not overwrite an existing cell background, so this base coat becomes the overall background
    // as-is (leaves it unpainted when it's "none" — the terminal default = stays transparent).
    if let Some(bg) = app.cfg.ui.theme.bg() {
        let area = frame.area();
        frame.buffer_mut().set_style(area, Style::new().bg(bg));
    }

    // Whether to show the tab bar: always=always on / hidden=never / auto=when there are 2+ tabs.
    let show_tabs = match app.cfg.ui.tabbar.as_str() {
        "always" => true,
        "hidden" => false,
        _ => app.tab_count() > 1,
    };
    let layout = StatusbarLayout::parse(&app.cfg.ui.statusbar);
    let split = layout == StatusbarLayout::Split;

    // The top row (header) exists always under split, and only when tabs exist under bottom. The bottom row always exists.
    let top_present = split || show_tabs;
    let mut constraints = Vec::with_capacity(3);
    if top_present {
        constraints.push(Constraint::Length(1));
    }
    constraints.push(Constraint::Min(0));
    constraints.push(Constraint::Length(1));
    let areas = Layout::vertical(constraints).split(frame.area());

    let mut idx = 0;
    let top = if top_present {
        let a = areas[idx];
        idx += 1;
        Some(a)
    } else {
        None
    };
    let content = areas[idx];
    idx += 1;
    let bottom = areas[idx];

    // The content area holds the VCS project screens when one is open, else the tree or preview.
    if app.project_active() {
        project::render(frame, app, content);
    } else {
        match app.tab.mode {
            Mode::Tree => tree::render(frame, app, content),
            Mode::Preview => preview::render(frame, app, content),
        }
    }

    // The tab bar and the status rows are drawn **after** the content: what they say depends on
    // state the content's layout pass settles (a presentation's slide chip and `J/K` hint read the
    // freshly built heading positions and the clamped scroll), so drawn first they would trail the
    // body by one frame. The areas never overlap, so the order is invisible otherwise.
    if let Some(top) = top {
        // Tabs on the left, context on the right (split). To let them coexist on the same row, the
        // tab bar is given a width with the context display's share excluded (so the overflow
        // visible-window plan doesn't encroach on the right side).
        if show_tabs {
            let mut tab_area = top;
            if split {
                let ctx = status::context_width(app).saturating_add(1); // 1 = spacing
                tab_area.width = top.width.saturating_sub(ctx);
            }
            tabbar::render(frame, app, tab_area);
        }
        if split {
            status::render_context(frame, app, top);
        }
    }
    // Bottom row: split=key hints only / bottom=context + hints combined.
    // The vertical layout preserves the frame's width; use the actual row for later resizes.
    let footer_width = notice_footer_width(app, usize::from(bottom.width));
    app.refit_notice_flash(footer_width);
    let project_dialog = dialog::project_presentation(app);
    if split {
        status::render_footer_with_project_dialog(frame, app, bottom, project_dialog.as_ref());
    } else {
        status::render_combined_with_project_dialog(frame, app, bottom, project_dialog.as_ref());
    }

    // The bookmark list overlay also sits on top.
    if app.is_bookmark_list() {
        bookmarks::render(frame, app, frame.area());
    }
    // The tab list overlay (`T`).
    if app.is_tab_list() {
        tab_list::render(frame, app, frame.area());
    }
    // The heading outline overlay (`o`).
    // A project screen covers the screen under it, so a state left open there is not drawn.
    if app.is_outline() && !app.project_screen_covers() {
        outline::render(frame, app, frame.area());
    }
    // The file info popup.
    if app.is_info() {
        info::render(frame, app, frame.area());
    }
    // The table cell full-text popup (`Enter`).
    if app.is_table_cell_open() && !app.project_screen_covers() {
        table::render_cell_popup(frame, app, frame.area());
    }
    // `?` help sits above every other overlay (bookmarks, tab list, outline, info, cell popup),
    // matching its key priority (`frontmost_layer`: dialogs, then help). Keep it right before the dialog.
    if app.show_help {
        help::render(frame, app, frame.area());
    }
    // A confirm/input dialog has top priority (it also intercepts keys), so it sits on top of everything.
    if app.is_dialog() || app.project_dialog.is_some() {
        dialog::render_with_project_dialog(frame, app, frame.area(), project_dialog.as_ref());
    }
    // If an inline image moved/disappeared since the previous frame, the run loop fully redraws the
    // terminal once to clean up kitty placeholder debris (the old ID's colored row).
    app.finish_md_overlay_frame();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use crate::config::Config;
    use crate::test_support::unique_tmp;
    use ratatui::backend::TestBackend;
    use ratatui::style::Color;
    use ratatui::Terminal;

    #[test]
    fn preview_line_start_end_horizontal_jump() {
        // While not wrapping, `$` jumps straight to line-end (END) and `0` jumps straight to line-start (START).
        let dir = unique_tmp("konoma_hscroll_jump_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let long = format!("START{}END", "x".repeat(200));
        std::fs::write(dir.join("a.txt"), &long).unwrap();

        let mut cfg = Config::default();
        cfg.ui.wrap = false; // enable horizontal scroll
        let mut app = App::new(dir.to_path_buf(), cfg).unwrap();
        app.rebuild_tree().unwrap();
        app.tree_descend().unwrap(); // a.txt is selected → into Preview
        assert!(
            matches!(app.tab.mode, crate::app::Mode::Preview),
            "Preview 遷移"
        );

        let dump = |app: &mut App| -> String {
            let mut term = Terminal::new(TestBackend::new(40, 8)).unwrap();
            term.draw(|f| render(f, app)).unwrap();
            let buf = term.backend().buffer();
            buf.content().iter().map(|c| c.symbol()).collect()
        };
        // Default (caret at line start) = line start: START is visible, the far-off END is off-screen.
        let s0 = dump(&mut app);
        assert!(s0.contains("START"), "行頭に START が無い");
        assert!(!s0.contains("END"), "行頭で END が見えてはいけない");
        // `$` to the last character: the view follows and END becomes visible (START goes off-screen).
        app.preview_col_end();
        let se = dump(&mut app);
        assert!(se.contains("END"), "$ で行末(END)が見えない");
        assert!(!se.contains("START"), "行末で START が見えてはいけない");
        // `0` returns to line start.
        app.preview_col_home();
        let sh = dump(&mut app);
        assert!(
            sh.contains("START") && !sh.contains("END"),
            "0 で行頭へ戻らない"
        );
    }

    #[test]
    fn global_bg_fills_buffer() {
        let dir = unique_tmp("konoma_ui_bg_test");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), b"hi").unwrap();

        let mut cfg = Config::default();
        cfg.ui.theme.bg = "#102030".into();
        let mut app = App::new(dir.to_path_buf(), cfg).unwrap();

        let mut term = Terminal::new(TestBackend::new(24, 8)).unwrap();
        term.draw(|f| render(f, &mut app)).unwrap();

        let buf = term.backend().buffer();
        let target = Color::Rgb(16, 32, 48);
        // Text cells also keep the base coat underneath, so most cells should have the configured background.
        let total = buf.content().len();
        let hit = buf.content().iter().filter(|c| c.bg == target).count();
        assert!(
            hit * 5 >= total * 4,
            "全体背景が塗られていない: {hit}/{total} セルのみ"
        );
    }

    fn row_text(app: &mut App, w: u16, h: u16, row: u16) -> String {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| render(f, app)).unwrap();
        let buf = term.backend().buffer();
        (0..w)
            .map(|x| buf.cell((x, row)).unwrap().symbol().to_string())
            .collect()
    }

    #[test]
    fn statusbar_split_puts_context_top_hints_bottom() {
        let dir = unique_tmp("konoma_chrome_split_test");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), b"x").unwrap();
        let mut app = App::new(dir.to_path_buf(), Config::default()).unwrap(); // default = split
        let (w, h) = (70, 6);
        // Context (TREE/path) on top, key hints on the bottom.
        assert!(row_text(&mut app, w, h, 0).contains("TREE"), "上にモード");
        assert!(row_text(&mut app, w, h, 0).contains("path:"), "上にパス");
        let bottom = row_text(&mut app, w, h, h - 1);
        assert!(bottom.contains("j/k:move"), "下にヒント: {bottom}");
        assert!(!bottom.contains("TREE"), "下にモードは出さない: {bottom}");
    }

    #[test]
    fn statusbar_bottom_puts_everything_on_bottom() {
        let dir = unique_tmp("konoma_chrome_bottom_test");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), b"x").unwrap();
        let mut cfg = Config::default();
        cfg.ui.statusbar = "bottom".into();
        let mut app = App::new(dir.to_path_buf(), cfg).unwrap();
        let (w, h) = (70, 6);
        let bottom = row_text(&mut app, w, h, h - 1);
        assert!(bottom.contains("TREE"), "下にモード: {bottom}");
        assert!(bottom.contains("j/k:move"), "下にヒント: {bottom}");
        // The top row shows no context (there's no tab bar either, since it's 1 tab).
        assert!(!row_text(&mut app, w, h, 0).contains("TREE"));
    }

    #[test]
    fn lang_jp_localizes_chrome_and_help() {
        let dir = unique_tmp("konoma_lang_jp_test");
        std::fs::create_dir_all(&dir).unwrap();
        let mut cfg = Config::default();
        cfg.ui.lang = "jp".into();
        cfg.ui.statusbar = "bottom".into(); // consolidate onto the bottom row, easier to inspect
        let mut app = App::new(dir.to_path_buf(), cfg).unwrap();
        let buf = |app: &mut App| -> String {
            let mut term = Terminal::new(TestBackend::new(72, 26)).unwrap();
            term.draw(|f| render(f, app)).unwrap();
            term.backend()
                .buffer()
                .content()
                .iter()
                .map(|c| c.symbol())
                .collect::<String>()
        };
        // Japanese chip/hints on the bottom row. Since CJK is split across cells by the test
        // backend, check for a single character ("ツ"=tree, "移"=move). English TREE/jk:move do
        // not appear.
        let s = buf(&mut app);
        assert!(s.contains('ツ'), "日本語チップが無い");
        assert!(s.contains('移'), "日本語ヒントが無い");
        assert!(
            !s.contains("TREE") && !s.contains("j/k:move"),
            "英語が残る: {s}"
        );
    }

    #[cfg(feature = "git")]
    #[test]
    fn tree_shows_git_status_marker() {
        let dir = unique_tmp("konoma_git_marker_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        konoma_vcs_core::git2::Repository::init(&dir).unwrap();
        std::fs::write(dir.join("foo.txt"), b"x").unwrap(); // untracked → 'U'
        let mut app = App::new(dir.canonicalize().unwrap(), Config::default()).unwrap();
        let mut term = Terminal::new(TestBackend::new(40, 8)).unwrap();
        term.draw(|f| render(f, &mut app)).unwrap();
        let buf = term.backend().buffer();
        // There's a cell where the 'U' marker appears in LightGreen (the untracked color).
        let found = buf
            .content()
            .iter()
            .any(|c| c.symbol() == "U" && c.fg == ratatui::style::Color::LightGreen);
        assert!(found, "未追跡 'U' マーカーが無い");
    }

    #[test]
    fn tree_visible_range_keeps_selection_onscreen() {
        // C3: guarantees that the optimization of turning only the visible range into Lines always
        // keeps the selected row on-screen. With more entries than the viewport, select the last
        // one — the last name gets drawn and the first name is off-screen (= not drawn).
        let dir = unique_tmp("konoma_tree_visrange_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("aaa_first.txt"), b"x").unwrap();
        for i in 0..30 {
            std::fs::write(dir.join(format!("m{i:02}.txt")), b"x").unwrap();
        }
        std::fs::write(dir.join("zzz_last.txt"), b"x").unwrap();
        let mut app = App::new(dir.canonicalize().unwrap(), Config::default()).unwrap();
        // Ascending by name, so zzz_last is last. Select the last one → it lands at the bottom edge of the screen.
        app.tab.selected = app.tab.entries.len() - 1;
        assert!(
            app.tab.entries[app.tab.selected]
                .path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap()
                .starts_with("zzz_last"),
            "末尾が zzz_last でない(ソート前提が崩れた)"
        );

        let mut term = Terminal::new(TestBackend::new(40, 8)).unwrap(); // 6 visible rows
        term.draw(|f| render(f, &mut app)).unwrap();
        let s: String = term
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(
            s.contains("zzz_last"),
            "選択した末尾行が画面に出ない: 可視範囲が選択を外した"
        );
        assert!(
            !s.contains("aaa_first"),
            "先頭行は画面外のはずなのに描画された(スクロールが効いていない)"
        );
    }

    #[cfg(feature = "git")]
    #[test]
    fn tree_title_shows_branch() {
        let dir = unique_tmp("konoma_branch_title_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        konoma_vcs_core::git2::Repository::init(&dir).unwrap();
        let mut app = App::new(dir.canonicalize().unwrap(), Config::default()).unwrap();
        app.refresh_git_if_needed();
        assert!(app.git_branch().is_some(), "ブランチ名が取得できない");
        // The branch symbol ⎇ appears in the title border.
        let mut term = Terminal::new(TestBackend::new(50, 6)).unwrap();
        term.draw(|f| render(f, &mut app)).unwrap();
        let s: String = term
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(s.contains('⎇'), "タイトルにブランチ記号が無い");
    }

    #[test]
    fn help_overlay_renders_when_shown() {
        let dir = unique_tmp("konoma_help_render_test");
        std::fs::create_dir_all(&dir).unwrap();
        let mut app = App::new(dir.to_path_buf(), Config::default()).unwrap();
        let buf_text = |app: &mut App| -> String {
            // Verify at a height where every help section fits without wrapping (so a section
            // being added doesn't push it off-screen; on a real small terminal, scrolling (j/k) to
            // see the bottom is the correct behavior).
            // Bumped 50→54 for the Agent Watch (C/n·N/F) 3-line addition. Bumped 54→55 for the
            // Space→D duplicate 1-line addition. Bumped 55→56 for the `I` (tab_new_after) 1-line
            // addition to the Tabs section.
            let mut term = Terminal::new(TestBackend::new(72, 56)).unwrap();
            term.draw(|f| render(f, app)).unwrap();
            term.backend()
                .buffer()
                .content()
                .iter()
                .map(|c| c.symbol())
                .collect::<String>()
        };
        // While hidden, the help sections (ASCII) do not appear.
        // (Since CJK gets split across cells by the test backend, check the ASCII "Tabs"/"Copy")
        assert!(!buf_text(&mut app).contains("Tabs"));
        // Once shown, the popup's sections appear.
        app.show_help = true;
        let s = buf_text(&mut app);
        // Check via the ASCII sections visible near the top (the lower part can be off-screen due to wrapping/scrolling).
        assert!(
            s.contains("Tabs") && s.contains("Copy"),
            "ヘルプのセクションが無い"
        );
    }

    #[test]
    fn sort_indicator_and_menu_render() {
        let dir = unique_tmp("konoma_sort_render_test");
        std::fs::create_dir_all(&dir).unwrap();
        let mut app = App::new(dir.to_path_buf(), Config::default()).unwrap();
        let buf_text = |app: &mut App| -> String {
            let mut term = Terminal::new(TestBackend::new(80, 12)).unwrap();
            term.draw(|f| render(f, app)).unwrap();
            term.backend()
                .buffer()
                .content()
                .iter()
                .map(|c| c.symbol())
                .collect::<String>()
        };
        // Normally: the current sort "sort:" appears in the top context.
        assert!(
            buf_text(&mut app).contains("sort:"),
            "context に並び表示が無い"
        );
        // Opening the `,` menu shows the choices in the footer (en "n:name" / jp "名前").
        app.pending_leader = Some(crate::keymap::LeaderId::Options);
        let s = buf_text(&mut app);
        assert!(
            s.contains("n:name") || s.contains("名前"),
            "ソートメニューが出ない"
        );
    }

    #[test]
    fn bookmark_overlay_renders() {
        let dir = unique_tmp("konoma_bm_render_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        let base = unique_tmp("konoma_bm_render_base");
        let _ = std::fs::remove_dir_all(&base);
        let mut app = App::new(dir.to_path_buf(), Config::default()).unwrap();
        // Swap in a test-only base so the real ~/.config isn't touched, and register local `a`.
        app.bookmarks =
            crate::bookmarks::Bookmarks::with_base(base.to_path_buf(), &app.tab.open_dir);
        app.bookmarks.set('a', dir.join("sub")).unwrap();
        let buf_text = |app: &mut App| -> String {
            let mut term = Terminal::new(TestBackend::new(80, 16)).unwrap();
            term.draw(|f| render(f, app)).unwrap();
            term.backend()
                .buffer()
                .content()
                .iter()
                .map(|c| c.symbol())
                .collect::<String>()
        };
        // Normally there is no overlay.
        assert!(!buf_text(&mut app).contains("Bookmarks"));
        // Opening the list shows the overlay (title + local heading).
        app.open_bookmark_list();
        let s = buf_text(&mut app);
        assert!(s.contains("Bookmarks"), "一覧オーバーレイのタイトルが無い");
        assert!(
            s.contains("Local") || s.contains("ローカル"),
            "ローカル見出しが無い"
        );
    }

    #[test]
    fn dialog_overlay_renders_input_and_confirm() {
        let dir = unique_tmp("konoma_dialog_render_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), b"x").unwrap();
        let mut app = App::new(dir.to_path_buf(), Config::default()).unwrap();
        let buf_text = |app: &mut App| -> String {
            let mut term = Terminal::new(TestBackend::new(72, 24)).unwrap();
            term.draw(|f| render(f, app)).unwrap();
            term.backend()
                .buffer()
                .content()
                .iter()
                .map(|c| c.symbol())
                .collect::<String>()
        };
        // Input dialog: the text being typed appears (the cursor is a reversed cell, so the body is "> hi").
        app.start_create();
        app.dialog_input_push('h');
        app.dialog_input_push('i');
        assert!(buf_text(&mut app).contains("> hi"), "入力テキストが出ない");
        app.dialog_cancel();
        assert!(!buf_text(&mut app).contains("> hi"), "取消で消える");

        // Delete confirmation: both trash (y) and permanent delete (!) are presented.
        app.tab.selected = 0;
        app.start_delete();
        let s = buf_text(&mut app);
        assert!(s.contains("Trash"), "ゴミ箱の選択肢が出ない: {s:?}");
        assert!(
            s.contains("Delete permanently"),
            "完全削除の選択肢が出ない: {s:?}"
        );
    }

    #[test]
    fn batch_rename_preview_overlay_renders() {
        let dir = unique_tmp("konoma_batchrename_render_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), b"x").unwrap();
        std::fs::write(dir.join("b.txt"), b"x").unwrap();
        let mut app = App::new(dir.to_path_buf(), Config::default()).unwrap();
        app.rebuild_tree().unwrap();
        app.visual_select_scope(true);
        app.start_batch_rename();
        for c in "img_{n}".chars() {
            app.dialog_input_push(c);
        }
        app.dialog_submit().unwrap();
        assert!(app.dialog_is_preview());

        let mut term = Terminal::new(TestBackend::new(72, 24)).unwrap();
        term.draw(|f| render(f, &mut app)).unwrap();
        let s: String = term
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(s.contains('→'), "旧→新 の矢印が出ない");
        assert!(s.contains("img_1"), "新名のプレビューが出ない");
        assert!(s.contains("y = apply"), "適用ヒントが出ない: {s:?}");
    }

    #[test]
    fn selection_marker_and_count_render() {
        let dir = unique_tmp("konoma_selmarker_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), b"x").unwrap();
        let mut app = App::new(dir.to_path_buf(), Config::default()).unwrap();
        app.rebuild_tree().unwrap();
        let buf_text = |app: &mut App| -> String {
            let mut term = Terminal::new(TestBackend::new(72, 24)).unwrap();
            term.draw(|f| render(f, app)).unwrap();
            term.backend()
                .buffer()
                .content()
                .iter()
                .map(|c| c.symbol())
                .collect::<String>()
        };
        // No marker before selecting.
        assert!(!buf_text(&mut app).contains('●'));
        // Selecting `a` shows the marker (●) and the count (sel).
        app.tab.selected = 0;
        app.toggle_select();
        let s = buf_text(&mut app);
        assert!(s.contains('●'), "選択マーカーが出ない");
        assert!(
            s.contains("sel") || s.contains('選'),
            "選択件数が出ない: {s:?}"
        );
    }

    #[test]
    fn info_popup_renders_file_details() {
        let dir = unique_tmp("konoma_info_render_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("hello.txt"), b"hello world").unwrap(); // 11 bytes
        let mut app = App::new(dir.to_path_buf(), Config::default()).unwrap();
        app.rebuild_tree().unwrap();
        app.tab.selected = app
            .tab
            .entries
            .iter()
            .position(|e| e.path.ends_with("hello.txt"))
            .unwrap();
        let text = |app: &mut App| -> String {
            let mut term = Terminal::new(TestBackend::new(70, 16)).unwrap();
            term.draw(|f| render(f, app)).unwrap();
            term.backend()
                .buffer()
                .content()
                .iter()
                .map(|c| c.symbol())
                .collect()
        };
        // No popup before opening it.
        assert!(!text(&mut app).contains("Info"));
        // `i` opens the info popup. Kind/size/permissions appear.
        app.toggle_info();
        assert!(app.is_info());
        let s = text(&mut app);
        assert!(s.contains("Info"), "タイトル");
        assert!(s.contains("file"), "種別");
        assert!(s.contains("11 B"), "サイズ");
        assert!(s.contains("rw-r--r--"), "権限");
        assert!(s.contains("UTC"), "更新日時");
    }

    #[test]
    fn details_columns_render_aligned() {
        let dir = unique_tmp("konoma_details_render_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("assets")).unwrap();
        std::fs::write(dir.join("main.rs"), vec![0u8; 12600]).unwrap(); // ~12.3 KB
        std::fs::write(dir.join("README.md"), vec![0u8; 1200]).unwrap();
        let mut cfg = Config::default();
        cfg.ui.details = vec!["size".into(), "modified".into()];
        let mut app = App::new(dir.to_path_buf(), cfg).unwrap();
        app.rebuild_tree().unwrap();
        let rows = |app: &mut App| -> Vec<String> {
            let mut term = Terminal::new(TestBackend::new(60, 10)).unwrap();
            term.draw(|f| render(f, app)).unwrap();
            let buf = term.backend().buffer();
            (0..10u16)
                .map(|y| {
                    (0..60u16)
                        .map(|x| buf.cell((x, y)).unwrap().symbol().to_string())
                        .collect::<String>()
                })
                .collect()
        };
        let r = rows(&mut app);
        let joined = r.join("\n");
        assert!(joined.contains("12.3 KB"), "サイズ列: {joined}");
        assert!(joined.contains("--"), "ディレクトリのサイズは --");
        assert!(!joined.contains("UTC"), "列は短い日時(UTC ラベル無し)");
        // The size column aligns vertically: "12.3 KB" and "1.2 KB" end at the same column.
        let main_row = r.iter().find(|l| l.contains("main.rs")).unwrap();
        let readme_row = r.iter().find(|l| l.contains("README.md")).unwrap();
        let col_main = main_row.find("12.3 KB").unwrap() + "12.3 KB".len();
        let col_readme = readme_row.find("1.2 KB").unwrap() + "1.2 KB".len();
        assert_eq!(col_main, col_readme, "サイズ列が右揃いで縦に揃う");
    }

    #[test]
    fn mode_chips_have_background_colors() {
        let dir = unique_tmp("konoma_modechip_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), b"x").unwrap();
        let mut app = App::new(dir.to_path_buf(), Config::default()).unwrap();
        // Check the top row's (row 0) chip background color.
        let chip_bg = |app: &mut App, color: Color| -> bool {
            let mut term = Terminal::new(TestBackend::new(72, 24)).unwrap();
            term.draw(|f| render(f, app)).unwrap();
            let buf = term.backend().buffer();
            (0..72u16).any(|x| buf.cell((x, 0)).unwrap().bg == color)
        };
        // Normally: the TREE chip = white background. Red (delete) does not appear.
        assert!(chip_bg(&mut app, Color::White), "TREE チップの白背景が無い");
        assert!(!chip_bg(&mut app, Color::Red), "通常で赤チップが出ている");
        // Delete confirmation: the DELETE chip = red background (the outer TREE white also remains = 2 chips).
        app.start_delete();
        assert!(chip_bg(&mut app, Color::Red), "DELETE チップの赤背景が無い");
        assert!(
            chip_bg(&mut app, Color::White),
            "外側 TREE チップが消えている"
        );
    }

    #[test]
    fn footer_reflects_internal_mode() {
        let dir = unique_tmp("konoma_footer_mode_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), b"x").unwrap();
        let mut app = App::new(dir.to_path_buf(), Config::default()).unwrap();
        let footer = |app: &mut App| row_text(app, 72, 24, 23);
        // Normally: the tree's key hints.
        assert!(footer(&mut app).contains("j/k:move"), "通常フッター");
        // Visual: range-selection keys. Destructive operations go through the Space leader (the old direct D/R was removed).
        app.enter_visual();
        let f = footer(&mut app);
        assert!(
            f.contains("extend") && f.contains("Space:ops"),
            "ビジュアルフッター: {f}"
        );
        app.exit_visual_cancel();
        // Delete confirmation: y/!/n.
        app.start_delete();
        let f = footer(&mut app);
        assert!(
            f.contains("y:Trash") && f.contains("!:"),
            "削除フッター: {f}"
        );
    }

    /// Regression test for bug 1: since `?` help is a centered popup that doesn't cover the
    /// top/bottom edges, if the chip/footer's "which surface is it now" judgment
    /// (`internal_mode()`) doesn't reflect help, the Tree's footer behind it (`l:enter`/`Space:file
    /// ops` etc. — which don't work while help is shown) keeps being displayed.
    #[test]
    fn help_chip_and_footer_reflect_the_help_surface_not_the_view_behind_it() {
        let dir = unique_tmp("konoma_help_footer_mode_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), b"x").unwrap();
        let mut app = App::new(dir.to_path_buf(), Config::default()).unwrap();
        let (w, h) = (80, 24);

        // Normally: the TREE chip on top, the tree's key hints (l:enter, etc.) on the bottom.
        assert!(
            row_text(&mut app, w, h, 0).contains("TREE"),
            "通常時は TREE チップ"
        );
        let footer = row_text(&mut app, w, h, h - 1);
        assert!(
            footer.contains("l:enter"),
            "通常時はツリーのフッター: {footer}"
        );

        // Open help via `?`: internal_mode()/surface() return Help, the inner chip shows HELP, and
        // the footer shows help's own j/k/g/G/q (scroll/close) (the tree's l/Space do not appear —
        // while help is shown, only Surface::Help's bindings work, so guidance should only cover those).
        app.toggle_help();
        assert_eq!(
            app.internal_mode(),
            Some(crate::app::InternalMode::Help),
            "show_help 中は internal_mode() が Help を返す"
        );
        assert_eq!(app.surface(), crate::keymap::Surface::Help);
        let top = row_text(&mut app, w, h, 0);
        assert!(
            top.contains("HELP"),
            "ヘルプ表示中は内側チップに HELP: {top}"
        );
        let help_footer = row_text(&mut app, w, h, h - 1);
        assert!(
            !help_footer.contains("l:enter") && !help_footer.contains("Space:file ops"),
            "ヘルプ表示中はツリーのフッター(押しても効かないキー)を出さない: {help_footer}"
        );
        assert!(
            help_footer.contains("scroll") && help_footer.contains("close"),
            "ヘルプ表示中はヘルプ自身のスクロール/閉じるヒントを出す: {help_footer}"
        );
    }

    #[test]
    fn no_bg_by_default_keeps_terminal_default() {
        // With the default (bg=none), **no overall base coat** is painted = it stays the terminal
        // default (Reset). (Since a mode chip is a small region that intentionally has a background
        // color, verify that the body area = the terminal default.)
        let dir = unique_tmp("konoma_ui_nobg_test");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), b"hi").unwrap();

        let mut app = App::new(dir.to_path_buf(), Config::default()).unwrap();
        let mut term = Terminal::new(TestBackend::new(24, 8)).unwrap();
        term.draw(|f| render(f, &mut app)).unwrap();

        let buf = term.backend().buffer();
        // The body (row 2 onward = not the chip/status-line area) is never painted at all (Reset).
        let body_painted = (2..8u16)
            .flat_map(|y| (0..24u16).map(move |x| (x, y)))
            .any(|(x, y)| buf.cell((x, y)).unwrap().bg != Color::Reset);
        assert!(!body_painted, "本文領域に背景が塗られている");
        // Overall, most (>=85%) stays Reset too (evidence that no overall base coat was painted).
        let total = buf.content().len();
        let reset = buf
            .content()
            .iter()
            .filter(|c| c.bg == Color::Reset)
            .count();
        assert!(
            reset * 100 >= total * 85,
            "全体下地が塗られている: {reset}/{total}"
        );
    }
}
