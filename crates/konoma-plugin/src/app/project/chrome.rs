//! Terminal chrome and overlay input adapters; all project entries come from the presentation.
use super::*;
use crate::host::project::draw::{draw_project_dialog, project_dialog_footer};
use konoma_host_core::keymap::CoreMenu;
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::Span;

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
        // A git or jj project names its backend and screen, as the host's status bar does (the
        // same derivation); any other provider keeps the generic project chip.
        let vcs = self
            .screen
            .as_ref()
            .and_then(|screen| screen.vcs_chips(prepared.lang == "ja"));
        let vcs_chip = |chip: &crate::host::project::vcs_chips::VcsChip| {
            let fg = if chip.dark {
                Color::White
            } else {
                Color::Black
            };
            Span::styled(
                format!(" {} ", chip.text),
                Style::new().fg(fg).bg(chip.bg).bold(),
            )
        };
        let mut header = vec![match &vcs {
            Some(chips) => vcs_chip(&chips.backend),
            None => Span::styled(
                format!(" {} ", CoreMsg::ProjectTitle.tr(prepared.keys.lang())),
                Style::new().fg(Color::Black).bg(Color::Cyan).bold(),
            ),
        }];
        let diff_open = self.diff.as_ref().is_some_and(|d| d.closed)
            || (self.preview_project && self.file_preview.is_none());
        // Help and the diff view are modes in front of the screen, and take its place as the
        // inner chip (the host shows the mode in front, not the screen behind it).
        if let Some(screen) = vcs
            .as_ref()
            .and_then(|chips| chips.screen.as_ref())
            .filter(|_| self.help.is_none() && !diff_open)
        {
            header.extend([Span::raw(" "), vcs_chip(screen)]);
        }
        if diff_open {
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
        if let Some(origin) = &prepared.worktree_origin {
            use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};
            let origin = if origin.width() <= 20 {
                origin.clone()
            } else {
                let mut used = 0;
                let mut label: String = origin
                    .chars()
                    .take_while(|c| {
                        used += c.width().unwrap_or(0);
                        used <= 19
                    })
                    .collect();
                label.push('…');
                label
            };
            header.extend([
                Span::raw(" "),
                Span::styled(" WT ", Style::new().fg(Color::Black).bg(Color::Cyan).bold()),
                Span::raw(format!(" {origin}")).dim(),
            ]);
        }
        let path = match prepared.display.path_style {
            konoma_host_core::view_options::PathStyle::Home => "~",
            konoma_host_core::view_options::PathStyle::Full => t("abs", "絶対"),
            _ => t("rel", "相対"),
        };
        header.push(Span::raw(format!("  {}{path}", t("path:", "パス:"))).dim());
        let combined = prepared.services.project_diff_chrome() && prepared.display.combined_status;
        let footer_width = crate::app::draw::status_footer_width(area.width, combined, &header);
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
                footer_width,
            )
        } else if let Some(dialog) = &dialog {
            project_dialog_footer(dialog, footer_width)
        } else if self.help.is_some() {
            vec![Span::raw(help_panel::fit_tokens(
                &prepared.keys.help_footer_tokens(),
                footer_width,
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
                footer_width,
            ))
            .dim()]
        };
        crate::app::draw::draw_project_status(buf, area, combined, header, &spans);
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
