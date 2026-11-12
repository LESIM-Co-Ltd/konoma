//! Reply adoption and ordered deliveries, independent of the host's tabs and surfaces.

use super::*;
use crate::project::ProjectView;
use crate::view::ViewReq;
use crate::{Failure, Outcome};
use std::path::PathBuf;
use std::sync::Arc;

/// A closed or mismatched context never borrows another surface's paths.
pub fn request<H>(info: &ProjectContextInfo<H>, key: &ScreenKey) -> Option<ViewReq> {
    if info.id != key.tab {
        return None;
    }
    let mut view = ProjectView::new(&key.screen)
        .tab(key.tab)
        .icons(info.icons)
        .paths(&info.root, &info.open_dir)
        .home_dir(info.home.clone());
    if let Some(selection) = &key.selection {
        view = view.selection(selection);
    }
    Some(
        ViewReq::new(
            if info.viewport.cols == 0 {
                80
            } else {
                info.viewport.cols
            },
            info.viewport.rows.max(1),
        )
        .project(view),
    )
}

/// Resolve a refresh against the same live-context snapshot used by polling.
pub fn request_for<H>(live: &[ProjectContextInfo<H>], key: &ScreenKey) -> Option<ViewReq> {
    request(live.iter().find(|info| info.id == key.tab)?, key)
}

#[derive(Clone)]
/// A snapshot supplied for each live context, including inactive surfaces.
pub struct ProjectContextInfo<H> {
    pub id: u64,
    pub root: PathBuf,
    pub open_dir: PathBuf,
    pub home: Option<PathBuf>,
    pub icons: bool,
    pub viewport: ProjectViewport,
    pub surface: H,
}

#[derive(Clone)]
/// Effects retain the same request-time origin as prompts and invocations.
pub struct ProjectEffect<H> {
    pub origin: ProjectOrigin<H>,
    pub outcome: Option<Outcome>,
    pub tree_status: bool,
    pub label: String,
}

impl<H> ProjectEffect<H> {
    /// Create a request-time payload before the owner attaches its outcome and refresh flags.
    pub fn new(origin: ProjectOrigin<H>, label: impl Into<String>) -> Self {
        Self {
            origin,
            outcome: None,
            tree_status: false,
            label: label.into(),
        }
    }
}

pub enum ProjectEvent<I: ProjectActionInvocation> {
    Watch(ProjectKey, Vec<PathBuf>),
    Effect(I::Effect),
    /// The host drains delayed effects and resolves pending discovery before ensuring the view.
    EnsureView,
    Error(Failure),
    ChipError(ProjectKey, Failure),
    Retry(I),
}

#[derive(Clone, Copy)]
pub enum ProjectInvalidation {
    Detection,
    Resize,
}

