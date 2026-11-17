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
#[path = "quit_guard.rs"]
mod quit_guard;
#[path = "root_switch.rs"]
pub(super) mod root_switch;
use input::key_event;
pub use quit_guard::quit_while_write_text;

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
    preview_project: bool,
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
    /// The "quit while a write runs?" question is open (see `quit_guard`).
    quit_confirm: bool,
    /// A project this loop shows is no longer detected: the program ends with the no-project
    /// error as soon as no write is running (see `Self::no_project_exit`).
    project_gone: bool,
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
            preview_project: prepared.preview_project,
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
            quit_confirm: false,
            project_gone: false,
        }
    }

    /// Whether the loop ends now because its project is gone. A running write is waited for, not
    /// asked about: ending would orphan it.
    fn no_project_exit(&self) -> bool {
        self.project_gone && !self.runtimes.values().any(|r| r.write_act_running())
    }

    fn poll_detection(
        &mut self,
        prepared: &mut PreparedProjects,
        info: &ProjectContextInfo<StandaloneSurface>,
    ) -> bool {
        let previously_ready = !prepared.detection.pending();
        let keys = |projects: &[crate::host::project::detection::MatchedProject]| {
            projects.iter().map(ProjectKey::from).collect::<Vec<_>>()
        };
        let before = keys(&prepared.detection.projects);
        let detection = prepared.detection.poll();
        let after = keys(&prepared.detection.projects);
        // An adopted answer retires the projects it no longer lists, failed or not (a watch failure
        // after a successful scan still carries the new list). A failed scan keeps the previous
        // projects, so nothing differs then.
        if previously_ready
            && (detection == Some(false) || (detection.is_some() && before != after))
        {
            // A project that is no longer listed is retired. This program shows projects only:
            // when the one on screen (or the last one) is gone, it ends like a start in a
            // directory with no project. A gone project that is not shown is just dropped.
            for gone in before.iter().filter(|key| !after.contains(key)) {
                let shown = self
                    .screen
                    .as_ref()
                    .into_iter()
                    .chain(self.diff_return.as_ref())
                    .any(|nav| &nav.project == gone);
                // A confirmation or text input of the gone project can no longer act; it must
                // not keep the keys while a running write of that project is waited for.
                if crate::host::project::prompt::close_act_dialog_of(&mut self.dialog, gone) {
                    self.project_gone = true;
                }
                self.pending_acts
                    .retain(|invocation| &invocation.prompt_invocation().origin.project != gone);
                if shown {
                    self.project_gone = true;
                } else if !self
                    .runtimes
                    .get(gone)
                    .is_some_and(|r| r.write_act_running())
                {
                    self.runtimes.remove(gone);
                }
            }
            // A chooser that lists a gone project drops it; with none left nothing can be opened.
            let emptied = crate::host::project::prompt::retain_projects(&mut self.dialog, |key| {
                after.contains(key)
            });
            // Runtimes are created only at start (see `prepare`): a project detected later has
            // none and cannot be opened by this program. So no runtime left means nothing this
            // program can open remains, and it ends like a start without a project (exit code 2).
            if emptied || self.runtimes.is_empty() || after.is_empty() {
                self.project_gone = true;
            }
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

    /// Whether the diff screen's view (the diff screen, or its Preview representation) is in
    /// front: it is open, it was opened from a project screen, and no project screen covers it.
    fn diff_view_in_front(&self) -> bool {
        self.diff.as_ref().is_some_and(|diff| !diff.closed)
            && self.diff_return.is_some()
            && !self.preview_project
    }

    /// The first project screen covers the diff screen (`p` from the diff screen or from its
    /// Preview representation). Covering is leaving, as in the host
    /// (`App::navigation_over_here`): everything the diff screen's view asked goes
    /// (`DiffScreenState::leave`), nothing asks it again while covered (a refresh is held,
    /// `DiffScreenState::hold_refresh`), and the return (`ProjectDispatch::LeaveProject`) asks it
    /// once. Its pictures are left by the run loop, which owns them (`diff_view_in_front`).
    fn cover_diff_screen(
        &mut self,
        info: &ProjectContextInfo<StandaloneSurface>,
        pictures: Option<&mut Pictures>,
    ) {
        let project = self.diff_return.as_ref().unwrap().project.clone();
        let Some(first) = self.with_host(info, |host, _| host.first_screen(&project, info.id))
        else {
            return;
        };
        self.screen = Some(ProjectHost::start_screen(project, first));
        self.preview_project = true;
        self.leave_diff_view(pictures);
        self.ensure(info);
    }

    /// The diff screen's view (the diff screen or its Preview representation) stops being in
    /// front: a project screen covers it, or it went back to its project screen. Covering is
    /// leaving, as in the host (`App::leave_tab_view`): what the view owns that asks a worker,
    /// takes an answer or wakes the loop is left here, in one place, and the return reads it all
    /// once, after the file is read again (`ProjectDispatch::LeaveProject`). The pictures are
    /// owned by the run loop, which calls this again with them (`diff_view_in_front`); leaving
    /// twice is leaving once. Every field is named, so a new one is decided here.
    fn leave_diff_view(&mut self, pictures: Option<&mut Pictures>) {
        let Self {
            // The project screens and their owners: what is in front now.
            runtimes: _,
            screen: _,
            dialog: _,
            replay: _,
            pending_acts: _,
            presentation: _,
            root_switch: _,
            project_gone: _,
            // Where the view goes back to, and whether it is covered: the return's own state.
            diff_return: _,
            preview_project: _,
            // The program's keys, size, help, menu and launcher: they serve the screen in front.
            launch_enabled: _,
            launch: _,
            viewport: _,
            keys: _,
            help: _,
            menu: _,
            // A write that runs goes on to its end (the quit guard asks about it).
            quit_confirm: _,
            file_preview,
            preview_open,
            diff,
        } = self;
        // An open of the Preview that is out: the return asks it again (`resume_project_diff`).
        *preview_open = None;
        if let Some(diff) = diff {
            diff.leave();
        }
        if let Some(preview) = file_preview {
            // The pose is the user's view: taken before the pictures are left.
            preview.leave(pictures.as_deref().and_then(Pictures::still_pose));
        }
        if let Some(pictures) = pictures {
            pictures.leave_view();
        }
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
        let jumps = self.jumps(prepared);
        let update = draw_project(
            buffer,
            area,
            ProjectDrawValues {
                jumps: &jumps,
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
                query: &frame.query,
            },
        );
        #[cfg(any(test, feature = "test-hooks"))]
        crate::app::draw::record_test_canvas(
            area,
            matches!(
                content.as_deref(),
                Some(ProjectContent::List(_) | ProjectContent::Diff(_))
            ),
            buffer,
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
        // The pictures of the diff screen's view, left as a konoma tab leaves its own
        // (`leave_tab_view`): when the view stops being in front by any road (a project screen
        // covers it, it goes back to its project screen, it is discarded or fails), what
        // animates or may yet animate goes (`ProjectLoopState::leave_diff_view`); the return draws
        // and decodes them again.
        let mut diff_view_was_in_front = state.diff_view_in_front();
        loop {
            let diff_view_in_front = state.diff_view_in_front();
            if diff_view_was_in_front && !diff_view_in_front {
                state.leave_diff_view(pictures.as_mut());
            }
            diff_view_was_in_front = diff_view_in_front;
            if state.poll_root_switch(prepared, pictures.as_mut()) {
                redraw = true;
            }
            let size = terminal
                .size()
                .map_err(|e| AppError::Terminal(e.to_string()))?;
            let combined =
                prepared.services.project_diff_chrome() && prepared.display.combined_status;
            let area = crate::app::draw::project_preview_body(
                Rect::new(0, 0, size.width, size.height),
                combined,
            );
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
            if state.no_project_exit() {
                return Err(AppError::NoProject(prepared.open_dir.clone()));
            }
            redraw |= state
                .launch
                .poll(terminal, prepared, cx, pictures.as_mut())?;
            let tool_returned = state.launch.take_refresh();
            if tool_returned {
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
            }
            // The project screen in front is read again above; the diff screen (or its Preview)
            // is read only while it is in front (see `cover_diff_screen`). Behind a project
            // screen the read is held: the diff screen's return reads it anyway
            // (`resume_project_diff`), the Preview's return takes it here.
            if let Some(diff) = state.diff.as_mut().filter(|diff| !diff.closed) {
                if tool_returned {
                    diff.hold_refresh();
                }
            }
            if let Some(diff) = state
                .diff
                .as_mut()
                .filter(|diff| !diff.closed && !state.preview_project)
            {
                if diff.take_held_refresh() {
                    if let Err(error) = diff.reload() {
                        prepared
                            .notice
                            .get_or_insert_with(|| error.message.get(prepared.lang).into());
                    }
                    if state.file_preview.is_some() {
                        if let Some(p) = pictures.as_mut() {
                            p.doc_pics().prune_media_diff_picture_cache();
                        }
                        state.preview_open =
                            super::preview::PendingPreview::start(prepared, diff.return_info());
                    }
                }
            }
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
            if state.diff.as_mut().is_some_and(|d| d.take_discarded()) {
                let diff = state.diff.as_mut().unwrap();
                let label = prepared
                    .display
                    .path_style
                    .format(&prepared.open_dir, &diff.request.path);
                prepared.notice = Some(format!(
                    "{}: {label}",
                    if prepared.lang == "ja" {
                        "破棄しました"
                    } else {
                        "Discarded"
                    }
                ));
                diff.close();
                state.screen = state.diff_return.take();
                if let Some(project) = state.screen.as_ref().map(|n| n.project.clone()) {
                    state.with_host(&info, |host, _| {
                        host.invalidate(&project, ProjectInvalidation::Detection)
                    });
                }
                redraw = true;
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
            #[cfg(any(test, feature = "test-hooks"))]
            if events.project_fenced() && !prepared.test_observe_pending {
                if let Some(pending) = state.preview_open.as_mut() {
                    pending.fence_for_test();
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
                        let mut refreshed =
                            pending.finish(opened, prepared, cx, state.diff.as_mut().unwrap());
                        // The file opened again (a Refresh, a tool's return, the return to a
                        // covered Preview): the new loop is put where the user was, once, and
                        // the pictures of the old document go, so the new one decodes what it
                        // draws (a konoma tab drops its changed pictures). Nothing of the old
                        // loop (the session it converted to) is kept.
                        if let Some(old) = state.file_preview.take() {
                            let back = old.into_position();
                            if let Some(p) = pictures.as_mut() {
                                p.reset_document();
                            }
                            if let Some((position, pose)) = back {
                                if let Some(runner) = refreshed.runner.as_mut() {
                                    runner.restore(position);
                                }
                                if let (Some(p), Some(pose)) = (pictures.as_mut(), pose) {
                                    p.restore_still_pose(pose);
                                }
                            }
                            refreshed.gutter.retain_position();
                        }
                        state.file_preview = Some(refreshed);
                    }
                    Err(error) => prepared.notice = Some(error.to_string()),
                }
                redraw = true;
            }
            // A left Preview runs only once its open lands. When that open is gone (it failed, a
            // key on the diff screen dropped it, or none could start) the Preview is left for its
            // source, as `ReturnToSource` leaves it, so nothing waits on an open that will not come.
            if !state.preview_project
                && state.preview_open.is_none()
                && state
                    .file_preview
                    .as_ref()
                    .is_some_and(|preview| preview.runner.is_none())
            {
                state.file_preview = None;
                if let Some(p) = pictures.as_mut() {
                    p.forget();
                    p.forget_still();
                }
                if let Some(diff) = state.diff.as_mut().filter(|diff| {
                    !diff.closed
                        && diff.return_info().choice == konoma_host_core::project::DiffView::Preview
                }) {
                    if let Err(error) = diff.return_from_preview() {
                        prepared.notice = Some(error.message.get(prepared.lang).into());
                    }
                }
                redraw = true;
            }
            #[cfg(any(test, feature = "test-hooks"))]
            let preview_snapshot = (events.project_fenced() || prepared.test_probe.is_some())
                .then(|| state.idle_snapshot(prepared, &info, pictures.as_ref(), &fence));
            #[cfg(any(test, feature = "test-hooks"))]
            if (events.project_fenced() || prepared.test_probe.is_some())
                && state.file_preview.is_some()
            {
                state.publish_state_for_test(prepared, &info, pictures.as_ref());
            }
            // The Preview in front runs; a left one (`runner` is `None`) waits for its open, and the
            // diff screen is in front meanwhile, as before the first open lands.
            if let Some(preview) = state
                .file_preview
                .as_mut()
                .filter(|preview| !state.preview_project && preview.runner.is_some())
            {
                #[cfg(any(test, feature = "test-hooks"))]
                if events.project_fenced() || prepared.test_probe.is_some() {
                    preview.gutter.detection = Some(prepared.detection.clone());
                    preview.gutter.idle = preview_snapshot;
                }
                preview.gutter.owner_wake = (state.preview_open.is_some()
                    || state.launch.pending()
                    || state
                        .runtimes
                        .values()
                        .any(|r| r.busy || r.pending.is_some())
                    || prepared.detection.pending())
                .then_some(Duration::from_millis(16));
                let context = preview.context(prepared, cx);
                let runner = preview.runner.as_mut().expect("a live Preview");
                let exit = crate::app::looping::run_file_with_preview(
                    terminal,
                    &mut *preview.opened.session,
                    events,
                    &context,
                    runner,
                    pictures.as_mut(),
                    Some(&mut preview.gutter),
                )?;
                #[cfg(any(test, feature = "test-hooks"))]
                if let Some(snapshot) = &preview.gutter.idle {
                    fence.adopt(snapshot);
                }
                if preview.gutter.project_owned && exit != crate::app::looping::FileLoopExit::Yield
                {
                    let display = runner.display().clone();
                    prepared.adopt_display(display.clone());
                    if let Some(diff) = state.diff.as_mut() {
                        diff.adopt_display(display);
                    }
                }
                match exit {
                    crate::app::looping::FileLoopExit::Project => {
                        state.cover_diff_screen(&info, pictures.as_mut())
                    }
                    crate::app::looping::FileLoopExit::Tool => {
                        let service = state.diff.as_ref().unwrap().request.info.clone();
                        state.launch.tool(prepared, &service);
                    }
                    crate::app::looping::FileLoopExit::Refresh => {
                        if let Some(p) = pictures.as_mut() {
                            p.doc_pics().prune_media_diff_picture_cache();
                        }
                        let project = state.diff_return.as_ref().unwrap().project.clone();
                        state.with_host(&info, |host, _| {
                            host.invalidate(&project, ProjectInvalidation::Detection)
                        });
                        if let Err(error) = state.diff.as_mut().unwrap().reload() {
                            prepared.notice = Some(error.message.get(prepared.lang).into());
                        }
                        state.preview_open = super::preview::PendingPreview::start(
                            prepared,
                            state.diff.as_ref().unwrap().return_info(),
                        );
                    }
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
                        let keep_media = state.diff.as_ref().is_some_and(|d| d.project_owned);
                        state.file_preview = None;
                        state.diff = None;
                        state.screen = state.diff_return.take();
                        if let Some(p) = pictures.as_mut() {
                            p.forget();
                            p.forget_still();
                            p.reset_document();
                            if !keep_media {
                                p.doc_pics().prune_media_diff_picture_cache();
                            }
                        }
                        redraw = true;
                    }
                    crate::app::looping::FileLoopExit::Quit => {
                        if state.request_quit() {
                            break;
                        }
                        // A write is running: leave the preview for the diff screen it was
                        // opened from (as `ReturnToSource` does), where the question is drawn.
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
                    crate::app::looping::FileLoopExit::End => break,
                }
                // Preview may have resized the terminal. Recompute the owner's rectangles
                // and viewport before its first returning frame.
                continue;
            }
            if let Some(p) = pictures.as_mut() {
                p.poll();
                redraw |= p.take_doc_relayout() | p.take_doc_redraw();
            }
            if let Some(diff) = state
                .diff
                .as_mut()
                .filter(|d| !d.closed && !state.preview_project)
            {
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
            if state.preview_project {
                if let Some(diff) = state.diff.as_mut().filter(|d| !d.closed) {
                    diff.drain_media_covered();
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
                        if state.diff_return.is_some() && !state.preview_project {
                            let state_quit_dialog = state.quit_presentation(prepared);
                            let diff = state.diff.as_mut().unwrap();
                            let footer_width = crate::app::draw::status_footer_width(
                                whole.width,
                                combined,
                                &diff.context_spans(),
                            );
                            let quit = state_quit_dialog.as_ref();
                            let footer = match quit {
                                Some(dialog) => crate::host::project::draw::project_dialog_footer(
                                    dialog,
                                    footer_width,
                                ),
                                None => diff.footer(prepared.notice.as_deref(), footer_width),
                            };
                            let mut borrowed = crate::app::draw::Frame::Diff(diff);
                            crate::app::draw::draw_in(
                                frame,
                                &mut borrowed,
                                &[],
                                prepared.display.bg(),
                                area,
                            );
                            // The file diff view has its own frame (the old hub's), not the list
                            // frame: its cells are not moved.
                            #[cfg(any(test, feature = "test-hooks"))]
                            crate::app::draw::record_test_canvas(area, false, frame.buffer_mut());
                            if let Some(p) = pictures.as_mut() {
                                state.diff.as_ref().unwrap().draw_overlay(
                                    p.doc_pics(),
                                    frame.buffer_mut(),
                                    area,
                                );
                            }
                            state.diff.as_ref().unwrap().draw_chrome(
                                frame.buffer_mut(),
                                whole,
                                &footer,
                            );
                            state
                                .diff
                                .as_mut()
                                .unwrap()
                                .draw_help(frame.buffer_mut(), whole);
                            state
                                .diff
                                .as_ref()
                                .unwrap()
                                .draw_discard(frame.buffer_mut(), whole);
                            if let Some(dialog) = &state_quit_dialog {
                                crate::host::project::draw::draw_project_dialog(
                                    frame.buffer_mut(),
                                    whole,
                                    dialog,
                                );
                            }
                        } else {
                            if let Some(p) = pictures.as_mut() {
                                p.draw(frame.buffer_mut(), area, false);
                                p.doc_pics().draw(frame.buffer_mut(), area, None);
                            }
                            state.draw(prepared, &info, frame.buffer_mut(), area);
                            state.presentation = state.presentation(prepared, &info);
                            state.draw_chrome(prepared, frame.buffer_mut(), whole);
                        }
                        crate::app::draw::finish_frame(frame.buffer_mut());
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
            if let Some(probe) = &prepared.test_probe {
                prepared.detection.fence_watch_for_test();
                let snapshot = state.idle_snapshot(prepared, &info, pictures.as_ref(), &fence);
                if !snapshot.owners
                    || !snapshot.contexts
                    || !snapshot.detection
                    || !snapshot.watch
                    || !snapshot.navigation
                    || !snapshot.replay
                    || state.preview_open.is_some()
                {
                    state.wait_project_for_test(prepared, &snapshot);
                    redraw = true;
                    continue;
                }
                state.publish_state_for_test(prepared, &info, pictures.as_ref());
                let observed = prepared
                    .test_observed
                    .as_ref()
                    .unwrap()
                    .lock()
                    .unwrap()
                    .clone()
                    .unwrap();
                probe.capture(snapshot, crate::app::draw::test_frame(), observed);
            }
            #[cfg(any(test, feature = "test-hooks"))]
            let fenced = if events.project_fenced() {
                prepared.detection.fence_watch_for_test();
                let snapshot = state.idle_snapshot(prepared, &info, pictures.as_ref(), &fence);
                if !snapshot.is_idle() && !prepared.test_observe_pending {
                    state.wait_idle_for_test(prepared, &snapshot, pictures.as_mut());
                    redraw = true;
                    continue;
                }
                state.publish_state_for_test(prepared, &info, pictures.as_ref());
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
                        if let Some(diff) = state.diff.as_ref().filter(|diff| diff.project_owned) {
                            prepared.adopt_display(diff.display().clone());
                        }
                        if was_diff && state.diff_return.is_none() {
                            state.preview_open = None;
                            if let Some(p) = pictures.as_mut() {
                                p.reset_document();
                                if !state.diff.as_ref().is_some_and(|d| d.project_owned) {
                                    p.doc_pics().prune_media_diff_picture_cache();
                                }
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
    prepared.preview_project = state.preview_project;
    prepared.screen = state.screen;
    prepared.dialog = state.dialog;
    prepared.root_switch = state.root_switch;
    result
}
