//! Centered popup for confirm/input dialogs (M7 Phase B).
//! Confirm = the delete y/!/n; Input = name entry for create/rename (a block cursor at the cursor position). Follows bookmarks/help.

use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};
use ratatui::Frame;

use crate::app::App;
use crate::i18n::tr;

pub fn render(frame: &mut Frame, app: &App, area: Rect) {
    // 一括リネームのプレビューは専用描画(旧 → 新 の一覧 + スクロール)。
    if let Some((title, pairs, scroll)) = app.dialog_preview_view() {
        render_preview(frame, app, area, title, pairs, scroll);
        return;
    }
    let Some((is_confirm, head, buffer, cursor)) = app.dialog_view() else {
        return;
    };

    let mut lines: Vec<Line<'static>> = Vec::new();
    // head は複数行(改行区切り)になり得る(例: ブックマーク上書き確認の old → new)。
    // 1 行目=見出し(太字)、以降=詳細(通常)として1行ずつ積む。
    for (i, l) in head.split('\n').enumerate() {
        if i == 0 {
            lines.push(Line::from(l.to_string()).bold());
        } else {
            lines.push(Line::from(l.to_string()));
        }
    }
    lines.push(Line::from(""));

    let (title, accent) = if is_confirm {
        if app.confirm_is_drop() {
            // D&D 転送: c=コピー(緑) / m=移動(黄) / n,Esc=取消。破壊的でないのでシアン。
            lines.push(Line::from(tr(app.lang, crate::i18n::Msg::DlgCopyKey)).fg(Color::Green));
            lines.push(Line::from(tr(app.lang, crate::i18n::Msg::DlgMove)).fg(Color::Yellow));
            lines.push(Line::from(tr(app.lang, crate::i18n::Msg::DlgCancel)).dim());
            (tr(app.lang, crate::i18n::Msg::DlgDropTitle), Color::Cyan)
        } else if app.dialog_allow_permanent() {
            // 破壊操作=赤系。y/!/n は文脈で文言を変える(ファイル削除 or ブランチ削除)。
            let (y_label, bang_label) = if app.confirm_is_branch_delete() {
                (
                    tr(app.lang, crate::i18n::Msg::DlgDeleteSafe),
                    tr(app.lang, crate::i18n::Msg::DlgForceDeleteHint),
                )
            } else {
                (
                    tr(app.lang, crate::i18n::Msg::DlgTrash),
                    tr(app.lang, crate::i18n::Msg::DlgDeletePermanentHint),
                )
            };
            lines.push(Line::from(y_label).dim());
            lines.push(Line::from(bang_label).fg(Color::Red));
            lines.push(Line::from(tr(app.lang, crate::i18n::Msg::DlgCancel)).dim());
            (tr(app.lang, crate::i18n::Msg::DlgConfirmTitle), Color::Red)
        } else if app.confirm_is_quit() {
            // アプリ終了確認: 非破壊なので黄(削除の赤と区別)。本文(head)に "Quit konoma?" を表示。
            lines.push(Line::from(tr(app.lang, crate::i18n::Msg::StQuitHint)).dim());
            (
                tr(app.lang, crate::i18n::Msg::DlgConfirmTitle),
                Color::Yellow,
            )
        } else if app.confirm_is_bookmark() {
            // ブックマーク上書き確認: 非破壊なので黄。y/Enter=上書き / n/Esc=取消。
            lines.push(Line::from(tr(app.lang, crate::i18n::Msg::StMarkOverwriteHint)).dim());
            (
                tr(app.lang, crate::i18n::Msg::DlgConfirmTitle),
                Color::Yellow,
            )
        } else {
            lines.push(Line::from(tr(app.lang, crate::i18n::Msg::DlgYesNo)).dim());
            (tr(app.lang, crate::i18n::Msg::DlgConfirmTitle), Color::Red)
        }
    } else {
        // 入力=シアン。入力行はカーソル位置に**ブロックカーソル**(反転1セル)を置く。
        // buffer を [前][カーソル位置の1文字][後] に分け、中央を反転表示する(末尾なら空白を反転)。
        let chars: Vec<char> = buffer.chars().collect();
        let cur = cursor.min(chars.len());
        let before: String = chars[..cur].iter().collect();
        let (at, after) = if cur < chars.len() {
            (chars[cur].to_string(), chars[cur + 1..].iter().collect())
        } else {
            (" ".to_string(), String::new())
        };
        lines.push(Line::from(vec![
            Span::raw("> "),
            Span::raw(before).fg(Color::White),
            Span::styled(at, Style::new().add_modifier(Modifier::REVERSED)),
            Span::raw(after).fg(Color::White),
        ]));
        // 入力系のキー(↵/Esc/←→)は下部フッターが正なので箱には出さない(下部フッターを正に)。
        (tr(app.lang, crate::i18n::Msg::DlgInputTitle), Color::Cyan)
    };

    // ポップアップ寸法 (bookmarks を踏襲)。
    let w = 66.min(area.width.saturating_sub(2)).max(24);
    let h = (lines.len() as u16 + 2)
        .min(area.height.saturating_sub(2))
        .max(3);
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    let popup = Rect {
        x,
        y,
        width: w,
        height: h,
    };

    let block = Block::bordered()
        .title(title)
        .border_style(Style::new().fg(accent));
    let para = Paragraph::new(lines)
        .block(block)
        .alignment(Alignment::Left);

    frame.render_widget(Clear, popup); // 下地を消してから描く
    frame.render_widget(para, popup);
}

/// Batch-rename preview (old → new list). Cyan border, scrollable, applied with y.
fn render_preview(
    frame: &mut Frame,
    app: &App,
    area: Rect,
    title: &str,
    pairs: &[String],
    scroll: usize,
) {
    // 見出し + 一覧 + ヒント。一覧は scroll から表示。
    let mut lines: Vec<Line<'static>> = Vec::new();
    lines.push(Line::from(title.to_string()).bold());
    lines.push(Line::from(""));
    for p in pairs.iter().skip(scroll) {
        lines.push(Line::from(p.clone()));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(tr(app.lang, crate::i18n::Msg::DlgApply)).dim());

    let w = 72.min(area.width.saturating_sub(2)).max(30);
    let h = (lines.len() as u16 + 2)
        .min(area.height.saturating_sub(2))
        .max(5);
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    let popup = Rect {
        x,
        y,
        width: w,
        height: h,
    };

    let block = Block::bordered()
        .title(tr(app.lang, crate::i18n::Msg::DlgRenamePreviewTitle))
        .border_style(Style::new().fg(Color::Cyan));
    let para = Paragraph::new(lines)
        .block(block)
        .alignment(Alignment::Left);

    frame.render_widget(Clear, popup);
    frame.render_widget(para, popup);
}
