//! Stateful help/menu adapters around the frame's shared presentation.
use super::*;
use konoma_host_core::{
    help::{help_key, menu_view, CoreSection},
    keymap::CoreMenu,
    menu::Item,
};

impl DiffScreenState {
    pub(super) fn options_declaration(
        &self,
    ) -> Vec<Item<DiffUiOp, crate::host::acts::FixedMenuLabel>> {
        let Some(menu) = menu_view(
            &self.keys,
            CoreContext::Diff,
            CoreMenu::Options,
            self.lang,
            false,
        ) else {
            return vec![];
        };
        menu.items
            .into_iter()
            .filter_map(|item| {
                Some(Item {
                    key: item.key,
                    action: DiffUiOp::from_core(HelpOp::Act(item.action))?,
                    label: crate::host::acts::FixedMenuLabel {
                        declared: Some(item.label),
                        fallback: None,
                    },
                })
            })
            .collect()
    }

    pub fn options_menu(&self) -> Vec<Item<DiffUiOp, String>> {
        self.presentation()
            .options_menu(&self.options_declaration())
    }

    pub fn help_sections(&self) -> Vec<CoreSection> {
        let menus = self.options_menu();
        let lookup = CoreLookup {
            keys: &self.keys,
            ctx: CoreContext::Diff,
        };
        let mut menus = lookup
            .keys_of(HelpOp::Menu(CoreMenu::Options))
            .first()
            .map(|key| DiffHelpMenu {
                title: (if self.project_owned {
                    CoreMsg::WkOptionsTitle
                } else {
                    CoreMsg::HintOptions
                })
                .tr(self.lang)
                .into(),
                prefix: help_key(*key),
                items: menus,
            })
            .into_iter()
            .collect::<Vec<_>>();
        if self.project_owned {
            if let Some(key) = lookup.keys_of(HelpOp::Menu(CoreMenu::Actions)).first() {
                menus.insert(
                    0,
                    DiffHelpMenu {
                        title: CoreMsg::WkActionsTitle.tr(self.lang).into(),
                        prefix: help_key(*key),
                        items: self.actions_menu(),
                    },
                );
            }
            if let (Some(key), Some(copy)) = (
                lookup.keys_of(HelpOp::Menu(CoreMenu::Copy)).first(),
                menu_view(
                    &self.keys,
                    CoreContext::Diff,
                    CoreMenu::Copy,
                    self.lang,
                    false,
                ),
            ) {
                menus.push(DiffHelpMenu {
                    title: copy.title,
                    prefix: help_key(*key),
                    items: copy
                        .items
                        .into_iter()
                        .enumerate()
                        .map(|(i, item)| Item {
                            key: item.key,
                            action: DiffUiOp::Host(100 + i),
                            label: item.label,
                        })
                        .collect(),
                });
            }
        }
        let mut sections = self.presentation().help_sections(&[], &menus);
        if self.project_owned {
            // General rows use the same core declaration as other project-owned previews.
            sections.extend(crate::app::looping::global_only(
                konoma_host_core::help::core_sections(
                    CoreContext::Diff,
                    &konoma_host_core::help::HelpFacts::default(),
                    &lookup,
                    self.lang,
                ),
                self.lang,
            ));
        }
        sections
    }

    pub(in crate::app) fn footer(
        &self,
        notice: Option<&str>,
        width: u16,
    ) -> Vec<ratatui::text::Span<'static>> {
        use konoma_widgets::{footer_line, help_panel};
        use ratatui::style::Stylize;
        if let Some(note) = notice {
            return footer_line::flash_spans(note);
        }
        if let Some(footer) = self.extra_menu_footer(width) {
            return footer;
        }
        if self.discard.confirming() {
            return vec![ratatui::text::Span::raw(self.word(
                "y:Trash  !:delete permanently  n/Esc:cancel",
                "y:ゴミ箱  !:完全削除  n/Esc:取消",
            ))
            .bold()];
        }
        if self.menu {
            return footer_line::which_key_spans(
                (if self.project_owned {
                    CoreMsg::WkOptionsTitle
                } else {
                    CoreMsg::HintOptions
                })
                .tr(self.lang),
                &self
                    .options_menu()
                    .iter()
                    .map(|i| (help_key(i.key), i.label.clone()))
                    .collect::<Vec<_>>(),
                width,
            );
        }
        if self.help.is_some() {
            return vec![ratatui::text::Span::raw(help_panel::fit_tokens(
                &self.app_keys.help_footer_tokens(),
                width,
            ))
            .bold()];
        }
        let tokens: Vec<_> = self
            .presentation()
            .footer_tokens()
            .iter()
            .map(|t| t.text.clone())
            .collect();
        vec![ratatui::text::Span::raw(help_panel::fit_tokens(&tokens, width)).bold()]
    }

    pub(in crate::app) fn draw_help(&mut self, buf: &mut Buffer, area: Rect) {
        let Some(top) = self.help else { return };
        let sections = self.help_sections();
        let lines = konoma_widgets::help_panel::help_lines(
            &sections
                .iter()
                .map(|s| (s.title.as_str(), s.rows.as_slice()))
                .collect::<Vec<_>>(),
        );
        let top = if self.project_owned {
            top.min(konoma_widgets::help_panel::max_scroll(lines.len(), area))
        } else {
            top
        };
        self.help = Some(top);
        konoma_widgets::help_panel::render_panel(buf, area, lines, top, self.app_keys.help_title());
    }
}
