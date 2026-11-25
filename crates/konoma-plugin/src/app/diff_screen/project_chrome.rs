//! Project-owned diff menus. Ordinary standalone file screens do not enter this adapter.
use super::*;
use konoma_host_core::{
    help::{help_key, menu_view},
    keymap::{CoreMenu, CoreResolution},
};

impl DiffScreenState {
    pub(super) fn actions_menu(&self) -> Vec<konoma_host_core::menu::Item<DiffUiOp, String>> {
        self.presentation().actions_menu(&self.keys, self.lang)
    }

    pub(super) fn project_menu_key(
        &mut self,
        key: KeyPress,
    ) -> Result<Option<(DiffScreenAction, Option<String>)>, Failure> {
        if let Some(menu) = self.extra_menu.take() {
            if menu == CoreMenu::Actions {
                if let Some(item) = self.actions_menu().into_iter().find(|i| i.key == key) {
                    return self.execute_op(item.action).map(Some);
                }
            } else if let Some(view) =
                menu_view(&self.keys, CoreContext::Diff, menu, self.lang, false)
            {
                if let Some(CoreAction::CopyPath(kind)) = view.find(key) {
                    return Ok(Some((DiffScreenAction::Copy(kind), None)));
                }
            }
            return Ok(Some((DiffScreenAction::Stay, None)));
        }
        if self.help.is_none() && !self.menu {
            if let CoreResolution::Menu(menu @ (CoreMenu::Actions | CoreMenu::Copy)) =
                self.keys.resolve(CoreContext::Diff, key)
            {
                self.extra_menu = Some(menu);
                return Ok(Some((DiffScreenAction::Stay, None)));
            }
        }
        Ok(None)
    }

    pub(super) fn extra_menu_footer(
        &self,
        width: u16,
    ) -> Option<Vec<ratatui::text::Span<'static>>> {
        let menu = self.extra_menu?;
        let (title, items) = if menu == CoreMenu::Actions {
            (
                CoreMsg::WkActionsTitle.tr(self.lang).to_owned(),
                self.actions_menu()
                    .into_iter()
                    .map(|i| (help_key(i.key), i.label))
                    .collect::<Vec<_>>(),
            )
        } else {
            let view = menu_view(&self.keys, CoreContext::Diff, menu, self.lang, false)?;
            (
                view.title,
                view.items
                    .into_iter()
                    .map(|i| (help_key(i.key), i.label))
                    .collect(),
            )
        };
        Some(konoma_widgets::footer_line::which_key_spans(
            &title, &items, width,
        ))
    }
}
