//! Centered popup for confirm/input dialogs (M7 Phase B).
//! Confirm = the delete y/!/n; Input = name entry for create/rename (a block cursor at the cursor position). Follows bookmarks/help.

use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use ratatui::Frame;

use crate::app::App;
use crate::i18n::tr;

#[cfg(test)]
pub fn render(frame: &mut Frame, app: &App, area: Rect) {
    let presentation = project_presentation(app);
    render_with_project_dialog(frame, app, area, presentation.as_ref());
}

pub(super) fn render_with_project_dialog(
    frame: &mut Frame,
    app: &App,
    area: Rect,
    presentation: Option<&konoma_plugin::host::project::draw::ProjectDialogPresentation>,
) {
    if let Some(presentation) = presentation {
        konoma_plugin::host::project::draw::draw_project_dialog(
            frame.buffer_mut(),
            area,
            presentation,
        );
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
    konoma_widgets::dialog::draw_dialog_box(frame.buffer_mut(), area, lines, title, accent, 0);
}

/// Resolve paths and plugin names on the host; prompt drawing only sees values.
pub(super) fn project_presentation(
    app: &App,
) -> Option<konoma_plugin::host::project::draw::ProjectDialogPresentation> {
    use crate::app::project_screens::ProjectDialog;
    use crate::i18n::Msg;
    use konoma_plugin::host::project::draw::{
        present_project_dialog, ProjectDialogLabel, ProjectDialogWords,
    };
    let dialog = app.project_dialog.as_ref()?;
    let lang = app.plugin_lang();
    let labels = match dialog {
        ProjectDialog::Projects { projects, .. } => projects
            .iter()
            .map(|project| ProjectDialogLabel {
                name: app
                    .plugin_keys
                    .name_of(&project.plugin)
                    .map(|n| n.get(lang))
                    .unwrap_or(&project.plugin)
                    .to_string(),
                path: app.format_path(&project.root),
            })
            .collect(),
        _ => Vec::new(),
    };
    Some(present_project_dialog(
        dialog,
        &labels,
        &ProjectDialogWords {
            lang,
            choose: tr(app.lang, Msg::ProjectChoose),
            confirm_title: tr(app.lang, Msg::DlgConfirmTitle),
            input_title: tr(app.lang, Msg::DlgInputTitle),
            keys: tr(app.lang, Msg::ProjectDialogKeys),
            confirm_cancel: tr(app.lang, Msg::ProjectConfirmCancel),
            confirm_cancel_status: tr(app.lang, Msg::ProjectConfirmCancelStatus),
        },
    ))
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
