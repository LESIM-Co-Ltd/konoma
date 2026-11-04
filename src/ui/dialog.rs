//! Centered popup for confirm/input dialogs (M7 Phase B).
//! Confirm = the delete y/!/n; Input = name entry for create/rename (a block cursor at the cursor position). Follows bookmarks/help.

use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use ratatui::Frame;

use crate::app::App;
use crate::i18n::tr;

pub fn render(frame: &mut Frame, app: &App, area: Rect) {
    if let Some(dialog) = &app.project_dialog {
        render_project(frame, app, area, dialog);
        return;
    }
    // The batch-rename preview uses dedicated rendering (an old → new list + scrolling).
    if let Some((title, pairs, scroll)) = app.dialog_preview_view() {
        render_preview(frame, app, area, title, pairs, scroll);
        return;
    }
    let Some((is_confirm, head, buffer, cursor)) = app.dialog_view() else {
        return;
    };

    let mut lines: Vec<Line<'static>> = Vec::new();
    // head can be multi-line (newline-separated) (e.g. old → new for a bookmark overwrite confirmation).
    // Line 1 = heading (bold), the rest = details (normal), stacked one line at a time.
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
            // D&D transfer: c = copy (green) / m = move (yellow) / n, Esc = cancel. Cyan since it's not destructive.
            lines.push(Line::from(tr(app.lang, crate::i18n::Msg::DlgCopyKey)).fg(Color::Green));
            lines.push(Line::from(tr(app.lang, crate::i18n::Msg::DlgMove)).fg(Color::Yellow));
            lines.push(Line::from(tr(app.lang, crate::i18n::Msg::DlgCancel)).dim());
            (tr(app.lang, crate::i18n::Msg::DlgDropTitle), Color::Cyan)
        } else if app.dialog_allow_permanent() {
            // Destructive operation = red-ish. y/!/n change wording by context (file delete or branch delete).
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
            // App-quit confirmation: yellow since it's non-destructive (distinct from delete's red). The body (head) shows "Quit konoma?".
            lines.push(Line::from(tr(app.lang, crate::i18n::Msg::StQuitHint)).dim());
            (
                tr(app.lang, crate::i18n::Msg::DlgConfirmTitle),
                Color::Yellow,
            )
        } else if app.confirm_is_bookmark() {
            // Bookmark overwrite confirmation: yellow since it's non-destructive. y/Enter = overwrite / n/Esc = cancel.
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
        // Input = cyan. The input line places a **block cursor** (one reversed cell) at the cursor position.
        // Split buffer into [before][the character at the cursor][after], and render the middle reversed (a space if at the end).
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
        // Input-related keys (↵/Esc/←→) are shown in the footer as the source of truth, so they're not drawn in the box (footer is authoritative).
        (tr(app.lang, crate::i18n::Msg::DlgInputTitle), Color::Cyan)
    };

    render_box(frame, area, lines, title, accent);
}

fn render_box(
    frame: &mut Frame,
    area: Rect,
    lines: Vec<Line<'static>>,
    title: &str,
    accent: Color,
) {
    render_box_selected(frame, area, lines, title, accent, 0);
}

fn render_box_selected(
    frame: &mut Frame,
    area: Rect,
    lines: Vec<Line<'static>>,
    title: &str,
    accent: Color,
    selected: usize,
) {
    // The legacy dialogs and project prompts share exactly the same frame.
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
        .scroll((
            selected
                .saturating_sub(usize::from(h.saturating_sub(3)))
                .min(u16::MAX as usize) as u16,
            0,
        ))
        .alignment(Alignment::Left);

    konoma_widgets::overlay::clear_overlay(frame.buffer_mut(), popup);
    frame.render_widget(para, popup);
}

fn render_project(
    frame: &mut Frame,
    app: &App,
    area: Rect,
    dialog: &crate::app::project_screens::ProjectDialog,
) {
    use crate::app::project_screens::{ProjectDialog, ProjectPrompt};
    use crate::i18n::Msg;
    let lang = app.plugin_lang();
    let mut lines = Vec::new();
    let mut selected_line = 0;
    let mut keys = Msg::ProjectDialogKeys;
    let title = match dialog {
        ProjectDialog::Projects { projects, selected } => {
            selected_line = *selected;
            for (index, project) in projects.iter().enumerate() {
                let name = app
                    .plugin_keys
                    .name_of(&project.plugin)
                    .map(|n| n.get(lang))
                    .unwrap_or(&project.plugin);
                let line = Line::raw(format!(
                    "{name}  {} ({})",
                    app.format_path(&project.root),
                    project.marker
                ));
                lines.push(if index == *selected {
                    line.reversed()
                } else {
                    line
                });
            }
            tr(app.lang, Msg::ProjectChoose)
        }
        ProjectDialog::Act {
            invocation,
            prompts,
            stage,
            buffer,
            cursor,
            selected,
            ..
        } => match &prompts[*stage] {
            ProjectPrompt::Confirmation(prompt) => {
                keys = Msg::ProjectConfirmCancel;
                lines.extend(
                    invocation
                        .confirmation_text(prompt, lang)
                        .lines()
                        .map(|l| Line::raw(l.to_string()).bold()),
                );
                lines.push(Line::raw(""));
                for choice in &prompt.choices {
                    lines.push(Line::raw(format!(
                        "{}: {}",
                        choice.key,
                        choice.choice.label.get(lang)
                    )));
                }
                tr(app.lang, Msg::DlgConfirmTitle)
            }
            ProjectPrompt::Text(prompt) => {
                lines.push(Line::raw(prompt.label.get(lang).to_string()).bold());
                let chars: Vec<_> = buffer.chars().collect();
                let at = (*cursor).min(chars.len());
                lines.push(Line::from(vec![
                    Span::raw("> "),
                    Span::raw(chars[..at].iter().collect::<String>()),
                    Span::raw(chars.get(at).unwrap_or(&' ').to_string()).reversed(),
                    Span::raw(chars.iter().skip(at + 1).collect::<String>()),
                ]));
                tr(app.lang, Msg::DlgInputTitle)
            }
            ProjectPrompt::Choice(prompt) => {
                selected_line = selected.saturating_add(1);
                lines.push(Line::raw(prompt.label.get(lang).to_string()).bold());
                for (index, choice) in prompt.choices.iter().enumerate() {
                    let line = Line::raw(choice.label.get(lang).to_string());
                    lines.push(if index == *selected {
                        line.reversed()
                    } else {
                        line
                    });
                }
                tr(app.lang, Msg::ProjectChoose)
            }
        },
    };
    lines.push(Line::raw(""));
    lines.push(Line::raw(tr(app.lang, keys)).dim());
    render_box_selected(frame, area, lines, title, Color::Cyan, selected_line);
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
    // Heading + list + hint. The list is displayed starting from scroll.
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

    konoma_widgets::overlay::clear_overlay(frame.buffer_mut(), popup);
    frame.render_widget(para, popup);
}
