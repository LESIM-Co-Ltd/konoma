// ステータス系クローム (lightline 風)。規約 (htop/lazygit/k9s/yazi で収束):
//   - 永続コンテキスト(モード/パス/倍率/タブ) = 上 (ヘッダ)
//   - キーヒント = 下 (フッター)。最も有用な数個を常時表示し、幅に入る分だけ優先度順に出す。
//   - 一時メッセージ(flash)・入力途中のコード(prefix→…) はフッター位置を一時的に占有。
// 配置は設定 `ui.statusbar` (split=既定 / bottom) で切替。ここは「中身(spans)」を組み立てる。

use crossterm::event::KeyCode;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use crate::app::{App, DisplayMode, InternalMode, KeyScheme, Mode, PathStyle};
use crate::i18n::{tr, Lang, Msg};

/// Shared helper that builds one mode chip. Background color = meaning; foreground is black/white by lightness.
fn chip(lang: Lang, msg: Msg, bg: Color, dark_bg: bool) -> Span<'static> {
    chip_str(tr(lang, msg).to_string(), bg, dark_bg)
}

/// A chip for arbitrary text (used for breadcrumbs like `GRAPH - BRANCH`).
fn chip_str(text: String, bg: Color, dark_bg: bool) -> Span<'static> {
    let fg = if dark_bg { Color::White } else { Color::Black };
    Span::styled(
        format!(" {text} "),
        Style::new().fg(fg).bg(bg).add_modifier(Modifier::BOLD),
    )
}

/// Display-mode (outer) chip. In a Git view = GIT (yellow) / TREE = white / PREVIEW/IMAGE = blue.
/// Since Git has grown, the changes hub/log/graph/branches/detail **promote the main mode to GIT**.
fn display_chip(app: &App) -> Span<'static> {
    if app.in_git_view() {
        return chip(app.lang, Msg::StGit, Color::Yellow, false);
    }
    let (msg, bg, dark) = match app.display_mode() {
        DisplayMode::Tree => (Msg::StTree, Color::White, false),
        // 「中身を見る」青系の家族。IMAGE は明度違い(LightBlue)でズーム/パンの別操作を示す。
        DisplayMode::Preview => (Msg::StPreview, Color::Blue, true),
        DisplayMode::Image => (Msg::StImage, Color::LightBlue, false),
        // テーブルも「中身を見る」青系の家族(csvlens 風の閲覧モード)。
        DisplayMode::Table => (Msg::StTable, Color::Cyan, true),
    };
    chip(app.lang, msg, bg, dark)
}

