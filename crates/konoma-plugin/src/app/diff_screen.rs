//! Standalone file-diff ownership. Raw retrieval and file eligibility run on a service worker.
use std::path::PathBuf;
use std::sync::{mpsc, Arc};
use std::time::Duration;

use konoma_host_core::help::{default_help, default_label, CoreLookup, HelpOp, KeyLookup};
use konoma_host_core::help_msg::{CoreMsg, DiffMsg};
use konoma_host_core::keymap::{
    scheme_from_str, CoreAction, CoreContext, CoreKeys, KeyPress, Motion,
};
use konoma_host_core::lang::Lang;
use konoma_host_core::project::{DiffLayout, DiffView};
use konoma_host_core::shared::SharedConfig;
use konoma_vcs_core::git::DiffLine;
use ratatui::{buffer::Buffer, layout::Rect, widgets::Block};

use super::project::{ProjectServiceInfo, ProjectServices};
use super::{doc_pictures::DocPics, Display};
use crate::host::project::md_diff::{MdDiffResult, MdDiffWorker};
use crate::view::DocOptions;
mod chrome;
mod discard;
mod media;
#[cfg(test)]
mod preview_tests;
mod project_chrome;
mod rendered;
#[cfg(test)]
mod representation_tests;
#[cfg(any(test, feature = "test-hooks"))]
#[doc(hidden)]
pub mod test_hooks;
use crate::host::project::diff_blocks::{source_diff_blocks, step_change_block, DiffBlockCursor};
use crate::host::project::diff_files::{changed_path_position, step_changed_path};
use crate::host::project::diff_frame::{
    draw_source_diff, source_diff_layout, DiffTitleValues, SourceDiffValues,
};
use crate::host::project::diff_keys::file_diff_keys;
use crate::host::project::diff_presentation::*;
use crate::Failure;
use rendered::RenderedCache;

#[derive(Clone, Debug)]
pub struct FileDiffRequest {
    pub root: PathBuf,
    pub path: PathBuf,
    pub info: ProjectServiceInfo,
}

impl FileDiffRequest {
    /// The default worker, also reusable by hosts that add a reply-publication fence.
    pub fn spawn<S: ProjectServices + ?Sized>(
        self,
        services: Arc<S>,
    ) -> Result<FileDiffReply, Failure> {
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("file-diff".into())
            .spawn(move || {
                let lines = services.file_diff(&self.info, &self.path);
                let paths = crate::host::project::diff_files::ordered_changed_paths(
                    &self.root,
                    services
                        .changed_files(&self.info)
                        .into_iter()
                        .map(|e| e.path)
                        .filter(|p| p.is_file()),
                );
                let _ = tx.send(Ok(FileDiffData { lines, paths }));
            })
            .map_err(|e| Failure::external(e.to_string()))?;
        Ok(rx)
    }
}

pub struct FileDiffData {
    pub lines: Vec<DiffLine>,
    /// Eligible existing files, in the host's sorted review order.
    pub paths: Vec<PathBuf>,
}

pub type FileDiffReply = mpsc::Receiver<Result<FileDiffData, Failure>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffScreenAction {
    Stay,
    Back,
    Quit,
    Edit,
    Open,
    Tool,
    Project,
    Copy(konoma_host_core::keymap::CopyKind),
}

/// Navigation requested by a diff representation; execution belongs to its caller.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiffScreenCommand {
    OpenPreview { path: PathBuf },
}

/// Diff context retained by the Preview executor, separate from file page, caret and pose.
#[derive(Clone, Debug)]
pub struct DiffScreenReturn {
    pub path: PathBuf,
    pub choice: DiffView,
    pub effective: DiffView,
    pub layout: DiffLayout,
    pub scroll: u16,
    pub hscroll: u16,
}

pub struct DiffScreenState {
    discard: discard::Discard,
    pub(in crate::app) project_owned: bool,
    command: Option<DiffScreenCommand>,
    file_previews: Option<crate::host::file_preview::FilePreviewSource>,
    preview_reply: Option<mpsc::Receiver<(crate::host::file_preview::ResolvedFilePreview, bool)>>,
    resolved_preview: Option<(crate::host::file_preview::ResolvedFilePreview, bool)>,
    pub(in crate::app) launch_enabled: bool,
    pub(super) closed: bool,
    pub request: FileDiffRequest,
    pub choice: DiffView,
    /// The executable representation; fallback never overwrites the user's requested view.
    pub effective: DiffView,
    pub layout: DiffLayout,
    pub scroll: u16,
    pub hscroll: u16,
    pub split_drawn: bool,
    pub data: Option<FileDiffData>,
    services: Arc<dyn ProjectServices>,
    pending: Option<FileDiffReply>,
    keys: CoreKeys,
    app_keys: super::AppKeys,
    help: Option<u16>,
    menu: bool,
    extra_menu: Option<konoma_host_core::keymap::CoreMenu>,
    display: Display,
    lang: Lang,
    block: Option<usize>,
    viewport: u16,
    rendered: Option<RenderedCache>,
    /// The first-change reservation belongs to one file, including while it computes.
    pub pending_first_change: Option<PathBuf>,
    /// The first answer of the target resolution (the diff's first load) has not been taken yet:
    /// it reserves the scroll to the first change. Set again by the view key (a new choice).
    first_answer_awaited: bool,
    /// The diff screen is to be read again (a tool came back) but was not in front: covering is
    /// leaving, so the read waits until it is in front again. Any read (`reload`) is this one.
    refresh_held: bool,
    markdown: MdDiffWorker,
    markdown_rx: mpsc::Receiver<MdDiffResult>,
    media: media::Media,
    options: DocOptions,
    notice: Option<String>,
}

