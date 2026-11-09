//! Project navigation, action contexts and the shared prompts. Session ownership is on workers.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use konoma_plugin::project::{Confirmation, ProjectAct, ProjectRow};
use ratatui::text::Line;

use super::*;
use crate::i18n::Msg;
use crate::keymap::{Context, KeyPress};
use crate::plugins::{ActBinding, PluginAct};

pub(crate) use konoma_plugin::host::project::prompt::ProjectPrompt;
use konoma_plugin::host::project::prompt::{
    self, ProjectActionId, ProjectOrigin, ProjectPromptBlockers, ProjectPromptUpdate,
    PromptInvocation,
};
pub(crate) use konoma_plugin::host::project::ProjectKey;
type SharedInvocation = prompt::ProjectInvocation<ProjectSurface>;
pub(crate) type ProjectDialog = prompt::ProjectDialog<ProjectInvocation>;
use konoma_plugin::host::project::actions::{
    self, ProjectActFacts, ProjectAction, ProjectActionScope, ProjectCommand, ProjectDispatch,
    ProjectNotice, ProjectUpdate, ProjectWritePolicy,
};
use konoma_plugin::host::project::effects;
use konoma_plugin::host::project::input::{self, ProjectKeyFacts};
use konoma_plugin::host::project::{
    ProjectActionInvocation, ProjectContextMut, ProjectEvent, ProjectHost,
};
use konoma_plugin::host::project::{ProjectMotionResult, ProjectScreenState, ProjectViewport};
pub(crate) type ProjectFrame = konoma_plugin::host::project::ProjectFrame<ProjectInvocation>;
type ProjectInput = konoma_plugin::host::project::ProjectInput<ProjectInvocation>;

#[cfg(test)]
pub(crate) use konoma_plugin::host::project::ProjectDiff;
pub(crate) use konoma_plugin::host::project::{ProjectContent, ScreenKey};

pub(crate) use konoma_plugin::host::project::presentation::ProjectControl;
#[cfg(test)]
pub(crate) use konoma_plugin::host::project::presentation::ProjectControlId;
use konoma_plugin::host::project::presentation::{
    ProjectCapabilities, ProjectMotionKeys, ProjectOpenFacts, ProjectPresentation,
    ProjectPresentationBinding, ProjectPresentationContent, ProjectPresentationFacts,
    ProjectPresentationWords,
};
#[derive(Clone)]
pub(crate) struct ProjectNavigation {
    state: ProjectScreenState<ProjectInvocation>,
    origin_mode: Mode,
    origin_path: Option<PathBuf>,
    origin_kind: Option<PreviewKind>,
    origin_scroll: u16,
    origin_hscroll: u16,
}

impl std::ops::Deref for ProjectNavigation {
    type Target = ProjectScreenState<ProjectInvocation>;

    fn deref(&self) -> &Self::Target {
        &self.state
    }
}

impl std::ops::DerefMut for ProjectNavigation {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.state
    }
}

pub(crate) type ProjectEffect = effects::ProjectEffect<ProjectSurface>;

#[derive(Clone, Default, PartialEq, Eq)]
pub(crate) struct ProjectSurface {
    mode: Mode,
    path: Option<PathBuf>,
}

impl ProjectSurface {
    pub(super) fn from_tab(tab: &PerTab) -> Self {
        Self {
            mode: tab.mode,
            path: tab.preview_path.clone(),
        }
    }
}

#[derive(Clone)]
pub(crate) struct ProjectInvocation {
    shared: SharedInvocation,
    effect: ProjectEffect,
}

impl std::ops::Deref for ProjectInvocation {
    type Target = SharedInvocation;
    fn deref(&self) -> &Self::Target {
        &self.shared
    }
}

impl std::ops::DerefMut for ProjectInvocation {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.shared
    }
}

impl PromptInvocation for ProjectInvocation {
    type Surface = ProjectSurface;
    fn prompt_invocation(&self) -> &SharedInvocation {
        &self.shared
    }
    fn prompt_invocation_mut(&mut self) -> &mut SharedInvocation {
        &mut self.shared
    }
}

impl ProjectActionInvocation for ProjectInvocation {
    type Effect = ProjectEffect;
    fn into_effect(mut self, tree_status: bool) -> ProjectEffect {
        self.effect.tree_status = tree_status;
        self.effect
    }
}

impl ProjectInvocation {
    pub(crate) fn destructive(&self) -> bool {
        self.shared.destructive()
    }

    pub(crate) fn confirmation_text(&self, prompt: &Confirmation, lang: &str) -> String {
        self.shared.confirmation_text(prompt, lang)
    }
}

fn project_context(tab: &mut PerTab) -> ProjectContextMut<'_, ProjectInvocation> {
    ProjectContextMut {
        id: tab.id,
        active: tab.project_navigation.as_mut().map(|nav| &mut nav.state),
        returned: tab.project_return.as_mut().map(|nav| &mut nav.state),
        pending_acts: &mut tab.project_pending_acts,
        request: None,
    }
}

impl App {
    fn project_list_right(&self, key: KeyEvent) -> bool {
        use crate::keymap::{Action, Motion, Resolution};
        let kp = KeyPress::norm(&key);
        #[cfg(feature = "git")]
        let list_override = self
            .keymaps
            .types
            .get(&crate::keymap::ViewType::List)
            .and_then(|table| table.get(&kp))
            .map(|binding| {
                matches!(
                    binding,
                    crate::keymap::Binding::Run(Action::Navigate(Motion::Right))
                )
            });
        #[cfg(not(feature = "git"))]
        let list_override = None;
        input::list_right(
            self.project_content()
                .is_some_and(|view| matches!(view.as_ref(), ProjectContent::List(_))),
            key.code,
            list_override,
            self.keymaps.resolve(Context::Text, None, kp)
                == Resolution::Action(Action::Navigate(Motion::Right)),
        )
    }

