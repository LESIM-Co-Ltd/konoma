//! Owned standalone navigation, borrowing only shared project transitions and drawing.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::time::Duration;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use konoma_host_core::keymap::{
    scheme_from_str, CoreAction, CoreBinding, CoreContext, CoreKeys, CoreResolution, CoreViewType,
    KeyPress, Motion,
};
use konoma_host_core::preview_plan::{fixed_keys, PreviewOp};
use ratatui::{backend::Backend, layout::Rect, Terminal};

use crate::app::{AppActs, AppError, AppEvent, EventSource, Next, Pictures};
use crate::host::project::actions::acts_in;
use crate::host::project::draw::{draw_project, project_body_area, ProjectDrawValues};
use crate::host::project::effects::{self, request};
use crate::host::project::*;
use crate::{project::ProjectAct, Key, KeyMods, Outcome};

use super::PreparedProjects;

/// Context supplied without reading the process environment on the UI thread.
#[derive(Default)]
pub struct ProjectLoopCx {
    pub home: Option<PathBuf>,
}

/// The standalone project surface; it carries no konoma tab or preview state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StandaloneSurface;

/// The shared invocation with the effect its serial owner will complete.
#[derive(Clone)]
pub struct StandaloneInvocation {
    shared: ProjectInvocation<StandaloneSurface>,
    effect: ProjectEffect<StandaloneSurface>,
}

impl PromptInvocation for StandaloneInvocation {
    type Surface = StandaloneSurface;
    fn prompt_invocation(&self) -> &ProjectInvocation<StandaloneSurface> {
        &self.shared
    }
    fn prompt_invocation_mut(&mut self) -> &mut ProjectInvocation<StandaloneSurface> {
        &mut self.shared
    }
}

impl ProjectActionInvocation for StandaloneInvocation {
    type Effect = ProjectEffect<StandaloneSurface>;
    fn into_effect(mut self, tree_status: bool) -> Self::Effect {
        self.effect.tree_status = tree_status;
        self.effect
    }
}

type Runtime = ProjectRuntime<ProjectEffect<StandaloneSurface>, StandaloneInvocation>;

struct ProjectLoopState {
    runtimes: HashMap<ProjectKey, Runtime>,
    screen: Option<ProjectScreenState<StandaloneInvocation>>,
    dialog: Option<ProjectDialog<StandaloneInvocation>>,
    replay: ProjectReplayState,
    pending_acts: VecDeque<StandaloneInvocation>,
    viewport: ProjectViewport,
    keys: CoreKeys,
}

impl ProjectLoopState {
    fn info(
        &self,
        prepared: &PreparedProjects,
        cx: &ProjectLoopCx,
    ) -> ProjectContextInfo<StandaloneSurface> {
        ProjectContextInfo {
            id: 0,
            root: prepared.open_dir.clone(),
            open_dir: prepared.open_dir.clone(),
            home: cx.home.clone(),
            icons: prepared.settings.icons,
            viewport: self.viewport,
            surface: StandaloneSurface,
        }
    }

    fn with_host<T>(
        &mut self,
        info: &ProjectContextInfo<StandaloneSurface>,
        f: impl FnOnce(
            &mut ProjectHost<'_, StandaloneInvocation>,
            &mut ProjectContextMut<'_, StandaloneInvocation>,
        ) -> T,
    ) -> T {
        let request = self
            .screen
            .as_ref()
            .and_then(|nav| nav.frame())
            .and_then(|frame| request(info, &frame.key))
            .and_then(|req| req.project);
        let mut host = ProjectHost::new(&mut self.runtimes, &mut self.dialog, &mut self.replay);
        let mut context = ProjectContextMut {
            id: info.id,
            active: self.screen.as_mut(),
            returned: None,
            pending_acts: &mut self.pending_acts,
            request,
        };
        f(&mut host, &mut context)
    }

    fn ensure(&mut self, info: &ProjectContextInfo<StandaloneSurface>) {
        self.with_host(info, |host, context| host.ensure_view(context, info));
    }

    fn content(
        &self,
        info: &ProjectContextInfo<StandaloneSurface>,
    ) -> Option<std::sync::Arc<ProjectContent>> {
        let nav = self.screen.as_ref()?;
        let key = &nav.frame()?.key;
        let runtime = self.runtimes.get(&nav.project)?;
        (runtime.contexts.get(key) == request(info, key)?.project.as_ref())
            .then(|| runtime.views.get(key).cloned())
            .flatten()
    }