/// Internal-mode (inner) chip. Color = meaning (delete = red / create = green / select = magenta / input = cyan / auxiliary = yellow).
/// Since a Git view's main mode is GIT (above), the inner chip shows the concrete view name (CHANGES/LOG/GRAPH/BRANCH/…).
/// Nested state is a breadcrumb `parent - child` (e.g. the graph's branch panel = `GRAPH - BRANCH`).
fn internal_chip(app: &App) -> Option<Span<'static>> {
    // グラフのブランチパネルは GRAPH のサブ状態なのでパンくず表記。
    if matches!(app.internal_mode()?, InternalMode::GitGraphPicker) {
        let text = format!(
            "{} - {}",
            tr(app.lang, Msg::StGraph),
            tr(app.lang, Msg::StBranch)
        );
        return Some(chip_str(text, Color::Yellow, false));
    }
    let (msg, bg, dark) = match app.internal_mode()? {
        InternalMode::Visual => (Msg::StVisual, Color::Magenta, true),
        // プレビューの選択: 文字範囲(v)=VISUAL / 行(V)=V-LINE。
        InternalMode::PreviewVisual => {
            if app.preview_visual_linewise() {
                (Msg::StVisualLine, Color::Magenta, true)
            } else {
                (Msg::StVisual, Color::Magenta, true)
            }
        }
        InternalMode::Filter => (Msg::StFilter, Color::Yellow, false),
        // 変更ファイルのみ表示(Agent Watch)。git 変更の一覧=Git 家族の黄系。
        InternalMode::ChangedFilter => (Msg::StChangedOnly, Color::Yellow, false),
        InternalMode::Search => (Msg::StSearch, Color::Yellow, false),
        InternalMode::Sort => (Msg::StSort, Color::Yellow, false),
        InternalMode::Mark => (Msg::StMark, Color::Yellow, false),
        InternalMode::Bookmarks => (Msg::StBookmarks, Color::Yellow, false),
        InternalMode::Info => (Msg::StInfo, Color::DarkGray, true),
        InternalMode::Create => (Msg::StCreate, Color::Green, false),
        InternalMode::Rename => (Msg::StRename, Color::Cyan, false),
        InternalMode::BatchRename => (Msg::StBatchRename, Color::Cyan, false),
        InternalMode::RenamePreview => (Msg::StRenameConfirm, Color::Cyan, false),
        InternalMode::DeleteConfirm => (Msg::StDelete, Color::Red, true),
        // D&D 転送は非破壊なのでシアン(コピー/移動の選択)。
        InternalMode::DropConfirm => (Msg::StDrop, Color::Cyan, false),
        // アプリ終了確認は非破壊=黄。
        InternalMode::QuitConfirm => (Msg::StQuit, Color::Yellow, false),
        InternalMode::GitChanges => (Msg::StChanges, Color::Yellow, false),
        // diff は「中身を見る」プレビュー家族なので青系。
        InternalMode::GitDiff => (Msg::StDiff, Color::Blue, true),
        // コミットは「作る/確定する」ので緑系(作成と同系統)。
        InternalMode::Commit => (Msg::StCommit, Color::Green, false),
        // log は履歴閲覧(Git 家族の黄系)。詳細は中身を見る diff 家族なので青系。
        InternalMode::GitLog => (Msg::StLog, Color::Yellow, false),
        InternalMode::GitDetail => (Msg::StCommitDiff, Color::Blue, true),
        // ブランチ操作も Git 家族の黄系。
        InternalMode::GitBranch => (Msg::StBranch, Color::Yellow, false),
        InternalMode::GitGraph => (Msg::StGraph, Color::Yellow, false),
        // GitGraphPicker は冒頭(早期 return)でパンくず処理済み=ここには来ない。
        // 万一ガードが外れても描画ループ内で panic させないよう、GRAPH チップに穏当にフォールバックする。
        InternalMode::GitGraphPicker => (Msg::StGraph, Color::Yellow, false),
    };
    Some(chip(app.lang, msg, bg, dark))
}

/// Shared helper that formats one hint as `"key:label"`. Used by each view's `footer_hints`.
pub fn hint(lang: Lang, keys: &str, msg: Msg) -> String {
    format!("{keys}:{}", tr(lang, msg))
}

/// Page-step hint (notation changes by scheme). Shared hint used by both Tree and Preview.
pub fn page_hint(app: &App) -> String {
    match app.key_scheme {
        KeyScheme::Vim => hint(app.lang, "^f/^b", crate::i18n::Msg::HintPage),
        KeyScheme::Less => hint(app.lang, "Spc/b", crate::i18n::Msg::HintPage),
    }
}

/// Page-step description for the `?` help (wording changes by scheme). Shared string used by Tree/Preview help.
pub fn page_help(app: &App) -> String {
    match app.key_scheme {
        KeyScheme::Vim => tr(app.lang, crate::i18n::Msg::StScrollHintCtrl),
        KeyScheme::Less => tr(app.lang, crate::i18n::Msg::StPagerSpaceHint),
    }
    .to_string()
}

/// Persistent context line: **each view owns its own context spans (chips, etc.)**; here we
/// delegate to that and then append the common path display style. (Tree = chip only / Preview = chip + image zoom.)
pub fn context_spans(app: &App) -> Vec<Span<'static>> {
    // 外側チップ(表示モード) ＋ 内側チップ(内部モード・あれば)。
    let mut spans = vec![display_chip(app)];
    if let Some(ic) = internal_chip(app) {
        spans.push(Span::from(" "));
        spans.push(ic);
    }
    // フォローモード(`F`)は他の状態と併存するので独立チップで常時見せる(ON が一目で分かるように)。
    if app.follow_enabled() {
        spans.push(Span::from(" "));
        spans.push(chip(app.lang, Msg::StFollow, Color::Green, false));
    }
    // 各ビュー固有の追記(Tree=ソート/選択件数 / Preview=画像倍率)。チップはここでは出さない。
    spans.extend(match app.mode {
        Mode::Tree => crate::ui::tree::context(app),
        Mode::Preview => crate::ui::preview::context(app),
    });
    let lang = app.lang;
    let style = match app.path_style {
        PathStyle::Relative => tr(lang, crate::i18n::Msg::RelLabel),
        PathStyle::Home => "~",
        PathStyle::Full => tr(lang, crate::i18n::Msg::StPathAbs),
    };
    let prefix = tr(lang, crate::i18n::Msg::PathLabel);
    spans.push(Span::from(format!("  {prefix}{style}")).dim());
    spans
}