impl<I: ProjectActionInvocation> ProjectHost<'_, I> {
    pub fn ensure_project(
        &mut self,
        project: MatchedProject,
        runtime: ProjectRuntime<I::Effect, I>,
    ) {
        self.runtimes
            .entry(ProjectKey::from(&project))
            .or_insert(runtime);
    }

    pub fn first_screen(&self, project: &ProjectKey, context: u64) -> Option<ScreenKey> {
        let screen = self
            .runtimes
            .get(project)?
            .project
            .declaration
            .screens
            .iter()
            .find(|s| !s.pushed)?;
        Some(ScreenKey::new(&screen.id, context))
    }

    pub fn start_screen(project: ProjectKey, screen: ScreenKey) -> ProjectScreenState<I> {
        ProjectScreenState::new(project, screen)
    }

    pub fn ensure_view(
        &mut self,
        context: &mut ProjectContextMut<'_, I>,
        info: &ProjectContextInfo<I::Surface>,
    ) {
        let Some(nav) = context.active.as_mut() else {
            return;
        };
        let Some(runtime) = self.runtimes.get_mut(&nav.project) else {
            return;
        };
        if let Some(view) = nav.frame().and_then(|frame| runtime.views.get(&frame.key)) {
            nav.initialize_frame(view, &runtime.project.declaration);
        }
        let Some(frame) = nav.frame() else {
            return;
        };
        let Some(req) = request(info, &frame.key).filter(|_| context.id == info.id) else {
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
            #[cfg(any(test, feature = "test-hooks"))]
            {
                runtime.view_request = Some(frame.key.clone());
            }
        }
    }

    /// The user's reload of the current screen: its rows are read again (the next
    /// [`ProjectHost::ensure_view`] sends the job). The cursor follows the selected row, like a
    /// reload from the screen's own `r`.
    pub(super) fn mark_screen_stale(&mut self, nav: &ProjectScreenState<I>) {
        if let Some(runtime) = self.runtimes.get_mut(&nav.project) {
            if let Some(frame) = nav.frame() {
                runtime.dirty.insert(frame.key.clone());
            }
            runtime.invalidated |= runtime.busy;
            runtime.error = None;
        }
    }

    /// A reload of the current screen that starts the list over: the same reading as `r`, and
    /// the cursor is on the first row when the new rows arrive.
    pub fn reload_screen_from_top(&mut self, context: &mut ProjectContextMut<'_, I>) {
        if let Some(nav) = context.active.as_mut() {
            nav.top_on_next_reload();
            self.mark_screen_stale(nav);
        }
    }

    /// Viewport storage belongs to the host; pass both measurements rather than mirror it here.
    pub fn resize(
        &mut self,
        context: &mut ProjectContextMut<'_, I>,
        previous: ProjectViewport,
        viewport: ProjectViewport,
    ) {
        if previous != viewport {
            if let Some(nav) = context.active.as_ref() {
                self.invalidate(&nav.project, ProjectInvalidation::Resize);
            }
        }
    }

    /// Move the opaque host wrapper together with its shared screen state.
    pub fn restore_return<T>(active: &mut Option<T>, returned: &mut Option<T>) -> bool {
        let Some(state) = returned.take() else {
            return false;
        };
        *active = Some(state);
        true
    }
    pub fn reopen_retry(
        &mut self,
        invocation: I,
        current: &ProjectOrigin<I::Surface>,
        blockers: ProjectPromptBlockers,
    ) -> bool {
        prompt::reopen_text(self.dialog, invocation, current, blockers)
    }
    pub fn apply_effect(
        &mut self,
        context: &mut ProjectContextMut<'_, I>,
        current: &ProjectContextInfo<I::Surface>,
        effect: ProjectEffect<I::Surface>,
    ) -> ProjectUpdate<I> {
        let mut update = ProjectUpdate::handled();
        if current.root != effect.origin.root
            || current.id != effect.origin.context
            || context.id != effect.origin.context
            || matches!(effect.outcome, Some(Outcome::Unhandled))
        {
            return update;
        }
        if effect.tree_status {
            update.commands.push(ProjectCommand::TreeStatus);
        }
        if !matches!(
            effect.outcome,
            Some(Outcome::Redraw | Outcome::PushScreen { .. } | Outcome::PopScreen)
        ) {
            update.commands.push(ProjectCommand::Label(effect.label));
        }
        update.changed = !update.commands.is_empty();
        let navigation = matches!(
            effect.outcome,
            Some(
                Outcome::PushScreen { .. }
                    | Outcome::PopScreen
                    | Outcome::OpenFile { .. }
                    | Outcome::OpenDiff { .. }
                    | Outcome::SwitchRoot { .. }
                    | Outcome::OpenTab { .. }
            )
        );
        if navigation
            && effect.origin.screen.is_some()
            && !context.active.as_ref().is_some_and(|nav| {
                nav.project == effect.origin.project
                    && nav.frame().map(|f| &f.key) == effect.origin.screen.as_ref()
            })
        {
            return update;
        }
        match effect.outcome {
            Some(Outcome::PopScreen) => {
                if self.pop_screen(context) {
                    update.commands.push(ProjectCommand::EnsureView);
                    update.changed = true;
                }
            }
            Some(
                outcome @ (Outcome::OpenFile { .. }
                | Outcome::OpenDiff { .. }
                | Outcome::SwitchRoot { .. }
                | Outcome::OpenTab { .. }),
            ) => {
                let save_return =
                    matches!(outcome, Outcome::OpenFile { .. } | Outcome::OpenDiff { .. });
                update.commands.push(ProjectCommand::Navigation {
                    outcome,
                    save_return,
                });
                update.changed = true;
            }
            Some(Outcome::Copy { text }) => update.commands.push(ProjectCommand::Copy(text)),
            Some(Outcome::Flash(message)) => update.commands.push(ProjectCommand::Flash(message)),
            Some(Outcome::PushScreen { screen, selection }) => {
                if !self
                    .runtimes
                    .get(&effect.origin.project)
                    .is_some_and(|runtime| {
                        runtime
                            .project
                            .declaration
                            .screens
                            .iter()
                            .any(|s| s.id == screen)
                    })
                {
                    update.commands.push(ProjectCommand::UnknownScreen);
                    update.changed = true;
                    return update;
                }
                let target = ScreenKey {
                    screen,
                    selection,
                    tab: effect.origin.context,
                };
                if context.active.is_none() && effect.origin.screen.is_none() {
                    update.commands.push(ProjectCommand::StartScreen {
                        project: effect.origin.project,
                        screen: target,
                    });
                    update.changed = true;
                } else if let Some(nav) = context.active.as_mut().filter(|nav| {
                    nav.project == effect.origin.project
                        && nav.frame().map(|f| &f.key) == effect.origin.screen.as_ref()
                }) {
                    nav.stack.push(ProjectFrame::new(target));
                    update.changed = true;
                }
            }
            _ => {}
        }
        update
    }

    /// Returns false at the root; only the host can restore its saved preview surface.
    pub fn pop_screen(&mut self, context: &mut ProjectContextMut<'_, I>) -> bool {
        if let Some(nav) = context.active.as_mut().filter(|nav| nav.stack.len() > 1) {
            nav.stack.pop();
            true
        } else {
            false
        }
    }

    pub fn invalidate(&mut self, project: &ProjectKey, cause: ProjectInvalidation) {
        if let Some(runtime) = self.runtimes.get_mut(project) {
            runtime.dirty.extend(runtime.views.keys().cloned());
            if matches!(cause, ProjectInvalidation::Detection) {
                runtime.refresh_watch = true;
                runtime.error = None;
                runtime.chip_dirty = true;
            }
            runtime.invalidated |= runtime.busy;
        }
    }
    pub fn close_context(&mut self, id: u64) {
        for runtime in self.runtimes.values_mut() {
            runtime.tab_closed(id);
            if runtime
                .pending_retry
                .as_ref()
                .is_some_and(|i| i.prompt_invocation().origin.context == id)
            {
                runtime.pending_retry = None;
            }
        }
    }

    /// The first context is the foreground context for view-error notices. Every context's
    /// active and return state participates in row remapping. Delivery ordering keeps view
    /// ensuring before errors and retries, with host effects accepted one at a time.
    pub fn poll(
        &mut self,
        contexts: &mut [ProjectContextMut<'_, I>],
        live: &[ProjectContextInfo<I::Surface>],
        retained: &[ProjectKey],
    ) -> ProjectUpdate<I> {
        self.runtimes.retain(|key, _| retained.contains(key));
        let mut update = ProjectUpdate::handled();
        let mut effects = Vec::new();
        let mut errors = Vec::new();
        let mut chip_errors = Vec::new();
        let mut retries = Vec::new();
        for (key, runtime) in self.runtimes.iter_mut() {
            while let Some(reply) = runtime
                .pending
                .take()
                .or_else(|| runtime.rx.try_recv().ok())
            {
                update.changed = true;
                runtime.busy = false;
                runtime.acting = false;
                if let Some(invocation) = runtime.pending_retry.take().filter(|_| reply.act_failed)
                {
                    retries.push(ProjectEvent::Retry(*invocation));
                }
                #[cfg(any(test, feature = "test-hooks"))]
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
                    let mut launch_targets = reply.launch_targets;
                    runtime.contexts.extend(
                        reply
                            .contexts
                            .into_iter()
                            .filter(|(screen, _)| live.iter().any(|info| info.id == screen.tab)),
                    );
                    for (screen, view) in reply
                        .views
                        .into_iter()
                        .filter(|(screen, _)| live.iter().any(|info| info.id == screen.tab))
                    {
                        let old = runtime.views.get(&screen).map(Arc::as_ref);
                        for context in contexts.iter_mut() {
                            for state in [&mut context.active, &mut context.returned]
                                .into_iter()
                                .flatten()
                            {
                                if &state.project == key {
                                    state.remap_rows(&screen, old, &view);
                                }
                            }
                        }
                        runtime.dirty.remove(&screen);
                        runtime.launch_targets.insert(
                            screen.clone(),
                            launch_targets.remove(&screen).unwrap_or_default(),
                        );
                        runtime.views.insert(screen, view);
                    }
                }
                if let Some(chip) = reply.chip {
                    if !runtime.chip_dirty {
                        runtime.chip = chip;
                    }
                }
                if let Some(error) = reply.chip_error {
                    chip_errors.push(ProjectEvent::ChipError(key.clone(), error));
                }
                if runtime.invalidated {
                    runtime.dirty.extend(runtime.views.keys().cloned());
                    runtime.invalidated = false;
                }
                if let Some(effect) = reply.effect {
                    effects.push(ProjectEvent::Effect(effect));
                }
                if let Some(error) = &runtime.error {
                    if contexts
                        .first()
                        .and_then(|c| c.active.as_ref())
                        .is_some_and(|nav| &nav.project == key)
                    {
                        errors.push(ProjectEvent::Error(error.clone()));
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
                    Err(std::sync::mpsc::TrySendError::Disconnected(_)) => {
                        runtime.closed_tabs.clear()
                    }
                    Err(std::sync::mpsc::TrySendError::Full(_)) => {}
                }
            }
            if let Some(paths) = &runtime.watch_paths {
                update
                    .events
                    .push(ProjectEvent::Watch(key.clone(), paths.clone()));
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
        update.events.extend(effects);
        update.events.push(ProjectEvent::EnsureView);
        update.events.extend(errors);
        update.events.extend(chip_errors);
        update.events.extend(retries);
        update
    }
}
