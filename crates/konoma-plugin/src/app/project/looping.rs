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
#[cfg(any(test, feature = "test-hooks"))]
#[path = "idle.rs"]
mod idle;
#[path = "input.rs"]
mod input;
#[path = "presentation.rs"]
mod presentation;
#[path = "root_switch.rs"]
pub(super) mod root_switch;
use input::key_event;

#[cfg(test)]
#[path = "preview_tests.rs"]
mod preview_tests;

/// Context supplied without reading the process environment on the UI thread.
#[derive(Default)]
pub struct ProjectLoopCx {
    pub home: Option<PathBuf>,
    pub launch: Option<crate::app::LaunchCx>,
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
    file_preview: Option<super::preview::FilePreviewState>,
    preview_open: Option<super::preview::PendingPreview>,
    launch_enabled: bool,
    launch: super::launch::LaunchState,
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
    root_switch: Option<root_switch::PendingRootSwitch>,
}

impl ProjectLoopState {
    fn new(prepared: &mut PreparedProjects, launch_enabled: bool) -> Self {
        Self {
            file_preview: prepared.file_preview.take(),
            preview_open: prepared.preview_open.take(),
            launch_enabled,
            launch: super::launch::LaunchState::default(),
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
            root_switch: prepared.root_switch.take(),
        }
    }
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
    let mut state = ProjectLoopState::new(prepared, cx.launch.is_some());
    #[cfg(any(test, feature = "test-hooks"))]
    let mut fence = idle::FrameFence::default();
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
            if state.poll_root_switch(prepared, pictures.as_mut()) {
                redraw = true;
            }
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
            #[cfg(any(test, feature = "test-hooks"))]
            if events.project_fenced() && state.file_preview.is_some() {
                // A Preview yields to its owner without tearing down its file runner.
                if state
                    .runtimes
                    .values()
                    .any(|r| r.busy || r.pending.is_some())
                {
                    state.wait_owners_for_test();
                    let retained: Vec<_> = state.runtimes.keys().cloned().collect();
                    let update = state.with_host(&info, |host, context| {
                        host.poll(
                            std::slice::from_mut(context),
                            std::slice::from_ref(&info),
                            &retained,
                        )
                    });
                    if state.apply(prepared, &info, update) {
                        break;
                    }
                    continue;
                }
                prepared.detection.fence_watch_for_test();
                if !prepared.detection.watch_settled_for_test() {
                    continue;
                }
            }
            // A resumed Preview owns this navigation until it leaves. A late raw reply
            // cannot discard its source diff or take its project return owner away.
            if let Some(diff) = state.diff.as_mut().filter(|_| state.file_preview.is_none()) {
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
            if state.preview_open.as_ref().is_some_and(|pending| {
                state
                    .diff
                    .as_ref()
                    .is_none_or(|diff| diff.closed || !pending.matches(&diff.return_info()))
            }) {
                state.preview_open = None;
            }
            if state.diff_return.is_some()
                && state.preview_open.is_none()
                && state.file_preview.is_none()
            {
                if let Some(diff) = state.diff.as_mut() {
                    if let Some(crate::app::diff_screen::DiffScreenCommand::OpenPreview { path }) =
                        diff.take_command()
                    {
                        let source = diff.return_info();
                        debug_assert_eq!(source.path, path);
                        state.preview_open =
                            super::preview::PendingPreview::start(prepared, source);
                    }
                }
            }
            if let Some(result) = state.preview_open.as_ref().and_then(|p| p.poll()) {
                let pending = state.preview_open.take().unwrap();
                match result {
                    Ok(opened) => {
                        crate::app::edit_line::clear();
                        if let Some(p) = pictures.as_mut() {
                            p.begin_file(&opened.plugin_id, &opened.path);
                        }
                        state.file_preview = Some(pending.finish(
                            opened,
                            prepared,
                            cx,
                            state.diff.as_mut().unwrap(),
                        ));
                    }
                    Err(error) => prepared.notice = Some(error.to_string()),
                }
                redraw = true;
            }
            #[cfg(any(test, feature = "test-hooks"))]
            let preview_snapshot = events
                .project_fenced()
                .then(|| state.idle_snapshot(prepared, &info, pictures.as_ref(), &fence));
            if let Some(preview) = &mut state.file_preview {
                #[cfg(any(test, feature = "test-hooks"))]
                if events.project_fenced() {
                    preview.gutter.detection = Some(prepared.detection.clone());
                    preview.gutter.idle = preview_snapshot;
                }
                preview.gutter.owner_wake = (state
                    .runtimes
                    .values()
                    .any(|r| r.busy || r.pending.is_some())
                    || prepared.detection.pending())
                .then_some(Duration::from_millis(16));
                let context = preview.context(prepared, cx);
                let exit = crate::app::looping::run_file_with_preview(
                    terminal,
                    &mut *preview.opened.session,
                    events,
                    &context,
                    &mut preview.runner,
                    pictures.as_mut(),
                    Some(&mut preview.gutter),
                )?;
                #[cfg(any(test, feature = "test-hooks"))]
                if let Some(snapshot) = &preview.gutter.idle {
                    fence.adopt(snapshot);
                }
                match exit {
                    crate::app::looping::FileLoopExit::ReturnToSource => {
                        state.file_preview = None;
                        if let Some(p) = pictures.as_mut() {
                            p.forget();
                            p.forget_still();
                        }
                        if let Err(error) = state.diff.as_mut().unwrap().return_from_preview() {
                            prepared.notice = Some(error.message.get(prepared.lang).into());
                        }
                        redraw = true;
                    }
                    crate::app::looping::FileLoopExit::Yield => {
                        let retained: Vec<_> = state.runtimes.keys().cloned().collect();
                        let update = state.with_host(&info, |host, context| {
                            host.poll(
                                std::slice::from_mut(context),
                                std::slice::from_ref(&info),
                                &retained,
                            )
                        });
                        if state.apply(prepared, &info, update) {
                            break;
                        }
                        state.ensure(&info);
                        continue;
                    }
                    crate::app::looping::FileLoopExit::Back => {
                        debug_assert_eq!(
                            preview.source.path,
                            state.diff.as_ref().unwrap().request.path
                        );
                        state.file_preview = None;
                        state.diff = None;
                        state.screen = state.diff_return.take();
                        if let Some(p) = pictures.as_mut() {
                            p.forget();
                            p.forget_still();
                            p.reset_document();
                            p.doc_pics().prune_media_diff_picture_cache();
                        }
                        redraw = true;
                    }
                    crate::app::looping::FileLoopExit::Quit
                    | crate::app::looping::FileLoopExit::End => break,
                }
            }
            if let Some(p) = pictures.as_mut() {
                p.poll();
                redraw |= p.take_doc_relayout() | p.take_doc_redraw();
            }
            if let Some(diff) = state.diff.as_mut().filter(|d| !d.closed) {
                diff.launch_enabled = state.launch_enabled;
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
            redraw |= state
                .launch
                .poll(terminal, prepared, cx, pictures.as_mut())?;
            if state.launch.take_refresh() {
                if let Some(project) = state
                    .screen
                    .as_ref()
                    .or(state.diff_return.as_ref())
                    .map(|nav| nav.project.clone())
                {
                    state.with_host(&info, |host, _| {
                        host.invalidate(&project, ProjectInvalidation::Detection)
                    });
                }
                if let Some(diff) = state.diff.as_mut().filter(|diff| !diff.closed) {
                    if let Err(error) = diff.reload() {
                        prepared
                            .notice
                            .get_or_insert_with(|| error.message.get(prepared.lang).into());
                    }
                }
            }
            state.ensure(&info);
            #[cfg(any(test, feature = "test-hooks"))]
            if events.project_fenced() && redraw {
                fence.changed();
            }
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
                                p.draw(frame.buffer_mut(), area, false);
                                p.doc_pics().draw(frame.buffer_mut(), area, None);
                            }
                            state.draw(prepared, &info, frame.buffer_mut(), area);
                            state.presentation = state.presentation(prepared, &info);
                            state.draw_chrome(prepared, frame.buffer_mut(), whole);
                        }
                        #[cfg(any(test, feature = "test-hooks"))]
                        crate::app::draw::record_test_frame(frame.buffer_mut());
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
                #[cfg(any(test, feature = "test-hooks"))]
                if events.project_fenced() {
                    fence.drawn();
                }
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
            let preview_wake = state.file_preview.as_ref().and_then(|p| p.gutter.wake_in());
            let picture_wake = pictures.as_ref().and_then(Pictures::wake_in);
            let owner_wake = (pending
                || prepared.detection.pending()
                || state.launch.pending()
                || state.preview_open.is_some()
                || state.root_switch.is_some())
            .then_some(Duration::from_millis(16));
            let watch_wake = prepared
                .detection
                .watching()
                .then_some(Duration::from_millis(100));
            let wake_in = [
                diff_wake,
                preview_wake,
                picture_wake,
                owner_wake,
                watch_wake,
            ]
            .into_iter()
            .flatten()
            .min();
            #[cfg(any(test, feature = "test-hooks"))]
            let fenced = if events.project_fenced() {
                prepared.detection.fence_watch_for_test();
                let snapshot = state.idle_snapshot(prepared, &info, pictures.as_ref(), &fence);
                if !snapshot.is_idle() && !prepared.test_observe_pending {
                    state.wait_idle_for_test(prepared, &snapshot, pictures.as_mut());
                    redraw = true;
                    continue;
                }
                state.publish_state_for_test(prepared, &info);
                Some(events.project_idle(snapshot, crate::app::draw::test_frame()))
            } else {
                None
            };
            #[cfg(any(test, feature = "test-hooks"))]
            let next = fenced.unwrap_or_else(|| events.next(wake_in));
            #[cfg(not(any(test, feature = "test-hooks")))]
            let next = events.next(wake_in);
            match next {
                Next::End => break,
                Next::Event(AppEvent::Key(key)) => {
                    redraw = true;
                    if let Some(key) = key_event(&key) {
                        let was_diff = state.diff_return.is_some();
                        if state.key(prepared, &info, key) {
                            break;
                        }
                        if was_diff && state.diff_return.is_none() {
                            state.preview_open = None;
                            if let Some(p) = pictures.as_mut() {
                                p.reset_document();
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
    prepared.file_preview = state.file_preview;
    prepared.preview_open = state.preview_open;
    prepared.diff = state.diff;
    prepared.diff_return = state.diff_return;
    prepared.screen = state.screen;
    prepared.dialog = state.dialog;
    prepared.root_switch = state.root_switch;
    result
}
