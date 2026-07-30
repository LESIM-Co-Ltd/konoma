// The entry point for ratatui rendering. Draws full-screen according to the current mode.
// Following the "mode transition" design principle, there is no split display between Tree and
// Preview. (The bottom status line is overall chrome, not a content split — the body area stays
// full-screen per mode.)

pub mod bookmarks;
pub mod dialog;
pub mod git;
pub mod help;
pub mod icons;
pub mod info;
pub mod outline;
pub mod preview;
pub mod status;
pub mod tab_list;
pub mod tabbar;
pub mod table;
pub mod tree;

use ratatui::layout::{Constraint, Layout};
use ratatui::style::Style;
use ratatui::Frame;

use crate::app::{App, Mode, StatusbarLayout};

/// Full-screen rendering per mode + status chrome (placement via `ui.statusbar`).
pub fn render(frame: &mut Frame, app: &mut App) {
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
    if split {
        status::render_footer(frame, app, bottom);
    } else {
        status::render_combined(frame, app, bottom);
    }

    // Git-related full-screen views go into content in place of tree/preview.
    // Priority: commit detail > git log > changes hub (detail overlays on top of log).
    if app.is_git_detail() {
        git::render_detail(frame, app, content);
    } else if app.is_git_graph_picker() {
        git::render_graph_picker(frame, app, content);
    } else if app.is_git_graph() {
        git::render_graph(frame, app, content);
    } else if app.is_git_log() {
        git::render_log(frame, app, content);
    } else if app.is_git_branches() {
        git::render_branches(frame, app, content);
    } else if app.is_git_view() {
        git::render_changes(frame, app, content);
    } else {
        match app.tab.mode {
            Mode::Tree => tree::render(frame, app, content),
            Mode::Preview => preview::render(frame, app, content),
        }
    }

    // `?` help overlays on top of everything.
    if app.show_help {
        help::render(frame, app, frame.area());
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
    if app.is_outline() {
        outline::render(frame, app, frame.area());
    }
    // The file info popup.
    if app.is_info() {
        info::render(frame, app, frame.area());
    }
    // The table cell full-text popup (`Enter`).
    if app.is_table_cell_open() {
        table::render_cell_popup(frame, app, frame.area());
    }
    // A confirm/input dialog has top priority (it also intercepts keys), so it sits on top of everything.
    if app.is_dialog() {
        dialog::render(frame, app, frame.area());
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
    use ratatui::backend::TestBackend;
    use ratatui::style::Color;
    use ratatui::Terminal;

    #[cfg(feature = "git")]
    #[test]
    fn git_graph_renders_lanes_refs_and_detail() {
        use std::process::Command;
        let dir = std::env::temp_dir().join("konoma_graph_render_test");
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
        std::fs::write(dir.join("a.txt"), b"a").unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "A init"]);
        git(&["checkout", "-q", "-b", "feature"]);
        std::fs::write(dir.join("c.txt"), b"c").unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "C feature work"]);
        git(&["checkout", "-q", "-"]);
        std::fs::write(dir.join("b.txt"), b"b").unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "B main work"]);

        let mut app = App::new(dir.canonicalize().unwrap(), Config::default()).unwrap();
        app.open_git_graph();
        assert!(app.is_git_graph());

        let mut term = Terminal::new(TestBackend::new(72, 12)).unwrap();
        term.draw(|f| render(f, &mut app)).unwrap();
        let buf = term.backend().buffer();
        let text: String = buf.content().iter().map(|c| c.symbol()).collect();
        // The 3 commits' subjects, ref labels, and the graph's node/branch characters appear.
        assert!(
            text.contains("C feature work"),
            "feature の subject: {text}"
        );
        assert!(text.contains("B main work"));
        assert!(text.contains("feature"), "ref ラベル feature");
        // The custom renderer's angular node (normal = ●). Angular connectors (├/┘/─) appear.
        // (That the graph cells have no diagonal lines is verified by
        // git::tests::lay_out_lanes_draws_angular_fork_and_merge.)
        assert!(text.contains('●'), "グラフのノード ●");
        assert!(
            text.contains('├') || text.contains('┘') || text.contains('┐'),
            "角ばった連結文字が出ない: {text}"
        );
        // The graph section's cells are colored (a per-lane rotating palette color).
        let has_color = buf
            .content()
            .iter()
            .any(|c| !matches!(c.fg, Color::Reset | Color::White | Color::Black));
        assert!(has_color, "グラフレーンに色が付いていない");
        // Move only among commit rows, and Enter opens the detail (commit_diff).
        app.git_graph_move(1);
        app.open_git_graph_detail();
        assert!(app.is_git_detail(), "Enter でコミット詳細が開く");
        assert!(!app.git_detail_lines().is_empty(), "詳細に diff 行がある");

        // --- Uncommitted working-tree changes appear at the top as an "Uncommitted changes" row ---
        app.close_git_detail();
        std::fs::write(dir.join("a.txt"), b"a changed").unwrap(); // change a tracked file (unstaged)
        std::fs::write(dir.join("untracked.txt"), b"new").unwrap(); // untracked
        git(&["add", "b.txt"]); // re-stage an existing tracked file (1 staged)
        std::fs::write(dir.join("b.txt"), b"b staged change").unwrap();
        git(&["add", "b.txt"]);
        app.open_git_graph();
        // The top row (cursor) is the working-tree row, sitting at index 0.
        let row0 = app.git_graph_selected_row().expect("先頭行");
        assert!(row0.worktree, "先頭が作業ツリー行");
        assert_eq!(app.git_graph_sel(), 0, "カーソルが作業ツリー行に乗る");
        let mut term2 = Terminal::new(TestBackend::new(88, 14)).unwrap();
        term2.draw(|f| render(f, &mut app)).unwrap();
        let buf2 = term2.backend().buffer();
        let text2: String = buf2.content().iter().map(|c| c.symbol()).collect();
        assert!(
            text2.contains("Uncommitted changes"),
            "作業ツリー行の見出し: {text2}"
        );
        assert!(text2.contains('●'), "作業ツリーノード ●: {text2}");
        // Enter opens worktree_diff (multiple files) as a full-screen diff.
        app.open_git_graph_detail();
        assert!(app.is_git_detail(), "Enter で作業ツリー詳細が開く");
        // A file-boundary header = Context and both line numbers are None.
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
            "作業ツリー diff が複数ファイル: headers={headers}"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn preview_line_start_end_horizontal_jump() {
        // While not wrapping, `$` jumps straight to line-end (END) and `0` jumps straight to line-start (START).
        let dir = std::env::temp_dir().join("konoma_hscroll_jump_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let long = format!("START{}END", "x".repeat(200));
        std::fs::write(dir.join("a.txt"), &long).unwrap();

        let mut cfg = Config::default();
        cfg.ui.wrap = false; // enable horizontal scroll
        let mut app = App::new(dir.clone(), cfg).unwrap();
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

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn global_bg_fills_buffer() {
        let dir = std::env::temp_dir().join("konoma_ui_bg_test");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), b"hi").unwrap();

        let mut cfg = Config::default();
        cfg.ui.theme.bg = "#102030".into();
        let mut app = App::new(dir.clone(), cfg).unwrap();

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
        std::fs::remove_dir_all(&dir).ok();
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
        let dir = std::env::temp_dir().join("konoma_chrome_split_test");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), b"x").unwrap();
        let mut app = App::new(dir.clone(), Config::default()).unwrap(); // default = split
        let (w, h) = (70, 6);
        // Context (TREE/path) on top, key hints on the bottom.
        assert!(row_text(&mut app, w, h, 0).contains("TREE"), "上にモード");
        assert!(row_text(&mut app, w, h, 0).contains("path:"), "上にパス");
        let bottom = row_text(&mut app, w, h, h - 1);
        assert!(bottom.contains("jk:move"), "下にヒント: {bottom}");
        assert!(!bottom.contains("TREE"), "下にモードは出さない: {bottom}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn statusbar_bottom_puts_everything_on_bottom() {
        let dir = std::env::temp_dir().join("konoma_chrome_bottom_test");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), b"x").unwrap();
        let mut cfg = Config::default();
        cfg.ui.statusbar = "bottom".into();
        let mut app = App::new(dir.clone(), cfg).unwrap();
        let (w, h) = (70, 6);
        let bottom = row_text(&mut app, w, h, h - 1);
        assert!(bottom.contains("TREE"), "下にモード: {bottom}");
        assert!(bottom.contains("jk:move"), "下にヒント: {bottom}");
        // The top row shows no context (there's no tab bar either, since it's 1 tab).
        assert!(!row_text(&mut app, w, h, 0).contains("TREE"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn lang_jp_localizes_chrome_and_help() {
        let dir = std::env::temp_dir().join("konoma_lang_jp_test");
        std::fs::create_dir_all(&dir).unwrap();
        let mut cfg = Config::default();
        cfg.ui.lang = "jp".into();
        cfg.ui.statusbar = "bottom".into(); // consolidate onto the bottom row, easier to inspect
        let mut app = App::new(dir.clone(), cfg).unwrap();
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
            !s.contains("TREE") && !s.contains("jk:move"),
            "英語が残る: {s}"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(feature = "git")]
    #[test]
    fn tree_shows_git_status_marker() {
        let dir = std::env::temp_dir().join("konoma_git_marker_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        git2::Repository::init(&dir).unwrap();
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
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn tree_visible_range_keeps_selection_onscreen() {
        // C3: guarantees that the optimization of turning only the visible range into Lines always
        // keeps the selected row on-screen. With more entries than the viewport, select the last
        // one — the last name gets drawn and the first name is off-screen (= not drawn).
        let dir = std::env::temp_dir().join("konoma_tree_visrange_test");
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
        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(feature = "git")]
    #[test]
    fn tree_title_shows_branch() {
        let dir = std::env::temp_dir().join("konoma_branch_title_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        git2::Repository::init(&dir).unwrap();
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
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn help_overlay_renders_when_shown() {
        let dir = std::env::temp_dir().join("konoma_help_render_test");
        std::fs::create_dir_all(&dir).unwrap();
        let mut app = App::new(dir.clone(), Config::default()).unwrap();
        let buf_text = |app: &mut App| -> String {
            // Verify at a height where every help section fits without wrapping (so a section
            // being added doesn't push it off-screen; on a real small terminal, scrolling (j/k) to
            // see the bottom is the correct behavior).
            // Bumped 50→54 for the Agent Watch (C/n·N/F) 3-line addition. Bumped 54→55 for the
            // Space→D duplicate 1-line addition.
            let mut term = Terminal::new(TestBackend::new(72, 55)).unwrap();
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
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn sort_indicator_and_menu_render() {
        let dir = std::env::temp_dir().join("konoma_sort_render_test");
        std::fs::create_dir_all(&dir).unwrap();
        let mut app = App::new(dir.clone(), Config::default()).unwrap();
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
        // Opening the menu via the s-equivalent shows choices in the footer (en's "[n]ame" / jp's "名前").
        app.open_sort_menu();
        let s = buf_text(&mut app);
        assert!(
            s.contains("[n]ame") || s.contains("名前"),
            "ソートメニューが出ない"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn bookmark_overlay_renders() {
        let dir = std::env::temp_dir().join("konoma_bm_render_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        let base = std::env::temp_dir().join("konoma_bm_render_base");
        let _ = std::fs::remove_dir_all(&base);
        let mut app = App::new(dir.clone(), Config::default()).unwrap();
        // Swap in a test-only base so the real ~/.config isn't touched, and register local `a`.
        app.bookmarks = crate::bookmarks::Bookmarks::with_base(base.clone(), &app.tab.open_dir);
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

        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn dialog_overlay_renders_input_and_confirm() {
        let dir = std::env::temp_dir().join("konoma_dialog_render_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), b"x").unwrap();
        let mut app = App::new(dir.clone(), Config::default()).unwrap();
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

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn batch_rename_preview_overlay_renders() {
        let dir = std::env::temp_dir().join("konoma_batchrename_render_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), b"x").unwrap();
        std::fs::write(dir.join("b.txt"), b"x").unwrap();
        let mut app = App::new(dir.clone(), Config::default()).unwrap();
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

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn selection_marker_and_count_render() {
        let dir = std::env::temp_dir().join("konoma_selmarker_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), b"x").unwrap();
        let mut app = App::new(dir.clone(), Config::default()).unwrap();
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

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn info_popup_renders_file_details() {
        let dir = std::env::temp_dir().join("konoma_info_render_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("hello.txt"), b"hello world").unwrap(); // 11 bytes
        let mut app = App::new(dir.clone(), Config::default()).unwrap();
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
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn details_columns_render_aligned() {
        let dir = std::env::temp_dir().join("konoma_details_render_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("assets")).unwrap();
        std::fs::write(dir.join("main.rs"), vec![0u8; 12600]).unwrap(); // ~12.3 KB
        std::fs::write(dir.join("README.md"), vec![0u8; 1200]).unwrap();
        let mut cfg = Config::default();
        cfg.ui.details = vec!["size".into(), "modified".into()];
        let mut app = App::new(dir.clone(), cfg).unwrap();
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
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn mode_chips_have_background_colors() {
        let dir = std::env::temp_dir().join("konoma_modechip_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), b"x").unwrap();
        let mut app = App::new(dir.clone(), Config::default()).unwrap();
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
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn footer_reflects_internal_mode() {
        let dir = std::env::temp_dir().join("konoma_footer_mode_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), b"x").unwrap();
        let mut app = App::new(dir.clone(), Config::default()).unwrap();
        let footer = |app: &mut App| row_text(app, 72, 24, 23);
        // Normally: the tree's key hints.
        assert!(footer(&mut app).contains("jk:move"), "通常フッター");
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
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Regression test for bug 1: since `?` help is a centered popup that doesn't cover the
    /// top/bottom edges, if the chip/footer's "which surface is it now" judgment
    /// (`internal_mode()`) doesn't reflect help, the Tree's footer behind it (`l:enter`/`Space:file
    /// ops` etc. — which don't work while help is shown) keeps being displayed.
    #[test]
    fn help_chip_and_footer_reflect_the_help_surface_not_the_view_behind_it() {
        let dir = std::env::temp_dir().join("konoma_help_footer_mode_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), b"x").unwrap();
        let mut app = App::new(dir.clone(), Config::default()).unwrap();
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
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn no_bg_by_default_keeps_terminal_default() {
        // With the default (bg=none), **no overall base coat** is painted = it stays the terminal
        // default (Reset). (Since a mode chip is a small region that intentionally has a background
        // color, verify that the body area = the terminal default.)
        let dir = std::env::temp_dir().join("konoma_ui_nobg_test");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), b"hi").unwrap();

        let mut app = App::new(dir.clone(), Config::default()).unwrap();
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
        std::fs::remove_dir_all(&dir).ok();
    }
}
