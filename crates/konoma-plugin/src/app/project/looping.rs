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

#[path = "chrome.rs"]
mod chrome;
#[path = "effects.rs"]
mod delivery;
#[path = "input.rs"]
mod input;
#[path = "presentation.rs"]
mod presentation;
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
    diff: Option<crate::app::diff_screen::DiffScreenState>,
    diff_return: Option<ProjectScreenState<StandaloneInvocation>>,
    runtimes: HashMap<ProjectKey, Runtime>,
    screen: Option<ProjectScreenState<StandaloneInvocation>>,
    dialog: Option<ProjectDialog<StandaloneInvocation>>,
    replay: ProjectReplayState,
    pending_acts: VecDeque<StandaloneInvocation>,
    viewport: ProjectViewport,
    keys: CoreKeys,
    help: Option<u16>,
    menu: Option<konoma_host_core::keymap::CoreMenu>,
    presentation: Option<presentation::Presentation>,
}

impl ProjectLoopState {
    fn poll_detection(
        &mut self,
        prepared: &mut PreparedProjects,
        info: &ProjectContextInfo<StandaloneSurface>,
    ) -> bool {
        let previously_ready = !prepared.detection.pending();
        let detection = prepared.detection.poll();
        if previously_ready && detection == Some(false) {
            for project in &prepared.detection.projects {
                self.with_host(info, |host, _| {
                    host.invalidate(&ProjectKey::from(project), ProjectInvalidation::Detection)
                });
            }
        }
        if detection == Some(true) {
            prepared.notice = Some(super::watch_warning(prepared.lang).into());
        }
        detection.is_some()
    }

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
            returned: self.diff_return.as_mut(),
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

/// Run project acts, prompts, navigation and presentation. The caller owns terminal entry/teardown.
/// Preparation is borrowed so its owners and corrected navigation survive an injected input
/// source ending. The caller supplies terminal entry and launch capabilities.
pub fn run_project_with<B: Backend, E: EventSource>(
    terminal: &mut Terminal<B>,
    prepared: &mut PreparedProjects,
    events: &mut E,
    cx: &ProjectLoopCx,
    mut pictures: Option<Pictures>,
) -> Result<(), AppError> {
    let mut state = ProjectLoopState {
        diff: prepared.diff.take(),
        diff_return: prepared.diff_return.take(),
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
        help: None,
        menu: None,
        presentation: None,
    };
    let result = (|| {
        if state.screen.is_none()
            && state.diff_return.is_none()
            && state.diff.is_none()
            && state.dialog.is_none()
            && state.runtimes.len() == 1
        {
            let project = state.runtimes.keys().next().unwrap().clone();
            let info = state.info(prepared, cx);
            let first = state.with_host(&info, |host, _| host.first_screen(&project, info.id));
            if let Some(first) = first {
                state.screen = Some(ProjectHost::start_screen(project, first));
            }
        }
        let mut redraw = true;
        if prepared.watch_failed {
            prepared.notice = Some(super::watch_warning(prepared.lang).into());
        }
        loop {
            let size = terminal
                .size()
                .map_err(|e| AppError::Terminal(e.to_string()))?;
            // Match the host's split status rows; tabs and source-screen context stay there.
            let area = Rect::new(0, 1, size.width, size.height.saturating_sub(2));
            let body = project_body_area(area);
            let previous = state.viewport;
            state.viewport = ProjectViewport {
                cols: body.width,
                rows: body.height,
            };
            let info = state.info(prepared, cx);
            redraw |= previous != state.viewport;
            state.with_host(&info, |host, context| {
                host.resize(context, previous, info.viewport)
            });
            redraw |= state.poll_detection(prepared, &info);
            if let Some(diff) = &mut state.diff {
                match diff.poll() {
                    Ok(true) => {
                        if state.diff_return.is_none() {
                            state.diff_return = state.screen.take();
                        }
                        redraw = true;
                    }
                    Ok(false) => {}
                    Err(error) => {
                        prepared.notice = Some(error.message.get(prepared.lang).into());
                        if let Some(p) = pictures.as_mut() {
                            p.doc_pics().prune_media_diff_picture_cache();
                        }
                        state.diff = None;
                        if state.screen.is_none() {
                            state.screen = state.diff_return.take();
                        }
                        redraw = true;
                    }
                }
            }
            if let Some(p) = pictures.as_mut() {
                p.poll();
                redraw |= p.take_doc_relayout() | p.take_doc_redraw();
            }
            if let Some(diff) = state.diff.as_mut().filter(|d| !d.closed) {
                if let Some(p) = pictures.as_mut() {
                    p.doc_pics().set_path(&diff.request.path);
                }
                redraw |= diff.prepare_frame(area, pictures.as_mut().map(Pictures::doc_pics));
                if let Some(note) = diff.take_notice() {
                    prepared.notice = Some(note);
                    redraw = true;
                }
            }
            let retained: Vec<_> = state.runtimes.keys().cloned().collect();
            let update = state.with_host(&info, |host, context| {
                host.poll(
                    std::slice::from_mut(context),
                    std::slice::from_ref(&info),
                    &retained,
                )
            });
            redraw |= update.changed;
            if state.apply(prepared, &info, update) {
                break;
            }
            let (quit, replayed) = state.replay_keys(prepared, &info);
            redraw |= replayed;
            if quit {
                break;
            }
            state.ensure(&info);
            if redraw {
                terminal
                    .draw(|frame| {
                        let whole = frame.area();
                        if let Some(bg) = prepared.display.bg() {
                            frame
                                .buffer_mut()
                                .set_style(whole, ratatui::style::Style::new().bg(bg));
                        }
                        if state.diff_return.is_some() {
                            let diff = state.diff.as_mut().unwrap();
                            let footer = diff.footer(prepared.notice.as_deref(), whole.width);
                            let mut borrowed = crate::app::draw::Frame::Diff(diff);
                            crate::app::draw::draw_in(
                                frame,
                                &mut borrowed,
                                &footer,
                                prepared.display.bg(),
                                area,
                            );
                            if let Some(p) = pictures.as_mut() {
                                state.diff.as_ref().unwrap().draw_overlay(
                                    p.doc_pics(),
                                    frame.buffer_mut(),
                                    area,
                                );
                            }
                            state
                                .diff
                                .as_ref()
                                .unwrap()
                                .draw_header(frame.buffer_mut(), whole);
                            state
                                .diff
                                .as_ref()
                                .unwrap()
                                .draw_help(frame.buffer_mut(), whole);
                        } else {
                            if let Some(p) = pictures.as_mut() {
                                p.doc_pics().draw(frame.buffer_mut(), area, None);
                            }
                            state.draw(prepared, &info, frame.buffer_mut(), area);
                            state.presentation = state.presentation(prepared, &info);
                            state.draw_chrome(prepared, frame.buffer_mut(), whole);
                        }
                    })
                    .map_err(|e| AppError::Terminal(e.to_string()))?;
                if let Some(p) = pictures.as_mut() {
                    if p.take_moved() {
                        terminal
                            .clear()
                            .map_err(|e| AppError::Terminal(e.to_string()))?;
                        redraw = true;
                        continue;
                    }
                    p.flush_deletes();
                }
                redraw = false;
            }
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
            let diff_wake = state.diff.as_ref().and_then(|d| d.wake_in());
            let picture_wake = pictures.as_ref().and_then(Pictures::wake_in);
            let owner_wake =
                (pending || prepared.detection.pending()).then_some(Duration::from_millis(16));
            let watch_wake = prepared
                .detection
                .watching()
                .then_some(Duration::from_millis(100));
            let wake_in = [diff_wake, picture_wake, owner_wake, watch_wake]
                .into_iter()
                .flatten()
                .min();
            match events.next(wake_in) {
                Next::End => break,
                Next::Event(AppEvent::Key(key)) => {
                    redraw = true;
                    if let Some(key) = key_event(&key) {
                        let was_diff = state.diff_return.is_some();
                        if state.key(prepared, &info, key) {
                            break;
                        }
                        if was_diff && state.diff_return.is_none() {
                            if let Some(p) = pictures.as_mut() {
                                p.doc_pics().prune_media_diff_picture_cache();
                            }
                        }
                    }
                }
                Next::Event(AppEvent::Resize) => redraw = true,
                Next::Event(AppEvent::Paste(_)) | Next::Timeout => {}
            }
        }
        Ok(())
    })();
    if state.diff_return.is_some() {
        if let Some(p) = pictures.as_mut() {
            p.doc_pics().prune_media_diff_picture_cache();
        }
    }
    prepared.runtimes = state.runtimes;
    prepared.diff = state.diff;
    prepared.diff_return = state.diff_return;
    prepared.screen = state.screen;
    prepared.dialog = state.dialog;
    result
}
