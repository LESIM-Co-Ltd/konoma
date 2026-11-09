//! Project navigation, action contexts and the shared prompts. Session ownership is on workers.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use konoma_plugin::project::{Confirmation, ProjectAct, ProjectRow};
use konoma_plugin::Outcome;
use ratatui::text::Line;

use super::project_worker::ProjectJob;
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
use konoma_plugin::host::project::input::{self, ProjectKeyFacts};
use konoma_plugin::host::project::{ProjectActionInvocation, ProjectContextMut, ProjectHost};
use konoma_plugin::host::project::{ProjectMotionResult, ProjectScreenState, ProjectViewport};
pub(crate) type ProjectFrame = konoma_plugin::host::project::ProjectFrame<ProjectInvocation>;
type ProjectInput = konoma_plugin::host::project::ProjectInput<ProjectInvocation>;

#[cfg(test)]
pub(crate) use konoma_plugin::host::project::ProjectDiff;
pub(crate) use konoma_plugin::host::project::{ProjectContent, ScreenKey};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ProjectControlId {
    Screens,
    Move,
    Mark,
    Filter,
    Back,
    Help,
    Open,
    NewTab,
    Copy,
    Actions,
    Scroll,
    Hscroll,
    Ends,
    DiffLayout,
    Act,
}

#[derive(Clone)]
pub(crate) struct ProjectControl {
    pub id: ProjectControlId,
    pub keys: Vec<KeyPress>,
    pub label: String,
    shared_notation: bool,
}

impl ProjectControl {
    pub(crate) fn new(id: ProjectControlId, keys: Vec<KeyPress>, label: impl Into<String>) -> Self {
        Self {
            id,
            keys,
            label: label.into(),
            shared_notation: false,
        }
    }

    fn shared(mut self) -> Self {
        self.shared_notation = true;
        self
    }

    pub(crate) fn rendered_keys(&self) -> String {
        self.keys
            .iter()
            .map(|key| {
                if self.shared_notation {
                    if key.code == KeyCode::BackTab && !key.ctrl {
                        "Shift-Tab".into()
                    } else {
                        crate::ops::keys::help_key(*key)
                    }
                } else {
                    crate::ops::keys::footer_key(*key, true)
                }
            })
            .collect::<Vec<_>>()
            .join("/")
    }

    pub(crate) fn render(&self) -> String {
        format!("{}:{}", self.rendered_keys(), self.label)
    }
}
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

