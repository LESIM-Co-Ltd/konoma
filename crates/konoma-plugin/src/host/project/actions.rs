//! Project action policy and dispatch. The serial owner alone invokes the session.

use super::*;
use crate::project::ActContext;
use crate::project::{ProjectAct, ProjectRow, RefreshTarget};
use crate::view::ViewReq;
use konoma_host_core::keymap::KeyPress;
use std::collections::BTreeSet;
use std::collections::HashMap;

pub enum ProjectActionScope {
    None,
    Screen {
        plugin: String,
        context: ProjectAct,
        diff: bool,
        jumps: Vec<char>,
    },
    TreeSelection {
        plugins: Vec<String>,
        context: ProjectAct,
    },
    HostDiff {
        plugins: Vec<String>,
    },
}

pub fn acts_in<'a>(
    scope: &ProjectActionScope,
    actions: &'a [ProjectAction],
) -> Vec<&'a ProjectAction> {
    actions
        .iter()
        .filter(|action| match scope {
            ProjectActionScope::None => false,
            ProjectActionScope::Screen {
                plugin,
                context,
                diff,
                ..
            } => {
                action.id.plugin == *plugin
                    && match &action.declaration.project_context {
                        Some(ActContext::Screen { screen, row_kinds }) => {
                            context.screen.as_ref() == Some(screen)
                                && (row_kinds.is_empty()
                                    || context
                                        .selected
                                        .as_ref()
                                        .is_some_and(|row| row_kinds.contains(&row.kind)))
                        }
                        Some(ActContext::Diff) => *diff,
                        _ => false,
                    }
            }
            ProjectActionScope::TreeSelection { plugins, context } => {
                plugins.contains(&action.id.plugin)
                    && matches!(
                        action.declaration.project_context,
                        Some(ActContext::TreeSelection)
                    )
                    && context.selected.is_some()
            }
            ProjectActionScope::HostDiff { plugins } => {
                plugins.contains(&action.id.plugin)
                    && matches!(action.declaration.project_context, Some(ActContext::Diff))
            }
        })
        .collect()
}

pub fn action_key(
    scope: &ProjectActionScope,
    action: &ProjectAction,
    key: KeyPress,
) -> Option<KeyPress> {
    if let ProjectActionScope::Screen { jumps, .. } = scope {
        if jumps.iter().any(|c| key == KeyPress::ch(*c))
            && !matches!(
                action.declaration.project_context,
                Some(ActContext::Screen { .. })
            )
        {
            return None;
        }
    }
    Some(key)
}

#[derive(Clone)]
pub struct ProjectAction {
    pub id: ProjectActionId,
    pub declaration: crate::Act,
    pub key: Option<KeyPress>,
}

#[derive(Clone, Default)]
pub struct ProjectWritePolicy {
    pub declared_read_only: bool,
    pub configured: Option<toml::Value>,
}

impl ProjectWritePolicy {
    pub fn allow_write(&self, writes: bool) -> bool {
        super::super::acts::write_policy(writes, self.declared_read_only, self.configured.as_ref())
            == super::super::acts::WriteVerdict::Allowed
    }
}

