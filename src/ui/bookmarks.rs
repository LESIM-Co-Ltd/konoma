//! Centered popup for the bookmark list (M7 auxiliary), opened directly by `'`. Shows local
//! (lowercase a-z) and global (uppercase A-Z) under separate headings, with the selected row
//! reversed. A plain letter jumps straight to that bookmark; `↵` = jump to selection /
//! `Ctrl-e` = edit / `Ctrl-d` = delete / `'`/`q`/`Esc` = close. Follows the help popup.

use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::Line;
use ratatui::widgets::{Block, Clear, Paragraph};
use ratatui::Frame;

use crate::app::App;
use crate::i18n::tr;

pub fn render(frame: &mut Frame, app: &App, area: Rect) {
    let items = app.bookmark_list_items(); // (is_local, key, path)
    let sel = app.bookmark_list_sel();
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut sel_line: u16 = 0;

    if items.is_empty() {
        lines.push(Line::from(tr(app.lang, crate::i18n::Msg::BmEmpty)).dim());
    } else {
        let mut last: Option<bool> = None;
        for (i, (is_local, key, path)) in items.iter().enumerate() {
            if last != Some(*is_local) {
                let header = if *is_local {
                    format!(
                        "{} ({})",
                        tr(app.lang, crate::i18n::Msg::BmLocal),
                        app.format_path(&app.open_dir)
                    )
                } else {
                    tr(app.lang, crate::i18n::Msg::BmGlobal).to_string()
                };
                lines.push(Line::from(header).bold().fg(Color::Cyan));
                last = Some(*is_local);
            }
            // ローカル=文脈相対表示 / グローバル=絶対(~短縮)表示。ツリー外を指すグローバルが
            // `../../..` になる読みにくさを避ける。
            let row = format!("  {key}  {}", app.bookmark_display_path(*is_local, path));
            if i == sel {
                sel_line = lines.len() as u16;
                lines.push(Line::from(row).reversed());
            } else {
                lines.push(Line::from(row));
            }
        }
    }
    lines.push(Line::from(""));
    lines.push(Line::from(tr(app.lang, crate::i18n::Msg::BmActions)).dim());

    // ポップアップ (help を踏襲)。幅最大66、高さ=内容+枠、画面に収める。
    let w = 66.min(area.width.saturating_sub(2)).max(20);
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

    // 選択行が見えるよう縦スクロール。
    let inner_h = h.saturating_sub(2);
    let max_scroll = (lines.len() as u16).saturating_sub(inner_h);
    let scroll = sel_line
        .saturating_sub(inner_h.saturating_sub(1))
        .min(max_scroll);

    let title = tr(app.lang, crate::i18n::Msg::BmTitle);
    let block = Block::bordered()
        .title(title)
        .border_style(Style::new().fg(Color::Cyan));
    let para = Paragraph::new(lines)
        .block(block)
        .scroll((scroll, 0))
        .alignment(Alignment::Left);

    frame.render_widget(Clear, popup); // 下地を消してから描く
    frame.render_widget(para, popup);
}
