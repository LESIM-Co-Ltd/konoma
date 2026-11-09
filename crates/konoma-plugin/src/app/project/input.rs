//! Fresh action/key facts and adapters around the shared prompt and replay transitions.

use super::*;
use crate::host::project::actions::{action_key, acts_in};
use crate::host::project::draw::{present_project_dialog, ProjectDialogLabel, ProjectDialogWords};
use crate::project::ProjectAct;

impl ProjectLoopState {
    pub(super) fn actions(
        &self,
        prepared: &PreparedProjects,
        info: &ProjectContextInfo<StandaloneSurface>,
    ) -> Vec<ProjectAction> {
        let Some(nav) = &self.screen else {
            return Vec::new();
        };
        let Some(manifest) = prepared.registry.manifest(&nav.project.plugin) else {
            return Vec::new();
        };
        let (bound, _) = AppActs::from_config(
            &nav.project.plugin,
            manifest,
            &prepared.config.keys,
            &prepared.config.plugins,
        );
        let content = self.content(info);
        let scope = ProjectActionScope::Screen {
            plugin: nav.project.plugin.clone(),
            context: nav.act_context(content.as_deref()),
            diff: matches!(content.as_deref(), Some(ProjectContent::Diff(_))),
            jumps: self.runtimes[&nav.project]
                .project
                .declaration
                .screens
                .iter()
                .filter_map(|s| s.key)
                .collect(),
        };
        let actions: Vec<_> = manifest
            .actions
            .iter()
            .map(|a| {
                let key = bound
                    .bindings
                    .iter()
                    .find(|b| b.act == a.id)
                    .filter(|b| b.context.as_deref().is_none_or(|c| c == "canvas"))
                    .and_then(|b| b.key);
                let mut action = ProjectAction {
                    id: ProjectActionId {
                        plugin: nav.project.plugin.clone(),
                        action: a.id.clone(),
                    },
                    declaration: a.clone(),
                    key,
                };
                action.key = action.key.and_then(|k| action_key(&scope, &action, k));
                action
            })
            .collect();
        acts_in(&scope, &actions).into_iter().cloned().collect()
    }

    pub(super) fn act_facts(
        &self,
        prepared: &PreparedProjects,
        info: &ProjectContextInfo<StandaloneSurface>,
        action: &ProjectActionId,
    ) -> Option<ProjectActFacts<StandaloneSurface>> {
        let nav = self.screen.as_ref()?;
        let manifest = prepared.registry.manifest(&action.plugin)?;
        let declaration = manifest.actions.iter().find(|a| a.id == action.action)?;
        let settings = crate::config::plugin_settings(
            &prepared.config.plugins,
            &prepared.config.ui.moved(),
            &action.plugin,
        );
        let confirmation = declaration.confirmation.as_ref().is_none_or(|p| {
            let default = p
                .setting
                .as_ref()
                .and_then(|key| manifest.settings.iter().find(|s| &s.name == key))
                .map(|s| &s.default);
            prompt::confirmation_enabled(
                p,
                p.setting.as_ref().and_then(|key| settings.get(key)),
                default,
            )
        });
        Some(ProjectActFacts {
            origin: ProjectOrigin {
                context: info.id,
                project: nav.project.clone(),
                root: info.root.clone(),
                screen: nav.frame().map(|f| f.key.clone()),
                surface: info.surface.clone(),
            },
            binding: self.actions(prepared, info).iter().any(|a| &a.id == action)
                && prepared.registry.is_enabled(&action.plugin),
            policy: ProjectWritePolicy {
                declared_read_only: manifest.read_only,
                configured: prepared
                    .config
                    .plugins
                    .get(&action.plugin)
                    .and_then(|p| p.get("read_only"))
                    .cloned(),
            },
            context: ProjectAct::new(),
            confirmation,
        })
    }

    pub(super) fn dispatch_act(
        &mut self,
        prepared: &mut PreparedProjects,
        info: &ProjectContextInfo<StandaloneSurface>,
        id: ProjectActionId,
    ) {
        let Some(action) = self
            .actions(prepared, info)
            .into_iter()
            .find(|a| a.id == id)
        else {
            return;
        };
        let Some(facts) = self.act_facts(prepared, info, &id).filter(|f| f.binding) else {
            return;
        };
        let effect = ProjectEffect::new(
            facts.origin.clone(),
            action.declaration.label.get(prepared.lang),
        );
        let update = self.with_host(info, |host, context| {
            host.request_act(context, &action, &facts, |shared| StandaloneInvocation {
                shared,
                effect,
            })
        });
        self.apply(prepared, info, update);
    }

