// タブバー描画 (M4 / FR-5)。
//
// 複数のツリーコンテキストをタブとして上端に並べる。表示可否は ui::render が
// config `ui.tabbar` ("always" | "auto" | "hidden") を見て決め、ここは描画だけを担う。
// アクティブタブは反転＋太字。各タブは「番号:ディレクトリ名」。

use ratatui::layout::Rect;
use ratatui::style::Stylize;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::app::App;

pub fn render(frame: &mut Frame, app: &App, area: Rect) {
    let active = app.active_tab_index();
    let mut spans: Vec<Span<'static>> = Vec::new();
    for i in 0..app.tab_count() {
        if i > 0 {
            spans.push(Span::from(" "));
        }
        let chip = format!(" {}:{} ", i + 1, app.tab_label(i));
        if i == active {
            spans.push(Span::from(chip).reversed().bold());
        } else {
            spans.push(Span::from(chip).dim());
        }
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}