/// Per-internal-mode footer help (the bottom footer is the source of truth for "keys you can press now").
/// Dialog/visual keys are shown here (the delete confirmation's y/!/n also appears in the box, but the source of truth is here).
fn mode_footer(app: &App) -> Option<Vec<Span<'static>>> {
    let lang = app.lang;
    let s = match app.internal_mode()? {
        InternalMode::DeleteConfirm if app.confirm_is_branch_delete() => {
            tr(lang, crate::i18n::Msg::StDeleteForce)
        }
        InternalMode::DeleteConfirm => tr(lang, crate::i18n::Msg::StTrashDelete),
        InternalMode::DropConfirm => tr(lang, crate::i18n::Msg::StCopyMoveHint),
        InternalMode::QuitConfirm => tr(lang, crate::i18n::Msg::StQuitHint),
        InternalMode::RenamePreview => tr(lang, crate::i18n::Msg::StApply),
        InternalMode::Create => tr(lang, crate::i18n::Msg::StCreateHint),
        InternalMode::BatchRename => tr(lang, crate::i18n::Msg::StRenamePreviewHint),
        InternalMode::Rename => tr(lang, crate::i18n::Msg::StRenameHint),
        InternalMode::Visual => tr(lang, crate::i18n::Msg::VisualOpsHint),
        InternalMode::PreviewVisual => {
            if app.preview_visual_linewise() {
                tr(lang, crate::i18n::Msg::PreviewVisualLineHint)
            } else {
                tr(lang, crate::i18n::Msg::PreviewVisualHint)
            }
        }
        InternalMode::Info => tr(lang, crate::i18n::Msg::StCloseHint),
        InternalMode::ChangedFilter => tr(lang, crate::i18n::Msg::ChangedFilterHint),
        InternalMode::GitChanges => tr(lang, crate::i18n::Msg::StGitHubKeys),
        InternalMode::GitGraph => tr(lang, crate::i18n::Msg::GitNavDetailCommitHint),
        InternalMode::GitGraphPicker => tr(lang, crate::i18n::Msg::GraphPickerFooter),
        InternalMode::GitBranch => tr(lang, crate::i18n::Msg::BranchesNavHint),
        InternalMode::GitDiff => tr(lang, crate::i18n::Msg::DiffScrollDiscardHint),
        InternalMode::Commit => tr(lang, crate::i18n::Msg::StCommitHint),
        InternalMode::GitLog => tr(lang, crate::i18n::Msg::GitNavDetailHint),
        InternalMode::GitDetail => tr(lang, crate::i18n::Msg::DiffScrollHint),
        // 絞り込み/検索/ソート/マーク/ブックマークは下の専用プロンプトが担当。
        _ => return None,
    };
    Some(vec![Span::from(s).bold()])
}