impl DiffScreenState {
    pub(in crate::app) fn display(&self) -> &Display {
        &self.display
    }

    pub(in crate::app) fn adopt_display(&mut self, display: Display) {
        if self.display != display {
            self.app_keys = self.app_keys.clone().with_wrap(display.wrap);
            self.display = display;
            self.rendered = None;
        }
    }
    pub fn open(
        request: FileDiffRequest,
        services: Arc<dyn ProjectServices>,
        config: &SharedConfig,
        layout: DiffLayout,
        choice: Option<DiffView>,
    ) -> Result<Self, Failure> {
        Self::open_for_project(request, services, config, layout, choice, None)
    }

    pub(in crate::app) fn open_for_project(
        request: FileDiffRequest,
        services: Arc<dyn ProjectServices>,
        config: &SharedConfig,
        layout: DiffLayout,
        choice: Option<DiffView>,
        source: Option<crate::host::file_preview::FilePreviewSource>,
    ) -> Result<Self, Failure> {
        Self::open_for_project_mode(request, services, config, layout, choice, source, false)
    }

    pub(in crate::app) fn open_for_project_mode(
        request: FileDiffRequest,
        services: Arc<dyn ProjectServices>,
        config: &SharedConfig,
        layout: DiffLayout,
        choice: Option<DiffView>,
        source: Option<crate::host::file_preview::FilePreviewSource>,
        project_owned: bool,
    ) -> Result<Self, Failure> {
        let (tx, rx) = mpsc::channel();
        let mut worker = MdDiffWorker::default();
        worker.attach(tx);
        Self::open_with_workers(
            request,
            services,
            config,
            layout,
            choice,
            (worker, rx),
            (source, project_owned),
        )
    }

    /// Use an attached worker and its receiver, allowing a host's publication fence.
    pub fn open_with_markdown_worker(
        request: FileDiffRequest,
        services: Arc<dyn ProjectServices>,
        config: &SharedConfig,
        layout: DiffLayout,
        choice: Option<DiffView>,
        markdown: MdDiffWorker,
        markdown_rx: mpsc::Receiver<MdDiffResult>,
    ) -> Result<Self, Failure> {
        Self::open_with_workers(
            request,
            services,
            config,
            layout,
            choice,
            (markdown, markdown_rx),
            (None, false),
        )
    }

    fn open_with_workers(
        request: FileDiffRequest,
        services: Arc<dyn ProjectServices>,
        config: &SharedConfig,
        layout: DiffLayout,
        choice: Option<DiffView>,
        markdown: (MdDiffWorker, mpsc::Receiver<MdDiffResult>),
        preview: (Option<crate::host::file_preview::FilePreviewSource>, bool),
    ) -> Result<Self, Failure> {
        let (source, project_owned) = preview;
        let pending = services.clone().spawn_file_diff(request.clone())?;
        let read_context = request.info.backend.diff_read_context();
        let mut keys = file_diff_keys(scheme_from_str(&config.ui.keys), &config.keys);
        if project_owned {
            // Project-only host operations never enter ordinary standalone file apps.
            keys.contexts
                .entry(CoreContext::Diff)
                .or_default()
                .extend(konoma_host_core::keymap::host_ops_table(CoreContext::Diff));
            keys = keys.with_config(
                &config.keys.to_keymap_config(),
                &konoma_host_core::keymap::HostOp::ALL,
                &[("preview_git_diff", CoreContext::Diff)],
            );
        }
        let mut state = Self {
            discard: discard::Discard::new(&request, &*services, config),
            project_owned,
            command: None,
            file_previews: source,
            preview_reply: None,
            resolved_preview: None,
            launch_enabled: false,
            closed: false,
            request,
            services,
            pending: Some(pending),
            data: None,
            choice: choice.unwrap_or_else(|| DiffView::parse(&config.ui.diff_view)),
            effective: DiffView::Source,
            layout,
            scroll: 0,
            hscroll: 0,
            split_drawn: false,
            keys,
            app_keys: super::AppKeys::for_file_diff(config),
            help: None,
            menu: false,
            extra_menu: None,
            display: Display::of(config),
            lang: Lang::parse(&config.ui.lang),
            block: None,
            viewport: 0,
            rendered: None,
            pending_first_change: None,
            first_answer_awaited: true,
            refresh_held: false,
            markdown: markdown.0.with_read_context(read_context),
            markdown_rx: markdown.1,
            media: media::Media::new(&config.git.media_diff),
            options: DocOptions::from_settings(&crate::config::plugin_settings(
                &config.plugins,
                &config.ui.moved(),
                "markdown",
            )),
            notice: None,
        };
        #[cfg(any(test, feature = "test-hooks"))]
        if project_owned {
            state.markdown_rx = super::project::idle_test_hooks::observe_replies(
                &*state.services,
                "rendered",
                state.markdown_rx,
            );
            state.media.observe_replies(&*state.services);
        }
        state.resolve_preview();
        state.choose_effective();
        Ok(state)
    }

