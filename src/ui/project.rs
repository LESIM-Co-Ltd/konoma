//! A project's declared tabs and its existing list, text, diff or document widget.

use ratatui::layout::Rect;
use ratatui::style::{Color, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use ratatui::Frame;

use crate::app::project_screens::ProjectContent;
use crate::app::App;
use crate::i18n::{tr, Msg};

pub(super) fn render(frame: &mut Frame, app: &mut App, area: Rect) {
    let body = Rect {
        y: area.y.saturating_add(1),
        height: area.height.saturating_sub(1),
        ..area
    };
    app.resize_project(body.width, body.height);
    let Some(nav) = &app.tab.project_navigation else {
        return;
    };
    let Some(runtime) = app.project_runtimes.get(&nav.project) else {
        return;
    };
    let Some(active) = nav.stack.last() else {
        return;
    };
    let mut tabs = Vec::new();
    for screen in &runtime.project.declaration.screens {
        let label = match screen.key {
            Some(key) => format!(" {key}:{} ", screen.title.get(app.plugin_lang())),
            None => format!(" {} ", screen.title.get(app.plugin_lang())),
        };
        let span = Span::raw(label);
        tabs.push(if screen.id == active.key.screen {
            span.reversed().bold()
        } else {
            span.dim()
        });
    }
    frame.render_widget(
        Paragraph::new(Line::from(tabs)),
        Rect {
            height: area.height.min(1),
            ..area
        },
    );
    let Some(content) = app.project_content() else {
        let message = runtime
            .error
            .as_ref()
            .map(|e| e.message.get(app.plugin_lang()))
            .unwrap_or(tr(app.lang, Msg::Loading));
        frame.render_widget(Paragraph::new(message), body);
        return;
    };
    let mut lines = Vec::new();
    let visible = app.project_visible_rows();
    match content.as_ref() {
        ProjectContent::List(list) => {
            for (cursor, index) in visible.iter().enumerate() {
                let row = &list.rows[*index];
                let mark = if active.marked.contains(index) {
                    "●"
                } else {
                    " "
                };
                let check = match row.checked {
                    Some(true) => "[x] ",
                    Some(false) => "[ ] ",
                    None => "",
                };
                let mut spans = vec![
                    Span::raw(format!("{mark} {check}")),
                    Span::raw(format!("{} ", row.kind)).fg(Color::DarkGray),
                ];
                for (cell, text) in row.cells.iter().enumerate() {
                    if cell != 0 {
                        spans.push(Span::raw("  "));
                    }
                    let line = konoma_plugin::ratatui_draw::into_line(text.clone());
                    spans.extend(line.spans.into_iter().map(|mut span| {
                        span.style = line.style.patch(span.style);
                        span
                    }));
                }
                let line = Line::from(spans);
                lines.push(if cursor == active.cursor {
                    line.reversed()
                } else {
                    line
                });
            }
        }
        ProjectContent::Lines(text) | ProjectContent::Diff(text) => lines.clone_from(text),
    }
    let cursor = active.cursor.min(lines.len().saturating_sub(1));
    let scroll = active
        .scroll
        .min(cursor)
        .max(cursor.saturating_sub(usize::from(body.height.saturating_sub(1))));
    if let Some(frame) = app
        .tab
        .project_navigation
        .as_mut()
        .and_then(|nav| nav.stack.last_mut())
    {
        frame.cursor = cursor;
        frame.scroll = scroll;
    }
    super::list::render(
        frame,
        body,
        lines,
        scroll.min(u16::MAX as usize) as u16,
        Block::default(),
    );
}