/// Footer: flash → which-key leader → in-progress input guide → priority-ordered key hints (as many as fit in `width`).
pub fn footer_spans(app: &App, width: u16) -> Vec<Span<'static>> {
    let lang = app.lang;
    if let Some(msg) = &app.flash {
        return vec![Span::from(msg.clone()).bold()];
    }
    // which-key: リーダー(Space=ファイル管理 / y=パスコピー)押下後、次の打鍵の候補を出す。
    if let Some(spans) = whichkey_spans(app) {
        return spans;
    }
    // ブランチ絞り込み入力中はクエリのプロンプトを出す(モードフッターより優先)。
    if app.git_branch_filtering() {
        let q = app.git_branch_query();
        return vec![
            Span::from(format!("/{q}")).bold(),
            Span::from("▏").dim(),
            Span::from(format!(
                "  {}",
                tr(lang, crate::i18n::Msg::StApplyClearHint)
            ))
            .dim(),
        ];
    }
    // ダイアログ/ビジュアル中はそのモードのキーを出す(下部フッターが正)。
    if let Some(spans) = mode_footer(app) {
        return spans;
    }
    // 絞り込み入力中はクエリのプロンプトをフッターに出す ("/<query>▏")。
    if app.is_filtering() {
        let q = app.filter_query().unwrap_or("");
        return vec![
            Span::from(format!("/{q}")).bold(),
            Span::from("▏").dim(),
            Span::from(format!(
                "  {}",
                tr(lang, crate::i18n::Msg::StApplyClearHint)
            ))
            .dim(),
        ];
    }
    // プレビュー内検索の入力中も同様にプロンプトを出す。
    if app.is_searching() {
        let q = app.search_input().unwrap_or("");
        return vec![
            Span::from(format!("/{q}")).bold(),
            Span::from("▏").dim(),
            Span::from(format!("  {}", tr(lang, crate::i18n::Msg::StSearchHint))).dim(),
        ];
    }
    // ソート選択メニュー表示中はフッターに選択肢を出す (現在値 ▸ キー一覧)。
    if app.is_sort_menu() {
        return vec![
            Span::from(format!("{} ▸ ", app.sort_label())).bold(),
            Span::from(tr(lang, crate::i18n::Msg::StSortHint)).dim(),
        ];
    }
    // ブックマークの登録待ちプロンプト (m=登録。'=一覧は Surface::Bookmarks 側)。
    if app.is_marking() {
        return vec![Span::from(tr(lang, crate::i18n::Msg::MarkHint)).bold()];
    }
    vec![Span::from(fit_tokens(&hint_tokens(app), width)).dim()]
}

/// which-key popup (footer-line version). Only while waiting on a leader, shows the menu heading + candidates
/// (`key:label`). Returns None if pending_leader is None. Zero per-frame cost (only while waiting).
fn whichkey_spans(app: &App) -> Option<Vec<Span<'static>>> {
    let lead = app.pending_leader?;
    let menu = app.keymaps.leaders.get(&lead)?;
    let title = tr(app.lang, menu.title);
    let items: Vec<String> = menu
        .items
        .iter()
        .map(|it| {
            let k = match it.key.code {
                KeyCode::Char(' ') => "Space".to_string(),
                KeyCode::Char(c) => c.to_string(),
                _ => "?".to_string(),
            };
            let label = tr(app.lang, it.label);
            format!("{k}:{label}")
        })
        .collect();
    Some(vec![
        Span::from(format!("{title} ▸ ")).bold(),
        Span::from(items.join("  ")).dim(),
    ])
}

/// Operation hints are **owned by each view**. Here we just delegate to the active view.
/// (Tree = `ui::tree::footer_hints` / Preview = `ui::preview::footer_hints`. Kind differences are absorbed inside preview.)
fn hint_tokens(app: &App) -> Vec<String> {
    match app.mode {
        Mode::Tree => crate::ui::tree::footer_hints(app),
        Mode::Preview => crate::ui::preview::footer_hints(app),
    }
}

/// Join tokens with the separator `  ` and return only as many as fit in display width `max`.
/// If cut off partway (not all fit), append ` …` at the end to indicate there is more.
fn fit_tokens(tokens: &[String], max: u16) -> String {
    const SEP: &str = "  ";
    let max = max as usize;
    let ellipsis = " …";
    let mut out = String::new();
    let mut used = 0usize;
    let mut shown = 0usize;
    for tok in tokens {
        let add = if out.is_empty() {
            tok.width()
        } else {
            SEP.width() + tok.width()
        };
        // 残りトークンがあるなら省略記号分を予約して入るか判定。
        let reserve = if shown + 1 < tokens.len() {
            ellipsis.width()
        } else {
            0
        };
        if used + add + reserve > max {
            break;
        }
        if !out.is_empty() {
            out.push_str(SEP);
        }
        out.push_str(tok);
        used += add;
        shown += 1;
    }
    if shown < tokens.len() && shown > 0 {
        out.push_str(ellipsis);
    }
    out
}

