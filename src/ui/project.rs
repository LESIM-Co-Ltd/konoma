//! A project's declared tabs and its existing list, text, diff or document widget.

use ratatui::layout::Rect;
use ratatui::style::Stylize;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use ratatui::Frame;

use crate::app::project_screens::ProjectContent;
use crate::app::App;
use crate::i18n::{tr, Msg};

pub(super) fn render(frame: &mut Frame, app: &mut App, area: Rect) {
    let body = Rect {
        y: area.y.saturating_add(1),
        height: area.height.saturating_sub(2),
        width: area.width.saturating_sub(2),
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
    let screen = runtime
        .project
        .declaration
        .screens
        .iter()
        .find(|s| s.id == active.key.screen);
    let marks = screen.is_some_and(|s| s.marks);
    let checks = screen.is_some_and(|s| s.checks);
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
                    None => "    ",
                };
                let mut spans = Vec::new();
                if marks {
                    spans.push(Span::raw(format!("{mark} ")));
                }
                if checks {
                    spans.push(Span::raw(check));
                }
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
                let spans = if let Some(right) = &row.right {
                    let right = konoma_plugin::ratatui_draw::into_line(right.clone());
                    let right = right
                        .spans
                        .into_iter()
                        .map(|mut span| {
                            span.style = right.style.patch(span.style);
                            span
                        })
                        .collect();
                    line_with_right(spans, right, usize::from(body.width))
                } else {
                    spans
                };
                let line = Line::from(spans);
                lines.push(if row.selectable && cursor == active.cursor {
                    line.reversed()
                } else {
                    line
                });
            }
        }
        ProjectContent::Lines(text) | ProjectContent::Diff(text) => lines.clone_from(text),
    }
    let cursor = active.cursor.min(lines.len().saturating_sub(1));
    let scroll = if body.height == 0 {
        0
    } else {
        cursor.saturating_sub(usize::from(body.height - 1))
    };
    if let Some(frame) = app
        .tab
        .project_navigation
        .as_mut()
        .and_then(|nav| nav.stack.last_mut())
    {
        frame.cursor = cursor;
    }
    super::list::render(
        frame,
        body,
        lines,
        scroll.min(u16::MAX as usize) as u16,
        Block::default(),
    );
}

/// Ellipsize by display width, retaining each span's style and the legacy plain ellipsis.
fn truncate_spans(spans: Vec<Span<'static>>, max: usize) -> Vec<Span<'static>> {
    use unicode_width::UnicodeWidthChar;
    if spans.iter().map(Span::width).sum::<usize>() <= max {
        return spans;
    }
    if max == 0 {
        return Vec::new();
    }
    let budget = max - 1;
    let mut out = Vec::new();
    let mut used = 0;
    for span in spans {
        let mut text = String::new();
        for ch in span.content.chars() {
            let width = ch.width().unwrap_or(0);
            if used + width > budget {
                if !text.is_empty() {
                    out.push(Span::styled(text, span.style));
                }
                out.push(Span::raw("…"));
                return out;
            }
            text.push(ch);
            used += width;
        }
        if !text.is_empty() {
            out.push(Span::styled(text, span.style));
        }
    }
    out
}

/// The suffix is optional when narrow; its styles are entirely plugin-owned.
fn line_with_right(
    left: Vec<Span<'static>>,
    right: Vec<Span<'static>>,
    width: usize,
) -> Vec<Span<'static>> {
    let right_width = right.iter().map(Span::width).sum::<usize>();
    if right_width == 0 || right_width + 2 > width {
        return truncate_spans(left, width);
    }
    let mut out = truncate_spans(left, width - right_width - 1);
    let left_width = out.iter().map(Span::width).sum::<usize>();
    out.push(Span::raw(
        " ".repeat(width.saturating_sub(left_width + right_width).max(1)),
    ));
    out.extend(right);
    out
}