    fn project_enter_action(&self) -> Option<crate::keymap::Action> {
        self.plugin_action_for_key(
            Context::Canvas,
            crate::keymap::KeyPress::key(KeyCode::Enter),
        )
    }

    pub(crate) fn project_open_control(&self) -> Option<ProjectControl> {
        self.project_presentation()?.0.open_control().cloned()
    }

    fn project_open_facts(&self) -> Option<ProjectOpenFacts> {
        use crate::keymap::{Action, KeyPress, Motion};
        if !self
            .project_content()
            .is_some_and(|v| matches!(v.as_ref(), ProjectContent::List(_)))
        {
            return None;
        }
        let action = self.project_enter_action()?;
        if !crate::ops::action_enabled(self, Context::Canvas, action) {
            return None;
        }
        let Action::Plugin(act) = action else {
            return None;
        };
        let label = self
            .plugin_keys
            .declaration(act)?
            .label
            .get(self.plugin_lang())
            .to_string();
        let mut candidates = self
            .keymaps
            .keys_of(Context::Text, Action::Navigate(Motion::Right));
        #[cfg(feature = "git")]
        if let Some(table) = self.keymaps.types.get(&crate::keymap::ViewType::List) {
            candidates.extend(table.keys().copied());
        }
        candidates.push(KeyPress::key(KeyCode::Right));
        let mut keys = Vec::new();
        for key in candidates {
            if self.project_list_right(KeyEvent::new(
                key.code,
                if key.ctrl {
                    KeyModifiers::CONTROL
                } else {
                    KeyModifiers::NONE
                },
            )) && !keys.contains(&key)
            {
                keys.push(key);
            }
        }
        Some(ProjectOpenFacts {
            right_aliases: keys,
            label,
        })
    }

    fn project_viewport(&self) -> ProjectViewport {
        ProjectViewport {
            cols: self.tab.preview_cols,
            rows: self.tab.preview_viewport,
        }
    }

    fn apply_project_motion_result(&mut self, result: ProjectMotionResult) -> bool {
        match result {
            ProjectMotionResult::Unhandled => false,
            ProjectMotionResult::Handled => true,
            ProjectMotionResult::Back => {
                self.project_back();
                true
            }
            ProjectMotionResult::LayoutChanged(layout) => {
                // Project diffs always show raw lines, independent of the origin preview's presentation.
                self.diff_layout = layout;
                self.tab.preview_hscroll = 0;
                self.tab.git_detail_hscroll = 0;
                let label = match layout {
                    DiffLayout::Unified => Msg::DiffUnified,
                    DiffLayout::Split => Msg::DiffSideBySide,
                    DiffLayout::Auto => Msg::DiffAuto,
                };
                self.flash = Some(tr(self.lang, label).into());
                true
            }
        }
    }

    pub(crate) fn project_active(&self) -> bool {
        self.tab.project_navigation.is_some()
    }

    #[cfg(test)]
    fn project_waiting_for_view(&self) -> bool {
        let request = self
            .project_frame()
            .and_then(|frame| self.project_request(&frame.key));
        input::waiting(
            self.tab.project_navigation.as_ref().map(|nav| &nav.state),
            &self.project_runtimes,
            request.as_ref().and_then(|req| req.project.as_ref()),
        )
    }

    fn with_project_host<T>(
        &mut self,
        run: impl FnOnce(
            &mut ProjectHost<'_, ProjectInvocation>,
            &mut ProjectContextMut<'_, ProjectInvocation>,
        ) -> T,
    ) -> T {
        let request = self
            .project_frame()
            .and_then(|frame| self.project_request(&frame.key))
            .and_then(|req| req.project);
        let mut host = ProjectHost::new(
            &mut self.project_runtimes,
            &mut self.project_dialog,
            &mut self.project_replaying,
        );
        let mut context = ProjectContextMut {
            id: self.tab.id,
            active: self
                .tab
                .project_navigation
                .as_mut()
                .map(|nav| &mut nav.state),
            returned: self.tab.project_return.as_mut().map(|nav| &mut nav.state),
            pending_acts: &mut self.tab.project_pending_acts,
            request,
        };
        run(&mut host, &mut context)
    }

    pub(crate) fn project_input_replaying(&self) -> bool {
        self.project_replaying.is_replaying()
    }

    pub(super) fn cancel_project_input(&mut self) {
        self.with_project_host(|host, context| host.cancel_input(context));
    }

    /// Each raw key returns through the ordinary dispatcher with all SDK borrows released.
    pub(super) fn replay_project_keys(&mut self) -> bool {
        if !self.with_project_host(|host, _| host.begin_replay()) {
            return false;
        }
        let mut changed = false;
        loop {
            let input = self.with_project_host(|host, context| host.next_replay(context));
            let Some(input) = input else {
                break;
            };
            changed = true;
            match input {
                ProjectInput::Key(key) => {
                    let result = crate::handle_key(self, key);
                    crate::resolve_key_result(self, result);
                }
                ProjectInput::Act(invocation) => self.execute_project_act(*invocation),
            }
        }
        self.with_project_host(|host, _| host.end_replay());
        changed
    }
    pub(crate) fn project_frame(&self) -> Option<&ProjectFrame> {
        self.tab.project_navigation.as_ref()?.frame()
    }

