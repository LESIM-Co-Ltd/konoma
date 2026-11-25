//! Ordered input queues and incremental replay, independent of the ordinary host dispatcher.

use super::actions::{
    ProjectAction, ProjectCommand, ProjectDispatch, ProjectNotice, ProjectUpdate,
    ProjectWritePolicy,
};
use super::*;
use crate::host::bind::jump_target;
use crate::project::ProjectView;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use konoma_host_core::keymap::{KeyPress, Motion};
use konoma_host_core::project::DiffLayout;
use std::collections::VecDeque;

pub struct ProjectKeyFacts {
    pub blocked: bool,
    pub leader: bool,
    pub global: bool,
    pub motion: Option<Motion>,
    pub cycle_layout: bool,
    /// None means no List override; false means an override that is not Right.
    pub list_override: Option<bool>,
    pub text_right: bool,
    pub action: Option<ProjectActionId>,
    pub enter_action: Option<ProjectActionId>,
    pub actions: Vec<ProjectAction>,
    pub policy: ProjectWritePolicy,
    pub viewport: ProjectViewport,
    pub layout: DiffLayout,
    /// The keys that leave the screen (the canvas's back) on this screen: `q` while the host's
    /// effective keymap leaves it as the fixed key, plus any key the keymap binds to the screen's
    /// own back operation (a Ctrl chord included). The footer's controls read the same keys.
    pub back_keys: Vec<KeyPress>,
    /// The keys that start the list filter on this screen, decided the same way as `back_keys`
    /// (`/` is the fixed key).
    pub filter_keys: Vec<KeyPress>,
    /// The screen-jump operations of the project's plugin with the keys they have now
    /// ([`crate::host::bind::bind_jumps`]). A screen's declared key is only its default; the
    /// keys that jump are these.
    pub jumps: Vec<crate::host::bind::JumpBinding>,
}

impl Default for ProjectKeyFacts {
    fn default() -> Self {
        Self {
            blocked: false,
            leader: false,
            global: false,
            motion: None,
            cycle_layout: false,
            list_override: None,
            text_right: false,
            action: None,
            enter_action: None,
            actions: Vec::new(),
            policy: ProjectWritePolicy::default(),
            viewport: ProjectViewport::default(),
            layout: DiffLayout::Unified,
            back_keys: vec![KeyPress::ch('q')],
            filter_keys: vec![KeyPress::ch('/')],
            jumps: Vec::new(),
        }
    }
}

/// The key as the project layer reads it: the code and Ctrl. The project compares modifiers
/// exactly, so a chord with Alt has no form in its key language (`None`). This is the one
/// definition of that reading: every lookup the project layer makes for a key (motions, fixed
/// keys, plugin keys, the keymap) goes through it, in konoma and in the standalone app alike.
pub fn project_key_press(key: &KeyEvent) -> Option<KeyPress> {
    (!key.modifiers.contains(KeyModifiers::ALT)).then(|| KeyPress::norm(key))
}

impl ProjectKeyFacts {
    /// The facts of a key the project layer has no form for ([`project_key_press`] is `None`):
    /// it is in no key table, so it is no motion, action, back or filter key.
    pub fn of_no_key() -> Self {
        Self {
            back_keys: Vec::new(),
            filter_keys: Vec::new(),
            ..Self::default()
        }
    }
}

pub fn list_right(
    list: bool,
    physical: KeyCode,
    list_override: Option<bool>,
    text_right: bool,
) -> bool {
    list && (physical == KeyCode::Right || list_override.unwrap_or(text_right))
}

pub struct ProjectContextMut<'a, I> {
    pub id: u64,
    pub active: Option<&'a mut ProjectScreenState<I>>,
    pub returned: Option<&'a mut ProjectScreenState<I>>,
    pub pending_acts: &'a mut VecDeque<I>,
    /// The freshly prepared view context for the current frame.
    pub request: Option<ProjectView>,
}

pub type ProjectReplay<I> = ProjectInput<I>;

#[derive(Default)]
pub struct ProjectReplayState {
    active: bool,
    remaining: usize,
}

impl ProjectReplayState {
    pub fn is_replaying(&self) -> bool {
        self.active
    }
}

/// The host's refresh key on a project canvas. The one source of that key: [`ProjectHost::handle_key`]
/// matches it and the `Space` menu's refresh item presses it, so the two are one path.
pub const REFRESH_KEY: char = 'r';