/// Render everything on one line (for statusbar="bottom"/"top"). Concatenates context + footer.
pub fn render_combined(frame: &mut Frame, app: &App, area: Rect) {
    let mut spans = context_spans(app);
    let used: usize = spans.iter().map(|s| s.width()).sum();
    spans.push(Span::from("  "));
    let rest = area.width.saturating_sub(used as u16 + 2);
    spans.extend(footer_spans(app, rest));
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// Render the context only (for the right side of the split header). Right-aligned.
pub fn render_context(frame: &mut Frame, app: &App, area: Rect) {
    let p =
        Paragraph::new(Line::from(context_spans(app))).alignment(ratatui::layout::Alignment::Right);
    frame.render_widget(p, area);
}

/// Render the footer only (key hints, etc.) (for the bottom row of the split).
pub fn render_footer(frame: &mut Frame, app: &App, area: Rect) {
    frame.render_widget(
        Paragraph::new(Line::from(footer_spans(app, area.width))),
        area,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    #[test]
    fn markdown_preview_footer_shows_link_keys() {
        // Markdown プレビューのフッターにはリンク操作(Tab=フォーカス / ↵=開く)を出す。
        // 普通のテキスト/コードには出さない(リンクが無いため)。
        let dir = std::env::temp_dir().join("konoma_md_hints_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.md"), b"[x](https://e.com)\n").unwrap();
        std::fs::write(dir.join("b.txt"), b"plain text\n").unwrap();
        let mut app = App::new(dir.canonicalize().unwrap(), Config::default()).unwrap();

        // a.md をプレビュー → リンク操作あり。
        app.selected = app
            .entries
            .iter()
            .position(|e| e.path.ends_with("a.md"))
            .unwrap();
        app.tree_activate().unwrap();
        let md = hint_tokens(&app).join(" ");
        assert!(md.contains("Tab:link"), "md にリンク操作が無い: {md}");
        assert!(md.contains("↵:open"), "md に開く操作が無い: {md}");

        // b.txt をプレビュー → リンク操作なし(通常のテキストヒント)。
        app.selected = app
            .entries
            .iter()
            .position(|e| e.path.ends_with("b.txt"))
            .unwrap();
        app.tree_activate().unwrap();
        let txt = hint_tokens(&app).join(" ");
        assert!(
            !txt.contains("Tab:link") && !txt.contains("↵:open"),
            "テキストにリンク操作が出ている: {txt}"
        );
        assert!(txt.contains("hl:"), "テキストは横移動ヒントを出す: {txt}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn fit_tokens_priority_and_truncation() {
        let toks: Vec<String> = ["aaa", "bbb", "ccc", "ddd"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        // 十分広い → 全部、省略記号なし。
        let all = fit_tokens(&toks, 80);
        assert_eq!(all, "aaa  bbb  ccc  ddd");
        // 狭い → 先頭優先で入る分だけ＋ " …"。
        let some = fit_tokens(&toks, 12);
        assert!(some.starts_with("aaa"), "先頭優先: {some}");
        assert!(some.ends_with('…'), "続きありの省略記号: {some}");
        assert!(!some.contains("ddd"), "末尾は落ちる: {some}");
        // 幅0でもパニックしない。
        assert_eq!(fit_tokens(&toks, 0), "");
    }

    #[test]
    fn footer_shows_whichkey_menu_while_leader_pending() {
        let dir = std::env::temp_dir().join("konoma_whichkey_footer_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut app = App::new(dir.clone(), Config::default()).unwrap();
        // リーダー未押下: which-key は出ない(footer は通常ヒント)。
        assert!(whichkey_spans(&app).is_none(), "未押下では None");

        // パスコピーリーダー(y)押下相当: pending_leader をセット → which-key 見出し+候補。
        app.pending_leader = Some(crate::keymap::LeaderId::Copy);
        let spans = footer_spans(&app, 120);
        let text: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains('▸'), "which-key 見出しが無い: {text}");
        assert!(text.contains(':'), "key:label 形式の候補が無い: {text}");
        // 直接 whichkey_spans も Some(見出し + 候補の2 span)。
        let direct = whichkey_spans(&app).expect("リーダー中は Some");
        assert_eq!(direct.len(), 2, "見出し span + 候補 span");

        // flash があれば which-key より flash が優先される(footer_spans の分岐確認)。
        app.flash = Some("hello flash".into());
        let f: String = footer_spans(&app, 120)
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        assert!(
            f.contains("hello flash") && !f.contains('▸'),
            "flash 優先: {f}"
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