    pub(crate) fn project_content(&self) -> Option<Arc<ProjectContent>> {
        let nav = self.tab.project_navigation.as_ref()?;
        let key = &nav.stack.last()?.key;
        let runtime = self.project_runtimes.get(&nav.project)?;
        if runtime.contexts.get(key)
            != self
                .project_request(key)
                .as_ref()
                .and_then(|req| req.project.as_ref())
        {
            return None;
        }
        runtime.views.get(key).cloned()
    }

    pub(crate) fn project_marks_enabled(&self) -> bool {
        self.tab.project_navigation.as_ref().is_some_and(|nav| {
            nav.marks_enabled(
                self.project_runtimes
                    .get(&nav.project)
                    .map(|runtime| &runtime.project.declaration),
            )
        })
    }

    pub(crate) fn project_visible_rows(&self) -> Vec<usize> {
        let Some(nav) = self.tab.project_navigation.as_ref() else {
            return Vec::new();
        };
        let Some(content) = self.project_content() else {
            return Vec::new();
        };
        nav.visible_rows(&content, self.project_viewport(), self.diff_layout)
    }

    fn project_selected_index(&self) -> Option<usize> {
        self.tab.project_navigation.as_ref()?.selected_index(
            self.project_content()?.as_ref(),
            self.project_viewport(),
            self.diff_layout,
        )
    }

    pub(crate) fn project_row_target(&self) -> Option<PathBuf> {
        let index = self.project_selected_index()?;
        self.tab
            .project_navigation
            .as_ref()?
            .row_target(self.project_content()?.as_ref(), index)
    }

    pub(crate) fn project_new_tab_hint(&self) -> Option<(String, String)> {
        let entry = self.project_new_tab_control()?;
        Some((entry.rendered_keys(), entry.label))
    }

    fn project_new_tab_control(&self) -> Option<ProjectControl> {
        self.project_presentation()?.0.new_tab_control().cloned()
    }

    fn action_project(&self, action: PluginAct) -> Option<ProjectKey> {
        let detected: Vec<_> = self
            .tab
            .projects
            .projects
            .iter()
            .map(ProjectKey::from)
            .collect();
        actions::action_project(
            self.tab.project_navigation.as_ref().map(|nav| &nav.project),
            self.tab.project_return.as_ref().map(|nav| &nav.project),
            &detected,
            action.plugin(),
            |key| self.project_runtimes.contains_key(key),
        )
    }
    pub(crate) fn project_act_context(&self) -> ProjectAct {
        if let Some(nav) = self
            .tab
            .project_navigation
            .as_ref()
            .filter(|nav| nav.frame().is_some())
        {
            return nav.act_context(self.project_content().as_deref());
        }
        if self.is_git_diff_preview() {
            return self
                .tab
                .preview_path
                .as_ref()
                .map(|p| ProjectAct::new().selected(ProjectRow::new("file", p.to_string_lossy())))
                .unwrap_or_default();
        }
        let row = |path: &PathBuf| {
            let kind = if self.tab.entries.iter().any(|e| &e.path == path && e.is_dir) {
                "directory"
            } else {
                "file"
            };
            ProjectRow::new(kind, path.to_string_lossy())
        };
        let mut context = ProjectAct::new().marked(self.tab.selection.iter().map(row));
        if let Some(entry) = self.tab.entries.get(self.tab.selected) {
            context = context.selected(row(&entry.path));
        }
        context
    }

    fn project_action(&self, action: PluginAct) -> Option<ProjectAction> {
        let declaration = self.plugin_keys.declaration(action)?.clone();
        let key = self
            .plugin_keys
            .acts_of(action.plugin())
            .iter()
            .find(|binding| binding.act == action)
            .and_then(|binding| binding.key);
        Some(ProjectAction {
            id: ProjectActionId {
                plugin: action.plugin().into(),
                action: action.act().into(),
            },
            declaration,
            key,
        })
    }

    fn project_scope(&self, ctx: Context) -> ProjectActionScope {
        if let Some(nav) = &self.tab.project_navigation {
            if ctx != Context::Canvas {
                return ProjectActionScope::None;
            }
            return ProjectActionScope::Screen {
                plugin: nav.project.plugin.clone(),
                context: self.project_act_context(),
                diff: self
                    .project_content()
                    .is_some_and(|view| matches!(view.as_ref(), ProjectContent::Diff(_))),
                jumps: self
                    .project_runtimes
                    .get(&nav.project)
                    .map(|runtime| {
                        runtime
                            .project
                            .declaration
                            .screens
                            .iter()
                            .filter_map(|screen| screen.key)
                            .collect()
                    })
                    .unwrap_or_default(),
            };
        }
        let plugins = self
            .tab
            .projects
            .projects
            .iter()
            .map(|project| project.plugin.clone())
            .collect();
        if matches!(ctx, Context::Tree | Context::TreeVisual) {
            ProjectActionScope::TreeSelection {
                plugins,
                context: self.project_act_context(),
            }
        } else if ctx == Context::Diff && self.is_git_diff_preview() {
            ProjectActionScope::HostDiff { plugins }
        } else {
            ProjectActionScope::None
        }
    }