    pub(super) fn execute_act(
        &mut self,
        prepared: &mut PreparedProjects,
        info: &ProjectContextInfo<StandaloneSurface>,
        invocation: StandaloneInvocation,
    ) {
        let Some(facts) = self.act_facts(prepared, info, &invocation.shared.action) else {
            return;
        };
        let update = self.with_host(info, |host, context| {
            host.execute_act(context, invocation, &facts, |key| request(info, key))
        });
        self.apply(prepared, info, update);
    }

    pub(super) fn key(
        &mut self,
        prepared: &mut PreparedProjects,
        info: &ProjectContextInfo<StandaloneSurface>,
        key: KeyEvent,
    ) -> bool {
        // Dialogs get the raw key first, just as in konoma's dispatcher.
        prepared.notice = None;
        if self.dialog.is_none() && KeyPress::norm(&key) == KeyPress::ctrl_ch('c') {
            return true;
        }
        if self.diff_return.is_some() {
            use crate::app::diff_screen::DiffScreenAction;
            let diff = self.diff.as_mut().unwrap();
            let action = diff.handle_key(KeyPress::norm(&key));
            prepared.settings.diff_layout = diff.layout;
            match action {
                Ok((DiffScreenAction::Back, _)) => {
                    self.diff.as_mut().unwrap().close();
                    self.screen = self.diff_return.take();
                }
                Ok((DiffScreenAction::Quit, _)) => return true,
                Ok((DiffScreenAction::Stay, notice)) => prepared.notice = notice,
                Err(error) => prepared.notice = Some(error.message.get(prepared.lang).into()),
            }
            return false;
        }
        if self.dialog.is_none() && KeyPress::norm(&key) == KeyPress::ctrl_ch('t') {
            return false;
        }
        if self.dialog.is_none()
            && (self.help.is_some() || self.menu.is_some())
            && self.presentation_key(prepared, info, key)
        {
            return false;
        }
        let kp = KeyPress::norm(&key);
        let resolution = self.keys.resolve(CoreContext::Text, kp);
        let motion = fixed_keys(CoreContext::Text)
            .into_iter()
            .find(|b| b.key == kp)
            .and_then(|b| match b.op {
                PreviewOp::Core(CoreAction::Navigate(m)) => Some(m),
                _ => None,
            })
            .or(match resolution {
                CoreResolution::Action(CoreAction::Navigate(m)) => Some(m),
                _ => None,
            });
        let actions = self.actions(prepared, info);
        let enter = actions
            .iter()
            .find(|a| a.key == Some(KeyPress::key(KeyCode::Enter)))
            .map(|a| a.id.clone());
        let action = actions
            .iter()
            .find(|a| a.key == Some(kp))
            .map(|a| a.id.clone());
        let facts = ProjectKeyFacts {
            // An already open menu was handled above. Its trigger must still pass through
            // the shared filter/pending-input transitions before opening a new menu.
            global: self.keys.layer_of_key(CoreContext::Canvas, kp)
                == Some(konoma_host_core::help::KeyLayer::Global),
            motion,
            cycle_layout: self.keys.resolve(CoreContext::Diff, kp)
                == CoreResolution::Action(CoreAction::CycleDiffLayout),
            list_override: self
                .keys
                .types
                .get(&CoreViewType::List)
                .and_then(|t| t.get(&kp))
                .map(|b| matches!(b, CoreBinding::Run(CoreAction::Navigate(Motion::Right)))),
            text_right: resolution == CoreResolution::Action(CoreAction::Navigate(Motion::Right)),
            policy: action
                .as_ref()
                .and_then(|id| self.act_facts(prepared, info, id))
                .map(|f| f.policy)
                .unwrap_or_default(),
            action,
            enter_action: enter,
            actions,
            viewport: self.viewport,
            layout: prepared.settings.diff_layout,
            ..Default::default()
        };
        let update = self.with_host(info, |host, context| host.handle_key(context, key, &facts));
        let unhandled = update.dispatch == ProjectDispatch::Unhandled;
        if self.apply(prepared, info, update) {
            return true;
        }
        if unhandled {
            if self.presentation_key(prepared, info, key) {
                return false;
            }
            if let Some(id) = facts.action {
                self.dispatch_act(prepared, info, id);
            }
        }
        false
    }

