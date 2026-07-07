//! Centered popup for the tab list (`T`). One row per tab: number, display name (tree = root dir /
//! preview = shown file) and the root path (`~`-shortened). The active tab is marked with `●`, the
//! selected row is reversed. `1-9`/`Enter` switch, `w` closes the selected tab, `T`/`q`/`Esc` close
//! the list. Follows the bookmark-list popup.

use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::Line;
use ratatui::widgets::{Block, Clear, Paragraph};
use ratatui::Frame;

use crate::app::App;
use crate::i18n::tr;

pub fn render(frame: &mut Frame, app: &App, area: Rect) {
    let sel = app.tab_list_sel();
    let active = app.active_tab_index();
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut sel_line: u16 = 0;

    for i in 0..app.tab_count() {
        let marker = if i == active { "●" } else { " " };
        let root = crate::app::home_relative(&app.tab_root(i));
        let row = format!(" {marker} {}:{}  —  {root}", i + 1, app.tab_label(i));
        if i == sel {
            sel_line = lines.len() as u16;
            lines.push(Line::from(row).reversed());
        } else {
            lines.push(Line::from(row));
        }
    }
    lines.push(Line::from(""));
    lines.push(Line::from(tr(app.lang, crate::i18n::Msg::TabsActions)).dim());

    // ポップアップ (bookmarks を踏襲)。幅最大66、高さ=内容+枠、画面に収める。
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

    let block = Block::bordered()
        .title(tr(app.lang, crate::i18n::Msg::TabsTitle))
        .border_style(Style::new().fg(Color::Cyan));
    let para = Paragraph::new(lines)
        .block(block)
        .scroll((scroll, 0))
        .alignment(Alignment::Left);

    frame.render_widget(Clear, popup);
    frame.render_widget(para, popup);
}