    pub(crate) fn project_acts_in(&self, ctx: Context) -> Vec<&ActBinding> {
        let scope = self.project_scope(ctx);
        let ids: BTreeSet<_> = self
            .tab
            .project_navigation
            .as_ref()
            .map(|nav| vec![nav.project.plugin.as_str()])
            .unwrap_or_else(|| {
                self.tab
                    .projects
                    .projects
                    .iter()
                    .map(|p| p.plugin.as_str())
                    .collect()
            })
            .into_iter()
            .collect();
        let bindings: Vec<_> = ids
            .into_iter()
            .flat_map(|id| self.plugin_keys.acts_of(id))
            .collect();
        let actions: Vec<_> = bindings
            .iter()
            .filter_map(|binding| self.project_action(binding.act))
            .collect();
        let allowed = actions::acts_in(&scope, &actions);
        bindings
            .into_iter()
            .filter(|binding| {
                allowed.iter().any(|a| {
                    a.id.plugin == binding.act.plugin() && a.id.action == binding.act.act()
                })
            })
            .collect()
    }

    pub(super) fn project_action_key(
        &self,
        ctx: Context,
        action: PluginAct,
        key: KeyPress,
    ) -> Option<KeyPress> {
        actions::action_key(&self.project_scope(ctx), &self.project_action(action)?, key)
    }

    pub(crate) fn project_act_enabled(&self, action: PluginAct) -> Option<bool> {
        self.plugin_keys
            .declaration(action)?
            .project_context
            .as_ref()?;
        let project = self.action_project(action)?;
        Some(actions::act_enabled(
            self.project_runtimes.get(&project),
            action.act(),
        ))
    }
    /// `p` already belongs to OpenGitView. The legacy hub remains the fallback.
    pub(crate) fn enter_project(&mut self) -> bool {
        self.ensure_project_detection();
        if self.tab.projects.pending() {
            self.tab.project_enter_pending = true;
            return true;
        }
        let projects: Vec<_> = self
            .tab
            .projects
            .projects
            .iter()
            .filter(|p| !p.declaration.screens.is_empty())
            .map(ProjectKey::from)
            .collect();
        let update = prompt::choose_projects(&mut self.project_dialog, projects);
        let handled = !matches!(update, ProjectPromptUpdate::Unchanged);
        self.apply_project_prompt_update(update);
        handled
    }

    pub(super) fn open_project(&mut self, key: ProjectKey) {
        if let Some(project) = self
            .tab
            .projects
            .projects
            .iter()
            .find(|p| ProjectKey::from(*p) == key)
            .cloned()
        {
            self.start_project_runtime(project);
        }
        let Some(screen) =
            self.with_project_host(|host, context| host.first_screen(&key, context.id))
        else {
            return;
        };
        self.show_project_screen(key, screen);
    }

    fn show_project_screen(&mut self, key: ProjectKey, screen: ScreenKey) {
        self.tab.project_navigation = Some(ProjectNavigation {
            state: ProjectHost::<ProjectInvocation>::start_screen(key, screen),
            origin_mode: self.tab.mode,
            origin_path: self.tab.preview_path.clone(),
            origin_kind: self.tab.preview_kind.clone(),
            origin_scroll: self.tab.preview_scroll,
            origin_hscroll: self.tab.preview_hscroll,
        });
        self.ensure_project_view();
    }

    fn ensure_project_view(&mut self) {
        let info = self.project_context_info(&self.tab);
        self.with_project_host(|host, context| host.ensure_view(context, &info));
    }

    fn project_back(&mut self) {
        if self.with_project_host(|host, context| host.pop_screen(context)) {
            self.ensure_project_view();
            return;
        }
        let Some(nav) = self.tab.project_navigation.take() else {
            return;
        };
        match (nav.origin_mode, nav.origin_path) {
            (Mode::Preview, Some(path)) => {
                if matches!(nav.origin_kind, Some(PreviewKind::GitDiff(_))) {
                    self.open_git_diff(&path);
                } else {
                    self.enter_preview(&path);
                }
                self.tab.preview_scroll = nav.origin_scroll;
                self.tab.preview_hscroll = nav.origin_hscroll;
            }
            _ => self.back_to_tree(),
        }
    }

    pub(crate) fn handle_project_key(&mut self, key: KeyEvent) -> bool {
        use crate::keymap::{Action, Motion, Resolution};
        #[cfg(feature = "git")]
        use crate::keymap::{Binding, ViewType};
        if self.project_dialog.is_some() {
            self.project_dialog_key(key);
            return true;
        }
        let kp = KeyPress::norm(&key);
        #[cfg(feature = "git")]
        let list_override = self
            .keymaps
            .types
            .get(&ViewType::List)
            .and_then(|table| table.get(&kp))
            .map(|binding| matches!(binding, Binding::Run(Action::Navigate(Motion::Right))));
        #[cfg(not(feature = "git"))]
        let list_override = None;
        let motion = crate::ops::keys::fixed_action(Context::Text, key.code)
            .or_else(|| match self.keymaps.resolve(Context::Text, None, kp) {
                Resolution::Action(action) => Some(action),
                _ => None,
            })
            .and_then(|action| match action {
                Action::Navigate(motion) => Some(motion),
                _ => None,
            });
        let action =
            self.plugin_action_for_key(Context::Canvas, kp)
                .and_then(|action| match action {
                    Action::Plugin(p) => Some(p),
                    _ => None,
                });
        let actions = self
            .tab
            .project_navigation
            .as_ref()
            .map(|nav| {
                self.plugin_keys
                    .acts_of(&nav.project.plugin)
                    .iter()
                    .filter_map(|b| self.project_action(b.act))
                    .collect()
            })
            .unwrap_or_default();
        let facts = ProjectKeyFacts {
            blocked: self.show_help
                || self.dialog.is_some()
                || self.is_tab_list()
                || self.is_bookmark_list()
                || self.is_info(),
            leader: self.pending_leader.is_some(),
            global: self.keymaps.layer_of_key(Context::Canvas, kp)
                == Some(crate::keymap::KeyLayer::Global),
            motion,
            cycle_layout: self
                .project_content()
                .is_some_and(|c| matches!(c.as_ref(), ProjectContent::Diff(_)))
                && self.keymaps.resolve(Context::Diff, None, kp)
                    == Resolution::Action(Action::CycleDiffLayout),
            list_override,
            text_right: self.keymaps.resolve(Context::Text, None, kp)
                == Resolution::Action(Action::Navigate(Motion::Right)),
            action: action.and_then(|p| self.project_action(p).map(|a| a.id)),
            enter_action: self.project_enter_action().and_then(|action| match action {
                Action::Plugin(p) => self.project_action(p).map(|a| a.id),
                _ => None,
            }),
            actions,
            policy: action
                .map(|p| self.project_write_policy(p.plugin()))
                .unwrap_or_default(),
            viewport: self.project_viewport(),
            layout: self.diff_layout,
        };
        let update = self.with_project_host(|host, context| host.handle_key(context, key, &facts));
        self.apply_project_update(update)
    }