    fn actions(
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
            .map(|a| ProjectAction {
                id: ProjectActionId {
                    plugin: nav.project.plugin.clone(),
                    action: a.id.clone(),
                },
                declaration: a.clone(),
                key: bound
                    .bindings
                    .iter()
                    .find(|b| b.act == a.id)
                    .and_then(|b| b.key),
            })
            .collect();
        // U6b executes only the row's Enter operation. Prompt and menu declarations do not
        // acquire an alternate execution path through List Right.
        acts_in(&scope, &actions)
            .into_iter()
            .filter(|a| {
                a.key == Some(KeyPress::key(KeyCode::Enter))
                    && !a.declaration.copy
                    && a.declaration.confirmation.is_none()
                    && a.declaration.text_input.is_none()
                    && a.declaration.choice_input.is_none()
            })
            .cloned()
            .collect()
    }

    fn act_facts(
        &self,
        prepared: &PreparedProjects,
        info: &ProjectContextInfo<StandaloneSurface>,
        action: &ProjectActionId,
    ) -> Option<ProjectActFacts<StandaloneSurface>> {
        let nav = self.screen.as_ref()?;
        let manifest = prepared.registry.manifest(&action.plugin)?;
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
            confirmation: true,
        })
    }

    fn dispatch_act(
        &mut self,
        prepared: &PreparedProjects,
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
        // Replay is not connected yet: a busy view must not enqueue an invocation for later.
        if self
            .runtimes
            .get(&facts.origin.project)
            .is_none_or(|r| r.busy)
        {
            return;
        }
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
        if let Some(ProjectPromptUpdate::InvocationReady(invocation)) = update.prompt {
            if let Some(fresh) = self.act_facts(prepared, info, &invocation.shared.action) {
                self.with_host(info, |host, context| {
                    host.execute_act(context, invocation, &fresh, |key| {
                        effects::request(info, key)
                    })
                });
            }
        }
    }

    fn apply(
        &mut self,
        prepared: &mut PreparedProjects,
        info: &ProjectContextInfo<StandaloneSurface>,
        update: ProjectUpdate<StandaloneInvocation>,
    ) -> bool {
        // Consume deliveries in their shared order. No generic fallback, prompts or replay.
        for event in update.events {
            match event {
                ProjectEvent::Effect(effect)
                    if matches!(
                        effect.outcome,
                        Some(Outcome::PushScreen { .. } | Outcome::PopScreen)
                    ) =>
                {
                    let update = self.with_host(info, |host, context| {
                        host.apply_effect(context, info, effect)
                    });
                    if self.apply(prepared, info, update) {
                        return true;
                    }
                }
                ProjectEvent::EnsureView => self.ensure(info),
                _ => {}
            }
        }
        if update.dispatch == ProjectDispatch::LeaveProject {
            if !self.with_host(info, |host, context| host.pop_screen(context)) {
                return true;
            }
            self.ensure(info);
        }
        for command in update.commands {
            match command {
                ProjectCommand::EnsureView => self.ensure(info),
                ProjectCommand::LayoutChanged(layout) => prepared.settings.diff_layout = layout,
                ProjectCommand::Dispatch(id) => self.dispatch_act(prepared, info, id),
                _ => {}
            }
        }
        false
    }

    fn key(
        &mut self,
        prepared: &mut PreparedProjects,
        info: &ProjectContextInfo<StandaloneSurface>,
        key: KeyEvent,
    ) -> bool {
        if KeyPress::norm(&key) == KeyPress::ctrl_ch('c') {
            return true;
        }
        if KeyPress::norm(&key) == KeyPress::ctrl_ch('t') || self.dialog.is_some() {
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
        let enter = actions.first().map(|a| a.id.clone());
        let facts = ProjectKeyFacts {
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
            action: (key.code == KeyCode::Enter)
                .then(|| enter.clone())
                .flatten(),
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
        if unhandled && key.code == KeyCode::Enter {
            if let Some(id) = facts.action {
                self.dispatch_act(prepared, info, id);
            }
        }
        // Keys that would wait for a view cannot create a dormant replay path in this unit.
        if let Some(nav) = &mut self.screen {
            for frame in &mut nav.stack {
                frame.pending.clear();
            }
        }
        false
    }

    fn draw(
        &mut self,
        prepared: &PreparedProjects,
        info: &ProjectContextInfo<StandaloneSurface>,
        buffer: &mut ratatui::buffer::Buffer,
        area: Rect,
    ) {
        let Some(nav) = &self.screen else { return };
        let Some(runtime) = self.runtimes.get(&nav.project) else {
            return;
        };
        let Some(frame) = nav.frame() else { return };
        let content = self.content(info);
        let visible = content
            .as_deref()
            .filter(|c| matches!(c, ProjectContent::List(_)))
            .map(|c| nav.visible_rows(c, self.viewport, prepared.settings.diff_layout))
            .unwrap_or_default();
        let unavailable = runtime
            .error
            .as_ref()
            .map(|e| e.message.get(prepared.lang))
            .unwrap_or(konoma_host_core::help_msg::CoreMsg::Loading.tr(prepared.keys.lang()));
        let update = draw_project(
            buffer,
            area,
            ProjectDrawValues {
                declaration: &runtime.project.declaration,
                active_screen: &frame.key.screen,
                cursor: frame.cursor,
                scroll: frame.scroll,
                hscroll: frame.hscroll,
                marked: &frame.marked,
                content: content.as_deref(),
                visible_rows: &visible,
                layout: prepared.settings.diff_layout,
                lang: prepared.lang,
                unavailable,
            },
        );
        if let Some(frame) = self.screen.as_mut().and_then(|n| n.frame_mut()) {
            if let Some(cursor) = update.cursor {
                frame.cursor = cursor;
            }
            if let Some(scroll) = update.scroll {
                frame.scroll = scroll;
            }
            if let Some(hscroll) = update.hscroll {
                frame.hscroll = hscroll;
            }
        }
    }
}

/// Run only project navigation and body drawing. The caller owns terminal entry/teardown.
/// Preparation is borrowed so its owners and corrected navigation survive an injected input
/// source ending. No terminal entry, watcher wakes, menus or launch executor are started here.
pub fn run_project_with<B: Backend, E: EventSource>(
    terminal: &mut Terminal<B>,
    prepared: &mut PreparedProjects,
    events: &mut E,
    cx: &ProjectLoopCx,
    _pictures: Option<Pictures>,
) -> Result<(), AppError> {
    let mut state = ProjectLoopState {
        runtimes: std::mem::take(&mut prepared.runtimes),
        screen: prepared.screen.take(),
        dialog: prepared.dialog.take(),
        replay: ProjectReplayState::default(),
        pending_acts: VecDeque::new(),
        viewport: ProjectViewport::default(),
        keys: CoreKeys::from_keys_config(
            scheme_from_str(&prepared.config.ui.keys),
            &prepared.config.keys,
        ),
    };
    let result = (|| {
        if state.screen.is_none() && state.dialog.is_none() && state.runtimes.len() == 1 {
            let project = state.runtimes.keys().next().unwrap().clone();
            let info = state.info(prepared, cx);
            let first = state.with_host(&info, |host, _| host.first_screen(&project, info.id));
            if let Some(first) = first {
                state.screen = Some(ProjectHost::start_screen(project, first));
            }
        }
        loop {
            let size = terminal
                .size()
                .map_err(|e| AppError::Terminal(e.to_string()))?;
            // Reserve the host's header/footer rows without advertising their later contents.
            let area = Rect::new(0, 1, size.width, size.height.saturating_sub(2));
            let body = project_body_area(area);
            let previous = state.viewport;
            state.viewport = ProjectViewport {
                cols: body.width,
                rows: body.height,
            };
            let info = state.info(prepared, cx);
            state.with_host(&info, |host, context| {
                host.resize(context, previous, info.viewport)
            });
            let retained: Vec<_> = state.runtimes.keys().cloned().collect();
            let update = state.with_host(&info, |host, context| {
                host.poll_with_reply(
                    std::slice::from_mut(context),
                    std::slice::from_ref(&info),
                    &retained,
                    |reply| {
                        if reply.effect.as_ref().is_some_and(|effect| {
                            !matches!(
                                effect.outcome,
                                Some(Outcome::PushScreen { .. } | Outcome::PopScreen)
                            )
                        }) {
                            // Unsupported outcomes cannot change the visible frame indirectly through
                            // their refresh/chip/error deliveries. Still retire the job and adopt the
                            // latest enabled guards; no act is replayed or retried here.
                            reply.views.clear();
                            reply.contexts.clear();
                            reply.chip = None;
                            reply.chip_error = None;
                            reply.error = None;
                            reply.watch_paths = None;
                        }
                    },
                )
            });
            state.apply(prepared, &info, update);
            state.ensure(&info);
            terminal
                .draw(|frame| state.draw(prepared, &info, frame.buffer_mut(), area))
                .map_err(|e| AppError::Terminal(e.to_string()))?;
            let pending = state
                .runtimes
                .values()
                .any(|r| r.busy || r.pending.is_some());
            match events.next(pending.then_some(Duration::from_millis(16))) {
                Next::End => break,
                Next::Event(AppEvent::Key(key)) => {
                    if let Some(key) = key_event(&key) {
                        if state.key(prepared, &info, key) {
                            break;
                        }
                    }
                }
                Next::Event(AppEvent::Resize | AppEvent::Paste(_)) | Next::Timeout => {}
            }
        }
        Ok(())
    })();
    prepared.runtimes = state.runtimes;
    prepared.screen = state.screen;
    prepared.dialog = state.dialog;
    result
}

fn key_event(key: &Key) -> Option<KeyEvent> {
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