pub struct ProjectActFacts<H> {
    pub origin: ProjectOrigin<H>,
    pub binding: bool,
    pub policy: ProjectWritePolicy,
    /// Real tree/diff selection, prepared by the host. Screens use their shared state.
    pub context: ProjectAct,
    pub confirmation: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProjectDispatch {
    Unhandled,
    Handled,
    LeaveProject,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProjectNotice {
    Busy,
    ReadOnly,
    SelectionChanged,
}

pub enum ProjectCommand {
    EnsureView,
    Dispatch(ProjectActionId),
    NewTab,
    LayoutChanged(konoma_host_core::project::DiffLayout),
    Notice(ProjectNotice),
}

pub struct ProjectUpdate<I> {
    pub dispatch: ProjectDispatch,
    pub commands: Vec<ProjectCommand>,
    pub prompt: Option<ProjectPromptUpdate<I>>,
}

impl<I> ProjectUpdate<I> {
    pub fn new(dispatch: ProjectDispatch) -> Self {
        Self {
            dispatch,
            commands: Vec::new(),
            prompt: None,
        }
    }
    pub fn handled() -> Self {
        Self::new(ProjectDispatch::Handled)
    }
    pub fn notice(notice: ProjectNotice) -> Self {
        let mut update = Self::handled();
        update.commands.push(ProjectCommand::Notice(notice));
        update
    }
}

/// The host may keep its effects envelope until effects move into the shared controller.
pub trait ProjectActionInvocation: PromptInvocation + Clone {
    type Effect: Send + 'static;
    fn into_effect(self, tree_status: bool) -> Self::Effect;
}

impl<H: Clone> ProjectActionInvocation for ProjectInvocation<H> {
    type Effect = ();
    fn into_effect(self, _: bool) {}
}

pub struct ProjectHost<'a, I: ProjectActionInvocation> {
    pub runtimes: &'a mut HashMap<ProjectKey, ProjectRuntime<I::Effect, I>>,
    pub dialog: &'a mut Option<ProjectDialog<I>>,
    pub replay: &'a mut ProjectReplayState,
}

impl<'a, I: ProjectActionInvocation> ProjectHost<'a, I> {
    pub fn new(
        runtimes: &'a mut HashMap<ProjectKey, ProjectRuntime<I::Effect, I>>,
        dialog: &'a mut Option<ProjectDialog<I>>,
        replay: &'a mut ProjectReplayState,
    ) -> Self {
        Self {
            runtimes,
            dialog,
            replay,
        }
    }
}

impl<I: ProjectActionInvocation> ProjectHost<'_, I>
where
    I::Surface: Clone + PartialEq,
{
    pub fn request_act(
        &mut self,
        context: &mut ProjectContextMut<'_, I>,
        action: &ProjectAction,
        facts: &ProjectActFacts<I::Surface>,
        wrap: impl FnOnce(ProjectInvocation<I::Surface>) -> I,
    ) -> ProjectUpdate<I> {
        if action.declaration.project_context.is_none() {
            return ProjectUpdate::new(ProjectDispatch::Unhandled);
        }
        if !facts.policy.allow_write(action.declaration.writes) {
            return ProjectUpdate::notice(ProjectNotice::ReadOnly);
        }
        if !self.act_enabled(&facts.origin.project, &action.id) {
            return ProjectUpdate::handled();
        }
        let mut act = context
            .active
            .as_ref()
            .filter(|nav| nav.frame().is_some())
            .map(|nav| nav.act_context(self.content(context)))
            .unwrap_or_else(|| facts.context.clone());
        if action.declaration.copy && act.selected.is_none() {
            if let Some(selection) = context
                .active
                .as_ref()
                .and_then(|nav| nav.frame())
                .and_then(|frame| frame.key.selection.as_ref())
            {
                act = act.selected(ProjectRow::new("selection", selection));
            }
        }
        let invocation = wrap(ProjectInvocation {
            action: action.id.clone(),
            declaration: action.declaration.clone(),
            context: act,
            origin: facts.origin.clone(),
        });
        let mut update = ProjectUpdate::handled();
        update.prompt = Some(prompt::begin_act(
            self.dialog,
            invocation,
            facts.confirmation,
        ));
        update
    }

    /// Fresh policy, binding, origin and row facts are required for every attempt, including replay.
    pub fn execute_act(
        &mut self,
        context: &mut ProjectContextMut<'_, I>,
        invocation: I,
        facts: &ProjectActFacts<I::Surface>,
        mut request: impl FnMut(&ScreenKey) -> Option<ViewReq>,
    ) -> ProjectUpdate<I> {
        let shared = invocation.prompt_invocation();
        if shared.origin != facts.origin {
            return ProjectUpdate::handled();
        }
        if !facts.policy.allow_write(shared.declaration.writes) {
            return ProjectUpdate::notice(ProjectNotice::ReadOnly);
        }
        let Some(runtime) = self.runtimes.get(&shared.origin.project) else {
            return ProjectUpdate::handled();
        };
        if runtime.busy && !runtime.acting {
            return if self.queue_input(context, ProjectInput::Act(Box::new(invocation))) {
                ProjectUpdate::handled()
            } else {
                ProjectUpdate::notice(ProjectNotice::Busy)
            };
        }
        if runtime.busy || !runtime.ready {
            return ProjectUpdate::notice(ProjectNotice::Busy);
        }
        if !runtime
            .enabled
            .get(&shared.action.action)
            .copied()
            .unwrap_or(false)
        {
            return ProjectUpdate::handled();
        }
        if shared.context.screen.is_some() && shared.context.selected.as_ref().is_some_and(|selected| {
            matches!(self.content(context), Some(ProjectContent::List(list)) if !list.rows.iter().any(|row| &ProjectRow::from(row) == selected))
        }) { return ProjectUpdate::notice(ProjectNotice::SelectionChanged); }
        if !facts.binding {
            return ProjectUpdate::handled();
        }
        let mut targets = BTreeSet::new();
        let mut chip = false;
        let mut tree_status = false;
        // An empty refresh still discovers a changed gitdir; only Chip alone skips discovery.
        let mut watch = shared.declaration.refresh.is_empty();
        for target in &shared.declaration.refresh {
            watch |= !matches!(target, RefreshTarget::Chip);
            match target {
                RefreshTarget::CurrentScreen => {
                    if let Some(key) = &shared.origin.screen {
                        targets.insert(key.clone());
                    }
                }
                RefreshTarget::Screen { screen } => {
                    let cached: Vec<_> = runtime
                        .views
                        .keys()
                        .filter(|key| &key.screen == screen)
                        .cloned()
                        .collect();
                    if cached.is_empty() {
                        targets.insert(ScreenKey::new(screen, shared.origin.context));
                    } else {
                        targets.extend(cached);
                    }
                }
                RefreshTarget::AllScreens => {
                    targets.extend(runtime.views.keys().cloned());
                    for screen in &runtime.project.declaration.screens {
                        if !targets.iter().any(|key| key.screen == screen.id) {
                            targets.insert(ScreenKey::new(&screen.id, shared.origin.context));
                        }
                    }
                }
                RefreshTarget::TreeStatus => tree_status = true,
                RefreshTarget::Chip => chip = true,
            }
        }
        let refresh = targets
            .into_iter()
            .filter_map(|key| request(&key).map(|req| (key, req)))
            .collect();
        let retry = shared
            .declaration
            .text_input
            .as_ref()
            .filter(|input| input.keep_on_failure)
            .map(|_| Box::new(invocation.clone()));
        let project = shared.origin.project.clone();
        let action = shared.action.action.clone();
        let act = shared.context.clone();
        let runtime = self.runtimes.get_mut(&project).unwrap();
        let job = ProjectJob::Act {
            action,
            context: act,
            refresh,
            chip,
            watch,
            effect: Box::new(invocation.into_effect(tree_status)),
        };
        if runtime.tx.try_send(job).is_ok() {
            runtime.busy = true;
            runtime.acting = true;
            runtime.pending_retry = retry;
            if let Some(frame) = context.active.as_mut().and_then(|nav| nav.frame_mut()) {
                frame.pending.clear();
            }
        }
        ProjectUpdate::handled()
    }
}

impl<I: ProjectActionInvocation> ProjectHost<'_, I> {
    pub fn acts_in<'b>(
        &self,
        scope: &ProjectActionScope,
        actions: &'b [ProjectAction],
    ) -> Vec<&'b ProjectAction> {
        acts_in(scope, actions)
    }
    pub fn action_project(
        &self,
        context: &ProjectContextMut<'_, I>,
        action: &ProjectActionId,
        detected: &[ProjectKey],
    ) -> Option<ProjectKey> {
        action_project(
            context.active.as_ref().map(|nav| &nav.project),
            context.returned.as_ref().map(|nav| &nav.project),
            detected,
            &action.plugin,
            |key| self.runtimes.contains_key(key),
        )
    }
    pub fn content(&self, context: &ProjectContextMut<'_, I>) -> Option<&ProjectContent> {
        let nav = context.active.as_ref()?;
        let key = &nav.frame()?.key;
        let runtime = self.runtimes.get(&nav.project)?;
        if runtime.contexts.get(key) != context.request.as_ref() {
            return None;
        }
        runtime.views.get(key).map(|content| content.as_ref())
    }

    pub fn act_enabled(&self, project: &ProjectKey, action: &ProjectActionId) -> bool {
        act_enabled(self.runtimes.get(project), &action.action)
    }
}

pub fn act_enabled<E: Send + 'static, I>(
    runtime: Option<&ProjectRuntime<E, I>>,
    action: &str,
) -> bool {
    runtime.is_some_and(|r| r.ready && !r.acting && r.enabled.get(action).copied().unwrap_or(false))
}

pub fn action_project(
    active: Option<&ProjectKey>,
    returned: Option<&ProjectKey>,
    detected: &[ProjectKey],
    plugin: &str,
    contains: impl Fn(&ProjectKey) -> bool,
) -> Option<ProjectKey> {
    if let Some(project) = active {
        return (project.plugin == plugin).then(|| project.clone());
    }
    if let Some(project) = returned.filter(|project| project.plugin == plugin) {
        return Some(project.clone());
    }
    detected
        .iter()
        .find(|project| project.plugin == plugin && contains(project))
        .cloned()
}