    fn apply_project_update(&mut self, update: ProjectUpdate<ProjectInvocation>) -> bool {
        let handled = update.dispatch != ProjectDispatch::Unhandled;
        if update.dispatch == ProjectDispatch::LeaveProject {
            self.project_back();
        }
        for command in update.commands {
            match command {
                ProjectCommand::TreeStatus => self.git_status_dirty = true,
                ProjectCommand::Label(label) => self.flash = Some(label),
                ProjectCommand::Copy(text) => self.set_clipboard_flash(&text),
                ProjectCommand::Flash(message) => {
                    self.flash = Some(message.get(self.plugin_lang()).into())
                }
                ProjectCommand::UnknownScreen => {
                    self.flash = Some(tr(self.lang, Msg::ProjectUnknownScreen).into())
                }
                ProjectCommand::StartScreen { project, screen } => {
                    self.show_project_screen(project, screen)
                }
                ProjectCommand::Navigation {
                    outcome,
                    save_return,
                } => {
                    if save_return {
                        self.tab.project_return = self.tab.project_navigation.take();
                    }
                    let result = self.apply_plugin_navigation(outcome);
                    crate::resolve_key_result(self, result.map(|()| false));
                }
                ProjectCommand::EnsureView => self.ensure_project_view(),
                ProjectCommand::Dispatch(id) => {
                    let action =
                        crate::keymap::Action::Plugin(PluginAct::new(&id.plugin, &id.action));
                    let result = crate::dispatch_action(self, action, self.surface());
                    crate::resolve_key_result(self, result);
                }
                ProjectCommand::NewTab => {
                    let result = self.tab_new_from_selection();
                    crate::resolve_key_result(self, result.map(|()| false));
                }
                ProjectCommand::LayoutChanged(layout) => {
                    self.apply_project_motion_result(ProjectMotionResult::LayoutChanged(layout));
                }
                ProjectCommand::Notice(notice) => {
                    let message = match notice {
                        ProjectNotice::Busy => Msg::ProjectBusy,
                        ProjectNotice::ReadOnly => Msg::PluginReadOnly,
                        ProjectNotice::SelectionChanged => Msg::ProjectSelectionChanged,
                    };
                    self.flash = Some(tr(self.lang, message).into());
                }
            }
        }
        if let Some(prompt) = update.prompt {
            self.apply_project_prompt_update(prompt);
        }
        handled
    }

    fn project_write_policy(&self, plugin: &str) -> ProjectWritePolicy {
        let declared_read_only =
            crate::plugins::with_registry(&self.cfg.plugin_disabled(), |registry| {
                registry
                    .manifest(plugin)
                    .is_some_and(|manifest| manifest.read_only)
            });
        let configured = self
            .cfg
            .plugins
            .get(plugin)
            .and_then(|table| table.get("read_only"))
            .cloned();
        ProjectWritePolicy {
            declared_read_only,
            configured,
        }
    }

    fn project_act_facts(
        &self,
        action: &ProjectActionId,
        project: ProjectKey,
        confirmation: bool,
    ) -> ProjectActFacts<ProjectSurface> {
        ProjectActFacts {
            origin: ProjectOrigin {
                context: self.tab.id,
                project,
                root: self.tab.root.clone(),
                screen: self.project_frame().map(|frame| frame.key.clone()),
                surface: ProjectSurface {
                    mode: self.tab.mode,
                    path: self.tab.preview_path.clone(),
                },
            },
            binding: self
                .plugin_binding_in(
                    self.key_context().unwrap_or_else(|| self.base_context()),
                    PluginAct::new(&action.plugin, &action.action),
                )
                .is_some(),
            policy: self.project_write_policy(&action.plugin),
            context: self.project_act_context(),
            confirmation,
        }
    }
    fn project_confirmation_enabled(&self, plugin: &str, prompt: &Confirmation) -> bool {
        let Some(key) = &prompt.setting else {
            return true;
        };
        let settings = crate::plugins::effective_settings(&self.cfg, plugin);
        let registry = crate::plugins::shared_registry(&self.cfg.plugin_disabled());
        let default = registry.manifest(plugin).and_then(|manifest| {
            manifest
                .settings
                .iter()
                .find(|setting| &setting.name == key)
                .map(|setting| &setting.default)
        });
        prompt::confirmation_enabled(prompt, settings.get(key), default)
    }

