// ratatui 描画のエントリ。現在のモードに応じて全画面描画する。
// 設計原則「モード遷移」に従い、Tree と Preview の中間の分割表示はしない。
// (最下部のステータス行は内容の分割ではなく全体の chrome。本文領域はモード全画面のまま。)

pub mod bookmarks;
pub mod dialog;
pub mod git;
pub mod help;
pub mod icons;
pub mod info;
pub mod preview;
pub mod status;
pub mod tabbar;
pub mod table;
pub mod tree;

use ratatui::layout::{Constraint, Layout};
use ratatui::style::Style;
use ratatui::Frame;

use crate::app::{App, Mode, StatusbarLayout};

/// Full-screen rendering per mode + status chrome (placement via `ui.statusbar`).
pub fn render(frame: &mut Frame, app: &mut App) {
    // 設定 `ui.theme.bg` があれば全体背景を先に塗る。テキスト span の bg=None は既存セル背景を
    // 上書きしないので、この下地がそのまま全体背景になる ("none" のときは塗らず端末既定=透過維持)。
    if let Some(bg) = app.cfg.ui.theme.bg() {
        let area = frame.area();
        frame.buffer_mut().set_style(area, Style::new().bg(bg));
    }

    // タブバー表示可否: always=常時 / hidden=出さない / auto=2枚以上のとき。
    let show_tabs = match app.cfg.ui.tabbar.as_str() {
        "always" => true,
        "hidden" => false,
        _ => app.tab_count() > 1,
    };
    let layout = StatusbarLayout::parse(&app.cfg.ui.statusbar);
    let split = layout == StatusbarLayout::Split;

    // 上段(ヘッダ)は split のとき常に・bottom のときはタブがある時だけ。下段は常にある。
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
        // タブは左、コンテキストは右(split)。同じ行に共存させる。
        if show_tabs {
            tabbar::render(frame, app, top);
        }
        if split {
            status::render_context(frame, app, top);
        }
    }
    // 下段: split=キーヒントのみ / bottom=コンテキスト＋ヒントをまとめて。
    if split {
        status::render_footer(frame, app, bottom);
    } else {
        status::render_combined(frame, app, bottom);
    }

    // Git 系の全画面ビューは tree/preview の代わりに content へ出す。
    // 優先: コミット詳細 > git log > 変更ハブ(詳細は log の上に被さる)。
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
        match app.mode {
            Mode::Tree => tree::render(frame, app, content),
            Mode::Preview => preview::render(frame, app, content),
        }
    }

    // `?` ヘルプは全要素の上に重ねる。
    if app.show_help {
        help::render(frame, app, frame.area());
    }
    // ブックマーク一覧オーバーレイも最前面に重ねる。
    if app.is_bookmark_list() {
        bookmarks::render(frame, app, frame.area());
    }
    // ファイル情報ポップアップ。
    if app.is_info() {
        info::render(frame, app, frame.area());
    }
    // 確認/入力ダイアログは最優先(キーも横取りされる)なので最前面。
    if app.is_dialog() {
        dialog::render(frame, app, frame.area());
    }
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
        // 3 コミットの subject・ref ラベル・グラフのノード/分岐文字が出る。
        assert!(
            text.contains("C feature work"),
            "feature の subject: {text}"
        );
        assert!(text.contains("B main work"));
        assert!(text.contains("feature"), "ref ラベル feature");
        // 自作レンダラの角ばったノード(通常=●)。角ばった連結(├/┘/─)が出る。
        // (グラフセルに斜め線が無いことは git::tests::lay_out_lanes_draws_angular_fork_and_merge で検証。)
        assert!(text.contains('●'), "グラフのノード ●");
        assert!(
            text.contains('├') || text.contains('┘') || text.contains('┐'),
            "角ばった連結文字が出ない: {text}"
        );
        // グラフ部のセルに色が付く(レーンごとの循環パレット色)。
        let has_color = buf
            .content()
            .iter()
            .any(|c| !matches!(c.fg, Color::Reset | Color::White | Color::Black));
        assert!(has_color, "グラフレーンに色が付いていない");
        // コミット行だけを移動・Enter で詳細(commit_diff)を開ける。
        app.git_graph_move(1);
        app.open_git_graph_detail();
        assert!(app.is_git_detail(), "Enter でコミット詳細が開く");
        assert!(!app.git_detail_lines().is_empty(), "詳細に diff 行がある");

        // --- 未コミットの作業ツリー変更が "Uncommitted changes" 行として最上部に出る ---
        app.close_git_detail();
        std::fs::write(dir.join("a.txt"), b"a changed").unwrap(); // 追跡ファイルを変更(unstaged)
        std::fs::write(dir.join("untracked.txt"), b"new").unwrap(); // 未追跡
        git(&["add", "b.txt"]); // 既存追跡ファイルを再ステージ(staged 1件)
        std::fs::write(dir.join("b.txt"), b"b staged change").unwrap();
        git(&["add", "b.txt"]);
        app.open_git_graph();
        // 先頭行(カーソル)が作業ツリー行で、index 0 に乗る。
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
        // Enter で worktree_diff(複数ファイル)を全画面 diff として開ける。
        app.open_git_graph_detail();
        assert!(app.is_git_detail(), "Enter で作業ツリー詳細が開く");
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
            "作業ツリー diff が複数ファイル: headers={headers}"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn preview_line_start_end_horizontal_jump() {
        // 非折返し時、$ で行末(END)へ・0 で行頭(START)へ一発移動できる。
        let dir = std::env::temp_dir().join("konoma_hscroll_jump_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let long = format!("START{}END", "x".repeat(200));
        std::fs::write(dir.join("a.txt"), &long).unwrap();

        let mut cfg = Config::default();
        cfg.ui.wrap = false; // 横スクロールを有効化
        let mut app = App::new(dir.clone(), cfg).unwrap();
        app.rebuild_tree().unwrap();
        app.tree_descend().unwrap(); // a.txt を選択中 → Preview へ
        assert!(
            matches!(app.mode, crate::app::Mode::Preview),
            "Preview 遷移"
        );

        let dump = |app: &mut App| -> String {
            let mut term = Terminal::new(TestBackend::new(40, 8)).unwrap();
            term.draw(|f| render(f, app)).unwrap();
            let buf = term.backend().buffer();
            buf.content().iter().map(|c| c.symbol()).collect()
        };
        // 既定(hscroll=0)=行頭: START は見え、遠い END は画面外。
        let s0 = dump(&mut app);
        assert!(s0.contains("START"), "行頭に START が無い");
        assert!(!s0.contains("END"), "行頭で END が見えてはいけない");
        // $ で行末へ: END が見える(START は画面外)。
        app.preview_hscroll_end();
        let se = dump(&mut app);
        assert!(se.contains("END"), "$ で行末(END)が見えない");
        assert!(!se.contains("START"), "行末で START が見えてはいけない");
        // 0 で行頭へ戻る。
        app.preview_hscroll_home();
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
        // テキストセルも下地が残るので、大半のセルが設定背景になっているはず。
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
        let mut app = App::new(dir.clone(), Config::default()).unwrap(); // 既定=split
        let (w, h) = (70, 6);
        // 上段にコンテキスト(TREE/path)、下段にキーヒント。
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
        // 上段はコンテキストを出さない(1タブなのでタブバーも無し)。
        assert!(!row_text(&mut app, w, h, 0).contains("TREE"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn lang_jp_localizes_chrome_and_help() {
        let dir = std::env::temp_dir().join("konoma_lang_jp_test");
        std::fs::create_dir_all(&dir).unwrap();
        let mut cfg = Config::default();
        cfg.ui.lang = "jp".into();
        cfg.ui.statusbar = "bottom".into(); // 下1行にまとめて検査しやすく
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
        // 下段に日本語のチップ/ヒント。CJK はテストバックエンドでセル分割されるので
        // 単一文字で判定する("ツ"=ツリー, "移"=移動)。英語の TREE/jk:move は出ない。
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
        // 'U' マーカーが LightGreen(未追跡色) で出ているセルがある。
        let found = buf
            .content()
            .iter()
            .any(|c| c.symbol() == "U" && c.fg == ratatui::style::Color::LightGreen);
        assert!(found, "未追跡 'U' マーカーが無い");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn tree_visible_range_keeps_selection_onscreen() {
        // C3: 可視範囲だけを Line 化する最適化が、選択行を必ず画面内に保つことを保証する。
        // ビューポートより多いエントリで末尾を選び、末尾名が描画され先頭名は画面外(=描かれない)。
        let dir = std::env::temp_dir().join("konoma_tree_visrange_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("aaa_first.txt"), b"x").unwrap();
        for i in 0..30 {
            std::fs::write(dir.join(format!("m{i:02}.txt")), b"x").unwrap();
        }
        std::fs::write(dir.join("zzz_last.txt"), b"x").unwrap();
        let mut app = App::new(dir.canonicalize().unwrap(), Config::default()).unwrap();
        // 名前昇順なので zzz_last が末尾。末尾を選択 → 末尾が画面下端に来る。
        app.selected = app.entries.len() - 1;
        assert!(
            app.entries[app.selected]
                .path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap()
                .starts_with("zzz_last"),
            "末尾が zzz_last でない(ソート前提が崩れた)"
        );

        let mut term = Terminal::new(TestBackend::new(40, 8)).unwrap(); // 可視 6 行
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
        // タイトル枠にブランチ記号 ⎇ が出る。
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
            // ヘルプ全節が折返し無く収まる高さで検証する(節が増えても画面外へ押し出されないように。
            // 実機の小さい端末ではスクロール(j/k)で下部を見る挙動が正)。
            let mut term = Terminal::new(TestBackend::new(72, 50)).unwrap();
            term.draw(|f| render(f, app)).unwrap();
            term.backend()
                .buffer()
                .content()
                .iter()
                .map(|c| c.symbol())
                .collect::<String>()
        };
        // 非表示ではヘルプのセクション(ASCII)は出ない。
        // (CJK はテスト用バックエンドでセル分割されるので ASCII の "Tabs"/"Copy" で判定)
        assert!(!buf_text(&mut app).contains("Tabs"));
        // 表示にするとポップアップのセクションが出る。
        app.show_help = true;
        let s = buf_text(&mut app);
        // 上部に見える ASCII セクションで判定 (下方は折返し/スクロールで画面外のことがある)。
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
        // 通常時: 上部 context に現在の並び "sort:" が出る。
        assert!(
            buf_text(&mut app).contains("sort:"),
            "context に並び表示が無い"
        );
        // s 相当でメニューを開くとフッターに選択肢が出る (en の "[n]ame" / jp の "名前")。
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
        // 実 ~/.config を汚さないようテスト用ベースに差し替え、ローカル a を登録。
        app.bookmarks = crate::bookmarks::Bookmarks::with_base(base.clone(), &app.open_dir);
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
        // 通常時はオーバーレイ無し。
        assert!(!buf_text(&mut app).contains("Bookmarks"));
        // 一覧を開くとオーバーレイ(タイトル + ローカル見出し)が出る。
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
        // 入力ダイアログ: 入力中テキストが出る(カーソルは反転セルなので本文は "> hi")。
        app.start_create();
        app.dialog_input_push('h');
        app.dialog_input_push('i');
        assert!(buf_text(&mut app).contains("> hi"), "入力テキストが出ない");
        app.dialog_cancel();
        assert!(!buf_text(&mut app).contains("> hi"), "取消で消える");

        // 削除確認: ゴミ箱(y)と完全削除(!)の両方が提示される。
        app.selected = 0;
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
        // 選択前はマーカー無し。
        assert!(!buf_text(&mut app).contains('●'));
        // a を選択するとマーカー(●)と件数(sel)が出る。
        app.selected = 0;
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
        app.selected = app
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
        // 開く前はポップアップ無し。
        assert!(!text(&mut app).contains("Info"));
        // i で情報ポップアップ。種別/サイズ/権限が出る。
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
        // サイズ列が縦に揃う: "12.3 KB" と "1.2 KB" の末尾桁が同じ列。
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
        // 上段(row 0)のチップ背景色を調べる。
        let chip_bg = |app: &mut App, color: Color| -> bool {
            let mut term = Terminal::new(TestBackend::new(72, 24)).unwrap();
            term.draw(|f| render(f, app)).unwrap();
            let buf = term.backend().buffer();
            (0..72u16).any(|x| buf.cell((x, 0)).unwrap().bg == color)
        };
        // 通常: TREE チップ=白背景。赤(削除)は出ない。
        assert!(chip_bg(&mut app, Color::White), "TREE チップの白背景が無い");
        assert!(!chip_bg(&mut app, Color::Red), "通常で赤チップが出ている");
        // 削除確認: DELETE チップ=赤背景(外側 TREE 白も残る=2チップ)。
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
        // 通常: ツリーのキーヒント。
        assert!(footer(&mut app).contains("jk:move"), "通常フッター");
        // ビジュアル: 範囲選択のキー。破壊操作は Space リーダー経由 (旧直 D/R は廃止)。
        app.enter_visual();
        let f = footer(&mut app);
        assert!(
            f.contains("extend") && f.contains("Space:ops"),
            "ビジュアルフッター: {f}"
        );
        app.exit_visual_cancel();
        // 削除確認: y/!/n。
        app.start_delete();
        let f = footer(&mut app);
        assert!(
            f.contains("y:Trash") && f.contains("!:"),
            "削除フッター: {f}"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn no_bg_by_default_keeps_terminal_default() {
        // 既定 (bg=none) では**全体下地**を塗らない=端末既定(Reset)のまま。
        // (モードチップは意図して背景色を持つ小領域なので、本文領域=端末既定であることを確認する。)
        let dir = std::env::temp_dir().join("konoma_ui_nobg_test");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), b"hi").unwrap();

        let mut app = App::new(dir.clone(), Config::default()).unwrap();
        let mut term = Terminal::new(TestBackend::new(24, 8)).unwrap();
        term.draw(|f| render(f, &mut app)).unwrap();

        let buf = term.backend().buffer();
        // 本文(行 2 以降=チップやステータス行ではない領域)は一切塗らない (Reset)。
        let body_painted = (2..8u16)
            .flat_map(|y| (0..24u16).map(move |x| (x, y)))
            .any(|(x, y)| buf.cell((x, y)).unwrap().bg != Color::Reset);
        assert!(!body_painted, "本文領域に背景が塗られている");
        // 全体としても大半(>=85%)は Reset のまま(全体下地塗りをしていない証拠)。
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
