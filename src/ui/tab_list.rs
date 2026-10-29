//! Centered popup for the tab list (`T`). One row per tab: number, display name (tree = root dir /
//! preview = shown file) and the root path (`~`-shortened). The active tab is marked with `●`, the
//! selected row is reversed. `1-9`/`Enter` switch, `w` closes the selected tab, `T`/`q`/`Esc` close
//! the list. Follows the bookmark-list popup.

use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::Line;
use ratatui::widgets::{Block, Paragraph};
use ratatui::Frame;

use crate::app::list_filter::ListId;
use crate::app::App;
use crate::i18n::tr;

pub fn render(frame: &mut Frame, app: &App, area: Rect) {
    let sel = app.tab_list_sel();
    let active = app.active_tab_index();
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut sel_line: u16 = 0;

    // The `/` filter narrows the rows that are drawn; `sel` and `active` index all the tabs.
    let shown = app.list_visible(ListId::Tabs);
    for i in 0..app.tab_count() {
        if shown.as_ref().is_some_and(|v| v.binary_search(&i).is_err()) {
            continue;
        }
        let marker = if i == active { "●" } else { " " };
        let row = format!(" {marker} {}", app.tab_row_text(i));
        if i == sel {
            sel_line = lines.len() as u16;
            lines.push(Line::from(row).reversed());
        } else {
            lines.push(Line::from(row));
        }
    }
    lines.push(Line::from(""));
    lines.push(Line::from(tr(app.lang, crate::i18n::Msg::TabsActions)).dim());

    // A popup (follows bookmarks). Width up to 66, height = content + border, fits within the screen.
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

    // Scroll vertically so the selected row is visible.
    let inner_h = h.saturating_sub(2);
    let max_scroll = (lines.len() as u16).saturating_sub(inner_h);
    let scroll = sel_line
        .saturating_sub(inner_h.saturating_sub(1))
        .min(max_scroll);

    // A query shows as " /<query>" after the title.
    let q = app.list_filter_query(ListId::Tabs);
    let title = if q.is_empty() {
        tr(app.lang, crate::i18n::Msg::TabsTitle).to_string()
    } else {
        format!(
            "{} /{q} ",
            tr(app.lang, crate::i18n::Msg::TabsTitle).trim_end()
        )
    };
    let block = Block::bordered()
        .title(title)
        .border_style(Style::new().fg(Color::Cyan));
    let para = Paragraph::new(lines)
        .block(block)
        .scroll((scroll, 0))
        .alignment(Alignment::Left);

    konoma_widgets::overlay::clear_overlay(frame.buffer_mut(), popup);
    frame.render_widget(para, popup);
}