    pub(crate) fn request_project_act(&mut self, action: PluginAct) -> bool {
        let Some(declaration) = self.project_action(action) else {
            return false;
        };
        if declaration.declaration.project_context.is_none() {
            return false;
        }
        let Some(project) = self.action_project(action) else {
            return true;
        };
        let confirmation = declaration
            .declaration
            .confirmation
            .as_ref()
            .is_none_or(|prompt| self.project_confirmation_enabled(action.plugin(), prompt));
        let facts = self.project_act_facts(&declaration.id, project.clone(), confirmation);
        let effect = ProjectEffect::new(
            facts.origin.clone(),
            declaration.declaration.label.get(self.plugin_lang()),
        );
        let update = self.with_project_host(|host, context| {
            host.request_act(context, &declaration, &facts, |shared| ProjectInvocation {
                shared,
                effect,
            })
        });
        self.apply_project_update(update)
    }

    pub(super) fn execute_project_act(&mut self, invocation: ProjectInvocation) {
        let project = self
            .tab
            .project_navigation
            .as_ref()
            .map(|nav| nav.project.clone())
            .unwrap_or_else(|| invocation.origin.project.clone());
        let facts = self.project_act_facts(&invocation.action, project, true);
        // Snapshot every live context before borrowing the shared screen states.
        let live: Vec<_> = std::iter::once(&self.tab)
            .chain(self.tabs.iter())
            .map(|tab| self.project_context_info(tab))
            .collect();
        let update = self.with_project_host(|host, context| {
            host.execute_act(context, invocation, &facts, |key| {
                effects::request_for(&live, key)
            })
        });
        self.apply_project_update(update);
    }
    fn reopen_project_input(&mut self, invocation: ProjectInvocation) {
        let Some(nav) = self.tab.project_navigation.as_ref() else {
            return;
        };
        let current = ProjectOrigin {
            context: self.tab.id,
            project: nav.project.clone(),
            root: self.tab.root.clone(),
            screen: nav.frame().map(|frame| frame.key.clone()),
            surface: ProjectSurface {
                mode: self.tab.mode,
                path: self.tab.preview_path.clone(),
            },
        };
        let blockers = ProjectPromptBlockers {
            dialog: self.dialog.is_some(),
            text_input: self.is_text_input_active(),
            filtering: self.project_frame().is_some_and(|frame| frame.filtering),
        };
        self.with_project_host(|host, _| host.reopen_retry(invocation, &current, blockers));
    }

    pub(crate) fn poll_project_jobs(&mut self) -> bool {
        let live: Vec<_> = std::iter::once(&self.tab)
            .chain(self.tabs.iter())
            .map(|tab| self.project_context_info(tab))
            .collect();
        let retained: Vec<_> = std::iter::once(&self.tab)
            .chain(self.tabs.iter())
            .flat_map(|tab| tab.projects.projects.iter().map(ProjectKey::from))
            .collect();
        // Registry, settings and ReadScope remain host-owned.
        for project in self.tab.projects.projects.clone() {
            self.start_project_runtime(project);
        }
        let update = {
            let mut host = ProjectHost::new(
                &mut self.project_runtimes,
                &mut self.project_dialog,
                &mut self.project_replaying,
            );
            let mut contexts: Vec<_> = std::iter::once(&mut self.tab)
                .chain(self.tabs.iter_mut())
                .map(project_context)
                .collect();
            host.poll(&mut contexts, &live, &retained)
        };
        let mut changed = update.changed;
        for event in update.events {
            match event {
                ProjectEvent::Watch(key, paths) => {
                    for tab in std::iter::once(&mut self.tab).chain(self.tabs.iter_mut()) {
                        tab.projects.set_watch_paths(
                            &key.plugin,
                            &key.root,
                            &key.marker,
                            paths.clone(),
                        );
                    }
                }
                ProjectEvent::Effect(effect) => {
                    if effect.tree_status && self.tab.root.starts_with(&effect.origin.project.root)
                    {
                        self.git_status_dirty = true;
                    }
                    if effect.origin.context == self.tab.id {
                        self.apply_project_effect(effect);
                    } else if let Some(tab) = self
                        .tabs
                        .iter_mut()
                        .find(|tab| tab.id == effect.origin.context)
                    {
                        tab.project_effects.push(effect);
                    }
                }
                ProjectEvent::EnsureView => {
                    for effect in std::mem::take(&mut self.tab.project_effects) {
                        self.apply_project_effect(effect);
                        changed = true;
                    }
                    if self.tab.project_enter_pending && !self.tab.projects.pending() {
                        self.tab.project_enter_pending = false;
                        if !self.enter_project() {
                            self.open_git_view();
                        }
                        changed = true;
                    }
                    self.ensure_project_view();
                }
                ProjectEvent::Error(error) => {
                    self.flash = Some(error.message.get(self.plugin_lang()).into())
                }
                ProjectEvent::ChipError(key, error) => {
                    if self
                        .tab
                        .projects
                        .projects
                        .iter()
                        .any(|p| ProjectKey::from(p) == key)
                    {
                        self.flash = Some(error.message.get(self.plugin_lang()).into());
                    }
                }
                ProjectEvent::Retry(invocation) => self.reopen_project_input(invocation),
            }
        }
        changed
    }

    fn apply_project_effect(&mut self, effect: ProjectEffect) {
        let current = self.project_context_info(&self.tab);
        let update =
            self.with_project_host(|host, context| host.apply_effect(context, &current, effect));
        self.apply_project_update(update);
    }

    #[cfg(test)]
    pub(crate) fn project_controls(&self) -> Option<String> {
        Some(
            self.project_control_entries()?
                .iter()
                .map(ProjectControl::render)
                .collect::<Vec<_>>()
                .join("  "),
        )
    }

