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
mod media;
#[cfg(test)]
mod preview_tests;
mod rendered;
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
}

/// Reserved navigation boundary. Preview is not an executable capability yet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiffScreenCommand {
    OpenPreview { path: PathBuf },
}

/// Context an eventual Preview executor must retain when returning to the file diff.
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
    display: Display,
    lang: Lang,
    block: Option<usize>,
    viewport: u16,
    rendered: Option<RenderedCache>,
    /// The first-change reservation belongs to one file, including while it computes.
    pub pending_first_change: Option<PathBuf>,
    markdown: MdDiffWorker,
    markdown_rx: mpsc::Receiver<MdDiffResult>,
    media: media::Media,
    options: DocOptions,
    notice: Option<String>,
}

impl DiffScreenState {
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
            source,
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
            None,
        )
    }

    fn open_with_workers(
        request: FileDiffRequest,
        services: Arc<dyn ProjectServices>,
        config: &SharedConfig,
        layout: DiffLayout,
        choice: Option<DiffView>,
        markdown: (MdDiffWorker, mpsc::Receiver<MdDiffResult>),
        source: Option<crate::host::file_preview::FilePreviewSource>,
    ) -> Result<Self, Failure> {
        let pending = services.clone().spawn_file_diff(request.clone())?;
        let read_context = request.info.backend.diff_read_context();
        let mut state = Self {
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
            keys: file_diff_keys(scheme_from_str(&config.ui.keys), &config.keys),
            app_keys: super::AppKeys::for_file_diff(config),
            help: None,
            menu: false,
            display: Display::of(config),
            lang: Lang::parse(&config.ui.lang),
            block: None,
            viewport: 0,
            rendered: None,
            pending_first_change: None,
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
        state.resolve_preview();
        state.choose_effective();
        Ok(state)
    }

    /// Adopt only this receiver's reply. Replacing the receiver drops a stale refresh/open.
    pub fn poll(&mut self) -> Result<bool, Failure> {
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
            || self.preview_reply.is_some()
            || self.markdown.pending_for(&self.request.path)
            || self.media.pending())
        .then_some(Duration::from_millis(16))
    }

    pub(super) fn set_lang(&mut self, lang: Lang) {
        self.lang = lang;
        self.app_keys = self.app_keys.clone().with_lang(lang);
    }

    /// Drop raw ownership on return; retain the last surface context, as konoma does.
    pub(super) fn close(&mut self) {
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
        self.resolve_preview();
        self.markdown.invalidate();
        self.media.worker.invalidate();
        self.media.page = 1;
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

    pub fn handle_key(
        &mut self,
        key: KeyPress,
    ) -> Result<(DiffScreenAction, Option<String>), Failure> {
        use konoma_host_core::preview_plan::{fixed_keys, PreviewOp};
        use DiffUiOp as O;
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
        let Some(op) = fixed.or_else(|| O::resolve_file_diff(&self.keys, key)) else {
            return Ok((DiffScreenAction::Stay, None));
        };
        self.execute_op(op)
    }

    fn execute_op(&mut self, op: DiffUiOp) -> Result<(DiffScreenAction, Option<String>), Failure> {
        use DiffUiOp as O;
        let mut notice = None;
        match op {
            O::Edit if self.launch_enabled => return Ok((DiffScreenAction::Edit, None)),
            O::OpenExternal if self.launch_enabled => return Ok((DiffScreenAction::Open, None)),
            O::Tool if self.launch_enabled => return Ok((DiffScreenAction::Tool, None)),
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
                if let Some(next) = crate::host::project::diff_keys::next_diff_view(
                    self.effective,
                    &self.representations(),
                ) {
                    self.choice = next;
                    self.scroll = 0;
                    self.block = None;
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
                empty_text: if self.pending.is_some() {
                    CoreMsg::Loading.tr(self.lang)
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
                    enabled: matches!(
                        op,
                        DiffUiOp::Navigate(_)
                            | DiffUiOp::Layout
                            | DiffUiOp::Back
                            | DiffUiOp::PageNext
                            | DiffUiOp::PagePrev
                            | DiffUiOp::PathStyle
                    ) && !(self.effective == DiffView::Rendered
                        && (self.media_active() || self.display.wrap)
                        && matches!(
                            op,
                            DiffUiOp::Navigate(
                                Motion::Left | Motion::Right | Motion::LineHome | Motion::LineEnd
                            )
                        )),
                    label: if op == DiffUiOp::Layout {
                        Some(msg(CoreMsg::FtDiffLayout))
                    } else {
                        default_label(CoreContext::Diff, a).map(msg)
                    },
                    help: default_help(CoreContext::Diff, a).map(msg),
                    launch: false,
                    layers: vec![],
                })
            })
            .collect();
        for (op, en, ja) in [
            (DiffUiOp::View, "view", "表現"),
            (DiffUiOp::NextFile, "file", "ファイル"),
            (DiffUiOp::PrevFile, "file", "ファイル"),
            (DiffUiOp::NextBlock, "change", "変更"),
            (DiffUiOp::PrevBlock, "change", "変更"),
            (DiffUiOp::Refresh, "refresh", "再読込"),
        ] {
            let mut keys: Vec<_> = self
                .keys
                .global
                .keys()
                .chain(self.keys.basic.keys())
                .chain(self.keys.contexts.values().flat_map(|t| t.keys()))
                .chain(self.keys.types.values().flat_map(|t| t.keys()))
                .copied()
                .filter(|k| DiffUiOp::resolve_file_diff(&self.keys, *k) == Some(op))
                .collect();
            keys.sort_by_key(|k| konoma_host_core::help::display_order(*k));
            keys.dedup();
            bindings.push(DiffBinding {
                op,
                keys,
                enabled: true,
                label: Some(self.word(en, ja).into()),
                help: None,
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
                surface: if self.media_active() {
                    DiffSurface::Media
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
                change_files: self.data.as_ref().is_some_and(|d| !d.paths.is_empty()),
                ..Default::default()
            },
            capabilities: DiffCapabilities {
                view: true,
                project: false,
                discard: false,
                global: false,
                tabs: false,
                follow: false,
                tool: false,
                blocks: true,
            },
            bindings,
            words: DiffPresentationWords {
                view: msg(CoreMsg::PreviewGitDiff),
                movement: msg(CoreMsg::HelpMove),
                launch: String::new(),
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
                    (DiffWord::Options, msg(CoreMsg::HintOptions)),
                    (
                        DiffWord::HorizontalEnds,
                        DiffMsg::GitHScrollEnds.tr(self.lang).into(),
                    ),
                ],
                dynamic: vec![
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
                        DiffTextId::CyclePair,
                        self.word(
                            "cycle diff view (source ⇄ rendered)",
                            "表現を切り替え（ソース ⇄ 整形）",
                        )
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

    pub(super) fn draw_header(&self, buf: &mut Buffer, area: Rect) {
        use ratatui::{
            layout::Alignment,
            style::{Color, Style, Stylize},
            text::{Line, Span},
            widgets::{Paragraph, Widget},
        };
        if area.height == 0 {
            return;
        }
        let chip = |s: &str| {
            Span::styled(
                format!(" {s} "),
                Style::new().fg(Color::White).bg(Color::Blue).bold(),
            )
        };
        let path = match self.display.path_style {
            konoma_host_core::view_options::PathStyle::Relative => self.word("rel", "相対"),
            konoma_host_core::view_options::PathStyle::Full => self.word("abs", "絶対"),
            _ => "~",
        };
        Paragraph::new(Line::from(vec![
            Span::styled(
                match self.request.info.backend {
                    super::project::ProjectBackend::Git => " GIT ",
                    super::project::ProjectBackend::Jj => " JJ ",
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
            chip(self.word("DIFF", "差分")),
            Span::raw(format!("  {}{path}", self.word("path:", "パス:"))).dim(),
        ]))
        .alignment(Alignment::Right)
        .render(Rect::new(area.x, area.y, area.width, 1), buf);
    }
}