#[derive(Clone)]
pub(crate) struct ProjectEffect {
    pub project: ProjectKey,
    pub tab: u64,
    pub tree_root: PathBuf,
    pub view: Option<ScreenKey>,
    pub outcome: Option<Outcome>,
    pub tree_status: bool,
    pub label: String,
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct ProjectSurface {
    mode: Mode,
    path: Option<PathBuf>,
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

/// Refreshes keep the identity the user selected, even when the provider reorders its rows.
fn remap_project_rows(
    tab: &mut PerTab,
    project: &ProjectKey,
    screen: &ScreenKey,
    old: Option<&ProjectContent>,
    new: &ProjectContent,
) {
    for nav in [&mut tab.project_navigation, &mut tab.project_return]
        .into_iter()
        .flatten()
        .filter(|nav| &nav.project == project)
    {
        nav.remap_rows(screen, old, new);
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
        let mut keys = vec![KeyPress::key(KeyCode::Enter)];
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
        Some(ProjectControl::new(ProjectControlId::Open, keys, label))
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
            .map(|frame| self.project_request(&frame.key));
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
            .and_then(|frame| self.project_request(&frame.key).project);
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
        if runtime.contexts.get(key) != self.project_request(key).project.as_ref() {
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
        self.project_row_target().map(|_| {
            ProjectControl::new(
                ProjectControlId::NewTab,
                vec![KeyPress::ctrl_ch('t')],
                tr(self.lang, Msg::HintNewTab),
            )
            .shared()
        })
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
        let Some(runtime) = self.project_runtimes.get(&key) else {
            return;
        };
        let Some(screen) = runtime
            .project
            .declaration
            .screens
            .iter()
            .find(|s| !s.pushed)
        else {
            return;
        };
        let screen = ScreenKey::new(&screen.id, self.tab.id);
        self.show_project_screen(key, screen);
    }

    fn show_project_screen(&mut self, key: ProjectKey, screen: ScreenKey) {
        self.tab.project_navigation = Some(ProjectNavigation {
            state: ProjectScreenState::new(key, screen),
            origin_mode: self.tab.mode,
            origin_path: self.tab.preview_path.clone(),
            origin_kind: self.tab.preview_kind.clone(),
            origin_scroll: self.tab.preview_scroll,
            origin_hscroll: self.tab.preview_hscroll,
        });
        self.ensure_project_view();
    }

    fn ensure_project_view(&mut self) {
        self.initialize_project_frame();
        let Some(nav) = &self.tab.project_navigation else {
            return;
        };
        let Some(frame) = nav.stack.last() else {
            return;
        };
        let req = self.project_request(&frame.key);
        let Some(runtime) = self.project_runtimes.get_mut(&nav.project) else {
            return;
        };
        if runtime.busy
            || !runtime.ready
            || runtime.error.is_some()
            || (runtime.views.contains_key(&frame.key)
                && !runtime.dirty.contains(&frame.key)
                && runtime.contexts.get(&frame.key) == req.project.as_ref())
        {
            return;
        }
        if runtime
            .tx
            .try_send(ProjectJob::View {
                key: frame.key.clone(),
                req,
            })
            .is_ok()
        {
            runtime.busy = true;
            #[cfg(test)]
            {
                runtime.view_request = Some(frame.key.clone());
            }
        }
    }

    fn project_back(&mut self) {
        let Some(mut nav) = self.tab.project_navigation.take() else {
            return;
        };
        if nav.stack.len() > 1 {
            nav.stack.pop();
            self.tab.project_navigation = Some(nav);
            self.ensure_project_view();
            return;
        }
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
        let effect = ProjectEffect {
            project,
            tab: self.tab.id,
            tree_root: self.tab.root.clone(),
            view: self.project_frame().map(|f| f.key.clone()),
            outcome: None,
            tree_status: false,
            label: declaration.declaration.label.get(self.plugin_lang()).into(),
        };
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
        // Prepare one immutable value per live context before borrowing its shared screen state.
        let requests: std::collections::HashMap<_, _> = std::iter::once(&self.tab)
            .chain(self.tabs.iter())
            .map(|tab| (tab.id, self.project_request(&ScreenKey::new("", tab.id))))
            .collect();
        let update = self.with_project_host(|host, context| {
            host.execute_act(context, invocation, &facts, |key| {
                let mut req = requests.get(&key.tab)?.clone();
                let project = req.project.as_mut()?;
                project.tab = key.tab;
                project.screen.clone_from(&key.screen);
                project.selection.clone_from(&key.selection);
                Some(req)
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
        prompt::reopen_text(&mut self.project_dialog, invocation, &current, blockers);
    }

    pub(crate) fn poll_project_jobs(&mut self) -> bool {
        let live_tabs: std::collections::HashSet<_> = std::iter::once(&self.tab)
            .chain(self.tabs.iter())
            .map(|tab| tab.id)
            .collect();
        let retained: std::collections::HashSet<_> = std::iter::once(&self.tab)
            .chain(self.tabs.iter())
            .flat_map(|tab| tab.projects.projects.iter().map(ProjectKey::from))
            .collect();
        self.project_runtimes
            .retain(|key, _| retained.contains(key));
        for project in self.tab.projects.projects.clone() {
            self.start_project_runtime(project);
        }
        let mut changed = false;
        let mut effects = Vec::new();
        let mut watches = Vec::new();
        let mut chip_errors = Vec::new();
        let mut errors = Vec::new();
        let mut retries = Vec::new();
        for (key, runtime) in &mut self.project_runtimes {
            while let Some(reply) = runtime
                .pending
                .take()
                .or_else(|| runtime.rx.try_recv().ok())
            {
                changed = true;
                runtime.busy = false;
                runtime.acting = false;
                if let Some(invocation) = runtime.pending_retry.take().filter(|_| reply.act_failed)
                {
                    retries.push(*invocation);
                }
                #[cfg(test)]
                {
                    runtime.view_request = None;
                }
                runtime.ready |= reply.error.is_none();
                runtime.error = reply.error;
                if let Some(paths) = reply.watch_paths {
                    runtime.watch_paths = Some(paths);
                }
                if !reply.enabled.is_empty() {
                    runtime.enabled = reply.enabled;
                }
                if !runtime.invalidated {
                    runtime.contexts.extend(
                        reply
                            .contexts
                            .into_iter()
                            .filter(|(screen, _)| live_tabs.contains(&screen.tab)),
                    );
                    for (screen, view) in reply
                        .views
                        .into_iter()
                        .filter(|(screen, _)| live_tabs.contains(&screen.tab))
                    {
                        let old = runtime.views.get(&screen).map(Arc::as_ref);
                        remap_project_rows(&mut self.tab, key, &screen, old, &view);
                        for tab in &mut self.tabs {
                            remap_project_rows(tab, key, &screen, old, &view);
                        }
                        runtime.dirty.remove(&screen);
                        runtime.views.insert(screen, view);
                    }
                }
                if let Some(chip) = reply.chip {
                    if !runtime.chip_dirty {
                        runtime.chip = chip;
                    }
                }
                if let Some(error) = reply.chip_error {
                    chip_errors.push((key.clone(), error));
                }
                if runtime.invalidated {
                    runtime.dirty.extend(runtime.views.keys().cloned());
                    runtime.invalidated = false;
                }
                if let Some(effect) = reply.effect {
                    effects.push(effect);
                }
                if let Some(error) = &runtime.error {
                    if self
                        .tab
                        .project_navigation
                        .as_ref()
                        .is_some_and(|nav| &nav.project == key)
                    {
                        errors.push(error.clone());
                    }
                }
            }
            if !runtime.busy && !runtime.closed_tabs.is_empty() {
                match runtime.tx.try_send(ProjectJob::TabsClosed {
                    tabs: runtime.closed_tabs.iter().copied().collect(),
                }) {
                    Ok(()) => {
                        runtime.closed_tabs.clear();
                        runtime.busy = true;
                    }
                    // A failed open has no session left to notify.
                    Err(std::sync::mpsc::TrySendError::Disconnected(_)) => {
                        runtime.closed_tabs.clear()
                    }
                    Err(std::sync::mpsc::TrySendError::Full(_)) => {}
                }
            }
            if let Some(paths) = &runtime.watch_paths {
                watches.push((key.clone(), paths.clone()));
            }
            if runtime.refresh_watch
                && !runtime.busy
                && runtime.ready
                && runtime.tx.try_send(ProjectJob::Watch).is_ok()
            {
                runtime.refresh_watch = false;
                runtime.chip_dirty = false;
                runtime.busy = true;
            }
            if runtime.chip_dirty
                && runtime.ready
                && !runtime.busy
                && runtime.tx.try_send(ProjectJob::Chip).is_ok()
            {
                runtime.chip_dirty = false;
                runtime.busy = true;
            }
        }
        for (key, paths) in watches {
            for tab in std::iter::once(&mut self.tab).chain(self.tabs.iter_mut()) {
                tab.projects
                    .set_watch_paths(&key.plugin, &key.root, &key.marker, paths.clone());
            }
        }
        for effect in effects {
            if effect.tree_status && self.tab.root.starts_with(&effect.project.root) {
                self.git_status_dirty = true;
            }
            if effect.tab == self.tab.id {
                self.apply_project_effect(effect);
            } else if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == effect.tab) {
                tab.project_effects.push(effect);
            }
        }
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
        for error in errors {
            self.flash = Some(error.message.get(self.plugin_lang()).into());
        }
        for (key, error) in chip_errors {
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
        for invocation in retries {
            self.reopen_project_input(invocation);
        }
        changed
    }

    fn initialize_project_frame(&mut self) {
        if let Some(nav) = self.tab.project_navigation.as_mut() {
            if let Some(runtime) = self.project_runtimes.get(&nav.project) {
                if let Some(view) = nav.frame().and_then(|frame| runtime.views.get(&frame.key)) {
                    nav.initialize_frame(view, &runtime.project.declaration);
                }
            }
        }
    }

    fn apply_project_effect(&mut self, effect: ProjectEffect) {
        if self.tab.root != effect.tree_root || matches!(effect.outcome, Some(Outcome::Unhandled)) {
            return;
        }
        if effect.tree_status {
            self.git_status_dirty = true;
        }
        if !matches!(
            effect.outcome,
            Some(Outcome::Redraw | Outcome::PushScreen { .. } | Outcome::PopScreen)
        ) {
            self.flash = Some(effect.label);
        }
        if matches!(
            &effect.outcome,
            Some(
                Outcome::PushScreen { .. }
                    | Outcome::PopScreen
                    | Outcome::OpenFile { .. }
                    | Outcome::OpenDiff { .. }
                    | Outcome::SwitchRoot { .. }
                    | Outcome::OpenTab { .. }
            )
        ) && effect.view.is_some()
            && !self.tab.project_navigation.as_ref().is_some_and(|nav| {
                nav.project == effect.project
                    && nav.stack.last().map(|frame| &frame.key) == effect.view.as_ref()
            })
        {
            return;
        }
        match effect.outcome {
            Some(Outcome::PopScreen) => {
                if self.project_pushed() {
                    self.project_back();
                }
            }
            Some(Outcome::Copy { text }) => self.set_clipboard_flash(&text),
            Some(Outcome::Flash(message)) => {
                self.flash = Some(message.get(self.plugin_lang()).into())
            }
            Some(Outcome::PushScreen { screen, selection }) => {
                if !self
                    .project_runtimes
                    .get(&effect.project)
                    .is_some_and(|runtime| {
                        runtime
                            .project
                            .declaration
                            .screens
                            .iter()
                            .any(|s| s.id == screen)
                    })
                {
                    self.flash = Some(tr(self.lang, Msg::ProjectUnknownScreen).into());
                    return;
                }
                let target = ScreenKey {
                    screen,
                    selection,
                    tab: effect.tab,
                };
                if self.tab.project_navigation.is_none() && effect.view.is_none() {
                    self.show_project_screen(effect.project, target);
                    return;
                }
                let Some(nav) = self.tab.project_navigation.as_mut().filter(|nav| {
                    nav.project == effect.project
                        && nav.stack.last().map(|f| &f.key) == effect.view.as_ref()
                }) else {
                    return;
                };
                nav.stack.push(ProjectFrame::new(target));
            }
            Some(outcome @ (Outcome::OpenFile { .. } | Outcome::OpenDiff { .. })) => {
                self.tab.project_return = self.tab.project_navigation.take();
                let result = self.apply_plugin_navigation(outcome);
                crate::resolve_key_result(self, result.map(|()| false));
            }
            Some(outcome @ Outcome::SwitchRoot { .. }) => {
                let result = self.apply_plugin_navigation(outcome);
                crate::resolve_key_result(self, result.map(|()| false));
            }
            Some(outcome @ Outcome::OpenTab { .. }) => {
                let result = self.apply_plugin_navigation(outcome);
                crate::resolve_key_result(self, result.map(|()| false));
            }
            _ => {}
        }
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

    pub(crate) fn project_control_entries(&self) -> Option<Vec<ProjectControl>> {
        use ProjectControlId as Id;
        if !self.project_active() {
            return None;
        }
        let mut controls = if let Some(controls) = self.project_text_controls() {
            controls
        } else {
            let list = self
                .project_content()
                .is_some_and(|view| matches!(view.as_ref(), ProjectContent::List(_)));
            let mut controls = vec![
                ProjectControl::new(
                    Id::Screens,
                    vec![KeyPress::key(KeyCode::Tab), KeyPress::key(KeyCode::BackTab)],
                    tr(self.lang, Msg::FtScreens),
                )
                .shared(),
                ProjectControl::new(
                    Id::Move,
                    vec![KeyPress::ch('j'), KeyPress::ch('k')],
                    if list {
                        konoma_plugin::t!(en: "move", ja: "移動")
                            .get(self.plugin_lang())
                            .to_string()
                    } else {
                        tr(self.lang, Msg::Scroll).into()
                    },
                )
                .shared(),
            ];
            if list && self.project_marks_enabled() {
                controls.push(
                    ProjectControl::new(
                        Id::Mark,
                        vec![KeyPress::ch('V')],
                        konoma_plugin::t!(en: "mark", ja: "マーク").get(self.plugin_lang()),
                    )
                    .shared(),
                );
            }
            if list && self.project_filter_enabled() {
                controls.push(
                    ProjectControl::new(
                        Id::Filter,
                        vec![KeyPress::ch('/')],
                        tr(self.lang, Msg::HintFilter),
                    )
                    .shared(),
                );
            }
            controls.push(
                ProjectControl::new(
                    Id::Back,
                    vec![KeyPress::ch('q')],
                    konoma_plugin::t!(en: "back", ja: "戻る").get(self.plugin_lang()),
                )
                .shared(),
            );
            controls.push(
                ProjectControl::new(
                    Id::Help,
                    vec![KeyPress::ch('?')],
                    tr(self.lang, Msg::HintHelp),
                )
                .shared(),
            );
            controls
        };
        controls.extend(self.project_open_control());
        controls.extend(self.project_new_tab_control());
        if crate::ops::menu::opens(self, Context::Canvas, crate::keymap::LeaderId::Copy) {
            let keys = self
                .keymaps
                .leader_keys_of(Context::Canvas)
                .into_iter()
                .filter(|(_, id)| *id == crate::keymap::LeaderId::Copy)
                .map(|(key, _)| key)
                .collect::<Vec<_>>();
            if !keys.is_empty() {
                controls.push(ProjectControl::new(
                    Id::Copy,
                    keys,
                    tr(self.lang, Msg::CopyHint),
                ));
            }
        }
        let bindings = self.project_acts_in(Context::Canvas);
        if konoma_plugin::host::acts::menu_candidates(&bindings)
            .iter()
            .any(|(act, _)| {
                crate::ops::action_enabled(
                    self,
                    Context::Canvas,
                    crate::keymap::Action::Plugin(*act),
                )
            })
        {
            controls.push(
                ProjectControl::new(
                    Id::Actions,
                    vec![KeyPress::ch(' ')],
                    tr(self.lang, Msg::WkActionsTitle).trim(),
                )
                .shared(),
            );
        }
        Some(controls)
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
        use crate::keymap::{Action, Motion};
        use ProjectControlId as Id;
        let content = self.project_content()?;
        if matches!(content.as_ref(), ProjectContent::List(_)) {
            return None;
        }
        let groups = [
            (Id::Scroll, vec![Motion::Down, Motion::Up], Msg::Scroll),
            (
                Id::Hscroll,
                vec![Motion::Left, Motion::Right],
                Msg::HintHscroll,
            ),
            (Id::Ends, vec![Motion::Top, Motion::Bottom], Msg::HintEnds),
        ];
        let mut hints = Vec::new();
        for (id, motions, label) in groups {
            let keys = motions
                .into_iter()
                .flat_map(|m| self.keymaps.keys_of(Context::Text, Action::Navigate(m)))
                .collect::<Vec<_>>();
            if !keys.is_empty() {
                hints.push(ProjectControl::new(id, keys, tr(self.lang, label)));
            }
        }
        if matches!(content.as_ref(), ProjectContent::Diff(_)) {
            let keys = self
                .keymaps
                .keys_of(Context::Diff, Action::CycleDiffLayout)
                .into_iter()
                .collect::<Vec<_>>();
            if !keys.is_empty() {
                hints.insert(
                    hints.len().min(2),
                    ProjectControl::new(Id::DiffLayout, keys, tr(self.lang, Msg::FtDiffLayout)),
                );
            }
        }
        if !self.project_pushed() {
            hints.push(
                ProjectControl::new(
                    Id::Screens,
                    vec![KeyPress::key(KeyCode::Tab), KeyPress::key(KeyCode::BackTab)],
                    tr(self.lang, Msg::ProjectCycle),
                )
                .shared(),
            );
        }
        hints.push(
            ProjectControl::new(
                Id::Back,
                vec![KeyPress::ch('q'), KeyPress::key(KeyCode::Esc)],
                tr(self.lang, Msg::ProjectBack),
            )
            .shared(),
        );
        hints.push(
            ProjectControl::new(
                Id::Help,
                vec![KeyPress::ch('?')],
                tr(self.lang, Msg::HintHelp),
            )
            .shared(),
        );
        Some(hints)
    }

    pub(crate) fn project_text_help(&self) -> Option<Vec<(String, String)>> {
        use crate::keymap::{Action, Motion};
        let mut hints = self.project_text_hints()?;
        for (motions, label) in [
            ([Motion::HalfDown, Motion::HalfUp], Msg::HelpHalfPage),
            ([Motion::PageDown, Motion::PageUp], Msg::HintPage),
            ([Motion::LineHome, Motion::LineEnd], Msg::HintLineEnds),
        ] {
            let keys = motions
                .into_iter()
                .flat_map(|m| self.keymaps.keys_of(Context::Text, Action::Navigate(m)))
                .map(crate::ops::keys::help_key)
                .collect::<Vec<_>>()
                .join(" / ");
            if !keys.is_empty() {
                hints.push((keys, tr(self.lang, label).into()));
            }
        }
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
        let Some(nav) = self.tab.project_return.take() else {
            return false;
        };
        self.tab.project_navigation = Some(nav);
        self.ensure_project_view();
        true
    }

    pub(crate) fn resize_project(&mut self, cols: u16, rows: u16) {
        if self.tab.preview_cols != cols || self.tab.preview_viewport != rows {
            self.tab.preview_cols = cols;
            self.tab.preview_viewport = rows;
            if let Some(nav) = &self.tab.project_navigation {
                if let Some(runtime) = self.project_runtimes.get_mut(&nav.project) {
                    runtime.dirty.extend(runtime.views.keys().cloned());
                    runtime.invalidated |= runtime.busy;
                }
            }
        }
        self.ensure_project_view();
    }

    #[cfg(test)]
    pub(crate) fn project_jobs_pending(&self) -> bool {
        self.project_runtimes
            .values()
            .any(|r| r.busy || !r.closed_tabs.is_empty())
    }

    pub(super) fn project_tab_closed(&mut self, tab: u64) {
        for runtime in self.project_runtimes.values_mut() {
            runtime.tab_closed(tab);
            if runtime
                .pending_retry
                .as_ref()
                .is_some_and(|act| act.effect.tab == tab)
            {
                runtime.pending_retry = None;
            }
        }
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