    #[cfg(test)]
    pub(crate) fn project_control_entries(&self) -> Option<Vec<ProjectControl>> {
        Some(self.project_presentation()?.0.controls().to_vec())
    }

    pub(crate) fn project_presentation(&self) -> Option<(ProjectPresentation, Vec<PluginAct>)> {
        use crate::keymap::{Action, LeaderId, Motion};
        if !self.project_active() {
            return None;
        }
        let motion_keys = |motions: &[Motion]| {
            motions
                .iter()
                .flat_map(|m| self.keymaps.keys_of(Context::Text, Action::Navigate(*m)))
                .collect()
        };
        let filter_label = self.tab.project_navigation.as_ref().and_then(|nav| {
            let screen = &nav.stack.last()?.key.screen;
            self.project_runtimes
                .get(&nav.project)?
                .project
                .declaration
                .screens
                .iter()
                .find(|decl| &decl.id == screen)?
                .filter_label
                .as_ref()
                .map(|label| label.get(self.plugin_lang()).to_string())
        });
        let mut graph_scroll = Vec::new();
        let enabled: Vec<_> = [Action::Navigate(Motion::Down), Action::Navigate(Motion::Up)]
            .into_iter()
            .filter(|a| crate::ops::action_enabled(self, Context::Text, *a))
            .collect();
        let (columns, _) = crate::ops::keys::columns(self, Context::Text, &enabled);
        for key in columns.into_iter().flatten() {
            if !graph_scroll.contains(&key) {
                graph_scroll.push(key);
            }
        }
        let facts = ProjectPresentationFacts {
            content: self
                .project_content()
                .map(|content| match content.as_ref() {
                    ProjectContent::List(_) => ProjectPresentationContent::List,
                    ProjectContent::Lines(_) => ProjectPresentationContent::Lines,
                    ProjectContent::Diff(_) => ProjectPresentationContent::Diff,
                }),
            marks: self.project_marks_enabled(),
            filter: self.project_filter_enabled(),
            pushed: self.project_pushed(),
            picker: self.project_picker(),
            graph_jj: self.project_graph_jj(),
            row_target: self.project_row_target().is_some(),
            open: self.project_open_facts(),
            motions: ProjectMotionKeys {
                scroll: motion_keys(&[Motion::Down, Motion::Up]),
                hscroll: motion_keys(&[Motion::Left, Motion::Right]),
                ends: motion_keys(&[Motion::Top, Motion::Bottom]),
                diff_layout: self.keymaps.keys_of(Context::Diff, Action::CycleDiffLayout),
                half_page: motion_keys(&[Motion::HalfDown, Motion::HalfUp]),
                page: motion_keys(&[Motion::PageDown, Motion::PageUp]),
                line_ends: motion_keys(&[Motion::LineHome, Motion::LineEnd]),
            },
            graph_scroll,
            copy_keys: if crate::ops::menu::opens(self, Context::Canvas, LeaderId::Copy) {
                self.keymaps
                    .leader_keys_of(Context::Canvas)
                    .into_iter()
                    .filter(|(_, id)| *id == LeaderId::Copy)
                    .map(|(key, _)| key)
                    .collect()
            } else {
                vec![]
            },
            filter_label,
        };
        let mut actions = Vec::new();
        let bindings: Vec<_> = self
            .plugin_context_bindings(Context::Canvas)
            .into_iter()
            .enumerate()
            .map(|(index, b)| {
                actions.push(b.act);
                ProjectPresentationBinding {
                    enabled: crate::ops::action_enabled(
                        self,
                        Context::Canvas,
                        Action::Plugin(b.act),
                    ),
                    declared_enter: self
                        .plugin_keys
                        .declaration(b.act)
                        .is_some_and(|a| a.key.as_deref() == Some("Enter")),
                    binding: konoma_plugin::host::acts::ActBinding {
                        act: index,
                        copy_key: b.copy_key,
                        hint_group: b.hint_group,
                        key: b.key,
                        context: b.context,
                        label: b.label,
                        help: b.help,
                        footer: b.footer,
                        writes: b.writes,
                    },
                }
            })
            .collect();
        let move_list = konoma_plugin::t!(en: "move", ja: "移動");
        let mark = konoma_plugin::t!(en: "mark", ja: "マーク");
        let back = konoma_plugin::t!(en: "back", ja: "戻る");
        let words = ProjectPresentationWords {
            language: self.plugin_lang(),
            screens: tr(self.lang, Msg::FtScreens),
            move_list: move_list.get(self.plugin_lang()),
            scroll: tr(self.lang, Msg::Scroll),
            mark: mark.get(self.plugin_lang()),
            filter: tr(self.lang, Msg::HintFilter),
            back: back.get(self.plugin_lang()),
            help: tr(self.lang, Msg::HintHelp),
            new_tab: tr(self.lang, Msg::HintNewTab),
            copy: tr(self.lang, Msg::CopyHint),
            actions: tr(self.lang, Msg::WkActionsTitle),
            hscroll: tr(self.lang, Msg::HintHscroll),
            ends: tr(self.lang, Msg::HintEnds),
            diff_layout: tr(self.lang, Msg::FtDiffLayout),
            cycle: tr(self.lang, Msg::ProjectCycle),
            pop: tr(self.lang, Msg::ProjectBack),
            apply: tr(self.lang, Msg::FtApply),
            cancel: tr(self.lang, Msg::FtCancel),
            nav: tr(self.lang, Msg::FtNav),
            search: tr(self.lang, Msg::HintSearch),
            half_page: tr(self.lang, Msg::HelpHalfPage),
            page: tr(self.lang, Msg::HintPage),
            line_ends: tr(self.lang, Msg::HintLineEnds),
        };
        Some((
            ProjectPresentation::build(
                &facts,
                ProjectCapabilities { can_open_tab: true },
                &bindings,
                &words,
            ),
            actions,
        ))
    }

