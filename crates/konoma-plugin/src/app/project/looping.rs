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
use crate::host::project::draw::{draw_project, project_body_area, ProjectDrawValues};
use crate::host::project::effects::request;
use crate::host::project::*;
use crate::{Key, KeyMods};

use super::PreparedProjects;

#[path = "effects.rs"]
mod delivery;
#[path = "input.rs"]
mod input;
use input::key_event;

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

/// Run project acts, prompts, navigation and body drawing. The caller owns terminal entry/teardown.
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
                host.poll(
                    std::slice::from_mut(context),
                    std::slice::from_ref(&info),
                    &retained,
                )
            });
            if state.apply(prepared, &info, update) || state.replay_keys(prepared, &info) {
                break;
            }
            state.ensure(&info);
            terminal
                .draw(|frame| {
                    let whole = frame.area();
                    state.draw(prepared, &info, frame.buffer_mut(), area);
                    state.draw_dialog(prepared, frame.buffer_mut(), whole);
                })
                .map_err(|e| AppError::Terminal(e.to_string()))?;
            let pending = state
                .runtimes
                .values()
                .any(|r| r.busy || r.pending.is_some());
            // The shared replay budget is per pass. Finish another slice before waiting for
            // terminal input when no owner reply is needed to make progress.
            if !pending
                && (state
                    .screen
                    .as_ref()
                    .and_then(|nav| nav.frame())
                    .is_some_and(|frame| !frame.pending.is_empty())
                    || !state.pending_acts.is_empty())
            {
                continue;
            }
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
