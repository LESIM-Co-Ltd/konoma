//! Terminal chrome and overlay input adapters; all project entries come from the presentation.
use super::*;
use crate::host::project::draw::{draw_project_dialog, project_dialog_footer};
use konoma_host_core::keymap::CoreMenu;
use ratatui::layout::Alignment;
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};

impl ProjectLoopState {
    pub(super) fn presentation_key(
        &mut self,
        prepared: &mut PreparedProjects,
        info: &ProjectContextInfo<StandaloneSurface>,
        key: KeyEvent,
    ) -> bool {
        let kp = KeyPress::norm(&key);
        if let Some(menu) = self.menu.take() {
            let id = self.presentation.as_ref().and_then(|p| {
                let index = match menu {
                    CoreMenu::Copy => p.project.copy_action(kp),
                    CoreMenu::Actions => p
                        .project
                        .actions_menu()
                        .iter()
                        .find(|i| i.key == kp)
                        .map(|i| i.action),
                    _ => None,
                }?;
                p.actions.get(index).cloned()
            });
            if let Some(id) = id {
                self.dispatch_act(prepared, info, id);
            }
            return true;
        }
        if let Some(top) = self.help.as_mut() {
            let resolution = match key.code {
                KeyCode::Esc if key.modifiers.is_empty() => {
                    CoreResolution::Action(CoreAction::PreviewBack)
                }
                KeyCode::Down => CoreResolution::Action(CoreAction::Navigate(Motion::Down)),
                KeyCode::Up => CoreResolution::Action(CoreAction::Navigate(Motion::Up)),
                KeyCode::Home => CoreResolution::Action(CoreAction::Navigate(Motion::Top)),
                KeyCode::End => CoreResolution::Action(CoreAction::Navigate(Motion::Bottom)),
                _ => self.keys.help_resolve(kp),
            };
            match resolution {
                CoreResolution::Action(CoreAction::ToggleHelp | CoreAction::PreviewBack) => {
                    self.help = None
                }
                CoreResolution::Action(CoreAction::Navigate(m)) => match m {
                    Motion::Down => *top = top.saturating_add(1),
                    Motion::Up => *top = top.saturating_sub(1),
                    Motion::Top => *top = 0,
                    Motion::Bottom => *top = u16::MAX,
                    _ => {}
                },
                _ => {}
            }
            return true;
        }
        if self
            .screen
            .as_ref()
            .and_then(|n| n.frame())
            .is_some_and(|f| f.filtering)
        {
            return false;
        }
        match self.keys.resolve(CoreContext::Canvas, kp) {
            CoreResolution::Action(CoreAction::ToggleHelp) => {
                self.help = Some(0);
                true
            }
            CoreResolution::Menu(menu @ (CoreMenu::Copy | CoreMenu::Actions)) => {
                if self.presentation.as_ref().is_some_and(|p| match menu {
                    CoreMenu::Copy => !p.project.copy_menu().is_empty(),
                    _ => !p.project.actions_menu().is_empty(),
                }) {
                    self.menu = Some(menu);
                }
                true
            }
            _ => false,
        }
    }

    pub(super) fn draw_chrome(
        &mut self,
        prepared: &PreparedProjects,
        buf: &mut ratatui::buffer::Buffer,
        area: Rect,
    ) {
        use konoma_host_core::help_msg::CoreMsg;
        use konoma_widgets::{footer_line, help_panel};
        let t = |en, ja| if prepared.lang == "ja" { ja } else { en };
        let mut header = vec![Span::styled(
            format!(" {} ", CoreMsg::ProjectTitle.tr(prepared.keys.lang())),
            Style::new().fg(Color::Black).bg(Color::Cyan).bold(),
        )];
        if self.diff.as_ref().is_some_and(|d| d.closed) {
            header.extend([
                Span::raw(" "),
                Span::styled(
                    format!(" {} ", t("DIFF", "差分")),
                    Style::new().fg(Color::White).bg(Color::Blue).bold(),
                ),
            ]);
        }
        if self.help.is_some() {
            header.extend([
                Span::raw(" "),
                Span::styled(
                    format!(" {} ", t("HELP", "ヘルプ")),
                    Style::new().fg(Color::White).bg(Color::DarkGray).bold(),
                ),
            ]);
        }
        let path = match prepared.display.path_style {
            konoma_host_core::view_options::PathStyle::Home => "~",
            konoma_host_core::view_options::PathStyle::Full => t("abs", "絶対"),
            _ => t("rel", "相対"),
        };
        header.push(Span::raw(format!("  {}{path}", t("path:", "パス:"))).dim());
        if area.height > 0 {
            Paragraph::new(Line::from(header))
                .alignment(Alignment::Right)
                .render(Rect::new(area.x, area.y, area.width, 1), buf);
        }
        let dialog = self.dialog_presentation(prepared);
        let spans = if let Some(note) = &prepared.notice {
            footer_line::flash_spans(note)
        } else if let Some(menu) = self.menu {
            let items = self
                .presentation
                .as_ref()
                .map(|p| match menu {
                    CoreMenu::Copy => p.project.copy_menu(),
                    _ => p.project.actions_menu(),
                })
                .unwrap_or_default();
            footer_line::which_key_spans(
                if menu == CoreMenu::Copy {
                    CoreMsg::HintCopy.tr(prepared.keys.lang())
                } else {
                    CoreMsg::WkActionsTitle.tr(prepared.keys.lang())
                },
                &items
                    .iter()
                    .map(|i| (konoma_host_core::help::help_key(i.key), i.label.clone()))
                    .collect::<Vec<_>>(),
                area.width,
            )
        } else if let Some(dialog) = &dialog {
            project_dialog_footer(dialog, area.width)
        } else if self.help.is_some() {
            vec![Span::raw(help_panel::fit_tokens(
                &prepared.keys.help_footer_tokens(),
                area.width,
            ))
            .bold()]
        } else if let Some(frame) = self
            .screen
            .as_ref()
            .and_then(|n| n.frame())
            .filter(|f| f.filtering)
        {
            vec![Span::raw(format!(
                "/{}▏  {}",
                frame.query,
                t("Enter:apply Esc:clear", "Enter:確定 Esc:解除")
            ))]
        } else {
            vec![Span::raw(help_panel::fit_tokens(
                &self
                    .presentation
                    .as_ref()
                    .map(|p| {
                        p.project
                            .footer_tokens()
                            .iter()
                            .map(|t| t.text.clone())
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default(),
                area.width,
            ))
            .dim()]
        };
        if area.height > 0 {
            Paragraph::new(Line::from(spans))
                .render(Rect::new(area.x, area.bottom() - 1, area.width, 1), buf);
        }
        if let Some(dialog) = dialog {
            draw_project_dialog(buf, area, &dialog);
        }
        if let Some(top) = self.help.as_mut() {
            let sections = self
                .presentation
                .as_ref()
                .map(|p| p.help.as_slice())
                .unwrap_or_default();
            let lines = help_panel::help_lines(
                &sections
                    .iter()
                    .map(|s| (s.title.as_str(), s.rows.as_slice()))
                    .collect::<Vec<_>>(),
            );
            *top = (*top).min(help_panel::max_scroll(lines.len(), area));
            help_panel::render_panel(buf, area, lines, *top, prepared.keys.help_title());
        }
    }
}
