//! Value-based project body and prompt drawing, without host state or I/O.

use std::collections::BTreeSet;

use super::{ProjectContent, ProjectDialog, ProjectPrompt, PromptInvocation};
use crate::host::bind::{jump_key_of, JumpBinding};
use crate::project::ProjectDecl;
use konoma_host_core::project::DiffLayout;
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Stylize},
    text::{Line, Span},
    widgets::{Block, Paragraph, Widget},
};

/// Draw-time facts; the owner retains the runtime, navigation and prepared content.
pub struct ProjectDrawValues<'a> {
    pub declaration: &'a ProjectDecl,
    pub active_screen: &'a str,
    pub cursor: usize,
    pub scroll: u16,
    pub hscroll: u16,
    pub marked: &'a BTreeSet<usize>,
    pub content: Option<&'a ProjectContent>,
    pub visible_rows: &'a [usize],
    pub layout: DiffLayout,
    pub lang: &'a str,
    /// The screen-jump operations with the keys they have now: the tab bar writes these keys.
    pub jumps: &'a [JumpBinding],
    /// Already localized loading or failure message.
    pub unavailable: &'a str,
    /// The active screen's filter query (empty when none): a list's heading shows it.
    pub query: &'a str,
}

/// Only corrected fields are returned. Lists retain their stored scroll and hscroll.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ProjectDrawUpdate {
    pub cursor: Option<usize>,
    pub scroll: Option<u16>,
    pub hscroll: Option<u16>,
}

/// The inside of the outer frame, the old hub's body geometry; also the worker viewport before
/// reserving chip rows. A list and a diff are drawn here.
pub fn project_body_area(area: Rect) -> Rect {
    Block::bordered().inner(area)
}

/// The body of content that has no frame (a text screen, the loading or failure message): one row
/// of tabs above it and no border. The old hub had no such screen, so this keeps its own look.
pub fn unframed_body_area(area: Rect) -> Rect {
    Rect {
        y: area.y.saturating_add(1),
        height: area.height.saturating_sub(2),
        width: area.width.saturating_sub(2),
        ..area
    }
}

/// A heading's line: the plugin's `before`, ` /query` while a query is set, `  (n)`, the plugin's
/// `after`. `n` counts the shown rows that count toward the heading.
fn heading_line(
    list: &crate::view::ListView,
    heading: &crate::view::ListHeading,
    query: &str,
    visible: &[usize],
) -> Line<'static> {
    let count = visible
        .iter()
        .filter(|index| {
            list.rows
                .get(**index)
                .is_some_and(|row| row.counts_toward_heading())
        })
        .count();
    fn push(spans: &mut Vec<Span<'static>>, line: crate::style::Line) {
        let line = crate::ratatui_draw::into_line(line);
        spans.extend(line.spans.into_iter().map(|mut span| {
            span.style = line.style.patch(span.style);
            span
        }));
    }
    let mut spans = Vec::new();
    push(&mut spans, heading.before.clone());
    let mut middle = String::new();
    if !query.is_empty() {
        middle.push_str(&" ".repeat(usize::from(heading.query_gap)));
        middle.push('/');
        middle.push_str(query);
    }
    middle.push_str(&format!("  ({count})"));
    spans.push(Span::raw(middle));
    push(&mut spans, heading.after.clone());
    Line::from(spans)
}