    /// Legacy VCS wording during migration; remove this bridge when the old hub is removed.
    pub(crate) fn project_picker(&self) -> bool {
        self.tab
            .project_navigation
            .as_ref()
            .is_some_and(|nav| nav.picker())
    }

    pub(crate) fn project_filter_enabled(&self) -> bool {
        self.tab.project_navigation.as_ref().is_none_or(|nav| {
            nav.filter_enabled(
                self.project_runtimes
                    .get(&nav.project)
                    .map(|runtime| &runtime.project.declaration),
            )
        })
    }

    /// Legacy VCS wording during migration; remove this bridge when the old hub is removed.
    pub(crate) fn project_graph_jj(&self) -> Option<bool> {
        self.tab.project_navigation.as_ref()?.graph_jj()
    }

    pub(crate) fn project_pushed(&self) -> bool {
        self.tab.project_navigation.as_ref().is_some_and(|nav| {
            nav.pushed(
                self.project_runtimes
                    .get(&nav.project)
                    .map(|runtime| &runtime.project.declaration),
            )
        })
    }

    pub(crate) fn project_text_hints(&self) -> Option<Vec<(String, String)>> {
        Some(
            self.project_text_controls()?
                .into_iter()
                .map(|entry| (entry.rendered_keys(), entry.label))
                .collect(),
        )
    }

    fn project_text_controls(&self) -> Option<Vec<ProjectControl>> {
        Some(self.project_presentation()?.0.text_controls()?.to_vec())
    }

    pub(crate) fn project_text_help(&self) -> Option<Vec<(String, String)>> {
        let mut hints = self.project_text_hints()?;
        hints.extend_from_slice(self.project_presentation()?.0.additional_text_help()?);
        Some(hints)
    }

    pub(crate) fn project_chips(&self) -> Vec<Line<'static>> {
        self.tab
            .projects
            .projects
            .iter()
            .filter_map(|project| {
                self.project_runtimes
                    .get(&ProjectKey::from(project))
                    .and_then(|runtime| runtime.chip.as_ref())
                    .map(|chip| konoma_plugin::ratatui_draw::into_line(chip.text.clone()))
                    .filter(|line| line.width() > 0)
            })
            .collect()
    }

    pub(crate) fn return_to_project(&mut self) -> bool {
        if !ProjectHost::<ProjectInvocation>::restore_return(
            &mut self.tab.project_navigation,
            &mut self.tab.project_return,
        ) {
            return false;
        }
        self.ensure_project_view();
        true
    }

    pub(crate) fn resize_project(&mut self, cols: u16, rows: u16) {
        let previous = ProjectViewport {
            cols: self.tab.preview_cols,
            rows: self.tab.preview_viewport,
        };
        self.tab.preview_cols = cols;
        self.tab.preview_viewport = rows;
        self.with_project_host(|host, context| {
            host.resize(context, previous, ProjectViewport { cols, rows })
        });
        self.ensure_project_view();
    }

    #[cfg(test)]
    pub(crate) fn project_jobs_pending(&self) -> bool {
        self.project_runtimes
            .values()
            .any(|r| r.busy || !r.closed_tabs.is_empty())
    }

    pub(super) fn project_tab_closed(&mut self, tab: u64) {
        self.with_project_host(|host, _| host.close_context(tab));
    }

    #[cfg(test)]
    pub(crate) fn project_cached_tabs_for_test(&self) -> BTreeSet<u64> {
        self.project_runtimes
            .values()
            .flat_map(|runtime| {
                runtime
                    .views
                    .keys()
                    .chain(runtime.contexts.keys())
                    .chain(runtime.dirty.iter())
                    .map(|key| key.tab)
            })
            .collect()
    }

    #[cfg(test)]
    pub(crate) fn project_tree_dirty(&self) -> bool {
        self.git_status_dirty
    }

    #[cfg(test)]
    pub(crate) fn clear_project_tree_dirty(&mut self) {
        self.git_status_dirty = false;
    }

    #[cfg(test)]
    pub(crate) fn wait_project_view_request(&mut self, screen: &str) {
        for _ in 0..32 {
            let nav = self
                .tab
                .project_navigation
                .as_ref()
                .expect("project navigation");
            let runtime = self
                .project_runtimes
                .get(&nav.project)
                .expect("project runtime");
            if runtime
                .view_request
                .as_ref()
                .is_some_and(|key| key.screen == screen)
            {
                return;
            }
            assert!(
                runtime.busy && !runtime.acting,
                "the requested view must be scheduled"
            );
            self.wait_project_job();
        }
        panic!("project view was not requested");
    }

    #[cfg(test)]
    pub(crate) fn wait_project_view(&mut self) {
        for _ in 0..32 {
            if !self.project_waiting_for_view() {
                return;
            }
            self.wait_project_job();
        }
        panic!("project view did not complete");
    }

    #[cfg(test)]
    pub(crate) fn wait_project_job(&mut self) {
        // Tests wait for a concrete result, never a sleep or a timing assertion.
        if let Some(runtime) = self.project_runtimes.values_mut().find(|r| r.busy) {
            let reply = runtime
                .rx
                .recv_timeout(std::time::Duration::from_secs(120))
                .expect("project worker result");
            runtime.pending = Some(reply);
            self.poll_projects();
        }
    }
}