    pub(super) fn replay_keys(
        &mut self,
        prepared: &mut PreparedProjects,
        info: &ProjectContextInfo<StandaloneSurface>,
    ) -> (bool, bool) {
        if !self.with_host(info, |host, _| host.begin_replay()) {
            return (false, false);
        }
        let mut quit = false;
        let mut changed = false;
        loop {
            let input = self.with_host(info, |host, context| host.next_replay(context));
            match input {
                Some(ProjectInput::Key(key)) => {
                    changed = true;
                    // Rebuild the key facts after every intermediate dialog or navigation change.
                    if self.key(prepared, info, key) {
                        quit = true;
                        break;
                    }
                }
                Some(ProjectInput::Act(invocation)) => {
                    changed = true;
                    self.execute_act(prepared, info, *invocation)
                }
                None => break,
            }
        }
        self.with_host(info, |host, _| host.end_replay());
        (quit, changed)
    }

    pub(super) fn dialog_presentation(
        &self,
        prepared: &PreparedProjects,
    ) -> Option<crate::host::project::draw::ProjectDialogPresentation> {
        let dialog = self.dialog.as_ref()?;
        let labels = match dialog {
            ProjectDialog::Projects { projects, .. } => projects
                .iter()
                .map(|p| ProjectDialogLabel {
                    name: prepared
                        .registry
                        .manifest(&p.plugin)
                        .map(|m| m.name.get(prepared.lang))
                        .unwrap_or(&p.plugin)
                        .into(),
                    path: p.root.display().to_string(),
                })
                .collect(),
            _ => Vec::new(),
        };
        let text = |en, ja| if prepared.lang == "ja" { ja } else { en };
        let presentation = present_project_dialog(
            dialog,
            &labels,
            &ProjectDialogWords {
                lang: prepared.lang,
                choose: text("Choose a project", "プロジェクトを選択"),
                confirm_title: text(" Confirm ", " 確認 "),
                input_title: text(" Input ", " 入力 "),
                keys: text("Enter:choose  Esc:cancel", "Enter:選択  Esc:キャンセル"),
                confirm_cancel: text("n / Esc = cancel", "n / Esc = 取消"),
                confirm_cancel_status: text("n/Esc:cancel", "n/Esc:取消"),
            },
        );
        Some(presentation)
    }
}

/// Raw input conversion, separate from the file app's binding-only `key_press`.
pub(super) fn key_event(key: &Key) -> Option<KeyEvent> {
    use crate::KeyCode as K;
    let code = match key.code {
        K::Char(c) => KeyCode::Char(c),
        K::Enter => KeyCode::Enter,
        K::Backspace => KeyCode::Backspace,
        K::Delete => KeyCode::Delete,
        K::Tab => KeyCode::Tab,
        K::BackTab => KeyCode::BackTab,
        K::Left => KeyCode::Left,
        K::Right => KeyCode::Right,
        K::Up => KeyCode::Up,
        K::Down => KeyCode::Down,
        K::Home => KeyCode::Home,
        K::End => KeyCode::End,
        K::PageUp => KeyCode::PageUp,
        K::PageDown => KeyCode::PageDown,
        K::Esc => KeyCode::Esc,
        K::F(n) => KeyCode::F(n),
        K::Paste(_) => return None,
    };
    let mut modifiers = KeyModifiers::NONE;
    for (from, to) in [
        (KeyMods::CTRL, KeyModifiers::CONTROL),
        (KeyMods::ALT, KeyModifiers::ALT),
        (KeyMods::SHIFT, KeyModifiers::SHIFT),
    ] {
        if key.mods.contains(from) {
            modifiers |= to;
        }
    }
    Some(KeyEvent::new(code, modifiers))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_project_keys_preserve_functions_modifiers_and_backtab() {
        for code in [
            crate::KeyCode::F(5),
            crate::KeyCode::BackTab,
            crate::KeyCode::Char('é'),
        ] {
            let raw = Key::new(code).with_mods(KeyMods::CTRL | KeyMods::ALT | KeyMods::SHIFT);
            let converted = key_event(&raw).unwrap();
            assert_eq!(
                converted.modifiers,
                KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SHIFT
            );
            assert_eq!(
                converted.code,
                match raw.code {
                    crate::KeyCode::F(n) => KeyCode::F(n),
                    crate::KeyCode::BackTab => KeyCode::BackTab,
                    crate::KeyCode::Char(c) => KeyCode::Char(c),
                    _ => unreachable!(),
                }
            );
        }
    }
}