impl<I: ProjectActionInvocation> ProjectHost<'_, I> {
    pub fn handle_key(
        &mut self,
        context: &mut ProjectContextMut<'_, I>,
        key: KeyEvent,
        facts: &ProjectKeyFacts,
    ) -> ProjectUpdate<I> {
        if self.dialog.is_some() {
            let mut update = ProjectUpdate::handled();
            update.prompt = Some(prompt::handle_key(self.dialog, key));
            return update;
        }
        if context.active.is_none() || facts.blocked {
            return ProjectUpdate::new(ProjectDispatch::Unhandled);
        }
        if context.active.as_ref().is_some_and(|nav| nav.picker())
            && matches!(key.code, KeyCode::Left | KeyCode::Right)
            && key.modifiers.is_empty()
        {
            return ProjectUpdate::handled();
        }
        // The press as the keymap sees it (code and Ctrl). An Alt chord is never one of these.
        // A key the project layer has no form for is ignored by it, here at its entry: it never
        // reaches the queue below (neither to wait in line behind a read nor to cancel what
        // waits), nor the filter field, nor any fixed key (host and standalone program decide it
        // here, from the one reading of a key).
        let Some(pressed) = project_key_press(&key) else {
            return ProjectUpdate::new(ProjectDispatch::Unhandled);
        };
        let interaction = self.pending_interaction(context, &facts.actions);
        let back_key = facts.back_keys.contains(&pressed);
        let filter_key = facts.filter_keys.contains(&pressed);
        let jump_key = jump_target(&facts.jumps, pressed).is_some();
        let navigation = !interaction
            && !facts.leader
            && (matches!(key.code, KeyCode::Esc | KeyCode::Tab | KeyCode::BackTab)
                || back_key
                || jump_key);
        let global = !interaction
            && (key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                || facts.global);
        let local_movement = !interaction
            && !facts.leader
            && self.content(context).is_some()
            && (matches!(
                key.code,
                KeyCode::Char('j' | 'k' | 'g' | 'G' | 'V')
                    | KeyCode::Up
                    | KeyCode::Down
                    | KeyCode::Home
                    | KeyCode::End
                    | KeyCode::PageUp
                    | KeyCode::PageDown
            ) || filter_key);
        if !navigation && !global && !local_movement && self.must_queue(context) {
            if facts
                .action
                .as_ref()
                .and_then(|id| facts.actions.iter().find(|a| &a.id == id))
                .is_some_and(|a| !facts.policy.allow_write(a.declaration.writes))
            {
                return ProjectUpdate::notice(ProjectNotice::ReadOnly);
            }
            return self.queue_or_busy(context, ProjectInput::Key(key));
        }
        if global {
            if let Some(frame) = context.active.as_mut().and_then(|nav| nav.frame_mut()) {
                frame.pending.clear();
            }
        }
        if facts.leader {
            return ProjectUpdate::new(ProjectDispatch::Unhandled);
        }
        let content = self.content(context).and_then(|_| {
            let nav = context.active.as_ref()?;
            self.runtimes
                .get(&nav.project)?
                .views
                .get(&nav.frame()?.key)
                .cloned()
        });
        let nav = context.active.as_mut().unwrap();
        if nav.handle_filter_key(content.as_deref(), key) {
            return ProjectUpdate::handled();
        }
        let declaration = self
            .runtimes
            .get(&nav.project)
            .map(|r| &r.project.declaration);
        let motion = content
            .as_ref()
            .map(|content| {
                if facts.cycle_layout {
                    nav.cycle_layout(content, facts.layout)
                } else if let Some(motion) = facts.motion {
                    nav.apply_motion(
                        content,
                        motion,
                        key.code,
                        facts.viewport,
                        facts.layout,
                        declaration,
                    )
                } else {
                    ProjectMotionResult::Unhandled
                }
            })
            .unwrap_or(ProjectMotionResult::Unhandled);
        match motion {
            ProjectMotionResult::Back => return ProjectUpdate::new(ProjectDispatch::LeaveProject),
            ProjectMotionResult::Handled => return ProjectUpdate::handled(),
            ProjectMotionResult::LayoutChanged(layout) => {
                let mut update = ProjectUpdate::handled();
                update.commands.push(ProjectCommand::LayoutChanged(layout));
                return update;
            }
            ProjectMotionResult::Unhandled => {}
        }
        let list = matches!(content.as_deref(), Some(ProjectContent::List(_)));
        if list_right(list, key.code, facts.list_override, facts.text_right) {
            let mut update = ProjectUpdate::handled();
            if let Some(action) = &facts.enter_action {
                if self.act_enabled(&nav.project, action) {
                    update
                        .commands
                        .push(ProjectCommand::Dispatch(action.clone()));
                }
            }
            return update;
        }
        if pressed == KeyPress::ctrl_ch('t') {
            let mut update = ProjectUpdate::handled();
            update.commands.push(ProjectCommand::NewTab);
            return update;
        }
        // The keys the host decided are this screen's back and filter operations
        // (`ProjectKeyFacts`) win over the fixed letters below, as a key the config bound to an
        // operation wins on any screen. They come before the Ctrl/Alt return because a screen
        // table may bind a Ctrl chord to one of them.
        if back_key {
            let mut update = ProjectUpdate::handled();
            update.dispatch = ProjectDispatch::LeaveProject;
            return update;
        }
        if filter_key && list && nav.filter_enabled(declaration) {
            nav.start_filter(content.as_deref());
            return ProjectUpdate::handled();
        }
        if key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            return ProjectUpdate::new(ProjectDispatch::Unhandled);
        }
        let mut update = ProjectUpdate::handled();
        match key.code {
            KeyCode::Char(REFRESH_KEY) => update = self.refresh_update(nav),
            KeyCode::Esc if nav.frame().is_some_and(|f| !f.query.is_empty()) => {
                nav.clear_filter(content.as_deref())
            }
            KeyCode::Esc => update.dispatch = ProjectDispatch::LeaveProject,
            KeyCode::Tab | KeyCode::BackTab => {
                let Some(runtime) = self.runtimes.get(&nav.project) else {
                    return ProjectUpdate::new(ProjectDispatch::Unhandled);
                };
                let Some(frame) = nav.frame_mut() else {
                    return ProjectUpdate::new(ProjectDispatch::Unhandled);
                };
                let screens = &runtime.project.declaration.screens;
                if screens.iter().any(|s| s.id == frame.key.screen && s.pushed) {
                    return update;
                }
                let screens: Vec<_> = screens.iter().filter(|s| !s.pushed).collect();
                if screens.is_empty() {
                    return update;
                }
                let at = screens
                    .iter()
                    .position(|s| s.id == frame.key.screen)
                    .unwrap_or(0);
                let next = if key.code == KeyCode::BackTab {
                    (at + screens.len() - 1) % screens.len()
                } else {
                    (at + 1) % screens.len()
                };
                *frame = ProjectFrame::new(ScreenKey::new(&screens[next].id, context.id));
                update.commands.push(ProjectCommand::EnsureView);
            }
            KeyCode::Char('V') if list => {
                if let (Some(content), Some(declaration)) = (&content, declaration) {
                    nav.toggle_mark(content, declaration);
                }
            }
            KeyCode::Char(_) => {
                if facts.action.is_some() {
                    return ProjectUpdate::new(ProjectDispatch::Unhandled);
                }
                if let Some(screen) = jump_target(&facts.jumps, pressed) {
                    if !nav.pushed(declaration) {
                        *nav.frame_mut().unwrap() =
                            ProjectFrame::new(ScreenKey::new(screen, context.id));
                        update.commands.push(ProjectCommand::EnsureView);
                    }
                } else {
                    update.dispatch = ProjectDispatch::Unhandled;
                }
            }
            _ => update.dispatch = ProjectDispatch::Unhandled,
        }
        update
    }
    /// The canvas's refresh as an operation, not a key: the `Space` menu's refresh item and the
    /// host's `refresh` action run it from whichever layer they came, so no key of the config can
    /// turn it into another operation. Busy as the key is: while the screen waits for a read it
    /// answers the busy notice.
    pub fn refresh(&mut self, context: &mut ProjectContextMut<'_, I>) -> ProjectUpdate<I> {
        if context.active.is_none() {
            return ProjectUpdate::new(ProjectDispatch::Unhandled);
        }
        // Through the gate of the key `r`: it waits in line behind a read and behind input that is
        // still pending, and a full queue answers the busy notice.
        if self.must_queue(context) {
            return self.queue_or_busy(context, ProjectInput::Refresh);
        }
        let nav = context.active.as_ref().unwrap();
        self.refresh_update(nav)
    }

    /// The canvas's refresh (the screen's rows are read again). Reached only through the
    /// [`REFRESH_KEY`] arm of [`Self::handle_key`], after the busy guard.
    fn refresh_update(&mut self, nav: &ProjectScreenState<I>) -> ProjectUpdate<I> {
        let mut update = ProjectUpdate::handled();
        self.mark_screen_stale(nav);
        update.commands.push(ProjectCommand::EnsureView);
        update
    }

    /// The one gate of an input that acts on the screen (the key `r` and the direct refresh):
    /// whether it must wait in line, because the screen waits for a read or, outside replay,
    /// input that came earlier is still pending.
    fn must_queue(&self, context: &ProjectContextMut<'_, I>) -> bool {
        self.waiting(context)
            || !self.replay.active
                && context
                    .active
                    .as_ref()
                    .and_then(|nav| nav.frame())
                    .is_some_and(|f| !f.pending.is_empty())
    }

    /// Queue `input` behind what is waiting; a full queue answers the busy notice.
    fn queue_or_busy(
        &mut self,
        context: &mut ProjectContextMut<'_, I>,
        input: ProjectInput<I>,
    ) -> ProjectUpdate<I> {
        if self.queue_input(context, input) {
            ProjectUpdate::handled()
        } else {
            ProjectUpdate::notice(ProjectNotice::Busy)
        }
    }

    pub fn waiting(&self, context: &ProjectContextMut<'_, I>) -> bool {
        waiting(
            context.active.as_deref(),
            self.runtimes,
            context.request.as_ref(),
        )
    }

    pub fn pending_interaction(
        &self,
        context: &ProjectContextMut<'_, I>,
        actions: &[ProjectAction],
    ) -> bool {
        let Some(nav) = context.active.as_ref() else {
            return false;
        };
        let Some(ProjectInput::Key(key)) = nav.frame().and_then(|f| f.pending.front()) else {
            return false;
        };
        key.code == KeyCode::Char(' ')
            || actions.iter().any(|action| {
                action.id.plugin == nav.project.plugin
                    && action.key == Some(KeyPress::norm(key))
                    && (action.declaration.confirmation.is_some()
                        || action.declaration.text_input.is_some()
                        || action.declaration.choice_input.is_some())
            })
    }

    /// False means the bounded queue refused the input; the host supplies its busy notice.
    pub fn queue_input(
        &mut self,
        context: &mut ProjectContextMut<'_, I>,
        input: ProjectInput<I>,
    ) -> bool {
        if let Some(nav) = context.active.as_mut() {
            if let Some(frame) = nav.frame_mut() {
                if frame.pending.len() >= 4096 {
                    return false;
                }
                if self.replay.active {
                    frame.pending.push_front(input);
                } else {
                    frame.pending.push_back(input);
                }
            }
        } else if let ProjectInput::Act(invocation) = input {
            if context.pending_acts.len() >= 4096 {
                return false;
            }
            context.pending_acts.push_back(*invocation);
        }
        true
    }

    pub fn begin_replay(&mut self) -> bool {
        if self.replay.active {
            return false;
        }
        self.replay.active = true;
        self.replay.remaining = 64;
        true
    }

    /// Return just one raw item. The caller must dispatch it before asking for another.
    pub fn next_replay(
        &mut self,
        context: &mut ProjectContextMut<'_, I>,
    ) -> Option<ProjectReplay<I>> {
        if !self.replay.active || self.replay.remaining == 0 || self.waiting(context) {
            return None;
        }
        let input = context
            .active
            .as_mut()
            .and_then(|nav| nav.frame_mut())
            .and_then(|frame| frame.pending.pop_front());
        let input = input.or_else(|| {
            let invocation = context.pending_acts.front()?.prompt_invocation();
            if self
                .runtimes
                .get(&invocation.origin.project)
                .is_some_and(|runtime| runtime.busy)
            {
                return None;
            }
            context
                .pending_acts
                .pop_front()
                .map(|invocation| ProjectInput::Act(Box::new(invocation)))
        });
        if input.is_some() {
            self.replay.remaining -= 1;
        }
        input
    }

    pub fn end_replay(&mut self) {
        self.replay.active = false;
    }

    pub fn cancel_input(&mut self, context: &mut ProjectContextMut<'_, I>) {
        context.pending_acts.clear();
        if let Some(nav) = context.active.as_mut() {
            while nav.stack.len() > 1
                && nav.frame().is_some_and(|frame| {
                    self.runtimes.get(&nav.project).is_some_and(|runtime| {
                        runtime
                            .project
                            .declaration
                            .screens
                            .iter()
                            .any(|screen| screen.id == frame.key.screen && screen.transient)
                    })
                })
            {
                nav.stack.pop();
            }
            for frame in &mut nav.stack {
                frame.pending.clear();
            }
        }
    }
}

pub fn waiting<I: ProjectActionInvocation>(
    nav: Option<&ProjectScreenState<I>>,
    runtimes: &std::collections::HashMap<ProjectKey, ProjectRuntime<I::Effect, I>>,
    request: Option<&ProjectView>,
) -> bool {
    let Some(nav) = nav else {
        return false;
    };
    let Some(frame) = nav.frame() else {
        return false;
    };
    runtimes.get(&nav.project).is_some_and(|runtime| {
        !runtime.acting
            && runtime.error.is_none()
            && (!runtime.ready
                || runtime.busy
                || runtime.contexts.get(&frame.key) != request
                || !runtime.views.contains_key(&frame.key)
                || runtime.dirty.contains(&frame.key))
    })
}