    /// Adopt only this receiver's reply. Replacing the receiver drops a stale refresh/open.
    pub fn poll(&mut self) -> Result<bool, Failure> {
        match self.discard.poll() {
            Ok(true) => return Ok(true),
            Err(error) => {
                self.discard_failure(error);
                return Ok(true);
            }
            Ok(false) => {}
        }
        let mut changed = false;
        if let Some(rx) = &self.preview_reply {
            match rx.try_recv() {
                Ok(resolved) => {
                    self.resolved_preview = Some(resolved);
                    self.preview_reply = None;
                    self.choose_effective();
                    changed = true;
                }
                Err(mpsc::TryRecvError::Disconnected) => self.preview_reply = None,
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        let Some(rx) = &self.pending else {
            return Ok(changed);
        };
        let reply = match rx.try_recv() {
            Ok(reply) => reply,
            Err(mpsc::TryRecvError::Empty) => return Ok(changed),
            Err(mpsc::TryRecvError::Disconnected) => {
                Err(Failure::external("file diff worker disconnected"))
            }
        };
        self.pending = None;
        self.data = Some(reply?);
        Ok(true)
    }

    pub fn wake_in(&self) -> Option<Duration> {
        if self.closed {
            return None;
        }
        (self.pending.is_some()
            || self.discard.pending()
            || self.preview_reply.is_some()
            || self.markdown.pending_for(&self.request.path)
            || self.media.pending())
        .then_some(Duration::from_millis(16))
    }

    pub(in crate::app) fn take_preview_lines(
        &mut self,
    ) -> (Option<FileDiffReply>, Option<Vec<DiffLine>>) {
        (
            self.pending.take(),
            self.data.as_ref().map(|d| d.lines.clone()),
        )
    }

    pub(super) fn set_lang(&mut self, lang: Lang) {
        self.lang = lang;
        self.app_keys = self.app_keys.clone().with_lang(lang);
    }

    /// Drop raw ownership on return; retain the last surface context, as konoma does.
    pub(super) fn close(&mut self) {
        self.command = None;
        self.closed = true;
        self.data = None;
        self.pending = None;
        self.preview_reply = None;
        self.resolved_preview = None;
        self.markdown.invalidate();
        self.media.worker.invalidate();
        self.rendered = None;
        self.pending_first_change = None;
    }

    pub(super) fn reload(&mut self) -> Result<(), Failure> {
        self.refresh_held = false;
        self.resolve_preview();
        self.markdown.invalidate();
        self.media.worker.invalidate();
        if !self.project_owned {
            self.media.page = 1;
        }
        self.rendered = None;
        self.choose_effective();
        self.data = None;
        self.pending = Some(
            self.services
                .clone()
                .spawn_file_diff(self.request.clone())?,
        );
        Ok(())
    }

    /// The diff screen stops being in front (a project screen covers it). Covering is leaving,
    /// as in the host (`App::leave_tab_view`): everything its view asked is dropped, so no
    /// answer landing behind the project screen asks a worker again: the target-kind answer
    /// (whose adoption chooses the representation and would start a Markdown block diff), the
    /// raw diff, the Markdown block diff and the media diff. The scroll, the chosen
    /// representation, the media page and the reservation to land on the first change stay;
    /// the return reads it all once (`resume_project_diff`).
    ///
    /// Every field is named, so a new one is decided here.
    pub(in crate::app) fn leave(&mut self) {
        let Self {
            // A confirmed write (a discard) runs to its end, as a write does when a tab is left.
            discard: _,
            // What the diff is, how it is shown and where: the return shows the same.
            project_owned: _,
            file_previews: _,
            launch_enabled: _,
            closed: _,
            request: _,
            choice: _,
            effective: _,
            layout: _,
            scroll: _,
            hscroll: _,
            split_drawn: _,
            keys: _,
            app_keys: _,
            help: _,
            menu: _,
            extra_menu: _,
            display: _,
            lang: _,
            block: _,
            viewport: _,
            options: _,
            notice: _,
            // What was read, answered and laid out: the return reads it again (`reload`).
            data: _,
            resolved_preview: _,
            rendered: _,
            // The landing on the first change: the return lands as the first load would.
            pending_first_change: _,
            first_answer_awaited: _,
            // A held read: taken by the return.
            refresh_held: _,
            // What answers the reads.
            services: _,
            // The receiver: an answer in it is refused by the worker's generation.
            markdown_rx: _,
            // Left by `leave_markdown` and `leave_media` below.
            markdown: _,
            media: _,
            command,
            preview_reply,
            pending,
        } = self;
        // The open of the Preview it wants: the return's read wants it again (`choose_effective`).
        *command = None;
        *preview_reply = None;
        *pending = None;
        self.leave_markdown();
        self.leave_media();
    }

    /// A tool came back: the diff screen is to be read again, when it is in front.
    pub(in crate::app) fn hold_refresh(&mut self) {
        self.refresh_held = true;
    }

    /// The read held by `hold_refresh`, taken by the loop while the diff screen is in front.
    pub(in crate::app) fn take_held_refresh(&mut self) -> bool {
        std::mem::take(&mut self.refresh_held)
    }

    pub(in crate::app) fn resume_project_diff(
        &mut self,
        config: &SharedConfig,
    ) -> Result<(), Failure> {
        // The covered diff is put back as it was: the chosen representation, the media page and
        // the scroll stay; only the data is read again. The reservation to scroll to the first
        // change is the landing of the first load (`first_answer_awaited`): the read for the
        // return, and its asynchronous target answer, keep it as it was at the cover.
        let _ = config;
        self.reload()
    }

    pub fn handle_key(
        &mut self,
        key: KeyPress,
    ) -> Result<(DiffScreenAction, Option<String>), Failure> {
        use konoma_host_core::preview_plan::{fixed_keys, PreviewOp};
        use DiffUiOp as O;
        if self.discard.confirming() {
            return self.handle_discard_key(key);
        }
        if self.project_owned {
            if let Some(action) = self.project_menu_key(key)? {
                return Ok(action);
            }
        }
        if self.menu {
            self.menu = false;
            if let Some(item) = self.options_menu().into_iter().find(|i| i.key == key) {
                return self.execute_op(item.action);
            }
            return Ok((DiffScreenAction::Stay, None));
        }
        if let Some(top) = self.help.as_mut() {
            use konoma_host_core::keymap::CoreResolution;
            match self.keys.help_resolve(key) {
                CoreResolution::Action(CoreAction::Quit) => {
                    return Ok((DiffScreenAction::Quit, None))
                }
                CoreResolution::Action(CoreAction::ToggleHelp | CoreAction::PreviewBack) => {
                    self.help = None
                }
                CoreResolution::Action(CoreAction::Navigate(Motion::Down)) => {
                    *top = top.saturating_add(1)
                }
                CoreResolution::Action(CoreAction::Navigate(Motion::Up)) => {
                    *top = top.saturating_sub(1)
                }
                CoreResolution::Action(CoreAction::Navigate(Motion::Top)) => *top = 0,
                CoreResolution::Action(CoreAction::Navigate(Motion::Bottom)) => *top = u16::MAX,
                _ if key == KeyPress::key(crossterm::event::KeyCode::Esc) => self.help = None,
                _ => {}
            }
            return Ok((DiffScreenAction::Stay, None));
        }
        let fixed = fixed_keys(CoreContext::Diff)
            .into_iter()
            .find(|b| b.key == key)
            .and_then(|b| match b.op {
                PreviewOp::Core(action) => O::from_core(HelpOp::Act(action)),
                PreviewOp::Cancel => Some(O::Cancel),
                _ => None,
            });
        let Some(op) = fixed.or_else(|| self.op_for_key(key)) else {
            return Ok((DiffScreenAction::Stay, None));
        };
        self.execute_op(op)
    }

    fn op_for_key(&self, key: KeyPress) -> Option<DiffUiOp> {
        if self.discard.key(&self.keys, key) {
            Some(DiffUiOp::Discard)
        } else if self.file_previews.is_some()
            && self.keys.host_op(CoreContext::Diff, key)
                == Some(konoma_host_core::keymap::HostOp::CycleRepresentation)
        {
            Some(DiffUiOp::View)
        } else {
            DiffUiOp::resolve_file_diff(&self.keys, key)
        }
    }

    fn execute_op(&mut self, op: DiffUiOp) -> Result<(DiffScreenAction, Option<String>), Failure> {
        use DiffUiOp as O;
        if self.project_owned
            && matches!(op, O::Navigate(_) | O::Layout | O::PageNext | O::PagePrev)
            && !self.presentation().enabled(op)
        {
            return Ok((DiffScreenAction::Stay, None));
        }
        let mut notice = None;
        match op {
            O::Discard if self.discard.enabled => self.discard.confirm(),
            O::Edit if self.launch_enabled => return Ok((DiffScreenAction::Edit, None)),
            O::OpenExternal if self.launch_enabled => return Ok((DiffScreenAction::Open, None)),
            O::Tool if self.launch_enabled => return Ok((DiffScreenAction::Tool, None)),
            O::Project if self.project_owned => return Ok((DiffScreenAction::Project, None)),
            O::Global(1) => self.help = Some(0),
            O::Options => self.menu = !self.options_menu().is_empty(),
            O::PathStyle => self.display.path_style = self.display.path_style.next(),
            O::Back | O::Cancel => return Ok((DiffScreenAction::Back, None)),
            O::Global(2) => return Ok((DiffScreenAction::Quit, None)),
            O::Layout => {
                if self.media_active() {
                    if self.media_showing_pictures() {
                        self.media.layout = self.media.layout.next();
                        notice = Some(self.media_layout_label(self.media.layout).into());
                    }
                    return Ok((DiffScreenAction::Stay, notice));
                }
                if self.effective != DiffView::Source {
                    return Ok((DiffScreenAction::Stay, None));
                }
                self.layout = self.layout.next();
                self.hscroll = 0;
                notice = Some(
                    match self.layout {
                        DiffLayout::Unified => CoreMsg::DiffUnified,
                        DiffLayout::Split => CoreMsg::DiffSideBySide,
                        DiffLayout::Auto => CoreMsg::DiffAuto,
                    }
                    .tr(self.lang)
                    .into(),
                );
            }
            O::View => {
                if let Some(transition) = crate::host::project::diff_keys::diff_view_transition(
                    self.effective,
                    &self.representations(),
                ) {
                    self.choice = transition.choice;
                    if transition.effective != DiffView::Preview {
                        self.scroll = 0;
                        self.block = None;
                    }
                    // A new choice of the reader: its landing is made again.
                    self.first_answer_awaited = true;
                    self.choose_effective();
                }
            }
            O::NextBlock | O::PrevBlock => {
                let blocks = self.blocks();
                if let Some(target) = step_change_block(
                    &blocks,
                    DiffBlockCursor {
                        scroll: self.scroll as usize,
                        block: self.block,
                    },
                    if op == O::NextBlock { 1 } else { -1 },
                ) {
                    self.scroll = target.scroll.min(u16::MAX as usize) as u16;
                    self.block = target.block;
                } else {
                    notice = Some(
                        self.word("no changes in this file", "このファイルに変更はありません")
                            .into(),
                    );
                }
            }
            O::NextFile | O::PrevFile => {
                if let Some(data) = &self.data {
                    if let Some(target) = step_changed_path(
                        &data.paths,
                        &self.request.path,
                        if op == O::NextFile { 1 } else { -1 },
                    ) {
                        if target != self.request.path {
                            self.request.path = target;
                            self.scroll = 0;
                            self.hscroll = 0;
                            self.block = None;
                            // Another file is a new open: its first answer lands.
                            self.first_answer_awaited = true;
                            self.reload()?;
                        }
                    } else {
                        notice = Some(
                            self.word("no changed files", "変更ファイルはありません")
                                .into(),
                        );
                    }
                }
            }
            O::Refresh => self.reload()?,
            O::PageNext | O::PagePrev => {
                self.turn_media_page(if op == O::PageNext { 1 } else { -1 })
            }
            O::Navigate(m) => {
                if matches!(m, Motion::PageDown | Motion::PageUp) && self.media_can_page() {
                    self.turn_media_page(if m == Motion::PageDown { 1 } else { -1 });
                    return Ok((DiffScreenAction::Stay, None));
                }
                if self.media_active()
                    || (self.effective == DiffView::Rendered
                        && self.display.wrap
                        && matches!(
                            m,
                            Motion::Left | Motion::Right | Motion::LineHome | Motion::LineEnd
                        ))
                {
                    return Ok((DiffScreenAction::Stay, None));
                }
                let page = self.viewport.max(1);
                let (v, h) = match m {
                    Motion::Down => (1, 0),
                    Motion::Up => (-1, 0),
                    Motion::Left => (0, -4),
                    Motion::Right => (0, 4),
                    Motion::PageDown => (i32::from(page.saturating_sub(1).max(1)), 0),
                    Motion::PageUp => (-i32::from(page.saturating_sub(1).max(1)), 0),
                    Motion::HalfDown => (i32::from((page / 2).max(1)), 0),
                    Motion::HalfUp => (-i32::from((page / 2).max(1)), 0),
                    Motion::Top => {
                        self.scroll = 0;
                        (0, 0)
                    }
                    Motion::Bottom => {
                        self.scroll = u16::MAX;
                        (0, 0)
                    }
                    Motion::LineHome => {
                        self.hscroll = 0;
                        (0, 0)
                    }
                    Motion::LineEnd => {
                        self.hscroll = u16::MAX;
                        (0, 0)
                    }
                };
                self.scroll = (i32::from(self.scroll) + v).clamp(0, i32::from(u16::MAX)) as u16;
                self.hscroll = (i32::from(self.hscroll) + h).clamp(0, i32::from(u16::MAX)) as u16;
            }
            _ => {}
        }
        Ok((DiffScreenAction::Stay, notice))
    }

    fn word<'a>(&self, en: &'a str, ja: &'a str) -> &'a str {
        if self.lang == Lang::Jp {
            ja
        } else {
            en
        }
    }

    pub(in crate::app) fn take_command(&mut self) -> Option<DiffScreenCommand> {
        self.command.take()
    }

    pub(in crate::app) fn return_from_preview(&mut self) -> Result<(), Failure> {
        let transition = crate::host::project::diff_keys::diff_view_transition(
            DiffView::Preview,
            &self.representations(),
        )
        .expect("Preview returns to Source");
        self.choice = transition.choice;
        self.scroll = 0;
        self.hscroll = 0;
        self.media.page = 1;
        self.block = None;
        // Back to the diff is a new open of it (the host's `open_git_diff_with`): it lands.
        self.first_answer_awaited = true;
        // The gutter may own the previous raw receiver. Reopen on the worker, as the host does.
        self.reload()
    }

    #[cfg(test)]
    pub(in crate::app) fn settle_preview_for_test(&mut self) {
        if let Some(rx) = self.preview_reply.take() {
            self.resolved_preview = Some(rx.recv_timeout(Duration::from_secs(10)).unwrap());
            self.choose_effective();
        }
    }

    fn blocks(&self) -> crate::host::project::diff_blocks::DiffBlocks {
        if self.media_active() {
            return Default::default();
        }
        if self.effective == DiffView::Rendered {
            return self
                .rendered
                .as_ref()
                .map(|c| c.blocks(self.viewport))
                .unwrap_or_default();
        }
        source_diff_blocks(
            self.data
                .as_ref()
                .map(|d| d.lines.as_slice())
                .unwrap_or_default(),
            self.request
                .path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or(""),
            self.split_drawn,
            self.viewport as usize,
        )
    }

    /// The current Source block's new-side line; rendered/media have no source caret.
    pub(in crate::app) fn edit_line(&self) -> Option<usize> {
        if self.media_active() || self.effective != DiffView::Source {
            return None;
        }
        crate::host::project::diff_blocks::source_diff_edit_line(
            &self.data.as_ref()?.lines,
            &self.blocks(),
            DiffBlockCursor {
                scroll: self.scroll as usize,
                block: self.block,
            },
        )
    }

    pub(super) fn draw(&mut self, buf: &mut Buffer, area: Rect) {
        self.viewport = Block::bordered().inner(area).height;
        if self.media_active() {
            self.draw_media(buf, area);
            return;
        }
        if self.effective == DiffView::Rendered {
            self.draw_rendered(buf, area);
            return;
        }
        let path = self
            .display
            .path_style
            .format(&self.request.root, &self.request.path);
        let lines = self
            .data
            .as_ref()
            .map(|d| d.lines.as_slice())
            .unwrap_or_default();
        let position = self
            .data
            .as_ref()
            .and_then(|d| changed_path_position(&d.paths, &self.request.path));
        let summary = self.source_summary().then(|| self.source_summary_text());
        let update = draw_source_diff(
            buf,
            area,
            SourceDiffValues {
                lines,
                extension: self
                    .request
                    .path
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or(""),
                theme: &self.display.theme.code_theme,
                layout: source_diff_layout(self.layout),
                scroll: self.scroll,
                hscroll: self.hscroll,
                title: DiffTitleValues {
                    path: Some(&path),
                    position,
                    ..Default::default()
                },
                empty_text: if self.pending.is_some() || self.preview_reply.is_some() {
                    CoreMsg::Loading.tr(self.lang)
                } else if let Some(summary) = &summary {
                    summary
                } else {
                    self.word("(no changes)", "(変更なし)")
                },
                scroll_words: super::document::scroll_words(self.lang),
            },
        );
        self.scroll = update.scroll;
        self.hscroll = update.hscroll;
        self.split_drawn = update.diff_split_drawn;
    }

    fn presentation(&self) -> DiffPresentation {
        let lookup = CoreLookup {
            keys: &self.keys,
            ctx: CoreContext::Diff,
        };
        let msg = |m: CoreMsg| m.tr(self.lang).to_string();
        let mut bindings: Vec<_> = CoreAction::all()
            .into_iter()
            .filter_map(|a| {
                let op = DiffUiOp::from_core(HelpOp::Act(a))?;
                Some(DiffBinding {
                    op,
                    keys: lookup.keys_of(HelpOp::Act(a)),
                    enabled: (self.project_owned && matches!(op, DiffUiOp::Global(1 | 2)))
                        || (self.launch_enabled
                            && matches!(op, DiffUiOp::Edit | DiffUiOp::OpenExternal)
                            && self
                                .resolved_preview
                                .as_ref()
                                .is_some_and(|(_, exists)| *exists))
                        || matches!(
                            op,
                            DiffUiOp::Navigate(_)
                                | DiffUiOp::Layout
                                | DiffUiOp::Back
                                | DiffUiOp::PageNext
                                | DiffUiOp::PagePrev
                                | DiffUiOp::PathStyle
                        ),
                    label: if op == DiffUiOp::Layout {
                        Some(msg(CoreMsg::FtDiffLayout))
                    } else {
                        default_label(CoreContext::Diff, a).map(msg)
                    },
                    help: if op == DiffUiOp::Layout {
                        Some(DiffMsg::GitLayout.tr(self.lang).into())
                    } else {
                        default_help(CoreContext::Diff, a).map(msg)
                    },
                    launch: matches!(op, DiffUiOp::Edit | DiffUiOp::OpenExternal),
                    layers: vec![],
                })
            })
            .collect();
        bindings.push(DiffBinding {
            op: DiffUiOp::Discard,
            keys: self.discard.keys(&self.keys),
            enabled: self.discard.enabled,
            label: Some(self.word("discard", "破棄").into()),
            help: Some(
                self.word("discard whole file (confirm)", "ファイル全体を破棄(確認)")
                    .into(),
            ),
            launch: false,
            layers: vec![],
        });
        for (op, en, ja) in [
            (DiffUiOp::View, "view", "表現"),
            (DiffUiOp::NextFile, "file", "ファイル"),
            (DiffUiOp::PrevFile, "file", "ファイル"),
            (DiffUiOp::NextBlock, "change", "変更"),
            (DiffUiOp::PrevBlock, "change", "変更"),
            (DiffUiOp::Refresh, "refresh", "再読込"),
            (DiffUiOp::Project, "git", "git"),
            (DiffUiOp::Tool, "tool", "ツール"),
        ] {
            let mut keys: Vec<_> = self
                .keys
                .global
                .keys()
                .chain(self.keys.basic.keys())
                .chain(self.keys.contexts.values().flat_map(|t| t.keys()))
                .chain(self.keys.types.values().flat_map(|t| t.keys()))
                .copied()
                .filter(|k| self.op_for_key(*k) == Some(op))
                .collect();
            keys.sort_by_key(|k| konoma_host_core::help::display_order(*k));
            keys.dedup();
            bindings.push(DiffBinding {
                op,
                keys,
                enabled: match op {
                    DiffUiOp::Project => self.project_owned,
                    DiffUiOp::Tool => self.project_owned && self.launch_enabled,
                    _ => true,
                },
                label: Some(self.word(en, ja).into()),
                help: self
                    .project_owned
                    .then_some(op)
                    .and_then(|op| project_diff_help(op, self.lang)),
                launch: false,
                layers: vec![],
            });
        }
        bindings.push(DiffBinding {
            op: DiffUiOp::Cancel,
            keys: vec![KeyPress::key(crossterm::event::KeyCode::Esc)],
            enabled: true,
            label: None,
            help: None,
            launch: false,
            layers: vec![],
        });
        bindings.push(DiffBinding {
            op: DiffUiOp::Options,
            keys: vec![],
            enabled: true,
            label: None,
            help: None,
            launch: false,
            layers: vec![],
        });
        let mut values = DiffPresentationValues {
            state: DiffPresentationState {
                drawn_surface: self.project_owned.then_some(if self.media_active() {
                    DiffSurface::Media
                } else if self.effective == DiffView::Rendered {
                    DiffSurface::Rendered
                } else if matches!(
                    self.target(),
                    konoma_host_core::project::DiffTarget::Other
                        | konoma_host_core::project::DiffTarget::Svg
                ) && self.data.as_ref().is_none_or(|data| data.lines.is_empty())
                {
                    DiffSurface::Summary
                } else {
                    DiffSurface::Source
                }),
                surface: if self.media_active() {
                    DiffSurface::Media
                } else if self.project_owned
                    && matches!(
                        self.target(),
                        konoma_host_core::project::DiffTarget::Other
                            | konoma_host_core::project::DiffTarget::Svg
                    )
                {
                    DiffSurface::Summary
                } else if self.effective == DiffView::Rendered {
                    DiffSurface::Rendered
                } else {
                    DiffSurface::Source
                },
                wrap: self.display.wrap,
                media_pictures: self.media_showing_pictures(),
                can_page: self.media_can_page(),
                next_media_layout: self.media.layout.next(),
                next_representation: crate::host::project::diff_keys::next_diff_view(
                    self.effective,
                    &self.representations(),
                )
                .map(|v| match v {
                    DiffView::Source => DiffRepresentation::Source,
                    DiffView::Rendered => DiffRepresentation::Rendered,
                    DiffView::Preview => DiffRepresentation::Preview,
                }),
                representation_count: self.representations().len(),
                change_blocks: !self.blocks().rows.is_empty(),
                change_files: self.project_owned
                    || self.data.as_ref().is_some_and(|d| !d.paths.is_empty()),
                ..Default::default()
            },
            capabilities: DiffCapabilities {
                view: true,
                project: self.project_owned,
                discard: self.discard.enabled,
                global: self.project_owned,
                tabs: false,
                follow: false,
                tool: self.project_owned && self.launch_enabled,
                blocks: true,
            },
            bindings,
            words: DiffPresentationWords {
                view: msg(CoreMsg::PreviewGitDiff),
                movement: msg(CoreMsg::HelpMove),
                launch: msg(CoreMsg::HelpLaunch),
                tabs: String::new(),
                global: String::new(),
                words: vec![
                    (DiffWord::Scroll, msg(CoreMsg::Scroll)),
                    (
                        DiffWord::Files,
                        self.word("next/prev file", "次/前の変更").into(),
                    ),
                    (DiffWord::Change, self.word("change", "変更箇所").into()),
                    (DiffWord::Hscroll, msg(CoreMsg::HintHscroll)),
                    (DiffWord::Back, msg(CoreMsg::GitBack)),
                    (
                        DiffWord::BackHelp,
                        DiffMsg::BackToGitView.tr(self.lang).into(),
                    ),
                    (
                        DiffWord::ToolHelp,
                        project_diff_help(DiffUiOp::Tool, self.lang).unwrap(),
                    ),
                    (DiffWord::Options, msg(CoreMsg::HintOptions)),
                    (
                        DiffWord::HorizontalEnds,
                        DiffMsg::GitHScrollEnds.tr(self.lang).into(),
                    ),
                ],
                dynamic: vec![
                    (
                        DiffTextId::Files,
                        self.word("next/prev file", "次/前の変更").into(),
                    ),
                    (
                        DiffTextId::MediaLayout(self.media.layout.next()),
                        self.media_layout_label(self.media.layout.next()).into(),
                    ),
                    (
                        DiffTextId::MediaLayoutHelp,
                        self.word(
                            "cycle layout (auto → side by side → stacked → auto)",
                            "並べ方を切替（自動→左右→上下→自動）",
                        )
                        .into(),
                    ),
                    (
                        DiffTextId::Representation(DiffRepresentation::Source),
                        self.word("source", "ソース").into(),
                    ),
                    (
                        DiffTextId::Representation(DiffRepresentation::Rendered),
                        self.word("rendered", "整形").into(),
                    ),
                    (
                        DiffTextId::Representation(DiffRepresentation::Preview),
                        self.word("preview", "プレビュー").into(),
                    ),
                    (
                        DiffTextId::CycleAll,
                        self.word(
                            "cycle diff view (source → rendered → preview)",
                            "表現を切り替え（ソース → 整形 → プレビュー）",
                        )
                        .into(),
                    ),
                    (
                        DiffTextId::CyclePair,
                        if self.representations().contains(&DiffView::Preview) {
                            self.word(
                                "cycle diff view (source ⇄ preview)",
                                "表現を切り替え（ソース ⇄ プレビュー）",
                            )
                        } else {
                            self.word(
                                "cycle diff view (source ⇄ rendered)",
                                "表現を切り替え（ソース ⇄ 整形）",
                            )
                        }
                        .into(),
                    ),
                ],
            },
        };
        let options = self.options_declaration().iter().any(|item| {
            values.state.enabled(values.capabilities, item.action)
                && values
                    .bindings
                    .iter()
                    .any(|b| b.op == item.action && b.enabled)
        });
        if let Some(binding) = values
            .bindings
            .iter_mut()
            .find(|b| b.op == DiffUiOp::Options)
        {
            binding.enabled = options;
        }
        self.app_keys.diff_presentation(&values)
    }

    pub(super) fn context_spans(&self) -> Vec<ratatui::text::Span<'static>> {
        use ratatui::{
            style::{Color, Style, Stylize},
            text::Span,
        };
        let chip = |s: &str, bg| {
            Span::styled(
                format!(" {s} "),
                Style::new().fg(Color::White).bg(bg).bold(),
            )
        };
        let path = match self.display.path_style {
            konoma_host_core::view_options::PathStyle::Relative => self.word("rel", "相対"),
            konoma_host_core::view_options::PathStyle::Full => self.word("abs", "絶対"),
            _ => "~",
        };
        vec![
            Span::styled(
                match self.request.info.backend {
                    super::project::ProjectBackend::Git => self.word(" GIT ", " Git "),
                    super::project::ProjectBackend::Jj => self.word(" JJ ", " jj "),
                },
                Style::new()
                    .fg(Color::Black)
                    .bg(match self.request.info.backend {
                        super::project::ProjectBackend::Git => Color::Yellow,
                        super::project::ProjectBackend::Jj => Color::Magenta,
                    })
                    .bold(),
            ),
            Span::raw(" "),
            if self.discard.confirming() {
                chip(self.word("DELETE", "削除確認"), Color::Red)
            } else if self.help.is_some() && self.project_owned {
                chip(self.word("HELP", "ヘルプ"), Color::DarkGray)
            } else {
                chip(self.word("DIFF", "差分"), Color::Blue)
            },
            Span::raw(format!("  {}{path}", self.word("path:", "パス:"))).dim(),
        ]
    }

    #[cfg(test)]
    pub(super) fn draw_header(&self, buf: &mut Buffer, area: Rect) {
        crate::app::draw::draw_project_header(buf, area, self.context_spans());
    }

    pub(super) fn draw_chrome(
        &self,
        buf: &mut Buffer,
        area: Rect,
        footer: &[ratatui::text::Span<'static>],
    ) {
        crate::app::draw::draw_project_status(
            buf,
            area,
            self.project_owned && self.display.combined_status,
            self.context_spans(),
            footer,
        );
    }
}