pub fn draw_project(
    buf: &mut Buffer,
    area: Rect,
    values: ProjectDrawValues<'_>,
) -> ProjectDrawUpdate {
    let framed = matches!(
        values.content,
        Some(ProjectContent::List(_) | ProjectContent::Diff(_))
    );
    let mut body = if framed {
        project_body_area(area)
    } else {
        unframed_body_area(area)
    };
    let screen = values
        .declaration
        .screens
        .iter()
        .find(|s| s.id == values.active_screen);
    let marks = screen.is_some_and(|s| s.marks);
    let checks = screen.is_some_and(|s| s.checks);
    let mut tabs = Vec::new();
    for screen in values.declaration.screens.iter().filter(|s| !s.pushed) {
        // The key shown is the one that jumps now, not the declared default.
        let label = match jump_key_of(values.jumps, &screen.id).map(|k| k.code) {
            Some(crossterm::event::KeyCode::Char(key)) => {
                format!(" {key}:{} ", screen.title.get(values.lang))
            }
            _ => format!(" {} ", screen.title.get(values.lang)),
        };
        let span = Span::raw(label);
        tabs.push(if screen.id == values.active_screen {
            span.reversed().bold()
        } else {
            span.dim()
        });
    }
    let tab_width = tabs
        .iter()
        .map(Span::width)
        .sum::<usize>()
        .min(area.width as usize) as u16;
    let tabs = Line::from(tabs);
    let Some(content) = values.content else {
        let mut draw = |widget: Paragraph<'_>, area| widget.render(area, buf);
        draw(
            Paragraph::new(tabs),
            Rect {
                height: area.height.min(1),
                ..area
            },
        );
        draw(Paragraph::new(values.unavailable), body);
        return ProjectDrawUpdate::default();
    };
    if framed {
        // The outer frame carries the title at the left of its top edge and the screen tabs at the
        // right end. When both do not fit between the corners the tabs are dropped, never the title.
        let title = match content {
            ProjectContent::List(list) => match &list.heading {
                Some(heading) => Some(heading_line(
                    list,
                    heading,
                    values.query,
                    values.visible_rows,
                )),
                None => list
                    .title
                    .as_ref()
                    .map(|title| crate::ratatui_draw::into_line(title.clone())),
            },
            ProjectContent::Diff(diff) => diff.title.clone().map(|mut title| {
                // A side-by-side layout is marked after the title, as the old hub did
                // (` title ⇆ `).
                if values.layout.is_split(body.width) {
                    if let Some(last) = title.spans.last_mut() {
                        let kept = last.content.trim_end().to_owned();
                        last.content = kept.into();
                    }
                    title.spans.push(Span::raw(" ⇆ "));
                }
                title
            }),
            ProjectContent::Lines(_) => None,
        };
        let title_width = title.as_ref().map_or(0, Line::width);
        let mut block = Block::bordered();
        if let Some(title) = title {
            block = block.title(title);
        }
        block.render(area, buf);
        let room = usize::from(area.width.saturating_sub(2));
        if tab_width > 0 && title_width + usize::from(tab_width) <= room {
            Paragraph::new(tabs).render(
                Rect {
                    x: area.right().saturating_sub(1 + tab_width),
                    y: area.y,
                    width: tab_width,
                    height: area.height.min(1),
                },
                buf,
            );
        }
    } else {
        Paragraph::new(tabs).render(
            Rect {
                height: area.height.min(1),
                ..area
            },
            buf,
        );
    }
    let mut draw = |widget: Paragraph<'_>, area| widget.render(area, buf);
    if let ProjectContent::List(list) = content {
        if let Some(strip) = &list.bottom {
            let chip_lines = if strip.estimated_width <= usize::from(body.width.max(1)) {
                1
            } else {
                2
            };
            if !strip.chips.is_empty() && body.height >= 3 + chip_lines {
                body.height -= 1 + chip_lines;
                let divider = Rect {
                    y: body.y + body.height,
                    height: 1,
                    ..body
                };
                draw(
                    Paragraph::new(Line::from(
                        Span::raw("─".repeat(body.width as usize)).dark_gray(),
                    )),
                    divider,
                );
                let mut spans = Vec::new();
                for chip in &strip.chips {
                    if !spans.is_empty() {
                        spans.push(Span::raw("  "));
                    }
                    let line = crate::ratatui_draw::into_line(chip.clone());
                    spans.extend(line.spans.into_iter().map(|mut span| {
                        span.style = line.style.patch(span.style);
                        span
                    }));
                }
                draw(
                    Paragraph::new(Line::from(spans)).wrap(ratatui::widgets::Wrap { trim: true }),
                    Rect {
                        y: divider.y + 1,
                        height: chip_lines,
                        ..body
                    },
                );
            }
        }
    }
    let mut lines = Vec::new();
    let mut horizontal = values.hscroll;
    let mut kept_hscroll = horizontal;
    let mut diff_scroll = None;
    let is_list = matches!(content, ProjectContent::List(_));
    let visible = values.visible_rows;
    match content {
        ProjectContent::List(list) => {
            for (cursor, index) in visible.iter().enumerate() {
                let row = &list.rows[*index];
                let mark = if values.marked.contains(index) {
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
                    let line = crate::ratatui_draw::into_line(text.clone());
                    spans.extend(line.spans.into_iter().map(|mut span| {
                        span.style = line.style.patch(span.style);
                        span
                    }));
                }
                let spans = if let Some(right) = &row.right {
                    let right = crate::ratatui_draw::into_line(right.clone());
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
                lines.push(if row.selectable && cursor == values.cursor {
                    line.reversed()
                } else {
                    line
                });
            }
        }
        ProjectContent::Lines(text) => {
            lines.clone_from(text);
            let max = lines
                .iter()
                .map(Line::width)
                .max()
                .unwrap_or(0)
                .saturating_sub(body.width as usize)
                .min(u16::MAX as usize) as u16;
            horizontal = horizontal.min(max);
            kept_hscroll = horizontal;
        }
        ProjectContent::Diff(diff) => {
            if diff.lines.is_empty() && diff.header.is_empty() {
                draw(
                    Paragraph::new(diff.no_changes.clone())
                        .alignment(ratatui::layout::Alignment::Center),
                    Rect {
                        y: body.y + body.height / 2,
                        height: body.height.min(1),
                        ..body
                    },
                );
                return ProjectDrawUpdate {
                    cursor: None,
                    scroll: Some(0),
                    hscroll: Some(0),
                };
            }
            let split = values.layout.is_split(body.width);
            let (total, max_h) = diff.metrics(body.width, split);
            let scroll = usize::from(values.scroll)
                .min(total.saturating_sub(usize::from(body.height)))
                .min(u16::MAX as usize);
            horizontal = horizontal.min(max_h);
            kept_hscroll = horizontal;
            let drawn = diff.layout(
                body.width,
                split,
                horizontal,
                scroll,
                usize::from(body.height),
            );
            lines.clone_from(&drawn.lines);
            if split {
                horizontal = 0;
            }
            diff_scroll = Some(scroll);
        }
    }
    let cursor = values.cursor.min(lines.len().saturating_sub(1));
    let scroll = if let Some(scroll) = diff_scroll {
        scroll
    } else if !is_list {
        usize::from(values.scroll)
            .min(lines.len().saturating_sub(body.height as usize))
            .min(u16::MAX as usize)
    } else if body.height == 0 {
        0
    } else {
        cursor.saturating_sub(usize::from(body.height - 1))
    };
    let update = ProjectDrawUpdate {
        cursor: Some(cursor),
        scroll: (!is_list).then_some(scroll as u16),
        hscroll: (!is_list).then_some(kept_hscroll),
    };
    if !is_list {
        draw(
            Paragraph::new(lines).scroll((
                if diff_scroll.is_some() {
                    0
                } else {
                    scroll as u16
                },
                horizontal,
            )),
            body,
        );
        return update;
    }
    draw(
        Paragraph::new(lines)
            .block(Block::default())
            .scroll((scroll.min(u16::MAX as usize) as u16, 0))
            .alignment(ratatui::layout::Alignment::Left),
        body,
    );
    update
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

/// Host-resolved plugin name and display path, in the chooser's project order.
pub struct ProjectDialogLabel {
    pub name: String,
    pub path: String,
}

/// Translations remain the host's responsibility.
pub struct ProjectDialogWords<'a> {
    pub lang: &'a str,
    pub choose: &'a str,
    pub confirm_title: &'a str,
    pub input_title: &'a str,
    pub keys: &'a str,
    pub confirm_cancel: &'a str,
    pub confirm_cancel_status: &'a str,
}

/// One frame's derived prompt body and footer. A malformed stage has no visible box,
/// but retains the ordinary prompt key guide in the footer.
pub struct ProjectDialogPresentation {
    pub title: String,
    pub lines: Vec<Line<'static>>,
    pub accent: Color,
    pub selected_line: usize,
    pub visible: bool,
    footer: ProjectDialogFooter,
}

enum ProjectDialogFooter {
    Keys(String),
    Confirmation {
        choices: Vec<String>,
        cancel: String,
    },
}

#[cfg(any(test, feature = "test-hooks"))]
thread_local! {
    static DIALOG_BUILD_COUNT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(any(test, feature = "test-hooks"))]
pub fn project_dialog_presentation_build_count() -> usize {
    DIALOG_BUILD_COUNT.with(std::cell::Cell::get)
}

pub fn present_project_dialog<I: PromptInvocation>(
    dialog: &ProjectDialog<I>,
    resolved_project_labels: &[ProjectDialogLabel],
    words: &ProjectDialogWords<'_>,
) -> ProjectDialogPresentation {
    #[cfg(any(test, feature = "test-hooks"))]
    DIALOG_BUILD_COUNT.with(|count| count.set(count.get() + 1));
    let lang = words.lang;
    let mut presentation = ProjectDialogPresentation {
        title: words.choose.into(),
        lines: Vec::new(),
        accent: Color::Cyan,
        selected_line: 0,
        visible: true,
        footer: ProjectDialogFooter::Keys(words.keys.into()),
    };
    let lines = &mut presentation.lines;
    match dialog {
        ProjectDialog::Projects { projects, selected } => {
            presentation.selected_line = *selected;
            for (index, (project, label)) in
                projects.iter().zip(resolved_project_labels).enumerate()
            {
                let line = Line::raw(format!(
                    "{}  {} ({})",
                    label.name, label.path, project.marker
                ));
                lines.push(if index == *selected {
                    line.reversed()
                } else {
                    line
                });
            }
        }
        ProjectDialog::Act {
            invocation,
            prompts,
            stage,
            buffer,
            cursor,
            selected,
        } => {
            let invocation = invocation.prompt_invocation();
            match prompts.get(*stage) {
                Some(ProjectPrompt::Confirmation(prompt)) => {
                    lines.extend(
                        invocation
                            .confirmation_text(prompt, lang)
                            .lines()
                            .map(|l| Line::raw(l.to_string()).bold()),
                    );
                    lines.push(Line::raw(""));
                    for choice in &prompt.choices {
                        let line = Line::raw(format!(
                            "{} = {}",
                            choice.key,
                            choice.choice.label.get(lang)
                        ));
                        lines.push(if choice.destructive {
                            line.fg(Color::Red)
                        } else {
                            line.dim()
                        });
                    }
                    lines.push(Line::raw(words.confirm_cancel.to_string()).dim());
                    presentation.title = words.confirm_title.into();
                    presentation.accent = if invocation.destructive() {
                        Color::Red
                    } else {
                        Color::Cyan
                    };
                    presentation.footer = ProjectDialogFooter::Confirmation {
                        choices: prompt
                            .choices
                            .iter()
                            .map(|choice| {
                                format!(
                                    "{}:{}",
                                    choice.key,
                                    choice
                                        .status
                                        .as_ref()
                                        .unwrap_or(&choice.choice.label)
                                        .get(lang)
                                )
                            })
                            .collect(),
                        cancel: words.confirm_cancel_status.into(),
                    };
                    return presentation;
                }
                Some(ProjectPrompt::Text(prompt)) => {
                    lines.push(Line::raw(prompt.label.get(lang).to_string()).bold());
                    let chars: Vec<_> = buffer.chars().collect();
                    let at = (*cursor).min(chars.len());
                    lines.push(Line::from(vec![
                        Span::raw("> "),
                        Span::raw(chars[..at].iter().collect::<String>()),
                        Span::raw(chars.get(at).unwrap_or(&' ').to_string()).reversed(),
                        Span::raw(chars.iter().skip(at + 1).collect::<String>()),
                    ]));
                    presentation.title = words.input_title.into();
                }
                Some(ProjectPrompt::Choice(prompt)) => {
                    presentation.selected_line = selected.saturating_add(1);
                    lines.push(Line::raw(prompt.label.get(lang).to_string()).bold());
                    for (index, choice) in prompt.choices.iter().enumerate() {
                        let line = Line::raw(choice.label.get(lang).to_string());
                        lines.push(if index == *selected {
                            line.reversed()
                        } else {
                            line
                        });
                    }
                }
                None => {
                    presentation.visible = false;
                    return presentation;
                }
            }
        }
    }
    lines.push(Line::raw(""));
    lines.push(Line::raw(words.keys.to_string()).dim());
    presentation
}

pub fn draw_project_dialog(buf: &mut Buffer, area: Rect, presentation: &ProjectDialogPresentation) {
    if presentation.visible {
        konoma_widgets::dialog::draw_dialog_box(
            buf,
            area,
            presentation.lines.clone(),
            &presentation.title,
            presentation.accent,
            presentation.selected_line,
        );
    }
}

pub fn project_dialog_footer(
    presentation: &ProjectDialogPresentation,
    width: u16,
) -> Vec<Span<'static>> {
    use konoma_widgets::help_panel::fit_tokens;
    use unicode_width::UnicodeWidthStr;
    let text = match &presentation.footer {
        ProjectDialogFooter::Keys(keys) => keys.clone(),
        ProjectDialogFooter::Confirmation { choices, cancel } => {
            let room = usize::from(width);
            if room < cancel.width() {
                return vec![Span::raw(fit_tokens(&["Esc".into()], width)).dim()];
            }
            let choices = fit_tokens(choices, room.saturating_sub(cancel.width() + 2) as u16);
            if choices.is_empty() {
                cancel.clone()
            } else {
                format!("{choices}  {cancel}")
            }
        }
    };
    vec![Span::raw(text).dim()]
}
